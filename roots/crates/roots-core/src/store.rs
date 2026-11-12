//! JSONL persistence, compaction, atomic writes, and advisory lock files.

use crate::errors::{RootsError, RootsResult};
use crate::ids::random_hex;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant, SystemTime};

#[cfg(windows)]
use std::fs::File;
#[cfg(unix)]
use std::os::unix::fs::MetadataExt;
#[cfg(windows)]
use std::os::windows::io::AsRawHandle;

pub const LOCK_STALE_MS: u64 = 30_000;
pub const LOCK_RETRY_MS: u64 = 25;
pub const LOCK_TIMEOUT_MS: u64 = 30_000;

#[derive(Clone, Copy, Debug)]
pub struct LockOptions {
    pub stale_ms: u64,
    pub timeout_ms: u64,
}

impl Default for LockOptions {
    fn default() -> Self {
        Self {
            stale_ms: LOCK_STALE_MS,
            timeout_ms: LOCK_TIMEOUT_MS,
        }
    }
}

pub trait HasId {
    fn id(&self) -> &str;
}

fn io_error(error: impl ToString) -> RootsError {
    RootsError::error(error.to_string())
}

fn lock_path(file: &Path) -> std::path::PathBuf {
    std::path::PathBuf::from(format!("{}.lock", file.display()))
}

fn try_unlink(path: &Path) {
    let _ = fs::remove_file(path);
}

/// A platform-stable identity for one directory entry.
///
/// Unix uses `(st_dev, st_ino)`; Windows uses the volume serial number plus
/// `nFileIndex` returned by `GetFileInformationByHandle`.  Both values must
/// accompany an mtime before a stale lock may be deleted.
#[derive(Clone, Debug, Eq, PartialEq)]
struct FileIdentity {
    device_or_volume: u64,
    inode_or_file_index: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct LockObservation {
    mtime: SystemTime,
    identity: Option<FileIdentity>,
}

#[cfg(unix)]
fn lock_observation(path: &Path) -> Option<LockObservation> {
    let metadata = fs::metadata(path).ok()?;
    Some(LockObservation {
        mtime: metadata.modified().ok()?,
        identity: Some(FileIdentity {
            device_or_volume: metadata.dev(),
            inode_or_file_index: metadata.ino(),
        }),
    })
}

#[cfg(windows)]
fn lock_observation(path: &Path) -> Option<LockObservation> {
    // Read metadata from the opened handle rather than first statting the
    // pathname: a replacement between those calls must not splice the old
    // mtime together with the new file ID.
    let file = File::open(path).ok()?;
    let metadata = file.metadata().ok()?;
    Some(LockObservation {
        mtime: metadata.modified().ok()?,
        identity: windows_file_identity(&file),
    })
}

#[cfg(not(any(unix, windows)))]
fn lock_observation(path: &Path) -> Option<LockObservation> {
    let metadata = fs::metadata(path).ok()?;
    Some(LockObservation {
        mtime: metadata.modified().ok()?,
        // No stable file identity is exposed by std on this target.  The
        // reclaimer below treats this as a changed lock and restores it rather
        // than risking a timestamp-only deletion.
        identity: None,
    })
}

/// Return true only when both observations prove that this is the same lock.
/// A missing identity is deliberately not treated as equality: platforms that
/// cannot expose one choose restoration over deleting an ambiguous sidecar.
fn same_lock(seen: &LockObservation, actual: &LockObservation) -> bool {
    seen.mtime == actual.mtime
        && seen
            .identity
            .as_ref()
            .zip(actual.identity.as_ref())
            .is_some_and(|(seen, actual)| seen == actual)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ReclaimAction {
    DeleteSidecar,
    RestoreLock,
}

fn reclaim_action(seen: &LockObservation, actual: Option<&LockObservation>) -> ReclaimAction {
    match actual {
        Some(actual) if same_lock(seen, actual) => ReclaimAction::DeleteSidecar,
        Some(_) => ReclaimAction::RestoreLock,
        // Match the TypeScript best-effort catch branch: if the sidecar cannot
        // be inspected after rename, unlink it rather than leaving an
        // inaccessible stale sidecar behind.
        None => ReclaimAction::DeleteSidecar,
    }
}

fn reclaim_stale_lock(lock: &Path, seen: &LockObservation) {
    let sidecar = std::path::PathBuf::from(format!("{}.stale.{}", lock.display(), random_hex(12)));
    if fs::rename(lock, &sidecar).is_err() {
        return;
    }
    // The rename won only the pathname race. Re-read the sidecar before
    // deletion because a fresh lock may have replaced the stale one between
    // the observation above and this rename. A later creation can still race
    // the best-effort restore below, as it does in the TypeScript source.
    match reclaim_action(seen, lock_observation(&sidecar).as_ref()) {
        ReclaimAction::DeleteSidecar => try_unlink(&sidecar),
        ReclaimAction::RestoreLock => {
            let _ = fs::rename(&sidecar, lock);
        }
    }
}

fn reclaim_if_stale(lock: &Path, stale: Duration) -> bool {
    let Some(observed) = lock_observation(lock) else {
        return true;
    };
    if SystemTime::now()
        .duration_since(observed.mtime)
        .is_ok_and(|age| age <= stale)
    {
        return false;
    }
    reclaim_stale_lock(lock, &observed);
    true
}

#[cfg(windows)]
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct ByHandleFileInformation {
    file_attributes: u32,
    creation_time_low: u32,
    creation_time_high: u32,
    last_access_time_low: u32,
    last_access_time_high: u32,
    last_write_time_low: u32,
    last_write_time_high: u32,
    volume_serial_number: u32,
    file_size_high: u32,
    file_size_low: u32,
    number_of_links: u32,
    file_index_high: u32,
    file_index_low: u32,
}

#[cfg(windows)]
#[link(name = "Kernel32")]
unsafe extern "system" {
    #[link_name = "GetFileInformationByHandle"]
    fn get_file_information_by_handle(
        file: *mut std::ffi::c_void,
        information: *mut ByHandleFileInformation,
    ) -> i32;
}

#[cfg(windows)]
fn windows_file_identity(file: &File) -> Option<FileIdentity> {
    let mut information = ByHandleFileInformation::default();
    if unsafe { get_file_information_by_handle(file.as_raw_handle(), &mut information) } == 0 {
        return None;
    }
    Some(FileIdentity {
        device_or_volume: u64::from(information.volume_serial_number),
        inode_or_file_index: (u64::from(information.file_index_high) << 32)
            | u64::from(information.file_index_low),
    })
}

fn try_create_lock(lock: &Path) -> RootsResult<bool> {
    match OpenOptions::new().write(true).create_new(true).open(lock) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
        Err(error) => Err(io_error(error)),
    }
}

pub fn acquire_lock(file: impl AsRef<Path>, options: LockOptions) -> RootsResult<()> {
    let file = file.as_ref();
    if let Some(parent) = file.parent() {
        fs::create_dir_all(parent).map_err(io_error)?;
    }
    let lock = lock_path(file);
    let start = Instant::now();
    while !try_create_lock(&lock)? {
        if reclaim_if_stale(&lock, Duration::from_millis(options.stale_ms)) {
            continue;
        }
        if start.elapsed() > Duration::from_millis(options.timeout_ms) {
            return Err(RootsError::error(format!(
                "timeout acquiring lock for {}",
                file.display()
            )));
        }
        let jitter = u64::from(rand::random::<u8>()) % LOCK_RETRY_MS;
        thread::sleep(Duration::from_millis(LOCK_RETRY_MS + jitter));
    }
    Ok(())
}

pub fn release_lock(file: impl AsRef<Path>) {
    try_unlink(&lock_path(file.as_ref()));
}

pub fn with_lock<T, F>(file: impl AsRef<Path>, action: F, options: LockOptions) -> RootsResult<T>
where
    F: FnOnce() -> RootsResult<T>,
{
    let file = file.as_ref();
    acquire_lock(file, options)?;
    let result = action();
    release_lock(file);
    result
}

/// Write via a sibling temp file and rename, so readers never observe a
/// partial JSONL/YAML file.
pub fn atomic_write(file: impl AsRef<Path>, content: &str) -> RootsResult<()> {
    let file = file.as_ref();
    if let Some(parent) = file.parent() {
        fs::create_dir_all(parent).map_err(io_error)?;
    }
    let temp = std::path::PathBuf::from(format!("{}.tmp.{}", file.display(), random_hex(8)));
    let result = (|| -> std::io::Result<()> {
        let mut handle = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)?;
        handle.write_all(content.as_bytes())?;
        handle.flush()?;
        fs::rename(&temp, file)?;
        Ok(())
    })();
    if let Err(error) = result {
        try_unlink(&temp);
        return Err(io_error(error));
    }
    Ok(())
}

/// Parse JSONL exactly as Roots historically does: blank, malformed, and
/// non-object rows are ignored.  Type conversion failures are ignored too,
/// which preserves the tolerant read behavior of the TypeScript cast.
#[must_use]
pub fn parse_jsonl_values(content: &str) -> Vec<Value> {
    content
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() {
                return None;
            }
            match serde_json::from_str::<Value>(line) {
                Ok(Value::Object(map)) => Some(Value::Object(map)),
                _ => None,
            }
        })
        .collect()
}

#[must_use]
pub fn parse_jsonl<T: DeserializeOwned>(content: &str) -> Vec<T> {
    parse_jsonl_values(content)
        .into_iter()
        .filter_map(|value| serde_json::from_value(value).ok())
        .collect()
}

pub fn read_jsonl_values(file: impl AsRef<Path>) -> RootsResult<Vec<Value>> {
    let file = file.as_ref();
    if !file.exists() {
        return Ok(Vec::new());
    }
    fs::read_to_string(file)
        .map(|content| parse_jsonl_values(&content))
        .map_err(io_error)
}

pub fn read_jsonl_file<T: DeserializeOwned>(file: impl AsRef<Path>) -> RootsResult<Vec<T>> {
    let file = file.as_ref();
    if !file.exists() {
        return Ok(Vec::new());
    }
    fs::read_to_string(file)
        .map(|content| parse_jsonl(&content))
        .map_err(io_error)
}

#[must_use]
pub fn dedup_by<T, F>(items: Vec<T>, mut key: F) -> Vec<T>
where
    F: FnMut(&T) -> String,
{
    let mut indices = HashMap::new();
    let mut output = Vec::new();
    for item in items {
        let item_key = key(&item);
        if let Some(index) = indices.get(&item_key).copied() {
            output[index] = item;
        } else {
            indices.insert(item_key, output.len());
            output.push(item);
        }
    }
    output
}

#[must_use]
pub fn dedup_by_id<T: HasId>(items: Vec<T>) -> Vec<T> {
    dedup_by(items, |item| item.id().to_owned())
}

pub fn serialize_jsonl<T: Serialize>(records: &[T]) -> RootsResult<String> {
    if records.is_empty() {
        return Ok(String::new());
    }
    records
        .iter()
        .map(|record| serde_json::to_string(record).map_err(io_error))
        .collect::<RootsResult<Vec<_>>>()
        .map(|lines| format!("{}\n", lines.join("\n")))
}

pub fn write_jsonl_file<T: Serialize>(file: impl AsRef<Path>, records: &[T]) -> RootsResult<()> {
    atomic_write(file, &serialize_jsonl(records)?)
}

pub fn append_jsonl_file<T: Serialize>(file: impl AsRef<Path>, records: &[T]) -> RootsResult<()> {
    if records.is_empty() {
        return Ok(());
    }
    let file = file.as_ref();
    let existing = if file.exists() {
        fs::read_to_string(file).map_err(io_error)?
    } else {
        String::new()
    };
    let prefix = if existing.is_empty() || existing.ends_with('\n') {
        existing
    } else {
        format!("{existing}\n")
    };
    atomic_write(file, &(prefix + &serialize_jsonl(records)?))
}

pub fn read_table<T: DeserializeOwned + HasId>(file: impl AsRef<Path>) -> RootsResult<Vec<T>> {
    read_jsonl_file(file).map(dedup_by_id)
}

pub fn update_table<T, R, F>(file: impl AsRef<Path>, action: F) -> RootsResult<R>
where
    T: DeserializeOwned + Serialize + HasId,
    F: FnOnce(Vec<T>) -> RootsResult<(Vec<T>, bool, R)>,
{
    let file = file.as_ref();
    with_lock(
        file,
        || {
            let (rows, write, result) = action(read_table(file)?)?;
            if write {
                write_jsonl_file(file, &dedup_by_id(rows))?;
            }
            Ok(result)
        },
        LockOptions::default(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};

    #[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
    struct Row {
        id: String,
        value: u8,
    }
    impl HasId for Row {
        fn id(&self) -> &str {
            &self.id
        }
    }

    #[test]
    fn jsonl_is_tolerant_and_last_wins_in_first_seen_order() {
        let values: Vec<Row> =
            parse_jsonl("{\"id\":\"a\",\"value\":1}\nnot json\n[]\n{\"id\":\"a\",\"value\":2}\n");
        let rows = dedup_by_id(values);
        assert_eq!(
            rows,
            vec![Row {
                id: "a".into(),
                value: 2
            }]
        );
        assert_eq!(
            serialize_jsonl(&rows).expect("serialize"),
            "{\"id\":\"a\",\"value\":2}\n"
        );
    }

    #[test]
    fn stale_reclaim_requires_matching_mtime_and_file_identity() {
        let mtime = SystemTime::UNIX_EPOCH + Duration::from_secs(42);
        let seen = LockObservation {
            mtime,
            identity: Some(FileIdentity {
                device_or_volume: 7,
                inode_or_file_index: 11,
            }),
        };
        let same = seen.clone();
        let replacement = LockObservation {
            mtime,
            identity: Some(FileIdentity {
                device_or_volume: 7,
                inode_or_file_index: 12,
            }),
        };
        let unsupported = LockObservation {
            mtime,
            identity: None,
        };

        assert!(same_lock(&seen, &same));
        assert!(!same_lock(&seen, &replacement));
        assert_eq!(
            reclaim_action(&seen, Some(&same)),
            ReclaimAction::DeleteSidecar
        );
        assert_eq!(
            reclaim_action(&seen, Some(&replacement)),
            ReclaimAction::RestoreLock,
            "an equal mtime cannot make a replacement lock reclaimable"
        );
        assert_eq!(
            reclaim_action(&seen, Some(&unsupported)),
            ReclaimAction::RestoreLock,
            "platforms without a file ID choose restoration over deletion"
        );
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn verified_stale_lock_is_deleted_after_sidecar_recheck() {
        let root =
            std::env::temp_dir().join(format!("roots-store-reclaim-{}", crate::ids::random_hex(8)));
        fs::create_dir_all(&root).expect("create sandbox");
        let lock = lock_path(&root.join("records.jsonl"));
        fs::write(&lock, "stale").expect("create lock");
        let seen = lock_observation(&lock).expect("observe lock");
        assert!(
            seen.identity.is_some(),
            "supported platform exposes file ID"
        );

        reclaim_stale_lock(&lock, &seen);

        assert!(!lock.exists(), "the verified stale lock is reclaimed");
        let sidecar_prefix = format!(
            "{}.stale.",
            lock.file_name()
                .and_then(|name| name.to_str())
                .expect("lock file name")
        );
        assert!(
            !fs::read_dir(&root)
                .expect("read sandbox")
                .flatten()
                .any(|entry| entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(&sidecar_prefix)),
            "the recheck leaves no sidecar behind"
        );
        fs::remove_dir_all(root).expect("clean sandbox");
    }
}
