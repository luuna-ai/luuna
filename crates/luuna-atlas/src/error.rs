//! The crate's error type.
//!
//! Messages that the Go tool shows to users are carried verbatim, so that a command's
//! failure envelope matches the Go tool byte for byte.

use std::path::PathBuf;

/// Everything that can go wrong in this crate.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum AtlasError {
    /// The operation is declared but not yet ported. Later tasks of plan s5-atlas-lib
    /// replace every use of this variant.
    #[error("luuna-atlas (Rust port): {feature} is not implemented yet")]
    NotImplemented {
        /// What is missing, for example `json::parse`.
        feature: &'static str,
    },

    /// A failure whose text is the Go tool's message, shown to users unchanged.
    #[error("{0}")]
    Message(String),

    /// An I/O failure on a path inside the store.
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// YAML the crate's reader does not support (anchors, aliases, merge keys, tags,
    /// several documents). The reader refuses instead of guessing.
    #[error("unsupported YAML at line {line}: {message}")]
    UnsupportedYaml { line: usize, message: String },
}

impl AtlasError {
    /// The error for an operation that a later task of this plan implements.
    pub(crate) fn not_implemented(feature: &'static str) -> Self {
        AtlasError::NotImplemented { feature }
    }
}

/// Result alias used throughout the crate.
pub type Result<T> = std::result::Result<T, AtlasError>;
