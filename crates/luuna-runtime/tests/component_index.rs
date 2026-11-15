//! Synthetic runtime with explicit temporary data/checkout paths. Native
//! component libraries only: no git, external toolchain, model or user store.
use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
    Router,
};
use luuna_domain::{DirtyState, RepositoryIdentity, RepositoryObservation};
use luuna_runtime::{router, AppState, RuntimeConfig};
use luuna_storage::Store;
use serde_json::{json, Value};
use std::{fs, path::Path};
use tower::ServiceExt;
const TOKEN: &str = "synthetic-component-index-token-32-bytes";
fn open(data: &Path) -> Router {
    router(
        AppState::open(RuntimeConfig {
            data_dir: data.into(),
            pi_program: "unused-python".into(),
            pi_launcher_args: vec![],
            git_program: "unused-git".into(),
            provider: None,
            model: None,
            token: TOKEN.into(),
            max_run_seconds: 10,
        })
        .unwrap(),
    )
}
async fn request(
    app: &Router,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut req = Request::builder()
        .method(method)
        .uri(format!("/api/v1/projects/app/{path}"))
        .header("Authorization", format!("Bearer {TOKEN}"));
    let body = if let Some(body) = body {
        req = req.header("content-type", "application/json");
        Body::from(body.to_string())
    } else {
        Body::empty()
    };
    let response = app.clone().oneshot(req.body(body).unwrap()).await.unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 8 * 1024 * 1024)
        .await
        .unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}
#[tokio::test]
async fn lost_database_keeps_native_memory_identity_and_explicit_user_inclusion() {
    let data = tempfile::tempdir().unwrap();
    let checkout = tempfile::tempdir().unwrap();
    let mut store = Store::open(data.path().join("luuna.sqlite3")).unwrap();
    store
        .register_project(
            "app",
            "App",
            &[(
                "api".into(),
                RepositoryObservation {
                    local_path: checkout.path().to_string_lossy().into_owned(),
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
                    observed_at: "2026-11-15T00:00:00Z".into(),
                },
            )],
        )
        .unwrap();
    drop(store);
    let app = open(data.path());
    let (status, body) = request(&app, "POST", "components", None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["enabled"], true);
    for (key, scope, content) in [
        ("project-one", "project", "Project fixture knowledge"),
        ("user-one", "user", "PRIVATE USER FIXTURE CONTENT"),
    ] {
        let (status,body)=request(&app,"POST","memory",Some(json!({"idempotency_key":key,"scope":scope,"content":content,"provenance":"Explicit synthetic fixture"}))).await;
        assert_eq!(status, StatusCode::OK, "{body}");
    }
    let (status, visible) = request(&app, "GET", "memory", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(visible.as_array().unwrap().len(), 1);
    assert_eq!(visible[0]["scope"], "project");
    let (status, all) = request(&app, "GET", "memory?include_user=true", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(all.as_array().unwrap().len(), 2);
    assert!(all.as_array().unwrap().iter().any(|r| r["scope"] == "user"));
    let metadata =
        fs::read_to_string(data.path().join("project-records/app/components.json")).unwrap();
    assert!(!metadata.contains("PRIVATE USER FIXTURE CONTENT"));
    assert!(!metadata.contains("Project fixture knowledge"));
    assert!(!metadata.contains("project-one"));
    assert!(!metadata.contains("user-one"));
    drop(app);
    fs::remove_file(data.path().join("luuna.sqlite3")).unwrap();
    let app = open(data.path());
    let (status, body) = request(&app, "GET", "components", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["enabled"], true);
    let (status, recovered) = request(&app, "GET", "memory", None).await;
    assert_eq!(status, StatusCode::OK, "{recovered}");
    assert_eq!(recovered, visible);
    let (status, recovered) = request(&app, "GET", "memory?include_user=true", None).await;
    assert_eq!(status, StatusCode::OK, "{recovered}");
    assert_eq!(recovered, all);
    assert_eq!(fs::read_dir(checkout.path()).unwrap().count(), 0);
}
