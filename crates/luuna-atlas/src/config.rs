//! config.yaml: reading with the Go tool's defaults, writing, and adding domains.
//!
//! Every function takes the store's `.luuna/atlas` directory as a parameter; finding the
//! store (and the legacy `.mulch/` migration the Go tool runs on the way) is the job of
//! [`crate::AtlasStore::open`]. The functions follow `ReadConfig`, `WriteConfig`,
//! `AddDomain` and `ValidateDomainName` in the Go tool's `config.go`.

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::Path;

use crate::error::{AtlasError, Result};
use crate::json::{Object, Value};
use crate::yaml;

/// A parsed config.yaml, keys in file order.
pub type Config = Object;

/// The line `init` adds to the project's `.gitattributes`.
pub const GIT_ATTRIBUTES_LINE: &str = ".luuna/atlas/expertise/*.jsonl merge=union";

/// The file name of the config inside the store directory.
const CONFIG_FILE: &str = "config.yaml";

/// The config.yaml that `init` writes, comments included: the Go tool's
/// `InitialConfigYAML`. Any command that rewrites the config drops the comments.
pub const INITIAL_CONFIG_YAML: &str = r##"# Atlas configuration. See https://github.com/luuna-ai/luuna-atlas for docs.
#
# Required fields below. Optional knobs are commented out at the bottom — uncomment
# to enable. Note: commands that mutate this file (e.g. `luuna-atlas add <domain>`,
# `luuna-atlas record` auto-creating a domain) rewrite it via the YAML serializer and
# strip these comments.

version: '1'
domains: {}
governance:
  max_entries: 100
  warn_entries: 150
  hard_limit: 200
classification_defaults:
  shelf_life:
    tactical: 14
    observational: 30

# ─── Optional knobs (uncomment to enable) ────────────────────────────────────
#
# prime:
#   # Pin `luuna-atlas prime`'s unscoped output shape. When unset (recommended), prime
#   # emits a budget-capped record index (one line per record, failures first).
#   #   - full     → the record index (same as unset)
#   #   - manifest → a per-domain count table instead of records
#   # --full / --manifest / scoping flags always override this on a per-call basis.
#   default_mode: full        # one of: full, manifest
#   # Trust-tier ranking weights for full-mode output. Sort score per record =
#   # (★ count * star) + classification weight. Override only the knobs you
#   # want to retune; unset fields keep their default. Higher scores surface
#   # first; within-tier ties preserve insertion order.
#   tier_weights:
#     star: 100               # multiplier on the ★ confirmation count
#     foundational: 50        # base score for foundational records
#     tactical: 20             # base score for tactical records
#     observational: 10        # base score for observational records
#
# search:
#   # Multiplier applied to BM25 scores so records with more confirmed outcomes
#   # rank higher. 0 disables (pure BM25). Default 0.1: a record with N successes
#   # gets a (1 + 0.1*N) boost. Override per-call with `luuna-atlas search --no-boost`.
#   boost_factor: 0.1
#
# disabled_types:
#   # Names of registered types (built-in or custom) that emit a deprecation
#   # warning on write. Reads still succeed; the type stays in CLI choices so
#   # peers in shared domains don't hard-fail. Use to retire a type gracefully.
#   - failure
#
# custom_types:
#   # Project-specific record types. Required + optional fields, dedup_key,
#   # and a summary template. See README for the full schema.
#   hypothesis:
#     required: [statement, prediction]
#     optional: [evidence_files]
#     dedup_key: statement
#     summary: "{statement}"   # tokens use single braces; {{statement}} also accepted
#     # aliases: rename a field while still reading legacy on-disk records.
#     # Map canonical (current) name → list of legacy names. At read time,
#     # legacy fields are rewritten to canonical and dropped from the record.
#     aliases:
#       statement: [claim]
#   # Inherit from a built-in type with extends:. Required/optional arrays
#   # merge with the parent's (union); dedup_key, summary, compact, section_title,
#   # extracts_files, and files_field override only when set, otherwise inherit.
#   # Custom-from-custom is not supported in v1.
#   adr:
#     extends: decision
#     required: [decision_status, deciders]   # added on top of decision's [title, rationale]
#     summary: "{decision_status}: {title}"   # tokens must be declared fields (parent's included)
#
# quality:
#   # Write-time gate on luuna-atlas record: near-duplicates, conventions/patterns with
#   # no file/dir anchor or evidence, conventions that restate code.
#   # warn (default) prints and writes; error blocks unless --force; off skips.
#   # Exact duplicates are always blocked.
#   level: warn
#
# hooks:
#   # Lifecycle hook scripts. Each event maps to an ordered list of shell
#   # commands. Atlas invokes each with the relevant payload as JSON on stdin.
#   # Exit 0 = continue. Non-zero from a `pre-*` hook blocks the action; from
#   # a `post-*` hook emits a warning. `pre-record`, `pre-prime`, and
#   # `pre-compact` may mutate the payload by printing modified JSON on
#   # stdout; `pre-prune` is block-or-allow only — its stdout is ignored.
#   #
#   # pre-record:    [./.luuna/atlas/hooks/scan-secrets.sh, ./.luuna/atlas/hooks/require-owner.sh]
#   # post-record:   [./.luuna/atlas/hooks/post-to-slack.sh]
#   # pre-prime:     [./.luuna/atlas/hooks/filter-by-team.sh]
#   # pre-prune:     [./.luuna/atlas/hooks/digest-then-confirm.sh]
#   # pre-compact:   [./.luuna/atlas/hooks/llm-summarize.sh]
#
# hook_settings:
#   # Per-hook execution timeout in milliseconds. Default 5000.
#   timeout_ms: 5000
"##;

/// The error when the store has no config.yaml.
const NO_STORE: &str =
    "No .luuna/atlas/ directory found. Run `luuna-atlas init` to set up this project.";

/// Refuses a domain name that does not match `^[a-zA-Z0-9][a-zA-Z0-9_-]*$`, with the Go
/// tool's message.
pub fn validate_domain_name(domain: &str) -> Result<()> {
    let mut chars = domain.chars();
    let valid = chars.next().is_some_and(|c| c.is_ascii_alphanumeric())
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    if valid {
        Ok(())
    } else {
        Err(AtlasError::Message(format!(
            "Invalid domain name: \"{domain}\". Only alphanumeric characters, hyphens, and underscores are allowed."
        )))
    }
}

fn default_governance() -> Object {
    let mut governance = Object::new();
    governance.insert("max_entries", 100.0);
    governance.insert("warn_entries", 150.0);
    governance.insert("hard_limit", 200.0);
    governance
}

fn default_shelf_life() -> Object {
    let mut shelf_life = Object::new();
    shelf_life.insert("tactical", 14.0);
    shelf_life.insert("observational", 30.0);
    shelf_life
}

/// Reads `<atlas_dir>/config.yaml` and merges the Go tool's defaults into it.
///
/// As in the Go tool: `domains` is always present (a list of names becomes a map from
/// each string name to `{}`, anything else that is not a map becomes `{}`); `governance`
/// and `classification_defaults.shelf_life` are the defaults with the file's keys laid
/// over them, defaults first; `classification_defaults` keeps its other keys; keys the
/// file lacks are added after the file's own keys, in the order domains, governance,
/// classification_defaults. `version` is not added.
///
/// A missing file is the Go tool's "No .luuna/atlas/ directory found" error. YAML that
/// cannot be read is the Go tool's "Failed to parse config.yaml" error, except YAML that
/// the crate does not support, which is [`AtlasError::UnsupportedYaml`].
pub fn read_config(atlas_dir: &Path) -> Result<Config> {
    let path = atlas_dir.join(CONFIG_FILE);
    let mut file = match File::open(&path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err(AtlasError::Message(NO_STORE.to_owned()));
        }
        Err(source) => return Err(path_error("open", &path, source)),
    };
    let mut data = Vec::new();
    file.read_to_end(&mut data)
        .map_err(|source| path_error("read", &path, source))?;
    let parsed = match String::from_utf8(data) {
        Ok(text) => yaml::parse(&text),
        Err(_) => Err(AtlasError::Message(
            "yaml: input error: invalid UTF-8".to_owned(),
        )),
    };
    let mut parsed = parsed.map_err(|error| match error {
        AtlasError::Message(message) => AtlasError::Message(format!(
            "Failed to parse config.yaml: {message}. Check the YAML syntax."
        )),
        other => other,
    })?;
    let mut config = match &mut parsed {
        Value::Object(object) => std::mem::take(object),
        _ => Object::new(),
    };
    merge_defaults(&mut config);
    Ok(config)
}

/// The defaults part of the Go tool's `ReadConfig`.
fn merge_defaults(config: &mut Config) {
    let domains = match config.get("domains") {
        Some(Value::Array(items)) => {
            let mut domains = Object::new();
            for item in items {
                if let Value::String(name) = item {
                    domains.insert(name.as_str(), Object::new());
                }
            }
            domains
        }
        Some(Value::Object(domains)) => domains.clone(),
        _ => Object::new(),
    };
    config.insert("domains", domains);

    let mut governance = default_governance();
    if let Some(Value::Object(user)) = config.get("governance") {
        for (key, value) in user.iter() {
            governance.insert(key, value.clone());
        }
    }
    config.insert("governance", governance);

    let user_classification = match config.get("classification_defaults") {
        Some(Value::Object(user)) => user.clone(),
        _ => Object::new(),
    };
    let mut shelf_life = default_shelf_life();
    if let Some(Value::Object(user)) = user_classification.get("shelf_life") {
        for (key, value) in user.iter() {
            shelf_life.insert(key, value.clone());
        }
    }
    let mut classification = Object::new();
    classification.insert("shelf_life", Object::new());
    for (key, value) in user_classification.iter() {
        classification.insert(key, value.clone());
    }
    classification.insert("shelf_life", shelf_life);
    config.insert("classification_defaults", classification);
}

/// Rewrites `<atlas_dir>/config.yaml` from `config` (not atomically, as in the Go tool).
pub fn write_config(atlas_dir: &Path, config: &Config) -> Result<()> {
    let text = yaml::to_yaml_string(&Value::Object(config.clone()))?;
    write_file(&atlas_dir.join(CONFIG_FILE), text.as_bytes())
}

/// Adds `domain` to config.yaml if absent and creates its empty expertise file if absent.
///
/// The name is checked first. The config is rewritten only when the domain is new; the
/// file `<atlas_dir>/expertise/<domain>.jsonl` is created (zero bytes) only when nothing
/// exists at that path. The expertise directory is not created.
pub fn add_domain(atlas_dir: &Path, domain: &str) -> Result<()> {
    validate_domain_name(domain)?;
    let mut config = read_config(atlas_dir)?;
    let is_new = !config
        .get("domains")
        .and_then(Value::as_object)
        .is_some_and(|domains| domains.contains_key(domain));
    if is_new {
        if let Some(domains) = config.get_mut("domains").and_then(Value::as_object_mut) {
            domains.insert(domain, Object::new());
        }
        write_config(atlas_dir, &config)?;
    }
    let path = atlas_dir.join("expertise").join(format!("{domain}.jsonl"));
    if !path.exists() {
        write_file(&path, &[])?;
    }
    Ok(())
}

/// Go's `os.WriteFile(path, data, 0666)`: create or truncate, then write.
fn write_file(path: &Path, data: &[u8]) -> Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)
        .map_err(|source| path_error("open", path, source))?;
    file.write_all(data)
        .map_err(|source| path_error("write", path, source))
}

fn path_error(op: &'static str, path: &Path, source: io::Error) -> AtlasError {
    AtlasError::PathOp {
        op,
        path: path.to_path_buf(),
        source,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::json;

    // Expected values below were produced by the Go tool's ReadConfig and WriteConfig at
    // the reference commit, built offline from the reference clone.

    fn store(config: Option<&str>) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("expertise")).unwrap();
        if let Some(config) = config {
            std::fs::write(dir.path().join(CONFIG_FILE), config).unwrap();
        }
        dir
    }

    fn read_json(config: &str) -> String {
        let dir = store(Some(config));
        let config = read_config(dir.path()).unwrap();
        json::to_compact_string(&Value::Object(config)).unwrap()
    }

    const DEFAULT_TAIL: &str = r#""governance":{"max_entries":100,"warn_entries":150,"hard_limit":200},"classification_defaults":{"shelf_life":{"tactical":14,"observational":30}}"#;

    #[test]
    fn defaults_merge_in_go_order() {
        // User keys lie over the defaults, which keep their places; new user keys follow;
        // keys the file lacks come after the file's own keys. `version` is not added.
        assert_eq!(
            read_json(
                "classification_defaults:\n  extra: 1\n  shelf_life:\n    observational: 3\n    custom: 9\ngovernance:\n  hard_limit: 7\n  zz: 1\n  \"0\": x\nversion: 2\n"
            ),
            r#"{"classification_defaults":{"shelf_life":{"tactical":14,"observational":3,"custom":9},"extra":1},"governance":{"0":"x","max_entries":100,"warn_entries":150,"hard_limit":7,"zz":1},"version":2,"domains":{}}"#
        );
        assert_eq!(
            read_json(""),
            format!(r#"{{"domains":{{}},{DEFAULT_TAIL}}}"#)
        );
        assert_eq!(
            read_json("- a\n"),
            format!(r#"{{"domains":{{}},{DEFAULT_TAIL}}}"#)
        );
        assert_eq!(
            read_json("classification_defaults:\n  shelf_life: 5\n"),
            format!(
                r#"{{"classification_defaults":{{"shelf_life":{{"tactical":14,"observational":30}}}},"domains":{{}},"governance":{{"max_entries":100,"warn_entries":150,"hard_limit":200}}}}"#
            )
        );
        assert_eq!(
            read_json(INITIAL_CONFIG_YAML),
            format!(r#"{{"version":"1","domains":{{}},{DEFAULT_TAIL}}}"#)
        );
    }

    #[test]
    fn domains_list_becomes_map() {
        assert_eq!(
            read_json("z: 1\ndomains:\n  - b\n  - 5\n  - a\n  - b\ngovernance: 5\n"),
            format!(r#"{{"z":1,"domains":{{"b":{{}},"a":{{}}}},{DEFAULT_TAIL}}}"#)
        );
        // Only string entries count; a timestamp-like entry is a string in the Go tool.
        assert_eq!(
            read_json("domains:\n  - 2025-01-01\n  - '7'\n  - 7\n  - 10\n  - 2\n"),
            format!(r#"{{"domains":{{"7":{{}},"2025-01-01T00:00:00.000Z":{{}}}},{DEFAULT_TAIL}}}"#)
        );
        // A map stays as it is; anything else becomes {}.
        assert_eq!(
            read_json("domains:\n  x:\n    allowed_types: [a, b]\n  y: {}\n"),
            format!(
                r#"{{"domains":{{"x":{{"allowed_types":["a","b"]}},"y":{{}}}},{DEFAULT_TAIL}}}"#
            )
        );
        assert_eq!(
            read_json("domains: abc\n"),
            format!(r#"{{"domains":{{}},{DEFAULT_TAIL}}}"#)
        );
    }

    #[test]
    fn invalid_domain_name_is_refused() {
        for name in ["a", "A-1_b", "9", "z9-"] {
            assert!(validate_domain_name(name).is_ok(), "{name:?}");
        }
        for name in ["", "-a", "_a", "a b", "a/b", "../x", "a.b", "é", "a\n"] {
            assert!(validate_domain_name(name).is_err(), "{name:?}");
        }
        assert_eq!(
            validate_domain_name("../x").unwrap_err().to_string(),
            "Invalid domain name: \"../x\". Only alphanumeric characters, hyphens, and underscores are allowed."
        );
        // add_domain checks the name before touching anything.
        let dir = store(Some(INITIAL_CONFIG_YAML));
        assert!(add_domain(dir.path(), "../escape").is_err());
        assert_eq!(
            std::fs::read_to_string(dir.path().join(CONFIG_FILE)).unwrap(),
            INITIAL_CONFIG_YAML
        );
        assert!(!dir.path().join("escape.jsonl").exists());
    }

    #[test]
    fn add_domain_rewrites_config_and_creates_empty_file() {
        let dir = store(Some(INITIAL_CONFIG_YAML));
        add_domain(dir.path(), "first").unwrap();
        add_domain(dir.path(), "second").unwrap();
        let written = std::fs::read_to_string(dir.path().join(CONFIG_FILE)).unwrap();
        assert_eq!(
            written,
            "version: '1'\ndomains:\n  first: {}\n  second: {}\ngovernance:\n  max_entries: 100\n  warn_entries: 150\n  hard_limit: 200\nclassification_defaults:\n  shelf_life:\n    tactical: 14\n    observational: 30\n"
        );
        let file = dir.path().join("expertise").join("first.jsonl");
        assert_eq!(std::fs::read(&file).unwrap(), b"");
        // An existing domain and an existing file are left alone.
        std::fs::write(&file, "{}\n").unwrap();
        add_domain(dir.path(), "first").unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "{}\n");
        assert_eq!(
            std::fs::read_to_string(dir.path().join(CONFIG_FILE)).unwrap(),
            written
        );
    }

    #[test]
    fn read_config_errors_are_the_go_tool_errors() {
        let dir = store(None);
        assert_eq!(
            read_config(dir.path()).unwrap_err().to_string(),
            "No .luuna/atlas/ directory found. Run `luuna-atlas init` to set up this project."
        );
        let dir = store(Some("a: 1\na: 2\n"));
        assert_eq!(
            read_config(dir.path()).unwrap_err().to_string(),
            "Failed to parse config.yaml: duplicated mapping key (2:1)\n\n 1 | a: 1\n 2 | a: 2\n-----^. Check the YAML syntax."
        );
        let dir = store(Some("a: *x\n"));
        assert!(matches!(
            read_config(dir.path()),
            Err(AtlasError::UnsupportedYaml { line: 1, .. })
        ));
    }

    #[test]
    fn write_config_round_trips() {
        let text = "version: '1'\ndomains:\n  a: {}\ncustom_types:\n  hypothesis:\n    required:\n      - statement\n    summary: '{statement}'\n    section_title: 'Hypotheses: open'\ngovernance:\n  max_entries: 50\n  warn_entries: 150\n  hard_limit: 200\nclassification_defaults:\n  shelf_life:\n    tactical: 14\n    observational: 30\nsearch:\n  boost_factor: 0.25\ndisabled_types:\n  - failure\n";
        let dir = store(Some(text));
        let config = read_config(dir.path()).unwrap();
        write_config(dir.path(), &config).unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join(CONFIG_FILE)).unwrap(),
            text
        );
    }

    #[test]
    fn too_deep_config_is_the_go_error() {
        // On the default test thread: the reader returns yaml.v3's depth error, wrapped
        // as the Go tool wraps it, instead of overflowing the stack.
        let deep = |n: usize| {
            format!(
                "version: '1'\ndomains: {}{}\n",
                "[".repeat(n),
                "]".repeat(n)
            )
        };
        let dir = store(Some(&deep(10_001)));
        match read_config(dir.path()) {
            Err(AtlasError::Message(message)) => assert_eq!(
                message,
                "Failed to parse config.yaml: yaml: line 2: exceeded max depth of 10000. Check the YAML syntax."
            ),
            other => panic!("expected the depth error, got {:?}", other.map(|_| ())),
        }
        // As deep as yaml.v3 allows reads; a list of non-strings gives no domains.
        assert_eq!(
            read_json(&deep(10_000)),
            format!(r#"{{"version":"1","domains":{{}},{DEFAULT_TAIL}}}"#)
        );
        let maps = format!(
            "domains: {}{{}}{}\n",
            "{a: ".repeat(9_999),
            "}".repeat(9_999)
        );
        let config = read_json(&maps);
        assert!(
            config.starts_with(r#"{"domains":{"a":{"a":"#),
            "{}",
            &config[..40]
        );
    }
}
