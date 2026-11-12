//! Reading and writing the store's JSONL files: expertise and archive files, file locks,
//! atomic writes and record ids.
//!
//! Every function takes the file it works on as a parameter; nothing here finds a store.
//! Task t1 declares the functions; task t3 fills them in.

use std::path::Path;

use crate::error::{AtlasError, Result};
use crate::json::Object;
use crate::registry::Registry;

/// One record: a JSON object whose key order is kept.
pub type Record = Object;

/// The first line of every archive file, as the Go tool writes it.
pub const ARCHIVE_BANNER: &str =
    "# ARCHIVED — not for active use. Run `luuna-atlas restore <id>` to revive.";

/// Reads the records of an expertise file. A missing file reads as no records. Unknown
/// record types are refused unless `allow_unknown` is set.
pub fn read_records(
    _path: &Path,
    _registry: &Registry,
    _allow_unknown: bool,
) -> Result<Vec<Record>> {
    Err(AtlasError::not_implemented("store::read_records (task t3)"))
}

/// Replaces an expertise file atomically with `records`, assigning an id to every record
/// that lacks one.
pub fn write_records(_path: &Path, _records: &mut [Record], _registry: &Registry) -> Result<()> {
    Err(AtlasError::not_implemented(
        "store::write_records (task t3)",
    ))
}

/// Appends one record as one line, assigning its id first if it lacks one. The caller
/// holds the file's lock.
pub fn append_record(_path: &Path, _record: &mut Record, _registry: &Registry) -> Result<()> {
    Err(AtlasError::not_implemented(
        "store::append_record (task t3)",
    ))
}

/// Reads the records of an archive file (unknown types allowed, banner skipped).
pub fn read_archive(_path: &Path, _registry: &Registry) -> Result<Vec<Record>> {
    Err(AtlasError::not_implemented("store::read_archive (task t3)"))
}

/// Replaces an archive file atomically: the banner line, then one record per line.
pub fn write_archive(_path: &Path, _records: &[Record]) -> Result<()> {
    Err(AtlasError::not_implemented(
        "store::write_archive (task t3)",
    ))
}

/// Runs `f` while holding the advisory lock `<path>.lock`, and removes the lock afterwards.
pub fn with_file_lock<T>(_path: &Path, _f: impl FnOnce() -> Result<T>) -> Result<T> {
    Err(AtlasError::not_implemented(
        "store::with_file_lock (task t3)",
    ))
}

/// The Go tool's deterministic id for `record`: `mx-` and the first six hex digits of the
/// SHA-256 of `<type>:<value of the type's id key>`. Unknown types are an error.
pub fn generate_record_id(_record: &Record, _registry: &Registry) -> Result<String> {
    Err(AtlasError::not_implemented(
        "store::generate_record_id (task t3)",
    ))
}
