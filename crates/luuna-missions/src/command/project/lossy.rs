//! Targeted A8 checks at the point a Go rewrite would drop raw data.
use super::template_record::{array, string_end};
use crate::{
    command::Failure,
    error::Error,
    json,
    store::{trim_space, Record},
};
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::Path,
};
fn read(path: &Path) -> Result<Vec<u8>, Failure> {
    match fs::read(path) {
        Ok(v) => Ok(v),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(Error::io("read", path, e).into()),
    }
}
/// Top-level key tokens. Nested raw JSON is deliberately not decoded.
pub(super) fn keys(raw: &[u8]) -> Vec<&[u8]> {
    let (mut i, mut depth, mut key) = (0, 0usize, false);
    let mut out = Vec::new();
    while i < raw.len() {
        match raw[i] {
            b'"' => {
                let end = string_end(raw, i);
                if depth == 1 && key {
                    out.push(&raw[i..end]);
                    key = false;
                }
                i = end;
                continue;
            }
            b'{' | b'[' => {
                depth += 1;
                if depth == 1 {
                    key = true;
                }
            }
            b'}' | b']' => depth = depth.saturating_sub(1),
            b',' if depth == 1 => key = true,
            _ => {}
        }
        i += 1;
    }
    out
}
fn repeated(raw: &[u8]) -> Option<String> {
    let mut seen = HashSet::new();
    for raw in keys(raw) {
        let key = json::unquote(raw);
        if !seen.insert(key.clone()) {
            return Some(key);
        }
    }
    None
}
fn refuse(path: &Path, line: usize, why: &str) -> Failure {
    Failure::message(format!("Refusing to rewrite {}: line {line} {why}, and the Go tool would drop it without a message. Nothing was written.",path.display()))
}
pub(super) fn refuse_lossy_rewrite<R: Record>(path: &Path) -> Result<(), Failure> {
    let bytes = read(path)?;
    let mut ids = HashMap::new();
    for (i, line) in bytes.split(|b| *b == b'\n').enumerate() {
        let n = i + 1;
        let line = trim_space(line);
        if line.is_empty() {
            continue;
        }
        let record = R::from_line(line)
            .ok_or_else(|| refuse(path, n, "is not a record the tool can read"))?;
        let id = record.record_id();
        if let Some(first) = ids.insert(id.to_owned(), n) {
            return Err(refuse(
                path,
                n,
                &format!("repeats the id \"{id}\" of line {first}"),
            ));
        }
        if let Some(key) = repeated(line) {
            return Err(refuse(path, n, &format!("repeats the key \"{key}\"")));
        }
    }
    Ok(())
}
/// Invalid encodings that Go normalises when it re-encodes a string token.
pub(super) fn string_problem(token: &[u8]) -> Option<String> {
    let inner = &token[1..token.len() - 1];
    let mut i = 0;
    while i < inner.len() {
        if inner[i] == b'\\' {
            if inner[i + 1] != b'u' {
                i += 2;
                continue;
            }
            let first = u16::from_str_radix(std::str::from_utf8(&inner[i + 2..i + 6]).unwrap(), 16)
                .unwrap();
            if (0xd800..=0xdbff).contains(&first) {
                let second = inner
                    .get(i + 6..i + 12)
                    .filter(|s| s.starts_with(b"\\u"))
                    .and_then(|s| std::str::from_utf8(&s[2..]).ok())
                    .and_then(|s| u16::from_str_radix(s, 16).ok());
                if !second.is_some_and(|s| (0xdc00..=0xdfff).contains(&s)) {
                    return Some(format!(
                        "the unpaired escape {}",
                        String::from_utf8_lossy(&inner[i..i + 6])
                    ));
                }
                i += 12;
            } else if (0xdc00..=0xdfff).contains(&first) {
                return Some(format!(
                    "the unpaired escape {}",
                    String::from_utf8_lossy(&inner[i..i + 6])
                ));
            } else {
                i += 6;
            }
        } else {
            let (c, n) = json::decode_char(&inner[i..]);
            if c.is_none() {
                return Some("invalid UTF-8".into());
            }
            i += n;
        }
    }
    None
}
pub(super) fn refuse_replaced_steps(path: &Path, id: &str) -> Result<(), Failure> {
    let bytes = read(path)?;
    for (i, line) in bytes.split(|b| *b == b'\n').enumerate() {
        let n = i + 1;
        let line = trim_space(line);
        if line.is_empty() {
            continue;
        }
        let Ok(m) = json::parse_object(line) else {
            continue;
        };
        if m.get("id")
            .filter(|v| v.first() == Some(&b'"'))
            .map(json::unquote)
            .as_deref()
            != Some(id)
        {
            continue;
        }
        let Some(steps) = m.get("steps").filter(|v| *v != b"null") else {
            return Ok(());
        };
        let items=array(steps).ok_or_else(||Failure::message(format!("Refusing to rewrite {}: line {n} holds template \"{id}\" whose steps is not a list, and the Go tool would replace it without a message. Nothing was written.",path.display())))?;
        for (k, item) in items.into_iter().enumerate() {
            if item.first() != Some(&b'{') {
                return Err(Failure::message(format!("Refusing to rewrite {}: line {n} holds template \"{id}\" whose steps item {} is not an object, and the Go tool would replace it without a message. Nothing was written.",path.display(),k+1)));
            }
            if let Some(key) = repeated(item) {
                return Err(refuse(path, n, &format!("repeats the key \"{key}\"")));
            }
            for token in keys(item) {
                if string_problem(token).is_some() {
                    return Err(Failure::message(format!("Refusing to rewrite {}: line {n} holds template \"{id}\" whose steps item {} has a key with invalid UTF-8 or an unpaired surrogate escape, and the Go tool would write it as U+FFFD. Nothing was written.",path.display(),k+1)));
                }
            }
        }
        return Ok(());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::template_record::Template;
    use super::*;
    #[test]
    fn detects_each_kind_of_loss_and_accepts_a_clean_file() {
        let t = tempfile::tempdir().unwrap();
        let path = t.path().join("templates.jsonl");
        for (data, why) in [
            (
                b"bad\n".as_slice(),
                "line 1 is not a record the tool can read",
            ),
            (b"{}\n{}\n", "line 2 repeats the id \"\" of line 1"),
            (
                b"{\"id\":\"a\",\"id\":\"b\"}\n",
                "line 1 repeats the key \"id\"",
            ),
        ] {
            fs::write(&path, data).unwrap();
            match refuse_lossy_rewrite::<Template>(&path).unwrap_err(){Failure::Message(text)=>assert_eq!(text,format!("Refusing to rewrite {}: {why}, and the Go tool would drop it without a message. Nothing was written.",path.display())),other=>panic!("{other:?}")};
            assert_eq!(fs::read(&path).unwrap(), data);
        }
        fs::write(
            &path,
            b"{\"id\":\"a\",\"steps\":[{\"title\":\"x\",\"extra\":{\"a\":1,\"a\":2}}]}\n",
        )
        .unwrap();
        assert!(refuse_lossy_rewrite::<Template>(&path).is_ok());
        assert!(refuse_replaced_steps(&path, "a").is_ok());
        assert!(string_problem(br#""\ud83d\ude00""#).is_none());
        assert_eq!(
            string_problem(br#""\ud800""#),
            Some("the unpaired escape \\ud800".into())
        );
    }
}
