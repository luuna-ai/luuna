//! Project discovery, `init`, and a reader for the `config.yaml` subset the
//! Go tool writes (`internal/config/config.go`, `internal/config/legacy.go`).
//!
//! - **Discovery** ([`find_dir`]) walks up from a directory to the first
//!   `.luuna/missions/config.yaml`, as Go's `FindDir` does, and resolves a
//!   linked git worktree to the main checkout's store when that store is
//!   initialised (`resolveWorktreeRoot`). It runs `git rev-parse` exactly as
//!   Go does, with the caller's environment.
//! - **Legacy layout.** Go migrates a `.seeds/` folder to `.luuna/missions/`
//!   at every directory it visits. That migration is ported in
//!   s4-missions-lib-3; until then every case where Go would migrate or
//!   refuse returns [`Error::Unsupported`]. A `.seeds` entry that Go leaves
//!   alone is left alone here, too.
//! - **`init`** ([`init`]) writes exactly the files Go's `config.Init`
//!   writes, with the same modes.
//! - **Reading.** Go reads `config.yaml` with yaml.v3 and writes it with its
//!   own minimal emitter (`internal/config/tree.go:57-129`) or, in `init`,
//!   with a fixed template. [`read_config`] reads the forms those write for
//!   top-level scalars: `key: value` lines with a plain or double-quoted key
//!   and a plain, double-quoted, empty, `{}` or `[]` value, resolved as
//!   yaml.v3 resolves them (including its integers in other bases and its
//!   floats, so a string the emitter writes plain, such as `1e3` or `Null`,
//!   is not a string when read back, SD20). Anything else (nested blocks,
//!   sequences, comments, single quotes, a repeated key, an indented or
//!   CRLF line, a timestamp-shaped plain value) returns
//!   [`Error::Unsupported`] naming s4-missions-lib-3, which ports the full
//!   `config` command; it is never read differently from Go.
//!
//! Errors from the file system are [`Error::Io`]: their text is not Go's,
//! so the command layer returns them rather than printing them.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::error::{Error, Result, PLAN_3};
use crate::gopath;

/// `model.MissionsDir`.
pub(crate) const MISSIONS_DIR: &str = ".luuna/missions";
/// `model.ConfigFile`.
pub(crate) const CONFIG_FILE: &str = "config.yaml";
/// `model.DefaultMaxPlanDepth`.
pub(crate) const DEFAULT_MAX_PLAN_DEPTH: i64 = 3;
/// The legacy store folder (`legacyDir`).
const LEGACY_DIR: &str = ".seeds";

/// The lines `init` keeps in the project's `.gitattributes`
/// (`mergeUnionLines`).
const MERGE_UNION_LINES: [&str; 3] = [
    ".luuna/missions/issues.jsonl merge=union",
    ".luuna/missions/templates.jsonl merge=union",
    ".luuna/missions/plans.jsonl merge=union",
];

/// The environment a child process such as git gets.
#[derive(Debug, Clone, Copy)]
pub(crate) enum ProcessEnv<'a> {
    /// The calling process's environment, as Go's `exec.Command` passes it.
    Inherit,
    /// Exactly these variables (an [`Invocation`](crate::command::Invocation)'s
    /// environment).
    Exactly(&'a BTreeMap<String, String>),
}

impl ProcessEnv<'_> {
    fn path(&self) -> Option<std::ffi::OsString> {
        match self {
            Self::Inherit => std::env::var_os("PATH"),
            Self::Exactly(env) => env.get("PATH").map(Into::into),
        }
    }
}

// ---------------------------------------------------------------------------
// Discovery
// ---------------------------------------------------------------------------

/// Go's `FindDir`: the `.luuna/missions` directory found by walking up from
/// `start`, resolved to the main checkout's store from a linked worktree.
pub(crate) fn find_dir(start: &Path, env: ProcessEnv<'_>) -> Result<PathBuf> {
    let mut dir = gopath::abs(start).map_err(|error| Error::io("getwd", start, error))?;
    loop {
        refuse_legacy_layout(&dir)?;
        let candidate = dir.join(MISSIONS_DIR);
        let config = candidate.join(CONFIG_FILE);
        match fs::metadata(&config) {
            Ok(_) => return Ok(resolve_worktree_root(&candidate, env)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(Error::io("stat", config, error)),
        }
        let parent = gopath::dir(&dir);
        if parent == dir {
            return Err(Error::NotInProject);
        }
        dir = parent;
    }
}

/// Go's `ProjectRoot`: the directory two levels above the store.
pub(crate) fn project_root(missions_dir: &Path) -> PathBuf {
    gopath::dir(&gopath::dir(missions_dir))
}

/// Go's `resolveWorktreeRoot`.
fn resolve_worktree_root(candidate: &Path, env: ProcessEnv<'_>) -> PathBuf {
    let root = project_root(candidate);
    let Some(common) = git_path(&root, "--git-common-dir", env) else {
        return candidate.to_path_buf();
    };
    let mut main_root = gopath::dir(&common);
    if gopath::base(&common) != Path::new(".git") {
        main_root = gopath::dir(&main_root);
    }
    if same_path(&main_root, &root) {
        return candidate.to_path_buf();
    }
    let main_missions = main_root.join(MISSIONS_DIR);
    if fs::metadata(main_missions.join(CONFIG_FILE)).is_ok() {
        return main_missions;
    }
    candidate.to_path_buf()
}

/// Go's `samePath`.
fn same_path(a: &Path, b: &Path) -> bool {
    let (a, b) = (gopath::clean(a), gopath::clean(b));
    if cfg!(windows) {
        a.to_string_lossy().to_lowercase() == b.to_string_lossy().to_lowercase()
    } else {
        a == b
    }
}

/// Go's `gitPath`: `git rev-parse <arg>` in `cwd`, trimmed and made
/// absolute; `None` where Go returns `""` (git missing or failing, or empty
/// output).
fn git_path(cwd: &Path, arg: &str, env: ProcessEnv<'_>) -> Option<PathBuf> {
    let git = look_path("git", env)?;
    let mut command = Command::new(git);
    command
        .arg("rev-parse")
        .arg(arg)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    if let ProcessEnv::Exactly(variables) = env {
        command.env_clear().envs(variables);
    }
    // Go's exec.Cmd adds PWD for a command with a directory and the
    // inherited environment.
    if !cfg!(windows) {
        if let Ok(pwd) = gopath::abs(cwd) {
            command.env("PWD", pwd);
        }
    }
    let output = command.output().ok()?;
    if !output.status.success() {
        return None;
    }
    let raw = crate::store::trim_space(&output.stdout);
    if raw.is_empty() {
        return None;
    }
    let raw = path_from_bytes(raw);
    if raw.is_absolute() {
        Some(gopath::clean(&raw))
    } else {
        Some(gopath::clean(&cwd.join(raw)))
    }
}

#[cfg(unix)]
fn path_from_bytes(bytes: &[u8]) -> PathBuf {
    use std::os::unix::ffi::OsStrExt;
    PathBuf::from(std::ffi::OsStr::from_bytes(bytes))
}

#[cfg(not(unix))]
fn path_from_bytes(bytes: &[u8]) -> PathBuf {
    PathBuf::from(String::from_utf8_lossy(bytes).into_owned())
}

/// Go's `exec.LookPath` on Unix: the first executable `name` in the
/// environment's `PATH`, an empty entry meaning `.`. A match found through a
/// relative entry fails, as Go's `ErrDot` makes the command fail. No `PATH`,
/// or an empty one, finds nothing.
#[cfg(unix)]
fn look_path(name: &str, env: ProcessEnv<'_>) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    let path = env.path()?;
    if path.is_empty() {
        return None;
    }
    for dir in std::env::split_paths(&path) {
        let dir = if dir.as_os_str().is_empty() {
            PathBuf::from(".")
        } else {
            dir
        };
        let candidate = dir.join(name);
        let Ok(meta) = fs::metadata(&candidate) else {
            continue;
        };
        if meta.is_dir() || meta.permissions().mode() & 0o111 == 0 {
            continue;
        }
        return candidate.is_absolute().then_some(candidate);
    }
    None
}

/// Elsewhere the standard library's lookup is used. Not verified on this
/// host (see `README.md`).
#[cfg(not(unix))]
fn look_path(name: &str, env: ProcessEnv<'_>) -> Option<PathBuf> {
    env.path()?;
    Some(PathBuf::from(name))
}

/// Go's `MigrateLegacyLayout`, up to the point where it would change
/// anything: a `.seeds` entry Go leaves alone is accepted, and every case
/// where Go migrates it or refuses is [`Error::Unsupported`] until
/// s4-missions-lib-3.
pub(crate) fn refuse_legacy_layout(project_root: &Path) -> Result<()> {
    let legacy = project_root.join(LEGACY_DIR);
    let Some(legacy_meta) = lstat(&legacy)? else {
        return Ok(());
    };
    let target_exists = lstat(&project_root.join(MISSIONS_DIR))?.is_some();
    let luuna_is_link =
        lstat(&project_root.join(".luuna"))?.is_some_and(|meta| meta.file_type().is_symlink());
    let left_alone = !target_exists
        && !legacy_meta.file_type().is_symlink()
        && !luuna_is_link
        && !legacy_meta.is_dir();
    if left_alone {
        return Ok(());
    }
    Err(Error::unsupported(
        format!(
            "the legacy {LEGACY_DIR}/ layout (found {})",
            legacy.display()
        ),
        PLAN_3,
    ))
}

fn lstat(path: &Path) -> Result<Option<fs::Metadata>> {
    match fs::symlink_metadata(path) {
        Ok(meta) => Ok(Some(meta)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(Error::io("lstat", path, error)),
    }
}

// ---------------------------------------------------------------------------
// init
// ---------------------------------------------------------------------------

/// Go's `config.Init`: creates the store with exactly the files Go creates,
/// or, when `config.yaml` exists, only backfills `.gitattributes`.
pub(crate) fn init(project_root: &Path) -> Result<()> {
    refuse_legacy_layout(project_root)?;
    let dir = project_root.join(MISSIONS_DIR);
    let config = dir.join(CONFIG_FILE);
    match fs::metadata(&config) {
        Ok(_) => return ensure_gitattributes(project_root),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(Error::io("stat", config, error)),
    }
    mkdir_all(&dir)?;
    let mut content = b"project: \"".to_vec();
    content.extend_from_slice(&gopath::bytes(&gopath::base(project_root)));
    content.extend_from_slice(
        format!("\"\nversion: \"1\"\nmax_plan_depth: {DEFAULT_MAX_PLAN_DEPTH}\n").as_bytes(),
    );
    write_file(&config, &content)?;
    for name in ["issues.jsonl", "templates.jsonl", "plans.jsonl"] {
        write_file(&dir.join(name), b"")?;
    }
    write_file(&dir.join(".gitignore"), b"*.lock\n")?;
    ensure_gitattributes(project_root)
}

/// Go's `ensureGitattributes`: appends the merge=union lines that are
/// missing, adding a newline first if the file does not end with one.
fn ensure_gitattributes(root: &Path) -> Result<()> {
    let path = root.join(".gitattributes");
    let mut text = match fs::read(&path) {
        Ok(content) => content,
        Err(error) if error.kind() == io::ErrorKind::NotFound => Vec::new(),
        Err(error) => return Err(Error::io("read", path, error)),
    };
    let missing: Vec<&str> = MERGE_UNION_LINES
        .into_iter()
        .filter(|line| !contains(&text, line.as_bytes()))
        .collect();
    if missing.is_empty() {
        return Ok(());
    }
    if !text.is_empty() && !text.ends_with(b"\n") {
        text.push(b'\n');
    }
    text.extend_from_slice(missing.join("\n").as_bytes());
    text.push(b'\n');
    write_file(&path, &text)
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

/// Go's `os.MkdirAll(dir, 0755)`.
fn mkdir_all(dir: &Path) -> Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o755);
    builder
        .create(dir)
        .map_err(|error| Error::io("mkdir", dir, error))
}

/// Go's `os.WriteFile(path, content, 0644)`: created with mode 0644 (less
/// the umask) or truncated keeping its mode.
fn write_file(path: &Path, content: &[u8]) -> Result<()> {
    use std::io::Write;
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o644);
    let mut file = options
        .open(path)
        .map_err(|error| Error::io("open", path, error))?;
    file.write_all(content)
        .map_err(|error| Error::io("write", path, error))
}

// ---------------------------------------------------------------------------
// Reading config.yaml
// ---------------------------------------------------------------------------

/// The settings Go's `ReadConfig` returns (`model.Config`).
#[allow(
    dead_code,
    reason = "read by create and plans, ported in t4 and s4-missions-lib-4"
)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Config {
    /// `project`, a string; `missions` when absent or not a string.
    pub project: String,
    /// `version`, a string; `1` when absent or not a string.
    pub version: String,
    /// `max_plan_depth` when it is an integer (or a float with an integer
    /// value).
    pub max_plan_depth: Option<i64>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            project: "missions".to_owned(),
            version: "1".to_owned(),
            max_plan_depth: None,
        }
    }
}

/// Go's `ReadConfig` for the subset of `config.yaml` Go writes.
#[allow(dead_code, reason = "read by create, ported in t4")]
pub(crate) fn read_config(missions_dir: &Path) -> Result<Config> {
    let path = missions_dir.join(CONFIG_FILE);
    let content = fs::read(&path).map_err(|error| Error::io("read", &path, error))?;
    parse_config(&content).map_err(|reason| {
        Error::unsupported(
            format!(
                "reading {} outside the config.yaml subset Go writes ({reason})",
                path.display()
            ),
            PLAN_3,
        )
    })
}

/// A top-level value as yaml.v3 decodes it into `any`, as far as
/// `ReadConfig` tells the kinds apart.
#[derive(Debug, Clone, PartialEq)]
enum Scalar {
    /// `!!str`.
    Str(String),
    /// `!!int` that fits Go's `int`.
    Int(i64),
    /// `!!float`.
    Float(f64),
    /// Anything else: null, a bool, an unsigned integer above `int`, `{}`
    /// or `[]`.
    Other,
}

/// Parses `content` as the subset described in the module documentation.
/// The error says why the content is outside the subset.
fn parse_config(content: &[u8]) -> Result<Config, String> {
    let text = std::str::from_utf8(content).map_err(|_| "not UTF-8".to_owned())?;
    let mut values: BTreeMap<String, Scalar> = BTreeMap::new();
    let mut seen = BTreeSet::new();
    for (index, line) in text.split('\n').enumerate() {
        let at = |reason: &str| format!("line {}: {reason}", index + 1);
        if line.trim_matches(' ').is_empty() {
            continue;
        }
        if let Some(bad) = line.chars().find(|ch| !is_printable(*ch) || *ch == '\r') {
            return Err(at(&format!("character U+{:04X}", u32::from(bad))));
        }
        if line.starts_with([' ', '\t']) {
            return Err(at("an indented line (a nested value)"));
        }
        let (key, rest) = split_entry(line).ok_or_else(|| at("not a `key: value` line"))?;
        if !seen.insert(key.clone()) {
            return Err(at(&format!("the key {key:?} is repeated")));
        }
        let value = parse_value(rest).map_err(|reason| at(&reason))?;
        values.insert(key, value);
    }
    let mut config = Config::default();
    if let Some(Scalar::Str(project)) = values.get("project") {
        config.project = project.clone();
    }
    if let Some(Scalar::Str(version)) = values.get("version") {
        config.version = version.clone();
    }
    config.max_plan_depth = match values.get("max_plan_depth") {
        Some(Scalar::Int(depth)) => Some(*depth),
        // Go's `integer`: a float64 with an integer value converts.
        Some(Scalar::Float(depth))
            if depth.fract() == 0.0
                && (-9.223_372_036_854_776e18..9.223_372_036_854_776e18).contains(depth) =>
        {
            Some(*depth as i64)
        }
        _ => None,
    };
    Ok(config)
}

/// Splits `key: value` (or `key:` at the end of the line) into the decoded
/// key and the rest after the colon.
fn split_entry(line: &str) -> Option<(String, &str)> {
    if line.starts_with('"') {
        let (key, end) = double_quoted(line).ok()?;
        let rest = line[end..].strip_prefix(':')?;
        return Some((key, rest));
    }
    let colon = line
        .match_indices(':')
        .map(|(at, _)| at)
        .find(|at| matches!(line.as_bytes().get(at + 1), None | Some(b' ')))?;
    let key = &line[..colon];
    // Go's emitter writes a key plain only without these characters
    // (`quoteKey`); keys starting with `-` or `?` are not read here.
    let plain = !key.is_empty()
        && !key.starts_with(['-', '?'])
        && !key.contains(|ch: char| ch.is_ascii_whitespace() || "\"'[]{},:#&*!|>%@`".contains(ch));
    if !plain || resolve_plain(key) != Ok(Scalar::Str(key.to_owned())) {
        return None;
    }
    Some((key.to_owned(), &line[colon + 1..]))
}

/// The value after a key's colon.
fn parse_value(rest: &str) -> Result<Scalar, String> {
    if rest.is_empty() {
        return Ok(Scalar::Other);
    }
    let Some(value) = rest.strip_prefix(' ') else {
        return Err("no space after the colon".to_owned());
    };
    let value = value.trim_start_matches(' ');
    if value.starts_with('\t') {
        return Err("a tab before the value".to_owned());
    }
    if value.is_empty() {
        return Ok(Scalar::Other);
    }
    if value.starts_with('"') {
        let (text, end) = double_quoted(value)?;
        if !value[end..].trim_end_matches(' ').is_empty() {
            return Err("text after a quoted value".to_owned());
        }
        return Ok(Scalar::Str(text));
    }
    let value = value.trim_end_matches([' ', '\t']);
    if value == "{}" || value == "[]" {
        return Ok(Scalar::Other);
    }
    let indicator = value.starts_with(|ch: char| "[]{},#&*!|>'\"%@`".contains(ch))
        || matches!(value, "-" | "?" | ":")
        || value.starts_with("- ")
        || value.starts_with("? ")
        || value.starts_with(": ");
    if indicator
        || value.contains(": ")
        || value.contains(" #")
        || value.contains("\t#")
        || value.ends_with(':')
    {
        return Err(format!("the plain value {value:?}"));
    }
    resolve_plain(value)
}

/// yaml.v3's `resolve` for an untagged plain scalar (`resolve.go`). Only a
/// value shaped like a timestamp (four digits and `-`) is outside the
/// subset, because whether Go's `time.Parse` accepts it decides its type.
fn resolve_plain(value: &str) -> Result<Scalar, String> {
    match value {
        "true" | "True" | "TRUE" | "false" | "False" | "FALSE" | "" | "~" | "null" | "Null"
        | "NULL" => return Ok(Scalar::Other),
        ".nan" | ".NaN" | ".NAN" => return Ok(Scalar::Float(f64::NAN)),
        ".inf" | ".Inf" | ".INF" | "+.inf" | "+.Inf" | "+.INF" => {
            return Ok(Scalar::Float(f64::INFINITY))
        }
        "-.inf" | "-.Inf" | "-.INF" => return Ok(Scalar::Float(f64::NEG_INFINITY)),
        "<<" => return Err("a merge key".to_owned()),
        _ => {}
    }
    let string = || Ok(Scalar::Str(value.to_owned()));
    match value.as_bytes()[0] {
        // `strconv.ParseFloat(in, 64)`; a failure, including overflow, is a
        // string.
        b'.' => match is_yaml_float(value).then(|| value.parse::<f64>()) {
            Some(Ok(number)) if number.is_finite() => Ok(Scalar::Float(number)),
            _ => string(),
        },
        b'+' | b'-' | b'0'..=b'9' => {
            let digits = value.bytes().take_while(u8::is_ascii_digit).count();
            if digits == 4 && value.as_bytes().get(4) == Some(&b'-') {
                return Err(format!("the timestamp-like value {value:?}"));
            }
            let plain = value.replace('_', "");
            if let Some(magnitude) = parse_base0(&plain) {
                let negative = plain.starts_with('-');
                let limit = (1u128 << 63) - u128::from(!negative);
                match magnitude {
                    Some(magnitude) if magnitude <= limit => {
                        let magnitude = magnitude as i128;
                        let number = if negative { -magnitude } else { magnitude };
                        return Ok(Scalar::Int(number as i64));
                    }
                    // `strconv.ParseUint` (no sign allowed): a uint64, which
                    // ReadConfig takes as neither a string nor an int.
                    Some(magnitude)
                        if !plain.starts_with(['+', '-']) && magnitude <= u128::from(u64::MAX) =>
                    {
                        return Ok(Scalar::Other)
                    }
                    _ => {}
                }
            }
            if is_yaml_float(&plain) {
                if let Ok(number) = plain.parse::<f64>() {
                    if number.is_finite() {
                        return Ok(Scalar::Float(number));
                    }
                }
            }
            // yaml.v3's later `0b` and `0o` cases accept nothing that
            // `ParseInt` with base 0 has not already accepted.
            string()
        }
        _ => string(),
    }
}

/// The syntax of Go's `strconv.ParseInt(text, 0, 64)` for text without
/// underscores: an optional sign, then `0x`, `0o`, `0b`, a leading `0`
/// (octal) or decimal digits. `None` is a syntax error; otherwise the
/// magnitude, `None` inside when it does not fit 128 bits.
fn parse_base0(text: &str) -> Option<Option<u128>> {
    let unsigned = text.strip_prefix(['+', '-']).unwrap_or(text);
    let lower = unsigned.to_ascii_lowercase();
    let (radix, digits) = if let Some(rest) = lower.strip_prefix("0x") {
        (16, rest)
    } else if let Some(rest) = lower.strip_prefix("0o") {
        (8, rest)
    } else if let Some(rest) = lower.strip_prefix("0b") {
        (2, rest)
    } else if lower.len() > 1 && lower.starts_with('0') {
        (8, &lower[1..])
    } else {
        (10, lower.as_str())
    };
    if digits.is_empty() || !digits.chars().all(|ch| ch.is_digit(radix)) {
        return None;
    }
    Some(digits.chars().try_fold(0u128, |total, ch| {
        total
            .checked_mul(u128::from(radix))?
            .checked_add(u128::from(ch.to_digit(radix)?))
    }))
}

/// yaml.v3's `yamlStyleFloat`:
/// `^[-+]?(\.[0-9]+|[0-9]+(\.[0-9]*)?)([eE][-+]?[0-9]+)?$`.
fn is_yaml_float(text: &str) -> bool {
    let bytes = text.as_bytes();
    let mut at = usize::from(matches!(bytes.first(), Some(b'+' | b'-')));
    let run = |from: usize| {
        bytes[from..]
            .iter()
            .take_while(|byte| byte.is_ascii_digit())
            .count()
    };
    if bytes.get(at) == Some(&b'.') {
        let fraction = run(at + 1);
        if fraction == 0 {
            return false;
        }
        at += 1 + fraction;
    } else {
        let whole = run(at);
        if whole == 0 {
            return false;
        }
        at += whole;
        if bytes.get(at) == Some(&b'.') {
            at += 1 + run(at + 1);
        }
    }
    if matches!(bytes.get(at), Some(b'e' | b'E')) {
        at += 1;
        if matches!(bytes.get(at), Some(b'+' | b'-')) {
            at += 1;
        }
        let exponent = run(at);
        if exponent == 0 {
            return false;
        }
        at += exponent;
    }
    at == bytes.len()
}

/// A YAML double-quoted scalar on one line, starting at `text[0] == '"'`:
/// the decoded string and the index after the closing quote.
fn double_quoted(text: &str) -> Result<(String, usize), String> {
    let mut out = String::new();
    let mut chars = text.char_indices().skip(1);
    while let Some((at, ch)) = chars.next() {
        match ch {
            '"' => return Ok((out, at + 1)),
            '\\' => {
                let Some((_, escape)) = chars.next() else {
                    return Err("a quoted value continued on the next line".to_owned());
                };
                let simple = match escape {
                    '0' => Some('\0'),
                    'a' => Some('\u{7}'),
                    'b' => Some('\u{8}'),
                    't' | '\t' => Some('\t'),
                    'n' => Some('\n'),
                    'v' => Some('\u{b}'),
                    'f' => Some('\u{c}'),
                    'r' => Some('\r'),
                    'e' => Some('\u{1b}'),
                    ' ' => Some(' '),
                    '"' => Some('"'),
                    '/' => Some('/'),
                    '\\' => Some('\\'),
                    'N' => Some('\u{85}'),
                    '_' => Some('\u{a0}'),
                    'L' => Some('\u{2028}'),
                    'P' => Some('\u{2029}'),
                    _ => None,
                };
                if let Some(simple) = simple {
                    out.push(simple);
                    continue;
                }
                let width = match escape {
                    'x' => 2,
                    'u' => 4,
                    'U' => 8,
                    other => return Err(format!("the escape \\{other}")),
                };
                let mut code = 0u32;
                for _ in 0..width {
                    let digit = chars
                        .next()
                        .and_then(|(_, ch)| ch.to_digit(16))
                        .ok_or_else(|| "a short hexadecimal escape".to_owned())?;
                    code = code * 16 + digit;
                }
                out.push(char::from_u32(code).ok_or_else(|| format!("the escape code {code:#x}"))?);
            }
            other => out.push(other),
        }
    }
    Err("an unterminated quoted value".to_owned())
}

/// YAML's printable characters (tab and the printable ranges); yaml.v3
/// rejects a document with any other.
fn is_printable(ch: char) -> bool {
    matches!(ch,
        '\t' | '\n' | '\r' | '\u{20}'..='\u{7e}' | '\u{85}' | '\u{a0}'..='\u{d7ff}'
        | '\u{e000}'..='\u{fffd}' | '\u{10000}'..='\u{10ffff}')
        && ch != '\u{feff}'
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(text: &str) -> Config {
        parse_config(text.as_bytes()).unwrap_or_else(|reason| panic!("{text:?}: {reason}"))
    }

    fn config(project: &str, version: &str, depth: Option<i64>) -> Config {
        Config {
            project: project.to_owned(),
            version: version.to_owned(),
            max_plan_depth: depth,
        }
    }

    #[test]
    fn the_init_form_is_read() {
        assert_eq!(
            parsed("project: \"project\"\nversion: \"1\"\nmax_plan_depth: 3\n"),
            config("project", "1", Some(3))
        );
    }

    #[test]
    fn the_emitter_forms_are_read() {
        // Plain strings, as the config emitter writes them, and the forms
        // the regression fixtures use.
        assert_eq!(
            parsed("project: parity\nversion: \"1\"\n"),
            config("parity", "1", None)
        );
        assert_eq!(
            parsed("project: parity\nversion: \"1\"\nmax_plan_depth: 5\n"),
            config("parity", "1", Some(5))
        );
        // strconv.Quote escapes, read as YAML double-quoted escapes.
        assert_eq!(
            parsed("project: \"a\\\"b\\\\c\\td\\u00e9\\x41\\U0001F600\"\n").project,
            "a\"b\\c\td\u{e9}A\u{1F600}"
        );
        // Quoted keys, other keys, null, {} and [] values, floats and bools.
        assert_eq!(
            parsed("\"project\": x\nother:\nlist: []\nmap: {}\nflag: true\nmax_plan_depth: 4.0\n"),
            config("x", "1", Some(4))
        );
        assert_eq!(parsed(""), Config::default());
        assert_eq!(parsed("\n  \n"), Config::default());
    }

    #[test]
    fn values_resolve_as_yaml_v3_resolves_them() {
        // Not a string: Go keeps the default.
        assert_eq!(
            parsed("project: 12\nversion: 1\n"),
            config("missions", "1", None)
        );
        assert_eq!(parsed("project: true\n").project, "missions");
        assert_eq!(parsed("project: ~\n").project, "missions");
        assert_eq!(parsed("project: .inf\n").project, "missions");
        // Strings yaml.v3 does not resolve to another type.
        assert_eq!(parsed("project: yes\n").project, "yes");
        assert_eq!(parsed("project: no-1\n").project, "no-1");
        assert_eq!(parsed("project: -x\n").project, "-x");
        assert_eq!(parsed("project: .x\n").project, ".x");
        assert_eq!(parsed("project: a b  \n").project, "a b");
        // A quoted number is a string, so it is no depth.
        assert_eq!(parsed("max_plan_depth: \"3\"\n").max_plan_depth, None);
        assert_eq!(parsed("max_plan_depth: -2\n").max_plan_depth, Some(-2));
        assert_eq!(parsed("max_plan_depth: 2.5\n").max_plan_depth, None);
        assert_eq!(
            parsed("max_plan_depth: 18446744073709551615\n").max_plan_depth,
            None
        );
        // Go's ParseInt with base 0 after yaml.v3 removes underscores, then
        // ParseUint, then yaml.v3's float pattern. Each project result was
        // checked against the Go binary (the id prefix `create` prints).
        for (text, depth) in [
            ("0x10", Some(16)),
            ("0O17", Some(15)),
            ("-0b101", Some(-5)),
            ("010", Some(8)),
            ("08", Some(8)),
            ("1_000", Some(1000)),
            ("+7", Some(7)),
            ("1e3", Some(1000)),
            ("5.", Some(5)),
            (".5e1", Some(5)),
            ("-9223372036854775808", Some(i64::MIN)),
            ("9223372036854775808", None),
        ] {
            let config = parsed(&format!("max_plan_depth: {text}\nproject: {text}\n"));
            assert_eq!(config.max_plan_depth, depth, "{text}");
            assert_eq!(config.project, "missions", "{text}");
        }
        // These stay strings in yaml.v3: a project, and no depth.
        for text in [
            "0x1FFFFFFFFFFFFFFFF",
            "1e400",
            "0x",
            "1e",
            "1.2.3",
            "-0xFFFFFFFFFFFFFFFFF",
            ".5x",
            "+-1",
        ] {
            let config = parsed(&format!("project: {text}\nmax_plan_depth: {text}\n"));
            assert_eq!(config.project, text, "{text}");
            assert_eq!(config.max_plan_depth, None, "{text}");
        }
        // The emitter writes these plain, and Go reads them back as other
        // types (SD20).
        assert_eq!(parsed("project: 1e3\n").project, "missions");
        assert_eq!(parsed("project: Null\n").project, "missions");
    }

    #[test]
    fn other_yaml_is_rejected_explicitly() {
        for text in [
            "project:\n  nested: x\n",
            "templates:\n- a\n",
            "# comment\nproject: x\n",
            "project: x # comment\n",
            "project: 'single'\n",
            "project: \"open\n",
            "project: \"a\"b\"\n",
            "project: [a]\n",
            "project: 2024-01-01\n",
            "project: 1999-x\n",
            "project: a: b\n",
            "project:x\n",
            "---\n",
            "project: x\nproject: y\n",
            "project: x\r\n",
            "project: \"\\q\"\n",
            "- a\n",
            "<<: x\n",
            "1: x\n",
        ] {
            assert!(
                parse_config(text.as_bytes()).is_err(),
                "{text:?} was accepted"
            );
        }
        assert!(parse_config(b"project: \xff\n").is_err());
    }

    #[test]
    fn reading_outside_the_subset_is_unsupported() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(CONFIG_FILE), "project:\n  nested: x\n").unwrap();
        let error = read_config(dir.path()).unwrap_err();
        assert!(
            matches!(&error, Error::Unsupported { plan, .. } if plan == PLAN_3),
            "{error}"
        );
        fs::write(dir.path().join(CONFIG_FILE), "project: parity\n").unwrap();
        assert_eq!(read_config(dir.path()).unwrap().project, "parity");
    }

    /// A project folder with an initialised store, under the system temp
    /// folder (AS7).
    fn initialised(root: &Path) -> PathBuf {
        let dir = root.join(MISSIONS_DIR);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(CONFIG_FILE), "project: \"p\"\n").unwrap();
        dir
    }

    /// Runs git in `cwd` for a test repository the test created under the
    /// system temp folder, with its own identity and no redirecting
    /// variables (A4 `tests_with_git`).
    fn git(cwd: &Path, args: &[&str]) {
        let null = if cfg!(windows) { "NUL" } else { "/dev/null" };
        let status = Command::new("git")
            .args(args)
            .current_dir(cwd)
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", null)
            .env("GIT_AUTHOR_NAME", "Luuna Missions Test")
            .env("GIT_AUTHOR_EMAIL", "test@luuna.invalid")
            .env("GIT_COMMITTER_NAME", "Luuna Missions Test")
            .env("GIT_COMMITTER_EMAIL", "test@luuna.invalid")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?} in {}", cwd.display());
    }

    fn same(a: &Path, b: &Path) -> bool {
        fs::canonicalize(a).unwrap() == fs::canonicalize(b).unwrap()
    }

    #[test]
    fn discovery_walks_up_to_the_first_config_yaml() {
        let temp = tempfile::tempdir().unwrap();
        let outer = initialised(temp.path());
        let project = temp.path().join("project");
        let inner = initialised(&project);
        let deep = project.join("a").join("b");
        fs::create_dir_all(&deep).unwrap();
        // The nearest store wins; directories without config.yaml are passed.
        fs::create_dir_all(project.join("a").join(MISSIONS_DIR)).unwrap();
        let found = find_dir(&deep, ProcessEnv::Inherit).unwrap();
        assert_eq!(found, inner);
        let found = find_dir(&temp.path().join("project/a/../a/./b"), ProcessEnv::Inherit);
        assert_eq!(found.unwrap(), inner);
        assert_eq!(find_dir(temp.path(), ProcessEnv::Inherit).unwrap(), outer);
        let store = crate::Store::discover(&deep).unwrap();
        assert_eq!(store.dir(), inner);
    }

    #[test]
    fn discovery_without_a_store_reports_go_s_message() {
        let temp = tempfile::tempdir().unwrap();
        let error = find_dir(temp.path(), ProcessEnv::Inherit).unwrap_err();
        assert!(matches!(error, Error::NotInProject));
        assert_eq!(
            error.to_string(),
            "Not in a missions project. Run `luuna-missions init` first."
        );
    }

    #[test]
    fn a_linked_worktree_resolves_to_the_main_checkout_s_store() {
        let temp = tempfile::tempdir().unwrap();
        let main = temp.path().join("main");
        fs::create_dir_all(&main).unwrap();
        git(&main, &["init", "-q"]);
        let main_store = initialised(&main);
        git(&main, &["add", "-A"]);
        git(&main, &["commit", "-q", "-m", "init"]);
        git(&main, &["worktree", "add", "-q", "../linked"]);
        let linked = temp.path().join("linked");
        let linked_store = linked.join(MISSIONS_DIR);
        assert!(linked_store.join(CONFIG_FILE).is_file());
        let deep = linked.join("src");
        fs::create_dir_all(&deep).unwrap();

        // From the linked worktree: the main checkout's store.
        let found = find_dir(&deep, ProcessEnv::Inherit).unwrap();
        assert!(same(&found, &main_store), "{}", found.display());
        // From the main checkout: its own store.
        let found = find_dir(&main, ProcessEnv::Inherit).unwrap();
        assert_eq!(found, main_store);
        // git runs in the given environment: without PATH it is not found,
        // as in Go, and the worktree keeps its own store.
        let no_path = BTreeMap::new();
        let found = find_dir(&deep, ProcessEnv::Exactly(&no_path)).unwrap();
        assert_eq!(found, linked_store);
        let with_path =
            BTreeMap::from([("PATH".to_owned(), std::env::var("PATH").unwrap_or_default())]);
        let found = find_dir(&deep, ProcessEnv::Exactly(&with_path)).unwrap();
        assert!(same(&found, &main_store), "{}", found.display());
        // A main checkout whose store is not initialised is not used.
        fs::remove_file(main_store.join(CONFIG_FILE)).unwrap();
        let found = find_dir(&deep, ProcessEnv::Inherit).unwrap();
        assert_eq!(found, linked_store);
    }

    #[cfg(unix)]
    #[test]
    fn a_legacy_seeds_folder_is_refused_as_unsupported() {
        let refused = |root: &Path| match find_dir(root, ProcessEnv::Inherit) {
            Err(Error::Unsupported { plan, feature }) => {
                assert_eq!(plan, PLAN_3);
                assert!(feature.contains(".seeds"), "{feature}");
            }
            other => panic!("expected Unsupported, got {other:?}"),
        };
        // A folder Go would migrate.
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir(temp.path().join(LEGACY_DIR)).unwrap();
        refused(temp.path());
        assert!(temp.path().join(LEGACY_DIR).is_dir(), "nothing was moved");
        // Both layouts, which Go refuses.
        let temp = tempfile::tempdir().unwrap();
        initialised(temp.path());
        fs::create_dir(temp.path().join(LEGACY_DIR)).unwrap();
        refused(temp.path());
        // A symbolic link, which Go refuses.
        let temp = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink("elsewhere", temp.path().join(LEGACY_DIR)).unwrap();
        refused(temp.path());
        // In a parent directory visited on the way up.
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir(temp.path().join(LEGACY_DIR)).unwrap();
        let child = temp.path().join("child");
        fs::create_dir(&child).unwrap();
        refused(&child);
        // A plain file Go leaves alone is left alone.
        let temp = tempfile::tempdir().unwrap();
        let store = initialised(&temp.path().join("p"));
        fs::write(temp.path().join("p").join(LEGACY_DIR), "x").unwrap();
        assert!(
            refuse_legacy_layout(&temp.path().join("p")).is_err(),
            "store exists"
        );
        fs::remove_dir_all(&store).unwrap();
        let fresh = temp.path().join("q");
        initialised(&fresh.join("inner"));
        fs::write(fresh.join(LEGACY_DIR), "x").unwrap();
        refuse_legacy_layout(&fresh).unwrap();
        assert_eq!(
            find_dir(&fresh.join("inner"), ProcessEnv::Inherit).unwrap(),
            fresh.join("inner").join(MISSIONS_DIR)
        );
    }
}
