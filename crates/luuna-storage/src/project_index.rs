//! The project file `<data_dir>/projects/<id>/project.json` and the database as
//! a rebuildable index of it.
//!
//! The file holds a project's registration, its repositories, every confirmed
//! manifest revision and its memory retention setting. The four writers of
//! those tables write it before they commit; [`reconcile_project_files`] runs
//! on every `Store::open` of a file and brings the database in line with the
//! files, or refuses with nothing changed. Nothing here follows a link, and
//! writes are private and atomic, as in `project_files.rs`.
use crate::project_files::{existing, is_link};
use crate::{Result, StorageError};
use luuna_domain::valid_project_id;
use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
};

const PROJECT_FILE: &str = "project.json";
const TEMP_FILE: &str = "project.json.tmp";
const SCHEMA_VERSION: u64 = 1;
/// Largest project file accepted or written.
const MAX_PROJECT_FILE_BYTES: u64 = 8 * 1024 * 1024;
const TOP_LEVEL_KEYS: [&str; 5] = [
    "schema_version",
    "project",
    "repositories",
    "manifests",
    "memory_retention",
];
const PROJECT_KEYS: [&str; 6] = [
    "id",
    "name",
    "revision",
    "manifest_hash",
    "created_at",
    "updated_at",
];

/// The file form of one project. Keys and order are fixed; the JSON-text
/// columns are embedded as JSON values.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ProjectDocument {
    schema_version: u64,
    project: DocumentProject,
    /// Sorted by id.
    repositories: Vec<DocumentRepository>,
    /// Sorted by revision.
    manifests: Vec<DocumentManifest>,
    memory_retention: Option<DocumentRetention>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DocumentProject {
    id: String,
    name: String,
    revision: u64,
    manifest_hash: Option<String>,
    created_at: String,
    updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DocumentRepository {
    id: String,
    local_path: String,
    observation: Value,
    registered_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DocumentManifest {
    revision: u64,
    hash: String,
    manifest: Value,
    base_commits: Value,
    confirmed_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DocumentRetention {
    policy: Value,
    updated_at: String,
}

fn project_folder(data_dir: &Path, project_id: &str) -> PathBuf {
    data_dir.join("projects").join(project_id)
}

fn refused(path: &Path, detail: impl Into<String>) -> StorageError {
    StorageError::ProjectFile {
        path: path.display().to_string(),
        detail: detail.into(),
    }
}

/// `existing`, with a refusal described in plain words for the error detail.
fn check(path: &Path, directory: bool) -> std::result::Result<Option<fs::Metadata>, String> {
    existing(path, directory).map_err(|_| match fs::symlink_metadata(path) {
        Ok(metadata) if is_link(&metadata) => format!("{} is a link", describe(path)),
        Ok(_) if directory => format!("{} is not a folder", describe(path)),
        Ok(_) => format!("{} is not a regular file", describe(path)),
        Err(error) => format!(
            "{} cannot be inspected ({:?})",
            describe(path),
            error.kind()
        ),
    })
}

fn describe(path: &Path) -> String {
    match path.file_name().and_then(|name| name.to_str()) {
        Some(PROJECT_FILE) => "it".into(),
        Some(TEMP_FILE) => "the temporary file beside it".into(),
        _ => "a folder on its path".into(),
    }
}

fn parse_error(error: &serde_json::Error) -> String {
    format!("line {} column {}", error.line(), error.column())
}

/// Check the parsed document for `folder_id`. Details never quote content.
fn validate(document: &ProjectDocument, folder_id: &str) -> std::result::Result<(), String> {
    if document.schema_version != SCHEMA_VERSION {
        return Err("it has an unsupported schema_version".into());
    }
    if !valid_project_id(folder_id) {
        return Err("its folder name is not a valid project id".into());
    }
    if document.project.id != folder_id {
        return Err("it belongs to another project".into());
    }
    if i64::try_from(document.project.revision).is_err() {
        return Err("its project revision is out of range".into());
    }
    let mut repositories = BTreeSet::new();
    for repository in &document.repositories {
        if !valid_project_id(&repository.id) {
            return Err("it names an invalid repository id".into());
        }
        if !repositories.insert(repository.id.as_str()) {
            return Err("it lists a repository twice".into());
        }
    }
    let mut revisions = BTreeSet::new();
    for manifest in &document.manifests {
        if manifest.revision == 0 || i64::try_from(manifest.revision).is_err() {
            return Err("it holds an invalid manifest revision".into());
        }
        if !revisions.insert(manifest.revision) {
            return Err("it lists a manifest revision twice".into());
        }
    }
    Ok(())
}

/// Parse and check the bytes of `folder_id`'s project file.
fn parse(bytes: &[u8], folder_id: &str) -> std::result::Result<ProjectDocument, String> {
    if bytes.len() as u64 > MAX_PROJECT_FILE_BYTES {
        return Err("it is larger than 8 MiB".into());
    }
    let value: Value = serde_json::from_slice(bytes)
        .map_err(|error| format!("not JSON, at {}", parse_error(&error)))?;
    let Some(object) = value.as_object() else {
        return Err("it is not a JSON object".into());
    };
    match object.get("schema_version").and_then(Value::as_u64) {
        Some(SCHEMA_VERSION) => {}
        Some(version) if version > SCHEMA_VERSION => {
            return Err("written by a newer Luuna; run that Luuna build".into())
        }
        _ => return Err("it has no valid schema_version".into()),
    }
    let complete = TOP_LEVEL_KEYS.iter().all(|key| object.contains_key(*key))
        && object
            .get("project")
            .and_then(Value::as_object)
            .is_some_and(|project| PROJECT_KEYS.iter().all(|key| project.contains_key(*key)));
    // Unknown, missing or wrongly typed fields; serde's message names the
    // field, which is file content, so only the position is reported.
    let document: ProjectDocument = serde_json::from_slice(bytes).map_err(|error| {
        format!(
            "it does not have the project file format (an unknown, missing or wrongly typed field at {})",
            parse_error(&error)
        )
    })?;
    if !complete {
        return Err(
            "it does not have the project file format (a required field is missing)".into(),
        );
    }
    validate(&document, folder_id)?;
    Ok(document)
}

/// The bytes written for `document`, after checking that the reader accepts them.
fn render(document: &ProjectDocument, folder_id: &str) -> std::result::Result<Vec<u8>, String> {
    let mut bytes = serde_json::to_vec_pretty(document)
        .map_err(|_| "it cannot be encoded as JSON".to_string())?;
    bytes.push(b'\n');
    parse(&bytes, folder_id)?;
    Ok(bytes)
}

/// Read `<folder>/project.json`: `Ok(None)` when it is absent. Links, wrong
/// file types, files over 8 MiB, newer schema versions, unknown keys, another
/// project's id and invalid ids are refused.
fn read_project_file(folder: &Path, folder_id: &str) -> Result<Option<ProjectDocument>> {
    let path = folder.join(PROJECT_FILE);
    if check(&path, false)
        .map_err(|detail| refused(&path, detail))?
        .is_none()
    {
        return Ok(None);
    }
    let file = match fs::File::open(&path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(refused(
                &path,
                format!("it cannot be opened ({:?})", error.kind()),
            ))
        }
    };
    let mut bytes = Vec::new();
    file.take(MAX_PROJECT_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| refused(&path, format!("it cannot be read ({:?})", error.kind())))?;
    parse(&bytes, folder_id)
        .map(Some)
        .map_err(|detail| refused(&path, detail))
}

fn json_column(text: &str, what: impl FnOnce() -> String) -> Result<Value> {
    serde_json::from_str(text)
        .map_err(|_| StorageError::Invalid(format!("the database's {} is not JSON", what())))
}

/// The file view of one project from the rows `connection` sees, which may be
/// an open transaction. `None` when the project has no row.
pub(crate) fn project_document(
    connection: &Connection,
    project_id: &str,
) -> Result<Option<ProjectDocument>> {
    let Some(project) = connection
        .query_row(
            "SELECT id,name,revision,manifest_hash,created_at,updated_at FROM projects WHERE id=?1",
            [project_id],
            |r| {
                Ok(DocumentProject {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    revision: r.get(2)?,
                    manifest_hash: r.get(3)?,
                    created_at: r.get(4)?,
                    updated_at: r.get(5)?,
                })
            },
        )
        .optional()?
    else {
        return Ok(None);
    };
    let mut statement = connection.prepare(
        "SELECT id,local_path,observation,registered_at FROM repositories WHERE project_id=?1 ORDER BY id",
    )?;
    let rows = statement
        .query_map([project_id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
            ))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let repositories = rows
        .into_iter()
        .map(|(id, local_path, observation, registered_at)| {
            let observation = json_column(&observation, || {
                format!("observation of repository {id} of project {project_id}")
            })?;
            Ok(DocumentRepository {
                id,
                local_path,
                observation,
                registered_at,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let mut statement = connection.prepare(
        "SELECT revision,hash,payload,base_commits,confirmed_at FROM manifest_revisions WHERE project_id=?1 ORDER BY revision",
    )?;
    let rows = statement
        .query_map([project_id], |r| {
            Ok((
                r.get::<_, u64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
            ))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let manifests = rows
        .into_iter()
        .map(|(revision, hash, payload, base_commits, confirmed_at)| {
            Ok(DocumentManifest {
                revision,
                hash,
                manifest: json_column(&payload, || {
                    format!("manifest revision {revision} of project {project_id}")
                })?,
                base_commits: json_column(&base_commits, || {
                    format!("base commits of manifest revision {revision} of project {project_id}")
                })?,
                confirmed_at,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let memory_retention = connection
        .query_row(
            "SELECT policy,updated_at FROM memory_retention WHERE project_id=?1",
            [project_id],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
        )
        .optional()?
        .map(|(policy, updated_at)| {
            Ok::<_, StorageError>(DocumentRetention {
                policy: json_column(&policy, || {
                    format!("memory retention policy of project {project_id}")
                })?,
                updated_at,
            })
        })
        .transpose()?;
    Ok(Some(ProjectDocument {
        schema_version: SCHEMA_VERSION,
        project,
        repositories,
        manifests,
        memory_retention,
    }))
}

/// Every check a write of `project_id`'s file makes before it creates or
/// removes anything: no link or wrong file type on the way. Returns whether a
/// stale temporary file is to be removed.
fn writable(data_dir: &Path, project_id: &str) -> std::result::Result<bool, String> {
    let folder = project_folder(data_dir, project_id);
    check(&data_dir.join("projects"), true)?;
    check(&folder, true)?;
    check(&folder.join(PROJECT_FILE), false)?;
    Ok(check(&folder.join(TEMP_FILE), false)?.is_some())
}

/// Replace `project_id`'s file with `bytes`: folder created 0700, a 0600
/// temporary file created new, written, synced and renamed over the file,
/// then the folder synced. A link or wrong file type in the way refuses the
/// write with nothing changed.
fn write_bytes(data_dir: &Path, project_id: &str, bytes: &[u8]) -> Result<()> {
    if !valid_project_id(project_id) {
        return Err(StorageError::Invalid("project id is not valid".into()));
    }
    let folder = project_folder(data_dir, project_id);
    let path = folder.join(PROJECT_FILE);
    let temporary = folder.join(TEMP_FILE);
    // Every check happens before anything is created or removed.
    let stale = writable(data_dir, project_id).map_err(|detail| {
        StorageError::Invalid(format!(
            "project file {} cannot be written: {detail}",
            path.display()
        ))
    })?;
    let located = |error: std::io::Error| {
        StorageError::Io(std::io::Error::new(
            error.kind(),
            format!("project file {}: {error}", path.display()),
        ))
    };
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(&folder).map_err(located)?;
    if stale {
        fs::remove_file(&temporary).map_err(located)?;
    }
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let result = (|| -> std::io::Result<()> {
        let mut handle = options.open(&temporary)?;
        handle.write_all(bytes)?;
        handle.sync_all()?;
        drop(handle);
        fs::rename(&temporary, &path)
    })();
    if let Err(error) = result {
        let _ = fs::remove_file(&temporary);
        return Err(located(error));
    }
    #[cfg(unix)]
    fs::File::open(&folder)
        .and_then(|handle| handle.sync_all())
        .map_err(located)?;
    Ok(())
}

/// What `project.json` held before a write, so that a failed commit can put
/// it back (P47).
#[allow(dead_code)] // Used by the four writers of the covered tables.
pub(crate) struct PreviousProjectFile {
    data_dir: PathBuf,
    project_id: String,
    bytes: Option<Vec<u8>>,
}

#[allow(dead_code)] // Used by the four writers of the covered tables.
impl PreviousProjectFile {
    /// Put the previous bytes back, or remove the file if there was none.
    pub(crate) fn restore(self) -> Result<()> {
        match &self.bytes {
            Some(bytes) => write_bytes(&self.data_dir, &self.project_id, bytes),
            None => {
                let folder = project_folder(&self.data_dir, &self.project_id);
                let path = folder.join(PROJECT_FILE);
                if check(&path, false)
                    .map_err(|detail| {
                        StorageError::Invalid(format!(
                            "project file {} cannot be removed: {detail}",
                            path.display()
                        ))
                    })?
                    .is_some()
                {
                    fs::remove_file(&path)?;
                    #[cfg(unix)]
                    fs::File::open(&folder)?.sync_all()?;
                }
                Ok(())
            }
        }
    }
}

/// Write `project_id`'s file from the rows `connection` sees; call it inside
/// the writer's transaction, after its SQL and immediately before commit. A
/// store without a data folder (`:memory:`) writes nothing and returns
/// `None`. On error nothing was changed and the caller drops (rolls back) its
/// transaction; on success the caller commits with [`commit_or_restore`].
#[allow(dead_code)] // Used by the four writers of the covered tables.
pub(crate) fn write_project_file(
    connection: &Connection,
    data_dir: Option<&Path>,
    project_id: &str,
) -> Result<Option<PreviousProjectFile>> {
    let Some(data_dir) = data_dir else {
        return Ok(None);
    };
    let document = project_document(connection, project_id)?
        .ok_or_else(|| StorageError::NotFound(project_id.into()))?;
    let bytes = render(&document, project_id).map_err(|detail| {
        StorageError::Invalid(format!(
            "project file for {project_id} cannot be written: {detail}"
        ))
    })?;
    let path = project_folder(data_dir, project_id).join(PROJECT_FILE);
    writable(data_dir, project_id).map_err(|detail| {
        StorageError::Invalid(format!(
            "project file {} cannot be written: {detail}",
            path.display()
        ))
    })?;
    let previous = match fs::File::open(&path) {
        Ok(file) => {
            let mut previous = Vec::new();
            file.take(MAX_PROJECT_FILE_BYTES + 1)
                .read_to_end(&mut previous)?;
            if previous.len() as u64 > MAX_PROJECT_FILE_BYTES {
                return Err(StorageError::Invalid(format!(
                    "project file {} is larger than 8 MiB and is not replaced",
                    path.display()
                )));
            }
            Some(previous)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };
    write_bytes(data_dir, project_id, &bytes)?;
    Ok(Some(PreviousProjectFile {
        data_dir: data_dir.to_path_buf(),
        project_id: project_id.into(),
        bytes: previous,
    }))
}

/// Commit `tx`; if the commit fails, put the previous project file back. If
/// that fails too, the error names both failures.
#[allow(dead_code)] // Used by the four writers of the covered tables.
pub(crate) fn commit_or_restore(
    tx: Transaction<'_>,
    previous: Option<PreviousProjectFile>,
) -> Result<()> {
    let Err(commit) = tx.commit() else {
        return Ok(());
    };
    if let Some(previous) = previous {
        if let Err(restore) = previous.restore() {
            return Err(StorageError::Io(std::io::Error::other(format!(
                "the database commit failed ({commit}) and the project file could not be put back ({restore})"
            ))));
        }
    }
    Err(commit.into())
}

/// What phase 2 decided for one project.
enum Action {
    /// (a) file only: insert every row.
    Index(Box<ProjectDocument>),
    /// (b) database only: write the file after the commit.
    Backfill(Vec<u8>),
    /// (d) the database lacks only what the file has: bring it forward.
    BringForward {
        file: Box<ProjectDocument>,
        database: Box<ProjectDocument>,
    },
}

/// Phase 1: read and validate every `<data_dir>/projects/<id>/project.json`.
/// Folders without one (a store.json-only folder) and plain files are left
/// alone; a link where a project folder would be is refused.
fn read_project_files(data_dir: &Path) -> Result<BTreeMap<String, (PathBuf, ProjectDocument)>> {
    let projects = data_dir.join("projects");
    let mut files = BTreeMap::new();
    if check(&projects, true)
        .map_err(|detail| refused(&projects, detail))?
        .is_none()
    {
        return Ok(files);
    }
    let entries = fs::read_dir(&projects).map_err(|error| {
        refused(
            &projects,
            format!("it cannot be listed ({:?})", error.kind()),
        )
    })?;
    for entry in entries {
        let entry = entry.map_err(|error| {
            refused(
                &projects,
                format!("it cannot be listed ({:?})", error.kind()),
            )
        })?;
        let folder = entry.path();
        let metadata = fs::symlink_metadata(&folder).map_err(|error| {
            refused(
                &folder,
                format!("it cannot be inspected ({:?})", error.kind()),
            )
        })?;
        if is_link(&metadata) {
            return Err(refused(
                &folder.join(PROJECT_FILE),
                "a folder on its path is a link",
            ));
        }
        if !metadata.is_dir() {
            continue;
        }
        let name = entry.file_name();
        let folder_id = name.to_str().unwrap_or_default();
        if let Some(document) = read_project_file(&folder, folder_id)? {
            files.insert(folder_id.to_owned(), (folder.join(PROJECT_FILE), document));
        }
    }
    Ok(files)
}

/// Phase 2 for a project present in both: `Ok(None)` when equal, the
/// bring-forward action when every database row is in the file, otherwise
/// the refusal detail.
fn compare(
    file: &ProjectDocument,
    database: &ProjectDocument,
) -> std::result::Result<Option<Action>, String> {
    if file == database {
        return Ok(None);
    }
    for held in &database.manifests {
        match file.manifests.iter().find(|m| m.revision == held.revision) {
            None => {
                return Err(format!(
                    "the database holds manifest revision {}, which this file lacks",
                    held.revision
                ))
            }
            Some(other) if other != held => {
                return Err(format!(
                    "manifest revision {} differs between this file and the database",
                    held.revision
                ))
            }
            Some(_) => {}
        }
    }
    let ids = |document: &ProjectDocument| -> BTreeSet<String> {
        document.repositories.iter().map(|r| r.id.clone()).collect()
    };
    if ids(file) != ids(database) {
        return Err("the database registers a different set of repositories".into());
    }
    if file.project.created_at != database.project.created_at {
        return Err("the database records a different creation time for the project".into());
    }
    if database.memory_retention.is_some() && file.memory_retention.is_none() {
        return Err("the database holds a memory retention setting, which this file lacks".into());
    }
    Ok(Some(Action::BringForward {
        file: Box::new(file.clone()),
        database: Box::new(database.clone()),
    }))
}

/// Phase 2 for every project in files or database, read inside `tx`. Every
/// refusal happens here, before anything is written.
fn plan_actions(
    tx: &Transaction<'_>,
    data_dir: &Path,
    files: &BTreeMap<String, (PathBuf, ProjectDocument)>,
) -> Result<Vec<(String, Action)>> {
    let database_ids: Vec<String> = tx
        .prepare("SELECT id FROM projects ORDER BY id")?
        .query_map([], |r| r.get(0))?
        .collect::<std::result::Result<_, _>>()?;
    let mut ids: BTreeSet<&str> = files.keys().map(String::as_str).collect();
    ids.extend(database_ids.iter().map(String::as_str));

    let mut actions = Vec::new();
    // Each checkout path, with the projects that register it once reconcile
    // is done and whether that registration comes from a file being applied.
    let mut owners: BTreeMap<String, Vec<(String, bool)>> = BTreeMap::new();
    for id in ids {
        let path = project_folder(data_dir, id).join(PROJECT_FILE);
        let database = project_document(tx, id).map_err(|error| match error {
            StorageError::Invalid(detail) => refused(&path, detail),
            other => other,
        })?;
        let (action, repositories) = match (files.get(id), database) {
            (Some((_, file)), None) => (
                Some(Action::Index(Box::new(file.clone()))),
                file.repositories.clone(),
            ),
            (None, Some(database)) => {
                let bytes = render(&database, id).map_err(|detail| {
                    refused(
                        &path,
                        format!("the database's record cannot be written to it: {detail}"),
                    )
                })?;
                writable(data_dir, id).map_err(|detail| refused(&path, detail))?;
                (Some(Action::Backfill(bytes)), database.repositories)
            }
            (Some((file_path, file)), Some(database)) => (
                compare(file, &database).map_err(|detail| refused(file_path, detail))?,
                file.repositories.clone(),
            ),
            (None, None) => continue,
        };
        let applied = matches!(action, Some(Action::Index(_) | Action::BringForward { .. }));
        for repository in repositories {
            owners
                .entry(repository.local_path)
                .or_default()
                .push((id.to_owned(), applied));
        }
        if let Some(action) = action {
            actions.push((id.to_owned(), action));
        }
    }
    // A checkout that an applied file registers may not belong to another
    // project. Registrations already in the database are not judged here.
    for registered in owners.values() {
        let projects: BTreeSet<&str> = registered.iter().map(|(id, _)| id.as_str()).collect();
        if projects.len() < 2 {
            continue;
        }
        if let Some((id, _)) = registered.iter().find(|(_, applied)| *applied) {
            return Err(refused(
                &files[id].0,
                "it registers a checkout that belongs to another project",
            ));
        }
    }
    Ok(actions)
}

fn insert_repository(tx: &Transaction<'_>, project_id: &str, r: &DocumentRepository) -> Result<()> {
    tx.execute(
        "INSERT INTO repositories(project_id,id,local_path,observation,registered_at) VALUES (?1,?2,?3,?4,?5)",
        params![
            project_id,
            r.id,
            r.local_path,
            serde_json::to_string(&r.observation)?,
            r.registered_at
        ],
    )?;
    Ok(())
}

fn insert_manifest(tx: &Transaction<'_>, project_id: &str, m: &DocumentManifest) -> Result<()> {
    tx.execute(
        "INSERT INTO manifest_revisions(project_id,revision,hash,payload,base_commits,confirmed_at) VALUES (?1,?2,?3,?4,?5,?6)",
        params![
            project_id,
            m.revision,
            m.hash,
            serde_json::to_string(&m.manifest)?,
            serde_json::to_string(&m.base_commits)?,
            m.confirmed_at
        ],
    )?;
    Ok(())
}

fn put_retention(tx: &Transaction<'_>, project_id: &str, r: &DocumentRetention) -> Result<()> {
    tx.execute(
        "INSERT INTO memory_retention(project_id,policy,updated_at) VALUES (?1,?2,?3)
         ON CONFLICT(project_id) DO UPDATE SET policy=excluded.policy,updated_at=excluded.updated_at",
        params![project_id, serde_json::to_string(&r.policy)?, r.updated_at],
    )?;
    Ok(())
}

/// Phase 3 for one project. Projects go in before their dependent rows
/// (foreign keys are on); manifest revisions are only ever inserted.
fn apply(tx: &Transaction<'_>, project_id: &str, action: &Action) -> Result<()> {
    match action {
        Action::Index(file) => {
            let p = &file.project;
            tx.execute(
                "INSERT INTO projects(id,name,revision,manifest_hash,created_at,updated_at) VALUES (?1,?2,?3,?4,?5,?6)",
                params![p.id, p.name, p.revision, p.manifest_hash, p.created_at, p.updated_at],
            )?;
            for repository in &file.repositories {
                insert_repository(tx, project_id, repository)?;
            }
            for manifest in &file.manifests {
                insert_manifest(tx, project_id, manifest)?;
            }
            if let Some(retention) = &file.memory_retention {
                put_retention(tx, project_id, retention)?;
            }
        }
        Action::BringForward { file, database } => {
            for manifest in &file.manifests {
                if !database
                    .manifests
                    .iter()
                    .any(|held| held.revision == manifest.revision)
                {
                    insert_manifest(tx, project_id, manifest)?;
                }
            }
            if file.project != database.project {
                let p = &file.project;
                tx.execute(
                    "UPDATE projects SET name=?1,revision=?2,manifest_hash=?3,updated_at=?4 WHERE id=?5",
                    params![p.name, p.revision, p.manifest_hash, p.updated_at, project_id],
                )?;
            }
            for repository in &file.repositories {
                let held = database.repositories.iter().find(|r| r.id == repository.id);
                if held != Some(repository) {
                    tx.execute(
                        "UPDATE repositories SET local_path=?1,observation=?2,registered_at=?3 WHERE project_id=?4 AND id=?5",
                        params![
                            repository.local_path,
                            serde_json::to_string(&repository.observation)?,
                            repository.registered_at,
                            project_id,
                            repository.id
                        ],
                    )?;
                }
            }
            if let Some(retention) = &file.memory_retention {
                if database.memory_retention.as_ref() != Some(retention) {
                    put_retention(tx, project_id, retention)?;
                }
            }
        }
        Action::Backfill(_) => {}
    }
    Ok(())
}

/// Reconcile the database with the project files under `data_dir` (P49).
/// Phase 1 reads and validates every file; phase 2 decides, per project, to
/// index a file, backfill a file from the database, bring the database
/// forward, or refuse; phase 3 applies every database change in one
/// IMMEDIATE transaction; phase 4 writes the backfilled files. Every refusal
/// happens in phases 1-2, before anything is written.
pub(crate) fn reconcile_project_files(connection: &mut Connection, data_dir: &Path) -> Result<()> {
    let files = read_project_files(data_dir)?;
    let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let actions = plan_actions(&tx, data_dir, &files)?;
    for (id, action) in &actions {
        apply(&tx, id, action)?;
    }
    tx.commit()?;
    for (id, action) in &actions {
        if let Action::Backfill(bytes) = action {
            write_bytes(data_dir, id, bytes)?;
        }
    }
    Ok(())
}

/// A sortable UTC timestamp that is also a valid file name on every platform
/// (the form `backups/` already uses).
fn timestamp() -> String {
    chrono::Utc::now().format("%Y%m%dT%H%M%S%3fZ").to_string()
}

fn sync_folder(folder: &Path) {
    #[cfg(unix)]
    if let Ok(handle) = fs::File::open(folder) {
        let _ = handle.sync_all();
    }
    #[cfg(not(unix))]
    let _ = folder;
}

/// With the main database file absent, move a surviving `-wal` or `-shm`
/// beside it into `<directory>/backups/orphaned-sidecars-<timestamp>/`
/// (0700), so SQLite can never replay it into a new file; the bytes are kept.
pub(crate) fn move_orphaned_sidecars(database: &Path) -> Result<()> {
    match fs::symlink_metadata(database) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
        Ok(_) => return Ok(()),
    }
    let sidecar = |suffix: &str| {
        let mut name = database.as_os_str().to_owned();
        name.push(suffix);
        PathBuf::from(name)
    };
    let mut present = Vec::new();
    for suffix in ["-wal", "-shm"] {
        let path = sidecar(suffix);
        match fs::symlink_metadata(&path) {
            Ok(_) => present.push(path),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    if present.is_empty() {
        return Ok(());
    }
    let directory = database
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let backups = directory.join("backups");
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(&backups)?;
    builder.recursive(false);
    let stem = format!("orphaned-sidecars-{}", timestamp());
    let mut target = None;
    for attempt in 0..100 {
        let candidate = if attempt == 0 {
            backups.join(&stem)
        } else {
            backups.join(format!("{stem}-{attempt}"))
        };
        match builder.create(&candidate) {
            Ok(()) => {
                target = Some(candidate);
                break;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        }
    }
    let target = target
        .ok_or_else(|| StorageError::AlreadyExists(backups.join(&stem).display().to_string()))?;
    sync_folder(&backups);
    for path in present {
        let name = path.file_name().expect("a sidecar names a file");
        fs::rename(&path, target.join(name))?;
    }
    sync_folder(&target);
    sync_folder(directory);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Store;

    fn registered(data: &Path) -> Store {
        let store = Store::open(data.join("luuna.sqlite3")).unwrap();
        store
            .connection
            .execute_batch(
                "INSERT INTO projects(id,name,revision,manifest_hash,created_at,updated_at)
                 VALUES ('alpha','Alpha',0,NULL,'2026-10-17T00:00:00.000Z','2026-10-17T00:00:00.000Z');",
            )
            .unwrap();
        store
    }

    #[test]
    fn a_failed_commit_puts_the_previous_file_back() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path();
        let store = registered(data);
        let path = project_folder(data, "alpha").join(PROJECT_FILE);

        // Absent before: restore removes the written file.
        let previous = write_project_file(&store.connection, Some(data), "alpha")
            .unwrap()
            .unwrap();
        assert!(path.is_file());
        previous.restore().unwrap();
        assert!(!path.exists());

        // Present before: restore puts the exact bytes back.
        fs::write(&path, b"earlier bytes").unwrap();
        let previous = write_project_file(&store.connection, Some(data), "alpha")
            .unwrap()
            .unwrap();
        assert_ne!(fs::read(&path).unwrap(), b"earlier bytes");
        previous.restore().unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"earlier bytes");
        assert!(!path.with_file_name(TEMP_FILE).exists());

        // A memory store writes nothing.
        assert!(write_project_file(&store.connection, None, "alpha")
            .unwrap()
            .is_none());
    }

    #[cfg(unix)]
    #[test]
    fn a_link_in_the_way_refuses_the_write_with_nothing_changed() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        fs::create_dir_all(&data).unwrap();
        let store = registered(&data);
        let elsewhere = dir.path().join("elsewhere");
        fs::create_dir_all(&elsewhere).unwrap();
        fs::create_dir_all(data.join("projects")).unwrap();
        std::os::unix::fs::symlink(&elsewhere, project_folder(&data, "alpha")).unwrap();
        assert!(write_project_file(&store.connection, Some(&data), "alpha").is_err());
        assert_eq!(fs::read_dir(&elsewhere).unwrap().count(), 0);
    }

    fn database_view(data: &Path) -> ProjectDocument {
        let store = Store::open(data.join("luuna.sqlite3")).unwrap();
        project_document(&store.connection, "alpha")
            .unwrap()
            .unwrap()
    }

    fn remove(data: &Path, suffixes: &[&str]) {
        for suffix in suffixes {
            let _ = fs::remove_file(data.join(format!("luuna.sqlite3{suffix}")));
        }
    }

    /// Without the writers, rows inserted directly are backfilled (b), an
    /// older database is brought forward (d), and a deleted one is rebuilt
    /// (a), with stale sidecars moved aside (P52).
    #[test]
    fn reconcile_backfills_brings_forward_and_rebuilds() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        fs::create_dir_all(&data).unwrap();
        let file = project_folder(&data, "alpha").join(PROJECT_FILE);
        let older = dir.path().join("older.sqlite3");
        {
            let store = registered(&data);
            store
                .connection
                .execute_batch(
                    r#"INSERT INTO repositories(project_id,id,local_path,observation,registered_at)
                     VALUES ('alpha','app','/srv/alpha/app','{"b":1,"a":[2,3]}','2026-10-17T00:00:00.000Z');"#,
                )
                .unwrap();
        }
        remove(&data, &["-wal", "-shm"]);
        fs::copy(data.join("luuna.sqlite3"), &older).unwrap();
        {
            let store = Store::open(data.join("luuna.sqlite3")).unwrap();
            let written: ProjectDocument =
                serde_json::from_slice(&fs::read(&file).unwrap()).unwrap();
            assert_eq!(
                Some(written),
                project_document(&store.connection, "alpha").unwrap()
            );
            store
                .connection
                .execute_batch(
                    r#"INSERT INTO manifest_revisions(project_id,revision,hash,payload,base_commits,confirmed_at)
                     VALUES ('alpha',1,'h1','{"id":"alpha"}','{"app":"c"}','2026-10-18T00:00:00.000Z');
                     UPDATE projects SET revision=1,manifest_hash='h1',updated_at='2026-10-18T00:00:00.000Z';
                     UPDATE repositories SET local_path='/srv/moved/app';
                     INSERT INTO memory_retention(project_id,policy,updated_at)
                     VALUES ('alpha','{"process_max_age_days":3}','2026-10-18T00:00:00.000Z');"#,
                )
                .unwrap();
        }
        fs::remove_file(&file).unwrap();
        let expected = database_view(&data);
        let file_bytes = fs::read(&file).unwrap();

        remove(&data, &["-wal", "-shm"]);
        fs::copy(&older, data.join("luuna.sqlite3")).unwrap();
        assert_eq!(database_view(&data), expected, "brought forward");
        assert_eq!(fs::read(&file).unwrap(), file_bytes);

        remove(&data, &["", "-wal", "-shm"]);
        fs::write(data.join("luuna.sqlite3-wal"), b"stale").unwrap();
        assert_eq!(database_view(&data), expected, "rebuilt");
        let moved: Vec<_> = fs::read_dir(data.join("backups"))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| {
                path.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("orphaned-sidecars-")
            })
            .collect();
        assert_eq!(moved.len(), 1);
        assert_eq!(
            fs::read(moved[0].join("luuna.sqlite3-wal")).unwrap(),
            b"stale"
        );
    }
}
