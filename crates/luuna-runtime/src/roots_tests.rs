//! Unit tests of the native Roots module: actor construction, the human and
//! agent operations on a data-directory store, and the error mapping. No test
//! needs git, a component toolchain, the network or a model.
use super::*;
use crate::RuntimeConfig;
use luuna_domain::{DirtyState, RepositoryIdentity, RepositoryObservation};
use roots_core::config::{default_config, write_config};
use roots_core::events::read_events;
use roots_core::graph::graph_records;
use roots_core::model::{Graph, IdeaStatus, NodeKind, NodeStatus, ProposalStatus, SproutStatus};
use roots_core::paths::prose_file_name;
use roots_core::prose::canonical_node_dir;
use roots_core::store::write_jsonl_file;
use serde_json::json;

const IDEA_A: &str = "r-a1b2";
const IDEA_B: &str = "r-c3d4";

fn observation(local_path: &Path) -> RepositoryObservation {
    RepositoryObservation {
        local_path: local_path.to_string_lossy().into_owned(),
        identity: RepositoryIdentity {
            remote: Some("https://example.invalid/fixture.git".into()),
            push_remote: None,
            root_commits: vec!["1".repeat(40)],
            shallow: false,
        },
        head_commit: "1".repeat(40),
        branch: Some("main".into()),
        remote_default_branch: None,
        local_branches: vec!["main".into()],
        dirty: DirtyState::default(),
        git_version: "git version 2.43.0".into(),
        observed_at: "2026-11-12T00:00:00Z".into(),
    }
}

/// A runtime over `root/data` with projects `app` and `other`, run `r1` of
/// `app` and run `r2` of `other`.
fn runtime(root: &Path) -> AppState {
    let state = AppState::open(RuntimeConfig {
        data_dir: root.join("data"),
        pi_program: "node".into(),
        git_program: "git".into(),
        pi_launcher_args: vec![],
        provider: None,
        model: None,
        token: "synthetic-roots-token-with-at-least-32-bytes".into(),
        max_run_seconds: 10,
    })
    .unwrap()
    .without_components();
    {
        let mut store = state.store.lock().unwrap();
        for id in ["app", "other"] {
            store
                .register_project(
                    id,
                    id,
                    &[("api".into(), observation(&root.join("checkout").join(id)))],
                )
                .unwrap();
        }
        store.create_run("r1", "app", json!({})).unwrap();
        store.create_run("r2", "other", json!({})).unwrap();
    }
    state
}

fn idea(id: &str, slug: &str) -> NodeRecord {
    NodeRecord::new(
        id.into(),
        NodeKind::Idea,
        slug.into(),
        NodeStatus::Idea(IdeaStatus::Planted),
        "human:fixture".into(),
        "2026-01-01T00:00:00Z".into(),
    )
}

/// Set up a Roots store in `project`'s data-directory store the way Roots'
/// own tests do: the config, the human and agent folders, and two ideas with
/// their prose.
fn set_up_roots(state: &AppState, project: &str) -> RootsPaths {
    let store = crate::stores::project_store(state, project).unwrap();
    let paths = roots_paths(&store);
    write_config(&paths, &default_config(project)).unwrap();
    for folder in [&paths.human, &paths.sprouts, &paths.notes] {
        std::fs::create_dir_all(folder).unwrap();
    }
    let nodes = vec![idea(IDEA_A, "one"), idea(IDEA_B, "two")];
    for (node, text) in nodes.iter().zip([STATEMENT_A, STATEMENT_B]) {
        let folder = canonical_node_dir(&paths, node);
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
    paths
}

const STATEMENT_A: &str = "Keep every project's data in one place";
const STATEMENT_B: &str = "Make backups simple";

fn edge_proposal() -> ProposalRequest {
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
    }
}

fn sprout() -> SproutRequest {
    SproutRequest {
        statement: "Keep the store in the data directory".into(),
        body: None,
        slug: None,
    }
}

#[test]
fn local_user_refuses_agents_roots_and_invalid_names() {
    for (name, status) in [
        ("agent:x", StatusCode::FORBIDDEN),
        ("agent:run-1", StatusCode::FORBIDDEN),
        ("agent:", StatusCode::FORBIDDEN),
        ("roots", StatusCode::FORBIDDEN),
        ("roots:rule", StatusCode::FORBIDDEN),
        ("", StatusCode::UNPROCESSABLE_ENTITY),
        ("human:", StatusCode::UNPROCESSABLE_ENTITY),
        ("bad name", StatusCode::UNPROCESSABLE_ENTITY),
        ("-alice", StatusCode::UNPROCESSABLE_ENTITY),
        ("human:a/b", StatusCode::UNPROCESSABLE_ENTITY),
        ("other:alice", StatusCode::UNPROCESSABLE_ENTITY),
    ] {
        let error = HumanActor::local_user(name).expect_err(name);
        assert_eq!(error.0, status, "{name:?}");
    }
    let long = "a".repeat(MAX_HUMAN_NAME_BYTES + 1);
    assert_eq!(
        HumanActor::local_user(&long).unwrap_err().0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
}

#[test]
fn local_user_accepts_a_plain_or_prefixed_name_as_human() {
    assert_eq!(
        HumanActor::local_user("alice").unwrap().as_str(),
        "human:alice"
    );
    assert_eq!(
        HumanActor::local_user("human:alice").unwrap().as_str(),
        "human:alice"
    );
}

#[test]
fn for_run_refuses_an_unknown_run_and_a_run_of_another_project() {
    let root = tempfile::tempdir().unwrap();
    let state = runtime(root.path());
    assert_eq!(
        AgentActor::for_run(&state, "app", "missing").unwrap_err().0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        AgentActor::for_run(&state, "app", "r2").unwrap_err().0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        AgentActor::for_run(&state, "../x", "r1").unwrap_err().0,
        StatusCode::NOT_FOUND
    );
    let agent = AgentActor::for_run(&state, "app", "r1").unwrap();
    assert_eq!(agent.as_str(), "agent:run-r1");
    assert_eq!(agent.project_id(), "app");
}

#[tokio::test]
async fn an_agent_proposal_accepted_by_a_human_records_the_human() {
    let root = tempfile::tempdir().unwrap();
    let state = runtime(root.path());
    let paths = set_up_roots(&state, "app");
    let store = RootsStore::for_project(&state, "app").unwrap();
    let agent = AgentActor::for_run(&state, "app", "r1").unwrap();
    let proposal = store.propose(&agent, edge_proposal()).await.unwrap();
    assert_eq!(proposal.status, ProposalStatus::Pending);

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
    assert_eq!(
        decision.edge.as_ref().unwrap().proposed_by.as_deref(),
        Some("agent:run-r1")
    );
    let graph = store.graph().await.unwrap();
    assert_eq!(graph.edges.len(), 1);
    assert_eq!(graph.edges[0].by, "human:alice");
    assert!(read_events(&paths)
        .unwrap()
        .iter()
        .any(|event| event.by == "human:alice"));
    assert!(store.pending_proposals().await.unwrap().is_empty());
}

#[tokio::test]
async fn a_human_rejects_an_agent_proposal() {
    let root = tempfile::tempdir().unwrap();
    let state = runtime(root.path());
    set_up_roots(&state, "app");
    let store = RootsStore::for_project(&state, "app").unwrap();
    let agent = AgentActor::for_run(&state, "app", "r1").unwrap();
    let proposal = store.propose(&agent, edge_proposal()).await.unwrap();
    let human = HumanActor::local_user("human:alice").unwrap();
    let decision = store
        .reject_proposal(&human, &proposal.id, Some("not related".into()))
        .await
        .unwrap();
    assert_eq!(decision.proposals[0].status, ProposalStatus::Rejected);
    assert_eq!(
        decision.proposals[0].decided_by.as_deref(),
        Some("human:alice")
    );
    assert!(store.graph().await.unwrap().edges.is_empty());
}

#[tokio::test]
async fn a_human_rejects_an_agent_sprout() {
    let root = tempfile::tempdir().unwrap();
    let state = runtime(root.path());
    set_up_roots(&state, "app");
    let store = RootsStore::for_project(&state, "app").unwrap();
    let agent = AgentActor::for_run(&state, "app", "r1").unwrap();
    let filed = store.file_sprout(&agent, sprout()).await.unwrap();
    let human = HumanActor::local_user("alice").unwrap();
    let node = store
        .reject_sprout(&human, &filed.node.id, Some("not now".into()))
        .await
        .unwrap();
    assert_eq!(node.status, NodeStatus::Sprout(SproutStatus::Rejected));
    assert_eq!(node.decided_by.as_deref(), Some("human:alice"));
    assert!(store.open_sprouts().await.unwrap().is_empty());
}

#[tokio::test]
async fn agent_operations_record_the_run_agent() {
    let root = tempfile::tempdir().unwrap();
    let state = runtime(root.path());
    let paths = set_up_roots(&state, "app");
    let store = RootsStore::for_project(&state, "app").unwrap();
    let agent = AgentActor::for_run(&state, "app", "r1").unwrap();

    let proposal = store.propose(&agent, edge_proposal()).await.unwrap();
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
    let filed = store.file_sprout(&agent, sprout()).await.unwrap();
    assert_eq!(filed.node.author, "agent:run-r1");

    let pending = store.pending_proposals().await.unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].by, "agent:run-r1");
    assert_eq!(store.questions().await.unwrap().len(), 1);
    assert_eq!(store.open_sprouts().await.unwrap().len(), 1);
    assert_eq!(store.config().await.unwrap().project, "app");
    for event in read_events(&paths).unwrap() {
        assert_eq!(event.by, "agent:run-r1", "{event:?}");
    }
    // The filed sprout's response carries no host path.
    let text = serde_json::to_string(&filed).unwrap();
    assert!(!text.contains(&root.path().to_string_lossy().into_owned()));
}

#[tokio::test]
async fn an_agent_of_another_project_is_refused() {
    let root = tempfile::tempdir().unwrap();
    let state = runtime(root.path());
    set_up_roots(&state, "app");
    let store = RootsStore::for_project(&state, "app").unwrap();
    let stranger = AgentActor::for_run(&state, "other", "r2").unwrap();
    assert_eq!(
        store
            .propose(&stranger, edge_proposal())
            .await
            .unwrap_err()
            .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        store.file_sprout(&stranger, sprout()).await.unwrap_err().0,
        StatusCode::FORBIDDEN
    );
    assert!(store.pending_proposals().await.unwrap().is_empty());
}

#[tokio::test]
async fn roots_refusals_reach_the_caller_with_their_status() {
    let root = tempfile::tempdir().unwrap();
    let state = runtime(root.path());
    set_up_roots(&state, "app");
    let store = RootsStore::for_project(&state, "app").unwrap();
    let human = HumanActor::local_user("alice").unwrap();
    let agent = AgentActor::for_run(&state, "app", "r1").unwrap();
    let missing = store
        .accept_proposal(&human, "p-ffff", AcceptRequest::default())
        .await
        .unwrap_err();
    assert_eq!(missing.0, StatusCode::NOT_FOUND);
    let invalid = store
        .reject_sprout(&human, "r-a1b2", None)
        .await
        .unwrap_err();
    assert_eq!(invalid.0, StatusCode::UNPROCESSABLE_ENTITY);
    let mut no_reason = edge_proposal();
    no_reason.reason = None;
    assert_eq!(
        store.propose(&agent, no_reason).await.unwrap_err().0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
}

#[test]
fn roots_error_kinds_map_to_runtime_statuses() {
    let root = Path::new("/store-root-for-the-test");
    for (error, status) in [
        (
            RootsError::not_found("no proposal p-1"),
            StatusCode::NOT_FOUND,
        ),
        (
            RootsError::conflict("already accepted"),
            StatusCode::CONFLICT,
        ),
        (RootsError::guard("only agents"), StatusCode::FORBIDDEN),
        (
            RootsError::usage("bad id"),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (
            RootsError::validation("too long"),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (
            RootsError::ambiguous("which one", vec!["r-a".into()]),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (
            RootsError::error("disk failed"),
            StatusCode::INTERNAL_SERVER_ERROR,
        ),
        (
            RootsError::cancelled("cancelled"),
            StatusCode::INTERNAL_SERVER_ERROR,
        ),
        (
            RootsError::config("config.yaml: bad"),
            StatusCode::INTERNAL_SERVER_ERROR,
        ),
        (
            RootsError::not_initialized("/somewhere"),
            StatusCode::CONFLICT,
        ),
    ] {
        let code = error.code;
        assert_eq!(roots_error(error, root).0, status, "{code:?}");
    }
}

#[test]
fn mapped_messages_never_carry_the_store_path() {
    let root = Path::new("/store-root-for-the-test");
    let guard = RootsError::guard(
        "refusing to write /store-root-for-the-test/.luuna/roots/human/x: it is outside",
    );
    let mapped = roots_error(guard, root);
    assert_eq!(mapped.0, StatusCode::FORBIDDEN);
    assert!(
        !mapped.1.contains("/store-root-for-the-test"),
        "{}",
        mapped.1
    );
    let failed = roots_error(
        RootsError::error("failed to write /store-root-for-the-test/x"),
        root,
    );
    assert!(
        !failed.1.contains("/store-root-for-the-test"),
        "{}",
        failed.1
    );
    let uninitialised = roots_error(RootsError::not_initialized("/elsewhere"), root);
    assert_eq!(uninitialised.1, NOT_SET_UP);
    let plain = roots_error(RootsError::not_found("no proposal p-1"), root);
    assert_eq!(plain.1, "no proposal p-1");
}
