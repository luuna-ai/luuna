//! The project store resolver: which component store a project's requests
//! reach. Runtime mode keeps the managed workspace under the data directory;
//! every other mode, and every store file that cannot be trusted, is refused
//! before any component data is touched. No test needs git, a component
//! toolchain, Bun, the network or a model.
use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
    Router,
};
use luuna_domain::{DirtyState, RepositoryIdentity, RepositoryObservation, StoreMode, StoreSide};
use luuna_runtime::{
    router,
    stores::{resolve_project_store, user_store_path, StoreRefusal},
    AppState, RuntimeConfig,
};
use luuna_storage::{write_project_store_mode, Store};
use serde_json::Value;
use std::{fs, path::Path, path::PathBuf};
use tower::ServiceExt;

const TOKEN: &str = "synthetic-test-token-with-at-least-32-bytes";
const REMOTE: &str = "https://example.invalid/app.git";

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
/// Register project `id` with one repository at `checkout` in the database
/// the runtime will open, then close that database again.
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
fn repository() -> StoreMode {
    StoreMode::Repository {
        repository: "api".into(),
    }
}
fn both(writable: StoreSide) -> StoreMode {
    StoreMode::Both {
        repository: "api".into(),
        writable,
    }
}
fn non_runtime_modes() -> Vec<StoreMode> {
    vec![
        repository(),
        both(StoreSide::Runtime),
        both(StoreSide::Repository),
    ]
}
fn is_empty(directory: &Path) -> bool {
    fs::read_dir(directory).unwrap().next().is_none()
}

#[test]
fn runtime_mode_resolves_to_the_managed_workspace_under_the_data_directory() {
    let data = tempfile::tempdir().unwrap();
    assert_eq!(
        resolve_project_store(data.path(), "app", &StoreMode::Runtime),
        Ok(data.path().join("components").join("projects").join("app"))
    );
}

#[test]
fn user_scope_resolves_to_the_single_user_workspace() {
    let data = tempfile::tempdir().unwrap();
    assert_eq!(
        user_store_path(data.path()),
        data.path().join("components").join("user")
    );
}

#[test]
fn invalid_project_ids_never_form_a_store_path() {
    let data = tempfile::tempdir().unwrap();
    let long = "a".repeat(65);
    let mut modes = vec![StoreMode::Runtime];
    modes.extend(non_runtime_modes());
    for id in ["", "../x", "A", "a/b", "a b", long.as_str()] {
        for mode in &modes {
            assert_eq!(
                resolve_project_store(data.path(), id, mode),
                Err(StoreRefusal::InvalidProject),
                "{id:?} in {mode:?}"
            );
        }
    }
}

#[test]
fn repository_and_both_modes_are_refused_without_touching_the_filesystem() {
    let data = tempfile::tempdir().unwrap();
    for mode in non_runtime_modes() {
        assert_eq!(
            resolve_project_store(data.path(), "app", &mode),
            Err(StoreRefusal::NotAvailable(mode.clone()))
        );
        assert!(
            is_empty(data.path()),
            "{mode:?} left something in the data directory"
        );
    }
}

#[tokio::test]
async fn an_unknown_project_is_not_found_even_with_a_stray_store_file() {
    let data = tempfile::tempdir().unwrap();
    write_project_store_mode(data.path(), "nope", &repository()).unwrap();
    let app = serve(data.path());
    let (status, _) = request(&app, "GET", "/api/v1/projects/nope/components").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn without_a_store_file_the_workspace_is_unchanged() {
    let data = tempfile::tempdir().unwrap();
    let checkout = tempfile::tempdir().unwrap();
    register(data.path(), "app", checkout.path());
    let app = serve(data.path());
    let (status, body) = request(&app, "GET", "/api/v1/projects/app/components").await;
    assert_eq!(status, StatusCode::OK);
    let expected = data.path().join("components").join("projects").join("app");
    assert_eq!(
        body["workspace_path"],
        Value::String(expected.to_string_lossy().into_owned())
    );
    assert!(!data.path().join("projects").exists());
}

#[tokio::test]
async fn a_runtime_store_file_resolves_like_no_file() {
    let data = tempfile::tempdir().unwrap();
    let checkout = tempfile::tempdir().unwrap();
    register(data.path(), "app", checkout.path());
    let app = serve(data.path());
    let path = "/api/v1/projects/app/components";

    let (status, without_file) = request(&app, "GET", path).await;
    assert_eq!(status, StatusCode::OK);
    write_project_store_mode(data.path(), "app", &StoreMode::Runtime).unwrap();
    let (status, with_file) = request(&app, "GET", path).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(with_file, without_file);
    let expected = data.path().join("components").join("projects").join("app");
    assert_eq!(
        with_file["workspace_path"],
        Value::String(expected.to_string_lossy().into_owned())
    );
}

#[tokio::test]
async fn repository_and_both_store_files_fail_closed_over_the_api() {
    for mode in non_runtime_modes() {
        let data = tempfile::tempdir().unwrap();
        let checkout = tempfile::tempdir().unwrap();
        register(data.path(), "app", checkout.path());
        write_project_store_mode(data.path(), "app", &mode).unwrap();
        let file = data.path().join("projects").join("app").join("store.json");
        let before = fs::read(&file).unwrap();

        let app = serve(data.path());
        for method in ["GET", "POST"] {
            let (status, _) = request(&app, method, "/api/v1/projects/app/components").await;
            assert_eq!(status, StatusCode::CONFLICT, "{method} in {mode:?}");
        }
        drop(app);

        assert!(
            !data
                .path()
                .join("components")
                .join("projects")
                .join("app")
                .exists(),
            "{mode:?} created a managed workspace"
        );
        assert!(
            is_empty(checkout.path()),
            "{mode:?} wrote into the checkout"
        );
        assert_eq!(
            fs::read(&file).unwrap(),
            before,
            "{mode:?} rewrote store.json"
        );
        let store = Store::open(data.path().join("luuna.sqlite3")).unwrap();
        assert_eq!(store.component_state("app").unwrap(), None, "{mode:?}");
    }
}

#[tokio::test]
async fn an_unusable_store_file_fails_closed_over_the_api() {
    let cases: [(&str, &[u8]); 4] = [
        ("not json", b"this is not json"),
        (
            "unknown key",
            br#"{"schema_version":1,"project_id":"app","mode":"runtime","extra":true}"#,
        ),
        (
            "schema_version 2",
            br#"{"schema_version":2,"project_id":"app","mode":"runtime"}"#,
        ),
        (
            "another project",
            br#"{"schema_version":1,"project_id":"other","mode":"runtime"}"#,
        ),
    ];
    for (name, bytes) in cases {
        let data = tempfile::tempdir().unwrap();
        let checkout = tempfile::tempdir().unwrap();
        register(data.path(), "app", checkout.path());
        put_store_file(data.path(), "app", bytes);
        let app = serve(data.path());
        let (status, _) = request(&app, "GET", "/api/v1/projects/app/components").await;
        assert_eq!(status, StatusCode::CONFLICT, "{name}");
        assert!(
            !data
                .path()
                .join("components")
                .join("projects")
                .join("app")
                .exists(),
            "{name} created a managed workspace"
        );
    }
}

#[tokio::test]
async fn the_store_file_is_read_on_every_resolution() {
    let data = tempfile::tempdir().unwrap();
    let checkout = tempfile::tempdir().unwrap();
    register(data.path(), "app", checkout.path());
    let app = serve(data.path());
    let path = "/api/v1/projects/app/components";

    let (status, _) = request(&app, "GET", path).await;
    assert_eq!(status, StatusCode::OK);
    write_project_store_mode(data.path(), "app", &repository()).unwrap();
    let (status, _) = request(&app, "GET", path).await;
    assert_eq!(status, StatusCode::CONFLICT);
    fs::remove_file(data.path().join("projects").join("app").join("store.json")).unwrap();
    let (status, _) = request(&app, "GET", path).await;
    assert_eq!(status, StatusCode::OK);
}
