//! The A8 helper: refusing a rewrite of `issues.jsonl` that would lose data
//! the Go tool loses (s4-missions-lib-2 t4; `go_data_loss_cases` of the plan).
//!
//! Go's `ReadIssues` skips a line it cannot decode, keeps only the last of
//! two records with one id, and keeps only the last value of a key repeated
//! in a record. `WriteIssues` then writes only what was read, so a command
//! that rewrites the file drops those lines and values. A command that
//! changes `blocks`, `blockedBy` or `labels` also replaces a raw value that
//! Go decodes as something else (a value that is not a list, an element that
//! is not a string, a string that is not valid UTF-8 or holds a lone
//! surrogate escape). These functions find such a case and give the text of
//! the refusal; they write nothing and take no lock (the caller holds the
//! lock of `issues.jsonl`).

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io;

use crate::command::Failure;
use crate::error::Error;
use crate::json;
use crate::model::Ticket;
use crate::store::{trim_space, DataFile, Store};

/// The text every refusal ends with.
const NOTHING_WRITTEN: &str = "Nothing was written.";

/// One decoded line of `issues.jsonl`.
struct Record<'a> {
    /// The line number, from 1.
    number: usize,
    /// The line, trimmed as Go's reader trims it.
    line: &'a [u8],
    /// The ticket id.
    id: String,
}

/// The first file-level case in `content`, as the text of the refusal:
/// a line Go skips, a key repeated at the top level of a record, or an id
/// held by two records. `None` when rewriting the file loses no line.
#[allow(
    dead_code,
    reason = "called by close, dep, label and unblock (t6) and by s4-missions-lib-4"
)]
pub(in crate::command) fn lossy_rewrite_problem(content: &[u8]) -> Option<String> {
    scan_file(content).err()
}

/// [`lossy_rewrite_problem`] on the `issues.jsonl` of `store` (a missing file
/// is empty), as the error Go-style commands print. Reads once, writes
/// nothing, takes no lock.
#[allow(
    dead_code,
    reason = "called by close, dep, label and unblock (t6) and by s4-missions-lib-4"
)]
pub(in crate::command) fn refuse_lossy_issues_rewrite(
    store: &Store,
) -> std::result::Result<(), Failure> {
    let content = read_issues(store)?;
    match lossy_rewrite_problem(&content) {
        Some(text) => Err(Failure::Message(text)),
        None => Ok(()),
    }
}

/// The first case of [`lossy_rewrite_problem`] or, when there is none, the
/// first field case: a command changed `blocks`, `blockedBy` or `labels` of
/// `after[k]` from `before[k]`, and the raw value on the line of that ticket
/// is not exactly what Go decodes. The case on the lowest line comes first;
/// on one line `blocks`, `blockedBy`, then `labels`.
#[allow(
    dead_code,
    reason = "called by close, dep, label and unblock (t6) and by s4-missions-lib-4"
)]
pub(in crate::command) fn lossy_change_problem(
    content: &[u8],
    before: &[Ticket],
    after: &[Ticket],
) -> Option<String> {
    let records = match scan_file(content) {
        Ok(records) => records,
        Err(text) => return Some(text),
    };
    field_problem(&records, before, after)
}

/// [`lossy_change_problem`] on the `issues.jsonl` of `store` (a missing file
/// is empty). Reads once, writes nothing, takes no lock.
#[allow(
    dead_code,
    reason = "called by close, dep, label and unblock (t6) and by s4-missions-lib-4"
)]
pub(in crate::command) fn refuse_lossy_ticket_changes(
    store: &Store,
    before: &[Ticket],
    after: &[Ticket],
) -> std::result::Result<(), Failure> {
    let content = read_issues(store)?;
    match lossy_change_problem(&content, before, after) {
        Some(text) => Err(Failure::Message(text)),
        None => Ok(()),
    }
}

fn read_issues(store: &Store) -> std::result::Result<Vec<u8>, Failure> {
    let path = store.path(DataFile::Issues);
    match fs::read(&path) {
        Ok(content) => Ok(content),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(Failure::from(Error::io("read", path, error))),
    }
}

/// Decodes every line as Go's reader does and returns the records, or the
/// text of the first file-level case in file order: (a) a line
/// `Ticket::from_json` rejects, else (c) a repeated top-level key, else (b)
/// an id seen on an earlier line.
fn scan_file(content: &[u8]) -> std::result::Result<Vec<Record<'_>>, String> {
    let mut records = Vec::new();
    let mut first_line_of: HashMap<String, usize> = HashMap::new();
    for (index, raw) in content.split(|byte| *byte == b'\n').enumerate() {
        let number = index + 1;
        let line = trim_space(raw);
        if line.is_empty() {
            continue;
        }
        let Some(ticket) = Ticket::from_json(line) else {
            return Err(refusal(&format!(
                "line {number} is not a ticket record the Go tool can read, and rewriting the file would drop it."
            )));
        };
        if let Some(key) = repeated_top_level_key(line) {
            return Err(refusal(&format!(
                "the record on line {number} repeats the key \"{key}\", and rewriting the file would keep only its last value."
            )));
        }
        let id = ticket.id().to_owned();
        if let Some(&first) = first_line_of.get(&id) {
            let holds = if id.is_empty() {
                "a record without an id".to_owned()
            } else {
                format!("the ticket \"{id}\"")
            };
            return Err(refusal(&format!(
                "lines {first} and {number} both hold {holds}, and rewriting the file would keep only the last one."
            )));
        }
        first_line_of.insert(id.clone(), number);
        records.push(Record { number, line, id });
    }
    Ok(records)
}

fn refusal(reason: &str) -> String {
    format!("Refusing to rewrite issues.jsonl: {reason} {NOTHING_WRITTEN}")
}

/// The first key (decoded) that appears a second time at the top level of
/// the valid JSON object `line`. One pass, no recursion: nesting is a depth
/// counter.
fn repeated_top_level_key(line: &[u8]) -> Option<String> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut depth = 0usize;
    let mut expect_key = false;
    let mut pos = 0;
    while pos < line.len() {
        match line[pos] {
            b'"' => {
                let end = string_end(line, pos);
                if depth == 1 && expect_key {
                    let key = json::unquote(&line[pos..end]);
                    if !seen.insert(key.clone()) {
                        return Some(key);
                    }
                    expect_key = false;
                }
                pos = end;
                continue;
            }
            b'{' | b'[' => {
                depth += 1;
                if depth == 1 {
                    expect_key = true;
                }
            }
            b'}' | b']' => depth = depth.saturating_sub(1),
            b',' if depth == 1 => expect_key = true,
            _ => {}
        }
        pos += 1;
    }
    None
}

/// The position after the string token that starts at `pos` (a quote) in
/// valid JSON.
fn string_end(src: &[u8], pos: usize) -> usize {
    let mut index = pos + 1;
    while index < src.len() {
        match src[index] {
            b'\\' => index += 2,
            b'"' => return index + 1,
            _ => index += 1,
        }
    }
    src.len()
}

/// What is wrong with the raw value of a list field.
#[derive(Clone, Copy)]
enum Offence {
    /// Case (d): not a list (and not `null`).
    NotAList,
    /// Case (e): an element that is not a string.
    NotAString,
    /// Case (f): a string that Go does not decode exactly.
    Undecodable,
}

/// The three fields, in Go's struct order.
fn fields(ticket: &Ticket) -> [(&'static str, &Option<Vec<String>>); 3] {
    [
        ("blocks", &ticket.blocks),
        ("blockedBy", &ticket.blocked_by),
        ("labels", &ticket.labels),
    ]
}

fn field_problem(records: &[Record<'_>], before: &[Ticket], after: &[Ticket]) -> Option<String> {
    let by_id: HashMap<&str, &Record<'_>> = records
        .iter()
        .map(|record| (record.id.as_str(), record))
        .collect();
    // The case with the lowest (line, field rank).
    let mut best: Option<(usize, usize, String)> = None;
    for (old, new) in before.iter().zip(after) {
        let Some(record) = by_id.get(old.id()) else {
            continue;
        };
        let old_fields = fields(old);
        let new_fields = fields(new);
        if (0..3).all(|rank| old_fields[rank].1 == new_fields[rank].1) {
            continue;
        }
        let Ok(members) = json::parse_object(record.line) else {
            continue;
        };
        for rank in 0..3 {
            if old_fields[rank].1 == new_fields[rank].1 {
                continue;
            }
            let key = old_fields[rank].0;
            let Some(offence) = members.get(key).and_then(field_offence) else {
                continue;
            };
            let candidate = (record.number, rank);
            if best
                .as_ref()
                .is_some_and(|(line, at, _)| (*line, *at) <= candidate)
            {
                continue;
            }
            best = Some((
                record.number,
                rank,
                field_refusal(record.number, old.id(), key, offence),
            ));
        }
    }
    best.map(|(_, _, text)| text)
}

fn field_refusal(number: usize, id: &str, field: &str, offence: Offence) -> String {
    match offence {
        Offence::NotAList => refusal(&format!(
            "line {number} (ticket \"{id}\") holds a \"{field}\" value that is not a list of strings, and changing the field would replace that value."
        )),
        Offence::NotAString => refusal(&format!(
            "line {number} (ticket \"{id}\") holds an element of \"{field}\" that is not a string, and changing the field would write it as an empty string."
        )),
        Offence::Undecodable => refusal(&format!(
            "line {number} (ticket \"{id}\") holds an element of \"{field}\" with invalid UTF-8 or a lone surrogate escape, and changing the field would write it as U+FFFD."
        )),
    }
}

/// Whether the raw value of a list field is something Go does not decode
/// exactly: `None` for an absent-like `null`, a list of exact strings.
fn field_offence(raw: &[u8]) -> Option<Offence> {
    if raw == b"null" {
        return None;
    }
    if raw.first() != Some(&b'[') {
        return Some(Offence::NotAList);
    }
    let mut pos = skip_ws(raw, 1);
    loop {
        match raw.get(pos) {
            None | Some(b']') => return None,
            Some(b'"') => {
                let end = string_end(raw, pos);
                if !string_is_exact(&raw[pos..end]) {
                    return Some(Offence::Undecodable);
                }
                pos = skip_ws(raw, end);
                if raw.get(pos) == Some(&b',') {
                    pos = skip_ws(raw, pos + 1);
                } else {
                    return None;
                }
            }
            Some(_) => return Some(Offence::NotAString),
        }
    }
}

/// Whether the string token (quotes included, valid JSON) decodes without a
/// replacement character: every unescaped byte sequence is valid UTF-8, a
/// high surrogate escape is followed at once by a low one, and no low one
/// stands alone.
fn string_is_exact(token: &[u8]) -> bool {
    let inner = &token[1..token.len() - 1];
    let mut pos = 0;
    while pos < inner.len() {
        if inner[pos] == b'\\' {
            if inner[pos + 1] != b'u' {
                pos += 2;
                continue;
            }
            let first = hex4(&inner[pos + 2..pos + 6]);
            pos += 6;
            if (0xd800..0xdc00).contains(&first) {
                let paired = inner.get(pos) == Some(&b'\\')
                    && inner.get(pos + 1) == Some(&b'u')
                    && inner.len() >= pos + 6
                    && (0xdc00..0xe000).contains(&hex4(&inner[pos + 2..pos + 6]));
                if !paired {
                    return false;
                }
                pos += 6;
            } else if (0xdc00..0xe000).contains(&first) {
                return false;
            }
        } else {
            let (ch, width) = json::decode_char(&inner[pos..]);
            if ch.is_none() {
                return false;
            }
            pos += width;
        }
    }
    true
}

fn hex4(digits: &[u8]) -> u32 {
    digits.iter().fold(0, |value, digit| {
        let nibble = match digit {
            b'0'..=b'9' => digit - b'0',
            b'a'..=b'f' => digit - b'a' + 10,
            _ => digit - b'A' + 10,
        };
        (value << 4) | u32::from(nibble)
    })
}

fn skip_ws(src: &[u8], mut pos: usize) -> usize {
    while matches!(src.get(pos), Some(b' ' | b'\t' | b'\n' | b'\r')) {
        pos += 1;
    }
    pos
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID_TWICE_A: &str = "Refusing to rewrite issues.jsonl: the record on line 1 repeats the key \"id\", and rewriting the file would keep only its last value. Nothing was written.";
    const LABELS_NOT_LIST: &str = "Refusing to rewrite issues.jsonl: line 1 (ticket \"a\") holds a \"labels\" value that is not a list of strings, and changing the field would replace that value. Nothing was written.";
    const LABELS_NOT_STRING: &str = "Refusing to rewrite issues.jsonl: line 1 (ticket \"a\") holds an element of \"labels\" that is not a string, and changing the field would write it as an empty string. Nothing was written.";
    const LABELS_UNDECODABLE: &str = "Refusing to rewrite issues.jsonl: line 1 (ticket \"a\") holds an element of \"labels\" with invalid UTF-8 or a lone surrogate escape, and changing the field would write it as U+FFFD. Nothing was written.";

    fn cat(parts: &[&[u8]]) -> Vec<u8> {
        parts.concat()
    }

    /// A record in Go's shape with `extra` members (each starting with a
    /// comma) after the standard ones.
    fn rec(id: &str, extra: &[u8]) -> Vec<u8> {
        cat(&[
            b"{\"id\":\"",
            id.as_bytes(),
            b"\",\"title\":\"t\",\"status\":\"open\",\"type\":\"task\",\"priority\":2,\"createdAt\":\"2026-01-01T00:00:00.000Z\",\"updatedAt\":\"2026-01-01T00:00:00.000Z\"",
            extra,
            b"}",
        ])
    }

    fn file(lines: &[Vec<u8>]) -> Vec<u8> {
        let mut out = Vec::new();
        for line in lines {
            out.extend_from_slice(line);
            out.push(b'\n');
        }
        out
    }

    fn ticket(line: &[u8]) -> Ticket {
        Ticket::from_json(line).expect("Ticket::from_json reads the line")
    }

    fn set(ticket: &Ticket, field: &str, value: &[&str]) -> Ticket {
        let mut changed = ticket.clone();
        let list = Some(value.iter().map(|item| (*item).to_owned()).collect());
        match field {
            "blocks" => changed.blocks = list,
            "blockedBy" => changed.blocked_by = list,
            _ => changed.labels = list,
        }
        changed
    }

    /// The problem when `field` of the one record `a`, holding `value`,
    /// is changed to `["zz"]`.
    fn changed(field: &str, value: &[u8]) -> Option<String> {
        let line = rec("a", &cat(&[b",\"", field.as_bytes(), b"\":", value]));
        let before = ticket(&line);
        let after = set(&before, field, &["zz"]);
        assert_ne!(before, after, "{field} {value:?}");
        lossy_change_problem(&file(&[line]), &[before], &[after])
    }

    fn labels(value: &[u8]) -> Option<String> {
        changed("labels", value)
    }

    fn store_with(content: &[u8]) -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("issues.jsonl"), content).unwrap();
        let store = Store::open(dir.path()).unwrap();
        (dir, store)
    }

    fn message(result: std::result::Result<(), Failure>) -> String {
        match result {
            Err(Failure::Message(text)) => text,
            other => panic!("expected a command error, got {other:?}"),
        }
    }

    #[test]
    fn skipped_lines_are_found() {
        let a3 = "Refusing to rewrite issues.jsonl: line 3 is not a ticket record the Go tool can read, and rewriting the file would drop it. Nothing was written.";
        let a2 = "Refusing to rewrite issues.jsonl: line 2 is not a ticket record the Go tool can read, and rewriting the file would drop it. Nothing was written.";
        let a1 = "Refusing to rewrite issues.jsonl: line 1 is not a ticket record the Go tool can read, and rewriting the file would drop it. Nothing was written.";
        // Blank lines count in the numbering.
        assert_eq!(
            lossy_rewrite_problem(b"\n\nnot json\n").as_deref(),
            Some(a3)
        );
        let good = rec("a", b"");
        assert_eq!(
            lossy_rewrite_problem(&cat(&[&good, b"\n[1,2]\n"])).as_deref(),
            Some(a2)
        );
        assert_eq!(
            lossy_rewrite_problem(&cat(&[&good, b"\n{\"id\":\"b\"\n"])).as_deref(),
            Some(a2)
        );
        assert_eq!(
            lossy_rewrite_problem(&cat(&[b"{\"id\":\"a\"} {\"id\":\"b\"}\n", &good, b"\n"]))
                .as_deref(),
            Some(a1)
        );
        // Invalid UTF-8 outside a string is not JSON.
        assert_eq!(
            lossy_rewrite_problem(&cat(&[&good, b"\n", &[0xff], b"\n"])).as_deref(),
            Some(a2)
        );
        // The first case in file order: a skipped line before a repeated id.
        let dup = file(&[b"nope".to_vec(), rec("a", b""), rec("a", b"")]);
        assert_eq!(lossy_rewrite_problem(&dup).as_deref(), Some(a1));
    }

    #[test]
    fn repeated_ids_are_found_including_records_without_an_id() {
        let b13 = "Refusing to rewrite issues.jsonl: lines 1 and 3 both hold the ticket \"a\", and rewriting the file would keep only the last one. Nothing was written.";
        let content = file(&[rec("a", b""), rec("b", b""), rec("a", b"")]);
        assert_eq!(lossy_rewrite_problem(&content).as_deref(), Some(b13));
        // Ids compare decoded.
        let escaped = file(&[
            rec("a", b""),
            rec("b", b""),
            b"{\"id\":\"\\u0061\"}".to_vec(),
        ]);
        assert_eq!(lossy_rewrite_problem(&escaped).as_deref(), Some(b13));
        // Three records with one id name the first earlier line each time.
        let three = file(&[rec("a", b""), rec("a", b""), rec("a", b"")]);
        assert_eq!(
            lossy_rewrite_problem(&three).as_deref(),
            Some("Refusing to rewrite issues.jsonl: lines 1 and 2 both hold the ticket \"a\", and rewriting the file would keep only the last one. Nothing was written.")
        );
        // Records without an id share the id "".
        let no_id = "Refusing to rewrite issues.jsonl: lines 1 and 2 both hold a record without an id, and rewriting the file would keep only the last one. Nothing was written.";
        assert_eq!(
            lossy_rewrite_problem(b"{\"title\":\"x\"}\n{\"title\":\"y\"}\n").as_deref(),
            Some(no_id)
        );
        assert_eq!(
            lossy_rewrite_problem(b"{}\n\n{\"id\":\"\"}\n").as_deref(),
            Some("Refusing to rewrite issues.jsonl: lines 1 and 3 both hold a record without an id, and rewriting the file would keep only the last one. Nothing was written.")
        );
        // One record without an id is not a case.
        assert_eq!(lossy_rewrite_problem(b"{\"title\":\"x\"}\n"), None);
    }

    #[test]
    fn repeated_top_level_keys_are_found() {
        let id_twice = "{\"id\":\"a\",\"id\":\"b\"}";
        assert_eq!(
            lossy_rewrite_problem(id_twice.as_bytes()).as_deref(),
            Some(ID_TWICE_A)
        );
        let title = file(&[rec("x", b""), rec("a", b",\"title\":\"again\"")]);
        assert_eq!(
            lossy_rewrite_problem(&title).as_deref(),
            Some("Refusing to rewrite issues.jsonl: the record on line 2 repeats the key \"title\", and rewriting the file would keep only its last value. Nothing was written.")
        );
        // Keys compare decoded, as Go's parseObject compares them.
        let escaped = rec("a", b",\"t\\u0069tle\":\"again\"");
        assert_eq!(
            lossy_rewrite_problem(&escaped).as_deref(),
            Some("Refusing to rewrite issues.jsonl: the record on line 1 repeats the key \"title\", and rewriting the file would keep only its last value. Nothing was written.")
        );
        // An unknown key repeated at the top level is a case too.
        let unknown = rec("a", b",\"zz\":1,\"zz\":[2]");
        assert_eq!(
            lossy_rewrite_problem(&unknown).as_deref(),
            Some("Refusing to rewrite issues.jsonl: the record on line 1 repeats the key \"zz\", and rewriting the file would keep only its last value. Nothing was written.")
        );
        // On one line the repeated key comes before the repeated id.
        let both = file(&[rec("a", b""), rec("a", b",\"title\":\"again\"")]);
        assert_eq!(
            lossy_rewrite_problem(&both).as_deref(),
            Some("Refusing to rewrite issues.jsonl: the record on line 2 repeats the key \"title\", and rewriting the file would keep only its last value. Nothing was written.")
        );
    }

    #[test]
    fn nested_repeated_keys_and_unknown_keys_are_not_cases() {
        let nested = rec(
            "a",
            b",\"extensions\":{\"k\":1,\"k\":2},\"x\":[{\"a\":1,\"a\":2},{\"a\":3}],\"unknown\":{\"id\":1,\"id\":2},\"zz\":\"{\\\"q\\\":1,\\\"q\\\":2}\"",
        );
        assert_eq!(lossy_rewrite_problem(&nested), None);
        assert_eq!(
            lossy_rewrite_problem(&file(std::slice::from_ref(&nested))),
            None
        );
        // A key that equals a nested key is not a repeat.
        let same = rec("a", b",\"title2\":{\"title\":\"inner\"}");
        assert_eq!(lossy_rewrite_problem(&same), None);
        assert_eq!(
            lossy_change_problem(&same, &[ticket(&same)], &[ticket(&same)]),
            None
        );
        // Unknown keys of different names.
        let unknown = file(&[rec("a", b",\"p\":1,\"q\":2"), rec("b", b",\"p\":1")]);
        assert_eq!(lossy_rewrite_problem(&unknown), None);
    }

    #[test]
    fn a_clean_file_and_blank_lines_pass() {
        assert_eq!(lossy_rewrite_problem(b""), None);
        assert_eq!(lossy_rewrite_problem(b"\n\n \t \n"), None);
        let content = cat(&[
            b"\n  ",
            &rec("a", b""),
            b"  \r\n\n\t\n",
            &rec("b", b",\"labels\":[\"x\"]"),
            b"\n",
        ]);
        assert_eq!(lossy_rewrite_problem(&content), None);
        assert_eq!(lossy_change_problem(&content, &[], &[]), None);
        // The store functions: a missing file is empty, a clean file passes,
        // and nothing is written.
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        assert!(refuse_lossy_issues_rewrite(&store).is_ok());
        assert!(refuse_lossy_ticket_changes(&store, &[], &[]).is_ok());
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
        let (dir, store) = store_with(&content);
        assert!(refuse_lossy_issues_rewrite(&store).is_ok());
        assert!(refuse_lossy_ticket_changes(&store, &[], &[]).is_ok());
        assert_eq!(fs::read(dir.path().join("issues.jsonl")).unwrap(), content);
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn the_refusal_texts_name_the_file_line_and_reason() {
        let cases: [(Vec<u8>, &str); 4] = [
            (
                b"x\n".to_vec(),
                "Refusing to rewrite issues.jsonl: line 1 is not a ticket record the Go tool can read, and rewriting the file would drop it. Nothing was written.",
            ),
            (
                file(&[rec("p-1", b""), rec("p-1", b"")]),
                "Refusing to rewrite issues.jsonl: lines 1 and 2 both hold the ticket \"p-1\", and rewriting the file would keep only the last one. Nothing was written.",
            ),
            (
                b"{}\n{}\n".to_vec(),
                "Refusing to rewrite issues.jsonl: lines 1 and 2 both hold a record without an id, and rewriting the file would keep only the last one. Nothing was written.",
            ),
            (
                b"{\"id\":\"p-1\",\"labels\":[],\"labels\":[]}\n".to_vec(),
                "Refusing to rewrite issues.jsonl: the record on line 1 repeats the key \"labels\", and rewriting the file would keep only its last value. Nothing was written.",
            ),
        ];
        for (content, text) in &cases {
            assert_eq!(lossy_rewrite_problem(content).as_deref(), Some(*text));
            assert_eq!(
                lossy_change_problem(content, &[], &[]).as_deref(),
                Some(*text)
            );
            let (dir, store) = store_with(content);
            assert_eq!(message(refuse_lossy_issues_rewrite(&store)), *text);
            assert_eq!(
                message(refuse_lossy_ticket_changes(&store, &[], &[])),
                *text
            );
            // Nothing written, no lock taken.
            assert_eq!(fs::read(dir.path().join("issues.jsonl")).unwrap(), *content);
            assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
        }
        // The three field texts through the store function.
        let line = rec(
            "p-7",
            b",\"blockedBy\":\"x\",\"blocks\":[1],\"labels\":[\"\\ud800\"]",
        );
        let content = file(std::slice::from_ref(&line));
        let before = ticket(&line);
        let (dir, store) = store_with(&content);
        let expected = [
            (
                "blockedBy",
                "Refusing to rewrite issues.jsonl: line 1 (ticket \"p-7\") holds a \"blockedBy\" value that is not a list of strings, and changing the field would replace that value. Nothing was written.",
            ),
            (
                "blocks",
                "Refusing to rewrite issues.jsonl: line 1 (ticket \"p-7\") holds an element of \"blocks\" that is not a string, and changing the field would write it as an empty string. Nothing was written.",
            ),
            (
                "labels",
                "Refusing to rewrite issues.jsonl: line 1 (ticket \"p-7\") holds an element of \"labels\" with invalid UTF-8 or a lone surrogate escape, and changing the field would write it as U+FFFD. Nothing was written.",
            ),
        ];
        for (field, text) in expected {
            let after = set(&before, field, &["zz"]);
            assert_eq!(
                message(refuse_lossy_ticket_changes(
                    &store,
                    std::slice::from_ref(&before),
                    &[after]
                )),
                text
            );
        }
        assert_eq!(fs::read(dir.path().join("issues.jsonl")).unwrap(), content);
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn deep_and_long_records_are_scanned_within_bounds() {
        // One record nested 10,000 levels deep (the object and 9,999 arrays),
        // the deepest Go accepts.
        let deep = cat(&[b",\"deep\":", &b"[".repeat(9_999), &b"]".repeat(9_999)]);
        let line = rec("a", &deep);
        assert!(line.len() < 21_000);
        assert!(Ticket::from_json(&line).is_some());
        let content = file(std::slice::from_ref(&line));
        assert_eq!(lossy_rewrite_problem(&content), None);
        // A repeated key after the deep value is still found.
        let repeated = file(&[rec("a", &cat(&[&deep, b",\"id\":\"b\""]))]);
        assert_eq!(
            lossy_rewrite_problem(&repeated).as_deref(),
            Some(ID_TWICE_A)
        );
        // A changed list after the deep value is read.
        let tail = rec("a", &cat(&[&deep, b",\"labels\":[\"a\",5]"]));
        let before = ticket(&tail);
        let after = set(&before, "labels", &["zz"]);
        assert_eq!(
            lossy_change_problem(&file(&[tail]), &[before], &[after]).as_deref(),
            Some(LABELS_NOT_STRING)
        );

        // One line of 1 MiB: a long string, then a long list.
        let text = cat(&[b",\"description\":\"", &b"x".repeat(1 << 20), b"\""]);
        let long = rec("a", &text);
        assert!(long.len() > 1 << 20);
        assert_eq!(lossy_rewrite_problem(&file(&[long])), None);
        let items = b"\"aaaaaaaa\",".repeat(100_000);
        let list = cat(&[b",\"labels\":[", &items, b"\"last\"]"]);
        let record = rec("a", &list);
        assert!(record.len() > 1 << 20);
        let before = ticket(&record);
        let after = set(&before, "labels", &["zz"]);
        let clean = file(&[record]);
        assert_eq!(
            lossy_change_problem(
                &clean,
                std::slice::from_ref(&before),
                std::slice::from_ref(&after)
            ),
            None
        );
        let list = cat(&[b",\"labels\":[", &items, b"\"last\",true]"]);
        let record = rec("a", &list);
        let before = ticket(&record);
        let after = set(&before, "labels", &["zz"]);
        assert_eq!(
            lossy_change_problem(&file(&[record]), &[before], &[after]).as_deref(),
            Some(LABELS_NOT_STRING)
        );
    }

    #[test]
    fn a_changed_field_that_is_not_a_list_is_found() {
        for value in [&b"\"urgent\""[..], b"5", b"true", b"{}"] {
            assert_eq!(labels(value).as_deref(), Some(LABELS_NOT_LIST), "{value:?}");
        }
        // The same for the other two fields.
        assert_eq!(
            changed("blocks", b"\"p-b\"").as_deref(),
            Some("Refusing to rewrite issues.jsonl: line 1 (ticket \"a\") holds a \"blocks\" value that is not a list of strings, and changing the field would replace that value. Nothing was written.")
        );
        assert_eq!(
            changed("blockedBy", b"{\"x\":1}").as_deref(),
            Some("Refusing to rewrite issues.jsonl: line 1 (ticket \"a\") holds a \"blockedBy\" value that is not a list of strings, and changing the field would replace that value. Nothing was written.")
        );
    }

    #[test]
    fn a_changed_list_with_a_non_string_element_is_found() {
        for value in [
            &b"[\"a\",5]"[..],
            b"[\"a\",null]",
            b"[\"a\",true]",
            b"[\"a\",{}]",
            b"[\"a\",[]]",
        ] {
            assert_eq!(
                labels(value).as_deref(),
                Some(LABELS_NOT_STRING),
                "{value:?}"
            );
        }
        // A whitespace-spread list and a first element that is not a string.
        assert_eq!(labels(b"[ 1 , \"a\" ]").as_deref(), Some(LABELS_NOT_STRING));
    }

    #[test]
    fn a_changed_list_with_an_undecodable_string_is_found() {
        let ff = [0xff];
        let fe = [0xfe];
        let surrogate_utf8 = [0xed, 0xa0, 0x80];
        let undecodable: [Vec<u8>; 7] = [
            cat(&[b"[\"a\",\"", &ff, b"\"]"]),
            cat(&[b"[\"", &fe, b"\"]"]),
            b"[\"\\ud800\"]".to_vec(),
            b"[\"\\udc00\"]".to_vec(),
            b"[\"\\ud800x\"]".to_vec(),
            cat(&[b"[\"", &surrogate_utf8, b"\"]"]),
            cat(&[b"[\"", &ff, b"\",5]"]),
        ];
        for value in &undecodable {
            assert_eq!(
                labels(value).as_deref(),
                Some(LABELS_UNDECODABLE),
                "{value:?}"
            );
        }
        // The first offending element decides between (e) and (f).
        assert_eq!(
            labels(&cat(&[b"[5,\"", &ff, b"\"]"])).as_deref(),
            Some(LABELS_NOT_STRING)
        );
        // A high surrogate escape followed by a non-low escape is lone.
        assert_eq!(
            labels(b"[\"\\ud800\\u0041\"]").as_deref(),
            Some(LABELS_UNDECODABLE)
        );
        // A lone low surrogate after a complete pair.
        assert_eq!(
            labels(b"[\"\\ud83d\\ude00\\ude00\"]").as_deref(),
            Some(LABELS_UNDECODABLE)
        );
    }

    #[test]
    fn changed_lists_go_decodes_exactly_pass() {
        let replacement = [0xef, 0xbf, 0xbd];
        let exact: [Vec<u8>; 9] = [
            b"null".to_vec(),
            b"[]".to_vec(),
            b"[\"\\u00e9\"]".to_vec(),
            b"[\"\\/\"]".to_vec(),
            b"[\"\\ud83d\\ude00\"]".to_vec(),
            cat(&[b"[\"", &replacement, b"\"]"]),
            b"[\"\\ufffd\"]".to_vec(),
            b"[\"<&>\"]".to_vec(),
            b"[\"a\",\"a\"]".to_vec(),
        ];
        for value in &exact {
            assert_eq!(labels(value), None, "{value:?}");
        }
        // A literal multi-byte character and an escaped quote or backslash.
        assert_eq!(labels("[\"é\\\"\\\\\"]".as_bytes()), None);
        // An absent field changed to a value is exact as well.
        let line = rec("a", b"");
        let before = ticket(&line);
        let after = set(&before, "labels", &["zz"]);
        assert_eq!(
            lossy_change_problem(&file(&[line]), &[before], &[after]),
            None
        );
    }

    #[test]
    fn unchanged_fields_are_never_cases() {
        let ff = [0xff];
        let fe = [0xfe];
        let values: [Vec<u8>; 6] = [
            b"\"urgent\"".to_vec(),
            b"[\"a\",5]".to_vec(),
            b"[\"a\",null]".to_vec(),
            cat(&[b"[\"a\",\"", &ff, b"\"]"]),
            b"[\"a\",\"\\ud800\"]".to_vec(),
            cat(&[b"[\"", &ff, b"\",\"", &fe, b"\"]"]),
        ];
        for value in &values {
            for field in ["blocks", "blockedBy", "labels"] {
                let line = rec("a", &cat(&[b",\"", field.as_bytes(), b"\":", value]));
                let before = ticket(&line);
                assert_eq!(
                    lossy_change_problem(
                        &file(&[line]),
                        std::slice::from_ref(&before),
                        std::slice::from_ref(&before)
                    ),
                    None,
                    "{field} {value:?}"
                );
            }
        }
        // A non-exact labels beside a changed exact blockedBy on one ticket.
        let line = rec("a", b",\"labels\":[\"a\",5],\"blockedBy\":[\"p\"]");
        let before = ticket(&line);
        let after = set(&before, "blockedBy", &["p", "q"]);
        assert_eq!(
            lossy_change_problem(&file(&[line]), &[before], &[after]),
            None
        );
        // A non-exact labels on another ticket.
        let first = rec("a", b",\"labels\":[\"a\",5]");
        let second = rec("b", b",\"labels\":[\"x\"]");
        let content = file(&[first.clone(), second.clone()]);
        let before = [ticket(&first), ticket(&second)];
        let after = [before[0].clone(), set(&before[1], "labels", &["x", "y"])];
        assert_eq!(lossy_change_problem(&content, &before, &after), None);
        // A scalar field the command sets is not a case.
        let mut closed = before[0].clone();
        closed.status = "closed".to_owned();
        assert_eq!(
            lossy_change_problem(&content, &before[..1], &[closed]),
            None
        );
    }

    #[test]
    fn field_cases_follow_file_cases_and_file_order() {
        let v2 = b",\"labels\":[\"a\",5]";
        // A skipped line 3 comes before a field case on line 1.
        let first = rec("a", v2);
        let second = rec("b", b"");
        let content = file(&[first.clone(), second.clone(), b"not json".to_vec()]);
        let before = [ticket(&first), ticket(&second)];
        let after = [set(&before[0], "labels", &["zz"]), before[1].clone()];
        assert_eq!(
            lossy_change_problem(&content, &before, &after).as_deref(),
            Some("Refusing to rewrite issues.jsonl: line 3 is not a ticket record the Go tool can read, and rewriting the file would drop it. Nothing was written.")
        );
        // A repeated id on lines 2 and 3 as well.
        let content = file(&[first.clone(), second.clone(), second.clone()]);
        assert_eq!(
            lossy_change_problem(&content, &before, &after).as_deref(),
            Some("Refusing to rewrite issues.jsonl: lines 2 and 3 both hold the ticket \"b\", and rewriting the file would keep only the last one. Nothing was written.")
        );
        // Changed V2 on lines 2 and 1: line 1, whatever the order of the lists.
        let second = rec("b", v2);
        let content = file(&[first.clone(), second.clone()]);
        let both = [ticket(&second), ticket(&first)];
        let changed_both = [
            set(&both[0], "labels", &["zz"]),
            set(&both[1], "labels", &["zz"]),
        ];
        assert_eq!(
            lossy_change_problem(&content, &both, &changed_both).as_deref(),
            Some(LABELS_NOT_STRING)
        );
        let only_second = [ticket(&first), set(&ticket(&second), "labels", &["zz"])];
        assert_eq!(
            lossy_change_problem(&content, &[ticket(&first), ticket(&second)], &only_second)
                .as_deref(),
            Some("Refusing to rewrite issues.jsonl: line 2 (ticket \"b\") holds an element of \"labels\" that is not a string, and changing the field would write it as an empty string. Nothing was written.")
        );
        // On one line: blocks, then blockedBy, then labels.
        let line = rec("a", b",\"blocks\":[1],\"blockedBy\":[2],\"labels\":\"x\"");
        let before = ticket(&line);
        let all = set(
            &set(&set(&before, "blocks", &["q"]), "blockedBy", &["q"]),
            "labels",
            &["q"],
        );
        let expected_blocks = "Refusing to rewrite issues.jsonl: line 1 (ticket \"a\") holds an element of \"blocks\" that is not a string, and changing the field would write it as an empty string. Nothing was written.";
        assert_eq!(
            lossy_change_problem(
                &file(std::slice::from_ref(&line)),
                std::slice::from_ref(&before),
                std::slice::from_ref(&all)
            )
            .as_deref(),
            Some(expected_blocks)
        );
        let no_blocks = set(&set(&before, "blockedBy", &["q"]), "labels", &["q"]);
        assert_eq!(
            lossy_change_problem(&file(&[line]), &[before], &[no_blocks]).as_deref(),
            Some("Refusing to rewrite issues.jsonl: line 1 (ticket \"a\") holds an element of \"blockedBy\" that is not a string, and changing the field would write it as an empty string. Nothing was written.")
        );
    }
}
