//! Schema upgrade, refusal and backup drills (ADR 0016). A database is rebuilt
//! at every earlier schema version from the shipped migration scripts and the
//! rows that version could hold, then opened by the current store; newer and
//! damaged databases are refused before they are written; a backup taken while
//! a writer is active restores exactly, even beside a stale write-ahead log.
use luuna_domain::{
    hash_value, Approval, AttemptState, BaselineEvidence, BaselineIdentity, BaselineStatus,
    CandidateSet, CheckResult, CheckStatus, DirtyState, Discovery, ExecutionProfile, LeaseState,
    ManifestCheck, PlanRevision, ProjectManifest, ProjectPolicy, RepositoryDiscovery,
    RepositoryIdentity, RepositoryManifest, RepositoryObservation, ReviewResult, ReviewVerdict,
    RunState, Ticket, TicketSource, WorktreeLease, SCHEMA_VERSION,
};
use luuna_storage::{
    backup_database, inspect_database, migration_sql, restore_database, StorageError, Store,
    DATABASE_SCHEMA_VERSION,
};
use rusqlite::{types::Value as Sql, Connection, OpenFlags};
use serde_json::json;
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

const REMOTE: &str = "https://example.invalid/app.git";
const HOST: &str = "sha256:1111111111111111111111111111111111111111111111111111111111111111";

/// The tables each shipped migration introduced.
const INTRODUCED: [(u32, &[&str]); 6] = [
    (1, &["plan_revisions", "approvals", "runs", "events"]),
    (
        2,
        &[
            "projects",
            "manifest_revisions",
            "repositories",
            "discoveries",
            "project_events",
        ],
    ),
    (3, &["baselines", "check_results"]),
    (4, &["tickets", "worktree_leases", "attempts"]),
    (
        5,
        &["candidates", "candidate_check_results", "candidate_reviews"],
    ),
    (6, &["ticket_sources", "ticket_delivery_attempts"]),
];

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
fn discovery() -> Discovery {
    Discovery {
        schema_version: "0.1".into(),
        id: "discovery-1".into(),
        project_id: "project".into(),
        created_at: "2026-10-17T00:00:00.000Z".into(),
        repositories: vec![RepositoryDiscovery {
            repository_id: "app".into(),
            observation: observation(),
            profiles: vec!["rust".into()],
            evidence: vec!["Cargo.toml".into()],
            proposed_checks: vec![],
        }],
        proposed_manifest: manifest(),
        manifest_problems: vec![],
        questions: vec![],
        host_capabilities: None,
    }
}
fn ticket(id: &str) -> Ticket {
    Ticket {
        schema_version: SCHEMA_VERSION.into(),
        id: id.into(),
        project_id: "project".into(),
        revision: 1,
        source: TicketSource::LuunaSqliteTemporary,
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
    }
}
fn plan(manifest_hash: &str) -> PlanRevision {
    let mut plan: PlanRevision =
        serde_json::from_str(include_str!("../../../fixtures/domain/plan.valid.json")).unwrap();
    plan.revision = 1;
    plan.ticket_id = "ticket-1".into();
    plan.ticket_revision = 1;
    plan.manifest_hash = manifest_hash.to_owned();
    plan.base_commits
        .insert("app".into(), observation().head_commit);
    plan.seal().unwrap()
}
fn approval(plan: &PlanRevision) -> Approval {
    Approval {
        schema_version: SCHEMA_VERSION.into(),
        id: "approval-1".into(),
        plan_id: plan.id.clone(),
        plan_revision: plan.revision,
        plan_hash: plan.hash.clone(),
        approved_by: "human".into(),
        approved_at: "2026-10-17T00:00:00Z".into(),
        permissions: plan.permissions.clone(),
    }
}
fn check_result(id: &str, run_id: &str, candidate_hash: &str, commit: &str) -> CheckResult {
    CheckResult {
        schema_version: SCHEMA_VERSION.into(),
        id: id.into(),
        run_id: run_id.into(),
        repository_id: "app".into(),
        recipe_id: "unit".into(),
        candidate_hash: candidate_hash.into(),
        commit: commit.into(),
        environment_fingerprint: HOST.into(),
        command: vec!["cargo".into(), "test".into(), "--locked".into()],
        status: CheckStatus::Passed,
        exit_code: Some(0),
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
fn passed_baseline(store: &mut Store, plan: &PlanRevision) {
    let identity = BaselineIdentity {
        manifest_hash: plan.manifest_hash.clone(),
        repository_id: "app".into(),
        commit: plan.base_commits["app"].clone(),
        tree: "3".repeat(40),
    };
    let run = store
        .create_run("baseline-run", "project", json!({"kind":"baseline"}))
        .unwrap();
    store
        .start_baseline(&BaselineEvidence {
            id: "baseline".into(),
            run_id: run.id.clone(),
            project_id: "project".into(),
            manifest_revision: 1,
            identity: identity.clone(),
            environment_fingerprint: HOST.into(),
            status: BaselineStatus::Running,
            required_checks: vec!["unit".into()],
            results: vec![],
            started_at: "2026-10-17T00:00:00.000Z".into(),
            finished_at: None,
            reason: None,
        })
        .unwrap();
    store
        .record_check_result(
            "baseline",
            &check_result(
                "baseline-unit",
                &run.id,
                &identity.hash().unwrap(),
                &identity.commit,
            ),
        )
        .unwrap();
    store
        .finish_baseline("baseline", BaselineStatus::Passed, None)
        .unwrap();
    let run = store
        .transition_run(
            &run.id,
            run.revision,
            RunState::Running,
            "run.running",
            json!({}),
        )
        .unwrap();
    store
        .transition_run(
            &run.id,
            run.revision,
            RunState::Completed,
            "run.completed",
            json!({}),
        )
        .unwrap();
}

/// Every kind of record the current schema holds: a demo run, a confirmed
/// project with a discovery, a baseline, and a workflow taken through lease,
/// attempt, candidate, required check and independent review to `reviewed`,
/// plus a Missions delivery intent.
fn populate(path: &Path) {
    let mut store = Store::open(path).unwrap();
    let demo = store
        .create_run("demo-run", "demo", json!({"prompt":"read only","tools":[]}))
        .unwrap();
    let demo = store
        .transition_run(
            &demo.id,
            demo.revision,
            RunState::Running,
            "run.running",
            json!({}),
        )
        .unwrap();
    store
        .append_event(&demo.id, "pi.message_end", json!({"text":"done"}))
        .unwrap();
    store
        .transition_run(
            &demo.id,
            demo.revision,
            RunState::Completed,
            "run.completed",
            json!({}),
        )
        .unwrap();
    store
        .register_project("project", "Project", &[("app".into(), observation())])
        .unwrap();
    store.record_discovery(&discovery()).unwrap();
    let fresh = BTreeMap::from([("app".to_owned(), observation())]);
    let hash = store
        .confirm_manifest("project", 0, &manifest(), &fresh)
        .unwrap()
        .manifest_hash
        .unwrap();
    store.create_ticket(&ticket("ticket-1"), "key-1").unwrap();
    let plan = plan(&hash);
    let run = store.open_plan_workflow("workflow-1", &plan).unwrap();
    let run = store
        .approve_plan(&run.id, run.revision, &approval(&plan))
        .unwrap();
    passed_baseline(&mut store, &plan);
    let run = store
        .begin_implementation(&run.id, run.revision, HOST)
        .unwrap();
    store
        .acquire_worktree_lease(&WorktreeLease {
            id: "lease-1".into(),
            run_id: run.id.clone(),
            project_id: "project".into(),
            repository_id: "app".into(),
            path: "/private/worktrees/1".into(),
            base_commit: observation().head_commit,
            state: LeaseState::Active,
            candidate_commit: None,
            created_at: "2026-10-17T00:00:00.000Z".into(),
            released_at: None,
            reason: None,
        })
        .unwrap();
    store
        .start_attempt(
            "writer-1",
            &run.id,
            "implement",
            plan.budgets.max_attempts,
            json!({}),
        )
        .unwrap();
    let commit = "c".repeat(40);
    store
        .release_worktree_lease(
            "lease-1",
            LeaseState::Released,
            Some(&commit),
            Some("candidate preserved for checks and review"),
        )
        .unwrap();
    store
        .finish_attempt(
            "writer-1",
            AttemptState::Completed,
            json!({"usage":{"elapsed_seconds":10,"cost_usd_micros":100}}),
        )
        .unwrap();
    let mut candidate = CandidateSet {
        schema_version: SCHEMA_VERSION.into(),
        id: "candidate-1".into(),
        revision: 1,
        plan_hash: plan.hash.clone(),
        repository_commits: BTreeMap::from([("app".into(), commit.clone())]),
        dependency_bindings: BTreeMap::new(),
        hash: String::new(),
    };
    candidate.hash = candidate.computed_hash().unwrap();
    let run = store.get_run(&run.id).unwrap().unwrap();
    store
        .record_candidate(
            &run.id,
            run.revision,
            &candidate,
            &BTreeMap::from([("app".into(), "d".repeat(40))]),
            "writer-1",
        )
        .unwrap();
    let run = store.get_run(&run.id).unwrap().unwrap();
    store
        .begin_candidate_validation(&run.id, run.revision, &candidate.hash)
        .unwrap();
    store
        .record_candidate_check(
            &run.id,
            &candidate.hash,
            &check_result("candidate-unit", &run.id, &candidate.hash, &commit),
        )
        .unwrap();
    let run = store.get_run(&run.id).unwrap().unwrap();
    store
        .finish_candidate_validation(&run.id, run.revision, &candidate.hash)
        .unwrap();
    let run = store.get_run(&run.id).unwrap().unwrap();
    store
        .finish_candidate_review(
            &run.id,
            run.revision,
            &candidate.hash,
            &ReviewResult {
                schema_version: SCHEMA_VERSION.into(),
                id: "review-1".into(),
                candidate_hash: candidate.hash.clone(),
                reviewer_session: "independent-reviewer".into(),
                findings: vec![],
                verdict: ReviewVerdict::Passed,
                evidence: vec!["artifact:workflow-1:review".into()],
            },
        )
        .unwrap();
    assert_eq!(
        store.get_run(&run.id).unwrap().unwrap().state,
        RunState::Reviewed
    );
    store.create_ticket(&ticket("ticket-2"), "key-2").unwrap();
    store.reserve_missions_delivery("ticket-2").unwrap();
    store.begin_missions_delivery("ticket-2").unwrap();
}

/// Build `target` as a database at `level` would have been: only the shipped
/// migrations up to it, holding the source's rows for the tables it had.
fn build_level(source: &Path, target: &Path, level: u32) {
    let connection = Connection::open(target).unwrap();
    connection
        .execute_batch("PRAGMA journal_mode=WAL;")
        .unwrap();
    for version in 1..=level {
        connection
            .execute_batch(migration_sql(version).unwrap())
            .unwrap();
    }
    connection
        .execute("ATTACH DATABASE ?1 AS source", [source.to_str().unwrap()])
        .unwrap();
    for table in tables_at(level) {
        // Only the columns the table had at `level`: a later migration may
        // have added columns (migration 13 adds projects.removed_at).
        let columns: Vec<String> = connection
            .prepare(&format!("PRAGMA main.table_info({table})"))
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        let columns = columns.join(",");
        connection
            .execute_batch(&format!(
                "INSERT INTO main.{table}({columns}) SELECT {columns} FROM source.{table};"
            ))
            .unwrap();
    }
    connection.execute_batch("DETACH DATABASE source;").unwrap();
}

fn tables_at(level: u32) -> Vec<&'static str> {
    INTRODUCED
        .iter()
        .filter(|(introduced, _)| *introduced <= level)
        .flat_map(|(_, tables)| tables.iter().copied())
        .collect()
}

fn read_only(path: &Path) -> Connection {
    Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap()
}

fn rows(path: &Path, table: &str) -> Vec<Vec<Sql>> {
    let connection = read_only(path);
    let mut statement = connection
        .prepare(&format!("SELECT * FROM {table} ORDER BY rowid"))
        .unwrap();
    let columns = statement.column_count();
    statement
        .query_map([], |row| {
            (0..columns).map(|i| row.get::<_, Sql>(i)).collect()
        })
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

fn dump(path: &Path, tables: &[&str]) -> BTreeMap<String, Vec<Vec<Sql>>> {
    tables
        .iter()
        .map(|table| (table.to_string(), rows(path, table)))
        .collect()
}

/// Every table, index and trigger with its definition.
fn schema(path: &Path) -> Vec<(String, String, String)> {
    let connection = read_only(path);
    let mut statement = connection
        .prepare("SELECT type,name,COALESCE(sql,'') FROM sqlite_master WHERE name NOT LIKE 'sqlite_%' ORDER BY type,name")
        .unwrap();
    statement
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

fn backups(directory: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(directory.join("backups")) else {
        return Vec::new();
    };
    let mut paths: Vec<_> = entries.map(|entry| entry.unwrap().path()).collect();
    paths.sort();
    paths
}

fn sidecar(path: &Path, suffix: &str) -> PathBuf {
    PathBuf::from(format!("{}{suffix}", path.display()))
}

#[test]
fn every_earlier_schema_version_upgrades_with_rows_intact_and_later_tables_usable() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source/luuna.sqlite3");
    populate(&source);
    let current = Store::open(&source).unwrap();
    let fresh = root.path().join("fresh/luuna.sqlite3");
    drop(Store::open(&fresh).unwrap());
    // A new database records every version and never needs a backup.
    assert!(backups(fresh.parent().unwrap()).is_empty());
    assert_eq!(
        inspect_database(&fresh).unwrap().schema_version,
        DATABASE_SCHEMA_VERSION
    );
    for level in 1..DATABASE_SCHEMA_VERSION {
        let directory = root.path().join(format!("level-{level}"));
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("luuna.sqlite3");
        build_level(&source, &path, level);
        assert_eq!(inspect_database(&path).unwrap().schema_version, level);
        let tables = tables_at(level);
        let before = dump(&path, &tables);
        assert!(before.values().any(|rows| !rows.is_empty()));

        let mut store = Store::open(&path).unwrap();
        let status = inspect_database(&path).unwrap();
        assert_eq!(
            status.schema_version, DATABASE_SCHEMA_VERSION,
            "level {level}"
        );
        assert_eq!(status.integrity_problem, None);
        // Columns a later migration added (migration 13's projects.removed_at)
        // start NULL; every column the rows had is unchanged.
        let mut after = dump(&path, &tables);
        for (table, rows) in after.iter_mut() {
            let width = before[table].first().map_or(0, Vec::len);
            for row in rows.iter_mut() {
                assert!(
                    row[width..].iter().all(|value| *value == Sql::Null),
                    "level {level} {table}"
                );
                row.truncate(width);
            }
        }
        assert_eq!(after, before, "level {level} rows changed");
        assert_eq!(
            schema(&path),
            schema(&fresh),
            "level {level} schema differs"
        );
        // One automatic snapshot of the database as it was before migrating.
        let snapshots = backups(&directory);
        assert_eq!(snapshots.len(), 1, "level {level}: {snapshots:?}");
        let name = snapshots[0].file_name().unwrap().to_string_lossy();
        assert!(
            name.starts_with(&format!("pre-migration-v{level}-")) && name.ends_with(".sqlite3"),
            "{name}"
        );
        assert_eq!(
            inspect_database(&snapshots[0]).unwrap().schema_version,
            level
        );
        assert_eq!(dump(&snapshots[0], &tables), before);

        // Old rows decode through the current API exactly as the source does.
        for run in ["demo-run", "workflow-1", "baseline-run"] {
            assert_eq!(store.get_run(run).unwrap(), current.get_run(run).unwrap());
            assert_eq!(
                store.events_after(run, 0).unwrap(),
                current.events_after(run, 0).unwrap()
            );
        }
        let plan_id = plan(HOST).id;
        assert_eq!(
            store.latest_plan(&plan_id).unwrap(),
            current.latest_plan(&plan_id).unwrap()
        );
        if level >= 2 {
            assert_eq!(
                store.get_project("project").unwrap(),
                current.get_project("project").unwrap()
            );
            assert_eq!(
                store.latest_discovery("project").unwrap(),
                current.latest_discovery("project").unwrap()
            );
            assert_eq!(
                store.project_events_after("project", 0).unwrap(),
                current.project_events_after("project", 0).unwrap()
            );
        }
        if level >= 3 {
            assert_eq!(
                store.baselines("project").unwrap(),
                current.baselines("project").unwrap()
            );
        }
        if level >= 4 {
            assert_eq!(
                store.tickets("project").unwrap(),
                current.tickets("project").unwrap()
            );
            assert_eq!(
                store.worktree_leases("workflow-1").unwrap(),
                current.worktree_leases("workflow-1").unwrap()
            );
            assert_eq!(
                store.attempts("workflow-1").unwrap(),
                current.attempts("workflow-1").unwrap()
            );
        }
        if level >= 5 {
            assert_eq!(
                store.candidates("workflow-1").unwrap(),
                current.candidates("workflow-1").unwrap()
            );
        }
        if level >= 6 {
            assert!(
                store
                    .missions_binding("ticket-2")
                    .unwrap()
                    .unwrap()
                    .delivery_started
            );
        }

        // Tables this level never had accept writes through the API.
        if level < 2 {
            store
                .register_project("project", "Project", &[("app".into(), observation())])
                .unwrap();
        }
        store
            .create_ticket(&ticket("ticket-later"), "later")
            .unwrap();
        store.reserve_missions_delivery("ticket-later").unwrap();
        assert!(store.baselines("project").is_ok());
        drop(store);
        // Reopening applies nothing again and takes no further snapshot.
        drop(Store::open(&path).unwrap());
        assert_eq!(backups(&directory).len(), 1);
    }
}

#[test]
fn shipped_migrations_are_never_edited() {
    // A database never re-runs a migration it recorded, so an edited script
    // would silently diverge from every database created before the edit.
    // Change the schema with a new migration instead.
    let shipped = [
        (
            1,
            "sha256:838113b1530f6ef535556174e032a4915cb5a39903e38cd97af02692a5d52116",
        ),
        (
            2,
            "sha256:a9dfb64df25d6d9dde590322728284e80f73d330d828d470d69337331967ada8",
        ),
        (
            3,
            "sha256:7429ac2bc45872c5ea29d0581984b6454f87123898e777e1009ea649803d483d",
        ),
        (
            4,
            "sha256:a9fd8dc5d3b860225cca2c38d4265b6422bcd28bf9ea757da60dc5f925475253",
        ),
        (
            5,
            "sha256:4b303e12b3a9139ab2fa561416d2967253ef39c838249e103940d160a3a78b7e",
        ),
        (
            6,
            "sha256:ca67553d2349b9bafb5b6773dd28610537f862a3603aa6c572de3cf0ca525989",
        ),
        (
            7,
            "sha256:2affb407c1dbea3af54f25d5710699a45b3056af87b5a11239309be7dbdf0274",
        ),
    ];
    for (version, digest) in shipped {
        let sql = migration_sql(version).unwrap();
        assert_eq!(
            hash_value(&json!(sql)).unwrap(),
            digest,
            "migration {version} was edited"
        );
        assert!(sql.contains(&format!(
            "INSERT OR IGNORE INTO schema_migrations(version) VALUES ({version});"
        )));
    }
    assert!(migration_sql(0).is_none());
    assert!(migration_sql(DATABASE_SCHEMA_VERSION + 1).is_none());
}

#[test]
fn lookups_use_indexes_instead_of_scanning() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("luuna.sqlite3");
    drop(Store::open(&path).unwrap());
    let connection = read_only(&path);
    let plan = |sql: &str| -> String {
        let mut statement = connection
            .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
            .unwrap();
        statement
            .query_map([], |r| r.get::<_, String>(3))
            .unwrap()
            .map(Result::unwrap)
            .collect::<Vec<_>>()
            .join("; ")
    };
    for (sql, index) in [
        (
            "SELECT payload FROM check_results WHERE baseline_id='b' ORDER BY rowid",
            "check_results_baseline",
        ),
        (
            "SELECT id FROM baselines WHERE project_id='p' AND repository_id='r' ORDER BY rowid DESC LIMIT 1",
            "baselines_repository",
        ),
        (
            "SELECT id FROM runs WHERE plan_id='p' AND plan_revision=1 ORDER BY rowid DESC LIMIT 1",
            "runs_plan",
        ),
        (
            "SELECT payload FROM worktree_leases WHERE run_id='r'",
            "worktree_leases_run",
        ),
        (
            "SELECT payload FROM discoveries WHERE project_id='p' ORDER BY rowid DESC LIMIT 1",
            "discoveries_project",
        ),
    ] {
        let explained = plan(sql);
        assert!(explained.contains(index), "{sql}: {explained}");
        assert!(!explained.contains("TEMP B-TREE"), "{sql}: {explained}");
    }
}

#[test]
fn a_database_from_a_newer_schema_is_refused_before_anything_is_written() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("luuna.sqlite3");
    let mut store = Store::open(&path).unwrap();
    let run = store.create_run("run", "project", json!({})).unwrap();
    store
        .transition_run(
            &run.id,
            run.revision,
            RunState::Running,
            "run.running",
            json!({}),
        )
        .unwrap();
    drop(store);
    // As a newer Luuna would leave it: a later version, a table and a run
    // state this build has never heard of.
    let newer = DATABASE_SCHEMA_VERSION + 1;
    let connection = Connection::open(&path).unwrap();
    connection
        .execute(
            "INSERT INTO schema_migrations(version) VALUES (?1)",
            [newer],
        )
        .unwrap();
    connection
        .execute_batch(
            "CREATE TABLE future_records(id TEXT PRIMARY KEY); UPDATE runs SET state='\"publishing\"';",
        )
        .unwrap();
    drop(connection);
    let before = fs::read(&path).unwrap();
    let error = Store::open(&path).err().expect("newer schema refused");
    assert!(
        matches!(error, StorageError::SchemaTooNew { found, supported, .. } if found == newer && supported == DATABASE_SCHEMA_VERSION),
        "{error:?}"
    );
    let message = error.to_string();
    assert!(message.contains("newer than version"), "{message}");
    assert!(message.contains("luuna-runtime restore"), "{message}");
    assert_eq!(format!("{error:?}"), message);
    assert_eq!(fs::read(&path).unwrap(), before);
    assert!(backups(root.path()).is_empty());
    let status = inspect_database(&path).unwrap();
    assert_eq!(status.schema_version, newer);
    assert_eq!(status.integrity_problem, None);
    assert!(matches!(
        status.require_usable(),
        Err(StorageError::SchemaTooNew { .. })
    ));
}

/// The classic manual-restore mistake: an older main file copied back beside
/// the write-ahead log a hard kill left. SQLite replays the stale log onto it.
fn older_main_file_beside_a_stale_log(root: &Path) -> PathBuf {
    let live = root.join("live/luuna.sqlite3");
    let mut store = Store::open(&live).unwrap();
    store.create_run("run", "project", json!({})).unwrap();
    drop(store);
    let older = root.join("older.sqlite3");
    fs::copy(&live, &older).unwrap();
    let connection = Connection::open(&live).unwrap();
    let insert = |from: u32, to: u32| {
        let tx = connection.unchecked_transaction().unwrap();
        for sequence in from..to {
            tx.execute(
                "INSERT INTO events(run_id,sequence,event_id,envelope) VALUES ('run',?1,?2,?3)",
                rusqlite::params![sequence, format!("run:{sequence}"), "x".repeat(400)],
            )
            .unwrap();
        }
        tx.commit().unwrap();
    };
    insert(2, 400);
    connection
        .execute_batch("PRAGMA wal_checkpoint(TRUNCATE); PRAGMA wal_autocheckpoint=0;")
        .unwrap();
    insert(400, 500);
    // What a hard kill leaves on disk: the main file and an unapplied log.
    let crashed = root.join("crashed");
    fs::create_dir_all(&crashed).unwrap();
    let path = crashed.join("luuna.sqlite3");
    fs::copy(sidecar(&live, "-wal"), sidecar(&path, "-wal")).unwrap();
    drop(connection);
    fs::copy(&older, &path).unwrap();
    path
}

#[test]
fn a_damaged_database_is_refused_and_names_the_restore_command() {
    let root = tempfile::tempdir().unwrap();
    let path = older_main_file_beside_a_stale_log(root.path());
    let error = Store::open(&path).err().expect("damaged database refused");
    assert!(matches!(error, StorageError::Corrupt { .. }), "{error:?}");
    let message = error.to_string();
    assert!(message.contains("integrity check"), "{message}");
    assert!(
        message.contains("luuna-runtime restore --input"),
        "{message}"
    );
    // The service reports a startup error with `Debug`; it says the same.
    assert_eq!(format!("{error:?}"), message);
    let status = inspect_database(&path).unwrap();
    assert!(status.integrity_problem.is_some());
    assert!(status.require_usable().is_err());

    // A file that is not a database at all is refused the same way.
    let junk = root.path().join("junk/luuna.sqlite3");
    fs::create_dir_all(junk.parent().unwrap()).unwrap();
    fs::write(
        &junk,
        "this is not a SQLite database, only text of some length\n".repeat(200),
    )
    .unwrap();
    assert!(matches!(
        Store::open(&junk),
        Err(StorageError::Corrupt { .. })
    ));
}

#[test]
fn online_backup_restores_exactly_and_quarantines_a_stale_log() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("data/luuna.sqlite3");
    populate(&path);
    // The open store is the running runtime: recent commits live in the log.
    let mut store = Store::open(&path).unwrap();
    let run = store
        .create_run("before-backup", "project", json!({}))
        .unwrap();
    store
        .append_event(&run.id, "worker.output", json!({"text":"kept"}))
        .unwrap();
    assert!(fs::metadata(sidecar(&path, "-wal")).unwrap().len() > 0);
    let output = root.path().join("snapshot.sqlite3");
    let report = backup_database(&path, &output).unwrap();
    assert_eq!(report.path, output);
    assert_eq!(report.schema_version, DATABASE_SCHEMA_VERSION);
    assert_eq!(report.size_bytes, fs::metadata(&output).unwrap().len());
    // One self-contained file; the same output is never overwritten.
    assert!(!sidecar(&output, "-wal").exists());
    assert!(matches!(
        backup_database(&path, &output),
        Err(StorageError::AlreadyExists(_))
    ));
    let mut all: Vec<&str> = tables_at(DATABASE_SCHEMA_VERSION);
    all.extend([
        "component_projects",
        "component_operations",
        "component_memories",
    ]);
    let snapshot = dump(&output, &all);
    assert_eq!(snapshot, dump(&path, &all));
    let runs = store.runs().unwrap();

    // Work continues after the snapshot, then the runtime is hard-killed.
    store
        .create_run("after-backup", "project", json!({}))
        .unwrap();
    let crashed = root.path().join("crashed/luuna.sqlite3");
    fs::create_dir_all(crashed.parent().unwrap()).unwrap();
    fs::copy(&path, &crashed).unwrap();
    fs::copy(sidecar(&path, "-wal"), sidecar(&crashed, "-wal")).unwrap();
    drop(store);

    let restored = restore_database(&output, &crashed).unwrap();
    assert_eq!(restored.database, crashed);
    assert_eq!(restored.schema_version, DATABASE_SCHEMA_VERSION);
    let replaced = restored.replaced.expect("previous files kept");
    assert!(replaced.starts_with(root.path().join("crashed/backups")));
    assert!(replaced.join("luuna.sqlite3").exists());
    assert!(replaced.join("luuna.sqlite3-wal").exists());
    for suffix in ["-wal", "-shm", "-journal"] {
        assert!(
            !sidecar(&crashed, suffix).exists(),
            "{suffix} left beside the restore"
        );
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&crashed).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    let mut store = Store::open(&crashed).unwrap();
    assert_eq!(dump(&crashed, &all), snapshot);
    assert_eq!(store.runs().unwrap(), runs);
    assert!(store.get_run("after-backup").unwrap().is_none());
    assert_eq!(
        store.events_after("before-backup", 0).unwrap().len(),
        2,
        "the snapshot holds commits that were only in the log"
    );
    // The snapshot's queued run is recovered like after any restart.
    assert_eq!(store.interrupt_active_runs().unwrap(), 1);
    drop(store);
    // Nothing was discarded: the replaced pair still opens with later work.
    let previous = Store::open(replaced.join("luuna.sqlite3")).unwrap();
    assert!(previous.get_run("after-backup").unwrap().is_some());
}

#[test]
fn restore_refuses_damaged_newer_or_incomplete_input_and_keeps_the_database() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("data/luuna.sqlite3");
    let mut store = Store::open(&path).unwrap();
    store.create_run("current", "project", json!({})).unwrap();
    drop(store);
    let original = fs::read(&path).unwrap();
    let keeps_database = |error: StorageError| {
        assert_eq!(fs::read(&path).unwrap(), original, "{error:?}");
        assert!(backups(&root.path().join("data")).is_empty(), "{error:?}");
        let mut leftovers: Vec<_> = fs::read_dir(root.path().join("data"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| name.contains(".restore-"))
            .collect();
        leftovers.sort();
        assert!(leftovers.is_empty(), "{leftovers:?}");
    };

    // A copy whose pages were damaged after the header.
    let damaged_copy = root.path().join("damaged-copy.sqlite3");
    let mut bytes = original.clone();
    assert!(bytes.len() > 8192);
    for byte in &mut bytes[4096..] {
        *byte = 0xa5;
    }
    fs::write(&damaged_copy, bytes).unwrap();
    let error = restore_database(&damaged_copy, &path).unwrap_err();
    assert!(matches!(error, StorageError::Corrupt { .. }), "{error:?}");
    keeps_database(error);
    let junk = root.path().join("junk.sqlite3");
    fs::write(&junk, "not a database ".repeat(400)).unwrap();
    let error = restore_database(&junk, &path).unwrap_err();
    assert!(matches!(error, StorageError::Corrupt { .. }), "{error:?}");
    keeps_database(error);

    // A main file with its log beside it may be incomplete on its own.
    let damaged = older_main_file_beside_a_stale_log(&root.path().join("damaged"));
    let error = restore_database(&damaged, &path).unwrap_err();
    assert!(matches!(error, StorageError::Invalid(_)), "{error:?}");
    keeps_database(error);

    let newer = root.path().join("newer.sqlite3");
    let connection = Connection::open(&newer).unwrap();
    connection.execute_batch(migration_sql(1).unwrap()).unwrap();
    connection
        .execute(
            "INSERT INTO schema_migrations(version) VALUES (?1)",
            [DATABASE_SCHEMA_VERSION + 1],
        )
        .unwrap();
    drop(connection);
    let error = restore_database(&newer, &path).unwrap_err();
    assert!(
        matches!(error, StorageError::SchemaTooNew { .. }),
        "{error:?}"
    );
    keeps_database(error);

    let empty = root.path().join("empty.sqlite3");
    Connection::open(&empty)
        .unwrap()
        .execute_batch("CREATE TABLE unrelated(id INTEGER);")
        .unwrap();
    let error = restore_database(&empty, &path).unwrap_err();
    assert!(matches!(error, StorageError::Invalid(_)), "{error:?}");
    keeps_database(error);

    let missing = root.path().join("missing.sqlite3");
    let error = restore_database(&missing, &path).unwrap_err();
    assert!(matches!(error, StorageError::NotFound(_)), "{error:?}");
    keeps_database(error);

    // A valid snapshot whose predecessor cannot be moved aside: nothing is
    // replaced and the verified copy is not left behind.
    let valid = root.path().join("valid.sqlite3");
    backup_database(&path, &valid).unwrap();
    let blocker = root.path().join("data/backups");
    fs::write(&blocker, "not a directory").unwrap();
    let error = restore_database(&valid, &path).unwrap_err();
    fs::remove_file(&blocker).unwrap();
    keeps_database(error);
}
