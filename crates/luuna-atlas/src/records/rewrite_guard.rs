//! The A8 rewrite guard: refuses a rewrite of an expertise file that would lose data the
//! Go tool silently drops (plan s5-atlas-lib-2a, "Go data loss (A8)", P22).
//!
//! The Go tool has no counterpart: it rewrites such files and loses the data. Every
//! rewrite of an expertise file in s5-atlas-lib-2b and s5-atlas-lib-3 calls
//! [`check_lossless_rewrite`] inside the file's lock, before
//! [`crate::store::write_records`]. Reading is unchanged: `read_records` and read-only
//! commands never call the guard.

use std::path::Path;

use crate::error::{AtlasError, Result};
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
/// A missing file is `Ok`. The duplicate-key scan is iterative and linear in the line's
/// length.
pub fn check_lossless_rewrite(path: &Path, registry: &Registry, replaced: &[usize]) -> Result<()> {
    // stub until t7
    let _ = (path, registry, replaced);
    Err(AtlasError::Message(
        "not implemented: records::rewrite_guard::check_lossless_rewrite".to_owned(),
    ))
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
