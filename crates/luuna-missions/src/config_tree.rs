//! The full `config.yaml` and the legacy `.seeds` migration, owned by
//! s4-missions-lib-3 (`internal/config/tree.go`, `internal/config/legacy.go`).
//!
//! [`read_outside_subset`] reads any `config.yaml` as Go's `ReadConfig`
//! does, through the YAML reader in [`yaml`], which every `config.yaml`
//! reader of this crate uses. [`migrate_legacy_layout`] still refuses with
//! [`Error::Unsupported`] naming s4-missions-lib-3, and
//! [`with_stderr_notices`] runs the command unchanged. s4-missions-lib-3
//! replaces their bodies and the `checks` it owns, and edits neither
//! `config.rs` nor `command/init.rs`.

use std::path::Path;

use crate::command::Outcome;
use crate::config::{Config, LEGACY_DIR};
use crate::error::{Error, Result, PLAN_3};

pub(crate) mod yaml;

/// A value of `config.yaml` as yaml.v3 decodes it into `any`
/// ([`yaml::decode_document`]). Built at most 1000 levels deep, so every
/// function that walks one may recurse.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum TreeValue {
    /// `nil`.
    Null,
    /// A `bool`.
    Bool(bool),
    /// An `int`.
    Int(i64),
    /// A `uint64`: an integer above `int`'s range.
    Uint(u64),
    /// A `float64`.
    Float(f64),
    /// A `string`.
    Str(String),
    /// A `[]any`.
    Seq(Vec<TreeValue>),
    /// A `map[string]any`, in file order (a merged key after the mapping's
    /// own keys); every key once.
    Map(Vec<(String, TreeValue)>),
    /// A value Go decodes into a type its encoders refuse, named as Go names
    /// it: a mapping with a key that is not a string,
    /// `map[interface {}]interface {}`.
    Other(&'static str),
}

/// Runs one command and adds to its stderr the notices Go prints to
/// `os.Stderr` while the command runs (such as the line of the legacy
/// migration, which Go's `FindDir` prints for every command). No notice is
/// produced at this commit, so the command's result is returned unchanged.
pub(crate) fn with_stderr_notices(run: impl FnOnce() -> Result<Outcome>) -> Result<Outcome> {
    run()
}

/// Go's `ReadConfig` (`internal/config/config.go:174-194`) for the
/// `content` of the `config.yaml` at `path`, which the foundation's subset
/// reader does not read (`_reason` says why). `project` and `version` are
/// taken only when they are strings, `max_plan_depth` only when it is an
/// int or a float with an integer value; legacy keys are not upgraded. An
/// empty file, or a null first document, gives the defaults.
pub(crate) fn read_outside_subset(path: &Path, content: &[u8], _reason: String) -> Result<Config> {
    let decoded = yaml::decode_document(content).map_err(|error| yaml::in_file(path, error))?;
    let mut config = Config::default();
    let Some(TreeValue::Map(entries)) = decoded else {
        return Ok(config);
    };
    let get = |name: &str| {
        entries
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value)
    };
    if let Some(TreeValue::Str(project)) = get("project") {
        config.project = project.clone();
    }
    if let Some(TreeValue::Str(version)) = get("version") {
        config.version = version.clone();
    }
    config.max_plan_depth = match get("max_plan_depth") {
        Some(TreeValue::Int(depth)) => Some(*depth),
        // Go's `integer`: a float64 with an integer value converts.
        Some(TreeValue::Float(depth))
            if depth.fract() == 0.0
                && (-9.223_372_036_854_776e18..9.223_372_036_854_776e18).contains(depth) =>
        {
            Some(*depth as i64)
        }
        _ => None,
    };
    Ok(config)
}

/// Go's `MigrateLegacyLayout` for the `.seeds` entry `legacy` in
/// `project_root`, in every case where Go migrates it or refuses. Refused
/// until s4-missions-lib-3.
pub(crate) fn migrate_legacy_layout(_project_root: &Path, legacy: &Path) -> Result<()> {
    Err(Error::unsupported(
        format!(
            "the legacy {LEGACY_DIR}/ layout (found {})",
            legacy.display()
        ),
        PLAN_3,
    ))
}

/// The expectations of the unit tests in `config.rs` and `command/init.rs`
/// for what this module ports. Their inputs stay in those tests. While the
/// behaviour is deferred, each function asserts the refusal exactly as the
/// foundation made it; once s4-missions-lib-3 ports it, Go's exact result
/// for the same input, as literals.
#[cfg(test)]
pub(crate) mod checks {
    use std::fs;
    use std::path::Path;

    use crate::command::{run, Invocation};
    use crate::config::{find_dir, read_config, Config, ProcessEnv, LEGACY_DIR};
    use crate::error::{Error, PLAN_3};

    /// `read_config(missions_dir)`, whose `config.yaml` holds
    /// `project:\n  nested: x\n`: Go's defaults, because `project` is not a
    /// string.
    pub(crate) fn outside_subset(missions_dir: &Path) {
        assert_eq!(
            read_config(missions_dir).unwrap(),
            Config {
                project: "missions".into(),
                version: "1".into(),
                max_plan_depth: None,
            }
        );
    }

    /// Discovery from `start`, with a `.seeds` entry in `root` that Go
    /// migrates or refuses: Unsupported naming s4-missions-lib-3 with a
    /// feature naming `.seeds`, and `root/.seeds` is still there with the
    /// same file type.
    pub(crate) fn legacy_found(root: &Path, start: &Path) {
        let before = fs::symlink_metadata(root.join(LEGACY_DIR))
            .unwrap()
            .file_type();
        match find_dir(start, ProcessEnv::Inherit) {
            Err(Error::Unsupported { plan, feature }) => {
                assert_eq!(plan, PLAN_3);
                assert!(feature.contains(".seeds"), "{feature}");
            }
            other => panic!("expected Unsupported, got {other:?}"),
        }
        let after = fs::symlink_metadata(root.join(LEGACY_DIR))
            .expect("nothing was moved")
            .file_type();
        assert_eq!(after, before, "nothing was moved");
        if before.is_dir() {
            assert!(root.join(LEGACY_DIR).is_dir(), "nothing was moved");
        }
    }

    /// `init` in `root`, which holds a `.seeds` directory: Unsupported naming
    /// s4-missions-lib-3, and `root/.luuna` was not created.
    pub(crate) fn init_with_legacy_folder(root: &Path) {
        match run(&Invocation::new(root, ["init"])) {
            Err(Error::Unsupported { plan, .. }) => assert_eq!(plan, PLAN_3),
            other => panic!("expected Unsupported, got {other:?}"),
        }
        assert!(!root.join(".luuna").exists());
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::config::{read_config, resolve_plain, Scalar, CONFIG_FILE};

    #[test]
    fn outcomes_without_notices_pass_through_unchanged() {
        let outcome = Outcome {
            stdout: b"out\n".to_vec(),
            stderr: b"err\n".to_vec(),
            exit_code: 3,
        };
        let passed = with_stderr_notices(|| Ok(outcome.clone())).unwrap();
        assert_eq!(passed, outcome);
        let error = with_stderr_notices(|| Err(Error::unsupported("thing", "later"))).unwrap_err();
        assert!(
            matches!(&error, Error::Unsupported { feature, plan } if feature == "thing" && plan == "later"),
            "{error}"
        );
        assert_eq!(
            error.to_string(),
            "thing is not supported yet; it is ported in later"
        );
    }

    #[test]
    fn the_tree_resolver_agrees_with_the_subset_reader() {
        // Every plain scalar of config.rs's resolver tests, read as the value
        // of a key by both readers.
        for text in [
            "project",
            "parity",
            "x",
            "1",
            "3",
            "4",
            "5",
            "12",
            "-2",
            "2.5",
            "4.0",
            "true",
            "~",
            ".inf",
            "yes",
            "no-1",
            "-x",
            ".x",
            "a b",
            "Null",
            "1e3",
            "0x10",
            "0O17",
            "-0b101",
            "010",
            "08",
            "1_000",
            "+7",
            "5.",
            ".5e1",
            "-9223372036854775808",
            "9223372036854775808",
            "18446744073709551615",
            "0x1FFFFFFFFFFFFFFFF",
            "1e400",
            "0x",
            "1e",
            "1.2.3",
            "-0xFFFFFFFFFFFFFFFFF",
            ".5x",
            "+-1",
            "2024-01-01",
            "1999-x",
            ".nan",
            "-.Inf",
            "FALSE",
            "null",
            "0",
            "-0",
            "0b",
            "1_",
            "1e-400",
        ] {
            let subset = resolve_plain(text);
            let tree = yaml::decode_document(format!("v: {text}\n").as_bytes());
            let value = match &tree {
                Ok(Some(TreeValue::Map(entries))) if entries.len() == 1 => &entries[0].1,
                _ => {
                    // The subset reader refuses timestamp-like values; the
                    // tree reader refuses them too.
                    assert!(subset.is_err(), "{text}: {subset:?} but {tree:?}");
                    assert!(
                        matches!(tree, Err(Error::Unsupported { .. })),
                        "{text}: {tree:?}"
                    );
                    continue;
                }
            };
            let same = match (&subset, value) {
                (Ok(Scalar::Str(a)), TreeValue::Str(b)) => a == b,
                (Ok(Scalar::Int(a)), TreeValue::Int(b)) => a == b,
                (Ok(Scalar::Float(a)), TreeValue::Float(b)) => a == b || (a.is_nan() && b.is_nan()),
                (Ok(Scalar::Other), TreeValue::Null | TreeValue::Bool(_) | TreeValue::Uint(_)) => {
                    true
                }
                _ => false,
            };
            assert!(same, "{text}: {subset:?} but {value:?}");
        }
        // `<<` is a merge key to the subset reader; as a value yaml.v3 reads
        // the string.
        assert!(resolve_plain("<<").is_err());
        assert_eq!(
            yaml::decode_document(b"v: <<\n").unwrap(),
            Some(TreeValue::Map(vec![(
                "v".into(),
                TreeValue::Str("<<".into())
            )]))
        );
    }

    #[test]
    fn read_outside_subset_reads_nested_yaml_as_go_reads_it() {
        // The file is written in a temporary folder and read from there; the
        // reader does no discovery.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(CONFIG_FILE);
        let read = |content: &str| {
            fs::write(&path, content).unwrap();
            read_config(dir.path())
        };
        let config = |project: &str, version: &str, depth: Option<i64>| Config {
            project: project.to_owned(),
            version: version.to_owned(),
            max_plan_depth: depth,
        };
        assert_eq!(
            read(concat!(
                "# Missions\n",
                "project: &name demo # the prefix\n",
                "version: '2'\n",
                "max_plan_depth: 4.0\n",
                "plan_templates:\n",
                "  small:\n",
                "    sections: {goal: {required: true}}\n",
                "alias: *name\n",
                "---\n",
                "project: later\n",
            ))
            .unwrap(),
            config("demo", "2", Some(4))
        );
        // Merged keys count, and the mapping's own keys win.
        assert_eq!(
            read("base: &b {project: merged, version: \"3\"}\nversion: own\n<<: *b\n").unwrap(),
            config("merged", "own", None)
        );
        // Values of other types leave the defaults, as in Go.
        assert_eq!(
            read("project: [a]\nversion: 2\nmax_plan_depth: 2.5\n").unwrap(),
            Config::default()
        );
        assert_eq!(
            read("project:\n  nested: x\nmax_plan_depth: 0x7\n").unwrap(),
            config("missions", "1", Some(7))
        );
        assert_eq!(read("---\n").unwrap(), Config::default());
        assert_eq!(
            read_outside_subset(&path, b"", String::new()).unwrap(),
            Config::default()
        );
        // Go's alias error is Go's text; other refusals name the file.
        assert_eq!(
            read("project: demo\nloop: &l [*l]\n")
                .unwrap_err()
                .to_string(),
            "yaml: anchor 'l' value contains itself"
        );
        match read("- a\n") {
            Err(Error::Unsupported { feature, plan }) => {
                assert_eq!(plan, yaml::YAML_ERROR_TEXTS_NOT_PORTED);
                assert!(
                    feature.starts_with(&format!("{}: ", path.display())),
                    "{feature}"
                );
            }
            other => panic!("expected Unsupported, got {other:?}"),
        }
    }
}
