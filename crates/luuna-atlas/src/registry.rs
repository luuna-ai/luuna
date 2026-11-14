//! The record type registry: the six built-in types and the custom types and field
//! aliases declared under `custom_types` in config.yaml.
//!
//! A port of the Go tool's `internal/atlas/registry.go` (`NewRegistry`, `buildCustomType`,
//! `ApplyAliases`). Validation runs in the same order and fails with the same messages.
//! Unlike the Go tool, building a registry does not replace a process-wide registry: every
//! function that needs one takes it as a parameter.

use crate::config::Config;
use crate::error::{AtlasError, Result};
use crate::json::{self, Object, Value};

/// The built-in record types, in registry order.
pub const BUILTIN_NAMES: [&str; 6] = [
    "convention",
    "pattern",
    "failure",
    "decision",
    "reference",
    "guide",
];

/// Fields every record may carry, which a custom type may not declare.
pub const CUSTOM_BASE_FIELDS: [&str; 9] = [
    "id",
    "type",
    "classification",
    "recorded_at",
    "evidence",
    "tags",
    "relates_to",
    "supersedes",
    "outcomes",
];

const NAMING_RULE: &str =
    "lowercase letters, digits, and underscores; must start with a letter; no hyphens";

const COMPACT_STRATEGIES: [&str; 4] = ["concat", "merge_outcomes", "keep_latest", "manual"];

/// One record type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeDefinition {
    pub name: String,
    pub required: Vec<String>,
    pub optional: Vec<String>,
    /// The field duplicates are detected by, or `content_hash`.
    pub dedup_key: String,
    /// The field whose value the record id is derived from, or `content_hash`.
    pub id_key: String,
    pub summary: String,
    pub section_title: String,
    pub compact: String,
    pub files_field: String,
    pub extracts_files: bool,
    pub disabled: bool,
    pub builtin: bool,
    pub summary_inherited: bool,
    /// Canonical field name and the legacy names rewritten to it on read, in config order.
    pub aliases: Vec<(String, Vec<String>)>,
    /// The built-in type this one extends, or empty.
    pub parent: String,
}

/// The record types known to one store.
#[derive(Debug, Clone)]
pub struct Registry {
    /// Built-in types, then custom types in config order.
    types: Vec<TypeDefinition>,
    disabled: Vec<String>,
}

fn builtin(
    name: &str,
    required: &[&str],
    optional: &[&str],
    key: &str,
    section_title: &str,
    extracts_files: bool,
) -> TypeDefinition {
    TypeDefinition {
        name: name.to_owned(),
        required: required.iter().map(|s| (*s).to_owned()).collect(),
        optional: optional.iter().map(|s| (*s).to_owned()).collect(),
        dedup_key: key.to_owned(),
        id_key: key.to_owned(),
        summary: format!("{{{key}}}"),
        section_title: section_title.to_owned(),
        compact: "concat".to_owned(),
        files_field: "files".to_owned(),
        extracts_files,
        disabled: false,
        builtin: true,
        summary_inherited: false,
        aliases: Vec::new(),
        parent: String::new(),
    }
}

impl Registry {
    /// Builds the registry for a store. `None` gives the built-in types only, as the Go
    /// tool does when no config could be read.
    pub fn from_config(config: Option<&Config>) -> Result<Registry> {
        let mut registry = Registry {
            types: vec![
                builtin(
                    "convention",
                    &["content"],
                    &[],
                    "content",
                    "Conventions",
                    false,
                ),
                builtin(
                    "pattern",
                    &["name", "description"],
                    &["files"],
                    "name",
                    "Patterns",
                    true,
                ),
                builtin(
                    "failure",
                    &["description", "resolution"],
                    &[],
                    "description",
                    "Known Failures",
                    false,
                ),
                builtin(
                    "decision",
                    &["title", "rationale"],
                    &["date"],
                    "title",
                    "Decisions",
                    false,
                ),
                builtin(
                    "reference",
                    &["name", "description"],
                    &["files"],
                    "name",
                    "References",
                    true,
                ),
                builtin(
                    "guide",
                    &["name", "description"],
                    &[],
                    "name",
                    "Guides",
                    false,
                ),
            ],
            disabled: go_list(config.and_then(|c| c.get("disabled_types"))),
        };
        let empty = Object::new();
        let custom = config
            .and_then(|c| c.get("custom_types"))
            .and_then(Value::as_object)
            .unwrap_or(&empty);
        let disabled_bytes = go_any_list(config.and_then(|c| c.get("disabled_types")))
            .iter()
            .filter_map(|v| v.string_bytes().map(<[u8]>::to_vec))
            .collect::<Vec<_>>();
        for (identity, value) in custom.iter() {
            let name = custom.key_text(identity);
            let c = value.as_object().unwrap_or(&empty);
            let parent_name = go_text(c.get("extends"));
            let parent_bytes = crate::records::text_bytes(c.get("extends"));
            if !parent_name.is_empty() && disabled_bytes.iter().any(|name| name == &parent_bytes) {
                return Err(message(format!(
                    "Custom type \"{name}\" extends \"{parent_name}\", which is in disabled_types. Re-enable the parent or remove the extends clause."
                )));
            }
            let definition = build_custom_type(name, c, &registry)?;
            registry.types.push(definition);
        }
        for name in registry.disabled.clone() {
            match registry.types.iter_mut().find(|t| t.name == name) {
                Some(definition) => definition.disabled = true,
                None => {
                    return Err(message(format!(
                        "disabled_types references unregistered type \"{name}\". Declare it under custom_types or remove it from disabled_types."
                    )))
                }
            }
        }
        Ok(registry)
    }

    /// The type called `name`, disabled or not.
    pub fn get(&self, name: &str) -> Option<&TypeDefinition> {
        self.types.iter().find(|t| t.name == name)
    }

    /// Every type name: built-ins first, then custom types in config order.
    pub fn names(&self) -> Vec<&str> {
        self.types.iter().map(|t| t.name.as_str()).collect()
    }

    /// The names listed under `disabled_types`.
    pub fn disabled(&self) -> &[String] {
        &self.disabled
    }
}

fn message(text: String) -> AtlasError {
    AtlasError::Message(text)
}

/// `^[a-z][a-z0-9_]*$`
fn is_valid_name(s: &str) -> bool {
    let mut bytes = s.bytes();
    matches!(bytes.next(), Some(b'a'..=b'z'))
        && bytes.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

/// Why `s` breaks the naming rule (the Go tool's `nameViolation`).
fn name_violation(s: &str) -> &'static str {
    let first = s.as_bytes().first().copied();
    match first {
        None => "must not be empty",
        _ if s.contains('-') => "contains a hyphen",
        Some(b'0'..=b'9') => "starts with a digit",
        Some(b) if !b.is_ascii_lowercase() => "must start with a lowercase letter",
        _ if s.bytes().any(|b| b.is_ascii_uppercase()) => "contains uppercase letters",
        _ => "contains a character outside [a-z0-9_]",
    }
}

fn contains(values: &[String], s: &str) -> bool {
    values.iter().any(|v| v == s)
}

/// Appends `value` unless present (the Go tool's `Unique(append(values, value))`).
fn push_unique(values: &mut Vec<String>, value: &str) {
    if !contains(values, value) {
        values.push(value.to_owned());
    }
}

fn compact_json(value: &Value) -> String {
    json::to_compact_string(value).unwrap_or_default()
}

fn build_custom_type(name: &str, c: &Object, registry: &Registry) -> Result<TypeDefinition> {
    let builtin_names = BUILTIN_NAMES.join(", ");
    if !is_valid_name(name) {
        return Err(message(format!(
            "Invalid custom_types name \"{name}\" ({}). Allowed: {NAMING_RULE}.",
            name_violation(name)
        )));
    }
    if BUILTIN_NAMES.contains(&name) {
        return Err(message(format!(
            "Custom type \"{name}\" shadows a built-in record type. Reserved names: {builtin_names}."
        )));
    }
    let mut parent: Option<&TypeDefinition> = None;
    let parent_name = go_text(c.get("extends"));
    if let Some(value) = c.get("extends") {
        if value.as_str().is_none() || parent_name.is_empty() {
            return Err(message(format!(
                "Custom type \"{name}\" extends must be a non-empty string."
            )));
        }
        if !BUILTIN_NAMES.contains(&parent_name.as_str()) {
            return Err(message(format!(
                "Custom type \"{name}\" extends \"{parent_name}\" must be a built-in type. Built-ins: {builtin_names}. Custom-from-custom inheritance is not supported in v1."
            )));
        }
        parent = registry.get(&parent_name);
    }
    let required_values = go_any_list(c.get("required"));
    let optional_values = go_any_list(c.get("optional"));
    if parent.is_none() && required_values.is_empty() {
        return Err(message(format!(
            "Custom type \"{name}\" must declare a non-empty \"required\" array (or set \"extends\" to inherit from a built-in)."
        )));
    }
    let mut merged: Vec<String> = Vec::new();
    if let Some(p) = parent {
        merged.extend(p.required.iter().cloned());
        merged.extend(p.optional.iter().cloned());
    }
    let mut seen: Vec<String> = Vec::new();
    for (kind, values) in [
        ("required", &required_values),
        ("optional", &optional_values),
    ] {
        for value in values.iter() {
            let field = match value.as_str() {
                Some(field) if !field.is_empty() => field,
                _ => {
                    return Err(message(format!(
                        "Custom type \"{name}\" has an invalid {kind} field: {}.",
                        compact_json(value)
                    )))
                }
            };
            if !is_valid_name(field) {
                return Err(message(format!(
                    "Custom type \"{name}\" {kind} field \"{field}\" ({}). Allowed: {NAMING_RULE}.",
                    name_violation(field)
                )));
            }
            if CUSTOM_BASE_FIELDS.contains(&field) {
                if kind == "required" {
                    return Err(message(format!(
                        "Custom type \"{name}\" cannot declare base field \"{field}\" as required (base fields: {}).",
                        CUSTOM_BASE_FIELDS.join(", ")
                    )));
                }
                return Err(message(format!(
                    "Custom type \"{name}\" cannot declare base field \"{field}\" as optional."
                )));
            }
            if contains(&seen, field) {
                if kind == "required" {
                    return Err(message(format!(
                        "Custom type \"{name}\" has duplicate required field \"{field}\"."
                    )));
                }
                return Err(message(format!(
                    "Custom type \"{name}\" has field \"{field}\" in both required and optional."
                )));
            }
            seen.push(field.to_owned());
            push_unique(&mut merged, field);
        }
    }
    let mut d = TypeDefinition {
        name: name.to_owned(),
        required: Vec::new(),
        optional: Vec::new(),
        dedup_key: String::new(),
        id_key: String::new(),
        summary: String::new(),
        // The name is ASCII here, so its first byte is a whole character.
        section_title: format!("{}{}s", name[..1].to_ascii_uppercase(), &name[1..]),
        compact: "manual".to_owned(),
        files_field: "files".to_owned(),
        extracts_files: false,
        disabled: false,
        builtin: false,
        summary_inherited: false,
        aliases: Vec::new(),
        parent: parent_name.clone(),
    };
    if let Some(p) = parent {
        d.required = p.required.clone();
        d.optional = p.optional.clone();
        d.dedup_key = p.dedup_key.clone();
        d.id_key = p.id_key.clone();
        d.summary = p.summary.clone();
        d.extracts_files = p.extracts_files;
        d.files_field = p.files_field.clone();
        d.compact = p.compact.clone();
        d.section_title = p.section_title.clone();
    }
    let mut required: Vec<String> = Vec::new();
    for field in d.required.iter().chain(&go_list(c.get("required"))) {
        push_unique(&mut required, field);
    }
    let mut optional: Vec<String> = Vec::new();
    for field in d.optional.iter().chain(&go_list(c.get("optional"))) {
        push_unique(&mut optional, field);
    }
    d.required = required;
    d.optional = optional
        .into_iter()
        .filter(|f| !contains(&d.required, f))
        .collect();
    if let Some(v) = c.get("dedup_key") {
        d.dedup_key = go_text(Some(v));
    }
    if d.dedup_key.is_empty() {
        return Err(message(format!(
            "Custom type \"{name}\" must set \"dedup_key\" (field name or \"content_hash\"), or extend a built-in that defines one."
        )));
    }
    if d.dedup_key != "content_hash" && !contains(&merged, &d.dedup_key) {
        return Err(message(format!(
            "Custom type \"{name}\" dedup_key \"{}\" must be declared in required/optional or be \"content_hash\".",
            d.dedup_key
        )));
    }
    if let Some(v) = c.get("id_key") {
        d.id_key = go_text(Some(v));
    } else if let Some(v) = c.get("dedup_key") {
        d.id_key = go_text(Some(v));
    } else if d.id_key.is_empty() {
        d.id_key = d.dedup_key.clone();
    }
    if d.id_key != "content_hash" && !contains(&merged, &d.id_key) {
        return Err(message(format!(
            "Custom type \"{name}\" id_key \"{}\" must be declared in required/optional or be \"content_hash\".",
            d.id_key
        )));
    }
    if let Some(v) = c.get("summary") {
        match v.as_str() {
            Some(s) if !s.is_empty() => d.summary = s.to_owned(),
            _ => {
                return Err(message(format!(
                    "Custom type \"{name}\" \"summary\" must be a non-empty string."
                )))
            }
        }
    } else if parent.is_none() {
        return Err(message(format!(
            "Custom type \"{name}\" must set \"summary\" template string."
        )));
    } else {
        d.summary_inherited = true;
    }
    let mut known: Vec<String> = Vec::new();
    for field in CUSTOM_BASE_FIELDS
        .iter()
        .copied()
        .chain(merged.iter().map(String::as_str))
    {
        push_unique(&mut known, field);
    }
    for token in summary_tokens(&d.summary) {
        if !contains(&known, &token) {
            known.sort();
            return Err(message(format!(
                "Custom type \"{name}\" summary references unknown field \"{{{token}}}\". Declared fields: {}.",
                known.join(", ")
            )));
        }
    }
    if let Some(v) = c.get("compact") {
        d.compact = go_text(Some(v));
        if !COMPACT_STRATEGIES.contains(&d.compact.as_str()) {
            return Err(message(format!(
                "Custom type \"{name}\" compact strategy \"{}\" is invalid. Use one of: concat, merge_outcomes, keep_latest, manual.",
                d.compact
            )));
        }
    }
    if let Some(v) = c.get("section_title") {
        d.section_title = go_text(Some(v));
    }
    if let Some(v) = c.get("extracts_files") {
        d.extracts_files = go_bool(Some(v));
    }
    if let Some(v) = c.get("files_field") {
        d.files_field = go_text(Some(v));
        if d.extracts_files && !contains(&merged, &d.files_field) {
            return Err(message(format!(
                "Custom type \"{name}\" files_field \"{}\" must be declared in required/optional when extracts_files: true.",
                d.files_field
            )));
        }
    }
    if let Some(value) = c.get("aliases") {
        let Some(aliases) = value.as_object() else {
            return Err(message(format!(
                "Custom type \"{name}\" aliases must be a map of canonical field name → legacy alias names."
            )));
        };
        let mut all_builtins: Vec<String> = Vec::new();
        for builtin_name in BUILTIN_NAMES {
            if let Some(t) = registry.get(builtin_name) {
                for field in t.required.iter().chain(&t.optional) {
                    push_unique(&mut all_builtins, field);
                }
            }
        }
        let mut seen_aliases: Vec<String> = Vec::new();
        for (identity, legacy_value) in aliases.iter() {
            let canonical = aliases.key_text(identity);
            if !contains(&merged, canonical) {
                return Err(message(format!(
                    "Custom type \"{name}\" aliases key \"{canonical}\" must be declared in required/optional."
                )));
            }
            let raw_names = go_any_list(Some(legacy_value));
            let names = go_list(Some(legacy_value));
            if raw_names.is_empty() {
                return Err(message(format!(
                    "Custom type \"{name}\" aliases entry \"{canonical}\" must be a non-empty array of legacy field names."
                )));
            }
            for raw_legacy in &raw_names {
                let legacy = match raw_legacy.as_str() {
                    Some(legacy) if !legacy.is_empty() => legacy,
                    _ => {
                        return Err(message(format!(
                            "Custom type \"{name}\" aliases entry \"{canonical}\" has an invalid legacy name: {}.",
                            compact_json(raw_legacy)
                        )))
                    }
                };
                if !is_valid_name(legacy) {
                    return Err(message(format!(
                        "Custom type \"{name}\" aliases legacy name \"{legacy}\" ({}). Allowed: {NAMING_RULE}.",
                        name_violation(legacy)
                    )));
                }
                if CUSTOM_BASE_FIELDS.contains(&legacy) {
                    return Err(message(format!(
                        "Custom type \"{name}\" aliases legacy name \"{legacy}\" collides with a base field."
                    )));
                }
                if contains(&all_builtins, legacy) && !contains(&merged, legacy) {
                    return Err(message(format!(
                        "Custom type \"{name}\" aliases legacy name \"{legacy}\" collides with a built-in type's field. Use a name that isn't already owned by another type, or set it as required/optional on this type if the field really belongs here."
                    )));
                }
                if contains(&merged, legacy) {
                    return Err(message(format!(
                        "Custom type \"{name}\" aliases legacy name \"{legacy}\" is already declared as a current field; aliases are for retired names only."
                    )));
                }
                if contains(&seen_aliases, legacy) {
                    return Err(message(format!(
                        "Custom type \"{name}\" aliases legacy name \"{legacy}\" appears under multiple canonical fields; each alias maps to one canonical name."
                    )));
                }
                seen_aliases.push(legacy.to_owned());
            }
            d.aliases.push((canonical.to_owned(), names));
        }
    }
    Ok(d)
}

/// Whether `c` is in the class `(?i)[a-z_]` as Go's regexp folds it (the Kelvin sign and
/// the long s fold to `k` and `s`).
fn is_token_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_' || c == '\u{17F}' || c == '\u{212A}'
}

fn is_token_char(c: char) -> bool {
    is_token_start(c) || c.is_ascii_digit()
}

/// The field names a summary template references: every non-overlapping match of the Go
/// tool's `(?i)\{\{([a-z_][a-z0-9_]*)\}\}|\{([a-z_][a-z0-9_]*)\}`, leftmost first.
pub(crate) fn summary_tokens(template: &str) -> Vec<String> {
    /// The identifier at the start of `s` followed by `}`, and its length in bytes.
    fn ident(s: &str) -> Option<&str> {
        let mut chars = s.char_indices();
        let (_, first) = chars.next()?;
        if !is_token_start(first) {
            return None;
        }
        let end = chars
            .find(|(_, c)| !is_token_char(*c))
            .map_or(s.len(), |(i, _)| i);
        s[end..].starts_with('}').then(|| &s[..end])
    }
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < template.len() {
        let rest = &template[i..];
        if let Some(after) = rest.strip_prefix("{{") {
            if let Some(name) = ident(after) {
                if after[name.len() + 1..].starts_with('}') {
                    tokens.push(name.to_owned());
                    i += 2 + name.len() + 2;
                    continue;
                }
            }
        }
        if let Some(after) = rest.strip_prefix('{') {
            if let Some(name) = ident(after) {
                tokens.push(name.to_owned());
                i += 1 + name.len() + 1;
                continue;
            }
        }
        i += rest.chars().next().map_or(1, char::len_utf8);
    }
    tokens
}

/// Rewrites a record's legacy field names to their canonical names (the Go tool's
/// `ApplyAliases`). A legacy value fills the canonical field only when that field is
/// absent, null or the empty string; the legacy field is removed either way.
pub fn apply_aliases(record: &mut Object, aliases: &[(String, Vec<String>)]) {
    for (canonical, names) in aliases {
        for legacy in names {
            if let Some(value) = record.get(legacy).cloned() {
                let empty = match record.get(canonical) {
                    None | Some(Value::Null) => true,
                    Some(Value::String(s)) => s.is_empty(),
                    Some(_) => false,
                };
                if empty {
                    // A Go map assignment: the field keeps its place if Go remembers one.
                    record.insert_unordered(canonical.clone(), value);
                }
                record.remove(legacy);
            }
        }
    }
}

// ---------------------------------------------------------------------------------------
// The Go tool's loose conversions of a JSON or YAML value (`model.go`).

/// `Text`: a value as the Go tool prints it into a string.
pub(crate) fn go_text(value: Option<&Value>) -> String {
    match value {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(s)) => s.clone(),
        Some(Value::RawString(s)) => s.text().to_owned(),
        Some(Value::Bool(b)) => b.to_string(),
        Some(Value::Number(n)) => go_format_float(*n),
        // strconv.Itoa for an int, fmt.Sprint for a uint64: the decimal digits.
        Some(Value::Integer(i)) => i.to_string(),
        Some(Value::Array(_)) => crate::records::text_value(value),
        Some(Value::Object(_)) => "[object Object]".to_owned(),
    }
}

/// `strconv.FormatFloat(n, 'f', -1, 64)`: the shortest digits that read back as `n`,
/// never in exponent form.
fn go_format_float(n: f64) -> String {
    if n.is_nan() {
        "NaN".to_owned()
    } else if n.is_infinite() {
        if n > 0.0 { "+Inf" } else { "-Inf" }.to_owned()
    } else {
        // Rust's `Display` for f64 prints the shortest round-trip digits without an
        // exponent, which is what Go's 'f' format with precision -1 prints.
        n.to_string()
    }
}

/// `Bool`: whether a value counts as set.
pub(crate) fn go_bool(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::RawString(s)) => !s.bytes().is_empty(),
        Some(Value::Number(n)) => *n != 0.0 && !n.is_nan(),
        // An int counts when it is not 0; a uint64 (always above the int range) always does.
        Some(Value::Integer(i)) => *i != 0,
        Some(Value::Array(_) | Value::Object(_)) => true,
    }
}

/// `List`: the string elements of an array; anything else is empty.
pub(crate) fn go_list(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|item| item.as_str().map(str::to_owned))
            .collect(),
        _ => Vec::new(),
    }
}

/// `AnyList`: the elements of an array; anything else is empty.
fn go_any_list(value: Option<&Value>) -> Vec<Value> {
    match value {
        Some(Value::Array(items)) => items.clone(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::json::parse;

    fn config(text: &str) -> Config {
        parse(text).unwrap().as_object().unwrap().clone()
    }

    fn strings(values: &[&str]) -> Vec<String> {
        values.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn builtin_types_match_reference() {
        // registry.go:44-49 at the reference commit.
        let registry = Registry::from_config(None).unwrap();
        assert_eq!(registry.names(), BUILTIN_NAMES);
        assert!(registry.disabled().is_empty());
        /// Name, required, optional, dedup and id key, section title, extracts files.
        type Expected<'a> = (
            &'a str,
            &'a [&'a str],
            &'a [&'a str],
            &'a str,
            &'a str,
            bool,
        );
        let expected: [Expected; 6] = [
            (
                "convention",
                &["content"],
                &[],
                "content",
                "Conventions",
                false,
            ),
            (
                "pattern",
                &["name", "description"],
                &["files"],
                "name",
                "Patterns",
                true,
            ),
            (
                "failure",
                &["description", "resolution"],
                &[],
                "description",
                "Known Failures",
                false,
            ),
            (
                "decision",
                &["title", "rationale"],
                &["date"],
                "title",
                "Decisions",
                false,
            ),
            (
                "reference",
                &["name", "description"],
                &["files"],
                "name",
                "References",
                true,
            ),
            (
                "guide",
                &["name", "description"],
                &[],
                "name",
                "Guides",
                false,
            ),
        ];
        for (name, required, optional, key, section, extracts) in expected {
            let t = registry.get(name).unwrap();
            assert_eq!(t.required, strings(required), "{name}");
            assert_eq!(t.optional, strings(optional), "{name}");
            assert_eq!(t.dedup_key, key, "{name}");
            assert_eq!(t.id_key, key, "{name}");
            assert_eq!(t.summary, format!("{{{key}}}"), "{name}");
            assert_eq!(t.section_title, section, "{name}");
            assert_eq!(t.compact, "concat", "{name}");
            assert_eq!(t.files_field, "files", "{name}");
            assert_eq!(t.extracts_files, extracts, "{name}");
            assert!(t.builtin && !t.disabled && t.aliases.is_empty() && t.parent.is_empty());
        }
        assert!(registry.get("hypothesis").is_none());
        // An empty config is the same as none.
        let registry = Registry::from_config(Some(&Object::new())).unwrap();
        assert_eq!(registry.names(), BUILTIN_NAMES);
    }

    #[test]
    fn custom_type_aliases_rewrite_fields() {
        let cfg = config(
            r#"{"custom_types":{
                "hypothesis":{"required":["statement"],"optional":["prediction","evidence_files"],
                  "dedup_key":"statement","summary":"{statement} -> {{prediction}}",
                  "aliases":{"statement":["claim","thesis"],"prediction":["guess"]}},
                "runbook":{"extends":"guide","optional":["steps"]}},
              "disabled_types":["runbook"]}"#,
        );
        let registry = Registry::from_config(Some(&cfg)).unwrap();
        assert_eq!(
            registry.names(),
            [&BUILTIN_NAMES[..], &["hypothesis", "runbook"]].concat()
        );
        let h = registry.get("hypothesis").unwrap();
        assert_eq!(h.required, strings(&["statement"]));
        assert_eq!(h.optional, strings(&["prediction", "evidence_files"]));
        assert_eq!(
            (h.dedup_key.as_str(), h.id_key.as_str()),
            ("statement", "statement")
        );
        assert_eq!(h.section_title, "Hypothesiss");
        assert_eq!(h.compact, "manual");
        assert_eq!(
            h.aliases,
            vec![
                ("statement".to_owned(), strings(&["claim", "thesis"])),
                ("prediction".to_owned(), strings(&["guess"])),
            ]
        );
        let r = registry.get("runbook").unwrap();
        assert!(r.disabled && r.summary_inherited);
        assert_eq!(r.parent, "guide");
        assert_eq!(r.required, strings(&["name", "description"]));
        assert_eq!(r.optional, strings(&["steps"]));
        assert_eq!(
            (r.id_key.as_str(), r.section_title.as_str()),
            ("name", "Guides")
        );

        // Legacy names are rewritten: an absent canonical field is set after the fields
        // Go remembers, sorted; a present but empty one is filled in place; a non-empty
        // one wins and the legacy field is still dropped.
        let mut record =
            config(r#"{"type":"hypothesis","thesis":"T","prediction":"","guess":"G","z":1}"#);
        apply_aliases(&mut record, &h.aliases);
        assert_eq!(
            json::to_compact_string(&Value::Object(record)).unwrap(),
            r#"{"type":"hypothesis","prediction":"G","z":1,"statement":"T"}"#
        );
        let mut record = config(r#"{"statement":"S","claim":"C","thesis":"T"}"#);
        apply_aliases(&mut record, &h.aliases);
        assert_eq!(
            json::to_compact_string(&Value::Object(record)).unwrap(),
            r#"{"statement":"S"}"#
        );

        // The Go messages for invalid declarations.
        let cases = [
            (r#"{"custom_types":{"Bad-name":{}}}"#, "Invalid custom_types name \"Bad-name\" (contains a hyphen). Allowed: lowercase letters, digits, and underscores; must start with a letter; no hyphens."),
            (r#"{"custom_types":{"guide":{}}}"#, "Custom type \"guide\" shadows a built-in record type. Reserved names: convention, pattern, failure, decision, reference, guide."),
            (r#"{"custom_types":{"x":{"required":["a"],"dedup_key":"a","summary":"{b}"}}}"#, "Custom type \"x\" summary references unknown field \"{b}\". Declared fields: a, classification, evidence, id, outcomes, recorded_at, relates_to, supersedes, tags, type."),
            (r#"{"custom_types":{"x":{"required":["a"],"dedup_key":"a","summary":"s","aliases":{"a":["name"]}}}}"#, "Custom type \"x\" aliases legacy name \"name\" collides with a built-in type's field. Use a name that isn't already owned by another type, or set it as required/optional on this type if the field really belongs here."),
            (r#"{"custom_types":{"x":{"required":["a",1]}}}"#, "Custom type \"x\" has an invalid required field: 1."),
            (r#"{"disabled_types":["nope"]}"#, "disabled_types references unregistered type \"nope\". Declare it under custom_types or remove it from disabled_types."),
        ];
        for (text, expected) in cases {
            let error = Registry::from_config(Some(&config(text))).unwrap_err();
            assert_eq!(error.to_string(), expected, "{text}");
        }
    }

    #[test]
    fn summary_tokens_follow_the_go_pattern() {
        assert_eq!(
            summary_tokens("{a} {{b_1}} {{c} {D} {1x} {e"),
            ["a", "b_1", "c", "D"]
        );
        assert_eq!(summary_tokens("é{\u{212A}}"), ["\u{212A}"]);
    }

    #[test]
    fn go_text_and_bool_match_model_go() {
        assert_eq!(
            go_text(Some(&Value::Number(1e21))),
            "1000000000000000000000"
        );
        assert_eq!(go_text(Some(&Value::Number(1.5))), "1.5");
        assert_eq!(go_text(Some(&Value::Number(-0.0))), "-0");
        assert_eq!(
            go_text(Some(&parse(r#"["a",1,null,true,{}]"#).unwrap())),
            "a,1,,true,[object Object]"
        );
        assert!(!go_bool(Some(&Value::String(String::new()))));
        assert!(!go_bool(Some(&Value::Number(0.0))));
        assert!(go_bool(Some(&Value::Array(Vec::new()))));
    }

    #[test]
    fn go_text_and_bool_of_exact_integers() {
        // strconv.Itoa for an int, fmt.Sprint for a uint64; Bool is n != 0 for an int and
        // true for a uint64.
        assert_eq!(
            go_text(Some(&Value::Integer(9_007_199_254_740_993))),
            "9007199254740993"
        );
        assert_eq!(
            go_text(Some(&Value::Integer(i128::from(i64::MIN)))),
            "-9223372036854775808"
        );
        assert_eq!(
            go_text(Some(&Value::Integer(i128::from(u64::MAX)))),
            "18446744073709551615"
        );
        assert!(!go_bool(Some(&Value::Integer(0))));
        assert!(go_bool(Some(&Value::Integer(-1))));
        assert!(go_bool(Some(&Value::Integer(i128::from(u64::MAX)))));
    }
}
