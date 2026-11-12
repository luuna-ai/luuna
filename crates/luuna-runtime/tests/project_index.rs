//! A runtime started on a data folder whose database was deleted serves the
//! same projects: the project files rebuild the registrations, the confirmed
//! manifests and the retention setting. No test needs git, a component
//! toolchain, the network or a model.
use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
    Router,
};
use luuna_domain::{
    DirtyState, ExecutionProfile, ManifestCheck, ProjectManifest, ProjectPolicy,
    RepositoryIdentity, RepositoryManifest, RepositoryObservation,
};
use luuna_runtime::{router, AppState, RuntimeConfig};
use luuna_storage::{MemoryRetentionPolicy, Store};
use serde_json::Value;
use std::{collections::BTreeMap, fs, path::Path, path::PathBuf};
use tower::ServiceExt;

const TOKEN: &str = "synthetic-test-token-with-at-least-32-bytes";
const REMOTE: &str = "https://example.invalid/api.git";

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
async fn get(app: &Router, path: &str) -> (StatusCode, Value) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
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
fn manifest(project: &str) -> ProjectManifest {
    ProjectManifest {
        schema_version: "0.1".into(),
        id: project.into(),
        name: format!("Project {project}"),
        revision: 1,
        repositories: vec![RepositoryManifest {
            id: "api".into(),
            remote: REMOTE.into(),
            default_branch: "main".into(),
            profiles: vec!["rust".into()],
            execution: ExecutionProfile::Native,
            checks: vec![ManifestCheck {
                id: "cargo-test".into(),
                executable: "cargo".into(),
                args: vec!["test".into(), "--locked".into()],
                required: true,
                timeout_seconds: 600,
                capabilities: vec!["rust".into()],
                cwd: ".".into(),
            }],
        }],
        relationships: vec![],
        policy: ProjectPolicy::v1(1, 1, 2),
    }
}

#[tokio::test]
async fn a_runtime_started_on_a_deleted_database_serves_the_same_projects() {
    let data = tempfile::tempdir().unwrap();
    let checkouts = tempfile::tempdir().unwrap();
    let confirmed_checkout = checkouts.path().join("confirmed");
    let plain_checkout = checkouts.path().join("plain");
    for checkout in [&confirmed_checkout, &plain_checkout] {
        fs::create_dir_all(checkout).unwrap();
    }
    {
        let mut store = Store::open(data.path().join("luuna.sqlite3")).unwrap();
        store
            .register_project(
                "confirmed",
                "Confirmed",
                &[("api".into(), observation(&confirmed_checkout))],
            )
            .unwrap();
        store
            .confirm_manifest(
                "confirmed",
                0,
                &manifest("confirmed"),
                &BTreeMap::from([("api".to_owned(), observation(&confirmed_checkout))]),
            )
            .unwrap();
        store
            .set_memory_retention(
                "confirmed",
                &MemoryRetentionPolicy {
                    process_max_age_days: Some(30),
                    project_max_age_days: Some(90),
                },
            )
            .unwrap();
        store
            .register_project(
                "plain",
                "Plain",
                &[("api".into(), observation(&plain_checkout))],
            )
            .unwrap();
    }

    let paths = [
        "/api/v1/projects".to_owned(),
        "/api/v1/projects/confirmed".to_owned(),
        "/api/v1/projects/confirmed/memory/retention".to_owned(),
    ];
    let mut before = Vec::new();
    {
        let app = serve(data.path());
        for path in &paths {
            let (status, body) = get(&app, path).await;
            assert_eq!(status, StatusCode::OK, "{path}");
            before.push(body);
        }
    }
    assert_eq!(
        before[2],
        serde_json::json!({"process_max_age_days": 30, "project_max_age_days": 90})
    );

    for suffix in ["", "-wal", "-shm"] {
        let _ = fs::remove_file(data.path().join(format!("luuna.sqlite3{suffix}")));
    }
    assert!(!data.path().join("luuna.sqlite3").exists());

    let app = serve(data.path());
    for (path, expected) in paths.iter().zip(&before) {
        let (status, body) = get(&app, path).await;
        assert_eq!(status, StatusCode::OK, "{path} after the rebuild");
        assert_eq!(&body, expected, "{path} after the rebuild");
    }
}
