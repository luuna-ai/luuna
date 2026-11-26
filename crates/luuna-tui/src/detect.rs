//! Proposes test commands from a repository's own files, so the manifest step can offer
//! choices instead of asking for a command (owner feedback C2). Only fixed, top-level file
//! names are read, each bounded; nothing is executed.
use std::path::Path;

/// One proposed command and the file that suggested it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Suggestion {
    pub command: String,
    pub because: String,
}

const LIMIT: u64 = 1024 * 1024;

fn read(root: &Path, name: &str) -> Option<String> {
    let path = root.join(name);
    let metadata = std::fs::symlink_metadata(&path).ok()?;
    if !metadata.is_file() || metadata.len() > LIMIT {
        return None;
    }
    std::fs::read_to_string(path).ok()
}

fn exists(root: &Path, name: &str) -> bool {
    std::fs::symlink_metadata(root.join(name)).is_ok_and(|m| m.is_file())
}

/// A line that defines `test` as a make target or a just recipe (`test:` or `test arg:`).
fn defines_test(text: &str) -> bool {
    text.lines().any(|line| {
        let Some(rest) = line.strip_prefix("test") else {
            return false;
        };
        if !rest.starts_with([':', ' ', '\t']) {
            return false;
        }
        rest.find(':')
            .is_some_and(|colon| !rest[colon..].starts_with(":="))
    })
}

/// The Python interpreter this host runs, chosen as the runtime's discovery
/// chooses it: `python` first on Windows, otherwise `python3` when it is on
/// PATH, then `python`; `python3` (Unix) or `python` (Windows) when neither is.
/// The executor runs the exact name, with no fallback between the two.
pub fn python_executable() -> &'static str {
    let on_path = |name: &str| {
        std::env::var_os("PATH").is_some_and(|path| {
            std::env::split_paths(&path).any(|dir| {
                let file = dir.join(name);
                file.is_file() || (cfg!(windows) && file.with_extension("exe").is_file())
            })
        })
    };
    match (on_path("python3"), on_path("python")) {
        (_, true) if cfg!(windows) => "python",
        (true, _) => "python3",
        (_, true) => "python",
        _ if cfg!(windows) => "python",
        _ => "python3",
    }
}

/// Likely test commands for the repository at `root`, most specific first.
pub fn test_commands(root: &Path) -> Vec<Suggestion> {
    test_commands_with(root, python_executable())
}

/// [`test_commands`] with the Python interpreter given.
fn test_commands_with(root: &Path, python: &str) -> Vec<Suggestion> {
    let mut out: Vec<Suggestion> = Vec::new();
    let mut add = |command: &str, because: &str| {
        if !out.iter().any(|s| s.command == command) {
            out.push(Suggestion {
                command: command.into(),
                because: because.into(),
            });
        }
    };
    if let Some(text) = read(root, "package.json") {
        let script = serde_json::from_str::<serde_json::Value>(&text)
            .ok()
            .and_then(|v| v["scripts"]["test"].as_str().map(str::to_owned));
        if let Some(script) = script.filter(|s| !s.contains("no test specified")) {
            let manager = if exists(root, "pnpm-lock.yaml") {
                "pnpm"
            } else if exists(root, "yarn.lock") {
                "yarn"
            } else if exists(root, "bun.lock") || exists(root, "bun.lockb") {
                "bun run"
            } else {
                "npm"
            };
            add(
                &format!("{manager} test"),
                &format!("package.json runs `{script}`"),
            );
        }
    }
    if let Some(text) = read(root, "Cargo.toml") {
        if text.lines().any(|line| line.trim() == "[workspace]") {
            add("cargo test --workspace", "Cargo.toml declares a workspace");
        } else {
            add("cargo test", "Cargo.toml (Rust)");
        }
    }
    if exists(root, "go.mod") {
        add("go test ./...", "go.mod (Go)");
    }
    let pyproject = read(root, "pyproject.toml");
    let pytest = exists(root, "pytest.ini")
        || exists(root, "conftest.py")
        || pyproject
            .as_deref()
            .is_some_and(|t| t.contains("[tool.pytest") || t.contains("pytest"))
        || read(root, "setup.cfg").is_some_and(|t| t.contains("[tool:pytest]"))
        || read(root, "tox.ini").is_some_and(|t| t.contains("[pytest]"));
    if pytest {
        add(&format!("{python} -m pytest"), "pytest is configured");
    } else if pyproject.is_some() || exists(root, "setup.py") {
        add(
            &format!("{python} -m unittest -v"),
            "a Python project without pytest",
        );
    }
    if exists(root, "pom.xml") {
        add("mvn test", "pom.xml (Maven)");
    }
    if exists(root, "gradlew") {
        add("./gradlew test", "the Gradle wrapper");
    } else if exists(root, "build.gradle") || exists(root, "build.gradle.kts") {
        add("gradle test", "build.gradle (Gradle)");
    }
    let dotnet = std::fs::read_dir(root).ok().is_some_and(|entries| {
        entries.flatten().any(|entry| {
            let name = entry.file_name().to_string_lossy().to_lowercase();
            (name.ends_with(".sln") || name.ends_with(".csproj") || name.ends_with(".fsproj"))
                && entry.file_type().is_ok_and(|t| t.is_file())
        })
    });
    if dotnet {
        add("dotnet test", "a .NET solution or project");
    }
    if ["Makefile", "makefile", "GNUmakefile"]
        .iter()
        .filter_map(|name| read(root, name))
        .any(|text| defines_test(&text))
    {
        add("make test", "the Makefile has a test target");
    }
    if ["justfile", "Justfile", ".justfile"]
        .iter()
        .filter_map(|name| read(root, name))
        .any(|text| defines_test(&text))
    {
        add("just test", "the justfile has a test recipe");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo(files: &[(&str, &str)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for (name, text) in files {
            std::fs::write(dir.path().join(name), text).unwrap();
        }
        dir
    }

    fn commands(files: &[(&str, &str)]) -> Vec<String> {
        test_commands_with(repo(files).path(), "python")
            .into_iter()
            .map(|s| s.command)
            .collect()
    }

    #[test]
    fn python_checks_name_the_host_interpreter() {
        let dir = repo(&[("pytest.ini", "")]);
        let command = &test_commands_with(dir.path(), "python3")[0].command;
        assert_eq!(command, "python3 -m pytest");
        assert!(["python", "python3"].contains(&python_executable()));
    }

    #[test]
    fn each_ecosystem_proposes_its_usual_command() {
        assert_eq!(commands(&[("Cargo.toml", "[package]")]), ["cargo test"]);
        assert_eq!(
            commands(&[("Cargo.toml", "[workspace]\nmembers = [\"a\"]")]),
            ["cargo test --workspace"]
        );
        assert_eq!(commands(&[("go.mod", "module x")]), ["go test ./..."]);
        assert_eq!(commands(&[("pom.xml", "<project/>")]), ["mvn test"]);
        assert_eq!(commands(&[("build.gradle", "")]), ["gradle test"]);
        assert_eq!(
            commands(&[("build.gradle", ""), ("gradlew", "")]),
            ["./gradlew test"]
        );
        assert_eq!(commands(&[("App.sln", "")]), ["dotnet test"]);
        assert_eq!(commands(&[("pytest.ini", "")]), ["python -m pytest"]);
        assert_eq!(
            commands(&[("pyproject.toml", "[tool.pytest.ini_options]")]),
            ["python -m pytest"]
        );
        assert_eq!(
            commands(&[("pyproject.toml", "[project]\nname = \"x\"")]),
            ["python -m unittest -v"]
        );
        assert_eq!(
            commands(&[("Makefile", "build:\n\ttrue\ntest: build\n\ttrue\n")]),
            ["make test"]
        );
        assert_eq!(
            commands(&[("justfile", "test *args:\n    cargo test")]),
            ["just test"]
        );
        assert!(commands(&[("Makefile", "TESTS := a\nbuild:\n")]).is_empty());
        assert!(commands(&[("README.md", "# x")]).is_empty());
    }

    #[test]
    fn node_uses_the_lockfiles_manager_and_skips_the_npm_placeholder() {
        let pkg = r#"{"scripts":{"test":"vitest run"}}"#;
        assert_eq!(commands(&[("package.json", pkg)]), ["npm test"]);
        assert_eq!(
            commands(&[("package.json", pkg), ("pnpm-lock.yaml", "")]),
            ["pnpm test"]
        );
        assert_eq!(
            commands(&[("package.json", pkg), ("yarn.lock", "")]),
            ["yarn test"]
        );
        let placeholder = r#"{"scripts":{"test":"echo \"Error: no test specified\" && exit 1"}}"#;
        assert!(commands(&[("package.json", placeholder)]).is_empty());
        assert!(commands(&[("package.json", "not json")]).is_empty());
        let found = test_commands(repo(&[("package.json", pkg)]).path());
        assert_eq!(found[0].because, "package.json runs `vitest run`");
    }

    #[test]
    fn several_ecosystems_are_all_offered_once() {
        let pkg = r#"{"scripts":{"test":"jest"}}"#;
        assert_eq!(
            commands(&[
                ("package.json", pkg),
                ("Cargo.toml", ""),
                ("justfile", "test:\n  cargo test")
            ]),
            ["npm test", "cargo test", "just test"]
        );
    }

    #[cfg(unix)]
    #[test]
    fn linked_files_are_not_read() {
        let outside = repo(&[("Cargo.toml", "")]);
        let dir = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(
            outside.path().join("Cargo.toml"),
            dir.path().join("Cargo.toml"),
        )
        .unwrap();
        assert!(test_commands(dir.path()).is_empty());
    }
}
