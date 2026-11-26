//! Committing a checkout's Luuna stores (ADR 0068).
//!
//! When a project keeps its component stores in a developer checkout, the
//! runtime records each change to `.luuna/` as its own commit on the checkout's
//! current branch. The commit is built in a private temporary index from HEAD
//! plus `.luuna/` and, when its only changes are lines for `.luuna/` paths,
//! `.gitattributes`. Nothing else the developer changed or staged is included.
//! HEAD moves with a compare-and-swap, so a commit the developer made
//! meanwhile is never overwritten. Afterwards only the committed paths are
//! reset in the developer's own index, so `git status` stays truthful. Hooks,
//! signing, content filters and inherited `GIT_*` state do not apply.
use crate::{GitError, GitInspector, Output};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Identity used when the checkout has no configured Git identity.
pub const STORE_AUTHOR_NAME: &str = "Luuna";
pub const STORE_AUTHOR_EMAIL: &str = "luuna@luuna.invalid";

/// The folder whose changes are committed.
const STORE_PATH: &str = ".luuna";
const ATTRIBUTES: &str = ".gitattributes";
/// Files whose presence means an operation is in progress in the checkout.
const IN_PROGRESS: [&str; 6] = [
    "MERGE_HEAD",
    "CHERRY_PICK_HEAD",
    "REVERT_HEAD",
    "BISECT_LOG",
    "rebase-merge",
    "rebase-apply",
];
const ZERO_OID: &str = "0000000000000000000000000000000000000000";

/// What committing a checkout's stores did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreCommit {
    /// A commit was made and HEAD now points at it.
    Committed(String),
    /// `.luuna/` holds no change against HEAD.
    Unchanged,
    /// The developer's ignore rules exclude `.luuna`, so nothing is committed.
    Ignored,
    /// A merge, rebase, cherry-pick, revert or bisect is in progress.
    Busy,
}

static SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Removes the temporary index however the commit ends.
struct TemporaryIndex(PathBuf);
impl Drop for TemporaryIndex {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
        let mut lock = self.0.clone().into_os_string();
        lock.push(".lock");
        let _ = std::fs::remove_file(lock);
    }
}

/// Whether the developer's `.gitattributes` differs from HEAD's only by added
/// lines that name `.luuna/` paths, which is what a component init writes.
/// HEAD's lines must all remain, in their order and number; a reordered,
/// removed or changed rule is the developer's edit and is never committed.
pub fn only_store_attribute_lines(head: Option<&str>, worktree: &str) -> bool {
    let mut before = head.unwrap_or("").lines().peekable();
    for line in worktree.lines() {
        if before.peek() == Some(&line) {
            before.next();
        } else if !line.trim_start().starts_with(".luuna/") {
            return false;
        }
    }
    before.next().is_none()
}

impl GitInspector {
    /// The checkout's git command, with its own `.git` and work tree named
    /// explicitly and, when given, the temporary index.
    async fn store_git(
        &self,
        root: &Path,
        args: &[&str],
        overrides: &[String],
        index: Option<&Path>,
    ) -> Result<Output, GitError> {
        let mut full = vec![
            format!("--git-dir={}", root.join(".git").display()),
            format!("--work-tree={}", root.display()),
        ];
        full.extend(args.iter().map(|a| (*a).to_owned()));
        let refs: Vec<&str> = full.iter().map(String::as_str).collect();
        let mut command = self.command(Some(root), &refs, overrides);
        if let Some(index) = index {
            command.env("GIT_INDEX_FILE", index);
        }
        let name = args.first().copied().unwrap_or("git");
        self.run(command, name).await
    }

    async fn store_expect(
        &self,
        root: &Path,
        args: &[&str],
        overrides: &[String],
        index: Option<&Path>,
    ) -> Result<String, GitError> {
        let output = self.store_git(root, args, overrides, index).await?;
        if output.status != Some(0) {
            return Err(GitError::Command {
                command: args.first().copied().unwrap_or("git").to_owned(),
                detail: output.stderr,
            });
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
    }

    /// Commit the changes under `root/.luuna` (and store-only lines of
    /// `.gitattributes`) to the checkout's current branch with `message`. The
    /// checkout must be a main checkout whose `.git` is a directory; callers
    /// check that first. See the module documentation for what is guaranteed.
    pub async fn commit_store(&self, root: &Path, message: &str) -> Result<StoreCommit, GitError> {
        let git_dir = root.join(".git");
        if !git_dir.is_dir() {
            return Err(GitError::NotRepository);
        }
        if IN_PROGRESS
            .iter()
            .any(|name| std::fs::symlink_metadata(git_dir.join(name)).is_ok())
        {
            return Ok(StoreCommit::Busy);
        }
        let ignored = self
            .store_git(root, &["check-ignore", "-q", "--", STORE_PATH], &[], None)
            .await?;
        if ignored.status == Some(0) {
            return Ok(StoreCommit::Ignored);
        }
        let overrides = self.filter_overrides(root).await?;
        let head = {
            let output = self
                .store_git(root, &["rev-parse", "--verify", "-q", "HEAD"], &[], None)
                .await?;
            (output.status == Some(0))
                .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
        };
        let index = TemporaryIndex(git_dir.join(format!(
            "luuna-store-index-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        )));
        let temp = Some(index.0.as_path());
        match &head {
            Some(head) => {
                self.store_expect(root, &["read-tree", head], &overrides, temp)
                    .await?;
            }
            None => {
                self.store_expect(root, &["read-tree", "--empty"], &overrides, temp)
                    .await?;
            }
        }
        self.store_expect(root, &["add", "--all", "--", STORE_PATH], &overrides, temp)
            .await?;
        let attributes = self.store_attributes(root, head.as_deref()).await?;
        if attributes {
            self.store_expect(root, &["add", "--", ATTRIBUTES], &overrides, temp)
                .await?;
        }
        let tree = self
            .store_expect(root, &["write-tree"], &overrides, temp)
            .await?;
        if let Some(head) = &head {
            let spec = format!("{head}^{{tree}}");
            let current = self
                .store_expect(root, &["rev-parse", &spec], &[], None)
                .await?;
            if current == tree {
                return Ok(StoreCommit::Unchanged);
            }
        }
        let mut commit_args = vec!["commit-tree", tree.as_str()];
        if let Some(head) = &head {
            commit_args.extend(["-p", head.as_str()]);
        }
        commit_args.extend(["-m", message]);
        let mut settings = overrides.clone();
        settings.extend([
            "commit.gpgsign=false".to_owned(),
            "gc.auto=0".to_owned(),
            "maintenance.auto=false".to_owned(),
        ]);
        let commit = match self.store_expect(root, &commit_args, &settings, None).await {
            Ok(commit) => commit,
            // No identity is configured for this checkout: record Luuna's.
            Err(GitError::Command { detail, .. })
                if detail.contains("ident") || detail.contains("tell me who you are") =>
            {
                let mut settings = settings.clone();
                settings.extend([
                    format!("user.name={STORE_AUTHOR_NAME}"),
                    format!("user.email={STORE_AUTHOR_EMAIL}"),
                ]);
                self.store_expect(root, &commit_args, &settings, None)
                    .await?
            }
            Err(error) => return Err(error),
        };
        let old = head.as_deref().unwrap_or(ZERO_OID);
        self.store_expect(
            root,
            &["update-ref", "-m", message, "HEAD", &commit, old],
            &[],
            None,
        )
        .await?;
        // The developer's index still holds the old entries for these paths;
        // match them to the new HEAD so nothing reads as a staged revert.
        let mut paths = vec!["reset", "-q", "HEAD", "--", STORE_PATH];
        if attributes {
            paths.push(ATTRIBUTES);
        }
        self.store_expect(root, &paths, &overrides, None).await?;
        Ok(StoreCommit::Committed(commit))
    }

    /// Whether `.gitattributes` should be committed with the stores.
    async fn store_attributes(&self, root: &Path, head: Option<&str>) -> Result<bool, GitError> {
        let path = root.join(ATTRIBUTES);
        match std::fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.is_file() => {}
            _ => return Ok(false),
        }
        let Ok(worktree) = std::fs::read_to_string(&path) else {
            return Ok(false);
        };
        let committed = match head {
            Some(head) => {
                let spec = format!("{head}:{ATTRIBUTES}");
                let output = self
                    .store_git(root, &["cat-file", "blob", &spec], &[], None)
                    .await?;
                (output.status == Some(0))
                    .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
            }
            None => None,
        };
        if committed.as_deref() == Some(worktree.as_str()) {
            return Ok(false);
        }
        // A `.gitattributes` the developer staged differently from HEAD is
        // theirs: committing and resetting it would lose the staged version.
        let staged = self
            .store_git(
                root,
                &["diff", "--cached", "--quiet", "--", ATTRIBUTES],
                &[],
                None,
            )
            .await?;
        if staged.status != Some(0) {
            return Ok(false);
        }
        Ok(only_store_attribute_lines(committed.as_deref(), &worktree))
    }
}

#[cfg(test)]
mod tests {
    use super::only_store_attribute_lines;

    #[test]
    fn only_added_store_lines_count_as_store_attributes() {
        let union = ".luuna/missions/issues.jsonl merge=union\n";
        assert!(only_store_attribute_lines(None, union));
        assert!(only_store_attribute_lines(
            Some("*.png binary\n"),
            &format!("*.png binary\n{union}")
        ));
        assert!(!only_store_attribute_lines(
            Some("*.png binary\n"),
            &format!("*.jpg binary\n{union}")
        ));
        assert!(!only_store_attribute_lines(None, "*.png binary\n"));
        // Reordering or dropping a duplicate rule is the developer's edit.
        assert!(!only_store_attribute_lines(
            Some("*.txt text\n*.txt -text\n"),
            &format!("*.txt -text\n*.txt text\n{union}")
        ));
        assert!(!only_store_attribute_lines(
            Some("*.txt text\n*.txt text\n"),
            &format!("*.txt text\n{union}")
        ));
        assert!(only_store_attribute_lines(
            Some("*.txt text\n*.txt text\n"),
            &format!("*.txt text\n{union}*.txt text\n")
        ));
    }
}
