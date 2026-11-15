//! JSON exactly as Go's `encoding/json` writes and reads it (assumption AS7).
//!
//! Writing: compact output, struct fields in declaration order (the callers
//! in `model.rs` write them in that order), map keys sorted bytewise, `<`,
//! `>`, `&`, U+2028 and U+2029 escaped, and every number held in a Go `any`
//! (or a `float64`) formatted as Go formats a `float64`.
//!
//! Reading: [`parse`] accepts exactly the grammar Go's scanner accepts and
//! keeps what Go's decoder needs (key order, repeated keys, the number text).
//! The `*_into` helpers then decode with Go's rules: `null` leaves a string,
//! number or struct unchanged and clears a slice, map or pointer; a value is
//! decoded *into* what an earlier repeated key left behind; a key binds to a
//! struct field by exact name, else by Go's case folding; numbers are parsed
//! from their text with correct rounding; a type mismatch is an error.
//! serde_json is used for the `serde_json::Value` documents only.

use crate::JsonValue;
use serde_json::{Map, Number, Value};
use std::borrow::Borrow;
use std::collections::BTreeMap;
use std::fmt::Write as _;

// ---- Writing -----------------------------------------------------------------

/// Appends `text` as a Go `encoding/json` string (with HTML escaping on, as
/// `json.Marshal` has it).
pub(crate) fn write_string(out: &mut String, text: &str) {
    out.push('"');
    for character in text.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '<' | '>' | '&' | '\u{0}'..='\u{1f}' | '\u{2028}' | '\u{2029}' => {
                let _ = write!(out, "\\u{:04x}", u32::from(character));
            }
            _ => out.push(character),
        }
    }
    out.push('"');
}

/// Appends a Go `float64` as `encoding/json` writes it: the shortest digits
/// that read back to the same value, in plain form for `1e-6 <= |x| < 1e21`
/// (and for zero, including `-0`), otherwise in exponent form with an
/// explicit sign and no zero padding (`1e+21`, `1e-7`).
///
/// Go refuses to encode NaN and infinities; no value the library reads or
/// builds can hold one (decoding rejects out-of-range numbers), so `null` is
/// written as a defined fallback.
pub(crate) fn write_float(out: &mut String, value: f64) {
    if !value.is_finite() {
        out.push_str("null");
        return;
    }
    let magnitude = value.abs();
    if magnitude != 0.0 && !(1e-6..1e21).contains(&magnitude) {
        let formatted = format!("{value:e}");
        let (mantissa, exponent) = formatted
            .split_once('e')
            .expect("exponent formatting has an `e`");
        out.push_str(mantissa);
        out.push('e');
        if !exponent.starts_with('-') {
            out.push('+');
        }
        out.push_str(exponent);
    } else {
        let _ = write!(out, "{value}");
    }
}

/// Appends a Go `int`.
pub(crate) fn write_int(out: &mut String, value: i64) {
    let _ = write!(out, "{value}");
}

/// Appends a Go `bool`.
pub(crate) fn write_bool(out: &mut String, value: bool) {
    out.push_str(if value { "true" } else { "false" });
}

/// Appends a Go `[]string`; `None` is a nil slice and writes `null`.
pub(crate) fn write_strings(out: &mut String, values: Option<&[String]>) {
    match values {
        None => out.push_str("null"),
        Some(values) => {
            out.push('[');
            for (index, value) in values.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                write_string(out, value);
            }
            out.push(']');
        }
    }
}

/// Appends a value held in a Go `any` (decoded from JSON): every number is a
/// `float64`, objects are `map[string]any` with sorted keys. Nesting is
/// walked with an explicit stack, not by recursion.
pub(crate) fn write_any(out: &mut String, value: &Value) {
    let mut pending = vec![Pending::Value(value)];
    write_pending(out, &mut pending);
}

/// Appends a Go `map[string]any`; keys bytewise sorted as Go sorts them.
pub(crate) fn write_map<V: Borrow<Value>>(out: &mut String, map: &BTreeMap<String, V>) {
    // BTreeMap<String, _> iterates in byte order of the UTF-8 keys.
    let mut pending = Vec::new();
    push_object(
        &mut pending,
        map.iter()
            .map(|(key, value)| (key, value.borrow()))
            .collect(),
    );
    write_pending(out, &mut pending);
}

/// What remains to be written, last item first.
enum Pending<'a> {
    Value(&'a Value),
    Key(&'a str),
    Text(&'static str),
}

fn write_pending(out: &mut String, pending: &mut Vec<Pending<'_>>) {
    while let Some(item) = pending.pop() {
        match item {
            Pending::Text(text) => out.push_str(text),
            Pending::Key(key) => {
                write_string(out, key);
                out.push(':');
            }
            Pending::Value(Value::Null) => out.push_str("null"),
            Pending::Value(Value::Bool(flag)) => write_bool(out, *flag),
            Pending::Value(Value::Number(number)) => {
                write_float(out, number.as_f64().unwrap_or(f64::NAN));
            }
            Pending::Value(Value::String(text)) => write_string(out, text),
            Pending::Value(Value::Array(items)) => {
                pending.push(Pending::Text("]"));
                for (index, item) in items.iter().enumerate().rev() {
                    pending.push(Pending::Value(item));
                    if index > 0 {
                        pending.push(Pending::Text(","));
                    }
                }
                pending.push(Pending::Text("["));
            }
            Pending::Value(Value::Object(map)) => {
                // Sort explicitly: serde_json's map is ordered by insertion
                // when its `preserve_order` feature is on anywhere in the build.
                let mut entries: Vec<(&String, &Value)> = map.iter().collect();
                entries.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
                push_object(pending, entries);
            }
        }
    }
}

/// Queues an object whose entries are already in output order.
fn push_object<'a>(pending: &mut Vec<Pending<'a>>, entries: Vec<(&'a String, &'a Value)>) {
    pending.push(Pending::Text("}"));
    for (index, (key, value)) in entries.into_iter().enumerate().rev() {
        pending.push(Pending::Value(value));
        pending.push(Pending::Key(key));
        if index > 0 {
            pending.push(Pending::Text(","));
        }
    }
    pending.push(Pending::Text("{"));
}

/// Writes one JSON object field by field, in the order of the calls.
pub(crate) struct ObjectWriter<'a> {
    out: &'a mut String,
    empty: bool,
}

impl<'a> ObjectWriter<'a> {
    pub(crate) fn new(out: &'a mut String) -> Self {
        out.push('{');
        Self { out, empty: true }
    }

    /// Starts a field and returns the buffer to write its value into.
    pub(crate) fn key(&mut self, name: &str) -> &mut String {
        if !self.empty {
            self.out.push(',');
        }
        self.empty = false;
        write_string(self.out, name);
        self.out.push(':');
        self.out
    }

    pub(crate) fn string(&mut self, name: &str, value: &str) {
        write_string(self.key(name), value);
    }

    /// A string field tagged `omitempty`.
    pub(crate) fn string_omitempty(&mut self, name: &str, value: &str) {
        if !value.is_empty() {
            self.string(name, value);
        }
    }

    pub(crate) fn int(&mut self, name: &str, value: i64) {
        write_int(self.key(name), value);
    }

    /// A `[]string` field tagged `omitempty` (nil and empty are both left out).
    pub(crate) fn strings_omitempty(&mut self, name: &str, values: &[String]) {
        if !values.is_empty() {
            write_strings(self.key(name), Some(values));
        }
    }

    pub(crate) fn finish(self) {
        self.out.push('}');
    }
}

// ---- Reading -----------------------------------------------------------------

/// Go's `maxNestingDepth` for the scanner.
const MAX_DEPTH: usize = 10_000;

/// A parsed JSON value that keeps what Go's decoder sees: object keys in
/// order (repeats included) and the literal text of every number.
#[derive(Debug, PartialEq)]
pub(crate) enum GoValue {
    Null,
    Bool(bool),
    Number(String),
    String(String),
    Array(Vec<GoValue>),
    Object(Vec<(String, GoValue)>),
}

impl GoValue {
    fn kind(&self) -> &'static str {
        match self {
            GoValue::Null => "null",
            GoValue::Bool(_) => "bool",
            GoValue::Number(_) => "number",
            GoValue::String(_) => "string",
            GoValue::Array(_) => "array",
            GoValue::Object(_) => "object",
        }
    }
}

/// Parses one JSON document with the grammar of Go's scanner: RFC 8259
/// syntax, white space limited to space, tab, CR and LF, at most 10000 levels
/// of nesting, nothing but white space after the value. Inside strings,
/// invalid UTF-8 is replaced byte by byte with U+FFFD and an escaped lone
/// surrogate becomes U+FFFD, as Go's decoder does.
///
/// The parser keeps its own stack of open arrays and objects instead of
/// recursing, so the deepest nesting Go accepts cannot exhaust the thread's
/// stack.
pub(crate) fn parse(input: &[u8]) -> Result<GoValue, String> {
    let mut parser = Parser { input, offset: 0 };
    let value = parser.document()?;
    parser.skip_space();
    if parser.offset < input.len() {
        return Err(parser.unexpected("after top-level value"));
    }
    Ok(value)
}

/// An array or object whose closing bracket has not been read yet.
enum Open {
    Array(Vec<GoValue>),
    /// The entries so far and the key of the value being read.
    Object(Vec<(String, GoValue)>, String),
}

struct Parser<'a> {
    input: &'a [u8],
    offset: usize,
}

impl Parser<'_> {
    fn peek(&self) -> Option<u8> {
        self.input.get(self.offset).copied()
    }

    fn skip_space(&mut self) {
        while let Some(b' ' | b'\t' | b'\n' | b'\r') = self.peek() {
            self.offset += 1;
        }
    }

    fn unexpected(&self, context: &str) -> String {
        match self.peek() {
            None => "unexpected end of JSON input".to_owned(),
            Some(byte) => format!(
                "invalid character {} {context} at offset {}",
                describe(byte),
                self.offset + 1
            ),
        }
    }

    fn expect(&mut self, byte: u8, context: &str) -> Result<(), String> {
        if self.peek() == Some(byte) {
            self.offset += 1;
            Ok(())
        } else {
            Err(self.unexpected(context))
        }
    }

    /// A key and its colon, white space around both skipped.
    fn object_key(&mut self) -> Result<String, String> {
        self.skip_space();
        if self.peek() != Some(b'"') {
            return Err(self.unexpected("looking for beginning of object key string"));
        }
        let key = self.string()?;
        self.skip_space();
        self.expect(b':', "after object key")?;
        Ok(key)
    }

    fn document(&mut self) -> Result<GoValue, String> {
        let mut open: Vec<Open> = Vec::new();
        loop {
            // Read the start of a value: a scalar is complete at once; an
            // array or object is complete at once only when empty.
            self.skip_space();
            let mut value = match self.peek() {
                Some(bracket @ (b'[' | b'{')) => {
                    self.offset += 1;
                    if open.len() >= MAX_DEPTH {
                        return Err("exceeded max depth".to_owned());
                    }
                    self.skip_space();
                    match (bracket, self.peek()) {
                        (b'[', Some(b']')) => {
                            self.offset += 1;
                            GoValue::Array(Vec::new())
                        }
                        (b'{', Some(b'}')) => {
                            self.offset += 1;
                            GoValue::Object(Vec::new())
                        }
                        (b'[', _) => {
                            open.push(Open::Array(Vec::new()));
                            continue;
                        }
                        _ => {
                            let key = self.object_key()?;
                            open.push(Open::Object(Vec::new(), key));
                            continue;
                        }
                    }
                }
                Some(b'"') => GoValue::String(self.string()?),
                Some(b'-' | b'0'..=b'9') => self.number()?,
                Some(b't') => self.literal("true", GoValue::Bool(true))?,
                Some(b'f') => self.literal("false", GoValue::Bool(false))?,
                Some(b'n') => self.literal("null", GoValue::Null)?,
                _ => return Err(self.unexpected("looking for beginning of value")),
            };
            // Hand the complete value to the innermost open container; close
            // containers for as long as their closing bracket follows.
            loop {
                let Some(innermost) = open.last_mut() else {
                    return Ok(value);
                };
                self.skip_space();
                match innermost {
                    Open::Array(items) => {
                        items.push(value);
                        match self.peek() {
                            Some(b',') => {
                                self.offset += 1;
                                break;
                            }
                            Some(b']') => {
                                self.offset += 1;
                                let Some(Open::Array(items)) = open.pop() else {
                                    unreachable!("the innermost container is an array");
                                };
                                value = GoValue::Array(items);
                            }
                            _ => return Err(self.unexpected("after array element")),
                        }
                    }
                    Open::Object(entries, key) => {
                        entries.push((std::mem::take(key), value));
                        match self.peek() {
                            Some(b',') => {
                                self.offset += 1;
                                *key = self.object_key()?;
                                break;
                            }
                            Some(b'}') => {
                                self.offset += 1;
                                let Some(Open::Object(entries, _)) = open.pop() else {
                                    unreachable!("the innermost container is an object");
                                };
                                value = GoValue::Object(entries);
                            }
                            _ => return Err(self.unexpected("after object key:value pair")),
                        }
                    }
                }
            }
        }
    }

    fn literal(&mut self, word: &str, value: GoValue) -> Result<GoValue, String> {
        for expected in word.bytes() {
            if self.peek() != Some(expected) {
                return Err(self.unexpected(&format!("in literal {word}")));
            }
            self.offset += 1;
        }
        Ok(value)
    }

    fn number(&mut self) -> Result<GoValue, String> {
        let start = self.offset;
        if self.peek() == Some(b'-') {
            self.offset += 1;
        }
        match self.peek() {
            Some(b'0') => self.offset += 1,
            Some(b'1'..=b'9') => self.digits(),
            _ => return Err(self.unexpected("in numeric literal")),
        }
        if self.peek() == Some(b'.') {
            self.offset += 1;
            if !matches!(self.peek(), Some(b'0'..=b'9')) {
                return Err(self.unexpected("after decimal point in numeric literal"));
            }
            self.digits();
        }
        if let Some(b'e' | b'E') = self.peek() {
            self.offset += 1;
            if let Some(b'+' | b'-') = self.peek() {
                self.offset += 1;
            }
            if !matches!(self.peek(), Some(b'0'..=b'9')) {
                return Err(self.unexpected("in exponent of numeric literal"));
            }
            self.digits();
        }
        let text = std::str::from_utf8(&self.input[start..self.offset])
            .expect("a numeric literal is ASCII")
            .to_owned();
        Ok(GoValue::Number(text))
    }

    fn digits(&mut self) {
        while let Some(b'0'..=b'9') = self.peek() {
            self.offset += 1;
        }
    }

    fn hex4(&self, at: usize) -> Option<u32> {
        let digits = self.input.get(at..at + 4)?;
        let text = std::str::from_utf8(digits).ok()?;
        if !text.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return None;
        }
        u32::from_str_radix(text, 16).ok()
    }

    fn string(&mut self) -> Result<String, String> {
        self.offset += 1; // opening quote
        let mut out = String::new();
        loop {
            let Some(byte) = self.peek() else {
                return Err("unexpected end of JSON input".to_owned());
            };
            match byte {
                b'"' => {
                    self.offset += 1;
                    return Ok(out);
                }
                b'\\' => {
                    self.offset += 1;
                    let Some(escape) = self.peek() else {
                        return Err("unexpected end of JSON input".to_owned());
                    };
                    self.offset += 1;
                    match escape {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{8}'),
                        b'f' => out.push('\u{c}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => {
                            let Some(first) = self.hex4(self.offset) else {
                                return Err(self.bad_unicode_escape());
                            };
                            self.offset += 4;
                            out.push(self.unicode_escape(first));
                        }
                        _ => {
                            self.offset -= 1;
                            return Err(self.unexpected("in string escape code"));
                        }
                    }
                }
                0x00..=0x1f => return Err(self.unexpected("in string literal")),
                0x20..=0x7f => {
                    out.push(char::from(byte));
                    self.offset += 1;
                }
                _ => {
                    let (character, width) = decode_rune(&self.input[self.offset..]);
                    out.push(character);
                    self.offset += width;
                }
            }
        }
    }

    /// The character for `\uXXXX` whose value is `first`, consuming a
    /// following `\uXXXX` when the two form a surrogate pair.
    fn unicode_escape(&mut self, first: u32) -> char {
        if !(0xd800..0xe000).contains(&first) {
            return char::from_u32(first).unwrap_or('\u{fffd}');
        }
        let second = if self.input.get(self.offset..self.offset + 2) == Some(b"\\u") {
            self.hex4(self.offset + 2)
        } else {
            None
        };
        match second {
            Some(low) if (0xd800..0xdc00).contains(&first) && (0xdc00..0xe000).contains(&low) => {
                self.offset += 6;
                let code = 0x10000 + ((first - 0xd800) << 10) + (low - 0xdc00);
                char::from_u32(code).unwrap_or('\u{fffd}')
            }
            _ => '\u{fffd}',
        }
    }

    fn bad_unicode_escape(&mut self) -> String {
        let mut at = self.offset;
        while at < self.input.len().min(self.offset + 4) && self.input[at].is_ascii_hexdigit() {
            at += 1;
        }
        self.offset = at;
        self.unexpected("in \\u hexadecimal character escape")
    }
}

/// Go's `utf8.DecodeRune` on a non-ASCII lead byte: the character and its
/// width, or U+FFFD and width 1 when the bytes are not valid UTF-8.
fn decode_rune(bytes: &[u8]) -> (char, usize) {
    let width = match bytes[0] {
        0xc2..=0xdf => 2,
        0xe0..=0xef => 3,
        0xf0..=0xf4 => 4,
        _ => return ('\u{fffd}', 1),
    };
    match bytes.get(..width).map(std::str::from_utf8) {
        Some(Ok(text)) => (text.chars().next().expect("one character"), width),
        _ => ('\u{fffd}', 1),
    }
}

fn describe(byte: u8) -> String {
    match byte {
        b'\'' => "'\\''".to_owned(),
        b'"' => "'\"'".to_owned(),
        0x20..=0x7e => format!("'{}'", char::from(byte)),
        _ => format!("0x{byte:02x}"),
    }
}

// ---- Decoding with Go's rules --------------------------------------------------

/// Go's key folding for struct fields (`encoding/json/fold.go`) as far as
/// it can matter for ASCII field names: ASCII letters fold to upper case, and
/// the two non-ASCII letters whose fold set contains an ASCII letter (U+017F
/// LATIN SMALL LETTER LONG S and U+212A KELVIN SIGN) fold to `S` and `K`.
fn fold(text: &str) -> String {
    text.chars()
        .map(|character| match character {
            '\u{17f}' => 'S',
            '\u{212a}' => 'K',
            _ => character.to_ascii_uppercase(),
        })
        .collect()
}

/// The struct field a JSON key binds to: an exact match first, otherwise the
/// first field whose folded name equals the folded key. `None` for an unknown
/// key, which Go ignores.
pub(crate) fn field<'f>(key: &str, fields: &[&'f str]) -> Option<&'f str> {
    if let Some(found) = fields.iter().find(|name| **name == key) {
        return Some(found);
    }
    let folded = fold(key);
    fields.iter().find(|name| fold(name) == folded).copied()
}

fn mismatch(value: &GoValue, target: &str) -> String {
    format!(
        "cannot unmarshal {} into Go value of type {target}",
        value.kind()
    )
}

/// The entries of a JSON object, `None` for `null` (which leaves a struct
/// unchanged), an error for anything else.
pub(crate) fn object_entries<'v>(
    value: &'v GoValue,
    target: &str,
) -> Result<Option<&'v [(String, GoValue)]>, String> {
    match value {
        GoValue::Null => Ok(None),
        GoValue::Object(entries) => Ok(Some(entries)),
        other => Err(mismatch(other, target)),
    }
}

/// Into a Go `string`: `null` leaves it unchanged.
pub(crate) fn string_into(target: &mut String, value: &GoValue) -> Result<(), String> {
    match value {
        GoValue::Null => Ok(()),
        GoValue::String(text) => {
            target.clone_from(text);
            Ok(())
        }
        other => Err(mismatch(other, "string")),
    }
}

/// `strconv.ParseInt(text, 10, 64)` on a JSON number literal.
fn parse_int(text: &str) -> Result<i64, String> {
    text.parse::<i64>()
        .map_err(|_| format!("cannot unmarshal number {text} into Go value of type int"))
}

/// `strconv.ParseFloat(text, 64)` on a JSON number literal: correctly
/// rounded; out of range is an error.
fn parse_float(text: &str) -> Result<f64, String> {
    match text.parse::<f64>() {
        Ok(number) if number.is_finite() => Ok(number),
        _ => Err(format!(
            "cannot unmarshal number {text} into Go value of type float64"
        )),
    }
}

/// Into a Go `int`: `null` leaves it unchanged.
pub(crate) fn int_into(target: &mut i64, value: &GoValue) -> Result<(), String> {
    match value {
        GoValue::Null => Ok(()),
        GoValue::Number(text) => {
            *target = parse_int(text)?;
            Ok(())
        }
        other => Err(mismatch(other, "int")),
    }
}

/// Into a Go `*int`: `null` sets nil.
pub(crate) fn optional_int_into(target: &mut Option<i64>, value: &GoValue) -> Result<(), String> {
    match value {
        GoValue::Null => {
            *target = None;
            Ok(())
        }
        GoValue::Number(text) => {
            *target = Some(parse_int(text)?);
            Ok(())
        }
        other => Err(mismatch(other, "int")),
    }
}

/// Into a Go `*float64`: `null` sets nil.
pub(crate) fn optional_float_into(target: &mut Option<f64>, value: &GoValue) -> Result<(), String> {
    match value {
        GoValue::Null => {
            *target = None;
            Ok(())
        }
        GoValue::Number(text) => {
            *target = Some(parse_float(text)?);
            Ok(())
        }
        other => Err(mismatch(other, "float64")),
    }
}

/// Into a Go `bool`: `null` leaves it unchanged.
pub(crate) fn bool_into(target: &mut bool, value: &GoValue) -> Result<(), String> {
    match value {
        GoValue::Null => Ok(()),
        GoValue::Bool(flag) => {
            *target = *flag;
            Ok(())
        }
        other => Err(mismatch(other, "bool")),
    }
}

/// Into a Go `*bool`: `null` sets nil.
pub(crate) fn optional_bool_into(target: &mut Option<bool>, value: &GoValue) -> Result<(), String> {
    match value {
        GoValue::Null => {
            *target = None;
            Ok(())
        }
        GoValue::Bool(flag) => {
            *target = Some(*flag);
            Ok(())
        }
        other => Err(mismatch(other, "bool")),
    }
}

/// Into a Go slice held as `Option<Vec<T>>` (`None` is nil): `null` sets nil;
/// an array decodes element `i` into the existing element `i` when there is
/// one (as Go reuses the slice's backing array), appends otherwise, and
/// truncates to the array's length; `[]` gives an empty, non-nil slice.
pub(crate) fn slice_into<T: Default>(
    target: &mut Option<Vec<T>>,
    value: &GoValue,
    element_into: impl Fn(&mut T, &GoValue) -> Result<(), String>,
) -> Result<(), String> {
    match value {
        GoValue::Null => {
            *target = None;
            Ok(())
        }
        GoValue::Array(items) => {
            let slice = target.get_or_insert_with(Vec::new);
            for (index, item) in items.iter().enumerate() {
                if index == slice.len() {
                    slice.push(T::default());
                }
                element_into(&mut slice[index], item)?;
            }
            slice.truncate(items.len());
            Ok(())
        }
        other => Err(mismatch(other, "slice")),
    }
}

/// Into a Go `[]string` field tagged `omitempty`, held as a plain `Vec`
/// (nil and empty write the same, so both are an empty `Vec`).
pub(crate) fn strings_into(target: &mut Vec<String>, value: &GoValue) -> Result<(), String> {
    let mut slice = Some(std::mem::take(target));
    let result = slice_into(&mut slice, value, string_into);
    *target = slice.unwrap_or_default();
    result
}

/// Into a Go `map[string]any`: `null` sets nil (empty); an object adds its
/// keys to the map that is already there, a later key replacing an earlier
/// value.
pub(crate) fn map_into(
    target: &mut BTreeMap<String, JsonValue>,
    value: &GoValue,
) -> Result<(), String> {
    match value {
        GoValue::Null => {
            target.clear();
            Ok(())
        }
        GoValue::Object(entries) => {
            for (key, item) in entries {
                target.insert(key.clone(), to_any(item)?);
            }
            Ok(())
        }
        other => Err(mismatch(other, "map[string]interface {}")),
    }
}

/// A value decoded into a Go `any`: numbers become `float64`, objects
/// `map[string]any` (a later repeated key replacing an earlier one). Nesting
/// is walked with an explicit stack, not by recursion.
pub(crate) fn to_any(value: &GoValue) -> Result<JsonValue, String> {
    enum Step<'a> {
        Visit(&'a GoValue),
        Array(usize),
        Object(&'a [(String, GoValue)]),
    }
    let mut steps = vec![Step::Visit(value)];
    let mut done: Vec<JsonValue> = Vec::new();
    while let Some(step) = steps.pop() {
        match step {
            Step::Visit(GoValue::Null) => done.push(Value::Null.into()),
            Step::Visit(GoValue::Bool(flag)) => done.push(Value::Bool(*flag).into()),
            Step::Visit(GoValue::Number(text)) => {
                let number = parse_float(text)?;
                done.push(Value::Number(Number::from_f64(number).expect("a finite float")).into());
            }
            Step::Visit(GoValue::String(text)) => done.push(Value::String(text.clone()).into()),
            Step::Visit(GoValue::Array(items)) => {
                steps.push(Step::Array(items.len()));
                steps.extend(items.iter().rev().map(Step::Visit));
            }
            Step::Visit(GoValue::Object(entries)) => {
                steps.push(Step::Object(entries));
                steps.extend(entries.iter().rev().map(|(_, item)| Step::Visit(item)));
            }
            Step::Array(length) => {
                let items = done.split_off(done.len() - length);
                done.push(
                    Value::Array(items.into_iter().map(JsonValue::into_inner).collect()).into(),
                );
            }
            Step::Object(entries) => {
                let values = done.split_off(done.len() - entries.len());
                let mut map = Map::new();
                for ((key, _), item) in entries.iter().zip(values) {
                    if let Some(replaced) = map.insert(key.clone(), item.into_inner()) {
                        drop(JsonValue::from(replaced));
                    }
                }
                done.push(Value::Object(map).into());
            }
        }
    }
    Ok(done.pop().expect("one value remains"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Prompt, Schema, Section};
    use serde_json::json;

    fn string(text: &str) -> String {
        let mut out = String::new();
        write_string(&mut out, text);
        out
    }

    fn float(value: f64) -> String {
        let mut out = String::new();
        write_float(&mut out, value);
        out
    }

    /// Reads a number literal the way Go's decoder does for a `float64`.
    fn read_float(text: &str) -> f64 {
        match parse(text.as_bytes()).unwrap() {
            GoValue::Number(literal) => parse_float(&literal).unwrap(),
            other => panic!("not a number: {other:?}"),
        }
    }

    #[test]
    fn escapes_html_and_line_separators_like_go() {
        assert_eq!(string("<b>&</b>"), r#""\u003cb\u003e\u0026\u003c/b\u003e""#);
        assert_eq!(string("\u{2028}\u{2029}"), r#""\u2028\u2029""#);
        assert_eq!(string("q\"b\\s/"), r#""q\"b\\s/""#);
        assert_eq!(
            string("\u{8}\u{c}\n\r\t"),
            r#""\b\f\n\r\t""#,
            "Go's short escapes"
        );
        assert_eq!(
            string("\u{0}\u{1}\u{b}\u{1f}"),
            r#""\u0000\u0001\u000b\u001f""#
        );
        // DEL, other separators and non-ASCII text are written as they are.
        assert_eq!(
            string("\u{7f}\u{85}\u{a0}caf\u{e9}\u{1f600}\u{fffd}"),
            "\"\u{7f}\u{85}\u{a0}caf\u{e9}\u{1f600}\u{fffd}\""
        );
        // Raw and escaped input both read back to the same text.
        let raw = parse("\"<b>&</b> \u{2028}\u{2029}\"".as_bytes()).unwrap();
        let escaped = parse(br#""\u003cb\u003e\u0026\u003C/b\u003E \u2028\u2029""#).unwrap();
        assert_eq!(raw, escaped);
        assert_eq!(
            parse(br#""\/ \b\f \ud83d\ude00 \ud800x \udc00 \ud800\u0041""#).unwrap(),
            GoValue::String("/ \u{8}\u{c} \u{1f600} \u{fffd}x \u{fffd} \u{fffd}A".to_owned())
        );
        // Invalid UTF-8 in a string becomes U+FFFD byte by byte, as in Go.
        assert_eq!(
            parse(b"\"a\xe2\x82b\xffc\"").unwrap(),
            GoValue::String("a\u{fffd}\u{fffd}b\u{fffd}c".to_owned())
        );
        // What Go's scanner refuses.
        for bad in [
            &b"\"tab\there\""[..],
            b"\"\\'\"",
            b"\"\\u12\"",
            b"\"open",
            b"{\"a\":1,}",
            b"[1,]",
            b"01",
            b"1.",
            b"-",
            b"1e",
            b"+1",
            b"nul",
            b"{} x",
            b"\xef\xbb\xbf{}",
            b"{'a':1}",
            b"",
        ] {
            assert!(
                parse(bad).is_err(),
                "{:?} must be refused",
                String::from_utf8_lossy(bad)
            );
        }
        assert!(parse(b" \t\r\n{\"a\" : [ 1 , 2 ] }\r\n").is_ok());
    }

    #[test]
    fn formats_floats_like_go_encoding_json() {
        let cases: &[(&str, &str)] = &[
            ("1", "1"),
            ("1.0", "1"),
            ("-0", "-0"),
            ("-0.0", "-0"),
            ("0", "0"),
            ("0.000001", "0.000001"),
            ("1e-6", "0.000001"),
            ("1e-7", "1e-7"),
            ("0.00000099", "9.9e-7"),
            ("1e-10", "1e-10"),
            ("1e21", "1e+21"),
            ("-1e21", "-1e+21"),
            ("1e20", "100000000000000000000"),
            ("999999999999999900000", "999999999999999900000"),
            ("123456789012345678901", "123456789012345680000"),
            ("1E5", "100000"),
            ("0.1", "0.1"),
            ("100", "100"),
            ("2.5e-5", "0.000025"),
            ("1.5e300", "1.5e+300"),
            ("5e-324", "5e-324"),
            ("1.7976931348623157e308", "1.7976931348623157e+308"),
            ("-1.0", "-1"),
            ("0.30000000000000004", "0.30000000000000004"),
            ("3.141592653589793238462643383279", "3.141592653589793"),
            ("9007199254740993", "9007199254740992"),
            ("12345678901234567890123", "1.2345678901234568e+22"),
        ];
        for (literal, expected) in cases {
            assert_eq!(float(read_float(literal)), *expected, "literal {literal}");
        }
        // Out of range is an error on read, as strconv.ParseFloat's ErrRange.
        assert!(parse_float("1e400").is_err());
        assert!(parse_float("-1e400").is_err());
        assert_eq!(parse_float("1e-400"), Ok(0.0));
        // Integers in a Go int field must be integer literals.
        assert_eq!(parse_int("-0"), Ok(0));
        assert!(parse_int("1.0").is_err());
        assert!(parse_int("1e2").is_err());
        assert!(parse_int("9223372036854775808").is_err());
        // A Go any holds float64: integers in a Value are written as floats.
        let mut out = String::new();
        write_any(&mut out, &json!([1, -2, 1.5, u64::MAX, null, true, "x"]));
        assert_eq!(out, r#"[1,-2,1.5,18446744073709552000,null,true,"x"]"#);
    }

    #[test]
    fn keeps_null_distinct_from_an_empty_array() {
        let mut prompt = Prompt {
            id: "p-1".to_owned(),
            name: "n".to_owned(),
            version: 1,
            status: "active".to_owned(),
            created_at: "c".to_owned(),
            updated_at: "u".to_owned(),
            ..Prompt::default()
        };
        assert!(prompt.to_go_json().contains(r#""sections":null"#));
        prompt.sections = Some(Vec::new());
        assert!(prompt.to_go_json().contains(r#""sections":[]"#));
        // Read back: null stays nil, [] stays empty, absent is nil.
        for (line, expected) in [
            (r#"{"sections":null}"#, None),
            (r#"{"sections":[]}"#, Some(Vec::new())),
            (r#"{}"#, None),
            (
                r#"{"sections":[{"name":"a","body":"b"}]}"#,
                Some(vec![Section {
                    name: "a".to_owned(),
                    body: "b".to_owned(),
                    required: None,
                }]),
            ),
        ] {
            let read = Prompt::from_go_json(line.as_bytes()).unwrap();
            assert_eq!(read.sections, expected, "{line}");
        }
        let mut schema = Schema::default();
        assert!(schema.to_go_json().contains(r#""requiredSections":null"#));
        schema.required_sections = Some(Vec::new());
        assert!(schema.to_go_json().contains(r#""requiredSections":[]"#));
        let read = Schema::from_go_json(br#"{"requiredSections":[]}"#).unwrap();
        assert_eq!(read.required_sections, Some(Vec::new()));
        let read = Schema::from_go_json(br#"{"requiredSections":null}"#).unwrap();
        assert_eq!(read.required_sections, None);
        // Empty omitempty slices are left out either way.
        assert!(!schema.to_go_json().contains("optionalSections"));
        assert!(!schema.to_go_json().contains("rules"));
    }

    #[test]
    fn sorts_map_keys_bytewise() {
        let mut map = BTreeMap::new();
        for key in [
            "b",
            "B",
            "a",
            "_",
            "\u{e9}",
            "aa",
            "Z",
            "1",
            "\u{1f600}",
            "~",
        ] {
            map.insert(key.to_owned(), json!(key.len()));
        }
        let mut out = String::new();
        write_map(&mut out, &map);
        assert_eq!(
            out,
            "{\"1\":1,\"B\":1,\"Z\":1,\"_\":1,\"a\":1,\"aa\":2,\"b\":1,\"~\":1,\"\u{e9}\":2,\"\u{1f600}\":4}"
        );
        // Nested objects in a Go any are sorted the same way.
        let mut out = String::new();
        write_any(
            &mut out,
            &json!({"z": {"b": 1, "a": [{"y": 1, "x": 2}]}, "<": "&"}),
        );
        assert_eq!(
            out,
            r#"{"\u003c":"\u0026","z":{"a":[{"x":2,"y":1}],"b":1}}"#
        );
    }

    #[test]
    fn decodes_into_existing_values_like_go() {
        // A repeated key decodes into what the first one left behind.
        let line = br#"{"frontmatter":{"a":1},"frontmatter":{"b":2},"atlas":{"budget":1},"atlas":{"on_empty":"x"},"sections":[{"name":"s","required":true}],"sections":[{"body":"b"}]}"#;
        let prompt = Prompt::from_go_json(line).unwrap();
        assert_eq!(
            prompt.frontmatter.keys().collect::<Vec<_>>(),
            vec!["a", "b"]
        );
        let atlas = prompt.atlas.unwrap();
        assert_eq!(atlas.budget, Some(1.0));
        assert_eq!(atlas.on_empty, "x");
        assert_eq!(
            prompt.sections,
            Some(vec![Section {
                name: "s".to_owned(),
                body: "b".to_owned(),
                required: Some(true),
            }])
        );
        // Keys bind case-insensitively when there is no exact match.
        let prompt =
            Prompt::from_go_json(br#"{"ID":"x","NAME":"n","\u017ftatus":"s","Version":3}"#)
                .unwrap();
        assert_eq!(
            (
                prompt.id.as_str(),
                prompt.name.as_str(),
                prompt.status.as_str()
            ),
            ("x", "n", "s")
        );
        assert_eq!(prompt.version, 3);
        // null leaves scalars unchanged; a type mismatch is an error.
        let prompt = Prompt::from_go_json(br#"{"name":"n","name":null,"version":null}"#).unwrap();
        assert_eq!(prompt.name, "n");
        assert!(Prompt::from_go_json(br#"{"version":"1"}"#).is_err());
        assert!(Prompt::from_go_json(br#"{"version":1.5}"#).is_err());
        assert!(Prompt::from_go_json(br#"{"name":1}"#).is_err());
        assert!(Prompt::from_go_json(br#"{"frontmatter":{"a":1e999}}"#).is_err());
        assert!(Prompt::from_go_json(br#"[]"#).is_err());
        // A top-level null is a zero record, as in Go.
        assert_eq!(Prompt::from_go_json(b"null").unwrap(), Prompt::default());
        assert_eq!(Schema::from_go_json(b" null ").unwrap(), Schema::default());
    }

    #[test]
    fn nesting_is_limited_like_go() {
        let deep = |levels: usize| {
            let mut text = "[".repeat(levels);
            text.push_str(&"]".repeat(levels));
            text
        };
        assert!(parse(deep(MAX_DEPTH).as_bytes()).is_ok());
        assert_eq!(
            parse(deep(MAX_DEPTH + 1).as_bytes()),
            Err("exceeded max depth".to_owned())
        );
        let mut object = "{\"k\":".repeat(MAX_DEPTH);
        object.push('1');
        object.push_str(&"}".repeat(MAX_DEPTH));
        assert!(parse(object.as_bytes()).is_ok());
        object.insert(0, '[');
        object.push(']');
        assert!(parse(object.as_bytes()).is_err());
        // The deepest frontmatter Go accepts (the record object is one level)
        // reads and writes back without exhausting the stack.
        let levels = MAX_DEPTH - 2;
        let line = format!(
            "{{\"frontmatter\":{{\"deep\":{}{}}}}}",
            "[".repeat(levels),
            "]".repeat(levels)
        );
        let prompt = Prompt::from_go_json(line.as_bytes()).unwrap();
        assert!(prompt.to_go_json().contains(&line[1..line.len() - 1]));
        let too_deep = format!(
            "{{\"frontmatter\":{{\"deep\":{}{}}}}}",
            "[".repeat(levels + 1),
            "]".repeat(levels + 1)
        );
        assert!(Prompt::from_go_json(too_deep.as_bytes()).is_err());
    }
}
