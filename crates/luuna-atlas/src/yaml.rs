//! The subset of YAML that config.yaml uses, read into the order-preserving JSON value and
//! written the way the Go tool's yaml.v3 encoder writes it.
//!
//! Supported: block maps and sequences, empty and simple flow collections, plain, single-
//! and double-quoted scalars, literal and folded block scalars, comments. Anchors, aliases,
//! merge keys, tags and several documents are refused with
//! [`AtlasError::UnsupportedYaml`]. Task t1 declares the functions; task t4 fills them in.

use crate::error::{AtlasError, Result};
use crate::json::Value;

/// Reads one YAML document.
pub fn parse(_text: &str) -> Result<Value> {
    Err(AtlasError::not_implemented("yaml::parse (task t4)"))
}

/// Writes `value` as the Go tool does: two-space indentation, ending with a newline.
pub fn to_yaml_string(_value: &Value) -> Result<String> {
    Err(AtlasError::not_implemented(
        "yaml::to_yaml_string (task t4)",
    ))
}
