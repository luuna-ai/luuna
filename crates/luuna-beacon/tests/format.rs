//! Store-format parity with the Go tool: what one side writes the other reads,
//! byte for byte where bytes are the contract. Run:
//! LUUNA_BEACON_REFERENCE=<clone> cargo test -p luuna-beacon --locked --test format -- --ignored
//!
//! These tests are written against the library contract of the plan and
//! compile once the library exists.

mod support;

use luuna_beacon::{CreatePrompt, UpdatePrompt};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fs;
use support::case::*;
use support::go::{self, GoRun};

fn up() -> UpdatePrompt {
    UpdatePrompt::default()
}

fn some(text: &str) -> Option<String> {
    Some(text.to_owned())
}

fn go_ok(shared: &Shared, step: Step) -> GoRun {
    let run = shared.go(&step);
    assert_eq!(
        run.code,
        0,
        "Go failed on `{}`: {}",
        describe(&step),
        String::from_utf8_lossy(&run.stdout)
    );
    run
}

fn lib_ok(shared: &Shared, step: Step) {
    let run = shared.lib(&step);
    assert!(
        run.ok && run.code == 0,
        "the library failed on `{}`: {:?}",
        describe(&step),
        run.error
    );
}

/// The write steps every read test starts from: both sides can run them.
fn script() -> Vec<Step> {
    vec![
        Step::Init,
        create_with(CreatePrompt {
            name: "base".into(),
            sections: vec![sec("Context", "ctx"), sec("Style", "terse")],
            frontmatter: pairs(&[("from", "base"), ("n", "1")]),
            ..Default::default()
        }),
        create("mix", &[("Extra", "extra")]),
        create_with(CreatePrompt {
            name: "full".into(),
            description: "Full <prompt> & more".into(),
            extends: "base".into(),
            mixins: strings(&["mix"]),
            tags: strings(&["command", "review"]),
            emit_as: "full.md".into(),
            emit_dir: "out".into(),
            status: "draft".into(),
            sections: vec![
                sec("Task", "<b>&</b> \u{2028}\u{2029} done"),
                sec("Notes", "tab\there\u{1}\u{7f}\n\u{1f600}"),
            ],
            frontmatter: pairs(&[("k", "v"), ("html", "<&>")]),
            ..Default::default()
        }),
        create("retired", &[("Task", "old")]),
        archive("retired"),
        schema_create("s1", &["Task"], &["Notes"]),
        rule_add("s1", "Task", ".", "needs text"),
        create_with(CreatePrompt {
            name: "p".into(),
            schema: "s1".into(),
            sections: vec![sec("Task", "x")],
            ..Default::default()
        }),
        update(
            "full",
            UpdatePrompt {
                description: some("changed"),
                add_tags: strings(&["more"]),
                sections: vec![sec("Extra2", "e2")],
                ..up()
            },
        ),
        update(
            "p",
            UpdatePrompt {
                name: some("q"),
                ..up()
            },
        ),
    ]
}

/// The read-only steps compared on a store both sides can read.
fn reads() -> Vec<Step> {
    vec![
        list(),
        Step::List(luuna_beacon::ListFilter {
            status: some("archived"),
            ..Default::default()
        }),
        show("full"),
        show("full@1"),
        show("base"),
        show("retired"),
        show("q"),
        history("full", None),
        history("base", Some(1)),
        render("full", &["full", "base", "mix"]),
        render("base", &["base"]),
        render("q", &["q"]),
        render_json("full"),
        validate("full"),
        validate("q"),
        validate("p"),
        Step::ValidateAll,
        Step::SchemaList,
        schema_show("s1"),
    ]
}

fn read_all(shared: &mut Shared) {
    for step in reads() {
        shared.both_read(&step);
    }
}

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with --ignored, see crates/luuna-beacon/README.md"]
fn go_written_store_is_read_by_rust_exactly() {
    let mut shared = Shared::new();
    for step in script() {
        go_ok(&shared, step);
    }
    read_all(&mut shared);
}

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with --ignored, see crates/luuna-beacon/README.md"]
fn rust_written_store_is_read_by_go_exactly() {
    let mut shared = Shared::new();
    for step in script() {
        lib_ok(&shared, step);
    }
    read_all(&mut shared);
}

/// The lines of a store file, without their terminators.
fn lines_of(shared: &Shared, name: &str) -> Vec<String> {
    let text = fs::read_to_string(shared.store_file(name)).expect("read a store file");
    assert!(
        text.is_empty() || text.ends_with('\n'),
        "{name} ends without a newline"
    );
    text.lines().map(str::to_owned).collect()
}

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with --ignored, see crates/luuna-beacon/README.md"]
fn go_written_lines_reencode_byte_identical_in_rust() {
    let shared = Shared::new();
    for step in script() {
        go_ok(&shared, step);
    }
    // A record with every field, nested frontmatter and atlas, written by
    // hand; Go rewrites it with its own encoder on update, and so does a
    // schema through `rule add`.
    let rich = format!(
        "{}\n",
        json!({
            "id": "luuna-beacon-d1",
            "name": "rich",
            "description": "rich <&> \u{2028}",
            "version": 1,
            "sections": [
                {"name": "S <1>", "body": "b\u{1}\u{2028}\u{2029}<&>\"\\\u{7f}\u{1f600}", "required": true},
                {"name": "T", "body": "", "required": false}
            ],
            "extends": "base",
            "mixins": ["mix"],
            "tags": ["a", "b"],
            "schema": "s1",
            "emitAs": "rich.md",
            "emitDir": "out",
            "pinned": 1,
            "frontmatter": {
                "nested": {"list": [1, 2.5, {"deep": null}, "x<y"], "empty": {}, "e": []},
                "z": "last",
                "a": true
            },
            "atlas": {
                "prime": {"domains": ["d1", "d2"], "files": ["f<1>.rs"]},
                "budget": 12.5,
                "on_empty": "warn"
            },
            "extends_atlas": true,
            "status": "active",
            "createdAt": STAMP,
            "updatedAt": STAMP,
        })
    );
    let prompts = shared.store_file("prompts.jsonl");
    let mut text = fs::read_to_string(&prompts).unwrap();
    text.push_str(&rich);
    fs::write(&prompts, text).unwrap();
    go_ok(
        &shared,
        update(
            "rich",
            UpdatePrompt {
                description: some("rewritten <&> by Go"),
                ..up()
            },
        ),
    );
    go_ok(&shared, rule_add("s1", "Task", "[<>&]", "no html \u{2028}"));

    // Prompts: every line Go wrote re-encodes to itself.
    let beacon = shared.beacon();
    let mut checked = 0;
    for line in lines_of(&shared, "prompts.jsonl") {
        if format!("{line}\n") == rich {
            continue;
        }
        let record: Value = serde_json::from_str(&line).unwrap();
        let (id, version) = (
            record["id"].as_str().unwrap().to_owned(),
            record["version"].as_i64().unwrap(),
        );
        let name = record["name"].as_str().unwrap();
        let versions = beacon.history(name, 1000).expect("history by name");
        let prompt = versions
            .iter()
            .find(|prompt| prompt.id == id && prompt.version == version)
            .unwrap_or_else(|| panic!("record {id}@{version} is not returned by history"));
        assert_eq!(prompt.to_go_json(), line, "prompt {name}@{version}");
        checked += 1;
    }
    assert_eq!(checked, 9, "only {checked} prompt lines were checked");

    // Schemas: the last line of each id is the current record.
    let mut last: BTreeMap<String, String> = BTreeMap::new();
    for line in lines_of(&shared, "schemas.jsonl") {
        let record: Value = serde_json::from_str(&line).unwrap();
        last.insert(record["id"].as_str().unwrap().to_owned(), line);
    }
    let schemas = beacon.schema_list().expect("schema list");
    assert_eq!(schemas.len(), last.len());
    for schema in schemas {
        assert_eq!(
            schema.to_go_json(),
            last[&schema.id],
            "schema {}",
            schema.name
        );
    }
}

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with --ignored, see crates/luuna-beacon/README.md"]
fn alternating_go_and_rust_appends_stay_readable_by_both() {
    let mut shared = Shared::new();
    go_ok(&shared, Step::Init);
    let rounds: Vec<(bool, Step)> = vec![
        (true, create("a", &[("Task", "go one")])),
        (false, create("b", &[("Task", "rust one <&>")])),
        (
            true,
            update(
                "b",
                UpdatePrompt {
                    description: some("from go"),
                    ..up()
                },
            ),
        ),
        (
            false,
            update(
                "a",
                UpdatePrompt {
                    description: some("from rust"),
                    ..up()
                },
            ),
        ),
        (true, schema_create("s", &["Task"], &[])),
        (false, rule_add("s", "Task", ".", "text")),
        (true, create("c", &[("Task", "go two")])),
        (false, archive("c")),
        (
            true,
            update(
                "a",
                UpdatePrompt {
                    sections: vec![sec("Notes", "n")],
                    ..up()
                },
            ),
        ),
        (false, create("d", &[("Task", "rust two")])),
    ];
    for (by_go, step) in rounds {
        let label = describe(&step);
        if by_go {
            go_ok(&shared, step);
        } else {
            lib_ok(&shared, step);
        }
        for read in [
            list(),
            show("a"),
            history("a", None),
            Step::SchemaList,
            Step::ValidateAll,
        ] {
            shared.both_read(&read);
        }
        for name in ["prompts.jsonl", "schemas.jsonl"] {
            for line in lines_of(&shared, name) {
                serde_json::from_str::<Value>(&line).unwrap_or_else(|error| {
                    panic!("after `{label}` a line of {name} is invalid: {error}")
                });
            }
        }
    }
    for read in [
        show("b"),
        show("d"),
        history("b", None),
        render("a", &["a"]),
    ] {
        shared.both_read(&read);
    }
}

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with --ignored, see crates/luuna-beacon/README.md"]
fn file_modes_after_rewrite_match_go() {
    let mut case = Case::new();
    case.step(Step::Init);
    case.step(create("a", &[("Task", "x")]));
    case.step(schema_create("s", &["Task"], &[]));
    case.step(update(
        "a",
        UpdatePrompt {
            description: some("d"),
            ..up()
        },
    ));
    // compare_stores has compared every mode after each step; state the ones
    // that matter explicitly.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for name in [
            "prompts.jsonl",
            "schemas.jsonl",
            "artifacts.jsonl",
            "config.yaml",
            ".gitignore",
        ] {
            let mode = |root: &std::path::Path| {
                fs::metadata(root.join(".luuna/beacon").join(name))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o7777
            };
            assert_eq!(
                format!("{:o}", mode(case.lib_root())),
                format!("{:o}", mode(case.go_root())),
                "mode of {name}"
            );
        }
    }
    // A file that was 0644 before the rewrite takes the rewrite's mode.
    case.step(write_file(".luuna/beacon/artifacts.jsonl", b""));
    case.step(create("b", &[("Task", "y")]));
    case.check_stores();
}

/// Lines for `legacy_keys_null_sections_unknown_keys_and_blank_lines`.
fn legacy_lines() -> String {
    let legacy_atlas = r#"{"id":"luuna-beacon-e1","name":"legacy","version":1,"sections":[{"name":"T","body":"b"}],"mulch":{"prime":{"domains":["old"]},"budget":3},"extends_mulch":true,"status":"active","createdAt":"2024-01-01T00:00:00Z","updatedAt":"2024-01-01T00:00:00Z"}"#;
    let both_keys = r#"{"id":"luuna-beacon-e2","name":"both","version":1,"sections":[],"atlas":{"budget":9},"mulch":{"budget":1},"extends_atlas":false,"extends_mulch":true,"status":"active","createdAt":"2024-01-01T00:00:00Z","updatedAt":"2024-01-01T00:00:00Z"}"#;
    let nulls = r#"{"id":"luuna-beacon-e3","name":"nulls","version":1,"sections":null,"mixins":null,"tags":null,"frontmatter":null,"atlas":null,"pinned":null,"status":"active","createdAt":"2024-01-01T00:00:00Z","updatedAt":"2024-01-01T00:00:00Z"}"#;
    let unknown = r#"{"id":"luuna-beacon-e4","name":"unknown","version":1,"sections":[{"name":"T","body":"b","extra":1}],"future":{"a":[1,2]},"description":"","tags":[],"status":"active","createdAt":"2024-01-01T00:00:00Z","updatedAt":"2024-01-01T00:00:00Z"}"#;
    format!("\n   \n{legacy_atlas}\n\t\n{both_keys}\n\n{nulls}\n{unknown}\n   \n")
}

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with --ignored, see crates/luuna-beacon/README.md"]
fn legacy_keys_null_sections_unknown_keys_and_blank_lines() {
    let mut case = Case::new();
    case.step(Step::Init);
    case.step(write_file(
        ".luuna/beacon/prompts.jsonl",
        legacy_lines().as_bytes(),
    ));
    case.step(list());
    for name in ["legacy", "both", "nulls", "unknown"] {
        case.step(show(name));
        case.step(render_json(name));
    }
    case.step(render("legacy", &["legacy"]));
    case.step(render("nulls", &["nulls"]));
    // Updating rewrites the record with the current key names.
    for name in ["legacy", "both", "nulls", "unknown"] {
        case.step(update(
            name,
            UpdatePrompt {
                description: some("rewritten"),
                ..up()
            },
        ));
        case.step(show(name));
    }
    case.step(history("legacy", None));

    // A corrupt line stops every read; only the start of the message is
    // compared, as the JSON error text is each language's own.
    let mut case = Case::new();
    case.step(Step::Init);
    let corrupt = format!("{}{{not json\n", legacy_lines());
    case.step(write_file(
        ".luuna/beacon/prompts.jsonl",
        corrupt.as_bytes(),
    ));
    let seen = case.step(list());
    assert_eq!(seen.code, 1);
    case.step(show("legacy"));
    case.step(create("x", &[("Task", "t")]));
    let truncated = "{\"id\":\"luuna-beacon-e9\",\"name\":\"cut\",\"vers";
    case.step(write_file(
        ".luuna/beacon/schemas.jsonl",
        truncated.as_bytes(),
    ));
    case.step(Step::SchemaList);
}

static BUDGET_NAMES: [&str; 10] = [
    "budget0", "budget1", "budget2", "budget3", "budget4", "budget5", "budget6", "budget7",
    "budget8", "budget9",
];

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with --ignored, see crates/luuna-beacon/README.md"]
fn numbers_in_frontmatter_and_atlas_budget_match_go() {
    let stamp = r#""createdAt":"2024-01-01T00:00:00Z","updatedAt":"2024-01-01T00:00:00Z""#;
    let mut text = String::new();
    text.push_str(&format!(
        r#"{{"id":"luuna-beacon-f0","name":"numbers","version":1,"sections":[],"frontmatter":{{"a":1,"b":1.0,"c":-0,"d":0.000001,"e":1e-7,"f":1e21,"g":123456789012345678901,"h":[1,[2.5,{{"x":null}}],{{"y":[]}}],"i":null,"j":{{}},"k":1E5,"l":0.1,"m":100,"n":1.5e300,"o":5e-324,"p":-1.0}},"status":"active",{stamp}}}
"#
    ));
    let budgets = [
        "1",
        "1.0",
        "-0",
        "0.000001",
        "1e-7",
        "1e21",
        "123456789012345678901",
        "0.1",
        "100",
        "2.5e-5",
    ];
    for (index, budget) in budgets.iter().enumerate() {
        text.push_str(&format!(
            r#"{{"id":"luuna-beacon-f{n}","name":"budget{index}","version":1,"sections":[],"atlas":{{"budget":{budget}}},"status":"active",{stamp}}}
"#,
            n = index + 1,
        ));
    }
    let mut case = Case::new();
    case.step(Step::Init);
    case.step(write_file(".luuna/beacon/prompts.jsonl", text.as_bytes()));
    case.step(list());
    case.step(show("numbers"));
    case.step(render("numbers", &["numbers"]));
    // One update by each side; the new lines are compared byte for byte.
    case.step(update(
        "numbers",
        UpdatePrompt {
            description: some("u"),
            ..up()
        },
    ));
    case.step(update(
        "numbers",
        UpdatePrompt {
            frontmatter: pairs(&[("added", "7")]),
            ..up()
        },
    ));
    for index in 0..budgets.len() {
        let name = format!("budget{index}");
        case.step(render(&name, &BUDGET_NAMES[index..=index]));
        case.step(update(
            &name,
            UpdatePrompt {
                description: some("u"),
                ..up()
            },
        ));
        case.step(show(&name));
    }
    case.step(render_json("numbers"));
}

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with --ignored, see crates/luuna-beacon/README.md"]
fn timestamps_use_go_rfc3339nano_trimming() {
    // The normaliser asserts, for every createdAt and updatedAt on both sides,
    // Go's form: UTC `Z`, fractional digits only when non-zero, trailing
    // zeros trimmed. Enough writes make a zero last digit certain to occur.
    let mut case = Case::new();
    case.step(Step::Init);
    case.step(create("a", &[("Task", "x")]));
    let mut trimmed = 0;
    for round in 0..80 {
        let seen = case.step(update(
            "a",
            UpdatePrompt {
                description: Some(format!("round {round}")),
                ..up()
            },
        ));
        let stamp = seen.raw[0]["prompt"]["updatedAt"]
            .as_str()
            .unwrap()
            .to_owned();
        if let Some((_, fraction)) = stamp.trim_end_matches('Z').split_once('.') {
            assert!(
                !fraction.ends_with('0'),
                "Go left a trailing zero in {stamp}"
            );
            if fraction.len() < 9 {
                trimmed += 1;
            }
        }
    }
    assert!(trimmed > 0, "no trimmed fraction occurred in 80 updates");
    case.step(history("a", Some(100)));
}

/// Runs `config set project <name>` with Go in the case's Go root and copies
/// the config it wrote, with its permission bits, into the library root.
fn go_config_set_project(case: &Case, name: &str) {
    let run = go::run_go(case.go_root(), &["config", "set", "project", name]);
    assert_eq!(run.code, 0, "{}", String::from_utf8_lossy(&run.stdout));
    let source = case.go_root().join(".luuna/beacon/config.yaml");
    let target = case.lib_root().join(".luuna/beacon/config.yaml");
    fs::copy(&source, &target).unwrap();
    fs::set_permissions(&target, fs::metadata(&source).unwrap().permissions()).unwrap();
}

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with --ignored, see crates/luuna-beacon/README.md"]
fn config_yaml_written_by_go_is_read_like_go() {
    // The file `init` writes.
    let mut case = Case::new();
    case.step(Step::Init);
    case.step(create("a", &[("Task", "x")]));

    // `config set project demo-x` by Go: Go's emitter output is read by both.
    let mut case = Case::with_project("demo-x");
    case.step(Step::Init);
    go_config_set_project(&case, "demo-x");
    case.check_stores();
    let seen = case.step(create("a", &[("Task", "x")]));
    assert!(seen.raw[0]["id"].as_str().unwrap().starts_with("demo-x-"));

    // Comments, flow style and anchors.
    let mut case = Case::with_project("demo-c");
    case.step(Step::Init);
    case.step(write_file(
        ".luuna/beacon/config.yaml",
        b"# header comment\nproject: demo-c # trailing\nversion: \"1\"\ntargets: {default: {dir: agents, default: true}}\n",
    ));
    let seen = case.step(create("a", &[("Task", "x")]));
    assert!(seen.raw[0]["id"].as_str().unwrap().starts_with("demo-c-"));

    // Quoted and multi-document-free variants.
    let mut case = Case::with_project("demo-q");
    case.step(Step::Init);
    case.step(write_file(
        ".luuna/beacon/config.yaml",
        b"project: 'demo-q'\nversion: 1\n",
    ));
    case.step(create("a", &[("Task", "x")]));

    // A numeric, boolean, empty or null project falls back to the default.
    for config in [
        "project: 123\nversion: \"1\"\n",
        "project: true\nversion: \"1\"\n",
        "project: ''\nversion: \"1\"\n",
        "project:\nversion: \"1\"\n",
        "version: \"1\"\n",
        "",
        "# only a comment\n",
    ] {
        let mut case = Case::new();
        case.step(Step::Init);
        case.step(write_file(".luuna/beacon/config.yaml", config.as_bytes()));
        let seen = case.step(create("a", &[("Task", "x")]));
        assert_eq!(seen.code, 0, "config {config:?}");
        assert!(
            seen.raw[0]["id"]
                .as_str()
                .unwrap()
                .starts_with("luuna-beacon-"),
            "config {config:?} did not fall back"
        );
    }

    // Invalid YAML and a top-level that is not a mapping: both sides fail.
    // The YAML parsers' messages are their own; only the failure is compared.
    for config in [
        "project: [unclosed\n",
        "- a\n- b\n",
        "project: a: b\n",
        "\t\n  bad: [\n",
    ] {
        let mut case = Case::new();
        case.step(Step::Init);
        case.step(write_file(".luuna/beacon/config.yaml", config.as_bytes()));
        let seen = case.step_os_error(create("a", &[("Task", "x")]), "");
        assert_eq!(seen.code, 1, "config {config:?}");
    }

    // A missing file: both fail with the operating system's error.
    let mut case = Case::new();
    let seen = case.step_os_error(
        create("a", &[("Task", "x")]),
        "open <root>/.luuna/beacon/config.yaml: ",
    );
    assert_eq!(seen.code, 1);
}

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with --ignored, see crates/luuna-beacon/README.md"]
fn unterminated_last_line_behaves_like_go() {
    // GD1: an append to a file whose last line has no newline glues the new
    // record onto that line, and later reads fail on it.
    let mut case = Case::new();
    case.step(Step::Init);
    let line = prompt_record(
        "luuna-beacon-a1",
        "first",
        1,
        STAMP,
        &[("Task", "t")],
        json!({}),
    );
    let unterminated = line.trim_end_matches('\n');
    case.step(write_file(
        ".luuna/beacon/prompts.jsonl",
        unterminated.as_bytes(),
    ));
    // The unterminated line alone is read normally.
    case.step(list());
    case.step(show("first"));
    case.step(create("second", &[("Task", "u")]));
    let seen = case.step(list());
    assert_eq!(seen.code, 1, "the glued line is unreadable");
    let seen = case.step(show("first"));
    assert_eq!(seen.code, 1);
    case.step(update(
        "first",
        UpdatePrompt {
            description: some("d"),
            ..up()
        },
    ));

    // The same for schemas.
    let mut case = Case::new();
    case.step(Step::Init);
    let schema = schema_record("schema-a1", "s", &["Task"], &[], STAMP);
    case.step(write_file(
        ".luuna/beacon/schemas.jsonl",
        schema.trim_end_matches('\n').as_bytes(),
    ));
    case.step(Step::SchemaList);
    case.step(schema_create("t", &["Task"], &[]));
    let seen = case.step(Step::SchemaList);
    assert_eq!(seen.code, 1);
}

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with --ignored, see crates/luuna-beacon/README.md"]
fn raw_and_escaped_html_input_both_accepted() {
    let raw = "{\"id\":\"luuna-beacon-a1\",\"name\":\"raw\",\"description\":\"<d> & \u{2028}\",\"version\":1,\"sections\":[{\"name\":\"T\",\"body\":\"<b>&</b> \u{2028}\u{2029} \u{7f}\"}],\"status\":\"active\",\"createdAt\":\"2024-01-01T00:00:00Z\",\"updatedAt\":\"2024-01-01T00:00:00Z\"}\n";
    let escaped = r#"{"id":"luuna-beacon-a2","name":"escaped","description":"<d> &  ","version":1,"sections":[{"name":"T","body":"<b>&</b>    \u007f \/ \b\f"}],"status":"active","createdAt":"2024-01-01T00:00:00Z","updatedAt":"2024-01-01T00:00:00Z"}
"#;
    let mut case = Case::new();
    case.step(Step::Init);
    case.step(write_file(
        ".luuna/beacon/prompts.jsonl",
        format!("{raw}{escaped}").as_bytes(),
    ));
    case.step(list());
    case.step(show("raw"));
    case.step(show("escaped"));
    case.step(render("raw", &["raw"]));
    case.step(render("escaped", &["escaped"]));
    // Rewriting normalises both to Go's escaped form.
    case.step(update(
        "raw",
        UpdatePrompt {
            description: some("r"),
            ..up()
        },
    ));
    case.step(update(
        "escaped",
        UpdatePrompt {
            description: some("e"),
            ..up()
        },
    ));
    case.step(history("raw", None));
    case.step(history("escaped", None));
}

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with --ignored, see crates/luuna-beacon/README.md"]
fn lock_files_are_shared_with_go() {
    // A stale lock (older than 30 s) is removed and the write goes ahead, for
    // each of the two store files that take locks here.
    let mut case = Case::new();
    case.step(Step::Init);
    for (lock, step) in [
        (
            ".luuna/beacon/prompts.jsonl.lock",
            create("a", &[("Task", "x")]),
        ),
        (
            ".luuna/beacon/schemas.jsonl.lock",
            schema_create("s", &["Task"], &[]),
        ),
    ] {
        case.step(write_file(lock, b""));
        case.step(set_mtime(lock, 60));
        let seen = case.step(step);
        assert_eq!(seen.code, 0);
        assert!(!case.go_root().join(lock).exists(), "Go left {lock}");
        assert!(
            !case.lib_root().join(lock).exists(),
            "the library left {lock}"
        );
    }

    // A fresh lock held by someone else: both wait out the 5 s timeout and
    // fail with the same message, and the foreign lock is left alone.
    let lock = ".luuna/beacon/prompts.jsonl.lock";
    case.step(write_file(lock, b""));
    let seen = case.step(create("b", &[("Task", "y")]));
    assert_eq!(seen.code, 1);
    assert_eq!(
        seen.error.as_deref(),
        Some("timeout acquiring lock on <root>/.luuna/beacon/prompts.jsonl after 5000ms")
    );
    assert!(case.go_root().join(lock).exists());
    assert!(case.lib_root().join(lock).exists());

    // Alternating on one store leaves no lock behind.
    let shared = Shared::new();
    go_ok(&shared, Step::Init);
    go_ok(&shared, create("a", &[("Task", "x")]));
    lib_ok(&shared, create("b", &[("Task", "y")]));
    go_ok(&shared, schema_create("s", &["Task"], &[]));
    lib_ok(&shared, schema_create("t", &["Task"], &[]));
    for relative in store_paths(shared.root()) {
        assert!(!relative.ends_with(".lock"), "{relative} remains");
        assert!(!relative.contains(".tmp"), "{relative} remains");
    }
}
