//! Global options, the process environment and the run context shared by the commands.
//! Owner: s5-atlas-lib-2.
//!
//! A port of the Go tool's global flag handling (`cli.go`: the raw pre-scan in `Run` and
//! `setGlobal`) and of the parts of its `App` the building blocks read: the working
//! directory, the environment and `exec.LookPath`.
//!
//! Differences from the Go tool (plan s5-atlas-lib-2a, D1 to D4): the store root, the
//! working directory and the environment are parameters ([`Context`], [`Env`]) instead of
//! process state. [`Env::from_process`] captures the process values, so a caller that
//! passes it gets the Go tool's results; tests pass [`Env::fixed`] and never touch the
//! test process's environment. Only [`Env::from_process`] reads the process environment.

use std::ffi::{OsStr, OsString};
use std::fmt;
use std::path::{Path, PathBuf};

/// The global flags every command accepts (`--json`, `--quiet`/`-q`, `--verbose`,
/// `--allow-unknown-types`, `--allow-domain-mismatch`, `--format`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GlobalOptions {
    pub json: bool,
    pub quiet: bool,
    pub verbose: bool,
    pub allow_unknown_types: bool,
    pub allow_domain_mismatch: bool,
    /// The last `--format` value, or empty.
    pub format: String,
}

impl GlobalOptions {
    /// The Go tool's raw scan of every argument before parsing (`cli.go:71-84`): exactly
    /// `--json`, `--quiet`, `-q`, `--verbose`, `--allow-unknown-types` and
    /// `--allow-domain-mismatch` set their flag wherever they appear, flag values and
    /// arguments after `--` included. `format` is not set by the scan.
    pub fn prescan(args: &[String]) -> GlobalOptions {
        let mut g = GlobalOptions::default();
        for arg in args {
            match arg.as_str() {
                "--json" => g.json = true,
                "--quiet" | "-q" => g.quiet = true,
                "--verbose" => g.verbose = true,
                "--allow-unknown-types" => g.allow_unknown_types = true,
                "--allow-domain-mismatch" => g.allow_domain_mismatch = true,
                _ => {}
            }
        }
        g
    }

    /// `setGlobal` (`cli.go:291-310`): the value is the last of `values`, or `true` when
    /// there is none; a boolean flag is set to `value != "false"`; `format` takes the value
    /// as it is. Unknown names change nothing.
    pub fn set(&mut self, name: &str, values: &[String]) {
        let v = values.last().map_or("true", String::as_str);
        match name {
            "json" => self.json = v != "false",
            "quiet" => self.quiet = v != "false",
            "verbose" => self.verbose = v != "false",
            "allow-unknown-types" => self.allow_unknown_types = v != "false",
            "allow-domain-mismatch" => self.allow_domain_mismatch = v != "false",
            "format" => self.format = v.to_owned(),
            _ => {}
        }
    }
}

/// The environment a command runs in: environment variables, the arguments after the
/// program name (read by [`crate::style::color_level`]) and whether stdout is a terminal.
///
/// Child processes (git, hooks) get exactly these variables, and programs are looked up
/// on this `PATH` ([`Env::look_path`]), never on the test process's.
///
/// `Debug` prints variable names only, never their values.
#[derive(Clone, Default)]
pub struct Env {
    /// Variables in the order they were given. Lookups use the first occurrence of a name.
    vars: Vec<(OsString, OsString)>,
    args: Vec<String>,
    stdout_is_terminal: bool,
}

impl Env {
    /// The process's environment, its arguments after the program name (non-UTF-8 bytes
    /// replaced by U+FFFD) and [`crate::style::stdout_is_terminal`]. The only reader of the
    /// process environment in this crate's building blocks.
    pub fn from_process() -> Env {
        Env {
            vars: std::env::vars_os().collect(),
            args: std::env::args_os()
                .skip(1)
                .map(|a| a.to_string_lossy().into_owned())
                .collect(),
            stdout_is_terminal: crate::style::stdout_is_terminal(),
        }
    }

    /// An environment given in full, for tests and embedders.
    pub fn fixed<I, K, V>(vars: I, args: Vec<String>, stdout_is_terminal: bool) -> Env
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<OsString>,
        V: Into<OsString>,
    {
        Env {
            vars: vars
                .into_iter()
                .map(|(k, v)| (k.into(), v.into()))
                .collect(),
            args,
            stdout_is_terminal,
        }
    }

    /// The value of `name` among this environment's own variables (`os.LookupEnv`), with
    /// non-UTF-8 bytes replaced by U+FFFD; `None` when it is not set.
    pub fn var(&self, name: &str) -> Option<String> {
        // Go's environment map keeps the first occurrence of a name.
        self.vars
            .iter()
            .find(|(k, _)| k.as_os_str() == OsStr::new(name))
            .map(|(_, v)| v.to_string_lossy().into_owned())
    }

    /// Every variable, in order, as given to child processes.
    pub fn vars(&self) -> &[(OsString, OsString)] {
        &self.vars
    }

    /// The arguments after the program name.
    pub fn args(&self) -> &[String] {
        &self.args
    }

    /// Whether stdout is a terminal.
    pub fn stdout_is_terminal(&self) -> bool {
        self.stdout_is_terminal
    }

    /// Go's `exec.LookPath(name)` over this environment's `PATH` (P25): each element in
    /// order, an empty element meaning `.`; the first regular file with an execute bit
    /// wins. A result that is not absolute (found through `.`, an empty element or a
    /// relative element) is refused, as Go 1.19+ does with `ErrDot`; `GODEBUG` is not
    /// honoured (D4). `None` when nothing is found or the result is refused.
    pub fn look_path(&self, name: &str) -> Option<PathBuf> {
        if name.contains('/') {
            // Go returns the name itself when it holds a slash, after the same check.
            return is_executable_file(Path::new(name)).then(|| PathBuf::from(name));
        }
        let path = self.var("PATH").unwrap_or_default();
        // filepath.SplitList: an empty PATH has no elements.
        if path.is_empty() {
            return None;
        }
        for dir in path.split(':') {
            let dir = if dir.is_empty() { "." } else { dir };
            let candidate = go_join(dir, name);
            if is_executable_file(&candidate) {
                // Go stops at the first match; a relative one is ErrDot, not a result.
                return candidate.is_absolute().then_some(candidate);
            }
        }
        None
    }
}

/// `findExecutable` (os/exec, unix): the file exists (symlinks followed), is a regular file
/// and has an execute bit set.
#[cfg(unix)]
fn is_executable_file(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable_file(path: &Path) -> bool {
    std::fs::metadata(path)
        .map(|m| m.is_file())
        .unwrap_or(false)
}

/// `filepath.Join(dir, name)` for two non-empty-or-empty elements: joined with `/` and
/// lexically cleaned (`filepath.Clean`).
fn go_join(dir: &str, name: &str) -> PathBuf {
    let joined = if name.is_empty() {
        dir.to_owned()
    } else {
        format!("{dir}/{name}")
    };
    let rooted = joined.starts_with('/');
    let mut parts: Vec<&str> = Vec::new();
    for part in joined.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                if parts.last().is_some_and(|p| *p != "..") {
                    parts.pop();
                } else if !rooted {
                    parts.push("..");
                }
            }
            other => parts.push(other),
        }
    }
    let mut out = parts.join("/");
    if rooted {
        out.insert(0, '/');
    }
    if out.is_empty() {
        out.push('.');
    }
    PathBuf::from(out)
}

impl fmt::Debug for Env {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Env")
            .field(
                "var_names",
                &self.vars.iter().map(|(k, _)| k).collect::<Vec<_>>(),
            )
            .field("args", &self.args.len())
            .field("stdout_is_terminal", &self.stdout_is_terminal)
            .finish()
    }
}

/// What every building block needs to know about the run: the store's project root, the
/// working directory git and hooks run in, the environment and the global flags.
#[derive(Debug, Clone)]
pub struct Context {
    /// The project root whose `.luuna/atlas` is the store (D1: a parameter).
    pub root: PathBuf,
    /// The working directory for git and hooks (the Go tool's `a.Cwd`).
    pub cwd: PathBuf,
    pub env: Env,
    pub globals: GlobalOptions,
}

impl Context {
    /// A context whose working directory is `root`, with [`Env::from_process`] and
    /// default global options.
    pub fn new(root: impl Into<PathBuf>) -> Context {
        let root = root.into();
        Context {
            cwd: root.clone(),
            root,
            env: Env::from_process(),
            globals: GlobalOptions::default(),
        }
    }
}

/// Helpers shared by the unit tests of the building blocks. They create their own
/// directories with tempfile and never set or clear the test process's environment.
#[cfg(test)]
pub(crate) mod test_support {
    use std::ffi::OsString;
    use std::fs;
    use std::path::{Component, Path, PathBuf};
    use std::process::{Command, Stdio};

    use super::Env;

    /// The `PATH` of the test process, read through [`Env::from_process`].
    pub(crate) fn process_path() -> OsString {
        Env::from_process()
            .var("PATH")
            .map(OsString::from)
            .unwrap_or_default()
    }

    /// Writes an executable shell script `name` into `dir` and returns its path.
    pub(crate) fn write_script(dir: &Path, name: &str, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join(name);
        fs::write(&path, format!("#!/bin/sh\n{body}\n")).expect("write script");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("chmod script");
        path
    }

    /// A relative path from the test process's working directory to `target` (both
    /// canonical), so that a `PATH` element can name `target` without being absolute.
    pub(crate) fn relative_from_cwd(target: &Path) -> PathBuf {
        let cwd = std::env::current_dir()
            .expect("current dir")
            .canonicalize()
            .expect("canonical cwd");
        let target = target.canonicalize().expect("canonical target");
        let cwd_parts: Vec<Component<'_>> = cwd.components().collect();
        let target_parts: Vec<Component<'_>> = target.components().collect();
        let common = cwd_parts
            .iter()
            .zip(&target_parts)
            .take_while(|(a, b)| a == b)
            .count();
        let mut out = PathBuf::new();
        for _ in common..cwd_parts.len() {
            out.push("..");
        }
        for part in &target_parts[common..] {
            out.push(part.as_os_str());
        }
        if out.as_os_str().is_empty() {
            out.push(".");
        }
        out
    }

    /// The variables a hermetic git run needs: the process `PATH`, a `HOME` inside the
    /// test's own directory, no system config and a fixed identity.
    pub(crate) fn git_vars(home: &Path) -> Vec<(OsString, OsString)> {
        vec![
            ("PATH".into(), process_path()),
            ("HOME".into(), home.as_os_str().to_owned()),
            ("GIT_CONFIG_NOSYSTEM".into(), "1".into()),
            ("GIT_AUTHOR_NAME".into(), "Test".into()),
            ("GIT_AUTHOR_EMAIL".into(), "test@example.invalid".into()),
            ("GIT_COMMITTER_NAME".into(), "Test".into()),
            ("GIT_COMMITTER_EMAIL".into(), "test@example.invalid".into()),
            ("GIT_AUTHOR_DATE".into(), "2024-01-01T00:00:00Z".into()),
            ("GIT_COMMITTER_DATE".into(), "2024-01-01T00:00:00Z".into()),
        ]
    }

    /// Runs `git args` in `cwd` with exactly [`git_vars`] (test setup only, independent of
    /// the code under test) and panics on failure.
    pub(crate) fn setup_git(cwd: &Path, home: &Path, args: &[&str]) {
        let status = Command::new("git")
            .args(args)
            .current_dir(cwd)
            .env_clear()
            .envs(git_vars(home))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .expect("start git for test setup");
        assert!(status.success(), "git {args:?} failed in test setup");
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::{relative_from_cwd, write_script};
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    fn strings(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_owned()).collect()
    }

    fn env_with_path(path: impl Into<OsString>) -> Env {
        Env::fixed([("PATH", path.into())], Vec::new(), false)
    }

    #[test]
    fn prescan_matches_values_too() {
        // A flag's value that happens to be a global flag sets it, as Go's raw scan does.
        let g = GlobalOptions::prescan(&strings(&["record", "d", "--description", "--json"]));
        assert!(g.json);
        assert!(!g.quiet);
        // Arguments after `--` are scanned too.
        let g = GlobalOptions::prescan(&strings(&["search", "--", "--verbose", "-q"]));
        assert!(g.verbose);
        assert!(g.quiet);
        let g = GlobalOptions::prescan(&strings(&[
            "--quiet",
            "--allow-unknown-types",
            "--allow-domain-mismatch",
        ]));
        assert!(g.quiet && g.allow_unknown_types && g.allow_domain_mismatch);
        assert!(!g.json && !g.verbose);
        // Only the exact spellings match; format is never set by the scan.
        let g = GlobalOptions::prescan(&strings(&["--json=true", "--JSON", "-Q", "--format", "x"]));
        assert_eq!(g, GlobalOptions::default());
    }

    #[test]
    fn set_global_false_and_last_value() {
        let mut g = GlobalOptions::default();
        g.set("json", &[]);
        assert!(g.json);
        g.set("json", &strings(&["false"]));
        assert!(!g.json);
        g.set("json", &strings(&["false", "true"]));
        assert!(g.json);
        g.set("json", &strings(&["true", "false"]));
        assert!(!g.json);
        // Anything but the exact text `false` counts as true.
        g.set("quiet", &strings(&["0"]));
        assert!(g.quiet);
        g.set("quiet", &strings(&["FALSE"]));
        assert!(g.quiet);
        g.set("verbose", &strings(&["yes"]));
        g.set("allow-unknown-types", &[]);
        g.set("allow-domain-mismatch", &strings(&["false"]));
        assert!(g.verbose && g.allow_unknown_types && !g.allow_domain_mismatch);
        g.set("format", &strings(&["markdown", "xml"]));
        assert_eq!(g.format, "xml");
        // With no value the value is the text `true`, for format too.
        g.set("format", &[]);
        assert_eq!(g.format, "true");
        let before = g.clone();
        g.set("unknown", &strings(&["false"]));
        g.set("allow_unknown_types", &strings(&["false"]));
        assert_eq!(g, before);
    }

    #[test]
    fn look_path_uses_env_path_only() {
        let a = tempfile::tempdir().expect("tempdir");
        let b = tempfile::tempdir().expect("tempdir");
        let tool = write_script(b.path(), "atlas-test-tool", "exit 0");
        let joined = format!("{}:{}", a.path().display(), b.path().display());
        assert_eq!(
            env_with_path(joined).look_path("atlas-test-tool"),
            Some(tool)
        );
        // `sh` exists on the test process's PATH, but not on Env's.
        assert_eq!(env_with_path(a.path().as_os_str()).look_path("sh"), None);
        // No PATH in Env means nothing is found, whatever the process has.
        let empty = Env::fixed(Vec::<(String, String)>::new(), Vec::new(), false);
        assert_eq!(empty.look_path("sh"), None);
        assert_eq!(empty.var("PATH"), None);
    }

    #[test]
    fn look_path_refuses_relative_result() {
        let dir = tempfile::tempdir().expect("tempdir");
        let abs = tempfile::tempdir().expect("tempdir");
        write_script(dir.path(), "atlas-test-tool", "exit 0");
        let found = write_script(abs.path(), "atlas-test-tool", "exit 0");
        let relative = relative_from_cwd(dir.path());
        assert!(relative.is_relative());
        // Found only through a relative element: refused (Go ErrDot).
        assert_eq!(
            env_with_path(relative.as_os_str()).look_path("atlas-test-tool"),
            None
        );
        // Go stops at the first match even when it is refused.
        let rel_first = format!("{}:{}", relative.display(), abs.path().display());
        assert_eq!(env_with_path(rel_first).look_path("atlas-test-tool"), None);
        let abs_first = format!("{}:{}", abs.path().display(), relative.display());
        assert_eq!(
            env_with_path(abs_first).look_path("atlas-test-tool"),
            Some(found)
        );
        // An empty element means `.`, which is relative too.
        let with_empty = format!(":{}", abs.path().display());
        let in_cwd = std::env::current_dir()
            .expect("cwd")
            .join("atlas-test-tool")
            .exists();
        if !in_cwd {
            assert!(env_with_path(with_empty)
                .look_path("atlas-test-tool")
                .is_some());
        }
    }

    #[test]
    fn look_path_skips_non_executable() {
        let plain = tempfile::tempdir().expect("tempdir");
        let dir_entry = tempfile::tempdir().expect("tempdir");
        let exec = tempfile::tempdir().expect("tempdir");
        let not_exec = plain.path().join("atlas-test-tool");
        fs::write(&not_exec, "#!/bin/sh\nexit 0\n").expect("write");
        fs::set_permissions(&not_exec, fs::Permissions::from_mode(0o644)).expect("chmod");
        fs::create_dir(dir_entry.path().join("atlas-test-tool")).expect("mkdir");
        let found = write_script(exec.path(), "atlas-test-tool", "exit 0");
        let path = format!(
            "{}:{}:{}",
            plain.path().display(),
            dir_entry.path().display(),
            exec.path().display()
        );
        assert_eq!(
            env_with_path(path).look_path("atlas-test-tool"),
            Some(found)
        );
        assert_eq!(
            env_with_path(plain.path().as_os_str()).look_path("atlas-test-tool"),
            None
        );
    }
}
