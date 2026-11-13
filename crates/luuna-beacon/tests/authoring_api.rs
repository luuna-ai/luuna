//! Owner: plan s3-beacon-lib-2. Rust-only tests of the authoring library:
//! its public contract, the orders it fixes where Go's come from map
//! iteration, the YAML emitter and JSON rules read from yaml.v3 and
//! encoding/json, and the A8 refusals (each asserts the refusal's exact
//! text and that every file under the project root is unchanged).
//!
//! No test here needs Go or the reference clone: every project root is a
//! fresh `tempfile::tempdir`, canonicalised, and the inputs are benign.
//! Expected texts were read from the Go tool at the reference commit
//! (1c34973) and from `go-lossy-probe-1` and `go-lossy-probe-2`.

mod support;

use luuna_beacon::{
    AuthoringError, Beacon, BeaconError, ChangeKind, EmitReport, EmitSelection, ImportMarkdown,
    Prompt, Section, YamlValue,
};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use support::authoring::*;
use tempfile::TempDir;

// ---- helpers ------------------------------------------------------------------

/// A fresh initialised project with a Beacon on the fixed clock.
fn project() -> (TempDir, PathBuf, Beacon) {
    let (dir, root) = api_root();
    let beacon = fixed_beacon(&root);
    beacon.init().expect("init");
    (dir, root, beacon)
}

/// The error of a call that must fail (the Ok types need not be Debug).
fn error_of<T>(result: Result<T, BeaconError>, context: &str) -> BeaconError {
    match result {
        Err(error) => error,
        Ok(_) => panic!("{context}: the call must fail"),
    }
}

/// Runs a call that must be refused: checks the exact Display and that
/// every file under `root` is byte for byte and mode for mode as before,
/// with no file added (no `.lock`, no `.tmp*`).
fn assert_refused<T>(
    root: &Path,
    context: &str,
    expected: &str,
    call: impl FnOnce() -> Result<T, BeaconError>,
) -> BeaconError {
    let before = tree_bytes(root);
    let error = error_of(call(), context);
    assert!(
        matches!(error, BeaconError::Authoring(_)),
        "{context}: the refusal is an AuthoringError: {error}"
    );
    assert_eq!(error.to_string(), expected, "{context}: the refusal's text");
    assert_tree_unchanged(&before, root, context);
    error
}

fn section(name: &str, body: &str) -> Section {
    Section {
        name: name.to_owned(),
        body: body.to_owned(),
        required: None,
    }
}

fn prompt(name: &str, sections: &[(&str, &str)]) -> Prompt {
    Prompt {
        id: format!("luuna-beacon-{name}"),
        name: name.to_owned(),
        version: 1,
        sections: Some(
            sections
                .iter()
                .map(|(name, body)| section(name, body))
                .collect(),
        ),
        status: "active".to_owned(),
        created_at: "2024-01-01T00:00:00Z".to_owned(),
        updated_at: "2024-01-01T00:00:00Z".to_owned(),
        ..Default::default()
    }
}

fn frontmatter(value: Value) -> BTreeMap<String, Value> {
    match value {
        Value::Object(map) => map.into_iter().collect(),
        other => panic!("frontmatter must be an object: {other}"),
    }
}

/// Writes the prompt store under `root` as exactly `bytes`.
fn write_store(root: &Path, bytes: &[u8]) {
    write_under(root, PROMPTS, bytes);
}

fn write_config(root: &Path, bytes: &[u8]) {
    write_under(root, CONFIG, bytes);
}

fn go_json(value: &YamlValue) -> String {
    value.to_go_json().expect("to_go_json")
}

fn emitted(report: EmitReport) -> Vec<luuna_beacon::EmittedFile> {
    match report {
        EmitReport::Files(files) => files,
        EmitReport::Check { .. } => panic!("expected files, got a check"),
    }
}

fn stale(report: EmitReport) -> Vec<String> {
    match report {
        EmitReport::Check { stale } => stale,
        EmitReport::Files(_) => panic!("expected a check, got files"),
    }
}

fn create(beacon: &Beacon, input: luuna_beacon::CreatePrompt) {
    beacon.create(&input).expect("create");
}

fn named(name: &str) -> luuna_beacon::CreatePrompt {
    luuna_beacon::CreatePrompt {
        name: name.to_owned(),
        sections: vec![section("Task", name)],
        ..Default::default()
    }
}

// ---- paths --------------------------------------------------------------------

#[test]
fn relative_out_and_import_path_use_the_project_root() {
    // RD6: the library never reads the process's working folder; relative
    // paths are taken from the project root.
    let (_dir, root, beacon) = project();
    let cwd = std::env::current_dir().expect("the test's working folder");
    assert_ne!(cwd, root);
    create(&beacon, named("p"));
    let mut options = emit_one("p");
    options.out = Some("rel-out-of-test/out.md".to_owned());
    let files = emitted(beacon.emit(&options).expect("emit"));
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].path, "rel-out-of-test/out.md", "--out is reported verbatim");
    assert!(root.join("rel-out-of-test/out.md").is_file());
    assert!(!cwd.join("rel-out-of-test").exists(), "nothing under the working folder");
    let mut options = emit_one("p");
    options.out_dir = Some("rel-dir-of-test".to_owned());
    let files = emitted(beacon.emit(&options).expect("emit"));
    assert_eq!(
        files[0].path,
        root.join("rel-dir-of-test/p.md").to_string_lossy()
    );
    assert!(!cwd.join("rel-dir-of-test").exists());
    write_under(&root, "src-of-test/in.md", b"## Imported\nbody\n");
    let imported = beacon
        .import_markdown(&import_input("src-of-test/in.md", "in", &[], false))
        .expect("import a path relative to the root");
    assert_eq!(imported.sections, 1);
    assert_eq!(imported.prompt.name, "in");
    let missing = error_of(
        beacon.import_markdown(&import_input("src-of-test/none.md", "none", &[], false)),
        "a missing source",
    );
    assert!(
        missing
            .to_string()
            .starts_with("open src-of-test/none.md: "),
        "{missing}"
    );
}

#[test]
fn emit_reports_out_verbatim_and_joined_paths_absolute() {
    let (_dir, root, beacon) = project();
    create(&beacon, named("p"));
    let files = emitted(beacon.emit(&emit_one("p")).expect("emit"));
    assert_eq!(files[0].name, "p");
    assert_eq!(files[0].version, 1);
    assert!(!files[0].skipped);
    assert_eq!(files[0].path, root.join("agents/p.md").to_string_lossy());
    assert!(Path::new(&files[0].path).is_absolute());
    let files = emitted(beacon.emit(&emit_one("p")).expect("emit again"));
    assert!(files[0].skipped, "an unchanged file is skipped");
    let mut options = emit_one("p");
    options.force = true;
    assert!(!emitted(beacon.emit(&options).expect("force"))[0].skipped);
    let mut options = emit_one("p");
    options.out_dir = Some("od".to_owned());
    assert_eq!(
        emitted(beacon.emit(&options).expect("out-dir"))[0].path,
        root.join("od/p.md").to_string_lossy()
    );
    let absolute = root.join("abs/x.md").to_string_lossy().into_owned();
    let mut options = emit_one("p");
    options.out = Some(absolute.clone());
    assert_eq!(emitted(beacon.emit(&options).expect("abs"))[0].path, absolute);
    let mut options = emit_one("p");
    options.out = Some("x.md".to_owned());
    options.out_dir = Some("ignored".to_owned());
    assert_eq!(emitted(beacon.emit(&options).expect("rel"))[0].path, "x.md");
    let mut options = emit_one("p");
    options.out = Some(String::new());
    options.out_dir = Some(String::new());
    assert_eq!(
        emitted(beacon.emit(&options).expect("empty options"))[0].path,
        root.join("agents/p.md").to_string_lossy(),
        "Some(\"\") is absent"
    );
    let mut options = emit_one("p");
    options.dry_run = true;
    options.out = Some("dry.md".to_owned());
    let files = emitted(beacon.emit(&options).expect("dry run"));
    assert_eq!(files[0].path, "dry.md");
    assert!(!root.join("dry.md").exists());
}

// ---- fixed orders -------------------------------------------------------------

#[test]
fn map_ordered_outputs_are_deterministic() {
    // RD5 (AS9): where Go's order comes from map iteration (GD12-GD16), the
    // library's is fixed: current-record order, the first version's
    // section order, config-file order with the first match winning.
    let (_dir, root, beacon) = project();
    create(&beacon, named("base"));
    create(&beacon, named("mix"));
    for name in ["c2", "c1", "c3"] {
        create(
            &beacon,
            luuna_beacon::CreatePrompt {
                extends: "base".into(),
                mixins: vec!["mix".to_owned()],
                ..named(name)
            },
        );
    }
    for _ in 0..2 {
        let tree = beacon.tree("base").expect("tree");
        assert_eq!(tree.children, ["c2", "c1", "c3"], "GD12: current order");
        let tree = beacon.tree("mix").expect("tree");
        assert_eq!(tree.mixin_users, ["c2", "c1", "c3"]);
        assert_eq!(tree.children, Vec::<String>::new());
    }
    create(
        &beacon,
        luuna_beacon::CreatePrompt {
            sections: vec![
                section("A", "a"),
                section("Z", "z"),
                section("B", "b"),
                section("M", "m"),
            ],
            ..named("d")
        },
    );
    beacon
        .update(
            "d",
            &luuna_beacon::UpdatePrompt {
                remove_sections: vec!["Z".into(), "B".into(), "M".into()],
                sections: vec![section("New", "n")],
                ..Default::default()
            },
        )
        .expect("update");
    let changes = beacon.diff("d", "1", "2").expect("diff");
    let listed: Vec<(String, &str)> = changes
        .iter()
        .map(|change| (change.section.clone(), change.kind.as_str()))
        .collect();
    assert_eq!(
        listed,
        [
            ("New".to_owned(), "added"),
            ("Z".to_owned(), "removed"),
            ("B".to_owned(), "removed"),
            ("M".to_owned(), "removed"),
        ],
        "GD13: removed sections in the first version's order"
    );
    let names: Vec<String> = emitted(beacon.emit(&emit_all()).expect("emit"))
        .into_iter()
        .map(|file| file.name)
        .collect();
    assert_eq!(names, ["base", "mix", "c2", "c1", "c3", "d"], "GD14");
    for name in ["c1", "base"] {
        fs::write(root.join(format!("agents/{name}.md")), "edited").expect("edit");
    }
    let mut check = emit_all();
    check.check = true;
    assert_eq!(stale(beacon.emit(&check).expect("check")), ["base", "c1"]);

    // GD15: the first matching target in config-file order wins, for tags
    // and for defaults.
    write_config(
        &root,
        b"project: luuna-beacon\nversion: \"1\"\ntargets:\n  second:\n    dir: second-dir\n    tags: [t]\n  first:\n    dir: first-dir\n    tags: [t, u]\n  dz:\n    dir: dz-dir\n    default: true\n  da:\n    dir: da-dir\n    default: true\n",
    );
    let config = beacon.config_targets().expect("config targets");
    let targets = config.targets.expect("targets");
    let order: Vec<&str> = targets.iter().map(|target| target.name.as_str()).collect();
    assert_eq!(order, ["second", "first", "dz", "da"], "config-file order");
    create(
        &beacon,
        luuna_beacon::CreatePrompt {
            tags: vec!["u".into(), "t".into()],
            ..named("tagged")
        },
    );
    create(&beacon, named("untagged"));
    for _ in 0..2 {
        let files = emitted(beacon.emit(&emit_one("tagged")).expect("emit"));
        assert_eq!(files[0].path, root.join("second-dir/tagged.md").to_string_lossy());
        let files = emitted(beacon.emit(&emit_one("untagged")).expect("emit"));
        assert_eq!(files[0].path, root.join("dz-dir/untagged.md").to_string_lossy());
    }

    // GD16: legacy emitDirByTag, tags and directories in config-file order;
    // the second directory that cleans to the same name gets `-2`.
    write_config(
        &root,
        b"emitDir: base-dir\nemitDirByTag:\n  b: x/y\n  a: x/y\n  c: x-y\n  d: base-dir\n  e: \"\"\n",
    );
    for _ in 0..2 {
        let config = beacon.config_targets().expect("legacy targets");
        assert_eq!(config.project, "luuna-beacon");
        assert_eq!(config.version, "1");
        let targets = config.targets.expect("migrated targets");
        let find = |name: &str| {
            targets
                .iter()
                .find(|target| target.name == name)
                .unwrap_or_else(|| panic!("target {name}"))
        };
        let default = find("default");
        assert_eq!(default.dir, "base-dir");
        assert!(default.default);
        assert_eq!(default.tags, ["d"]);
        let xy = find("x-y");
        assert_eq!(xy.dir, "x/y");
        assert_eq!(xy.tags, ["b", "a"]);
        assert!(!xy.default);
        let xy2 = find("x-y-2");
        assert_eq!(xy2.dir, "x-y");
        assert_eq!(xy2.tags, ["c"]);
        assert_eq!(targets.len(), 3);
        let position = |name: &str| targets.iter().position(|target| target.name == name);
        assert!(position("x-y") < position("x-y-2"));
    }
    write_config(&root, b"project: p\nversion: \"2\"\n");
    let config = beacon.config_targets().expect("no targets");
    assert!(config.targets.is_none(), "Go's nil Targets");
    assert_eq!((config.project.as_str(), config.version.as_str()), ("p", "2"));
}

// ---- the YAML emitter -----------------------------------------------------------

#[test]
fn yaml_key_order_follows_yaml_v3_sorter() {
    // sorter.go: numbers before letters, digit runs compared as numbers,
    // upper case before lower case at the first difference.
    let keys = [
        "a10", "a2", "a_b", "A", "b", "1", "10", "9", "item-1", "item_1", "a1b", "a01", "a1",
    ];
    let mut p = prompt("keys", &[("main", "body")]);
    p.frontmatter = keys.iter().map(|key| ((*key).to_owned(), json!(1))).collect();
    assert_eq!(
        luuna_beacon::render_markdown(&p),
        "---\n\"1\": 1\n\"9\": 1\n\"10\": 1\nA: 1\na_b: 1\na1: 1\na1b: 1\na01: 1\na2: 1\na10: 1\nb: 1\nitem-1: 1\nitem_1: 1\nname: keys\n---\nbody\n"
    );
    let value = YamlValue::from_json(&json!({"b": 1, "a": {"z": 1, "y": 2}, "A": [], "10": 1, "9": 1}));
    assert_eq!(
        value.to_yaml(),
        "\"9\": 1\n\"10\": 1\nA: []\na:\n    \"y\": 2\n    z: 1\nb: 1\n"
    );
}

#[test]
fn yaml_emitter_layout_follows_yaml_v3() {
    // Indent 4, indented block sequences, flow {} and [] for empties,
    // literal blocks with chomping and indent indicators, quoting of
    // strings that would read as other types, FormatFloat('g').
    let mut p = prompt("layout", &[("A", "a"), ("B", "b")]);
    p.description = "desc".to_owned();
    p.frontmatter = frontmatter(json!({
        "map": {"z": 1, "a": {"b": [1, 2]}},
        "list": ["x", {"k": "v", "j": []}, [1, [2]]],
        "empty_map": {},
        "empty_list": [],
        "null": null,
        "bool": true,
        "multi": "one\ntwo\n",
        "strip": "one\ntwo",
        "keep": "one\n\n",
        "lead": "  lead\nx",
        "float": 1.5,
        "big": 1e21,
        "small": 1e-5,
        "zero": -0.0,
        "str_num": "12",
        "str_bool": "true",
        "quote": "it's",
        "colon": "a: b",
        "empty": "",
        "long": "aaaaaaaaaa bbbbbbbbbb cccccccccc dddddddddd eeeeeeeeee ffffffffff gggggggggg hhhhhhhhhh",
    }));
    let expected = "---\nbig: 1e+21\nbool: true\ncolon: 'a: b'\ndescription: desc\nempty: \"\"\nempty_list: []\nempty_map: {}\nfloat: 1.5\nkeep: |+\n    one\n\nlead: |4-\n      lead\n    x\nlist:\n    - x\n    - j: []\n      k: v\n    - - 1\n      - - 2\nlong: aaaaaaaaaa bbbbbbbbbb cccccccccc dddddddddd eeeeeeeeee ffffffffff gggggggggg hhhhhhhhhh\nmap:\n    a:\n        b:\n            - 1\n            - 2\n    z: 1\nmulti: |\n    one\n    two\nname: layout\n\"null\": null\nquote: it's\nsmall: 1e-05\nstr_bool: \"true\"\nstr_num: \"12\"\nstrip: |-\n    one\n    two\nzero: -0\n---\n## A\n\na\n\n## B\n\nb\n";
    assert_eq!(luuna_beacon::render_markdown(&p), expected);
    assert_eq!(YamlValue::from_json(&json!(null)).to_yaml(), "null\n");
    assert_eq!(YamlValue::from_json(&json!("x")).to_yaml(), "x\n");
    assert_eq!(YamlValue::from_json(&json!([])).to_yaml(), "[]\n");
    assert_eq!(YamlValue::from_json(&json!({})).to_yaml(), "{}\n");
    assert_eq!(
        YamlValue::from_json(&json!(["a", ["b"]])).to_yaml(),
        "- a\n- - b\n"
    );
}

#[test]
fn yaml_values_convert_to_go_json_like_encoding_json() {
    // Values as yaml.v3 decodes config.yaml into `any`, printed as
    // encoding/json prints them (read from Go's `config show --path`).
    let (_dir, root, beacon) = project();
    write_config(
        &root,
        b"project: luuna-beacon\nversion: \"1\"\nx:\n  a: 1\n  b: 1.5\n  c: 18446744073709551615\n  d: 2001-12-14\n  e: \"<&>\"\n  f: [1, two]\n  g: {1: a}\n  h: 1e20\n  i: 1e21\n  j: -0.0\n  k: ~\n  l: true\n  m: -9223372036854775808\n  n: 0.1\n  o: \"\\u2028\"\n  p: 2001-12-14T21:59:43.1Z\n  nan: .nan\n  inf: .inf\n  ninf: -.inf\n  boolkey: {true: a}\n",
    );
    for (key, expected) in [
        ("a", "1"),
        ("b", "1.5"),
        ("c", "18446744073709551615"),
        ("d", "\"2001-12-14T00:00:00Z\""),
        ("e", "\"\\u003c\\u0026\\u003e\""),
        ("f", "[1,\"two\"]"),
        ("g", "{\"1\":\"a\"}"),
        ("h", "100000000000000000000"),
        ("i", "1e+21"),
        ("j", "-0"),
        ("k", "null"),
        ("l", "true"),
        ("m", "-9223372036854775808"),
        ("n", "0.1"),
        ("o", "\"\\u2028\""),
        ("p", "\"2001-12-14T21:59:43.1Z\""),
    ] {
        let value = beacon.config_get(&format!("x.{key}")).expect("config_get");
        assert_eq!(go_json(&value), expected, "x.{key}");
    }
    assert!(beacon.config_get("x.k").expect("null").is_null());
    assert!(!beacon.config_get("x.a").expect("one").is_null());
    for (key, expected) in [
        ("nan", "json: unsupported value: NaN"),
        ("inf", "json: unsupported value: +Inf"),
        ("ninf", "json: unsupported value: -Inf"),
        ("boolkey", "json: unsupported type: map[interface {}]interface {}"),
    ] {
        let value = beacon.config_get(&format!("x.{key}")).expect("config_get");
        let error = error_of(value.to_go_json(), key);
        assert_eq!(error.to_string(), expected, "x.{key}");
    }
    // from_json: every JSON number becomes a float64, as json.Unmarshal
    // gives.
    let value = YamlValue::from_json(&json!({"n": 9007199254740993_u64, "s": "<", "l": [1, 2.5]}));
    assert_eq!(go_json(&value), "{\"l\":[1,2.5],\"n\":9007199254740992,\"s\":\"\\u003c\"}");
    assert_eq!(YamlValue::from_json(&json!(1)), YamlValue::from_json(&json!(1.0)));
    assert_ne!(YamlValue::from_json(&json!(1)), YamlValue::from_json(&json!("1")));
    let missing = error_of(beacon.config_get("x.none"), "a missing path");
    assert_eq!(missing.to_string(), "path not found: x.none");
}

#[test]
fn config_schema_document_is_fixed() {
    assert_eq!(
        luuna_beacon::config_schema(),
        json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "type": "object",
            "required": ["project", "version"],
            "properties": {
                "project": {"type": "string", "minLength": 1},
                "version": {"type": "string", "minLength": 1},
                "targets": {"type": "object"},
            },
            "additionalProperties": false,
        })
    );
}

#[test]
fn go_panic_is_returned_as_an_error() {
    // RD3, AS7: Go panics (assignment to entry in nil map) on config set
    // when the document is empty or null; the library returns GoPanic and
    // writes nothing.
    for content in [&b""[..], b"null\n", b"# comment only\n"] {
        let (_dir, root, beacon) = project();
        write_config(&root, content);
        let before = tree_bytes(&root);
        let error = error_of(beacon.config_set("project", "x"), "config set on an empty file");
        assert!(matches!(
            &error,
            BeaconError::Authoring(AuthoringError::GoPanic(message))
                if message == "assignment to entry in nil map"
        ));
        assert_eq!(error.to_string(), "panic: assignment to entry in nil map");
        assert_tree_unchanged(&before, &root, "after the panic");
        assert!(!beacon.config_unset("project").expect("unset on an empty file"));
        assert_tree_unchanged(&before, &root, "after unset");
        assert!(beacon.config_show().expect("show").raw.is_null());
    }
}

#[test]
fn render_markdown_body_rules() {
    // renderMarkdown (cli.go:1961-1996): one section is its body alone,
    // several are `## name` blocks joined by a blank line; a final newline;
    // frontmatter only when the name is not empty or there are other keys;
    // the .ts form escapes `\`, backticks and `${`.
    assert_eq!(
        luuna_beacon::render_markdown(&prompt("one", &[("Task", "body")])),
        "---\nname: one\n---\nbody\n"
    );
    assert_eq!(
        luuna_beacon::render_markdown(&prompt("two", &[("A", "a"), ("B", "b\n")])),
        "---\nname: two\n---\n## A\n\na\n\n## B\n\nb\n\n"
    );
    let mut none = prompt("nosec", &[]);
    assert_eq!(luuna_beacon::render_markdown(&none), "---\nname: nosec\n---\n\n");
    none.sections = None;
    assert_eq!(luuna_beacon::render_markdown(&none), "---\nname: nosec\n---\n\n");
    let nameless = prompt("", &[("main", "nameless")]);
    assert_eq!(luuna_beacon::render_markdown(&nameless), "nameless\n");
    let mut overridden = prompt("own", &[("Task", "x")]);
    overridden.description = "own description".to_owned();
    overridden.frontmatter = frontmatter(json!({"name": "other", "description": "fm"}));
    assert_eq!(
        luuna_beacon::render_markdown(&overridden),
        "---\ndescription: fm\nname: other\n---\nx\n",
        "GD25: frontmatter overrides name and description"
    );
    let mut ts = prompt("my-ts", &[("main", "a\\b `c` ${d}")]);
    ts.description = "line".to_owned();
    ts.emit_as = "x.ts".to_owned();
    assert_eq!(
        luuna_beacon::render_markdown(&ts),
        "// Auto-generated by Luuna Beacon \u{2014} do not edit manually\n// Prompt: my-ts\n// line\n\nexport const MY_TS = `a\\\\b \\`c\\` \\${d}\n`;\n"
    );
    ts.description = String::new();
    ts.emit_as = "dir/upper.TS".to_owned();
    assert!(
        luuna_beacon::render_markdown(&ts).starts_with("---\nname: my-ts\n---\n"),
        "only a `.ts` suffix selects the export form"
    );
    ts.emit_as = "x.ts".to_owned();
    assert_eq!(
        luuna_beacon::render_markdown(&ts),
        "// Auto-generated by Luuna Beacon \u{2014} do not edit manually\n// Prompt: my-ts\n\nexport const MY_TS = `a\\\\b \\`c\\` \\${d}\n`;\n"
    );
}

// ---- the public contract --------------------------------------------------------

/// Every AuthoringError variant by name; a variant the contract does not
/// list fails to compile here.
fn variant_name(error: &luuna_beacon::AuthoringError) -> &'static str {
    use luuna_beacon::AuthoringError as E;
    match error {
        E::ExpectedNameAtVersion => "ExpectedNameAtVersion",
        E::VersionNotFound(_) => "VersionNotFound",
        E::PromptVersionsNotFound => "PromptVersionsNotFound",
        E::PathNotFound(_) => "PathNotFound",
        E::ConfigPathEmpty => "ConfigPathEmpty",
        E::NotAnObject(_) => "NotAnObject",
        E::ConfigValidation(_) => "ConfigValidation",
        E::InvalidFrontmatter(_) => "InvalidFrontmatter",
        E::StaleFiles(_) => "StaleFiles",
        E::JsonUnsupportedValue(_) => "JsonUnsupportedValue",
        E::JsonUnsupportedType(_) => "JsonUnsupportedType",
        E::GoPanic(_) => "GoPanic",
        E::ConfigLaterDocuments(_) => "ConfigLaterDocuments",
        E::ConfigNullKey => "ConfigNullKey",
        E::ConfigReadBack(_) => "ConfigReadBack",
        E::ImportUnrecognisedFrontmatter => "ImportUnrecognisedFrontmatter",
        E::ImportTextBeforeFirstHeading => "ImportTextBeforeFirstHeading",
        E::ImportDescriptionNotString => "ImportDescriptionNotString",
        E::ImportNullFrontmatterKey => "ImportNullFrontmatterKey",
        E::ImportLaterFrontmatterDocuments => "ImportLaterFrontmatterDocuments",
        E::ImportFrontmatterNameDiffers(_) => "ImportFrontmatterNameDiffers",
        E::ImportReadBack(_) => "ImportReadBack",
        E::EmitIntoStore(_) => "EmitIntoStore",
        E::PinRecordKeyUnknown(_, _) => "PinRecordKeyUnknown",
        E::PinRecordKeyRepeated(_, _) => "PinRecordKeyRepeated",
    }
}

#[test]
fn authoring_errors_display_go_messages() {
    use luuna_beacon::AuthoringError as E;
    let cases: Vec<(E, &str)> = vec![
        (E::ExpectedNameAtVersion, "expected name@version"),
        (E::VersionNotFound(3), "version 3 not found"),
        (E::PromptVersionsNotFound, "prompt versions not found"),
        (E::PathNotFound("a.b".into()), "path not found: a.b"),
        (E::ConfigPathEmpty, "config path must be non-empty"),
        (E::NotAnObject("project".into()), "cannot set: \"project\" is not an object"),
        (E::NotAnObject("a\"b\n".into()), "cannot set: \"a\\\"b\\n\" is not an object"),
        (
            E::ConfigValidation("project must be a non-empty string".into()),
            "config validation failed: project must be a non-empty string",
        ),
        (
            E::InvalidFrontmatter("yaml: line 1: did not find expected node content".into()),
            "invalid frontmatter: yaml: line 1: did not find expected node content",
        ),
        (E::StaleFiles(2), "2 stale file(s)"),
        (E::JsonUnsupportedValue("NaN".into()), "json: unsupported value: NaN"),
        (
            E::JsonUnsupportedType("map[interface {}]interface {}".into()),
            "json: unsupported type: map[interface {}]interface {}",
        ),
        (
            E::GoPanic("assignment to entry in nil map".into()),
            "panic: assignment to entry in nil map",
        ),
        (
            E::ConfigLaterDocuments("config.yaml"),
            "refused: config.yaml holds more than one YAML document and the rewrite would keep only the first; config.yaml was not changed",
        ),
        (
            E::ConfigLaterDocuments("the value"),
            "refused: the value holds more than one YAML document and the rewrite would keep only the first; config.yaml was not changed",
        ),
        (
            E::ConfigNullKey,
            "refused: config.yaml has a null top-level key that the rewrite would drop; config.yaml was not changed",
        ),
        (
            E::ConfigReadBack("targets.other.extra".into()),
            "refused: after the rewrite the value at targets.other.extra would read back as a different value; config.yaml was not changed",
        ),
        (
            E::ImportUnrecognisedFrontmatter,
            "refused: the frontmatter block is not in the form the Go tool reads (a byte order mark, CRLF line ends or an empty block) and would not be stored as frontmatter; nothing was stored",
        ),
        (
            E::ImportTextBeforeFirstHeading,
            "refused: text before the first \"## \" heading would be dropped; nothing was stored",
        ),
        (
            E::ImportDescriptionNotString,
            "refused: the frontmatter description is not a string and would be dropped; nothing was stored",
        ),
        (
            E::ImportNullFrontmatterKey,
            "refused: a frontmatter key is null and would be dropped; nothing was stored",
        ),
        (
            E::ImportLaterFrontmatterDocuments,
            "refused: the frontmatter holds more than one YAML document and only the first would be stored; nothing was stored",
        ),
        (
            E::ImportFrontmatterNameDiffers("other".into()),
            "refused: the frontmatter name \"other\" differs from the name given and would be dropped; nothing was stored",
        ),
        (
            E::ImportReadBack("a.n".into()),
            "refused: the frontmatter value at a.n would read back from the store as a different value; nothing was stored",
        ),
        (
            E::EmitIntoStore("/project/.luuna/beacon/prompts.jsonl".into()),
            "refused: emit would overwrite /project/.luuna/beacon/prompts.jsonl inside .luuna/ with a rendered prompt; nothing was written",
        ),
        (
            E::PinRecordKeyUnknown("p1".into(), "sections.0.extra".into()),
            "refused: the current record of \"p1\" holds the key \"sections.0.extra\" that the Go tool does not read, and the new version would drop it; prompts.jsonl was not changed",
        ),
        (
            E::PinRecordKeyRepeated("p1".into(), "frontmatter.k".into()),
            "refused: the current record of \"p1\" holds the key \"frontmatter.k\" more than once, and the new version would keep only the last value; prompts.jsonl was not changed",
        ),
    ];
    let mut seen = std::collections::BTreeSet::new();
    for (error, expected) in cases {
        seen.insert(variant_name(&error));
        assert_eq!(error.to_string(), expected, "{}", variant_name(&error));
        let wrapped = BeaconError::from(error);
        assert!(matches!(wrapped, BeaconError::Authoring(_)));
        assert_eq!(wrapped.to_string(), expected, "through BeaconError");
    }
    assert_eq!(seen.len(), 25, "every one of the 25 variants");
}

#[test]
fn public_api_matches_the_contract() {
    use luuna_beacon::{
        Beacon, BeaconError, ChangeKind, ConfigShow, EmitOptions, EmitReport, EmitSelection,
        EmitTarget, EmittedFile, ImportMarkdown, Imported, Prompt, ProjectConfig, PromptStats,
        PromptTree, SectionChange, YamlValue,
    };
    use std::collections::BTreeMap;

    // Functions and methods, by signature.
    let _: fn(&Prompt) -> String = luuna_beacon::render_markdown;
    let _: fn() -> serde_json::Value = luuna_beacon::config_schema;
    let _: fn(&Beacon, &str) -> Result<String, BeaconError> = Beacon::render_text;
    let _: fn(&Beacon, &EmitOptions) -> Result<EmitReport, BeaconError> = Beacon::emit;
    let _: fn(&Beacon, &ImportMarkdown) -> Result<Imported, BeaconError> = Beacon::import_markdown;
    let _: fn(&Beacon, &str) -> Result<Prompt, BeaconError> = Beacon::pin;
    let _: fn(&Beacon, &str) -> Result<Prompt, BeaconError> = Beacon::unpin;
    let _: fn(&Beacon, &str, &str, &str) -> Result<Vec<SectionChange>, BeaconError> = Beacon::diff;
    let _: fn(&Beacon, &str) -> Result<PromptTree, BeaconError> = Beacon::tree;
    let _: fn(&Beacon) -> Result<PromptStats, BeaconError> = Beacon::stats;
    let _: fn(&Beacon) -> Result<ProjectConfig, BeaconError> = Beacon::config_targets;
    let _: fn(&Beacon) -> Result<ConfigShow, BeaconError> = Beacon::config_show;
    let _: fn(&Beacon, &str) -> Result<YamlValue, BeaconError> = Beacon::config_get;
    let _: fn(&Beacon, &str, &str) -> Result<YamlValue, BeaconError> = Beacon::config_set;
    let _: fn(&Beacon, &str) -> Result<bool, BeaconError> = Beacon::config_unset;
    let _: fn(&YamlValue) -> String = YamlValue::to_yaml;
    let _: fn(&YamlValue) -> Result<String, BeaconError> = YamlValue::to_go_json;
    let _: fn(&serde_json::Value) -> YamlValue = YamlValue::from_json;
    let _: fn(&YamlValue) -> bool = YamlValue::is_null;
    let _: fn(&ChangeKind) -> &'static str = ChangeKind::as_str;
    let _: fn(&EmitReport) -> Option<BeaconError> = EmitReport::failure;

    // Traits.
    fn value_traits<T: std::fmt::Debug + Clone + PartialEq>() {}
    value_traits::<YamlValue>();
    fn has_default<T: Default>() {}
    has_default::<ImportMarkdown>();
    fn into_beacon_error<T: Into<BeaconError>>() {}
    into_beacon_error::<luuna_beacon::AuthoringError>();
    fn error_traits<T: std::error::Error + std::fmt::Display + std::fmt::Debug>() {}
    error_traits::<luuna_beacon::AuthoringError>();

    // Types, by their public fields and variants.
    let raw = YamlValue::from_json(&serde_json::json!({"project": "p"}));
    let shown = ConfigShow {
        raw: raw.clone(),
        text: b"project: p\n".to_vec(),
    };
    assert_eq!(shown.raw, raw);
    assert_eq!(shown.text, b"project: p\n");
    let target = EmitTarget {
        name: "t".into(),
        dir: "d".into(),
        default: true,
        tags: vec!["x".into()],
    };
    let config = ProjectConfig {
        project: "p".into(),
        version: "1".into(),
        targets: Some(vec![target]),
    };
    assert_eq!(config.targets.as_ref().map(Vec::len), Some(1));
    let options = EmitOptions {
        selection: EmitSelection::Prompt("p".into()),
        check: false,
        dry_run: false,
        force: false,
        out: None,
        out_dir: Some(String::new()),
    };
    assert!(matches!(options.selection, EmitSelection::Prompt(ref name) if name == "p"));
    let _ = EmitSelection::All;
    let file = EmittedFile {
        name: "p".into(),
        path: "/x/p.md".into(),
        version: 2,
        skipped: false,
    };
    assert!(EmitReport::Files(vec![file]).failure().is_none());
    assert!(EmitReport::Check { stale: Vec::new() }.failure().is_none());
    let failure = EmitReport::Check {
        stale: vec!["a".into(), "b".into()],
    }
    .failure()
    .expect("a failing check");
    assert!(matches!(
        failure,
        BeaconError::Authoring(luuna_beacon::AuthoringError::StaleFiles(2))
    ));
    assert_eq!(failure.to_string(), "2 stale file(s)");
    let input = ImportMarkdown::default();
    assert!(input.path.is_empty() && input.name.is_empty() && input.tags.is_empty());
    assert!(!input.no_split);
    let imported = Imported {
        prompt: Prompt::default(),
        sections: 0,
    };
    assert_eq!(imported.sections, 0);
    for (kind, text) in [
        (ChangeKind::Added, "added"),
        (ChangeKind::Modified, "modified"),
        (ChangeKind::Removed, "removed"),
    ] {
        let change = SectionChange {
            section: "s".into(),
            kind,
        };
        assert_eq!(change.kind.as_str(), text);
    }
    let tree = PromptTree {
        name: "p".into(),
        ancestors: Vec::new(),
        mixins: None,
        children: Vec::new(),
        mixin_users: Vec::new(),
    };
    assert!(tree.mixins.is_none());
    let stats = PromptStats {
        total: 0,
        by_status: BTreeMap::new(),
        by_tag: BTreeMap::new(),
    };
    assert_eq!(stats.total, 0);
    assert!(YamlValue::from_json(&serde_json::Value::Null).is_null());
    assert_eq!(
        YamlValue::from_json(&serde_json::json!({"b": 1, "a": [true]})).to_yaml(),
        "a:\n    - true\nb: 1\n"
    );
}

// ---- import -------------------------------------------------------------------

#[test]
fn import_returns_the_prompt_go_would_read_back() {
    // The stored line matches Go's byte for byte (key order, omitempty,
    // the time value as RFC3339Nano), and Imported.prompt is that line read
    // back. Go's line for the same file, at the reference commit:
    // {"id":"luuna-beacon-<hex>","name":"imp","description":"Desc","version":1,
    //  "sections":[{"name":"A","body":"a"},{"name":"B","body":"b\n"}],"tags":["t"],
    //  "frontmatter":{"k":"v","list":[1,"two"],"n":1.5,"when":"2001-12-14T00:00:00Z"},
    //  "status":"active","createdAt":"<now>","updatedAt":"<now>"}
    let (_dir, root, beacon) = project();
    write_under(
        &root,
        "src/imp.md",
        b"---\ndescription: Desc\nname: imp\nk: v\nn: 1.5\nwhen: 2001-12-14\nlist: [1, two]\n---\n## A\na\n## B\nb\n",
    );
    let imported = beacon
        .import_markdown(&ImportMarkdown {
            path: "src/imp.md".into(),
            name: "imp".into(),
            tags: vec!["t".into()],
            no_split: false,
        })
        .expect("import");
    assert_eq!(imported.sections, 2);
    let line = String::from_utf8(last_store_line(&root)).expect("UTF-8 line");
    let prefix = "{\"id\":\"luuna-beacon-";
    assert!(line.starts_with(prefix), "{line}");
    let rest = &line[prefix.len()..];
    let hex_end = rest.find('"').expect("the id ends");
    assert!(rest[..hex_end].chars().all(|c| c.is_ascii_hexdigit()));
    assert_eq!(
        &rest[hex_end..],
        format!("\",\"name\":\"imp\",\"description\":\"Desc\",\"version\":1,\"sections\":[{{\"name\":\"A\",\"body\":\"a\"}},{{\"name\":\"B\",\"body\":\"b\\n\"}}],\"tags\":[\"t\"],\"frontmatter\":{{\"k\":\"v\",\"list\":[1,\"two\"],\"n\":1.5,\"when\":\"2001-12-14T00:00:00Z\"}},\"status\":\"active\",\"createdAt\":\"{FIXED_TIME_TEXT}\",\"updatedAt\":\"{FIXED_TIME_TEXT}\"}}")
    );
    let shown = beacon.show("imp").expect("show");
    assert_eq!(imported.prompt, shown, "Imported.prompt is the stored line read back");
    assert_eq!(imported.prompt.description, "Desc");
    assert!(!imported.prompt.frontmatter.contains_key("name"));
    assert!(!imported.prompt.frontmatter.contains_key("description"));
    assert_eq!(imported.prompt.tags, ["t"]);
    // --no-split and a heading-less body: one `body` section, trimmed.
    write_under(&root, "src/plain.md", b"\n  plain text\n\n## not split\n\n");
    let imported = beacon
        .import_markdown(&import_input("src/plain.md", "plain", &[], true))
        .expect("import --no-split");
    assert_eq!(imported.sections, 1);
    let sections = imported.prompt.sections.expect("sections");
    assert_eq!(sections[0].name, "body");
    assert_eq!(sections[0].body, "plain text\n\n## not split");
    let error = error_of(
        beacon.import_markdown(&import_input("src/plain.md", "plain", &[], true)),
        "a second import under the same name",
    );
    assert_eq!(error.to_string(), "prompt name \"plain\" already exists");
}

#[test]
fn decoder_refuses_control_characters_and_skips_a_bom() {
    // yaml.v3's reader: control characters refused with Go's text, one
    // leading U+FEFF skipped (so no BOM ends up in the first key).
    let (_dir, root, beacon) = project();
    write_config(&root, "\u{feff}project: bom\nversion: \"1\"\n".as_bytes());
    assert_eq!(go_json(&beacon.config_get("project").expect("project")), "\"bom\"");
    assert_eq!(beacon.config_targets().expect("targets").project, "bom");
    write_config(&root, b"project: luuna-beacon\nversion: \"1\"\nx: a\x00b\n");
    let before = tree_bytes(&root);
    for result in [
        beacon.config_show().map(|_| ()),
        beacon.config_get("x").map(|_| ()),
        beacon.config_set("version", "\"2\"").map(|_| ()),
    ] {
        let error = error_of(result, "NUL in config.yaml");
        assert_eq!(error.to_string(), "yaml: control characters are not allowed");
    }
    assert_tree_unchanged(&before, &root, "NUL in config.yaml");
    write_config(&root, b"project: luuna-beacon\nversion: \"1\"\n");
    let before = tree_bytes(&root);
    for value in ["a\u{0}b", "a\u{1}b", "a\u{7f}b"] {
        let error = error_of(beacon.config_set("version", value), "a control character in the value");
        assert_eq!(error.to_string(), "yaml: control characters are not allowed");
    }
    assert_tree_unchanged(&before, &root, "control characters in the value");
    for (index, byte) in [0_u8, 1, 0x7f].into_iter().enumerate() {
        let mut source = b"---\nk: a".to_vec();
        source.push(byte);
        source.extend_from_slice(b"b\n---\n## S\nb\n");
        let path = format!("src/control-{index}.md");
        write_under(&root, &path, &source);
        let before = tree_bytes(&root);
        let error = error_of(
            beacon.import_markdown(&import_input(&path, &format!("c{index}"), &[], false)),
            "a control character in frontmatter",
        );
        assert_eq!(
            error.to_string(),
            "invalid frontmatter: yaml: control characters are not allowed"
        );
        assert_tree_unchanged(&before, &root, "control character in frontmatter");
    }
    write_under(&root, "src/bom.md", "---\n\u{feff}k: v\n---\n## S\nb\n".as_bytes());
    let imported = beacon
        .import_markdown(&import_input("src/bom.md", "bom", &[], false))
        .expect("a BOM at the start of the frontmatter text");
    assert_eq!(imported.prompt.frontmatter.get("k"), Some(&json!("v")));
    assert_eq!(imported.prompt.frontmatter.len(), 1);
}

// ---- the normalisation hook -----------------------------------------------------

/// Every leaf of a document (strings, numbers, booleans, nulls) as text,
/// sorted: equal before and after the hook means it only reordered.
fn leaves(doc: &Value) -> Vec<String> {
    let mut found = Vec::new();
    let mut pending = vec![doc];
    while let Some(value) = pending.pop() {
        match value {
            Value::Array(items) => pending.extend(items.iter()),
            Value::Object(object) => {
                for (key, item) in object {
                    found.push(format!("key:{key}"));
                    pending.push(item);
                }
            }
            leaf => found.push(leaf.to_string()),
        }
    }
    found.sort();
    found
}

#[test]
fn sort_map_ordered_only_reorders_map_ordered_arrays() {
    let samples: Vec<(&str, Value, Value)> = vec![
        (
            "tree",
            json!({"success": true, "command": "tree", "name": "p", "ancestors": ["z", "a"], "mixins": ["m2", "m1"],
                   "tree": {"name": "p", "children": ["c", "a", "b"], "mixinUsers": ["z", "y", "y"]}}),
            json!({"success": true, "command": "tree", "name": "p", "ancestors": ["z", "a"], "mixins": ["m2", "m1"],
                   "tree": {"name": "p", "children": ["a", "b", "c"], "mixinUsers": ["y", "y", "z"]}}),
        ),
        (
            "diff",
            json!({"success": true, "command": "diff", "name": "p", "from": "1", "to": "2", "changes": [
                {"section": "q", "type": "removed"}, {"section": "b", "type": "added"},
                {"section": "x", "type": "modified"}, {"section": "z", "type": "removed"},
                {"section": "a", "type": "removed"}, {"section": "m", "type": "removed"}]}),
            json!({"success": true, "command": "diff", "name": "p", "from": "1", "to": "2", "changes": [
                {"section": "q", "type": "removed"}, {"section": "b", "type": "added"},
                {"section": "x", "type": "modified"}, {"section": "a", "type": "removed"},
                {"section": "m", "type": "removed"}, {"section": "z", "type": "removed"}]}),
        ),
        (
            "emit",
            json!({"success": true, "command": "emit", "count": 3, "files": [
                {"name": "b", "path": "<root>/2", "version": 1},
                {"name": "a", "path": "<root>/9", "version": 2, "skipped": true},
                {"name": "a", "path": "<root>/1", "version": 3}]}),
            json!({"success": true, "command": "emit", "count": 3, "files": [
                {"name": "a", "path": "<root>/1", "version": 3},
                {"name": "a", "path": "<root>/9", "version": 2, "skipped": true},
                {"name": "b", "path": "<root>/2", "version": 1}]}),
        ),
        (
            "emit",
            json!({"success": false, "command": "emit", "check": true, "stale": ["c", "a", "b"], "upToDate": false}),
            json!({"success": false, "command": "emit", "check": true, "stale": ["a", "b", "c"], "upToDate": false}),
        ),
    ];
    for (command, input, expected) in samples {
        let mut doc = input.clone();
        sort_map_ordered(command, &mut doc, "luuna-beacon");
        assert_eq!(doc, expected, "{command}");
        assert_eq!(leaves(&doc), leaves(&input), "{command}: only reordered");
        let mut again = doc.clone();
        sort_map_ordered(command, &mut again, "luuna-beacon");
        assert_eq!(again, doc, "{command}: sorting twice changes nothing");
    }
    // Every other document is left exactly as it is.
    let unchanged: Vec<(&str, Value)> = vec![
        ("emit", json!({"success": false, "command": "emit", "error": "2 stale file(s)"})),
        ("tree", json!({"success": false, "command": "tree", "error": "prompt \"x\" not found"})),
        ("diff", json!({"success": true, "command": "diff", "changes": [{"section": "b", "type": "removed"}, {"section": "a", "type": "added"}]})),
        ("stats", json!({"success": true, "command": "stats", "prompts": {"total": 2, "byStatus": {"active": 2}, "byTag": {"b": 1, "a": 1}}})),
        ("config show", json!({"success": true, "command": "config show", "config": {"targets": {"b": {"dir": "b"}, "a": {"dir": "a"}}, "list": ["b", "a"]}})),
        ("config show", json!({"success": true, "command": "config show", "path": "x", "value": ["b", "a"]})),
        ("import", json!({"success": true, "command": "import", "id": "luuna-beacon-1", "name": "p", "sections": 2})),
        ("pin", json!({"success": true, "command": "pin", "name": "p", "pinned": 1})),
        ("unpin", json!({"success": true, "command": "unpin", "name": "p", "pinned": null})),
        ("list", json!({"success": true, "command": "list", "prompts": [{"name": "b"}, {"name": "a"}], "count": 2})),
        ("config", json!({"success": false, "command": "config", "error": "path not found: x"})),
        ("something-else", json!({"children": ["b", "a"], "stale": ["b", "a"], "files": [{"name": "b"}, {"name": "a"}]})),
    ];
    for (command, input) in unchanged {
        let mut doc = input.clone();
        sort_map_ordered(command, &mut doc, "luuna-beacon");
        assert_eq!(doc, input, "{command}: left unchanged");
    }
}

// ---- A8: config ---------------------------------------------------------------

const LATER_DOCUMENTS_FILE: &str =
    "refused: config.yaml holds more than one YAML document and the rewrite would keep only the first; config.yaml was not changed";
const LATER_DOCUMENTS_VALUE: &str =
    "refused: the value holds more than one YAML document and the rewrite would keep only the first; config.yaml was not changed";
const NULL_KEY: &str =
    "refused: config.yaml has a null top-level key that the rewrite would drop; config.yaml was not changed";
const READ_BACK: &str =
    "refused: after the rewrite the value at targets.other.extra would read back as a different value; config.yaml was not changed";

#[test]
fn a8_config_set_refuses_later_documents_and_leaves_the_file_unchanged() {
    // RD12.
    for later in [
        "---\nkept: second document\n",
        "...\n---\nkept: after an end marker\n",
        "--- !!map\nkept: tagged start\n",
    ] {
        let (_dir, root, beacon) = project();
        let file = format!("{PROBE_BASE_CONFIG}{later}");
        write_config(&root, file.as_bytes());
        assert_refused(&root, "set", LATER_DOCUMENTS_FILE, || {
            beacon.config_set("targets.other.dir", "o2")
        });
        assert_refused(&root, "unset", LATER_DOCUMENTS_FILE, || {
            beacon.config_unset("targets.default.default")
        });
        // Go's earlier errors keep their order: a failing validation and an
        // unset that removes nothing are reported as Go reports them.
        let error = error_of(beacon.config_set("unknown", "1"), "validation first");
        assert_eq!(
            error.to_string(),
            "config validation failed: unknown property \"unknown\""
        );
        assert!(!beacon.config_unset("targets.none").expect("unset of a missing key"));
        // Reading ignores the later document (GD46, copied).
        assert_eq!(go_json(&beacon.config_get("targets.other.dir").expect("get")), "\"o\"");
    }
    let (_dir, root, beacon) = project();
    write_config(&root, PROBE_BASE_CONFIG.as_bytes());
    for value in ["o2\n---\nlost", "o2\n--- x\n", "{a: 1}\n---\n[b]\n"] {
        assert_refused(&root, value, LATER_DOCUMENTS_VALUE, || {
            beacon.config_set("targets.other.dir", value)
        });
    }
    // A single document with markers is one document.
    let (_dir, root, beacon) = project();
    write_config(&root, format!("---\n{PROBE_BASE_CONFIG}...\n").as_bytes());
    beacon
        .config_set("targets.other.dir", "---\no2\n...\n")
        .expect("one document with markers");
    assert_eq!(go_json(&beacon.config_get("targets.other.dir").expect("get")), "\"o2\"");
}

#[test]
fn a8_config_set_refuses_a_null_top_level_key_and_leaves_the_file_unchanged() {
    // RD13. A nested null key is kept by yaml.v3 and is not refused.
    for null_key in ["~: lost\n", "null: lost\n", "Null: lost\n", "? \n: lost\n", "? ~\n: lost\n"] {
        let (_dir, root, beacon) = project();
        write_config(&root, format!("{PROBE_BASE_CONFIG}{null_key}").as_bytes());
        assert_refused(&root, null_key, NULL_KEY, || {
            beacon.config_set("targets.other.dir", "o2")
        });
        assert_refused(&root, null_key, NULL_KEY, || {
            beacon.config_unset("targets.default.default")
        });
    }
    let (_dir, root, beacon) = project();
    write_config(&root, format!("{PROBE_BASE_CONFIG}\"null\": a quoted key is a string\n").as_bytes());
    let error = error_of(beacon.config_set("targets.other.dir", "o2"), "a string key");
    assert_eq!(
        error.to_string(),
        "config validation failed: unknown property \"null\"",
        "a quoted \"null\" is a string key, not a null one"
    );
}

#[test]
fn a8_config_write_refuses_values_that_read_back_differently() {
    // RD14, probe row v10d `-0.0` of go-lossy-probe-1 on its file: Go writes
    // `extra: -0`, which reads back as the integer 0 (A1 :48671, A2 :48722,
    // A3 :48773).
    let variant = family_a("v10d");
    let text = std::str::from_utf8(variant).expect("UTF-8");
    let (_dir, root, beacon) = project();
    write_config(&root, &probe_config_with(variant));
    assert_refused(&root, "A1", READ_BACK, || {
        beacon.config_set("targets.other.dir", "o2")
    });
    assert_refused(&root, "A2", READ_BACK, || {
        beacon.config_unset("targets.default.default")
    });
    let error = error_of(beacon.config_set("targets.other.dir", "o2"), "variant");
    assert!(matches!(
        error,
        BeaconError::Authoring(AuthoringError::ConfigReadBack(ref path)) if path == "targets.other.extra"
    ));
    let (_dir, root, beacon) = project();
    write_config(&root, PROBE_BASE_CONFIG.as_bytes());
    assert_refused(&root, "A3", READ_BACK, || {
        beacon.config_set("targets.other.extra", text)
    });
    // The rows Go reads the same are copied (v10q `-0`, v10g `1.0`).
    for row in ["v10q", "v10g", "v07a", "v08b", "v10r"] {
        let (_dir, root, beacon) = project();
        write_config(&root, &probe_config_with(family_a(row)));
        beacon
            .config_set("targets.other.dir", "o2")
            .unwrap_or_else(|error| panic!("row {row} is copied: {error}"));
    }
}

#[test]
fn a8_config_set_drops_only_comments_and_form() {
    // GD17 (copied part): the rewrite drops comments and changes the form,
    // and every value reads back the same (AS19 through Go's JSON form).
    let (_dir, root, beacon) = project();
    let file = "# a comment\nproject: luuna-beacon # trailing\nversion: \"1\"\ntargets:\n  # inside\n  docs: {dir: d, tags: [a, b]}\n  base: &b\n    dir: shared\n  copy: *b\n  merged:\n    <<: *b\n    default: true\n  num:\n    dir: n\n    hex: 0x1F\n    one: 1.0\n    big: 123456789012345678901234\n    when: 2001-12-14\n    bin: !!binary aGVsbG8=\n    tilde: ~\n";
    write_config(&root, file.as_bytes());
    let paths = [
        "project",
        "version",
        "targets.docs.tags",
        "targets.base",
        "targets.copy",
        "targets.merged",
        "targets.num",
    ];
    let before: Vec<String> = paths
        .iter()
        .map(|path| go_json(&beacon.config_get(path).expect("before")))
        .collect();
    let replacement = beacon.config_set("targets.docs.dir", "d2").expect("config set");
    assert_eq!(go_json(&replacement), "\"d2\"");
    let after: Vec<String> = paths
        .iter()
        .map(|path| go_json(&beacon.config_get(path).expect("after")))
        .collect();
    assert_eq!(before, after, "every untouched value reads the same");
    assert_eq!(go_json(&beacon.config_get("targets.docs.dir").expect("dir")), "\"d2\"");
    let text = fs::read_to_string(root.join(CONFIG)).expect("config.yaml");
    assert!(!text.contains('#'), "comments are dropped: {text}");
    assert!(!text.contains('&') && !text.contains('*'), "anchors are not kept: {text}");
    assert!(text.contains("hex: 31\n"), "number form changes: {text}");
    assert!(!root.join(".luuna/beacon/config.yaml.lock").exists());
    assert!(beacon.config_unset("targets.copy").expect("unset"));
    assert_eq!(go_json(&beacon.config_get("targets.base").expect("base")), before[3]);
}

// ---- A8: import -----------------------------------------------------------------

const UNRECOGNISED: &str = "refused: the frontmatter block is not in the form the Go tool reads (a byte order mark, CRLF line ends or an empty block) and would not be stored as frontmatter; nothing was stored";

/// Writes `source` at `src/<name>.md` and asserts that importing it as
/// `name` is refused with `expected`, every file unchanged.
fn assert_import_refused(root: &Path, beacon: &Beacon, name: &str, source: &[u8], expected: &str) {
    let path = format!("src/{name}.md");
    write_under(root, &path, source);
    assert_refused(root, name, expected, || {
        beacon.import_markdown(&import_input(&path, name, &[], false))
    });
}

/// Writes `source` and asserts that importing it as `name` succeeds.
fn assert_imports(root: &Path, beacon: &Beacon, name: &str, source: &[u8], no_split: bool) -> Prompt {
    let path = format!("src/{name}.md");
    write_under(root, &path, source);
    beacon
        .import_markdown(&import_input(&path, name, &[], no_split))
        .unwrap_or_else(|error| panic!("{name} is imported: {error}"))
        .prompt
}

#[test]
fn a8_import_refuses_an_unrecognised_frontmatter_block() {
    // RD15.
    let (_dir, root, beacon) = project();
    for (name, source) in [
        ("bom", "\u{feff}---\nk: v\n---\n## S\nb\n".as_bytes()),
        ("crlf", b"---\r\nk: v\r\n---\r\n## S\r\nb\r\n"),
        ("crlf-open", b"---\r\nk: v\n---\n## S\nb\n"),
        ("crlf-close", b"---\nk: v\n---\r\n## S\nb\n"),
        ("crlf-close-at-end", b"---\nk: v\n---\r\n"),
        ("close-at-end", b"\xef\xbb\xbf---\nk: v\n---"),
        ("empty", b"---\n---\n## S\nb\n"),
        ("empty-crlf", b"---\r\n---\r\n## S\nb\n"),
        ("skipped-crlf-line", b"---\nk: v\n---\r\nj: w\n---\n## S\nb\n"),
        ("bom-no-heading", "\u{feff}---\nk: v\n---\nbody\n".as_bytes()),
    ] {
        assert_import_refused(&root, &beacon, name, source, UNRECOGNISED);
    }
    // A first-line thematic break with no closing line anywhere is imported
    // as Go imports it (GD21, copied).
    let prompt = assert_imports(&root, &beacon, "rule", b"---\nplain text after a rule\n", false);
    assert!(prompt.frontmatter.is_empty());
    let prompt = assert_imports(&root, &beacon, "fm", b"---\nk: v\n---\n## S\nb\n", false);
    assert_eq!(prompt.frontmatter.get("k"), Some(&json!("v")));
}

#[test]
fn a8_import_refuses_text_before_the_first_heading() {
    // RD16, with whitespace-only text copied (AS21) and the --no-split and
    // heading-less forms, which drop nothing, copied.
    const DROPPED: &str =
        "refused: text before the first \"## \" heading would be dropped; nothing was stored";
    let (_dir, root, beacon) = project();
    for (name, source) in [
        ("intro", &b"intro\n## S\nb\n"[..]),
        ("after-fm", b"---\nk: v\n---\nlead text\n\n## S\nb\n"),
        ("deeper-heading", b"# Title\n## S\nb\n"),
        ("tab-text", b"\t.\n## S\nb\n"),
        ("nbsp", "\u{a0}\n## S\nb\n".as_bytes()),
    ] {
        assert_import_refused(&root, &beacon, name, source, DROPPED);
    }
    assert_imports(&root, &beacon, "blank", b"\n \t\r\n\n## S\nb\n", false);
    assert_imports(&root, &beacon, "fm-blank", b"---\nk: v\n---\n\n\n## S\nb\n", false);
    assert_imports(&root, &beacon, "no-split", b"intro\n## S\nb\n", true);
    assert_imports(&root, &beacon, "no-heading", b"intro\n# Title\nb\n", false);
}

#[test]
fn a8_import_refuses_a_non_string_description() {
    // RD17.
    const NOT_STRING: &str =
        "refused: the frontmatter description is not a string and would be dropped; nothing was stored";
    let (_dir, root, beacon) = project();
    for (index, value) in ["5", "true", "~", "[a]", "{a: b}", "1.5", "2001-12-14"].iter().enumerate() {
        let source = format!("---\ndescription: {value}\n---\n## S\nb\n");
        assert_import_refused(&root, &beacon, &format!("d{index}"), source.as_bytes(), NOT_STRING);
    }
    let prompt = assert_imports(&root, &beacon, "string", b"---\ndescription: \"5\"\n---\n## S\nb\n", false);
    assert_eq!(prompt.description, "5");
    assert!(prompt.frontmatter.is_empty());
}

#[test]
fn a8_import_refuses_null_frontmatter_keys() {
    // RD18.
    const NULL: &str = "refused: a frontmatter key is null and would be dropped; nothing was stored";
    let (_dir, root, beacon) = project();
    for (index, key) in ["~: x\n", "null: x\n", "NULL: x\n", "? \n: x\n", "? ~\n: x\n"].iter().enumerate() {
        let source = format!("---\nk: v\n{key}---\n## S\nb\n");
        assert_import_refused(&root, &beacon, &format!("n{index}"), source.as_bytes(), NULL);
    }
    let prompt = assert_imports(&root, &beacon, "quoted", b"---\n\"null\": x\nk: ~\n---\n## S\nb\n", false);
    assert_eq!(prompt.frontmatter.get("null"), Some(&json!("x")));
    assert_eq!(prompt.frontmatter.get("k"), Some(&Value::Null));
}

#[test]
fn a8_import_refuses_later_frontmatter_documents() {
    // RD19.
    const LATER: &str = "refused: the frontmatter holds more than one YAML document and only the first would be stored; nothing was stored";
    let (_dir, root, beacon) = project();
    for (name, source) in [
        ("spaced-marker", &b"---\nk: v\n--- \nj: w\n---\n## S\nb\n"[..]),
        ("end-marker", b"---\nk: v\n...\nj: w\n---\n## S\nb\n"),
        ("marker-with-content", b"---\nk: v\n--- j\n---\n## S\nb\n"),
    ] {
        assert_import_refused(&root, &beacon, name, source, LATER);
    }
    let prompt = assert_imports(&root, &beacon, "one", b"---\nk: v\n...\n---\n## S\nb\n", false);
    assert_eq!(prompt.frontmatter.get("k"), Some(&json!("v")));
}

#[test]
fn a8_import_refuses_a_frontmatter_name_other_than_the_imported_name() {
    // RD20. A name equal to the imported one is dropped as Go drops it.
    let (_dir, root, beacon) = project();
    for (index, (value, shown)) in [("other", "other"), ("\"\"", ""), ("5", "5"), ("[a]", "- a\n")]
        .iter()
        .enumerate()
    {
        let name = format!("named{index}");
        let source = format!("---\nname: {value}\n---\n## S\nb\n");
        let expected = format!(
            "refused: the frontmatter name {} differs from the name given and would be dropped; nothing was stored",
            serde_json::to_string(shown).expect("quote")
        );
        assert_import_refused(&root, &beacon, &name, source.as_bytes(), &expected);
    }
    let prompt = assert_imports(&root, &beacon, "same", b"---\nname: same\nk: v\n---\n## S\nb\n", false);
    assert!(!prompt.frontmatter.contains_key("name"));
    assert_eq!(prompt.frontmatter.len(), 1);
}

#[test]
fn a8_import_refuses_values_that_read_back_differently() {
    // RD21: Go's JSON reading would give a different number or U+FFFD.
    let (_dir, root, beacon) = project();
    for (index, (frontmatter, path)) in [
        ("n: 9007199254740993\n", "n"),
        ("a:\n  n: 18446744073709551615\n", "a.n"),
        ("big: -9223372036854775807\n", "big"),
        ("bin: !!binary /w==\n", "bin"),
        ("b: 1\na: {z: 1, y: 9007199254740995}\n", "a.y"),
    ]
    .iter()
    .enumerate()
    {
        let source = format!("---\n{frontmatter}---\n## S\nb\n");
        let expected = format!(
            "refused: the frontmatter value at {path} would read back from the store as a different value; nothing was stored"
        );
        assert_import_refused(&root, &beacon, &format!("r{index}"), source.as_bytes(), &expected);
    }
    // Exactly representable numbers, kinds and times pass (copied).
    let prompt = assert_imports(
        &root,
        &beacon,
        "exact",
        b"---\nn: 9007199254740992\nf: 1.0\nt: 2001-12-14T21:59:43.1Z\nbin: !!binary aGVsbG8=\n---\n## S\nb\n",
        false,
    );
    assert_eq!(prompt.frontmatter.get("t"), Some(&json!("2001-12-14T21:59:43.1Z")));
    assert_eq!(prompt.frontmatter.get("bin"), Some(&json!("hello")));
}

// ---- A8: emit -----------------------------------------------------------------

fn into_store(path: &Path) -> String {
    format!(
        "refused: emit would overwrite {} inside .luuna/ with a rendered prompt; nothing was written",
        path.display()
    )
}

#[test]
fn a8_emit_refuses_to_overwrite_files_inside_dot_luuna() {
    // RD22: every target is computed first; one that would overwrite a
    // differing file inside .luuna/ refuses the whole call before any write,
    // so earlier prompts' targets stay as they were.
    let (_dir, root, beacon) = project();
    create(&beacon, named("ordinary"));
    create(
        &beacon,
        luuna_beacon::CreatePrompt {
            emit_dir: ".luuna/beacon".into(),
            emit_as: "prompts.jsonl".into(),
            ..named("into-store")
        },
    );
    write_under(&root, "agents/ordinary.md", b"an earlier target\n");
    let target = root.join(PROMPTS);
    assert_refused(&root, "--all", &into_store(&target), || beacon.emit(&emit_all()));
    assert_refused(&root, "by name", &into_store(&target), || {
        beacon.emit(&emit_one("into-store"))
    });
    let mut forced = emit_all();
    forced.force = true;
    assert_refused(&root, "--force", &into_store(&target), || beacon.emit(&forced));
    // --check and --dry-run write nothing and are not refused.
    let mut check = emit_all();
    check.check = true;
    let stale_names = stale(beacon.emit(&check).expect("check"));
    assert!(stale_names.contains(&"into-store".to_owned()));
    let mut dry = emit_all();
    dry.dry_run = true;
    let before = tree_bytes(&root);
    beacon.emit(&dry).expect("dry run");
    assert_tree_unchanged(&before, &root, "dry run");

    // --out naming config.yaml, relative and absolute.
    let config = root.join(CONFIG);
    let mut options = emit_one("ordinary");
    options.out = Some(config.to_string_lossy().into_owned());
    assert_refused(&root, "absolute --out", &into_store(&config), || beacon.emit(&options));
    let mut options = emit_one("ordinary");
    options.out_dir = Some(".luuna/beacon".into());
    write_under(&root, ".luuna/beacon/ordinary.md", b"another component's file\n");
    let inside = root.join(".luuna/beacon/ordinary.md");
    assert_refused(&root, "--out-dir", &into_store(&inside), || beacon.emit(&options));
    // A folder link that resolves into .luuna/.
    symlink_into(&root, ".luuna/beacon", "linked");
    let mut options = emit_one("ordinary");
    options.out_dir = Some("linked".into());
    let through_link = root.join("linked/ordinary.md");
    assert_refused(&root, "a linked folder", &into_store(&through_link), || {
        beacon.emit(&options)
    });

    // A target inside .luuna/ that does not exist yet, or holds the same
    // bytes, loses nothing and is written as Go writes it.
    let mut options = emit_one("ordinary");
    options.out_dir = Some(".luuna/new-folder".into());
    beacon.emit(&options).expect("a new file inside .luuna");
    assert!(root.join(".luuna/new-folder/ordinary.md").is_file());
    let files = emitted(beacon.emit(&options).expect("the same bytes again"));
    assert!(files[0].skipped);
}

#[cfg(unix)]
fn symlink_into(root: &Path, target: &str, link: &str) {
    std::os::unix::fs::symlink(root.join(target), root.join(link)).expect("create a link");
}

#[cfg(not(unix))]
fn symlink_into(_root: &Path, _target: &str, _link: &str) {}

// ---- A8: pin and unpin --------------------------------------------------------

/// A project whose store is exactly `record`.
fn store_project(record: &[u8]) -> (TempDir, PathBuf, Beacon) {
    let (dir, root, beacon) = project();
    write_store(&root, record);
    (dir, root, beacon)
}

fn refuse_pin_and_unpin(loss: &LossCase) {
    for command in ["pin", "unpin"] {
        let (_dir, root, beacon) = store_project(loss.record.as_bytes());
        let label = format!("{} `{command}`", loss.id);
        let error = assert_refused(&root, &label, &loss.display(), || {
            if command == "pin" {
                beacon.pin(&format!("{}@1", loss.name))
            } else {
                beacon.unpin(loss.name)
            }
        });
        let (name, key) = match error {
            BeaconError::Authoring(AuthoringError::PinRecordKeyUnknown(name, key)) => {
                assert!(loss.unknown, "{label}: expected a repeated key");
                (name, key)
            }
            BeaconError::Authoring(AuthoringError::PinRecordKeyRepeated(name, key)) => {
                assert!(!loss.unknown, "{label}: expected an unknown key");
                (name, key)
            }
            other => panic!("{label}: unexpected error {other}"),
        };
        assert_eq!((name.as_str(), key.as_str()), (loss.name, loss.key), "{label}");
    }
}

#[test]
fn a8_pin_and_unpin_refuse_unknown_record_keys_and_leave_the_store_unchanged() {
    // RD23: b04a, b04b, b04c of go-lossy-probe-1, a record with both atlas
    // and mulch, and one with MULCH.
    let probed = probed_loss_cases();
    let unprobed = unprobed_loss_cases();
    let wanted = ["b04a", "b04b", "b04c", "r23-mulch-beside-atlas", "r23-MULCH"];
    let mut ran = 0;
    for loss in probed.iter().chain(unprobed.iter()) {
        if wanted.contains(&loss.id) {
            assert!(loss.unknown);
            refuse_pin_and_unpin(loss);
            ran += 1;
        }
    }
    assert_eq!(ran, wanted.len());
    // Bound keys are copied: a case variant (b05e), the legacy key Go uses
    // (b05i) and an explicit null (b05j).
    for row in ["b05e", "b05i", "b05j", "b00ctl"] {
        let (_dir, _root, beacon) = store_project(family_b(row));
        beacon
            .pin("p1@1")
            .unwrap_or_else(|error| panic!("row {row} is copied: {error}"));
    }
}

#[test]
fn a8_pin_and_unpin_refuse_repeated_record_keys_and_leave_the_store_unchanged() {
    // RD24, refused: b05d of go-lossy-probe-1, b05m's record pinned as
    // dup@1, a repeated section key with two different strings, a nested
    // frontmatter repeat with two different numbers, r24-fm-later-null and
    // r24-tags-later-null.
    let probed = probed_loss_cases();
    let unprobed = unprobed_loss_cases();
    let wanted = [
        "b05d",
        "b05m-as-dup",
        "r24-section-repeat",
        "r24-nested-frontmatter-repeat",
        "r24-fm-later-null",
        "r24-tags-later-null",
    ];
    let mut ran = 0;
    for loss in probed.iter().chain(unprobed.iter()) {
        if wanted.contains(&loss.id) {
            assert!(!loss.unknown);
            refuse_pin_and_unpin(loss);
            ran += 1;
        }
    }
    assert_eq!(ran, wanted.len());

    // RD24, copied: the discarded value is null or reads the same. For n1,
    // n2 and n3 of go-lossy-probe-2 the appended line is Go's new record
    // line from the probe with updatedAt replaced by the fixed clock's text.
    //   n1: table lines 5127-5128; Go's lines 1441 (pin), 1960 (unpin).
    //   n2: table lines 5129-5130; Go's lines 2486 (pin), 3016 (unpin).
    //   n3: table lines 5131-5132; Go's lines 3521 (pin), 4016 (unpin).
    for row in &PROBE_TWO {
        for command in ["pin", "unpin"] {
            let label = format!("{} `{command}`", row.id);
            let (_dir, root, beacon) = store_project(row.record.as_bytes());
            let result = if command == "pin" {
                beacon.pin("p1@1")
            } else {
                beacon.unpin("p1")
            };
            let returned = result.unwrap_or_else(|error| panic!("{label} is copied: {error}"));
            let go_line = if command == "pin" {
                row.pin_line
            } else {
                row.unpin_line
            };
            let expected = with_updated_at(go_line, FIXED_TIME_TEXT);
            let line = String::from_utf8(last_store_line(&root)).expect("UTF-8");
            assert_eq!(line, expected, "{label}: the appended line is Go's");
            let stored = fs::read(root.join(PROMPTS)).expect("store");
            assert!(stored.starts_with(row.record.as_bytes()), "{label}: append only");
            let shown = beacon.show("p1").expect("show");
            assert_eq!(shown, returned, "{label}: the returned prompt is the new record");
            match row.id {
                "n1" => assert_eq!(shown.description, "d", "{label}"),
                "n2" => assert_eq!(shown.tags, ["a"], "{label}"),
                "n3" => assert_eq!(
                    shown.frontmatter.get("k").and_then(Value::as_f64),
                    Some(1.0),
                    "{label}"
                ),
                _ => unreachable!(),
            }
            assert_eq!(shown.version, 2);
            assert_eq!(shown.pinned, (command == "pin").then_some(1));
        }
    }
    for (id, record) in [
        ("r24-fm-earlier-null", R24_FM_EARLIER_NULL),
        ("r24-fm-merge", R24_FM_MERGE),
    ] {
        for command in ["pin", "unpin"] {
            let (_dir, _root, beacon) = store_project(record.as_bytes());
            let result = if command == "pin" {
                beacon.pin("p1@1")
            } else {
                beacon.unpin("p1")
            };
            let shown = result.unwrap_or_else(|error| panic!("{id} `{command}` is copied: {error}"));
            if id == "r24-fm-earlier-null" {
                assert_eq!(shown.frontmatter.get("k"), Some(&json!("x")));
            } else {
                assert_eq!(shown.frontmatter.keys().collect::<Vec<_>>(), ["a", "b"]);
            }
        }
    }
    // Pin's own checks come first (AS22): a missing version is Go's error,
    // not a refusal.
    let (_dir, _root, beacon) = store_project(family_b("b05d"));
    let error = error_of(beacon.pin("p1@2"), "a missing version");
    assert_eq!(error.to_string(), "version 2 not found");
    let error = error_of(beacon.pin("p1"), "no version");
    assert_eq!(error.to_string(), "expected name@version");
    let _: Option<ChangeKind> = None;
    let _: Option<EmitSelection> = None;
}
