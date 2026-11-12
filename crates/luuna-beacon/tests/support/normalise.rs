//! Normalisation applied to both sides of every comparison with the Go tool.
//!
//! One [`Normaliser`] serves one side (Go or the library) for a whole case,
//! so that placeholders are numbered by first appearance in that side's
//! stream of documents and store files:
//!
//! - ids `<project>-<hex>` and `schema-<hex>` become `<id:n>`; equal ids get
//!   the same number, so an id kept by update is checked;
//! - `createdAt` and `updatedAt` values must be Go's RFC3339Nano in UTC with
//!   trailing zeros trimmed (a mismatch fails the test) and become `<ts:n>`,
//!   grouped by equality;
//! - the side's canonical project root becomes `<root>` in every string;
//! - arrays that Go builds by iterating a map (`list.prompts`,
//!   `validate.results`, `schema list.schemas`) are sorted by name;
//! - JSONL store files get the same id and timestamp substitution line by
//!   line, with every other byte kept.
//!
//! [`strip_runtime_fields`] removes the keys the luuna runtime needs and the
//! Go tool lacks (A4 differences), and nothing else.

use regex::Regex;
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::path::Path;
use std::sync::OnceLock;

/// Go's `time.RFC3339Nano` in UTC: fractional seconds only when non-zero,
/// with trailing zeros trimmed.
pub const TIMESTAMP_PATTERN: &str =
    r"^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}(\.[0-9]*[1-9])?Z$";

/// Keys whose values are timestamps.
pub const TIMESTAMP_KEYS: [&str; 2] = ["createdAt", "updatedAt"];

/// The placeholder for a project root.
pub const ROOT: &str = "<root>";

fn timestamp_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| Regex::new(TIMESTAMP_PATTERN).expect("timestamp pattern compiles"))
}

/// Panics unless `value` has Go's trimmed RFC3339Nano form.
pub fn assert_go_timestamp(value: &str) {
    assert!(
        timestamp_regex().is_match(value),
        "timestamp {value:?} is not Go's RFC3339Nano in UTC with trailing zeros trimmed"
    );
}

/// For a document with this `command`, the key of the array whose order Go
/// takes from map iteration.
fn map_ordered_key(command: &str) -> Option<&'static str> {
    match command {
        "list" => Some("prompts"),
        "validate" => Some("results"),
        "schema list" => Some("schemas"),
        _ => None,
    }
}

/// The `command` values of the foundation's documents, including `schema`,
/// the `args[0]` of its error documents. Documents of any other command are
/// left to the hooks of the later library plans.
pub const FOUNDATION_COMMANDS: [&str; 14] = [
    "init",
    "create",
    "update",
    "archive",
    "list",
    "show",
    "history",
    "render",
    "validate",
    "schema",
    "schema create",
    "schema list",
    "schema show",
    "schema rule add",
];

/// Sorts the map-ordered array of a document by each entry's `name`.
/// Entries with equal names are ordered by their content with ids and
/// timestamps blanked, so the order does not depend on Go's map iteration.
pub fn sort_map_ordered_arrays(doc: &mut Value, project: &str) {
    if let Some(command) = doc.get("command").and_then(Value::as_str) {
        if !FOUNDATION_COMMANDS.contains(&command) {
            let command = command.to_owned();
            super::authoring::sort_map_ordered(&command, doc, project);
            super::artifacts::sort_map_ordered(&command, doc, project);
            return;
        }
    }
    let Some(key) = doc
        .get("command")
        .and_then(Value::as_str)
        .and_then(map_ordered_key)
    else {
        return;
    };
    let Some(entries) = doc.get_mut(key).and_then(Value::as_array_mut) else {
        return;
    };
    let ids = id_regex(project);
    entries.sort_by_cached_key(|entry| {
        let name = entry
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let mut blank = entry.clone();
        blank_volatile(&mut blank, &ids);
        (name, blank.to_string())
    });
}

fn blank_volatile(value: &mut Value, ids: &Regex) {
    match value {
        Value::String(text) if ids.is_match(text) => text.clear(),
        Value::Array(items) => items.iter_mut().for_each(|item| blank_volatile(item, ids)),
        Value::Object(object) => {
            for (key, item) in object.iter_mut() {
                if TIMESTAMP_KEYS.contains(&key.as_str()) {
                    *item = Value::Null;
                } else {
                    blank_volatile(item, ids);
                }
            }
        }
        _ => {}
    }
}

fn id_regex(project: &str) -> Regex {
    Regex::new(&format!(
        "^(?:{}-[0-9a-f]+|schema-[0-9a-f]+)$",
        regex::escape(project)
    ))
    .expect("id pattern compiles")
}

/// The normalisation state of one side of a case.
pub struct Normaliser {
    project: String,
    ids: Regex,
    root: String,
    id_numbers: HashMap<String, usize>,
    timestamp_numbers: HashMap<String, usize>,
}

impl Normaliser {
    /// `project` is the project name in the side's config (ids are
    /// `<project>-<hex>`); `root` is the side's project root, replaced by
    /// its canonical form.
    pub fn new(project: &str, root: &Path) -> Self {
        let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
        Self {
            project: project.to_owned(),
            ids: id_regex(project),
            root: root.to_string_lossy().into_owned(),
            id_numbers: HashMap::new(),
            timestamp_numbers: HashMap::new(),
        }
    }

    fn id(&mut self, id: &str) -> String {
        let next = self.id_numbers.len() + 1;
        let number = *self.id_numbers.entry(id.to_owned()).or_insert(next);
        format!("<id:{number}>")
    }

    fn timestamp(&mut self, value: &str) -> String {
        assert_go_timestamp(value);
        let next = self.timestamp_numbers.len() + 1;
        let number = *self
            .timestamp_numbers
            .entry(value.to_owned())
            .or_insert(next);
        format!("<ts:{number}>")
    }

    /// Normalises one stdout document in place.
    pub fn document(&mut self, doc: &mut Value) {
        sort_map_ordered_arrays(doc, &self.project);
        self.value(doc);
    }

    /// Normalises a stream of stdout documents in order.
    pub fn documents(&mut self, docs: &mut [Value]) {
        docs.iter_mut().for_each(|doc| self.document(doc));
    }

    /// Replaces the project root in free text, such as an error string read
    /// outside a document.
    pub fn text(&self, text: &str) -> String {
        if self.root.is_empty() {
            text.to_owned()
        } else {
            text.replace(&self.root, ROOT)
        }
    }

    fn value(&mut self, value: &mut Value) {
        match value {
            Value::String(text) => {
                if self.ids.is_match(text) {
                    *text = self.id(text);
                } else {
                    *text = self.text(text);
                }
            }
            Value::Array(items) => items.iter_mut().for_each(|item| self.value(item)),
            Value::Object(object) => {
                for (key, item) in object.iter_mut() {
                    if TIMESTAMP_KEYS.contains(&key.as_str()) {
                        let Value::String(text) = item else {
                            panic!("{key} is {item}, not a timestamp string");
                        };
                        *text = self.timestamp(text);
                    } else {
                        self.value(item);
                    }
                }
            }
            _ => {}
        }
    }

    /// Normalises a JSONL store file line by line: string values that are
    /// ids, and the values of `createdAt` and `updatedAt`, are replaced by
    /// their placeholders; every other byte is kept.
    pub fn jsonl(&mut self, bytes: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(bytes.len());
        for line in bytes.split_inclusive(|byte| *byte == b'\n') {
            self.jsonl_line(line, &mut out);
        }
        out
    }

    fn jsonl_line(&mut self, line: &[u8], out: &mut Vec<u8>) {
        let mut index = 0;
        // Set when the previous literal was a timestamp key; holds the key.
        let mut timestamp_key: Option<String> = None;
        while index < line.len() {
            if line[index] != b'"' {
                let byte = line[index];
                if timestamp_key.is_some() && !matches!(byte, b':' | b' ' | b'\t') {
                    let key = timestamp_key.take().unwrap_or_default();
                    panic!("{key} in a store line is not a timestamp string");
                }
                out.push(byte);
                index += 1;
                continue;
            }
            let Some(end) = string_end(line, index) else {
                // An unterminated literal (a truncated line): keep the rest.
                out.extend_from_slice(&line[index..]);
                return;
            };
            let literal = &line[index..=end];
            let decoded = std::str::from_utf8(literal)
                .ok()
                .and_then(|text| serde_json::from_str::<String>(text).ok());
            index = end + 1;
            if is_key(line, index) {
                timestamp_key = decoded.filter(|key| TIMESTAMP_KEYS.contains(&key.as_str()));
                out.extend_from_slice(literal);
                continue;
            }
            if let Some(key) = timestamp_key.take() {
                let value = decoded
                    .unwrap_or_else(|| panic!("{key} in a store line is not a valid string"));
                let placeholder = self.timestamp(&value);
                out.extend_from_slice(quoted(&placeholder).as_bytes());
            } else if let Some(value) = decoded.filter(|value| self.ids.is_match(value)) {
                let placeholder = self.id(&value);
                out.extend_from_slice(quoted(&placeholder).as_bytes());
            } else {
                out.extend_from_slice(literal);
            }
        }
    }
}

fn quoted(text: &str) -> String {
    serde_json::to_string(text).expect("a string serialises")
}

/// Index of the quote closing the string literal that opens at `start`.
fn string_end(line: &[u8], start: usize) -> Option<usize> {
    let mut index = start + 1;
    while index < line.len() {
        match line[index] {
            b'\\' => index += 2,
            b'"' => return Some(index),
            _ => index += 1,
        }
    }
    None
}

/// Whether the literal that ended just before `index` is an object key.
fn is_key(line: &[u8], index: usize) -> bool {
    line[index..]
        .iter()
        .find(|byte| !matches!(byte, b' ' | b'\t' | b'\r' | b'\n'))
        == Some(&b':')
}

/// Removes and returns exactly the runtime keys the Go tool does not print
/// (A4 differences): `resolvedFrom` for `render`, and the top-level `valid`,
/// `errors` and `warnings` for `validate` when the document has exactly one
/// result. For every other command, and for other documents, nothing is
/// removed.
pub fn strip_runtime_fields(command: &str, doc: &mut Value) -> Map<String, Value> {
    let mut removed = Map::new();
    let Some(object) = doc.as_object_mut() else {
        return removed;
    };
    let keys: &[&str] = match command {
        "render" => &["resolvedFrom"],
        "validate"
            if object
                .get("results")
                .and_then(Value::as_array)
                .is_some_and(|results| results.len() == 1) =>
        {
            &["valid", "errors", "warnings"]
        }
        _ => &[],
    };
    for key in keys {
        if let Some(value) = object.remove(*key) {
            removed.insert((*key).to_owned(), value);
        }
    }
    removed
}
