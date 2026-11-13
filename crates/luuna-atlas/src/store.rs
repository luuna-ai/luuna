//! Reading and writing the store's JSONL files: expertise and archive files, file locks,
//! atomic writes and record ids.
//!
//! Every function takes the file it works on as a parameter; nothing here finds a store.
//! A port of the Go tool's `internal/atlas/store.go` (`ReadRecords`, `GenerateRecordID`,
//! `AppendRecord`, `WriteRecords`, `WriteFileAtomic`, `RenameReplacing`, `WithFileLock`,
//! `ResolveRecordID`) and of `ReadArchive` and `WriteArchive` in `retention.go`.

use std::collections::hash_map::RandomState;
use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::hash::{BuildHasher, Hash, Hasher};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

use crate::error::{AtlasError, Result};
use crate::json::{self, Object, Value};
use crate::registry::{apply_aliases, go_bool, go_text, Registry};

/// One record: a JSON object whose key order is kept.
pub type Record = Object;

/// The first line of every archive file, as the Go tool writes it.
pub const ARCHIVE_BANNER: &str =
    "# ARCHIVED — not for active use. Run `luuna-atlas restore <id>` to revive.";

/// How long [`with_file_lock`] waits for a lock before giving up.
pub const LOCK_TIMEOUT: Duration = Duration::from_secs(5);
/// How often [`with_file_lock`] retries a held lock.
pub const LOCK_RETRY: Duration = Duration::from_millis(50);
/// A lock file older than this is treated as left behind and removed.
pub const LOCK_STALE_AFTER: Duration = Duration::from_secs(30);

/// Reads the records of an expertise file. A missing file reads as no records. Unknown
/// record types are refused unless `allow_unknown` is set.
///
/// As in the Go tool: the file is split on LF; lines that are blank or start with `#`
/// after trimming are skipped; any read failure, not only a missing file, reads as no
/// records; a legacy singular `outcome` object becomes a one-element `outcomes` array; and
/// the type's field aliases are applied. A JSON array on a line is accepted and reads as
/// an empty record, which is what the Go tool's code sees of it.
///
/// The original array is retained in a private record sidecar, as Go's
/// RecordFromValue/RecordValue does. Field lookup still sees an empty map,
/// but JSON query/API/archive output preserves the array. Whole expertise
/// rewrites still fail when an array has no type from which to derive an ID.
pub fn read_records(path: &Path, registry: &Registry, allow_unknown: bool) -> Result<Vec<Record>> {
    let Ok(data) = fs::read(path) else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for (idx, line) in data.split(|b| *b == b'\n').enumerate() {
        let text = lossy_per_byte(line);
        let trimmed = text.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let mut value = match json::parse_bytes(line) {
            Ok(value) => value,
            Err(_) => {
                let preview = if utf16_len(trimmed) > 80 {
                    format!("{}...", utf16_prefix(trimmed, 77))
                } else {
                    trimmed.to_owned()
                };
                return Err(AtlasError::Message(format!(
                    "Malformed JSONL at {}:{}: {}. Line: {}",
                    path.display(),
                    idx + 1,
                    json_parse_error(line),
                    preview
                )));
            }
        };
        let mut raw = match &mut value {
            Value::Object(object) => std::mem::take(object),
            // The Go tool's code sees an array row as an empty record (model.go:36-45).
            Value::Array(items) => Object::from_record_array(std::mem::take(items)),
            _ => {
                return Err(AtlasError::Message(
                    "raw is not an Object. (evaluating '\"outcome\" in raw')".to_owned(),
                ))
            }
        };
        rewrite_legacy_outcome(&mut raw);
        let type_name = raw
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let definition = registry.get(&type_name);
        if !type_name.is_empty() && definition.is_none() && !allow_unknown {
            let id_part = match raw.get("id").and_then(Value::as_str) {
                Some(id) => format!(" (id={id})"),
                None => String::new(),
            };
            return Err(AtlasError::Message(format!(
                "Unknown record type \"{type_name}\" at {}:{}{id_part}. Register it under custom_types in config.yaml, remove the record, or pass --allow-unknown-types to bypass.",
                path.display(),
                idx + 1
            )));
        }
        if let Some(definition) = definition {
            apply_aliases(&mut raw, &definition.aliases);
        }
        out.push(raw);
    }
    Ok(out)
}

/// A record with a non-null `outcome` and no `outcomes` gets `outcomes: [{status,
/// duration, test_results, agent}]` (those present, keys sorted) after its remembered
/// fields, and loses `outcome` (store.go:43-58).
fn rewrite_legacy_outcome(raw: &mut Record) {
    let Some(legacy) = raw.get("outcome") else {
        return;
    };
    if legacy.is_null() || raw.contains_key("outcomes") {
        return;
    }
    let mut normalized = Object::new();
    if let Some(legacy) = legacy.as_object() {
        for key in ["status", "duration", "test_results", "agent"] {
            if let Some(value) = legacy.get(key) {
                normalized.insert_unordered(key, value.clone());
            }
        }
    }
    raw.insert_unordered("outcomes", Value::Array(vec![Value::Object(normalized)]));
    raw.remove("outcome");
}

/// Replaces an expertise file atomically with `records`, assigning an id to every record
/// that lacks one.
///
/// Ids are assigned in order, so on an unknown type the records before it keep the ids
/// they were given, as in the Go tool. Zero records write an empty file.
pub fn write_records(path: &Path, records: &mut [Record], registry: &Registry) -> Result<()> {
    let mut body = String::new();
    for record in records.iter_mut() {
        ensure_id(record, registry)?;
        body.push_str(&encode(record)?);
        body.push('\n');
    }
    write_file_atomic(path, body.as_bytes())
}

/// Appends one record as one line, assigning its id first if it lacks one. The caller
/// holds the file's lock.
///
/// The file is opened for appending and created if missing (its directory is not); the
/// line and its LF go out in one write call, as in the Go tool. This write is not atomic.
pub fn append_record(path: &Path, record: &mut Record, registry: &Registry) -> Result<()> {
    ensure_id(record, registry)?;
    let mut line = encode(record)?;
    line.push('\n');
    let mut file = OpenOptions::new()
        .append(true)
        .create(true)
        .open(path)
        .map_err(|source| path_error("open", path, source))?;
    file.write_all(line.as_bytes())
        .map_err(|source| path_error("write", path, source))
}

/// Reads the records of an archive file (unknown types allowed, banner skipped).
pub fn read_archive(path: &Path, registry: &Registry) -> Result<Vec<Record>> {
    read_records(path, registry, true)
}

/// Replaces an archive file atomically: the banner line, then one record per line. The
/// archive directory is created if missing. Records are written as they are; no id is
/// assigned.
pub fn write_archive(path: &Path, records: &[Record]) -> Result<()> {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        fs::create_dir_all(dir).map_err(|source| path_error("mkdir", dir, source))?;
    }
    let mut body = String::from(ARCHIVE_BANNER);
    body.push('\n');
    for record in records {
        body.push_str(&encode(record)?);
        body.push('\n');
    }
    write_file_atomic(path, body.as_bytes())
}

/// Runs `f` while holding the advisory lock `<path>.lock`, and removes the lock afterwards.
///
/// The lock file is created exclusively. While another holder has it, the call retries
/// every 50 ms; a lock file whose modification time is more than 30 s old is removed and
/// the call tries again at once; after 5 s it fails with the Go tool's message. The lock
/// is removed when `f` returns, fails or panics. It is not reentrant.
pub fn with_file_lock<T>(path: &Path, f: impl FnOnce() -> Result<T>) -> Result<T> {
    with_file_lock_timed(path, LOCK_TIMEOUT, LOCK_RETRY, LOCK_STALE_AFTER, f)
}

fn with_file_lock_timed<T>(
    path: &Path,
    timeout: Duration,
    retry: Duration,
    stale_after: Duration,
    f: impl FnOnce() -> Result<T>,
) -> Result<T> {
    let lock = suffixed(path, ".lock");
    let deadline = Instant::now() + timeout;
    loop {
        match OpenOptions::new().write(true).create_new(true).open(&lock) {
            Ok(_) => break,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(source) => return Err(path_error("open", &lock, source)),
        }
        if let Ok(metadata) = fs::symlink_metadata(&lock) {
            let stale = metadata
                .modified()
                .ok()
                .and_then(|modified| SystemTime::now().duration_since(modified).ok())
                .is_some_and(|age| age > stale_after);
            if stale {
                // As in the Go tool: a failed removal is ignored and the loop goes on
                // without checking the deadline.
                let _ = fs::remove_file(&lock);
                continue;
            }
        }
        if Instant::now() > deadline {
            return Err(AtlasError::Message(format!(
                "Timed out waiting for lock on {}. If no other luuna-atlas process is running, delete the lock file manually.",
                lock.display()
            )));
        }
        std::thread::sleep(retry);
    }
    let _guard = LockGuard(lock);
    f()
}

/// Removes the lock file when dropped, so that a failing or panicking closure still
/// releases the lock (the Go tool's `defer os.Remove(lock)`).
struct LockGuard(PathBuf);

impl Drop for LockGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

/// The Go tool's deterministic id for `record`: `mx-` and the first six hex digits of the
/// SHA-256 of `<type>:<value of the type's id key>`. Unknown types are an error.
pub fn generate_record_id(record: &Record, registry: &Registry) -> Result<String> {
    let type_name = go_text(record.get("type"));
    let Some(definition) = registry.get(&type_name) else {
        return Err(AtlasError::Message(format!(
            "Unknown record type: {type_name}"
        )));
    };
    let key = format!("{type_name}:{}", go_text(record.get(&definition.id_key)));
    let digest = Sha256::digest(key.as_bytes());
    let hex: String = digest[..3].iter().map(|b| format!("{b:02x}")).collect();
    Ok(format!("mx-{hex}"))
}

/// The index of the record `id` names: an exact id (with or without the `mx-` prefix),
/// otherwise the one record whose id starts with it. Several matches or none are an
/// error with the Go tool's message.
pub fn resolve_record_id(records: &[Record], id: &str) -> Result<usize> {
    let hash = id.strip_prefix("mx-").unwrap_or(id);
    let wanted = format!("mx-{hash}");
    if let Some(index) = records.iter().position(|r| go_text(r.get("id")) == wanted) {
        return Ok(index);
    }
    let matches: Vec<(usize, String)> = records
        .iter()
        .enumerate()
        .map(|(i, r)| (i, go_text(r.get("id"))))
        .filter(|(_, rid)| rid.starts_with(&wanted))
        .collect();
    match matches.as_slice() {
        [(index, _)] => Ok(*index),
        [] => Err(AtlasError::Message(format!(
            "Record \"{id}\" not found. Run `luuna-atlas query` to see record IDs."
        ))),
        _ => Err(AtlasError::Message(format!(
            "Ambiguous identifier \"{id}\" matches {} records: {}. Use more characters to disambiguate.",
            matches.len(),
            matches
                .iter()
                .map(|(_, rid)| rid.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ))),
    }
}

/// Gives `record` its generated id unless its `id` is set (non-empty, non-zero, true).
/// The id is assigned as a Go map assignment: in place if the key exists, otherwise after
/// the remembered fields.
fn ensure_id(record: &mut Record, registry: &Registry) -> Result<()> {
    if !go_bool(record.get("id")) {
        let id = generate_record_id(record, registry)?;
        record.insert_unordered("id", id);
    }
    Ok(())
}

fn encode(record: &Record) -> Result<String> {
    json::to_compact_string(&Value::Object(record.clone()))
}

fn path_error(op: &'static str, path: &Path, source: io::Error) -> AtlasError {
    AtlasError::PathOp {
        op,
        path: path.to_path_buf(),
        source,
    }
}

/// `path` with `suffix` appended to its last component.
fn suffixed(path: &Path, suffix: &str) -> PathBuf {
    let mut name = OsString::from(path.as_os_str());
    name.push(suffix);
    PathBuf::from(name)
}

/// Sixteen hex digits that differ between calls, from std only: a randomly keyed
/// `RandomState` hash of the time, the process id and a per-process counter.
fn temp_suffix() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let mut hasher = RandomState::new().build_hasher();
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default()
        .hash(&mut hasher);
    std::process::id().hash(&mut hasher);
    COUNTER.fetch_add(1, Ordering::Relaxed).hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

/// Writes `data` to `<path>.tmp.<16 hex>` and renames it over `path`. If the rename fails
/// the temporary file is removed. As in the Go tool, a failed write of the temporary file
/// returns at once, and nothing is synced to disk.
pub(crate) fn write_file_atomic(path: &Path, data: &[u8]) -> Result<()> {
    let tmp = suffixed(path, &format!(".tmp.{}", temp_suffix()));
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&tmp)
        .map_err(|source| path_error("open", &tmp, source))?;
    file.write_all(data)
        .map_err(|source| path_error("write", &tmp, source))?;
    drop(file);
    if let Err(source) = rename_replacing(&tmp, path) {
        let _ = fs::remove_file(&tmp);
        return Err(AtlasError::Rename {
            from: tmp,
            to: path.to_path_buf(),
            source,
        });
    }
    Ok(())
}

/// `fs::rename`, retried on Windows up to 20 attempts while the target is in use, with a
/// delay of 5 ms doubled per attempt and capped at 100 ms (store.go:144-162).
pub(crate) fn rename_replacing(from: &Path, to: &Path) -> io::Result<()> {
    let mut attempt: u32 = 1;
    loop {
        let error = match fs::rename(from, to) {
            Ok(()) => return Ok(()),
            Err(error) => error,
        };
        if !cfg!(windows) || attempt >= 20 {
            return Err(error);
        }
        let text = error.to_string().to_lowercase();
        if error.kind() != io::ErrorKind::PermissionDenied
            && !text.contains("used by another process")
            && !text.contains("access is denied")
        {
            return Err(error);
        }
        let delay = Duration::from_millis(5)
            .saturating_mul(1 << attempt.min(31))
            .min(Duration::from_millis(100));
        std::thread::sleep(delay);
        attempt += 1;
    }
}

// ---------------------------------------------------------------------------------------
// Text helpers for the malformed-line message.

/// `bytes` as text with each byte that is not part of valid UTF-8 replaced by U+FFFD, as
/// Go's string-to-rune conversion does.
fn lossy_per_byte(mut bytes: &[u8]) -> String {
    let mut out = String::new();
    while !bytes.is_empty() {
        match std::str::from_utf8(bytes) {
            Ok(valid) => {
                out.push_str(valid);
                break;
            }
            Err(error) => {
                let (valid, rest) = bytes.split_at(error.valid_up_to());
                out.push_str(std::str::from_utf8(valid).unwrap_or_default());
                out.push(char::REPLACEMENT_CHARACTER);
                bytes = &rest[1..];
            }
        }
    }
    out
}

/// The length of `s` in UTF-16 code units, as JavaScript counts it.
fn utf16_len(s: &str) -> usize {
    s.encode_utf16().count()
}

/// The first `n` UTF-16 code units of `s`; a surrogate pair cut in half becomes U+FFFD.
fn utf16_prefix(s: &str, n: usize) -> String {
    let units: Vec<u16> = s.encode_utf16().take(n).collect();
    String::from_utf16_lossy(&units)
}

/// The Go tool's `writeJSONParseError`: the message JavaScriptCore's `JSON.parse` gave
/// for `input`, found by a diagnostic walk over its bytes (write_json.go).
pub(crate) fn json_parse_error(input: &[u8]) -> String {
    let mut walk = Diagnostic { s: input, pos: 0 };
    let message = walk.value();
    let message = if message.is_empty() {
        "Unable to parse JSON string".to_owned()
    } else {
        message
    };
    format!("JSON Parse error: {message}")
}

struct Diagnostic<'a> {
    s: &'a [u8],
    pos: usize,
}

impl Diagnostic<'_> {
    fn at(&self) -> Option<u8> {
        self.s.get(self.pos).copied()
    }

    fn space(&mut self) {
        while let Some(b' ' | b'\t' | b'\r' | b'\n') = self.at() {
            self.pos += 1;
        }
    }

    /// An empty message means the value was read without error.
    /// Container continuations use explicit frames, preserving the diagnostic
    /// grammar without a Rust stack frame for every user nesting level.
    fn value(&mut self) -> String {
        enum Frame {
            ObjectAfterValue,
            ArrayAfterValue,
        }
        let mut stack = Vec::new();
        'read_value: loop {
            self.space();
            let Some(ch) = self.at() else {
                return "Unexpected EOF".to_owned();
            };
            match ch {
                b'{' => {
                    self.pos += 1;
                    self.space();
                    if self.at() == Some(b'}') {
                        self.pos += 1;
                    } else {
                        if self.at() != Some(b'"') {
                            return "Expected '}'".to_owned();
                        }
                        let message = self.property();
                        if !message.is_empty() {
                            return message;
                        }
                        stack.push(Frame::ObjectAfterValue);
                        continue 'read_value;
                    }
                }
                b'[' => {
                    self.pos += 1;
                    self.space();
                    if self.at() == Some(b']') {
                        self.pos += 1;
                    } else {
                        stack.push(Frame::ArrayAfterValue);
                        continue 'read_value;
                    }
                }
                _ => {
                    let message = self.scalar_value(ch);
                    if !message.is_empty() {
                        return message;
                    }
                }
            }
            loop {
                self.space();
                match stack.last() {
                    None => return String::new(),
                    Some(Frame::ObjectAfterValue) => {
                        if self.at() == Some(b'}') {
                            self.pos += 1;
                            stack.pop();
                            continue;
                        }
                        if self.at() != Some(b',') {
                            return "Expected '}'".to_owned();
                        }
                        self.pos += 1;
                        self.space();
                        if self.at() != Some(b'"') {
                            return "Property name must be a string literal".to_owned();
                        }
                        let message = self.property();
                        if !message.is_empty() {
                            return message;
                        }
                        continue 'read_value;
                    }
                    Some(Frame::ArrayAfterValue) => {
                        if self.at() == Some(b']') {
                            self.pos += 1;
                            stack.pop();
                            continue;
                        }
                        if self.at() != Some(b',') {
                            return "Expected ']'".to_owned();
                        }
                        self.pos += 1;
                        self.space();
                        if self.at() == Some(b']') {
                            return "Unexpected comma at the end of array expression".to_owned();
                        }
                        continue 'read_value;
                    }
                }
            }
        }
    }

    fn property(&mut self) -> String {
        let message = self.string_value();
        if !message.is_empty() {
            return message;
        }
        self.space();
        if self.at() != Some(b':') {
            return "Expected ':' before value in object property definition".to_owned();
        }
        self.pos += 1;
        String::new()
    }

    fn scalar_value(&mut self, ch: u8) -> String {
        match ch {
            b'"' => self.string_value(),
            b'\'' => "Single quotes (') are not allowed in JSON".to_owned(),
            // Go's %c of a byte prints the code point of the same value.
            b'}' | b']' | b':' | b',' | b'.' => format!("Unexpected token '{}'", char::from(ch)),
            b'-' | b'0'..=b'9' => self.number(),
            b'a'..=b'z' | b'A'..=b'Z' | b'_' | b'$' => {
                let start = self.pos;
                while let Some(b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'_' | b'$') = self.at() {
                    self.pos += 1;
                }
                let word = &self.s[start..self.pos];
                for literal in [&b"true"[..], b"false", b"null"] {
                    if word.starts_with(literal) {
                        self.pos = start + literal.len();
                        return String::new();
                    }
                }
                // The word is ASCII.
                let word = String::from_utf8_lossy(word).into_owned();
                let quoted = json::to_compact_string(&Value::String(word)).unwrap_or_default();
                format!("Unexpected identifier {quoted}")
            }
            _ => format!("Unrecognized token '{}'", char::from(ch)),
        }
    }

    fn digit_at(&self) -> bool {
        self.at().is_some_and(|b| b.is_ascii_digit())
    }

    fn number(&mut self) -> String {
        if self.at() == Some(b'-') {
            self.pos += 1;
        }
        if !self.digit_at() {
            return "Invalid number".to_owned();
        }
        if self.at() == Some(b'0') {
            self.pos += 1;
        } else {
            while self.digit_at() {
                self.pos += 1;
            }
        }
        if self.at() == Some(b'.') {
            self.pos += 1;
            if !self.digit_at() {
                return "Invalid digits after decimal point".to_owned();
            }
            while self.digit_at() {
                self.pos += 1;
            }
        }
        if let Some(b'e' | b'E') = self.at() {
            let start = self.pos;
            self.pos += 1;
            if let Some(b'+' | b'-') = self.at() {
                self.pos += 1;
            }
            if !self.digit_at() {
                self.pos = start;
                return String::new();
            }
            while self.digit_at() {
                self.pos += 1;
            }
        }
        String::new()
    }

    fn string_value(&mut self) -> String {
        self.pos += 1;
        while let Some(ch) = self.at() {
            self.pos += 1;
            if ch == b'"' {
                return String::new();
            }
            if ch < 0x20 {
                return "Unterminated string".to_owned();
            }
            if ch != b'\\' {
                continue;
            }
            let Some(ch) = self.at() else {
                return "Unterminated string".to_owned();
            };
            self.pos += 1;
            if b"\"\\/bfnrt".contains(&ch) {
                continue;
            }
            if ch != b'u' {
                return format!("Invalid escape character {}", char::from(ch));
            }
            let start = self.pos - 2;
            let end = (self.pos + 4).min(self.s.len());
            let valid =
                end - self.pos == 4 && self.s[self.pos..end].iter().all(u8::is_ascii_hexdigit);
            if !valid {
                return format!(
                    "\"{}\" is not a valid unicode escape",
                    lossy_per_byte(&self.s[start..end])
                );
            }
            self.pos = end;
        }
        "Unterminated string".to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::json::parse;

    fn registry() -> Registry {
        Registry::from_config(None).unwrap()
    }

    fn record(text: &str) -> Record {
        parse(text).unwrap().as_object().unwrap().clone()
    }

    fn leftovers(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains(".tmp.") || n.ends_with(".lock"))
            .collect();
        names.sort();
        names
    }

    #[test]
    fn append_assigns_go_style_id() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("d.jsonl");
        let registry = registry();
        // sha256("convention:Use tabs") begins 1d9aeb...; the id key of a convention is
        // its content.
        let mut first = record(
            r#"{"type":"convention","classification":"tactical","recorded_at":"2025-01-02T03:04:05.000Z","content":"Use tabs"}"#,
        );
        append_record(&path, &mut first, &registry).unwrap();
        let expected = format!(
            "mx-{}",
            &Sha256::digest(b"convention:Use tabs")
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()[..6]
        );
        assert_eq!(first.get("id").and_then(Value::as_str), Some(&*expected));
        // A record that has an id keeps it; a null id is replaced in place.
        let mut second = record(r#"{"type":"guide","id":"mx-123456","name":"n"}"#);
        append_record(&path, &mut second, &registry).unwrap();
        let mut third = record(r#"{"id":null,"type":"guide","name":"n"}"#);
        append_record(&path, &mut third, &registry).unwrap();
        let guide_id = generate_record_id(&third, &registry).unwrap();
        assert!(guide_id.starts_with("mx-") && guide_id.len() == 9);
        assert!(guide_id[3..]
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()));
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            format!(
                "{{\"type\":\"convention\",\"classification\":\"tactical\",\"recorded_at\":\"2025-01-02T03:04:05.000Z\",\"content\":\"Use tabs\",\"id\":\"{expected}\"}}\n\
                 {{\"type\":\"guide\",\"id\":\"mx-123456\",\"name\":\"n\"}}\n\
                 {{\"id\":\"{guide_id}\",\"type\":\"guide\",\"name\":\"n\"}}\n"
            )
        );
        // Unknown types get no id, with the Go message.
        let mut unknown = record(r#"{"type":"nope"}"#);
        let error = append_record(&path, &mut unknown, &registry).unwrap_err();
        assert_eq!(error.to_string(), "Unknown record type: nope");
        // A missing directory is not created; the error is worded as Go's.
        let missing = dir.path().join("no").join("d.jsonl");
        let error = append_record(&missing, &mut first, &registry).unwrap_err();
        assert_eq!(
            error.to_string(),
            format!("open {}: no such file or directory", missing.display())
        );
        // Lookup by full id, bare hex, or a unique prefix.
        let records = read_records(&path, &registry, false).unwrap();
        assert_eq!(resolve_record_id(&records, &expected).unwrap(), 0);
        assert_eq!(resolve_record_id(&records, "123456").unwrap(), 1);
        assert_eq!(resolve_record_id(&records, "mx-1234").unwrap(), 1);
        assert_eq!(
            resolve_record_id(&records, "zz").unwrap_err().to_string(),
            "Record \"zz\" not found. Run `luuna-atlas query` to see record IDs."
        );
        let both = vec![
            record(r#"{"id":"mx-abc111"}"#),
            record(r#"{"id":"mx-abc222"}"#),
        ];
        assert_eq!(
            resolve_record_id(&both, "abc").unwrap_err().to_string(),
            "Ambiguous identifier \"abc\" matches 2 records: mx-abc111, mx-abc222. Use more characters to disambiguate."
        );
    }

    #[test]
    fn atomic_write_leaves_no_temp_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("d.jsonl");
        let registry = registry();
        fs::write(&path, "old\n").unwrap();
        let mut records = vec![
            record(r#"{"type":"guide","name":"a","description":"x"}"#),
            record(r#"{"type":"guide","name":"b","description":"y","id":"mx-000000"}"#),
        ];
        write_records(&path, &mut records, &registry).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert_eq!(text.lines().count(), 2);
        assert!(text.ends_with("\"id\":\"mx-000000\"}\n"));
        assert!(
            leftovers(dir.path()).is_empty(),
            "{:?}",
            leftovers(dir.path())
        );
        // No records: an empty file.
        write_records(&path, &mut [], &registry).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"");
        // A failed rename removes the temporary file: the target is a non-empty directory.
        let blocked = dir.path().join("blocked");
        fs::create_dir(&blocked).unwrap();
        fs::write(blocked.join("x"), "").unwrap();
        let error = write_records(&blocked, &mut records, &registry).unwrap_err();
        assert!(error.to_string().starts_with("rename "), "{error}");
        assert!(
            leftovers(dir.path()).is_empty(),
            "{:?}",
            leftovers(dir.path())
        );
        // The suffix is sixteen lowercase hex digits and differs between calls.
        let (a, b) = (temp_suffix(), temp_suffix());
        assert_ne!(a, b);
        for s in [a, b] {
            assert_eq!(s.len(), 16);
            assert!(s
                .bytes()
                .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c)));
        }
    }

    #[test]
    fn lock_times_out_with_go_message() {
        assert_eq!(
            (LOCK_TIMEOUT, LOCK_RETRY, LOCK_STALE_AFTER),
            (
                Duration::from_secs(5),
                Duration::from_millis(50),
                Duration::from_secs(30)
            )
        );
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("d.jsonl");
        let lock = dir.path().join("d.jsonl.lock");
        fs::write(&lock, "").unwrap();
        let started = Instant::now();
        let mut ran = false;
        let result = with_file_lock_timed(
            &path,
            Duration::from_millis(200),
            LOCK_RETRY,
            LOCK_STALE_AFTER,
            || {
                ran = true;
                Ok(())
            },
        );
        assert!(started.elapsed() >= Duration::from_millis(200));
        assert!(!ran);
        assert_eq!(
            result.unwrap_err().to_string(),
            format!(
                "Timed out waiting for lock on {}. If no other luuna-atlas process is running, delete the lock file manually.",
                lock.display()
            )
        );
        // Another holder's lock is left alone.
        assert!(lock.exists());
        fs::remove_file(&lock).unwrap();
        // A free lock is taken, held while the closure runs and removed afterwards, also
        // when the closure fails.
        let value = with_file_lock(&path, || {
            assert!(lock.is_file());
            Ok(7)
        })
        .unwrap();
        assert_eq!(value, 7);
        assert!(!lock.exists());
        let failed: Result<()> =
            with_file_lock(&path, || Err(AtlasError::Message("no".to_owned())));
        assert_eq!(failed.unwrap_err().to_string(), "no");
        assert!(!lock.exists());
    }

    #[test]
    fn stale_lock_is_removed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("d.jsonl");
        let lock = dir.path().join("d.jsonl.lock");
        let file = fs::File::create(&lock).unwrap();
        file.set_modified(SystemTime::now() - Duration::from_secs(31))
            .unwrap();
        drop(file);
        let started = Instant::now();
        with_file_lock(&path, || {
            assert!(lock.is_file());
            Ok(())
        })
        .unwrap();
        assert!(started.elapsed() < LOCK_TIMEOUT);
        assert!(!lock.exists());
        // A lock younger than the limit is not stale.
        let file = fs::File::create(&lock).unwrap();
        file.set_modified(SystemTime::now() - Duration::from_secs(10))
            .unwrap();
        drop(file);
        let result = with_file_lock_timed(
            &path,
            Duration::from_millis(100),
            LOCK_RETRY,
            LOCK_STALE_AFTER,
            || Ok(()),
        );
        assert!(result.is_err());
        assert!(lock.exists());
    }

    #[test]
    fn archive_banner_is_first_line() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("archive").join("d.jsonl");
        let registry = registry();
        // The directory is created; records are written as they are, unknown types and
        // missing ids included.
        let records = vec![
            record(r#"{"type":"guide","name":"a","status":"archived"}"#),
            record(r#"{"type":"retired_type","x":1}"#),
        ];
        write_archive(&path, &records).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert_eq!(
            text,
            format!(
                "{ARCHIVE_BANNER}\n{{\"type\":\"guide\",\"name\":\"a\",\"status\":\"archived\"}}\n{{\"type\":\"retired_type\",\"x\":1}}\n"
            )
        );
        assert_eq!(
            text.lines().next().unwrap(),
            "# ARCHIVED — not for active use. Run `luuna-atlas restore <id>` to revive."
        );
        // Reading skips the banner and allows unknown types.
        assert_eq!(read_archive(&path, &registry).unwrap(), records);
        assert!(read_records(&path, &registry, false).is_err());
        // No records: the banner alone.
        write_archive(&path, &[]).unwrap();
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            format!("{ARCHIVE_BANNER}\n")
        );
        assert!(leftovers(path.parent().unwrap()).is_empty());
    }

    #[test]
    fn malformed_line_reports_path_and_line() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("d.jsonl");
        let registry = registry();
        fs::write(
            &path,
            "\n  # comment\n{\"type\":\"guide\",\"name\":\"a\"}\r\n{\"type\":\"guide\",}\n",
        )
        .unwrap();
        assert_eq!(
            read_records(&path, &registry, false).unwrap_err().to_string(),
            format!(
                "Malformed JSONL at {}:4: JSON Parse error: Property name must be a string literal. Line: {{\"type\":\"guide\",}}",
                path.display()
            )
        );
        // A long line is cut to 77 UTF-16 units and "...".
        let long = format!("{{\"content\":\"{}\"", "é".repeat(100));
        fs::write(&path, &long).unwrap();
        let expected_preview: String = long.chars().take(77).collect();
        assert_eq!(
            read_records(&path, &registry, false).unwrap_err().to_string(),
            format!(
                "Malformed JSONL at {}:1: JSON Parse error: Expected '}}'. Line: {expected_preview}...",
                path.display()
            )
        );
        // Other diagnostics of the Go walk.
        for (line, message) in [
            ("{\"a\":tru}", "Unexpected identifier \"tru\""),
            ("{\"a\":1.}", "Invalid digits after decimal point"),
            ("{\"a\":\"\\x\"}", "Invalid escape character x"),
            (
                "{\"a\":\"\\u12g4\"}",
                "\"\\u12g4\" is not a valid unicode escape",
            ),
            ("{'a':1}", "Expected '}'"),
            ("[1,]", "Unexpected comma at the end of array expression"),
            ("{} {}", "Unable to parse JSON string"),
            ("{\"a\":01}", "Expected '}'"),
            ("\u{e9}", "Unrecognized token '\u{c3}'"),
        ] {
            fs::write(&path, line).unwrap();
            let error = read_records(&path, &registry, false)
                .unwrap_err()
                .to_string();
            assert!(
                error.contains(&format!("JSON Parse error: {message}. Line: ")),
                "{line}: {error}"
            );
        }
        // A scalar line is refused with the Go tool's message.
        fs::write(&path, "\"text\"\n").unwrap();
        assert_eq!(
            read_records(&path, &registry, false)
                .unwrap_err()
                .to_string(),
            "raw is not an Object. (evaluating '\"outcome\" in raw')"
        );
        // A missing file is no records.
        assert!(
            read_records(&dir.path().join("none.jsonl"), &registry, false)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn read_rewrites_legacy_outcome_and_checks_types() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("d.jsonl");
        let registry = registry();
        fs::write(
            &path,
            concat!(
                r#"{"type":"decision","title":"t","outcome":{"notes":"n","status":"success","agent":"a"},"id":"mx-111111"}"#,
                "\n",
                r#"{"type":"decision","title":"u","outcome":null}"#,
                "\n",
                r#"{"type":"decision","title":"v","outcome":{"status":"x"},"outcomes":[]}"#,
                "\n",
                r#"[1,2]"#,
                "\n",
            ),
        )
        .unwrap();
        let records = read_records(&path, &registry, false).unwrap();
        let encoded: Vec<String> = records.iter().map(|r| encode(r).unwrap()).collect();
        assert_eq!(
            encoded,
            [
                r#"{"type":"decision","title":"t","id":"mx-111111","outcomes":[{"agent":"a","status":"success"}]}"#,
                r#"{"type":"decision","title":"u","outcome":null}"#,
                r#"{"type":"decision","title":"v","outcome":{"status":"x"},"outcomes":[]}"#,
                "{}",
            ]
        );
        fs::write(&path, "{\"type\":\"nope\",\"id\":\"mx-1\"}\n").unwrap();
        assert_eq!(
            read_records(&path, &registry, false).unwrap_err().to_string(),
            format!(
                "Unknown record type \"nope\" at {}:1 (id=mx-1). Register it under custom_types in config.yaml, remove the record, or pass --allow-unknown-types to bypass.",
                path.display()
            )
        );
        assert_eq!(read_records(&path, &registry, true).unwrap().len(), 1);
    }

    #[test]
    fn custom_field_named_outcome_returns_to_its_place() {
        // Go deletes the legacy `outcome`, then the alias sets it again; Go's remembered
        // order still holds it, so it returns to its old place. Checked against the
        // reference binary.
        let cfg = parse(
            r#"{"custom_types":{"thing":{"required":["title"],"optional":["outcome"],"summary":"{title}","dedup_key":"title","aliases":{"outcome":["result"]}}}}"#,
        )
        .unwrap();
        let registry = Registry::from_config(cfg.as_object()).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("d.jsonl");
        fs::write(
            &path,
            r#"{"type":"thing","outcome":"a","classification":"tactical","recorded_at":"2025-01-01T00:00:00.000Z","result":"b","title":"T","id":"mx-abcdef"}"#,
        )
        .unwrap();
        let records = read_records(&path, &registry, false).unwrap();
        assert_eq!(
            encode(&records[0]).unwrap(),
            r#"{"type":"thing","outcome":"b","classification":"tactical","recorded_at":"2025-01-01T00:00:00.000Z","title":"T","id":"mx-abcdef","outcomes":[{}]}"#
        );
    }
}
