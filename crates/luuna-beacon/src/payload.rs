//! The two documents the luuna runtime reads: `render --json` and the named
//! form of `validate --json`, as `serde_json::Value` (assumption AS5).
//!
//! Each is the object Go prints (`cli.go:645-648`, `cli.go:1564-1566`), as
//! `serde_json` reads Go's output, plus the keys the runtime needs and Go
//! does not print (A4 differences, `runtime_differences` of the plan):
//!
//! - `resolvedFrom` in the render document (RD1, assumptions AS2 and AS3);
//! - top-level `valid`, `errors` and `warnings` in the named validate
//!   document, equal to its single result (RD2, assumption AS4).
//!
//! Nothing else in the crate writes these keys, and they are never stored.

use crate::gojson;
use crate::model::Prompt;
use serde_json::{Map, Number, Value};

/// A rendered prompt: the record with inheritance, mixins and Atlas
/// metadata resolved (`cli.go:455-585`), and the names of every prompt the
/// resolver used.
#[derive(Debug, Clone, PartialEq)]
pub struct Rendered {
    /// The resolved record; `render --format json` prints exactly this.
    pub prompt: Prompt,
    /// The prompts the resolver visited, by name: the focal prompt first,
    /// then the parent's own list, then each mixin's list left to right; a
    /// name is listed once, at its first position; nothing is truncated.
    pub resolved_from: Vec<String>,
}

/// The validation of one prompt against its schema (`cli.go:1531-1565`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Validation {
    pub name: String,
    pub valid: bool,
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
}

impl Rendered {
    /// The document Go prints for `render <name> --json` (`cli.go:645-648`:
    /// `success`, `command`, `name`, `version`, `sections`, `frontmatter`,
    /// which is `null` when empty, and `atlas` only when present), plus
    /// `resolvedFrom` (RD1).
    pub fn to_payload(&self) -> Value {
        let prompt = &self.prompt;
        let mut payload = Map::new();
        payload.insert("success".to_owned(), Value::Bool(true));
        payload.insert("command".to_owned(), Value::String("render".to_owned()));
        payload.insert("name".to_owned(), Value::String(prompt.name.clone()));
        payload.insert("version".to_owned(), Value::from(prompt.version));

        // Sections and Atlas are shallow: write them as Go writes them and
        // read the text back, so every number has the form serde_json gives
        // Go's output. Frontmatter is converted separately, without
        // recursion, because it can be nested 10000 levels deep.
        let shallow = Prompt {
            sections: prompt.sections.clone(),
            atlas: prompt.atlas.clone(),
            ..Prompt::default()
        };
        let mut record: Value = serde_json::from_str(&shallow.to_go_json())
            .expect("a record without frontmatter is shallow, valid JSON");
        let sections = record
            .get_mut("sections")
            .map(Value::take)
            .unwrap_or(Value::Null);
        payload.insert("sections".to_owned(), sections);
        if let Some(atlas) = record.get_mut("atlas").map(Value::take) {
            payload.insert("atlas".to_owned(), atlas);
        }

        let frontmatter = if prompt.frontmatter.is_empty() {
            Value::Null
        } else {
            let mut object = Map::new();
            for (key, value) in &prompt.frontmatter {
                object.insert(key.clone(), as_printed(value));
            }
            Value::Object(object)
        };
        payload.insert("frontmatter".to_owned(), frontmatter);

        payload.insert(
            "resolvedFrom".to_owned(),
            Value::Array(
                self.resolved_from
                    .iter()
                    .cloned()
                    .map(Value::String)
                    .collect(),
            ),
        );
        Value::Object(payload)
    }
}

impl Validation {
    /// The `results` entry Go prints for this prompt (`cli.go:1564`).
    fn entry(&self) -> Value {
        let mut entry = Map::new();
        entry.insert("name".to_owned(), Value::String(self.name.clone()));
        entry.insert("valid".to_owned(), Value::Bool(self.valid));
        entry.insert("errors".to_owned(), strings(&self.errors));
        entry.insert("warnings".to_owned(), strings(&self.warnings));
        Value::Object(entry)
    }

    /// The document Go prints for `validate <name> --json` (`cli.go:1566`:
    /// `success` equal to `valid`, `command`, `results` holding this one
    /// entry), plus top-level `valid`, `errors` and `warnings` equal to the
    /// entry's (RD2). For an invalid prompt Go then prints its
    /// `validation failed` error document; that belongs to the caller.
    pub fn to_named_payload(&self) -> Value {
        let mut payload = Map::new();
        payload.insert("success".to_owned(), Value::Bool(self.valid));
        payload.insert("command".to_owned(), Value::String("validate".to_owned()));
        payload.insert("results".to_owned(), Value::Array(vec![self.entry()]));
        payload.insert("valid".to_owned(), Value::Bool(self.valid));
        payload.insert("errors".to_owned(), strings(&self.errors));
        payload.insert("warnings".to_owned(), strings(&self.warnings));
        Value::Object(payload)
    }
}

fn strings(values: &[String]) -> Value {
    Value::Array(values.iter().cloned().map(Value::String).collect())
}

/// A frontmatter value as serde_json reads it from Go's output: the same
/// structure, with each number (a Go `float64`) in the form serde_json gives
/// the text Go writes for it (`1` is an integer, `2.5` and `1e+21` floats).
/// Walks with an explicit stack, so depth costs no call stack.
fn as_printed(value: &Value) -> Value {
    enum Open<'a> {
        Array(std::slice::Iter<'a, Value>, Vec<Value>),
        Object(
            serde_json::map::Iter<'a>,
            Map<String, Value>,
            Option<String>,
        ),
    }

    fn leaf(value: &Value) -> Option<Value> {
        Some(match value {
            Value::Null => Value::Null,
            Value::Bool(flag) => Value::Bool(*flag),
            Value::String(text) => Value::String(text.clone()),
            Value::Number(number) => Value::Number(printed_number(number)),
            Value::Array(_) | Value::Object(_) => return None,
        })
    }

    fn open(value: &Value) -> Open<'_> {
        match value {
            Value::Array(items) => Open::Array(items.iter(), Vec::with_capacity(items.len())),
            Value::Object(map) => Open::Object(map.iter(), Map::new(), None),
            _ => unreachable!("only containers are opened"),
        }
    }

    if let Some(done) = leaf(value) {
        return done;
    }
    let mut stack = vec![open(value)];
    loop {
        let top = stack.last_mut().expect("an open container");
        let next = match top {
            Open::Array(items, _) => items.next(),
            Open::Object(entries, _, pending) => entries.next().map(|(key, item)| {
                *pending = Some(key.clone());
                item
            }),
        };
        let finished = match next {
            Some(child) => match leaf(child) {
                Some(done) => done,
                None => {
                    stack.push(open(child));
                    continue;
                }
            },
            None => {
                let done = match stack.pop().expect("an open container") {
                    Open::Array(_, items) => Value::Array(items),
                    Open::Object(_, map, _) => Value::Object(map),
                };
                if stack.is_empty() {
                    return done;
                }
                done
            }
        };
        match stack.last_mut().expect("a parent for the finished value") {
            Open::Array(_, items) => items.push(finished),
            Open::Object(_, map, pending) => {
                let key = pending.take().expect("a key for the value");
                map.insert(key, finished);
            }
        }
    }
}

/// The number serde_json reads from the text Go writes for this `float64`.
fn printed_number(number: &Number) -> Number {
    let mut text = String::new();
    gojson::write_float(&mut text, number.as_f64().unwrap_or(f64::NAN));
    match serde_json::from_str::<Value>(&text) {
        Ok(Value::Number(printed)) => printed,
        _ => number.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Atlas, AtlasPrime, Section};
    use serde_json::json;
    use std::collections::BTreeMap;

    #[test]
    fn render_payload_matches_go_shape() {
        let mut frontmatter = BTreeMap::new();
        frontmatter.insert(
            "n".to_owned(),
            json!({"a": [Number::from_f64(1.0), Number::from_f64(2.5), {"b": null}], "big": Number::from_f64(1e21)}).into(),
        );
        let rendered = Rendered {
            prompt: Prompt {
                name: "p".to_owned(),
                version: 3,
                sections: Some(vec![Section {
                    name: "Task".to_owned(),
                    body: "<b>".to_owned(),
                    required: Some(true),
                }]),
                frontmatter,
                atlas: Some(Atlas {
                    prime: Some(AtlasPrime {
                        domains: vec!["d".to_owned()],
                        files: vec![],
                    }),
                    budget: Some(100.0),
                    on_empty: String::new(),
                }),
                ..Prompt::default()
            },
            resolved_from: vec!["p".to_owned(), "q".to_owned()],
        };
        let expected: Value = serde_json::from_str(
            r#"{"success":true,"command":"render","name":"p","version":3,
                "sections":[{"name":"Task","body":"<b>","required":true}],
                "frontmatter":{"n":{"a":[1,2.5,{"b":null}],"big":1e+21}},
                "atlas":{"prime":{"domains":["d"]},"budget":100},
                "resolvedFrom":["p","q"]}"#,
        )
        .unwrap();
        assert_eq!(rendered.to_payload(), expected);

        let bare = Rendered {
            prompt: Prompt {
                sections: Some(vec![]),
                ..Prompt::default()
            },
            resolved_from: vec![],
        };
        let payload = bare.to_payload();
        assert_eq!(payload["frontmatter"], Value::Null);
        assert_eq!(payload["sections"], json!([]));
        assert!(payload.get("atlas").is_none());
    }

    #[test]
    fn deep_frontmatter_is_converted_without_recursion() {
        let depth = 10_000;
        let mut value = Value::Number(Number::from_f64(7.0).unwrap());
        for _ in 0..depth {
            value = Value::Array(vec![value]);
        }
        let printed = std::thread::Builder::new()
            .stack_size(256 << 10)
            .spawn(move || {
                let printed = as_printed(&value);
                // Unwrap both iteratively so that neither drop recurses deeply.
                let mut levels = 0;
                let mut cursor = printed;
                let mut original = value;
                loop {
                    match (cursor, original) {
                        (Value::Array(mut a), Value::Array(mut b)) => {
                            cursor = a.pop().unwrap();
                            original = b.pop().unwrap();
                            levels += 1;
                        }
                        (leaf, _) => break (levels, leaf),
                    }
                }
            })
            .unwrap()
            .join()
            .unwrap();
        assert_eq!(printed, (depth, json!(7)));
    }

    #[test]
    fn named_validate_payload_repeats_its_result_at_top_level() {
        let validation = Validation {
            name: "p".to_owned(),
            valid: false,
            errors: vec!["Required section \"Task\" is missing".to_owned()],
            warnings: vec![],
        };
        assert_eq!(
            validation.to_named_payload(),
            json!({
                "success": false,
                "command": "validate",
                "results": [{
                    "name": "p",
                    "valid": false,
                    "errors": ["Required section \"Task\" is missing"],
                    "warnings": [],
                }],
                "valid": false,
                "errors": ["Required section \"Task\" is missing"],
                "warnings": [],
            })
        );
    }
}
