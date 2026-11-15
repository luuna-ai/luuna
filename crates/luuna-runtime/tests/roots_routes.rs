//! Every case uses private temporary data and checkout folders, passed explicitly.
//! No process environment or working directory is changed and no git/model is run.
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
};
use tower::ServiceExt;
const TOKEN: &str = "synthetic-intent-token-at-least-32-bytes";
fn observation(path: &Path) -> RepositoryObservation {
    RepositoryObservation {
        local_path: path.to_string_lossy().into_owned(),
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
        observed_at: "2026-11-15T00:00:00Z".into(),
    }
}
fn open(data: &Path, checkout: &Path) -> AppState {
    let mut store = Store::open(data.join("luuna.sqlite3")).unwrap();
    store
        .register_project("app", "App", &[("api".into(), observation(checkout))])
        .unwrap();
    store.create_run("r1", "app", json!({})).unwrap();
    drop(store);
    AppState::open(RuntimeConfig {
        data_dir: data.into(),
        pi_program: "unused-python".into(),
        pi_launcher_args: vec![],
        git_program: "unused-git".into(),
        provider: None,
        model: None,
        token: TOKEN.into(),
        max_run_seconds: 10,
    })
    .unwrap()
    .without_components()
}
fn managed(data: &Path) -> PathBuf {
    data.join("components/projects/app/.luuna/roots")
}
async fn request(
    app: &Router,
    method: &str,
    suffix: &str,
    body: Option<Value>,
    auth: bool,
) -> (StatusCode, Value) {
    let mut req = Request::builder()
        .method(method)
        .uri(format!("/api/v1/projects/{suffix}"));
    if auth {
        req = req.header("Authorization", format!("Bearer {TOKEN}"));
    }
    let body = if let Some(body) = body {
        req = req.header("content-type", "application/json");
        Body::from(body.to_string())
    } else {
        Body::empty()
    };
    let res = app.clone().oneshot(req.body(body).unwrap()).await.unwrap();
    let status = res.status();
    let bytes = to_bytes(res.into_body(), 8 * 1024 * 1024).await.unwrap();
    (
        status,
        serde_json::from_slice(&bytes)
            .unwrap_or_else(|_| json!({"text":String::from_utf8_lossy(&bytes)})),
    )
}
fn snapshot(root: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    fn walk(base: &Path, p: &Path, out: &mut Vec<(PathBuf, Vec<u8>)>) {
        if !p.exists() {
            return;
        }
        for entry in fs::read_dir(p).unwrap() {
            let p = entry.unwrap().path();
            if p.is_dir() {
                walk(base, &p, out)
            } else {
                out.push((p.strip_prefix(base).unwrap().into(), fs::read(p).unwrap()))
            }
        }
    }
    let mut out = vec![];
    walk(root, root, &mut out);
    out.sort();
    out
}
#[tokio::test]
async fn every_intent_route_requires_a_token_and_no_agent_route_exists() {
    let data = tempfile::tempdir().unwrap();
    let checkout = tempfile::tempdir().unwrap();
    let app = router(open(data.path(), checkout.path()));
    for (method, path) in [
        ("GET", "app/intent"),
        ("POST", "app/intent"),
        ("GET", "app/intent/proposals"),
        ("GET", "app/intent/questions"),
        ("GET", "app/intent/sprouts"),
        ("POST", "app/intent/proposals/p-1/accept"),
        ("POST", "app/intent/proposals/p-1/reject"),
        ("POST", "app/intent/sprouts/r-1/reject"),
    ] {
        assert_eq!(
            request(&app, method, path, Some(json!({"by":"alice"})), false)
                .await
                .0,
            StatusCode::UNAUTHORIZED,
            "{method} {path}"
        );
    }
    assert!(!managed(data.path()).exists());
    for path in [
        "app/intent/proposals",
        "app/intent/questions",
        "app/intent/sprouts",
    ] {
        assert_eq!(
            request(&app, "POST", path, Some(json!({})), true).await.0,
            StatusCode::METHOD_NOT_ALLOWED
        );
    }
}
#[tokio::test]
async fn missing_store_is_refused_and_init_has_exact_roots_layout_and_is_not_repeated() {
    let data = tempfile::tempdir().unwrap();
    let checkout = tempfile::tempdir().unwrap();
    let app = router(open(data.path(), checkout.path()));
    for (method, path) in [
        ("GET", "app/intent"),
        ("GET", "app/intent/proposals"),
        ("GET", "app/intent/questions"),
        ("GET", "app/intent/sprouts"),
        ("POST", "app/intent/proposals/p-1/accept"),
    ] {
        let (status, body) = request(&app, method, path, Some(json!({"by":"alice"})), true).await;
        assert_eq!(status, StatusCode::CONFLICT, "{body}");
        assert_eq!(body["error"], "Roots is not set up for this project");
    }
    let root = managed(data.path());
    assert!(!root.exists());
    let (status, body) = request(
        &app,
        "POST",
        "app/intent",
        Some(json!({"by":"alice"})),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["settings"]["project"], "app");
    assert_eq!(body["nodes"], json!([]));
    assert_eq!(body["edges"], json!([]));
    assert_eq!(
        fs::read_to_string(root.join("config.yaml")).unwrap(),
        roots_core::config::render_initial_config(&roots_core::config::default_config("app"))
    );
    assert_eq!(
        fs::read_to_string(root.join(".gitignore")).unwrap(),
        "*.lock\n*.tmp.*\n"
    );
    for name in ["graph.jsonl", "proposals.jsonl", "questions.jsonl"] {
        assert!(fs::read(root.join(name)).unwrap().is_empty());
    }
    for name in ["human", "agent/sprouts", "agent/notes"] {
        assert!(root.join(name).is_dir());
    }
    let mut entries = fs::read_dir(&root)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect::<Vec<_>>();
    entries.sort();
    assert_eq!(
        entries,
        vec![
            ".gitignore",
            "agent",
            "config.yaml",
            "events.jsonl",
            "graph.jsonl",
            "human",
            "proposals.jsonl",
            "questions.jsonl"
        ]
    );
    let events = fs::read_to_string(root.join("events.jsonl")).unwrap();
    let events = events
        .lines()
        .map(|l| serde_json::from_str::<Value>(l).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["type"], "init");
    assert_eq!(events[0]["by"], "human:alice");
    assert_eq!(events[0]["project"], "app");
    let before = snapshot(&root);
    assert_eq!(
        request(&app, "POST", "app/intent", Some(json!({"by":"bob"})), true)
            .await
            .0,
        StatusCode::CONFLICT
    );
    assert_eq!(before, snapshot(&root));
    for path in [
        "app/intent/proposals",
        "app/intent/questions",
        "app/intent/sprouts",
    ] {
        let (s, b) = request(&app, "GET", path, None, true).await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(b, json!([]));
    }
    assert!(snapshot(data.path())
        .iter()
        .all(|(p, _)| p.file_name().unwrap() != ".gitattributes"));
}
#[tokio::test]
async fn human_name_is_required_and_cannot_be_an_agent_or_roots() {
    let data = tempfile::tempdir().unwrap();
    let checkout = tempfile::tempdir().unwrap();
    let app = router(open(data.path(), checkout.path()));
    for (body, expected) in [
        (json!({}), 422),
        (json!({"by":""}), 422),
        (json!({"by":"x".repeat(201)}), 422),
        (json!({"by":"agent:run-1"}), 403),
        (json!({"by":"agent:x"}), 403),
        (json!({"by":"roots"}), 403),
        (json!({"by":"roots:rule"}), 403),
        (json!({"by":"alice","actor":"human:alice"}), 422),
    ] {
        assert_eq!(
            request(&app, "POST", "app/intent", Some(body), true)
                .await
                .0
                .as_u16(),
            expected
        );
        assert!(!managed(data.path()).exists());
    }
}
#[tokio::test]
async fn mode_and_registry_refusals_create_no_component_data() {
    let data = tempfile::tempdir().unwrap();
    let checkout = tempfile::tempdir().unwrap();
    let state = open(data.path(), checkout.path());
    write_project_store_mode(
        data.path(),
        "app",
        &StoreMode::Repository {
            repository: "api".into(),
        },
    )
    .unwrap();
    let app = router(state);
    let before = snapshot(checkout.path());
    for method in ["GET", "POST"] {
        assert_eq!(
            request(
                &app,
                method,
                "app/intent",
                Some(json!({"by":"alice"})),
                true
            )
            .await
            .0,
            StatusCode::CONFLICT
        );
    }
    assert!(!data.path().join("components").exists());
    assert_eq!(before, snapshot(checkout.path()));
    for id in ["missing", "invalid!id"] {
        for method in ["GET", "POST"] {
            assert_eq!(
                request(
                    &app,
                    method,
                    &format!("{id}/intent"),
                    Some(json!({"by":"alice"})),
                    true
                )
                .await
                .0,
                StatusCode::NOT_FOUND
            );
        }
    }
}
#[cfg(unix)]
#[tokio::test]
async fn init_refuses_links_at_every_managed_directory_and_existing_file() {
    use std::os::unix::fs::symlink;
    for suffix in [
        "components",
        "components/projects",
        "components/projects/app",
        "components/projects/app/.luuna",
        "components/projects/app/.luuna/roots",
        "components/projects/app/.luuna/roots/events.jsonl",
    ] {
        let data = tempfile::tempdir().unwrap();
        let checkout = tempfile::tempdir().unwrap();
        let external = tempfile::tempdir().unwrap();
        let state = open(data.path(), checkout.path());
        let link = data.path().join(suffix);
        fs::create_dir_all(link.parent().unwrap()).unwrap();
        let target = if suffix.ends_with("jsonl") {
            let p = external.path().join("untouched");
            fs::write(&p, "").unwrap();
            p
        } else {
            external.path().to_path_buf()
        };
        symlink(&target, &link).unwrap();
        let before = snapshot(external.path());
        let (status, body) = request(
            &router(state),
            "POST",
            "app/intent",
            Some(json!({"by":"alice"})),
            true,
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT, "{suffix}: {body}");
        assert!(fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(fs::read_link(&link).unwrap(), target);
        assert_eq!(before, snapshot(external.path()));
    }
}
fn seed_ideas(data: &Path) {
    use roots_core::{
        graph::graph_records,
        model::{Graph, IdeaStatus, NodeKind, NodeRecord, NodeStatus},
        paths::{prose_file_name, roots_paths},
        prose::canonical_node_dir,
        store::write_jsonl_file,
    };
    let paths = roots_paths(data.join("components/projects/app"));
    let nodes = [
        ("r-a1b2", "one", "Keep every project's data in one place"),
        ("r-c3d4", "two", "Make backups simple"),
    ]
    .into_iter()
    .map(|(id, slug, text)| {
        let node = NodeRecord::new(
            id.into(),
            NodeKind::Idea,
            slug.into(),
            NodeStatus::Idea(IdeaStatus::Planted),
            "roots:fixture".into(),
            "2026-01-01T00:00:00Z".into(),
        );
        let dir = canonical_node_dir(&paths, &node);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join(prose_file_name(NodeKind::Idea)),
            format!("{text}\n"),
        )
        .unwrap();
        node
    })
    .collect();
    write_jsonl_file(
        &paths.graph,
        &graph_records(&Graph {
            nodes,
            edges: vec![],
        }),
    )
    .unwrap();
}
async fn proposal(state: &AppState, rel: &str) -> String {
    use luuna_runtime::roots::{AgentActor, ProposalRequest, RootsStore};
    RootsStore::for_project(state, "app")
        .unwrap()
        .propose(
            &AgentActor::for_run(state, "app", "r1").unwrap(),
            ProposalRequest {
                kind: roots_core::model::ProposalKind::Edge,
                nodes: vec!["r-a1b2".into(), "r-c3d4".into()],
                rel: Some(rel.into()),
                reason: Some("One place makes backups simple".into()),
                cites: vec![
                    "r-a1b2:Keep every project's data in one place".into(),
                    "r-c3d4:Make backups simple".into(),
                ],
                mention: false,
            },
        )
        .await
        .unwrap()
        .id
}
#[tokio::test]
async fn only_explicit_human_decisions_change_agent_proposals_and_sprouts() {
    use luuna_runtime::roots::{AgentActor, RootsStore, SproutRequest};
    let data = tempfile::tempdir().unwrap();
    let checkout = tempfile::tempdir().unwrap();
    let state = open(data.path(), checkout.path());
    let app = router(state.clone());
    assert_eq!(
        request(
            &app,
            "POST",
            "app/intent",
            Some(json!({"by":"alice"})),
            true
        )
        .await
        .0,
        StatusCode::OK
    );
    seed_ideas(data.path());
    let p = proposal(&state, "serves").await;
    let sprout = RootsStore::for_project(&state, "app")
        .unwrap()
        .file_sprout(
            &AgentActor::for_run(&state, "app", "r1").unwrap(),
            SproutRequest {
                statement: "Keep the store in the data directory".into(),
                body: None,
                slug: None,
            },
        )
        .await
        .unwrap()
        .node
        .id;
    for path in [
        format!("app/intent/proposals/{p}/accept"),
        format!("app/intent/proposals/{p}/reject"),
        format!("app/intent/sprouts/{sprout}/reject"),
    ] {
        for (body, status) in [
            (json!({"by":"agent:run-r1"}), 403),
            (json!({"by":"alice","actor":"human:alice"}), 422),
            (json!({"by":"alice","as":"human:alice"}), 422),
            (json!({}), 422),
        ] {
            assert_eq!(
                request(&app, "POST", &path, Some(body), true)
                    .await
                    .0
                    .as_u16(),
                status
            );
        }
    }
    assert_eq!(
        request(&app, "GET", "app/intent/proposals", None, true)
            .await
            .1
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        request(&app, "GET", "app/intent/sprouts", None, true)
            .await
            .1
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let (s, b) = request(
        &app,
        "POST",
        &format!("app/intent/proposals/{p}/accept"),
        Some(json!({"by":"alice"})),
        true,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{b}");
    assert_eq!(b["proposals"][0]["decidedBy"], "human:alice");
    let p = proposal(&state, "tension").await;
    let (s, b) = request(
        &app,
        "POST",
        &format!("app/intent/proposals/{p}/reject"),
        Some(json!({"by":"alice","reason":"no"})),
        true,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{b}");
    let (s, b) = request(
        &app,
        "POST",
        &format!("app/intent/sprouts/{sprout}/reject"),
        Some(json!({"by":"bob"})),
        true,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{b}");
    assert_eq!(b["decidedBy"], "human:bob");
    assert_eq!(
        request(
            &app,
            "POST",
            "app/intent/proposals/p-missing/accept",
            Some(json!({"by":"alice"})),
            true
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
}
#[cfg(unix)]
#[tokio::test]
async fn errors_never_disclose_written_or_canonical_host_paths() {
    let root = tempfile::tempdir().unwrap();
    let real = tempfile::tempdir().unwrap();
    let link = root.path().join("link");
    std::os::unix::fs::symlink(real.path(), &link).unwrap();
    let data = link.join("data");
    let checkout = tempfile::tempdir().unwrap();
    let app = router(open(&data, checkout.path()));
    let mut responses = vec![
        request(&app, "GET", "missing/intent", None, true).await,
        request(&app, "GET", "app/intent/proposals", None, true).await,
        request(
            &app,
            "POST",
            "app/intent",
            Some(json!({"by":"agent:x"})),
            true,
        )
        .await,
        request(&app, "POST", "app/intent", Some(json!({"by":""})), true).await,
    ];
    assert_eq!(
        request(
            &app,
            "POST",
            "app/intent",
            Some(json!({"by":"alice"})),
            true
        )
        .await
        .0,
        StatusCode::OK
    );
    responses.extend([
        request(
            &app,
            "POST",
            "app/intent/proposals/p-missing/accept",
            Some(json!({"by":"alice"})),
            true,
        )
        .await,
        request(
            &app,
            "POST",
            "app/intent",
            Some(json!({"by":"alice"})),
            true,
        )
        .await,
        request(
            &app,
            "POST",
            "app/intent/proposals/p-missing/accept",
            Some(json!({"by":"agent:run-r1"})),
            true,
        )
        .await,
        request(
            &app,
            "POST",
            "app/intent/proposals/p-missing/accept",
            Some(json!({"by":"alice","actor":"human:alice"})),
            true,
        )
        .await,
    ]);
    for ((status, body), expected) in responses
        .into_iter()
        .zip([404, 409, 403, 422, 404, 409, 403, 422])
    {
        assert_eq!(status.as_u16(), expected, "{body}");
        let text = body.to_string();
        for path in [&data, &fs::canonicalize(&data).unwrap(), &link, real.path()] {
            assert!(!text.contains(path.to_str().unwrap()), "{text}");
        }
        assert!(!text.contains("/home/"));
        assert!(!text.contains("/tmp/"));
    }
}
