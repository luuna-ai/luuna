//! JSONL files, locks and atomic rewrites, interoperable with the Go tool on
//! the same store (`internal/store/store.go`).
//!
//! - Reading takes no lock. A missing file is empty; lines that are blank
//!   after trimming white space are skipped; the first line that does not
//!   decode fails the whole read with its 1-based line number.
//! - Appending takes the sibling lock `<file>.lock`, created exclusively
//!   (mode 0600), retried every 50 ms for up to 5 s; a lock older than 30 s
//!   is removed and the attempt repeated at once. Under the lock the whole
//!   file is rewritten as its existing bytes, the new line and `\n`, through
//!   a temporary file in the same folder that is synced and then renamed
//!   over the target. The existing bytes are kept as they are, so a last
//!   line without a newline is glued to the new record, as Go does (GD1).
//!   The lock is removed afterwards, whatever the outcome.

use crate::error::BeaconError;
use crate::model::Record;
use std::fs;
use std::io::{self, Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

const LOCK_STALE: Duration = Duration::from_secs(30);
const LOCK_RETRY: Duration = Duration::from_millis(50);
const LOCK_TIMEOUT: Duration = Duration::from_secs(5);

/// A held `<file>.lock`; dropping it removes the lock file (errors ignored,
/// as Go's `Release`).
pub(crate) struct Lock {
    path: PathBuf,
}

impl Drop for Lock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn lock_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".lock");
    PathBuf::from(name)
}

/// Go's `store.Acquire(path)`: takes `<path>.lock`.
pub(crate) fn acquire(path: &Path) -> Result<Lock, BeaconError> {
    let lock = lock_path(path);
    let deadline = Instant::now() + LOCK_TIMEOUT;
    while Instant::now() < deadline {
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(&lock) {
            Ok(file) => {
                drop(file);
                return Ok(Lock { path: lock });
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(source) => return Err(BeaconError::AcquireLock { lock, source }),
        }
        // A lock last modified more than 30 s ago (by the wall clock, as Go's
        // time.Since(ModTime)) is stale: remove it and try again at once.
        let stale = fs::metadata(&lock)
            .and_then(|info| info.modified())
            .ok()
            .and_then(|modified| SystemTime::now().duration_since(modified).ok())
            .is_some_and(|age| age > LOCK_STALE);
        if stale {
            let _ = fs::remove_file(&lock);
            continue;
        }
        thread::sleep(LOCK_RETRY);
    }
    Err(BeaconError::LockTimeout {
        path: path.to_path_buf(),
    })
}

/// Go's `store.AtomicWrite(path, content)`: a temporary file `.tmp.*` in the
/// same folder (mode 0600), written, synced and closed, then renamed over
/// `path`. The temporary file is removed on any failure.
pub(crate) fn atomic_write(path: &Path, content: &[u8]) -> Result<(), BeaconError> {
    let directory = match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    };
    let temp = tempfile::Builder::new()
        .prefix(".tmp.")
        .tempfile_in(directory)
        .map_err(|source| BeaconError::CreateTemp {
            pattern: directory.join(".tmp.*"),
            source,
        })?;
    let (mut file, temp_path) = temp.into_parts();
    let written = file
        .write_all(content)
        .map_err(|source| ("write", source))
        .and_then(|()| file.sync_all().map_err(|source| ("sync", source)));
    drop(file);
    if let Err((op, source)) = written {
        return Err(BeaconError::WriteTemp {
            op,
            path: temp_path.to_path_buf(),
            source,
        });
    }
    let from = temp_path.to_path_buf();
    temp_path
        .persist(path)
        .map_err(|error| BeaconError::Replace {
            path: path.to_path_buf(),
            from,
            source: error.error,
        })
}

/// Go's `os.ReadFile`: `None` when the file does not exist.
fn read_existing(path: &Path) -> Result<Option<Vec<u8>>, BeaconError> {
    let mut file = match fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(BeaconError::os("open", path, error)),
    };
    let mut content = Vec::new();
    file.read_to_end(&mut content)
        .map_err(|error| BeaconError::os("read", path, error))?;
    Ok(Some(content))
}

/// Go's `strings.TrimSpace(line) == ""`. A line that is not valid UTF-8
/// cannot be all white space.
fn is_blank(line: &[u8]) -> bool {
    std::str::from_utf8(line).is_ok_and(|text| text.trim().is_empty())
}

/// Go's `store.ReadJSONL[T](path)`.
pub(crate) fn read_jsonl<T: Record>(path: &Path) -> Result<Vec<T>, BeaconError> {
    let Some(content) = read_existing(path)? else {
        return Ok(Vec::new());
    };
    let mut items = Vec::new();
    for (index, line) in content.split(|byte| *byte == b'\n').enumerate() {
        if is_blank(line) {
            continue;
        }
        let item = T::from_go_json(line).map_err(|message| BeaconError::InvalidJsonl {
            line: index + 1,
            path: path.to_path_buf(),
            message,
        })?;
        items.push(item);
    }
    Ok(items)
}

/// Go's `store.AppendJSONL(path, item)`.
pub(crate) fn append_jsonl<T: Record>(path: &Path, item: &T) -> Result<(), BeaconError> {
    let _lock = acquire(path)?;
    let encoded = item.to_go_json();
    let mut content = read_existing(path)?.unwrap_or_default();
    content.extend_from_slice(encoded.as_bytes());
    content.push(b'\n');
    atomic_write(path, &content)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Prompt, Schema};

    fn store() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().expect("temporary folder");
        let path = dir.path().canonicalize().expect("canonical path");
        (dir, path)
    }

    fn schema(id: &str) -> Schema {
        Schema {
            id: id.to_owned(),
            name: format!("name-{id}"),
            required_sections: Some(Vec::new()),
            created_at: "2024-01-01T00:00:00Z".to_owned(),
            updated_at: "2024-01-01T00:00:00Z".to_owned(),
            ..Schema::default()
        }
    }

    fn names(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    fn age(path: &Path, seconds: u64) {
        let file = fs::File::options().write(true).open(path).unwrap();
        file.set_modified(SystemTime::now() - Duration::from_secs(seconds))
            .unwrap();
    }

    #[test]
    fn append_preserves_existing_bytes_and_adds_one_line() {
        let (_dir, dir) = store();
        let path = dir.join("schemas.jsonl");
        // A missing file is created with one line.
        append_jsonl(&path, &schema("a")).unwrap();
        let first = format!("{}\n", schema("a").to_go_json());
        assert_eq!(fs::read_to_string(&path).unwrap(), first);

        // Existing bytes are kept exactly, including blank lines, CRLF and
        // bytes that are not UTF-8.
        let odd = b"\n  \r\n{\"id\":\"b\",\"requiredSections\":null}\r\n\t\n\xff\xfe junk kept\n";
        let mut existing = first.into_bytes();
        existing.extend_from_slice(odd);
        fs::write(&path, &existing).unwrap();
        append_jsonl(&path, &schema("c")).unwrap();
        let mut expected = existing.clone();
        expected.extend_from_slice(schema("c").to_go_json().as_bytes());
        expected.push(b'\n');
        assert_eq!(fs::read(&path).unwrap(), expected);
        // No temporary or lock file is left behind.
        assert_eq!(names(&dir), vec!["schemas.jsonl"]);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            // The rewrite takes the temporary file's mode, 0600, as Go's
            // os.CreateTemp gives.
            let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }

        // GD1: an unterminated last line is glued to the new record, and the
        // file no longer reads.
        let path = dir.join("glued.jsonl");
        let line = schema("d").to_go_json();
        fs::write(&path, &line).unwrap();
        assert_eq!(read_jsonl::<Schema>(&path).unwrap(), vec![schema("d")]);
        append_jsonl(&path, &schema("e")).unwrap();
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            format!("{line}{}\n", schema("e").to_go_json())
        );
        let error = read_jsonl::<Schema>(&path).unwrap_err();
        assert!(
            error
                .to_string()
                .starts_with(&format!("invalid JSONL line 1 in {}: ", path.display())),
            "{error}"
        );
    }

    #[test]
    fn reads_like_go() {
        let (_dir, dir) = store();
        let path = dir.join("prompts.jsonl");
        assert!(read_jsonl::<Prompt>(&path).unwrap().is_empty());
        fs::write(&path, b"").unwrap();
        assert!(read_jsonl::<Prompt>(&path).unwrap().is_empty());
        fs::write(
            &path,
            b"\n   \n{\"name\":\"a\"}\r\n\t\n\xc2\xa0\n{\"name\":\"b\"}\n\n",
        )
        .unwrap();
        let read: Vec<String> = read_jsonl::<Prompt>(&path)
            .unwrap()
            .into_iter()
            .map(|prompt| prompt.name)
            .collect();
        assert_eq!(read, vec!["a", "b"]);
        fs::write(&path, b"{\"name\":\"a\"}\n\n{not json\n{\"name\":\"b\"}\n").unwrap();
        let error = read_jsonl::<Prompt>(&path).unwrap_err();
        assert!(error
            .to_string()
            .starts_with(&format!("invalid JSONL line 3 in {}: ", path.display())));
        // A folder in place of the file is an operating-system error.
        let folder = dir.join("folder.jsonl");
        fs::create_dir(&folder).unwrap();
        assert!(matches!(
            read_jsonl::<Prompt>(&folder),
            Err(BeaconError::Os { .. })
        ));
    }

    #[test]
    fn lock_times_out_after_five_seconds() {
        let (_dir, dir) = store();
        let path = dir.join("prompts.jsonl");
        fs::write(&path, b"").unwrap();
        let lock = dir.join("prompts.jsonl.lock");
        fs::write(&lock, b"held").unwrap();
        let started = Instant::now();
        let error = append_jsonl(&path, &schema("a")).unwrap_err();
        let waited = started.elapsed();
        assert_eq!(
            error.to_string(),
            format!("timeout acquiring lock on {} after 5000ms", path.display())
        );
        assert!(waited >= LOCK_TIMEOUT, "gave up after {waited:?}");
        assert!(waited < LOCK_TIMEOUT + Duration::from_secs(2), "{waited:?}");
        // The foreign lock is left alone and the file is unchanged.
        assert_eq!(fs::read(&lock).unwrap(), b"held");
        assert_eq!(fs::read(&path).unwrap(), b"");
    }

    #[test]
    fn stale_lock_is_replaced() {
        let (_dir, dir) = store();
        let path = dir.join("schemas.jsonl");
        let lock = dir.join("schemas.jsonl.lock");
        fs::write(&lock, b"").unwrap();
        age(&lock, 60);
        let started = Instant::now();
        append_jsonl(&path, &schema("a")).unwrap();
        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(!lock.exists(), "the stale lock is gone");
        assert_eq!(read_jsonl::<Schema>(&path).unwrap(), vec![schema("a")]);
        // A lock just under the limit is not stale: it is waited for.
        fs::write(&lock, b"").unwrap();
        age(&lock, 20);
        let waiter = {
            let path = path.clone();
            thread::spawn(move || append_jsonl(&path, &schema("b")))
        };
        thread::sleep(Duration::from_millis(300));
        assert!(lock.exists(), "a fresh lock is respected");
        fs::remove_file(&lock).unwrap();
        waiter.join().unwrap().unwrap();
        assert_eq!(read_jsonl::<Schema>(&path).unwrap().len(), 2);
    }

    #[test]
    fn lock_is_removed_after_a_write() {
        let (_dir, dir) = store();
        let path = dir.join("prompts.jsonl");
        append_jsonl(&path, &schema("a")).unwrap();
        append_jsonl(&path, &schema("b")).unwrap();
        assert_eq!(names(&dir), vec!["prompts.jsonl"]);
        // Also when the rewrite fails (here: the target is a folder).
        let folder = dir.join("folder.jsonl");
        fs::create_dir(&folder).unwrap();
        assert!(append_jsonl(&folder, &schema("c")).is_err());
        assert!(!dir.join("folder.jsonl.lock").exists());
        assert_eq!(names(&dir), vec!["folder.jsonl", "prompts.jsonl"]);
        // The lock is held while the rewrite happens, and only then.
        let lock = acquire(&path).unwrap();
        assert!(dir.join("prompts.jsonl.lock").exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(dir.join("prompts.jsonl.lock"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode & !0o600, 0, "lock mode {mode:o}");
        }
        drop(lock);
        assert!(!dir.join("prompts.jsonl.lock").exists());
    }
}
