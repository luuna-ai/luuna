//! Native component integration tests: no model, provider credential or personal store.
use crate::{integrations::*, AppState, RuntimeConfig};
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use luuna_domain::{DirtyState, PromptReference, RepositoryIdentity, RepositoryObservation};
use luuna_storage::MemoryRetentionPolicy;
use serde_json::json;

fn setup(root: &std::path::Path) -> AppState {
    let config = RuntimeConfig {
        data_dir: root.join("private"),
        pi_program: "node".into(),
        git_program: "git".into(),
        pi_launcher_args: vec![],
        provider: None,
        model: None,
        token: "synthetic-components-token-at-least-32-bytes".into(),
        max_run_seconds: 10,
    };
    let state = AppState::open(config)
        .unwrap()
        .with_components(luuna_components::Components::discover().unwrap());
    for id in ["context-a", "context-b"] {
        let observation = RepositoryObservation {
            local_path: root.join(id).to_string_lossy().into_owned(),
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
            observed_at: "2026-10-21T00:00:00Z".into(),
        };
        state
            .store
            .lock()
            .unwrap()
            .register_project(id, id, &[("app".into(), observation)])
            .unwrap();
    }
    state
}
async fn memory(
    state: &AppState,
    project: &str,
    key: &str,
    scope: &str,
    content: &str,
) -> MemoryRecord {
    create_memory(
        State(state.clone()),
        Path(project.into()),
        Json(CreateMemory {
            idempotency_key: key.into(),
            scope: scope.into(),
            content: content.into(),
            provenance: "Explicit synthetic fixture".into(),
        }),
    )
    .await
    .unwrap()
    .0
}
async fn prompt(state: &AppState, key: &str, body: &str, version: Option<u64>) -> RenderedPrompt {
    create_prompt(
        State(state.clone()),
        Path("context-a".into()),
        Json(CreatePrompt {
            idempotency_key: key.into(),
            name: "guidance".into(),
            body: body.into(),
            expected_version: version,
        }),
    )
    .await
    .unwrap()
    .0
}
async fn raw(state: &AppState, component: &str, project: &str, args: &[&str]) -> serde_json::Value {
    state
        .components
        .as_ref()
        .unwrap()
        .call(
            component,
            &workspace(state, project).unwrap(),
            &args.iter().map(|arg| arg.to_string()).collect::<Vec<_>>(),
        )
        .await
        .unwrap()
}
#[tokio::test]
async fn components_real_scopes_snapshot_staleness_and_restart() {
    Box::pin(scopes_snapshot_staleness_and_restart()).await;
}
async fn scopes_snapshot_staleness_and_restart() {
    let root = tempfile::tempdir().unwrap();
    let state = setup(root.path());
    let (selected, process) = Box::pin(scope_checks(&state)).await;
    let second = Box::pin(prompt_checks(&state, &selected, &process)).await;
    Box::pin(delete_checks(&state, &selected)).await;
    let config = (*state.config).clone();
    drop(state);
    let restarted = AppState::open(config)
        .unwrap()
        .with_components(luuna_components::Components::discover().unwrap());
    assert!(enabled(&restarted, "context-a").unwrap());
    assert_eq!(
        prompt(&restarted, "p2", "Prompt v2", Some(1)).await.hash,
        second.hash
    );
    assert_eq!(
        memory(
            &restarted,
            "context-a",
            "m3",
            "process",
            "selected process convention"
        )
        .await
        .id,
        process.id
    );
}
// Keep each fixture future small on native Rust's default test-thread stack.
async fn scope_checks(state: &AppState) -> (MemoryRecord, MemoryRecord) {
    assert_eq!(
        create_memory(
            State(state.clone()),
            Path("context-a".into()),
            Json(CreateMemory {
                idempotency_key: "disabled".into(),
                scope: "project".into(),
                content: "x".into(),
                provenance: "fixture".into()
            })
        )
        .await
        .err()
        .unwrap()
        .0,
        StatusCode::CONFLICT
    );
    for id in ["context-a", "context-b"] {
        let _ = enable(State(state.clone()), Path(id.into())).await.unwrap();
    }
    let selected = memory(
        state,
        "context-a",
        "m1",
        "project",
        "selected project memory",
    )
    .await;
    let private = memory(state, "context-a", "m2", "user", "PRIVATE_NOT_SELECTED").await;
    let process = memory(
        state,
        "context-a",
        "m3",
        "process",
        "selected process convention",
    )
    .await;
    let list = list_memory(
        State(state.clone()),
        Path("context-a".into()),
        Query(MemoryQuery {
            include_user: false,
            q: None,
        }),
    )
    .await
    .unwrap()
    .0;
    assert_eq!(list.len(), 2);
    assert!(!list.iter().any(|row| row.id == private.id));
    assert!(list_memory(
        State(state.clone()),
        Path("context-b".into()),
        Query(MemoryQuery {
            include_user: false,
            q: None
        })
    )
    .await
    .unwrap()
    .0
    .is_empty());
    let opted = list_memory(
        State(state.clone()),
        Path("context-b".into()),
        Query(MemoryQuery {
            include_user: true,
            q: None,
        }),
    )
    .await
    .unwrap()
    .0;
    assert_eq!(opted.len(), 1);
    assert_eq!(opted[0].id, private.id);
    assert_eq!(
        memory(
            state,
            "context-a",
            "m1",
            "project",
            "selected project memory"
        )
        .await
        .id,
        selected.id
    );
    let conflict = create_memory(
        State(state.clone()),
        Path("context-a".into()),
        Json(CreateMemory {
            idempotency_key: "m1".into(),
            scope: "project".into(),
            content: "different".into(),
            provenance: "fixture".into(),
        }),
    )
    .await
    .err()
    .unwrap();
    assert_eq!(conflict.0, StatusCode::CONFLICT);
    (selected, process)
}
async fn prompt_checks(
    state: &AppState,
    selected: &MemoryRecord,
    process: &MemoryRecord,
) -> RenderedPrompt {
    let first = prompt(state, "p1", "Prompt v1", None).await;
    assert_eq!(
        prompt(state, "p1", "Prompt v1", None).await.hash,
        first.hash
    );
    let selection = SelectContext {
        memory_ids: vec![selected.id.clone(), process.id.clone()],
        prompt: Some(PromptReference {
            name: first.name.clone(),
            version: first.version,
            hash: first.hash.clone(),
        }),
    };
    let snapshot = resolve_selection(state, "context-a", Some(&selection))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(snapshot.memory[0].content, selected.content);
    assert_eq!(snapshot.prompt.as_ref().unwrap().body, first.body);
    let resources = resolve_approved(state, "context-a", Some(&snapshot))
        .await
        .unwrap()
        .unwrap()
        .to_string();
    assert!(resources.contains("selected project memory"));
    assert!(!resources.contains("PRIVATE_NOT_SELECTED"));
    assert!(resolve_selection(state, "context-b", Some(&selection))
        .await
        .is_err());
    let second = prompt(state, "p2", "Prompt v2", Some(1)).await;
    assert_eq!(second.version, 2);
    assert_eq!(
        resolve_approved(state, "context-a", Some(&snapshot))
            .await
            .err()
            .unwrap()
            .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        render_prompt(
            State(state.clone()),
            Path(("context-a".into(), "guidance".into())),
            Query(PromptQuery { version: Some(1) })
        )
        .await
        .err()
        .unwrap()
        .0,
        StatusCode::CONFLICT
    );
    second
}
async fn delete_checks(state: &AppState, selected: &MemoryRecord) {
    let selected_context = resolve_selection(
        state,
        "context-a",
        Some(&SelectContext {
            memory_ids: vec![selected.id.clone()],
            prompt: None,
        }),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(
        delete_memory(
            State(state.clone()),
            Path(("context-a".into(), selected.id.clone())),
            Query(DeleteMemoryQuery {
                expected_hash:
                    "sha256:0000000000000000000000000000000000000000000000000000000000000000".into()
            })
        )
        .await
        .err()
        .unwrap()
        .0,
        StatusCode::CONFLICT
    );
    let _ = delete_memory(
        State(state.clone()),
        Path(("context-a".into(), selected.id.clone())),
        Query(DeleteMemoryQuery {
            expected_hash: selected.hash.clone(),
        }),
    )
    .await
    .unwrap();
    // Repeated DELETE is idempotent even after its mapping is tombstoned.
    let _ = delete_memory(
        State(state.clone()),
        Path(("context-a".into(), selected.id.clone())),
        Query(DeleteMemoryQuery {
            expected_hash: selected.hash.clone(),
        }),
    )
    .await
    .unwrap();
    assert_eq!(
        resolve_approved(state, "context-a", Some(&selected_context))
            .await
            .err()
            .unwrap()
            .0,
        StatusCode::CONFLICT
    );
}
#[tokio::test]
async fn components_real_uncertain_memory_and_delete_reconcile_after_restart() {
    let root = tempfile::tempdir().unwrap();
    let state = setup(root.path());
    let _ = enable(State(state.clone()), Path("context-a".into()))
        .await
        .unwrap();
    let payload = json!({"scope":"project","content":"uncertain content","provenance":"Explicit synthetic fixture"});
    let op = state
        .store
        .lock()
        .unwrap()
        .begin_component_operation(
            "context-a",
            "pending",
            "memory.create",
            &payload,
            "mem-reserved",
        )
        .unwrap();
    state
        .store
        .lock()
        .unwrap()
        .mark_component_operation_attempted("context-a", "pending")
        .unwrap();
    let envelope=json!({"id":op.record_id,"scope":"project","content":"uncertain content","provenance":"Explicit synthetic fixture"}).to_string();
    raw(
        &state,
        "atlas",
        "context-a",
        &[
            "record",
            "luuna-project",
            &envelope,
            "--type",
            "convention",
            "--tags",
            "mem-reserved",
        ],
    )
    .await;
    let config = (*state.config).clone();
    drop(state);
    let restarted = AppState::open(config)
        .unwrap()
        .with_components(luuna_components::Components::discover().unwrap());
    let recovered = memory(
        &restarted,
        "context-a",
        "pending",
        "project",
        "uncertain content",
    )
    .await;
    assert_eq!(recovered.id, "mem-reserved");
    let binding = restarted
        .store
        .lock()
        .unwrap()
        .memory_binding("context-a", &recovered.id)
        .unwrap()
        .unwrap();
    // Simulate CLI deletion committed before a local response/tombstone was saved.
    raw(
        &restarted,
        "atlas",
        "context-a",
        &["delete", &binding.domain, &binding.source_id],
    )
    .await;
    let _ = delete_memory(
        State(restarted.clone()),
        Path(("context-a".into(), recovered.id)),
        Query(DeleteMemoryQuery {
            expected_hash: recovered.hash,
        }),
    )
    .await
    .unwrap();
    assert!(list_memory(
        State(restarted.clone()),
        Path("context-a".into()),
        Query(MemoryQuery {
            include_user: false,
            q: None
        })
    )
    .await
    .unwrap()
    .0
    .is_empty());
    // An uncertain missing create cannot be blindly repeated.
    restarted
        .store
        .lock()
        .unwrap()
        .begin_component_operation(
            "context-a",
            "missing",
            "memory.create",
            &payload,
            "mem-missing",
        )
        .unwrap();
    restarted
        .store
        .lock()
        .unwrap()
        .mark_component_operation_attempted("context-a", "missing")
        .unwrap();
    let result = create_memory(
        State(restarted.clone()),
        Path("context-a".into()),
        Json(CreateMemory {
            idempotency_key: "missing".into(),
            scope: "project".into(),
            content: "uncertain content".into(),
            provenance: "Explicit synthetic fixture".into(),
        }),
    )
    .await;
    assert_eq!(result.err().unwrap().0, StatusCode::CONFLICT);
}
#[tokio::test]
async fn components_real_duplicate_memory_delivery_refuses_acknowledgement() {
    let root = tempfile::tempdir().unwrap();
    let state = setup(root.path());
    let _ = enable(State(state.clone()), Path("context-a".into()))
        .await
        .unwrap();
    let payload = json!({"scope":"project","content":"uncertain content","provenance":"Explicit synthetic fixture"});
    state
        .store
        .lock()
        .unwrap()
        .begin_component_operation(
            "context-a",
            "duplicate",
            "memory.create",
            &payload,
            "mem-duplicate",
        )
        .unwrap();
    state
        .store
        .lock()
        .unwrap()
        .mark_component_operation_attempted("context-a", "duplicate")
        .unwrap();
    for content in ["uncertain content", "external duplicate"] {
        let envelope=json!({"id":"mem-duplicate","scope":"project","content":content,"provenance":"Explicit synthetic fixture"}).to_string();
        raw(
            &state,
            "atlas",
            "context-a",
            &[
                "record",
                "luuna-project",
                &envelope,
                "--type",
                "convention",
                "--tags",
                "mem-duplicate",
                "--force",
            ],
        )
        .await;
    }
    let error = create_memory(
        State(state.clone()),
        Path("context-a".into()),
        Json(CreateMemory {
            idempotency_key: "duplicate".into(),
            scope: "project".into(),
            content: "uncertain content".into(),
            provenance: "Explicit synthetic fixture".into(),
        }),
    )
    .await
    .err()
    .unwrap();
    assert_eq!(error.0, StatusCode::CONFLICT);
    assert!(error.1.contains("Duplicate Atlas delivery identities"));
    assert!(state
        .store
        .lock()
        .unwrap()
        .memory_binding("context-a", "mem-duplicate")
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn components_real_schema_change_invalidates_approval_even_with_same_prompt_body() {
    let root = tempfile::tempdir().unwrap();
    let state = setup(root.path());
    let _ = enable(State(state.clone()), Path("context-a".into()))
        .await
        .unwrap();
    prompt(&state, "schema-prompt", "Required guidance", None).await;
    raw(
        &state,
        "beacon",
        "context-a",
        &[
            "schema",
            "create",
            "--name",
            "approval",
            "--required",
            "guidance",
        ],
    )
    .await;
    raw(
        &state,
        "beacon",
        "context-a",
        &["update", "guidance", "--schema", "approval"],
    )
    .await;
    let rendered = render_prompt(
        State(state.clone()),
        Path(("context-a".into(), "guidance".into())),
        Query(PromptQuery { version: None }),
    )
    .await
    .unwrap()
    .0;
    let context = resolve_selection(
        &state,
        "context-a",
        Some(&SelectContext {
            memory_ids: vec![],
            prompt: Some(PromptReference {
                name: rendered.name.clone(),
                version: rendered.version,
                hash: rendered.hash.clone(),
            }),
        }),
    )
    .await
    .unwrap()
    .unwrap();
    raw(
        &state,
        "beacon",
        "context-a",
        &[
            "schema",
            "rule",
            "add",
            "approval",
            "--section",
            "guidance",
            "--pattern",
            "Required",
            "--message",
            "Requires expected text",
        ],
    )
    .await;
    let updated = render_prompt(
        State(state.clone()),
        Path(("context-a".into(), "guidance".into())),
        Query(PromptQuery { version: None }),
    )
    .await
    .unwrap()
    .0;
    assert_eq!(updated.body, rendered.body);
    assert_ne!(updated.hash, rendered.hash);
    assert_eq!(
        resolve_approved(&state, "context-a", Some(&context))
            .await
            .err()
            .unwrap()
            .0,
        StatusCode::CONFLICT
    );
    raw(
        &state,
        "beacon",
        "context-a",
        &[
            "schema",
            "rule",
            "add",
            "approval",
            "--section",
            "guidance",
            "--pattern",
            "IMPOSSIBLE",
            "--message",
            "Fixture failure",
        ],
    )
    .await;
    assert_eq!(
        render_prompt(
            State(state.clone()),
            Path(("context-a".into(), "guidance".into())),
            Query(PromptQuery { version: None })
        )
        .await
        .err()
        .unwrap()
        .0,
        StatusCode::CONFLICT
    );
}

#[tokio::test]
async fn components_real_uncertain_prompt_reconciles_and_memory_edits_stale_context() {
    let root = tempfile::tempdir().unwrap();
    let state = setup(root.path());
    let _ = enable(State(state.clone()), Path("context-a".into()))
        .await
        .unwrap();
    let request = json!({"name":"guidance","body":"Reserved prompt","expected_version":null});
    let operation = state
        .store
        .lock()
        .unwrap()
        .begin_component_operation(
            "context-a",
            "pending-prompt",
            "prompt.create",
            &request,
            "prompt-reserved",
        )
        .unwrap();
    state
        .store
        .lock()
        .unwrap()
        .mark_component_operation_attempted("context-a", "pending-prompt")
        .unwrap();
    let marker = format!("luuna_request={}", operation.record_id);
    raw(
        &state,
        "beacon",
        "context-a",
        &[
            "create",
            "--name",
            "guidance",
            "--section",
            "guidance=Reserved prompt",
            "--fm",
            &marker,
        ],
    )
    .await;
    let config = (*state.config).clone();
    drop(state);
    let restarted = AppState::open(config)
        .unwrap()
        .with_components(luuna_components::Components::discover().unwrap());
    let recovered = prompt(&restarted, "pending-prompt", "Reserved prompt", None).await;
    assert_eq!(recovered.version, 1);
    assert_eq!(
        list_prompts(State(restarted.clone()), Path("context-a".into()))
            .await
            .unwrap()
            .0
            .len(),
        1
    );
    let original = memory(
        &restarted,
        "context-a",
        "editable",
        "project",
        "Original memory",
    )
    .await;
    let context = resolve_selection(
        &restarted,
        "context-a",
        Some(&SelectContext {
            memory_ids: vec![original.id.clone()],
            prompt: None,
        }),
    )
    .await
    .unwrap()
    .unwrap();
    let binding = restarted
        .store
        .lock()
        .unwrap()
        .memory_binding("context-a", &original.id)
        .unwrap()
        .unwrap();
    let edited=json!({"id":original.id,"scope":"project","content":"Externally edited memory","provenance":"Explicit synthetic fixture"}).to_string();
    raw(
        &restarted,
        "atlas",
        "context-a",
        &[
            "edit",
            &binding.domain,
            &binding.source_id,
            "--content",
            &edited,
        ],
    )
    .await;
    assert_eq!(
        resolve_approved(&restarted, "context-a", Some(&context))
            .await
            .err()
            .unwrap()
            .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        delete_memory(
            State(restarted.clone()),
            Path(("context-a".into(), original.id)),
            Query(DeleteMemoryQuery {
                expected_hash: original.hash
            })
        )
        .await
        .err()
        .unwrap()
        .0,
        StatusCode::CONFLICT
    );
}
#[tokio::test]
async fn components_real_promotion_widens_scope_and_export_keeps_private_memory_home() {
    let root = tempfile::tempdir().unwrap();
    let state = setup(root.path());
    let _ = enable(State(state.clone()), Path("context-a".into()))
        .await
        .unwrap();
    let process = memory(
        &state,
        "context-a",
        "p1",
        "process",
        "process convention worth keeping",
    )
    .await;
    let private = memory(&state, "context-a", "u1", "user", "PRIVATE_USER_ONLY").await;
    let promote = |id: String, key: &str, hash: String, to_scope: &str| {
        let state = state.clone();
        let key = key.to_owned();
        let to_scope = to_scope.to_owned();
        async move {
            promote_memory(
                State(state),
                Path(("context-a".into(), id)),
                Json(PromoteMemory {
                    idempotency_key: key,
                    expected_hash: hash,
                    to_scope,
                }),
            )
            .await
        }
    };
    // Only one widening step at a time, and only from the displayed state.
    assert_eq!(
        promote(process.id.clone(), "skip", process.hash.clone(), "user")
            .await
            .err()
            .unwrap()
            .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let stale =
        "sha256:0000000000000000000000000000000000000000000000000000000000000000".to_owned();
    assert_eq!(
        promote(process.id.clone(), "stale", stale, "project")
            .await
            .err()
            .unwrap()
            .0,
        StatusCode::CONFLICT
    );
    let first = promote(process.id.clone(), "pr1", process.hash.clone(), "project")
        .await
        .unwrap()
        .0;
    assert_eq!(first["promoted"]["scope"], "project");
    assert_eq!(
        first["promoted"]["content"],
        "process convention worth keeping"
    );
    let provenance = first["promoted"]["provenance"].as_str().unwrap();
    assert!(
        provenance.starts_with(&format!("promoted from process memory {}: ", process.id)),
        "{provenance}"
    );
    assert_eq!(first["from"]["id"], process.id);
    // The same key replays the same answer without a second record.
    let replay = promote(process.id.clone(), "pr1", process.hash.clone(), "project")
        .await
        .unwrap()
        .0;
    assert_eq!(replay, first);
    let listed = list_memory(
        State(state.clone()),
        Path("context-a".into()),
        Query(MemoryQuery {
            include_user: false,
            q: None,
        }),
    )
    .await
    .unwrap()
    .0;
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, first["promoted"]["id"]);
    assert_ne!(listed[0].id, process.id);
    // Project -> user leaves the project list and joins the explicit user view.
    let promoted_id = first["promoted"]["id"].as_str().unwrap().to_owned();
    let promoted_hash = first["promoted"]["hash"].as_str().unwrap().to_owned();
    let second = promote(promoted_id.clone(), "pr2", promoted_hash, "user")
        .await
        .unwrap()
        .0;
    assert_eq!(second["promoted"]["scope"], "user");
    let project_only = list_memory(
        State(state.clone()),
        Path("context-a".into()),
        Query(MemoryQuery {
            include_user: false,
            q: None,
        }),
    )
    .await
    .unwrap()
    .0;
    assert!(project_only.is_empty());
    let with_user = list_memory(
        State(state.clone()),
        Path("context-a".into()),
        Query(MemoryQuery {
            include_user: true,
            q: Some("worth keeping".into()),
        }),
    )
    .await
    .unwrap()
    .0;
    assert_eq!(with_user.len(), 1);
    assert_eq!(with_user[0].id, second["promoted"]["id"]);
    // The export carries project and process memory only, and no path of this host.
    let project_memory = memory(
        &state,
        "context-a",
        "p2",
        "project",
        "exported project convention",
    )
    .await;
    let export = export_project(State(state.clone()), Path("context-a".into()))
        .await
        .unwrap()
        .0;
    let memories = export["memory"].as_array().unwrap();
    assert_eq!(memories.len(), 1);
    assert_eq!(memories[0]["id"], project_memory.id);
    let text = export.to_string();
    assert!(!text.contains("PRIVATE_USER_ONLY"));
    assert!(!text.contains(&private.content));
    assert!(!text.contains(&root.path().to_string_lossy().replace('\\', "\\\\")));
    assert!(export["omitted"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item == "user memory"));
    // Text that would carry a host path is refused rather than exported.
    let ticket = luuna_domain::Ticket {
        schema_version: luuna_domain::SCHEMA_VERSION.into(),
        id: "leaky".into(),
        project_id: "context-a".into(),
        revision: 1,
        source: luuna_domain::TicketSource::LuunaSqliteTemporary,
        title: "Leaky ticket".into(),
        intent: format!("Look in {}", root.path().join("context-a").display()),
        acceptance_criteria: vec!["none".into()],
        repository_ids: vec!["app".into()],
        purpose: luuna_domain::TicketPurpose::Feature,
        ticket_type: Default::default(),
        priority: luuna_domain::DEFAULT_TICKET_PRIORITY,
        labels: vec![],
        created_at: "2026-10-21T00:00:00.000Z".into(),
        updated_at: "2026-10-21T00:00:00.000Z".into(),
    };
    state
        .store
        .lock()
        .unwrap()
        .create_ticket(&ticket, "leaky-key")
        .unwrap();
    let refused = export_project(State(state.clone()), Path("context-a".into()))
        .await
        .err()
        .unwrap();
    assert_eq!(refused.0, StatusCode::CONFLICT);
    assert!(refused.1.contains("Export refused"), "{}", refused.1);
}

fn git(dir: &std::path::Path, args: &[&str]) {
    let output = std::process::Command::new("git")
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

/// A runtime with the real component toolchain and its own data directory,
/// registering nothing: the import registers from the export.
fn real_runtime(data_dir: &std::path::Path) -> AppState {
    let config = RuntimeConfig {
        data_dir: data_dir.to_path_buf(),
        pi_program: "node".into(),
        git_program: "git".into(),
        pi_launcher_args: vec![],
        provider: None,
        model: None,
        token: "synthetic-components-token-at-least-32-bytes".into(),
        max_run_seconds: 10,
    };
    AppState::open(config)
        .unwrap()
        .with_components(luuna_components::Components::discover().unwrap())
}

/// ADR 0021 amendment: an export of tickets, project/process memory and a
/// prompt round-trips through the real CLIs into a second runtime whose
/// checkout is a fresh clone with the same remote and root commits.
#[tokio::test]
async fn components_real_import_round_trips_tickets_memory_and_prompts_from_an_export() {
    use crate::projects::{
        confirm_manifest, discover, import_project, register_project, ConfirmManifest,
        ImportProject, RegisterProject, RegisterRepository,
    };
    use crate::writer::{create_ticket, list_tickets, CreateTicket};
    let root = tempfile::tempdir().unwrap();
    let fixture = root.path().join("fixture");
    std::fs::create_dir_all(&fixture).unwrap();
    git(&fixture, &["init", "-q", "-b", "main"]);
    git(&fixture, &["config", "commit.gpgsign", "false"]);
    std::fs::write(fixture.join("README.md"), "fixture\n").unwrap();
    std::fs::write(
        fixture.join("Cargo.toml"),
        "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    std::fs::write(fixture.join("Cargo.lock"), "# synthetic\n").unwrap();
    git(&fixture, &["add", "."]);
    git(&fixture, &["commit", "-q", "-m", "initial"]);
    git(
        &fixture,
        &[
            "remote",
            "add",
            "origin",
            "https://example.invalid/roundtrip.git",
        ],
    );
    let clone = root.path().join("clone");
    let output = std::process::Command::new("git")
        .args(["clone", "-q"])
        .arg(&fixture)
        .arg(&clone)
        .output()
        .unwrap();
    assert!(output.status.success());
    git(
        &clone,
        &[
            "remote",
            "set-url",
            "origin",
            "https://example.invalid/roundtrip.git",
        ],
    );

    // Source machine: register, confirm, activate, write through every adapter.
    let source = real_runtime(&root.path().join("source-private"));
    let registered = register_project(
        State(source.clone()),
        Json(RegisterProject {
            id: Some("roundtrip".into()),
            name: "Round trip".into(),
            repositories: vec![RegisterRepository {
                id: "app".into(),
                path: fixture.to_string_lossy().into_owned(),
            }],
        }),
    )
    .await
    .unwrap();
    assert_eq!(registered.0, StatusCode::CREATED);
    let discovery = discover(State(source.clone()), Path("roundtrip".into()))
        .await
        .unwrap()
        .1
         .0;
    let _ = confirm_manifest(
        State(source.clone()),
        Path("roundtrip".into()),
        Json(ConfirmManifest {
            expected_revision: 0,
            manifest: discovery.proposed_manifest.clone(),
        }),
    )
    .await
    .map_err(|e| e.1)
    .unwrap();
    let _ = enable(State(source.clone()), Path("roundtrip".into()))
        .await
        .map_err(|e| e.1)
        .unwrap();
    let ticket = create_ticket(
        State(source.clone()),
        Path("roundtrip".into()),
        Json(CreateTicket {
            idempotency_key: "t1".into(),
            title: "Exported ticket".into(),
            intent: "Keep src/lib.rs tidy".into(),
            acceptance_criteria: vec!["cargo test passes".into()],
            repository_ids: vec!["app".into()],
            purpose: luuna_domain::TicketPurpose::Feature,
            ticket_type: luuna_domain::TicketType::Bug,
            priority: 1,
            labels: vec!["export".into()],
        }),
    )
    .await
    .unwrap()
    .1
     .0;
    assert_eq!(ticket.source, luuna_domain::TicketSource::Missions);
    let project_memory = memory(
        &source,
        "roundtrip",
        "m1",
        "project",
        "exported project convention",
    )
    .await;
    let process_memory = memory(
        &source,
        "roundtrip",
        "m2",
        "process",
        "exported process lesson",
    )
    .await;
    let _private = memory(&source, "roundtrip", "u1", "user", "PRIVATE_USER_ONLY").await;
    let prompt = create_prompt(
        State(source.clone()),
        Path("roundtrip".into()),
        Json(CreatePrompt {
            idempotency_key: "pr1".into(),
            name: "guidance".into(),
            body: "Review every diff twice.".into(),
            expected_version: None,
        }),
    )
    .await
    .unwrap()
    .0;
    assert_eq!(prompt.version, 1);
    let export = export_project(State(source.clone()), Path("roundtrip".into()))
        .await
        .unwrap()
        .0;
    assert_eq!(export["tickets"].as_array().unwrap().len(), 1);
    assert_eq!(export["memory"].as_array().unwrap().len(), 2);
    assert_eq!(export["prompts"].as_array().unwrap().len(), 1);
    assert!(!export.to_string().contains("PRIVATE_USER_ONLY"));

    // Target machine: a fresh runtime and a fresh clone.
    let target = real_runtime(&root.path().join("target-private"));
    let import = |key: &str| {
        let target = target.clone();
        let export = export.clone();
        let key = key.to_owned();
        let path = clone.to_string_lossy().into_owned();
        async move {
            import_project(
                State(target),
                Json(ImportProject {
                    export,
                    repositories: vec![RegisterRepository {
                        id: "app".into(),
                        path,
                    }],
                    idempotency_key: key,
                }),
            )
            .await
        }
    };
    let (status, imported) = import("imp").await.unwrap();
    let imported = imported.0;
    assert_eq!(status, StatusCode::CREATED, "{imported}");
    assert_eq!(imported["components_enabled"], true);
    assert_eq!(
        imported["counts"],
        json!({"tickets": 1, "memory": 2, "prompts": 1})
    );
    assert_eq!(
        imported["project"]["project"]["manifest_hash"],
        export["project"]["manifest_hash"]
    );
    let memory_mapping = imported["mapping"]["memory"].as_array().unwrap();
    assert_eq!(memory_mapping.len(), 2);
    assert!(memory_mapping
        .iter()
        .any(|m| m["exported_id"] == project_memory.id && m["imported_id"] != project_memory.id));
    assert!(memory_mapping
        .iter()
        .any(|m| m["exported_id"] == process_memory.id && m["scope"] == "process"));
    assert_eq!(imported["mapping"]["prompts"][0]["name"], "guidance");
    assert_eq!(imported["mapping"]["prompts"][0]["imported_version"], 1);
    assert_eq!(
        imported["mapping"]["prompts"][0]["sections_flattened"],
        false
    );

    // Missions owns the imported ticket under its exported identity.
    let tickets = list_tickets(State(target.clone()), Path("roundtrip".into()))
        .await
        .unwrap()
        .0;
    assert_eq!(tickets.len(), 1);
    assert_eq!(tickets[0].id, ticket.id);
    assert_eq!(tickets[0].revision, 1);
    assert_eq!(tickets[0].source, luuna_domain::TicketSource::Missions);
    assert_eq!(tickets[0].intent, ticket.intent);
    assert_eq!(tickets[0].created_at, ticket.created_at);
    // Type, priority and labels travel through Missions on both sides.
    assert_eq!(ticket.ticket_type, luuna_domain::TicketType::Bug);
    assert_eq!(ticket.priority, 1);
    assert_eq!(ticket.labels, vec!["export".to_owned()]);
    assert_eq!(tickets[0].ticket_type, luuna_domain::TicketType::Bug);
    assert_eq!(tickets[0].priority, 1);
    assert_eq!(tickets[0].labels, ticket.labels);
    let status = crate::missions::source(&target, &tickets[0]).await.unwrap();
    assert_eq!(status["reconciliation_required"], false);
    // Atlas holds project and process memory with the exported text; no user memory arrived.
    let listed = list_memory(
        State(target.clone()),
        Path("roundtrip".into()),
        Query(MemoryQuery {
            include_user: true,
            q: None,
        }),
    )
    .await
    .unwrap()
    .0;
    assert_eq!(listed.len(), 2);
    for exported in [&project_memory, &process_memory] {
        let record = listed
            .iter()
            .find(|record| record.content == exported.content)
            .unwrap();
        assert_eq!(record.scope, exported.scope);
        assert_eq!(record.provenance, exported.provenance);
        assert_ne!(record.id, exported.id);
    }
    // Beacon renders the imported prompt with the exported body.
    let rendered = render_prompt(
        State(target.clone()),
        Path(("roundtrip".into(), "guidance".into())),
        Query(PromptQuery { version: None }),
    )
    .await
    .unwrap()
    .0;
    assert_eq!(rendered.body, prompt.body);
    assert_eq!(rendered.version, 1);

    // The same key replays without a second record anywhere.
    let (status, replay) = import("imp").await.unwrap();
    assert_eq!(status, StatusCode::OK);
    assert_eq!(replay.0["replayed"], true);
    assert_eq!(
        list_tickets(State(target.clone()), Path("roundtrip".into()))
            .await
            .unwrap()
            .0
            .len(),
        1
    );
    assert_eq!(
        list_memory(
            State(target.clone()),
            Path("roundtrip".into()),
            Query(MemoryQuery {
                include_user: true,
                q: None
            }),
        )
        .await
        .unwrap()
        .0
        .len(),
        2
    );
    assert_eq!(import("imp-2").await.err().unwrap().0, StatusCode::CONFLICT);
    // The target exports the same project again.
    let again = export_project(State(target.clone()), Path("roundtrip".into()))
        .await
        .unwrap()
        .0;
    assert_eq!(
        again["project"]["manifest_hash"],
        export["project"]["manifest_hash"]
    );
    assert_eq!(again["tickets"][0]["id"], export["tickets"][0]["id"]);
    assert_eq!(again["memory"].as_array().unwrap().len(), 2);
    assert_eq!(again["prompts"][0]["body"], export["prompts"][0]["body"]);
}

#[tokio::test]
async fn components_real_memory_correction_stales_the_plan_that_selected_it_and_exports_user_memory(
) {
    let root = tempfile::tempdir().unwrap();
    let state = setup(root.path());
    let _ = enable(State(state.clone()), Path("context-a".into()))
        .await
        .unwrap();
    let selected = memory(&state, "context-a", "c1", "project", "tests use unittest").await;
    let selection = SelectContext {
        memory_ids: vec![selected.id.clone()],
        prompt: None,
    };
    let snapshot = resolve_selection(&state, "context-a", Some(&selection))
        .await
        .unwrap()
        .unwrap();
    assert!(resolve_approved(&state, "context-a", Some(&snapshot))
        .await
        .is_ok());
    let correct = |key: &str, hash: String, content: &str| {
        let state = state.clone();
        let key = key.to_owned();
        let content = content.to_owned();
        let id = selected.id.clone();
        async move {
            update_memory(
                State(state),
                Path(("context-a".into(), id)),
                Json(UpdateMemory {
                    idempotency_key: key,
                    expected_hash: hash,
                    content,
                }),
            )
            .await
        }
    };
    // A stale displayed hash and an unknown record are refused without a write.
    assert_eq!(
        correct(
            "stale",
            format!("sha256:{}", "0".repeat(64)),
            "tests use pytest"
        )
        .await
        .err()
        .unwrap()
        .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        update_memory(
            State(state.clone()),
            Path(("context-a".into(), "mem-missing".into())),
            Json(UpdateMemory {
                idempotency_key: "missing".into(),
                expected_hash: selected.hash.clone(),
                content: "x".into()
            })
        )
        .await
        .err()
        .unwrap()
        .0,
        StatusCode::NOT_FOUND
    );
    let corrected = correct("c2", selected.hash.clone(), "tests use pytest")
        .await
        .unwrap()
        .0;
    assert_eq!(corrected.id, selected.id);
    assert_eq!(corrected.scope, "project");
    assert_eq!(corrected.content, "tests use pytest");
    assert_ne!(corrected.hash, selected.hash);
    assert!(
        corrected
            .provenance
            .starts_with(&format!("corrected from {}: ", selected.hash)),
        "{}",
        corrected.provenance
    );
    assert!(corrected.provenance.ends_with("Explicit synthetic fixture"));
    // Replaying the key returns the recorded correction; the stale hash now refuses.
    let replayed = correct("c2", selected.hash.clone(), "tests use pytest")
        .await
        .unwrap()
        .0;
    assert_eq!(replayed.hash, corrected.hash);
    assert_eq!(
        correct("c3", selected.hash.clone(), "another correction")
            .await
            .err()
            .unwrap()
            .0,
        StatusCode::CONFLICT
    );
    // The plan that selected the original snapshot can no longer be implemented.
    let refused = resolve_approved(&state, "context-a", Some(&snapshot))
        .await
        .err()
        .unwrap();
    assert_eq!(refused.0, StatusCode::CONFLICT);
    assert!(refused.1.contains("approve a new plan"), "{}", refused.1);
    let listed = list_memory(
        State(state.clone()),
        Path("context-a".into()),
        Query(MemoryQuery {
            include_user: false,
            q: None,
        }),
    )
    .await
    .unwrap()
    .0;
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].hash, corrected.hash);

    // The personal export carries user memory with provenance and nothing of the project.
    let empty = export_user_memory(State(state.clone())).await.unwrap().0;
    assert_eq!(empty["kind"], "luuna-user-memory-export");
    assert_eq!(empty["memory"].as_array().unwrap().len(), 0);
    let private = memory(&state, "context-a", "u1", "user", "PRIVATE_USER_ONLY").await;
    let export = export_user_memory(State(state.clone())).await.unwrap().0;
    let records = export["memory"].as_array().unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0]["id"], private.id);
    assert_eq!(records[0]["scope"], "user");
    assert_eq!(records[0]["provenance"], "Explicit synthetic fixture");
    assert!(!export.to_string().contains("tests use pytest"));
    let leaking = memory(
        &state,
        "context-a",
        "u2",
        "user",
        "see /home/dev/private/notes.md",
    )
    .await;
    let refused = export_user_memory(State(state.clone()))
        .await
        .err()
        .unwrap();
    assert_eq!(refused.0, StatusCode::CONFLICT);
    assert!(refused.1.contains("Export refused"), "{}", refused.1);
    let _ = delete_memory(
        State(state.clone()),
        Path(("context-a".into(), leaking.id)),
        Query(DeleteMemoryQuery {
            expected_hash: leaking.hash,
        }),
    )
    .await
    .unwrap();
    assert_eq!(
        export_user_memory(State(state.clone())).await.unwrap().0["memory"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn components_real_retention_deletes_only_expired_process_memory_under_the_ledger() {
    let root = tempfile::tempdir().unwrap();
    let state = setup(root.path());
    let _ = enable(State(state.clone()), Path("context-a".into()))
        .await
        .unwrap();
    let old = memory(
        &state,
        "context-a",
        "r1",
        "process",
        "an old process lesson",
    )
    .await;
    let fresh = memory(
        &state,
        "context-a",
        "r2",
        "process",
        "a fresh process lesson",
    )
    .await;
    let project = memory(
        &state,
        "context-a",
        "r3",
        "project",
        "an old project convention",
    )
    .await;
    let private = memory(&state, "context-a", "r4", "user", "an old private note").await;
    // Age the records in the binding table: creation time is Luuna's record, not Atlas's.
    {
        let store = state.store.lock().unwrap();
        for id in [&old.id, &project.id, &private.id] {
            store
                .set_memory_binding_created_at(id, "2020-01-01T00:00:00Z")
                .unwrap();
        }
    }
    let apply = |key: &str| {
        let state = state.clone();
        let key = key.to_owned();
        async move {
            apply_memory_retention(
                State(state),
                Path("context-a".into()),
                Json(ApplyRetention {
                    idempotency_key: key,
                }),
            )
            .await
        }
    };
    // No policy: nothing expires.
    let nothing = apply("a0").await.unwrap().0;
    assert_eq!(nothing["deleted"].as_array().unwrap().len(), 0);
    let _ = set_memory_retention(
        State(state.clone()),
        Path("context-a".into()),
        Json(MemoryRetentionPolicy {
            process_max_age_days: Some(30),
            project_max_age_days: None,
        }),
    )
    .await
    .unwrap();
    let applied = apply("a1").await.unwrap().0;
    let deleted = applied["deleted"].as_array().unwrap();
    assert_eq!(deleted.len(), 1, "{applied}");
    assert_eq!(deleted[0]["id"], old.id);
    assert_eq!(deleted[0]["scope"], "process");
    // Replaying the key returns the recorded result; a new key finds nothing left to delete.
    assert_eq!(apply("a1").await.unwrap().0, applied);
    assert_eq!(
        apply("a2").await.unwrap().0["deleted"]
            .as_array()
            .unwrap()
            .len(),
        0
    );
    let remaining = list_memory(
        State(state.clone()),
        Path("context-a".into()),
        Query(MemoryQuery {
            include_user: true,
            q: None,
        }),
    )
    .await
    .unwrap()
    .0;
    let ids: Vec<&str> = remaining.iter().map(|record| record.id.as_str()).collect();
    assert!(ids.contains(&fresh.id.as_str()));
    assert!(ids.contains(&project.id.as_str()));
    assert!(ids.contains(&private.id.as_str()));
    assert!(!ids.contains(&old.id.as_str()));
    let events = state
        .store
        .lock()
        .unwrap()
        .project_events_after("context-a", 0)
        .unwrap();
    let recorded = events
        .iter()
        .filter(|event| event.event_type == "memory.retention_applied")
        .count();
    assert_eq!(recorded, 3);
    assert!(events
        .iter()
        .any(|event| event.event_type == "memory.retention_applied"
            && event.payload["deleted"][0]["id"] == old.id));
}
