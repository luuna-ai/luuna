//! The error text `json.Unmarshal` gives for a document it rejects, as the
//! reference build produces it. Go 1.27 implements `encoding/json` on top of
//! the `encoding/json/v2` decoder, run with the v1 options, so the messages
//! are those of `internal/jsonwire` and `jsontext`, rewritten by
//! `transformSyntacticError` and `transformUnmarshalError`
//! (`encoding/json/v2_scanner.go`, `v2_inject.go`). `update --extensions`
//! prints this text as part of its own message.
//!
//! [`unmarshal_error`] finds the first syntax error, or else the first number
//! that `float64` cannot hold, the one error Go reports when it decodes into
//! `any`.

use crate::json::{decode_char, unquote};

/// The deepest nesting accepted (`maxNestingDepth`).
const MAX_DEPTH: usize = 10_000;

/// How a step failed: the input ended inside a value
/// (`io.ErrUnexpectedEOF`), or with this error text.
enum Fail {
    Eof,
    Text(String),
}

type Step<T> = Result<T, Fail>;

/// Go's `unicode.IsPrint`, approximated beyond Latin-1 by excluding control,
/// space, format, private-use and non-character code points.
fn is_print(ch: char) -> bool {
    let code = u32::from(ch);
    if code <= 0xff {
        return (0x20..0x7f).contains(&code) || (0xa1..=0xff).contains(&code) && code != 0xad;
    }
    !(ch.is_control()
        || ch.is_whitespace()
        || matches!(code, 0x061c | 0x180e | 0x200b..=0x200f | 0x202a..=0x202e | 0x2060..=0x206f
            | 0xe000..=0xf8ff | 0xfeff | 0xfff9..=0xfffb | 0xfffe | 0xffff | 0xf0000..))
}

/// The escape `strconv` writes for a non-printable (or quote-special)
/// character, or the character itself.
fn escape_char(ch: char, quote: char, out: &mut String) {
    if ch == quote || ch == '\\' {
        out.push('\\');
        out.push(ch);
        return;
    }
    if is_print(ch) {
        out.push(ch);
        return;
    }
    let code = u32::from(ch);
    match ch {
        '\u{7}' => out.push_str("\\a"),
        '\u{8}' => out.push_str("\\b"),
        '\u{c}' => out.push_str("\\f"),
        '\n' => out.push_str("\\n"),
        '\r' => out.push_str("\\r"),
        '\t' => out.push_str("\\t"),
        '\u{b}' => out.push_str("\\v"),
        _ if code < 0x20 || code == 0x7f => out.push_str(&format!("\\x{code:02x}")),
        _ if code < 0x10000 => out.push_str(&format!("\\u{code:04x}")),
        _ => out.push_str(&format!("\\U{code:08x}")),
    }
}

/// `strconv.QuoteRune`.
fn quote_rune(ch: char) -> String {
    let mut out = String::from("'");
    escape_char(ch, '\'', &mut out);
    out.push('\'');
    out
}

/// `strconv.Quote` for bytes, an invalid byte written as `\x..`.
fn quote_bytes(bytes: &[u8]) -> String {
    let mut out = String::from("\"");
    let mut pos = 0;
    while pos < bytes.len() {
        let (ch, width) = decode_char(&bytes[pos..]);
        match ch {
            Some(ch) => escape_char(ch, '"', &mut out),
            None => out.push_str(&format!("\\x{:02x}", bytes[pos])),
        }
        pos += width;
    }
    out.push('"');
    out
}

/// `jsonwire.InvalidTextError.Error`.
fn invalid_text(label: &str, what: &[u8], place: &str) -> Fail {
    let mut chars = Vec::new();
    let mut pos = 0;
    while pos < what.len() {
        let (ch, width) = decode_char(&what[pos..]);
        chars.push(ch);
        pos += width;
    }
    let need_escape = chars.iter().any(|ch| match ch {
        None => true,
        Some(ch) => matches!(ch, '`' | '\u{fffd}') || ch.is_whitespace() || !is_print(*ch),
    });
    let shown = if chars.len() == 1 {
        match chars[0] {
            // `jsonwire.QuoteRune`: no padding for an invalid byte.
            None => format!("'\\x{:x}'", what[0]),
            Some(ch) => quote_rune(ch),
        }
    } else if need_escape {
        quote_bytes(what)
    } else {
        format!("`{}`", String::from_utf8_lossy(what))
    };
    Fail::Text(format!("invalid {label} {shown} {place}"))
}

/// `jsonwire.NewInvalidCharacterError`: the character at the start of `rest`.
fn invalid_char(rest: &[u8], place: &str) -> Fail {
    let (_, width) = decode_char(rest);
    invalid_text("character", &rest[..width], place)
}

/// `jsonwire.NewInvalidEscapeSequenceError`.
fn invalid_escape(what: &[u8]) -> Fail {
    let label = if what.len() > 6 {
        "surrogate pair"
    } else {
        "escape sequence"
    };
    invalid_text(label, what, "in string")
}

fn skip_ws(src: &[u8], mut pos: usize) -> usize {
    while matches!(src.get(pos), Some(b' ' | b'\t' | b'\r' | b'\n')) {
        pos += 1;
    }
    pos
}

/// `jsonwire.ConsumeLiteral`.
fn consume_literal(src: &[u8], pos: usize, literal: &str) -> Step<usize> {
    let rest = &src[pos..];
    for (index, expected) in literal.bytes().enumerate() {
        match rest.get(index) {
            Some(&byte) if byte == expected => {}
            Some(_) => {
                return Err(invalid_char(
                    &rest[index..],
                    &format!(
                        "in literal {literal} (expecting {})",
                        quote_rune(char::from(expected))
                    ),
                ))
            }
            None => return Err(Fail::Eof),
        }
    }
    Ok(pos + literal.len())
}

/// `jsonwire.hasEscapedUTF16Prefix` for the first half of a pair.
fn has_escaped_prefix(bytes: &[u8]) -> bool {
    bytes.iter().enumerate().all(|(index, &byte)| match index {
        0 => byte == b'\\',
        1 => byte == b'u',
        2..=5 => byte.is_ascii_hexdigit(),
        _ => true,
    })
}

/// `jsonwire.ConsumeStringResumable` for the string at `pos`, with invalid
/// UTF-8 allowed (the v1 options).
fn consume_string(src: &[u8], pos: usize) -> Step<usize> {
    let rest = &src[pos..];
    if rest.first() != Some(&b'"') {
        return Err(invalid_char(rest, "at start of string (expecting '\"')"));
    }
    let mut n = 1;
    while n < rest.len() {
        let byte = rest[n];
        match byte {
            b'"' => return Ok(pos + n + 1),
            b'\\' => {
                if rest.len() < n + 2 {
                    return Err(Fail::Eof);
                }
                match rest[n + 1] {
                    b'/' | b'"' | b'\\' | b'b' | b'f' | b'n' | b'r' | b't' => n += 2,
                    b'u' => {
                        if rest.len() < n + 6 {
                            if has_escaped_prefix(&rest[n..]) {
                                return Err(Fail::Eof);
                            }
                            return Err(invalid_escape(&rest[n..]));
                        }
                        if !rest[n + 2..n + 6].iter().all(u8::is_ascii_hexdigit) {
                            return Err(invalid_escape(&rest[n..n + 6]));
                        }
                        n += 6;
                    }
                    _ => return Err(invalid_escape(&rest[n..n + 2])),
                }
            }
            0..=0x1f => {
                return Err(invalid_char(
                    &rest[n..],
                    "in string (expecting non-control character)",
                ))
            }
            _ => n += 1,
        }
    }
    Err(Fail::Eof)
}

/// `jsonwire.ConsumeNumber`, where the end of the input may end a number.
fn consume_number(src: &[u8], pos: usize) -> Step<usize> {
    let rest = &src[pos..];
    let digits = |mut n: usize| {
        while rest.get(n).is_some_and(u8::is_ascii_digit) {
            n += 1;
        }
        n
    };
    let expect_digit = |n: usize| -> Step<usize> {
        match rest.get(n) {
            None => Err(Fail::Eof),
            Some(byte) if byte.is_ascii_digit() => Ok(digits(n)),
            Some(_) => Err(invalid_char(&rest[n..], "in number (expecting digit)")),
        }
    };
    let mut n = usize::from(rest[0] == b'-');
    n = match rest.get(n) {
        None => return Err(Fail::Eof),
        Some(b'0') => n + 1,
        Some(b'1'..=b'9') => digits(n),
        Some(_) => return Err(invalid_char(&rest[n..], "in number (expecting digit)")),
    };
    if rest.get(n) == Some(&b'.') {
        n = expect_digit(n + 1)?;
    }
    if matches!(rest.get(n), Some(b'e' | b'E')) {
        n += 1;
        if matches!(rest.get(n), Some(b'+' | b'-')) {
            n += 1;
        }
        n = expect_digit(n)?;
    }
    Ok(pos + n)
}

/// One open object or array, with the path token of the member being read.
struct Frame {
    object: bool,
    key: String,
    index: usize,
}

/// Go's path for the number at the current position
/// (`transformUnmarshalError`): JSON pointer tokens joined by dots.
fn field_path(stack: &[Frame]) -> String {
    let tokens: Vec<String> = stack
        .iter()
        .map(|frame| {
            if frame.object {
                frame.key.replace('~', "~0").replace('/', "~1")
            } else {
                frame.index.to_string()
            }
        })
        .collect();
    tokens.join(".")
}

/// `UnmarshalTypeError.Error` for a number that overflows `float64`.
fn overflow_message(number: &str, field: &str) -> String {
    if field.is_empty() {
        return format!("json: cannot unmarshal number {number} into Go value of type float64");
    }
    let last = field.rsplit('.').next().unwrap_or(field);
    let into = if !last.is_empty() && last.bytes().all(|byte| byte.is_ascii_digit()) {
        ""
    } else {
        "Go struct field "
    };
    format!("json: cannot unmarshal number {number} into {into}.{field} of type float64")
}

/// `transformSyntacticError`: drops the expectation text (except for
/// literals) and renames the v2 terms to v1's.
fn transform(message: &str) -> String {
    let mut message = message.to_owned();
    if let Some(index) = message.find(" (expecting") {
        if !message.contains(" in literal") {
            message.truncate(index);
        }
    }
    const PAIRS: [(&str, &str); 5] = [
        ("object name", "object key"),
        ("at start of value", "looking for beginning of value"),
        (
            "at start of string",
            "looking for beginning of object key string",
        ),
        ("after object value", "after object key:value pair"),
        ("in number", "in numeric literal"),
    ];
    let mut out = String::with_capacity(message.len());
    let mut rest = message.as_str();
    'scan: while !rest.is_empty() {
        for (old, new) in PAIRS {
            if let Some(tail) = rest.strip_prefix(old) {
                out.push_str(new);
                rest = tail;
                continue 'scan;
            }
        }
        let ch = rest.chars().next().expect("not empty");
        out.push(ch);
        rest = &rest[ch.len_utf8()..];
    }
    out
}

/// The text of the error `json.Unmarshal(src, &any)` returns, or `None` when
/// it succeeds.
pub(crate) fn unmarshal_error(src: &str) -> Option<String> {
    match scan(src.as_bytes()) {
        Ok(None) => None,
        Ok(Some(message)) => Some(message),
        Err(Fail::Eof) => Some("unexpected end of JSON input".to_owned()),
        Err(Fail::Text(message)) => Some(transform(&message)),
    }
}

/// Reads the document: the first overflowing number, if any, on success.
fn scan(src: &[u8]) -> Step<Option<String>> {
    let mut stack: Vec<Frame> = Vec::new();
    let mut overflow: Option<String> = None;
    let mut pos = skip_ws(src, 0);
    if pos >= src.len() {
        return Err(Fail::Eof);
    }
    // Reads an object member name and its colon at `pos`; returns the
    // position of the value.
    let member = |src: &[u8], pos: usize, frame: &mut Frame| -> Step<usize> {
        let start = skip_ws(src, pos);
        if start >= src.len() {
            return Err(Fail::Eof);
        }
        let end = consume_string(src, start)?;
        frame.key = unquote(&src[start..end]);
        let colon = skip_ws(src, end);
        match src.get(colon) {
            None => return Err(Fail::Eof),
            Some(b':') => {}
            Some(_) => {
                return Err(invalid_char(
                    &src[colon..],
                    "after object name (expecting ':')",
                ))
            }
        }
        let value = skip_ws(src, colon + 1);
        if value >= src.len() {
            return Err(Fail::Eof);
        }
        Ok(value)
    };
    loop {
        // A value starts at `pos`.
        match src[pos] {
            open @ (b'{' | b'[') => {
                if stack.len() + 1 == MAX_DEPTH + 1 {
                    return Err(Fail::Text("exceeded max depth".to_owned()));
                }
                let object = open == b'{';
                let after = skip_ws(src, pos + 1);
                if after >= src.len() {
                    return Err(Fail::Eof);
                }
                if src[after] == if object { b'}' } else { b']' } {
                    pos = after + 1;
                } else {
                    let mut frame = Frame {
                        object,
                        key: String::new(),
                        index: 0,
                    };
                    pos = if object {
                        member(src, after, &mut frame)?
                    } else {
                        after
                    };
                    stack.push(frame);
                    continue;
                }
            }
            b'"' => pos = consume_string(src, pos)?,
            b'n' => pos = consume_literal(src, pos, "null")?,
            b'f' => pos = consume_literal(src, pos, "false")?,
            b't' => pos = consume_literal(src, pos, "true")?,
            b'-' | b'0'..=b'9' => {
                let end = consume_number(src, pos)?;
                if overflow.is_none() {
                    let text = std::str::from_utf8(&src[pos..end]).unwrap_or("0");
                    if text.parse::<f64>().is_ok_and(f64::is_infinite) {
                        overflow = Some(overflow_message(text, &field_path(&stack)));
                    }
                }
                pos = end;
            }
            _ => return Err(invalid_char(&src[pos..], "at start of value")),
        }
        // The value ended at `pos`: close containers, or go on to the next.
        loop {
            let after = skip_ws(src, pos);
            let Some(frame) = stack.last_mut() else {
                return if after < src.len() {
                    Err(invalid_char(&src[after..], "after top-level value"))
                } else {
                    Ok(overflow)
                };
            };
            let Some(&byte) = src.get(after) else {
                return Err(Fail::Eof);
            };
            let (close, place) = if frame.object {
                (b'}', "after object value (expecting ',' or '}')")
            } else {
                (b']', "after array element (expecting ',' or ']')")
            };
            if byte == b',' {
                if frame.object {
                    pos = member(src, after + 1, frame)?;
                } else {
                    frame.index += 1;
                    pos = skip_ws(src, after + 1);
                    if pos >= src.len() {
                        return Err(Fail::Eof);
                    }
                }
                break;
            } else if byte == close {
                pos = after + 1;
                stack.pop();
            } else {
                return Err(invalid_char(&src[after..], place));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn error(text: &str) -> Option<String> {
        unmarshal_error(text)
    }

    #[test]
    fn valid_documents_have_no_error() {
        for text in [
            "{}",
            " [1, 2.5e-3, \"a\\u00e9\", true, false, null, {\"a\": []}] ",
            "0",
            "-0.5",
            "\"x\"",
            "1E+9",
            "{\"a\":1,\"a\":2}",
            "\"\\ud800\"",
            "1e-999",
        ] {
            assert_eq!(error(text), None, "{text}");
        }
    }

    #[test]
    fn messages_follow_go() {
        for (text, message) in [
            ("", "unexpected end of JSON input"),
            ("   ", "unexpected end of JSON input"),
            ("{", "unexpected end of JSON input"),
            ("{\"a\":", "unexpected end of JSON input"),
            ("[1,", "unexpected end of JSON input"),
            ("\"abc", "unexpected end of JSON input"),
            ("tru", "unexpected end of JSON input"),
            ("1.", "unexpected end of JSON input"),
            ("-", "unexpected end of JSON input"),
            ("\"\\u12", "unexpected end of JSON input"),
            ("x", "invalid character 'x' looking for beginning of value"),
            (",1", "invalid character ',' looking for beginning of value"),
            (
                "{a:1}",
                "invalid character 'a' looking for beginning of object key string",
            ),
            ("{\"a\" 1}", "invalid character '1' after object key"),
            (
                "{\"a\":1 \"b\"}",
                "invalid character '\"' after object key:value pair",
            ),
            ("[1 2]", "invalid character '2' after array element"),
            ("{} x", "invalid character 'x' after top-level value"),
            (
                "{\"a\":1,}",
                "invalid character '}' looking for beginning of object key string",
            ),
            (
                "[1,]",
                "invalid character ']' looking for beginning of value",
            ),
            (
                "tx",
                "invalid character 'x' in literal true (expecting 'r')",
            ),
            (
                "nul1",
                "invalid character '1' in literal null (expecting 'l')",
            ),
            ("-x", "invalid character 'x' in numeric literal"),
            ("1.x", "invalid character 'x' in numeric literal"),
            ("1ex", "invalid character 'x' in numeric literal"),
            ("1e+x", "invalid character 'x' in numeric literal"),
            ("\"\\q\"", "invalid escape sequence `\\q` in string"),
            ("\"\\u12x4\"", "invalid escape sequence `\\u12x4` in string"),
            ("\"a\nb\"", "invalid character '\\n' in string"),
            (
                "'a'",
                "invalid character '\\'' looking for beginning of value",
            ),
            (
                "\u{e9}",
                "invalid character '\u{e9}' looking for beginning of value",
            ),
            ("01", "invalid character '1' after top-level value"),
        ] {
            assert_eq!(error(text).as_deref(), Some(message), "{text:?}");
        }
    }

    #[test]
    fn nesting_is_limited() {
        let deep = "[".repeat(MAX_DEPTH) + &"]".repeat(MAX_DEPTH);
        assert_eq!(error(&deep), None);
        let too_deep = "[".repeat(MAX_DEPTH + 1);
        assert_eq!(error(&too_deep).as_deref(), Some("exceeded max depth"));
    }

    #[test]
    fn numbers_out_of_range_are_reported_with_their_path() {
        for (text, message) in [
            (
                "1e999",
                "json: cannot unmarshal number 1e999 into Go value of type float64",
            ),
            (
                "[1e999]",
                "json: cannot unmarshal number 1e999 into .0 of type float64",
            ),
            (
                "{\"a\":{\"b\":-1e999}}",
                "json: cannot unmarshal number -1e999 into Go struct field .a.b of type float64",
            ),
            (
                "{\"a\":[{\"b\":1e999}]}",
                "json: cannot unmarshal number 1e999 into Go struct field .a.0.b of type float64",
            ),
            (
                "{\"a\":[1e999]}",
                "json: cannot unmarshal number 1e999 into .a.0 of type float64",
            ),
            (
                "{\"a\":1e999,\"b\":2e999}",
                "json: cannot unmarshal number 1e999 into Go struct field .a of type float64",
            ),
        ] {
            assert_eq!(error(text).as_deref(), Some(message), "{text}");
        }
        // A syntax error wins over an overflow.
        assert_eq!(
            error("[1e999,").as_deref(),
            Some("unexpected end of JSON input")
        );
    }
}
