mod support;
use luuna_tui::setup::git;
use std::process::Command;

#[tokio::test]
async fn explicit_stash_preserves_tracked_and_untracked_work_and_can_be_restored() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    for args in [
        vec!["init", "-q"],
        vec!["config", "user.name", "Fixture"],
        vec!["config", "user.email", "fixture@example.invalid"],
    ] {
        assert!(Command::new("git")
            .args(args)
            .current_dir(root)
            .status()
            .unwrap()
            .success());
    }
    std::fs::write(root.join("tracked"), "original\n").unwrap();
    git(root, &["add", "tracked"]).await.unwrap();
    git(root, &["commit", "-qm", "fixture"]).await.unwrap();
    std::fs::write(root.join("tracked"), "developer edit\n").unwrap();
    std::fs::write(root.join("untracked"), "new work\n").unwrap();
    assert!(!git(root, &["status", "--short"]).await.unwrap().is_empty());
    git(
        root,
        &[
            "stash",
            "push",
            "--include-untracked",
            "--message",
            "Luuna setup: saved before baseline",
        ],
    )
    .await
    .unwrap();
    assert!(git(root, &["status", "--short"]).await.unwrap().is_empty());
    assert_eq!(
        std::fs::read_to_string(root.join("tracked")).unwrap(),
        "original\n"
    );
    assert!(!root.join("untracked").exists());
    git(root, &["stash", "pop"]).await.unwrap();
    assert_eq!(
        std::fs::read_to_string(root.join("tracked")).unwrap(),
        "developer edit\n"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("untracked")).unwrap(),
        "new work\n"
    );
}

#[test]
fn configuration_command_writes_to_the_selected_file_and_rejects_invalid_edits() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("new/tui.json");
    let invoke = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_luuna-tui"))
            .args(args)
            .env("LUUNA_TUI_CONFIG", &path)
            .output()
            .unwrap()
    };
    let saved = invoke(&["config", "set", "keyboard_enhancement", "false"]);
    assert!(
        saved.status.success(),
        "{}",
        String::from_utf8_lossy(&saved.stderr)
    );
    let before = std::fs::read(&path).unwrap();
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&before).unwrap()["keyboard_enhancement"],
        false
    );
    assert!(!invoke(&["config", "set", "keyboard_enhancement", "42"])
        .status
        .success());
    assert_eq!(std::fs::read(&path).unwrap(), before);
}

#[tokio::test]
async fn setup_checks_the_existing_runtimes_authenticated_diagnostics() {
    use std::{
        collections::BTreeMap,
        sync::{Arc, Mutex},
    };
    let paths = Arc::new(Mutex::new(Vec::new()));
    let seen = paths.clone();
    let server = support::serve(move |request| {
        assert_eq!(
            request.header("authorization"),
            Some("Bearer synthetic-service-token")
        );
        seen.lock().unwrap().push(request.path.clone());
        match request.path.as_str() {
            "/api/v1/health" => support::json(serde_json::json!({"status":"ok"})),
            "/api/v1/doctor" => support::json(serde_json::json!({"ok":true,"items":[]})),
            _ => panic!("unexpected setup request: {}", request.path),
        }
    });
    let vars = BTreeMap::from([
        ("LUUNA_URL".into(), server.url()),
        ("LUUNA_API_TOKEN".into(), "synthetic-service-token".into()),
    ]);
    luuna_tui::setup::ensure_runtime(&vars).await.unwrap();
    assert_eq!(
        *paths.lock().unwrap(),
        vec!["/api/v1/health", "/api/v1/doctor"]
    );
}

#[test]
fn help_still_works_without_a_home_directory_or_saved_settings() {
    let result = Command::new(env!("CARGO_BIN_EXE_luuna-tui"))
        .arg("--help")
        .env_remove("HOME")
        .env_remove("USERPROFILE")
        .env_remove("XDG_CONFIG_HOME")
        .env_remove("APPDATA")
        .env_remove("LUUNA_TUI_CONFIG")
        .env_remove("LUUNA_PRIVATE_SETTINGS_LOADED")
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(!result.stdout.is_empty());
}

#[cfg(unix)]
#[tokio::test]
#[ignore = "downloads the pinned Pi dependency graph; run explicitly"]
async fn installed_setup_uses_the_embedded_pi_lock_without_a_source_checkout() {
    const CHILD: &str = "LUUNA_TEST_EMBEDDED_BOOTSTRAP";
    if std::env::var_os(CHILD).is_some() {
        let release = std::path::PathBuf::from(std::env::var_os(CHILD).unwrap());
        assert!(!release.join("tools/pi/package-lock.json").exists());
        let mut vars = std::collections::BTreeMap::new();
        luuna_tui::setup::prepare(&release, &mut vars)
            .await
            .unwrap();
        let arguments: Vec<String> = serde_json::from_str(&vars["LUUNA_PI_ARGS_JSON"]).unwrap();
        let output = Command::new(&vars["LUUNA_PI_COMMAND"])
            .args(arguments)
            .arg("--version")
            .output()
            .unwrap();
        assert!(output.status.success(), "Pi version probe failed");
        assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "1.3.0");
        let saved = luuna_tui::setup::directory()
            .unwrap()
            .join("environment.json");
        assert!(luuna_tui::setup::load(&saved)
            .unwrap()
            .contains_key("LUUNA_API_TOKEN"));
        return;
    }
    // Tool installation is sizeable. Use disk-backed temporary storage, never /tmp.
    let dir = tempfile::tempdir_in("/var/tmp").unwrap();
    let release = dir.path().join("release/bin");
    std::fs::create_dir_all(&release).unwrap();
    let mut child = Command::new(std::env::current_exe().unwrap());
    child
        .args([
            "installed_setup_uses_the_embedded_pi_lock_without_a_source_checkout",
            "--exact",
            "--ignored",
            "--nocapture",
        ])
        .env(CHILD, &release)
        .env("HOME", dir.path().join("home"))
        .env("LUUNA_TUI_CONFIG", dir.path().join("config/tui.json"))
        .env("OPENROUTER_API_KEY", "synthetic-unused-provider-key");
    for name in [
        "LUUNA_API_TOKEN",
        "LUUNA_PI_COMMAND",
        "LUUNA_PI_ARGS_JSON",
        "LUUNA_BUN",
        "LUUNA_COMPONENTS_ROOT",
        "LUUNA_PI_AUTH_FILE",
    ] {
        child.env_remove(name);
    }
    assert!(
        child.status().unwrap().success(),
        "Disposable first-run setup failed"
    );
}
