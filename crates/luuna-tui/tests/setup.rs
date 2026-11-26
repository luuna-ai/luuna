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

#[test]
fn minted_tokens_are_long_random_and_different() {
    let first = luuna_tui::setup::mint_token().unwrap();
    let second = luuna_tui::setup::mint_token().unwrap();
    assert_eq!(first.len(), 64);
    assert!(first.chars().all(|c| c.is_ascii_hexdigit()));
    assert_ne!(first, second);
}

fn closed_url() -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    format!("http://127.0.0.1:{port}")
}

#[tokio::test]
async fn a_token_is_minted_and_saved_when_none_is_set_and_nothing_answers() {
    use std::collections::BTreeMap;
    let dir = tempfile::tempdir().unwrap();
    let private = dir.path().join("private");
    let url = closed_url();
    let mut vars = BTreeMap::new();
    let minted =
        luuna_tui::setup::mint_service_token(&mut vars, |_| None, &url, || Ok(private.clone()))
            .await
            .unwrap();
    assert!(minted);
    let token = vars["LUUNA_API_TOKEN"].clone();
    assert_eq!(token.len(), 64);
    let saved = luuna_tui::setup::load(&private.join("environment.json")).unwrap();
    assert_eq!(saved.get("LUUNA_API_TOKEN"), Some(&token));
    assert!(saved.contains_key("LUUNA_DATA_DIR"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = |p: &std::path::Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&private.join("environment.json")), 0o600);
        assert_eq!(mode(&private), 0o700);
    }
    // A second launch keeps the saved token.
    let again =
        luuna_tui::setup::mint_service_token(&mut vars, |_| None, &url, || Ok(private.clone()))
            .await
            .unwrap();
    assert!(!again);
    assert_eq!(vars["LUUNA_API_TOKEN"], token);
}

#[tokio::test]
async fn no_token_is_minted_over_a_set_token_another_url_or_a_running_runtime() {
    use std::collections::BTreeMap;
    let dir = tempfile::tempdir().unwrap();
    let private = dir.path().join("private");
    let url = closed_url();
    let unused = || -> std::io::Result<std::path::PathBuf> { panic!("nothing should be saved") };
    // An explicit LUUNA_API_TOKEN wins.
    let mut vars = BTreeMap::new();
    let env = |name: &str| (name == "LUUNA_API_TOKEN").then(|| "explicit".to_string());
    assert!(
        !luuna_tui::setup::mint_service_token(&mut vars, env, &url, unused)
            .await
            .unwrap()
    );
    // A runtime elsewhere is not Luuna's to give a token.
    let env = |name: &str| (name == "LUUNA_URL").then(|| "http://127.0.0.1:1".to_string());
    assert!(
        !luuna_tui::setup::mint_service_token(&mut vars, env, &url, unused)
            .await
            .unwrap()
    );
    // A runtime someone started by hand already has a token Luuna cannot know.
    let server = support::serve(|request| match request.path.as_str() {
        "/api/v1/health" | "/health" => support::json(serde_json::json!({"status":"ok"})),
        other => panic!("unexpected request: {other}"),
    });
    let running = server.url();
    assert!(!luuna_tui::setup::mint_service_token(
        &mut vars,
        |_| None,
        &running,
        || Ok(private.clone())
    )
    .await
    .unwrap());
    assert!(vars.is_empty());
    assert!(!private.exists());
}

#[test]
fn help_never_mints_a_token() {
    let dir = tempfile::tempdir().unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_luuna-tui"))
        .args(["doctor", "--help"])
        .env("HOME", dir.path())
        .env("XDG_CONFIG_HOME", dir.path().join("config"))
        .env_remove("LUUNA_TUI_CONFIG")
        .env_remove("LUUNA_API_TOKEN")
        .env_remove("LUUNA_URL")
        .env_remove("LUUNA_PRIVATE_SETTINGS_LOADED")
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    // Nothing is created: no private folder, token or settings file.
    assert!(!dir.path().join("config").exists());
}

#[tokio::test]
async fn concurrent_first_launches_share_one_token() {
    use std::collections::BTreeMap;
    let dir = tempfile::tempdir().unwrap();
    let private = dir.path().join("private");
    let url = closed_url();
    let launch = || {
        let private = private.clone();
        let url = url.clone();
        async move {
            let mut vars = BTreeMap::new();
            luuna_tui::setup::mint_service_token(&mut vars, |_| None, &url, || Ok(private))
                .await
                .unwrap();
            vars
        }
    };
    let launches = tokio::join!(launch(), launch(), launch());
    let saved = luuna_tui::setup::load(&private.join("environment.json")).unwrap();
    // Every launch ends with the saved token and data folder.
    for vars in [&launches.0, &launches.1, &launches.2] {
        assert_eq!(vars, &saved);
    }
    assert!(!private.join(".mint.lock").exists());
}

#[tokio::test]
async fn init_stops_the_runtime_it_started_so_new_settings_apply() {
    use std::collections::BTreeMap;
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };
    let stopped = Arc::new(AtomicBool::new(false));
    let seen = stopped.clone();
    let server = support::serve(move |request| match request.path.as_str() {
        "/api/v1/health" | "/health" if !seen.load(Ordering::SeqCst) => {
            support::json(serde_json::json!({"status":"ok"}))
        }
        "/api/v1/projects" if !seen.load(Ordering::SeqCst) => {
            support::json(serde_json::json!({"projects":[]}))
        }
        "/api/v1/shutdown" => {
            assert_eq!(
                request.header("authorization"),
                Some("Bearer saved-service-token-0123456789abcdef")
            );
            seen.store(true, Ordering::SeqCst);
            support::json(serde_json::json!({"status":"shutting_down"}))
        }
        _ => support::status(503, "{\"error\":\"stopped\"}"),
    });
    let vars = BTreeMap::from([(
        "LUUNA_API_TOKEN".to_string(),
        "saved-service-token-0123456789abcdef".to_string(),
    )]);
    luuna_tui::setup::stop_owned_runtime(&vars, &server.url())
        .await
        .unwrap();
    assert!(stopped.load(Ordering::SeqCst));
}

#[tokio::test]
async fn a_runtime_another_launch_started_is_adopted_with_its_saved_settings() {
    use std::collections::BTreeMap;
    let dir = tempfile::tempdir().unwrap();
    let private = dir.path().join("private");
    let url = closed_url();
    let mut first = BTreeMap::new();
    luuna_tui::setup::mint_service_token(&mut first, |_| None, &url, || Ok(private.clone()))
        .await
        .unwrap();
    // The first launch's runtime now answers.
    let server = support::serve(|request| match request.path.as_str() {
        "/api/v1/health" | "/health" => support::json(serde_json::json!({"status":"ok"})),
        other => panic!("unexpected request: {other}"),
    });
    let mut second = BTreeMap::new();
    let minted = luuna_tui::setup::mint_service_token(
        &mut second,
        |_| None,
        &server.url(),
        || Ok(private.clone()),
    )
    .await
    .unwrap();
    assert!(!minted);
    assert_eq!(second, first);
}

#[tokio::test]
async fn a_runtime_that_rejects_the_saved_token_is_not_stopped() {
    use std::collections::BTreeMap;
    let server = support::serve(|request| match request.path.as_str() {
        "/api/v1/health" | "/health" => support::json(serde_json::json!({"status":"ok"})),
        "/api/v1/projects" => support::status(401, "{\"error\":\"authentication required\"}"),
        other => panic!("unexpected request: {other}"),
    });
    let vars = BTreeMap::from([(
        "LUUNA_API_TOKEN".to_string(),
        "saved-service-token-0123456789abcdef".to_string(),
    )]);
    luuna_tui::setup::stop_owned_runtime(&vars, &server.url())
        .await
        .unwrap();
}
