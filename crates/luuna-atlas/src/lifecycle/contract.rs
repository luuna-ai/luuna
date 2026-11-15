//! Exact pinned Atlas compatibility contract; data shared with the later dispatcher.
use crate::json::{self, Object, Value};
use std::sync::OnceLock;
pub fn contract() -> &'static Value {
    static CONTRACT: OnceLock<Value> = OnceLock::new();
    CONTRACT.get_or_init(|| {
        json::parse(include_str!("contract.json")).expect("embedded pinned contract is valid JSON")
    })
}
pub fn config_schema() -> &'static Object {
    contract()
        .as_object()
        .and_then(|o| o.get("configSchema"))
        .and_then(Value::as_object)
        .expect("pinned contract contains configSchema")
}
