//! Human renderers shared with later native commands.
use crate::{
    json::{Object, Value},
    registry::{Registry, TypeDefinition},
};
use chrono::{DateTime, Utc};
use std::collections::{BTreeMap, BTreeSet};
#[derive(Default)]
pub struct FormatOptions {
    pub format: String,
    pub full: bool,
    pub annotations: BTreeMap<String, String>,
    pub stale_ids: BTreeSet<String>,
}
pub fn xml_escape(_text: &str) -> String {
    "not implemented: xml_escape".into()
}
pub fn format_links(_record: &Object) -> String {
    "not implemented: format_links".into()
}
pub fn format_evidence(_value: &Value) -> String {
    "not implemented: format_evidence".into()
}
pub fn format_record_meta(_record: &Object, _full: bool) -> String {
    "not implemented: format_record_meta".into()
}
pub fn format_record_index(_record: &Object, _registry: &Registry, _stale: bool) -> String {
    "not implemented: format_record_index".into()
}
pub fn format_markdown_records(
    _def: &TypeDefinition,
    _records: &[Object],
    _full: bool,
    _registry: &Registry,
    _annotations: &BTreeMap<String, String>,
) -> String {
    "not implemented: format_markdown_records".into()
}
pub fn format_plain_records(
    _def: &TypeDefinition,
    _records: &[Object],
    _annotations: &BTreeMap<String, String>,
) -> Vec<String> {
    vec!["not implemented: format_plain_records".into()]
}
pub fn format_xml_record(
    _record: &Object,
    _def: &TypeDefinition,
    _annotations: &BTreeMap<String, String>,
) -> Vec<String> {
    vec!["not implemented: format_xml_record".into()]
}
pub fn format_domain(
    _domain: &str,
    _records: &[Object],
    _last_updated: Option<DateTime<Utc>>,
    _registry: &Registry,
    _opts: &FormatOptions,
) -> String {
    "not implemented: format_domain".into()
}
pub fn format_type_records(_records: &[Object], _registry: &Registry, _full: bool) -> String {
    "not implemented: format_type_records".into()
}
