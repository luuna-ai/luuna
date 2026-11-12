//! Format compatibility with the Go reference tool, in both directions: what the Go tool
//! writes the crate reads and rewrites byte for byte, and what the crate writes the Go tool
//! validates, shows, edits and searches exactly as it does its own files.
//!
//! Every test needs the reference clone and Go, so all are ignored by default. Run:
//! `LUUNA_ATLAS_REFERENCE=<clone> LUUNA_ATLAS_GO=<go> cargo test -p luuna-atlas --locked
//! --test format_compat -- --ignored`.

mod support;

use luuna_atlas::json::{self, Object, Value};
use luuna_atlas::registry::Registry;
use luuna_atlas::store::{self, Record};
use luuna_atlas::{commands, config, AtlasStore};

use support::project::{assert_same_output, copy_store, Project, Sandbox, Twin};

/// The archive banner, exactly as the Go tool writes it.
const BANNER: &str = "# ARCHIVED — not for active use. Run `luuna-atlas restore <id>` to revive.";

/// The Go tool's CLI calls that write one record of each built-in type.
const BUILTIN_RECORDS: &[&[&str]] = &[
    &[
        "record",
        "d",
        "--type",
        "convention",
        "--content",
        "Use tabs in Makefiles",
        "--classification",
        "foundational",
    ],
    &[
        "record",
        "d",
        "--type",
        "pattern",
        "--name",
        "Scoped locking",
        "--description",
        "Use a lock per domain file",
        "--files",
        "src/a.rs",
    ],
    &[
        "record",
        "d",
        "--type",
        "failure",
        "--description",
        "Shared temp filename",
        "--resolution",
        "Use a unique temp directory",
        "--classification",
        "tactical",
    ],
    &[
        "record",
        "d",
        "--type",
        "decision",
        "--title",
        "Atomic replacement",
        "--rationale",
        "Readers must see complete files",
    ],
    &[
        "record",
        "d",
        "--type",
        "reference",
        "--name",
        "Schema docs",
        "--description",
        "See the schema source",
        "--files",
        "src/schema.rs,src/b.rs",
    ],
    &[
        "record",
        "d",
        "--type",
        "guide",
        "--name",
        "Review records",
        "--description",
        "Query then validate",
        "--classification",
        "observational",
    ],
];

/// A description that exercises non-ASCII text, HTML characters, quotes, backslashes,
/// control characters and U+2028.
const TRICKY_TEXT: &str =
    "Unicode é 中文 🚀 <a href=\"x\">&amp;</a> \"q\" back\\slash\ttab\nnewline \u{2028} sep";

/// Records with fixed timestamps, valid for the Go tool's schema, covering every built-in
/// type and every optional base field. Fed to the Go tool on stdin and parsed by the crate.
const RECORDS_JSON: &str = r#"[
{"type":"convention","classification":"foundational","recorded_at":"2025-01-02T03:04:05.000Z","content":"Prefer tabs in Makefiles because make requires them","dir_anchors":["build"]},
{"type":"pattern","classification":"tactical","recorded_at":"2025-01-02T03:04:06.000Z","name":"Scoped locking","description":"Use a lock per domain file","files":["src/store.rs","src/lock.rs"],"tags":["concurrency"]},
{"type":"failure","classification":"tactical","recorded_at":"2025-01-02T03:04:07.000Z","evidence":{"commit":"abc123","missions":"lms-1","gh":"owner/repo#12"},"tags":["a","b"],"relates_to":["mx-abcdef"],"outcomes":[{"status":"success","duration":1500,"test_results":"3 passed","agent":"tester-agent"}],"description":"Unicode é 中文 🚀 <a href=\"x\">&amp;</a> \"q\" back\\slash\ttab\nnewline   sep","resolution":"Use a unique temp directory"},
{"type":"decision","classification":"observational","recorded_at":"2025-01-02T03:04:08.000Z","title":"Atomic replacement","rationale":"Readers must see complete files","date":"2025-01-01"},
{"type":"reference","classification":"tactical","recorded_at":"2025-01-02T03:04:09.000Z","name":"Schema docs","description":"See the schema source","files":["schema.json"]},
{"type":"guide","classification":"foundational","recorded_at":"2025-01-02T03:04:10.000Z","name":"Review records","description":"Query then validate","supersedes":["mx-123456"],"status":"active","owner":"team-a"}
]"#;

/// A hand-written line the Go tool accepts: spaces, escapes it re-encodes differently,
/// and numbers it reformats.
const ESCAPED_LINE: &str = r#"{ "type" : "convention", "classification":"tactical","recorded_at":"2025-01-02T03:04:05.000Z", "content":"esc é \/ <b> &     🚀 \u0001 \u007f \t \"", "tags":[], "n": 1.50, "big": 12345678901234567890, "small": 0.000001, "neg": -0, "e": 1e21, "f": 1e-7, "x": 100000000000000000000, "id":"mx-bbbbbb"}"#;

/// The content of [`ESCAPED_LINE`] once decoded.
const ESCAPED_CONTENT: &str = "esc é / <b> & \u{2028} \u{2029} 🚀 \u{1} \u{7f} \t \"";

/// A line in the legacy shape: a singular `outcome` object.
const LEGACY_OUTCOME_LINE: &str = r#"{"type":"decision","classification":"foundational","recorded_at":"2025-01-02T03:04:05.000Z","title":"Legacy","rationale":"r","outcome":{"agent":"x","status":"failure","duration":5,"notes":"dropped"},"id":"mx-111111"}"#;

/// A custom type with a field alias, set through the Go tool.
const HYPOTHESIS_TYPE: &str = r#"{required: [statement, prediction], optional: [evidence_files], dedup_key: statement, aliases: {statement: [claim]}, summary: "{statement}", section_title: "Hypotheses: open"}"#;

/// A custom-type record that still uses the legacy field name `claim`.
const LEGACY_CLAIM_LINE: &str = r#"{"type":"hypothesis","classification":"tactical","recorded_at":"2025-01-02T03:04:05.000Z","claim":"Legacy claim","prediction":"P0","id":"mx-222222"}"#;

// ---------------------------------------------------------------------------------------
// helpers

/// Unwraps a crate result, showing the crate's error (for example "not implemented").
#[track_caller]
fn ok<T>(result: luuna_atlas::Result<T>, what: &str) -> T {
    result.unwrap_or_else(|e| panic!("{what} failed: {e}"))
}

/// Opens the crate's view of `project`: the store and its registry.
#[track_caller]
fn open(project: &Project) -> (AtlasStore, Registry) {
    let store = ok(AtlasStore::open(&project.dir), "AtlasStore::open");
    let config = ok(
        config::read_config(&store.atlas_dir()),
        "config::read_config",
    );
    let registry = ok(
        Registry::from_config(Some(&config)),
        "Registry::from_config",
    );
    (store, registry)
}

#[track_caller]
fn parse(text: &str) -> Value {
    ok(json::parse(text), "json::parse")
}

#[track_caller]
fn parse_records(text: &str) -> Vec<Record> {
    parse(text)
        .as_array()
        .expect("an array of records")
        .iter()
        .map(|v| v.as_object().expect("a record object").clone())
        .collect()
}

#[track_caller]
fn text<'a>(record: &'a Record, key: &str) -> &'a str {
    record
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("{key} is not a string in {record:?}"))
}

#[track_caller]
fn texts<'a>(record: &'a Record, key: &str) -> Vec<&'a str> {
    record
        .get(key)
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("{key} is not an array in {record:?}"))
        .iter()
        .map(|v| v.as_str().expect("a string element"))
        .collect()
}

#[track_caller]
fn number(record: &Record, key: &str) -> f64 {
    record
        .get(key)
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("{key} is not a number in {record:?}"))
}

#[track_caller]
fn object<'a>(record: &'a Record, key: &str) -> &'a Object {
    record
        .get(key)
        .and_then(Value::as_object)
        .unwrap_or_else(|| panic!("{key} is not an object in {record:?}"))
}

fn keys(record: &Object) -> Vec<&str> {
    record.keys().collect()
}

fn is_go_id(id: &str) -> bool {
    id.len() == 9
        && id.starts_with("mx-")
        && id[3..]
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn is_go_timestamp(value: &str) -> bool {
    let shape = b"dddd-dd-ddTdd:dd:dd.dddZ";
    value.len() == shape.len()
        && value.bytes().zip(shape.iter()).all(|(b, s)| {
            if *s == b'd' {
                b.is_ascii_digit()
            } else {
                b == *s
            }
        })
}

/// The non-empty, non-comment lines of a JSONL file.
fn record_lines(contents: &str) -> Vec<&str> {
    contents
        .split('\n')
        .filter(|l| !l.trim().is_empty() && !l.trim_start().starts_with('#'))
        .collect()
}

/// The record the Go tool's `show <id>` prints.
#[track_caller]
fn go_show(project: &Project, id: &str) -> Value {
    let stdout = project.go_ok(&["show", id]);
    let shown = parse(&stdout);
    let records = shown
        .as_object()
        .and_then(|o| o.get("records"))
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("show printed no records: {stdout}"));
    assert_eq!(records.len(), 1, "{stdout}");
    records[0]
        .as_object()
        .and_then(|o| o.get("record"))
        .cloned()
        .unwrap_or_else(|| panic!("show printed no record: {stdout}"))
}

/// The Go tool's `show <id>` envelope for one record, as the Go tool prints it.
#[track_caller]
fn show_envelope(domain: &str, record: &Record) -> String {
    let mut item = Object::new();
    item.insert("domain", domain);
    item.insert("record", record.clone());
    let mut envelope = Object::new();
    envelope.insert("success", true);
    envelope.insert("command", "show");
    envelope.insert("records", vec![Value::Object(item)]);
    format!(
        "{}\n",
        ok(
            json::to_indented_string(&Value::Object(envelope)),
            "json::to_indented_string"
        )
    )
}

/// A Go project with `init` and domain `d`.
#[track_caller]
fn go_store(sandbox: &Sandbox, name: &str) -> Project {
    let project = sandbox.project(name, true);
    project.go_ok(&["init"]);
    project.go_ok(&["add", "d"]);
    project
}

#[track_caller]
fn assert_no_leftovers(project: &Project, label: &str) {
    let leftovers = project.leftovers();
    assert!(
        leftovers.is_empty(),
        "{label}: leftover files {leftovers:?}"
    );
}

// ---------------------------------------------------------------------------------------
// Go writes, the crate reads

#[test]
#[ignore = "needs the luuna-atlas reference clone and Go; run the -- --ignored check"]
fn format_go_writes_rust_reads_all_builtin_types() {
    let sandbox = Sandbox::new();
    let project = go_store(&sandbox, "go");
    for args in BUILTIN_RECORDS {
        project.go_ok(args);
    }
    let file = project.read(&project.expertise("d"));
    assert_eq!(record_lines(&file).len(), 6);

    let (store, registry) = open(&project);
    let records = ok(
        store::read_records(
            &ok(store.expertise_path("d"), "expertise_path"),
            &registry,
            false,
        ),
        "store::read_records",
    );
    let types: Vec<&str> = records.iter().map(|r| text(r, "type")).collect();
    assert_eq!(
        types,
        [
            "convention",
            "pattern",
            "failure",
            "decision",
            "reference",
            "guide"
        ]
    );
    assert_eq!(text(&records[0], "content"), "Use tabs in Makefiles");
    assert_eq!(text(&records[0], "classification"), "foundational");
    assert_eq!(text(&records[1], "name"), "Scoped locking");
    assert_eq!(
        text(&records[1], "description"),
        "Use a lock per domain file"
    );
    assert_eq!(texts(&records[1], "files"), ["src/a.rs"]);
    assert_eq!(text(&records[1], "classification"), "tactical");
    assert_eq!(text(&records[2], "description"), "Shared temp filename");
    assert_eq!(
        text(&records[2], "resolution"),
        "Use a unique temp directory"
    );
    assert_eq!(text(&records[3], "title"), "Atomic replacement");
    assert_eq!(
        text(&records[3], "rationale"),
        "Readers must see complete files"
    );
    assert_eq!(texts(&records[4], "files"), ["src/schema.rs", "src/b.rs"]);
    assert_eq!(text(&records[5], "classification"), "observational");
    // Keys come back in the order the Go tool wrote them.
    assert_eq!(
        keys(&records[1]),
        [
            "type",
            "classification",
            "recorded_at",
            "name",
            "description",
            "files",
            "id"
        ]
    );
    for (record, line) in records.iter().zip(record_lines(&file)) {
        let id = text(record, "id");
        assert!(is_go_id(id), "{id}");
        assert!(line.contains(&format!("\"id\":\"{id}\"")), "{line}");
        assert!(is_go_timestamp(text(record, "recorded_at")));
    }
}

#[test]
#[ignore = "needs the luuna-atlas reference clone and Go; run the -- --ignored check"]
fn format_go_writes_rust_rewrites_byte_identical() {
    let sandbox = Sandbox::new();
    let project = go_store(&sandbox, "go");
    for args in BUILTIN_RECORDS {
        project.go_ok(args);
    }
    project.go_ok(&[
        "record",
        "d",
        "--type",
        "failure",
        "--description",
        TRICKY_TEXT,
        "--resolution",
        "ok",
        "--outcome-status",
        "partial",
        "--outcome-duration",
        "12.5",
        "--outcome-agent",
        "a1",
        "--evidence-commit",
        "abc123",
        "--evidence-linear",
        "LIN-1",
        "--tags",
        "x,y",
    ]);
    project.go_ok(&["add", "s"]);
    project.go_ok_with_input(&["record", "s", "--stdin"], RECORDS_JSON);

    let (_store, registry) = open(&project);
    for (domain, count) in [("d", 7), ("s", 6)] {
        let path = project.expertise(domain);
        let original = std::fs::read(&path).unwrap();
        assert_eq!(
            record_lines(&String::from_utf8(original.clone()).unwrap()).len(),
            count
        );
        let mut records = ok(
            store::read_records(&path, &registry, false),
            "store::read_records",
        );
        assert_eq!(records.len(), count);
        ok(
            store::with_file_lock(&path, || {
                store::write_records(&path, &mut records, &registry)
            }),
            "store::write_records under the lock",
        );
        assert_eq!(
            std::fs::read(&path).unwrap(),
            original,
            "rewrite of {domain} changed the bytes"
        );
        // A second rewrite is just as stable.
        let mut again = ok(
            store::read_records(&path, &registry, false),
            "store::read_records",
        );
        ok(
            store::write_records(&path, &mut again, &registry),
            "store::write_records",
        );
        assert_eq!(std::fs::read(&path).unwrap(), original);
    }
    assert_no_leftovers(&project, "after rewrites");
    // The Go tool still reads the files as its own.
    let validated = project.go_ok(&["validate"]);
    assert!(validated.contains("\"totalRecords\": 13"), "{validated}");
    assert!(validated.contains("\"totalErrors\": 0"), "{validated}");
}

#[test]
#[ignore = "needs the luuna-atlas reference clone and Go; run the -- --ignored check"]
fn format_go_writes_rust_reads_unicode_and_escapes() {
    let twin = Twin::new(true);
    let go = &twin.go;
    go.go_ok(&["init"]);
    go.go_ok(&["add", "d"]);
    go.go_ok(&[
        "record",
        "d",
        "--type",
        "failure",
        "--description",
        TRICKY_TEXT,
        "--resolution",
        "ok",
    ]);
    let path = go.expertise("d");
    go.append(&path, &format!("{ESCAPED_LINE}\n"));
    copy_store(go, &twin.rust);

    let (store, registry) = open(&twin.rust);
    let rust_path = ok(store.expertise_path("d"), "expertise_path");
    let mut records = ok(
        store::read_records(&rust_path, &registry, false),
        "store::read_records",
    );
    assert_eq!(records.len(), 2);
    assert_eq!(text(&records[0], "description"), TRICKY_TEXT);
    let escaped = &records[1];
    assert_eq!(text(escaped, "content"), ESCAPED_CONTENT);
    assert_eq!(number(escaped, "n"), 1.5);
    assert_eq!(number(escaped, "big"), 12345678901234567890.0);
    assert_eq!(number(escaped, "small"), 0.000001);
    assert_eq!(number(escaped, "neg"), 0.0);
    assert_eq!(number(escaped, "e"), 1e21);
    assert_eq!(number(escaped, "f"), 1e-7);
    assert_eq!(number(escaped, "x"), 1e20);
    assert!(escaped
        .get("tags")
        .and_then(Value::as_array)
        .unwrap()
        .is_empty());

    // The Go tool edits the first record, which rewrites the whole file in its encoding;
    // the crate makes the same edit and must produce the same bytes.
    let id = text(&records[0], "id").to_owned();
    go.go_ok(&["edit", "d", &id, "--resolution", "changed"]);
    records[0].insert("resolution", "changed");
    ok(
        store::write_records(&rust_path, &mut records, &registry),
        "store::write_records",
    );
    assert_eq!(
        twin.rust.read(&rust_path),
        go.read(&path),
        "the crate's rewrite differs from the Go tool's"
    );
    // The Go tool shows the escaped record exactly as the crate reads it.
    assert_eq!(go_show(go, "mx-bbbbbb"), Value::Object(records[1].clone()));
}

#[test]
#[ignore = "needs the luuna-atlas reference clone and Go; run the -- --ignored check"]
fn format_go_writes_rust_reads_outcomes_and_evidence() {
    let sandbox = Sandbox::new();
    let project = go_store(&sandbox, "go");
    project.go_ok(&[
        "record",
        "d",
        "--type",
        "failure",
        "--description",
        "Flaky lock test",
        "--resolution",
        "Retry with backoff",
        "--outcome-status",
        "success",
        "--outcome-duration",
        "1500",
        "--outcome-agent",
        "tester-agent",
        "--outcome-test-results",
        "3 passed",
        "--evidence-commit",
        "abc123",
        "--evidence-gh",
        "owner/repo#12",
        "--evidence-missions",
        "lms-1",
        "--tags",
        "a,b",
        "--relates-to",
        "mx-abcdef",
    ]);
    let path = project.expertise("d");
    project.append(&path, &format!("{LEGACY_OUTCOME_LINE}\n"));

    let (_store, registry) = open(&project);
    let records = ok(
        store::read_records(&path, &registry, false),
        "store::read_records",
    );
    assert_eq!(records.len(), 2);

    let fresh = &records[0];
    let evidence = object(fresh, "evidence");
    assert_eq!(keys(evidence), ["commit", "missions", "gh"]);
    assert_eq!(
        evidence.get("commit").and_then(Value::as_str),
        Some("abc123")
    );
    assert_eq!(
        evidence.get("gh").and_then(Value::as_str),
        Some("owner/repo#12")
    );
    assert_eq!(
        evidence.get("missions").and_then(Value::as_str),
        Some("lms-1")
    );
    assert_eq!(texts(fresh, "tags"), ["a", "b"]);
    assert_eq!(texts(fresh, "relates_to"), ["mx-abcdef"]);
    let outcomes = fresh.get("outcomes").and_then(Value::as_array).unwrap();
    assert_eq!(outcomes.len(), 1);
    let outcome = outcomes[0].as_object().unwrap();
    assert_eq!(
        keys(outcome),
        ["status", "duration", "test_results", "agent"]
    );
    assert_eq!(
        outcome.get("duration").and_then(Value::as_f64),
        Some(1500.0)
    );

    // The legacy singular `outcome` becomes `outcomes`: placed last, keys sorted, extra
    // fields dropped, as the Go tool does.
    let legacy = &records[1];
    assert!(!legacy.contains_key("outcome"));
    assert_eq!(keys(legacy).last().copied(), Some("outcomes"));
    let outcomes = legacy.get("outcomes").and_then(Value::as_array).unwrap();
    assert_eq!(outcomes.len(), 1);
    let outcome = outcomes[0].as_object().unwrap();
    assert_eq!(keys(outcome), ["agent", "duration", "status"]);
    assert_eq!(
        outcome.get("status").and_then(Value::as_str),
        Some("failure")
    );
    assert_eq!(outcome.get("duration").and_then(Value::as_f64), Some(5.0));

    // The Go tool shows both records exactly as the crate reads them.
    let fresh_id = text(fresh, "id").to_owned();
    assert_eq!(go_show(&project, &fresh_id), Value::Object(fresh.clone()));
    assert_eq!(
        go_show(&project, "mx-111111"),
        Value::Object(legacy.clone())
    );
}

#[test]
#[ignore = "needs the luuna-atlas reference clone and Go; run the -- --ignored check"]
fn format_go_config_rust_add_matches_go_add() {
    let twin = Twin::new(true);
    twin.prepare(|p: &Project| {
        p.go_ok(&["init"]);
    });
    // On the commented initial template.
    let go = twin.go.go(&["add", "first"]);
    let rust = commands::run_add(&twin.rust.dir, "first");
    assert_same_output("add on the template", &twin.go, &go, &twin.rust, &rust);
    twin.assert_same_tree("add on the template");

    // On a config the Go tool rewrote with values of several kinds.
    twin.prepare(|p: &Project| {
        p.go_ok(&["config", "set", "custom_types.hypothesis", HYPOTHESIS_TYPE]);
        p.go_ok(&["config", "set", "search.boost_factor", "0.25"]);
        p.go_ok(&["config", "set", "prime.tier_weights.star", "120"]);
        p.go_ok(&["config", "set", "disabled_types", "[failure]"]);
        p.go_ok(&["config", "set", "governance.max_entries", "50"]);
    });
    twin.assert_same_tree("after config set");
    let go = twin.go.go(&["add", "second"]);
    let rust = commands::run_add(&twin.rust.dir, "second");
    assert_same_output("add after config set", &twin.go, &go, &twin.rust, &rust);
    twin.assert_same_tree("add after config set");

    // The library call underneath gives the same file as the Go tool's add.
    let go = twin.go.go(&["add", "third"]);
    assert_eq!(go.exit_code, 0, "{go:?}");
    let store = ok(AtlasStore::open(&twin.rust.dir), "AtlasStore::open");
    ok(
        config::add_domain(&store.atlas_dir(), "third"),
        "config::add_domain",
    );
    twin.assert_same_tree("config::add_domain");
}

#[test]
#[ignore = "needs the luuna-atlas reference clone and Go; run the -- --ignored check"]
fn format_go_config_set_values_rust_rewrites_identical() {
    let sandbox = Sandbox::new();
    let project = go_store(&sandbox, "go");
    for (path, value) in [
        ("custom_types.hypothesis", HYPOTHESIS_TYPE),
        ("search.boost_factor", "0.25"),
        ("quality.level", "error"),
        ("prime.default_mode", "manifest"),
        ("prime.tier_weights.star", "120"),
        ("disabled_types", "[failure]"),
        ("hook_settings.timeout_ms", "7000"),
        ("governance.max_entries", "50"),
        ("classification_defaults.shelf_life.tactical", "7"),
    ] {
        project.go_ok(&["config", "set", path, value]);
    }
    let written = project.read(&project.config_path());
    // The Go tool's quoting is part of what must be reproduced.
    assert!(
        written.contains("    summary: '{statement}'\n"),
        "{written}"
    );
    assert!(
        written.contains("    section_title: 'Hypotheses: open'\n"),
        "{written}"
    );
    assert!(
        written.contains("disabled_types:\n  - failure\n"),
        "{written}"
    );

    let store = ok(AtlasStore::open(&project.dir), "AtlasStore::open");
    let config = ok(
        config::read_config(&store.atlas_dir()),
        "config::read_config",
    );
    ok(
        config::write_config(&store.atlas_dir(), &config),
        "config::write_config",
    );
    assert_eq!(project.read(&project.config_path()), written);

    // `config show` prints the same as the Go tool's.
    let go = project.go(&["config", "show"]);
    let rust = commands::run_config_show(&project.dir);
    assert_eq!(rust, go);
}

#[test]
#[ignore = "needs the luuna-atlas reference clone and Go; run the -- --ignored check"]
fn format_go_archive_rust_reads_and_rewrites() {
    let sandbox = Sandbox::new();
    let project = go_store(&sandbox, "go");
    project.go_ok(&[
        "record",
        "d",
        "--type",
        "guide",
        "--name",
        "Keep me",
        "--description",
        "stays live",
    ]);
    project.go_ok(&[
        "record",
        "d",
        "--type",
        "guide",
        "--name",
        "Archive me",
        "--description",
        "goes away",
    ]);
    let lines = project.read(&project.expertise("d"));
    let archived_id = lines
        .lines()
        .nth(1)
        .and_then(|l| l.split("\"id\":\"").nth(1))
        .and_then(|rest| rest.get(..9))
        .expect("the second record's id")
        .to_owned();
    project.go_ok(&["archive", "d", &archived_id, "--reason", "superseded"]);
    let archive_path = project.archive("d");
    let original = project.read(&archive_path);
    assert_eq!(original.lines().next(), Some(BANNER));

    let (store, registry) = open(&project);
    let records = ok(
        store::read_archive(&ok(store.archive_path("d"), "archive_path"), &registry),
        "store::read_archive",
    );
    assert_eq!(records.len(), 1);
    let record = &records[0];
    assert_eq!(text(record, "id"), archived_id);
    assert_eq!(text(record, "name"), "Archive me");
    assert_eq!(text(record, "status"), "archived");
    assert_eq!(text(record, "archive_reason"), "manual: superseded");
    assert!(is_go_timestamp(text(record, "archived_at")));
    let order = keys(record);
    assert_eq!(
        &order[order.len() - 3..],
        ["status", "archived_at", "archive_reason"]
    );

    ok(
        store::write_archive(&archive_path, &records),
        "store::write_archive",
    );
    assert_eq!(project.read(&archive_path), original);
    assert_no_leftovers(&project, "after write_archive");

    let live = ok(
        store::read_records(&project.expertise("d"), &registry, false),
        "store::read_records",
    );
    assert_eq!(live.len(), 1);
    assert_eq!(text(&live[0], "name"), "Keep me");
}

// ---------------------------------------------------------------------------------------
// The crate writes, Go reads

#[test]
#[ignore = "needs the luuna-atlas reference clone and Go; run the -- --ignored check"]
fn format_rust_writes_go_validates() {
    let sandbox = Sandbox::new();
    let project = sandbox.project("rust", true);
    assert_eq!(commands::run_init(&project.dir).exit_code, 0);
    assert_eq!(commands::run_add(&project.dir, "d").exit_code, 0);
    let (store, registry) = open(&project);
    let path = ok(store.expertise_path("d"), "expertise_path");
    for mut record in parse_records(RECORDS_JSON) {
        ok(
            store::append_record(&path, &mut record, &registry),
            "store::append_record",
        );
        assert!(is_go_id(text(&record, "id")));
    }
    let contents = project.read(&path);
    assert_eq!(record_lines(&contents).len(), 6);
    assert!(contents.ends_with("}\n"));
    assert!(!contents.contains("\n\n"));

    let validated = project.go_ok(&["validate"]);
    assert!(validated.contains("\"valid\": true"), "{validated}");
    assert!(validated.contains("\"totalRecords\": 6"), "{validated}");
    assert!(validated.contains("\"totalErrors\": 0"), "{validated}");
    assert!(validated.contains("\"totalWarnings\": 0"), "{validated}");
    // The Go tool's own consistency checks pass on the crate's store too.
    let doctor = project.go(&["doctor"]);
    assert!(
        !doctor.stdout.contains("\"status\": \"fail\""),
        "{doctor:?}"
    );
}

#[test]
#[ignore = "needs the luuna-atlas reference clone and Go; run the -- --ignored check"]
fn format_rust_writes_go_show_matches() {
    let sandbox = Sandbox::new();
    let project = go_store(&sandbox, "rust");
    let (store, registry) = open(&project);
    let path = ok(store.expertise_path("d"), "expertise_path");
    let mut written = Vec::new();
    for mut record in parse_records(RECORDS_JSON) {
        ok(
            store::append_record(&path, &mut record, &registry),
            "store::append_record",
        );
        written.push(record);
    }
    for record in &written {
        let id = text(record, "id");
        let output = project.go(&["show", id]);
        assert_eq!(output.exit_code, 0, "{output:?}");
        assert_eq!(output.stderr, "");
        assert_eq!(output.stdout, show_envelope("d", record), "show {id}");
    }
}

#[test]
#[ignore = "needs the luuna-atlas reference clone and Go; run the -- --ignored check"]
fn format_rust_writes_go_edit_matches_go_twin() {
    let twin = Twin::new(true);
    twin.prepare(|p: &Project| {
        p.go_ok(&["init"]);
        p.go_ok(&["add", "d"]);
    });
    // The Go tool writes the records itself in one project, the crate in the other.
    let created = twin
        .go
        .go_ok_with_input(&["record", "d", "--stdin"], RECORDS_JSON);
    assert!(created.contains("\"created\": 6"), "{created}");
    let (store, registry) = open(&twin.rust);
    let path = ok(store.expertise_path("d"), "expertise_path");
    let mut ids = Vec::new();
    for mut record in parse_records(RECORDS_JSON) {
        ok(
            store::append_record(&path, &mut record, &registry),
            "store::append_record",
        );
        ids.push(text(&record, "id").to_owned());
    }
    assert_eq!(
        twin.rust.read(&path),
        twin.go.read(&twin.go.expertise("d")),
        "the crate's appended records differ from the Go tool's"
    );
    twin.assert_same_tree("after writing");

    // The Go tool then edits both stores the same way; the results must match.
    for (id, args) in [
        (&ids[2], vec!["--resolution", "Edited by Go"]),
        (
            &ids[1],
            vec![
                "--files",
                "src/other.rs",
                "--classification",
                "foundational",
            ],
        ),
        (
            &ids[3],
            vec![
                "--outcome-status",
                "success",
                "--outcome-agent",
                "edit-agent",
            ],
        ),
    ] {
        let mut full = vec!["edit", "d", id.as_str()];
        full.extend(args);
        let go = twin.go.go(&full);
        let rust = twin.rust.go(&full);
        assert_eq!(go.exit_code, 0, "{go:?}");
        assert_same_output(&format!("{full:?}"), &twin.go, &go, &twin.rust, &rust);
        twin.assert_same_tree(&format!("{full:?}"));
    }
}

#[test]
#[ignore = "needs the luuna-atlas reference clone and Go; run the -- --ignored check"]
fn format_rust_archive_go_search_archived() {
    let sandbox = Sandbox::new();
    let project = go_store(&sandbox, "rust");
    let mut archived = parse_records(
        r#"[{"type":"guide","classification":"tactical","recorded_at":"2025-01-02T03:04:05.000Z","name":"Zebracorn guide","description":"Retired zebracorn workflow","id":"mx-e0979a","status":"archived","archived_at":"2025-02-03T04:05:06.000Z","archive_reason":"manual: obsolete"}]"#,
    );
    let (store, _registry) = open(&project);
    let archive_path = ok(store.archive_path("d"), "archive_path");
    ok(
        store::write_archive(&archive_path, &archived),
        "store::write_archive",
    );
    let written = project.read(&archive_path);
    assert_eq!(written.lines().next(), Some(BANNER));
    assert_eq!(written.lines().count(), 2);
    assert!(written.ends_with("}\n"));

    let stdout = project.go_ok(&["search", "zebracorn", "--archived"]);
    let result = parse(&stdout);
    let result = result.as_object().unwrap();
    assert_eq!(
        result.get("total").and_then(Value::as_f64),
        Some(1.0),
        "{stdout}"
    );
    let domains = result.get("domains").and_then(Value::as_array).unwrap();
    assert_eq!(domains.len(), 1, "{stdout}");
    let domain = domains[0].as_object().unwrap();
    assert_eq!(domain.get("domain").and_then(Value::as_str), Some("d"));
    let found = domain.get("archived").and_then(Value::as_array).unwrap();
    assert_eq!(found, &vec![Value::Object(archived.remove(0))], "{stdout}");
    // The live search does not see it.
    let live = project.go_ok(&["search", "zebracorn"]);
    assert!(live.contains("\"total\": 0"), "{live}");
}

#[test]
#[ignore = "needs the luuna-atlas reference clone and Go; run the -- --ignored check"]
fn format_rust_ids_match_go_ids() {
    let sandbox = Sandbox::new();
    let project = go_store(&sandbox, "go");
    for args in BUILTIN_RECORDS {
        project.go_ok(args);
    }
    project.go_ok(&[
        "record",
        "d",
        "--type",
        "guide",
        "--name",
        TRICKY_TEXT,
        "--description",
        "unicode id key",
    ]);
    let (_store, registry) = open(&project);
    let records = ok(
        store::read_records(&project.expertise("d"), &registry, false),
        "store::read_records",
    );
    assert_eq!(records.len(), 7);
    for record in &records {
        let mut without_id = record.clone();
        let go_id = without_id.remove("id").expect("the Go tool assigned an id");
        assert_eq!(
            ok(
                store::generate_record_id(&without_id, &registry),
                "store::generate_record_id"
            ),
            go_id.as_str().unwrap(),
            "{record:?}"
        );
    }
    // Known values: mx- and the first six hex digits of sha256("<type>:<id key value>").
    assert_eq!(text(&records[0], "id"), "mx-1ab8cf");
    assert_eq!(text(&records[2], "id"), "mx-c80f7b");
    let mut record = Object::new();
    record.insert("type", "guide");
    record.insert("name", "A");
    assert_eq!(
        ok(
            store::generate_record_id(&record, &registry),
            "store::generate_record_id"
        ),
        "mx-eb3079"
    );
    // An unknown type has no id.
    let mut unknown = Object::new();
    unknown.insert("type", "no_such_type");
    unknown.insert("name", "A");
    assert!(store::generate_record_id(&unknown, &registry).is_err());
}

// ---------------------------------------------------------------------------------------
// Both directions

#[test]
#[ignore = "needs the luuna-atlas reference clone and Go; run the -- --ignored check"]
fn format_custom_types_and_aliases_round_trip() {
    let twin = Twin::new(true);
    let go = &twin.go;
    go.go_ok(&["init"]);
    go.go_ok(&["add", "d"]);
    go.go_ok(&["config", "set", "custom_types.hypothesis", HYPOTHESIS_TYPE]);
    go.go_ok_with_input(
        &["record", "d", "--stdin"],
        r#"{"type":"hypothesis","statement":"S1","prediction":"P1","recorded_at":"2025-01-02T03:04:05.000Z","classification":"tactical"}"#,
    );
    go.append(&go.expertise("d"), &format!("{LEGACY_CLAIM_LINE}\n"));
    copy_store(go, &twin.rust);

    // Go writes, the crate reads: the alias is applied and the canonical field goes last.
    let (store, registry) = open(&twin.rust);
    let path = ok(store.expertise_path("d"), "expertise_path");
    let mut records = ok(
        store::read_records(&path, &registry, false),
        "store::read_records",
    );
    assert_eq!(records.len(), 2);
    assert_eq!(text(&records[0], "id"), "mx-03f75e");
    assert_eq!(
        ok(
            store::generate_record_id(&records[0], &registry),
            "store::generate_record_id"
        ),
        "mx-03f75e"
    );
    let legacy = &records[1];
    assert!(!legacy.contains_key("claim"));
    assert_eq!(
        keys(legacy),
        [
            "type",
            "classification",
            "recorded_at",
            "prediction",
            "id",
            "statement"
        ]
    );
    assert_eq!(text(legacy, "statement"), "Legacy claim");
    assert_eq!(go_show(go, "mx-222222"), Value::Object(legacy.clone()));
    // Unknown types are refused unless allowed.
    let builtin_only = ok(Registry::from_config(None), "Registry::from_config(None)");
    assert!(store::read_records(&path, &builtin_only, false).is_err());
    assert_eq!(
        ok(
            store::read_records(&path, &builtin_only, true),
            "read_records allowing unknown"
        )
        .len(),
        2
    );

    // Both rewrite after the same edit: identical bytes.
    go.go_ok(&["edit", "d", "mx-222222", "--prediction", "P9"]);
    records[1].insert("prediction", "P9");
    ok(
        store::write_records(&path, &mut records, &registry),
        "store::write_records",
    );
    assert_eq!(twin.rust.read(&path), go.read(&go.expertise("d")));

    // The crate writes a custom-type record; the Go tool validates and shows it.
    let mut fresh = Object::new();
    fresh.insert("type", "hypothesis");
    fresh.insert("classification", "foundational");
    fresh.insert("recorded_at", "2025-03-04T05:06:07.000Z");
    fresh.insert("statement", "Caching halves latency");
    fresh.insert("prediction", "p95 drops below 50 ms");
    fresh.insert("evidence_files", "bench/cache.txt");
    ok(
        store::append_record(&path, &mut fresh, &registry),
        "store::append_record",
    );
    let validated = twin.rust.go_ok(&["validate"]);
    assert!(validated.contains("\"valid\": true"), "{validated}");
    assert!(validated.contains("\"totalRecords\": 3"), "{validated}");
    assert_eq!(
        go_show(&twin.rust, text(&fresh, "id")),
        Value::Object(fresh.clone())
    );
    // And the Go tool's own id for the same content is the crate's.
    let go_created = go.go_ok_with_input(
        &["record", "d", "--stdin"],
        r#"{"type":"hypothesis","classification":"foundational","recorded_at":"2025-03-04T05:06:07.000Z","statement":"Caching halves latency","prediction":"p95 drops below 50 ms","evidence_files":"bench/cache.txt"}"#,
    );
    assert!(go_created.contains("\"created\": 1"), "{go_created}");
    assert_eq!(twin.rust.read(&path), go.read(&go.expertise("d")));
}

#[test]
#[ignore = "needs the luuna-atlas reference clone and Go; run the -- --ignored check"]
fn format_no_leftover_lock_or_tmp_files() {
    let sandbox = Sandbox::new();
    let project = sandbox.project("both", true);
    assert_eq!(commands::run_init(&project.dir).exit_code, 0);
    assert_eq!(commands::run_add(&project.dir, "d").exit_code, 0);
    let (store, registry) = open(&project);
    let path = ok(store.expertise_path("d"), "expertise_path");
    let lock = path.with_file_name("d.jsonl.lock");

    // The crate's writes.
    ok(
        store::with_file_lock(&path, || {
            assert!(lock.is_file(), "the lock file exists while it is held");
            for mut record in parse_records(RECORDS_JSON) {
                store::append_record(&path, &mut record, &registry)?;
            }
            Ok(())
        }),
        "append under the lock",
    );
    assert!(!lock.exists(), "the lock is removed afterwards");
    let mut records = ok(
        store::read_records(&path, &registry, false),
        "store::read_records",
    );
    ok(
        store::with_file_lock(&path, || {
            store::write_records(&path, &mut records, &registry)
        }),
        "rewrite under the lock",
    );
    let archive_path = ok(store.archive_path("d"), "archive_path");
    ok(
        store::with_file_lock(&archive_path, || {
            store::write_archive(&archive_path, &records[..1])
        }),
        "archive under the lock",
    );
    // A failing closure still releases the lock.
    let failed: luuna_atlas::Result<()> = store::with_file_lock(&path, || {
        Err(luuna_atlas::AtlasError::Message("deliberate".into()))
    });
    assert!(failed.is_err());
    assert!(!lock.exists(), "the lock is removed after a failure");
    ok(
        config::add_domain(&store.atlas_dir(), "e"),
        "config::add_domain",
    );
    assert_eq!(commands::run_add(&project.dir, "f").exit_code, 0);
    assert_no_leftovers(&project, "after the crate's writes");

    // The Go tool's writes on the crate's store.
    let id = text(&records[0], "id").to_owned();
    project.go_ok(&[
        "edit",
        "d",
        &id,
        "--content",
        "Edited by Go because the crate wrote it",
    ]);
    project.go_ok(&[
        "record",
        "e",
        "--type",
        "guide",
        "--name",
        "From Go",
        "--description",
        "go wrote this",
    ]);
    project.go_ok(&["archive", "d", &id, "--reason", "done"]);
    assert_no_leftovers(&project, "after the Go tool's writes");

    // And the crate reads back what the Go tool wrote.
    let (store, registry) = open(&project);
    let archive = ok(
        store::read_archive(&archive_path, &registry),
        "store::read_archive",
    );
    assert_eq!(archive.len(), 2);
    let e = ok(
        store::read_records(
            &ok(store.expertise_path("e"), "expertise_path"),
            &registry,
            false,
        ),
        "store::read_records",
    );
    assert_eq!(e.len(), 1);
    assert_no_leftovers(&project, "at the end");
}
