//! Iterative raw JSON loss checks at precisely the levels Go re-encodes.
use crate::json::{self, Value as V};
use std::collections::HashSet;
pub(super) fn array(raw: &[u8]) -> Option<Vec<&[u8]>> {
    if raw.first() != Some(&b'[') || !json::is_valid(raw) {
        return None;
    }
    let mut result = Vec::new();
    let (mut start, mut i, mut depth) = (1, 1, 0usize);
    while i < raw.len() {
        match raw[i] {
            b'"' => {
                i = string_end(raw, i);
                continue;
            }
            b'[' | b'{' => depth += 1,
            b']' if depth == 0 => {
                let part = crate::store::trim_space(&raw[start..i]);
                if !part.is_empty() {
                    result.push(part);
                }
                return Some(result);
            }
            b']' | b'}' => depth -= 1,
            b',' if depth == 0 => {
                result.push(crate::store::trim_space(&raw[start..i]));
                start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    None
}
pub(super) fn string_end(raw: &[u8], start: usize) -> usize {
    let mut i = start + 1;
    while i < raw.len() {
        match raw[i] {
            b'\\' => i += 2,
            b'"' => return i + 1,
            _ => i += 1,
        }
    }
    raw.len()
}
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
fn value_end(raw: &[u8], start: usize) -> usize {
    if raw[start] == b'"' {
        return string_end(raw, start);
    }
    if matches!(raw[start], b'{' | b'[') {
        let (mut i, mut depth) = (start, 0usize);
        while i < raw.len() {
            match raw[i] {
                b'"' => {
                    i = string_end(raw, i);
                    continue;
                }
                b'{' | b'[' => depth += 1,
                b'}' | b']' => {
                    depth -= 1;
                    if depth == 0 {
                        return i + 1;
                    }
                }
                _ => {}
            }
            i += 1;
        }
        return raw.len();
    }
    let mut i = start;
    while i < raw.len() && !matches!(raw[i], b',' | b'}' | b']' | b' ' | b'\n' | b'\r' | b'\t') {
        i += 1;
    }
    i
}
fn ws(raw: &[u8], mut i: usize) -> usize {
    while i < raw.len() && matches!(raw[i], b' ' | b'\n' | b'\r' | b'\t') {
        i += 1;
    }
    i
}
fn pairs(raw: &[u8]) -> Vec<(&[u8], &[u8])> {
    let mut out = Vec::new();
    let mut i = ws(raw, 1);
    while i < raw.len() && raw[i] != b'}' {
        let end = string_end(raw, i);
        let key = &raw[i..end];
        i = ws(raw, end) + 1;
        i = ws(raw, i);
        let end = value_end(raw, i);
        out.push((key, &raw[i..end]));
        i = ws(raw, end);
        if raw.get(i) == Some(&b',') {
            i = ws(raw, i + 1);
        } else {
            break;
        }
    }
    out
}
fn decimal(text: &str) -> Option<(bool, String, i64)> {
    let (negative, text) = text.strip_prefix('-').map_or((false, text), |s| (true, s));
    let (base, exponent) = match text.find(['e', 'E']) {
        Some(i) => (&text[..i], text[i + 1..].parse::<i64>().ok()?),
        None => (text, 0),
    };
    let fraction = base.find('.').map_or(0, |i| base.len() - i - 1);
    let mut digits = base.replace('.', "").trim_start_matches('0').to_owned();
    if digits.is_empty() {
        return Some((false, "0".into(), 0));
    }
    let mut exponent = exponent.checked_sub(fraction as i64)?;
    while digits.ends_with('0') {
        digits.pop();
        exponent = exponent.checked_add(1)?;
    }
    Some((negative, digits, exponent))
}
pub(super) fn problem(raw: &[u8], ignored: &[Vec<String>]) -> Option<String> {
    let (mut number, mut repeated, mut invalid) = (None, None, None);
    let mut stack = vec![(raw, Vec::<String>::new())];
    while let Some((value, path)) = stack.pop() {
        if ignored.iter().any(|prefix| path.starts_with(prefix)) {
            continue;
        }
        match value.first() {
            Some(b'{') => {
                let entries = pairs(value);
                let mut keys = HashSet::new();
                let mut pending = Vec::new();
                for (key, value) in entries {
                    let decoded = json::unquote(key);
                    let mut child = path.clone();
                    child.push(decoded.clone());
                    if !keys.insert(decoded.clone()) && repeated.is_none() {
                        repeated = Some(format!("repeats the key \"{decoded}\""));
                    }
                    if invalid.is_none() {
                        if let Some(why) = string_problem(key) {
                            invalid = Some(format!(
                                "holds {why} at {}, which the Go tool would write as U+FFFD",
                                child.join(".")
                            ));
                        }
                    }
                    pending.push((value, child));
                }
                stack.extend(pending.into_iter().rev());
            }
            Some(b'[') => {
                if let Some(items) = array(value) {
                    for (i, v) in items.into_iter().enumerate().rev() {
                        let mut child = path.clone();
                        child.push(i.to_string());
                        stack.push((v, child));
                    }
                }
            }
            Some(b'"') => {
                if invalid.is_none() {
                    if let Some(why) = string_problem(value) {
                        invalid = Some(format!(
                            "holds {why} at {}, which the Go tool would write as U+FFFD",
                            path.join(".")
                        ));
                    }
                }
            }
            Some(b'-' | b'0'..=b'9') => {
                if number.is_none() {
                    if let Ok(V::Number(n)) = V::decode(value) {
                        if let Ok(encoded) = json::format_float(n) {
                            let source = std::str::from_utf8(value).unwrap();
                            if decimal(source) != decimal(&encoded) {
                                number=Some(format!("holds the number {source}, which the Go tool would write as {encoded}"));
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }
    number.or(repeated).or(invalid)
}
