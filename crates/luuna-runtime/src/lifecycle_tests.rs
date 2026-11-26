//! Worker supervision, overflow-safe deadlines, stream completion and error
//! text. The workflow cases use the labeled synthetic Pi fixture; no model is
//! involved.
use crate::{cancel, deadline_after, one_line, router, stream_complete, AppState, RuntimeConfig};
use axum::{
    body::{to_bytes, Body},
    extract::{Path, State},
    http::{Request, StatusCode},
    Router,
};
use luuna_domain::RunState;
use luuna_storage::RunRecord;
use serde_json::{json, Value};
use std::{path::Path as FsPath, process::Command, time::Duration};
use tokio::sync::watch;
use tower::ServiceExt;

const TOKEN: &str = "synthetic-supervision-token-at-least-32-bytes";

fn python() -> &'static str {
    if cfg!(windows) {
        "python"
    } else {
        "python3"
    }
}

fn config(root: &FsPath) -> RuntimeConfig {
    RuntimeConfig {
        data_dir: root.join("private"),
        pi_program: python().into(),
        pi_launcher_args: vec![FsPath::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/pi-rpc/mock_pi.py")
            .canonicalize()
            .unwrap()
            .to_string_lossy()
            .into_owned()],
        git_program: "git".into(),
        provider: None,
        model: None,
        token: TOKEN.into(),
        max_run_seconds: 10,
    }
}

async fn until(mut done: impl FnMut() -> bool) {
    for _ in 0..400 {
        if done() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("condition not reached");
}

fn events(state: &AppState, id: &str) -> Vec<luuna_domain::EventEnvelope> {
    state.store.lock().unwrap().events_after(id, 0).unwrap()
}

#[tokio::test]
async fn deadline_after_saturates_instead_of_panicking() {
    let now = tokio::time::Instant::now();
    let far = deadline_after(Duration::from_secs(u64::MAX));
    assert!(far > now + Duration::from_secs(10 * 365 * 86_400));
    let near = deadline_after(Duration::from_secs(5));
    assert!(near >= now + Duration::from_secs(5));
    assert!(near <= tokio::time::Instant::now() + Duration::from_secs(5));
    // tokio's timer accepts the saturated deadline.
    assert!(tokio::time::timeout_at(far, async {}).await.is_ok());
}

#[test]
fn reasons_and_log_causes_are_one_bounded_line() {
    assert_eq!(one_line(" \n\t ", 280), None);
    assert_eq!(
        one_line("Failed to parse\r\nthe body:\u{1b}[31m bad", 280).unwrap(),
        "Failed to parse the body: [31m bad"
    );
    let clipped = one_line(&"é".repeat(400), 280).unwrap();
    assert_eq!(clipped.encode_utf16().count(), 280);
    assert!(clipped.ends_with('…'));
    let astral = one_line(&"😀".repeat(200), 280).unwrap();
    assert!(astral.encode_utf16().count() <= 280);
}

#[test]
fn streams_end_only_once_a_run_can_never_change_again() {
    use RunState::*;
    let all = [
        Queued,
        Running,
        Completed,
        Failed,
        Cancelled,
        Interrupted,
        Discovered,
        Exploring,
        TestingBlocked,
        PlanReady,
        Approved,
        Implementing,
        Validating,
        Reviewing,
        Reviewed,
        Paused,
    ];
    let run = |metadata: Value, state: RunState| RunRecord {
        id: "run".into(),
        project_id: "project".into(),
        state,
        revision: 1,
        metadata,
        created_at: String::new(),
        updated_at: String::new(),
    };
    for state in all {
        // Derived from the transition table: a failed or interrupted workflow
        // can still be repaired, re-validated or cancelled on the same run id.
        let resumable = all.iter().any(|next| state.transition(*next).is_ok());
        let workflow = run(json!({"kind": "workflow"}), state);
        assert_eq!(stream_complete(&workflow, false), !resumable, "{state:?}");
        assert!(
            !stream_complete(&workflow, true),
            "owned workflow {state:?}"
        );
        // Demo and baseline runs keep closing at any terminal state.
        for metadata in [json!({"demo_only": true}), json!({"kind": "baseline"})] {
            assert_eq!(
                stream_complete(&run(metadata, state), false),
                state.is_terminal(),
                "{state:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_panicking_worker_settles_its_run_frees_its_slot_and_keeps_the_store_usable() {
    let root = tempfile::tempdir().unwrap();
    let state = AppState::open(config(root.path())).unwrap();
    let run = {
        let mut store = state.store.lock().unwrap();
        let run = store.create_run("panicking", "demo", json!({})).unwrap();
        store
            .transition_run(
                &run.id,
                run.revision,
                RunState::Running,
                "run.running",
                json!({}),
            )
            .unwrap()
    };
    let (signal, receiver) = watch::channel(false);
    state.active.lock().unwrap().insert(run.id.clone(), signal);
    let worker = state.clone();
    state.spawn_supervised(run.id.clone(), async move {
        let _receiver = receiver;
        // Panic while holding the store, as a storage-layer bug would.
        let _store = worker.store.lock().unwrap();
        panic!("synthetic worker panic");
    });
    until(|| state.active.lock().unwrap().is_empty()).await;
    let settled = state
        .store
        .lock()
        .unwrap()
        .get_run(&run.id)
        .unwrap()
        .unwrap();
    assert_eq!(settled.state, RunState::Failed);
    assert!(events(&state, &run.id)
        .iter()
        .any(|e| e.event_type == "run.failed"
            && e.payload["reason"]
                .as_str()
                .is_some_and(|r| r.contains("stopped unexpectedly"))));
    // The slot is free and a repeated cancel answers with the settled run.
    let (status, _) = cancel(State(state.clone()), Path(run.id.clone()))
        .await
        .unwrap();
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn work_registered_after_the_shutdown_broadcast_starts_signalled() {
    let root = tempfile::tempdir().unwrap();
    let state = AppState::open(config(root.path())).unwrap();
    // Shutdown has run to completion; its broadcast reached no worker.
    state.shutdown().await;
    // A request admitted before admission closed registers its worker late.
    let (signal, receiver) = watch::channel(false);
    state.active.lock().unwrap().insert("late".into(), signal);
    let (observed, observe) = tokio::sync::oneshot::channel();
    state.spawn_supervised("late".into(), async move {
        let _ = observed.send(*receiver.borrow());
    });
    assert!(
        observe.await.unwrap(),
        "late work must see the stop signal before it starts"
    );
    until(|| state.active.lock().unwrap().is_empty()).await;
}

#[tokio::test]
async fn cancel_never_fails_on_a_worker_that_already_returned() {
    let root = tempfile::tempdir().unwrap();
    let state = AppState::open(config(root.path())).unwrap();
    let run = {
        let mut store = state.store.lock().unwrap();
        let run = store.create_run("returned", "demo", json!({})).unwrap();
        store
            .transition_run(
                &run.id,
                run.revision,
                RunState::Running,
                "run.running",
                json!({}),
            )
            .unwrap()
    };
    // The worker returned (its receiver is gone) before its supervisor
    // removed the entry.
    let (signal, receiver) = watch::channel(false);
    drop(receiver);
    state.active.lock().unwrap().insert(run.id.clone(), signal);
    let refused = cancel(State(state.clone()), Path(run.id.clone()))
        .await
        .unwrap_err();
    assert_eq!(refused.0, StatusCode::CONFLICT, "{refused:?}");
    state
        .finish(&run.id, RunState::Completed, json!({"reason": "settled"}))
        .unwrap();
    let (status, body) = cancel(State(state.clone()), Path(run.id.clone()))
        .await
        .unwrap();
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body.0.state, RunState::Completed);
}

/// A workflow with a confirmed manifest and a passed baseline, driven through
/// the authenticated router like a client.
struct Workflow {
    app: Router,
    state: AppState,
    _root: tempfile::TempDir,
    _repository: tempfile::TempDir,
}

impl Workflow {
    async fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let repository = tempfile::tempdir().unwrap();
        let git = |args: &[&str]| {
            let output = Command::new("git")
                .arg("-C")
                .arg(repository.path())
                .args(args)
                .env("GIT_AUTHOR_NAME", "Fixture")
                .env("GIT_AUTHOR_EMAIL", "fixture@example.invalid")
                .env("GIT_COMMITTER_NAME", "Fixture")
                .env("GIT_COMMITTER_EMAIL", "fixture@example.invalid")
                .output()
                .unwrap();
            assert!(output.status.success(), "git {args:?}");
        };
        git(&["init", "-q", "-b", "main"]);
        git(&["config", "commit.gpgsign", "false"]);
        std::fs::create_dir_all(repository.path().join("src")).unwrap();
        std::fs::write(repository.path().join("src/main.py"), "print('fixture')\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "initial"]);
        git(&[
            "remote",
            "add",
            "origin",
            "https://example.invalid/fixture.git",
        ]);
        let state = AppState::open(config(root.path())).unwrap();
        let workflow = Self {
            app: router(state.clone()),
            state,
            _root: root,
            _repository: repository,
        };
        let path = workflow._repository.path().to_string_lossy().into_owned();
        let (status, view) = workflow
            .request(
                "POST",
                "/api/v1/projects",
                Some(json!({"id":"fixture","name":"Fixture","repositories":[{"id":"app","path":path}]})),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED, "{view}");
        let check = json!({"id": "unit", "executable": python(), "required": true,
            "args": ["-c", "print('running 1 tests'); print('test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured')"],
            "timeout_seconds": 30, "capabilities": ["python"], "cwd": "."});
        let manifest = json!({
            "schema_version": "0.1", "id": "fixture", "name": "Fixture", "revision": 1,
            "repositories": [{"id": "app", "remote": "https://example.invalid/fixture.git", "default_branch": "main",
                "profiles": ["python"], "execution": "native", "checks": [check]}],
            "relationships": [],
            "policy": {"approval": "plan", "testing_prerequisite": true, "publish": "automatic_pr", "merge": "human",
                "max_explorers": 1, "max_implementers": 1, "max_task_repairs": 2},
        });
        let (status, view) = workflow
            .request(
                "PUT",
                "/api/v1/projects/fixture/manifest",
                Some(json!({"expected_revision": 0, "manifest": manifest})),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{view}");
        let (status, baseline) = workflow
            .request(
                "POST",
                "/api/v1/projects/fixture/baselines",
                Some(json!({"repository_id": "app"})),
            )
            .await;
        assert_eq!(status, StatusCode::ACCEPTED, "{baseline}");
        let id = baseline["id"].as_str().unwrap().to_owned();
        workflow
            .until_state(&id, &["completed", "failed", "cancelled"])
            .await;
        assert_eq!(workflow.run(&id).await["state"], "completed");
        workflow
    }

    async fn request(&self, method: &str, path: &str, body: Option<Value>) -> (StatusCode, Value) {
        let mut request = Request::builder()
            .method(method)
            .uri(path)
            .header("Authorization", format!("Bearer {TOKEN}"));
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
        let bytes = to_bytes(response.into_body(), 8 * 1024 * 1024)
            .await
            .unwrap();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    async fn run(&self, id: &str) -> Value {
        self.request("GET", &format!("/api/v1/runs/{id}"), None)
            .await
            .1
    }

    async fn until_state(&self, id: &str, states: &[&str]) -> Value {
        for _ in 0..600 {
            let run = self.run(id).await;
            if states.contains(&run["state"].as_str().unwrap_or_default()) {
                return run;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!("run never reached {states:?}: {}", self.run(id).await);
    }

    async fn until_event(&self, id: &str, kind: &str) {
        for _ in 0..600 {
            if events(&self.state, id).iter().any(|e| e.event_type == kind) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!("{kind} never appeared for {id}");
    }

    /// An approved workflow whose writer follows the fixture `scenario`,
    /// with the default budgets spelled out.
    async fn implementing(&self, scenario: &str) -> String {
        let (status, ticket) = self
            .request(
                "POST",
                "/api/v1/projects/fixture/tickets",
                Some(json!({
                    "idempotency_key": format!("key-{scenario}"),
                    "title": "Implement the greeting",
                    "intent": "Make src/main.py print the greeting.",
                    "acceptance_criteria": [format!("fixture:{scenario}"), "main prints the greeting"],
                    "repository_ids": ["app"],
                })),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED, "{ticket}");
        let (status, opened) = self
            .request(
                "POST",
                &format!("/api/v1/tickets/{}/plans", ticket["id"].as_str().unwrap()),
                Some(json!({
                    "task": {"repository_id": "app", "paths": ["src"], "operations": ["read", "write"]},
                    "budgets": {"max_attempts": 2, "timeout_seconds": 600, "cost_usd_micros": 1_000_000},
                })),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED, "{opened}");
        let plan = &opened["plan"];
        let (status, approved) = self
            .request(
                "POST",
                &format!("/api/v1/plans/{}/approvals", plan["id"].as_str().unwrap()),
                Some(json!({"plan_revision": plan["revision"], "plan_hash": plan["hash"], "approved_by": "developer"})),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED, "{approved}");
        let id = opened["run"]["id"].as_str().unwrap().to_owned();
        let (status, body) = self
            .request(
                "POST",
                &format!("/api/v1/runs/{id}/implement"),
                Some(json!({"provider": "test", "model": "synthetic"})),
            )
            .await;
        assert_eq!(status, StatusCode::ACCEPTED, "{body}");
        id
    }

    /// Stop the still-running fixture worker and wait for its slot, so its
    /// child is reaped before the data directory is removed.
    async fn release(&self) {
        self.state.cancel_active();
        until(|| self.state.active.lock().unwrap().is_empty()).await;
    }
}

#[tokio::test]
async fn a_panicked_writer_reserves_the_remaining_budget_and_abandons_its_lease() {
    let workflow = Workflow::new().await;
    let id = workflow.implementing("slow").await;
    // The writer holds a lease and a running attempt while its turn is open.
    workflow.until_event(&id, "pi.agent_start").await;
    workflow.state.settle_panicked_worker(&id);

    let run = workflow.run(&id).await;
    assert_eq!(run["state"], "failed", "{run}");
    let (_, attempts) = workflow
        .request("GET", &format!("/api/v1/runs/{id}/attempts"), None)
        .await;
    let attempt = &attempts.as_array().unwrap()[0];
    assert_eq!(attempt["state"], "failed", "{attempt}");
    // Nothing was reported, so the whole approved remainder is reserved.
    assert_eq!(attempt["payload"]["usage"]["elapsed_seconds"], 600);
    assert_eq!(attempt["payload"]["usage"]["cost_usd_micros"], 1_000_000);
    let usage = workflow
        .state
        .store
        .lock()
        .unwrap()
        .workflow_usage(&id)
        .unwrap();
    assert_eq!(
        (usage.elapsed_seconds, usage.cost_usd_micros),
        (600, 1_000_000)
    );
    let (_, leases) = workflow
        .request("GET", &format!("/api/v1/runs/{id}/worktrees"), None)
        .await;
    assert_eq!(leases[0]["state"], "abandoned", "{leases}");

    // The stand-in for the dead worker cannot overwrite that settlement.
    workflow.release().await;
    assert_eq!(workflow.run(&id).await["state"], "failed");
}

#[tokio::test]
async fn a_panicked_review_fails_its_candidate_and_reserves_the_remaining_cost() {
    let workflow = Workflow::new().await;
    let id = workflow.implementing("review_hold").await;
    let run = workflow.until_state(&id, &["validating", "failed"]).await;
    assert_eq!(run["state"], "validating", "{run}");
    until(|| !workflow.state.active.lock().unwrap().contains_key(&id)).await;
    let (_, candidate) = workflow
        .request("GET", &format!("/api/v1/runs/{id}/candidate"), None)
        .await;
    let hash = candidate["evidence"]["candidate"]["hash"].clone();
    let (status, body) = workflow
        .request(
            "POST",
            &format!("/api/v1/runs/{id}/validate"),
            Some(json!({"candidate_hash": hash, "provider": "test", "model": "synthetic"})),
        )
        .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    workflow.until_event(&id, "review.started").await;
    workflow.until_state(&id, &["reviewing"]).await;
    workflow.state.settle_panicked_worker(&id);

    assert_eq!(workflow.run(&id).await["state"], "failed");
    let (_, candidate) = workflow
        .request("GET", &format!("/api/v1/runs/{id}/candidate"), None)
        .await;
    assert_eq!(candidate["evidence"]["status"], "failed", "{candidate}");
    assert!(candidate["evidence"]["reason"]
        .as_str()
        .is_some_and(|r| r.contains("stopped unexpectedly")));
    assert_eq!(candidate["revalidate_allowed"], false, "{candidate}");
    assert_eq!(candidate["repair_allowed"], false, "{candidate}");
    let usage = workflow
        .state
        .store
        .lock()
        .unwrap()
        .workflow_usage(&id)
        .unwrap();
    assert_eq!(usage.cost_usd_micros, 1_000_000, "{usage:?}");
    workflow.release().await;
}
