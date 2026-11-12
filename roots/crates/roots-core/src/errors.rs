//! Typed errors and stable process exit codes for the Roots compatibility API.

use serde_json::{Map, Value};
use std::fmt::{Display, Formatter};

pub const EXIT_OK: i32 = 0;
pub const EXIT_ERROR: i32 = 1;
pub const EXIT_USAGE: i32 = 2;
pub const EXIT_NOT_FOUND: i32 = 3;
pub const EXIT_GUARD: i32 = 4;
pub const EXIT_CONFLICT: i32 = 5;
pub const EXIT_VALIDATION: i32 = 6;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ErrorCode {
    Error,
    Cancelled,
    Usage,
    Ambiguous,
    NotFound,
    NotInitialized,
    Guard,
    Conflict,
    Validation,
    Config,
}

impl ErrorCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Cancelled => "cancelled",
            Self::Usage => "usage",
            Self::Ambiguous => "ambiguous",
            Self::NotFound => "not_found",
            Self::NotInitialized => "not_initialized",
            Self::Guard => "guard",
            Self::Conflict => "conflict",
            Self::Validation => "validation",
            Self::Config => "config",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RootsError {
    pub message: String,
    pub code: ErrorCode,
    pub exit_code: i32,
    pub detail: Option<Map<String, Value>>,
}

pub type RootsResult<T> = Result<T, RootsError>;

impl RootsError {
    pub fn new(message: impl Into<String>, code: ErrorCode, exit_code: i32) -> Self {
        Self {
            message: message.into(),
            code,
            exit_code,
            detail: None,
        }
    }

    pub fn with_detail(mut self, detail: Map<String, Value>) -> Self {
        self.detail = Some(detail);
        self
    }

    pub fn error(message: impl Into<String>) -> Self {
        Self::new(message, ErrorCode::Error, EXIT_ERROR)
    }

    pub fn cancelled(message: impl Into<String>) -> Self {
        Self::new(message, ErrorCode::Cancelled, EXIT_ERROR)
    }

    pub fn usage(message: impl Into<String>) -> Self {
        Self::new(message, ErrorCode::Usage, EXIT_USAGE)
    }

    pub fn ambiguous(message: impl Into<String>, candidates: Vec<String>) -> Self {
        let mut detail = Map::new();
        detail.insert(
            "candidates".to_owned(),
            Value::Array(candidates.into_iter().map(Value::String).collect()),
        );
        Self::new(message, ErrorCode::Ambiguous, EXIT_USAGE).with_detail(detail)
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(message, ErrorCode::NotFound, EXIT_NOT_FOUND)
    }

    pub fn not_initialized(cwd: impl Display) -> Self {
        Self::new(
            format!(
                "not a roots project (no .luuna/roots/ found from {cwd}); run `luuna-roots init`"
            ),
            ErrorCode::NotInitialized,
            EXIT_NOT_FOUND,
        )
    }

    pub fn guard(message: impl Into<String>) -> Self {
        Self::new(message, ErrorCode::Guard, EXIT_GUARD)
    }

    pub fn conflict(message: impl Into<String>) -> Self {
        Self::new(message, ErrorCode::Conflict, EXIT_CONFLICT)
    }

    pub fn validation(message: impl Into<String>) -> Self {
        Self::new(message, ErrorCode::Validation, EXIT_VALIDATION)
    }

    pub fn config(message: impl Into<String>) -> Self {
        Self::new(message, ErrorCode::Config, EXIT_VALIDATION)
    }
}

impl Display for RootsError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for RootsError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ambiguous_errors_include_candidates() {
        let error = RootsError::ambiguous("ambiguous", vec!["r-a".to_owned()]);
        assert_eq!(error.code, ErrorCode::Ambiguous);
        assert_eq!(error.exit_code, EXIT_USAGE);
        assert_eq!(
            error.detail.unwrap()["candidates"],
            Value::Array(vec![Value::String("r-a".to_owned())])
        );
    }
}
