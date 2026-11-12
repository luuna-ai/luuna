//! Temporary projects in which the Go tool and the crate run, and the comparison of what
//! they print and leave on disk (plan s5-atlas-lib, "Harness" and "Normalisation").
//!
//! Every project lives in a fresh temporary directory created by the test. Git runs only
//! there: `GIT_CEILING_DIRECTORIES` stops it from finding any repository above the
//! sandbox, so neither the Go tool nor the harness ever reaches the reference clone or a
//! workspace repository.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::SystemTime;

use luuna_atlas::{commands, CommandOutput};

use super::normalize::Normalizer;
use super::reference::{null_device, reference};

/// Fixed identity and dates for every git call in a test project.
const GIT_IDENTITY: &[(&str, &str)] = &[
    ("GIT_AUTHOR_NAME", "Atlas Parity"),
    ("GIT_AUTHOR_EMAIL", "atlas-parity@example.invalid"),
    ("GIT_AUTHOR_DATE", "2026-01-01T00:00:00Z"),
    ("GIT_COMMITTER_NAME", "Atlas Parity"),
    ("GIT_COMMITTER_EMAIL", "atlas-parity@example.invalid"),
    ("GIT_COMMITTER_DATE", "2026-01-01T00:00:00Z"),
];

/// A temporary directory holding one or two projects and their shared fake home.
pub struct Sandbox {
    _temp: tempfile::TempDir,
    root: PathBuf,
    home: PathBuf,
    started: SystemTime,
    binary: Option<PathBuf>,
}

impl Sandbox {
    /// A sandbox whose projects can run the Go tool. Checks and builds the reference
    /// first, so a bad reference fails here with the guard's message.
    pub fn new() -> Sandbox {
        let binary = reference().binary.clone();
        Sandbox::create(Some(binary))
    }

    /// A sandbox for harness self-tests: host git only, no Go tool.
    pub fn without_go() -> Sandbox {
        Sandbox::create(None)
    }

    fn create(binary: Option<PathBuf>) -> Sandbox {
        let started = SystemTime::now();
        let temp = tempfile::Builder::new()
            .prefix("luuna-atlas-test-")
            .tempdir()
            .expect("create the sandbox directory");
        let root = std::fs::canonicalize(temp.path()).expect("canonicalize the sandbox");
        let home = root.join("home");
        std::fs::create_dir_all(&home).expect("create the sandbox home");
        Sandbox {
            _temp: temp,
            root,
            home,
            started,
            binary,
        }
    }

    /// The sandbox directory (canonical).
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// When the test started, for timestamp normalisation.
    pub fn started(&self) -> SystemTime {
        self.started
    }

    /// Creates `<sandbox>/<name>/project`, with `git init` if `git` is set.
    pub fn project(&self, name: &str, git: bool) -> Project {
        let dir = self.root.join(name).join("project");
        std::fs::create_dir_all(&dir).expect("create the project directory");
        let project = Project {
            dir,
            home: self.home.clone(),
            ceiling: self.root.clone(),
            started: self.started,
            binary: self.binary.clone(),
        };
        if git {
            project.git(&["-c", "init.defaultBranch=main", "init", "-q"]);
        }
        project
    }
}

/// One temporary project.
pub struct Project {
    pub dir: PathBuf,
    home: PathBuf,
    ceiling: PathBuf,
    started: SystemTime,
    binary: Option<PathBuf>,
}

impl Project {
    /// `<project>/.luuna/atlas`.
    pub fn atlas_dir(&self) -> PathBuf {
        self.dir.join(".luuna").join("atlas")
    }

    /// `<project>/.luuna/atlas/config.yaml`.
    pub fn config_path(&self) -> PathBuf {
        self.atlas_dir().join("config.yaml")
    }

    /// `<project>/.luuna/atlas/expertise/<domain>.jsonl`.
    pub fn expertise(&self, domain: &str) -> PathBuf {
        self.atlas_dir()
            .join("expertise")
            .join(format!("{domain}.jsonl"))
    }

    /// `<project>/.luuna/atlas/archive/<domain>.jsonl`.
    pub fn archive(&self, domain: &str) -> PathBuf {
        self.atlas_dir()
            .join("archive")
            .join(format!("{domain}.jsonl"))
    }

    /// The normaliser for this project's output.
    pub fn normalizer(&self) -> Normalizer {
        Normalizer::new(&self.dir, self.started)
    }

    /// Runs host git in the project with the sandboxed environment; panics on failure.
    #[track_caller]
    pub fn git(&self, args: &[&str]) -> String {
        let output = Command::new("git")
            .args(args)
            .current_dir(&self.dir)
            .env_clear()
            .envs(self.run_env())
            .stdin(Stdio::null())
            .output()
            .expect("run git in the test project");
        assert!(
            output.status.success(),
            "git {args:?} failed in {}: {}",
            self.dir.display(),
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    /// The environment of the Go tool and of git in a test project: cleared except `PATH`,
    /// then a fake home, no colour, UTC, the C locale, no system or global git config, a
    /// fixed git identity and dates, and a ceiling at the sandbox.
    pub fn run_env(&self) -> Vec<(OsString, OsString)> {
        let mut env: Vec<(OsString, OsString)> = Vec::new();
        if let Some(path) = std::env::var_os("PATH") {
            env.push(("PATH".into(), path));
        }
        env.push(("HOME".into(), self.home.clone().into()));
        env.push(("NO_COLOR".into(), "1".into()));
        env.push(("TERM".into(), "dumb".into()));
        env.push(("TZ".into(), "UTC".into()));
        env.push(("LC_ALL".into(), "C".into()));
        env.push(("GIT_CONFIG_NOSYSTEM".into(), "1".into()));
        env.push(("GIT_CONFIG_GLOBAL".into(), null_device().into()));
        env.push((
            "GIT_CEILING_DIRECTORIES".into(),
            self.ceiling.clone().into(),
        ));
        for (key, value) in GIT_IDENTITY {
            env.push(((*key).into(), (*value).into()));
        }
        env
    }

    /// Runs the Go tool with `args` and `--json --quiet`, as the runtime adapter does.
    pub fn go(&self, args: &[&str]) -> CommandOutput {
        self.go_with_input(args, None)
    }

    /// Like [`Project::go`], with `input` on stdin.
    pub fn go_with_input(&self, args: &[&str], input: Option<&str>) -> CommandOutput {
        use std::io::Write;
        let binary = self
            .binary
            .as_ref()
            .expect("this sandbox was created without the Go tool");
        let mut child = Command::new(binary)
            .args(args)
            .args(["--json", "--quiet"])
            .current_dir(&self.dir)
            .env_clear()
            .envs(self.run_env())
            .stdin(if input.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("start the Go tool");
        if let Some(input) = input {
            let mut stdin = child.stdin.take().expect("piped stdin");
            stdin
                .write_all(input.as_bytes())
                .expect("write the Go tool's stdin");
        }
        let output = child.wait_with_output().expect("wait for the Go tool");
        CommandOutput {
            exit_code: output.status.code().unwrap_or(-1),
            stdout: String::from_utf8(output.stdout).expect("Go stdout is UTF-8"),
            stderr: String::from_utf8(output.stderr).expect("Go stderr is UTF-8"),
        }
    }

    /// Runs the Go tool and requires exit code 0; returns stdout.
    #[track_caller]
    pub fn go_ok(&self, args: &[&str]) -> String {
        let output = self.go(args);
        assert_eq!(
            output.exit_code,
            0,
            "Go tool {args:?} failed in {}:\nstdout:\n{}\nstderr:\n{}",
            self.dir.display(),
            output.stdout,
            output.stderr
        );
        output.stdout
    }

    /// Like [`Project::go_ok`], with `input` on stdin.
    #[track_caller]
    pub fn go_ok_with_input(&self, args: &[&str], input: &str) -> String {
        let output = self.go_with_input(args, Some(input));
        assert_eq!(
            output.exit_code,
            0,
            "Go tool {args:?} failed in {}:\nstdout:\n{}\nstderr:\n{}",
            self.dir.display(),
            output.stdout,
            output.stderr
        );
        output.stdout
    }

    /// Runs `step` through the crate.
    pub fn rust(&self, step: &Step) -> CommandOutput {
        match step {
            Step::Init => commands::run_init(&self.dir),
            Step::Add(domain) => commands::run_add(&self.dir, domain),
            Step::ConfigShow => commands::run_config_show(&self.dir),
        }
    }

    /// Runs `step` through the Go tool.
    pub fn go_step(&self, step: &Step) -> CommandOutput {
        let args = step.go_args();
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        self.go(&args)
    }

    /// The project's file tree, `.git` excluded, contents normalised.
    pub fn snapshot(&self) -> Vec<TreeEntry> {
        snapshot(&self.dir, &self.normalizer())
    }

    /// Every leftover `*.lock` or `*.tmp.*` file in the project.
    pub fn leftovers(&self) -> Vec<String> {
        leftovers(&self.dir)
    }

    /// Writes `contents` to `relative` inside the project, creating parent directories.
    pub fn write(&self, relative: &str, contents: &str) {
        let path = self.dir.join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create parent directories");
        }
        std::fs::write(&path, contents).expect("write a fixture file");
    }

    /// Appends `contents` to `path`.
    pub fn append(&self, path: &Path, contents: &str) {
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(path)
            .expect("open a store file for appending");
        file.write_all(contents.as_bytes())
            .expect("append to a store file");
    }

    /// Reads `path` as UTF-8.
    pub fn read(&self, path: &Path) -> String {
        std::fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
    }
}

/// A command run by both sides of a parity test.
#[derive(Debug, Clone)]
pub enum Step {
    Init,
    Add(String),
    ConfigShow,
}

impl Step {
    pub fn add(domain: &str) -> Step {
        Step::Add(domain.to_owned())
    }

    /// The Go tool's arguments, without `--json --quiet` (added by [`Project::go`]).
    pub fn go_args(&self) -> Vec<String> {
        match self {
            Step::Init => vec!["init".into()],
            Step::Add(domain) => vec!["add".into(), domain.clone()],
            Step::ConfigShow => vec!["config".into(), "show".into()],
        }
    }

    pub fn label(&self) -> String {
        self.go_args().join(" ")
    }
}

/// Two projects in one sandbox: the Go tool runs in `go`, the crate in `rust`.
pub struct Twin {
    pub sandbox: Sandbox,
    pub go: Project,
    pub rust: Project,
}

impl Twin {
    /// Twin projects, each with `git init` if `git` is set.
    pub fn new(git: bool) -> Twin {
        let sandbox = Sandbox::new();
        let go = sandbox.project("go", git);
        let rust = sandbox.project("rust", git);
        Twin { sandbox, go, rust }
    }

    /// Applies the same preparation to both projects.
    pub fn prepare(&self, prepare: impl Fn(&Project)) {
        prepare(&self.go);
        prepare(&self.rust);
    }

    /// Runs `step` on both sides, then compares exit codes, stdout, stderr and the trees.
    #[track_caller]
    pub fn step(&self, index: usize, step: &Step) {
        let label = format!("step {index}: {}", step.label());
        let go = self.go.go_step(step);
        let rust = self.rust.rust(step);
        assert_same_output(&label, &self.go, &go, &self.rust, &rust);
        self.assert_same_tree(&label);
    }

    /// Runs every step in order.
    #[track_caller]
    pub fn steps(&self, steps: &[Step]) {
        for (index, step) in steps.iter().enumerate() {
            self.step(index, step);
        }
    }

    /// Compares both projects' trees and fails on any leftover lock or temporary file.
    #[track_caller]
    pub fn assert_same_tree(&self, label: &str) {
        for project in [&self.go, &self.rust] {
            let leftovers = project.leftovers();
            assert!(
                leftovers.is_empty(),
                "{label}: leftover lock or temporary files in {}: {leftovers:?}",
                project.dir.display()
            );
        }
        let go = self.go.snapshot();
        let rust = self.rust.snapshot();
        if go != rust {
            panic!(
                "{label}: file trees differ\n--- Go tool ---\n{}\n--- crate ---\n{}",
                describe_tree(&go),
                describe_tree(&rust)
            );
        }
    }
}

/// Compares two command outputs after normalising each with its own project.
#[track_caller]
pub fn assert_same_output(
    label: &str,
    go_project: &Project,
    go: &CommandOutput,
    rust_project: &Project,
    rust: &CommandOutput,
) {
    let go_norm = go_project.normalizer();
    let rust_norm = rust_project.normalizer();
    let expected = (
        go.exit_code,
        go_norm.normalize(&go.stdout),
        go_norm.normalize(&go.stderr),
    );
    let actual = (
        rust.exit_code,
        rust_norm.normalize(&rust.stdout),
        rust_norm.normalize(&rust.stderr),
    );
    if expected != actual {
        panic!(
            "{label}: output differs\n--- Go tool (normalised) ---\nexit {}\nstdout:\n{}\nstderr:\n{}\n--- crate (normalised) ---\nexit {}\nstdout:\n{}\nstderr:\n{}",
            expected.0, expected.1, expected.2, actual.0, actual.1, actual.2
        );
    }
}

/// What a tree entry is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    Dir,
    File(Contents),
    Symlink(String),
    Other,
}

/// A file's contents: normalised text, or raw bytes if not UTF-8.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Contents {
    Text(String),
    Bytes(Vec<u8>),
}

/// One entry of a project tree: its path relative to the project (with `/`), its kind
/// and contents, and its permission bits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeEntry {
    pub path: String,
    pub kind: Kind,
    pub mode: u32,
}

/// Walks `root`, skipping the top-level `.git`, in sorted path order.
pub fn snapshot(root: &Path, normalizer: &Normalizer) -> Vec<TreeEntry> {
    let mut entries = Vec::new();
    walk(root, root, normalizer, &mut entries);
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    entries
}

fn walk(root: &Path, dir: &Path, normalizer: &Normalizer, entries: &mut Vec<TreeEntry>) {
    let mut children: Vec<_> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("read {}: {e}", dir.display()))
        .map(|entry| entry.expect("read a directory entry").path())
        .collect();
    children.sort();
    for path in children {
        let relative = relative_path(root, &path);
        if relative == ".git" {
            continue;
        }
        let meta = std::fs::symlink_metadata(&path)
            .unwrap_or_else(|e| panic!("stat {}: {e}", path.display()));
        let kind = if meta.file_type().is_symlink() {
            let target = std::fs::read_link(&path).expect("read a symlink");
            Kind::Symlink(normalizer.normalize(&target.to_string_lossy()))
        } else if meta.is_dir() {
            Kind::Dir
        } else if meta.is_file() {
            let bytes =
                std::fs::read(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
            match String::from_utf8(bytes) {
                Ok(text) => Kind::File(Contents::Text(normalizer.normalize(&text))),
                Err(e) => Kind::File(Contents::Bytes(e.into_bytes())),
            }
        } else {
            Kind::Other
        };
        let descend = matches!(kind, Kind::Dir);
        entries.push(TreeEntry {
            path: relative,
            kind,
            mode: permission_bits(&meta),
        });
        if descend {
            walk(root, &path, normalizer, entries);
        }
    }
}

#[cfg(unix)]
fn permission_bits(meta: &std::fs::Metadata) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    meta.permissions().mode() & 0o7777
}

#[cfg(not(unix))]
fn permission_bits(meta: &std::fs::Metadata) -> u32 {
    u32::from(meta.permissions().readonly())
}

fn relative_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .expect("a path inside the project")
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

/// Every `*.lock` or `*.tmp.*` file under `root`, `.git` excluded.
pub fn leftovers(root: &Path) -> Vec<String> {
    snapshot_paths(root)
        .into_iter()
        .filter(|path| {
            let name = path.rsplit('/').next().unwrap_or(path);
            name.ends_with(".lock") || name.contains(".tmp.")
        })
        .collect()
}

fn snapshot_paths(root: &Path) -> Vec<String> {
    let mut paths = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for entry in
            std::fs::read_dir(&dir).unwrap_or_else(|e| panic!("read {}: {e}", dir.display()))
        {
            let path = entry.expect("read a directory entry").path();
            let relative = relative_path(root, &path);
            if relative == ".git" {
                continue;
            }
            let is_dir = std::fs::symlink_metadata(&path)
                .map(|m| m.is_dir())
                .unwrap_or(false);
            if is_dir {
                pending.push(path);
            }
            paths.push(relative);
        }
    }
    paths.sort();
    paths
}

/// A readable listing of a tree for failure messages.
pub fn describe_tree(entries: &[TreeEntry]) -> String {
    let mut out = String::new();
    for entry in entries {
        match &entry.kind {
            Kind::Dir => out.push_str(&format!("{:o} dir  {}\n", entry.mode, entry.path)),
            Kind::Symlink(target) => out.push_str(&format!(
                "{:o} link {} -> {target}\n",
                entry.mode, entry.path
            )),
            Kind::Other => out.push_str(&format!("{:o} other {}\n", entry.mode, entry.path)),
            Kind::File(Contents::Bytes(bytes)) => out.push_str(&format!(
                "{:o} file {} ({} non-UTF-8 bytes)\n",
                entry.mode,
                entry.path,
                bytes.len()
            )),
            Kind::File(Contents::Text(text)) => {
                out.push_str(&format!("{:o} file {}\n", entry.mode, entry.path));
                for line in text.split_inclusive('\n') {
                    out.push_str(&format!("    | {}", line.escape_debug()));
                    out.push('\n');
                }
            }
        }
    }
    out
}

/// Copies `<from>/.luuna` to `<to>/.luuna`, files and directories only.
pub fn copy_store(from: &Project, to: &Project) {
    copy_dir(&from.dir.join(".luuna"), &to.dir.join(".luuna"));
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).expect("create a directory");
    for entry in std::fs::read_dir(from).expect("read a store directory") {
        let entry = entry.expect("read a directory entry");
        let target = to.join(entry.file_name());
        if entry.file_type().expect("file type").is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), &target).expect("copy a store file");
        }
    }
}
