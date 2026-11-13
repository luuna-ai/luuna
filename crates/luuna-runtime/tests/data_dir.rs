//! The one data-folder resolver (`luuna_runtime::data_dir`) and the two
//! callers that share it: the service (`ServiceSettings::from_env`) and the
//! admin commands of the binary. Library tests inject the variable and the
//! current directory; binary tests start the real executable as a child with
//! its own environment and current directory, so no test changes this
//! process's environment or current directory. No test needs git, the
//! network, a model, Bun or a component toolchain.
use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
    Router,
};
use luuna_domain::{DirtyState, RepositoryIdentity, RepositoryObservation, StoreMode};
use luuna_runtime::{
    data_dir::{resolve_data_dir, DATA_DIR_VARIABLE, DEFAULT_DATA_DIR},
    router, AppState, ServiceSettings,
};
use luuna_storage::{write_project_store_mode, Store};
use serde_json::Value;
use std::{
    ffi::OsString,
    fs, io,
    path::{Path, PathBuf},
};
use tower::ServiceExt;

const TOKEN: &str = "synthetic-test-token-with-at-least-32-bytes";

/// The variable lookup of a service whose only settings are `value` for the
/// data folder (`None` leaves it unset) and the API token.
fn environment(value: Option<OsString>) -> impl Fn(&str) -> Option<OsString> {
    move |name| match name {
        "LUUNA_DATA_DIR" => value.clone(),
        "LUUNA_API_TOKEN" => Some(TOKEN.into()),
        _ => None,
    }
}

fn only(value: Option<&str>) -> impl Fn(&str) -> Option<OsString> {
    let value = value.map(OsString::from);
    move |name| (name == DATA_DIR_VARIABLE).then(|| value.clone()).flatten()
}

fn unreadable() -> io::Result<PathBuf> {
    Err(io::Error::new(io::ErrorKind::NotFound, "gone"))
}

#[test]
fn resolver_an_unset_variable_means_dot_luuna_in_the_current_directory() {
    assert_eq!(DATA_DIR_VARIABLE, "LUUNA_DATA_DIR");
    assert_eq!(DEFAULT_DATA_DIR, ".luuna");
    let base = tempfile::tempdir().unwrap();
    let resolved = resolve_data_dir(only(None), || Ok(base.path().to_path_buf())).unwrap();
    assert_eq!(resolved, base.path().join(".luuna"));
}

#[test]
fn resolver_uses_an_absolute_value_as_is_without_reading_the_current_directory() {
    let data = tempfile::tempdir().unwrap();
    let value = data.path().to_str().unwrap();
    let resolved = resolve_data_dir(only(Some(value)), || {
        panic!("an absolute value needs no current directory")
    })
    .unwrap();
    assert_eq!(resolved, data.path());
}

#[test]
fn resolver_joins_a_relative_or_empty_value_onto_the_current_directory() {
    let base = tempfile::tempdir().unwrap();
    let relative = resolve_data_dir(only(Some("rel/data")), || Ok(base.path().to_path_buf()));
    assert_eq!(relative.unwrap(), base.path().join("rel/data"));
    let empty = resolve_data_dir(only(Some("")), || Ok(base.path().to_path_buf()));
    assert_eq!(empty.unwrap(), base.path().join(""));
}

#[test]
fn resolver_names_the_variable_when_the_current_directory_cannot_be_read() {
    let error = resolve_data_dir(only(None), unreadable).unwrap_err();
    assert_eq!(
        error.to_string(),
        "LUUNA_DATA_DIR .luuna is relative and the current directory cannot be read: gone"
    );
    assert_eq!(format!("{error:?}"), error.to_string());
    let error = resolve_data_dir(only(Some("rel")), unreadable).unwrap_err();
    assert_eq!(
        error.to_string(),
        "LUUNA_DATA_DIR rel is relative and the current directory cannot be read: gone"
    );
    assert_eq!(format!("{error:?}"), error.to_string());
}

#[cfg(unix)]
#[test]
fn resolver_treats_a_non_utf8_value_as_a_path() {
    use std::os::unix::ffi::OsStringExt;
    let base = tempfile::tempdir().unwrap();
    let value = OsString::from_vec(b"data-\xff".to_vec());
    let relative = {
        let value = value.clone();
        resolve_data_dir(
            move |_| Some(value.clone()),
            || Ok(base.path().to_path_buf()),
        )
    };
    assert_eq!(relative.unwrap(), base.path().join(&value));

    let data = tempfile::tempdir().unwrap();
    let mut absolute = data.path().as_os_str().to_os_string();
    absolute.push("/");
    absolute.push(&value);
    let expected = PathBuf::from(&absolute);
    let resolved = resolve_data_dir(
        move |_| Some(absolute.clone()),
        || panic!("an absolute value needs no current directory"),
    );
    assert_eq!(resolved.unwrap(), expected);
}

#[test]
fn service_settings_use_the_shared_resolver() {
    let absolute = tempfile::tempdir().unwrap();
    let here = std::env::current_dir().unwrap();
    let existed = (here.join("rel/data").exists(), here.join(".luuna").exists());
    let values = [
        Some(absolute.path().as_os_str().to_os_string()),
        Some(OsString::from("rel/data")),
        Some(OsString::new()),
        None,
    ];
    for value in values {
        let var = environment(value.clone());
        let settings = ServiceSettings::from_env(&var).unwrap();
        let expected = resolve_data_dir(&var, std::env::current_dir).unwrap();
        assert_eq!(settings.runtime.data_dir, expected, "{value:?}");
        assert!(expected.is_absolute(), "{expected:?}");
        if value.is_some_and(|value| value == absolute.path().as_os_str()) {
            assert!(fs::read_dir(absolute.path()).unwrap().next().is_none());
        }
    }
    // Nothing is created for the relative, empty and unset values.
    assert_eq!(
        existed,
        (here.join("rel/data").exists(), here.join(".luuna").exists())
    );
}

fn observation(local_path: &Path) -> RepositoryObservation {
    RepositoryObservation {
        local_path: local_path.to_string_lossy().into_owned(),
        identity: RepositoryIdentity {
            remote: Some("https://example.invalid/app.git".into()),
            push_remote: None,
            root_commits: vec!["1".repeat(40)],
            shallow: false,
        },
        head_commit: "2".repeat(40),
        branch: Some("main".into()),
        remote_default_branch: None,
        local_branches: vec!["main".into()],
        dirty: DirtyState::default(),
        git_version: "git version 2.43.0".into(),
        observed_at: "2026-10-17T00:00:00.000Z".into(),
    }
}

async fn request(app: &Router, method: &str, path: &str) -> (StatusCode, Value) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header("Authorization", format!("Bearer {TOKEN}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 8 * 1024 * 1024)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

#[tokio::test]
async fn service_reads_project_files_from_the_folder_it_resolved() {
    let data = tempfile::tempdir().unwrap();
    let checkout = tempfile::tempdir().unwrap();
    let var = environment(Some(data.path().as_os_str().to_os_string()));
    let settings = ServiceSettings::from_env(&var).unwrap();
    assert_eq!(settings.runtime.data_dir, data.path());
    {
        let mut store = Store::open(data.path().join("luuna.sqlite3")).unwrap();
        store
            .register_project(
                "app",
                "Project",
                &[("api".into(), observation(checkout.path()))],
            )
            .unwrap();
    }
    let state = AppState::open(settings.runtime)
        .unwrap()
        .without_components();
    let app = router(state);
    let path = "/api/v1/projects/app/components";

    let (status, body) = request(&app, "GET", path).await;
    assert_eq!(status, StatusCode::OK);
    let expected = data.path().join("components").join("projects").join("app");
    assert_eq!(
        body["workspace_path"],
        Value::String(expected.to_string_lossy().into_owned())
    );

    let mode = StoreMode::Repository {
        repository: "api".into(),
    };
    write_project_store_mode(data.path(), "app", &mode).unwrap();
    let (status, _) = request(&app, "GET", path).await;
    assert_eq!(status, StatusCode::CONFLICT);

    fs::remove_file(data.path().join("projects/app/store.json")).unwrap();
    let (status, body) = request(&app, "GET", path).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body["workspace_path"],
        Value::String(expected.to_string_lossy().into_owned())
    );
}

#[cfg(unix)]
mod binary {
    use std::{
        ffi::OsString,
        io::{BufRead, BufReader, Read, Write},
        net::TcpStream,
        path::{Path, PathBuf},
        process::{Command, Output, Stdio},
        time::Duration,
    };

    const TOKEN: &str = "synthetic-test-token-with-at-least-32-bytes";
    const BINARY: &str = env!("CARGO_BIN_EXE_luuna-runtime");

    /// An admin command with a minimal environment, run in `cwd`. `variable`
    /// is LUUNA_DATA_DIR, or unset for `None`.
    fn admin(cwd: &Path, variable: Option<OsString>, arguments: &[OsString]) -> Output {
        let mut command = Command::new(BINARY);
        command
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .current_dir(cwd)
            .args(arguments);
        if let Some(variable) = variable {
            command.env("LUUNA_DATA_DIR", variable);
        }
        command.output().unwrap()
    }

    /// `restore` of a file that does not exist: it creates the data folder and
    /// `runtime.lock` before it reads the input, then fails.
    fn probe(missing: &Path, extra: &[OsString]) -> Vec<OsString> {
        let mut arguments = vec!["restore".into(), "--input".into(), missing.into()];
        arguments.extend(extra.iter().cloned());
        arguments
    }

    fn missing_input(root: &Path) -> PathBuf {
        root.join("missing-snapshot.sqlite3")
    }

    fn text(bytes: &[u8]) -> String {
        String::from_utf8_lossy(bytes).into_owned()
    }

    #[test]
    fn admin_defaults_to_dot_luuna_in_its_current_directory() {
        let cwd = tempfile::tempdir().unwrap();
        let input = tempfile::tempdir().unwrap();
        let canonical = cwd.path().canonicalize().unwrap();
        let output = admin(&canonical, None, &probe(&missing_input(input.path()), &[]));
        assert!(!output.status.success(), "{}", text(&output.stderr));
        assert!(canonical.join(".luuna/runtime.lock").exists());
    }

    #[test]
    fn admin_joins_a_relative_or_empty_variable_onto_its_current_directory() {
        let cwd = tempfile::tempdir().unwrap();
        let input = tempfile::tempdir().unwrap();
        let canonical = cwd.path().canonicalize().unwrap();
        let arguments = probe(&missing_input(input.path()), &[]);
        let output = admin(&canonical, Some("rel".into()), &arguments);
        assert!(!output.status.success(), "{}", text(&output.stderr));
        assert!(canonical.join("rel/runtime.lock").exists());
        let output = admin(&canonical, Some(OsString::new()), &arguments);
        assert!(!output.status.success(), "{}", text(&output.stderr));
        assert!(canonical.join("runtime.lock").exists());
    }

    #[test]
    fn admin_uses_a_non_utf8_variable_like_the_service() {
        use std::os::unix::ffi::OsStringExt;
        let cwd = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let input = tempfile::tempdir().unwrap();
        let canonical = cwd.path().canonicalize().unwrap();
        let mut variable = data.path().as_os_str().to_os_string();
        variable.push("/");
        variable.push(OsString::from_vec(b"data-\xff".to_vec()));
        let folder = PathBuf::from(&variable);
        let filesystem_result = std::fs::create_dir(&folder);
        let output = admin(
            &canonical,
            Some(variable),
            &probe(&missing_input(input.path()), &[]),
        );
        assert!(!output.status.success(), "{}", text(&output.stderr));
        match filesystem_result {
            Ok(()) => assert!(folder.join("runtime.lock").exists()),
            Err(error) => {
                // APFS rejects invalid UTF-8 filenames. Surface that error
                // rather than silently falling back to .luuna.
                assert_eq!(error.raw_os_error(), Some(libc::EILSEQ));
                assert!(text(&output.stderr).contains(&error.to_string()));
            }
        }
        assert!(!canonical.join(".luuna").exists());
    }

    #[test]
    fn admin_data_dir_flag_wins_over_the_variable_and_the_last_flag_wins() {
        let cwd = tempfile::tempdir().unwrap();
        let roots = tempfile::tempdir().unwrap();
        let input = tempfile::tempdir().unwrap();
        let (a, b, c) = (
            roots.path().join("a"),
            roots.path().join("b"),
            roots.path().join("c"),
        );
        let flags: Vec<OsString> = vec![
            "--data-dir".into(),
            b.clone().into(),
            "--data-dir".into(),
            c.clone().into(),
        ];
        let output = admin(
            cwd.path(),
            Some(a.clone().into()),
            &probe(&missing_input(input.path()), &flags),
        );
        assert!(!output.status.success(), "{}", text(&output.stderr));
        assert!(c.join("runtime.lock").exists());
        assert!(!a.exists());
        assert!(!b.exists());
    }

    /// The service on `LUUNA_DATA_DIR=rel` in `cwd`, stopped again before this
    /// returns. The data folder is complete once the listening line is shown.
    fn run_service_and_stop(cwd: &Path, home: &Path) {
        let mut child = Command::new(BINARY)
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", home)
            .env("LUUNA_API_TOKEN", TOKEN)
            .env("LUUNA_DATA_DIR", "rel")
            .env("LUUNA_PORT", "0")
            .current_dir(cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let stderr = child.stderr.take().unwrap();
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                if let Some(at) = line.find("http://") {
                    let address = line[at + "http://".len()..]
                        .split_whitespace()
                        .next()
                        .unwrap_or_default()
                        .trim_end_matches('/')
                        .to_owned();
                    let _ = sender.send(address);
                }
            }
        });
        let address = receiver.recv_timeout(Duration::from_secs(60));
        let result = address.map(|address| {
            assert!(cwd.join("rel/luuna.sqlite3").exists());
            assert!(cwd.join("rel/runtime.lock").exists());
            let mut stream = TcpStream::connect(&address).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(30)))
                .unwrap();
            write!(
                stream,
                "POST /api/v1/shutdown HTTP/1.1\r\nHost: {address}\r\nAuthorization: Bearer {TOKEN}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            )
            .unwrap();
            let mut response = Vec::new();
            let _ = stream.read_to_end(&mut response);
        });
        if result.is_err() {
            let _ = child.kill();
        }
        let _ = child.wait();
        result.expect("the runtime reports its loopback address");
    }

    #[test]
    fn admin_and_service_resolve_the_same_folder() {
        let cwd = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let canonical = cwd.path().canonicalize().unwrap();
        run_service_and_stop(&canonical, home.path());
        let output = admin(&canonical, Some("rel".into()), &["check".into()]);
        assert!(output.status.success(), "{}", text(&output.stderr));
        let stdout = text(&output.stdout);
        let first = stdout.lines().next().unwrap_or_default();
        let reported = first
            .strip_prefix("database: ")
            .unwrap_or_else(|| panic!("unexpected first line {first:?}"));
        assert_eq!(
            PathBuf::from(reported),
            canonical.join("rel").join("luuna.sqlite3")
        );
    }

    /// Run the binary in a current directory that has been removed, so only
    /// the child's current directory is gone.
    #[cfg(target_os = "linux")]
    fn admin_in_removed_directory(arguments: &[OsString]) -> Output {
        let gone = tempfile::tempdir().unwrap();
        Command::new("sh")
            .arg("-c")
            .arg("cd \"$1\" && rmdir \"$1\" && shift && exec \"$@\"")
            .arg("sh")
            .arg(gone.path())
            .arg(BINARY)
            .args(arguments)
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .output()
            .unwrap()
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn admin_without_a_readable_current_directory_reports_the_service_message() {
        let input = tempfile::tempdir().unwrap();
        let output = admin_in_removed_directory(&probe(&missing_input(input.path()), &[]));
        assert!(!output.status.success());
        let stderr = text(&output.stderr);
        assert!(
            stderr.contains(
                "LUUNA_DATA_DIR .luuna is relative and the current directory cannot be read"
            ),
            "{stderr}"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn admin_with_an_absolute_data_dir_needs_no_current_directory() {
        let input = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let flags: Vec<OsString> = vec!["--data-dir".into(), data.path().into()];
        let output = admin_in_removed_directory(&probe(&missing_input(input.path()), &flags));
        let stderr = text(&output.stderr);
        assert!(data.path().join("runtime.lock").exists(), "{stderr}");
        assert!(!stderr.contains("current directory"), "{stderr}");
    }
}
