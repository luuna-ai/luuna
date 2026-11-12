//! The subset of YAML that config.yaml uses, read into the order-preserving JSON value and
//! written the way the Go tool's yaml.v3 encoder writes it.
//!
//! Supported: block maps and sequences, empty and simple (single-line) flow collections,
//! plain, single- and double-quoted scalars (also over several lines), literal and folded
//! block scalars, comments, and one leading `---`. Anchors, aliases, merge keys, tags,
//! complex (`?`) keys, directives and several documents are refused with
//! [`AtlasError::UnsupportedYaml`], which carries the line number. The reader never guesses:
//! anything else it cannot read is a syntax error.
//!
//! As in yaml.v3, at most 10000 block collections and, separately, 10000 flow collections
//! may nest; one more is yaml.v3's "exceeded max depth of 10000" error. Reading, converting,
//! dropping and writing use explicit stacks rather than recursion, so nesting that deep is
//! safe on a thread with the default 2 MiB stack.
//!
//! # Reading
//!
//! Scalars resolve as yaml.v3 resolves them for an untyped Go value (`resolve.go`), and are
//! then turned into the value the Go tool builds (`yamlValue` in `config.go`): `null`,
//! booleans (`true`, `True`, `TRUE`, ...), integers (decimal, `0x`, `0o`, `0b`, a leading
//! `0` for octal, `_` ignored), floats, `.inf`, `.nan`, timestamps (rewritten as
//! `YYYY-MM-DDTHH:MM:SS.mmmZ` from their wall-clock fields) and strings. Mapping keys become
//! the Go tool's key text (`yamlMappingKey`); a key repeated in one mapping is the Go tool's
//! "duplicated mapping key" error with its source snippet.
//!
//! The YAML scanner's line breaks are LF, CR, CRLF, NEL, LS and PS. LS and PS are kept as
//! they are where a break is kept; the others become LF.
//!
//! # Writing
//!
//! [`to_yaml_string`] reproduces the Go tool's `MarshalYAML` (yaml.v3 `Encoder` with
//! `SetIndent(2)`, fed by `yamlNode`): block mappings and sequences, `{}` and `[]` for empty
//! ones, two-space indentation (four after a `- ` that opens a nested collection), no line
//! wrapping. A string is first encoded alone and read back, as yaml.v3's `Node.Encode`
//! does, and the style found is kept, except that a string without NUL, tab, CR or LF is
//! single-quoted when yaml.v3 double-quoted it or when it reads as y, yes, n, no, true,
//! false, on or off in any case. The emitter's choice between plain, single-quoted,
//! double-quoted and literal styles, its escapes and its block scalar indicators follow
//! `emitterc.go`.
//!
//! An integer is read exactly into [`Value::Integer`], as yaml.v3 decodes it to a Go `int`
//! or `uint64`, and written back as its decimal digits. A float is read into
//! [`Value::Number`] and written as yaml.v3 writes a Go `float64` (`strconv.FormatFloat`
//! with format `'g'`: `1.5`, `1e+06`, `1e+21`, `.inf`, `.nan`).

use crate::error::{AtlasError, Result};
use crate::json::{Object, Value};

// =======================================================================================
// Reading
// =======================================================================================

/// Reads one YAML document. An empty document (only blank lines and comments) is `null`.
pub fn parse(text: &str) -> Result<Value> {
    let mut parser = Parser::new(text)?;
    match parser.document()? {
        None => Ok(Value::Null),
        Some(node) => to_value(node, text),
    }
}

/// How a scalar was written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Style {
    Plain,
    Single,
    Double,
    Literal,
    Folded,
}

/// A parsed node, before conversion to [`Value`]. Scalars keep their raw text and style so
/// that resolution happens as in yaml.v3: only plain scalars are resolved.
///
/// Nodes nest as deep as yaml.v3 allows (see [`MAX_DEPTH`]), so dropping one uses an
/// explicit stack rather than recursion; contents are taken out with [`std::mem::take`].
#[derive(Debug)]
enum Node {
    Scalar(String, Style),
    Seq(Vec<Node>),
    Map(Vec<(Key, Node)>),
}

impl Node {
    fn empty() -> Node {
        Node::Scalar(String::new(), Style::Plain)
    }
}

impl Drop for Node {
    fn drop(&mut self) {
        let nested = |n: &Node| matches!(n, Node::Seq(_) | Node::Map(_));
        let mut stack: Vec<Node> = match self {
            Node::Seq(items) if items.iter().any(nested) => std::mem::take(items),
            Node::Map(entries) if entries.iter().any(|(_, n)| nested(n)) => std::mem::take(entries)
                .into_iter()
                .map(|(_, n)| n)
                .collect(),
            _ => return,
        };
        while let Some(mut node) = stack.pop() {
            match &mut node {
                Node::Seq(items) => stack.append(items),
                Node::Map(entries) => stack.extend(entries.drain(..).map(|(_, n)| n)),
                Node::Scalar(..) => {}
            }
            // `node` holds no children now, so dropping it does not recurse.
        }
    }
}

/// yaml.v3's nesting limits (`max_indents` and `max_flow_level` in `scannerc.go`): at most
/// this many nested block collections, and separately at most this many nested flow
/// collections. One more is the error "exceeded max depth of 10000".
const MAX_DEPTH: usize = 10_000;

/// yaml.v3's "exceeded max depth" scanner error. `marks` are the 0-based lines of the
/// scanner's context mark, then its problem mark; as `parser.fail` does, the first one
/// that is not line 0 is reported (1-based), and none is reported when both are line 0.
fn depth_error(marks: [usize; 2]) -> AtlasError {
    let message = format!("exceeded max depth of {MAX_DEPTH}");
    match marks.into_iter().find(|&line| line != 0) {
        Some(line) => syntax(line, &message),
        None => AtlasError::Message(format!("yaml: {message}")),
    }
}

/// A block collection whose entries are being read (the iterative form of the reader's
/// block recursion).
enum Block {
    /// A block mapping at byte `col`, its entries so far and the key whose value is next.
    Map {
        col: usize,
        entries: Vec<(Key, Node)>,
        key: Option<Key>,
    },
    /// A block sequence whose `-` entries are at byte `col`.
    Seq {
        col: usize,
        indentless: bool,
        items: Vec<Node>,
    },
}

/// What the block reader does next.
enum Step {
    /// Read the node at byte `col` of `line`; the enclosing indentation is `parent`.
    Node(usize, usize, isize),
    /// Open an indentless sequence whose entries are at byte `col` of `line`.
    Indentless(usize, usize),
    /// A node is complete.
    Done(Node),
}

/// A flow collection whose entries are being read.
enum Flow {
    Seq(Vec<Node>),
    /// The entries so far and the key whose value is being read.
    Map(Vec<(Key, Node)>, Option<Key>),
}

/// What the flow reader does next.
enum FlowStep {
    /// Read a node at the current byte.
    Node,
    /// Read the next entry of the innermost open collection, or its end.
    Entry,
    /// A node is complete.
    Done(Node),
}

/// A mapping key: always a scalar here (complex keys are refused).
#[derive(Debug)]
struct Key {
    raw: String,
    style: Style,
    /// 1-based line, as yaml.v3 counts lines.
    line: usize,
    /// 1-based column in characters, as yaml.v3 counts columns.
    column: usize,
}

/// One source line without its break, and the break that ended it as the YAML scanner
/// reports it: "\n" for LF, CR, CRLF and NEL, LS and PS as themselves, "" at the end of
/// the input.
struct Line<'a> {
    text: &'a str,
    brk: &'static str,
}

fn split_lines(text: &str) -> Vec<Line<'_>> {
    let mut lines = Vec::new();
    let mut start = 0;
    let mut chars = text.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        let (brk, end) = match c {
            '\n' | '\u{85}' => ("\n", i + c.len_utf8()),
            '\r' => match chars.peek() {
                Some(&(j, '\n')) => {
                    chars.next();
                    ("\n", j + 1)
                }
                _ => ("\n", i + 1),
            },
            '\u{2028}' => ("\u{2028}", i + 3),
            '\u{2029}' => ("\u{2029}", i + 3),
            _ => continue,
        };
        lines.push(Line {
            text: &text[start..i],
            brk,
        });
        start = end;
    }
    if start < text.len() {
        lines.push(Line {
            text: &text[start..],
            brk: "",
        });
    }
    lines
}

/// Whether `c` may appear in a YAML stream (libyaml's reader check).
fn allowed_char(c: char) -> bool {
    matches!(c,
        '\t' | '\n' | '\r' | '\u{20}'..='\u{7E}' | '\u{85}' | '\u{A0}'..='\u{D7FF}'
        | '\u{E000}'..='\u{FFFD}' | '\u{10000}'..='\u{10FFFF}')
}

fn is_ws(b: u8) -> bool {
    b == b' ' || b == b'\t'
}

/// The index of the first byte at or after `i` that is not a space or tab.
fn skip_ws(text: &str, mut i: usize) -> usize {
    let bytes = text.as_bytes();
    while i < bytes.len() && is_ws(bytes[i]) {
        i += 1;
    }
    i
}

/// Whether the byte after `i` ends a token: end of line, space or tab.
fn blank_after(text: &str, i: usize) -> bool {
    text.as_bytes().get(i + 1).is_none_or(|&b| is_ws(b))
}

/// Whether `rest` starts a block sequence entry: `-` then a blank or the end of the line.
fn is_seq_entry(rest: &str) -> bool {
    rest.starts_with('-') && blank_after(rest, 0)
}

/// 1-based character column of byte index `i` in `text`.
fn column_of(text: &str, i: usize) -> usize {
    text[..i].chars().count() + 1
}

fn syntax(line_index: usize, message: &str) -> AtlasError {
    AtlasError::Message(format!("yaml: line {}: {message}", line_index + 1))
}

fn unsupported(line_index: usize, message: &str) -> AtlasError {
    AtlasError::UnsupportedYaml {
        line: line_index + 1,
        message: message.to_owned(),
    }
}

/// The error for a node that starts with an anchor, alias or tag indicator, if `c` is one.
fn refused_indicator(line_index: usize, c: u8) -> Option<AtlasError> {
    let message = match c {
        b'&' => "anchors (&) are not supported",
        b'*' => "aliases (*) are not supported",
        b'!' => "tags (!) are not supported",
        _ => return None,
    };
    Some(unsupported(line_index, message))
}

struct Parser<'a> {
    lines: Vec<Line<'a>>,
    /// The next line not yet consumed.
    pos: usize,
    /// The 0-based line of the last token that could start a simple key in block context
    /// (a scalar or the opening bracket of a flow collection): yaml.v3's
    /// `simple_keys[len-1].mark`, which a block sequence's depth error reports. Line 0
    /// until there is one, as the stream start's zero mark.
    simple_key_line: std::cell::Cell<usize>,
}

impl<'a> Parser<'a> {
    fn new(text: &'a str) -> Result<Self> {
        let text = text.strip_prefix('\u{FEFF}').unwrap_or(text);
        let lines = split_lines(text);
        for (index, line) in lines.iter().enumerate() {
            if !line.text.chars().all(allowed_char) {
                return Err(syntax(index, "control characters are not allowed"));
            }
        }
        Ok(Parser {
            lines,
            pos: 0,
            simple_key_line: std::cell::Cell::new(0),
        })
    }

    fn text(&self, index: usize) -> &'a str {
        self.lines[index].text
    }

    /// Whether a line holds only blanks or a comment.
    fn is_blank(&self, index: usize) -> bool {
        let text = self.text(index);
        let i = skip_ws(text, 0);
        i == text.len() || text.as_bytes()[i] == b'#'
    }

    fn skip_blank(&mut self) {
        while self.pos < self.lines.len() && self.is_blank(self.pos) {
            self.pos += 1;
        }
    }

    /// Whether the line is the document marker `marker` (`---` or `...`) at column 0.
    fn is_marker(&self, index: usize, marker: &str) -> bool {
        let text = self.text(index);
        text.starts_with(marker) && blank_after(text, 2)
    }

    fn is_doc_marker(&self, index: usize) -> bool {
        self.is_marker(index, "---") || self.is_marker(index, "...")
    }

    /// The indentation (leading spaces) of a content line. A tab in the indentation is an
    /// error, as in libyaml.
    fn indent(&self, index: usize) -> Result<usize> {
        let text = self.text(index);
        let n = text.bytes().take_while(|&b| b == b' ').count();
        if text.as_bytes().get(n) == Some(&b'\t') && !self.is_blank(index) {
            return Err(syntax(index, "found character that cannot start any token"));
        }
        Ok(n)
    }

    /// The next content line, if it is not past the end and not a document marker.
    fn next_content(&mut self) -> Option<usize> {
        self.skip_blank();
        (self.pos < self.lines.len() && !self.is_doc_marker(self.pos)).then_some(self.pos)
    }

    fn document(&mut self) -> Result<Option<Node>> {
        self.skip_blank();
        if self.pos < self.lines.len() && self.text(self.pos).starts_with('%') {
            return Err(unsupported(
                self.pos,
                "directives (%YAML, %TAG) are not supported",
            ));
        }
        if self.pos < self.lines.len() && self.is_marker(self.pos, "---") {
            let text = self.text(self.pos);
            let rest = skip_ws(text, 3);
            if rest < text.len() && text.as_bytes()[rest] != b'#' {
                return Err(unsupported(
                    self.pos,
                    "content on the document start line (---) is not supported",
                ));
            }
            self.pos += 1;
            self.skip_blank();
        }
        if self.pos >= self.lines.len() {
            return Ok(None);
        }
        let node = if self.is_marker(self.pos, "...") || self.is_marker(self.pos, "---") {
            None
        } else {
            let line = self.pos;
            let indent = self.indent(line)?;
            Some(self.block(line, indent, -1)?)
        };
        self.skip_blank();
        if self.pos < self.lines.len() {
            if self.is_marker(self.pos, "...") {
                self.pos += 1;
                self.skip_blank();
                if self.pos < self.lines.len() {
                    return Err(unsupported(
                        self.pos,
                        "several documents in one file are not supported",
                    ));
                }
            } else if self.is_marker(self.pos, "---") {
                return Err(unsupported(
                    self.pos,
                    "several documents in one file are not supported",
                ));
            } else {
                // Content indented less than the root node: yaml.v3 reads it as the start
                // of a second document and never reads it.
                return Err(unsupported(
                    self.pos,
                    "content after the end of the document is not supported",
                ));
            }
        }
        Ok(node)
    }

    /// A node that starts at byte `col` of line `line`: a block sequence, a block mapping
    /// or an inline value. `parent` is the indentation of the enclosing block collection
    /// (-1 at the root).
    ///
    /// Nested block collections are read with an explicit stack of [`Block`]s rather than
    /// recursion. As in yaml.v3, more than [`MAX_DEPTH`] nested block collections (an
    /// indentless sequence does not count) is an error.
    fn block(&mut self, line: usize, col: usize, parent: isize) -> Result<Node> {
        let mut open: Vec<Block> = Vec::new();
        // The block collections yaml.v3 has pushed an indentation level for.
        let mut depth = 0usize;
        let mut step = Step::Node(line, col, parent);
        loop {
            step = match step {
                Step::Node(line, col, parent) => {
                    if is_seq_entry(&self.text(line)[col..]) {
                        // yaml.v3 checks the depth at the `-`; the context mark is the
                        // last simple key, the problem mark the `-`.
                        if depth == MAX_DEPTH {
                            return Err(depth_error([self.simple_key_line.get(), line]));
                        }
                        depth += 1;
                        open.push(Block::Seq {
                            col,
                            indentless: false,
                            items: Vec::new(),
                        });
                        self.sequence_entry(line, col)?
                    } else if let Some((key, after)) = self.key(line, col)? {
                        // Checked at the `:`, with the key as the context mark.
                        if depth == MAX_DEPTH {
                            return Err(depth_error([key.line - 1, line]));
                        }
                        depth += 1;
                        open.push(Block::Map {
                            col,
                            entries: Vec::new(),
                            key: Some(key),
                        });
                        self.mapping_value(line, after, col)?
                    } else {
                        Step::Done(self.value(line, col, parent)?)
                    }
                }
                Step::Indentless(line, col) => {
                    open.push(Block::Seq {
                        col,
                        indentless: true,
                        items: Vec::new(),
                    });
                    self.sequence_entry(line, col)?
                }
                Step::Done(node) => {
                    let Some(top) = open.last_mut() else {
                        return Ok(node);
                    };
                    let next = match top {
                        Block::Map { col, entries, key } => {
                            entries.push((key.take().expect("a key awaits its value"), node));
                            match self.next_key(*col)? {
                                Some((next, found, after)) => {
                                    let col = *col;
                                    *key = Some(found);
                                    Some(self.mapping_value(next, after, col)?)
                                }
                                None => None,
                            }
                        }
                        Block::Seq {
                            col,
                            indentless,
                            items,
                        } => {
                            items.push(node);
                            match self.next_entry(*col, *indentless)? {
                                Some(next) => {
                                    let col = *col;
                                    Some(self.sequence_entry(next, col)?)
                                }
                                None => None,
                            }
                        }
                    };
                    match next {
                        Some(step) => step,
                        None => match open.pop() {
                            Some(Block::Map { entries, .. }) => {
                                depth -= 1;
                                Step::Done(Node::Map(entries))
                            }
                            Some(Block::Seq {
                                indentless, items, ..
                            }) => {
                                if !indentless {
                                    depth -= 1;
                                }
                                Step::Done(Node::Seq(items))
                            }
                            None => unreachable!("the top collection was just read"),
                        },
                    }
                }
            };
        }
    }

    /// The key at byte `col` of `line`, with the byte index after its `:`, if the line holds
    /// a `key:` there.
    fn key(&self, line: usize, col: usize) -> Result<Option<(Key, usize)>> {
        let text = self.text(line);
        let bytes = text.as_bytes();
        let first = bytes[col];
        if !matches!(first, b'|' | b'>') {
            self.simple_key_line.set(line);
        }
        if let Some(error) = refused_indicator(line, first) {
            return Err(error);
        }
        if first == b'?' && blank_after(text, col) {
            return Err(unsupported(
                line,
                "complex mapping keys (?) are not supported",
            ));
        }
        let (raw, style, end) = match first {
            b'[' | b'{' => return Ok(None),
            b'"' | b'\'' => match self.quoted(line, col, true)? {
                Some((raw, _, end)) => {
                    let style = if first == b'"' {
                        Style::Double
                    } else {
                        Style::Single
                    };
                    (raw, style, skip_ws(text, end))
                }
                None => return Ok(None),
            },
            b',' | b']' | b'}' | b'#' | b'|' | b'>' | b'%' | b'@' | b'`' => return Ok(None),
            _ => {
                let mut i = col;
                let mut found = None;
                while i < bytes.len() {
                    match bytes[i] {
                        b'#' if i > col && is_ws(bytes[i - 1]) => break,
                        b':' if blank_after(text, i) => {
                            found = Some(i);
                            break;
                        }
                        _ => i += 1,
                    }
                }
                let Some(colon) = found else {
                    return Ok(None);
                };
                let raw = text[col..colon].trim_end_matches([' ', '\t']);
                if raw.is_empty() {
                    return Err(unsupported(line, "empty mapping keys are not supported"));
                }
                if raw == "<<" {
                    return Err(unsupported(line, "merge keys (<<) are not supported"));
                }
                (raw.to_owned(), Style::Plain, colon)
            }
        };
        if bytes.get(end) == Some(&b':') && blank_after(text, end) {
            let key = Key {
                raw,
                style,
                line: line + 1,
                column: column_of(text, col),
            };
            Ok(Some((key, end + 1)))
        } else {
            Ok(None)
        }
    }

    /// After a mapping entry: the next key of the mapping indented `col`, its line and the
    /// byte after its `:`, or `None` where the mapping ends.
    fn next_key(&mut self, col: usize) -> Result<Option<(usize, Key, usize)>> {
        let Some(next) = self.next_content() else {
            return Ok(None);
        };
        let indent = self.indent(next)?;
        if indent < col {
            return Ok(None);
        }
        if indent > col {
            return Err(syntax(next, "did not find expected key"));
        }
        match self.key(next, col)? {
            Some((key, after)) => Ok(Some((next, key, after))),
            None => {
                self.refuse_flow_key(next, col)?;
                Err(syntax(next, "did not find expected key"))
            }
        }
    }

    /// Refuses a one-line flow collection followed by `:` at byte `col` of `line`, where a
    /// later key of a block mapping is expected. yaml.v3 reads it as a mapping key (as it
    /// does for a mapping's first key, which [`Parser::line_end`] refuses), so it is
    /// unsupported, not a syntax error. Anything else, including a flow collection that
    /// does not parse on its line, is left to the caller's syntax error.
    fn refuse_flow_key(&self, line: usize, col: usize) -> Result<()> {
        if !matches!(self.text(line).as_bytes()[col], b'[' | b'{') {
            return Ok(());
        }
        let mut i = col;
        if self.flow_node(line, &mut i).is_err() {
            return Ok(());
        }
        let text = self.text(line);
        let rest = skip_ws(text, i);
        if text.as_bytes().get(rest) == Some(&b':') {
            return Err(unsupported(
                line,
                "flow collections as mapping keys are not supported",
            ));
        }
        Ok(())
    }

    /// The value after a key's `:` at byte `after` of `line`, in a mapping indented `col`.
    fn mapping_value(&mut self, line: usize, after: usize, col: usize) -> Result<Step> {
        let text = self.text(line);
        let start = skip_ws(text, after);
        self.pos = line + 1;
        if start >= text.len() || text.as_bytes()[start] == b'#' {
            if let Some(next) = self.next_content() {
                let indent = self.indent(next)?;
                if indent > col {
                    return Ok(Step::Node(next, indent, col as isize));
                }
                if indent == col && is_seq_entry(&self.text(next)[indent..]) {
                    return Ok(Step::Indentless(next, indent));
                }
            }
            return Ok(Step::Done(Node::empty()));
        }
        if is_seq_entry(&text[start..]) {
            return Err(syntax(
                line,
                "block sequence entries are not allowed in this context",
            ));
        }
        Ok(Step::Done(self.value(line, start, col as isize)?))
    }

    /// The entry whose `-` is at byte `col` of `line`, in a block sequence. An indentless
    /// sequence (the value of a key at the same indentation) ends at the first line that
    /// is not an entry.
    fn sequence_entry(&mut self, line: usize, col: usize) -> Result<Step> {
        let text = self.text(line);
        let start = skip_ws(text, col + 1);
        self.pos = line + 1;
        if start >= text.len() || text.as_bytes()[start] == b'#' {
            return Ok(match self.next_content() {
                Some(next) => {
                    let indent = self.indent(next)?;
                    if indent > col {
                        Step::Node(next, indent, col as isize)
                    } else {
                        Step::Done(Node::empty())
                    }
                }
                None => Step::Done(Node::empty()),
            });
        }
        Ok(Step::Node(line, start, col as isize))
    }

    /// After a sequence entry: the line of the next `-` of the sequence at byte `col`, or
    /// `None` where the sequence ends.
    fn next_entry(&mut self, col: usize, indentless: bool) -> Result<Option<usize>> {
        let Some(next) = self.next_content() else {
            return Ok(None);
        };
        let indent = self.indent(next)?;
        if indent < col {
            return Ok(None);
        }
        if indent > col {
            return Err(syntax(next, "did not find expected '-' indicator"));
        }
        if !is_seq_entry(&self.text(next)[col..]) {
            if indentless {
                return Ok(None);
            }
            return Err(syntax(next, "did not find expected '-' indicator"));
        }
        Ok(Some(next))
    }

    /// An inline value at byte `start` of `line`: a scalar of any style or a flow
    /// collection.
    fn value(&mut self, line: usize, start: usize, parent: isize) -> Result<Node> {
        let text = self.text(line);
        let first = text.as_bytes()[start];
        if !matches!(first, b'|' | b'>') {
            self.simple_key_line.set(line);
        }
        if let Some(error) = refused_indicator(line, first) {
            return Err(error);
        }
        match first {
            b'|' | b'>' => self.block_scalar(line, start, parent),
            b'[' | b'{' => {
                let mut i = start;
                let node = self.flow_node(line, &mut i)?;
                self.pos = line + 1;
                self.line_end(line, i, true)?;
                Ok(node)
            }
            b'"' | b'\'' => {
                let style = if first == b'"' {
                    Style::Double
                } else {
                    Style::Single
                };
                let Some((raw, end_line, end)) = self.quoted(line, start, false)? else {
                    unreachable!("a quoted scalar over several lines always ends or fails");
                };
                self.pos = end_line + 1;
                self.line_end(end_line, end, false)?;
                Ok(Node::Scalar(raw, style))
            }
            b'?' if blank_after(text, start) => Err(unsupported(
                line,
                "complex mapping keys (?) are not supported",
            )),
            b'%' | b'@' | b'`' => Err(syntax(line, "found character that cannot start any token")),
            b',' | b']' | b'}' => Err(syntax(line, "did not find expected node content")),
            b':' if blank_after(text, start) => Err(syntax(
                line,
                "mapping values are not allowed in this context",
            )),
            _ => self.plain(line, start, parent),
        }
    }

    /// Checks that only blanks or a comment follow byte `i` of `line`.
    fn line_end(&self, line: usize, i: usize, flow: bool) -> Result<()> {
        let text = self.text(line);
        let rest = skip_ws(text, i);
        // After a quoted scalar or a flow collection the scanner is between tokens, so a
        // `#` starts a comment even without a blank before it.
        if rest >= text.len() || text.as_bytes()[rest] == b'#' {
            return Ok(());
        }
        if text.as_bytes()[rest] == b':' {
            if flow {
                return Err(unsupported(
                    line,
                    "flow collections as mapping keys are not supported",
                ));
            }
            return Err(syntax(
                line,
                "mapping values are not allowed in this context",
            ));
        }
        Err(syntax(line, "did not find expected key"))
    }

    /// The text of a plain scalar on one line from byte `start`, and whether a comment
    /// ended it.
    fn plain_piece(&self, line: usize, start: usize) -> Result<(&'a str, bool)> {
        let text = self.text(line);
        let bytes = text.as_bytes();
        let mut i = start;
        let mut comment = false;
        while i < bytes.len() {
            match bytes[i] {
                b'#' if i > start && is_ws(bytes[i - 1]) => {
                    comment = true;
                    break;
                }
                b':' if blank_after(text, i) => {
                    return Err(syntax(
                        line,
                        "mapping values are not allowed in this context",
                    ));
                }
                _ => i += 1,
            }
        }
        Ok((text[start..i].trim_end_matches([' ', '\t']), comment))
    }

    /// A plain scalar in block context, continued on following lines indented more than
    /// `parent` and folded as libyaml folds them.
    fn plain(&mut self, line: usize, start: usize, parent: isize) -> Result<Node> {
        let (first, mut ended) = self.plain_piece(line, start)?;
        let mut value = first.to_owned();
        let mut last = line;
        let mut next = line + 1;
        while !ended && next < self.lines.len() {
            let text = self.text(next);
            let content = skip_ws(text, 0);
            if content == text.len() {
                next += 1;
                continue;
            }
            if self.is_doc_marker(next) {
                break;
            }
            let spaces = text.bytes().take_while(|&b| b == b' ').count();
            if spaces as isize <= parent || text.as_bytes()[content] == b'#' {
                break;
            }
            let (piece, comment) = self.plain_piece(next, content)?;
            let leading = self.lines[last].brk;
            let trailing: String = (last + 1..next).map(|i| self.lines[i].brk).collect();
            if leading == "\n" {
                if trailing.is_empty() {
                    value.push(' ');
                } else {
                    value.push_str(&trailing);
                }
            } else {
                value.push_str(leading);
                value.push_str(&trailing);
            }
            value.push_str(piece);
            last = next;
            next += 1;
            ended = comment;
        }
        self.pos = last + 1;
        Ok(Node::Scalar(value, Style::Plain))
    }

    /// A quoted scalar whose opening quote is at byte `start` of `line`. Returns its value,
    /// the line it ends on and the byte after its closing quote. With `single_line`, a
    /// scalar that does not close on its line gives `None`.
    fn quoted(
        &self,
        mut line: usize,
        start: usize,
        single_line: bool,
    ) -> Result<Option<(String, usize, usize)>> {
        let text = self.text(line);
        let single = text.as_bytes()[start] == b'\'';
        let quote = if single { '\'' } else { '"' };
        let mut out = String::new();
        let mut i = start + 1;
        loop {
            let text = self.text(line);
            let mut whitespace = String::new();
            let mut escaped_break = false;
            while i < text.len() {
                let c = text[i..].chars().next().unwrap_or_default();
                if single && c == '\'' && text[i + 1..].starts_with('\'') {
                    out.push_str(&whitespace);
                    whitespace.clear();
                    out.push('\'');
                    i += 2;
                } else if c == quote {
                    out.push_str(&whitespace);
                    return Ok(Some((out, line, i + 1)));
                } else if !single && c == '\\' {
                    out.push_str(&whitespace);
                    whitespace.clear();
                    if i + 1 == text.len() && !self.lines[line].brk.is_empty() {
                        escaped_break = true;
                        break;
                    }
                    i = self.escape(line, text, i, &mut out)?;
                } else if c == ' ' || c == '\t' {
                    whitespace.push(c);
                    i += 1;
                } else {
                    out.push_str(&whitespace);
                    whitespace.clear();
                    out.push(c);
                    i += c.len_utf8();
                }
            }
            if single_line {
                return Ok(None);
            }
            if self.lines[line].brk.is_empty() {
                return Err(syntax(line, "found unexpected end of stream"));
            }
            let leading = if escaped_break {
                ""
            } else {
                self.lines[line].brk
            };
            let mut trailing = String::new();
            line += 1;
            loop {
                if line >= self.lines.len() {
                    return Err(syntax(line - 1, "found unexpected end of stream"));
                }
                if self.is_doc_marker(line) {
                    return Err(syntax(line, "found unexpected document indicator"));
                }
                let text = self.text(line);
                let content = skip_ws(text, 0);
                if content < text.len() {
                    i = content;
                    break;
                }
                if self.lines[line].brk.is_empty() {
                    return Err(syntax(line, "found unexpected end of stream"));
                }
                trailing.push_str(self.lines[line].brk);
                line += 1;
            }
            if leading == "\n" {
                if trailing.is_empty() {
                    out.push(' ');
                } else {
                    out.push_str(&trailing);
                }
            } else {
                out.push_str(leading);
                out.push_str(&trailing);
            }
        }
    }

    /// Decodes the escape whose backslash is at byte `i` of `text` into `out`, and returns
    /// the byte after it.
    fn escape(&self, line: usize, text: &str, i: usize, out: &mut String) -> Result<usize> {
        let Some(c) = text[i + 1..].chars().next() else {
            return Err(syntax(line, "found unknown escape character"));
        };
        let simple = match c {
            '0' => Some('\0'),
            'a' => Some('\u{7}'),
            'b' => Some('\u{8}'),
            't' | '\t' => Some('\t'),
            'n' => Some('\n'),
            'v' => Some('\u{B}'),
            'f' => Some('\u{C}'),
            'r' => Some('\r'),
            'e' => Some('\u{1B}'),
            ' ' => Some(' '),
            '"' => Some('"'),
            '\'' => Some('\''),
            '\\' => Some('\\'),
            'N' => Some('\u{85}'),
            '_' => Some('\u{A0}'),
            'L' => Some('\u{2028}'),
            'P' => Some('\u{2029}'),
            'x' | 'u' | 'U' => None,
            _ => return Err(syntax(line, "found unknown escape character")),
        };
        if let Some(decoded) = simple {
            out.push(decoded);
            return Ok(i + 1 + c.len_utf8());
        }
        let length = match c {
            'x' => 2,
            'u' => 4,
            _ => 8,
        };
        let digits = text.get(i + 2..i + 2 + length).unwrap_or_default();
        if digits.len() != length || !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(syntax(line, "did not find expected hexdecimal number"));
        }
        let code = u32::from_str_radix(digits, 16).unwrap_or(u32::MAX);
        match char::from_u32(code) {
            Some(decoded) => out.push(decoded),
            None => {
                return Err(syntax(line, "found invalid Unicode character escape code"));
            }
        }
        Ok(i + 2 + length)
    }

    /// A literal (`|`) or folded (`>`) block scalar whose indicator is at byte `start` of
    /// `line`, scanned as libyaml's `yaml_parser_scan_block_scalar` does.
    fn block_scalar(&mut self, line: usize, start: usize, parent: isize) -> Result<Node> {
        let text = self.text(line);
        let bytes = text.as_bytes();
        let literal = bytes[start] == b'|';
        let mut chomp = 0i8;
        let mut increment = 0usize;
        let mut i = start + 1;
        for _ in 0..2 {
            match bytes.get(i) {
                Some(b'+') if chomp == 0 => chomp = 1,
                Some(b'-') if chomp == 0 => chomp = -1,
                Some(b'0') if increment == 0 => {
                    return Err(syntax(line, "found an indentation indicator equal to 0"));
                }
                Some(&d @ b'1'..=b'9') if increment == 0 => increment = usize::from(d - b'0'),
                _ => break,
            }
            i += 1;
        }
        let i = skip_ws(text, i);
        if i < bytes.len() && bytes[i] != b'#' {
            return Err(syntax(line, "did not find expected comment or line break"));
        }
        let mut indent = if increment == 0 {
            0
        } else if parent >= 0 {
            parent as usize + increment
        } else {
            increment
        };
        let (mut trailing, mut column, mut next) =
            self.block_breaks(line + 1, &mut indent, parent)?;
        let mut out = String::new();
        let mut leading_break = "";
        let mut leading_blank = false;
        while next < self.lines.len() && column == Some(indent) {
            let content = &self.text(next)[indent..];
            let trailing_blank = content.starts_with([' ', '\t']);
            if !literal && leading_break == "\n" && !leading_blank && !trailing_blank {
                if trailing.is_empty() {
                    out.push(' ');
                }
                leading_break = "";
            }
            out.push_str(leading_break);
            out.push_str(&trailing);
            leading_blank = trailing_blank;
            out.push_str(content);
            leading_break = self.lines[next].brk;
            if leading_break.is_empty() {
                next += 1;
                trailing = String::new();
                break;
            }
            (trailing, column, next) = self.block_breaks(next + 1, &mut indent, parent)?;
        }
        if chomp != -1 {
            out.push_str(leading_break);
        }
        if chomp == 1 {
            out.push_str(&trailing);
        }
        self.pos = next;
        let style = if literal {
            Style::Literal
        } else {
            Style::Folded
        };
        Ok(Node::Scalar(out, style))
    }

    /// libyaml's `yaml_parser_scan_block_scalar_breaks` from line `next`: consumes the empty
    /// lines of a block scalar and returns their breaks, the column at which the next
    /// content line starts (`None` at the end of the input) and that line. With `*indent`
    /// 0 it detects the indentation.
    fn block_breaks(
        &self,
        mut next: usize,
        indent: &mut usize,
        parent: isize,
    ) -> Result<(String, Option<usize>, usize)> {
        let auto = *indent == 0;
        let mut max_indent = 0;
        let mut breaks = String::new();
        let mut column = None;
        while next < self.lines.len() {
            let bytes = self.text(next).as_bytes();
            let mut k = 0;
            while (auto || k < *indent) && k < bytes.len() && bytes[k] == b' ' {
                k += 1;
            }
            max_indent = max_indent.max(k);
            if (auto || k < *indent) && bytes.get(k) == Some(&b'\t') {
                return Err(syntax(
                    next,
                    "found a tab character where an indentation space is expected",
                ));
            }
            if k < bytes.len() || self.lines[next].brk.is_empty() {
                if k < bytes.len() {
                    column = Some(k);
                }
                break;
            }
            breaks.push_str(self.lines[next].brk);
            next += 1;
        }
        if auto {
            let minimum = usize::try_from(parent + 1).unwrap_or(0).max(1);
            *indent = max_indent.max(minimum);
        }
        Ok((breaks, column, next))
    }

    /// A flow node on one line at byte `*i`, which is advanced past it.
    ///
    /// Nested flow collections are read with an explicit stack of [`Flow`]s rather than
    /// recursion. As in yaml.v3, more than [`MAX_DEPTH`] nested flow collections is an
    /// error, reported at the line of the bracket that opens one too many.
    fn flow_node(&self, line: usize, i: &mut usize) -> Result<Node> {
        let text = self.text(line);
        let bytes = text.as_bytes();
        let mut open: Vec<Flow> = Vec::new();
        let mut step = FlowStep::Node;
        loop {
            step = match step {
                FlowStep::Node => {
                    *i = skip_ws(text, *i);
                    match bytes.get(*i) {
                        Some(&bracket @ (b'[' | b'{')) => {
                            if open.len() == MAX_DEPTH {
                                return Err(depth_error([line, line]));
                            }
                            *i += 1;
                            open.push(if bracket == b'[' {
                                Flow::Seq(Vec::new())
                            } else {
                                Flow::Map(Vec::new(), None)
                            });
                            FlowStep::Entry
                        }
                        _ => FlowStep::Done(self.flow_scalar(line, i)?),
                    }
                }
                FlowStep::Entry => {
                    *i = skip_ws(text, *i);
                    let depth = open.len();
                    match open.last_mut().expect("a flow collection is open") {
                        Flow::Seq(_) => match bytes.get(*i) {
                            None | Some(b'#') => return Err(multi_line_flow(line)),
                            Some(b']') => {
                                *i += 1;
                                let Some(Flow::Seq(items)) = open.pop() else {
                                    unreachable!("the innermost collection is a sequence");
                                };
                                FlowStep::Done(Node::Seq(items))
                            }
                            _ => FlowStep::Node,
                        },
                        Flow::Map(_, pending) => match bytes.get(*i) {
                            None | Some(b'#') => return Err(multi_line_flow(line)),
                            Some(b'}') => {
                                *i += 1;
                                let Some(Flow::Map(entries, _)) = open.pop() else {
                                    unreachable!("the innermost collection is a mapping");
                                };
                                FlowStep::Done(Node::Map(entries))
                            }
                            Some(b'[' | b'{') => {
                                // yaml.v3's scanner counts the bracket before its parser
                                // sees a collection used as a key.
                                if depth == MAX_DEPTH {
                                    return Err(depth_error([line, line]));
                                }
                                return Err(unsupported(
                                    line,
                                    "flow collections as mapping keys are not supported",
                                ));
                            }
                            Some(b'?') if blank_after(text, *i) => {
                                return Err(unsupported(
                                    line,
                                    "complex mapping keys (?) are not supported",
                                ));
                            }
                            _ => {
                                let key_start = *i;
                                let mut scalar = self.flow_scalar(line, i)?;
                                let Node::Scalar(raw, style) = &mut scalar else {
                                    unreachable!("only a scalar can start with this character");
                                };
                                if *style == Style::Plain && raw == "<<" {
                                    return Err(unsupported(
                                        line,
                                        "merge keys (<<) are not supported",
                                    ));
                                }
                                *pending = Some(Key {
                                    raw: std::mem::take(raw),
                                    style: *style,
                                    line: line + 1,
                                    column: column_of(text, key_start),
                                });
                                *i = skip_ws(text, *i);
                                if bytes.get(*i) == Some(&b':') {
                                    *i = skip_ws(text, *i + 1);
                                    match bytes.get(*i) {
                                        Some(b',' | b'}') => FlowStep::Done(Node::empty()),
                                        _ => FlowStep::Node,
                                    }
                                } else {
                                    FlowStep::Done(Node::empty())
                                }
                            }
                        },
                    }
                }
                FlowStep::Done(node) => {
                    let Some(top) = open.last_mut() else {
                        return Ok(node);
                    };
                    *i = skip_ws(text, *i);
                    match top {
                        Flow::Seq(items) => {
                            items.push(node);
                            match bytes.get(*i) {
                                Some(b',') => *i += 1,
                                Some(b']') => {}
                                Some(b':') => {
                                    return Err(unsupported(
                                        line,
                                        "mappings inside flow sequences are not supported",
                                    ));
                                }
                                None | Some(b'#') => return Err(multi_line_flow(line)),
                                _ => return Err(syntax(line, "did not find expected ',' or ']'")),
                            }
                        }
                        Flow::Map(entries, pending) => {
                            let key = pending.take().expect("a key awaits its value");
                            entries.push((key, node));
                            match bytes.get(*i) {
                                Some(b',') => *i += 1,
                                Some(b'}') => {}
                                None | Some(b'#') => return Err(multi_line_flow(line)),
                                _ => return Err(syntax(line, "did not find expected ',' or '}'")),
                            }
                        }
                    }
                    FlowStep::Entry
                }
            };
        }
    }

    /// A flow node at byte `*i` of `line` that is not a collection (a scalar, or an error
    /// for what cannot start a node), with `*i` advanced past it.
    fn flow_scalar(&self, line: usize, i: &mut usize) -> Result<Node> {
        let text = self.text(line);
        let bytes = text.as_bytes();
        *i = skip_ws(text, *i);
        let Some(&first) = bytes.get(*i) else {
            return Err(multi_line_flow(line));
        };
        if let Some(error) = refused_indicator(line, first) {
            return Err(error);
        }
        match first {
            b'"' | b'\'' => match self.quoted(line, *i, true)? {
                Some((raw, _, end)) => {
                    *i = end;
                    let style = if first == b'"' {
                        Style::Double
                    } else {
                        Style::Single
                    };
                    Ok(Node::Scalar(raw, style))
                }
                None => Err(multi_line_flow(line)),
            },
            b'?' if blank_after(text, *i) => Err(unsupported(
                line,
                "complex mapping keys (?) are not supported",
            )),
            b'-' if blank_after(text, *i) => Err(syntax(
                line,
                "block sequence entries are not allowed in this context",
            )),
            b',' | b']' | b'}' | b'#' => Err(syntax(line, "did not find expected node content")),
            b'|' | b'>' | b'%' | b'@' | b'`' => {
                Err(syntax(line, "found character that cannot start any token"))
            }
            _ => {
                let start = *i;
                let mut j = start;
                while j < bytes.len() {
                    match bytes[j] {
                        b',' | b'?' | b'[' | b']' | b'{' | b'}' => break,
                        b':' if blank_after(text, j) => break,
                        b'#' if j > start && is_ws(bytes[j - 1]) => break,
                        _ => j += 1,
                    }
                }
                *i = j;
                let raw = text[start..j].trim_end_matches([' ', '\t']);
                Ok(Node::Scalar(raw.to_owned(), Style::Plain))
            }
        }
    }
}

fn multi_line_flow(line: usize) -> AtlasError {
    unsupported(
        line,
        "flow collections that do not close on their line are not supported",
    )
}

// ---------------------------------------------------------------------------------------
// Conversion to the Go tool's values

/// A scalar as yaml.v3 resolves it for an untyped Go value.
#[derive(Debug, Clone, PartialEq)]
enum Scalar {
    Null,
    Bool(bool),
    Int(i128),
    Float(f64),
    /// A timestamp, already in the Go tool's form `YYYY-MM-DDTHH:MM:SS.mmmZ`.
    Timestamp(String),
    Str(String),
}

fn scalar_of(raw: &str, style: Style) -> Scalar {
    if style == Style::Plain {
        resolve(raw)
    } else {
        Scalar::Str(raw.to_owned())
    }
}

/// The Go tool's text for a mapping key (`yamlMappingKey`).
fn key_text(scalar: Scalar) -> String {
    match scalar {
        Scalar::Null => "null".to_owned(),
        Scalar::Bool(b) => b.to_string(),
        Scalar::Int(i) => i.to_string(),
        Scalar::Float(f) if f == f64::INFINITY => "Infinity".to_owned(),
        Scalar::Float(f) if f == f64::NEG_INFINITY => "-Infinity".to_owned(),
        Scalar::Float(f) if f.is_nan() => "NaN".to_owned(),
        // strconv.FormatFloat(f, 'f', -1, 64): the shortest digits, never an exponent.
        Scalar::Float(f) => format!("{f}"),
        Scalar::Timestamp(s) | Scalar::Str(s) => s,
    }
}

/// The value of a scalar node.
fn scalar_value(raw: &str, style: Style) -> Value {
    match scalar_of(raw, style) {
        Scalar::Null => Value::Null,
        Scalar::Bool(b) => Value::Bool(b),
        Scalar::Int(i) => Value::Integer(i),
        Scalar::Float(f) => Value::Number(f),
        Scalar::Timestamp(s) | Scalar::Str(s) => Value::String(s),
    }
}

/// A node collection being converted by [`to_value`]: its remaining children and the
/// values made so far.
enum Convert {
    Seq(std::vec::IntoIter<Node>, Vec<Value>),
    /// The remaining entries, the object so far and the key of the value being converted.
    Map(std::vec::IntoIter<(Key, Node)>, Object, Option<Key>),
}

/// Converts a parsed node into the Go tool's value, with an explicit stack rather than
/// recursion.
fn to_value(root: Node, input: &str) -> Result<Value> {
    let mut open: Vec<Convert> = Vec::new();
    let mut next = root;
    loop {
        let mut done = match &mut next {
            Node::Scalar(raw, style) => Some(scalar_value(raw, *style)),
            Node::Seq(items) => {
                let items = std::mem::take(items);
                let values = Vec::with_capacity(items.len());
                open.push(Convert::Seq(items.into_iter(), values));
                None
            }
            Node::Map(entries) => {
                open.push(Convert::Map(
                    std::mem::take(entries).into_iter(),
                    Object::new(),
                    None,
                ));
                None
            }
        };
        loop {
            let Some(top) = open.last_mut() else {
                return Ok(done.expect("the root value is complete"));
            };
            let child = match top {
                Convert::Seq(rest, values) => {
                    if let Some(value) = done.take() {
                        values.push(value);
                    }
                    rest.next()
                }
                Convert::Map(rest, object, pending) => {
                    // validateYAMLKeys: each value is checked before its key, then the key
                    // against the keys before it.
                    if let Some(value) = done.take() {
                        let key = pending.take().expect("a key awaits its value");
                        let name = key_text(scalar_of(&key.raw, key.style));
                        if object.contains_key(&name) {
                            return Err(AtlasError::Message(source_error(
                                "duplicated mapping key",
                                input,
                                key.line,
                                key.column,
                            )));
                        }
                        object.insert(name, value);
                    }
                    rest.next().map(|(key, node)| {
                        *pending = Some(key);
                        node
                    })
                }
            };
            if let Some(child) = child {
                next = child;
                break;
            }
            done = Some(match open.pop() {
                Some(Convert::Seq(_, values)) => Value::Array(values),
                Some(Convert::Map(_, object, _)) => Value::Object(object),
                None => unreachable!("the top collection was just read"),
            });
        }
    }
}

/// The Go tool's `yamlSourceError`: the reason, the position and a snippet of the source
/// with a caret under the column.
fn source_error(reason: &str, input: &str, line: usize, column: usize) -> String {
    let input = input.replace("\r\n", "\n").replace('\r', "\n");
    let lines: Vec<&str> = input
        .strip_suffix('\n')
        .unwrap_or(&input)
        .split('\n')
        .collect();
    let line = line as i64;
    let first = (line - 4).max(0);
    let last = (line + 1).min(lines.len() as i64 - 1);
    let max_number = (line + 1).min(lines.len() as i64);
    let digits = max_number.to_string().len() as i64;
    let mut snippet = Vec::new();
    for i in first..=last {
        let (text, pos) = snippet_line(lines[i as usize], column as i64 - 1, 79 - (1 + digits + 3));
        snippet.push(format!(
            " {:>width$} | {text}",
            i + 1,
            width = digits as usize
        ));
        if i == line - 1 {
            snippet.push(format!(
                "{}^",
                "-".repeat(usize::try_from(1 + digits + 3 + pos).unwrap_or(0))
            ));
        }
    }
    format!("{reason} ({line}:{column})\n\n{}", snippet.join("\n"))
}

/// The Go tool's `yamlSnippetLine`, which counts in UTF-16 code units.
fn snippet_line(text: &str, position: i64, max_length: i64) -> (String, i64) {
    let units: Vec<u16> = text.encode_utf16().collect();
    let length = units.len() as i64;
    let (mut start, mut end) = (0i64, length);
    let (mut head, mut tail) = ("", "");
    let half = max_length / 2 - 1;
    if position > half {
        head = " ... ";
        start = position - half + head.len() as i64;
    }
    if end - position > half {
        tail = " ...";
        end = position + half - tail.len() as i64;
    }
    start = start.min(length);
    end = end.min(length);
    if end < start {
        end = start;
    }
    let middle = String::from_utf16_lossy(&units[start as usize..end as usize]).replace('\t', "→");
    (
        format!("{head}{middle}{tail}"),
        position - start + head.len() as i64,
    )
}

// ---------------------------------------------------------------------------------------
// Scalar resolution (yaml.v3 resolve.go)

fn resolve(s: &str) -> Scalar {
    match s {
        "" | "~" | "null" | "Null" | "NULL" => return Scalar::Null,
        "true" | "True" | "TRUE" => return Scalar::Bool(true),
        "false" | "False" | "FALSE" => return Scalar::Bool(false),
        ".nan" | ".NaN" | ".NAN" => return Scalar::Float(f64::NAN),
        ".inf" | ".Inf" | ".INF" | "+.inf" | "+.Inf" | "+.INF" => {
            return Scalar::Float(f64::INFINITY);
        }
        "-.inf" | "-.Inf" | "-.INF" => return Scalar::Float(f64::NEG_INFINITY),
        _ => {}
    }
    match s.as_bytes()[0] {
        b'.' => dot_float(s).map_or_else(|| Scalar::Str(s.to_owned()), Scalar::Float),
        b'+' | b'-' | b'0'..=b'9' => {
            if let Some(timestamp) = parse_timestamp(s) {
                return Scalar::Timestamp(timestamp);
            }
            let plain = s.replace('_', "");
            if let Some(i) = go_parse_int(&plain) {
                return Scalar::Int(i128::from(i));
            }
            if let Some(u) = go_parse_uint(&plain) {
                return Scalar::Int(i128::from(u));
            }
            if yaml_style_float(&plain) {
                if let Ok(f) = plain.parse::<f64>() {
                    if f.is_finite() {
                        return Scalar::Float(f);
                    }
                }
            }
            Scalar::Str(s.to_owned())
        }
        _ => Scalar::Str(s.to_owned()),
    }
}

/// Go's `strconv.ParseFloat` for a string starting with `.`: digits, an optional exponent,
/// `_` between digits; an overflow is an error.
fn dot_float(s: &str) -> Option<f64> {
    if s.contains('_') && !underscore_ok(s) {
        return None;
    }
    let t = s.replace('_', "");
    let b = t.as_bytes();
    let mut i = 1;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    if i == 1 {
        return None;
    }
    if i < b.len() {
        if b[i] != b'e' && b[i] != b'E' {
            return None;
        }
        i += 1;
        if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
            i += 1;
        }
        let digits = i;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        if i == digits || i != b.len() {
            return None;
        }
    }
    let f: f64 = t.parse().ok()?;
    f.is_finite().then_some(f)
}

/// Go's `strconv.underscoreOK`: underscores only between digits (or after a base prefix).
fn underscore_ok(s: &str) -> bool {
    let mut b = s.as_bytes();
    if let [b'+' | b'-', rest @ ..] = b {
        b = rest;
    }
    let mut saw = b'^';
    let mut i = 0;
    let mut hex = false;
    if b.len() >= 2 && b[0] == b'0' && matches!(b[1].to_ascii_lowercase(), b'b' | b'o' | b'x') {
        i = 2;
        saw = b'0';
        hex = b[1].eq_ignore_ascii_case(&b'x');
    }
    while i < b.len() {
        let c = b[i];
        if c.is_ascii_digit() || (hex && c.is_ascii_hexdigit()) {
            saw = b'0';
        } else if c == b'_' {
            if saw != b'0' {
                return false;
            }
            saw = b'_';
        } else {
            if saw == b'_' {
                return false;
            }
            saw = b'!';
        }
        i += 1;
    }
    saw != b'_'
}

/// Go's `strconv.ParseUint(s, 0, 64)` for a string without underscores.
fn go_parse_uint(s: &str) -> Option<u64> {
    let b = s.as_bytes();
    if b.is_empty() {
        return None;
    }
    let (base, digits): (u64, &[u8]) = if b[0] == b'0' {
        match b.get(1).map(u8::to_ascii_lowercase) {
            Some(b'b') if b.len() >= 3 => (2, &b[2..]),
            Some(b'o') if b.len() >= 3 => (8, &b[2..]),
            Some(b'x') if b.len() >= 3 => (16, &b[2..]),
            _ => (8, &b[1..]),
        }
    } else {
        (10, b)
    };
    let mut n: u64 = 0;
    for &c in digits {
        let d = match c {
            b'0'..=b'9' => u64::from(c - b'0'),
            b'a'..=b'z' => u64::from(c - b'a') + 10,
            b'A'..=b'Z' => u64::from(c - b'A') + 10,
            _ => return None,
        };
        if d >= base {
            return None;
        }
        n = n.checked_mul(base)?.checked_add(d)?;
    }
    Some(n)
}

/// Go's `strconv.ParseInt(s, 0, 64)` for a string without underscores.
fn go_parse_int(s: &str) -> Option<i64> {
    let (negative, body) = match s.as_bytes().first() {
        Some(b'+') => (false, &s[1..]),
        Some(b'-') => (true, &s[1..]),
        _ => (false, s),
    };
    let u = go_parse_uint(body)?;
    if negative {
        if u > 1 << 63 {
            return None;
        }
        Some((-i128::from(u)) as i64)
    } else {
        i64::try_from(u).ok()
    }
}

/// `^[-+]?(\.[0-9]+|[0-9]+(\.[0-9]*)?)([eE][-+]?[0-9]+)?$`
fn yaml_style_float(s: &str) -> bool {
    let b = s.as_bytes();
    let mut i = 0;
    if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
        i += 1;
    }
    let digits = |i: &mut usize| {
        let start = *i;
        while *i < b.len() && b[*i].is_ascii_digit() {
            *i += 1;
        }
        *i - start
    };
    if b.get(i) == Some(&b'.') {
        i += 1;
        if digits(&mut i) == 0 {
            return false;
        }
    } else {
        if digits(&mut i) == 0 {
            return false;
        }
        if b.get(i) == Some(&b'.') {
            i += 1;
            digits(&mut i);
        }
    }
    if matches!(b.get(i), Some(b'e' | b'E')) {
        i += 1;
        if matches!(b.get(i), Some(b'+' | b'-')) {
            i += 1;
        }
        if digits(&mut i) == 0 {
            return false;
        }
    }
    i == b.len()
}

/// yaml.v3's `parseTimestamp`: Go's `time.Parse` with the layouts
/// `2006-1-2T15:4:5.999999999Z07:00`, the same with `t`, `2006-1-2 15:4:5.999999999` and
/// `2006-1-2`, rendered as the Go tool renders it (`Format("2006-01-02T15:04:05.000Z")`,
/// the wall clock of the parsed zone with a literal `Z`).
fn parse_timestamp(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let year_digits = b.iter().take_while(|c| c.is_ascii_digit()).count();
    if year_digits != 4 || b.get(4) != Some(&b'-') {
        return None;
    }
    [Some(b'T'), Some(b't'), Some(b' '), None]
        .into_iter()
        .find_map(|separator| parse_layout(b, separator))
}

fn parse_layout(mut v: &[u8], separator: Option<u8>) -> Option<String> {
    fn literal(v: &mut &[u8], c: u8) -> Option<()> {
        if v.first() == Some(&c) {
            *v = &v[1..];
            Some(())
        } else {
            None
        }
    }
    fn number(v: &mut &[u8]) -> Option<u32> {
        let first = *v.first().filter(|c| c.is_ascii_digit())?;
        match v.get(1).filter(|c| c.is_ascii_digit()) {
            Some(&second) => {
                *v = &v[2..];
                Some(u32::from(first - b'0') * 10 + u32::from(second - b'0'))
            }
            None => {
                *v = &v[1..];
                Some(u32::from(first - b'0'))
            }
        }
    }
    let year: u32 = std::str::from_utf8(&v[..4]).ok()?.parse().ok()?;
    v = &v[4..];
    literal(&mut v, b'-')?;
    let month = number(&mut v)?;
    if !(1..=12).contains(&month) {
        return None;
    }
    literal(&mut v, b'-')?;
    let day = number(&mut v)?;
    let (mut hour, mut minute, mut second, mut nanos) = (0, 0, 0, 0u64);
    if let Some(separator) = separator {
        if separator == b' ' {
            // A space in a Go layout matches one or more spaces.
            if v.first().is_some_and(|&c| c != b' ') {
                return None;
            }
            while v.first() == Some(&b' ') {
                v = &v[1..];
            }
        } else {
            literal(&mut v, separator)?;
        }
        hour = number(&mut v)?;
        if hour >= 24 {
            return None;
        }
        literal(&mut v, b':')?;
        minute = number(&mut v)?;
        if minute >= 60 {
            return None;
        }
        literal(&mut v, b':')?;
        second = number(&mut v)?;
        if second >= 60 {
            return None;
        }
        if v.len() >= 2 && (v[0] == b'.' || v[0] == b',') && v[1].is_ascii_digit() {
            let mut i = 0;
            while i + 1 < v.len() && v[i + 1].is_ascii_digit() {
                i += 1;
            }
            let digits = &v[1..(1 + i).min(10)];
            nanos = digits
                .iter()
                .fold(0u64, |n, d| n * 10 + u64::from(d - b'0'))
                * 10u64.pow(9 - digits.len() as u32);
            v = &v[1 + i..];
        }
        if separator != b' ' {
            if v.first() == Some(&b'Z') {
                v = &v[1..];
            } else {
                if v.len() < 6 || v[3] != b':' || !(v[0] == b'+' || v[0] == b'-') {
                    return None;
                }
                let two = |d: &[u8]| -> Option<u32> {
                    (d[0].is_ascii_digit() && d[1].is_ascii_digit())
                        .then(|| u32::from(d[0] - b'0') * 10 + u32::from(d[1] - b'0'))
                };
                let (zone_hour, zone_minute) = (two(&v[1..3])?, two(&v[4..6])?);
                if zone_hour > 24 || zone_minute > 60 {
                    return None;
                }
                v = &v[6..];
            }
        }
    }
    if !v.is_empty() {
        return None;
    }
    let leap = year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400));
    let days = match month {
        2 if leap => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    };
    if day < 1 || day > days {
        return None;
    }
    Some(format!(
        "{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{:03}Z",
        nanos / 1_000_000
    ))
}

// =======================================================================================
// Writing
// =======================================================================================

/// Writes `value` as the Go tool does: two-space indentation, ending with a newline.
pub fn to_yaml_string(value: &Value) -> Result<String> {
    let mut emitter = Emitter::new();
    emitter.node(value, false)?;
    emitter.end_document();
    Ok(emitter.out)
}

/// A scalar ready to emit: its text, the style asked for, and whether yaml.v3 tags it
/// `!!merge` (a plain `<<`).
struct Out {
    value: String,
    style: Style,
    merge: bool,
}

/// The emitter's analysis of a scalar (`yaml_emitter_analyze_scalar`).
struct Analysis {
    multiline: bool,
    block_plain_allowed: bool,
    single_quoted_allowed: bool,
    block_allowed: bool,
}

fn at(b: &[u8], i: usize) -> u8 {
    b.get(i).copied().unwrap_or(0)
}

fn utf8_width(c: u8) -> usize {
    match c {
        _ if c & 0x80 == 0 => 1,
        _ if c & 0xE0 == 0xC0 => 2,
        _ if c & 0xF0 == 0xE0 => 3,
        _ => 4,
    }
}

/// yaml.v3's `is_printable`, which (as in the Go tool) treats every four-byte character
/// as not printable.
fn is_printable(b: &[u8], i: usize) -> bool {
    let (c, c1, c2) = (at(b, i), at(b, i + 1), at(b, i + 2));
    c == 0x0A
        || (0x20..=0x7E).contains(&c)
        || (c == 0xC2 && c1 >= 0xA0)
        || (c > 0xC2 && c < 0xED)
        || (c == 0xED && c1 < 0xA0)
        || c == 0xEE
        || (c == 0xEF && !(c1 == 0xBB && c2 == 0xBF) && !(c1 == 0xBF && (c2 == 0xBE || c2 == 0xBF)))
}

fn is_break(b: &[u8], i: usize) -> bool {
    let (c, c1, c2) = (at(b, i), at(b, i + 1), at(b, i + 2));
    c == b'\r'
        || c == b'\n'
        || (c == 0xC2 && c1 == 0x85)
        || (c == 0xE2 && c1 == 0x80 && (c2 == 0xA8 || c2 == 0xA9))
}

fn is_blank(b: &[u8], i: usize) -> bool {
    at(b, i) == b' ' || at(b, i) == b'\t'
}

fn is_blankz(b: &[u8], i: usize) -> bool {
    is_blank(b, i) || is_break(b, i) || at(b, i) == 0
}

fn analyze(value: &str) -> Analysis {
    let b = value.as_bytes();
    if b.is_empty() {
        return Analysis {
            multiline: false,
            block_plain_allowed: true,
            single_quoted_allowed: true,
            block_allowed: false,
        };
    }
    let mut block_indicators = b.starts_with(b"---") || b.starts_with(b"...");
    let (mut line_breaks, mut special_characters, mut tab_characters) = (false, false, false);
    let (mut leading_space, mut leading_break) = (false, false);
    let (mut trailing_space, mut trailing_break) = (false, false);
    let (mut break_space, mut space_break) = (false, false);
    let (mut previous_space, mut previous_break) = (false, false);
    let mut preceded_by_whitespace = true;
    let mut i = 0;
    while i < b.len() {
        let w = utf8_width(b[i]);
        let followed_by_whitespace = i + w >= b.len() || is_blank(b, i + w);
        if i == 0 {
            match b[i] {
                b'#' | b',' | b'[' | b']' | b'{' | b'}' | b'&' | b'*' | b'!' | b'|' | b'>'
                | b'\'' | b'"' | b'%' | b'@' | b'`' => block_indicators = true,
                b'?' | b':' | b'-' if followed_by_whitespace => block_indicators = true,
                _ => {}
            }
        } else {
            match b[i] {
                b':' if followed_by_whitespace => block_indicators = true,
                b'#' if preceded_by_whitespace => block_indicators = true,
                _ => {}
            }
        }
        if b[i] == b'\t' {
            tab_characters = true;
        } else if !is_printable(b, i) {
            special_characters = true;
        }
        if b[i] == b' ' {
            if i == 0 {
                leading_space = true;
            }
            if i + w == b.len() {
                trailing_space = true;
            }
            if previous_break {
                break_space = true;
            }
            previous_space = true;
            previous_break = false;
        } else if is_break(b, i) {
            line_breaks = true;
            if i == 0 {
                leading_break = true;
            }
            if i + w == b.len() {
                trailing_break = true;
            }
            if previous_space {
                space_break = true;
            }
            previous_space = false;
            previous_break = true;
        } else {
            previous_space = false;
            previous_break = false;
        }
        preceded_by_whitespace = is_blankz(b, i);
        i += w;
    }
    let mut analysis = Analysis {
        multiline: line_breaks,
        block_plain_allowed: true,
        single_quoted_allowed: true,
        block_allowed: true,
    };
    if leading_space || leading_break || trailing_space || trailing_break {
        analysis.block_plain_allowed = false;
    }
    if trailing_space {
        analysis.block_allowed = false;
    }
    if break_space {
        analysis.block_plain_allowed = false;
        analysis.single_quoted_allowed = false;
    }
    if space_break || tab_characters || special_characters {
        analysis.block_plain_allowed = false;
        analysis.single_quoted_allowed = false;
    }
    if space_break || special_characters {
        analysis.block_allowed = false;
    }
    if line_breaks || block_indicators {
        analysis.block_plain_allowed = false;
    }
    analysis
}

/// `yaml_emitter_select_scalar_style` in block context, for an untagged or `!!merge`
/// scalar.
fn select_style(requested: Style, a: &Analysis, empty: bool, simple_key: bool) -> Style {
    let mut style = requested;
    if simple_key && a.multiline {
        style = Style::Double;
    }
    if style == Style::Plain && (!a.block_plain_allowed || (empty && simple_key)) {
        style = Style::Single;
    }
    if style == Style::Single && !a.single_quoted_allowed {
        style = Style::Double;
    }
    if matches!(style, Style::Literal | Style::Folded) && (!a.block_allowed || simple_key) {
        style = Style::Double;
    }
    style
}

/// yaml.v3's `isOldBool`.
fn is_old_bool(s: &str) -> bool {
    matches!(
        s,
        "y" | "Y"
            | "yes"
            | "Yes"
            | "YES"
            | "on"
            | "On"
            | "ON"
            | "n"
            | "N"
            | "no"
            | "No"
            | "NO"
            | "off"
            | "Off"
            | "OFF"
    )
}

/// yaml.v3's `isBase60Float`: `^[-+]?[0-9][0-9_]*(?::[0-5]?[0-9])+(?:\.[0-9_]*)?$`.
fn is_base60_float(s: &str) -> bool {
    let b = s.as_bytes();
    let mut i = 0;
    if matches!(b.first(), Some(b'+' | b'-')) {
        i += 1;
    }
    if !b.get(i).is_some_and(u8::is_ascii_digit) || !s.contains(':') {
        return false;
    }
    i += 1;
    while i < b.len() && (b[i].is_ascii_digit() || b[i] == b'_') {
        i += 1;
    }
    let mut groups = 0;
    while b.get(i) == Some(&b':') {
        i += 1;
        let start = i;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        match i - start {
            1 => {}
            2 if b[start] <= b'5' => {}
            _ => return false,
        }
        groups += 1;
    }
    if groups == 0 {
        return false;
    }
    if b.get(i) == Some(&b'.') {
        i += 1;
        while i < b.len() && (b[i].is_ascii_digit() || b[i] == b'_') {
            i += 1;
        }
    }
    i == b.len()
}

/// The Go tool's `(?i)^(y|yes|n|no|true|false|on|off)$`. Go's case folding also maps
/// U+017F (long s) to `s`.
fn is_bool_like(s: &str) -> bool {
    let folded: String = s
        .chars()
        .map(|c| {
            if c == '\u{17F}' {
                's'
            } else {
                c.to_ascii_lowercase()
            }
        })
        .collect();
    matches!(
        folded.as_str(),
        "y" | "yes" | "n" | "no" | "true" | "false" | "on" | "off"
    )
}

/// A Go `float64` as yaml.v3 writes it (`encode.go`, `floatv`).
fn format_number(f: f64) -> String {
    if f.is_nan() {
        return ".nan".to_owned();
    }
    if f.is_infinite() {
        return if f > 0.0 { ".inf" } else { "-.inf" }.to_owned();
    }
    // strconv.FormatFloat(f, 'g', -1, 64).
    let exponent_form = format!("{f:e}");
    let (mantissa, exponent) = exponent_form
        .split_once('e')
        .unwrap_or((exponent_form.as_str(), "0"));
    let exponent: i32 = exponent.parse().unwrap_or(0);
    if (-4..6).contains(&exponent) {
        return format!("{f}");
    }
    let sign = if exponent < 0 { '-' } else { '+' };
    format!("{mantissa}e{sign}{:02}", exponent.abs())
}

/// The scalar yaml.v3 emits for a Go string, after the Go tool's `yamlNode`: the string is
/// encoded alone and read back (`Node.Encode`), and the style found is kept or changed to
/// single quotes.
fn string_out(text: &str) -> Result<Out> {
    let resolves_to_string = matches!(resolve(text), Scalar::Str(_));
    let requested = if text.contains('\n') {
        Style::Literal
    } else if resolves_to_string && !is_base60_float(text) && !is_old_bool(text) {
        Style::Plain
    } else {
        Style::Double
    };
    let mut alone = Emitter::new();
    alone.scalar(
        &Out {
            value: text.to_owned(),
            style: requested,
            merge: false,
        },
        false,
        false,
    )?;
    alone.end_document();
    // When yaml.v3 cannot read its own output back (a multi-line string whose first line
    // starts with a tab), Node.Encode fails, the Go tool ignores the error and the empty
    // node it keeps is written as `null`.
    let read_back = Parser::new(&alone.out).and_then(|mut parser| parser.document());
    let mut read_back = read_back;
    let Ok(Some(Node::Scalar(value, style))) = &mut read_back else {
        return Ok(Out {
            value: "null".to_owned(),
            style: Style::Plain,
            merge: false,
        });
    };
    let (value, style) = (std::mem::take(value), *style);
    let merge = style == Style::Plain && value == "<<";
    let quote =
        !text.contains(['\0', '\t', '\r', '\n']) && (style == Style::Double || is_bool_like(text));
    Ok(Out {
        value,
        style: if quote { Style::Single } else { style },
        merge,
    })
}

fn scalar_out(value: &Value) -> Result<Out> {
    let plain = |text: String| Out {
        value: text,
        style: Style::Plain,
        merge: false,
    };
    Ok(match value {
        Value::Null => plain("null".to_owned()),
        Value::Bool(b) => plain(b.to_string()),
        Value::Number(f) => plain(format_number(*f)),
        // yaml.v3 writes a Go int or uint64 with strconv.FormatInt or FormatUint.
        Value::Integer(i) => plain(i.to_string()),
        Value::String(s) => string_out(s)?,
        Value::Array(_) | Value::Object(_) => unreachable!("collections are not scalars"),
    })
}

/// A mapping or sequence being written by [`Emitter::node`]: its remaining entries.
enum Emit<'a> {
    Map(Box<dyn Iterator<Item = (&'a str, &'a Value)> + 'a>),
    Seq(std::slice::Iter<'a, Value>),
}

/// A port of the parts of yaml.v3's emitter that `MarshalYAML` reaches: block collections,
/// empty flow collections and untagged scalars, with indentation 2 and no line width.
struct Emitter {
    out: String,
    column: usize,
    whitespace: bool,
    indention: bool,
    indent: isize,
    indents: Vec<isize>,
}

impl Emitter {
    fn new() -> Self {
        Emitter {
            out: String::new(),
            column: 0,
            whitespace: true,
            indention: true,
            indent: -1,
            indents: Vec::new(),
        }
    }

    fn put(&mut self, c: char) {
        self.out.push(c);
        self.column += 1;
    }

    fn put_break(&mut self) {
        self.out.push('\n');
        self.column = 0;
        self.indention = true;
    }

    fn write_break(&mut self, c: char) {
        if c == '\n' {
            self.put_break();
        } else {
            self.put(c);
            self.column = 0;
            self.indention = true;
        }
    }

    fn write_indicator(&mut self, indicator: &str, need_ws: bool, is_ws: bool, is_indention: bool) {
        if need_ws && !self.whitespace {
            self.put(' ');
        }
        for c in indicator.chars() {
            self.put(c);
        }
        self.whitespace = is_ws;
        self.indention = self.indention && is_indention;
    }

    fn write_indent(&mut self) {
        let indent = self.indent.max(0) as usize;
        if !self.indention || self.column > indent || (self.column == indent && !self.whitespace) {
            self.put_break();
        }
        while self.column < indent {
            self.put(' ');
        }
        self.whitespace = true;
    }

    /// `yaml_emitter_increase_indent` with indentation 2; `in_sequence_item` tells whether
    /// the state that will follow the node is a block sequence item.
    fn increase_indent(&mut self, flow: bool, in_sequence_item: bool) {
        self.indents.push(self.indent);
        if self.indent < 0 {
            self.indent = if flow { 2 } else { 0 };
        } else if in_sequence_item {
            self.indent += 2;
        } else {
            self.indent = 2 * ((self.indent + 2) / 2);
        }
    }

    fn pop_indent(&mut self) {
        self.indent = self.indents.pop().unwrap_or(-1);
    }

    fn end_document(&mut self) {
        self.write_indent();
    }

    /// Writes `value`. Nested mappings and sequences are walked with an explicit stack, so
    /// the depth of `value` does not use the thread's stack.
    fn node(&mut self, value: &Value, in_sequence_item: bool) -> Result<()> {
        let mut open: Vec<Emit<'_>> = Vec::new();
        let mut next = Some((value, in_sequence_item));
        loop {
            if let Some((value, in_sequence_item)) = next.take() {
                match value {
                    Value::Object(object) if object.is_empty() => {
                        self.write_indicator("{", true, true, false);
                        self.write_indicator("}", false, false, false);
                    }
                    Value::Array(items) if items.is_empty() => {
                        self.write_indicator("[", true, true, false);
                        self.write_indicator("]", false, false, false);
                    }
                    Value::Object(object) => {
                        self.increase_indent(false, in_sequence_item);
                        open.push(Emit::Map(Box::new(object.iter())));
                    }
                    Value::Array(items) => {
                        self.increase_indent(false, in_sequence_item);
                        open.push(Emit::Seq(items.iter()));
                    }
                    _ => self.scalar(&scalar_out(value)?, in_sequence_item, false)?,
                }
            }
            let Some(top) = open.last_mut() else {
                return Ok(());
            };
            match top {
                Emit::Map(entries) => match entries.next() {
                    Some((key, value)) => {
                        self.write_indent();
                        let key = string_out(key)?;
                        let analysis = analyze(&key.value);
                        let length = key.value.len() + if key.merge { 7 } else { 0 };
                        if !analysis.multiline && length <= 128 {
                            self.scalar(&key, false, true)?;
                            self.write_indicator(":", false, false, false);
                        } else {
                            self.write_indicator("?", true, false, true);
                            self.scalar(&key, false, false)?;
                            self.write_indent();
                            self.write_indicator(":", true, false, true);
                        }
                        next = Some((value, false));
                    }
                    None => {
                        self.pop_indent();
                        open.pop();
                    }
                },
                Emit::Seq(items) => match items.next() {
                    Some(item) => {
                        self.write_indent();
                        self.write_indicator("-", true, false, true);
                        next = Some((item, true));
                    }
                    None => {
                        self.pop_indent();
                        open.pop();
                    }
                },
            }
        }
    }

    fn scalar(&mut self, out: &Out, in_sequence_item: bool, simple_key: bool) -> Result<()> {
        let analysis = analyze(&out.value);
        let style = select_style(out.style, &analysis, out.value.is_empty(), simple_key);
        if out.merge {
            if !self.whitespace {
                self.put(' ');
            }
            for c in "!!merge".chars() {
                self.put(c);
            }
            self.whitespace = false;
            self.indention = false;
        }
        self.increase_indent(true, in_sequence_item);
        match style {
            Style::Plain => self.write_plain(&out.value),
            Style::Single => self.write_single(&out.value),
            Style::Double => self.write_double(&out.value),
            Style::Literal => self.write_literal(&out.value),
            Style::Folded => {
                return Err(AtlasError::Message(
                    "yaml: folded output is not produced by the Go tool".to_owned(),
                ));
            }
        }
        self.pop_indent();
        Ok(())
    }

    fn write_plain(&mut self, value: &str) {
        let b = value.as_bytes();
        if !value.is_empty() && !self.whitespace {
            self.put(' ');
        }
        let mut breaks = false;
        for (i, c) in value.char_indices() {
            if c == ' ' {
                self.put(c);
            } else if is_break(b, i) {
                if !breaks && c == '\n' {
                    self.put_break();
                }
                self.write_break(c);
                breaks = true;
            } else {
                if breaks {
                    self.write_indent();
                }
                self.put(c);
                self.indention = false;
                breaks = false;
            }
        }
        if !value.is_empty() {
            self.whitespace = false;
        }
        self.indention = false;
    }

    fn write_single(&mut self, value: &str) {
        let b = value.as_bytes();
        self.write_indicator("'", true, false, false);
        let mut breaks = false;
        for (i, c) in value.char_indices() {
            if c == ' ' {
                self.put(c);
            } else if is_break(b, i) {
                if !breaks && c == '\n' {
                    self.put_break();
                }
                self.write_break(c);
                breaks = true;
            } else {
                if breaks {
                    self.write_indent();
                }
                if c == '\'' {
                    self.put('\'');
                }
                self.put(c);
                self.indention = false;
                breaks = false;
            }
        }
        self.write_indicator("'", false, false, false);
        self.whitespace = false;
        self.indention = false;
    }

    fn write_double(&mut self, value: &str) {
        let b = value.as_bytes();
        // yaml.v3's is_bom looks at the start of the value, not at the current character.
        let bom = b.starts_with(&[0xEF, 0xBB, 0xBF]);
        self.write_indicator("\"", true, false, false);
        for (i, c) in value.char_indices() {
            if !is_printable(b, i) || bom || is_break(b, i) || c == '"' || c == '\\' {
                self.put('\\');
                let code = u32::from(c);
                let short = match code {
                    0x00 => Some('0'),
                    0x07 => Some('a'),
                    0x08 => Some('b'),
                    0x09 => Some('t'),
                    0x0A => Some('n'),
                    0x0B => Some('v'),
                    0x0C => Some('f'),
                    0x0D => Some('r'),
                    0x1B => Some('e'),
                    0x22 => Some('"'),
                    0x5C => Some('\\'),
                    0x85 => Some('N'),
                    0xA0 => Some('_'),
                    0x2028 => Some('L'),
                    0x2029 => Some('P'),
                    _ => None,
                };
                match short {
                    Some(s) => self.put(s),
                    None => {
                        let (prefix, digits) = match code {
                            0..=0xFF => ('x', 2),
                            0x100..=0xFFFF => ('u', 4),
                            _ => ('U', 8),
                        };
                        self.put(prefix);
                        for c in format!("{code:0digits$X}").chars() {
                            self.put(c);
                        }
                    }
                }
            } else {
                self.put(c);
            }
        }
        self.write_indicator("\"", false, false, false);
        self.whitespace = false;
        self.indention = false;
    }

    fn write_literal(&mut self, value: &str) {
        let b = value.as_bytes();
        self.write_indicator("|", true, false, false);
        if at(b, 0) == b' ' || is_break(b, 0) {
            self.write_indicator("2", false, false, false);
        }
        let chomp = if b.is_empty() {
            Some("-")
        } else {
            let mut i = b.len() - 1;
            while b[i] & 0xC0 == 0x80 {
                i -= 1;
            }
            if !is_break(b, i) {
                Some("-")
            } else if i == 0 {
                Some("+")
            } else {
                i -= 1;
                while b[i] & 0xC0 == 0x80 {
                    i -= 1;
                }
                is_break(b, i).then_some("+")
            }
        };
        if let Some(chomp) = chomp {
            self.write_indicator(chomp, false, false, false);
        }
        self.whitespace = true;
        let mut breaks = true;
        for (i, c) in value.char_indices() {
            if is_break(b, i) {
                self.write_break(c);
                breaks = true;
            } else {
                if breaks {
                    self.write_indent();
                }
                self.put(c);
                self.indention = false;
                breaks = false;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::INITIAL_CONFIG_YAML;
    use crate::json;

    // Expected values below were produced by the Go tool's own functions (YAMLValue,
    // MarshalYAML, ReadConfig) at the reference commit, built offline from the reference
    // clone.

    fn read(text: &str) -> Value {
        parse(text).unwrap_or_else(|e| panic!("{text:?}: {e}"))
    }

    fn read_json(text: &str) -> String {
        json::to_compact_string(&read(text)).unwrap()
    }

    fn write_json(text: &str) -> String {
        to_yaml_string(&json::parse(text).unwrap()).unwrap()
    }

    fn unsupported_line(text: &str) -> usize {
        match parse(text) {
            Err(AtlasError::UnsupportedYaml { line, .. }) => line,
            other => panic!("{text:?}: expected UnsupportedYaml, got {other:?}"),
        }
    }

    #[test]
    fn reads_initial_template() {
        assert_eq!(
            read_json(INITIAL_CONFIG_YAML),
            r#"{"version":"1","domains":{},"governance":{"max_entries":100,"warn_entries":150,"hard_limit":200},"classification_defaults":{"shelf_life":{"tactical":14,"observational":30}}}"#
        );
        // A rewrite drops the template's comments.
        assert_eq!(
            to_yaml_string(&read(INITIAL_CONFIG_YAML)).unwrap(),
            "version: '1'\ndomains: {}\ngovernance:\n  max_entries: 100\n  warn_entries: 150\n  hard_limit: 200\nclassification_defaults:\n  shelf_life:\n    tactical: 14\n    observational: 30\n"
        );
    }

    #[test]
    fn refuses_anchor_and_alias_with_line() {
        assert_eq!(unsupported_line("a: 1\nb: &base\n  c: 2\n"), 2);
        assert_eq!(unsupported_line("a: 1\nb: 2\nc: *base\n"), 3);
        assert_eq!(unsupported_line("a:\n  - x\n  - *base\n"), 3);
        assert_eq!(unsupported_line("a: {b: &x 1}\n"), 1);
        // Merge keys, tags and several documents are refused the same way.
        assert_eq!(unsupported_line("a: 1\n<<: {b: 1}\n"), 2);
        assert_eq!(unsupported_line("a: !!str 1\n"), 1);
        assert_eq!(unsupported_line("a: 1\n---\nb: 2\n"), 2);
        assert_eq!(unsupported_line("a: 1\n...\nb: 2\n"), 3);
        assert_eq!(unsupported_line("%YAML 1.2\n---\na: 1\n"), 1);
        assert_eq!(unsupported_line("? a\n: b\n"), 1);
        let message = parse("x: 1\ny: *a\n").unwrap_err().to_string();
        assert_eq!(
            message,
            "unsupported YAML at line 2: aliases (*) are not supported"
        );
    }

    #[test]
    fn quotes_boolean_like_strings() {
        assert_eq!(
            write_json(
                r#"{"a":"yes","b":"No","c":"ON","d":"off","e":"y","f":"N","g":"true","h":"False","i":"tRuE","j":"oN","k":"yes please","yes":1,"true":2}"#
            ),
            "a: 'yes'\nb: 'No'\nc: 'ON'\nd: 'off'\ne: 'y'\nf: 'N'\ng: 'true'\nh: 'False'\ni: 'tRuE'\nj: 'oN'\nk: yes please\n'yes': 1\n'true': 2\n"
        );
        // Strings the encoder would double-quote are single-quoted; others keep their style.
        assert_eq!(
            write_json(
                r#"{"a":"","b":"null","c":"1","d":"2025-01-01","e":"a: b","f":"{s}","g":"it's","h":"a\tb","i":"🚀","j":"x\u2028y","k":"<<","l":"plain text"}"#
            ),
            "a: ''\nb: 'null'\nc: '1'\nd: '2025-01-01'\ne: 'a: b'\nf: '{s}'\ng: it's\nh: \"a\\tb\"\ni: \"\\U0001F680\"\nj: 'x\u{2028}  y'\nk: !!merge <<\nl: plain text\n"
        );
    }

    #[test]
    fn round_trips_block_scalars() {
        let yaml = "lit: |\n  a\n  b\nstrip: |-\n  a\n  b\nkeep: |+\n  a\n\nlead: |2\n    x\n  y\n";
        let value = read(yaml);
        assert_eq!(
            json::to_compact_string(&value).unwrap(),
            r#"{"lit":"a\nb\n","strip":"a\nb","keep":"a\n\n","lead":"  x\ny\n"}"#
        );
        assert_eq!(to_yaml_string(&value).unwrap(), yaml);
        // Folded scalars read as the Go tool reads them, and are written back as literals.
        let folded = read("f: >\n  a\n  b\n\n  c\n    d\n  e\ng: >-\n  x\n");
        assert_eq!(
            json::to_compact_string(&folded).unwrap(),
            r#"{"f":"a b\nc\n  d\ne\n","g":"x"}"#
        );
        assert_eq!(
            to_yaml_string(&folded).unwrap(),
            "f: |\n  a b\n  c\n    d\n  e\ng: x\n"
        );
        // yaml.v3 loses a lone line break.
        assert_eq!(write_json(r#"{"e":"\n"}"#), "e: \"\"\n");
    }

    #[test]
    fn keeps_integers_exact() {
        // yaml.v3 decodes an integer to a Go int or, above that, a uint64, and writes it
        // with FormatInt or FormatUint, so nothing is rounded to 2^53. An integer beyond
        // uint64, and every float, is a float64, written with FormatFloat(f, 'g', -1, 64).
        let yaml = "a: 9007199254740993\nb: 9223372036854775807\nc: -9223372036854775808\nd: 18446744073709551615\ne: 0x7FFFFFFFFFFFFFFF\nf: 1_000\ng: 18446744073709551616\nh: -9223372036854775809\ni: 1000000.0\nj: 1e6\nk: 2.0\nl: 123456.0\nm: 0.00001\ns: [9007199254740993, -0]\n";
        assert_eq!(
            read_json(yaml),
            r#"{"a":9007199254740993,"b":9223372036854775807,"c":-9223372036854775808,"d":18446744073709551615,"e":9223372036854775807,"f":1000,"g":18446744073709552000,"h":-9223372036854776000,"i":1000000,"j":1000000,"k":2,"l":123456,"m":0.00001,"s":[9007199254740993,0]}"#
        );
        assert_eq!(
            read("a: 9007199254740993\n").as_object().unwrap().get("a"),
            Some(&Value::Integer(9_007_199_254_740_993))
        );
        assert_eq!(
            to_yaml_string(&read(yaml)).unwrap(),
            "a: 9007199254740993\nb: 9223372036854775807\nc: -9223372036854775808\nd: 18446744073709551615\ne: 9223372036854775807\nf: 1000\ng: 1.8446744073709552e+19\nh: -9.223372036854776e+18\ni: 1e+06\nj: 1e+06\nk: 2\nl: 123456\nm: 1e-05\ns:\n  - 9007199254740993\n  - 0\n"
        );
    }

    #[test]
    fn refuses_flow_collection_as_later_block_key() {
        // yaml.v3 reads a flow collection before `:` as a mapping key; like the first key
        // of a mapping, a later one is refused as unsupported, not as a syntax error.
        assert_eq!(unsupported_line("[a]: d\n"), 1);
        assert_eq!(unsupported_line("domains: {}\n[a]: d\n"), 2);
        assert_eq!(unsupported_line("domains: {}\n{a: 1}: d\n"), 2);
        assert_eq!(unsupported_line("m:\n  a: 1\n\n  [b, c] : 2\n"), 4);
        assert_eq!(unsupported_line("- a: 1\n  {b: 2}: 3\n"), 2);
        let message = parse("x: 1\n[a]: d\n").unwrap_err().to_string();
        assert_eq!(
            message,
            "unsupported YAML at line 2: flow collections as mapping keys are not supported"
        );
        // Without a `:` after it, a flow collection where a key belongs is invalid YAML,
        // which yaml.v3 also rejects: it stays a syntax error, not an unsupported construct.
        for text in ["a: 1\n[b]\n", "a: 1\n[b] x: 2\n", "a: 1\n[b\n"] {
            match parse(text) {
                Err(AtlasError::Message(message)) => {
                    assert!(message.starts_with("yaml: line "), "{text:?}: {message}");
                }
                other => panic!("{text:?}: expected a syntax error, got {other:?}"),
            }
        }
    }

    #[test]
    fn resolves_scalars_as_yaml_v3() {
        assert_eq!(
            read_json(
                "a: 2025-01-01\nb: 2001-12-14t21:59:43.10-05:00\nc: 2001-12-14 21:59:43.1234\nd: 0777\ne: 0x1F\nf: 1_000\ng: 09\nh: 1e400\ni: .5\nj: +.5\nl: 1e6\nm: yes\nn: ~\no: 2025-04-31\np: 12:30\nq: -0\nr: 0o17\ns: 0b101\nt: ._5\nu: .5_0\nv: 1_0.5\nw: 2025-1-2T3:4:5Z\nx: 2025-01-01T25:00:00Z\n"
            ),
            r#"{"a":"2025-01-01T00:00:00.000Z","b":"2001-12-14T21:59:43.100Z","c":"2001-12-14T21:59:43.123Z","d":511,"e":31,"f":1000,"g":9,"h":"1e400","i":0.5,"j":0.5,"l":1000000,"m":"yes","n":null,"o":"2025-04-31","p":"12:30","q":0,"r":15,"s":5,"t":"._5","u":0.5,"v":10.5,"w":"2025-01-02T03:04:05.000Z","x":"2025-01-01T25:00:00Z"}"#
        );
        assert_eq!(
            read_json("1: a\n\"x\": b\ntrue: c\n~: d\n2025-01-01: e\n1.50: f\n.inf: g\n.nan: h\n0x10: i\n1e21: j\n"),
            r#"{"1":"a","16":"i","x":"b","true":"c","null":"d","2025-01-01T00:00:00.000Z":"e","1.5":"f","Infinity":"g","NaN":"h","1000000000000000000000":"j"}"#
        );
    }

    #[test]
    fn reads_block_flow_and_quoted_forms() {
        assert_eq!(
            read_json(
                "a:\n  - x\n  -\n  - - y\n  - k: v\n    k2: |\n      lit\n\n      more\nb: plain\n  continued\n\n  para\nc: \"dq \\\n  join \\u00e9 \\x41\"\nd: 'sq\n  fold'\ne: [a, 'b', {c: d}, []] # comment\nf: {x: 1, y}\nlist:\n- 1\n- 2\n"
            ),
            r#"{"a":["x",null,["y"],{"k":"v","k2":"lit\n\nmore\n"}],"b":"plain continued\npara","c":"dq join é A","d":"sq fold","e":["a","b",{"c":"d"},[]],"f":{"x":1,"y":null},"list":[1,2]}"#
        );
        assert_eq!(read_json(""), "null");
        assert_eq!(read_json("# only a comment\n"), "null");
        assert_eq!(read_json("---\na: 1\n...\n"), r#"{"a":1}"#);
    }

    #[test]
    fn duplicate_key_is_the_go_error() {
        let error =
            parse("version: '1'\ndomains: {}\ngovernance:\n  max_entries: 42\n  max_entries: 55\n")
                .unwrap_err();
        assert_eq!(
            error.to_string(),
            "duplicated mapping key (5:3)\n\n 2 | domains: {}\n 3 | governance:\n 4 |   max_entries: 42\n 5 |   max_entries: 55\n-------^"
        );
        // Key text, not key spelling, decides: 1 and '1' are the same key.
        assert!(parse("1: a\n'1': b\n").is_err());
    }

    #[test]
    fn refuses_what_it_cannot_read() {
        for text in [
            "a: b: c\n",
            "a: \"x\n",
            "a:\n\tb: 1\n",
            "a: 1\n b: 2\n",
            "a: 1\n- b\n",
            "a: \"\\q\"\n",
            "- a\nb: 1\n",
        ] {
            assert!(
                matches!(parse(text), Err(AtlasError::Message(_))),
                "{text:?}"
            );
        }
        assert_eq!(unsupported_line("a: [1,\n  2]\n"), 1);
    }

    #[test]
    fn writes_nested_collections_as_go() {
        assert_eq!(
            write_json(
                r#"{"n":1,"f":1.5,"big":1e21,"neg":-0.5,"t":true,"nul":null,"arr":[],"obj":{},"nested":[{"a":1,"b":[1,2]},[3,[4]],{}],"k":{"x":{"y":[]}}}"#
            ),
            "'n': 1\nf: 1.5\nbig: 1e+21\nneg: -0.5\nt: true\nnul: null\narr: []\nobj: {}\nnested:\n  - a: 1\n    b:\n      - 1\n      - 2\n  - - 3\n    - - 4\n  - {}\nk:\n  x:\n    'y': []\n"
        );
        assert_eq!(
            write_json(r#"{"":"empty key","1":"idx","a b":2,"long\nkey":3}"#),
            "'1': idx\n'': empty key\na b: 2\n? |-\n  long\n  key\n: 3\n"
        );
        // A multi-line string whose first line starts with a tab cannot be read back by
        // yaml.v3, and the Go tool writes it as null.
        assert_eq!(
            write_json(r#"{"a":"\tx\ny","b":"x\n\ty"}"#),
            "a: null\nb: |-\n  x\n  \ty\n"
        );
        assert_eq!(write_json(r#""x""#), "x\n");
        assert_eq!(write_json("[]"), "[]\n");
    }

    // The depth tests run on the default test thread (2 MiB unless RUST_MIN_STACK says
    // otherwise). Their messages are the Go tool's, checked against the reference build.

    /// The message of the error `parse` gives for `text`.
    fn error_message(text: &str) -> String {
        match parse(text) {
            Err(AtlasError::Message(message)) => message,
            Err(other) => panic!("expected a YAML error, got {other}"),
            Ok(_) => panic!("expected a YAML error, got a value"),
        }
    }

    const TOO_DEEP: &str = "exceeded max depth of 10000";

    #[test]
    fn deep_flow_nesting_is_limited_as_in_go() {
        let seqs = |n: usize| format!("{}{}", "[".repeat(n), "]".repeat(n));
        let value = read(&format!("version: '1'\ndomains: {}\n", seqs(MAX_DEPTH)));
        let domains = value.as_object().unwrap().get("domains").unwrap();
        assert_eq!(json::to_compact_string(domains).unwrap(), seqs(MAX_DEPTH));
        assert_eq!(
            error_message(&format!("version: '1'\ndomains: {}\n", seqs(MAX_DEPTH + 1))),
            format!("yaml: line 2: {TOO_DEEP}")
        );
        // On the first line yaml.v3 reports no line.
        read(&seqs(MAX_DEPTH));
        assert_eq!(
            error_message(&seqs(MAX_DEPTH + 1)),
            format!("yaml: {TOO_DEEP}")
        );
        // Flow mappings count the same way.
        let maps = |n: usize| format!("{}b{}", "{a: ".repeat(n), "}".repeat(n));
        let value = read(&format!("version: '1'\nx: {}\n", maps(MAX_DEPTH)));
        let mut node = value.as_object().unwrap().get("x").unwrap();
        for _ in 0..MAX_DEPTH {
            node = node.as_object().unwrap().get("a").unwrap();
        }
        assert_eq!(node.as_str(), Some("b"));
        assert_eq!(
            error_message(&format!("version: '1'\nx: {}\n", maps(MAX_DEPTH + 1))),
            format!("yaml: line 2: {TOO_DEEP}")
        );
        // Far deeper input is the same error, not a stack overflow.
        assert_eq!(
            error_message(&format!("version: '1'\ndomains: {}\n", seqs(100_000))),
            format!("yaml: line 2: {TOO_DEEP}")
        );
    }

    #[test]
    fn deep_block_nesting_is_limited_as_in_go() {
        let entries = |n: usize| format!("{}x\n", "- ".repeat(n));
        let value = read(&entries(MAX_DEPTH));
        let mut node = &value;
        for _ in 0..MAX_DEPTH {
            node = &node.as_array().unwrap()[0];
        }
        assert_eq!(node.as_str(), Some("x"));
        // A sequence's error reports the line of the last simple key before it, or the
        // line of its `-` when that is line 1, or no line.
        assert_eq!(
            error_message(&entries(MAX_DEPTH + 1)),
            format!("yaml: {TOO_DEEP}")
        );
        let under_key = |n: usize| format!("version: '1'\nfoo: bar\ndomains:\n  {}", entries(n));
        read(&under_key(MAX_DEPTH - 1));
        assert_eq!(
            error_message(&under_key(MAX_DEPTH)),
            format!("yaml: line 3: {TOO_DEEP}")
        );
        // An indentless sequence opens no indentation level.
        let indentless = |n: usize| format!("a:\n{}", entries(n));
        read(&indentless(MAX_DEPTH));
        assert_eq!(
            error_message(&indentless(MAX_DEPTH + 1)),
            format!("yaml: line 2: {TOO_DEEP}")
        );
        // A mapping's error reports the line of its first key.
        let maps = |seqs: usize| {
            let col = 2 * seqs;
            format!(
                "{}k0:\n{}k1:\n{}k2: v\n",
                "- ".repeat(seqs),
                " ".repeat(col + 2),
                " ".repeat(col + 4)
            )
        };
        read(&maps(MAX_DEPTH - 3));
        assert_eq!(
            error_message(&maps(MAX_DEPTH - 2)),
            format!("yaml: line 3: {TOO_DEEP}")
        );
        // Mappings nested one line at a time, as far as yaml.v3 allows and one more.
        let nested = |n: usize| {
            let mut text = String::from("version: '1'\ndomains:\n");
            for i in 1..n {
                text.push_str(&" ".repeat(i));
                text.push_str("k:\n");
            }
            text.push_str(&" ".repeat(n));
            text.push_str("v: 1\n");
            text
        };
        read(&nested(MAX_DEPTH - 1));
        assert_eq!(
            error_message(&nested(MAX_DEPTH)),
            format!("yaml: line {}: {TOO_DEEP}", MAX_DEPTH + 2)
        );
        // Far deeper input is the same error, not a stack overflow.
        assert_eq!(
            error_message(&entries(100_000)),
            format!("yaml: {TOO_DEEP}")
        );
    }

    #[test]
    fn deep_values_convert_drop_and_write_on_a_default_stack() {
        // A deep subtree is complete when a later error discards it.
        let text = format!(
            "a: {}{}\na: 1\n",
            "[".repeat(MAX_DEPTH),
            "]".repeat(MAX_DEPTH)
        );
        assert!(error_message(&text).starts_with("duplicated mapping key (2:1)"));
        // A deep value is written as yaml.v3 writes it: the sequence that was indentless
        // is indented, so it opens one more level when read back (in the Go tool too).
        let value = read(&format!("a:\n{}x\n", "- ".repeat(MAX_DEPTH)));
        let written = to_yaml_string(&value).unwrap();
        assert_eq!(written, format!("a:\n  {}x\n", "- ".repeat(MAX_DEPTH)));
        assert_eq!(error_message(&written), format!("yaml: line 2: {TOO_DEEP}"));
        let value = read(&format!("a:\n{}x\n", "- ".repeat(MAX_DEPTH - 1)));
        assert_eq!(
            json::to_compact_string(&read(&to_yaml_string(&value).unwrap())).unwrap(),
            json::to_compact_string(&value).unwrap()
        );
    }
}
