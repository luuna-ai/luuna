//! The per-project store file `<data_dir>/projects/<id>/store.json`: the one
//! place a project's store mode is kept. An absent file means runtime mode;
//! any file that exists but cannot be trusted fails closed, and writes are
//! private, atomic and never follow a link.
use luuna_domain::{StoreMode, StoreSide};
use luuna_storage::{project_store_file_path, read_project_store_mode, write_project_store_mode};
use std::{fs, path::Path};

const BOTH_REPOSITORY_JSON: &str = "{\n  \"schema_version\": 1,\n  \"project_id\": \"app\",\n  \"mode\": \"both\",\n  \"repository\": \"api\",\n  \"writable\": \"repository\"\n}\n";
const VALID_REPOSITORY_JSON: &str = "{\n  \"schema_version\": 1,\n  \"project_id\": \"app\",\n  \"mode\": \"repository\",\n  \"repository\": \"api\"\n}\n";

fn repository(name: &str) -> StoreMode {
    StoreMode::Repository {
        repository: name.into(),
    }
}
fn both(name: &str, writable: StoreSide) -> StoreMode {
    StoreMode::Both {
        repository: name.into(),
        writable,
    }
}
fn entries(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}
/// Put `bytes` at `<data_dir>/projects/<id>/store.json`.
fn put(data_dir: &Path, id: &str, bytes: &[u8]) {
    let directory = data_dir.join("projects").join(id);
    fs::create_dir_all(&directory).unwrap();
    fs::write(directory.join("store.json"), bytes).unwrap();
}

#[test]
fn an_absent_file_or_folder_means_runtime() {
    let data = tempfile::tempdir().unwrap();
    // No projects folder at all.
    assert_eq!(
        read_project_store_mode(data.path(), "app").unwrap(),
        StoreMode::Runtime
    );
    // A project folder without a store file.
    fs::create_dir_all(data.path().join("projects").join("app")).unwrap();
    assert_eq!(
        read_project_store_mode(data.path(), "app").unwrap(),
        StoreMode::Runtime
    );
    assert_eq!(StoreMode::default(), StoreMode::Runtime);
}

#[test]
fn every_mode_round_trips_through_a_readable_file() {
    let modes = [
        StoreMode::Runtime,
        repository("api"),
        both("api", StoreSide::Runtime),
        both("api", StoreSide::Repository),
    ];
    for mode in modes {
        let data = tempfile::tempdir().unwrap();
        write_project_store_mode(data.path(), "app", &mode).unwrap();
        assert_eq!(read_project_store_mode(data.path(), "app").unwrap(), mode);
        let path = project_store_file_path(data.path(), "app").unwrap();
        assert_eq!(
            path,
            data.path().join("projects").join("app").join("store.json")
        );
        assert!(path.is_file());
    }
    let data = tempfile::tempdir().unwrap();
    write_project_store_mode(data.path(), "app", &both("api", StoreSide::Repository)).unwrap();
    let path = project_store_file_path(data.path(), "app").unwrap();
    assert_eq!(fs::read_to_string(path).unwrap(), BOTH_REPOSITORY_JSON);
}

#[test]
fn the_written_file_is_private_and_replaced_atomically() {
    let data = tempfile::tempdir().unwrap();
    write_project_store_mode(data.path(), "app", &repository("api")).unwrap();
    let directory = data.path().join("projects").join("app");
    let file = directory.join("store.json");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = |path: &Path| fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&file), 0o600);
        assert_eq!(mode(&data.path().join("projects")), 0o700);
        assert_eq!(mode(&directory), 0o700);
    }
    write_project_store_mode(data.path(), "app", &both("api", StoreSide::Runtime)).unwrap();
    assert_eq!(
        read_project_store_mode(data.path(), "app").unwrap(),
        both("api", StoreSide::Runtime)
    );
    assert_eq!(entries(&directory), vec!["store.json".to_string()]);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&file).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}

#[test]
fn invalid_project_ids_are_refused_before_any_path_is_used() {
    let long = "a".repeat(65);
    for id in ["", "../x", "A", "a/b", "a b", long.as_str()] {
        let data = tempfile::tempdir().unwrap();
        assert!(
            project_store_file_path(data.path(), id).is_err(),
            "path {id:?}"
        );
        assert!(
            read_project_store_mode(data.path(), id).is_err(),
            "read {id:?}"
        );
        assert!(
            write_project_store_mode(data.path(), id, &StoreMode::Runtime).is_err(),
            "write {id:?}"
        );
        assert!(
            entries(data.path()).is_empty(),
            "{id:?} left something behind"
        );
    }
}

#[test]
fn unusable_files_fail_closed_instead_of_defaulting() {
    let mut padded = VALID_REPOSITORY_JSON.as_bytes().to_vec();
    padded.resize(4097, b' ');
    let cases: Vec<(&str, Vec<u8>)> = vec![
        ("not json", b"this is not json".to_vec()),
        ("empty object", b"{}".to_vec()),
        ("array", b"[]".to_vec()),
        (
            "unknown key",
            br#"{"schema_version":1,"project_id":"app","mode":"runtime","extra":true}"#.to_vec(),
        ),
        (
            "schema_version missing",
            br#"{"project_id":"app","mode":"runtime"}"#.to_vec(),
        ),
        (
            "schema_version 2",
            br#"{"schema_version":2,"project_id":"app","mode":"runtime"}"#.to_vec(),
        ),
        (
            "another project",
            br#"{"schema_version":1,"project_id":"other","mode":"runtime"}"#.to_vec(),
        ),
        (
            "unknown mode",
            br#"{"schema_version":1,"project_id":"app","mode":"shared"}"#.to_vec(),
        ),
        (
            "repository mode without repository",
            br#"{"schema_version":1,"project_id":"app","mode":"repository"}"#.to_vec(),
        ),
        (
            "both without writable",
            br#"{"schema_version":1,"project_id":"app","mode":"both","repository":"api"}"#
                .to_vec(),
        ),
        (
            "runtime with writable",
            br#"{"schema_version":1,"project_id":"app","mode":"runtime","writable":"runtime"}"#
                .to_vec(),
        ),
        (
            "runtime with repository",
            br#"{"schema_version":1,"project_id":"app","mode":"runtime","repository":"api"}"#
                .to_vec(),
        ),
        (
            "writable neither",
            br#"{"schema_version":1,"project_id":"app","mode":"both","repository":"api","writable":"neither"}"#
                .to_vec(),
        ),
        (
            "repository path",
            br#"{"schema_version":1,"project_id":"app","mode":"repository","repository":"../api"}"#
                .to_vec(),
        ),
        ("oversized file", padded),
    ];
    for (name, bytes) in cases {
        let data = tempfile::tempdir().unwrap();
        put(data.path(), "app", &bytes);
        assert!(
            read_project_store_mode(data.path(), "app").is_err(),
            "{name} must not read as a mode"
        );
    }
    // The sanity case: the padding-free form of the oversized file is usable.
    let data = tempfile::tempdir().unwrap();
    put(data.path(), "app", VALID_REPOSITORY_JSON.as_bytes());
    assert_eq!(
        read_project_store_mode(data.path(), "app").unwrap(),
        repository("api")
    );
    // store.json being a directory.
    let data = tempfile::tempdir().unwrap();
    fs::create_dir_all(data.path().join("projects").join("app").join("store.json")).unwrap();
    assert!(read_project_store_mode(data.path(), "app").is_err());
}

#[cfg(unix)]
#[test]
fn links_in_the_project_file_path_are_refused() {
    use std::os::unix::fs::symlink;
    let valid = VALID_REPOSITORY_JSON.as_bytes();

    // store.json is a link to a valid file elsewhere.
    let data = tempfile::tempdir().unwrap();
    let elsewhere = tempfile::tempdir().unwrap();
    fs::write(elsewhere.path().join("valid.json"), valid).unwrap();
    let directory = data.path().join("projects").join("app");
    fs::create_dir_all(&directory).unwrap();
    symlink(
        elsewhere.path().join("valid.json"),
        directory.join("store.json"),
    )
    .unwrap();
    assert!(read_project_store_mode(data.path(), "app").is_err());
    assert!(write_project_store_mode(data.path(), "app", &StoreMode::Runtime).is_err());
    assert_eq!(
        fs::read(elsewhere.path().join("valid.json")).unwrap(),
        valid
    );
    assert_eq!(entries(&directory), vec!["store.json".to_string()]);
    assert_eq!(entries(elsewhere.path()), vec!["valid.json".to_string()]);

    // projects/<id> is a link to a folder holding a valid file.
    let data = tempfile::tempdir().unwrap();
    let elsewhere = tempfile::tempdir().unwrap();
    let target = elsewhere.path().join("folder");
    fs::create_dir_all(&target).unwrap();
    fs::write(target.join("store.json"), valid).unwrap();
    fs::create_dir_all(data.path().join("projects")).unwrap();
    symlink(&target, data.path().join("projects").join("app")).unwrap();
    assert!(read_project_store_mode(data.path(), "app").is_err());
    assert!(write_project_store_mode(data.path(), "app", &StoreMode::Runtime).is_err());
    assert_eq!(fs::read(target.join("store.json")).unwrap(), valid);
    assert_eq!(entries(&target), vec!["store.json".to_string()]);

    // projects is a link to a folder holding projects/app/store.json.
    let data = tempfile::tempdir().unwrap();
    let elsewhere = tempfile::tempdir().unwrap();
    put(elsewhere.path(), "app", valid);
    symlink(
        elsewhere.path().join("projects"),
        data.path().join("projects"),
    )
    .unwrap();
    assert!(read_project_store_mode(data.path(), "app").is_err());
    assert!(write_project_store_mode(data.path(), "app", &StoreMode::Runtime).is_err());
    let through = elsewhere.path().join("projects").join("app");
    assert_eq!(fs::read(through.join("store.json")).unwrap(), valid);
    assert_eq!(entries(&through), vec!["store.json".to_string()]);
}

#[test]
fn writing_an_invalid_mode_writes_nothing() {
    let data = tempfile::tempdir().unwrap();
    assert!(write_project_store_mode(data.path(), "app", &repository("Bad/id")).is_err());
    assert!(!data
        .path()
        .join("projects")
        .join("app")
        .join("store.json")
        .exists());
}
