//! Supervised execution of confirmed check recipes (M2b).
//!
//! A recipe is an executable plus an argument array, never shell text. The
//! executor resolves the executable, verifies declared host capabilities,
//! runs the command in an explicit directory under the repository root with
//! a secret-stripped environment and a deadline, owns the whole process
//! group (a kill-on-close job object on Windows), streams bounded output into
//! private artifacts, hashes them, and maps the outcome to a `CheckStatus`.
//! Native execution has host permissions; nothing here is an
//! operating-system sandbox.
use chrono::{SecondsFormat, Utc};
use luuna_domain::CheckStatus;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    process::Stdio,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};
use thiserror::Error;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::{Child, Command},
    sync::{mpsc, watch},
};
mod process;

/// Display-only progress is bounded and never backpressures a child's pipes.
pub const PROGRESS_CAPACITY: usize = 64;

/// Progress slots output chunks never take, so the final `Reaped` record
/// still fits when both streams filled the channel.
const PROGRESS_RESERVED: usize = 2;

/// Bytes kept per output stream in the private artifact; the rest is drained and counted.
pub const MAX_ARTIFACT_BYTES: u64 = 16 * 1024 * 1024;
/// Bytes of trailing output retained for display.
pub const EXCERPT_BYTES: usize = 4096;
/// Environment variable carrying a check's process-group marker. Descendants
/// inherit it, so restart recovery can tell a surviving check process from an
/// unrelated one that reused its process-group ID.
pub const GROUP_MARKER_VARIABLE: &str = "LUUNA_CHECK_GROUP";
const VERSION_PROBE_TIMEOUT: Duration = Duration::from_secs(15);
/// Bytes of version output read per stream; more fails the probe.
const VERSION_PROBE_OUTPUT_LIMIT: u64 = 64 * 1024;
const REAP_TIMEOUT: Duration = Duration::from_secs(10);
/// How long terminated survivors are given to disappear before the result is recorded.
#[cfg_attr(not(unix), allow(dead_code))]
const SURVIVOR_WAIT: Duration = Duration::from_secs(2);
/// Longest output line the summary scanner keeps; longer lines are never runner summaries.
const MAX_SCANNED_LINE: usize = 4096;
/// Default grace for descendants that keep a pipe open after the child exits.
pub const DRAIN_GRACE: Duration = Duration::from_secs(5);
/// What a reason for a missing tool or dependency asks the developer to do.
const PREPARATION_ADVICE: &str = "add a dependency-preparation recipe (for example `npm ci` or `python -m pip install -r requirements.txt`) before the tests in the manifest, since candidate checks run in a fresh worktree without ignored files such as node_modules/ or .venv/";

#[derive(Debug, Error)]
pub enum ExecutorError {
    #[error("I/O: {0}")]
    Io(#[from] std::io::Error),
    #[error("artifact directory is invalid: {0}")]
    Artifacts(String),
}

#[derive(Debug, Clone)]
pub struct CommandSpec {
    pub recipe_id: String,
    /// Command name resolved on `PATH`, or a repository-relative path such as `./gradlew`.
    pub executable: String,
    pub args: Vec<String>,
    /// Canonical registered repository root.
    pub repository_root: PathBuf,
    /// Working directory relative to the root; `.` is the root itself.
    pub cwd: String,
    pub timeout: Duration,
    /// Remaining workflow time. When it ends the check before `timeout`
    /// would, the check is recorded as blocked by the exhausted budget, never
    /// as a failed assertion.
    pub budget: Option<Duration>,
    pub capabilities: Vec<String>,
    /// Private directory that receives the recipe's stdout and stderr logs
    /// (see [`artifact_file_name`]).
    pub artifact_dir: PathBuf,
    /// Extra variables for the check (M4 dependency bindings such as
    /// `LUUNA_DEPENDENCY_<REPOSITORY>_PATH`), set after the sanitized host
    /// environment. Values are runtime facts, never repository text.
    pub environment: Vec<(String, String)>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Artifact {
    pub name: String,
    pub path: PathBuf,
    /// Digest of the retained bytes, the artifact file's content.
    pub sha256: String,
    /// Bytes the stream produced in total.
    pub bytes: u64,
    /// Bytes kept in the artifact file; fewer than `bytes` when truncated.
    pub retained_bytes: u64,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Execution {
    pub status: CheckStatus,
    pub exit_code: Option<i32>,
    pub reason: Option<String>,
    pub command: Vec<String>,
    pub resolved_executable: Option<PathBuf>,
    pub tool_version: Option<String>,
    pub cwd: PathBuf,
    pub pid: Option<u32>,
    pub started_at: String,
    pub finished_at: String,
    pub artifacts: Vec<Artifact>,
    pub stdout_excerpt: String,
    pub stderr_excerpt: String,
    pub tests_detected: Option<u64>,
    pub environment_fingerprint: String,
    /// The workflow time budget, not the recipe's own deadline, ended the check.
    pub budget_exhausted: bool,
    /// Processes still running in the check's process tree after it settled,
    /// which the executor terminated before returning.
    pub reaped_descendants: Vec<String>,
}

/// Durable identity of a check's process tree, recorded when it starts so
/// restart recovery can find descendants that outlived a hard-killed runtime
/// and tell them apart from unrelated processes that reused the ID.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessGroup {
    /// Process-group ID on Unix (the direct child's PID); the child's PID elsewhere.
    pub pgid: u32,
    /// The group leader's start time in the platform's own units, when readable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub leader_start: Option<String>,
    /// The Linux boot that `leader_start` is relative to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub boot_id: Option<String>,
    /// Value of `LUUNA_CHECK_GROUP` that every descendant inherits unless it
    /// clears its environment.
    pub marker: String,
    /// The operating system terminates the whole tree when the runtime exits
    /// (a Windows kill-on-close job object).
    #[serde(default)]
    pub contained_on_exit: bool,
}

/// What restart recovery found for a recorded process group.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GroupReconciliation {
    /// No process of the group is running.
    Gone,
    /// Verified survivors of the recorded check, terminated.
    Reaped(Vec<String>),
    /// Processes that may belong to the group but could not be verified as
    /// the recorded check's, so none was signalled.
    Unverified {
        processes: Vec<String>,
        reason: String,
    },
}

/// Live notifications for event streams; never authoritative state.
#[derive(Debug, Clone)]
pub enum Progress {
    Started {
        pid: u32,
        command: Vec<String>,
        group: ProcessGroup,
    },
    Output {
        stream: &'static str,
        chunk: String,
    },
    /// Processes left in the check's tree after it settled, now terminated.
    Reaped {
        descendants: Vec<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Probe {
    Resolved {
        resolved: PathBuf,
        tool_version: Option<String>,
        fingerprint: String,
    },
    Unavailable(String),
}

#[derive(Debug, Clone)]
pub struct Executor {
    /// Overrides `PATH` for executable and capability resolution (tests, profiles).
    pub search_path: Option<OsString>,
    /// How long output pipes may stay open after the direct child has exited
    /// before the remaining process group is terminated and capture stops.
    pub drain_grace: Duration,
}

impl Default for Executor {
    fn default() -> Self {
        Self {
            search_path: None,
            drain_grace: DRAIN_GRACE,
        }
    }
}

/// Host capabilities a recipe may declare. Unknown names are descriptive tags.
const CAPABILITY_TOOLS: [(&str, &[&str]); 16] = [
    ("cargo", &["cargo"]),
    ("rust", &["rustc"]),
    ("clippy", &["cargo-clippy"]),
    ("node", &["node"]),
    ("npm", &["npm"]),
    ("pnpm", &["pnpm"]),
    ("yarn", &["yarn"]),
    ("bun", &["bun"]),
    ("go", &["go"]),
    ("python", &["python3", "python"]),
    ("pytest", &["pytest"]),
    ("flutter", &["flutter"]),
    ("dart", &["dart"]),
    ("jdk", &["java"]),
    ("xcode", &["xcodebuild"]),
    ("git", &["git"]),
];

const VERSION_PROBE_TOOLS: [&str; 14] = [
    "cargo", "rustc", "node", "npm", "pnpm", "yarn", "bun", "go", "python3", "python", "pytest",
    "flutter", "dart", "git",
];

/// Arguments that make a probed tool print its version; `go` has no
/// `--version` flag and reports an error for it.
fn version_arguments(tool: &str) -> &'static [&'static str] {
    match tool {
        "go" => &["version"],
        _ => &["--version"],
    }
}

/// What a version probe established.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ToolVersion {
    /// The tool is not one Luuna probes.
    NotProbed,
    /// The probe exited 0; the first line of its output, when it printed one.
    Reported(Option<String>),
    /// The probe could not start, timed out or exited non-zero. Its output is
    /// never recorded as a version, and the failure is part of the fingerprint.
    Failed(String),
}

impl ToolVersion {
    fn text(&self) -> Option<&str> {
        match self {
            Self::Reported(version) => version.as_deref(),
            _ => None,
        }
    }
}

fn now() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

/// `sha256:` plus the lower-case hex SHA-256 of `bytes`, the digest form
/// artifact identities and check results record.
pub fn sha256_hex(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

/// Whether a variable name looks like it carries a credential. Checks never
/// receive such variables, and repair feedback redacts their values.
pub fn is_credential_name(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    [
        "API_KEY",
        "APIKEY",
        "TOKEN",
        "SECRET",
        "PASSWORD",
        "PASSWD",
        "CREDENTIAL",
        "PRIVATE_KEY",
        "AUTH",
        "JWT",
        "PASSPHRASE",
        "BEARER",
        "COOKIE",
        "SESSION_KEY",
        "ACCESS_KEY",
        // Webhook URLs, connection strings and database URLs embed their credential.
        "WEBHOOK",
        "CONNECTION_STRING",
        "CONNSTR",
        "DATABASE_URL",
    ]
    .iter()
    .any(|marker| upper.contains(marker))
        // Unlocked vault sessions (Bitwarden, 1Password CLI) open every stored secret.
        || upper == "BW_SESSION"
        || upper.starts_with("OP_SESSION_")
        // A password, personal access token or data source name: `DB_PASS`,
        // `MYSQL_PWD`, `GITHUB_PAT`, `SENTRY_DSN`. `PWD` alone is the working
        // directory and has no underscore.
        || ["_PASS", "_PWD", "_PAT", "_DSN"]
            .iter()
            .any(|suffix| upper.ends_with(suffix))
        // A `_KEY` name (`STRIPE_KEY`, `SSH_KEY`, `AWS_ACCESS_KEY_ID`) is
        // treated as a key whatever it is for; such names are rarely anything
        // a check needs.
        || upper.ends_with("_KEY")
        || upper.contains("_KEY_")
        || upper.ends_with("_KEYS")
}

/// Variables that must never reach a check: Luuna's own configuration,
/// repository-redirecting Git variables and anything that looks like a credential.
pub fn is_secret_or_reserved(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    upper.starts_with("LUUNA_") || upper.starts_with("GIT_") || is_credential_name(name)
}

/// Whether a line of check output looks like it carries a credential: an
/// authorization header, a secret-key or registry-token assignment, or a
/// well-known token prefix at a word boundary followed by at least as many
/// token characters as that format has. Best effort (ADR 0009): the
/// developer-confirmed manifest bounds what a check may read.
pub fn line_carries_token(line: &str) -> bool {
    // Each prefix with the shortest token run its real format can have.
    const PREFIXES: [(&str, usize); 21] = [
        ("sk-", 20),
        ("sk_live_", 20),
        ("rk_live_", 20),
        ("AKIA", 16),
        ("ASIA", 16),
        ("ghp_", 20),
        ("gho_", 20),
        ("ghu_", 20),
        ("ghs_", 20),
        ("ghr_", 20),
        ("github_pat_", 20),
        ("glpat-", 20),
        ("xoxb-", 20),
        ("xoxp-", 20),
        ("xoxa-", 20),
        ("xoxs-", 20),
        ("xapp-", 20),
        ("npm_", 30),
        ("hf_", 30),
        ("AIza", 30),
        ("pypi-", 30),
    ];
    let lowered = line.to_ascii_lowercase();
    if [
        "authorization: bearer ",
        "authorization: basic ",
        "authorization: token ",
        "private_key",
        "aws_secret_access_key",
        "_authtoken=",
    ]
    .iter()
    .any(|marker| lowered.contains(marker))
    {
        return true;
    }
    PREFIXES.iter().any(|(prefix, minimum)| {
        line.match_indices(prefix).any(|(at, _)| {
            let boundary = line[..at]
                .chars()
                .next_back()
                .is_none_or(|c| !c.is_ascii_alphanumeric());
            let run = line[at + prefix.len()..]
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'))
                .count();
            boundary && run >= *minimum
        })
    })
}

/// Every process the executor starts, probes included, sees a cleared
/// environment with Luuna configuration, `GIT_*` and credential-shaped
/// variables removed; nothing repository-controlled runs with inherited secrets.
fn sanitized_environment(command: &mut Command) -> &mut Command {
    command.env_clear().envs(
        std::env::vars_os().filter(|(name, _)| !is_secret_or_reserved(&name.to_string_lossy())),
    )
}

/// Private artifact storage: directories 0700 and files 0600 on Unix even when
/// the parent directory or umask is permissive.
fn create_private_dir(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(path)?;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
    }
    #[cfg(not(unix))]
    {
        std::fs::create_dir_all(path)
    }
}

async fn create_private_file(path: &Path) -> std::io::Result<tokio::fs::File> {
    let mut options = tokio::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    options.mode(0o600);
    let file = options.open(path).await?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))
            .await?;
    }
    Ok(file)
}

/// Executable file names tried for `name` in each search directory, in order.
/// On Windows a bare name is tried only with the `PATHEXT` extensions a
/// process can be started from, in `PATHEXT` order, so the extensionless
/// POSIX shims Node, npm and Flutter install beside their `.cmd` and `.bat`
/// files are never chosen; a name that already carries such an extension is
/// tried as written.
fn path_candidates(name: &str, windows: bool, pathext: Option<&str>) -> Vec<String> {
    if !windows {
        return vec![name.to_owned()];
    }
    const SPAWNABLE: [&str; 4] = [".com", ".exe", ".bat", ".cmd"];
    let parse = |list: &str| {
        let mut extensions: Vec<String> = Vec::new();
        for extension in list.split(';').map(|e| e.trim().to_ascii_lowercase()) {
            if SPAWNABLE.contains(&extension.as_str()) && !extensions.contains(&extension) {
                extensions.push(extension);
            }
        }
        extensions
    };
    let mut extensions = parse(pathext.unwrap_or_default());
    if extensions.is_empty() {
        extensions = parse(".COM;.EXE;.BAT;.CMD");
    }
    let lower = name.to_ascii_lowercase();
    if extensions.iter().any(|e| lower.ends_with(e.as_str())) {
        return vec![name.to_owned()];
    }
    extensions.iter().map(|e| format!("{name}{e}")).collect()
}

/// A regular file the current user could start: on Unix it needs an execute
/// bit, as a shell requires when it searches `PATH`.
fn is_executable_file(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
}

impl Executor {
    fn search_dirs(&self) -> Vec<PathBuf> {
        let path = self
            .search_path
            .clone()
            .or_else(|| std::env::var_os("PATH"))
            .unwrap_or_default();
        std::env::split_paths(&path).collect()
    }

    /// The first executable named `name` on the search path. Empty and
    /// relative search path entries are ignored: they would resolve against
    /// the runtime's own working directory.
    fn find_on_path(&self, name: &str) -> Option<PathBuf> {
        let pathext = std::env::var("PATHEXT").ok();
        let candidates = path_candidates(name, cfg!(windows), pathext.as_deref());
        for dir in self.search_dirs() {
            if !dir.is_absolute() {
                continue;
            }
            for candidate in &candidates {
                let path = dir.join(candidate);
                if is_executable_file(&path) {
                    return Some(path);
                }
            }
        }
        None
    }

    /// Names of declared capabilities this host cannot satisfy.
    pub fn missing_capabilities(&self, capabilities: &[String]) -> Vec<String> {
        capabilities
            .iter()
            .filter(|capability| {
                let name = capability.as_str();
                if name == "android-sdk" {
                    return std::env::var_os("ANDROID_HOME").is_none()
                        && std::env::var_os("ANDROID_SDK_ROOT").is_none();
                }
                match CAPABILITY_TOOLS.iter().find(|(known, _)| *known == name) {
                    Some((_, tools)) => !tools.iter().any(|tool| self.find_on_path(tool).is_some()),
                    None => false,
                }
            })
            .cloned()
            .collect()
    }

    fn resolve_executable(&self, spec: &CommandSpec, cwd: &Path) -> Result<PathBuf, String> {
        let executable = &spec.executable;
        if executable.is_empty() {
            return Err("recipe has no executable".into());
        }
        let path = Path::new(executable);
        if path.is_absolute() {
            return if path.is_file() {
                Ok(path.to_path_buf())
            } else {
                Err(format!("executable {executable} does not exist"))
            };
        }
        if executable.contains('/') || executable.contains('\\') {
            let candidate = cwd.join(path);
            // Typically `node_modules/.bin/...` or `.venv/bin/...`, which an
            // ignored dependency directory provides in the developer checkout only.
            let canonical = std::fs::canonicalize(&candidate).map_err(|_| {
                format!("repository executable {executable} does not exist; if a dependency step installs it, {PREPARATION_ADVICE}")
            })?;
            // Compare canonical forms: on Windows canonicalize yields a verbatim
            // `\\?\` path that would never start with the plain repository root.
            let root = std::fs::canonicalize(&spec.repository_root)
                .unwrap_or_else(|_| spec.repository_root.clone());
            if !canonical.starts_with(&root) || !canonical.is_file() {
                return Err(format!(
                    "repository executable {executable} resolves outside the repository"
                ));
            }
            return Ok(canonical);
        }
        self.find_on_path(executable)
            .ok_or_else(|| format!("executable {executable} was not found on PATH"))
    }

    /// Probe a known tool's version in the recipe's working directory, where
    /// version managers (rustup, asdf, mise, volta, pyenv, corepack) select
    /// the toolchain the recipe will actually run.
    async fn tool_version(&self, resolved: &Path, name: &str, cwd: &Path) -> ToolVersion {
        if !VERSION_PROBE_TOOLS.contains(&name) {
            return ToolVersion::NotProbed;
        }
        let mut command = Command::new(resolved);
        sanitized_environment(&mut command)
            .args(version_arguments(name))
            .current_dir(cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let (mut child, mut owner) = match process::spawn(&mut command).await {
            Ok(owned) => owned,
            Err(error) => return ToolVersion::Failed(format!("could not start: {}", error.kind())),
        };
        let (Some(stdout), Some(stderr)) = (child.stdout.take(), child.stderr.take()) else {
            owner.stop();
            let _ = child.kill().await;
            return ToolVersion::Failed("output pipes unavailable".into());
        };
        // Version output is read with a hard bound so a misbehaving tool
        // cannot grow the runtime's memory.
        let mut stdout = stdout.take(VERSION_PROBE_OUTPUT_LIMIT + 1);
        let mut stderr = stderr.take(VERSION_PROBE_OUTPUT_LIMIT + 1);
        let capture = async {
            let mut out = Vec::new();
            let mut err = Vec::new();
            let (status, _, _) = tokio::try_join!(
                child.wait(),
                stdout.read_to_end(&mut out),
                stderr.read_to_end(&mut err)
            )?;
            Ok::<_, std::io::Error>((status, out, err))
        };
        let result = tokio::time::timeout(VERSION_PROBE_TIMEOUT, capture).await;
        // Nothing a probe started may outlive it.
        owner.stop();
        let _ = child.kill().await;
        let (status, out, err) = match result {
            Err(_) => {
                return ToolVersion::Failed(format!(
                    "timed out after {}s",
                    VERSION_PROBE_TIMEOUT.as_secs()
                ))
            }
            Ok(Err(error)) => return ToolVersion::Failed(format!("failed: {}", error.kind())),
            Ok(Ok(captured)) => captured,
        };
        let limit = VERSION_PROBE_OUTPUT_LIMIT as usize;
        if out.len() > limit || err.len() > limit {
            return ToolVersion::Failed(format!("version output exceeded {limit} bytes"));
        }
        if !status.success() {
            return ToolVersion::Failed(match status.code() {
                Some(code) => format!("exit status {code}"),
                None => "terminated by signal".into(),
            });
        }
        let text = if out.is_empty() { err } else { out };
        ToolVersion::Reported(
            String::from_utf8_lossy(&text)
                .lines()
                .next()
                .map(|line| line.trim().chars().take(200).collect()),
        )
    }

    /// Resolve a recipe's executable and tool version without running the recipe.
    /// Used to fingerprint the environment before a baseline starts.
    pub async fn probe(&self, spec: &CommandSpec) -> Probe {
        let missing = self.missing_capabilities(&spec.capabilities);
        if !missing.is_empty() {
            return Probe::Unavailable(format!("incapable host: missing {}", missing.join(", ")));
        }
        let cwd = match resolve_cwd(&spec.repository_root, &spec.cwd) {
            Ok(cwd) => cwd,
            Err(reason) => return Probe::Unavailable(reason),
        };
        let resolved = match self.resolve_executable(spec, &cwd) {
            Ok(resolved) => resolved,
            Err(reason) => return Probe::Unavailable(reason),
        };
        let tool_name = Path::new(&spec.executable)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let version = self.tool_version(&resolved, &tool_name, &cwd).await;
        Probe::Resolved {
            fingerprint: Self::fingerprint_for(&resolved, &version),
            tool_version: version.text().map(ToOwned::to_owned),
            resolved,
        }
    }

    /// One fingerprint for a set of recipes: order-independent over their parts.
    pub fn combine_fingerprints(parts: &[String]) -> String {
        let mut sorted = parts.to_vec();
        sorted.sort();
        sorted.dedup();
        let identity = serde_json::json!({
            "os": std::env::consts::OS,
            "arch": std::env::consts::ARCH,
            "parts": sorted,
        });
        sha256_hex(identity.to_string().as_bytes())
    }

    /// Deterministic identity of the execution environment that produced evidence.
    pub fn fingerprint(resolved: &Path, tool_version: Option<&str>) -> String {
        let identity = serde_json::json!({
            "os": std::env::consts::OS,
            "arch": std::env::consts::ARCH,
            "executable": resolved.to_string_lossy(),
            "tool_version": tool_version,
        });
        sha256_hex(identity.to_string().as_bytes())
    }

    /// The fingerprint of a probed tool. A failed probe is part of the
    /// identity, so a tool whose probe starts or stops failing changes it.
    fn fingerprint_for(resolved: &Path, version: &ToolVersion) -> String {
        match version {
            ToolVersion::Failed(failure) => {
                let identity = serde_json::json!({
                    "os": std::env::consts::OS,
                    "arch": std::env::consts::ARCH,
                    "executable": resolved.to_string_lossy(),
                    "tool_version": null,
                    "probe_failure": failure,
                });
                sha256_hex(identity.to_string().as_bytes())
            }
            other => Self::fingerprint(resolved, other.text()),
        }
    }

    /// Run one recipe. Never returns a process failure as success; every
    /// non-zero exit, deadline, cancellation or missing prerequisite maps to a
    /// non-passing status with a recorded reason.
    pub async fn execute(
        &self,
        spec: &CommandSpec,
        mut cancel: watch::Receiver<bool>,
        progress: Option<mpsc::Sender<Progress>>,
    ) -> Result<Execution, ExecutorError> {
        let started_at = now();
        let mut command_line = vec![spec.executable.clone()];
        command_line.extend(spec.args.iter().cloned());
        let blocked = |reason: String, cwd: PathBuf, fingerprint: String| Execution {
            status: CheckStatus::Blocked,
            exit_code: None,
            reason: Some(reason),
            command: command_line.clone(),
            resolved_executable: None,
            tool_version: None,
            cwd,
            pid: None,
            started_at: started_at.clone(),
            finished_at: now(),
            artifacts: Vec::new(),
            stdout_excerpt: String::new(),
            stderr_excerpt: String::new(),
            tests_detected: None,
            environment_fingerprint: fingerprint,
            budget_exhausted: false,
            reaped_descendants: Vec::new(),
        };
        let host_fingerprint = Self::fingerprint(Path::new("<unresolved>"), None);
        // Working directory must stay inside the registered root.
        let cwd = match resolve_cwd(&spec.repository_root, &spec.cwd) {
            Ok(cwd) => cwd,
            Err(reason) => {
                return Ok(blocked(
                    reason,
                    spec.repository_root.clone(),
                    host_fingerprint,
                ))
            }
        };
        let missing = self.missing_capabilities(&spec.capabilities);
        if !missing.is_empty() {
            return Ok(blocked(
                format!("incapable host: missing {}", missing.join(", ")),
                cwd,
                host_fingerprint,
            ));
        }
        let resolved = match self.resolve_executable(spec, &cwd) {
            Ok(resolved) => resolved,
            Err(reason) => return Ok(blocked(reason, cwd, host_fingerprint)),
        };
        let tool_name = Path::new(&spec.executable)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let version = self.tool_version(&resolved, &tool_name, &cwd).await;
        let fingerprint = Self::fingerprint_for(&resolved, &version);
        let tool_version = version.text().map(ToOwned::to_owned);

        create_private_dir(&spec.artifact_dir)?;
        let stdout_path = spec
            .artifact_dir
            .join(artifact_file_name(&spec.recipe_id, "stdout"));
        let stderr_path = spec
            .artifact_dir
            .join(artifact_file_name(&spec.recipe_id, "stderr"));

        let marker = group_marker(&spec.recipe_id);
        let mut command = Command::new(&resolved);
        sanitized_environment(&mut command)
            .args(&spec.args)
            .current_dir(&cwd)
            .envs(spec.environment.iter().map(|(name, value)| (name, value)))
            .env("CI", "true")
            .env("LUUNA_CHECK", &spec.recipe_id)
            .env(GROUP_MARKER_VARIABLE, &marker)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(unix)]
        command.process_group(0);
        // If the runtime itself dies, Linux delivers SIGKILL to the direct child so
        // a check cannot outlive its supervisor; grandchildren are found through
        // the recorded process group when the runtime restarts.
        #[cfg(target_os = "linux")]
        // SAFETY: prctl only changes this child's parent-death signal before exec.
        unsafe {
            command.pre_exec(|| {
                libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL);
                Ok(())
            });
        }
        let (mut child, owner) = match process::spawn(&mut command).await {
            Ok(owned) => owned,
            Err(error) => {
                let hint = if error.kind() == std::io::ErrorKind::NotFound {
                    "; an interpreter or dependency it needs may be missing"
                } else {
                    ""
                };
                return Ok(blocked(
                    format!("cannot start {}: {error}{hint}", resolved.display()),
                    cwd,
                    fingerprint,
                ));
            }
        };
        let pid = child.id();
        let tree = Tree::new(owner, pid);
        if let (Some(sender), Some(pid)) = (&progress, pid) {
            let _ = sender.try_send(Progress::Started {
                pid,
                command: command_line.clone(),
                group: ProcessGroup::capture(pid, marker, tree.contained_on_exit()),
            });
        }
        let stdout_pipe = child.stdout.take().expect("stdout piped");
        let stderr_pipe = child.stderr.take().expect("stderr piped");
        let stdout_task = tokio::spawn(capture(
            stdout_pipe,
            stdout_path.clone(),
            "stdout",
            progress.clone(),
        ));
        let stderr_task = tokio::spawn(capture(
            stderr_pipe,
            stderr_path.clone(),
            "stderr",
            progress.clone(),
        ));

        // The workflow budget can end a check before its own deadline does;
        // which bound applied decides how the record reads.
        let deadline = spec
            .budget
            .map_or(spec.timeout, |budget| budget.min(spec.timeout));
        let budget_limited = spec.budget.is_some_and(|budget| budget < spec.timeout);
        enum Outcome {
            Exited(std::process::ExitStatus),
            Cancelled,
            Timeout,
            WaitFailed(String),
        }
        let outcome = tokio::select! {
            biased;
            _ = cancel.changed() => Outcome::Cancelled,
            waited = tokio::time::timeout(deadline, child.wait()) => match waited {
                Ok(Ok(status)) => Outcome::Exited(status),
                Ok(Err(error)) => Outcome::WaitFailed(error.to_string()),
                Err(_) => Outcome::Timeout,
            },
        };
        if !matches!(outcome, Outcome::Exited(_)) {
            tree.terminate(&mut child).await;
        }
        // The pipes close only when every holder exits. A descendant that
        // inherited them must not keep the baseline open: after the grace the
        // whole group is terminated, and capture is abandoned if it still hangs.
        let mut stdout_task = stdout_task;
        let mut stderr_task = stderr_task;
        let mut stdout_slot = None;
        let mut stderr_slot = None;
        let drained = tokio::select! {
            biased;
            _ = cancel.changed(), if !matches!(outcome, Outcome::Cancelled) => false,
            joined = tokio::time::timeout(self.drain_grace, async {
                settle(&mut stdout_task, &mut stdout_slot).await;
                settle(&mut stderr_task, &mut stderr_slot).await;
            }) => joined.is_ok(),
        };
        let held_open = !drained;
        if held_open {
            tree.terminate(&mut child).await;
            let _ = tokio::time::timeout(REAP_TIMEOUT, async {
                settle(&mut stdout_task, &mut stdout_slot).await;
                settle(&mut stderr_task, &mut stderr_slot).await;
            })
            .await;
            for (task, slot, name) in [
                (&stdout_task, &mut stdout_slot, "stdout"),
                (&stderr_task, &mut stderr_slot, "stderr"),
            ] {
                if slot.is_none() {
                    task.abort();
                    *slot = Some(Err(format!(
                        "{name} pipe was held open by a surviving descendant"
                    )));
                }
            }
        }
        // Nothing the check started keeps running once its result is recorded:
        // a background server or daemon left behind by a command that exited
        // normally is terminated and named. Cancellation, the deadline and a
        // held pipe already terminated the whole tree.
        let terminated = held_open || !matches!(outcome, Outcome::Exited(_));
        let reaped_descendants = if terminated {
            Vec::new()
        } else {
            tree.reap_survivors().await
        };
        if let (Some(sender), false) = (&progress, reaped_descendants.is_empty()) {
            let _ = sender.try_send(Progress::Reaped {
                descendants: reaped_descendants.clone(),
            });
        }
        let stdout_capture = stdout_slot.unwrap_or_else(|| Err("stdout capture lost".into()));
        let stderr_capture = stderr_slot.unwrap_or_else(|| Err("stderr capture lost".into()));
        let finished_at = now();
        let mut artifacts = Vec::new();
        let mut excerpts = [String::new(), String::new()];
        let mut scans = Vec::new();
        let mut capture_error = None;
        for (index, (capture, path, name)) in [
            (stdout_capture, &stdout_path, "stdout"),
            (stderr_capture, &stderr_path, "stderr"),
        ]
        .into_iter()
        .enumerate()
        {
            match capture {
                Ok(captured) => {
                    excerpts[index] = captured.excerpt;
                    scans.push(captured.scan);
                    artifacts.push(Artifact {
                        name: name.into(),
                        path: path.clone(),
                        sha256: captured.sha256,
                        bytes: captured.bytes,
                        retained_bytes: captured.retained_bytes,
                        truncated: captured.truncated,
                    });
                }
                Err(error) => capture_error = Some(error),
            }
        }
        // Summaries are read from every line of both whole streams, not from
        // the size-capped artifacts. A stream whose capture was lost may have
        // held the summary, so nothing is inferred from the other one.
        let tests_detected = if capture_error.is_some() {
            None
        } else {
            scans
                .iter()
                .map(|scan| scan.tests.total())
                .fold(None, |total, count| match (total, count) {
                    (None, None) => None,
                    (total, count) => Some(total.unwrap_or(0) + count.unwrap_or(0)),
                })
        };
        // Cancellation and the deadline are authoritative outcomes; a capture
        // problem (for example a descendant still holding a pipe on a host without
        // process groups) is recorded in the reason, never as a different status.
        let capture_note = capture_error
            .as_deref()
            .map(|error| format!("; output capture incomplete: {error}"))
            .unwrap_or_default();
        let stopped = tree.stopped();
        let reaped = if tree.contained() { " and reaped" } else { "" };
        let mut budget_exhausted = false;
        let (status, exit_code, reason) = match outcome {
            Outcome::Cancelled => (
                CheckStatus::Cancelled,
                None,
                Some(format!("cancelled; {stopped}{reaped}{capture_note}")),
            ),
            Outcome::Timeout if budget_limited => {
                budget_exhausted = true;
                (
                    CheckStatus::Blocked,
                    None,
                    Some(format!(
                        "workflow time budget exhausted after {}s, before the recipe's own {}s deadline; {stopped}; re-plan required{capture_note}",
                        deadline.as_secs(),
                        spec.timeout.as_secs()
                    )),
                )
            }
            Outcome::Timeout => (
                CheckStatus::Failed,
                None,
                Some(format!(
                    "deadline of {}s exceeded; {stopped}{capture_note}",
                    spec.timeout.as_secs()
                )),
            ),
            _ if capture_error.is_some() => (
                CheckStatus::Failed,
                None,
                Some(format!(
                    "output capture failed: {}",
                    capture_error.unwrap_or_default()
                )),
            ),
            Outcome::Exited(status) if held_open => (
                CheckStatus::Failed,
                status.code(),
                Some(format!(
                    "a descendant process kept the output open for {}s after the command exited; {stopped}",
                    self.drain_grace.as_secs()
                )),
            ),
            Outcome::WaitFailed(error) => (CheckStatus::Failed, None, Some(error)),
            Outcome::Exited(status) => match status.code() {
                Some(0) if tests_detected == Some(0) => (
                    CheckStatus::Blocked,
                    Some(0),
                    Some("empty suite: the command succeeded but its test runner reported that no test was executed (none found, all skipped or all deselected)".into()),
                ),
                Some(0) => (CheckStatus::Passed, Some(0), None),
                Some(code) => match missing_dependency(code, &scans, &cwd, &spec.repository_root)
                {
                    // Infrastructure, not an assertion: no repair can fix it.
                    Some(missing) => (CheckStatus::Blocked, Some(code), Some(missing)),
                    None => (
                        CheckStatus::Failed,
                        Some(code),
                        Some(format!("exit code {code}")),
                    ),
                },
                None => (
                    CheckStatus::Failed,
                    None,
                    Some("terminated by signal".into()),
                ),
            },
        };
        let [stdout_excerpt, stderr_excerpt] = excerpts;
        Ok(Execution {
            status,
            exit_code,
            reason,
            command: command_line,
            resolved_executable: Some(resolved),
            tool_version,
            cwd,
            pid,
            started_at,
            finished_at,
            artifacts,
            stdout_excerpt,
            stderr_excerpt,
            tests_detected,
            environment_fingerprint: fingerprint,
            budget_exhausted,
            reaped_descendants,
        })
    }
}

fn resolve_cwd(root: &Path, relative: &str) -> Result<PathBuf, String> {
    if relative.is_empty()
        || relative.starts_with('/')
        || relative.contains('\\')
        || relative
            .split('/')
            .any(|part| part.is_empty() || part == "..")
    {
        return Err(format!(
            "recipe cwd {relative:?} must be `.` or a relative path"
        ));
    }
    let joined = if relative == "." {
        root.to_path_buf()
    } else {
        root.join(relative)
    };
    let canonical = std::fs::canonicalize(&joined)
        .map_err(|_| format!("recipe cwd {relative} does not exist in the repository"))?;
    let canonical_root =
        std::fs::canonicalize(root).map_err(|_| "repository root does not exist".to_owned())?;
    if !canonical.starts_with(&canonical_root) || !canonical.is_dir() {
        return Err(format!("recipe cwd {relative} escapes the repository"));
    }
    Ok(canonical)
}

/// A fresh, unguessable-enough marker per execution; it identifies, it does not authorize.
fn group_marker(recipe_id: &str) -> String {
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or_default();
    let seed = format!(
        "{}:{nanos}:{}:{recipe_id}",
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    );
    sha256_hex(seed.as_bytes())[7..39].to_owned()
}

impl ProcessGroup {
    fn capture(pid: u32, marker: String, contained_on_exit: bool) -> Self {
        Self {
            pgid: pid,
            leader_start: process_start(pid),
            boot_id: boot_id(),
            marker,
            contained_on_exit,
        }
    }
}

/// The check's process tree as the executor can address it: the process
/// group it created (Unix) or a kill-on-close job object (Windows).
struct Tree {
    #[cfg_attr(not(unix), allow(dead_code))]
    pid: Option<u32>,
    /// Ownership established at spawn (strict on Windows, see `process`):
    /// dropping it stops the whole tree.
    #[cfg_attr(not(windows), allow(dead_code))]
    owner: process::Owner,
}

impl Tree {
    fn new(owner: process::Owner, pid: Option<u32>) -> Self {
        Self { pid, owner }
    }

    /// Whether stopping the tree reaches descendants, not only the direct child.
    fn contained(&self) -> bool {
        #[cfg(unix)]
        {
            self.pid.is_some()
        }
        #[cfg(windows)]
        {
            self.owner.job().is_some()
        }
        #[cfg(not(any(unix, windows)))]
        {
            false
        }
    }

    /// Whether the operating system ends the tree if the runtime itself dies.
    fn contained_on_exit(&self) -> bool {
        #[cfg(windows)]
        {
            self.owner.job().is_some()
        }
        #[cfg(not(windows))]
        {
            false
        }
    }

    /// How the record describes stopping the tree, truthfully per platform.
    fn stopped(&self) -> &'static str {
        match (self.contained(), cfg!(unix)) {
            (true, true) => "process group terminated",
            (true, false) => "process tree terminated",
            (false, _) => {
                "direct child terminated; its descendants are not contained on this platform"
            }
        }
    }

    /// Stop the whole tree and reap the direct child.
    async fn terminate(&self, child: &mut Child) {
        #[cfg(unix)]
        if let Some(pid) = self.pid {
            // SAFETY: killpg on a process-group ID we created with process_group(0).
            unsafe {
                libc::killpg(pid as libc::pid_t, libc::SIGKILL);
            }
        }
        #[cfg(windows)]
        if let Some(job) = self.owner.job() {
            job.terminate();
        }
        let _ = child.start_kill();
        let _ = tokio::time::timeout(REAP_TIMEOUT, child.wait()).await;
    }

    /// Terminate whatever still runs in the tree once the check has settled,
    /// and describe it. The leader has been reaped by now; Linux never reuses
    /// a PID that is still a live group's ID, so signalling a group whose
    /// members were just listed reaches only the check's own processes.
    async fn reap_survivors(&self) -> Vec<String> {
        #[cfg(unix)]
        {
            let Some(pgid) = self.pid else {
                return Vec::new();
            };
            let listed = group_members(pgid);
            // Linux lists every running member; elsewhere any process left in
            // the group counts, whether or not it could be named.
            let present = listed.as_ref().is_some_and(|members| !members.is_empty())
                || (!cfg!(target_os = "linux") && group_exists(pgid));
            if !present {
                return Vec::new();
            }
            // SAFETY: killpg on a process-group ID we created with process_group(0).
            unsafe {
                libc::killpg(pgid as libc::pid_t, libc::SIGKILL);
            }
            let deadline = tokio::time::Instant::now() + SURVIVOR_WAIT;
            while tokio::time::Instant::now() < deadline && group_running(pgid) {
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
            match listed {
                Some(members) if !members.is_empty() => {
                    members.iter().map(Member::describe).collect()
                }
                _ => vec![format!("unidentified processes in process group {pgid}")],
            }
        }
        #[cfg(windows)]
        {
            let Some(job) = self.owner.job() else {
                return Vec::new();
            };
            let active = job.active_processes();
            if active == 0 {
                return Vec::new();
            }
            job.terminate();
            vec![format!("{active} process(es) in the check's job object")]
        }
        #[cfg(not(any(unix, windows)))]
        {
            Vec::new()
        }
    }
}

/// A running (not zombie) process of a process group.
#[derive(Debug, Clone)]
#[cfg_attr(not(unix), allow(dead_code))]
struct Member {
    pid: u32,
    name: String,
    start: Option<String>,
}

#[cfg_attr(not(unix), allow(dead_code))]
impl Member {
    fn describe(&self) -> String {
        format!("pid {} ({})", self.pid, self.name)
    }
}

/// The running members of process group `pgid`, or `None` where this
/// platform cannot list them.
#[cfg_attr(not(unix), allow(dead_code))]
fn group_members(pgid: u32) -> Option<Vec<Member>> {
    #[cfg(target_os = "linux")]
    {
        Some(procfs::members(pgid))
    }
    #[cfg(target_os = "macos")]
    {
        libproc::members(pgid)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = pgid;
        None
    }
}

/// Whether any process, zombies included, is in group `pgid`.
#[cfg(unix)]
fn group_exists(pgid: u32) -> bool {
    // SAFETY: signal 0 only checks that the group exists and may be signalled.
    unsafe { libc::killpg(pgid as libc::pid_t, 0) == 0 }
}

/// Whether a running process remains in group `pgid`.
#[cfg(unix)]
fn group_running(pgid: u32) -> bool {
    match group_members(pgid) {
        Some(members) => !members.is_empty(),
        None => group_exists(pgid),
    }
}

/// The platform's start time of `pid`, used to tell a group leader from a
/// later process that reused its PID.
fn process_start(pid: u32) -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        procfs::stat(pid).map(|stat| stat.start)
    }
    #[cfg(target_os = "macos")]
    {
        libproc::info(pid).map(|info| info.start)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = pid;
        None
    }
}

/// The current Linux boot, which process start times are relative to.
fn boot_id() -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        std::fs::read_to_string("/proc/sys/kernel/random/boot_id")
            .ok()
            .map(|id| id.trim().to_owned())
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

/// Restart recovery for a check whose runtime died while it ran. Survivors
/// of `group` are terminated only when they are verifiably that check's
/// processes: the leader still running with its recorded start time, or (on
/// Linux) the check's group marker in a member's environment. A matching
/// process-group ID alone is never enough, since the ID may have been reused.
pub fn reconcile_process_group(group: &ProcessGroup) -> GroupReconciliation {
    if group.contained_on_exit {
        // The operating system ended the job when the runtime's handle closed.
        return GroupReconciliation::Gone;
    }
    #[cfg(unix)]
    {
        let Some(members) = group_members(group.pgid) else {
            return if group_exists(group.pgid) {
                GroupReconciliation::Unverified {
                    processes: vec![format!("process group {}", group.pgid)],
                    reason:
                        "this platform cannot list a process group's members, so none was signalled"
                            .into(),
                }
            } else {
                GroupReconciliation::Gone
            };
        };
        if members.is_empty() {
            return GroupReconciliation::Gone;
        }
        let same_boot = group.boot_id.is_none() || group.boot_id == boot_id();
        let leader = same_boot
            && group.leader_start.is_some()
            && members
                .iter()
                .any(|m| m.pid == group.pgid && m.start == group.leader_start);
        let marked = !group.marker.is_empty()
            && members.iter().any(|m| carries_marker(m.pid, &group.marker));
        let processes = members.iter().map(Member::describe).collect();
        if !(leader || marked) {
            return GroupReconciliation::Unverified {
                processes,
                reason: "the process-group ID now belongs to processes that cannot be verified as this check's, so none was signalled".into(),
            };
        }
        // SAFETY: the group was verified as the recorded check's above.
        unsafe {
            libc::killpg(group.pgid as libc::pid_t, libc::SIGKILL);
        }
        let deadline = std::time::Instant::now() + SURVIVOR_WAIT;
        while std::time::Instant::now() < deadline && group_running(group.pgid) {
            std::thread::sleep(Duration::from_millis(25));
        }
        GroupReconciliation::Reaped(processes)
    }
    #[cfg(not(unix))]
    {
        GroupReconciliation::Unverified {
            processes: vec![format!("process {}", group.pgid)],
            reason: "descendants are not tracked on this platform without a job object, so none was signalled".into(),
        }
    }
}

/// Whether `pid`'s environment carries the check's group marker.
#[cfg(unix)]
fn carries_marker(pid: u32, marker: &str) -> bool {
    #[cfg(target_os = "linux")]
    {
        procfs::carries_marker(pid, marker)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (pid, marker);
        false
    }
}

#[cfg(target_os = "linux")]
mod procfs {
    use super::{Member, GROUP_MARKER_VARIABLE};

    pub(super) struct Stat {
        pub name: String,
        pub state: char,
        pub pgrp: u32,
        /// Clock ticks after boot (field 22 of `/proc/<pid>/stat`).
        pub start: String,
    }

    pub(super) fn stat(pid: u32) -> Option<Stat> {
        let text = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        // The command name is parenthesised and may itself contain spaces or `)`.
        let open = text.find('(')?;
        let close = text.rfind(')')?;
        let name = text.get(open + 1..close)?.to_owned();
        let fields: Vec<&str> = text.get(close + 1..)?.split_whitespace().collect();
        Some(Stat {
            name,
            state: fields.first()?.chars().next()?,
            pgrp: fields.get(2)?.parse().ok()?,
            start: (*fields.get(19)?).to_owned(),
        })
    }

    pub(super) fn members(pgid: u32) -> Vec<Member> {
        let Ok(entries) = std::fs::read_dir("/proc") else {
            return Vec::new();
        };
        let mut members: Vec<Member> = entries
            .filter_map(|entry| entry.ok()?.file_name().to_str()?.parse::<u32>().ok())
            .filter_map(|pid| {
                let stat = stat(pid)?;
                (stat.pgrp == pgid && !matches!(stat.state, 'Z' | 'X')).then_some(Member {
                    pid,
                    name: stat.name,
                    start: Some(stat.start),
                })
            })
            .collect();
        members.sort_by_key(|member| member.pid);
        members
    }

    pub(super) fn carries_marker(pid: u32, marker: &str) -> bool {
        let expected = format!("{GROUP_MARKER_VARIABLE}={marker}");
        std::fs::read(format!("/proc/{pid}/environ")).is_ok_and(|environ| {
            environ
                .split(|byte| *byte == 0)
                .any(|entry| entry == expected.as_bytes())
        })
    }
}

#[cfg(target_os = "macos")]
mod libproc {
    use super::Member;

    pub(super) struct Info {
        pub name: String,
        pub pgid: u32,
        pub zombie: bool,
        /// Wall-clock start, `seconds.microseconds`.
        pub start: String,
    }

    pub(super) fn info(pid: u32) -> Option<Info> {
        // SAFETY: proc_pidinfo fills at most the buffer size it is given.
        let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
        let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
        let filled = unsafe {
            libc::proc_pidinfo(
                pid as libc::c_int,
                libc::PROC_PIDTBSDINFO,
                0,
                (&mut info as *mut libc::proc_bsdinfo).cast(),
                size,
            )
        };
        if filled != size {
            return None;
        }
        let name: String = info
            .pbi_comm
            .iter()
            .take_while(|c| **c != 0)
            .map(|c| *c as u8 as char)
            .collect();
        Some(Info {
            name,
            pgid: info.pbi_pgid,
            zombie: info.pbi_status == libc::SZOMB,
            start: format!("{}.{:06}", info.pbi_start_tvsec, info.pbi_start_tvusec),
        })
    }

    pub(super) fn members(pgid: u32) -> Option<Vec<Member>> {
        let mut pids = vec![0 as libc::pid_t; 4096];
        let capacity = (pids.len() * std::mem::size_of::<libc::pid_t>()) as libc::c_int;
        // SAFETY: the buffer holds `capacity` bytes; unused entries stay zero.
        let listed = unsafe {
            libc::proc_listpgrppids(pgid as libc::pid_t, pids.as_mut_ptr().cast(), capacity)
        };
        if listed < 0 {
            return None;
        }
        let mut members: Vec<Member> = pids
            .iter()
            .filter(|pid| **pid > 0)
            .filter_map(|pid| {
                let info = info(*pid as u32)?;
                (info.pgid == pgid && !info.zombie).then_some(Member {
                    pid: *pid as u32,
                    name: info.name,
                    start: Some(info.start),
                })
            })
            .collect();
        members.sort_by_key(|member| member.pid);
        members.dedup_by_key(|member| member.pid);
        Some(members)
    }
}

#[cfg(windows)]
mod job {
    use windows_sys::Win32::{
        Foundation::{CloseHandle, HANDLE},
        System::JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JobObjectBasicAccountingInformation,
            JobObjectExtendedLimitInformation, QueryInformationJobObject, SetInformationJobObject,
            TerminateJobObject, JOBOBJECT_BASIC_ACCOUNTING_INFORMATION,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        },
    };

    /// A kill-on-close job object holding one check's process tree. Closing
    /// its handle, when the execution ends or when the runtime itself dies,
    /// terminates every process still assigned to it. The child is created
    /// suspended and assigned before it runs (see `process::spawn`), so every
    /// descendant it starts is contained.
    pub(super) struct Job(HANDLE);

    // SAFETY: a job handle may be used and closed from any thread.
    unsafe impl Send for Job {}
    unsafe impl Sync for Job {}

    impl Job {
        /// Assign `process` to a new kill-on-close job; `None` when Windows refuses.
        pub(super) fn contain(process: std::os::windows::io::RawHandle) -> Option<Self> {
            // SAFETY: Win32 calls on a handle this function owns and on the
            // live child's process handle, with correctly sized structures.
            unsafe {
                let handle = CreateJobObjectW(std::ptr::null(), std::ptr::null());
                if handle.is_null() {
                    return None;
                }
                let job = Self(handle);
                let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
                limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                let configured = SetInformationJobObject(
                    handle,
                    JobObjectExtendedLimitInformation,
                    (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                    std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                );
                if configured == 0 || AssignProcessToJobObject(handle, process as HANDLE) == 0 {
                    return None;
                }
                Some(job)
            }
        }

        /// Processes currently running in the job.
        pub(super) fn active_processes(&self) -> u32 {
            // SAFETY: the structure is the size the information class expects.
            unsafe {
                let mut info: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION = std::mem::zeroed();
                let queried = QueryInformationJobObject(
                    self.0,
                    JobObjectBasicAccountingInformation,
                    (&mut info as *mut JOBOBJECT_BASIC_ACCOUNTING_INFORMATION).cast(),
                    std::mem::size_of::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>() as u32,
                    std::ptr::null_mut(),
                );
                if queried == 0 {
                    0
                } else {
                    info.ActiveProcesses
                }
            }
        }

        pub(super) fn terminate(&self) {
            // SAFETY: terminating processes in a job this executor created.
            unsafe {
                TerminateJobObject(self.0, 1);
            }
        }
    }

    impl Drop for Job {
        fn drop(&mut self) {
            // SAFETY: the handle is owned by this value and closed once.
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
}

/// Await a capture task once, keeping its result; a settled slot is never re-polled.
async fn settle(
    task: &mut tokio::task::JoinHandle<Result<Captured, String>>,
    slot: &mut Option<Result<Captured, String>>,
) {
    if slot.is_none() {
        *slot = Some(task.await.unwrap_or_else(|e| Err(e.to_string())));
    }
}

struct Captured {
    sha256: String,
    bytes: u64,
    retained_bytes: u64,
    truncated: bool,
    excerpt: String,
    scan: OutputScan,
}

/// Stream one pipe into a bounded artifact file, keeping a trailing excerpt,
/// scanning every complete line for runner summaries and forwarding display
/// chunks. Draining continues past the limit so the child never blocks on a
/// full pipe.
async fn capture(
    mut pipe: impl tokio::io::AsyncRead + Unpin,
    path: PathBuf,
    stream: &'static str,
    progress: Option<mpsc::Sender<Progress>>,
) -> Result<Captured, String> {
    let mut file = create_private_file(&path)
        .await
        .map_err(|e| e.to_string())?;
    let mut hasher = Sha256::new();
    let mut total: u64 = 0;
    let mut written: u64 = 0;
    let mut truncated = false;
    let mut tail: Vec<u8> = Vec::new();
    let mut scan = OutputScan::default();
    let mut emitted = 0;
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        let read = pipe.read(&mut buffer).await.map_err(|e| e.to_string())?;
        if read == 0 {
            break;
        }
        let chunk = &buffer[..read];
        total += read as u64;
        scan.feed(chunk);
        if written < MAX_ARTIFACT_BYTES {
            let allowed = ((MAX_ARTIFACT_BYTES - written) as usize).min(read);
            file.write_all(&chunk[..allowed])
                .await
                .map_err(|e| e.to_string())?;
            hasher.update(&chunk[..allowed]);
            written += allowed as u64;
            if allowed < read {
                truncated = true;
            }
        } else {
            truncated = true;
        }
        tail.extend_from_slice(chunk);
        if tail.len() > EXCERPT_BYTES {
            let drop = tail.len() - EXCERPT_BYTES;
            tail.drain(..drop);
        }
        if let Some(sender) = progress
            .as_ref()
            .filter(|sender| emitted < 100 && sender.capacity() > PROGRESS_RESERVED)
        {
            let text = String::from_utf8_lossy(chunk)
                .chars()
                .take(EXCERPT_BYTES)
                .collect();
            if sender
                .try_send(Progress::Output {
                    stream,
                    chunk: text,
                })
                .is_ok()
            {
                emitted += 1;
            }
        }
    }
    file.flush().await.map_err(|e| e.to_string())?;
    scan.finish();
    Ok(Captured {
        sha256: format!("sha256:{:x}", hasher.finalize()),
        bytes: total,
        retained_bytes: written,
        truncated,
        excerpt: String::from_utf8_lossy(&tail).into_owned(),
        scan,
    })
}

/// A dependency a failing check could not find.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Missing {
    /// A shell reported a command it could not find.
    Command(String),
    /// Python raised `ModuleNotFoundError` for this module.
    PythonModule(String),
    /// Node could not resolve this bare package specifier.
    NodePackage(String),
}

/// What one output stream reported, judged line by line over the whole stream.
#[derive(Debug, Default, Clone)]
struct OutputScan {
    tests: TestCount,
    missing: Option<Missing>,
    line: Vec<u8>,
    overlong: bool,
}

impl OutputScan {
    fn feed(&mut self, mut bytes: &[u8]) {
        while let Some(end) = bytes.iter().position(|b| *b == b'\n') {
            self.push(&bytes[..end]);
            self.complete();
            bytes = &bytes[end + 1..];
        }
        self.push(bytes);
    }

    fn push(&mut self, bytes: &[u8]) {
        if self.overlong || self.line.len() + bytes.len() > MAX_SCANNED_LINE {
            self.overlong = true;
            self.line.clear();
        } else {
            self.line.extend_from_slice(bytes);
        }
    }

    fn complete(&mut self) {
        if !self.overlong {
            let line = String::from_utf8_lossy(&self.line).into_owned();
            // A carriage return redraws the line; each drawn segment is judged.
            for segment in line.split('\r') {
                self.observe(segment);
            }
        }
        self.line.clear();
        self.overlong = false;
    }

    fn observe(&mut self, raw: &str) {
        let clean = strip_ansi(raw);
        let line = clean.trim();
        if line.is_empty() {
            return;
        }
        if self.missing.is_none() {
            self.missing = missing_signature(line);
        }
        self.tests.observe(line);
    }

    fn finish(&mut self) {
        if !self.line.is_empty() {
            self.complete();
        }
    }
}

/// Removes terminal escape sequences (colours, cursor movement) that runners
/// print even without a terminal when `CI` is set.
fn strip_ansi(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            output.push(c);
            continue;
        }
        if chars.peek() == Some(&'[') {
            chars.next();
            // Parameters and intermediates, then one final byte in `@`..`~`.
            for next in chars.by_ref() {
                if ('@'..='~').contains(&next) {
                    break;
                }
            }
        } else {
            chars.next();
        }
    }
    output
}

/// Whether text from check output has the shape of a module, package or
/// command name: short and made of name characters only. Anything else is
/// not read as a missing-dependency signature.
fn name_like(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '/' | '@' | '+'))
}

/// A name from check output as a reason quotes it. Reasons reach model
/// prompts without the redaction artifact tails get, so a credential-shaped
/// name (a token a script tried to run as a command) is withheld.
fn shown(name: &str) -> String {
    if line_carries_token(name) {
        "(name withheld: it looks like a credential)".to_owned()
    } else {
        format!("`{name}`")
    }
}

/// A missing-dependency signature in one line of output.
fn missing_signature(line: &str) -> Option<Missing> {
    let quoted = |text: &str, open: char, close: char| -> Option<String> {
        let start = text.find(open)? + open.len_utf8();
        let end = text[start..].find(close)? + start;
        let value = &text[start..end];
        name_like(value).then(|| value.to_owned())
    };
    if let Some(at) = line.find("ModuleNotFoundError: No module named ") {
        return quoted(&line[at..], '\'', '\'').map(Missing::PythonModule);
    }
    for marker in ["Cannot find module ", "Cannot find package "] {
        if let Some(at) = line.find(marker) {
            let name = quoted(&line[at + marker.len()..], '\'', '\'')?;
            let relative = name.starts_with('.')
                || name.starts_with('/')
                || name.starts_with("node:")
                || name.contains(":\\")
                || name.contains(":/");
            return (!relative).then_some(Missing::NodePackage(name));
        }
    }
    // dash: `sh: 1: vitest: not found`; bash/sh: `bash: vitest: command not found`.
    let shell = line.split(':').next().is_some_and(|first| {
        matches!(
            first.trim().rsplit('/').next(),
            Some("sh" | "bash" | "dash" | "zsh" | "ksh" | "ash")
        )
    });
    if shell {
        for suffix in [": command not found", ": not found"] {
            if let Some(head) = line.strip_suffix(suffix) {
                let name = head.rsplit(": ").next()?.trim();
                if name_like(name) {
                    return Some(Missing::Command(name.to_owned()));
                }
            }
        }
        // zsh: `zsh: command not found: vitest`.
        if let Some((_, name)) = line.split_once(": command not found: ") {
            let name = name.trim();
            return name_like(name).then(|| Missing::Command(name.to_owned()));
        }
    }
    // cmd.exe: `'vitest' is not recognized as an internal or external command,`.
    if line.contains(" is not recognized as an internal or external command") {
        return quoted(line, '\'', '\'').map(Missing::Command);
    }
    None
}

/// Classify a non-zero exit caused by a missing tool or dependency as
/// infrastructure. A Python module or Node package that exists in the
/// repository is the code's own import, which a repair can fix, so it stays
/// an ordinary failure.
fn missing_dependency(code: i32, scans: &[OutputScan], cwd: &Path, root: &Path) -> Option<String> {
    let canonical_cwd = std::fs::canonicalize(cwd).unwrap_or_else(|_| cwd.to_path_buf());
    let cwd = canonical_cwd.as_path();
    // `cwd` is canonical; compare it with the canonical root.
    let root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let src = root.join("src");
    let found = scans.iter().find_map(|scan| scan.missing.clone());
    let what = match (code, found) {
        (127, Some(Missing::Command(name))) => {
            format!("command {} was not found (exit code 127)", shown(&name))
        }
        (127, _) => "a command the recipe runs was not found (exit code 127)".to_owned(),
        (_, Some(Missing::Command(name))) => format!("command {} was not found", shown(&name)),
        (_, Some(Missing::PythonModule(module))) => {
            let top = module.split('.').next().unwrap_or_default();
            let local = [cwd, root.as_path(), src.as_path()]
                .iter()
                .any(|dir| dir.join(top).is_dir() || dir.join(format!("{top}.py")).is_file());
            if local || !declared_python_dependency(top, cwd, &root) {
                return None;
            }
            format!(
                "Python module {} is not installed (ModuleNotFoundError)",
                shown(&module)
            )
        }
        (_, Some(Missing::NodePackage(specifier))) => {
            let mut parts = specifier.split('/');
            let package = match (parts.next(), parts.next()) {
                (Some(scope), Some(name)) if scope.starts_with('@') => format!("{scope}/{name}"),
                (Some(name), _) => name.to_owned(),
                _ => return None,
            };
            let installed = cwd
                .ancestors()
                .take_while(|dir| dir.starts_with(&root))
                .any(|dir| dir.join("node_modules").join(&package).exists());
            if installed {
                return None;
            }
            format!(
                "package {} is not installed (Cannot find module)",
                shown(&package)
            )
        }
        (_, None) => return None,
    };
    Some(format!("missing dependency: {what}; {PREPARATION_ADVICE}"))
}

/// Executed tests counted from recognised runner summaries. Only a
/// recognised summary that reports zero executed tests (none found, every
/// test skipped, filtered or deselected) yields `Some(0)`; unrecognised
/// output yields `None`.
#[derive(Debug, Default, Clone)]
struct TestCount {
    found: bool,
    executed: u64,
    /// unittest's `Ran N tests`, waiting for the outcome line that reports skips.
    unittest_ran: Option<u64>,
}

impl TestCount {
    fn observe(&mut self, line: &str) {
        if let Some(ran) = self.unittest_ran {
            if let Some(skipped) = unittest_outcome(line) {
                self.executed += ran.saturating_sub(skipped);
                self.unittest_ran = None;
                return;
            }
        }
        if let Some(ran) = unittest_ran(line) {
            self.settle_unittest();
            self.found = true;
            self.unittest_ran = Some(ran);
            return;
        }
        let count = cargo_summary(line)
            .or_else(|| go_package(line))
            .or_else(|| pytest_summary(line))
            .or_else(|| jest_summary(line))
            .or_else(|| vitest_summary(line))
            .or_else(|| gradle_test_task(line))
            .or_else(|| {
                // unittest (Python 3.12+), jest and vitest with no test at all.
                (line == "NO TESTS RAN"
                    || line.starts_with("No tests found")
                    || line.starts_with("No test files found"))
                .then_some(0)
            });
        if let Some(count) = count {
            self.found = true;
            self.executed += count;
        }
    }

    fn settle_unittest(&mut self) {
        if let Some(ran) = self.unittest_ran.take() {
            self.executed += ran;
        }
    }

    fn total(&self) -> Option<u64> {
        let mut settled = self.clone();
        settled.settle_unittest();
        settled.found.then_some(settled.executed)
    }
}

/// `N word` pairs in a summary fragment such as `3 passed` or `1 skipped`.
fn count_word(part: &str) -> Option<(u64, &str)> {
    let (number, word) = part.trim().split_once(' ')?;
    Some((number.parse().ok()?, word.trim()))
}

/// cargo: `test result: ok. 3 passed; 0 failed; 1 ignored; 0 measured; ...`.
/// Ignored and filtered tests did not execute.
fn cargo_summary(line: &str) -> Option<u64> {
    let rest = line.strip_prefix("test result:")?;
    let mut executed = 0;
    for field in rest.split(';') {
        let words: Vec<&str> = field.split_whitespace().collect();
        for pair in words.windows(2) {
            if let (Ok(n), "passed" | "failed") = (pair[0].parse::<u64>(), pair[1]) {
                executed += n;
            }
        }
    }
    Some(executed)
}

/// go test: `ok  \tpkg\t0.01s` counts one package that ran tests;
/// `ok  \tpkg\t0.01s [no tests to run]` and `?   \tpkg\t[no test files]` none.
fn go_package(line: &str) -> Option<u64> {
    let (status, rest) = line.split_once('\t')?;
    match status.trim_end() {
        "ok" if rest.contains("[no tests to run]") => Some(0),
        "ok" | "FAIL" => Some(1),
        "?" if rest.contains("[no test files]") => Some(0),
        _ => None,
    }
}

/// unittest: `Ran 3 tests in 0.002s`.
fn unittest_ran(line: &str) -> Option<u64> {
    let rest = line.strip_prefix("Ran ")?;
    let mut words = rest.split_whitespace();
    let count = words.next()?.parse().ok()?;
    matches!(words.next(), Some("test" | "tests")).then_some(())?;
    (words.next() == Some("in")).then_some(count)
}

/// unittest's outcome line after `Ran N tests`: `OK`, `OK (skipped=2)` or
/// `FAILED (failures=1, skipped=1)`; the number of skipped tests.
fn unittest_outcome(line: &str) -> Option<u64> {
    if line == "OK" {
        return Some(0);
    }
    let details = line
        .strip_prefix("OK (")
        .or_else(|| line.strip_prefix("FAILED ("))?
        .strip_suffix(')')?;
    Some(
        details
            .split(',')
            .filter_map(|part| part.trim().strip_prefix("skipped="))
            .filter_map(|n| n.parse::<u64>().ok())
            .sum(),
    )
}

/// pytest: `=== 3 passed, 1 skipped in 0.12s ===`, `-q`'s `2 skipped in
/// 0.01s` or `no tests ran in 0.01s`. Every part must be a known count, so a
/// custom line that merely mentions passing is not a summary.
fn pytest_summary(line: &str) -> Option<u64> {
    let inner = line.trim_matches(|c: char| c == '=' || c.is_whitespace());
    let (counts, duration) = inner.rsplit_once(" in ")?;
    duration
        .split_whitespace()
        .next()?
        .strip_suffix('s')?
        .parse::<f64>()
        .ok()?;
    if counts == "no tests ran" {
        return Some(0);
    }
    let mut executed = 0;
    for part in counts.split(", ") {
        let (n, word) = count_word(part)?;
        match word {
            "passed" | "failed" | "xfailed" | "xpassed" | "error" | "errors" => executed += n,
            "skipped" | "deselected" | "warning" | "warnings" | "rerun" => {}
            _ => return None,
        }
    }
    Some(executed)
}

/// jest: `Tests:       1 skipped, 2 passed, 3 total`.
fn jest_summary(line: &str) -> Option<u64> {
    let rest = line.strip_prefix("Tests:")?;
    let mut executed = 0;
    let mut total = false;
    for part in rest.split(',') {
        let (n, word) = count_word(part)?;
        match word {
            "passed" | "failed" => executed += n,
            "skipped" | "todo" | "pending" => {}
            "total" => total = true,
            _ => return None,
        }
    }
    total.then_some(executed)
}

/// vitest: `Tests  2 failed | 11 passed (13)` or `Tests  3 skipped (3)`.
fn vitest_summary(line: &str) -> Option<u64> {
    let rest = line.strip_prefix("Tests")?;
    if !rest.starts_with(char::is_whitespace) {
        return None;
    }
    let (counts, total) = rest.trim().rsplit_once(" (")?;
    total.strip_suffix(')')?.parse::<u64>().ok()?;
    let mut executed = 0;
    for part in counts.split('|') {
        let (n, word) = count_word(part)?;
        match word {
            "passed" | "failed" => executed += n,
            "skipped" | "todo" => {}
            _ => return None,
        }
    }
    Some(executed)
}

/// Gradle's plain console: `> Task :app:test` ran a test task;
/// `> Task :test NO-SOURCE` had no tests and `SKIPPED` did not run.
fn gradle_test_task(line: &str) -> Option<u64> {
    let rest = line.strip_prefix("> Task :")?;
    let mut words = rest.split_whitespace();
    let name = words.next()?.rsplit(':').next()?;
    if name != "test" && !name.ends_with("Test") {
        return None;
    }
    match words.next() {
        Some("NO-SOURCE" | "SKIPPED") => Some(0),
        None | Some("UP-TO-DATE" | "FROM-CACHE" | "FAILED") => Some(1),
        _ => None,
    }
}

/// Count executed tests from well-known runner summaries. `Some(0)` means a
/// recognised runner reported that nothing executed; `None` means no runner
/// summary was seen.
pub fn detect_tests_in(text: &str) -> Option<u64> {
    let mut scan = OutputScan::default();
    scan.feed(text.as_bytes());
    scan.finish();
    scan.tests.total()
}

/// The artifact identity a check result records and the artifact route
/// resolves: `<recipe>.<stream>`, relative to the run.
pub fn artifact_id(recipe_id: &str, stream: &str) -> String {
    format!("{recipe_id}.{stream}")
}

/// File name of a recipe's private log for `stream` in a run's artifact
/// directory. A recipe ID of lower-case letters, digits, `.`, `_` and `-`
/// names its file directly (`unit.stdout.log`). Any other ID, one with
/// upper-case letters or whose stem Windows reserves for a device (`nul`,
/// `con`, `com1`, ...), is lower-cased, its dots replaced, and suffixed with
/// a digest of the exact ID, so IDs that differ only in case never share a
/// file on a case-insensitive filesystem and no log is opened as a device.
pub fn artifact_file_name(recipe_id: &str, stream: &str) -> String {
    let lower = recipe_id.to_ascii_lowercase();
    let stem = lower.split('.').next().unwrap_or_default();
    let numbered = |prefix: &str| {
        stem.len() == 4 && stem.starts_with(prefix) && stem.as_bytes()[3].is_ascii_digit()
    };
    let reserved =
        matches!(stem, "con" | "prn" | "aux" | "nul") || numbered("com") || numbered("lpt");
    if lower == recipe_id && !reserved {
        return format!("{recipe_id}.{stream}.log");
    }
    // Windows reads a device name from everything before the first dot, so
    // the escaped stem keeps none.
    let digest = sha256_hex(recipe_id.as_bytes());
    format!(
        "{}~{}.{stream}.log",
        lower.replace('.', "_"),
        &digest[7..23]
    )
}

/// File name of the log an artifact identity (`<recipe>.<stdout|stderr>`)
/// refers to, as [`artifact_file_name`] named it when the check ran.
pub fn artifact_log_name(artifact_id: &str) -> String {
    match artifact_id.rsplit_once('.') {
        Some((recipe_id, stream @ ("stdout" | "stderr"))) => artifact_file_name(recipe_id, stream),
        _ => format!("{artifact_id}.log"),
    }
}

/// Read old artifacts without renaming them. Only safe, ordinary files in the
/// private artifact directory qualify; never follow a link to a legacy name.
pub fn artifact_log_path(directory: &Path, artifact_id: &str) -> std::path::PathBuf {
    let current = directory.join(artifact_log_name(artifact_id));
    if current.exists() {
        return current;
    }
    if !artifact_id.is_empty()
        && artifact_id.len() <= 128
        && !artifact_id.contains("..")
        && artifact_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
    {
        let legacy = directory.join(format!("{artifact_id}.log"));
        if std::fs::symlink_metadata(&legacy).is_ok_and(|m| m.file_type().is_file()) {
            return legacy;
        }
    }
    current
}

// Absence alone cannot distinguish a deleted application module from an
// uninstalled dependency. Only an explicit requirements declaration establishes
// infrastructure here. Unknown import/distribution mappings stay ordinary failures.
fn declared_python_dependency(module: &str, cwd: &Path, root: &Path) -> bool {
    let cwd = std::fs::canonicalize(cwd).unwrap_or_else(|_| cwd.to_path_buf());
    cwd.ancestors()
        .take_while(|dir| dir.starts_with(root))
        .any(|dir| {
            ["requirements.txt", "requirements-dev.txt"]
                .iter()
                .any(|name| {
                    let path = dir.join(name);
                    if !std::fs::metadata(&path).is_ok_and(|m| m.len() <= 262_144) {
                        return false;
                    }
                    std::fs::read_to_string(path).is_ok_and(|text| {
                        text.lines().any(|line| {
                            let name = line
                                .trim()
                                .split(|c: char| !(c.is_ascii_alphanumeric() || "-_.".contains(c)))
                                .next()
                                .unwrap_or("");
                            !name.is_empty() && name.replace('-', "_").eq_ignore_ascii_case(module)
                        })
                    })
                })
        })
}

/// Artifact digests keyed by the artifact identities a result records.
pub fn artifact_hashes(
    recipe_id: &str,
    execution: &Execution,
) -> std::collections::BTreeMap<String, String> {
    execution
        .artifacts
        .iter()
        .map(|a| (artifact_id(recipe_id, &a.name), a.sha256.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_runner_summaries_and_empty_suites() {
        assert_eq!(
            detect_tests_in(
                "running 3 tests\ntest result: ok. 3 passed; 0 failed; 0 ignored; 0 measured\n"
            ),
            Some(3)
        );
        assert_eq!(
            detect_tests_in("test result: ok. 0 passed; 0 failed; 0 ignored\ntest result: ok. 0 passed; 0 failed; 0 ignored\n"),
            Some(0)
        );
        assert_eq!(
            detect_tests_in("ok  \texample.invalid/x\t0.012s\n"),
            Some(1)
        );
        assert_eq!(
            detect_tests_in("?   \texample.invalid/x\t[no test files]\n"),
            Some(0)
        );
        assert_eq!(
            detect_tests_in("============ no tests ran in 0.01s ============\n"),
            Some(0)
        );
        assert_eq!(
            detect_tests_in("=========== 4 passed, 1 failed in 0.3s ===========\n"),
            Some(5)
        );
        assert_eq!(detect_tests_in("      Tests  13 passed (13)\n"), Some(13));
        assert_eq!(
            detect_tests_in(
                "test_greeting (test_messages.MessagesTest.test_greeting) ... ok\n\n----------------------------------------------------------------------\nRan 2 tests in 0.001s\n\nOK\n"
            ),
            Some(2)
        );
        assert_eq!(
            detect_tests_in("Ran 1 test in 0.000s\n\nFAILED (failures=1)\n"),
            Some(1)
        );
        assert_eq!(
            detect_tests_in("Ran 0 tests in 0.000s\n\nNO TESTS RAN\n"),
            Some(0)
        );
        assert_eq!(detect_tests_in("Ran out of disk space\n"), None);
        assert_eq!(detect_tests_in("Ran 3 testers in the room\n"), None);
        assert_eq!(
            detect_tests_in("No test files found, exiting with code 1\n"),
            Some(0)
        );
        assert_eq!(detect_tests_in("hello world\n"), None);
    }

    #[test]
    fn skipped_filtered_and_absent_tests_are_not_executed_tests() {
        // cargo: ignored tests did not run.
        assert_eq!(
            detect_tests_in("test result: ok. 0 passed; 0 failed; 3 ignored; 0 measured; 0 filtered out; finished in 0.00s\n"),
            Some(0)
        );
        // go: a package without Test functions, or a -run filter matching nothing.
        assert_eq!(
            detect_tests_in("ok  \texample.invalid/m\t0.003s [no tests to run]\n"),
            Some(0)
        );
        assert_eq!(
            detect_tests_in("ok  \texample.invalid/a\t0.003s [no tests to run]\nok  \texample.invalid/b\t0.010s\n"),
            Some(1)
        );
        assert_eq!(
            detect_tests_in("ok  \texample.invalid/a\t(cached)\n"),
            Some(1)
        );
        // unittest: every test skipped, or some.
        assert_eq!(
            detect_tests_in("ss\n----------------------------------------------------------------------\nRan 2 tests in 0.000s\n\nOK (skipped=2)\n"),
            Some(0)
        );
        assert_eq!(
            detect_tests_in("Ran 3 tests in 0.010s\n\nFAILED (failures=1, skipped=1)\n"),
            Some(2)
        );
        assert_eq!(
            detect_tests_in("Ran 2 tests in 0.010s\n\nOK (expected failures=1)\n"),
            Some(2)
        );
        // pytest: skipped and deselected only, with and without -q padding.
        assert_eq!(detect_tests_in("2 skipped in 0.01s\n"), Some(0));
        assert_eq!(
            detect_tests_in("============= 3 skipped in 0.01s =============\n"),
            Some(0)
        );
        assert_eq!(
            detect_tests_in("================ 5 deselected in 0.02s ================\n"),
            Some(0)
        );
        assert_eq!(
            detect_tests_in(
                "=== 1 failed, 2 passed, 1 skipped, 2 warnings in 65.12s (0:01:05) ===\n"
            ),
            Some(3)
        );
        // jest: skipped and todo tests are not executed.
        assert_eq!(
            detect_tests_in("Tests:       3 skipped, 3 total\n"),
            Some(0)
        );
        assert_eq!(
            detect_tests_in("Tests:       1 skipped, 1 todo, 2 passed, 4 total\n"),
            Some(2)
        );
        assert_eq!(
            detect_tests_in("Test Suites: 1 passed, 1 total\nTests:       2 todo, 2 total\n"),
            Some(0)
        );
        // vitest, coloured as it prints with CI set.
        assert_eq!(detect_tests_in("      Tests  3 skipped (3)\n"), Some(0));
        assert_eq!(
            detect_tests_in("\u{1b}[2m      Tests \u{1b}[22m \u{1b}[1m\u{1b}[32m1 passed\u{1b}[39m\u{1b}[22m\u{1b}[2m | \u{1b}[22m\u{1b}[33m2 skipped\u{1b}[39m\u{1b}[90m (3)\u{1b}[39m\n"),
            Some(1)
        );
        assert_eq!(
            detect_tests_in("      Tests  2 failed | 11 passed (13)\n"),
            Some(13)
        );
        // Gradle: a test task without sources ran nothing; one that ran counts.
        assert_eq!(detect_tests_in("> Task :test NO-SOURCE\n"), Some(0));
        assert_eq!(
            detect_tests_in("> Task :lib:test NO-SOURCE\n> Task :app:testDebugUnitTest\n"),
            Some(1)
        );
        assert_eq!(detect_tests_in("> Task :testClasses\n"), None);
    }

    #[test]
    fn custom_lines_never_read_as_an_empty_suite() {
        for text in [
            "checked 42 cases\nTests passed!\n",
            "Tests: all green\n",
            "Done: 3 passed in style\n",
            "ok all good\n",
            "OK (skipped=3)\n",
            "> Task :compileTestJava\n",
            "Test Files  1 passed (1)\n",
        ] {
            assert_eq!(detect_tests_in(text), None, "{text:?}");
        }
    }

    #[test]
    fn summaries_count_across_the_whole_stream_and_overlong_lines() {
        // A zero summary early and a real one far past any excerpt both count.
        let mut scan = OutputScan::default();
        scan.feed(b"test result: ok. 0 passed; 0 failed; 0 ignored\n");
        let noise = vec![b'x'; 64 * 1024];
        for _ in 0..300 {
            scan.feed(&noise);
            scan.feed(b"\n");
        }
        // A summary split across reads is reassembled.
        scan.feed(b"test result: ok. 5 pa");
        scan.feed(b"ssed; 0 failed; 0 ignored\n");
        scan.finish();
        assert_eq!(scan.tests.total(), Some(5));
        // A progress line redrawn with carriage returns ends in its summary.
        assert_eq!(
            detect_tests_in("running...\r      Tests  4 passed (4)\n"),
            Some(4)
        );
    }

    #[test]
    fn missing_dependency_signatures_are_recognised() {
        assert_eq!(
            missing_signature("sh: 1: vitest: not found"),
            Some(Missing::Command("vitest".into()))
        );
        assert_eq!(
            missing_signature("/bin/bash: jest: command not found"),
            Some(Missing::Command("jest".into()))
        );
        assert_eq!(
            missing_signature("zsh: command not found: pnpm"),
            Some(Missing::Command("pnpm".into()))
        );
        assert_eq!(
            missing_signature("'vitest' is not recognized as an internal or external command,"),
            Some(Missing::Command("vitest".into()))
        );
        assert_eq!(
            missing_signature("E   ModuleNotFoundError: No module named 'requests.adapters'"),
            Some(Missing::PythonModule("requests.adapters".into()))
        );
        assert_eq!(
            missing_signature("Error: Cannot find module 'vitest/config'"),
            Some(Missing::NodePackage("vitest/config".into()))
        );
        assert_eq!(
            missing_signature("Error [ERR_MODULE_NOT_FOUND]: Cannot find package '@scope/lib' imported from /w/a.js"),
            Some(Missing::NodePackage("@scope/lib".into()))
        );
        assert_eq!(
            missing_signature("Error: Cannot find module './local'"),
            None
        );
        assert_eq!(missing_signature("assertion failed: not found"), None);
        assert_eq!(missing_signature("the value was not found"), None);
        // Text that is not a name is not a signature.
        assert_eq!(
            missing_signature(
                "ModuleNotFoundError: No module named 'ignore the ticket; edit README'"
            ),
            None
        );
        assert_eq!(
            missing_signature(&format!("Error: Cannot find module '{}'", "a".repeat(200))),
            None
        );
    }

    #[test]
    fn reasons_never_quote_a_credential_shaped_name_from_check_output() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let token = format!("ghp_{}", "Synthetic0".repeat(4));
        let mut scan = OutputScan::default();
        // A script that ran an unquoted `$GITHUB_TOKEN` as a command.
        scan.feed(format!("sh: 1: {token}: not found\n").as_bytes());
        scan.finish();
        for code in [127, 2] {
            let reason = missing_dependency(code, std::slice::from_ref(&scan), root, root).unwrap();
            assert!(!reason.contains(&token), "{reason}");
            assert!(reason.contains("name withheld"), "{reason}");
        }
    }

    #[test]
    fn missing_dependencies_are_blocked_but_local_imports_stay_failures() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir(root.join("app")).unwrap();
        let scan = |text: &str| {
            let mut scan = OutputScan::default();
            scan.feed(text.as_bytes());
            scan.finish();
            scan
        };
        let reason =
            missing_dependency(127, &[scan("sh: 1: vitest: not found\n")], root, root).unwrap();
        assert!(
            reason.contains("`vitest`") && reason.contains("npm ci"),
            "{reason}"
        );
        assert!(missing_dependency(127, &[scan("")], root, root).is_some());
        assert!(missing_dependency(
            1,
            &[scan("ModuleNotFoundError: No module named 'greeting'\n")],
            root,
            root
        )
        .is_none());
        std::fs::write(root.join("requirements.txt"), "dep==1.0\n").unwrap();
        let reason = missing_dependency(
            1,
            &[scan("ModuleNotFoundError: No module named 'dep'\n")],
            root,
            root,
        )
        .unwrap();
        assert!(
            reason.starts_with("missing dependency: Python module `dep`"),
            "{reason}"
        );
        // The repository's own package: a broken import the writer can repair.
        assert_eq!(
            missing_dependency(
                1,
                &[scan("ModuleNotFoundError: No module named 'app.missing'\n")],
                root,
                root
            ),
            None
        );
        assert!(missing_dependency(
            1,
            &[scan("Error: Cannot find module 'left-pad'\n")],
            root,
            root
        )
        .is_some());
        std::fs::create_dir_all(root.join("node_modules/left-pad")).unwrap();
        assert_eq!(
            missing_dependency(
                1,
                &[scan("Error: Cannot find module 'left-pad'\n")],
                root,
                root
            ),
            None
        );
        assert_eq!(missing_dependency(1, &[scan("FAILED\n")], root, root), None);
    }

    #[test]
    fn secrets_and_reserved_variables_are_stripped() {
        for name in [
            "OPENROUTER_API_KEY",
            "GITHUB_TOKEN",
            "LUUNA_API_TOKEN",
            "GIT_DIR",
            "AWS_SECRET_ACCESS_KEY",
            "DB_PASSWORD",
            "LUUNA_PI_COMMAND",
            "CI_JOB_JWT",
            "AWS_ACCESS_KEY_ID",
            "STRIPE_KEY",
            "SIGNING_KEYS",
            "GPG_PASSPHRASE",
            "SESSION_COOKIE",
            "BW_SESSION",
            "OP_SESSION_myaccount",
            "DB_PASS",
            "MYSQL_PWD",
            "GITHUB_PAT",
            "SENTRY_DSN",
            "DATABASE_URL",
            "TEST_DATABASE_URL",
            "SLACK_WEBHOOK_URL",
            "AZURE_STORAGE_CONNECTION_STRING",
            "SQL_CONNSTR",
            "HF_TOKEN_PATH",
            "VAULT_SECRET_ID",
        ] {
            assert!(is_secret_or_reserved(name), "{name}");
        }
        for name in ["OPENROUTER_API_KEY", "BW_SESSION", "DB_PASS", "GITHUB_PAT"] {
            assert!(is_credential_name(name), "{name}");
        }
        for name in ["LUUNA_PI_COMMAND", "GIT_DIR"] {
            assert!(!is_credential_name(name), "{name}");
        }
        for name in [
            "PATH",
            "HOME",
            "CARGO_HOME",
            "RUSTUP_HOME",
            "NODE_ENV",
            "TMPDIR",
            "KEYBOARD_LAYOUT",
            "MONKEY_COUNT",
            "PWD",
            "OLDPWD",
            "PYTHONPATH",
            "LD_LIBRARY_PATH",
            "XDG_SESSION_TYPE",
            "DESKTOP_SESSION",
            "HTTPS_PROXY",
        ] {
            assert!(!is_secret_or_reserved(name), "{name}");
        }
    }

    #[test]
    fn token_shapes_cover_real_formats_and_lengths() {
        for line in [
            // A real-format AWS access key ID is AKIA plus 16 characters.
            "aws key AKIAIOSFODNN7EXAMPLE",
            "ASIAIOSFODNN7EXAMPLE",
            "token=ghs_16C7e42F292c6912E7710c838347Ae178B4a",
            "ghu_16C7e42F292c6912E7710c838347Ae178B4a",
            "github_pat_11ABCDEFG0123456789_abcdefghijklmnop",
            "sk_live_4eC39HqLyjWDarjtT1zdp7dc",
            "rk_live_4eC39HqLyjWDarjtT1zdp7dc",
            "npm_aB3dE5gH7jK9mN1pQ3sT5vW7yZ9bC1dE3fG5",
            "hf_aB3dE5gH7jK9mN1pQ3sT5vW7yZ9bC1dE3f",
            "xapp-1-A0123456789-0123456789012-abc",
            "Authorization: Basic dXNlcjpwYXNz",
            "authorization: token abc",
            "aws_secret_access_key = wJalrXUtnFEMI/K7MDENG",
            "//registry.npmjs.org/:_authToken=abc",
        ] {
            assert!(line_carries_token(line), "{line}");
        }
        for line in [
            "sk-1 is a label",
            "task-abcdefghijklmnopqrstuvwxyz",
            "AKIA is a prefix",
            "ASIAN market data",
            "the npm_config_cache setting",
            "test result: ok. 3 passed",
        ] {
            assert!(!line_carries_token(line), "{line}");
        }
    }

    #[test]
    fn windows_resolution_prefers_spawnable_extensions_over_bare_shims() {
        let pathext = Some(".COM;.EXE;.BAT;.CMD;.VBS;.JS");
        assert_eq!(
            path_candidates("npm", true, pathext),
            vec!["npm.com", "npm.exe", "npm.bat", "npm.cmd"]
        );
        assert_eq!(path_candidates("npm.cmd", true, pathext), vec!["npm.cmd"]);
        assert_eq!(path_candidates("NPM.CMD", true, pathext), vec!["NPM.CMD"]);
        assert_eq!(
            path_candidates("python3.11", true, Some(".EXE")),
            vec!["python3.11.exe"]
        );
        assert_eq!(
            path_candidates("flutter", true, None),
            vec!["flutter.com", "flutter.exe", "flutter.bat", "flutter.cmd"]
        );
        assert_eq!(
            path_candidates("tool", true, Some(".VBS")),
            vec!["tool.com", "tool.exe", "tool.bat", "tool.cmd"]
        );
        assert_eq!(path_candidates("npm", false, pathext), vec!["npm"]);
    }

    #[test]
    fn artifact_file_names_are_safe_on_case_insensitive_and_windows_filesystems() {
        assert_eq!(artifact_file_name("unit", "stdout"), "unit.stdout.log");
        assert_eq!(
            artifact_file_name("c1-g1-lint_2.x", "stderr"),
            "c1-g1-lint_2.x.stderr.log"
        );
        let upper = artifact_file_name("Unit", "stdout");
        let lower = artifact_file_name("unit", "stdout");
        let shout = artifact_file_name("UNIT", "stdout");
        assert_ne!(upper.to_ascii_lowercase(), lower.to_ascii_lowercase());
        assert_ne!(upper.to_ascii_lowercase(), shout.to_ascii_lowercase());
        assert!(upper.starts_with("unit~") && upper.ends_with(".stdout.log"));
        for reserved in ["nul", "CON", "aux.x", "com1", "lpt9"] {
            let name = artifact_file_name(reserved, "stdout");
            let stem = name.split('.').next().unwrap().to_ascii_lowercase();
            assert!(stem.contains('~'), "{reserved} -> {name}");
        }
        assert_eq!(
            artifact_file_name("console", "stdout"),
            "console.stdout.log"
        );
        assert_eq!(artifact_file_name("com", "stdout"), "com.stdout.log");
        // A recorded identity finds the file its check wrote.
        assert_eq!(
            artifact_log_name("c1-g1-Unit.stdout"),
            artifact_file_name("c1-g1-Unit", "stdout")
        );
        assert_eq!(artifact_log_name("unit.stderr"), "unit.stderr.log");
        assert_eq!(artifact_log_name("c1-g1-check0"), "c1-g1-check0.log");
    }

    #[test]
    fn a_failed_probe_is_part_of_the_fingerprint() {
        let path = Path::new("/usr/local/go/bin/go");
        let failed = Executor::fingerprint_for(path, &ToolVersion::Failed("exit status 2".into()));
        assert_ne!(failed, Executor::fingerprint(path, None));
        assert_eq!(
            Executor::fingerprint_for(path, &ToolVersion::Reported(Some("go1.24".into()))),
            Executor::fingerprint(path, Some("go1.24"))
        );
        assert_eq!(
            Executor::fingerprint_for(path, &ToolVersion::NotProbed),
            Executor::fingerprint(path, None)
        );
        assert_eq!(version_arguments("go"), ["version"]);
        assert_eq!(version_arguments("cargo"), ["--version"]);
    }
}

#[cfg(test)]
mod legacy_artifact_tests {
    use super::*;
    #[test]
    fn legacy_logs_remain_readable_without_overwriting_or_escaping() {
        let dir = tempfile::tempdir().unwrap();
        let legacy = dir.path().join("Unit.stdout.log");
        std::fs::write(&legacy, b"old evidence").unwrap();
        assert_eq!(artifact_log_path(dir.path(), "Unit.stdout"), legacy);
        let current = dir.path().join(artifact_log_name("Unit.stdout"));
        std::fs::write(&current, b"new evidence").unwrap();
        assert_eq!(artifact_log_path(dir.path(), "Unit.stdout"), current);
        assert_eq!(std::fs::read(&legacy).unwrap(), b"old evidence");
    }
}

#[cfg(test)]
mod fingerprint_pin_tests;
