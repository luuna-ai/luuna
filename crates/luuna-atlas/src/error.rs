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

    /// A failed file-system call, worded as the Go tool's `*os.PathError`:
    /// `<op> <path>: <reason>`, for example `open /p/d.jsonl: permission denied`.
    #[error("{op} {}: {}", path.display(), go_reason(source))]
    PathOp {
        /// The Go operation name: `open`, `write`, `mkdir`.
        op: &'static str,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// A failed rename, worded as the Go tool's `*os.LinkError`:
    /// `rename <from> <to>: <reason>`.
    #[error("rename {} {}: {}", from.display(), to.display(), go_reason(source))]
    Rename {
        from: PathBuf,
        to: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// YAML the crate's reader does not support (anchors, aliases, merge keys, tags,
    /// several documents). The reader refuses instead of guessing.
    #[error("unsupported YAML at line {line}: {message}")]
    UnsupportedYaml { line: usize, message: String },
}

/// The reason text Go prints for an operating-system error: the C library's message with
/// its first letter in lower case and without Rust's ` (os error N)` suffix, as in Go's
/// errno table on Linux (`no such file or directory`, `permission denied`). Errors that
/// carry no OS code print as Rust prints them.
fn go_reason(error: &std::io::Error) -> String {
    let text = error.to_string();
    let Some(code) = error.raw_os_error() else {
        return text;
    };
    let text = text
        .strip_suffix(&format!(" (os error {code})"))
        .unwrap_or(&text);
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_lowercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// Result alias used throughout the crate.
pub type Result<T> = std::result::Result<T, AtlasError>;
