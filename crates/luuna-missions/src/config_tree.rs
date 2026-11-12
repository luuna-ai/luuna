//! The full `config.yaml` and the legacy `.seeds` migration, owned by
//! s4-missions-lib-3 (`internal/config/tree.go`, `internal/config/legacy.go`).
//!
//! [`read_outside_subset`] reads any `config.yaml` as Go's `ReadConfig`
//! does, through the YAML reader in [`yaml`], which every `config.yaml`
//! reader of this crate uses. [`migrate_legacy_layout`] migrates or refuses
//! a legacy `.seeds` store as Go does ([`legacy`]), and
//! [`with_stderr_notices`] puts the notices Go prints to stderr while a
//! command runs (the migration's line) before the command's stderr.
//! `config.rs` and `command/init.rs` are not edited by s4-missions-lib-3.

use std::cell::RefCell;
use std::io::Write;
use std::path::Path;

use crate::command::Outcome;
use crate::config::Config;
use crate::error::Result;

pub(crate) mod legacy;
pub(crate) mod yaml;

#[allow(unused_imports, reason = "used by t4, t5, t9 and s4-missions-lib-4")]
pub(crate) use legacy::{legacy_hook_command, upgrade_legacy_config_keys, upgrade_legacy_markers};

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

thread_local! {
    /// The notices captured by the innermost open scope of this thread;
    /// `None` when no scope is open.
    static NOTICES: RefCell<Option<Vec<u8>>> = const { RefCell::new(None) };
}

/// Runs one command and adds to its stderr the notices Go prints to
/// `os.Stderr` while the command runs (such as the line of the legacy
/// migration, which Go's `FindDir` prints for every command). On success
/// the notices come first, as Go prints them before the command's own
/// output; on an error, which carries no output, they are written to the
/// process's stderr, where Go would have written them.
pub(crate) fn with_stderr_notices(run: impl FnOnce() -> Result<Outcome>) -> Result<Outcome> {
    let (result, notices) = capture_stderr_notices(run);
    match result {
        Ok(mut outcome) => {
            if !notices.is_empty() {
                let mut stderr = notices;
                stderr.append(&mut outcome.stderr);
                outcome.stderr = stderr;
            }
            Ok(outcome)
        }
        Err(error) => {
            write_to_process_stderr(&notices);
            Err(error)
        }
    }
}

/// Runs `f` with a notice scope open on this thread and returns its result
/// with the notices raised meanwhile. Scopes nest: an inner scope captures
/// what is raised inside it, and the outer scope is restored afterwards,
/// also when `f` panics.
pub(crate) fn capture_stderr_notices<T>(f: impl FnOnce() -> T) -> (T, Vec<u8>) {
    /// Restores the enclosing scope (or none) when dropped.
    struct Scope(Option<Vec<u8>>);
    impl Drop for Scope {
        fn drop(&mut self) {
            let outer = self.0.take();
            NOTICES.with(|notices| *notices.borrow_mut() = outer);
        }
    }
    let scope = Scope(NOTICES.with(|notices| notices.borrow_mut().replace(Vec::new())));
    let value = f();
    let captured = NOTICES.with(|notices| notices.borrow_mut().take());
    drop(scope);
    (value, captured.unwrap_or_default())
}

/// Raises a notice Go writes to `os.Stderr`: captured by the open scope of
/// this thread, or, with none open (as in `Store::discover`), written to the
/// process's stderr as Go writes it.
pub(crate) fn notice(bytes: &[u8]) {
    let captured = NOTICES.with(|notices| match notices.borrow_mut().as_mut() {
        Some(buffer) => {
            buffer.extend_from_slice(bytes);
            true
        }
        None => false,
    });
    if !captured {
        write_to_process_stderr(bytes);
    }
}

/// Writes `bytes` to the process's stderr. Like Go's `fmt.Fprintf` to
/// `os.Stderr`, a failed write is ignored.
fn write_to_process_stderr(bytes: &[u8]) {
    if !bytes.is_empty() {
        let mut stderr = std::io::stderr().lock();
        let _ = stderr.write_all(bytes);
        let _ = stderr.flush();
    }
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
/// `project_root`, in every case where Go migrates it or refuses
/// ([`legacy::migrate`]). Go's refusals are [`Error::Command`] with Go's
/// text; file-system failures are [`Error::Io`].
///
/// [`Error::Command`]: crate::error::Error::Command
/// [`Error::Io`]: crate::error::Error::Io
pub(crate) fn migrate_legacy_layout(project_root: &Path, legacy: &Path) -> Result<()> {
    legacy::migrate(project_root, legacy)
}

/// The expectations of the unit tests in `config.rs` and `command/init.rs`
/// for what this module ports. Their inputs stay in those tests; each
/// function asserts Go's exact result for the same input, as literals
/// (`go-first-1.txt` items b and c, with the test's root in place of Go's).
#[cfg(test)]
pub(crate) mod checks {
    use std::fs;
    use std::path::Path;

    use super::capture_stderr_notices;
    use crate::command::{run, Invocation};
    use crate::config::{find_dir, read_config, Config, ProcessEnv, LEGACY_DIR, MISSIONS_DIR};
    use crate::error::Error;
    use crate::gopath;

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

    /// The legacy cases [`legacy_found`] knows.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum LegacyCase {
        /// An empty `.seeds` directory in `start` (Bb1).
        Move,
        /// An empty `.seeds` directory in `root`, found from below (Bb2).
        Parent,
        /// `.seeds` and `.luuna/missions` both present (Bb3).
        Both,
        /// `.seeds` a symbolic link (Bb4, Bb4b).
        Symlink,
    }

    /// The notice Go prints for a migration in `root`
    /// (`go-first-1.txt:151-154`).
    fn migrated_notice(root: &Path) -> String {
        format!(
            "Migrated legacy .seeds/ to .luuna/missions/ in {}\n",
            gopath::text(root)
        )
    }

    /// Discovery from `start`, with a `.seeds` entry in `root` that Go
    /// migrates or refuses, against Go's results (`go-first-1.txt:151-161`,
    /// no store above the system temp folder): an empty `.seeds` directory
    /// is moved to `.luuna/missions` with Go's notice and discovery goes on
    /// to "Not in a missions project"; with `.luuna/missions` present, or
    /// with `.seeds` a link, Go's refusal text, no notice, and `.seeds`
    /// untouched. Panics for any other input.
    pub(crate) fn legacy_found(root: &Path, start: &Path) {
        let legacy = root.join(LEGACY_DIR);
        let before = fs::symlink_metadata(&legacy).unwrap().file_type();
        let empty_dir = || before.is_dir() && fs::read_dir(&legacy).unwrap().next().is_none();
        let case = if start != root && empty_dir() {
            LegacyCase::Parent
        } else if start != root {
            panic!("legacy_found: unknown case below {}", root.display());
        } else if before.is_symlink() {
            LegacyCase::Symlink
        } else if fs::symlink_metadata(root.join(MISSIONS_DIR)).is_ok() {
            LegacyCase::Both
        } else if empty_dir() {
            LegacyCase::Move
        } else {
            panic!("legacy_found: unknown case in {}", root.display());
        };
        let (result, notices) = capture_stderr_notices(|| find_dir(start, ProcessEnv::Inherit));
        let notices = String::from_utf8(notices).unwrap();
        let root_text = gopath::text(root);
        match case {
            LegacyCase::Move | LegacyCase::Parent => {
                assert!(matches!(result, Err(Error::NotInProject)), "{result:?}");
                assert_eq!(
                    result.unwrap_err().to_string(),
                    "Not in a missions project. Run `luuna-missions init` first."
                );
                assert_eq!(notices, migrated_notice(root), "{case:?}");
                let moved = fs::symlink_metadata(root.join(MISSIONS_DIR)).unwrap();
                assert!(moved.is_dir(), "{case:?}");
                assert!(
                    fs::symlink_metadata(root.join(".luuna")).unwrap().is_dir(),
                    "{case:?}"
                );
                assert_eq!(
                    fs::symlink_metadata(&legacy).unwrap_err().kind(),
                    std::io::ErrorKind::NotFound,
                    "{case:?}: .seeds is gone"
                );
            }
            LegacyCase::Both | LegacyCase::Symlink => {
                let expected = if case == LegacyCase::Both {
                    format!("Both .seeds/ and .luuna/missions/ exist in {root_text}. Resolve manually: keep .luuna/missions/ and remove .seeds/, or the reverse.")
                } else {
                    format!("Refusing to migrate .seeds/ in {root_text}: .seeds or .luuna is a symlink. Move it to .luuna/missions/ manually.")
                };
                match result {
                    Err(Error::Command { message }) => assert_eq!(message, expected),
                    other => panic!("{case:?}: expected Go's refusal, got {other:?}"),
                }
                assert_eq!(notices, "", "{case:?}");
                let after = fs::symlink_metadata(&legacy)
                    .expect("nothing was moved")
                    .file_type();
                assert_eq!(after, before, "{case:?}: nothing was moved");
            }
        }
    }

    /// `init` (no `--json`) in `root`, which holds an empty `.seeds`
    /// directory: Go's result (`go-first-1.txt:167-170`): exit 0, the
    /// success line on stdout, the migration notice on stderr, and exactly
    /// Go's files, contents and modes (umask 022), `.seeds` gone.
    pub(crate) fn init_with_legacy_folder(root: &Path) {
        let outcome = run(&Invocation::new(root, ["init"])).unwrap();
        let root_text = gopath::text(root);
        assert_eq!(
            String::from_utf8(outcome.stdout).unwrap(),
            format!("✓ Initialized .luuna/missions/ in {root_text}\n")
        );
        assert_eq!(
            String::from_utf8(outcome.stderr).unwrap(),
            migrated_notice(root)
        );
        assert_eq!(outcome.exit_code, 0);
        let project = gopath::text(&gopath::base(root));
        let config = format!("project: \"{project}\"\nversion: \"1\"\nmax_plan_depth: 3\n");
        let attributes = ".luuna/missions/issues.jsonl merge=union\n.luuna/missions/templates.jsonl merge=union\n.luuna/missions/plans.jsonl merge=union\n";
        let expected: Vec<(&str, Option<&str>, u32)> = vec![
            (".gitattributes", Some(attributes), 0o644),
            (".luuna", None, 0o755),
            (".luuna/missions", None, 0o755),
            (".luuna/missions/.gitignore", Some("*.lock\n"), 0o644),
            (".luuna/missions/config.yaml", Some(&config), 0o644),
            (".luuna/missions/issues.jsonl", Some(""), 0o644),
            (".luuna/missions/plans.jsonl", Some(""), 0o644),
            (".luuna/missions/templates.jsonl", Some(""), 0o644),
        ];
        let mut found = Vec::new();
        let mut pending = vec![String::new()];
        while let Some(relative) = pending.pop() {
            for entry in fs::read_dir(root.join(&relative)).unwrap() {
                let entry = entry.unwrap();
                let name = entry.file_name().into_string().unwrap();
                let path = if relative.is_empty() {
                    name
                } else {
                    format!("{relative}/{name}")
                };
                let meta = fs::symlink_metadata(entry.path()).unwrap();
                let content = if meta.is_dir() {
                    pending.push(path.clone());
                    None
                } else {
                    assert!(meta.is_file(), "{path}");
                    Some(String::from_utf8(fs::read(entry.path()).unwrap()).unwrap())
                };
                found.push((path, content, mode(&meta)));
            }
        }
        found.sort();
        let expected: Vec<(String, Option<String>, u32)> = expected
            .into_iter()
            .map(|(path, content, mode)| (path.to_owned(), content.map(str::to_owned), mode))
            .collect();
        assert_eq!(found, expected);
    }

    #[cfg(unix)]
    fn mode(meta: &fs::Metadata) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode() & 0o7777
    }

    #[cfg(not(unix))]
    fn mode(meta: &fs::Metadata) -> u32 {
        if meta.is_dir() {
            0o755
        } else {
            0o644
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::command::Invocation;
    use crate::config::{
        read_config, resolve_plain, Scalar, CONFIG_FILE, LEGACY_DIR, MISSIONS_DIR,
    };
    use crate::error::Error;

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
    fn notices_are_put_before_the_command_s_stderr() {
        // The seam alone: notices first, then the command's own stderr.
        let outcome = with_stderr_notices(|| {
            notice(b"first\n");
            notice(b"second\n");
            Ok(Outcome {
                stdout: b"out\n".to_vec(),
                stderr: b"err\n".to_vec(),
                exit_code: 1,
            })
        })
        .unwrap();
        assert_eq!(
            outcome,
            Outcome {
                stdout: b"out\n".to_vec(),
                stderr: b"first\nsecond\nerr\n".to_vec(),
                exit_code: 1,
            }
        );
        // An error carries no output: it is returned unchanged, its notice
        // goes to the process's stderr, and the scope is closed.
        let error = with_stderr_notices(|| {
            notice(
                b"(a notice written to stderr by notices_are_put_before_the_command_s_stderr)\n",
            );
            Err(Error::Command {
                message: "failed".into(),
            })
        })
        .unwrap_err();
        assert!(matches!(&error, Error::Command { message } if message == "failed"));
        NOTICES.with(|notices| assert!(notices.borrow().is_none()));
        // Scopes nest, and a panic closes the scope it ran in.
        let ((value, inner), outer) = capture_stderr_notices(|| {
            notice(b"outer\n");
            let inner = capture_stderr_notices(|| {
                notice(b"inner\n");
                7
            });
            notice(b"after\n");
            inner
        });
        assert_eq!((value, inner.as_slice()), (7, b"inner\n".as_slice()));
        assert_eq!(outer, b"outer\nafter\n");
        let (_, outer) = capture_stderr_notices(|| {
            let panicked = std::panic::catch_unwind(|| {
                capture_stderr_notices(|| {
                    notice(b"lost\n");
                    panic!("inside a scope");
                })
            });
            assert!(panicked.is_err());
            notice(b"kept\n");
        });
        assert_eq!(outer, b"kept\n");
        NOTICES.with(|notices| assert!(notices.borrow().is_none()));
        // A command: the migration's notice comes before the `--timing` line
        // the command writes. The legacy store holds config.yaml, so
        // discovery stops inside the temporary project (go-first-1.txt Bb6).
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        fs::create_dir(root.join(LEGACY_DIR)).unwrap();
        fs::write(
            root.join(LEGACY_DIR).join(CONFIG_FILE),
            "project: p\nversion: \"1\"\n",
        )
        .unwrap();
        let outcome =
            crate::command::run(&Invocation::new(root, ["--timing", "list", "--json"])).unwrap();
        assert_eq!(
            String::from_utf8(outcome.stdout).unwrap(),
            "{\n  \"success\": true,\n  \"command\": \"list\",\n  \"issues\": [],\n  \"count\": 0\n}\n"
        );
        assert_eq!(outcome.exit_code, 0);
        let stderr = String::from_utf8(outcome.stderr).unwrap();
        let notice_line = format!(
            "Migrated legacy .seeds/ to .luuna/missions/ in {}\n",
            crate::gopath::text(root)
        );
        let timing = stderr
            .strip_prefix(&notice_line)
            .unwrap_or_else(|| panic!("{stderr:?}"));
        assert!(
            timing.starts_with("⏱ ") && timing.ends_with('\n') && timing.lines().count() == 1,
            "{stderr:?}"
        );
        assert!(root.join(MISSIONS_DIR).join(CONFIG_FILE).is_file());
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
