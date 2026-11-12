//! Running the configured hooks (`sh -c`, process group, diagnostics).
//! Owner: s5-atlas-lib-2.
//!
//! A port of the Go tool's `hooks.go` (`RunHooks`, `runOneHook`, `hookDiagnostic`) and
//! `hooks_process_unix.go` (setsid, group SIGKILL). Hooks run the config text through
//! `sh -c` in `ctx.cwd` with the caller's whole environment plus `LUUNA_ATLAS_HOOK=1`, as
//! in Go (P3); `sh` is found on `ctx.env`'s `PATH` (P25). On non-unix systems only the
//! hook child itself is killed on timeout (D5).

use crate::config::Config;
use crate::error::{AtlasError, Result};
use crate::json::Value;
use crate::options::{Context, Env};

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
    // stub until t6
    let _ = (ctx, cfg, event, payload, stderr);
    Err(AtlasError::Message(
        "not implemented: hooks::run_hooks".to_owned(),
    ))
}

/// One diagnostic line as `hookDiagnostic` writes it (without the newline), or `None`
/// when the level in `env` filters it out. The time (Unix milliseconds), pid and hostname
/// are parameters so that tests can fix them.
#[cfg_attr(not(test), allow(dead_code))] // called by run_hooks from t6 on
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
    // stub until t6
    let _ = (
        env, event, execution, reason, blocking, debug, time_ms, pid, hostname,
    );
    None
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
        // dash exports PWD; giving it the hook's own cwd keeps the output exactly Env's.
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
        let mut got: Vec<String> = printed.lines().map(str::to_owned).collect();
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
