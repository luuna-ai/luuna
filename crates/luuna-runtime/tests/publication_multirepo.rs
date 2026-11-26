//! M7 publication of a linked multi-repository candidate set (ADR 0017 and
//! ADR 0018): the three-repository fixture of `multirepo.rs` (api -> shared
//! -> consumer, real `python -m unittest` checks bound through
//! `LUUNA_DEPENDENCY_*`) registered under GitHub-shaped identities
//! (`https://github.com/luuna-test/<id>.git`), each developer checkout
//! carrying a `url.<bare>.pushInsteadOf` rewrite to its own local bare
//! repository, and the shared fake GitHub REST server numbering pull requests
//! per repository. No model, no network, no real GitHub; the developer
//! checkouts are byte identical afterwards.
mod support;

use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
    Router,
};
use luuna_runtime::{router, AppState, PublicationSettings, RuntimeConfig};
use serde_json::{json, Value};
use std::{fs, path::Path, path::PathBuf, process::Command, time::Duration};
use support::{assert_no_merge_requests, start_fake_github, Fake};
use tower::ServiceExt;

const TOKEN: &str = "synthetic-test-token-with-at-least-32-bytes";
const REPOSITORIES: [&str; 3] = ["api", "shared", "consumer"];

fn remote(id: &str) -> String {
    format!("https://github.com/luuna-test/{id}.git")
}

struct Harness {
    app: Router,
    state: AppState,
    root: PathBuf,
    fake: Fake,
}
impl Harness {
    async fn new() -> Self {
        let (fake, api) = start_fake_github().await;
        let root = std::env::temp_dir().join(format!(
            "luuna-publication-multirepo-test-{}",
            uuid::Uuid::new_v4()
        ));
        let state = AppState::open(RuntimeConfig {
            data_dir: root.clone(),
            pi_program: python().into(),
            pi_launcher_args: vec![PathBuf::from(env!("CARGO_MANIFEST_DIR"))
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
        })
        .unwrap()
        .with_publication(PublicationSettings {
            github_api_url: api,
            github_token: Some("synthetic-github-token".into()),
            request_timeout: Duration::from_secs(5),
            observe_timeout: Duration::from_secs(5),
            git_timeout: Duration::from_secs(60),
        });
        Self {
            app: router(state.clone()),
            state,
            root,
            fake,
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
        for _ in 0..2400 {
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
        for _ in 0..2400 {
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
    async fn confirm(&self, revision: u64) -> (StatusCode, Value) {
        let repositories: Vec<Value> = REPOSITORIES
            .iter()
            .map(|id| {
                json!({"id": id, "remote": remote(id), "default_branch": "main",
                    "profiles": ["python"], "execution": "native", "checks": [unit_check()]})
            })
            .collect();
        let manifest = json!({
            "schema_version": "0.1", "id": "fixture", "name": "Fixture", "revision": revision + 1,
            "repositories": repositories,
            "relationships": [relationship("api", "shared"), relationship("shared", "consumer")],
            "policy": {"approval": "plan", "testing_prerequisite": true, "publish": "automatic_pr", "merge": "human",
                "max_explorers": 1, "max_implementers": 3, "max_task_repairs": 2},
        });
        self.request(
            "PUT",
            "/api/v1/projects/fixture/manifest",
            Some(json!({"expected_revision": revision, "manifest": manifest})),
        )
        .await
    }
    async fn baseline(&self, repository: &str) {
        let (status, run) = self
            .request(
                "POST",
                "/api/v1/projects/fixture/baselines",
                Some(json!({"repository_id": repository})),
            )
            .await;
        assert_eq!(status, StatusCode::ACCEPTED, "{run}");
        let run = self.settled(run["id"].as_str().unwrap()).await;
        assert_eq!(run["state"], "completed", "{run}");
    }
    async fn ticket(&self) -> Value {
        let (status, ticket) = self
            .request(
                "POST",
                "/api/v1/projects/fixture/tickets",
                Some(json!({
                    "idempotency_key": format!("key-{}", uuid::Uuid::new_v4()),
                    "title": "Change the API, the shared package and the consumer",
                    "intent": "Make the three repositories agree on the greeting.",
                    "acceptance_criteria": ["every repository's tests pass against the others' candidates"],
                    "repository_ids": REPOSITORIES,
                })),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED, "{ticket}");
        ticket
    }
    /// The `api -> shared -> consumer` chain as one plan revision.
    async fn plan(&self, ticket_id: &str) -> Value {
        let tasks: Vec<Value> = [("api", &[][..]), ("shared", &["api"][..]), ("consumer", &["shared"][..])]
            .iter()
            .map(|(id, depends_on)| {
                json!({"id": id, "repository_id": id, "paths": ["src"], "operations": ["read", "write"],
                    "acceptance_criteria": ["fixture:write_in_scope", "the repository's tests pass"],
                    "depends_on": depends_on})
            })
            .collect();
        let (status, opened) = self
            .request(
                "POST",
                &format!("/api/v1/tickets/{ticket_id}/plans"),
                Some(
                    json!({"tasks": tasks, "budgets": {"max_attempts": 4, "timeout_seconds": 600}}),
                ),
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
        for _ in 0..2400 {
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
    /// Register, confirm, baseline, plan, approve, implement the graph,
    /// validate and review: a reviewed three-repository candidate set.
    async fn reviewed_set(&self, fixture: &Fixture) -> (String, Value, Value) {
        let repositories: Vec<Value> = fixture
            .roots
            .iter()
            .map(|(id, root)| json!({"id": id, "path": root.to_string_lossy()}))
            .collect();
        let (status, view) = self
            .request(
                "POST",
                "/api/v1/projects",
                Some(json!({"id":"fixture","name":"Fixture","repositories":repositories})),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED, "{view}");
        let (status, view) = self.confirm(0).await;
        assert_eq!(status, StatusCode::OK, "{view}");
        for id in REPOSITORIES {
            self.baseline(id).await;
        }
        let ticket = self.ticket().await;
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
        let set = self.candidate(&run_id).await["evidence"]["candidate"].clone();
        assert_eq!(set["repository_commits"].as_object().unwrap().len(), 3);
        let (status, body) = self
            .request(
                "POST",
                &format!("/api/v1/runs/{run_id}/validate"),
                Some(json!({"candidate_hash": set["hash"], "provider": "test", "model": "synthetic"})),
            )
            .await;
        assert_eq!(status, StatusCode::ACCEPTED, "{body}");
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
        for _ in 0..2400 {
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
    async fn reconcile(&self, run_id: &str) -> (StatusCode, Value) {
        self.request(
            "POST",
            &format!("/api/v1/runs/{run_id}/publication/reconcile"),
            None,
        )
        .await
    }
    /// The Luuna-owned worktree that recorded the repository's candidate commit.
    async fn worktree(&self, run_id: &str, repository: &str, commit: &str) -> PathBuf {
        let (_, leases) = self
            .request("GET", &format!("/api/v1/runs/{run_id}/worktrees"), None)
            .await;
        let lease = leases
            .as_array()
            .unwrap()
            .iter()
            .find(|l| l["repository_id"] == repository && l["candidate_commit"] == commit)
            .unwrap_or_else(|| panic!("no worktree for {repository} at {commit}: {leases}"));
        PathBuf::from(lease["path"].as_str().unwrap())
    }
    fn requests(&self, method: &str, path_part: &str) -> usize {
        self.fake.lock().unwrap().count(method, path_part)
    }
    fn pull_request_body(&self, repo: &str, number: u64) -> String {
        self.fake
            .lock()
            .unwrap()
            .pull_request(repo, number)
            .unwrap_or_else(|| panic!("no pull request {repo}#{number}"))["body"]
            .as_str()
            .unwrap()
            .to_owned()
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
fn unit_check() -> Value {
    json!({"id": "unit", "executable": python(), "args": ["-m", "unittest", "discover", "-s", "tests", "-t", "."],
        "required": true, "timeout_seconds": 60, "capabilities": ["python"], "cwd": "."})
}
fn relationship(producer: &str, consumer: &str) -> Value {
    json!({"producer": producer, "consumer": consumer, "kind": "package", "contract": format!("npm:@fixture/{producer}"),
        "confirmed": true, "evidence": [format!("{consumer}/package.json dependencies names @fixture/{producer}")]})
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
fn py_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}
/// The local bare repository a developer checkout's push rewrite targets.
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
    let output = Command::new("git")
        .args(["-C", &bare_path(dir), "rev-parse", "--verify", "--quiet"])
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
fn count(events: &[Value], kind: &str) -> usize {
    events.iter().filter(|e| e["type"] == kind).count()
}

/// The three repositories of `multirepo.rs` (`api` exports `greet`, `shared`
/// imports it in `banner`, `consumer` imports `shared_pkg.helper`), each with
/// a GitHub-shaped origin and a sibling bare repository that Git's push URL
/// rewrite makes the effective remote.
struct Fixture {
    roots: Vec<(String, PathBuf)>,
}
impl Fixture {
    fn new(dir: &Path) -> Self {
        let roots: Vec<(String, PathBuf)> = REPOSITORIES
            .iter()
            .map(|id| ((*id).to_owned(), dir.join(id)))
            .collect();
        let root_of = |id: &str| roots.iter().find(|(r, _)| r == id).unwrap().1.clone();
        for (id, root) in &roots {
            fs::create_dir_all(root.join("src").join(format!("{id}_pkg"))).unwrap();
            fs::create_dir_all(root.join("tests")).unwrap();
            fs::write(root.join("tests/__init__.py"), "").unwrap();
            fs::write(root.join("README.md"), format!("{id} fixture\n")).unwrap();
            fs::write(root.join("src/main.py"), "print('fixture')\n").unwrap();
            fs::write(
                root.join("package.json"),
                match id.as_str() {
                    "api" => r#"{"name": "@fixture/api", "version": "1.0.0"}"#.to_owned(),
                    "shared" => r#"{"name": "@fixture/shared", "dependencies": {"@fixture/api": "1.0.0"}}"#.to_owned(),
                    _ => r#"{"name": "@fixture/consumer", "dependencies": {"@fixture/shared": "1.0.0"}}"#.to_owned(),
                },
            )
            .unwrap();
        }
        fs::write(
            root_of("api").join("src/api_pkg/__init__.py"),
            "def greet(name):\n    return 'hello ' + name\n",
        )
        .unwrap();
        fs::write(
            root_of("api").join("tests/test_api.py"),
            "import os, sys, unittest\nsys.path.insert(0, os.path.join(os.getcwd(), 'src'))\nimport api_pkg\n\nclass ApiTest(unittest.TestCase):\n    def test_greet(self):\n        self.assertEqual(api_pkg.greet('x'), 'hello x')\n",
        )
        .unwrap();
        fs::write(
            root_of("shared").join("src/shared_pkg/__init__.py"),
            "def helper():\n    return 'helped'\n\ndef banner():\n    import api_pkg\n    return api_pkg.greet('shared')\n",
        )
        .unwrap();
        fs::write(
            root_of("shared").join("tests/test_shared.py"),
            format!(
                "import os, sys, unittest\nsys.path.insert(0, os.path.join(os.getcwd(), 'src'))\nsys.path.insert(0, os.path.join(os.environ.get('LUUNA_DEPENDENCY_API_PATH', '{}'), 'src'))\nimport shared_pkg\n\nclass SharedTest(unittest.TestCase):\n    def test_banner_uses_the_bound_api(self):\n        self.assertEqual(shared_pkg.banner(), 'hello shared')\n",
                py_path(&root_of("api"))
            ),
        )
        .unwrap();
        fs::write(
            root_of("consumer").join("src/consumer_pkg/__init__.py"),
            "def use_shared():\n    import shared_pkg\n    return shared_pkg.helper()\n",
        )
        .unwrap();
        fs::write(
            root_of("consumer").join("tests/test_consumer.py"),
            format!(
                "import os, sys, unittest\nsys.path.insert(0, os.path.join(os.getcwd(), 'src'))\nsys.path.insert(0, os.path.join(os.environ.get('LUUNA_DEPENDENCY_SHARED_PATH', '{}'), 'src'))\nimport consumer_pkg\n\nclass ConsumerTest(unittest.TestCase):\n    def test_shared_helper_is_importable(self):\n        self.assertEqual(consumer_pkg.use_shared(), 'helped')\n",
                py_path(&root_of("shared"))
            ),
        )
        .unwrap();
        for (id, root) in &roots {
            git(root, &["init", "-q", "-b", "main"]);
            git(root, &["config", "commit.gpgsign", "false"]);
            git(root, &["add", "."]);
            git(root, &["commit", "-q", "-m", "initial"]);
            git(root, &["remote", "add", "origin", &remote(id)]);
            let bare = bare_path(root);
            git(root, &["init", "-q", "--bare", "-b", "main", &bare]);
            git(root, &["push", "-q", &bare, "main:refs/heads/main"]);
            git(
                root,
                &["config", &format!("url.{bare}.pushInsteadOf"), &remote(id)],
            );
        }
        Self { roots }
    }
    fn root(&self, id: &str) -> PathBuf {
        self.roots.iter().find(|(r, _)| r == id).unwrap().1.clone()
    }
    /// Every file of every developer checkout (the shared object store and
    /// the worktree registrations excluded, as in `multirepo.rs`).
    fn snapshot(&self) -> Vec<(String, Vec<u8>)> {
        let mut out = Vec::new();
        fn walk(root: &Path, dir: &Path, out: &mut Vec<(String, Vec<u8>)>) {
            for entry in fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                let key = path
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                if key == ".git/worktrees" || key == ".git/objects" {
                    continue;
                }
                if path.is_dir() {
                    walk(root, &path, out);
                } else {
                    out.push((key, fs::read(&path).unwrap()));
                }
            }
        }
        for (id, root) in &self.roots {
            let mut files = Vec::new();
            walk(root, root, &mut files);
            out.extend(files.into_iter().map(|(k, v)| (format!("{id}/{k}"), v)));
        }
        out.sort();
        out
    }
}

fn pr_url(repo: &str, number: u64) -> String {
    format!("https://github.com/luuna-test/{repo}/pull/{number}")
}

/// Every pull request body of the set names the other two by URL and states
/// the merge order `api`, `shared`, `consumer` with the bindings.
fn assert_linked_bodies(harness: &Harness, set: &Value) {
    for id in REPOSITORIES {
        let body = harness.pull_request_body(id, 1);
        for other in REPOSITORIES.iter().filter(|other| **other != id) {
            assert!(body.contains(&pr_url(other, 1)), "{id}: {body}");
        }
        assert!(!body.contains("pull request pending"), "{id}: {body}");
        assert!(
            body.contains("## Dependencies and merge order"),
            "{id}: {body}"
        );
        let api = body
            .find("1. `api`")
            .unwrap_or_else(|| panic!("{id}: {body}"));
        let shared = body
            .find("2. `shared`")
            .unwrap_or_else(|| panic!("{id}: {body}"));
        let consumer = body
            .find("3. `consumer`")
            .unwrap_or_else(|| panic!("{id}: {body}"));
        assert!(api < shared && shared < consumer, "{id}: {body}");
        assert!(
            body.contains(&format!("`{id}` (this pull request)")),
            "{id}: {body}"
        );
        assert!(
            body.contains("Merge producers before their consumers"),
            "{id}: {body}"
        );
        assert!(
            body.contains(&format!(
                "(depends on `api` at `{}`)",
                set["dependency_bindings"]["shared:api"].as_str().unwrap()
            )),
            "{id}: {body}"
        );
        assert!(
            body.contains(&format!(
                "(depends on `shared` at `{}`)",
                set["dependency_bindings"]["consumer:shared"]
                    .as_str()
                    .unwrap()
            )),
            "{id}: {body}"
        );
        assert!(body.contains("3-repository candidate set"), "{id}: {body}");
        assert!(body.contains("3 of 3 passed"), "{id}: {body}");
        assert!(body.contains(set["hash"].as_str().unwrap()), "{id}: {body}");
    }
}

#[tokio::test]
async fn a_reviewed_set_publishes_three_linked_pull_requests_at_the_exact_commits() {
    let harness = Harness::new().await;
    let dir = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(dir.path());
    let (run_id, ticket, set) = harness.reviewed_set(&fixture).await;
    let before = fixture.snapshot();
    let hash = set["hash"].clone();
    let (status, accepted) = harness.publish(&run_id, &hash, "publish-set").await;
    assert_eq!(status, StatusCode::ACCEPTED, "{accepted}");
    let record = harness.publication_settled(&run_id).await;
    assert_eq!(record["publication_state"], "pr_open", "{record}");
    let branch = format!(
        "luuna/{}/{}",
        ticket["id"].as_str().unwrap(),
        &hash.as_str().unwrap()[7..19]
    );
    for id in REPOSITORIES {
        let repository = &record["repositories"][id];
        let commit = set["repository_commits"][id].as_str().unwrap();
        assert_eq!(repository["branch"], branch, "{record}");
        assert_eq!(repository["commit"], commit);
        assert_eq!(repository["push"], "done");
        assert_eq!(repository["pull_request_step"], "done");
        assert_eq!(repository["pull_request"]["number"], 1, "{record}");
        assert_eq!(repository["pull_request"]["url"], pr_url(id, 1));
        assert_eq!(repository["ci"]["state"], "unknown");
        assert_eq!(repository["merge"]["merged"], false);
        assert_eq!(
            remote_branch(&fixture.root(id), &branch).as_deref(),
            Some(commit),
            "{id}"
        );
        assert_eq!(
            harness.requests("POST", &format!("/repos/luuna-test/{id}/pulls")),
            1
        );
        assert_eq!(
            harness.requests("PATCH", &format!("/repos/luuna-test/{id}/pulls/1")),
            1
        );
        assert_eq!(record["contract"]["pull_request_urls"][id], pr_url(id, 1));
        assert_eq!(record["contract"]["per_repository_branch"][id], branch);
    }
    assert_eq!(harness.requests("POST", "/pulls"), 3);
    assert_eq!(harness.fake.lock().unwrap().pull_requests.len(), 3);
    assert_linked_bodies(&harness, &set);
    let events = harness.events(&run_id).await;
    assert_eq!(count(&events, "publication.requested"), 1);
    assert_eq!(count(&events, "publication.pushed"), 3);
    assert_eq!(count(&events, "publication.pr_opened"), 3);
    assert_eq!(
        count(&events, "publication.failed"),
        0,
        "{:?}",
        event_types(&events)
    );
    let pushed: Vec<&str> = events
        .iter()
        .filter(|e| e["type"] == "publication.pushed")
        .map(|e| e["payload"]["repository_id"].as_str().unwrap())
        .collect();
    assert_eq!(
        pushed,
        ["api", "consumer", "shared"],
        "one push per repository"
    );
    // A replay under the same key changes nothing; reconciliation has
    // nothing to do.
    let (status, replay) = harness.publish(&run_id, &hash, "publish-set").await;
    assert_eq!(status, StatusCode::OK, "{replay}");
    let (status, same) = harness.reconcile(&run_id).await;
    assert_eq!(status, StatusCode::OK, "{same}");
    assert_eq!(harness.requests("POST", "/pulls"), 3);
    assert_eq!(harness.requests("PATCH", "/pulls"), 3);
    assert_no_merge_requests(&harness.fake);
    assert_eq!(fixture.snapshot(), before);
}

#[tokio::test]
async fn a_partial_set_is_completed_by_reconcile_without_a_second_push_or_duplicate_pull_request() {
    let harness = Harness::new().await;
    let dir = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(dir.path());
    let (run_id, _ticket, set) = harness.reviewed_set(&fixture).await;
    let before = fixture.snapshot();
    let hash = set["hash"].clone();
    // Repositories publish in identifier order (api, consumer, shared): the
    // second one's pull request creation fails once. The publisher goes on
    // to the third repository (each repository's steps are independent and
    // the ledger records each), so the third pull request is attempted and
    // opened; only the linking step, which needs every pull request, waits.
    harness
        .fake
        .lock()
        .unwrap()
        .fail_create_once
        .insert("consumer".into());
    let (status, accepted) = harness.publish(&run_id, &hash, "publish-partial").await;
    assert_eq!(status, StatusCode::ACCEPTED, "{accepted}");
    let failed = harness.publication_settled(&run_id).await;
    assert_eq!(failed["publication_state"], "failed", "{failed}");
    for id in REPOSITORIES {
        assert_eq!(failed["repositories"][id]["push"], "done", "{failed}");
        assert_eq!(
            remote_branch(
                &fixture.root(id),
                failed["repositories"][id]["branch"].as_str().unwrap()
            )
            .as_deref(),
            set["repository_commits"][id].as_str()
        );
    }
    assert_eq!(failed["repositories"]["api"]["pull_request"]["number"], 1);
    assert_eq!(
        failed["repositories"]["shared"]["pull_request"]["number"],
        1
    );
    assert_eq!(
        failed["repositories"]["consumer"]["pull_request_step"],
        "failed"
    );
    assert!(failed["repositories"]["consumer"]["pull_request"].is_null());
    assert!(failed["repositories"]["consumer"]["reason"]
        .as_str()
        .unwrap()
        .contains("500"));
    assert!(failed["reason"]
        .as_str()
        .unwrap()
        .contains("consumer pull_request"));
    assert_eq!(
        failed["contract"]["pull_request_urls"]
            .as_object()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(harness.requests("POST", "/pulls"), 3);
    assert_eq!(harness.fake.lock().unwrap().pull_requests.len(), 2);
    assert_eq!(
        harness.requests("PATCH", "/pulls"),
        0,
        "linking waits for the set"
    );
    let api_body = harness.pull_request_body("api", 1);
    assert!(
        api_body.contains("`consumer`: branch `luuna/")
            && api_body.contains("pull request pending"),
        "{api_body}"
    );
    let events = harness.events(&run_id).await;
    assert_eq!(count(&events, "publication.pushed"), 3);
    assert_eq!(count(&events, "publication.pr_opened"), 2);
    assert_eq!(count(&events, "publication.failed"), 1);

    // Reconcile retries only the missing pull request and the links.
    let (status, accepted) = harness.reconcile(&run_id).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{accepted}");
    let reconciled = harness.publication_settled(&run_id).await;
    assert_eq!(reconciled["publication_state"], "pr_open", "{reconciled}");
    for id in REPOSITORIES {
        assert_eq!(
            reconciled["repositories"][id]["pull_request"]["number"], 1,
            "{reconciled}"
        );
        assert_eq!(
            harness.requests("PATCH", &format!("/repos/luuna-test/{id}/pulls/1")),
            1
        );
    }
    assert_eq!(
        harness.requests("POST", "/pulls"),
        4,
        "only the consumer creation was retried"
    );
    assert_eq!(
        harness.requests("POST", "/repos/luuna-test/consumer/pulls"),
        2
    );
    assert_eq!(harness.fake.lock().unwrap().pull_requests.len(), 3);
    let events = harness.events(&run_id).await;
    assert_eq!(count(&events, "publication.pushed"), 3, "no second push");
    assert_eq!(count(&events, "publication.pr_opened"), 3);
    assert_eq!(count(&events, "publication.reconciled"), 1);
    let reopened = events
        .iter()
        .filter(|e| e["type"] == "publication.pr_opened")
        .all(|e| e["payload"]["reused_existing"] == false);
    assert!(reopened, "{events:?}");
    assert_linked_bodies(&harness, &set);
    assert_no_merge_requests(&harness.fake);
    assert_eq!(fixture.snapshot(), before);
}

#[tokio::test]
async fn integration_needs_every_pull_request_merged_and_is_recorded_once() {
    let harness = Harness::new().await;
    let dir = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(dir.path());
    let (run_id, _ticket, set) = harness.reviewed_set(&fixture).await;
    let before = fixture.snapshot();
    let (status, _) = harness
        .publish(&run_id, &set["hash"], "publish-merge")
        .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let open = harness.publication_settled(&run_id).await;
    assert_eq!(open["publication_state"], "pr_open", "{open}");

    // A person merges the api pull request only.
    harness.fake.lock().unwrap().merged.insert("api".into());
    let (status, partial) = harness.publication(&run_id).await;
    assert_eq!(status, StatusCode::OK, "{partial}");
    assert_eq!(partial["publication_state"], "pr_open", "{partial}");
    assert_eq!(partial["repositories"]["api"]["merge"]["merged"], true);
    assert_eq!(
        partial["repositories"]["api"]["merge"]["merged_by"],
        "a-human"
    );
    assert_eq!(partial["repositories"]["shared"]["merge"]["merged"], false);
    assert_eq!(
        partial["repositories"]["consumer"]["merge"]["merged"],
        false
    );
    assert_eq!(partial["contract"]["publication_state"], "pr_open");
    let events = harness.events(&run_id).await;
    assert_eq!(
        count(&events, "publication.integrated"),
        0,
        "{:?}",
        event_types(&events)
    );
    let (_, again) = harness.publication(&run_id).await;
    assert_eq!(again["publication_state"], "pr_open");

    // Every pull request merged: integrated, once.
    {
        let mut fake = harness.fake.lock().unwrap();
        fake.merged.insert("shared".into());
        fake.merged.insert("consumer".into());
    }
    let (status, integrated) = harness.publication(&run_id).await;
    assert_eq!(status, StatusCode::OK, "{integrated}");
    assert_eq!(
        integrated["publication_state"], "integrated",
        "{integrated}"
    );
    for id in REPOSITORIES {
        assert_eq!(integrated["repositories"][id]["merge"]["merged"], true);
    }
    let (_, still) = harness.publication(&run_id).await;
    assert_eq!(still["publication_state"], "integrated");
    let events = harness.events(&run_id).await;
    assert_eq!(
        count(&events, "publication.integrated"),
        1,
        "{:?}",
        event_types(&events)
    );
    let integrated_event = events
        .iter()
        .find(|e| e["type"] == "publication.integrated")
        .unwrap();
    assert_eq!(
        integrated_event["payload"]["pull_requests"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    assert_no_merge_requests(&harness.fake);
    assert_eq!(fixture.snapshot(), before);
}

#[tokio::test]
async fn a_newer_plan_revision_refuses_reconciliation_of_the_partial_set() {
    let harness = Harness::new().await;
    let dir = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(dir.path());
    let (run_id, ticket, set) = harness.reviewed_set(&fixture).await;
    let before = fixture.snapshot();
    harness
        .fake
        .lock()
        .unwrap()
        .fail_create_once
        .insert("shared".into());
    let (status, _) = harness
        .publish(&run_id, &set["hash"], "publish-then-replan")
        .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let failed = harness.publication_settled(&run_id).await;
    assert_eq!(failed["publication_state"], "failed", "{failed}");
    assert_eq!(
        failed["repositories"]["shared"]["pull_request_step"],
        "failed"
    );
    assert_eq!(harness.requests("POST", "/pulls"), 3);

    let second = harness.plan(ticket["id"].as_str().unwrap()).await;
    assert_eq!(second["plan"]["revision"], 2, "{second}");
    let (status, refused) = harness.reconcile(&run_id).await;
    assert_eq!(status, StatusCode::CONFLICT, "{refused}");
    let reason = refused["error"].as_str().unwrap();
    assert!(
        reason.contains("newer plan revision supersedes"),
        "{reason}"
    );
    assert_eq!(
        harness.requests("POST", "/pulls"),
        3,
        "no retry under superseded authority"
    );
    assert_eq!(harness.requests("PATCH", "/pulls"), 0);
    assert_eq!(harness.fake.lock().unwrap().pull_requests.len(), 2);
    let (_, record) = harness.publication(&run_id).await;
    assert_eq!(record["publication_state"], "failed");
    // The pushed branches are left exactly where they were.
    for id in REPOSITORIES {
        assert_eq!(
            remote_branch(
                &fixture.root(id),
                record["repositories"][id]["branch"].as_str().unwrap()
            )
            .as_deref(),
            set["repository_commits"][id].as_str()
        );
    }
    assert_eq!(
        harness.candidate(&run_id).await["evidence"]["status"],
        "reviewed"
    );
    assert_no_merge_requests(&harness.fake);
    assert_eq!(fixture.snapshot(), before);
}

#[tokio::test]
async fn a_manual_edit_in_one_repositorys_worktree_refuses_the_whole_set_naming_the_repository() {
    let harness = Harness::new().await;
    let dir = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(dir.path());
    let (run_id, _ticket, set) = harness.reviewed_set(&fixture).await;
    let before = fixture.snapshot();
    let hash = set["hash"].clone();
    let worktree = harness
        .worktree(
            &run_id,
            "shared",
            set["repository_commits"]["shared"].as_str().unwrap(),
        )
        .await;
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
    assert!(reason.contains("repository shared:"), "{reason}");
    assert!(reason.contains("src/main.py"), "{reason}");
    assert!(reason.contains("nothing was pushed"), "{reason}");
    assert_eq!(harness.requests("POST", "/pulls"), 0);
    assert_eq!(harness.requests("GET", "/pulls"), 0);
    for id in REPOSITORIES {
        assert!(remote_branch(&fixture.root(id), "luuna").is_none());
        let bare = bare_path(&fixture.root(id));
        let heads = git(Path::new(&bare), &["for-each-ref", "refs/heads"]);
        assert_eq!(heads.lines().count(), 1, "{id}: {heads}");
    }
    let (status, body) = harness.publication(&run_id).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    let view = harness.candidate(&run_id).await;
    assert_eq!(view["evidence"]["status"], "failed", "{view}");
    assert_eq!(view["evidence"]["content_drift"], true);
    assert_eq!(harness.run(&run_id).await["state"], "failed");
    let events = harness.events(&run_id).await;
    let refusal = events
        .iter()
        .find(|e| e["type"] == "publication.refused")
        .unwrap_or_else(|| panic!("{:?}", event_types(&events)));
    assert_eq!(refusal["payload"]["kind"], "candidate_drift");
    assert_eq!(refusal["payload"]["repository_id"], "shared");
    assert!(events.iter().any(|e| e["type"] == "candidate.invalidated"
        && e["payload"]["content_drift"] == true
        && e["payload"]["candidate_hash"] == hash));
    // Repair of a multi-repository set is re-planning; the view says so.
    assert_eq!(view["repair_allowed"], false);
    assert_no_merge_requests(&harness.fake);
    assert_eq!(fixture.snapshot(), before);
}
