//! `sync` (`cli.go:1704-1742`): `git status`, `git add` and `git commit` of
//! the store folder, run in the project root.
//!
//! This is the only file of the crate that starts a process (AS52). The
//! program and its environment come from the caller's [`GitRunner`] (RD52):
//! the library reads no environment variable and never uses a shell. Each
//! start gets a null stdin and one pipe for both stdout and stderr, read to
//! its end before the wait, which is Go's `CombinedOutput`.

use super::ArtifactsError;
use crate::{Beacon, BeaconError};
use std::ffi::OsString;
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};

/// The pathspec Go passes to `git status` and `git add`.
const STORE_PATHSPEC: &str = ".luuna/beacon/";
/// The commit message before the UTC date.
const MESSAGE_PREFIX: &str = "luuna-beacon: sync ";

/// The environment a git start receives.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum GitEnvironment {
    /// The parent's environment, unchanged, as Go (the default).
    #[default]
    Inherit,
    /// A cleared environment holding exactly this list. A bare program name
    /// is looked up on the list's own `PATH`.
    Exact(Vec<(OsString, OsString)>),
}

/// The git program sync starts and the environment it gets (RD52).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitRunner {
    pub program: OsString,
    pub environment: GitEnvironment,
}

impl Default for GitRunner {
    /// `git`, with the parent's environment: Go's `exec.Command("git", ...)`.
    fn default() -> Self {
        GitRunner {
            program: OsString::from("git"),
            environment: GitEnvironment::Inherit,
        }
    }
}

/// The flags of `sync` and the git it runs.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SyncOptions {
    /// `--status`: report the status only.
    pub status_only: bool,
    pub git: GitRunner,
}

/// What `sync` did. `files` are the status paths as Go reports them, the
/// first character of the first one lost (GD76).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Synced {
    Status {
        files: Vec<String>,
        uncommitted: bool,
    },
    NothingToCommit,
    Committed {
        files: Vec<String>,
        message: String,
    },
}

impl Beacon {
    /// Go's `sync`: the legacy move, then `git status --porcelain
    /// .luuna/beacon/`; with `status_only` the status is the result.
    /// Otherwise, if the status names any file, `git add .luuna/beacon/` and
    /// `git commit -m "luuna-beacon: sync <YYYY-MM-DD>"` (no pathspec: the
    /// whole index is committed, GD75), the date from the handle's clock
    /// (RD54). A failure is `git <step>: <trimmed combined output>`, with an
    /// empty detail when git cannot be started (GD77).
    pub fn sync(&self, options: &SyncOptions) -> Result<Synced, BeaconError> {
        self.migrate_legacy()?;
        let root = self.project_root();
        let status = run_git(
            &options.git,
            root,
            "status",
            &["status", "--porcelain", STORE_PATHSPEC],
        )?;
        let files = status_files(&status);
        if options.status_only {
            return Ok(Synced::Status {
                uncommitted: !files.is_empty(),
                files,
            });
        }
        if files.is_empty() {
            return Ok(Synced::NothingToCommit);
        }
        run_git(&options.git, root, "add", &["add", STORE_PATHSPEC])?;
        let now = self.now_text();
        let day = now.get(..10).unwrap_or(&now);
        let message = format!("{MESSAGE_PREFIX}{day}");
        run_git(&options.git, root, "commit", &["commit", "-m", &message])?;
        Ok(Synced::Committed { files, message })
    }
}

/// One git start in `root`: the combined output on success, otherwise
/// `ArtifactsError::Git` with the trimmed output (empty when the start
/// itself failed, as Go's `CombinedOutput` returns no bytes then).
fn run_git(
    runner: &GitRunner,
    root: &Path,
    step: &str,
    arguments: &[&str],
) -> Result<Vec<u8>, ArtifactsError> {
    let failed = |output: &[u8]| ArtifactsError::Git {
        step: step.to_owned(),
        output: go_text(go_trim_space(output)),
    };
    let (mut reader, writer) = std::io::pipe().map_err(|_| failed(b""))?;
    let writer_for_stderr = writer.try_clone().map_err(|_| failed(b""))?;
    let mut command = Command::new(&runner.program);
    command
        .args(arguments)
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(writer)
        .stderr(writer_for_stderr);
    if let GitEnvironment::Exact(list) = &runner.environment {
        command.env_clear();
        command.envs(list.iter().map(|(name, value)| (name, value)));
    }
    let started = command.spawn();
    // The command holds the parent's copies of the pipe's write end; they
    // must be closed before reading, or the read never sees its end.
    drop(command);
    let mut child = started.map_err(|_| failed(b""))?;
    let mut output = Vec::new();
    let read = reader.read_to_end(&mut output);
    let status = child.wait();
    match (read, status) {
        (Ok(_), Ok(status)) if status.success() => Ok(output),
        _ => Err(failed(&output)),
    }
}

/// The status paths as `cli.go:1717-1724` takes them: the whole output
/// trimmed, split on `\n`, and of every line of at least 4 bytes the rest
/// after byte 3, trimmed.
fn status_files(output: &[u8]) -> Vec<String> {
    let trimmed = go_trim_space(output);
    if trimmed.is_empty() {
        return Vec::new();
    }
    trimmed
        .split(|byte| *byte == b'\n')
        .filter(|line| line.len() >= 4)
        .map(|line| go_text(go_trim_space(&line[3..])))
        .collect()
}

/// Go's `strings.TrimSpace` on bytes: leading and trailing characters with
/// the Unicode White_Space property (Go's `unicode.IsSpace` is the same set);
/// a byte that is not valid UTF-8 is not space.
fn go_trim_space(mut bytes: &[u8]) -> &[u8] {
    while let Some((character, width)) = first_char(bytes) {
        if !character.is_whitespace() {
            break;
        }
        bytes = &bytes[width..];
    }
    while let Some((character, width)) = last_char(bytes) {
        if !character.is_whitespace() {
            break;
        }
        bytes = &bytes[..bytes.len() - width];
    }
    bytes
}

/// The first character and its width, or `None` for no bytes or an invalid
/// first byte (Go's `utf8.RuneError`).
fn first_char(bytes: &[u8]) -> Option<(char, usize)> {
    let head = &bytes[..bytes.len().min(4)];
    let valid = match std::str::from_utf8(head) {
        Ok(text) => text,
        Err(error) => std::str::from_utf8(&head[..error.valid_up_to()]).ok()?,
    };
    valid
        .chars()
        .next()
        .map(|character| (character, character.len_utf8()))
}

/// The last character and its width, or `None` as for [`first_char`]
/// (Go's `utf8.DecodeLastRune`).
fn last_char(bytes: &[u8]) -> Option<(char, usize)> {
    (1..=bytes.len().min(4)).find_map(|width| {
        let text = std::str::from_utf8(&bytes[bytes.len() - width..]).ok()?;
        let mut characters = text.chars();
        let character = characters.next()?;
        characters.next().is_none().then_some((character, width))
    })
}

/// The bytes as Go's `encoding/json` carries a string: each byte that is
/// not valid UTF-8 becomes U+FFFD.
fn go_text(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len());
    let mut rest = bytes;
    while !rest.is_empty() {
        match std::str::from_utf8(rest) {
            Ok(text) => {
                out.push_str(text);
                break;
            }
            Err(error) => {
                let valid = error.valid_up_to();
                out.push_str(std::str::from_utf8(&rest[..valid]).expect("a valid prefix"));
                out.push('\u{FFFD}');
                rest = &rest[valid + 1..];
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_status_line_loses_a_character_like_go() {
        let output = b" M .luuna/beacon/a.jsonl\nabc\n M .luuna/beacon/b.jsonl\n?? x\n";
        assert_eq!(
            status_files(output),
            ["luuna/beacon/a.jsonl", ".luuna/beacon/b.jsonl", "x"],
            "GD76; a line under 4 bytes is skipped"
        );
        assert_eq!(
            status_files(b"R  .luuna/beacon/a -> .luuna/beacon/b\nab\n"),
            [".luuna/beacon/a -> .luuna/beacon/b"]
        );
        assert!(status_files(b" \n\t\r\n").is_empty());
        assert!(status_files(b"").is_empty());
    }

    #[test]
    fn trimming_follows_go_unicode_space_and_keeps_invalid_bytes() {
        assert_eq!(go_trim_space(b"\xc2\xa0 a b\t\xe3\x80\x80\n"), b"a b");
        assert_eq!(go_trim_space(b"\x85a\xff "), b"\x85a\xff");
        assert_eq!(go_trim_space(b" \xe3\x80"), b"\xe3\x80");
        assert_eq!(go_trim_space(b"\xe2\x80\xa8"), b"");
        assert_eq!(
            go_text(b"a\xe2\x82b\xffc\xc3\xa9"),
            "a\u{FFFD}\u{FFFD}b\u{FFFD}c\u{e9}"
        );
    }

    #[test]
    fn defaults_are_go_and_a_failed_start_has_no_detail() {
        let options = SyncOptions::default();
        assert!(!options.status_only);
        assert_eq!(options.git.program, OsString::from("git"));
        assert_eq!(options.git.environment, GitEnvironment::Inherit);
        let dir = tempfile::tempdir().unwrap();
        let runner = GitRunner {
            program: dir.path().join("missing-git").into_os_string(),
            environment: GitEnvironment::Exact(Vec::new()),
        };
        let error = run_git(&runner, dir.path(), "add", &["add"]).unwrap_err();
        assert_eq!(error.to_string(), "git add: ");
    }
}
