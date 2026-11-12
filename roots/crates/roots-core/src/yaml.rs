//! The deliberately small YAML subset used by `config.yaml`.
//!
//! Roots has never depended on a general YAML library: accepting a broad YAML
//! dialect would silently alter what existing projects can parse.  This module
//! therefore ports the historical block-mapping/scalar grammar verbatim.

use serde_json::{Map, Number, Value};
use std::error::Error;
use std::fmt::{Display, Formatter};

pub type YamlValue = Value;
pub type YamlMap = Map<String, Value>;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct YamlError {
    message: String,
    line: Option<usize>,
}

impl YamlError {
    #[must_use]
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            line: None,
        }
    }

    #[must_use]
    pub fn at_line(message: impl Into<String>, line: usize) -> Self {
        Self {
            message: message.into(),
            line: Some(line),
        }
    }
}

impl Display for YamlError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self.line {
            Some(line) => write!(formatter, "line {line}: {}", self.message),
            None => formatter.write_str(&self.message),
        }
    }
}

impl Error for YamlError {}

#[derive(Clone, Debug)]
struct Line {
    number: usize,
    indent: usize,
    text: String,
}

fn strip_comment(raw: &str) -> &str {
    let mut quote = None;
    for (index, character) in raw.char_indices() {
        if let Some(open) = quote {
            if character == open {
                quote = None;
            }
        } else if matches!(character, '\"' | '\'') {
            quote = Some(character);
        } else if character == '#'
            && (index == 0
                || raw[..index]
                    .chars()
                    .next_back()
                    .is_some_and(char::is_whitespace))
        {
            return &raw[..index];
        }
    }
    raw
}

fn tokenize(content: &str) -> Result<Vec<Line>, YamlError> {
    let mut lines = Vec::new();
    for (offset, raw) in content.lines().enumerate() {
        let text = strip_comment(raw).trim_end();
        let body = text.trim_start();
        if body.is_empty() {
            continue;
        }
        let indent = text.len() - body.len();
        if text[..indent].contains('\t') {
            return Err(YamlError::at_line("tabs are not allowed", offset + 1));
        }
        lines.push(Line {
            number: offset + 1,
            indent,
            text: body.to_owned(),
        });
    }
    Ok(lines)
}

/// Index of a YAML `key: value` separator, or `None`.
fn key_colon(text: &str) -> Option<usize> {
    let mut quote = None;
    for (index, character) in text.char_indices() {
        if let Some(open) = quote {
            if character == open {
                quote = None;
            }
        } else if matches!(character, '\"' | '\'') {
            quote = Some(character);
        } else if character == ':' {
            let after = index + character.len_utf8();
            if after == text.len() || text.as_bytes().get(after) == Some(&b' ') {
                return Some(index);
            }
        }
    }
    None
}

fn unquote(value: &str) -> String {
    if value.len() >= 2 && value.starts_with('\"') && value.ends_with('\"') {
        return serde_json::from_str(value)
            .unwrap_or_else(|_| value[1..value.len() - 1].to_owned());
    }
    if value.len() >= 2 && value.starts_with('\'') && value.ends_with('\'') {
        return value[1..value.len() - 1].replace("''", "'");
    }
    value.to_owned()
}

fn split_flow(inner: &str) -> Vec<&str> {
    let mut pieces = Vec::new();
    let mut quote = None;
    let mut start = 0;
    for (index, character) in inner.char_indices() {
        if let Some(open) = quote {
            if character == open {
                quote = None;
            }
        } else if matches!(character, '\"' | '\'') {
            quote = Some(character);
        } else if character == ',' {
            pieces.push(&inner[start..index]);
            start = index + 1;
        }
    }
    pieces.push(&inner[start..]);
    pieces
        .into_iter()
        .map(str::trim)
        .filter(|piece| !piece.is_empty())
        .collect()
}

fn is_number(value: &str) -> bool {
    let bytes = value.as_bytes();
    let mut index = 0;
    if bytes.first() == Some(&b'-') {
        index = 1;
    }
    let start_digits = index;
    while bytes.get(index).is_some_and(u8::is_ascii_digit) {
        index += 1;
    }
    if index == start_digits {
        return false;
    }
    if bytes.get(index) == Some(&b'.') {
        index += 1;
        let decimal_start = index;
        while bytes.get(index).is_some_and(u8::is_ascii_digit) {
            index += 1;
        }
        if index == decimal_start {
            return false;
        }
    }
    index == bytes.len()
}

pub fn parse_scalar(value: &str) -> Result<YamlValue, YamlError> {
    if value.starts_with('[') {
        if !value.ends_with(']') {
            return Err(YamlError::new(format!("unclosed flow sequence: {value}")));
        }
        return split_flow(&value[1..value.len() - 1])
            .into_iter()
            .map(parse_scalar)
            .collect::<Result<Vec<_>, _>>()
            .map(Value::Array);
    }
    if value == "{}" {
        return Ok(Value::Object(Map::new()));
    }
    if value.starts_with('{') {
        return Err(YamlError::new(format!(
            "flow mappings are not supported: {value}"
        )));
    }
    if value.starts_with('\"') || value.starts_with('\'') {
        return Ok(Value::String(unquote(value)));
    }
    match value {
        "true" => Ok(Value::Bool(true)),
        "false" => Ok(Value::Bool(false)),
        "null" | "~" | "" => Ok(Value::Null),
        _ if is_number(value) => value
            .parse::<Number>()
            .map(Value::Number)
            .map_err(|_| YamlError::new(format!("invalid number: {value}"))),
        _ => Ok(Value::String(value.to_owned())),
    }
}

struct Parsed<T> {
    value: T,
    next: usize,
}

fn parse_sequence(
    lines: &[Line],
    start: usize,
    indent: usize,
) -> Result<Parsed<Vec<YamlValue>>, YamlError> {
    let mut values = Vec::new();
    let mut index = start;
    while let Some(line) = lines.get(index) {
        if line.indent != indent || !line.text.starts_with('-') {
            break;
        }
        let rest = line.text[1..].trim();
        if rest.is_empty() || key_colon(rest).is_some() {
            return Err(YamlError::at_line(
                "only scalar sequence items are supported",
                line.number,
            ));
        }
        values.push(
            parse_scalar(rest).map_err(|error| YamlError::at_line(error.message, line.number))?,
        );
        index += 1;
    }
    Ok(Parsed {
        value: values,
        next: index,
    })
}

fn parse_value_block(
    lines: &[Line],
    start: usize,
    parent_indent: usize,
) -> Result<Parsed<YamlValue>, YamlError> {
    let Some(next) = lines.get(start) else {
        return Ok(Parsed {
            value: Value::Null,
            next: start,
        });
    };
    if next.indent < parent_indent || (next.indent == parent_indent && !next.text.starts_with('-'))
    {
        return Ok(Parsed {
            value: Value::Null,
            next: start,
        });
    }
    if next.text.starts_with('-') {
        let parsed = parse_sequence(lines, start, next.indent)?;
        return Ok(Parsed {
            value: Value::Array(parsed.value),
            next: parsed.next,
        });
    }
    let parsed = parse_mapping(lines, start, next.indent)?;
    Ok(Parsed {
        value: Value::Object(parsed.value),
        next: parsed.next,
    })
}

fn parse_mapping(
    lines: &[Line],
    start: usize,
    indent: usize,
) -> Result<Parsed<YamlMap>, YamlError> {
    let mut map = Map::new();
    let mut index = start;
    while let Some(line) = lines.get(index) {
        if line.indent < indent {
            break;
        }
        if line.indent > indent {
            return Err(YamlError::at_line("unexpected indentation", line.number));
        }
        let Some(colon) = key_colon(&line.text) else {
            return Err(YamlError::at_line(
                format!(r#"expected "key: value" (got "{}")"#, line.text),
                line.number,
            ));
        };
        let key = unquote(line.text[..colon].trim());
        let rest = line.text[colon + 1..].trim();
        if !rest.is_empty() {
            map.insert(
                key,
                parse_scalar(rest)
                    .map_err(|error| YamlError::at_line(error.message, line.number))?,
            );
            index += 1;
            continue;
        }
        let parsed = parse_value_block(lines, index + 1, indent)?;
        map.insert(key, parsed.value);
        index = parsed.next;
    }
    Ok(Parsed {
        value: map,
        next: index,
    })
}

pub fn parse_yaml(content: &str) -> Result<YamlMap, YamlError> {
    let lines = tokenize(content)?;
    let Some(first) = lines.first() else {
        return Ok(Map::new());
    };
    if first.text.starts_with('-') {
        return Err(YamlError::new("top-level sequences are not supported"));
    }
    let parsed = parse_mapping(&lines, 0, first.indent)?;
    if let Some(leftover) = lines.get(parsed.next) {
        return Err(YamlError::at_line("unexpected content", leftover.number));
    }
    Ok(parsed.value)
}

fn needs_quotes(value: &str) -> bool {
    if value.is_empty() || value != value.trim() || matches!(value, "true" | "false" | "null" | "~")
    {
        return true;
    }
    is_number(value)
        || value.starts_with('-')
        || value.chars().any(|character| {
            matches!(
                character,
                ':' | '#'
                    | '\"'
                    | '\''
                    | '['
                    | ']'
                    | '{'
                    | '}'
                    | ','
                    | '&'
                    | '*'
                    | '!'
                    | '|'
                    | '>'
                    | '%'
                    | '@'
                    | '`'
                    | '\n'
            )
        })
}

fn emit_number(value: &Number) -> String {
    if let Some(integer) = value.as_i64() {
        return integer.to_string();
    }
    if let Some(integer) = value.as_u64() {
        return integer.to_string();
    }
    let decimal = value.as_f64().unwrap_or(0.0);
    if decimal.fract() == 0.0 {
        format!("{decimal:.0}")
    } else {
        decimal.to_string()
    }
}

fn emit_scalar(value: &YamlValue) -> Result<String, YamlError> {
    match value {
        Value::Null => Ok("null".to_owned()),
        Value::String(text) => Ok(if needs_quotes(text) {
            serde_json::to_string(text).expect("a JSON string always serializes")
        } else {
            text.clone()
        }),
        Value::Bool(value) => Ok(value.to_string()),
        Value::Number(value) => Ok(emit_number(value)),
        Value::Array(values) => values
            .iter()
            .map(emit_scalar)
            .collect::<Result<Vec<_>, _>>()
            .map(|items| format!("[{}]", items.join(", "))),
        Value::Object(_) => Err(YamlError::new(
            "nested mapping cannot be emitted as a scalar",
        )),
    }
}

fn emit_map(map: &YamlMap, indent: usize, out: &mut Vec<String>) -> Result<(), YamlError> {
    let pad = " ".repeat(indent);
    for (key, value) in map {
        let key = if needs_quotes(key) {
            serde_json::to_string(key).expect("a JSON string always serializes")
        } else {
            key.clone()
        };
        if let Value::Object(child) = value {
            if !child.is_empty() {
                out.push(format!("{pad}{key}:"));
                emit_map(child, indent + 2, out)?;
                continue;
            }
            out.push(format!("{pad}{key}: {{}}"));
            continue;
        }
        out.push(format!("{pad}{key}: {}", emit_scalar(value)?));
    }
    Ok(())
}

pub fn stringify_yaml(map: &YamlMap) -> Result<String, YamlError> {
    let mut lines = Vec::new();
    emit_map(map, 0, &mut lines)?;
    Ok(format!("{}\n", lines.join("\n")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_roots_subset_and_ignores_comments() {
        let value = parse_yaml(
            "project: \"roots # not comment\"\nagent:\n  command: null # trailing\n  flags: [a, 'b,c', 3]\n",
        )
        .expect("parse config");
        assert_eq!(value["project"], "roots # not comment");
        assert_eq!(value["agent"]["command"], Value::Null);
        assert_eq!(value["agent"]["flags"], serde_json::json!(["a", "b,c", 3]));
    }

    #[test]
    fn serializes_stably_and_rejects_unsupported_shapes() {
        let value = parse_yaml("a: hello\nb:\n  c: 1\n").expect("parse config");
        assert_eq!(
            stringify_yaml(&value).expect("render config"),
            "a: hello\nb:\n  c: 1\n"
        );
        assert_eq!(
            parse_yaml("a:\n  - key: value\n").unwrap_err().to_string(),
            "line 2: only scalar sequence items are supported"
        );
    }
}
