//! Event-based YAML decoding; no recursively owned YAML loader values.
use super::yaml_events::{
    parser::Event,
    parser::Parser,
    scanner::{Marker, Scanner, TScalarStyle, Token, TokenType},
};
use super::{
    yaml_resolve,
    yaml_value::{Node, YamlValue},
};
use crate::{gofmt, BeaconError};
use std::{
    collections::{HashMap, HashSet},
    str::Chars,
};
const MAX_DEPTH: usize = 10_000;
#[derive(Debug)]
struct Raw {
    node: Node,
    children: Vec<usize>,
    alias: Option<(usize, String)>,
    merge: bool,
    key: String,
    line: usize,
    decode_error: Option<String>,
}
pub(super) struct Decoded {
    pub value: YamlValue,
    pub later: bool,
    pub null_key: bool,
}
pub(super) fn decode(text: &str, top_strings: bool) -> Result<Decoded, BeaconError> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    if text
        .chars()
        .any(|c| matches!(c,'\0'..='\u{8}'|'\u{b}'|'\u{c}'|'\u{e}'..='\u{1f}'|'\u{7f}'))
    {
        return Err(BeaconError::Yaml(
            "control characters are not allowed".into(),
        ));
    }
    let mut parser = Parser::new_from_str(text);
    let mut depth = DepthLimit::new(text);
    let mut nodes: Vec<Raw> = Vec::new();
    let mut open: Vec<usize> = Vec::new();
    let mut anchors = HashMap::new();
    let mut root = None;
    let mut later = false;
    loop {
        let (event, mark) = parser
            .next_token()
            .map_err(|e| BeaconError::Yaml(e.to_string()))?;
        depth.advance_to(mark.index())?;
        let mut decode_error = None;
        let (node, anchor, alias, merge, key, opens) = match event {
            Event::StreamEnd => break,
            Event::DocumentEnd => {
                // Later documents are metadata, never decoded.
                let remaining: String = text.chars().skip(mark.index()).collect();
                later = remaining.lines().any(|line| {
                    let line = line.trim();
                    !line.is_empty() && !line.starts_with('#') && line != "..."
                });
                break;
            }
            Event::StreamStart | Event::DocumentStart | Event::Nothing => continue,
            Event::SequenceEnd | Event::MappingEnd => {
                open.pop();
                continue;
            }
            Event::Scalar(s, style, anchor, tag) => {
                let merge = s == "<<"
                    && (tag
                        .as_ref()
                        .is_some_and(|t| t.handle == "tag:yaml.org,2002:" && t.suffix == "merge")
                        || tag.is_none() && style == TScalarStyle::Plain);
                (
                    match yaml_resolve::resolve(&s, style, tag.as_ref()) {
                        Ok(node) => node,
                        Err(BeaconError::Yaml(message)) => {
                            decode_error = Some(message);
                            Node::Null
                        }
                        Err(_) => unreachable!("resolver returns YAML errors"),
                    },
                    anchor,
                    None,
                    merge,
                    s,
                    false,
                )
            }
            Event::SequenceStart(a, _) => {
                (Node::Seq(Vec::new()), a, None, false, String::new(), true)
            }
            Event::MappingStart(a, _) => {
                (Node::Map(Vec::new()), a, None, false, String::new(), true)
            }
            Event::Alias(a) => {
                let target = *anchors
                    .get(&a)
                    .ok_or_else(|| BeaconError::Yaml("unknown anchor".into()))?;
                let rest: String = text.chars().skip(mark.index()).collect();
                let name = rest
                    .trim_start_matches('*')
                    .split(|c: char| c.is_whitespace() || ",]}".contains(c))
                    .next()
                    .unwrap_or("")
                    .to_owned();
                (
                    Node::Null,
                    0,
                    Some((target, name.clone())),
                    false,
                    name,
                    false,
                )
            }
        };
        let id = nodes.len();
        nodes.push(Raw {
            node,
            children: Vec::new(),
            alias,
            merge,
            key,
            line: mark.line(),
            decode_error,
        });
        if anchor > 0 {
            anchors.insert(anchor, id);
        }
        if let Some(&parent) = open.last() {
            nodes[parent].children.push(id)
        } else {
            root = Some(id)
        }
        if opens {
            open.push(id)
        }
    }
    let Some(root) = root else {
        return Ok(Decoded {
            value: YamlValue::null(),
            later,
            null_key: false,
        });
    };
    enum Work {
        Visit(usize, usize, bool),
        Finish(usize, usize, bool),
        Leave(usize),
        Key(usize, usize, bool),
    }
    let mut value = YamlValue::null();
    let mut work = vec![Work::Visit(root, 0, top_strings)];
    let mut active = HashSet::new();
    let mut alias_depth = 0u64;
    let mut count = 1u64;
    let mut aliases = 0u64;
    let mut null_key = false;
    while let Some(step) = work.pop() {
        match step {
            Work::Leave(id) => {
                active.remove(&id);
                alias_depth -= 1
            }
            Work::Visit(raw, out, top) => {
                count += 1;
                if alias_depth > 0 {
                    aliases += 1
                }
                let ratio = if count <= 400000 {
                    0.99
                } else if count >= 4000000 {
                    0.1
                } else {
                    0.99 - 0.89 * (count - 400000) as f64 / 3600000.0
                };
                if aliases > 100 && count > 1000 && aliases as f64 / count as f64 > ratio {
                    return Err(BeaconError::Yaml(
                        "document contains excessive aliasing".into(),
                    ));
                }
                if let Some((target, name)) = &nodes[raw].alias {
                    if !active.insert(raw) {
                        return Err(BeaconError::Yaml(format!(
                            "anchor '{name}' value contains itself"
                        )));
                    }
                    alias_depth += 1;
                    work.push(Work::Leave(raw));
                    work.push(Work::Visit(*target, out, top));
                    continue;
                }
                if let Some(error) = &nodes[raw].decode_error {
                    return Err(BeaconError::Yaml(error.clone()));
                }
                match &nodes[raw].node {
                    Node::Seq(_) => {
                        let mut ids = Vec::new();
                        let mut visits = Vec::new();
                        for &child in &nodes[raw].children {
                            let id = value.push(Node::Null);
                            ids.push(id);
                            visits.push(Work::Visit(child, id, false))
                        }
                        work.extend(visits.into_iter().rev());
                        value.nodes[out] = Node::Seq(ids)
                    }
                    Node::Map(_) => {
                        let mut seen = HashMap::new();
                        for pair in nodes[raw].children.chunks_exact(2) {
                            let k = pair[0];
                            if let Some(first) = seen.insert(
                                (
                                    if nodes[k].alias.is_some() {
                                        3
                                    } else {
                                        match nodes[k].node {
                                            Node::Seq(_) => 1,
                                            Node::Map(_) => 2,
                                            _ => 0,
                                        }
                                    },
                                    nodes[k].key.clone(),
                                ),
                                k,
                            ) {
                                return Err(BeaconError::Yaml(format!("unmarshal errors:\n  line {}: mapping key {} already defined at line {}",nodes[k].line,gofmt::quote(&nodes[k].key),nodes[first].line)));
                            }
                        }
                        let mut pairs = Vec::new();
                        let mut visits = Vec::new();
                        for pair in nodes[raw].children.chunks_exact(2) {
                            let k = value.push(Node::Null);
                            let v = value.push(Node::Null);
                            pairs.push((k, v));
                            visits.push(Work::Visit(pair[0], k, false));
                            visits.push(Work::Key(pair[0], k, top));
                            let mut key = pair[0];
                            while let Some((target, _)) = nodes[key].alias {
                                key = target
                            }
                            if !(top
                                && nodes[key].decode_error.is_none()
                                && matches!(nodes[key].node, Node::Null))
                            {
                                visits.push(Work::Visit(pair[1], v, false));
                            }
                        }
                        value.nodes[out] = Node::Map(pairs);
                        work.push(Work::Finish(raw, out, top));
                        work.extend(visits.into_iter().rev());
                    }
                    node => value.nodes[out] = node.clone(),
                }
            }
            Work::Key(raw, key, top) => {
                if matches!(value.nodes[key], Node::Seq(_) | Node::Map(_)) {
                    if top {
                        return Err(BeaconError::Yaml(format!(
                            "unmarshal errors:\n  line {}: cannot unmarshal !!{} into string",
                            nodes[raw].line,
                            if matches!(value.nodes[key], Node::Seq(_)) {
                                "seq"
                            } else {
                                "map"
                            }
                        )));
                    }
                    return Err(BeaconError::Yaml(format!(
                        "invalid map key: {}",
                        super::yaml_gofmt::value(&value, key)
                    )));
                }
                if top {
                    if matches!(value.nodes[key], Node::Null) {
                        null_key = true
                    } else if !matches!(value.nodes[key], Node::String(_)) {
                        let mut raw = raw;
                        while let Some((target, _)) = nodes[raw].alias {
                            raw = target
                        }
                        value.nodes[key] =
                            Node::String(std::sync::Arc::from(nodes[raw].key.as_bytes()))
                    }
                }
            }
            Work::Finish(raw, out, top) => {
                let Node::Map(pairs) = &value.nodes[out] else {
                    unreachable!()
                };
                let pairs = pairs.clone();
                let mut explicit = Vec::new();
                let mut merged = Vec::new();
                for (pos, (k, v)) in pairs.into_iter().enumerate() {
                    if nodes[nodes[raw].children[pos * 2]].merge {
                        let sources = match &value.nodes[v] {
                            Node::Map(_) => vec![v],
                            Node::Seq(ids) => ids.clone(),
                            _ => {
                                return Err(BeaconError::Yaml(
                                    "map merge requires map or sequence of maps as the value"
                                        .into(),
                                ))
                            }
                        };
                        for src in sources {
                            if let Node::Map(fields) = &value.nodes[src] {
                                merged.extend(fields.clone())
                            } else {
                                return Err(BeaconError::Yaml(
                                    "map merge requires map or sequence of maps as the value"
                                        .into(),
                                ));
                            }
                        }
                    } else if !top || !matches!(value.nodes[k], Node::Null) {
                        explicit.push((k, v))
                    }
                }
                for (k, v) in merged {
                    if !explicit
                        .iter()
                        .any(|(existing, _)| value.nodes[*existing] == value.nodes[k])
                    {
                        explicit.push((k, v))
                    }
                }
                value.nodes[out] = Node::Map(explicit);
            }
        }
    }
    Ok(Decoded {
        value,
        later,
        null_key,
    })
}

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
                TokenType::FlowSequenceStart | TokenType::FlowMappingStart => {
                    self.flow_level += 1;
                    if self.flow_level > MAX_DEPTH {
                        return Err(BeaconError::Yaml(format!(
                            "line {}: exceeded max depth of {MAX_DEPTH}",
                            mark.line()
                        )));
                    }
                }
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
