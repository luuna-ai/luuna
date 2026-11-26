//! Project event and discovery files (ADR 0057). Private synthetic temporary
//! databases and files only; no git, model or environment mutation.
use luuna_domain::{
    DirtyState, Discovery, ExecutionProfile, ManifestCheck, ProjectManifest, ProjectPolicy,
    RepositoryDiscovery, RepositoryIdentity, RepositoryManifest, RepositoryObservation,
};
use luuna_storage::{backup_database, restore_database, StorageError, Store};
use rusqlite::Connection;
use serde_json::json;
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

const EVENTS: &str = "project-records/app/events.jsonl";
const DISCOVERIES: &str = "project-records/app/discoveries.jsonl";

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

fn discovery(root: &Path, id: &str) -> Discovery {
    Discovery {
        schema_version: "0.1".into(),
        id: id.into(),
        project_id: "app".into(),
        created_at: "2026-11-26T00:00:00.000Z".into(),
        repositories: vec![RepositoryDiscovery {
            repository_id: "api".into(),
            observation: observation(root),
            profiles: vec!["rust".into()],
            evidence: vec!["Cargo.toml".into()],
            proposed_checks: vec![],
        }],
        proposed_manifest: ProjectManifest {
            schema_version: "0.1".into(),
            id: "app".into(),
            name: "App".into(),
            revision: 1,
            repositories: vec![RepositoryManifest {
                id: "api".into(),
                remote: "https://example.invalid/app.git".into(),
                default_branch: "main".into(),
                profiles: vec!["rust".into()],
                execution: ExecutionProfile::Native,
                checks: vec![ManifestCheck {
                    id: "unit".into(),
                    executable: "cargo".into(),
                    args: vec!["test".into()],
                    required: true,
                    timeout_seconds: 120,
                    capabilities: vec!["rust".into()],
                    cwd: ".".into(),
                }],
            }],
            relationships: vec![],
            policy: ProjectPolicy::v1(1, 1, 2),
        },
        manifest_problems: vec![],
        questions: vec![],
        host_capabilities: None,
    }
}

/// A registered project with three events and two discoveries.
fn populated(root: &Path) -> Store {
    let mut store = Store::open(database(root)).unwrap();
    store
        .register_project("app", "App", &[("api".into(), observation(root))])
        .unwrap();
    store.record_discovery(&discovery(root, "d-1")).unwrap();
    store
        .record_project_event("app", "project.scaffolded", json!({"profile": "rust"}))
        .unwrap();
    store.record_discovery(&discovery(root, "d-2")).unwrap();
    store
}

/// Every event envelope and discovery payload, in database order.
fn rows(root: &Path) -> (Vec<String>, Vec<String>) {
    let connection = Connection::open(database(root)).unwrap();
    let read = |sql: &str| {
        connection
            .prepare(sql)
            .unwrap()
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .map(Result::unwrap)
            .collect::<Vec<_>>()
    };
    (
        read("SELECT envelope FROM project_events WHERE project_id='app' ORDER BY sequence"),
        read("SELECT payload FROM discoveries WHERE project_id='app' ORDER BY rowid"),
    )
}

fn lines(root: &Path, file: &str) -> Vec<String> {
    fs::read_to_string(root.join(file))
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect()
}

fn lose_database(root: &Path) {
    for suffix in ["", "-wal", "-shm"] {
        let mut name = database(root).into_os_string();
        name.push(suffix);
        let _ = fs::remove_file(PathBuf::from(name));
    }
}

#[test]
fn writers_append_each_committed_event_and_discovery_as_one_line() {
    let dir = tempfile::tempdir().unwrap();
    let store = populated(dir.path());
    let (events, discoveries) = rows(dir.path());
    assert_eq!(
        events.len(),
        5,
        "registration, repository, 2 discoveries, scaffold"
    );
    assert_eq!(lines(dir.path(), EVENTS), events);
    assert_eq!(lines(dir.path(), DISCOVERIES), discoveries);
    drop(store);
    let before = fs::read(dir.path().join(EVENTS)).unwrap();
    drop(Store::open(database(dir.path())).unwrap());
    assert_eq!(fs::read(dir.path().join(EVENTS)).unwrap(), before);
}

#[test]
fn losing_sqlite_rebuilds_the_event_stream_and_discoveries_and_continues_the_sequence() {
    let dir = tempfile::tempdir().unwrap();
    drop(populated(dir.path()));
    let expected = rows(dir.path());
    lose_database(dir.path());
    let mut store = Store::open(database(dir.path())).unwrap();
    assert_eq!(rows(dir.path()), expected);
    assert_eq!(
        store.latest_discovery("app").unwrap().unwrap().id,
        "d-2",
        "insertion order survives"
    );
    let next = store
        .record_project_event("app", "project.noted", json!({}))
        .unwrap();
    assert_eq!(next.sequence, 6);
    assert_eq!(lines(dir.path(), EVENTS).len(), 6);
}

#[test]
fn an_existing_database_backfills_missing_files() {
    let dir = tempfile::tempdir().unwrap();
    drop(populated(dir.path()));
    let (events, discoveries) = rows(dir.path());
    fs::remove_file(dir.path().join(EVENTS)).unwrap();
    fs::remove_file(dir.path().join(DISCOVERIES)).unwrap();
    drop(Store::open(database(dir.path())).unwrap());
    assert_eq!(lines(dir.path(), EVENTS), events);
    assert_eq!(lines(dir.path(), DISCOVERIES), discoveries);
}

#[test]
fn a_file_behind_the_database_is_caught_up_and_one_ahead_is_imported() {
    let dir = tempfile::tempdir().unwrap();
    drop(populated(dir.path()));
    let expected = rows(dir.path());
    // Behind: as after a crash between the commit and the append.
    let full = lines(dir.path(), EVENTS);
    fs::write(
        dir.path().join(EVENTS),
        format!("{}\n", full[..2].join("\n")),
    )
    .unwrap();
    drop(Store::open(database(dir.path())).unwrap());
    assert_eq!(lines(dir.path(), EVENTS), full);
    // Ahead: the database lost its newest rows.
    let connection = Connection::open(database(dir.path())).unwrap();
    connection
        .execute("DELETE FROM project_events WHERE sequence>3", [])
        .unwrap();
    connection
        .execute("DELETE FROM discoveries WHERE id='d-2'", [])
        .unwrap();
    drop(connection);
    drop(Store::open(database(dir.path())).unwrap());
    assert_eq!(rows(dir.path()), expected);
}

#[test]
fn a_torn_last_line_is_dropped_and_the_stream_continues() {
    let dir = tempfile::tempdir().unwrap();
    drop(populated(dir.path()));
    let full = lines(dir.path(), EVENTS);
    fs::OpenOptions::new()
        .append(true)
        .open(dir.path().join(EVENTS))
        .unwrap()
        .write_all(b"{\"schema_version\":\"0.1\",\"event_")
        .unwrap();
    let mut store = Store::open(database(dir.path())).unwrap();
    assert_eq!(lines(dir.path(), EVENTS), full);
    store
        .record_project_event("app", "project.noted", json!({}))
        .unwrap();
    let after = lines(dir.path(), EVENTS);
    assert_eq!(after.len(), full.len() + 1);
    assert_eq!(after, rows(dir.path()).0);
}

#[test]
fn a_torn_tail_written_while_open_is_repaired_by_the_next_append() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = populated(dir.path());
    fs::OpenOptions::new()
        .append(true)
        .open(dir.path().join(EVENTS))
        .unwrap()
        .write_all(b"{\"torn")
        .unwrap();
    store
        .record_project_event("app", "project.noted", json!({}))
        .unwrap();
    assert_eq!(lines(dir.path(), EVENTS), rows(dir.path()).0);
}

#[test]
fn disagreeing_malformed_or_misplaced_files_refuse_the_open_without_changes() {
    let dir = tempfile::tempdir().unwrap();
    drop(populated(dir.path()));
    let path = dir.path().join(EVENTS);
    let good = fs::read(&path).unwrap();
    let expected = rows(dir.path());
    let full = lines(dir.path(), EVENTS);
    let mut edited = full.clone();
    edited[1] = edited[1].replace("repository.registered", "repository.forged");
    let cases = [
        format!("{}\n", edited.join("\n")),
        format!("{}\nnot json\n", full.join("\n")),
        format!("{}\n{}\n", full.join("\n"), full[0]),
        format!("{}\n", full[1..].join("\n")),
    ];
    for case in cases {
        fs::write(&path, &case).unwrap();
        let refused = Store::open(database(dir.path()));
        assert!(
            matches!(refused, Err(StorageError::ProjectFile { .. })),
            "{case}"
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), case);
        assert_eq!(rows(dir.path()), expected);
    }
    fs::write(&path, &good).unwrap();
    // Another project's discovery inside this project's file.
    let other = serde_json::to_string(&Discovery {
        project_id: "other".into(),
        ..discovery(dir.path(), "d-9")
    })
    .unwrap();
    let discoveries = dir.path().join(DISCOVERIES);
    let mut text = fs::read_to_string(&discoveries).unwrap();
    text.push_str(&other);
    text.push('\n');
    fs::write(&discoveries, &text).unwrap();
    assert!(matches!(
        Store::open(database(dir.path())),
        Err(StorageError::ProjectFile { .. })
    ));
    assert_eq!(rows(dir.path()), expected);
}

#[cfg(unix)]
#[test]
fn a_linked_event_file_is_refused_and_its_target_is_never_written() {
    let dir = tempfile::tempdir().unwrap();
    drop(populated(dir.path()));
    let path = dir.path().join(EVENTS);
    let target = dir.path().join("elsewhere.jsonl");
    fs::rename(&path, &target).unwrap();
    std::os::unix::fs::symlink(&target, &path).unwrap();
    let before = fs::read(&target).unwrap();
    assert!(matches!(
        Store::open(database(dir.path())),
        Err(StorageError::ProjectFile { .. })
    ));
    assert_eq!(fs::read(&target).unwrap(), before);
}

#[test]
fn snapshot_restore_sets_newer_logs_aside_and_backfills_the_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let snapshot = dir.path().join("snapshot.sqlite3");
    let mut store = populated(dir.path());
    let expected = rows(dir.path());
    backup_database(&database(dir.path()), &snapshot).unwrap();
    store
        .record_project_event("app", "project.noted", json!({}))
        .unwrap();
    let newer = fs::read(dir.path().join(EVENTS)).unwrap();
    drop(store);
    let report = restore_database(&snapshot, &database(dir.path())).unwrap();
    assert_eq!(
        fs::read(report.replaced.unwrap().join(EVENTS)).unwrap(),
        newer
    );
    drop(Store::open(database(dir.path())).unwrap());
    assert_eq!(rows(dir.path()), expected);
    assert_eq!(lines(dir.path(), EVENTS), expected.0);
}

#[test]
fn a_memory_only_store_writes_no_log_file() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(":memory:").unwrap();
    store
        .register_project("app", "App", &[("api".into(), observation(dir.path()))])
        .unwrap();
    store
        .record_discovery(&discovery(dir.path(), "d-1"))
        .unwrap();
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
    assert!(!Path::new("project-records").exists());
}
