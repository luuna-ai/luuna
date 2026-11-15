//! Authenticated runtime prompt evidence with no legacy component handle.
use super::*;
use crate::integrations::{self, SelectContext};
use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
};
use luuna_beacon::{Beacon, CreatePrompt, CreateSchema, Rule, Section, UpdatePrompt};
use luuna_domain::{DirtyState, PromptReference, RepositoryIdentity, RepositoryObservation};
use tower::ServiceExt;
const TOKEN: &str = "synthetic-native-beacon-runtime-token-32-bytes";
fn fixture(root: &std::path::Path) -> AppState {
    let state = AppState::open(RuntimeConfig {
        data_dir: root.join("private"),
        pi_program: "node".into(),
        git_program: "git".into(),
        pi_launcher_args: Vec::new(),
        provider: None,
        model: None,
        token: TOKEN.into(),
        max_run_seconds: 10,
    })
    .unwrap()
    .without_components();
    for id in ["app", "other"] {
        let observation = RepositoryObservation {
            local_path: root.join(id).to_string_lossy().into_owned(),
            identity: RepositoryIdentity {
                remote: Some("https://example.invalid/native-fixture.git".into()),
                push_remote: None,
                root_commits: vec!["1".repeat(40)],
                shallow: false,
            },
            head_commit: "1".repeat(40),
            branch: Some("main".into()),
            remote_default_branch: None,
            local_branches: vec!["main".into()],
            dirty: DirtyState::default(),
            git_version: "git version 2.43.0".into(),
            observed_at: "2026-01-01T00:00:00Z".into(),
        };
        state
            .store
            .lock()
            .unwrap()
            .register_project(id, id, &[("app".into(), observation)])
            .unwrap();
    }
    state
}
async fn enable_existing_store(state: &AppState, id: &str) {
    // An already activated persisted store is usable independently of the
    // remaining Missions/Atlas toolchain. Fresh full activation remains explicit.
    state
        .store
        .lock()
        .unwrap()
        .begin_component_enable(id)
        .unwrap();
    let root = integrations::workspace(state, id).unwrap();
    luuna_components::call_beacon_native(&root, &["init".into()])
        .await
        .unwrap();
    state
        .store
        .lock()
        .unwrap()
        .finish_component_enable(id)
        .unwrap();
}
async fn http(
    state: &AppState,
    method: &str,
    path: &str,
    body: Value,
    auth: bool,
) -> (StatusCode, Value) {
    let mut req = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json");
    if auth {
        req = req.header("authorization", format!("Bearer {TOKEN}"))
    }
    let result = router(state.clone())
        .oneshot(req.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let status = result.status();
    let bytes = to_bytes(result.into_body(), 256 * 1024).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}
fn prompt_request(key: &str, body: &str, version: Option<u64>) -> Value {
    json!({"idempotency_key":key,"name":"guidance","body":body,"expected_version":version})
}
async fn create(state: &AppState) -> Value {
    let (status, value) = http(
        state,
        "POST",
        "/api/v1/projects/app/prompts",
        prompt_request("create", "Required guidance", None),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{value}");
    value
}

#[tokio::test]
async fn native_prompt_http_auth_idempotency_versions_and_project_scope() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let state = fixture(&root);
    assert!(state.components.is_none());
    enable_existing_store(&state, "app").await;
    enable_existing_store(&state, "other").await;
    assert_eq!(
        http(
            &state,
            "GET",
            "/api/v1/projects/app/prompts",
            Value::Null,
            false
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    let first = create(&state).await;
    assert_eq!(first["version"], 1);
    let replay = create(&state).await;
    assert_eq!(first, replay);
    let (status, list) = http(
        &state,
        "GET",
        "/api/v1/projects/app/prompts",
        Value::Null,
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list, json!([{"name":"guidance","version":1}]));
    let (status, next) = http(
        &state,
        "POST",
        "/api/v1/projects/app/prompts",
        prompt_request("update", "Updated guidance", Some(1)),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(next["version"], 2);
    assert_eq!(
        http(
            &state,
            "POST",
            "/api/v1/projects/app/prompts",
            prompt_request("stale", "Stale guidance", Some(1)),
            true
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        http(
            &state,
            "GET",
            "/api/v1/projects/other/prompts/guidance/render",
            Value::Null,
            true
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        http(
            &state,
            "POST",
            "/api/v1/projects/app/prompts",
            prompt_request("create", "Conflicting replay", None),
            true
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    let selected = SelectContext {
        memory_ids: vec!["absent".into()],
        prompt: None,
    };
    assert_eq!(
        integrations::resolve_selection(&state, "app", Some(&selected))
            .await
            .unwrap_err()
            .0,
        StatusCode::SERVICE_UNAVAILABLE
    );
}

#[tokio::test]
async fn native_schema_and_parent_source_changes_stale_approved_prompt_context() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let state = fixture(&root);
    enable_existing_store(&state, "app").await;
    create(&state).await;
    let b = Beacon::open(&integrations::workspace(&state, "app").unwrap()).unwrap();
    b.schema_create(&CreateSchema {
        name: "approval".into(),
        required: vec!["guidance".into()],
        optional: Vec::new(),
    })
    .unwrap();
    b.update(
        "guidance",
        &UpdatePrompt {
            schema: Some("approval".into()),
            ..Default::default()
        },
    )
    .unwrap();
    let (status, before) = http(
        &state,
        "GET",
        "/api/v1/projects/app/prompts/guidance/render",
        Value::Null,
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let selection = SelectContext {
        memory_ids: Vec::new(),
        prompt: Some(PromptReference {
            name: "guidance".into(),
            version: before["version"].as_u64().unwrap(),
            hash: before["hash"].as_str().unwrap().into(),
        }),
    };
    let context = integrations::resolve_selection(&state, "app", Some(&selection))
        .await
        .unwrap()
        .unwrap();
    assert!(
        integrations::resolve_approved(&state, "app", Some(&context))
            .await
            .is_ok()
    );
    b.schema_rule_add(
        "approval",
        &Rule {
            section: "guidance".into(),
            pattern: "Required".into(),
            message: "Expected text".into(),
        },
    )
    .unwrap();
    let (status, after) = http(
        &state,
        "GET",
        "/api/v1/projects/app/prompts/guidance/render",
        Value::Null,
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(before["body"], after["body"]);
    assert_ne!(before["hash"], after["hash"]);
    assert_eq!(
        integrations::resolve_approved(&state, "app", Some(&context))
            .await
            .unwrap_err()
            .0,
        StatusCode::CONFLICT
    );
    b.create(&CreatePrompt {
        name: "base".into(),
        sections: vec![Section {
            name: "extra".into(),
            body: "Base guidance".into(),
            required: None,
        }],
        ..Default::default()
    })
    .unwrap();
    b.update(
        "guidance",
        &UpdatePrompt {
            extends: Some("base".into()),
            ..Default::default()
        },
    )
    .unwrap();
    let (_, before) = http(
        &state,
        "GET",
        "/api/v1/projects/app/prompts/guidance/render",
        Value::Null,
        true,
    )
    .await;
    let selected = SelectContext {
        memory_ids: Vec::new(),
        prompt: Some(PromptReference {
            name: "guidance".into(),
            version: before["version"].as_u64().unwrap(),
            hash: before["hash"].as_str().unwrap().into(),
        }),
    };
    let context = integrations::resolve_selection(&state, "app", Some(&selected))
        .await
        .unwrap()
        .unwrap();
    b.update(
        "base",
        &UpdatePrompt {
            description: Some("Changed source metadata".into()),
            ..Default::default()
        },
    )
    .unwrap();
    let (_, after) = http(
        &state,
        "GET",
        "/api/v1/projects/app/prompts/guidance/render",
        Value::Null,
        true,
    )
    .await;
    assert_eq!(before["body"], after["body"]);
    assert_ne!(before["hash"], after["hash"]);
    assert_eq!(
        integrations::resolve_approved(&state, "app", Some(&context))
            .await
            .unwrap_err()
            .0,
        StatusCode::CONFLICT
    );
    b.schema_rule_add(
        "approval",
        &Rule {
            section: "guidance".into(),
            pattern: "IMPOSSIBLE".into(),
            message: "Synthetic failing rule".into(),
        },
    )
    .unwrap();
    assert_eq!(
        http(
            &state,
            "GET",
            "/api/v1/projects/app/prompts/guidance/render",
            Value::Null,
            true
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
}

#[tokio::test]
async fn native_store_failures_do_not_expose_source_bytes_or_host_paths() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let state = fixture(&root);
    enable_existing_store(&state, "app").await;
    let path = integrations::workspace(&state, "app")
        .unwrap()
        .join(".luuna/beacon/prompts.jsonl");
    let source = b"synthetic-malformed-source-marker\n";
    std::fs::write(&path, source).unwrap();
    let (status, error) = http(
        &state,
        "GET",
        "/api/v1/projects/app/prompts",
        Value::Null,
        true,
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    let text = error.to_string();
    assert!(!text.contains("synthetic-malformed-source-marker"));
    assert!(!text.contains(&*root.to_string_lossy()));
    assert_eq!(std::fs::read(path).unwrap(), source)
}

#[tokio::test]
async fn native_prompts_do_not_implicitly_enable_a_project() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let state = fixture(&root);
    let (status, _) = http(
        &state,
        "POST",
        "/api/v1/projects/app/prompts",
        prompt_request("blocked", "Blocked guidance", None),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(!integrations::workspace(&state, "app")
        .unwrap()
        .join(".luuna/beacon")
        .exists())
}
