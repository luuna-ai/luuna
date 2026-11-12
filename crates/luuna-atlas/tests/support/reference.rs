//! Locating, checking and building the Go reference tool (plan s5-atlas-lib, "Harness").
//!
//! `LUUNA_ATLAS_REFERENCE` names the luuna-atlas clone and `LUUNA_ATLAS_GO` the `go`
//! executable. Before anything runs, the harness refuses a bad reference and names the
//! variable at fault. The clone is only read: every git call against it sets
//! `GIT_OPTIONAL_LOCKS=0` so that not even its index is refreshed, and the Go build writes
//! its binary, caches, temporary files and home under
//! `<workspace>/target/atlas-reference/<commit>/`, offline and in vendor mode.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;

/// The luuna-atlas commit the crate is compared against.
pub const REFERENCE_COMMIT: &str = "c4db80c3f2e622afbdd84f09077fd73b2a92d199";
/// The Go release that builds it.
pub const GO_VERSION: &str = "go1.27.0";
/// Names the reference clone.
pub const REFERENCE_VAR: &str = "LUUNA_ATLAS_REFERENCE";
/// Names the `go` executable.
pub const GO_VAR: &str = "LUUNA_ATLAS_GO";

const HOW_TO_RUN: &str = "set LUUNA_ATLAS_REFERENCE to the luuna-atlas clone at \
c4db80c3f2e622afbdd84f09077fd73b2a92d199 and LUUNA_ATLAS_GO to a go1.27.0 executable, then run \
`cargo test -p luuna-atlas --locked --test parity -- --ignored` (or `--test format_compat`)";

/// Environment variables that could point a git call at another repository or index.
const GIT_REDIRECT_VARS: &[&str] = &[
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_COMMON_DIR",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_NAMESPACE",
    "GIT_CEILING_DIRECTORIES",
];

/// The workspace root: two levels above this crate (plan assumption A2). It does not
/// depend on `CARGO_TARGET_DIR`.
pub fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("the crate lives at <workspace>/crates/luuna-atlas")
        .to_path_buf()
}

/// `<workspace>/target/atlas-reference/<commit>`: everything the Go build writes.
pub fn build_root(workspace: &Path) -> PathBuf {
    workspace
        .join("target")
        .join("atlas-reference")
        .join(REFERENCE_COMMIT)
}

/// Where the built Go tool is placed.
pub fn binary_path(build_root: &Path) -> PathBuf {
    build_root.join("bin").join("luuna-atlas")
}

/// The complete environment of every `go` invocation: nothing is inherited except `PATH`.
///
/// `go build` stamps VCS information, so it runs `git status --porcelain` and `git log` in
/// the reference clone with this environment. `GIT_OPTIONAL_LOCKS=0` keeps that `status`
/// from taking `index.lock` or refreshing the clone's index, and no system or global git
/// config is read, so no user setting (fsmonitor, hooks into status) runs in the clone.
pub fn go_build_env(build_root: &Path, path: Option<OsString>) -> Vec<(OsString, OsString)> {
    let mut env: Vec<(OsString, OsString)> = vec![
        ("GIT_OPTIONAL_LOCKS".into(), "0".into()),
        ("GIT_CONFIG_NOSYSTEM".into(), "1".into()),
        ("GIT_CONFIG_GLOBAL".into(), null_device().into()),
        ("GOFLAGS".into(), "-mod=vendor".into()),
        ("GOTOOLCHAIN".into(), "local".into()),
        ("GOPROXY".into(), "off".into()),
        ("GOSUMDB".into(), "off".into()),
        ("GOWORK".into(), "off".into()),
        ("GOENV".into(), "off".into()),
        ("CGO_ENABLED".into(), "0".into()),
        ("GOCACHE".into(), build_root.join("gocache").into()),
        ("GOTMPDIR".into(), build_root.join("tmp").into()),
        ("GOPATH".into(), build_root.join("gopath").into()),
        ("GOMODCACHE".into(), build_root.join("gomodcache").into()),
        ("HOME".into(), build_root.join("home").into()),
    ];
    if let Some(path) = path {
        env.push(("PATH".into(), path));
    }
    env
}

/// The directories named in [`go_build_env`], which must exist before `go` runs.
pub fn build_dirs(build_root: &Path) -> Vec<PathBuf> {
    ["bin", "gocache", "tmp", "gopath", "gomodcache", "home"]
        .iter()
        .map(|d| build_root.join(d))
        .collect()
}

/// A `go` invocation with the cleared build environment, run from the build root.
pub fn go_command(go: &Path, build_root: &Path) -> Command {
    let mut cmd = Command::new(go);
    cmd.env_clear()
        .envs(go_build_env(build_root, std::env::var_os("PATH")))
        .current_dir(build_root)
        .stdin(Stdio::null());
    cmd
}

/// `go -C <reference> build -trimpath -o <output> ./cmd/luuna-atlas`.
pub fn go_build_command(go: &Path, reference: &Path, build_root: &Path, output: &Path) -> Command {
    let mut cmd = go_command(go, build_root);
    cmd.arg("-C")
        .arg(reference)
        .args(["build", "-trimpath", "-o"])
        .arg(output)
        .arg("./cmd/luuna-atlas");
    cmd
}

/// A read-only git call against the reference clone. `GIT_OPTIONAL_LOCKS=0` keeps
/// `git status` from refreshing the clone's index; system and global config are ignored so
/// that no user setting (fsmonitor, hidden untracked files) changes what is read or
/// written; variables that could redirect git elsewhere are removed.
pub fn reference_git(reference: &Path) -> Command {
    let mut cmd = Command::new("git");
    for var in GIT_REDIRECT_VARS {
        cmd.env_remove(var);
    }
    cmd.env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", null_device())
        .env("GIT_TERMINAL_PROMPT", "0")
        .arg("-C")
        .arg(reference)
        .stdin(Stdio::null());
    cmd
}

/// The platform's empty file, for `GIT_CONFIG_GLOBAL`.
pub fn null_device() -> &'static str {
    if cfg!(windows) {
        "NUL"
    } else {
        "/dev/null"
    }
}

/// Checks that both variables are set and name something that exists. Every problem is
/// reported, each naming its variable.
pub fn check_variables(
    reference: Option<OsString>,
    go: Option<OsString>,
) -> Result<(PathBuf, PathBuf), String> {
    let mut problems = Vec::new();
    let reference = match reference.filter(|v| !v.is_empty()) {
        None => {
            problems.push(format!(
                "{REFERENCE_VAR} is not set; it must name the luuna-atlas reference clone"
            ));
            None
        }
        Some(v) => {
            let path = PathBuf::from(v);
            if path.is_dir() {
                Some(path)
            } else {
                problems.push(format!(
                    "{REFERENCE_VAR}={} does not exist or is not a directory",
                    path.display()
                ));
                None
            }
        }
    };
    let go = match go.filter(|v| !v.is_empty()) {
        None => {
            problems.push(format!(
                "{GO_VAR} is not set; it must name the {GO_VERSION} `go` executable"
            ));
            None
        }
        Some(v) => {
            let path = PathBuf::from(v);
            if path.is_file() {
                Some(path)
            } else {
                problems.push(format!(
                    "{GO_VAR}={} does not exist or is not a file",
                    path.display()
                ));
                None
            }
        }
    };
    match (reference, go) {
        (Some(reference), Some(go)) if problems.is_empty() => Ok((reference, go)),
        _ => Err(refusal(&problems.join("; "))),
    }
}

/// Checks that `go version` reports exactly [`GO_VERSION`].
pub fn check_go_version(go: &Path, build_root: &Path) -> Result<(), String> {
    let output = go_command(go, build_root)
        .arg("version")
        .output()
        .map_err(|e| {
            refusal(&format!(
                "{GO_VAR}={}: `go version` failed to start: {e}",
                go.display()
            ))
        })?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let reported = stdout.split_whitespace().nth(2).unwrap_or("");
    if !output.status.success() || reported != GO_VERSION {
        return Err(refusal(&format!(
            "{GO_VAR}={}: `go version` printed {:?} (stderr {:?}), expected {GO_VERSION}",
            go.display(),
            stdout.trim(),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(())
}

/// Checks that the clone's HEAD is `expected`.
pub fn check_head(reference: &Path, expected: &str) -> Result<(), String> {
    let output = reference_git(reference)
        .args(["rev-parse", "HEAD"])
        .output()
        .map_err(|e| {
            refusal(&format!(
                "{REFERENCE_VAR}={}: git failed to start: {e}",
                reference.display()
            ))
        })?;
    let head = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if !output.status.success() || head != expected {
        return Err(refusal(&format!(
            "{REFERENCE_VAR}={}: HEAD is {:?} (git stderr {:?}), expected {expected}",
            reference.display(),
            head,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(())
}

/// Checks that `git status --porcelain --ignored` prints nothing in the clone.
pub fn check_clean(reference: &Path) -> Result<(), String> {
    let output = reference_git(reference)
        .args(["status", "--porcelain", "--ignored"])
        .output()
        .map_err(|e| {
            refusal(&format!(
                "{REFERENCE_VAR}={}: git failed to start: {e}",
                reference.display()
            ))
        })?;
    if !output.status.success() || !output.stdout.is_empty() {
        return Err(refusal(&format!(
            "{REFERENCE_VAR}={} is not clean: `git status --porcelain --ignored` exited {:?} and printed {:?} (stderr {:?})",
            reference.display(),
            output.status.code(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(())
}

/// Builds the Go tool into [`binary_path`]: into `<binary>.tmp.<pid>` first, then renamed
/// into place, so that concurrent test binaries never see a half-written file.
pub fn build_go_tool(go: &Path, reference: &Path, build_root: &Path) -> Result<PathBuf, String> {
    for dir in build_dirs(build_root) {
        std::fs::create_dir_all(&dir).map_err(|e| format!("creating {}: {e}", dir.display()))?;
    }
    let binary = binary_path(build_root);
    let partial = binary.with_file_name(format!("luuna-atlas.tmp.{}", std::process::id()));
    let output = go_build_command(go, reference, build_root, &partial)
        .output()
        .map_err(|e| {
            refusal(&format!(
                "{GO_VAR}={}: `go build` failed to start: {e}",
                go.display()
            ))
        })?;
    if !output.status.success() {
        let _ = std::fs::remove_file(&partial);
        return Err(refusal(&format!(
            "building the Go tool from {REFERENCE_VAR}={} with {GO_VAR}={} failed:\n{}",
            reference.display(),
            go.display(),
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    std::fs::rename(&partial, &binary).map_err(|e| {
        let _ = std::fs::remove_file(&partial);
        format!("moving {} to {}: {e}", partial.display(), binary.display())
    })?;
    Ok(binary)
}

/// The checked reference and the Go tool built from it.
#[derive(Debug)]
pub struct Reference {
    pub clone: PathBuf,
    pub go: PathBuf,
    pub build_root: PathBuf,
    pub binary: PathBuf,
}

/// The reference for this test binary: checked and built once, on first use. Panics with
/// a message naming the variable at fault if the reference is missing or wrong; every test
/// that needs the reference then fails with that message.
pub fn reference() -> &'static Reference {
    static REFERENCE: OnceLock<Result<Reference, String>> = OnceLock::new();
    match REFERENCE.get_or_init(prepare) {
        Ok(reference) => reference,
        Err(message) => panic!("{message}"),
    }
}

fn prepare() -> Result<Reference, String> {
    let (clone, go) = check_variables(std::env::var_os(REFERENCE_VAR), std::env::var_os(GO_VAR))?;
    let build_root = build_root(&workspace_root());
    for dir in build_dirs(&build_root) {
        std::fs::create_dir_all(&dir).map_err(|e| format!("creating {}: {e}", dir.display()))?;
    }
    check_go_version(&go, &build_root)?;
    check_head(&clone, REFERENCE_COMMIT)?;
    check_clean(&clone)?;
    let binary = build_go_tool(&go, &clone, &build_root)?;
    check_head(&clone, REFERENCE_COMMIT).map_err(|e| format!("after the Go build: {e}"))?;
    check_clean(&clone).map_err(|e| format!("after the Go build: {e}"))?;
    Ok(Reference {
        clone,
        go,
        build_root,
        binary,
    })
}

/// The refusal message: the problem on the first line, then how to run the check.
fn refusal(problem: &str) -> String {
    format!("refusing to run the Go comparison: {problem}\nTo run it, {HOW_TO_RUN}.")
}
