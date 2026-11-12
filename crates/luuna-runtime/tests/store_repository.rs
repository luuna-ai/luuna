//! The project store resolver, second half: store files that cannot be
//! trusted (repeated keys, a repository that is not registered) and modes
//! that are not available yet are refused with a fixed message before any
//! component data is touched, and a refused repository store leaves the
//! checkout alone. No test needs git, a component toolchain, Bun, the network
//! or a model.
use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
    Router,
};
use luuna_domain::{
    DirtyState, ProjectStoreFile, RepositoryIdentity, RepositoryObservation, StoreMode, StoreSide,
};
use luuna_runtime::{router, AppState, RuntimeConfig};
use luuna_storage::{read_project_store_mode, write_project_store_mode, Store};
use serde_json::{json, Value};
use std::{fs, path::Path, path::PathBuf};
use tower::ServiceExt;

const TOKEN: &str = "synthetic-test-token-with-at-least-32-bytes";
const REMOTE: &str = "https://example.invalid/app.git";
const NOT_REGISTERED: &str = "This project's store mode file names a repository that is not registered for this project; fix or remove the file. No component data was read or written";
const UNUSABLE: &str = "This project's store mode file in the data directory cannot be used; fix or remove it. No component data was read or written";

fn not_available(mode: &str) -> String {
    format!("This project's component store mode is '{mode}'; only the runtime store is available in this version. No component data was read or written")
}

fn configuration(data_dir: PathBuf) -> RuntimeConfig {
    RuntimeConfig {
        data_dir,
        pi_program: if cfg!(windows) { "python" } else { "python3" }.into(),
        pi_launcher_args: vec![],
        git_program: "git".into(),
        provider: None,
        model: None,
        token: TOKEN.into(),
        max_run_seconds: 10,
    }
}
/// A runtime with no component toolchain over `data_dir`. The router owns the
/// only state, so dropping it releases the database.
fn serve(data_dir: &Path) -> Router {
    let state = AppState::open(configuration(data_dir.to_path_buf()))
        .unwrap()
        .without_components();
    router(state)
}
async fn request(
    app: &Router,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let builder = Request::builder()
        .method(method)
        .uri(path)
        .header("Authorization", format!("Bearer {TOKEN}"));
    let request = match body {
        Some(body) => builder
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap(),
        None => builder.body(Body::empty()).unwrap(),
    };
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 8 * 1024 * 1024)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}
fn observation(local_path: &Path) -> RepositoryObservation {
    RepositoryObservation {
        local_path: local_path.to_string_lossy().into_owned(),
        identity: RepositoryIdentity {
            remote: Some(REMOTE.into()),
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
/// Register project `id` with one repository `api` at `checkout` in the
/// database the runtime will open, then close that database again.
fn register(data_dir: &Path, id: &str, checkout: &Path) {
    let mut store = Store::open(data_dir.join("luuna.sqlite3")).unwrap();
    store
        .register_project(id, "Project", &[("api".into(), observation(checkout))])
        .unwrap();
}
fn put_store_file(data_dir: &Path, id: &str, bytes: &[u8]) {
    let directory = data_dir.join("projects").join(id);
    fs::create_dir_all(&directory).unwrap();
    fs::write(directory.join("store.json"), bytes).unwrap();
}
fn is_empty(directory: &Path) -> bool {
    fs::read_dir(directory).unwrap().next().is_none()
}
fn error_text(body: &Value) -> String {
    body["error"].as_str().unwrap_or_default().to_owned()
}

#[test]
fn duplicate_keys_in_a_store_file_are_refused() {
    let texts = [
        r#"{"schema_version":1,"project_id":"app","mode":"repository","repository":"api","mode":"runtime"}"#,
        r#"{"schema_version":1,"project_id":"app","mode":"runtime","mode":"runtime"}"#,
        r#"{"schema_version":1,"project_id":"app","mode":"repository","repository":"api","repository":"web"}"#,
        r#"{"schema_version":1,"project_id":"other","project_id":"app","mode":"runtime"}"#,
        r#"{"schema_version":1,"schema_version":1,"project_id":"app","mode":"runtime"}"#,
        r#"{"schema_version":1,"project_id":"app","mode":"repository","mode":"runtime"}"#,
    ];
    for text in texts {
        let error = ProjectStoreFile::parse(text.as_bytes())
            .expect_err(text)
            .to_string();
        assert!(!error.contains("web"), "{text}: {error}");
        assert!(!error.contains("other"), "{text}: {error}");
        let data = tempfile::tempdir().unwrap();
        put_store_file(data.path(), "app", text.as_bytes());
        assert!(
            read_project_store_mode(data.path(), "app").is_err(),
            "{text}"
        );
    }
    let control =
        r#"{"schema_version":1,"project_id":"app","mode":"repository","repository":"api"}"#;
    assert_eq!(
        ProjectStoreFile::parse(control.as_bytes()).unwrap().mode,
        StoreMode::Repository {
            repository: "api".into()
        }
    );
    let newer = r#"{"schema_version":2,"schema_version":2,"project_id":"app","mode":"runtime"}"#;
    let error = ProjectStoreFile::parse(newer.as_bytes())
        .unwrap_err()
        .to_string();
    assert!(error.contains("newer Luuna"), "{error}");
}

#[tokio::test]
async fn a_duplicate_key_store_file_fails_closed_over_the_api() {
    let data = tempfile::tempdir().unwrap();
    let checkout = tempfile::tempdir().unwrap();
    register(data.path(), "app", checkout.path());
    put_store_file(
        data.path(),
        "app",
        br#"{"schema_version":1,"project_id":"app","mode":"repository","mode":"runtime"}"#,
    );
    let app = serve(data.path());
    let (status, body) = request(&app, "GET", "/api/v1/projects/app/components", None).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(error_text(&body), UNUSABLE, "{body}");
    drop(app);
    assert!(!data
        .path()
        .join("components")
        .join("projects")
        .join("app")
        .exists());
    assert!(is_empty(checkout.path()));
}

#[tokio::test]
async fn a_store_file_naming_an_unregistered_repository_is_refused() {
    let modes = [
        StoreMode::Repository {
            repository: "ghost-repo".into(),
        },
        StoreMode::Both {
            repository: "ghost-repo".into(),
            writable: StoreSide::Runtime,
        },
        StoreMode::Both {
            repository: "ghost-repo".into(),
            writable: StoreSide::Repository,
        },
    ];
    for mode in modes {
        let data = tempfile::tempdir().unwrap();
        let checkout = tempfile::tempdir().unwrap();
        register(data.path(), "app", checkout.path());
        write_project_store_mode(data.path(), "app", &mode).unwrap();
        let file = data.path().join("projects").join("app").join("store.json");
        let before = fs::read(&file).unwrap();
        let app = serve(data.path());
        for method in ["GET", "POST"] {
            let (status, body) =
                request(&app, method, "/api/v1/projects/app/components", None).await;
            assert_eq!(status, StatusCode::CONFLICT, "{mode:?} {method}: {body}");
            assert_eq!(error_text(&body), NOT_REGISTERED, "{mode:?} {method}");
        }
        drop(app);
        assert!(!data
            .path()
            .join("components")
            .join("projects")
            .join("app")
            .exists());
        assert!(is_empty(checkout.path()));
        assert_eq!(fs::read(&file).unwrap(), before);
        let store = Store::open(data.path().join("luuna.sqlite3")).unwrap();
        assert_eq!(store.component_state("app").unwrap(), None);
    }
}

#[tokio::test]
async fn a_registered_repository_is_refused_with_the_mode_named() {
    let cases = [
        (
            StoreMode::Repository {
                repository: "api".into(),
            },
            "repository",
        ),
        (
            StoreMode::Both {
                repository: "api".into(),
                writable: StoreSide::Runtime,
            },
            "both",
        ),
        (
            StoreMode::Both {
                repository: "api".into(),
                writable: StoreSide::Repository,
            },
            "both",
        ),
    ];
    for (mode, name) in cases {
        let data = tempfile::tempdir().unwrap();
        let checkout = tempfile::tempdir().unwrap();
        register(data.path(), "app", checkout.path());
        write_project_store_mode(data.path(), "app", &mode).unwrap();
        let app = serve(data.path());
        let (status, body) = request(&app, "GET", "/api/v1/projects/app/components", None).await;
        assert_eq!(status, StatusCode::CONFLICT, "{mode:?}: {body}");
        assert_eq!(error_text(&body), not_available(name), "{mode:?}");
    }
}

#[tokio::test]
async fn an_unusable_store_file_keeps_its_message() {
    let data = tempfile::tempdir().unwrap();
    let checkout = tempfile::tempdir().unwrap();
    register(data.path(), "app", checkout.path());
    put_store_file(data.path(), "app", b"this is not json");
    let app = serve(data.path());
    let (status, body) = request(&app, "GET", "/api/v1/projects/app/components", None).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(error_text(&body), UNUSABLE);
}

#[tokio::test]
async fn refusal_messages_carry_no_path_or_file_content() {
    let mut texts = Vec::new();
    let mut paths = Vec::new();
    // (3) an unregistered repository, (4) a registered one, (5) an unusable file.
    for case in 0..3 {
        let data = tempfile::tempdir().unwrap();
        let checkout = tempfile::tempdir().unwrap();
        register(data.path(), "app", checkout.path());
        match case {
            0 => write_project_store_mode(
                data.path(),
                "app",
                &StoreMode::Repository {
                    repository: "ghost-repo".into(),
                },
            )
            .unwrap(),
            1 => write_project_store_mode(
                data.path(),
                "app",
                &StoreMode::Repository {
                    repository: "api".into(),
                },
            )
            .unwrap(),
            _ => put_store_file(data.path(), "app", b"this is not json"),
        }
        let app = serve(data.path());
        let (status, body) = request(&app, "GET", "/api/v1/projects/app/components", None).await;
        assert_eq!(status, StatusCode::CONFLICT, "{body}");
        texts.push(error_text(&body));
        paths.push((
            data.path().to_string_lossy().into_owned(),
            checkout.path().to_string_lossy().into_owned(),
        ));
    }
    for (text, (data, checkout)) in texts.iter().zip(paths) {
        assert!(!text.is_empty());
        for forbidden in [data.as_str(), checkout.as_str(), "ghost-repo", "store.json"] {
            assert!(!text.contains(forbidden), "{forbidden}: {text}");
        }
    }
}

/// Sorted relative paths with entry type and, for files, the bytes.
fn snapshot(root: &Path) -> Vec<(String, String, Vec<u8>)> {
    fn walk(root: &Path, directory: &Path, out: &mut Vec<(String, String, Vec<u8>)>) {
        for entry in fs::read_dir(directory).unwrap() {
            let path = entry.unwrap().path();
            let relative = path
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .into_owned();
            let kind = fs::symlink_metadata(&path).unwrap().file_type();
            if kind.is_symlink() {
                out.push((relative, "symlink".into(), Vec::new()));
            } else if kind.is_dir() {
                out.push((relative, "dir".into(), Vec::new()));
                walk(root, &path, out);
            } else {
                out.push((relative, "file".into(), fs::read(&path).unwrap()));
            }
        }
    }
    let mut out = Vec::new();
    walk(root, root, &mut out);
    out.sort();
    out
}

#[cfg(unix)]
#[tokio::test]
async fn a_refused_repository_store_leaves_the_checkout_untouched() {
    use std::os::unix::fs::PermissionsExt;
    let data = tempfile::tempdir().unwrap();
    let checkout = tempfile::tempdir().unwrap();
    fs::set_permissions(checkout.path(), fs::Permissions::from_mode(0o755)).unwrap();
    fs::create_dir_all(checkout.path().join(".seeds")).unwrap();
    fs::write(checkout.path().join(".seeds").join("issues.jsonl"), "{}\n").unwrap();
    fs::create_dir_all(checkout.path().join(".luuna").join("missions")).unwrap();
    fs::write(
        checkout
            .path()
            .join(".luuna")
            .join("missions")
            .join("config.yaml"),
        "project: demo\n",
    )
    .unwrap();
    fs::write(checkout.path().join(".gitattributes"), "* text=auto\n").unwrap();
    register(data.path(), "app", checkout.path());
    {
        let store = Store::open(data.path().join("luuna.sqlite3")).unwrap();
        store.begin_component_enable("app").unwrap();
        store.finish_component_enable("app").unwrap();
    }
    write_project_store_mode(
        data.path(),
        "app",
        &StoreMode::Repository {
            repository: "api".into(),
        },
    )
    .unwrap();
    let before = snapshot(checkout.path());
    let app = serve(data.path());
    let expected = not_available("repository");
    for method in ["GET", "POST"] {
        let (status, body) = request(&app, method, "/api/v1/projects/app/components", None).await;
        assert_eq!(status, StatusCode::CONFLICT, "{method}: {body}");
        assert_eq!(error_text(&body), expected, "{method}");
    }
    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/projects/app/tickets",
        Some(json!({
            "idempotency_key": "key-repository-store",
            "title": "Repository store ticket",
            "intent": "Prove the refusal.",
            "acceptance_criteria": ["refused"],
            "repository_ids": ["api"],
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(error_text(&body), expected);
    for path in [
        "/api/v1/projects/app/memory",
        "/api/v1/projects/app/prompts",
    ] {
        let (status, body) = request(&app, "GET", path, None).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{path}: {body}");
    }
    drop(app);
    assert_eq!(snapshot(checkout.path()), before);
    let mode = fs::metadata(checkout.path()).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o755);
    assert!(!checkout.path().join(".luuna-component-home").exists());
    assert!(!data
        .path()
        .join("components")
        .join("projects")
        .join("app")
        .exists());
}
