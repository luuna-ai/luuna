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
//!
//! The walker ([`validate_schema_at`]) never recurses: it keeps an explicit stack of tasks
//! (check a value against a schema, emit an issue, take the next `oneOf` / `anyOf` option)
//! and a stack of issue buffers for the options being tried, so the issues come out in the
//! order of Go's recursive `validateSchemaAt`. A schema that keeps referring to itself
//! (through `$ref`, `oneOf` or `anyOf`) without checking the value makes Go recurse until
//! its stack overflows; here it is an error after [`MAX_SCHEMA_STEPS`] such steps. No
//! schema this crate builds or reads has `$ref`, `oneOf` or `anyOf`.

use crate::commands::go_quote;
use crate::config::Config;
use crate::error::{AtlasError, Result};
use crate::evidence::LEGACY_EVIDENCE_KEY;
use crate::json::{Object, Value};
use crate::records::{any_list, contains, go_deep_equal, list, number, obj};
use crate::registry::{go_bool, go_text, Registry, TypeDefinition};
use crate::store::Record;

/// `baseRecordFields` (schema.go:13).
const BASE_RECORD_FIELDS: [&str; 14] = [
    "id",
    "type",
    "classification",
    "recorded_at",
    "evidence",
    "tags",
    "relates_to",
    "supersedes",
    "outcomes",
    "dir_anchors",
    "supersession_demoted_at",
    "anchor_decay_demoted_at",
    "owner",
    "status",
];

/// The record id pattern (schema.go:55).
const ID_PATTERN: &str = "^mx-[0-9a-f]{4,8}$";
/// The `relates_to` / `supersedes` link pattern (schema.go:54).
const LINK_PATTERN: &str = "^([a-z0-9-]+:)?mx-[0-9a-f]{4,8}$";

/// How many `$ref`, `oneOf` and `anyOf` steps in a row may apply to one value before the
/// schema counts as referring to itself. Go has no limit and overflows its stack.
const MAX_SCHEMA_STEPS: usize = 1000;

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
    // RecordValue(record): a record read from an object row is that object. (The store
    // reads an array row as an empty record; a caller holding the row's raw value uses
    // validate_record_value, as Go's RecordValue would.)
    validate_record_value(&Value::Object(record.clone()), registry)
}

/// `ValidateRecordValue(record, registry)`: the value must be an object; then it is
/// checked against every type's variant schema in registry order; exactly one must pass.
/// Otherwise the result is the first issue of each failing variant, then the `oneOf`
/// issue. The error case arises only from a `pattern` without a matcher, which no schema
/// [`record_variant_schema`] builds contains.
pub fn validate_record_value(record: &Value, registry: &Registry) -> Result<Vec<ValidationIssue>> {
    let mut object_schema = Object::new();
    object_schema.insert_unordered("type", "object");
    // Go passes a nil root here; Obj(nil) is an empty object.
    let issues = validate_schema_at(record, &object_schema, &Object::new())?;
    if !issues.is_empty() {
        return Ok(issues);
    }
    let mut issues = Vec::new();
    let mut valid = 0usize;
    for name in registry.names() {
        let Some(def) = registry.get(name) else {
            continue;
        };
        let schema = record_variant_schema(def);
        let errors = validate_schema_at(record, &schema, &schema)?;
        match errors.into_iter().next() {
            None => valid += 1,
            Some(first) => issues.push(first),
        }
    }
    if valid == 1 {
        return Ok(Vec::new());
    }
    issues.push(issue(
        "",
        "oneOf",
        "must match exactly one schema in oneOf".to_owned(),
        params([("passingSchemas", Value::Null)]),
    ));
    Ok(issues)
}

/// `ValidateJSONSchema(value, schema)`: `validateSchemaAt` from the root. A `pattern`
/// with no hand-written matcher is an error.
pub fn validate_json_schema(value: &Value, schema: &Object) -> Result<Vec<ValidationIssue>> {
    validate_schema_at(value, schema, schema)
}

/// `recordVariantSchema(def)`: the schema of one record type, its properties in Go's
/// remembered order (built-in and custom types differ).
///
/// Maps Go builds without `rememberOrder` hold their keys in byte order, as Go's
/// `ObjectKeys` lists them.
pub fn record_variant_schema(def: &TypeDefinition) -> Object {
    let mut ev_props = Object::new();
    for key in [
        "commit",
        "date",
        "issue",
        "file",
        "bead",
        "missions",
        LEGACY_EVIDENCE_KEY,
        "gh",
        "linear",
    ] {
        // `insert` remembers each new key in order, as rememberOrder(evProps, evKeys).
        ev_props.insert(key, str_schema());
    }
    let mut outcome_props = Object::new();
    outcome_props.insert("status", enum_schema(&["success", "failure", "partial"]));
    outcome_props.insert("duration", type_schema("number"));
    outcome_props.insert("test_results", str_schema());
    outcome_props.insert("agent", str_schema());
    outcome_props.insert("notes", str_schema());
    outcome_props.insert("recorded_at", str_schema());

    let mut link = type_schema("string");
    link.insert_unordered("pattern", LINK_PATTERN);
    let mut id = type_schema("string");
    id.insert_unordered("pattern", ID_PATTERN);
    let mut evidence = type_schema("object");
    evidence.insert_unordered("properties", ev_props);
    evidence.insert_unordered("additionalProperties", false);
    let mut outcome = type_schema("object");
    outcome.insert_unordered("properties", outcome_props);
    outcome.insert_unordered("required", strings(&["status"]));
    outcome.insert_unordered("additionalProperties", false);
    let mut type_const = type_schema("string");
    type_const.insert_unordered("const", def.name.as_str());

    let mut props = Object::new();
    props.insert_unordered("id", id);
    props.insert_unordered(
        "classification",
        enum_schema(&["foundational", "tactical", "observational"]),
    );
    props.insert_unordered("recorded_at", str_schema());
    props.insert_unordered("evidence", evidence);
    props.insert_unordered("tags", arr_schema(str_schema()));
    props.insert_unordered("relates_to", arr_schema(link.clone()));
    props.insert_unordered("supersedes", arr_schema(link));
    props.insert_unordered("outcomes", arr_schema(outcome));
    props.insert_unordered("dir_anchors", arr_schema(str_schema()));
    props.insert_unordered("supersession_demoted_at", str_schema());
    props.insert_unordered("anchor_decay_demoted_at", str_schema());
    props.insert_unordered("owner", str_schema());
    props.insert_unordered("status", enum_schema(&["draft", "active", "deprecated"]));
    props.insert_unordered("type", type_const);

    let base_order: [&str; 14] = if def.builtin {
        [
            "id",
            "classification",
            "recorded_at",
            "evidence",
            "tags",
            "relates_to",
            "supersedes",
            "outcomes",
            "dir_anchors",
            "supersession_demoted_at",
            "anchor_decay_demoted_at",
            "owner",
            "status",
            "type",
        ]
    } else {
        [
            "id",
            "type",
            "classification",
            "recorded_at",
            "evidence",
            "tags",
            "relates_to",
            "supersedes",
            "outcomes",
            "dir_anchors",
            "supersession_demoted_at",
            "anchor_decay_demoted_at",
            "owner",
            "status",
        ]
    };
    let mut order: Vec<String> = base_order.iter().map(|s| (*s).to_owned()).collect();
    for field in def.required.iter().chain(def.optional.iter()) {
        // A field of the type replaces a base property of the same name, as Go's
        // props[field] assignment does.
        let schema = if def.extracts_files && *field == def.files_field {
            arr_schema(str_schema())
        } else {
            str_schema()
        };
        props.insert_unordered(field.as_str(), schema);
        if !contains(&order, field) {
            order.push(field.clone());
        }
    }
    props.remember_order(&order);

    let mut required = vec![Value::from("type")];
    required.extend(def.required.iter().map(|f| Value::from(f.as_str())));
    required.push(Value::from("classification"));
    required.push(Value::from("recorded_at"));

    let mut schema = type_schema("object");
    schema.insert_unordered("properties", props);
    schema.insert_unordered("required", Value::Array(required));
    schema.insert_unordered("additionalProperties", false);
    schema
}

/// `{"type": kind}`.
fn type_schema(kind: &str) -> Object {
    let mut schema = Object::new();
    schema.insert_unordered("type", kind);
    schema
}

/// `str()`: `{"type": "string"}`.
fn str_schema() -> Object {
    type_schema("string")
}

/// `enum(values...)`: `{"type": "string", "enum": values}`.
fn enum_schema(values: &[&str]) -> Object {
    let mut schema = type_schema("string");
    schema.insert_unordered("enum", strings(values));
    schema
}

/// `arr(item)`: `{"type": "array", "items": item}`.
fn arr_schema(item: Object) -> Object {
    let mut schema = type_schema("array");
    schema.insert_unordered("items", item);
    schema
}

/// A JSON array of strings.
fn strings(values: &[&str]) -> Value {
    Value::Array(values.iter().map(|v| Value::from(*v)).collect())
}

/// The issues as one line: `<path> <message>` joined with `; ` (root issues begin with a
/// space).
pub fn issues_text(issues: &[ValidationIssue]) -> String {
    issues
        .iter()
        .map(|issue| format!("{} {}", issue.path, issue.message))
        .collect::<Vec<_>>()
        .join("; ")
}

/// `GetAllowedTypes(cfg, domain)`: `domains.<domain>.allowed_types` strings.
pub fn get_allowed_types(cfg: &Config, domain: &str) -> Vec<String> {
    list(obj(obj(cfg.get("domains")).get(domain)).get("allowed_types"))
}

/// `GetRequiredFields(cfg, domain)`: `domains.<domain>.required_fields` strings.
pub fn get_required_fields(cfg: &Config, domain: &str) -> Vec<String> {
    list(obj(obj(cfg.get("domains")).get(domain)).get("required_fields"))
}

/// `FindMissingDomainFields(record, fields)`: the fields that are absent, null, `""` or
/// an empty array.
pub fn find_missing_domain_fields(record: &Record, fields: &[String]) -> Vec<String> {
    fields
        .iter()
        .filter(|field| match record.get(field) {
            None | Some(Value::Null) => true,
            Some(Value::String(s)) => s.is_empty(),
            Some(Value::Array(items)) => items.is_empty(),
            Some(_) => false,
        })
        .cloned()
        .collect()
}

/// `FindIncompatibleRequiredFields(cfg, domain, registry)`.
pub fn find_incompatible_required_fields(
    cfg: &Config,
    domain: &str,
    registry: &Registry,
) -> Vec<IncompatibleRequiredField> {
    let mut allowed = get_allowed_types(cfg, domain);
    if allowed.is_empty() {
        allowed = registry.names().into_iter().map(str::to_owned).collect();
    }
    let mut out = Vec::new();
    for field in get_required_fields(cfg, domain) {
        if BASE_RECORD_FIELDS.contains(&field.as_str()) && field != "anchor_decay_demoted_at" {
            continue;
        }
        let accepted = allowed.iter().any(|type_name| {
            registry.get(type_name).is_some_and(|def| {
                contains(&def.required, &field) || contains(&def.optional, &field)
            })
        });
        if !accepted {
            out.push(IncompatibleRequiredField {
                field,
                allowed_types: allowed.clone(),
            });
        }
    }
    out
}

/// `ValidateDomainRecord(record, domain, cfg)`.
pub fn validate_domain_record(record: &Record, domain: &str, cfg: &Config) -> Vec<DomainViolation> {
    let mut out = Vec::new();
    let allowed = get_allowed_types(cfg, domain);
    let type_text = text(record.get("type"));
    if !allowed.is_empty() && !contains(&allowed, &type_text) {
        out.push(DomainViolation {
            kind: "allowed_types".to_owned(),
            field: "type".to_owned(),
            message: format!(
                "type {} is not allowed in domain {} (allowed: {})",
                go_quote(&type_text),
                go_quote(domain),
                allowed.join(", ")
            ),
        });
    }
    for field in find_missing_domain_fields(record, &get_required_fields(cfg, domain)) {
        out.push(DomainViolation {
            kind: "required_fields".to_owned(),
            message: format!(
                "missing required field {} in domain {}",
                go_quote(&field),
                go_quote(domain)
            ),
            field,
        });
    }
    out
}

/// Go's `Text(v)` (registry's `go_text`) without recursion: an array's elements are
/// joined with `,`, nested arrays the same way, walked with an explicit stack (C6:
/// `go_text` recurses once per nested array).
fn text(value: Option<&Value>) -> String {
    let Some(Value::Array(items)) = value else {
        return go_text(value);
    };
    let mut out = String::new();
    // Each level: the elements still to write and whether one was written already.
    let mut stack: Vec<(std::slice::Iter<'_, Value>, bool)> = vec![(items.iter(), false)];
    while let Some((rest, started)) = stack.last_mut() {
        let Some(item) = rest.next() else {
            stack.pop();
            continue;
        };
        if *started {
            out.push(',');
        }
        *started = true;
        match item {
            Value::Array(inner) => stack.push((inner.iter(), false)),
            scalar => out.push_str(&go_text(Some(scalar))),
        }
    }
    out
}

/// `isNumeric(v)`: a finite JSON number or any integer.
fn is_numeric(value: &Value) -> bool {
    match value {
        Value::Number(n) => n.is_finite(),
        Value::Integer(_) => true,
        _ => false,
    }
}

/// Whether `value` has the JSON type `kind` as Go's `validateSchemaAt` checks it; any other
/// `kind` (none, or a name Go does not know) accepts every value.
fn type_matches(kind: &str, value: &Value) -> bool {
    match kind {
        "object" => matches!(value, Value::Object(_)),
        "array" => matches!(value, Value::Array(_)),
        "string" => matches!(value, Value::String(_)),
        "number" => is_numeric(value),
        "integer" => {
            // Go's Number gives 0 for a uint64 (CF6), which counts as whole.
            let n = number(Some(value));
            is_numeric(value) && n == n.trunc()
        }
        "boolean" => matches!(value, Value::Bool(_)),
        "null" => matches!(value, Value::Null),
        _ => true,
    }
}

/// A JSON pointer token: `~` as `~0`, then `/` as `~1`.
fn escape_pointer(name: &str) -> String {
    name.replace('~', "~0").replace('/', "~1")
}

/// Go's `int(f)` for the limits: truncated toward zero. (Go leaves the conversion of an
/// out-of-range float undefined; Rust saturates.)
fn go_int(f: f64) -> i64 {
    f as i64
}

/// The hand-written matcher for a schema `pattern`, chosen by its exact text, or `None`
/// when there is none (P8).
fn pattern_matcher(pattern: &str) -> Option<fn(&str) -> bool> {
    match pattern {
        ID_PATTERN => Some(matches_id),
        LINK_PATTERN => Some(matches_link),
        _ => None,
    }
}

/// `^mx-[0-9a-f]{4,8}$`. Go's `$` without the `m` flag matches only at the end of the
/// text, so a trailing newline does not match.
fn matches_id(s: &str) -> bool {
    s.strip_prefix("mx-").is_some_and(|hex| {
        (4..=8).contains(&hex.len())
            && hex
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}

/// `^([a-z0-9-]+:)?mx-[0-9a-f]{4,8}$`. Neither the prefix class nor the id contains `:`,
/// so the group, when present, ends at the first `:`.
fn matches_link(s: &str) -> bool {
    match s.split_once(':') {
        None => matches_id(s),
        Some((prefix, rest)) => {
            !prefix.is_empty()
                && prefix
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
                && matches_id(rest)
        }
    }
}

/// One step of the walk.
enum Task<'a> {
    /// `validateSchemaAt(value, schema, root, path)`.
    Check(Check<'a>),
    /// Append an issue to the innermost buffer.
    Emit(ValidationIssue),
    /// Take the result of the option just checked and start the next one.
    Options(Options<'a>),
}

/// A value to check against a schema.
struct Check<'a> {
    value: &'a Value,
    schema: &'a Object,
    path: String,
    /// `$ref`, `oneOf` and `anyOf` steps in a row on this value.
    steps: usize,
}

/// A `oneOf` or `anyOf` whose options are checked one by one, each into its own buffer.
struct Options<'a> {
    one_of: bool,
    value: &'a Value,
    options: &'a [Value],
    next: usize,
    valid: usize,
    /// The issues of the options that failed, in option order.
    failed: Vec<ValidationIssue>,
    path: String,
    steps: usize,
}

/// `validateSchemaAt(value, schema, root, "")`, walked with an explicit stack.
fn validate_schema_at(
    value: &Value,
    schema: &Object,
    root: &Object,
) -> Result<Vec<ValidationIssue>> {
    let mut tasks: Vec<Task<'_>> = vec![Task::Check(Check {
        value,
        schema,
        path: String::new(),
        steps: 0,
    })];
    // The outermost buffer is the result; each option being checked has one more.
    let mut buffers: Vec<Vec<ValidationIssue>> = vec![Vec::new()];
    while let Some(task) = tasks.pop() {
        match task {
            Task::Emit(issue) => {
                if let Some(buffer) = buffers.last_mut() {
                    buffer.push(issue);
                }
            }
            Task::Check(check) => check_one(check, root, &mut tasks)?,
            Task::Options(options) => next_option(options, &mut tasks, &mut buffers),
        }
    }
    Ok(buffers.pop().unwrap_or_default())
}

/// A [`ValidationIssue`] at `path`.
fn issue(path: &str, keyword: &str, message: String, params: Object) -> ValidationIssue {
    ValidationIssue {
        path: path.to_owned(),
        message,
        keyword: keyword.to_owned(),
        params,
    }
}

/// Params with the given entries, in Go's map order (keys sorted).
fn params<const N: usize>(entries: [(&str, Value); N]) -> Object {
    let mut object = Object::new();
    for (key, value) in entries {
        object.insert_unordered(key, value);
    }
    object
}

/// The error for a schema that refers to itself without checking the value.
fn too_many_steps(path: &str) -> AtlasError {
    AtlasError::Message(format!(
        "schema validation at {}: more than {MAX_SCHEMA_STEPS} $ref, oneOf or anyOf steps in a row on one value; the schema refers to itself",
        go_quote(path)
    ))
}

/// One `validateSchemaAt` call. The issues it adds itself and the checks it would recurse
/// into are pushed as tasks, in reverse, so they run in Go's order.
fn check_one<'a>(check: Check<'a>, root: &'a Object, tasks: &mut Vec<Task<'a>>) -> Result<()> {
    let Check {
        value,
        schema,
        path,
        steps,
    } = check;

    let reference = text(schema.get("$ref"));
    if let Some(pointer) = reference.strip_prefix("#/") {
        if steps >= MAX_SCHEMA_STEPS {
            return Err(too_many_steps(&path));
        }
        let mut target = root;
        for part in pointer.split('/') {
            target = obj(target.get(&part.replace("~1", "/").replace("~0", "~")));
        }
        tasks.push(Task::Check(Check {
            value,
            schema: target,
            path,
            steps: steps + 1,
        }));
        return Ok(());
    }
    for (keyword, one_of) in [("oneOf", true), ("anyOf", false)] {
        let options = any_list(schema.get(keyword));
        if !options.is_empty() {
            if steps >= MAX_SCHEMA_STEPS {
                return Err(too_many_steps(&path));
            }
            tasks.push(Task::Options(Options {
                one_of,
                value,
                options,
                next: 0,
                valid: 0,
                failed: Vec::new(),
                path,
                steps,
            }));
            return Ok(());
        }
    }

    let kind = text(schema.get("type"));
    if !type_matches(&kind, value) {
        tasks.push(Task::Emit(issue(
            &path,
            "type",
            format!("must be {kind}"),
            params([("type", Value::from(kind.as_str()))]),
        )));
        return Ok(());
    }
    // What this call does, in Go's order: issues it adds and checks it recurses into.
    let mut seq: Vec<Task<'a>> = Vec::new();
    let choices = any_list(schema.get("enum"));
    if !choices.is_empty() && !choices.iter().any(|choice| go_deep_equal(choice, value)) {
        seq.push(Task::Emit(issue(
            &path,
            "enum",
            "must be equal to one of the allowed values".to_owned(),
            params([("allowedValues", Value::Array(choices.to_vec()))]),
        )));
    }
    if let Some(constant) = schema.get("const") {
        if !go_deep_equal(constant, value) {
            seq.push(Task::Emit(issue(
                &path,
                "const",
                "must be equal to constant".to_owned(),
                params([("allowedValue", constant.clone())]),
            )));
        }
    }
    if kind == "object" {
        let object = obj(Some(value));
        for required in list(schema.get("required")) {
            if !object.contains_key(&required) {
                seq.push(Task::Emit(issue(
                    &path,
                    "required",
                    format!("must have required property '{required}'"),
                    params([("missingProperty", Value::String(required))]),
                )));
            }
        }
        let properties = obj(schema.get("properties"));
        // Object iteration is Go's ObjectKeys order.
        for (name, item) in object.iter() {
            if properties.contains_key(name) {
                continue;
            }
            match schema.get("additionalProperties") {
                Some(Value::Bool(false)) => seq.push(Task::Emit(issue(
                    &path,
                    "additionalProperties",
                    "must NOT have additional properties".to_owned(),
                    params([("additionalProperty", Value::from(name))]),
                ))),
                Some(Value::Object(additional)) => seq.push(Task::Check(Check {
                    value: item,
                    schema: additional,
                    path: format!("{path}/{}", escape_pointer(name)),
                    steps: 0,
                })),
                _ => {}
            }
        }
        for (name, property) in properties.iter() {
            if let Some(item) = object.get(name) {
                seq.push(Task::Check(Check {
                    value: item,
                    schema: obj(Some(property)),
                    path: format!("{path}/{}", escape_pointer(name)),
                    steps: 0,
                }));
            }
        }
    }
    if kind == "array" {
        let items = any_list(Some(value));
        if let Some(min) = schema.get("minItems") {
            if (items.len() as i64) < go_int(number(Some(min))) {
                seq.push(Task::Emit(issue(
                    &path,
                    "minItems",
                    format!("must NOT have fewer than {} items", text(Some(min))),
                    params([("limit", min.clone())]),
                )));
            }
        }
        if go_bool(schema.get("uniqueItems")) {
            for i in 0..items.len() {
                for j in i + 1..items.len() {
                    if go_deep_equal(&items[i], &items[j]) {
                        seq.push(Task::Emit(issue(
                            &path,
                            "uniqueItems",
                            format!(
                                "must NOT have duplicate items (items ## {i} and {j} are identical)"
                            ),
                            Object::new(),
                        )));
                    }
                }
            }
        }
        if let Some(Value::Object(item_schema)) = schema.get("items") {
            for (i, item) in items.iter().enumerate() {
                seq.push(Task::Check(Check {
                    value: item,
                    schema: item_schema,
                    path: format!("{path}/{i}"),
                    steps: 0,
                }));
            }
        }
    }
    if kind == "string" {
        let s = value.as_str().unwrap_or_default();
        let pattern = text(schema.get("pattern"));
        if !pattern.is_empty() {
            // Go compiles the pattern with regexp; here a pattern without a matcher is an
            // error, never a silent pass (P8).
            let Some(matcher) = pattern_matcher(&pattern) else {
                return Err(AtlasError::Message(format!(
                    "schema pattern {} has no matcher in this port",
                    go_quote(&pattern)
                )));
            };
            if !matcher(s) {
                seq.push(Task::Emit(issue(
                    &path,
                    "pattern",
                    format!("must match pattern {}", go_quote(&pattern)),
                    params([("pattern", Value::String(pattern))]),
                )));
            }
        }
        if let Some(min) = schema.get("minLength") {
            if (s.chars().count() as i64) < go_int(number(Some(min))) {
                seq.push(Task::Emit(issue(
                    &path,
                    "minLength",
                    format!("must NOT have fewer than {} characters", text(Some(min))),
                    params([("limit", min.clone())]),
                )));
            }
        }
    }
    if kind == "number" || kind == "integer" {
        let v = number(Some(value));
        if let Some(min) = schema.get("minimum") {
            if v < number(Some(min)) {
                seq.push(Task::Emit(issue(
                    &path,
                    "minimum",
                    format!("must be >= {}", text(Some(min))),
                    params([("comparison", Value::from(">=")), ("limit", min.clone())]),
                )));
            }
        }
        if let Some(max) = schema.get("maximum") {
            if v > number(Some(max)) {
                seq.push(Task::Emit(issue(
                    &path,
                    "maximum",
                    format!("must be <= {}", text(Some(max))),
                    params([("comparison", Value::from("<=")), ("limit", max.clone())]),
                )));
            }
        }
    }
    tasks.extend(seq.into_iter().rev());
    Ok(())
}

/// Takes the result of the option just checked (the innermost buffer), then checks the
/// next option into a fresh buffer, or, when the options are done, adds the `oneOf` /
/// `anyOf` result to the enclosing buffer. Go stops `anyOf` at the first valid option.
fn next_option<'a>(
    mut options: Options<'a>,
    tasks: &mut Vec<Task<'a>>,
    buffers: &mut Vec<Vec<ValidationIssue>>,
) {
    if options.next > 0 {
        let errors = buffers.pop().unwrap_or_default();
        if errors.is_empty() {
            options.valid += 1;
            if !options.one_of {
                return;
            }
        } else {
            options.failed.extend(errors);
        }
    }
    if let Some(option) = options.options.get(options.next) {
        options.next += 1;
        let check = Check {
            value: options.value,
            schema: obj(Some(option)),
            path: options.path.clone(),
            steps: options.steps + 1,
        };
        tasks.push(Task::Options(options));
        buffers.push(Vec::new());
        tasks.push(Task::Check(check));
        return;
    }
    let failed = if options.one_of {
        options.valid != 1
    } else {
        options.valid == 0
    };
    if !failed {
        return;
    }
    let (keyword, message) = if options.one_of {
        ("oneOf", "must match exactly one schema in oneOf")
    } else {
        ("anyOf", "must match a schema in anyOf")
    };
    if let Some(buffer) = buffers.last_mut() {
        buffer.append(&mut options.failed);
        buffer.push(issue(
            &options.path,
            keyword,
            message.to_owned(),
            Object::new(),
        ));
    }
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

    fn object(text: &str) -> Object {
        record(text)
    }

    #[test]
    fn json_schema_primitives_and_keyword_order() {
        // Go TestNativeJSONSchemaConfigPrimitives (registry_test.go:57-66), with the texts
        // schema.go:251 and :300 give.
        let schema = object(
            r#"{"type":"object","required":["count"],"properties":{"count":{"type":"integer","minimum":1}},"additionalProperties":false}"#,
        );
        let ok = validate_json_schema(&json::parse(r#"{"count":2}"#).expect("json"), &schema)
            .expect("validate");
        assert!(ok.is_empty(), "{:?}", lines(&ok));
        let value = json::parse(r#"{"count":0,"other":true}"#).expect("json");
        let issues = validate_json_schema(&value, &schema).expect("validate");
        assert_eq!(
            lines(&issues),
            [
                " must NOT have additional properties",
                "/count must be >= 1"
            ]
        );
        assert_eq!(issues[1].keyword, "minimum");
        assert_eq!(
            issues[1].params.keys().collect::<Vec<_>>(),
            ["comparison", "limit"]
        );

        // oneOf collects every failing option's issues, then its own; anyOf likewise;
        // $ref resolves from the root with ~1 then ~0 unescaped; paths escape ~ then /.
        let schema = object(
            r##"{"definitions":{"s/~":{"type":"string","minLength":2}},"type":"object","properties":{"a":{"oneOf":[{"$ref":"#/definitions/s~1~0"},{"type":"number"}]},"b":{"anyOf":[{"type":"string"},{"type":"null"}]},"c~/d":{"type":"array","minItems":2,"items":{"type":"string"},"maximum":0}}}"##,
        );
        let value = json::parse(r#"{"c~/d":[1],"b":3,"a":"x"}"#).expect("json");
        let issues = validate_json_schema(&value, &schema).expect("validate");
        assert_eq!(
            lines(&issues),
            [
                "/a must NOT have fewer than 2 characters",
                "/a must be number",
                "/a must match exactly one schema in oneOf",
                "/b must be string",
                "/b must be null",
                "/b must match a schema in anyOf",
                "/c~0~1d must NOT have fewer than 2 items",
                "/c~0~1d/0 must be string",
            ]
        );
        // anyOf stops at the first valid option; oneOf with two valid options fails alone.
        let value = json::parse(r#"{"a":"xy","b":"s","c~/d":["p","q"]}"#).expect("json");
        assert!(validate_json_schema(&value, &schema)
            .expect("validate")
            .is_empty());
        let both = object(r#"{"oneOf":[{"type":"string"},{}]}"#);
        let issues = validate_json_schema(&Value::from("x"), &both).expect("validate");
        assert_eq!(lines(&issues), [" must match exactly one schema in oneOf"]);
        assert!(issues[0].params.is_empty());
        // An unknown type name checks nothing (Go's switch has no default).
        let odd = object(r#"{"type":"text","minLength":5}"#);
        assert!(validate_json_schema(&Value::from(1.5), &odd)
            .expect("validate")
            .is_empty());
    }

    #[test]
    fn recursive_schema_walks_without_recursion_and_cycles_are_errors() {
        // A schema that recurses per array level: the walk keeps going without growing the
        // call stack.
        // ("#" alone is not a reference for Go: only "#/..." is resolved.)
        let schema = object(
            r##"{"$ref":"#/definitions/n","definitions":{"n":{"type":"array","items":{"$ref":"#/definitions/n"}}}}"##,
        );
        let mut deep = Value::from("leaf");
        for _ in 0..500 {
            deep = Value::Array(vec![deep]);
        }
        let issues = validate_json_schema(&deep, &schema).expect("validate");
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].path, "/0".repeat(500));
        assert_eq!(issues[0].message, "must be array");
        // A schema that refers to itself without checking the value is an error (Go
        // recurses until its stack overflows).
        let cycle = object(
            r##"{"$ref":"#/definitions/a","definitions":{"a":{"$ref":"#/definitions/a"}}}"##,
        );
        assert!(validate_json_schema(&Value::Null, &cycle).is_err());
        let cycle = object(
            r##"{"$ref":"#/definitions/a","definitions":{"a":{"oneOf":[{"$ref":"#/definitions/a"}]}}}"##,
        );
        assert!(validate_json_schema(&Value::Null, &cycle).is_err());
        let not_a_reference = object(r##"{"oneOf":[{"$ref":"#"}]}"##);
        assert!(validate_json_schema(&Value::Null, &not_a_reference)
            .expect("validate")
            .is_empty());
    }

    #[test]
    fn variant_schema_property_order_and_fields() {
        let builtin = builtin();
        let pattern = record_variant_schema(builtin.get("pattern").expect("pattern"));
        let props = obj(pattern.get("properties"));
        assert_eq!(
            props.keys().collect::<Vec<_>>(),
            [
                "id",
                "classification",
                "recorded_at",
                "evidence",
                "tags",
                "relates_to",
                "supersedes",
                "outcomes",
                "dir_anchors",
                "supersession_demoted_at",
                "anchor_decay_demoted_at",
                "owner",
                "status",
                "type",
                "name",
                "description",
                "files",
            ]
        );
        assert_eq!(
            text(obj(props.get("files")).get("type")),
            "array",
            "files of a type that extracts files is an array of strings"
        );
        assert_eq!(
            list(pattern.get("required")),
            [
                "type",
                "name",
                "description",
                "classification",
                "recorded_at"
            ]
        );
        let evidence = obj(obj(props.get("evidence")).get("properties"));
        assert_eq!(
            evidence.keys().collect::<Vec<_>>(),
            ["commit", "date", "issue", "file", "bead", "missions", "seeds", "gh", "linear"]
        );
        let registry = fixture_registry();
        let runbook = record_variant_schema(registry.get("runbook").expect("runbook"));
        let props = obj(runbook.get("properties"));
        let keys: Vec<&str> = props.keys().collect();
        assert_eq!(&keys[..3], ["id", "type", "classification"]);
        assert_eq!(&keys[13..], ["status", "title", "steps", "owner_team"]);
        // Properties are checked in that order: a custom type reports `type` before its
        // own fields.
        let r = record(
            r#"{"type":"runbook","title":5,"steps":"s","classification":"tactical","recorded_at":"now","owner_team":7}"#,
        );
        let issues = validate_record(&r, &registry).expect("validate");
        let all = lines(&issues);
        let runbook_first = registry
            .names()
            .iter()
            .position(|n| *n == "runbook")
            .expect("runbook listed");
        assert_eq!(all[runbook_first], "/title must be string");
    }

    #[test]
    fn domain_rules() {
        let cfg = object(
            r#"{"domains":{"testing":{"allowed_types":["failure",3],"required_fields":["owner","files","anchor_decay_demoted_at","resolution"]},"open":{"required_fields":["files","nothing"]}}}"#,
        );
        assert_eq!(get_allowed_types(&cfg, "testing"), ["failure"]);
        assert!(get_allowed_types(&cfg, "missing").is_empty());
        let r = record(
            r#"{"type":"pattern","owner":"","files":[],"resolution":0,"anchor_decay_demoted_at":null}"#,
        );
        assert_eq!(
            find_missing_domain_fields(&r, &get_required_fields(&cfg, "testing")),
            ["owner", "files", "anchor_decay_demoted_at"]
        );
        let violations = validate_domain_record(&r, "testing", &cfg);
        let messages: Vec<&str> = violations.iter().map(|v| v.message.as_str()).collect();
        assert_eq!(
            messages,
            [
                "type \"pattern\" is not allowed in domain \"testing\" (allowed: failure)",
                "missing required field \"owner\" in domain \"testing\"",
                "missing required field \"files\" in domain \"testing\"",
                "missing required field \"anchor_decay_demoted_at\" in domain \"testing\"",
            ]
        );
        assert_eq!(violations[0].kind, "allowed_types");
        assert_eq!(violations[1].kind, "required_fields");
        assert_eq!(violations[1].field, "owner");
        // A type that is an array is Go's Text of it, joined with commas.
        let odd = record(r#"{"type":["a",["b"]]}"#);
        assert_eq!(
            validate_domain_record(&odd, "testing", &cfg)[0].message,
            "type \"a,b\" is not allowed in domain \"testing\" (allowed: failure)"
        );

        let registry = builtin();
        let incompatible = find_incompatible_required_fields(&cfg, "testing", &registry);
        let fields: Vec<&str> = incompatible.iter().map(|i| i.field.as_str()).collect();
        assert_eq!(fields, ["files", "anchor_decay_demoted_at"]);
        assert_eq!(incompatible[0].allowed_types, ["failure"]);
        // Without allowed_types every registered type is allowed.
        let incompatible = find_incompatible_required_fields(&cfg, "open", &registry);
        assert_eq!(incompatible.len(), 1);
        assert_eq!(incompatible[0].field, "nothing");
        assert_eq!(incompatible[0].allowed_types, registry.names());
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
