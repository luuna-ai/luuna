//! The YAML reader behind every `config.yaml` read of this crate: the first
//! document of the file, decoded as yaml.v3 v3.0.1 decodes it into a
//! `map[string]any` (`yaml.Unmarshal`, `gopkg.in/yaml.v3@v3.0.1`).
//!
//! `config.yaml` is repository content anyone with write access can shape,
//! so the reader is built in three steps that never recurse on the input:
//!
//! 1. yaml-rust2's parser events are pulled one at a time into an arena, one
//!    small node per event, with each alias kept as a reference to its
//!    anchored node and never copied. Reading stops at the first
//!    `DocumentEnd`: yaml.v3 parses one document, so later documents, a
//!    broken one or a billion-laughs one alike, are never read (A301,
//!    observed with the Go tool).
//! 2. An iterative walk over the arena, in the order yaml.v3's decoder visits
//!    nodes, applies yaml.v3's alias limit and self-reference check
//!    (`decode.go:455-491`, `:533-544`) and the depth bound of this port
//!    (A312), and finds every input refused below, before any value is
//!    built.
//! 3. The same walk runs again and builds the [`TreeValue`].
//!
//! The walk follows the approach of `luuna-beacon`'s `config.rs`
//! (`Document::read` and `Document::check`, its alias constants and tests);
//! the code was copied and adapted here, and this crate does not depend on
//! `luuna-beacon`.
//!
//! The alias errors are Go's texts, as [`Error::Command`]. Every other input
//! yaml.v3 rejects, or that this port cannot show it reads as yaml.v3 does,
//! is an [`Error::Unsupported`] whose plan is
//! [`YAML_ERROR_TEXTS_NOT_PORTED`] (A303).

use std::collections::{HashMap, HashSet};
use std::path::Path;

use yaml_rust2::parser::{Event, Parser, Tag};
use yaml_rust2::scanner::{Scanner, TScalarStyle, TokenType};

use super::TreeValue;
use crate::error::{Error, Result};

/// The plan named by every refusal of this reader other than the two alias
/// errors: no plan ports yaml.v3's error texts (U7).
pub(crate) const YAML_ERROR_TEXTS_NOT_PORTED: &str =
    "none (yaml.v3's error texts are not ported; see notes/s4-missions-lib-3/README.md)";

/// The deepest value this reader builds, aliases expanded (A312): the root
/// mapping is level 1. yaml.v3 accepts up to 10000 block levels; this bound
/// keeps every recursive walk of a [`TreeValue`] (its derived `Drop`,
/// `Clone`, `PartialEq` and `Debug` included) within 1000 frames.
const MAX_DEPTH: usize = 1000;

/// yaml.v3's `alias_ratio_range_low` and `alias_ratio_range_high`
/// (decode.go:455-462).
const ALIAS_RATIO_RANGE_LOW: u64 = 400_000;
const ALIAS_RATIO_RANGE_HIGH: u64 = 4_000_000;

/// yaml.v3's error for too many decodes under an alias (decode.go:490), with
/// the `yaml: ` prefix of `handleErr`.
const EXCESSIVE_ALIASING: &str = "yaml: document contains excessive aliasing";

/// The Go type yaml.v3 decodes a mapping with a key that is not a string
/// into, when it decodes into `any`.
const ANY_KEY_MAP: &str = "map[interface {}]interface {}";

/// yaml.v3's long tag prefix (resolve.go:94).
const LONG_TAG_PREFIX: &str = "tag:yaml.org,2002:";

/// yaml.v3's `allowedAliasRatio` (decode.go:468-482).
fn allowed_alias_ratio(decode_count: u64) -> f64 {
    if decode_count <= ALIAS_RATIO_RANGE_LOW {
        0.99
    } else if decode_count >= ALIAS_RATIO_RANGE_HIGH {
        0.10
    } else {
        let range = (ALIAS_RATIO_RANGE_HIGH - ALIAS_RATIO_RANGE_LOW) as f64;
        0.99 - 0.89 * ((decode_count - ALIAS_RATIO_RANGE_LOW) as f64 / range)
    }
}

/// Decodes the first YAML document of `text` as yaml.v3 decodes it into a
/// `map[string]any`: `None` when Go's map stays nil (an empty stream, or a
/// first document whose root is null), otherwise a [`TreeValue::Map`].
///
/// This is the only function that turns YAML into values.
pub(crate) fn decode_document(text: &[u8]) -> Result<Option<TreeValue>> {
    let text = checked_text(text)?;
    let (document, _) = Document::read(text)?;
    let Some(root) = document.root else {
        return Ok(None);
    };
    // Every limit is applied, and every refusal found, before a value is
    // built; the second walk then makes the same decisions and builds.
    Walk::new(&document, false).run(root)?;
    Walk::new(&document, true).run(root)
}

/// Config flow arguments decode into `any`, through the same pre-build guards.
pub(crate) fn decode_flow_value(text: &[u8]) -> Result<TreeValue> {
    let text = checked_text(text)?;
    let (document, _) = Document::read(text)?;
    let Some(root) = document.root else {
        return Ok(TreeValue::Null);
    };
    Walk::new(&document, false).run_value(root)?;
    Ok(Walk::new(&document, true)
        .run_value(root)?
        .unwrap_or(TreeValue::Null))
}

/// Declaration order from unexpanded YAML nodes, as collectKeyOrder does.
pub(crate) fn key_orders(text: &[u8]) -> Result<std::collections::BTreeMap<String, Vec<String>>> {
    let text = checked_text(text)?;
    let (document, _) = Document::read(text)?;
    let mut result = std::collections::BTreeMap::new();
    let mut stack = Vec::new();
    if let Some(root) = document.root {
        stack.push((root, String::new()));
    }
    while let Some((index, path)) = stack.pop() {
        match &document.nodes[index].kind {
            Kind::Sequence(children) => {
                for (i, child) in children.iter().enumerate().rev() {
                    stack.push((*child, format!("{path}/{i}")));
                }
            }
            Kind::Mapping(children) => {
                let mut keys = Vec::new();
                let mut children_in_order = Vec::new();
                for pair in children.chunks_exact(2) {
                    let key = match &document.nodes[pair[0]].kind {
                        Kind::Scalar { text, .. } => text.clone(),
                        Kind::Alias { ordinal, .. } => document.alias_names[*ordinal].clone(),
                        _ => String::new(),
                    };
                    keys.push(key.clone());
                    children_in_order.push((pair[1], format!("{path}/{key}")));
                }
                stack.extend(children_in_order.into_iter().rev());
                result.insert(path, keys);
            }
            _ => {}
        }
    }
    Ok(result)
}

/// Whether `text` holds anything after its first YAML document: pulls
/// exactly one parser event after the first `DocumentEnd`. A parse error in
/// that event means a later document is there (a broken one).
#[allow(dead_code, reason = "called by config set and unset in t4 (DL304)")]
pub(crate) fn has_later_document(text: &[u8]) -> Result<bool> {
    let text = checked_text(text)?;
    let (_, rest) = Document::read(text)?;
    let Some(mut parser) = rest else {
        return Ok(false);
    };
    Ok(!matches!(parser.next_token(), Ok((Event::StreamEnd, _))))
}

/// `error` for the file at `path`: a refusal of this reader names the file.
pub(crate) fn in_file(path: &Path, error: Error) -> Error {
    match error {
        Error::Unsupported { feature, plan } => Error::Unsupported {
            feature: format!("{}: {feature}", path.display()),
            plan,
        },
        other => other,
    }
}

/// A refusal for input yaml.v3 rejects, or treats in a way not ported.
fn yaml_v3_error(reason: impl std::fmt::Display) -> Error {
    Error::unsupported(
        format!("yaml.v3's error for this input ({reason})"),
        YAML_ERROR_TEXTS_NOT_PORTED,
    )
}

/// A refusal for input Go reads but this port does not, because a rewrite
/// could not keep what Go reads or because Go's result is not ported.
fn not_read(reason: impl std::fmt::Display) -> Error {
    Error::unsupported(reason.to_string(), YAML_ERROR_TEXTS_NOT_PORTED)
}

/// `text` as UTF-8 with only the characters yaml.v3's reader accepts
/// (`yaml_parser_update_buffer`: tab, line breaks and the printable ranges).
/// U+FEFF is refused anywhere, also as a leading byte order mark.
fn checked_text(text: &[u8]) -> Result<&str> {
    let text = std::str::from_utf8(text)
        .map_err(|error| yaml_v3_error(format_args!("invalid UTF-8: {error}")))?;
    if let Some(bad) = text.chars().find(|&ch| !is_printable(ch)) {
        return Err(yaml_v3_error(format_args!(
            "the character U+{:04X}",
            u32::from(bad)
        )));
    }
    Ok(text)
}

/// The characters yaml.v3 reads, without U+FEFF.
fn is_printable(ch: char) -> bool {
    matches!(ch,
        '\t' | '\n' | '\r' | '\u{20}'..='\u{7e}' | '\u{85}' | '\u{a0}'..='\u{d7ff}'
        | '\u{e000}'..='\u{fffd}' | '\u{10000}'..='\u{10ffff}')
        && ch != '\u{feff}'
}

// ---------------------------------------------------------------------------
// Scalars
// ---------------------------------------------------------------------------

/// A scalar as yaml.v3 decodes it into `any` (`resolve`, resolve.go:126-265,
/// and `decoder.scalar`, decode.go:565-588). `Str` is the node's text.
#[derive(Debug, Clone, PartialEq)]
enum Resolved {
    Null,
    Bool(bool),
    Int(i64),
    Uint(u64),
    Float(f64),
    Str,
}

/// A key's `ShortTag` as far as yaml.v3's `isStringMap` tells them apart
/// (decode.go:877-889, yaml.go:435-461).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KeyTag {
    Str,
    Merge,
    Other,
}

/// yaml.v3's `resolve` for a plain scalar: `untagged` when it has no tag
/// (or the non-specific `!`), otherwise for a `!!int`, `!!float`, `!!bool`
/// or `!!null` tag, where no timestamp is tried. The error names a value
/// whose type depends on Go's `time.Parse`, which is not ported.
fn resolve(value: &str, untagged: bool) -> Result<Resolved, String> {
    match value {
        "true" | "True" | "TRUE" => return Ok(Resolved::Bool(true)),
        "false" | "False" | "FALSE" => return Ok(Resolved::Bool(false)),
        "" | "~" | "null" | "Null" | "NULL" => return Ok(Resolved::Null),
        ".nan" | ".NaN" | ".NAN" => return Ok(Resolved::Float(f64::NAN)),
        ".inf" | ".Inf" | ".INF" | "+.inf" | "+.Inf" | "+.INF" => {
            return Ok(Resolved::Float(f64::INFINITY))
        }
        "-.inf" | "-.Inf" | "-.INF" => return Ok(Resolved::Float(f64::NEG_INFINITY)),
        _ => {}
    }
    match value.as_bytes()[0] {
        // `strconv.ParseFloat(in, 64)`; an error, overflow included, leaves
        // a string.
        b'.' => Ok(match finite_float(value) {
            Some(number) => Resolved::Float(number),
            None => Resolved::Str,
        }),
        first @ (b'+' | b'-' | b'0'..=b'9') => {
            // `parseTimestamp` only tries text that starts with four digits
            // and `-` (resolve.go:184-189, :302-312).
            let digits = value.bytes().take_while(u8::is_ascii_digit).count();
            if untagged
                && first.is_ascii_digit()
                && digits == 4
                && value.as_bytes().get(4) == Some(&b'-')
            {
                return Err(format!("the timestamp-like value {value:?}"));
            }
            let plain = value.replace('_', "");
            let negative = plain.starts_with('-');
            if let Some(Some(magnitude)) = go_base0_magnitude(&plain) {
                // `strconv.ParseInt(plain, 0, 64)`.
                let limit = (1u128 << 63) - u128::from(!negative);
                if magnitude <= limit {
                    let magnitude = magnitude as i128;
                    let number = if negative { -magnitude } else { magnitude };
                    return Ok(Resolved::Int(number as i64));
                }
                // `strconv.ParseUint(plain, 0, 64)`, which takes no sign.
                if !plain.starts_with(['+', '-']) {
                    if let Ok(number) = u64::try_from(magnitude) {
                        return Ok(Resolved::Uint(number));
                    }
                }
            }
            if is_yaml_style_float(&plain) {
                if let Some(number) = finite_float(&plain) {
                    return Ok(Resolved::Float(number));
                }
            }
            // The later `0b`, `-0b`, `0o` and `-0o` cases accept nothing
            // that `ParseInt` and `ParseUint` with base 0 did not.
            Ok(Resolved::Str)
        }
        _ => Ok(Resolved::Str),
    }
}

/// `text` as a finite f64 when Go's `ParseFloat` accepts it without error.
fn finite_float(text: &str) -> Option<f64> {
    if !is_yaml_style_float(text) {
        return None;
    }
    text.parse::<f64>().ok().filter(|number| number.is_finite())
}

/// The syntax of Go's `strconv.ParseInt(text, 0, 64)` for text without
/// underscores: an optional sign, then `0x`, `0o`, `0b` (any case), a
/// leading `0` (octal) or decimal digits. `None` is a syntax error;
/// otherwise the magnitude, `None` inside when it does not fit 128 bits.
fn go_base0_magnitude(text: &str) -> Option<Option<u128>> {
    let unsigned = text.strip_prefix(['+', '-']).unwrap_or(text);
    let (radix, digits) = match unsigned.as_bytes() {
        [b'0', b'x' | b'X', ..] => (16, &unsigned[2..]),
        [b'0', b'o' | b'O', ..] => (8, &unsigned[2..]),
        [b'0', b'b' | b'B', ..] => (2, &unsigned[2..]),
        [b'0', _, ..] => (8, &unsigned[1..]),
        _ => (10, unsigned),
    };
    if digits.is_empty() || !digits.chars().all(|ch| ch.is_digit(radix)) {
        return None;
    }
    let mut total = Some(0u128);
    for ch in digits.chars() {
        total = total
            .and_then(|total| total.checked_mul(u128::from(radix)))
            .and_then(|total| total.checked_add(u128::from(ch.to_digit(radix)?)));
    }
    Some(total)
}

/// yaml.v3's `yamlStyleFloat`:
/// `^[-+]?(\.[0-9]+|[0-9]+(\.[0-9]*)?)([eE][-+]?[0-9]+)?$`.
fn is_yaml_style_float(text: &str) -> bool {
    let bytes = text.as_bytes();
    let digits_from = |at: usize| {
        bytes[at.min(bytes.len())..]
            .iter()
            .take_while(|byte| byte.is_ascii_digit())
            .count()
    };
    let mut at = usize::from(matches!(bytes.first(), Some(b'+' | b'-')));
    if bytes.get(at) == Some(&b'.') {
        let fraction = digits_from(at + 1);
        if fraction == 0 {
            return false;
        }
        at += 1 + fraction;
    } else {
        let whole = digits_from(at);
        if whole == 0 {
            return false;
        }
        at += whole;
        if bytes.get(at) == Some(&b'.') {
            at += 1 + digits_from(at + 1);
        }
    }
    if matches!(bytes.get(at), Some(b'e' | b'E')) {
        at += 1;
        if matches!(bytes.get(at), Some(b'+' | b'-')) {
            at += 1;
        }
        let exponent = digits_from(at);
        if exponent == 0 {
            return false;
        }
        at += exponent;
    }
    at == bytes.len()
}

/// A tag as yaml.v3 writes it in full, or `None` for no tag and the
/// non-specific `!`, which yaml.v3 treats as no tag (`parser.node`,
/// decode.go:170-178).
fn explicit_tag(tag: Option<&Tag>) -> Option<String> {
    let tag = tag?;
    let full = format!("{}{}", tag.handle, tag.suffix);
    (full != "!").then_some(full)
}

/// How a scalar event decodes: its value, its tag for `isStringMap`, and
/// whether it is a merge key (`isMerge`, decode.go:998-1000). A tag other
/// than `!!str`, `!!int`, `!!float`, `!!bool` and `!!null` (and `!!merge`
/// on `<<`), a value its tag does not fit, and an untagged plain value that
/// looks like a timestamp are refused.
fn scalar(value: &str, style: TScalarStyle, tag: Option<&Tag>) -> Result<(Resolved, KeyTag, bool)> {
    if let Some(tag) = explicit_tag(tag) {
        let short = tag
            .strip_prefix(LONG_TAG_PREFIX)
            .ok_or_else(|| yaml_v3_error(format_args!("the tag {tag:?}")))?;
        let wanted = match short {
            "str" => return Ok((Resolved::Str, KeyTag::Str, false)),
            "merge" if value == "<<" => return Ok((Resolved::Str, KeyTag::Merge, true)),
            "int" | "float" | "bool" | "null" => short,
            _ => return Err(yaml_v3_error(format_args!("the tag !!{short}"))),
        };
        // `resolve` with the tag, and its check that the value fits
        // (resolve.go:131-148).
        let resolved = resolve(value, false).map_err(yaml_v3_error)?;
        let resolved = match (wanted, resolved) {
            ("int", resolved @ (Resolved::Int(_) | Resolved::Uint(_)))
            | ("float", resolved @ Resolved::Float(_))
            | ("bool", resolved @ Resolved::Bool(_))
            | ("null", resolved @ Resolved::Null) => resolved,
            ("float", Resolved::Int(number)) => Resolved::Float(number as f64),
            _ => {
                return Err(yaml_v3_error(format_args!(
                    "the value {value:?} does not fit its tag !!{wanted}"
                )))
            }
        };
        return Ok((resolved, KeyTag::Other, false));
    }
    if style != TScalarStyle::Plain {
        return Ok((Resolved::Str, KeyTag::Str, false));
    }
    if value == "<<" {
        return Ok((Resolved::Str, KeyTag::Merge, true));
    }
    let resolved = resolve(value, true).map_err(yaml_v3_error)?;
    let key_tag = if resolved == Resolved::Str {
        KeyTag::Str
    } else {
        KeyTag::Other
    };
    Ok((resolved, key_tag, false))
}

// ---------------------------------------------------------------------------
// The arena
// ---------------------------------------------------------------------------

/// One node of the first document. An alias refers to its anchored node by
/// index and is never copied.
struct Node {
    kind: Kind,
    /// The 1-based line of the event.
    line: usize,
}

enum Kind {
    Scalar {
        /// The scalar's text after unquoting.
        text: String,
        resolved: Resolved,
        key_tag: KeyTag,
        merge: bool,
    },
    Sequence(Vec<usize>),
    /// Keys and values, alternating, in document order.
    Mapping(Vec<usize>),
    /// `target` is the anchored node; `ordinal` counts the aliases of the
    /// document from 0, in order, and indexes [`Document::alias_names`].
    Alias {
        target: usize,
        ordinal: usize,
    },
}

/// What yaml.v3 compares to find a repeated mapping key: the key node's kind
/// and value (decode.go:801-814). An alias's value is its anchor name; a
/// collection's value is empty.
#[derive(PartialEq, Eq, Hash)]
enum KeyId<'a> {
    Scalar(&'a str),
    Alias(&'a str),
    Sequence,
    Mapping,
}

/// The first document of a YAML stream as an arena.
struct Document {
    nodes: Vec<Node>,
    root: Option<usize>,
    /// The anchor name of each alias, in document order.
    alias_names: Vec<String>,
    /// Mappings with a repeated key, with a description of the first.
    repeated: HashMap<usize, String>,
}

impl Document {
    /// Reads the first document of `text`, pulling parser events one at a
    /// time, and stops at its `DocumentEnd`. Also returns the parser, placed
    /// after that event, unless the stream ended first.
    fn read(text: &str) -> Result<(Document, Option<Parser<std::str::Chars<'_>>>)> {
        let mut document = Document {
            nodes: Vec::new(),
            root: None,
            alias_names: Vec::new(),
            repeated: HashMap::new(),
        };
        let mut parser = Parser::new_from_str(text);
        let mut open: Vec<usize> = Vec::new();
        let mut anchors: HashMap<usize, usize> = HashMap::new();
        let mut aliases = 0;
        let (end, more) = loop {
            let (event, mark) = parser.next_token().map_err(yaml_v3_error)?;
            let line = mark.line();
            let (kind, anchor) = match event {
                Event::StreamEnd => break (mark.index(), false),
                Event::DocumentEnd => break (mark.index(), true),
                Event::StreamStart | Event::DocumentStart | Event::Nothing => continue,
                Event::SequenceEnd | Event::MappingEnd => {
                    open.pop();
                    continue;
                }
                Event::Scalar(value, style, anchor, tag) => {
                    let (resolved, key_tag, merge) = scalar(&value, style, tag.as_ref())?;
                    let kind = Kind::Scalar {
                        text: value,
                        resolved,
                        key_tag,
                        merge,
                    };
                    (kind, anchor)
                }
                Event::SequenceStart(anchor, tag) => {
                    collection_tag(tag.as_ref(), "seq")?;
                    (Kind::Sequence(Vec::new()), anchor)
                }
                Event::MappingStart(anchor, tag) => {
                    collection_tag(tag.as_ref(), "map")?;
                    (Kind::Mapping(Vec::new()), anchor)
                }
                Event::Alias(anchor) => {
                    // yaml-rust2 refuses an unknown anchor before this event.
                    let target = *anchors
                        .get(&anchor)
                        .ok_or_else(|| yaml_v3_error("an unknown anchor"))?;
                    let kind = Kind::Alias {
                        target,
                        ordinal: aliases,
                    };
                    aliases += 1;
                    (kind, 0)
                }
            };
            let opens = matches!(kind, Kind::Sequence(_) | Kind::Mapping(_));
            let index = document.nodes.len();
            document.nodes.push(Node { kind, line });
            // yaml.v3 and yaml-rust2 both register an anchor when its node
            // starts, so an alias inside the node can refer to it.
            if anchor > 0 {
                anchors.insert(anchor, index);
            }
            match open.last() {
                Some(&parent) => {
                    if let Kind::Sequence(children) | Kind::Mapping(children) =
                        &mut document.nodes[parent].kind
                    {
                        children.push(index);
                    }
                }
                None => document.root = Some(index),
            }
            if opens {
                open.push(index);
            }
        };
        document.alias_names = anchor_names(text, end)?;
        if document.alias_names.len() != aliases {
            return Err(yaml_v3_error("aliases that cannot be named"));
        }
        document.find_repeated_keys();
        Ok((document, more.then_some(parser)))
    }

    /// Records, for each mapping with a repeated key, the first repetition
    /// yaml.v3 reports (decode.go:801-814).
    fn find_repeated_keys(&mut self) {
        let mut repeated = HashMap::new();
        for (index, node) in self.nodes.iter().enumerate() {
            let Kind::Mapping(children) = &node.kind else {
                continue;
            };
            let mut seen: HashMap<KeyId<'_>, usize> = HashMap::new();
            for &key in children.iter().step_by(2) {
                let id = match &self.nodes[key].kind {
                    Kind::Scalar { text, .. } => KeyId::Scalar(text),
                    Kind::Alias { ordinal, .. } => KeyId::Alias(&self.alias_names[*ordinal]),
                    Kind::Sequence(_) => KeyId::Sequence,
                    Kind::Mapping(_) => KeyId::Mapping,
                };
                let line = self.nodes[key].line;
                if let Some(first) = seen.get(&id) {
                    let name = match id {
                        KeyId::Scalar(text) | KeyId::Alias(text) => text,
                        KeyId::Sequence | KeyId::Mapping => "",
                    };
                    repeated.insert(
                        index,
                        format!(
                            "line {line}: mapping key {name:?} already defined at line {first}"
                        ),
                    );
                    break;
                }
                seen.insert(id, line);
            }
        }
        self.repeated = repeated;
    }

    /// The node an alias refers to, or the node itself.
    fn target(&self, node: usize) -> usize {
        match self.nodes[node].kind {
            Kind::Alias { target, .. } => target,
            _ => node,
        }
    }

    fn is_mapping(&self, node: usize) -> bool {
        matches!(self.nodes[node].kind, Kind::Mapping(_))
    }

    fn is_merge(&self, node: usize) -> bool {
        matches!(self.nodes[node].kind, Kind::Scalar { merge: true, .. })
    }

    /// A key's tag for `isStringMap`; an alias has its target's.
    fn key_tag(&self, key: usize) -> KeyTag {
        match self.nodes[self.target(key)].kind {
            Kind::Scalar { key_tag, .. } => key_tag,
            _ => KeyTag::Other,
        }
    }

    /// yaml.v3's `isStringMap` (decode.go:877-889): every key is a string or
    /// a merge key, so the mapping decodes into `map[string]any`.
    fn is_string_map(&self, mapping: usize) -> bool {
        match &self.nodes[mapping].kind {
            Kind::Mapping(children) => children
                .iter()
                .step_by(2)
                .all(|&key| self.key_tag(key) != KeyTag::Other),
            _ => false,
        }
    }
}

/// Refuses a tag on a sequence or mapping other than its own (`!!seq`,
/// `!!map`) or the non-specific `!`.
fn collection_tag(tag: Option<&Tag>, own: &str) -> Result<()> {
    match explicit_tag(tag) {
        None => Ok(()),
        Some(tag) if tag.strip_prefix(LONG_TAG_PREFIX) == Some(own) => Ok(()),
        Some(tag) => Err(yaml_v3_error(format_args!("the tag {tag:?} on a !!{own}"))),
    }
}

/// The names of the aliases before char index `end` (the first document),
/// in order, read with a second yaml-rust2 scanner over the same text.
/// Anchor and alias names must use yaml.v3's characters (`0-9A-Za-z_-`,
/// `yaml_parser_scan_anchor`); yaml-rust2 accepts more, such as `:`.
fn anchor_names(text: &str, end: usize) -> Result<Vec<String>> {
    let mut names = Vec::new();
    // The scanner stops at an error; the parser already met any error
    // inside the first document.
    for token in Scanner::new(text.chars()) {
        if token.0.index() >= end {
            break;
        }
        let (TokenType::Anchor(name) | TokenType::Alias(name)) = &token.1 else {
            continue;
        };
        if !name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
        {
            return Err(yaml_v3_error(format_args!("the anchor name {name:?}")));
        }
        if let TokenType::Alias(name) = token.1 {
            names.push(name);
        }
    }
    Ok(names)
}

// ---------------------------------------------------------------------------
// The walk
// ---------------------------------------------------------------------------

#[cfg(test)]
thread_local! {
    /// The number of [`TreeValue`] nodes this thread's reader has built.
    static VALUES_BUILT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Counts a built value in tests.
fn built(value: TreeValue) -> TreeValue {
    #[cfg(test)]
    VALUES_BUILT.with(|count| count.set(count.get() + 1));
    value
}

/// One step of yaml.v3's decode of the document (`decoder.unmarshal` and the
/// functions it calls), kept on an explicit stack.
enum Step {
    /// `unmarshal(node, out)` with `out` an `interface{}`; the value is
    /// level `depth` and is built when `build`.
    Value {
        node: usize,
        depth: usize,
        build: bool,
    },
    /// The end of `decoder.alias` for this alias node.
    LeaveAlias(usize),
    /// The end of a sequence that is being built.
    CloseSequence,
    /// The end of a mapping; its value is handed on when `build`.
    CloseMapping { build: bool },
    /// One key and value of a mapping that decodes into `map[string]any`
    /// (`decoder.mapping`, decode.go:836-856), or of a mapping merged into
    /// one. The value is level `depth + 1`.
    StringPair {
        key: usize,
        value: usize,
        depth: usize,
        build: bool,
    },
    /// One key and value of a mapping that decodes into
    /// `map[interface{}]interface{}`, which is never built.
    AnyPair {
        key: usize,
        value: usize,
        depth: usize,
    },
    /// The `invalid map key` check after a key of such a mapping is decoded
    /// (decode.go:847-853).
    KeyCheck(usize),
    /// `decoder.merge` for `mapping` and the value of its merge key
    /// (decode.go:938-996).
    Merge {
        mapping: usize,
        value: usize,
        depth: usize,
        build: bool,
    },
    /// One element of a merged sequence (decode.go:980-988).
    MergeElement {
        node: usize,
        depth: usize,
        build: bool,
    },
    /// `unmarshal` of a mapping merged into the mapping being decoded.
    MergeMapping {
        node: usize,
        depth: usize,
        build: bool,
    },
}

/// A collection being decoded.
enum Frame {
    /// A sequence being built.
    Sequence(Vec<TreeValue>),
    Mapping(MappingFrame),
}

/// A mapping being decoded, with what yaml.v3 tracks for it.
struct MappingFrame {
    /// It decodes into `map[string]any`; otherwise into
    /// `map[interface{}]interface{}`, which is not built.
    string_keys: bool,
    entries: Vec<(String, TreeValue)>,
    /// The keys decoded so far, outside a merge.
    seen: HashSet<String>,
    /// yaml.v3's `mergedFields` while a merge into this mapping runs
    /// (decode.go:938-948): a merged key already present is skipped.
    merged: Option<HashSet<String>>,
    /// The key whose value is being decoded.
    key: Option<String>,
}

/// One decode of the document, with yaml.v3's counters.
struct Walk<'d> {
    document: &'d Document,
    build: bool,
    decode_count: u64,
    alias_count: u64,
    alias_depth: u64,
    /// Alias nodes being expanded (`decoder.aliases`).
    active: HashSet<usize>,
    /// The first refusal met where yaml.v3 records a type error, or where Go
    /// goes on but this port refuses, and the decode continues: yaml.v3 still
    /// fails at once on an alias error met later.
    refusal: Option<Error>,
    frames: Vec<Frame>,
    steps: Vec<Step>,
    result: Option<TreeValue>,
}

impl<'d> Walk<'d> {
    fn new(document: &'d Document, build: bool) -> Self {
        Walk {
            document,
            build,
            // The document node is the first `unmarshal` (decode.go:524-531).
            decode_count: 1,
            alias_count: 0,
            alias_depth: 0,
            active: HashSet::new(),
            refusal: None,
            frames: Vec::new(),
            steps: Vec::new(),
            result: None,
        }
    }

    fn run_value(mut self, root: usize) -> Result<Option<TreeValue>> {
        self.steps.push(Step::Value {
            node: root,
            depth: 1,
            build: self.build,
        });
        while let Some(step) = self.steps.pop() {
            self.step(step)?;
        }
        match self.refusal {
            Some(error) => Err(error),
            None => Ok(self.result),
        }
    }

    /// Decodes the document's root into a `map[string]any`.
    fn run(mut self, root: usize) -> Result<Option<TreeValue>> {
        self.count()?;
        match &self.document.nodes[root].kind {
            Kind::Mapping(_) => {
                if let Some(repeated) = self.document.repeated.get(&root) {
                    return Err(yaml_v3_error(repeated));
                }
                // The root decodes into `map[string]any` whatever its keys:
                // a key that is not a string is decoded as its text
                // (decode.go:609-614).
                self.frames.push(Frame::Mapping(MappingFrame {
                    string_keys: true,
                    entries: Vec::new(),
                    seen: HashSet::new(),
                    merged: None,
                    key: None,
                }));
                self.steps.push(Step::CloseMapping { build: self.build });
                self.push_pairs(root, 1, self.build, true);
            }
            Kind::Scalar {
                resolved: Resolved::Null,
                ..
            } => return Ok(None),
            Kind::Scalar { .. } | Kind::Sequence(_) | Kind::Alias { .. } => {
                return Err(yaml_v3_error("a top level that is not a mapping"))
            }
        }
        while let Some(step) = self.steps.pop() {
            self.step(step)?;
        }
        match self.refusal {
            Some(refusal) => Err(refusal),
            None => Ok(self.result),
        }
    }

    /// yaml.v3's counters and alias rule at the start of every `unmarshal`
    /// (decode.go:484-491).
    fn count(&mut self) -> Result<()> {
        self.decode_count += 1;
        if self.alias_depth > 0 {
            self.alias_count += 1;
        }
        if self.alias_count > 100
            && self.decode_count > 1000
            && self.alias_count as f64 / self.decode_count as f64
                > allowed_alias_ratio(self.decode_count)
        {
            return Err(Error::Command {
                message: EXCESSIVE_ALIASING.to_owned(),
            });
        }
        Ok(())
    }

    /// The start of `decoder.alias` (decode.go:533-540).
    fn enter_alias(&mut self, alias: usize) -> Result<()> {
        if !self.active.insert(alias) {
            let name = match self.document.nodes[alias].kind {
                Kind::Alias { ordinal, .. } => self.document.alias_names[ordinal].as_str(),
                _ => "",
            };
            return Err(Error::Command {
                message: format!("yaml: anchor '{name}' value contains itself"),
            });
        }
        self.alias_depth += 1;
        Ok(())
    }

    fn leave_alias(&mut self, alias: usize) {
        self.alias_depth -= 1;
        self.active.remove(&alias);
    }

    /// Keeps the first refusal and goes on decoding.
    fn refuse(&mut self, error: Error) {
        self.refusal.get_or_insert(error);
    }

    /// The mapping a pair or merge step decodes into. Every mapping step is
    /// pushed with its frame, so a missing one is a defect of this reader,
    /// reported as an error rather than a panic.
    fn mapping_frame(&mut self) -> Result<&mut MappingFrame> {
        match self.frames.last_mut() {
            Some(Frame::Mapping(frame)) => Ok(frame),
            _ => Err(not_read(
                "a mapping step outside a mapping (a defect of this reader)",
            )),
        }
    }

    /// Hands a built value to the collection being built, or makes it the
    /// result.
    fn deliver(&mut self, value: TreeValue) {
        match self.frames.last_mut() {
            Some(Frame::Sequence(items)) => items.push(value),
            Some(Frame::Mapping(frame)) => {
                if let Some(key) = frame.key.take() {
                    frame.entries.push((key, value));
                }
            }
            None => self.result = Some(value),
        }
    }

    /// Pushes the steps of `decoder.mapping` for the pairs of `mapping`,
    /// level `depth`: every pair but the merge key, in order, then the merge
    /// (decode.go:836-862). Two merge keys are a repeated key.
    fn push_pairs(&mut self, mapping: usize, depth: usize, build: bool, string_keys: bool) {
        let document = self.document;
        let Kind::Mapping(children) = &document.nodes[mapping].kind else {
            return;
        };
        let mut next = Vec::new();
        let mut merge = None;
        for pair in children.chunks(2) {
            let (key, Some(&value)) = (pair[0], pair.get(1)) else {
                continue;
            };
            if document.is_merge(key) {
                merge = Some(value);
            } else if string_keys {
                next.push(Step::StringPair {
                    key,
                    value,
                    depth,
                    build,
                });
            } else {
                next.push(Step::AnyPair { key, value, depth });
            }
        }
        if let Some(value) = merge {
            if string_keys {
                next.push(Step::Merge {
                    mapping,
                    value,
                    depth,
                    build,
                });
            } else {
                self.refuse(not_read(
                    "a merge key in a mapping with a key that is not a string",
                ));
            }
        }
        self.steps.extend(next.into_iter().rev());
    }

    fn step(&mut self, step: Step) -> Result<()> {
        match step {
            Step::Value { node, depth, build } => self.value(node, depth, build)?,
            Step::LeaveAlias(alias) => self.leave_alias(alias),
            Step::CloseSequence => {
                if let Some(Frame::Sequence(items)) = self.frames.pop() {
                    self.deliver(built(TreeValue::Seq(items)));
                }
            }
            Step::CloseMapping { build } => {
                if let Some(Frame::Mapping(frame)) = self.frames.pop() {
                    if build {
                        let value = if frame.string_keys {
                            TreeValue::Map(frame.entries)
                        } else {
                            TreeValue::Other(ANY_KEY_MAP)
                        };
                        self.deliver(built(value));
                    }
                }
            }
            Step::StringPair {
                key,
                value,
                depth,
                build,
            } => {
                let Some(text) = self.string_key(key)? else {
                    return Ok(());
                };
                let frame = self.mapping_frame()?;
                let repeated = match &mut frame.merged {
                    // A key already present is not merged (decode.go:840-845).
                    Some(merged) => {
                        if !merged.insert(text.clone()) {
                            return Ok(());
                        }
                        false
                    }
                    None => !frame.seen.insert(text.clone()),
                };
                if repeated {
                    // Two keys yaml.v3 does not see as repeated (an alias
                    // and a scalar) decode to the same string, and Go keeps
                    // only the later value.
                    self.refuse(not_read(format_args!(
                        "the mapping key {text:?} twice after decoding, where Go keeps only the later value"
                    )));
                }
                self.mapping_frame()?.key = Some(text);
                self.steps.push(Step::Value {
                    node: value,
                    depth: depth + 1,
                    build,
                });
            }
            Step::AnyPair { key, value, depth } => {
                self.steps.push(Step::Value {
                    node: value,
                    depth: depth + 1,
                    build: false,
                });
                self.steps.push(Step::KeyCheck(key));
                self.steps.push(Step::Value {
                    node: key,
                    depth: depth + 1,
                    build: false,
                });
            }
            Step::KeyCheck(key) => {
                let target = self.document.target(key);
                if matches!(
                    self.document.nodes[target].kind,
                    Kind::Sequence(_) | Kind::Mapping(_)
                ) {
                    return Err(yaml_v3_error("invalid map key"));
                }
            }
            Step::Merge {
                mapping,
                value,
                depth,
                build,
            } => self.merge(mapping, value, depth, build)?,
            Step::MergeElement { node, depth, build } => {
                if !self.document.is_mapping(self.document.target(node)) {
                    return Err(want_map());
                }
                self.merge_mapping_or_alias(node, depth, build)?;
            }
            Step::MergeMapping { node, depth, build } => {
                self.count()?;
                if let Some(repeated) = self.document.repeated.get(&node) {
                    self.refuse(yaml_v3_error(repeated));
                    return Ok(());
                }
                self.push_pairs(node, depth, build, true);
            }
        }
        Ok(())
    }

    /// `unmarshal(node, out)` with `out` an `interface{}`.
    fn value(&mut self, node: usize, depth: usize, build: bool) -> Result<()> {
        self.count()?;
        if depth > MAX_DEPTH {
            return Err(not_read(format_args!(
                "YAML nested deeper than {MAX_DEPTH} levels (aliases expanded)"
            )));
        }
        let document = self.document;
        match &document.nodes[node].kind {
            Kind::Alias { target, .. } => {
                self.enter_alias(node)?;
                self.steps.push(Step::LeaveAlias(node));
                self.steps.push(Step::Value {
                    node: *target,
                    depth,
                    build,
                });
            }
            Kind::Scalar { text, resolved, .. } => {
                if build {
                    let value = match resolved {
                        Resolved::Null => TreeValue::Null,
                        Resolved::Bool(value) => TreeValue::Bool(*value),
                        Resolved::Int(value) => TreeValue::Int(*value),
                        Resolved::Uint(value) => TreeValue::Uint(*value),
                        Resolved::Float(value) => TreeValue::Float(*value),
                        Resolved::Str => TreeValue::Str(text.clone()),
                    };
                    self.deliver(built(value));
                }
            }
            Kind::Sequence(children) => {
                if build {
                    self.frames.push(Frame::Sequence(Vec::new()));
                    self.steps.push(Step::CloseSequence);
                }
                self.steps
                    .extend(children.iter().rev().map(|&child| Step::Value {
                        node: child,
                        depth: depth + 1,
                        build,
                    }));
            }
            Kind::Mapping(_) => {
                if let Some(repeated) = document.repeated.get(&node) {
                    // yaml.v3 records the error and decodes nothing of the
                    // mapping (decode.go:801-814).
                    self.refuse(yaml_v3_error(repeated));
                    return Ok(());
                }
                let string_keys = document.is_string_map(node);
                self.frames.push(Frame::Mapping(MappingFrame {
                    string_keys,
                    entries: Vec::new(),
                    seen: HashSet::new(),
                    merged: None,
                    key: None,
                }));
                self.steps.push(Step::CloseMapping { build });
                self.push_pairs(node, depth, build && string_keys, string_keys);
            }
        }
        Ok(())
    }

    /// `unmarshal(key, out)` with `out` a string: the key's text, or `None`
    /// when the pair is not decoded (a null key, which yaml.v3 skips without
    /// an error and this port refuses, or a collection, a type error).
    fn string_key(&mut self, key: usize) -> Result<Option<String>> {
        self.count()?;
        let document = self.document;
        let target = match document.nodes[key].kind {
            Kind::Alias { target, .. } => {
                self.enter_alias(key)?;
                let counted = self.count();
                self.leave_alias(key);
                counted?;
                target
            }
            _ => key,
        };
        match &document.nodes[target].kind {
            Kind::Scalar {
                resolved: Resolved::Null,
                ..
            } => {
                self.refuse(not_read(
                    "a null mapping key, whose pair Go skips without a message",
                ));
                Ok(None)
            }
            Kind::Scalar { text, .. } => Ok(Some(text.clone())),
            Kind::Sequence(_) | Kind::Mapping(_) | Kind::Alias { .. } => {
                self.refuse(yaml_v3_error("a mapping key that is not a scalar"));
                Ok(None)
            }
        }
    }

    /// `decoder.merge` (decode.go:938-996) of `value` into the mapping being
    /// decoded, `mapping`'s.
    fn merge(&mut self, mapping: usize, value: usize, depth: usize, build: bool) -> Result<()> {
        let document = self.document;
        if self.mapping_frame()?.merged.is_none() {
            let Kind::Mapping(children) = &document.nodes[mapping].kind else {
                return Ok(());
            };
            // Outside a merge, every key of the mapping is decoded once more
            // into `interface{}` and marked as present. A key that is not a
            // string would be marked as another Go type than the merged
            // keys, which are decoded as strings; that is not ported.
            if children
                .iter()
                .step_by(2)
                .any(|&key| document.key_tag(key) == KeyTag::Other)
            {
                self.refuse(not_read(
                    "a merge key in a mapping with a key that is not a string",
                ));
                return Ok(());
            }
            let mut merged = HashSet::new();
            for &key in children.iter().step_by(2) {
                if let Some(text) = self.string_key(key)? {
                    merged.insert(text);
                }
            }
            self.mapping_frame()?.merged = Some(merged);
        }
        match &document.nodes[value].kind {
            Kind::Mapping(_) => self.merge_mapping_or_alias(value, depth, build)?,
            Kind::Alias { target, .. } if document.is_mapping(*target) => {
                self.merge_mapping_or_alias(value, depth, build)?;
            }
            Kind::Sequence(elements) => {
                self.steps
                    .extend(elements.iter().rev().map(|&node| Step::MergeElement {
                        node,
                        depth,
                        build,
                    }));
            }
            Kind::Alias { .. } | Kind::Scalar { .. } => return Err(want_map()),
        }
        Ok(())
    }

    /// `unmarshal` of a merged mapping, or of an alias of one.
    fn merge_mapping_or_alias(&mut self, node: usize, depth: usize, build: bool) -> Result<()> {
        match self.document.nodes[node].kind {
            Kind::Alias { target, .. } => {
                self.count()?;
                self.enter_alias(node)?;
                self.steps.push(Step::LeaveAlias(node));
                self.steps.push(Step::MergeMapping {
                    node: target,
                    depth,
                    build,
                });
            }
            _ => self.steps.push(Step::MergeMapping { node, depth, build }),
        }
        Ok(())
    }
}

/// yaml.v3's `failWantMap` (decode.go:917-919).
fn want_map() -> Error {
    yaml_v3_error("map merge requires map or sequence of maps as the value")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The nine-level "billion laughs" source text of luuna-beacon's test
    /// (about 400 bytes). Fully expanded it would be 9^9 nodes; only the
    /// reader under test may expand it, and it must stop first.
    const BILLION_LAUGHS: &str = concat!(
        "project: demo\n",
        "a: &a [\"lol\",\"lol\",\"lol\",\"lol\",\"lol\",\"lol\",\"lol\",\"lol\",\"lol\"]\n",
        "b: &b [*a,*a,*a,*a,*a,*a,*a,*a,*a]\n",
        "c: &c [*b,*b,*b,*b,*b,*b,*b,*b,*b]\n",
        "d: &d [*c,*c,*c,*c,*c,*c,*c,*c,*c]\n",
        "e: &e [*d,*d,*d,*d,*d,*d,*d,*d,*d]\n",
        "f: &f [*e,*e,*e,*e,*e,*e,*e,*e,*e]\n",
        "g: &g [*f,*f,*f,*f,*f,*f,*f,*f,*f]\n",
        "h: &h [*g,*g,*g,*g,*g,*g,*g,*g,*g]\n",
        "i: &i [*h,*h,*h,*h,*h,*h,*h,*h,*h]\n",
    );

    /// The same expansion with mappings, seven levels of ten (about 600
    /// bytes): fully expanded it would be 10^7 mappings.
    const NESTED_MAPPINGS: &str = concat!(
        "project: demo\n",
        "a: &a {k0: lol, k1: lol, k2: lol, k3: lol, k4: lol, k5: lol, k6: lol, k7: lol, k8: lol, k9: lol}\n",
        "b: &b {k0: *a, k1: *a, k2: *a, k3: *a, k4: *a, k5: *a, k6: *a, k7: *a, k8: *a, k9: *a}\n",
        "c: &c {k0: *b, k1: *b, k2: *b, k3: *b, k4: *b, k5: *b, k6: *b, k7: *b, k8: *b, k9: *b}\n",
        "d: &d {k0: *c, k1: *c, k2: *c, k3: *c, k4: *c, k5: *c, k6: *c, k7: *c, k8: *c, k9: *c}\n",
        "e: &e {k0: *d, k1: *d, k2: *d, k3: *d, k4: *d, k5: *d, k6: *d, k7: *d, k8: *d, k9: *d}\n",
        "f: &f {k0: *e, k1: *e, k2: *e, k3: *e, k4: *e, k5: *e, k6: *e, k7: *e, k8: *e, k9: *e}\n",
        "g: &g {k0: *f, k1: *f, k2: *f, k3: *f, k4: *f, k5: *f, k6: *f, k7: *f, k8: *f, k9: *f}\n",
    );

    /// An anchored sequence that holds an alias of itself.
    const SELF_ALIAS: &str = "project: demo\nloop: &self [a, *self]\n";

    fn decoded(text: &str) -> TreeValue {
        decode_document(text.as_bytes())
            .unwrap_or_else(|error| panic!("{text:?}: {error}"))
            .unwrap_or_else(|| panic!("{text:?}: no document"))
    }

    fn command_error(text: &str) -> String {
        match decode_document(text.as_bytes()) {
            Err(Error::Command { message }) => message,
            other => panic!("{text:?}: expected a command error, got {other:?}"),
        }
    }

    fn unsupported(text: &[u8]) -> String {
        match decode_document(text) {
            Err(Error::Unsupported { feature, plan }) => {
                assert_eq!(plan, YAML_ERROR_TEXTS_NOT_PORTED);
                feature
            }
            other => panic!(
                "{:?}: expected Unsupported, got {other:?}",
                String::from_utf8_lossy(text)
            ),
        }
    }

    fn str(text: &str) -> TreeValue {
        TreeValue::Str(text.to_owned())
    }

    fn map(entries: &[(&str, TreeValue)]) -> TreeValue {
        TreeValue::Map(
            entries
                .iter()
                .map(|(key, value)| ((*key).to_owned(), value.clone()))
                .collect(),
        )
    }

    /// The value of `key` in a decoded root mapping.
    fn field(root: &TreeValue, key: &str) -> TreeValue {
        let TreeValue::Map(entries) = root else {
            panic!("not a mapping: {root:?}");
        };
        entries
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.clone())
            .unwrap_or_else(|| panic!("no {key:?} in {root:?}"))
    }

    /// `x: &x [a, ...]` with `items` scalars, then `y:` a sequence of
    /// `aliases` aliases of it. yaml.v3 counts 8 + items + aliases *
    /// (items + 2) decodes, aliases * (items + 1) of them under an alias.
    fn repeated_list(items: usize, aliases: usize) -> String {
        format!(
            "project: demo\nx: &x [{}a]\ny: [{}*x]\n",
            "a, ".repeat(items - 1),
            "*x, ".repeat(aliases - 1)
        )
    }

    #[test]
    fn billion_laughs_is_refused_like_go() {
        // Go's tool refuses this file with this message (Ba3,
        // go-first-1.txt:142).
        let started = std::time::Instant::now();
        assert_eq!(command_error(BILLION_LAUGHS), EXCESSIVE_ALIASING);
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "took {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn excessive_aliasing_is_refused_before_any_value_is_built() {
        for text in [BILLION_LAUGHS, NESTED_MAPPINGS] {
            VALUES_BUILT.with(|count| count.set(0));
            assert_eq!(command_error(text), EXCESSIVE_ALIASING);
            assert_eq!(VALUES_BUILT.with(|count| count.get()), 0, "{text}");
        }
        // The counter does count what a successful decode builds: the root,
        // `project` and its value.
        VALUES_BUILT.with(|count| count.set(0));
        decoded("project: demo\n");
        assert_eq!(VALUES_BUILT.with(|count| count.get()), 2);
    }

    #[test]
    fn an_alias_inside_its_own_anchor_is_refused_like_go() {
        assert_eq!(
            command_error(SELF_ALIAS),
            "yaml: anchor 'self' value contains itself"
        );
        assert_eq!(
            command_error("project: demo\nm: &me {a: 1, b: *me}\n"),
            "yaml: anchor 'me' value contains itself"
        );
    }

    #[test]
    fn aliasing_at_go_s_limit_is_counted_like_go() {
        // Inputs of at most 8 KiB (about 3.5 KB here), built by repeating
        // literals. 1000 items: 110 aliases give 110110 of 111228 decodes
        // under an alias (0.98995, allowed); 111 give 111111 of 112230
        // (0.99003, refused). Go's tool gives the same answers for these two
        // inputs (luuna-beacon's test, config.rs:1230).
        let accepted = repeated_list(1000, 110);
        let refused = repeated_list(1000, 111);
        assert!(refused.len() <= 8 * 1024, "{}", refused.len());
        let root = decoded(&accepted);
        assert_eq!(field(&root, "project"), str("demo"));
        let TreeValue::Seq(lists) = field(&root, "y") else {
            panic!("y is not a sequence");
        };
        assert_eq!(lists.len(), 110);
        assert!(lists
            .iter()
            .all(|list| matches!(list, TreeValue::Seq(items) if items.len() == 1000)));
        assert_eq!(command_error(&refused), EXCESSIVE_ALIASING);
    }

    #[test]
    fn only_the_first_document_is_read_like_go() {
        assert_eq!(
            decoded("project: one\n---\nproject: two\n"),
            map(&[("project", str("one"))])
        );
        assert_eq!(
            decoded("--- \nproject: a\n...\n--- [x\n"),
            map(&[("project", str("a"))])
        );
        // An empty stream and a null first document leave Go's map nil.
        for text in [
            "",
            "# only a comment\n",
            "---\n",
            "~\n",
            "null\n---\nproject: b\n",
        ] {
            assert_eq!(decode_document(text.as_bytes()).unwrap(), None, "{text:?}");
        }
        for (text, later) in [
            ("project: one\n", false),
            ("project: one\n...\n", false),
            ("", false),
            ("project: one\n---\nproject: two\n", true),
            ("project: one\n---\n", true),
            ("project: one\n...\n[x\n", true),
        ] {
            assert_eq!(
                has_later_document(text.as_bytes()).unwrap(),
                later,
                "{text:?}"
            );
        }
    }

    #[test]
    fn a_syntax_error_in_a_later_document_is_ignored_like_go() {
        // Ba1 (go-first-1.txt:137-138): Go prints the first document's
        // config and ignores the broken second document.
        let first = map(&[("project", str("p")), ("version", str("1"))]);
        let ba1 = "project: p\nversion: \"1\"\n---\nproject: [unclosed\n";
        assert_eq!(decoded(ba1), first);
        assert!(has_later_document(ba1.as_bytes()).unwrap());
        // Ba2 (:139-141): the later document is the billion-laughs literal,
        // which is not decoded, so not refused.
        let ba2 = format!("project: p\nversion: \"1\"\n---\n{BILLION_LAUGHS}");
        assert_eq!(decoded(&ba2), first);
        assert!(has_later_document(ba2.as_bytes()).unwrap());
    }

    #[test]
    fn a_few_aliases_and_merge_keys_still_load() {
        let text = concat!(
            "base: &base {dir: agents, default: true}\n",
            "name: &name demo-a\n",
            "project: *name\n",
            "targets:\n",
            "    default: *base\n",
            "    other:\n",
            "        <<: *base\n",
            "        dir: other\n",
            "list: [*name, *name, *base]\n",
        );
        let base = map(&[("dir", str("agents")), ("default", TreeValue::Bool(true))]);
        assert_eq!(
            decoded(text),
            map(&[
                ("base", base.clone()),
                ("name", str("demo-a")),
                ("project", str("demo-a")),
                (
                    "targets",
                    map(&[
                        ("default", base.clone()),
                        // The mapping's own key wins; merged keys follow it.
                        (
                            "other",
                            map(&[("dir", str("other")), ("default", TreeValue::Bool(true))])
                        ),
                    ])
                ),
                (
                    "list",
                    TreeValue::Seq(vec![str("demo-a"), str("demo-a"), base])
                ),
            ])
        );
        // In a merged sequence the first mapping that has a key wins, and a
        // merged mapping's own merge comes after its keys (decode.go:959-996).
        assert_eq!(
            decoded("<<: [{project: first, a: 1}, {project: second, b: 2}]\n"),
            map(&[
                ("project", str("first")),
                ("a", TreeValue::Int(1)),
                ("b", TreeValue::Int(2)),
            ])
        );
        assert_eq!(
            decoded("i: &i {project: inner, v: i}\no: &o {<<: *i, v: o}\n<<: *o\n"),
            map(&[
                ("i", map(&[("project", str("inner")), ("v", str("i"))])),
                ("o", map(&[("v", str("o")), ("project", str("inner"))])),
                ("v", str("o")),
                ("project", str("inner")),
            ])
        );
        // A quoted `<<` is an ordinary key; `!!merge` makes any `<<` a merge.
        assert_eq!(decoded("\"<<\": x\n"), map(&[("<<", str("x"))]));
        assert_eq!(
            decoded("!!merge '<<': {a: 1}\n"),
            map(&[("a", TreeValue::Int(1))])
        );
        assert_eq!(
            field(&decoded(&repeated_list(9, 9)), "project"),
            str("demo")
        );
        // A merge value must be a mapping or a sequence of mappings.
        for text in [
            "project: demo\n<<: 1\n",
            "s: &s [a]\n<<: *s\n",
            "<<: [{a: 1}, x]\n",
        ] {
            let feature = unsupported(text.as_bytes());
            assert!(feature.contains("map merge requires"), "{feature}");
        }
    }

    #[test]
    fn go_s_alias_ratio_is_ported() {
        assert_eq!(allowed_alias_ratio(0), 0.99);
        assert_eq!(allowed_alias_ratio(400_000), 0.99);
        assert_eq!(allowed_alias_ratio(4_000_000), 0.10);
        assert_eq!(allowed_alias_ratio(9_000_000), 0.10);
        assert!((allowed_alias_ratio(2_200_000) - 0.545).abs() < 1e-12);
    }

    #[test]
    fn plain_scalars_resolve_as_yaml_v3_resolves_them() {
        let value = |text: &str| field(&decoded(&format!("v: {text}\n")), "v");
        for (text, expected) in [
            ("true", TreeValue::Bool(true)),
            ("FALSE", TreeValue::Bool(false)),
            ("~", TreeValue::Null),
            ("Null", TreeValue::Null),
            ("12", TreeValue::Int(12)),
            ("-0b101", TreeValue::Int(-5)),
            ("0x10", TreeValue::Int(16)),
            ("0O17", TreeValue::Int(15)),
            ("010", TreeValue::Int(8)),
            ("1_000", TreeValue::Int(1000)),
            ("-9223372036854775808", TreeValue::Int(i64::MIN)),
            ("9223372036854775808", TreeValue::Uint(1 << 63)),
            ("18446744073709551615", TreeValue::Uint(u64::MAX)),
            (
                "18446744073709551616",
                TreeValue::Float(18_446_744_073_709_551_616.0),
            ),
            ("08", TreeValue::Float(8.0)),
            ("1e3", TreeValue::Float(1000.0)),
            ("5.", TreeValue::Float(5.0)),
            (".5e1", TreeValue::Float(5.0)),
            ("-.inf", TreeValue::Float(f64::NEG_INFINITY)),
            ("yes", str("yes")),
            ("0x", str("0x")),
            ("1e400", str("1e400")),
            ("0x1FFFFFFFFFFFFFFFF", str("0x1FFFFFFFFFFFFFFFF")),
            // Out of range for ParseInt and signed for ParseUint, but a float.
            (
                "-18446744073709551616",
                TreeValue::Float(-18_446_744_073_709_551_616.0),
            ),
            ("-0xFFFFFFFFFFFFFFFFF", str("-0xFFFFFFFFFFFFFFFFF")),
            ("+-1", str("+-1")),
            ("a b", str("a b")),
        ] {
            assert_eq!(value(text), expected, "{text}");
        }
        assert!(matches!(value(".NaN"), TreeValue::Float(number) if number.is_nan()));
        // Quoted and block scalars are strings; tags give their own type.
        assert_eq!(value("'12'"), str("12"));
        assert_eq!(value("\"true\""), str("true"));
        assert_eq!(value("|\n  12\n"), str("12\n"));
        assert_eq!(value("!!str 12"), str("12"));
        assert_eq!(value("!!float 3"), TreeValue::Float(3.0));
        assert_eq!(value("!!int \"0x10\""), TreeValue::Int(16));
        assert_eq!(value("!!null ''"), TreeValue::Null);
        assert_eq!(value("! 12"), TreeValue::Int(12));
        assert_eq!(value("! '12'"), str("12"));
        assert_eq!(value("!!str 2024-01-01"), str("2024-01-01"));
        // Keys of the root are strings whatever they resolve to; a nested
        // mapping with such a key is what Go decodes into
        // map[interface {}]interface {}.
        assert_eq!(
            decoded("1: a\ntrue: b\n0x10: c\n"),
            map(&[("1", str("a")), ("true", str("b")), ("0x10", str("c"))])
        );
        assert_eq!(
            decoded("m: {1: a}\n"),
            map(&[("m", TreeValue::Other("map[interface {}]interface {}"))])
        );
    }

    #[test]
    fn yaml_rust2_errors_are_unsupported_naming_no_plan() {
        for text in [
            "project: [unclosed\n",
            "project: a: b\n",
            "\t\n  bad: [\n",
            "a: *nowhere\n",
            "a: &x:y 1\n",
            "a: \"\\q\"\n",
        ] {
            let feature = unsupported(text.as_bytes());
            assert!(
                feature.starts_with("yaml.v3's error for this input ("),
                "{text:?}: {feature}"
            );
        }
        // A top level that is not a mapping, invalid UTF-8, and characters
        // yaml.v3's reader refuses.
        for text in [
            &b"- a\n"[..],
            b"just a string\n",
            b"42\n",
            b"project: \xff\n",
            b"project: a\x01\n",
            b"\xef\xbb\xbfproject: a\n",
        ] {
            unsupported(text);
        }
        assert_eq!(
            YAML_ERROR_TEXTS_NOT_PORTED,
            "none (yaml.v3's error texts are not ported; see notes/s4-missions-lib-3/README.md)"
        );
        let error = decode_document(b"project: [unclosed\n").unwrap_err();
        let error = in_file(Path::new("config.yaml"), error);
        assert!(
            error
                .to_string()
                .starts_with("config.yaml: yaml.v3's error for this input ("),
            "{error}"
        );
    }

    #[test]
    fn repeated_keys_tags_and_timestamps_are_unsupported() {
        for (text, part) in [
            (
                "a: 1\na: 2\n",
                "line 2: mapping key \"a\" already defined at line 1",
            ),
            ("1: a\n\"1\": b\n", "mapping key \"1\" already defined"),
            ("m:\n  x: 1\n  x: 2\n", "line 3: mapping key \"x\""),
            ("<<: {a: 1}\n<<: {b: 2}\n", "mapping key \"<<\""),
            (
                "k: &k project\nproject: a\n*k : b\n",
                "\"project\" twice after decoding",
            ),
            ("a: !foo x\n", "the tag \"!foo\""),
            ("a: !!binary aGk=\n", "the tag !!binary"),
            ("a: !!timestamp 2001-12-14\n", "the tag !!timestamp"),
            ("a: !!int abc\n", "does not fit its tag !!int"),
            (
                "a: !!float 18446744073709551615\n",
                "does not fit its tag !!float",
            ),
            ("a: !!map [x]\n", "on a !!seq"),
            ("a: 2024-01-01\n", "the timestamp-like value \"2024-01-01\""),
            ("a:\n  b: [1999-x]\n", "the timestamp-like value \"1999-x\""),
            ("~: a\n", "a null mapping key"),
            ("? [a]\n: b\n", "a mapping key that is not a scalar"),
            ("m: {[a]: b}\n", "invalid map key"),
            ("m: {1: a, <<: {b: 2}}\n", "a merge key in a mapping"),
            ("1: a\n<<: {b: 2}\n", "a merge key in a mapping"),
        ] {
            let feature = unsupported(text.as_bytes());
            assert!(feature.contains(part), "{text:?}: {feature}");
        }
        // Quoted, these are strings.
        assert_eq!(
            decoded("a: '2024-01-01'\nb: \"!foo\"\n"),
            map(&[("a", str("2024-01-01")), ("b", str("!foo"))])
        );
    }

    #[test]
    fn deep_nesting_is_refused_without_overflowing_the_stack() {
        // Runs on the default 2 MiB test thread. Inputs of at most 400 KB.
        let block = |levels: usize| format!("z:\n{}a\n", "- ".repeat(levels));
        let flow = |levels: usize| format!("z: {}{}\n", "[".repeat(levels), "]".repeat(levels));
        // The root mapping is level 1, so 998 sequences put `a` at level
        // 1000, the deepest value built; the 2 MiB stack also drops it.
        let deepest = decoded(&block(998));
        drop(deepest.clone());
        assert_eq!(deepest, decoded(&block(998)));
        for text in [block(999), block(100_000)] {
            let feature = unsupported(text.as_bytes());
            assert!(feature.contains("deeper than 1000 levels"), "{feature}");
        }
        // yaml-rust2 refuses flow nesting deeper than 255 levels itself.
        assert!(decode_document(flow(255).as_bytes()).is_ok());
        let feature = unsupported(flow(100_000).as_bytes());
        assert!(
            feature.starts_with("yaml.v3's error for this input ("),
            "{feature}"
        );
        assert!(block(100_000).len() <= 400_000);
    }

    #[test]
    fn aliases_that_nest_deeper_than_the_limit_are_refused() {
        // Each anchor nests the one before it 200 flow levels deeper, so
        // the source stays shallow while the value grows 200 levels per
        // anchor.
        let chain = |anchors: usize| {
            let mut text = format!("a0: &a0 {}x{}\n", "[".repeat(200), "]".repeat(200));
            for level in 1..anchors {
                text.push_str(&format!(
                    "a{level}: &a{level} {}*a{}{}\n",
                    "[".repeat(200),
                    level - 1,
                    "]".repeat(200)
                ));
            }
            text
        };
        // Four anchors: the deepest value is at level 1 + 4 * 200 + 1.
        decoded(&chain(4));
        let feature = unsupported(chain(5).as_bytes());
        assert!(feature.contains("deeper than 1000 levels"), "{feature}");
        // Nothing is built before the refusal.
        VALUES_BUILT.with(|count| count.set(0));
        unsupported(chain(6).as_bytes());
        assert_eq!(VALUES_BUILT.with(|count| count.get()), 0);
    }
}
