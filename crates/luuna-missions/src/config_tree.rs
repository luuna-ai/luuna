//! The full `config.yaml` and the legacy `.seeds` migration, owned by
//! s4-missions-lib-3 (`internal/config/tree.go`, `internal/config/legacy.go`).
//!
//! At this commit every function here returns exactly what the foundation
//! returns: [`read_outside_subset`] and [`migrate_legacy_layout`] refuse with
//! [`Error::Unsupported`] naming s4-missions-lib-3, and
//! [`with_stderr_notices`] runs the command unchanged. s4-missions-lib-3
//! replaces their bodies and the `checks` it owns, and edits neither
//! `config.rs` nor `command/init.rs`.

use std::path::Path;

use crate::command::Outcome;
use crate::config::{Config, LEGACY_DIR};
use crate::error::{Error, Result, PLAN_3};

/// Runs one command and adds to its stderr the notices Go prints to
/// `os.Stderr` while the command runs (such as the line of the legacy
/// migration, which Go's `FindDir` prints for every command). No notice is
/// produced at this commit, so the command's result is returned unchanged.
pub(crate) fn with_stderr_notices(run: impl FnOnce() -> Result<Outcome>) -> Result<Outcome> {
    run()
}

/// Go's `ReadConfig` for a `config.yaml` at `path` that the foundation's
/// subset reader does not read; `reason` says why. Refused until
/// s4-missions-lib-3.
pub(crate) fn read_outside_subset(path: &Path, _content: &[u8], reason: String) -> Result<Config> {
    Err(Error::unsupported(
        format!(
            "reading {} outside the config.yaml subset Go writes ({reason})",
            path.display()
        ),
        PLAN_3,
    ))
}

/// Go's `MigrateLegacyLayout` for the `.seeds` entry `legacy` in
/// `project_root`, in every case where Go migrates it or refuses. Refused
/// until s4-missions-lib-3.
pub(crate) fn migrate_legacy_layout(_project_root: &Path, legacy: &Path) -> Result<()> {
    Err(Error::unsupported(
        format!(
            "the legacy {LEGACY_DIR}/ layout (found {})",
            legacy.display()
        ),
        PLAN_3,
    ))
}

/// The expectations of the unit tests in `config.rs` and `command/init.rs`
/// for what this module ports. Their inputs stay in those tests. While the
/// behaviour is deferred, each function asserts the refusal exactly as the
/// foundation made it; once s4-missions-lib-3 ports it, Go's exact result
/// for the same input, as literals.
#[cfg(test)]
pub(crate) mod checks {
    use std::fs;
    use std::path::Path;

    use crate::command::{run, Invocation};
    use crate::config::{find_dir, read_config, ProcessEnv, LEGACY_DIR};
    use crate::error::{Error, PLAN_3};

    /// `read_config(missions_dir)`, whose `config.yaml` holds
    /// `project:\n  nested: x\n`: Unsupported naming s4-missions-lib-3.
    pub(crate) fn outside_subset(missions_dir: &Path) {
        let error = read_config(missions_dir).unwrap_err();
        assert!(
            matches!(&error, Error::Unsupported { plan, .. } if plan == PLAN_3),
            "{error}"
        );
    }

    /// Discovery from `start`, with a `.seeds` entry in `root` that Go
    /// migrates or refuses: Unsupported naming s4-missions-lib-3 with a
    /// feature naming `.seeds`, and `root/.seeds` is still there with the
    /// same file type.
    pub(crate) fn legacy_found(root: &Path, start: &Path) {
        let before = fs::symlink_metadata(root.join(LEGACY_DIR))
            .unwrap()
            .file_type();
        match find_dir(start, ProcessEnv::Inherit) {
            Err(Error::Unsupported { plan, feature }) => {
                assert_eq!(plan, PLAN_3);
                assert!(feature.contains(".seeds"), "{feature}");
            }
            other => panic!("expected Unsupported, got {other:?}"),
        }
        let after = fs::symlink_metadata(root.join(LEGACY_DIR))
            .expect("nothing was moved")
            .file_type();
        assert_eq!(after, before, "nothing was moved");
        if before.is_dir() {
            assert!(root.join(LEGACY_DIR).is_dir(), "nothing was moved");
        }
    }

    /// `init` in `root`, which holds a `.seeds` directory: Unsupported naming
    /// s4-missions-lib-3, and `root/.luuna` was not created.
    pub(crate) fn init_with_legacy_folder(root: &Path) {
        match run(&Invocation::new(root, ["init"])) {
            Err(Error::Unsupported { plan, .. }) => assert_eq!(plan, PLAN_3),
            other => panic!("expected Unsupported, got {other:?}"),
        }
        assert!(!root.join(".luuna").exists());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outcomes_without_notices_pass_through_unchanged() {
        let outcome = Outcome {
            stdout: b"out\n".to_vec(),
            stderr: b"err\n".to_vec(),
            exit_code: 3,
        };
        let passed = with_stderr_notices(|| Ok(outcome.clone())).unwrap();
        assert_eq!(passed, outcome);
        let error = with_stderr_notices(|| Err(Error::unsupported("thing", "later"))).unwrap_err();
        assert!(
            matches!(&error, Error::Unsupported { feature, plan } if feature == "thing" && plan == "later"),
            "{error}"
        );
        assert_eq!(
            error.to_string(),
            "thing is not supported yet; it is ported in later"
        );
    }
}
