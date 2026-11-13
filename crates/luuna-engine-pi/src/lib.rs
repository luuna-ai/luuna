//! Supervised JSONL transport for pinned Pi. A response acknowledges a command;
//! only `agent_settled` finishes all automatic work in Pi 1.3.0. No shell is used.
use serde::de::{self, DeserializeSeed, IgnoredAny, MapAccess, SeqAccess, Visitor};
use serde_json::{json, Map, Value};
use std::{
    collections::{HashMap, VecDeque},
    ffi::OsStr,
    fmt,
    path::{Component, Path, PathBuf},
    process::Stdio,
    sync::{Arc, Mutex},
    time::Duration,
};
use thiserror::Error;
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout, Command},
    task::JoinHandle,
};

pub const PI_PACKAGE: &str = "@earendil-works/pi-coding-agent";
pub const PI_VERSION: &str = "1.3.0";
/// Largest record buffered and returned verbatim, including its LF.
pub const MAX_RECORD_BYTES: usize = 1024 * 1024;
/// Largest oversized session event read as a summary (see [`PiProcess::next`]).
/// Its bulky content is skipped as it streams past, never buffered, so this
/// bounds reading time rather than memory.
pub const MAX_SUMMARIZED_RECORD_BYTES: usize = 256 * 1024 * 1024;
/// Session events whose size grows with the session in Pi 1.3.0: `agent_end`
/// and `turn_end` repeat every message and tool result of the run, and message
/// and tool events carry whole prompts, file contents and tool arguments.
/// Beyond `MAX_RECORD_BYTES` only the fields the runtime reads are kept; any
/// other record that large is still a protocol error.
pub const SUMMARIZED_KINDS: [&str; 8] = [
    "agent_end",
    "turn_end",
    "message_start",
    "message_update",
    "message_end",
    "tool_execution_start",
    "tool_execution_update",
    "tool_execution_end",
];
/// Messages a summary keeps from `messages` or `toolResults`: the last ones,
/// plus the first failed one, which [`Record::model_error`] reports.
const SUMMARY_MESSAGES: usize = 64;
/// Longest string a summary keeps, such as a provider error message.
const SUMMARY_STRING_BYTES: usize = 1024;
/// Message fields the runtime reads; content and tool arguments are dropped.
const MESSAGE_FIELDS: [&str; 7] = [
    "role",
    "stopReason",
    "errorMessage",
    "usage",
    "toolCallId",
    "toolName",
    "isError",
];
/// Top-level event fields a summary keeps besides its messages.
const EVENT_FIELDS: [&str; 7] = [
    "type",
    "willRetry",
    "aborted",
    "usage",
    "toolCallId",
    "toolName",
    "isError",
];
/// Pi reads the system prompt from this file in the session's private agent
/// directory (`--system-prompt <path>`), never from argv text: writer
/// instructions embed approved ticket text that can exceed per-argument limits
/// (128 KiB on Linux, 32,767 characters for a whole Windows command line), and
/// argv is visible to other local accounts in the process list.
pub const SYSTEM_PROMPT_FILE: &str = "luuna-system-prompt.md";
const TOOL_FREE_PROMPT: &str = "You are the Luuna read-only connectivity demo. You have no tools. Answer the user's request as plain text.";
/// Built-in tools that Pi 1.3.0 implements with a helper program: `grep` runs
/// ripgrep and `find` runs fd (`fdfind` on Debian). Pi looks in its agent
/// `bin` directory, then on PATH, and downloads a missing helper only when
/// online. Luuna always passes `--offline` with a fresh agent directory, so a
/// tool whose helper is absent fails every call and is not declared at all.
pub const TOOL_HELPERS: [(&str, &[&str]); 2] = [("grep", &["rg"]), ("find", &["fd", "fdfind"])];
/// The reviewer's fixed read-only tools, before helper availability.
pub const REVIEWER_TOOLS: [&str; 4] = ["read", "grep", "find", "ls"];
/// Pi's stderr is drained continuously and only its end is kept: this much is
/// redacted as a whole, then at most `STDERR_TAIL_BYTES` of it is reported.
const STDERR_WINDOW_BYTES: usize = 16 * 1024;
pub const STDERR_TAIL_BYTES: usize = 2 * 1024;

/// Direct provider API-key variables the isolated child may inherit. Each also
/// accepts a `LUUNA_PI_` prefixed alias for hosts that reserve the plain name;
/// the alias takes precedence and is never passed through under its own name.
pub const PROVIDER_KEY_VARIABLES: [&str; 8] = [
    "ANTHROPIC_API_KEY",
    "OPENAI_API_KEY",
    "GEMINI_API_KEY",
    "GOOGLE_API_KEY",
    "MISTRAL_API_KEY",
    "GROQ_API_KEY",
    "XAI_API_KEY",
    "OPENROUTER_API_KEY",
];

/// Explicit opt-in only: the OAuth launcher uses this one provider's grant,
/// while settings, extensions and session history remain isolated.
pub const OAUTH_AUTH_FILE_VARIABLE: &str = "LUUNA_PI_AUTH_FILE";

fn openai_oauth_at(path: &Path) -> Option<Value> {
    if !path.is_absolute() || std::fs::metadata(path).ok()?.len() > 1024 * 1024 {
        return None;
    }
    let data: Value = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
    let credential = data.get("openai")?;
    (credential.get("type")?.as_str()? == "oauth"
        && ["access", "refresh", "clientId"].iter().all(|name| {
            credential
                .get(name)
                .and_then(Value::as_str)
                .is_some_and(|s| !s.is_empty())
        })
        && credential.get("expires")?.is_number()
        && credential
            .get("scopes")?
            .as_array()?
            .iter()
            .any(|scope| scope.as_str() == Some("chatgpt.tokens.use.direct")))
    .then(|| credential.clone())
}

/// Presence only, not a provider-access assertion. A real model run verifies access.
pub fn openai_oauth_configured() -> bool {
    std::env::var_os(OAUTH_AUTH_FILE_VARIABLE)
        .and_then(|path| openai_oauth_at(Path::new(&path)))
        .is_some()
}

/// Names (never values) of provider credentials the child would receive from
/// the current process environment, after alias resolution.
pub fn configured_provider_keys() -> Vec<&'static str> {
    PROVIDER_KEY_VARIABLES
        .into_iter()
        .filter(|name| {
            std::env::var_os(format!("LUUNA_PI_{name}")).is_some()
                || std::env::var_os(name).is_some()
        })
        .collect()
}

/// Values of every provider credential the child can receive, for redaction.
fn credential_values() -> Vec<String> {
    let mut values: Vec<String> = PROVIDER_KEY_VARIABLES
        .iter()
        .flat_map(|name| {
            [
                std::env::var(format!("LUUNA_PI_{name}")).ok(),
                std::env::var(name).ok(),
            ]
        })
        .flatten()
        .filter(|value| !value.is_empty())
        .collect();
    if let Some(credential) = std::env::var_os(OAUTH_AUTH_FILE_VARIABLE)
        .and_then(|path| openai_oauth_at(Path::new(&path)))
    {
        for name in ["access", "refresh"] {
            if let Some(value) = credential.get(name).and_then(Value::as_str) {
                values.push(value.to_owned());
            }
        }
    }
    values
}

#[derive(Debug, Error)]
pub enum EngineError {
    #[error("Pi I/O: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid Pi RPC record: {0}")]
    Protocol(String),
    /// Pi's stdout ended; carries its exit status and redacted stderr tail.
    #[error("Pi closed stdout before the operation completed{0}")]
    Closed(String),
    /// Pi stopped reading commands; carries the same diagnostics as `Closed`.
    #[error("cannot send a command to Pi: {0}{1}")]
    Write(std::io::Error, String),
    #[error("Pi launcher: {0}")]
    Launcher(String),
    #[error("Pi command {command} failed: {message}")]
    Rejected { command: String, message: String },
    #[error("Pi RPC timed out")]
    Timeout,
}

#[derive(Clone, Debug)]
pub struct PiConfig {
    pub program: PathBuf,
    /// Launcher arguments (e.g. Node's absolute path to Pi's cli.js). Never shell text.
    pub launcher_args: Vec<String>,
    pub cwd: PathBuf,
    pub config_dir: PathBuf,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub profile: SessionProfile,
}

/// What a session may do. Every profile disables discovered resources,
/// sessions, MCP, skills and context files; only the explicit items below load.
#[derive(Clone, Debug, Default)]
pub enum SessionProfile {
    /// The M1 connectivity demo: no tools at all.
    #[default]
    ToolFree,
    /// An approved writer (M2c): named built-in tools inside a leased worktree,
    /// the Luuna scope guard loaded explicitly, and a task-specific prompt.
    Writer(WriterProfile),
    /// A fresh independent reviewer (M3): file reads only, with no writer
    /// session or conversation restored. The runtime supplies candidate evidence.
    Reviewer(ReviewerProfile),
}

#[derive(Clone, Debug)]
pub struct WriterProfile {
    /// Built-in tool names requested for the model, e.g. `read`, `edit`,
    /// `write`; a tool whose helper program is missing is left out.
    pub tools: Vec<String>,
    pub system_prompt: String,
    /// Extension files loaded with `-e`; the scope guard is always one of them.
    pub extensions: Vec<PathBuf>,
    /// Extra variables for the child, such as `LUUNA_SCOPE_JSON`.
    pub environment: Vec<(String, String)>,
}

#[derive(Clone, Debug)]
pub struct ReviewerProfile {
    pub system_prompt: String,
    pub extensions: Vec<PathBuf>,
    /// Normally LUUNA_SCOPE_JSON with only the `read` operation. The reviewer
    /// cannot widen its tool set: those tools are fixed below, not prompt input.
    pub environment: Vec<(String, String)>,
}

/// The Luuna scope guard extension, written into a private directory per attempt.
pub const SCOPE_GUARD_SOURCE: &str = include_str!("../assets/luuna-scope-guard.js");

/// Write the scope guard beside the attempt's Pi configuration and return its path.
pub fn install_scope_guard(dir: &std::path::Path) -> Result<PathBuf, std::io::Error> {
    private_directory(dir)?;
    let path = dir.join("luuna-scope-guard.js");
    std::fs::write(&path, SCOPE_GUARD_SOURCE)?;
    Ok(path)
}

/// The system prompt a profile runs with.
pub fn system_prompt(profile: &SessionProfile) -> &str {
    match profile {
        SessionProfile::ToolFree => TOOL_FREE_PROMPT,
        SessionProfile::Writer(profile) => &profile.system_prompt,
        SessionProfile::Reviewer(profile) => &profile.system_prompt,
    }
}

/// Where [`PiProcess::spawn`] writes the system prompt. The path is absolute:
/// Pi resolves a relative one against the session's working directory.
pub fn system_prompt_path(config: &PiConfig) -> PathBuf {
    let path = config.config_dir.join(SYSTEM_PROMPT_FILE);
    std::path::absolute(&path).unwrap_or(path)
}

/// Tools declared to a session and the requested ones left out.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ToolSelection {
    /// Declared to the model, in the requested order.
    pub declared: Vec<String>,
    /// Requested but not declared, each with its reason.
    pub omitted: Vec<(String, String)>,
}

impl ToolSelection {
    /// The omitted tools as `[{"tool", "reason"}]` for a durable event.
    pub fn omitted_json(&self) -> Value {
        Value::Array(
            self.omitted
                .iter()
                .map(|(tool, reason)| json!({"tool": tool, "reason": reason}))
                .collect(),
        )
    }
}

/// Splits requested built-in tools into those Pi can run (see
/// [`TOOL_HELPERS`]) and those it cannot, given which helpers are available.
pub fn select_tools(
    requested: &[String],
    helper_available: impl Fn(&str) -> bool,
) -> ToolSelection {
    let mut selection = ToolSelection::default();
    for tool in requested {
        match TOOL_HELPERS.iter().find(|(name, _)| name == tool) {
            Some((_, helpers)) if !helpers.iter().any(|helper| helper_available(helper)) => {
                selection.omitted.push((
                    tool.clone(),
                    format!(
                        "Pi's {tool} tool runs {}, which is not on PATH; Pi runs offline and cannot download it",
                        helpers.join(" or ")
                    ),
                ))
            }
            _ => selection.declared.push(tool.clone()),
        }
    }
    selection
}

/// The tools a configuration's session declares on this host. Helpers are
/// looked up as Pi does: its agent `bin` directory, then the PATH the child
/// inherits.
pub fn session_tools(config: &PiConfig) -> ToolSelection {
    let requested: Vec<String> = match &config.profile {
        SessionProfile::ToolFree => Vec::new(),
        SessionProfile::Writer(profile) => profile.tools.clone(),
        SessionProfile::Reviewer(_) => REVIEWER_TOOLS.map(String::from).to_vec(),
    };
    let bin = config.config_dir.join("bin");
    let path = std::env::var_os("PATH");
    select_tools(&requested, |helper| {
        helper_available(helper, &bin, path.as_deref(), cfg!(windows))
    })
}

/// Whether Pi would find a helper program: in its agent `bin` directory or on
/// PATH, with `.exe`/`.com` appended on Windows as Node's process API does.
fn helper_available(name: &str, bin: &Path, path: Option<&OsStr>, windows: bool) -> bool {
    let names: Vec<String> = if windows {
        vec![format!("{name}.exe"), format!("{name}.com")]
    } else {
        vec![name.to_owned()]
    };
    std::iter::once(bin.to_path_buf())
        .chain(
            path.map(|path| search_path(path, windows))
                .unwrap_or_default(),
        )
        .any(|dir| names.iter().any(|name| is_executable(&dir.join(name))))
}

fn is_executable(path: &Path) -> bool {
    match std::fs::metadata(path) {
        #[cfg(unix)]
        Ok(metadata) => {
            use std::os::unix::fs::PermissionsExt;
            metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
        }
        #[cfg(not(unix))]
        Ok(metadata) => metadata.is_file(),
        Err(_) => false,
    }
}

/// Directories of a PATH value: `;`-separated (quotes removed) on Windows.
fn search_path(path: &OsStr, windows: bool) -> Vec<PathBuf> {
    if windows {
        path.to_string_lossy()
            .split(';')
            .map(|dir| dir.trim().trim_matches('"'))
            .filter(|dir| !dir.is_empty())
            .map(PathBuf::from)
            .collect()
    } else {
        std::env::split_paths(path).collect()
    }
}

/// Command-line arguments for a profile, after the launcher arguments.
pub fn session_arguments(config: &PiConfig) -> Vec<String> {
    launch_arguments(config, &session_tools(config))
}

fn launch_arguments(config: &PiConfig, tools: &ToolSelection) -> Vec<String> {
    let mut args: Vec<String> = [
        "--mode",
        "rpc",
        "--no-session",
        "--no-extensions",
        "--no-skills",
        "--no-prompt-templates",
        "--no-themes",
        "--no-context-files",
        "--no-mcp",
        "--no-approve",
        "--offline",
    ]
    .into_iter()
    .map(ToOwned::to_owned)
    .collect();
    let extensions = match &config.profile {
        SessionProfile::ToolFree => None,
        SessionProfile::Writer(profile) => Some(&profile.extensions),
        SessionProfile::Reviewer(profile) => Some(&profile.extensions),
    };
    // An empty selection disables tools; it never falls back to Pi's
    // defaults, which include `bash`.
    if extensions.is_none() || tools.declared.is_empty() {
        args.push("--no-tools".into());
    } else {
        args.push("--tools".into());
        args.push(tools.declared.join(","));
    }
    for extension in extensions.into_iter().flatten() {
        args.push("--extension".into());
        args.push(extension.to_string_lossy().into_owned());
    }
    args.push("--system-prompt".into());
    args.push(system_prompt_path(config).to_string_lossy().into_owned());
    if let Some(provider) = &config.provider {
        args.push("--provider".into());
        args.push(provider.clone());
    }
    if let Some(model) = &config.model {
        args.push("--model".into());
        args.push(model.clone());
    }
    args
}

/// Resolves the configured launcher to a program started without a shell.
/// Elsewhere the operating system resolves it as given. On Windows, Rust's
/// process API only appends `.exe` to a bare name while npm installs Pi as a
/// `pi.cmd` shim, and a batch file would run through cmd.exe, which rejects
/// multi-line arguments and survives a stop request as Pi's parent. A bare
/// name is therefore searched through PATH, executables first and then
/// PATHEXT, and an npm shim is replaced by the Node executable and Pi entry
/// point it would start; any other batch launcher is refused. The version
/// probe still verifies the result.
pub fn resolve_launcher(
    program: &Path,
    launcher_args: &[String],
) -> Result<(PathBuf, Vec<String>), EngineError> {
    resolve_launcher_for(
        program,
        launcher_args,
        cfg!(windows),
        std::env::var_os("PATH").as_deref(),
        std::env::var_os("PATHEXT").as_deref(),
    )
}

fn resolve_launcher_for(
    program: &Path,
    launcher_args: &[String],
    windows: bool,
    path: Option<&OsStr>,
    pathext: Option<&OsStr>,
) -> Result<(PathBuf, Vec<String>), EngineError> {
    if !windows {
        return Ok((program.to_path_buf(), launcher_args.to_vec()));
    }
    let extensions: Vec<String> = pathext
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_else(|| ".COM;.EXE;.BAT;.CMD".into())
        .split(';')
        .filter(|extension| extension.starts_with('.') && extension.len() > 1)
        .map(str::to_ascii_lowercase)
        .collect();
    let bare = matches!(
        program.components().collect::<Vec<_>>()[..],
        [Component::Normal(_)]
    );
    // An executable anywhere on PATH wins, as it does for Rust's own lookup;
    // only a name with no executable is looked up as a PATHEXT script.
    let resolved = if bare {
        windows_search(program, path, &[".exe".into(), ".com".into()])
            .or_else(|| windows_search(program, path, &extensions))
            .unwrap_or_else(|| program.to_path_buf())
    } else {
        program.to_path_buf()
    };
    let batch = resolved
        .extension()
        .map(|extension| extension.to_string_lossy().to_ascii_lowercase())
        .is_some_and(|extension| extension == "cmd" || extension == "bat");
    if !batch {
        return Ok((resolved, launcher_args.to_vec()));
    }
    // npm writes `<prefix>\pi.cmd` beside `node_modules` for a global install
    // and `node_modules\.bin\pi.cmd` for a local one; both start Node with
    // the package's `bin` entry, preferring a `node.exe` beside the shim.
    let shim_dir = resolved.parent().unwrap_or(Path::new(""));
    let entry = [shim_dir.join("node_modules"), shim_dir.join("..")]
        .into_iter()
        .map(|root| {
            root.join("@earendil-works")
                .join("pi-coding-agent")
                .join("dist")
                .join("bundle")
                .join("cli.js")
        })
        .find(|candidate| candidate.is_file());
    let node = Some(shim_dir.join("node.exe"))
        .filter(|candidate| candidate.is_file())
        .or_else(|| windows_search(Path::new("node"), path, &[".exe".into(), ".com".into()]));
    match (entry, node) {
        (Some(entry), Some(node)) => {
            let mut args = vec![entry.to_string_lossy().into_owned()];
            args.extend(launcher_args.iter().cloned());
            Ok((node, args))
        }
        _ => Err(EngineError::Launcher(format!(
            "{} is a batch file, which would run Pi through cmd.exe; configure node.exe as the launcher with Pi's dist\\bundle\\cli.js as its first argument",
            resolved.display()
        ))),
    }
}

/// The first `program` on a Windows PATH: as named when its extension is
/// listed in `extensions`, otherwise with each listed extension appended.
fn windows_search(program: &Path, path: Option<&OsStr>, extensions: &[String]) -> Option<PathBuf> {
    let name = program.to_string_lossy();
    let listed = program
        .extension()
        .map(|extension| format!(".{}", extension.to_string_lossy().to_ascii_lowercase()))
        .is_some_and(|extension| extensions.contains(&extension));
    let candidates: Vec<String> = if listed {
        vec![name.into_owned()]
    } else {
        extensions
            .iter()
            .map(|extension| format!("{name}{extension}"))
            .collect()
    };
    search_path(path?, true).into_iter().find_map(|dir| {
        candidates
            .iter()
            .map(|candidate| dir.join(candidate))
            .find(|candidate| candidate.is_file())
    })
}

#[derive(Debug, Clone)]
pub struct Record {
    pub kind: String,
    pub raw: Value,
}

impl Record {
    pub fn parse(line: &[u8]) -> Result<Self, EngineError> {
        if line.len() > MAX_RECORD_BYTES {
            return Err(EngineError::Protocol("record exceeds 1 MiB".into()));
        }
        let raw: Value =
            serde_json::from_slice(line).map_err(|e| EngineError::Protocol(e.to_string()))?;
        Self::from_value(raw)
    }

    /// Validates a decoded record: a whole one, or an oversized event's summary.
    fn from_value(raw: Value) -> Result<Self, EngineError> {
        let kind = raw
            .get("type")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| EngineError::Protocol("record requires a nonempty type".into()))?
            .to_owned();
        if kind == "response"
            && (raw.get("id").and_then(Value::as_str).is_none()
                || raw.get("command").and_then(Value::as_str).is_none()
                || raw.get("success").and_then(Value::as_bool).is_none())
        {
            return Err(EngineError::Protocol(
                "response requires id, command and success".into(),
            ));
        }
        // Pi 1.3.0 session events carry mandatory completion fields. Missing
        // or mistyped fields cannot serve as evidence that a run succeeded.
        let invalid = match kind.as_str() {
            "agent_settled" if raw.get("aborted").and_then(Value::as_bool).is_none() => {
                Some("agent_settled requires aborted boolean")
            }
            "agent_end"
                if !raw
                    .get("messages")
                    .and_then(Value::as_array)
                    .is_some_and(|messages| messages.iter().all(Value::is_object))
                    || raw.get("willRetry").and_then(Value::as_bool).is_none() =>
            {
                Some("agent_end requires messages array of objects and willRetry boolean")
            }
            "message_end" if !raw.get("message").is_some_and(Value::is_object) => {
                Some("message_end requires message object")
            }
            _ => None,
        };
        if let Some(message) = invalid {
            return Err(EngineError::Protocol(message.into()));
        }
        Ok(Self { kind, raw })
    }

    pub fn model_error(&self) -> Option<String> {
        let messages: Vec<&Value> = if self.kind == "agent_end" {
            self.raw
                .get("messages")
                .and_then(Value::as_array)
                .map(|a| a.iter().collect())
                .unwrap_or_default()
        } else if self.kind == "message_end" {
            self.raw.get("message").into_iter().collect()
        } else {
            Vec::new()
        };
        messages.into_iter().find_map(|message| {
            let reason = message.get("stopReason").and_then(Value::as_str);
            if matches!(reason, Some("error" | "aborted")) {
                Some(
                    message
                        .get("errorMessage")
                        .and_then(Value::as_str)
                        .unwrap_or("model operation failed")
                        .to_owned(),
                )
            } else {
                None
            }
        })
    }

    /// Completed assistant text only. Partial deltas, user/tool messages and
    /// `agent_end` repeats are not a structured review result. Runtime code must
    /// still validate the JSON and bind its verdict to the exact candidate.
    pub fn assistant_text(&self) -> Option<String> {
        if self.kind != "message_end" {
            return None;
        }
        let message = self.raw.get("message")?;
        if message.get("role")?.as_str()? != "assistant"
            || message.get("stopReason").and_then(Value::as_str) != Some("stop")
        {
            return None;
        }
        let content = message.get("content")?.as_array()?;
        let text: Vec<&str> = content
            .iter()
            .filter(|part| part.get("type").and_then(Value::as_str) == Some("text"))
            .filter_map(|part| part.get("text").and_then(Value::as_str))
            .collect();
        (!text.is_empty()).then(|| text.join("\n"))
    }
}

/// Monotonic session cost in micros across assistant/tool turns. Pi's message
/// updates repeat the current message's cumulative usage, while each completed
/// assistant message adds a new cost. `agent_end` repeats those same messages
/// and must never charge them again. A successful `compaction_end` reports the
/// separate summarization call Pi makes outside the message stream, which Pi
/// counts in its session totals, so it is added once. This counter is local
/// measurement; the runtime persists its deltas so interruptions cannot reset
/// consumed budgets.
#[derive(Debug, Default)]
pub struct SessionUsage {
    completed_cost: u64,
    current_cost: u64,
    total_cost: u64,
    cost_unreported: bool,
}

/// Provider-reported cost in micros, rounded up; a missing, negative or
/// non-finite value is no report.
fn reported_cost(usage: &Value) -> Option<u64> {
    usage["cost"]["total"]
        .as_f64()
        .filter(|cost| cost.is_finite() && *cost >= 0.0)
        .map(|cost| (cost * 1_000_000.0).ceil() as u64)
}

/// Whether a completed usage report counted tokens without pricing them: Pi
/// reports cost 0 (or none) for a model missing from its price table, a
/// custom endpoint or a local model.
fn tokens_without_cost(usage: &Value) -> bool {
    let tokens = usage["totalTokens"].as_u64().unwrap_or_else(|| {
        ["input", "output", "cacheRead", "cacheWrite"]
            .iter()
            .filter_map(|field| usage[*field].as_u64())
            .sum()
    });
    tokens > 0 && reported_cost(usage).unwrap_or(0) == 0
}

impl SessionUsage {
    /// Whether a completed message or summarization consumed tokens that the
    /// provider reported no cost for: the cost budget cannot meter that spend.
    pub fn cost_unreported(&self) -> bool {
        self.cost_unreported
    }

    pub fn observe(&mut self, record: &Record) -> u64 {
        let message = &record.raw["message"];
        let assistant = message["role"] == "assistant";
        let cost = reported_cost(&message["usage"]).or_else(|| reported_cost(&record.raw["usage"]));
        match record.kind.as_str() {
            "message_start" if assistant => {
                // Retain provisional spend if a provider retry starts another
                // message without a terminal event for the previous request.
                self.completed_cost = self.completed_cost.saturating_add(self.current_cost);
                self.current_cost = cost.unwrap_or(0);
            }
            "message_update" if assistant || message.is_null() => {
                self.current_cost = self.current_cost.max(cost.unwrap_or(0));
            }
            "message_end" if assistant => {
                self.completed_cost = self
                    .completed_cost
                    .saturating_add(self.current_cost.max(cost.unwrap_or(0)));
                self.current_cost = 0;
                self.cost_unreported |= tokens_without_cost(&message["usage"]);
            }
            "compaction_end" if record.raw["aborted"] != true => {
                let usage = &record.raw["result"]["usage"];
                self.completed_cost = self
                    .completed_cost
                    .saturating_add(reported_cost(usage).unwrap_or(0));
                self.cost_unreported |= tokens_without_cost(usage);
            }
            _ => {}
        }
        self.total_cost = self
            .total_cost
            .max(self.completed_cost.saturating_add(self.current_cost));
        self.total_cost
    }
}

/// The kind of a record from its first member, which Pi 1.3.0 always writes as
/// `"type"`; the buffered start of an oversized record is enough to read it.
fn leading_kind(prefix: &[u8]) -> Option<String> {
    let start = String::from_utf8_lossy(&prefix[..prefix.len().min(256)]);
    let rest = start
        .trim_start()
        .strip_prefix('{')?
        .trim_start()
        .strip_prefix("\"type\"")?
        .trim_start()
        .strip_prefix(':')?
        .trim_start()
        .strip_prefix('"')?;
    let kind = &rest[..rest.find('"')?];
    (!kind.is_empty() && kind.bytes().all(|b| b.is_ascii_lowercase() || b == b'_'))
        .then(|| kind.to_owned())
}

/// A summary keeps strings to `SUMMARY_STRING_BYTES`, cut at a character boundary.
fn clip(value: Value) -> Value {
    match value {
        Value::String(text) if text.len() > SUMMARY_STRING_BYTES => {
            let mut cut = SUMMARY_STRING_BYTES;
            while !text.is_char_boundary(cut) {
                cut -= 1;
            }
            Value::String(format!(
                "{}… [truncated {} bytes]",
                &text[..cut],
                text.len() - cut
            ))
        }
        other => other,
    }
}

/// Keeps the listed members of a JSON object and skips the others without
/// buffering them. Anything but an object is an error, as for a whole record.
#[derive(Clone, Copy)]
struct Fields(&'static [&'static str]);

impl<'de> DeserializeSeed<'de> for Fields {
    type Value = Map<String, Value>;
    fn deserialize<D: de::Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> Result<Self::Value, D::Error> {
        deserializer.deserialize_map(self)
    }
}

impl<'de> Visitor<'de> for Fields {
    type Value = Map<String, Value>;
    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("a JSON object")
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut kept = Map::new();
        while let Some(key) = map.next_key::<String>()? {
            if self.0.contains(&key.as_str()) {
                let value = map.next_value::<Value>()?;
                kept.insert(key, clip(value));
            } else {
                map.next_value::<IgnoredAny>()?;
            }
        }
        Ok(kept)
    }
}

/// A message array reduced to the message fields of its last
/// `SUMMARY_MESSAGES` elements and its first failed one, with its length.
struct Messages;

impl<'de> DeserializeSeed<'de> for Messages {
    type Value = (Vec<Value>, usize);
    fn deserialize<D: de::Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> Result<Self::Value, D::Error> {
        deserializer.deserialize_seq(self)
    }
}

impl<'de> Visitor<'de> for Messages {
    type Value = (Vec<Value>, usize);
    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("an array of message objects")
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
        let mut first_failure = None;
        let mut last = VecDeque::new();
        let mut total = 0usize;
        while let Some(message) = seq.next_element_seed(Fields(&MESSAGE_FIELDS))? {
            let failed = matches!(
                message.get("stopReason").and_then(Value::as_str),
                Some("error" | "aborted")
            );
            if failed && first_failure.is_none() {
                first_failure = Some((total, message.clone()));
            }
            last.push_back((total, message));
            if last.len() > SUMMARY_MESSAGES {
                last.pop_front();
            }
            total += 1;
        }
        let mut kept = Vec::new();
        if let Some((index, message)) = first_failure {
            if last
                .front()
                .is_some_and(|(first_kept, _)| index < *first_kept)
            {
                kept.push(Value::Object(message));
            }
        }
        kept.extend(last.into_iter().map(|(_, message)| Value::Object(message)));
        Ok((kept, total))
    }
}

/// The summary of an oversized session event: its scalar fields, message
/// fields and message counts, under a `luuna_summary` marker.
struct EventSummary;

impl<'de> Visitor<'de> for EventSummary {
    type Value = Map<String, Value>;
    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("a Pi session event object")
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut kept = Map::new();
        let mut marker = Map::from_iter([("content_omitted".to_owned(), json!(true))]);
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "message" => {
                    let message = map.next_value_seed(Fields(&MESSAGE_FIELDS))?;
                    kept.insert(key, Value::Object(message));
                }
                "messages" | "toolResults" => {
                    let (messages, total) = map.next_value_seed(Messages)?;
                    marker.insert(key.clone(), json!(total));
                    kept.insert(key, Value::Array(messages));
                }
                "assistantMessageEvent" => {
                    let event = map.next_value_seed(Fields(&["type", "contentIndex"]))?;
                    kept.insert(key, Value::Object(event));
                }
                field if EVENT_FIELDS.contains(&field) => {
                    let value = map.next_value::<Value>()?;
                    kept.insert(key, clip(value));
                }
                _ => {
                    map.next_value::<IgnoredAny>()?;
                }
            }
        }
        kept.insert("luuna_summary".into(), Value::Object(marker));
        Ok(kept)
    }
}

/// A blocking reader over the chunks of one oversized record.
struct ChunkReader {
    receiver: tokio::sync::mpsc::Receiver<Vec<u8>>,
    chunk: Vec<u8>,
    offset: usize,
}

impl std::io::Read for ChunkReader {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        while self.offset == self.chunk.len() {
            match self.receiver.blocking_recv() {
                Some(chunk) => {
                    self.chunk = chunk;
                    self.offset = 0;
                }
                None => return Ok(0),
            }
        }
        let count = buffer.len().min(self.chunk.len() - self.offset);
        buffer[..count].copy_from_slice(&self.chunk[self.offset..self.offset + count]);
        self.offset += count;
        Ok(count)
    }
}

/// Parses one streamed record into its summary; the sender closing ends it.
fn summarize(
    receiver: tokio::sync::mpsc::Receiver<Vec<u8>>,
) -> Result<Map<String, Value>, EngineError> {
    let protocol = |error: serde_json::Error| EngineError::Protocol(error.to_string());
    let mut deserializer = serde_json::Deserializer::from_reader(ChunkReader {
        receiver,
        chunk: Vec::new(),
        offset: 0,
    });
    let summary =
        de::Deserializer::deserialize_map(&mut deserializer, EventSummary).map_err(protocol)?;
    deserializer.end().map_err(protocol)?;
    Ok(summary)
}

pub struct PiProcess {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    /// Taken once it has finished, so it is never polled again.
    stderr_drain: Option<JoinHandle<()>>,
    stderr_tail: Arc<Mutex<Vec<u8>>>,
    tools: ToolSelection,
    pending: HashMap<String, String>,
}

impl PiProcess {
    /// Launch a fresh session with its explicit profile. Only named provider
    /// credentials are inherited; Git/publication auth is excluded. The runtime
    /// gives every assignment a separate private configuration directory.
    pub async fn spawn(config: &PiConfig) -> Result<Self, EngineError> {
        let (program, launcher_args) = resolve_launcher(&config.program, &config.launcher_args)?;
        probe_version(config, &program, &launcher_args).await?;
        write_private_file(&system_prompt_path(config), system_prompt(&config.profile))?;
        let tools = session_tools(config);
        let mut command = Command::new(&program);
        command
            .args(&launcher_args)
            .args(launch_arguments(config, &tools));
        command.env_clear();
        for name in [
            "PATH",
            "SystemRoot",
            "WINDIR",
            "TEMP",
            "TMP",
            "TMPDIR",
            "SSL_CERT_FILE",
            "SSL_CERT_DIR",
            "NODE_EXTRA_CA_CERTS",
            "HTTP_PROXY",
            "HTTPS_PROXY",
            "ALL_PROXY",
            "NO_PROXY",
            "http_proxy",
            "https_proxy",
            "all_proxy",
            "no_proxy",
            "NODE_USE_ENV_PROXY",
        ] {
            if let Some(value) = std::env::var_os(name) {
                command.env(name, value);
            }
        }
        // Cloud secret bindings can use the non-reserved `LUUNA_PI_` alias when
        // the platform reserves the provider's own name. Pi only reads the
        // standard name, so the alias is mapped and never passed through itself.
        for name in PROVIDER_KEY_VARIABLES {
            if let Some(value) =
                std::env::var_os(format!("LUUNA_PI_{name}")).or_else(|| std::env::var_os(name))
            {
                command.env(name, value);
            }
        }
        let environment = match &config.profile {
            SessionProfile::Writer(profile) => &profile.environment[..],
            SessionProfile::Reviewer(profile) => &profile.environment[..],
            SessionProfile::ToolFree => &[],
        };
        for (name, value) in environment {
            command.env(name, value);
        }
        command
            .env("HOME", &config.config_dir)
            .env("USERPROFILE", &config.config_dir)
            .env("PI_CODING_AGENT_DIR", &config.config_dir)
            .current_dir(&config.cwd)
            .kill_on_drop(true)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn()?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| EngineError::Protocol("stdin unavailable".into()))?;
        let stdout = BufReader::new(
            child
                .stdout
                .take()
                .ok_or_else(|| EngineError::Protocol("stdout unavailable".into()))?,
        );
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| EngineError::Protocol("stderr unavailable".into()))?;
        // Diagnostics stay private until a failure needs them, and then only a
        // redacted tail is reported: they may contain paths or credentials.
        let stderr_tail = Arc::new(Mutex::new(Vec::new()));
        let stderr_drain = tokio::spawn(keep_tail(stderr, stderr_tail.clone()));
        Ok(Self {
            child,
            stdin,
            stdout,
            stderr_drain: Some(stderr_drain),
            stderr_tail,
            tools,
            pending: HashMap::new(),
        })
    }

    /// Operating-system process ID of the owned child while it has not been reaped.
    pub fn pid(&self) -> Option<u32> {
        self.child.id()
    }

    /// The tools declared to this session and those left out, with reasons.
    pub fn tools(&self) -> &ToolSelection {
        &self.tools
    }

    pub async fn command(
        &mut self,
        id: &str,
        command_type: &str,
        data: Value,
    ) -> Result<(), EngineError> {
        match self.send(id, command_type, data).await {
            Err(EngineError::Io(error)) => Err(EngineError::Write(error, self.diagnostics().await)),
            other => other,
        }
    }

    async fn send(
        &mut self,
        id: &str,
        command_type: &str,
        mut data: Value,
    ) -> Result<(), EngineError> {
        if self.pending.contains_key(id) {
            return Err(EngineError::Protocol("duplicate command ID".into()));
        }
        self.pending.insert(id.to_owned(), command_type.to_owned());
        data["id"] = json!(id);
        data["type"] = json!(command_type);
        let mut bytes =
            serde_json::to_vec(&data).map_err(|e| EngineError::Protocol(e.to_string()))?;
        bytes.push(b'\n');
        self.stdin.write_all(&bytes).await?;
        self.stdin.flush().await?;
        Ok(())
    }

    /// Bounded LF framing, including Unicode separators inside JSON strings.
    /// A session event of a kind in [`SUMMARIZED_KINDS`] that exceeds
    /// `MAX_RECORD_BYTES` is returned as a summary of the fields the runtime
    /// reads (its type, retry and error state, message roles, stop reasons and
    /// usage), marked `luuna_summary`; its content is skipped unbuffered.
    pub async fn next(&mut self) -> Result<Record, EngineError> {
        let mut line = Vec::new();
        loop {
            let available = self.stdout.fill_buf().await?;
            if available.is_empty() {
                return Err(EngineError::Closed(self.diagnostics().await));
            }
            let count = available
                .iter()
                .position(|b| *b == b'\n')
                .map(|p| p + 1)
                .unwrap_or(available.len());
            if line.len() + count > MAX_RECORD_BYTES {
                return self.summarize_oversized(line).await;
            }
            let complete = available[count - 1] == b'\n';
            line.extend_from_slice(&available[..count]);
            self.stdout.consume(count);
            if complete {
                break;
            }
        }
        let record = Record::parse(&line)?;
        if record.kind == "response" {
            let id = record.raw["id"].as_str().unwrap_or_default();
            let expected = self
                .pending
                .remove(id)
                .ok_or_else(|| EngineError::Protocol(format!("unknown response ID {id}")))?;
            if record.raw["command"].as_str() != Some(&expected) {
                return Err(EngineError::Protocol(
                    "response command does not match request".into(),
                ));
            }
        }
        Ok(record)
    }

    /// Streams the rest of an oversized record through a blocking parser that
    /// keeps only its summary, so memory stays bounded by the first
    /// `MAX_RECORD_BYTES` and a few chunks in flight.
    async fn summarize_oversized(&mut self, head: Vec<u8>) -> Result<Record, EngineError> {
        let kind = leading_kind(&head)
            .filter(|kind| SUMMARIZED_KINDS.contains(&kind.as_str()))
            .ok_or_else(|| EngineError::Protocol("record exceeds 1 MiB".into()))?;
        let (sender, receiver) = tokio::sync::mpsc::channel::<Vec<u8>>(4);
        let parser = tokio::task::spawn_blocking(move || summarize(receiver));
        let mut size = head.len();
        let mut open = sender.send(head).await.is_ok();
        while open {
            let available = self.stdout.fill_buf().await?;
            if available.is_empty() {
                drop(sender);
                return Err(EngineError::Closed(self.diagnostics().await));
            }
            let count = available
                .iter()
                .position(|b| *b == b'\n')
                .map(|p| p + 1)
                .unwrap_or(available.len());
            let complete = available[count - 1] == b'\n';
            size += count;
            if size > MAX_SUMMARIZED_RECORD_BYTES {
                return Err(EngineError::Protocol(format!(
                    "{kind} record exceeds 256 MiB"
                )));
            }
            let chunk = available[..count].to_vec();
            self.stdout.consume(count);
            // A parser that stopped early has already failed; its error is the result.
            open = sender.send(chunk).await.is_ok() && !complete;
        }
        drop(sender);
        let mut summary = parser
            .await
            .map_err(|error| EngineError::Protocol(error.to_string()))??;
        if summary.get("type").and_then(Value::as_str) != Some(kind.as_str()) {
            return Err(EngineError::Protocol(
                "oversized record type is inconsistent".into(),
            ));
        }
        if let Some(Value::Object(marker)) = summary.get_mut("luuna_summary") {
            marker.insert("record_bytes".into(), json!(size));
        }
        Record::from_value(Value::Object(summary))
    }

    /// Why Pi stopped, for a failure reason: its exit status once it has
    /// exited, and the redacted end of its stderr, where Pi 1.3.0 reports
    /// startup failures such as an unknown provider or a broken extension.
    async fn diagnostics(&mut self) -> String {
        // Pi writes its reason and exits; give both a moment so the reason is
        // not lost to a race with the end of stdout.
        if let Some(drain) = self.stderr_drain.as_mut() {
            if tokio::time::timeout(Duration::from_millis(500), drain)
                .await
                .is_ok()
            {
                self.stderr_drain = None;
            }
        }
        let mut diagnostics = String::new();
        if let Ok(Ok(status)) =
            tokio::time::timeout(Duration::from_millis(250), self.child.wait()).await
        {
            diagnostics.push_str(&format!("; Pi exited with {status}"));
        }
        let tail = self
            .stderr_tail
            .lock()
            .map(|window| diagnostic_tail(&window, &credential_values()))
            .unwrap_or_default();
        if !tail.is_empty() {
            diagnostics.push_str(&format!("; Pi stderr (redacted tail): {tail}"));
        }
        diagnostics
    }

    /// Abort is a request, not proof the process has stopped. Always reap it.
    pub async fn stop(&mut self) -> Result<(), EngineError> {
        let _ = tokio::time::timeout(
            Duration::from_secs(1),
            self.send("shutdown-abort", "abort", json!({})),
        )
        .await;
        let _ = tokio::time::timeout(Duration::from_secs(1), self.stdin.shutdown()).await;
        let _ = tokio::time::timeout(Duration::from_millis(150), self.child.wait()).await;
        if self.child.try_wait()?.is_none() {
            self.child.kill().await?;
        }
        self.child.wait().await?;
        if let Some(drain) = self.stderr_drain.take() {
            drain.abort();
        }
        Ok(())
    }
}

/// Drains a child's stderr, keeping only its last `STDERR_WINDOW_BYTES`,
/// from a line start when the window holds one: a line cut by the window
/// edge could hold the unredactable remainder of a credential, so it is
/// dropped. A single longer line stays cut, far ahead of the reported tail.
async fn keep_tail(mut stream: impl AsyncRead + Unpin, window: Arc<Mutex<Vec<u8>>>) {
    let mut buffer = [0u8; 4096];
    loop {
        let read = match stream.read(&mut buffer).await {
            Ok(0) | Err(_) => return,
            Ok(read) => read,
        };
        if let Ok(mut kept) = window.lock() {
            kept.extend_from_slice(&buffer[..read]);
            if kept.len() > STDERR_WINDOW_BYTES {
                let mut excess = kept.len() - STDERR_WINDOW_BYTES;
                if let Some(newline) = kept[excess..].iter().position(|b| *b == b'\n') {
                    excess += newline + 1;
                }
                kept.drain(..excess);
            }
        }
    }
}

/// The end of a diagnostic stream as one line: redacted as a whole, control
/// characters removed, then cut to its last `STDERR_TAIL_BYTES`.
pub fn diagnostic_tail(window: &[u8], known: &[String]) -> String {
    let text = redact_diagnostics(&String::from_utf8_lossy(window), known);
    let line: String = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" | ")
        .chars()
        .filter(|c| !c.is_control())
        .collect();
    if line.len() <= STDERR_TAIL_BYTES {
        return line;
    }
    let mut start = line.len() - STDERR_TAIL_BYTES;
    while !line.is_char_boundary(start) {
        start += 1;
    }
    format!("…{}", &line[start..])
}

/// Best-effort removal of credentials from diagnostic text before it becomes
/// part of a durable failure reason: the configured provider key values
/// (longest first), URL userinfo, bearer and well-known API token shapes, and
/// the values of secret-named assignments (`api_key=…`, `"token": "…"`).
pub fn redact_diagnostics(text: &str, known: &[String]) -> String {
    let mut values: Vec<&str> = known
        .iter()
        .map(String::as_str)
        .filter(|value| !value.is_empty())
        .collect();
    values.sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| a.cmp(b)));
    values.dedup();
    let mut text = text.to_owned();
    for value in values {
        text = text.replace(value, "[redacted credential]");
    }
    let mut output = String::with_capacity(text.len());
    let mut after_bearer = false;
    for piece in text.split_inclusive(char::is_whitespace) {
        let word = piece.trim_end_matches(char::is_whitespace);
        output.push_str(&redact_word(word, after_bearer));
        output.push_str(&piece[word.len()..]);
        if !word.is_empty() {
            after_bearer = word.eq_ignore_ascii_case("bearer");
        }
    }
    output
}

fn redact_word(word: &str, after_bearer: bool) -> String {
    const TOKEN_PREFIXES: [&str; 10] = [
        "sk-",
        "AKIA",
        "ghp_",
        "gho_",
        "ghs_",
        "github_pat_",
        "glpat-",
        "xoxb-",
        "xoxp-",
        "AIza",
    ];
    const SECRET_NAMES: [&str; 6] = ["key", "secret", "password", "passwd", "token", "credential"];
    let core = word.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '_' && c != '-');
    if after_bearer && core.len() >= 8 {
        return word.replacen(core, "[redacted token]", 1);
    }
    if let Some(scheme) = word.find("://") {
        let rest = &word[scheme + 3..];
        let host = rest.find(['/', '?', '#']).unwrap_or(rest.len());
        if let Some(at) = rest[..host].rfind('@') {
            return format!("{}[redacted userinfo]{}", &word[..scheme + 3], &rest[at..]);
        }
        return word.to_owned();
    }
    let token_like = |text: &str| {
        TOKEN_PREFIXES.iter().any(|prefix| {
            text.strip_prefix(prefix).is_some_and(|rest| {
                rest.len() >= 16
                    && rest
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
            })
        })
    };
    if token_like(core) {
        return word.replacen(core, "[redacted token]", 1);
    }
    if let Some(separator) = word.find(['=', ':']) {
        let name = word[..separator]
            .trim_matches(|c: char| !c.is_ascii_alphanumeric())
            .to_ascii_lowercase();
        let value = word[separator + 1..].trim_matches(|c: char| c == '"' || c == '\'' || c == ',');
        if value.len() >= 8 && SECRET_NAMES.iter().any(|secret| name.ends_with(secret)) {
            return format!("{}[redacted]", &word[..=separator]);
        }
    }
    word.to_owned()
}

fn private_directory(path: &std::path::Path) -> Result<(), std::io::Error> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path)
}

/// Writes text readable by the current account only: mode 0600 on Unix, and
/// on Windows the private data directory's inherited access.
fn write_private_file(path: &Path, contents: &str) -> Result<(), std::io::Error> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    std::io::Write::write_all(&mut options.open(path)?, contents.as_bytes())
}

/// Reject unsupported launchers before any provider request. The configured
/// launcher is trusted local configuration, never an API request parameter.
pub async fn verify_version(config: &PiConfig) -> Result<(), EngineError> {
    let (program, launcher_args) = resolve_launcher(&config.program, &config.launcher_args)?;
    probe_version(config, &program, &launcher_args).await
}

/// The probe runs with the session's private agent directory and working
/// directory, as the session itself does: Pi 1.3.0 reads (and locks) its
/// global settings before it answers `--version`, and the user's own Pi
/// configuration must never be touched.
async fn probe_version(
    config: &PiConfig,
    program: &Path,
    launcher_args: &[String],
) -> Result<(), EngineError> {
    private_directory(&config.cwd)?;
    private_directory(&config.config_dir)?;
    let mut command = Command::new(program);
    command
        .args(launcher_args)
        .arg("--version")
        .env_clear()
        .current_dir(&config.cwd)
        .kill_on_drop(true);
    for name in ["PATH", "SystemRoot", "WINDIR", "TEMP", "TMP"] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    command
        .env("HOME", &config.config_dir)
        .env("USERPROFILE", &config.config_dir)
        .env("PI_CODING_AGENT_DIR", &config.config_dir);
    let output = tokio::time::timeout(Duration::from_secs(15), command.output())
        .await
        .map_err(|_| EngineError::Timeout)??;
    let reported = String::from_utf8_lossy(&output.stdout);
    if !output.status.success() || reported.trim() != PI_VERSION {
        let known = credential_values();
        let mut detail = format!("launcher exited with {}", output.status);
        let reported = diagnostic_tail(reported.trim().as_bytes(), &known);
        if !reported.is_empty() {
            detail.push_str(&format!("; reported {:.64}", reported));
        }
        let stderr = diagnostic_tail(&output.stderr, &known);
        if !stderr.is_empty() {
            detail.push_str(&format!("; stderr (redacted tail): {stderr}"));
        }
        return Err(EngineError::Protocol(format!(
            "unsupported Pi version; expected {PI_VERSION} ({detail})"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_oauth_store_selects_only_the_openai_grant_and_redacts_both_tokens() {
        let path = std::env::temp_dir().join(format!("luuna-oauth-{}.json", std::process::id()));
        let grant = json!({"type":"oauth","access":"fixture-access-value","refresh":"fixture-refresh-value",
            "expires":12345,"clientId":"fixture-client","scopes":["chatgpt.tokens.use.direct"]});
        std::fs::write(
            &path,
            json!({"openai":grant,"anthropic":{"type":"api_key","key":"unrelated"}}).to_string(),
        )
        .unwrap();
        let selected = openai_oauth_at(&path).unwrap();
        assert_eq!(selected, grant);
        let known = ["access", "refresh"].map(|name| selected[name].as_str().unwrap().to_owned());
        let redacted = redact_diagnostics("fixture-access-value fixture-refresh-value", &known);
        assert!(!redacted.contains("fixture-access-value"));
        assert!(!redacted.contains("fixture-refresh-value"));
        std::fs::write(&path, r#"{"openai":{"type":"api_key","key":"unrelated"}}"#).unwrap();
        assert!(openai_oauth_at(&path).is_none());
        std::fs::write(&path, "not JSON").unwrap();
        assert!(openai_oauth_at(&path).is_none());
        std::fs::remove_file(path).unwrap();
        assert!(openai_oauth_at(Path::new("relative/auth.json")).is_none());
    }

    #[test]
    fn writer_profile_declares_tools_guard_and_prompt_while_demo_stays_tool_free() {
        let base = PiConfig {
            program: "pi".into(),
            launcher_args: vec![],
            cwd: "/tmp/work".into(),
            config_dir: "/tmp/config".into(),
            provider: Some("openrouter".into()),
            model: Some("m".into()),
            profile: SessionProfile::ToolFree,
        };
        let demo = session_arguments(&base);
        assert!(demo.contains(&"--no-tools".to_owned()));
        assert!(demo.contains(&"--no-extensions".to_owned()));
        assert!(!demo.contains(&"--tools".to_owned()));
        let writer = PiConfig {
            profile: SessionProfile::Writer(WriterProfile {
                tools: vec!["read".into(), "edit".into(), "write".into()],
                system_prompt: "task".into(),
                extensions: vec!["/tmp/config/luuna-scope-guard.js".into()],
                environment: vec![("LUUNA_SCOPE_JSON".into(), "{}".into())],
            }),
            ..base
        };
        let args = session_arguments(&writer);
        let position = |flag: &str| args.iter().position(|a| a == flag).unwrap();
        assert_eq!(args[position("--tools") + 1], "read,edit,write");
        assert_eq!(
            args[position("--extension") + 1],
            "/tmp/config/luuna-scope-guard.js"
        );
        // Only the private file's absolute path is argv; the instructions are
        // not. Pi would resolve a relative path against the worktree.
        let prompt_file = PathBuf::from(&args[position("--system-prompt") + 1]);
        assert!(prompt_file.is_absolute());
        assert_eq!(
            prompt_file,
            std::path::absolute("/tmp/config/luuna-system-prompt.md").unwrap()
        );
        let relative = PiConfig {
            config_dir: "relative-config".into(),
            ..writer.clone()
        };
        assert_eq!(
            system_prompt_path(&relative),
            std::env::current_dir()
                .unwrap()
                .join("relative-config")
                .join(SYSTEM_PROMPT_FILE)
        );
        assert_eq!(system_prompt(&writer.profile), "task");
        assert!(!args.contains(&"task".to_owned()));
        assert!(!demo.iter().any(|arg| arg.contains("connectivity demo")));
        assert!(!args.contains(&"--no-tools".to_owned()));
        // Discovery stays off even for writers; only the explicit extension loads.
        assert!(args.contains(&"--no-extensions".to_owned()));
        assert!(args.contains(&"--no-mcp".to_owned()));
        assert!(SCOPE_GUARD_SOURCE.contains("tool_call"));
    }

    #[test]
    fn prompt_acceptance_is_not_completion() {
        let ack =
            Record::parse(br#"{"type":"response","id":"p1","command":"prompt","success":true}"#)
                .unwrap();
        assert_ne!(ack.kind, "agent_end");
    }

    #[test]
    fn reviewer_profile_has_only_read_tools_and_no_restored_resources() {
        let config = PiConfig {
            program: "pi".into(),
            launcher_args: vec![],
            cwd: "/tmp/candidate".into(),
            config_dir: "/tmp/independent-reviewer".into(),
            provider: None,
            model: None,
            profile: SessionProfile::Reviewer(ReviewerProfile {
                system_prompt: "Luuna independent reviewer; exact candidate evidence".into(),
                extensions: vec!["/tmp/independent-reviewer/luuna-scope-guard.js".into()],
                environment: vec![("LUUNA_SCOPE_JSON".into(), "{}".into())],
            }),
        };
        let args = session_arguments(&config);
        // grep and find are declared only where their helpers exist.
        let declared = session_tools(&config).declared;
        assert!(
            declared.starts_with(&["read".to_owned()]) && declared.ends_with(&["ls".to_owned()])
        );
        assert!(declared
            .iter()
            .all(|tool| REVIEWER_TOOLS.contains(&tool.as_str())));
        assert_eq!(
            args[args.iter().position(|arg| arg == "--tools").unwrap() + 1],
            declared.join(",")
        );
        for flag in [
            "--no-session",
            "--no-context-files",
            "--no-mcp",
            "--no-skills",
            "--no-extensions",
        ] {
            assert!(args.iter().any(|arg| arg == flag));
        }
        assert!(args.iter().any(|arg| arg == "--extension"));
    }

    #[test]
    fn review_text_comes_only_from_successful_completed_assistant_messages() {
        let text = r#"{"passed":true,"findings":[]}"#;
        let record = Record::parse(
            json!({"type":"message_end","message":{
                "role":"assistant","stopReason":"stop","content":[{"type":"text","text":text}]
            }})
            .to_string()
            .as_bytes(),
        )
        .unwrap();
        assert_eq!(record.assistant_text().as_deref(), Some(text));
        for raw in [
            json!({"type":"message_end","message":{"role":"toolResult","stopReason":"stop","content":[{"type":"text","text":text}]}}),
            json!({"type":"message_end","message":{"role":"assistant","stopReason":"error","content":[{"type":"text","text":text}]}}),
            json!({"type":"message_end","message":{"role":"assistant","stopReason":"length","content":[{"type":"text","text":text}]}}),
            json!({"type":"message_update","assistantMessageEvent":{"type":"text_delta","delta":text}}),
            json!({"type":"agent_end","messages":[{"role":"assistant","stopReason":"stop","content":[{"type":"text","text":text}]}],"willRetry":false}),
        ] {
            assert_eq!(
                Record::parse(raw.to_string().as_bytes())
                    .unwrap()
                    .assistant_text(),
                None
            );
        }
    }

    #[test]
    fn session_cost_sums_messages_without_counting_stream_updates_or_agent_end_twice() {
        let mut usage = SessionUsage::default();
        let mut observe =
            |raw: Value| usage.observe(&Record::parse(raw.to_string().as_bytes()).unwrap());
        let message = |cost, text| json!({"role":"assistant","usage":{"cost":{"total":cost}},"content":[{"type":"text","text":text}]});
        assert_eq!(
            observe(json!({"type":"message_start","message":message(0.0,"first")})),
            0
        );
        assert_eq!(
            observe(json!({"type":"message_update","message":message(0.002,"first")})),
            2000
        );
        assert_eq!(
            observe(json!({"type":"message_update","message":message(0.003,"first")})),
            3000
        );
        assert_eq!(
            observe(json!({"type":"message_update","message":message(0.003,"first")})),
            3000
        );
        assert_eq!(
            observe(json!({"type":"message_end","message":message(0.003,"first")})),
            3000
        );
        assert_eq!(
            observe(
                json!({"type":"message_end","message":{"role":"toolResult","usage":{"cost":{"total":9}}}})
            ),
            3000
        );
        assert_eq!(
            observe(json!({"type":"message_start","message":message(0.0,"second")})),
            3000
        );
        assert_eq!(
            observe(json!({"type":"message_update","usage":{"cost":{"total":0.001}}})),
            4000
        );
        assert_eq!(
            observe(json!({"type":"message_end","message":message(0.002,"second")})),
            5000
        );
        assert_eq!(
            observe(
                json!({"type":"agent_end","messages":[message(0.003,"first"),message(0.002,"second")],"willRetry":false})
            ),
            5000
        );
    }

    #[test]
    fn session_cost_retains_partial_spend_across_provider_retry_and_ignores_invalid_usage() {
        let mut usage = SessionUsage::default();
        let mut observe =
            |raw: Value| usage.observe(&Record::parse(raw.to_string().as_bytes()).unwrap());
        assert_eq!(
            observe(json!({"type":"message_update","usage":{"cost":{"total":0.003}}})),
            3000
        );
        assert_eq!(
            observe(json!({"type":"message_start","message":{"role":"assistant"}})),
            3000
        );
        assert_eq!(
            observe(
                json!({"type":"message_end","message":{"role":"assistant","usage":{"cost":{"total":0.002}}}})
            ),
            5000
        );
        assert_eq!(
            observe(json!({"type":"message_update","usage":{"cost":{"total":-1}}})),
            5000
        );
        assert_eq!(
            observe(json!({"type":"message_update","usage":{"cost":{"total":null}}})),
            5000
        );
    }
    #[test]
    fn compaction_usage_is_charged_once_and_unpriced_tokens_are_flagged() {
        let mut usage = SessionUsage::default();
        let mut observe =
            |raw: Value| usage.observe(&Record::parse(raw.to_string().as_bytes()).unwrap());
        let priced = json!({"input":100,"output":10,"totalTokens":110,"cost":{"total":0.0005}});
        observe(json!({"type":"message_end","message":{"role":"assistant","usage":priced}}));
        // Pi's summarization call is reported only by `compaction_end`.
        let summary = json!({"summary":"s","usage":{"input":9000,"output":500,"totalTokens":9500,"cost":{"total":0.25}}});
        assert_eq!(
            observe(
                json!({"type":"compaction_end","reason":"threshold","result":summary,"aborted":false,"willRetry":false})
            ),
            250_500
        );
        // An aborted or failed compaction reports no result and costs nothing.
        assert_eq!(
            observe(
                json!({"type":"compaction_end","reason":"threshold","aborted":true,"willRetry":false})
            ),
            250_500
        );
        assert_eq!(
            observe(
                json!({"type":"compaction_end","reason":"overflow","aborted":false,"willRetry":false,"errorMessage":"failed"})
            ),
            250_500
        );
        assert!(!usage.cost_unreported());
    }

    #[test]
    fn tokens_reported_without_a_cost_mark_the_session_unmetered() {
        for unpriced in [
            json!({"input":100,"output":10,"totalTokens":110,"cost":{"total":0}}),
            json!({"input":100,"output":10}),
        ] {
            let mut usage = SessionUsage::default();
            let record =
                json!({"type":"message_end","message":{"role":"assistant","usage":unpriced}});
            assert_eq!(
                usage.observe(&Record::parse(record.to_string().as_bytes()).unwrap()),
                0
            );
            assert!(usage.cost_unreported(), "{unpriced}");
        }
        // No tokens, as for an errored request before any output, is not a gap.
        let mut usage = SessionUsage::default();
        let record = json!({"type":"message_end","message":{"role":"assistant","stopReason":"error","usage":{"input":0,"output":0,"cost":{"total":0}}}});
        usage.observe(&Record::parse(record.to_string().as_bytes()).unwrap());
        assert!(!usage.cost_unreported());
    }

    #[test]
    fn rejects_malformed_records() {
        for value in ["not json", "{}", r#"{"type":"response","success":true}"#] {
            assert!(Record::parse(value.as_bytes()).is_err());
        }
    }
    #[test]
    fn preserves_unknown_events_and_unicode() {
        let record =
            Record::parse("{\"type\":\"future_event\",\"text\":\"a\u{2028}b\"}".as_bytes())
                .unwrap();
        assert_eq!(record.kind, "future_event");
        assert_eq!(record.raw["text"], "a\u{2028}b");
    }
    #[test]
    fn agent_end_can_contain_provider_failure() {
        let record = Record::parse(br#"{"type":"agent_end","messages":[{"role":"assistant","stopReason":"error","errorMessage":"missing credentials"}],"willRetry":false}"#).unwrap();
        assert_eq!(record.model_error().as_deref(), Some("missing credentials"));
    }

    #[test]
    fn rejects_malformed_known_completion_shapes() {
        for raw in [
            json!({"type":"agent_settled"}),
            json!({"type":"agent_settled","aborted":"false"}),
            json!({"type":"agent_settled","aborted":null}),
            json!({"type":"agent_end","messages":[]}),
            json!({"type":"agent_end","messages":{},"willRetry":false}),
            json!({"type":"agent_end","messages":[null],"willRetry":false}),
            json!({"type":"agent_end","messages":[],"willRetry":"false"}),
            json!({"type":"message_end"}),
            json!({"type":"message_end","message":null}),
            json!({"type":"message_end","message":[]}),
        ] {
            assert!(
                matches!(
                    Record::parse(raw.to_string().as_bytes()),
                    Err(EngineError::Protocol(_))
                ),
                "{raw}"
            );
        }
        assert!(Record::parse(br#"{"type":"agent_settled","aborted":false}"#).is_ok());
        assert!(Record::parse(br#"{"type":"agent_end","messages":[],"willRetry":false}"#).is_ok());
    }

    /// Streams `record` to the summary parser in small chunks, as `next` does.
    fn summary_of(record: &Value) -> Result<Record, EngineError> {
        let bytes = format!("{record}\n").into_bytes();
        let (sender, receiver) = tokio::sync::mpsc::channel(bytes.len() / 4096 + 2);
        for chunk in bytes.chunks(4096) {
            sender.try_send(chunk.to_vec()).unwrap();
        }
        drop(sender);
        Record::from_value(Value::Object(summarize(receiver)?))
    }

    #[test]
    fn oversized_events_keep_completion_fields_and_drop_content() {
        let read = |i: usize| json!({"role":"toolResult","toolName":"read","toolCallId":format!("read-{i}"),"isError":false,"content":[{"type":"text","text":"x".repeat(50_000)}]});
        let mut messages: Vec<Value> = (0..100).map(read).collect();
        messages.insert(3, json!({"role":"assistant","stopReason":"error","errorMessage":"e".repeat(5_000),"content":[{"type":"text","text":"partial"}]}));
        messages.push(json!({"role":"assistant","stopReason":"aborted","errorMessage":"later","usage":{"cost":{"total":0.5}}}));
        let record = json!({"type":"agent_end","messages":messages,"willRetry":false});
        assert!(record.to_string().len() > MAX_RECORD_BYTES);
        let summary = summary_of(&record).unwrap();
        assert_eq!(summary.kind, "agent_end");
        assert_eq!(summary.raw["willRetry"], false);
        assert_eq!(summary.raw["luuna_summary"]["messages"], 102);
        let kept = summary.raw["messages"].as_array().unwrap();
        // The first failure (index 3) plus the last 64 messages, in order.
        assert_eq!(kept.len(), SUMMARY_MESSAGES + 1);
        assert_eq!(kept[0]["stopReason"], "error");
        assert_eq!(kept.last().unwrap()["stopReason"], "aborted");
        assert!(kept.iter().all(|message| message.get("content").is_none()));
        let error = summary.model_error().unwrap();
        assert!(
            error.starts_with("eee") && error.len() < 2 * SUMMARY_STRING_BYTES,
            "{error}"
        );
        assert!(summary.raw.to_string().len() < 64 * 1024);

        let turn = json!({"type":"turn_end","message":{"role":"assistant","stopReason":"toolUse","content":[{"type":"text","text":"y".repeat(10)}]},"toolResults":(0..30).map(read).collect::<Vec<_>>()});
        let summary = summary_of(&turn).unwrap();
        assert_eq!(summary.raw["message"]["stopReason"], "toolUse");
        assert!(summary.raw["message"].get("content").is_none());
        assert_eq!(summary.raw["luuna_summary"]["toolResults"], 30);

        // Usage survives on message events, so cost accounting is unchanged.
        let update = json!({"type":"message_update","usage":{"cost":{"total":0.25}},"assistantMessageEvent":{"type":"toolcall_end","contentIndex":0,"toolCall":{"arguments":{"content":"z".repeat(2_000_000)}}}});
        let summary = summary_of(&update).unwrap();
        assert_eq!(
            summary.raw["assistantMessageEvent"],
            json!({"type":"toolcall_end","contentIndex":0})
        );
        assert_eq!(SessionUsage::default().observe(&summary), 250_000);
    }

    #[test]
    fn oversized_events_are_validated_like_whole_records() {
        for record in [
            json!({"type":"agent_end","messages":[{"role":"user"}, null],"willRetry":false}),
            json!({"type":"agent_end","messages":[],"willRetry":"false"}),
            json!({"type":"agent_end","messages":{}}),
            json!({"type":"agent_end","messages":[]}),
            json!({"type":"message_end","message":null}),
            json!({"type":"message_end","message":[]}),
        ] {
            assert!(
                matches!(summary_of(&record), Err(EngineError::Protocol(_))),
                "{record}"
            );
        }
        let bytes = br#"{"type":"agent_end","messages":[],"willRetry":false} trailing"#;
        let (sender, receiver) = tokio::sync::mpsc::channel(1);
        sender.try_send(bytes.to_vec()).unwrap();
        drop(sender);
        assert!(summarize(receiver).is_err());
        assert_eq!(
            leading_kind(br#" { "type" : "turn_end", "message""#).as_deref(),
            Some("turn_end")
        );
        assert_eq!(leading_kind(br#"{"message":{},"type":"turn_end"}"#), None);
        assert_eq!(leading_kind(br#"{"type":"agent\"end"}"#), None);
    }

    #[test]
    fn tools_whose_helper_is_missing_are_omitted_with_a_reason() {
        let requested: Vec<String> = ["read", "grep", "find", "ls", "edit"]
            .map(String::from)
            .to_vec();
        let all = select_tools(&requested, |_| true);
        assert_eq!(all.declared, requested);
        assert!(all.omitted.is_empty());
        let fdfind_only = select_tools(&requested, |helper| helper == "fdfind");
        assert_eq!(fdfind_only.declared, ["read", "find", "ls", "edit"]);
        assert_eq!(fdfind_only.omitted.len(), 1);
        assert!(fdfind_only.omitted[0].1.contains("runs rg"));
        let none = select_tools(&requested, |_| false);
        assert_eq!(none.declared, ["read", "ls", "edit"]);
        assert_eq!(none.omitted_json()[1]["tool"], "find");
        assert!(none.omitted_json()[1]["reason"]
            .as_str()
            .unwrap()
            .contains("fd or fdfind"));

        // An empty selection disables tools instead of falling back to Pi's defaults.
        let config = PiConfig {
            program: "pi".into(),
            launcher_args: vec![],
            cwd: "/tmp/work".into(),
            config_dir: "/tmp/config".into(),
            provider: None,
            model: None,
            profile: SessionProfile::Writer(WriterProfile {
                tools: vec!["grep".into()],
                system_prompt: "task".into(),
                extensions: vec![],
                environment: vec![],
            }),
        };
        let args = launch_arguments(&config, &select_tools(&["grep".into()], |_| false));
        assert!(args.contains(&"--no-tools".to_owned()) && !args.contains(&"--tools".to_owned()));

        let dir = std::env::temp_dir().join(format!("luuna-helper-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for name in ["rg", "fd.exe"] {
            std::fs::write(dir.join(name), "").unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(dir.join(name), std::fs::Permissions::from_mode(0o755))
                    .unwrap();
            }
        }
        let empty = dir.join("empty-bin");
        assert!(helper_available("rg", &empty, Some(dir.as_os_str()), false));
        assert!(!helper_available(
            "fd",
            &empty,
            Some(dir.as_os_str()),
            false
        ));
        assert!(helper_available("fd", &empty, Some(dir.as_os_str()), true));
        assert!(helper_available("rg", &dir, None, false));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn windows_launchers_resolve_through_pathext_and_unwrap_npm_shims() {
        let root = std::env::temp_dir().join(format!("luuna-launcher-{}", std::process::id()));
        let global = root.join("npm");
        let package = |base: &Path| {
            let entry = base
                .join("@earendil-works/pi-coding-agent/dist/bundle")
                .join("cli.js");
            std::fs::create_dir_all(entry.parent().unwrap()).unwrap();
            std::fs::write(&entry, "").unwrap();
            entry
        };
        let global_entry = package(&global.join("node_modules"));
        std::fs::write(global.join("pi.cmd"), "@ECHO off").unwrap();
        std::fs::write(global.join("node.exe"), "").unwrap();
        let path = std::ffi::OsString::from(format!(
            "{};\"{}\"",
            root.join("missing").display(),
            global.display()
        ));
        let pathext = OsStr::new(".COM;.EXE;.BAT;.CMD");
        let args = vec!["--extra".to_owned()];

        // Off Windows the launcher is used exactly as configured.
        let (program, launcher) =
            resolve_launcher_for(Path::new("pi"), &args, false, Some(&path), Some(pathext))
                .unwrap();
        assert_eq!((program, launcher), (PathBuf::from("pi"), args.clone()));
        // A bare `pi` finds the npm shim, which becomes Node and Pi's entry point.
        let (program, launcher) =
            resolve_launcher_for(Path::new("pi"), &args, true, Some(&path), Some(pathext)).unwrap();
        assert_eq!(program, global.join("node.exe"));
        // Compared as paths: on Windows `/` and `\` are both separators.
        assert_eq!(PathBuf::from(&launcher[0]), global_entry);
        assert_eq!(launcher[1..], ["--extra"]);

        // A local `node_modules\.bin\pi.cmd` uses Node from PATH.
        let local = root.join("project/node_modules");
        let local_entry = package(&local);
        std::fs::create_dir_all(local.join(".bin")).unwrap();
        std::fs::write(local.join(".bin/pi.cmd"), "@ECHO off").unwrap();
        let tools = root.join("tools");
        std::fs::create_dir_all(&tools).unwrap();
        std::fs::write(tools.join("node.cmd"), "").unwrap();
        std::fs::write(tools.join("node.exe"), "").unwrap();
        let tools_path = tools.clone().into_os_string();
        let (program, launcher) = resolve_launcher_for(
            &local.join(".bin/pi.cmd"),
            &[],
            true,
            Some(&tools_path),
            Some(pathext),
        )
        .unwrap();
        assert_eq!(program, tools.join("node.exe"));
        assert_eq!(
            PathBuf::from(&launcher[0]).canonicalize().unwrap(),
            local_entry.canonicalize().unwrap()
        );

        // Any other batch launcher is refused rather than run through cmd.exe.
        std::fs::write(root.join("other.bat"), "").unwrap();
        let refused = resolve_launcher_for(
            &root.join("other.bat"),
            &[],
            true,
            Some(&path),
            Some(pathext),
        );
        assert!(matches!(refused, Err(EngineError::Launcher(reason)) if reason.contains("cli.js")));
        // Executables and unresolvable names are left to the operating system.
        let (program, _) =
            resolve_launcher_for(Path::new("node"), &[], true, Some(&path), Some(pathext)).unwrap();
        assert_eq!(program, global.join("node.exe"));
        let (program, _) =
            resolve_launcher_for(Path::new("absent"), &[], true, Some(&path), Some(pathext))
                .unwrap();
        assert_eq!(program, PathBuf::from("absent"));
        // An executable later on PATH wins over an earlier script shim, as
        // Rust's own lookup of a bare name would pick it.
        let shims = root.join("shims");
        std::fs::create_dir_all(&shims).unwrap();
        std::fs::write(shims.join("python.bat"), "").unwrap();
        std::fs::write(tools.join("python.exe"), "").unwrap();
        let shims_first =
            std::ffi::OsString::from(format!("{};{}", shims.display(), tools.display()));
        let (program, _) = resolve_launcher_for(
            Path::new("python"),
            &[],
            true,
            Some(&shims_first),
            Some(pathext),
        )
        .unwrap();
        assert_eq!(program, tools.join("python.exe"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn diagnostics_are_redacted_bounded_and_keep_the_reason() {
        let text = "Loading\n\
            Error: No API key found for provider \"openai\"; set OPENAI_API_KEY\n\
            proxy=https://user:hunter22@proxy.invalid:8080/path\n\
            Authorization: Bearer abcdefghijklmnop\n\
            leaked sk-ant-api03-ABCDEFGHIJKLMNOPQRSTUVWX in output\n\
            {\"apiKey\":\"configured-value-1\",\"token\": \"short\"}\n\
            unexpected token in JSON at position 3\n";
        let known = vec!["configured-value-1".to_owned()];
        let tail = diagnostic_tail(text.as_bytes(), &known);
        for secret in [
            "hunter22",
            "abcdefghijklmnop",
            "sk-ant-api03",
            "configured-value-1",
        ] {
            assert!(!tail.contains(secret), "{secret} in {tail}");
        }
        for kept in [
            "No API key found for provider \"openai\"; set OPENAI_API_KEY",
            "proxy.invalid:8080",
            "unexpected token in JSON",
            " | ",
        ] {
            assert!(tail.contains(kept), "{kept} missing from {tail}");
        }
        assert!(!tail.contains('\n'));
        let long = format!(
            "{}\nfinal reason\u{1b}[0m\n",
            "é".repeat(STDERR_WINDOW_BYTES)
        );
        let tail = diagnostic_tail(long.as_bytes(), &[]);
        assert!(tail.len() <= STDERR_TAIL_BYTES + "…".len() && tail.ends_with("final reason[0m"));
    }
}
