//! One isolated test changes environment/cwd and restores both through a guard.
//! Its HOME, checkout, Git config and runtime data are private temporary folders.
use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
    Router,
};
use luuna_domain::{DirtyState, RepositoryIdentity, RepositoryObservation};
use luuna_runtime::{router, AppState, RuntimeConfig};
use luuna_storage::Store;
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
use std::process::Command;
fn git(home: &Path, repository: &Path, args: &[&str]) {
    let global = home.join("empty-gitconfig");
    std::fs::write(&global, "").unwrap();
    let mut command = Command::new("git");
    command
        .env_clear()
        .env("HOME", home)
        .env("GIT_CONFIG_GLOBAL", &global)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .arg("-C")
        .arg(repository)
        .args(args);
    for name in ["PATH", "SystemRoot", "WINDIR", "PATHEXT"] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    let output = command.output().expect("git runs");
    assert!(output.status.success(), "git {args:?} failed");
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
async fn route_actors_never_come_from_environment_git_or_checkout() {
    use luuna_runtime::roots::{AgentActor, RootsStore, SproutRequest};
    use roots_core::{
        config::{default_config, write_config},
        paths::roots_paths,
    };
    struct Restore {
        cwd: PathBuf,
        vars: Vec<(&'static str, Option<std::ffi::OsString>)>,
    }
    impl Drop for Restore {
        fn drop(&mut self) {
            std::env::set_current_dir(&self.cwd).unwrap();
            for (name, value) in &self.vars {
                if let Some(value) = value {
                    std::env::set_var(name, value)
                } else {
                    std::env::remove_var(name)
                }
            }
        }
    }
    let home = tempfile::tempdir().unwrap();
    let repository = tempfile::tempdir().unwrap();
    let names = [
        "ROOTS_USER",
        "ROOTS_AGENT",
        "LUUNA_ROOTS_USER",
        "LUUNA_ROOTS_AGENT",
        "USER",
        "USERNAME",
        "LOGNAME",
        "HOME",
        "XDG_CONFIG_HOME",
    ];
    let _restore = Restore {
        cwd: std::env::current_dir().unwrap(),
        vars: names.iter().map(|&n| (n, std::env::var_os(n))).collect(),
    };
    git(home.path(), repository.path(), &["init", "-q"]);
    git(
        home.path(),
        repository.path(),
        &["config", "user.name", "Mallory Gitname"],
    );
    let checkout = roots_paths(repository.path());
    write_config(&checkout, &default_config("checkout")).unwrap();
    fs::create_dir_all(&checkout.human).unwrap();
    let original = fs::read(&checkout.config).unwrap();
    for name in names {
        std::env::set_var(
            name,
            if name == "HOME" || name == "XDG_CONFIG_HOME" {
                home.path().as_os_str()
            } else {
                std::ffi::OsStr::new("mallory")
            },
        );
    }
    std::env::set_var("ROOTS_AGENT", "agent:intruder");
    std::env::set_current_dir(repository.path()).unwrap();
    let data = repository.path().join("data");
    let state = open(&data, repository.path());
    let app = router(state.clone());
    assert_eq!(
        request(&app, "POST", "app/intent", Some(json!({})), true)
            .await
            .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert!(!managed(&data).exists());
    assert_eq!(
        request(
            &app,
            "POST",
            "app/intent",
            Some(json!({"by":"agent:intruder"})),
            true
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
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
    seed_ideas(&data);
    let p = proposal(&state, "serves").await;
    let store = RootsStore::for_project(&state, "app").unwrap();
    let agent = AgentActor::for_run(&state, "app", "r1").unwrap();
    assert_eq!(agent.as_str(), "agent:run-r1");
    let sprout = store
        .file_sprout(
            &agent,
            SproutRequest {
                statement: "Keep all records together".into(),
                body: None,
                slug: None,
            },
        )
        .await
        .unwrap()
        .node
        .id;
    assert_eq!(
        request(
            &app,
            "POST",
            &format!("app/intent/proposals/{p}/accept"),
            Some(json!({"by":"agent:run-r1"})),
            true
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        request(
            &app,
            "POST",
            &format!("app/intent/proposals/{p}/accept"),
            Some(json!({})),
            true
        )
        .await
        .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        request(
            &app,
            "POST",
            &format!("app/intent/sprouts/{sprout}/reject"),
            Some(json!({"by":"agent:run-r1"})),
            true
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
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
        request(
            &app,
            "POST",
            &format!("app/intent/proposals/{p}/accept"),
            Some(json!({"by":"alice"})),
            true
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        request(
            &app,
            "POST",
            &format!("app/intent/sprouts/{sprout}/reject"),
            Some(json!({"by":"bob"})),
            true
        )
        .await
        .0,
        StatusCode::OK
    );
    let paths = roots_paths(data.join("components/projects/app"));
    let mut actors = vec![];
    let graph = roots_core::graph::read_graph(&paths).unwrap();
    for node in graph.nodes {
        actors.push(node.author);
        actors.extend(node.decided_by);
    }
    for edge in graph.edges {
        actors.push(edge.by);
        actors.extend(edge.proposed_by);
    }
    for p in roots_core::proposals::read_proposals(&paths).unwrap() {
        actors.push(p.by);
        actors.extend(p.decided_by);
    }
    for q in roots_core::questions::read_questions(&paths).unwrap() {
        actors.push(q.by);
    }
    for e in roots_core::events::read_events(&paths).unwrap() {
        actors.push(e.by);
    }
    for actor in actors {
        if actor.starts_with("human:") {
            assert!(["human:alice", "human:bob"].contains(&actor.as_str()));
        }
        assert!(!actor.to_lowercase().contains("mallory"));
    }
    assert_eq!(fs::read(&checkout.config).unwrap(), original);
    assert_eq!(fs::read_dir(&checkout.human).unwrap().count(), 0);
    let mut files = fs::read_dir(&checkout.dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect::<Vec<_>>();
    files.sort();
    assert_eq!(files, vec!["config.yaml", "human"]);
    assert!(!repository.path().join(".gitattributes").exists());
    assert!(!data.join(".gitattributes").exists());
}
