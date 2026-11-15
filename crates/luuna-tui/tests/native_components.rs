//! Installed component aliases and named dispatch need no checkout or runtime setup.
use std::process::{Command, Stdio};
fn run(cwd: &std::path::Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_luuna-tui"))
        .args(args)
        .current_dir(cwd)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", cwd.join("absent-home"))
        .env("LUUNA_BUN", cwd.join("missing-bun"))
        .stdin(Stdio::null())
        .output()
        .unwrap()
}
#[test]
fn native_commands_work_outside_checkout_without_private_settings() {
    let dir = tempfile::tempdir().unwrap();
    for name in ["atlas", "beacon", "missions"] {
        let out = run(dir.path(), &[name, "init"]);
        assert!(
            out.status.success(),
            "{name}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(dir
            .path()
            .join(format!(".luuna/{name}/config.yaml"))
            .is_file());
    }
    assert!(!dir.path().join("absent-home").exists());
    let out = run(dir.path(), &["atlas", "not-a-command"]);
    assert!(!out.status.success());
}
#[cfg(unix)]
#[test]
fn installed_alias_preserves_piped_input_and_never_loads_runtime_settings() {
    use std::io::Write;
    let dir = tempfile::tempdir().unwrap();
    let alias = dir.path().join("lat");
    std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_luuna-tui"), &alias).unwrap();
    assert!(run(dir.path(), &["atlas", "init"]).status.success());
    let mut child = Command::new(alias)
        .args([
            "record",
            "example",
            "--type",
            "convention",
            "--stdin",
            "--json",
        ])
        .current_dir(dir.path())
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(br#"{"content":"Piped native memory","type":"convention"}"#)
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&out.stdout).unwrap()["success"],
        true
    );
    let stored = run(
        dir.path(),
        &["atlas", "search", "Piped native memory", "--json"],
    );
    assert!(stored.status.success());
    assert!(String::from_utf8_lossy(&stored.stdout).contains("Piped native memory"));
}
