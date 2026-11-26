//! Per-project append-only files for the project event stream and the
//! discovery proposals (ADR 0057), so both survive a lost database.
//!
//! The database row is written first; the file line follows the commit, under
//! a fresh writer lease, so a file is always a prefix of the database. A crash
//! in between leaves the file behind, and the next open appends what it lacks.
//! With the database lost, the next open imports the files instead. A file
//! that disagrees with the database refuses the open before anything changes.
//! Each line is exactly the text the database row holds.
use crate::{
    project_files::existing, project_index::PROJECT_RECORDS, projects::project_stream_id, Result,
    StorageError,
};
use luuna_domain::{valid_project_id, Discovery, EventEnvelope};
use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

/// A whole log larger than this refuses the open rather than load unbounded.
const MAX_FILE_BYTES: u64 = 256 * 1024 * 1024;
/// One line larger than this is refused; the tail read covers two of them.
const MAX_LINE_BYTES: usize = 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Log {
    Events,
    Discoveries,
}

impl Log {
    const ALL: [Log; 2] = [Log::Events, Log::Discoveries];
    fn file(self) -> &'static str {
        match self {
            Log::Events => "events.jsonl",
            Log::Discoveries => "discoveries.jsonl",
        }
    }
    fn temp(self) -> &'static str {
        match self {
            Log::Events => "events.jsonl.tmp",
            Log::Discoveries => "discoveries.jsonl.tmp",
        }
    }
    fn noun(self) -> &'static str {
        match self {
            Log::Events => "project event",
            Log::Discoveries => "discovery",
        }
    }
}

/// One validated line: its identity in the database, and its exact text.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Line {
    key: String,
    created_at: Option<String>,
    text: String,
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
            "project log project id is invalid".into(),
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

/// Parse one line and check it belongs at `index` of this project's log.
fn parse_line(
    log: Log,
    id: &str,
    index: usize,
    text: &str,
    seen: &mut BTreeSet<String>,
    path: &Path,
) -> Result<Line> {
    let at = || format!("line {}", index + 1);
    if text.len() > MAX_LINE_BYTES {
        return Err(refused(path, format!("{} is larger than 1 MiB", at())));
    }
    match log {
        Log::Events => {
            let event: EventEnvelope = serde_json::from_str(text)
                .map_err(|_| refused(path, format!("{} is not a project event", at())))?;
            let stream = project_stream_id(id);
            if event.project_id != id
                || event.run_id != stream
                || event.sequence != index as u64 + 1
                || event.event_id != format!("{stream}:{}", event.sequence)
                || event.event_type.trim().is_empty()
                || !event.payload.is_object()
            {
                return Err(refused(
                    path,
                    format!("{} is not the next event of this project's stream", at()),
                ));
            }
            Ok(Line {
                key: event.sequence.to_string(),
                created_at: None,
                text: text.to_owned(),
            })
        }
        Log::Discoveries => {
            let discovery: Discovery = serde_json::from_str(text)
                .map_err(|_| refused(path, format!("{} is not a discovery", at())))?;
            if discovery.project_id != id
                || discovery.id.trim().is_empty()
                || !seen.insert(discovery.id.clone())
            {
                return Err(refused(
                    path,
                    format!("{} is a duplicate or another project's discovery", at()),
                ));
            }
            Ok(Line {
                key: discovery.id,
                created_at: Some(discovery.created_at),
                text: text.to_owned(),
            })
        }
    }
}

/// A file read at open: its valid lines, and whether a torn last line (a
/// write cut short before its newline) has to be dropped.
struct FileLog {
    lines: Vec<Line>,
    torn: bool,
}

fn read_file(data: &Path, id: &str, log: Log) -> Result<Option<FileLog>> {
    let dir = folder(data, id)?;
    let path = dir.join(log.file());
    if !check(&data.join(PROJECT_RECORDS), true)? || !check(&dir, true)? || !check(&path, false)? {
        return Ok(None);
    }
    let mut bytes = vec![];
    fs::File::open(&path)
        .and_then(|file| file.take(MAX_FILE_BYTES + 1).read_to_end(&mut bytes))
        .map_err(|e| {
            refused(
                &path,
                format!("cannot read {} file ({:?})", log.noun(), e.kind()),
            )
        })?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err(refused(
            &path,
            format!("{} file is larger than 256 MiB", log.noun()),
        ));
    }
    let torn = !bytes.is_empty() && !bytes.ends_with(b"\n");
    let complete = match bytes.iter().rposition(|b| *b == b'\n') {
        Some(end) => &bytes[..=end],
        None => &[][..],
    };
    let text = std::str::from_utf8(complete)
        .map_err(|_| refused(&path, format!("{} file is not UTF-8", log.noun())))?;
    let mut seen = BTreeSet::new();
    let lines = text
        .lines()
        .enumerate()
        .map(|(index, line)| parse_line(log, id, index, line, &mut seen, &path))
        .collect::<Result<Vec<_>>>()?;
    Ok(Some(FileLog { lines, torn }))
}

fn database(connection: &Connection, id: &str, log: Log) -> Result<Vec<Line>> {
    let sql = match log {
        Log::Events => {
            "SELECT CAST(sequence AS TEXT),NULL,envelope FROM project_events WHERE project_id=?1 ORDER BY sequence"
        }
        Log::Discoveries => {
            "SELECT id,created_at,payload FROM discoveries WHERE project_id=?1 ORDER BY rowid"
        }
    };
    let rows = connection
        .prepare(sql)?
        .query_map([id], |r| {
            Ok(Line {
                key: r.get(0)?,
                created_at: r.get(1)?,
                text: r.get(2)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

fn render(lines: &[Line]) -> Vec<u8> {
    let mut bytes = Vec::new();
    for line in lines {
        bytes.extend_from_slice(line.text.as_bytes());
        bytes.push(b'\n');
    }
    bytes
}

fn create_folders(data: &Path, dir: &Path, path: &Path) -> Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    for directory in [data.join(PROJECT_RECORDS), dir.to_path_buf()] {
        if !check(&directory, true)? {
            builder.create(&directory).map_err(|e| {
                refused(
                    path,
                    format!("cannot create project record directory ({:?})", e.kind()),
                )
            })?;
        }
    }
    Ok(())
}

fn sync_dir(dir: &Path, path: &Path) -> Result<()> {
    #[cfg(unix)]
    fs::File::open(dir)
        .and_then(|h| h.sync_all())
        .map_err(|e| {
            refused(
                path,
                format!("cannot sync project record directory ({:?})", e.kind()),
            )
        })?;
    #[cfg(not(unix))]
    let _ = (dir, path);
    Ok(())
}

fn writable(data: &Path, id: &str, log: Log) -> Result<()> {
    let dir = folder(data, id)?;
    check(&data.join(PROJECT_RECORDS), true)?;
    check(&dir, true)?;
    check(&dir.join(log.file()), false)?;
    check(&dir.join(log.temp()), false)?;
    Ok(())
}

/// Replace the whole file through a fresh temporary file and a rename.
fn write(data: &Path, id: &str, log: Log, bytes: &[u8]) -> Result<()> {
    writable(data, id, log)?;
    let dir = folder(data, id)?;
    let path = dir.join(log.file());
    let temp = dir.join(log.temp());
    create_folders(data, &dir, &path)?;
    if check(&temp, false)? {
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
            format!("cannot write {} file ({:?})", log.noun(), error.kind()),
        ));
    }
    sync_dir(&dir, &path)
}

/// The last complete line of an existing file and the offset just past it,
/// read from the tail only. A torn tail is reported by `end < len`.
fn tail(path: &Path) -> Result<Option<(String, u64, u64)>> {
    let mut file = fs::File::open(path)?;
    let len = file.metadata()?.len();
    let window = len.min(2 * MAX_LINE_BYTES as u64 + 2);
    file.seek(SeekFrom::Start(len - window))?;
    let mut bytes = Vec::with_capacity(window as usize);
    file.read_to_end(&mut bytes)?;
    let Some(last) = bytes.iter().rposition(|b| *b == b'\n') else {
        return Ok(None);
    };
    let start = bytes[..last]
        .iter()
        .rposition(|b| *b == b'\n')
        .map_or(0, |i| i + 1);
    if start == 0 && window < len {
        // The line began before the window: too long to be one of ours.
        return Ok(None);
    }
    let text = String::from_utf8(bytes[start..last].to_vec())
        .map_err(|_| StorageError::Io(std::io::Error::other("project log tail is not UTF-8")))?;
    Ok(Some((text, len - window + last as u64 + 1, len)))
}

fn key_of(log: Log, text: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    match log {
        Log::Events => value.get("sequence")?.as_u64().map(|s| s.to_string()),
        Log::Discoveries => value.get("id")?.as_str().map(str::to_owned),
    }
}

/// Lines the database holds after the file's last one, or `None` when the
/// file's last line is not the database's (the next open decides then).
fn missing(
    tx: &Transaction<'_>,
    id: &str,
    log: Log,
    last: Option<&str>,
) -> Result<Option<Vec<Line>>> {
    let Some(last) = last else {
        return database(tx, id, log).map(Some);
    };
    let Some(key) = key_of(log, last) else {
        return Ok(None);
    };
    let (anchor, after) = match log {
        Log::Events => (
            "SELECT envelope,sequence FROM project_events WHERE project_id=?1 AND sequence=CAST(?2 AS INTEGER)",
            "SELECT CAST(sequence AS TEXT),NULL,envelope FROM project_events WHERE project_id=?1 AND sequence>?2 ORDER BY sequence",
        ),
        Log::Discoveries => (
            "SELECT payload,rowid FROM discoveries WHERE project_id=?1 AND id=?2",
            "SELECT id,created_at,payload FROM discoveries WHERE project_id=?1 AND rowid>?2 ORDER BY rowid",
        ),
    };
    let found: Option<(String, i64)> = tx
        .query_row(anchor, params![id, key], |r| Ok((r.get(0)?, r.get(1)?)))
        .optional()?;
    let Some((_, position)) = found.filter(|(text, _)| text == last) else {
        return Ok(None);
    };
    let rows = tx
        .prepare(after)?
        .query_map(params![id, position], |r| {
            Ok(Line {
                key: r.get(0)?,
                created_at: r.get(1)?,
                text: r.get(2)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(Some(rows))
}

fn mirror_one(tx: &Transaction<'_>, data: &Path, id: &str, log: Log) -> Result<()> {
    let dir = folder(data, id)?;
    let path = dir.join(log.file());
    writable(data, id, log)?;
    if !check(&path, false)? {
        let rows = database(tx, id, log)?;
        if !rows.is_empty() {
            write(data, id, log, &render(&rows))?;
        }
        return Ok(());
    }
    let (last, end, len) = match tail(&path)? {
        Some((text, end, len)) => (Some(text), end, len),
        None => {
            let len = fs::metadata(&path)?.len();
            if len > 2 * MAX_LINE_BYTES as u64 {
                return Ok(());
            }
            (None, 0, len)
        }
    };
    let Some(rows) = missing(tx, id, log, last.as_deref())? else {
        return Ok(());
    };
    if rows.is_empty() && end == len {
        return Ok(());
    }
    let mut file = fs::OpenOptions::new().write(true).open(&path)?;
    if end < len {
        file.set_len(end)?;
    }
    file.seek(SeekFrom::Start(end))?;
    file.write_all(&render(&rows))?;
    file.sync_all()?;
    Ok(())
}

/// After a commit, append what the database holds past each file's last line.
/// It runs under its own writer lease, so no other writer appends between the
/// read and the write. Any failure is left for the next open, which appends
/// the same lines or refuses a file that disagrees.
pub(crate) fn mirror(connection: &mut Connection, data: Option<&Path>, ids: &[&str]) {
    let Some(data) = data else {
        return;
    };
    let Ok(tx) = connection.transaction_with_behavior(TransactionBehavior::Immediate) else {
        return;
    };
    for id in ids {
        for log in Log::ALL {
            let _ = mirror_one(&tx, data, id, log);
        }
    }
    let _ = tx.commit();
}

/// Every project with events or discoveries, for writers that touch many.
pub(crate) fn mirror_all(connection: &mut Connection, data: Option<&Path>) {
    if data.is_none() {
        return;
    }
    let Ok(ids) = connection
        .prepare("SELECT project_id FROM project_events UNION SELECT project_id FROM discoveries")
        .and_then(|mut s| {
            s.query_map([], |r| r.get::<_, String>(0))?
                .collect::<std::result::Result<Vec<_>, _>>()
        })
    else {
        return;
    };
    let ids: Vec<&str> = ids.iter().map(String::as_str).collect();
    mirror(connection, data, &ids);
}

/// What the open decided for one log of one project.
pub(crate) struct Plan {
    project_id: String,
    log: Log,
    import: Vec<Line>,
    rewrite: Option<Vec<u8>>,
}

/// Decide every log after the project index has been applied in `tx`, so
/// imported lines can name projects the project files just brought back.
/// Every refusal happens here, before any row or file changes.
pub(crate) fn plan(tx: &Transaction<'_>, data: &Path) -> Result<Vec<Plan>> {
    let root = data.join(PROJECT_RECORDS);
    let mut files: BTreeMap<(String, Log), FileLog> = BTreeMap::new();
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
            for log in Log::ALL {
                if let Some(file) = read_file(data, id, log)? {
                    files.insert((id.to_owned(), log), file);
                }
            }
        }
    }
    let known: BTreeSet<String> = tx
        .prepare("SELECT id FROM projects")?
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<std::result::Result<_, _>>()?;
    let mut keys: BTreeSet<(String, Log)> = files.keys().cloned().collect();
    for id in &known {
        for log in Log::ALL {
            keys.insert((id.clone(), log));
        }
    }
    let mut plans = vec![];
    for (id, log) in keys {
        let path = folder(data, &id)?.join(log.file());
        let rows = database(tx, &id, log)?;
        let file = files.remove(&(id.clone(), log));
        let (import, rewrite) = match file {
            None if rows.is_empty() => continue,
            None => (vec![], Some(render(&rows))),
            Some(file) => {
                if !known.contains(&id) {
                    return Err(refused(
                        &path,
                        format!(
                            "{} file names a project absent from registrations",
                            log.noun()
                        ),
                    ));
                }
                let shared = rows.len().min(file.lines.len());
                if file.lines[..shared] != rows[..shared] {
                    return Err(refused(
                        &path,
                        format!("{} file and database disagree", log.noun()),
                    ));
                }
                if file.lines.len() > rows.len() {
                    let import = file.lines[rows.len()..].to_vec();
                    (import, file.torn.then(|| render(&file.lines)))
                } else if file.lines.len() < rows.len() || file.torn {
                    (vec![], Some(render(&rows)))
                } else {
                    continue;
                }
            }
        };
        if rewrite.is_some() {
            writable(data, &id, log)?;
        }
        plans.push(Plan {
            project_id: id,
            log,
            import,
            rewrite,
        });
    }
    Ok(plans)
}

pub(crate) fn apply(tx: &Transaction<'_>, plans: &[Plan]) -> Result<()> {
    for plan in plans {
        for line in &plan.import {
            match plan.log {
                Log::Events => {
                    let sequence: u64 = line.key.parse().unwrap_or_default();
                    tx.execute(
                        "INSERT INTO project_events(project_id,sequence,event_id,envelope) VALUES (?1,?2,?3,?4)",
                        params![
                            plan.project_id,
                            sequence,
                            format!("{}:{sequence}", project_stream_id(&plan.project_id)),
                            line.text
                        ],
                    )?;
                }
                Log::Discoveries => {
                    tx.execute(
                        "INSERT INTO discoveries(id,project_id,created_at,payload) VALUES (?1,?2,?3,?4)",
                        params![line.key, plan.project_id, line.created_at, line.text],
                    )?;
                }
            }
        }
    }
    Ok(())
}

pub(crate) fn backfill(data: &Path, plans: &[Plan]) -> Result<()> {
    for plan in plans {
        if let Some(bytes) = &plan.rewrite {
            write(data, &plan.project_id, plan.log, bytes)?;
        }
    }
    Ok(())
}
