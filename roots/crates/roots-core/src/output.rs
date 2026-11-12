//! Stable JSON envelopes shared by every Roots command.
//!
//! The TypeScript CLI promised one pretty-printed document on stdout whenever
//! `--json` is selected. Keeping that serialization contract in core makes it
//! equally available to the process CLI, hooks, and future embedding callers.

use crate::errors::ErrorCode;
use serde_json::{Map, Value};

/// Serialize an object using Roots' newline-terminated, pretty JSON format.
pub fn pretty_json(value: &Value) -> String {
    format!(
        "{}\n",
        serde_json::to_string_pretty(value).expect("JSON values are serializable")
    )
}

/// Make the common successful command result envelope.
///
/// Keys from `data` intentionally overwrite earlier keys, matching JavaScript
/// object spread in the legacy implementation.
pub fn success_envelope(command: &str, data: Map<String, Value>) -> String {
    let mut body = Map::new();
    body.insert("success".to_owned(), Value::Bool(true));
    body.insert("command".to_owned(), Value::String(command.to_owned()));
    body.extend(data);
    pretty_json(&Value::Object(body))
}

/// Make the common failed-command result envelope.
///
/// `detail` is deliberately spread last for compatibility with the former
/// JavaScript object construction.
pub fn error_envelope(
    command: Option<&str>,
    message: &str,
    code: ErrorCode,
    detail: Option<&Map<String, Value>>,
) -> String {
    let mut body = Map::new();
    body.insert("success".to_owned(), Value::Bool(false));
    body.insert(
        "command".to_owned(),
        command.map_or(Value::Null, |value| Value::String(value.to_owned())),
    );
    body.insert("error".to_owned(), Value::String(message.to_owned()));
    body.insert("code".to_owned(), Value::String(code.as_str().to_owned()));
    if let Some(detail) = detail {
        body.extend(detail.clone());
    }
    pretty_json(&Value::Object(body))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_success_and_error_contracts() {
        let mut data = Map::new();
        data.insert("count".to_owned(), Value::from(2));
        assert_eq!(
            success_envelope("list", data),
            "{\n  \"success\": true,\n  \"command\": \"list\",\n  \"count\": 2\n}\n"
        );

        let mut detail = Map::new();
        detail.insert("suggestion".to_owned(), Value::String("show".to_owned()));
        assert_eq!(
            error_envelope(Some("shw"), "bad command", ErrorCode::Usage, Some(&detail)),
            "{\n  \"success\": false,\n  \"command\": \"shw\",\n  \"error\": \"bad command\",\n  \"code\": \"usage\",\n  \"suggestion\": \"show\"\n}\n"
        );
    }
}
