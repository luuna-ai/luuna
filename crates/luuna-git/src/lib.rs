//! Read-only repository inspection through the pinned host `git` executable.
//!
//! Every command runs with optional locks disabled, so even `git status` leaves
//! the index and working tree byte-identical. Repository-controlled hooks,
//! filesystem monitors and content filters (including those configured only in
//! a populated submodule) are neutralised from the command line, inspection
//! issues no network operation (publication's push in `remote` and Orbit's
//! explicit fetch and clone in `sync` are the only ones), and remote URLs are
//! redacted before they leave this crate. `store_commit` is the one place that
//! commits in a developer checkout, and only `.luuna/` (ADR 0068).
//! Output is capped per stream: a command passing the cap is killed at once.
use chrono::{SecondsFormat, Utc};
use luuna_domain::{
    is_filesystem_remote, remotes_equivalent, DirtyState, RepositoryIdentity,
    RepositoryObservation, MAX_RECORDED_DIRTY_PATHS,
};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use thiserror::Error;
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    process::Command,
};

pub const MAX_OUTPUT_BYTES: usize = 8 * 1024 * 1024;

mod remote;
mod store_commit;
mod sync;
mod worktree;
pub use remote::*;
pub use store_commit::*;
pub use sync::*;
pub use worktree::*;
const MAX_ROOT_COMMITS: usize = 64;
/// Roots reachable from any local branch, observed only to match a registration.
const MAX_REACHABLE_ROOTS: usize = 4096;
/// Populated submodules (at any depth) whose filter drivers are enumerated.
const MAX_SUBMODULES: usize = 256;
const MAX_SUBMODULE_DEPTH: usize = 8;

#[derive(Debug, Error)]
pub enum GitError {
    #[error("repository path must be an existing absolute directory")]
    InvalidPath,
    #[error("not a Git repository")]
    NotRepository,
    /// Git's own safe.directory refusal; Luuna never changes that setting.
    #[error("Git refuses to open {0} because another account owns it (dubious ownership); if you trust this checkout, add it to safe.directory in your global Git configuration (git config --global --add safe.directory <path>)")]
    UnsafeOwnership(String),
    #[error("bare repositories cannot be imported")]
    Bare,
    #[error("path is inside a repository whose root is {0}")]
    NotRoot(String),
    #[error("repository has no commits on HEAD")]
    Unborn,
    #[error("repository has {0} root commits; more than {MAX_ROOT_COMMITS} reachable from HEAD (or {MAX_REACHABLE_ROOTS} from all local branches) cannot form a stable identity")]
    TooManyRoots(usize),
    #[error("{0}")]
    Identity(#[from] luuna_domain::DomainError),
    /// Configuration that inspection cannot neutralise, so it refuses to read.
    #[error("{0}")]
    UnsafeConfiguration(String),
    #[error("git {command} failed: {detail}")]
    Command { command: String, detail: String },
    #[error("git {0} timed out")]
    Timeout(String),
    #[error(
        "git output too large: a stream exceeded the configured limit and the command was stopped"
    )]
    Oversized,
    #[error("I/O: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Clone, Debug)]
pub struct GitInspector {
    pub program: PathBuf,
    pub command_timeout: Duration,
    /// Per-stream output cap enforced while reading; the child is killed beyond it.
    pub max_output_bytes: usize,
}

impl Default for GitInspector {
    fn default() -> Self {
        Self {
            program: PathBuf::from("git"),
            command_timeout: Duration::from_secs(30),
            max_output_bytes: MAX_OUTPUT_BYTES,
        }
    }
}

struct Output {
    status: Option<i32>,
    stdout: Vec<u8>,
    stderr: String,
}

/// Which parentless commits an inspection records as identity.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Roots {
    /// Reachable from HEAD: the identity a registration stores.
    Head,
    /// Reachable from HEAD or any local branch: what a stored identity must be
    /// contained in, so new roots (an unrelated-history merge, an orphan branch
    /// checked out) do not read as a different repository.
    Reachable,
}

/// Read a stream to its end, failing as soon as it passes `max` bytes so the
/// caller stops the child at once instead of waiting for it to block on a full pipe.
async fn capped(mut pipe: impl AsyncRead + Unpin, max: usize) -> Result<Vec<u8>, GitError> {
    let mut buffer = Vec::new();
    (&mut pipe)
        .take((max as u64).saturating_add(1))
        .read_to_end(&mut buffer)
        .await?;
    if buffer.len() > max {
        return Err(GitError::Oversized);
    }
    Ok(buffer)
}

/// Collect the populated submodules among the gitlinks that `git ls-files --stage -z`
/// lists, without buffering the listing, whose size grows with the repository.
/// `populated` maps a gitlink path to its checkout when Git would recurse into it;
/// only those count toward the bound, so recorded but uninitialised submodules
/// never make a repository uninspectable.
async fn scan_gitlinks<T: PartialEq>(
    mut pipe: impl AsyncRead + Unpin,
    mut populated: impl FnMut(&[u8]) -> Option<T>,
) -> Result<Vec<T>, GitError> {
    const GITLINK: &[u8] = b"160000 ";
    const MAX_RECORD: usize = 64 * 1024;
    let mut links: Vec<T> = Vec::new();
    let mut record = Vec::new();
    let mut oversized = false;
    let mut chunk = vec![0u8; 64 * 1024];
    loop {
        let read = pipe.read(&mut chunk).await?;
        if read == 0 {
            break;
        }
        for part in chunk[..read].split_inclusive(|byte| *byte == 0) {
            let complete = part.last() == Some(&0);
            let bytes = if complete {
                &part[..part.len() - 1]
            } else {
                part
            };
            // Keep the start of an overlong record, so a gitlink is still recognised.
            let room = MAX_RECORD - record.len();
            oversized |= bytes.len() > room;
            record.extend_from_slice(&bytes[..bytes.len().min(room)]);
            if !complete {
                continue;
            }
            if record.starts_with(GITLINK) {
                if oversized {
                    return Err(GitError::Oversized);
                }
                let path = record
                    .iter()
                    .position(|byte| *byte == b'\t')
                    .map(|tab| &record[tab + 1..])
                    .filter(|path| !path.is_empty());
                if let Some(submodule) = path.and_then(&mut populated) {
                    // Unmerged gitlinks appear once per stage.
                    if !links.contains(&submodule) {
                        if links.len() == MAX_SUBMODULES {
                            return Err(too_many_submodules());
                        }
                        links.push(submodule);
                    }
                }
            }
            record.clear();
            oversized = false;
        }
    }
    Ok(links)
}

/// The checkout of the submodule recorded at index path `link` when Git would
/// recurse into it (it is populated: it has a `.git`). Git never records a path
/// that leaves the repository, so such a path is ignored rather than followed.
fn populated_submodule(repository: &Path, link: &[u8]) -> Option<PathBuf> {
    let link = relative_path(link);
    if link.is_absolute()
        || link
            .components()
            .any(|part| !matches!(part, std::path::Component::Normal(_)))
    {
        return None;
    }
    let submodule = repository.join(link);
    std::fs::symlink_metadata(submodule.join(".git"))
        .is_ok()
        .then_some(submodule)
}

fn too_many_submodules() -> GitError {
    GitError::UnsafeConfiguration(format!(
        "more than {MAX_SUBMODULES} populated submodules, or submodules nested more than {MAX_SUBMODULE_DEPTH} levels deep; their filter configuration cannot all be disabled for inspection"
    ))
}

/// Git prints one path per line; only the line terminator is removed, so a
/// directory name ending in whitespace stays intact.
fn output_path(bytes: &[u8]) -> PathBuf {
    let bytes = bytes.strip_suffix(b"\n").unwrap_or(bytes);
    let bytes = bytes.strip_suffix(b"\r").unwrap_or(bytes);
    relative_path(bytes)
}

#[cfg(unix)]
fn relative_path(bytes: &[u8]) -> PathBuf {
    use std::os::unix::ffi::OsStrExt;
    PathBuf::from(std::ffi::OsStr::from_bytes(bytes))
}

#[cfg(not(unix))]
fn relative_path(bytes: &[u8]) -> PathBuf {
    PathBuf::from(String::from_utf8_lossy(bytes).into_owned())
}

/// Classify a failed repository probe without discarding Git's reason.
fn probe_error(path: &Path, stderr: &str) -> GitError {
    let lower = stderr.to_ascii_lowercase();
    if lower.contains("dubious ownership") {
        return GitError::UnsafeOwnership(path.display().to_string());
    }
    if lower.is_empty() || lower.contains("not a git repository") {
        return GitError::NotRepository;
    }
    GitError::Command {
        command: "rev-parse".into(),
        detail: stderr.split_whitespace().collect::<Vec<_>>().join(" "),
    }
}

impl GitInspector {
    /// A host `git` command with the inspection environment: no pager, hooks,
    /// filesystem monitor, optional locks, prompts or inherited `GIT_*` state.
    fn command(&self, cwd: Option<&Path>, args: &[&str], overrides: &[String]) -> Command {
        let mut command = Command::new(&self.program);
        command.arg("--no-pager");
        if let Some(cwd) = cwd {
            command.arg("-C").arg(cwd);
        }
        // Repository configuration cannot reintroduce these: -c wins over every file.
        command.args([
            "-c",
            "core.fsmonitor=false",
            "-c",
            "core.hooksPath=/dev/null",
        ]);
        for setting in overrides {
            command.arg("-c").arg(setting);
        }
        command.args(args);
        // Every inherited GIT_* variable (GIT_DIR, GIT_COMMON_DIR, GIT_OBJECT_DIRECTORY,
        // GIT_CONFIG_*, ...) could redirect inspection away from the path given to -C.
        for (name, _) in std::env::vars_os() {
            if name.to_string_lossy().starts_with("GIT_") {
                command.env_remove(&name);
            }
        }
        command
            .env("GIT_OPTIONAL_LOCKS", "0")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("LC_ALL", "C")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        command
    }

    /// `overrides` are additional `-c key=value` settings, used to neutralise
    /// repository-selected content filters once their drivers are known.
    async fn git(
        &self,
        cwd: Option<&Path>,
        args: &[&str],
        overrides: &[String],
    ) -> Result<Output, GitError> {
        let name = args.first().copied().unwrap_or("git").to_owned();
        self.run(self.command(cwd, args, overrides), &name).await
    }

    /// Run a prepared command (from [`Self::command`], possibly with extra
    /// environment) under the output cap and deadline; `name` labels a timeout.
    async fn run(&self, mut command: Command, name: &str) -> Result<Output, GitError> {
        let name = name.to_owned();
        let mut child = command.spawn()?;
        let stdout_pipe = child.stdout.take().expect("stdout is piped");
        let stderr_pipe = child.stderr.take().expect("stderr is piped");
        let max = self.max_output_bytes;
        // Either stream passing the cap ends both reads at once; the child is
        // then killed instead of blocking on a pipe nobody drains.
        let read = async { tokio::try_join!(capped(stdout_pipe, max), capped(stderr_pipe, max)) };
        let (stdout, stderr) = match tokio::time::timeout(self.command_timeout, read).await {
            Ok(Ok(streams)) => streams,
            Ok(Err(error)) => {
                let _ = child.kill().await;
                return Err(error);
            }
            Err(_) => {
                let _ = child.kill().await;
                return Err(GitError::Timeout(name));
            }
        };
        let status = match tokio::time::timeout(self.command_timeout, child.wait()).await {
            Ok(status) => status?,
            Err(_) => {
                let _ = child.kill().await;
                return Err(GitError::Timeout(name));
            }
        };
        Ok(Output {
            status: status.code(),
            stdout,
            stderr: String::from_utf8_lossy(&stderr).chars().take(400).collect(),
        })
    }

    async fn expect(
        &self,
        cwd: &Path,
        args: &[&str],
        overrides: &[String],
    ) -> Result<String, GitError> {
        let output = self.git(Some(cwd), args, overrides).await?;
        if output.status != Some(0) {
            return Err(GitError::Command {
                command: args.join(" "),
                detail: output.stderr,
            });
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }

    async fn optional(
        &self,
        cwd: &Path,
        args: &[&str],
        overrides: &[String],
    ) -> Result<Option<String>, GitError> {
        let output = self.git(Some(cwd), args, overrides).await?;
        Ok((output.status == Some(0))
            .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned()))
    }

    /// Every configured `filter.<driver>.*` entry (system, global, repository or
    /// a populated submodule's own configuration, at any depth) is overridden to an
    /// empty command with `required=false`, so no clean, smudge or process filter
    /// can execute during inspection. `-c` settings reach the `git status` that Git
    /// runs inside each submodule. Enumerating config and the index runs no filter.
    /// A driver whose name contains `=` cannot be addressed by `-c` (Git splits the
    /// setting at the first `=`), so such configuration is refused.
    async fn filter_overrides(&self, cwd: &Path) -> Result<Vec<String>, GitError> {
        let mut drivers = BTreeSet::new();
        self.filter_drivers(cwd, &mut drivers).await?;
        let mut pending = vec![(cwd.to_path_buf(), 0usize)];
        let mut visited = 0usize;
        while let Some((repository, depth)) = pending.pop() {
            for submodule in self.populated_submodules(&repository).await? {
                // Git would recurse into every one of them; beyond the bounds the
                // remaining drivers are unknown, so inspection refuses to run.
                visited += 1;
                if visited > MAX_SUBMODULES || depth >= MAX_SUBMODULE_DEPTH {
                    return Err(too_many_submodules());
                }
                self.filter_drivers(&submodule, &mut drivers).await?;
                pending.push((submodule, depth + 1));
            }
        }
        if let Some(driver) = drivers.iter().find(|driver| driver.contains('=')) {
            return Err(GitError::UnsafeConfiguration(format!(
                "filter driver {driver:?} cannot be disabled for inspection because its name contains '='; remove or rename that filter.<driver> configuration"
            )));
        }
        Ok(drivers
            .iter()
            .flat_map(|driver| {
                [
                    format!("filter.{driver}.clean="),
                    format!("filter.{driver}.smudge="),
                    format!("filter.{driver}.process="),
                    format!("filter.{driver}.required=false"),
                ]
            })
            .collect())
    }

    /// Add the `filter.<driver>` names configured for the repository at `cwd`.
    async fn filter_drivers(
        &self,
        cwd: &Path,
        drivers: &mut BTreeSet<String>,
    ) -> Result<(), GitError> {
        let listed = self
            .git(
                Some(cwd),
                &["config", "-z", "--get-regexp", r"^filter\."],
                &[],
            )
            .await?;
        // Exit status 1 means no filter is configured.
        if listed.status == Some(1) {
            return Ok(());
        }
        if listed.status != Some(0) {
            return Err(GitError::Command {
                command: "config".into(),
                detail: listed.stderr,
            });
        }
        // With -z each entry is `key` NUL or `key` LF `value` NUL, so a subsection
        // containing whitespace is read exactly.
        for entry in listed.stdout.split(|byte| *byte == 0) {
            let key = entry.split(|byte| *byte == b'\n').next().unwrap_or(entry);
            let key = String::from_utf8_lossy(key);
            let driver = key
                .strip_prefix("filter.")
                .and_then(|rest| rest.rsplit_once('.'))
                .map(|(driver, _attribute)| driver)
                .filter(|driver| !driver.is_empty());
            if let Some(driver) = driver {
                drivers.insert(driver.to_owned());
            }
        }
        Ok(())
    }

    /// Populated submodules recorded as gitlinks in the index of the repository at `cwd`.
    async fn populated_submodules(&self, cwd: &Path) -> Result<Vec<PathBuf>, GitError> {
        let mut child = self
            .command(Some(cwd), &["ls-files", "--stage", "-z"], &[])
            .spawn()?;
        let stdout_pipe = child.stdout.take().expect("stdout is piped");
        let stderr_pipe = child.stderr.take().expect("stderr is piped");
        let max = self.max_output_bytes;
        let read = async {
            tokio::try_join!(
                scan_gitlinks(stdout_pipe, |link| populated_submodule(cwd, link)),
                capped(stderr_pipe, max)
            )
        };
        let (links, stderr) = match tokio::time::timeout(self.command_timeout, read).await {
            Ok(Ok(result)) => result,
            Ok(Err(error)) => {
                let _ = child.kill().await;
                return Err(error);
            }
            Err(_) => {
                let _ = child.kill().await;
                return Err(GitError::Timeout("ls-files".into()));
            }
        };
        let status = match tokio::time::timeout(self.command_timeout, child.wait()).await {
            Ok(status) => status?,
            Err(_) => {
                let _ = child.kill().await;
                return Err(GitError::Timeout("ls-files".into()));
            }
        };
        if status.code() != Some(0) {
            return Err(GitError::Command {
                command: "ls-files".into(),
                detail: String::from_utf8_lossy(&stderr).chars().take(400).collect(),
            });
        }
        Ok(links)
    }

    /// The first configured value of `key`, read from configuration rather than
    /// through `git remote get-url`, which applies machine-local `insteadOf` rewrites.
    async fn configured(
        &self,
        cwd: &Path,
        key: &str,
        overrides: &[String],
    ) -> Result<Option<String>, GitError> {
        let output = self
            .git(Some(cwd), &["config", "-z", "--get-all", key], overrides)
            .await?;
        if output.status != Some(0) {
            return Ok(None);
        }
        Ok(output
            .stdout
            .split(|byte| *byte == 0)
            .next()
            .map(|value| String::from_utf8_lossy(value).trim().to_owned())
            .filter(|value| !value.is_empty()))
    }

    /// Whether `refs/remotes/origin/<branch>` exists; read-only and exact.
    pub async fn has_remote_branch(&self, root: &Path, branch: &str) -> Result<bool, GitError> {
        if !luuna_domain::valid_branch_name(branch) {
            return Ok(false);
        }
        let reference = format!("refs/remotes/origin/{branch}");
        let output = self
            .git(
                Some(root),
                &["show-ref", "--verify", "--quiet", &reference],
                &[],
            )
            .await?;
        Ok(output.status == Some(0))
    }

    /// Exact tree identity of a commit in a repository root; read-only.
    pub async fn commit_tree(&self, root: &Path, commit: &str) -> Result<String, GitError> {
        let spec = format!("{commit}^{{tree}}");
        self.optional(root, &["rev-parse", "--verify", "--quiet", &spec], &[])
            .await?
            .filter(|t| t.len() == 40 || t.len() == 64)
            .ok_or_else(|| GitError::Command {
                command: "rev-parse".into(),
                detail: format!("commit {commit} has no tree"),
            })
    }

    pub async fn version(&self) -> Result<String, GitError> {
        let output = self.git(None, &["--version"], &[]).await?;
        if output.status != Some(0) {
            return Err(GitError::Command {
                command: "--version".into(),
                detail: output.stderr,
            });
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
    }

    /// Observe a registered repository root without modifying it, to compare it
    /// with its registration: `identity.root_commits` lists the roots reachable
    /// from HEAD or any local branch, which must contain the registered roots.
    pub async fn inspect(&self, path: &Path) -> Result<RepositoryObservation, GitError> {
        self.observe(path, Roots::Reachable).await
    }

    /// Observe a repository root for registration (or an explicit rebind): its
    /// identity records the roots reachable from HEAD only. The path must be the
    /// repository's top level so that registration identity stays stable.
    pub async fn inspect_for_registration(
        &self,
        path: &Path,
    ) -> Result<RepositoryObservation, GitError> {
        self.observe(path, Roots::Head).await
    }

    async fn observe(&self, path: &Path, roots: Roots) -> Result<RepositoryObservation, GitError> {
        if !path.is_absolute() || !path.is_dir() {
            return Err(GitError::InvalidPath);
        }
        let canonical = std::fs::canonicalize(path).map_err(|_| GitError::InvalidPath)?;
        let git_version = self.version().await?;
        // Git for Windows does not accept `\\?\` verbatim paths; use the plain form for -C.
        let canonical = strip_verbatim_prefix(&canonical);
        let probe = self
            .git(
                Some(&canonical),
                &[
                    "rev-parse",
                    "--is-bare-repository",
                    "--is-shallow-repository",
                ],
                &[],
            )
            .await?;
        if probe.status != Some(0) {
            return Err(probe_error(&canonical, &probe.stderr));
        }
        let text = String::from_utf8_lossy(&probe.stdout);
        let mut lines = text.lines();
        if lines.next() == Some("true") {
            return Err(GitError::Bare);
        }
        let shallow = lines.next() == Some("true");
        let toplevel = self
            .git(Some(&canonical), &["rev-parse", "--show-toplevel"], &[])
            .await?;
        if toplevel.status != Some(0) || toplevel.stdout.is_empty() {
            return Err(GitError::NotRepository);
        }
        let toplevel = output_path(&toplevel.stdout);
        let toplevel_path = std::fs::canonicalize(&toplevel)
            .map(|p| strip_verbatim_prefix(&p))
            .map_err(|_| GitError::NotRepository)?;
        if toplevel_path != canonical {
            return Err(GitError::NotRoot(toplevel.display().to_string()));
        }
        // `.gitattributes` can select `filter.<driver>.clean`/`.process` commands that
        // `git status` would run while re-hashing changed files; disable every
        // configured driver before any worktree content is read.
        let overrides = self.filter_overrides(&canonical).await?;
        let head_commit = self
            .optional(
                &canonical,
                &["rev-parse", "--verify", "--quiet", "HEAD^{commit}"],
                &overrides,
            )
            .await?
            .filter(|c| c.len() == 40 || c.len() == 64)
            .ok_or(GitError::Unborn)?;
        // Full reference names: `--short` would print `heads/main` when a tag is
        // also called `main`.
        let branch = self
            .optional(&canonical, &["symbolic-ref", "--quiet", "HEAD"], &overrides)
            .await?
            .and_then(|r| r.strip_prefix("refs/heads/").map(ToOwned::to_owned))
            .filter(|b| !b.is_empty());
        // The configured URLs, not `remote get-url`, which applies machine-local
        // `url.<base>.insteadOf` rewrites. Filesystem origins are not portable
        // identities; they are observed as absent so discovery proposes an
        // explicit local: identity instead.
        let fetch_url = self
            .configured(&canonical, "remote.origin.url", &overrides)
            .await?
            .map(|r| redact_remote(&r));
        let push_url = self
            .configured(&canonical, "remote.origin.pushurl", &overrides)
            .await?
            .map(|r| redact_remote(&r));
        let remote = fetch_url.clone().filter(|r| !is_filesystem_remote(r));
        let push_remote = push_url
            .filter(|p| {
                !fetch_url
                    .as_ref()
                    .is_some_and(|fetch| fetch == p || remotes_equivalent(fetch, p))
            })
            .filter(|p| !is_filesystem_remote(p));
        let remote_default_branch = self
            .optional(
                &canonical,
                &["symbolic-ref", "--quiet", "refs/remotes/origin/HEAD"],
                &overrides,
            )
            .await?
            .and_then(|r| {
                r.strip_prefix("refs/remotes/origin/")
                    .map(ToOwned::to_owned)
            })
            .filter(|b| !b.is_empty());
        let mut root_commits: Vec<String> = if shallow {
            Vec::new()
        } else {
            let mut args = vec!["rev-list", "--max-parents=0", "HEAD"];
            if roots == Roots::Reachable {
                args.push("--branches");
            }
            self.expect(&canonical, &args, &overrides)
                .await?
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .map(ToOwned::to_owned)
                .collect()
        };
        root_commits.sort();
        root_commits.dedup();
        // Identity must cover every root; truncating would let a history change slip past.
        let limit = match roots {
            Roots::Head => MAX_ROOT_COMMITS,
            Roots::Reachable => MAX_REACHABLE_ROOTS,
        };
        if root_commits.len() > limit {
            return Err(GitError::TooManyRoots(root_commits.len()));
        }
        let local_branches = self
            .expect(
                &canonical,
                &["for-each-ref", "--format=%(refname)", "refs/heads/"],
                &overrides,
            )
            .await?
            .lines()
            .filter_map(|l| l.trim_end_matches('\r').strip_prefix("refs/heads/"))
            .filter(|l| !l.is_empty())
            .map(ToOwned::to_owned)
            .collect();
        let status = self
            .git(
                Some(&canonical),
                &[
                    "status",
                    "--porcelain=v2",
                    "-z",
                    "--untracked-files=normal",
                    "--no-renames",
                ],
                &overrides,
            )
            .await?;
        if status.status != Some(0) {
            return Err(GitError::Command {
                command: "status".into(),
                detail: status.stderr,
            });
        }
        let observation = RepositoryObservation {
            local_path: canonical.to_string_lossy().into_owned(),
            identity: RepositoryIdentity {
                remote,
                push_remote,
                root_commits,
                shallow,
            },
            head_commit,
            branch,
            remote_default_branch,
            local_branches,
            dirty: parse_porcelain_v2(&status.stdout),
            git_version,
            observed_at: Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
        };
        observation.validate()?;
        Ok(observation)
    }
}

/// `std::fs::canonicalize` returns `\\?\C:\…` on Windows; Git and users expect `C:\…`.
pub fn strip_verbatim_prefix(path: &Path) -> PathBuf {
    let text = path.to_string_lossy();
    if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{rest}"));
    }
    if let Some(rest) = text.strip_prefix(r"\\?\") {
        return PathBuf::from(rest);
    }
    path.to_path_buf()
}

/// Strip user information, query strings and fragments from remotes so tokens never persist.
pub fn redact_remote(remote: &str) -> String {
    let remote = remote.split(['?', '#']).next().unwrap_or(remote);
    if let Some((scheme, rest)) = remote.split_once("://") {
        if let Some((authority, path)) = rest.split_once('/') {
            let host = authority.rsplit('@').next().unwrap_or(authority);
            return format!("{scheme}://{host}/{path}");
        }
        let host = rest.rsplit('@').next().unwrap_or(rest);
        return format!("{scheme}://{host}");
    }
    remote.to_owned()
}

/// `git status --porcelain=v2 -z --no-renames` entries: `1 XY ...`, `u XY ...`, `? path`, `! path`.
pub fn parse_porcelain_v2(bytes: &[u8]) -> DirtyState {
    let mut state = DirtyState::default();
    for entry in bytes.split(|b| *b == 0).filter(|e| !e.is_empty()) {
        let text = String::from_utf8_lossy(entry);
        let mut fields = text.splitn(2, ' ');
        let kind = fields.next().unwrap_or("");
        let rest = fields.next().unwrap_or("");
        let path = match kind {
            "1" | "2" => {
                let xy = rest.get(0..2).unwrap_or("..");
                if !xy.starts_with('.') {
                    state.staged += 1;
                }
                if !xy.ends_with('.') {
                    state.unstaged += 1;
                }
                let columns = if kind == "1" { 8 } else { 9 };
                rest.splitn(columns, ' ').last().unwrap_or("")
            }
            "u" => {
                state.conflicted += 1;
                rest.splitn(10, ' ').last().unwrap_or("")
            }
            "?" => {
                state.untracked += 1;
                rest
            }
            _ => continue,
        };
        if state.paths.len() < MAX_RECORDED_DIRTY_PATHS {
            state.paths.push(path.to_owned());
        } else {
            state.truncated = true;
        }
    }
    state
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verbatim_prefixes_are_removed_for_git_and_display() {
        assert_eq!(
            strip_verbatim_prefix(Path::new(r"\\?\C:\work\repo")),
            PathBuf::from(r"C:\work\repo")
        );
        assert_eq!(
            strip_verbatim_prefix(Path::new(r"\\?\UNC\server\share\repo")),
            PathBuf::from(r"\\server\share\repo")
        );
        assert_eq!(
            strip_verbatim_prefix(Path::new("/srv/repo")),
            PathBuf::from("/srv/repo")
        );
    }

    #[test]
    fn probe_failures_keep_gits_reason() {
        let path = Path::new("/srv/app");
        assert!(matches!(
            probe_error(
                path,
                "fatal: detected dubious ownership in repository at '/srv/app'\nTo add an exception for this directory, call:\n\n\tgit config --global --add safe.directory /srv/app"
            ),
            GitError::UnsafeOwnership(owned) if owned == "/srv/app"
        ));
        assert!(matches!(
            probe_error(
                path,
                "fatal: not a git repository (or any of the parent directories): .git"
            ),
            GitError::NotRepository
        ));
        let other = probe_error(
            path,
            "fatal: unknown repository format version 9\n\textension: x",
        )
        .to_string();
        assert!(
            other.contains("unknown repository format version 9 extension: x"),
            "{other}"
        );
        assert!(GitError::UnsafeOwnership("/srv/app".into())
            .to_string()
            .contains("safe.directory"));
    }

    #[tokio::test]
    async fn gitlinks_are_scanned_from_a_stream_without_buffering_it() {
        let listing = b"100644 1111111111111111111111111111111111111111 0\tREADME.md\0\
160000 2222222222222222222222222222222222222222 0\tvendor/lib\0\
160000 3333333333333333333333333333333333333333 1\tconflicted\0\
160000 4444444444444444444444444444444444444444 2\tconflicted\0\
100644 5555555555555555555555555555555555555555 0\tspace name\0";
        let every = |link: &[u8]| Some(link.to_vec());
        let links = scan_gitlinks(&listing[..], every).await.unwrap();
        assert_eq!(links, vec![b"vendor/lib".to_vec(), b"conflicted".to_vec()]);
        let mut many = Vec::new();
        for index in 0..=MAX_SUBMODULES {
            many.extend_from_slice(format!("160000 {} 0\tm{index}\0", "a".repeat(40)).as_bytes());
        }
        assert!(matches!(
            scan_gitlinks(&many[..], every).await,
            Err(GitError::UnsafeConfiguration(_))
        ));
        // Only populated submodules count toward the bound.
        let none = |_: &[u8]| None::<Vec<u8>>;
        assert!(scan_gitlinks(&many[..], none).await.unwrap().is_empty());
        // An overlong gitlink record is refused rather than silently skipped.
        let long = format!("160000 {} 0\t{}\0", "a".repeat(40), "p".repeat(70 * 1024));
        assert!(matches!(
            scan_gitlinks(long.as_bytes(), every).await,
            Err(GitError::Oversized)
        ));
    }

    #[test]
    fn remote_redaction_removes_credentials_only() {
        assert_eq!(
            redact_remote("https://user:tok3n@github.com/org/repo.git"),
            "https://github.com/org/repo.git"
        );
        assert_eq!(
            redact_remote("ssh://git@github.com/org/repo.git"),
            "ssh://github.com/org/repo.git"
        );
        assert_eq!(
            redact_remote("git@github.com:org/repo.git"),
            "git@github.com:org/repo.git"
        );
        assert_eq!(redact_remote("https://x@host"), "https://host");
        assert_eq!(
            redact_remote("https://host/org/repo.git?access_token=secret#frag"),
            "https://host/org/repo.git"
        );
        assert_eq!(
            redact_remote("git@github.com:org/repo.git?token=secret"),
            "git@github.com:org/repo.git"
        );
    }

    #[test]
    fn porcelain_v2_counts_each_change_class() {
        let raw = b"1 M. N... 100644 100644 100644 abc def staged.txt\0\
1 .M N... 100644 100644 100644 abc def unstaged.txt\0\
1 MM N... 100644 100644 100644 abc def both.txt\0\
u UU N... 100644 100644 100644 100644 a b c conflicted.txt\0\
? new dir/\0! ignored.log\0";
        let state = parse_porcelain_v2(raw);
        assert_eq!(
            (
                state.staged,
                state.unstaged,
                state.untracked,
                state.conflicted
            ),
            (2, 2, 1, 1)
        );
        assert_eq!(
            state.paths,
            vec![
                "staged.txt",
                "unstaged.txt",
                "both.txt",
                "conflicted.txt",
                "new dir/"
            ]
        );
        assert!(!state.truncated);
        assert!(parse_porcelain_v2(b"").is_clean());
    }
}
