//! Repository store mode against real temporary git checkouts (ADR 0061).
//! A project whose store mode file names a registered repository uses that
//! checkout's own `.luuna/<component>` stores, exactly as the developer
//! initialised them: the runtime never initialises, moves, re-permissions,
//! stages or commits anything there, and writes nothing outside
//! `.luuna/<component>`. Uses git and the native component libraries on
//! synthetic data; no network, toolchain or model.
use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
    Router,
};
use luuna_domain::{DirtyState, RepositoryIdentity, RepositoryObservation, StoreMode};
use luuna_runtime::{router, AppState, RuntimeConfig};
use luuna_storage::{write_project_store_mode, Store};
use serde_json::{json, Value};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};
use tower::ServiceExt;

const TOKEN: &str = "synthetic-test-token-with-at-least-32-bytes";
const STORES: [&str; 3] = ["missions", "atlas", "beacon"];
const UNUSABLE_CHECKOUT: &str = "This project's store mode is 'repository', but that repository's registered checkout cannot hold its stores: it must be a main checkout with a .git directory, without links, outside the data directory. No component data was read or written";

fn configuration(data_dir: PathBuf) -> RuntimeConfig {
    RuntimeConfig {
        data_dir,
        pi_program: if cfg!(windows) { "python" } else { "python3" }.into(),
        pi_launcher_args: vec![],
        git_program: "git".into(),
        provider: None,
        model: None,
        token: TOKEN.into(),
        max_run_seconds: 10,
    }
}

fn serve(data_dir: &Path) -> Router {
    router(AppState::open(configuration(data_dir.to_path_buf())).unwrap())
}

async fn request(
    app: &Router,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let builder = Request::builder()
        .method(method)
        .uri(path)
        .header("Authorization", format!("Bearer {TOKEN}"));
    let request = match body {
        Some(body) => builder
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap(),
        None => builder.body(Body::empty()).unwrap(),
    };
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 8 * 1024 * 1024)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
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
    String::from_utf8(output.stdout).unwrap()
}

fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

/// The developer's own initialisation, done elsewhere and copied in, so
/// nothing in the test checkout is written by an `init`.
async fn initialised_stores(components: &[&str]) -> tempfile::TempDir {
    let seed = tempfile::tempdir().unwrap();
    let root = seed.path().join("seed");
    let init = vec!["init".to_owned()];
    for component in components {
        match *component {
            "missions" => luuna_components::call_missions_native(&root, &init).await,
            "atlas" => luuna_components::call_atlas_native(&root, &init).await,
            _ => luuna_components::call_beacon_native(&root, &init).await,
        }
        .unwrap();
    }
    seed
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Tracking {
    Ignored,
    Untracked,
    Committed,
}

/// A main checkout with a first commit and the developer's stores.
async fn checkout(parent: &Path, name: &str, tracking: Tracking, components: &[&str]) -> PathBuf {
    let dir = parent.join(name);
    fs::create_dir_all(&dir).unwrap();
    git(&dir, &["init", "-q", "-b", "main"]);
    git(&dir, &["config", "commit.gpgsign", "false"]);
    fs::write(dir.join("README.md"), "fixture\n").unwrap();
    if tracking == Tracking::Ignored {
        fs::write(dir.join(".gitignore"), ".luuna/\n").unwrap();
    }
    git(&dir, &["add", "."]);
    git(&dir, &["commit", "-q", "-m", "first"]);
    let seed = initialised_stores(components).await;
    copy_tree(&seed.path().join("seed/.luuna"), &dir.join(".luuna"));
    if tracking == Tracking::Committed {
        git(&dir, &["add", ".luuna"]);
        git(&dir, &["commit", "-q", "-m", "stores"]);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
    }
    dir.canonicalize().unwrap()
}

fn observation(local_path: &Path) -> RepositoryObservation {
    RepositoryObservation {
        local_path: local_path.to_string_lossy().into_owned(),
        identity: RepositoryIdentity {
            remote: Some("https://example.invalid/app.git".into()),
            push_remote: None,
            root_commits: vec!["1".repeat(40)],
            shallow: false,
        },
        head_commit: "2".repeat(40),
        branch: Some("main".into()),
        remote_default_branch: None,
        local_branches: vec!["main".into()],
        dirty: DirtyState::default(),
        git_version: "git version 2.43.0".into(),
        observed_at: "2026-11-26T00:00:00.000Z".into(),
    }
}

/// Register `app` with repository `api` at `checkout` and select it.
fn select(data: &Path, checkout: &Path) {
    let mut store = Store::open(data.join("luuna.sqlite3")).unwrap();
    store
        .register_project("app", "App", &[("api".into(), observation(checkout))])
        .unwrap();
    write_project_store_mode(
        data,
        "app",
        &StoreMode::Repository {
            repository: "api".into(),
        },
    )
    .unwrap();
}

/// Every path outside `.git` and `.luuna/<component>`, with its bytes.
fn outside_stores(root: &Path) -> Vec<(String, Vec<u8>)> {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<(String, Vec<u8>)>) {
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            let relative = path
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            if relative == ".git" || STORES.iter().any(|c| relative == format!(".luuna/{c}")) {
                continue;
            }
            let kind = fs::symlink_metadata(&path).unwrap().file_type();
            if kind.is_dir() {
                out.push((relative, b"dir".to_vec()));
                walk(root, &path, out);
            } else {
                out.push((relative, fs::read(&path).unwrap()));
            }
        }
    }
    let mut out = vec![];
    walk(root, root, &mut out);
    out.sort();
    out
}

fn contains(dir: &Path, needle: &str) -> bool {
    fs::read_dir(dir).unwrap().any(|entry| {
        let path = entry.unwrap().path();
        if path.is_dir() {
            contains(&path, needle)
        } else {
            fs::read(&path)
                .map(|bytes| String::from_utf8_lossy(&bytes).contains(needle))
                .unwrap_or(false)
        }
    })
}

/// Enable, record a memory, create a ticket and list prompts, all against
/// the checkout's stores.
async fn use_the_stores(app: &Router, checkout: &Path) {
    let (status, body) = request(app, "POST", "/api/v1/projects/app/components", None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["workspace_path"], checkout.to_string_lossy().as_ref());
    let (status, body) = request(
        app,
        "POST",
        "/api/v1/projects/app/memory",
        Some(json!({
            "idempotency_key": "memory-in-repository",
            "scope": "project",
            "content": "Synthetic repository memory marker",
            "provenance": "test",
        })),
    )
    .await;
    assert!(status.is_success(), "{status}: {body}");
    let (status, body) = request(app, "GET", "/api/v1/projects/app/memory", None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body
        .to_string()
        .contains("Synthetic repository memory marker"));
    let (status, body) = request(
        app,
        "POST",
        "/api/v1/projects/app/tickets",
        Some(json!({
            "idempotency_key": "ticket-in-repository",
            "title": "Synthetic repository ticket marker",
            "intent": "Prove the repository store.",
            "acceptance_criteria": ["It is stored in the checkout."],
            "repository_ids": ["api"],
        })),
    )
    .await;
    assert!(status.is_success(), "{status}: {body}");
    let (status, body) = request(app, "GET", "/api/v1/projects/app/prompts", None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(contains(
        &checkout.join(".luuna/atlas"),
        "Synthetic repository memory marker"
    ));
    assert!(contains(
        &checkout.join(".luuna/missions"),
        "Synthetic repository ticket marker"
    ));
}

fn assert_left_alone(data: &Path, checkout: &Path, before: &[(String, Vec<u8>)], head: &str) {
    assert_eq!(outside_stores(checkout), before);
    assert_eq!(
        git(checkout, &["rev-parse", "HEAD"]),
        head,
        "nothing committed"
    );
    assert_eq!(
        git(checkout, &["diff", "--cached", "--name-only"]),
        "",
        "nothing staged"
    );
    assert!(!checkout.join(".luuna-component-home").exists());
    assert!(!checkout.join(".gitattributes").exists());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(checkout).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o755, "checkout permissions unchanged");
    }
    assert!(!data.join("components/projects/app").exists());
}

#[tokio::test]
async fn an_ignored_store_is_used_in_place_and_the_checkout_stays_clean() {
    let parent = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let checkout = checkout(parent.path(), "app", Tracking::Ignored, &STORES).await;
    select(data.path(), &checkout);
    let before = outside_stores(&checkout);
    let head = git(&checkout, &["rev-parse", "HEAD"]);
    let app = serve(data.path());
    use_the_stores(&app, &checkout).await;
    drop(app);
    assert_left_alone(data.path(), &checkout, &before, &head);
    assert_eq!(git(&checkout, &["status", "--porcelain"]), "");
}

#[tokio::test]
async fn an_untracked_store_stays_untracked_for_the_developer_to_ignore_or_commit() {
    let parent = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let checkout = checkout(parent.path(), "app", Tracking::Untracked, &STORES).await;
    select(data.path(), &checkout);
    let before = outside_stores(&checkout);
    let head = git(&checkout, &["rev-parse", "HEAD"]);
    let app = serve(data.path());
    use_the_stores(&app, &checkout).await;
    drop(app);
    assert_left_alone(data.path(), &checkout, &before, &head);
    assert_eq!(git(&checkout, &["status", "--porcelain"]), "?? .luuna/\n");
}

#[tokio::test]
async fn a_committed_store_changes_only_as_unstaged_edits() {
    let parent = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let checkout = checkout(parent.path(), "app", Tracking::Committed, &STORES).await;
    select(data.path(), &checkout);
    let before = outside_stores(&checkout);
    let head = git(&checkout, &["rev-parse", "HEAD"]);
    let app = serve(data.path());
    use_the_stores(&app, &checkout).await;
    drop(app);
    assert_left_alone(data.path(), &checkout, &before, &head);
    let status = git(&checkout, &["status", "--porcelain"]);
    assert!(!status.is_empty());
    for line in status.lines() {
        assert!(
            (line.starts_with(" M .luuna/") || line.starts_with("?? .luuna/")),
            "{line}"
        );
    }
}

#[tokio::test]
async fn a_store_the_developer_has_not_initialised_is_refused_and_never_created() {
    let parent = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let checkout = checkout(
        parent.path(),
        "app",
        Tracking::Ignored,
        &["atlas", "beacon"],
    )
    .await;
    select(data.path(), &checkout);
    let before = outside_stores(&checkout);
    let app = serve(data.path());
    let (status, body) = request(&app, "POST", "/api/v1/projects/app/components", None).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    let text = body["error"].as_str().unwrap();
    assert!(text.contains("initialise .luuna/missions"), "{text}");
    assert!(!text.contains(&*checkout.to_string_lossy()), "{text}");
    drop(app);
    assert!(!checkout.join(".luuna/missions").exists());
    assert_eq!(outside_stores(&checkout), before);
    let store = Store::open(data.path().join("luuna.sqlite3")).unwrap();
    assert_eq!(store.component_state("app").unwrap(), None);
}

#[tokio::test]
async fn a_linked_worktree_or_an_overlapping_data_folder_is_refused() {
    let parent = tempfile::tempdir().unwrap();
    let main = checkout(parent.path(), "app", Tracking::Ignored, &STORES).await;
    git(&main, &["worktree", "add", "-q", "../linked"]);
    let linked = parent.path().join("linked").canonicalize().unwrap();
    copy_tree(&main.join(".luuna"), &linked.join(".luuna"));
    let data = tempfile::tempdir().unwrap();
    select(data.path(), &linked);
    {
        // Enabled earlier, so every route reaches the resolver.
        let store = Store::open(data.path().join("luuna.sqlite3")).unwrap();
        store.begin_component_enable("app").unwrap();
        store.finish_component_enable("app").unwrap();
    }
    let before = outside_stores(&linked);
    let app = serve(data.path());
    let ticket = json!({
        "idempotency_key": "ticket-refused",
        "title": "Refused ticket",
        "intent": "Prove the refusal.",
        "acceptance_criteria": ["refused"],
        "repository_ids": ["api"],
    });
    for (method, path, body) in [
        ("GET", "/api/v1/projects/app/components", None),
        ("GET", "/api/v1/projects/app/memory", None),
        ("GET", "/api/v1/projects/app/prompts", None),
        ("POST", "/api/v1/projects/app/tickets", Some(ticket)),
    ] {
        let (status, body) = request(&app, method, path, body).await;
        assert_eq!(status, StatusCode::CONFLICT, "{path}: {body}");
        assert_eq!(body["error"], UNUSABLE_CHECKOUT, "{path}");
    }
    drop(app);
    assert_eq!(outside_stores(&linked), before);
    // The data folder inside the checkout.
    let inside = main.join("luuna-data");
    fs::create_dir_all(&inside).unwrap();
    select(&inside, &main);
    let app = serve(&inside);
    let (status, body) = request(&app, "GET", "/api/v1/projects/app/components", None).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["error"], UNUSABLE_CHECKOUT);
}

#[cfg(unix)]
#[tokio::test]
async fn a_linked_store_folder_is_refused() {
    let parent = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let checkout = checkout(parent.path(), "app", Tracking::Ignored, &STORES).await;
    fs::rename(checkout.join(".luuna"), parent.path().join("elsewhere")).unwrap();
    std::os::unix::fs::symlink(parent.path().join("elsewhere"), checkout.join(".luuna")).unwrap();
    select(data.path(), &checkout);
    let app = serve(data.path());
    let (status, body) = request(&app, "GET", "/api/v1/projects/app/components", None).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["error"], UNUSABLE_CHECKOUT);
}

#[tokio::test]
async fn the_store_follows_a_rebind_to_the_new_checkout() {
    let parent = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let first = checkout(parent.path(), "first", Tracking::Ignored, &STORES).await;
    let second = checkout(parent.path(), "second", Tracking::Ignored, &STORES).await;
    select(data.path(), &first);
    let app = serve(data.path());
    let (_, body) = request(&app, "GET", "/api/v1/projects/app/components", None).await;
    assert_eq!(body["workspace_path"], first.to_string_lossy().as_ref());
    drop(app);
    Store::open(data.path().join("luuna.sqlite3"))
        .unwrap()
        .rebind_repository("app", "api", &observation(&second))
        .unwrap();
    let before = outside_stores(&first);
    let app = serve(data.path());
    let (_, body) = request(&app, "GET", "/api/v1/projects/app/components", None).await;
    assert_eq!(body["workspace_path"], second.to_string_lossy().as_ref());
    drop(app);
    assert_eq!(outside_stores(&first), before);
}

fn register_only(data: &Path, checkout: &Path) {
    Store::open(data.join("luuna.sqlite3"))
        .unwrap()
        .register_project("app", "App", &[("api".into(), observation(checkout))])
        .unwrap();
}

fn store_file(data: &Path) -> Option<Vec<u8>> {
    fs::read(data.join("projects/app/store.json")).ok()
}

async fn mode_changes(app: &Router) -> usize {
    let (_, body) = request(app, "GET", "/api/v1/projects/app/events", None).await;
    body.to_string()
        .matches("project.store_mode.changed")
        .count()
}

#[tokio::test]
async fn the_store_mode_is_shown_and_chosen_from_the_product() {
    let parent = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let checkout = checkout(parent.path(), "app", Tracking::Ignored, &STORES).await;
    register_only(data.path(), &checkout);
    let app = serve(data.path());
    let (status, body) = request(&app, "GET", "/api/v1/projects/app/store-mode", None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["mode"], "runtime");
    assert_eq!(body["usable"], true);
    assert_eq!(
        body["choices"],
        json!([
            {"mode": "runtime", "repository": null},
            {"mode": "repository", "repository": "api"},
        ])
    );
    let choose = json!({"mode": "repository", "repository": "api"});
    let (status, body) = request(
        &app,
        "PUT",
        "/api/v1/projects/app/store-mode",
        Some(choose.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["mode"], "repository");
    assert_eq!(body["repository"], "api");
    assert_eq!(body["usable"], true);
    let (_, status_body) = request(&app, "GET", "/api/v1/projects/app/components", None).await;
    assert_eq!(
        status_body["workspace_path"],
        checkout.to_string_lossy().as_ref()
    );
    assert_eq!(mode_changes(&app).await, 1);
    // The same choice again changes nothing.
    let written = store_file(data.path());
    let (status, _) = request(&app, "PUT", "/api/v1/projects/app/store-mode", Some(choose)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(store_file(data.path()), written);
    assert_eq!(mode_changes(&app).await, 1);
    // And back to the runtime store, whose side is still empty.
    let (status, body) = request(
        &app,
        "PUT",
        "/api/v1/projects/app/store-mode",
        Some(json!({"mode": "runtime"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["mode"], "runtime");
    assert_eq!(mode_changes(&app).await, 2);
}

#[tokio::test]
async fn a_change_that_would_strand_or_merge_records_is_refused() {
    let parent = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let checkout = checkout(parent.path(), "app", Tracking::Ignored, &STORES).await;
    register_only(data.path(), &checkout);
    let app = serve(data.path());
    let put = |body: Value| {
        let app = app.clone();
        async move { request(&app, "PUT", "/api/v1/projects/app/store-mode", Some(body)).await }
    };
    // Invalid or unavailable choices.
    for (body, expected) in [
        (
            json!({"mode": "both", "repository": "api"}),
            StatusCode::CONFLICT,
        ),
        (
            json!({"mode": "repository", "repository": "ghost"}),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (
            json!({"mode": "repository"}),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (
            json!({"mode": "runtime", "repository": "api"}),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (
            json!({"mode": "sideways"}),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
    ] {
        let (status, response) = put(body.clone()).await;
        assert_eq!(status, expected, "{body}: {response}");
        assert_eq!(store_file(data.path()), None, "{body}");
    }
    // The checkout's Missions store already holds a record.
    let issues = checkout.join(".luuna/missions/issues.jsonl");
    fs::write(&issues, "{\"id\":\"someone-else\"}\n").unwrap();
    let (status, body) = put(json!({"mode": "repository", "repository": "api"})).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(body["error"]
        .as_str()
        .unwrap()
        .contains("missions store already holds records"));
    assert_eq!(store_file(data.path()), None);
    fs::write(&issues, "").unwrap();
    // Memory recorded in the runtime store would be left behind.
    let (status, body) = request(&app, "POST", "/api/v1/projects/app/components", None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/projects/app/memory",
        Some(json!({
            "idempotency_key": "runtime-memory",
            "scope": "project",
            "content": "Kept in the runtime store",
            "provenance": "test",
        })),
    )
    .await;
    assert!(status.is_success(), "{status}: {body}");
    let (status, body) = put(json!({"mode": "repository", "repository": "api"})).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(body["error"]
        .as_str()
        .unwrap()
        .contains("never moves or merges"));
    assert_eq!(store_file(data.path()), None);
    assert_eq!(mode_changes(&app).await, 0);
}

#[cfg(unix)]
#[tokio::test]
async fn a_link_inside_a_component_store_is_refused_before_it_is_read() {
    let parent = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let checkout = checkout(parent.path(), "app", Tracking::Ignored, &STORES).await;
    // A Roots store that is a link to a store elsewhere.
    let elsewhere = parent.path().join("other-roots");
    fs::create_dir_all(&elsewhere).unwrap();
    fs::write(elsewhere.join("config.yaml"), "outside: true\n").unwrap();
    std::os::unix::fs::symlink(&elsewhere, checkout.join(".luuna/roots")).unwrap();
    // A link below a real Atlas store.
    std::os::unix::fs::symlink(&elsewhere, checkout.join(".luuna/atlas/linked")).unwrap();
    select(data.path(), &checkout);
    let app = serve(data.path());
    let (status, body) = request(&app, "GET", "/api/v1/projects/app/intent", None).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(
        body["error"]
            .as_str()
            .unwrap()
            .contains("no usable roots store"),
        "{body}"
    );
    let (status, body) = request(&app, "POST", "/api/v1/projects/app/components", None).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(
        body["error"]
            .as_str()
            .unwrap()
            .contains("no usable atlas store"),
        "{body}"
    );
    drop(app);
    // Nothing was written through either link.
    assert_eq!(fs::read_dir(&elsewhere).unwrap().count(), 1);
    assert_eq!(
        fs::read_to_string(elsewhere.join("config.yaml")).unwrap(),
        "outside: true\n"
    );
}

#[tokio::test]
async fn a_component_write_with_no_recorded_outcome_holds_the_store_mode() {
    let parent = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let checkout = checkout(parent.path(), "app", Tracking::Ignored, &STORES).await;
    register_only(data.path(), &checkout);
    {
        // A memory write that reached Atlas before the runtime stopped.
        let store = Store::open(data.path().join("luuna.sqlite3")).unwrap();
        store
            .begin_component_operation(
                "app",
                "memory-cut-short",
                "memory.create",
                &json!({"scope": "project", "content": "x", "provenance": "test"}),
                "mem-cut-short",
            )
            .unwrap();
        store
            .mark_component_operation_attempted("app", "memory-cut-short")
            .unwrap();
    }
    let app = serve(data.path());
    let (status, body) = request(
        &app,
        "PUT",
        "/api/v1/projects/app/store-mode",
        Some(json!({"mode": "repository", "repository": "api"})),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    let text = body["error"].as_str().unwrap();
    assert!(
        text.contains("memory.create") && text.contains("retry it"),
        "{text}"
    );
    assert_eq!(store_file(data.path()), None);
}
