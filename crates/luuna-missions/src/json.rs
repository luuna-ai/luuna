//! JSON reading and writing as Go's `encoding/json` does it.
//!
//! The reference Go tool is built with go1.27, whose `encoding/json` runs on
//! the v2 implementation in v1 mode (`GOEXPERIMENT=jsonv2` is on by default).
//! The rules copied here are that implementation's:
//!
//! - `json.Marshal` quotes strings with `<`, `>`, `&`, U+2028 and U+2029
//!   escaped as `\u003c`, `\u003e`, `\u0026`, `\u2028` and `\u2029`, `\b`,
//!   `\f`, `\n`, `\r`, `\t` short escapes, other control characters as
//!   `\u00XX`, and invalid UTF-8 replaced by U+FFFD.
//! - Raw JSON (a `RawMessage` or a `MarshalJSON` result) is reformatted with
//!   its strings preserved: escapes already present stay as they are, and
//!   only literal `<`, `>`, `&` (when HTML escaping is on) and literal
//!   U+2028 and U+2029 (always) are escaped. Numbers are copied verbatim, and
//!   invalid UTF-8 inside a raw string is copied as it is.
//! - Command output goes through a `json.Encoder` with HTML escaping off and
//!   two-space indentation, which still escapes U+2028 and U+2029.
//! - Floats are formatted like ECMAScript (`1e+21`, `1e-7`, `-0`).
//! - Decoding into `any` gives `float64` numbers (±Inf when out of range) and
//!   maps whose duplicate keys keep the last value; strings with invalid
//!   UTF-8 or lone surrogate escapes decode with U+FFFD.
//!
//! The scanner works on bytes, not on `str`, because Go accepts and keeps
//! record lines whose strings hold invalid UTF-8; serde_json's `RawValue`
//! needs valid UTF-8 and would drop such a line where Go keeps it.

use std::collections::BTreeMap;
use std::fmt;

/// The deepest nesting Go accepts (`maxNestingDepth` in jsontext).
const MAX_DEPTH: usize = 10_000;

/// How raw JSON is rewritten.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Format {
    /// Escape literal `<`, `>` and `&` inside strings.
    pub escape_html: bool,
    /// Escape literal U+2028 and U+2029 inside strings.
    pub escape_js: bool,
    /// Indent with this unit after each newline; `None` is compact.
    pub indent: Option<&'static str>,
}

impl Format {
    /// `json.Compact`: whitespace removed, strings untouched.
    pub const COMPACT: Self = Self {
        escape_html: false,
        escape_js: false,
        indent: None,
    };
    /// `json.Marshal`: compact, HTML and JS characters escaped.
    pub const MARSHAL: Self = Self {
        escape_html: true,
        escape_js: true,
        indent: None,
    };
    /// The Go tool's `Context.JSON` encoder: `SetEscapeHTML(false)` and
    /// `SetIndent("", "  ")`.
    #[allow(dead_code, reason = "used by the command output ported in t3 to t5")]
    pub const OUTPUT: Self = Self {
        escape_html: false,
        escape_js: true,
        indent: Some("  "),
    };
}

/// Input that is not one valid JSON value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SyntaxError {
    /// The byte offset where the input stopped making sense.
    pub offset: usize,
}

impl fmt::Display for SyntaxError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "invalid JSON at byte {}", self.offset)
    }
}

/// A value Go cannot encode: a number that is not finite
/// (`json: unsupported value: +Inf`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct UnsupportedValue(pub f64);

impl fmt::Display for UnsupportedValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = if self.0.is_nan() {
            "NaN"
        } else if self.0 > 0.0 {
            "+Inf"
        } else {
            "-Inf"
        };
        write!(formatter, "json: unsupported value: {text}")
    }
}

/// A value decoded the way Go decodes JSON into `any`.
#[derive(Debug, Clone)]
pub enum Value {
    /// `null` (Go `nil`).
    Null,
    /// `true` or `false`.
    Bool(bool),
    /// A number, always a `float64` in Go.
    Number(f64),
    /// A string.
    String(String),
    /// An array (`[]any`).
    Array(Vec<Value>),
    /// An object (`map[string]any`); Go writes its keys sorted by bytes.
    Object(BTreeMap<String, Value>),
}

impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Null, Self::Null) => true,
            (Self::Bool(a), Self::Bool(b)) => a == b,
            // Bit equality keeps `Eq` lawful; JSON input never yields NaN.
            (Self::Number(a), Self::Number(b)) => a.to_bits() == b.to_bits(),
            (Self::String(a), Self::String(b)) => a == b,
            (Self::Array(a), Self::Array(b)) => a == b,
            (Self::Object(a), Self::Object(b)) => a == b,
            _ => false,
        }
    }
}

impl Eq for Value {}

impl Value {
    /// Decodes `src`, one valid JSON value, as Go's `json.Unmarshal` into
    /// `any` does.
    pub(crate) fn decode(src: &[u8]) -> Result<Self, SyntaxError> {
        enum Frame {
            Array(Vec<Value>),
            Object(BTreeMap<String, Value>, String),
        }
        let mut stack: Vec<Frame> = Vec::new();
        let mut pos = 0;
        loop {
            // Parse one value at `pos`, or open a container.
            pos = skip_ws(src, pos);
            let mut value = match src.get(pos) {
                Some(b'{') | Some(b'[') => {
                    let object = src[pos] == b'{';
                    if stack.len() >= MAX_DEPTH {
                        return Err(SyntaxError { offset: pos });
                    }
                    pos = skip_ws(src, pos + 1);
                    let close = if object { b'}' } else { b']' };
                    if src.get(pos) == Some(&close) {
                        pos += 1;
                        if object {
                            Value::Object(BTreeMap::new())
                        } else {
                            Value::Array(Vec::new())
                        }
                    } else if object {
                        let (key, next) = object_key(src, pos)?;
                        stack.push(Frame::Object(BTreeMap::new(), key));
                        pos = next;
                        continue;
                    } else {
                        stack.push(Frame::Array(Vec::new()));
                        continue;
                    }
                }
                Some(b'"') => {
                    let end = string_end(src, pos)?;
                    let text = unquote(&src[pos..end]);
                    pos = end;
                    Value::String(text)
                }
                Some(b't') => {
                    pos = literal(src, pos, b"true")?;
                    Value::Bool(true)
                }
                Some(b'f') => {
                    pos = literal(src, pos, b"false")?;
                    Value::Bool(false)
                }
                Some(b'n') => {
                    pos = literal(src, pos, b"null")?;
                    Value::Null
                }
                Some(b'-' | b'0'..=b'9') => {
                    let end = number_end(src, pos)?;
                    let number = parse_float(&src[pos..end]);
                    pos = end;
                    Value::Number(number)
                }
                _ => return Err(SyntaxError { offset: pos }),
            };
            // Attach the value to its parent and close finished containers.
            loop {
                pos = skip_ws(src, pos);
                match stack.last_mut() {
                    None => {
                        return if pos == src.len() {
                            Ok(value)
                        } else {
                            Err(SyntaxError { offset: pos })
                        };
                    }
                    Some(Frame::Array(items)) => {
                        items.push(value);
                        match src.get(pos) {
                            Some(b',') => {
                                pos += 1;
                                break;
                            }
                            Some(b']') => {
                                pos += 1;
                                let Some(Frame::Array(items)) = stack.pop() else {
                                    unreachable!("the top frame is an array");
                                };
                                value = Value::Array(items);
                            }
                            _ => return Err(SyntaxError { offset: pos }),
                        }
                    }
                    Some(Frame::Object(map, key)) => {
                        map.insert(std::mem::take(key), value);
                        match src.get(pos) {
                            Some(b',') => {
                                let (next_key, next) = object_key(src, skip_ws(src, pos + 1))?;
                                *key = next_key;
                                pos = next;
                                break;
                            }
                            Some(b'}') => {
                                pos += 1;
                                let Some(Frame::Object(map, _)) = stack.pop() else {
                                    unreachable!("the top frame is an object");
                                };
                                value = Value::Object(map);
                            }
                            _ => return Err(SyntaxError { offset: pos }),
                        }
                    }
                }
            }
        }
    }

    /// Appends the value as `json.Marshal` writes it, with `escape_html`
    /// choosing whether `<`, `>` and `&` are escaped.
    pub(crate) fn encode(
        &self,
        out: &mut Vec<u8>,
        escape_html: bool,
    ) -> Result<(), UnsupportedValue> {
        match self {
            Self::Null => out.extend_from_slice(b"null"),
            Self::Bool(true) => out.extend_from_slice(b"true"),
            Self::Bool(false) => out.extend_from_slice(b"false"),
            Self::Number(number) => out.extend_from_slice(format_float(*number)?.as_bytes()),
            Self::String(text) => quote(out, text, escape_html),
            Self::Array(items) => {
                out.push(b'[');
                for (index, item) in items.iter().enumerate() {
                    if index > 0 {
                        out.push(b',');
                    }
                    item.encode(out, escape_html)?;
                }
                out.push(b']');
            }
            Self::Object(map) => encode_map(out, map, escape_html)?,
        }
        Ok(())
    }
}

/// Appends `map` as Go writes a `map[string]any`: keys sorted by bytes.
pub(crate) fn encode_map(
    out: &mut Vec<u8>,
    map: &BTreeMap<String, Value>,
    escape_html: bool,
) -> Result<(), UnsupportedValue> {
    out.push(b'{');
    for (index, (key, value)) in map.iter().enumerate() {
        if index > 0 {
            out.push(b',');
        }
        quote(out, key, escape_html);
        out.push(b':');
        value.encode(out, escape_html)?;
    }
    out.push(b'}');
    Ok(())
}

/// Appends `text` as a quoted JSON string, as Go's `json.Marshal`
/// (`escape_html` true) or an encoder with `SetEscapeHTML(false)` writes it.
/// U+2028 and U+2029 are always escaped.
pub(crate) fn quote(out: &mut Vec<u8>, text: &str, escape_html: bool) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    out.push(b'"');
    let bytes = text.as_bytes();
    let mut start = 0;
    for (index, ch) in text.char_indices() {
        let escape: Option<&[u8]> = match ch {
            '"' => Some(b"\\\""),
            '\\' => Some(b"\\\\"),
            '\u{8}' => Some(b"\\b"),
            '\u{c}' => Some(b"\\f"),
            '\n' => Some(b"\\n"),
            '\r' => Some(b"\\r"),
            '\t' => Some(b"\\t"),
            '\u{2028}' => Some(b"\\u2028"),
            '\u{2029}' => Some(b"\\u2029"),
            '<' if escape_html => Some(b"\\u003c"),
            '>' if escape_html => Some(b"\\u003e"),
            '&' if escape_html => Some(b"\\u0026"),
            _ => None,
        };
        if let Some(escape) = escape {
            out.extend_from_slice(&bytes[start..index]);
            out.extend_from_slice(escape);
            start = index + ch.len_utf8();
        } else if (ch as u32) < 0x20 {
            out.extend_from_slice(&bytes[start..index]);
            let byte = ch as u8;
            out.extend_from_slice(b"\\u00");
            out.push(HEX[usize::from(byte >> 4)]);
            out.push(HEX[usize::from(byte & 0xf)]);
            start = index + 1;
        }
    }
    out.extend_from_slice(&bytes[start..]);
    out.push(b'"');
}

/// `text` quoted as `json.Marshal` quotes a string.
pub(crate) fn quoted(text: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len() + 2);
    quote(&mut out, text, true);
    out
}

/// Formats a float as Go's encoder does (ECMAScript number formatting, but
/// with `-0` kept). Non-finite numbers cannot be encoded.
pub(crate) fn format_float(number: f64) -> Result<String, UnsupportedValue> {
    if !number.is_finite() {
        return Err(UnsupportedValue(number));
    }
    let magnitude = number.abs();
    if magnitude != 0.0 && !(1e-6..1e21).contains(&magnitude) {
        // Rust's `{:e}` is the shortest round-trip form, like Go's 'e' with
        // precision -1 after Go strips the exponent's leading zero.
        let text = format!("{number:e}");
        return Ok(match text.split_once('e') {
            Some((mantissa, exponent)) if !exponent.starts_with('-') => {
                format!("{mantissa}e+{exponent}")
            }
            _ => text,
        });
    }
    // Rust's `Display` is the shortest round-trip form without exponent.
    Ok(format!("{number}"))
}

/// Rewrites `src`, one JSON value with optional surrounding whitespace, in
/// `format`: insignificant whitespace removed (or replaced by indentation),
/// numbers and escapes kept, and literal characters escaped as `format`
/// asks.
pub(crate) fn reformat(src: &[u8], format: Format) -> Result<Vec<u8>, SyntaxError> {
    let mut out = Vec::with_capacity(src.len());
    let start = skip_ws(src, 0);
    let end = walk(src, start, format, Some(&mut out))?;
    let end = skip_ws(src, end);
    if end == src.len() {
        Ok(out)
    } else {
        Err(SyntaxError { offset: end })
    }
}

/// `json.Compact`.
pub(crate) fn compact(src: &[u8]) -> Result<Vec<u8>, SyntaxError> {
    reformat(src, Format::COMPACT)
}

/// What the Go tool's `Context.JSON` prints for the compact JSON `src`:
/// indented by two spaces, HTML characters literal, and a final newline.
#[allow(dead_code, reason = "used by the command output ported in t3 to t5")]
pub(crate) fn to_output(src: &[u8]) -> Result<Vec<u8>, SyntaxError> {
    let mut out = reformat(src, Format::OUTPUT)?;
    out.push(b'\n');
    Ok(out)
}

/// Whether `src` is exactly one JSON value, with optional surrounding
/// whitespace, as Go's `json.Unmarshal` requires.
pub(crate) fn is_valid(src: &[u8]) -> bool {
    let start = skip_ws(src, 0);
    match walk(src, start, Format::COMPACT, None) {
        Ok(end) => skip_ws(src, end) == src.len(),
        Err(_) => false,
    }
}

/// The members of the JSON object `src` as Go's record reader collects them
/// (`parseObject` in `internal/model/json.go`): keys decoded, raw values as
/// written (inner whitespace kept), a repeated key keeping its first
/// position and its last value.
pub(crate) fn parse_object(src: &[u8]) -> Result<Members, SyntaxError> {
    let mut members = Members::default();
    let mut pos = skip_ws(src, 0);
    if src.get(pos) != Some(&b'{') {
        return Err(SyntaxError { offset: pos });
    }
    pos = skip_ws(src, pos + 1);
    if src.get(pos) == Some(&b'}') {
        return Ok(members);
    }
    loop {
        let (key, next) = object_key(src, pos)?;
        let start = skip_ws(src, next);
        let end = walk(src, start, Format::COMPACT, None)?;
        members.insert(key, src[start..end].to_vec());
        pos = skip_ws(src, end);
        match src.get(pos) {
            Some(b',') => pos = skip_ws(src, pos + 1),
            Some(b'}') => return Ok(members),
            _ => return Err(SyntaxError { offset: pos }),
        }
    }
}

/// The members of a JSON object in first-seen key order, each with its last
/// raw value.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Members {
    order: Vec<String>,
    values: BTreeMap<String, Vec<u8>>,
}

impl Members {
    fn insert(&mut self, key: String, raw: Vec<u8>) {
        if !self.values.contains_key(&key) {
            self.order.push(key.clone());
        }
        self.values.insert(key, raw);
    }

    /// The keys in first-seen order.
    pub(crate) fn keys(&self) -> &[String] {
        &self.order
    }

    /// The last raw value of `key`.
    pub(crate) fn get(&self, key: &str) -> Option<&[u8]> {
        self.values.get(key).map(Vec::as_slice)
    }
}

/// Decodes a JSON string token (quotes included, already validated) as Go
/// does: escapes resolved, a lone surrogate escape and each invalid UTF-8
/// byte replaced by U+FFFD.
pub(crate) fn unquote(token: &[u8]) -> String {
    let inner = &token[1..token.len() - 1];
    let mut out = String::with_capacity(inner.len());
    let mut pos = 0;
    while pos < inner.len() {
        match inner[pos] {
            b'\\' => {
                let escape = inner[pos + 1];
                pos += 2;
                let ch = match escape {
                    b'"' => '"',
                    b'\\' => '\\',
                    b'/' => '/',
                    b'b' => '\u{8}',
                    b'f' => '\u{c}',
                    b'n' => '\n',
                    b'r' => '\r',
                    b't' => '\t',
                    _ => {
                        // `u`: validated to be followed by four hex digits.
                        let first = hex4(&inner[pos..pos + 4]);
                        pos += 4;
                        if (0xd800..0xdc00).contains(&first)
                            && inner.get(pos) == Some(&b'\\')
                            && inner.get(pos + 1) == Some(&b'u')
                            && inner.len() >= pos + 6
                        {
                            let second = hex4(&inner[pos + 2..pos + 6]);
                            if (0xdc00..0xe000).contains(&second) {
                                pos += 6;
                                let combined =
                                    0x10000 + ((first - 0xd800) << 10) + (second - 0xdc00);
                                out.push(char::from_u32(combined).unwrap_or('\u{fffd}'));
                                continue;
                            }
                        }
                        char::from_u32(first).unwrap_or('\u{fffd}')
                    }
                };
                out.push(ch);
            }
            _ => {
                let (ch, width) = decode_char(&inner[pos..]);
                out.push(ch.unwrap_or('\u{fffd}'));
                pos += width;
            }
        }
    }
    out
}

/// Decodes the UTF-8 character at the start of `bytes` (not empty) as Go's
/// `utf8.DecodeRune` does: `None` with width 1 for an invalid byte.
pub(crate) fn decode_char(bytes: &[u8]) -> (Option<char>, usize) {
    let width = match bytes[0] {
        0x00..=0x7f => 1,
        0xc2..=0xdf => 2,
        0xe0..=0xef => 3,
        0xf0..=0xf4 => 4,
        _ => return (None, 1),
    };
    match bytes.get(..width).map(std::str::from_utf8) {
        Some(Ok(text)) => (text.chars().next(), width),
        _ => (None, 1),
    }
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

/// Parses a validated JSON number as Go's `strconv.ParseFloat` does; out of
/// range gives ±Inf, as Go stores it before reporting the error it ignores.
fn parse_float(token: &[u8]) -> f64 {
    std::str::from_utf8(token)
        .ok()
        .and_then(|text| text.parse::<f64>().ok())
        .unwrap_or(0.0)
}

/// JSON whitespace: space, tab, newline and carriage return.
fn skip_ws(src: &[u8], mut pos: usize) -> usize {
    while matches!(src.get(pos), Some(b' ' | b'\t' | b'\n' | b'\r')) {
        pos += 1;
    }
    pos
}

/// An object key at `pos` (after whitespace) and its colon; returns the
/// decoded key and the position after the colon.
fn object_key(src: &[u8], pos: usize) -> Result<(String, usize), SyntaxError> {
    if src.get(pos) != Some(&b'"') {
        return Err(SyntaxError { offset: pos });
    }
    let end = string_end(src, pos)?;
    let key = unquote(&src[pos..end]);
    let colon = skip_ws(src, end);
    if src.get(colon) != Some(&b':') {
        return Err(SyntaxError { offset: colon });
    }
    Ok((key, colon + 1))
}

fn literal(src: &[u8], pos: usize, word: &[u8]) -> Result<usize, SyntaxError> {
    if src[pos..].starts_with(word) {
        Ok(pos + word.len())
    } else {
        Err(SyntaxError { offset: pos })
    }
}

/// The end of the string token starting at `pos` (a quote). Control
/// characters and unknown escapes are errors; any byte from 0x20 up,
/// including invalid UTF-8, is accepted, as Go accepts it.
fn string_end(src: &[u8], pos: usize) -> Result<usize, SyntaxError> {
    let mut index = pos + 1;
    loop {
        match src.get(index) {
            None => return Err(SyntaxError { offset: index }),
            Some(b'"') => return Ok(index + 1),
            Some(b'\\') => match src.get(index + 1) {
                Some(b'"' | b'\\' | b'/' | b'b' | b'f' | b'n' | b'r' | b't') => index += 2,
                Some(b'u') => {
                    let digits = src.get(index + 2..index + 6);
                    if !digits.is_some_and(|digits| digits.iter().all(u8::is_ascii_hexdigit)) {
                        return Err(SyntaxError { offset: index });
                    }
                    index += 6;
                }
                _ => return Err(SyntaxError { offset: index }),
            },
            Some(byte) if *byte < 0x20 => return Err(SyntaxError { offset: index }),
            Some(_) => index += 1,
        }
    }
}

/// The end of the number starting at `pos`:
/// `-?(0|[1-9][0-9]*)(\.[0-9]+)?([eE][+-]?[0-9]+)?`.
fn number_end(src: &[u8], pos: usize) -> Result<usize, SyntaxError> {
    let digits = |mut index: usize| -> Result<usize, SyntaxError> {
        let start = index;
        while src.get(index).is_some_and(u8::is_ascii_digit) {
            index += 1;
        }
        if index == start {
            Err(SyntaxError { offset: index })
        } else {
            Ok(index)
        }
    };
    let mut index = pos;
    if src.get(index) == Some(&b'-') {
        index += 1;
    }
    index = match src.get(index) {
        Some(b'0') => index + 1,
        Some(b'1'..=b'9') => digits(index)?,
        _ => return Err(SyntaxError { offset: index }),
    };
    if src.get(index) == Some(&b'.') {
        index = digits(index + 1)?;
    }
    if matches!(src.get(index), Some(b'e' | b'E')) {
        index += 1;
        if matches!(src.get(index), Some(b'+' | b'-')) {
            index += 1;
        }
        index = digits(index)?;
    }
    Ok(index)
}

/// Appends the string token `token` (validated) to `out`, escaping literal
/// characters as `format` asks and keeping everything else byte for byte.
fn copy_string(out: &mut Vec<u8>, token: &[u8], format: Format) {
    if !format.escape_html && !format.escape_js {
        out.extend_from_slice(token);
        return;
    }
    let mut start = 0;
    let mut index = 0;
    while index < token.len() {
        let byte = token[index];
        let escape: Option<&[u8]> = match byte {
            b'<' if format.escape_html => Some(b"\\u003c"),
            b'>' if format.escape_html => Some(b"\\u003e"),
            b'&' if format.escape_html => Some(b"\\u0026"),
            0xe2 if format.escape_js && token.get(index + 1) == Some(&0x80) => {
                match token.get(index + 2) {
                    Some(0xa8) => Some(b"\\u2028"),
                    Some(0xa9) => Some(b"\\u2029"),
                    _ => None,
                }
            }
            _ => None,
        };
        match escape {
            Some(escape) => {
                out.extend_from_slice(&token[start..index]);
                out.extend_from_slice(escape);
                index += if byte == 0xe2 { 3 } else { 1 };
                start = index;
            }
            None => index += 1,
        }
    }
    out.extend_from_slice(&token[start..]);
}

fn newline(out: &mut Option<&mut Vec<u8>>, format: Format, depth: usize) {
    if let (Some(out), Some(unit)) = (out.as_deref_mut(), format.indent) {
        out.push(b'\n');
        for _ in 0..depth {
            out.extend_from_slice(unit.as_bytes());
        }
    }
}

fn emit(out: &mut Option<&mut Vec<u8>>, bytes: &[u8]) {
    if let Some(out) = out.as_deref_mut() {
        out.extend_from_slice(bytes);
    }
}

/// Walks one JSON value starting at `pos` (no leading whitespace), writing
/// it to `out` in `format` when given; returns the end of the value. The
/// walk is iterative so that deep nesting cannot overflow the stack.
fn walk(
    src: &[u8],
    mut pos: usize,
    format: Format,
    mut out: Option<&mut Vec<u8>>,
) -> Result<usize, SyntaxError> {
    // `true` for an object, `false` for an array.
    let mut stack: Vec<bool> = Vec::new();
    // Writes a key at `pos` and its colon; returns the position after it.
    let key = |src: &[u8], pos: usize, out: &mut Option<&mut Vec<u8>>| {
        if src.get(pos) != Some(&b'"') {
            return Err(SyntaxError { offset: pos });
        }
        let end = string_end(src, pos)?;
        if let Some(out) = out.as_deref_mut() {
            copy_string(out, &src[pos..end], format);
        }
        let colon = skip_ws(src, end);
        if src.get(colon) != Some(&b':') {
            return Err(SyntaxError { offset: colon });
        }
        emit(out, if format.indent.is_some() { b": " } else { b":" });
        Ok(skip_ws(src, colon + 1))
    };
    loop {
        match src.get(pos) {
            Some(open @ (b'{' | b'[')) => {
                let object = *open == b'{';
                if stack.len() >= MAX_DEPTH {
                    return Err(SyntaxError { offset: pos });
                }
                emit(&mut out, &[*open]);
                pos = skip_ws(src, pos + 1);
                let close = if object { b'}' } else { b']' };
                if src.get(pos) == Some(&close) {
                    emit(&mut out, &[close]);
                    pos += 1;
                } else {
                    stack.push(object);
                    newline(&mut out, format, stack.len());
                    if object {
                        pos = key(src, pos, &mut out)?;
                    }
                    continue;
                }
            }
            Some(b'"') => {
                let end = string_end(src, pos)?;
                if let Some(out) = out.as_deref_mut() {
                    copy_string(out, &src[pos..end], format);
                }
                pos = end;
            }
            Some(b't') => {
                pos = literal(src, pos, b"true")?;
                emit(&mut out, b"true");
            }
            Some(b'f') => {
                pos = literal(src, pos, b"false")?;
                emit(&mut out, b"false");
            }
            Some(b'n') => {
                pos = literal(src, pos, b"null")?;
                emit(&mut out, b"null");
            }
            Some(b'-' | b'0'..=b'9') => {
                let end = number_end(src, pos)?;
                emit(&mut out, &src[pos..end]);
                pos = end;
            }
            _ => return Err(SyntaxError { offset: pos }),
        }
        // After a value: a separator, or the end of one or more containers.
        loop {
            let Some(&object) = stack.last() else {
                return Ok(pos);
            };
            pos = skip_ws(src, pos);
            let close = if object { b'}' } else { b']' };
            match src.get(pos) {
                Some(b',') => {
                    emit(&mut out, b",");
                    newline(&mut out, format, stack.len());
                    pos = skip_ws(src, pos + 1);
                    if object {
                        pos = key(src, pos, &mut out)?;
                    }
                    break;
                }
                Some(byte) if *byte == close => {
                    stack.pop();
                    newline(&mut out, format, stack.len());
                    emit(&mut out, &[close]);
                    pos += 1;
                }
                _ => return Err(SyntaxError { offset: pos }),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn marshal(text: &str) -> String {
        String::from_utf8(quoted(text)).unwrap()
    }

    #[test]
    fn strings_are_escaped_as_go_marshal_escapes_them() {
        assert_eq!(
            marshal("a<b>&c\u{2028}\u{2029}\u{8}\u{c}\u{1}\u{7f}\t\n\"\\/\u{fc}"),
            "\"a\\u003cb\\u003e\\u0026c\\u2028\\u2029\\b\\f\\u0001\u{7f}\\t\\n\\\"\\\\/\u{fc}\""
        );
        let mut out = Vec::new();
        quote(&mut out, "<&>\u{2028}", false);
        assert_eq!(out, b"\"<&>\\u2028\"");
    }

    #[test]
    fn floats_are_formatted_as_go_formats_them() {
        let cases: &[(f64, &str)] = &[
            (1.0, "1"),
            (2.5, "2.5"),
            (100.0, "100"),
            (1e20, "100000000000000000000"),
            (1e21, "1e+21"),
            (1e-6, "0.000001"),
            (1e-7, "1e-7"),
            (0.000_001_234, "0.000001234"),
            (123_456_789_012_345_680.0, "123456789012345680"),
            (-0.0, "-0"),
            (0.0, "0"),
            (1.5e300, "1.5e+300"),
            (1.234e-10, "1.234e-10"),
            (3.000_000_000_000_000_4, "3.0000000000000004"),
        ];
        for (number, want) in cases {
            assert_eq!(format_float(*number).unwrap(), *want, "{number}");
        }
        assert_eq!(
            format_float(f64::INFINITY).unwrap_err().to_string(),
            "json: unsupported value: +Inf"
        );
    }

    #[test]
    fn marshal_reformat_escapes_literals_and_keeps_escapes_and_numbers() {
        let raw =
            "{ \"a<\" : \"x \\u003c <&> \u{2028} \\u00fc \u{fc}\" , \"n\" : [ 1.50 , 1e2 , -0 ] }";
        let out = reformat(raw.as_bytes(), Format::MARSHAL).unwrap();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "{\"a\\u003c\":\"x \\u003c \\u003c\\u0026\\u003e \\u2028 \\u00fc \u{fc}\",\"n\":[1.50,1e2,-0]}"
        );
        let compact = compact(raw.as_bytes()).unwrap();
        assert_eq!(
            String::from_utf8(compact).unwrap(),
            "{\"a<\":\"x \\u003c <&> \u{2028} \\u00fc \u{fc}\",\"n\":[1.50,1e2,-0]}"
        );
    }

    #[test]
    fn invalid_utf8_inside_raw_strings_is_kept() {
        let raw = b"{\"t\":\"a\xffb\"}";
        assert!(is_valid(raw));
        assert_eq!(reformat(raw, Format::MARSHAL).unwrap(), raw.to_vec());
        assert_eq!(unquote(b"\"a\xffb\""), "a\u{fffd}b");
    }

    #[test]
    fn output_is_indented_like_go_encoder() {
        let raw = br#"{"issue":{"id":"x","e":{},"a":[],"n":[1,{"k":"<&>"}]},"ok":true}"#;
        let out = to_output(raw).unwrap();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "{\n  \"issue\": {\n    \"id\": \"x\",\n    \"e\": {},\n    \"a\": [],\n    \"n\": [\n      1,\n      {\n        \"k\": \"<&>\"\n      }\n    ]\n  },\n  \"ok\": true\n}\n"
        );
    }

    #[test]
    fn validation_follows_the_json_grammar() {
        for good in [
            "{}",
            " [1, -0.5e+3, true, false, null, \"\\ud800\"] ",
            "\"\\u00FC\"",
            "0",
        ] {
            assert!(is_valid(good.as_bytes()), "{good}");
        }
        for bad in [
            "",
            "{",
            "{not json",
            "{\"a\":1,}",
            "[1,]",
            "01",
            "1.",
            "-",
            "1e",
            "tru",
            "\"\\x\"",
            "\"a\nb\"",
            "{\"a\" 1}",
            "{} x",
            "{1:2}",
            "\"\\u12\"",
        ] {
            assert!(!is_valid(bad.as_bytes()), "{bad:?}");
        }
        let deep = "[".repeat(MAX_DEPTH) + &"]".repeat(MAX_DEPTH);
        assert!(is_valid(deep.as_bytes()));
        let too_deep = "[".repeat(MAX_DEPTH + 1) + &"]".repeat(MAX_DEPTH + 1);
        assert!(!is_valid(too_deep.as_bytes()));
        assert!(Value::decode(deep.as_bytes()).is_ok());
    }

    #[test]
    fn objects_keep_first_position_and_last_value() {
        let members = parse_object(br#" {"b":1,"a": [ 1 ],"b":{"x":2},"t\u0069":3} "#).unwrap();
        assert_eq!(members.keys(), ["b", "a", "ti"]);
        assert_eq!(members.get("b"), Some(&br#"{"x":2}"#[..]));
        assert_eq!(members.get("a"), Some(&b"[ 1 ]"[..]));
        assert!(parse_object(b"[1]").is_err());
        assert!(parse_object(b"null").is_err());
    }

    #[test]
    fn strings_decode_as_go_decodes_them() {
        assert_eq!(
            unquote(br#""\ud83d\ude00 \udc00 \ud800\u0041 \/ \u00fc""#),
            "\u{1f600} \u{fffd} \u{fffd}A / \u{fc}"
        );
        // A truncated multi-byte sequence decodes byte by byte, as Go does.
        assert_eq!(unquote(b"\"\xe2\x82A\""), "\u{fffd}\u{fffd}A");
    }

    #[test]
    fn values_decode_as_go_any() {
        let value =
            Value::decode(br#"{"b":1.50,"a":[true,null,"x"],"a":2,"n":1e400,"z":-0}"#).unwrap();
        let Value::Object(map) = &value else {
            panic!("an object");
        };
        assert_eq!(map["a"], Value::Number(2.0));
        assert_eq!(map["b"], Value::Number(1.5));
        assert_eq!(map["n"], Value::Number(f64::INFINITY));
        let mut out = Vec::new();
        assert!(value.encode(&mut out, true).is_err());
        let value = Value::decode(br#"{"html":"<&>","keep":{"n":[1,2.5,null]},"z":-0}"#).unwrap();
        let mut out = Vec::new();
        value.encode(&mut out, true).unwrap();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            r#"{"html":"\u003c\u0026\u003e","keep":{"n":[1,2.5,null]},"z":-0}"#
        );
    }
}
