//! Decoder.Token diagnostics on top of the shared iterative JSON value parser.
use crate::{
    actions::record::message,
    json::{self, Value},
    Result,
};
fn ws(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\r' | b'\n')
}
fn error(input: &[u8], raw: &str) -> String {
    let (mut text, offset) = raw
        .rsplit_once(" (offset ")
        .and_then(|(s, n)| {
            n.strip_suffix(')')
                .and_then(|n| n.parse::<usize>().ok())
                .map(|n| (s.to_owned(), Some(n)))
        })
        .unwrap_or((raw.to_owned(), None));
    if text == "unexpected end of JSON input" {
        let trimmed = input
            .iter()
            .copied()
            .filter(|b| !ws(*b))
            .collect::<Vec<_>>();
        if trimmed.is_empty() || trimmed.last() == Some(&b':') {
            return "EOF".into();
        }
        let (mut string, mut escaped) = (false, false);
        for &b in input {
            if escaped {
                escaped = false;
                continue;
            }
            if string && b == b'\\' {
                escaped = true;
                continue;
            }
            if b == b'"' {
                string = !string;
            }
        }
        if string
            || trimmed.last().is_some_and(|b| {
                b.is_ascii_alphabetic() || matches!(b, b'e' | b'E' | b'+' | b'-' | b'.')
            })
        {
            return "unexpected EOF".into();
        }
        return text;
    }
    let Some(at) = offset else { return text };
    let before = &input[..at.min(input.len())];
    let previous = before.iter().rfind(|b| !ws(**b)).copied();
    if text.ends_with("after top-level value") {
        let tail = &input[at..];
        return match json::parse_bytes(tail) {
            Ok(_) => "unexpected non-whitespace character after JSON".into(),
            Err(e) => {
                let e = e.to_string();
                if e.contains("after top-level value") {
                    "unexpected non-whitespace character after JSON".into()
                } else {
                    error(tail, &e)
                }
            }
        };
    }
    if text.ends_with("looking for beginning of value")
        && previous == Some(b':')
        && input.get(at) == Some(&b'}')
    {
        return "missing value after object key".into();
    }
    if previous == Some(b',') && matches!(input.get(at), Some(b']' | b'}')) {
        return "invalid character ',' looking for beginning of value".into();
    }
    if text.ends_with("in literal") {
        let mut start = at;
        while start > 0 && input[start - 1].is_ascii_alphabetic() {
            start -= 1
        }
        let word = match input.get(start) {
            Some(b't') => "true",
            Some(b'f') => "false",
            Some(b'n') => "null",
            _ => return text,
        };
        if let Some(expected) = word.as_bytes().get(at - start) {
            text += &format!(" {word} (expecting '{}')", char::from(*expected));
        }
    }
    if text.ends_with("in string escape code") {
        if let Some(b) = input.get(at) {
            let tick = char::from(96);
            return format!(
                "invalid escape sequence {tick}\\{}{tick} in string",
                char::from(*b)
            );
        }
    }
    text.replace("in exponent of numeric literal", "in numeric literal")
        .replace(
            "after decimal point in numeric literal",
            "in numeric literal",
        )
}
pub(super) fn parse(input: &[u8]) -> Result<Value> {
    json::parse_bytes(input).map_err(|e| message(error(input, &e.to_string())))
}
