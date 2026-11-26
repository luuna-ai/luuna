//! Baseline evidence gates implementation transactionally; nothing stale unlocks a writer.
use luuna_domain::{
    Approval, BaselineEvidence, BaselineIdentity, BaselineStatus, CheckResult, CheckStatus,
    DirtyState, DomainError, ExecutionProfile, ManifestCheck, PlanRevision, ProjectManifest,
    ProjectPolicy, RepositoryIdentity, RepositoryManifest, RepositoryObservation, RunState,
    SCHEMA_VERSION,
};
use luuna_storage::{StorageError, Store};
use std::collections::BTreeMap;

const REMOTE: &str = "https://example.invalid/app.git";
const HOST: &str = "sha256:1111111111111111111111111111111111111111111111111111111111111111";

fn observation() -> RepositoryObservation {
    RepositoryObservation {
        local_path: "/srv/checkouts/app".into(),
        identity: RepositoryIdentity {
            remote: Some(REMOTE.into()),
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
        observed_at: "2026-10-17T00:00:00.000Z".into(),
    }
}
fn manifest() -> ProjectManifest {
    ProjectManifest {
        schema_version: "0.1".into(),
        id: "project".into(),
        name: "Project".into(),
        revision: 1,
        repositories: vec![RepositoryManifest {
            id: "app".into(),
            remote: REMOTE.into(),
            default_branch: "main".into(),
            profiles: vec!["rust".into()],
            execution: ExecutionProfile::Native,
            checks: vec![ManifestCheck {
                id: "unit".into(),
                executable: "cargo".into(),
                args: vec!["test".into(), "--locked".into()],
                required: true,
                timeout_seconds: 120,
                capabilities: vec!["rust".into()],
                cwd: ".".into(),
            }],
        }],
        relationships: vec![],
        policy: ProjectPolicy::v1(1, 1, 2),
    }
}

/// Register, confirm, plan, approve: a workflow run sitting in `Approved`.
struct Fixture {
    store: Store,
    run: luuna_storage::RunRecord,
    plan: PlanRevision,
}
fn approved_workflow() -> Fixture {
    let mut store = Store::open(":memory:").unwrap();
    store
        .register_project("project", "Project", &[("app".into(), observation())])
        .unwrap();
    let fresh = BTreeMap::from([("app".to_owned(), observation())]);
    let project = store
        .confirm_manifest("project", 0, &manifest(), &fresh)
        .unwrap();
    let ticket = luuna_domain::Ticket {
        schema_version: SCHEMA_VERSION.into(),
        id: "ticket-1".into(),
        project_id: "project".into(),
        revision: 1,
        source: luuna_domain::TicketSource::LuunaSqliteTemporary,
        title: "Fixture feature".into(),
        intent: "Implement the fixture feature.".into(),
        acceptance_criteria: vec!["The fixture test passes.".into()],
        repository_ids: vec!["app".into()],
        purpose: luuna_domain::TicketPurpose::Feature,
        ticket_type: Default::default(),
        priority: luuna_domain::DEFAULT_TICKET_PRIORITY,
        labels: vec![],
        created_at: "2026-10-17T00:00:00.000Z".into(),
        updated_at: "2026-10-17T00:00:00.000Z".into(),
    };
    store.create_ticket(&ticket, "fixture-key").unwrap();
    let mut plan: PlanRevision =
        serde_json::from_str(include_str!("../../../fixtures/domain/plan.valid.json")).unwrap();
    plan.manifest_hash = project.manifest_hash.clone().unwrap();
    plan.base_commits
        .insert("app".into(), observation().head_commit);
    let plan = plan.seal().unwrap();
    store.save_plan(&plan).unwrap();
    let run = store
        .create_workflow_run("workflow", "project", serde_json::json!({}))
        .unwrap();
    let run = store
        .transition_run(
            &run.id,
            run.revision,
            RunState::Exploring,
            "run.exploring",
            serde_json::json!({}),
        )
        .unwrap();
    let run = store
        .bind_plan(&run.id, run.revision, &plan.id, plan.revision)
        .unwrap();
    let approval = Approval {
        schema_version: SCHEMA_VERSION.into(),
        id: "approval-1".into(),
        plan_id: plan.id.clone(),
        plan_revision: plan.revision,
        plan_hash: plan.hash.clone(),
        approved_by: "human".into(),
        approved_at: "2026-10-17T00:00:00Z".into(),
        permissions: plan.permissions.clone(),
    };
    let run = store
        .approve_plan(&run.id, run.revision, &approval)
        .unwrap();
    assert_eq!(run.state, RunState::Approved);
    Fixture { store, run, plan }
}

fn identity(plan: &PlanRevision) -> BaselineIdentity {
    BaselineIdentity {
        manifest_hash: plan.manifest_hash.clone(),
        repository_id: "app".into(),
        commit: plan.base_commits["app"].clone(),
        tree: "3".repeat(40),
    }
}
fn evidence(id: &str, run_id: &str, identity: BaselineIdentity) -> BaselineEvidence {
    BaselineEvidence {
        id: id.into(),
        run_id: run_id.into(),
        project_id: "project".into(),
        manifest_revision: 1,
        identity,
        environment_fingerprint: HOST.into(),
        status: BaselineStatus::Running,
        required_checks: vec!["unit".into()],
        results: vec![],
        started_at: "2026-10-17T00:00:00.000Z".into(),
        finished_at: None,
        reason: None,
    }
}
fn result(id: &str, run_id: &str, identity: &BaselineIdentity, status: CheckStatus) -> CheckResult {
    CheckResult {
        schema_version: SCHEMA_VERSION.into(),
        id: id.into(),
        run_id: run_id.into(),
        repository_id: "app".into(),
        recipe_id: "unit".into(),
        candidate_hash: identity.hash().unwrap(),
        commit: identity.commit.clone(),
        environment_fingerprint: HOST.into(),
        command: vec!["cargo".into(), "test".into(), "--locked".into()],
        status,
        exit_code: if status == CheckStatus::Passed {
            Some(0)
        } else {
            Some(101)
        },
        artifact_refs: vec![format!("artifact:{run_id}:unit:stdout")],
        cwd: ".".into(),
        started_at: None,
        finished_at: None,
        reason: None,
        artifact_hashes: BTreeMap::new(),
        tests_detected: Some(5),
        truncated_artifacts: Vec::new(),
    }
}

/// Record a full baseline run with one result and the given outcome.
fn record_baseline(store: &mut Store, id: &str, identity: BaselineIdentity, status: CheckStatus) {
    let run_id = format!("baseline-run-{id}");
    store
        .create_run(&run_id, "project", serde_json::json!({"kind":"baseline"}))
        .unwrap();
    store
        .start_baseline(&evidence(id, &run_id, identity.clone()))
        .unwrap();
    store
        .record_check_result(
            id,
            &result(&format!("{id}-unit"), &run_id, &identity, status),
        )
        .unwrap();
    let aggregate =
        BaselineStatus::from_results(&store.get_baseline(id).unwrap().unwrap().results, 1);
    store.finish_baseline(id, aggregate, None).unwrap();
    settle_run(store, &run_id, RunState::Completed);
}

/// Drive a baseline run through `running` to its terminal state, as the worker does.
fn settle_run(store: &mut Store, run_id: &str, terminal: RunState) {
    let run = store.get_run(run_id).unwrap().unwrap();
    let run = store
        .transition_run(
            &run.id,
            run.revision,
            RunState::Running,
            "run.running",
            serde_json::json!({}),
        )
        .unwrap();
    store
        .transition_run(
            &run.id,
            run.revision,
            terminal,
            "run.settled",
            serde_json::json!({}),
        )
        .unwrap();
}

#[test]
fn a_passed_verdict_whose_run_never_settled_is_not_evidence() {
    let Fixture {
        mut store,
        run,
        plan,
    } = approved_workflow();
    // The crash window: the verdict committed, the run still running.
    store
        .create_run(
            "baseline-run-crash",
            "project",
            serde_json::json!({"kind":"baseline"}),
        )
        .unwrap();
    let crash_run = store.get_run("baseline-run-crash").unwrap().unwrap();
    store
        .transition_run(
            &crash_run.id,
            crash_run.revision,
            RunState::Running,
            "run.running",
            serde_json::json!({}),
        )
        .unwrap();
    let identity = identity(&plan);
    store
        .start_baseline(&evidence("crash", "baseline-run-crash", identity.clone()))
        .unwrap();
    store
        .record_check_result(
            "crash",
            &result(
                "crash-unit",
                "baseline-run-crash",
                &identity,
                CheckStatus::Passed,
            ),
        )
        .unwrap();
    store
        .finish_baseline("crash", BaselineStatus::Passed, None)
        .unwrap();
    let error = store
        .begin_implementation(&run.id, run.revision, HOST)
        .unwrap_err();
    assert!(error.to_string().contains("not completed"), "{error}");
    // Startup recovery reconciles the window: the run and its verdict are interrupted.
    assert_eq!(store.interrupt_active_runs().unwrap(), 1);
    let reconciled = store.get_baseline("crash").unwrap().unwrap();
    assert_eq!(reconciled.status, BaselineStatus::Interrupted);
    assert!(reconciled.reason.unwrap().contains("run settled"));
    assert_eq!(
        store.get_run("baseline-run-crash").unwrap().unwrap().state,
        RunState::Interrupted
    );
    assert!(store
        .begin_implementation(&run.id, run.revision, HOST)
        .is_err());
    // A properly settled pass still authorizes.
    record_baseline(&mut store, "settled", identity, CheckStatus::Passed);
    let run = store
        .begin_implementation(&run.id, run.revision, HOST)
        .unwrap();
    assert_eq!(run.state, RunState::Implementing);
}

#[test]
fn implementation_requires_a_matching_passed_baseline() {
    let Fixture {
        mut store,
        run,
        plan,
    } = approved_workflow();
    assert!(matches!(
        store.begin_implementation(&run.id, run.revision, HOST),
        Err(StorageError::Domain(DomainError::BaselineRequired(_)))
    ));
    let mut stale_commit = identity(&plan);
    stale_commit.commit = "9".repeat(40);
    record_baseline(
        &mut store,
        "stale-commit",
        stale_commit,
        CheckStatus::Passed,
    );
    assert!(store
        .begin_implementation(&run.id, run.revision, HOST)
        .is_err());
    record_baseline(&mut store, "failed", identity(&plan), CheckStatus::Failed);
    assert!(store
        .begin_implementation(&run.id, run.revision, HOST)
        .is_err());
    record_baseline(&mut store, "good", identity(&plan), CheckStatus::Passed);
    assert!(
        store
            .begin_implementation(&run.id, run.revision, "sha256:other-host")
            .is_err(),
        "another environment's baseline cannot unlock a writer"
    );
    assert_eq!(
        store.get_run(&run.id).unwrap().unwrap().state,
        RunState::Approved
    );
    let implementing = store
        .begin_implementation(&run.id, run.revision, HOST)
        .unwrap();
    assert_eq!(implementing.state, RunState::Implementing);
    let events = store.events_after(&run.id, 0).unwrap();
    let entered = events.last().unwrap();
    assert_eq!(entered.event_type, "run.implementing");
    assert_eq!(entered.payload["baselines"]["app"], "good");
    assert_eq!(entered.payload["manifest_hash"], plan.manifest_hash);
    let project_events = store.project_events_after("project", 0).unwrap();
    assert!(project_events
        .iter()
        .any(|e| e.event_type == "baseline.finished" && e.payload["status"] == "passed"));
}

#[test]
fn a_later_manifest_revision_or_blocked_baseline_fails_closed() {
    let Fixture {
        mut store,
        run,
        plan,
    } = approved_workflow();
    record_baseline(&mut store, "good", identity(&plan), CheckStatus::Passed);
    // A newer baseline that is blocked supersedes the older passed one.
    record_baseline(&mut store, "blocked", identity(&plan), CheckStatus::Blocked);
    assert!(store
        .begin_implementation(&run.id, run.revision, HOST)
        .is_err());
    record_baseline(
        &mut store,
        "good-again",
        identity(&plan),
        CheckStatus::Passed,
    );
    // Re-confirming the manifest changes the project's current hash; the plan is now stale.
    let mut revised = manifest();
    revised.revision = 2;
    revised.name = "Project renamed".into();
    let fresh = BTreeMap::from([("app".to_owned(), observation())]);
    store
        .confirm_manifest("project", 1, &revised, &fresh)
        .unwrap();
    assert!(matches!(
        store.begin_implementation(&run.id, run.revision, HOST),
        Err(StorageError::Domain(DomainError::BaselineRequired(_)))
    ));
    assert_eq!(
        store.get_run(&run.id).unwrap().unwrap().state,
        RunState::Approved
    );
}

#[test]
fn running_baselines_are_interrupted_on_restart_and_results_are_immutable() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.sqlite3");
    let plan: PlanRevision =
        serde_json::from_str(include_str!("../../../fixtures/domain/plan.valid.json")).unwrap();
    {
        let mut store = Store::open(&path).unwrap();
        store
            .register_project("project", "Project", &[("app".into(), observation())])
            .unwrap();
        store
            .create_run(
                "baseline-run-open",
                "project",
                serde_json::json!({"kind":"baseline"}),
            )
            .unwrap();
        let mut identity = identity(&plan);
        identity.manifest_hash = format!("sha256:{}", "a".repeat(64));
        store
            .start_baseline(&evidence("open", "baseline-run-open", identity.clone()))
            .unwrap();
        store
            .record_check_result(
                "open",
                &result(
                    "open-unit",
                    "baseline-run-open",
                    &identity,
                    CheckStatus::Passed,
                ),
            )
            .unwrap();
        assert!(matches!(
            store.record_check_result(
                "open",
                &result(
                    "open-unit",
                    "baseline-run-open",
                    &identity,
                    CheckStatus::Passed
                )
            ),
            Err(StorageError::Sqlite(_))
        ));
        assert!(store
            .start_baseline(&evidence("open", "baseline-run-open", identity))
            .is_err());
    }
    let mut store = Store::open(&path).unwrap();
    assert_eq!(store.interrupt_active_runs().unwrap(), 1);
    let interrupted = store.get_baseline("open").unwrap().unwrap();
    assert_eq!(interrupted.status, BaselineStatus::Interrupted);
    assert_eq!(interrupted.results.len(), 1, "recorded results survive");
    assert!(interrupted.finished_at.is_some());
    assert!(store
        .record_check_result("open", &interrupted.results[0])
        .is_err());
    assert_eq!(store.baselines("project").unwrap().len(), 1);
    assert_eq!(
        store.latest_baseline("project", "app").unwrap().unwrap().id,
        "open"
    );
    let events = store.project_events_after("project", 0).unwrap();
    assert_eq!(events.last().unwrap().event_type, "baseline.finished");
    assert_eq!(events.last().unwrap().payload["status"], "interrupted");
}
