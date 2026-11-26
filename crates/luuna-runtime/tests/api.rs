//! Synthetic transport integration; these tests make no provider call.
use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
    Router,
};
use luuna_domain::RunState;
use luuna_runtime::{router, AppState, RuntimeConfig};
use luuna_storage::RunRecord;
use serde_json::{json, Value};
use std::{path::PathBuf, time::Duration};
use tower::ServiceExt;

const TOKEN: &str = "synthetic-test-token-with-at-least-32-bytes";

struct Harness {
    app: Router,
    state: AppState,
    root: PathBuf,
}
impl Harness {
    fn new() -> Self {
        Self::with(|_| {})
    }
    fn with(adjust: impl FnOnce(&mut RuntimeConfig)) -> Self {
        let root =
            std::env::temp_dir().join(format!("luuna-runtime-test-{}", uuid::Uuid::new_v4()));
        let mut config = configuration(root.clone());
        adjust(&mut config);
        let state = AppState::open(config).unwrap();
        Self {
            app: router(state.clone()),
            state,
            root,
        }
    }
    async fn request(
        &self,
        method: &str,
        path: &str,
        body: Option<Value>,
        auth: bool,
    ) -> (StatusCode, Value) {
        let mut request = Request::builder().method(method).uri(path);
        if auth {
            request = request.header("Authorization", format!("Bearer {TOKEN}"));
        }
        if body.is_some() {
            request = request.header("Content-Type", "application/json");
        }
        let response = self
            .app
            .clone()
            .oneshot(
                request
                    .body(Body::from(body.map(|b| b.to_string()).unwrap_or_default()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 2 * 1024 * 1024)
            .await
            .unwrap();
        (status, serde_json::from_slice(&bytes).unwrap())
    }
    async fn start(&self, scenario: &str) -> RunRecord {
        let (status, body) = self
            .request(
                "POST",
                "/api/v1/runs",
                Some(json!({"prompt":scenario,"provider":"test","model":"synthetic"})),
                true,
            )
            .await;
        assert_eq!(status, StatusCode::ACCEPTED);
        serde_json::from_value(body).unwrap()
    }
    async fn terminal(&self, id: &str) -> RunRecord {
        for _ in 0..100 {
            let (_, value) = self
                .request("GET", &format!("/api/v1/runs/{id}"), None, true)
                .await;
            let record: RunRecord = serde_json::from_value(value).unwrap();
            if record.state.is_terminal() {
                return record;
            }
            tokio::time::sleep(Duration::from_millis(40)).await;
        }
        panic!("run did not terminate");
    }
}
fn configuration(root: PathBuf) -> RuntimeConfig {
    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/pi-rpc/mock_pi.py")
        .canonicalize()
        .unwrap();
    RuntimeConfig {
        data_dir: root,
        pi_program: if cfg!(windows) { "python" } else { "python3" }.into(),
        pi_launcher_args: vec![script.to_string_lossy().into_owned()],
        git_program: "git".into(),
        provider: None,
        model: None,
        token: TOKEN.into(),
        max_run_seconds: 10,
    }
}
impl Drop for Harness {
    fn drop(&mut self) {
        self.state.cancel_active();
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[tokio::test]
async fn health_public_run_data_authenticated_and_input_constrained() {
    let harness = Harness::new();
    assert_eq!(
        harness.request("GET", "/health", None, false).await.0,
        StatusCode::OK
    );
    assert_eq!(
        harness.request("GET", "/api/v1/runs", None, false).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        harness
            .request(
                "POST",
                "/api/v1/runs",
                Some(json!({"prompt":"/bash secret"})),
                true
            )
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        harness
            .request(
                "POST",
                "/api/v1/runs",
                Some(json!({"prompt":"hello"})),
                true
            )
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        harness
            .request("GET", "/api/v1/runs/missing/events", None, true)
            .await
            .0,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn shutdown_requires_authentication_is_idempotent_and_closes_admission() {
    let harness = Harness::new();
    assert_eq!(
        harness
            .request("POST", "/api/v1/shutdown", None, false)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    assert!(tokio::time::timeout(
        Duration::from_millis(25),
        harness.state.wait_for_shutdown_request()
    )
    .await
    .is_err());
    let run = harness.start("slow").await;
    for _ in 0..100 {
        let (_, events) = harness
            .request(
                "GET",
                &format!("/api/v1/runs/{}/events", run.id),
                None,
                true,
            )
            .await;
        if events
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["type"] == "pi.agent_start")
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    for _ in 0..2 {
        assert_eq!(
            harness
                .request("POST", "/api/v1/shutdown", None, true)
                .await
                .0,
            StatusCode::ACCEPTED
        );
    }
    tokio::time::timeout(
        Duration::from_secs(1),
        harness.state.wait_for_shutdown_request(),
    )
    .await
    .unwrap();
    assert_eq!(
        harness
            .request(
                "POST",
                "/api/v1/runs",
                Some(json!({"prompt":"success"})),
                true
            )
            .await
            .0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    harness.state.shutdown().await;
    assert_eq!(harness.terminal(&run.id).await.state, RunState::Interrupted);
    let (_, events) = harness
        .request(
            "GET",
            &format!("/api/v1/runs/{}/events", run.id),
            None,
            true,
        )
        .await;
    assert!(events
        .as_array()
        .unwrap()
        .iter()
        .any(|e| e["type"] == "run.interrupted"
            && e["payload"]["reason"]
                .as_str()
                .is_some_and(|s| s.contains("interrupted by runtime shutdown"))));
}

#[tokio::test]
async fn persistence_replay_unknown_events_and_successful_completion() {
    let harness = Harness::new();
    let run = harness.start("success").await;
    let terminal = harness.terminal(&run.id).await;
    assert_eq!(terminal.state, RunState::Completed);
    let (_, events) = harness
        .request(
            "GET",
            &format!("/api/v1/runs/{}/events", run.id),
            None,
            true,
        )
        .await;
    let events = events.as_array().unwrap();
    assert!(events.iter().any(|e| e["type"] == "pi.future_event"));
    let settled = events
        .iter()
        .position(|e| e["type"] == "pi.agent_settled")
        .unwrap();
    let completed = events
        .iter()
        .position(|e| e["type"] == "run.completed")
        .unwrap();
    assert!(settled < completed);
    let after = events[settled]["sequence"].as_u64().unwrap();
    let (_, replay) = harness
        .request(
            "GET",
            &format!("/api/v1/runs/{}/events?after={after}", run.id),
            None,
            true,
        )
        .await;
    assert!(replay
        .as_array()
        .unwrap()
        .iter()
        .all(|e| e["sequence"].as_u64().unwrap() > after));
    assert_eq!(
        luuna_storage::Store::open(harness.root.join("luuna.sqlite3"))
            .unwrap()
            .get_run(&run.id)
            .unwrap()
            .unwrap()
            .state,
        RunState::Completed
    );
}

#[tokio::test]
async fn failures_do_not_report_success() {
    let harness = Harness::new();
    for scenario in [
        "provider_error",
        "malformed",
        "wrong_id",
        "rejected",
        "crash",
    ] {
        let run = harness.start(scenario).await;
        assert_eq!(
            harness.terminal(&run.id).await.state,
            RunState::Failed,
            "{scenario}"
        );
    }
}

#[tokio::test]
async fn retry_can_recover_before_agent_settled() {
    let harness = Harness::new();
    let run = harness.start("retry_success").await;
    assert_eq!(harness.terminal(&run.id).await.state, RunState::Completed);
}

#[tokio::test]
async fn malformed_known_completion_fails_closed_after_prompt_acceptance() {
    let harness = Harness::new();
    for scenario in [
        "malformed_settled",
        "invalid_settled_aborted",
        "malformed_agent_end",
        "malformed_message_end",
    ] {
        let run = harness.start(scenario).await;
        assert_eq!(
            harness.terminal(&run.id).await.state,
            RunState::Failed,
            "{scenario}"
        );
        let (_, events) = harness
            .request(
                "GET",
                &format!("/api/v1/runs/{}/events", run.id),
                None,
                true,
            )
            .await;
        let events = events.as_array().unwrap();
        assert!(
            events.iter().any(|event| event["type"] == "pi.response"
                && event["payload"]["id"] == "prompt-1"
                && event["payload"]["success"] == true),
            "prompt acceptance must precede the malformed completion: {scenario}"
        );
        assert!(
            events.iter().any(|event| event["type"] == "run.failed"
                && event["payload"]["reason"]
                    .as_str()
                    .unwrap()
                    .contains("invalid Pi RPC record")),
            "{scenario}"
        );
        assert!(
            !events.iter().any(|event| event["type"] == "run.completed"),
            "{scenario}"
        );
    }
}

#[tokio::test]
async fn low_level_agent_end_does_not_finish_run_and_cancel_reaps() {
    let harness = Harness::new();
    let run = harness.start("end_without_settled").await;
    let mut saw_low_end = false;
    for _ in 0..100 {
        let (_, events) = harness
            .request(
                "GET",
                &format!("/api/v1/runs/{}/events", run.id),
                None,
                true,
            )
            .await;
        if events
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["type"] == "pi.agent_end")
        {
            saw_low_end = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(saw_low_end);
    let (_, record) = harness
        .request("GET", &format!("/api/v1/runs/{}", run.id), None, true)
        .await;
    assert_eq!(record["state"], "running");
    assert_eq!(
        harness
            .request(
                "POST",
                &format!("/api/v1/runs/{}/cancel", run.id),
                None,
                true
            )
            .await
            .0,
        StatusCode::ACCEPTED
    );
    assert_eq!(harness.terminal(&run.id).await.state, RunState::Cancelled);
}

#[tokio::test]
async fn sse_replays_with_last_event_id_and_closes_after_terminal() {
    let harness = Harness::new();
    let run = harness.start("success").await;
    harness.terminal(&run.id).await;
    let request = Request::builder()
        .uri(format!("/api/v1/runs/{}/stream", run.id))
        .header("Authorization", format!("Bearer {TOKEN}"))
        .header("Last-Event-ID", "1")
        .body(Body::empty())
        .unwrap();
    let response = harness.app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = String::from_utf8(
        to_bytes(response.into_body(), 2 * 1024 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(body.contains("event: run_event"));
    assert!(body.contains("run.completed"));
    assert!(!body.contains("run.created"));
}

#[test]
fn data_directory_ownership_precedes_recovery() {
    let harness = Harness::new();
    let mut storage = luuna_storage::Store::open(harness.root.join("luuna.sqlite3")).unwrap();
    let run = storage
        .create_run("still-owned", "demo", json!({}))
        .unwrap();
    storage
        .transition_run(
            &run.id,
            run.revision,
            RunState::Running,
            "run.running",
            json!({}),
        )
        .unwrap();
    assert!(AppState::open(configuration(harness.root.clone())).is_err());
    assert_eq!(
        storage.get_run(&run.id).unwrap().unwrap().state,
        RunState::Running
    );
}

#[tokio::test]
async fn idle_event_stream_closes_when_service_shutdown_is_requested() {
    let harness = Harness::new();
    let mut store = luuna_storage::Store::open(harness.root.join("luuna.sqlite3")).unwrap();
    store.create_run("idle", "demo", json!({})).unwrap();
    drop(store);
    let request = Request::builder()
        .uri("/api/v1/runs/idle/stream")
        .header("Authorization", format!("Bearer {TOKEN}"))
        .body(Body::empty())
        .unwrap();
    let response = harness.app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        harness
            .request("POST", "/api/v1/shutdown", None, true)
            .await
            .0,
        StatusCode::ACCEPTED
    );
    let bytes = tokio::time::timeout(
        Duration::from_secs(1),
        to_bytes(response.into_body(), 1024 * 1024),
    )
    .await
    .expect("idle SSE must not keep graceful HTTP shutdown open")
    .unwrap();
    assert!(String::from_utf8_lossy(&bytes).contains("run.created"));
    assert_eq!(
        harness
            .request("GET", "/api/v1/runs/idle", None, true)
            .await
            .0,
        StatusCode::OK
    );
}

#[tokio::test]
async fn provider_key_aliases_reach_the_child_by_standard_name_only() {
    // Synthetic names/values; the fixture reports variable names, never values.
    std::env::set_var("LUUNA_PI_ANTHROPIC_API_KEY", "synthetic-alias-secret-value");
    std::env::set_var("GROQ_API_KEY", "synthetic-direct-secret-value");
    let harness = Harness::new();
    let run = harness.start("env_names").await;
    assert_eq!(harness.terminal(&run.id).await.state, RunState::Completed);
    let (_, events) = harness
        .request(
            "GET",
            &format!("/api/v1/runs/{}/events", run.id),
            None,
            true,
        )
        .await;
    let serialized = events.to_string();
    assert!(
        !serialized.contains("secret-value"),
        "values must never persist"
    );
    let events = events.as_array().unwrap();
    let names: Vec<&str> = events
        .iter()
        .find(|e| e["type"] == "pi.fixture_env")
        .and_then(|e| e["payload"]["names"].as_array())
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert!(names.contains(&"ANTHROPIC_API_KEY"), "{names:?}");
    assert!(names.contains(&"GROQ_API_KEY"), "{names:?}");
    assert!(
        !names.iter().any(|n| n.starts_with("LUUNA_PI_")),
        "{names:?}"
    );
    let spawned = events
        .iter()
        .find(|e| e["type"] == "pi.spawned")
        .expect("spawn event");
    let configured: Vec<&str> = spawned["payload"]["provider_credentials"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert!(configured.contains(&"ANTHROPIC_API_KEY") && configured.contains(&"GROQ_API_KEY"));
    assert!(spawned["payload"]["pid"].as_u64().is_some());
}

#[cfg(unix)]
#[tokio::test]
async fn cancelled_run_records_pid_and_child_is_reaped_before_terminal_event() {
    let harness = Harness::new();
    let run = harness.start("slow").await;
    let mut pid = None;
    for _ in 0..100 {
        let (_, events) = harness
            .request(
                "GET",
                &format!("/api/v1/runs/{}/events", run.id),
                None,
                true,
            )
            .await;
        if let Some(found) = events
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["type"] == "pi.spawned")
            .and_then(|e| e["payload"]["pid"].as_u64())
        {
            pid = Some(found);
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let pid = pid.expect("pi.spawned must record the child PID");
    let alive = |pid: u64| {
        std::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    };
    assert!(alive(pid), "fixture child should be running before cancel");
    assert_eq!(
        harness
            .request(
                "POST",
                &format!("/api/v1/runs/{}/cancel", run.id),
                None,
                true
            )
            .await
            .0,
        StatusCode::ACCEPTED
    );
    assert_eq!(harness.terminal(&run.id).await.state, RunState::Cancelled);
    assert!(
        !alive(pid),
        "child must be reaped before run.cancelled persists"
    );
}

#[tokio::test]
async fn explicit_shutdown_interrupts_a_demo_run_and_reaps_its_child() {
    let harness = Harness::new();
    let run = harness.start("slow").await;
    let mut pid = None;
    for _ in 0..100 {
        let (_, events) = harness
            .request(
                "GET",
                &format!("/api/v1/runs/{}/events", run.id),
                None,
                true,
            )
            .await;
        if let Some(found) = events
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["type"] == "pi.spawned")
            .and_then(|e| e["payload"]["pid"].as_u64())
        {
            pid = Some(found);
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let pid = pid.expect("pi.spawned must record the child PID");
    harness.state.shutdown().await;
    let terminal = harness.terminal(&run.id).await;
    assert_eq!(terminal.state, RunState::Interrupted);
    let alive = std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    assert!(
        !alive,
        "child must be reaped before run.interrupted persists"
    );
    let (_, events) = harness
        .request(
            "GET",
            &format!("/api/v1/runs/{}/events", run.id),
            None,
            true,
        )
        .await;
    let terminal = events
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["type"] == "run.interrupted")
        .expect("run.interrupted persisted");
    assert!(terminal["payload"]["reason"]
        .as_str()
        .unwrap()
        .starts_with("interrupted by runtime shutdown"));
}

impl Harness {
    async fn raw(&self, request: Request<Body>) -> (StatusCode, Option<String>, Value) {
        let response = self.app.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .map(ToOwned::to_owned);
        let bytes = to_bytes(response.into_body(), 2 * 1024 * 1024)
            .await
            .unwrap();
        (
            status,
            content_type,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }
}

#[tokio::test]
async fn every_rejection_carries_a_displayable_json_reason() {
    let harness = Harness::new();
    let post = |content_type: Option<&str>, body: Vec<u8>| {
        let mut request = Request::builder()
            .method("POST")
            .uri("/api/v1/runs")
            .header("Authorization", format!("Bearer {TOKEN}"));
        if let Some(content_type) = content_type {
            request = request.header("Content-Type", content_type);
        }
        request.body(Body::from(body)).unwrap()
    };
    let get = |method: &str, uri: &str| {
        Request::builder()
            .method(method)
            .uri(uri)
            .header("Authorization", format!("Bearer {TOKEN}"))
            .body(Body::empty())
            .unwrap()
    };
    let cases = [
        (
            post(Some("application/json"), b"{\"prompt\":".to_vec()),
            StatusCode::BAD_REQUEST,
            "",
        ),
        (
            post(
                Some("application/json"),
                br#"{"prompt":"success","provider":"test","model":"synthetic","temperature":1}"#
                    .to_vec(),
            ),
            StatusCode::UNPROCESSABLE_ENTITY,
            "unknown field `temperature`",
        ),
        (
            post(Some("application/json"), br#"{"prompt":7}"#.to_vec()),
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid type",
        ),
        (
            post(None, br#"{"prompt":"success"}"#.to_vec()),
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "",
        ),
        (
            post(
                Some("application/json"),
                format!("{{\"prompt\":\"{}\"}}", "x".repeat(300 * 1024)).into_bytes(),
            ),
            StatusCode::PAYLOAD_TOO_LARGE,
            "",
        ),
        (
            get("GET", "/api/v1/runs/x/events?after=abc"),
            StatusCode::BAD_REQUEST,
            "",
        ),
        (get("GET", "/api/v1/missing"), StatusCode::NOT_FOUND, ""),
        (
            get("DELETE", "/api/v1/runs"),
            StatusCode::METHOD_NOT_ALLOWED,
            "",
        ),
    ];
    for (request, expected, detail) in cases {
        let label = format!("{} {}", request.method(), request.uri());
        let (status, content_type, body) = harness.raw(request).await;
        assert_eq!(status, expected, "{label}: {body}");
        assert_eq!(content_type.as_deref(), Some("application/json"), "{label}");
        let reason = body["error"].as_str().unwrap_or_default();
        // The desktop shows a reason of 1..=300 characters without controls.
        assert!(
            !reason.trim().is_empty()
                && reason.encode_utf16().count() <= 300
                && !reason.chars().any(char::is_control),
            "{label}: {reason:?}"
        );
        assert!(reason.contains(detail), "{label}: {reason:?}");
    }
}

#[tokio::test]
async fn health_reports_native_execution_and_the_bearer_scheme_is_case_insensitive() {
    let harness = Harness::new();
    let (status, health) = harness.request("GET", "/health", None, false).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(health["native_execution"], true);
    assert!(health.get("demo_only").is_none(), "{health}");
    for (header, expected) in [
        (format!("bearer {TOKEN}"), StatusCode::OK),
        (format!("BEARER {TOKEN}"), StatusCode::OK),
        (format!("Basic {TOKEN}"), StatusCode::UNAUTHORIZED),
        (format!("Bearer {TOKEN}x"), StatusCode::UNAUTHORIZED),
        (TOKEN.to_owned(), StatusCode::UNAUTHORIZED),
    ] {
        let request = Request::builder()
            .uri("/api/v1/runs")
            .header("Authorization", header.as_str())
            .body(Body::empty())
            .unwrap();
        assert_eq!(harness.raw(request).await.0, expected, "{header}");
    }
}

#[tokio::test]
async fn the_shared_slots_refuse_a_fifth_run_and_listing_can_be_limited() {
    let harness = Harness::new();
    let mut runs = Vec::new();
    for _ in 0..4 {
        runs.push(harness.start("slow").await);
    }
    let (status, body) = harness
        .request(
            "POST",
            "/api/v1/runs",
            Some(json!({"prompt":"slow","provider":"test","model":"synthetic"})),
            true,
        )
        .await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(body["error"], "four runs are already active");
    // The two list reads below must not straddle a run moving from `queued` to `running`.
    for run in &runs {
        let mut running = false;
        for _ in 0..400 {
            let (_, current) = harness
                .request("GET", &format!("/api/v1/runs/{}", run.id), None, true)
                .await;
            if current["state"] == "running" {
                running = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert!(running, "run {} never started running", run.id);
    }
    let (status, listed) = harness
        .request("GET", "/api/v1/runs?limit=2", None, true)
        .await;
    assert_eq!(status, StatusCode::OK);
    let listed = listed.as_array().unwrap();
    assert_eq!(listed.len(), 2);
    let (_, everything) = harness.request("GET", "/api/v1/runs", None, true).await;
    assert_eq!(listed[..], everything.as_array().unwrap()[..2]);
    for run in &runs {
        harness
            .request(
                "POST",
                &format!("/api/v1/runs/{}/cancel", run.id),
                None,
                true,
            )
            .await;
    }
    for run in &runs {
        assert_eq!(harness.terminal(&run.id).await.state, RunState::Cancelled);
    }
}

#[tokio::test]
async fn an_out_of_range_last_event_id_is_a_client_error() {
    let harness = Harness::new();
    let run = harness.start("success").await;
    harness.terminal(&run.id).await;
    let request = Request::builder()
        .uri(format!("/api/v1/runs/{}/stream", run.id))
        .header("Authorization", format!("Bearer {TOKEN}"))
        .header("Last-Event-ID", "18446744073709551615")
        .body(Body::empty())
        .unwrap();
    let (status, _, body) = harness.raw(request).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(body["error"].as_str().unwrap().contains("cursor"));
}

#[tokio::test]
async fn a_demo_run_past_its_deadline_fails_after_its_child_is_reaped() {
    let harness = Harness::with(|config| config.max_run_seconds = 1);
    let run = harness.start("slow").await;
    let terminal = harness.terminal(&run.id).await;
    assert_eq!(terminal.state, RunState::Failed);
    let (_, events) = harness
        .request(
            "GET",
            &format!("/api/v1/runs/{}/events", run.id),
            None,
            true,
        )
        .await;
    let events = events.as_array().unwrap();
    assert!(events
        .iter()
        .any(|e| e["type"] == "run.failed" && e["payload"]["reason"] == "run deadline exceeded"));
    #[cfg(unix)]
    {
        let pid = events
            .iter()
            .find(|e| e["type"] == "pi.spawned")
            .and_then(|e| e["payload"]["pid"].as_u64())
            .unwrap();
        let alive = std::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        assert!(!alive, "the child is reaped before run.failed persists");
    }
}
