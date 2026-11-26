//! Native component commands for the terminal guide.
//! Atlas, Beacon and Missions use compiled Rust libraries.
//! All command environments exclude the runtime connection URL and token.
use std::path::{Path, PathBuf};

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
pub struct ComponentRunner;
impl ComponentRunner {
    pub fn locate(_root: &Path) -> ComponentRunner {
        ComponentRunner
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
        COMPONENTS.to_vec()
    }
    fn run<'a>(
        &'a self,
        name: &'a str,
        args: Vec<String>,
        cwd: &'a str,
    ) -> crate::commands::Fut<'a, Outcome> {
        Box::pin(async move {
            let env = std::env::vars()
                .filter(|(k, _)| {
                    !k.eq_ignore_ascii_case("LUUNA_API_TOKEN")
                        && !k.eq_ignore_ascii_case("LUUNA_URL")
                })
                .chain([
                    ("NO_COLOR".into(), "1".into()),
                    ("FORCE_COLOR".into(), "0".into()),
                ])
                .collect();
            let result = match name {
                "missions" => {
                    luuna_components::run_missions_native(Path::new(cwd), &args, env, Vec::new())
                        .await
                        .map(|o| (o.exit_code, o.stdout, o.stderr))
                }
                "atlas" => {
                    luuna_components::run_atlas_native(Path::new(cwd), &args, env, Vec::new())
                        .await
                        .map(|o| (o.exit_code, o.stdout.into_bytes(), o.stderr.into_bytes()))
                }
                "beacon" => {
                    luuna_components::run_beacon_native(Path::new(cwd), &args, env, Vec::new())
                        .await
                        .map(|o| (o.exit_code, o.stdout, o.stderr))
                }
                _ => {
                    return Outcome {
                        code: 127,
                        stdout: String::new(),
                        stderr: format!("Unknown component: {name}"),
                    }
                }
            };
            match result {
                Ok((code, stdout, stderr)) => Outcome {
                    code,
                    stdout: strip_ansi(&String::from_utf8_lossy(&stdout)),
                    stderr: strip_ansi(&String::from_utf8_lossy(&stderr)),
                },
                Err(error) => Outcome {
                    code: 1,
                    stdout: String::new(),
                    stderr: format!("{error}\n"),
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
    fn ansi_colour_codes_are_stripped() {
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

#[cfg(test)]
mod native_all_tests {
    use super::*;
    #[tokio::test]
    async fn empty_install_runs_every_native_component() {
        let dir = tempfile::tempdir().unwrap();
        let runner = ComponentRunner::locate(dir.path());
        assert_eq!(runner.available().len(), 3);
        for name in ["atlas", "beacon", "missions"] {
            let result = runner
                .run(name, vec!["init".into()], dir.path().to_str().unwrap())
                .await;
            assert_eq!(result.code, 0, "{name}: {}", result.stderr);
            assert!(dir
                .path()
                .join(format!(".luuna/{name}/config.yaml"))
                .is_file());
        }
    }
}

/// Historical names emitted by component hooks and completions.
pub fn component_alias(name: &str) -> Option<&'static str> {
    #[cfg(windows)]
    let normalized = name.to_ascii_lowercase();
    #[cfg(windows)]
    let name = normalized.as_str();
    match name {
        "luuna-atlas" | "atlas" | "lat" | "la" | "mulch" => Some("atlas"),
        "luuna-beacon" | "beacon" | "lbc" | "lb" | "canopy" => Some("beacon"),
        "luuna-missions" | "missions" | "lms" | "lm" | "seeds" => Some("missions"),
        _ => None,
    }
}

/// Native CLI streams preserve bytes, stderr and exit status, including piped stdin.
pub async fn dispatch(name: &str, args: &[String]) -> i32 {
    use std::io::{IsTerminal, Read, Write};
    let result = async {
        let mut input = Vec::new();
        if !std::io::stdin().is_terminal() {
            std::io::stdin()
                .take(4 * 1024 * 1024 + 1)
                .read_to_end(&mut input)?;
        }
        let cwd = std::env::current_dir()?;
        let env = std::env::vars()
            .filter(|(k, _)| {
                !k.eq_ignore_ascii_case("LUUNA_API_TOKEN") && !k.eq_ignore_ascii_case("LUUNA_URL")
            })
            .collect();
        let args = if args.is_empty() {
            vec!["--help".into()]
        } else {
            args.to_vec()
        };
        let out = match name {
            "atlas" => luuna_components::run_atlas_native(&cwd, &args, env, input)
                .await
                .map(|o| (o.exit_code, o.stdout.into_bytes(), o.stderr.into_bytes())),
            "beacon" => luuna_components::run_beacon_native(&cwd, &args, env, input)
                .await
                .map(|o| (o.exit_code, o.stdout, o.stderr)),
            "missions" => luuna_components::run_missions_native(&cwd, &args, env, input)
                .await
                .map(|o| (o.exit_code, o.stdout, o.stderr)),
            _ => return Err(std::io::Error::other("Unknown component")),
        }
        .map_err(std::io::Error::other)?;
        std::io::stdout().write_all(&out.1)?;
        std::io::stderr().write_all(&out.2)?;
        Ok::<_, std::io::Error>(out.0)
    }
    .await;
    match result {
        Ok(code) => code,
        Err(error) => {
            eprintln!("Luuna component: {error}");
            1
        }
    }
}
