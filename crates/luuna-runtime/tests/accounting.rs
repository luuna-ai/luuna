//! M3 accounting through the authenticated API with the synthetic Pi fixture:
//! aggregate and per-ticket usage, unmetered and compaction spend, reviewer
//! retries and usage persistence, restart reservation, plan idempotency, the
//! one-writer-per-repository gate and automatic-repair failure reasons. No
//! model is involved.
use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
    Router,
};
use luuna_runtime::{router, AppState, RuntimeConfig};
use serde_json::{json, Value};
use std::{fs, path::Path, path::PathBuf, process::Command, time::Duration};
use tower::ServiceExt;

const TOKEN: &str = "synthetic-accounting-token-with-at-least-32-bytes";

fn mock_pi() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/pi-rpc/mock_pi.py")
        .canonicalize()
        .unwrap()
}

struct Harness {
    app: Router,
    state: AppState,
    root: PathBuf,
}

impl Harness {
    fn new() -> Self {
        Self::with_pi(mock_pi())
    }
    fn with_pi(script: PathBuf) -> Self {
        let root =
            std::env::temp_dir().join(format!("luuna-accounting-test-{}", uuid::Uuid::new_v4()));
        let state = AppState::open(RuntimeConfig {
            data_dir: root.clone(),
            pi_program: python().into(),
            pi_launcher_args: vec![script.to_string_lossy().into_owned()],
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
            if matches!(
                run["state"].as_str(),
                Some("validating" | "completed" | "failed" | "cancelled" | "interrupted")
            ) {
                return run;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("run did not settle: {}", self.run(id).await);
    }
    async fn events(&self, id: &str) -> Vec<Value> {
        self.request("GET", &format!("/api/v1/runs/{id}/events"), None)
            .await
            .1
            .as_array()
            .cloned()
            .unwrap_or_default()
    }
    async fn wait_for_event(&self, id: &str, kind: &str) {
        for _ in 0..600 {
            if self.events(id).await.iter().any(|e| e["type"] == kind) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!("{kind} never appeared for {id}");
    }
    async fn confirm(&self, checks: Vec<Value>) {
        let manifest = json!({
            "schema_version": "0.1", "id": "fixture", "name": "Fixture", "revision": 1,
            "repositories": [{"id": "app", "remote": "https://example.invalid/fixture.git", "default_branch": "main",
                "profiles": ["python"], "execution": "native", "checks": checks}],
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
    }
    async fn ready(&self, repo: &Path, checks: Vec<Value>) {
        clean_repository(repo);
        let (status, view) = self
            .request(
                "POST",
                "/api/v1/projects",
                Some(json!({"id":"fixture","name":"Fixture","repositories":[{"id":"app","path":repo.to_string_lossy()}]})),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED, "{view}");
        self.confirm(checks).await;
        let (status, run) = self
            .request(
                "POST",
                "/api/v1/projects/fixture/baselines",
                Some(json!({"repository_id": "app"})),
            )
            .await;
        assert_eq!(status, StatusCode::ACCEPTED, "{run}");
        let run = self.settled(run["id"].as_str().unwrap()).await;
        assert_eq!(run["state"], "completed", "{run}");
    }
    async fn ticket(&self, scenario: &str) -> String {
        let (status, ticket) = self
            .request(
                "POST",
                "/api/v1/projects/fixture/tickets",
                Some(json!({
                    "idempotency_key": format!("key-{}", uuid::Uuid::new_v4()),
                    "title": "Implement the greeting",
                    "intent": "Make src/main.py print the greeting.",
                    "acceptance_criteria": [format!("fixture:{scenario}"), "main prints the greeting"],
                    "repository_ids": ["app"],
                })),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED, "{ticket}");
        ticket["id"].as_str().unwrap().to_owned()
    }
    async fn plan(&self, ticket_id: &str, body: Value) -> (StatusCode, Value) {
        self.request(
            "POST",
            &format!("/api/v1/tickets/{ticket_id}/plans"),
            Some(body),
        )
        .await
    }
    async fn approve(&self, plan: &Value) {
        let (status, approved) = self
            .request(
                "POST",
                &format!("/api/v1/plans/{}/approvals", plan["id"].as_str().unwrap()),
                Some(json!({"plan_revision": plan["revision"], "plan_hash": plan["hash"], "approved_by": "developer"})),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED, "{approved}");
    }
    /// Ticket, plan with `budgets`, approval: an approved workflow run.
    async fn approved(&self, scenario: &str, budgets: Value) -> String {
        let ticket = self.ticket(scenario).await;
        let (status, opened) = self.plan(&ticket, task(budgets)).await;
        assert_eq!(status, StatusCode::CREATED, "{opened}");
        self.approve(&opened["plan"]).await;
        opened["run"]["id"].as_str().unwrap().to_owned()
    }
    async fn implement(&self, run_id: &str) -> (StatusCode, Value) {
        self.request(
            "POST",
            &format!("/api/v1/runs/{run_id}/implement"),
            Some(json!({"provider": "test", "model": "synthetic"})),
        )
        .await
    }
    async fn candidate(&self, run_id: &str) -> Value {
        let (status, body) = self
            .request("GET", &format!("/api/v1/runs/{run_id}/candidate"), None)
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        body
    }
    async fn validate(&self, run_id: &str, auto_repair: bool) -> (StatusCode, Value) {
        let hash = self.candidate(run_id).await["evidence"]["candidate"]["hash"].clone();
        self.request(
            "POST",
            &format!("/api/v1/runs/{run_id}/validate"),
            Some(json!({"candidate_hash":hash,"provider":"test","model":"synthetic","auto_repair":auto_repair})),
        )
        .await
    }
    async fn candidate_settled(&self, run_id: &str) -> Value {
        for _ in 0..800 {
            let candidate = self.candidate(run_id).await;
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
        panic!("candidate did not settle: {}", self.candidate(run_id).await);
    }
    /// An approved workflow whose writer has settled into `validating`.
    async fn implemented(&self, scenario: &str, budgets: Value) -> String {
        let id = self.approved(scenario, budgets).await;
        assert_eq!(self.implement(&id).await.0, StatusCode::ACCEPTED);
        let run = self.settled(&id).await;
        assert_eq!(run["state"], "validating", "{run}");
        id
    }
    fn store(&self) -> luuna_storage::Store {
        luuna_storage::Store::open(self.root.join("luuna.sqlite3")).unwrap()
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
fn check(id: &str, code: &str) -> Value {
    json!({"id": id, "executable": python(), "args": ["-c", code], "required": true,
        "timeout_seconds": 30, "capabilities": ["python"], "cwd": "."})
}
fn passing_check() -> Value {
    check(
        "unit",
        "print('running 1 tests'); print('test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured')",
    )
}
fn task(budgets: Value) -> Value {
    json!({
        "task": {"repository_id": "app", "paths": ["src"], "operations": ["read", "write"]},
        "budgets": budgets,
    })
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
    fs::write(
        dir.join("pyproject.toml"),
        "[project]\nname = \"fixture\"\n",
    )
    .unwrap();
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
fn of_type<'a>(events: &'a [Value], kind: &str) -> Vec<&'a Value> {
    events.iter().filter(|e| e["type"] == kind).collect()
}

#[tokio::test]
async fn a_reviewer_tolerates_a_provider_error_that_pi_retries_like_the_writer() {
    let harness = Harness::new();
    let repo = tempfile::tempdir().unwrap();
    harness.ready(repo.path(), vec![passing_check()]).await;
    let id = harness.implemented("review_retry", json!({})).await;
    assert_eq!(harness.validate(&id, false).await.0, StatusCode::ACCEPTED);
    let settled = harness.candidate_settled(&id).await;
    assert_eq!(settled["evidence"]["status"], "reviewed", "{settled}");
    assert_eq!(settled["evidence"]["review"]["verdict"], "passed");
    // The errored request and the successful retry are both charged.
    assert_eq!(settled["evidence"]["cost_usd_micros"], 600, "{settled}");
}

#[tokio::test]
async fn reviewer_usage_is_persisted_per_completed_message_not_per_streaming_record() {
    let harness = Harness::new();
    let repo = tempfile::tempdir().unwrap();
    harness.ready(repo.path(), vec![passing_check()]).await;
    let id = harness.implemented("review_stream", json!({})).await;
    assert_eq!(harness.validate(&id, false).await.0, StatusCode::ACCEPTED);
    let settled = harness.candidate_settled(&id).await;
    assert_eq!(settled["evidence"]["status"], "reviewed", "{settled}");
    // Forty cumulative streaming updates are one message's cost, once.
    assert_eq!(settled["evidence"]["cost_usd_micros"], 500, "{settled}");
    let events = harness.events(&id).await;
    let started = events
        .iter()
        .position(|e| e["type"] == "review.started")
        .unwrap();
    let finished = events
        .iter()
        .position(|e| e["type"] == "candidate.review_finished")
        .unwrap();
    let charges = events[started..finished]
        .iter()
        .filter(|e| e["type"] == "candidate.usage")
        .count();
    assert!(charges <= 4, "{charges} usage events during one review");
}

#[tokio::test]
async fn compaction_spend_counts_against_the_approved_cost_budget() {
    let harness = Harness::new();
    let repo = tempfile::tempdir().unwrap();
    harness.ready(repo.path(), vec![passing_check()]).await;
    let id = harness
        .approved("write_compaction", json!({"cost_usd_micros": 100_000}))
        .await;
    assert_eq!(harness.implement(&id).await.0, StatusCode::ACCEPTED);
    let run = harness.settled(&id).await;
    assert_eq!(run["state"], "failed", "{run}");
    let attempts = harness.store().attempts(&id).unwrap();
    let reason = attempts[0].payload["reason"].as_str().unwrap();
    assert!(reason.contains("budget exceeded"), "{reason}");
    // A failed attempt records its usage beside the reason.
    let payload = &attempts[0].payload;
    let usage = payload.get("usage").unwrap_or(payload);
    assert!(
        usage["cost_usd_micros"].as_u64().unwrap() >= 250_000,
        "{payload}"
    );
}

#[tokio::test]
async fn unpriced_token_usage_is_flagged_without_refusing_the_model() {
    let harness = Harness::new();
    let repo = tempfile::tempdir().unwrap();
    harness.ready(repo.path(), vec![passing_check()]).await;
    let id = harness.implemented("write_unpriced", json!({})).await;
    let events = harness.events(&id).await;
    let flagged = of_type(&events, luuna_storage::COST_UNREPORTED_EVENT);
    assert_eq!(flagged.len(), 1, "{events:?}");
    assert_eq!(flagged[0]["payload"]["source"], "writer");
    let attempts = harness.store().attempts(&id).unwrap();
    assert_eq!(attempts[0].payload["usage"]["cost_unreported"], true);
    let candidate = harness.candidate(&id).await;
    assert_eq!(candidate["usage"]["cost_unreported"], true, "{candidate}");
    assert_eq!(candidate["usage"]["cost_usd_micros"], 0);
}

#[tokio::test]
async fn candidate_usage_and_attempt_provenance_describe_the_whole_workflow() {
    let harness = Harness::new();
    let repo = tempfile::tempdir().unwrap();
    harness.ready(repo.path(), vec![passing_check()]).await;
    let id = harness.implemented("write_in_scope", json!({})).await;
    assert_eq!(harness.validate(&id, false).await.0, StatusCode::ACCEPTED);
    let settled = harness.candidate_settled(&id).await;
    assert_eq!(settled["evidence"]["status"], "reviewed", "{settled}");
    let usage = &settled["usage"];
    assert_eq!(usage["attempts"], 1, "{usage}");
    assert_eq!(usage["max_attempts"], 2);
    assert_eq!(usage["timeout_seconds"], 600);
    // The writer attempt and the review, not the candidate's share alone.
    assert_eq!(usage["cost_usd_micros"], 1000);
    assert_eq!(usage["cost_budget_usd_micros"], 1_000_000);
    assert_eq!(usage["cost_unreported"], false);
    assert_eq!(usage["repairs"], 0);
    assert_eq!(usage["max_repairs"], 2);
    assert!(usage["elapsed_seconds"].as_u64().unwrap() >= 1);
    // A settled attempt keeps the provenance it was admitted with.
    let (_, attempts) = harness
        .request("GET", &format!("/api/v1/runs/{id}/attempts"), None)
        .await;
    let payload = &attempts[0]["payload"];
    assert_eq!(payload["provider"], "test", "{payload}");
    assert_eq!(payload["model"], "synthetic");
    assert_eq!(payload["trigger"], "implement");
    assert_eq!(payload["pi_version"], "1.3.0");
    assert!(payload["environment_fingerprint"]
        .as_str()
        .unwrap()
        .starts_with("sha256:"));
    assert!(payload["candidate_hash"].is_string() && payload["usage"].is_object());
    let events = harness.events(&id).await;
    let started = of_type(&events, "review.started")[0];
    assert_eq!(started["payload"]["provider"], "test");
    assert_eq!(started["payload"]["model"], "synthetic");
    assert_eq!(started["payload"]["generation"], 1);
    let session = started["payload"]["reviewer_session"].clone();
    for marker in ["review.prompt_sent", "review.usage_settled"] {
        assert_eq!(
            of_type(&events, marker)[0]["payload"]["reviewer_session"],
            session,
            "{marker}"
        );
    }
}

#[tokio::test]
async fn a_new_plan_revision_shows_what_the_ticket_already_spent() {
    let harness = Harness::new();
    let repo = tempfile::tempdir().unwrap();
    harness.ready(repo.path(), vec![passing_check()]).await;
    let ticket = harness.ticket("write_in_scope").await;
    let (status, first) = harness.plan(&ticket, task(json!({}))).await;
    assert_eq!(status, StatusCode::CREATED, "{first}");
    assert_eq!(first["ticket_usage"]["workflows"], 1);
    assert_eq!(first["ticket_usage"]["cost_usd_micros"], 0);
    harness.approve(&first["plan"]).await;
    let id = first["run"]["id"].as_str().unwrap();
    assert_eq!(harness.implement(id).await.0, StatusCode::ACCEPTED);
    assert_eq!(harness.settled(id).await["state"], "validating");
    // The next revision gets its own budgets; the approver sees the spend.
    let (status, second) = harness.plan(&ticket, task(json!({}))).await;
    assert_eq!(status, StatusCode::CREATED, "{second}");
    assert_eq!(second["plan"]["revision"], 2);
    let carried = &second["ticket_usage"];
    assert_eq!(carried["workflows"], 2, "{carried}");
    assert_eq!(carried["attempts"], 1);
    assert_eq!(carried["cost_usd_micros"], 500);
    let (status, view) = harness
        .request("GET", &format!("/api/v1/tickets/{ticket}"), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(view["id"], ticket.as_str());
    assert_eq!(view["usage"], *carried);
    // Work in progress is never superseded by a re-plan.
    assert_eq!(harness.run(id).await["state"], "validating");
}

#[tokio::test]
async fn plan_creation_replays_an_idempotency_key_and_supersedes_waiting_revisions() {
    let harness = Harness::new();
    let repo = tempfile::tempdir().unwrap();
    harness.ready(repo.path(), vec![passing_check()]).await;
    let ticket = harness.ticket("write_in_scope").await;
    let mut body = task(json!({"max_attempts": 2}));
    body["idempotency_key"] = json!("client-retry-1");
    let (status, first) = harness.plan(&ticket, body.clone()).await;
    assert_eq!(status, StatusCode::CREATED, "{first}");
    // A retry after a lost response returns the same revision and workflow.
    let (status, replay) = harness.plan(&ticket, body.clone()).await;
    assert_eq!(status, StatusCode::OK, "{replay}");
    assert_eq!(replay["plan"]["hash"], first["plan"]["hash"]);
    assert_eq!(replay["run"]["id"], first["run"]["id"]);
    // Defaults spelled out are the same request.
    let mut explicit = body.clone();
    explicit["budgets"] =
        json!({"max_attempts": 2, "timeout_seconds": 600, "cost_usd_micros": 1_000_000});
    assert_eq!(harness.plan(&ticket, explicit).await.0, StatusCode::OK);
    let (_, plans) = harness
        .request("GET", &format!("/api/v1/tickets/{ticket}/plans"), None)
        .await;
    assert_eq!(plans.as_array().unwrap().len(), 1);
    // The same key with a different request is refused.
    let mut different = body.clone();
    different["budgets"] = json!({"max_attempts": 1});
    let (status, refused) = harness.plan(&ticket, different).await;
    assert_eq!(status, StatusCode::CONFLICT, "{refused}");
    for key in [json!(""), json!("has space"), json!("x".repeat(129))] {
        let mut invalid = body.clone();
        invalid["idempotency_key"] = key;
        assert_eq!(
            harness.plan(&ticket, invalid).await.0,
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }
    // Without a key every submission is the next revision; the waiting one
    // it replaces is settled instead of staying open forever.
    let (status, second) = harness.plan(&ticket, task(json!({}))).await;
    assert_eq!(status, StatusCode::CREATED, "{second}");
    assert_eq!(second["plan"]["revision"], 2);
    let earlier = first["run"]["id"].as_str().unwrap();
    assert_eq!(harness.run(earlier).await["state"], "cancelled");
    let events = harness.events(earlier).await;
    assert_eq!(
        of_type(&events, "plan.superseded")[0]["payload"]["superseded_by"],
        2
    );
    // The superseded revision cannot be approved.
    let (status, _) = harness
        .request(
            "POST",
            &format!("/api/v1/plans/{}/approvals", first["plan"]["id"].as_str().unwrap()),
            Some(json!({"plan_revision": 1, "plan_hash": first["plan"]["hash"], "approved_by": "developer"})),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
}

#[tokio::test]
async fn concurrent_implementations_of_one_repository_admit_exactly_one_writer() {
    let harness = Harness::new();
    let repo = tempfile::tempdir().unwrap();
    harness.ready(repo.path(), vec![passing_check()]).await;
    let first = harness.approved("slow", json!({})).await;
    let second = harness.approved("slow", json!({})).await;
    // Neither request waits for the other's writer to lease its worktree.
    let (a, b) = tokio::join!(harness.implement(&first), harness.implement(&second));
    let mut statuses = [a.0, b.0];
    statuses.sort();
    assert_eq!(
        statuses,
        [StatusCode::ACCEPTED, StatusCode::CONFLICT],
        "{a:?} {b:?}"
    );
    let (refused, refusal) = if a.0 == StatusCode::CONFLICT {
        (&first, a.1)
    } else {
        (&second, b.1)
    };
    assert!(
        refusal["error"]
            .as_str()
            .unwrap()
            .contains("already has an active writer"),
        "{refusal}"
    );
    // The refused workflow keeps its approval and spent nothing.
    assert_eq!(harness.run(refused).await["state"], "approved");
    assert!(harness.store().attempts(refused).unwrap().is_empty());
}

#[tokio::test]
async fn automatic_repair_reports_the_repair_writers_own_failure() {
    let harness = Harness::new();
    let repo = tempfile::tempdir().unwrap();
    harness.ready(repo.path(), vec![passing_check()]).await;
    // The writer (500) and the blocking review (500) fit the budget; the
    // repair writer's 500 does not.
    let id = harness
        .implemented(
            "review_blocking",
            json!({"max_attempts": 3, "cost_usd_micros": 1200}),
        )
        .await;
    assert_eq!(harness.validate(&id, true).await.0, StatusCode::ACCEPTED);
    let settled = harness.candidate_settled(&id).await;
    assert_eq!(settled["evidence"]["status"], "paused", "{settled}");
    let reason = settled["evidence"]["reason"].as_str().unwrap();
    assert!(
        reason.contains("automatic repair writer failed: budget exceeded"),
        "{reason}"
    );
    // The repair gate's refusal names the spent budget and the re-plan.
    assert!(
        reason.contains("budget exhausted; re-plan required"),
        "{reason}"
    );
    // The candidate's own earlier reason is kept.
    assert!(
        reason.contains("independent review did not pass"),
        "{reason}"
    );
    assert_eq!(settled["repair_allowed"], false);
    let attempts = harness.store().attempts(&id).unwrap();
    assert_eq!(attempts.len(), 2);
    assert_eq!(attempts[1].payload["trigger"], "automatic_repair");
    assert_eq!(attempts[1].payload["model"], "synthetic");
    assert_eq!(
        attempts[1].payload["repair_of"],
        settled["evidence"]["candidate"]["hash"]
    );
}

#[tokio::test]
async fn a_restart_before_the_reviewer_was_prompted_reserves_no_provider_spend() {
    // A launcher that recognises the reviewer by its system-prompt file (the
    // runtime passes `--system-prompt <path>`, never the text itself) and holds
    // it before its get_state handshake while a `stall` file exists, and
    // otherwise runs the synthetic fixture.
    let launcher = tempfile::tempdir().unwrap();
    let script = launcher.path().join("pi.py");
    fs::write(
        &script,
        format!(
            "import os, runpy, sys, time\n\
             here = os.path.dirname(os.path.abspath(__file__))\n\
             text = ''\n\
             if '--version' not in sys.argv and '--system-prompt' in sys.argv:\n\
             \x20   i = sys.argv.index('--system-prompt')\n\
             \x20   if i + 1 < len(sys.argv) and os.path.isfile(sys.argv[i + 1]):\n\
             \x20       with open(sys.argv[i + 1], encoding='utf-8') as handle:\n\
             \x20           text = handle.read().lower()\n\
             if 'luuna independent reviewer' in text and os.path.exists(os.path.join(here, 'stall')):\n\
             \x20   open(os.path.join(here, 'held'), 'w').close()\n\
             \x20   while os.path.exists(os.path.join(here, 'stall')):\n\
             \x20       time.sleep(0.05)\n\
             sys.argv[0] = {mock:?}\n\
             runpy.run_path({mock:?}, run_name='__main__')\n",
            mock = mock_pi().to_string_lossy()
        ),
    )
    .unwrap();
    fs::write(launcher.path().join("stall"), "").unwrap();
    let harness = Harness::with_pi(script);
    let repo = tempfile::tempdir().unwrap();
    harness.ready(repo.path(), vec![passing_check()]).await;
    let id = harness.implemented("write_in_scope", json!({})).await;
    assert_eq!(harness.validate(&id, false).await.0, StatusCode::ACCEPTED);
    harness.wait_for_event(&id, "review.started").await;
    // The test's premise: the launcher really holds the reviewer.
    let mut held = false;
    for _ in 0..600 {
        if launcher.path().join("held").exists() {
            held = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(held, "the reviewer was never held before its handshake");
    // The runtime dies while its reviewer is still starting up.
    let mut store = harness.store();
    assert_eq!(store.interrupt_active_runs().unwrap(), 1);
    harness.state.cancel_active();
    let interrupted = harness.candidate_settled(&id).await;
    assert_eq!(interrupted["evidence"]["status"], "interrupted");
    let reason = interrupted["evidence"]["reason"].as_str().unwrap();
    assert!(
        reason.contains("before the reviewer was prompted"),
        "{reason}"
    );
    assert!(
        store.workflow_usage(&id).unwrap().cost_usd_micros < 1_000_000,
        "no spend may be reserved for a reviewer that was never prompted"
    );
    // The documented recovery is open: validate again with a fresh reviewer.
    fs::remove_file(launcher.path().join("stall")).unwrap();
    let (status, response) = harness.validate(&id, false).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{response}");
    let reviewed = harness.candidate_settled(&id).await;
    assert_eq!(reviewed["evidence"]["status"], "reviewed", "{reviewed}");
    assert_eq!(reviewed["evidence"]["validation_generation"], 2);
}

#[tokio::test]
async fn a_running_check_stamps_its_stage_heartbeat() {
    let harness = Harness::new();
    let repo = tempfile::tempdir().unwrap();
    // The check holds only on the candidate, long enough for one heartbeat.
    let hold = luuna_storage::STAGE_HEARTBEAT_SECONDS + 2;
    harness
        .ready(
            repo.path(),
            vec![check(
                "unit",
                &format!("from pathlib import Path; import time; time.sleep({hold}) if 'implemented by' in Path('src/main.py').read_text() else None; print('running 1 tests'); print('test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured')"),
            )],
        )
        .await;
    let id = harness.implemented("write_in_scope", json!({})).await;
    assert_eq!(harness.validate(&id, false).await.0, StatusCode::ACCEPTED);
    let mut stamped = false;
    for _ in 0..200 {
        let evidence = harness.candidate(&id).await["evidence"].clone();
        if evidence["status"] == "validating"
            && evidence["stage_checkpoint_at"].is_string()
            && evidence["stage_checkpoint_at"] != evidence["stage_started_at"]
        {
            stamped = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(stamped, "no heartbeat while the check ran");
    let settled = harness.candidate_settled(&id).await;
    assert_eq!(settled["evidence"]["status"], "reviewed", "{settled}");
    assert!(settled["evidence"]["stage_checkpoint_at"].is_null());
}
