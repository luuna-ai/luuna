//! An order-preserving JSON value with its own parser and a Go-compatible encoder.
//!
//! The Go tool remembers the key order of every object it parses and writes keys back in
//! that order, so the store's files keep their bytes across a read and a rewrite. This
//! module provides the same model without `serde_json` (plan s5-atlas-lib, "Dependencies").
//!
//! The Go tool's model (`internal/atlas/model.go`, `ObjectKeys` and `jsObjectKeyOrder`) is a
//! Go map plus an optional remembered key order. Its encoding order is: canonical
//! uint32-like keys ("0", "1", ... "4294967294") first in numeric order, then the
//! remembered keys in their remembered order, then every key that is not remembered,
//! sorted by bytes. [`Object`] keeps its entries physically in that order, so iteration,
//! [`Object::keys`] and both encoders all see the order the Go tool would write.
//!
//! The byte rules were checked against the Go tool's own code built with go1.27.0, whose
//! `encoding/json` runs on the v2 implementation by default:
//!
//! * strings escape `"`, `\`, `\b`, `\f`, `\n`, `\r`, `\t` with their short forms, other
//!   control characters as `\u00xx` (lowercase hex), and U+2028 and U+2029 as `\u2028`
//!   and `\u2029`; `<`, `>`, `&`, `/` and DEL are written as they are (HTML escaping off);
//! * numbers: NaN and infinities write `null`, zero (also `-0`) writes `0`, magnitudes in
//!   `[1e-6, 1e21)` use the shortest plain decimal, others the shortest exponent form with
//!   a sign and no leading zero in the exponent (`1e+21`, `1e-7`);
//! * the parser follows RFC 8259 strictly, as Go's decoder does: no trailing commas, no
//!   leading zeros, no comments, whitespace is space, tab, LF and CR only, at most 10000
//!   nested arrays and objects, and a number that overflows `f64` is an error. Duplicate
//!   keys keep the first key's position and the last value. In strings, an invalid UTF-8
//!   byte and an unpaired UTF-16 surrogate escape each become U+FFFD.

use crate::error::{AtlasError, Result};

/// The deepest nesting of arrays and objects the Go decoder accepts.
const MAX_DEPTH: usize = 10_000;

/// A JSON value. Numbers are `f64`, as in the Go tool, which decodes every JSON number
/// to `float64`; [`parse`] makes only [`Value::Number`].
///
/// [`Value::Integer`] holds an integer read from config.yaml. yaml.v3 decodes an untyped
/// integer to a Go `int` (64-bit) or, above that range, a `uint64`, so the Go tool keeps
/// every such integer exactly; holding it as `f64` would round it above 2^53.
///
/// Dropping and cloning a value, and both encoders, work with an explicit stack rather than
/// recursion, so a value nested as deep as the parser accepts ([`MAX_DEPTH`] levels) or
/// deeper is safe on a thread with the default 2 MiB stack. Because `Value` implements
/// [`Drop`], its contents cannot be moved out by a pattern; take them with
/// [`std::mem::take`] through a mutable reference instead.
#[derive(Debug, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Number(f64),
    /// An exact integer in the range of Go's `int64` or `uint64`.
    Integer(i128),
    String(String),
    /// A Go string containing bytes YAML binary scalars can hold but UTF-8 cannot.
    /// JSON exposes replacement characters; YAML preserves the original bytes.
    RawString(GoString),
    Array(Vec<Value>),
    Object(Object),
}

fn go_utf8_text(mut bytes: &[u8]) -> String {
    let mut text = String::new();
    loop {
        match std::str::from_utf8(bytes) {
            Ok(valid) => {
                text.push_str(valid);
                return text;
            }
            Err(error) => {
                let valid = error.valid_up_to();
                text.push_str(
                    std::str::from_utf8(&bytes[..valid]).expect("validated UTF-8 prefix"),
                );
                text.push('\u{fffd}');
                bytes = &bytes[valid + 1..];
            }
        }
    }
}

/// The exact bytes and the JSON-visible UTF-8 view of a Go string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoString {
    bytes: Vec<u8>,
    text: String,
}
impl GoString {
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
    pub fn text(&self) -> &str {
        &self.text
    }
}

/// A JSON object that keeps its keys in encoding order.
///
/// Two objects are equal when they hold the same entries in the same encoding order.
/// [`Object::insert`] of a new key places it last, as the Go tool does when it remembers
/// the order in which a record's fields were set; replacing an existing key keeps its
/// position.
///
/// As in the Go tool, keys that are canonical uint32 array indexes ("0", "17", but not
/// "01" or "4294967295") always come first, in numeric order, whatever method added them.
/// Keys added with [`Object::insert_unordered`] behave like keys the Go tool sets on a map
/// without remembering their order: they follow the remembered keys, sorted by bytes.
#[derive(Debug, Clone, Default)]
pub struct Object {
    /// Index-like keys (numeric order), then remembered keys, then unremembered keys
    /// (byte order). This is the encoding order.
    entries: Vec<(String, Value)>,
    /// `entries[unordered_from..]` are the unremembered keys, none of them index-like.
    unordered_from: usize,
    /// The Go tool's remembered order as it stood when a remembered key was first
    /// removed, kept only from then until the order is next replaced. Go's `delete` leaves
    /// a key in the remembered order, so setting the key again later puts it back in its
    /// old place; [`Object::insert_unordered`] uses this to do the same.
    removed_order: Option<Vec<String>>,
    // Opaque internal identities keep invalid-byte keys distinct even when JSON's
    // replacement characters give two different keys the same visible spelling.
    raw_keys: std::collections::BTreeMap<String, GoString>,
    // Go RecordFromValue keeps an array outside its otherwise empty map.
    // Only the store constructs this sidecar, never recursively inside itself.
    record_array: Option<Vec<Value>>,
}

impl PartialEq for Object {
    fn eq(&self, other: &Self) -> bool {
        // Only the encoding order and the values matter, not which keys are remembered.
        self.entries == other.entries
    }
}

/// The value of `key` if it is a canonical uint32 array index other than 2^32-1, the keys
/// JavaScript (and so the Go tool's `jsObjectKeyOrder`) enumerates first.
fn array_index(key: &str) -> Option<u32> {
    let bytes = key.as_bytes();
    if bytes.is_empty() || !bytes.iter().all(u8::is_ascii_digit) {
        return None;
    }
    if bytes.len() > 1 && bytes[0] == b'0' {
        return None;
    }
    match key.parse::<u32>() {
        Ok(u32::MAX) | Err(_) => None,
        Ok(index) => Some(index),
    }
}

impl Object {
    /// An empty object.
    pub fn new() -> Self {
        Object::default()
    }

    /// Original array record contents for iterative IPC numeric-sidecar traversal.
    pub(crate) fn record_array(&self) -> Option<&[Value]> {
        self.record_array.as_deref()
    }

    pub(crate) fn from_record_array(items: Vec<Value>) -> Self {
        Self {
            record_array: Some(items),
            ..Default::default()
        }
    }

    /// The original bytes of a key returned by iteration, including YAML binary keys.
    pub fn key_bytes<'a>(&'a self, key: &'a str) -> &'a [u8] {
        self.raw_keys
            .get(key)
            .map_or(key.as_bytes(), GoString::bytes)
    }
    /// The JSON-visible spelling of an iterated key.
    pub fn key_text<'a>(&'a self, key: &'a str) -> &'a str {
        self.raw_keys.get(key).map_or(key, GoString::text)
    }
    /// Lookup by exact Go string bytes, including binary YAML keys.
    pub fn get_go_key(&self, bytes: &[u8]) -> Option<&Value> {
        self.iter()
            .find(|(key, _)| self.key_bytes(key) == bytes)
            .map(|(_, value)| value)
    }

    /// Insert a YAML/Go key without replacing invalid bytes by Unicode characters.
    pub fn insert_go_key(&mut self, bytes: &[u8], value: Value) -> Option<Value> {
        if let Ok(key) = std::str::from_utf8(bytes) {
            return self.insert(key, value);
        }
        if let Some(key) = self
            .raw_keys
            .iter()
            .find(|(_, raw)| raw.bytes() == bytes)
            .map(|(key, _)| key.clone())
        {
            if let Some(slot) = self.get_mut(&key) {
                return Some(std::mem::replace(slot, value));
            }
            self.entries.push((key, value));
            self.unordered_from = self.entries.len();
            self.removed_order = None;
            return None;
        }
        let mut key = String::from("\0yaml-binary-key:");
        for byte in bytes {
            use std::fmt::Write;
            let _ = write!(key, "{byte:02x}");
        }
        while self.entries.iter().any(|(k, _)| k == &key) {
            key.push('\0');
        }
        self.entries.push((key.clone(), value));
        self.unordered_from = self.entries.len();
        self.removed_order = None;
        self.raw_keys.insert(
            key,
            GoString {
                bytes: bytes.to_vec(),
                text: go_utf8_text(bytes),
            },
        );
        None
    }

    /// Sets `key` to `value`. A new key goes last; an existing key keeps its position.
    /// Returns the previous value, if any.
    ///
    /// A new key makes the whole current order remembered, as the Go tool's
    /// `rememberOrder(m, append(ObjectKeys(m), key))` does. An index-like key goes to its
    /// numeric place among the index-like keys instead of last.
    pub fn insert(&mut self, key: impl Into<String>, value: impl Into<Value>) -> Option<Value> {
        let key = key.into();
        let value = value.into();
        self.displace_raw_identity(&key);
        if let Some(slot) = self.get_mut(&key) {
            return Some(std::mem::replace(slot, value));
        }
        if let Some(index) = array_index(&key) {
            self.insert_index_key(index, key, value);
        } else {
            self.entries.push((key, value));
            self.unordered_from = self.entries.len();
        }
        // Go replaced the remembered order with the current keys, so removed keys are gone
        // from it.
        self.removed_order = None;
        None
    }

    /// Sets `key` to `value` without remembering its order, as the Go tool does when it
    /// assigns a map entry that is not in the map's remembered order. A new key goes
    /// after the remembered keys, among the other unremembered keys in byte order (or to
    /// its numeric place if it is index-like); an existing key keeps its position. A key
    /// removed while remembered returns to its remembered place (see [`Object::remove`]).
    /// Returns the previous value, if any.
    pub fn insert_unordered(
        &mut self,
        key: impl Into<String>,
        value: impl Into<Value>,
    ) -> Option<Value> {
        let key = key.into();
        let value = value.into();
        self.displace_raw_identity(&key);
        if let Some(slot) = self.get_mut(&key) {
            return Some(std::mem::replace(slot, value));
        }
        if let Some(index) = array_index(&key) {
            self.insert_index_key(index, key, value);
        } else if let Some(rank) = self.removed_rank(&key) {
            // A removed key that Go still remembers returns to its remembered place: before
            // the first present remembered key that comes after it in that order.
            let order = self.removed_order.as_deref().unwrap_or_default();
            let position = (0..self.unordered_from)
                .find(|&i| {
                    let k = self.entries[i].0.as_str();
                    array_index(k).is_none() && order.iter().position(|o| o == k) > Some(rank)
                })
                .unwrap_or(self.unordered_from);
            self.entries.insert(position, (key, value));
            self.unordered_from += 1;
        } else {
            let tail = &self.entries[self.unordered_from..];
            let offset = tail.partition_point(|(k, _)| k.as_str() < key.as_str());
            self.entries
                .insert(self.unordered_from + offset, (key, value));
        }
        None
    }

    /// Replaces the remembered order, as the Go tool's `rememberOrder(m, keys)` does.
    ///
    /// Afterwards the present keys of `keys` come in that order (a repeated key counts at
    /// its first place, an absent key is ignored), followed by every other key sorted by
    /// bytes, which stays unremembered. Index-like keys still come first.
    pub fn remember_order<I, K>(&mut self, keys: I)
    where
        I: IntoIterator<Item = K>,
        K: AsRef<str>,
    {
        let mut rest = std::mem::take(&mut self.entries);
        let mut indexed: Vec<(u32, (String, Value))> = Vec::new();
        let mut remembered: Vec<(String, Value)> = Vec::new();
        for key in keys {
            let key = key.as_ref();
            if let Some(position) = rest.iter().position(|(k, _)| k == key) {
                let entry = rest.remove(position);
                match array_index(&entry.0) {
                    Some(index) => indexed.push((index, entry)),
                    None => remembered.push(entry),
                }
            }
        }
        let mut unremembered: Vec<(String, Value)> = Vec::new();
        for entry in rest {
            match array_index(&entry.0) {
                Some(index) => indexed.push((index, entry)),
                None => unremembered.push(entry),
            }
        }
        indexed.sort_by_key(|(index, _)| *index);
        unremembered.sort_by(|a, b| self.key_bytes(&a.0).cmp(self.key_bytes(&b.0)));
        self.entries = indexed.into_iter().map(|(_, entry)| entry).collect();
        self.entries.extend(remembered);
        self.unordered_from = self.entries.len();
        self.entries.extend(unremembered);
        self.removed_order = None;
    }

    fn displace_raw_identity(&mut self, key: &str) {
        let Some(raw) = self.raw_keys.remove(key) else {
            return;
        };
        let mut replacement = format!("{key}\0");
        while self.entries.iter().any(|(k, _)| k == &replacement) {
            replacement.push('\0');
        }
        if let Some((k, _)) = self.entries.iter_mut().find(|(k, _)| k == key) {
            k.clone_from(&replacement);
        }
        if let Some(order) = &mut self.removed_order {
            for k in order {
                if k == key {
                    k.clone_from(&replacement);
                }
            }
        }
        self.raw_keys.insert(replacement, raw);
    }

    /// The place of `key` in the remembered order kept since a remembered key was
    /// removed, if `key` is in it.
    fn removed_rank(&self, key: &str) -> Option<usize> {
        self.removed_order
            .as_ref()
            .and_then(|order| order.iter().position(|k| k == key))
    }

    /// Inserts a new index-like key at its numeric place among the leading index keys.
    fn insert_index_key(&mut self, index: u32, key: String, value: Value) {
        let position = self
            .entries
            .iter()
            .position(|(k, _)| array_index(k).is_none_or(|other| other > index))
            .unwrap_or(self.entries.len());
        self.entries.insert(position, (key, value));
        self.unordered_from += 1;
    }

    /// The value stored under `key`.
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.entries.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    /// The value stored under `key`, mutably.
    pub fn get_mut(&mut self, key: &str) -> Option<&mut Value> {
        self.entries
            .iter_mut()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v)
    }

    /// Removes `key`, keeping the order of the remaining keys.
    ///
    /// As in the Go tool, a removed remembered key stays in the remembered order: setting
    /// it again with [`Object::insert_unordered`] puts it back in its old place, until
    /// [`Object::insert`] of a new key or [`Object::remember_order`] replaces the order.
    pub fn remove(&mut self, key: &str) -> Option<Value> {
        let index = self.entries.iter().position(|(k, _)| k == key)?;
        if index < self.unordered_from {
            if self.removed_order.is_none() {
                self.removed_order = Some(
                    self.entries[..self.unordered_from]
                        .iter()
                        .map(|(k, _)| k.clone())
                        .collect(),
                );
            }
            self.unordered_from -= 1;
        }
        Some(self.entries.remove(index).1)
    }

    /// Whether `key` is present.
    pub fn contains_key(&self, key: &str) -> bool {
        self.get(key).is_some()
    }

    /// The keys in encoding order.
    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.entries.iter().map(|(k, _)| k.as_str())
    }

    /// The entries in encoding order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &Value)> {
        self.entries.iter().map(|(k, v)| (k.as_str(), v))
    }

    /// The number of keys.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the object has no keys.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

impl Value {
    /// A Go string, preserving invalid UTF-8 bytes from a YAML binary scalar.
    pub fn from_go_bytes(bytes: Vec<u8>) -> Self {
        match String::from_utf8(bytes) {
            Ok(text) => Self::String(text),
            Err(error) => {
                let bytes = error.into_bytes();
                let text = go_utf8_text(&bytes);
                Self::RawString(GoString { bytes, text })
            }
        }
    }
    /// Exact bytes when this is either kind of string.
    pub fn string_bytes(&self) -> Option<&[u8]> {
        match self {
            Self::String(s) => Some(s.as_bytes()),
            Self::RawString(s) => Some(s.bytes()),
            _ => None,
        }
    }
    /// The string, if this is a string.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::String(s) => Some(s),
            Value::RawString(s) => Some(s.text()),
            _ => None,
        }
    }

    /// The number, if this is a number. An [`Value::Integer`] is rounded to the nearest
    /// `f64`, as Go's `float64(n)` conversion of an `int` rounds it.
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Number(n) => Some(*n),
            Value::Integer(i) => Some(*i as f64),
            _ => None,
        }
    }

    /// The boolean, if this is a boolean.
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }

    /// The elements, if this is an array.
    pub fn as_array(&self) -> Option<&Vec<Value>> {
        match self {
            Value::Array(a) => Some(a),
            _ => None,
        }
    }

    /// The object, if this is an object.
    pub fn as_object(&self) -> Option<&Object> {
        match self {
            Value::Object(o) => Some(o),
            _ => None,
        }
    }

    /// The object, mutably, if this is an object.
    pub fn as_object_mut(&mut self) -> Option<&mut Object> {
        match self {
            Value::Object(o) => Some(o),
            _ => None,
        }
    }

    /// Whether this is `null`.
    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }
}

impl Value {
    /// Whether this is an array or object that holds an array or object.
    fn holds_containers(&self) -> bool {
        let nested = |v: &Value| matches!(v, Value::Array(_) | Value::Object(_));
        match self {
            Value::Array(items) => items.iter().any(nested),
            Value::Object(object) => object.entries.iter().any(|(_, v)| nested(v)),
            _ => false,
        }
    }

    /// A copy of a value that holds no arrays or objects.
    fn clone_flat(&self) -> Value {
        match self {
            Value::Null => Value::Null,
            Value::Bool(b) => Value::Bool(*b),
            Value::Number(n) => Value::Number(*n),
            Value::Integer(i) => Value::Integer(*i),
            Value::String(s) => Value::String(s.clone()),
            Value::RawString(s) => Value::RawString(s.clone()),
            // Only scalars inside: cloning the elements does not recurse further.
            Value::Array(items) => Value::Array(items.iter().map(Value::clone_flat).collect()),
            Value::Object(object) => Value::Object(Object {
                entries: object
                    .entries
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone_flat()))
                    .collect(),
                unordered_from: object.unordered_from,
                removed_order: object.removed_order.clone(),
                raw_keys: object.raw_keys.clone(),
                record_array: object.record_array.clone(),
            }),
        }
    }
}

/// An array or object being copied by [`Value::clone`]: the source's remaining elements
/// and the copies made so far.
enum CloneFrame<'a> {
    Array(std::slice::Iter<'a, Value>, Vec<Value>),
    Object(
        &'a Object,
        std::slice::Iter<'a, (String, Value)>,
        Vec<(String, Value)>,
        String,
    ),
}

impl Clone for Value {
    /// A deep copy, made with an explicit stack so that nesting depth does not use the
    /// thread's stack.
    fn clone(&self) -> Self {
        if !self.holds_containers() {
            return self.clone_flat();
        }
        let mut stack: Vec<CloneFrame<'_>> = Vec::new();
        let mut next = self;
        loop {
            // Open `next`, or copy it whole if it holds no containers.
            let mut done = if next.holds_containers() {
                match next {
                    Value::Array(items) => {
                        stack.push(CloneFrame::Array(
                            items.iter(),
                            Vec::with_capacity(items.len()),
                        ));
                    }
                    Value::Object(object) => stack.push(CloneFrame::Object(
                        object,
                        object.entries.iter(),
                        Vec::with_capacity(object.entries.len()),
                        String::new(),
                    )),
                    _ => unreachable!("only arrays and objects hold containers"),
                }
                None
            } else {
                Some(next.clone_flat())
            };
            // Hand the finished copy to its parent and find the next element to copy,
            // closing every container that has no elements left.
            loop {
                let Some(frame) = stack.last_mut() else {
                    return done.expect("the root copy is finished");
                };
                let child = match frame {
                    CloneFrame::Array(source, copies) => {
                        if let Some(value) = done.take() {
                            copies.push(value);
                        }
                        source.next()
                    }
                    CloneFrame::Object(_, source, copies, key) => {
                        if let Some(value) = done.take() {
                            copies.push((std::mem::take(key), value));
                        }
                        source.next().map(|(k, v)| {
                            key.clone_from(k);
                            v
                        })
                    }
                };
                if let Some(child) = child {
                    next = child;
                    break;
                }
                done = Some(match stack.pop() {
                    Some(CloneFrame::Array(_, copies)) => Value::Array(copies),
                    Some(CloneFrame::Object(object, _, copies, _)) => Value::Object(Object {
                        entries: copies,
                        unordered_from: object.unordered_from,
                        removed_order: object.removed_order.clone(),
                        raw_keys: object.raw_keys.clone(),
                        record_array: object.record_array.clone(),
                    }),
                    None => unreachable!("the frame was just inspected"),
                });
            }
        }
    }
}

impl Drop for Value {
    /// Frees nested arrays and objects with an explicit stack instead of the recursion a
    /// derived drop would use.
    fn drop(&mut self) {
        if !self.holds_containers() {
            return;
        }
        let mut stack: Vec<Value> = Vec::new();
        take_children(self, &mut stack);
        while let Some(mut value) = stack.pop() {
            take_children(&mut value, &mut stack);
            // `value` holds no arrays or objects now, so dropping it does not recurse.
        }
    }
}

/// Moves the elements of an array or object onto `stack`.
fn take_children(value: &mut Value, stack: &mut Vec<Value>) {
    match value {
        Value::Array(items) => stack.append(items),
        Value::Object(object) => {
            stack.extend(object.entries.drain(..).map(|(_, v)| v));
            object.unordered_from = 0;
        }
        _ => {}
    }
}

impl From<bool> for Value {
    fn from(value: bool) -> Self {
        Value::Bool(value)
    }
}

impl From<f64> for Value {
    fn from(value: f64) -> Self {
        Value::Number(value)
    }
}

impl From<i64> for Value {
    fn from(value: i64) -> Self {
        Value::Integer(i128::from(value))
    }
}

impl From<&str> for Value {
    fn from(value: &str) -> Self {
        Value::String(value.to_owned())
    }
}

impl From<String> for Value {
    fn from(value: String) -> Self {
        Value::String(value)
    }
}

impl From<Vec<Value>> for Value {
    fn from(value: Vec<Value>) -> Self {
        Value::Array(value)
    }
}

impl From<Object> for Value {
    fn from(value: Object) -> Self {
        Value::Object(value)
    }
}

// ---------------------------------------------------------------------------------------
// Parser

/// Parses one JSON text, keeping the key order of every object.
pub fn parse(text: &str) -> Result<Value> {
    parse_bytes(text.as_bytes())
}

/// Parses one JSON text given as bytes, as the Go tool parses a line it read from a file:
/// an invalid UTF-8 byte inside a string becomes U+FFFD (one per byte); outside a string
/// it is a syntax error.
pub fn parse_bytes(input: &[u8]) -> Result<Value> {
    Parser { input, pos: 0 }.parse_document()
}

/// An array or object whose elements are still being read.
enum Frame {
    Array(Vec<Value>),
    /// The object so far and the key of the member whose value is being read.
    Object(Object, String),
}

struct Parser<'a> {
    input: &'a [u8],
    pos: usize,
}

fn syntax_error(message: impl Into<String>) -> AtlasError {
    AtlasError::Message(message.into())
}

fn describe(byte: u8) -> String {
    if byte.is_ascii_graphic() || byte == b' ' {
        format!("'{}'", byte as char)
    } else {
        format!("'\\x{byte:02x}'")
    }
}

impl Parser<'_> {
    fn peek(&self) -> Option<u8> {
        self.input.get(self.pos).copied()
    }

    fn skip_whitespace(&mut self) {
        while let Some(b' ' | b'\t' | b'\n' | b'\r') = self.peek() {
            self.pos += 1;
        }
    }

    fn unexpected(&self, context: &str) -> AtlasError {
        match self.peek() {
            None => syntax_error("unexpected end of JSON input"),
            Some(byte) => syntax_error(format!(
                "invalid character {} {context} (offset {})",
                describe(byte),
                self.pos
            )),
        }
    }

    /// The whole document: one value, then only whitespace. Iterative, so deep nesting
    /// up to [`MAX_DEPTH`] cannot exhaust the stack.
    fn parse_document(&mut self) -> Result<Value> {
        let mut stack: Vec<Frame> = Vec::new();
        'value: loop {
            self.skip_whitespace();
            let mut value = match self.peek() {
                None => return Err(syntax_error("unexpected end of JSON input")),
                Some(open @ (b'{' | b'[')) => {
                    if stack.len() >= MAX_DEPTH {
                        return Err(syntax_error("exceeded max depth"));
                    }
                    self.pos += 1;
                    self.skip_whitespace();
                    if open == b'[' {
                        if self.peek() == Some(b']') {
                            self.pos += 1;
                            Value::Array(Vec::new())
                        } else {
                            stack.push(Frame::Array(Vec::new()));
                            continue 'value;
                        }
                    } else if self.peek() == Some(b'}') {
                        self.pos += 1;
                        Value::Object(Object::new())
                    } else {
                        let key = self.parse_member_name()?;
                        stack.push(Frame::Object(Object::new(), key));
                        continue 'value;
                    }
                }
                Some(b'"') => Value::String(self.parse_string()?),
                Some(b't') => self.parse_literal(b"true", Value::Bool(true))?,
                Some(b'f') => self.parse_literal(b"false", Value::Bool(false))?,
                Some(b'n') => self.parse_literal(b"null", Value::Null)?,
                Some(b'-' | b'0'..=b'9') => Value::Number(self.parse_number()?),
                Some(_) => return Err(self.unexpected("looking for beginning of value")),
            };
            // A value is complete: hand it to the enclosing array or object, closing
            // every container that ends here.
            loop {
                self.skip_whitespace();
                match stack.last_mut() {
                    None => {
                        if self.pos != self.input.len() {
                            return Err(syntax_error(format!(
                                "invalid character {} after top-level value (offset {})",
                                describe(self.input[self.pos]),
                                self.pos
                            )));
                        }
                        return Ok(value);
                    }
                    Some(Frame::Array(items)) => {
                        items.push(value);
                        match self.peek() {
                            Some(b',') => {
                                self.pos += 1;
                                continue 'value;
                            }
                            Some(b']') => {
                                self.pos += 1;
                                let Some(Frame::Array(items)) = stack.pop() else {
                                    unreachable!("the top frame is an array");
                                };
                                value = Value::Array(items);
                            }
                            _ => return Err(self.unexpected("after array element")),
                        }
                    }
                    Some(Frame::Object(object, key)) => {
                        object.insert(std::mem::take(key), value);
                        match self.peek() {
                            Some(b',') => {
                                self.pos += 1;
                                self.skip_whitespace();
                                *key = self.parse_member_name()?;
                                continue 'value;
                            }
                            Some(b'}') => {
                                self.pos += 1;
                                let Some(Frame::Object(object, _)) = stack.pop() else {
                                    unreachable!("the top frame is an object");
                                };
                                value = Value::Object(object);
                            }
                            _ => return Err(self.unexpected("after object key:value pair")),
                        }
                    }
                }
            }
        }
    }

    /// A member name and its colon; the caller has skipped whitespace before it.
    fn parse_member_name(&mut self) -> Result<String> {
        if self.peek() != Some(b'"') {
            return Err(self.unexpected("looking for beginning of object key string"));
        }
        let key = self.parse_string()?;
        self.skip_whitespace();
        if self.peek() != Some(b':') {
            return Err(self.unexpected("after object key"));
        }
        self.pos += 1;
        Ok(key)
    }

    fn parse_literal(&mut self, word: &[u8], value: Value) -> Result<Value> {
        for &expected in word {
            match self.peek() {
                Some(byte) if byte == expected => self.pos += 1,
                _ => return Err(self.unexpected("in literal")),
            }
        }
        Ok(value)
    }

    fn take_digits(&mut self) -> usize {
        let start = self.pos;
        while let Some(b'0'..=b'9') = self.peek() {
            self.pos += 1;
        }
        self.pos - start
    }

    /// `-?(0|[1-9][0-9]*)(\.[0-9]+)?([eE][+-]?[0-9]+)?`, converted with `str::parse`.
    fn parse_number(&mut self) -> Result<f64> {
        let start = self.pos;
        if self.peek() == Some(b'-') {
            self.pos += 1;
        }
        match self.peek() {
            Some(b'0') => self.pos += 1,
            Some(b'1'..=b'9') => {
                self.take_digits();
            }
            _ => return Err(self.unexpected("in numeric literal")),
        }
        if self.peek() == Some(b'.') {
            self.pos += 1;
            if self.take_digits() == 0 {
                return Err(self.unexpected("after decimal point in numeric literal"));
            }
        }
        if let Some(b'e' | b'E') = self.peek() {
            self.pos += 1;
            if let Some(b'+' | b'-') = self.peek() {
                self.pos += 1;
            }
            if self.take_digits() == 0 {
                return Err(self.unexpected("in exponent of numeric literal"));
            }
        }
        // The slice holds only ASCII sign, digit, point and exponent bytes.
        let text = std::str::from_utf8(&self.input[start..self.pos])
            .map_err(|_| syntax_error("invalid number"))?;
        let number: f64 = text
            .parse()
            .map_err(|_| syntax_error(format!("invalid number {text}")))?;
        if number.is_infinite() {
            // Go's strconv.ParseFloat reports a range error, which the decoder returns.
            return Err(syntax_error(format!(
                "json: cannot unmarshal number {text} into Go value of type float64"
            )));
        }
        Ok(number)
    }

    fn parse_hex4(&mut self) -> Result<u16> {
        let mut out: u16 = 0;
        for _ in 0..4 {
            let digit = match self.peek() {
                Some(b @ b'0'..=b'9') => b - b'0',
                Some(b @ b'a'..=b'f') => b - b'a' + 10,
                Some(b @ b'A'..=b'F') => b - b'A' + 10,
                _ => return Err(self.unexpected("in \\u hexadecimal character escape")),
            };
            out = out * 16 + u16::from(digit);
            self.pos += 1;
        }
        Ok(out)
    }

    /// A string starting at its opening quote.
    fn parse_string(&mut self) -> Result<String> {
        debug_assert_eq!(self.peek(), Some(b'"'));
        self.pos += 1;
        let mut out = String::new();
        loop {
            // Copy the longest run of plain bytes at once.
            let start = self.pos;
            while let Some(byte) = self.peek() {
                if byte == b'"' || byte == b'\\' || byte < 0x20 {
                    break;
                }
                self.pos += 1;
            }
            push_lossy_per_byte(&mut out, &self.input[start..self.pos]);
            match self.peek() {
                None => return Err(syntax_error("unexpected end of JSON input")),
                Some(b'"') => {
                    self.pos += 1;
                    return Ok(out);
                }
                Some(b'\\') => {
                    self.pos += 1;
                    let escaped = match self.peek() {
                        Some(b'"') => '"',
                        Some(b'\\') => '\\',
                        Some(b'/') => '/',
                        Some(b'b') => '\u{8}',
                        Some(b'f') => '\u{c}',
                        Some(b'n') => '\n',
                        Some(b'r') => '\r',
                        Some(b't') => '\t',
                        Some(b'u') => {
                            self.pos += 1;
                            let unit = self.parse_hex4()?;
                            out.push(self.finish_unicode_escape(unit)?);
                            continue;
                        }
                        None => return Err(syntax_error("unexpected end of JSON input")),
                        Some(_) => return Err(self.unexpected("in string escape code")),
                    };
                    self.pos += 1;
                    out.push(escaped);
                }
                Some(_) => return Err(self.unexpected("in string literal")),
            }
        }
    }

    /// The character for `\uXXXX` whose code unit `unit` was just read. A high surrogate
    /// followed by a `\u` low surrogate combines with it; any other surrogate becomes
    /// U+FFFD and the following escape, if any, is read on its own (as in Go).
    fn finish_unicode_escape(&mut self, unit: u16) -> Result<char> {
        if let Some(c) = char::from_u32(u32::from(unit)) {
            return Ok(c);
        }
        if (0xD800..0xDC00).contains(&unit)
            && self.input.get(self.pos..self.pos + 2) == Some(b"\\u")
        {
            let saved = self.pos;
            self.pos += 2;
            if let Ok(low) = self.parse_hex4() {
                if (0xDC00..0xE000).contains(&low) {
                    let combined =
                        0x10000 + ((u32::from(unit) - 0xD800) << 10) + (u32::from(low) - 0xDC00);
                    if let Some(c) = char::from_u32(combined) {
                        return Ok(c);
                    }
                }
            }
            self.pos = saved;
        }
        Ok(char::REPLACEMENT_CHARACTER)
    }
}

/// Appends `bytes`, replacing each byte that is not part of a valid UTF-8 sequence with
/// U+FFFD, one per byte, as Go's `utf8.DecodeRune` loop does.
fn push_lossy_per_byte(out: &mut String, mut bytes: &[u8]) {
    while !bytes.is_empty() {
        match std::str::from_utf8(bytes) {
            Ok(valid) => {
                out.push_str(valid);
                return;
            }
            Err(error) => {
                let (valid, rest) = bytes.split_at(error.valid_up_to());
                // `valid` is valid UTF-8 by construction.
                out.push_str(std::str::from_utf8(valid).unwrap_or_default());
                out.push(char::REPLACEMENT_CHARACTER);
                bytes = &rest[1..];
            }
        }
    }
}

// ---------------------------------------------------------------------------------------
// Encoder

/// Encodes `value` the way the Go tool writes a JSONL line: one line, no spaces, no HTML
/// escaping. The result has no trailing newline.
pub fn to_compact_string(value: &Value) -> Result<String> {
    let mut out = String::new();
    write_value(&mut out, value, None)?;
    Ok(out)
}

/// Encodes `value` the way the Go tool prints a command envelope: two-space indentation,
/// no HTML escaping. The result has no trailing newline.
///
/// This is what `json.Indent(dst, compact, "", "  ")` makes of the compact form: an empty
/// array or object stays `[]` or `{}`, and a colon is followed by one space.
///
/// Like `json.Indent`, it fails for a value nested more than [`MAX_DEPTH`] levels deep;
/// the Go tool's `JSONPretty` then prints an empty string, as
/// `CommandOutput`'s printing does with this error. The compact form has no such limit.
pub fn to_indented_string(value: &Value) -> Result<String> {
    let mut out = String::new();
    write_value(&mut out, value, Some(0))?;
    Ok(out)
}

fn newline(out: &mut String, depth: usize) {
    out.push('\n');
    for _ in 0..depth {
        out.push_str("  ");
    }
}

/// An array or object being written by [`write_value`]: its remaining elements, whether
/// none has been written yet, and its own depth for the indented form.
enum EncodeFrame<'a> {
    Array(std::slice::Iter<'a, Value>, bool, Option<usize>),
    Object(
        &'a Object,
        std::slice::Iter<'a, (String, Value)>,
        bool,
        Option<usize>,
    ),
}

/// Writes `value`; `indent` is the current depth for the indented form, `None` for the
/// compact form. Nested arrays and objects are walked with an explicit stack, so the depth
/// of `value` does not use the thread's stack. The indented form fails past
/// [`MAX_DEPTH`] levels, as `json.Indent` does.
fn write_value(out: &mut String, value: &Value, indent: Option<usize>) -> Result<()> {
    let mut stack: Vec<EncodeFrame<'_>> = Vec::new();
    let mut next = Some((value, indent));
    loop {
        if let Some((value, indent)) = next.take() {
            if indent.is_some()
                && stack.len() == MAX_DEPTH
                && matches!(value, Value::Array(_) | Value::Object(_))
            {
                return Err(syntax_error("exceeded max depth"));
            }
            match value {
                Value::Null => out.push_str("null"),
                Value::Bool(true) => out.push_str("true"),
                Value::Bool(false) => out.push_str("false"),
                Value::Number(n) => write_number(out, *n),
                // encoding/json writes a Go int or uint64 as its decimal digits.
                Value::Integer(i) => out.push_str(&i.to_string()),
                Value::String(s) => write_string(out, s),
                Value::RawString(s) => write_string(out, s.text()),
                Value::Array(items) => {
                    out.push('[');
                    stack.push(EncodeFrame::Array(items.iter(), true, indent));
                }
                Value::Object(object) => {
                    if let Some(items) = &object.record_array {
                        out.push('[');
                        stack.push(EncodeFrame::Array(items.iter(), true, indent));
                    } else {
                        out.push('{');
                        stack.push(EncodeFrame::Object(
                            object,
                            object.entries.iter(),
                            true,
                            indent,
                        ));
                    }
                }
            }
        }
        let Some(frame) = stack.last_mut() else {
            return Ok(());
        };
        match frame {
            EncodeFrame::Array(items, first, indent) => {
                let inner = indent.map(|d| d + 1);
                if let Some(item) = items.next() {
                    if !*first {
                        out.push(',');
                    }
                    *first = false;
                    if let Some(depth) = inner {
                        newline(out, depth);
                    }
                    next = Some((item, inner));
                } else {
                    if let (Some(depth), false) = (*indent, *first) {
                        newline(out, depth);
                    }
                    out.push(']');
                    stack.pop();
                }
            }
            EncodeFrame::Object(object, entries, first, indent) => {
                let inner = indent.map(|d| d + 1);
                if let Some((key, item)) = entries.next() {
                    if !*first {
                        out.push(',');
                    }
                    *first = false;
                    if let Some(depth) = inner {
                        newline(out, depth);
                    }
                    write_string(out, object.key_text(key));
                    out.push(':');
                    if inner.is_some() {
                        out.push(' ');
                    }
                    next = Some((item, inner));
                } else {
                    if let (Some(depth), false) = (*indent, *first) {
                        newline(out, depth);
                    }
                    out.push('}');
                    stack.pop();
                }
            }
        }
    }
}

/// Go's float64 encoding as the Go tool applies it (`marshalOrdered` in model.go, then
/// `encoding/json`'s float encoder).
fn write_number(out: &mut String, n: f64) {
    if !n.is_finite() {
        out.push_str("null");
        return;
    }
    if n == 0.0 {
        out.push('0');
        return;
    }
    let magnitude = n.abs();
    if (1e-6..1e21).contains(&magnitude) {
        // Rust's Display is the shortest round-tripping plain decimal, as Go's 'f', -1.
        out.push_str(&n.to_string());
        return;
    }
    // Shortest round-tripping exponent form; Go writes a sign on the exponent and, after
    // its cleanup, no leading zero ("1e+21", "1e-7").
    let text = format!("{n:e}");
    match text.split_once('e') {
        Some((mantissa, exponent)) if !exponent.starts_with('-') => {
            out.push_str(mantissa);
            out.push_str("e+");
            out.push_str(exponent);
        }
        _ => out.push_str(&text),
    }
}

/// Go's string encoding with HTML escaping off.
fn write_string(out: &mut String, s: &str) {
    out.push('"');
    let mut plain_from = 0;
    for (i, c) in s.char_indices() {
        let escape: Option<&str> = match c {
            '"' => Some("\\\""),
            '\\' => Some("\\\\"),
            '\u{8}' => Some("\\b"),
            '\u{c}' => Some("\\f"),
            '\n' => Some("\\n"),
            '\r' => Some("\\r"),
            '\t' => Some("\\t"),
            '\u{2028}' => Some("\\u2028"),
            '\u{2029}' => Some("\\u2029"),
            c if (c as u32) < 0x20 => None,
            _ => continue,
        };
        out.push_str(&s[plain_from..i]);
        match escape {
            Some(text) => out.push_str(text),
            None => out.push_str(&format!("\\u{:04x}", c as u32)),
        }
        plain_from = i + c.len_utf8();
    }
    out.push_str(&s[plain_from..]);
    out.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;

    fn compact(value: &Value) -> String {
        to_compact_string(value).expect("compact encoding")
    }

    fn indented(value: &Value) -> String {
        to_indented_string(value).expect("indented encoding")
    }

    fn round_trip(text: &str) -> String {
        compact(&parse(text).expect("valid JSON"))
    }

    #[test]
    fn parses_preserving_key_order() {
        let value = parse(r#"{"type":"guide","name":"A","classification":"tactical","b":1,"a":2}"#)
            .unwrap();
        let object = value.as_object().unwrap();
        assert_eq!(
            object.keys().collect::<Vec<_>>(),
            ["type", "name", "classification", "b", "a"]
        );
        // Nested objects keep their order too, and a rewrite reproduces the bytes.
        let text = r#"{"z":{"y":1,"x":[{"q":true,"p":null}]},"a":"s"}"#;
        assert_eq!(round_trip(text), text);
        // A repeated key keeps its first place and takes the last value, as in Go.
        assert_eq!(round_trip(r#"{"a":1,"b":2,"a":3}"#), r#"{"a":3,"b":2}"#);
        // A new key goes last; replacing a key keeps its place.
        let mut object = object.clone();
        object.insert("added", "x");
        object.insert("type", "pattern");
        assert_eq!(
            compact(&Value::Object(object.clone())),
            r#"{"type":"pattern","name":"A","classification":"tactical","b":1,"a":2,"added":"x"}"#
        );
        object.remove("name");
        assert_eq!(
            object.keys().collect::<Vec<_>>(),
            ["type", "classification", "b", "a", "added"]
        );
    }

    #[test]
    fn compact_encoding_is_one_line_without_spaces() {
        let text = r#"{"a":[1,2,{"b":"c d"}],"e":{},"f":[],"g":null,"h":true,"i":false}"#;
        let value = parse(&format!(" \t\r\n{text}\n")).unwrap();
        let out = compact(&value);
        assert_eq!(out, text);
        assert!(!out.contains('\n'));
        // A newline inside a string is escaped, so the line stays one line.
        assert_eq!(
            compact(&Value::String("two\nlines".into())),
            r#""two\nlines""#
        );
    }

    #[test]
    fn indented_encoding_uses_two_spaces() {
        let value = parse(r#"{"success":true,"command":"show","records":[{"domain":"d","record":{"a":[1,"x"],"e":{},"f":[]}}]}"#).unwrap();
        let expected = "{\n  \"success\": true,\n  \"command\": \"show\",\n  \"records\": [\n    {\n      \"domain\": \"d\",\n      \"record\": {\n        \"a\": [\n          1,\n          \"x\"\n        ],\n        \"e\": {},\n        \"f\": []\n      }\n    }\n  ]\n}";
        assert_eq!(indented(&value), expected);
        assert_eq!(indented(&Value::Object(Object::new())), "{}");
        assert_eq!(indented(&Value::Array(Vec::new())), "[]");
        assert_eq!(indented(&Value::String("a: b".into())), "\"a: b\"");
    }

    #[test]
    fn does_not_escape_html() {
        let value = Value::String("<a href=\"x\">&amp;</a> / \u{7f}".into());
        assert_eq!(compact(&value), "\"<a href=\\\"x\\\">&amp;</a> / \u{7f}\"");
        // What Go does escape: quotes, backslashes, control characters, U+2028, U+2029.
        let value = Value::String("\"\\\u{8}\u{c}\n\r\t\u{0}\u{1f}\u{2028}\u{2029}é😀".into());
        assert_eq!(
            compact(&value),
            "\"\\\"\\\\\\b\\f\\n\\r\\t\\u0000\\u001f\\u2028\\u2029é😀\""
        );
        // Keys follow the same rules.
        let mut object = Object::new();
        object.insert("<k>&", "v");
        assert_eq!(compact(&Value::Object(object)), r#"{"<k>&":"v"}"#);
    }

    #[test]
    fn number_formatting_matches_go() {
        // Expected strings were produced by the Go tool's MarshalJSON (go1.27.0).
        let cases: &[(f64, &str)] = &[
            (1.0, "1"),
            (-0.0, "0"),
            (0.1, "0.1"),
            (1.5, "1.5"),
            (100.0, "100"),
            (-123.456, "-123.456"),
            (1e20, "100000000000000000000"),
            (1e21, "1e+21"),
            (-1e21, "-1e+21"),
            (0.000001, "0.000001"),
            (9.99e-7, "9.99e-7"),
            (1e-7, "1e-7"),
            (-1.5e-10, "-1.5e-10"),
            (1e100, "1e+100"),
            (1.2345678901234569e23, "1.2345678901234569e+23"),
            (12345678901234567890.0, "12345678901234567000"),
            (f64::MAX, "1.7976931348623157e+308"),
            (5e-324, "5e-324"),
        ];
        for (number, expected) in cases {
            assert_eq!(compact(&Value::Number(*number)), *expected, "{number:e}");
        }
        assert_eq!(
            round_trip("[1.0,-0,0.1e1,1E5,-0.0e+0,2.5e-324]"),
            "[1,0,1,100000,0,5e-324]"
        );
        // Numbers parse with str::parse, so they round-trip exactly.
        assert_eq!(
            parse("0.30000000000000004").unwrap(),
            Value::Number(0.1 + 0.2)
        );
        // Underflow reads as zero; overflow is an error, as in Go.
        assert_eq!(round_trip("1e-400"), "0");
        assert!(parse("1e400").is_err());
        assert!(parse("-1.7976931348623157e309").is_err());
    }

    #[test]
    fn non_finite_numbers_encode_as_null() {
        let value = Value::Array(vec![
            Value::Number(f64::NAN),
            Value::Number(f64::INFINITY),
            Value::Number(f64::NEG_INFINITY),
        ]);
        assert_eq!(compact(&value), "[null,null,null]");
        assert_eq!(indented(&value), "[\n  null,\n  null,\n  null\n]");
    }

    #[test]
    fn exact_integers_encode_as_decimal_digits() {
        // Integers from config.yaml (Go int or uint64) are written exactly, as
        // encoding/json writes them; JSON input still parses to f64, as Go's decoder does.
        let value = Value::Array(vec![
            Value::Integer(9_007_199_254_740_993),
            Value::Integer(i128::from(i64::MAX)),
            Value::Integer(i128::from(i64::MIN)),
            Value::Integer(i128::from(u64::MAX)),
            Value::from(0_i64),
        ]);
        assert_eq!(
            compact(&value),
            "[9007199254740993,9223372036854775807,-9223372036854775808,18446744073709551615,0]"
        );
        assert_eq!(
            indented(&Value::Array(vec![Value::Integer(-1)])),
            "[\n  -1\n]"
        );
        assert_eq!(
            parse("9007199254740993").unwrap(),
            Value::Number(9_007_199_254_740_992.0)
        );
        assert_eq!(round_trip("[9223372036854775807]"), "[9223372036854776000]");
        assert_eq!(value.clone(), value);
    }

    #[test]
    fn index_like_keys_sort_first_for_unordered_keys() {
        // A map the Go tool builds without remembering an order: index keys in numeric
        // order, then the rest sorted by bytes.
        let mut object = Object::new();
        for key in [
            "b",
            "10",
            "a",
            "2",
            "B",
            "01",
            "4294967295",
            "4294967294",
            "_",
        ] {
            object.insert_unordered(key, 1.0);
        }
        assert_eq!(
            object.keys().collect::<Vec<_>>(),
            [
                "2",
                "10",
                "4294967294",
                "01",
                "4294967295",
                "B",
                "_",
                "a",
                "b"
            ]
        );
        // Keys not in the remembered order follow the remembered keys, sorted.
        let mut record = parse(r#"{"z":1,"y":2}"#)
            .unwrap()
            .as_object()
            .unwrap()
            .clone();
        record.insert_unordered("status", "archived");
        record.insert_unordered("archived_at", "t");
        record.insert_unordered("archive_reason", "r");
        record.insert_unordered("7", 0.0);
        assert_eq!(
            compact(&Value::Object(record.clone())),
            r#"{"7":0,"z":1,"y":2,"archive_reason":"r","archived_at":"t","status":"archived"}"#
        );
        // Inserting a remembered key afterwards keeps the current order and goes last.
        record.insert("w", 3.0);
        record.insert_unordered("c", 4.0);
        assert_eq!(
            record.keys().collect::<Vec<_>>(),
            [
                "7",
                "z",
                "y",
                "archive_reason",
                "archived_at",
                "status",
                "w",
                "c"
            ]
        );
        // Parsed objects put index keys first as well, whatever their place in the text.
        assert_eq!(
            round_trip(r#"{"z":1,"10":2,"a":{"1":1,"0":0},"0":3}"#),
            r#"{"0":3,"10":2,"z":1,"a":{"0":0,"1":1}}"#
        );
        // remember_order: listed keys first (absent and repeated ones ignored), the rest
        // sorted and still unremembered.
        let mut object = Object::new();
        for key in ["d", "c", "b", "a", "3"] {
            object.insert(key, 0.0);
        }
        object.remember_order(["c", "missing", "a", "c"]);
        assert_eq!(object.keys().collect::<Vec<_>>(), ["3", "c", "a", "b", "d"]);
        object.insert_unordered("bb", 0.0);
        assert_eq!(
            object.keys().collect::<Vec<_>>(),
            ["3", "c", "a", "b", "bb", "d"]
        );
        // Equality looks at the encoding order, not at what is remembered.
        let mut ordered = Object::new();
        for key in ["3", "c", "a", "b", "bb", "d"] {
            ordered.insert(key, 0.0);
        }
        assert_eq!(object, ordered);
    }

    #[test]
    fn removed_remembered_key_returns_to_its_place() {
        // Go's `delete` keeps the key in the remembered order, so a later map assignment
        // puts it back where it was, ahead of keys Go never remembered.
        let mut record = parse(r#"{"type":"t","outcome":"a","result":"b","z":1}"#)
            .unwrap()
            .as_object()
            .unwrap()
            .clone();
        record.insert_unordered("outcomes", Value::Array(Vec::new()));
        record.remove("outcome");
        record.remove("result");
        record.insert_unordered("outcome", "b");
        assert_eq!(
            compact(&Value::Object(record.clone())),
            r#"{"type":"t","outcome":"b","z":1,"outcomes":[]}"#
        );
        record.insert_unordered("result", "c");
        assert_eq!(
            record.keys().collect::<Vec<_>>(),
            ["type", "outcome", "result", "z", "outcomes"]
        );
        // Once the order is replaced, a removed key is forgotten.
        record.remove("result");
        record.insert("w", 0.0);
        record.insert_unordered("result", "d");
        assert_eq!(
            record.keys().collect::<Vec<_>>(),
            ["type", "outcome", "z", "outcomes", "w", "result"]
        );
    }

    #[test]
    fn malformed_input_is_an_error() {
        for text in [
            "",
            "   ",
            "{",
            "[1,]",
            r#"{"a":1,}"#,
            r#"{"a" 1}"#,
            "{1:2}",
            "[1 2]",
            "01",
            "-01",
            "1.",
            ".5",
            "1e",
            "1e+",
            "-",
            "+1",
            "NaN",
            "Infinity",
            "nul",
            "True",
            "'a'",
            "\"a",
            "\"a\u{1}\"",
            "\"tab\there\"",
            r#""\x""#,
            r#""\u12g4""#,
            "1 2",
            r#"{"a":1}{"b":2}"#,
            "{\"a\":1} x",
            "\u{feff}1",
            "\u{c}1",
            "// c\n1",
        ] {
            assert!(parse(text).is_err(), "accepted {text:?}");
        }
        // Errors are values, not panics, for any byte.
        assert!(parse_bytes(b"\xff").is_err());
        // Trailing CR is whitespace, as Go's decoder treats it.
        assert_eq!(round_trip("{\"a\":1}\r"), r#"{"a":1}"#);
    }

    #[test]
    fn invalid_utf8_and_lone_surrogates_become_replacement_characters() {
        // One U+FFFD per invalid byte, as the Go decoder does (checked with go1.27.0).
        let cases: &[(&[u8], &str)] = &[
            (b"\"\xff\"", "\u{fffd}"),
            (b"\"\xe2\x82\"", "\u{fffd}\u{fffd}"),
            (b"\"\xed\xb0\x80\"", "\u{fffd}\u{fffd}\u{fffd}"),
            (b"\"\xc0\xaf\"", "\u{fffd}\u{fffd}"),
            (b"\"\xff\x41\xfe\"", "\u{fffd}A\u{fffd}"),
        ];
        for (input, expected) in cases {
            assert_eq!(
                parse_bytes(input).unwrap(),
                Value::String((*expected).into()),
                "{input:?}"
            );
        }
        assert_eq!(parse(r#""😀""#).unwrap(), Value::from("😀"));
        assert_eq!(parse(r#""\ud800""#).unwrap(), Value::from("\u{fffd}"));
        assert_eq!(parse(r#""\ud800A""#).unwrap(), Value::from("\u{fffd}A"));
        assert_eq!(
            parse(r#""\udc00\ud800""#).unwrap(),
            Value::from("\u{fffd}\u{fffd}")
        );
        assert_eq!(parse(r#""\ud800😀""#).unwrap(), Value::from("\u{fffd}😀"));
        assert_eq!(parse(r#""\/é""#).unwrap(), Value::from("/é"));
    }

    #[test]
    fn nesting_is_limited_to_go_max_depth() {
        // Kept on a large stack as it was written; deep_values_are_safe_on_a_default_stack
        // runs the same depths on the default test thread.
        let check = || {
            let deep = |n: usize| format!("{}{}", "[".repeat(n), "]".repeat(n));
            let value = parse(&deep(MAX_DEPTH)).unwrap();
            assert_eq!(compact(&value), deep(MAX_DEPTH));
            assert!(parse(&deep(MAX_DEPTH + 1)).is_err());
            let objects = |n: usize| format!("{}1{}", "[{\"a\":".repeat(n), "}]".repeat(n));
            assert!(parse(&objects(MAX_DEPTH / 2)).is_ok());
            assert!(parse(&objects(MAX_DEPTH / 2 + 1)).is_err());
        };
        std::thread::Builder::new()
            .stack_size(64 << 20)
            .spawn(check)
            .unwrap()
            .join()
            .unwrap();
    }

    /// The recursive encoder this module had before it became iterative, kept as the
    /// oracle for byte-identical output.
    fn recursive_encoding(out: &mut String, value: &Value, indent: Option<usize>) {
        match value {
            Value::Array(items) => {
                out.push('[');
                let inner = indent.map(|d| d + 1);
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    if let Some(depth) = inner {
                        newline(out, depth);
                    }
                    recursive_encoding(out, item, inner);
                }
                if let (Some(depth), false) = (indent, items.is_empty()) {
                    newline(out, depth);
                }
                out.push(']');
            }
            Value::Object(object) => {
                out.push('{');
                let inner = indent.map(|d| d + 1);
                for (i, (key, item)) in object.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    if let Some(depth) = inner {
                        newline(out, depth);
                    }
                    write_string(out, key);
                    out.push(':');
                    if inner.is_some() {
                        out.push(' ');
                    }
                    recursive_encoding(out, item, inner);
                }
                if let (Some(depth), false) = (indent, object.is_empty()) {
                    newline(out, depth);
                }
                out.push('}');
            }
            scalar => write_value(out, scalar, indent).unwrap(),
        }
    }

    #[test]
    fn iterative_encoding_matches_the_recursive_encoding() {
        let texts = [
            r#"null"#,
            r#""s""#,
            r#"[]"#,
            r#"{}"#,
            r#"[[]]"#,
            r#"[{}]"#,
            r#"{"a":[]}"#,
            r#"{"a":{}}"#,
            r#"[1,[2,[3,[]],{}],{"x":{"y":[null,true,false]}},"<&>"]"#,
            r#"{"b":1,"a":{"c":[{"d":[[],[{}],{"e":" "}]}],"f":-0.0},"1":2,"0":{}}"#,
            r#"[[[[["deep",{"k":[1e21,1e-7,0.5]}]]]],{"z":[{"y":[{"x":[]}]}]}]"#,
        ];
        for text in texts {
            let value = parse(text).unwrap();
            for indent in [None, Some(0)] {
                let mut expected = String::new();
                recursive_encoding(&mut expected, &value, indent);
                let mut actual = String::new();
                write_value(&mut actual, &value, indent).unwrap();
                assert_eq!(actual, expected, "{text} indent {indent:?}");
            }
            let copy = value.clone();
            assert_eq!(copy, value, "{text}");
            assert_eq!(compact(&copy), compact(&value), "{text}");
        }
    }

    #[test]
    fn deep_values_are_safe_on_a_default_stack() {
        // Runs on the default test thread (2 MiB unless RUST_MIN_STACK says otherwise):
        // drop, clone and both encoders of a value nested as deep as the parser allows.
        let arrays = |n: usize| format!("{}{}", "[".repeat(n), "]".repeat(n));
        let text = arrays(MAX_DEPTH);
        let value = parse(&text).unwrap();
        assert_eq!(compact(&value), text);
        let copy = value.clone();
        drop(value);
        let pretty = indented(&copy);
        // Line k (from 0) of the indented form is indented 2*k spaces up to the innermost
        // `[]`, then the closing brackets come back out.
        let lines: Vec<&str> = pretty.split('\n').collect();
        assert_eq!(lines.len(), 2 * MAX_DEPTH - 1);
        assert_eq!(lines[0], "[");
        assert_eq!(lines[1], "  [");
        assert_eq!(
            lines[MAX_DEPTH - 1],
            format!("{}[]", "  ".repeat(MAX_DEPTH - 1))
        );
        assert_eq!(lines[2 * MAX_DEPTH - 2], "]");
        drop(copy);

        // Alternating objects and arrays, with siblings at every level.
        let mixed = |n: usize| format!("{}1{}", "[0,{\"a\":".repeat(n), "}]".repeat(n));
        let text = mixed(MAX_DEPTH / 2);
        let value = parse(&text).unwrap();
        assert_eq!(compact(&value), text);
        assert_eq!(compact(&value.clone()), text);
        assert!(parse(&mixed(MAX_DEPTH / 2 + 1)).is_err());

        // A value built in code can be deeper than the parser allows.
        let mut built = Value::Null;
        for _ in 0..10 * MAX_DEPTH {
            let mut object = Object::new();
            object.insert("k", built);
            built = Value::Array(vec![Value::Object(object)]);
        }
        assert_eq!(compact(&built).len(), 10 * MAX_DEPTH * 8 + 4);
        drop(built.clone());
    }

    #[test]
    fn indented_encoding_fails_past_go_max_depth() {
        // json.Indent, which the Go tool's JSONPretty runs on the compact form, accepts
        // 10000 levels and fails on more; JSONPretty then prints an empty string.
        let nest = |n: usize| {
            let mut value = Value::Null;
            for _ in 0..n {
                value = Value::Array(vec![value]);
            }
            value
        };
        assert!(to_indented_string(&nest(MAX_DEPTH)).is_ok());
        assert!(to_indented_string(&nest(MAX_DEPTH + 1)).is_err());
        let mut object = Object::new();
        object.insert("a", nest(MAX_DEPTH));
        assert!(to_indented_string(&Value::Object(object)).is_err());
        assert!(to_compact_string(&nest(MAX_DEPTH + 1)).is_ok());
    }
}
