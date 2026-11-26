//! `GET /api/v1/doctor` (ADR 0027 point 4): the running runtime's own doctor
//! report behind the token, with credential presence by name and never a
//! value. Nothing here calls a model; the Pi probe runs `python3 --version`
//! in a private scratch directory and reports the version mismatch.
use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
};
use luuna_runtime::{router, AppState, RuntimeConfig};
use serde_json::Value;
use std::{fs, path::PathBuf};
use tower::ServiceExt;

const TOKEN: &str = "synthetic-doctor-test-token-with-at-least-32-bytes";
const PROVIDER_VALUE: &str = "synthetic-openrouter-secret-value-never-shown";

#[tokio::test]
async fn the_doctor_route_needs_the_token_and_reports_presence_without_values() {
    // The provider presence is read from this process's environment; this
    // test binary runs no other test, so the variables affect nothing else.
    std::env::set_var("OPENROUTER_API_KEY", PROVIDER_VALUE);
    std::env::set_var("ANTHROPIC_API_KEY", "");
    let root: PathBuf =
        std::env::temp_dir().join(format!("luuna-doctor-test-{}", uuid::Uuid::new_v4()));
    let state = AppState::open(RuntimeConfig {
        data_dir: root.clone(),
        pi_program: if cfg!(windows) { "python" } else { "python3" }.into(),
        pi_launcher_args: vec![],
        git_program: "git".into(),
        provider: None,
        model: None,
        token: TOKEN.into(),
        max_run_seconds: 10,
    })
    .unwrap()
    .with_listen_port(43121);
    let app = router(state.clone());

    let anonymous = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/doctor")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/doctor")
                .header("Authorization", format!("Bearer {TOKEN}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    let report: Value = serde_json::from_slice(&bytes).unwrap();
    let items = report["items"].as_array().unwrap();
    let names: Vec<&str> = items
        .iter()
        .map(|item| item["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "git",
            "python",
            "node",
            "pi",
            "data_dir",
            "database",
            "api_token",
            "github_token",
            "provider_credentials",
            "port",
            "components"
        ]
    );
    let missing = items.iter().any(|item| item["status"] == "missing");
    assert_eq!(report["ok"], Value::Bool(!missing));
    let by_name = |name: &str| items.iter().find(|item| item["name"] == name).unwrap();
    for item in items {
        let detail = item["detail"].as_str().unwrap();
        assert!(!detail.contains(TOKEN), "{detail}");
        assert!(!detail.contains(PROVIDER_VALUE), "{detail}");
        assert!(["ok", "warning", "missing"].contains(&item["status"].as_str().unwrap()));
        assert!(item["required"].is_boolean());
    }
    assert_eq!(by_name("git")["status"], "ok");
    assert_eq!(by_name("git")["required"], true);
    assert_eq!(by_name("api_token")["status"], "ok");
    assert_eq!(
        by_name("api_token")["detail"],
        "LUUNA_API_TOKEN is configured (value not shown)"
    );
    assert_eq!(by_name("github_token")["status"], "warning");
    assert_eq!(
        by_name("data_dir")["detail"].as_str().unwrap(),
        format!("{} exists and is writable", root.display())
    );
    assert_eq!(by_name("database")["status"], "ok");
    let providers = by_name("provider_credentials")["detail"].as_str().unwrap();
    assert!(
        providers.starts_with("configured: OPENROUTER_API_KEY (values not shown)"),
        "{providers}"
    );
    assert!(
        providers.contains("set but empty: ANTHROPIC_API_KEY"),
        "{providers}"
    );
    assert_eq!(
        by_name("port")["detail"],
        "this runtime is the one listening on http://127.0.0.1:43121"
    );
    // The Pi probe ran the configured program, not a model: a version
    // mismatch is a missing required item, reported without any credential.
    assert_eq!(by_name("pi")["required"], true);
    assert!(!bytes
        .windows(PROVIDER_VALUE.len())
        .any(|w| w == PROVIDER_VALUE.as_bytes()));
    state.cancel_active();
    drop(state);
    let _ = fs::remove_dir_all(&root);
}
