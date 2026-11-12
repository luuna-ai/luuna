//! The `.luuna/missions/` store: JSONL files, atomic replace and lock files
//! compatible with the Go tool's (`internal/store/store.go`).
//!
//! The location is a parameter: [`Store::open`] takes the store directory
//! itself, and [`Store::discover`] walks up from a working directory to the
//! first `.luuna/missions/config.yaml`, as Go's `FindDir` does.
//!
//! The format layer follows Go exactly, including behaviour that looks like a
//! defect (see `UPSTREAM.md`):
//!
//! - Reading splits the file on `\n`, trims Unicode white space, skips blank
//!   lines and silently skips lines Go cannot decode as a record (SD2). A
//!   later record with the same id replaces the earlier one in the earlier
//!   one's position, and records without an id all share the id `""` (SD1).
//!   Reads take no lock.
//! - Writing rewrites the whole file through an atomic replace, so lines that
//!   were skipped on read are dropped (SD2). Appending reads the file, adds a
//!   missing final newline, appends the record and replaces the file, so it
//!   keeps every existing line byte for byte.
//! - An atomic replace writes `<file>.tmp.<random>` in the same directory,
//!   gives it the old file's permission bits (a file that did not exist keeps
//!   the temporary file's 0600, SD3), and renames it over the file, without
//!   fsync (SD3).
//! - A lock is `<file>.lock`, created exclusively, empty, mode 0600, and
//!   removed on release. A taker retries every 100 ms (plus at most 65.5 µs
//!   of jitter, SD18) for up to 30 s, and reclaims a lock older than 30 s by
//!   renaming it to `<file>.lock.stale.<16 hex>` and removing that only if it
//!   is still the same file with the same modification time.
//!
//! Discovery follows Go's `FindDir` (see [`Store::discover`]).

use std::collections::HashMap;
use std::ffi::OsString;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use crate::config::{self, ProcessEnv};
use crate::error::{Error, Result};
use crate::model::{self, Ticket};

/// A lock older than this is stale and may be reclaimed (`lockStale`).
const LOCK_STALE: Duration = Duration::from_secs(30);
/// How long a taker waits for a lock (`lockTimeout`).
const LOCK_TIMEOUT: Duration = Duration::from_secs(30);
/// The pause between attempts, before jitter (`lockRetry`).
const LOCK_RETRY: Duration = Duration::from_millis(100);

/// A data file of the store, each guarded by its own `<file>.lock`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DataFile {
    /// `issues.jsonl`
    Issues,
    /// `plans.jsonl`
    Plans,
    /// `templates.jsonl`
    Templates,
    /// `config.yaml`
    Config,
}

impl DataFile {
    /// The file name inside the store directory.
    pub fn file_name(self) -> &'static str {
        match self {
            Self::Issues => "issues.jsonl",
            Self::Plans => "plans.jsonl",
            Self::Templates => "templates.jsonl",
            Self::Config => "config.yaml",
        }
    }
}

/// A `.luuna/missions` store directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Store {
    dir: PathBuf,
}

impl Store {
    /// The store whose directory is `dir` (the `.luuna/missions` directory
    /// itself). Nothing is read until a record or lock is asked for; a data
    /// file that does not exist reads as empty, as in Go.
    pub fn open(dir: impl Into<PathBuf>) -> Result<Self> {
        Ok(Self { dir: dir.into() })
    }

    /// The store found by walking up from `cwd` to the first
    /// `.luuna/missions/config.yaml`, as Go's `FindDir` does. A relative
    /// `cwd` is taken from the process's working directory.
    ///
    /// From a linked git worktree, the main checkout's store is returned when
    /// it is initialised; this runs `git rev-parse --git-common-dir` with the
    /// process's environment, as Go does. Reaching the file system root
    /// returns [`Error::NotInProject`]. A legacy `.seeds` folder that Go
    /// would migrate (or refuse) on the way returns
    /// [`Error::Unsupported`] until s4-missions-lib-3.
    pub fn discover(cwd: &Path) -> Result<Self> {
        Self::discover_with(cwd, ProcessEnv::Inherit)
    }

    /// [`Store::discover`] with git run in `env`.
    pub(crate) fn discover_with(cwd: &Path, env: ProcessEnv<'_>) -> Result<Self> {
        Self::open(config::find_dir(cwd, env)?)
    }

    /// The store directory.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The path of `file` in the store.
    pub fn path(&self, file: DataFile) -> PathBuf {
        self.dir.join(file.file_name())
    }

    /// The tickets in `issues.jsonl`, as Go's `ReadIssues` reads them. Takes
    /// no lock.
    pub fn read_tickets(&self) -> Result<Vec<Ticket>> {
        read_records(&self.path(DataFile::Issues))
    }

    /// Replaces `issues.jsonl` with `tickets`, as Go's `WriteIssues` does.
    /// Lines that were skipped on read are dropped (SD2). The caller must
    /// hold the lock of [`DataFile::Issues`].
    pub fn write_tickets(&self, tickets: &[Ticket]) -> Result<()> {
        write_records(&self.path(DataFile::Issues), tickets)
    }

    /// Appends `ticket` to `issues.jsonl`, as Go's `AppendIssue` does,
    /// keeping every existing line. The caller must hold the lock of
    /// [`DataFile::Issues`].
    pub fn append_ticket(&self, ticket: &Ticket) -> Result<()> {
        append_record(&self.path(DataFile::Issues), ticket)
    }

    /// Takes the lock of `file` with Go's protocol: `<file>.lock` created
    /// exclusively, waiting up to 30 s and reclaiming a lock older than 30 s.
    /// The lock is released when the returned guard is dropped. Go takes
    /// `plans.jsonl` before `issues.jsonl` when it needs both.
    pub fn lock(&self, file: DataFile) -> Result<StoreLock> {
        acquire(&self.path(file), LOCK_TIMEOUT)
    }
}

/// A held store lock; dropping it removes the lock file.
#[derive(Debug)]
pub struct StoreLock {
    path: PathBuf,
}

impl StoreLock {
    /// The lock file.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for StoreLock {
    fn drop(&mut self) {
        // Go ignores the result of removing its lock, too.
        let _ = fs::remove_file(&self.path);
    }
}

/// A record kind stored one JSON object per line.
pub(crate) trait Record: Sized {
    /// Decodes a trimmed, non-empty line; `None` where Go skips the line.
    fn from_line(line: &[u8]) -> Option<Self>;
    /// The id records are deduplicated by.
    fn record_id(&self) -> &str;
    /// The line Go writes, without the newline.
    fn to_line(&self) -> Result<Vec<u8>>;
}

impl Record for Ticket {
    fn from_line(line: &[u8]) -> Option<Self> {
        Ticket::from_json(line)
    }

    fn record_id(&self) -> &str {
        self.id()
    }

    fn to_line(&self) -> Result<Vec<u8>> {
        self.to_json_line()
    }
}

/// Go's `readRecords`.
pub(crate) fn read_records<R: Record>(path: &Path) -> Result<Vec<R>> {
    let content = match fs::read(path) {
        Ok(content) => content,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(Error::io("read", path, error)),
    };
    let mut items: Vec<R> = Vec::new();
    let mut positions: HashMap<String, usize> = HashMap::new();
    for line in content.split(|byte| *byte == b'\n') {
        let line = trim_space(line);
        if line.is_empty() {
            continue;
        }
        let Some(item) = R::from_line(line) else {
            continue;
        };
        match positions.get(item.record_id()) {
            Some(&position) => items[position] = item,
            None => {
                positions.insert(item.record_id().to_owned(), items.len());
                items.push(item);
            }
        }
    }
    Ok(items)
}

/// Go's `writeRecords`.
pub(crate) fn write_records<R: Record>(path: &Path, items: &[R]) -> Result<()> {
    let mut out = Vec::new();
    for item in items {
        out.extend_from_slice(&item.to_line()?);
        out.push(b'\n');
    }
    atomic_replace(path, &out)
}

/// Go's `appendRecord`.
pub(crate) fn append_record<R: Record>(path: &Path, item: &R) -> Result<()> {
    let mut content = match fs::read(path) {
        Ok(content) => content,
        Err(error) if error.kind() == io::ErrorKind::NotFound => Vec::new(),
        Err(error) => return Err(Error::io("read", path, error)),
    };
    let encoded = item.to_line()?;
    if content.last().is_some_and(|byte| *byte != b'\n') {
        content.push(b'\n');
    }
    content.extend_from_slice(&encoded);
    content.push(b'\n');
    atomic_replace(path, &content)
}

/// Go's `bytes.TrimSpace`: leading and trailing Unicode white space; an
/// invalid UTF-8 byte stops the trim.
pub(crate) fn trim_space(mut bytes: &[u8]) -> &[u8] {
    while !bytes.is_empty() {
        let (ch, width) = crate::json::decode_char(bytes);
        if !ch.is_some_and(char::is_whitespace) {
            break;
        }
        bytes = &bytes[width..];
    }
    'outer: while !bytes.is_empty() {
        for width in 1..=4.min(bytes.len()) {
            let tail = &bytes[bytes.len() - width..];
            if let (Some(ch), used) = crate::json::decode_char(tail) {
                if used == width {
                    if ch.is_whitespace() {
                        bytes = &bytes[..bytes.len() - width];
                        continue 'outer;
                    }
                    break 'outer;
                }
            }
        }
        break;
    }
    bytes
}

/// Removes a path when dropped (Go's deferred `os.Remove(tmpPath)`).
struct RemoveOnDrop(PathBuf);

impl Drop for RemoveOnDrop {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

/// Go's `atomicReplace`: a temporary file in the same directory, the old
/// file's permission bits, and a rename over the file, with no fsync (SD3).
pub(crate) fn atomic_replace(path: &Path, content: &[u8]) -> Result<()> {
    let dir = match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    };
    let previous = match fs::metadata(path) {
        Ok(meta) => Some(meta),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(Error::io("stat", path, error)),
    };
    let base = path.file_name().unwrap_or_default();
    let (tmp_path, mut file) = create_temp(dir, base)?;
    let _cleanup = RemoveOnDrop(tmp_path.clone());
    file.write_all(content)
        .map_err(|error| Error::io("write", &tmp_path, error))?;
    if let Some(previous) = previous {
        file.set_permissions(permission_bits(&previous))
            .map_err(|error| Error::io("chmod", &tmp_path, error))?;
    }
    drop(file);
    fs::rename(&tmp_path, path).map_err(|error| Error::io("rename", &tmp_path, error))
}

#[cfg(unix)]
fn permission_bits(meta: &fs::Metadata) -> fs::Permissions {
    use std::os::unix::fs::PermissionsExt;
    fs::Permissions::from_mode(meta.permissions().mode() & 0o777)
}

#[cfg(not(unix))]
fn permission_bits(meta: &fs::Metadata) -> fs::Permissions {
    meta.permissions()
}

/// Go's `os.CreateTemp(dir, base + ".tmp.*")`: a random decimal number in
/// place of `*`, created exclusively with mode 0600, up to 10000 attempts.
fn create_temp(dir: &Path, base: &std::ffi::OsStr) -> Result<(PathBuf, fs::File)> {
    let mut last = None;
    for _ in 0..10_000 {
        let mut random = [0; 4];
        model::random_bytes(&mut random);
        let mut name = OsString::from(base);
        name.push(format!(".tmp.{}", u32::from_be_bytes(random)));
        let path = dir.join(name);
        match create_exclusive(&path, true) {
            Ok(file) => return Ok((path, file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                last = Some((path, error))
            }
            Err(error) => return Err(Error::io("create", path, error)),
        }
    }
    let (path, error) = last.expect("at least one attempt was made");
    Err(Error::io("create", path, error))
}

/// Opens `path` with `O_CREATE|O_EXCL` and mode 0600.
fn create_exclusive(path: &Path, read: bool) -> io::Result<fs::File> {
    let mut options = fs::OpenOptions::new();
    options.read(read).write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    options.open(path)
}

/// `<data_path>.lock`.
fn lock_path(data_path: &Path) -> PathBuf {
    let mut name = data_path.as_os_str().to_owned();
    name.push(".lock");
    PathBuf::from(name)
}

/// Go's `WithLock` up to the call of its function.
fn acquire(data_path: &Path, timeout: Duration) -> Result<StoreLock> {
    let lock = lock_path(data_path);
    let deadline = Instant::now() + timeout;
    loop {
        match create_exclusive(&lock, false) {
            Ok(file) => {
                drop(file);
                return Ok(StoreLock { path: lock });
            }
            Err(error) if error.kind() != io::ErrorKind::AlreadyExists => {
                // Windows may deny access to an existing exclusive lock; that
                // is contention only while the lock path exists.
                if let Err(stat) = fs::symlink_metadata(&lock) {
                    if stat.kind() == io::ErrorKind::NotFound
                        && error.kind() == io::ErrorKind::PermissionDenied
                    {
                        continue;
                    }
                    return Err(Error::io("open", &lock, error));
                }
            }
            Err(_) => {}
        }
        if let Ok(meta) = fs::metadata(&lock) {
            let age = meta
                .modified()
                .ok()
                .and_then(|modified| SystemTime::now().duration_since(modified).ok());
            if age.is_some_and(|age| age > LOCK_STALE) {
                reclaim_stale_lock(&lock, &meta);
                continue;
            }
        }
        if Instant::now() > deadline {
            return Err(Error::LockTimeout {
                data_path: data_path.to_path_buf(),
            });
        }
        thread::sleep(LOCK_RETRY + jitter());
    }
}

/// Go's `randomJitter(lockRetry)`: two random bytes taken as nanoseconds,
/// so at most 65.535 µs rather than up to 100 ms (SD18).
fn jitter() -> Duration {
    let mut random = [0; 2];
    model::random_bytes(&mut random);
    let nanos = u64::from(u16::from_be_bytes(random));
    Duration::from_nanos(nanos % LOCK_RETRY.as_nanos() as u64)
}

/// `<lock>.stale.<16 hex>` from 8 random bytes.
fn stale_sidecar(lock: &Path) -> PathBuf {
    let mut random = [0; 8];
    model::random_bytes(&mut random);
    let mut name = lock.as_os_str().to_owned();
    name.push(".stale.");
    name.push(model::hex(&random));
    PathBuf::from(name)
}

/// Go's `ReclaimStaleLock`: moves the lock aside and removes it only if it is
/// the entry that was observed; otherwise moves it back.
fn reclaim_stale_lock(lock: &Path, observed: &fs::Metadata) -> bool {
    reclaim_into(lock, observed, &stale_sidecar(lock))
}

fn reclaim_into(lock: &Path, observed: &fs::Metadata, sidecar: &Path) -> bool {
    if fs::rename(lock, sidecar).is_err() {
        return false;
    }
    let moved = match fs::metadata(sidecar) {
        Ok(meta) => meta,
        Err(_) => {
            let _ = fs::remove_file(sidecar);
            return false;
        }
    };
    if same_file(observed, &moved) && observed.modified().ok() == moved.modified().ok() {
        return fs::remove_file(sidecar).is_ok();
    }
    if fs::rename(sidecar, lock).is_err() {
        let _ = fs::remove_file(sidecar);
    }
    false
}

#[cfg(unix)]
fn same_file(a: &fs::Metadata, b: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    a.dev() == b.dev() && a.ino() == b.ino()
}

/// Go compares size and mode where a rename may change the file identity.
#[cfg(not(unix))]
fn same_file(a: &fs::Metadata, b: &fs::Metadata) -> bool {
    a.len() == b.len() && a.permissions() == b.permissions()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    fn mode(path: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    fn record(id: &str, title: &str) -> String {
        format!(
            r#"{{"id":"{id}","title":"{title}","status":"open","type":"task","priority":2,"createdAt":"c","updatedAt":"u"}}"#
        )
    }

    fn store() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        (dir, store)
    }

    fn leftovers(dir: &Path) -> Vec<String> {
        fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| name.contains(".tmp") || name.contains(".lock"))
            .collect()
    }

    #[test]
    fn data_files_keep_the_go_names() {
        assert_eq!(DataFile::Issues.file_name(), "issues.jsonl");
        assert_eq!(DataFile::Plans.file_name(), "plans.jsonl");
        assert_eq!(DataFile::Templates.file_name(), "templates.jsonl");
        assert_eq!(DataFile::Config.file_name(), "config.yaml");
    }

    #[test]
    fn a_missing_file_reads_as_empty() {
        let (_dir, store) = store();
        assert!(store.read_tickets().unwrap().is_empty());
    }

    #[test]
    fn the_last_record_for_an_id_wins_at_the_first_position() {
        let (_dir, store) = store();
        let content = [
            record("a", "first"),
            record("b", "second"),
            record("a", "last"),
        ]
        .join("\n");
        fs::write(store.path(DataFile::Issues), content).unwrap();
        let tickets = store.read_tickets().unwrap();
        let seen: Vec<_> = tickets.iter().map(|t| (t.id(), t.title.as_str())).collect();
        assert_eq!(seen, [("a", "last"), ("b", "second")]);
    }

    #[test]
    fn records_without_an_id_collapse_into_one_slot() {
        // SD1: every record without an id decodes with id "".
        let (_dir, store) = store();
        let content = [
            r#"{"title":"no id one"}"#.to_owned(),
            record("a", "A"),
            r#"{"title":"no id two"}"#.to_owned(),
            r#"{"id":5,"title":"numeric id"}"#.to_owned(),
        ]
        .join("\n");
        fs::write(store.path(DataFile::Issues), content).unwrap();
        let tickets = store.read_tickets().unwrap();
        let seen: Vec<_> = tickets.iter().map(|t| (t.id(), t.title.as_str())).collect();
        assert_eq!(seen, [("", "numeric id"), ("a", "A")]);
    }

    #[test]
    fn malformed_and_blank_lines_are_skipped_on_read() {
        let (_dir, store) = store();
        let content = format!(
            "\n   \n{}\n{{not json\n[1]\nnull\n\t{} \r\n\u{2028}{}\u{3000}\n",
            record("a", "A"),
            record("b", "B"),
            record("c", "C")
        );
        fs::write(store.path(DataFile::Issues), content).unwrap();
        let ids: Vec<_> = store
            .read_tickets()
            .unwrap()
            .iter()
            .map(|t| t.id().to_owned())
            .collect();
        assert_eq!(ids, ["a", "b", "c"]);
    }

    #[test]
    fn trimming_follows_go_trim_space() {
        assert_eq!(trim_space(b" \t\r\n x \x0b\x0c"), b"x");
        assert_eq!(trim_space("\u{a0}\u{85}x\u{2029}\u{3000}".as_bytes()), b"x");
        assert_eq!(trim_space(b"\xff x \xff"), b"\xff x \xff");
        assert_eq!(trim_space(b" \xe2\x80"), b"\xe2\x80");
        assert_eq!(trim_space(b"   "), b"");
    }

    #[test]
    fn append_adds_a_missing_newline_and_keeps_malformed_lines() {
        let (dir, store) = store();
        let path = store.path(DataFile::Issues);
        let existing = format!("{}\n{{not json\n  {}", record("a", "A"), record("b", "B"));
        fs::write(&path, &existing).unwrap();
        let ticket = Ticket::from_json(record("c", "C").as_bytes()).unwrap();
        store.append_ticket(&ticket).unwrap();
        let after = fs::read_to_string(&path).unwrap();
        assert_eq!(after, format!("{existing}\n{}\n", record("c", "C")));
        assert!(leftovers(dir.path()).is_empty());
    }

    #[test]
    fn append_creates_a_missing_file() {
        let (_dir, store) = store();
        let ticket = Ticket::from_json(record("a", "A").as_bytes()).unwrap();
        store.append_ticket(&ticket).unwrap();
        let after = fs::read_to_string(store.path(DataFile::Issues)).unwrap();
        assert_eq!(after, format!("{}\n", record("a", "A")));
    }

    #[test]
    fn a_full_rewrite_drops_malformed_lines() {
        // SD2: the rewrite keeps only what was read.
        let (_dir, store) = store();
        let path = store.path(DataFile::Issues);
        fs::write(
            &path,
            format!("{}\n{{not json\n\n{}\n", record("a", "A"), record("b", "B")),
        )
        .unwrap();
        let tickets = store.read_tickets().unwrap();
        store.write_tickets(&tickets).unwrap();
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            format!("{}\n{}\n", record("a", "A"), record("b", "B"))
        );
        store.write_tickets(&[]).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"");
    }

    #[cfg(unix)]
    #[test]
    fn atomic_replace_keeps_the_mode_and_leaves_no_temporary_file() {
        use std::os::unix::fs::PermissionsExt;
        let (dir, store) = store();
        let path = store.path(DataFile::Issues);
        fs::write(&path, b"old\n").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
        atomic_replace(&path, b"new\n").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"new\n");
        assert_eq!(mode(&path), 0o640);
        assert!(leftovers(dir.path()).is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn atomic_replace_gives_a_new_file_mode_0600() {
        // SD3: Go's CreateTemp mode stays on a file that did not exist.
        let (dir, store) = store();
        let path = store.path(DataFile::Plans);
        atomic_replace(&path, b"x\n").unwrap();
        assert_eq!(mode(&path), 0o600);
        assert!(leftovers(dir.path()).is_empty());
    }

    #[test]
    fn a_lock_file_is_created_exclusively_empty_and_removed_on_release() {
        let (dir, store) = store();
        let lock = store.lock(DataFile::Issues).unwrap();
        assert_eq!(lock.path(), dir.path().join("issues.jsonl.lock"));
        assert_eq!(fs::metadata(lock.path()).unwrap().len(), 0);
        #[cfg(unix)]
        assert_eq!(mode(lock.path()), 0o600);
        // Held: another taker times out with Go's message.
        let error = acquire(&store.path(DataFile::Issues), Duration::from_millis(250)).unwrap_err();
        assert_eq!(
            error.to_string(),
            format!(
                "Timeout acquiring lock for {}",
                store.path(DataFile::Issues).display()
            )
        );
        let path = lock.path().to_path_buf();
        drop(lock);
        assert!(!path.exists());
        // Released: the next taker gets it.
        let again = store.lock(DataFile::Issues).unwrap();
        drop(again);
        assert!(leftovers(dir.path()).is_empty());
    }

    #[test]
    fn a_waiting_taker_gets_the_lock_once_it_is_removed() {
        let (_dir, store) = store();
        let lock = store.lock(DataFile::Issues).unwrap();
        let data = store.path(DataFile::Issues);
        let waiter = thread::spawn(move || acquire(&data, Duration::from_secs(10)).map(drop));
        thread::sleep(Duration::from_millis(300));
        assert!(!waiter.is_finished());
        drop(lock);
        waiter.join().unwrap().unwrap();
    }

    #[test]
    fn a_lock_older_than_30_s_is_reclaimed() {
        let (dir, store) = store();
        let lock = lock_path(&store.path(DataFile::Issues));
        create_exclusive(&lock, false).unwrap();
        let file = fs::File::options().write(true).open(&lock).unwrap();
        file.set_modified(SystemTime::now() - Duration::from_secs(60))
            .unwrap();
        drop(file);
        let started = Instant::now();
        let held = store.lock(DataFile::Issues).unwrap();
        assert!(started.elapsed() < Duration::from_secs(5));
        assert_eq!(leftovers(dir.path()), ["issues.jsonl.lock"]);
        drop(held);
        assert!(leftovers(dir.path()).is_empty());
    }

    #[test]
    fn a_fresh_lock_is_not_reclaimed() {
        let (_dir, store) = store();
        let lock = lock_path(&store.path(DataFile::Issues));
        create_exclusive(&lock, false).unwrap();
        assert!(acquire(&store.path(DataFile::Issues), Duration::from_millis(200)).is_err());
        assert!(lock.exists());
    }

    #[test]
    fn reclaim_goes_through_a_stale_sidecar() {
        let (dir, store) = store();
        let lock = lock_path(&store.path(DataFile::Issues));
        let sidecar = stale_sidecar(&lock);
        let name = sidecar.file_name().unwrap().to_string_lossy().into_owned();
        let suffix = name.strip_prefix("issues.jsonl.lock.stale.").unwrap();
        assert_eq!(suffix.len(), 16);
        assert!(suffix
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')));

        // The observed lock is the one moved aside: it is removed.
        create_exclusive(&lock, false).unwrap();
        let observed = fs::metadata(&lock).unwrap();
        assert!(reclaim_into(&lock, &observed, &sidecar));
        assert!(!lock.exists() && !sidecar.exists());

        // Another process's newer lock is moved back, not removed.
        create_exclusive(&lock, false).unwrap();
        let observed = fs::metadata(&lock).unwrap();
        fs::remove_file(&lock).unwrap();
        fs::write(&lock, b"newer").unwrap();
        assert!(!reclaim_into(&lock, &observed, &sidecar));
        assert_eq!(fs::read(&lock).unwrap(), b"newer");
        assert!(!sidecar.exists());
        fs::remove_file(&lock).unwrap();
        assert!(leftovers(dir.path()).is_empty());
    }

    #[test]
    fn jitter_stays_below_66_microseconds() {
        for _ in 0..100 {
            assert!(jitter() < Duration::from_nanos(65_536));
        }
    }
}
