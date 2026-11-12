//! Errors. Where the Go tool reports a condition with its own message, the
//! `Display` text of the variant is that message exactly, with Go's `%q`
//! quoting (`cli.go`, `paths.go`, `store.go`). Operating-system errors carry
//! the operation and path the way Go's `*PathError` does (`open <path>: `),
//! followed by the operating system's own text; YAML and JSON parse errors
//! carry their parser's text.

use crate::gofmt::quote;
use std::io;
use std::path::PathBuf;

/// An error from a Beacon operation.
#[derive(Debug, thiserror::Error)]
pub enum BeaconError {
    // ---- Library-only conditions (no Go equivalent) ----
    /// `Beacon::open` was given a relative project root.
    #[error("project root must be an absolute path: {}", .0.display())]
    RelativeRoot(PathBuf),

    // ---- Legacy layout (paths.go:34-75) ----
    #[error("refusing to migrate symlinked legacy directory")]
    SymlinkedLegacy,
    #[error("both .canopy and .luuna/beacon exist; merge or remove one before continuing")]
    BothLayouts,
    #[error("refusing to migrate into symlinked .luuna directory")]
    SymlinkedLuuna,

    // ---- Store files (store.go) ----
    #[error("timeout acquiring lock on {} after 5000ms", .path.display())]
    LockTimeout { path: PathBuf },
    #[error("acquire lock {}: open {}: {source}", .lock.display(), .lock.display())]
    AcquireLock { lock: PathBuf, source: io::Error },
    #[error("create temporary file: open {}: {source}", .pattern.display())]
    CreateTemp { pattern: PathBuf, source: io::Error },
    #[error("write temporary file: {op} {}: {source}", .path.display())]
    WriteTemp {
        op: &'static str,
        path: PathBuf,
        source: io::Error,
    },
    #[error("atomically replace {}: rename {} {}: {source}", .path.display(), .from.display(), .path.display())]
    Replace {
        path: PathBuf,
        from: PathBuf,
        source: io::Error,
    },
    #[error("invalid JSONL line {line} in {}: {message}", .path.display())]
    InvalidJsonl {
        line: usize,
        path: PathBuf,
        message: String,
    },

    // ---- Operating-system errors, as Go's *PathError and *LinkError ----
    #[error("{op} {}: {source}", .path.display())]
    Os {
        op: &'static str,
        path: PathBuf,
        source: io::Error,
    },
    #[error("rename {} {}: {source}", .from.display(), .to.display())]
    Rename {
        from: PathBuf,
        to: PathBuf,
        source: io::Error,
    },

    // ---- config.yaml (cli.go:1744-1763) ----
    /// config.yaml is not valid YAML, or its top level is not a mapping.
    #[error("yaml: {0}")]
    Yaml(String),

    // ---- Commands (cli.go:144-1014, 1497-1571, 2055-2077) ----
    #[error(".luuna/beacon/ already exists")]
    AlreadyInitialized,
    #[error("--name is required")]
    NameRequired,
    #[error("prompt name required")]
    PromptNameRequired,
    #[error("prompt name {} already exists", quote(.0))]
    PromptNameExists(String),
    #[error("parent prompt {} not found", quote(.0))]
    ParentNotFound(String),
    #[error("mixin prompt {} not found", quote(.0))]
    MixinNotFound(String),
    #[error("invalid status: {0}")]
    InvalidStatus(String),
    #[error("prompt {} is already archived", quote(.0))]
    AlreadyArchived(String),
    #[error("prompt {} not found", quote(.0))]
    PromptNotFound(String),
    #[error("maximum inheritance depth exceeded")]
    MaxInheritanceDepth,
    #[error("circular inheritance involving {}", quote(.0))]
    CircularInheritance(String),
    #[error("invalid limit: {0}")]
    InvalidLimit(String),
    #[error("invalid version: {0}")]
    InvalidVersion(String),
    #[error("prompt name or --all required")]
    PromptNameOrAllRequired,
    #[error("validation failed")]
    ValidationFailed,
    #[error("schema name {} already exists", quote(.0))]
    SchemaNameExists(String),
    #[error("schema name required")]
    SchemaNameRequired,
    #[error("schema {} not found", quote(.0))]
    SchemaNotFound(String),
    #[error("--section, --pattern, and --message are required")]
    RuleFieldsRequired,
    /// The pattern does not compile; the text after the prefix is the
    /// pattern compiler's own.
    #[error("invalid rule pattern: {0}")]
    InvalidRulePattern(String),
}

impl BeaconError {
    /// An operating-system error on `path` during `op` (`open`, `read`,
    /// `lstat`, `mkdir`, ...), shown as Go shows a `*PathError`.
    pub(crate) fn os(op: &'static str, path: impl Into<PathBuf>, source: io::Error) -> Self {
        BeaconError::Os {
            op,
            path: path.into(),
            source,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_match_go() {
        let cases: Vec<(BeaconError, &str)> = vec![
            (
                BeaconError::PromptNotFound("a\"b@2".to_owned()),
                r#"prompt "a\"b@2" not found"#,
            ),
            (
                BeaconError::PromptNameExists("x\ny".to_owned()),
                r#"prompt name "x\ny" already exists"#,
            ),
            (
                BeaconError::CircularInheritance("c".to_owned()),
                r#"circular inheritance involving "c""#,
            ),
            (
                BeaconError::InvalidStatus("gone".to_owned()),
                "invalid status: gone",
            ),
            (
                BeaconError::InvalidVersion("x".to_owned()),
                "invalid version: x",
            ),
            (
                BeaconError::LockTimeout {
                    path: PathBuf::from("/p/.luuna/beacon/prompts.jsonl"),
                },
                "timeout acquiring lock on /p/.luuna/beacon/prompts.jsonl after 5000ms",
            ),
            (
                BeaconError::InvalidJsonl {
                    line: 3,
                    path: PathBuf::from("/p/.luuna/beacon/schemas.jsonl"),
                    message: "unexpected end of JSON input".to_owned(),
                },
                "invalid JSONL line 3 in /p/.luuna/beacon/schemas.jsonl: unexpected end of JSON input",
            ),
            (
                BeaconError::BothLayouts,
                "both .canopy and .luuna/beacon exist; merge or remove one before continuing",
            ),
        ];
        for (error, expected) in cases {
            assert_eq!(error.to_string(), expected);
        }
        let os = BeaconError::os(
            "open",
            "/p/.luuna/beacon/config.yaml",
            io::Error::from(io::ErrorKind::NotFound),
        );
        assert!(os
            .to_string()
            .starts_with("open /p/.luuna/beacon/config.yaml: "));
    }
}
