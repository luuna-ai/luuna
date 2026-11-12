//! An order-preserving JSON value with its own parser and a Go-compatible encoder.
//!
//! The Go tool remembers the key order of every object it parses and writes keys back in
//! that order, so the store's files keep their bytes across a read and a rewrite. This
//! module provides the same model without `serde_json` (plan s5-atlas-lib, "Dependencies").
//!
//! Task t1 declares the value type and its accessors. The parser and both encoders are
//! filled in by task t2; until then they return [`AtlasError::NotImplemented`].

use crate::error::{AtlasError, Result};

/// A JSON value. Numbers are `f64`, as in the Go tool.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Number(f64),
    String(String),
    Array(Vec<Value>),
    Object(Object),
}

/// A JSON object that keeps its keys in encoding order.
///
/// Two objects are equal when they hold the same entries in the same encoding order.
/// [`Object::insert`] of a new key places it last, as the Go tool does when it remembers
/// the order in which a record's fields were set; replacing an existing key keeps its
/// position.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Object {
    entries: Vec<(String, Value)>,
}

impl Object {
    /// An empty object.
    pub fn new() -> Self {
        Object::default()
    }

    /// Sets `key` to `value`. A new key goes last; an existing key keeps its position.
    /// Returns the previous value, if any.
    pub fn insert(&mut self, key: impl Into<String>, value: impl Into<Value>) -> Option<Value> {
        let key = key.into();
        let value = value.into();
        match self.entries.iter_mut().find(|(k, _)| *k == key) {
            Some((_, slot)) => Some(std::mem::replace(slot, value)),
            None => {
                self.entries.push((key, value));
                None
            }
        }
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
    pub fn remove(&mut self, key: &str) -> Option<Value> {
        let index = self.entries.iter().position(|(k, _)| k == key)?;
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
    /// The string, if this is a string.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::String(s) => Some(s),
            _ => None,
        }
    }

    /// The number, if this is a number.
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Number(n) => Some(*n),
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

/// Parses one JSON text, keeping the key order of every object.
pub fn parse(_text: &str) -> Result<Value> {
    Err(AtlasError::not_implemented("json::parse (task t2)"))
}

/// Encodes `value` the way the Go tool writes a JSONL line: one line, no spaces, no HTML
/// escaping. The result has no trailing newline.
pub fn to_compact_string(_value: &Value) -> Result<String> {
    Err(AtlasError::not_implemented(
        "json::to_compact_string (task t2)",
    ))
}

/// Encodes `value` the way the Go tool prints a command envelope: two-space indentation,
/// no HTML escaping. The result has no trailing newline.
pub fn to_indented_string(_value: &Value) -> Result<String> {
    Err(AtlasError::not_implemented(
        "json::to_indented_string (task t2)",
    ))
}
