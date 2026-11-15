//! Windows child ownership, established before the first instruction executes.
use std::{
    io::{self, Read, Write},
    os::windows::{io::AsRawHandle, process::CommandExt},
    process::{Child, Command, Output, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE},
    System::{Diagnostics::ToolHelp::*, JobObjects::*, Threading::*},
};

struct Job(HANDLE);
impl Drop for Job {
    fn drop(&mut self) {
        // SAFETY: the job contains only this call's processes. Explicit cleanup
        // does not depend on whether a child duplicated a job handle; kill-on-close
        // also protects the tree if the owning process exits unexpectedly.
        unsafe {
            TerminateJobObject(self.0, 1);
            CloseHandle(self.0);
        }
    }
}
impl Job {
    fn stop(&self) -> io::Result<()> {
        // SAFETY: this handle owns only the call's child tree. Querying the job
        // waits for all its processes, including children that closed their pipes.
        unsafe {
            if TerminateJobObject(self.0, 1) == 0 {
                return Err(io::Error::last_os_error());
            }
        }
        let start = Instant::now();
        loop {
            let mut info = JOBOBJECT_BASIC_ACCOUNTING_INFORMATION::default();
            // SAFETY: info is initialized, exclusively borrowed and exactly the
            // size of the selected information class.
            let queried = unsafe {
                QueryInformationJobObject(
                    self.0,
                    JobObjectBasicAccountingInformation,
                    (&mut info as *mut JOBOBJECT_BASIC_ACCOUNTING_INFORMATION).cast(),
                    std::mem::size_of_val(&info) as u32,
                    std::ptr::null_mut(),
                )
            };
            if queried == 0 {
                return Err(io::Error::last_os_error());
            }
            if info.ActiveProcesses == 0 {
                return Ok(());
            }
            if start.elapsed() >= Duration::from_secs(2) {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "managed subprocess cleanup deadline exceeded",
                ));
            }
            thread::sleep(Duration::from_millis(5));
        }
    }
    fn contain(child: &Child) -> io::Result<Self> {
        // SAFETY: all structures have their documented sizes; the process is
        // still suspended, and every newly opened handle is closed here or owned.
        unsafe {
            let handle = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if handle.is_null() {
                return Err(io::Error::last_os_error());
            }
            let job = Self(handle);
            let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            if SetInformationJobObject(
                handle,
                JobObjectExtendedLimitInformation,
                (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                std::mem::size_of_val(&limits) as u32,
            ) == 0
                || AssignProcessToJobObject(handle, child.as_raw_handle().cast()) == 0
            {
                return Err(io::Error::last_os_error());
            }
            let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
            if snapshot == INVALID_HANDLE_VALUE {
                return Err(io::Error::last_os_error());
            }
            let mut entry = THREADENTRY32 {
                dwSize: std::mem::size_of::<THREADENTRY32>() as u32,
                ..Default::default()
            };
            let mut found = Thread32First(snapshot, &mut entry);
            let mut resumed = false;
            while found != 0 {
                if entry.th32OwnerProcessID == child.id() {
                    let thread = OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID);
                    if !thread.is_null() {
                        resumed = ResumeThread(thread) != u32::MAX;
                        CloseHandle(thread);
                    }
                    break;
                }
                found = Thread32Next(snapshot, &mut entry);
            }
            CloseHandle(snapshot);
            if !resumed {
                return Err(io::Error::other("could not resume owned child"));
            }
            Ok(job)
        }
    }
}
struct Owner {
    child: Child,
    job: Option<Job>,
}
impl Drop for Owner {
    fn drop(&mut self) {
        self.job.take();
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
fn reader(
    mut pipe: impl Read + Send + 'static,
    exceeded: Arc<AtomicBool>,
    limit: usize,
) -> thread::JoinHandle<io::Result<Vec<u8>>> {
    thread::spawn(move || {
        let mut bytes = Vec::new();
        let mut buffer = [0u8; 8192];
        loop {
            let read = match pipe.read(&mut buffer) {
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                other => other?,
            };
            if read == 0 {
                return Ok(bytes);
            }
            if bytes.len().saturating_add(read) > limit {
                exceeded.store(true, Ordering::Release);
                return Err(io::Error::other("managed subprocess output limit exceeded"));
            }
            bytes.extend_from_slice(&buffer[..read]);
        }
    })
}
pub(super) fn capture(
    command: &mut Command,
    input: Option<Vec<u8>>,
    timeout: Duration,
    limit: usize,
    combined: bool,
) -> io::Result<Output> {
    command.creation_flags(CREATE_NO_WINDOW | CREATE_SUSPENDED);
    let merged = if combined {
        let (read, write) = io::pipe()?;
        command.stdout(write.try_clone()?).stderr(write);
        Some(read)
    } else {
        command.stdout(Stdio::piped()).stderr(Stdio::piped());
        None
    };
    if input.is_some() {
        command.stdin(Stdio::piped());
    }
    let mut child = command.spawn()?;
    command.stdout(Stdio::null()).stderr(Stdio::null());
    let job = match Job::contain(&child) {
        Ok(job) => job,
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
    };
    let mut owner = Owner {
        child,
        job: Some(job),
    };
    let exceeded = Arc::new(AtomicBool::new(false));
    let stdout = if let Some(pipe) = merged {
        reader(pipe, exceeded.clone(), limit)
    } else {
        reader(
            owner.child.stdout.take().expect("piped stdout"),
            exceeded.clone(),
            limit,
        )
    };
    let stderr = owner
        .child
        .stderr
        .take()
        .map(|pipe| reader(pipe, exceeded.clone(), limit));
    let writer = input.and_then(|bytes| {
        owner.child.stdin.take().map(|mut pipe| {
            thread::spawn(move || {
                let _ = pipe.write_all(&bytes);
            })
        })
    });
    let start = Instant::now();
    let failure = loop {
        if exceeded.load(Ordering::Acquire) {
            break Some(io::Error::other("managed subprocess output limit exceeded"));
        }
        match owner.child.try_wait() {
            Ok(Some(_)) => break None,
            Ok(None) => (),
            Err(error) => break Some(error),
        }
        if start.elapsed() >= timeout {
            break Some(io::Error::new(
                io::ErrorKind::TimedOut,
                "managed subprocess deadline exceeded",
            ));
        }
        thread::sleep(Duration::from_millis(5));
    };
    // Close the job before joining either pipe: descendants may hold its write
    // ends after the leader exits. No unrelated process handle is targeted.
    let cleanup = match owner.job.take() {
        Some(job) => job.stop(),
        None => Ok(()),
    };
    let status = owner.child.wait();
    if let Some(writer) = writer {
        let _ = writer.join();
    }
    let stdout = stdout
        .join()
        .map_err(|_| io::Error::other("subprocess stdout reader panicked"))?;
    let stderr = match stderr {
        Some(reader) => reader
            .join()
            .map_err(|_| io::Error::other("subprocess stderr reader panicked"))?,
        None => Ok(Vec::new()),
    };
    cleanup?;
    if let Some(error) = failure {
        return Err(error);
    }
    Ok(Output {
        status: status?,
        stdout: stdout?,
        stderr: stderr?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        path::{Path, PathBuf},
        time::{SystemTime, UNIX_EPOCH},
    };
    fn powershell(script: &str) -> Command {
        let root = std::env::var_os("SystemRoot").expect("Windows SystemRoot");
        let program = Path::new(&root).join("System32/WindowsPowerShell/v1.0/powershell.exe");
        let mut command = Command::new(program);
        command.creation_flags(CREATE_NO_WINDOW);
        command
            .args(["-NoProfile", "-NonInteractive", "-Command", script])
            .env_clear()
            .stdin(Stdio::null());
        for name in ["SystemRoot", "WINDIR", "TEMP", "TMP", "COMSPEC"] {
            if let Some(value) = std::env::var_os(name) {
                command.env(name, value);
            }
        }
        command
    }
    #[test]
    fn windows_streams_status_and_combined_order_are_preserved() {
        let script = "[Console]::Out.Write('A'); [Console]::Error.Write('B'); [Console]::Out.Write('C'); exit 7";
        let out = capture(
            &mut powershell(script),
            None,
            Duration::from_secs(10),
            1024,
            false,
        )
        .unwrap();
        assert_eq!(out.status.code(), Some(7));
        assert_eq!(out.stdout, b"AC");
        assert_eq!(out.stderr, b"B");
        let out = capture(
            &mut powershell(script),
            None,
            Duration::from_secs(10),
            1024,
            true,
        )
        .unwrap();
        assert_eq!(out.status.code(), Some(7));
        assert_eq!(out.stdout, b"ABC");
        assert!(out.stderr.is_empty());
    }
    #[test]
    fn windows_deadline_covers_blocked_stdin_and_output_is_bounded() {
        let start = Instant::now();
        let error = capture(
            &mut powershell("Start-Sleep -Seconds 20"),
            Some(vec![b'x'; 1024 * 1024]),
            Duration::from_millis(150),
            1024,
            false,
        )
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert!(start.elapsed() < Duration::from_secs(5));
        let error = capture(
            &mut powershell("while ($true) { [Console]::Out.Write('payload') }"),
            None,
            Duration::from_secs(10),
            1024,
            true,
        )
        .unwrap_err();
        assert!(error.to_string().contains("output limit"));
    }
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "luuna-process-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    struct ChildGuard(Child);
    impl Drop for ChildGuard {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    fn quote(path: &Path) -> String {
        format!("'{}'", path.to_string_lossy().replace('\'', "''"))
    }
    fn script_file(path: &Path, script: &str) {
        let mut bytes = vec![0xff, 0xfe];
        bytes.extend(script.encode_utf16().flat_map(u16::to_le_bytes));
        fs::write(path, bytes).unwrap();
    }
    #[test]
    fn windows_completion_kills_started_descendants_and_keeps_unrelated_processes() {
        let fixture = Fixture::new();
        let ready = fixture.0.join("ready");
        let late = fixture.0.join("late");
        let child = fixture.0.join("child.ps1");
        script_file(&child, &format!("[IO.File]::WriteAllText({}, 'ready')\nStart-Sleep -Milliseconds 1500\n[IO.File]::WriteAllText({}, 'late')\n", quote(&ready), quote(&late)));
        let child_argument = format!("'\"{}\"'", child.to_string_lossy().replace('\'', "''"));
        let parent = format!("Start-Process -FilePath (Join-Path $PSHOME 'powershell.exe') -ArgumentList @('-NoProfile','-NonInteractive','-ExecutionPolicy','Bypass','-File',{child_argument}) -NoNewWindow; $deadline = [DateTime]::UtcNow.AddSeconds(5); while (-not [IO.File]::Exists({}) -and [DateTime]::UtcNow -lt $deadline) {{ Start-Sleep -Milliseconds 20 }}; if (-not [IO.File]::Exists({})) {{ throw 'descendant did not start' }}; [Console]::Out.Write('done')", quote(&ready), quote(&ready));
        let mut unrelated = ChildGuard(
            powershell("Start-Sleep -Seconds 20")
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        );
        let out = capture(
            &mut powershell(&parent),
            None,
            Duration::from_secs(10),
            4096,
            false,
        )
        .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(out.stdout, b"done");
        assert!(ready.is_file(), "the descendant really started");
        thread::sleep(Duration::from_secs(2));
        assert!(
            !late.exists(),
            "owned descendant must not write after completion"
        );
        assert!(unrelated.0.try_wait().unwrap().is_none());
    }
}
