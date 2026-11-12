//! Record schema validation. Owner: s5-atlas-lib-2.
//!
//! A port of the Go tool's `schema.go`: the record validator (`ValidateRecord`,
//! `ValidateRecordValue`, `recordVariantSchema`, `validateSchemaAt`) with Go's keyword
//! order, early returns and messages, and the domain rules (`GetAllowedTypes`,
//! `GetRequiredFields`, `FindMissingDomainFields`, `ValidateDomainRecord`,
//! `FindIncompatibleRequiredFields`).
//!
//! Schemas are order-preserving [`Object`]s. There is no regex (P8): a `pattern` is matched
//! by a hand-written matcher chosen by its exact text, and a pattern without one is an
//! error, never a silent pass. Values are compared as Go's `reflect.DeepEqual` compares
//! them, numbers as `f64` (CF1), without recursion per nesting level of user data.

use crate::config::Config;
use crate::error::{AtlasError, Result};
use crate::json::{Object, Value};
use crate::registry::{Registry, TypeDefinition};
use crate::store::Record;

/// One validation failure (`ValidationIssue`).
#[derive(Debug, Clone, Default)]
pub struct ValidationIssue {
    /// The JSON pointer of the failing value (`instancePath`), `""` for the root.
    pub path: String,
    pub message: String,
    pub keyword: String,
    /// Go's `params`; empty where Go's is nil.
    pub params: Object,
}

/// A record that breaks its domain's rules (`DomainViolation`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DomainViolation {
    /// `allowed_types` or `required_fields`.
    pub kind: String,
    pub field: String,
    pub message: String,
}

/// A domain's required field that none of its allowed types declares
/// (`IncompatibleRequiredField`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IncompatibleRequiredField {
    pub field: String,
    pub allowed_types: Vec<String>,
}

/// `ValidateRecord(record, registry)`.
pub fn validate_record(record: &Record, registry: &Registry) -> Result<Vec<ValidationIssue>> {
    // stub until t4
    let _ = (record, registry);
    Err(not_implemented("schema::validate_record"))
}

/// `ValidateRecordValue(record, registry)`: the value must be an object; then it is
/// checked against every type's variant schema in registry order; exactly one must pass.
/// Otherwise the result is the first issue of each failing variant, then the `oneOf`
/// issue. The error case arises only from a `pattern` without a matcher, which no schema
/// [`record_variant_schema`] builds contains.
pub fn validate_record_value(record: &Value, registry: &Registry) -> Result<Vec<ValidationIssue>> {
    // stub until t4
    let _ = (record, registry);
    Err(not_implemented("schema::validate_record_value"))
}

/// `ValidateJSONSchema(value, schema)`: `validateSchemaAt` from the root. A `pattern`
/// with no hand-written matcher is an error.
pub fn validate_json_schema(value: &Value, schema: &Object) -> Result<Vec<ValidationIssue>> {
    // stub until t4
    let _ = (value, schema);
    Err(not_implemented("schema::validate_json_schema"))
}

/// `recordVariantSchema(def)`: the schema of one record type, its properties in Go's
/// remembered order (built-in and custom types differ).
pub fn record_variant_schema(def: &TypeDefinition) -> Object {
    // stub until t4: an empty schema.
    let _ = def;
    Object::new()
}

/// The issues as one line: `<path> <message>` joined with `; ` (root issues begin with a
/// space).
pub fn issues_text(issues: &[ValidationIssue]) -> String {
    // stub until t4
    let _ = issues;
    String::new()
}

/// `GetAllowedTypes(cfg, domain)`: `domains.<domain>.allowed_types` strings.
pub fn get_allowed_types(cfg: &Config, domain: &str) -> Vec<String> {
    // stub until t4
    let _ = (cfg, domain);
    Vec::new()
}

/// `GetRequiredFields(cfg, domain)`: `domains.<domain>.required_fields` strings.
pub fn get_required_fields(cfg: &Config, domain: &str) -> Vec<String> {
    // stub until t4
    let _ = (cfg, domain);
    Vec::new()
}

/// `FindMissingDomainFields(record, fields)`: the fields that are absent, null, `""` or
/// an empty array.
pub fn find_missing_domain_fields(record: &Record, fields: &[String]) -> Vec<String> {
    // stub until t4
    let _ = (record, fields);
    Vec::new()
}

/// `FindIncompatibleRequiredFields(cfg, domain, registry)`.
pub fn find_incompatible_required_fields(
    cfg: &Config,
    domain: &str,
    registry: &Registry,
) -> Vec<IncompatibleRequiredField> {
    // stub until t4
    let _ = (cfg, domain, registry);
    Vec::new()
}

/// `ValidateDomainRecord(record, domain, cfg)`.
pub fn validate_domain_record(record: &Record, domain: &str, cfg: &Config) -> Vec<DomainViolation> {
    // stub until t4
    let _ = (record, domain, cfg);
    Vec::new()
}

/// The hand-written matcher for a schema `pattern`, chosen by its exact text, or `None`
/// when there is none (P8).
#[cfg_attr(not(test), allow(dead_code))] // called by the validator from t4 on
fn pattern_matcher(pattern: &str) -> Option<fn(&str) -> bool> {
    // stub until t4
    let _ = pattern;
    None
}

fn not_implemented(name: &str) -> AtlasError {
    AtlasError::Message(format!("not implemented: {name}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::json;
    use crate::yaml;

    fn builtin() -> Registry {
        Registry::from_config(None).expect("built-in registry")
    }

    fn fixture_registry() -> Registry {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/s5-atlas-lib-2/custom-types.yaml"
        );
        let text = std::fs::read_to_string(path).expect("read custom-types.yaml");
        let parsed = yaml::parse(&text).expect("parse custom-types.yaml");
        let Value::Object(config) = &parsed else {
            panic!("custom-types.yaml is not a mapping");
        };
        Registry::from_config(Some(config)).expect("registry from custom-types.yaml")
    }

    fn record(text: &str) -> Record {
        match &mut json::parse(text).expect("record json") {
            Value::Object(object) => std::mem::take(object),
            _ => panic!("not an object"),
        }
    }

    fn lines(issues: &[ValidationIssue]) -> Vec<String> {
        issues
            .iter()
            .map(|i| format!("{} {}", i.path, i.message))
            .collect()
    }

    #[test]
    fn valid_record_has_no_issues() {
        let r = record(
            r#"{"id":"mx-abc123","type":"convention","content":"Use the lock","classification":"tactical","recorded_at":"2024-01-01T00:00:00.000Z","tags":["a"],"relates_to":["proj-1:mx-1234"],"evidence":{"commit":"abc","seeds":"s-1"},"outcomes":[{"status":"success","duration":1.5,"notes":"n"}],"status":"active"}"#,
        );
        let issues = validate_record(&r, &builtin()).expect("validate");
        assert!(issues.is_empty(), "{:?}", lines(&issues));
        let registry = fixture_registry();
        let r = record(
            r#"{"type":"runbook","title":"Restart","steps":"1. stop","classification":"tactical","recorded_at":"now"}"#,
        );
        let issues = validate_record(&r, &registry).expect("validate");
        assert!(issues.is_empty(), "{:?}", lines(&issues));
        assert_eq!(issues_text(&issues), "");
    }

    #[test]
    fn extra_field_is_rejected() {
        let r = record(
            r#"{"type":"convention","content":"x","classification":"tactical","recorded_at":"now","bogus":1}"#,
        );
        let issues = validate_record(&r, &builtin()).expect("validate");
        assert_eq!(
            lines(&issues),
            [
                " must NOT have additional properties",
                " must have required property 'name'",
                " must have required property 'description'",
                " must have required property 'title'",
                " must have required property 'name'",
                " must have required property 'name'",
                " must match exactly one schema in oneOf",
            ]
        );
        assert_eq!(issues[0].keyword, "additionalProperties");
        assert_eq!(
            issues[0]
                .params
                .get("additionalProperty")
                .and_then(Value::as_str),
            Some("bogus")
        );
        assert_eq!(issues[6].keyword, "oneOf");
        assert_eq!(
            issues_text(&issues[..2]),
            " must NOT have additional properties;  must have required property 'name'"
        );
        // A nested extra field carries its escaped path.
        let r = record(
            r#"{"type":"convention","content":"x","classification":"tactical","recorded_at":"now","evidence":{"a/b~c":"x"}}"#,
        );
        let issues = validate_record(&r, &builtin()).expect("validate");
        assert_eq!(issues[0].path, "/evidence");
        assert_eq!(issues[0].message, "must NOT have additional properties");
    }

    #[test]
    fn ajv_one_of_failure_contract() {
        // Go TestNativeAJVOneOfFailureContract (registry_test.go:68-81).
        let r = record(
            r#"{"type":"pattern","name":"p","description":17,"classification":"tactical","recorded_at":"now"}"#,
        );
        let issues = validate_record(&r, &builtin()).expect("validate");
        assert_eq!(
            lines(&issues),
            [
                " must have required property 'content'",
                "/description must be string",
                " must have required property 'resolution'",
                " must have required property 'title'",
                "/type must be equal to constant",
                "/type must be equal to constant",
                " must match exactly one schema in oneOf",
            ]
        );
    }

    #[test]
    fn array_row_is_one_type_issue() {
        let value = json::parse(r#"[{"type":"convention"}]"#).expect("json");
        let issues = validate_record_value(&value, &builtin()).expect("validate");
        assert_eq!(lines(&issues), [" must be object"]);
        assert_eq!(issues[0].keyword, "type");
        assert_eq!(
            issues[0].params.get("type").and_then(Value::as_str),
            Some("object")
        );
        let issues = validate_record_value(&Value::Null, &builtin()).expect("validate");
        assert_eq!(lines(&issues), [" must be object"]);
    }

    /// Every `pattern` string in `schema`, walked with an explicit stack.
    fn patterns(schema: &Object) -> Vec<String> {
        let mut out = Vec::new();
        let mut stack: Vec<&Object> = vec![schema];
        while let Some(object) = stack.pop() {
            for (key, value) in object.iter() {
                if key == "pattern" {
                    if let Some(p) = value.as_str() {
                        out.push(p.to_owned());
                    }
                }
                match value {
                    Value::Object(inner) => stack.push(inner),
                    Value::Array(items) => {
                        stack.extend(items.iter().filter_map(Value::as_object));
                    }
                    _ => {}
                }
            }
        }
        out
    }

    #[test]
    fn every_built_schema_pattern_has_a_matcher() {
        let mut seen = Vec::new();
        for registry in [builtin(), fixture_registry()] {
            for name in registry.names() {
                let def = registry.get(name).expect("type");
                for pattern in patterns(&record_variant_schema(def)) {
                    assert!(
                        pattern_matcher(&pattern).is_some(),
                        "no matcher for {pattern:?} in {name}"
                    );
                    if !seen.contains(&pattern) {
                        seen.push(pattern);
                    }
                }
            }
        }
        seen.sort();
        assert_eq!(
            seen,
            ["^([a-z0-9-]+:)?mx-[0-9a-f]{4,8}$", "^mx-[0-9a-f]{4,8}$"]
        );
        let id = pattern_matcher("^mx-[0-9a-f]{4,8}$").expect("id matcher");
        for yes in ["mx-abcd", "mx-12345678", "mx-0f0f0"] {
            assert!(id(yes), "{yes}");
        }
        for no in [
            "mx-abc",
            "mx-123456789",
            "mx-ABCD",
            "x-abcd",
            "mx-abcd\n",
            " mx-abcd",
            "p:mx-abcd",
        ] {
            assert!(!id(no), "{no:?}");
        }
        let link = pattern_matcher("^([a-z0-9-]+:)?mx-[0-9a-f]{4,8}$").expect("link matcher");
        for yes in ["mx-abcd", "proj-1:mx-1234abcd", "a:mx-0000", "-:mx-0000"] {
            assert!(link(yes), "{yes}");
        }
        for no in [
            "Proj:mx-abcd",
            ":mx-abcd",
            "a:b:mx-abcd",
            "a_b:mx-abcd",
            "mx-abcd\n",
        ] {
            assert!(!link(no), "{no:?}");
        }
        // A pattern without a matcher is an error, never a silent pass.
        let mut schema = Object::new();
        schema.insert("type", "string");
        schema.insert("pattern", "^[a-z]+$");
        assert!(pattern_matcher("^[a-z]+$").is_none());
        assert!(validate_json_schema(&Value::from("abc"), &schema).is_err());
    }

    #[test]
    fn enum_compares_integer_and_float_as_go() {
        let mut schema = Object::new();
        schema.insert("type", "number");
        schema.insert(
            "enum",
            Value::Array(vec![Value::Integer(100), Value::from("a")]),
        );
        let ok = validate_json_schema(&Value::Number(100.0), &schema).expect("validate");
        assert!(ok.is_empty(), "{:?}", lines(&ok));
        let bad = validate_json_schema(&Value::Number(100.5), &schema).expect("validate");
        assert_eq!(lines(&bad), [" must be equal to one of the allowed values"]);

        let mut schema = Object::new();
        schema.insert("type", "integer");
        schema.insert("const", Value::Integer(2));
        let ok = validate_json_schema(&Value::Number(2.0), &schema).expect("validate");
        assert!(ok.is_empty(), "{:?}", lines(&ok));

        let mut schema = Object::new();
        schema.insert("type", "array");
        schema.insert("uniqueItems", true);
        let items = Value::Array(vec![
            Value::Number(1.0),
            Value::Integer(1),
            Value::from("1"),
        ]);
        let issues = validate_json_schema(&items, &schema).expect("validate");
        assert_eq!(
            lines(&issues),
            [" must NOT have duplicate items (items ## 0 and 1 are identical)"]
        );
        // Objects compare by content, not key order; arrays by order.
        let a = json::parse(r#"[{"x":1,"y":[1,2]},{"y":[1,2],"x":1.0}]"#).expect("json");
        let issues = validate_json_schema(&a, &schema).expect("validate");
        assert_eq!(issues.len(), 1);
        let b = json::parse(r#"[{"y":[1,2]},{"y":[2,1]}]"#).expect("json");
        assert!(validate_json_schema(&b, &schema)
            .expect("validate")
            .is_empty());
    }

    #[test]
    #[ignore = "hostile input: run under prlimit (PIPELINE rule 10)"]
    fn hostile_deep_value_on_small_stack() {
        let handle = std::thread::Builder::new()
            .stack_size(2 * 1024 * 1024)
            .spawn(|| {
                let mut deep = Value::from("leaf");
                for _ in 0..9990 {
                    deep = Value::Array(vec![deep]);
                }
                let mut r = record(
                    r#"{"type":"convention","content":"x","classification":"tactical","recorded_at":"now"}"#,
                );
                r.insert("tags", deep);
                let issues = validate_record(&r, &builtin()).expect("validate");
                lines(&issues)
            })
            .expect("spawn");
        let issues = handle.join().expect("no stack overflow");
        assert_eq!(issues[0], "/tags/0 must be string");
        assert_eq!(
            issues.last().map(String::as_str),
            Some(" must match exactly one schema in oneOf")
        );
    }
}
