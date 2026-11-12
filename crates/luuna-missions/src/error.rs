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
    /// The lock of a data file could not be taken within 30 s. The message
    /// is Go's (`internal/store/store.go`, `WithLock`).
    #[error("Timeout acquiring lock for {}", data_path.display())]
    LockTimeout {
        /// The data file whose lock was wanted.
        data_path: PathBuf,
    },
    /// A record holds a value Go cannot encode, such as a number out of
    /// range. The message is Go's (`json: unsupported value: +Inf`).
    #[error("{message}")]
    Encode {
        /// Go's message.
        message: String,
    },
    /// Store discovery reached the file system root without finding
    /// `.luuna/missions/config.yaml`. The message is Go's
    /// (`internal/config/config.go:50`, `FindDir`).
    #[error("Not in a missions project. Run `luuna-missions init` first.")]
    NotInProject,
}

impl Error {
    /// An [`Error::Io`] for `operation` on `path`.
    pub(crate) fn io(
        operation: &'static str,
        path: impl Into<PathBuf>,
        source: std::io::Error,
    ) -> Self {
        Self::Io {
            operation,
            path: path.into(),
            source,
        }
    }
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

/// The plan that ports close, ready, blocked, stats, dep, label, unblock,
/// help and version output, unknown-command suggestions, the logging
/// variables and completions.
pub(crate) const PLAN_2: &str = "s4-missions-lib-2";
/// The plan that ports config and the full config.yaml, tpl, doctor, sync,
/// migrate-from-beads, the legacy `.seeds` migration, prime, onboard, setup,
/// guard and upgrade.
pub(crate) const PLAN_3: &str = "s4-missions-lib-3";
/// The plan that ports plans.
pub(crate) const PLAN_4: &str = "s4-missions-lib-4";

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
