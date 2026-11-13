//! Repo-local component commands for the terminal guide.
//! Missions runs through its native Rust library without a toolchain checkout.
//! Atlas and Beacon retain their legacy runners until the remaining CLI wiring.
//! All command environments exclude the runtime connection URL and token.
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::AsyncReadExt;

#[derive(Clone, Debug, PartialEq)]
pub struct Component {
    pub name: &'static str,
    pub brand: &'static str,
    pub directory: &'static str,
    pub entry: &'static str,
    pub store: &'static str,
    pub what: &'static str,
    pub prime: &'static [&'static str],
    pub hooks: bool,
}

pub const COMPONENTS: [Component; 3] = [
    Component {
        name: "missions",
        brand: "Missions",
        directory: "luuna-missions",
        entry: "src/index.ts",
        store: ".luuna/missions",
        what: "tickets and plans",
        prime: &["prime", "--compact"],
        hooks: true,
    },
    Component {
        name: "atlas",
        brand: "Atlas",
        directory: "luuna-atlas",
        entry: "src/cli.ts",
        store: ".luuna/atlas",
        what: "expertise and memory",
        prime: &["prime"],
        hooks: true,
    },
    Component {
        name: "beacon",
        brand: "Beacon",
        directory: "luuna-beacon",
        entry: "src/index.ts",
        store: ".luuna/beacon",
        what: "versioned prompts",
        prime: &["prime", "--compact"],
        hooks: false,
    },
];

const LIMIT: usize = 256 * 1024;
const TIMEOUT: Duration = Duration::from_secs(60);

/// The Luuna root this binary belongs to: the checkout (a development build under target/) or the
/// directory the packaged executable sits in.
pub fn install_root() -> PathBuf {
    let exe = std::env::current_exe()
        .ok()
        .and_then(|p| p.canonicalize().ok().or(Some(p)));
    let directory = exe
        .as_deref()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let mut candidate = Some(directory.as_path());
    for _ in 0..5 {
        let Some(dir) = candidate else { break };
        if dir
            .join("crates")
            .join("luuna-tui")
            .join("Cargo.toml")
            .is_file()
        {
            return dir.to_path_buf();
        }
        candidate = dir.parent();
    }
    directory
}

/// The Bun executable itself, never a shell shim: `LUUNA_BUN`, a `bun`/`bun.exe` on PATH, the
/// executable behind npm's `bun.cmd` shim (node_modules/bun/bin), or ~/.bun/bin.
pub fn resolve_bun(
    env: &dyn Fn(&str) -> Option<String>,
    exists: &dyn Fn(&Path) -> bool,
) -> PathBuf {
    if let Some(bun) = env("LUUNA_BUN").filter(|b| !b.is_empty()) {
        return PathBuf::from(bun);
    }
    let exe = if cfg!(windows) { "bun.exe" } else { "bun" };
    let path = env("PATH").or_else(|| env("Path")).unwrap_or_default();
    let directories: Vec<PathBuf> = std::env::split_paths(&path)
        .filter(|p| !p.as_os_str().is_empty())
        .collect();
    for directory in &directories {
        if exists(&directory.join(exe)) {
            return directory.join(exe);
        }
    }
    for directory in &directories {
        let behind = directory
            .join("node_modules")
            .join("bun")
            .join("bin")
            .join(exe);
        if exists(&behind) {
            return behind;
        }
    }
    let home = env("USERPROFILE")
        .or_else(|| env("HOME"))
        .unwrap_or_default();
    let candidate = Path::new(&home).join(".bun").join("bin").join(exe);
    if exists(&candidate) {
        candidate
    } else {
        PathBuf::from(exe)
    }
}

/// The result of one component run.
#[derive(Clone, Debug, PartialEq)]
pub struct Outcome {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

/// What the guide needs of the component CLIs (a real runner, or a scripted one in tests).
pub trait Runner: Send + Sync {
    fn available(&self) -> Vec<Component>;
    fn run<'a>(
        &'a self,
        name: &'a str,
        args: Vec<String>,
        cwd: &'a str,
    ) -> crate::commands::Fut<'a, Outcome>;
}

/// The real runner over the located CLIs.
pub struct ComponentRunner {
    bun: PathBuf,
    entries: Vec<(String, PathBuf)>,
}

impl ComponentRunner {
    pub fn locate(root: &Path) -> ComponentRunner {
        let env = |name: &str| std::env::var(name).ok();
        let bun = resolve_bun(&env, &|p: &Path| p.is_file());
        let mut entries = Vec::new();
        for component in &COMPONENTS {
            let candidates = [
                root.parent().map(|p| p.join(component.directory)),
                Some(root.join("components").join(component.name)),
            ];
            for directory in candidates.into_iter().flatten() {
                let entry = directory.join(component.entry);
                if entry.is_file() && directory.join("node_modules").exists() {
                    entries.push((component.name.to_string(), entry));
                    break;
                }
            }
        }
        ComponentRunner { bun, entries }
    }
}

/// Removes SGR and other CSI sequences from component output.
fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' && chars.peek() == Some(&'[') {
            chars.next();
            while chars.peek().is_some_and(|n| ('0'..='?').contains(n)) {
                chars.next();
            }
            while chars.peek().is_some_and(|n| (' '..='/').contains(n)) {
                chars.next();
            }
            if chars.peek().is_some_and(|n| ('@'..='~').contains(n)) {
                chars.next();
            }
            continue;
        }
        out.push(c);
    }
    out
}

impl Runner for ComponentRunner {
    fn available(&self) -> Vec<Component> {
        COMPONENTS
            .iter()
            .filter(|c| c.name == "missions" || self.entries.iter().any(|(n, _)| n == c.name))
            .cloned()
            .collect()
    }
    fn run<'a>(
        &'a self,
        name: &'a str,
        args: Vec<String>,
        cwd: &'a str,
    ) -> crate::commands::Fut<'a, Outcome> {
        Box::pin(async move {
            if name == "missions" {
                let env = std::env::vars()
                    .filter(|(key, _)| {
                        !key.eq_ignore_ascii_case("LUUNA_API_TOKEN")
                            && !key.eq_ignore_ascii_case("LUUNA_URL")
                    })
                    .chain([
                        ("NO_COLOR".into(), "1".into()),
                        ("FORCE_COLOR".into(), "0".into()),
                    ])
                    .collect();
                return match luuna_components::run_missions_native(
                    Path::new(cwd),
                    &args,
                    env,
                    Vec::new(),
                )
                .await
                {
                    Ok(out) => Outcome {
                        code: out.exit_code,
                        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
                        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
                    },
                    Err(error) => Outcome {
                        code: 1,
                        stdout: String::new(),
                        stderr: format!("{error}\n"),
                    },
                };
            }
            let Some((_, entry)) = self.entries.iter().find(|(n, _)| n == name) else {
                return Outcome {
                    code: 127,
                    stdout: String::new(),
                    stderr: format!("Luuna {name} is not installed."),
                };
            };
            let mut command = tokio::process::Command::new(&self.bun);
            command
                .arg(entry)
                .args(&args)
                .current_dir(cwd)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .kill_on_drop(true);
            // The component never receives the runtime's connection.
            for name in ["LUUNA_API_TOKEN", "LUUNA_URL"] {
                command.env_remove(name);
            }
            for (key, _) in std::env::vars() {
                if key.eq_ignore_ascii_case("LUUNA_API_TOKEN")
                    || key.eq_ignore_ascii_case("LUUNA_URL")
                {
                    command.env_remove(key);
                }
            }
            command.env("NO_COLOR", "1").env("FORCE_COLOR", "0");
            #[cfg(windows)]
            command.creation_flags(0x08000000);
            let mut child = match command.spawn() {
                Ok(child) => child,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    return Outcome {
                        code: 1,
                        stdout: String::new(),
                        stderr: "Bun is not on PATH; install Bun 1.4 or set LUUNA_BUN.".into(),
                    };
                }
                Err(error) => {
                    return Outcome {
                        code: 1,
                        stdout: String::new(),
                        stderr: error.to_string(),
                    }
                }
            };
            let mut stdout = child.stdout.take().unwrap();
            let mut stderr = child.stderr.take().unwrap();
            let read = async {
                let (mut out, mut err) = (Vec::new(), Vec::new());
                let _ = tokio::join!(
                    async { (&mut stdout).take(LIMIT as u64).read_to_end(&mut out).await },
                    async { (&mut stderr).take(LIMIT as u64).read_to_end(&mut err).await }
                );
                let status = child.wait().await;
                (out, err, status)
            };
            match tokio::time::timeout(TIMEOUT, read).await {
                Ok((out, err, status)) => Outcome {
                    code: status.ok().and_then(|s| s.code()).unwrap_or(1),
                    stdout: strip_ansi(&String::from_utf8_lossy(&out)),
                    stderr: strip_ansi(&String::from_utf8_lossy(&err)),
                },
                Err(_) => Outcome {
                    code: 1,
                    stdout: String::new(),
                    stderr: "The component did not finish within 60 seconds.".into(),
                },
            }
        })
    }
}

/// The agent instructions file each `onboard` writes, chosen as the components choose it.
pub fn instructions_file(repository: &Path, exists: &dyn Fn(&Path) -> bool) -> (String, bool) {
    for name in ["CLAUDE.md", ".claude/CLAUDE.md", "AGENTS.md"] {
        if exists(&repository.join(name)) {
            return (
                if cfg!(windows) {
                    name.replace('/', "\\")
                } else {
                    name.to_string()
                },
                false,
            );
        }
    }
    ("CLAUDE.md".into(), true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bun_resolves_to_the_executable_never_a_shim() {
        let exe = if cfg!(windows) { "bun.exe" } else { "bun" };
        let shim_dir = if cfg!(windows) { r"C:\npm" } else { "/npm" };
        let path_env = shim_dir.to_string();
        let env = |name: &str| (name == "PATH").then(|| path_env.clone());
        let behind = Path::new(shim_dir)
            .join("node_modules")
            .join("bun")
            .join("bin")
            .join(exe);
        let found = resolve_bun(&env, &|p: &Path| p == behind);
        assert_eq!(found, behind);
        let none = resolve_bun(&|_: &str| None, &|_: &Path| false);
        assert_eq!(none, PathBuf::from(exe));
        assert_eq!(strip_ansi("\u{1b}[32mok\u{1b}[0m"), "ok");
    }
}

#[cfg(test)]
mod native_missions_tests {
    use super::*;
    #[tokio::test]
    async fn missing_install_still_exposes_native_missions() {
        let root = tempfile::tempdir().unwrap();
        let runner = ComponentRunner::locate(root.path());
        assert!(runner.available().iter().any(|c| c.name == "missions"));
        let cwd = root.path().to_str().unwrap();
        let init = runner.run("missions", vec!["init".into()], cwd).await;
        assert_eq!(init.code, 0, "{}", init.stderr);
        let prime = runner
            .run("missions", vec!["prime".into(), "--compact".into()], cwd)
            .await;
        assert_eq!(prime.code, 0, "{}", prime.stderr);
        assert!(!prime.stdout.is_empty());
    }
}
