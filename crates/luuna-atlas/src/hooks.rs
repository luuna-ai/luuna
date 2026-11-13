//! Running the configured hooks (`sh -c`, process group, diagnostics).
//! Owner: s5-atlas-lib-2.
//!
//! A port of the Go tool's `hooks.go` (`RunHooks`, `runOneHook`, `hookDiagnostic`) and
//! `hooks_process_unix.go` (setsid, group SIGKILL). Hooks run the config text through
//! `sh -c` in `ctx.cwd` with the caller's whole environment plus `LUUNA_ATLAS_HOOK=1`, as
//! in Go (P3); `sh` is found on `ctx.env`'s `PATH` (P25). On non-unix systems only the
//! hook child itself is killed on timeout (D5).

use std::io::{Read, Write};
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::config::{self, Config};
use crate::error::{AtlasError, Result};
use crate::json::{self, Object, Value};
use crate::options::{Context, Env};
use crate::records::{list, number, obj};
use crate::store::json_parse_error;

/// The timeout when `hook_settings.timeout_ms` is missing or not above zero.
const DEFAULT_TIMEOUT_MS: f64 = 5000.0;

/// Go's `cmd.WaitDelay`: how long, after the hook process ends, its output pipes may stay
/// open (held by a background child) before they are abandoned.
const WAIT_DELAY: Duration = Duration::from_millis(100);

/// The longest pause between two checks of whether the hook process has ended.
const MAX_POLL: Duration = Duration::from_millis(10);

/// The hook events, in Go's order.
pub const HOOK_EVENTS: [&str; 5] = [
    "pre-record",
    "post-record",
    "pre-prime",
    "pre-prune",
    "pre-compact",
];

/// One hook run (`HookExecution`).
#[derive(Debug, Clone, Default)]
pub struct HookExecution {
    /// The script as configured.
    pub command: String,
    /// The exit code; 1 when the hook could not start, 124 on timeout.
    pub exit_code: i32,
    pub stderr: String,
    pub stdout: String,
    pub duration_ms: i64,
    pub timed_out: bool,
}

/// What a run of an event's hooks produced (`HookResult`).
#[derive(Debug, Clone)]
pub struct HookResult {
    /// The payload after mutations by `pre-record`, `pre-prime` and `pre-compact` hooks.
    pub payload: Value,
    /// A `pre-*` hook failed; the command stops.
    pub blocked: bool,
    pub block_reason: String,
    /// Failures of `post-*` hooks and ignored mutations.
    pub warnings: Vec<String>,
    pub ran_any: bool,
    pub executions: Vec<HookExecution>,
}

/// `RunHooks(cwd, cfg, event, payload, stderr)`: runs the scripts configured for `event`
/// in order, in `ctx.cwd`.
///
/// An unknown event is refused (`Unknown hook event: "x".`). With `cfg` `None` the config
/// is read with [`crate::config::read_config`] from `ctx.root`'s store (keeping its
/// refusals, GDL1); a read error means no hooks. Timeouts come from
/// `hook_settings.timeout_ms` (default 5000 when <= 0, truncated to whole milliseconds).
/// Diagnostics and each hook's stderr are appended to `stderr`. `pre-*` failures block and
/// stop; `post-*` failures warn and continue; mutable events replace the payload with the
/// hook's JSON stdout (its `payload` member if it has one).
pub fn run_hooks(
    ctx: &Context,
    cfg: Option<&Config>,
    event: &str,
    payload: &Value,
    stderr: &mut String,
) -> Result<HookResult> {
    let mut result = HookResult {
        payload: payload.clone(),
        blocked: false,
        block_reason: String::new(),
        warnings: Vec::new(),
        ran_any: false,
        executions: Vec::new(),
    };
    if !HOOK_EVENTS.contains(&event) {
        return Err(AtlasError::Message(format!(
            "Unknown hook event: {}.",
            crate::commands::go_quote(event)
        )));
    }
    let read;
    let cfg = match cfg {
        Some(cfg) => cfg,
        None => {
            // Go's ReadConfig; any error means no hooks. The indented-first-key refusal
            // (GDL1) is the reader's, so such a config runs no hook.
            match config::read_config(&ctx.root.join(".luuna").join("atlas")) {
                Ok(c) => {
                    read = c;
                    &read
                }
                Err(_) => return Ok(result),
            }
        }
    };
    let scripts = list(obj(cfg.get("hooks")).get(event));
    let mut timeout_ms = number(obj(cfg.get("hook_settings")).get("timeout_ms"));
    // Go: `if timeout <= 0` (NaN is not <= 0 and stays).
    if timeout_ms <= 0.0 {
        timeout_ms = DEFAULT_TIMEOUT_MS;
    }
    let timeout = go_millis_duration(timeout_ms);
    let mutable = event == "pre-record" || event == "pre-prime" || event == "pre-compact";
    let blocking = event.starts_with("pre-");
    for script in &scripts {
        if script.trim().is_empty() {
            continue;
        }
        let mut envelope = Object::new();
        envelope.insert("event", event);
        envelope.insert("payload", result.payload.clone());
        // Go ignores a marshalling error and then gives the hook an empty stdin.
        let stdin = json::to_compact_string(&Value::Object(envelope))
            .map(String::into_bytes)
            .unwrap_or_default();
        let (execution, raw_stdout) = run_one_hook(&ctx.cwd, &ctx.env, script, stdin, timeout);
        result.ran_any = true;
        emit_diagnostic(
            &ctx.env,
            stderr,
            event,
            &execution,
            "hook executed",
            blocking,
            true,
        );
        if !execution.stderr.is_empty() {
            stderr.push_str(&execution.stderr);
            if !execution.stderr.ends_with('\n') {
                stderr.push('\n');
            }
        }
        if execution.exit_code != 0 {
            let reason = if execution.timed_out {
                format!(
                    "hook `{script}` timed out after {}ms",
                    go_format_g(timeout_ms)
                )
            } else {
                format!("hook `{script}` exited with code {}", execution.exit_code)
            };
            emit_diagnostic(
                &ctx.env, stderr, event, &execution, &reason, blocking, false,
            );
            result.executions.push(execution);
            if blocking {
                result.blocked = true;
                result.block_reason = reason;
                return Ok(result);
            }
            result.warnings.push(reason);
            continue;
        }
        result.executions.push(execution);
        let trimmed = go_trim_space(&raw_stdout);
        if mutable && !trimmed.is_empty() {
            match json::parse_bytes(trimmed) {
                Err(_) => {
                    result.warnings.push(format!(
                        "hook `{script}` printed non-JSON on stdout (mutation ignored): {}",
                        json_parse_error(trimmed)
                    ));
                }
                Ok(parsed) => {
                    // Go: `Obj(parsed)["payload"]` when present (even null), else the
                    // whole value.
                    let inner = match &parsed {
                        Value::Object(m) => m.get("payload").cloned(),
                        _ => None,
                    };
                    result.payload = inner.unwrap_or(parsed);
                }
            }
        }
    }
    Ok(result)
}

/// `runOneHook`: runs `[<sh>, "-c", command]` in `cwd` with `env`'s variables plus
/// `LUUNA_ATLAS_HOOK=1`, `stdin` on its standard input, and `timeout`. Returns the
/// execution and the raw bytes of its stdout (the execution's `stdout` is the same with
/// invalid UTF-8 replaced by U+FFFD).
///
/// A start failure (no `sh` on `env`'s `PATH` included) is exit code 1 with no output, as
/// `hooks.go:45-48`. On unix the child leads a new session; on timeout the whole process
/// group is killed and the exit code is 124. After the hook process ends, output still
/// arriving through pipes a background child holds is read for at most [`WAIT_DELAY`].
fn run_one_hook(
    cwd: &Path,
    env: &Env,
    command: &str,
    stdin: Vec<u8>,
    timeout: Duration,
) -> (HookExecution, Vec<u8>) {
    let start = Instant::now();
    let mut execution = HookExecution {
        command: command.to_owned(),
        ..HookExecution::default()
    };
    let started = env
        .look_path("sh")
        .and_then(|sh| spawn_hook(&sh, cwd, env, command).ok());
    let Some(mut child) = started else {
        execution.exit_code = 1;
        execution.duration_ms = elapsed_ms(start);
        return (execution, Vec::new());
    };

    // stdin is written from its own thread, which is not waited for: a hook that never
    // reads it cannot hold up the run (Go closes the pipe after WaitDelay).
    if let Some(mut pipe) = child.stdin.take() {
        thread::spawn(move || {
            let _ = pipe.write_all(&stdin);
        });
    }
    let stop = Arc::new(AtomicBool::new(false));
    let (done_tx, done_rx) = mpsc::channel::<()>();
    let out_buf = Arc::new(Mutex::new(Vec::new()));
    let err_buf = Arc::new(Mutex::new(Vec::new()));
    let mut readers = 0;
    if let Some(pipe) = child.stdout.take() {
        spawn_reader(
            pipe,
            Arc::clone(&out_buf),
            Arc::clone(&stop),
            done_tx.clone(),
        );
        readers += 1;
    }
    if let Some(pipe) = child.stderr.take() {
        spawn_reader(
            pipe,
            Arc::clone(&err_buf),
            Arc::clone(&stop),
            done_tx.clone(),
        );
        readers += 1;
    }
    drop(done_tx);

    let deadline = Instant::now().checked_add(timeout);
    let status = wait_with_deadline(&mut child, deadline);
    let status = match status {
        Waited::Exited(status) => Some(status),
        Waited::Failed => None,
        Waited::TimedOut => {
            execution.timed_out = true;
            kill_hook_process(&mut child);
            let _ = child.wait();
            None
        }
    };

    // WaitDelay: the readers get until then to see the end of both pipes.
    let delay_end = Instant::now() + WAIT_DELAY;
    while readers > 0 {
        let left = delay_end.saturating_duration_since(Instant::now());
        match done_rx.recv_timeout(left) {
            Ok(()) => readers -= 1,
            Err(_) => break,
        }
    }
    stop.store(true, Ordering::SeqCst);
    let raw_stdout = take_buffer(&out_buf);
    let raw_stderr = take_buffer(&err_buf);

    execution.exit_code = if execution.timed_out {
        124
    } else {
        match status {
            Some(status) if status.success() => 0,
            // Go's ProcessState.ExitCode(): -1 when a signal ended the process.
            Some(status) => status.code().unwrap_or(-1),
            None => 1,
        }
    };
    execution.stdout = String::from_utf8_lossy(&raw_stdout).into_owned();
    execution.stderr = String::from_utf8_lossy(&raw_stderr).into_owned();
    execution.duration_ms = elapsed_ms(start);
    (execution, raw_stdout)
}

/// Starts the hook child with its pipes; `argv[0]` is `sh` as in Go's
/// `exec.Command("sh", ...)`.
fn spawn_hook(sh: &Path, cwd: &Path, env: &Env, command: &str) -> std::io::Result<Child> {
    let mut cmd = Command::new(sh);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.arg0("sh");
    }
    cmd.arg("-c")
        .arg(command)
        .env_clear()
        .envs(env.vars().iter().map(|(k, v)| (k, v)))
        // Go appends the marker to the environment and its exec keeps the last value of
        // a repeated name; Command's environment map does the same.
        .env("LUUNA_ATLAS_HOOK", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if !cwd.as_os_str().is_empty() {
        cmd.current_dir(cwd);
    }
    configure_hook_process(&mut cmd);
    cmd.spawn()
}

/// Copies `pipe` into `buf` until its end, a read error or `stop`; then signals `done`.
/// Data read after `stop` is set is dropped, and the pipe is closed when the thread ends.
fn spawn_reader<R: Read + Send + 'static>(
    mut pipe: R,
    buf: Arc<Mutex<Vec<u8>>>,
    stop: Arc<AtomicBool>,
    done: mpsc::Sender<()>,
) {
    thread::spawn(move || {
        let mut chunk = [0u8; 8192];
        loop {
            match pipe.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => {
                    if stop.load(Ordering::SeqCst) {
                        break;
                    }
                    match buf.lock() {
                        Ok(mut b) => b.extend_from_slice(&chunk[..n]),
                        Err(_) => break,
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(_) => break,
            }
        }
        let _ = done.send(());
    });
}

/// The bytes read so far, leaving the buffer empty.
fn take_buffer(buf: &Mutex<Vec<u8>>) -> Vec<u8> {
    match buf.lock() {
        Ok(mut b) => std::mem::take(&mut *b),
        Err(poisoned) => std::mem::take(&mut *poisoned.into_inner()),
    }
}

/// How waiting for the hook process ended.
enum Waited {
    Exited(ExitStatus),
    /// Waiting itself failed (Go: a `Wait` error without a process state, exit code 1).
    Failed,
    TimedOut,
}

/// Waits for `child` until `deadline` (`None`: no deadline), checking with growing pauses
/// of at most [`MAX_POLL`]. The child is reaped only here, so on [`Waited::TimedOut`] its
/// pid (and so its process group id) cannot have been reused yet.
fn wait_with_deadline(child: &mut Child, deadline: Option<Instant>) -> Waited {
    let mut pause = Duration::from_millis(1);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Waited::Exited(status),
            Ok(None) => {}
            Err(_) => return Waited::Failed,
        }
        let now = Instant::now();
        let left = match deadline {
            Some(deadline) if now >= deadline => return Waited::TimedOut,
            Some(deadline) => deadline - now,
            None => MAX_POLL,
        };
        thread::sleep(pause.min(left));
        pause = (pause * 2).min(MAX_POLL);
    }
}

/// `configureHookProcess` (unix): the child calls `setsid` before `exec`, so it leads a
/// new session and process group whose id is its pid.
#[cfg(unix)]
fn configure_hook_process(cmd: &mut Command) {
    use std::os::unix::process::CommandExt;
    // SAFETY: the closure runs in the forked child between fork and exec. It calls only
    // setsid(2), which is async-signal-safe, and builds an io::Error from errno without
    // allocating; it takes no lock and touches no memory shared with the parent.
    unsafe {
        cmd.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

#[cfg(not(unix))]
fn configure_hook_process(_cmd: &mut Command) {}

/// `killHookProcess` (unix): SIGKILL to the hook's whole process group, background
/// children included.
#[cfg(unix)]
fn kill_hook_process(child: &mut Child) {
    let Ok(pid) = libc::pid_t::try_from(child.id()) else {
        return;
    };
    if pid <= 0 {
        return;
    }
    // SAFETY: kill(2) takes plain integers and has no memory-safety preconditions. The
    // child has not been reaped yet (only wait_with_deadline and the wait after this call
    // reap it), so its pid, which is the group id setsid gave it, still belongs to this
    // hook's process group and cannot name another group.
    unsafe {
        libc::kill(-pid, libc::SIGKILL);
    }
}

/// Without process groups only the hook child itself is killed (D5).
#[cfg(not(unix))]
fn kill_hook_process(child: &mut Child) {
    let _ = child.kill();
}

/// Whole milliseconds since `start` (Go's `time.Since(start).Milliseconds()`).
fn elapsed_ms(start: Instant) -> i64 {
    i64::try_from(start.elapsed().as_millis()).unwrap_or(i64::MAX)
}

/// `time.Duration(timeout) * time.Millisecond` as Go computes it on amd64: the float is
/// truncated toward zero to an int64 (NaN or out of range gives the minimum int64), the
/// product wraps, and a timer with a duration of zero or less fires at once.
fn go_millis_duration(timeout_ms: f64) -> Duration {
    // 2^63 as f64; the conversion is exact for every in-range truncated value.
    const LIMIT: f64 = 9_223_372_036_854_775_808.0;
    // NaN is in no range, so it takes the first branch too.
    let ms = if !(-LIMIT..LIMIT).contains(&timeout_ms) {
        i64::MIN
    } else {
        timeout_ms as i64
    };
    let nanos = ms.wrapping_mul(1_000_000);
    match u64::try_from(nanos) {
        Ok(n) if n > 0 => Duration::from_nanos(n),
        _ => Duration::ZERO,
    }
}

/// Go's `fmt.Sprintf("%g", f)`: the shortest digits that read back as `f`, in exponent
/// form (`1e+06`, at least two exponent digits) when the decimal exponent is below -4 or
/// at least 6, else plain (`5000`, `0.5`); `NaN`, `+Inf`, `-Inf`.
fn go_format_g(f: f64) -> String {
    if f.is_nan() {
        return "NaN".to_owned();
    }
    if f.is_infinite() {
        return if f > 0.0 { "+Inf" } else { "-Inf" }.to_owned();
    }
    if f == 0.0 {
        return if f.is_sign_negative() { "-0" } else { "0" }.to_owned();
    }
    // Rust's `{:e}` gives the shortest round-trip digits as `d[.ddd]e<exp>`.
    let sci = format!("{f:e}");
    let (mantissa, exp) = sci.split_once('e').unwrap_or((sci.as_str(), "0"));
    let exp: i32 = exp.parse().unwrap_or(0);
    if !(-4..6).contains(&exp) {
        let sign = if exp < 0 { '-' } else { '+' };
        format!("{mantissa}e{sign}{:02}", exp.unsigned_abs())
    } else {
        // Rust's `Display` gives the same shortest digits without an exponent.
        f.to_string()
    }
}

/// Go's `strings.TrimSpace` on bytes: leading and trailing Unicode white space removed;
/// invalid UTF-8 is not white space.
fn go_trim_space(bytes: &[u8]) -> &[u8] {
    let mut start = 0;
    while start < bytes.len() {
        match leading_char(&bytes[start..]) {
            Some(c) if c.is_whitespace() => start += c.len_utf8(),
            _ => break,
        }
    }
    let mut end = bytes.len();
    while end > start {
        match trailing_char(&bytes[start..end]) {
            Some(c) if c.is_whitespace() => end -= c.len_utf8(),
            _ => break,
        }
    }
    &bytes[start..end]
}

/// The valid UTF-8 character at the start of `bytes`, if there is one.
fn leading_char(bytes: &[u8]) -> Option<char> {
    let head = &bytes[..bytes.len().min(4)];
    let valid = match std::str::from_utf8(head) {
        Ok(s) => s,
        // The prefix up to the first invalid byte is valid UTF-8.
        Err(e) => std::str::from_utf8(&head[..e.valid_up_to()]).unwrap_or(""),
    };
    valid.chars().next()
}

/// The valid UTF-8 character at the end of `bytes`, if there is one.
fn trailing_char(bytes: &[u8]) -> Option<char> {
    for len in 1..=bytes.len().min(4) {
        if let Ok(s) = std::str::from_utf8(&bytes[bytes.len() - len..]) {
            return s.chars().next_back().filter(|c| c.len_utf8() == len);
        }
    }
    None
}

/// Appends the diagnostic line for one hook run to `stderr`, unless the level hides it.
fn emit_diagnostic(
    env: &Env,
    stderr: &mut String,
    event: &str,
    execution: &HookExecution,
    reason: &str,
    blocking: bool,
    debug: bool,
) {
    let time_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0);
    // The level is checked first so that a hidden line costs no hostname lookup.
    if !level_shows(env, debug) {
        return;
    }
    if let Some(line) = diagnostic_line(
        env,
        event,
        execution,
        reason,
        blocking,
        debug,
        time_ms,
        std::process::id(),
        &hostname(),
    ) {
        stderr.push_str(&line);
        stderr.push('\n');
    }
}

/// Whether a message of this kind passes the level in `env` (`hooks.go:78-91`):
/// `LUUNA_ATLAS_LOG_LEVEL` when set (even empty), else `debug` when `LUUNA_ATLAS_DEBUG` is
/// non-empty, else `info`; unknown names rank 0.
fn level_shows(env: &Env, debug: bool) -> bool {
    let level = match env.var("LUUNA_ATLAS_LOG_LEVEL") {
        Some(level) => level,
        None if env.var("LUUNA_ATLAS_DEBUG").is_some_and(|v| !v.is_empty()) => "debug".to_owned(),
        None => "info".to_owned(),
    };
    let rank = match level.as_str() {
        "trace" => 10,
        "debug" => 20,
        "info" => 30,
        "warn" => 40,
        "error" => 50,
        "fatal" => 60,
        "silent" => 100,
        _ => 0,
    };
    rank <= message_level(debug)
}

/// The level of a diagnostic: 20 (debug) for "hook executed", 40 (warn) for a failure.
fn message_level(debug: bool) -> i64 {
    if debug {
        20
    } else {
        40
    }
}

/// Go's `os.Hostname()`: the kernel's host name, `""` on failure.
#[cfg(unix)]
fn hostname() -> String {
    let mut buf = [0u8; 256];
    // SAFETY: `buf` is a live, writable buffer of `buf.len()` bytes, and gethostname(2)
    // writes at most that many bytes into it.
    let rc = unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) };
    if rc != 0 {
        return String::new();
    }
    let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    String::from_utf8_lossy(&buf[..end]).into_owned()
}

/// No host name lookup is ported for other systems (D5).
#[cfg(not(unix))]
fn hostname() -> String {
    String::new()
}

/// One diagnostic line as `hookDiagnostic` writes it (without the newline), or `None`
/// when the level in `env` filters it out. The time (Unix milliseconds), pid and hostname
/// are parameters so that tests can fix them.
#[allow(clippy::too_many_arguments)]
fn diagnostic_line(
    env: &Env,
    event: &str,
    execution: &HookExecution,
    reason: &str,
    blocking: bool,
    debug: bool,
    time_ms: i64,
    pid: u32,
    hostname: &str,
) -> Option<String> {
    if !level_shows(env, debug) {
        return None;
    }
    // Go's key order (hooks.go:93-99): level, time, pid, hostname, name, event, command,
    // exitCode, durationMs or nothing, timedOut, blocking or nothing, msg.
    let mut m = Object::new();
    m.insert("level", message_level(debug));
    m.insert("time", time_ms);
    m.insert("pid", i64::from(pid));
    m.insert("hostname", hostname);
    m.insert("name", "luuna-atlas");
    m.insert("event", event);
    m.insert("command", execution.command.as_str());
    m.insert("exitCode", i64::from(execution.exit_code));
    if debug {
        m.insert("durationMs", execution.duration_ms);
    }
    m.insert("timedOut", execution.timed_out);
    if !debug {
        m.insert("blocking", blocking);
    }
    m.insert("msg", reason);
    // Go ignores a marshalling error and prints the empty result.
    Some(json::to_compact_string(&Value::Object(m)).unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::json;
    use crate::options::test_support::{process_path, relative_from_cwd, write_script};
    use crate::options::GlobalOptions;
    use std::ffi::OsString;
    use std::path::Path;
    use std::time::{Duration, Instant};

    fn context(cwd: &Path, vars: Vec<(OsString, OsString)>) -> Context {
        Context {
            root: cwd.to_owned(),
            cwd: cwd.to_owned(),
            env: Env::fixed(vars, Vec::new(), false),
            globals: GlobalOptions::default(),
        }
    }

    fn path_only() -> Vec<(OsString, OsString)> {
        vec![("PATH".into(), process_path())]
    }

    fn config(text: &str) -> Config {
        match &mut json::parse(text).expect("config json") {
            Value::Object(object) => std::mem::take(object),
            _ => panic!("config must be an object"),
        }
    }

    fn payload() -> Value {
        json::parse(r#"{"type":"convention","content":"x"}"#).expect("payload")
    }

    fn compact(value: &Value) -> String {
        json::to_compact_string(value).expect("encode")
    }

    #[test]
    fn mutation_block_and_observe() {
        let dir = tempfile::tempdir().expect("tempdir");
        let ctx = context(dir.path(), path_only());
        let cfg = config(
            r#"{"hooks":{
                "pre-record":["cat > seen.json; printf '{\"payload\":{\"content\":\"changed\"}}'", "  ", "printf '  {\"bare\":1}  '"],
                "pre-prime":["exit 3", "printf 'never' > second-ran"],
                "post-record":["echo oops >&2; exit 2", "printf ok"],
                "pre-compact":["printf 'not json'"]
            }}"#,
        );
        let mut stderr = String::new();

        // Mutable event: the first hook's `payload` member, then the second hook's whole
        // stdout (it has no `payload` member). The blank script is skipped.
        let result =
            run_hooks(&ctx, Some(&cfg), "pre-record", &payload(), &mut stderr).expect("run");
        assert!(result.ran_any);
        assert!(!result.blocked);
        assert_eq!(result.executions.len(), 2);
        assert_eq!(compact(&result.payload), r#"{"bare":1}"#);
        // The hook got {event, payload} as compact JSON on stdin.
        let seen = std::fs::read_to_string(dir.path().join("seen.json")).expect("seen");
        assert_eq!(
            seen,
            r#"{"event":"pre-record","payload":{"type":"convention","content":"x"}}"#
        );

        // A pre-* failure blocks and stops.
        let result =
            run_hooks(&ctx, Some(&cfg), "pre-prime", &payload(), &mut stderr).expect("run");
        assert!(result.blocked);
        assert_eq!(result.block_reason, "hook `exit 3` exited with code 3");
        assert_eq!(result.executions.len(), 1);
        assert!(!dir.path().join("second-ran").exists());

        // A post-* failure warns and continues; its stderr is copied with a newline.
        let mut stderr = String::new();
        let result =
            run_hooks(&ctx, Some(&cfg), "post-record", &payload(), &mut stderr).expect("run");
        assert!(!result.blocked);
        assert_eq!(
            result.warnings,
            ["hook `echo oops >&2; exit 2` exited with code 2"]
        );
        assert_eq!(result.executions.len(), 2);
        assert_eq!(result.executions[1].stdout, "ok");
        assert!(stderr.contains("oops\n"), "{stderr}");
        // post-record is not mutable: the payload is unchanged.
        assert_eq!(compact(&result.payload), compact(&payload()));

        // Non-JSON stdout of a mutable event: a warning, the payload kept.
        let result =
            run_hooks(&ctx, Some(&cfg), "pre-compact", &payload(), &mut stderr).expect("run");
        assert_eq!(result.warnings.len(), 1);
        assert!(
            result.warnings[0].starts_with(
                "hook `printf 'not json'` printed non-JSON on stdout (mutation ignored): "
            ),
            "{}",
            result.warnings[0]
        );
        assert_eq!(compact(&result.payload), compact(&payload()));

        // An event with no hooks runs nothing.
        let result =
            run_hooks(&ctx, Some(&cfg), "pre-prune", &payload(), &mut stderr).expect("run");
        assert!(!result.ran_any);
        assert!(result.executions.is_empty());
    }

    /// The live (not zombie) processes whose working directory is `dir`: every process a
    /// hook started there, background children included.
    fn live_processes_in(dir: &Path) -> Vec<String> {
        let mut out = Vec::new();
        let Ok(entries) = std::fs::read_dir("/proc") else {
            return out;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if std::fs::read_link(path.join("cwd")).ok().as_deref() != Some(dir) {
                continue;
            }
            let Ok(stat) = std::fs::read_to_string(path.join("stat")) else {
                continue;
            };
            // The state follows the parenthesised command name.
            let zombie = stat
                .rfind(')')
                .is_some_and(|i| stat[i + 1..].trim_start().starts_with('Z'));
            if !zombie {
                out.push(entry.file_name().to_string_lossy().into_owned());
            }
        }
        out
    }

    #[test]
    fn timeout_kills_the_group_promptly() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cwd = dir.path().canonicalize().expect("canonical");
        let ctx = context(&cwd, path_only());
        let cfg = config(
            r#"{"hooks":{"pre-record":["sleep 5 & sleep 5"]},"hook_settings":{"timeout_ms":50}}"#,
        );
        let mut stderr = String::new();
        let start = Instant::now();
        let result =
            run_hooks(&ctx, Some(&cfg), "pre-record", &payload(), &mut stderr).expect("run");
        assert!(
            start.elapsed() < Duration::from_secs(3),
            "{:?}",
            start.elapsed()
        );
        assert!(result.blocked);
        assert!(result.executions[0].timed_out);
        assert_eq!(result.executions[0].exit_code, 124);
        assert_eq!(
            result.block_reason,
            "hook `sleep 5 & sleep 5` timed out after 50ms"
        );
        // Neither sh nor either sleep (the background one included) survives.
        let deadline = Instant::now() + Duration::from_secs(2);
        while !live_processes_in(&cwd).is_empty() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(live_processes_in(&cwd), Vec::<String>::new());
    }

    fn execution(exit_code: i32, timed_out: bool) -> HookExecution {
        HookExecution {
            command: "exit 3".to_owned(),
            exit_code,
            stderr: String::new(),
            stdout: String::new(),
            duration_ms: 7,
            timed_out,
        }
    }

    #[test]
    fn diagnostic_line_format() {
        let debug_env = Env::fixed([("LUUNA_ATLAS_DEBUG", "1")], Vec::new(), false);
        let line = diagnostic_line(
            &debug_env,
            "pre-record",
            &execution(3, false),
            "hook executed",
            true,
            true,
            1_700_000_000_123,
            42,
            "host-a",
        );
        assert_eq!(
            line.as_deref(),
            Some(
                r#"{"level":20,"time":1700000000123,"pid":42,"hostname":"host-a","name":"luuna-atlas","event":"pre-record","command":"exit 3","exitCode":3,"durationMs":7,"timedOut":false,"msg":"hook executed"}"#
            )
        );
        // The warning line has `blocking` instead of `durationMs`; info is the default
        // level, which hides debug lines.
        let info = Env::fixed(Vec::<(String, String)>::new(), Vec::new(), false);
        let line = diagnostic_line(
            &info,
            "post-record",
            &execution(124, true),
            "hook `exit 3` timed out after 5000ms",
            false,
            false,
            5,
            1,
            "h",
        );
        assert_eq!(
            line.as_deref(),
            Some(
                r#"{"level":40,"time":5,"pid":1,"hostname":"h","name":"luuna-atlas","event":"post-record","command":"exit 3","exitCode":124,"timedOut":true,"blocking":false,"msg":"hook `exit 3` timed out after 5000ms"}"#
            )
        );
        let hidden = diagnostic_line(
            &info,
            "pre-record",
            &execution(0, false),
            "hook executed",
            true,
            true,
            5,
            1,
            "h",
        );
        assert_eq!(hidden, None);
        // LUUNA_ATLAS_LOG_LEVEL wins over LUUNA_ATLAS_DEBUG even when empty; empty and
        // unknown names rank 0, so everything prints.
        for level in ["", "verbose"] {
            let env = Env::fixed([("LUUNA_ATLAS_LOG_LEVEL", level)], Vec::new(), false);
            let line = diagnostic_line(
                &env,
                "pre-record",
                &execution(0, false),
                "hook executed",
                true,
                true,
                5,
                1,
                "h",
            );
            assert!(line.is_some(), "level {level:?}");
        }
        let warn = Env::fixed(
            [
                ("LUUNA_ATLAS_LOG_LEVEL", "warn"),
                ("LUUNA_ATLAS_DEBUG", "1"),
            ],
            Vec::new(),
            false,
        );
        let hidden = diagnostic_line(
            &warn,
            "pre-record",
            &execution(0, false),
            "hook executed",
            true,
            true,
            5,
            1,
            "h",
        );
        assert_eq!(hidden, None);
    }

    #[test]
    fn silent_level_prints_nothing() {
        let silent = Env::fixed([("LUUNA_ATLAS_LOG_LEVEL", "silent")], Vec::new(), false);
        for debug in [true, false] {
            let line = diagnostic_line(
                &silent,
                "pre-record",
                &execution(3, false),
                "hook `exit 3` exited with code 3",
                true,
                debug,
                5,
                1,
                "h",
            );
            assert_eq!(line, None);
        }
        let dir = tempfile::tempdir().expect("tempdir");
        let mut vars = path_only();
        vars.push(("LUUNA_ATLAS_LOG_LEVEL".into(), "silent".into()));
        let ctx = context(dir.path(), vars);
        let cfg = config(r#"{"hooks":{"post-record":["exit 1"]}}"#);
        let mut stderr = String::new();
        let result =
            run_hooks(&ctx, Some(&cfg), "post-record", &payload(), &mut stderr).expect("run");
        assert_eq!(result.warnings.len(), 1);
        assert_eq!(stderr, "");
    }

    #[test]
    fn unknown_event_is_refused() {
        let dir = tempfile::tempdir().expect("tempdir");
        let ctx = context(dir.path(), path_only());
        let cfg = config(r#"{"hooks":{"x":["printf ran > ran"]}}"#);
        let mut stderr = String::new();
        let err =
            run_hooks(&ctx, Some(&cfg), "x", &payload(), &mut stderr).expect_err("unknown event");
        assert_eq!(err.to_string(), "Unknown hook event: \"x\".");
        let err = run_hooks(&ctx, Some(&cfg), "post-prime", &payload(), &mut stderr)
            .expect_err("unknown event");
        assert_eq!(err.to_string(), "Unknown hook event: \"post-prime\".");
        assert!(!dir.path().join("ran").exists());
        assert_eq!(stderr, "");
    }

    #[test]
    fn env_is_env_plus_hook_marker() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cwd = dir.path().canonicalize().expect("canonical");
        // Shells export PWD; supply the hook's cwd so it stays unchanged.
        let vars: Vec<(OsString, OsString)> = vec![
            ("PATH".into(), process_path()),
            ("PWD".into(), cwd.as_os_str().to_owned()),
            ("ATLAS_TEST_ONE".into(), "1".into()),
            ("ATLAS_TEST_SPACE".into(), "a b".into()),
        ];
        let ctx = context(&cwd, vars.clone());
        let cfg = config(r#"{"hooks":{"post-record":["env > env.txt"]}}"#);
        let mut stderr = String::new();
        let result =
            run_hooks(&ctx, Some(&cfg), "post-record", &payload(), &mut stderr).expect("run");
        assert!(result.warnings.is_empty(), "{:?}", result.warnings);
        let printed = std::fs::read_to_string(cwd.join("env.txt")).expect("env.txt");
        // macOS sh also exports SHLVL and _ (dash does not). These are shell
        // bookkeeping, not variables inherited from the parent process.
        let mut got: Vec<String> = printed
            .lines()
            .filter(|line| !line.starts_with("SHLVL=") && !line.starts_with("_="))
            .map(str::to_owned)
            .collect();
        got.sort();
        let mut want: Vec<String> = vars
            .iter()
            .map(|(k, v)| format!("{}={}", k.to_string_lossy(), v.to_string_lossy()))
            .collect();
        want.push("LUUNA_ATLAS_HOOK=1".to_owned());
        want.sort();
        assert_eq!(got, want);
    }

    #[test]
    fn sh_is_found_on_env_path() {
        let bin = tempfile::tempdir().expect("tempdir");
        let work = tempfile::tempdir().expect("tempdir");
        let marker = work.path().join("stub-sh-ran");
        write_script(
            bin.path(),
            "sh",
            &format!(": > '{}'; exec /bin/sh \"$@\"", marker.display()),
        );
        let ctx = context(
            work.path(),
            vec![("PATH".into(), bin.path().as_os_str().to_owned())],
        );
        let cfg = config(r#"{"hooks":{"pre-record":["printf '{\"n\":1}'"]}}"#);
        let mut stderr = String::new();
        let result =
            run_hooks(&ctx, Some(&cfg), "pre-record", &payload(), &mut stderr).expect("run");
        assert!(marker.exists(), "the stub sh on Env's PATH did not run");
        assert!(!result.blocked);
        assert_eq!(compact(&result.payload), r#"{"n":1}"#);
    }

    #[test]
    fn no_sh_on_env_path_is_exit_1() {
        let empty = tempfile::tempdir().expect("tempdir");
        let work = tempfile::tempdir().expect("tempdir");
        let ctx = context(
            work.path(),
            vec![("PATH".into(), empty.path().as_os_str().to_owned())],
        );
        let cfg = config(r#"{"hooks":{"pre-record":["true"]}}"#);
        let mut stderr = String::new();
        let result =
            run_hooks(&ctx, Some(&cfg), "pre-record", &payload(), &mut stderr).expect("run");
        assert!(result.ran_any);
        assert!(result.blocked);
        assert_eq!(result.block_reason, "hook `true` exited with code 1");
        let execution = &result.executions[0];
        assert_eq!(execution.exit_code, 1);
        assert!(!execution.timed_out);
        assert_eq!(execution.stdout, "");
        assert_eq!(execution.stderr, "");
    }

    #[test]
    fn sh_found_only_through_dot_is_not_run() {
        let work = tempfile::tempdir().expect("tempdir");
        let marker = work.path().join("dot-sh-ran");
        write_script(
            work.path(),
            "sh",
            &format!(": > '{}'; exec /bin/sh \"$@\"", marker.display()),
        );
        // A relative PATH element naming the hook's own cwd: Go's ErrDot refuses it.
        let relative = relative_from_cwd(work.path());
        let ctx = context(
            work.path(),
            vec![("PATH".into(), relative.into_os_string())],
        );
        let cfg = config(r#"{"hooks":{"pre-record":["true"]}}"#);
        let mut stderr = String::new();
        let result =
            run_hooks(&ctx, Some(&cfg), "pre-record", &payload(), &mut stderr).expect("run");
        assert!(!marker.exists(), "a sh found through a relative PATH ran");
        assert!(result.blocked);
        assert_eq!(result.executions[0].exit_code, 1);
    }

    #[test]
    fn timeout_text_and_duration_as_go() {
        // fmt's %g with the shortest digits (hooks.go:136).
        for (value, want) in [
            (5000.0, "5000"),
            (50.0, "50"),
            (50.5, "50.5"),
            (100000.0, "100000"),
            (1e6, "1e+06"),
            (1234567.0, "1.234567e+06"),
            (1e21, "1e+21"),
            (0.0001, "0.0001"),
            (0.00001, "1e-05"),
            (f64::NAN, "NaN"),
            (f64::INFINITY, "+Inf"),
        ] {
            assert_eq!(go_format_g(value), want, "{value}");
        }
        // time.Duration(timeout) * time.Millisecond on amd64.
        assert_eq!(go_millis_duration(50.9), Duration::from_millis(50));
        assert_eq!(go_millis_duration(0.5), Duration::ZERO);
        assert_eq!(go_millis_duration(f64::INFINITY), Duration::ZERO);
        assert_eq!(go_millis_duration(f64::NAN), Duration::ZERO);
        assert_eq!(go_millis_duration(1e300), Duration::ZERO);
    }

    #[test]
    fn trim_space_keeps_invalid_utf8() {
        assert_eq!(go_trim_space(b" \t{\"a\":1}\n "), b"{\"a\":1}");
        assert_eq!(
            go_trim_space("\u{a0}\u{85}x\u{3000}".as_bytes()),
            b"x".as_slice()
        );
        assert_eq!(go_trim_space(b"\xff \n"), b"\xff".as_slice());
        assert_eq!(go_trim_space(b" \xe3\x80"), b"\xe3\x80".as_slice());
        assert_eq!(go_trim_space(b"  \n"), b"".as_slice());
    }

    #[test]
    fn background_child_holding_stdout_waits_only_the_delay() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cwd = dir.path().canonicalize().expect("canonical");
        let ctx = context(&cwd, path_only());
        // The background sleep keeps stdout open after sh exits; Go's WaitDelay (100 ms)
        // then gives up on the pipe. The hook succeeded, so exit code 0.
        let cfg = config(r#"{"hooks":{"post-record":["sleep 1 & printf done"]}}"#);
        let mut stderr = String::new();
        let start = Instant::now();
        let result =
            run_hooks(&ctx, Some(&cfg), "post-record", &payload(), &mut stderr).expect("run");
        assert!(
            start.elapsed() < Duration::from_millis(900),
            "{:?}",
            start.elapsed()
        );
        assert!(result.warnings.is_empty(), "{:?}", result.warnings);
        assert_eq!(result.executions[0].exit_code, 0);
        assert_eq!(result.executions[0].stdout, "done");
        // The background child outlives the hook, as in Go; wait for it to end so the
        // test leaves nothing running.
        let deadline = Instant::now() + Duration::from_secs(3);
        while !live_processes_in(&cwd).is_empty() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(live_processes_in(&cwd), Vec::<String>::new());
    }

    #[test]
    fn signal_exit_is_minus_one_and_null_payload_member_is_kept() {
        let dir = tempfile::tempdir().expect("tempdir");
        let ctx = context(dir.path(), path_only());
        let cfg = config(
            r#"{"hooks":{
                "post-record":["kill -9 $$"],
                "pre-compact":["printf '{\"payload\":null}'"]
            },"hook_settings":{"timeout_ms":"0"}}"#,
        );
        let mut stderr = String::new();
        let result =
            run_hooks(&ctx, Some(&cfg), "post-record", &payload(), &mut stderr).expect("run");
        assert_eq!(result.executions[0].exit_code, -1);
        assert_eq!(result.warnings, ["hook `kill -9 $$` exited with code -1"]);
        let result =
            run_hooks(&ctx, Some(&cfg), "pre-compact", &payload(), &mut stderr).expect("run");
        assert!(result.payload.is_null());
    }

    #[test]
    fn config_is_read_from_the_root_when_none() {
        let dir = tempfile::tempdir().expect("tempdir");
        let ctx = context(dir.path(), path_only());
        let mut stderr = String::new();
        // No store: a read error means no hooks.
        let result = run_hooks(&ctx, None, "pre-record", &payload(), &mut stderr).expect("run");
        assert!(!result.ran_any);
        let atlas = dir.path().join(".luuna").join("atlas");
        std::fs::create_dir_all(&atlas).expect("mkdir");
        std::fs::write(
            atlas.join("config.yaml"),
            "hooks:\n  pre-record:\n    - printf '{\"read\":1}'\n",
        )
        .expect("write config");
        let result = run_hooks(&ctx, None, "pre-record", &payload(), &mut stderr).expect("run");
        assert!(result.ran_any);
        assert_eq!(compact(&result.payload), r#"{"read":1}"#);
        // The indented first key (GDL1) is refused by the reader: no hooks run.
        std::fs::write(
            atlas.join("config.yaml"),
            "  b: 1\nhooks:\n  pre-record:\n    - printf ran > ran\n",
        )
        .expect("write config");
        let result = run_hooks(&ctx, None, "pre-record", &payload(), &mut stderr).expect("run");
        assert!(!result.ran_any);
        assert!(!dir.path().join("ran").exists());
    }

    #[test]
    #[ignore = "hostile input: run under prlimit (PIPELINE rule 10)"]
    fn hostile_large_hook_stdout() {
        let dir = tempfile::tempdir().expect("tempdir");
        let ctx = context(dir.path(), path_only());
        let cfg = config(r#"{"hooks":{"pre-record":["head -c 1048576 /dev/zero"]}}"#);
        let mut stderr = String::new();
        let result =
            run_hooks(&ctx, Some(&cfg), "pre-record", &payload(), &mut stderr).expect("run");
        assert_eq!(result.executions[0].stdout.len(), 1_048_576);
        assert!(!result.blocked);
        assert_eq!(result.warnings.len(), 1);
        assert_eq!(compact(&result.payload), compact(&payload()));
    }
}
