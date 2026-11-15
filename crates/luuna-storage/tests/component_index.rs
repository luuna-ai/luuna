//! Private synthetic temporary databases and files only. No git, model,
//! environment mutation or user/component-store discovery is needed.
use luuna_domain::{DirtyState, RepositoryIdentity, RepositoryObservation};
use luuna_storage::{backup_database, restore_database, MemoryBinding, Store};
use rusqlite::{types::ValueRef, Connection};
use serde_json::{json, Value};
use std::{
    fs,
    path::{Path, PathBuf},
};
const FILE: &str = "project-records/components.json";
fn database(root: &Path) -> PathBuf {
    root.join("luuna.sqlite3")
}
fn binding(id: &str, scope: &str) -> MemoryBinding {
    MemoryBinding {
        id: id.into(),
        project_id: "app".into(),
        scope: scope.into(),
        source_id: format!("mx-{id}"),
        domain: "app".into(),
        created_at: "2026-11-15T00:00:00Z".into(),
        deleted: false,
    }
}
fn register(store: &mut Store, root: &Path) {
    store
        .register_project(
            "app",
            "App",
            &[(
                "api".into(),
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
                    observed_at: "2026-11-15T00:00:00Z".into(),
                },
            )],
        )
        .unwrap();
}
fn populated(root: &Path) -> Store {
    let mut store = Store::open(database(root)).unwrap();
    register(&mut store, root);
    store.begin_component_enable("app").unwrap();
    store.finish_component_enable("app").unwrap();
    store
        .record_memory_binding(&binding("one", "project"))
        .unwrap();
    store
        .record_memory_binding(&binding("two", "user"))
        .unwrap();
    store.delete_memory_binding("two").unwrap();
    store
}
fn rows(root: &Path) -> Value {
    let connection = Connection::open(database(root)).unwrap();
    let mut result = serde_json::Map::new();
    for (name,sql) in [("projects","SELECT project_id,state,updated_at FROM component_projects ORDER BY project_id"),("memories","SELECT id,project_id,scope,source_id,domain,created_at,deleted FROM component_memories ORDER BY id")] {
  let mut statement=connection.prepare(sql).unwrap();let count=statement.column_count();let data=statement.query_map([],|r| {Ok((0..count).map(|i|match r.get_ref(i).unwrap(){ValueRef::Null=>Value::Null,ValueRef::Integer(v)=>json!(v),ValueRef::Text(v)=>json!(std::str::from_utf8(v).unwrap()),_=>panic!("unexpected column type")}).collect::<Vec<_>>())}).unwrap().map(|r|json!(r.unwrap())).collect::<Vec<_>>();result.insert(name.into(),json!(data));
 }
    Value::Object(result)
}
fn document(root: &Path) -> Value {
    serde_json::from_slice(&fs::read(root.join(FILE)).unwrap()).unwrap()
}
#[test]
fn component_writers_save_readable_metadata_without_the_operation_ledger() {
    let dir = tempfile::tempdir().unwrap();
    let store = populated(dir.path());
    store
        .begin_component_operation(
            "app",
            "private-ledger-only",
            "memory.create",
            &json!({"private":"synthetic ledger payload"}),
            "memory-unconfirmed",
        )
        .unwrap();
    store
        .mark_component_operation_attempted("app", "private-ledger-only")
        .unwrap();
    let bytes = fs::read(dir.path().join(FILE)).unwrap();
    let file: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(file["schema_version"], 1);
    assert_eq!(file["projects"][0]["project_id"], "app");
    assert_eq!(file["projects"][0]["state"], "enabled");
    assert_eq!(file["memories"].as_array().unwrap().len(), 2);
    assert_eq!(file["memories"][1]["scope"], "user");
    assert_eq!(file["memories"][1]["deleted"], true);
    assert!(!String::from_utf8(bytes)
        .unwrap()
        .contains("private-ledger-only"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(dir.path().join(FILE))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(dir.path().join("project-records"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
    }
}
#[test]
fn losing_sqlite_rebuilds_activation_bindings_tombstones_and_timestamps_but_no_ledger() {
    let dir = tempfile::tempdir().unwrap();
    let store = populated(dir.path());
    store
        .set_memory_binding_created_at("one", "2025-01-01T00:00:00Z")
        .unwrap();
    store
        .begin_component_operation(
            "app",
            "private-ledger-only",
            "memory.create",
            &json!({}),
            "pending",
        )
        .unwrap();
    let expected = rows(dir.path());
    drop(store);
    fs::remove_file(database(dir.path())).unwrap();
    let store = Store::open(database(dir.path())).unwrap();
    assert_eq!(
        store.component_state("app").unwrap().as_deref(),
        Some("enabled")
    );
    assert_eq!(rows(dir.path()), expected);
    assert!(store.memory_binding("app", "two").unwrap().unwrap().deleted);
    assert!(store
        .component_operation("app", "private-ledger-only")
        .unwrap()
        .is_none());
    assert!(store.get_project("app").unwrap().is_some());
}
#[test]
fn an_existing_database_backfills_the_component_file_and_repeated_open_changes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    drop(populated(dir.path()));
    let expected = rows(dir.path());
    let file = dir.path().join(FILE);
    if file.exists() {
        fs::remove_file(&file).unwrap();
    }
    drop(Store::open(database(dir.path())).unwrap());
    assert!(file.is_file());
    assert_eq!(rows(dir.path()), expected);
    let bytes = fs::read(&file).unwrap();
    drop(Store::open(database(dir.path())).unwrap());
    assert_eq!(fs::read(file).unwrap(), bytes);
    assert_eq!(rows(dir.path()), expected);
}
#[test]
fn file_ahead_activation_and_tombstones_bring_an_older_index_forward() {
    let dir = tempfile::tempdir().unwrap();
    drop(populated(dir.path()));
    let expected = rows(dir.path());
    let before = fs::read(dir.path().join(FILE)).unwrap();
    let c = Connection::open(database(dir.path())).unwrap();
    c.execute_batch("DELETE FROM component_memories WHERE id='two'; UPDATE component_projects SET state='initializing'; UPDATE component_memories SET created_at='2020-01-01T00:00:00Z';").unwrap();
    drop(c);
    drop(Store::open(database(dir.path())).unwrap());
    assert_eq!(rows(dir.path()), expected);
    assert_eq!(fs::read(dir.path().join(FILE)).unwrap(), before);
}
#[test]
fn missing_history_changed_identity_and_resurrected_tombstones_refuse_without_changes() {
    for change in 0..4 {
        let dir = tempfile::tempdir().unwrap();
        drop(populated(dir.path()));
        let before = rows(dir.path());
        let mut file = document(dir.path());
        match change {
            0 => {
                file["memories"].as_array_mut().unwrap().remove(0);
            }
            1 => file["memories"][0]["source_id"] = json!("changed-private-id"),
            2 => file["memories"][1]["deleted"] = json!(false),
            _ => file["projects"][0]["state"] = json!("initializing"),
        };
        let bytes = serde_json::to_vec_pretty(&file).unwrap();
        fs::write(dir.path().join(FILE), &bytes).unwrap();
        let error = Store::open(database(dir.path()))
            .err()
            .expect("incompatible component file must refuse");
        assert!(!format!("{error}").contains("changed-private-id"));
        assert_eq!(rows(dir.path()), before);
        assert_eq!(fs::read(dir.path().join(FILE)).unwrap(), bytes);
    }
}
#[test]
fn malformed_newer_or_duplicate_records_are_refused_before_project_reconciliation() {
    for change in 0..5 {
        let dir = tempfile::tempdir().unwrap();
        drop(populated(dir.path()));
        let before = rows(dir.path());
        let mut file = document(dir.path());
        match change {
            0 => file["schema_version"] = json!(2),
            1 => file["unreviewed-private-field"] = json!("do-not-log"),
            2 => file["memories"][0]["deleted"] = json!("wrong-type"),
            3 => {
                let duplicate = file["memories"][0].clone();
                file["memories"].as_array_mut().unwrap().push(duplicate);
            }
            _ => {
                file.as_object_mut().unwrap().remove("projects");
            }
        };
        fs::write(dir.path().join(FILE), serde_json::to_vec(&file).unwrap()).unwrap();
        let project = dir.path().join("project-records/app/project.json");
        let mut p: Value = serde_json::from_slice(&fs::read(&project).unwrap()).unwrap();
        p["project"]["name"] = json!("file-ahead-name");
        fs::write(&project, serde_json::to_vec(&p).unwrap()).unwrap();
        let error = Store::open(database(dir.path()))
            .err()
            .expect("invalid component file refuses");
        let text = format!("{error}");
        assert!(!text.contains("do-not-log"));
        assert!(!text.contains("unreviewed-private-field"));
        assert_eq!(rows(dir.path()), before);
        let c = Connection::open(database(dir.path())).unwrap();
        assert_eq!(
            c.query_row("SELECT name FROM projects WHERE id='app'", [], |r| r
                .get::<_, String>(0))
                .unwrap(),
            "App"
        );
    }
}
#[test]
fn refused_file_writes_roll_back_activation_and_binding_changes() {
    for activation in [true, false] {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(database(dir.path())).unwrap();
        register(&mut store, dir.path());
        fs::create_dir_all(dir.path().join(FILE)).unwrap();
        let before = rows(dir.path());
        let result = if activation {
            store.begin_component_enable("app")
        } else {
            store.record_memory_binding(&binding("one", "project"))
        };
        assert!(result.is_err());
        assert_eq!(rows(dir.path()), before);
        assert!(dir.path().join(FILE).is_dir());
    }
}
#[test]
fn failed_sql_commit_restores_the_previous_component_file() {
    let dir = tempfile::tempdir().unwrap();
    drop(populated(dir.path()));
    let c = Connection::open(database(dir.path())).unwrap();
    c.execute_batch("PRAGMA foreign_keys=ON; CREATE TABLE fixture_parent(id INTEGER PRIMARY KEY); CREATE TABLE fixture_child(id INTEGER REFERENCES fixture_parent(id) DEFERRABLE INITIALLY DEFERRED); CREATE TRIGGER fixture_failure AFTER INSERT ON component_memories BEGIN INSERT INTO fixture_child VALUES (999); END;").unwrap();
    drop(c);
    let store = Store::open(database(dir.path())).unwrap();
    let before = fs::read(dir.path().join(FILE)).unwrap();
    let expected = rows(dir.path());
    assert!(store
        .record_memory_binding(&binding("three", "project"))
        .is_err());
    assert_eq!(fs::read(dir.path().join(FILE)).unwrap(), before);
    assert_eq!(rows(dir.path()), expected);
}
#[cfg(unix)]
#[test]
fn linked_component_files_never_write_their_target() {
    let dir = tempfile::tempdir().unwrap();
    drop(populated(dir.path()));
    let target = tempfile::tempdir().unwrap();
    let external = target.path().join("private.json");
    fs::write(&external, b"do-not-read-or-change").unwrap();
    fs::remove_file(dir.path().join(FILE)).unwrap();
    std::os::unix::fs::symlink(&external, dir.path().join(FILE)).unwrap();
    assert!(Store::open(database(dir.path())).is_err());
    assert_eq!(fs::read(&external).unwrap(), b"do-not-read-or-change");
    assert!(fs::symlink_metadata(dir.path().join(FILE))
        .unwrap()
        .file_type()
        .is_symlink());
}
#[test]
fn snapshot_restore_sets_the_new_component_metadata_aside_and_recovers_its_old_state() {
    let dir = tempfile::tempdir().unwrap();
    let snapshot = dir.path().join("snapshot.sqlite3");
    let store = populated(dir.path());
    let expected = rows(dir.path());
    let old = document(dir.path());
    backup_database(&database(dir.path()), &snapshot).unwrap();
    store
        .record_memory_binding(&binding("three", "process"))
        .unwrap();
    let newer = fs::read(dir.path().join(FILE)).unwrap();
    drop(store);
    let report = restore_database(&snapshot, &database(dir.path())).unwrap();
    assert_eq!(
        fs::read(report.replaced.unwrap().join(FILE)).unwrap(),
        newer
    );
    drop(Store::open(database(dir.path())).unwrap());
    assert_eq!(rows(dir.path()), expected);
    assert_eq!(document(dir.path()), old);
}
#[test]
fn memory_only_store_never_writes_a_component_record_file() {
    if std::env::var_os("LUUNA_COMPONENT_INDEX_MEMORY_CHILD").is_some() {
        let mut store = Store::open(":memory:").unwrap();
        register(&mut store, &std::env::current_dir().unwrap());
        store.begin_component_enable("app").unwrap();
        store.finish_component_enable("app").unwrap();
        store
            .record_memory_binding(&binding("one", "project"))
            .unwrap();
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let mut c = std::process::Command::new(std::env::current_exe().unwrap());
    c.arg("--exact")
        .arg("memory_only_store_never_writes_a_component_record_file")
        .env("LUUNA_COMPONENT_INDEX_MEMORY_CHILD", "1")
        .current_dir(dir.path());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x08000000);
    }
    let output = c.output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[test]
fn oversized_non_json_and_duplicate_field_documents_refuse_without_echoing_content() {
    for case in 0..3 {
        let dir = tempfile::tempdir().unwrap();
        drop(populated(dir.path()));
        let expected = rows(dir.path());
        let original = fs::read_to_string(dir.path().join(FILE)).unwrap();
        let bytes = match case {
            0 => vec![b'x'; 8 * 1024 * 1024 + 1],
            1 => b"not-json-private-marker".to_vec(),
            _ => {
                let changed = original.replacen(
                    "\"schema_version\": 1",
                    "\"schema_version\": 1, \"schema_version\": 1",
                    1,
                );
                assert_ne!(changed, original);
                changed.into_bytes()
            }
        };
        fs::write(dir.path().join(FILE), bytes).unwrap();
        let error = Store::open(database(dir.path()))
            .err()
            .expect("unusable component document refuses");
        assert!(!format!("{error}").contains("not-json-private-marker"));
        assert_eq!(rows(dir.path()), expected);
    }
}
