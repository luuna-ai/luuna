//! Typed API operations; intentionally thinner than command validation.
use crate::{
    json::{Object, Value},
    registry::Registry,
    AtlasError, Result,
};
use std::path::Path;
#[derive(Clone, Debug, Default)]
pub struct RecordOptions {
    pub force: bool,
}
#[derive(Clone, Debug, Default)]
pub struct SearchOptions {
    pub domain: String,
    pub type_name: String,
    pub tag: String,
    pub classification: String,
    pub file: String,
}
#[derive(Clone, Debug, Default)]
pub struct QueryOptions {
    pub type_name: String,
    pub classification: String,
    pub file: String,
}
pub struct RecordResult {
    pub action: String,
    pub record: Object,
}
impl RecordResult {
    pub fn to_value(&self) -> Value {
        let mut o = Object::new();
        o.insert("action", self.action.clone());
        o.insert("record", self.record.clone());
        o.remember_order(["action", "record"]);
        Value::Object(o)
    }
}
pub struct SearchResult {
    pub domain: String,
    pub records: Vec<Object>,
}
impl SearchResult {
    pub fn to_value(&self) -> Value {
        let mut o = Object::new();
        o.insert("domain", self.domain.clone());
        o.insert(
            "records",
            Value::Array(self.records.iter().cloned().map(Value::Object).collect()),
        );
        o.remember_order(["domain", "records"]);
        Value::Object(o)
    }
}
pub struct AppendOutcomeResult {
    pub record: Object,
    pub outcome: Object,
    pub total_outcomes: usize,
}
impl AppendOutcomeResult {
    pub fn to_value(&self) -> Value {
        let mut o = Object::new();
        o.insert("record", self.record.clone());
        o.insert("outcome", self.outcome.clone());
        o.insert("total_outcomes", self.total_outcomes as i64);
        o.remember_order(["record", "outcome", "total_outcomes"]);
        Value::Object(o)
    }
}
pub fn record_expertise(
    _root: &Path,
    _domain: &str,
    _record: Object,
    _options: &RecordOptions,
    _registry: Option<&Registry>,
) -> Result<RecordResult> {
    Err(AtlasError::Message(
        "not implemented: record_expertise".into(),
    ))
}
pub fn search_expertise(
    _root: &Path,
    _query: &str,
    _options: &SearchOptions,
    _registry: Option<&Registry>,
) -> Result<Vec<SearchResult>> {
    Err(AtlasError::Message(
        "not implemented: search_expertise".into(),
    ))
}
pub fn query_domain(
    _root: &Path,
    _domain: &str,
    _options: &QueryOptions,
    _registry: Option<&Registry>,
) -> Result<Vec<Object>> {
    Err(AtlasError::Message("not implemented: query_domain".into()))
}
pub fn edit_record(
    _root: &Path,
    _domain: &str,
    _id: &str,
    _updates: &Object,
    _registry: Option<&Registry>,
) -> Result<Object> {
    Err(AtlasError::Message("not implemented: edit_record".into()))
}
pub fn append_outcome(
    _root: &Path,
    _domain: &str,
    _id: &str,
    _outcome: &Object,
    _registry: Option<&Registry>,
) -> Result<AppendOutcomeResult> {
    Err(AtlasError::Message(
        "not implemented: append_outcome".into(),
    ))
}
