//! Go's debug logging (`internal/cli/logging.go`): the log level the
//! invocation's environment gives, the debug record written before a
//! command error, and the host name in it.

use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use super::super::Context;
use crate::error::{Error, Result, PLAN_2};
use crate::json;

/// Go's `resolvedLogLevel`, for the environment `get` reads. The level
/// variable is matched exactly and untrimmed; an invalid value falls back to
/// the `LUUNA_MISSIONS_DEBUG=1` rule.
fn resolved_level<'a>(get: impl Fn(&str) -> Option<&'a str>) -> &'a str {
    if let Some(level) = get("LUUNA_MISSIONS_LOG_LEVEL") {
        if let Some(valid) = ["trace", "debug", "info", "warn", "error", "fatal", "silent"]
            .into_iter()
            .find(|valid| *valid == level)
        {
            return valid;
        }
    }
    if get("LUUNA_MISSIONS_DEBUG") == Some("1") {
        return "debug";
    }
    "info"
}

/// Whether the level logs command errors (`logDebugError`).
fn logs_errors(level: &str) -> bool {
    level == "debug" || level == "trace"
}

/// Whether the targets whose host name is read as Go reads it include this
/// one (DC3).
pub(in crate::command) const HOSTNAME_AS_GO: bool = cfg!(any(
    target_os = "linux",
    target_os = "android",
    target_os = "macos",
    target_os = "ios",
    target_os = "freebsd",
    target_os = "dragonfly"
));

/// The logging environment on a target whose host name cannot be read as Go
/// reads it (CF8, stricter than Go): refused when the level would log. On
/// every other target nothing is refused.
pub(in crate::command) fn refuse_debug_logging(env: &BTreeMap<String, String>) -> Result<()> {
    if HOSTNAME_AS_GO {
        return Ok(());
    }
    let level = resolved_level(|key| env.get(key).map(String::as_str));
    if logs_errors(level) {
        return Err(Error::unsupported(
            "debug logging (LUUNA_MISSIONS_DEBUG=1 or LUUNA_MISSIONS_LOG_LEVEL=debug|trace) \
             on this target, because the host name in its record cannot be read as Go reads it",
            PLAN_2,
        ));
    }
    Ok(())
}

/// The debug record of `logDebugError`: one compact JSON line, or nothing
/// when the invocation's level does not log.
pub(in crate::command) fn debug_error_record(
    context: &Context<'_>,
    name: &str,
    message: &str,
) -> Vec<u8> {
    if !logs_errors(resolved_level(|key| context.env(key))) {
        return Vec::new();
    }
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis());
    let mut out = format!(
        "{{\"level\":20,\"time\":{millis},\"pid\":{},\"hostname\":",
        std::process::id()
    )
    .into_bytes();
    json::quote(&mut out, &hostname(), false);
    out.extend_from_slice(b",\"err\":{\"type\":\"Error\",\"message\":");
    json::quote(&mut out, message, false);
    out.extend_from_slice(b",\"stack\":");
    json::quote(&mut out, &format!("Error: {message}"), false);
    out.extend_from_slice(b"},\"cmd\":");
    json::quote(&mut out, name, false);
    out.extend_from_slice(b",\"msg\":\"command failed\"}\n");
    out
}

/// `bytes` as Go's encoder would write them: each byte that is not part of a
/// valid UTF-8 sequence becomes U+FFFD.
#[cfg(any(
    target_os = "linux",
    target_os = "android",
    target_os = "macos",
    target_os = "ios",
    target_os = "freebsd",
    target_os = "dragonfly"
))]
fn text(bytes: &[u8]) -> String {
    let mut out = String::new();
    let mut rest = bytes;
    while !rest.is_empty() {
        match std::str::from_utf8(rest) {
            Ok(valid) => {
                out.push_str(valid);
                break;
            }
            Err(error) => {
                let (valid, invalid) = rest.split_at(error.valid_up_to());
                out.push_str(&String::from_utf8_lossy(valid));
                out.push('\u{fffd}');
                rest = &invalid[1..];
            }
        }
    }
    out
}

/// Go's `os.Hostname` on Linux and Android (`src/os/sys_linux.go`): the
/// `uname` node name when it is 1 to 63 bytes, else (Android) that name or
/// `localhost`, else (Linux) the first read of `/proc/sys/kernel/hostname`
/// without one final newline. An error gives "" (`logging.go:31` ignores
/// it).
#[cfg(any(target_os = "linux", target_os = "android"))]
pub(in crate::command) fn hostname() -> String {
    use std::io::Read;

    // SAFETY: `utsname` is plain old data (arrays of integers), for which
    // all-zero bytes are a valid value.
    let mut info: libc::utsname = unsafe { std::mem::zeroed() };
    // SAFETY: `info` is a valid, writable `utsname` that lives for the call.
    let status = unsafe { libc::uname(&mut info) };
    let nodename: Vec<u8> = info
        .nodename
        .iter()
        .map(|&byte| byte as u8)
        .take_while(|&byte| byte != 0)
        .collect();
    // Go finds a name only when a NUL ends it within the 65 bytes.
    let terminated = info.nodename.contains(&0);
    let name = if terminated { nodename } else { Vec::new() };
    if status == 0 && !name.is_empty() && name.len() < 64 {
        return text(&name);
    }
    if cfg!(target_os = "android") {
        return if name.is_empty() {
            "localhost".to_owned()
        } else {
            text(&name)
        };
    }
    let Ok(mut file) = std::fs::File::open("/proc/sys/kernel/hostname") else {
        return String::new();
    };
    let mut buffer = [0u8; 512];
    let Ok(mut read) = file.read(&mut buffer) else {
        return String::new();
    };
    if read > 0 && buffer[read - 1] == b'\n' {
        read -= 1;
    }
    text(&buffer[..read])
}

/// Go's `os.Hostname` on the BSDs and macOS (`src/os/sys_bsd.go`): the
/// `kern.hostname` sysctl. An error gives "".
#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "freebsd",
    target_os = "dragonfly"
))]
pub(in crate::command) fn hostname() -> String {
    let key = c"kern.hostname";
    let mut size: libc::size_t = 0;
    // SAFETY: `key` is NUL-terminated, `size` is a valid out pointer, and a
    // null buffer asks only for the size.
    let status = unsafe {
        libc::sysctlbyname(
            key.as_ptr(),
            std::ptr::null_mut(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if status != 0 {
        return String::new();
    }
    let mut buffer = vec![0u8; size];
    // SAFETY: `buffer` holds `size` writable bytes, which `size` states.
    let status = unsafe {
        libc::sysctlbyname(
            key.as_ptr(),
            buffer.as_mut_ptr().cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if status != 0 {
        return String::new();
    }
    buffer.truncate(size);
    if buffer.last() == Some(&0) {
        buffer.pop();
    }
    text(&buffer)
}

/// Targets where the host name is not read as Go reads it: never reached,
/// because [`refuse_debug_logging`] refuses a level that logs.
#[cfg(not(any(
    target_os = "linux",
    target_os = "android",
    target_os = "macos",
    target_os = "ios",
    target_os = "freebsd",
    target_os = "dragonfly"
)))]
pub(in crate::command) fn hostname() -> String {
    String::new()
}

#[cfg(test)]
mod tests {
    use super::super::super::{run, Invocation};
    use super::*;

    fn env(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect()
    }

    fn level(pairs: &[(&str, &str)]) -> String {
        let map = env(pairs);
        resolved_level(|key| map.get(key).map(String::as_str)).to_owned()
    }

    fn record(pairs: &[(&str, &str)], name: &str, message: &str) -> Vec<u8> {
        let invocation = Invocation::new(".", ["x"]).with_env(env(pairs));
        let context = Context::new(&invocation, name).expect("a context");
        debug_error_record(&context, name, message)
    }

    #[test]
    fn log_level_resolution_follows_go() {
        assert_eq!(level(&[]), "info");
        assert_eq!(level(&[("LUUNA_MISSIONS_DEBUG", "1")]), "debug");
        assert_eq!(level(&[("LUUNA_MISSIONS_DEBUG", "0")]), "info");
        assert_eq!(level(&[("LUUNA_MISSIONS_DEBUG", "true")]), "info");
        for valid in ["trace", "debug", "info", "warn", "error", "fatal", "silent"] {
            assert_eq!(level(&[("LUUNA_MISSIONS_LOG_LEVEL", valid)]), valid);
        }
        // An explicit level wins over DEBUG=1: silent gives no record.
        let both = [
            ("LUUNA_MISSIONS_LOG_LEVEL", "silent"),
            ("LUUNA_MISSIONS_DEBUG", "1"),
        ];
        assert_eq!(level(&both), "silent");
        assert!(record(&both, "show", "boom").is_empty());
        // An invalid level falls back to the DEBUG rule.
        assert_eq!(
            level(&[
                ("LUUNA_MISSIONS_LOG_LEVEL", "loud"),
                ("LUUNA_MISSIONS_DEBUG", "1")
            ]),
            "debug"
        );
        assert_eq!(level(&[("LUUNA_MISSIONS_LOG_LEVEL", "loud")]), "info");
        // Values are neither trimmed nor case-folded.
        for odd in [" debug", "debug ", "DEBUG", "Trace", ""] {
            assert_eq!(
                level(&[("LUUNA_MISSIONS_LOG_LEVEL", odd)]),
                "info",
                "{odd:?}"
            );
        }
        assert_eq!(level(&[("LUUNA_MISSIONS_DEBUG", " 1")]), "info");
        assert_eq!(level(&[("LUUNA_MISSIONS_DEBUG", "1 ")]), "info");
        // Only debug and trace log.
        assert!(record(&[("LUUNA_MISSIONS_LOG_LEVEL", "warn")], "show", "x").is_empty());
        assert!(!record(&[("LUUNA_MISSIONS_LOG_LEVEL", "trace")], "show", "x").is_empty());
    }

    #[cfg(any(target_os = "linux", target_os = "android", target_os = "macos"))]
    #[test]
    fn the_debug_record_has_go_s_shape() {
        let bytes = record(
            &[("LUUNA_MISSIONS_DEBUG", "1")],
            "show",
            "a <b> & \"c\"\u{2028}",
        );
        let line = String::from_utf8(bytes).expect("UTF-8");
        assert!(line.ends_with("\"msg\":\"command failed\"}\n"), "{line}");
        assert_eq!(line.matches('\n').count(), 1);
        let value: serde_json::Value = serde_json::from_str(line.trim_end()).expect("JSON");
        let order = ["level", "time", "pid", "hostname", "err", "cmd", "msg"];
        let positions: Vec<usize> = order
            .iter()
            .map(|key| line.find(&format!("\"{key}\":")).expect(key))
            .collect();
        assert!(positions.windows(2).all(|pair| pair[0] < pair[1]), "{line}");
        assert!(line.starts_with("{\"level\":20,\"time\":"), "{line}");
        assert_eq!(value.as_object().unwrap().len(), 7);
        assert_eq!(value["level"], 20);
        assert!(value["time"].as_i64().unwrap() > 1_000_000_000_000);
        assert_eq!(value["pid"], std::process::id());
        assert_eq!(value["hostname"], hostname().as_str());
        assert_eq!(value["cmd"], "show");
        assert_eq!(value["msg"], "command failed");
        assert_eq!(value["err"]["type"], "Error");
        assert_eq!(value["err"]["message"], "a <b> & \"c\"\u{2028}");
        assert_eq!(value["err"]["stack"], "Error: a <b> & \"c\"\u{2028}");
        // HTML escaping is off; U+2028 is still escaped.
        assert!(
            line.contains(r#""message":"a <b> & \"c\"\u2028""#),
            "{line}"
        );
        assert!(
            line.contains(r#""stack":"Error: a <b> & \"c\"\u2028""#),
            "{line}"
        );
    }

    #[test]
    fn no_record_for_unknown_options_or_unknown_commands() {
        let dir = tempfile::tempdir().expect("a temporary folder");
        let logging = env(&[("LUUNA_MISSIONS_DEBUG", "1")]);
        for args in [
            &["--nope"][..],
            &["show", "--nope"],
            &["nope"],
            &["nope", "--json"],
        ] {
            let invocation =
                Invocation::new(dir.path(), args.iter().copied()).with_env(logging.clone());
            let outcome = match run(&invocation) {
                Ok(outcome) => outcome,
                // A target that refuses the logging environment.
                Err(_) if !HOSTNAME_AS_GO => continue,
                Err(error) => panic!("{args:?}: {error}"),
            };
            assert_eq!(outcome.exit_code, 1, "{args:?}");
            let stdout = String::from_utf8_lossy(&outcome.stdout);
            assert!(!stdout.contains("\"level\""), "{args:?}: {stdout}");
            assert!(!stdout.contains("command failed"), "{args:?}: {stdout}");
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn the_hostname_is_go_s() {
        let raw = std::fs::read("/proc/sys/kernel/hostname").expect("the kernel host name");
        let trimmed = raw.strip_suffix(b"\n").unwrap_or(&raw);
        assert_eq!(hostname(), text(trimmed));
        assert!(!hostname().is_empty());
        assert_eq!(text(b"a\xffb"), "a\u{fffd}b");
        assert_eq!(text(b"a\xe2\x82"), "a\u{fffd}\u{fffd}");
    }
}
