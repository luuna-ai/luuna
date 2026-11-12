//! Native, Windows-only behavior fixtures captured from the TypeScript/Bun
//! reference CLI.  The fixtures deliberately live outside this crate so the
//! same corpus can be retained after Bun is removed from the build.
//!
//! Values which the public CLI intentionally generates at runtime (IDs,
//! dates, timestamps, and temporary roots) are replaced only with named
//! tokens.  Everything else -- exit code, stderr, JSON field order, store
//! order, generated reports, and hook protocol bytes -- is asserted here.

#![cfg(windows)]

use roots_core::serde_json::{self, Value};
use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

const EVIDENCE: &[u8] = b"Evidence collected by the fixture agent.";

struct TempDir {
    root: PathBuf,
}

impl TempDir {
    fn new(label: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "luuna-roots-native-behavior-{label}-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir_all(&root).expect("create isolated fixture root");
        Self { root }
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        // This is a test-generated, uniquely named directory. It never
        // points at user/project data.
        let _ = fs::remove_dir_all(&self.root);
    }
}

struct FixtureProject {
    temp: TempDir,
    project: PathBuf,
    label: String,
}

impl FixtureProject {
    fn from_input(label: &str, input: &str) -> Self {
        let temp = TempDir::new(label);
        let project = temp.root.join(label);
        copy_tree(&compat_path(&format!("behavior/input/{input}")), &project);
        for store in ["proposals.jsonl", "questions.jsonl"] {
            let path = project.join(".luuna/roots").join(store);
            if !path.exists() {
                fs::write(path, "").expect("create omitted empty JSONL store");
            }
        }
        for directory in [".luuna/roots/agent/sprouts", ".luuna/roots/agent/notes"] {
            fs::create_dir_all(project.join(directory)).expect("create agent-owned fixture tree");
        }
        Self {
            temp,
            project,
            label: label.to_owned(),
        }
    }

    fn normalizers(&self) -> Normalizers {
        Normalizers {
            root: Some(self.temp.root.display().to_string()),
            project: Some(self.label.clone()),
            ..Normalizers::default()
        }
    }

    fn write_evidence(&self) {
        fs::write(self.project.join("evidence.txt"), EVIDENCE).expect("write fixture evidence");
    }

    fn write_read_note(&self) {
        let path = self
            .project
            .join(".luuna/roots/agent/notes/7528/2026-10-23-evidence.txt");
        fs::create_dir_all(path.parent().expect("note parent")).expect("create note parent");
        fs::write(path, EVIDENCE).expect("write read-state note");
    }
}

#[derive(Default)]
struct Normalizers {
    root: Option<String>,
    project: Option<String>,
    normalize_project: bool,
    values: BTreeMap<String, String>,
    timestamps: bool,
    note_date: Option<String>,
}

impl Normalizers {
    fn map(&mut self, actual: impl Into<String>, expected: impl Into<String>) {
        self.values.insert(actual.into(), expected.into());
    }
}

#[derive(Debug)]
struct CommandOutput {
    exit: i32,
    stdout: String,
    stderr: String,
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("workspace root")
        .to_path_buf()
}

fn compat_path(relative: &str) -> PathBuf {
    workspace_root().join("tests").join("compat").join(relative)
}

fn fixture(relative: &str) -> String {
    fs::read_to_string(compat_path(relative))
        .unwrap_or_else(|error| panic!("read fixture {relative}: {error}"))
}

fn fixture_bytes(relative: &str) -> Vec<u8> {
    fs::read(compat_path(relative))
        .unwrap_or_else(|error| panic!("read fixture bytes {relative}: {error}"))
}

fn json_fixture(relative: &str) -> String {
    let value: Value = serde_json::from_str(&fixture(relative))
        .unwrap_or_else(|error| panic!("parse JSON fixture {relative}: {error}"));
    format!(
        "{}\n",
        serde_json::to_string_pretty(&value).expect("pretty fixture JSON")
    )
}

fn copy_tree(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).expect("create fixture destination");
    for entry in fs::read_dir(source).expect("read fixture source tree") {
        let entry = entry.expect("fixture directory entry");
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        if entry.file_type().expect("fixture type").is_dir() {
            copy_tree(&source_path, &destination_path);
        } else {
            fs::copy(&source_path, &destination_path).unwrap_or_else(|error| {
                panic!(
                    "copy fixture {} to {}: {error}",
                    source_path.display(),
                    destination_path.display()
                )
            });
        }
    }
}

fn command_for(project: &Path, args: &[&str], environment: &[(&str, Option<&str>)]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_luuna-roots"));
    command
        .current_dir(project)
        .args(args)
        // Keep capture-style commands deterministic without making this an
        // interactive test. The reference corpus used the same override.
        .env("ROOTS_FORCE_TTY", "1")
        .env("NODE_ENV", "test")
        .env("ROOTS_USER", "Fixture Human")
        // Prevent a developer's global Git identity from changing tests that
        // exercise blank LUUNA_ROOTS_USER masking.
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "NUL")
        .env_remove("ROOTS_AGENT")
        .env_remove("LUUNA_ROOTS_AGENT")
        .env_remove("LUUNA_ROOTS_USER");
    for (key, value) in environment {
        match value {
            Some(value) => {
                command.env(key, value);
            }
            None => {
                command.env_remove(key);
            }
        }
    }
    command
}

fn run(project: &Path, args: &[&str]) -> CommandOutput {
    run_with_env(project, args, &[])
}

fn run_with_env(
    project: &Path,
    args: &[&str],
    environment: &[(&str, Option<&str>)],
) -> CommandOutput {
    let output = command_for(project, args, environment)
        .output()
        .unwrap_or_else(|error| panic!("run {args:?}: {error}"));
    CommandOutput {
        exit: output.status.code().unwrap_or(-1),
        stdout: String::from_utf8(output.stdout).expect("stdout UTF-8"),
        stderr: String::from_utf8(output.stderr).expect("stderr UTF-8"),
    }
}

fn run_with_stdin(project: &Path, args: &[&str], input: &str) -> CommandOutput {
    let mut child = command_for(project, args, &[])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap_or_else(|error| panic!("spawn {args:?}: {error}"));
    child
        .stdin
        .take()
        .expect("child stdin")
        .write_all(input.as_bytes())
        .expect("write hook stdin");
    let output = child.wait_with_output().expect("collect hook output");
    CommandOutput {
        exit: output.status.code().unwrap_or(-1),
        stdout: String::from_utf8(output.stdout).expect("stdout UTF-8"),
        stderr: String::from_utf8(output.stderr).expect("stderr UTF-8"),
    }
}

fn replace_general(value: &str, normalizers: &Normalizers) -> String {
    let mut replaced = value.to_owned();
    if let Some(root) = &normalizers.root {
        replaced = replaced.replace(root, "<ROOT>");
    }
    let mut values: Vec<_> = normalizers.values.iter().collect();
    values.sort_by_key(|(actual, _)| std::cmp::Reverse(actual.len()));
    for (actual, expected) in values {
        replaced = replaced.replace(actual, expected);
    }
    if let Some(date) = &normalizers.note_date {
        if replaced.contains("-evidence.txt") {
            replaced = replaced.replace(date, "<NOTE_DATE>");
        }
    }
    if normalizers.normalize_project {
        if let Some(project) = &normalizers.project {
            replaced = replaced.replace(project, "<PROJECT>");
        }
    }
    replaced
}

fn timestamp_token(key: Option<&str>, normalizers: &Normalizers) -> Option<&'static str> {
    if !normalizers.timestamps {
        return None;
    }
    match key {
        Some("createdAt") | Some("updatedAt") | Some("decidedAt") => Some("<CREATED_AT>"),
        Some("expiresAt") => Some("<EXPIRES_AT>"),
        Some("at") => Some("<EVENT_AT>"),
        _ => None,
    }
}

fn collect_timestamp_replacements(
    value: &Value,
    key: Option<&str>,
    normalizers: &Normalizers,
    replacements: &mut BTreeMap<String, String>,
) {
    match value {
        Value::Object(object) => {
            for (name, child) in object {
                collect_timestamp_replacements(child, Some(name), normalizers, replacements);
            }
        }
        Value::Array(values) => {
            for child in values {
                collect_timestamp_replacements(child, key, normalizers, replacements);
            }
        }
        Value::String(text) => {
            if let Some(token) = timestamp_token(key, normalizers) {
                replacements.insert(json_fragment(text), token.to_owned());
            }
        }
        _ => {}
    }
}

fn collect_note_replacements(
    value: &Value,
    normalizers: &Normalizers,
    replacements: &mut BTreeMap<String, String>,
) {
    match value {
        Value::Object(object) => {
            for child in object.values() {
                collect_note_replacements(child, normalizers, replacements);
            }
        }
        Value::Array(values) => {
            for child in values {
                collect_note_replacements(child, normalizers, replacements);
            }
        }
        Value::String(text) => {
            if let Some(date) = &normalizers.note_date {
                if text.contains("-evidence.txt") {
                    let normalized = text.replace(date, "<NOTE_DATE>");
                    replacements.insert(json_fragment(text), json_fragment(&normalized));
                }
            }
        }
        _ => {}
    }
}

fn json_fragment(value: &str) -> String {
    let encoded = serde_json::to_string(value).expect("serialize JSON string fragment");
    encoded[1..encoded.len() - 1].to_owned()
}

fn normalize_json_text(actual: &str, normalizers: &Normalizers) -> String {
    let value: Value = serde_json::from_str(actual).expect("candidate JSON output");
    let mut replacements = BTreeMap::new();
    collect_timestamp_replacements(&value, None, normalizers, &mut replacements);
    collect_note_replacements(&value, normalizers, &mut replacements);
    if let Some(root) = &normalizers.root {
        replacements.insert(json_fragment(root), "<ROOT>".to_owned());
    }
    for (actual, expected) in &normalizers.values {
        replacements.insert(json_fragment(actual), expected.clone());
    }
    if normalizers.normalize_project {
        if let Some(project) = &normalizers.project {
            replacements.insert(json_fragment(project), "<PROJECT>".to_owned());
        }
    }
    let mut ordered: Vec<_> = replacements.into_iter().collect();
    ordered.sort_by_key(|(actual, _)| std::cmp::Reverse(actual.len()));
    ordered
        .into_iter()
        .fold(actual.to_owned(), |output, (actual, expected)| {
            output.replace(&actual, &expected)
        })
}

fn normalize_jsonl(actual: &str, normalizers: &Normalizers) -> String {
    assert!(
        !actual.contains('\r'),
        "reference JSONL uses LF; candidate used CRLF"
    );
    actual
        .split_inclusive('\n')
        .map(|segment| {
            if let Some(line) = segment.strip_suffix('\n') {
                if line.is_empty() {
                    "\n".to_owned()
                } else {
                    format!("{}\n", normalize_json_text(line, normalizers))
                }
            } else if segment.is_empty() {
                String::new()
            } else {
                normalize_json_text(segment, normalizers)
            }
        })
        .collect()
}

fn assert_json(output: &CommandOutput, expected: &str, exit: i32, normalizers: &Normalizers) {
    assert_eq!(
        output.exit, exit,
        "exit for JSON fixture {expected}; stderr={}",
        output.stderr
    );
    assert!(
        output.stderr.is_empty(),
        "stderr for JSON fixture {expected}: {}",
        output.stderr
    );
    assert_eq!(
        normalize_json_text(&output.stdout, normalizers),
        json_fixture(expected),
        "JSON stdout for fixture {expected}"
    );
}

fn assert_text(output: &CommandOutput, expected: &str, exit: i32) {
    assert_eq!(
        output.exit, exit,
        "exit for text fixture {expected}; stderr={}",
        output.stderr
    );
    assert!(
        output.stderr.is_empty(),
        "stderr for text fixture {expected}: {}",
        output.stderr
    );
    assert_eq!(output.stdout, fixture(expected), "stdout for {expected}");
}

fn json_value(output: &CommandOutput) -> Value {
    serde_json::from_str(&output.stdout)
        .unwrap_or_else(|error| panic!("parse command JSON {}: {error}", output.stdout))
}

fn required_string(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("JSON string {key} missing from {value}"))
        .to_owned()
}

fn assert_normalized_jsonl(path: &Path, expected: &str, normalizers: &Normalizers) {
    let actual = fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("read JSONL {}: {error}", path.display()));
    assert_eq!(
        normalize_jsonl(&actual, normalizers),
        fixture(expected),
        "JSONL snapshot {}",
        path.display()
    );
}

fn assert_file_bytes(path: &Path, expected: &str) {
    assert_eq!(
        fs::read(path).unwrap_or_else(|error| panic!("read {}: {error}", path.display())),
        fixture_bytes(expected),
        "file snapshot {}",
        path.display()
    );
}

fn has_atomic_artifact(path: &Path) -> bool {
    fs::read_dir(path)
        .expect("read store for artifacts")
        .filter_map(Result::ok)
        .any(|entry| {
            let name = entry.file_name().to_string_lossy().to_string();
            name.ends_with(".lock") || name.starts_with(".tmp.")
        })
}

#[test]
fn windows_mutation_fixtures_match_the_reference_cli() {
    let fixture = FixtureProject::from_input("mutation", "current");
    fixture.write_evidence();
    let mut normalizers = fixture.normalizers();
    normalizers.timestamps = true;

    let moved = run(
        &fixture.project,
        &["mv", "r-e724", "resilient-offline", "--json"],
    );
    assert_json(
        &moved,
        "behavior/expected/mutation/mv.json",
        0,
        &normalizers,
    );

    let linked = run(
        &fixture.project,
        &["link", "r-7528", "r-e724", "serves", "--json"],
    );
    let link_id = required_string(&json_value(&linked), "id");
    normalizers.map(&link_id, "<LINK_EDGE_ID>");
    assert_json(
        &linked,
        "behavior/expected/mutation/link.json",
        0,
        &normalizers,
    );

    let unlinked = run(&fixture.project, &["unlink", &link_id, "--json"]);
    assert_json(
        &unlinked,
        "behavior/expected/mutation/unlink.json",
        0,
        &normalizers,
    );

    let tier = run(&fixture.project, &["tier", "r-7528", "1", "--json"]);
    assert_json(
        &tier,
        "behavior/expected/mutation/tier.json",
        0,
        &normalizers,
    );

    let asked = run(
        &fixture.project,
        &[
            "ask",
            "r-7528",
            "Who owns conflict resolution?",
            "--as",
            "agent:fixture",
            "--json",
        ],
    );
    let question_id = required_string(&json_value(&asked), "id");
    normalizers.map(&question_id, "<QUESTION_ID>");
    assert_json(
        &asked,
        "behavior/expected/mutation/ask.json",
        0,
        &normalizers,
    );

    let noted = run(
        &fixture.project,
        &[
            "note",
            "r-7528",
            "--file",
            "evidence.txt",
            "--as",
            "agent:fixture",
            "--json",
        ],
    );
    let note = json_value(&noted);
    let note_name = note["note"]["name"].as_str().expect("note name").to_owned();
    normalizers.note_date = note_name.get(..10).map(ToOwned::to_owned);
    assert_json(
        &noted,
        "behavior/expected/mutation/note.json",
        0,
        &normalizers,
    );

    let proposed = run(
        &fixture.project,
        &[
            "propose",
            "edge",
            "r-bb76",
            "r-e724",
            "tension",
            "--reason",
            "The interface and offline mode trade off.",
            "--cite",
            "r-bb76:The interface remains comprehensible.",
            "--cite",
            "r-e724:Offline mode depends on durable sync.",
            "--as",
            "agent:fixture",
            "--json",
        ],
    );
    let proposal_id = required_string(&json_value(&proposed), "id");
    normalizers.map(&proposal_id, "<PROPOSAL_ID>");
    assert_json(
        &proposed,
        "behavior/expected/mutation/propose.json",
        0,
        &normalizers,
    );

    let accepted = run(&fixture.project, &["accept", &proposal_id, "--json"]);
    let accepted_edge = required_string(&json_value(&accepted)["edge"], "id");
    normalizers.map(&accepted_edge, "<ACCEPTED_EDGE_ID>");
    assert_json(
        &accepted,
        "behavior/expected/mutation/accept.json",
        0,
        &normalizers,
    );

    let sprouted = run(
        &fixture.project,
        &[
            "sprout",
            "Add a reconciliation model.",
            "--as",
            "agent:fixture",
            "--json",
        ],
    );
    let sprout_id = required_string(&json_value(&sprouted), "id");
    let sprout_hex = sprout_id
        .strip_prefix("s-")
        .expect("sprout id prefix")
        .to_owned();
    normalizers.map(&sprout_id, "<SPROUT_ID>");
    normalizers.map(&sprout_hex, "<SPROUT_HEX>");
    assert_json(
        &sprouted,
        "behavior/expected/mutation/sprout.json",
        0,
        &normalizers,
    );

    let rejected = run(
        &fixture.project,
        &[
            "reject",
            &sprout_id,
            "--reason",
            "Already covered by the tension.",
            "--json",
        ],
    );
    assert_json(
        &rejected,
        "behavior/expected/mutation/reject.json",
        0,
        &normalizers,
    );

    let duplicate_sprout = run(
        &fixture.project,
        &[
            "sprout",
            "Add a reconciliation model.",
            "--as",
            "agent:fixture",
            "--json",
        ],
    );
    assert_json(
        &duplicate_sprout,
        "behavior/expected/errors/rejected-sprout.json",
        6,
        &normalizers,
    );

    let scanned = run(&fixture.project, &["scan", "--all", "--json"]);
    let mention_id = json_value(&scanned)["ideas"]
        .as_array()
        .and_then(|ideas| ideas.get(1))
        .and_then(|idea| idea["filed"].as_array())
        .and_then(|filed| filed.first())
        .and_then(|proposal| proposal["id"].as_str())
        .expect("mention proposal id")
        .to_owned();
    normalizers.map(&mention_id, "<MENTION_PROPOSAL_ID>");
    assert_json(
        &scanned,
        "behavior/expected/mutation/scan.json",
        0,
        &normalizers,
    );

    let compost_interface = run(
        &fixture.project,
        &["status", "r-bb76", "composted", "--json"],
    );
    assert_json(
        &compost_interface,
        "behavior/expected/mutation/status.json",
        0,
        &normalizers,
    );
    let compost_offline = run(
        &fixture.project,
        &[
            "compost",
            "r-e724",
            "--reason",
            "Replaced by the focused tension.",
            "--json",
        ],
    );
    assert_json(
        &compost_offline,
        "behavior/expected/mutation/compost.json",
        0,
        &normalizers,
    );

    let planted_commit = run(&fixture.project, &["commit", "r-7528", "--json"]);
    assert_json(
        &planted_commit,
        "behavior/expected/errors/commit-planted.json",
        5,
        &normalizers,
    );

    let store = fixture.project.join(".luuna/roots");
    assert_normalized_jsonl(
        &store.join("graph.jsonl"),
        "behavior/expected/mutation/graph.jsonl",
        &normalizers,
    );
    assert_normalized_jsonl(
        &store.join("events.jsonl"),
        "behavior/expected/mutation/events.jsonl",
        &normalizers,
    );
    assert_normalized_jsonl(
        &store.join("proposals.jsonl"),
        "behavior/expected/mutation/proposals.jsonl",
        &normalizers,
    );
    assert_normalized_jsonl(
        &store.join("questions.jsonl"),
        "behavior/expected/mutation/questions.jsonl",
        &normalizers,
    );
    assert_file_bytes(
        &store
            .join("agent/sprouts")
            .join(format!("{sprout_hex}-add-reconciliation-model"))
            .join("sprout.md"),
        "behavior/expected/mutation/sprout.md",
    );
    assert!(
        !store.join("human/e724-offline-mode").exists(),
        "mv removes the old human-owned directory"
    );
    assert!(store.join("human/e724-resilient-offline").is_dir());
    assert!(
        !has_atomic_artifact(&store),
        "atomic mutations must not leave lock/temp files"
    );
}

#[test]
fn windows_read_and_heading_fixtures_match_the_reference_cli() {
    let read = FixtureProject::from_input("read", "read");
    read.write_read_note();
    let mut normalizers = read.normalizers();
    // The captured read-state note has a deliberately fixed historical date;
    // only its filename date is abstracted in the reference output.
    normalizers.note_date = Some("2026-10-23".to_owned());

    assert_json(
        &run(&read.project, &["context", "r-7528", "--no-repo", "--json"]),
        "behavior/expected/read/context.json",
        0,
        &normalizers,
    );
    assert_json(
        &run(&read.project, &["prime", "--scope", "r-7528", "--json"]),
        "behavior/expected/read/prime-scope.json",
        0,
        &normalizers,
    );
    assert_json(
        &run(&read.project, &["drift", "--json"]),
        "behavior/expected/read/drift.json",
        0,
        &normalizers,
    );
    assert_json(
        &run(&read.project, &["show", "r-7528", "--json"]),
        "behavior/expected/read/show.json",
        0,
        &normalizers,
    );
    assert_json(
        &run(&read.project, &["blame", "r-7528", "--json"]),
        "behavior/expected/read/blame.json",
        0,
        &normalizers,
    );
    assert_json(
        &run(&read.project, &["list", "--all", "--json"]),
        "behavior/expected/read/list.json",
        0,
        &normalizers,
    );
    assert_json(
        &run(&read.project, &["queue", "--json"]),
        "behavior/expected/read/queue.json",
        0,
        &normalizers,
    );
    assert_json(
        &run(&read.project, &["log", "--limit", "5", "--json"]),
        "behavior/expected/read/log.json",
        0,
        &normalizers,
    );

    let markdown = run(&read.project, &["view", "--out", "report.md", "--json"]);
    assert_json(
        &markdown,
        "behavior/expected/read/view-markdown.json",
        0,
        &normalizers,
    );
    assert_file_bytes(
        &read.project.join("report.md"),
        "behavior/expected/read/report.md",
    );

    let html = run(
        &read.project,
        &["view", "--html", "--out", "report.html", "--json"],
    );
    assert_json(
        &html,
        "behavior/expected/read/view-html.json",
        0,
        &normalizers,
    );
    assert_file_bytes(
        &read.project.join("report.html"),
        "behavior/expected/read/report.html",
    );

    assert_json(
        &run(&read.project, &["view", "--from", "r-7528", "--json"]),
        "behavior/expected/read/view-from.json",
        0,
        &normalizers,
    );
    assert_json(
        &run(&read.project, &["view", "--sprouts", "--json"]),
        "behavior/expected/read/view-sprouts.json",
        0,
        &normalizers,
    );
    assert_json(
        &run(&read.project, &["verify", "--json"]),
        "behavior/expected/read/verify.json",
        0,
        &normalizers,
    );

    let outside = TempDir::new("prime-hook-outside");
    assert_json(
        &run(&outside.root, &["prime", "--hook", "--json"]),
        "behavior/expected/read/prime-hook-outside.json",
        0,
        &Normalizers::default(),
    );

    let heading = FixtureProject::from_input("heading", "heading");
    heading.write_evidence();
    let mut heading_normalizers = heading.normalizers();
    heading_normalizers.timestamps = true;
    let result = run(
        &heading.project,
        &[
            "heading",
            "Clarify r-7528 ownership.",
            "--cite",
            "r-7528:Durable sync",
            "--next",
            "r-e724",
            "--flow",
            "fl-heading",
            "--as",
            "agent:fixture",
            "--json",
        ],
    );
    let heading_id = required_string(&json_value(&result), "id");
    heading_normalizers.map(heading_id, "<HEADING_ID>");
    assert_json(
        &result,
        "behavior/expected/heading/result.json",
        0,
        &heading_normalizers,
    );
    assert_normalized_jsonl(
        &heading.project.join(".luuna/roots/headings.jsonl"),
        "behavior/expected/heading/headings.jsonl",
        &heading_normalizers,
    );
}

#[test]
fn windows_setup_guard_environment_and_legacy_fixtures_match_reference() {
    let setup = FixtureProject::from_input("setup", "current");
    let setup_normalizers = setup.normalizers();
    let settings = setup.project.join(".claude/settings.json");

    let dry_run = run(&setup.project, &["setup", "claude", "--dry-run", "--json"]);
    assert_json(
        &dry_run,
        "behavior/expected/setup/dry-run.json",
        0,
        &setup_normalizers,
    );
    assert!(
        !settings.exists(),
        "setup --dry-run must not create settings.json"
    );

    let installed = run(&setup.project, &["setup", "claude", "--json"]);
    assert_json(
        &installed,
        "behavior/expected/setup/install.json",
        0,
        &setup_normalizers,
    );
    assert_file_bytes(&settings, "behavior/expected/setup/settings-installed.json");

    let idempotent = run(&setup.project, &["setup", "claude", "--json"]);
    assert_json(
        &idempotent,
        "behavior/expected/setup/idempotent.json",
        0,
        &setup_normalizers,
    );
    assert_file_bytes(&settings, "behavior/expected/setup/settings-installed.json");

    let removed = run(&setup.project, &["setup", "claude", "--remove", "--json"]);
    assert_json(
        &removed,
        "behavior/expected/setup/remove.json",
        0,
        &setup_normalizers,
    );
    assert_file_bytes(&settings, "behavior/expected/setup/settings-removed.json");

    let remove_idempotent = run(&setup.project, &["setup", "claude", "--remove", "--json"]);
    assert_json(
        &remove_idempotent,
        "behavior/expected/setup/remove-idempotent.json",
        0,
        &setup_normalizers,
    );

    let agent_remove = run_with_env(
        &setup.project,
        &["setup", "claude", "--remove", "--json"],
        &[("ROOTS_AGENT", Some("agent:fixture"))],
    );
    assert_json(
        &agent_remove,
        "behavior/expected/errors/setup-remove-agent.json",
        4,
        &setup_normalizers,
    );

    let guard = FixtureProject::from_input("guard", "current");
    fs::create_dir_all(guard.project.join(".roots/human")).expect("create legacy roots tree");
    let hook = r#"{"tool_name":"Write","tool_input":{"file_path":".roots/human/legacy.md"}}"#;
    let guarded_json = run_with_stdin(&guard.project, &["guard", "--json"], hook);
    assert_json(
        &guarded_json,
        "behavior/expected/guard/legacy-human.json",
        0,
        &guard.normalizers(),
    );
    let guarded_plain = run_with_stdin(&guard.project, &["guard"], hook);
    assert_text(
        &guarded_plain,
        "behavior/expected/guard/legacy-deny-hook.txt",
        0,
    );
    // The current spelling is guarded too; use the same protocol and retain
    // the exact legacy fixture above as the public compatibility oracle.
    let current_hook = r#"{"tool_name":"Write","tool_input":{"file_path":".luuna/roots/human/7528-durable-sync/idea.md"}}"#;
    let current_guard = run_with_stdin(&guard.project, &["guard", "--json"], current_hook);
    assert_eq!(current_guard.exit, 0);
    assert!(current_guard.stderr.is_empty());
    let current_guard_json = json_value(&current_guard);
    assert_eq!(current_guard_json["decision"], "deny");
    assert_eq!(
        current_guard_json["target"],
        ".luuna/roots/human/7528-durable-sync/idea.md"
    );

    let root_agent = FixtureProject::from_input("root-agent", "current");
    let root_agent_error = run_with_env(
        &root_agent.project,
        &["link", "r-7528", "r-e724", "serves", "--json"],
        &[("ROOTS_AGENT", Some("agent:fixture"))],
    );
    assert_json(
        &root_agent_error,
        "behavior/expected/errors/root-agent-human-refusal.json",
        4,
        &root_agent.normalizers(),
    );

    let agent_precedence = FixtureProject::from_input("agent-precedence", "current");
    let mut agent_normalizers = agent_precedence.normalizers();
    agent_normalizers.timestamps = true;
    let agent = run_with_env(
        &agent_precedence.project,
        &["ask", "r-7528", "Which agent wins?", "--json"],
        &[
            ("ROOTS_AGENT", Some("agent:legacy")),
            ("LUUNA_ROOTS_AGENT", Some("agent:new")),
        ],
    );
    agent_normalizers.map(required_string(&json_value(&agent), "id"), "<QUESTION_ID>");
    assert_json(
        &agent,
        "behavior/expected/env/agent-precedence.json",
        0,
        &agent_normalizers,
    );

    let agent_blank = FixtureProject::from_input("agent-blank", "current");
    let agent_blank_error = run_with_env(
        &agent_blank.project,
        &["ask", "r-7528", "Which agent wins?", "--json"],
        &[
            ("ROOTS_AGENT", Some("agent:legacy")),
            ("LUUNA_ROOTS_AGENT", Some("")),
        ],
    );
    assert_json(
        &agent_blank_error,
        "behavior/expected/errors/agent-blank-mask.json",
        4,
        &agent_blank.normalizers(),
    );

    let user_precedence = FixtureProject::from_input("user-precedence", "current");
    let mut user_normalizers = user_precedence.normalizers();
    user_normalizers.timestamps = true;
    let user = run_with_env(
        &user_precedence.project,
        &["link", "r-7528", "r-e724", "serves", "--json"],
        &[
            ("ROOTS_USER", Some("legacy-user")),
            ("LUUNA_ROOTS_USER", Some("New User")),
        ],
    );
    user_normalizers.map(required_string(&json_value(&user), "id"), "<LINK_EDGE_ID>");
    assert_json(
        &user,
        "behavior/expected/env/user-precedence.json",
        0,
        &user_normalizers,
    );

    let user_blank = FixtureProject::from_input("user-blank", "current");
    let user_blank_error = run_with_env(
        &user_blank.project,
        &["link", "r-7528", "r-e724", "serves", "--json"],
        &[
            ("ROOTS_USER", Some("legacy-user")),
            ("LUUNA_ROOTS_USER", Some("")),
        ],
    );
    assert_json(
        &user_blank_error,
        "behavior/expected/errors/user-blank-mask.json",
        4,
        &user_blank.normalizers(),
    );

    let luuna_agent = FixtureProject::from_input("luuna-agent", "current");
    let mut luuna_agent_normalizers = luuna_agent.normalizers();
    luuna_agent_normalizers.timestamps = true;
    let luuna_agent_link = run_with_env(
        &luuna_agent.project,
        &["link", "r-7528", "r-e724", "serves", "--json"],
        &[("LUUNA_ROOTS_AGENT", Some("agent:fixture"))],
    );
    luuna_agent_normalizers.map(
        required_string(&json_value(&luuna_agent_link), "id"),
        "<LINK_EDGE_ID>",
    );
    assert_json(
        &luuna_agent_link,
        "behavior/expected/env/luuna-agent-does-not-block-human.json",
        0,
        &luuna_agent_normalizers,
    );

    let legacy = TempDir::new("legacy-only");
    let legacy_project = legacy.root.join("legacy-only");
    fs::create_dir_all(legacy_project.join(".roots")).expect("create legacy-only store");
    let legacy_error = run(&legacy_project, &["list", "--json"]);
    let legacy_normalizers = Normalizers {
        root: Some(legacy.root.display().to_string()),
        ..Normalizers::default()
    };
    assert_json(
        &legacy_error,
        "behavior/expected/errors/legacy-not-initialized.json",
        3,
        &legacy_normalizers,
    );
}

#[test]
fn windows_init_preserves_reference_store_bytes() {
    let temp = TempDir::new("init");
    let mut normalizers = Normalizers {
        root: Some(temp.root.display().to_string()),
        project: temp
            .root
            .file_name()
            .and_then(|name| name.to_str())
            .map(ToOwned::to_owned),
        normalize_project: true,
        timestamps: true,
        ..Normalizers::default()
    };
    let initialized = run(&temp.root, &["init", "--json"]);
    assert_json(&initialized, "init/result.json", 0, &normalizers);

    let roots = temp.root.join(".luuna/roots");
    let config = fs::read_to_string(roots.join("config.yaml")).expect("read initialized config");
    assert_eq!(
        replace_general(&config, &normalizers),
        fixture("init/config.yaml"),
        "config project name"
    );
    // The byte fixture above deliberately uses <PROJECT>; compare all static
    // lines byte-for-byte after that single documented project-name token.
    assert_file_bytes(&temp.root.join(".gitattributes"), "init/gitattributes");
    assert_file_bytes(&roots.join(".gitignore"), "init/roots.gitignore");
    assert_normalized_jsonl(
        &roots.join("events.jsonl"),
        "init/events.jsonl",
        &normalizers,
    );
    for empty in ["graph.jsonl", "proposals.jsonl", "questions.jsonl"] {
        assert_eq!(
            fs::read(roots.join(empty)).expect("read initialized empty store"),
            Vec::<u8>::new(),
            "{empty} starts empty"
        );
    }
    for directory in ["human", "agent/sprouts", "agent/notes"] {
        assert!(roots.join(directory).is_dir(), "init creates {directory}");
    }
    // Make it explicit that the only config replacement is <PROJECT>, not a
    // broad whitespace/line-ending normalization.
    normalizers.normalize_project = false;
    assert!(
        !replace_general(&config, &normalizers).contains("<PROJECT>"),
        "project token is opt-in"
    );
}

#[test]
fn windows_malformed_jsonl_last_wins_fixture_matches_reference() {
    let project_fixture = FixtureProject::from_input("malformed", "current");
    project_fixture.write_evidence();
    let store = project_fixture.project.join(".luuna/roots");
    fs::write(
        store.join("graph.jsonl"),
        fixture("behavior/input/malformed/graph.jsonl"),
    )
    .expect("install malformed JSONL overlay");
    assert!(
        fs::read_to_string(store.join("graph.jsonl"))
            .expect("read malformed input")
            .contains("intentionally malformed"),
        "the fixture begins with captured malformed JSONL"
    );

    let output = run(
        &project_fixture.project,
        &["mv", "r-e724", "normalized-offline", "--json"],
    );
    let mut normalizers = project_fixture.normalizers();
    normalizers.timestamps = true;
    assert_json(
        &output,
        "behavior/expected/malformed/mv.json",
        0,
        &normalizers,
    );
    assert_normalized_jsonl(
        &store.join("graph.jsonl"),
        "behavior/expected/malformed/graph.jsonl",
        &normalizers,
    );
    assert_normalized_jsonl(
        &store.join("events.jsonl"),
        "behavior/expected/malformed/events.jsonl",
        &normalizers,
    );
    let normalized = fs::read_to_string(store.join("graph.jsonl")).expect("read compacted graph");
    assert!(
        !normalized.contains("intentionally malformed"),
        "successful mutation compacts tolerated malformed records away"
    );
    assert_eq!(
        normalized.lines().count(),
        3,
        "last-wins duplicate is compacted while first-seen ordering remains"
    );
    assert!(store.join("human/e724-normalized-offline").is_dir());
    assert!(!store.join("human/e724-offline-mode").exists());
    assert!(
        !has_atomic_artifact(&store),
        "the compacting mutation must not leave lock/temp files"
    );
}
