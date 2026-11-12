//! `config.yaml`: the bytes `init` writes and the lenient read of
//! `loadConfig` (`cli.go:155-160`, `1744-1763`), with YAML parsed by
//! yaml-rust2 (assumption AS8).

use crate::error::BeaconError;
use crate::paths;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;
use yaml_rust2::parser::{MarkedEventReceiver, Parser};
use yaml_rust2::scanner::{Marker, TScalarStyle};
use yaml_rust2::{Event, Yaml, YamlLoader};

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
    // YamlLoader expands every alias by copying the anchored node, with no
    // limit, so a few hundred bytes of nested aliases would exhaust memory.
    // Check the input first, without expanding anything, the way yaml.v3
    // decodes it.
    check_aliasing(text)?;
    let documents =
        YamlLoader::load_from_str(text).map_err(|error| BeaconError::Yaml(error.to_string()))?;
    // yaml.v3's Unmarshal decodes the first document only; no document, an
    // empty one or an explicit null leaves the map nil.
    let mapping = match documents.into_iter().next() {
        None | Some(Yaml::Null | Yaml::BadValue) => None,
        Some(Yaml::Hash(mapping)) => Some(mapping),
        Some(other) => {
            return Err(BeaconError::Yaml(format!(
                "cannot unmarshal {} into a mapping",
                kind(&other)
            )))
        }
    };
    let text_of = |key: &str| {
        mapping
            .as_ref()
            .and_then(|mapping| mapping.get(&Yaml::String(key.to_owned())))
            .and_then(|value| match value {
                Yaml::String(text) if !text.is_empty() => Some(text.clone()),
                _ => None,
            })
    };
    Ok(Config {
        project: text_of("project").unwrap_or_else(|| DEFAULT_PROJECT.to_owned()),
        version: text_of("version").unwrap_or_else(|| DEFAULT_VERSION.to_owned()),
    })
}

/// Refuses input that yaml.v3 would refuse for its aliases, before
/// YamlLoader can expand any of them.
///
/// The text is read as events (nothing is expanded) into one small node tree
/// per document. At the end of each document the tree is walked in the order
/// yaml.v3's decoder visits it (`decoder.unmarshal`, decode.go:484-519), with
/// its counters and its rule (decode.go:455-491), and an alias that re-enters
/// itself is refused as `decoder.alias` does (decode.go:533-543). A syntax
/// error ends the check, and no document is loaded, so YamlLoader never sees
/// a document that was not checked.
///
/// Where yaml.v3 would skip a value (a key it cannot decode, a mapping with a
/// repeated key, a merged key already present) the walk still counts it,
/// because YamlLoader builds it. yaml.v3 decodes only the first document;
/// YamlLoader builds every document, so each is checked the same way.
fn check_aliasing(text: &str) -> Result<(), BeaconError> {
    let mut guard = AliasGuard::new(text);
    let scanned = Parser::new_from_str(text).load(&mut guard, true);
    if let Some(error) = guard.failure {
        return Err(error);
    }
    scanned.map_err(|error| BeaconError::Yaml(error.to_string()))
}

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

/// One node of a document as yaml.v3's parser builds it, without values.
enum Node {
    /// A scalar; `merge` when yaml.v3's `isMerge` holds for it
    /// (decode.go:998-1000), using the tag that `parser.scalar` and `parser.node`
    /// give it (decode.go:224-252, 170-194).
    Scalar {
        merge: bool,
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

/// Collects each document's node tree from parser events and checks it when
/// the document ends. After the first failure it ignores further events.
struct AliasGuard<'a> {
    text: &'a str,
    nodes: Vec<Node>,
    open: Vec<usize>,
    anchors: HashMap<usize, usize>,
    root: Option<usize>,
    failure: Option<BeaconError>,
}

impl<'a> AliasGuard<'a> {
    fn new(text: &'a str) -> Self {
        AliasGuard {
            text,
            nodes: Vec::new(),
            open: Vec::new(),
            anchors: HashMap::new(),
            root: None,
            failure: None,
        }
    }

    fn add(&mut self, node: Node, anchor: usize) -> usize {
        let index = self.nodes.len();
        self.nodes.push(node);
        // yaml.v3 and yaml-rust2 both register an anchor when its node
        // starts, so an alias inside the node can refer to it.
        if anchor > 0 {
            self.anchors.insert(anchor, index);
        }
        match self.open.last() {
            Some(&parent) => match &mut self.nodes[parent] {
                Node::Sequence(children) | Node::Mapping(children) => children.push(index),
                Node::Scalar { .. } | Node::Alias { .. } => {}
            },
            None => self.root = Some(index),
        }
        index
    }

    /// The anchor name an alias at `mark` uses, as yaml-rust2's scanner reads
    /// it (`scan_anchor`), for yaml.v3's message.
    fn alias_name(&self, mark: usize) -> String {
        self.text
            .chars()
            .skip(mark + 1)
            .take_while(|c| {
                !matches!(
                    c,
                    ' ' | '\t' | '\n' | '\r' | '\u{feff}' | '\0' | ',' | '[' | ']' | '{' | '}'
                )
            })
            .collect()
    }

    /// Walks the document as yaml.v3's decoder does and applies its rule.
    /// The walk uses its own stack, so deep nesting cannot overflow ours, and
    /// it stops as soon as the rule fails, so it never runs longer than
    /// yaml.v3's own decode would.
    fn check_document(&self, root: usize) -> Result<(), BeaconError> {
        enum Step {
            /// `decoder.unmarshal` of a node; `merging` when the node is
            /// being merged into a mapping (`d.mergedFields` is set).
            Visit { node: usize, merging: bool },
            /// The end of `decoder.alias` for this alias node.
            LeaveAlias(usize),
        }
        // The document node itself is the first `unmarshal`
        // (decode.go:524-531).
        let mut decode_count: u64 = 1;
        let mut alias_count: u64 = 0;
        let mut alias_depth: u64 = 0;
        let mut active_aliases = HashSet::new();
        let mut steps = vec![Step::Visit {
            node: root,
            merging: false,
        }];
        while let Some(step) = steps.pop() {
            let (node, merging) = match step {
                Step::Visit { node, merging } => (node, merging),
                Step::LeaveAlias(alias) => {
                    alias_depth -= 1;
                    active_aliases.remove(&alias);
                    continue;
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
            let mut next = Vec::new();
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
                            self.alias_name(*mark)
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
                    // decode.go:767-862: every key then its value, in order,
                    // except merge keys, whose values are merged afterwards.
                    let mut merges = Vec::new();
                    for pair in children.chunks(2) {
                        let key = pair[0];
                        if matches!(self.nodes[key], Node::Scalar { merge: true }) {
                            merges.extend(pair.get(1).copied());
                            continue;
                        }
                        next.extend(pair.iter().map(|&node| Step::Visit {
                            node,
                            merging: false,
                        }));
                    }
                    if !merges.is_empty() {
                        // decode.go:959-996: outside a merge, every key of
                        // the mapping is decoded once more first.
                        if !merging {
                            next.extend(children.iter().step_by(2).map(|&node| Step::Visit {
                                node,
                                merging: false,
                            }));
                        }
                        // yaml.v3 merges only the last merge value; the
                        // others are counted too, because YamlLoader builds
                        // them.
                        for value in merges {
                            match &self.nodes[value] {
                                Node::Sequence(elements) => {
                                    next.extend(elements.iter().map(|&node| Step::Visit {
                                        node,
                                        merging: true,
                                    }))
                                }
                                _ => next.push(Step::Visit {
                                    node: value,
                                    merging: true,
                                }),
                            }
                        }
                    }
                }
            }
            steps.extend(next.into_iter().rev());
        }
        Ok(())
    }
}

impl MarkedEventReceiver for AliasGuard<'_> {
    fn on_event(&mut self, event: Event, mark: Marker) {
        if self.failure.is_some() {
            return;
        }
        match event {
            Event::DocumentStart => {
                self.nodes.clear();
                self.open.clear();
                self.anchors.clear();
                self.root = None;
            }
            Event::DocumentEnd => {
                if let Some(root) = self.root {
                    if let Err(error) = self.check_document(root) {
                        self.failure = Some(error);
                    }
                }
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
                self.add(Node::Scalar { merge }, anchor);
            }
            Event::SequenceStart(anchor, _) => {
                let index = self.add(Node::Sequence(Vec::new()), anchor);
                self.open.push(index);
            }
            Event::MappingStart(anchor, _) => {
                let index = self.add(Node::Mapping(Vec::new()), anchor);
                self.open.push(index);
            }
            Event::SequenceEnd | Event::MappingEnd => {
                self.open.pop();
            }
            Event::Alias(anchor) => {
                // yaml-rust2 refuses an unknown anchor before this event.
                if let Some(&target) = self.anchors.get(&anchor) {
                    self.add(
                        Node::Alias {
                            target,
                            mark: mark.index(),
                        },
                        0,
                    );
                }
            }
            Event::StreamStart | Event::StreamEnd | Event::Nothing => {}
        }
    }
}

fn kind(value: &Yaml) -> &'static str {
    match value {
        Yaml::Array(_) => "a sequence",
        Yaml::String(_) => "a string",
        Yaml::Integer(_) => "an integer",
        Yaml::Real(_) => "a float",
        Yaml::Boolean(_) => "a boolean",
        _ => "a value",
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
    fn excessive_aliasing_in_a_later_document_is_refused() {
        let text = format!("project: one\n---\n{BILLION_LAUGHS}");
        assert_eq!(parse(text.as_bytes()).unwrap_err().to_string(), EXCESSIVE);
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
