//! The native Roots module never takes an actor from the process environment,
//! from git configuration or from a checkout's `.luuna` folder.
//!
//! This is its own test binary with a single test, because it changes the
//! process environment and working directory. It creates its own temporary
//! git repository; its git calls see only that repository's configuration
//! (private HOME, an empty global configuration file, no system
//! configuration). No test needs a component toolchain, the network or a
//! model.
use luuna_domain::{DirtyState, RepositoryIdentity, RepositoryObservation};
use luuna_runtime::roots::{
    AcceptRequest, AgentActor, HumanActor, ProposalRequest, QuestionRequest, RootsStore,
    SproutRequest,
};
use luuna_runtime::{AppState, RuntimeConfig};
use luuna_storage::Store;
use roots_core::config::{default_config, write_config};
use roots_core::events::read_events;
use roots_core::graph::{graph_records, read_graph};
use roots_core::model::{
    Graph, IdeaStatus, NodeKind, NodeRecord, NodeStatus, ProposalKind, ProposalStatus,
};
use roots_core::paths::{prose_file_name, roots_paths, RootsPaths};
use roots_core::proposals::read_proposals;
use roots_core::prose::canonical_node_dir;
use roots_core::questions::read_questions;
use roots_core::store::write_jsonl_file;
use std::path::Path;
use std::process::Command;

const IDEA_A: &str = "r-a1b2";
const IDEA_B: &str = "r-c3d4";
const STATEMENT_A: &str = "Keep every project's data in one place";
const STATEMENT_B: &str = "Make backups simple";
const GIT_NAME: &str = "Mallory Gitname";

/// Run git in `repository` with only that repository's configuration.
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
        observed_at: "2026-11-12T00:00:00.000Z".into(),
    }
}

fn idea(id: &str, slug: &str) -> NodeRecord {
    // Seeded by a Roots rule, so any `human:` actor seen later was written
    // by an operation under test.
    NodeRecord::new(
        id.into(),
        NodeKind::Idea,
        slug.into(),
        NodeStatus::Idea(IdeaStatus::Planted),
        "roots:fixture".into(),
        "2026-01-01T00:00:00Z".into(),
    )
}

/// A Roots store set up the way Roots' own tests do: config, the human and
/// agent folders, and two ideas with their prose.
fn set_up_roots(paths: &RootsPaths) {
    write_config(paths, &default_config("app")).unwrap();
    for folder in [&paths.human, &paths.sprouts, &paths.notes] {
        std::fs::create_dir_all(folder).unwrap();
    }
    let nodes = vec![idea(IDEA_A, "one"), idea(IDEA_B, "two")];
    for (node, text) in nodes.iter().zip([STATEMENT_A, STATEMENT_B]) {
        let folder = canonical_node_dir(paths, node);
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(
            folder.join(prose_file_name(NodeKind::Idea)),
            format!("{text}\n"),
        )
        .unwrap();
    }
    let graph = Graph {
        nodes,
        edges: Vec::new(),
    };
    write_jsonl_file(&paths.graph, &graph_records(&graph)).unwrap();
}

/// Every actor any graph record, proposal, question or event carries.
fn recorded_actors(paths: &RootsPaths) -> Vec<String> {
    let mut actors = Vec::new();
    let graph = read_graph(paths).unwrap();
    for node in &graph.nodes {
        actors.push(node.author.clone());
        actors.extend(node.decided_by.clone());
    }
    for edge in &graph.edges {
        actors.push(edge.by.clone());
        actors.extend(edge.proposed_by.clone());
    }
    for proposal in read_proposals(paths).unwrap() {
        actors.push(proposal.by.clone());
        actors.extend(proposal.decided_by.clone());
    }
    for question in read_questions(paths).unwrap() {
        actors.push(question.by.clone());
    }
    for event in read_events(paths).unwrap() {
        actors.push(event.by.clone());
    }
    actors
}

#[tokio::test]
async fn no_environment_git_or_checkout_identity_reaches_roots() {
    let original_directory = std::env::current_dir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let repository = tempfile::tempdir().unwrap();
    let repository = repository.path();

    // The repository's own git identity looks like a human.
    git(home.path(), repository, &["init", "-q"]);
    git(home.path(), repository, &["config", "user.name", GIT_NAME]);

    // A checkout Roots store with a config and a human folder.
    let checkout_roots = roots_paths(repository);
    write_config(&checkout_roots, &default_config("checkout")).unwrap();
    std::fs::create_dir_all(&checkout_roots.human).unwrap();
    let checkout_config = std::fs::read(&checkout_roots.config).unwrap();

    // Every identity variable Roots' command line reads.
    std::env::set_var("ROOTS_USER", "mallory");
    std::env::set_var("LUUNA_ROOTS_USER", "human:mallory");
    std::env::set_var("ROOTS_AGENT", "human:mallory");
    std::env::set_var("LUUNA_ROOTS_AGENT", "agent:intruder");
    std::env::set_current_dir(repository).unwrap();

    // The data directory lives inside the repository.
    let data = repository.join("data");
    std::fs::create_dir_all(&data).unwrap();
    {
        let mut store = Store::open(data.join("luuna.sqlite3")).unwrap();
        store
            .register_project("app", "App", &[("api".into(), observation(repository))])
            .unwrap();
        store
            .create_run("r1", "app", serde_json::json!({}))
            .unwrap();
    }
    let managed = data.join("components").join("projects").join("app");
    let paths = roots_paths(&managed);
    set_up_roots(&paths);
    let state = AppState::open(RuntimeConfig {
        data_dir: data.clone(),
        pi_program: "python3".into(),
        pi_launcher_args: vec![],
        git_program: "git".into(),
        provider: None,
        model: None,
        token: "synthetic-test-token-with-at-least-32-bytes".into(),
        max_run_seconds: 10,
    })
    .unwrap()
    .without_components();

    // The store is the data-directory store, not the repository's.
    let store = RootsStore::for_project(&state, "app").unwrap();
    assert_eq!(store.store_dir(), paths.dir.as_path());
    assert_ne!(store.store_dir(), checkout_roots.dir.as_path());

    // Every reader and every agent operation.
    let agent = AgentActor::for_run(&state, "app", "r1").unwrap();
    assert_eq!(agent.as_str(), "agent:run-r1");
    let proposal = store
        .propose(
            &agent,
            ProposalRequest {
                kind: ProposalKind::Edge,
                nodes: vec![IDEA_A.into(), IDEA_B.into()],
                rel: Some("serves".into()),
                reason: Some("one place makes backups simple".into()),
                cites: vec![
                    format!("{IDEA_A}:{STATEMENT_A}"),
                    format!("{IDEA_B}:{STATEMENT_B}"),
                ],
                mention: false,
            },
        )
        .await
        .unwrap();
    assert_eq!(proposal.by, "agent:run-r1");
    let asked = store
        .ask(
            &agent,
            QuestionRequest {
                node: IDEA_A.into(),
                text: "Who is this for?".into(),
            },
        )
        .await
        .unwrap();
    assert_eq!(asked.question.by, "agent:run-r1");
    let filed = store
        .file_sprout(
            &agent,
            SproutRequest {
                statement: "Keep the store in the data directory".into(),
                body: None,
                slug: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(filed.node.author, "agent:run-r1");
    store.graph().await.unwrap();
    store.questions().await.unwrap();
    store.open_sprouts().await.unwrap();
    store.config().await.unwrap();
    let pending = store.pending_proposals().await.unwrap();

    // The agent's proposal is still pending, and nothing is human.
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].id, proposal.id);
    assert_eq!(pending[0].status, ProposalStatus::Pending);
    let actors = recorded_actors(&paths);
    assert!(
        actors.iter().all(|actor| !actor.starts_with("human:")),
        "{actors:?}"
    );

    // No agent string becomes a human.
    for name in [
        "agent:intruder",
        "agent:run-r1",
        "agent:mallory",
        agent.as_str(),
    ] {
        assert!(HumanActor::local_user(name).is_err(), "{name}");
    }

    // Accept is reached only through a runtime-built human.
    let human = HumanActor::local_user("alice").unwrap();
    let decision = store
        .accept_proposal(&human, &proposal.id, AcceptRequest::default())
        .await
        .unwrap();
    assert_eq!(decision.proposals[0].status, ProposalStatus::Accepted);
    assert_eq!(
        decision.proposals[0].decided_by.as_deref(),
        Some("human:alice")
    );
    assert_eq!(decision.edge.as_ref().unwrap().by, "human:alice");
    let humans: Vec<String> = recorded_actors(&paths)
        .into_iter()
        .filter(|actor| actor.starts_with("human:"))
        .collect();
    assert!(!humans.is_empty());
    assert!(
        humans.iter().all(|actor| actor == "human:alice"),
        "{humans:?}"
    );

    // The repository's Roots store was not touched.
    assert_eq!(
        std::fs::read(&checkout_roots.config).unwrap(),
        checkout_config
    );
    let mut entries: Vec<_> = std::fs::read_dir(&checkout_roots.dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    entries.sort();
    assert_eq!(entries, vec!["config.yaml", "human"]);
    assert!(std::fs::read_dir(&checkout_roots.human)
        .unwrap()
        .next()
        .is_none());

    drop(store);
    drop(state);
    std::env::set_current_dir(original_directory).unwrap();
    for name in [
        "ROOTS_USER",
        "LUUNA_ROOTS_USER",
        "ROOTS_AGENT",
        "LUUNA_ROOTS_AGENT",
    ] {
        std::env::remove_var(name);
    }
}
