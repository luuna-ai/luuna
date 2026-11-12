//! Owner: plan s3-beacon-lib-2. Reference tests of the authoring commands against the Go tool.
//!
//! Each Go-dependent test of the plan carries the ignore string
//! `#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with LUUNA_BEACON_REFERENCE=<clone> LUUNA_BEACON_GO=<go> cargo test -p luuna-beacon --locked --test parity_authoring -- --ignored"]`.

mod support;

use luuna_beacon::{AuthoringError, BeaconError, CreatePrompt, UpdatePrompt};
use serde_json::{json, Value};
use std::fs;
use std::path::Path;
use support::authoring::*;
use support::case::{self, *};
use support::go;
use support::payload::PlanDone;

// ---- helpers ------------------------------------------------------------------

/// A case whose project has been initialised on both sides.
fn ready() -> Case {
    let mut case = Case::new();
    case.step(Step::Init);
    case
}

fn up() -> UpdatePrompt {
    UpdatePrompt::default()
}

fn lossy(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// Runs `run`; a panic inside it is raised again with `label` in front, so
/// that a failure names the probe row and command.
fn labelled<T>(label: &str, run: impl FnOnce() -> T) -> T {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(run)) {
        Ok(value) => value,
        Err(payload) => {
            let message = payload
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| {
                    payload
                        .downcast_ref::<&str>()
                        .map(|text| (*text).to_owned())
                })
                .unwrap_or_else(|| "a panic without a message".to_owned());
            panic!("{label}: {message}");
        }
    }
}

/// The error Go printed without `--json` (`Error: <message>` on stderr),
/// with the project root written `<root>`.
fn go_error_text(stderr: &[u8], root: &Path) -> String {
    let text = lossy(stderr);
    let text = text.strip_suffix('\n').unwrap_or(&text);
    let text = text.strip_prefix("Error: ").unwrap_or_else(|| {
        panic!("Go's stderr does not start with `Error: `: {text:?}");
    });
    text.replace(&*root.to_string_lossy(), "<root>")
}

/// Compares a command Go prints as text (no `--json`) with the library's
/// bytes for it: exit status, stdout byte for byte, or the error text.
fn assert_text_matches(
    case: &Case,
    go_args: &[&str],
    lib: Result<Vec<u8>, BeaconError>,
    os_prefix: Option<&str>,
) {
    let context = format!("text output of `{}`", go_args.join(" "));
    let (code, stdout, stderr) = run_go_text(case.go_root(), go_args);
    match lib {
        Ok(bytes) => {
            assert_eq!(
                code,
                0,
                "{context}: the library succeeded, Go failed: {}",
                lossy(&stderr)
            );
            assert_eq!(lossy(&stdout), lossy(&bytes), "{context}: text differs");
            assert_eq!(stdout, bytes, "{context}: bytes differ");
        }
        Err(error) => {
            assert_eq!(
                code,
                1,
                "{context}: the library failed ({error}), Go exited {code} with {}",
                lossy(&stdout)
            );
            let go = go_error_text(&stderr, case.go_root());
            let lib = error
                .to_string()
                .replace(&*case.lib_root().to_string_lossy(), "<root>");
            case::assert_errors_match(&go, &lib, os_prefix, &context);
        }
    }
}

/// `render <reference>` without `--json`, against `Beacon::render_text`.
fn assert_render_text(case: &Case, reference: &str) {
    let lib = case.beacon().render_text(reference).map(String::into_bytes);
    assert_text_matches(case, &["render", reference], lib, None);
}

/// `config show` without `--json`, against `ConfigShow::text`.
fn assert_config_show_text(case: &Case) {
    let lib = case.beacon().config_show().map(|shown| shown.text);
    assert_text_matches(case, &["config", "show"], lib, None);
}

/// `config show --path <path>` without `--json`, against
/// `YamlValue::to_yaml`.
fn assert_config_path_text(case: &Case, path: &str) {
    let lib = case
        .beacon()
        .config_get(path)
        .map(|value| value.to_yaml().into_bytes());
    assert_text_matches(case, &["config", "show", "--path", path], lib, None);
}

/// Every file outside the stores, compared byte for byte.
fn assert_outside(case: &Case, context: &str) {
    compare_outside_store(case.go_root(), case.lib_root(), context);
}

/// One hand-written prompt line whose frontmatter is `frontmatter`, written
/// as given (compact JSON text), with one section.
fn fm_record(hex: &str, name: &str, frontmatter: &str) -> String {
    format!(
        r#"{{"id":"luuna-beacon-{hex}","name":"{name}","version":1,"sections":[{{"name":"main","body":"body"}}],"frontmatter":{frontmatter},"status":"active","createdAt":"{STAMP}","updatedAt":"{STAMP}"}}"#
    ) + "\n"
}

/// Writes the given lines as the whole prompt store on both sides.
fn store(case: &mut Case, lines: &[String]) {
    case.step(write_file(PROMPTS, lines.concat().as_bytes()));
}

/// The record the probes created before overwriting the store.
fn probe_create() -> Step {
    create_with(CreatePrompt {
        name: "p1".into(),
        description: "d".into(),
        sections: vec![sec("main", "body")],
        frontmatter: pairs(&[("k", "x")]),
        ..Default::default()
    })
}

/// A frontmatter number as a float64 (as Go reads every JSON number).
fn number(frontmatter: &std::collections::BTreeMap<String, Value>, key: &str) -> Option<f64> {
    frontmatter.get(key).and_then(Value::as_f64)
}

/// A JSON object text built from `(key, value)` pairs with each string
/// JSON-escaped.
fn json_object(entries: &[(String, Value)]) -> String {
    let parts: Vec<String> = entries
        .iter()
        .map(|(key, value)| format!("{}:{}", Value::String(key.clone()), value))
        .collect();
    format!("{{{}}}", parts.join(","))
}

// ---- stats ----------------------------------------------------------------------

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with LUUNA_BEACON_REFERENCE=<clone> LUUNA_BEACON_GO=<go> cargo test -p luuna-beacon --locked --test parity_authoring -- --ignored"]
fn stats_counts_current_records_of_every_status_and_repeated_tags() {
    // GD42: archived prompts are counted, and a tag repeated inside one
    // prompt is counted twice.
    let mut case = ready();
    case.step(create_with(CreatePrompt {
        name: "a".into(),
        tags: strings(&["x", "y"]),
        sections: vec![sec("Task", "one")],
        ..Default::default()
    }));
    case.step(create_with(CreatePrompt {
        name: "b".into(),
        tags: strings(&["x"]),
        status: "draft".into(),
        ..Default::default()
    }));
    case.step(create("c", &[("Task", "gone")]));
    case.step(archive("c"));
    case.step(create_with(CreatePrompt {
        name: "d".into(),
        tags: strings(&["z", "z"]),
        ..Default::default()
    }));
    case.step(update(
        "a",
        UpdatePrompt {
            add_tags: strings(&["w"]),
            ..up()
        },
    ));
    let seen = case.step(stats());
    assert_eq!(seen.code, 0);
    assert_eq!(seen.raw[0]["prompts"]["total"], json!(4));
    assert_eq!(seen.raw[0]["prompts"]["byStatus"]["archived"], json!(1));
    case.step(write_file(
        PROMPTS,
        [
            prompt_record(
                "luuna-beacon-a1",
                "r",
                1,
                STAMP,
                &[("S", "1")],
                json!({"tags": ["t", "t", "u"]}),
            ),
            prompt_record(
                "luuna-beacon-a1",
                "r",
                2,
                STAMP,
                &[("S", "2")],
                json!({"tags": ["t"], "status": "deprecated"}),
            ),
            prompt_record(
                "luuna-beacon-a2",
                "s",
                1,
                STAMP,
                &[("S", "1")],
                json!({"status": ""}),
            ),
        ]
        .concat()
        .as_bytes(),
    ));
    case.step(stats());
}

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with LUUNA_BEACON_REFERENCE=<clone> LUUNA_BEACON_GO=<go> cargo test -p luuna-beacon --locked --test parity_authoring -- --ignored"]
fn stats_on_an_empty_store_and_without_init() {
    let mut case = Case::new();
    let seen = case.step(stats());
    assert_eq!(seen.code, 0);
    assert_eq!(seen.raw[0]["prompts"]["total"], json!(0));
    assert!(
        store_paths(case.lib_root()).is_empty(),
        "stats must not create files"
    );
    case.step(Step::Init);
    case.step(stats());
    case.step(write_file(PROMPTS, b""));
    case.step(stats());
    case.step(write_file(PROMPTS, b"\n\n"));
    case.step(stats());
    case.step(write_file(PROMPTS, b"{not json}\n"));
    case.step(stats());
}

// ---- tree -----------------------------------------------------------------------

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with LUUNA_BEACON_REFERENCE=<clone> LUUNA_BEACON_GO=<go> cargo test -p luuna-beacon --locked --test parity_authoring -- --ignored"]
fn tree_reports_ancestors_children_and_mixin_users() {
    // GD12: children and mixinUsers come in map order; the hook sorts them.
    let mut case = ready();
    case.step(create("base", &[("Context", "base")]));
    case.step(create("m1", &[("Style", "terse")]));
    case.step(create("m2", &[("Tone", "plain")]));
    case.step(create_with(CreatePrompt {
        name: "mid".into(),
        extends: "base".into(),
        mixins: strings(&["m1"]),
        ..Default::default()
    }));
    case.step(create_with(CreatePrompt {
        name: "leaf".into(),
        extends: "mid".into(),
        mixins: strings(&["m1", "m2"]),
        ..Default::default()
    }));
    case.step(create_with(CreatePrompt {
        name: "sibling".into(),
        extends: "base".into(),
        ..Default::default()
    }));
    case.step(create_with(CreatePrompt {
        name: "other".into(),
        extends: "mid".into(),
        mixins: strings(&["m2"]),
        ..Default::default()
    }));
    for name in ["leaf", "mid", "base", "m1", "m2", "other"] {
        let seen = case.step(tree(name));
        assert_eq!(seen.code, 0, "tree {name}");
    }
    let seen = case.step(tree("leaf"));
    assert_eq!(seen.raw[0]["ancestors"], json!(["mid", "base"]));
    let seen = case.step(tree("base"));
    assert_eq!(seen.raw[0]["mixins"], Value::Null);
    case.step(archive("sibling"));
    case.step(tree("base"));
    case.step(tree("leaf@1"));
    case.step(tree("ghost"));
}

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with LUUNA_BEACON_REFERENCE=<clone> LUUNA_BEACON_GO=<go> cargo test -p luuna-beacon --locked --test parity_authoring -- --ignored"]
fn tree_keeps_a_missing_parent_and_refuses_an_old_version() {
    let mut case = ready();
    store(
        &mut case,
        &[
            prompt_record(
                "luuna-beacon-b1",
                "child",
                1,
                STAMP,
                &[("S", "x")],
                json!({"extends": "ghost"}),
            ),
            prompt_record(
                "luuna-beacon-b2",
                "grand",
                1,
                STAMP,
                &[("S", "x")],
                json!({"extends": "child"}),
            ),
            prompt_record("luuna-beacon-b3", "p", 1, STAMP, &[("S", "1")], json!({})),
            prompt_record(
                "luuna-beacon-b3",
                "p",
                2,
                "2024-01-02T00:00:00Z",
                &[("S", "2")],
                json!({}),
            ),
        ],
    );
    let seen = case.step(tree("grand"));
    assert_eq!(seen.raw[0]["ancestors"], json!(["child", "ghost"]));
    case.step(tree("child"));
    let seen = case.step(tree("p@1"));
    assert_eq!(seen.error.as_deref(), Some("prompt \"p@1\" not found"));
    case.step(tree("p@2"));
    case.step(tree("p@+2"));
    case.step(tree("p@x"));
    case.step(tree("ghost"));
}

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with LUUNA_BEACON_REFERENCE=<clone> LUUNA_BEACON_GO=<go> cargo test -p luuna-beacon --locked --test parity_authoring -- --ignored"]
fn tree_mixins_explicit_empty_array_differs() {
    // RD11: Go prints a stored `"mixins": []` as [], the library as null
    // (the foundation's Prompt cannot tell [] from absent). Everything else
    // is equal.
    let mut pair = Pair::new();
    pair.step(Step::Init);
    pair.raw(write_file(
        PROMPTS,
        prompt_record(
            "luuna-beacon-c1",
            "e",
            1,
            STAMP,
            &[("S", "x")],
            json!({"mixins": []}),
        )
        .as_bytes(),
    ));
    let step = tree("e");
    let go_run = pair.go(&step);
    assert_eq!(go_run.code, 0);
    let mut go_doc = go_run.documents[0].clone();
    assert_eq!(go_doc["mixins"], json!([]), "Go prints the stored []");
    let lib = pair.beacon().tree("e").expect("the library's tree");
    assert!(lib.mixins.is_none(), "the library reports no mixins (RD11)");
    let mut lib_doc = TreeDone(lib).documents()[0].clone();
    assert_eq!(lib_doc["mixins"], Value::Null);
    go_doc["mixins"] = Value::Null;
    sort_map_ordered("tree", &mut go_doc, case::DEFAULT_PROJECT);
    sort_map_ordered("tree", &mut lib_doc, case::DEFAULT_PROJECT);
    assert_eq!(go_doc, lib_doc, "apart from mixins the documents are equal");
}

// ---- pin and unpin --------------------------------------------------------------

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with LUUNA_BEACON_REFERENCE=<clone> LUUNA_BEACON_GO=<go> cargo test -p luuna-beacon --locked --test parity_authoring -- --ignored"]
fn pin_and_unpin_append_a_new_version() {
    // GD38: pin and unpin always append, even when nothing changes; unpin
    // ignores an @version.
    let mut case = ready();
    case.step(create("p", &[("Task", "one")]));
    case.step(update(
        "p",
        UpdatePrompt {
            sections: vec![sec("Task", "two")],
            ..up()
        },
    ));
    let seen = case.step(pin("p@1"));
    assert_eq!(seen.raw[0]["pinned"], json!(1));
    case.step(show("p"));
    case.step(history("p", None));
    case.step(pin("p@1"));
    case.step(pin("p@3"));
    let seen = case.step(unpin("p@99"));
    assert_eq!(seen.raw[0]["pinned"], Value::Null);
    case.step(unpin("p"));
    case.step(show("p"));
    case.step(history("p", None));
    case.step(list());
    case.step(create_with(CreatePrompt {
        name: "q".into(),
        status: "draft".into(),
        ..Default::default()
    }));
    case.step(archive("q"));
    case.step(pin("q@1"));
    case.step(show("q"));
}

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with LUUNA_BEACON_REFERENCE=<clone> LUUNA_BEACON_GO=<go> cargo test -p luuna-beacon --locked --test parity_authoring -- --ignored"]
fn pin_errors_match_go() {
    // GD39: the version is read with Sscanf %d (signs and trailing text).
    let mut case = Case::new();
    case.step(pin("p@1"));
    case.step(unpin("p"));
    case.step(Step::Init);
    case.step(create("p", &[("Task", "one")]));
    let seen = case.step(pin("p"));
    assert_eq!(seen.error.as_deref(), Some("expected name@version"));
    let seen = case.step(pin("p@x"));
    assert_eq!(seen.error.as_deref(), Some("invalid version: x"));
    case.step(pin("p@"));
    let seen = case.step(pin("p@9"));
    assert_eq!(seen.error.as_deref(), Some("version 9 not found"));
    case.step(pin("p@-1"));
    case.step(pin("ghost@1"));
    case.step(unpin("ghost"));
    case.step(pin("p@+1"));
    case.step(pin("p@1abc"));
    case.step(pin("p@ 1"));
    case.step(pin("@1"));
    case.step(show("p"));
    case.step(write_file(PROMPTS, b"{\"id\":1}\n"));
    case.step(pin("p@1"));
}

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with LUUNA_BEACON_REFERENCE=<clone> LUUNA_BEACON_GO=<go> cargo test -p luuna-beacon --locked --test parity_authoring -- --ignored"]
fn pin_is_honoured_by_emit_and_ignored_by_render() {
    // GD37: render ignores a pin; only emit honours it.
    let mut case = ready();
    case.step(create("p", &[("Task", "first")]));
    case.step(update(
        "p",
        UpdatePrompt {
            sections: vec![sec("Task", "second")],
            ..up()
        },
    ));
    case.step(pin("p@1"));
    assert_render_text(&case, "p");
    assert_render_text(&case, "p@1");
    assert_render_text(&case, "p@3");
    case.step(emit(emit_one("p")));
    assert_outside(&case, "emit of a pinned prompt");
    let emitted = fs::read_to_string(case.lib_root().join("agents/p.md")).expect("emitted file");
    assert!(
        emitted.contains("first"),
        "emit honours the pin: {emitted:?}"
    );
    case.step(unpin("p"));
    case.step(emit(emit_all()));
    assert_outside(&case, "emit after unpin");
}

/// Runs one probe record through `command` on a fresh case and compares
/// every step, then the show read-back.
fn run_probe_record(row: &str, record: &[u8], command: &str) -> Case {
    labelled(&format!("row {row} `{command}`"), || {
        let mut case = ready();
        case.step(probe_create());
        case.step(write_file(PROMPTS, record));
        let step = if command == "pin" {
            pin("p1@1")
        } else {
            unpin("p1")
        };
        case.step(step);
        case.step(show("p1"));
        case
    })
}

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with LUUNA_BEACON_REFERENCE=<clone> LUUNA_BEACON_GO=<go> cargo test -p luuna-beacon --locked --test parity_authoring -- --ignored"]
fn pin_and_unpin_probe_rows_like_go() {
    // Every family B row of go-lossy-probe-1 except the refused b04a, b04b,
    // b04c and b05d (tables B1 and B2). S2 applies to these rows.
    for (row, record) in FAMILY_B {
        if ["b04a", "b04b", "b04c", "b05d"].contains(&row) {
            continue;
        }
        for command in ["pin", "unpin"] {
            let case = run_probe_record(row, record, command);
            let after = fs::read(case.lib_root().join(PROMPTS)).expect("the store");
            match row {
                // Go refuses these itself; the store stays as written.
                "b05c" | "b05m" => assert_eq!(
                    after, record,
                    "S2: row {row} `{command}`: the store must be unchanged"
                ),
                _ => assert!(
                    after.len() > record.len() && after.starts_with(record),
                    "S2: row {row} `{command}`: one line must be appended to the unchanged record"
                ),
            }
        }
    }
    // The repeated-key records RD24 copies (S3 applies): n1, n2 and n3 of
    // go-lossy-probe-2 and the unprobed r24-fm-earlier-null and
    // r24-fm-merge. Both sides exit 0, the stores are equal after
    // normalisation (so the library's new line is Go's) and so are the show
    // read-backs.
    //   n1: table lines 5127-5128 (Go keeps "d").
    //   n2: table lines 5129-5130 (Go keeps ["a"]).
    //   n3: table lines 5131-5132 (Go keeps 1.0, read as 1).
    for (row, record) in copied_repeat_records() {
        for command in ["pin", "unpin"] {
            let label = format!("S3: copied repeated-key case {row} `{command}`");
            let case = run_probe_record(row, record.as_bytes(), command);
            let shown = case.beacon().show("p1").expect("show p1");
            match row {
                "n1" => assert_eq!(shown.description, "d", "{label}"),
                "n2" => assert_eq!(shown.tags, strings(&["a"]), "{label}"),
                "n3" => assert_eq!(number(&shown.frontmatter, "k"), Some(1.0), "{label}"),
                "r24-fm-earlier-null" => {
                    assert_eq!(shown.frontmatter.get("k"), Some(&json!("x")), "{label}")
                }
                "r24-fm-merge" => {
                    assert_eq!(number(&shown.frontmatter, "a"), Some(1.0), "{label}");
                    assert_eq!(number(&shown.frontmatter, "b"), Some(2.0), "{label}");
                }
                _ => unreachable!(),
            }
        }
    }
}

// ---- diff -----------------------------------------------------------------------

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with LUUNA_BEACON_REFERENCE=<clone> LUUNA_BEACON_GO=<go> cargo test -p luuna-beacon --locked --test parity_authoring -- --ignored"]
fn diff_reports_added_modified_and_removed() {
    // GD13: removed sections come in map order; the hook sorts them.
    let mut case = ready();
    case.step(create(
        "p",
        &[("A", "a"), ("B", "b"), ("C", "c"), ("E", "e")],
    ));
    case.step(update(
        "p",
        UpdatePrompt {
            sections: vec![sec("B", "changed"), sec("D", "new")],
            remove_sections: strings(&["C", "E"]),
            ..up()
        },
    ));
    let seen = case.step(diff("p", "1", "2"));
    assert_eq!(seen.code, 0);
    case.step(diff("p", "2", "1"));
    case.step(diff("p", "1", "1"));
    case.step(diff("p", "2", "2"));
    case.step(diff("p", "1", "3"));
    case.step(diff("ghost", "1", "2"));
}

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with LUUNA_BEACON_REFERENCE=<clone> LUUNA_BEACON_GO=<go> cargo test -p luuna-beacon --locked --test parity_authoring -- --ignored"]
fn diff_version_parsing_and_name_matching_quirks() {
    // GD40: an unparsable second version keeps the first's value, and
    // trailing text is accepted. GD41: records match by name across ids and
    // statuses (the last duplicate wins) and duplicate section names
    // collapse.
    let mut case = ready();
    store(
        &mut case,
        &[
            prompt_record(
                "luuna-beacon-d1",
                "p",
                1,
                STAMP,
                &[("S", "1"), ("T", "t")],
                json!({}),
            ),
            prompt_record(
                "luuna-beacon-d1",
                "p",
                2,
                STAMP,
                &[("S", "2")],
                json!({"status": "archived"}),
            ),
            prompt_record(
                "luuna-beacon-d2",
                "p",
                1,
                STAMP,
                &[("S", "x"), ("S", "y"), ("U", "u")],
                json!({}),
            ),
            prompt_record(
                "luuna-beacon-d2",
                "p",
                2,
                STAMP,
                &[("S", "y"), ("S", "z")],
                json!({}),
            ),
        ],
    );
    for (from, to) in [
        ("1", "2"),
        ("2", "1"),
        ("1", "x"),
        ("x", "1"),
        ("2abc", "1"),
        ("+1", "2"),
        ("-1", "2"),
        ("1", " 2"),
        ("", "2"),
        ("1", "2.5"),
    ] {
        case.step(diff("p", from, to));
    }
}

// ---- render text ---------------------------------------------------------------

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with LUUNA_BEACON_REFERENCE=<clone> LUUNA_BEACON_GO=<go> cargo test -p luuna-beacon --locked --test parity_authoring -- --ignored"]
fn render_text_matches_go_markdown_output() {
    // GD25: frontmatter keys `name` and `description` override the prompt's
    // own.
    let mut case = ready();
    case.step(create("single", &[("Task", "only body")]));
    case.step(create(
        "multi",
        &[("One", "first"), ("Two", "second\nline")],
    ));
    case.step(create_with(CreatePrompt {
        name: "described".into(),
        description: "A description: with a colon".into(),
        sections: vec![sec("Task", "x")],
        frontmatter: pairs(&[("owner", "me"), ("level", "3")]),
        ..Default::default()
    }));
    case.step(create_with(CreatePrompt {
        name: "overrides".into(),
        description: "own".into(),
        sections: vec![sec("Task", "x")],
        frontmatter: pairs(&[("name", "other"), ("description", "fm description")]),
        ..Default::default()
    }));
    case.step(create_with(CreatePrompt {
        name: "child".into(),
        extends: "multi".into(),
        mixins: strings(&["single"]),
        sections: vec![sec("Two", "overridden")],
        ..Default::default()
    }));
    case.step(create_with(CreatePrompt {
        name: "empty".into(),
        ..Default::default()
    }));
    case.step(create("trailing", &[("Task", "ends with a newline\n")]));
    case.step(update(
        "single",
        UpdatePrompt {
            sections: vec![sec("Task", "second version")],
            ..up()
        },
    ));
    for reference in [
        "single",
        "single@1",
        "single@2",
        "multi",
        "described",
        "overrides",
        "child",
        "empty",
        "trailing",
        "ghost",
        "single@9",
    ] {
        assert_render_text(&case, reference);
    }
    store(
        &mut case,
        &[
            prompt_record(
                "luuna-beacon-e1",
                "",
                1,
                STAMP,
                &[("S", "nameless")],
                json!({}),
            ),
            prompt_record("luuna-beacon-e2", "nosections", 1, STAMP, &[], json!({})),
            prompt_record(
                "luuna-beacon-e3",
                "nullsections",
                1,
                STAMP,
                &[],
                json!({"sections": null}),
            ),
        ],
    );
    assert_render_text(&case, "");
    assert_render_text(&case, "nosections");
    assert_render_text(&case, "nullsections");
}

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with LUUNA_BEACON_REFERENCE=<clone> LUUNA_BEACON_GO=<go> cargo test -p luuna-beacon --locked --test parity_authoring -- --ignored"]
fn render_text_ts_form_matches_go() {
    // GD26: the export name is only upper-cased with '-' as '_', and a
    // description with a newline breaks the comment.
    let mut case = ready();
    case.step(create_with(CreatePrompt {
        name: "my-prompt".into(),
        description: "Line one\nline two".into(),
        emit_as: "my-prompt.ts".into(),
        sections: vec![sec("Task", "back\\slash `tick` ${x} $y {z}")],
        ..Default::default()
    }));
    case.step(create_with(CreatePrompt {
        name: "9 lives.v2".into(),
        emit_as: "out/x.ts".into(),
        sections: vec![sec("A", "a"), sec("B", "b")],
        frontmatter: pairs(&[("k", "v")]),
        ..Default::default()
    }));
    case.step(create_with(CreatePrompt {
        name: "plain".into(),
        emit_as: "plain.TS".into(),
        sections: vec![sec("A", "not ts")],
        ..Default::default()
    }));
    for reference in ["my-prompt", "9 lives.v2", "plain"] {
        assert_render_text(&case, reference);
    }
    case.step(emit(emit_all()));
    assert_outside(&case, "emit of .ts prompts");
}

// ---- frontmatter YAML --------------------------------------------------------

/// The scalar corpus of the YAML emitter, as frontmatter strings.
const SCALAR_CORPUS: [&str; 52] = [
    "",
    "yes",
    "y",
    "on",
    "1:30",
    "0x1f",
    "0o17",
    "1_000",
    "+5",
    "1e3",
    ".5",
    ".inf",
    "~",
    "<<",
    "2024-01-02",
    "a: b",
    "- x",
    "-",
    "#x",
    "a #b",
    "---",
    "...",
    "it's",
    " leading",
    "trailing ",
    "tab\there",
    "\tlead tab",
    "é",
    "a\u{a0}b",
    "a\u{85}b",
    "a\u{2028}b",
    "a\u{2029}b",
    "\u{feff}bom",
    "emoji \u{1f600}",
    "ctl \u{1}",
    "del \u{7f}",
    "a\nb",
    "a\nb\n",
    "a\n\n",
    "\n",
    " a\nb",
    "a \nb",
    "a\r\nb",
    "a\rb",
    "null",
    "true",
    "False",
    "12",
    "-0",
    "1.0",
    "'quoted'",
    "\"double\"",
];

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with LUUNA_BEACON_REFERENCE=<clone> LUUNA_BEACON_GO=<go> cargo test -p luuna-beacon --locked --test parity_authoring -- --ignored"]
fn frontmatter_yaml_scalars_match_go_byte_for_byte() {
    let mut case = ready();
    let mut lines = Vec::new();
    // Each corpus string as a value, one prompt per string, so that a
    // difference names its value.
    for (index, text) in SCALAR_CORPUS.iter().enumerate() {
        let entries = vec![("v".to_owned(), Value::String((*text).to_owned()))];
        lines.push(fm_record(
            &format!("{:x}", 0x100 + index),
            &format!("value-{index}"),
            &json_object(&entries),
        ));
    }
    // Each corpus string as a key, all in one prompt (key order included).
    let keys: Vec<(String, Value)> = SCALAR_CORPUS
        .iter()
        .filter(|text| !["name", "description"].contains(text))
        .map(|text| ((*text).to_owned(), json!("v")))
        .collect();
    lines.push(fm_record("200", "keys", &json_object(&keys)));
    // A 300-character line, a 130-byte key and multi-line keys.
    let long_line = "word ".repeat(60);
    let long_key = "k".repeat(130);
    lines.push(fm_record(
        "201",
        "long",
        &json_object(&[
            ("line".to_owned(), Value::String(long_line)),
            (long_key, json!("v")),
            ("two\nlines".to_owned(), json!("v")),
            ("ends\n".to_owned(), json!(["x"])),
            ("x".repeat(128), json!("exactly 128")),
            ("y".repeat(129), json!({"nested": "129"})),
        ]),
    ));
    store(&mut case, &lines);
    for (index, text) in SCALAR_CORPUS.iter().enumerate() {
        labelled(&format!("corpus value {text:?}"), || {
            assert_render_text(&case, &format!("value-{index}"))
        });
    }
    assert_render_text(&case, "keys");
    assert_render_text(&case, "long");
    case.step(emit(emit_all()));
    assert_outside(&case, "emitted scalar corpus");
}

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with LUUNA_BEACON_REFERENCE=<clone> LUUNA_BEACON_GO=<go> cargo test -p luuna-beacon --locked --test parity_authoring -- --ignored"]
fn frontmatter_yaml_numbers_and_floats_match_go() {
    // FormatFloat('g', -1, 64) thresholds and -0, from JSON numbers as the
    // store holds them (every number is a float64 in Go).
    let numbers = [
        "0",
        "-0",
        "1",
        "-1",
        "1.5",
        "1e5",
        "1e6",
        "123456",
        "1e20",
        "1e21",
        "123456789",
        "1e-4",
        "1e-5",
        "0.30000000000000004",
        "12345678901234567890",
        "9007199254740993",
        "1.7976931348623157e308",
        "5e-324",
        "100000",
        "1000000",
        "0.000001",
        "0.0000001",
        "2.5",
        "-2.5e-7",
        "1E2",
        "18446744073709551615",
    ];
    let mut case = ready();
    let mut lines = Vec::new();
    for (index, number) in numbers.iter().enumerate() {
        lines.push(fm_record(
            &format!("{:x}", 0x300 + index),
            &format!("n{index}"),
            &format!(r#"{{"n":{number}}}"#),
        ));
    }
    let all: Vec<String> = numbers
        .iter()
        .enumerate()
        .map(|(index, number)| format!(r#""k{index:02}":{number}"#))
        .collect();
    lines.push(fm_record("3ff", "all", &format!("{{{}}}", all.join(","))));
    lines.push(fm_record(
        "3fe",
        "nested",
        &format!(
            r#"{{"list":[{}],"map":{{"a":-0,"b":1e21}}}}"#,
            numbers.join(",")
        ),
    ));
    store(&mut case, &lines);
    for (index, number) in numbers.iter().enumerate() {
        labelled(&format!("number {number}"), || {
            assert_render_text(&case, &format!("n{index}"))
        });
    }
    assert_render_text(&case, "all");
    assert_render_text(&case, "nested");
}

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with LUUNA_BEACON_REFERENCE=<clone> LUUNA_BEACON_GO=<go> cargo test -p luuna-beacon --locked --test parity_authoring -- --ignored"]
fn frontmatter_yaml_collections_keys_and_order_match_go() {
    // GD43: sorter.go's key order; ordinary keys compared against Go.
    let key_set = [
        "a10", "a2", "a_b", "A", "b", "1", "10", "9", "item-1", "item_1", "a1b", "a01", "a1", "B",
        "a", "_", "-", "z9", "z10", "a.b", "ä", "Z",
    ];
    let mut case = ready();
    let keys: Vec<(String, Value)> = key_set
        .iter()
        .map(|key| ((*key).to_owned(), json!(*key)))
        .collect();
    let collections = json!({
        "empty_map": {},
        "empty_list": [],
        "list": ["a", 1, true, null, {"x": 1}, [], {}, ["nested", ["deeper"]]],
        "map": {"z": 1, "a": {"b": {"c": [1, 2]}}, "m": []},
        "list_of_maps": [{"b": 1, "a": 2}, {"only": "one"}],
        "null": null,
        "bool": false,
        "multi": ["line\none", "line\ntwo\n"],
    });
    store(
        &mut case,
        &[
            fm_record("401", "keys", &json_object(&keys)),
            fm_record("402", "collections", &collections.to_string()),
            fm_record(
                "403",
                "nested_keys",
                &json!({"outer": Value::Object(keys.iter().cloned().collect())}).to_string(),
            ),
            fm_record("404", "empty", "{}"),
            fm_record("405", "nullfm", "null"),
        ],
    );
    for name in ["keys", "collections", "nested_keys", "empty", "nullfm"] {
        assert_render_text(&case, name);
    }
    case.step(emit(emit_all()));
    assert_outside(&case, "emitted collections");
}

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with LUUNA_BEACON_REFERENCE=<clone> LUUNA_BEACON_GO=<go> cargo test -p luuna-beacon --locked --test parity_authoring -- --ignored"]
fn frontmatter_yaml_line_breaks_and_unicode_match_go() {
    // Single-quoted style with \r, U+0085, U+2028, U+2029; plain scalars
    // with U+00A0; literal blocks with indent and chomping indicators.
    let values = [
        "carriage\rreturn",
        "next\u{85}line",
        "line\u{2028}separator",
        "para\u{2029}separator",
        "no\u{a0}break",
        "\u{a0}leading nbsp",
        "trailing nbsp\u{a0}",
        "multi\nline",
        "multi\nline\n",
        "multi\nline\n\n",
        "\nleading newline",
        "  indented\nfirst",
        "trailing space \nline",
        "tab\tinside\nlines",
        "crlf\r\nlines",
        "unicode é\nsecond 漢字",
        "\u{1f600}\n\u{1f601}",
        "ends with tab\t",
        "\u{1}\nctl",
        "#comment-like\nx",
        "- dash\nlist-like",
        "key: value\nmap-like",
    ];
    let mut case = ready();
    let mut lines = Vec::new();
    for (index, text) in values.iter().enumerate() {
        lines.push(fm_record(
            &format!("{:x}", 0x500 + index),
            &format!("t{index}"),
            &json_object(&[("v".to_owned(), Value::String((*text).to_owned()))]),
        ));
    }
    let keys: Vec<(String, Value)> = values
        .iter()
        .map(|text| ((*text).to_owned(), json!(*text)))
        .collect();
    lines.push(fm_record("5ff", "both", &json_object(&keys)));
    store(&mut case, &lines);
    for (index, text) in values.iter().enumerate() {
        labelled(&format!("value {text:?}"), || {
            assert_render_text(&case, &format!("t{index}"))
        });
    }
    assert_render_text(&case, "both");
}

// ---- emit ---------------------------------------------------------------------

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with LUUNA_BEACON_REFERENCE=<clone> LUUNA_BEACON_GO=<go> cargo test -p luuna-beacon --locked --test parity_authoring -- --ignored"]
fn emit_writes_markdown_and_ts_files_like_go() {
    let mut case = ready();
    case.step(create("a", &[("Task", "plain")]));
    case.step(create_with(CreatePrompt {
        name: "b".into(),
        description: "custom file".into(),
        emit_as: "b-custom.md".into(),
        sections: vec![sec("One", "1"), sec("Two", "2")],
        frontmatter: pairs(&[("model", "x"), ("temperature", "0.5")]),
        ..Default::default()
    }));
    case.step(create_with(CreatePrompt {
        name: "c-ts".into(),
        description: "ts export".into(),
        emit_as: "c.ts".into(),
        sections: vec![sec("Task", "use `code` and ${vars}")],
        ..Default::default()
    }));
    case.step(create_with(CreatePrompt {
        name: "d".into(),
        emit_dir: "docs/sub".into(),
        sections: vec![sec("Task", "d")],
        ..Default::default()
    }));
    case.step(create_with(CreatePrompt {
        name: "e".into(),
        status: "draft".into(),
        sections: vec![sec("Task", "draft")],
        ..Default::default()
    }));
    case.step(create_with(CreatePrompt {
        name: "f".into(),
        emit_as: "nested/dir/f.md".into(),
        extends: "a".into(),
        sections: vec![sec("Extra", "f")],
        ..Default::default()
    }));
    let seen = case.step(emit(emit_all()));
    assert_eq!(seen.code, 0);
    assert_outside(&case, "emit --all");
    case.step(emit(emit_one("e")));
    assert_outside(&case, "emit of a draft by name");
    case.step(emit(emit_one("b")));
    case.step(emit(emit_one("ghost")));
    case.step(update(
        "a",
        UpdatePrompt {
            sections: vec![sec("Task", "changed")],
            ..up()
        },
    ));
    case.step(emit(emit_all()));
    assert_outside(&case, "emit after an update");
    case.step(archive("d"));
    case.step(emit(emit_all()));
    case.step(emit(emit_one("d")));
    assert_outside(&case, "emit of an archived prompt by name");
}

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with LUUNA_BEACON_REFERENCE=<clone> LUUNA_BEACON_GO=<go> cargo test -p luuna-beacon --locked --test parity_authoring -- --ignored"]
fn emit_dry_run_check_and_force_like_go() {
    // GD14: files and stale names come in map order; the hook sorts them.
    let mut case = ready();
    case.step(create("a", &[("Task", "a")]));
    case.step(create("b", &[("Task", "b")]));
    case.step(create_with(CreatePrompt {
        name: "draft".into(),
        status: "draft".into(),
        ..Default::default()
    }));
    let check = || {
        let mut options = emit_all();
        options.check = true;
        options
    };
    let seen = case.step(emit(check()));
    assert_eq!(seen.code, 1, "a check with stale files fails");
    let mut dry = emit_all();
    dry.dry_run = true;
    case.step(emit(dry));
    assert!(
        !case.lib_root().join("agents").exists(),
        "a dry run writes nothing"
    );
    assert_outside(&case, "after the dry run");
    case.step(emit(emit_all()));
    let seen = case.step(emit(check()));
    assert_eq!(seen.code, 0);
    let seen = case.step(emit(emit_all()));
    assert_eq!(seen.raw[0]["files"][0]["skipped"], json!(true));
    let mut force = emit_all();
    force.force = true;
    case.step(emit(force));
    case.step(write_file("agents/a.md", b"edited by hand\n"));
    case.step(emit(check()));
    let mut check_one = emit_one("b");
    check_one.check = true;
    case.step(emit(check_one));
    let mut dry_one = emit_one("a");
    dry_one.dry_run = true;
    case.step(emit(dry_one));
    let mut dry_check = emit_all();
    dry_check.dry_run = true;
    dry_check.check = true;
    case.step(emit(dry_check));
    case.step(emit(emit_one("a")));
    assert_outside(&case, "after re-emitting the edited file");
    case.step(emit(check()));
    case.step(create_with(CreatePrompt {
        name: "dir-target".into(),
        emit_dir: "dirs".into(),
        ..Default::default()
    }));
    case.step(mkdir("dirs/dir-target.md"));
    case.step(emit(check()));
}

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with LUUNA_BEACON_REFERENCE=<clone> LUUNA_BEACON_GO=<go> cargo test -p luuna-beacon --locked --test parity_authoring -- --ignored"]
fn emit_routes_by_prompt_dir_tags_default_and_legacy_config() {
    // Inputs with one possible answer only (GD15 and GD16 are pinned by
    // authoring_api::map_ordered_outputs_are_deterministic).
    let prompts = || {
        vec![
            create_with(CreatePrompt {
                name: "tagged".into(),
                tags: strings(&["doc"]),
                ..Default::default()
            }),
            create_with(CreatePrompt {
                name: "untagged".into(),
                ..Default::default()
            }),
            create_with(CreatePrompt {
                name: "own-dir".into(),
                tags: strings(&["doc"]),
                emit_dir: "custom".into(),
                ..Default::default()
            }),
            create_with(CreatePrompt {
                name: "other-tag".into(),
                tags: strings(&["zzz", "same"]),
                ..Default::default()
            }),
        ]
    };
    let configs: [&[u8]; 6] = [
        // Targets: a tag route and one default.
        b"project: luuna-beacon\nversion: \"1\"\ntargets:\n  docs:\n    dir: out/docs\n    tags: [doc]\n  main:\n    dir: out/main\n    default: true\n",
        // GD18: default given as the string "true".
        b"project: luuna-beacon\nversion: \"1\"\ntargets:\n  main:\n    dir: string-default\n    default: \"true\"\n  docs:\n    tags: [doc, same]\n",
        // No default target: the fallback `agents`.
        b"project: luuna-beacon\nversion: \"1\"\ntargets:\n  docs:\n    dir: only-docs\n    tags: [doc]\n  skipped: not a map\n",
        // Legacy emitDir and emitDirByTag, migrated in memory.
        b"project: luuna-beacon\nversion: \"1\"\nemitDir: legacy\nemitDirByTag:\n  doc: legacy/docs!\n  same: legacy\n",
        // Legacy emitDirByTag alone: the default directory is `agents`.
        b"emitDirByTag:\n  doc: by-tag\n",
        // GD19: a `targets` that is not a mapping falls through to legacy.
        b"project: luuna-beacon\nversion: \"1\"\ntargets: [x]\nemitDir: fell-through\n",
    ];
    for (index, config) in configs.iter().enumerate() {
        labelled(&format!("config {index}"), || {
            let mut case = ready();
            case.step(write_file(CONFIG, config));
            case.steps(prompts());
            case.step(emit(emit_all()));
            assert_outside(&case, &format!("routing with config {index}"));
            let mut dry = emit_one("other-tag");
            dry.dry_run = true;
            case.step(emit(dry));
        });
    }
}

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with LUUNA_BEACON_REFERENCE=<clone> LUUNA_BEACON_GO=<go> cargo test -p luuna-beacon --locked --test parity_authoring -- --ignored"]
fn emit_out_and_out_dir_like_go() {
    // RD6: a relative --out or --out-dir is resolved against the project
    // root (the folder Go runs in). GD35: --out with several prompts writes
    // them all to one path; the two prompts render the same bytes (their
    // frontmatter `name` overrides), so the result has one possible answer.
    let mut case = ready();
    for name in ["a", "b"] {
        case.step(create_with(CreatePrompt {
            name: name.into(),
            sections: vec![sec("Task", "same body")],
            frontmatter: pairs(&[("name", "shared")]),
            ..Default::default()
        }));
    }
    let with = |out: Option<&str>, out_dir: Option<&str>| {
        let mut options = emit_one("a");
        options.out = out.map(str::to_owned);
        options.out_dir = out_dir.map(str::to_owned);
        options
    };
    case.step(emit(with(Some("out/a-out.md"), None)));
    assert_outside(&case, "relative --out");
    case.step(emit(with(None, Some("out-dir/deeper"))));
    assert_outside(&case, "relative --out-dir");
    case.step(emit(with(Some("both.md"), Some("ignored-dir"))));
    assert_outside(&case, "--out wins over --out-dir");
    case.step(emit(with(Some(""), None)));
    case.step(emit(with(None, Some(""))));
    assert_outside(&case, "an empty --out or --out-dir is absent");
    let go_root = case.go_root().to_path_buf();
    let lib_root = case.lib_root().to_path_buf();
    case.step(emit_absolute(
        emit_one("a"),
        &go_root,
        &lib_root,
        Some("abs/a.md"),
        None,
    ));
    case.step(emit_absolute(
        emit_one("b"),
        &go_root,
        &lib_root,
        None,
        Some("abs-dir"),
    ));
    assert_outside(&case, "absolute --out and --out-dir");
    let mut all_out = emit_all();
    all_out.out = Some("one-file.md".to_owned());
    case.step(emit(all_out));
    assert_outside(&case, "--all with one --out");
    let mut dry = emit_all();
    dry.dry_run = true;
    dry.out = Some("dry.md".to_owned());
    case.step(emit(dry));
    let mut check = emit_all();
    check.check = true;
    check.out = Some("one-file.md".to_owned());
    case.step(emit(check));
    let mut check_dir = emit_all();
    check_dir.check = true;
    check_dir.out_dir = Some("out-dir/deeper".to_owned());
    case.step(emit(check_dir));
}

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with LUUNA_BEACON_REFERENCE=<clone> LUUNA_BEACON_GO=<go> cargo test -p luuna-beacon --locked --test parity_authoring -- --ignored"]
fn emit_errors_match_go() {
    // RD10: operating-system errors keep Go's `<op> <path>: ` prefix.
    // GD36: emit creates the target's folder before it compares or writes.
    // (Which prompts an --all run has written when a later one fails depends
    // on Go's map order, so that half is not compared.)
    let mut case = Case::new();
    case.step_os_error(emit(emit_all()), "open <root>/.luuna/beacon/config.yaml: ");
    case.step(emit(emit_one("ghost")));
    case.step(Step::Init);
    case.step(emit(emit_one("ghost")));
    case.step(create("a", &[("Task", "a")]));
    case.step(write_file("agents", b"a file where a folder should be\n"));
    case.step_os_error(emit(emit_one("a")), "mkdir <root>/agents: ");
    let mut dry = emit_one("a");
    dry.dry_run = true;
    case.step(emit(dry));
    let mut check = emit_one("a");
    check.check = true;
    case.step(emit(check));
    case.step(create_with(CreatePrompt {
        name: "b".into(),
        emit_dir: "dirs".into(),
        ..Default::default()
    }));
    case.step(mkdir("dirs/b.md"));
    case.step_os_error(emit(emit_one("b")), "open <root>/dirs/b.md: ");
    case.step(create_with(CreatePrompt {
        name: "c".into(),
        emit_dir: "made/by/emit".into(),
        ..Default::default()
    }));
    case.step(emit(emit_one("c")));
    case.step(emit(emit_one("c")));
    assert_outside(&case, "folders made by emit");
    case.step(write_file(
        PROMPTS,
        [
            prompt_record(
                "luuna-beacon-f1",
                "orphan",
                1,
                STAMP,
                &[("S", "x")],
                json!({"extends": "ghost"}),
            ),
            prompt_record(
                "luuna-beacon-f2",
                "loop1",
                1,
                STAMP,
                &[("S", "x")],
                json!({"extends": "loop2"}),
            ),
            prompt_record(
                "luuna-beacon-f3",
                "loop2",
                1,
                STAMP,
                &[("S", "x")],
                json!({"extends": "loop1"}),
            ),
            prompt_record(
                "luuna-beacon-f4",
                "pinned",
                1,
                STAMP,
                &[("S", "x")],
                json!({"pinned": 7}),
            ),
        ]
        .concat()
        .as_bytes(),
    ));
    case.step(emit(emit_one("orphan")));
    case.step(emit(emit_one("loop1")));
    case.step(emit(emit_one("pinned")));
    case.step(write_file(CONFIG, b"project: [\n"));
    case.step_os_error(emit(emit_one("orphan")), "yaml: ");
    assert_outside(&case, "after the errors");
}

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with LUUNA_BEACON_REFERENCE=<clone> LUUNA_BEACON_GO=<go> cargo test -p luuna-beacon --locked --test parity_authoring -- --ignored"]
fn emit_escapes_the_root_like_go() {
    // GD27-GD31: committed content steers emit outside the project root;
    // GD34: an existing file inside the root but outside .luuna/ is
    // overwritten. Copied, so both sides write the same files; each side's
    // whole temporary folder is compared.
    let mut pair = Pair::nested();
    pair.step(Step::Init);
    pair.raw(write_file("notes.txt", b"the user's own notes\n"));
    pair.step(create_with(CreatePrompt {
        name: "dir-up".into(),
        emit_dir: "../escaped-dir".into(),
        ..Default::default()
    }));
    pair.step(create_with(CreatePrompt {
        name: "as-up".into(),
        emit_as: "../../escaped-as.md".into(),
        ..Default::default()
    }));
    pair.step(create_with(CreatePrompt {
        name: "../../escaped-name".into(),
        ..Default::default()
    }));
    pair.step(create_with(CreatePrompt {
        name: "inside".into(),
        emit_dir: ".".into(),
        emit_as: "notes.txt".into(),
        sections: vec![sec("Task", "replaces the notes")],
        ..Default::default()
    }));
    pair.step(emit(emit_all()));
    pair.compare_outside("prompt fields that leave the root");
    let notes = fs::read_to_string(pair.lib_root.join("notes.txt")).expect("notes.txt");
    assert!(notes.contains("replaces the notes"), "GD34: {notes:?}");

    let mut pair = Pair::nested();
    pair.step(Step::Init);
    pair.raw(write_file(
        CONFIG,
        b"project: luuna-beacon\nversion: \"1\"\ntargets:\n  out:\n    dir: ../escaped-target\n    tags: [esc]\n  main:\n    dir: agents\n    default: true\n",
    ));
    pair.step(create_with(CreatePrompt {
        name: "tagged".into(),
        tags: strings(&["esc"]),
        ..Default::default()
    }));
    pair.step(emit(emit_all()));
    pair.compare_outside("a config target that leaves the root");

    let mut pair = Pair::nested();
    pair.step(Step::Init);
    pair.raw(write_file(
        CONFIG,
        b"emitDir: ../legacy-up\nemitDirByTag:\n  t: ../legacy-tag\n",
    ));
    pair.step(create("plain", &[("Task", "x")]));
    pair.step(create_with(CreatePrompt {
        name: "tagged".into(),
        tags: strings(&["t"]),
        ..Default::default()
    }));
    pair.step(emit(emit_all()));
    pair.compare_outside("legacy keys that leave the root");
}

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with LUUNA_BEACON_REFERENCE=<clone> LUUNA_BEACON_GO=<go> cargo test -p luuna-beacon --locked --test parity_authoring -- --ignored"]
fn emit_follows_committed_symlinks_like_go() {
    // GD32: emit writes through a linked folder and a linked file, and
    // --check reads through them.
    let mut pair = Pair::nested();
    pair.step(Step::Init);
    pair.raw(mkdir("../linked-dir"));
    pair.raw(symlink_step("agents", "../linked-dir"));
    pair.raw(write_file("../linked-file.md", b"before\n"));
    pair.raw(mkdir("out"));
    pair.raw(symlink_step("out/b.md", "../../linked-file.md"));
    pair.step(create("a", &[("Task", "through a linked folder")]));
    pair.step(create_with(CreatePrompt {
        name: "b".into(),
        emit_dir: "out".into(),
        sections: vec![sec("Task", "through a linked file")],
        ..Default::default()
    }));
    let mut check = emit_all();
    check.check = true;
    pair.step(emit(check));
    pair.step(emit(emit_all()));
    pair.compare_outside("emit through committed links");
    let mut check = emit_all();
    check.check = true;
    let seen = pair.step(emit(check));
    assert_eq!(seen.code, 0, "--check reads through the links");
    pair.step(emit(emit_all()));
    pair.compare_outside("a second emit through the links");
}

// ---- import -------------------------------------------------------------------

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with LUUNA_BEACON_REFERENCE=<clone> LUUNA_BEACON_GO=<go> cargo test -p luuna-beacon --locked --test parity_authoring -- --ignored"]
fn import_splits_frontmatter_and_sections_like_go() {
    // GD21 (copied part: a first-line thematic break without a closing line)
    // and GD22 (copied part: `## ` inside a code fence is a heading). Only
    // whitespace is dropped (AS21).
    let sources: [(&str, &[u8], &[&str], bool); 10] = [
        (
            "with-fm",
            b"---\ndescription: Imported description\ntitle: T\n---\n## One\ntext one\n\n## Two\n\ntext two\n",
            &["t1", "t2"],
            false,
        ),
        ("no-heading", b"just a body\n\nwith lines\n", &[], false),
        ("no-split", b"## A\na\n## B\nb\n", &[], true),
        (
            "fence",
            b"## A\n```\n## not a heading\n```\nafter\n",
            &[],
            false,
        ),
        ("rule", b"---\nplain text after a rule\n", &[], false),
        ("blank-lead", b"\n\n  \n## A\n\n\nx\n\n", &[], false),
        ("same-name", b"---\nname: same-name\nk: v\n---\n## A\nx\n", &[], false),
        ("empty", b"", &[], false),
        ("heading-only", b"## Lonely\n", &[], false),
        (
            "spaces",
            b"##  Spaced name  \nbody\n## \nunnamed\n###deeper\n",
            &[],
            false,
        ),
    ];
    let mut case = ready();
    for (name, bytes, tags, no_split) in sources {
        let path = format!("src/{name}.md");
        case.step(write_file(&path, bytes));
        let seen = case.step(import(&path, name, tags, no_split));
        assert_eq!(seen.code, 0, "import {name}");
        case.step(show(name));
    }
    case.step(list());
    let seen = case.step(show("with-fm"));
    assert_eq!(
        seen.raw[0]["prompt"]["description"],
        json!("Imported description")
    );
    case.step(emit(emit_all()));
    assert_outside(&case, "emit of imported prompts");
}

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with LUUNA_BEACON_REFERENCE=<clone> LUUNA_BEACON_GO=<go> cargo test -p luuna-beacon --locked --test parity_authoring -- --ignored"]
fn import_frontmatter_typing_matches_go() {
    // yaml.v3's typing into `any`, then encoding/json into the store; and
    // yaml.v3's reader refusing control characters (exact text).
    let typed = "---\nint: 42\nneg: -7\nhex: 0x1F\noctal: 0o17\nbinary: 0b101\nunderscore: 1_000\nfloat: 1.5\nexp: 1e3\ninf_text: .inf_x\nbool: true\nyes: yes\nnul: null\ntilde: ~\nts: 2001-12-14T21:59:43.1Z\nspaced_ts: 2001-12-14 21:59:43.10 -5\ndate: 2001-12-14\nstr: \"quoted\"\nsingle: 'single'\nlist: [1, two, 3.0]\nmap: {a: 1, b: [x]}\nnested:\n  deep:\n    deeper: v\nbin: !!binary aGVsbG8=\nanchor: &a {x: 1}\nalias: *a\nmerged:\n  <<: {m: 1}\n  own: 2\nexact: 9007199254740992\nlong: \"0.1000000000000000055511151231257827021181583404541015625001\"\nblock: |\n  line one\n  line two\nfolded: >\n  folded\n  text\n---\n## S\nbody\n";
    let mut case = ready();
    case.step(write_file("src/typed.md", typed.as_bytes()));
    case.step(import("src/typed.md", "typed", &[], false));
    case.step(show("typed"));
    assert_render_text(&case, "typed");
    for (index, bad) in [
        &b"---\nk: a\x00b\n---\n## S\nb\n"[..],
        &b"---\nk: a\x01b\n---\n## S\nb\n"[..],
        &b"---\nk: a\x7fb\n---\n## S\nb\n"[..],
    ]
    .iter()
    .enumerate()
    {
        let path = format!("src/control-{index}.md");
        case.step(write_file(&path, bad));
        let seen = case.step(import(&path, &format!("control-{index}"), &[], false));
        assert_eq!(seen.code, 1, "control character {index}");
    }
    case.step(write_file(
        "src/bom-inside.md",
        "---\n\u{feff}k: v\n---\n## S\nb\n".as_bytes(),
    ));
    case.step(import("src/bom-inside.md", "bom-inside", &[], false));
    case.step(write_file("src/strings.md", b"---\nempty: \"\"\nspace: \" \"\nunicode: \"\\u00e9\\U0001F600\"\nescapes: \"a\\tb\\nc\"\n---\n## S\nb\n"));
    case.step(import("src/strings.md", "strings", &[], false));
    case.step(show("strings"));
    case.step(list());
}

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with LUUNA_BEACON_REFERENCE=<clone> LUUNA_BEACON_GO=<go> cargo test -p luuna-beacon --locked --test parity_authoring -- --ignored"]
fn import_errors_match_go() {
    // RD7: yaml-rust2's syntax-error texts after `invalid frontmatter:
    // yaml: `. RD10: operating-system errors keep Go's `<op> <path>: `.
    let mut case = Case::new();
    case.step(write_file("src/a.md", b"## A\nx\n"));
    case.step_os_error(
        import("src/a.md", "a", &[], false),
        "open <root>/.luuna/beacon/config.yaml: ",
    );
    case.step(Step::Init);
    case.step_os_error(
        import("src/missing.md", "a", &[], false),
        "open src/missing.md: ",
    );
    case.step(mkdir("src/folder.md"));
    case.step_os_error(
        import("src/folder.md", "a", &[], false),
        "read src/folder.md: ",
    );
    let seen = case.step(import("src/a.md", "", &[], false));
    assert_eq!(seen.error.as_deref(), Some("--name is required"));
    case.step(import("src/a.md", "a", &[], false));
    let seen = case.step(import("src/a.md", "a", &[], false));
    assert_eq!(
        seen.error.as_deref(),
        Some("prompt name \"a\" already exists")
    );
    case.step(archive("a"));
    case.step(import("src/a.md", "a", &[], false));
    case.step(write_file(
        "src/syntax.md",
        b"---\nk: [unclosed\n---\n## S\nb\n",
    ));
    case.step_os_error(
        import("src/syntax.md", "syntax", &[], false),
        "invalid frontmatter: yaml: ",
    );
    case.step(write_file("src/dup.md", b"---\na: 1\na: 2\n---\n## S\nb\n"));
    case.step(import("src/dup.md", "dup", &[], false));
    case.step(write_file("src/seq.md", b"---\n- a\n- b\n---\n## S\nb\n"));
    case.step(import("src/seq.md", "seq", &[], false));
    case.step(write_file(
        "src/scalar.md",
        b"---\njust text\n---\n## S\nb\n",
    ));
    case.step(import("src/scalar.md", "scalar", &[], false));
    case.step(write_file(
        "src/badkeys.md",
        b"---\nk: {true: a}\n---\n## S\nb\n",
    ));
    case.step(import("src/badkeys.md", "badkeys", &[], false));
    case.step(write_file("src/nan.md", b"---\nk: .nan\n---\n## S\nb\n"));
    case.step(import("src/nan.md", "nan", &[], false));
    case.step(write_file("src/merge.md", b"---\n<<: 1\n---\n## S\nb\n"));
    case.step(import("src/merge.md", "merge", &[], false));
    case.step(write_file(
        "src/self.md",
        b"---\na: &x [*x]\n---\n## S\nb\n",
    ));
    case.step(import("src/self.md", "self", &[], false));
    case.step(write_file(PROMPTS, b"{broken\n"));
    case.step(import("src/a.md", "after-broken", &[], false));
    case.step(write_file(CONFIG, b"project: [\n"));
    case.step(write_file(PROMPTS, b""));
    case.step_os_error(import("src/a.md", "bad-config", &[], false), "yaml: ");
}

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with LUUNA_BEACON_REFERENCE=<clone> LUUNA_BEACON_GO=<go> cargo test -p luuna-beacon --locked --test parity_authoring -- --ignored"]
fn import_then_emit_does_not_round_trip_like_go() {
    // GD24: render appends a newline to a body that already ends in one, so
    // emit -> import -> emit grows the file.
    let mut case = ready();
    case.step(create_with(CreatePrompt {
        name: "orig".into(),
        description: "round trip".into(),
        sections: vec![sec("One", "first"), sec("Two", "second")],
        frontmatter: pairs(&[("k", "v")]),
        ..Default::default()
    }));
    case.step(emit(emit_one("orig")));
    case.step(import("agents/orig.md", "copy", &[], false));
    case.step(show("copy"));
    case.step(emit(emit_one("copy")));
    assert_outside(&case, "emit after import");
    case.step(import("agents/copy.md", "copy2", &[], false));
    case.step(emit(emit_one("copy2")));
    assert_outside(&case, "a second round");
    let first = fs::read(case.lib_root().join("agents/orig.md")).expect("orig.md");
    let second = fs::read(case.lib_root().join("agents/copy.md")).expect("copy.md");
    assert_ne!(first, second, "GD24: the round trip changes the file");
}

// ---- config -------------------------------------------------------------------

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with LUUNA_BEACON_REFERENCE=<clone> LUUNA_BEACON_GO=<go> cargo test -p luuna-beacon --locked --test parity_authoring -- --ignored"]
fn config_schema_matches_go() {
    let mut case = Case::new();
    let seen = case.step(config_schema());
    assert_eq!(seen.code, 0);
    assert_eq!(seen.raw[0], luuna_beacon::config_schema());
    case.step(Step::Init);
    case.step(config_schema());
    case.step(write_file(CONFIG, b"project: [\n"));
    case.step(config_schema());
}

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with LUUNA_BEACON_REFERENCE=<clone> LUUNA_BEACON_GO=<go> cargo test -p luuna-beacon --locked --test parity_authoring -- --ignored"]
fn config_show_json_and_path_like_go() {
    let mut case = Case::new();
    case.step_os_error(config_show(), "open <root>/.luuna/beacon/config.yaml: ");
    case.step_os_error(
        config_show_path("project"),
        "open <root>/.luuna/beacon/config.yaml: ",
    );
    case.step(Step::Init);
    case.step(config_show());
    assert_config_show_text(&case);
    for path in [
        "project",
        "version",
        "targets",
        "targets.default",
        "targets.default.dir",
        "targets.default.default",
        "nope",
        "targets.nope.x",
        "project.x",
        "targets.",
        ".",
    ] {
        case.step(config_show_path(path));
        assert_config_path_text(&case, path);
    }
    let rich = "# a comment Go prints as it is\nproject: luuna-beacon\nversion: \"1\"\ntargets:\n  default:\n    dir: agents\n    default: true\n    tags: [a, b]\n  docs:\n    dir: \"docs out\"\n    when: 2001-12-14\n    stamp: 2001-12-14T21:59:43.1Z\n    n: 1.5\n    big: 18446744073709551615\n    neg: -9223372036854775808\n    list: [1, two, {three: 3}]\n    empty_map: {}\n    empty_list: []\n    nul: null\n    text: |\n      multi\n      line\n    quoted: 'it''s'\n    bin: !!binary aGVsbG8=\n    int_keys: {1: a, 2: b}\n";
    case.step(write_file(CONFIG, rich.as_bytes()));
    case.step(config_show());
    assert_config_show_text(&case);
    for path in [
        "targets",
        "targets.default.tags",
        "targets.docs",
        "targets.docs.dir",
        "targets.docs.when",
        "targets.docs.stamp",
        "targets.docs.n",
        "targets.docs.big",
        "targets.docs.neg",
        "targets.docs.list",
        "targets.docs.list.0",
        "targets.docs.empty_map",
        "targets.docs.empty_list",
        "targets.docs.nul",
        "targets.docs.text",
        "targets.docs.quoted",
        "targets.docs.bin",
        "targets.docs.int_keys",
    ] {
        case.step(config_show_path(path));
        assert_config_path_text(&case, path);
    }
}

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with LUUNA_BEACON_REFERENCE=<clone> LUUNA_BEACON_GO=<go> cargo test -p luuna-beacon --locked --test parity_authoring -- --ignored"]
fn config_show_values_go_cannot_encode_print_nothing() {
    // GD10: writeJSON drops encoding/json's error, so Go prints one empty
    // line (no document) and exits 0. YamlValue::to_go_json returns the
    // error instead; the step yields no document.
    let mut case = ready();
    case.step(write_file(
        CONFIG,
        b"project: luuna-beacon\nversion: \"1\"\ntargets:\n  t:\n    dir: d\n    nan: .nan\n    inf: .inf\n    ninf: -.inf\n    boolkey: {true: a}\n    floatkey: {1.5: a}\n    nullkey: {~: a}\n    fine: 1\n",
    ));
    let seen = case.step(config_show());
    assert_eq!(seen.code, 0);
    assert!(seen.raw.is_empty(), "Go prints no document: {:?}", seen.raw);
    for (path, printable) in [
        ("targets.t.nan", false),
        ("targets.t.inf", false),
        ("targets.t.ninf", false),
        ("targets.t.boolkey", false),
        ("targets.t.floatkey", false),
        ("targets.t.nullkey", false),
        ("targets.t.fine", true),
    ] {
        let seen = case.step(config_show_path(path));
        assert_eq!(seen.code, 0, "{path}");
        assert_eq!(!seen.raw.is_empty(), printable, "{path}: {:?}", seen.raw);
        let value = case.beacon().config_get(path).expect("config_get");
        assert_eq!(value.to_go_json().is_ok(), printable, "{path}");
        assert_config_path_text(&case, path);
    }
    assert_config_show_text(&case);
    // The set echo prints nothing either when the value cannot be encoded.
    let seen = case.step(config_set("targets.t.more", ".nan"));
    assert!(seen.raw.is_empty(), "the set echo of NaN: {:?}", seen.raw);
    assert_config_show_text(&case);
}

/// The files of config_set_rewrites_the_file_like_go: comments, anchors and
/// merges, unsorted keys, flow style, quoting and block text.
const REWRITE_FILES: [&str; 5] = [
    "# top comment\nproject: luuna-beacon # trailing comment\nversion: \"1\"\n# before targets\ntargets:\n  zeta:\n    dir: z\n  alpha:\n    dir: a # inline\n    default: true\n",
    "project: luuna-beacon\nversion: \"1\"\ntargets:\n  base: &b\n    dir: shared\n    tags: [x, y]\n  copy:\n    <<: *b\n    default: true\n  alias: *b\n  merged_list:\n    <<: [{dir: first}, {dir: second, tags: [z]}]\n",
    "version: '1'\nproject: \"luuna-beacon\"\ntargets: {b: {dir: b}, a: {dir: a, tags: [t1, t2]}, A: {dir: upper}, a10: {dir: x}, a2: {dir: y}}\n",
    "project: luuna-beacon\nversion: \"1\"\ntargets:\n  x:\n    dir: x\n    note: |\n      first line\n      second line\n    folded: >\n      folded\n      text\n    long: \"a very long string that goes on and on and on and on and on and on and on and on and on and on past eighty columns\"\n    special: \"colon: and # hash\"\n    unicode: \"é 漢字 \\u2028\"\n    empty: \"\"\n    yes_text: \"yes\"\n    number_text: \"012\"\n",
    "---\nproject: luuna-beacon\nversion: \"1\"\ntargets:\n  default:\n    dir: agents\n    default: true\n...\n",
];

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with LUUNA_BEACON_REFERENCE=<clone> LUUNA_BEACON_GO=<go> cargo test -p luuna-beacon --locked --test parity_authoring -- --ignored"]
fn config_set_rewrites_the_file_like_go() {
    // GD17 (copied part): the whole file is rewritten by yaml.Marshal;
    // comments are dropped and the form changes, every value reads the same.
    for (index, file) in REWRITE_FILES.iter().enumerate() {
        labelled(&format!("file {index}"), || {
            let mut case = ready();
            case.step(write_file(CONFIG, file.as_bytes()));
            case.step(config_set("targets.new.dir", "fresh"));
            assert_config_show_text(&case);
            case.step(config_show());
            case.step(config_unset("targets.new"));
            assert_config_show_text(&case);
            case.step(config_set("version", "\"2\""));
            case.step(config_set("targets.new.tags", "[q, \"r s\"]"));
            case.step(config_set("targets.new.dir", "|\n  block\n  text\n"));
            case.step(config_set("targets.new.default", "false"));
            case.step(config_set(
                "targets.new.extra",
                "{z: 1, a: [2, {b: c}], m: {}}",
            ));
            case.step(config_set("targets.new.extra.deeper", "2001-12-14"));
            assert_config_show_text(&case);
            case.step(config_unset("targets.new.extra.m"));
            case.step(config_unset("targets.missing"));
            assert_config_show_text(&case);
            case.step(config_show());
        });
    }
}

/// The rewriting command of a family A mode of go-lossy-probe-1.
fn probe_rewrite(mode: &str, variant: &str) -> Step {
    match mode {
        "A1" => config_set("targets.other.dir", "o2"),
        "A2" => config_unset("targets.default.default"),
        "A3" => config_set("targets.other.extra", variant),
        _ => unreachable!("modes are A1, A2 and A3"),
    }
}

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with LUUNA_BEACON_REFERENCE=<clone> LUUNA_BEACON_GO=<go> cargo test -p luuna-beacon --locked --test parity_authoring -- --ignored"]
fn config_rewrite_probe_rows_like_go() {
    // Every family A row of go-lossy-probe-1 except v10d (refused, RD14), in
    // its three modes: A1 `config set targets.other.dir o2` and A2 `config
    // unset targets.default.default` on the base file holding the variant at
    // targets.other.extra, A3 `config set targets.other.extra <variant>` on
    // the base file without it. S1 and S2 apply.
    let go_refuses = ["v09a", "v09b", "v09c", "v10m"];
    let gate_text = ["v09a", "v09b", "v09c"];
    for (row, variant) in FAMILY_A {
        if row == "v10d" {
            continue;
        }
        for mode in ["A1", "A2", "A3"] {
            labelled(&format!("S1/S2: row {row} mode {mode}"), || {
                let file = if mode == "A3" {
                    PROBE_BASE_CONFIG.as_bytes().to_vec()
                } else {
                    probe_config_with(variant)
                };
                let mut case = ready();
                case.step(write_file(CONFIG, &file));
                if mode == "A3" && (row == "v09a" || row == "v09b") {
                    // Not valid UTF-8: the library's config_set takes &str,
                    // so only Go runs (left to the wiring plan).
                    let run = run_go_bytes(
                        case.go_root(),
                        &[b"config", b"set", b"targets.other.extra", variant],
                    );
                    assert_eq!(run.code, 1, "S2: Go's exit status");
                    assert_eq!(
                        run.documents,
                        vec![support::payload::error_document(
                            "config",
                            "yaml: invalid leading UTF-8 octet"
                        )],
                        "S2: Go's error"
                    );
                    assert_eq!(
                        fs::read(case.go_root().join(CONFIG)).expect("config.yaml"),
                        file,
                        "S2: Go must leave config.yaml unchanged"
                    );
                    return;
                }
                let text = std::str::from_utf8(variant).unwrap_or("");
                let step = probe_rewrite(mode, text);
                let seen = if gate_text.contains(&row) {
                    // RD7, RD8: compared by the `yaml: ` prefix.
                    case.step_os_error(step, "yaml: ")
                } else {
                    case.step(step)
                };
                let refused = go_refuses.contains(&row);
                assert_eq!(
                    seen.code,
                    i32::from(refused),
                    "S2: Go's exit status differs from go-lossy-probe-1"
                );
                if refused {
                    for root in [case.go_root(), case.lib_root()] {
                        assert_eq!(
                            fs::read(root.join(CONFIG)).expect("config.yaml"),
                            file,
                            "config.yaml must be unchanged"
                        );
                    }
                }
                if row == "v10m" && mode != "A3" {
                    assert_eq!(
                        seen.error.as_deref(),
                        Some("yaml: unmarshal errors:\n  line 9: mapping key \"a\" already defined at line 9"),
                        "S4: the duplicate-key text"
                    );
                }
                let file_unreadable = mode != "A3" && gate_text.contains(&row);
                for read in [config_show(), config_show_path("targets.other.extra")] {
                    if file_unreadable {
                        case.step_os_error(read, "yaml: ");
                    } else {
                        case.step(read);
                    }
                }
                let refused_file = mode != "A3" && row == "v10m";
                if !(file_unreadable || refused_file) {
                    assert_config_show_text(&case);
                }
            });
        }
    }
}

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with LUUNA_BEACON_REFERENCE=<clone> LUUNA_BEACON_GO=<go> cargo test -p luuna-beacon --locked --test parity_authoring -- --ignored"]
fn config_set_and_unset_errors_like_go() {
    // GD9: unset of a missing key skips validation and the write. GD18: a
    // string default loads but blocks every set. GD19: a non-mapping
    // targets is refused by validation. GD20: a legacy-only config cannot
    // be edited.
    let mut case = Case::new();
    case.step_os_error(
        config_set("project", "x"),
        "open <root>/.luuna/beacon/config.yaml: ",
    );
    case.step_os_error(
        config_unset("project"),
        "open <root>/.luuna/beacon/config.yaml: ",
    );
    case.step(Step::Init);
    for (path, value) in [
        ("", "x"),
        (".x", "x"),
        ("project.x", "y"),
        ("targets.default.dir.x", "y"),
        ("unknown", "1"),
        ("project", "\"\""),
        ("project", "\"  \""),
        ("project", "1"),
        ("version", "1"),
        ("version", "~"),
        ("targets", "[a]"),
        ("targets", "~"),
        ("targets.x", "1"),
        ("targets.x.dir", "\"\""),
        ("targets.x.dir", "5"),
        ("targets.default.default", "\"true\""),
        ("targets.default.default", "yes"),
        ("targets.default.tags", "a"),
        ("targets.default.tags", "[1]"),
        ("targets.default.tags", "[a, ~]"),
        (
            "targets.default",
            "{dir: d, default: false, tags: [a], free: {any: thing}}",
        ),
        ("targets.default.dir", "{a: 1, a: 2}"),
        ("a.b", "\"x\""),
    ] {
        case.step(config_set(path, value));
    }
    for (path, value) in [
        ("targets.x.dir", "[unclosed"),
        ("targets.x.dir", "\"\\ud800\""),
    ] {
        case.step_os_error(config_set(path, value), "yaml: ");
    }
    for path in [
        "nope",
        "targets.nope.x",
        "project.x",
        "project",
        "targets.default.dir",
        "targets.default.default",
        "",
        "targets",
    ] {
        case.step(config_unset(path));
    }
    let configs: [&[u8]; 4] = [
        b"project: luuna-beacon\nversion: \"1\"\ntargets:\n  main:\n    dir: d\n    default: \"true\"\n",
        b"project: luuna-beacon\nversion: \"1\"\ntargets: [x]\n",
        b"emitDir: legacy\n",
        b"project: luuna-beacon\nversion: 1\n",
    ];
    for config in configs {
        case.step(write_file(CONFIG, config));
        case.step(config_set("targets.x.dir", "d"));
        case.step(config_unset("targets.main.default"));
        case.step(config_unset("emitDir"));
        case.step(config_show());
    }
}

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with LUUNA_BEACON_REFERENCE=<clone> LUUNA_BEACON_GO=<go> cargo test -p luuna-beacon --locked --test parity_authoring -- --ignored"]
fn config_set_on_an_empty_config_panics_in_go_and_errs_in_the_library() {
    // RD3, GD7 (AS7): Go panics on an empty or null document (assignment to
    // entry in nil map); the library returns GoPanic with the same first
    // line and writes nothing. If Go does not panic, this test fails and
    // shows what Go did (inferred_rule_checks fallback).
    for content in [&b""[..], b"null\n", b"# only a comment\n", b"~\n", b"---\n"] {
        labelled(&format!("config.yaml {:?}", lossy(content)), || {
            let mut case = ready();
            case.step(write_file(CONFIG, content));
            for (path, value) in [("project", "x"), ("a.b", "c")] {
                let go_run = go::run_go(case.go_root(), &["config", "set", path, value]);
                let first_line = lossy(&go_run.stderr)
                    .lines()
                    .next()
                    .unwrap_or("")
                    .to_owned();
                assert_eq!(
                    (go_run.code, first_line.as_str()),
                    (2, "panic: assignment to entry in nil map"),
                    "Go's exit status and first stderr line (stdout {:?})",
                    lossy(&go_run.stdout)
                );
                assert!(go_run.documents.is_empty(), "Go printed no document");
                let lib = case.beacon().config_set(path, value);
                match lib {
                    Err(BeaconError::Authoring(AuthoringError::GoPanic(message))) => {
                        assert_eq!(message, "assignment to entry in nil map")
                    }
                    Err(other) => panic!("expected GoPanic, got {other}"),
                    Ok(_) => panic!("expected GoPanic, the library succeeded"),
                }
                assert_eq!(
                    luuna_beacon::BeaconError::from(AuthoringError::GoPanic(
                        "assignment to entry in nil map".to_owned()
                    ))
                    .to_string(),
                    "panic: assignment to entry in nil map"
                );
                for root in [case.go_root(), case.lib_root()] {
                    assert_eq!(
                        fs::read(root.join(CONFIG)).expect("config.yaml"),
                        content,
                        "config.yaml must be unchanged"
                    );
                }
            }
            case.check_stores();
            case.step(config_unset("project"));
            case.step(config_show());
            assert_config_show_text(&case);
            case.step(config_show_path("project"));
        });
    }
}

/// One config.yaml of config_yaml_decoding_edge_cases_like_go: written on
/// both sides, then `config show` (JSON and text) and `config show --path`
/// for each path, compared exactly, or by `prefix` when given (RD7, RD8).
fn decoding_case(case: &mut Case, file: &[u8], paths: &[&str], prefix: Option<&str>) {
    case.step(write_file(CONFIG, file));
    let mut steps = vec![config_show()];
    steps.extend(paths.iter().map(|path| config_show_path(path)));
    for step in steps {
        match prefix {
            Some(prefix) => {
                case.step_os_error(step, prefix);
            }
            None => {
                case.step(step);
            }
        }
    }
    let lib = case.beacon().config_show().map(|shown| shown.text);
    assert_text_matches(case, &["config", "show"], lib, prefix);
    for path in paths {
        let lib = case
            .beacon()
            .config_get(path)
            .map(|value| value.to_yaml().into_bytes());
        assert_text_matches(case, &["config", "show", "--path", path], lib, prefix);
    }
}

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with LUUNA_BEACON_REFERENCE=<clone> LUUNA_BEACON_GO=<go> cargo test -p luuna-beacon --locked --test parity_authoring -- --ignored"]
fn config_yaml_decoding_edge_cases_like_go() {
    // GD46: later documents and null top-level keys are ignored on reading.
    // RD7, RD8: syntax errors and a non-mapping top level compared by
    // prefix. RD9: anchor names with `.` are tokenised as yaml-rust2 does.
    // Inferred checks: merges, duplicate keys, timestamps, !!binary,
    // 0o/0b/underscore integers, .inf/.nan, uint64, multi-document input,
    // control characters (exact text) and a leading BOM.
    let mut case = ready();
    let base = "project: luuna-beacon\nversion: \"1\"\n";
    let with = |rest: &str| format!("{base}{rest}").into_bytes();
    decoding_case(
        &mut case,
        &with("x: 1\n---\nlater: doc\n---\n[broken\n"),
        &["x", "later"],
        None,
    );
    decoding_case(
        &mut case,
        &with("~: a\nnull: b\n\"\": c\nx: 1\n"),
        &["x", ""],
        None,
    );
    decoding_case(
        &mut case,
        b"<<: {project: merged, version: \"1\"}\nx: {<<: [{a: 1}, {a: 2, b: 3}], c: 4}\n",
        &["project", "x", "x.a", "x.b"],
        None,
    );
    decoding_case(
        &mut case,
        &with("ts: 2001-12-14t21:59:43.10-05:00\ndate: 2002-12-14\nspace: 2001-12-14 21:59:43.10 -5\nbin: !!binary aGVsbG8=\noct: 0o17\nold_oct: 017\nbinnum: 0b101\nunder: 1_000\nbig: 18446744073709551615\nbigger: 18446744073709551616\nhexbig: 0xFFFFFFFFFFFFFFFF\nfloat: 6.8523015e+5\nsexa: 190:20:30\nstr_tag: !!str 12\nbool_words: [y, Yes, ON, off, True, FALSE]\nnulls: [~, null, Null, NULL, \"\"]\n"),
        &["ts", "date", "space", "bin", "oct", "old_oct", "binnum", "under", "big", "bigger", "hexbig", "float", "sexa", "str_tag", "bool_words", "nulls"],
        None,
    );
    decoding_case(
        &mut case,
        &with("anchored: &k {a: 1}\nref: *k\nlist: [&v 1, *v]\n"),
        &["ref", "list"],
        None,
    );
    decoding_case(&mut case, &with("project: again\n"), &["project"], None);
    decoding_case(&mut case, &with("x: {a: 1, a: 2}\n"), &["x"], None);
    decoding_case(&mut case, &with("x: {<<: 1}\n"), &["x"], None);
    decoding_case(&mut case, b"- a\n- b\n", &["project"], Some("yaml: "));
    decoding_case(&mut case, b"just a string\n", &["project"], Some("yaml: "));
    decoding_case(&mut case, &with("x: [\n"), &["x"], Some("yaml: "));
    decoding_case(&mut case, &with("x: \"\\ud800\"\n"), &["x"], Some("yaml: "));
    decoding_case(
        &mut case,
        &with("x: a\tb\ny:\n\t- tab indent\n"),
        &["x"],
        Some("yaml: "),
    );
    decoding_case(&mut case, &with("x: a\x01b\n"), &["x"], None);
    decoding_case(&mut case, &with("x: a\x7fb\n"), &["x"], None);
    decoding_case(
        &mut case,
        &with("x: a\x00b\nlater: hidden\n"),
        &["x", "later"],
        None,
    );
    decoding_case(
        &mut case,
        "\u{feff}project: bom\nversion: \"1\"\nx: 1\n".as_bytes(),
        &["project", "x"],
        None,
    );
    decoding_case(&mut case, b"", &["project"], None);
    decoding_case(&mut case, b"# only a comment\n", &["project"], None);
    decoding_case(
        &mut case,
        &with("x: .inf\ny: -.Inf\nz: .NaN\n"),
        &["x", "y", "z"],
        None,
    );
    decoding_case(
        &mut case,
        &with("? [complex, key]\n: v\n"),
        &["project"],
        None,
    );
    // RD9: `&a.b` is a syntax error for yaml.v3 (its anchor names end at
    // `.`); yaml-rust2 reads the anchor `a.b`. The difference is pinned.
    case.step(write_file(CONFIG, &with("x: &a.b 1\ny: *a.b\n")));
    let go_run = go::run_go(case.go_root(), &["config", "show", "--path", "y"]);
    assert_eq!(go_run.code, 1, "RD9: Go refuses the anchor name");
    let error = go_run.documents[0]["error"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    assert!(error.starts_with("yaml: "), "RD9: Go's error {error:?}");
    let value = case
        .beacon()
        .config_get("y")
        .expect("RD9: the library reads the anchor a.b");
    assert_eq!(value.to_go_json().expect("JSON"), "1");
}

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with LUUNA_BEACON_REFERENCE=<clone> LUUNA_BEACON_GO=<go> cargo test -p luuna-beacon --locked --test parity_authoring -- --ignored"]
fn config_validation_with_several_offenders_matches_one_of_gos_messages() {
    // GD8: Go reports a random one of several offenders (map order); the
    // library reports the first in file order. Neither side writes.
    let cases: [(&[u8], &[&str], &str); 3] = [
        (
            b"project: luuna-beacon\nversion: \"1\"\nzz: 1\naa: 2\n",
            &["unknown property \"zz\"", "unknown property \"aa\""],
            "unknown property \"zz\"",
        ),
        (
            b"project: luuna-beacon\nversion: \"1\"\ntargets:\n  zz: 1\n  aa: 2\n",
            &["targets.zz must be an object", "targets.aa must be an object"],
            "targets.zz must be an object",
        ),
        (
            b"project: luuna-beacon\nversion: \"1\"\ntargets:\n  zz:\n    dir: \"\"\n  aa:\n    dir: a\n    default: 1\n",
            &["targets.zz.dir must be a non-empty string", "targets.aa.default must be boolean"],
            "targets.zz.dir must be a non-empty string",
        ),
    ];
    for (file, go_choices, lib_choice) in cases {
        let mut case = ready();
        case.step(write_file(CONFIG, file));
        for _ in 0..3 {
            let go_run = go::run_go(case.go_root(), &["config", "set", "targets.new.dir", "d"]);
            assert_eq!(go_run.code, 1);
            let error = go_run.documents[0]["error"].as_str().unwrap_or_default();
            let detail = error
                .strip_prefix("config validation failed: ")
                .unwrap_or_else(|| panic!("Go's error {error:?}"));
            assert!(
                go_choices.contains(&detail),
                "Go's message {detail:?} is one of {go_choices:?}"
            );
        }
        let lib = case.beacon().config_set("targets.new.dir", "d");
        match lib {
            Err(BeaconError::Authoring(AuthoringError::ConfigValidation(detail))) => {
                assert_eq!(detail, lib_choice)
            }
            Err(other) => panic!("expected ConfigValidation, got {other}"),
            Ok(_) => panic!("expected ConfigValidation, the library succeeded"),
        }
        for root in [case.go_root(), case.lib_root()] {
            assert_eq!(fs::read(root.join(CONFIG)).expect("config.yaml"), file);
        }
        case.check_stores();
    }
}

// ---- A8: Go's loss shown, the library's refusal ------------------------------

/// Runs `lib` on the library root and checks the refusal's exact Display
/// and that every file under the root is as before.
fn assert_refused(
    pair: &Pair,
    context: &str,
    expected: &str,
    lib: impl FnOnce(&luuna_beacon::Beacon) -> Result<(), BeaconError>,
) {
    let before = tree_bytes(&pair.lib_root);
    let error = match lib(pair.beacon()) {
        Err(error) => error,
        Ok(()) => panic!("{context}: the library must refuse"),
    };
    assert!(
        matches!(error, BeaconError::Authoring(_)),
        "{context}: the refusal is an AuthoringError: {error}"
    );
    assert_eq!(error.to_string(), expected, "{context}: the refusal's text");
    assert_tree_unchanged(&before, &pair.lib_root, context);
}

const LATER_DOCUMENTS_FILE: &str =
    "refused: config.yaml holds more than one YAML document and the rewrite would keep only the first; config.yaml was not changed";
const LATER_DOCUMENTS_VALUE: &str =
    "refused: the value holds more than one YAML document and the rewrite would keep only the first; config.yaml was not changed";
const NULL_KEY: &str =
    "refused: config.yaml has a null top-level key that the rewrite would drop; config.yaml was not changed";
const READ_BACK: &str =
    "refused: after the rewrite the value at targets.other.extra would read back as a different value; config.yaml was not changed";

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with LUUNA_BEACON_REFERENCE=<clone> LUUNA_BEACON_GO=<go> cargo test -p luuna-beacon --locked --test parity_authoring -- --ignored"]
fn a8_go_config_set_loses_later_documents_and_null_keys() {
    // RD12 and RD13.
    let later = format!("{PROBE_BASE_CONFIG}---\nkept: only in the second document\n");
    let cases: Vec<(&str, String, Step, &str, &str)> = vec![
        (
            "later document, set",
            later.clone(),
            config_set("targets.other.dir", "o2"),
            "second document",
            LATER_DOCUMENTS_FILE,
        ),
        (
            "later document, unset",
            later.clone(),
            config_unset("targets.default.default"),
            "second document",
            LATER_DOCUMENTS_FILE,
        ),
        (
            "later document in the value",
            PROBE_BASE_CONFIG.to_owned(),
            config_set("targets.other.dir", "o2\n---\nlost-part"),
            "lost-part",
            LATER_DOCUMENTS_VALUE,
        ),
        (
            "null key ~, set",
            format!("{PROBE_BASE_CONFIG}~: lost by the tilde key\n"),
            config_set("targets.other.dir", "o2"),
            "lost by the tilde key",
            NULL_KEY,
        ),
        (
            "null key null, unset",
            format!("{PROBE_BASE_CONFIG}null: lost by the null key\n"),
            config_unset("targets.default.default"),
            "lost by the null key",
            NULL_KEY,
        ),
        (
            "empty null key, set",
            format!("{PROBE_BASE_CONFIG}? \n: lost by the empty key\n"),
            config_set("targets.other.dir", "o2"),
            "lost by the empty key",
            NULL_KEY,
        ),
    ];
    for (label, file, step, lost, display) in cases {
        labelled(label, || {
            let mut pair = Pair::new();
            pair.step(Step::Init);
            pair.raw(write_file(CONFIG, file.as_bytes()));
            let go_run = pair.go(&step);
            assert_eq!(
                go_run.code, 0,
                "Go rewrites the file: {:?}",
                go_run.documents
            );
            let go_file = fs::read_to_string(pair.go_root.join(CONFIG)).expect("Go's config.yaml");
            assert!(
                !go_file.contains(lost),
                "Go's rewritten file no longer holds {lost:?}: {go_file:?}"
            );
            let argv = case::lower(&step).argv.expect("a command step");
            assert_refused(&pair, label, display, |beacon| match argv[1].as_str() {
                "set" => beacon.config_set(&argv[2], &argv[3]).map(|_| ()),
                _ => beacon.config_unset(&argv[2]).map(|_| ()),
            });
            assert_eq!(
                fs::read_to_string(pair.lib_root.join(CONFIG)).expect("the library's config.yaml"),
                file,
                "the library's config.yaml is byte for byte the original"
            );
        });
    }
}

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with LUUNA_BEACON_REFERENCE=<clone> LUUNA_BEACON_GO=<go> cargo test -p luuna-beacon --locked --test parity_authoring -- --ignored"]
fn a8_go_config_rewrite_read_back_cases() {
    // RD14, probe row v10d `-0.0` of go-lossy-probe-1 in modes A1, A2 and A3
    // (table lines 48671, 48722, 48773).
    let variant = family_a("v10d");
    let text = std::str::from_utf8(variant).expect("v10d is UTF-8");
    for mode in ["A1", "A2", "A3"] {
        labelled(&format!("v10d mode {mode}"), || {
            let file = if mode == "A3" {
                PROBE_BASE_CONFIG.as_bytes().to_vec()
            } else {
                probe_config_with(variant)
            };
            let mut pair = Pair::new();
            pair.step(Step::Init);
            pair.raw(write_file(CONFIG, &file));
            let read = config_show_path("targets.other.extra");
            let step = probe_rewrite(mode, text);
            let minus_zero = "\"value\": -0\n";
            if mode != "A3" {
                let before = pair.go(&read);
                assert!(
                    lossy(&before.stdout).contains(minus_zero),
                    "Go reads -0 before: {}",
                    lossy(&before.stdout)
                );
            }
            let go_run = pair.go(&step);
            assert_eq!(go_run.code, 0, "Go rewrites the file");
            if mode == "A3" {
                assert!(
                    lossy(&go_run.stdout).contains(minus_zero),
                    "Go's own set document shows -0: {}",
                    lossy(&go_run.stdout)
                );
            }
            let after = pair.go(&read);
            assert!(
                lossy(&after.stdout).contains("\"value\": 0\n"),
                "Go reads 0 after: {}",
                lossy(&after.stdout)
            );
            let go_file = fs::read_to_string(pair.go_root.join(CONFIG)).expect("Go's config.yaml");
            assert!(
                go_file.contains("    extra: -0\n"),
                "Go's file: {go_file:?}"
            );
            assert_refused(&pair, mode, READ_BACK, |beacon| match mode {
                "A1" => beacon.config_set("targets.other.dir", "o2").map(|_| ()),
                "A2" => beacon.config_unset("targets.default.default").map(|_| ()),
                _ => beacon.config_set("targets.other.extra", text).map(|_| ()),
            });
            if mode != "A3" {
                let result = pair.beacon().config_set("targets.other.dir", "o2");
                assert!(
                    matches!(
                        result,
                        Err(BeaconError::Authoring(AuthoringError::ConfigReadBack(ref path)))
                            if path == "targets.other.extra"
                    ),
                    "ConfigReadBack names targets.other.extra"
                );
            }
        });
    }
}

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with LUUNA_BEACON_REFERENCE=<clone> LUUNA_BEACON_GO=<go> cargo test -p luuna-beacon --locked --test parity_authoring -- --ignored"]
fn a8_go_import_loses_text_keys_and_values() {
    // RD15-RD21. Go stores the record without the data; the library refuses
    // and stores nothing.
    const UNRECOGNISED: &str = "refused: the frontmatter block is not in the form the Go tool reads (a byte order mark, CRLF line ends or an empty block) and would not be stored as frontmatter; nothing was stored";
    let cases: Vec<(&str, &[u8], &str, String)> = vec![
        ("bom", "\u{feff}---\nkeep: me\n---\n## S\nb\n".as_bytes(), "keep", UNRECOGNISED.to_owned()),
        ("crlf", b"---\r\nkeep: me\r\n---\r\n## S\r\nb\r\n", "keep", UNRECOGNISED.to_owned()),
        ("crlf-close", b"---\nkeep: me\n---\r\nlost: yes\n---\n## S\nb\n", "lost", UNRECOGNISED.to_owned()),
        ("empty-block", b"---\n---\n## S\nb\n", "---", UNRECOGNISED.to_owned()),
        (
            "intro",
            b"intro text\n## S\nb\n",
            "intro text",
            "refused: text before the first \"## \" heading would be dropped; nothing was stored".to_owned(),
        ),
        (
            "number-description",
            b"---\ndescription: 5\n---\n## S\nb\n",
            "description",
            "refused: the frontmatter description is not a string and would be dropped; nothing was stored".to_owned(),
        ),
        (
            "null-key",
            b"---\n~: lost\nk: v\n---\n## S\nb\n",
            "lost",
            "refused: a frontmatter key is null and would be dropped; nothing was stored".to_owned(),
        ),
        (
            "later-document",
            b"---\nk: v\n--- \nj: w\n---\n## S\nb\n",
            "\"j\"",
            "refused: the frontmatter holds more than one YAML document and only the first would be stored; nothing was stored".to_owned(),
        ),
        (
            "rd20",
            b"---\nname: other\n---\n## S\nb\n",
            "other",
            "refused: the frontmatter name \"other\" differs from the name given and would be dropped; nothing was stored".to_owned(),
        ),
        (
            "read-back",
            b"---\nn: 9007199254740993\n---\n## S\nb\n",
            "9007199254740993",
            "refused: the frontmatter value at n would read back from the store as a different value; nothing was stored".to_owned(),
        ),
    ];
    let mut pair = Pair::new();
    pair.step(Step::Init);
    for (name, bytes, lost, display) in cases {
        labelled(name, || {
            let path = format!("src/{name}.md");
            pair.raw(write_file(&path, bytes));
            let go_run = pair.go(&import(&path, name, &[], false));
            assert_eq!(go_run.code, 0, "Go imports {name}: {:?}", go_run.documents);
            let shown = pair.go(&show(name));
            let record = shown.documents[0]["prompt"].to_string();
            assert!(
                !record.contains(lost),
                "Go's stored record no longer holds {lost:?}: {record}"
            );
            if name == "read-back" {
                assert_eq!(
                    shown.documents[0]["prompt"]["frontmatter"]["n"],
                    json!(9007199254740992_u64),
                    "Go reads 9007199254740993 back as 9007199254740992"
                );
            }
            assert_refused(&pair, name, &display, |beacon| {
                beacon
                    .import_markdown(&import_input(&path, name, &[], false))
                    .map(|_| ())
            });
        });
    }
}

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with LUUNA_BEACON_REFERENCE=<clone> LUUNA_BEACON_GO=<go> cargo test -p luuna-beacon --locked --test parity_authoring -- --ignored"]
fn a8_go_emit_overwrites_store_files() {
    // RD22: Go replaces store files under .luuna/ with Markdown; the library
    // refuses before its first write, so earlier prompts' targets are not
    // written either.
    let refusal = |path: &Path| {
        format!(
            "refused: emit would overwrite {} inside .luuna/ with a rendered prompt; nothing was written",
            path.display()
        )
    };
    // A prompt's emitDir and emitAs aimed at prompts.jsonl, alone and beside
    // an ordinary prompt.
    for with_other in [false, true] {
        labelled(
            &format!("emitAs into the store, other prompt {with_other}"),
            || {
                let mut pair = Pair::new();
                pair.step(Step::Init);
                if with_other {
                    pair.step(create("ordinary", &[("Task", "x")]));
                }
                pair.step(create_with(CreatePrompt {
                    name: "into-store".into(),
                    emit_dir: ".luuna/beacon".into(),
                    emit_as: "prompts.jsonl".into(),
                    sections: vec![sec("Task", "replaces the records")],
                    ..Default::default()
                }));
                let step = emit(if with_other {
                    emit_all()
                } else {
                    emit_one("into-store")
                });
                let go_run = pair.go(&step);
                assert_eq!(go_run.code, 0);
                let go_store = fs::read_to_string(pair.go_root.join(PROMPTS)).expect("Go's store");
                assert!(
                    go_store.starts_with("---\n") && !go_store.contains("\"id\""),
                    "Go's prompts.jsonl now holds Markdown: {go_store:?}"
                );
                let target = pair.lib_root.join(PROMPTS);
                assert_refused(
                    &pair,
                    "emitAs into the store",
                    &refusal(&target),
                    |beacon| {
                        beacon
                            .emit(&if with_other {
                                emit_all()
                            } else {
                                emit_one("into-store")
                            })
                            .map(|_| ())
                    },
                );
                assert!(!pair.lib_root.join("agents").exists());
            },
        );
    }
    // An absolute --out naming config.yaml.
    let mut pair = Pair::new();
    pair.step(Step::Init);
    pair.step(create("p", &[("Task", "x")]));
    let go_root = pair.go_root.clone();
    let lib_root = pair.lib_root.clone();
    let step = emit_absolute(emit_one("p"), &go_root, &lib_root, Some(CONFIG), None);
    let go_run = pair.go(&step);
    assert_eq!(go_run.code, 0);
    let go_config = fs::read_to_string(go_root.join(CONFIG)).expect("Go's config.yaml");
    assert!(
        go_config.starts_with("---\nname: p\n"),
        "Go's config.yaml now holds Markdown: {go_config:?}"
    );
    let target = lib_root.join(CONFIG);
    let mut options = emit_one("p");
    options.out = Some(target.to_string_lossy().into_owned());
    assert_refused(
        &pair,
        "--out into config.yaml",
        &refusal(&target),
        |beacon| beacon.emit(&options).map(|_| ()),
    );
}

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with LUUNA_BEACON_REFERENCE=<clone> LUUNA_BEACON_GO=<go> cargo test -p luuna-beacon --locked --test parity_authoring -- --ignored"]
fn a8_go_pin_and_unpin_lose_unknown_and_repeated_keys() {
    // RD23 and RD24: b04a, b04b, b04c, b05d of go-lossy-probe-1 and the
    // unprobed cases (S3 applies). Go exits 0 and its appended line lacks
    // the key (RD23) or no longer holds the value RD24 says Go discards; the
    // library refuses with its root unchanged.
    for loss in probed_loss_cases().into_iter().chain(unprobed_loss_cases()) {
        for command in ["pin", "unpin"] {
            let label = format!("S3: case {} `{command}`", loss.id);
            labelled(&label, || {
                let mut pair = Pair::new();
                pair.step(Step::Init);
                pair.step(probe_create());
                pair.raw(write_file(PROMPTS, loss.record.as_bytes()));
                let reference = if command == "pin" {
                    format!("{}@1", loss.name)
                } else {
                    loss.name.to_owned()
                };
                let step = if command == "pin" {
                    pin(&reference)
                } else {
                    unpin(&reference)
                };
                let go_run = pair.go(&step);
                assert_eq!(
                    go_run.code, 0,
                    "Go refuses this input itself: {:?}",
                    go_run.documents
                );
                let go_store = fs::read(pair.go_root.join(PROMPTS)).expect("Go's store");
                assert!(
                    go_store.starts_with(loss.record.as_bytes()),
                    "the old line stays a prefix of Go's store"
                );
                let appended = lossy(&go_store[loss.record.len()..]);
                assert_eq!(appended.matches('\n').count(), 1, "Go appends one line");
                for text in loss.go_line_lacks {
                    let what = if loss.id == "r24-tags-later-null" {
                        "RD24 point (3) says Go's later null sets tags to nil and discards the earlier [\"a\"], but Go's appended line still holds it"
                    } else {
                        "Go's appended line still holds data RD23/RD24 say Go loses"
                    };
                    assert!(!appended.contains(text), "{what}: {text:?} in {appended}");
                }
                for text in loss.go_line_holds {
                    assert!(
                        appended.contains(text),
                        "Go's appended line lacks the value RD24 says Go keeps: {text:?} in {appended}"
                    );
                }
                assert_refused(&pair, &label, &loss.display(), |beacon| {
                    if command == "pin" {
                        beacon.pin(&reference).map(|_| ())
                    } else {
                        beacon.unpin(&reference).map(|_| ())
                    }
                });
                let error = if command == "pin" {
                    pair.beacon().pin(&reference).err()
                } else {
                    pair.beacon().unpin(&reference).err()
                };
                let matches_variant = match error {
                    Some(BeaconError::Authoring(AuthoringError::PinRecordKeyUnknown(
                        name,
                        key,
                    ))) => loss.unknown && name == loss.name && key == loss.key,
                    Some(BeaconError::Authoring(AuthoringError::PinRecordKeyRepeated(
                        name,
                        key,
                    ))) => !loss.unknown && name == loss.name && key == loss.key,
                    _ => false,
                };
                assert!(matches_variant, "the refusal's variant, name and key");
            });
        }
    }
}
