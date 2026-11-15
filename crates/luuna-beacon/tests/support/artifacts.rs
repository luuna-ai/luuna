//! Owner: plan s3-beacon-lib-3.
//!
//! The plan's `PlanStep` and `PlanDone` implementations and the
//! normalisation for its own commands live here. The hook must leave the
//! documents of every other command unchanged.
//!
//! - [`sort_map_ordered`] only reorders: the `details` of doctor's
//!   `inheritance` check, the one array of this plan's documents that Go
//!   builds from map iteration (`cli.go:1447` over `current()`, `:2050`).
//! - [`ArtifactStep`] (a [`PlanStep`]) maps one step to its Go argv and its
//!   library call; its raw variants write source trees, symlinks, folders
//!   and modes, never under `.luuna/beacon` or `.canopy`. It never overrides
//!   `PlanStep::written_paths` (security finding 2): store files a case
//!   writes by hand go through the foundation's `Step::WriteFile` and
//!   `Step::SetMtime`.
//! - [`ArtifactDone`] (a [`PlanDone`]) builds Go's documents.
//! - [`compare_outside`] compares every file outside the compared store
//!   (exports, `CLAUDE.md`, source trees): type, bytes, mode and symlink
//!   target text, and refuses a leftover `.lbc-stage-`/`.lbc-backup-` name
//!   (AS54).
//! - [`Pair`] runs the Go binary and the library in two fresh roots with
//!   their own outside folders, optional test-made git repositories and the
//!   cleared environment of the plan's `test_isolation` (AS53). Every Go,
//!   git and library start is preceded by [`pair_guard`]. Its document
//!   comparison exempts, for doctor's parser and OS texts only, everything
//!   after the recognised prefix (AS55).
//! - [`Record`] writes artifact lines by hand, with Go's `json.Marshal`
//!   escaping and real digests.

use serde_json::Value;

/// Reorders the arrays Go builds from map iteration in a document of this
/// plan's commands: for `doctor`, the `details` of the check named
/// `inheritance`, sorted by string. Nothing is removed, inserted or
/// rewritten, and every other document and array is left as it is.
pub fn sort_map_ordered(command: &str, doc: &mut serde_json::Value, _project: &str) {
    if command != "doctor" {
        return;
    }
    let Some(checks) = doc.get_mut("checks").and_then(Value::as_array_mut) else {
        return;
    };
    for check in checks.iter_mut() {
        if check.get("name").and_then(Value::as_str) != Some("inheritance") {
            continue;
        }
        if let Some(details) = check.get_mut("details").and_then(Value::as_array_mut) {
            details.sort_by_cached_key(|detail| {
                detail
                    .as_str()
                    .map_or_else(|| detail.to_string(), str::to_owned)
            });
        }
    }
}

#[cfg(unix)]
#[allow(unused_imports)]
pub use harness::*;

#[cfg(unix)]
mod harness {
    use super::super::case::{
        assert_errors_match, fresh_root, lower, run_lib, snapshot, Case, LibRun, PlanStep, Seen,
        Step, MIGRATED_LINE,
    };
    use super::super::go;
    use super::super::normalise::Normaliser;
    use super::super::payload::{error_document, Done, PlanDone};
    use luuna_beacon::{
        Artifact, ArtifactExport, ArtifactExported, ArtifactFile, ArtifactHistory, ArtifactKind,
        ArtifactRead, ArtifactsError, Beacon, BeaconError, DoctorReport, DoctorStatus,
        GitEnvironment, GitRunner, OnboardAction, OnboardMode, OnboardStatus, Onboarded, Prime,
        PrimeOptions, SyncOptions, Synced,
    };
    use serde_json::{json, Value};
    use sha2::{Digest, Sha256};
    use std::collections::{BTreeMap, HashSet};
    use std::ffi::{OsStr, OsString};
    use std::fs;
    use std::io::{Read as _, Write as _};
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Component, Path, PathBuf};
    use std::process::{Command, Output, Stdio};
    use std::sync::OnceLock;
    use std::time::{Duration, SystemTime};
    use tempfile::TempDir;

    // ---- constants ------------------------------------------------------------

    /// The project name of every artifact case (AS50): ids `artifact-<hex>`
    /// are numbered by the frozen normaliser's `<project>-<hex>` pattern.
    pub const PROJECT: &str = "artifact";
    pub const SKILL: &str = "skill";
    pub const AGENT: &str = "agent";
    pub const BUNDLE: &str = "bundle";

    /// The timestamp hand-written records carry.
    pub const STAMP: &str = "2024-01-01T00:00:00Z";

    /// Go's onboarding snippet (`cli.go:1351-1352`), byte for byte.
    pub const GO_SNIPPET: &str = "<!-- luuna-beacon:start -->\n## Prompt Management (Luuna Beacon)\n<!-- luuna-beacon-onboard-v:3 -->\n\nRun `lbc prime` at the start of every session. Use `lbc update` then `lbc emit`; do not edit emitted files.\n<!-- luuna-beacon:end -->\n";
    pub const ONBOARD_START: &str = "<!-- luuna-beacon:start -->";
    pub const ONBOARD_END: &str = "<!-- luuna-beacon:end -->";
    pub const ONBOARD_VERSION: &str = "<!-- luuna-beacon-onboard-v:3 -->";

    /// Go's two prime texts (`cli.go:1321-1323`).
    pub const PRIME_DEFAULT: &str = "# Luuna Beacon Workflow Context\n\n- Storage: `.luuna/beacon/prompts.jsonl`\n- Use `lbc update`, then `lbc emit`; do not edit emitted files.\n- Run `lbc render <name>` to resolve composition.\n";
    pub const PRIME_COMPACT: &str = "# Luuna Beacon Quick Reference\n\nlbc list\nlbc render <name>\nlbc emit --all\nlbc update <name>\n";

    /// Root-relative paths the store comparison covers (or, for `.git`, a
    /// test-made repository's own files), left out of [`compare_outside`].
    pub const STORE_SKIPS: [&str; 5] = [
        ".luuna/beacon",
        ".canopy",
        ".git",
        ".gitattributes",
        ".gitignore",
    ];

    /// The arguments Go's `Run` takes out of argv before dispatch; a
    /// positional with one of these values would never reach the command.
    const GLOBAL_FLAGS: [&str; 8] = [
        "--json",
        "--quiet",
        "-q",
        "--timing",
        "--version",
        "-v",
        "--help",
        "-h",
    ];

    // ---- kinds and small values ---------------------------------------------

    /// The library kind for Go's kind word.
    pub fn kind_of(kind: &str) -> ArtifactKind {
        match kind {
            "skill" => ArtifactKind::Skill,
            "agent" => ArtifactKind::Agent,
            "bundle" => ArtifactKind::Bundle,
            other => panic!("not an artifact kind: {other:?}"),
        }
    }

    /// The onboard mode of a step.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Onboard {
        Apply,
        Check,
        Stdout,
    }

    impl Onboard {
        fn library(self) -> OnboardMode {
            match self {
                Onboard::Apply => OnboardMode::Apply,
                Onboard::Check => OnboardMode::Check,
                Onboard::Stdout => OnboardMode::Stdout,
            }
        }
    }

    /// One file of a raw source tree.
    #[derive(Clone, Debug)]
    pub struct Entry {
        /// Relative path bytes (may be invalid UTF-8).
        pub path: Vec<u8>,
        pub contents: Vec<u8>,
        pub executable: bool,
    }

    pub fn entry(path: &str, contents: &str) -> Entry {
        entry_bytes(path.as_bytes(), contents.as_bytes(), false)
    }

    pub fn exec_entry(path: &str, contents: &str) -> Entry {
        entry_bytes(path.as_bytes(), contents.as_bytes(), true)
    }

    pub fn entry_bytes(path: &[u8], contents: &[u8], executable: bool) -> Entry {
        Entry {
            path: path.to_vec(),
            contents: contents.to_vec(),
            executable,
        }
    }

    /// A piece of a streamed file: literal bytes, or a constant chunk
    /// written a constant number of times.
    #[derive(Clone, Debug)]
    pub enum Part {
        Bytes(Vec<u8>),
        Repeat { chunk: &'static [u8], times: usize },
    }

    /// Writes `parts` to `path` one piece at a time; nothing larger than one
    /// chunk is held in memory.
    pub fn stream_file(path: &Path, parts: &[Part]) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create the parent of a streamed file");
        }
        let file = fs::File::create(path)
            .unwrap_or_else(|error| panic!("create {}: {error}", path.display()));
        let mut out = std::io::BufWriter::new(file);
        for part in parts {
            match part {
                Part::Bytes(bytes) => out.write_all(bytes).expect("write a streamed part"),
                Part::Repeat { chunk, times } => {
                    for _ in 0..*times {
                        out.write_all(chunk).expect("write a streamed chunk");
                    }
                }
            }
        }
        out.flush().expect("flush a streamed file");
    }

    // ---- the step -------------------------------------------------------------

    /// How Go treats the legacy move for a command.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Migration {
        /// Run first; a refusal is the command's error.
        Required,
        /// Run first; a refusal is ignored (prime).
        Ignored,
        /// Never run (onboard, and `<kind>` without a subcommand).
        Never,
    }

    /// A step of this plan: one command in Go's grammar, or a raw change to
    /// both roots outside the store.
    #[derive(Clone, Debug)]
    pub enum ArtifactStep {
        /// `<kind>` alone.
        NoSubcommand { kind: &'static str },
        /// `<kind> <subcommand>` for a subcommand Go does not know.
        Unknown {
            kind: &'static str,
            subcommand: String,
        },
        /// `<kind> import [source] [--name name]`.
        Import {
            kind: &'static str,
            source: Option<String>,
            name: Option<String>,
        },
        /// `<kind> update [name] [source]`.
        Update {
            kind: &'static str,
            name: Option<String>,
            source: Option<String>,
        },
        /// `<kind> list`.
        List { kind: &'static str },
        /// `<kind> verify [reference]`.
        Verify {
            kind: &'static str,
            reference: Option<String>,
        },
        /// `<kind> show [reference]`.
        Show {
            kind: &'static str,
            reference: Option<String>,
        },
        /// `<kind> history [reference]`.
        History {
            kind: &'static str,
            reference: Option<String>,
        },
        /// `<kind> read [reference] [file]`.
        Read {
            kind: &'static str,
            reference: Option<String>,
            file: Option<String>,
        },
        /// `<kind> export [reference] [--out o] [--target t] [--force]`.
        Export {
            kind: &'static str,
            reference: Option<String>,
            out: Option<String>,
            target: Option<String>,
            force: bool,
        },
        /// `doctor [--fix]`.
        Doctor { fix: bool },
        /// `prime [--compact] [--export]`.
        Prime { compact: bool, export: bool },
        /// `onboard`, `onboard --check` or `onboard --stdout`.
        Onboard { mode: Onboard },
        /// `sync [--status]`; only through [`Pair`].
        Sync { status_only: bool },
        /// Raw: writes the entries under `dir` (files created or truncated;
        /// mode 0755 when executable, else 0644).
        Tree { dir: String, entries: Vec<Entry> },
        /// Raw: a symbolic link at `path` whose target text is `target`.
        Symlink { path: String, target: String },
        /// Raw: a folder and its missing parents.
        Folder { path: String },
        /// Raw: sets the permission bits of a path.
        Chmod { path: String, mode: u32 },
        /// Raw: a file written piece by piece ([`stream_file`]).
        Stream { path: String, parts: Vec<Part> },
        /// Raw: removes a file, a link or a folder with its contents.
        Remove { path: String },
    }

    pub fn no_subcommand(kind: &'static str) -> ArtifactStep {
        ArtifactStep::NoSubcommand { kind }
    }

    pub fn unknown(kind: &'static str, subcommand: &str) -> ArtifactStep {
        ArtifactStep::Unknown {
            kind,
            subcommand: subcommand.to_owned(),
        }
    }

    fn owned(value: Option<&str>) -> Option<String> {
        value.map(str::to_owned)
    }

    pub fn import(kind: &'static str, source: &str, name: &str) -> ArtifactStep {
        import_args(kind, Some(source), Some(name))
    }

    pub fn import_args(
        kind: &'static str,
        source: Option<&str>,
        name: Option<&str>,
    ) -> ArtifactStep {
        ArtifactStep::Import {
            kind,
            source: owned(source),
            name: owned(name),
        }
    }

    pub fn update(kind: &'static str, name: &str, source: &str) -> ArtifactStep {
        update_args(kind, Some(name), Some(source))
    }

    pub fn update_args(
        kind: &'static str,
        name: Option<&str>,
        source: Option<&str>,
    ) -> ArtifactStep {
        ArtifactStep::Update {
            kind,
            name: owned(name),
            source: owned(source),
        }
    }

    pub fn list(kind: &'static str) -> ArtifactStep {
        ArtifactStep::List { kind }
    }

    pub fn verify(kind: &'static str, reference: Option<&str>) -> ArtifactStep {
        ArtifactStep::Verify {
            kind,
            reference: owned(reference),
        }
    }

    pub fn show(kind: &'static str, reference: &str) -> ArtifactStep {
        show_args(kind, Some(reference))
    }

    pub fn show_args(kind: &'static str, reference: Option<&str>) -> ArtifactStep {
        ArtifactStep::Show {
            kind,
            reference: owned(reference),
        }
    }

    pub fn history(kind: &'static str, reference: &str) -> ArtifactStep {
        history_args(kind, Some(reference))
    }

    pub fn history_args(kind: &'static str, reference: Option<&str>) -> ArtifactStep {
        ArtifactStep::History {
            kind,
            reference: owned(reference),
        }
    }

    pub fn read(kind: &'static str, reference: &str, file: &str) -> ArtifactStep {
        read_args(kind, Some(reference), Some(file))
    }

    pub fn read_args(
        kind: &'static str,
        reference: Option<&str>,
        file: Option<&str>,
    ) -> ArtifactStep {
        ArtifactStep::Read {
            kind,
            reference: owned(reference),
            file: owned(file),
        }
    }

    /// `<kind> export <reference>`; add flags with [`ArtifactStep::out`],
    /// [`ArtifactStep::target`] and [`ArtifactStep::force`].
    pub fn export(kind: &'static str, reference: &str) -> ArtifactStep {
        export_args(kind, Some(reference))
    }

    pub fn export_args(kind: &'static str, reference: Option<&str>) -> ArtifactStep {
        ArtifactStep::Export {
            kind,
            reference: owned(reference),
            out: None,
            target: None,
            force: false,
        }
    }

    pub fn doctor(fix: bool) -> ArtifactStep {
        ArtifactStep::Doctor { fix }
    }

    pub fn prime(compact: bool, export: bool) -> ArtifactStep {
        ArtifactStep::Prime { compact, export }
    }

    pub fn onboard(mode: Onboard) -> ArtifactStep {
        ArtifactStep::Onboard { mode }
    }

    pub fn sync(status_only: bool) -> ArtifactStep {
        ArtifactStep::Sync { status_only }
    }

    pub fn tree(dir: &str, entries: Vec<Entry>) -> ArtifactStep {
        ArtifactStep::Tree {
            dir: dir.to_owned(),
            entries,
        }
    }

    pub fn symlink(path: &str, target: &str) -> ArtifactStep {
        ArtifactStep::Symlink {
            path: path.to_owned(),
            target: target.to_owned(),
        }
    }

    pub fn folder(path: &str) -> ArtifactStep {
        ArtifactStep::Folder {
            path: path.to_owned(),
        }
    }

    pub fn chmod(path: &str, mode: u32) -> ArtifactStep {
        ArtifactStep::Chmod {
            path: path.to_owned(),
            mode,
        }
    }

    pub fn stream(path: &str, parts: Vec<Part>) -> ArtifactStep {
        ArtifactStep::Stream {
            path: path.to_owned(),
            parts,
        }
    }

    pub fn remove(path: &str) -> ArtifactStep {
        ArtifactStep::Remove {
            path: path.to_owned(),
        }
    }

    /// A positional argument: refused if Go's `Run` would take it out of
    /// argv, or if a flag scan could read it as a flag.
    fn positional(value: &str) -> String {
        assert!(
            !value.starts_with("--") && !GLOBAL_FLAGS.contains(&value),
            "a positional {value:?} would not reach the Go command"
        );
        value.to_owned()
    }

    /// A flag value. Go's `option()` drops a value that starts with `--`.
    fn flag_value(value: &str) -> String {
        assert!(
            !value.starts_with("--") && !GLOBAL_FLAGS.contains(&value),
            "a flag value {value:?} is dropped by Go's option()"
        );
        value.to_owned()
    }

    fn command(kind: &str, sub: &str) -> Vec<String> {
        vec![kind.to_owned(), sub.to_owned()]
    }

    fn substitute(text: &str, root: &Path, outside: &Path) -> String {
        let root = root.to_str().expect("a test root is UTF-8");
        let outside = outside.to_str().expect("a test folder is UTF-8");
        text.replace("<root>", root).replace("<outside>", outside)
    }

    impl ArtifactStep {
        /// Adds `--out <out>` to an export step.
        pub fn out(mut self, value: &str) -> Self {
            match &mut self {
                ArtifactStep::Export { out, .. } => *out = Some(value.to_owned()),
                _ => panic!("--out belongs to an export step"),
            }
            self
        }

        /// Adds `--target <target>` to an export step.
        pub fn target(mut self, value: &str) -> Self {
            match &mut self {
                ArtifactStep::Export { target, .. } => *target = Some(value.to_owned()),
                _ => panic!("--target belongs to an export step"),
            }
            self
        }

        /// Adds `--force` to an export step.
        pub fn force(mut self) -> Self {
            match &mut self {
                ArtifactStep::Export { force, .. } => *force = true,
                _ => panic!("--force belongs to an export step"),
            }
            self
        }

        /// How Go treats the legacy move for this step.
        pub fn migration(&self) -> Migration {
            match self {
                ArtifactStep::Prime { .. } => Migration::Ignored,
                ArtifactStep::Onboard { .. } | ArtifactStep::NoSubcommand { .. } => {
                    Migration::Never
                }
                _ => Migration::Required,
            }
        }

        /// Whether Go prints raw text instead of JSON (`onboard --stdout`
        /// ignores `--json`, GD67).
        pub fn prints_raw(&self) -> bool {
            matches!(
                self,
                ArtifactStep::Onboard {
                    mode: Onboard::Stdout
                }
            )
        }

        /// The path-like texts of the step that may hold the placeholders
        /// `<root>` and `<outside>`.
        fn placeholder_texts(&self) -> Vec<&String> {
            match self {
                ArtifactStep::Import { source, .. } | ArtifactStep::Update { source, .. } => {
                    source.iter().collect()
                }
                ArtifactStep::Export { out, .. } => out.iter().collect(),
                ArtifactStep::Symlink { target, .. } => vec![target],
                _ => Vec::new(),
            }
        }

        /// Whether the step names a per-side folder; such steps run only
        /// through [`Pair`].
        pub fn has_placeholders(&self) -> bool {
            self.placeholder_texts()
                .iter()
                .any(|text| text.contains("<root>") || text.contains("<outside>"))
        }

        /// The step for one side: `<root>` and `<outside>` replaced by that
        /// side's folders.
        pub fn resolved(&self, root: &Path, outside: &Path) -> ArtifactStep {
            let mut step = self.clone();
            let fix = |value: &mut Option<String>| {
                if let Some(text) = value {
                    *text = substitute(text, root, outside);
                }
            };
            match &mut step {
                ArtifactStep::Import { source, .. } | ArtifactStep::Update { source, .. } => {
                    fix(source)
                }
                ArtifactStep::Export { out, .. } => fix(out),
                ArtifactStep::Symlink { target, .. } => {
                    *target = substitute(target, root, outside);
                }
                _ => {}
            }
            step
        }

        /// The library call with an optional git runner (sync needs one).
        pub fn run(
            &self,
            beacon: &Beacon,
            git: Option<GitRunner>,
        ) -> Result<ArtifactDone, BeaconError> {
            use ArtifactStep as S;
            match self {
                S::NoSubcommand { .. } => {
                    Err(BeaconError::from(ArtifactsError::SubcommandRequired))
                }
                S::Unknown { kind, subcommand } => {
                    beacon.artifact_store_check()?;
                    Err(BeaconError::from(ArtifactsError::UnknownSubcommand {
                        kind: (*kind).into(),
                        subcommand: subcommand.as_str().into(),
                    }))
                }
                S::Import { kind, source, name } => beacon
                    .artifact_import(
                        kind_of(kind),
                        as_path(source),
                        name.as_deref().unwrap_or_default(),
                    )
                    .map(|artifact| ArtifactDone::Stored {
                        command: format!("{kind} import"),
                        artifact,
                    }),
                S::Update { kind, name, source } => beacon
                    .artifact_update(kind_of(kind), name.as_deref(), as_path(source))
                    .map(|artifact| ArtifactDone::Stored {
                        command: format!("{kind} update"),
                        artifact,
                    }),
                S::List { kind } => beacon
                    .artifact_list(kind_of(kind))
                    .map(|items| ArtifactDone::Listed { kind, items }),
                S::Verify { kind, reference } => beacon
                    .artifact_verify(kind_of(kind), reference.as_deref())
                    .map(|checked| ArtifactDone::Verified { kind, checked }),
                S::Show { kind, reference } => beacon
                    .artifact_show(kind_of(kind), reference.as_deref())
                    .map(|artifact| ArtifactDone::Shown { kind, artifact }),
                S::History { kind, reference } => beacon
                    .artifact_history(kind_of(kind), reference.as_deref())
                    .map(|history| ArtifactDone::History { kind, history }),
                S::Read {
                    kind,
                    reference,
                    file,
                } => beacon
                    .artifact_read(kind_of(kind), reference.as_deref(), file.as_deref())
                    .map(|read| ArtifactDone::Read { kind, read }),
                S::Export {
                    kind,
                    reference,
                    out,
                    target,
                    force,
                } => {
                    let options = ArtifactExport {
                        out: out.clone().unwrap_or_default(),
                        target: target.clone().unwrap_or_default(),
                        force: *force,
                    };
                    beacon
                        .artifact_export(kind_of(kind), reference.as_deref(), &options)
                        .map(|exported| ArtifactDone::Exported { kind, exported })
                }
                S::Doctor { fix } => beacon.doctor(*fix).map(ArtifactDone::Doctor),
                S::Prime { compact, export } => {
                    Ok(ArtifactDone::Prime(beacon.prime(&PrimeOptions {
                        compact: *compact,
                        export: *export,
                    })))
                }
                S::Onboard { mode } => beacon.onboard(mode.library()).map(ArtifactDone::Onboarded),
                S::Sync { status_only } => {
                    let git = git.unwrap_or_else(|| {
                        panic!("sync runs only through Pair, with the test's git environment")
                    });
                    beacon
                        .sync(&SyncOptions {
                            status_only: *status_only,
                            git,
                        })
                        .map(ArtifactDone::Synced)
                }
                S::Tree { .. }
                | S::Symlink { .. }
                | S::Folder { .. }
                | S::Chmod { .. }
                | S::Stream { .. }
                | S::Remove { .. } => unreachable!("a raw step has no library call"),
            }
        }
    }

    fn as_path(value: &Option<String>) -> Option<&Path> {
        value.as_deref().map(Path::new)
    }

    /// Panics unless `relative` stays inside the root and outside the
    /// compared store (AS54: raw steps never write store files).
    fn checked_relative(relative: &str) -> &Path {
        let path = Path::new(relative);
        assert!(
            !relative.is_empty() && path.is_relative(),
            "a raw path must be relative: {relative:?}"
        );
        assert!(
            path.components()
                .all(|component| matches!(component, Component::Normal(_) | Component::CurDir)),
            "a raw path must stay inside the root: {relative:?}"
        );
        assert!(
            !path.starts_with(".luuna/beacon") && !path.starts_with(".canopy"),
            "raw steps do not write store files ({relative:?}); use Step::WriteFile"
        );
        path
    }

    fn set_mode(path: &Path, mode: u32) {
        fs::set_permissions(path, fs::Permissions::from_mode(mode))
            .unwrap_or_else(|error| panic!("chmod {}: {error}", path.display()));
    }

    fn apply_raw_step(step: &ArtifactStep, root: &Path) {
        match step {
            ArtifactStep::Tree { dir, entries } => {
                let base = root.join(checked_relative(dir));
                for entry in entries {
                    let relative = Path::new(OsStr::from_bytes(&entry.path));
                    assert!(
                        relative.is_relative()
                            && relative
                                .components()
                                .all(|component| matches!(component, Component::Normal(_))),
                        "a tree entry must stay inside its folder: {relative:?}"
                    );
                    let target = base.join(relative);
                    if let Some(parent) = target.parent() {
                        fs::create_dir_all(parent).expect("create a source folder");
                    }
                    fs::write(&target, &entry.contents)
                        .unwrap_or_else(|error| panic!("write {}: {error}", target.display()));
                    set_mode(&target, if entry.executable { 0o755 } else { 0o644 });
                }
            }
            ArtifactStep::Symlink { path, target } => {
                let link = root.join(checked_relative(path));
                if let Some(parent) = link.parent() {
                    fs::create_dir_all(parent).expect("create the parent of a link");
                }
                std::os::unix::fs::symlink(target, &link)
                    .unwrap_or_else(|error| panic!("symlink {}: {error}", link.display()));
            }
            ArtifactStep::Folder { path } => {
                fs::create_dir_all(root.join(checked_relative(path))).expect("create a folder");
            }
            ArtifactStep::Chmod { path, mode } => {
                set_mode(&root.join(checked_relative(path)), *mode);
            }
            ArtifactStep::Stream { path, parts } => {
                stream_file(&root.join(checked_relative(path)), parts);
            }
            ArtifactStep::Remove { path } => {
                let target = root.join(checked_relative(path));
                let meta = fs::symlink_metadata(&target)
                    .unwrap_or_else(|error| panic!("remove {}: {error}", target.display()));
                if meta.is_dir() {
                    fs::remove_dir_all(&target).expect("remove a folder");
                } else {
                    fs::remove_file(&target).expect("remove a file");
                }
            }
            _ => unreachable!("not a raw step"),
        }
    }

    impl PlanStep for ArtifactStep {
        fn argv(&self) -> Option<Vec<String>> {
            use ArtifactStep as S;
            let argv = match self {
                S::NoSubcommand { kind } => vec![(*kind).to_owned()],
                S::Unknown { kind, subcommand } => vec![(*kind).to_owned(), positional(subcommand)],
                S::Import { kind, source, name } => {
                    assert!(
                        source.is_some() || name.is_none(),
                        "without a source, Go would read --name as the source"
                    );
                    let mut argv = command(kind, "import");
                    argv.extend(source.iter().map(|source| positional(source)));
                    if let Some(name) = name {
                        argv.push("--name".to_owned());
                        argv.push(flag_value(name));
                    }
                    argv
                }
                S::Update { kind, name, source } => {
                    assert!(
                        name.is_some() || source.is_none(),
                        "update takes the name first"
                    );
                    let mut argv = command(kind, "update");
                    argv.extend(name.iter().map(|name| positional(name)));
                    argv.extend(source.iter().map(|source| positional(source)));
                    argv
                }
                S::List { kind } => command(kind, "list"),
                S::Verify { kind, reference } => {
                    let mut argv = command(kind, "verify");
                    argv.extend(reference.iter().map(|reference| positional(reference)));
                    argv
                }
                S::Show { kind, reference } | S::History { kind, reference } => {
                    let sub = if matches!(self, S::Show { .. }) {
                        "show"
                    } else {
                        "history"
                    };
                    let mut argv = command(kind, sub);
                    argv.extend(reference.iter().map(|reference| positional(reference)));
                    argv
                }
                S::Read {
                    kind,
                    reference,
                    file,
                } => {
                    assert!(
                        reference.is_some() || file.is_none(),
                        "read takes the reference first"
                    );
                    let mut argv = command(kind, "read");
                    argv.extend(reference.iter().map(|reference| positional(reference)));
                    argv.extend(file.iter().map(|file| positional(file)));
                    argv
                }
                S::Export {
                    kind,
                    reference,
                    out,
                    target,
                    force,
                } => {
                    assert!(
                        reference.is_some() || (out.is_none() && target.is_none() && !force),
                        "without a reference, Go would read the first flag as the reference"
                    );
                    let mut argv = command(kind, "export");
                    argv.extend(reference.iter().map(|reference| positional(reference)));
                    if let Some(out) = out {
                        argv.push("--out".to_owned());
                        argv.push(flag_value(out));
                    }
                    if let Some(target) = target {
                        argv.push("--target".to_owned());
                        argv.push(flag_value(target));
                    }
                    if *force {
                        argv.push("--force".to_owned());
                    }
                    argv
                }
                S::Doctor { fix } => {
                    let mut argv = vec!["doctor".to_owned()];
                    if *fix {
                        argv.push("--fix".to_owned());
                    }
                    argv
                }
                S::Prime { compact, export } => {
                    let mut argv = vec!["prime".to_owned()];
                    if *compact {
                        argv.push("--compact".to_owned());
                    }
                    if *export {
                        argv.push("--export".to_owned());
                    }
                    argv
                }
                S::Onboard { mode } => {
                    let mut argv = vec!["onboard".to_owned()];
                    match mode {
                        Onboard::Apply => {}
                        Onboard::Check => argv.push("--check".to_owned()),
                        Onboard::Stdout => argv.push("--stdout".to_owned()),
                    }
                    argv
                }
                S::Sync { status_only } => {
                    let mut argv = vec!["sync".to_owned()];
                    if *status_only {
                        argv.push("--status".to_owned());
                    }
                    argv
                }
                S::Tree { .. }
                | S::Symlink { .. }
                | S::Folder { .. }
                | S::Chmod { .. }
                | S::Stream { .. }
                | S::Remove { .. } => return None,
            };
            Some(argv)
        }

        fn call(&self, beacon: &Beacon) -> Result<Done, BeaconError> {
            self.run(beacon, None)
                .map(|done| Done::Plan(Box::new(done)))
        }

        fn apply_raw(&self, root: &Path) {
            apply_raw_step(self, root);
        }

        fn describe_raw(&self) -> String {
            match self {
                ArtifactStep::Tree { dir, entries } => {
                    format!("tree {dir} ({} entries)", entries.len())
                }
                ArtifactStep::Symlink { path, target } => format!("symlink {path} -> {target}"),
                ArtifactStep::Folder { path } => format!("folder {path}"),
                ArtifactStep::Chmod { path, mode } => format!("chmod {mode:o} {path}"),
                ArtifactStep::Stream { path, parts } => {
                    format!("stream {path} ({} parts)", parts.len())
                }
                ArtifactStep::Remove { path } => format!("remove {path}"),
                _ => "command step".to_owned(),
            }
        }
    }

    impl ArtifactStep {
        /// A short text for failure messages.
        pub fn describe(&self) -> String {
            match self.argv() {
                Some(argv) => argv.join(" "),
                None => self.describe_raw(),
            }
        }
    }

    // ---- the result -------------------------------------------------------------

    /// What a library call of this plan returned.
    pub enum ArtifactDone {
        /// import and update: `{command, artifact}`.
        Stored {
            command: String,
            artifact: Artifact,
        },
        Listed {
            kind: &'static str,
            items: Vec<Artifact>,
        },
        Verified {
            kind: &'static str,
            checked: usize,
        },
        Shown {
            kind: &'static str,
            artifact: Artifact,
        },
        History {
            kind: &'static str,
            history: ArtifactHistory,
        },
        Read {
            kind: &'static str,
            read: ArtifactRead,
        },
        Exported {
            kind: &'static str,
            exported: ArtifactExported,
        },
        Doctor(DoctorReport),
        Prime(Prime),
        Onboarded(Onboarded),
        Synced(Synced),
    }

    /// An artifact as Go prints it inside a document: its compact line
    /// parsed back.
    pub fn artifact_value(artifact: &Artifact) -> Value {
        serde_json::from_str(&artifact.to_go_json()).expect("to_go_json writes valid JSON")
    }

    /// A stored file as Go prints it (`model.ArtifactFile`).
    pub fn file_value(file: &ArtifactFile) -> Value {
        json!({
            "path": file.path,
            "encoding": file.encoding,
            "content": file.content,
            "sha256": file.sha256,
            "executable": file.executable,
        })
    }

    fn doctor_status(status: &DoctorStatus) -> &'static str {
        match status {
            DoctorStatus::Pass => "pass",
            DoctorStatus::Warn => "warn",
            DoctorStatus::Fail => "fail",
        }
    }

    /// Doctor's report document (`cli.go:1490`).
    pub fn doctor_value(report: &DoctorReport) -> Value {
        let summary = report.summary();
        json!({
            "success": report.success(),
            "command": "doctor",
            "checks": report
                .checks
                .iter()
                .map(|check| json!({
                    "name": check.name,
                    "status": doctor_status(&check.status),
                    "message": check.message,
                    "details": check.details,
                    "fixable": check.fixable,
                }))
                .collect::<Vec<_>>(),
            "summary": {"pass": summary.pass, "warn": summary.warn, "fail": summary.fail},
        })
    }

    fn onboard_status(status: &OnboardStatus) -> &'static str {
        match status {
            OnboardStatus::Missing => "missing",
            OnboardStatus::Outdated => "outdated",
            OnboardStatus::Current => "current",
        }
    }

    fn onboard_action(action: &OnboardAction) -> &'static str {
        match action {
            OnboardAction::Created => "created",
            OnboardAction::Appended => "appended",
            OnboardAction::Updated => "updated",
            OnboardAction::Unchanged => "unchanged",
        }
    }

    impl ArtifactDone {
        /// The raw text Go prints instead of JSON (`onboard --stdout`).
        pub fn stdout_text(&self) -> Option<String> {
            match self {
                ArtifactDone::Onboarded(Onboarded::Snippet(snippet)) => Some(snippet.clone()),
                _ => None,
            }
        }
    }

    impl PlanDone for ArtifactDone {
        fn documents(&self) -> Vec<Value> {
            match self {
                ArtifactDone::Stored { command, artifact } => vec![json!({
                    "success": true,
                    "command": command,
                    "artifact": artifact_value(artifact),
                })],
                ArtifactDone::Listed { kind, items } => vec![json!({
                    "success": true,
                    "command": format!("{kind} list"),
                    "artifacts": items.iter().map(artifact_value).collect::<Vec<_>>(),
                    "count": items.len(),
                })],
                ArtifactDone::Verified { kind, checked } => vec![json!({
                    "success": true,
                    "command": format!("{kind} verify"),
                    "checked": checked,
                })],
                ArtifactDone::Shown { kind, artifact } => vec![json!({
                    "success": true,
                    "command": format!("{kind} show"),
                    "artifact": artifact_value(artifact),
                })],
                ArtifactDone::History { kind, history } => vec![json!({
                    "success": true,
                    "command": format!("{kind} history"),
                    "name": history.name,
                    "versions": history.versions.iter().map(artifact_value).collect::<Vec<_>>(),
                })],
                ArtifactDone::Read { kind, read } => vec![json!({
                    "success": true,
                    "command": format!("{kind} read"),
                    "name": read.name,
                    "version": read.version,
                    "file": file_value(&read.file),
                })],
                ArtifactDone::Exported { kind, exported } => vec![json!({
                    "success": true,
                    "command": format!("{kind} export"),
                    "name": exported.name,
                    "version": exported.version,
                    "path": exported.path.to_string_lossy(),
                })],
                ArtifactDone::Doctor(report) => {
                    let mut docs = vec![doctor_value(report)];
                    if !report.success() {
                        docs.push(error_document("doctor", "doctor found failures"));
                    }
                    docs
                }
                ArtifactDone::Prime(prime) => vec![json!({
                    "success": true,
                    "command": "prime",
                    "content": prime.go_json_text(),
                })],
                ArtifactDone::Onboarded(onboarded) => match onboarded {
                    Onboarded::Snippet(_) => Vec::new(),
                    Onboarded::Checked { file, status } => vec![json!({
                        "success": true,
                        "command": "onboard",
                        "status": onboard_status(status),
                        "file": file.to_string_lossy(),
                    })],
                    Onboarded::Applied { file, action } => vec![json!({
                        "success": true,
                        "command": "onboard",
                        "action": onboard_action(action),
                        "file": file.to_string_lossy(),
                    })],
                },
                ArtifactDone::Synced(synced) => match synced {
                    Synced::Status { files, uncommitted } => vec![json!({
                        "success": true,
                        "command": "sync",
                        "uncommitted": uncommitted,
                        "files": files,
                    })],
                    Synced::NothingToCommit => vec![json!({
                        "success": true,
                        "command": "sync",
                        "committed": false,
                        "message": "Nothing to commit",
                    })],
                    Synced::Committed { files, message } => vec![json!({
                        "success": true,
                        "command": "sync",
                        "committed": true,
                        "files": files,
                        "message": message,
                    })],
                },
            }
        }

        fn exit_code(&self) -> i32 {
            match self {
                ArtifactDone::Doctor(report) if !report.success() => 1,
                _ => 0,
            }
        }
    }

    // ---- files outside the store (AS54) -----------------------------------------

    /// One entry of a folder tree, as compared.
    #[derive(Debug, PartialEq, Eq)]
    pub enum Found {
        Folder {
            mode: u32,
        },
        File {
            mode: u32,
            size: u64,
            sha256: String,
        },
        /// A file this process cannot open (its mode is still compared).
        Unreadable {
            mode: u32,
        },
        /// A symbolic link: its target text with placeholders.
        Link {
            target: String,
        },
        Other,
    }

    fn file_digest(file: &mut fs::File) -> (u64, String) {
        let mut hasher = Sha256::new();
        let mut buffer = vec![0u8; 64 * 1024];
        let mut size = 0u64;
        loop {
            let read = file.read(&mut buffer).expect("read a compared file");
            if read == 0 {
                break;
            }
            size += read as u64;
            hasher.update(&buffer[..read]);
        }
        (size, hex(&hasher.finalize()))
    }

    /// Every entry under `base` except the root-relative paths in `skip`,
    /// without following links. Panics on a leftover export stage or backup.
    pub fn tree_entries(
        base: &Path,
        skip: &[&str],
        placeholders: &[(&Path, &str)],
    ) -> BTreeMap<Vec<u8>, Found> {
        let mut found = BTreeMap::new();
        let mut stack = vec![PathBuf::new()];
        while let Some(relative) = stack.pop() {
            let Ok(entries) = fs::read_dir(base.join(&relative)) else {
                continue;
            };
            for entry in entries {
                let entry = entry.expect("read a folder entry");
                let child = relative.join(entry.file_name());
                let key = child.as_os_str().as_bytes().to_vec();
                if skip.iter().any(|skipped| key == skipped.as_bytes()) {
                    continue;
                }
                let name = entry.file_name();
                let name = name.to_string_lossy();
                let path = base.join(&child);
                assert!(
                    !name.contains(".lbc-stage-") && !name.contains(".lbc-backup-"),
                    "an export stage or backup remains: {}",
                    path.display()
                );
                let meta = fs::symlink_metadata(&path).expect("metadata of a compared entry");
                let mode = meta.permissions().mode() & 0o7777;
                let item = if meta.file_type().is_symlink() {
                    let mut target = fs::read_link(&path)
                        .expect("read a link")
                        .to_string_lossy()
                        .into_owned();
                    for (folder, placeholder) in placeholders {
                        target = target.replace(&*folder.to_string_lossy(), placeholder);
                    }
                    Found::Link { target }
                } else if meta.is_dir() {
                    stack.push(child.clone());
                    Found::Folder { mode }
                } else if meta.is_file() {
                    match fs::File::open(&path) {
                        Ok(mut file) => {
                            let (size, sha256) = file_digest(&mut file);
                            Found::File { mode, size, sha256 }
                        }
                        Err(_) => Found::Unreadable { mode },
                    }
                } else {
                    Found::Other
                };
                found.insert(key, item);
            }
        }
        found
    }

    /// The names under `base` (lossy), every level, links not followed.
    pub fn names_under(base: &Path) -> Vec<String> {
        tree_entries(base, &[], &[])
            .keys()
            .map(|key| String::from_utf8_lossy(key).into_owned())
            .collect()
    }

    /// Compares two folder trees entry by entry.
    pub fn compare_trees(
        go: (&Path, &[(&Path, &str)]),
        lib: (&Path, &[(&Path, &str)]),
        skip: &[&str],
        context: &str,
    ) {
        let go_entries = tree_entries(go.0, skip, go.1);
        let lib_entries = tree_entries(lib.0, skip, lib.1);
        let names = |entries: &BTreeMap<Vec<u8>, Found>| {
            entries
                .keys()
                .map(|key| String::from_utf8_lossy(key).into_owned())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            names(&go_entries),
            names(&lib_entries),
            "{context}: the sets of files outside the store differ"
        );
        for (key, go_item) in &go_entries {
            assert_eq!(
                Some(go_item),
                lib_entries.get(key),
                "{context}: {} differs outside the store",
                String::from_utf8_lossy(key)
            );
        }
    }

    /// Compares every file of the two roots that the store comparison does
    /// not cover: type, bytes, mode and link target text (with `<root>`).
    pub fn compare_outside(go_root: &Path, lib_root: &Path, context: &str) {
        compare_trees(
            (go_root, &[(go_root, "<root>")]),
            (lib_root, &[(lib_root, "<root>")]),
            &STORE_SKIPS,
            context,
        );
    }

    // ---- Case wrappers -----------------------------------------------------------

    /// Runs an artifact step through the frozen [`Case`], then compares the
    /// files outside the store.
    pub fn art(case: &mut Case, step: ArtifactStep) -> Seen {
        assert!(
            !step.has_placeholders(),
            "a step naming per-side folders runs through Pair"
        );
        assert!(
            !matches!(step, ArtifactStep::Sync { .. }),
            "sync runs only through Pair"
        );
        let context = format!("after `{}`", step.describe());
        let seen = case.step(Step::Plan(Box::new(step)));
        compare_outside(case.go_root(), case.lib_root(), &context);
        seen
    }

    /// [`art`] for a step that fails with an operating-system error whose
    /// prefix (with `<root>`) is compared.
    pub fn art_os_error(case: &mut Case, step: ArtifactStep, prefix: &str) -> Seen {
        assert!(
            !step.has_placeholders(),
            "a step naming per-side folders runs through Pair"
        );
        let context = format!("after `{}`", step.describe());
        let seen = case.step_os_error(Step::Plan(Box::new(step)), prefix);
        compare_outside(case.go_root(), case.lib_root(), &context);
        seen
    }

    /// Runs a foundation step, then compares the files outside the store.
    pub fn plain(case: &mut Case, step: Step) -> Seen {
        let seen = case.step(step);
        compare_outside(case.go_root(), case.lib_root(), "after a foundation step");
        seen
    }

    /// Applies the same change to both roots (for files the raw steps do
    /// not write, such as a symlink inside the store), then compares.
    pub fn on_both(case: &mut Case, change: impl Fn(&Path)) {
        change(case.go_root());
        change(case.lib_root());
        case.check_stores();
        compare_outside(
            case.go_root(),
            case.lib_root(),
            "after a change on both roots",
        );
    }

    /// The config `init` wrote with the project set to [`PROJECT`], so that
    /// prompt ids are numbered too (AS50).
    pub fn project_config(case: &Case) -> Step {
        let path = case.lib_root().join(".luuna/beacon/config.yaml");
        let text = fs::read_to_string(&path).expect("read the config init wrote");
        assert!(
            text.contains("project: luuna-beacon\n"),
            "unexpected default config"
        );
        Step::WriteFile {
            path: ".luuna/beacon/config.yaml".to_owned(),
            bytes: text
                .replace("project: luuna-beacon\n", &format!("project: {PROJECT}\n"))
                .into_bytes(),
        }
    }

    /// Panics unless this process really cannot open `path` for reading
    /// (for example when the tests run as root).
    pub fn assert_unreadable(path: &Path) {
        assert!(
            fs::File::open(path).is_err(),
            "{} is still readable; run these tests as a user that file modes apply to",
            path.display()
        );
    }

    /// Panics unless this process really cannot open `path` for writing.
    pub fn assert_unwritable(path: &Path) {
        assert!(
            fs::OpenOptions::new().write(true).open(path).is_err(),
            "{} is still writable; run these tests as a user that file modes apply to",
            path.display()
        );
    }

    /// The permission bits of a path (links not followed).
    pub fn mode_of(path: &Path) -> u32 {
        fs::symlink_metadata(path)
            .unwrap_or_else(|error| panic!("metadata of {}: {error}", path.display()))
            .permissions()
            .mode()
            & 0o7777
    }

    /// The bytes of a file that is mode-protected against reading: the mode
    /// is lifted for the read and put back.
    pub fn read_protected(path: &Path) -> Vec<u8> {
        let mode = mode_of(path);
        set_mode(path, 0o600);
        let bytes = fs::read(path).expect("read a protected file");
        set_mode(path, mode);
        bytes
    }

    // ---- hand-written records ----------------------------------------------------

    /// Lowercase hexadecimal.
    pub fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    /// Go's `artifact.Digest`: lowercase hex sha256.
    pub fn digest(bytes: &[u8]) -> String {
        hex(&Sha256::digest(bytes))
    }

    /// `base64.StdEncoding.EncodeToString`.
    pub fn base64_std(bytes: &[u8]) -> String {
        const TABLE: &[u8; 64] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
        for chunk in bytes.chunks(3) {
            let b = [
                chunk[0],
                chunk.get(1).copied().unwrap_or(0),
                chunk.get(2).copied().unwrap_or(0),
            ];
            let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
            for index in 0..4 {
                if index <= chunk.len() {
                    out.push(char::from(TABLE[((n >> (18 - 6 * index)) & 63) as usize]));
                } else {
                    out.push('=');
                }
            }
        }
        out
    }

    /// A string as Go's `json.Marshal` writes it: HTML characters, U+2028
    /// and U+2029 escaped, `\b` and `\f` short forms (Go 1.22 and later),
    /// other control characters as `\u00XX` with lowercase digits.
    pub fn go_json_string(text: &str) -> String {
        let mut out = String::with_capacity(text.len() + 2);
        out.push('"');
        for character in text.chars() {
            match character {
                '"' => out.push_str("\\\""),
                '\\' => out.push_str("\\\\"),
                '\n' => out.push_str("\\n"),
                '\r' => out.push_str("\\r"),
                '\t' => out.push_str("\\t"),
                '\u{8}' => out.push_str("\\b"),
                '\u{c}' => out.push_str("\\f"),
                '<' | '>' | '&' | '\u{2028}' | '\u{2029}' => {
                    out.push_str(&format!("\\u{:04x}", u32::from(character)));
                }
                c if u32::from(c) < 0x20 => out.push_str(&format!("\\u{:04x}", u32::from(c))),
                c => out.push(c),
            }
        }
        out.push('"');
        out
    }

    /// One stored file of a hand-written record.
    #[derive(Clone, Debug)]
    pub struct RecordFile {
        pub path: String,
        pub encoding: String,
        pub content: String,
        pub sha256: String,
        pub executable: bool,
    }

    /// A file stored as `utf8` with the digest of its text.
    pub fn utf8_file(path: &str, text: &str) -> RecordFile {
        RecordFile {
            path: path.to_owned(),
            encoding: "utf8".to_owned(),
            content: text.to_owned(),
            sha256: digest(text.as_bytes()),
            executable: false,
        }
    }

    /// A file stored as `base64` with the digest of its bytes.
    pub fn base64_file(path: &str, bytes: &[u8]) -> RecordFile {
        RecordFile {
            path: path.to_owned(),
            encoding: "base64".to_owned(),
            content: base64_std(bytes),
            sha256: digest(bytes),
            executable: false,
        }
    }

    impl RecordFile {
        pub fn executable(mut self) -> Self {
            self.executable = true;
            self
        }

        /// Replaces the stored digest.
        pub fn with_sha256(mut self, sha256: &str) -> Self {
            self.sha256 = sha256.to_owned();
            self
        }

        /// Replaces encoding and content, keeping the digest.
        pub fn with_encoded(mut self, encoding: &str, content: &str) -> Self {
            self.encoding = encoding.to_owned();
            self.content = content.to_owned();
            self
        }

        fn line(&self) -> String {
            format!(
                "{{\"path\":{},\"encoding\":{},\"content\":{},\"sha256\":{},\"executable\":{}}}",
                go_json_string(&self.path),
                go_json_string(&self.encoding),
                go_json_string(&self.content),
                go_json_string(&self.sha256),
                self.executable
            )
        }
    }

    /// Go's `manifestBytes`: `[{path, sha256, executable}, ...]`.
    pub fn manifest_json(files: &[RecordFile]) -> String {
        let entries: Vec<String> = files
            .iter()
            .map(|file| {
                format!(
                    "{{\"path\":{},\"sha256\":{},\"executable\":{}}}",
                    go_json_string(&file.path),
                    go_json_string(&file.sha256),
                    file.executable
                )
            })
            .collect();
        format!("[{}]", entries.join(","))
    }

    /// A hand-written artifact record.
    #[derive(Clone, Debug)]
    pub struct Record {
        pub id: String,
        pub kind: String,
        pub name: String,
        pub version: i64,
        pub files: Vec<RecordFile>,
        /// `None`: the digest of the manifest of `files`.
        pub sha256: Option<String>,
        pub created_at: String,
        /// Keys written after `createdAt`, each with its raw JSON value.
        pub extra: Vec<(String, String)>,
    }

    /// A valid record (id `artifact-a0`, [`STAMP`]).
    pub fn record(kind: &str, name: &str, version: i64, files: Vec<RecordFile>) -> Record {
        Record {
            id: "artifact-a0".to_owned(),
            kind: kind.to_owned(),
            name: name.to_owned(),
            version,
            files,
            sha256: None,
            created_at: STAMP.to_owned(),
            extra: Vec::new(),
        }
    }

    impl Record {
        pub fn with_id(mut self, id: &str) -> Self {
            self.id = id.to_owned();
            self
        }

        pub fn with_sha256(mut self, sha256: &str) -> Self {
            self.sha256 = Some(sha256.to_owned());
            self
        }

        /// Adds a key after `createdAt` with a raw JSON value.
        pub fn with_key(mut self, key: &str, raw_json: &str) -> Self {
            self.extra.push((key.to_owned(), raw_json.to_owned()));
            self
        }

        /// The manifest digest of the files.
        pub fn manifest_sha256(&self) -> String {
            digest(manifest_json(&self.files).as_bytes())
        }

        /// The compact line without its newline: Go's `json.Marshal` of the
        /// record when no extra key is set.
        pub fn json(&self) -> String {
            let files: Vec<String> = self.files.iter().map(RecordFile::line).collect();
            let sha256 = self
                .sha256
                .clone()
                .unwrap_or_else(|| self.manifest_sha256());
            let mut out = format!(
                "{{\"id\":{},\"kind\":{},\"name\":{},\"version\":{},\"files\":[{}],\"sha256\":{},\"createdAt\":{}",
                go_json_string(&self.id),
                go_json_string(&self.kind),
                go_json_string(&self.name),
                self.version,
                files.join(","),
                go_json_string(&sha256),
                go_json_string(&self.created_at)
            );
            for (key, raw) in &self.extra {
                out.push(',');
                out.push_str(&go_json_string(key));
                out.push(':');
                out.push_str(raw);
            }
            out.push('}');
            out
        }

        /// [`Record::json`] with its newline.
        pub fn line(&self) -> String {
            format!("{}\n", self.json())
        }
    }

    /// A `Step::WriteFile` of `artifacts.jsonl` with these lines.
    pub fn write_artifacts(lines: &[String]) -> Step {
        Step::WriteFile {
            path: ".luuna/beacon/artifacts.jsonl".to_owned(),
            bytes: lines.concat().into_bytes(),
        }
    }

    // ---- the git helper ------------------------------------------------------------

    fn is_git_marker(dot_git: &Path) -> bool {
        dot_git.is_file() || dot_git.join("HEAD").exists()
    }

    /// `git`, resolved once from the test process's own PATH (never in the
    /// library). Panics when there is none.
    pub fn host_git() -> &'static PathBuf {
        static GIT: OnceLock<PathBuf> = OnceLock::new();
        GIT.get_or_init(|| {
            let path = std::env::var_os("PATH")
                .unwrap_or_else(|| panic!("the test process has no PATH, so git cannot be found"));
            for folder in std::env::split_paths(&path) {
                if !folder.is_absolute() {
                    continue;
                }
                let candidate = folder.join("git");
                if let Ok(meta) = fs::metadata(&candidate) {
                    if meta.is_file() && meta.permissions().mode() & 0o111 != 0 {
                        return candidate;
                    }
                }
            }
            panic!("git was not found on the test process's PATH")
        })
    }

    /// A test-made folder with `bin/git` (a link to [`host_git`]), an empty
    /// `empty/` for the missing-git case, HOME, XDG and TMPDIR folders, an
    /// empty global git config and an empty hooks folder.
    pub struct GitHelper {
        _dir: TempDir,
        dir: PathBuf,
    }

    impl GitHelper {
        pub fn new() -> GitHelper {
            let holder = tempfile::Builder::new()
                .prefix("beacon-git-helper-")
                .tempdir()
                .expect("create the git helper folder");
            let dir = holder
                .path()
                .canonicalize()
                .expect("canonical helper folder");
            for folder in ["bin", "empty", "home", "xdg-config", "tmp", "no-hooks"] {
                fs::create_dir_all(dir.join(folder)).expect("create a helper folder");
            }
            fs::write(dir.join("gitconfig"), b"").expect("write the empty global config");
            std::os::unix::fs::symlink(host_git(), dir.join("bin").join("git"))
                .expect("link git into the helper's bin");
            GitHelper { _dir: holder, dir }
        }

        /// The helper folder.
        pub fn dir(&self) -> &Path {
            &self.dir
        }

        /// The complete environment of every Go, git and library start of a
        /// side whose root is `root` (`test_isolation`).
        pub fn environment(&self, root: &Path, missing_git: bool) -> Vec<(OsString, OsString)> {
            let folder = |name: &str| self.dir.join(name).into_os_string();
            let ceiling = root
                .parent()
                .expect("a project root has a parent")
                .as_os_str()
                .to_owned();
            vec![
                (
                    "PATH".into(),
                    folder(if missing_git { "empty" } else { "bin" }),
                ),
                ("HOME".into(), folder("home")),
                ("XDG_CONFIG_HOME".into(), folder("xdg-config")),
                ("TMPDIR".into(), folder("tmp")),
                ("GIT_CONFIG_NOSYSTEM".into(), "1".into()),
                ("GIT_CONFIG_GLOBAL".into(), folder("gitconfig")),
                ("GIT_TERMINAL_PROMPT".into(), "0".into()),
                ("GIT_CEILING_DIRECTORIES".into(), ceiling),
                ("LC_ALL".into(), "C".into()),
                ("LANG".into(), "C".into()),
                ("TZ".into(), "UTC".into()),
            ]
        }

        fn command(&self, root: &Path) -> Command {
            let mut command = Command::new(host_git());
            command
                .env_clear()
                .envs(self.environment(root, false))
                .arg("-C")
                .arg(root)
                .stdin(Stdio::null());
            command
        }

        /// Runs `git -C <root> <args>` after the guard. `repository` says
        /// whether this helper ran `git init` in `root`.
        pub fn run(&self, root: &Path, repository: bool, args: &[&str]) -> Output {
            pair_guard(root, repository.then_some(self));
            self.command(root)
                .args(args)
                .output()
                .unwrap_or_else(|error| panic!("could not start git: {error}"))
        }

        /// [`GitHelper::run`], panicking unless git succeeds.
        pub fn ok(&self, root: &Path, repository: bool, args: &[&str]) -> Output {
            let output = self.run(root, repository, args);
            assert!(
                output.status.success(),
                "git {} in {} failed: {}{}",
                args.join(" "),
                root.display(),
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            output
        }

        /// `git init` and the repository-local settings, so no identity,
        /// signing, hook or alias of the host applies.
        fn init_repository(&self, root: &Path) {
            self.ok(root, false, &["init", "-q"]);
            let hooks = self.dir.join("no-hooks");
            let hooks = hooks.to_str().expect("helper folder is UTF-8");
            for setting in [
                ["user.name", "Beacon Test"],
                ["user.email", "beacon-test@example.invalid"],
                ["commit.gpgsign", "false"],
                ["tag.gpgsign", "false"],
                ["core.autocrlf", "false"],
                ["core.hooksPath", hooks],
            ] {
                self.ok(root, true, &["config", setting[0], setting[1]]);
            }
        }
    }

    impl Default for GitHelper {
        fn default() -> Self {
            Self::new()
        }
    }

    /// Panics unless `root` may be used by a Pair or the git helper: absolute
    /// and canonical; under the canonical system temporary folder; not inside
    /// the workspace; no strict ancestor is a git repository; a `.git` in the
    /// root itself exists only when `own_repository` (the helper that ran
    /// `git init` there) is given, and git then names the root as the top
    /// level; and not inside the reference clone (checked last, since it
    /// builds the Go tool).
    pub fn pair_guard(root: &Path, own_repository: Option<&GitHelper>) {
        assert!(
            root.is_absolute(),
            "pair guard: {} is not absolute",
            root.display()
        );
        let canonical = root.canonicalize().unwrap_or_else(|error| {
            panic!("pair guard: {} cannot be opened: {error}", root.display())
        });
        assert!(
            canonical.as_path() == root,
            "pair guard: {} is not canonical",
            root.display()
        );
        let temp = std::env::temp_dir()
            .canonicalize()
            .expect("the system temporary folder exists");
        assert!(
            root.starts_with(&temp),
            "pair guard: {} is not under the temporary folder {}",
            root.display(),
            temp.display()
        );
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
        for folder in manifest.ancestors().take(3) {
            let folder = folder
                .canonicalize()
                .unwrap_or_else(|_| folder.to_path_buf());
            assert!(
                !root.starts_with(&folder),
                "pair guard: {} is inside the workspace folder {}",
                root.display(),
                folder.display()
            );
        }
        for folder in root.ancestors().skip(1) {
            assert!(
                !is_git_marker(&folder.join(".git")),
                "pair guard: {} is inside the git repository {}",
                root.display(),
                folder.display()
            );
        }
        if fs::symlink_metadata(root.join(".git")).is_ok() {
            let helper = own_repository.unwrap_or_else(|| {
                panic!(
                    "pair guard: {} has a .git that this Pair did not create",
                    root.display()
                )
            });
            let output = helper
                .command(root)
                .args(["rev-parse", "--show-toplevel"])
                .output()
                .unwrap_or_else(|error| panic!("pair guard: could not start git: {error}"));
            assert!(
                output.status.success()
                    && String::from_utf8_lossy(&output.stdout).trim_end() == root.to_string_lossy(),
                "pair guard: git does not name {} as its own top level: {}",
                root.display(),
                String::from_utf8_lossy(&output.stdout)
            );
        }
        assert!(
            !root.starts_with(&go::go_build().clone),
            "pair guard: {} is inside the reference clone",
            root.display()
        );
    }

    // ---- the Pair runner (AS53) -------------------------------------------------------

    /// What git a Pair offers.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Git {
        /// `git` on PATH, no repository.
        Available,
        /// `git` on PATH and a test-made repository in each root.
        Repository,
        /// PATH is an empty folder.
        Missing,
    }

    struct Side {
        _dir: TempDir,
        root: PathBuf,
        _outside_dir: TempDir,
        outside: PathBuf,
        norm: Normaliser,
        repository: bool,
    }

    impl Side {
        fn new(label: &str) -> Side {
            let (dir, root) = fresh_root(label);
            let outside_dir = tempfile::Builder::new()
                .prefix("beacon-outside-")
                .tempdir()
                .expect("create an outside folder");
            let outside = outside_dir
                .path()
                .canonicalize()
                .expect("canonical outside folder");
            Side {
                norm: Normaliser::new(PROJECT, &root),
                _dir: dir,
                root,
                _outside_dir: outside_dir,
                outside,
                repository: false,
            }
        }

        fn placeholders(&self) -> [(&Path, &str); 2] {
            [(&self.root, "<root>"), (&self.outside, "<outside>")]
        }
    }

    /// One side's result of a Pair step.
    #[derive(Debug, Clone)]
    pub struct SideRun {
        pub code: i32,
        /// Documents before normalisation.
        pub raw: Vec<Value>,
        /// Documents after normalisation (`<root>`, `<outside>`, ids,
        /// timestamps, the hook's order).
        pub documents: Vec<Value>,
        /// The error of the first error document, normalised.
        pub error: Option<String>,
        pub migrated: bool,
        /// Raw stdout of a step that prints no JSON.
        pub stdout: Vec<u8>,
    }

    impl SideRun {
        fn empty() -> SideRun {
            SideRun {
                code: 0,
                raw: Vec::new(),
                documents: Vec::new(),
                error: None,
                migrated: false,
                stdout: Vec::new(),
            }
        }
    }

    /// Both sides of a step that the test compares itself.
    pub struct Apart {
        pub go: SideRun,
        pub lib: SideRun,
    }

    fn is_error_document(doc: &Value) -> bool {
        doc.as_object().is_some_and(|object| {
            object.len() == 3
                && object.get("success") == Some(&Value::Bool(false))
                && object.contains_key("command")
                && object.get("error").is_some_and(Value::is_string)
        })
    }

    fn take_errors(docs: &mut [Value]) -> Vec<String> {
        let mut errors = Vec::new();
        for doc in docs.iter_mut() {
            if is_error_document(doc) {
                errors.push(doc["error"].as_str().unwrap_or_default().to_owned());
                doc["error"] = Value::String("<error>".to_owned());
            }
        }
        errors
    }

    fn replace_in_strings(value: &mut Value, from: &str, to: &str) {
        match value {
            Value::String(text) => {
                if text.contains(from) {
                    *text = text.replace(from, to);
                }
            }
            Value::Array(items) => items
                .iter_mut()
                .for_each(|item| replace_in_strings(item, from, to)),
            Value::Object(object) => object
                .values_mut()
                .for_each(|item| replace_in_strings(item, from, to)),
            _ => {}
        }
    }

    /// The checks whose failing details may carry Go parser or OS texts.
    const PARSER_CHECKS: [&str; 4] = [
        "config",
        "prompts-integrity",
        "schemas-integrity",
        "artifacts-integrity",
    ];

    /// The recognised prefix of a parser or OS text: `yaml: `,
    /// `invalid JSONL line <n> in <path>: ` or `<op> <path>: `.
    pub fn parser_prefix(text: &str) -> Option<String> {
        if text.starts_with("yaml: ") {
            return Some("yaml: ".to_owned());
        }
        let operations = ["invalid JSONL line ", "open ", "read ", "stat ", "lstat "];
        if operations
            .iter()
            .any(|operation| text.starts_with(operation))
        {
            let end = text.find(": ")? + 2;
            return Some(text[..end].to_owned());
        }
        None
    }

    /// For doctor documents only: in the failing details of the four store
    /// checks, a Go text with a recognised prefix is compared by that prefix,
    /// and both strings become `<prefix><parser text>`. Every other field is
    /// left for the exact comparison (AS55).
    pub fn exempt_parser_texts(go: &mut [Value], lib: &mut [Value], context: &str) {
        for (go_doc, lib_doc) in go.iter_mut().zip(lib.iter_mut()) {
            if go_doc.get("command").and_then(Value::as_str) != Some("doctor")
                || lib_doc.get("command").and_then(Value::as_str) != Some("doctor")
            {
                continue;
            }
            let (Some(go_checks), Some(lib_checks)) = (
                go_doc.get_mut("checks").and_then(Value::as_array_mut),
                lib_doc.get_mut("checks").and_then(Value::as_array_mut),
            ) else {
                continue;
            };
            for (go_check, lib_check) in go_checks.iter_mut().zip(lib_checks.iter_mut()) {
                let name = go_check
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if !PARSER_CHECKS.contains(&name)
                    || go_check.get("status").and_then(Value::as_str) != Some("fail")
                    || lib_check.get("name") != go_check.get("name")
                {
                    continue;
                }
                let name = name.to_owned();
                let (Some(go_details), Some(lib_details)) = (
                    go_check.get_mut("details").and_then(Value::as_array_mut),
                    lib_check.get_mut("details").and_then(Value::as_array_mut),
                ) else {
                    continue;
                };
                if go_details.len() != lib_details.len() {
                    continue;
                }
                for (go_detail, lib_detail) in go_details.iter_mut().zip(lib_details.iter_mut()) {
                    let Some(prefix) = go_detail.as_str().and_then(parser_prefix) else {
                        continue;
                    };
                    let lib_text = lib_detail.as_str().unwrap_or_default();
                    assert!(
                        lib_text.starts_with(&prefix),
                        "{context}: doctor {name} detail: expected the prefix {prefix:?}; library {lib_text:?}"
                    );
                    *go_detail = Value::String(format!("{prefix}<parser text>"));
                    *lib_detail = Value::String(format!("{prefix}<parser text>"));
                }
            }
        }
    }

    /// The UTC date as Go formats it for the sync message.
    pub fn utc_day() -> String {
        chrono::Utc::now().format("%Y-%m-%d").to_string()
    }

    fn sync_day(text: &str) -> Option<&str> {
        let rest = text.strip_prefix("luuna-beacon: sync ")?;
        let valid = rest.len() == 10
            && rest.bytes().enumerate().all(|(index, byte)| {
                if index == 4 || index == 7 {
                    byte == b'-'
                } else {
                    byte.is_ascii_digit()
                }
            });
        valid.then_some(rest)
    }

    /// Replaces `luuna-beacon: sync <YYYY-MM-DD>` in free text.
    fn blank_sync_dates(text: &str) -> String {
        let marker = "luuna-beacon: sync ";
        let mut out = String::with_capacity(text.len());
        let mut rest = text;
        while let Some(at) = rest.find(marker) {
            let after = &rest[at..];
            out.push_str(&rest[..at]);
            match after.get(..marker.len() + 10).and_then(sync_day) {
                Some(_) => {
                    out.push_str(marker);
                    out.push_str("<date>");
                    rest = &after[marker.len() + 10..];
                }
                None => {
                    out.push_str(marker);
                    rest = &after[marker.len()..];
                }
            }
        }
        out.push_str(rest);
        out
    }

    /// For sync documents: both dates equal the UTC day (or, if the day
    /// changed during the step, one of the two days); both become `<date>`.
    fn reconcile_sync_dates(
        go: &mut [Value],
        lib: &mut [Value],
        days: (&str, &str),
        context: &str,
    ) {
        for (go_doc, lib_doc) in go.iter_mut().zip(lib.iter_mut()) {
            if go_doc.get("command").and_then(Value::as_str) != Some("sync") {
                continue;
            }
            let (Some(go_message), Some(lib_message)) = (
                go_doc
                    .get("message")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                lib_doc
                    .get("message")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            ) else {
                continue;
            };
            let (Some(go_day), Some(lib_day)) = (sync_day(&go_message), sync_day(&lib_message))
            else {
                continue;
            };
            if days.0 == days.1 {
                assert_eq!(
                    go_day, days.0,
                    "{context}: Go's sync date is not today (UTC)"
                );
                assert_eq!(
                    lib_day, days.0,
                    "{context}: the library's sync date is not today (UTC)"
                );
            } else {
                for day in [go_day, lib_day] {
                    assert!(
                        day == days.0 || day == days.1,
                        "{context}: sync date {day} is neither {} nor {}",
                        days.0,
                        days.1
                    );
                }
            }
            go_doc["message"] = Value::String("luuna-beacon: sync <date>".to_owned());
            lib_doc["message"] = Value::String("luuna-beacon: sync <date>".to_owned());
        }
    }

    /// Two roots (Go and library), each with its own outside folder, and a
    /// git helper; every comparison of the frozen Case plus the files
    /// outside the store, the outside folders, and the git state after sync.
    pub struct Pair {
        go: Side,
        lib: Side,
        beacon: Beacon,
        helper: GitHelper,
        git: Git,
        written: HashSet<String>,
        log: Vec<String>,
    }

    impl Pair {
        pub fn new(git: Git) -> Pair {
            let helper = GitHelper::new();
            let go = Side::new("pair-go");
            let lib = Side::new("pair-lib");
            let beacon = Beacon::open(&lib.root).expect("open the library project root");
            let mut pair = Pair {
                go,
                lib,
                beacon,
                helper,
                git,
                written: HashSet::new(),
                log: Vec::new(),
            };
            if git == Git::Repository {
                pair.helper.init_repository(&pair.go.root);
                pair.go.repository = true;
                pair.helper.init_repository(&pair.lib.root);
                pair.lib.repository = true;
                pair_guard(&pair.go.root, Some(&pair.helper));
                pair_guard(&pair.lib.root, Some(&pair.helper));
            }
            pair
        }

        pub fn go_root(&self) -> &Path {
            &self.go.root
        }

        pub fn lib_root(&self) -> &Path {
            &self.lib.root
        }

        pub fn go_outside(&self) -> &Path {
            &self.go.outside
        }

        pub fn lib_outside(&self) -> &Path {
            &self.lib.outside
        }

        pub fn beacon(&self) -> &Beacon {
            &self.beacon
        }

        fn context(&mut self, description: String) -> String {
            self.log.push(description.clone());
            format!(
                "pair step {} `{description}` (steps so far: {})",
                self.log.len(),
                self.log.join(" | ")
            )
        }

        fn missing_git(&self) -> bool {
            self.git == Git::Missing
        }

        /// Runs a foundation step (Init, Create, WriteFile, SetMtime, ...)
        /// on both sides and compares.
        pub fn step(&mut self, step: Step) -> SideRun {
            let context = self.context(super::super::case::describe(&step));
            match &step {
                Step::WriteFile { path, bytes } => {
                    for root in [&self.go.root, &self.lib.root] {
                        let target = root.join(path);
                        if let Some(parent) = target.parent() {
                            fs::create_dir_all(parent).expect("create the parent of a raw file");
                        }
                        fs::write(&target, bytes).expect("write a raw file");
                    }
                    self.written.insert(path.clone());
                    self.compare_all(&context);
                    return SideRun::empty();
                }
                Step::SetMtime { path, seconds_ago } => {
                    for root in [&self.go.root, &self.lib.root] {
                        let file = fs::File::options()
                            .write(true)
                            .open(root.join(path))
                            .expect("open the file whose mtime is set");
                        file.set_modified(SystemTime::now() - Duration::from_secs(*seconds_ago))
                            .expect("set the modification time");
                    }
                    self.written.insert(path.clone());
                    self.compare_all(&context);
                    return SideRun::empty();
                }
                Step::Plan(_) => panic!("use Pair::art for this plan's steps"),
                Step::Render { .. } | Step::RenderJson(_) | Step::Validate(_) => {
                    panic!("steps with runtime fields run through Case")
                }
                _ => {}
            }
            let argv = lower(&step).argv.expect("a command step has an argv");
            let (code, docs, _, stderr) = self.go_run(&argv, false);
            pair_guard(&self.lib.root, self.lib.repository.then_some(&self.helper));
            let lib: LibRun = run_lib(&self.beacon, &step);
            let go_migrated = String::from_utf8_lossy(&stderr).contains(MIGRATED_LINE);
            let go = side_run(&mut self.go, code, docs, go_migrated, Vec::new());
            let lib = side_run(
                &mut self.lib,
                lib.code,
                lib.documents,
                lib.migrated,
                Vec::new(),
            );
            let day = utc_day();
            self.compare_runs(&go, &lib, None, (&day, &day), &context);
            self.compare_all(&context);
            go
        }

        /// Runs one of this plan's steps on both sides and compares
        /// everything; returns Go's side.
        pub fn art(&mut self, step: ArtifactStep) -> SideRun {
            self.art_with(step, None)
        }

        /// [`Pair::art`] for a step that fails with an operating-system
        /// error whose prefix (with `<root>`/`<outside>`) is compared.
        pub fn art_os_error(&mut self, step: ArtifactStep, prefix: &str) -> SideRun {
            self.art_with(step, Some(prefix))
        }

        fn art_with(&mut self, step: ArtifactStep, os_prefix: Option<&str>) -> SideRun {
            let context = self.context(step.describe());
            if step.argv().is_none() {
                for side in [&self.go, &self.lib] {
                    apply_raw_step(&step.resolved(&side.root, &side.outside), &side.root);
                }
                self.compare_all(&context);
                return SideRun::empty();
            }
            let before = utc_day();
            let apart = self.run_both(&step);
            let after = utc_day();
            self.compare_runs(
                &apart.go,
                &apart.lib,
                os_prefix,
                (&before, &after),
                &context,
            );
            self.compare_all(&context);
            if self.git == Git::Repository && matches!(step, ArtifactStep::Sync { .. }) {
                self.compare_git(&context);
            }
            apart.go
        }

        /// Runs a command step on both sides without comparing them (for the
        /// A8 refusals, where Go and the library differ by design).
        pub fn apart(&mut self, step: ArtifactStep) -> Apart {
            let description = step.describe();
            self.context(description);
            assert!(step.argv().is_some(), "apart runs command steps");
            self.run_both(&step)
        }

        /// Applies the same change to both sides (root, outside folder), then
        /// compares stores and trees.
        pub fn both(&mut self, change: impl Fn(&Path, &Path)) {
            let context = self.context("change on both sides".to_owned());
            change(&self.go.root, &self.go.outside);
            change(&self.lib.root, &self.lib.outside);
            self.compare_all(&context);
        }

        /// Runs `git add <path>` or `git commit -q -m <text>` in both
        /// test-made repositories (to prepare a sync case).
        pub fn git_both(&mut self, args: &[&str]) {
            assert!(
                self.git == Git::Repository,
                "git_both needs Git::Repository"
            );
            assert!(
                matches!(args, ["add", _] | ["commit", "-q", "-m", _]),
                "git_both runs only `add <path>` and `commit -q -m <text>`: {args:?}"
            );
            let context = self.context(format!("git {}", args.join(" ")));
            for root in [&self.go.root, &self.lib.root] {
                self.helper.ok(root, true, args);
            }
            self.compare_all(&context);
        }

        /// A read-only git command in the library side's repository; its
        /// stdout with `<root>`.
        pub fn lib_git(&self, args: &[&str]) -> String {
            assert!(
                matches!(args.first(), Some(&("log" | "show" | "status" | "diff"))),
                "lib_git runs only log, show, status and diff"
            );
            let output = self.helper.run(&self.lib.root, self.lib.repository, args);
            self.lib.norm.text(&String::from_utf8_lossy(&output.stdout))
        }

        fn go_run(&self, argv: &[String], raw_stdout: bool) -> (i32, Vec<Value>, Vec<u8>, Vec<u8>) {
            pair_guard(&self.go.root, self.go.repository.then_some(&self.helper));
            let build = go::go_build();
            let output = Command::new(&build.binary)
                .env_clear()
                .envs(self.helper.environment(&self.go.root, self.missing_git()))
                .current_dir(&self.go.root)
                .args(argv)
                .arg("--json")
                .stdin(Stdio::null())
                .output()
                .unwrap_or_else(|error| {
                    panic!("could not start {}: {error}", build.binary.display())
                });
            let code = output.status.code().unwrap_or_else(|| {
                panic!("the Go binary was stopped by a signal: {}", output.status)
            });
            let documents = if raw_stdout {
                Vec::new()
            } else {
                serde_json::Deserializer::from_slice(&output.stdout)
                    .into_iter::<Value>()
                    .collect::<Result<Vec<_>, _>>()
                    .unwrap_or_else(|error| {
                        panic!(
                            "Go's stdout for {argv:?} is not a stream of JSON values ({error}):\n{}",
                            String::from_utf8_lossy(&output.stdout)
                        )
                    })
            };
            (code, documents, output.stdout, output.stderr)
        }

        fn lib_run(&self, step: &ArtifactStep) -> (LibRun, Vec<u8>) {
            pair_guard(&self.lib.root, self.lib.repository.then_some(&self.helper));
            let argv = step.argv().expect("a command step");
            let command = argv[0].clone();
            let failed = |error: BeaconError, migrated: bool| {
                let message = error.to_string();
                LibRun {
                    code: 1,
                    documents: vec![error_document(&command, &message)],
                    ok: false,
                    error: Some(message),
                    migrated,
                }
            };
            let migrated = match step.migration() {
                Migration::Required => match self.beacon.migrate_legacy() {
                    Ok(moved) => moved,
                    Err(error) => return (failed(error, false), Vec::new()),
                },
                Migration::Ignored => self.beacon.migrate_legacy().unwrap_or(false),
                Migration::Never => false,
            };
            let git = GitRunner {
                program: OsString::from("git"),
                environment: GitEnvironment::Exact(
                    self.helper.environment(&self.lib.root, self.missing_git()),
                ),
            };
            match step.run(&self.beacon, Some(git)) {
                Ok(done) => {
                    let stdout = done.stdout_text().unwrap_or_default().into_bytes();
                    (
                        LibRun {
                            code: done.exit_code(),
                            documents: done.documents(),
                            ok: true,
                            error: None,
                            migrated,
                        },
                        stdout,
                    )
                }
                Err(error) => (failed(error, migrated), Vec::new()),
            }
        }

        fn run_both(&mut self, step: &ArtifactStep) -> Apart {
            let go_step = step.resolved(&self.go.root, &self.go.outside);
            let lib_step = step.resolved(&self.lib.root, &self.lib.outside);
            let raw_stdout = step.prints_raw();
            let argv = go_step.argv().expect("a command step");
            let (code, docs, stdout, stderr) = self.go_run(&argv, raw_stdout);
            let go_migrated = String::from_utf8_lossy(&stderr).contains(MIGRATED_LINE);
            let (lib, lib_stdout) = self.lib_run(&lib_step);
            let go_stdout = if raw_stdout { stdout } else { Vec::new() };
            let go = side_run(&mut self.go, code, docs, go_migrated, go_stdout);
            let lib = side_run(
                &mut self.lib,
                lib.code,
                lib.documents,
                lib.migrated,
                lib_stdout,
            );
            Apart { go, lib }
        }

        fn compare_runs(
            &self,
            go: &SideRun,
            lib: &SideRun,
            os_prefix: Option<&str>,
            days: (&str, &str),
            context: &str,
        ) {
            assert_eq!(go.code, lib.code, "{context}: exit status differs");
            assert_eq!(
                go.migrated, lib.migrated,
                "{context}: Go's `{MIGRATED_LINE}` stderr line against the library's migration"
            );
            assert_eq!(
                String::from_utf8_lossy(&go.stdout),
                String::from_utf8_lossy(&lib.stdout),
                "{context}: raw stdout differs"
            );
            let mut go_docs = go.documents.clone();
            let mut lib_docs = lib.documents.clone();
            reconcile_sync_dates(&mut go_docs, &mut lib_docs, days, context);
            exempt_parser_texts(&mut go_docs, &mut lib_docs, context);
            let go_errors = take_errors(&mut go_docs);
            let lib_errors = take_errors(&mut lib_docs);
            assert_eq!(
                go_errors.len(),
                lib_errors.len(),
                "{context}: error documents differ: Go {go_errors:?}, library {lib_errors:?}"
            );
            for (go_error, lib_error) in go_errors.iter().zip(&lib_errors) {
                assert_errors_match(go_error, lib_error, os_prefix, context);
            }
            assert_eq!(go_docs, lib_docs, "{context}: stdout documents differ");
        }

        /// Compares the stores (as the frozen Case does), the files outside
        /// the stores, and the two outside folders.
        pub fn compare_all(&mut self, context: &str) {
            let go = snapshot(&self.go.root, &mut self.go.norm, &self.written);
            let lib = snapshot(&self.lib.root, &mut self.lib.norm, &self.written);
            assert_eq!(
                go.keys().collect::<Vec<_>>(),
                lib.keys().collect::<Vec<_>>(),
                "{context}: the sets of store files differ"
            );
            for (path, go_file) in &go {
                let lib_file = &lib[path];
                assert_eq!(
                    String::from_utf8_lossy(&go_file.bytes),
                    String::from_utf8_lossy(&lib_file.bytes),
                    "{context}: bytes of {path} differ after normalisation"
                );
                assert_eq!(
                    format!("{:o}", go_file.mode),
                    format!("{:o}", lib_file.mode),
                    "{context}: permission bits of {path} differ"
                );
            }
            let go_marks = self.go.placeholders();
            let lib_marks = self.lib.placeholders();
            compare_trees(
                (&self.go.root, &go_marks),
                (&self.lib.root, &lib_marks),
                &STORE_SKIPS,
                context,
            );
            compare_trees(
                (&self.go.outside, &go_marks),
                (&self.lib.outside, &lib_marks),
                &[],
                context,
            );
        }

        /// After a sync step: log (subject, author, e-mail), the files of
        /// HEAD when a commit exists, the status and the staged changes, run
        /// by the helper on both sides. Hashes and dates are never compared.
        fn compare_git(&self, context: &str) {
            let text = |side: &Side, output: &Output| {
                blank_sync_dates(&side.norm.text(&format!(
                    "exit {:?}\n{}\n--stderr--\n{}",
                    output.status.code(),
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                )))
            };
            let both = |args: &[&str]| {
                let go = self.helper.run(&self.go.root, true, args);
                let lib = self.helper.run(&self.lib.root, true, args);
                assert_eq!(
                    text(&self.go, &go),
                    text(&self.lib, &lib),
                    "{context}: `git {}` differs",
                    args.join(" ")
                );
                lib.status.success()
            };
            let has_commit = both(&["log", "--format=%s%x00%an%x00%ae"]);
            if has_commit {
                both(&["show", "--name-status", "--format=", "HEAD"]);
            }
            both(&["status", "--porcelain=v1", "--untracked-files=all"]);
            both(&["diff", "--cached", "--name-status"]);
        }
    }

    fn side_run(
        side: &mut Side,
        code: i32,
        docs: Vec<Value>,
        migrated: bool,
        stdout: Vec<u8>,
    ) -> SideRun {
        let raw = docs.clone();
        let mut documents = docs;
        let outside = side.outside.to_string_lossy().into_owned();
        for doc in documents.iter_mut() {
            replace_in_strings(doc, &outside, "<outside>");
        }
        side.norm.documents(&mut documents);
        let error = documents
            .iter()
            .find(|doc| is_error_document(doc))
            .and_then(|doc| doc["error"].as_str())
            .map(str::to_owned);
        SideRun {
            code,
            raw,
            documents,
            error,
            migrated,
            stdout,
        }
    }
}
