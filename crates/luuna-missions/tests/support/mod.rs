//! Comparison harness shared by `tests/parity.rs` and
//! `tests/format_roundtrip.rs`.
//!
//! Frozen after t1 of s4-missions-lib, and frozen for s4-missions-lib-2 to -4.
//!
//! - Test projects live under `std::env::temp_dir()`, never inside the luuna
//!   worktree or the reference clone, so Go-style upward store discovery and
//!   git resolution cannot reach a workspace repository.
//! - Every git command that writes runs through [`git_in_temp_repo`], which
//!   refuses any directory outside the system temp folder, sets its own
//!   identity, passes `-c user.name`/`-c user.email`, never writes git
//!   configuration, and clears `GIT_DIR`, `GIT_WORK_TREE` and
//!   `GIT_INDEX_FILE`. Every other git command is read-only and runs through
//!   [`git_read`].
//! - Output is normalised as the Go regression harness does it
//!   (`internal/regression/compatibility_test.go` at the reference commit):
//!   CRLF folded, project roots replaced by `<PROJECT>`, bound ids replaced by
//!   `<label>`, and ISO timestamps replaced by `<ISO>`. Nothing else is
//!   normalised. Rust is compared with Go byte for byte after that
//!   normalisation, including every file and directory in the project tree
//!   and its permission bits.
#![allow(dead_code)]

pub mod fixtures;
pub mod reference;

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fs;
use std::io::{Read, Write};
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use serde_json::Value;

/// How long one command may run, as in the Go harness.
pub const COMMAND_TIMEOUT: Duration = Duration::from_secs(30);

/// The name of every test project directory. The fixtures were captured in a
/// directory named `project`, which is also the ticket id prefix.
pub const PROJECT_DIR_NAME: &str = "project";

/// Git environment variables that would redirect a git command to another
/// repository. They are removed from every git child.
const GIT_REDIRECTS: [&str; 5] = [
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_COMMON_DIR",
    "GIT_OBJECT_DIRECTORY",
];

/// The luuna worktree that contains this crate.
pub fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("canonical luuna worktree root")
}

/// The harness's own folder, `target/tmp/missions-parity`: the Go binary,
/// GOCACHE, GOMODCACHE, GOTMPDIR and the reports.
pub fn parity_dir() -> PathBuf {
    Path::new(env!("CARGO_TARGET_TMPDIR")).join("missions-parity")
}

/// The canonical system temp folder.
pub fn system_temp_dir() -> PathBuf {
    std::env::temp_dir()
        .canonicalize()
        .expect("canonical system temp folder")
}

fn git_command(dir: &Path) -> Command {
    let mut command = Command::new("git");
    command.arg("-C").arg(dir);
    for key in GIT_REDIRECTS {
        command.env_remove(key);
    }
    command
}

/// Runs a read-only git command in `dir` and returns its stdout, or a message
/// naming the command and its stderr.
pub fn git_read(dir: &Path, args: &[&str]) -> Result<String, String> {
    let output = git_command(dir)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("git {} in {}: {error}", args.join(" "), dir.display()))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        Err(format!(
            "git {} in {} exited with {}: {}",
            args.join(" "),
            dir.display(),
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

/// Panics unless `path` is inside the system temp folder and outside the
/// luuna worktree and the reference clone.
pub fn assert_outside_repositories(path: &Path) {
    let path = path
        .canonicalize()
        .unwrap_or_else(|error| panic!("canonicalize {}: {error}", path.display()));
    let temp = system_temp_dir();
    assert!(
        path.starts_with(&temp),
        "{} is not under the system temp folder {}",
        path.display(),
        temp.display()
    );
    let workspace = workspace_root();
    assert!(
        !path.starts_with(&workspace),
        "{} is inside the luuna worktree {}",
        path.display(),
        workspace.display()
    );
    if let Some(reference) = reference::configured_path() {
        if let Ok(reference) = reference.canonicalize() {
            assert!(
                !path.starts_with(&reference),
                "{} is inside the reference clone {}",
                path.display(),
                reference.display()
            );
        }
    }
}

/// The only way the tests run a git command that writes. `repo` must be a
/// temporary directory the test created (see [`assert_outside_repositories`]).
/// The command gets its own identity, no system or global configuration, no
/// hooks or signing, and no `GIT_DIR`, `GIT_WORK_TREE` or `GIT_INDEX_FILE`.
pub fn git_in_temp_repo(repo: &Path, args: &[&str]) -> String {
    assert_outside_repositories(repo);
    let null_config = if cfg!(windows) { "NUL" } else { "/dev/null" };
    let output = git_command(repo)
        .args([
            "-c",
            "user.name=Luuna Missions Parity",
            "-c",
            "user.email=parity@luuna.invalid",
            "-c",
            "commit.gpgsign=false",
            "-c",
            &format!("core.hooksPath={null_config}"),
        ])
        .args(args)
        .env("GIT_AUTHOR_NAME", "Luuna Missions Parity")
        .env("GIT_AUTHOR_EMAIL", "parity@luuna.invalid")
        .env("GIT_COMMITTER_NAME", "Luuna Missions Parity")
        .env("GIT_COMMITTER_EMAIL", "parity@luuna.invalid")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", null_config)
        .stdin(Stdio::null())
        .output()
        .unwrap_or_else(|error| panic!("git {} in {}: {error}", args.join(" "), repo.display()));
    assert!(
        output.status.success(),
        "git {} in {} exited with {}: {}",
        args.join(" "),
        repo.display(),
        output.status,
        String::from_utf8_lossy(&output.stderr).trim()
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// Stages everything in the temporary repository `repo` and records it.
pub fn commit_all(repo: &Path, message: &str) {
    git_in_temp_repo(repo, &["add", "-A"]);
    git_in_temp_repo(repo, &["commit", "-q", "-m", message]);
}

/// A git-initialised project directory named `project` inside a fresh
/// directory of the system temp folder. Removed on drop.
pub struct TempProject {
    holder: tempfile::TempDir,
    root: PathBuf,
}

impl TempProject {
    /// Creates the project and runs `git init -q` in it, as the Go harness
    /// does for every case.
    pub fn new() -> Self {
        let holder = tempfile::Builder::new()
            .prefix("luuna-missions-parity-")
            .tempdir_in(std::env::temp_dir())
            .expect("temporary directory");
        let root = holder.path().join(PROJECT_DIR_NAME);
        create_dir(&root);
        let root = root.canonicalize().expect("canonical project root");
        assert_outside_repositories(&root);
        git_in_temp_repo(&root, &["init", "-q"]);
        let toplevel = git_read(&root, &["rev-parse", "--show-toplevel"]).expect("git toplevel");
        assert_eq!(
            Path::new(toplevel.trim()),
            root,
            "the test project must be its own git repository"
        );
        Self { holder, root }
    }

    /// The canonical project root.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// An empty directory beside the project (not inside it).
    pub fn empty_dir(&self, name: &str) -> PathBuf {
        let dir = self.holder.path().join(name);
        create_dir(&dir);
        dir
    }

    /// Writes `files` (relative slash paths to contents) into the project,
    /// with mode 0644 and parent directories with mode 0755, as Go's
    /// `os.WriteFile` and `os.MkdirAll` do.
    pub fn write_files<'a>(&self, files: impl IntoIterator<Item = (&'a str, Vec<u8>)>) {
        for (name, content) in files {
            let target = self.root.join(name);
            create_dir(target.parent().expect("parent directory"));
            write_file(&target, &content, 0o644);
        }
    }

    /// Copies every file and directory of `other` except `.git` into this
    /// project, keeping permission bits.
    pub fn copy_tree_from(&self, other: &TempProject) {
        for (name, entry) in snapshot_tree(other.root()) {
            let target = self.root.join(&name);
            match entry.kind {
                EntryKind::Dir => {
                    create_dir(&target);
                    set_mode(&target, entry.mode);
                }
                EntryKind::File(content) => {
                    create_dir(target.parent().expect("parent directory"));
                    write_file(&target, &content, entry.mode);
                    set_mode(&target, entry.mode);
                }
                EntryKind::Symlink(_) => panic!("unexpected symlink {name} in a test project"),
            }
        }
    }

    /// The project tree without `.git`.
    pub fn snapshot(&self) -> BTreeMap<String, Entry> {
        snapshot_tree(&self.root)
    }
}

impl Default for TempProject {
    fn default() -> Self {
        Self::new()
    }
}

fn create_dir(dir: &Path) {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o755);
    builder
        .create(dir)
        .unwrap_or_else(|error| panic!("create {}: {error}", dir.display()));
}

fn write_file(path: &Path, content: &[u8], mode: u32) {
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, mode);
    #[cfg(not(unix))]
    let _ = mode;
    let mut file = options
        .open(path)
        .unwrap_or_else(|error| panic!("open {}: {error}", path.display()));
    file.write_all(content)
        .unwrap_or_else(|error| panic!("write {}: {error}", path.display()));
}

fn set_mode(path: &Path, mode: u32) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(mode))
            .unwrap_or_else(|error| panic!("chmod {}: {error}", path.display()));
    }
    #[cfg(not(unix))]
    let _ = (path, mode);
}

/// One entry of a project tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// What the entry is, with its content.
    pub kind: EntryKind,
    /// Permission bits (`mode & 0o7777`); 0 where the platform has none.
    pub mode: u32,
}

/// The kind of a tree entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntryKind {
    /// A directory.
    Dir,
    /// A regular file and its bytes.
    File(Vec<u8>),
    /// A symbolic link and its target.
    Symlink(PathBuf),
}

/// Every file, directory and symlink under `root` by slash path, skipping
/// directories named `.git` at any depth, as the Go harness does.
pub fn snapshot_tree(root: &Path) -> BTreeMap<String, Entry> {
    let mut entries = BTreeMap::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let listing =
            fs::read_dir(&dir).unwrap_or_else(|error| panic!("read {}: {error}", dir.display()));
        for item in listing {
            let item = item.expect("directory entry");
            let path = item.path();
            let meta = fs::symlink_metadata(&path).expect("metadata");
            let name = slash_path(path.strip_prefix(root).expect("inside root"));
            let kind = if meta.file_type().is_symlink() {
                EntryKind::Symlink(fs::read_link(&path).expect("symlink target"))
            } else if meta.is_dir() {
                if item.file_name() == OsStr::new(".git") {
                    continue;
                }
                pending.push(path.clone());
                EntryKind::Dir
            } else {
                EntryKind::File(
                    fs::read(&path)
                        .unwrap_or_else(|error| panic!("read {}: {error}", path.display())),
                )
            };
            entries.insert(
                name,
                Entry {
                    kind,
                    mode: permission_bits(&meta),
                },
            );
        }
    }
    entries
}

fn permission_bits(meta: &fs::Metadata) -> u32 {
    #[cfg(unix)]
    {
        std::os::unix::fs::PermissionsExt::mode(&meta.permissions()) & 0o7777
    }
    #[cfg(not(unix))]
    {
        let _ = meta;
        0
    }
}

fn slash_path(path: &Path) -> String {
    path.components()
        .map(|part| part.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

/// The environment both implementations get, built as the Go harness builds
/// it: the test's environment without `LUUNA_MISSIONS_*`, `GIT_DIR`,
/// `GIT_WORK_TREE`, `GIT_INDEX_FILE` and `PATH`, then `PATH` (the test's own,
/// or `path` when given), `NO_COLOR=1`, `FORCE_COLOR=0` and `CI=1`. Variables
/// that are not UTF-8 are dropped for both implementations alike.
pub fn child_env(path: Option<String>) -> BTreeMap<String, String> {
    let mut env = BTreeMap::new();
    for (key, value) in std::env::vars_os() {
        let (Some(key), Some(value)) = (key.to_str(), value.to_str()) else {
            continue;
        };
        if key.starts_with("LUUNA_MISSIONS_")
            || matches!(key, "GIT_DIR" | "GIT_WORK_TREE" | "GIT_INDEX_FILE" | "PATH")
        {
            continue;
        }
        env.insert(key.to_owned(), value.to_owned());
    }
    let path = path.unwrap_or_else(|| std::env::var("PATH").unwrap_or_default());
    env.insert("PATH".to_owned(), path);
    env.insert("NO_COLOR".to_owned(), "1".to_owned());
    env.insert("FORCE_COLOR".to_owned(), "0".to_owned());
    env.insert("CI".to_owned(), "1".to_owned());
    env
}

/// What one command printed and how it exited.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Run {
    /// Bytes on stdout.
    pub stdout: Vec<u8>,
    /// Bytes on stderr.
    pub stderr: Vec<u8>,
    /// Exit code; -1 when killed by a signal.
    pub exit: i32,
}

/// A running Go command.
pub struct GoProcess {
    child: Child,
    started: Instant,
    stdout: JoinHandle<Vec<u8>>,
    stderr: JoinHandle<Vec<u8>>,
}

impl GoProcess {
    /// Whether the process has exited.
    pub fn has_exited(&mut self) -> bool {
        self.child.try_wait().expect("poll Go process").is_some()
    }

    /// Waits until the process exits, at most `timeout` from its start; on
    /// timeout kills it and returns an error.
    pub fn finish(mut self, timeout: Duration) -> Result<Run, String> {
        let deadline = self.started + timeout;
        let status = loop {
            if let Some(status) = self.child.try_wait().expect("poll Go process") {
                break status;
            }
            if Instant::now() >= deadline {
                let _ = self.child.kill();
                let _ = self.child.wait();
                return Err(format!("the Go tool did not finish within {timeout:?}"));
            }
            thread::sleep(Duration::from_millis(5));
        };
        Ok(Run {
            stdout: self.stdout.join().expect("stdout reader"),
            stderr: self.stderr.join().expect("stderr reader"),
            exit: status.code().unwrap_or(-1),
        })
    }
}

/// Starts the Go binary `bin` with `args` in `cwd`, with exactly `env` and
/// `stdin`.
pub fn spawn_go(
    bin: &Path,
    cwd: &Path,
    args: &[String],
    stdin: &[u8],
    env: &BTreeMap<String, String>,
) -> GoProcess {
    let mut child = Command::new(bin)
        .args(args)
        .current_dir(cwd)
        .env_clear()
        .envs(env)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap_or_else(|error| panic!("start {}: {error}", bin.display()));
    let started = Instant::now();
    let mut input = child.stdin.take().expect("stdin pipe");
    let stdin = stdin.to_vec();
    thread::spawn(move || {
        let _ = input.write_all(&stdin);
    });
    let mut out = child.stdout.take().expect("stdout pipe");
    let stdout = thread::spawn(move || {
        let mut buffer = Vec::new();
        let _ = out.read_to_end(&mut buffer);
        buffer
    });
    let mut err = child.stderr.take().expect("stderr pipe");
    let stderr = thread::spawn(move || {
        let mut buffer = Vec::new();
        let _ = err.read_to_end(&mut buffer);
        buffer
    });
    GoProcess {
        child,
        started,
        stdout,
        stderr,
    }
}

/// Runs the Go binary to completion (see [`spawn_go`]).
pub fn run_go(
    bin: &Path,
    cwd: &Path,
    args: &[String],
    stdin: &[u8],
    env: &BTreeMap<String, String>,
) -> Result<Run, String> {
    spawn_go(bin, cwd, args, stdin, env).finish(COMMAND_TIMEOUT)
}

/// Starts the Rust implementation on another thread.
pub fn spawn_rust(
    cwd: &Path,
    args: &[String],
    stdin: &[u8],
    env: &BTreeMap<String, String>,
) -> mpsc::Receiver<Result<Run, String>> {
    let invocation = luuna_missions::command::Invocation::new(cwd, args.iter().cloned())
        .with_stdin(stdin.to_vec())
        .with_env(env.clone());
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let result = panic::catch_unwind(AssertUnwindSafe(|| {
            luuna_missions::command::run(&invocation)
        }));
        let result = match result {
            Ok(Ok(outcome)) => Ok(Run {
                stdout: outcome.stdout,
                stderr: outcome.stderr,
                exit: outcome.exit_code,
            }),
            Ok(Err(error)) => Err(format!(
                "luuna_missions::command::run returned Err: {error}"
            )),
            Err(_) => Err("luuna_missions::command::run panicked".to_owned()),
        };
        let _ = sender.send(result);
    });
    receiver
}

/// Waits for a Rust run started with [`spawn_rust`], at most `timeout`.
pub fn finish_rust(
    receiver: &mpsc::Receiver<Result<Run, String>>,
    timeout: Duration,
) -> Result<Run, String> {
    match receiver.recv_timeout(timeout) {
        Ok(result) => result,
        Err(_) => Err(format!(
            "the Rust implementation did not finish within {timeout:?}"
        )),
    }
}

/// Runs the Rust implementation (`luuna_missions::command::run`) to
/// completion. An `Err` from the library is a failure of the comparison.
pub fn run_rust(
    cwd: &Path,
    args: &[String],
    stdin: &[u8],
    env: &BTreeMap<String, String>,
) -> Result<Run, String> {
    finish_rust(&spawn_rust(cwd, args, stdin, env), COMMAND_TIMEOUT)
}

/// Converts string slices to owned arguments.
pub fn args(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

/// Replaces every occurrence of `from` with `to`. An empty `from` changes
/// nothing.
pub fn replace_bytes(haystack: &[u8], from: &[u8], to: &[u8]) -> Vec<u8> {
    if from.is_empty() || haystack.len() < from.len() {
        return haystack.to_vec();
    }
    let mut out = Vec::with_capacity(haystack.len());
    let mut index = 0;
    while index < haystack.len() {
        if haystack[index..].starts_with(from) {
            out.extend_from_slice(to);
            index += from.len();
        } else {
            out.push(haystack[index]);
            index += 1;
        }
    }
    out
}

fn is_word_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

/// Replaces every match of the Go harness's
/// `\b\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3}Z\b` with `<ISO>`.
pub fn mask_iso(input: &[u8]) -> Vec<u8> {
    const SHAPE: &[u8] = b"dddd-dd-ddTdd:dd:dd.dddZ";
    let matches_at = |start: usize| -> bool {
        if start + SHAPE.len() > input.len() {
            return false;
        }
        if start > 0 && is_word_byte(input[start - 1]) {
            return false;
        }
        let end = start + SHAPE.len();
        if end < input.len() && is_word_byte(input[end]) {
            return false;
        }
        SHAPE
            .iter()
            .zip(&input[start..end])
            .all(|(shape, byte)| match shape {
                b'd' => byte.is_ascii_digit(),
                other => other == byte,
            })
    };
    let mut out = Vec::with_capacity(input.len());
    let mut index = 0;
    while index < input.len() {
        if matches_at(index) {
            out.extend_from_slice(b"<ISO>");
            index += SHAPE.len();
        } else {
            out.push(input[index]);
            index += 1;
        }
    }
    out
}

/// The Go harness's normalisation (`norm` in compatibility_test.go): CRLF to
/// LF, then each replacement in order, then ISO timestamps.
#[derive(Debug, Clone, Default)]
pub struct Normaliser {
    replacements: Vec<(Vec<u8>, Vec<u8>)>,
}

impl Normaliser {
    /// A normaliser with no replacements (CRLF and ISO only).
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a replacement, applied after the earlier ones.
    pub fn replace(mut self, from: impl AsRef<[u8]>, to: impl AsRef<[u8]>) -> Self {
        self.replacements
            .push((from.as_ref().to_vec(), to.as_ref().to_vec()));
        self
    }

    /// Replaces the project roots as the Go harness does: the fixture root,
    /// the actual root, its slash form and its doubled-backslash form, the
    /// fixture root's doubled-backslash and slash forms.
    pub fn roots(self, fixture_root: &str, root: &Path) -> Self {
        let root = root.to_string_lossy().into_owned();
        let forms = [
            fixture_root.to_owned(),
            root.clone(),
            root.replace('\\', "/"),
            root.replace('\\', "\\\\"),
            fixture_root.replace('\\', "\\\\"),
            fixture_root.replace('\\', "/"),
        ];
        forms.into_iter().fold(self, |normaliser, form| {
            normaliser.replace(form, "<PROJECT>")
        })
    }

    /// Replaces ids by `<label>`. `ids` holds (label, id written in the
    /// output, id as captured in the fixture); they are applied longest
    /// captured id first, as the Go harness sorts them.
    pub fn ids(self, mut ids: Vec<(String, String, String)>) -> Self {
        ids.sort_by(|a, b| b.2.len().cmp(&a.2.len()).then_with(|| a.0.cmp(&b.0)));
        ids.into_iter()
            .fold(self, |normaliser, (label, actual, _)| {
                normaliser.replace(actual, format!("<{label}>"))
            })
    }

    /// Normalises `input`.
    pub fn apply(&self, input: &[u8]) -> Vec<u8> {
        let mut out = replace_bytes(input, b"\r\n", b"\n");
        for (from, to) in &self.replacements {
            out = replace_bytes(&out, from, to);
        }
        mask_iso(&out)
    }
}

/// Parses `bytes` as one JSON document, as Go's `json.Unmarshal` into `any`
/// accepts it (surrounding whitespace allowed).
pub fn parse_json(bytes: &[u8]) -> Option<Value> {
    serde_json::from_slice(bytes).ok()
}

/// Go's `reflect.DeepEqual` on values decoded by `json.Unmarshal` into
/// `any`: objects compare by key set, numbers as float64.
pub fn json_equal(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => x.as_f64() == y.as_f64(),
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(x, y)| json_equal(x, y))
        }
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len()
                && x.iter()
                    .all(|(key, value)| y.get(key).is_some_and(|other| json_equal(value, other)))
        }
        _ => a == b,
    }
}

/// The Go harness's `portablePaths`: in strings that start with `<PROJECT>`,
/// backslashes become slashes.
pub fn portable_paths(value: &mut Value) {
    match value {
        Value::String(text) if text.starts_with("<PROJECT>") => {
            *text = text.replace('\\', "/");
        }
        Value::Array(items) => items.iter_mut().for_each(portable_paths),
        Value::Object(map) => map.values_mut().for_each(portable_paths),
        _ => {}
    }
}

/// The Go harness's `bind`: wherever the expected JSON holds a captured id,
/// the actual string at the same position becomes that id's binding.
pub fn bind(expected: &Value, actual: &Value, bindings: &mut BTreeMap<String, String>) {
    match (expected, actual) {
        (Value::String(id), _) => {
            if let (Some(slot), Value::String(found)) = (bindings.get_mut(id), actual) {
                found.clone_into(slot);
            }
        }
        (Value::Object(expected), Value::Object(actual)) => {
            for (key, value) in expected {
                bind(value, actual.get(key).unwrap_or(&Value::Null), bindings);
            }
        }
        (Value::Array(expected), Value::Array(actual)) => {
            for (value, other) in expected.iter().zip(actual) {
                bind(value, other, bindings);
            }
        }
        _ => {}
    }
}

/// Go's `fmt.Sprint` of a decoded issue id, for the `list sort` case.
pub fn sprint(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(text)) => text.clone(),
        Some(other) => other.to_string(),
        None => "<nil>".to_owned(),
    }
}

/// The Go harness's `sortIssues`.
pub fn sort_issues(value: &mut Value) {
    if let Some(Value::Array(issues)) = value.get_mut("issues") {
        issues.sort_by_key(|issue| sprint(issue.get("id")));
    }
}

/// Text for a difference report, cut at 1500 characters as the Go harness
/// prints it.
pub fn excerpt(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let mut cut: String = text.chars().take(1500).collect();
    if text.chars().count() > 1500 {
        cut.push_str("...");
    }
    cut
}

/// Compares two byte strings exactly; on a difference returns a description
/// with the first differing offset.
pub fn exact_difference(label: &str, want: &[u8], got: &[u8]) -> Option<String> {
    if want == got {
        return None;
    }
    let offset = want
        .iter()
        .zip(got)
        .position(|(a, b)| a != b)
        .unwrap_or_else(|| want.len().min(got.len()));
    Some(format!(
        "{label} differs at byte {offset}\nwant: {}\n got: {}",
        excerpt(want),
        excerpt(got)
    ))
}

/// Compares two normalised project trees exactly: the same entries, kinds,
/// permission bits and normalised bytes.
pub fn tree_differences(
    want: &BTreeMap<String, Entry>,
    want_norm: &Normaliser,
    got: &BTreeMap<String, Entry>,
    got_norm: &Normaliser,
) -> Vec<String> {
    let mut differences = Vec::new();
    for name in want.keys().filter(|name| !got.contains_key(*name)) {
        differences.push(format!("missing {name}"));
    }
    for name in got.keys().filter(|name| !want.contains_key(*name)) {
        differences.push(format!("unexpected {name}"));
    }
    for (name, expected) in want {
        let Some(actual) = got.get(name) else {
            continue;
        };
        if expected.mode != actual.mode {
            differences.push(format!(
                "{name} mode {:o}, want {:o}",
                actual.mode, expected.mode
            ));
        }
        match (&expected.kind, &actual.kind) {
            (EntryKind::Dir, EntryKind::Dir) => {}
            (EntryKind::File(a), EntryKind::File(b)) => {
                if let Some(difference) =
                    exact_difference(name, &want_norm.apply(a), &got_norm.apply(b))
                {
                    differences.push(difference);
                }
            }
            (EntryKind::Symlink(a), EntryKind::Symlink(b)) if a == b => {}
            (a, b) => differences.push(format!("{name} is {b:?}, want {a:?}")),
        }
    }
    differences
}

/// Compares two runs exactly after normalising each: exit code, stdout and
/// stderr.
pub fn run_differences(
    want: &Run,
    want_norm: &Normaliser,
    got: &Run,
    got_norm: &Normaliser,
) -> Vec<String> {
    let mut differences = Vec::new();
    if want.exit != got.exit {
        differences.push(format!("exit {}, want {}", got.exit, want.exit));
    }
    for (label, a, b) in [
        ("stdout", &want.stdout, &got.stdout),
        ("stderr", &want.stderr, &got.stderr),
    ] {
        if let Some(difference) = exact_difference(label, &want_norm.apply(a), &got_norm.apply(b)) {
            differences.push(difference);
        }
    }
    differences
}

/// Runs `body`, then checks that the reference clone and the luuna worktree
/// are unchanged, writes the report and fails if `body` panicked, the clone
/// or worktree changed, or the report lists any failure.
pub fn guarded<F>(test: &str, clone: &reference::Reference, body: F)
where
    F: FnOnce(&mut Report),
{
    let guard = reference::Guard::capture(clone);
    let mut report = Report::new(test, clone);
    let outcome = panic::catch_unwind(AssertUnwindSafe(|| body(&mut report)));
    if let Err(change) = guard.verify() {
        report.fail(change);
    }
    if let Err(payload) = &outcome {
        let message = payload
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| {
                payload
                    .downcast_ref::<&str>()
                    .map(|text| (*text).to_owned())
            })
            .unwrap_or_else(|| "panic".to_owned());
        report.fail(format!("panicked: {message}"));
    }
    let path = report.write();
    assert!(
        report.failures.is_empty(),
        "{test} failed ({} failures; report {}):\n{}",
        report.failures.len(),
        path.display(),
        report.failures.join("\n---\n")
    );
}

/// The JSON report written to `target/tmp/missions-parity/report-<test>.json`.
pub struct Report {
    test: String,
    header: serde_json::Map<String, Value>,
    cases: Vec<Value>,
    failures: Vec<String>,
}

impl Report {
    fn new(test: &str, clone: &reference::Reference) -> Self {
        let mut header = serde_json::Map::new();
        header.insert("test".to_owned(), Value::from(test));
        header.insert(
            "reference_commit".to_owned(),
            Value::from(reference::REFERENCE_COMMIT),
        );
        header.insert(
            "reference".to_owned(),
            Value::from(clone.path().to_string_lossy().into_owned()),
        );
        Self {
            test: test.to_owned(),
            header,
            cases: Vec::new(),
            failures: Vec::new(),
        }
    }

    /// Records the Go toolchain used (go version, GOPROXY, GOMODCACHE, ...).
    pub fn record_go(&mut self, go: &reference::GoTool) {
        self.header.insert("go".to_owned(), go.describe());
    }

    /// Adds a top-level field.
    pub fn set(&mut self, key: &str, value: Value) {
        self.header.insert(key.to_owned(), value);
    }

    /// Records one case and its differences; a case with differences is a
    /// failure.
    pub fn case(
        &mut self,
        case: &str,
        mut detail: serde_json::Map<String, Value>,
        differences: Vec<String>,
    ) {
        detail.insert("case".to_owned(), Value::from(case));
        detail.insert("passed".to_owned(), Value::from(differences.is_empty()));
        detail.insert("differences".to_owned(), Value::from(differences.clone()));
        self.cases.push(Value::Object(detail));
        for difference in differences {
            self.failures.push(format!("{case}: {difference}"));
        }
    }

    /// Records a failure outside any case.
    pub fn fail(&mut self, failure: impl Into<String>) {
        self.failures.push(failure.into());
    }

    fn write(&self) -> PathBuf {
        let dir = parity_dir();
        fs::create_dir_all(&dir).expect("create the parity folder");
        let path = dir.join(format!("report-{}.json", self.test));
        let mut document = self.header.clone();
        document.insert("cases".to_owned(), Value::from(self.cases.clone()));
        document.insert("failures".to_owned(), Value::from(self.failures.clone()));
        document.insert("passed".to_owned(), Value::from(self.failures.is_empty()));
        let text = serde_json::to_string_pretty(&Value::Object(document)).expect("report JSON");
        fs::write(&path, text + "\n").expect("write the report");
        path
    }
}

/// Replays every fixture of the in-scope group `test` (see
/// [`fixtures::IN_SCOPE`]) three ways: Go against the fixture and Rust
/// against the fixture, both with the Go harness's rules, and Rust against
/// Go byte for byte after normalisation.
pub fn replay_group(test: &str) {
    let clone = reference::reference_from_env();
    guarded(test, &clone, |report| {
        let go = reference::go_tool(&clone);
        report.record_go(go);
        let group = fixtures::group(test);
        report.set(
            "fixtures",
            Value::from(
                group
                    .files
                    .iter()
                    .map(|file| (*file).to_owned())
                    .collect::<Vec<_>>(),
            ),
        );
        for file in group.files {
            replay_fixture(&clone, go, file, report);
        }
    });
}

/// A project prepared from a fixture's `before` tree.
struct FixtureProject {
    project: TempProject,
    env: BTreeMap<String, String>,
    stdin: Vec<u8>,
}

impl FixtureProject {
    fn new(clone: &reference::Reference, fixture: &fixtures::Fixture) -> Self {
        let project = TempProject::new();
        let root = project.root().to_string_lossy().into_owned();
        let replace_root = |text: &str| -> Vec<u8> {
            let text = text.replace(
                &fixture.root.replace('\\', "\\\\"),
                &root.replace('\\', "\\\\"),
            );
            let text = text.replace(&fixture.root.replace('\\', "/"), &root.replace('\\', "/"));
            text.replace(&fixture.root, &root).into_bytes()
        };
        project.write_files(
            fixture
                .before
                .iter()
                .map(|(name, content)| (name.as_str(), replace_root(content))),
        );
        let path = match fixture.env.get("PATH").filter(|value| !value.is_empty()) {
            None => None,
            Some(value) if value.contains("no-atlas-bin") => Some(
                project
                    .empty_dir("no-atlas-bin")
                    .to_string_lossy()
                    .into_owned(),
            ),
            Some(_) => {
                let mut dirs = vec![reference::atlas_dir(clone).to_path_buf()];
                dirs.extend(std::env::split_paths(
                    &std::env::var_os("PATH").unwrap_or_default(),
                ));
                Some(
                    std::env::join_paths(dirs)
                        .expect("PATH")
                        .to_string_lossy()
                        .into_owned(),
                )
            }
        };
        let stdin = replace_root(&fixture.input);
        Self {
            project,
            env: child_env(path),
            stdin,
        }
    }
}

fn replay_fixture(
    clone: &reference::Reference,
    go: &reference::GoTool,
    file: &str,
    report: &mut Report,
) {
    let fixture = fixtures::load(clone, file);
    let case = format!("{file}/{}", fixture.name);
    let go_project = FixtureProject::new(clone, &fixture);
    let go_run = run_go(
        go.binary(),
        go_project.project.root(),
        &fixture.args,
        &go_project.stdin,
        &go_project.env,
    );
    let rust_project = FixtureProject::new(clone, &fixture);
    let rust_run = run_rust(
        rust_project.project.root(),
        &fixture.args,
        &rust_project.stdin,
        &rust_project.env,
    );

    let mut detail = serde_json::Map::new();
    detail.insert("args".to_owned(), Value::from(fixture.args.clone()));
    let mut differences = Vec::new();
    let mut checked = |side: &str, run: &Result<Run, String>, project: &TempProject| match run {
        Ok(run) => {
            let (found, normaliser) =
                fixture_differences(&fixture, project.root(), run, &project.snapshot());
            detail.insert(format!("{side}_exit"), Value::from(run.exit));
            detail.insert(format!("{side}_vs_fixture"), Value::from(found.clone()));
            differences.extend(
                found
                    .into_iter()
                    .map(|found| format!("{side} vs fixture: {found}")),
            );
            Some(normaliser)
        }
        Err(error) => {
            detail.insert(format!("{side}_error"), Value::from(error.clone()));
            differences.push(format!("{side}: {error}"));
            None
        }
    };
    let go_norm = checked("go", &go_run, &go_project.project);
    let rust_norm = checked("rust", &rust_run, &rust_project.project);
    if let (Ok(go_run), Ok(rust_run), Some(go_norm), Some(rust_norm)) =
        (&go_run, &rust_run, &go_norm, &rust_norm)
    {
        let mut found = run_differences(go_run, go_norm, rust_run, rust_norm);
        found.extend(tree_differences(
            &go_project.project.snapshot(),
            go_norm,
            &rust_project.project.snapshot(),
            rust_norm,
        ));
        detail.insert("rust_vs_go".to_owned(), Value::from(found.clone()));
        differences.extend(
            found
                .into_iter()
                .map(|found| format!("rust vs go: {found}")),
        );
    }
    report.case(&case, detail, differences);
}

/// Go's platform spelling for the `version json` case.
fn go_platform() -> String {
    let os = match std::env::consts::OS {
        "windows" => "win32",
        "macos" => "darwin",
        other => other,
    };
    let arch = match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        "x86" => "386",
        other => other,
    };
    format!("{os}-{arch}")
}

/// Compares one run with its fixture by the Go harness's rules
/// (`TestCompatibility`) and returns the differences and the normaliser for
/// the actual output (with the ids it bound).
fn fixture_differences(
    fixture: &fixtures::Fixture,
    root: &Path,
    run: &Run,
    tree: &BTreeMap<String, Entry>,
) -> (Vec<String>, Normaliser) {
    let mut differences = Vec::new();
    if run.exit != fixture.result.exit {
        differences.push(format!(
            "exit {}, want {}: {}",
            run.exit,
            fixture.result.exit,
            excerpt(&run.stderr)
        ));
    }
    let mut bindings: BTreeMap<String, String> = fixture
        .ids
        .values()
        .map(|id| (id.clone(), id.clone()))
        .collect();
    let expected_json = parse_json(fixture.result.stdout.as_bytes()).unwrap_or(Value::Null);
    let actual_json = parse_json(&run.stdout).unwrap_or(Value::Null);
    bind(&expected_json, &actual_json, &mut bindings);
    let mut expected_stdout = fixture.result.stdout.clone();
    if fixture.name == "version json" {
        match expected_json.clone() {
            Value::Object(mut expected) => {
                expected.insert("platform".to_owned(), Value::from(go_platform()));
                expected_stdout = Value::Object(expected).to_string();
            }
            _ => differences.push("version json: the fixture stdout is not an object".to_owned()),
        }
    }
    let list_sort = fixture.name == "list sort";
    if list_sort {
        match actual_json.get("issues").and_then(Value::as_array) {
            Some(issues) => {
                if issues
                    .windows(2)
                    .any(|pair| sprint(pair[0].get("id")) > sprint(pair[1].get("id")))
                {
                    differences.push("list --sort id returned unsorted IDs".to_owned());
                }
            }
            None => differences.push("list sort: stdout has no issues array".to_owned()),
        }
    }
    let ids = |actual: bool| -> Vec<(String, String, String)> {
        fixture
            .ids
            .iter()
            .map(|(label, id)| {
                let written = if actual {
                    bindings[id].clone()
                } else {
                    id.clone()
                };
                (label.clone(), written, id.clone())
            })
            .collect()
    };
    let want_norm = Normaliser::new().roots(&fixture.root, root).ids(ids(false));
    let got_norm = Normaliser::new().roots(&fixture.root, root).ids(ids(true));
    let compare = |label: &str, want: &[u8], got: &[u8]| -> Option<String> {
        let want = want_norm.apply(want);
        let got = got_norm.apply(got);
        if let (Some(mut a), Some(mut b)) = (parse_json(&want), parse_json(&got)) {
            if list_sort && label == "stdout" {
                sort_issues(&mut a);
                sort_issues(&mut b);
            }
            portable_paths(&mut a);
            portable_paths(&mut b);
            if json_equal(&a, &b) {
                return None;
            }
        } else if want == got {
            return None;
        }
        Some(format!(
            "{label} differs\nwant: {}\n got: {}",
            excerpt(&want),
            excerpt(&got)
        ))
    };
    differences.extend(compare("stdout", expected_stdout.as_bytes(), &run.stdout));
    differences.extend(compare(
        "stderr",
        fixture.result.stderr.as_bytes(),
        &run.stderr,
    ));
    for (name, entry) in tree {
        let data = match &entry.kind {
            EntryKind::Dir => continue,
            EntryKind::File(data) => data.clone(),
            EntryKind::Symlink(_) => fs::read(root.join(name)).unwrap_or_default(),
        };
        let Some(want) = fixture.after.get(name) else {
            differences.push(format!("unexpected file {name}"));
            continue;
        };
        if name.ends_with(".jsonl") {
            let got = String::from_utf8_lossy(&data);
            let a: Vec<&str> = want.trim().split('\n').collect();
            let b: Vec<&str> = got.trim().split('\n').collect();
            if a.len() != b.len() {
                differences.push(format!("{name} record count: {} want {}", b.len(), a.len()));
                continue;
            }
            for (index, (a, b)) in a.iter().zip(&b).enumerate() {
                differences.extend(compare(
                    &format!("{name}:{}", index + 1),
                    a.as_bytes(),
                    b.as_bytes(),
                ));
            }
        } else {
            differences.extend(compare(name, want.as_bytes(), &data));
        }
    }
    for name in fixture.after.keys() {
        if !tree
            .get(name)
            .is_some_and(|entry| entry.kind != EntryKind::Dir)
        {
            differences.push(format!("missing file {name}"));
        }
    }
    (differences, got_norm)
}
