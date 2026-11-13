//! Full YAML nodes for constructs beyond the exact foundation reader.
//! Mirrors pinned Go config.go yamlValue and yaml_compat.go validation.
use super::{scalar_value, source_error, Style};
use crate::{
    json::{Object, Value},
    records, AtlasError, Result,
};
use granit_parser::{
    Event, Options, Parser, ScalarStyle as TScalarStyle, Scanner, StrInput, Tag, TokenType,
};
use std::collections::{HashMap, HashSet};
enum Kind {
    Scalar(String, TScalarStyle, Option<Tag>),
    Sequence(Vec<usize>),
    Mapping(Vec<usize>),
    Alias(usize),
}
struct Node {
    kind: Kind,
    line: usize,
    column: usize,
}
struct Document {
    nodes: Vec<Node>,
    root: Option<usize>,
    end: usize,
}
fn error(text: impl Into<String>) -> AtlasError {
    AtlasError::Message(text.into())
}
impl Document {
    fn read(text: &str) -> Result<Self> {
        for line in text.lines() {
            if let Some(version) = line.strip_prefix("%YAML ") {
                if version.trim() != "1.1" {
                    return Err(error("yaml: found incompatible YAML document"));
                }
            } else if !line.trim().is_empty()
                && !line.trim_start().starts_with('#')
                && !line.starts_with('%')
            {
                break;
            }
        }
        let mut doc = Self {
            nodes: Vec::new(),
            root: None,
            end: 0,
        };
        let mut options = Options::default();
        options.flow_nesting_limit = super::MAX_DEPTH + 1;
        options.block_nesting_limit = super::MAX_DEPTH * 2 + 2;
        options.max_directive_bytes = usize::MAX;
        options.max_reserved_directive_params = usize::MAX;
        options.emit_unresolved_aliases = true;
        options.emit_comments = false;
        let mut parser = Parser::new_from_str_with_options(text, options.clone());
        let mut open: Vec<usize> = Vec::new();
        let mut anchors = HashMap::new();
        let end = loop {
            let Some(result) = parser.next_event() else {
                break text.chars().count();
            };
            let (event, span) = result.map_err(|e| {
                let message = e.info();
                if message.contains("nesting limit") || message.contains("recursion limit") {
                    return super::depth_error([0, e.marker().line() - 1]);
                }
                if message.contains("unknown anchor") {
                    let name = text
                        .lines()
                        .nth(e.marker().line().saturating_sub(1))
                        .unwrap_or_default()
                        .split('*')
                        .nth(1)
                        .unwrap_or_default()
                        .split(|c: char| c.is_whitespace() || matches!(c, ',' | ']' | '}'))
                        .next()
                        .unwrap_or_default();
                    error(format!("yaml: unknown anchor '{name}' referenced"))
                } else {
                    error(format!("yaml: line {}: {message}", e.marker().line()))
                }
            })?;
            let mark = span.start;
            let (kind, anchor) = match event {
                Event::StreamEnd | Event::DocumentEnd => break mark.index(),
                Event::StreamStart | Event::DocumentStart(_, _) | Event::Anchor(_, _) => continue,
                Event::SequenceEnd | Event::MappingEnd => {
                    open.pop();
                    continue;
                }
                Event::Scalar(value, style, anchor, tag) => (
                    Kind::Scalar(value.into_owned(), style, tag.map(|t| t.into_owned())),
                    anchor,
                ),
                Event::SequenceStart(_, anchor, _) => (Kind::Sequence(Vec::new()), anchor),
                Event::MappingStart(_, anchor, _) => (Kind::Mapping(Vec::new()), anchor),
                Event::UnresolvedAlias(name) => {
                    return Err(error(format!("yaml: unknown anchor '{name}' referenced")))
                }
                Event::Alias(anchor) => (
                    Kind::Alias(
                        *anchors
                            .get(&anchor)
                            .ok_or_else(|| error("yaml: unknown anchor referenced"))?,
                    ),
                    0,
                ),
                _ => {
                    return Err(error(
                        "Unexpected YAML parser event; the file is unchanged.",
                    ))
                }
            };
            let collection = matches!(kind, Kind::Sequence(_) | Kind::Mapping(_));
            let index = doc.nodes.len();
            doc.nodes.push(Node {
                kind,
                line: mark.line(),
                column: mark.col() + 1,
            });
            if anchor > 0 {
                anchors.insert(anchor, index);
            }
            if let Some(&parent) = open.last() {
                match &mut doc.nodes[parent].kind {
                    Kind::Sequence(children) | Kind::Mapping(children) => children.push(index),
                    _ => unreachable!("open node is a collection"),
                }
            } else {
                doc.root = Some(index);
            }
            if collection {
                open.push(index);
            }
        };
        doc.end = end;
        // Parser events locate the scalar after its tag/anchor. Go's Node
        // locates the first property, which duplicate diagnostics must quote.
        let mut properties = HashMap::new();
        let mut pending = None;
        let mut flow_depth = 0usize;
        let mut block_depth = 0usize;
        let mut simple_key_line = 0usize;
        for token in Scanner::with_options(StrInput::new(text), options) {
            let Ok(token) = token else {
                break;
            }; // First document was already parsed above.
            let mark = token.span().start;
            if mark.index() >= end {
                break;
            }
            match token.token_type() {
                TokenType::FlowSequenceStart | TokenType::FlowMappingStart => {
                    flow_depth += 1;
                    if flow_depth > super::MAX_DEPTH {
                        return Err(super::depth_error([0, mark.line() - 1]));
                    }
                    simple_key_line = mark.line() - 1;
                }
                TokenType::FlowSequenceEnd | TokenType::FlowMappingEnd => {
                    flow_depth = flow_depth.saturating_sub(1);
                }
                TokenType::BlockSequenceStart | TokenType::BlockMappingStart => {
                    block_depth += 1;
                    if block_depth > super::MAX_DEPTH {
                        let context = if matches!(token.token_type(), TokenType::BlockMappingStart)
                        {
                            mark.line() - 1
                        } else {
                            simple_key_line
                        };
                        return Err(super::depth_error([context, mark.line() - 1]));
                    }
                }
                TokenType::BlockEnd => {
                    block_depth = block_depth.saturating_sub(1);
                }
                TokenType::Scalar(_, _)
                | TokenType::Tag(_, _)
                | TokenType::Anchor(_)
                | TokenType::Alias(_) => {
                    simple_key_line = mark.line() - 1;
                }
                _ => {}
            }
            match token.token_type() {
                TokenType::Tag(_, _) | TokenType::Anchor(_) => {
                    if pending.is_none() {
                        pending = Some(mark);
                    }
                }
                TokenType::Scalar(_, _)
                | TokenType::Alias(_)
                | TokenType::FlowSequenceStart
                | TokenType::FlowMappingStart
                | TokenType::BlockSequenceStart
                | TokenType::BlockMappingStart => {
                    if let Some(property) = pending.take() {
                        properties.insert(
                            (mark.line(), mark.col() + 1),
                            (property.line(), property.col() + 1),
                        );
                    }
                }
                _ => {}
            }
        }
        for node in &mut doc.nodes {
            if let Some(&(line, column)) = properties.get(&(node.line, node.column)) {
                node.line = line;
                node.column = column;
            }
        }
        Ok(doc)
    }
    fn merge(&self, index: usize) -> bool {
        match &self.nodes[index].kind {
            Kind::Scalar(raw, style, tag) => {
                core_tag(tag.as_ref()) == Some("merge")
                    || (tag.is_none() && *style == TScalarStyle::Plain && raw == "<<")
            }
            _ => false,
        }
    }
    fn convert(&self, text: &str) -> Result<Value> {
        let Some(root) = self.root else {
            return Ok(Value::Null);
        };
        enum Frame {
            Array {
                index: usize,
                next: usize,
                values: Vec<Value>,
                validate: bool,
            },
            Map {
                index: usize,
                next: usize,
                values: Object,
                key: Option<Value>,
                seen: HashSet<Vec<u8>>,
                validate: bool,
            },
            Alias(usize),
        }
        let mut stack: Vec<Frame> = Vec::new();
        let mut active = HashSet::new();
        let mut next = Some((root, true));
        let mut done = None;
        loop {
            if let Some((index, validate)) = next.take() {
                if !active.insert(index) {
                    return Err(error("Refusing cyclic YAML aliases: Go cannot finish expanding this document. The file is unchanged."));
                }
                match &self.nodes[index].kind {
                    Kind::Scalar(raw, style, tag) => {
                        done = Some(scalar(raw, *style, tag.as_ref()));
                        active.remove(&index);
                    }
                    Kind::Sequence(_) => stack.push(Frame::Array {
                        index,
                        next: 0,
                        values: Vec::new(),
                        validate,
                    }),
                    Kind::Mapping(_) => stack.push(Frame::Map {
                        index,
                        next: 0,
                        values: Object::new(),
                        key: None,
                        seen: HashSet::new(),
                        validate,
                    }),
                    Kind::Alias(target) => {
                        stack.push(Frame::Alias(index));
                        next = Some((*target, validate));
                        continue;
                    }
                }
            }
            let Some(frame) = stack.last_mut() else {
                return Ok(done.expect("root conversion finished"));
            };
            match frame {
                Frame::Alias(index) => {
                    active.remove(index);
                    stack.pop();
                }
                Frame::Array {
                    index,
                    next: position,
                    values,
                    validate,
                } => {
                    if let Some(value) = done.take() {
                        values.push(value);
                    }
                    let Kind::Sequence(children) = &self.nodes[*index].kind else {
                        unreachable!()
                    };
                    if let Some(child) = children.get(*position) {
                        *position += 1;
                        next = Some((*child, *validate));
                    } else {
                        active.remove(index);
                        done = Some(Value::Array(std::mem::take(values)));
                        stack.pop();
                    }
                }
                Frame::Map {
                    index,
                    next: position,
                    values,
                    key,
                    seen,
                    validate,
                } => {
                    let Kind::Mapping(children) = &self.nodes[*index].kind else {
                        unreachable!()
                    };
                    if let Some(value) = done.take() {
                        if key.is_none() {
                            *key = Some(value);
                        } else {
                            let key_node = children[*position - 2];
                            if self.merge(key_node) {
                                merge(values, &value);
                            } else {
                                let bytes = key_bytes(key.as_ref().expect("map key ready"));
                                if *validate && !seen.insert(bytes.clone()) {
                                    let node = &self.nodes[key_node];
                                    return Err(error(source_error(
                                        "duplicated mapping key",
                                        text,
                                        node.line,
                                        node.column,
                                    )));
                                }
                                values.insert_go_key(&bytes, value);
                            }
                            *key = None;
                        }
                    }
                    if let Some(child) = children.get(*position) {
                        let validate_child = *validate && *position % 2 == 1;
                        *position += 1;
                        next = Some((*child, validate_child));
                    } else {
                        active.remove(index);
                        done = Some(Value::Object(std::mem::take(values)));
                        stack.pop();
                    }
                }
            }
        }
    }
}
fn core_tag(tag: Option<&Tag>) -> Option<&'static str> {
    let suffix = tag?.suffix_in_namespace("tag:yaml.org,2002:")?;
    match suffix.as_ref() {
        "str" => Some("str"),
        "binary" => Some("binary"),
        "int" => Some("int"),
        "float" => Some("float"),
        "bool" => Some("bool"),
        "null" => Some("null"),
        "timestamp" => Some("timestamp"),
        "merge" => Some("merge"),
        "seq" => Some("seq"),
        "map" => Some("map"),
        _ => None,
    }
}
fn scalar(raw: &str, style: TScalarStyle, tag: Option<&Tag>) -> Value {
    let resolved = || scalar_value(raw, Style::Plain);
    match core_tag(tag) {
        Some("str") => Value::from(raw),
        Some("binary") => decode_binary(raw)
            .map(Value::from_go_bytes)
            .unwrap_or_else(|| Value::from(raw)),
        Some("int") => {
            let value = resolved();
            if matches!(value, Value::Integer(_)) {
                value
            } else {
                Value::from(raw)
            }
        }
        Some("float") => {
            let value = resolved();
            match &value {
                Value::Integer(i) => Value::Number(*i as f64),
                Value::Number(_) => value,
                _ => Value::from(raw),
            }
        }
        Some("bool") => {
            let value = resolved();
            if matches!(value, Value::Bool(_)) {
                value
            } else {
                Value::from(raw)
            }
        }
        Some("null") => {
            let value = resolved();
            if matches!(value, Value::Null) {
                value
            } else {
                Value::from(raw)
            }
        }
        Some("timestamp") => super::parse_timestamp(raw)
            .map(Value::String)
            .unwrap_or_else(|| Value::from(raw)),
        Some(_) => Value::from(raw),
        None if tag.is_some() => Value::from(raw),
        None => scalar_value(
            raw,
            if style == TScalarStyle::Plain {
                Style::Plain
            } else {
                Style::Single
            },
        ),
    }
}
fn decode_binary(raw: &str) -> Option<Vec<u8>> {
    let bytes = raw
        .bytes()
        .filter(|b| !matches!(b, b'\r' | b'\n'))
        .collect::<Vec<_>>();
    if bytes.len() % 4 != 0 {
        return None;
    }
    let mut out = Vec::new();
    let digit = |b: u8| match b {
        b'A'..=b'Z' => Some(b - b'A'),
        b'a'..=b'z' => Some(b - b'a' + 26),
        b'0'..=b'9' => Some(b - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    };
    for (index, chunk) in bytes.chunks(4).enumerate() {
        let a = digit(chunk[0])?;
        let b = digit(chunk[1])?;
        out.push((a << 2) | (b >> 4));
        if chunk[2] == b'=' {
            if chunk[3] != b'=' || index + 1 != bytes.len() / 4 {
                return None;
            }
            continue;
        }
        let c = digit(chunk[2])?;
        out.push((b << 4) | (c >> 2));
        if chunk[3] == b'=' {
            if index + 1 != bytes.len() / 4 {
                return None;
            }
            continue;
        }
        let d = digit(chunk[3])?;
        out.push((c << 6) | d);
    }
    Some(out)
}
fn key_bytes(value: &Value) -> Vec<u8> {
    if value.is_null() {
        return b"null".to_vec();
    }
    if let Some(bytes) = value.string_bytes() {
        return bytes.to_vec();
    }
    if let Value::Number(n) = value {
        if *n == f64::INFINITY {
            return b"Infinity".to_vec();
        }
        if *n == f64::NEG_INFINITY {
            return b"-Infinity".to_vec();
        }
    }
    // Text recursively joins arrays. Preserve binary scalar bytes during that walk.
    enum Part<'a> {
        Value(&'a Value),
        Comma,
    }
    let mut stack = vec![Part::Value(value)];
    let mut out = Vec::new();
    while let Some(part) = stack.pop() {
        match part {
            Part::Comma => out.push(b','),
            Part::Value(value) => {
                if let Some(bytes) = value.string_bytes() {
                    out.extend_from_slice(bytes);
                } else if let Value::Array(items) = value {
                    for (i, item) in items.iter().enumerate().rev() {
                        stack.push(Part::Value(item));
                        if i > 0 {
                            stack.push(Part::Comma);
                        }
                    }
                } else {
                    out.extend_from_slice(records::text_value(Some(value)).as_bytes());
                }
            }
        }
    }
    out
}
fn merge(target: &mut Object, value: &Value) {
    let sources: Vec<&Value> = if let Value::Array(values) = value {
        values.iter().collect()
    } else {
        vec![value]
    };
    for source in sources {
        if let Value::Object(object) = source {
            for (key, value) in object.iter() {
                let bytes = object.key_bytes(key);
                if target.get_go_key(bytes).is_none() {
                    target.insert_go_key(bytes, value.clone());
                }
            }
        }
    }
}
pub(super) fn parse(text: &str) -> Result<Value> {
    Document::read(text)?.convert(text)
}

pub(super) fn has_later_data(text: &str) -> Result<bool> {
    let document = Document::read(text)?;
    let offset = text
        .char_indices()
        .nth(document.end)
        .map(|(i, _)| i)
        .unwrap_or(text.len());
    Ok(text[offset..].lines().any(|line| {
        let line = line.trim();
        !line.is_empty() && !line.starts_with('#') && line != "---" && line != "..."
    }))
}
