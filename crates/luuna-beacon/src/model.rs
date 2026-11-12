//! The stored records: prompts and schemas, one JSON object per line, written
//! byte for byte as the Go tool writes them (`internal/model/model.go`).

use crate::gojson::{self, GoValue, ObjectWriter};
use serde_json::Value;
use std::collections::BTreeMap;

/// One named block of a prompt.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Section {
    pub name: String,
    pub body: String,
    pub required: Option<bool>,
}

/// The expertise a prompt asks to be primed with.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AtlasPrime {
    pub domains: Vec<String>,
    pub files: Vec<String>,
}

/// A prompt's Atlas declaration.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Atlas {
    pub prime: Option<AtlasPrime>,
    pub budget: Option<f64>,
    pub on_empty: String,
}

/// One version of a prompt, as stored in `prompts.jsonl`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Prompt {
    pub id: String,
    pub name: String,
    pub description: String,
    pub version: i64,
    /// `None` is written as JSON `null`, `Some(vec![])` as `[]`.
    pub sections: Option<Vec<Section>>,
    pub extends: String,
    pub mixins: Vec<String>,
    pub tags: Vec<String>,
    pub schema: String,
    pub emit_as: String,
    pub emit_dir: String,
    pub pinned: Option<i64>,
    pub frontmatter: BTreeMap<String, Value>,
    pub atlas: Option<Atlas>,
    pub extends_atlas: Option<bool>,
    pub status: String,
    pub created_at: String,
    pub updated_at: String,
}

/// A rule a schema applies to one section's body.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Rule {
    pub section: String,
    pub pattern: String,
    pub message: String,
}

/// One version of a schema, as stored in `schemas.jsonl`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Schema {
    pub id: String,
    pub name: String,
    /// `None` is written as JSON `null`, `Some(vec![])` as `[]`.
    pub required_sections: Option<Vec<String>>,
    pub optional_sections: Vec<String>,
    pub rules: Vec<Rule>,
    pub created_at: String,
    pub updated_at: String,
}

/// A record stored one per line in a JSONL file.
pub(crate) trait Record: Sized {
    /// Decodes one line as Go's `json.Unmarshal` does into the Go type.
    fn from_go_json(line: &[u8]) -> Result<Self, String>;
    /// The line Go's `json.Marshal` writes, without the newline.
    fn to_go_json(&self) -> String;
}

// ---- Writing -----------------------------------------------------------------

fn write_section(out: &mut String, section: &Section) {
    let mut object = ObjectWriter::new(out);
    object.string("name", &section.name);
    object.string("body", &section.body);
    if let Some(required) = section.required {
        gojson::write_bool(object.key("required"), required);
    }
    object.finish();
}

fn write_atlas_prime(out: &mut String, prime: &AtlasPrime) {
    let mut object = ObjectWriter::new(out);
    object.strings_omitempty("domains", &prime.domains);
    object.strings_omitempty("files", &prime.files);
    object.finish();
}

fn write_atlas(out: &mut String, atlas: &Atlas) {
    let mut object = ObjectWriter::new(out);
    if let Some(prime) = &atlas.prime {
        write_atlas_prime(object.key("prime"), prime);
    }
    if let Some(budget) = atlas.budget {
        gojson::write_float(object.key("budget"), budget);
    }
    object.string_omitempty("on_empty", &atlas.on_empty);
    object.finish();
}

fn write_rule(out: &mut String, rule: &Rule) {
    let mut object = ObjectWriter::new(out);
    object.string("section", &rule.section);
    object.string("pattern", &rule.pattern);
    object.string("message", &rule.message);
    object.finish();
}

impl Prompt {
    /// The compact line Go's `json.Marshal` writes for this record (fields in
    /// `model.Prompt` order with its `omitempty` rules), without a newline.
    pub fn to_go_json(&self) -> String {
        let mut out = String::new();
        let mut object = ObjectWriter::new(&mut out);
        object.string("id", &self.id);
        object.string("name", &self.name);
        object.string_omitempty("description", &self.description);
        object.int("version", self.version);
        match &self.sections {
            None => object.key("sections").push_str("null"),
            Some(sections) => {
                let out = object.key("sections");
                out.push('[');
                for (index, section) in sections.iter().enumerate() {
                    if index > 0 {
                        out.push(',');
                    }
                    write_section(out, section);
                }
                out.push(']');
            }
        }
        object.string_omitempty("extends", &self.extends);
        object.strings_omitempty("mixins", &self.mixins);
        object.strings_omitempty("tags", &self.tags);
        object.string_omitempty("schema", &self.schema);
        object.string_omitempty("emitAs", &self.emit_as);
        object.string_omitempty("emitDir", &self.emit_dir);
        if let Some(pinned) = self.pinned {
            object.int("pinned", pinned);
        }
        if !self.frontmatter.is_empty() {
            gojson::write_map(object.key("frontmatter"), &self.frontmatter);
        }
        if let Some(atlas) = &self.atlas {
            write_atlas(object.key("atlas"), atlas);
        }
        if let Some(extends_atlas) = self.extends_atlas {
            gojson::write_bool(object.key("extends_atlas"), extends_atlas);
        }
        object.string("status", &self.status);
        object.string("createdAt", &self.created_at);
        object.string("updatedAt", &self.updated_at);
        object.finish();
        out
    }

    /// Decodes one stored line as Go's `Prompt.UnmarshalJSON` does
    /// (`model.go:53-83`): unknown keys are ignored, and the legacy keys
    /// `mulch` and `extends_mulch` are read when `atlas` and `extends_atlas`
    /// are absent or null.
    pub(crate) fn from_go_json(line: &[u8]) -> Result<Prompt, String> {
        let value = gojson::parse(line)?;
        let mut prompt = Prompt::default();
        let Some(entries) = gojson::object_entries(&value, "model.Prompt")? else {
            return Ok(prompt);
        };
        for (key, item) in entries {
            prompt_field_into(&mut prompt, key, item)?;
        }
        // The legacy keys are looked up by exact name in a map, so the last
        // occurrence wins.
        let legacy = |name: &str| {
            entries
                .iter()
                .rev()
                .find(|(key, _)| key == name)
                .map(|(_, item)| item)
        };
        if prompt.atlas.is_none() {
            if let Some(item) = legacy("mulch") {
                // Go unmarshals into a zero Atlas and keeps it, even for null.
                let mut atlas = Atlas::default();
                atlas_into(&mut atlas, item)?;
                prompt.atlas = Some(atlas);
            }
        }
        if prompt.extends_atlas.is_none() {
            if let Some(item) = legacy("extends_mulch") {
                let mut enabled = false;
                gojson::bool_into(&mut enabled, item)?;
                prompt.extends_atlas = Some(enabled);
            }
        }
        Ok(prompt)
    }
}

impl Schema {
    /// The compact line Go's `json.Marshal` writes for this record (fields in
    /// `model.Schema` order with its `omitempty` rules), without a newline.
    pub fn to_go_json(&self) -> String {
        let mut out = String::new();
        let mut object = ObjectWriter::new(&mut out);
        object.string("id", &self.id);
        object.string("name", &self.name);
        gojson::write_strings(
            object.key("requiredSections"),
            self.required_sections.as_deref(),
        );
        object.strings_omitempty("optionalSections", &self.optional_sections);
        if !self.rules.is_empty() {
            let out = object.key("rules");
            out.push('[');
            for (index, rule) in self.rules.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                write_rule(out, rule);
            }
            out.push(']');
        }
        object.string("createdAt", &self.created_at);
        object.string("updatedAt", &self.updated_at);
        object.finish();
        out
    }

    /// Decodes one stored line as Go's `json.Unmarshal` into `model.Schema`.
    pub(crate) fn from_go_json(line: &[u8]) -> Result<Schema, String> {
        let value = gojson::parse(line)?;
        let mut schema = Schema::default();
        let Some(entries) = gojson::object_entries(&value, "model.Schema")? else {
            return Ok(schema);
        };
        for (key, item) in entries {
            let Some(name) = gojson::field(key, SCHEMA_FIELDS) else {
                continue;
            };
            match name {
                "id" => gojson::string_into(&mut schema.id, item)?,
                "name" => gojson::string_into(&mut schema.name, item)?,
                "requiredSections" => {
                    gojson::slice_into(&mut schema.required_sections, item, gojson::string_into)?
                }
                "optionalSections" => gojson::strings_into(&mut schema.optional_sections, item)?,
                "rules" => {
                    let mut rules = Some(std::mem::take(&mut schema.rules));
                    let result = gojson::slice_into(&mut rules, item, rule_into);
                    schema.rules = rules.unwrap_or_default();
                    result?;
                }
                "createdAt" => gojson::string_into(&mut schema.created_at, item)?,
                "updatedAt" => gojson::string_into(&mut schema.updated_at, item)?,
                _ => unreachable!("every schema field is handled"),
            }
        }
        Ok(schema)
    }
}

impl Record for Prompt {
    fn from_go_json(line: &[u8]) -> Result<Self, String> {
        Prompt::from_go_json(line)
    }
    fn to_go_json(&self) -> String {
        Prompt::to_go_json(self)
    }
}

impl Record for Schema {
    fn from_go_json(line: &[u8]) -> Result<Self, String> {
        Schema::from_go_json(line)
    }
    fn to_go_json(&self) -> String {
        Schema::to_go_json(self)
    }
}

// ---- Reading -----------------------------------------------------------------

const PROMPT_FIELDS: &[&str] = &[
    "id",
    "name",
    "description",
    "version",
    "sections",
    "extends",
    "mixins",
    "tags",
    "schema",
    "emitAs",
    "emitDir",
    "pinned",
    "frontmatter",
    "atlas",
    "extends_atlas",
    "status",
    "createdAt",
    "updatedAt",
];
const SECTION_FIELDS: &[&str] = &["name", "body", "required"];
const ATLAS_FIELDS: &[&str] = &["prime", "budget", "on_empty"];
const ATLAS_PRIME_FIELDS: &[&str] = &["domains", "files"];
const RULE_FIELDS: &[&str] = &["section", "pattern", "message"];
const SCHEMA_FIELDS: &[&str] = &[
    "id",
    "name",
    "requiredSections",
    "optionalSections",
    "rules",
    "createdAt",
    "updatedAt",
];

fn prompt_field_into(prompt: &mut Prompt, key: &str, item: &GoValue) -> Result<(), String> {
    let Some(name) = gojson::field(key, PROMPT_FIELDS) else {
        return Ok(());
    };
    match name {
        "id" => gojson::string_into(&mut prompt.id, item),
        "name" => gojson::string_into(&mut prompt.name, item),
        "description" => gojson::string_into(&mut prompt.description, item),
        "version" => gojson::int_into(&mut prompt.version, item),
        "sections" => gojson::slice_into(&mut prompt.sections, item, section_into),
        "extends" => gojson::string_into(&mut prompt.extends, item),
        "mixins" => gojson::strings_into(&mut prompt.mixins, item),
        "tags" => gojson::strings_into(&mut prompt.tags, item),
        "schema" => gojson::string_into(&mut prompt.schema, item),
        "emitAs" => gojson::string_into(&mut prompt.emit_as, item),
        "emitDir" => gojson::string_into(&mut prompt.emit_dir, item),
        "pinned" => gojson::optional_int_into(&mut prompt.pinned, item),
        "frontmatter" => gojson::map_into(&mut prompt.frontmatter, item),
        "atlas" => optional_atlas_into(&mut prompt.atlas, item),
        "extends_atlas" => gojson::optional_bool_into(&mut prompt.extends_atlas, item),
        "status" => gojson::string_into(&mut prompt.status, item),
        "createdAt" => gojson::string_into(&mut prompt.created_at, item),
        "updatedAt" => gojson::string_into(&mut prompt.updated_at, item),
        _ => unreachable!("every prompt field is handled"),
    }
}

fn section_into(section: &mut Section, value: &GoValue) -> Result<(), String> {
    let Some(entries) = gojson::object_entries(value, "model.Section")? else {
        return Ok(());
    };
    for (key, item) in entries {
        match gojson::field(key, SECTION_FIELDS) {
            Some("name") => gojson::string_into(&mut section.name, item)?,
            Some("body") => gojson::string_into(&mut section.body, item)?,
            Some("required") => gojson::optional_bool_into(&mut section.required, item)?,
            _ => {}
        }
    }
    Ok(())
}

/// Into a Go `*Atlas`: `null` sets nil; otherwise the existing value (or a
/// new zero one) is decoded into.
fn optional_atlas_into(target: &mut Option<Atlas>, value: &GoValue) -> Result<(), String> {
    if let GoValue::Null = value {
        *target = None;
        return Ok(());
    }
    atlas_into(target.get_or_insert_with(Atlas::default), value)
}

fn atlas_into(atlas: &mut Atlas, value: &GoValue) -> Result<(), String> {
    let Some(entries) = gojson::object_entries(value, "model.Atlas")? else {
        return Ok(());
    };
    for (key, item) in entries {
        match gojson::field(key, ATLAS_FIELDS) {
            Some("prime") => {
                if let GoValue::Null = item {
                    atlas.prime = None;
                } else {
                    atlas_prime_into(atlas.prime.get_or_insert_with(AtlasPrime::default), item)?;
                }
            }
            Some("budget") => gojson::optional_float_into(&mut atlas.budget, item)?,
            Some("on_empty") => gojson::string_into(&mut atlas.on_empty, item)?,
            _ => {}
        }
    }
    Ok(())
}

fn atlas_prime_into(prime: &mut AtlasPrime, value: &GoValue) -> Result<(), String> {
    let Some(entries) = gojson::object_entries(value, "model.AtlasPrime")? else {
        return Ok(());
    };
    for (key, item) in entries {
        match gojson::field(key, ATLAS_PRIME_FIELDS) {
            Some("domains") => gojson::strings_into(&mut prime.domains, item)?,
            Some("files") => gojson::strings_into(&mut prime.files, item)?,
            _ => {}
        }
    }
    Ok(())
}

fn rule_into(rule: &mut Rule, value: &GoValue) -> Result<(), String> {
    let Some(entries) = gojson::object_entries(value, "model.Rule")? else {
        return Ok(());
    };
    for (key, item) in entries {
        match gojson::field(key, RULE_FIELDS) {
            Some("section") => gojson::string_into(&mut rule.section, item)?,
            Some("pattern") => gojson::string_into(&mut rule.pattern, item)?,
            Some("message") => gojson::string_into(&mut rule.message, item)?,
            _ => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const STAMP: &str = r#""createdAt":"2024-01-01T00:00:00Z","updatedAt":"2024-01-01T00:00:00Z""#;

    #[test]
    fn legacy_atlas_keys_are_read_and_current_keys_written() {
        // Only the legacy keys: they are read into atlas and extends_atlas.
        let line = format!(
            r#"{{"id":"p-1","name":"legacy","version":1,"sections":[{{"name":"T","body":"b"}}],"mulch":{{"prime":{{"domains":["old"]}},"budget":3}},"extends_mulch":true,"status":"active",{STAMP}}}"#
        );
        let prompt = Prompt::from_go_json(line.as_bytes()).unwrap();
        assert_eq!(
            prompt.atlas,
            Some(Atlas {
                prime: Some(AtlasPrime {
                    domains: vec!["old".to_owned()],
                    files: Vec::new(),
                }),
                budget: Some(3.0),
                on_empty: String::new(),
            })
        );
        assert_eq!(prompt.extends_atlas, Some(true));
        // Writing uses the current names only.
        assert_eq!(
            prompt.to_go_json(),
            format!(
                r#"{{"id":"p-1","name":"legacy","version":1,"sections":[{{"name":"T","body":"b"}}],"atlas":{{"prime":{{"domains":["old"]}},"budget":3}},"extends_atlas":true,"status":"active",{STAMP}}}"#
            )
        );

        // Both present: the current keys win, the legacy ones are ignored.
        let line = format!(
            r#"{{"id":"p-2","name":"both","version":1,"sections":[],"atlas":{{"budget":9}},"mulch":{{"budget":1}},"extends_atlas":false,"extends_mulch":true,"status":"active",{STAMP}}}"#
        );
        let prompt = Prompt::from_go_json(line.as_bytes()).unwrap();
        assert_eq!(prompt.atlas.as_ref().unwrap().budget, Some(9.0));
        assert_eq!(prompt.extends_atlas, Some(false));
        assert_eq!(
            prompt.to_go_json(),
            format!(
                r#"{{"id":"p-2","name":"both","version":1,"sections":[],"atlas":{{"budget":9}},"extends_atlas":false,"status":"active",{STAMP}}}"#
            )
        );

        // A current key that is null falls back to the legacy key; a null
        // legacy atlas is a present, empty atlas, and a null extends_mulch is
        // false, as Go's unmarshalling into zero values gives.
        let prompt = Prompt::from_go_json(
            br#"{"atlas":null,"mulch":{"on_empty":"warn"},"extends_atlas":null,"extends_mulch":null}"#,
        )
        .unwrap();
        assert_eq!(prompt.atlas.as_ref().unwrap().on_empty, "warn");
        assert_eq!(prompt.extends_atlas, Some(false));
        let prompt = Prompt::from_go_json(br#"{"mulch":null}"#).unwrap();
        assert_eq!(prompt.atlas, Some(Atlas::default()));
        assert!(prompt.to_go_json().contains(r#""atlas":{}"#));
        // A malformed legacy key fails the read only when it is used.
        assert!(Prompt::from_go_json(br#"{"mulch":{"budget":"x"}}"#).is_err());
        assert!(Prompt::from_go_json(br#"{"atlas":{},"mulch":{"budget":"x"}}"#).is_ok());
        assert!(Prompt::from_go_json(br#"{"extends_mulch":"yes"}"#).is_err());
        // The legacy keys match exactly, not case-insensitively.
        let prompt = Prompt::from_go_json(br#"{"MULCH":{"budget":1}}"#).unwrap();
        assert_eq!(prompt.atlas, None);

        // Unknown keys, including nested ones, are ignored and not written.
        let line = format!(
            r#"{{"id":"p-4","name":"unknown","version":1,"sections":[{{"name":"T","body":"b","extra":1}}],"future":{{"a":[1,2]}},"description":"","tags":[],"status":"active",{STAMP}}}"#
        );
        let prompt = Prompt::from_go_json(line.as_bytes()).unwrap();
        assert_eq!(
            prompt.to_go_json(),
            format!(
                r#"{{"id":"p-4","name":"unknown","version":1,"sections":[{{"name":"T","body":"b"}}],"status":"active",{STAMP}}}"#
            )
        );
    }

    #[test]
    fn every_field_is_written_in_go_order() {
        let mut frontmatter = BTreeMap::new();
        frontmatter.insert("z".to_owned(), json!("<tag>"));
        frontmatter.insert("a".to_owned(), json!({"n": 1.5, "l": [null, true]}));
        let prompt = Prompt {
            id: "luuna-beacon-18a3".to_owned(),
            name: "full".to_owned(),
            description: "d & e".to_owned(),
            version: 7,
            sections: Some(vec![
                Section {
                    name: "A".to_owned(),
                    body: "line\nnext".to_owned(),
                    required: Some(false),
                },
                Section {
                    name: "B".to_owned(),
                    body: String::new(),
                    required: None,
                },
            ]),
            extends: "base".to_owned(),
            mixins: vec!["m1".to_owned(), "m2".to_owned()],
            tags: vec!["t".to_owned()],
            schema: "s".to_owned(),
            emit_as: "x.md".to_owned(),
            emit_dir: "out".to_owned(),
            pinned: Some(0),
            frontmatter,
            atlas: Some(Atlas {
                prime: Some(AtlasPrime::default()),
                budget: Some(0.0),
                on_empty: "skip".to_owned(),
            }),
            extends_atlas: Some(false),
            status: "draft".to_owned(),
            created_at: "2024-01-01T00:00:00Z".to_owned(),
            updated_at: "2024-01-02T00:00:00.5Z".to_owned(),
        };
        let line = prompt.to_go_json();
        assert_eq!(
            line,
            concat!(
                r#"{"id":"luuna-beacon-18a3","name":"full","description":"d \u0026 e","version":7,"#,
                r#""sections":[{"name":"A","body":"line\nnext","required":false},{"name":"B","body":""}],"#,
                r#""extends":"base","mixins":["m1","m2"],"tags":["t"],"schema":"s","emitAs":"x.md","#,
                r#""emitDir":"out","pinned":0,"frontmatter":{"a":{"l":[null,true],"n":1.5},"z":"\u003ctag\u003e"},"#,
                r#""atlas":{"prime":{},"budget":0,"on_empty":"skip"},"extends_atlas":false,"#,
                r#""status":"draft","createdAt":"2024-01-01T00:00:00Z","updatedAt":"2024-01-02T00:00:00.5Z"}"#
            )
        );
        assert_eq!(Prompt::from_go_json(line.as_bytes()).unwrap(), prompt);

        let schema = Schema {
            id: "schema-1".to_owned(),
            name: "s".to_owned(),
            required_sections: Some(vec!["A".to_owned()]),
            optional_sections: vec!["B".to_owned()],
            rules: vec![Rule {
                section: "A".to_owned(),
                pattern: "^<x>$".to_owned(),
                message: "m".to_owned(),
            }],
            created_at: "c".to_owned(),
            updated_at: "u".to_owned(),
        };
        let line = schema.to_go_json();
        assert_eq!(
            line,
            r#"{"id":"schema-1","name":"s","requiredSections":["A"],"optionalSections":["B"],"rules":[{"section":"A","pattern":"^\u003cx\u003e$","message":"m"}],"createdAt":"c","updatedAt":"u"}"#
        );
        assert_eq!(Schema::from_go_json(line.as_bytes()).unwrap(), schema);
    }
}
