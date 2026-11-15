//! The A8 rewrite guard: refuses a rewrite of an expertise file that would lose data the
//! Go tool silently drops (plan s5-atlas-lib-2a, "Go data loss (A8)", P22).
//!
//! The Go tool has no counterpart: it rewrites such files and loses the data. Every
//! rewrite of an expertise file in s5-atlas-lib-2b and s5-atlas-lib-3 calls
//! [`check_lossless_rewrite`] inside the file's lock, before
//! [`crate::store::write_records`]. Reading is unchanged: `read_records` and read-only
//! commands never call the guard.

use std::collections::HashSet;
use std::fs;
use std::io::ErrorKind;
use std::path::Path;

use crate::error::{AtlasError, Result};
use crate::json::{self, Object, Value};
use crate::records::go_deep_equal;
use crate::registry::Registry;

/// Checks that rewriting the expertise file at `path` loses nothing, without writing to it
/// or taking a lock (the caller holds it).
///
/// The file's bytes are split on LF; every line `read_records` keeps (not blank and not
/// starting with `#` after trimming) whose 0-based record index is not in `replaced`
/// (records the command deletes or replaces on purpose) is checked. The first loss found
/// is an `AtlasError::Message` naming the path and the 1-based line:
///
/// * GDL3, a key repeated in any object of the line: `Refusing to rewrite <path>: line
///   <N> repeats the key "<key>", and rewriting would keep only its last value. The file
///   is unchanged.`
/// * GDL4, a legacy `outcome` (non-null, no `outcomes`) that is not an object or holds a
///   key other than status, duration, test_results and agent: `Refusing to rewrite
///   <path>: line <N> has a legacy "outcome" that cannot be converted to "outcomes"
///   without losing data. The file is unchanged.`
/// * GDL5, a legacy alias name whose value `apply_aliases` would drop because the
///   canonical field is already non-empty with a different value: `Refusing to rewrite
///   <path>: line <N> has "<legacy>", an old name of "<canonical>", whose value differs
///   from "<canonical>" and would be dropped. The file is unchanged.`
///
/// A missing file is `Ok`. Any other read failure is returned as an error rather than
/// passed: `read_records` reads such a file as holding no records, so a rewrite would
/// replace whatever it holds. A line that is not valid JSON is not checked here (the
/// caller's `read_records` already refused it). Within a line the checks run in the order
/// GDL3, GDL4, GDL5. Keys and field names are quoted as JSON strings in the message, so a
/// key holding a quote or a control character cannot break the message's line.
///
/// The duplicate-key scan is iterative and linear in the line's length, so a line nested
/// as deep as the parser accepts (10000 levels) is safe on a 2 MiB stack.
pub fn check_lossless_rewrite(path: &Path, registry: &Registry, replaced: &[usize]) -> Result<()> {
    let data = match fs::read(path) {
        Ok(data) => data,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
        Err(source) => {
            return Err(AtlasError::Io {
                path: path.to_path_buf(),
                source,
            })
        }
    };
    let replaced: HashSet<usize> = replaced.iter().copied().collect();
    let mut record_index = 0usize;
    for (idx, line) in data.split(|b| *b == b'\n').enumerate() {
        if !is_record_line(line) {
            continue;
        }
        let index = record_index;
        record_index += 1;
        if replaced.contains(&index) {
            continue;
        }
        let Ok(value) = json::parse_bytes(line) else {
            continue;
        };
        if let Some(loss) = line_loss(line, value, registry) {
            return Err(AtlasError::Message(format!(
                "Refusing to rewrite {}: line {} {loss} The file is unchanged.",
                path.display(),
                idx + 1
            )));
        }
    }
    Ok(())
}

/// Whether `read_records` keeps `line` as a record: not blank and not starting with `#`
/// after trimming.
///
/// `read_records` trims a per-byte lossy decoding of the line; `from_utf8_lossy` may use
/// fewer U+FFFD for an invalid sequence, but U+FFFD is neither whitespace nor `#` and the
/// valid characters decode the same, so the answer is the same.
fn is_record_line(line: &[u8]) -> bool {
    let text = String::from_utf8_lossy(line);
    let trimmed = text.trim();
    !(trimmed.is_empty() || trimmed.starts_with('#'))
}

/// The loss a rewrite of this parsed line would cause, as the middle of the refusal
/// message, or `None`.
fn line_loss(line: &[u8], mut value: Value, registry: &Registry) -> Option<String> {
    if let Some(key) = first_repeated_key(line) {
        return Some(format!(
            "repeats the key {}, and rewriting would keep only its last value.",
            quoted(&key)
        ));
    }
    // Only an object row has fields; an array row reads as an empty record, which
    // write_records refuses (K7), and any other row is refused by read_records.
    let Value::Object(record) = &mut value else {
        return None;
    };
    if legacy_outcome_loses_data(record) {
        return Some(
            "has a legacy \"outcome\" that cannot be converted to \"outcomes\" without losing data."
                .to_owned(),
        );
    }
    alias_loss(record, registry).map(|(legacy, canonical)| {
        let canonical = quoted(&canonical);
        format!(
            "has {}, an old name of {canonical}, whose value differs from {canonical} and would be dropped.",
            quoted(&legacy)
        )
    })
}

/// `s` as a JSON string, quotes included.
fn quoted(s: &str) -> String {
    json::to_compact_string(&Value::String(s.to_owned())).unwrap_or_else(|_| format!("\"{s}\""))
}

/// The keys of the legacy `outcome` that `read_records` converts into `outcomes`.
const LEGACY_OUTCOME_KEYS: [&str; 4] = ["status", "duration", "test_results", "agent"];

/// GDL4: a non-null `outcome` without `outcomes` is converted by `read_records`
/// (store.rs `rewrite_legacy_outcome`), which keeps only the four keys of an object and
/// nothing of any other value.
fn legacy_outcome_loses_data(record: &Object) -> bool {
    let Some(legacy) = record.get("outcome") else {
        return false;
    };
    if legacy.is_null() || record.contains_key("outcomes") {
        return false;
    }
    match legacy.as_object() {
        Some(outcome) => outcome
            .keys()
            .any(|key| !LEGACY_OUTCOME_KEYS.contains(&key)),
        None => true,
    }
}

/// GDL5: replays, on `record`, what `read_records` does after parsing (the legacy outcome
/// conversion, then `registry::apply_aliases` in its order, canonical by canonical and
/// legacy name by legacy name) and returns the first `(legacy, canonical)` whose legacy
/// value is removed without the canonical field holding an equal value afterwards (Go
/// comparison, [`go_deep_equal`]).
///
/// This catches a canonical field that is already non-empty with a different value, and
/// also a second legacy name whose value differs from the value an earlier legacy name
/// filled in. An alias filling an absent, null or `""` canonical field loses nothing.
fn alias_loss(record: &mut Object, registry: &Registry) -> Option<(String, String)> {
    convert_legacy_outcome(record);
    let type_name = record
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let definition = registry.get(type_name)?;
    for (canonical, names) in &definition.aliases {
        for legacy in names {
            // Same steps as apply_aliases (registry.rs), with a check after each removal.
            let Some(value) = record.get(legacy).cloned() else {
                continue;
            };
            let empty = match record.get(canonical) {
                None | Some(Value::Null) => true,
                Some(Value::String(s)) => s.is_empty(),
                Some(_) => false,
            };
            if empty {
                record.insert_unordered(canonical.clone(), value.clone());
            }
            record.remove(legacy);
            let kept = record
                .get(canonical)
                .is_some_and(|now| go_deep_equal(now, &value));
            // Null and empty legacy values carry no data (decided items 3/6).
            // Go's successful rewrite and actual reader are equal before/after.
            let no_data =
                value.is_null() || matches!(&value, Value::String(text) if text.is_empty());
            if !kept && !no_data {
                return Some((legacy.clone(), canonical.clone()));
            }
        }
    }
    None
}

/// What `read_records` does to a legacy `outcome` before applying aliases (store.rs
/// `rewrite_legacy_outcome`, private there), so the alias replay sees the same fields.
fn convert_legacy_outcome(record: &mut Object) {
    let Some(legacy) = record.get("outcome") else {
        return;
    };
    if legacy.is_null() || record.contains_key("outcomes") {
        return;
    }
    let mut normalized = Object::new();
    if let Some(legacy) = legacy.as_object() {
        for key in LEGACY_OUTCOME_KEYS {
            if let Some(value) = legacy.get(key) {
                normalized.insert_unordered(key, value.clone());
            }
        }
    }
    record.insert_unordered("outcomes", Value::Array(vec![Value::Object(normalized)]));
    record.remove("outcome");
}

/// An open array or object of [`first_repeated_key`]'s scan.
enum ScanFrame {
    Array,
    /// The keys seen so far, decoded, and whether the next string is a key.
    Object(HashSet<String>, bool),
}

/// GDL3: the first key, in text order, that an object of `line` holds twice, compared
/// after decoding as the parser decodes it (escapes, invalid UTF-8 and lone surrogates
/// as U+FFFD).
///
/// `line` has already been parsed successfully, so it is well-formed JSON. One pass over
/// the bytes with an explicit stack of open containers: no recursion per nesting level,
/// and each byte is looked at a bounded number of times (each key is decoded once, from
/// its own slice), so the scan is linear in the line's length.
fn first_repeated_key(line: &[u8]) -> Option<String> {
    let mut stack: Vec<ScanFrame> = Vec::new();
    let mut i = 0;
    while i < line.len() {
        match line[i] {
            b'{' => stack.push(ScanFrame::Object(HashSet::new(), true)),
            b'[' => stack.push(ScanFrame::Array),
            b'}' | b']' => {
                stack.pop();
            }
            b',' => {
                if let Some(ScanFrame::Object(_, expect_key)) = stack.last_mut() {
                    *expect_key = true;
                }
            }
            b'"' => {
                let start = i;
                i += 1;
                while i < line.len() {
                    match line[i] {
                        b'\\' => i += 2,
                        b'"' => break,
                        _ => i += 1,
                    }
                }
                let end = (i + 1).min(line.len());
                if let Some(ScanFrame::Object(keys, expect_key)) = stack.last_mut() {
                    if *expect_key {
                        *expect_key = false;
                        // The parser's own decoding of the key, so equal keys are the
                        // keys Go's decoder merges.
                        if let Ok(Value::String(key)) =
                            json::parse_bytes(&line[start..end]).as_mut()
                        {
                            // `Value` implements Drop, so the text is taken, not moved.
                            let key = std::mem::take(key);
                            if keys.contains(&key) {
                                return Some(key);
                            }
                            keys.insert(key);
                        }
                    }
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::json::Value;
    use crate::yaml;
    use std::fs;
    use std::path::PathBuf;

    fn fixture_registry() -> Registry {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/s5-atlas-lib-2/custom-types.yaml"
        );
        let text = fs::read_to_string(path).expect("read custom-types.yaml");
        let parsed = yaml::parse(&text).expect("parse custom-types.yaml");
        let Value::Object(config) = &parsed else {
            panic!("custom-types.yaml is not a mapping");
        };
        Registry::from_config(Some(config)).expect("registry from custom-types.yaml")
    }

    /// Writes `body` as `d.jsonl` in a new temporary directory.
    fn file(body: &[u8]) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("d.jsonl");
        fs::write(&path, body).expect("write");
        (dir, path)
    }

    fn check(body: &str, replaced: &[usize]) -> Result<()> {
        let (_dir, path) = file(body.as_bytes());
        check_lossless_rewrite(&path, &fixture_registry(), replaced)
    }

    fn refusal(body: &str) -> String {
        let (_dir, path) = file(body.as_bytes());
        let err = check_lossless_rewrite(&path, &fixture_registry(), &[])
            .expect_err("the rewrite must be refused");
        let text = err.to_string();
        let prefix = format!("Refusing to rewrite {}: ", path.display());
        let rest = text
            .strip_prefix(&prefix)
            .unwrap_or_else(|| panic!("message does not name the path: {text}"));
        let rest = rest
            .strip_suffix(" The file is unchanged.")
            .unwrap_or_else(|| panic!("message does not end as planned: {text}"));
        rest.to_owned()
    }

    const BASE: &str = r#""classification":"tactical","recorded_at":"2024-01-01T00:00:00.000Z""#;

    #[test]
    fn duplicate_key_at_top_and_nested_is_refused() {
        let top = format!(r#"{{"type":"convention","content":"a","content":"b",{BASE}}}"#);
        assert_eq!(
            refusal(&top),
            "line 1 repeats the key \"content\", and rewriting would keep only its last value."
        );
        let nested = format!(
            r#"{{"type":"convention","content":"a",{BASE},"evidence":{{"commit":"x","commit":"y"}}}}"#
        );
        assert_eq!(
            refusal(&nested),
            "line 1 repeats the key \"commit\", and rewriting would keep only its last value."
        );
        let in_array = format!(
            r#"{{"type":"convention","content":"a",{BASE},"outcomes":[{{"status":"success"}},{{"status":"success","status":"failure"}}]}}"#
        );
        assert_eq!(
            refusal(&in_array),
            "line 1 repeats the key \"status\", and rewriting would keep only its last value."
        );
        // Keys are compared after decoding escapes, as Go's decoder does.
        let escaped = format!(r#"{{"type":"convention","content":"a","\u0063ontent":"b",{BASE}}}"#);
        assert_eq!(
            refusal(&escaped),
            "line 1 repeats the key \"content\", and rewriting would keep only its last value."
        );
        // The same key in sibling objects, or as a key and a value, is not a repeat.
        let fine = format!(
            r#"{{"type":"convention","content":"content",{BASE},"evidence":{{"commit":"x"}},"outcomes":[{{"status":"success"}},{{"status":"failure"}}]}}"#
        );
        assert!(check(&fine, &[]).is_ok());
    }

    #[test]
    fn legacy_outcome_with_four_keys_is_allowed() {
        for outcome in [
            r#"{"status":"success","duration":3,"test_results":"ok","agent":"a"}"#,
            r#"{"status":"failure"}"#,
            r#"{}"#,
            "null",
        ] {
            let line =
                format!(r#"{{"type":"convention","content":"a",{BASE},"outcome":{outcome}}}"#);
            assert!(check(&line, &[]).is_ok(), "{outcome}");
        }
        // With `outcomes` present Go keeps `outcome` as it is: nothing is lost.
        let line = format!(
            r#"{{"type":"convention","content":"a",{BASE},"outcome":"done","outcomes":[]}}"#
        );
        assert!(check(&line, &[]).is_ok());
    }

    #[test]
    fn legacy_outcome_extra_key_or_scalar_is_refused() {
        let want = "line 1 has a legacy \"outcome\" that cannot be converted to \"outcomes\" without losing data.";
        for outcome in [
            r#"{"status":"success","notes":"n"}"#,
            r#""done""#,
            "[]",
            "0",
            "false",
        ] {
            let line =
                format!(r#"{{"type":"convention","content":"a",{BASE},"outcome":{outcome}}}"#);
            assert_eq!(refusal(&line), want, "{outcome}");
        }
    }

    #[test]
    fn alias_filling_empty_field_is_allowed() {
        for title in [None, Some(r#""""#), Some("null")] {
            let title = title.map_or(String::new(), |t| format!(r#""title":{t},"#));
            let line =
                format!(r#"{{"type":"runbook",{title}"heading":"Restart","steps":"s",{BASE}}}"#);
            assert!(check(&line, &[]).is_ok(), "{line}");
        }
        // Equal values lose nothing, numbers compared as Go compares them.
        let line = format!(
            r#"{{"type":"runbook","title":"Restart","heading":"Restart","steps":"s",{BASE}}}"#
        );
        assert!(check(&line, &[]).is_ok());
        let line = format!(r#"{{"type":"runbook","title":"t","steps":1,"procedure":1.0,{BASE}}}"#);
        assert!(check(&line, &[]).is_ok());
        // An alias of another type is an ordinary field there.
        let line = format!(r#"{{"type":"convention","content":"a","heading":"x",{BASE}}}"#);
        assert!(check(&line, &[]).is_ok());
    }

    #[test]
    fn alias_conflict_is_refused() {
        let line = format!(r#"{{"type":"runbook","title":"A","heading":"B","steps":"s",{BASE}}}"#);
        assert_eq!(
            refusal(&line),
            "line 1 has \"heading\", an old name of \"title\", whose value differs from \"title\" and would be dropped."
        );
        let line = format!(r#"{{"type":"runbook","title":"t","steps":"a","procedure":5,{BASE}}}"#);
        assert_eq!(
            refusal(&line),
            "line 1 has \"procedure\", an old name of \"steps\", whose value differs from \"steps\" and would be dropped."
        );
        // A canonical false or 0 is not empty.
        let line =
            format!(r#"{{"type":"runbook","title":false,"heading":"x","steps":"s",{BASE}}}"#);
        assert!(refusal(&line).starts_with("line 1 has \"heading\""));
    }

    #[test]
    fn two_legacy_names_with_different_values_are_refused() {
        // No canonical field: apply_aliases fills `title` from `heading`, then drops
        // `headline`, whose value differs. The guard replays apply_aliases in order.
        let line =
            format!(r#"{{"type":"runbook","heading":"A","headline":"B","steps":"s",{BASE}}}"#);
        assert_eq!(
            refusal(&line),
            "line 1 has \"headline\", an old name of \"title\", whose value differs from \"title\" and would be dropped."
        );
        let line =
            format!(r#"{{"type":"runbook","heading":"A","headline":"A","steps":"s",{BASE}}}"#);
        assert!(check(&line, &[]).is_ok());
        // An empty first legacy value leaves the canonical empty, so the second fills it.
        let line =
            format!(r#"{{"type":"runbook","heading":"","headline":"B","steps":"s",{BASE}}}"#);
        assert!(check(&line, &[]).is_ok());
    }

    #[test]
    fn replaced_records_are_not_checked() {
        let ok = format!(r#"{{"type":"convention","content":"ok",{BASE}}}"#);
        let dup = format!(r#"{{"type":"convention","content":"a","content":"b",{BASE}}}"#);
        let body = format!("# header\n{ok}\n\n{dup}\n  # note\n{ok}\n");
        // The bad line is record 1 (comments and blank lines are not records).
        assert!(check(&body, &[1]).is_ok());
        assert!(check(&body, &[0, 1, 2]).is_ok());
        let err = check(&body, &[0, 2]).expect_err("record 1 is kept");
        assert!(
            err.to_string()
                .contains(": line 4 repeats the key \"content\""),
            "{err}"
        );
        assert!(check(&body, &[]).is_err());
    }

    #[test]
    fn comments_blank_lines_and_number_forms_are_allowed() {
        let mut body = Vec::new();
        body.extend_from_slice(b"# a comment\n\n   \n\t# indented comment\n");
        body.extend_from_slice(
            format!(
                r#"{{"type":"convention","content":"n","x":1.0,"y":1e2,"z":12345678901234567890,"w":-0.0,{BASE}}}"#
            )
            .as_bytes(),
        );
        body.push(b'\n');
        // Escapes and invalid UTF-8 inside a string (copied, K3).
        body.extend_from_slice(br#"{"type":"convention","content":"caf\u00e9 \ud800 "#);
        body.extend_from_slice(b"\xff\xfe");
        body.extend_from_slice(format!(r#"",{BASE}}}"#).as_bytes());
        body.extend_from_slice(b"\r\n");
        let (_dir, path) = file(&body);
        assert!(check_lossless_rewrite(&path, &fixture_registry(), &[]).is_ok());
        // A missing file is fine; an empty one too.
        let missing = path.with_file_name("missing.jsonl");
        assert!(check_lossless_rewrite(&missing, &fixture_registry(), &[]).is_ok());
        assert!(check("", &[]).is_ok());
    }

    #[test]
    fn message_names_path_line_and_key() {
        let dup =
            format!(r#"{{"type":"convention","content":"a",{BASE},"tags":["x"],"tags":["y"]}}"#);
        let body = format!("# c\n\n{dup}\n");
        let (_dir, path) = file(body.as_bytes());
        let err = check_lossless_rewrite(&path, &fixture_registry(), &[]).expect_err("refused");
        assert_eq!(
            err.to_string(),
            format!(
                "Refusing to rewrite {}: line 3 repeats the key \"tags\", and rewriting would keep only its last value. The file is unchanged.",
                path.display()
            )
        );
        assert!(matches!(err, AtlasError::Message(_)));
        // The first loss in file order is reported.
        let outcome = format!(r#"{{"type":"convention","content":"b",{BASE},"outcome":"x"}}"#);
        let body = format!("{outcome}\n{dup}\n");
        let (_dir, path) = file(body.as_bytes());
        let err = check_lossless_rewrite(&path, &fixture_registry(), &[]).expect_err("refused");
        assert!(
            err.to_string()
                .contains(": line 1 has a legacy \"outcome\""),
            "{err}"
        );
    }

    #[test]
    fn file_is_not_touched() {
        let dup = format!(r#"{{"type":"convention","content":"a","content":"b",{BASE}}}"#);
        let ok = format!(r#"{{"type":"convention","content":"ok",{BASE}}}"#);
        for body in [format!("{dup}\n"), format!("{ok}\n")] {
            let (dir, path) = file(body.as_bytes());
            let before = fs::metadata(&path)
                .expect("metadata")
                .modified()
                .expect("mtime");
            let _ = check_lossless_rewrite(&path, &fixture_registry(), &[]);
            assert_eq!(fs::read(&path).expect("read"), body.as_bytes());
            let after = fs::metadata(&path)
                .expect("metadata")
                .modified()
                .expect("mtime");
            assert_eq!(before, after);
            let names: Vec<String> = fs::read_dir(dir.path())
                .expect("read_dir")
                .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
                .collect();
            assert_eq!(names, ["d.jsonl"]);
        }
    }

    #[test]
    fn unreadable_file_and_odd_keys() {
        // A read failure other than a missing file is not passed: read_records would read
        // no records and a rewrite would replace the file's contents.
        let dir = tempfile::tempdir().expect("tempdir");
        let err = check_lossless_rewrite(dir.path(), &fixture_registry(), &[])
            .expect_err("a directory cannot be read as a file");
        assert!(matches!(err, AtlasError::Io { .. }));
        // Keys are quoted as JSON strings in the message.
        let line = format!(r#"{{"type":"convention","content":"a",{BASE},"a\"\nb":1,"a\"\nb":2}}"#);
        assert_eq!(
            refusal(&line),
            r#"line 1 repeats the key "a\"\nb", and rewriting would keep only its last value."#
        );
        // Braces, brackets and commas inside strings do not confuse the key scan.
        let line = format!(
            r#"{{"type":"convention","content":"{{\"content\":1,[","x":"}}],\"x\":",{BASE}}}"#
        );
        assert!(check(&line, &[]).is_ok());
        // A line that is not JSON is read_records' refusal, not the guard's.
        assert!(check("{\"a\":1,\"a\":\n", &[]).is_ok());
    }

    #[test]
    #[ignore = "hostile input: run under prlimit (PIPELINE rule 10)"]
    fn hostile_deep_line_with_duplicate_at_bottom() {
        // 10000 nested objects, the parser's limit, with the repeat in the innermost.
        let depth = 10_000;
        let mut line = String::with_capacity(depth * 8);
        for _ in 0..depth - 1 {
            line.push_str(r#"{"a":"#);
        }
        line.push_str(r#"{"k":1,"k":2}"#);
        for _ in 0..depth - 1 {
            line.push('}');
        }
        line.push('\n');
        let (_dir, path) = file(line.as_bytes());
        let handle = std::thread::Builder::new()
            .stack_size(2 * 1024 * 1024)
            .spawn(move || {
                check_lossless_rewrite(&path, &fixture_registry(), &[]).map_err(|e| e.to_string())
            })
            .expect("spawn");
        let result = handle.join().expect("no stack overflow");
        let err = result.expect_err("refused");
        assert!(err.contains(": line 1 repeats the key \"k\""), "{err}");
    }
}
