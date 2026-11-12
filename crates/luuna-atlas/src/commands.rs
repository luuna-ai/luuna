//! The commands `init`, `add` and `config show`.
//!
//! Each command has a typed form, returning a result, and a `run_*` form, returning the
//! [`CommandOutput`] the Go tool produces for the same command with `--json --quiet`.
//! Every command takes the project root as a parameter; none runs git or reads the
//! environment to find it.
//!
//! The `run_*` forms repeat what the Go tool's `Run` does before it dispatches a command:
//! read the config (which runs the legacy migration and prints its notice on stderr),
//! build the type registry, and, if that fails, print a `Config error` envelope under the
//! command name `init`, whatever the command.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use crate::config::{self, Config, GIT_ATTRIBUTES_LINE, INITIAL_CONFIG_YAML};
use crate::error::{AtlasError, Result};
use crate::json::{Object, Value};
use crate::legacy;
use crate::output::CommandOutput;
use crate::registry::Registry;
use crate::AtlasStore;

/// The result of `init`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InitResult {
    /// Whether `.luuna/atlas/` did not exist before.
    pub created: bool,
    /// The store directory, `<project_root>/.luuna/atlas`.
    pub path: PathBuf,
}

/// The result of `add`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddResult {
    pub domain: String,
}

/// Creates the store, or fills in whatever is missing from it.
///
/// Creates `expertise/`, writes `config.yaml` and `README.md` only if absent, and appends
/// the union-merge line to `<project_root>/.gitattributes` unless the file already
/// contains it (adding a newline first if the file lacks a trailing one).
pub fn init(project_root: &Path) -> Result<InitResult> {
    let store = AtlasStore::open(project_root)?;
    let dir = store.atlas_dir();
    let created = !dir.exists();
    let expertise = dir.join("expertise");
    fs::create_dir_all(&expertise).map_err(|source| AtlasError::PathOp {
        op: "mkdir",
        path: expertise.clone(),
        source,
    })?;
    let config_path = dir.join("config.yaml");
    if !config_path.exists() {
        write_file(&config_path, INITIAL_CONFIG_YAML.as_bytes())?;
    }
    let attributes = project_root.join(".gitattributes");
    let old = fs::read(&attributes).unwrap_or_default();
    if !contains(&old, GIT_ATTRIBUTES_LINE.as_bytes()) {
        let mut next = old.clone();
        if !old.is_empty() && !old.ends_with(b"\n") {
            next.push(b'\n');
        }
        next.extend_from_slice(GIT_ATTRIBUTES_LINE.as_bytes());
        next.push(b'\n');
        write_file(&attributes, &next)?;
    }
    let readme = dir.join("README.md");
    if !readme.exists() {
        write_file(&readme, ATLAS_README.as_bytes())?;
    }
    Ok(InitResult {
        created,
        path: clean_path(&dir),
    })
}

/// Adds a domain to an existing store.
///
/// Fails if the store directory is missing, if the domain is already in `config.yaml`,
/// or if the name is invalid; otherwise adds it to the config and creates its empty
/// expertise file.
pub fn add(project_root: &Path, domain: &str) -> Result<AddResult> {
    let store = AtlasStore::open(project_root)?;
    let dir = store.atlas_dir();
    if !dir.exists() {
        return Err(AtlasError::Message(
            "No .luuna/atlas/ directory found. Run `luuna-atlas init` first.".to_owned(),
        ));
    }
    let config = config::read_config(&dir)?;
    let known = config
        .get("domains")
        .and_then(Value::as_object)
        .is_some_and(|domains| domains.contains_key(domain));
    if known {
        return Err(AtlasError::Message(format!(
            "Domain {} already exists.",
            go_quote(domain)
        )));
    }
    config::add_domain(&dir, domain)?;
    Ok(AddResult {
        domain: domain.to_owned(),
    })
}

/// The store's config with defaults merged in.
pub fn config_show(project_root: &Path) -> Result<Config> {
    let store = AtlasStore::open(project_root)?;
    config::read_config(&store.atlas_dir())
}

/// `luuna-atlas init --json --quiet`.
pub fn run_init(project_root: &Path) -> CommandOutput {
    let (notice, early) = prelude(project_root);
    let output = match early {
        Some(output) => output,
        None => match init(project_root) {
            Ok(result) => {
                let mut fields = Object::new();
                fields.insert("created", result.created);
                fields.insert("path", result.path.to_string_lossy().into_owned());
                CommandOutput::success("init", fields, &["created", "path"])
            }
            Err(error) => CommandOutput::failure("init", &error.to_string()),
        },
    };
    with_notice(notice, output)
}

/// `luuna-atlas add <domain> --json --quiet`.
pub fn run_add(project_root: &Path, domain: &str) -> CommandOutput {
    let (notice, early) = prelude(project_root);
    let output = match early {
        Some(output) => output,
        None => match add(project_root, domain) {
            Ok(result) => {
                let mut fields = Object::new();
                fields.insert("domain", result.domain);
                CommandOutput::success("add", fields, &["domain"])
            }
            Err(error) => CommandOutput::failure("add", &error.to_string()),
        },
    };
    with_notice(notice, output)
}

/// `luuna-atlas config show --json --quiet`.
pub fn run_config_show(project_root: &Path) -> CommandOutput {
    let (notice, early) = prelude(project_root);
    let output = match early {
        Some(output) => output,
        None => match config_show(project_root) {
            Ok(config) => CommandOutput::emit(&config),
            // The Go tool prints this one as plain text, even with --json.
            Err(error) => CommandOutput::plain_failure(&error.to_string()),
        },
    };
    with_notice(notice, output)
}

/// What the Go tool's `Run` does before dispatching: reading the config migrates a
/// legacy store (its failure is ignored here and shows again when the command opens the
/// store) and builds the registry, whose failure ends the run.
pub(crate) fn prelude(project_root: &Path) -> (Option<String>, Option<CommandOutput>) {
    let (notice, config) = match legacy::migrate_legacy_store(project_root) {
        Ok(notice) => {
            let dir = project_root.join(".luuna").join("atlas");
            (notice, config::read_config(&dir).ok())
        }
        Err(_) => (None, None),
    };
    let early = Registry::from_config(config.as_ref())
        .err()
        .map(|error| CommandOutput::failure("init", &format!("Config error: {error}")));
    (notice, early)
}

pub(crate) fn with_notice(notice: Option<String>, output: CommandOutput) -> CommandOutput {
    match notice {
        Some(text) => output.with_stderr_prefix(&text),
        None => output,
    }
}

/// Go's `filepath.Clean` (what `filepath.Join` applies), done on the text alone: runs of
/// separators collapse, `.` elements go, `..` removes the element before it (and is
/// dropped at the root), and an empty result is `.`. Nothing is read from the disk and
/// symbolic links are not resolved.
pub(crate) fn clean_path(path: &Path) -> PathBuf {
    let text = path.to_string_lossy();
    let rooted = text.starts_with('/');
    let mut parts: Vec<&str> = Vec::new();
    for part in text.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                if parts.last().is_some_and(|last| *last != "..") {
                    parts.pop();
                } else if !rooted {
                    parts.push("..");
                }
            }
            other => parts.push(other),
        }
    }
    let joined = parts.join("/");
    PathBuf::from(match (rooted, joined.is_empty()) {
        (true, _) => format!("/{joined}"),
        (false, true) => ".".to_owned(),
        (false, false) => joined,
    })
}

/// Go's `os.WriteFile(path, data, 0666)`.
pub(crate) fn write_file(path: &Path, data: &[u8]) -> Result<()> {
    let fail = |op: &'static str| {
        move |source: io::Error| AtlasError::PathOp {
            op,
            path: path.to_path_buf(),
            source,
        }
    };
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)
        .map_err(fail("open"))?;
    file.write_all(data).map_err(fail("write"))
}

/// `strings.Contains` on bytes.
fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

/// Go's `%q`: a double-quoted string with Go escapes.
pub(crate) fn go_quote(text: &str) -> String {
    let mut out = String::from("\"");
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{7}' => out.push_str("\\a"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{b}' => out.push_str("\\v"),
            c if c.is_control() => {
                let code = c as u32;
                if code < 0x80 {
                    out.push_str(&format!("\\x{code:02x}"));
                } else if code < 0x10000 {
                    out.push_str(&format!("\\u{code:04x}"));
                } else {
                    out.push_str(&format!("\\U{code:08x}"));
                }
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

const ATLAS_README: &str = r##"# .luuna/atlas/

This directory is managed by [luuna-atlas](https://github.com/luuna-ai/luuna-atlas) — a structured expertise layer for coding agents.

## Key Commands

- `luuna-atlas init`      — Initialize a .luuna/atlas directory
- `luuna-atlas add`       — Add a new domain
- `luuna-atlas record`    — Record an expertise record
- `luuna-atlas edit`      — Edit an existing record
- `luuna-atlas query`     — Query expertise records
- `luuna-atlas prime [domain]` — Output a record index for priming (optionally scoped to one domain)
- `luuna-atlas show <id>`  — Show a full record
- `luuna-atlas search`   — Search records across domains
- `luuna-atlas status`    — Show domain statistics
- `luuna-atlas validate`  — Validate all records against the schema
- `luuna-atlas prune`     — Remove expired records

## Structure

- `config.yaml` — Configuration file
- `expertise/`        — JSONL files, one per domain

## Configuration

Optional knobs in `config.yaml`:

```yaml
prime:
  default_mode: manifest   # or "full" (the default): a budget-capped record
                           # index, failures first. "manifest" lists per-domain
                           # counts instead. Scoping flags (`--files`,
                           # `<domain>`) always emit records.

search:
  boost_factor: 0.1        # multiplier on BM25 scores for confirmed records.
                           # 0 disables (pure BM25). Override with
                           # `luuna-atlas search --no-boost`.
```
"##;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::json;

    #[test]
    fn clean_path_is_lexical_like_filepath_clean() {
        let cases = [
            ("/a/b/.luuna/atlas", "/a/b/.luuna/atlas"),
            ("/a//b/./c/", "/a/b/c"),
            ("/a/b/../c", "/a/c"),
            ("/../a", "/a"),
            ("/", "/"),
            ("", "."),
            (".", "."),
            ("a/..", "."),
            ("../a/../../b", "../../b"),
            ("a/./b//", "a/b"),
        ];
        for (input, want) in cases {
            assert_eq!(clean_path(Path::new(input)), Path::new(want), "{input}");
        }
    }

    #[test]
    fn init_reports_a_cleaned_path_without_resolving_links() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("proj");
        fs::create_dir_all(root.join("sub")).expect("mkdir");
        let messy = root.join("sub").join("..").join(".").join("");
        let result = init(&messy).expect("init");
        assert_eq!(result.path, root.join(".luuna").join("atlas"));
        let output = run_init(&root.join("x").join("..").join("."));
        assert!(
            output.stdout.contains(&format!(
                "\"path\": {}",
                json::to_compact_string(&Value::from(
                    root.join(".luuna/atlas").to_string_lossy().into_owned()
                ))
                .expect("json")
            )),
            "{}",
            output.stdout
        );
    }

    #[test]
    fn config_show_orders_a_config_with_its_own_command_key() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path();
        init(root).expect("init");
        let path = root.join(".luuna/atlas/config.yaml");
        let mut yaml = fs::read_to_string(&path).expect("read");
        yaml.push_str("command: init\npath: p\nzzz: 1\n");
        fs::write(&path, yaml).expect("write");
        let output = run_config_show(root);
        assert_eq!(output.exit_code, 0, "{}", output.stderr);
        let value = json::parse(&output.stdout).expect("json");
        let keys: Vec<&str> = value.as_object().expect("object").keys().collect();
        let command = keys.iter().position(|k| *k == "command").expect("command");
        let path = keys.iter().position(|k| *k == "path").expect("path");
        assert_eq!(command, 0, "{keys:?}");
        assert_eq!(path, 1, "{keys:?}");
        let rest = &keys[2..];
        let mut sorted = rest.to_vec();
        sorted.sort_unstable();
        assert_eq!(rest, sorted.as_slice(), "{keys:?}");
    }
}
