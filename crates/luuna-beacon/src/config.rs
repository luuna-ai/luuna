//! `config.yaml`: the bytes `init` writes and the lenient read of
//! `loadConfig` (`cli.go:155-160`, `1744-1763`), with YAML parsed by
//! yaml-rust2 (assumption AS8).
//!
//! yaml.v3 parses the first YAML document into a node tree and decodes that
//! tree into a `map[string]any`. This module reads the first document the
//! same way, as parser events, into a compact tree of its own: one small
//! entry per node, aliases kept as references and never copied, so memory
//! grows with the size of the input. It then walks the tree as yaml.v3's
//! decoder would, applying its limits, and looks up `project` and `version`.

use crate::error::BeaconError;
use crate::gofmt;
use crate::paths;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;
use std::str::Chars;
use yaml_rust2::parser::{Parser, Tag};
use yaml_rust2::scanner::{Marker, Scanner, TScalarStyle, Token, TokenType};
use yaml_rust2::{Event, Yaml};

/// The project name used when config.yaml does not give one.
pub(crate) const DEFAULT_PROJECT: &str = "luuna-beacon";

/// The config version used when config.yaml does not give one.
pub(crate) const DEFAULT_VERSION: &str = "1";

/// What Go's `yaml.Marshal` writes for the default config `init` creates:
/// `model.Config{Project: "luuna-beacon", Version: "1", Targets:
/// {"default": {Dir: "agents", Default: true}}}`, in field order with
/// yaml.v3's four-space indent; `"1"` is quoted because unquoted it would
/// read as an integer.
pub(crate) const DEFAULT_CONFIG_YAML: &str = concat!(
    "project: luuna-beacon\n",
    "version: \"1\"\n",
    "targets:\n",
    "    default:\n",
    "        dir: agents\n",
    "        default: true\n",
);

/// The part of config.yaml this library reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Config {
    pub(crate) project: String,
    pub(crate) version: String,
}

/// Go's `loadConfig` as far as `project` and `version`: runs the legacy
/// migration first (as `paths.Config` does), reads
/// `<root>/.luuna/beacon/config.yaml` (an unreadable or missing file is an
/// error), parses it as YAML (invalid YAML, or a top level that is neither a
/// mapping nor empty, is an error), and takes `project` and `version` when
/// they are non-empty strings, otherwise the defaults.
pub(crate) fn load(root: &Path) -> Result<Config, BeaconError> {
    paths::migrate_legacy(root)?;
    let path = paths::config_file(root);
    let content = fs::read(&path).map_err(|error| BeaconError::os("open", &path, error))?;
    parse(&content)
}

/// Parses config.yaml content as `loadConfig` does.
pub(crate) fn parse(content: &[u8]) -> Result<Config, BeaconError> {
    let text = std::str::from_utf8(content)
        .map_err(|error| BeaconError::Yaml(format!("invalid UTF-8: {error}")))?;
    let document = Document::read(text)?;
    // No document, an empty one or an explicit null leaves Go's map nil.
    let root = match document.root {
        None => None,
        Some(root) => match &document.nodes[document.target(root)] {
            Node::Mapping(_) => Some(root),
            Node::Scalar {
                kind: ScalarKind::Null | ScalarKind::Bad,
                ..
            } => None,
            other => {
                return Err(BeaconError::Yaml(format!(
                    "cannot unmarshal {} into a mapping",
                    other.kind_name()
                )))
            }
        },
    };
    if let Some(root) = root {
        document.check(root)?;
    }
    let text_of = |key: &str| root.and_then(|root| document.field(root, key));
    Ok(Config {
        project: text_of("project").unwrap_or_else(|| DEFAULT_PROJECT.to_owned()),
        version: text_of("version").unwrap_or_else(|| DEFAULT_VERSION.to_owned()),
    })
}

/// yaml.v3's `max_indents` (scannerc.go:989-1010): a block collection
/// nested deeper than this is a scanner error.
const MAX_DEPTH: usize = 10_000;

/// yaml.v3's `alias_ratio_range_low` and `alias_ratio_range_high`
/// (decode.go:455-462).
const ALIAS_RATIO_RANGE_LOW: u64 = 400_000;
const ALIAS_RATIO_RANGE_HIGH: u64 = 4_000_000;

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

/// yaml.v3's `failWantMap` message (decode.go:917-919).
const WANT_MAP: &str = "map merge requires map or sequence of maps as the value";

/// How YamlLoader would have typed a scalar (yaml-rust2's `Yaml`), which
/// decides whether `project` or `version` is a string.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScalarKind {
    Null,
    /// `Yaml::BadValue`: a value that does not fit its explicit tag.
    Bad,
    Str,
    Int,
    Real,
    Bool,
}

/// One node of the first document, without copies: an alias refers to its
/// anchored node by index.
enum Node {
    /// `start..end` is the value's range in [`Document::texts`]; `merge`
    /// when yaml.v3's `isMerge` holds for it (decode.go:998-1000), using the
    /// tag that `parser.scalar` and `parser.node` give it (decode.go:224-252,
    /// 170-194).
    Scalar {
        merge: bool,
        kind: ScalarKind,
        start: usize,
        end: usize,
    },
    Sequence(Vec<usize>),
    /// Keys and values, alternating, in document order.
    Mapping(Vec<usize>),
    /// `target` is the anchored node; `mark` is the char index of the `*`.
    Alias {
        target: usize,
        mark: usize,
    },
}

impl Node {
    /// The kind named in the error for a top level that is not a mapping.
    fn kind_name(&self) -> &'static str {
        match self {
            Node::Sequence(_) => "a sequence",
            Node::Scalar { kind, .. } => match kind {
                ScalarKind::Str => "a string",
                ScalarKind::Int => "an integer",
                ScalarKind::Real => "a float",
                ScalarKind::Bool => "a boolean",
                ScalarKind::Null | ScalarKind::Bad => "a value",
            },
            Node::Mapping(_) | Node::Alias { .. } => "a value",
        }
    }
}

/// What yaml.v3 compares to find a repeated mapping key: the key node's
/// kind and value (decode.go:801-814). An alias's value is its anchor name;
/// a collection's value is empty.
#[derive(Debug, PartialEq, Eq, Hash)]
enum KeyId {
    Scalar(String),
    Alias(String),
    Sequence,
    Mapping,
}

/// The keys seen so far in a mapping that is still open.
struct OpenKeys {
    /// The next child is a key (not a value).
    next_is_key: bool,
    /// Children that were keys so far.
    count: usize,
    /// Each distinct key: its position among the keys and its line.
    seen: HashMap<KeyId, (usize, usize)>,
    /// The error yaml.v3 reports first for this mapping: the key it meets
    /// first that is repeated later, with its first repetition. Kept with
    /// the position of the first key.
    first_repeat: Option<(usize, String)>,
}

/// The first document of config.yaml as a node tree.
struct Document<'a> {
    text: &'a str,
    nodes: Vec<Node>,
    /// The values of all scalars, one after the other.
    texts: String,
    root: Option<usize>,
    /// Mappings with a repeated key, with yaml.v3's message for the first.
    repeated: HashMap<usize, String>,
}

impl<'a> Document<'a> {
    /// Reads the first YAML document of `text`, as yaml.v3's `unmarshal`
    /// does with one `p.parse()` (yaml.go:156-168): later documents are not
    /// read, so neither their content nor their syntax errors matter.
    ///
    /// Events are pulled one at a time (yaml-rust2's `Parser::load` recurses
    /// once per nesting level), and the block nesting depth is limited as
    /// yaml.v3's scanner limits it.
    fn read(text: &'a str) -> Result<Document<'a>, BeaconError> {
        let mut document = Document {
            text,
            nodes: Vec::new(),
            texts: String::new(),
            root: None,
            repeated: HashMap::new(),
        };
        let mut parser = Parser::new_from_str(text);
        let mut depth = DepthLimit::new(text);
        let mut names = AnchorNames::new(text);
        let mut open: Vec<(usize, Option<OpenKeys>)> = Vec::new();
        let mut anchors: HashMap<usize, usize> = HashMap::new();
        loop {
            let (event, mark) = match parser.next_token() {
                Ok(next) => next,
                Err(error) => {
                    // yaml.v3's scanner would meet a depth overflow before
                    // this error if it comes first in the text.
                    depth.advance_to(error.marker().index())?;
                    return Err(BeaconError::Yaml(error.to_string()));
                }
            };
            depth.advance_to(mark.index())?;
            let (node, anchor, key) = match event {
                Event::StreamEnd | Event::DocumentEnd => break,
                Event::StreamStart | Event::DocumentStart | Event::Nothing => continue,
                Event::SequenceEnd => {
                    open.pop();
                    continue;
                }
                Event::MappingEnd => {
                    if let Some((index, Some(keys))) = open.pop() {
                        if let Some((_, message)) = keys.first_repeat {
                            document.repeated.insert(index, message);
                        }
                    }
                    continue;
                }
                Event::Scalar(value, style, anchor, tag) => {
                    let merge = value == "<<"
                        && match &tag {
                            Some(tag)
                                if tag.handle == "tag:yaml.org,2002:" && tag.suffix == "merge" =>
                            {
                                true
                            }
                            Some(tag) if format!("{}{}", tag.handle, tag.suffix) != "!" => false,
                            _ => style == TScalarStyle::Plain,
                        };
                    let kind = scalar_kind(&value, style, tag.as_ref());
                    let start = document.texts.len();
                    document.texts.push_str(&value);
                    let node = Node::Scalar {
                        merge,
                        kind,
                        start,
                        end: document.texts.len(),
                    };
                    (node, anchor, KeyId::Scalar(value))
                }
                Event::SequenceStart(anchor, _) => {
                    (Node::Sequence(Vec::new()), anchor, KeyId::Sequence)
                }
                Event::MappingStart(anchor, _) => {
                    (Node::Mapping(Vec::new()), anchor, KeyId::Mapping)
                }
                Event::Alias(anchor) => {
                    // yaml-rust2 refuses an unknown anchor before this event.
                    let Some(&target) = anchors.get(&anchor) else {
                        continue;
                    };
                    let node = Node::Alias {
                        target,
                        mark: mark.index(),
                    };
                    (node, 0, KeyId::Alias(names.name_at(mark.index())))
                }
            };
            let opens = match node {
                Node::Sequence(_) => Some(None),
                Node::Mapping(_) => Some(Some(OpenKeys {
                    next_is_key: true,
                    count: 0,
                    seen: HashMap::new(),
                    first_repeat: None,
                })),
                Node::Scalar { .. } | Node::Alias { .. } => None,
            };
            let index = document.nodes.len();
            document.nodes.push(node);
            // yaml.v3 and yaml-rust2 both register an anchor when its node
            // starts, so an alias inside the node can refer to it.
            if anchor > 0 {
                anchors.insert(anchor, index);
            }
            match open.last_mut() {
                Some((parent, keys)) => {
                    if let Node::Sequence(children) | Node::Mapping(children) =
                        &mut document.nodes[*parent]
                    {
                        children.push(index);
                    }
                    if let Some(keys) = keys {
                        keys.add(key, mark);
                    }
                }
                None => document.root = Some(index),
            }
            if let Some(keys) = opens {
                open.push((index, keys));
            }
        }
        Ok(document)
    }

    /// The node an alias refers to, or the node itself.
    fn target(&self, node: usize) -> usize {
        match self.nodes[node] {
            Node::Alias { target, .. } => target,
            _ => node,
        }
    }

    fn is_merge(&self, node: usize) -> bool {
        matches!(self.nodes[node], Node::Scalar { merge: true, .. })
    }

    /// Whether a mapping key reads as the string `name` (directly or through
    /// an alias), as YamlLoader's string keys did.
    fn key_is(&self, key: usize, name: &str) -> bool {
        matches!(
            self.nodes[self.target(key)],
            Node::Scalar { kind: ScalarKind::Str, start, end, .. } if &self.texts[start..end] == name
        )
    }

    /// A value that is a non-empty string (directly or through an alias).
    fn string_value(&self, value: usize) -> Option<String> {
        match self.nodes[self.target(value)] {
            Node::Scalar {
                kind: ScalarKind::Str,
                start,
                end,
                ..
            } if end > start => Some(self.texts[start..end].to_owned()),
            _ => None,
        }
    }

    /// The value `raw[name].(string)` has in `loadConfig` when it is a
    /// non-empty string. The top-level mapping's own keys come first, the
    /// last one winning (decode.go:836-856); only when none matches do merged
    /// mappings supply the key, the first one met winning, in yaml.v3's
    /// merge order (decode.go:959-996): a merged mapping's own keys, then
    /// what it merges in turn, and the mappings of a merged sequence from
    /// left to right. Call only after [`Document::check`] has passed, which
    /// bounds this walk and guarantees every merge value is valid.
    fn field(&self, root: usize, name: &str) -> Option<String> {
        let Node::Mapping(children) = &self.nodes[self.target(root)] else {
            return None;
        };
        let mut found = None;
        let mut merge = None;
        for pair in children.chunks(2) {
            if self.is_merge(pair[0]) {
                merge = pair.get(1).copied();
            } else if self.key_is(pair[0], name) {
                found = Some(pair.get(1).copied());
            }
        }
        if let Some(value) = found {
            return value.and_then(|value| self.string_value(value));
        }
        let mut pending: Vec<usize> = merge.into_iter().collect();
        while let Some(node) = pending.pop() {
            match &self.nodes[self.target(node)] {
                Node::Mapping(children) => {
                    let mut inner = None;
                    for pair in children.chunks(2) {
                        if self.is_merge(pair[0]) {
                            inner = pair.get(1).copied();
                        } else if self.key_is(pair[0], name) {
                            return pair.get(1).and_then(|&value| self.string_value(value));
                        }
                    }
                    pending.extend(inner);
                }
                Node::Sequence(elements) => pending.extend(elements.iter().rev()),
                Node::Scalar { .. } | Node::Alias { .. } => {}
            }
        }
        None
    }

    /// Decodes the top-level mapping as yaml.v3 decodes it into a
    /// `map[string]any`, without building any value, and returns the error
    /// yaml.v3 would return:
    ///
    /// - its alias rule: the counters and ratio of `decoder.unmarshal`
    ///   (decode.go:455-491), in the order it visits nodes (decode.go:484-519,
    ///   751-757, 767-862), and an alias that re-enters itself
    ///   (`decoder.alias`, decode.go:533-543);
    /// - a merge value that is not a mapping or a sequence of mappings
    ///   (`decoder.merge`, decode.go:917-996);
    /// - a repeated key (decode.go:801-814): yaml.v3 does not decode such a
    ///   mapping's content and reports the repetition when decoding ends.
    ///
    /// The first two stop the decode at once; a repeated key is reported only
    /// if neither happens. yaml.v3 lists every repetition it meets; this
    /// reports the first, so a hostile file cannot make the message large.
    ///
    /// Where yaml.v3 skips a value (a null key, a key that is not a string,
    /// a merged key already present) the walk still counts it, so the rule
    /// may refuse such input a little earlier than yaml.v3.
    ///
    /// The walk uses its own stack, so deep nesting cannot overflow ours, and
    /// it stops as soon as the rule fails, so it never runs longer than
    /// yaml.v3's own decode would.
    fn check(&self, root: usize) -> Result<(), BeaconError> {
        enum Step {
            /// `decoder.unmarshal` of a node; `merging` when the node is
            /// being merged into a mapping (`d.mergedFields` is set).
            Visit { node: usize, merging: bool },
            /// The end of `decoder.alias` for this alias node.
            LeaveAlias(usize),
            /// `decoder.merge` for a mapping and the value of its merge key.
            Merge {
                mapping: usize,
                value: usize,
                merging: bool,
            },
            /// The kind check and decode of the merge value itself.
            MergeValue(usize),
            /// One element of a merged sequence.
            MergeElement(usize),
        }
        let want_map = || BeaconError::Yaml(WANT_MAP.to_owned());
        let is_mapping = |node: usize| matches!(self.nodes[node], Node::Mapping(_));
        // The document node itself is the first `unmarshal`
        // (decode.go:524-531).
        let mut decode_count: u64 = 1;
        let mut alias_count: u64 = 0;
        let mut alias_depth: u64 = 0;
        let mut active_aliases = HashSet::new();
        let mut repeated_key: Option<&String> = None;
        let mut steps = vec![Step::Visit {
            node: root,
            merging: false,
        }];
        while let Some(step) = steps.pop() {
            let mut next = Vec::new();
            let (node, merging) = match step {
                Step::Visit { node, merging } => (node, merging),
                Step::LeaveAlias(alias) => {
                    alias_depth -= 1;
                    active_aliases.remove(&alias);
                    continue;
                }
                Step::Merge {
                    mapping,
                    value,
                    merging,
                } => {
                    // decode.go:959-967: outside a merge, every key of the
                    // mapping is decoded once more first.
                    if !merging {
                        if let Node::Mapping(children) = &self.nodes[mapping] {
                            next.extend(children.iter().step_by(2).map(|&node| Step::Visit {
                                node,
                                merging: false,
                            }));
                        }
                    }
                    next.push(Step::MergeValue(value));
                    steps.extend(next.into_iter().rev());
                    continue;
                }
                Step::MergeValue(value) => {
                    // decode.go:969-993.
                    match &self.nodes[value] {
                        Node::Mapping(_) => {}
                        Node::Alias { target, .. } if is_mapping(*target) => {}
                        Node::Sequence(elements) => {
                            steps.extend(
                                elements.iter().rev().map(|&node| Step::MergeElement(node)),
                            );
                            continue;
                        }
                        Node::Alias { .. } | Node::Scalar { .. } => return Err(want_map()),
                    }
                    (value, true)
                }
                Step::MergeElement(element) => {
                    // decode.go:980-988.
                    if !is_mapping(self.target(element)) {
                        return Err(want_map());
                    }
                    (element, true)
                }
            };
            // decode.go:485-491.
            decode_count += 1;
            if alias_depth > 0 {
                alias_count += 1;
            }
            if alias_count > 100
                && decode_count > 1000
                && alias_count as f64 / decode_count as f64 > allowed_alias_ratio(decode_count)
            {
                return Err(BeaconError::Yaml(
                    "document contains excessive aliasing".to_owned(),
                ));
            }
            match &self.nodes[node] {
                Node::Scalar { .. } => {}
                Node::Sequence(children) => {
                    // decode.go:751-757.
                    next.extend(children.iter().map(|&node| Step::Visit {
                        node,
                        merging: false,
                    }));
                }
                Node::Alias { target, mark } => {
                    // decode.go:533-543.
                    if !active_aliases.insert(node) {
                        return Err(BeaconError::Yaml(format!(
                            "anchor '{}' value contains itself",
                            AnchorNames::new(self.text).name_at(*mark)
                        )));
                    }
                    alias_depth += 1;
                    next.push(Step::Visit {
                        node: *target,
                        merging,
                    });
                    next.push(Step::LeaveAlias(node));
                }
                Node::Mapping(children) => {
                    if let Some(message) = self.repeated.get(&node) {
                        // decode.go:801-814: the content is not decoded.
                        repeated_key.get_or_insert(message);
                        continue;
                    }
                    // decode.go:836-862: every key then its value, in order,
                    // except merge keys; the last merge value is merged
                    // afterwards. (Two merge keys are a repeated key.)
                    let mut merge = None;
                    for pair in children.chunks(2) {
                        if self.is_merge(pair[0]) {
                            merge = pair.get(1).copied();
                            continue;
                        }
                        next.extend(pair.iter().map(|&node| Step::Visit {
                            node,
                            merging: false,
                        }));
                    }
                    if let Some(value) = merge {
                        next.push(Step::Merge {
                            mapping: node,
                            value,
                            merging,
                        });
                    }
                }
            }
            steps.extend(next.into_iter().rev());
        }
        match repeated_key {
            Some(message) => Err(BeaconError::Yaml(format!("unmarshal errors:\n  {message}"))),
            None => Ok(()),
        }
    }
}

impl OpenKeys {
    /// Records a child of the mapping; every other child is a key.
    fn add(&mut self, key: KeyId, mark: Marker) {
        let is_key = self.next_is_key;
        self.next_is_key = !is_key;
        if !is_key {
            return;
        }
        let position = self.count;
        self.count += 1;
        let line = mark.line();
        match self.seen.get(&key) {
            // yaml.v3 reports pairs in the order of the first key, then the
            // repetition; a key's first repetition is the one met here.
            Some(&(first, first_line)) => {
                if self.first_repeat.as_ref().is_none_or(|(at, _)| first < *at) {
                    let value = match &key {
                        KeyId::Scalar(value) | KeyId::Alias(value) => value.as_str(),
                        KeyId::Sequence | KeyId::Mapping => "",
                    };
                    self.first_repeat = Some((
                        first,
                        format!(
                            "line {line}: mapping key {} already defined at line {first_line}",
                            gofmt::quote(value)
                        ),
                    ));
                }
            }
            None => {
                self.seen.insert(key, (position, line));
            }
        }
    }
}

/// How YamlLoader types a scalar (yaml-rust2 0.10 `YamlLoader::on_event`):
/// quoted and block scalars are strings; a `!!bool`, `!!int`, `!!float` or
/// `!!null` tag must fit or the value is bad; other tags give a string;
/// an untagged plain scalar is resolved by `Yaml::from_str`.
fn scalar_kind(value: &str, style: TScalarStyle, tag: Option<&Tag>) -> ScalarKind {
    if style != TScalarStyle::Plain {
        return ScalarKind::Str;
    }
    let fits = |ok: bool, kind: ScalarKind| if ok { kind } else { ScalarKind::Bad };
    match tag {
        Some(tag) if tag.handle == "tag:yaml.org,2002:" => match tag.suffix.as_str() {
            "bool" => fits(
                matches!(
                    value,
                    "true" | "True" | "TRUE" | "false" | "False" | "FALSE"
                ),
                ScalarKind::Bool,
            ),
            "int" => fits(value.parse::<i64>().is_ok(), ScalarKind::Int),
            "float" => fits(parse_f64(value).is_some(), ScalarKind::Real),
            "null" => fits(matches!(value, "~" | "null"), ScalarKind::Null),
            _ => ScalarKind::Str,
        },
        Some(_) => ScalarKind::Str,
        None => match Yaml::from_str(value) {
            Yaml::Null => ScalarKind::Null,
            Yaml::Boolean(_) => ScalarKind::Bool,
            Yaml::Integer(_) => ScalarKind::Int,
            Yaml::Real(_) => ScalarKind::Real,
            Yaml::String(_) => ScalarKind::Str,
            _ => ScalarKind::Bad,
        },
    }
}

/// yaml-rust2's `parse_f64` (yaml.rs:63-73), used for `!!float`.
fn parse_f64(value: &str) -> Option<f64> {
    match value {
        ".inf" | ".Inf" | ".INF" | "+.inf" | "+.Inf" | "+.INF" => Some(f64::INFINITY),
        "-.inf" | "-.Inf" | "-.INF" => Some(f64::NEG_INFINITY),
        ".nan" | ".NaN" | ".NAN" => Some(f64::NAN),
        _ if value.bytes().any(|byte| byte.is_ascii_digit()) => value.parse::<f64>().ok(),
        _ => None,
    }
}

/// Reads anchor names after `*` marks, moving forward through the text (the
/// parser's marks only grow), so reading every alias costs one pass.
struct AnchorNames<'a> {
    text: &'a str,
    chars: Chars<'a>,
    /// The char index `chars` is at.
    at: usize,
}

impl<'a> AnchorNames<'a> {
    fn new(text: &'a str) -> Self {
        AnchorNames {
            text,
            chars: text.chars(),
            at: 0,
        }
    }

    /// The anchor name an alias at char index `mark` uses, as yaml-rust2's
    /// scanner reads it (`scan_anchor`).
    fn name_at(&mut self, mark: usize) -> String {
        if mark + 1 < self.at {
            self.chars = self.text.chars();
            self.at = 0;
        }
        while self.at < mark + 1 && self.chars.next().is_some() {
            self.at += 1;
        }
        let mut name = String::new();
        while let Some(character) = self.chars.clone().next() {
            if matches!(
                character,
                ' ' | '\t' | '\n' | '\r' | '\u{feff}' | '\0' | ',' | '[' | ']' | '{' | '}'
            ) {
                break;
            }
            name.push(character);
            self.chars.next();
            self.at += 1;
        }
        name
    }
}

/// yaml.v3's block depth limit (scannerc.go:989-1010), applied to the tokens
/// of a second yaml-rust2 scanner that follows the parser.
///
/// yaml-rust2 emits a block-start token where yaml.v3 pushes an indentation
/// level and a block-end token where it pops one, so their balance is
/// yaml.v3's `len(parser.indents)`. The error names the line yaml.v3 names:
/// the scanner's last saved simple key, tracked here from the tokens with
/// yaml.v3's `simple_key_allowed` rules at flow level 0
/// (scannerc.go:911-936, 1094-1604).
///
/// Flow collections are not counted: yaml-rust2 refuses flow nesting deeper
/// than 255 levels, well before yaml.v3's flow limit of 10000
/// (scannerc.go:955-975).
struct DepthLimit<'a> {
    tokens: Scanner<Chars<'a>>,
    next: Option<Token>,
    ended: bool,
    depth: usize,
    flow_level: usize,
    key_allowed: bool,
    /// A simple key's `:` is still to come at flow level 0.
    simple_value: bool,
    /// yaml.v3's (0-based) line of the last saved simple key; 0 when none.
    key_line: usize,
    /// The line of the last real token.
    line: usize,
}

impl<'a> DepthLimit<'a> {
    fn new(text: &'a str) -> Self {
        DepthLimit {
            tokens: Scanner::new(text.chars()),
            next: None,
            ended: false,
            depth: 0,
            flow_level: 0,
            key_allowed: false,
            simple_value: false,
            key_line: 0,
            line: 0,
        }
    }

    fn peek(&mut self) -> Option<&Token> {
        if self.next.is_none() && !self.ended {
            self.next = self.tokens.next();
            // A scanner error ends the tokens here; the parser reports it.
            self.ended = self.next.is_none();
        }
        self.next.as_ref()
    }

    /// Takes every token up to char index `index`, which the parser has
    /// already scanned.
    fn advance_to(&mut self, index: usize) -> Result<(), BeaconError> {
        while self.peek().is_some_and(|token| token.0.index() <= index) {
            if let Some(token) = self.next.take() {
                self.token(token)?;
            }
        }
        Ok(())
    }

    fn token(&mut self, Token(mark, kind): Token) -> Result<(), BeaconError> {
        match kind {
            TokenType::BlockSequenceStart | TokenType::BlockMappingStart => {
                self.depth += 1;
                if self.depth > MAX_DEPTH {
                    return Err(self.too_deep(mark, &kind));
                }
                return Ok(());
            }
            TokenType::BlockEnd => {
                self.depth = self.depth.saturating_sub(1);
                return Ok(());
            }
            TokenType::Key
                if self
                    .peek()
                    .is_some_and(|next| next.0.index() == mark.index()) =>
            {
                // The KEY token inserted before a simple key; the key's own
                // tokens follow.
                if self.flow_level == 0 {
                    self.simple_value = true;
                }
                return Ok(());
            }
            _ => {}
        }
        if self.flow_level > 0 {
            self.line = mark.line();
            match kind {
                TokenType::FlowSequenceStart | TokenType::FlowMappingStart => self.flow_level += 1,
                TokenType::FlowSequenceEnd | TokenType::FlowMappingEnd => {
                    self.flow_level -= 1;
                    if self.flow_level == 0 {
                        self.key_allowed = false;
                    }
                }
                _ => {}
            }
            return Ok(());
        }
        // In the block context a new line may start a simple key.
        if mark.line() > self.line {
            self.key_allowed = true;
        }
        self.line = mark.line();
        match kind {
            TokenType::StreamStart(_) | TokenType::BlockEntry | TokenType::Key => {
                self.key_allowed = true;
            }
            TokenType::Value => {
                // A simple key's `:` forbids another simple key; a complex
                // key's allows one.
                self.key_allowed = !self.simple_value;
                self.simple_value = false;
            }
            TokenType::Scalar(TScalarStyle::Literal | TScalarStyle::Folded, _) => {
                self.key_allowed = true;
            }
            TokenType::Anchor(_)
            | TokenType::Alias(_)
            | TokenType::Tag(..)
            | TokenType::Scalar(..) => {
                self.save_key(mark);
                self.key_allowed = false;
            }
            TokenType::FlowSequenceStart | TokenType::FlowMappingStart => {
                self.save_key(mark);
                self.flow_level += 1;
            }
            _ => self.key_allowed = false,
        }
        Ok(())
    }

    fn save_key(&mut self, mark: Marker) {
        if self.key_allowed {
            self.key_line = mark.line() - 1;
        }
    }

    /// yaml.v3's message: the line of the context mark (the simple key),
    /// else of the problem mark (this token), else none (`parser.fail`,
    /// decode.go:118-141).
    fn too_deep(&mut self, mark: Marker, kind: &TokenType) -> BeaconError {
        // A block mapping opened by a simple key: yaml.v3's context mark is
        // that key, whose KEY token follows with the key's position.
        let key = match kind {
            TokenType::BlockMappingStart => match self.peek() {
                Some(Token(next, TokenType::Key)) if next.index() != mark.index() => {
                    Some(next.line() - 1)
                }
                _ => None,
            },
            _ => None,
        };
        let context = key.unwrap_or(self.key_line);
        let problem = mark.line() - 1;
        let message = format!("exceeded max depth of {MAX_DEPTH}");
        BeaconError::Yaml(if context != 0 {
            format!("line {}: {message}", context + 1)
        } else if problem != 0 {
            format!("line {}: {message}", problem + 1)
        } else {
            message
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(text: &str) -> String {
        parse(text.as_bytes())
            .unwrap_or_else(|error| panic!("{text:?}: {error}"))
            .project
    }

    #[test]
    fn default_config_reads_back_as_the_defaults() {
        let config = parse(DEFAULT_CONFIG_YAML.as_bytes()).unwrap();
        assert_eq!(
            config,
            Config {
                project: DEFAULT_PROJECT.to_owned(),
                version: DEFAULT_VERSION.to_owned(),
            }
        );
    }

    #[test]
    fn project_falls_back_to_the_default() {
        assert_eq!(project("project: demo-x\nversion: \"1\"\n"), "demo-x");
        assert_eq!(project("project: 'demo-q'\nversion: 1\n"), "demo-q");
        assert_eq!(
            project("# header\nproject: demo-c # trailing\ntargets: {default: {dir: agents, default: true}}\n"),
            "demo-c"
        );
        assert_eq!(project("project: \"123\"\n"), "123");
        assert_eq!(project("project: yes\n"), "yes");
        assert_eq!(project("base: &b demo-a\nproject: *b\n"), "demo-a");
        assert_eq!(project("project: one\n---\nproject: two\n"), "one");
        for text in [
            "project: 123\nversion: \"1\"\n",
            "project: 1.5\n",
            "project: true\nversion: \"1\"\n",
            "project: ''\nversion: \"1\"\n",
            "project:\nversion: \"1\"\n",
            "project: ~\n",
            "project: [a]\n",
            "project: {a: b}\n",
            "version: \"1\"\n",
            "",
            "# only a comment\n",
            "---\n",
            "null\n",
        ] {
            assert_eq!(project(text), DEFAULT_PROJECT, "{text:?}");
        }
        let config = parse(b"version: 2\n").unwrap();
        assert_eq!(
            config.version, DEFAULT_VERSION,
            "a numeric version falls back"
        );
        let config = parse(b"version: \"2\"\n").unwrap();
        assert_eq!(config.version, "2");
    }

    #[test]
    fn invalid_yaml_is_an_error() {
        for text in [
            "project: [unclosed\n",
            "- a\n- b\n",
            "project: a: b\n",
            "\t\n  bad: [\n",
            "just a string\n",
            "42\n",
            "project: a\nproject: b\n",
        ] {
            let error = parse(text.as_bytes()).expect_err(text);
            assert!(matches!(error, BeaconError::Yaml(_)), "{text:?}: {error}");
        }
        assert!(matches!(
            parse(b"project: \xff\n"),
            Err(BeaconError::Yaml(_))
        ));
    }

    /// The nine-level "billion laughs" source text (about 400 bytes). Fully
    /// expanded it would be 9^9 nodes; only the parser under test may expand
    /// it, and the fix must stop it first.
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

    const EXCESSIVE: &str = "yaml: document contains excessive aliasing";

    /// `x: &x [a, ...]` with `items` scalars, then `y:` a sequence of
    /// `aliases` aliases of it. yaml.v3 counts 8 + items + aliases *
    /// (items + 2) decodes, aliases * (items + 1) of them under an alias.
    fn repeated_list(items: usize, aliases: usize) -> String {
        format!(
            "project: demo\nx: &x [{}]\ny: [{}]\n",
            vec!["a"; items].join(", "),
            vec!["*x"; aliases].join(", ")
        )
    }

    #[test]
    fn billion_laughs_is_refused_like_go() {
        let started = std::time::Instant::now();
        let error = parse(BILLION_LAUGHS.as_bytes()).unwrap_err();
        assert_eq!(error.to_string(), EXCESSIVE);
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "took {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn excessive_aliasing_in_a_later_document_is_ignored_like_go() {
        // yaml.v3 decodes the first document only; Go's tool reads this file
        // and uses project `one`.
        let text = format!("project: one\n---\n{BILLION_LAUGHS}");
        assert_eq!(project(&text), "one");
    }

    #[test]
    fn a_syntax_error_in_a_later_document_is_ignored_like_go() {
        // Go's tool reads both files and uses the first document's project.
        assert_eq!(project("project: demo\n---\nj: [unclosed\n"), "demo");
        assert_eq!(project("project: a\n...\n[x\n"), "a");
    }

    /// The first document of `text`, as read before the decode.
    fn read(text: &str) -> Document<'_> {
        Document::read(text).unwrap_or_else(|error| panic!("{error}"))
    }

    #[test]
    fn many_documents_that_each_pass_the_alias_rule_load_with_bounded_memory() {
        // 400 copies of the document Go accepts at its alias limit (1.4 MB).
        // Building every document with its aliases expanded took over 4 GiB.
        // Only the first document is read: the node count is that of one
        // copy. Go's tool reads this file and uses project `demo`.
        let one = repeated_list(1000, 110);
        let many = vec![one.as_str(); 400].join("---\n");
        assert_eq!(read(&many).nodes.len(), read(&one).nodes.len());
        assert_eq!(project(&many), "demo");
    }

    #[test]
    fn nested_anchors_around_content_load_with_bounded_memory() {
        // 250 nested anchors around 95 aliases of a 4000-item list (14 KB):
        // within Go's alias rule (ratio 0.9893), and Go's tool reads it. A
        // reader that copies each anchored node at every level needed over
        // 4 GiB. Here every node is one entry, aliases are not copied, so
        // the tree has fewer nodes than the input has bytes.
        let text = format!(
            "project: demo\nx: &x [{}]\ny: {}{}{}\n",
            vec!["a"; 4000].join(", "),
            (0..250)
                .map(|level| format!("&n{level} ["))
                .collect::<String>(),
            vec!["*x"; 95].join(", "),
            "]".repeat(250),
        );
        assert!(read(&text).nodes.len() < text.len());
        assert_eq!(project(&text), "demo");
    }

    /// `z:` holds `depth - 1` nested block sequences, so the document is
    /// `depth` block levels deep; the last `-` is on line 3.
    fn deep_sequence(depth: usize) -> String {
        format!("project: demo\nz:\n{}a\n", "- ".repeat(depth))
    }

    /// A block mapping at level `depth`, its key `b` on line 4.
    fn deep_mapping(depth: usize) -> String {
        let dashes = depth - 2;
        format!(
            "project: demo\nz:\n{}a:\n{}b: 1\n",
            "- ".repeat(dashes),
            " ".repeat(2 * dashes + 2)
        )
    }

    #[test]
    fn nesting_deeper_than_go_s_limit_is_refused_with_go_s_text() {
        // Go's tool gives exactly these messages for these inputs. yaml.v3
        // names the line of the last possible simple key (`z`, line 2), the
        // key that opens the mapping (`b`, line 4), or, when no key came
        // before, the line of the `-` (line 3), or no line at all.
        let error = |text: &str| parse(text.as_bytes()).unwrap_err().to_string();
        assert_eq!(
            error(&deep_sequence(10_001)),
            "yaml: line 2: exceeded max depth of 10000"
        );
        assert_eq!(
            error(&deep_mapping(10_001)),
            "yaml: line 4: exceeded max depth of 10000"
        );
        let three_lines = format!(
            "{}\n{}{}\n{}{}a\n",
            "- ".repeat(3000),
            " ".repeat(6000),
            "- ".repeat(3000),
            " ".repeat(12_000),
            "- ".repeat(4001)
        );
        assert_eq!(
            error(&three_lines),
            "yaml: line 3: exceeded max depth of 10000"
        );
        assert_eq!(
            error(&format!("{}a\n", "- ".repeat(10_001))),
            "yaml: exceeded max depth of 10000"
        );
        // The limit is checked before anything else in the document is
        // decoded, as yaml.v3's scanner checks it.
        let after_aliasing = format!("{BILLION_LAUGHS}{}", &deep_sequence(10_001)[14..]);
        assert_eq!(
            error(&after_aliasing),
            "yaml: line 11: exceeded max depth of 10000"
        );
    }

    #[test]
    fn nesting_at_go_s_limit_loads_on_a_2_mib_stack() {
        // 2 MiB is the stack of a tokio worker thread. Go's tool reads both
        // inputs and uses project `demo`.
        let loaded = std::thread::Builder::new()
            .stack_size(2 << 20)
            .spawn(|| {
                [deep_sequence(10_000), deep_mapping(10_000)]
                    .map(|text| parse(text.as_bytes()).map(|config| config.project))
            })
            .unwrap()
            .join()
            .unwrap();
        for project in loaded {
            assert_eq!(project.unwrap(), "demo");
        }
    }

    #[test]
    fn flow_nesting_beyond_yaml_rust2_s_limit_is_refused() {
        // A difference from Go (README): yaml-rust2 refuses flow collections
        // nested deeper than 255 levels; yaml.v3 allows 10000.
        let nested = |depth: usize| {
            format!(
                "project: demo\nz: {}{}\n",
                "[".repeat(depth),
                "]".repeat(depth)
            )
        };
        assert_eq!(project(&nested(255)), "demo");
        assert!(matches!(
            parse(nested(256).as_bytes()),
            Err(BeaconError::Yaml(_))
        ));
    }

    #[test]
    fn a_top_level_merge_key_supplies_project_like_go() {
        // Each project below is the one Go's tool uses for the same file.
        assert_eq!(project("<<: {project: merged}\nversion: \"1\"\n"), "merged");
        assert_eq!(project("project: own\n<<: {project: merged}\n"), "own");
        assert_eq!(
            project("<<: [{project: first}, {project: second}]\n"),
            "first"
        );
        assert_eq!(
            project("inner: &i {project: inner}\nouter: &o {<<: *i, version: \"3\"}\n<<: *o\n"),
            "inner"
        );
        assert_eq!(project("project:\n<<: {project: m}\n"), DEFAULT_PROJECT);
        // Keys that read as `project`: the last one wins. (The space after
        // `*k` is needed because yaml-rust2 allows `:` in anchor names.)
        assert_eq!(project("k: &k project\nproject: a\n*k : b\n"), "b");
        let config = parse(b"base: &b {project: b, version: \"2\"}\n<<: *b\n").unwrap();
        assert_eq!(
            config,
            Config {
                project: "b".to_owned(),
                version: "2".to_owned(),
            }
        );
    }

    #[test]
    fn a_merge_value_that_is_not_a_mapping_is_refused_like_go() {
        for text in [
            "project: demo\n<<: 1\n",
            "s: &s [a]\n<<: *s\n",
            "<<: [{project: m}, x]\n",
        ] {
            assert_eq!(
                parse(text.as_bytes()).unwrap_err().to_string(),
                "yaml: map merge requires map or sequence of maps as the value",
                "{text:?}"
            );
        }
    }

    #[test]
    fn a_repeated_key_is_refused_like_go() {
        let error = |text: &str| parse(text.as_bytes()).unwrap_err().to_string();
        assert_eq!(
            error("project: a\nproject: b\n"),
            "yaml: unmarshal errors:\n  line 2: mapping key \"project\" already defined at line 1"
        );
        // yaml.v3 compares the key's text, whatever its type.
        assert_eq!(
            error("project: demo\n1: a\n\"1\": b\n"),
            "yaml: unmarshal errors:\n  line 3: mapping key \"1\" already defined at line 2"
        );
        assert_eq!(
            error("<<: {project: a}\n<<: {version: \"2\"}\n"),
            "yaml: unmarshal errors:\n  line 2: mapping key \"<<\" already defined at line 1"
        );
        // Go lists every repetition; the first of its list is reported here.
        assert_eq!(
            error("a: 1\nb: 1\nb: 2\na: 2\n"),
            "yaml: unmarshal errors:\n  line 4: mapping key \"a\" already defined at line 1"
        );
    }

    #[test]
    fn a_syntax_error_after_excessive_aliasing_is_reported_first_in_that_document() {
        // yaml.v3 parses the whole first document before decoding it.
        let text = format!("{BILLION_LAUGHS}j: [unclosed\n");
        let error = parse(text.as_bytes()).unwrap_err().to_string();
        assert_ne!(error, EXCESSIVE);
        assert!(error.starts_with("yaml: "), "{error}");
    }

    #[test]
    fn aliasing_at_go_s_limit_is_counted_like_go() {
        // 1000 items: 110 aliases give 110110 of 111228 decodes under an
        // alias (0.98995, allowed); 111 give 111111 of 112230 (0.99003,
        // refused). Go's tool gives the same answers for these two inputs.
        let accepted = parse(repeated_list(1000, 110).as_bytes()).unwrap();
        assert_eq!(accepted.project, "demo");
        assert_eq!(
            parse(repeated_list(1000, 111).as_bytes())
                .unwrap_err()
                .to_string(),
            EXCESSIVE
        );
    }

    #[test]
    fn a_few_aliases_still_load() {
        let text = concat!(
            "base: &base {dir: agents, default: true}\n",
            "name: &name demo-a\n",
            "project: *name\n",
            "version: \"1\"\n",
            "targets:\n",
            "    default: *base\n",
            "    other:\n",
            "        <<: *base\n",
            "        dir: other\n",
            "list: [*name, *name, *base]\n",
        );
        let config = parse(text.as_bytes()).unwrap();
        assert_eq!(config.project, "demo-a");
        assert_eq!(config.version, "1");
        assert_eq!(project(&repeated_list(9, 9)), "demo");
    }

    #[test]
    fn an_alias_inside_its_own_anchor_is_refused_like_go() {
        let error = parse(b"project: demo\nloop: &self [a, *self]\n").unwrap_err();
        assert_eq!(
            error.to_string(),
            "yaml: anchor 'self' value contains itself"
        );
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
    fn a_missing_config_is_an_open_error() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let error = load(&root).unwrap_err();
        assert!(
            error.to_string().starts_with(&format!(
                "open {}: ",
                root.join(".luuna/beacon/config.yaml").display()
            )),
            "{error}"
        );
        fs::create_dir_all(paths::store_dir(&root)).unwrap();
        fs::write(paths::config_file(&root), DEFAULT_CONFIG_YAML).unwrap();
        assert_eq!(load(&root).unwrap().project, DEFAULT_PROJECT);
    }
}
