//! Parity cases: a case is a typed sequence of [`Step`]s run against the Go
//! tool and the library in two fresh project roots with identical setup.
//!
//! [`lower`] maps each step to exactly one Go argv (the flag grammar of
//! `cli.go`: `--section Name=body`, `--fm k=v`, `--required a,b`, ...) and one
//! library call. After every step [`Case::step`] compares, after
//! normalisation:
//!
//! - the exit status (0 or 1);
//! - the stream of stdout JSON documents;
//! - the error string, exactly, except three exemptions compared by prefix
//!   only: operating-system error text (the step names the prefix),
//!   `invalid rule pattern: ` and `invalid JSONL line N in <root>/.luuna/beacon/<file>: `;
//! - whether Go printed `Migrated .canopy to .luuna/beacon` on stderr against
//!   the result of `Beacon::migrate_legacy`;
//! - the store: the files under `.luuna/beacon` (and a leftover `.canopy`)
//!   plus `.gitattributes` and `.gitignore` at the root, each file's bytes
//!   after normalisation and each file's permission bits (unix). No `.tmp`
//!   or `.lock` file may remain unless the case wrote it.
//!
//! The A4 differences are handled by [`reconcile_runtime`]: the runtime keys
//! are removed from the library document after being checked, and Go's
//! document must not have them.

use super::go::{self, GoRun};
use super::normalise::{sort_map_ordered_arrays, strip_runtime_fields, Normaliser};
use super::payload::{self, Done};
use luuna_beacon::{
    Beacon, BeaconError, CreatePrompt, CreateSchema, ListFilter, Rule, Section, UpdatePrompt,
};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};
use tempfile::TempDir;

/// The project name `init` writes into the config; ids are `<project>-<hex>`.
pub const DEFAULT_PROJECT: &str = "luuna-beacon";
/// The stderr line Go prints when it moves `.canopy`.
pub const MIGRATED_LINE: &str = "Migrated .canopy to .luuna/beacon";

/// A step of a later library plan. Those plans implement this in their own
/// support files (`authoring.rs`, `artifacts.rs`); it goes through the same
/// run, error-document and store comparison as every other step.
pub trait PlanStep {
    /// The Go argv without `--json`; `None` for a raw step.
    fn argv(&self) -> Option<Vec<String>>;

    /// The library call; called only when [`PlanStep::argv`] is `Some`.
    fn call(&self, beacon: &Beacon) -> Result<Done, BeaconError>;

    /// Applies a raw step to one project root; called only when
    /// [`PlanStep::argv`] is `None`.
    fn apply_raw(&self, root: &Path);

    /// A short text for failure messages of a raw step.
    fn describe_raw(&self) -> String;

    /// Paths (relative to the project root) a raw step writes, so the store
    /// comparison accepts them.
    fn written_paths(&self) -> Vec<String> {
        Vec::new()
    }
}

/// One step of a case.
pub enum Step {
    Init,
    Create(CreatePrompt),
    Update {
        reference: String,
        input: UpdatePrompt,
    },
    Archive(String),
    List(ListFilter),
    Show(String),
    History {
        reference: String,
        /// `None` leaves Go's default of 20.
        limit: Option<usize>,
    },
    /// `render <reference>` with `--json`. `expected_resolved_from` is written
    /// by hand from the case's prompts: the names in the resolver's walk order.
    Render {
        reference: String,
        expected_resolved_from: &'static [&'static str],
    },
    /// `render <reference> --format json`: the bare record.
    RenderJson(String),
    Validate(String),
    ValidateAll,
    SchemaCreate(CreateSchema),
    SchemaList,
    SchemaShow(String),
    SchemaRuleAdd {
        schema: String,
        rule: Rule,
    },
    /// Writes a raw file (relative to the project root) on both sides.
    WriteFile {
        path: String,
        bytes: Vec<u8>,
    },
    /// Sets a file's modification time to `seconds_ago` seconds before now.
    SetMtime {
        path: String,
        seconds_ago: u64,
    },
    /// A step of a later library plan, given by its own [`PlanStep`].
    Plan(Box<dyn PlanStep>),
}

// ---- constructors that keep the tests short --------------------------------

pub fn sec(name: &str, body: &str) -> Section {
    Section {
        name: name.to_owned(),
        body: body.to_owned(),
        required: None,
    }
}

pub fn pairs(items: &[(&str, &str)]) -> Vec<(String, String)> {
    items
        .iter()
        .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
        .collect()
}

pub fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|item| (*item).to_owned()).collect()
}

/// `create --name <name> --section k=v ...`
pub fn create(name: &str, sections: &[(&str, &str)]) -> Step {
    Step::Create(CreatePrompt {
        name: name.to_owned(),
        sections: sections.iter().map(|(n, b)| sec(n, b)).collect(),
        ..Default::default()
    })
}

pub fn create_with(input: CreatePrompt) -> Step {
    Step::Create(input)
}

pub fn update(reference: &str, input: UpdatePrompt) -> Step {
    Step::Update {
        reference: reference.to_owned(),
        input,
    }
}

pub fn archive(reference: &str) -> Step {
    Step::Archive(reference.to_owned())
}

pub fn list() -> Step {
    Step::List(ListFilter::default())
}

pub fn show(reference: &str) -> Step {
    Step::Show(reference.to_owned())
}

pub fn history(reference: &str, limit: Option<usize>) -> Step {
    Step::History {
        reference: reference.to_owned(),
        limit,
    }
}

pub fn render(reference: &str, expected_resolved_from: &'static [&'static str]) -> Step {
    Step::Render {
        reference: reference.to_owned(),
        expected_resolved_from,
    }
}

pub fn render_json(reference: &str) -> Step {
    Step::RenderJson(reference.to_owned())
}

pub fn validate(name: &str) -> Step {
    Step::Validate(name.to_owned())
}

pub fn schema_create(name: &str, required: &[&str], optional: &[&str]) -> Step {
    Step::SchemaCreate(CreateSchema {
        name: name.to_owned(),
        required: strings(required),
        optional: strings(optional),
    })
}

pub fn schema_show(name: &str) -> Step {
    Step::SchemaShow(name.to_owned())
}

pub fn rule_add(schema: &str, section: &str, pattern: &str, message: &str) -> Step {
    Step::SchemaRuleAdd {
        schema: schema.to_owned(),
        rule: Rule {
            section: section.to_owned(),
            pattern: pattern.to_owned(),
            message: message.to_owned(),
        },
    }
}

pub fn write_file(path: &str, bytes: &[u8]) -> Step {
    Step::WriteFile {
        path: path.to_owned(),
        bytes: bytes.to_vec(),
    }
}

pub fn set_mtime(path: &str, seconds_ago: u64) -> Step {
    Step::SetMtime {
        path: path.to_owned(),
        seconds_ago,
    }
}

// ---- lowering ---------------------------------------------------------------

/// The library call of a step.
pub type LibCall<'a> = Box<dyn Fn(&Beacon) -> Result<Done, BeaconError> + 'a>;

/// A step lowered to its two sides.
pub struct Lowered<'a> {
    /// The Go argv without `--json` (the harness appends it); `None` for the
    /// raw file steps, which run on the files directly.
    pub argv: Option<Vec<String>>,
    /// The library call for the step.
    pub call: LibCall<'a>,
}

/// A flag value. Go's `option()` ignores a value that starts with `--`, so
/// such a value could never reach the program; refuse it here instead of
/// letting the two sides drift apart silently.
fn val(value: &str) -> String {
    assert!(
        !value.starts_with("--"),
        "a flag value starting with `--` is dropped by Go's option(): {value:?}"
    );
    value.to_owned()
}

fn flag(argv: &mut Vec<String>, name: &str, value: &str) {
    argv.push(name.to_owned());
    argv.push(val(value));
}

fn flags(argv: &mut Vec<String>, name: &str, values: &[String]) {
    for value in values {
        flag(argv, name, value);
    }
}

fn section_flags(argv: &mut Vec<String>, sections: &[Section]) {
    for section in sections {
        assert!(!section.name.contains('='), "section names hold no `=`");
        flag(
            argv,
            "--section",
            &format!("{}={}", section.name, section.body),
        );
    }
}

fn pair_flags(argv: &mut Vec<String>, name: &str, pairs: &[(String, String)]) {
    for (key, value) in pairs {
        flag(argv, name, &format!("{key}={value}"));
    }
}

fn create_argv(input: &CreatePrompt) -> Vec<String> {
    let mut argv = vec!["create".to_owned()];
    flag(&mut argv, "--name", &input.name);
    for (name, value) in [
        ("--description", &input.description),
        ("--extends", &input.extends),
        ("--schema", &input.schema),
        ("--emit-as", &input.emit_as),
        ("--emit-dir", &input.emit_dir),
        ("--status", &input.status),
    ] {
        if !value.is_empty() {
            flag(&mut argv, name, value);
        }
    }
    flags(&mut argv, "--mixin", &input.mixins);
    flags(&mut argv, "--tag", &input.tags);
    section_flags(&mut argv, &input.sections);
    pair_flags(&mut argv, "--fm", &input.frontmatter);
    argv
}

fn update_argv(reference: &str, input: &UpdatePrompt) -> Vec<String> {
    let mut argv = vec!["update".to_owned(), reference.to_owned()];
    for (name, value) in [
        ("--name", &input.name),
        ("--description", &input.description),
        ("--status", &input.status),
        ("--extends", &input.extends),
        ("--schema", &input.schema),
        ("--emit-as", &input.emit_as),
        ("--emit-dir", &input.emit_dir),
    ] {
        if let Some(value) = value {
            flag(&mut argv, name, value);
        }
    }
    flags(&mut argv, "--tag", &input.add_tags);
    flags(&mut argv, "--untag", &input.remove_tags);
    section_flags(&mut argv, &input.sections);
    flags(&mut argv, "--remove-section", &input.remove_sections);
    flags(&mut argv, "--mixin", &input.add_mixins);
    flags(&mut argv, "--remove-mixin", &input.remove_mixins);
    pair_flags(&mut argv, "--fm", &input.frontmatter);
    flags(&mut argv, "--remove-fm", &input.remove_frontmatter);
    argv
}

fn list_argv(filter: &ListFilter) -> Vec<String> {
    let mut argv = vec!["list".to_owned()];
    for (name, value) in [
        ("--status", &filter.status),
        ("--tag", &filter.tag),
        ("--extends", &filter.extends),
        ("--mixin", &filter.mixin),
    ] {
        if let Some(value) = value {
            flag(&mut argv, name, value);
        }
    }
    argv
}

fn strs(items: &[&str]) -> Vec<String> {
    items.iter().map(|item| (*item).to_owned()).collect()
}

/// Maps one step to its Go argv and its library call.
pub fn lower(step: &Step) -> Lowered<'_> {
    match step {
        Step::Init => Lowered {
            argv: Some(strs(&["init"])),
            call: Box::new(|beacon| beacon.init().map(Done::Init)),
        },
        Step::Create(input) => Lowered {
            argv: Some(create_argv(input)),
            call: Box::new(move |beacon| beacon.create(input).map(Done::Created)),
        },
        Step::Update { reference, input } => Lowered {
            argv: Some(update_argv(reference, input)),
            call: Box::new(move |beacon| beacon.update(reference, input).map(Done::Updated)),
        },
        Step::Archive(reference) => Lowered {
            argv: Some(vec!["archive".to_owned(), reference.clone()]),
            call: Box::new(move |beacon| beacon.archive(reference).map(Done::Archived)),
        },
        Step::List(filter) => Lowered {
            argv: Some(list_argv(filter)),
            call: Box::new(move |beacon| beacon.list(filter).map(Done::Listed)),
        },
        Step::Show(reference) => Lowered {
            argv: Some(vec!["show".to_owned(), reference.clone()]),
            call: Box::new(move |beacon| beacon.show(reference).map(Done::Shown)),
        },
        Step::History { reference, limit } => {
            let mut argv = vec!["history".to_owned(), reference.clone()];
            if let Some(limit) = limit {
                flag(&mut argv, "--limit", &limit.to_string());
            }
            Lowered {
                argv: Some(argv),
                call: Box::new(move |beacon| {
                    beacon
                        .history(reference, limit.unwrap_or(20))
                        .map(|versions| Done::History {
                            reference: reference.clone(),
                            versions,
                        })
                }),
            }
        }
        Step::Render { reference, .. } => Lowered {
            argv: Some(vec!["render".to_owned(), reference.clone()]),
            call: Box::new(move |beacon| beacon.render(reference).map(Done::Rendered)),
        },
        Step::RenderJson(reference) => Lowered {
            argv: Some(vec![
                "render".to_owned(),
                reference.clone(),
                "--format".to_owned(),
                "json".to_owned(),
            ]),
            call: Box::new(move |beacon| beacon.render(reference).map(Done::RenderedBare)),
        },
        Step::Validate(name) => Lowered {
            argv: Some(vec!["validate".to_owned(), name.clone()]),
            call: Box::new(move |beacon| beacon.validate(name).map(Done::Validated)),
        },
        Step::ValidateAll => Lowered {
            argv: Some(strs(&["validate", "--all"])),
            call: Box::new(|beacon| beacon.validate_all().map(Done::ValidatedAll)),
        },
        Step::SchemaCreate(input) => {
            let mut argv = strs(&["schema", "create"]);
            flag(&mut argv, "--name", &input.name);
            if !input.required.is_empty() {
                flag(&mut argv, "--required", &input.required.join(","));
            }
            if !input.optional.is_empty() {
                flag(&mut argv, "--optional", &input.optional.join(","));
            }
            Lowered {
                argv: Some(argv),
                call: Box::new(move |beacon| beacon.schema_create(input).map(Done::SchemaCreated)),
            }
        }
        Step::SchemaList => Lowered {
            argv: Some(strs(&["schema", "list"])),
            call: Box::new(|beacon| beacon.schema_list().map(Done::SchemaListed)),
        },
        Step::SchemaShow(name) => Lowered {
            argv: Some(vec!["schema".to_owned(), "show".to_owned(), name.clone()]),
            call: Box::new(move |beacon| beacon.schema_show(name).map(Done::SchemaShown)),
        },
        Step::SchemaRuleAdd { schema, rule } => {
            let mut argv = vec![
                "schema".to_owned(),
                "rule".to_owned(),
                "add".to_owned(),
                schema.clone(),
            ];
            flag(&mut argv, "--section", &rule.section);
            flag(&mut argv, "--pattern", &rule.pattern);
            flag(&mut argv, "--message", &rule.message);
            Lowered {
                argv: Some(argv),
                call: Box::new(move |beacon| {
                    beacon.schema_rule_add(schema, rule).map(Done::RuleAdded)
                }),
            }
        }
        Step::WriteFile { .. } | Step::SetMtime { .. } => Lowered {
            argv: None,
            call: Box::new(|_| Ok(Done::Nothing)),
        },
        Step::Plan(plan) => Lowered {
            argv: plan.argv(),
            call: Box::new(move |beacon| plan.call(beacon)),
        },
    }
}

/// A short text for failure messages.
pub fn describe(step: &Step) -> String {
    match (lower(step).argv, step) {
        (Some(argv), _) => argv.join(" "),
        (None, Step::WriteFile { path, bytes }) => format!("write {path} ({} bytes)", bytes.len()),
        (None, Step::SetMtime { path, seconds_ago }) => {
            format!("set mtime of {path} to {seconds_ago}s ago")
        }
        (None, Step::Plan(plan)) => plan.describe_raw(),
        (None, _) => "step".to_owned(),
    }
}

fn apply_raw(step: &Step, root: &Path) {
    match step {
        Step::WriteFile { path, bytes } => {
            let target = root.join(path);
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent).expect("create the parent of a raw file");
            }
            fs::write(&target, bytes).expect("write a raw file");
        }
        Step::SetMtime { path, seconds_ago } => {
            let file = fs::File::options()
                .write(true)
                .open(root.join(path))
                .expect("open the file whose mtime is set");
            file.set_modified(SystemTime::now() - Duration::from_secs(*seconds_ago))
                .expect("set the modification time");
        }
        Step::Plan(plan) => plan.apply_raw(root),
        _ => unreachable!("not a raw step"),
    }
}

fn is_raw(step: &Step) -> bool {
    matches!(step, Step::WriteFile { .. } | Step::SetMtime { .. })
        || matches!(step, Step::Plan(plan) if plan.argv().is_none())
}

// ---- the library side -------------------------------------------------------

/// What the library side of a step produced, as Go would have printed it.
pub struct LibRun {
    pub code: i32,
    pub documents: Vec<Value>,
    /// Whether the library call succeeded.
    pub ok: bool,
    pub error: Option<String>,
    /// The result of `migrate_legacy` (Go's stderr line).
    pub migrated: bool,
}

/// Runs the library call of `step` the way every Go command starts: the
/// legacy move first, then the call.
pub fn run_lib(beacon: &Beacon, step: &Step) -> LibRun {
    let lowered = lower(step);
    let command = lowered
        .argv
        .as_ref()
        .map(|argv| argv[0].clone())
        .expect("run_lib is for command steps");
    let failed = |error: BeaconError, migrated: bool| {
        let message = error.to_string();
        LibRun {
            code: 1,
            documents: vec![payload::error_document(&command, &message)],
            ok: false,
            error: Some(message),
            migrated,
        }
    };
    let migrated = match beacon.migrate_legacy() {
        Ok(moved) => moved,
        Err(error) => return failed(error, false),
    };
    match (lowered.call)(beacon) {
        Ok(done) => LibRun {
            code: payload::exit_code(&done),
            documents: payload::documents(&done),
            ok: true,
            error: None,
            migrated,
        },
        Err(error) => failed(error, migrated),
    }
}

// ---- A4 differences ---------------------------------------------------------

/// The checks of `integrations.rs:1450-1468` on a `resolvedFrom` value: a
/// non-empty array of at most 64 strings, every entry the name of a record in
/// the same side's list output unless the case archived it.
pub fn assert_runtime_source_chain(chain: &Value, listed: &[String], archived: &HashSet<String>) {
    let entries = chain
        .as_array()
        .unwrap_or_else(|| panic!("resolvedFrom is not an array: {chain}"));
    assert!(!entries.is_empty(), "resolvedFrom is empty");
    assert!(
        entries.len() <= 64,
        "resolvedFrom has {} entries, more than 64",
        entries.len()
    );
    for entry in entries {
        let name = entry
            .as_str()
            .unwrap_or_else(|| panic!("resolvedFrom entry {entry} is not a string"));
        assert!(
            listed.iter().any(|listed| listed == name) || archived.contains(name),
            "resolvedFrom entry {name:?} is not in list output {listed:?} and was not archived"
        );
    }
}

fn listed_names(beacon: &Beacon) -> Vec<String> {
    beacon
        .list(&ListFilter::default())
        .expect("list for the source chain check")
        .into_iter()
        .map(|prompt| prompt.name)
        .collect()
}

/// Applies the A4 differences to one step's documents (before
/// normalisation): the runtime keys are removed from the library document
/// after being checked, Go's document must not have them, and nothing else is
/// removed.
pub fn reconcile_runtime(
    step: &Step,
    go_docs: &mut [Value],
    lib_docs: &mut [Value],
    lib_ok: bool,
    beacon: &Beacon,
    archived: &HashSet<String>,
) {
    match step {
        Step::Render {
            expected_resolved_from,
            ..
        } => {
            for doc in go_docs.iter_mut() {
                let removed = strip_runtime_fields("render", doc);
                assert!(
                    removed.is_empty(),
                    "Go's render document has runtime keys: {:?}",
                    removed.keys().collect::<Vec<_>>()
                );
            }
            for doc in lib_docs.iter_mut() {
                let removed = strip_runtime_fields("render", doc);
                if !lib_ok {
                    continue;
                }
                assert_eq!(
                    removed.keys().map(String::as_str).collect::<Vec<_>>(),
                    vec!["resolvedFrom"],
                    "the library's render document must carry exactly resolvedFrom"
                );
                let chain = &removed["resolvedFrom"];
                assert_eq!(
                    chain,
                    &json!(expected_resolved_from),
                    "resolvedFrom differs from the case's expected sources"
                );
                assert_runtime_source_chain(chain, &listed_names(beacon), archived);
            }
        }
        Step::RenderJson(_) => {
            for doc in go_docs.iter().chain(lib_docs.iter()) {
                assert!(
                    doc.get("resolvedFrom").is_none(),
                    "render --format json must not have resolvedFrom"
                );
            }
        }
        Step::Validate(_) => {
            for doc in go_docs.iter_mut() {
                let removed = strip_runtime_fields("validate", doc);
                assert!(
                    removed.is_empty(),
                    "Go's validate document has top-level runtime keys: {:?}",
                    removed.keys().collect::<Vec<_>>()
                );
            }
            let mut stripped = false;
            for doc in lib_docs.iter_mut() {
                let first = doc["results"].get(0).cloned();
                let removed = strip_runtime_fields("validate", doc);
                if removed.is_empty() {
                    continue;
                }
                stripped = true;
                let first = first.expect("a stripped document has one result");
                assert_eq!(removed.len(), 3, "valid, errors and warnings only");
                for key in ["valid", "errors", "warnings"] {
                    assert_eq!(
                        removed[key], first[key],
                        "top-level {key} differs from results[0]"
                    );
                }
            }
            if lib_ok {
                assert!(
                    stripped,
                    "the library's named validate lacks the runtime keys"
                );
            }
        }
        _ => {}
    }
}

// ---- error comparison -------------------------------------------------------

fn is_error_document(doc: &Value) -> bool {
    doc.as_object().is_some_and(|object| {
        object.len() == 3
            && object.get("success") == Some(&Value::Bool(false))
            && object.contains_key("command")
            && object.get("error").is_some_and(Value::is_string)
    })
}

/// Takes the error string out of every error document (replacing it with a
/// placeholder so the rest of the document can be compared exactly).
fn take_errors(docs: &mut [Value]) -> Vec<String> {
    let mut errors = Vec::new();
    for doc in docs.iter_mut() {
        if is_error_document(doc) {
            let text = doc["error"].as_str().unwrap_or_default().to_owned();
            doc["error"] = Value::String("<error>".to_owned());
            errors.push(text);
        }
    }
    errors
}

/// Exact equality except for the three named exemptions.
pub fn assert_errors_match(go: &str, lib: &str, os_prefix: Option<&str>, context: &str) {
    if let Some(prefix) = os_prefix {
        assert!(
            go.starts_with(prefix) && lib.starts_with(prefix),
            "{context}: operating-system error prefix {prefix:?}: Go {go:?}, library {lib:?}"
        );
    } else if go.starts_with("invalid rule pattern: ") {
        assert!(
            lib.starts_with("invalid rule pattern: "),
            "{context}: Go {go:?}, library {lib:?}"
        );
    } else if go.starts_with("invalid JSONL line ") {
        let end = go.find(": ").expect("Go's JSONL error has a `: `") + 2;
        assert!(
            lib.starts_with(&go[..end]),
            "{context}: expected the prefix {:?}; library {lib:?}",
            &go[..end]
        );
    } else {
        assert_eq!(go, lib, "{context}: error strings differ");
    }
}

fn compare_documents(
    go_docs: Vec<Value>,
    lib_docs: Vec<Value>,
    os_prefix: Option<&str>,
    context: &str,
) {
    let (mut go_docs, mut lib_docs) = (go_docs, lib_docs);
    let go_errors = take_errors(&mut go_docs);
    let lib_errors = take_errors(&mut lib_docs);
    assert_eq!(
        go_errors.len(),
        lib_errors.len(),
        "{context}: error documents differ: Go {go_errors:?}, library {lib_errors:?}"
    );
    for (go, lib) in go_errors.iter().zip(&lib_errors) {
        assert_errors_match(go, lib, os_prefix, context);
    }
    assert_eq!(go_docs, lib_docs, "{context}: stdout documents differ");
}

// ---- the store --------------------------------------------------------------

/// One store file: normalised bytes and permission bits.
#[derive(Debug, PartialEq, Eq)]
pub struct StoredFile {
    pub bytes: Vec<u8>,
    pub mode: u32,
}

fn walk(base: &Path, dir: &Path, found: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries {
        let entry = entry.expect("read a store folder entry");
        let path = entry.path();
        if entry.file_type().expect("file type").is_dir() {
            walk(base, &path, found);
        } else {
            found.push(path.strip_prefix(base).expect("under base").to_path_buf());
        }
    }
}

#[cfg(unix)]
fn mode_of(path: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(path)
        .expect("metadata of a store file")
        .permissions()
        .mode()
        & 0o7777
}

#[cfg(not(unix))]
fn mode_of(_: &Path) -> u32 {
    0
}

/// The relative paths of the compared files of a project root.
pub fn store_paths(root: &Path) -> Vec<String> {
    let mut found = Vec::new();
    walk(root, &root.join(".luuna").join("beacon"), &mut found);
    walk(root, &root.join(".canopy"), &mut found);
    for name in [".gitattributes", ".gitignore"] {
        if root.join(name).is_file() {
            found.push(PathBuf::from(name));
        }
    }
    let mut names: Vec<String> = found
        .into_iter()
        .map(|path| path.to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

/// Reads the compared files of a root, normalising with `normaliser`, and
/// panics if a `.tmp` or `.lock` file remains that the case did not write.
pub fn snapshot(
    root: &Path,
    normaliser: &mut Normaliser,
    written: &HashSet<String>,
) -> BTreeMap<String, StoredFile> {
    let mut files = BTreeMap::new();
    for relative in store_paths(root) {
        let name = relative.rsplit('/').next().unwrap_or(&relative);
        if !written.contains(&relative) {
            assert!(
                !name.starts_with(".tmp") && !name.ends_with(".lock"),
                "a temporary or lock file remains: {relative}"
            );
        }
        let path = root.join(&relative);
        let raw = fs::read(&path).expect("read a store file");
        assert!(
            !String::from_utf8_lossy(&raw).contains("resolvedFrom"),
            "{relative} contains resolvedFrom"
        );
        let bytes = if relative.ends_with(".jsonl") {
            normaliser.jsonl(&raw)
        } else {
            normaliser.text(&String::from_utf8_lossy(&raw)).into_bytes()
        };
        files.insert(
            relative,
            StoredFile {
                bytes,
                mode: mode_of(&path),
            },
        );
    }
    files
}

// ---- roots ------------------------------------------------------------------

fn is_git_marker(dot_git: &Path) -> bool {
    dot_git.is_file() || dot_git.join("HEAD").exists()
}

/// A fresh canonicalised project root in the system temporary folder, never
/// inside a git repository and never inside the reference clone.
pub fn fresh_root(label: &str) -> (TempDir, PathBuf) {
    let dir = tempfile::Builder::new()
        .prefix(&format!("beacon-{label}-"))
        .tempdir()
        .expect("create a temporary project root");
    let path = dir.path().canonicalize().expect("canonical project root");
    for folder in path.ancestors() {
        assert!(
            !is_git_marker(&folder.join(".git")),
            "project root {} is inside the git repository {}",
            path.display(),
            folder.display()
        );
    }
    assert!(
        !path.starts_with(&go::go_build().clone),
        "project root {} is inside the reference clone",
        path.display()
    );
    (dir, path)
}

// ---- the case ---------------------------------------------------------------

/// What a step produced on the Go side (the library side is equal to it, or
/// the step panicked).
pub struct Seen {
    pub code: i32,
    /// Go's documents after the runtime-key check, before normalisation.
    pub raw: Vec<Value>,
    /// Go's documents after normalisation.
    pub documents: Vec<Value>,
    pub error: Option<String>,
    pub migrated: bool,
    /// The library side's documents before normalisation.
    pub lib_raw: Vec<Value>,
}

impl Seen {
    fn empty() -> Self {
        Seen {
            code: 0,
            raw: Vec::new(),
            documents: Vec::new(),
            error: None,
            migrated: false,
            lib_raw: Vec::new(),
        }
    }
}

/// A case: two roots, identical setup, one normaliser per side.
pub struct Case {
    _go_dir: TempDir,
    _lib_dir: TempDir,
    go_root: PathBuf,
    lib_root: PathBuf,
    go_norm: Normaliser,
    lib_norm: Normaliser,
    beacon: Beacon,
    archived: HashSet<String>,
    written: HashSet<String>,
    log: Vec<String>,
}

impl Case {
    pub fn new() -> Self {
        Self::with_project(DEFAULT_PROJECT)
    }

    /// `project` is the project name the case's config will hold.
    pub fn with_project(project: &str) -> Self {
        let (go_dir, go_root) = fresh_root("go");
        let (lib_dir, lib_root) = fresh_root("lib");
        let beacon = Beacon::open(&lib_root).expect("open the library project root");
        Case {
            go_norm: Normaliser::new(project, &go_root),
            lib_norm: Normaliser::new(project, &lib_root),
            _go_dir: go_dir,
            _lib_dir: lib_dir,
            go_root,
            lib_root,
            beacon,
            archived: HashSet::new(),
            written: HashSet::new(),
            log: Vec::new(),
        }
    }

    pub fn go_root(&self) -> &Path {
        &self.go_root
    }

    pub fn lib_root(&self) -> &Path {
        &self.lib_root
    }

    pub fn beacon(&self) -> &Beacon {
        &self.beacon
    }

    /// Runs several steps in order.
    pub fn steps(&mut self, steps: Vec<Step>) {
        for step in steps {
            self.step(step);
        }
    }

    /// Runs one step on both sides and compares everything.
    pub fn step(&mut self, step: Step) -> Seen {
        self.run(&step, None)
    }

    /// Like [`Case::step`] for a step that fails with an operating-system
    /// error: only the prefix (for example `open <root>/.luuna/beacon/config.yaml: `)
    /// of the error string is compared.
    pub fn step_os_error(&mut self, step: Step, prefix: &str) -> Seen {
        self.run(&step, Some(prefix))
    }

    fn run(&mut self, step: &Step, os_prefix: Option<&str>) -> Seen {
        let description = describe(step);
        self.log.push(description.clone());
        let context = format!(
            "step {} `{description}` (steps so far: {})",
            self.log.len(),
            self.log.join(" | ")
        );
        if is_raw(step) {
            apply_raw(step, &self.go_root);
            apply_raw(step, &self.lib_root);
            if let Step::WriteFile { path, .. } | Step::SetMtime { path, .. } = step {
                self.written.insert(path.clone());
            }
            if let Step::Plan(plan) = step {
                self.written.extend(plan.written_paths());
            }
            self.compare_stores(&context);
            return Seen::empty();
        }

        let lowered = lower(step);
        let argv = lowered.argv.clone().expect("a command step has an argv");
        let argv_refs: Vec<&str> = argv.iter().map(String::as_str).collect();
        let go_run: GoRun = go::run_go(&self.go_root, &argv_refs);
        let go_migrated = String::from_utf8_lossy(&go_run.stderr).contains(MIGRATED_LINE);
        let lib_run = run_lib(&self.beacon, step);

        assert_eq!(go_run.code, lib_run.code, "{context}: exit status differs");
        assert_eq!(
            go_migrated, lib_run.migrated,
            "{context}: Go's `{MIGRATED_LINE}` stderr line against migrate_legacy()"
        );

        let mut go_docs = go_run.documents;
        let mut lib_docs = lib_run.documents.clone();
        reconcile_runtime(
            step,
            &mut go_docs,
            &mut lib_docs,
            lib_run.ok,
            &self.beacon,
            &self.archived,
        );
        let raw = go_docs.clone();
        let lib_raw = lib_docs.clone();
        self.go_norm.documents(&mut go_docs);
        self.lib_norm.documents(&mut lib_docs);
        let go_error_text = {
            let mut copy = go_docs.clone();
            take_errors(&mut copy).first().cloned()
        };
        let seen_documents = go_docs.clone();
        compare_documents(go_docs, lib_docs, os_prefix, &context);

        if lib_run.ok {
            self.track_archived(step);
        }
        self.compare_stores(&context);
        Seen {
            code: go_run.code,
            raw,
            documents: seen_documents,
            error: go_error_text,
            migrated: go_migrated,
            lib_raw,
        }
    }

    fn track_archived(&mut self, step: &Step) {
        match step {
            Step::Archive(reference) => {
                self.archived
                    .insert(reference.split('@').next().unwrap_or_default().to_owned());
            }
            Step::Update { reference, input } => {
                let name = reference.split('@').next().unwrap_or_default().to_owned();
                match input.status.as_deref() {
                    Some("archived") => {
                        self.archived.insert(name);
                    }
                    Some(status) if !status.is_empty() => {
                        self.archived.remove(&name);
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    /// Compares the two stores: file set, normalised bytes, permission bits.
    pub fn compare_stores(&mut self, context: &str) {
        let go = snapshot(&self.go_root, &mut self.go_norm, &self.written);
        let lib = snapshot(&self.lib_root, &mut self.lib_norm, &self.written);
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
    }

    /// Compares the stores now (after a test changed a file by itself).
    pub fn check_stores(&mut self) {
        let context = format!("final store check (steps: {})", self.log.join(" | "));
        self.compare_stores(&context);
    }
}

impl Default for Case {
    fn default() -> Self {
        Self::new()
    }
}

// ---- one store read by both sides ------------------------------------------

/// One project root that both the Go tool and the library work in, for the
/// format tests. Reads are compared without placeholders: ids and timestamps
/// are the very same on both sides.
pub struct Shared {
    _dir: TempDir,
    root: PathBuf,
    beacon: Beacon,
    project: String,
    archived: HashSet<String>,
}

impl Shared {
    pub fn new() -> Self {
        let (dir, root) = fresh_root("shared");
        let beacon = Beacon::open(&root).expect("open the shared project root");
        Shared {
            _dir: dir,
            root,
            beacon,
            project: DEFAULT_PROJECT.to_owned(),
            archived: HashSet::new(),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn beacon(&self) -> &Beacon {
        &self.beacon
    }

    pub fn path(&self, relative: &str) -> PathBuf {
        self.root.join(relative)
    }

    pub fn store_file(&self, name: &str) -> PathBuf {
        self.root.join(".luuna").join("beacon").join(name)
    }

    /// Runs the step with the Go tool in the shared root.
    pub fn go(&self, step: &Step) -> GoRun {
        let argv = lower(step).argv.expect("a command step has an argv");
        let argv_refs: Vec<&str> = argv.iter().map(String::as_str).collect();
        go::run_go(&self.root, &argv_refs)
    }

    /// Runs the step with the library in the shared root.
    pub fn lib(&self, step: &Step) -> LibRun {
        run_lib(&self.beacon, step)
    }

    /// Applies a raw file step to the shared root.
    pub fn raw(&self, step: &Step) {
        assert!(is_raw(step), "not a raw step");
        apply_raw(step, &self.root);
    }

    /// Runs a read-only step with both tools on the same store and compares
    /// exit status and documents (map-ordered arrays sorted by name).
    pub fn both_read(&mut self, step: &Step) {
        let context = format!("shared read `{}`", describe(step));
        let go_run = self.go(step);
        let lib_run = self.lib(step);
        assert_eq!(go_run.code, lib_run.code, "{context}: exit status differs");
        let mut go_docs = go_run.documents;
        let mut lib_docs = lib_run.documents.clone();
        reconcile_runtime(
            step,
            &mut go_docs,
            &mut lib_docs,
            lib_run.ok,
            &self.beacon,
            &self.archived,
        );
        for doc in go_docs.iter_mut().chain(lib_docs.iter_mut()) {
            sort_map_ordered_arrays(doc, &self.project);
        }
        compare_documents(go_docs, lib_docs, None, &context);
    }
}

impl Default for Shared {
    fn default() -> Self {
        Self::new()
    }
}

// ---- hand-written store lines -----------------------------------------------

/// The timestamp hand-written records use unless a test needs another.
pub const STAMP: &str = "2024-01-01T00:00:00Z";

/// One hand-written prompt line (compact JSON, newline included). `extra`
/// is a JSON object whose keys are added or override the defaults.
pub fn prompt_record(
    id: &str,
    name: &str,
    version: i64,
    updated_at: &str,
    sections: &[(&str, &str)],
    extra: Value,
) -> String {
    let mut record = json!({
        "id": id,
        "name": name,
        "version": version,
        "sections": sections
            .iter()
            .map(|(name, body)| json!({"name": name, "body": body}))
            .collect::<Vec<_>>(),
        "status": "active",
        "createdAt": STAMP,
        "updatedAt": updated_at,
    });
    if let Value::Object(extra) = extra {
        for (key, value) in extra {
            record[key] = value;
        }
    }
    format!("{record}\n")
}

/// One hand-written schema line (compact JSON, newline included).
pub fn schema_record(
    id: &str,
    name: &str,
    required: &[&str],
    rules: &[(&str, &str, &str)],
    updated_at: &str,
) -> String {
    let mut record = json!({
        "id": id,
        "name": name,
        "requiredSections": required,
        "createdAt": STAMP,
        "updatedAt": updated_at,
    });
    if !rules.is_empty() {
        record["rules"] = rules
            .iter()
            .map(|(section, pattern, message)| {
                json!({"section": section, "pattern": pattern, "message": message})
            })
            .collect();
    }
    format!("{record}\n")
}
