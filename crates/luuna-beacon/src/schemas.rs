//! Schemas (`schema create|list|show|rule add`, `cli.go:923-1014`) and
//! `validate` (`cli.go:1497-1571`).
//!
//! As in Go:
//!
//! - the current schema of an id is its record with the greatest `updatedAt`
//!   compared as text (GD2), the earlier record on a tie;
//! - validate checks a prompt against every stored record whose name is the
//!   prompt's schema, not only the current one, so errors repeat once per
//!   schema record (GD3);
//! - a stored rule whose pattern does not compile is skipped (GD4);
//! - the named form resolves the prompt over the whole log (GD5), while
//!   `--all` takes the current records that name a schema, unresolved;
//! - rule patterns have Go's `regexp` meaning (GD6, `re2.rs`).
//!
//! Go's list order comes from map iteration and is unspecified; lists here
//! are sorted (AS6). Where Go would pick an arbitrary one of several current
//! schemas with the same name, the one first in that order is used.

use crate::beacon::Beacon;
use crate::error::BeaconError;
use crate::model::{Prompt, Rule, Schema, Section};
use crate::payload::Validation;
use crate::prompts::current;
use crate::resolve::resolve;
use crate::{paths, re2, store};
use regex::Regex;
use std::collections::HashMap;
use std::path::PathBuf;

/// The input of [`Beacon::schema_create`]. Mirrors `--name`, `--required a,b`
/// and `--optional a,b`: each list is joined with commas and then split as
/// Go splits the flag value, so an empty `required` is stored as `[]` and an
/// empty `optional` is left out.
#[derive(Debug, Clone, Default)]
pub struct CreateSchema {
    pub name: String,
    pub required: Vec<String>,
    pub optional: Vec<String>,
}

/// The current record of each schema id, sorted by name, then id.
fn current_schemas(records: Vec<Schema>) -> Vec<Schema> {
    let mut by_id: Vec<Schema> = Vec::new();
    let mut slot: HashMap<String, usize> = HashMap::new();
    for schema in records {
        match slot.get(&schema.id) {
            Some(&index) => {
                if schema.updated_at > by_id[index].updated_at {
                    by_id[index] = schema;
                }
            }
            None => {
                slot.insert(schema.id.clone(), by_id.len());
                by_id.push(schema);
            }
        }
    }
    by_id.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.id.cmp(&b.id)));
    by_id
}

/// The errors of one prompt (Go's loop at `cli.go:1534-1559`).
fn errors_for(
    sections: &[Section],
    schema_name: &str,
    schemas: &[Schema],
    compiled: &mut HashMap<String, Option<Regex>>,
) -> Vec<String> {
    let mut errors = Vec::new();
    for schema in schemas.iter().filter(|schema| schema.name == schema_name) {
        for required in schema.required_sections.as_deref().unwrap_or_default() {
            if !sections.iter().any(|section| &section.name == required) {
                errors.push(format!("Required section \"{required}\" is missing"));
            }
        }
        for rule in &schema.rules {
            for section in sections.iter().filter(|s| s.name == rule.section) {
                let expression = compiled
                    .entry(rule.pattern.clone())
                    .or_insert_with(|| re2::compile(&rule.pattern).ok());
                if let Some(expression) = expression {
                    if !expression.is_match(&section.body) {
                        errors.push(rule.message.clone());
                    }
                }
            }
        }
    }
    errors
}

fn validation(
    prompt: &Prompt,
    schemas: &[Schema],
    compiled: &mut HashMap<String, Option<Regex>>,
) -> Validation {
    let sections = prompt.sections.as_deref().unwrap_or_default();
    let errors = errors_for(sections, &prompt.schema, schemas, compiled);
    Validation {
        name: prompt.name.clone(),
        valid: errors.is_empty(),
        errors,
        warnings: Vec::new(),
    }
}

impl Beacon {
    /// `paths.Schemas`: runs the legacy move, then names the schemas file.
    fn schemas_path(&self) -> Result<PathBuf, BeaconError> {
        self.migrate_legacy()?;
        Ok(paths::schemas_file(self.project_root()))
    }

    /// Reads the schemas file, as every schema command does first.
    fn schema_records(&self) -> Result<(PathBuf, Vec<Schema>), BeaconError> {
        let path = self.schemas_path()?;
        let records = store::read_jsonl(&path)?;
        Ok((path, records))
    }

    /// Go's `schema create`. The id is `schema-<hex of the clock's
    /// nanoseconds>`.
    pub fn schema_create(&self, input: &CreateSchema) -> Result<Schema, BeaconError> {
        let (path, records) = self.schema_records()?;
        if input.name.is_empty() {
            return Err(BeaconError::NameRequired);
        }
        if current_schemas(records)
            .iter()
            .any(|schema| schema.name == input.name)
        {
            return Err(BeaconError::SchemaNameExists(input.name.clone()));
        }
        let now = self.now_text();
        let required_text = input.required.join(",");
        let required: Vec<String> = if required_text.is_empty() {
            Vec::new()
        } else {
            required_text.split(',').map(str::to_owned).collect()
        };
        let optional_text = input.optional.join(",");
        let mut optional: Vec<String> = optional_text
            .trim_matches(',')
            .split(',')
            .map(str::to_owned)
            .collect();
        if optional.len() == 1 && optional[0].is_empty() {
            optional.clear();
        }
        let schema = Schema {
            id: format!("schema-{}", self.next_id_hex()),
            name: input.name.clone(),
            required_sections: Some(required),
            optional_sections: optional,
            rules: Vec::new(),
            created_at: now.clone(),
            updated_at: now,
        };
        store::append_jsonl(&path, &schema)?;
        Ok(schema)
    }

    /// Go's `schema list`: the current record of every schema, sorted by
    /// name, then id.
    pub fn schema_list(&self) -> Result<Vec<Schema>, BeaconError> {
        let (_, records) = self.schema_records()?;
        Ok(current_schemas(records))
    }

    /// Go's `schema show <name>`.
    pub fn schema_show(&self, name: &str) -> Result<Schema, BeaconError> {
        let (_, records) = self.schema_records()?;
        current_schemas(records)
            .into_iter()
            .find(|schema| schema.name == name)
            .ok_or_else(|| BeaconError::SchemaNotFound(name.to_owned()))
    }

    /// Go's `schema rule add <schema>`: checks the three fields, then the
    /// pattern (with Go's `regexp` grammar), then looks the schema up, and
    /// appends a new record of it with the rule added and `updatedAt` set.
    pub fn schema_rule_add(&self, schema: &str, rule: &Rule) -> Result<Schema, BeaconError> {
        let (path, records) = self.schema_records()?;
        if rule.section.is_empty() || rule.pattern.is_empty() || rule.message.is_empty() {
            return Err(BeaconError::RuleFieldsRequired);
        }
        re2::compile(&rule.pattern).map_err(BeaconError::InvalidRulePattern)?;
        let Some(mut target) = current_schemas(records)
            .into_iter()
            .find(|candidate| candidate.name == schema)
        else {
            return Err(BeaconError::SchemaNotFound(schema.to_owned()));
        };
        target.rules.push(rule.clone());
        target.updated_at = self.now_text();
        store::append_jsonl(&path, &target)?;
        Ok(target)
    }

    /// Reads the prompts and then the schemas, as `validate` does.
    fn validate_inputs(&self) -> Result<(Vec<Prompt>, Vec<Schema>), BeaconError> {
        let prompts_path = self.prompts_path()?;
        let prompts: Vec<Prompt> = store::read_jsonl(&prompts_path)?;
        let (_, schemas) = self.schema_records()?;
        Ok((prompts, schemas))
    }

    /// Go's `validate <name>`: resolves the prompt over every stored record
    /// (GD5) and checks the result against its schema. An invalid prompt is
    /// `Ok` with `valid == false`; Go's `validation failed` exit belongs to
    /// the caller. [`Validation::to_named_payload`] is the `--json` document.
    pub fn validate(&self, name: &str) -> Result<Validation, BeaconError> {
        let (prompts, schemas) = self.validate_inputs()?;
        let rendered = resolve(prompts, name, None)?;
        Ok(validation(&rendered.prompt, &schemas, &mut HashMap::new()))
    }

    /// Go's `validate --all`: every current record that names a schema,
    /// checked without resolving inheritance, sorted by name.
    pub fn validate_all(&self) -> Result<Vec<Validation>, BeaconError> {
        let (prompts, schemas) = self.validate_inputs()?;
        let mut compiled = HashMap::new();
        let mut results: Vec<Validation> = current(prompts)
            .iter()
            .filter(|prompt| !prompt.schema.is_empty())
            .map(|prompt| validation(prompt, &schemas, &mut compiled))
            .collect();
        results.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn schema(id: &str, name: &str, updated_at: &str, rules: &[(&str, &str, &str)]) -> Schema {
        Schema {
            id: id.to_owned(),
            name: name.to_owned(),
            required_sections: Some(vec!["Task".to_owned()]),
            rules: rules
                .iter()
                .map(|(section, pattern, message)| Rule {
                    section: (*section).to_owned(),
                    pattern: (*pattern).to_owned(),
                    message: (*message).to_owned(),
                })
                .collect(),
            updated_at: updated_at.to_owned(),
            ..Schema::default()
        }
    }

    fn section(name: &str, body: &str) -> Section {
        Section {
            name: name.to_owned(),
            body: body.to_owned(),
            required: None,
        }
    }

    #[test]
    fn current_schema_is_the_greatest_updated_at_text() {
        let records = vec![
            schema("b", "second", "2024-01-01T00:00:00.12Z", &[]),
            schema("a", "first", "1", &[]),
            schema("b", "second-renamed", "2024-01-01T00:00:00.1Z", &[]),
            schema("a", "first-tie", "1", &[]),
        ];
        let names: Vec<String> = current_schemas(records)
            .into_iter()
            .map(|schema| schema.name)
            .collect();
        assert_eq!(names, ["first", "second-renamed"]);
    }

    #[test]
    fn every_schema_record_is_checked_and_bad_rules_are_skipped() {
        let schemas = vec![
            schema("s", "g", "1", &[("Task", "^do", "start")]),
            schema(
                "s",
                "g",
                "2",
                &[("Task", "^do", "start"), ("Task", "(", "bad")],
            ),
            schema("t", "other", "1", &[]),
        ];
        let mut compiled = HashMap::new();
        let errors = errors_for(&[section("Other", "x")], "g", &schemas, &mut compiled);
        assert_eq!(
            errors,
            [
                "Required section \"Task\" is missing",
                "Required section \"Task\" is missing"
            ]
        );
        let errors = errors_for(
            &[section("Task", "nope"), section("Task", "do")],
            "g",
            &schemas,
            &mut compiled,
        );
        assert_eq!(errors, ["start", "start"]);
        assert!(errors_for(&[], "", &schemas, &mut compiled).is_empty());
    }
}
