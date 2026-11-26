//! Candidate checks that cannot run meaningfully: a missing dependency and a
//! check cut short by the workflow time budget are infrastructure outcomes,
//! never assertion failures a repair writer is asked to fix. Uses the
//! synthetic Pi fixture; no model is involved.
use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
    Router,
};
use luuna_runtime::{router, AppState, RuntimeConfig};
use serde_json::{json, Value};
use std::{fs, path::Path, path::PathBuf, process::Command, time::Duration};
use tower::ServiceExt;

const TOKEN: &str = "synthetic-test-token-with-at-least-32-bytes";

struct Harness {
    app: Router,
    state: AppState,
    root: PathBuf,
}

impl Harness {
    fn new() -> Self {
        let mock = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/pi-rpc/mock_pi.py")
            .canonicalize()
            .unwrap();
        let root = std::env::temp_dir().join(format!("luuna-checks-test-{}", uuid::Uuid::new_v4()));
        let state = AppState::open(RuntimeConfig {
            data_dir: root.clone(),
            pi_program: python().into(),
            pi_launcher_args: vec![mock.to_string_lossy().into_owned()],
            git_program: "git".into(),
            provider: None,
            model: None,
            token: TOKEN.into(),
            max_run_seconds: 10,
        })
        .unwrap();
        Self {
            app: router(state.clone()),
            state,
            root,
        }
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
        let bytes = to_bytes(response.into_body(), 32 * 1024 * 1024)
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

    async fn settled(&self, id: &str) -> Value {
        for _ in 0..600 {
            let run = self.run(id).await;
            if run["state"] == "validating" {
                self.released(id).await;
                return run;
            }
            if matches!(
                run["state"].as_str(),
                Some("completed" | "failed" | "cancelled" | "interrupted")
            ) {
                return run;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("run did not settle: {}", self.run(id).await);
    }
    /// The runtime reports `validating` before it removes the run from its active
    /// map (src/lib.rs); wait for the candidate view to say it has let go.
    async fn released(&self, id: &str) {
        for _ in 0..800 {
            if self.candidate(id).await["active"] == false {
                return;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!("run was never released: {}", self.candidate(id).await);
    }

    async fn events(&self, id: &str) -> Vec<Value> {
        self.request("GET", &format!("/api/v1/runs/{id}/events"), None)
            .await
            .1
            .as_array()
            .cloned()
            .unwrap_or_default()
    }

    async fn candidate(&self, id: &str) -> Value {
        let (status, body) = self
            .request("GET", &format!("/api/v1/runs/{id}/candidate"), None)
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        body
    }

    async fn candidate_settled(&self, id: &str) -> Value {
        for _ in 0..800 {
            let candidate = self.candidate(id).await;
            if candidate["active"] == false
                && !matches!(
                    candidate["evidence"]["status"].as_str(),
                    Some("pending" | "validating" | "reviewing")
                )
            {
                return candidate;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!("candidate did not settle: {}", self.candidate(id).await);
    }

    /// Register, confirm `check`, run a passing baseline in the developer
    /// checkout, then plan, approve and implement: a workflow whose candidate
    /// awaits validation.
    async fn implemented(&self, repo: &Path, check: Value, budgets: Value) -> String {
        clean_repository(repo);
        let (status, view) = self
            .request(
                "POST",
                "/api/v1/projects",
                Some(json!({"id":"fixture","name":"Fixture","repositories":[{"id":"app","path":repo.to_string_lossy()}]})),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED, "{view}");
        let manifest = json!({
            "schema_version": "0.1", "id": "fixture", "name": "Fixture", "revision": 1,
            "repositories": [{"id": "app", "remote": "https://example.invalid/fixture.git", "default_branch": "main",
                "profiles": ["python"], "execution": "native", "checks": [check]}],
            "relationships": [],
            "policy": {"approval": "plan", "testing_prerequisite": true, "publish": "automatic_pr", "merge": "human",
                "max_explorers": 1, "max_implementers": 1, "max_task_repairs": 2},
        });
        let (status, view) = self
            .request(
                "PUT",
                "/api/v1/projects/fixture/manifest",
                Some(json!({"expected_revision": 0, "manifest": manifest})),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{view}");
        let (status, baseline) = self
            .request(
                "POST",
                "/api/v1/projects/fixture/baselines",
                Some(json!({"repository_id": "app"})),
            )
            .await;
        assert_eq!(status, StatusCode::ACCEPTED, "{baseline}");
        let baseline = self.settled(baseline["id"].as_str().unwrap()).await;
        assert_eq!(baseline["state"], "completed", "{baseline}");
        let (status, ticket) = self
            .request(
                "POST",
                "/api/v1/projects/fixture/tickets",
                Some(json!({
                    "idempotency_key": format!("key-{}", uuid::Uuid::new_v4()),
                    "title": "Implement the greeting",
                    "intent": "Make src/main.py print the greeting.",
                    "acceptance_criteria": ["fixture:write_in_scope", "main prints the greeting"],
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
                    "budgets": budgets,
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
        assert_eq!(self.settled(&id).await["state"], "validating");
        id
    }

    async fn validate(&self, id: &str, auto_repair: bool) -> Value {
        let pending = self.candidate(id).await;
        let (status, body) = self
            .request(
                "POST",
                &format!("/api/v1/runs/{id}/validate"),
                Some(json!({"candidate_hash": pending["evidence"]["candidate"]["hash"], "provider": "test", "model": "synthetic", "auto_repair": auto_repair})),
            )
            .await;
        assert_eq!(status, StatusCode::ACCEPTED, "{body}");
        self.candidate_settled(id).await
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        self.state.cancel_active();
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn python() -> String {
    if cfg!(windows) { "python" } else { "python3" }.into()
}

/// A required check that passes in the developer checkout and evaluates the
/// expression `in_candidate` once the synthetic writer has changed `src/main.py`.
fn candidate_check(in_candidate: &str, timeout_seconds: u64) -> Value {
    let code = format!(
        "import sys, time; from pathlib import Path; ({in_candidate}) if 'implemented by' in Path('src/main.py').read_text() else None; print('running 1 tests'); print('test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured')"
    );
    json!({"id": "unit", "executable": python(), "args": ["-c", code], "required": true,
        "timeout_seconds": timeout_seconds, "capabilities": ["python"], "cwd": "."})
}

fn git(dir: &Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_AUTHOR_NAME", "Fixture")
        .env("GIT_AUTHOR_EMAIL", "fixture@example.invalid")
        .env("GIT_COMMITTER_NAME", "Fixture")
        .env("GIT_COMMITTER_EMAIL", "fixture@example.invalid")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn clean_repository(dir: &Path) {
    git(dir, &["init", "-q", "-b", "main"]);
    git(dir, &["config", "commit.gpgsign", "false"]);
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(dir.join("README.md"), "fixture\n").unwrap();
    fs::write(dir.join("src/main.py"), "print('fixture')\n").unwrap();
    git(dir, &["add", "."]);
    git(dir, &["commit", "-q", "-m", "initial"]);
    git(
        dir,
        &[
            "remote",
            "add",
            "origin",
            "https://example.invalid/fixture.git",
        ],
    );
}

#[tokio::test]
async fn a_candidate_check_missing_its_dependencies_is_blocked_without_a_repair_attempt() {
    let harness = Harness::new();
    let repo = tempfile::tempdir().unwrap();
    // The developer checkout has the tool; the fresh candidate worktree does
    // not, as with an ignored node_modules/ directory.
    let id = harness
        .implemented(
            repo.path(),
            candidate_check(
                "print('sh: 1: vitest: not found', file=sys.stderr), sys.exit(127)",
                30,
            ),
            json!({"max_attempts": 2}),
        )
        .await;
    let settled = harness.validate(&id, true).await;
    assert_eq!(
        settled["evidence"]["status"], "testing_blocked",
        "{settled}"
    );
    assert_eq!(harness.run(&id).await["state"], "testing_blocked");
    let result = &settled["evidence"]["results"][0];
    assert_eq!(result["status"], "blocked", "{result}");
    assert_eq!(result["exit_code"], 127);
    let reason = result["reason"].as_str().unwrap();
    assert!(
        reason.contains("`vitest`") && reason.contains("dependency-preparation recipe"),
        "{reason}"
    );
    // No writer attempt is spent on a failure no repair can fix.
    let events = harness.events(&id).await;
    assert!(!events
        .iter()
        .any(|event| event["type"] == "candidate.repair_started"));
    let store = luuna_storage::Store::open(harness.root.join("luuna.sqlite3")).unwrap();
    assert_eq!(store.attempts(&id).unwrap().len(), 1);
    // The candidate check's process group is durable for restart recovery.
    assert!(events.iter().any(|event| event["type"] == "check.spawned"
        && event["payload"]["recipe_id"] == "unit"
        && event["payload"]["process_group"]["pgid"].is_u64()));
}

#[tokio::test]
async fn a_required_check_cut_short_by_the_workflow_budget_pauses_for_a_re_plan() {
    let harness = Harness::new();
    let repo = tempfile::tempdir().unwrap();
    // The recipe allows 60 s; the approved workflow has far less left.
    let id = harness
        .implemented(
            repo.path(),
            candidate_check("time.sleep(120)", 60),
            json!({"timeout_seconds": 10}),
        )
        .await;
    let started = std::time::Instant::now();
    let settled = harness.validate(&id, true).await;
    assert!(started.elapsed() < Duration::from_secs(30), "{settled}");
    assert_eq!(settled["evidence"]["status"], "paused", "{settled}");
    assert_eq!(
        settled["evidence"]["reason"],
        "workflow time budget exhausted during required checks; re-plan required"
    );
    assert_eq!(harness.run(&id).await["state"], "paused");
    let result = &settled["evidence"]["results"][0];
    assert_eq!(result["status"], "blocked", "{result}");
    assert!(result["reason"]
        .as_str()
        .unwrap()
        .starts_with("workflow time budget exhausted after"));
    assert_eq!(settled["repair_allowed"], false, "{settled}");
    assert!(!harness
        .events(&id)
        .await
        .iter()
        .any(|event| event["type"] == "candidate.repair_started"));
}
