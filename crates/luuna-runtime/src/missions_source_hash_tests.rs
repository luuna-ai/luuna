//! Pins the ticket source hash: its bytes are SHA-256 over compact JSON whose
//! object keys are in sorted order, whatever order they were inserted in.

use super::source_hash;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

const PINNED: &str = "298da7c808ee30512518291aa70676542181881f19870bae8ac593c5861707a7";

fn issue() -> Value {
    json!({
        "id": "T-1",
        "title": "Pin the hash",
        "priority": 2,
        "estimate": 1.5,
        "done": false,
        "parent": null,
        "labels": ["b", "a"],
        "meta": {"zeta": {"y": 1, "x": [true, null]}, "alpha": "text"},
        "blockedBy": ["T-0", "T-9"]
    })
}

fn all() -> Vec<Value> {
    vec![
        json!({"id": "T-0", "title": "Dependency", "status": "open", "meta": {"b": 1, "a": 2}}),
        json!({"id": "T-5", "title": "Unrelated"}),
    ]
}

fn sha(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))
}

#[test]
fn source_hash_is_the_pinned_digest() {
    assert_eq!(source_hash(&issue(), &all()).unwrap(), PINNED);
}

#[test]
fn source_hash_is_sha256_of_key_sorted_compact_json() {
    let text = concat!(
        r#"{"dependencies":["#,
        r#"{"id":"T-0","meta":{"a":2,"b":1},"status":"open","title":"Dependency"},"#,
        r#"{"id":"T-9","missing":true}],"#,
        r#""issue":{"blockedBy":["T-0","T-9"],"done":false,"estimate":1.5,"id":"T-1","#,
        r#""labels":["b","a"],"meta":{"alpha":"text","zeta":{"x":[true,null],"y":1}},"#,
        r#""parent":null,"priority":2,"title":"Pin the hash"}}"#
    );
    assert_eq!(source_hash(&issue(), &all()).unwrap(), sha(text));
}

#[test]
fn source_hash_ignores_key_insertion_order() {
    let forward = issue();
    let mut reversed = Map::new();
    let object = forward.as_object().unwrap();
    for (key, value) in object.iter().rev() {
        let value = match value {
            Value::Object(inner) => {
                let mut flipped = Map::new();
                for (k, v) in inner.iter().rev() {
                    flipped.insert(k.clone(), v.clone());
                }
                Value::Object(flipped)
            }
            other => other.clone(),
        };
        reversed.insert(key.clone(), value);
    }
    assert_eq!(
        source_hash(&Value::Object(reversed), &all()).unwrap(),
        source_hash(&forward, &all()).unwrap()
    );
}
