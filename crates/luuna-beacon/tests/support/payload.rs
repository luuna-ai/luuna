//! Turns a library result into the JSON document(s) the Go tool prints with
//! `--json` for the same step, field for field as `cli.go` writes them
//! (reference commit 1c34973):
//!
//! - init `:192`, create `:249`, update `:368`, archive `:397`, list `:426`,
//!   show `:451`, history `:617`, render `:645`, validate `:1564-1570`,
//!   schema `:971-1008`;
//! - the error document `{success:false, command:args[0], error}` is `:69`.
//!
//! The two documents that carry the runtime fields are built by the library
//! itself (`Rendered::to_payload`, `Validation::to_named_payload`); every
//! other document is built here.

use luuna_beacon::{Init, Prompt, Rendered, Schema, Validation};
use serde_json::{json, Value};

/// What a library call returned, one variant per command.
pub enum Done {
    Init(Init),
    Created(Prompt),
    Updated(Prompt),
    Archived(Prompt),
    Listed(Vec<Prompt>),
    Shown(Prompt),
    History {
        /// The reference exactly as the step gave it (Go prints `args[0]`).
        reference: String,
        versions: Vec<Prompt>,
    },
    Rendered(Rendered),
    /// `render --format json`: the bare resolved record.
    RenderedBare(Rendered),
    Validated(Validation),
    ValidatedAll(Vec<Validation>),
    SchemaCreated(Schema),
    SchemaListed(Vec<Schema>),
    SchemaShown(Schema),
    RuleAdded(Schema),
    /// A step that calls no library function (a raw file step).
    Nothing,
}

fn parsed(line: String) -> Value {
    serde_json::from_str(&line).expect("to_go_json writes valid JSON")
}

/// A prompt as Go prints it inside a document: the compact line parsed back.
pub fn prompt_value(prompt: &Prompt) -> Value {
    parsed(prompt.to_go_json())
}

/// A schema as Go prints it inside a document.
pub fn schema_value(schema: &Schema) -> Value {
    parsed(schema.to_go_json())
}

/// `{success:false, command, error}` (cli.go:69). `command` is `args[0]`.
pub fn error_document(command: &str, error: &str) -> Value {
    json!({"success": false, "command": command, "error": error})
}

fn result_entry(validation: &Validation) -> Value {
    json!({
        "name": validation.name,
        "valid": validation.valid,
        "errors": validation.errors,
        "warnings": validation.warnings,
    })
}

/// The documents Go prints on success, and for an invalid validation the
/// result document followed by the `validation failed` error document.
pub fn documents(done: &Done) -> Vec<Value> {
    match done {
        Done::Init(init) => vec![json!({
            "success": true,
            "command": "init",
            "dir": init.dir.to_string_lossy(),
        })],
        Done::Created(prompt) => vec![json!({
            "success": true,
            "command": "create",
            "id": prompt.id,
            "name": prompt.name,
            "version": prompt.version,
            "prompt": prompt_value(prompt),
        })],
        Done::Updated(prompt) => vec![json!({
            "success": true,
            "command": "update",
            "name": prompt.name,
            "version": prompt.version,
            "prompt": prompt_value(prompt),
        })],
        Done::Archived(prompt) => vec![json!({
            "success": true,
            "command": "archive",
            "name": prompt.name,
            "version": prompt.version,
        })],
        Done::Listed(prompts) => vec![json!({
            "success": true,
            "command": "list",
            "prompts": prompts.iter().map(prompt_value).collect::<Vec<_>>(),
            "count": prompts.len(),
        })],
        Done::Shown(prompt) => vec![json!({
            "success": true,
            "command": "show",
            "prompt": prompt_value(prompt),
        })],
        Done::History {
            reference,
            versions,
        } => vec![json!({
            "success": true,
            "command": "history",
            "name": reference,
            "versions": versions.iter().map(prompt_value).collect::<Vec<_>>(),
        })],
        Done::Rendered(rendered) => vec![rendered.to_payload()],
        Done::RenderedBare(rendered) => vec![prompt_value(&rendered.prompt)],
        Done::Validated(validation) => {
            let mut docs = vec![validation.to_named_payload()];
            if !validation.valid {
                docs.push(error_document("validate", "validation failed"));
            }
            docs
        }
        Done::ValidatedAll(validations) => {
            let valid = validations.iter().all(|validation| validation.valid);
            let mut docs = vec![json!({
                "success": valid,
                "command": "validate",
                "results": validations.iter().map(result_entry).collect::<Vec<_>>(),
            })];
            if !valid {
                docs.push(error_document("validate", "validation failed"));
            }
            docs
        }
        Done::SchemaCreated(schema) => vec![json!({
            "success": true,
            "command": "schema create",
            "id": schema.id,
            "name": schema.name,
        })],
        Done::SchemaListed(schemas) => vec![json!({
            "success": true,
            "command": "schema list",
            "schemas": schemas.iter().map(schema_value).collect::<Vec<_>>(),
            "count": schemas.len(),
        })],
        Done::SchemaShown(schema) => vec![json!({
            "success": true,
            "command": "schema show",
            "schema": schema_value(schema),
        })],
        Done::RuleAdded(schema) => {
            let rule = schema.rules.last().expect("a rule was just added");
            vec![json!({
                "success": true,
                "command": "schema rule add",
                "schema": schema.name,
                "rule": {
                    "section": rule.section,
                    "pattern": rule.pattern,
                    "message": rule.message,
                },
            })]
        }
        Done::Nothing => Vec::new(),
    }
}

/// The exit status Go ends with for this result: 1 only for an invalid
/// validation (the error document is part of [`documents`]).
pub fn exit_code(done: &Done) -> i32 {
    match done {
        Done::Validated(validation) if !validation.valid => 1,
        Done::ValidatedAll(validations) if validations.iter().any(|v| !v.valid) => 1,
        _ => 0,
    }
}
