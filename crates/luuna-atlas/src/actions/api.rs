//! Typed API operations; intentionally thinner than command validation.
use super::record::{domain_names, message, text};
use crate::{config, records, store, AtlasStore};
use crate::{
    json::{Object, Value},
    registry::Registry,
    AtlasError, Result,
};
use std::path::{Path, PathBuf};
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

pub(crate) fn domain_path(root: &Path, domain: &str) -> Result<PathBuf> {
    let store = AtlasStore::open(root)?;
    let cfg = config::read_config(&store.atlas_dir())?;
    if !records::obj(cfg.get("domains")).contains_key(domain) {
        let names = domain_names(&cfg).join(", ");
        return Err(message(format!(
            "Domain \"{domain}\" not found in config. Available domains: {}",
            if names.is_empty() { "(none)" } else { &names }
        )));
    }
    store.expertise_path(domain)
}
pub fn record_expertise(
    root: &Path,
    domain: &str,
    mut record: Object,
    options: &RecordOptions,
    registry: Option<&Registry>,
) -> Result<RecordResult> {
    record_expertise_mut(root, domain, &mut record, options, registry)
}
/// The bridge uses the mutable form to preserve Go's input mutation even if
/// ID assignment succeeds and the subsequent file operation fails.
pub fn record_expertise_mut(
    root: &Path,
    domain: &str,
    record: &mut Object,
    options: &RecordOptions,
    registry: Option<&Registry>,
) -> Result<RecordResult> {
    let path = domain_path(root, domain)?;
    let builtin = Registry::from_config(None)?;
    let registry = registry.unwrap_or(&builtin);
    store::with_file_lock(&path, || {
        let mut current = store::read_records(&path, registry, false)?;
        // An uncomparable dynamic slice type makes Go panic once a nonempty
        // dedup value causes comparison with another slice. Refuse before write.
        if matches!(record.get("type"), Some(Value::Array(_))) {
            if let Some(def) = registry.get(&text(record.get("type"))) {
                let key = if def.dedup_key == "content_hash" {
                    records::normalize_text(&records::record_body_text(record, registry))
                } else {
                    record
                        .get(&def.dedup_key)
                        .and_then(Value::as_str)
                        .map(records::normalize_text)
                        .unwrap_or_default()
                };
                if !key.is_empty()
                    && current
                        .iter()
                        .any(|r| matches!(r.get("type"), Some(Value::Array(_))))
                {
                    return Err(message("Refusing the record comparison: Go would panic on non-comparable array types. The file is unchanged."));
                }
            }
        }
        if let Some(index) =
            records::find_duplicate(&current, record, registry).filter(|_| !options.force)
        {
            if ["pattern", "decision", "reference", "guide"]
                .contains(&text(record.get("type")).as_str())
            {
                let before = current.clone();
                current[index] = record.clone();
                records::check_lossless_rewrite(&path, registry, &[index])?;
                records::check_retained_ids(
                    &path,
                    before
                        .iter()
                        .enumerate()
                        .filter(|(i, _)| *i != index)
                        .map(|(i, r)| (r, &current[i])),
                )?;
                records::check_retained_numbers(&path, before.iter().zip(current.iter()))?;
                // Match Go's per-map assignment/encoding order, including
                // input-map mutations visible when a later row fails.
                for (position, row) in current.iter_mut().enumerate() {
                    let assigned = records::assign_record_ids(std::slice::from_mut(row), registry);
                    if position == index {
                        *record = row.clone();
                    }
                    assigned?;
                    let _ = crate::json::to_compact_string(&Value::Object(row.clone()))?;
                }
                store::write_records(&path, &mut current, registry)?;
                *record = current[index].clone();
                return Ok(RecordResult {
                    action: "updated".into(),
                    record: record.clone(),
                });
            }
            return Ok(RecordResult {
                action: "skipped".into(),
                record: current[index].clone(),
            });
        }
        records::assign_record_ids(std::slice::from_mut(record), registry)?;
        store::append_record(&path, record, registry)?;
        Ok(RecordResult {
            action: "created".into(),
            record: record.clone(),
        })
    })
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
