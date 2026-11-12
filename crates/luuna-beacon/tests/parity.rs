//! Parity of the library with the Go tool, step by step.
//!
//! Every test runs a typed sequence of steps (tests/support/case.rs) against
//! the Go binary built from the reference clone and against the library, in
//! two fresh project roots, and compares exit status, stdout documents, error
//! strings and the store after every step. Run:
//! LUUNA_BEACON_REFERENCE=<clone> cargo test -p luuna-beacon --locked --test parity -- --ignored
//!
//! These tests are written against the library contract of the plan and
//! compile once the library exists.

mod support;

use luuna_beacon::{CreatePrompt, ListFilter, UpdatePrompt};
use serde_json::json;
use support::case::*;

fn up() -> UpdatePrompt {
    UpdatePrompt::default()
}

fn some(text: &str) -> Option<String> {
    Some(text.to_owned())
}

/// A case whose project has been initialised on both sides.
fn ready() -> Case {
    let mut case = Case::new();
    case.step(Step::Init);
    case
}

fn store_names(case: &Case) -> Vec<String> {
    store_paths(case.lib_root())
}

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with --ignored, see crates/luuna-beacon/README.md"]
fn init_creates_the_store_layout() {
    let mut case = Case::new();
    let seen = case.step(Step::Init);
    assert_eq!(seen.code, 0);
    assert_eq!(
        store_names(&case),
        [
            ".gitattributes",
            ".luuna/beacon/.gitignore",
            ".luuna/beacon/artifacts.jsonl",
            ".luuna/beacon/config.yaml",
            ".luuna/beacon/prompts.jsonl",
            ".luuna/beacon/schemas.jsonl",
        ]
    );
    let attrs = std::fs::read_to_string(case.lib_root().join(".gitattributes")).unwrap();
    assert_eq!(
        attrs,
        ".luuna/beacon/prompts.jsonl merge=union\n.luuna/beacon/schemas.jsonl merge=union\n.luuna/beacon/artifacts.jsonl merge=union\n"
    );
}

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with --ignored, see crates/luuna-beacon/README.md"]
fn init_twice_is_refused() {
    let mut case = ready();
    let seen = case.step(Step::Init);
    assert_eq!(seen.code, 1);
    assert_eq!(seen.error.as_deref(), Some(".luuna/beacon/ already exists"));
}

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with --ignored, see crates/luuna-beacon/README.md"]
fn init_appends_gitattributes_after_a_missing_final_newline() {
    let mut case = Case::new();
    case.step(write_file(".gitattributes", b"*.png binary"));
    case.step(Step::Init);
    let attrs = std::fs::read_to_string(case.lib_root().join(".gitattributes")).unwrap();
    assert!(attrs.starts_with("*.png binary\n.luuna/beacon/prompts.jsonl merge=union\n"));
}

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with --ignored, see crates/luuna-beacon/README.md"]
fn init_keeps_existing_gitattributes_entries() {
    let mut case = Case::new();
    case.step(write_file(
        ".gitattributes",
        b"*.png binary\n.luuna/beacon/prompts.jsonl merge=union\n",
    ));
    case.step(write_file(".gitignore", b"target/\n"));
    case.step(Step::Init);
    let attrs = std::fs::read_to_string(case.lib_root().join(".gitattributes")).unwrap();
    assert_eq!(attrs.matches("prompts.jsonl").count(), 1);
    assert!(attrs.contains("schemas.jsonl merge=union"));
    assert!(attrs.contains("artifacts.jsonl merge=union"));
}

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with --ignored, see crates/luuna-beacon/README.md"]
fn list_without_init_is_empty() {
    let mut case = Case::new();
    let seen = case.step(list());
    assert_eq!(seen.code, 0);
    assert_eq!(seen.raw[0]["count"], json!(0));
    assert!(store_names(&case).is_empty(), "list must not create files");
    case.step(Step::SchemaList);
    case.step(Step::ValidateAll);
}

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with --ignored, see crates/luuna-beacon/README.md"]
fn create_minimal_prompt() {
    let mut case = ready();
    case.step(create("review", &[("Task", "inspect the changes")]));
    case.step(list());
    case.step(show("review"));
    case.step(history("review", None));
}

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with --ignored, see crates/luuna-beacon/README.md"]
fn create_with_every_field() {
    let mut case = ready();
    case.step(create("parent", &[("Context", "base context")]));
    case.step(create("mixin-a", &[("Style", "terse")]));
    case.step(create("mixin-b", &[("Tone", "plain")]));
    case.step(create_with(CreatePrompt {
        name: "full".into(),
        description: "A prompt with every field".into(),
        extends: "parent".into(),
        mixins: strings(&["mixin-a", "mixin-b"]),
        tags: strings(&["command", "review"]),
        schema: "no-such-schema-yet".into(),
        emit_as: "full.md".into(),
        emit_dir: "out/dir".into(),
        status: "draft".into(),
        sections: vec![sec("Task", "do it"), sec("Notes", "two\nlines")],
        frontmatter: pairs(&[("owner", "me"), ("level", "3"), ("empty", "")]),
    }));
    case.step(show("full"));
    case.step(list());
    case.step(render("full", &["full", "parent", "mixin-a", "mixin-b"]));
}

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with --ignored, see crates/luuna-beacon/README.md"]
fn create_refuses_a_live_duplicate_but_allows_an_archived_name() {
    let mut case = ready();
    case.step(create("a", &[("Task", "first")]));
    let seen = case.step(create("a", &[("Task", "second")]));
    assert_eq!(
        seen.error.as_deref(),
        Some("prompt name \"a\" already exists")
    );
    case.step(archive("a"));
    let seen = case.step(create("a", &[("Task", "third")]));
    assert_eq!(seen.code, 0);
    case.step(list());
    case.step(show("a"));
    case.step(history("a", None));
}

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with --ignored, see crates/luuna-beacon/README.md"]
fn create_refuses_missing_parent_and_mixin() {
    let mut case = ready();
    case.step(create("base", &[("Task", "x")]));
    let seen = case.step(create_with(CreatePrompt {
        name: "child".into(),
        extends: "ghost".into(),
        ..Default::default()
    }));
    assert_eq!(
        seen.error.as_deref(),
        Some("parent prompt \"ghost\" not found")
    );
    let seen = case.step(create_with(CreatePrompt {
        name: "child".into(),
        extends: "base".into(),
        mixins: strings(&["base", "ghost"]),
        ..Default::default()
    }));
    assert_eq!(
        seen.error.as_deref(),
        Some("mixin prompt \"ghost\" not found")
    );
    let seen = case.step(create("", &[("Task", "x")]));
    assert_eq!(seen.error.as_deref(), Some("--name is required"));
    case.step(list());
}

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with --ignored, see crates/luuna-beacon/README.md"]
fn create_refuses_invalid_status() {
    let mut case = ready();
    for status in ["bogus", "archived", "Active"] {
        let seen = case.step(create_with(CreatePrompt {
            name: "p".into(),
            status: status.into(),
            ..Default::default()
        }));
        assert_eq!(seen.error, Some(format!("invalid status: {status}")));
    }
    case.step(list());
}

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with --ignored, see crates/luuna-beacon/README.md"]
fn create_without_config_fails() {
    let mut case = Case::new();
    let seen = case.step_os_error(
        create("a", &[("Task", "x")]),
        "open <root>/.luuna/beacon/config.yaml: ",
    );
    assert_eq!(seen.code, 1);
    assert!(store_names(&case).is_empty());
}

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with --ignored, see crates/luuna-beacon/README.md"]
fn update_upserts_sections_frontmatter_tags_and_mixins() {
    let mut case = ready();
    case.step(create("base", &[("Context", "ctx")]));
    case.step(create("mix", &[("Style", "terse")]));
    case.step(create_with(CreatePrompt {
        name: "a".into(),
        tags: strings(&["one", "two"]),
        sections: vec![sec("Task", "first"), sec("Notes", "n1")],
        frontmatter: pairs(&[("keep", "1"), ("drop", "2")]),
        ..Default::default()
    }));
    case.step(update(
        "a",
        UpdatePrompt {
            description: some("now described"),
            extends: some("base"),
            schema: some("sch"),
            emit_as: some("a.md"),
            emit_dir: some("emit"),
            add_tags: strings(&["two", "three"]),
            remove_tags: strings(&["one"]),
            sections: vec![sec("Task", "second"), sec("Added", "new")],
            add_mixins: strings(&["mix", "mix"]),
            frontmatter: pairs(&[("new", "x"), ("keep", "changed")]),
            remove_frontmatter: strings(&["drop"]),
            ..up()
        },
    ));
    case.step(show("a"));
    // Clearing optional fields and the last frontmatter key.
    case.step(update(
        "a",
        UpdatePrompt {
            description: some(""),
            extends: some(""),
            schema: some(""),
            remove_tags: strings(&["two", "three"]),
            remove_mixins: strings(&["mix"]),
            remove_frontmatter: strings(&["new", "keep"]),
            ..up()
        },
    ));
    case.step(show("a"));
    // Empty name and status mean unchanged.
    case.step(update(
        "a",
        UpdatePrompt {
            name: some(""),
            status: some(""),
            ..up()
        },
    ));
    case.step(update(
        "a",
        UpdatePrompt {
            status: some("draft"),
            ..up()
        },
    ));
    case.step(history("a", None));
    case.step(list());
}

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with --ignored, see crates/luuna-beacon/README.md"]
fn update_remove_section_with_and_without_extends() {
    let mut case = ready();
    case.step(create("base", &[("Context", "ctx"), ("Style", "terse")]));
    case.step(create("plain", &[("Task", "t"), ("Notes", "n")]));
    case.step(create_with(CreatePrompt {
        name: "child".into(),
        extends: "base".into(),
        sections: vec![sec("Task", "child task"), sec("Style", "loud")],
        ..Default::default()
    }));
    // Without a parent the section is removed.
    case.step(update(
        "plain",
        UpdatePrompt {
            remove_sections: strings(&["Notes", "Missing"]),
            ..up()
        },
    ));
    case.step(show("plain"));
    // With a parent the section's body is blanked (or an empty one added).
    case.step(update(
        "child",
        UpdatePrompt {
            remove_sections: strings(&["Style", "Context"]),
            ..up()
        },
    ));
    case.step(show("child"));
    case.step(render("child", &["child", "base"]));
}

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with --ignored, see crates/luuna-beacon/README.md"]
fn update_rename_keeps_id_and_lookup_follows_the_new_name() {
    let mut case = ready();
    let created = case.step(create("old", &[("Task", "x")]));
    let renamed = case.step(update(
        "old",
        UpdatePrompt {
            name: some("new"),
            ..up()
        },
    ));
    assert_eq!(created.raw[0]["id"], renamed.raw[0]["prompt"]["id"]);
    assert_eq!(renamed.raw[0]["version"], json!(2));
    case.step(show("new"));
    case.step(show("old"));
    case.step(show("new@1"));
    case.step(show("new@2"));
    case.step(history("new", None));
    case.step(history("old", None));
    case.step(list());
    case.step(render("new", &["new"]));
    case.step(render("old", &[]));
}

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with --ignored, see crates/luuna-beacon/README.md"]
fn update_refuses_invalid_status_and_unknown_prompt() {
    let mut case = ready();
    case.step(create("a", &[("Task", "x")]));
    let seen = case.step(update(
        "a",
        UpdatePrompt {
            status: some("bogus"),
            ..up()
        },
    ));
    assert_eq!(seen.error.as_deref(), Some("invalid status: bogus"));
    let seen = case.step(update(
        "ghost",
        UpdatePrompt {
            description: some("d"),
            ..up()
        },
    ));
    assert_eq!(seen.error.as_deref(), Some("prompt \"ghost\" not found"));
    // A status change to archived through update is accepted by Go.
    case.step(update(
        "a",
        UpdatePrompt {
            status: some("archived"),
            ..up()
        },
    ));
    case.step(list());
    case.step(Step::List(ListFilter {
        status: some("archived"),
        ..Default::default()
    }));
}

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with --ignored, see crates/luuna-beacon/README.md"]
fn archive_hides_from_default_list_and_list_filters_apply() {
    let mut case = ready();
    case.step(create_with(CreatePrompt {
        name: "base".into(),
        tags: strings(&["core"]),
        sections: vec![sec("Task", "x")],
        ..Default::default()
    }));
    case.step(create("mix", &[("Style", "s")]));
    case.step(create_with(CreatePrompt {
        name: "child".into(),
        extends: "base".into(),
        mixins: strings(&["mix"]),
        tags: strings(&["core", "extra"]),
        ..Default::default()
    }));
    case.step(create_with(CreatePrompt {
        name: "wip".into(),
        status: "draft".into(),
        ..Default::default()
    }));
    case.step(archive("mix"));
    case.step(list());
    for filter in [
        ListFilter {
            status: some("archived"),
            ..Default::default()
        },
        ListFilter {
            status: some("draft"),
            ..Default::default()
        },
        ListFilter {
            status: some("active"),
            ..Default::default()
        },
        ListFilter {
            tag: some("core"),
            ..Default::default()
        },
        ListFilter {
            tag: some("nothing"),
            ..Default::default()
        },
        ListFilter {
            extends: some("base"),
            ..Default::default()
        },
        ListFilter {
            mixin: some("mix"),
            ..Default::default()
        },
        ListFilter {
            tag: some("core"),
            extends: some("base"),
            mixin: some("mix"),
            status: some("active"),
        },
    ] {
        case.step(Step::List(filter));
    }
    case.step(show("mix"));
}

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with --ignored, see crates/luuna-beacon/README.md"]
fn archive_twice_is_refused() {
    let mut case = ready();
    case.step(create("a", &[("Task", "x")]));
    case.step(archive("a"));
    let seen = case.step(archive("a"));
    assert_eq!(
        seen.error.as_deref(),
        Some("prompt \"a\" is already archived")
    );
    let seen = case.step(archive("ghost"));
    assert_eq!(seen.error.as_deref(), Some("prompt \"ghost\" not found"));
    case.step(history("a", None));
}

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with --ignored, see crates/luuna-beacon/README.md"]
fn show_current_and_exact_version() {
    let mut case = ready();
    case.step(create("a", &[("Task", "v1")]));
    case.step(update(
        "a",
        UpdatePrompt {
            sections: vec![sec("Task", "v2")],
            ..up()
        },
    ));
    case.step(update(
        "a",
        UpdatePrompt {
            sections: vec![sec("Task", "v3")],
            ..up()
        },
    ));
    for reference in ["a", "a@1", "a@2", "a@3", "a@4", "ghost", "a@zero"] {
        case.step(show(reference));
    }
}

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with --ignored, see crates/luuna-beacon/README.md"]
fn history_is_newest_first_and_limited() {
    let mut case = ready();
    case.step(create("a", &[("Task", "v1")]));
    for body in ["v2", "v3", "v4"] {
        case.step(update(
            "a",
            UpdatePrompt {
                sections: vec![sec("Task", body)],
                ..up()
            },
        ));
    }
    case.step(history("a", None));
    case.step(history("a", Some(2)));
    case.step(history("a", Some(1)));
    case.step(history("a", Some(10)));
    case.step(history("a@2", None));
    // A limit of zero leaves no version and is reported as not found.
    case.step(history("a", Some(0)));
    case.step(history("ghost", None));
}

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with --ignored, see crates/luuna-beacon/README.md"]
fn render_merges_parent_mixins_and_focal() {
    let mut case = ready();
    case.step(create_with(CreatePrompt {
        name: "base".into(),
        sections: vec![sec("Context", "base context"), sec("Style", "base style")],
        frontmatter: pairs(&[("from", "base"), ("base-only", "1")]),
        ..Default::default()
    }));
    case.step(create_with(CreatePrompt {
        name: "mix".into(),
        sections: vec![sec("Style", "mix style"), sec("Extra", "mix extra")],
        frontmatter: pairs(&[("from", "mix")]),
        ..Default::default()
    }));
    case.step(create_with(CreatePrompt {
        name: "blank".into(),
        sections: vec![sec("Context", "")],
        ..Default::default()
    }));
    case.step(create_with(CreatePrompt {
        name: "focal".into(),
        extends: "base".into(),
        mixins: strings(&["mix", "blank"]),
        sections: vec![sec("Task", "focal task"), sec("Extra", "focal extra")],
        frontmatter: pairs(&[("focal", "yes")]),
        ..Default::default()
    }));
    case.step(render("focal", &["focal", "base", "mix", "blank"]));
    case.step(render("focal@1", &["focal", "base", "mix", "blank"]));
    case.step(render("base", &["base"]));
    case.step(render("ghost", &[]));
}

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with --ignored, see crates/luuna-beacon/README.md"]
fn render_merges_atlas_only_when_extends_atlas() {
    let mut case = ready();
    let atlas = |domains: &[&str], files: &[&str], budget: f64, on_empty: &str| json!({"prime": {"domains": domains, "files": files}, "budget": budget, "on_empty": on_empty});
    let lines = [
        prompt_record(
            "luuna-beacon-a1",
            "p",
            1,
            STAMP,
            &[("Context", "parent")],
            json!({"atlas": atlas(&["d1"], &["f1"], 100.0, "warn")}),
        ),
        prompt_record(
            "luuna-beacon-a2",
            "m",
            1,
            STAMP,
            &[("Style", "mix")],
            json!({"atlas": {"prime": {"domains": ["d2", "d1"]}, "on_empty": "skip"}}),
        ),
        prompt_record(
            "luuna-beacon-a3",
            "merged",
            1,
            STAMP,
            &[("Task", "focal")],
            json!({
                "extends": "p",
                "mixins": ["m"],
                "extends_atlas": true,
                "atlas": {"prime": {"domains": ["d3"]}, "budget": 50.0},
            }),
        ),
        prompt_record(
            "luuna-beacon-a4",
            "own",
            1,
            STAMP,
            &[("Task", "focal")],
            json!({
                "extends": "p",
                "mixins": ["m"],
                "atlas": {"prime": {"domains": ["d3"]}, "budget": 50.0},
            }),
        ),
        prompt_record(
            "luuna-beacon-a5",
            "falsy",
            1,
            STAMP,
            &[("Task", "focal")],
            json!({"extends": "p", "extends_atlas": false}),
        ),
        prompt_record(
            "luuna-beacon-a6",
            "inherits",
            1,
            STAMP,
            &[("Task", "focal")],
            json!({"extends": "p", "extends_atlas": true}),
        ),
    ]
    .concat();
    case.step(write_file(".luuna/beacon/prompts.jsonl", lines.as_bytes()));
    case.step(render("merged", &["merged", "p", "m"]));
    case.step(render("own", &["own", "p", "m"]));
    case.step(render("falsy", &["falsy", "p"]));
    case.step(render("inherits", &["inherits", "p"]));
    case.step(render_json("merged"));
    case.step(render_json("own"));
    case.step(render("p", &["p"]));
}

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with --ignored, see crates/luuna-beacon/README.md"]
fn render_refuses_an_old_version_and_accepts_the_current_one() {
    let mut case = ready();
    case.step(create("a", &[("Task", "v1")]));
    case.step(update(
        "a",
        UpdatePrompt {
            sections: vec![sec("Task", "v2")],
            ..up()
        },
    ));
    case.step(render("a@1", &[]));
    case.step(render("a@2", &["a"]));
    case.step(render("a", &["a"]));
    case.step(render("a@3", &[]));
    // Go reads the version with Sscanf("%d"): trailing text is accepted.
    case.step(render("a@2x", &["a"]));
    case.step(render("a@x", &[]));
    case.step(render("a@", &[]));
}

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with --ignored, see crates/luuna-beacon/README.md"]
fn render_reports_cycles_and_excess_depth() {
    let mut case = ready();
    case.step(create("a", &[("Task", "a")]));
    case.step(create_with(CreatePrompt {
        name: "b".into(),
        extends: "a".into(),
        ..Default::default()
    }));
    case.step(update(
        "a",
        UpdatePrompt {
            extends: some("b"),
            ..up()
        },
    ));
    let seen = case.step(render("a", &[]));
    assert_eq!(seen.code, 1);
    case.step(render("b", &[]));
    // A mixin that is its own ancestor.
    case.step(create("m", &[("Style", "s")]));
    case.step(update(
        "m",
        UpdatePrompt {
            add_mixins: strings(&["m"]),
            ..up()
        },
    ));
    case.step(render("m", &[]));

    // A chain of six prompts renders; a seventh is one level too deep.
    case.step(create("c0", &[("Task", "root")]));
    for index in 1..=6 {
        case.step(create_with(CreatePrompt {
            name: format!("c{index}"),
            extends: format!("c{}", index - 1),
            sections: vec![sec(&format!("S{index}"), "x")],
            ..Default::default()
        }));
    }
    case.step(render("c5", &["c5", "c4", "c3", "c2", "c1", "c0"]));
    let seen = case.step(render("c6", &[]));
    assert_eq!(
        seen.error.as_deref(),
        Some("maximum inheritance depth exceeded")
    );
}

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with --ignored, see crates/luuna-beacon/README.md"]
fn render_format_json_prints_the_bare_prompt() {
    let mut case = ready();
    case.step(create_with(CreatePrompt {
        name: "base".into(),
        sections: vec![sec("Context", "ctx")],
        frontmatter: pairs(&[("k", "v")]),
        ..Default::default()
    }));
    case.step(create_with(CreatePrompt {
        name: "child".into(),
        description: "d".into(),
        extends: "base".into(),
        tags: strings(&["t"]),
        sections: vec![sec("Task", "t")],
        ..Default::default()
    }));
    let seen = case.step(render_json("child"));
    assert!(seen.raw[0].get("resolvedFrom").is_none());
    assert!(
        seen.raw[0].get("success").is_none(),
        "the bare record has no envelope"
    );
    case.step(render_json("base"));
    case.step(render_json("base@1"));
    case.step(render_json("ghost"));
}

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with --ignored, see crates/luuna-beacon/README.md"]
fn render_resolved_from_lists_every_source_in_walk_order() {
    let mut case = ready();

    // A prompt with no parent: one entry.
    case.step(create("solo", &[("Task", "alone")]));
    case.step(render("solo", &["solo"]));

    // A parent chain of depth 5.
    case.step(create("c0", &[("Task", "root")]));
    for index in 1..=5 {
        case.step(create_with(CreatePrompt {
            name: format!("c{index}"),
            extends: format!("c{}", index - 1),
            sections: vec![sec(&format!("S{index}"), "x")],
            ..Default::default()
        }));
    }
    case.step(render("c5", &["c5", "c4", "c3", "c2", "c1", "c0"]));

    // Mixins that themselves extend a parent.
    case.step(create("pbase", &[("Style", "base style")]));
    case.step(create_with(CreatePrompt {
        name: "mx".into(),
        extends: "pbase".into(),
        sections: vec![sec("Extra", "mx extra")],
        ..Default::default()
    }));
    case.step(create_with(CreatePrompt {
        name: "uses-mx".into(),
        mixins: strings(&["mx"]),
        sections: vec![sec("Task", "t")],
        ..Default::default()
    }));
    case.step(render("uses-mx", &["uses-mx", "mx", "pbase"]));

    // One mixin reached twice is listed once.
    case.step(create("m1", &[("A", "a")]));
    case.step(create("m2", &[("B", "b")]));
    case.step(create_with(CreatePrompt {
        name: "pp".into(),
        mixins: strings(&["m1"]),
        sections: vec![sec("P", "p")],
        ..Default::default()
    }));
    case.step(create_with(CreatePrompt {
        name: "twice".into(),
        extends: "pp".into(),
        mixins: strings(&["m1", "m2"]),
        sections: vec![sec("Task", "t")],
        ..Default::default()
    }));
    case.step(render("twice", &["twice", "pp", "m1", "m2"]));

    // An archived parent is listed, although list output omits it.
    case.step(create("gone", &[("Task", "old")]));
    case.step(create_with(CreatePrompt {
        name: "orphan".into(),
        extends: "gone".into(),
        sections: vec![sec("Task", "t")],
        ..Default::default()
    }));
    case.step(archive("gone"));
    case.step(render("orphan", &["orphan", "gone"]));

    // render --format json is the bare record: no resolvedFrom on either side.
    for reference in ["twice", "orphan", "c5"] {
        let seen = case.step(render_json(reference));
        assert!(seen.raw[0].get("resolvedFrom").is_none());
        assert!(seen.lib_raw[0].get("resolvedFrom").is_none());
    }
}

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with --ignored, see crates/luuna-beacon/README.md"]
fn schema_create_list_show() {
    let mut case = ready();
    case.step(schema_create("s1", &["Task", "Notes"], &["Extra"]));
    case.step(schema_create("s2", &[], &[]));
    case.step(schema_create("s3", &["Only"], &["A", "B"]));
    case.step(Step::SchemaList);
    case.step(schema_show("s1"));
    case.step(schema_show("s2"));
    case.step(schema_show("s3"));
    let seen = case.step(schema_show("ghost"));
    assert_eq!(seen.error.as_deref(), Some("schema \"ghost\" not found"));
    let seen = case.step(schema_create("s1", &["x"], &[]));
    assert_eq!(
        seen.error.as_deref(),
        Some("schema name \"s1\" already exists")
    );
    let seen = case.step(schema_create("", &["x"], &[]));
    assert_eq!(seen.error.as_deref(), Some("--name is required"));
}

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with --ignored, see crates/luuna-beacon/README.md"]
fn schema_rule_add_and_invalid_pattern() {
    let mut case = ready();
    case.step(schema_create("s", &["Task"], &[]));
    case.step(rule_add("s", "Task", "^do", "must start with do"));
    case.step(rule_add("s", "Task", "(?i)done$", "must end with done"));
    case.step(schema_show("s"));
    // The pattern is compiled before the schema is looked up.
    for pattern in ["(", "(?=x)", "a{1001}", "[z-a]", "\\"] {
        let seen = case.step(rule_add("s", "Task", pattern, "m"));
        assert_eq!(seen.code, 1);
    }
    let seen = case.step(rule_add("ghost", "Task", "x", "m"));
    assert_eq!(seen.error.as_deref(), Some("schema \"ghost\" not found"));
    let seen = case.step(rule_add("s", "", "x", "m"));
    assert_eq!(
        seen.error.as_deref(),
        Some("--section, --pattern, and --message are required")
    );
    case.step(schema_show("s"));
    case.step(Step::SchemaList);
}

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with --ignored, see crates/luuna-beacon/README.md"]
fn validate_named_and_all_including_duplicate_schema_records() {
    let mut case = ready();
    case.step(schema_create("s", &["Task", "Notes"], &[]));
    case.step(create_with(CreatePrompt {
        name: "p".into(),
        schema: "s".into(),
        sections: vec![sec("Task", "do it")],
        ..Default::default()
    }));
    case.step(create("free", &[("Task", "no schema")]));
    case.step(create_with(CreatePrompt {
        name: "dangling".into(),
        schema: "no-such-schema".into(),
        sections: vec![sec("Task", "x")],
        ..Default::default()
    }));
    let seen = case.step(validate("p"));
    assert_eq!(seen.code, 1);
    case.step(validate("free"));
    case.step(validate("dangling"));
    case.step(Step::ValidateAll);

    // Each rule is a new stored record of the schema; Go validates against
    // every record, so the missing-section error repeats once per record.
    case.step(rule_add("s", "Task", "^do", "must start with do"));
    case.step(validate("p"));
    case.step(rule_add("s", "Task", "done$", "must end with done"));
    let seen = case.step(validate("p"));
    assert_eq!(seen.code, 1);
    case.step(Step::ValidateAll);

    case.step(update(
        "p",
        UpdatePrompt {
            sections: vec![sec("Notes", "n"), sec("Task", "do it and be done")],
            ..up()
        },
    ));
    let seen = case.step(validate("p"));
    assert_eq!(seen.code, 0);
    case.step(Step::ValidateAll);
    case.step(validate("ghost"));
    case.step(archive("p"));
    case.step(Step::ValidateAll);
}

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with --ignored, see crates/luuna-beacon/README.md"]
fn validate_named_adds_runtime_fields_and_matches_go_results() {
    let mut case = ready();
    case.step(schema_create("s", &["Task"], &[]));
    case.step(rule_add("s", "Task", "ok", "must say ok"));
    case.step(create_with(CreatePrompt {
        name: "good".into(),
        schema: "s".into(),
        sections: vec![sec("Task", "this is ok")],
        ..Default::default()
    }));
    case.step(create_with(CreatePrompt {
        name: "bad".into(),
        schema: "s".into(),
        sections: vec![sec("Other", "nothing")],
        ..Default::default()
    }));
    // reconcile_runtime checks valid, errors and warnings against results[0]
    // on the library side and their absence on Go's.
    let seen = case.step(validate("good"));
    assert_eq!(seen.code, 0);
    assert!(seen.raw[0].get("valid").is_none() && seen.raw[0].get("errors").is_none());
    assert_eq!(seen.lib_raw[0]["results"][0]["valid"], json!(true));
    let seen = case.step(validate("bad"));
    assert_eq!(seen.code, 1);
    assert_eq!(
        seen.raw.len(),
        2,
        "a failed validate prints a result and an error document"
    );
    assert_eq!(seen.lib_raw.len(), 2);
    // validate --all keeps Go's shape on both sides.
    let seen = case.step(Step::ValidateAll);
    assert!(seen.lib_raw[0].get("valid").is_none());
}

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with --ignored, see crates/luuna-beacon/README.md"]
fn validate_named_resolves_over_the_whole_log_like_go() {
    let mut case = ready();
    case.step(schema_create("s", &["Task"], &[]));
    case.step(create_with(CreatePrompt {
        name: "p".into(),
        schema: "s".into(),
        sections: vec![sec("Other", "x")],
        ..Default::default()
    }));
    case.step(create("plain", &[("Task", "t")]));
    // After a rename the old name no longer shows, but validate still finds
    // the old identity because it resolves over the whole log.
    case.step(update(
        "p",
        UpdatePrompt {
            name: some("q"),
            ..up()
        },
    ));
    case.step(update(
        "plain",
        UpdatePrompt {
            name: some("plain2"),
            ..up()
        },
    ));
    case.step(show("p"));
    case.step(validate("p"));
    case.step(validate("q"));
    case.step(validate("plain"));
    case.step(validate("plain2"));
    case.step(validate("q@1"));
    case.step(Step::ValidateAll);
    // An archived prompt is still found by name.
    case.step(archive("plain2"));
    case.step(validate("plain2"));
}

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with --ignored, see crates/luuna-beacon/README.md"]
fn validate_rules_use_go_regexp_semantics() {
    let mut case = ready();
    // (section, pattern, message): the schema is written by hand so that a
    // stored pattern Go cannot compile (GD4) can sit among the rules.
    let rules = [
        ("digits", r"^\d+$", "digits rule"),
        ("word", r"^\w+$", "word rule"),
        ("space", r"^a\sb$", "space rule"),
        ("boundary", r"\bfoo\b", "boundary rule"),
        ("fold", "(?i)^hello$", "fold rule"),
        ("kelvin", "(?i)^k$", "kelvin rule"),
        ("anchor", "^b", "anchor rule"),
        ("multi", "(?m)^b", "multi rule"),
        ("tail", "^a$", "tail rule"),
        ("bracket", r"^[\d]+$", "bracket rule"),
        ("letters", r"^\pL+$", "letters rule"),
        ("negated", r"^\D+$", "negated rule"),
        ("bad", "(", "bad rule"),
    ];
    let schema = schema_record("schema-1", "g", &[], &rules, STAMP);
    case.step(write_file(".luuna/beacon/schemas.jsonl", schema.as_bytes()));

    let tricky = [
        ("digits", "\u{663}\u{663}"),
        ("word", "caf\u{e9}"),
        ("space", "a\u{a0}b"),
        ("boundary", "foo\u{e9}"),
        ("fold", "HeLLo"),
        ("kelvin", "\u{212a}"),
        ("anchor", "a\nb"),
        ("multi", "a\nb"),
        ("tail", "a\n"),
        ("bracket", "\u{663}"),
        ("letters", "\u{e9}t\u{e9}"),
        ("negated", "\u{663}"),
        ("bad", "anything"),
    ];
    case.step(create_with(CreatePrompt {
        name: "unicode".into(),
        schema: "g".into(),
        sections: tricky.iter().map(|(n, b)| sec(n, b)).collect(),
        ..Default::default()
    }));
    let passing = [
        ("digits", "123"),
        ("word", "cafe"),
        ("space", "a b"),
        ("boundary", "a foo b"),
        ("fold", "hello"),
        ("kelvin", "k"),
        ("anchor", "b"),
        ("multi", "a\nb"),
        ("tail", "a"),
        ("bracket", "42"),
        ("letters", "abc"),
        ("negated", "abc"),
        ("bad", "anything"),
    ];
    case.step(create_with(CreatePrompt {
        name: "ascii".into(),
        schema: "g".into(),
        sections: passing.iter().map(|(n, b)| sec(n, b)).collect(),
        ..Default::default()
    }));
    let seen = case.step(validate("unicode"));
    assert_eq!(seen.code, 1);
    let seen = case.step(validate("ascii"));
    assert_eq!(seen.code, 0);
    case.step(Step::ValidateAll);
}

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with --ignored, see crates/luuna-beacon/README.md"]
fn updated_at_ties_are_broken_by_text_comparison_like_go() {
    let mut case = ready();
    // Equal version; ".1Z" sorts after ".12Z" as text although it is the
    // earlier instant. Go picks by text.
    let prompts = [
        prompt_record(
            "luuna-beacon-b1",
            "tie",
            1,
            "2024-01-01T00:00:00.12Z",
            &[("Task", "longer fraction")],
            json!({}),
        ),
        prompt_record(
            "luuna-beacon-b2",
            "tie",
            1,
            "2024-01-01T00:00:00.1Z",
            &[("Task", "shorter fraction")],
            json!({}),
        ),
        prompt_record(
            "luuna-beacon-b3",
            "other",
            1,
            "2024-01-01T00:00:00.5Z",
            &[("Task", "first")],
            json!({}),
        ),
        prompt_record(
            "luuna-beacon-b4",
            "other",
            1,
            "2024-01-01T00:00:00.45Z",
            &[("Task", "second")],
            json!({}),
        ),
    ]
    .concat();
    // Schemas: same id, so the later record wins only if its text is greater.
    let schemas = [
        schema_record(
            "schema-b1",
            "t",
            &["Task"],
            &[("Task", "longer", "rule from the .12 record")],
            "2024-01-01T00:00:00.12Z",
        ),
        schema_record(
            "schema-b1",
            "t",
            &["Task"],
            &[("Task", "shorter", "rule from the .1 record")],
            "2024-01-01T00:00:00.1Z",
        ),
        schema_record(
            "schema-b2",
            "u",
            &["Task"],
            &[("Task", "second", "rule from the .1 record")],
            "2024-01-01T00:00:00.1Z",
        ),
        schema_record(
            "schema-b2",
            "u",
            &["Task"],
            &[("Task", "first", "rule from the .12 record")],
            "2024-01-01T00:00:00.12Z",
        ),
    ]
    .concat();
    case.step(write_file(
        ".luuna/beacon/prompts.jsonl",
        prompts.as_bytes(),
    ));
    case.step(write_file(
        ".luuna/beacon/schemas.jsonl",
        schemas.as_bytes(),
    ));
    case.step(list());
    case.step(show("tie"));
    case.step(show("tie@1"));
    case.step(show("other"));
    case.step(render("tie", &["tie"]));
    case.step(render("other", &["other"]));
    case.step(Step::SchemaList);
    case.step(schema_show("t"));
    case.step(schema_show("u"));
    // update works from the record Go picked.
    case.step(update(
        "tie",
        UpdatePrompt {
            description: some("d"),
            ..up()
        },
    ));
    case.step(history("tie", None));
}

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with --ignored, see crates/luuna-beacon/README.md"]
fn html_control_and_unicode_text_in_bodies() {
    let mut case = ready();
    let body = "<script>alert(\"x\") && 'y'</script> a&b >c\u{2028}\u{2029}end\ttab\u{1}\u{1f}\u{7f}\u{85}\u{a0}\u{feff}\r\n\u{1f600} caf\u{e9} \u{65e5}\u{672c}\\ back\\slash";
    case.step(create_with(CreatePrompt {
        name: "html<&>".into(),
        description: "d <b>&amp;</b> \u{2028}".into(),
        tags: strings(&["<tag>", "t&t"]),
        sections: vec![sec("Body <1>", body), sec("Second", "plain")],
        frontmatter: pairs(&[("k<>", "v&\u{2028}"), ("emoji", "\u{1f600}")]),
        ..Default::default()
    }));
    case.step(show("html<&>"));
    case.step(list());
    case.step(render("html<&>", &["html<&>"]));
    case.step(render_json("html<&>"));
    case.step(update(
        "html<&>",
        UpdatePrompt {
            sections: vec![sec("Body <1>", "tail <>&")],
            ..up()
        },
    ));
    case.step(history("html<&>", None));
}

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with --ignored, see crates/luuna-beacon/README.md"]
fn legacy_canopy_layout_is_migrated_or_refused() {
    // A legacy layout is moved by the first command, and root files change.
    let mut case = Case::new();
    case.step(write_file(
        ".canopy/config.yaml",
        b"project: legacy\nversion: \"1\"\n",
    ));
    case.step(write_file(".canopy/prompts.jsonl", b""));
    case.step(write_file(
        ".gitattributes",
        b".canopy/prompts.jsonl merge=union\n",
    ));
    case.step(write_file(".gitignore", b".canopy/*.lock\nbuild/\n"));
    let seen = case.step(list());
    assert!(seen.migrated, "the first command moves .canopy");
    assert!(!case.lib_root().join(".canopy").exists());
    let seen = case.step(list());
    assert!(!seen.migrated, "the move happens once");

    // init moves a legacy folder and then finds the store already there.
    let mut case = Case::new();
    case.step(write_file(
        ".canopy/config.yaml",
        b"project: legacy\nversion: \"1\"\n",
    ));
    let seen = case.step(Step::Init);
    assert!(seen.migrated);
    assert_eq!(seen.error.as_deref(), Some(".luuna/beacon/ already exists"));

    // Both layouts present: refused, nothing moved.
    let mut case = Case::new();
    case.step(write_file(".canopy/config.yaml", b"project: old\n"));
    case.step(write_file(
        ".luuna/beacon/config.yaml",
        b"project: new\nversion: \"1\"\n",
    ));
    let seen = case.step(list());
    assert_eq!(
        seen.error.as_deref(),
        Some("both .canopy and .luuna/beacon exist; merge or remove one before continuing")
    );
    assert!(!seen.migrated);

    // A symlinked legacy folder is refused.
    #[cfg(unix)]
    {
        let mut case = Case::new();
        case.step(write_file("elsewhere.txt", b"x"));
        std::os::unix::fs::symlink("elsewhere.txt", case.go_root().join(".canopy")).unwrap();
        std::os::unix::fs::symlink("elsewhere.txt", case.lib_root().join(".canopy")).unwrap();
        let seen = case.step(list());
        assert_eq!(
            seen.error.as_deref(),
            Some("refusing to migrate symlinked legacy directory")
        );
    }
}

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with --ignored, see crates/luuna-beacon/README.md"]
fn runtime_minimum_sequence() {
    // The call sequence of luuna-runtime's integrations.rs:1409-1503 and
    // 1622-1663, with the guidance body the runtime writes.
    let mut case = Case::new();
    case.step(Step::Init);
    let first = "Follow the plan.\nKeep edits small.\n\nNever skip review.\n";
    let second = "Follow the plan.\nKeep edits small, and say approved.\n";
    case.step(create_with(CreatePrompt {
        name: "guidance-demo".into(),
        sections: vec![sec("guidance", first)],
        frontmatter: pairs(&[("luuna_request", "r1")]),
        ..Default::default()
    }));
    case.step(list());
    case.step(render("guidance-demo@1", &["guidance-demo"]));
    case.step(update(
        "guidance-demo",
        UpdatePrompt {
            sections: vec![sec("guidance", second)],
            frontmatter: pairs(&[("luuna_request", "r2")]),
            ..up()
        },
    ));
    let seen = case.step(render("guidance-demo@1", &[]));
    assert_eq!(seen.code, 1, "an old version is refused");
    case.step(render("guidance-demo@2", &["guidance-demo"]));
    case.step(schema_create("s", &["guidance"], &[]));
    case.step(rule_add(
        "s",
        "guidance",
        r"\bapproved\b",
        "guidance must say approved",
    ));
    case.step(update(
        "guidance-demo",
        UpdatePrompt {
            schema: some("s"),
            ..up()
        },
    ));
    let seen = case.step(validate("guidance-demo"));
    assert_eq!(seen.code, 0);
    assert_eq!(seen.lib_raw.len(), 1);
    case.step(update(
        "guidance-demo",
        UpdatePrompt {
            sections: vec![sec("guidance", "No such word here.\n")],
            frontmatter: pairs(&[("luuna_request", "r3")]),
            ..up()
        },
    ));
    let seen = case.step(validate("guidance-demo"));
    assert_eq!(seen.code, 1);
    case.step(schema_show("s"));
    case.step(list());
    case.step(render("guidance-demo@4", &["guidance-demo"]));
}
