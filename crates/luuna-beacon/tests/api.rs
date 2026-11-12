//! Behaviour of the library that needs no Go: the shape of the API, ids,
//! timestamps, the clock, where files go, and the two runtime documents
//! (`resolvedFrom` and the top-level fields of a named validate).
//!
//! These tests are written against the library contract of the plan and
//! compile once the library exists.

mod support;

use luuna_beacon::{
    Beacon, Clock, CreatePrompt, CreateSchema, ListFilter, Rule, Section, UpdatePrompt,
};
use regex::Regex;
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use support::case::assert_runtime_source_chain;
use tempfile::TempDir;

struct FixedClock(SystemTime);

impl Clock for FixedClock {
    fn now(&self) -> SystemTime {
        self.0
    }
}

fn at(seconds: u64, nanos: u32) -> Arc<dyn Clock> {
    Arc::new(FixedClock(UNIX_EPOCH + Duration::new(seconds, nanos)))
}

/// A fresh canonical project root in the system temporary folder.
fn root() -> (TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("temporary folder");
    let path = dir.path().canonicalize().expect("canonical path");
    (dir, path)
}

fn section(name: &str, body: &str) -> Section {
    Section {
        name: name.to_owned(),
        body: body.to_owned(),
        required: None,
    }
}

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|item| (*item).to_owned()).collect()
}

/// Creates a prompt with one section.
fn make(beacon: &Beacon, name: &str, extends: &str, mixins: &[&str]) {
    beacon
        .create(&CreatePrompt {
            name: name.to_owned(),
            extends: extends.to_owned(),
            mixins: strings(mixins),
            sections: vec![section(&format!("S-{name}"), "body")],
            ..Default::default()
        })
        .unwrap_or_else(|error| panic!("create {name}: {error}"));
}

fn ready() -> (TempDir, PathBuf, Beacon) {
    let (dir, path) = root();
    let beacon = Beacon::open(&path).expect("open");
    beacon.init().expect("init");
    (dir, path, beacon)
}

fn resolved_from(beacon: &Beacon, reference: &str) -> Vec<String> {
    let rendered = beacon.render(reference).expect("render");
    assert_eq!(
        rendered.to_payload()["resolvedFrom"],
        json!(rendered.resolved_from)
    );
    rendered.resolved_from
}

#[test]
fn open_refuses_a_relative_root() {
    for relative in ["relative/project", ".", "..", ""] {
        assert!(
            Beacon::open(Path::new(relative)).is_err(),
            "{relative:?} must be refused"
        );
    }
    // An absolute path that does not exist yet is accepted and not created.
    let (_dir, path) = root();
    let missing = path.join("not-yet");
    Beacon::open(&missing).expect("an absolute root is accepted");
    assert!(!missing.exists(), "open touches no file");
}

#[test]
fn store_dir_is_under_the_given_root() {
    let (_dir, path) = root();
    let beacon = Beacon::open(&path).unwrap();
    assert_eq!(beacon.project_root(), path.as_path());
    assert_eq!(beacon.store_dir(), path.join(".luuna").join("beacon"));
    assert!(!beacon.store_dir().exists(), "store_dir is pure");
    let clone = beacon.clone().with_clock(at(1, 0));
    assert_eq!(clone.store_dir(), beacon.store_dir());
}

#[test]
fn init_then_create_then_render_round_trip() {
    let (_dir, path, beacon) = ready();
    assert_eq!(beacon.store_dir(), path.join(".luuna/beacon"));
    let created = beacon
        .create(&CreatePrompt {
            name: "review".into(),
            description: "Review code".into(),
            tags: strings(&["command"]),
            sections: vec![section("Task", "inspect"), section("Notes", "two\nlines")],
            frontmatter: vec![("owner".into(), "me".into())],
            ..Default::default()
        })
        .unwrap();
    assert_eq!(created.version, 1);
    assert_eq!(created.status, "active");
    assert_eq!(created.sections.as_ref().unwrap().len(), 2);

    let rendered = beacon.render("review").unwrap();
    assert_eq!(rendered.prompt.name, "review");
    assert_eq!(rendered.resolved_from, ["review"]);
    let payload = rendered.to_payload();
    assert_eq!(payload["success"], json!(true));
    assert_eq!(payload["command"], json!("render"));
    assert_eq!(payload["name"], json!("review"));
    assert_eq!(payload["version"], json!(1));
    assert_eq!(payload["sections"][1]["body"], json!("two\nlines"));
    assert_eq!(payload["frontmatter"], json!({"owner": "me"}));
    assert!(payload.get("atlas").is_none());

    let updated = beacon
        .update(
            "review",
            &UpdatePrompt {
                description: Some("changed".into()),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(
        (updated.version, updated.id.as_str()),
        (2, created.id.as_str())
    );
    assert_eq!(beacon.show("review").unwrap().description, "changed");
    assert_eq!(beacon.show("review@1").unwrap().description, "Review code");
    let history = beacon.history("review", 20).unwrap();
    assert_eq!(
        history
            .iter()
            .map(|prompt| prompt.version)
            .collect::<Vec<_>>(),
        [2, 1]
    );
    assert_eq!(beacon.list(&ListFilter::default()).unwrap().len(), 1);
    beacon.archive("review").unwrap();
    assert!(beacon.list(&ListFilter::default()).unwrap().is_empty());
    assert!(
        beacon.render("review@1").is_err(),
        "an old version is refused"
    );
}

#[test]
fn ids_are_project_prefixed_lowercase_hex() {
    let (_dir, path, beacon) = ready();
    let default_id = Regex::new("^luuna-beacon-[0-9a-f]+$").unwrap();
    let first = beacon
        .create(&CreatePrompt {
            name: "a".into(),
            ..Default::default()
        })
        .unwrap();
    assert!(default_id.is_match(&first.id), "{}", first.id);

    // The project name comes from config.yaml.
    std::fs::write(
        path.join(".luuna/beacon/config.yaml"),
        "project: demo\nversion: \"1\"\n",
    )
    .unwrap();
    let second = beacon
        .create(&CreatePrompt {
            name: "b".into(),
            ..Default::default()
        })
        .unwrap();
    assert!(
        Regex::new("^demo-[0-9a-f]+$").unwrap().is_match(&second.id),
        "{}",
        second.id
    );

    let schema = beacon
        .schema_create(&CreateSchema {
            name: "s".into(),
            required: strings(&["Task"]),
            optional: vec![],
        })
        .unwrap();
    assert!(
        Regex::new("^schema-[0-9a-f]+$")
            .unwrap()
            .is_match(&schema.id),
        "{}",
        schema.id
    );
}

#[test]
fn timestamps_are_utc_with_trimmed_fractions() {
    // 1704164645 is 2024-01-02T03:04:05Z.
    for (nanos, expected) in [
        (0, "2024-01-02T03:04:05Z"),
        (100_000_000, "2024-01-02T03:04:05.1Z"),
        (120_000_000, "2024-01-02T03:04:05.12Z"),
        (10_000_000, "2024-01-02T03:04:05.01Z"),
        (123_456_789, "2024-01-02T03:04:05.123456789Z"),
        (1, "2024-01-02T03:04:05.000000001Z"),
        (999_999_999, "2024-01-02T03:04:05.999999999Z"),
    ] {
        let (_dir, path) = root();
        let beacon = Beacon::open(&path)
            .unwrap()
            .with_clock(at(1_704_164_645, nanos));
        beacon.init().unwrap();
        let prompt = beacon
            .create(&CreatePrompt {
                name: "a".into(),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(prompt.created_at, expected);
        assert_eq!(prompt.updated_at, expected);
        let schema = beacon
            .schema_create(&CreateSchema {
                name: "s".into(),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(
            (schema.created_at.as_str(), schema.updated_at.as_str()),
            (expected, expected)
        );
    }

    // With the system clock the form is the same.
    let (_dir, _path, beacon) = ready();
    let form =
        Regex::new(r"^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}(\.[0-9]*[1-9])?Z$")
            .unwrap();
    for index in 0..30 {
        let prompt = beacon
            .create(&CreatePrompt {
                name: format!("p{index}"),
                ..Default::default()
            })
            .unwrap();
        assert!(form.is_match(&prompt.created_at), "{}", prompt.created_at);
        assert!(form.is_match(&prompt.updated_at), "{}", prompt.updated_at);
    }
}

fn read_store(path: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut files = BTreeMap::new();
    let mut pending = vec![path.to_path_buf()];
    while let Some(folder) = pending.pop() {
        for entry in std::fs::read_dir(&folder).unwrap() {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_dir() {
                pending.push(entry.path());
            } else {
                let relative = entry
                    .path()
                    .strip_prefix(path)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned();
                files.insert(relative, std::fs::read(entry.path()).unwrap());
            }
        }
    }
    files
}

#[test]
fn a_fixed_clock_gives_reproducible_records() {
    let run = || {
        let (dir, path) = root();
        let beacon = Beacon::open(&path)
            .unwrap()
            .with_clock(at(1_704_164_645, 123_000_000));
        beacon.init().unwrap();
        let created = beacon
            .create(&CreatePrompt {
                name: "a".into(),
                sections: vec![section("Task", "x")],
                ..Default::default()
            })
            .unwrap();
        let updated = beacon
            .update(
                "a",
                &UpdatePrompt {
                    description: Some("d".into()),
                    ..Default::default()
                },
            )
            .unwrap();
        let schema = beacon
            .schema_create(&CreateSchema {
                name: "s".into(),
                required: strings(&["Task"]),
                optional: vec![],
            })
            .unwrap();
        let files = read_store(&beacon.store_dir());
        drop(dir);
        (
            created.to_go_json(),
            updated.to_go_json(),
            schema.to_go_json(),
            files,
        )
    };
    let (first, second) = (run(), run());
    assert_eq!(first, second);
    assert!(first
        .0
        .contains("\"createdAt\":\"2024-01-02T03:04:05.123Z\""));
}

#[test]
fn nothing_is_written_outside_the_project_root() {
    let (_outer, outer) = root();
    let project = outer.join("project");
    std::fs::create_dir(&project).unwrap();
    std::fs::create_dir(outer.join("sibling")).unwrap();
    std::fs::write(outer.join("sibling/keep.txt"), "keep").unwrap();
    std::fs::write(outer.join("top.txt"), "top").unwrap();
    let before = read_store(&outer);
    let cwd = std::env::current_dir().unwrap();

    let beacon = Beacon::open(&project).unwrap();
    beacon.init().unwrap();
    make(&beacon, "base", "", &[]);
    make(&beacon, "child", "base", &[]);
    beacon
        .update(
            "child",
            &UpdatePrompt {
                name: Some("renamed".into()),
                ..Default::default()
            },
        )
        .unwrap();
    beacon.archive("base").unwrap();
    beacon.render("renamed").unwrap();
    beacon.list(&ListFilter::default()).unwrap();
    beacon.history("renamed", 5).unwrap();
    beacon
        .schema_create(&CreateSchema {
            name: "s".into(),
            required: strings(&["Task"]),
            optional: vec![],
        })
        .unwrap();
    beacon
        .schema_rule_add(
            "s",
            &Rule {
                section: "Task".into(),
                pattern: "x".into(),
                message: "m".into(),
            },
        )
        .unwrap();
    beacon.validate_all().unwrap();
    beacon.migrate_legacy().unwrap();

    let after = read_store(&outer);
    let outside: BTreeMap<_, _> = after
        .iter()
        .filter(|(path, _)| !path.starts_with("project/") && path.as_str() != "project")
        .collect();
    let outside_before: BTreeMap<_, _> = before.iter().collect();
    assert_eq!(outside, outside_before, "files outside the project changed");
    assert_eq!(std::env::current_dir().unwrap(), cwd);

    let inside: Vec<&String> = after
        .keys()
        .filter(|path| path.starts_with("project/"))
        .collect();
    assert!(!inside.is_empty());
    for path in inside {
        assert!(
            path.starts_with("project/.luuna/beacon/") || path == "project/.gitattributes",
            "unexpected file {path}"
        );
        let name = path.rsplit('/').next().unwrap();
        assert!(
            !name.starts_with(".tmp") && !name.ends_with(".lock"),
            "{path} remains"
        );
    }
}

fn listed(beacon: &Beacon) -> Vec<String> {
    beacon
        .list(&ListFilter::default())
        .unwrap()
        .into_iter()
        .map(|prompt| prompt.name)
        .collect()
}

#[test]
fn render_output_meets_the_runtime_source_chain_contract() {
    // The checks of integrations.rs:1450-1468, applied to to_payload().
    let check = |beacon: &Beacon, name: &str, expected_len: usize, archived: &[&str]| {
        let payload = beacon.render(name).unwrap().to_payload();
        let chain = payload
            .get("resolvedFrom")
            .expect("resolvedFrom is present");
        let archived: HashSet<String> = archived.iter().map(|name| (*name).to_owned()).collect();
        assert_runtime_source_chain(chain, &listed(beacon), &archived);
        assert_eq!(chain.as_array().unwrap().len(), expected_len);
        assert_eq!(chain[0], json!(name), "the focal prompt comes first");
        assert!(payload["sections"].is_array());
    };

    // A single prompt.
    let (_dir, _path, beacon) = ready();
    make(&beacon, "solo", "", &[]);
    check(&beacon, "solo", 1, &[]);

    // A chain of depth 5 with mixins.
    let (_dir, _path, beacon) = ready();
    make(&beacon, "c0", "", &[]);
    make(&beacon, "mx", "", &[]);
    make(&beacon, "my", "c0", &[]);
    for index in 1..=4 {
        let mixins: &[&str] = if index == 2 { &["mx"] } else { &[] };
        make(
            &beacon,
            &format!("c{index}"),
            &format!("c{}", index - 1),
            mixins,
        );
    }
    make(&beacon, "c5", "c4", &["my"]);
    check(&beacon, "c5", 8, &[]);

    // A chain with an archived parent: it is listed, though list omits it.
    let (_dir, _path, beacon) = ready();
    make(&beacon, "parent", "", &[]);
    make(&beacon, "child", "parent", &[]);
    beacon.archive("parent").unwrap();
    assert!(!listed(&beacon).contains(&"parent".to_owned()));
    let payload = beacon.render("child").unwrap().to_payload();
    assert_eq!(payload["resolvedFrom"], json!(["child", "parent"]));
    check(&beacon, "child", 2, &["parent"]);
}

#[test]
fn resolved_from_order_is_focal_then_parent_then_mixins() {
    let (_dir, _path, beacon) = ready();
    make(&beacon, "gp", "", &[]);
    make(&beacon, "base", "gp", &[]);
    make(&beacon, "mp", "", &[]);
    make(&beacon, "m1", "mp", &[]);
    make(&beacon, "m2", "", &[]);
    make(&beacon, "focal", "base", &["m1", "m2"]);
    // The parent's own list (recursively) comes before the mixins; a mixin's
    // list is the mixin and then its own sources.
    assert_eq!(
        resolved_from(&beacon, "focal"),
        ["focal", "base", "gp", "m1", "mp", "m2"]
    );
    assert_eq!(
        resolved_from(&beacon, "focal@1"),
        resolved_from(&beacon, "focal")
    );

    // A name reached twice is listed once, at its first position.
    make(&beacon, "shared", "", &[]);
    make(&beacon, "left", "", &["shared"]);
    make(&beacon, "twice", "left", &["shared", "m2"]);
    assert_eq!(
        resolved_from(&beacon, "twice"),
        ["twice", "left", "shared", "m2"]
    );
}

#[test]
fn resolved_from_is_not_truncated_past_64_sources() {
    let (_dir, _path, beacon) = ready();
    let names: Vec<String> = (0..64).map(|index| format!("m{index:02}")).collect();
    for name in &names {
        make(&beacon, name, "", &[]);
    }
    let mixins: Vec<&str> = names.iter().map(String::as_str).collect();
    make(&beacon, "focal", "", &mixins);
    let sources = resolved_from(&beacon, "focal");
    assert_eq!(sources.len(), 65);
    assert_eq!(sources[0], "focal");
    assert_eq!(&sources[1..], names.as_slice());
}

#[test]
fn named_validate_payload_has_top_level_runtime_fields() {
    let (_dir, _path, beacon) = ready();
    beacon
        .schema_create(&CreateSchema {
            name: "s".into(),
            required: strings(&["Task"]),
            optional: vec![],
        })
        .unwrap();
    beacon
        .schema_rule_add(
            "s",
            &Rule {
                section: "Task".into(),
                pattern: "ok".into(),
                message: "must say ok".into(),
            },
        )
        .unwrap();
    for (name, body, valid) in [("good", "all ok", true), ("bad", "nothing", false)] {
        beacon
            .create(&CreatePrompt {
                name: name.into(),
                schema: "s".into(),
                sections: vec![section("Task", body)],
                ..Default::default()
            })
            .unwrap();
        let validation = beacon.validate(name).unwrap();
        assert_eq!(validation.valid, valid, "{name}");
        let payload: Value = validation.to_named_payload();
        assert_eq!(payload["command"], json!("validate"));
        assert_eq!(payload["success"], json!(valid));
        let results = payload["results"].as_array().unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0]["name"], json!(name));
        for key in ["valid", "errors", "warnings"] {
            assert!(
                payload.get(key).is_some(),
                "top-level {key} is missing for {name}"
            );
            assert_eq!(payload[key], results[0][key], "top-level {key} for {name}");
        }
        assert_eq!(payload["valid"], json!(valid));
        assert_eq!(payload["warnings"], json!([]));
        if valid {
            assert_eq!(payload["errors"], json!([]));
        } else {
            assert_eq!(payload["errors"], json!(["must say ok"]));
        }
    }
}
