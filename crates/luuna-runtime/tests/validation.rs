//! M3 candidate validation and independent review through the authenticated
//! API with the synthetic Pi fixture: every generation runs on exactly the
//! frozen candidate tree, drift settles with a recorded reason, review diffs
//! are bounded and attribute-neutral, reviews are self-consistent, reviewer
//! tool use is traceable, and the candidate view's decisions match the routes.
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
        let script = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/pi-rpc/mock_pi.py")
            .canonicalize()
            .unwrap();
        let root =
            std::env::temp_dir().join(format!("luuna-validation-test-{}", uuid::Uuid::new_v4()));
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
        let (status, bytes) = self.raw(method, path, body).await;
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    async fn raw(&self, method: &str, path: &str, body: Option<Value>) -> (StatusCode, Vec<u8>) {
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
        (status, bytes.to_vec())
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

    async fn register(&self, dir: &Path, checks: Vec<Value>) {
        let (status, view) = self
            .request(
                "POST",
                "/api/v1/projects",
                Some(json!({"id":"fixture","name":"Fixture","repositories":[{"id":"app","path":dir.to_string_lossy()}]})),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED, "{view}");
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

    /// Ticket, plan and exact-hash approval: an approved workflow run.
    async fn approved(&self, scenario: &str, budgets: Value) -> String {
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
        opened["run"]["id"].as_str().unwrap().to_owned()
    }

    /// An approved workflow whose writer settled with a pending candidate and whose
    /// workflow the runtime has released (the candidate view reports `active: false`),
    /// so the next action is admitted.
    async fn pending(&self, scenario: &str, budgets: Value) -> (String, Value) {
        let id = self.approved(scenario, budgets).await;
        let (status, run) = self
            .request(
                "POST",
                &format!("/api/v1/runs/{id}/implement"),
                Some(json!({"provider": "test", "model": "synthetic"})),
            )
            .await;
        assert_eq!(status, StatusCode::ACCEPTED, "{run}");
        let run = self.settled(&id).await;
        assert_eq!(
            run["state"],
            "validating",
            "run did not reach validating: run {run}; run.failed payloads {:?}; attempts {}",
            self.events(&id)
                .await
                .iter()
                .filter(|e| e["type"] == "run.failed")
                .map(|e| e["payload"].clone())
                .collect::<Vec<_>>(),
            self.request("GET", &format!("/api/v1/runs/{id}/attempts"), None)
                .await
                .1
        );
        let mut view = Value::Null;
        for _ in 0..600 {
            view = self.candidate(&id).await;
            if view["active"] == false {
                let hash = view["evidence"]["candidate"]["hash"].clone();
                return (id, hash);
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!("workflow {id} still active after the writer settled: {view}");
    }

    async fn candidate(&self, id: &str) -> Value {
        let (status, body) = self
            .request("GET", &format!("/api/v1/runs/{id}/candidate"), None)
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        body
    }

    async fn action(&self, id: &str, action: &str, hash: &Value) -> (StatusCode, Value) {
        self.request(
            "POST",
            &format!("/api/v1/runs/{id}/{action}"),
            Some(json!({"candidate_hash":hash,"provider":"test","model":"synthetic"})),
        )
        .await
    }

    async fn candidate_settled(&self, id: &str) -> Value {
        for _ in 0..600 {
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

    async fn validated(&self, id: &str, hash: &Value) -> Value {
        let (status, response) = self.action(id, "validate", hash).await;
        assert_eq!(status, StatusCode::ACCEPTED, "{response}");
        self.candidate_settled(id).await
    }

    async fn worktree(&self, id: &str) -> PathBuf {
        let (_, leases) = self
            .request("GET", &format!("/api/v1/runs/{id}/worktrees"), None)
            .await;
        PathBuf::from(
            leases.as_array().unwrap().last().unwrap()["path"]
                .as_str()
                .unwrap(),
        )
    }

    /// The candidate view must report exactly what the validate route does.
    async fn assert_validate_decision_matches(&self, id: &str, hash: &Value) {
        let view = self.candidate(id).await;
        let reason = view["decision_reasons"]["validate"].clone();
        assert_eq!(view["validate_allowed"], reason.is_null(), "{view}");
        assert!(
            view["validate_allowed"] == false,
            "only refusals are probed: {view}"
        );
        let (status, refused) = self.action(id, "validate", hash).await;
        assert_ne!(status, StatusCode::ACCEPTED, "{refused}");
        assert_eq!(refused["error"], reason, "{view}");
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

/// A committed fixture repository; `extra` files join the base commit.
fn repository(dir: &Path, extra: &[(&str, &str)]) {
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
    for (path, content) in extra {
        fs::write(dir.join(path), content).unwrap();
    }
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

fn reported(review: &Value, key: &str) -> Value {
    let prefix = format!("{key}=");
    let entry = review["evidence"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .find_map(|e| e.strip_prefix(&prefix))
        .unwrap_or_else(|| panic!("{key} missing from {review}"));
    serde_json::from_str(entry).unwrap()
}

#[tokio::test]
async fn every_generation_starts_from_exactly_the_candidate_tree_and_records_the_reset() {
    let harness = Harness::new();
    let repo = tempfile::tempdir().unwrap();
    repository(
        repo.path(),
        &[(".gitignore", "src/local_settings.py\n__pycache__/\n")],
    );
    // Fails on anything that is not candidate content; leaves build output
    // behind for the next generation to find.
    harness
        .register(repo.path(), vec![check("unit", "from pathlib import Path; import sys; stale = [p for p in ('src/local_settings.py', 'notes.txt', 'build.marker') if Path(p).exists()]; Path('build.marker').write_text('built') if 'implemented by' in Path('src/main.py').read_text() else None; print('stale: ' + ', '.join(stale)); print('running 1 tests'); print('test result: ' + ('FAILED. 0 passed; 1 failed' if stale else 'ok. 1 passed; 0 failed; 0 ignored; 0 measured')); sys.exit(1 if stale else 0)")])
        .await;
    let checkout = git(repo.path(), &["status", "--porcelain", "--ignored"]);
    let (id, hash) = harness
        .pending("review_malformed_once", json!({"max_attempts": 1}))
        .await;
    let worktree = harness.worktree(&id).await;
    // An ignored file and an untracked file that are not part of the commit.
    fs::write(
        worktree.join("src/local_settings.py"),
        "GREETING = 'hidden'\n",
    )
    .unwrap();
    fs::write(worktree.join("notes.txt"), "scratch\n").unwrap();

    let first = harness.validated(&id, &hash).await;
    assert_eq!(
        first["evidence"]["results"][0]["status"], "passed",
        "{first}"
    );
    assert_eq!(
        first["evidence"]["status"], "failed",
        "invalid first review"
    );
    assert_eq!(first["revalidate_allowed"], true, "{first}");
    assert!(!worktree.join("src/local_settings.py").exists());
    assert!(!worktree.join("notes.txt").exists());
    assert!(worktree.join("build.marker").exists());

    // Generation 2 of the same revision does not inherit generation 1's output.
    let second = harness.validated(&id, &hash).await;
    assert_eq!(second["evidence"]["status"], "reviewed", "{second}");
    assert_eq!(second["evidence"]["validation_generation"], 2);
    assert_eq!(second["evidence"]["results"][0]["status"], "passed");

    let events = harness.events(&id).await;
    let resets: Vec<&Value> = events
        .iter()
        .filter(|e| e["type"] == "candidate.worktree_cleaned")
        .collect();
    assert_eq!(resets.len(), 2, "{events:?}");
    assert_eq!(resets[0]["payload"]["candidate_hash"], hash);
    assert_eq!(resets[0]["payload"]["generation"], 1);
    assert_eq!(
        resets[0]["payload"]["removed"],
        json!(["notes.txt", "src/local_settings.py"])
    );
    assert_eq!(resets[0]["payload"]["removed_count"], 2);
    assert_eq!(resets[1]["payload"]["generation"], 2);
    assert!(resets[1]["payload"]["removed"]
        .as_array()
        .unwrap()
        .contains(&json!("build.marker")));
    // Each reset precedes its generation's checks.
    let position = |kind: &str, nth: usize| {
        events
            .iter()
            .enumerate()
            .filter(|(_, e)| e["type"] == kind)
            .nth(nth)
            .unwrap()
            .0
    };
    assert!(position("candidate.worktree_cleaned", 1) < position("candidate.check_finished", 1));
    assert!(
        position("candidate.validation_started", 1) < position("candidate.worktree_cleaned", 1)
    );
    assert_eq!(
        git(repo.path(), &["status", "--porcelain", "--ignored"]),
        checkout,
        "the developer checkout is never cleaned"
    );
}

#[tokio::test]
async fn manual_worktree_edits_fail_the_candidate_with_a_reason_and_next_step() {
    let harness = Harness::new();
    let repo = tempfile::tempdir().unwrap();
    repository(repo.path(), &[]);
    harness.register(repo.path(), vec![passing_check()]).await;
    let (id, hash) = harness.pending("write_in_scope", json!({})).await;
    let worktree = harness.worktree(&id).await;
    fs::write(worktree.join("src/main.py"), "print('manual mutation')\n").unwrap();

    let failed = harness.validated(&id, &hash).await;
    let evidence = &failed["evidence"];
    assert_eq!(evidence["status"], "failed", "{failed}");
    assert_eq!(harness.run(&id).await["state"], "failed");
    assert_eq!(evidence["content_drift"], true);
    let reason = evidence["reason"].as_str().unwrap();
    assert!(reason.starts_with("candidate content drift:"), "{reason}");
    assert!(reason.contains("changed outside Luuna"), "{reason}");
    assert!(reason.contains("src/main.py"), "{reason}");
    assert!(reason.contains("next plan revision"), "{reason}");
    assert!(evidence["results"].as_array().unwrap().is_empty());
    assert!(evidence["review"].is_null());
    let events = harness.events(&id).await;
    assert!(!events.iter().any(|e| e["type"] == "review.started"));
    assert!(!events
        .iter()
        .any(|e| e["type"] == "candidate.worktree_cleaned"));
    assert!(events
        .iter()
        .any(|e| e["type"] == "candidate.stage_failed" && e["payload"]["content_drift"] == true));
    // The developer's edit is kept, never reset.
    assert_eq!(
        fs::read_to_string(worktree.join("src/main.py")).unwrap(),
        "print('manual mutation')\n"
    );
    // The candidate cannot be validated again in place; repair (a fresh
    // worktree) is still open, and the decisions say so.
    assert_eq!(failed["validate_allowed"], false);
    assert_eq!(failed["revalidate_allowed"], false);
    assert!(failed["decision_reasons"]["revalidate"]
        .as_str()
        .unwrap()
        .contains("drifted"));
    assert_eq!(failed["repair_allowed"], true, "{failed}");
    assert!(failed["decision_reasons"]["repair"].is_null());
    harness.assert_validate_decision_matches(&id, &hash).await;

    // The diff between the frozen commits is still served, with the drift named.
    let (status, diff) = harness
        .request("GET", &format!("/api/v1/runs/{id}/diff"), None)
        .await;
    assert_eq!(status, StatusCode::OK, "{diff}");
    assert!(diff["diff"]
        .as_str()
        .unwrap()
        .contains("+print('implemented by the synthetic writer')"));
    assert!(diff["worktree_drift"]
        .as_str()
        .unwrap()
        .contains("src/main.py"));

    // After review, the evidence stays bound to the frozen commit and tree,
    // which a later edit does not change; the diff view names the drift.
    let (id, hash) = harness.pending("write_in_scope", json!({})).await;
    let reviewed = harness.validated(&id, &hash).await;
    assert_eq!(reviewed["evidence"]["status"], "reviewed", "{reviewed}");
    let worktree = harness.worktree(&id).await;
    fs::write(
        worktree.join("src/main.py"),
        "print('edited after review')\n",
    )
    .unwrap();
    let after = harness.candidate(&id).await;
    assert_eq!(after["evidence"]["status"], "reviewed");
    assert_eq!(after["evidence"]["review"], reviewed["evidence"]["review"]);
    assert_eq!(after["validate_allowed"], false);
    let (status, diff) = harness
        .request("GET", &format!("/api/v1/runs/{id}/diff"), None)
        .await;
    assert_eq!(status, StatusCode::OK, "{diff}");
    assert!(diff["worktree_drift"]
        .as_str()
        .unwrap()
        .contains("src/main.py"));
}

#[tokio::test]
async fn a_check_that_rewrites_tracked_content_names_the_check_and_closes_revalidation() {
    let harness = Harness::new();
    let repo = tempfile::tempdir().unwrap();
    repository(repo.path(), &[]);
    harness
        .register(repo.path(), vec![check("unit", "from pathlib import Path; p = Path('src/main.py'); candidate = 'implemented by' in p.read_text(); p.write_text(\"print('rewritten by check')\\n\") if candidate else None; print('running 1 tests'); print('test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured')")])
        .await;
    let (id, hash) = harness.pending("write_in_scope", json!({})).await;
    let failed = harness.validated(&id, &hash).await;
    let evidence = &failed["evidence"];
    assert_eq!(evidence["status"], "failed", "{failed}");
    assert_eq!(evidence["results"][0]["status"], "passed");
    assert_eq!(evidence["content_drift"], true);
    let reason = evidence["reason"].as_str().unwrap();
    assert!(reason.contains("required check `unit`"), "{reason}");
    assert!(reason.contains("src/main.py"), "{reason}");
    assert!(reason.contains("repair the candidate"), "{reason}");
    // Passed checks and no verdict no longer open a re-validation that could
    // only fail on the same drift again.
    assert_eq!(failed["revalidate_allowed"], false, "{failed}");
    assert_eq!(failed["repair_allowed"], true, "{failed}");
    harness.assert_validate_decision_matches(&id, &hash).await;
    let (status, diff) = harness
        .request("GET", &format!("/api/v1/runs/{id}/diff"), None)
        .await;
    assert_eq!(status, StatusCode::OK, "{diff}");
    assert!(diff["worktree_drift"].is_string());
}

#[tokio::test]
async fn review_diffs_show_attribute_hidden_changes_and_stay_bounded_without_refusing() {
    // A repository that already hides Python changes behind `-diff`.
    let harness = Harness::new();
    let repo = tempfile::tempdir().unwrap();
    repository(repo.path(), &[(".gitattributes", "*.py -diff\n")]);
    harness.register(repo.path(), vec![passing_check()]).await;
    let (id, hash) = harness.pending("review_report", json!({})).await;
    let (status, diff) = harness
        .request("GET", &format!("/api/v1/runs/{id}/diff"), None)
        .await;
    assert_eq!(status, StatusCode::OK, "{diff}");
    let text = diff["diff"].as_str().unwrap();
    assert!(!text.contains("GIT binary patch"), "{text}");
    assert!(
        text.contains("+print('implemented by the synthetic writer')"),
        "{text}"
    );
    assert_eq!(diff["truncated"], false);
    assert_eq!(
        diff["files"],
        json!([{"path": "src/main.py", "status": "M", "shown": "text"}])
    );
    let reviewed = harness.validated(&id, &hash).await;
    let review = &reviewed["evidence"]["review"];
    assert_eq!(reviewed["evidence"]["status"], "reviewed", "{reviewed}");
    assert_eq!(reported(review, "writer_change"), true);
    assert_eq!(reported(review, "binary_patch"), false);
    assert_eq!(reported(review, "diff_truncated"), false);
    assert_eq!(reported(review, "attention"), json!([]));
    // The reviewer's scope root is the plain path Pi runs in (on Windows,
    // never the `\\?\` verbatim form, which the guard cannot match).
    assert_eq!(reported(review, "scope_root_is_cwd"), true);

    // A diff beyond the review bound reaches the reviewer as a bounded diff
    // with a notice and the file list, instead of a refusal that changes nothing.
    let harness = Harness::new();
    let repo = tempfile::tempdir().unwrap();
    let large = format!(
        "print('fixture')\n{}",
        "# padding line for the review bound\n".repeat(9000)
    );
    repository(repo.path(), &[]);
    fs::write(repo.path().join("src/main.py"), &large).unwrap();
    git(repo.path(), &["commit", "-qam", "large module"]);
    harness.register(repo.path(), vec![passing_check()]).await;
    let (id, hash) = harness.pending("review_report", json!({})).await;
    let reviewed = harness.validated(&id, &hash).await;
    assert_eq!(reviewed["evidence"]["status"], "reviewed", "{reviewed}");
    let review = &reviewed["evidence"]["review"];
    assert_eq!(reported(review, "diff_truncated"), true);
    assert_eq!(reported(review, "diff_notice"), true);
    assert_eq!(
        reported(review, "files"),
        json!([{"path": "src/main.py", "status": "M", "shown": "omitted"}])
    );
    let (status, diff) = harness
        .request("GET", &format!("/api/v1/runs/{id}/diff"), None)
        .await;
    assert_eq!(status, StatusCode::OK, "{diff}");
    assert_eq!(
        diff["truncated"], false,
        "the developer view has a larger bound"
    );
    assert!(diff["diff"].as_str().unwrap().len() > 256 * 1024);
}

#[tokio::test]
async fn a_non_passing_review_without_findings_leaves_no_verdict_and_starts_no_repair() {
    let harness = Harness::new();
    let repo = tempfile::tempdir().unwrap();
    repository(repo.path(), &[]);
    harness.register(repo.path(), vec![passing_check()]).await;
    let (id, hash) = harness
        .pending("review_no_findings", json!({"max_attempts": 2}))
        .await;
    let (status, response) = harness
        .request(
            "POST",
            &format!("/api/v1/runs/{id}/validate"),
            Some(json!({"candidate_hash":hash,"provider":"test","model":"synthetic","auto_repair":true})),
        )
        .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{response}");
    let failed = harness.candidate_settled(&id).await;
    let evidence = &failed["evidence"];
    assert_eq!(evidence["status"], "failed", "{failed}");
    assert!(evidence["review"].is_null());
    assert!(evidence["reason"]
        .as_str()
        .unwrap()
        .contains("must name at least one finding"));
    assert_eq!(evidence["candidate"]["revision"], 1, "no repair revision");
    let store = luuna_storage::Store::open(harness.root.join("luuna.sqlite3")).unwrap();
    assert_eq!(
        store.attempts(&id).unwrap().len(),
        1,
        "no writer attempt spent"
    );
    assert_eq!(failed["revalidate_allowed"], true, "{failed}");
}

#[tokio::test]
async fn reviewer_tool_calls_and_rejected_output_are_kept_as_evidence() {
    let harness = Harness::new();
    let repo = tempfile::tempdir().unwrap();
    repository(repo.path(), &[]);
    harness.register(repo.path(), vec![passing_check()]).await;
    let (id, hash) = harness.pending("review_tools", json!({})).await;
    let reviewed = harness.validated(&id, &hash).await;
    assert_eq!(reviewed["evidence"]["status"], "reviewed", "{reviewed}");
    let session = reviewed["evidence"]["review"]["reviewer_session"].clone();
    let events = harness.events(&id).await;
    let calls: Vec<&Value> = events
        .iter()
        .filter(|e| e["type"] == "review.tool_call")
        .map(|e| &e["payload"])
        .collect();
    let summary: Vec<(Value, Value, Value)> = calls
        .iter()
        .map(|c| (c["tool"].clone(), c["path"].clone(), c["failed"].clone()))
        .collect();
    assert_eq!(
        summary,
        vec![
            (json!("read"), json!("src/main.py"), Value::Null),
            (json!("grep"), json!("src"), Value::Null),
            (json!("read"), json!("../outside.txt"), Value::Null),
            (json!("read"), Value::Null, json!(true)),
        ]
    );
    assert!(calls.iter().all(|c| c["reviewer_session"] == session));
    let text = serde_json::to_string(&events).unwrap();
    assert!(
        !text.contains("fixture-secret-pattern"),
        "only paths are recorded"
    );
    assert!(
        !text.contains("fixture-file-contents"),
        "results are never recorded"
    );

    // Invalid reviewer output is kept privately and named in the reason.
    let harness = Harness::new();
    let repo = tempfile::tempdir().unwrap();
    repository(repo.path(), &[]);
    harness.register(repo.path(), vec![passing_check()]).await;
    let (id, hash) = harness.pending("review_malformed", json!({})).await;
    let failed = harness.validated(&id, &hash).await;
    let reason = failed["evidence"]["reason"].as_str().unwrap().to_owned();
    assert!(
        reason.starts_with("invalid structured reviewer response"),
        "{reason}"
    );
    let artifact = reason
        .split("private artifact ")
        .nth(1)
        .and_then(|rest| rest.split_whitespace().next())
        .unwrap_or_else(|| panic!("{reason}"));
    assert!(
        artifact.starts_with("review-") && artifact.ends_with(".stdout"),
        "{artifact}"
    );
    let (status, body) = harness
        .raw(
            "GET",
            &format!("/api/v1/runs/{id}/artifacts/{artifact}"),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, b"not a review JSON object");
    assert!(reason.contains(&format!("sha256:{}", luuna_executor::sha256_hex(&body))));
    assert_eq!(failed["revalidate_allowed"], true, "{failed}");
}

#[tokio::test]
async fn candidate_decisions_match_what_the_routes_decide() {
    let harness = Harness::new();
    let repo = tempfile::tempdir().unwrap();
    repository(repo.path(), &[]);
    harness.register(repo.path(), vec![passing_check()]).await;

    // Pending: validation is the one admitted action.
    let (id, hash) = harness
        .pending("review_hold", json!({"max_attempts": 1}))
        .await;
    let pending = harness.candidate(&id).await;
    assert_eq!(pending["validate_allowed"], true, "{pending}");
    assert!(pending["decision_reasons"]["validate"].is_null());
    assert_eq!(pending["repair_allowed"], false);
    assert!(pending["decision_reasons"]["repair"].is_string());
    assert_eq!(pending["revalidate_allowed"], false);
    assert!(pending["decision_reasons"]["revalidate"].is_string());

    // Active work admits nothing, and the reason is the route's.
    let (status, _) = harness.action(&id, "validate", &hash).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    harness.wait_for_event(&id, "review.started").await;
    let active = harness.candidate(&id).await;
    assert_eq!(active["active"], true);
    assert_eq!(active["validate_allowed"], false);
    assert_eq!(
        active["decision_reasons"]["validate"],
        "workflow already has active work"
    );
    let (status, refused) = harness.action(&id, "validate", &hash).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(refused["error"], "workflow already has active work");

    // A runtime restart after the reviewer was prompted reserved the
    // unreported spend: the interrupted candidate cannot resume, and the view
    // says why.
    harness.wait_for_event(&id, "review.prompt_sent").await;
    let mut store = luuna_storage::Store::open(harness.root.join("luuna.sqlite3")).unwrap();
    assert_eq!(store.interrupt_active_runs().unwrap(), 1);
    harness.state.cancel_active();
    let interrupted = harness.candidate_settled(&id).await;
    assert_eq!(
        interrupted["evidence"]["status"], "interrupted",
        "{interrupted}"
    );
    assert!(interrupted["decision_reasons"]["validate"]
        .as_str()
        .unwrap()
        .contains("budget"));
    harness.assert_validate_decision_matches(&id, &hash).await;

    // A failed required check: repair, never validation in place.
    let harness = Harness::new();
    let repo = tempfile::tempdir().unwrap();
    repository(repo.path(), &[]);
    harness
        .register(repo.path(), vec![check("unit", "from pathlib import Path; import sys; bad = 'implemented by' in Path('src/main.py').read_text(); print('running 1 tests'); print('test result: ' + ('FAILED. 0 passed; 1 failed' if bad else 'ok. 1 passed; 0 failed; 0 ignored; 0 measured')); sys.exit(1 if bad else 0)")])
        .await;
    let (failed_id, failed_hash) = harness.pending("write_in_scope", json!({})).await;
    let failed = harness.validated(&failed_id, &failed_hash).await;
    assert_eq!(failed["evidence"]["status"], "failed", "{failed}");
    assert_eq!(failed["repair_allowed"], true, "{failed}");
    harness
        .assert_validate_decision_matches(&failed_id, &failed_hash)
        .await;

    // Another workflow's writer holds the repository: repair waits for it,
    // and the view reports the route's one-writer rule.
    let slow = harness.approved("slow", json!({})).await;
    let (status, _) = harness
        .request(
            "POST",
            &format!("/api/v1/runs/{slow}/implement"),
            Some(json!({"provider": "test", "model": "synthetic"})),
        )
        .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    harness.wait_for_event(&slow, "worktree.leased").await;
    let blocked = harness.candidate(&failed_id).await;
    assert_eq!(blocked["repair_allowed"], false, "{blocked}");
    let reason = blocked["decision_reasons"]["repair"].clone();
    assert!(reason
        .as_str()
        .unwrap()
        .contains("already has an active writer"));
    let (status, refused) = harness.action(&failed_id, "repair", &failed_hash).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(refused["error"], reason);
    let (status, _) = harness
        .request("POST", &format!("/api/v1/runs/{slow}/cancel"), None)
        .await;
    assert_eq!(status, StatusCode::ACCEPTED);
}

#[tokio::test]
#[cfg_attr(
    windows,
    ignore = "needs paths beyond MAX_PATH (or about 90k files) to pass Git's output cap"
)]
async fn a_repository_whose_index_listing_exceeds_the_git_output_cap_still_validates() {
    let harness = Harness::new();
    let repo = tempfile::tempdir().unwrap();
    // About 9 MB of `ls-files -v -z` output, past Git's default 8 MiB cap.
    for directory in 0..20 {
        let directory = repo
            .path()
            .join("vendor")
            .join(format!("{directory:03}{}", "d".repeat(190)));
        fs::create_dir_all(&directory).unwrap();
        for file in 0..1000 {
            fs::write(directory.join(format!("{file:04}{}", "f".repeat(240))), "").unwrap();
        }
    }
    repository(repo.path(), &[]);
    let listing = Command::new("git")
        .arg("-C")
        .arg(repo.path())
        .args(["ls-files", "-v", "-z"])
        .output()
        .unwrap()
        .stdout
        .len();
    assert!(listing > luuna_git::MAX_OUTPUT_BYTES, "{listing}");
    harness.register(repo.path(), vec![passing_check()]).await;
    let (id, hash) = harness.pending("write_in_scope", json!({})).await;
    let started = std::time::Instant::now();
    let reviewed = harness.validated(&id, &hash).await;
    assert_eq!(reviewed["evidence"]["status"], "reviewed", "{reviewed}");
    assert!(
        started.elapsed() < Duration::from_secs(25),
        "no Git timeout on the way"
    );
    let (status, diff) = harness
        .request("GET", &format!("/api/v1/runs/{id}/diff"), None)
        .await;
    assert_eq!(status, StatusCode::OK, "{diff}");
    assert!(diff["worktree_drift"].is_null(), "{diff}");
}

#[tokio::test]
async fn a_writer_interrupted_before_its_candidate_states_that_only_a_new_plan_revision_remains() {
    let harness = Harness::new();
    let repo = tempfile::tempdir().unwrap();
    repository(repo.path(), &[]);
    harness.register(repo.path(), vec![passing_check()]).await;
    let id = harness
        .approved("slow", json!({"timeout_seconds": 120}))
        .await;
    let (status, _) = harness
        .request(
            "POST",
            &format!("/api/v1/runs/{id}/implement"),
            Some(json!({"provider": "test", "model": "synthetic"})),
        )
        .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    harness.wait_for_event(&id, "worktree.leased").await;
    // Startup recovery on the same database, as a restarted runtime performs it.
    let mut store = luuna_storage::Store::open(harness.root.join("luuna.sqlite3")).unwrap();
    assert_eq!(store.interrupt_active_runs().unwrap(), 1);
    harness.state.cancel_active();
    let mut view = harness.candidate(&id).await;
    for _ in 0..400 {
        if view["active"] == false {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
        view = harness.candidate(&id).await;
    }
    assert_eq!(harness.run(&id).await["state"], "interrupted");
    assert!(view["evidence"].is_null(), "{view}");
    for action in ["validate", "repair", "revalidate"] {
        assert_eq!(view[format!("{action}_allowed")], false, "{view}");
        assert!(view["decision_reasons"][action]
            .as_str()
            .unwrap()
            .contains("next plan revision"));
    }
    let (status, refused) = harness
        .action(
            &id,
            "validate",
            &json!(format!("sha256:{}", "0".repeat(64))),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");
    assert_eq!(refused["error"], view["decision_reasons"]["validate"]);
}
