//! Project files beside store.json, and the database as a rebuildable index of
//! them: `<data>/projects/<id>/project.json` holds the registration, the
//! confirmed manifests and the retention setting; `Store::open` reconciles the
//! database with those files. Registrations use synthetic observations, so no
//! test runs git. The new error is matched by its Display text.
use luuna_domain::{
    DirtyState, ExecutionProfile, ManifestCheck, ProjectManifest, ProjectPolicy,
    RepositoryIdentity, RepositoryManifest, RepositoryObservation,
};
use luuna_storage::{MemoryRetentionPolicy, StorageError, Store};
use rusqlite::{types::ValueRef, Connection, OpenFlags};
use serde_json::{json, Map, Value};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

const JSON_COLUMNS: [&str; 4] = ["observation", "payload", "base_commits", "policy"];
const CANARY_NAME: &str = "Leak-Canary-Name";
const CANARY_KEY: &str = "leak_canary_key";

fn remote(repository: &str) -> String {
    format!("https://example.invalid/{repository}.git")
}
fn observation(path: &str, repository: &str) -> RepositoryObservation {
    RepositoryObservation {
        local_path: path.into(),
        identity: RepositoryIdentity {
            remote: Some(remote(repository)),
            push_remote: None,
            root_commits: vec!["1".repeat(40)],
            shallow: false,
        },
        head_commit: "2".repeat(40),
        branch: Some("main".into()),
        remote_default_branch: None,
        local_branches: vec!["main".into(), "feature".into()],
        dirty: DirtyState {
            unstaged: 1,
            paths: vec!["README.md".into()],
            ..DirtyState::default()
        },
        git_version: "git version 2.43.0".into(),
        observed_at: "2026-10-17T00:00:00.000Z".into(),
    }
}
fn manifest(project: &str, revision: u64, repositories: &[&str]) -> ProjectManifest {
    ProjectManifest {
        schema_version: "0.1".into(),
        id: project.into(),
        name: format!("Project {project}"),
        revision,
        repositories: repositories
            .iter()
            .map(|id| RepositoryManifest {
                id: (*id).into(),
                remote: remote(id),
                default_branch: "main".into(),
                profiles: vec!["rust".into()],
                execution: ExecutionProfile::Native,
                checks: vec![ManifestCheck {
                    id: "cargo-test".into(),
                    executable: "cargo".into(),
                    args: vec!["test".into(), "--locked".into()],
                    required: true,
                    timeout_seconds: 600 + revision,
                    capabilities: vec!["rust".into()],
                    cwd: ".".into(),
                }],
            })
            .collect(),
        relationships: vec![],
        policy: ProjectPolicy::v1(1, 1, 2),
    }
}
fn fresh(prefix: &str, repositories: &[&str]) -> BTreeMap<String, RepositoryObservation> {
    repositories
        .iter()
        .map(|id| {
            (
                (*id).to_owned(),
                observation(&format!("/srv/{prefix}/{id}"), id),
            )
        })
        .collect()
}
/// Register `project` with `repositories` checked out under `/srv/<project>/`.
fn register(store: &mut Store, project: &str, repositories: &[&str]) {
    let observations: Vec<_> = fresh(project, repositories).into_iter().collect();
    store
        .register_project(project, &format!("Project {project}"), &observations)
        .unwrap();
}
fn confirm(store: &mut Store, project: &str, expected: u64, repositories: &[&str]) {
    store
        .confirm_manifest(
            project,
            expected,
            &manifest(project, expected + 1, repositories),
            &fresh(project, repositories),
        )
        .unwrap();
}
fn policy(process: u32, project: u32) -> MemoryRetentionPolicy {
    MemoryRetentionPolicy {
        process_max_age_days: Some(process),
        project_max_age_days: Some(project),
    }
}

fn database(data: &Path) -> PathBuf {
    data.join("luuna.sqlite3")
}
fn project_file(data: &Path, id: &str) -> PathBuf {
    data.join("projects").join(id).join("project.json")
}
fn remove_database(data: &Path) {
    for suffix in ["", "-wal", "-shm"] {
        let mut name = database(data).into_os_string();
        name.push(suffix);
        let _ = fs::remove_file(name);
    }
}
fn error_text(result: Result<Store, StorageError>) -> String {
    match result {
        Ok(_) => panic!("Store::open succeeded where it had to refuse"),
        Err(error) => error.to_string(),
    }
}

fn value_of(value: ValueRef<'_>, column: &str, parse: bool) -> Value {
    match value {
        ValueRef::Null => Value::Null,
        ValueRef::Integer(number) => json!(number),
        ValueRef::Real(number) => json!(number),
        ValueRef::Text(bytes) => {
            let text = String::from_utf8(bytes.to_vec()).unwrap();
            if parse && JSON_COLUMNS.contains(&column) {
                serde_json::from_str(&text).unwrap()
            } else {
                Value::String(text)
            }
        }
        ValueRef::Blob(_) => panic!("unexpected blob in {column}"),
    }
}
/// The four covered tables, rows ordered by primary key, every column. With
/// `parse` the JSON-text columns are compared as parsed values (P50).
fn snapshot(data: &Path, parse: bool) -> Value {
    let connection =
        Connection::open_with_flags(database(data), OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let mut tables = Map::new();
    for (table, order) in [
        ("projects", "id"),
        ("repositories", "project_id,id"),
        ("manifest_revisions", "project_id,revision"),
        ("memory_retention", "project_id"),
    ] {
        let mut statement = connection
            .prepare(&format!("SELECT * FROM {table} ORDER BY {order}"))
            .unwrap();
        let columns: Vec<String> = statement
            .column_names()
            .into_iter()
            .map(str::to_owned)
            .collect();
        let rows = statement
            .query_map([], |row| {
                let mut object = Map::new();
                for (index, column) in columns.iter().enumerate() {
                    object.insert(column.clone(), value_of(row.get_ref(index)?, column, parse));
                }
                Ok(Value::Object(object))
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        tables.insert(table.to_owned(), Value::Array(rows));
    }
    Value::Object(tables)
}
fn project_exists_in_database(data: &Path, id: &str) -> bool {
    let connection =
        Connection::open_with_flags(database(data), OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let count: i64 = connection
        .query_row("SELECT COUNT(*) FROM projects WHERE id=?1", [id], |row| {
            row.get(0)
        })
        .unwrap();
    count > 0
}
/// What project.json must say about `id`, built from a parsed snapshot (P46).
fn expected_file(snapshot: &Value, id: &str) -> Value {
    let rows = |table: &str, key: &str| -> Vec<Value> {
        snapshot[table]
            .as_array()
            .unwrap()
            .iter()
            .filter(|row| row[key] == id)
            .cloned()
            .collect()
    };
    let pick = |row: &Value, columns: &[(&str, &str)]| -> Value {
        Value::Object(
            columns
                .iter()
                .map(|(name, column)| ((*name).to_owned(), row[*column].clone()))
                .collect(),
        )
    };
    let project = pick(
        &rows("projects", "id")[0],
        &[
            ("id", "id"),
            ("name", "name"),
            ("revision", "revision"),
            ("manifest_hash", "manifest_hash"),
            ("created_at", "created_at"),
            ("updated_at", "updated_at"),
        ],
    );
    let repositories: Vec<Value> = rows("repositories", "project_id")
        .iter()
        .map(|row| {
            pick(
                row,
                &[
                    ("id", "id"),
                    ("local_path", "local_path"),
                    ("observation", "observation"),
                    ("registered_at", "registered_at"),
                ],
            )
        })
        .collect();
    let manifests: Vec<Value> = rows("manifest_revisions", "project_id")
        .iter()
        .map(|row| {
            pick(
                row,
                &[
                    ("revision", "revision"),
                    ("hash", "hash"),
                    ("manifest", "payload"),
                    ("base_commits", "base_commits"),
                    ("confirmed_at", "confirmed_at"),
                ],
            )
        })
        .collect();
    let retention = rows("memory_retention", "project_id")
        .first()
        .map_or(Value::Null, |row| {
            pick(row, &[("policy", "policy"), ("updated_at", "updated_at")])
        });
    json!({
        "schema_version": 1,
        "project": project,
        "repositories": repositories,
        "manifests": manifests,
        "memory_retention": retention,
    })
}
fn read_file(data: &Path, id: &str) -> Value {
    serde_json::from_slice(&fs::read(project_file(data, id)).unwrap()).unwrap()
}

/// Everything the Store's own reads say about the given projects.
fn reads(store: &Store, projects: &[(&str, u64)]) -> Value {
    let mut all = Map::new();
    all.insert(
        "projects".into(),
        serde_json::to_value(store.projects().unwrap()).unwrap(),
    );
    for (id, revisions) in projects {
        let manifests: Vec<_> = (1..=*revisions)
            .map(|revision| store.get_manifest(id, revision).unwrap())
            .collect();
        all.insert(
            (*id).to_owned(),
            json!({
                "repositories": store.repositories(id).unwrap(),
                "manifests": manifests,
                "current": store.current_manifest(id).unwrap(),
                "retention": store.memory_retention(id).unwrap(),
            }),
        );
    }
    Value::Object(all)
}
/// Project `alpha` (two repositories, revisions 1 and 2, a retention policy)
/// and project `beta` (registered only).
fn populate(store: &mut Store) {
    register(store, "alpha", &["app", "lib"]);
    confirm(store, "alpha", 0, &["app", "lib"]);
    confirm(store, "alpha", 1, &["app", "lib"]);
    store
        .set_memory_retention("alpha", &policy(30, 90))
        .unwrap();
    register(store, "beta", &["tool"]);
}
const POPULATED: [(&str, u64); 2] = [("alpha", 2), ("beta", 0)];

#[test]
fn registration_writes_a_readable_project_file() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path();
    {
        let mut store = Store::open(database(data)).unwrap();
        register(&mut store, "alpha", &["app", "lib"]);
        confirm(&mut store, "alpha", 0, &["app", "lib"]);
        store.set_memory_retention("alpha", &policy(7, 14)).unwrap();
        store
            .rebind_repository("alpha", "app", &observation("/srv/moved/app", "app"))
            .unwrap()
            .expect("the rebind changes the path");
    }
    let path = project_file(data, "alpha");
    let text = fs::read_to_string(&path).unwrap();
    let parsed: Value = serde_json::from_str(&text).expect("project.json is JSON");
    assert_eq!(parsed["schema_version"], 1);
    // The P46 keys, in order (pretty JSON puts top-level keys at two spaces).
    let positions: Vec<usize> = [
        "schema_version",
        "project",
        "repositories",
        "manifests",
        "memory_retention",
    ]
    .iter()
    .map(|key| {
        text.find(&format!("\n  \"{key}\":"))
            .unwrap_or_else(|| panic!("top-level key {key} missing"))
    })
    .collect();
    assert!(positions.windows(2).all(|pair| pair[0] < pair[1]), "{text}");
    assert_eq!(
        parsed.as_object().unwrap().keys().len(),
        5,
        "exactly the five P46 keys"
    );
    assert_eq!(
        parsed["repositories"][0]["local_path"], "/srv/moved/app",
        "the rebind is in the file"
    );
    assert!(parsed["repositories"][0]["observation"].is_object());
    assert!(parsed["manifests"][0]["manifest"].is_object());
    assert!(parsed["manifests"][0]["base_commits"].is_object());
    assert!(parsed["memory_retention"]["policy"].is_object());
    assert_eq!(parsed, expected_file(&snapshot(data, true), "alpha"));
    assert!(!path.with_file_name("project.json.tmp").exists());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let file_mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        let folder_mode = fs::metadata(path.parent().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(file_mode, 0o600);
        assert_eq!(folder_mode, 0o700);
    }
}

#[test]
fn a_deleted_database_is_rebuilt_from_project_files() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path();
    let before = {
        let mut store = Store::open(database(data)).unwrap();
        populate(&mut store);
        reads(&store, &POPULATED)
    };
    let tables_before = snapshot(data, true);
    remove_database(data);
    assert!(!database(data).exists());

    let store = Store::open(database(data)).expect("the database is rebuilt from project files");
    assert_eq!(snapshot(data, true), tables_before);
    assert_eq!(reads(&store, &POPULATED), before);
    assert_eq!(store.projects().unwrap().len(), 2);
}

#[test]
fn an_existing_database_without_project_files_is_backfilled() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path();
    {
        let mut store = Store::open(database(data)).unwrap();
        populate(&mut store);
    }
    let raw_before = snapshot(data, false);
    for id in ["alpha", "beta"] {
        fs::remove_file(project_file(data, id)).unwrap();
    }
    {
        let _store = Store::open(database(data)).unwrap();
    }
    let parsed = snapshot(data, true);
    for id in ["alpha", "beta"] {
        assert_eq!(read_file(data, id), expected_file(&parsed, id), "{id}");
    }
    assert_eq!(
        snapshot(data, false),
        raw_before,
        "the tables are unchanged"
    );
}

#[test]
fn a_database_older_than_its_project_files_is_brought_forward() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    fs::create_dir_all(&data).unwrap();
    let older = dir.path().join("older.sqlite3");
    {
        let mut store = Store::open(database(&data)).unwrap();
        register(&mut store, "alpha", &["app", "lib"]);
        confirm(&mut store, "alpha", 0, &["app", "lib"]);
    }
    remove_sidecars(&data);
    fs::copy(database(&data), &older).unwrap();
    {
        let mut store = Store::open(database(&data)).unwrap();
        confirm(&mut store, "alpha", 1, &["app", "lib"]);
        store.set_memory_retention("alpha", &policy(5, 10)).unwrap();
    }
    let expected = snapshot(&data, true);
    let expected_reads = {
        let store = Store::open(database(&data)).unwrap();
        reads(&store, &[("alpha", 2)])
    };
    remove_sidecars(&data);
    fs::copy(&older, database(&data)).unwrap();

    let store = Store::open(database(&data)).expect("an older database is brought forward");
    assert_eq!(snapshot(&data, true), expected);
    assert_eq!(reads(&store, &[("alpha", 2)]), expected_reads);
}
fn remove_sidecars(data: &Path) {
    for suffix in ["-wal", "-shm"] {
        let mut name = database(data).into_os_string();
        name.push(suffix);
        let _ = fs::remove_file(name);
    }
}

#[test]
fn a_database_ahead_of_its_project_file_fails_closed() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path();
    let path = project_file(data, "alpha");
    let revision_one = {
        let mut store = Store::open(database(data)).unwrap();
        register(&mut store, "alpha", &["app"]);
        confirm(&mut store, "alpha", 0, &["app"]);
        fs::read(&path).unwrap()
    };
    {
        let mut store = Store::open(database(data)).unwrap();
        confirm(&mut store, "alpha", 1, &["app"]);
    }
    fs::write(&path, &revision_one).unwrap();
    remove_sidecars(data);
    let database_before = fs::read(database(data)).unwrap();

    let text = error_text(Store::open(database(data)));
    assert!(text.contains("project file"), "{text}");
    assert!(text.contains("manifest revision 2"), "{text}");
    assert_eq!(fs::read(database(data)).unwrap(), database_before);
    assert_eq!(fs::read(&path).unwrap(), revision_one);
}

/// A project file written by hand to the P46 contract.
fn handwritten(id: &str, local_path: &str) -> Value {
    json!({
        "schema_version": 1,
        "project": {
            "id": id,
            "name": CANARY_NAME,
            "revision": 0,
            "manifest_hash": null,
            "created_at": "2026-10-17T00:00:00.000Z",
            "updated_at": "2026-10-17T00:00:00.000Z"
        },
        "repositories": [{
            "id": "app",
            "local_path": local_path,
            "observation": observation(local_path, "app"),
            "registered_at": "2026-10-17T00:00:00.000Z"
        }],
        "manifests": [],
        "memory_retention": null
    })
}
fn put(data: &Path, id: &str, bytes: &[u8]) -> PathBuf {
    let path = project_file(data, id);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, bytes).unwrap();
    path
}
/// A data folder whose database exists and holds `owner`, registered at
/// `/srv/owner/app`.
fn data_folder(root: &Path, name: &str) -> PathBuf {
    let data = root.join(name);
    fs::create_dir_all(&data).unwrap();
    let mut store = Store::open(database(&data)).unwrap();
    register(&mut store, "owner", &["app"]);
    drop(store);
    data
}

#[test]
fn damaged_project_files_fail_closed() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();

    // Control: a well-formed hand-written file is indexed, a store.json-only
    // folder is left alone.
    let data = data_folder(root, "control");
    put(
        &data,
        "app",
        &serde_json::to_vec_pretty(&handwritten("app", "/srv/app/app")).unwrap(),
    );
    let solo = data.join("projects").join("solo");
    fs::create_dir_all(&solo).unwrap();
    fs::write(solo.join("store.json"), b"{}").unwrap();
    {
        let store =
            Store::open(database(&data)).expect("a valid file and a store.json-only folder");
        assert!(store.get_project("app").unwrap().is_some());
        assert!(store.get_project("solo").unwrap().is_none());
    }

    let mut cases: Vec<(&str, Vec<u8>)> = Vec::new();
    cases.push(("not-json", b"not json".to_vec()));
    let mut newer = handwritten("app", "/srv/app/app");
    newer["schema_version"] = json!(2);
    cases.push(("newer", serde_json::to_vec_pretty(&newer).unwrap()));
    let mut unknown = handwritten("app", "/srv/app/app");
    unknown[CANARY_KEY] = json!(1);
    cases.push(("unknown-key", serde_json::to_vec_pretty(&unknown).unwrap()));
    cases.push((
        "other-id",
        serde_json::to_vec_pretty(&handwritten("other", "/srv/app/app")).unwrap(),
    ));
    let mut oversized = serde_json::to_vec_pretty(&handwritten("app", "/srv/app/app")).unwrap();
    oversized.resize(8 * 1024 * 1024 + 1, b' ');
    cases.push(("oversized", oversized));
    // The path `owner` holds in the database.
    cases.push((
        "path-of-another-project",
        serde_json::to_vec_pretty(&handwritten("app", "/srv/owner/app")).unwrap(),
    ));

    for (name, bytes) in &cases {
        let data = data_folder(root, name);
        let path = put(&data, "app", bytes);
        let text = error_text(Store::open(database(&data)));
        assert!(text.starts_with("project file"), "{name}: {text}");
        assert!(!text.contains(CANARY_NAME), "{name}: {text}");
        assert!(!text.contains(CANARY_KEY), "{name}: {text}");
        if *name == "newer" {
            assert!(text.contains("newer Luuna"), "{name}: {text}");
        }
        assert_eq!(&fs::read(&path).unwrap(), bytes, "{name}: file changed");
        assert!(
            !project_exists_in_database(&data, "app"),
            "{name}: a row was written"
        );
    }

    #[cfg(unix)]
    {
        let data = data_folder(root, "symlink");
        let target = root.join("symlink-target.json");
        let bytes = serde_json::to_vec_pretty(&handwritten("app", "/srv/app/app")).unwrap();
        fs::write(&target, &bytes).unwrap();
        let path = project_file(&data, "app");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&target, &path).unwrap();
        let text = error_text(Store::open(database(&data)));
        assert!(text.starts_with("project file"), "symlink: {text}");
        assert!(!text.contains(CANARY_NAME), "symlink: {text}");
        assert_eq!(fs::read_link(&path).unwrap(), target);
        assert_eq!(fs::read(&target).unwrap(), bytes);
        assert!(!project_exists_in_database(&data, "app"));
    }
}

#[test]
fn stale_sidecars_beside_a_missing_database_are_moved_aside() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path();
    {
        let mut store = Store::open(database(data)).unwrap();
        populate(&mut store);
    }
    remove_database(data);
    let wal = data.join("luuna.sqlite3-wal");
    let shm = data.join("luuna.sqlite3-shm");
    fs::write(&wal, b"stale").unwrap();
    fs::write(&shm, b"stale").unwrap();

    let store = Store::open(database(data)).expect("stale sidecars do not stop the open");
    assert_eq!(store.projects().unwrap().len(), 2, "projects come back");

    // No file beside the database holds the stale bytes any more.
    for entry in fs::read_dir(data).unwrap() {
        let path = entry.unwrap().path();
        if path.is_file() {
            assert_ne!(fs::read(&path).unwrap(), b"stale", "{}", path.display());
        }
    }
    let moved: Vec<PathBuf> = fs::read_dir(data.join("backups"))
        .expect("a backups folder")
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("orphaned-sidecars-")
        })
        .collect();
    assert_eq!(moved.len(), 1, "{moved:?}");
    assert_eq!(
        fs::read(moved[0].join("luuna.sqlite3-wal")).unwrap(),
        b"stale"
    );
    assert_eq!(
        fs::read(moved[0].join("luuna.sqlite3-shm")).unwrap(),
        b"stale"
    );
}

#[test]
fn memory_stores_write_no_files() {
    // A ":memory:" store has no data folder (P53). The test does not change
    // the current directory and inspects no folder it does not own.
    let mut store = Store::open(":memory:").unwrap();
    register(&mut store, "alpha", &["app"]);
    confirm(&mut store, "alpha", 0, &["app"]);
    store.set_memory_retention("alpha", &policy(1, 2)).unwrap();
    assert_eq!(store.get_project("alpha").unwrap().unwrap().revision, 1);
}

#[test]
fn a_refused_write_leaves_the_project_file_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path();
    let mut store = Store::open(database(data)).unwrap();
    register(&mut store, "alpha", &["app"]);
    let path = project_file(data, "alpha");
    let before = fs::read(&path).expect("registration wrote project.json");

    let again = store.register_project(
        "alpha",
        "Again",
        &[("app".into(), observation("/srv/again/app", "app"))],
    );
    assert!(matches!(again, Err(StorageError::AlreadyExists(_))));
    assert_eq!(fs::read(&path).unwrap(), before);

    let wrong = store.confirm_manifest(
        "alpha",
        5,
        &manifest("alpha", 6, &["app"]),
        &fresh("alpha", &["app"]),
    );
    assert!(wrong.is_err());
    assert_eq!(fs::read(&path).unwrap(), before);
}
