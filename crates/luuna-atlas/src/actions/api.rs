//! Typed API operations; intentionally thinner than command validation.
use super::record::{domain_names, message, text};
use crate::{config, records, store, AtlasStore};
use crate::{
    json::{Object, Value},
    registry::Registry,
    Result,
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
    root: &Path,
    query: &str,
    options: &SearchOptions,
    registry: Option<&Registry>,
) -> Result<Vec<SearchResult>> {
    let store = AtlasStore::open(root)?;
    let cfg = config::read_config(&store.atlas_dir())?;
    let mut domains = domain_names(&cfg);
    if !options.domain.is_empty() {
        if !records::obj(cfg.get("domains")).contains_key(&options.domain) {
            return Err(message(format!(
                "Domain \"{}\" not found in config. Available domains: {}",
                options.domain,
                domains.join(", ")
            )));
        }
        domains = vec![options.domain.clone()];
    }
    let builtin = Registry::from_config(None)?;
    let registry = registry.unwrap_or(&builtin);
    let mut out = Vec::new();
    for domain in domains {
        let rows = store::read_records(&store.expertise_path(&domain)?, registry, false)?;
        let rows = filter(
            rows,
            &options.type_name,
            &options.tag,
            &options.classification,
            &options.file,
        );
        let matches = records::search_records(&rows, query, 0.0);
        if !matches.is_empty() {
            out.push(SearchResult {
                domain,
                records: matches,
            });
        }
    }
    Ok(out)
}
pub fn query_domain(
    root: &Path,
    domain: &str,
    options: &QueryOptions,
    registry: Option<&Registry>,
) -> Result<Vec<Object>> {
    let path = domain_path(root, domain)?;
    let builtin = Registry::from_config(None)?;
    let registry = registry.unwrap_or(&builtin);
    Ok(filter(
        store::read_records(&path, registry, false)?,
        &options.type_name,
        "",
        &options.classification,
        &options.file,
    ))
}
pub(crate) fn filter(
    rows: Vec<Object>,
    typ: &str,
    tag: &str,
    class: &str,
    file: &str,
) -> Vec<Object> {
    rows.into_iter()
        .filter(|r| {
            (typ.is_empty() || text(r.get("type")) == typ)
                && (class.is_empty() || text(r.get("classification")) == class)
                && (tag.is_empty()
                    || records::list(r.get("tags"))
                        .iter()
                        .any(|t| records::js_lower(t) == records::js_lower(tag)))
                && (file.is_empty()
                    || records::list(r.get("files"))
                        .iter()
                        .any(|f| records::js_lower(f).contains(&records::js_lower(file))))
        })
        .collect()
}
fn update_record(
    root: &Path,
    domain: &str,
    id: &str,
    registry: Option<&Registry>,
    mutate: impl FnOnce(&mut Object, &Path) -> Result<()>,
) -> Result<Object> {
    let path = domain_path(root, domain)?;
    let builtin = Registry::from_config(None)?;
    let registry = registry.unwrap_or(&builtin);
    store::with_file_lock(&path, || {
        let mut current = store::read_records(&path, registry, false)?;
        let index = records::resolve_record_id(&current, id)?;
        let before = current.clone();
        let mut updated = current[index].clone();
        let order: Vec<String> = updated.keys().map(str::to_owned).collect();
        updated.remember_order(order);
        mutate(&mut updated, &path)?;
        current[index] = updated;
        records::check_lossless_rewrite(&path, registry, &[])?;
        records::check_retained_ids(&path, before.iter().zip(current.iter()))?;
        records::check_retained_numbers(&path, before.iter().zip(current.iter()))?;
        records::assign_record_ids(&mut current, registry)?;
        store::write_records(&path, &mut current, registry)?;
        Ok(current[index].clone())
    })
}
pub fn edit_record(
    root: &Path,
    domain: &str,
    id: &str,
    updates: &Object,
    registry: Option<&Registry>,
) -> Result<Object> {
    update_record(root, domain, id, registry, |record, _| {
        let fields: &[&str] = match text(record.get("type")).as_str() {
            "convention" => &["content"],
            "pattern" | "reference" => &["name", "description", "files"],
            "failure" => &["description", "resolution"],
            "decision" => &["title", "rationale"],
            "guide" => &["name", "description"],
            _ => &[],
        };
        for field in [
            "classification",
            "tags",
            "relates_to",
            "supersedes",
            "outcomes",
        ]
        .iter()
        .chain(fields.iter())
        {
            if let Some(value) = updates.get(field) {
                record.insert_unordered(*field, value.clone());
            }
        }
        Ok(())
    })
}
pub fn append_outcome(
    root: &Path,
    domain: &str,
    id: &str,
    outcome: &Object,
    registry: Option<&Registry>,
) -> Result<AppendOutcomeResult> {
    let mut next = Object::new();
    for (key, value) in outcome.iter() {
        next.insert_unordered(key, value.clone());
    }
    if matches!(next.get("recorded_at"), None | Some(Value::Null)) {
        next.insert_unordered("recorded_at", crate::timefmt::now_iso());
    }
    let record = update_record(root, domain, id, registry, |record, path| {
        records::check_outcomes_collection(path, record, false)?;
        let mut values = match record.get("outcomes") {
            Some(Value::Array(values)) => values.clone(),
            _ => Vec::new(),
        };
        values.push(Value::Object(next.clone()));
        record.insert_unordered("outcomes", Value::Array(values));
        Ok(())
    })?;
    let total_outcomes = match record.get("outcomes") {
        Some(Value::Array(values)) => values.len(),
        _ => 0,
    };
    Ok(AppendOutcomeResult {
        record,
        outcome: next,
        total_outcomes,
    })
}
