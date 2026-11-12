//! The read-only reference clone of the Go tool and the Go build.
//!
//! Frozen after t1 of s4-missions-lib, and frozen for s4-missions-lib-2 to -4.
//!
//! - `LUUNA_MISSIONS_REFERENCE` names the clone. It must exist, be a git
//!   clone, and have HEAD at [`REFERENCE_COMMIT`]; anything else fails hard.
//!   The only git commands run in it are `rev-parse HEAD` and
//!   `--no-optional-locks status --porcelain --ignored`.
//! - `LUUNA_MISSIONS_GO` names the Go binary. The tool is built from the
//!   clone's `./cmd/luuna-missions` with `go build -buildvcs=false -trimpath`
//!   into `target/tmp/missions-parity/bin`, never with `cmd/dev`.
//! - Go gets its own module cache, build cache, temp folder and GOPATH under
//!   `target/tmp/missions-parity` (git-ignored), `GOFLAGS='-mod=readonly
//!   -modcacherw'` so go.mod and go.sum are never rewritten,
//!   `GOTOOLCHAIN=local`, `GOWORK=off`, `GOENV=off`, and `GOPROXY` from
//!   `LUUNA_MISSIONS_GOPROXY` (default https://proxy.golang.org). Nothing is
//!   written into the clone or into the host's Go cache.
//! - [`Guard`] records the clone's status and HEAD and the worktree's status
//!   before a run and verifies them after it.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;

use serde_json::{json, Value};

use super::{git_read, parity_dir, workspace_root};

/// The reference commit every comparison is tied to.
pub const REFERENCE_COMMIT: &str = "2fbbf8e5b6a238ea47543e1dad112fe1716faec4";
/// How many regression fixtures the reference has; the Go harness requires
/// exactly this many.
pub const FIXTURE_TOTAL: usize = 194;
/// Names the reference clone.
pub const REFERENCE_VAR: &str = "LUUNA_MISSIONS_REFERENCE";
/// Names the Go binary.
pub const GO_VAR: &str = "LUUNA_MISSIONS_GO";
/// The GOPROXY for the harness's module cache.
pub const GOPROXY_VAR: &str = "LUUNA_MISSIONS_GOPROXY";
/// GOPROXY when `LUUNA_MISSIONS_GOPROXY` is unset.
pub const DEFAULT_GOPROXY: &str = "https://proxy.golang.org";
/// The module archive the build must leave in the harness's module cache.
pub const REQUIRED_MODULE_ZIP: &str = "cache/download/gopkg.in/yaml.v3/@v/v3.0.1.zip";

fn non_empty_var(name: &str) -> Option<OsString> {
    std::env::var_os(name).filter(|value| !value.is_empty())
}

/// The clone path named by `LUUNA_MISSIONS_REFERENCE`, if set.
pub fn configured_path() -> Option<PathBuf> {
    non_empty_var(REFERENCE_VAR).map(PathBuf::from)
}

/// A verified reference clone.
#[derive(Debug, Clone)]
pub struct Reference {
    path: PathBuf,
}

impl Reference {
    /// The canonical clone path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The fixture folder, `internal/regression/testdata`.
    pub fn testdata(&self) -> PathBuf {
        self.path.join("internal/regression/testdata")
    }
}

/// Checks that `path` is a git clone at [`REFERENCE_COMMIT`] with the fixture
/// folder. A missing path, a folder that is not a git clone, and any other
/// commit are errors.
pub fn check_reference(path: &Path) -> Result<Reference, String> {
    if !path.exists() {
        return Err(format!("{} does not exist", path.display()));
    }
    if !path.is_dir() {
        return Err(format!("{} is not a directory", path.display()));
    }
    let path = path
        .canonicalize()
        .map_err(|error| format!("{}: {error}", path.display()))?;
    if !path.join(".git").exists() {
        return Err(format!(
            "{} is not a git clone (it has no .git)",
            path.display()
        ));
    }
    let head = git_read(&path, &["rev-parse", "HEAD"])
        .map_err(|error| format!("{} is not a usable git clone: {error}", path.display()))?;
    let head = head.trim();
    if head != REFERENCE_COMMIT {
        return Err(format!(
            "{} is at {head}, not at the reference commit {REFERENCE_COMMIT}",
            path.display()
        ));
    }
    clone_status(&path)?;
    let reference = Reference { path };
    if !reference.testdata().is_dir() {
        return Err(format!(
            "{} has no {}",
            reference.path.display(),
            reference.testdata().display()
        ));
    }
    Ok(reference)
}

fn clone_status(path: &Path) -> Result<String, String> {
    git_read(
        path,
        &["--no-optional-locks", "status", "--porcelain", "--ignored"],
    )
}

fn clone_head(path: &Path) -> Result<String, String> {
    git_read(path, &["rev-parse", "HEAD"]).map(|head| head.trim().to_owned())
}

fn worktree_status() -> Result<String, String> {
    git_read(
        &workspace_root(),
        &["--no-optional-locks", "status", "--porcelain"],
    )
}

/// The clone named by `LUUNA_MISSIONS_REFERENCE`, verified; panics with a
/// message naming the variable otherwise.
pub fn reference_from_env() -> Reference {
    let Some(path) = configured_path() else {
        panic!(
            "{REFERENCE_VAR} is not set. Set it to a clone of luuna-ai/luuna-missions at \
             {REFERENCE_COMMIT}."
        );
    };
    check_reference(&path)
        .unwrap_or_else(|error| panic!("{REFERENCE_VAR}={}: {error}", path.display()))
}

/// The state of the reference clone and the luuna worktree before a run.
pub struct Guard {
    clone: PathBuf,
    clone_status: String,
    worktree_status: String,
}

impl Guard {
    /// Records the clone's `status --porcelain --ignored` and HEAD and the
    /// worktree's `status --porcelain`, and checks that the harness folder
    /// is inside the worktree and git-ignored.
    pub fn capture(reference: &Reference) -> Self {
        let head =
            clone_head(reference.path()).unwrap_or_else(|error| panic!("{REFERENCE_VAR}: {error}"));
        assert_eq!(head, REFERENCE_COMMIT, "{REFERENCE_VAR}: the clone moved");
        let clone_status = clone_status(reference.path())
            .unwrap_or_else(|error| panic!("{REFERENCE_VAR}: {error}"));
        let worktree_status =
            worktree_status().unwrap_or_else(|error| panic!("luuna worktree: {error}"));
        assert_parity_dir_ignored();
        Self {
            clone: reference.path().to_path_buf(),
            clone_status,
            worktree_status,
        }
    }

    /// Checks that nothing changed since [`Guard::capture`].
    pub fn verify(&self) -> Result<(), String> {
        let mut changes = Vec::new();
        match clone_head(&self.clone) {
            Ok(head) if head == REFERENCE_COMMIT => {}
            Ok(head) => changes.push(format!("the reference clone moved to {head}")),
            Err(error) => changes.push(error),
        }
        match clone_status(&self.clone) {
            Ok(status) if status == self.clone_status => {}
            Ok(status) => changes.push(format!(
                "the reference clone's status changed\nbefore:\n{}\nafter:\n{status}",
                self.clone_status
            )),
            Err(error) => changes.push(error),
        }
        match worktree_status() {
            Ok(status) if status == self.worktree_status => {}
            Ok(status) => changes.push(format!(
                "the luuna worktree's status changed\nbefore:\n{}\nafter:\n{status}",
                self.worktree_status
            )),
            Err(error) => changes.push(error),
        }
        if changes.is_empty() {
            Ok(())
        } else {
            Err(changes.join("\n"))
        }
    }
}

fn assert_parity_dir_ignored() {
    let workspace = workspace_root();
    let target_tmp = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .canonicalize()
        .expect("canonical CARGO_TARGET_TMPDIR");
    let dir = target_tmp.join("missions-parity");
    assert!(
        dir.starts_with(&workspace),
        "{} is not inside the luuna worktree {}",
        dir.display(),
        workspace.display()
    );
    let relative = dir.strip_prefix(&workspace).expect("inside the worktree");
    let relative = relative.to_string_lossy().replace('\\', "/");
    assert!(
        git_read(&workspace, &["check-ignore", "-q", &relative]).is_ok(),
        "{relative} is not git-ignored in the luuna worktree"
    );
}

/// The Go toolchain and the Go tool built from the clone.
#[derive(Debug)]
pub struct GoTool {
    go: PathBuf,
    version: String,
    env: BTreeMap<String, String>,
    binary: PathBuf,
}

impl GoTool {
    /// The built `luuna-missions` binary.
    pub fn binary(&self) -> &Path {
        &self.binary
    }

    /// What the report records about the toolchain.
    pub fn describe(&self) -> Value {
        json!({
            "go_binary": self.go.to_string_lossy(),
            "go_version": self.version,
            "GOPROXY": self.env["GOPROXY"],
            "GOMODCACHE": self.env["GOMODCACHE"],
            "GOCACHE": self.env["GOCACHE"],
            "GOTMPDIR": self.env["GOTMPDIR"],
            "GOPATH": self.env["GOPATH"],
            "GOFLAGS": self.env["GOFLAGS"],
            "GOTOOLCHAIN": self.env["GOTOOLCHAIN"],
            "GOWORK": self.env["GOWORK"],
            "GOENV": self.env["GOENV"],
            "built_binary": self.binary.to_string_lossy(),
        })
    }

    fn command(&self, reference: &Reference) -> Command {
        let mut command = Command::new(&self.go);
        command.current_dir(reference.path()).stdin(Stdio::null());
        for (key, _) in std::env::vars_os() {
            if key.to_string_lossy().starts_with("GO") {
                command.env_remove(key);
            }
        }
        command.envs(&self.env);
        command
    }

    fn build(&self, reference: &Reference, package: &str, output: &Path) -> Result<(), String> {
        let parent = output.parent().expect("output folder");
        fs::create_dir_all(parent).map_err(|error| format!("{}: {error}", parent.display()))?;
        let staging = parent.join(format!(
            ".{}.{}.tmp",
            output.file_name().expect("file name").to_string_lossy(),
            std::process::id()
        ));
        let result = self
            .command(reference)
            .args(["build", "-buildvcs=false", "-trimpath", "-o"])
            .arg(&staging)
            .arg(package)
            .output()
            .map_err(|error| format!("{GO_VAR}={}: {error}", self.go.display()))?;
        if !result.status.success() {
            let _ = fs::remove_file(&staging);
            return Err(format!(
                "{GO_VAR}={}: go build {package} failed ({}); check {GOPROXY_VAR}={}:\n{}",
                self.go.display(),
                result.status,
                self.env["GOPROXY"],
                String::from_utf8_lossy(&result.stderr)
            ));
        }
        fs::rename(&staging, output).map_err(|error| format!("{}: {error}", output.display()))
    }
}

fn setup_go(reference: &Reference) -> Result<GoTool, String> {
    let go = non_empty_var(GO_VAR).ok_or_else(|| {
        format!("{GO_VAR} is not set. Set it to the Go binary that builds the reference.")
    })?;
    let go = PathBuf::from(go);
    if !go.is_file() {
        return Err(format!("{GO_VAR}={}: no such file", go.display()));
    }
    let root = parity_dir();
    let dir = |name: &str| -> Result<String, String> {
        let path = root.join(name);
        fs::create_dir_all(&path).map_err(|error| format!("{}: {error}", path.display()))?;
        Ok(path.to_string_lossy().into_owned())
    };
    let goproxy = std::env::var(GOPROXY_VAR)
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| DEFAULT_GOPROXY.to_owned());
    let env = BTreeMap::from([
        ("GOTOOLCHAIN".to_owned(), "local".to_owned()),
        ("GOWORK".to_owned(), "off".to_owned()),
        ("GOENV".to_owned(), "off".to_owned()),
        ("GOFLAGS".to_owned(), "-mod=readonly -modcacherw".to_owned()),
        ("GOPROXY".to_owned(), goproxy),
        ("GOMODCACHE".to_owned(), dir("gomod")?),
        ("GOCACHE".to_owned(), dir("gocache")?),
        ("GOTMPDIR".to_owned(), dir("gotmp")?),
        ("GOPATH".to_owned(), dir("gopath")?),
    ]);
    let mut tool = GoTool {
        go,
        version: String::new(),
        env,
        binary: root
            .join("bin")
            .join(format!("luuna-missions{}", std::env::consts::EXE_SUFFIX)),
    };
    let version = tool
        .command(reference)
        .arg("version")
        .output()
        .map_err(|error| format!("{GO_VAR}={}: {error}", tool.go.display()))?;
    if !version.status.success() {
        return Err(format!(
            "{GO_VAR}={}: go version failed: {}",
            tool.go.display(),
            String::from_utf8_lossy(&version.stderr)
        ));
    }
    tool.version = String::from_utf8_lossy(&version.stdout).trim().to_owned();
    let binary = tool.binary.clone();
    tool.build(reference, "./cmd/luuna-missions", &binary)?;
    let module = Path::new(&tool.env["GOMODCACHE"]).join(REQUIRED_MODULE_ZIP);
    if !module.is_file() {
        return Err(format!(
            "{} is missing after the build; check {GOPROXY_VAR}",
            module.display()
        ));
    }
    Ok(tool)
}

/// The Go tool built from the clone, once per test process. Panics with a
/// message naming `LUUNA_MISSIONS_GO` (and `LUUNA_MISSIONS_GOPROXY` for a
/// failed download) when Go is missing or the build fails.
pub fn go_tool(reference: &Reference) -> &'static GoTool {
    static TOOL: OnceLock<Result<GoTool, String>> = OnceLock::new();
    match TOOL.get_or_init(|| setup_go(reference)) {
        Ok(tool) => tool,
        Err(error) => panic!("{error}"),
    }
}

/// The folder holding the fixture stub `luuna-atlas`, built once per test
/// process from the clone's `internal/regression/testdata/atlas`, as the Go
/// harness builds it.
pub fn atlas_dir(reference: &Reference) -> &'static Path {
    static ATLAS: OnceLock<Result<PathBuf, String>> = OnceLock::new();
    let result = ATLAS.get_or_init(|| {
        let tool = go_tool(reference);
        let dir = parity_dir().join("bin").join("atlas");
        let binary = dir.join(format!("luuna-atlas{}", std::env::consts::EXE_SUFFIX));
        tool.build(reference, "./internal/regression/testdata/atlas", &binary)?;
        Ok(dir)
    });
    match result {
        Ok(dir) => dir,
        Err(error) => panic!("{error}"),
    }
}
