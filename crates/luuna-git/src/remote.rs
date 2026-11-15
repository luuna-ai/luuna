//! Remote operations for publication (M7): the effective push URL of a
//! registered checkout, a bounded `ls-remote` probe and the push of one exact
//! commit to one branch. These are the only network operations in this crate.
//!
//! Credentials never appear in command arguments: a GitHub token travels
//! through `GIT_CONFIG_*` environment variables as an `http.<base>.extraheader`
//! scoped to the GitHub origin, credential helpers are disabled and Git may
//! never prompt. A push names the remote by URL rather than by remote name, so
//! no remote-tracking reference of the developer checkout is written.

use crate::{GitError, GitInspector};
use std::{path::Path, time::Duration};

/// The GitHub origin that an extraheader credential is scoped to; a push to
/// any other host never carries it.
pub const GITHUB_HTTPS_BASE: &str = "https://github.com/";

/// What a remote branch points at, as `git ls-remote` reported it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteRef {
    /// The branch does not exist on the remote.
    Absent,
    /// The branch exists and points at this commit.
    At(String),
}

/// GitHub repository coordinates parsed from a remote identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitHubRepository {
    pub owner: String,
    pub repo: String,
}

impl GitHubRepository {
    /// `https://github.com/<owner>/<repo>(.git)`, `ssh://git@github.com/<owner>/<repo>(.git)`
    /// or `git@github.com:<owner>/<repo>(.git)`. Any other host or shape is
    /// refused with the reason: publication is GitHub-only in v1.
    pub fn parse(remote: &str) -> Result<Self, String> {
        let remote = remote.trim();
        let path = if let Some((scheme, rest)) = remote.split_once("://") {
            let scheme = scheme.to_ascii_lowercase();
            if !matches!(scheme.as_str(), "https" | "ssh" | "git+ssh" | "ssh+git") {
                return Err(format!(
                    "remote {remote} uses the {scheme} transport; publication supports https:// and ssh:// GitHub remotes only"
                ));
            }
            let (authority, path) = rest
                .split_once('/')
                .ok_or_else(|| format!("remote {remote} names no repository path"))?;
            let host = authority.rsplit('@').next().unwrap_or(authority);
            let host = host.rsplit_once(':').map(|(h, _)| h).unwrap_or(host);
            if !host.eq_ignore_ascii_case("github.com") {
                return Err(format!(
                    "remote {remote} is hosted on {host}; publication is GitHub-only in v1"
                ));
            }
            path
        } else if let Some((authority, path)) = remote.split_once(':') {
            let host = authority.rsplit('@').next().unwrap_or(authority);
            if !host.eq_ignore_ascii_case("github.com") {
                return Err(format!(
                    "remote {remote} is hosted on {host}; publication is GitHub-only in v1"
                ));
            }
            path.trim_start_matches('/')
        } else {
            return Err(format!(
                "remote {remote} is not a GitHub https:// or ssh remote; publication is GitHub-only in v1"
            ));
        };
        let path = path.trim_end_matches('/');
        let path = path.strip_suffix(".git").unwrap_or(path);
        let mut parts = path.split('/');
        let (owner, repo) = match (parts.next(), parts.next(), parts.next()) {
            (Some(owner), Some(repo), None) if !owner.is_empty() && !repo.is_empty() => {
                (owner, repo)
            }
            _ => {
                return Err(format!(
                    "remote {remote} does not name exactly <owner>/<repo> on github.com"
                ))
            }
        };
        let valid = |part: &str| {
            part.len() <= 100
                && part
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.')
                && part != "."
                && part != ".."
        };
        if !valid(owner) || !valid(repo) {
            return Err(format!(
                "remote {remote} names an owner or repository with characters GitHub does not use"
            ));
        }
        Ok(Self {
            owner: owner.to_owned(),
            repo: repo.to_owned(),
        })
    }
}

/// A credential for pushes to GitHub over HTTPS. The value is held only for
/// the duration of a command and is never formatted into arguments or errors.
#[derive(Clone)]
pub struct PushCredential {
    token: String,
}

impl PushCredential {
    pub fn github(token: impl Into<String>) -> Self {
        Self {
            token: token.into(),
        }
    }
}

impl std::fmt::Debug for PushCredential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PushCredential(redacted)")
    }
}

/// Which `git` operations are allowed to reach the network, with a longer
/// deadline than inspection has.
#[derive(Clone, Debug)]
pub struct RemoteOperations {
    pub inspector: GitInspector,
    pub timeout: Duration,
}

impl RemoteOperations {
    pub fn new(inspector: GitInspector, timeout: Duration) -> Self {
        Self {
            inspector: GitInspector {
                command_timeout: timeout,
                ..inspector
            },
            timeout,
        }
    }

    /// The URL `git push origin` would use in this checkout, read without
    /// writing anything: the configured push URL, else the fetch URL, after
    /// the developer's own `url.<base>.insteadOf`/`pushInsteadOf` rewrites.
    /// Those rewrites are machine-local transport choices (HTTPS to SSH, a
    /// local mirror); the identity of the remote is verified separately on the
    /// configured URL by the caller.
    pub async fn push_url(&self, checkout: &Path) -> Result<String, GitError> {
        let output = self
            .inspector
            .git(
                Some(checkout),
                &["remote", "get-url", "--push", "origin"],
                &[],
            )
            .await?;
        if output.status != Some(0) {
            return Err(GitError::Command {
                command: "remote get-url --push origin".into(),
                detail: output.stderr,
            });
        }
        let url = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        if url.is_empty() {
            return Err(GitError::Command {
                command: "remote get-url --push origin".into(),
                detail: "origin has no URL".into(),
            });
        }
        Ok(url)
    }

    /// The configured `remote.origin.url` (no rewrites), which is what a
    /// registration recorded as identity.
    pub async fn configured_url(&self, checkout: &Path) -> Result<Option<String>, GitError> {
        let output = self
            .inspector
            .git(
                Some(checkout),
                &["config", "--get", "remote.origin.url"],
                &[],
            )
            .await?;
        match output.status {
            Some(0) => Ok(Some(
                String::from_utf8_lossy(&output.stdout).trim().to_owned(),
            )),
            Some(1) => Ok(None),
            _ => Err(GitError::Command {
                command: "config --get remote.origin.url".into(),
                detail: output.stderr,
            }),
        }
    }

    /// Whether `commit` exists in the checkout's object store (shared with its
    /// linked worktrees).
    pub async fn has_commit(&self, checkout: &Path, commit: &str) -> Result<bool, GitError> {
        let spec = format!("{commit}^{{commit}}");
        let output = self
            .inspector
            .git(Some(checkout), &["cat-file", "-e", &spec], &[])
            .await?;
        Ok(output.status == Some(0))
    }

    /// What `refs/heads/<branch>` points at on `url`, or that it is absent.
    pub async fn remote_branch(
        &self,
        checkout: &Path,
        url: &str,
        branch: &str,
        credential: Option<&PushCredential>,
    ) -> Result<RemoteRef, GitError> {
        let reference = format!("refs/heads/{branch}");
        let mut command = self.inspector.command(
            Some(checkout),
            &["ls-remote", "--heads", "--", url, &reference],
            &[],
        );
        Self::credentials(&mut command, credential);
        let output = self.inspector.run(command, "ls-remote").await?;
        if output.status != Some(0) {
            return Err(GitError::Command {
                command: "ls-remote".into(),
                detail: output.stderr,
            });
        }
        let text = String::from_utf8_lossy(&output.stdout);
        for line in text.lines() {
            let mut fields = line.split_whitespace();
            if let (Some(sha), Some(name)) = (fields.next(), fields.next()) {
                if name == reference {
                    return Ok(RemoteRef::At(sha.to_owned()));
                }
            }
        }
        Ok(RemoteRef::Absent)
    }

    /// Push exactly `commit` to `refs/heads/<branch>` on `url`. No tags follow
    /// and nothing is forced: a branch that moved elsewhere refuses the push.
    /// A command-local remote without a fetch mapping prevents Git from updating
    /// any remote-tracking refs in the checkout's shared Git directory.
    pub async fn push_commit(
        &self,
        checkout: &Path,
        url: &str,
        commit: &str,
        branch: &str,
        credential: Option<&PushCredential>,
    ) -> Result<(), GitError> {
        // Recent Git versions recognize configured remotes by their effective
        // push URL, even when push is given a URL rather than a remote name.
        // In a linked worktree that also writes tracking refs/reflogs into the
        // developer checkout. Use an explicitly named, command-local remote with
        // no fetch refspec instead. Never reuse a configured name: its fetch,
        // mirror or push settings could mutate refs or publish extra branches.
        let output = self.inspector.git(Some(checkout), &["remote"], &[]).await?;
        if output.status != Some(0) {
            return Err(GitError::Command {
                command: "remote".into(),
                detail: output.stderr,
            });
        }
        let remotes = String::from_utf8_lossy(&output.stdout);
        let name = (0_u64..)
            .map(|n| format!("luuna-publication-{n}"))
            .find(|name| !remotes.lines().any(|remote| remote == name))
            .expect("a finite config leaves an unused remote name");
        let url_setting = format!("remote.{name}.url={url}");
        let refspec = format!("{commit}:refs/heads/{branch}");
        let mut command = self.inspector.command(
            Some(checkout),
            &[
                "-c",
                "credential.helper=",
                "-c",
                "push.default=nothing",
                "-c",
                &url_setting,
                "push",
                "--no-follow-tags",
                "--no-verify",
                "--porcelain",
                "--",
                &name,
                &refspec,
            ],
            &[],
        );
        Self::credentials(&mut command, credential);
        let output = self.inspector.run(command, "push").await?;
        if output.status != Some(0) {
            return Err(GitError::Command {
                command: "push".into(),
                detail: output.stderr,
            });
        }
        Ok(())
    }

    /// Credentials reach Git only through its environment configuration, scoped
    /// to the GitHub HTTPS origin. Helpers and prompts stay disabled either way.
    fn credentials(command: &mut tokio::process::Command, credential: Option<&PushCredential>) {
        command.env("GIT_TERMINAL_PROMPT", "0");
        command.env("GIT_ASKPASS", "");
        match credential {
            Some(credential) => {
                command.env("GIT_CONFIG_COUNT", "2");
                command.env(
                    "GIT_CONFIG_KEY_0",
                    format!("http.{GITHUB_HTTPS_BASE}.extraheader"),
                );
                command.env(
                    "GIT_CONFIG_VALUE_0",
                    format!("Authorization: Bearer {}", credential.token),
                );
                command.env("GIT_CONFIG_KEY_1", "credential.helper");
                command.env("GIT_CONFIG_VALUE_1", "");
            }
            None => {
                command.env("GIT_CONFIG_COUNT", "1");
                command.env("GIT_CONFIG_KEY_0", "credential.helper");
                command.env("GIT_CONFIG_VALUE_0", "");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn github_remotes_parse_and_other_hosts_are_refused() {
        for remote in [
            "https://github.com/octo/repo.git",
            "https://github.com/octo/repo",
            "https://GitHub.com/octo/repo/",
            "ssh://git@github.com/octo/repo.git",
            "git@github.com:octo/repo.git",
            "git@github.com:octo/repo",
        ] {
            let parsed =
                GitHubRepository::parse(remote).unwrap_or_else(|e| panic!("{remote}: {e}"));
            assert_eq!(parsed.owner, "octo", "{remote}");
            assert_eq!(parsed.repo, "repo", "{remote}");
        }
        for remote in [
            "https://gitlab.com/octo/repo.git",
            "git@gitlab.com:octo/repo.git",
            "https://github.com/octo",
            "https://github.com/octo/repo/extra",
            "local:app",
            "C:/repos/bare.git",
            "/srv/git/bare.git",
            "file:///srv/git/bare.git",
            "https://github.com/../repo",
        ] {
            let error = GitHubRepository::parse(remote).unwrap_err();
            assert!(
                error.contains("GitHub") || error.contains("github.com"),
                "{remote}: {error}"
            );
        }
    }

    #[test]
    fn credentials_are_debug_redacted() {
        let credential = PushCredential::github("ghp_secret");
        assert_eq!(format!("{credential:?}"), "PushCredential(redacted)");
    }
}
