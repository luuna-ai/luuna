//! Owner: plan s3-beacon-lib-2.
//!
//! The plan's `PlanStep` and `PlanDone` implementations and the
//! normalisation for its own commands live here. The hook must leave the
//! documents of every other command unchanged.
//!
//! - [`sort_map_ordered`]: the hook for this plan's documents (it only
//!   reorders arrays Go builds from map iteration);
//! - one [`PlanStep`] and one [`PlanDone`] per command (stats, tree, pin,
//!   unpin, diff, emit, import, config schema, config show, config show
//!   --path, config set, config unset), each argv built as `cli.go` reads it
//!   and each document written field for field as `cli.go` prints it
//!   (reference commit 1c34973: stats `:752`, tree `:788`, pin `:839`,
//!   config `:848`, `:871`, `:879`, `:901`, `:916`, emit `:729`, `:735`,
//!   import `:1647`, diff `:1698`);
//! - [`run_go_text`], the Go tool without `--json` (render, config show);
//! - [`compare_outside_store`], [`tree_bytes`], [`nested_roots`],
//!   [`assert_memory_limited`] and [`Pair`], a two-root runner for the cases
//!   that cannot use `Case` (nested roots, the A8 demonstrations);
//! - the inputs of the observed Go runs `go-lossy-probe-1` and
//!   `go-lossy-probe-2`, each named by its probe id.

use super::case::{self, Step};
use super::go::{self, GoRun};
use super::normalise::Normaliser;
use super::payload::{self, Done, PlanDone};
use luuna_beacon::{
    Beacon, BeaconError, Clock, ConfigShow, EmitOptions, EmitReport, EmitSelection, ImportMarkdown,
    Imported, Prompt, PromptStats, PromptTree, SectionChange, YamlValue,
};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashSet};
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tempfile::TempDir;

/// The `type` of a diff change Go lists from map iteration.
const REMOVED_CHANGE: &str = "removed";

/// Sorts the arrays of a document of one of this plan's commands that Go
/// builds from map iteration: `tree.children` and `tree.mixinUsers` of
/// `tree`; the trailing run of `removed` changes of `diff` (by section);
/// `files` (by name, then path) and `stale` of `emit`. Every other document
/// is left as it is. The hook only reorders.
pub fn sort_map_ordered(command: &str, doc: &mut serde_json::Value, _project: &str) {
    let text = |entry: &serde_json::Value, key: &str| {
        entry
            .get(key)
            .and_then(|value| value.as_str())
            .unwrap_or_default()
            .to_owned()
    };
    match command {
        "tree" => {
            if let Some(tree) = doc.get_mut("tree") {
                for key in ["children", "mixinUsers"] {
                    if let Some(items) = tree.get_mut(key).and_then(|items| items.as_array_mut()) {
                        items.sort_by_key(|item| item.as_str().unwrap_or_default().to_owned());
                    }
                }
            }
        }
        "diff" => {
            if let Some(changes) = doc
                .get_mut("changes")
                .and_then(|items| items.as_array_mut())
            {
                let start = changes
                    .iter()
                    .rposition(|change| {
                        change.get("type").and_then(|kind| kind.as_str()) != Some(REMOVED_CHANGE)
                    })
                    .map_or(0, |index| index + 1);
                changes[start..].sort_by_key(|change| text(change, "section"));
            }
        }
        "emit" => {
            if let Some(files) = doc.get_mut("files").and_then(|items| items.as_array_mut()) {
                files.sort_by_key(|file| (text(file, "name"), text(file, "path")));
            }
            if let Some(stale) = doc.get_mut("stale").and_then(|items| items.as_array_mut()) {
                stale.sort_by_key(|name| name.as_str().unwrap_or_default().to_owned());
            }
        }
        _ => {}
    }
}

// ---- argv -------------------------------------------------------------------

/// A flag value. Go's `option()` drops a value that starts with `--`, so
/// such a value could never reach the program; refuse it here instead of
/// letting the two sides drift apart.
fn flag_value(value: &str) -> String {
    assert!(
        !value.starts_with("--"),
        "a flag value starting with `--` is dropped by Go's option(): {value:?}"
    );
    value.to_owned()
}

/// A positional argument. Go's `Run` removes `--json`, `--quiet`, `-q` and
/// `--timing` wherever they stand and answers `--help`, `-h`, `--version`
/// and `-v` itself, so none of them can be a positional value.
fn argument(value: &str) -> String {
    assert!(
        ![
            "--json",
            "--quiet",
            "-q",
            "--timing",
            "--help",
            "-h",
            "--version",
            "-v"
        ]
        .contains(&value),
        "Go's Run consumes {value:?} before the command sees it"
    );
    value.to_owned()
}

fn push_flag(argv: &mut Vec<String>, name: &str, value: &str) {
    argv.push(name.to_owned());
    argv.push(flag_value(value));
}

/// A value as Go's `encoding/json` prints it, or `None` when Go's marshal
/// fails (GD10: `writeJSON` then prints an empty line, so no document).
pub fn yaml_json(value: &YamlValue) -> Option<Value> {
    value.to_go_json().ok().map(|text| {
        serde_json::from_str(&text).unwrap_or_else(|error| {
            panic!("YamlValue::to_go_json wrote invalid JSON ({error}): {text}")
        })
    })
}

/// A prompt as Go prints it inside a document.
fn prompt_json(prompt: &Prompt) -> Value {
    payload::prompt_value(prompt)
}

/// Implements `PlanStep::apply_raw` and `describe_raw` for a command step.
macro_rules! command_step {
    () => {
        fn apply_raw(&self, _root: &Path) {
            unreachable!("a command step is not raw");
        }

        fn describe_raw(&self) -> String {
            unreachable!("a command step is not raw");
        }
    };
}

// ---- stats --------------------------------------------------------------------

/// `stats`.
pub struct StatsStep;

/// The result of `stats` (cli.go:752).
pub struct StatsDone(pub PromptStats);

impl PlanDone for StatsDone {
    fn documents(&self) -> Vec<Value> {
        vec![json!({
            "success": true,
            "command": "stats",
            "prompts": {
                "total": self.0.total,
                "byStatus": self.0.by_status,
                "byTag": self.0.by_tag,
            },
        })]
    }
}

impl case::PlanStep for StatsStep {
    fn argv(&self) -> Option<Vec<String>> {
        Some(vec!["stats".to_owned()])
    }

    fn call(&self, beacon: &Beacon) -> Result<Done, BeaconError> {
        Ok(Done::Plan(Box::new(StatsDone(beacon.stats()?))))
    }

    command_step!();
}

// ---- tree ---------------------------------------------------------------------

/// `tree <reference>`.
pub struct TreeStep(pub String);

/// The result of `tree` (cli.go:788).
pub struct TreeDone(pub PromptTree);

impl PlanDone for TreeDone {
    fn documents(&self) -> Vec<Value> {
        let tree = &self.0;
        vec![json!({
            "success": true,
            "command": "tree",
            "name": tree.name,
            "ancestors": tree.ancestors,
            "mixins": tree.mixins,
            "tree": {
                "name": tree.name,
                "children": tree.children,
                "mixinUsers": tree.mixin_users,
            },
        })]
    }
}

impl case::PlanStep for TreeStep {
    fn argv(&self) -> Option<Vec<String>> {
        Some(vec!["tree".to_owned(), argument(&self.0)])
    }

    fn call(&self, beacon: &Beacon) -> Result<Done, BeaconError> {
        Ok(Done::Plan(Box::new(TreeDone(beacon.tree(&self.0)?))))
    }

    command_step!();
}

// ---- pin and unpin ----------------------------------------------------------

/// `pin <name@version>`.
pub struct PinStep(pub String);

/// `unpin <reference>`.
pub struct UnpinStep(pub String);

/// The result of `pin` (cli.go:839).
pub struct PinDone(pub Prompt);

/// The result of `unpin` (cli.go:839).
pub struct UnpinDone(pub Prompt);

impl PlanDone for PinDone {
    fn documents(&self) -> Vec<Value> {
        vec![json!({
            "success": true,
            "command": "pin",
            "name": self.0.name,
            "pinned": self.0.pinned,
        })]
    }
}

impl PlanDone for UnpinDone {
    fn documents(&self) -> Vec<Value> {
        vec![json!({
            "success": true,
            "command": "unpin",
            "name": self.0.name,
            "pinned": self.0.pinned,
        })]
    }
}

impl case::PlanStep for PinStep {
    fn argv(&self) -> Option<Vec<String>> {
        Some(vec!["pin".to_owned(), argument(&self.0)])
    }

    fn call(&self, beacon: &Beacon) -> Result<Done, BeaconError> {
        Ok(Done::Plan(Box::new(PinDone(beacon.pin(&self.0)?))))
    }

    command_step!();
}

impl case::PlanStep for UnpinStep {
    fn argv(&self) -> Option<Vec<String>> {
        Some(vec!["unpin".to_owned(), argument(&self.0)])
    }

    fn call(&self, beacon: &Beacon) -> Result<Done, BeaconError> {
        Ok(Done::Plan(Box::new(UnpinDone(beacon.unpin(&self.0)?))))
    }

    command_step!();
}

// ---- diff ---------------------------------------------------------------------

/// `diff <name> <from> <to>`.
pub struct DiffStep {
    pub name: String,
    pub from: String,
    pub to: String,
}

/// The result of `diff` (cli.go:1698): the arguments as given and the
/// changes.
pub struct DiffDone {
    pub name: String,
    pub from: String,
    pub to: String,
    pub changes: Vec<SectionChange>,
}

impl PlanDone for DiffDone {
    fn documents(&self) -> Vec<Value> {
        let changes: Vec<Value> = self
            .changes
            .iter()
            .map(|change| json!({"section": change.section, "type": change.kind.as_str()}))
            .collect();
        vec![json!({
            "success": true,
            "command": "diff",
            "name": self.name,
            "from": self.from,
            "to": self.to,
            "changes": changes,
        })]
    }
}

impl case::PlanStep for DiffStep {
    fn argv(&self) -> Option<Vec<String>> {
        Some(vec![
            "diff".to_owned(),
            argument(&self.name),
            argument(&self.from),
            argument(&self.to),
        ])
    }

    fn call(&self, beacon: &Beacon) -> Result<Done, BeaconError> {
        let changes = beacon.diff(&self.name, &self.from, &self.to)?;
        Ok(Done::Plan(Box::new(DiffDone {
            name: self.name.clone(),
            from: self.from.clone(),
            to: self.to.clone(),
            changes,
        })))
    }

    command_step!();
}

// ---- emit ---------------------------------------------------------------------

/// `emit`. `go_out` and `go_out_dir` replace the library's `--out` and
/// `--out-dir` in Go's argv, for an absolute path under each side's root.
pub struct EmitStep {
    pub options: EmitOptions,
    pub go_out: Option<String>,
    pub go_out_dir: Option<String>,
}

/// The result of `emit` (cli.go:729, :735).
pub struct EmitDone(pub EmitReport);

impl PlanDone for EmitDone {
    fn documents(&self) -> Vec<Value> {
        match &self.0 {
            EmitReport::Files(files) => {
                let entries: Vec<Value> = files
                    .iter()
                    .map(|file| {
                        let mut entry = json!({
                            "name": file.name,
                            "path": file.path,
                            "version": file.version,
                        });
                        if file.skipped {
                            entry["skipped"] = Value::Bool(true);
                        }
                        entry
                    })
                    .collect();
                vec![json!({
                    "success": true,
                    "command": "emit",
                    "files": entries,
                    "count": files.len(),
                })]
            }
            EmitReport::Check { stale } => {
                let success = stale.is_empty();
                let mut docs = vec![json!({
                    "success": success,
                    "command": "emit",
                    "check": true,
                    "stale": stale,
                    "upToDate": success,
                })];
                if !success {
                    docs.push(payload::error_document(
                        "emit",
                        &format!("{} stale file(s)", stale.len()),
                    ));
                }
                docs
            }
        }
    }

    fn exit_code(&self) -> i32 {
        match &self.0 {
            EmitReport::Check { stale } if !stale.is_empty() => 1,
            _ => 0,
        }
    }
}

fn emit_argv(options: &EmitOptions, out: Option<&String>, out_dir: Option<&String>) -> Vec<String> {
    let mut argv = vec!["emit".to_owned()];
    match &options.selection {
        EmitSelection::All => argv.push("--all".to_owned()),
        EmitSelection::Prompt(name) => argv.push(argument(name)),
    }
    if options.check {
        argv.push("--check".to_owned());
    }
    if options.dry_run {
        argv.push("--dry-run".to_owned());
    }
    if options.force {
        argv.push("--force".to_owned());
    }
    if let Some(out) = out {
        push_flag(&mut argv, "--out", out);
    }
    if let Some(out_dir) = out_dir {
        push_flag(&mut argv, "--out-dir", out_dir);
    }
    argv
}

impl case::PlanStep for EmitStep {
    fn argv(&self) -> Option<Vec<String>> {
        Some(emit_argv(
            &self.options,
            self.go_out.as_ref().or(self.options.out.as_ref()),
            self.go_out_dir.as_ref().or(self.options.out_dir.as_ref()),
        ))
    }

    fn call(&self, beacon: &Beacon) -> Result<Done, BeaconError> {
        let report = beacon.emit(&self.options)?;
        // The failing check is a successful call whose report Go turns into
        // a second error document and exit status 1.
        if let Some(failure) = report.failure() {
            assert!(
                matches!(&report, EmitReport::Check { stale } if failure.to_string() == format!("{} stale file(s)", stale.len())),
                "EmitReport::failure must be the stale-file count of a check: {failure}"
            );
        }
        Ok(Done::Plan(Box::new(EmitDone(report))))
    }

    command_step!();
}

// ---- import -------------------------------------------------------------------

/// `import <path> --name <name> [--tag t]... [--no-split]`.
pub struct ImportStep(pub ImportMarkdown);

/// The result of `import` (cli.go:1647).
pub struct ImportDone(pub Imported);

impl PlanDone for ImportDone {
    fn documents(&self) -> Vec<Value> {
        vec![json!({
            "success": true,
            "command": "import",
            "id": self.0.prompt.id,
            "name": self.0.prompt.name,
            "sections": self.0.sections,
        })]
    }
}

impl case::PlanStep for ImportStep {
    fn argv(&self) -> Option<Vec<String>> {
        let input = &self.0;
        let mut argv = vec!["import".to_owned(), argument(&input.path)];
        push_flag(&mut argv, "--name", &input.name);
        for tag in &input.tags {
            push_flag(&mut argv, "--tag", tag);
        }
        if input.no_split {
            argv.push("--no-split".to_owned());
        }
        Some(argv)
    }

    fn call(&self, beacon: &Beacon) -> Result<Done, BeaconError> {
        Ok(Done::Plan(Box::new(ImportDone(
            beacon.import_markdown(&self.0)?,
        ))))
    }

    command_step!();
}

// ---- config -------------------------------------------------------------------

/// `config schema`.
pub struct ConfigSchemaStep;

/// The document `config schema` prints (cli.go:848).
pub struct ConfigSchemaDone(pub Value);

impl PlanDone for ConfigSchemaDone {
    fn documents(&self) -> Vec<Value> {
        vec![self.0.clone()]
    }
}

impl case::PlanStep for ConfigSchemaStep {
    fn argv(&self) -> Option<Vec<String>> {
        Some(vec!["config".to_owned(), "schema".to_owned()])
    }

    fn call(&self, _beacon: &Beacon) -> Result<Done, BeaconError> {
        Ok(Done::Plan(Box::new(ConfigSchemaDone(
            luuna_beacon::config_schema(),
        ))))
    }

    command_step!();
}

/// `config show`.
pub struct ConfigShowStep;

/// The result of `config show` with `--json` (cli.go:879).
pub struct ConfigShowDone(pub ConfigShow);

impl PlanDone for ConfigShowDone {
    fn documents(&self) -> Vec<Value> {
        yaml_json(&self.0.raw)
            .map(|config| {
                vec![json!({"success": true, "command": "config show", "config": config})]
            })
            .unwrap_or_default()
    }
}

impl case::PlanStep for ConfigShowStep {
    fn argv(&self) -> Option<Vec<String>> {
        Some(vec!["config".to_owned(), "show".to_owned()])
    }

    fn call(&self, beacon: &Beacon) -> Result<Done, BeaconError> {
        Ok(Done::Plan(Box::new(ConfigShowDone(beacon.config_show()?))))
    }

    command_step!();
}

/// `config show --path <path>`.
pub struct ConfigShowPathStep(pub String);

/// The result of `config show --path` with `--json` (cli.go:871).
pub struct ConfigShowPathDone {
    pub path: String,
    pub value: YamlValue,
}

impl PlanDone for ConfigShowPathDone {
    fn documents(&self) -> Vec<Value> {
        yaml_json(&self.value)
            .map(|value| {
                vec![json!({
                    "success": true,
                    "command": "config show",
                    "path": self.path,
                    "value": value,
                })]
            })
            .unwrap_or_default()
    }
}

impl case::PlanStep for ConfigShowPathStep {
    fn argv(&self) -> Option<Vec<String>> {
        let mut argv = vec!["config".to_owned(), "show".to_owned()];
        push_flag(&mut argv, "--path", &self.0);
        Some(argv)
    }

    fn call(&self, beacon: &Beacon) -> Result<Done, BeaconError> {
        let value = beacon.config_get(&self.0)?;
        Ok(Done::Plan(Box::new(ConfigShowPathDone {
            path: self.0.clone(),
            value,
        })))
    }

    command_step!();
}

/// `config set <path> <value>`.
pub struct ConfigSetStep {
    pub path: String,
    pub value: String,
}

/// The result of `config set` (cli.go:901): the replacement as Go decoded
/// it.
pub struct ConfigSetDone {
    pub path: String,
    pub value: YamlValue,
}

impl PlanDone for ConfigSetDone {
    fn documents(&self) -> Vec<Value> {
        yaml_json(&self.value)
            .map(|value| {
                vec![json!({
                    "success": true,
                    "command": "config set",
                    "path": self.path,
                    "value": value,
                })]
            })
            .unwrap_or_default()
    }
}

impl case::PlanStep for ConfigSetStep {
    fn argv(&self) -> Option<Vec<String>> {
        Some(vec![
            "config".to_owned(),
            "set".to_owned(),
            argument(&self.path),
            argument(&self.value),
        ])
    }

    fn call(&self, beacon: &Beacon) -> Result<Done, BeaconError> {
        let value = beacon.config_set(&self.path, &self.value)?;
        Ok(Done::Plan(Box::new(ConfigSetDone {
            path: self.path.clone(),
            value,
        })))
    }

    command_step!();
}

/// `config unset <path>`.
pub struct ConfigUnsetStep(pub String);

/// The result of `config unset` (cli.go:916).
pub struct ConfigUnsetDone {
    pub path: String,
    pub removed: bool,
}

impl PlanDone for ConfigUnsetDone {
    fn documents(&self) -> Vec<Value> {
        vec![json!({
            "success": true,
            "command": "config unset",
            "path": self.path,
            "removed": self.removed,
        })]
    }
}

impl case::PlanStep for ConfigUnsetStep {
    fn argv(&self) -> Option<Vec<String>> {
        Some(vec![
            "config".to_owned(),
            "unset".to_owned(),
            argument(&self.0),
        ])
    }

    fn call(&self, beacon: &Beacon) -> Result<Done, BeaconError> {
        let removed = beacon.config_unset(&self.0)?;
        Ok(Done::Plan(Box::new(ConfigUnsetDone {
            path: self.0.clone(),
            removed,
        })))
    }

    command_step!();
}

// ---- raw steps ----------------------------------------------------------------

/// Creates a symbolic link at `path` (relative to the project root) that
/// points at `target` as written, on each side.
pub struct SymlinkStep {
    pub path: String,
    pub target: String,
}

impl case::PlanStep for SymlinkStep {
    fn argv(&self) -> Option<Vec<String>> {
        None
    }

    fn call(&self, _beacon: &Beacon) -> Result<Done, BeaconError> {
        unreachable!("a raw step has no library call");
    }

    fn apply_raw(&self, root: &Path) {
        let link = root.join(&self.path);
        if let Some(parent) = link.parent() {
            fs::create_dir_all(parent).expect("create the parent of a link");
        }
        symlink(Path::new(&self.target), &link);
    }

    fn describe_raw(&self) -> String {
        format!("link {} -> {}", self.path, self.target)
    }
}

/// Creates a folder (relative to the project root) on each side.
pub struct MkdirStep(pub String);

impl case::PlanStep for MkdirStep {
    fn argv(&self) -> Option<Vec<String>> {
        None
    }

    fn call(&self, _beacon: &Beacon) -> Result<Done, BeaconError> {
        unreachable!("a raw step has no library call");
    }

    fn apply_raw(&self, root: &Path) {
        fs::create_dir_all(root.join(&self.0)).expect("create a folder");
    }

    fn describe_raw(&self) -> String {
        format!("mkdir {}", self.0)
    }
}

#[cfg(unix)]
fn symlink(target: &Path, link: &Path) {
    std::os::unix::fs::symlink(target, link).expect("create a symbolic link");
}

#[cfg(not(unix))]
fn symlink(_target: &Path, _link: &Path) {
    panic!("symbolic link cases run on unix only");
}

// ---- step constructors --------------------------------------------------------

pub fn stats() -> Step {
    Step::Plan(Box::new(StatsStep))
}

pub fn tree(reference: &str) -> Step {
    Step::Plan(Box::new(TreeStep(reference.to_owned())))
}

pub fn pin(reference: &str) -> Step {
    Step::Plan(Box::new(PinStep(reference.to_owned())))
}

pub fn unpin(reference: &str) -> Step {
    Step::Plan(Box::new(UnpinStep(reference.to_owned())))
}

pub fn diff(name: &str, from: &str, to: &str) -> Step {
    Step::Plan(Box::new(DiffStep {
        name: name.to_owned(),
        from: from.to_owned(),
        to: to.to_owned(),
    }))
}

/// Emit options with nothing set but the selection.
pub fn emit_options(selection: EmitSelection) -> EmitOptions {
    EmitOptions {
        selection,
        check: false,
        dry_run: false,
        force: false,
        out: None,
        out_dir: None,
    }
}

/// `emit --all`.
pub fn emit_all() -> EmitOptions {
    emit_options(EmitSelection::All)
}

/// `emit <name>`.
pub fn emit_one(name: &str) -> EmitOptions {
    emit_options(EmitSelection::Prompt(name.to_owned()))
}

pub fn emit(options: EmitOptions) -> Step {
    Step::Plan(Box::new(EmitStep {
        options,
        go_out: None,
        go_out_dir: None,
    }))
}

/// An emit step whose `--out` (and/or `--out-dir`) is an absolute path: the
/// relative path `out` (or `out_dir`) under each side's own root.
pub fn emit_absolute(
    mut options: EmitOptions,
    go_root: &Path,
    lib_root: &Path,
    out: Option<&str>,
    out_dir: Option<&str>,
) -> Step {
    let under = |root: &Path, relative: &str| root.join(relative).to_string_lossy().into_owned();
    options.out = out.map(|relative| under(lib_root, relative));
    options.out_dir = out_dir.map(|relative| under(lib_root, relative));
    Step::Plan(Box::new(EmitStep {
        options,
        go_out: out.map(|relative| under(go_root, relative)),
        go_out_dir: out_dir.map(|relative| under(go_root, relative)),
    }))
}

pub fn import(path: &str, name: &str, tags: &[&str], no_split: bool) -> Step {
    Step::Plan(Box::new(ImportStep(import_input(
        path, name, tags, no_split,
    ))))
}

pub fn import_input(path: &str, name: &str, tags: &[&str], no_split: bool) -> ImportMarkdown {
    ImportMarkdown {
        path: path.to_owned(),
        name: name.to_owned(),
        tags: tags.iter().map(|tag| (*tag).to_owned()).collect(),
        no_split,
    }
}

pub fn config_schema() -> Step {
    Step::Plan(Box::new(ConfigSchemaStep))
}

pub fn config_show() -> Step {
    Step::Plan(Box::new(ConfigShowStep))
}

pub fn config_show_path(path: &str) -> Step {
    Step::Plan(Box::new(ConfigShowPathStep(path.to_owned())))
}

pub fn config_set(path: &str, value: &str) -> Step {
    Step::Plan(Box::new(ConfigSetStep {
        path: path.to_owned(),
        value: value.to_owned(),
    }))
}

pub fn config_unset(path: &str) -> Step {
    Step::Plan(Box::new(ConfigUnsetStep(path.to_owned())))
}

pub fn symlink_step(path: &str, target: &str) -> Step {
    Step::Plan(Box::new(SymlinkStep {
        path: path.to_owned(),
        target: target.to_owned(),
    }))
}

pub fn mkdir(path: &str) -> Step {
    Step::Plan(Box::new(MkdirStep(path.to_owned())))
}

/// The config file's path relative to a project root.
pub const CONFIG: &str = ".luuna/beacon/config.yaml";
/// The prompt store's path relative to a project root.
pub const PROMPTS: &str = ".luuna/beacon/prompts.jsonl";

// ---- the Go tool without --json ---------------------------------------------

fn is_git_marker(dot_git: &Path) -> bool {
    dot_git.is_file() || dot_git.join("HEAD").exists()
}

/// Panics unless `root` is absolute, outside the reference clone and outside
/// every git repository (the checks of `go::run_go`).
fn check_go_root(root: &Path, clone: &Path) {
    assert!(
        root.is_absolute(),
        "project root {} is not absolute",
        root.display()
    );
    let canonical = root.canonicalize().unwrap_or_else(|error| {
        panic!("project root {} cannot be opened: {error}", root.display())
    });
    assert!(
        !canonical.starts_with(clone),
        "project root {} is inside the reference clone",
        canonical.display()
    );
    for folder in canonical.ancestors() {
        assert!(
            !is_git_marker(&folder.join(".git")),
            "project root {} is inside the git repository {}",
            canonical.display(),
            folder.display()
        );
    }
}

/// Runs the Go binary in `root` with exactly `args` and the environment of
/// `go::run_go` (cleared; PATH, HOME and TMPDIR under the build directory).
fn go_output(root: &Path, args: &[OsString]) -> (i32, Vec<u8>, Vec<u8>) {
    let build = go::go_build();
    check_go_root(root, &build.clone);
    let oracle = build.dir.join("oracle");
    let (path, home, tmp) = (oracle.join("path"), oracle.join("home"), oracle.join("tmp"));
    for folder in [&path, &home, &tmp] {
        fs::create_dir_all(folder)
            .unwrap_or_else(|error| panic!("cannot create {}: {error}", folder.display()));
    }
    let output = Command::new(&build.binary)
        .env_clear()
        .env("PATH", &path)
        .env("HOME", &home)
        .env("TMPDIR", &tmp)
        .current_dir(root)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .unwrap_or_else(|error| panic!("could not start {}: {error}", build.binary.display()));
    let code = output
        .status
        .code()
        .unwrap_or_else(|| panic!("the Go binary was stopped by a signal: {}", output.status));
    (code, output.stdout, output.stderr)
}

/// Runs the Go tool without `--json` (render, config show): exit code,
/// stdout and stderr.
pub fn run_go_text(root: &Path, args: &[&str]) -> (i32, Vec<u8>, Vec<u8>) {
    let args: Vec<OsString> = args.iter().map(OsString::from).collect();
    go_output(root, &args)
}

/// Like `go::run_go` (with `--json` appended) for arguments that need not be
/// UTF-8, such as a `config set` value the library's `&str` cannot take.
pub fn run_go_bytes(root: &Path, args: &[&[u8]]) -> GoRun {
    let mut argv: Vec<OsString> = args.iter().map(|arg| os_string(arg)).collect();
    argv.push(OsString::from("--json"));
    let (code, stdout, stderr) = go_output(root, &argv);
    let documents = serde_json::Deserializer::from_slice(&stdout)
        .into_iter::<Value>()
        .collect::<Result<Vec<_>, _>>()
        .unwrap_or_else(|error| {
            panic!(
                "Go's stdout is not a stream of JSON values ({error}):\n{}",
                String::from_utf8_lossy(&stdout)
            )
        });
    GoRun {
        code,
        stdout,
        stderr,
        documents,
    }
}

#[cfg(unix)]
fn os_string(bytes: &[u8]) -> OsString {
    use std::os::unix::ffi::OsStringExt;
    OsString::from_vec(bytes.to_vec())
}

#[cfg(not(unix))]
fn os_string(bytes: &[u8]) -> OsString {
    OsString::from(String::from_utf8(bytes.to_vec()).expect("non-unix arguments are UTF-8"))
}

// ---- roots --------------------------------------------------------------------

/// A canonicalised project root for the Rust-only tests: a fresh
/// `tempfile::tempdir`, never `case::fresh_root` (which builds the Go tool).
pub fn api_root() -> (TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("create a temporary project root");
    let path = dir.path().canonicalize().expect("canonical project root");
    (dir, path)
}

/// Two fresh temporary folders (from `case::fresh_root`), each with an empty
/// project root at `<tempdir>/project`, so that writes that escape the root
/// stay inside the side's own temporary folder.
pub fn nested_roots() -> (TempDir, PathBuf, TempDir, PathBuf) {
    let (go_dir, go_top) = case::fresh_root("go-nested");
    let (lib_dir, lib_top) = case::fresh_root("lib-nested");
    let go_root = go_top.join("project");
    let lib_root = lib_top.join("project");
    fs::create_dir(&go_root).expect("create the Go side's project root");
    fs::create_dir(&lib_root).expect("create the library side's project root");
    (go_dir, go_root, lib_dir, lib_root)
}

// ---- files --------------------------------------------------------------------

#[cfg(unix)]
fn mode_bits(metadata: &fs::Metadata) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    metadata.permissions().mode() & 0o7777
}

#[cfg(not(unix))]
fn mode_bits(_metadata: &fs::Metadata) -> u32 {
    0
}

/// One entry under a root as [`compare_outside_store`] sees it.
#[derive(Debug, PartialEq, Eq)]
enum Entry {
    Dir { mode: u32 },
    File { bytes: Vec<u8>, mode: u32 },
    Link { target: String },
}

/// Every entry under `top` except folders named `.luuna`, keyed by relative
/// path; symbolic links are not followed, and a link target that names
/// `root_text` is written with `<root>` in its place.
fn entries_outside_store(top: &Path, root_text: &str) -> BTreeMap<String, Entry> {
    let mut found = BTreeMap::new();
    let mut pending = vec![top.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let listing = fs::read_dir(&dir)
            .unwrap_or_else(|error| panic!("cannot list {}: {error}", dir.display()));
        for item in listing {
            let item = item.expect("read a folder entry");
            let path = item.path();
            let relative = path
                .strip_prefix(top)
                .expect("under the top folder")
                .to_string_lossy()
                .into_owned();
            let metadata = fs::symlink_metadata(&path).expect("metadata of an entry");
            let kind = metadata.file_type();
            if kind.is_symlink() {
                let target = fs::read_link(&path).expect("read a link");
                let target = target.to_string_lossy().replace(root_text, "<root>");
                found.insert(relative, Entry::Link { target });
            } else if kind.is_dir() {
                if item.file_name() == ".luuna" {
                    continue;
                }
                found.insert(
                    relative,
                    Entry::Dir {
                        mode: mode_bits(&metadata),
                    },
                );
                pending.push(path);
            } else {
                found.insert(
                    relative,
                    Entry::File {
                        bytes: fs::read(&path).expect("read a file"),
                        mode: mode_bits(&metadata),
                    },
                );
            }
        }
    }
    found
}

/// Compares every entry under the two folders except folders named
/// `.luuna` (whose store files the case compares itself): relative path,
/// kind, bytes, unix mode bits and symbolic link target (links are not
/// followed). Either folder may be a project root or a temporary folder
/// holding one.
pub fn compare_outside_store(go_root: &Path, lib_root: &Path, context: &str) {
    let go = entries_outside_store(go_root, &go_root.to_string_lossy());
    let lib = entries_outside_store(lib_root, &lib_root.to_string_lossy());
    assert_eq!(
        go.keys().collect::<Vec<_>>(),
        lib.keys().collect::<Vec<_>>(),
        "{context}: the entries outside .luuna differ"
    );
    for (path, go_entry) in &go {
        let lib_entry = &lib[path];
        match (go_entry, lib_entry) {
            (
                Entry::File {
                    bytes: go_bytes,
                    mode: go_mode,
                },
                Entry::File {
                    bytes: lib_bytes,
                    mode: lib_mode,
                },
            ) => {
                assert_eq!(
                    String::from_utf8_lossy(go_bytes),
                    String::from_utf8_lossy(lib_bytes),
                    "{context}: bytes of {path} differ"
                );
                assert_eq!(go_bytes, lib_bytes, "{context}: bytes of {path} differ");
                assert_eq!(
                    format!("{go_mode:o}"),
                    format!("{lib_mode:o}"),
                    "{context}: mode of {path} differs"
                );
            }
            _ => assert_eq!(go_entry, lib_entry, "{context}: {path} differs"),
        }
    }
}

/// Every file (and symbolic link, as its target) under `root`, with its
/// bytes and mode bits, keyed by relative path. Symbolic links are not
/// followed. Used by every A8 test: equal maps before and after a call mean
/// that no file was added, removed or changed, so config.yaml,
/// prompts.jsonl, any `.lock` and any `.tmp*` file are all covered.
pub fn tree_bytes(root: &Path) -> BTreeMap<String, (Vec<u8>, u32)> {
    let mut found = BTreeMap::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let Ok(listing) = fs::read_dir(&dir) else {
            continue;
        };
        for item in listing {
            let item = item.expect("read a folder entry");
            let path = item.path();
            let relative = path
                .strip_prefix(root)
                .expect("under the root")
                .to_string_lossy()
                .into_owned();
            let metadata = fs::symlink_metadata(&path).expect("metadata of an entry");
            let kind = metadata.file_type();
            if kind.is_symlink() {
                let target = fs::read_link(&path).expect("read a link");
                let mut bytes = b"symlink -> ".to_vec();
                bytes.extend_from_slice(target.to_string_lossy().as_bytes());
                found.insert(relative, (bytes, 0));
            } else if kind.is_dir() {
                pending.push(path);
            } else {
                found.insert(
                    relative,
                    (fs::read(&path).expect("read a file"), mode_bits(&metadata)),
                );
            }
        }
    }
    found
}

/// Panics unless the files under `root` are exactly `before` (no file added
/// or removed, every file's bytes and mode unchanged); names any `.lock` or
/// `.tmp*` file left behind.
pub fn assert_tree_unchanged(
    before: &BTreeMap<String, (Vec<u8>, u32)>,
    root: &Path,
    context: &str,
) {
    let after = tree_bytes(root);
    let leftovers: Vec<&String> = after
        .keys()
        .filter(|path| !before.contains_key(*path))
        .collect();
    assert!(
        leftovers.is_empty(),
        "{context}: files were added (a .lock or .tmp* file counts): {leftovers:?}"
    );
    let missing: Vec<&String> = before
        .keys()
        .filter(|path| !after.contains_key(*path))
        .collect();
    assert!(
        missing.is_empty(),
        "{context}: files were removed: {missing:?}"
    );
    for (path, (bytes, mode)) in before {
        let (after_bytes, after_mode) = &after[path];
        assert!(
            bytes == after_bytes,
            "{context}: {path} changed:\nbefore {:?}\nafter  {:?}",
            String::from_utf8_lossy(bytes),
            String::from_utf8_lossy(after_bytes)
        );
        assert_eq!(
            format!("{mode:o}"),
            format!("{after_mode:o}"),
            "{context}: the mode of {path} changed"
        );
    }
}

/// Writes `bytes` at `relative` under `root`, creating parent folders.
pub fn write_under(root: &Path, relative: &str, bytes: &[u8]) {
    let target = root.join(relative);
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).expect("create the parent folder");
    }
    fs::write(&target, bytes).expect("write a file");
}

/// The last line of the prompt store under `root`, without its newline.
pub fn last_store_line(root: &Path) -> Vec<u8> {
    let bytes = fs::read(root.join(PROMPTS)).expect("read prompts.jsonl");
    let trimmed = bytes.strip_suffix(b"\n").unwrap_or(&bytes);
    let start = trimmed
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map_or(0, |index| index + 1);
    trimmed[start..].to_vec()
}

// ---- the memory limit ---------------------------------------------------------

/// The largest address-space limit a hostile test accepts.
pub const MEMORY_LIMIT: u64 = 4_294_967_296;

/// Panics unless this process runs under an address-space limit of at most
/// [`MEMORY_LIMIT`] bytes (`prlimit --as=4294967296`), read from
/// `/proc/self/limits`.
pub fn assert_memory_limited() {
    let limits = fs::read_to_string("/proc/self/limits")
        .expect("read /proc/self/limits (hostile tests run on Linux only)");
    let line = limits
        .lines()
        .find(|line| line.starts_with("Max address space"))
        .expect("/proc/self/limits has a `Max address space` line");
    let soft = line["Max address space".len()..]
        .split_whitespace()
        .next()
        .expect("the soft limit");
    let bytes: u64 = soft.parse().unwrap_or_else(|_| {
        panic!(
            "hostile test without a memory limit (`{soft}`); run under prlimit --as={MEMORY_LIMIT} timeout 120"
        )
    });
    assert!(
        bytes <= MEMORY_LIMIT,
        "hostile test with an address-space limit of {bytes} bytes, above {MEMORY_LIMIT}"
    );
}

// ---- a fixed clock ------------------------------------------------------------

/// A clock fixed at [`FIXED_TIME_TEXT`].
pub struct FixedClock;

impl Clock for FixedClock {
    fn now(&self) -> SystemTime {
        UNIX_EPOCH + Duration::new(1_700_000_000, 123_456_789)
    }
}

/// [`FixedClock`]'s time as Go's `time.RFC3339Nano` in UTC.
pub const FIXED_TIME_TEXT: &str = "2023-11-14T22:13:20.123456789Z";

/// A Beacon on `root` with [`FixedClock`].
pub fn fixed_beacon(root: &Path) -> Beacon {
    Beacon::open(root)
        .expect("open the project root")
        .with_clock(std::sync::Arc::new(FixedClock))
}

/// `line` with the value of its `"updatedAt"` member replaced by `text`.
pub fn with_updated_at(line: &str, text: &str) -> String {
    let key = "\"updatedAt\":\"";
    let start = line.find(key).expect("the line has updatedAt") + key.len();
    let end = start + line[start..].find('"').expect("updatedAt is a string");
    format!("{}{text}{}", &line[..start], &line[end..])
}

// ---- two roots without Case -----------------------------------------------------

/// What one step produced on both sides of a [`Pair`].
pub struct PairSeen {
    pub code: i32,
    /// Go's documents after normalisation.
    pub documents: Vec<Value>,
    pub error: Option<String>,
}

/// Two project roots with identical setup, for the cases `Case` cannot run:
/// project roots nested in a temporary folder (escape and symlink cases,
/// whose whole temporary folders are compared) and the A8 demonstrations,
/// whose refused step runs on each side separately ([`Pair::go`],
/// [`Pair::lib`]). Steps through [`Pair::step`] are compared as `Case::step`
/// compares them: exit status, documents after normalisation (paths under
/// each side's temporary folder written `<top>`), error text with the
/// foundation's exemptions and the stores.
pub struct Pair {
    _go_dir: TempDir,
    _lib_dir: TempDir,
    pub go_top: PathBuf,
    pub lib_top: PathBuf,
    pub go_root: PathBuf,
    pub lib_root: PathBuf,
    go_norm: Normaliser,
    lib_norm: Normaliser,
    beacon: Beacon,
    written: HashSet<String>,
    log: Vec<String>,
}

fn replace_in_strings(value: &mut Value, from: &str, to: &str) {
    let mut pending = vec![value];
    while let Some(value) = pending.pop() {
        match value {
            Value::String(text) => {
                if text.contains(from) {
                    *text = text.replace(from, to);
                }
            }
            Value::Array(items) => pending.extend(items.iter_mut()),
            Value::Object(object) => pending.extend(object.values_mut()),
            _ => {}
        }
    }
}

impl Pair {
    /// Project roots that are the temporary folders themselves.
    pub fn new() -> Self {
        let (go_dir, go_root) = case::fresh_root("go-pair");
        let (lib_dir, lib_root) = case::fresh_root("lib-pair");
        Self::build(
            go_dir,
            go_root.clone(),
            go_root,
            lib_dir,
            lib_root.clone(),
            lib_root,
        )
    }

    /// Project roots at `<tempdir>/project` ([`nested_roots`]).
    pub fn nested() -> Self {
        let (go_dir, go_root, lib_dir, lib_root) = nested_roots();
        let go_top = go_dir.path().canonicalize().expect("canonical folder");
        let lib_top = lib_dir.path().canonicalize().expect("canonical folder");
        Self::build(go_dir, go_top, go_root, lib_dir, lib_top, lib_root)
    }

    fn build(
        go_dir: TempDir,
        go_top: PathBuf,
        go_root: PathBuf,
        lib_dir: TempDir,
        lib_top: PathBuf,
        lib_root: PathBuf,
    ) -> Self {
        let beacon = Beacon::open(&lib_root).expect("open the library project root");
        Pair {
            go_norm: Normaliser::new(case::DEFAULT_PROJECT, &go_root),
            lib_norm: Normaliser::new(case::DEFAULT_PROJECT, &lib_root),
            _go_dir: go_dir,
            _lib_dir: lib_dir,
            go_top,
            lib_top,
            go_root,
            lib_root,
            beacon,
            written: HashSet::new(),
            log: Vec::new(),
        }
    }

    pub fn beacon(&self) -> &Beacon {
        &self.beacon
    }

    /// Applies a raw step to both roots.
    pub fn raw(&mut self, step: Step) {
        let description = case::describe(&step);
        self.log.push(description);
        if let Step::WriteFile { path, bytes } = &step {
            write_under(&self.go_root, path, bytes);
            write_under(&self.lib_root, path, bytes);
            self.written.insert(path.clone());
            return;
        }
        let Step::Plan(plan) = &step else {
            panic!("Pair::raw takes write_file or a raw plan step");
        };
        assert!(plan.argv().is_none(), "Pair::raw takes raw steps only");
        plan.apply_raw(&self.go_root);
        plan.apply_raw(&self.lib_root);
    }

    /// Runs a command step on the Go side only.
    pub fn go(&self, step: &Step) -> GoRun {
        let argv = case::lower(step).argv.expect("a command step");
        let argv: Vec<&str> = argv.iter().map(String::as_str).collect();
        go::run_go(&self.go_root, &argv)
    }

    /// Runs a command step on the library side only.
    pub fn lib(&self, step: &Step) -> case::LibRun {
        case::run_lib(&self.beacon, step)
    }

    fn top_texts(&self) -> (String, String) {
        (
            self.go_top.to_string_lossy().into_owned(),
            self.lib_top.to_string_lossy().into_owned(),
        )
    }

    /// Runs a command step on both sides and compares everything.
    pub fn step(&mut self, step: Step) -> PairSeen {
        self.step_with(step, None)
    }

    /// Like [`Pair::step`] for an operating-system error compared by prefix.
    pub fn step_os_error(&mut self, step: Step, prefix: &str) -> PairSeen {
        self.step_with(step, Some(prefix))
    }

    fn step_with(&mut self, step: Step, os_prefix: Option<&str>) -> PairSeen {
        let description = case::describe(&step);
        self.log.push(description.clone());
        let context = format!(
            "pair step {} `{description}` (steps so far: {})",
            self.log.len(),
            self.log.join(" | ")
        );
        let go_run = self.go(&step);
        let lib_run = self.lib(&step);
        assert_eq!(go_run.code, lib_run.code, "{context}: exit status differs");
        let (go_top, lib_top) = self.top_texts();
        let mut go_docs = go_run.documents;
        let mut lib_docs = lib_run.documents;
        self.go_norm.documents(&mut go_docs);
        self.lib_norm.documents(&mut lib_docs);
        for doc in go_docs.iter_mut() {
            replace_in_strings(doc, &go_top, "<top>");
        }
        for doc in lib_docs.iter_mut() {
            replace_in_strings(doc, &lib_top, "<top>");
        }
        let error_of = |docs: &mut [Value]| -> Vec<String> {
            let mut errors = Vec::new();
            for doc in docs.iter_mut() {
                let is_error = doc.as_object().is_some_and(|object| {
                    object.len() == 3
                        && object.get("success") == Some(&Value::Bool(false))
                        && object.get("error").is_some_and(Value::is_string)
                });
                if is_error {
                    errors.push(doc["error"].as_str().unwrap_or_default().to_owned());
                    doc["error"] = Value::String("<error>".to_owned());
                }
            }
            errors
        };
        let seen_documents = go_docs.clone();
        let go_errors = error_of(&mut go_docs);
        let lib_errors = error_of(&mut lib_docs);
        assert_eq!(
            go_errors.len(),
            lib_errors.len(),
            "{context}: error documents differ: Go {go_errors:?}, library {lib_errors:?}"
        );
        for (go_error, lib_error) in go_errors.iter().zip(&lib_errors) {
            case::assert_errors_match(go_error, lib_error, os_prefix, &context);
        }
        assert_eq!(go_docs, lib_docs, "{context}: stdout documents differ");
        self.compare_stores(&context);
        PairSeen {
            code: go_run.code,
            documents: seen_documents,
            error: go_errors.first().cloned(),
        }
    }

    /// Compares the stores of the two project roots as `Case` does.
    pub fn compare_stores(&mut self, context: &str) {
        let go = case::snapshot(&self.go_root, &mut self.go_norm, &self.written);
        let lib = case::snapshot(&self.lib_root, &mut self.lib_norm, &self.written);
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
                go_file.mode, lib_file.mode,
                "{context}: mode of {path} differs"
            );
        }
    }

    /// Compares everything outside the stores: the whole temporary folder
    /// of each side.
    pub fn compare_outside(&self, context: &str) {
        compare_outside_store(&self.go_top, &self.lib_top, context);
    }
}

impl Default for Pair {
    fn default() -> Self {
        Self::new()
    }
}

// ---- the observed Go runs -----------------------------------------------------

/// The base config.yaml of `go-lossy-probe-1` family A, without the
/// `    extra: <variant>` line (the file of mode A3).
pub const PROBE_BASE_CONFIG: &str = "project: luuna-beacon\nversion: \"1\"\ntargets:\n  default:\n    dir: agents\n    default: true\n  other:\n    dir: o\n";

/// The base config.yaml of `go-lossy-probe-1` with the variant at
/// `targets.other.extra` (the file of modes A1 and A2).
pub fn probe_config_with(variant: &[u8]) -> Vec<u8> {
    let mut bytes = PROBE_BASE_CONFIG.as_bytes().to_vec();
    bytes.extend_from_slice(b"    extra: ");
    bytes.extend_from_slice(variant);
    bytes.push(b'\n');
    bytes
}

/// The variant of a family A row by its probe id.
pub fn family_a(id: &str) -> &'static [u8] {
    FAMILY_A
        .iter()
        .find(|(row, _)| *row == id)
        .map(|(_, bytes)| *bytes)
        .unwrap_or_else(|| panic!("no family A row {id}"))
}

/// The record line of a family B row by its probe id.
pub fn family_b(id: &str) -> &'static [u8] {
    FAMILY_B
        .iter()
        .find(|(row, _)| *row == id)
        .map(|(_, bytes)| *bytes)
        .unwrap_or_else(|| panic!("no family B row {id}"))
}

/// The 46 family A variant texts of `go-lossy-probe-1` (the bytes after
/// `extra: ` in each row's `hexdump BEFORE` block), by probe id.
#[rustfmt::skip]
pub const FAMILY_A: [(&str, &[u8]); 46] = [
    ("v01a", br#"123456789012345678901234"#),
    ("v02", br#"18446744073709551615"#),
    ("v03a", br#"0.1000000000000000055511151231257827021181583404541015625001"#),
    ("v03b", br#"3.14159265358979323846264338327950288"#),
    ("v04", br#"1e400"#),
    ("v05a", br#"!mytag v"#),
    ("v05b", br#"!mytag {a: 1}"#),
    ("v06a", br#"!!str 12"#),
    ("v06b", br#"!!float 1"#),
    ("v06c", br#"!!binary aGVsbG8="#),
    ("v07a", br#"0x1F"#),
    ("v07b", br#"0o17"#),
    ("v07c", br#"017"#),
    ("v07d", br#"1_000"#),
    ("v08a", br#"2001-12-14T21:59:43.1234567891Z"#),
    ("v08b", br#"2001-12-14"#),
    ("v09a", b"\"a\xffb\""),
    ("v09b", b"a\xffb"),
    ("v09c", br#""\ud800""#),
    ("v09d", br#""\xff""#),
    ("v10a", br#".inf"#),
    ("v10b", br#"-.inf"#),
    ("v10c", br#".nan"#),
    ("v10d", br#"-0.0"#),
    ("v10e", br#"0b101"#),
    ("v10f", br#"1e3"#),
    ("v10g", br#"1.0"#),
    ("v10h", br#"-9223372036854775809"#),
    ("v10i", br#"!!binary /w=="#),
    ("v10j", br#"2001-12-14 21:59:43.10 -5"#),
    ("v10k", br#"{1: a}"#),
    ("v10l", br#"[1, 2.50, 1e2]"#),
    ("v10m", br#"{a: 1, a: 2}"#),
    ("v10n", br#"~"#),
    ("v10o", br#"100000000000000000000.0"#),
    ("v10p", br#"0x8000000000000000"#),
    ("v10q", br#"-0"#),
    ("v10r", br#"1e-400"#),
    ("v10s", br#"+12"#),
    ("v10t", br#".5"#),
    ("v10u", br#"5."#),
    ("v10v", br#""3""#),
    ("v10w", br#"!!int "3""#),
    ("v10x", br#"&a [1, 2]"#),
    ("v10y", br#"9223372036854775808"#),
    ("v10z", br#"0.30000000000000004"#),
];

/// The 25 family B record lines of `go-lossy-probe-1` (the bytes of each
/// row's `hexdump BEFORE store` block, newline included), by probe id.
#[rustfmt::skip]
pub const FAMILY_B: [(&str, &[u8]); 25] = [
    ("b00ctl", concat!(r#"{"id":"probe-id","name":"p1","version":1,"sections":[{"name":"main","body":"body"}],"frontmatter":{"k":"x"},"status":"active","createdAt":"2026-01-01T00:00:00Z","updatedAt":"2026-01-01T00:00:00Z"}"#, "\n").as_bytes()),
    ("b01a", concat!(r#"{"id":"probe-id","name":"p1","version":1,"sections":[{"name":"main","body":"body"}],"frontmatter":{"k":9007199254740993},"status":"active","createdAt":"2026-01-01T00:00:00Z","updatedAt":"2026-01-01T00:00:00Z"}"#, "\n").as_bytes()),
    ("b01b", concat!(r#"{"id":"probe-id","name":"p1","version":1,"sections":[{"name":"main","body":"body"}],"frontmatter":{"k":123456789012345678901234},"status":"active","createdAt":"2026-01-01T00:00:00Z","updatedAt":"2026-01-01T00:00:00Z"}"#, "\n").as_bytes()),
    ("b01c", concat!(r#"{"id":"probe-id","name":"p1","version":1,"sections":[{"name":"main","body":"body"}],"frontmatter":{"k":18446744073709551615},"status":"active","createdAt":"2026-01-01T00:00:00Z","updatedAt":"2026-01-01T00:00:00Z"}"#, "\n").as_bytes()),
    ("b02a", b"{\"id\":\"probe-id\",\"name\":\"p1\",\"version\":1,\"sections\":[{\"name\":\"main\",\"body\":\"body\"}],\"frontmatter\":{\"k\":\"a\xffb\"},\"status\":\"active\",\"createdAt\":\"2026-01-01T00:00:00Z\",\"updatedAt\":\"2026-01-01T00:00:00Z\"}\n"),
    ("b02b", b"{\"id\":\"probe-id\",\"name\":\"p1\",\"version\":1,\"sections\":[{\"name\":\"main\",\"body\":\"a\xffb\"}],\"frontmatter\":{\"k\":\"x\"},\"status\":\"active\",\"createdAt\":\"2026-01-01T00:00:00Z\",\"updatedAt\":\"2026-01-01T00:00:00Z\"}\n"),
    ("b02c", b"{\"id\":\"probe-id\",\"name\":\"p1\",\"version\":1,\"sections\":[{\"name\":\"s\xffn\",\"body\":\"body\"}],\"frontmatter\":{\"k\":\"x\"},\"status\":\"active\",\"createdAt\":\"2026-01-01T00:00:00Z\",\"updatedAt\":\"2026-01-01T00:00:00Z\"}\n"),
    ("b03a", concat!(r#"{"id":"probe-id","name":"p1","version":1,"sections":[{"name":"main","body":"body"}],"frontmatter":{"k":"\ud800"},"status":"active","createdAt":"2026-01-01T00:00:00Z","updatedAt":"2026-01-01T00:00:00Z"}"#, "\n").as_bytes()),
    ("b03b", concat!(r#"{"id":"probe-id","name":"p1","version":1,"sections":[{"name":"main","body":"\ud800"}],"frontmatter":{"k":"x"},"status":"active","createdAt":"2026-01-01T00:00:00Z","updatedAt":"2026-01-01T00:00:00Z"}"#, "\n").as_bytes()),
    ("b04a", concat!(r#"{"id":"probe-id","name":"p1","extraField":"x","version":1,"sections":[{"name":"main","body":"body"}],"frontmatter":{"k":"x"},"status":"active","createdAt":"2026-01-01T00:00:00Z","updatedAt":"2026-01-01T00:00:00Z"}"#, "\n").as_bytes()),
    ("b04b", concat!(r#"{"id":"probe-id","name":"p1","version":1,"sections":[{"name":"main","body":"body","extra":"x"}],"frontmatter":{"k":"x"},"status":"active","createdAt":"2026-01-01T00:00:00Z","updatedAt":"2026-01-01T00:00:00Z"}"#, "\n").as_bytes()),
    ("b04c", concat!(r#"{"id":"probe-id","name":"p1","version":1,"sections":[{"name":"main","body":"body"}],"frontmatter":{"k":"x"},"status":"active","createdAt":"2026-01-01T00:00:00Z","updatedAt":"2026-01-01T00:00:00Z","atlas":{"prime":{"domains":["a"],"zzz":1},"zzz":2}}"#, "\n").as_bytes()),
    ("b05a", concat!(r#"{"id":"probe-id","name":"p1","version":1,"sections":[{"name":"main","body":"body"}],"frontmatter":{"k":1E2},"status":"active","createdAt":"2026-01-01T00:00:00Z","updatedAt":"2026-01-01T00:00:00Z"}"#, "\n").as_bytes()),
    ("b05b", concat!(r#"{"id":"probe-id","name":"p1","version":1,"sections":[{"name":"main","body":"body"}],"frontmatter":{"k":-0},"status":"active","createdAt":"2026-01-01T00:00:00Z","updatedAt":"2026-01-01T00:00:00Z"}"#, "\n").as_bytes()),
    ("b05c", concat!(r#"{"id":"probe-id","name":"p1","version":1,"sections":[{"name":"main","body":"body"}],"frontmatter":{"k":1e400},"status":"active","createdAt":"2026-01-01T00:00:00Z","updatedAt":"2026-01-01T00:00:00Z"}"#, "\n").as_bytes()),
    ("b05d", concat!(r#"{"id":"probe-id","name":"p1","version":1,"sections":[{"name":"main","body":"body"}],"frontmatter":{"k":1,"k":2},"status":"active","createdAt":"2026-01-01T00:00:00Z","updatedAt":"2026-01-01T00:00:00Z"}"#, "\n").as_bytes()),
    ("b05e", concat!(r#"{"id":"probe-id","name":"p1","Description":"CASE","version":1,"sections":[{"name":"main","body":"body"}],"frontmatter":{"k":"x"},"status":"active","createdAt":"2026-01-01T00:00:00Z","updatedAt":"2026-01-01T00:00:00Z"}"#, "\n").as_bytes()),
    ("b05f", concat!(r#"{"id":"probe-id","name":"p1","version":1,"sections":[{"name":"main","body":"body"}],"frontmatter":{"k":"x"},"status":"active","createdAt":"2026-01-01T00:00:00Z","updatedAt":"2026-01-01T00:00:00Z","atlas":{"budget":0.1000000000000000055511151231257827021181583404541015625001}}"#, "\n").as_bytes()),
    ("b05g", concat!(r#"{"id":"probe-id","name":"p1","version":1,"sections":[{"name":"main","body":"body"}],"frontmatter":{"k":"<&>"},"status":"active","createdAt":"2026-01-01T00:00:00Z","updatedAt":"2026-01-01T00:00:00Z"}"#, "\n").as_bytes()),
    ("b05h", b"{\"id\":\"probe-id\",\"name\":\"p1\",\"version\":1,\"sections\":[{\"name\":\"main\",\"body\":\"body\"}],\"frontmatter\":{\"k\":\"\xe2\x80\xa8\"},\"status\":\"active\",\"createdAt\":\"2026-01-01T00:00:00Z\",\"updatedAt\":\"2026-01-01T00:00:00Z\"}\n"),
    ("b05i", concat!(r#"{"id":"probe-id","name":"p1","version":1,"sections":[{"name":"main","body":"body"}],"frontmatter":{"k":"x"},"status":"active","createdAt":"2026-01-01T00:00:00Z","updatedAt":"2026-01-01T00:00:00Z","mulch":{"budget":1.5}}"#, "\n").as_bytes()),
    ("b05j", concat!(r#"{"id":"probe-id","name":"p1","version":1,"sections":[{"name":"main","body":"body"}],"frontmatter":{"k":"x"},"status":"active","createdAt":"2026-01-01T00:00:00Z","updatedAt":"2026-01-01T00:00:00Z","tags":null}"#, "\n").as_bytes()),
    ("b05k", concat!(r#"{"id":"probe-id","name":"p1","version":1,"sections":[{"name":"main","body":"body"}],"frontmatter":{"k":"\ud83d\ude00"},"status":"active","createdAt":"2026-01-01T00:00:00Z","updatedAt":"2026-01-01T00:00:00Z"}"#, "\n").as_bytes()),
    ("b05l", concat!(r#"{"id":"probe-id","name":"p1","version":1,"sections":[{"name":"main","body":"body"}],"frontmatter":{"k":0.1000000000000000055511151231257827021181583404541015625001},"status":"active","createdAt":"2026-01-01T00:00:00Z","updatedAt":"2026-01-01T00:00:00Z"}"#, "\n").as_bytes()),
    ("b05m", concat!(r#"{"id":"probe-id","name":"p1","NAME":"dup","version":1,"sections":[{"name":"main","body":"body"}],"frontmatter":{"k":"x"},"status":"active","createdAt":"2026-01-01T00:00:00Z","updatedAt":"2026-01-01T00:00:00Z"}"#, "\n").as_bytes()),
];

/// A row of `go-lossy-probe-2`: the record line (b00ctl with one change)
/// and Go's new record line under `pin p1@1` and under `unpin p1`, with
/// Go's own updatedAt.
pub struct ProbeTwoRow {
    pub id: &'static str,
    pub record: &'static str,
    pub pin_line: &'static str,
    pub unpin_line: &'static str,
}

/// n1, n2 and n3 of `go-lossy-probe-2` (table lines 5127-5132; new record
/// lines 1441/1960, 2486/3016 and 3521/4016 of that log).
#[rustfmt::skip]
pub const PROBE_TWO: [ProbeTwoRow; 3] = [
    // n1: "description":"d","description":null, after "version":1, (table :5127-5128).
    ProbeTwoRow {
        id: "n1",
        record: concat!(r#"{"id":"probe-id","name":"p1","version":1,"description":"d","description":null,"sections":[{"name":"main","body":"body"}],"frontmatter":{"k":"x"},"status":"active","createdAt":"2026-01-01T00:00:00Z","updatedAt":"2026-01-01T00:00:00Z"}"#, "\n"),
        pin_line: r#"{"id":"probe-id","name":"p1","description":"d","version":2,"sections":[{"name":"main","body":"body"}],"pinned":1,"frontmatter":{"k":"x"},"status":"active","createdAt":"2026-01-01T00:00:00Z","updatedAt":"2026-11-12T21:11:39.484878799Z"}"#,
        unpin_line: r#"{"id":"probe-id","name":"p1","description":"d","version":2,"sections":[{"name":"main","body":"body"}],"frontmatter":{"k":"x"},"status":"active","createdAt":"2026-01-01T00:00:00Z","updatedAt":"2026-11-12T21:11:39.96520153Z"}"#,
    },
    // n2: "tags":null,"tags":["a"], after "version":1, (table :5129-5130).
    ProbeTwoRow {
        id: "n2",
        record: concat!(r#"{"id":"probe-id","name":"p1","version":1,"tags":null,"tags":["a"],"sections":[{"name":"main","body":"body"}],"frontmatter":{"k":"x"},"status":"active","createdAt":"2026-01-01T00:00:00Z","updatedAt":"2026-01-01T00:00:00Z"}"#, "\n"),
        pin_line: r#"{"id":"probe-id","name":"p1","version":2,"sections":[{"name":"main","body":"body"}],"tags":["a"],"pinned":1,"frontmatter":{"k":"x"},"status":"active","createdAt":"2026-01-01T00:00:00Z","updatedAt":"2026-11-12T21:11:40.489921595Z"}"#,
        unpin_line: r#"{"id":"probe-id","name":"p1","version":2,"sections":[{"name":"main","body":"body"}],"tags":["a"],"frontmatter":{"k":"x"},"status":"active","createdAt":"2026-01-01T00:00:00Z","updatedAt":"2026-11-12T21:11:41.041005373Z"}"#,
    },
    // n3: frontmatter {"k":1,"k":1.0} (table :5131-5132).
    ProbeTwoRow {
        id: "n3",
        record: concat!(r#"{"id":"probe-id","name":"p1","version":1,"sections":[{"name":"main","body":"body"}],"frontmatter":{"k":1,"k":1.0},"status":"active","createdAt":"2026-01-01T00:00:00Z","updatedAt":"2026-01-01T00:00:00Z"}"#, "\n"),
        pin_line: r#"{"id":"probe-id","name":"p1","version":2,"sections":[{"name":"main","body":"body"}],"pinned":1,"frontmatter":{"k":1},"status":"active","createdAt":"2026-01-01T00:00:00Z","updatedAt":"2026-11-12T21:11:41.601608594Z"}"#,
        unpin_line: r#"{"id":"probe-id","name":"p1","version":2,"sections":[{"name":"main","body":"body"}],"frontmatter":{"k":1},"status":"active","createdAt":"2026-01-01T00:00:00Z","updatedAt":"2026-11-12T21:11:42.182969776Z"}"#,
    },
];

/// The row of [`PROBE_TWO`] with this id.
pub fn probe_two(id: &str) -> &'static ProbeTwoRow {
    PROBE_TWO
        .iter()
        .find(|row| row.id == id)
        .unwrap_or_else(|| panic!("no go-lossy-probe-2 row {id}"))
}

/// Unprobed RD24 record: b00ctl with frontmatter `{"k":"x","k":null}`. Go
/// keeps the later null (RD24 point (3)): refused.
pub const R24_FM_LATER_NULL: &str = concat!(
    r#"{"id":"probe-id","name":"p1","version":1,"sections":[{"name":"main","body":"body"}],"frontmatter":{"k":"x","k":null},"status":"active","createdAt":"2026-01-01T00:00:00Z","updatedAt":"2026-01-01T00:00:00Z"}"#,
    "\n"
);

/// Unprobed RD24 record: b00ctl with frontmatter `{"k":null,"k":"x"}`. Go
/// keeps the later "x" and discards the earlier null: copied.
pub const R24_FM_EARLIER_NULL: &str = concat!(
    r#"{"id":"probe-id","name":"p1","version":1,"sections":[{"name":"main","body":"body"}],"frontmatter":{"k":null,"k":"x"},"status":"active","createdAt":"2026-01-01T00:00:00Z","updatedAt":"2026-01-01T00:00:00Z"}"#,
    "\n"
);

/// Unprobed RD24 record: b00ctl with its frontmatter member replaced by
/// `"frontmatter":{"a":1},"frontmatter":{"b":2}`. Go decodes the later
/// object into the earlier map (RD24 point (4)): nothing is discarded,
/// copied.
pub const R24_FM_MERGE: &str = concat!(
    r#"{"id":"probe-id","name":"p1","version":1,"sections":[{"name":"main","body":"body"}],"frontmatter":{"a":1},"frontmatter":{"b":2},"status":"active","createdAt":"2026-01-01T00:00:00Z","updatedAt":"2026-01-01T00:00:00Z"}"#,
    "\n"
);

/// Unprobed RD24 record: b00ctl with `"tags":["a"],"tags":null,` after
/// `"version":1,`. By RD24 point (3) a later null into a slice field sets it
/// to nil, so Go discards the earlier `["a"]`: refused. (The assumption line
/// of `go-lossy-probe-2`, not run, says the opposite; the parity test
/// `a8_go_pin_and_unpin_lose_unknown_and_repeated_keys` settles it.)
pub const R24_TAGS_LATER_NULL: &str = concat!(
    r#"{"id":"probe-id","name":"p1","version":1,"tags":["a"],"tags":null,"sections":[{"name":"main","body":"body"}],"frontmatter":{"k":"x"},"status":"active","createdAt":"2026-01-01T00:00:00Z","updatedAt":"2026-01-01T00:00:00Z"}"#,
    "\n"
);

/// A record pin and unpin refuse (RD23 or RD24): the record line, the name
/// the command uses, the refusal's key path and what Go's appended line
/// shows of the loss.
pub struct LossCase {
    pub id: &'static str,
    pub record: &'static str,
    /// The prompt name pin and unpin are given (`pin <name>@1`,
    /// `unpin <name>`), which is the name of the selected record.
    pub name: &'static str,
    /// The dotted key path the refusal names.
    pub key: &'static str,
    /// RD23 (a key Go does not read) when true, RD24 (a repeated key)
    /// otherwise.
    pub unknown: bool,
    /// Texts that must not be in Go's appended line (the lost key or value).
    pub go_line_lacks: &'static [&'static str],
    /// Texts that must be in Go's appended line (the value Go keeps).
    pub go_line_holds: &'static [&'static str],
}

impl LossCase {
    /// The Display of the refusal for this case.
    pub fn display(&self) -> String {
        if self.unknown {
            format!(
                "refused: the current record of {:?} holds the key {:?} that the Go tool does not read, and the new version would drop it; prompts.jsonl was not changed",
                self.name, self.key
            )
        } else {
            format!(
                "refused: the current record of {:?} holds the key {:?} more than once, and the new version would keep only the last value; prompts.jsonl was not changed",
                self.name, self.key
            )
        }
    }
}

/// b00ctl's line with `insert` placed right after `"version":1,`.
macro_rules! b00ctl_with {
    ($insert:literal) => {
        concat!(
            r#"{"id":"probe-id","name":"p1","version":1,"#,
            $insert,
            r#""sections":[{"name":"main","body":"body"}],"frontmatter":{"k":"x"},"status":"active","createdAt":"2026-01-01T00:00:00Z","updatedAt":"2026-01-01T00:00:00Z"}"#,
            "\n"
        )
    };
}

/// b00ctl's line with its sections and frontmatter members replaced.
macro_rules! b00ctl_body {
    ($sections_and_frontmatter:literal) => {
        concat!(
            r#"{"id":"probe-id","name":"p1","version":1,"#,
            $sections_and_frontmatter,
            r#","status":"active","createdAt":"2026-01-01T00:00:00Z","updatedAt":"2026-01-01T00:00:00Z"}"#,
            "\n"
        )
    };
}

/// The records of `go-lossy-probe-1` that pin and unpin refuse (b04a, b04b,
/// b04c under RD23; b05d under RD24).
pub fn probed_loss_cases() -> Vec<LossCase> {
    let line = |id: &str| -> &'static str {
        std::str::from_utf8(family_b(id)).expect("the probed refused records are UTF-8")
    };
    vec![
        LossCase {
            id: "b04a",
            record: line("b04a"),
            name: "p1",
            key: "extraField",
            unknown: true,
            go_line_lacks: &["extraField"],
            go_line_holds: &[],
        },
        LossCase {
            id: "b04b",
            record: line("b04b"),
            name: "p1",
            key: "sections.0.extra",
            unknown: true,
            go_line_lacks: &[r#""extra""#],
            go_line_holds: &[],
        },
        LossCase {
            id: "b04c",
            record: line("b04c"),
            name: "p1",
            key: "atlas.prime.zzz",
            unknown: true,
            go_line_lacks: &["zzz"],
            go_line_holds: &[r#""atlas":{"prime":{"domains":["a"]}}"#],
        },
        LossCase {
            id: "b05d",
            record: line("b05d"),
            name: "p1",
            key: "frontmatter.k",
            unknown: false,
            go_line_lacks: &[r#""k":1"#],
            go_line_holds: &[r#""frontmatter":{"k":2}"#],
        },
    ]
}

/// The refused cases of RD23 and RD24 that the probes did not run (stop
/// condition S3 applies to each).
pub fn unprobed_loss_cases() -> Vec<LossCase> {
    vec![
        // RD23: a legacy key Go does not use because atlas is present.
        LossCase {
            id: "r23-mulch-beside-atlas",
            record: b00ctl_with!(r#""atlas":{"budget":1},"mulch":{"budget":2},"#),
            name: "p1",
            key: "mulch",
            unknown: true,
            go_line_lacks: &["mulch", r#""budget":2"#],
            go_line_holds: &[r#""atlas":{"budget":1}"#],
        },
        LossCase {
            id: "r23-extends-mulch-beside-extends-atlas",
            record: b00ctl_with!(r#""extends_atlas":true,"extends_mulch":false,"#),
            name: "p1",
            key: "extends_mulch",
            unknown: true,
            go_line_lacks: &["extends_mulch"],
            go_line_holds: &[r#""extends_atlas":true"#],
        },
        // RD23: a case variant of a legacy key is matched exactly, so MULCH
        // binds to nothing.
        LossCase {
            id: "r23-MULCH",
            record: b00ctl_with!(r#""MULCH":{"budget":2},"#),
            name: "p1",
            key: "MULCH",
            unknown: true,
            go_line_lacks: &["MULCH", "budget"],
            go_line_holds: &[],
        },
        // RD23: an unknown key inside a used mulch.
        LossCase {
            id: "r23-unknown-in-used-mulch",
            record: b00ctl_with!(r#""mulch":{"budget":2,"zzz":1},"#),
            name: "p1",
            key: "mulch.zzz",
            unknown: true,
            go_line_lacks: &["zzz"],
            go_line_holds: &[r#""atlas":{"budget":2}"#],
        },
        // RD24 point (1): a repeated top-level key.
        LossCase {
            id: "r24-top-level-repeat",
            record: b00ctl_with!(r#""emitDir":"a","emitDir":"b","#),
            name: "p1",
            key: "emitDir",
            unknown: false,
            go_line_lacks: &[r#""emitDir":"a""#],
            go_line_holds: &[r#""emitDir":"b""#],
        },
        // RD24 point (1): b05m's record pinned by its folded name; Go keeps
        // the later "dup" and discards "p1".
        LossCase {
            id: "b05m-as-dup",
            record: std::str::from_utf8(family_b("b05m")).expect("b05m is UTF-8"),
            name: "dup",
            key: "NAME",
            unknown: false,
            go_line_lacks: &[r#""p1""#],
            go_line_holds: &[r#""name":"dup""#],
        },
        // RD24 point (1): a repeated key inside a section.
        LossCase {
            id: "r24-section-repeat",
            record: b00ctl_body!(
                r#""sections":[{"name":"main","body":"one","body":"two"}],"frontmatter":{"k":"x"}"#
            ),
            name: "p1",
            key: "sections.0.body",
            unknown: false,
            go_line_lacks: &[r#""one""#],
            go_line_holds: &[r#""body":"two""#],
        },
        // RD24 point (1): a repeated key inside atlas.
        LossCase {
            id: "r24-atlas-repeat",
            record: b00ctl_with!(r#""atlas":{"budget":1,"budget":2},"#),
            name: "p1",
            key: "atlas.budget",
            unknown: false,
            go_line_lacks: &[r#""budget":1"#],
            go_line_holds: &[r#""atlas":{"budget":2}"#],
        },
        // RD24 point (4): a repeated array in prime; element 0 of the later
        // array is decoded into element 0 of the earlier one.
        LossCase {
            id: "r24-prime-repeat",
            record: b00ctl_with!(r#""atlas":{"prime":{"domains":["a"],"domains":["b"]}},"#),
            name: "p1",
            key: "atlas.prime.domains.0",
            unknown: false,
            go_line_lacks: &[r#""a""#],
            go_line_holds: &[r#""domains":["b"]"#],
        },
        // RD24 point (1): a repeated key inside a used mulch.
        LossCase {
            id: "r24-used-mulch-inner-repeat",
            record: b00ctl_with!(r#""mulch":{"budget":1,"budget":2},"#),
            name: "p1",
            key: "mulch.budget",
            unknown: false,
            go_line_lacks: &[r#""budget":1"#],
            go_line_holds: &[r#""atlas":{"budget":2}"#],
        },
        // RD24: a repeated used mulch is used only at its last occurrence.
        LossCase {
            id: "r24-mulch-repeat",
            record: b00ctl_with!(r#""mulch":{"budget":1},"mulch":{"budget":2},"#),
            name: "p1",
            key: "mulch",
            unknown: false,
            go_line_lacks: &[r#""budget":1"#],
            go_line_holds: &[r#""atlas":{"budget":2}"#],
        },
        // RD24 point (3): a repeated key in a nested frontmatter object.
        LossCase {
            id: "r24-nested-frontmatter-repeat",
            record: b00ctl_body!(
                r#""sections":[{"name":"main","body":"body"}],"frontmatter":{"k":{"n":1,"n":2}}"#
            ),
            name: "p1",
            key: "frontmatter.k.n",
            unknown: false,
            go_line_lacks: &[r#""n":1"#],
            go_line_holds: &[r#""frontmatter":{"k":{"n":2}}"#],
        },
        // RD24 point (3): inside frontmatter the later null wins.
        LossCase {
            id: "r24-fm-later-null",
            record: R24_FM_LATER_NULL,
            name: "p1",
            key: "frontmatter.k",
            unknown: false,
            go_line_lacks: &[r#""x""#],
            go_line_holds: &[r#""frontmatter":{"k":null}"#],
        },
        // RD24 point (3): a later null into a slice field sets it to nil.
        LossCase {
            id: "r24-tags-later-null",
            record: R24_TAGS_LATER_NULL,
            name: "p1",
            key: "tags",
            unknown: false,
            go_line_lacks: &["tags", r#""a""#],
            go_line_holds: &[],
        },
    ]
}

/// The repeated-key records RD24 copies (Go discards a null or a value that
/// reads the same): n1, n2, n3 of `go-lossy-probe-2`, r24-fm-earlier-null
/// and r24-fm-merge, with the show read-back check for each.
pub fn copied_repeat_records() -> Vec<(&'static str, &'static str)> {
    vec![
        ("n1", probe_two("n1").record),
        ("n2", probe_two("n2").record),
        ("n3", probe_two("n3").record),
        ("r24-fm-earlier-null", R24_FM_EARLIER_NULL),
        ("r24-fm-merge", R24_FM_MERGE),
    ]
}
