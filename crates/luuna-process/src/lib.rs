//! Finite subprocess capture with ownership of the child process tree.
#[cfg(windows)]
mod windows;
use std::{
    io,
    process::{Command, Output},
    time::Duration,
};
#[cfg(unix)]
use std::{
    io::{Read, Write},
    process::{Child, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
    time::Instant,
};

#[cfg(unix)]
fn configure(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    // SAFETY: setsid is async-signal-safe and accesses no parent memory.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() < 0 {
                Err(io::Error::last_os_error())
            } else {
                Ok(())
            }
        });
    }
}

#[cfg(unix)]
struct Owner(Child, bool);
#[cfg(unix)]
impl Owner {
    fn kill_group(&mut self) {
        if self.1 {
            return;
        }
        // SAFETY: the leader is not reaped until after this call, so its session ID
        // cannot have been reassigned. Only this child's process group is targeted.
        unsafe {
            libc::kill(-(self.0.id() as libc::pid_t), libc::SIGKILL);
        }
    }
    fn exited(&mut self) -> io::Result<bool> {
        // SAFETY: siginfo is initialized and exclusively borrowed. WNOWAIT keeps the
        // leader's PID reserved until group cleanup and the subsequent Child::wait.
        unsafe {
            let mut info: libc::siginfo_t = std::mem::zeroed();
            if libc::waitid(
                libc::P_PID,
                self.0.id(),
                &mut info,
                libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
            ) < 0
            {
                return Err(io::Error::last_os_error());
            }
            Ok(info.si_pid() != 0)
        }
    }
}
#[cfg(unix)]
impl Drop for Owner {
    fn drop(&mut self) {
        self.kill_group();
        let _ = self.0.wait();
    }
}

#[cfg(unix)]
fn reader<R: Read + Send + std::os::fd::AsRawFd + 'static>(
    mut pipe: R,
    exceeded: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    limit: usize,
) -> thread::JoinHandle<io::Result<Vec<u8>>> {
    nonblocking(&pipe);
    thread::spawn(move || {
        let mut bytes = Vec::new();
        let mut buf = [0; 8192];
        loop {
            let n = match pipe.read(&mut buf) {
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    if stop.load(Ordering::Acquire) {
                        return Ok(bytes);
                    }
                    thread::sleep(Duration::from_millis(5));
                    continue;
                }
                other => other?,
            };
            if n == 0 {
                return Ok(bytes);
            }
            if bytes.len().saturating_add(n) > limit {
                exceeded.store(true, Ordering::Release);
                return Err(io::Error::other("managed subprocess output limit exceeded"));
            }
            bytes.extend_from_slice(&buf[..n]);
        }
    })
}

#[cfg(unix)]
fn nonblocking(pipe: &impl std::os::fd::AsRawFd) {
    // SAFETY: the owned descriptor remains live for both fcntl calls.
    unsafe {
        let fd = pipe.as_raw_fd();
        let flags = libc::fcntl(fd, libc::F_GETFL);
        if flags >= 0 {
            libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK);
        }
    }
}

pub fn capture(
    command: &mut Command,
    input: Option<Vec<u8>>,
    timeout: Duration,
    limit: usize,
) -> io::Result<Output> {
    capture_impl(command, input, timeout, limit, false)
}

/// Capture stdout and stderr in the same pipe, preserving their write order.
pub fn capture_combined(
    command: &mut Command,
    input: Option<Vec<u8>>,
    timeout: Duration,
    limit: usize,
) -> io::Result<Output> {
    capture_impl(command, input, timeout, limit, true)
}

fn capture_impl(
    command: &mut Command,
    input: Option<Vec<u8>>,
    timeout: Duration,
    limit: usize,
    combined: bool,
) -> io::Result<Output> {
    #[cfg(windows)]
    {
        windows::capture(command, input, timeout, limit, combined)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (command, input, timeout, limit, combined);
        Err(io::Error::other(
            "managed subprocess ownership is unavailable on this platform",
        ))
    }
    #[cfg(unix)]
    {
        configure(command);
        let merged = if combined {
            let (reader, writer) = io::pipe()?;
            command.stdout(writer.try_clone()?).stderr(writer);
            Some(reader)
        } else {
            command.stdout(Stdio::piped()).stderr(Stdio::piped());
            None
        };
        if input.is_some() {
            command.stdin(Stdio::piped());
        }
        let mut owner = Owner(command.spawn()?, false);
        // Custom pipe handles otherwise remain held by the reusable Command,
        // preventing the reader from ever observing EOF.
        command.stdout(Stdio::null()).stderr(Stdio::null());
        let stop = Arc::new(AtomicBool::new(false));
        let writer_stop = stop.clone();
        let writer = input.and_then(|bytes| {
            owner.0.stdin.take().map(|mut pipe| {
                nonblocking(&pipe);
                thread::spawn(move || {
                    let mut left = bytes.as_slice();
                    while !left.is_empty() && !writer_stop.load(Ordering::Acquire) {
                        match pipe.write(left) {
                            Ok(0) => break,
                            Ok(n) => left = &left[n..],
                            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                                thread::sleep(Duration::from_millis(5))
                            }
                            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                            Err(_) => break,
                        }
                    }
                })
            })
        });
        let exceeded = Arc::new(AtomicBool::new(false));
        let stdout = if let Some(pipe) = merged {
            reader(pipe, exceeded.clone(), stop.clone(), limit)
        } else {
            reader(
                owner.0.stdout.take().expect("piped stdout"),
                exceeded.clone(),
                stop.clone(),
                limit,
            )
        };
        let stderr = owner
            .0
            .stderr
            .take()
            .map(|pipe| reader(pipe, exceeded.clone(), stop.clone(), limit));
        let start = Instant::now();
        let failure = loop {
            if exceeded.load(Ordering::Acquire) {
                break Some(io::Error::other("managed subprocess output limit exceeded"));
            }
            match owner.exited() {
                Ok(true) => break None,
                Ok(false) => {}
                Err(e) => break Some(e),
            }
            if start.elapsed() >= timeout {
                break Some(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "managed subprocess deadline exceeded",
                ));
            }
            thread::sleep(Duration::from_millis(5));
        };
        owner.kill_group();
        let status = owner.0.wait();
        // The leader has now been reaped. Do not signal its numeric ID in Drop.
        owner.1 = true;
        stop.store(true, Ordering::Release);
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
        if let Some(error) = failure {
            return Err(error);
        }
        Ok(Output {
            status: status?,
            stdout: stdout?,
            stderr: stderr?,
        })
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    #[test]
    fn deadline_covers_blocked_input_and_kills_the_process_group() {
        let mut command = Command::new("sh");
        command.args(["-c", "sleep 20 & wait"]);
        let started = Instant::now();
        let error = capture(
            &mut command,
            Some(vec![b'x'; 1024 * 1024]),
            Duration::from_millis(100),
            1024,
        )
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert!(started.elapsed() < Duration::from_secs(3));
    }
    #[test]
    fn both_streams_are_captured_and_output_overflow_refuses() {
        let mut command = Command::new("sh");
        command.args(["-c", "printf output; printf diagnostic >&2; exit 7"]);
        let out = capture(&mut command, None, Duration::from_secs(2), 1024).unwrap();
        assert_eq!(out.status.code(), Some(7));
        assert_eq!(out.stdout, b"output");
        assert_eq!(out.stderr, b"diagnostic");
        let mut command = Command::new("sh");
        command.args(["-c", "yes overflow"]);
        assert!(capture(&mut command, None, Duration::from_secs(2), 1024)
            .unwrap_err()
            .to_string()
            .contains("output limit"));
    }
}

#[cfg(all(test, unix))]
mod combined_tests {
    use super::*;
    #[test]
    fn combined_capture_preserves_stream_write_order_and_exit_status() {
        let mut command = Command::new("sh");
        command.args(["-c", "printf A; printf B >&2; printf C; exit 7"]);
        let out = capture_combined(&mut command, None, Duration::from_secs(2), 1024).unwrap();
        assert_eq!(out.stdout, b"ABC");
        assert!(out.stderr.is_empty());
        assert_eq!(out.status.code(), Some(7));
    }
}
