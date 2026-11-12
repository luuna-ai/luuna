//! Errors returned by the library.
//!
//! A failure that the Go tool reports to its user (a missing ticket, an
//! invalid option, a refused claim) is not an [`Error`]: it is an
//! [`Outcome`](crate::command::Outcome) with Go's output and exit code.
//! [`Error`] is reserved for conditions the caller must handle itself, above
//! all behaviour this crate does not port yet, so that it never silently
//! differs from the Go tool.

use std::path::PathBuf;

/// Result type of this crate.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Errors returned by the library.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The behaviour exists in the Go tool but is not ported yet. `plan`
    /// names the plan that ports it.
    #[error("{feature} is not supported yet; it is ported in {plan}")]
    Unsupported {
        /// What was asked for.
        feature: String,
        /// The plan that ports it.
        plan: String,
    },
    /// A file system operation failed.
    #[error("{operation} {}: {source}", path.display())]
    Io {
        /// What was being done.
        operation: &'static str,
        /// The path involved.
        path: PathBuf,
        /// The underlying error.
        #[source]
        source: std::io::Error,
    },
}

impl Error {
    /// An [`Error::Unsupported`] for `feature`, ported in `plan`.
    pub fn unsupported(feature: impl Into<String>, plan: impl Into<String>) -> Self {
        Self::Unsupported {
            feature: feature.into(),
            plan: plan.into(),
        }
    }
}

/// The plan that ports the behaviour stubbed in this crate today.
pub(crate) const THIS_PLAN: &str = "s4-missions-lib";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsupported_names_the_plan() {
        let error = Error::unsupported("plan show", "s4-missions-lib-4");
        assert_eq!(
            error.to_string(),
            "plan show is not supported yet; it is ported in s4-missions-lib-4"
        );
    }
}
