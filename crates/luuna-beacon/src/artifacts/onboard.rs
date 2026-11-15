//! `onboard` (`cli.go:1339-1399`): the Beacon block in the project's agent
//! instructions file.
//!
//! It never runs the legacy move (GD70). Go discards the error of reading
//! the target and then writes the snippet over the whole file; the library
//! refuses to do that in Apply mode (RD55).

use super::ArtifactsError;
use crate::paths;
use crate::{Beacon, BeaconError};
use std::fs;
use std::io;
use std::path::PathBuf;

const START: &str = "<!-- luuna-beacon:start -->";
const END: &str = "<!-- luuna-beacon:end -->";
const VERSION: &str = "<!-- luuna-beacon-onboard-v:3 -->";

/// The block `onboard` writes (`cli.go:1351-1352`).
pub const ONBOARD_SNIPPET: &str = "<!-- luuna-beacon:start -->\n## Prompt Management (Luuna Beacon)\n<!-- luuna-beacon-onboard-v:3 -->\n\nRun `lbc prime` at the start of every session. Use `lbc update` then `lbc emit`; do not edit emitted files.\n<!-- luuna-beacon:end -->\n";

/// What `onboard` does: `--stdout` is [`OnboardMode::Stdout`], `--check` is
/// [`OnboardMode::Check`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnboardMode {
    Apply,
    Check,
    Stdout,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnboardStatus {
    Missing,
    Outdated,
    Current,
}

impl OnboardStatus {
    /// The word Go prints.
    pub fn as_str(self) -> &'static str {
        match self {
            OnboardStatus::Missing => "missing",
            OnboardStatus::Outdated => "outdated",
            OnboardStatus::Current => "current",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnboardAction {
    Created,
    Appended,
    Updated,
    Unchanged,
}

impl OnboardAction {
    /// The word Go prints.
    pub fn as_str(self) -> &'static str {
        match self {
            OnboardAction::Created => "created",
            OnboardAction::Appended => "appended",
            OnboardAction::Updated => "updated",
            OnboardAction::Unchanged => "unchanged",
        }
    }
}

/// The result of `onboard`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Onboarded {
    Snippet(String),
    Checked {
        file: PathBuf,
        status: OnboardStatus,
    },
    Applied {
        file: PathBuf,
        action: OnboardAction,
    },
}

/// Go's `strings.Index` on bytes.
fn find(haystack: &[u8], needle: &str) -> Option<usize> {
    let needle = needle.as_bytes();
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn contains(haystack: &[u8], needle: &str) -> bool {
    find(haystack, needle).is_some()
}

impl Beacon {
    /// Go's `onboard`.
    pub fn onboard(&self, mode: OnboardMode) -> Result<Onboarded, BeaconError> {
        let root = self.project_root();
        let candidates = [
            root.join("CLAUDE.md"),
            root.join(".claude").join("CLAUDE.md"),
            root.join("AGENTS.md"),
        ];
        // `os.Stat` follows symbolic links, as `fs::metadata` does.
        let target = candidates
            .iter()
            .find(|path| fs::metadata(path).is_ok())
            .cloned()
            .unwrap_or_else(|| root.join("CLAUDE.md"));
        if mode == OnboardMode::Stdout {
            return Ok(Onboarded::Snippet(ONBOARD_SNIPPET.to_owned()));
        }
        let read = fs::read(&target);
        if mode == OnboardMode::Check {
            // Go treats a read error as an empty file.
            let old = read.unwrap_or_default();
            let status = if contains(&old, START) && contains(&old, END) {
                if contains(&old, VERSION) {
                    OnboardStatus::Current
                } else {
                    OnboardStatus::Outdated
                }
            } else {
                OnboardStatus::Missing
            };
            return Ok(Onboarded::Checked {
                file: target,
                status,
            });
        }
        let old = match read {
            Ok(old) => old,
            Err(error) if error.kind() == io::ErrorKind::NotFound => Vec::new(),
            // RD55: Go would take this file as empty and replace it.
            Err(error) => {
                return Err(ArtifactsError::OnboardUnreadable {
                    file: target,
                    cause: error.to_string(),
                }
                .into())
            }
        };
        if let (Some(start_at), Some(end_at)) = (find(&old, START), find(&old, END)) {
            if end_at > start_at {
                if contains(&old[start_at..end_at], VERSION) {
                    return Ok(Onboarded::Applied {
                        file: target,
                        action: OnboardAction::Unchanged,
                    });
                }
                let mut updated = old[..start_at].to_vec();
                updated.extend_from_slice(ONBOARD_SNIPPET.as_bytes());
                updated.extend_from_slice(&old[end_at + END.len()..]);
                paths::write_file(&target, &updated)?;
                return Ok(Onboarded::Applied {
                    file: target,
                    action: OnboardAction::Updated,
                });
            }
        }
        let separator = if old.is_empty() {
            ""
        } else if old.ends_with(b"\n") {
            "\n"
        } else {
            "\n\n"
        };
        let created = old.is_empty();
        let mut content = old;
        content.extend_from_slice(separator.as_bytes());
        content.extend_from_slice(ONBOARD_SNIPPET.as_bytes());
        paths::write_file(&target, &content)?;
        Ok(Onboarded::Applied {
            file: target,
            action: if created {
                OnboardAction::Created
            } else {
                OnboardAction::Appended
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> (tempfile::TempDir, Beacon) {
        let dir = tempfile::tempdir().unwrap();
        let beacon = Beacon::open(dir.path()).unwrap();
        (dir, beacon)
    }

    #[test]
    fn snippet_is_built_from_the_markers() {
        assert_eq!(
            ONBOARD_SNIPPET,
            format!("{START}\n## Prompt Management (Luuna Beacon)\n{VERSION}\n\nRun `lbc prime` at the start of every session. Use `lbc update` then `lbc emit`; do not edit emitted files.\n{END}\n")
        );
    }

    #[test]
    fn create_append_update_and_unchanged() {
        let (dir, beacon) = setup();
        let file = dir.path().join("CLAUDE.md");
        let action = |beacon: &Beacon| match beacon.onboard(OnboardMode::Apply).unwrap() {
            Onboarded::Applied { action, .. } => action,
            other => panic!("{other:?}"),
        };
        assert_eq!(action(&beacon), OnboardAction::Created);
        assert_eq!(fs::read_to_string(&file).unwrap(), ONBOARD_SNIPPET);
        assert_eq!(action(&beacon), OnboardAction::Unchanged);

        fs::write(&file, b"notes").unwrap();
        assert_eq!(action(&beacon), OnboardAction::Appended);
        assert_eq!(
            fs::read_to_string(&file).unwrap(),
            format!("notes\n\n{ONBOARD_SNIPPET}")
        );

        let mut old = b"a\xff\n".to_vec();
        old.extend_from_slice(format!("{START}\nold\n{END}\n").as_bytes());
        old.extend_from_slice(b"tail\xfe");
        fs::write(&file, &old).unwrap();
        assert_eq!(action(&beacon), OnboardAction::Updated);
        let mut expected = b"a\xff\n".to_vec();
        expected.extend_from_slice(ONBOARD_SNIPPET.as_bytes());
        expected.extend_from_slice(b"\ntail\xfe");
        assert_eq!(fs::read(&file).unwrap(), expected);
    }

    #[test]
    fn check_and_stdout_write_nothing() {
        let (dir, beacon) = setup();
        let checked = beacon.onboard(OnboardMode::Check).unwrap();
        assert_eq!(
            checked,
            Onboarded::Checked {
                file: dir.path().join("CLAUDE.md"),
                status: OnboardStatus::Missing
            }
        );
        assert_eq!(
            beacon.onboard(OnboardMode::Stdout).unwrap(),
            Onboarded::Snippet(ONBOARD_SNIPPET.to_owned())
        );
        assert!(fs::read_dir(dir.path()).unwrap().next().is_none());
    }

    #[test]
    fn a_target_that_cannot_be_read_is_refused() {
        let (dir, beacon) = setup();
        let target = dir.path().join("CLAUDE.md");
        fs::create_dir(&target).unwrap();
        fs::write(target.join("inner.md"), b"inner").unwrap();
        let cause = fs::read(&target).unwrap_err().to_string();
        match beacon.onboard(OnboardMode::Apply) {
            Err(BeaconError::Artifacts(ArtifactsError::OnboardUnreadable { file, cause: got })) => {
                assert_eq!(file, target);
                assert_eq!(got, cause);
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(fs::read(target.join("inner.md")).unwrap(), b"inner");
    }
}
