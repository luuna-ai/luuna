//! Per-project ticket file (ADR 0060): the tickets of one project, with the
//! Missions delivery binding each one carries, so both survive a lost
//! database.
//!
//! The database stays the writer: a ticket change commits first, and the file
//! is then replaced from the database under a fresh writer lease. A file can
//! therefore only fall behind, never move ahead, except when the database is
//! lost. At open the two are compared ticket by ticket: a file behind is
//! rewritten, a database behind takes the file, and anything else refuses the
//! open before a row or file changes.
use crate::{project_files::existing, project_index::PROJECT_RECORDS, Result, StorageError};
use luuna_domain::{valid_project_id, Ticket, TicketSource};
use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
};

const FILE: &str = "tickets.json";
const TEMP: &str = "tickets.json.tmp";
const MAX_BYTES: u64 = 64 * 1024 * 1024;
/// The per-project ticket ceiling `create_ticket` enforces.
const MAX_TICKETS: usize = 1000;

/// The Missions delivery state of one ticket (`ticket_sources` and
/// `ticket_delivery_attempts`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Missions {
    source_id: Option<String>,
    source_revision: Option<String>,
    delivery_started: bool,
    /// The content first offered to Missions, kept for idempotent retries.
    original: Ticket,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    idempotency_key: String,
    ticket: Ticket,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    missions: Option<Missions>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Document {
    schema_version: u64,
    project_id: String,
    tickets: Vec<Entry>,
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
            "ticket file project id is invalid".into(),
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

/// How far a ticket's Missions delivery has gone; it never goes back.
fn stage(entry: &Entry) -> u8 {
    match &entry.missions {
        None => 0,
        Some(m) if m.source_id.is_some() => 3,
        Some(m) if m.delivery_started => 2,
        Some(_) => 1,
    }
}

fn validate(document: &Document, id: &str, path: &Path) -> Result<()> {
    if document.schema_version != 1 {
        return Err(refused(
            path,
            if document.schema_version > 1 {
                "written by a newer Luuna; run that Luuna build"
            } else {
                "unsupported ticket file schema_version"
            },
        ));
    }
    if document.project_id != id {
        return Err(refused(path, "ticket file belongs to a different project"));
    }
    if document.tickets.len() > MAX_TICKETS {
        return Err(refused(path, "ticket file holds more than 1000 tickets"));
    }
    let (mut ids, mut keys, mut sources) = (BTreeSet::new(), BTreeSet::new(), BTreeSet::new());
    for (index, entry) in document.tickets.iter().enumerate() {
        let at = format!("ticket {}", index + 1);
        let ticket = &entry.ticket;
        if ticket.project_id != id
            || ticket.validate().is_err()
            || entry.idempotency_key.is_empty()
            || !ids.insert(&ticket.id)
            || !keys.insert(&entry.idempotency_key)
        {
            return Err(refused(
                path,
                format!("{at} is invalid, another project's or a duplicate"),
            ));
        }
        let bound = ticket.source == TicketSource::Missions;
        match &entry.missions {
            None if bound => {
                return Err(refused(
                    path,
                    format!("{at} is a Missions ticket without its binding"),
                ))
            }
            None => {}
            Some(m) => {
                let identity = match (&m.source_id, &m.source_revision) {
                    (Some(source), Some(revision)) => {
                        !source.is_empty()
                            && source.len() <= 200
                            && revision.len() == 64
                            && revision.bytes().all(|b| b.is_ascii_hexdigit())
                            && sources.insert(source)
                    }
                    (None, None) => true,
                    _ => false,
                };
                if !identity
                    || bound != m.source_id.is_some()
                    || m.original.id != ticket.id
                    || m.original.project_id != id
                    || m.original.validate().is_err()
                {
                    return Err(refused(
                        path,
                        format!("{at} has an invalid or duplicate Missions binding"),
                    ));
                }
            }
        }
    }
    Ok(())
}

fn parse(bytes: &[u8], id: &str, path: &Path) -> Result<Document> {
    if bytes.len() as u64 > MAX_BYTES {
        return Err(refused(path, "ticket file is larger than 64 MiB"));
    }
    let value: serde_json::Value = serde_json::from_slice(bytes).map_err(|e| {
        refused(
            path,
            format!(
                "ticket file is not JSON at line {} column {}",
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
    let document: Document = serde_json::from_slice(bytes).map_err(|e| {
        refused(
            path,
            format!(
                "invalid ticket file fields at line {} column {}",
                e.line(),
                e.column()
            ),
        )
    })?;
    validate(&document, id, path)?;
    Ok(document)
}

fn read(data: &Path, id: &str) -> Result<Option<(Document, Vec<u8>)>> {
    let dir = folder(data, id)?;
    let path = dir.join(FILE);
    if !check(&data.join(PROJECT_RECORDS), true)? || !check(&dir, true)? || !check(&path, false)? {
        return Ok(None);
    }
    let mut bytes = vec![];
    fs::File::open(&path)
        .and_then(|file| file.take(MAX_BYTES + 1).read_to_end(&mut bytes))
        .map_err(|e| refused(&path, format!("cannot read ticket file ({:?})", e.kind())))?;
    Ok(Some((parse(&bytes, id, &path)?, bytes)))
}

fn database(connection: &Connection, id: &str) -> Result<Document> {
    type Row = (
        String,
        String,
        Option<(Option<String>, Option<String>, String, bool)>,
    );
    let rows: Vec<Row> = connection
        .prepare(
            "SELECT t.idempotency_key,t.payload,s.ticket_id IS NOT NULL,s.source_id,s.source_revision,s.original_payload,
                    EXISTS(SELECT 1 FROM ticket_delivery_attempts a WHERE a.ticket_id=t.id)
             FROM tickets t LEFT JOIN ticket_sources s ON s.ticket_id=t.id
             WHERE t.project_id=?1 ORDER BY t.created_at,t.rowid",
        )?
        .query_map([id], |r| {
            let bound: bool = r.get(2)?;
            Ok((
                r.get(0)?,
                r.get(1)?,
                bound
                    .then(|| -> rusqlite::Result<_> {
                        Ok((r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?))
                    })
                    .transpose()?,
            ))
        })?
        .collect::<std::result::Result<_, _>>()?;
    let tickets = rows
        .into_iter()
        .map(|(idempotency_key, payload, missions)| {
            Ok(Entry {
                idempotency_key,
                ticket: serde_json::from_str(&payload)?,
                missions: missions
                    .map(|(source_id, source_revision, original, delivery_started)| {
                        Ok::<_, StorageError>(Missions {
                            source_id,
                            source_revision,
                            delivery_started,
                            original: serde_json::from_str(&original)?,
                        })
                    })
                    .transpose()?,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(Document {
        schema_version: 1,
        project_id: id.into(),
        tickets,
    })
}

fn render(document: &Document) -> Result<Vec<u8>> {
    let mut bytes = serde_json::to_vec_pretty(document)?;
    bytes.push(b'\n');
    Ok(bytes)
}

fn writable(data: &Path, id: &str) -> Result<bool> {
    let dir = folder(data, id)?;
    check(&data.join(PROJECT_RECORDS), true)?;
    check(&dir, true)?;
    check(&dir.join(FILE), false)?;
    check(&dir.join(TEMP), false)
}

/// Replace the whole file through a fresh private temporary file and a rename.
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
                    format!("cannot create project record directory ({:?})", e.kind()),
                )
            })?;
        }
    }
    if stale {
        fs::remove_file(&temp).map_err(|e| {
            refused(
                &path,
                format!("cannot remove a stale temporary file ({:?})", e.kind()),
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
            format!("cannot write ticket file ({:?})", error.kind()),
        ));
    }
    #[cfg(unix)]
    fs::File::open(&dir)
        .and_then(|h| h.sync_all())
        .map_err(|e| {
            refused(
                &path,
                format!("cannot sync project record directory ({:?})", e.kind()),
            )
        })?;
    Ok(())
}

fn mirror_one(tx: &Transaction<'_>, data: &Path, id: &str) -> Result<()> {
    let document = database(tx, id)?;
    if document.tickets.is_empty() {
        // Tickets are never deleted, so an empty database with a file is a
        // lost database: leave the file for the next open to import.
        return Ok(());
    }
    let bytes = render(&document)?;
    let current = match read(data, id) {
        Ok(found) => found.map(|(_, bytes)| bytes),
        // A file the open would refuse is left for the open to report.
        Err(_) => return Ok(()),
    };
    if current.as_deref() != Some(&bytes[..]) {
        write(data, id, &bytes)?;
    }
    Ok(())
}

/// After a ticket commit, replace each project's ticket file from the
/// database under a fresh writer lease. A failure is left for the next open,
/// which rewrites a file that is behind.
pub(crate) fn mirror(connection: &mut Connection, data: Option<&Path>, ids: &[&str]) {
    let Some(data) = data else {
        return;
    };
    let Ok(tx) = connection.transaction_with_behavior(TransactionBehavior::Immediate) else {
        return;
    };
    for id in ids {
        let _ = mirror_one(&tx, data, id);
    }
    let _ = tx.commit();
}

/// Whether `old` is the same ticket as `new` at the same or an earlier point:
/// same identity, a revision no later, a delivery stage no later, and an
/// immutable Missions identity and original content once both have them.
fn not_ahead(old: &Entry, new: &Entry) -> bool {
    let (a, b) = (&old.ticket, &new.ticket);
    if a.id != b.id
        || a.project_id != b.project_id
        || a.created_at != b.created_at
        || old.idempotency_key != new.idempotency_key
        || a.revision > b.revision
        || stage(old) > stage(new)
    {
        return false;
    }
    if let (Some(x), Some(y)) = (&old.missions, &new.missions) {
        if x.original != y.original || (x.source_id.is_some() && x.source_id != y.source_id) {
            return false;
        }
    }
    if a.revision < b.revision {
        return true;
    }
    // At one revision only binding the delivery may change the ticket, and
    // then only its source.
    let mut unbound = b.clone();
    unbound.source = a.source;
    old == new || (stage(old) < stage(new) && *a == unbound)
}

fn covers(behind: &Document, ahead: &Document) -> bool {
    let ahead: BTreeMap<_, _> = ahead
        .tickets
        .iter()
        .map(|e| (e.ticket.id.as_str(), e))
        .collect();
    behind.tickets.iter().all(|e| {
        ahead
            .get(e.ticket.id.as_str())
            .is_some_and(|a| not_ahead(e, a))
    })
}

enum Action {
    Rewrite(Vec<u8>),
    Import(Document),
}

pub(crate) struct Plan {
    project_id: String,
    action: Action,
}

/// Decide every ticket file once the projects and their repositories are
/// indexed in `tx`. Every refusal happens here, before anything changes.
pub(crate) fn plan(tx: &Transaction<'_>, data: &Path) -> Result<Vec<Plan>> {
    let root = data.join(PROJECT_RECORDS);
    let mut files = BTreeMap::new();
    if check(&root, true)? {
        for entry in fs::read_dir(&root)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let name = entry.file_name();
            let Some(id) = name.to_str().filter(|id| valid_project_id(id)) else {
                continue;
            };
            if let Some((document, _)) = read(data, id)? {
                files.insert(id.to_owned(), document);
            }
        }
    }
    let known: BTreeSet<String> = tx
        .prepare("SELECT id FROM projects")?
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<std::result::Result<_, _>>()?;
    let mut ids: BTreeSet<String> = tx
        .prepare("SELECT DISTINCT project_id FROM tickets")?
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<std::result::Result<_, _>>()?;
    ids.extend(files.keys().cloned());
    let mut owners: BTreeMap<String, String> = BTreeMap::new();
    let mut plans = vec![];
    for id in ids {
        let path = folder(data, &id)?.join(FILE);
        let db = database(tx, &id)?;
        let Some(file) = files.remove(&id) else {
            writable(data, &id)?;
            plans.push(Plan {
                project_id: id,
                action: Action::Rewrite(render(&db)?),
            });
            continue;
        };
        if !known.contains(&id) {
            return Err(refused(
                &path,
                "ticket file names a project absent from registrations",
            ));
        }
        for entry in &file.tickets {
            let ticket = &entry.ticket.id;
            let elsewhere: Option<String> = tx
                .query_row(
                    "SELECT project_id FROM tickets WHERE id=?1 AND project_id!=?2",
                    params![ticket, id],
                    |r| r.get(0),
                )
                .optional()?;
            if elsewhere.is_some()
                || owners
                    .insert(ticket.clone(), id.clone())
                    .is_some_and(|other| other != id)
            {
                return Err(refused(
                    &path,
                    "a ticket id in this file belongs to another project",
                ));
            }
        }
        let action = if file == db {
            continue;
        } else if covers(&file, &db) {
            writable(data, &id)?;
            Action::Rewrite(render(&db)?)
        } else if covers(&db, &file) {
            let registered: BTreeSet<String> = crate::projects::repositories_in(tx, &id)?
                .into_iter()
                .map(|r| r.repository_id)
                .collect();
            if file.tickets.iter().any(|e| {
                e.ticket
                    .repository_ids
                    .iter()
                    .any(|r| !registered.contains(r))
            }) {
                return Err(refused(
                    &path,
                    "a ticket in this file names an unregistered repository",
                ));
            }
            Action::Import(file)
        } else {
            return Err(refused(&path, "ticket file and database disagree"));
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
        let Action::Import(file) = &plan.action else {
            continue;
        };
        for entry in &file.tickets {
            let t = &entry.ticket;
            tx.execute(
                "INSERT INTO tickets(id,project_id,revision,idempotency_key,payload,created_at,updated_at) VALUES (?1,?2,?3,?4,?5,?6,?7)
                 ON CONFLICT(id) DO UPDATE SET revision=excluded.revision,payload=excluded.payload,updated_at=excluded.updated_at",
                params![
                    t.id,
                    t.project_id,
                    t.revision,
                    entry.idempotency_key,
                    serde_json::to_string(t)?,
                    t.created_at,
                    t.updated_at
                ],
            )?;
            if let Some(m) = &entry.missions {
                tx.execute(
                    "INSERT INTO ticket_sources(ticket_id,project_id,source_id,source_revision,original_payload) VALUES (?1,?2,?3,?4,?5)
                     ON CONFLICT(ticket_id) DO UPDATE SET source_id=excluded.source_id,source_revision=excluded.source_revision",
                    params![
                        t.id,
                        t.project_id,
                        m.source_id,
                        m.source_revision,
                        serde_json::to_string(&m.original)?
                    ],
                )?;
                if m.delivery_started {
                    tx.execute(
                        "INSERT OR IGNORE INTO ticket_delivery_attempts(ticket_id) VALUES (?1)",
                        [&t.id],
                    )?;
                }
            }
        }
    }
    Ok(())
}

pub(crate) fn backfill(data: &Path, plans: &[Plan]) -> Result<()> {
    for plan in plans {
        if let Action::Rewrite(bytes) = &plan.action {
            write(data, &plan.project_id, bytes)?;
        }
    }
    Ok(())
}
