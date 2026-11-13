//! Owner: plan s3-beacon-lib-2.
//!
//! This folder will hold the authoring commands: markdown render with
//! yaml.v3-style frontmatter, emit, import, pin, unpin, diff, tree, stats and
//! config show, set, unset and schema.
//!
//! The plan declares its submodules here, re-exports its public items here
//! and adds its operations as `impl Beacon` blocks in this folder. Public
//! names differ from every name the crate root already exports and from the
//! other later plan's names; a clash between the two glob re-exports fails
//! clippy (ambiguous_glob_reexports).

mod config_edit;
mod diff;
mod emit;
mod import_md;
mod pin;
mod stats;
mod tree;
pub use diff::{ChangeKind, SectionChange};
pub use import_md::{ImportMarkdown, Imported};
pub use stats::PromptStats;
pub use tree::PromptTree;
mod gopath;
mod markdown;
pub use emit::{EmitOptions, EmitReport, EmitSelection, EmittedFile};
mod project_config;
mod unicode_data;
mod yaml_decode;
mod yaml_encode;
mod yaml_events;
mod yaml_gofmt;
mod yaml_json;
mod yaml_resolve;
mod yaml_sort;
mod yaml_value;
pub use config_edit::{config_schema, ConfigShow};
pub use markdown::render_markdown;
pub use project_config::{EmitTarget, ProjectConfig};
pub use yaml_value::YamlValue;

/// Errors reported by authoring operations.
#[derive(Debug, thiserror::Error)]
pub enum AuthoringError {
    #[error("expected name@version")]
    ExpectedNameAtVersion,
    #[error("version {0} not found")]
    VersionNotFound(i64),
    #[error("prompt versions not found")]
    PromptVersionsNotFound,
    #[error("path not found: {0}")]
    PathNotFound(String),
    #[error("config path must be non-empty")]
    ConfigPathEmpty,
    #[error("cannot set: {} is not an object", crate::gofmt::quote(.0))]
    NotAnObject(String),
    #[error("config validation failed: {0}")]
    ConfigValidation(String),
    #[error("invalid frontmatter: {0}")]
    InvalidFrontmatter(String),
    #[error("{0} stale file(s)")]
    StaleFiles(usize),
    #[error("json: unsupported value: {0}")]
    JsonUnsupportedValue(String),
    #[error("json: unsupported type: {0}")]
    JsonUnsupportedType(String),
    #[error("panic: {0}")]
    GoPanic(String),
    #[error("refused: {0} holds more than one YAML document and the rewrite would keep only the first; config.yaml was not changed")]
    ConfigLaterDocuments(&'static str),
    #[error("refused: config.yaml has a null top-level key that the rewrite would drop; config.yaml was not changed")]
    ConfigNullKey,
    #[error("refused: after the rewrite the value at {0} would read back as a different value; config.yaml was not changed")]
    ConfigReadBack(String),
    #[error("refused: the frontmatter block is not in the form the Go tool reads (a byte order mark, CRLF line ends or an empty block) and would not be stored as frontmatter; nothing was stored")]
    ImportUnrecognisedFrontmatter,
    #[error("refused: text before the first \"## \" heading would be dropped; nothing was stored")]
    ImportTextBeforeFirstHeading,
    #[error("refused: the frontmatter description is not a string and would be dropped; nothing was stored")]
    ImportDescriptionNotString,
    #[error("refused: a frontmatter key is null and would be dropped; nothing was stored")]
    ImportNullFrontmatterKey,
    #[error("refused: the frontmatter holds more than one YAML document and only the first would be stored; nothing was stored")]
    ImportLaterFrontmatterDocuments,
    #[error("refused: the frontmatter name {} differs from the name given and would be dropped; nothing was stored", crate::gofmt::quote(.0))]
    ImportFrontmatterNameDiffers(String),
    #[error("refused: the frontmatter value at {0} would read back from the store as a different value; nothing was stored")]
    ImportReadBack(String),
    #[error("refused: emit would overwrite {0} inside .luuna/ with a rendered prompt; nothing was written")]
    EmitIntoStore(String),
    #[error("refused: the current record of {} holds the key {} that the Go tool does not read, and the new version would drop it; prompts.jsonl was not changed", crate::gofmt::quote(.0), crate::gofmt::quote(.1))]
    PinRecordKeyUnknown(String, String),
    #[error("refused: the current record of {} holds the key {} more than once, and the new version would keep only the last value; prompts.jsonl was not changed", crate::gofmt::quote(.0), crate::gofmt::quote(.1))]
    PinRecordKeyRepeated(String, String),
}
impl From<AuthoringError> for crate::BeaconError {
    fn from(value: AuthoringError) -> Self {
        Self::Authoring(value)
    }
}

#[cfg(test)]
mod yaml_tests;
