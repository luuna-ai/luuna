//! The native Roots module resolves a project's Roots store through the store
//! resolver and refuses before touching anything when the store cannot be
//! used. No test needs git, a component toolchain, Bun, the network or a
//! model.
use axum::http::StatusCode;
use luuna_domain::{DirtyState, RepositoryIdentity, RepositoryObservation, StoreMode};
use luuna_runtime::{roots::RootsStore, AppState, RuntimeConfig};
use luuna_storage::{write_project_store_mode, Store};
use roots_core::config::{default_config, write_config};
use roots_core::paths::roots_paths;
use std::{fs, path::Path, path::PathBuf};

const TOKEN: &str = "synthetic-test-token-with-at-least-32-bytes";

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

fn open(data_dir: &Path) -> AppState {
    AppState::open(configuration(data_dir.to_path_buf()))
        .unwrap()
        .without_components()
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
        observed_at: "2026-11-12T00:00:00.000Z".into(),
    }
}

fn register(data_dir: &Path, id: &str, checkout: &Path) {
    let mut store = Store::open(data_dir.join("luuna.sqlite3")).unwrap();
    store
        .register_project(id, "Project", &[("api".into(), observation(checkout))])
        .unwrap();
}

fn managed_store(data_dir: &Path, id: &str) -> PathBuf {
    data_dir.join("components").join("projects").join(id)
}

fn is_empty(directory: &Path) -> bool {
    fs::read_dir(directory).unwrap().next().is_none()
}

#[tokio::test]
async fn a_runtime_project_store_resolves_under_the_data_directory() {
    let data = tempfile::tempdir().unwrap();
    let checkout = tempfile::tempdir().unwrap();
    register(data.path(), "app", checkout.path());
    write_config(
        &roots_paths(managed_store(data.path(), "app")),
        &default_config("app"),
    )
    .unwrap();
    let state = open(data.path());

    let store = RootsStore::for_project(&state, "app").unwrap();
    assert_eq!(
        store.store_dir(),
        managed_store(data.path(), "app")
            .join(".luuna")
            .join("roots")
    );
    assert_eq!(store.project_id(), "app");
    assert_eq!(store.config().await.unwrap().project, "app");
    assert!(store.graph().await.unwrap().nodes.is_empty());
    assert!(is_empty(checkout.path()));
}

#[test]
fn an_unregistered_or_invalid_project_is_not_found() {
    let data = tempfile::tempdir().unwrap();
    let state = open(data.path());
    for id in ["nope", "../x", "", "A"] {
        let error = RootsStore::for_project(&state, id).unwrap_err();
        assert_eq!(error.0, StatusCode::NOT_FOUND, "{id:?}");
    }
    assert!(!data.path().join("components").exists());
}

#[test]
fn a_repository_store_mode_is_refused_before_anything_is_read_or_written() {
    let data = tempfile::tempdir().unwrap();
    let checkout = tempfile::tempdir().unwrap();
    register(data.path(), "app", checkout.path());
    write_project_store_mode(
        data.path(),
        "app",
        &StoreMode::Repository {
            repository: "api".into(),
        },
    )
    .unwrap();
    // A Roots store in the checkout must never be used instead.
    let checkout_roots = roots_paths(checkout.path());
    write_config(&checkout_roots, &default_config("checkout")).unwrap();
    let file = data.path().join("projects").join("app").join("store.json");
    let before_mode = fs::read(&file).unwrap();
    let before_config = fs::read(&checkout_roots.config).unwrap();
    let state = open(data.path());

    let error = RootsStore::for_project(&state, "app").unwrap_err();
    assert_eq!(error.0, StatusCode::CONFLICT);
    assert!(!error
        .1
        .contains(&data.path().to_string_lossy().into_owned()));
    assert!(!data.path().join("components").exists());
    assert_eq!(fs::read(&file).unwrap(), before_mode);
    assert_eq!(fs::read(&checkout_roots.config).unwrap(), before_config);
    let entries: Vec<_> = fs::read_dir(&checkout_roots.dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(entries, vec![std::ffi::OsString::from("config.yaml")]);
}

#[test]
fn a_store_without_a_roots_config_is_not_set_up_and_nothing_is_created() {
    let data = tempfile::tempdir().unwrap();
    let checkout = tempfile::tempdir().unwrap();
    register(data.path(), "app", checkout.path());
    let state = open(data.path());

    let error = RootsStore::for_project(&state, "app").unwrap_err();
    assert_eq!(error.0, StatusCode::CONFLICT);
    assert_eq!(error.1, "Roots is not set up for this project");
    assert!(!managed_store(data.path(), "app").exists());

    // An existing managed workspace without Roots stays as it was.
    fs::create_dir_all(managed_store(data.path(), "app").join(".luuna")).unwrap();
    let error = RootsStore::for_project(&state, "app").unwrap_err();
    assert_eq!(error.0, StatusCode::CONFLICT);
    assert!(is_empty(&managed_store(data.path(), "app").join(".luuna")));
    assert!(is_empty(checkout.path()));
}

/// roots-core needs serde_json's preserve_order, so the runtime build that
/// runs the Missions source-hash pins has it on: object keys keep insertion
/// order.
#[test]
fn preserve_order_is_on_in_the_runtime_build() {
    assert_eq!(
        serde_json::json!({"b":1,"a":2}).to_string(),
        r#"{"b":1,"a":2}"#
    );
}
