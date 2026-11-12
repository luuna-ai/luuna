//! Checks of the Go reference harness. The ignored tests build and run the
//! Go tool from the reference clone:
//! LUUNA_BEACON_REFERENCE=<clone> cargo test -p luuna-beacon --locked --test go_reference -- --ignored
//! The other tests need neither Go nor the clone.

mod support;

use serde_json::{json, Value};
use std::ffi::OsString;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::Path;
use support::go::{self, BUILD_DIR_VARS, DEFAULT_GOPROXY, PROXY_VARS, REFERENCE_COMMIT};
use support::normalise::{strip_runtime_fields, Normaliser};

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with LUUNA_BEACON_REFERENCE=<clone> cargo test -p luuna-beacon --locked --test go_reference -- --ignored"]
fn reference_clone_is_at_the_recorded_commit() {
    let clone = go::reference();
    go::check_reference(&clone);
    let head = go::git(&clone, &["rev-parse", "HEAD"]);
    assert!(head.status.success());
    assert_eq!(
        String::from_utf8_lossy(&head.stdout).trim(),
        REFERENCE_COMMIT
    );
}

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with LUUNA_BEACON_REFERENCE=<clone> cargo test -p luuna-beacon --locked --test go_reference -- --ignored"]
fn go_build_writes_nothing_into_the_reference_clone() {
    let build = go::go_build();
    assert_eq!(
        go::status_with_ignored(&build.clone),
        build.status_before,
        "the reference clone's status including ignored files changed"
    );
    go::check_reference(&build.clone);
    let manifests = go::git(
        &build.clone,
        &[
            "diff",
            "--exit-code",
            REFERENCE_COMMIT,
            "--",
            "go.mod",
            "go.sum",
        ],
    );
    assert!(
        manifests.status.success(),
        "go.mod or go.sum differs from the recorded commit:\n{}",
        String::from_utf8_lossy(&manifests.stdout)
    );
    assert!(!build.dir.starts_with(&build.clone));
    assert!(build.binary.starts_with(&build.dir));
    assert!(build.binary.is_file());
}

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with LUUNA_BEACON_REFERENCE=<clone> cargo test -p luuna-beacon --locked --test go_reference -- --ignored"]
fn go_binary_reports_version_0_2_6() {
    let project = tempfile::tempdir().expect("temporary project root");
    let root = project.path().canonicalize().expect("canonical root");
    let run = go::run_go(&root, &["--version"]);
    assert_eq!(
        run.code,
        0,
        "stderr: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(run.documents.len(), 1, "one JSON document");
    let doc = &run.documents[0];
    assert_eq!(doc["version"], "0.2.6");
    assert_eq!(doc["runtime"], "go");
}

#[test]
fn missing_reference_variable_is_a_hard_error() {
    assert!(go::reference_from(None).is_err());
    assert!(go::reference_from(Some(OsString::new())).is_err());
    let unset = catch_unwind(|| go::require_reference(None));
    assert!(unset.is_err(), "a missing reference panics, never skips");
}

#[test]
fn relative_reference_path_is_refused() {
    for relative in ["luuna-beacon", "./luuna-beacon", "../luuna-beacon"] {
        assert!(
            go::reference_from(Some(OsString::from(relative))).is_err(),
            "{relative} must be refused"
        );
    }
    let absolute = Path::new(env!("CARGO_TARGET_TMPDIR")).join("clone");
    assert_eq!(
        go::reference_from(Some(absolute.clone().into_os_string())),
        Ok(absolute)
    );
}

const FIXED_NAMES: [&str; 15] = [
    "PATH",
    "HOME",
    "XDG_CONFIG_HOME",
    "XDG_CACHE_HOME",
    "GOPATH",
    "GOCACHE",
    "GOMODCACHE",
    "GOTMPDIR",
    "TMPDIR",
    "GOTOOLCHAIN",
    "GOFLAGS",
    "GOPROXY",
    "GOWORK",
    "GOENV",
    "CGO_ENABLED",
];

fn assert_paths_under(env: &[(String, OsString)], build_dir: &Path) {
    for (name, _) in BUILD_DIR_VARS {
        let value = env
            .iter()
            .find(|(entry, _)| entry == name)
            .unwrap_or_else(|| panic!("{name} is set"));
        assert!(
            Path::new(&value.1).starts_with(build_dir) && Path::new(&value.1) != build_dir,
            "{name} is under the build directory"
        );
    }
}

#[test]
fn build_environment_keeps_every_go_path_under_the_build_dir() {
    let build_dir = go::build_dir();
    let path = OsString::from("/usr/bin:/bin");
    let env = go::go_build_env(&build_dir, DEFAULT_GOPROXY, &path, &[]);
    let names: Vec<&str> = env.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(names, FIXED_NAMES);
    assert_paths_under(&env, &build_dir);
    let value = |name: &str| {
        env.iter()
            .find(|(entry, _)| entry == name)
            .map(|(_, value)| value.clone())
            .unwrap()
    };
    assert_eq!(value("PATH"), path);
    assert_eq!(value("GOTOOLCHAIN"), "local");
    assert_eq!(value("GOFLAGS"), "-mod=readonly -modcacherw");
    assert_eq!(value("GOPROXY"), DEFAULT_GOPROXY);
    assert_eq!(value("GOWORK"), "off");
    assert_eq!(value("GOENV"), "off");
    assert_eq!(value("CGO_ENABLED"), "0");
    assert!(build_dir.is_absolute());
}

#[test]
fn proxy_pass_through_takes_only_the_six_standard_variables_that_are_set() {
    let set = [
        ("HTTPS_PROXY", "value-1"),
        ("http_proxy", "value-2"),
        ("no_proxy", "value-3"),
        ("ALL_PROXY", "value-4"),
        ("all_proxy", "value-5"),
        ("FTP_PROXY", "value-6"),
        ("GRPC_PROXY", "value-7"),
        ("DOCKER_HTTPS_PROXY", "value-8"),
        ("CLOUDSDK_PROXY_PASSWORD", "value-9"),
    ];
    let get = |name: &str| {
        set.iter()
            .find(|(entry, _)| *entry == name)
            .map(|(_, value)| OsString::from(value))
    };
    let proxy = go::proxy_pass_through(get);
    let expected: Vec<(String, OsString)> = [
        ("HTTPS_PROXY", "value-1"),
        ("http_proxy", "value-2"),
        ("no_proxy", "value-3"),
    ]
    .iter()
    .map(|(name, value)| ((*name).to_owned(), OsString::from(value)))
    .collect();
    assert_eq!(proxy, expected);
    assert!(proxy
        .iter()
        .all(|(name, _)| PROXY_VARS.contains(&name.as_str())));

    let build_dir = go::build_dir();
    let env = go::go_build_env(
        &build_dir,
        DEFAULT_GOPROXY,
        &OsString::from("/usr/bin:/bin"),
        &proxy,
    );
    assert_eq!(env.len(), 18);
    let names: Vec<&str> = env.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(names[..15], FIXED_NAMES);
    assert_eq!(env[15..], expected[..]);
    assert_paths_under(&env, &build_dir);
}

fn root() -> tempfile::TempDir {
    tempfile::tempdir().expect("temporary folder")
}

#[test]
fn ids_are_numbered_by_first_appearance() {
    let folder = root();
    let mut side = Normaliser::new("luuna-beacon", folder.path());
    let mut docs = vec![
        json!({"command": "create", "id": "luuna-beacon-18a2f3", "name": "a",
               "prompt": {"id": "luuna-beacon-18a2f3", "extends": "luuna-beacon-xyz"}}),
        json!({"command": "schema create", "id": "schema-1f", "name": "s"}),
        json!({"command": "update", "id": "luuna-beacon-18a2f3", "other": "other-abc"}),
    ];
    side.documents(&mut docs);
    assert_eq!(docs[0]["id"], "<id:1>");
    assert_eq!(docs[0]["prompt"]["id"], "<id:1>");
    assert_eq!(docs[0]["prompt"]["extends"], "luuna-beacon-xyz");
    assert_eq!(docs[1]["id"], "<id:2>");
    assert_eq!(
        docs[2]["id"], "<id:1>",
        "an id reused by update keeps its number"
    );
    assert_eq!(docs[2]["other"], "other-abc");

    let line = b"{\"id\":\"luuna-beacon-9c\",\"name\":\"x\",\"version\":1,\"schema\":\"schema-1f\",\"createdAt\":\"2026-11-12T12:00:00.5Z\",\"updatedAt\":\"2026-11-12T12:00:00.5Z\"}\n{\"id\":\"luuna-beacon-18a2f3\", \"x\": \"<\\u003c\"}\n";
    let normalised = side.jsonl(line);
    assert_eq!(
        String::from_utf8(normalised).unwrap(),
        "{\"id\":\"<id:3>\",\"name\":\"x\",\"version\":1,\"schema\":\"<id:2>\",\"createdAt\":\"<ts:1>\",\"updatedAt\":\"<ts:1>\"}\n{\"id\":\"<id:1>\", \"x\": \"<\\u003c\"}\n"
    );

    let mut other = Normaliser::new("luuna-beacon", folder.path());
    let mut doc = json!({"id": "luuna-beacon-ffff"});
    other.document(&mut doc);
    assert_eq!(doc["id"], "<id:1>", "each side numbers its own stream");
}

#[test]
fn timestamps_must_have_go_trimming() {
    let folder = root();
    let mut side = Normaliser::new("luuna-beacon", folder.path());
    let mut doc = json!({"prompt": {
        "createdAt": "2026-11-12T12:00:00Z",
        "updatedAt": "2026-11-12T12:00:00.123456789Z"},
        "versions": [{"createdAt": "2026-11-12T12:00:00Z", "updatedAt": "2026-11-12T12:00:00.5Z"}]});
    side.document(&mut doc);
    assert_eq!(doc["prompt"]["createdAt"], "<ts:1>");
    assert_eq!(doc["prompt"]["updatedAt"], "<ts:2>");
    assert_eq!(
        doc["versions"][0]["createdAt"], "<ts:1>",
        "equal values group"
    );
    assert_eq!(doc["versions"][0]["updatedAt"], "<ts:3>");

    for bad in [
        "2026-11-12T12:00:00.500Z",
        "2026-11-12T12:00:00.0Z",
        "2026-11-12T12:00:00+00:00",
        "2026-11-12 12:00:00Z",
        "2026-11-12T12:00:00.Z",
    ] {
        let failed = catch_unwind(AssertUnwindSafe(|| {
            let mut doc = json!({ "createdAt": bad });
            side.document(&mut doc);
        }));
        assert!(failed.is_err(), "{bad} must fail");
        let failed = catch_unwind(AssertUnwindSafe(|| {
            side.jsonl(format!("{{\"updatedAt\":\"{bad}\"}}\n").as_bytes());
        }));
        assert!(failed.is_err(), "{bad} in a store line must fail");
    }
    let failed = catch_unwind(AssertUnwindSafe(|| {
        side.document(&mut json!({ "createdAt": null }));
    }));
    assert!(failed.is_err(), "a timestamp that is not a string fails");
}

#[test]
fn project_root_is_replaced() {
    let folder = root();
    let canonical = folder.path().canonicalize().unwrap();
    let text = canonical.to_string_lossy().into_owned();
    let mut side = Normaliser::new("luuna-beacon", folder.path());
    let mut docs = vec![
        json!({"command": "init", "dir": text, "success": true}),
        json!({"command": "list", "error": format!("invalid JSONL line 2 in {text}/.luuna/beacon/prompts.jsonl: x"), "success": false}),
    ];
    side.documents(&mut docs);
    assert_eq!(docs[0]["dir"], "<root>");
    assert_eq!(
        docs[1]["error"],
        "invalid JSONL line 2 in <root>/.luuna/beacon/prompts.jsonl: x"
    );
    assert_eq!(side.text(&format!("{text}/x")), "<root>/x");
}

#[test]
fn map_ordered_arrays_are_sorted_by_name() {
    let folder = root();
    let mut side = Normaliser::new("luuna-beacon", folder.path());
    let names = |doc: &Value, key: &str| -> Vec<String> {
        doc[key]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| entry["name"].as_str().unwrap().to_owned())
            .collect()
    };
    for (command, key) in [
        ("list", "prompts"),
        ("validate", "results"),
        ("schema list", "schemas"),
    ] {
        let mut doc = json!({"command": command,
            key: [{"name": "c"}, {"name": "a"}, {"name": "b"}]});
        side.document(&mut doc);
        assert_eq!(names(&doc, key), ["a", "b", "c"], "{command}");
    }
    let mut history = json!({"command": "history",
        "versions": [{"name": "c"}, {"name": "a"}]});
    side.document(&mut history);
    assert_eq!(
        names(&history, "versions"),
        ["c", "a"],
        "other arrays keep order"
    );
}

#[test]
fn runtime_fields_are_stripped_only_from_render_and_named_validate() {
    let mut render =
        json!({"command": "render", "name": "p", "resolvedFrom": ["p"], "sections": []});
    let removed = strip_runtime_fields("render", &mut render);
    assert_eq!(Value::Object(removed), json!({"resolvedFrom": ["p"]}));
    assert_eq!(
        render,
        json!({"command": "render", "name": "p", "sections": []})
    );

    let result = json!({"name": "p", "valid": true, "errors": [], "warnings": []});
    let mut named = json!({"command": "validate", "results": [result.clone()],
        "valid": true, "errors": [], "warnings": [], "success": true});
    let removed = strip_runtime_fields("validate", &mut named);
    assert_eq!(
        Value::Object(removed),
        json!({"valid": true, "errors": [], "warnings": []})
    );
    assert_eq!(
        named,
        json!({"command": "validate", "results": [result.clone()], "success": true})
    );

    let mut all = json!({"command": "validate", "results": [result.clone(), result.clone()],
        "valid": true, "success": true});
    let before = all.clone();
    assert!(strip_runtime_fields("validate", &mut all).is_empty());
    assert_eq!(all, before, "validate with several results keeps every key");

    let mut failure =
        json!({"command": "validate", "error": "validation failed", "success": false});
    let before = failure.clone();
    assert!(strip_runtime_fields("validate", &mut failure).is_empty());
    assert_eq!(failure, before);

    for command in [
        "list",
        "show",
        "create",
        "schema show",
        "render --format json",
    ] {
        let mut doc = json!({"command": command, "resolvedFrom": ["p"], "valid": true,
            "errors": [], "warnings": [], "results": [result.clone()]});
        let before = doc.clone();
        assert!(
            strip_runtime_fields(command, &mut doc).is_empty(),
            "{command}"
        );
        assert_eq!(doc, before, "{command}");
    }
}
