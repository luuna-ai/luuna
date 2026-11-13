//! The foundation store reclamation protocol, copied without changing store.rs.
use crate::model;
use std::{
    fs,
    path::{Path, PathBuf},
};
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
pub(super) fn reclaim_stale_lock(lock: &Path, observed: &fs::Metadata) -> bool {
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
    #[test]
    fn reclaim_matches_the_store_protocol() {
        let t = tempfile::tempdir().unwrap();
        let lock = t.path().join("issues.jsonl.lock");
        fs::write(&lock, b"old").unwrap();
        let observed = fs::metadata(&lock).unwrap();
        assert!(reclaim_stale_lock(&lock, &observed));
        assert!(!lock.exists());
        fs::write(&lock, b"old").unwrap();
        let observed = fs::metadata(&lock).unwrap();
        let replacement = t.path().join("replacement");
        fs::write(&replacement, b"replacement bytes").unwrap();
        fs::rename(&replacement, &lock).unwrap();
        assert!(!reclaim_stale_lock(&lock, &observed));
        assert_eq!(fs::read(&lock).unwrap(), b"replacement bytes");
        let observed = fs::metadata(&lock).unwrap();
        fs::remove_file(&lock).unwrap();
        assert!(!reclaim_stale_lock(&lock, &observed));
        assert_eq!(fs::read_dir(t.path()).unwrap().count(), 0);
    }
}
