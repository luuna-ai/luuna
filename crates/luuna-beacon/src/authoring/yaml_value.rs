//! Flat ownership keeps hostile nesting independent of the machine stack.
use crate::BeaconError;
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq)]
pub(super) enum Node {
    Null,
    Bool(bool),
    Int(i64),
    Uint(u64),
    Float(f64),
    String(Arc<[u8]>),
    Time(String),
    Seq(Vec<usize>),
    Map(Vec<(usize, usize)>),
}

/// A Go YAML value. Node ownership is flat, including collections and aliases.
#[derive(Clone)]
pub struct YamlValue {
    pub(super) nodes: Vec<Node>,
    pub(super) root: usize,
}
impl std::fmt::Debug for YamlValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("YamlValue")
            .field("nodes", &self.nodes)
            .field("root", &self.root)
            .finish()
    }
}
// Each collection owns indices, never another collection.
impl Drop for YamlValue {
    fn drop(&mut self) {
        self.nodes.clear();
    }
}
impl YamlValue {
    pub(super) fn null() -> Self {
        Self {
            nodes: vec![Node::Null],
            root: 0,
        }
    }
    pub(super) fn push(&mut self, node: Node) -> usize {
        let n = self.nodes.len();
        self.nodes.push(node);
        n
    }
    /// Whether the root is the YAML null value.
    pub fn is_null(&self) -> bool {
        matches!(self.nodes[self.root], Node::Null)
    }
    /// Encode with the Go tool's YAML conventions.
    pub fn to_yaml(&self) -> String {
        super::yaml_encode::encode(self)
    }
    /// Encode with Go's JSON value conventions.
    pub fn to_go_json(&self) -> Result<String, BeaconError> {
        super::yaml_json::encode(self)
    }
    /// JSON numeric values enter a Go interface as float64.
    pub fn from_json(value: &serde_json::Value) -> Self {
        let mut out = Self::null();
        let mut pending = vec![(value, 0)];
        while let Some((value, id)) = pending.pop() {
            out.nodes[id] = match value {
                serde_json::Value::Null => Node::Null,
                serde_json::Value::Bool(v) => Node::Bool(*v),
                serde_json::Value::Number(v) => Node::Float(v.as_f64().unwrap_or(0.0)),
                serde_json::Value::String(v) => Node::String(Arc::from(v.as_bytes())),
                serde_json::Value::Array(v) => {
                    let mut children = Vec::with_capacity(v.len());
                    for child in v {
                        let n = out.push(Node::Null);
                        children.push(n);
                        pending.push((child, n));
                    }
                    Node::Seq(children)
                }
                serde_json::Value::Object(v) => {
                    let mut children = Vec::with_capacity(v.len());
                    for (key, value) in v {
                        let k = out.push(Node::String(Arc::from(key.as_bytes())));
                        let n = out.push(Node::Null);
                        children.push((k, n));
                        pending.push((value, n));
                    }
                    Node::Map(children)
                }
            };
        }
        out
    }
}

impl PartialEq for YamlValue {
    fn eq(&self, other: &Self) -> bool {
        self.difference(other).is_none()
    }
}
impl YamlValue {
    pub(super) fn string(&self, id: usize) -> Option<&str> {
        match &self.nodes[id] {
            Node::String(s) => std::str::from_utf8(s).ok(),
            _ => None,
        }
    }
    pub(super) fn fields(&self, id: usize) -> Option<&[(usize, usize)]> {
        match &self.nodes[id] {
            Node::Map(p) => Some(p),
            _ => None,
        }
    }
    pub(super) fn string_fields(&self, id: usize) -> Option<&[(usize, usize)]> {
        let fields = self.fields(id)?;
        fields
            .iter()
            .all(|(k, _)| matches!(self.nodes[*k], Node::String(_)))
            .then_some(fields)
    }
    pub(super) fn field(&self, id: usize, name: &str) -> Option<usize> {
        self.string_fields(id)?
            .iter()
            .find(|(k, _)| self.string(*k) == Some(name))
            .map(|(_, v)| *v)
    }
    pub(super) fn subtree(&self, root: usize) -> Self {
        let mut v = self.clone();
        v.root = root;
        v
    }
    pub(super) fn append(&mut self, other: &Self) -> usize {
        let offset = self.nodes.len();
        self.nodes.extend(other.nodes.iter().map(|n| match n {
            Node::Seq(ids) => Node::Seq(ids.iter().map(|i| i + offset).collect()),
            Node::Map(p) => Node::Map(p.iter().map(|(k, v)| (k + offset, v + offset)).collect()),
            _ => n.clone(),
        }));
        other.root + offset
    }
    pub(super) fn difference(&self, other: &Self) -> Option<String> {
        let mut pending = vec![(self.root, other.root, String::new())];
        while let Some((a, b, path)) = pending.pop() {
            match (&self.nodes[a], &other.nodes[b]) {
                (Node::Seq(a), Node::Seq(b)) => {
                    if a.len() != b.len() {
                        return Some(path);
                    }
                    for (i, (&a, &b)) in a.iter().zip(b).enumerate().rev() {
                        pending.push((a, b, join(&path, &i.to_string())))
                    }
                }
                (Node::Map(a), Node::Map(b)) => {
                    if a.len() != b.len() {
                        return Some(path);
                    }
                    for &(k, v) in a.iter().rev() {
                        let Some(&(_, w)) = b
                            .iter()
                            .find(|(key, _)| scalar_equal(&self.nodes[k], &other.nodes[*key]))
                        else {
                            return Some(join(&path, self.string(k).unwrap_or("?")));
                        };
                        pending.push((v, w, join(&path, self.string(k).unwrap_or("?"))))
                    }
                }
                (a, b) => {
                    if !scalar_equal(a, b) {
                        return Some(path);
                    }
                }
            }
        }
        None
    }
}
fn join(parent: &str, name: &str) -> String {
    if parent.is_empty() {
        name.into()
    } else {
        format!("{parent}.{name}")
    }
}
fn scalar_equal(a: &Node, b: &Node) -> bool {
    match (a, b) {
        (Node::Float(a), Node::Float(b)) => {
            a.to_bits() == b.to_bits() || a.is_nan() && b.is_nan() || a == b && *a != 0.0
        }
        (Node::Int(a), Node::Float(b)) | (Node::Float(b), Node::Int(a)) => {
            *b >= i64::MIN as f64
                && *b < 9223372036854775808.0
                && b.fract() == 0.0
                && *b as i64 == *a
                && !(*a == 0 && b.is_sign_negative())
        }
        (Node::Uint(a), Node::Float(b)) | (Node::Float(b), Node::Uint(a)) => {
            *b >= 0.0
                && *b < 18446744073709551616.0
                && b.fract() == 0.0
                && *b as u64 == *a
                && !(*a == 0 && b.is_sign_negative())
        }
        (Node::Int(a), Node::Uint(b)) | (Node::Uint(b), Node::Int(a)) => *a >= 0 && *a as u64 == *b,
        (Node::Time(t), Node::String(s)) | (Node::String(s), Node::Time(t)) => t.as_bytes() == &**s,
        _ => a == b,
    }
}
