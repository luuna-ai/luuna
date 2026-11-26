//! Per-project ticket files (ADR 0060). Private synthetic temporary databases
//! and files only; no git, Missions binary, model or environment mutation.
use luuna_domain::{
    DirtyState, RepositoryIdentity, RepositoryObservation, Ticket, TicketPurpose, TicketSource,
    SCHEMA_VERSION,
};
use luuna_storage::{backup_database, restore_database, StorageError, Store};
use rusqlite::Connection;
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
};

const TICKETS: &str = "project-records/app/tickets.json";

fn database(root: &Path) -> PathBuf {
    root.join("luuna.sqlite3")
}

fn observation(root: &Path) -> RepositoryObservation {
    RepositoryObservation {
        local_path: root.join("checkout").to_string_lossy().into_owned(),
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

fn ticket(id: &str, title: &str) -> Ticket {
    Ticket {
        schema_version: SCHEMA_VERSION.into(),
        id: id.into(),
        project_id: "app".into(),
        revision: 1,
        source: TicketSource::LuunaSqliteTemporary,
        title: title.into(),
        intent: "Implement the fixture feature.".into(),
        acceptance_criteria: vec!["The fixture test passes.".into()],
        repository_ids: vec!["api".into()],
        purpose: TicketPurpose::Feature,
        created_at: format!("2026-11-26T00:00:0{}.000Z", id.len() % 10),
        updated_at: format!("2026-11-26T00:00:0{}.000Z", id.len() % 10),
    }
}

const SOURCE: &str = "app-abc1";

/// A registered project with a revised local ticket and a ticket delivered
/// to Missions and then reconciled once.
fn populated(root: &Path) -> Store {
    let mut store = Store::open(database(root)).unwrap();
    store
        .register_project("app", "App", &[("api".into(), observation(root))])
        .unwrap();
    store
        .create_ticket(&ticket("t-1", "Local"), "key-1")
        .unwrap();
    store
        .update_ticket(
            "t-1",
            1,
            "Local, revised",
            "Implement it again.",
            &["It passes.".into()],
        )
        .unwrap();
    store
        .create_ticket(&ticket("t-22", "Delivered"), "key-2")
        .unwrap();
    store.reserve_missions_delivery("t-22").unwrap();
    store.begin_missions_delivery("t-22").unwrap();
    let bound = store
        .complete_missions_delivery("t-22", 1, SOURCE, &"a".repeat(64))
        .unwrap();
    let projected = Ticket {
        title: "Delivered, edited in Missions".into(),
        ..bound
    };
    store
        .reconcile_missions_ticket(&projected, 1, SOURCE, &"b".repeat(64))
        .unwrap();
    store
}

/// Every ticket row and Missions binding row of the project, in order.
fn rows(root: &Path) -> Vec<String> {
    let connection = Connection::open(database(root)).unwrap();
    let mut all = vec![];
    for sql in [
        "SELECT id||'|'||revision||'|'||idempotency_key||'|'||payload||'|'||created_at||'|'||updated_at FROM tickets ORDER BY rowid",
        "SELECT ticket_id||'|'||project_id||'|'||ifnull(source_id,'-')||'|'||ifnull(source_revision,'-')||'|'||original_payload FROM ticket_sources ORDER BY ticket_id",
        "SELECT ticket_id FROM ticket_delivery_attempts ORDER BY ticket_id",
    ] {
        let mut statement = connection.prepare(sql).unwrap();
        all.extend(
            statement
                .query_map([], |r| r.get::<_, String>(0))
                .unwrap()
                .map(Result::unwrap),
        );
    }
    all
}

fn document(root: &Path) -> Value {
    serde_json::from_slice(&fs::read(root.join(TICKETS)).unwrap()).unwrap()
}

fn lose_database(root: &Path) {
    for suffix in ["", "-wal", "-shm"] {
        let mut name = database(root).into_os_string();
        name.push(suffix);
        let _ = fs::remove_file(PathBuf::from(name));
    }
}

fn refuses(root: &Path) {
    assert!(matches!(
        Store::open(database(root)),
        Err(StorageError::ProjectFile { .. })
    ));
}

#[test]
fn every_ticket_writer_leaves_the_file_equal_to_the_database() {
    let dir = tempfile::tempdir().unwrap();
    let store = populated(dir.path());
    let file = document(dir.path());
    assert_eq!(file["schema_version"], 1);
    assert_eq!(file["project_id"], "app");
    let tickets = file["tickets"].as_array().unwrap();
    assert_eq!(tickets.len(), 2);
    assert_eq!(tickets[0]["idempotency_key"], "key-1");
    assert_eq!(tickets[0]["ticket"]["revision"], 2);
    assert_eq!(tickets[0]["ticket"]["title"], "Local, revised");
    assert!(tickets[0].get("missions").is_none());
    assert_eq!(tickets[1]["ticket"]["source"], "missions");
    assert_eq!(tickets[1]["ticket"]["revision"], 2);
    assert_eq!(tickets[1]["missions"]["source_id"], SOURCE);
    assert_eq!(tickets[1]["missions"]["source_revision"], "b".repeat(64));
    assert_eq!(tickets[1]["missions"]["delivery_started"], true);
    assert_eq!(tickets[1]["missions"]["original"]["title"], "Delivered");
    drop(store);
    let before = fs::read(dir.path().join(TICKETS)).unwrap();
    drop(Store::open(database(dir.path())).unwrap());
    assert_eq!(fs::read(dir.path().join(TICKETS)).unwrap(), before);
}

#[test]
fn each_delivery_step_is_in_the_file_before_the_next_one_starts() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(database(dir.path())).unwrap();
    store
        .register_project("app", "App", &[("api".into(), observation(dir.path()))])
        .unwrap();
    store
        .create_ticket(&ticket("t-22", "Delivered"), "key-2")
        .unwrap();
    store.reserve_missions_delivery("t-22").unwrap();
    let reserved = &document(dir.path())["tickets"][0]["missions"];
    assert_eq!(reserved["delivery_started"], false);
    assert!(reserved["source_id"].is_null());
    store.begin_missions_delivery("t-22").unwrap();
    assert_eq!(
        document(dir.path())["tickets"][0]["missions"]["delivery_started"],
        true
    );
    // Lost here, a retry must still know a create may already have happened.
    drop(store);
    lose_database(dir.path());
    let store = Store::open(database(dir.path())).unwrap();
    let binding = store.missions_binding("t-22").unwrap().unwrap();
    assert!(binding.delivery_started);
    assert!(binding.source_id.is_none());
}

#[test]
fn losing_sqlite_rebuilds_tickets_bindings_and_idempotency_and_revisions_continue() {
    let dir = tempfile::tempdir().unwrap();
    drop(populated(dir.path()));
    let expected = rows(dir.path());
    let bytes = fs::read(dir.path().join(TICKETS)).unwrap();
    lose_database(dir.path());
    let mut store = Store::open(database(dir.path())).unwrap();
    assert_eq!(rows(dir.path()), expected);
    assert_eq!(fs::read(dir.path().join(TICKETS)).unwrap(), bytes);
    let tickets = store.tickets("app").unwrap();
    assert_eq!(
        tickets.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(),
        ["t-1", "t-22"]
    );
    let binding = store.missions_binding("t-22").unwrap().unwrap();
    assert_eq!(binding.source_id.as_deref(), Some(SOURCE));
    assert_eq!(binding.original.title, "Delivered");
    // The same key returns the rebuilt ticket instead of making another one.
    let (again, created) = store
        .create_ticket(&ticket("t-22", "Delivered"), "key-2")
        .unwrap();
    assert!(!created);
    assert_eq!(again.revision, 2);
    let next = store
        .update_ticket("t-1", 2, "Local, third", "Again.", &["It passes.".into()])
        .unwrap();
    assert_eq!(next.revision, 3);
    assert_eq!(document(dir.path())["tickets"][0]["ticket"]["revision"], 3);
}

#[test]
fn an_existing_database_backfills_a_missing_file() {
    let dir = tempfile::tempdir().unwrap();
    drop(populated(dir.path()));
    let bytes = fs::read(dir.path().join(TICKETS)).unwrap();
    fs::remove_file(dir.path().join(TICKETS)).unwrap();
    drop(Store::open(database(dir.path())).unwrap());
    assert_eq!(fs::read(dir.path().join(TICKETS)).unwrap(), bytes);
}

#[test]
fn a_file_behind_the_database_is_rewritten_and_one_ahead_is_imported() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(database(dir.path())).unwrap();
    store
        .register_project("app", "App", &[("api".into(), observation(dir.path()))])
        .unwrap();
    store
        .create_ticket(&ticket("t-1", "Local"), "key-1")
        .unwrap();
    store
        .create_ticket(&ticket("t-22", "Delivered"), "key-2")
        .unwrap();
    store.reserve_missions_delivery("t-22").unwrap();
    store.begin_missions_delivery("t-22").unwrap();
    let older = fs::read(dir.path().join(TICKETS)).unwrap();
    store
        .update_ticket("t-1", 1, "Local, revised", "Again.", &["It passes.".into()])
        .unwrap();
    store
        .complete_missions_delivery("t-22", 1, SOURCE, &"a".repeat(64))
        .unwrap();
    store
        .create_ticket(&ticket("t-333", "Third"), "key-3")
        .unwrap();
    drop(store);
    let newer = fs::read(dir.path().join(TICKETS)).unwrap();
    let expected = rows(dir.path());
    // Behind: as after a crash between a commit and the file replacement.
    fs::write(dir.path().join(TICKETS), &older).unwrap();
    drop(Store::open(database(dir.path())).unwrap());
    assert_eq!(fs::read(dir.path().join(TICKETS)).unwrap(), newer);
    // Ahead: the database lost its newest ticket changes.
    let connection = Connection::open(database(dir.path())).unwrap();
    connection
        .execute_batch(
            "DELETE FROM tickets WHERE id='t-333';
             UPDATE ticket_sources SET source_id=NULL,source_revision=NULL;",
        )
        .unwrap();
    let old_t1: String = Connection::open(database(dir.path()))
        .unwrap()
        .query_row("SELECT payload FROM tickets WHERE id='t-1'", [], |r| {
            r.get(0)
        })
        .unwrap();
    let mut reverted: Ticket = serde_json::from_str(&old_t1).unwrap();
    reverted.revision = 1;
    reverted.title = "Local".into();
    reverted.intent = "Implement the fixture feature.".into();
    reverted.acceptance_criteria = vec!["The fixture test passes.".into()];
    reverted.updated_at = reverted.created_at.clone();
    let mut delivered: Ticket = serde_json::from_str(
        &connection
            .query_row("SELECT payload FROM tickets WHERE id='t-22'", [], |r| {
                r.get::<_, String>(0)
            })
            .unwrap(),
    )
    .unwrap();
    delivered.source = TicketSource::LuunaSqliteTemporary;
    for (id, payload, revision) in [
        ("t-1", serde_json::to_string(&reverted).unwrap(), 1),
        ("t-22", serde_json::to_string(&delivered).unwrap(), 1),
    ] {
        connection
            .execute(
                "UPDATE tickets SET payload=?1,revision=?2,updated_at=created_at WHERE id=?3",
                rusqlite::params![payload, revision, id],
            )
            .unwrap();
    }
    drop(connection);
    drop(Store::open(database(dir.path())).unwrap());
    assert_eq!(rows(dir.path()), expected);
    assert_eq!(fs::read(dir.path().join(TICKETS)).unwrap(), newer);
}

#[test]
fn disagreeing_malformed_or_misplaced_files_refuse_the_open_without_changes() {
    let dir = tempfile::tempdir().unwrap();
    drop(populated(dir.path()));
    let path = dir.path().join(TICKETS);
    let good = document(dir.path());
    let expected = rows(dir.path());
    let edit = |change: &dyn Fn(&mut Value)| {
        let mut value = good.clone();
        change(&mut value);
        serde_json::to_vec_pretty(&value).unwrap()
    };
    let cases = [
        // Same revision, different content.
        edit(&|v| v["tickets"][0]["ticket"]["title"] = "Forged".into()),
        // A changed Missions identity.
        edit(&|v| v["tickets"][1]["missions"]["source_id"] = "app-zzz9".into()),
        // A Missions ticket without its binding.
        edit(&|v| {
            v["tickets"][1].as_object_mut().unwrap().remove("missions");
        }),
        // Another project's ticket.
        edit(&|v| v["tickets"][0]["ticket"]["project_id"] = "other".into()),
        // A duplicate idempotency key.
        edit(&|v| v["tickets"][1]["idempotency_key"] = "key-1".into()),
        // An unknown field.
        edit(&|v| v["tickets"][0]["extra"] = true.into()),
        // A newer format.
        edit(&|v| v["schema_version"] = 2.into()),
        b"not json".to_vec(),
    ];
    for case in cases {
        fs::write(&path, &case).unwrap();
        refuses(dir.path());
        assert_eq!(fs::read(&path).unwrap(), case);
        assert_eq!(rows(dir.path()), expected);
    }
    // A lost database cannot adopt a file moved from another project.
    fs::write(&path, serde_json::to_vec_pretty(&good).unwrap()).unwrap();
    lose_database(dir.path());
    let mut store = Store::open(database(dir.path())).unwrap();
    store
        .register_project(
            "other",
            "Other",
            &[("api".into(), observation(&dir.path().join("other")))],
        )
        .unwrap();
    drop(store);
    let other = dir.path().join("project-records/other/tickets.json");
    fs::copy(&path, &other).unwrap();
    refuses(dir.path());
}

#[cfg(unix)]
#[test]
fn a_linked_ticket_file_is_refused_and_its_target_is_never_written() {
    let dir = tempfile::tempdir().unwrap();
    drop(populated(dir.path()));
    let path = dir.path().join(TICKETS);
    let target = dir.path().join("elsewhere.json");
    fs::rename(&path, &target).unwrap();
    std::os::unix::fs::symlink(&target, &path).unwrap();
    let before = fs::read(&target).unwrap();
    refuses(dir.path());
    assert_eq!(fs::read(&target).unwrap(), before);
}

#[test]
fn snapshot_restore_sets_the_newer_file_aside_and_backfills_the_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let snapshot = dir.path().join("snapshot.sqlite3");
    let mut store = populated(dir.path());
    let expected = rows(dir.path());
    let older = fs::read(dir.path().join(TICKETS)).unwrap();
    backup_database(&database(dir.path()), &snapshot).unwrap();
    store
        .create_ticket(&ticket("t-333", "Third"), "key-3")
        .unwrap();
    let newer = fs::read(dir.path().join(TICKETS)).unwrap();
    drop(store);
    let report = restore_database(&snapshot, &database(dir.path())).unwrap();
    assert_eq!(
        fs::read(report.replaced.unwrap().join(TICKETS)).unwrap(),
        newer
    );
    drop(Store::open(database(dir.path())).unwrap());
    assert_eq!(rows(dir.path()), expected);
    assert_eq!(fs::read(dir.path().join(TICKETS)).unwrap(), older);
}

#[test]
fn a_memory_only_store_writes_no_ticket_file() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(":memory:").unwrap();
    store
        .register_project("app", "App", &[("api".into(), observation(dir.path()))])
        .unwrap();
    store
        .create_ticket(&ticket("t-1", "Local"), "key-1")
        .unwrap();
    assert!(!dir.path().join("project-records").exists());
    assert!(!PathBuf::from("project-records").join("app").exists());
}
