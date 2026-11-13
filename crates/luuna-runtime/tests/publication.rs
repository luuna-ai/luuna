//! M7 publication through the authenticated API with the synthetic Pi fixture,
//! a local bare repository standing in for the GitHub remote and a fake GitHub
//! REST server on a loopback port. No model, no network, no real GitHub.
//!
//! The registered identity is a GitHub-shaped remote
//! (`https://github.com/luuna-test/fixture.git`), so owner/repo parsing and the
//! identity check are the real ones; the developer checkout carries a
//! `url.<bare path>.pushInsteadOf` rewrite, as a developer's own transport
//! rewrite would, so Git's effective push URL is the local bare repository.
mod support;

use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
    Router,
};
use luuna_runtime::{router, AppState, PublicationSettings, RuntimeConfig};
use serde_json::{json, Value};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicU32, Ordering},
    time::Duration,
};
use support::{assert_no_merge_requests, start_fake_github, Fake};
use tower::ServiceExt;

const TOKEN: &str = "synthetic-test-token-with-at-least-32-bytes";
const REMOTE: &str = "https://github.com/luuna-test/fixture.git";

struct Harness {
    app: Router,
    state: AppState,
    root: PathBuf,
    fake: Fake,
    /// The manifest's `max_task_repairs`, set before the project is confirmed.
    max_task_repairs: AtomicU32,
}

impl Harness {
    async fn new() -> Self {
        let script = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/pi-rpc/mock_pi.py")
            .canonicalize()
            .unwrap();
        let (fake, api) = start_fake_github().await;
        let root =
            std::env::temp_dir().join(format!("luuna-publication-test-{}", uuid::Uuid::new_v4()));
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
        .unwrap()
        .with_publication(PublicationSettings {
            github_api_url: api,
            github_token: Some("synthetic-github-token".into()),
            request_timeout: Duration::from_secs(1),
            observe_timeout: Duration::from_secs(5),
            git_timeout: Duration::from_secs(60),
        });
        Self {
            app: router(state.clone()),
            state,
            root,
            fake,
            max_task_repairs: AtomicU32::new(2),
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
    async fn confirm(&self, revision: u64, checks: Vec<Value>) -> Value {
        let manifest = json!({
            "schema_version": "0.1", "id": "fixture", "name": "Fixture", "revision": revision + 1,
            "repositories": [{"id": "app", "remote": REMOTE, "default_branch": "main",
                "profiles": ["python"], "execution": "native", "checks": checks}],
            "relationships": [],
            "policy": {"approval": "plan", "testing_prerequisite": true, "publish": "automatic_pr", "merge": "human",
                "max_explorers": 1, "max_implementers": 1, "max_task_repairs": self.max_task_repairs.load(Ordering::Relaxed)},
        });
        let (status, view) = self
            .request(
                "PUT",
                "/api/v1/projects/fixture/manifest",
                Some(json!({"expected_revision": revision, "manifest": manifest})),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{view}");
        view
    }
    async fn baseline(&self) {
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
    async fn ticket(&self, scenario: &str) -> Value {
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
        ticket
    }
    async fn plan(&self, ticket_id: &str) -> Value {
        let (status, opened) = self
            .request(
                "POST",
                &format!("/api/v1/tickets/{ticket_id}/plans"),
                Some(json!({
                    "task": {"repository_id": "app", "paths": ["src"], "operations": ["read", "write"]},
                    "budgets": {},
                })),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED, "{opened}");
        opened
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
    async fn candidate(&self, run_id: &str) -> Value {
        let (status, body) = self
            .request("GET", &format!("/api/v1/runs/{run_id}/candidate"), None)
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        body
    }
    async fn candidate_settled(&self, run_id: &str) -> Value {
        for _ in 0..600 {
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
    /// Register, confirm, baseline, ticket, plan, approve, implement: a workflow
    /// run whose candidate is recorded and still pending validation.
    async fn implemented(&self, repo: &Path) -> (String, Value) {
        self.implemented_with(repo, "write_in_scope").await
    }
    async fn implemented_with(&self, repo: &Path, scenario: &str) -> (String, Value) {
        clean_repository(repo);
        let (status, view) = self
            .request(
                "POST",
                "/api/v1/projects",
                Some(json!({"id":"fixture","name":"Fixture","repositories":[{"id":"app","path":repo.to_string_lossy()}]})),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED, "{view}");
        self.confirm(0, vec![passing_check()]).await;
        self.baseline().await;
        let ticket = self.ticket(scenario).await;
        let opened = self.plan(ticket["id"].as_str().unwrap()).await;
        self.approve(&opened["plan"]).await;
        let run_id = opened["run"]["id"].as_str().unwrap().to_owned();
        let (status, run) = self
            .request(
                "POST",
                &format!("/api/v1/runs/{run_id}/implement"),
                Some(json!({"provider": "test", "model": "synthetic"})),
            )
            .await;
        assert_eq!(status, StatusCode::ACCEPTED, "{run}");
        assert_eq!(self.settled(&run_id).await["state"], "validating");
        (run_id, ticket)
    }
    /// A reviewed candidate with a passed review: the publication precondition.
    async fn reviewed(&self, repo: &Path) -> (String, Value, Value) {
        self.reviewed_with(repo, "write_in_scope").await
    }
    async fn reviewed_with(&self, repo: &Path, scenario: &str) -> (String, Value, Value) {
        let (run_id, ticket) = self.implemented_with(repo, scenario).await;
        let pending = self.candidate(&run_id).await;
        let hash = pending["evidence"]["candidate"]["hash"].clone();
        let (status, response) = self
            .request(
                "POST",
                &format!("/api/v1/runs/{run_id}/validate"),
                Some(json!({"candidate_hash":hash,"provider":"test","model":"synthetic"})),
            )
            .await;
        assert_eq!(status, StatusCode::ACCEPTED, "{response}");
        let reviewed = self.candidate_settled(&run_id).await;
        assert_eq!(reviewed["evidence"]["status"], "reviewed", "{reviewed}");
        assert_eq!(reviewed["evidence"]["review"]["verdict"], "passed");
        (run_id, ticket, reviewed["evidence"]["candidate"].clone())
    }
    async fn publish(&self, run_id: &str, hash: &Value, key: &str) -> (StatusCode, Value) {
        self.request(
            "POST",
            &format!("/api/v1/runs/{run_id}/publish"),
            Some(json!({"expected_hash": hash, "idempotency_key": key})),
        )
        .await
    }
    async fn publication(&self, run_id: &str) -> (StatusCode, Value) {
        self.request("GET", &format!("/api/v1/runs/{run_id}/publication"), None)
            .await
    }
    async fn publication_settled(&self, run_id: &str) -> Value {
        for _ in 0..1200 {
            let (status, record) = self.publication(run_id).await;
            assert_eq!(status, StatusCode::OK, "{record}");
            if record["active"] == false
                && !matches!(
                    record["publication_state"].as_str(),
                    Some("pending" | "pushing")
                )
            {
                return record;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!(
            "publication did not settle: {}",
            self.publication(run_id).await.1
        );
    }
    async fn worktree(&self, run_id: &str) -> PathBuf {
        let (_, leases) = self
            .request("GET", &format!("/api/v1/runs/{run_id}/worktrees"), None)
            .await;
        PathBuf::from(
            leases.as_array().unwrap().last().unwrap()["path"]
                .as_str()
                .unwrap(),
        )
    }
    async fn reconcile(&self, run_id: &str) -> (StatusCode, Value) {
        self.request(
            "POST",
            &format!("/api/v1/runs/{run_id}/publication/reconcile"),
            None,
        )
        .await
    }
    fn requests(&self, method: &str, path_part: &str) -> usize {
        self.fake.lock().unwrap().count(method, path_part)
    }
    async fn repair_publication(&self, run_id: &str, hash: &Value) -> (StatusCode, Value) {
        self.request(
            "POST",
            &format!("/api/v1/runs/{run_id}/publication/repair"),
            Some(json!({"candidate_hash": hash, "provider": "test", "model": "synthetic"})),
        )
        .await
    }
    async fn validate(&self, run_id: &str, hash: &Value) -> (StatusCode, Value) {
        self.request(
            "POST",
            &format!("/api/v1/runs/{run_id}/validate"),
            Some(json!({"candidate_hash": hash, "provider": "test", "model": "synthetic"})),
        )
        .await
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        self.state.cancel_active();
        let _ = fs::remove_dir_all(&self.root);
    }
}

/// A developer checkout whose sibling bare "origin" is removed with it.
struct Repo(tempfile::TempDir);
impl Repo {
    fn new() -> Self {
        Self(tempfile::tempdir().unwrap())
    }
    fn path(&self) -> &Path {
        self.0.path()
    }
}
impl Drop for Repo {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(bare_path(self.0.path()));
    }
}

fn python() -> String {
    if cfg!(windows) { "python" } else { "python3" }.into()
}
fn passing_check() -> Value {
    json!({"id": "unit", "executable": python(), "args": ["-c", "print('running 1 tests'); print('test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured')"], "required": true,
        "timeout_seconds": 30, "capabilities": ["python"], "cwd": "."})
}
fn git(dir: &Path, args: &[&str]) -> String {
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
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}
fn git_status(dir: &Path, args: &[&str]) -> bool {
    Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .unwrap()
        .status
        .success()
}
/// The developer checkout with a GitHub-shaped origin, plus the local bare
/// repository that Git's push URL rewrite makes the effective remote.
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
    git(dir, &["remote", "add", "origin", REMOTE]);
    let bare = bare_path(dir);
    git(dir, &["init", "-q", "--bare", "-b", "main", &bare]);
    git(dir, &["push", "-q", &bare, "main:refs/heads/main"]);
    git(
        dir,
        &["config", &format!("url.{bare}.pushInsteadOf"), REMOTE],
    );
}
fn bare_path(dir: &Path) -> String {
    dir.parent()
        .unwrap()
        .join(format!(
            "{}-origin.git",
            dir.file_name().unwrap().to_string_lossy()
        ))
        .to_string_lossy()
        .replace('\\', "/")
}
fn remote_branch(dir: &Path, branch: &str) -> Option<String> {
    let bare = bare_path(dir);
    let output = Command::new("git")
        .args(["-C", &bare, "rev-parse", "--verify", "--quiet"])
        .arg(format!("refs/heads/{branch}"))
        .output()
        .unwrap();
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}
fn event_types(events: &[Value]) -> Vec<String> {
    events
        .iter()
        .map(|e| e["type"].as_str().unwrap().to_owned())
        .collect()
}

#[tokio::test]
async fn publish_is_refused_before_a_passed_review_and_after_a_stale_hash() {
    let harness = Harness::new().await;
    let repo = Repo::new();
    let (run_id, _ticket) = harness.implemented(repo.path()).await;
    let (status, body) = harness.publication(&run_id).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert!(body["error"].as_str().unwrap().contains("POST /publish"));
    let pending = harness.candidate(&run_id).await;
    let hash = pending["evidence"]["candidate"]["hash"].clone();
    let (status, refused) = harness.publish(&run_id, &hash, "key-1").await;
    assert_eq!(status, StatusCode::CONFLICT, "{refused}");
    let reason = refused["error"].as_str().unwrap();
    assert!(reason.contains("candidate is pending"), "{reason}");
    assert!(reason.contains("validate the candidate"), "{reason}");
    assert_eq!(harness.requests("POST", "/pulls"), 0);
    assert!(remote_branch(repo.path(), "luuna").is_none());

    let (status, response) = harness
        .request(
            "POST",
            &format!("/api/v1/runs/{run_id}/validate"),
            Some(json!({"candidate_hash":hash,"provider":"test","model":"synthetic"})),
        )
        .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{response}");
    let reviewed = harness.candidate_settled(&run_id).await;
    assert_eq!(reviewed["evidence"]["status"], "reviewed", "{reviewed}");
    let stale = json!(format!("sha256:{}", "0".repeat(64)));
    let (status, refused) = harness.publish(&run_id, &stale, "key-2").await;
    assert_eq!(status, StatusCode::CONFLICT, "{refused}");
    assert!(refused["error"]
        .as_str()
        .unwrap()
        .contains("candidate hash changed"));
    let (status, refused) = harness
        .request(
            "POST",
            &format!("/api/v1/runs/{run_id}/publish"),
            Some(json!({"expected_hash": hash, "idempotency_key": ""})),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");
    assert_eq!(harness.requests("POST", "/pulls"), 0);
    let events = event_types(&harness.events(&run_id).await);
    assert!(
        !events.iter().any(|e| e.starts_with("publication.")),
        "{events:?}"
    );
}

#[tokio::test]
async fn publication_does_not_reuse_a_configured_transport_remote_or_change_local_refs() {
    let harness = Harness::new().await;
    let repo = Repo::new();
    let (run_id, _, candidate) = harness.reviewed(repo.path()).await;
    let commit = candidate["repository_commits"]["app"].as_str().unwrap();
    // Both an ordinary remote and one with only a fetch setting must be
    // avoided when selecting the command-local transport name. Reusing either
    // would import user settings into publication and write local refs.
    git(
        repo.path(),
        &["remote", "add", "luuna-publication-0", REMOTE],
    );
    git(
        repo.path(),
        &["config", "remote.luuna-publication-0.mirror", "true"],
    );
    git(
        repo.path(),
        &[
            "config",
            "remote.luuna-publication-1.fetch",
            "+refs/heads/*:refs/remotes/transport/*",
        ],
    );
    git(
        repo.path(),
        &["update-ref", "refs/remotes/origin/preserved", commit],
    );
    let refs_before = git(repo.path(), &["for-each-ref"]);
    let config_before = fs::read(repo.path().join(".git/config")).unwrap();

    let (status, accepted) = harness
        .publish(&run_id, &candidate["hash"], "isolated-push")
        .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{accepted}");
    let record = harness.publication_settled(&run_id).await;
    assert_eq!(record["publication_state"], "pr_open", "{record}");
    let branch = record["repositories"]["app"]["branch"].as_str().unwrap();
    assert_eq!(remote_branch(repo.path(), branch).as_deref(), Some(commit));
    assert_eq!(git(repo.path(), &["for-each-ref"]), refs_before);
    assert_eq!(
        fs::read(repo.path().join(".git/config")).unwrap(),
        config_before
    );
    assert_eq!(harness.requests("POST", "/pulls"), 1);
}

#[tokio::test]
async fn publication_opens_exactly_one_pull_request_at_the_exact_candidate_commit() {
    let harness = Harness::new().await;
    let repo = Repo::new();
    let (run_id, ticket, candidate) = harness.reviewed(repo.path()).await;
    let hash = candidate["hash"].clone();
    let commit = candidate["repository_commits"]["app"].as_str().unwrap();
    let tracking_before = git(repo.path(), &["for-each-ref", "refs/remotes"]);

    let (status, accepted) = harness.publish(&run_id, &hash, "publish-1").await;
    assert_eq!(status, StatusCode::ACCEPTED, "{accepted}");
    assert_eq!(accepted["publication_state"], "pending");
    assert_eq!(accepted["active"], true);
    let record = harness.publication_settled(&run_id).await;
    assert_eq!(record["publication_state"], "pr_open", "{record}");
    let branch = record["repositories"]["app"]["branch"].as_str().unwrap();
    assert_eq!(
        branch,
        format!(
            "luuna/{}/{}",
            ticket["id"].as_str().unwrap(),
            &hash.as_str().unwrap()[7..19]
        )
    );
    assert_eq!(remote_branch(repo.path(), branch).as_deref(), Some(commit));
    assert_eq!(record["repositories"]["app"]["push"], "done");
    assert_eq!(record["repositories"]["app"]["pull_request"]["number"], 1);
    assert_eq!(
        record["repositories"]["app"]["pull_request"]["url"],
        "https://github.com/luuna-test/fixture/pull/1"
    );
    assert_eq!(record["contract"]["publication_state"], "pr_open");
    assert_eq!(record["contract"]["per_repository_branch"]["app"], branch);
    assert_eq!(record["contract"]["candidate_hash"], hash);
    assert_eq!(record["contract"]["ticket_id"], ticket["id"]);
    assert_eq!(harness.requests("POST", "/pulls"), 1);
    {
        let fake = harness.fake.lock().unwrap();
        let pr = &fake.pull_requests[0];
        assert_eq!(pr["head"]["ref"], branch);
        assert_eq!(pr["base"]["ref"], "main");
        let body = pr["body"].as_str().unwrap();
        assert!(body.contains(hash.as_str().unwrap()), "{body}");
        assert!(body.contains(ticket["id"].as_str().unwrap()), "{body}");
        assert!(body.contains("1 of 1 passed"), "{body}");
        assert!(body.contains("passed by `review-"), "{body}");
        assert!(!body.contains("synthetic-github-token"));
    }
    // Publication must not write remote-tracking refs, even when Git recognizes
    // the effective push URL as origin's URL.
    assert_eq!(
        git(repo.path(), &["for-each-ref", "refs/remotes"]),
        tracking_before
    );
    let events = event_types(&harness.events(&run_id).await);
    for expected in [
        "publication.requested",
        "publication.pushed",
        "publication.pr_opened",
    ] {
        assert!(events.contains(&expected.to_owned()), "{events:?}");
    }
    assert!(
        !events.contains(&"publication.failed".to_owned()),
        "{events:?}"
    );

    // The same idempotency key replays the record without side effects.
    let (status, replay) = harness.publish(&run_id, &hash, "publish-1").await;
    assert_eq!(status, StatusCode::OK, "{replay}");
    assert_eq!(replay["publication_state"], "pr_open");
    assert_eq!(replay["idempotency_key"], "publish-1");
    assert_eq!(harness.requests("POST", "/pulls"), 1);
    assert_eq!(
        event_types(&harness.events(&run_id).await)
            .iter()
            .filter(|e| *e == "publication.pushed")
            .count(),
        1
    );
    // Another key for the same candidate is refused, not republished.
    let (status, refused) = harness.publish(&run_id, &hash, "publish-2").await;
    assert_eq!(status, StatusCode::CONFLICT, "{refused}");
    assert_eq!(harness.requests("POST", "/pulls"), 1);
    // Reconcile has nothing to do.
    let (status, same) = harness
        .request(
            "POST",
            &format!("/api/v1/runs/{run_id}/publication/reconcile"),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{same}");
    assert_eq!(harness.requests("POST", "/pulls"), 1);
    assert_eq!(remote_branch(repo.path(), branch).as_deref(), Some(commit));
}

#[tokio::test]
async fn a_timed_out_pull_request_creation_is_reconciled_without_a_duplicate() {
    let harness = Harness::new().await;
    let repo = Repo::new();
    let (run_id, _ticket, candidate) = harness.reviewed(repo.path()).await;
    let hash = candidate["hash"].clone();
    harness.fake.lock().unwrap().delay_create = Some(Duration::from_secs(3));
    let (status, accepted) = harness.publish(&run_id, &hash, "publish-timeout").await;
    assert_eq!(status, StatusCode::ACCEPTED, "{accepted}");
    let failed = harness.publication_settled(&run_id).await;
    assert_eq!(failed["publication_state"], "failed", "{failed}");
    assert_eq!(failed["repositories"]["app"]["push"], "done");
    assert_eq!(failed["repositories"]["app"]["pull_request_step"], "failed");
    let reason = failed["repositories"]["app"]["reason"].as_str().unwrap();
    assert!(reason.contains("deadline"), "{reason}");
    assert!(reason.contains("reconcile"), "{reason}");
    // The remote recorded the pull request before it answered.
    assert_eq!(harness.requests("POST", "/pulls"), 1);
    assert_eq!(harness.fake.lock().unwrap().pull_requests.len(), 1);
    let events = event_types(&harness.events(&run_id).await);
    assert!(
        events.contains(&"publication.failed".to_owned()),
        "{events:?}"
    );

    harness.fake.lock().unwrap().delay_create = None;
    let (status, accepted) = harness
        .request(
            "POST",
            &format!("/api/v1/runs/{run_id}/publication/reconcile"),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{accepted}");
    let reconciled = harness.publication_settled(&run_id).await;
    assert_eq!(reconciled["publication_state"], "pr_open", "{reconciled}");
    assert_eq!(
        reconciled["repositories"]["app"]["pull_request"]["number"],
        1
    );
    assert_eq!(
        harness.requests("POST", "/pulls"),
        1,
        "no duplicate creation"
    );
    assert_eq!(harness.fake.lock().unwrap().pull_requests.len(), 1);
    let events = harness.events(&run_id).await;
    let opened: Vec<&Value> = events
        .iter()
        .filter(|e| e["type"] == "publication.pr_opened")
        .collect();
    assert_eq!(opened.len(), 1, "{events:?}");
    assert_eq!(opened[0]["payload"]["reused_existing"], true);
    assert!(events.iter().any(|e| e["type"] == "publication.reconciled"));
    assert_eq!(
        events
            .iter()
            .filter(|e| e["type"] == "publication.pushed")
            .count(),
        1,
        "the push was not repeated"
    );
}

#[tokio::test]
async fn a_failed_pull_request_creation_is_retried_by_reconcile_without_a_second_push() {
    let harness = Harness::new().await;
    let repo = Repo::new();
    let (run_id, _ticket, candidate) = harness.reviewed(repo.path()).await;
    let hash = candidate["hash"].clone();
    let commit = candidate["repository_commits"]["app"].as_str().unwrap();
    harness
        .fake
        .lock()
        .unwrap()
        .fail_create_once
        .insert("fixture".into());
    let (status, accepted) = harness.publish(&run_id, &hash, "publish-fail").await;
    assert_eq!(status, StatusCode::ACCEPTED, "{accepted}");
    let failed = harness.publication_settled(&run_id).await;
    assert_eq!(failed["publication_state"], "failed", "{failed}");
    assert_eq!(failed["repositories"]["app"]["push"], "done");
    assert_eq!(failed["repositories"]["app"]["pull_request_step"], "failed");
    assert!(failed["repositories"]["app"]["reason"]
        .as_str()
        .unwrap()
        .contains("500"));
    assert_eq!(failed["contract"]["publication_state"], "failed");
    assert_eq!(failed["contract"]["pull_request_urls"], json!({}));
    let branch = failed["repositories"]["app"]["branch"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(remote_branch(repo.path(), &branch).as_deref(), Some(commit));

    let (status, accepted) = harness
        .request(
            "POST",
            &format!("/api/v1/runs/{run_id}/publication/reconcile"),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{accepted}");
    let reconciled = harness.publication_settled(&run_id).await;
    assert_eq!(reconciled["publication_state"], "pr_open", "{reconciled}");
    assert_eq!(harness.requests("POST", "/pulls"), 2);
    assert_eq!(harness.fake.lock().unwrap().pull_requests.len(), 1);
    let events = harness.events(&run_id).await;
    assert_eq!(
        events
            .iter()
            .filter(|e| e["type"] == "publication.pushed")
            .count(),
        1,
        "only the pull request step was retried: {:?}",
        event_types(&events)
    );
    assert_eq!(
        events
            .iter()
            .filter(|e| e["type"] == "publication.pr_opened")
            .count(),
        1
    );
    assert_eq!(remote_branch(repo.path(), &branch).as_deref(), Some(commit));
}

#[tokio::test]
async fn ci_failure_is_distinct_from_integration_and_luuna_never_merges() {
    let harness = Harness::new().await;
    let repo = Repo::new();
    let (run_id, _ticket, candidate) = harness.reviewed(repo.path()).await;
    let hash = candidate["hash"].clone();
    let (status, _) = harness.publish(&run_id, &hash, "publish-ci").await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let open = harness.publication_settled(&run_id).await;
    assert_eq!(open["publication_state"], "pr_open", "{open}");
    assert_eq!(open["repositories"]["app"]["ci"]["state"], "unknown");
    assert!(open["repositories"]["app"]["ci"]["reason"]
        .as_str()
        .unwrap()
        .contains("no checks"));

    harness.fake.lock().unwrap().check_runs.insert(
        "fixture".into(),
        vec![
            json!({"name": "unit", "status": "completed", "conclusion": "failure"}),
            json!({"name": "lint", "status": "completed", "conclusion": "success"}),
        ],
    );
    let (status, observed) = harness.publication(&run_id).await;
    assert_eq!(status, StatusCode::OK, "{observed}");
    let ci = &observed["repositories"]["app"]["ci"];
    assert_eq!(ci["state"], "failure", "{observed}");
    assert_eq!(ci["checks"][0]["name"], "unit");
    assert_eq!(ci["checks"][0]["conclusion"], "failure");
    assert_eq!(ci["checks"][1]["conclusion"], "success");
    assert!(ci["observed_at"].is_string());
    assert_eq!(observed["publication_state"], "pr_open");
    assert_eq!(observed["repositories"]["app"]["merge"]["merged"], false);
    // Observed CI never changes candidate evidence.
    let candidate_after = harness.candidate(&run_id).await;
    assert_eq!(candidate_after["evidence"]["status"], "reviewed");
    assert_eq!(candidate_after["evidence"]["candidate"], candidate);
    assert_eq!(harness.run(&run_id).await["state"], "reviewed");

    harness.fake.lock().unwrap().check_runs.insert(
        "fixture".into(),
        vec![json!({"name": "unit", "status": "in_progress", "conclusion": null})],
    );
    let (_, pending) = harness.publication(&run_id).await;
    assert_eq!(pending["repositories"]["app"]["ci"]["state"], "pending");

    // A person merges on GitHub; Luuna observes it and never calls merge.
    harness.fake.lock().unwrap().merged.insert("fixture".into());
    let (status, integrated) = harness.publication(&run_id).await;
    assert_eq!(status, StatusCode::OK, "{integrated}");
    assert_eq!(
        integrated["publication_state"], "integrated",
        "{integrated}"
    );
    assert_eq!(integrated["contract"]["publication_state"], "integrated");
    let merge = &integrated["repositories"]["app"]["merge"];
    assert_eq!(merge["merged"], true);
    assert_eq!(merge["merged_by"], "a-human");
    assert_eq!(merge["merged_at"], "2026-10-21T12:00:00Z");
    let events = event_types(&harness.events(&run_id).await);
    assert_eq!(
        events
            .iter()
            .filter(|e| *e == "publication.integrated")
            .count(),
        1,
        "{events:?}"
    );
    let (_, again) = harness.publication(&run_id).await;
    assert_eq!(again["publication_state"], "integrated");
    assert_eq!(
        event_types(&harness.events(&run_id).await)
            .iter()
            .filter(|e| *e == "publication.integrated")
            .count(),
        1,
        "integration is recorded once"
    );
    assert_no_merge_requests(&harness.fake);
    let requests = harness.fake.lock().unwrap().requests.clone();
    assert!(requests.iter().any(|(m, _)| m == "POST"), "{requests:?}");
}

#[tokio::test]
async fn there_is_no_merge_route_or_operation() {
    let source = include_str!("../src/lib.rs");
    let router = source
        .split("pub fn router(")
        .nth(1)
        .expect("router block")
        .split("fn health(")
        .next()
        .unwrap();
    // Every route path is a string literal in the router block; none names a
    // merge (axum's own `Router::merge` composes routers and is not a route).
    let paths: Vec<&str> = router
        .split('"')
        .skip(1)
        .step_by(2)
        .filter(|literal| literal.starts_with('/'))
        .collect();
    assert!(paths.len() > 20, "router paths: {paths:?}");
    assert!(
        paths
            .iter()
            .all(|path| !path.to_ascii_lowercase().contains("merge")),
        "the router must not expose a merge route: {paths:?}"
    );
    assert!(!router.contains("merge\""));
    let publication = include_str!("../src/publication.rs").to_ascii_lowercase();
    for forbidden in [
        "/merge\"",
        "pulls/{}/merge",
        ".put(",
        "method::put",
        "method::delete",
        "\"state\": \"closed\"",
    ] {
        assert!(
            !publication.contains(forbidden),
            "publication must not perform a merge ({forbidden})"
        );
    }
    let harness = Harness::new().await;
    for path in [
        "/api/v1/runs/run-1/publication/merge",
        "/api/v1/runs/run-1/merge",
        "/api/v1/publications/run-1/merge",
    ] {
        let (status, body) = harness.request("POST", path, Some(json!({}))).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{path}: {body}");
    }
    assert!(git_status(
        Path::new(env!("CARGO_MANIFEST_DIR")),
        &["--version"]
    ));
}

/// The `publication.refused` events of a run, by kind.
fn refusals(events: &[Value]) -> Vec<String> {
    events
        .iter()
        .filter(|e| e["type"] == "publication.refused")
        .map(|e| e["payload"]["kind"].as_str().unwrap_or_default().to_owned())
        .collect()
}

#[tokio::test]
async fn a_newer_plan_revision_supersedes_the_reviewed_candidates_authorization() {
    let harness = Harness::new().await;
    let repo = Repo::new();
    let (run_id, ticket, candidate) = harness.reviewed(repo.path()).await;
    let hash = candidate["hash"].clone();
    // The developer plans again: revision 2 of the same plan opens its own
    // workflow and supersedes the approval revision 1's candidate was made under.
    let second = harness.plan(ticket["id"].as_str().unwrap()).await;
    assert_eq!(second["plan"]["revision"], 2, "{second}");

    let (status, refused) = harness.publish(&run_id, &hash, "publish-superseded").await;
    assert_eq!(status, StatusCode::CONFLICT, "{refused}");
    let reason = refused["error"].as_str().unwrap();
    assert!(
        reason.contains("newer plan revision supersedes"),
        "{reason}"
    );
    assert!(reason.contains("re-plan"), "{reason}");
    assert_eq!(harness.requests("POST", "/pulls"), 0);
    assert_eq!(harness.requests("GET", "/pulls"), 0);
    assert!(remote_branch(repo.path(), "luuna").is_none());
    let (status, body) = harness.publication(&run_id).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    let events = event_types(&harness.events(&run_id).await);
    assert!(
        !events
            .iter()
            .any(|e| e == "publication.requested" || e == "publication.pushed"),
        "{events:?}"
    );
    // The candidate evidence itself is untouched: it is valid for revision 1,
    // which simply is no longer the authorized revision.
    let view = harness.candidate(&run_id).await;
    assert_eq!(view["evidence"]["status"], "reviewed");
    assert_eq!(view["evidence"]["candidate"]["hash"], hash);
    assert_eq!(harness.run(&run_id).await["state"], "reviewed");
}

#[tokio::test]
async fn reconciliation_is_refused_once_a_newer_plan_revision_exists() {
    let harness = Harness::new().await;
    let repo = Repo::new();
    let (run_id, ticket, candidate) = harness.reviewed(repo.path()).await;
    let hash = candidate["hash"].clone();
    let commit = candidate["repository_commits"]["app"].as_str().unwrap();
    harness
        .fake
        .lock()
        .unwrap()
        .fail_create_once
        .insert("fixture".into());
    let (status, _) = harness.publish(&run_id, &hash, "publish-then-replan").await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let failed = harness.publication_settled(&run_id).await;
    assert_eq!(failed["publication_state"], "failed", "{failed}");
    assert_eq!(failed["repositories"]["app"]["push"], "done");
    let branch = failed["repositories"]["app"]["branch"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(remote_branch(repo.path(), &branch).as_deref(), Some(commit));

    let second = harness.plan(ticket["id"].as_str().unwrap()).await;
    assert_eq!(second["plan"]["revision"], 2, "{second}");
    let (status, refused) = harness.reconcile(&run_id).await;
    assert_eq!(status, StatusCode::CONFLICT, "{refused}");
    assert!(refused["error"]
        .as_str()
        .unwrap()
        .contains("newer plan revision supersedes"));
    // The pull request was never created: the ledger shows one failed
    // attempt and reconciliation did not retry it under superseded authority.
    assert_eq!(harness.requests("POST", "/pulls"), 1);
    assert_eq!(harness.fake.lock().unwrap().pull_requests.len(), 0);
    let (status, record) = harness.publication(&run_id).await;
    assert_eq!(status, StatusCode::OK, "{record}");
    assert_eq!(record["publication_state"], "failed");
    assert_eq!(record["repositories"]["app"]["pull_request_step"], "failed");
    // The branch that was pushed before the re-plan is left as it was.
    assert_eq!(remote_branch(repo.path(), &branch).as_deref(), Some(commit));
}

#[tokio::test]
async fn manual_worktree_changes_after_the_review_invalidate_the_candidate_instead_of_publishing() {
    let harness = Harness::new().await;
    let repo = Repo::new();
    let (run_id, _ticket, candidate) = harness.reviewed(repo.path()).await;
    let hash = candidate["hash"].clone();
    let worktree = harness.worktree(&run_id).await;
    assert!(
        worktree.join("src/main.py").is_file(),
        "{}",
        worktree.display()
    );
    fs::write(
        worktree.join("src/main.py"),
        "print('edited by hand after the review')\n",
    )
    .unwrap();

    let (status, refused) = harness.publish(&run_id, &hash, "publish-drift").await;
    assert_eq!(status, StatusCode::CONFLICT, "{refused}");
    let reason = refused["error"].as_str().unwrap();
    assert!(reason.starts_with("candidate content drift:"), "{reason}");
    assert!(reason.contains("src/main.py"), "{reason}");
    assert!(reason.contains("nothing was pushed"), "{reason}");
    assert!(reason.contains("POST /repair"), "{reason}");
    assert_eq!(harness.requests("POST", "/pulls"), 0);
    assert_eq!(harness.requests("GET", "/pulls"), 0);
    assert!(remote_branch(repo.path(), "luuna").is_none());
    let (status, body) = harness.publication(&run_id).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");

    // The reviewed evidence is invalidated: the candidate is failed with the
    // drift marked, the run left `reviewed`, and the review is history only.
    let view = harness.candidate(&run_id).await;
    assert_eq!(view["evidence"]["status"], "failed", "{view}");
    assert_eq!(view["evidence"]["content_drift"], true);
    assert_eq!(view["evidence"]["candidate"]["hash"], hash);
    assert!(view["evidence"]["reason"]
        .as_str()
        .unwrap()
        .starts_with("candidate content drift:"));
    assert_eq!(view["revalidate_allowed"], false, "{view}");
    assert_eq!(harness.run(&run_id).await["state"], "failed");
    let events = harness.events(&run_id).await;
    assert!(events.iter().any(|e| e["type"] == "candidate.invalidated"
        && e["payload"]["content_drift"] == true
        && e["payload"]["candidate_hash"] == hash));
    assert_eq!(refusals(&events), vec!["candidate_drift"]);
    assert!(!event_types(&events).contains(&"publication.requested".to_owned()));
    // The edit is preserved for inspection; the developer checkout is untouched.
    assert_eq!(
        fs::read_to_string(worktree.join("src/main.py")).unwrap(),
        "print('edited by hand after the review')\n"
    );
    assert_eq!(
        fs::read_to_string(repo.path().join("src/main.py")).unwrap(),
        "print('fixture')\n"
    );
    // A second attempt meets the invalidated candidate, not the drift check again.
    let (status, again) = harness.publish(&run_id, &hash, "publish-drift-2").await;
    assert_eq!(status, StatusCode::CONFLICT, "{again}");
    assert!(again["error"]
        .as_str()
        .unwrap()
        .contains("candidate is failed"));
    assert_eq!(refusals(&harness.events(&run_id).await).len(), 1);
}

#[tokio::test]
async fn reconciliation_revalidates_the_worktree_before_retrying_a_step() {
    let harness = Harness::new().await;
    let repo = Repo::new();
    let (run_id, _ticket, candidate) = harness.reviewed(repo.path()).await;
    let hash = candidate["hash"].clone();
    harness
        .fake
        .lock()
        .unwrap()
        .fail_create_once
        .insert("fixture".into());
    let (status, _) = harness.publish(&run_id, &hash, "publish-then-drift").await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let failed = harness.publication_settled(&run_id).await;
    assert_eq!(failed["publication_state"], "failed", "{failed}");
    let worktree = harness.worktree(&run_id).await;
    fs::write(worktree.join("src/main.py"), "print('drifted')\n").unwrap();

    let (status, refused) = harness.reconcile(&run_id).await;
    assert_eq!(status, StatusCode::CONFLICT, "{refused}");
    assert!(refused["error"]
        .as_str()
        .unwrap()
        .starts_with("candidate content drift:"));
    assert_eq!(harness.requests("POST", "/pulls"), 1, "no retry");
    assert_eq!(harness.fake.lock().unwrap().pull_requests.len(), 0);
    let view = harness.candidate(&run_id).await;
    assert_eq!(view["evidence"]["status"], "failed", "{view}");
    assert_eq!(view["evidence"]["content_drift"], true);
    assert_eq!(harness.run(&run_id).await["state"], "failed");
    let (status, record) = harness.publication(&run_id).await;
    assert_eq!(status, StatusCode::OK, "{record}");
    assert_eq!(record["publication_state"], "failed");
    assert_eq!(
        refusals(&harness.events(&run_id).await),
        vec!["candidate_drift"]
    );
}

#[tokio::test]
async fn the_pre_push_policy_refuses_credentials_without_naming_their_values() {
    let harness = Harness::new().await;
    let repo = Repo::new();
    let (run_id, _ticket, candidate) = harness.reviewed_with(repo.path(), "write_secret").await;
    let hash = candidate["hash"].clone();
    let (status, refused) = harness.publish(&run_id, &hash, "publish-secret").await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");
    let reason = refused["error"].as_str().unwrap();
    assert!(reason.starts_with("pre-push policy refused"), "{reason}");
    assert!(
        reason.contains("src/config.py [credential-token]"),
        "{reason}"
    );
    assert!(reason.contains("GitHub personal access token"), "{reason}");
    assert!(reason.contains("src/.env [credential-path]"), "{reason}");
    assert!(reason.contains("src/.env [secret-assignment]"), "{reason}");
    assert!(reason.contains("nothing was pushed"), "{reason}");
    assert!(
        !reason.contains("ghp_a1b2"),
        "the value must never be reported: {reason}"
    );
    assert!(
        !reason.contains("Xk9f2mQ8"),
        "the value must never be reported: {reason}"
    );
    assert_eq!(harness.requests("POST", "/pulls"), 0);
    assert_eq!(harness.requests("GET", "/pulls"), 0);
    assert!(remote_branch(repo.path(), "luuna").is_none());
    let (status, body) = harness.publication(&run_id).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    let events = harness.events(&run_id).await;
    assert_eq!(refusals(&events), vec!["policy"]);
    let refusal = events
        .iter()
        .find(|e| e["type"] == "publication.refused")
        .unwrap();
    let findings = refusal["payload"]["findings"].as_array().unwrap();
    assert!(findings.len() >= 3, "{findings:?}");
    assert!(findings
        .iter()
        .any(|f| f["path"] == "src/config.py" && f["rule"] == "credential-token"));
    assert!(findings
        .iter()
        .any(|f| f["path"] == "src/.env" && f["rule"] == "credential-path"));
    let serialized = refusal.to_string();
    assert!(!serialized.contains("ghp_a1b2") && !serialized.contains("Xk9f2mQ8"));
    // The reviewed evidence is invalidated (not as drift) so the bounded
    // repair path is open: the candidate is failed, the run left `reviewed`.
    let view = harness.candidate(&run_id).await;
    assert_eq!(view["evidence"]["status"], "failed", "{view}");
    assert_ne!(view["evidence"]["content_drift"], true);
    assert!(view["evidence"]["reason"]
        .as_str()
        .unwrap()
        .starts_with("pre-push policy refused"));
    assert_eq!(view["repair_allowed"], true, "{view}");
    assert_eq!(harness.run(&run_id).await["state"], "failed");
    assert!(events
        .iter()
        .any(|e| e["type"] == "candidate.invalidated" && e["payload"]["content_drift"] == false));
    assert_eq!(view["evidence"]["policy_refused"], true, "{view}");
    // A repair is admitted from here: the next revision replaces the finding.
    let (status, repair) = harness
        .request(
            "POST",
            &format!("/api/v1/runs/{run_id}/repair"),
            Some(json!({"candidate_hash": hash, "provider": "test", "model": "synthetic"})),
        )
        .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{repair}");
    // The repair starts history-clean: its commit sits directly on the
    // approved base, so the refused commit is never an ancestor of what is
    // pushed, and the repaired candidate publishes with no credential in any
    // published commit.
    assert_eq!(harness.settled(&run_id).await["state"], "validating");
    let pending = harness.candidate(&run_id).await;
    let repaired = pending["evidence"]["candidate"].clone();
    assert_eq!(repaired["revision"], 2, "{pending}");
    let base = git(repo.path(), &["rev-parse", "HEAD"]);
    let repaired_commit = repaired["repository_commits"]["app"].as_str().unwrap();
    assert_eq!(
        git(repo.path(), &["rev-parse", &format!("{repaired_commit}^")]),
        base,
        "the repaired commit's parent is the approved base, not the refused commit"
    );
    let (status, response) = harness
        .request(
            "POST",
            &format!("/api/v1/runs/{run_id}/validate"),
            Some(json!({"candidate_hash": repaired["hash"], "provider": "test", "model": "synthetic"})),
        )
        .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{response}");
    let reviewed = harness.candidate_settled(&run_id).await;
    assert_eq!(reviewed["evidence"]["status"], "reviewed", "{reviewed}");
    let (status, accepted) = harness
        .publish(&run_id, &repaired["hash"], "publish-repaired")
        .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{accepted}");
    let record = harness.publication_settled(&run_id).await;
    assert_eq!(record["publication_state"], "pr_open", "{record}");
    let branch = record["repositories"]["app"]["branch"].as_str().unwrap();
    let pushed = remote_branch(repo.path(), branch).expect("branch pushed");
    assert_eq!(pushed, repaired_commit);
    let history = Command::new("git")
        .args(["-C", &bare_path(repo.path()), "log", "-p", "--all"])
        .output()
        .unwrap();
    let history = String::from_utf8_lossy(&history.stdout);
    assert!(
        !history.contains("ghp_a1b2") && !history.contains("Xk9f2mQ8"),
        "no published commit carries the credential"
    );
}

/// The pre-push policy scans every commit a push would publish, not only
/// the net patch: a repair that starts from a failed candidate's commit keeps
/// that commit as an ancestor, and a credential it added is pushed with it
/// even though the repaired tree no longer holds the file.
#[tokio::test]
async fn a_credential_in_an_ancestor_commit_is_refused_even_when_the_net_patch_is_clean() {
    let harness = Harness::new().await;
    let repo = Repo::new();
    clean_repository(repo.path());
    let (status, view) = harness
        .request(
            "POST",
            "/api/v1/projects",
            Some(json!({"id":"fixture","name":"Fixture","repositories":[{"id":"app","path":repo.path().to_string_lossy()}]})),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{view}");
    // The check fails while the credential file exists, so the first
    // candidate fails its check (not the policy) and its repair starts from
    // that commit.
    let check = json!({"id": "unit", "executable": python(), "args": ["-c",
        "import os, sys; bad = os.path.exists('src/config.py'); print('running 1 tests'); print('test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured' if bad else 'test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured'); sys.exit(1 if bad else 0)"],
        "required": true, "timeout_seconds": 30, "capabilities": ["python"], "cwd": "."});
    harness.confirm(0, vec![check]).await;
    harness.baseline().await;
    let ticket = harness.ticket("secret_then_repair").await;
    let opened = harness.plan(ticket["id"].as_str().unwrap()).await;
    harness.approve(&opened["plan"]).await;
    let run_id = opened["run"]["id"].as_str().unwrap().to_owned();
    let (status, run) = harness
        .request(
            "POST",
            &format!("/api/v1/runs/{run_id}/implement"),
            Some(json!({"provider": "test", "model": "synthetic"})),
        )
        .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{run}");
    assert_eq!(harness.settled(&run_id).await["state"], "validating");
    let first = harness.candidate(&run_id).await["evidence"]["candidate"].clone();
    let (status, response) = harness
        .request(
            "POST",
            &format!("/api/v1/runs/{run_id}/validate"),
            Some(
                json!({"candidate_hash": first["hash"], "provider": "test", "model": "synthetic"}),
            ),
        )
        .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{response}");
    let failed = harness.candidate_settled(&run_id).await;
    assert_eq!(failed["evidence"]["status"], "failed", "{failed}");
    assert_ne!(failed["evidence"]["policy_refused"], true);
    assert_eq!(failed["repair_allowed"], true, "{failed}");
    let (status, repair) = harness
        .request(
            "POST",
            &format!("/api/v1/runs/{run_id}/repair"),
            Some(
                json!({"candidate_hash": first["hash"], "provider": "test", "model": "synthetic"}),
            ),
        )
        .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{repair}");
    assert_eq!(harness.settled(&run_id).await["state"], "validating");
    let repaired = harness.candidate(&run_id).await["evidence"]["candidate"].clone();
    let repaired_commit = repaired["repository_commits"]["app"].as_str().unwrap();
    // An ordinary repair keeps the failed candidate's commit as its parent.
    assert_eq!(
        git(repo.path(), &["rev-parse", &format!("{repaired_commit}^")]),
        first["repository_commits"]["app"].as_str().unwrap()
    );
    let (status, response) = harness
        .request(
            "POST",
            &format!("/api/v1/runs/{run_id}/validate"),
            Some(json!({"candidate_hash": repaired["hash"], "provider": "test", "model": "synthetic"})),
        )
        .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{response}");
    let reviewed = harness.candidate_settled(&run_id).await;
    assert_eq!(reviewed["evidence"]["status"], "reviewed", "{reviewed}");
    // The net patch is clean; the ancestor is not. Nothing is pushed.
    let (status, refused) = harness
        .publish(&run_id, &repaired["hash"], "publish-ancestor")
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");
    let reason = refused["error"].as_str().unwrap();
    assert!(reason.starts_with("pre-push policy refused"), "{reason}");
    assert!(
        reason.contains("src/config.py [credential-token]"),
        "{reason}"
    );
    assert!(reason.contains("in ancestor commit"), "{reason}");
    assert!(!reason.contains("ghp_a1b2"), "{reason}");
    assert!(remote_branch(repo.path(), "luuna").is_none());
    assert_eq!(harness.requests("POST", "/pulls"), 0);
    let view = harness.candidate(&run_id).await;
    assert_eq!(view["evidence"]["status"], "failed", "{view}");
    assert_eq!(view["evidence"]["policy_refused"], true, "{view}");
}

/// The check runs the fake reports for the fixture repository's pull request head.
fn set_check_runs(harness: &Harness, runs: Vec<Value>) {
    harness
        .fake
        .lock()
        .unwrap()
        .check_runs
        .insert("fixture".into(), runs);
}

#[tokio::test]
async fn a_ci_failure_drives_a_bounded_repair_that_invalidates_the_published_candidate() {
    let harness = Harness::new().await;
    harness.max_task_repairs.store(1, Ordering::Relaxed);
    let repo = Repo::new();
    let (run_id, ticket, candidate) = harness.reviewed_with(repo.path(), "ci_repair").await;
    let hash = candidate["hash"].clone();
    let first_commit = candidate["repository_commits"]["app"]
        .as_str()
        .unwrap()
        .to_owned();
    // Before any publication there is nothing to repair.
    let (status, refused) = harness.repair_publication(&run_id, &hash).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{refused}");
    assert!(refused["error"].as_str().unwrap().contains("POST /publish"));
    let (status, _) = harness.publish(&run_id, &hash, "publish-ci-repair").await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let open = harness.publication_settled(&run_id).await;
    assert_eq!(open["publication_state"], "pr_open", "{open}");
    assert!(open.get("superseded_by_repair").is_none(), "{open}");

    // Unknown (no checks), pending and passing CI each refuse the repair,
    // naming the state and the next step; the evidence is untouched.
    let (status, refused) = harness.repair_publication(&run_id, &hash).await;
    assert_eq!(status, StatusCode::CONFLICT, "{refused}");
    let reason = refused["error"].as_str().unwrap();
    assert!(reason.contains("ci unknown"), "{reason}");
    assert!(reason.contains("GET /publication"), "{reason}");
    set_check_runs(
        &harness,
        vec![json!({"name": "ci-unit", "status": "in_progress", "conclusion": null})],
    );
    let (status, refused) = harness.repair_publication(&run_id, &hash).await;
    assert_eq!(status, StatusCode::CONFLICT, "{refused}");
    let reason = refused["error"].as_str().unwrap();
    assert!(reason.contains("ci pending"), "{reason}");
    assert!(reason.contains("still running"), "{reason}");
    set_check_runs(
        &harness,
        vec![json!({"name": "ci-unit", "status": "completed", "conclusion": "success"})],
    );
    let (status, refused) = harness.repair_publication(&run_id, &hash).await;
    assert_eq!(status, StatusCode::CONFLICT, "{refused}");
    let reason = refused["error"].as_str().unwrap();
    assert!(reason.contains("ci success"), "{reason}");
    assert!(reason.contains("nothing to repair"), "{reason}");
    let stale = json!(format!("sha256:{}", "0".repeat(64)));
    let (status, refused) = harness.repair_publication(&run_id, &stale).await;
    assert_eq!(status, StatusCode::CONFLICT, "{refused}");
    assert!(refused["error"]
        .as_str()
        .unwrap()
        .contains("candidate hash changed"));
    assert_eq!(
        harness.candidate(&run_id).await["evidence"]["status"],
        "reviewed"
    );
    assert_eq!(harness.run(&run_id).await["state"], "reviewed");
    let events = event_types(&harness.events(&run_id).await);
    assert!(
        !events
            .iter()
            .any(|e| e == "publication.repair_requested" || e == "candidate.invalidated"),
        "{events:?}"
    );

    // CI fails on the pull request: the repair is admitted. The observation
    // is refreshed by the route itself (the stale `success` view above does
    // not stand), the reviewed candidate is invalidated naming the check, the
    // publication is superseded and the repair writer runs with the CI
    // failure in its evidence.
    set_check_runs(
        &harness,
        vec![
            json!({"name": "ci-unit", "status": "completed", "conclusion": "failure"}),
            json!({"name": "lint", "status": "completed", "conclusion": "success"}),
        ],
    );
    let (status, accepted) = harness.repair_publication(&run_id, &hash).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{accepted}");
    assert_eq!(accepted["superseded_by_repair"], true, "{accepted}");
    assert_eq!(accepted["publication_state"], "pr_open");
    assert_eq!(accepted["run"]["state"], "implementing", "{accepted}");
    let superseded_reason = accepted["superseded_reason"].as_str().unwrap();
    assert!(
        superseded_reason.contains("ci-unit failure"),
        "{superseded_reason}"
    );
    assert!(
        superseded_reason.contains("pull request #1"),
        "{superseded_reason}"
    );
    assert!(!superseded_reason.contains("lint"), "{superseded_reason}");
    let settled = harness.settled(&run_id).await;
    assert_eq!(settled["state"], "validating", "{settled}");
    let events = harness.events(&run_id).await;
    let invalidated = events
        .iter()
        .find(|e| e["type"] == "candidate.invalidated")
        .unwrap_or_else(|| panic!("{:?}", event_types(&events)));
    assert_eq!(invalidated["payload"]["candidate_hash"], hash);
    assert_eq!(invalidated["payload"]["content_drift"], false);
    let reason = invalidated["payload"]["reason"].as_str().unwrap();
    assert!(
        reason.starts_with("CI failed on the published pull request"),
        "{reason}"
    );
    assert!(
        reason.contains("repository app, pull request #1"),
        "{reason}"
    );
    assert!(reason.contains("ci-unit failure"), "{reason}");
    let requested = events
        .iter()
        .find(|e| e["type"] == "publication.repair_requested")
        .unwrap();
    assert_eq!(requested["payload"]["candidate_hash"], hash);
    assert_eq!(requested["payload"]["candidate_status"], "failed");
    let ci = &requested["payload"]["ci_failure"][0];
    assert_eq!(ci["repository_id"], "app");
    assert_eq!(ci["pull_request"]["number"], 1);
    assert_eq!(ci["failed_checks"][0]["name"], "ci-unit");
    assert_eq!(ci["failed_checks"][0]["conclusion"], "failure");
    assert_eq!(ci["checks"].as_array().unwrap().len(), 2);
    let types = event_types(&events);
    let invalidated_at = types
        .iter()
        .position(|e| e == "candidate.invalidated")
        .unwrap();
    let requested_at = types
        .iter()
        .position(|e| e == "publication.repair_requested")
        .unwrap();
    let repair_at = types
        .iter()
        .position(|e| e == "candidate.repair_started")
        .unwrap();
    assert!(
        invalidated_at < requested_at && requested_at < repair_at,
        "{types:?}"
    );
    // One repair attempt, provenance `ci_repair`, started from the first candidate.
    let (_, attempts) = harness
        .request("GET", &format!("/api/v1/runs/{run_id}/attempts"), None)
        .await;
    let attempts = attempts.as_array().unwrap();
    assert_eq!(attempts.len(), 2, "{attempts:?}");
    assert_eq!(
        attempts[1]["payload"]["trigger"], "ci_repair",
        "{attempts:?}"
    );
    assert_eq!(attempts[1]["payload"]["repair_of"], hash);

    // The repaired candidate is a new revision in a fresh worktree whose
    // writer saw the CI failure (the synthetic writer only repairs when the
    // evidence names the failing check); it validates and reviews afresh.
    let pending = harness.candidate(&run_id).await;
    assert_eq!(pending["evidence"]["status"], "pending", "{pending}");
    assert_eq!(pending["evidence"]["candidate"]["revision"], 2);
    let second_hash = pending["evidence"]["candidate"]["hash"].clone();
    assert_ne!(second_hash, hash);
    let second_commit = pending["evidence"]["candidate"]["repository_commits"]["app"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_ne!(second_commit, first_commit);
    let worktree = harness.worktree(&run_id).await;
    assert_eq!(
        fs::read_to_string(worktree.join("src/main.py"))
            .unwrap()
            .replace("\r\n", "\n"),
        "print('repaired after the ci failure')\n"
    );
    let (status, body) = harness.validate(&run_id, &second_hash).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    let reviewed = harness.candidate_settled(&run_id).await;
    assert_eq!(reviewed["evidence"]["status"], "reviewed", "{reviewed}");
    assert_eq!(reviewed["evidence"]["review"]["verdict"], "passed");
    assert_eq!(reviewed["evidence"]["candidate"]["hash"], second_hash);

    // The first publication reads as superseded; its pull request is still
    // the developer's (never closed or merged by Luuna).
    let (status, earlier) = harness.publication(&run_id).await;
    assert_eq!(status, StatusCode::OK, "{earlier}");
    assert_eq!(earlier["candidate_hash"], hash);
    assert_eq!(earlier["superseded_by_repair"], true);
    assert_eq!(earlier["publication_state"], "pr_open");
    assert!(earlier["superseded_at"].is_string());
    assert_eq!(earlier["repositories"]["app"]["ci"]["state"], "failure");
    // A second request against the superseded record is refused.
    let (status, refused) = harness.repair_publication(&run_id, &hash).await;
    assert_eq!(status, StatusCode::CONFLICT, "{refused}");
    assert!(refused["error"]
        .as_str()
        .unwrap()
        .contains("already superseded"));

    // Publishing the repaired candidate opens pull request 2 on its own
    // branch at the exact new commit; its body names pull request 1 as
    // superseded. The first branch and pull request are left alone.
    let (status, accepted) = harness
        .publish(&run_id, &second_hash, "publish-ci-repair-2")
        .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{accepted}");
    let second = harness.publication_settled(&run_id).await;
    assert_eq!(second["publication_state"], "pr_open", "{second}");
    assert_eq!(second["candidate_hash"], second_hash);
    assert!(second.get("superseded_by_repair").is_none(), "{second}");
    assert_eq!(second["repositories"]["app"]["pull_request"]["number"], 2);
    let second_branch = second["repositories"]["app"]["branch"].as_str().unwrap();
    let first_branch = earlier["repositories"]["app"]["branch"].as_str().unwrap();
    assert_ne!(second_branch, first_branch);
    assert_eq!(
        second_branch,
        format!(
            "luuna/{}/{}",
            ticket["id"].as_str().unwrap(),
            &second_hash.as_str().unwrap()[7..19]
        )
    );
    assert_eq!(
        remote_branch(repo.path(), second_branch).as_deref(),
        Some(second_commit.as_str())
    );
    assert_eq!(
        remote_branch(repo.path(), first_branch).as_deref(),
        Some(first_commit.as_str())
    );
    assert_eq!(harness.requests("POST", "/pulls"), 2);
    {
        let fake = harness.fake.lock().unwrap();
        assert_eq!(fake.pull_requests.len(), 2);
        let body = fake.pull_requests[1]["body"].as_str().unwrap();
        assert!(body.contains("## Supersedes"), "{body}");
        assert!(
            body.contains("Pull request #1 (https://github.com/luuna-test/fixture/pull/1)"),
            "{body}"
        );
        assert!(body.contains(hash.as_str().unwrap()), "{body}");
        assert!(body.contains(second_hash.as_str().unwrap()), "{body}");
        assert!(body.contains("ci-unit failure"), "{body}");
        assert_eq!(fake.pull_requests[0]["state"], "open");
    }
    let events = harness.events(&run_id).await;
    assert_eq!(
        events
            .iter()
            .filter(|e| e["type"] == "publication.requested")
            .count(),
        2
    );

    // The repair budget bounds a further CI repair: with `max_task_repairs`
    // 1 spent, a new failure is refused before anything is invalidated.
    let (status, refused) = harness.repair_publication(&run_id, &second_hash).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");
    let reason = refused["error"].as_str().unwrap();
    assert!(reason.contains("max_task_repairs exhausted"), "{reason}");
    assert!(reason.contains("re-plan"), "{reason}");
    let view = harness.candidate(&run_id).await;
    assert_eq!(view["evidence"]["status"], "reviewed", "{view}");
    assert_eq!(view["evidence"]["candidate"]["hash"], second_hash);
    assert_eq!(harness.run(&run_id).await["state"], "reviewed");
    let (_, record) = harness.publication(&run_id).await;
    assert_eq!(record["candidate_hash"], second_hash);
    assert!(record.get("superseded_by_repair").is_none(), "{record}");
    assert_eq!(
        event_types(&harness.events(&run_id).await)
            .iter()
            .filter(|e| *e == "publication.repair_requested" || *e == "candidate.invalidated")
            .count(),
        2
    );
    // Nothing was ever merged, closed or deleted on GitHub.
    assert_no_merge_requests(&harness.fake);
    assert_eq!(harness.requests("PATCH", "/pulls"), 0);
    assert_eq!(
        fs::read_to_string(repo.path().join("src/main.py")).unwrap(),
        "print('fixture')\n"
    );
}

#[tokio::test]
async fn editing_the_ticket_after_the_review_refuses_publication_as_source_changed() {
    let harness = Harness::new().await;
    let repo = Repo::new();
    let (run_id, ticket, candidate) = harness.reviewed(repo.path()).await;
    let hash = candidate["hash"].clone();
    let ticket_id = ticket["id"].as_str().unwrap();
    let (status, edited) = harness
        .request(
            "PUT",
            &format!("/api/v1/tickets/{ticket_id}"),
            Some(json!({"expected_revision": ticket["revision"], "title": "Edited after the review"})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{edited}");
    assert_eq!(edited["revision"], 2, "{edited}");

    let (status, refused) = harness.publish(&run_id, &hash, "publish-edited").await;
    assert_eq!(status, StatusCode::CONFLICT, "{refused}");
    let reason = refused["error"].as_str().unwrap();
    assert!(
        reason.contains("Ticket changed since plan approval"),
        "{reason}"
    );
    assert_eq!(harness.requests("POST", "/pulls"), 0);
    assert_eq!(harness.requests("GET", "/pulls"), 0);
    assert!(remote_branch(repo.path(), "luuna").is_none());
    let (status, body) = harness.publication(&run_id).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    let events = harness.events(&run_id).await;
    assert_eq!(refusals(&events), vec!["source_changed"]);
    let refusal = events
        .iter()
        .find(|e| e["type"] == "publication.refused")
        .unwrap();
    assert_eq!(refusal["payload"]["candidate_hash"], hash);
    assert!(refusal["payload"]["reason"]
        .as_str()
        .unwrap()
        .contains("create a new plan"));
    assert!(
        !event_types(&events)
            .iter()
            .any(|e| e == "publication.requested" || e == "publication.pushed"),
        "{events:?}"
    );
    // Reconciliation meets the same gate; the reviewed evidence is kept for revision 1.
    let (status, body) = harness.reconcile(&run_id).await;
    assert_ne!(status, StatusCode::ACCEPTED, "{body}");
    let view = harness.candidate(&run_id).await;
    assert_eq!(view["evidence"]["status"], "reviewed");
    assert_eq!(harness.run(&run_id).await["state"], "reviewed");
}
