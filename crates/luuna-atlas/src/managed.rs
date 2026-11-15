//! Opt-in resource limits for a managed native job; raw library calls keep Go semantics.
use crate::{json::Value, AtlasError, Result};
use std::cell::Cell;
#[derive(Clone, Copy, Debug)]
pub struct YamlLimits {
    pub max_input_bytes: usize,
    pub max_depth: usize,
    pub max_nodes: usize,
    pub max_scalar_bytes: usize,
    pub max_text_bytes: usize,
}
impl Default for YamlLimits {
    fn default() -> Self {
        Self {
            max_input_bytes: 256 * 1024,
            max_depth: 96,
            max_nodes: 100_000,
            max_scalar_bytes: 8 * 1024 * 1024,
            max_text_bytes: 4 * 1024 * 1024,
        }
    }
}
thread_local! {static YAML_LIMITS:Cell<Option<YamlLimits>>=const{Cell::new(None)};}
struct Restore(Option<YamlLimits>);
impl Drop for Restore {
    fn drop(&mut self) {
        YAML_LIMITS.with(|s| s.set(self.0));
    }
}
/// Scope parser limits to this blocking job/thread, restoring an outer policy even on unwind.
/// This neither changes process environment nor affects other concurrent requests.
pub fn with_yaml_limits<T>(limits: YamlLimits, run: impl FnOnce() -> T) -> T {
    let previous = YAML_LIMITS.with(|s| s.replace(Some(limits)));
    let _restore = Restore(previous);
    run()
}
pub(crate) fn yaml_limits() -> Option<YamlLimits> {
    YAML_LIMITS.with(Cell::get)
}
fn refusal() -> AtlasError {
    AtlasError::Message("Refusing managed YAML: source or expanded representation exceeds this call's bounded input/output contract. No configuration was written.".into())
}
pub(crate) fn yaml_source(bytes: usize) -> Result<()> {
    if yaml_limits().is_some_and(|l| bytes > l.max_input_bytes) {
        Err(refusal())
    } else {
        Ok(())
    }
}
pub(crate) struct YamlBudget {
    limits: Option<YamlLimits>,
    nodes: usize,
    bytes: usize,
    text: usize,
}
impl YamlBudget {
    pub fn new() -> Self {
        Self {
            limits: yaml_limits(),
            nodes: 0,
            bytes: 0,
            text: 0,
        }
    }
    pub fn visit(&mut self, bytes: usize, depth: usize) -> Result<()> {
        let Some(limits) = self.limits else {
            return Ok(());
        };
        self.nodes = self.nodes.saturating_add(1);
        self.bytes = self.bytes.saturating_add(bytes);
        self.text = self.text.saturating_add(
            depth
                .saturating_mul(4)
                .saturating_add(bytes.saturating_mul(6))
                .saturating_add(24),
        );
        if depth > limits.max_depth
            || self.nodes > limits.max_nodes
            || self.bytes > limits.max_scalar_bytes
            || self.text > limits.max_text_bytes
        {
            return Err(refusal());
        }
        Ok(())
    }
}
/// Check an expanded value against the active managed request policy.
pub fn check_value(value: &Value) -> Result<()> {
    if yaml_limits().is_none() {
        return Ok(());
    }
    let mut budget = YamlBudget::new();
    let mut stack = vec![(value, 0usize)];
    while let Some((v, depth)) = stack.pop() {
        let bytes = match v {
            Value::String(s) => s.len(),
            Value::RawString(_) => v.string_bytes().unwrap().len(),
            Value::Array(a) => {
                stack.extend(a.iter().map(|v| (v, depth + 1)));
                0
            }
            Value::Object(o) => {
                for (k, v) in o.iter() {
                    budget.visit(o.key_bytes(k).len(), depth + 1)?;
                    stack.push((v, depth + 1));
                }
                0
            }
            _ => 32,
        };
        budget.visit(bytes, depth)?;
    }
    Ok(())
}
