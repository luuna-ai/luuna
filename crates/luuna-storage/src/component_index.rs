//! Per-project file authority for activation and memory bindings, never receipts.
use crate::{project_files::existing, project_index::PROJECT_RECORDS, Result, StorageError};
use luuna_domain::valid_project_id;
use rusqlite::{params, Connection, Transaction};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
};
const FILE: &str = "components.json";
const TEMP: &str = "components.json.tmp";
const MAX_BYTES: u64 = 8 * 1024 * 1024;
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Project {
    project_id: String,
    state: String,
    updated_at: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Memory {
    id: String,
    project_id: String,
    scope: String,
    source_id: String,
    domain: String,
    created_at: String,
    deleted: bool,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Document {
    schema_version: u64,
    project_id: String,
    projects: Vec<Project>,
    memories: Vec<Memory>,
}
fn refused(path: &Path, detail: impl Into<String>) -> StorageError {
    StorageError::ProjectFile {
        path: path.display().to_string(),
        detail: detail.into(),
    }
}
fn folder(data: &Path, id: &str) -> Result<PathBuf> {
    if !valid_project_id(id) {
        return Err(StorageError::Invalid(
            "component record project id is invalid".into(),
        ));
    }
    Ok(data.join(PROJECT_RECORDS).join(id))
}
fn check(path: &Path, dir: bool) -> Result<bool> {
    existing(path, dir).map(|m| m.is_some()).map_err(|_| {
        refused(
            path,
            "a required entry is linked, unreadable or has the wrong type",
        )
    })
}
fn validate(document: &Document, id: &str, path: &Path) -> Result<()> {
    if document.schema_version != 1 {
        return Err(refused(
            path,
            if document.schema_version > 1 {
                "written by a newer Luuna; run that Luuna build"
            } else {
                "unsupported component file schema_version"
            },
        ));
    }
    if document.project_id != id {
        return Err(refused(
            path,
            "component file belongs to a different project",
        ));
    }
    let mut projects = BTreeSet::new();
    let mut memories = BTreeSet::new();
    for p in &document.projects {
        if p.project_id != id
            || !projects.insert(&p.project_id)
            || !matches!(p.state.as_str(), "initializing" | "enabled")
        {
            return Err(refused(path, "invalid or duplicate activation record"));
        }
    }
    for m in &document.memories {
        if m.id.is_empty()
            || m.project_id != id
            || !memories.insert(&m.id)
            || m.scope.is_empty()
            || m.source_id.is_empty()
            || m.domain.is_empty()
        {
            return Err(refused(path, "invalid or duplicate memory binding"));
        }
    }
    Ok(())
}
fn parse(bytes: &[u8], id: &str, path: &Path) -> Result<Document> {
    if bytes.len() as u64 > MAX_BYTES {
        return Err(refused(path, "component file is larger than 8 MiB"));
    }
    let value: serde_json::Value = serde_json::from_slice(bytes).map_err(|e| {
        refused(
            path,
            format!(
                "component file is not JSON at line {} column {}",
                e.line(),
                e.column()
            ),
        )
    })?;
    if value
        .get("schema_version")
        .and_then(serde_json::Value::as_u64)
        .is_some_and(|v| v > 1)
    {
        return Err(refused(
            path,
            "written by a newer Luuna; run that Luuna build",
        ));
    }
    if !value.is_object()
        || ["projects", "memories"].iter().any(|key| {
            !value
                .get(key)
                .and_then(serde_json::Value::as_array)
                .is_some_and(|rows| rows.iter().all(serde_json::Value::is_object))
        })
    {
        return Err(refused(
            path,
            "component file requires named objects and record arrays",
        ));
    }
    // Decode original bytes too, so duplicate fields cannot disappear in Value.
    let mut doc: Document = serde_json::from_slice(bytes).map_err(|e| {
        refused(
            path,
            format!(
                "invalid component file fields at line {} column {}",
                e.line(),
                e.column()
            ),
        )
    })?;
    validate(&doc, id, path)?;
    doc.projects.sort_by(|a, b| a.project_id.cmp(&b.project_id));
    doc.memories.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(doc)
}
fn read(data: &Path, id: &str) -> Result<Option<(Document, Vec<u8>)>> {
    let root = data.join(PROJECT_RECORDS);
    let dir = folder(data, id)?;
    let path = dir.join(FILE);
    if !check(&root, true)? || !check(&dir, true)? || !check(&path, false)? {
        return Ok(None);
    }
    let file = fs::File::open(&path).map_err(|e| {
        refused(
            &path,
            format!("cannot read component file ({:?})", e.kind()),
        )
    })?;
    let mut bytes = vec![];
    file.take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| {
            refused(
                &path,
                format!("cannot read component file ({:?})", e.kind()),
            )
        })?;
    Ok(Some((parse(&bytes, id, &path)?, bytes)))
}
fn database(connection: &Connection, id: &str) -> Result<Document> {
    let projects = connection
        .prepare("SELECT project_id,state,updated_at FROM component_projects WHERE project_id=?1")?
        .query_map([id], |r| {
            Ok(Project {
                project_id: r.get(0)?,
                state: r.get(1)?,
                updated_at: r.get(2)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let memories=connection.prepare("SELECT id,project_id,scope,source_id,domain,created_at,deleted FROM component_memories WHERE project_id=?1 ORDER BY id")?.query_map([id],|r|Ok(Memory{id:r.get(0)?,project_id:r.get(1)?,scope:r.get(2)?,source_id:r.get(3)?,domain:r.get(4)?,created_at:r.get(5)?,deleted:r.get(6)?}))?.collect::<std::result::Result<Vec<_>,_>>()?;
    Ok(Document {
        schema_version: 1,
        project_id: id.into(),
        projects,
        memories,
    })
}
fn render(document: &Document, path: &Path) -> Result<Vec<u8>> {
    validate(document, &document.project_id, path)?;
    let mut bytes = serde_json::to_vec_pretty(document)?;
    bytes.push(b'\n');
    if bytes.len() as u64 > MAX_BYTES {
        return Err(refused(path, "component file is larger than 8 MiB"));
    }
    Ok(bytes)
}
fn writable(data: &Path, id: &str) -> Result<bool> {
    let dir = folder(data, id)?;
    check(&data.join(PROJECT_RECORDS), true)?;
    check(&dir, true)?;
    check(&dir.join(FILE), false)?;
    check(&dir.join(TEMP), false)
}
fn write(data: &Path, id: &str, bytes: &[u8]) -> Result<()> {
    let stale = writable(data, id)?;
    let dir = folder(data, id)?;
    let path = dir.join(FILE);
    let temp = dir.join(TEMP);
    let mut builder = fs::DirBuilder::new();
    builder.recursive(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    for directory in [data.join(PROJECT_RECORDS), dir.clone()] {
        if !check(&directory, true)? {
            builder.create(&directory).map_err(|e| {
                refused(
                    &path,
                    format!("cannot create component record directory ({:?})", e.kind()),
                )
            })?;
        }
    }
    if stale {
        fs::remove_file(&temp).map_err(|e| {
            refused(
                &path,
                format!(
                    "cannot remove stale component temporary file ({:?})",
                    e.kind()
                ),
            )
        })?;
    }
    let mut options = fs::OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let result = (|| {
        let mut handle = options.open(&temp)?;
        handle.write_all(bytes)?;
        handle.sync_all()?;
        drop(handle);
        fs::rename(&temp, &path)
    })();
    if let Err(error) = result {
        let _ = fs::remove_file(&temp);
        return Err(refused(
            &path,
            format!("cannot write component file ({:?})", error.kind()),
        ));
    }
    #[cfg(unix)]
    fs::File::open(&dir)
        .and_then(|h| h.sync_all())
        .map_err(|e| {
            refused(
                &path,
                format!("cannot sync component directory ({:?})", e.kind()),
            )
        })?;
    Ok(())
}
/// Writers hold an immediate transaction across file replacement. Commit
/// failure restores old bytes; memory-only stores commit without file I/O.
pub(crate) fn commit(
    tx: Transaction<'_>,
    data: Option<&Path>,
    project_id: Option<&str>,
) -> Result<()> {
    let previous = if let (Some(data), Some(id)) = (data, project_id) {
        let old = read(data, id)?.map(|(_, bytes)| bytes);
        let doc = database(&tx, id)?;
        if old.is_none() && doc.projects.is_empty() && doc.memories.is_empty() {
            None
        } else {
            let bytes = render(&doc, &folder(data, id)?.join(FILE))?;
            write(data, id, &bytes)?;
            Some((data, id, old))
        }
    } else {
        None
    };
    if let Err(commit) = tx.commit() {
        if let Some((data, id, old)) = previous {
            let restored = match old {
                Some(bytes) => write(data, id, &bytes),
                None => (|| -> Result<()> {
                    let dir = folder(data, id)?;
                    let path = dir.join(FILE);
                    if check(&path, false)? {
                        fs::remove_file(&path)?;
                        #[cfg(unix)]
                        fs::File::open(&dir)?.sync_all()?;
                    }
                    Ok(())
                })(),
            };
            if let Err(restore) = restored {
                return Err(StorageError::Io(std::io::Error::other(format!("database commit failed ({commit}); component file could not be restored ({restore})"))));
            }
        }
        return Err(commit.into());
    }
    Ok(())
}
enum Action {
    None,
    Backfill(Vec<u8>),
    Import(Document),
}
pub(crate) struct Plan {
    project_id: String,
    action: Action,
}
fn compare(file: &Document, db: &Document, path: &Path) -> Result<()> {
    let projects: BTreeMap<_, _> = file.projects.iter().map(|p| (&p.project_id, p)).collect();
    for old in &db.projects {
        let Some(new) = projects.get(&old.project_id) else {
            return Err(refused(
                path,
                "component file lacks database activation history",
            ));
        };
        if old.state == "enabled" && new.state != "enabled" {
            return Err(refused(path, "component file would roll activation back"));
        }
    }
    let memories: BTreeMap<_, _> = file.memories.iter().map(|m| (&m.id, m)).collect();
    for old in &db.memories {
        let Some(new) = memories.get(&old.id) else {
            return Err(refused(
                path,
                "component file lacks database memory bindings",
            ));
        };
        if old.project_id != new.project_id
            || old.scope != new.scope
            || old.source_id != new.source_id
            || old.domain != new.domain
            || (old.deleted && !new.deleted)
        {
            return Err(refused(
                path,
                "component file changes binding identity or resurrects a tombstone",
            ));
        }
    }
    Ok(())
}
/// Plan both indexes before mutation, inside the same SQLite writer lease.
/// File project ids must be registered or provided by valid project files.
pub(crate) fn plan(
    connection: &Connection,
    data: &Path,
    incoming: &BTreeSet<String>,
) -> Result<Vec<Plan>> {
    let root = data.join(PROJECT_RECORDS);
    let mut files = BTreeMap::new();
    if check(&root, true)? {
        for entry in fs::read_dir(&root)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let name = entry.file_name();
            let id = name.to_str().unwrap_or_default();
            if check(&entry.path().join(FILE), false)? {
                if let Some((doc, _)) = read(data, id)? {
                    files.insert(id.to_owned(), doc);
                }
            }
        }
    }
    let mut ids=connection.prepare("SELECT project_id FROM component_projects UNION SELECT project_id FROM component_memories")?.query_map([],|r|r.get::<_,String>(0))?.collect::<std::result::Result<BTreeSet<_>,_>>()?;
    ids.extend(files.keys().cloned());
    let mut known = incoming.clone();
    known.extend(
        connection
            .prepare("SELECT id FROM projects")?
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?,
    );
    let mut owners = connection
        .prepare("SELECT id,project_id FROM component_memories")?
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
        .collect::<std::result::Result<BTreeMap<_, _>, _>>()?;
    for (id, doc) in &files {
        let path = folder(data, id)?.join(FILE);
        for memory in &doc.memories {
            if owners.get(&memory.id).is_some_and(|owner| owner != id) {
                return Err(refused(
                    &path,
                    "memory binding identity belongs to another project",
                ));
            }
            owners.insert(memory.id.clone(), id.clone());
        }
    }
    let mut plans = vec![];
    for id in ids {
        let path = folder(data, &id)?.join(FILE);
        if !known.contains(&id) {
            return Err(refused(
                &path,
                "component record names a project absent from registrations",
            ));
        }
        let db = database(connection, &id)?;
        let action = match files.remove(&id) {
            None => {
                let bytes = render(&db, &path)?;
                writable(data, &id)?;
                Action::Backfill(bytes)
            }
            Some(file) => {
                compare(&file, &db, &path)?;
                if file == db {
                    Action::None
                } else {
                    Action::Import(file)
                }
            }
        };
        plans.push(Plan {
            project_id: id,
            action,
        });
    }
    Ok(plans)
}
pub(crate) fn apply(tx: &Transaction<'_>, plans: &[Plan]) -> Result<()> {
    for plan in plans {
        if let Action::Import(file) = &plan.action {
            for p in &file.projects {
                tx.execute("INSERT INTO component_projects(project_id,state,updated_at) VALUES (?1,?2,?3) ON CONFLICT(project_id) DO UPDATE SET state=excluded.state,updated_at=excluded.updated_at",params![p.project_id,p.state,p.updated_at])?;
            }
            for m in &file.memories {
                tx.execute("INSERT INTO component_memories(id,project_id,scope,source_id,domain,created_at,deleted) VALUES (?1,?2,?3,?4,?5,?6,?7) ON CONFLICT(id) DO UPDATE SET created_at=excluded.created_at,deleted=excluded.deleted",params![m.id,m.project_id,m.scope,m.source_id,m.domain,m.created_at,m.deleted])?;
            }
        }
    }
    Ok(())
}
pub(crate) fn backfill(data: &Path, plans: &[Plan]) -> Result<()> {
    for plan in plans {
        if let Action::Backfill(bytes) = &plan.action {
            write(data, &plan.project_id, bytes)?;
        }
    }
    Ok(())
}
