//! A8 raw retained-field checks for foundation updates and dependencies.
//! Whole-file rules reuse the native lib-2 scanner; field order and extensions
//! are handled here without recursive descent or dependency changes.
use crate::{
    command::Failure,
    error::Error,
    json,
    model::{Ticket, Value},
    store::{trim_space, DataFile, Store},
};
use std::{
    collections::{BTreeMap, HashSet},
    fs,
};
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(in crate::command) enum Field {
    Blocks,
    BlockedBy,
    Labels,
    Extensions,
}
impl Field {
    fn key(self) -> &'static str {
        match self {
            Self::Blocks => "blocks",
            Self::BlockedBy => "blockedBy",
            Self::Labels => "labels",
            Self::Extensions => "extensions",
        }
    }
}
pub(in crate::command) fn lossy_rewrite_problem(content: &[u8]) -> Option<String> {
    super::super::workflow::lossy::lossy_rewrite_problem(content)
}
pub(in crate::command) fn extensions_changed(
    before: &Option<BTreeMap<String, Value>>,
    after: &Option<BTreeMap<String, Value>>,
) -> Result<bool, Failure> {
    let encode = |value: &Option<BTreeMap<String, Value>>| -> Result<Vec<u8>, Failure> {
        let mut out = vec![];
        if let Some(map) = value.as_ref().filter(|m| !m.is_empty()) {
            json::encode_map(&mut out, map, true).map_err(|e| Failure::message(e.to_string()))?;
        }
        Ok(out)
    };
    Ok(encode(before)? != encode(after)?)
}
pub(in crate::command) fn refuse_lossy_issues_rewrite(
    store: &Store,
    changed: &[(&str, Field)],
) -> Result<(), Failure> {
    let path = store.path(DataFile::Issues);
    let content = match fs::read(&path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => vec![],
        Err(e) => return Err(Error::io("read", path, e).into()),
    };
    if let Some(text) = problem(&content, changed) {
        return Err(Failure::message(text));
    }
    Ok(())
}
fn problem(content: &[u8], changed: &[(&str, Field)]) -> Option<String> {
    if let Some(why) = lossy_rewrite_problem(content) {
        return Some(why);
    }
    let changed: HashSet<_> = changed.iter().copied().collect();
    for (i, raw) in content.split(|b| *b == b'\n').enumerate() {
        let raw = trim_space(raw);
        if raw.is_empty() {
            continue;
        }
        let ticket = Ticket::from_json(raw).expect("whole-file scan accepted the ticket");
        let members = json::parse_object(raw).expect("a ticket is an object");
        for field in [
            Field::Blocks,
            Field::BlockedBy,
            Field::Labels,
            Field::Extensions,
        ] {
            if !changed.contains(&(ticket.id(), field)) {
                continue;
            }
            let Some(raw) = members.get(field.key()) else {
                continue;
            };
            let why = if field == Field::Extensions {
                extensions_problem(raw)
            } else {
                list_problem(raw, field.key())
            };
            if let Some(why) = why {
                return Some(format!("Refusing to rewrite issues.jsonl: line {} (ticket \"{}\") holds {why}. Nothing was written.",i+1,ticket.id()));
            }
        }
    }
    None
}
fn list_problem(raw: &[u8], field: &str) -> Option<String> {
    if raw == b"null" {
        return None;
    }
    if raw.first() != Some(&b'[') {
        return Some(format!("a \"{field}\" value that is not a list of strings, and changing the field would replace that value"));
    }
    let mut i = skip_ws(raw, 1);
    while raw.get(i) != Some(&b']') {
        if raw.get(i) != Some(&b'"') {
            return Some(format!("an element of \"{field}\" that is not a string, and changing the field would write it as an empty string"));
        }
        let end = string_end(raw, i);
        if !string_is_exact(&raw[i..end]) {
            return Some(format!("an element of \"{field}\" with invalid UTF-8 or a lone surrogate escape, and changing the field would write it as U+FFFD"));
        }
        i = skip_ws(raw, end);
        if raw.get(i) == Some(&b',') {
            i = skip_ws(raw, i + 1)
        } else {
            break;
        }
    }
    None
}
fn decimal(text: &str) -> Option<(bool, String, i64)> {
    let (negative, text) = text.strip_prefix('-').map_or((false, text), |s| (true, s));
    let (base, exp) = text
        .find(['e', 'E'])
        .map_or((text, "0"), |i| (&text[..i], &text[i + 1..]));
    let fraction = base.find('.').map_or(0, |i| base.len() - i - 1);
    let mut digits = base.replace('.', "").trim_start_matches('0').to_owned();
    if digits.is_empty() {
        return Some((negative, "0".into(), 0));
    }
    let mut exponent = exp.parse::<i64>().ok()?.checked_sub(fraction as i64)?;
    while digits.ends_with('0') {
        digits.pop();
        exponent = exponent.checked_add(1)?;
    }
    Some((negative, digits, exponent))
}
fn extensions_problem(raw: &[u8]) -> Option<String> {
    if raw == b"null" {
        return None;
    }
    if raw.first() != Some(&b'{') {
        return Some("an \"extensions\" value that is not an object, and changing the field would replace that value".into());
    }
    struct Frame {
        object: bool,
        key: bool,
        seen: HashSet<String>,
    }
    let invalid = || {
        "a key or string in \"extensions\" with invalid UTF-8 or a lone surrogate escape, and changing the field would write it as U+FFFD".to_owned()
    };
    let mut stack: Vec<Frame> = vec![];
    let mut i = 0;
    while i < raw.len() {
        match raw[i] {
            b'{' => stack.push(Frame {
                object: true,
                key: true,
                seen: HashSet::new(),
            }),
            b'[' => stack.push(Frame {
                object: false,
                key: false,
                seen: HashSet::new(),
            }),
            b'}' | b']' => {
                stack.pop();
            }
            b',' => {
                if let Some(frame) = stack.last_mut() {
                    frame.key = frame.object;
                }
            }
            b'"' => {
                let end = string_end(raw, i);
                let token = &raw[i..end];
                if !string_is_exact(token) {
                    return Some(invalid());
                }
                if let Some(frame) = stack.last_mut() {
                    if frame.object && frame.key {
                        let key = json::unquote(token);
                        if !frame.seen.insert(key.clone()) {
                            return Some(format!("an object in \"extensions\" that repeats the key \"{key}\", and changing the field would keep only its last value"));
                        }
                        frame.key = false;
                    }
                }
                i = end;
                continue;
            }
            b'-' | b'0'..=b'9' => {
                let start = i;
                while i < raw.len()
                    && !matches!(raw[i], b',' | b'}' | b']' | b' ' | b'\n' | b'\r' | b'\t')
                {
                    i += 1
                }
                let token = std::str::from_utf8(&raw[start..i]).expect("JSON number is ASCII");
                let number = token.parse::<f64>().expect("valid JSON number");
                let encoded =
                    json::format_float(number).expect("readable ticket has finite known numbers");
                if decimal(token) != decimal(&encoded) {
                    return Some(format!("the number {token} in \"extensions\", and changing the field would write it as {encoded}"));
                }
                continue;
            }
            _ => (),
        }
        i += 1;
    }
    None
}
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
    fn data(key: &str, raw: &[u8]) -> Vec<u8> {
        [
            b"{\"id\":\"a\",\"".as_slice(),
            key.as_bytes(),
            b"\":",
            raw,
            b"}\n",
        ]
        .concat()
    }
    fn field(raw: &[u8], which: Field) -> Option<String> {
        problem(&data(which.key(), raw), &[("a", which)])
    }
    #[test]
    fn file_level_cases_are_found_in_line_order() {
        for raw in [
            b"{not json".as_slice(),
            b"[1]",
            b"null",
            br#"{"id":"a","extensions":{"n":1e999}}"#,
        ] {
            assert!(lossy_rewrite_problem(raw)
                .unwrap()
                .contains("line 1 is not a ticket record"));
        }
        assert!(lossy_rewrite_problem(b"{\"id\":\"a\"}\n{\"id\":\"a\"}\n")
            .unwrap()
            .contains("lines 1 and 2"));
        assert!(lossy_rewrite_problem(b"{}\n{}\n")
            .unwrap()
            .contains("a record without an id"));
        assert!(
            lossy_rewrite_problem(br#"{"id":"a","title":"x","title":"y"}"#)
                .unwrap()
                .contains("repeats the key \"title\"")
        );
        assert!(lossy_rewrite_problem(b"{not json\n{}\n{}\n")
            .unwrap()
            .contains("line 1"));
        assert!(
            lossy_rewrite_problem(b"{\"id\":\"a\"}\n{\"id\":\"a\",\"title\":1,\"title\":2}\n")
                .unwrap()
                .contains("repeats the key")
        );
    }
    #[test]
    fn repeated_top_level_keys_are_found_and_nested_ones_are_not() {
        assert!(lossy_rewrite_problem(br#"{"id":"x","a":1,"\u0061":2}"#)
            .unwrap()
            .contains("key \"a\""));
        assert!(lossy_rewrite_problem(br#"{"id":"x","e":{"a":1,"a":2}}"#).is_none());
    }
    #[test]
    fn files_go_rewrites_without_loss_pass() {
        for raw in [
            b"\n \t\r\n".as_slice(),
            br#" { "id" : "a", "title" : "<&>" } "#,
            br#"{"id":5}"#,
            br#"{"title":"no id"}"#,
            b"{\"id\":\"a\",\"title\":\"\xff\"}\n",
        ] {
            assert!(lossy_rewrite_problem(raw).is_none(), "{raw:?}");
        }
    }
    #[test]
    fn changed_lists_go_cannot_rewrite_are_found() {
        for raw in [br#""urgent""#.as_slice(), b"5", br#"{"x":1}"#] {
            assert!(field(raw, Field::Labels)
                .unwrap()
                .contains("not a list of strings"));
        }
        for raw in [br#"["a",5]"#.as_slice(), br#"["a",{}]"#, br#"["a",null]"#] {
            assert!(field(raw, Field::Labels).unwrap().contains("not a string"));
        }
        for raw in [
            b"[\"a\",\"\xff\"]".as_slice(),
            br#"["a","\ud800"]"#,
            br#"["a","\udc00"]"#,
        ] {
            assert!(field(raw, Field::Labels).unwrap().contains("U+FFFD"));
        }
    }
    #[test]
    fn changed_lists_go_decodes_exactly_pass() {
        for raw in [
            b"null".as_slice(),
            b"[]",
            br#"["a","a"]"#,
            br#"["\ufffd","\u00e9","\/","\ud83d\ude00"]"#,
            "[\"�\",\"é\"]".as_bytes(),
        ] {
            assert!(field(raw, Field::Labels).is_none(), "{raw:?}");
        }
    }
    #[test]
    fn unchanged_fields_are_never_cases() {
        for (key, raw) in [
            ("labels", br#"["a",5]"#.as_slice()),
            ("extensions", br#"{"n":9007199254740993}"#),
        ] {
            assert!(problem(&data(key, raw), &[]).is_none());
        }
        let raw = b"{\"id\":\"a\",\"labels\":5,\"blocks\":5}\n";
        let text = problem(raw, &[("a", Field::Labels), ("a", Field::Blocks)]).unwrap();
        assert!(text.contains("\"blocks\""));
    }
    #[test]
    fn changed_extensions_go_cannot_rewrite_are_found() {
        for raw in [br#""x""#.as_slice(), b"[1,2]", b"5", b"true"] {
            assert!(field(raw, Field::Extensions)
                .unwrap()
                .contains("not an object"));
        }
        for raw in [
            b"{\"a\":\"\xff\"}".as_slice(),
            b"{\"\xff\":1}",
            br#"{"a":"\ud800"}"#,
            br#"{"\ud800":1}"#,
            br#"{"o":["\udc00"]}"#,
        ] {
            assert!(field(raw, Field::Extensions).unwrap().contains("U+FFFD"));
        }
        for raw in [
            br#"{"n":9007199254740993}"#.as_slice(),
            br#"{"o":{"n":9007199254740993}}"#,
        ] {
            let text = field(raw, Field::Extensions).unwrap();
            assert!(text.contains("number 9007199254740993"));
            assert!(text.contains("9007199254740992"));
        }
        for raw in [br#"{"a":1,"a":2}"#.as_slice(), br#"{"o":{"a":1,"a":2}}"#] {
            assert!(field(raw, Field::Extensions).unwrap().contains("key \"a\""));
        }
        assert!(extensions_problem(br#"{"a":1,"a":2,"n":9007199254740993}"#)
            .unwrap()
            .contains("key \"a\""));
        assert!(extensions_problem(br#"{"n":9007199254740993,"a":1,"a":2}"#)
            .unwrap()
            .contains("number 9007199254740993"));
    }
    #[test]
    fn changed_extensions_go_reads_back_the_same_pass() {
        for raw in [
            b"null".as_slice(),
            b"{}",
            br#"{"a":null}"#,
            br#"{"n":1.50}"#,
            br#"{"n":1e21}"#,
            br#"{"n":0.1}"#,
            br#"{"n":1e2}"#,
            br#"{"n":-0}"#,
            br#"{"n":12.0}"#,
            br#"{"a":"<&>"}"#,
            br#"{"a":"\u00e9"}"#,
            br#"{"b":1,"a":2}"#,
            br#"{"a":[true,null,{"b":[]}]}"#,
        ] {
            assert!(field(raw, Field::Extensions).is_none(), "{raw:?}");
        }
        let neg = Some(
            json::Value::decode(br#"{"n":-0}"#)
                .unwrap()
                .into_object()
                .unwrap(),
        );
        let zero = Some(
            json::Value::decode(br#"{"n":0}"#)
                .unwrap()
                .into_object()
                .unwrap(),
        );
        assert!(extensions_changed(&neg, &zero).unwrap());
        assert!(!extensions_changed(&None, &Some(BTreeMap::new())).unwrap());
        assert!(extensions_problem(br#"{"n":0e999999999999999999999999999}"#).is_none());
    }
    #[test]
    fn deeply_nested_values_are_scanned_without_recursion() {
        std::thread::Builder::new()
            .stack_size(2 * 1024 * 1024)
            .spawn(|| {
                let mut arrays = String::from("{\"n\":");
                arrays.push_str(&"[".repeat(10000));
                arrays.push('1');
                arrays.push_str(&"]".repeat(10000));
                arrays.push('}');
                assert!(extensions_problem(arrays.as_bytes()).is_none());
                let mut objects = "{\"o\":".repeat(10000);
                objects.push_str("{\"a\":1,\"a\":2}");
                objects.push_str(&"}".repeat(10000));
                assert!(extensions_problem(objects.as_bytes())
                    .unwrap()
                    .contains("key \"a\""));
            })
            .unwrap()
            .join()
            .unwrap();
    }
}
