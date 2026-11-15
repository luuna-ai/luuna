//! The stored format round-trips between the Go tool and this crate.
//!
//! The tests need a clone of luuna-ai/luuna-missions at
//! 2fbbf8e5b6a238ea47543e1dad112fe1716faec4 and a Go toolchain:
//!
//! ```text
//! LUUNA_MISSIONS_REFERENCE=<clone> LUUNA_MISSIONS_GO=<go> \
//! LUUNA_MISSIONS_GOPROXY=<proxy list> \
//! cargo test --locked -p luuna-missions --test format_roundtrip -- --ignored
//! ```
//!
//! Frozen after t1 of s4-missions-lib, and frozen for s4-missions-lib-2 to -4.

mod support;

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::path::Path;
use std::thread;
use std::time::{Duration, SystemTime};

use serde::de::{Deserializer, IgnoredAny, MapAccess, Visitor};
use serde_json::Value;

use luuna_missions::{DataFile, Store};
use support::reference::{self, GoTool};
use support::{
    args, child_env, exact_difference, finish_rust, guarded, parse_json, run_differences, run_go,
    run_rust, spawn_go, spawn_rust, tree_differences, Entry, Normaliser, Report, Run, TempProject,
};

/// How long the lock holder keeps the lock before releasing it.
const HOLD: Duration = Duration::from_secs(1);
/// How long a waiting command may take after the lock is released.
const AFTER_RELEASE: Duration = Duration::from_secs(35);

const CONFIG: &str = "project: \"project\"\nversion: \"1\"\nmax_plan_depth: 3\n";
const GITATTRIBUTES: &str = ".luuna/missions/issues.jsonl merge=union\n.luuna/missions/templates.jsonl merge=union\n.luuna/missions/plans.jsonl merge=union\n";

/// Writes an initialised store, as Go's `init` leaves it, with `issues` as
/// the content of issues.jsonl.
fn write_store(project: &TempProject, issues: &str) {
    project.write_files([
        (".gitattributes", GITATTRIBUTES.as_bytes().to_vec()),
        (".luuna/missions/.gitignore", b"*.lock\n".to_vec()),
        (".luuna/missions/config.yaml", CONFIG.as_bytes().to_vec()),
        (".luuna/missions/issues.jsonl", issues.as_bytes().to_vec()),
        (".luuna/missions/plans.jsonl", Vec::new()),
        (".luuna/missions/templates.jsonl", Vec::new()),
    ]);
}

fn issues_path(project: &TempProject) -> std::path::PathBuf {
    project.root().join(".luuna/missions/issues.jsonl")
}

fn read_issues(project: &TempProject) -> String {
    String::from_utf8(fs::read(issues_path(project)).expect("issues.jsonl"))
        .expect("UTF-8 issues.jsonl")
}

fn describe(run: &Run) -> String {
    format!(
        "exit {}\nstdout: {}\nstderr: {}",
        run.exit,
        support::excerpt(&run.stdout),
        support::excerpt(&run.stderr)
    )
}

/// Runs Go and requires exit 0.
fn go_ok(go: &GoTool, project: &TempProject, command: &[String]) -> Run {
    let run = run_go(go.binary(), project.root(), command, b"", &child_env(None))
        .unwrap_or_else(|error| panic!("go {command:?}: {error}"));
    assert_eq!(run.exit, 0, "go {command:?}: {}", describe(&run));
    run
}

/// Runs Rust and requires exit 0.
fn rust_ok(project: &TempProject, command: &[String]) -> Run {
    let run = run_rust(project.root(), command, b"", &child_env(None))
        .unwrap_or_else(|error| panic!("rust {command:?}: {error}"));
    assert_eq!(run.exit, 0, "rust {command:?}: {}", describe(&run));
    run
}

fn created_id(run: &Run) -> String {
    parse_json(&run.stdout)
        .and_then(|value| value.get("id").and_then(Value::as_str).map(str::to_owned))
        .unwrap_or_else(|| panic!("create printed no id: {}", describe(run)))
}

/// The ids of the store built by [`build_store`].
struct Built {
    alpha: String,
    beta: String,
    gamma: String,
}

/// Builds a store with tickets of every kind the ported commands write,
/// running each command through `run` (Go or Rust), which must succeed.
fn build_store(run: &mut dyn FnMut(&[String]) -> Run) -> Built {
    run(&args(&["init", "--json"]));
    let alpha = created_id(&run(&args(&[
        "create",
        "--title=Alpha <b>bold</b> & \"quoted\"",
        "--type=feature",
        "--priority=P1",
        "--labels=api,core",
        "--description=Line one\nLine two \u{2028} \u{fc} <tag> & more",
        "--json",
    ])));
    let beta = created_id(&run(&args(&[
        "create",
        "--title=Beta",
        "--type=bug",
        "--json",
    ])));
    let gamma = created_id(&run(&args(&[
        "create",
        "--title=Gamma",
        "--assignee=someone",
        "--json",
    ])));
    run(&args(&[
        "update",
        &alpha,
        "--extensions={\"keep\":{\"n\":[1,2.5,null]},\"html\":\"<&>\"}",
        "--json",
    ]));
    run(&args(&[
        "update",
        &beta,
        "--claim",
        "--as=parity",
        "--json",
    ]));
    run(&args(&["block", &alpha, "--by", &beta, "--json"]));
    run(&args(&["update", &gamma, "--status=closed", "--json"]));
    Built { alpha, beta, gamma }
}

/// Read commands over a store built by [`build_store`].
fn read_commands(built: &Built) -> Vec<Vec<String>> {
    let Built { alpha, beta, gamma } = built;
    vec![
        args(&["show", alpha, "--json"]),
        args(&["show", beta, "--json"]),
        args(&["show", gamma, "--json"]),
        args(&["show", alpha, beta, gamma, "--json"]),
        args(&["show", alpha]),
        args(&["list", "--json"]),
        args(&["list", "--all", "--json"]),
        args(&["list", "--all", "--sort=id", "--json"]),
        args(&["list", "--all"]),
        args(&["list", "--all", "--format=compact"]),
        args(&["list", "--all", "--format=plain"]),
        args(&["list", "--all", "--format=ids"]),
        args(&["list", "--label=api", "--json"]),
        args(&["search", "Alpha", "--json"]),
        args(&["search", "Beta"]),
        args(&["search", "NoSuchPhrase", "--json"]),
    ]
}

/// Exact comparison of two runs, with no normalisation at all.
fn exact_run_differences(go: &Run, rust: &Run) -> Vec<String> {
    let mut differences = Vec::new();
    if go.exit != rust.exit {
        differences.push(format!("exit {}, want {}", rust.exit, go.exit));
    }
    differences.extend(exact_difference("stdout", &go.stdout, &rust.stdout));
    differences.extend(exact_difference("stderr", &go.stderr, &rust.stderr));
    differences
}

/// Exact comparison of two snapshots of one tree.
fn exact_tree_differences(
    before: &BTreeMap<String, Entry>,
    after: &BTreeMap<String, Entry>,
) -> Vec<String> {
    let mut differences = Vec::new();
    for (name, entry) in before {
        match after.get(name) {
            None => differences.push(format!("{name} was removed")),
            Some(other) if other != entry => differences.push(format!("{name} changed")),
            Some(_) => {}
        }
    }
    for name in after.keys().filter(|name| !before.contains_key(*name)) {
        differences.push(format!("{name} was added"));
    }
    differences
}

/// Runs every read command through Go and through Rust on the same store and
/// requires identical bytes and an unchanged store.
fn compare_reads(report: &mut Report, go: &GoTool, project: &TempProject, built: &Built) {
    let env = child_env(None);
    let before = project.snapshot();
    for command in read_commands(built) {
        let case = command.join(" ");
        let go_run = run_go(go.binary(), project.root(), &command, b"", &env)
            .unwrap_or_else(|error| panic!("go {case}: {error}"));
        let mut differences = exact_tree_differences(&before, &project.snapshot())
            .into_iter()
            .map(|difference| format!("go changed the store: {difference}"))
            .collect::<Vec<_>>();
        match run_rust(project.root(), &command, b"", &env) {
            Ok(rust_run) => differences.extend(exact_run_differences(&go_run, &rust_run)),
            Err(error) => differences.push(format!("rust: {error}")),
        }
        differences.extend(
            exact_tree_differences(&before, &project.snapshot())
                .into_iter()
                .map(|difference| format!("rust changed the store: {difference}")),
        );
        report.case(&case, serde_json::Map::new(), differences);
    }
}

/// The ids of the records in issues.jsonl, in file order, first occurrence
/// only, as Go reads them.
fn file_ids(issues: &str) -> Vec<String> {
    let mut ids = Vec::new();
    for line in issues.lines() {
        if let Some(id) = parse_json(line.as_bytes())
            .and_then(|value| value.get("id").and_then(Value::as_str).map(str::to_owned))
        {
            if !ids.contains(&id) {
                ids.push(id);
            }
        }
    }
    ids
}

/// A store Go wrote is read by Rust: every read command prints exactly what
/// Go prints on the same store, nothing in the store changes, and
/// `Store::read_tickets` returns the records in file order.
#[test]
#[ignore = "requires LUUNA_MISSIONS_REFERENCE and LUUNA_MISSIONS_GO"]
fn go_written_store_reads_in_rust() {
    let clone = reference::reference_from_env();
    guarded("go_written_store_reads_in_rust", &clone, |report| {
        let go = reference::go_tool(&clone);
        report.record_go(go);
        let project = TempProject::new();
        let built = build_store(&mut |command| go_ok(go, &project, command));
        compare_reads(report, go, &project, &built);
        let expected = file_ids(&read_issues(&project));
        let found = Store::open(project.root().join(".luuna/missions"))
            .and_then(|store| store.read_tickets())
            .map(|tickets| {
                tickets
                    .iter()
                    .map(|ticket| ticket.id().to_owned())
                    .collect::<Vec<_>>()
            });
        let differences = match found {
            Ok(ids) if ids == expected => Vec::new(),
            Ok(ids) => vec![format!("read_tickets returned {ids:?}, want {expected:?}")],
            Err(error) => vec![format!("read_tickets: {error}")],
        };
        report.case("Store::read_tickets", serde_json::Map::new(), differences);
    });
}

/// A store Rust wrote is read by Go: Go's `show --json` and `list --all
/// --json` print exactly what Rust prints, every other read command agrees,
/// and Go's `doctor --json` reports no failure.
#[test]
#[ignore = "requires LUUNA_MISSIONS_REFERENCE and LUUNA_MISSIONS_GO"]
fn rust_written_store_reads_in_go() {
    let clone = reference::reference_from_env();
    guarded("rust_written_store_reads_in_go", &clone, |report| {
        let go = reference::go_tool(&clone);
        report.record_go(go);
        let project = TempProject::new();
        let built = build_store(&mut |command| rust_ok(&project, command));
        compare_reads(report, go, &project, &built);
        let before = project.snapshot();
        let doctor = go_ok_or_failing(go, &project, &args(&["doctor", "--json"]));
        let mut differences = exact_tree_differences(&before, &project.snapshot());
        let failures = parse_json(&doctor.stdout)
            .and_then(|value| value.pointer("/summary/fail").and_then(Value::as_u64));
        if failures != Some(0) {
            differences.push(format!("go doctor reports failures: {}", describe(&doctor)));
        }
        report.case("doctor --json", serde_json::Map::new(), differences);
    });
}

/// Runs Go without requiring exit 0 (doctor exits 1 when it finds failures).
fn go_ok_or_failing(go: &GoTool, project: &TempProject, command: &[String]) -> Run {
    run_go(go.binary(), project.root(), command, b"", &child_env(None))
        .unwrap_or_else(|error| panic!("go {command:?}: {error}"))
}

/// Records written compactly in unusual shapes: key order other than Go's
/// struct order, unknown keys, a null known value, empty arrays, escapes Go
/// would not write itself (`\u00fc`, `1.50`, `1e2`), and a malformed line.
/// Line 3 holds literal `<`, `&` and U+2028, which Go re-escapes whenever it
/// rewrites the file (SD17), so it is compared with Go but not required to
/// stay byte-identical.
const UNUSUAL_ISSUES: &[&str] = &[
    "{\"id\":\"project-r001\",\"title\":\"Plain record\",\"status\":\"open\",\"type\":\"task\",\"priority\":2,\"createdAt\":\"2026-01-01T00:00:00.000Z\",\"updatedAt\":\"2026-01-01T00:00:00.000Z\"}",
    "{\"title\":\"Escaped \\u003cb\\u003e \\u0026 \\\"q\\\" \\u00fc \u{fc} \\u2028 \\t\",\"id\":\"project-r002\",\"zzz\":{\"b\":1,\"a\":[true,null,1.50,1e2,\"\\u003c\"]},\"status\":\"in_progress\",\"type\":\"bug\",\"priority\":0,\"createdAt\":\"2026-01-02T00:00:00.000Z\",\"updatedAt\":\"2026-01-02T00:00:00.000Z\",\"assignee\":null,\"labels\":[],\"extensions\":{\"k\":\"v\",\"n\":{\"x\":[]}}}",
    "{\"id\":\"project-r003\",\"title\":\"Literal <i> & \u{fc} \u{2028}\",\"status\":\"closed\",\"type\":\"feature\",\"priority\":4,\"createdAt\":\"2026-01-03T00:00:00.000Z\",\"updatedAt\":\"2026-01-03T00:00:00.000Z\",\"closedAt\":\"2026-01-03T00:00:00.000Z\",\"closeReason\":\"done\",\"blockedBy\":[],\"blocks\":[],\"x\":\"<&>\"}",
    "{not json",
    "{\"id\":\"project-r004\",\"title\":\"Target\",\"status\":\"open\",\"type\":\"task\",\"priority\":2,\"createdAt\":\"2026-01-04T00:00:00.000Z\",\"updatedAt\":\"2026-01-04T00:00:00.000Z\",\"description\":\"before\"}",
    "{\"id\":\"project-r005\",\"future\":\"field\",\"title\":\"After target\",\"status\":\"open\",\"type\":\"task\",\"priority\":3,\"createdAt\":\"2026-01-05T00:00:00.000Z\",\"updatedAt\":\"2026-01-05T00:00:00.000Z\"}",
];

fn unusual_store() -> String {
    UNUSUAL_ISSUES
        .iter()
        .map(|line| format!("{line}\n"))
        .collect()
}

/// One command run by Go and by Rust on two copies of the same store.
struct Pair {
    go_project: TempProject,
    rust_project: TempProject,
    go_run: Run,
    rust_run: Result<Run, String>,
}

impl Pair {
    fn run(go: &GoTool, issues: &str, command: &[String]) -> Self {
        let go_project = TempProject::new();
        write_store(&go_project, issues);
        let rust_project = TempProject::new();
        write_store(&rust_project, issues);
        let env = child_env(None);
        let go_run = run_go(go.binary(), go_project.root(), command, b"", &env)
            .unwrap_or_else(|error| panic!("go {command:?}: {error}"));
        assert_eq!(go_run.exit, 0, "go {command:?}: {}", describe(&go_run));
        let rust_run = run_rust(rust_project.root(), command, b"", &env);
        Self {
            go_project,
            rust_project,
            go_run,
            rust_run,
        }
    }

    /// Rust against Go byte for byte after normalising roots, timestamps and
    /// (for a create) the new id, over the streams and the whole tree.
    fn differences(&self, creates: bool) -> Vec<String> {
        let rust_run = match &self.rust_run {
            Ok(run) => run,
            Err(error) => return vec![format!("rust: {error}")],
        };
        let mut go_norm = Normaliser::new().replace(
            self.go_project.root().to_string_lossy().as_bytes(),
            "<PROJECT>",
        );
        let mut rust_norm = Normaliser::new().replace(
            self.rust_project.root().to_string_lossy().as_bytes(),
            "<PROJECT>",
        );
        if creates {
            let id = created_id(&self.go_run);
            go_norm = go_norm.ids(vec![("created".to_owned(), id.clone(), id)]);
            if let Some(id) = parse_json(&rust_run.stdout)
                .and_then(|value| value.get("id").and_then(Value::as_str).map(str::to_owned))
            {
                rust_norm = rust_norm.ids(vec![("created".to_owned(), id.clone(), id)]);
            }
        }
        let mut differences = run_differences(&self.go_run, &go_norm, rust_run, &rust_norm);
        differences.extend(tree_differences(
            &self.go_project.snapshot(),
            &go_norm,
            &self.rust_project.snapshot(),
            &rust_norm,
        ));
        differences
    }
}

/// Every original record line that `kept` names appears unchanged, in the
/// original order, in `after`.
fn unchanged_lines(side: &str, after: &str, kept: &[usize]) -> Vec<String> {
    let lines: Vec<&str> = after.lines().collect();
    let mut position = 0;
    let mut differences = Vec::new();
    for index in kept {
        let original = UNUSUAL_ISSUES[*index];
        match lines[position..].iter().position(|line| *line == original) {
            Some(offset) => position += offset + 1,
            None => differences.push(format!(
                "{side}: record line {} was not kept byte for byte in order\nwant: {original}\nfile: {after}",
                index + 1
            )),
        }
    }
    differences
}

/// A8 and A10 (s4-missions-lib-refuse, DL100 and DL111): Go exits 0 (checked
/// in `Pair::run`) and drops the malformed line 4 of `UNUSUAL_ISSUES`; the
/// port refuses with the JSON error envelope, exit 1, and leaves its whole
/// tree byte for byte as `write_store` wrote it.
fn refused_unchanged(pair: &Pair, original: &str, command: &[String]) -> Vec<String> {
    let mut differences = Vec::new();
    if read_issues(&pair.go_project)
        .lines()
        .any(|line| line == UNUSUAL_ISSUES[3])
    {
        differences.push("go: the malformed line 4 was kept, but Go drops it".to_owned());
    }
    let expected = TempProject::new();
    write_store(&expected, original);
    match &pair.rust_run {
        Ok(run) => {
            if run.exit != 1 {
                differences.push(format!("rust {command:?}: exit {}, want 1", run.exit));
            }
            let want = format!(
                "{{\n  \"success\": false,\n  \"command\": \"{}\",\n  \"error\": \"{REFUSED_LINE_4}\"\n}}\n",
                command[0]
            );
            differences.extend(exact_difference("stdout", want.as_bytes(), &run.stdout));
            differences.extend(exact_difference("stderr", b"", &run.stderr));
        }
        Err(error) => differences.push(format!("rust: {error}")),
    }
    differences.extend(
        exact_tree_differences(&expected.snapshot(), &pair.rust_project.snapshot())
            .into_iter()
            .map(|difference| format!("rust changed the store: {difference}")),
    );
    differences
}

/// The refusal (a) for line 4 of `UNUSUAL_ISSUES`.
const REFUSED_LINE_4: &str = "Refusing to rewrite issues.jsonl: line 4 is not a ticket record the Go tool can read, and rewriting the file would drop it. Nothing was written.";

/// Rewriting the store keeps every unchanged record byte for byte, as Go
/// does, both for a full rewrite (update, block) and for an append (create).
#[test]
#[ignore = "requires LUUNA_MISSIONS_REFERENCE and LUUNA_MISSIONS_GO"]
fn unchanged_records_rewrite_byte_identical() {
    let clone = reference::reference_from_env();
    guarded(
        "unchanged_records_rewrite_byte_identical",
        &clone,
        |report| {
            let go = reference::go_tool(&clone);
            report.record_go(go);
            let original = unusual_store();
            let cases: [(&str, Vec<String>, bool, &[usize]); 3] = [
                (
                    "full rewrite by update",
                    args(&["update", "project-r004", "--title=Changed", "--json"]),
                    false,
                    &[0, 1, 5],
                ),
                (
                    "full rewrite by block",
                    args(&["block", "project-r001", "--by", "project-r005", "--json"]),
                    false,
                    &[1, 4],
                ),
                (
                    "append by create",
                    args(&["create", "--title=Appended", "--json"]),
                    true,
                    &[0, 1, 2, 3, 4, 5],
                ),
            ];
            for (case, command, creates, kept) in cases {
                let pair = Pair::run(go, &original, &command);
                let mut differences = unchanged_lines("go", &read_issues(&pair.go_project), kept);
                if creates && !read_issues(&pair.go_project).starts_with(&original) {
                    differences.push("go: append did not keep the file as a prefix".to_owned());
                }
                if !creates {
                    differences.extend(refused_unchanged(&pair, &original, &command));
                    report.case(case, serde_json::Map::new(), differences);
                    continue;
                }
                if pair.rust_run.is_ok() {
                    let rust_issues = read_issues(&pair.rust_project);
                    differences.extend(unchanged_lines("rust", &rust_issues, kept));
                    if creates && !rust_issues.starts_with(&original) {
                        differences
                            .push("rust: append did not keep the file as a prefix".to_owned());
                    }
                }
                differences.extend(pair.differences(creates));
                report.case(case, serde_json::Map::new(), differences);
            }
        },
    );
}

/// The top-level keys of a JSON object, in order.
fn object_keys(line: &str) -> Vec<String> {
    struct Keys;
    impl<'de> Visitor<'de> for Keys {
        type Value = Vec<String>;
        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str("a JSON object")
        }
        fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
            let mut keys = Vec::new();
            while let Some(key) = map.next_key::<String>()? {
                map.next_value::<IgnoredAny>()?;
                keys.push(key);
            }
            Ok(keys)
        }
    }
    let mut deserializer = serde_json::Deserializer::from_str(line);
    deserializer
        .deserialize_map(Keys)
        .unwrap_or_else(|error| panic!("not a JSON object ({error}): {line}"))
}

const KEEPER: &str = r#"{"id":"project-u001","x_first":{"keep":[1,null,"<&>"],"z":{}},"title":"Unknown keeper","status":"open","type":"task","priority":2,"createdAt":"2026-02-01T00:00:00.000Z","updatedAt":"2026-02-01T00:00:00.000Z","extensions":{"known":1,"nested":{"deep":true,"list":[null]}},"x_last":null}"#;
const OTHER: &str = r#"{"id":"project-u002","title":"Neighbour","status":"open","type":"task","priority":3,"createdAt":"2026-02-02T00:00:00.000Z","updatedAt":"2026-02-02T00:00:00.000Z","x_other":[{"a":"b"}]}"#;

enum Side {
    Go,
    Rust,
}

/// Runs `command` on `project` through `side`; a failure is returned as text.
fn step(
    go: &GoTool,
    side: &Side,
    project: &TempProject,
    command: &[String],
) -> Result<Run, String> {
    let env = child_env(None);
    let run = match side {
        Side::Go => run_go(go.binary(), project.root(), command, b"", &env)?,
        Side::Rust => run_rust(project.root(), command, b"", &env)?,
    };
    if run.exit == 0 {
        Ok(run)
    } else {
        Err(format!("{command:?}: {}", describe(&run)))
    }
}

/// Unknown fields, nested unknown values and the original key order survive
/// when Go and Rust take turns writing the same records.
#[test]
#[ignore = "requires LUUNA_MISSIONS_REFERENCE and LUUNA_MISSIONS_GO"]
fn unknown_fields_survive_both_directions() {
    let clone = reference::reference_from_env();
    guarded("unknown_fields_survive_both_directions", &clone, |report| {
        let go = reference::go_tool(&clone);
        report.record_go(go);
        let store = format!("{KEEPER}\n{OTHER}\n");
        let commands = [
            args(&["update", "project-u001", "--title=First edit", "--json"]),
            args(&["update", "project-u001", "--priority=P3", "--json"]),
            args(&["show", "project-u001", "--json"]),
        ];
        let orders = [
            ("go, go, go", [Side::Go, Side::Go, Side::Go]),
            ("go, rust, go", [Side::Go, Side::Rust, Side::Go]),
            ("rust, go, rust", [Side::Rust, Side::Go, Side::Rust]),
        ];
        let original = parse_json(KEEPER.as_bytes()).expect("keeper JSON");
        let original_keys = object_keys(KEEPER);
        let mut results: Vec<(String, Vec<u8>, Vec<u8>)> = Vec::new();
        for (name, sides) in orders {
            let project = TempProject::new();
            write_store(&project, &store);
            let mut differences = Vec::new();
            let mut shown = None;
            for (side, command) in sides.iter().zip(&commands) {
                match step(go, side, &project, command) {
                    Ok(run) => shown = Some(run.stdout),
                    Err(error) => {
                        differences.push(error);
                        shown = None;
                        break;
                    }
                }
            }
            if let Some(shown) = shown {
                let issues = read_issues(&project);
                let lines: Vec<&str> = issues.lines().collect();
                if lines.len() != 2 || lines[1] != OTHER {
                    differences.push(format!("the neighbouring record changed: {issues}"));
                }
                let keeper = lines.first().copied().unwrap_or_default();
                let record = parse_json(keeper.as_bytes()).unwrap_or(Value::Null);
                for key in ["x_first", "x_last", "extensions"] {
                    if record.get(key) != original.get(key) {
                        differences.push(format!("{key} did not survive: {keeper}"));
                    }
                }
                if keeper.starts_with('{') && object_keys(keeper) != original_keys {
                    differences.push(format!("the key order changed: {keeper}"));
                }
                let issue = parse_json(&shown).and_then(|value| value.get("issue").cloned());
                for key in ["x_first", "x_last", "extensions"] {
                    if issue.as_ref().and_then(|issue| issue.get(key)) != original.get(key) {
                        differences.push(format!(
                            "show lost {key}: {}",
                            String::from_utf8_lossy(&shown)
                        ));
                    }
                }
                let normaliser = Normaliser::new();
                results.push((
                    name.to_owned(),
                    normaliser.apply(issues.as_bytes()),
                    normaliser.apply(&shown),
                ));
            }
            report.case(name, serde_json::Map::new(), differences);
        }
        if let Some((_, go_file, go_show)) =
            results.iter().find(|(name, _, _)| name == "go, go, go")
        {
            for (name, file, show) in results.iter().filter(|(name, _, _)| name != "go, go, go") {
                let mut differences = Vec::new();
                differences.extend(exact_difference("issues.jsonl", go_file, file));
                differences.extend(exact_difference("show", go_show, show));
                report.case(
                    &format!("{name} equals go, go, go"),
                    serde_json::Map::new(),
                    differences,
                );
            }
        }
    });
}

/// Creates a lock file the way Go does: exclusively, empty, mode 0600.
fn create_go_lock(path: &Path) {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    options
        .open(path)
        .unwrap_or_else(|error| panic!("create {}: {error}", path.display()));
}

/// Entries of the store folder that are locks, stale-lock sidecars or temp
/// files.
fn leftovers(project: &TempProject) -> Vec<String> {
    project
        .snapshot()
        .into_keys()
        .filter(|name| name.contains(".lock") || name.contains(".tmp"))
        .collect()
}

/// A lock held through `Store::lock` makes Go wait until it is released; a
/// lock file in Go's format makes Rust wait until it is removed; a stale
/// lock (older than 30 s) is reclaimed by both alike.
#[test]
#[ignore = "requires LUUNA_MISSIONS_REFERENCE and LUUNA_MISSIONS_GO"]
fn lock_files_are_honoured_across_implementations() {
    let clone = reference::reference_from_env();
    guarded(
        "lock_files_are_honoured_across_implementations",
        &clone,
        |report| {
            let go = reference::go_tool(&clone);
            report.record_go(go);
            let env = child_env(None);
            let create = args(&["create", "--title=Waited", "--json"]);

            // Go waits for a lock Rust holds.
            let mut differences = Vec::new();
            let project = TempProject::new();
            write_store(&project, "");
            let lock_path = project.root().join(".luuna/missions/issues.jsonl.lock");
            match Store::open(project.root().join(".luuna/missions"))
                .and_then(|store| store.lock(DataFile::Issues))
            {
                Err(error) => differences.push(format!("rust lock: {error}")),
                Ok(lock) => {
                    if lock.path() != lock_path {
                        differences.push(format!(
                            "the lock is {}, want {}",
                            lock.path().display(),
                            lock_path.display()
                        ));
                    }
                    match fs::metadata(&lock_path) {
                        Ok(meta) => {
                            if meta.len() != 0 {
                                differences.push("the lock file is not empty".to_owned());
                            }
                            #[cfg(unix)]
                            {
                                let mode =
                                    std::os::unix::fs::PermissionsExt::mode(&meta.permissions())
                                        & 0o777;
                                if mode != 0o600 {
                                    differences
                                        .push(format!("the lock file has mode {mode:o}, want 600"));
                                }
                            }
                        }
                        Err(error) => {
                            differences.push(format!("no lock file while Rust holds it: {error}"))
                        }
                    }
                    let mut waiting = spawn_go(go.binary(), project.root(), &create, b"", &env);
                    thread::sleep(HOLD);
                    if waiting.has_exited() {
                        differences.push("go did not wait for the lock Rust holds".to_owned());
                    }
                    if !read_issues(&project).is_empty() {
                        differences.push("go wrote while Rust held the lock".to_owned());
                    }
                    drop(lock);
                    match waiting.finish(HOLD + AFTER_RELEASE) {
                        Ok(run) if run.exit == 0 => {
                            if file_ids(&read_issues(&project)).len() != 1 {
                                differences.push(
                                    "go did not write after the lock was released".to_owned(),
                                );
                            }
                        }
                        Ok(run) => differences
                            .push(format!("go failed after the release: {}", describe(&run))),
                        Err(error) => differences.push(error),
                    }
                    let left = leftovers(&project);
                    if !left.is_empty() {
                        differences.push(format!("left behind: {left:?}"));
                    }
                }
            }
            report.case(
                "go waits for a lock rust holds",
                serde_json::Map::new(),
                differences,
            );

            // Rust waits for a lock file in Go's format.
            let mut differences = Vec::new();
            let project = TempProject::new();
            write_store(&project, "");
            let lock_path = project.root().join(".luuna/missions/issues.jsonl.lock");
            create_go_lock(&lock_path);
            let waiting = spawn_rust(project.root(), &create, b"", &env);
            thread::sleep(HOLD);
            match waiting.try_recv() {
                Ok(Ok(run)) => differences.push(format!(
                    "rust did not wait for the lock: {}",
                    describe(&run)
                )),
                Ok(Err(error)) => differences.push(format!("rust: {error}")),
                Err(_) => {
                    if !read_issues(&project).is_empty() {
                        differences.push("rust wrote while the lock was held".to_owned());
                    }
                    fs::remove_file(&lock_path).expect("release the lock");
                    match finish_rust(&waiting, AFTER_RELEASE) {
                        Ok(run) if run.exit == 0 => {
                            if file_ids(&read_issues(&project)).len() != 1 {
                                differences.push(
                                    "rust did not write after the lock was released".to_owned(),
                                );
                            }
                        }
                        Ok(run) => differences
                            .push(format!("rust failed after the release: {}", describe(&run))),
                        Err(error) => differences.push(error),
                    }
                    let left = leftovers(&project);
                    if !left.is_empty() {
                        differences.push(format!("left behind: {left:?}"));
                    }
                }
            }
            report.case(
                "rust waits for a lock in go's format",
                serde_json::Map::new(),
                differences,
            );

            // A stale lock is reclaimed by both, leaving no lock or sidecar.
            let mut differences = Vec::new();
            let stale = |project: &TempProject| {
                let path = project.root().join(".luuna/missions/issues.jsonl.lock");
                create_go_lock(&path);
                let file = fs::File::options()
                    .write(true)
                    .open(&path)
                    .expect("lock file");
                file.set_modified(SystemTime::now() - Duration::from_secs(60))
                    .expect("age the lock");
            };
            let go_project = TempProject::new();
            write_store(&go_project, "");
            stale(&go_project);
            let rust_project = TempProject::new();
            write_store(&rust_project, "");
            stale(&rust_project);
            let go_run = run_go(go.binary(), go_project.root(), &create, b"", &env);
            let rust_run = run_rust(rust_project.root(), &create, b"", &env);
            match (&go_run, &rust_run) {
                (Ok(go_run), Ok(rust_run)) => {
                    if go_run.exit != 0 {
                        differences.push(format!("go: {}", describe(go_run)));
                    }
                    let go_id = created_id(go_run);
                    let go_norm = Normaliser::new()
                        .replace(go_project.root().to_string_lossy().as_bytes(), "<PROJECT>")
                        .ids(vec![("created".to_owned(), go_id.clone(), go_id)]);
                    let mut rust_norm = Normaliser::new().replace(
                        rust_project.root().to_string_lossy().as_bytes(),
                        "<PROJECT>",
                    );
                    if let Some(id) = parse_json(&rust_run.stdout).and_then(|value| {
                        value.get("id").and_then(Value::as_str).map(str::to_owned)
                    }) {
                        rust_norm = rust_norm.ids(vec![("created".to_owned(), id.clone(), id)]);
                    }
                    differences.extend(run_differences(go_run, &go_norm, rust_run, &rust_norm));
                    differences.extend(tree_differences(
                        &go_project.snapshot(),
                        &go_norm,
                        &rust_project.snapshot(),
                        &rust_norm,
                    ));
                }
                (Err(error), _) => differences.push(format!("go: {error}")),
                (_, Err(error)) => differences.push(format!("rust: {error}")),
            }
            for (side, project) in [("go", &go_project), ("rust", &rust_project)] {
                let left = leftovers(project);
                if !left.is_empty() {
                    differences.push(format!("{side} left behind: {left:?}"));
                }
            }
            report.case(
                "a stale lock is reclaimed alike",
                serde_json::Map::new(),
                differences,
            );
        },
    );
}
