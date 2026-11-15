//! Consistent online backups, verified restores and the read-only inspection
//! that gates opening a database (ADR 0016).
//!
//! The database runs in WAL mode, so committed state can live in the `-wal`
//! file until a checkpoint: copying the main file alone while the runtime runs
//! loses it, and placing an older main file beside a newer `-wal` corrupts it.
//! A backup is therefore a `VACUUM INTO` snapshot, a single self-contained file
//! taken through SQLite while writers continue. A restore needs the data
//! directory's owner (the runtime stopped), verifies the file it installs and
//! moves the current database together with its `-wal`/`-shm` aside.
use crate::{applied_versions, Result, StorageError, DATABASE_SCHEMA_VERSION};
use rusqlite::{Connection, ErrorCode, OpenFlags};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    time::Duration,
};

/// Files SQLite may keep beside a database; they belong to that exact file.
const SIDECARS: [&str; 3] = ["-wal", "-shm", "-journal"];

/// Schema version and structural health of one database file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DatabaseStatus {
    pub path: PathBuf,
    /// Highest migration recorded in the file; 0 for an empty file.
    pub schema_version: u32,
    /// Highest migration this build knows.
    pub supported_version: u32,
    /// `None` when SQLite's quick check passed, otherwise its first findings.
    pub integrity_problem: Option<String>,
}

impl DatabaseStatus {
    /// A damaged or newer database may be neither served nor written.
    pub fn require_usable(&self) -> Result<()> {
        if let Some(detail) = &self.integrity_problem {
            return Err(StorageError::Corrupt {
                path: self.path.display().to_string(),
                detail: detail.clone(),
            });
        }
        if self.schema_version > self.supported_version {
            return Err(StorageError::SchemaTooNew {
                path: self.path.display().to_string(),
                found: self.schema_version,
                supported: self.supported_version,
            });
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BackupReport {
    pub path: PathBuf,
    pub schema_version: u32,
    pub size_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RestoreReport {
    pub database: PathBuf,
    pub schema_version: u32,
    /// Where the previous database and its sidecar files were moved, if any.
    pub replaced: Option<PathBuf>,
}

/// Report a database's schema version and integrity without migrating or
/// writing it; safe while the runtime runs.
pub fn inspect_database(path: &Path) -> Result<DatabaseStatus> {
    if !path.is_file() {
        return Err(StorageError::NotFound(format!(
            "database file {}",
            path.display()
        )));
    }
    inspect_read_only(path)
}

/// Write a consistent snapshot of `source` to the new file `output` while
/// other connections may keep writing. The snapshot is checked before it is
/// moved into place; a failed backup leaves no file at `output`.
pub fn backup_database(source: &Path, output: &Path) -> Result<BackupReport> {
    if !source.is_file() {
        return Err(StorageError::NotFound(format!(
            "database file {}",
            source.display()
        )));
    }
    if output.exists() {
        return Err(StorageError::AlreadyExists(output.display().to_string()));
    }
    if let Some(parent) = output.parent().filter(|p| !p.as_os_str().is_empty()) {
        if !parent.is_dir() {
            return Err(StorageError::Invalid(format!(
                "backup directory {} does not exist",
                parent.display()
            )));
        }
    }
    let partial = sibling(output, &format!(".partial-{}", std::process::id()))?;
    create_private_file(&partial)?;
    let result = (|| {
        let connection = Connection::open_with_flags(
            source,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        connection.busy_timeout(Duration::from_secs(5))?;
        vacuum_into(&connection, &partial)?;
        drop(connection);
        let status = inspect_read_only(&partial)?;
        if let Some(detail) = status.integrity_problem {
            return Err(StorageError::Corrupt {
                path: source.display().to_string(),
                detail,
            });
        }
        if output.exists() {
            return Err(StorageError::AlreadyExists(output.display().to_string()));
        }
        // A hard link publishes a complete snapshot atomically without replacing
        // a competing backup. Both names are on the output filesystem.
        fs::hard_link(&partial, output).map_err(|error| {
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                StorageError::AlreadyExists(output.display().to_string())
            } else {
                error.into()
            }
        })?;
        let _ = fs::remove_file(&partial);
        sync_directory(output);
        Ok(BackupReport {
            path: output.to_path_buf(),
            schema_version: status.schema_version,
            size_bytes: fs::metadata(output)?.len(),
        })
    })();
    if result.is_err() {
        let _ = fs::remove_file(&partial);
    }
    result
}

/// Install the backup `input` as `database`. The caller must own the data
/// directory (the runtime's owner lock), so no connection is open on it. The
/// input is copied, checked and made self-contained before anything moves;
/// then the current database and any `-wal`, `-shm` or `-journal` beside it
/// are moved, never deleted, to `<directory>/backups/replaced-<timestamp>/`,
/// so a stale log can never be replayed over the restored file. Project record
/// files are preserved there too and regenerated from the restored database.
pub fn restore_database(input: &Path, database: &Path) -> Result<RestoreReport> {
    recover_pending_restore(database)?;
    if !input.is_file() {
        return Err(StorageError::NotFound(format!(
            "backup file {}",
            input.display()
        )));
    }
    for suffix in ["-wal", "-journal"] {
        let log = sidecar(input, suffix);
        if fs::metadata(&log).is_ok_and(|metadata| metadata.len() > 0) {
            return Err(StorageError::Invalid(format!(
                "{} has a {suffix} file beside it, so the file alone may be incomplete; restore a snapshot taken with `luuna-runtime backup`, or stop the runtime that owns it first",
                input.display()
            )));
        }
    }
    let staged = sibling(database, &format!(".restore-{}", std::process::id()))?;
    if let Some(directory) = database.parent().filter(|p| !p.as_os_str().is_empty()) {
        private_directory(directory, true)?;
    }
    for path in std::iter::once(staged.clone()).chain(SIDECARS.map(|s| sidecar(&staged, s))) {
        if path.exists() {
            fs::remove_file(path)?;
        }
    }
    let verified = stage(input, &staged);
    let schema_version = match verified {
        Ok(version) => version,
        Err(error) => {
            for path in std::iter::once(staged.clone()).chain(SIDECARS.map(|s| sidecar(&staged, s)))
            {
                let _ = fs::remove_file(path);
            }
            return Err(error);
        }
    };
    let files: Vec<String> = std::iter::once("")
        .chain(SIDECARS)
        .filter(|suffix| sidecar(database, suffix).exists())
        .map(str::to_owned)
        .collect();
    let directory =
        match backups_directory(database).and_then(|dir| unique_directory(&dir, "replaced")) {
            Ok(directory) => directory,
            Err(error) => {
                let _ = fs::remove_file(&staged);
                return Err(error);
            }
        };
    let intent = RestoreIntent {
        staged: staged.file_name().unwrap().to_string_lossy().into_owned(),
        replaced: directory
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned(),
        files,
        schema_version,
    };
    let marker = sibling(database, ".restore-pending")?;
    let temporary = sibling(database, ".restore-intent-partial")?;
    // The marker is published only after the staged database and intent are
    // durable. No database files move until publication has succeeded.
    if temporary.exists() {
        fs::remove_file(&temporary)?;
    }
    let mut file = create_private_file(&temporary)?;
    file.write_all(&serde_json::to_vec(&intent)?)?;
    file.sync_all()?;
    drop(file);
    fs::hard_link(&temporary, &marker)?;
    sync_directory(&marker);
    fs::remove_file(&temporary)?;
    finish_restore(database, &intent)
}

#[derive(Serialize, Deserialize)]
struct RestoreIntent {
    staged: String,
    replaced: String,
    files: Vec<String>,
    schema_version: u32,
}

/// Complete a previously authorized restore before SQLite can open/create the
/// main file. A process crash at any rename leaves the intent for roll-forward;
/// an unreadable intent fails closed rather than creating an empty database.
pub(crate) fn recover_pending_restore(database: &Path) -> Result<()> {
    let marker = sibling(database, ".restore-pending")?;
    if !marker.exists() {
        return Ok(());
    }
    let intent: RestoreIntent = serde_json::from_slice(&fs::read(&marker)?)?;
    finish_restore(database, &intent)?;
    Ok(())
}

/// Preserve project record files with the replaced database before installing
/// the staged copy; interrupted moves resume without overwriting either side.
fn finish_restore(database: &Path, intent: &RestoreIntent) -> Result<RestoreReport> {
    let safe_name =
        |name: &str| !name.is_empty() && !name.contains(['/', '\\']) && name != "." && name != "..";
    if !safe_name(&intent.staged)
        || !safe_name(&intent.replaced)
        || !intent.replaced.starts_with("replaced-")
        || !intent.staged.starts_with(&format!(
            "{}.restore-",
            database.file_name().unwrap_or_default().to_string_lossy()
        ))
        || intent
            .files
            .iter()
            .any(|suffix| !suffix.is_empty() && !SIDECARS.contains(&suffix.as_str()))
    {
        return Err(StorageError::Invalid(
            "invalid pending restore; preserve the data directory for recovery".into(),
        ));
    }
    let parent = database.parent().unwrap_or(Path::new("."));
    let staged = parent.join(&intent.staged);
    let replaced = parent.join("backups").join(&intent.replaced);
    if staged.exists() {
        inspect_read_only(&staged)?.require_usable()?;
        for suffix in &intent.files {
            let original = sidecar(database, suffix);
            let target = replaced.join(original.file_name().unwrap());
            if target.exists() {
                if original.exists() {
                    return Err(StorageError::Invalid("pending restore found both original and preserved files; refusing to overwrite either".into()));
                }
                continue;
            }
            fs::rename(&original, &target)?;
            sync_directory(&original);
            sync_directory(&target);
        }
        let records = parent.join(crate::project_index::PROJECT_RECORDS);
        let preserved_records = replaced.join(crate::project_index::PROJECT_RECORDS);
        let original_exists = crate::project_files::existing(&records, true)?.is_some();
        let preserved_exists = crate::project_files::existing(&preserved_records, true)?.is_some();
        if original_exists && preserved_exists {
            return Err(StorageError::Invalid("pending restore found both original and preserved project files; refusing to overwrite either".into()));
        }
        if original_exists {
            fs::rename(&records, &preserved_records)?;
            sync_directory(&records);
            sync_directory(&preserved_records);
        }
        // Every old sidecar is now preserved, so only the verified new main
        // file is installed. A crash after this rename is recognized below.
        if database.exists()
            || crate::project_files::existing(&records, true)?.is_some()
            || SIDECARS
                .iter()
                .any(|suffix| sidecar(database, suffix).exists())
        {
            return Err(StorageError::Invalid(
                "unexpected files appeared during restore".into(),
            ));
        }
        fs::rename(&staged, database)?;
        sync_directory(database);
    }
    let status = inspect_read_only(database)?;
    status.require_usable()?;
    if status.schema_version != intent.schema_version {
        return Err(StorageError::Invalid(
            "pending restore schema does not match its intent".into(),
        ));
    }
    fs::remove_file(sibling(database, ".restore-pending")?)?;
    sync_directory(database);
    Ok(RestoreReport {
        database: database.to_path_buf(),
        schema_version: intent.schema_version,
        replaced: (!intent.files.is_empty()).then_some(replaced),
    })
}

/// Copy `input` to `staged`, verify it and convert it to a single file.
fn stage(input: &Path, staged: &Path) -> Result<u32> {
    {
        let mut source = fs::File::open(input)?;
        let mut target = create_private_file(staged)?;
        std::io::copy(&mut source, &mut target)?;
        target.flush()?;
        target.sync_all()?;
    }
    let connection = Connection::open_with_flags(
        staged,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    connection.busy_timeout(Duration::from_secs(5))?;
    if let Some(detail) = quick_check(&connection)? {
        return Err(StorageError::Corrupt {
            path: input.display().to_string(),
            detail,
        });
    }
    let version = applied_versions(&connection)?.last().copied().unwrap_or(0);
    if version == 0 {
        return Err(StorageError::Invalid(format!(
            "{} is not a Luuna database: it records no schema version",
            input.display()
        )));
    }
    if version > DATABASE_SCHEMA_VERSION {
        return Err(StorageError::SchemaTooNew {
            path: input.display().to_string(),
            found: version,
            supported: DATABASE_SCHEMA_VERSION,
        });
    }
    // Rollback-journal mode checkpoints and removes any log of the staged
    // copy; the runtime switches back to WAL when it next opens the file.
    connection.query_row("PRAGMA journal_mode=DELETE", [], |r| r.get::<_, String>(0))?;
    drop(connection);
    fs::OpenOptions::new()
        .write(true)
        .open(staged)?
        .sync_all()?;
    Ok(version)
}

/// Inspect through a read-only connection: a refused database is never
/// written, not even by the checkpoint a closing writer performs.
pub(crate) fn inspect_read_only(path: &Path) -> Result<DatabaseStatus> {
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    connection.busy_timeout(Duration::from_secs(5))?;
    let integrity_problem = quick_check(&connection)?;
    let versions = match integrity_problem {
        None => applied_versions(&connection)?,
        Some(_) => applied_versions(&connection).unwrap_or_default(),
    };
    Ok(DatabaseStatus {
        path: path.to_path_buf(),
        schema_version: versions.last().copied().unwrap_or(0),
        supported_version: DATABASE_SCHEMA_VERSION,
        integrity_problem,
    })
}

/// `None` when `PRAGMA quick_check` reports `ok`; a malformed file or one that
/// is not a database at all is a finding, other failures are errors.
fn quick_check(connection: &Connection) -> Result<Option<String>> {
    let rows = connection
        .prepare("PRAGMA quick_check")
        .and_then(|mut statement| {
            statement
                .query_map([], |r| r.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()
        });
    match rows {
        Ok(rows) if rows.len() == 1 && rows[0] == "ok" => Ok(None),
        Ok(rows) => {
            let lines: Vec<&str> = rows
                .iter()
                .flat_map(|row| row.lines())
                .map(str::trim)
                .filter(|line| !line.is_empty() && !line.starts_with("***"))
                .collect();
            let mut detail = lines.iter().take(3).copied().collect::<Vec<_>>().join("; ");
            if lines.len() > 3 {
                detail.push_str(&format!("; {} more findings", lines.len() - 3));
            }
            Ok(Some(detail))
        }
        Err(rusqlite::Error::SqliteFailure(error, message))
            if matches!(
                error.code,
                ErrorCode::DatabaseCorrupt | ErrorCode::NotADatabase
            ) =>
        {
            Ok(Some(message.unwrap_or_else(|| error.to_string())))
        }
        Err(error) => Err(error.into()),
    }
}

/// Snapshot of an existing database before this build migrates it; the
/// runtime never deletes these, so the developer can always step back.
pub(crate) fn pre_migration_backup(
    connection: &Connection,
    database: &Path,
    version: u32,
) -> Result<PathBuf> {
    let directory = backups_directory(database)?;
    let target = unique_file(
        &directory,
        &format!("pre-migration-v{version}-{}", timestamp()),
    )?;
    if let Err(error) = vacuum_into(connection, &target) {
        let _ = fs::remove_file(&target);
        return Err(error);
    }
    sync_directory(&target);
    Ok(target)
}

/// `VACUUM INTO` needs a missing or empty target; it is created private first.
fn vacuum_into(connection: &Connection, target: &Path) -> Result<()> {
    let name = target
        .to_str()
        .ok_or_else(|| StorageError::Invalid("backup path must be valid UTF-8".into()))?;
    connection.execute("VACUUM INTO ?1", [name])?;
    // SQLite does not sync the generated file itself.
    fs::OpenOptions::new()
        .write(true)
        .open(target)?
        .sync_all()?;
    Ok(())
}

fn backups_directory(database: &Path) -> Result<PathBuf> {
    let directory = database
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
        .join("backups");
    private_directory(&directory, true)?;
    Ok(directory)
}

fn private_directory(directory: &Path, recursive: bool) -> std::io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(recursive);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(directory)
}

fn create_private_file(path: &Path) -> std::io::Result<fs::File> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

fn unique_file(directory: &Path, stem: &str) -> Result<PathBuf> {
    for attempt in 0..100 {
        let name = if attempt == 0 {
            format!("{stem}.sqlite3")
        } else {
            format!("{stem}-{attempt}.sqlite3")
        };
        let path = directory.join(name);
        match create_private_file(&path) {
            Ok(_) => return Ok(path),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        }
    }
    Err(StorageError::AlreadyExists(
        directory.join(stem).display().to_string(),
    ))
}

fn unique_directory(parent: &Path, kind: &str) -> Result<PathBuf> {
    let stem = format!("{kind}-{}", timestamp());
    for attempt in 0..100 {
        let path = if attempt == 0 {
            parent.join(&stem)
        } else {
            parent.join(format!("{stem}-{attempt}"))
        };
        match private_directory(&path, false) {
            Ok(()) => return Ok(path),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        }
    }
    Err(StorageError::AlreadyExists(
        parent.join(stem).display().to_string(),
    ))
}

/// A sortable UTC timestamp that is also a valid file name on every platform.
fn timestamp() -> String {
    chrono::Utc::now().format("%Y%m%dT%H%M%S%3fZ").to_string()
}

fn sidecar(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

fn sibling(path: &Path, suffix: &str) -> Result<PathBuf> {
    if path.file_name().is_none() {
        return Err(StorageError::Invalid(format!(
            "{} does not name a file",
            path.display()
        )));
    }
    Ok(sidecar(path, suffix))
}

/// Persist a rename or a new file name; best effort where unsupported.
fn sync_directory(path: &Path) {
    #[cfg(unix)]
    if let Some(directory) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        if let Ok(handle) = fs::File::open(directory) {
            let _ = handle.sync_all();
        }
    }
    #[cfg(not(unix))]
    let _ = path;
}

#[cfg(test)]
mod recovery_tests {
    use super::*;
    use crate::Store;

    #[test]
    fn restore_resumes_after_every_file_move_including_installation() {
        for interrupted_after in 0..=5 {
            let dir = tempfile::tempdir().unwrap();
            let database = dir.path().join("luuna.sqlite3");
            let input = dir.path().join("snapshot.sqlite3");
            {
                let mut store = Store::open(&input).unwrap();
                store
                    .create_run("preserved", "project", serde_json::json!({}))
                    .unwrap();
            }
            let staged = sibling(&database, ".restore-test").unwrap();
            let schema_version = stage(&input, &staged).unwrap();
            let replaced =
                unique_directory(&backups_directory(&database).unwrap(), "replaced").unwrap();
            let files: Vec<String> = std::iter::once("")
                .chain(SIDECARS)
                .map(str::to_owned)
                .collect();
            for suffix in &files {
                fs::write(sidecar(&database, suffix), format!("old{suffix}")).unwrap();
            }
            let intent = RestoreIntent {
                staged: staged.file_name().unwrap().to_string_lossy().into_owned(),
                replaced: replaced.file_name().unwrap().to_string_lossy().into_owned(),
                files: files.clone(),
                schema_version,
            };
            fs::write(
                sibling(&database, ".restore-pending").unwrap(),
                serde_json::to_vec(&intent).unwrap(),
            )
            .unwrap();
            for suffix in files.iter().take(interrupted_after.min(4)) {
                let original = sidecar(&database, suffix);
                fs::rename(&original, replaced.join(original.file_name().unwrap())).unwrap();
            }
            if interrupted_after == 5 {
                fs::rename(&staged, &database).unwrap();
            }
            let store = Store::open(&database).unwrap();
            assert!(store.get_run("preserved").unwrap().is_some());
            for suffix in &files {
                assert_eq!(
                    fs::read(replaced.join(sidecar(&database, suffix).file_name().unwrap()))
                        .unwrap(),
                    format!("old{suffix}").as_bytes()
                );
            }
            assert!(!sibling(&database, ".restore-pending").unwrap().exists());
            drop(store);
            assert!(Store::open(&database)
                .unwrap()
                .get_run("preserved")
                .unwrap()
                .is_some());
        }
    }

    #[test]
    fn competing_backups_never_replace_the_winning_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.sqlite3");
        drop(Store::open(&source).unwrap());
        let output = dir.path().join("backup.sqlite3");
        let barrier = std::sync::Barrier::new(2);
        let successes = std::thread::scope(|scope| {
            let a = scope.spawn(|| {
                barrier.wait();
                backup_database(&source, &output).is_ok()
            });
            let b = scope.spawn(|| {
                barrier.wait();
                backup_database(&source, &output).is_ok()
            });
            usize::from(a.join().unwrap()) + usize::from(b.join().unwrap())
        });
        assert_eq!(successes, 1);
        let bytes = fs::read(&output).unwrap();
        assert!(backup_database(&source, &output).is_err());
        assert_eq!(fs::read(&output).unwrap(), bytes);
        inspect_database(&output).unwrap().require_usable().unwrap();
    }
}
