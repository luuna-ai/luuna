//! Builds the Go Beacon tool from the reference clone at the recorded commit
//! and runs it as the oracle.
//!
//! - The clone is found only through `LUUNA_BEACON_REFERENCE`, which must be
//!   an absolute path; there is no default. The Go program is
//!   `LUUNA_BEACON_GO` (default `go` on PATH) and the module source is
//!   `LUUNA_BEACON_GOPROXY` (default [`DEFAULT_GOPROXY`]; no fallback to
//!   version control).
//! - Before building, the clone must be at [`REFERENCE_COMMIT`] with a clean
//!   status, and its status including ignored files must be the same after
//!   the build. Every failure panics; nothing is skipped.
//! - The build runs once per test binary, with a cleared environment
//!   ([`go_build_env`]) that keeps every Go path under the build directory in
//!   Cargo's target folder, so modules are downloaded only there and nothing
//!   is written into the clone or the host's Go folders.
//! - Values of the passed proxy variables are never printed: a proxy URL may
//!   carry credentials.

use serde_json::Value;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::OnceLock;

/// The reference commit every comparison is measured against.
pub const REFERENCE_COMMIT: &str = "1c34973a7fba3d0ee1d2aa697f8e1a03cf57f927";
/// Module source used when `LUUNA_BEACON_GOPROXY` is not set.
pub const DEFAULT_GOPROXY: &str = "https://proxy.golang.org";
/// The proxy variables Go's HTTP client reads; only these are passed through
/// to the build, and only when set.
pub const PROXY_VARS: [&str; 6] = [
    "HTTPS_PROXY",
    "https_proxy",
    "HTTP_PROXY",
    "http_proxy",
    "NO_PROXY",
    "no_proxy",
];

pub const REFERENCE_VAR: &str = "LUUNA_BEACON_REFERENCE";
pub const GO_VAR: &str = "LUUNA_BEACON_GO";
pub const GOPROXY_VAR: &str = "LUUNA_BEACON_GOPROXY";

/// Names of the build-environment entries that are folders under the build
/// directory, with the sub-folder each one uses.
pub const BUILD_DIR_VARS: [(&str, &str); 8] = [
    ("HOME", "home"),
    ("XDG_CONFIG_HOME", "xdg-config"),
    ("XDG_CACHE_HOME", "xdg-cache"),
    ("GOPATH", "gopath"),
    ("GOCACHE", "gocache"),
    ("GOMODCACHE", "gomodcache"),
    ("GOTMPDIR", "tmp"),
    ("TMPDIR", "tmp"),
];

/// Interprets the value of `LUUNA_BEACON_REFERENCE`: unset, empty and
/// relative values are refused; there is no default location.
pub fn reference_from(value: Option<OsString>) -> Result<PathBuf, String> {
    let Some(value) = value else {
        return Err(format!(
            "{REFERENCE_VAR} is not set; set it to the absolute path of a luuna-beacon clone at {REFERENCE_COMMIT}"
        ));
    };
    if value.is_empty() {
        return Err(format!(
            "{REFERENCE_VAR} is empty; set it to the absolute path of a luuna-beacon clone at {REFERENCE_COMMIT}"
        ));
    }
    let path = PathBuf::from(value);
    if !path.is_absolute() {
        return Err(format!(
            "{REFERENCE_VAR} must be an absolute path, not {}",
            path.display()
        ));
    }
    Ok(path)
}

/// [`reference_from`], panicking on a refused value (a hard failure, never a
/// skip).
pub fn require_reference(value: Option<OsString>) -> PathBuf {
    reference_from(value).unwrap_or_else(|error| panic!("{error}"))
}

/// The reference clone named by `LUUNA_BEACON_REFERENCE`; panics if it is
/// unset, empty or relative.
pub fn reference() -> PathBuf {
    require_reference(std::env::var_os(REFERENCE_VAR))
}

/// The Go program: `LUUNA_BEACON_GO`, or `go` looked up on PATH.
pub fn go_program() -> OsString {
    match std::env::var_os(GO_VAR) {
        Some(value) if !value.is_empty() => value,
        _ => OsString::from("go"),
    }
}

/// The module source: `LUUNA_BEACON_GOPROXY`, or [`DEFAULT_GOPROXY`].
pub fn module_source() -> String {
    match std::env::var_os(GOPROXY_VAR) {
        Some(value) if !value.is_empty() => value
            .into_string()
            .unwrap_or_else(|_| panic!("{GOPROXY_VAR} is not valid UTF-8")),
        _ => DEFAULT_GOPROXY.to_owned(),
    }
}

/// The build directory, `<CARGO_TARGET_TMPDIR>/beacon-go`, inside the
/// workspace's ignored target folder.
pub fn build_dir() -> PathBuf {
    Path::new(env!("CARGO_TARGET_TMPDIR")).join("beacon-go")
}

/// Those of [`PROXY_VARS`] for which `get` returns a value, in
/// [`PROXY_VARS`] order, with their values unchanged.
pub fn proxy_pass_through(get: impl Fn(&str) -> Option<OsString>) -> Vec<(String, OsString)> {
    PROXY_VARS
        .iter()
        .filter_map(|name| get(name).map(|value| ((*name).to_owned(), value)))
        .collect()
}

/// The complete environment of the Go build: PATH as given, every Go and
/// home folder under `build_dir`, the fixed Go settings, then the given proxy
/// entries unchanged. Nothing else is passed, so GOSUMDB keeps Go's default.
pub fn go_build_env(
    build_dir: &Path,
    goproxy: &str,
    path_var: &OsStr,
    proxy: &[(String, OsString)],
) -> Vec<(String, OsString)> {
    let mut env = vec![("PATH".to_owned(), path_var.to_owned())];
    for (name, folder) in BUILD_DIR_VARS {
        env.push((name.to_owned(), build_dir.join(folder).into_os_string()));
    }
    for (name, value) in [
        ("GOTOOLCHAIN", "local"),
        ("GOFLAGS", "-mod=readonly -modcacherw"),
        ("GOPROXY", goproxy),
        ("GOWORK", "off"),
        ("GOENV", "off"),
        ("CGO_ENABLED", "0"),
    ] {
        env.push((name.to_owned(), OsString::from(value)));
    }
    env.extend(proxy.iter().cloned());
    env
}

/// Runs `git --no-optional-locks -C <clone> <args>`, read-only. Inherited
/// `GIT_*` variables are removed so that they cannot point git at another
/// repository or index.
pub fn git(clone: &Path, args: &[&str]) -> Output {
    let mut command = Command::new("git");
    for (name, _) in std::env::vars_os() {
        if name.to_string_lossy().starts_with("GIT_") {
            command.env_remove(name);
        }
    }
    command
        .arg("--no-optional-locks")
        .arg("-C")
        .arg(clone)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .unwrap_or_else(|error| panic!("could not start git: {error}"))
}

fn git_text(clone: &Path, args: &[&str]) -> String {
    let output = git(clone, args);
    assert!(
        output.status.success(),
        "git {} in the reference clone {} failed ({}): {}",
        args.join(" "),
        clone.display(),
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("git output is UTF-8")
}

/// `git status --porcelain --ignored` of the clone, as printed.
pub fn status_with_ignored(clone: &Path) -> String {
    git_text(clone, &["status", "--porcelain", "--ignored"])
}

/// Panics unless the clone's HEAD is [`REFERENCE_COMMIT`] and its status is
/// clean.
pub fn check_reference(clone: &Path) {
    let head = git_text(clone, &["rev-parse", "HEAD"]);
    assert_eq!(
        head.trim(),
        REFERENCE_COMMIT,
        "the reference clone {} is not at the recorded commit",
        clone.display()
    );
    let status = git_text(clone, &["status", "--porcelain"]);
    assert!(
        status.is_empty(),
        "the reference clone {} is not clean:\n{status}",
        clone.display()
    );
}

/// The result of the one Go build of this test binary.
#[derive(Debug)]
pub struct Build {
    /// The canonical reference clone.
    pub clone: PathBuf,
    /// The canonical build directory.
    pub dir: PathBuf,
    /// The built `luuna-beacon` binary.
    pub binary: PathBuf,
    /// The clone's `git status --porcelain --ignored` before the build.
    pub status_before: String,
}

static BUILD: OnceLock<Build> = OnceLock::new();

/// Builds the Go tool once per test binary and returns the build; panics if
/// the clone, Go or the build is missing or wrong.
pub fn go_build() -> &'static Build {
    BUILD.get_or_init(build)
}

fn build() -> Build {
    let reference = reference();
    let clone = reference.canonicalize().unwrap_or_else(|error| {
        panic!(
            "the reference clone {} cannot be opened: {error}",
            reference.display()
        )
    });
    check_reference(&clone);

    // Checked on the existing target folder before anything is created, and
    // again on the build directory itself (it could be a link) before any
    // sub-folder is created, so nothing is ever created inside the clone.
    let target_tmp = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .canonicalize()
        .expect("Cargo's target tmp folder exists");
    assert_outside_clone(&target_tmp, &clone);
    let dir = target_tmp.join("beacon-go");
    std::fs::create_dir_all(&dir)
        .unwrap_or_else(|error| panic!("cannot create {}: {error}", dir.display()));
    let dir = dir.canonicalize().expect("the build directory exists");
    assert_outside_clone(&dir, &clone);
    for folder in BUILD_DIR_VARS
        .iter()
        .map(|(_, folder)| *folder)
        .chain(["bin"])
    {
        std::fs::create_dir_all(dir.join(folder)).unwrap_or_else(|error| {
            panic!("cannot create {}: {error}", dir.join(folder).display())
        });
    }
    let binary = dir.join("bin").join("luuna-beacon");

    let status_before = status_with_ignored(&clone);
    let proxy = proxy_pass_through(|name| std::env::var_os(name));
    let env = go_build_env(
        &dir,
        &module_source(),
        &std::env::var_os("PATH").unwrap_or_default(),
        &proxy,
    );
    let program = go_program();
    let output = Command::new(&program)
        .env_clear()
        .envs(env.iter().map(|(name, value)| (name, value)))
        .current_dir(&dir)
        .arg("-C")
        .arg(&clone)
        .args(["build", "-trimpath", "-buildvcs=false", "-o"])
        .arg(&binary)
        .arg("./cmd/luuna-beacon")
        .stdin(Stdio::null())
        .output()
        .unwrap_or_else(|error| panic!("could not start the Go program {program:?}: {error}"));

    let status_after = status_with_ignored(&clone);
    assert_eq!(
        status_before,
        status_after,
        "the Go build changed the reference clone {} (status --porcelain --ignored)",
        clone.display()
    );
    check_reference(&clone);

    if !output.status.success() {
        let names: Vec<&str> = proxy.iter().map(|(name, _)| name.as_str()).collect();
        panic!(
            "the Go build failed ({}); proxy variables passed: [{}]; Go's stderr:\n{}",
            output.status,
            names.join(", "),
            redact(&String::from_utf8_lossy(&output.stderr), &proxy)
        );
    }
    assert!(
        binary.is_file(),
        "the Go build reported success but {} does not exist",
        binary.display()
    );
    Build {
        clone,
        dir,
        binary,
        status_before,
    }
}

fn assert_outside_clone(dir: &Path, clone: &Path) {
    assert!(
        !dir.starts_with(clone),
        "the build directory {} is inside the reference clone {}",
        dir.display(),
        clone.display()
    );
}

/// Replaces every occurrence of a passed proxy value (and of the value
/// without its scheme) with the variable's name, so a failure message never
/// carries a proxy value.
fn redact(text: &str, proxy: &[(String, OsString)]) -> String {
    let mut text = text.to_owned();
    for (name, value) in proxy {
        let value = value.to_string_lossy();
        let mut forms = vec![value.to_string()];
        if let Some((_, rest)) = value.split_once("://") {
            forms.push(rest.to_owned());
        }
        for form in forms {
            if !form.is_empty() {
                text = text.replace(&form, &format!("<value of {name}>"));
            }
        }
    }
    text
}

/// One run of the Go binary.
#[derive(Debug)]
pub struct GoRun {
    /// The exit code.
    pub code: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    /// Stdout parsed as a stream of JSON values: a failed validate prints two
    /// documents.
    pub documents: Vec<Value>,
}

/// Panics unless `root` is an absolute folder outside the reference clone
/// and outside every git repository.
fn check_project_root(root: &Path, clone: &Path) {
    assert!(
        root.is_absolute(),
        "project root {} is not absolute",
        root.display()
    );
    let canonical = root.canonicalize().unwrap_or_else(|error| {
        panic!("project root {} cannot be opened: {error}", root.display())
    });
    assert!(
        !canonical.starts_with(clone),
        "project root {} is inside the reference clone",
        canonical.display()
    );
    for folder in canonical.ancestors() {
        assert!(
            !is_git_marker(&folder.join(".git")),
            "project root {} is inside the git repository {}",
            canonical.display(),
            folder.display()
        );
    }
}

/// Whether `dot_git` marks a git repository without running git: a `.git`
/// file (a linked worktree or submodule) or a `.git` folder with a `HEAD`.
/// An empty `.git` folder, which git itself does not take for a repository,
/// is not a marker.
fn is_git_marker(dot_git: &Path) -> bool {
    dot_git.is_file() || dot_git.join("HEAD").exists()
}

/// Runs the built Go binary with `args` and `--json` appended, in
/// `project_root`, with a cleared environment whose PATH, HOME and TMPDIR are
/// folders under the build directory.
pub fn run_go(project_root: &Path, args: &[&str]) -> GoRun {
    let build = go_build();
    check_project_root(project_root, &build.clone);
    let oracle = build.dir.join("oracle");
    let (path, home, tmp) = (oracle.join("path"), oracle.join("home"), oracle.join("tmp"));
    for folder in [&path, &home, &tmp] {
        std::fs::create_dir_all(folder)
            .unwrap_or_else(|error| panic!("cannot create {}: {error}", folder.display()));
    }
    let output = Command::new(&build.binary)
        .env_clear()
        .env("PATH", &path)
        .env("HOME", &home)
        .env("TMPDIR", &tmp)
        .current_dir(project_root)
        .args(args)
        .arg("--json")
        .stdin(Stdio::null())
        .output()
        .unwrap_or_else(|error| panic!("could not start {}: {error}", build.binary.display()));
    let code = output
        .status
        .code()
        .unwrap_or_else(|| panic!("the Go binary was stopped by a signal: {}", output.status));
    let documents = serde_json::Deserializer::from_slice(&output.stdout)
        .into_iter::<Value>()
        .collect::<Result<Vec<_>, _>>()
        .unwrap_or_else(|error| {
            panic!(
                "Go's stdout for {args:?} is not a stream of JSON values ({error}):\n{}",
                String::from_utf8_lossy(&output.stdout)
            )
        });
    GoRun {
        code,
        stdout: output.stdout,
        stderr: output.stderr,
        documents,
    }
}
