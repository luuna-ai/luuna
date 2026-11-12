//! The parity tests of s4-missions-lib-2, the plan that owns this file.
//!
//! For each group in `tests/support/fixtures/s4_missions_lib_2.rs`, this
//! file holds one `parity_*` wrapper that calls
//! `crate::support::replay_group` with the group's name.
//!
//! It also holds four Go comparisons that cover what the fixtures do not:
//! [`help_and_version_shapes_match_go`], [`workflow_shapes_match_go`],
//! [`plan_context_in_ready_and_blocked_matches_go`] and
//! [`lossy_rewrites_are_refused_where_go_drops_data`]. Each case runs through
//! the Go tool built from the reference clone and through
//! `luuna_missions::command::run`, each side in its own `TempProject` with
//! `child_env(None)` plus only the variables the case names. Every store is
//! written literally below; none is generated from the code under test.
//! Unless a case says otherwise, Rust must equal Go byte for byte after the
//! project root is replaced by `<PROJECT>` and ISO timestamps are masked, in
//! stdout, stderr, exit code and the whole project tree, file modes
//! included; a case where Go itself does not give the case's stated Go
//! expectation fails too. Go-side failures are reported with the prefix
//! `go:`, Rust-side ones with `rust:`.
//!
//! The lossy-rewrite comparison is the one test where Rust is expected to
//! differ from Go: where the Go tool would silently drop data when it
//! rewrites `issues.jsonl` (charter A8), the port refuses with the exact text
//! of the plan and leaves the project unchanged. Its controls are the cells
//! where Go keeps the bytes, and there Rust must equal Go.
//!
//! Every input is written literally and is under 1 KiB per line; no JSON is
//! nested deeper than three levels. Nothing runs git except through the
//! harness, and nothing writes outside the test projects.

use std::collections::BTreeMap;
use std::fs;

use serde::de::{Deserialize, Deserializer, IgnoredAny, MapAccess, Visitor};
use serde_json::Value;

use crate::support::reference::{self, GoTool, Reference};
use crate::support::{
    args, child_env, exact_difference, excerpt, guarded, parse_json, run_differences, run_go,
    run_rust, tree_differences, Entry, EntryKind, Normaliser, Report, Run, TempProject,
};

#[test]
#[ignore = "requires LUUNA_MISSIONS_REFERENCE and LUUNA_MISSIONS_GO"]
fn parity_dispatch() {
    crate::support::replay_group("parity_dispatch");
}

#[test]
#[ignore = "requires LUUNA_MISSIONS_REFERENCE and LUUNA_MISSIONS_GO"]
fn parity_leading_format() {
    crate::support::replay_group("parity_leading_format");
}

#[test]
#[ignore = "requires LUUNA_MISSIONS_REFERENCE and LUUNA_MISSIONS_GO"]
fn parity_version_after_command() {
    crate::support::replay_group("parity_version_after_command");
}

#[test]
#[ignore = "requires LUUNA_MISSIONS_REFERENCE and LUUNA_MISSIONS_GO"]
fn parity_unknown_command() {
    crate::support::replay_group("parity_unknown_command");
}

#[test]
#[ignore = "requires LUUNA_MISSIONS_REFERENCE and LUUNA_MISSIONS_GO"]
fn parity_ready() {
    crate::support::replay_group("parity_ready");
}

#[test]
#[ignore = "requires LUUNA_MISSIONS_REFERENCE and LUUNA_MISSIONS_GO"]
fn parity_blocked() {
    crate::support::replay_group("parity_blocked");
}

#[test]
#[ignore = "requires LUUNA_MISSIONS_REFERENCE and LUUNA_MISSIONS_GO"]
fn parity_stats() {
    crate::support::replay_group("parity_stats");
}

#[test]
#[ignore = "requires LUUNA_MISSIONS_REFERENCE and LUUNA_MISSIONS_GO"]
fn parity_close() {
    crate::support::replay_group("parity_close");
}

#[test]
#[ignore = "requires LUUNA_MISSIONS_REFERENCE and LUUNA_MISSIONS_GO"]
fn parity_dep() {
    crate::support::replay_group("parity_dep");
}

#[test]
#[ignore = "requires LUUNA_MISSIONS_REFERENCE and LUUNA_MISSIONS_GO"]
fn parity_label() {
    crate::support::replay_group("parity_label");
}

#[test]
#[ignore = "requires LUUNA_MISSIONS_REFERENCE and LUUNA_MISSIONS_GO"]
fn parity_unblock() {
    crate::support::replay_group("parity_unblock");
}

#[test]
#[ignore = "requires LUUNA_MISSIONS_REFERENCE and LUUNA_MISSIONS_GO"]
fn parity_completions() {
    crate::support::replay_group("parity_completions");
}

// ---------------------------------------------------------------------------
// Stores and the shared comparison
// ---------------------------------------------------------------------------

/// `issues.jsonl` inside a project.
const ISSUES: &str = ".luuna/missions/issues.jsonl";
/// `plans.jsonl` inside a project.
const PLANS: &str = ".luuna/missions/plans.jsonl";

/// The other four files of the store every fixture starts from, as the Go
/// tool's `init` writes them.
const STORE_FILES: [(&str, &[u8]); 4] = [
    (
        ".gitattributes",
        b".luuna/missions/issues.jsonl merge=union\n.luuna/missions/templates.jsonl merge=union\n.luuna/missions/plans.jsonl merge=union\n",
    ),
    (".luuna/missions/.gitignore", b"*.lock\n"),
    (
        ".luuna/missions/config.yaml",
        b"project: \"project\"\nversion: \"1\"\nmax_plan_depth: 3\n",
    ),
    (".luuna/missions/templates.jsonl", b""),
];

/// The project a case starts from.
#[derive(Debug, Clone, Copy)]
enum Store {
    /// A project without `.luuna/missions`.
    Missing,
    /// The six files of the fixtures' store, with these `issues.jsonl` and
    /// `plans.jsonl` lines (each written with a final newline).
    Standard {
        issues: &'static [&'static [u8]],
        plans: &'static [&'static [u8]],
    },
}

/// A store with these tickets and an empty `plans.jsonl`.
const fn tickets(issues: &'static [&'static [u8]]) -> Store {
    Store::Standard { issues, plans: &[] }
}

/// The lines, each followed by a newline; no lines give an empty file.
fn jsonl(lines: &[&[u8]]) -> Vec<u8> {
    let mut out = Vec::new();
    for line in lines {
        out.extend_from_slice(line);
        out.push(b'\n');
    }
    out
}

/// A fresh project holding `store`.
fn project(store: Store) -> TempProject {
    let project = TempProject::new();
    if let Store::Standard { issues, plans } = store {
        project.write_files(
            STORE_FILES
                .iter()
                .map(|(name, content)| (*name, content.to_vec()))
                .chain([(ISSUES, jsonl(issues)), (PLANS, jsonl(plans))]),
        );
    }
    project
}

/// Replaces the project root by `<PROJECT>`; ISO timestamps are masked by
/// every [`Normaliser`].
fn normaliser(project: &TempProject) -> Normaliser {
    Normaliser::new().replace(project.root().to_string_lossy().as_bytes(), "<PROJECT>")
}

/// What the Go side of a case must show, beyond being compared with Rust.
enum GoExpect {
    /// Nothing is stated.
    Nothing,
    /// Go exits with this code.
    Exit(i32),
    /// Go exits 0 and prints exactly this help snapshot of
    /// `<reference-clone>/internal/cli/help/` on stdout.
    Snapshot(&'static str),
    /// Go's run passes this check, which returns what it found wrong.
    Check(fn(&Run) -> Vec<String>),
}

/// One case of a comparison test.
struct Case {
    name: String,
    args: Vec<String>,
    store: Store,
    env: &'static [(&'static str, &'static str)],
    go: GoExpect,
    /// The first stdout line is Go's debug record: it is compared after
    /// parsing, with `time` and `pid` ignored and every other key and value
    /// and the key order equal; the rest of stdout is compared byte for byte.
    debug_record: bool,
}

/// A case on `store` with nothing stated about Go.
fn case(store_name: &str, store: Store, values: &[&str]) -> Case {
    Case {
        name: format!("[{store_name}] {}", values.join(" ")),
        args: args(values),
        store,
        env: &[],
        go: GoExpect::Nothing,
        debug_record: false,
    }
}

/// Bytes for a report, every byte that is not printable ASCII escaped.
fn shown(bytes: &[u8]) -> String {
    bytes.escape_ascii().to_string()
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    needle.is_empty()
        || haystack
            .windows(needle.len())
            .any(|window| window == needle)
}

/// The top-level keys of a JSON object in the order they are written.
struct KeyOrder(Vec<String>);

impl<'de> Deserialize<'de> for KeyOrder {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Keys;
        impl<'de> Visitor<'de> for Keys {
            type Value = KeyOrder;
            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a JSON object")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<KeyOrder, A::Error> {
                let mut keys = Vec::new();
                while let Some((key, IgnoredAny)) = map.next_entry::<String, IgnoredAny>()? {
                    keys.push(key);
                }
                Ok(KeyOrder(keys))
            }
        }
        deserializer.deserialize_map(Keys)
    }
}

/// Splits stdout into its first line (without the newline) and the rest.
fn first_line(stdout: &[u8]) -> (&[u8], &[u8]) {
    match stdout.iter().position(|byte| *byte == b'\n') {
        Some(at) => (&stdout[..at], &stdout[at + 1..]),
        None => (stdout, &[]),
    }
}

/// Compares two debug records: both parse as JSON objects, hold the same
/// keys in the same order, and are equal once `time` and `pid` are removed.
fn debug_record_differences(go: &[u8], rust: &[u8]) -> Vec<String> {
    let order = |line: &[u8]| {
        serde_json::from_slice::<KeyOrder>(line)
            .ok()
            .map(|keys| keys.0)
    };
    let value = |line: &[u8]| match parse_json(line) {
        Some(Value::Object(mut map)) => {
            map.remove("time");
            map.remove("pid");
            Some(Value::Object(map))
        }
        _ => None,
    };
    let mut differences = Vec::new();
    match (order(go), order(rust)) {
        (Some(a), Some(b)) if a == b => {}
        (Some(a), Some(b)) => differences.push(format!("debug record keys {b:?}, want {a:?}")),
        (None, _) => differences.push(format!(
            "go: the first stdout line is not a JSON object: {}",
            shown(go)
        )),
        (_, None) => differences.push(format!(
            "the first stdout line is not a JSON object: {}",
            shown(rust)
        )),
    }
    if let (Some(a), Some(b)) = (value(go), value(rust)) {
        if a != b {
            differences.push(format!("debug record {b}, want {a}"));
        }
    }
    differences
}

/// What Go's run shows against the case's stated expectation.
fn go_expectation_differences(expect: &GoExpect, run: &Run, clone: &Reference) -> Vec<String> {
    match expect {
        GoExpect::Nothing => Vec::new(),
        GoExpect::Exit(code) if run.exit == *code => Vec::new(),
        GoExpect::Exit(code) => vec![format!(
            "exit {}, want {code}: {}",
            run.exit,
            excerpt(&run.stderr)
        )],
        GoExpect::Snapshot(file) => {
            let path = clone.path().join("internal/cli/help").join(file);
            let mut found = Vec::new();
            match fs::read(&path) {
                Ok(snapshot) => found.extend(exact_difference("stdout", &snapshot, &run.stdout)),
                Err(error) => found.push(format!("read {}: {error}", path.display())),
            }
            if run.exit != 0 {
                found.push(format!("exit {}, want 0", run.exit));
            }
            found
        }
        GoExpect::Check(check) => check(run),
    }
}

/// Runs `case` through Go and then through Rust, each in its own project,
/// and records what differs.
fn compare_case(clone: &Reference, go: &GoTool, case: &Case, report: &mut Report) {
    let mut env = child_env(None);
    for (key, value) in case.env {
        env.insert((*key).to_owned(), (*value).to_owned());
    }
    let go_project = project(case.store);
    let go_run = run_go(go.binary(), go_project.root(), &case.args, b"", &env);
    let rust_project = project(case.store);
    let rust_run = run_rust(rust_project.root(), &case.args, b"", &env);

    let mut detail = serde_json::Map::new();
    detail.insert("args".to_owned(), Value::from(case.args.clone()));
    let go_run = match go_run {
        Ok(run) => run,
        Err(error) => {
            report.case(&case.name, detail, vec![format!("go: {error}")]);
            return;
        }
    };
    detail.insert("go_exit".to_owned(), Value::from(go_run.exit));
    detail.insert("go_stdout".to_owned(), Value::from(excerpt(&go_run.stdout)));
    detail.insert("go_stderr".to_owned(), Value::from(excerpt(&go_run.stderr)));
    let mut differences: Vec<String> = go_expectation_differences(&case.go, &go_run, clone)
        .into_iter()
        .map(|found| format!("go: {found}"))
        .collect();
    match rust_run {
        Ok(rust_run) => {
            detail.insert("rust_exit".to_owned(), Value::from(rust_run.exit));
            let go_norm = normaliser(&go_project);
            let rust_norm = normaliser(&rust_project);
            let mut found = Vec::new();
            if case.debug_record {
                let (go_record, go_rest) = first_line(&go_run.stdout);
                let (rust_record, rust_rest) = first_line(&rust_run.stdout);
                found.extend(debug_record_differences(go_record, rust_record));
                let go_rest = Run {
                    stdout: go_rest.to_vec(),
                    ..go_run.clone()
                };
                let rust_rest = Run {
                    stdout: rust_rest.to_vec(),
                    ..rust_run.clone()
                };
                found.extend(run_differences(&go_rest, &go_norm, &rust_rest, &rust_norm));
            } else {
                found.extend(run_differences(&go_run, &go_norm, &rust_run, &rust_norm));
            }
            found.extend(tree_differences(
                &go_project.snapshot(),
                &go_norm,
                &rust_project.snapshot(),
                &rust_norm,
            ));
            differences.extend(found.into_iter().map(|found| {
                if found.starts_with("go: ") {
                    found
                } else {
                    format!("rust: {found}")
                }
            }));
        }
        Err(error) => differences.push(format!("rust: {error}")),
    }
    report.case(&case.name, detail, differences);
}

/// Runs every case of a comparison test under [`guarded`].
fn compare_all(test: &str, cases: impl FnOnce(&Reference) -> Vec<Case>) {
    let clone = reference::reference_from_env();
    guarded(test, &clone, |report| {
        let go = reference::go_tool(&clone);
        report.record_go(go);
        let cases = cases(&clone);
        report.set("case_count", Value::from(cases.len()));
        for case in &cases {
            compare_case(&clone, go, case, report);
        }
    });
}

// ---------------------------------------------------------------------------
// help_and_version_shapes_match_go
// ---------------------------------------------------------------------------

/// The store of the fixtures, without tickets.
const EMPTY_STORE: Store = tickets(&[]);

/// The 57 help snapshots of `<reference-clone>/internal/cli/help/` at the
/// reference commit, by file name.
const HELP_SNAPSHOTS: [&str; 57] = [
    "block.txt",
    "blocked.txt",
    "close.txt",
    "completions.txt",
    "config.txt",
    "config__schema.txt",
    "config__set.txt",
    "config__show.txt",
    "config__unset.txt",
    "create.txt",
    "dep.txt",
    "dep__add.txt",
    "dep__list.txt",
    "dep__remove.txt",
    "doctor.txt",
    "guard.txt",
    "init.txt",
    "label.txt",
    "label__add.txt",
    "label__list-all.txt",
    "label__list.txt",
    "label__remove.txt",
    "list.txt",
    "migrate-from-beads.txt",
    "onboard.txt",
    "plan.txt",
    "plan__adopt.txt",
    "plan__create.txt",
    "plan__edit.txt",
    "plan__list.txt",
    "plan__outcome.txt",
    "plan__prompt.txt",
    "plan__release.txt",
    "plan__reorder.txt",
    "plan__review.txt",
    "plan__show.txt",
    "plan__submit.txt",
    "plan__templates.txt",
    "plan__validate.txt",
    "prime.txt",
    "ready.txt",
    "search.txt",
    "setup.txt",
    "show.txt",
    "stats.txt",
    "sync.txt",
    "tpl.txt",
    "tpl__create.txt",
    "tpl__list.txt",
    "tpl__pour.txt",
    "tpl__show.txt",
    "tpl__status.txt",
    "tpl__step.txt",
    "tpl__step__add.txt",
    "unblock.txt",
    "update.txt",
    "upgrade.txt",
];

/// Help forms that are not a snapshot's own argument list, and root help.
const OTHER_HELP: [&[&str]; 8] = [
    &["tpl", "step", "add", "missing", "--help"],
    &["dep", "nope", "-h"],
    &["plan", "--help"],
    &["setup", "claude", "--help"],
    &["-h"],
    &["--timing", "--help"],
    &["--format", "x", "--help"],
    &["-q"],
];

/// Version forms.
const VERSION_FORMS: [&[&str]; 6] = [
    &["-v"],
    &["init", "-v"],
    &["list", "--json", "--version"],
    &["nope", "--json", "-v"],
    &["create", "--title", "-v"],
    &["--version", "--json", "--quiet"],
];

/// Unknown commands; each runs without and with `--json`.
const UNKNOWN_COMMANDS: [&str; 6] = ["nope", "inti", "help", "--", "créer", "zzzzzz"];

#[test]
#[ignore = "requires LUUNA_MISSIONS_REFERENCE and LUUNA_MISSIONS_GO"]
fn help_and_version_shapes_match_go() {
    compare_all("help_and_version_shapes_match_go", |clone| {
        let dir = clone.path().join("internal/cli/help");
        let mut listed: Vec<String> = fs::read_dir(&dir)
            .unwrap_or_else(|error| panic!("read {}: {error}", dir.display()))
            .map(|entry| entry.expect("directory entry").file_name())
            .map(|name| name.to_string_lossy().into_owned())
            .collect();
        listed.sort();
        let mut expected: Vec<String> = HELP_SNAPSHOTS
            .iter()
            .map(|name| (*name).to_owned())
            .collect();
        expected.sort();
        assert_eq!(
            listed, expected,
            "the reference's help snapshots are not the 57 this test lists"
        );

        let mut cases = Vec::new();
        for (store_name, store) in [("no store", Store::Missing), ("store", EMPTY_STORE)] {
            for file in HELP_SNAPSHOTS {
                let stem = file.strip_suffix(".txt").expect("a .txt file");
                let mut values: Vec<&str> = stem.split("__").collect();
                values.push("--help");
                let mut snapshot = case(store_name, store, &values);
                snapshot.go = GoExpect::Snapshot(file);
                cases.push(snapshot);
            }
            for values in OTHER_HELP.iter().chain(VERSION_FORMS.iter()) {
                cases.push(case(store_name, store, values));
            }
            for name in UNKNOWN_COMMANDS {
                for values in [&[name][..], &[name, "--json"]] {
                    let mut unknown = case(store_name, store, values);
                    unknown.go = GoExpect::Exit(1);
                    cases.push(unknown);
                }
            }
        }
        let mut debug = case("no store", Store::Missing, &["show", "missing", "--json"]);
        debug.name = format!("{} with LUUNA_MISSIONS_DEBUG=1", debug.name);
        debug.env = &[("LUUNA_MISSIONS_DEBUG", "1")];
        debug.go = GoExpect::Exit(1);
        debug.debug_record = true;
        cases.push(debug);
        cases
    });
}

// ---------------------------------------------------------------------------
// workflow_shapes_match_go
// ---------------------------------------------------------------------------

/// Tickets of every status, a blocked ticket, a dangling blocker, a ticket
/// blocked only by a closed one, an in-progress blocked ticket, duplicate
/// labels, priorities outside 0-4, a unicode title and every kind of
/// `scheduledFor`. No lossy input, no `plan_id`, and an empty `plans.jsonl`.
const WORKFLOW: Store = tickets(&[
    br#"{"id":"w-open","title":"Open task","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","labels":["api","core"],"blocks":["w-blocked","w-prog-blocked"]}"#,
    br#"{"id":"w-prog","title":"In progress feature","status":"in_progress","type":"feature","priority":1,"createdAt":"2026-01-02T00:00:00.000Z","updatedAt":"2026-01-09T00:00:00.000Z","labels":["api"],"assignee":"alice"}"#,
    br#"{"id":"w-closed","title":"Closed bug","status":"closed","type":"bug","priority":0,"createdAt":"2026-01-03T00:00:00.000Z","updatedAt":"2026-01-04T00:00:00.000Z","blocks":["w-after-closed"],"closedAt":"2026-01-04T00:00:00.000Z","closeReason":"fixed"}"#,
    br#"{"id":"w-blocked","title":"Blocked task","status":"open","type":"task","priority":3,"createdAt":"2026-01-04T00:00:00.000Z","updatedAt":"2026-01-04T00:00:00.000Z","blockedBy":["w-open"]}"#,
    br#"{"id":"w-dangling","title":"Dangling blocker","status":"open","type":"epic","priority":4,"createdAt":"2026-01-05T00:00:00.000Z","updatedAt":"2026-01-05T00:00:00.000Z","blockedBy":["w-missing"]}"#,
    br#"{"id":"w-dups","title":"Duplicate labels","status":"open","type":"task","priority":7,"createdAt":"2026-01-06T00:00:00.000Z","updatedAt":"2026-01-06T00:00:00.000Z","labels":["dup","dup","x"]}"#,
    "{\"id\":\"w-unicode\",\"title\":\"Ünïcode 标题 ✓ <&>\",\"status\":\"open\",\"type\":\"bug\",\"priority\":-1,\"createdAt\":\"2026-01-07T00:00:00.000Z\",\"updatedAt\":\"2026-01-07T00:00:00.000Z\",\"labels\":[\"ünï\"]}".as_bytes(),
    br#"{"id":"w-after-closed","title":"Blocked by a closed ticket","status":"open","type":"task","priority":2,"createdAt":"2026-01-08T00:00:00.000Z","updatedAt":"2026-01-08T00:00:00.000Z","blockedBy":["w-closed"]}"#,
    br#"{"id":"w-prog-blocked","title":"In progress and blocked","status":"in_progress","type":"task","priority":2,"createdAt":"2026-01-09T00:00:00.000Z","updatedAt":"2026-01-09T00:00:00.000Z","blockedBy":["w-open"],"assignee":"bob"}"#,
    br#"{"id":"w-plain","title":"Unlabelled feature","status":"open","type":"feature","priority":1,"createdAt":"2026-01-10T00:00:00.000Z","updatedAt":"2026-01-10T00:00:00.000Z"}"#,
    br#"{"id":"w-sched-past","title":"Scheduled in the past","status":"open","type":"task","priority":2,"createdAt":"2026-01-11T00:00:00.000Z","updatedAt":"2026-01-11T00:00:00.000Z","extensions":{"scheduledFor":"2020-01-01T00:00:00Z"}}"#,
    br#"{"id":"w-sched-future","title":"Scheduled far ahead","status":"open","type":"task","priority":2,"createdAt":"2026-01-12T00:00:00.000Z","updatedAt":"2026-01-12T00:00:00.000Z","extensions":{"scheduledFor":"2999-01-01T00:00:00Z"}}"#,
    br#"{"id":"w-sched-offset","title":"Scheduled with an offset","status":"open","type":"task","priority":2,"createdAt":"2026-01-13T00:00:00.000Z","updatedAt":"2026-01-13T00:00:00.000Z","extensions":{"scheduledFor":"2999-01-01T00:00:00+02:00"}}"#,
    br#"{"id":"w-sched-past-offset","title":"Scheduled in the past with an offset","status":"open","type":"task","priority":2,"createdAt":"2026-01-14T00:00:00.000Z","updatedAt":"2026-01-14T00:00:00.000Z","extensions":{"scheduledFor":"2020-01-01T00:00:00-05:00"}}"#,
    br#"{"id":"w-sched-fraction","title":"Scheduled with a fraction","status":"open","type":"task","priority":2,"createdAt":"2026-01-15T00:00:00.000Z","updatedAt":"2026-01-15T00:00:00.000Z","extensions":{"scheduledFor":"2999-01-01T00:00:00.123456789Z"}}"#,
    br#"{"id":"w-sched-date","title":"Scheduled on a far date","status":"open","type":"task","priority":2,"createdAt":"2026-01-16T00:00:00.000Z","updatedAt":"2026-01-16T00:00:00.000Z","extensions":{"scheduledFor":"2999-01-01"}}"#,
    br#"{"id":"w-sched-date-past","title":"Scheduled on a past date","status":"open","type":"task","priority":2,"createdAt":"2026-01-17T00:00:00.000Z","updatedAt":"2026-01-17T00:00:00.000Z","extensions":{"scheduledFor":"2020-01-01"}}"#,
    br#"{"id":"w-sched-invalid","title":"Scheduled for an invalid string","status":"open","type":"task","priority":2,"createdAt":"2026-01-18T00:00:00.000Z","updatedAt":"2026-01-18T00:00:00.000Z","extensions":{"scheduledFor":"not a date"}}"#,
    br#"{"id":"w-sched-spaced","title":"Scheduled with spaces","status":"open","type":"task","priority":2,"createdAt":"2026-01-19T00:00:00.000Z","updatedAt":"2026-01-19T00:00:00.000Z","extensions":{"scheduledFor":" 2999-01-01T00:00:00Z "}}"#,
    br#"{"id":"w-queued","title":"Queued","status":"open","type":"task","priority":2,"createdAt":"2026-01-20T00:00:00.000Z","updatedAt":"2026-01-20T00:00:00.000Z","extensions":{"queued":true}}"#,
]);

/// Read-only shapes of ready, blocked and stats.
const QUERY_SHAPES: &[&[&str]] = &[
    &["ready"],
    &["ready", "--format", "markdown"],
    &["ready", "--format", "plain"],
    &["ready", "--format", "compact"],
    &["ready", "--format", "ids"],
    &["ready", "--format", "json"],
    &["ready", "--json"],
    &["ready", "--format", "nope"],
    &["ready", "--json", "--format", "plain"],
    &["ready", "--format", "plain", "--json"],
    &["-q", "ready"],
    &["-q", "ready", "--json"],
    &["ready", "-q"],
    &["ready", "--type", "task", "--json"],
    &["ready", "--type", "bug"],
    &["ready", "--type=feature", "--format=compact"],
    &["ready", "--type", "nope"],
    &["ready", "--assignee", "alice", "--json"],
    &["ready", "--label", "api", "--json"],
    &["ready", "--label", "API, core"],
    &["ready", "--label", "", "--json"],
    &["ready", "--label-any", "core,x", "--json"],
    &["ready", "--label-any", " , ", "--json"],
    &["ready", "--unlabeled", "--json"],
    &["ready", "--priority", "2", "--json"],
    &["ready", "--priority", "P1,3"],
    &["ready", "--priority", ",4,"],
    &["ready", "--priority", "9"],
    &["ready", "--priority", "p01"],
    &["ready", "--priority-max", "2", "--json"],
    &["ready", "--priority-max", "P4", "--format", "ids"],
    &["ready", "--priority-max", "x"],
    &["ready", "--sort", "priority", "--format", "ids"],
    &["ready", "--sort", "created", "--format", "ids"],
    &["ready", "--sort", "updated", "--format", "ids"],
    &["ready", "--sort", "id", "--format", "ids"],
    &["ready", "--sort", "nope"],
    &["ready", "--sort", "nope", "--json"],
    &["ready", "--limit", "0"],
    &["ready", "--limit", "0", "--json"],
    &["ready", "--limit", "1"],
    &["ready", "--limit", "1", "--json"],
    &["ready", "--limit", "-1"],
    &["ready", "--limit", "abc"],
    &["ready", "--limit="],
    &["ready", "extra"],
    &["ready", "--nope"],
    &["ready", "--respect-schedule"],
    &["ready", "--respect-schedule", "--json"],
    &["ready", "--respect-schedule", "--format", "ids"],
    &["blocked"],
    &["blocked", "--format", "markdown"],
    &["blocked", "--format", "plain"],
    &["blocked", "--format", "compact"],
    &["blocked", "--format=compact"],
    &["blocked", "--format", "ids"],
    &["blocked", "--format", "json"],
    &["blocked", "--json"],
    &["blocked", "--format", "nope"],
    &["-q", "blocked"],
    &["blocked", "-q"],
    &["blocked", "--limit", "1"],
    &["blocked", "--limit", "1", "--json"],
    &["blocked", "extra"],
    &["stats"],
    &["stats", "--format", "markdown"],
    &["stats", "--format", "plain"],
    &["stats", "--format", "compact"],
    &["stats", "--format", "ids"],
    &["stats", "--format", "json"],
    &["stats", "--json"],
    &["stats", "--format", "nope"],
    &["-q", "stats"],
    &["stats", "-q"],
    &["stats", "extra"],
    &["stats", "--limit", "1"],
];

/// Writing shapes of close, dep, label and unblock, and completions.
const WRITE_SHAPES: &[&[&str]] = &[
    &["close", "w-open", "--json"],
    &["close", "w-open"],
    &["close", "w-open", "--reason", "done"],
    &["close", "w-open", "--reason", "done", "--json"],
    &["close", "w-open", "--reason", "", "--json"],
    &["close", "w-open", "w-prog", "--json"],
    &["close", "w-open", "w-prog"],
    &["close", "w-open", "w-open", "--json"],
    &["close", "w-missing", "--json"],
    &["close", "w-missing"],
    &["close", "w-open", "w-missing", "--json"],
    &["close", "w-closed", "--json"],
    &["close", "w-closed", "--reason", "again"],
    &["-q", "close", "w-open"],
    &["-q", "close", "w-open", "--json"],
    &["close", "--json"],
    &["close"],
    &["close", "w-unicode", "--reason", "Ünïcode <&> \"quoted\""],
    &["close", "w-open", "--nope"],
    &["dep", "add", "w-plain", "w-open", "--json"],
    &["dep", "add", "w-plain", "w-open"],
    &["dep", "add", "w-blocked", "w-open", "--json"],
    &["dep", "add", "w-open", "w-open", "--json"],
    &["dep", "add", "w-open", "w-open"],
    &["dep", "add", "w-plain", "w-missing", "--json"],
    &["dep", "add", "w-missing", "w-open", "--json"],
    &["dep", "add", "w-plain", "--json"],
    &["dep", "add"],
    &["dep", "remove", "w-blocked", "w-open", "--json"],
    &["dep", "remove", "w-blocked", "w-open"],
    &["dep", "remove", "w-plain", "w-open", "--json"],
    &["dep", "remove", "w-open", "w-open", "--json"],
    &["dep", "remove", "w-blocked", "--json"],
    &["dep", "list", "w-blocked", "--json"],
    &["dep", "list", "w-blocked"],
    &["dep", "list", "w-open"],
    &["dep", "list", "w-open", "--json"],
    &["dep", "list", "w-dangling"],
    &["dep", "list", "w-plain"],
    &["dep", "list", "w-missing", "--json"],
    &["dep", "list"],
    &["dep", "nope", "w-open", "--json"],
    &["dep", "nope"],
    &["dep", "--json"],
    &["dep"],
    &["label", "add", "w-open", "backend", "--json"],
    &["label", "add", "w-open", "backend"],
    &["label", "add", "w-open", " API ", "--json"],
    &["label", "add", "w-open", "", "--json"],
    &["label", "add", "w-open", " ", ""],
    &["label", "add", "w-open", "a,b", "--json"],
    &["label", "add", "w-open", "x", "Y", "x", "--json"],
    &["label", "add", "w-dups", "new", "--json"],
    &["label", "add", "w-missing", "x", "--json"],
    &["label", "add", "w-open", "--json"],
    &["-q", "label", "add", "w-open", "backend"],
    &["label", "remove", "w-open", "api", "--json"],
    &["label", "remove", "w-open", "api"],
    &["label", "remove", "w-open", " CORE ", "api", "--json"],
    &["label", "remove", "w-open", "absent", "--json"],
    &["label", "remove", "w-open", "", "--json"],
    &["label", "remove", "w-dups", "dup", "--json"],
    &["label", "remove", "w-plain", "x", "--json"],
    &["label", "list", "w-open", "--json"],
    &["label", "list", "w-open"],
    &["label", "list", "w-plain"],
    &["label", "list", "w-missing", "--json"],
    &["label", "list"],
    &["label", "list-all", "--json"],
    &["label", "list-all"],
    &["label", "nope", "--json"],
    &["label"],
    &["unblock", "w-blocked", "--all", "--json"],
    &["unblock", "w-blocked", "--all"],
    &["unblock", "w-after-closed", "--all", "--json"],
    &["unblock", "w-after-closed", "--all"],
    &["unblock", "w-blocked", "--from", "w-open", "--json"],
    &["unblock", "w-blocked", "--from", "w-open"],
    &["unblock", "w-blocked", "--from", "w-prog", "--json"],
    &[
        "unblock",
        "w-blocked",
        "--all",
        "--from",
        "w-prog",
        "--json",
    ],
    &["unblock", "w-blocked", "--from=", "--json"],
    &["unblock", "--all", "--json"],
    &["unblock", "w-blocked", "--json"],
    &["unblock", "w-missing", "--all", "--json"],
    &["completions"],
    &["completions", "bash", "zsh"],
    &["completions", "--json"],
    &["completions", "nope"],
];

/// Shapes run in a project without a store.
const OUTSIDE_SHAPES: &[&[&str]] = &[
    &["ready", "--json"],
    &["ready", "--sort", "nope"],
    &["ready", "--priority", "9", "--json"],
    &["blocked"],
    &["stats", "--json"],
    &["close", "w-open", "--json"],
    &["dep", "nope", "--json"],
    &["dep", "list", "w-open"],
    &["label", "nope"],
    &["label", "list-all", "--json"],
    &["unblock", "w-open", "--all", "--json"],
    &["unblock", "w-open"],
    &["completions", "bash"],
    &["completions", "zsh"],
    &["completions", "fish"],
    &["completions", "nope", "--json"],
];

/// Shapes run on a store without tickets.
const EMPTY_SHAPES: &[&[&str]] = &[
    &["ready"],
    &["ready", "--json"],
    &["blocked"],
    &["blocked", "--format", "compact"],
    &["blocked", "--json"],
    &["stats"],
    &["stats", "--format", "compact"],
    &["stats", "--json"],
    &["label", "list-all"],
    &["label", "list-all", "--json"],
];

#[test]
#[ignore = "requires LUUNA_MISSIONS_REFERENCE and LUUNA_MISSIONS_GO"]
fn workflow_shapes_match_go() {
    compare_all("workflow_shapes_match_go", |_| {
        let mut cases = Vec::new();
        for values in QUERY_SHAPES.iter().chain(WRITE_SHAPES) {
            cases.push(case("workflow", WORKFLOW, values));
        }
        for values in OUTSIDE_SHAPES {
            cases.push(case("no store", Store::Missing, values));
        }
        for values in EMPTY_SHAPES {
            cases.push(case("empty", EMPTY_STORE, values));
        }
        cases
    });
}

// ---------------------------------------------------------------------------
// plan_context_in_ready_and_blocked_matches_go
// ---------------------------------------------------------------------------

/// Plans in Go's plan record shape: draft, approved, active and done plans;
/// two plans seeding t-8 whose updatedAt differ (the newer one, a draft,
/// first in the file); two seeding t-10 whose updatedAt tie (the approved
/// one first); the unreadable line `not json`; the wrong-typed line pl-bad
/// (seeding t-1 and named by t-2's plan_id); and a cut-off object seeding
/// t-9, a second line Go cannot read whose skip shows in ready.
const PLAN_LINES: &[&[u8]] = &[
    br#"{"id":"pl-draft","seed":"t-6","template":"t","name":"draft plan","status":"draft","revision":1,"sections":{"steps":[]},"children":["t-3","t-16"],"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
    br#"{"id":"pl-appr","seed":"t-7","template":"t","name":"approved plan","status":"approved","revision":2,"sections":{"steps":[]},"children":["t-13"],"createdAt":"2026-01-02T00:00:00.000Z","updatedAt":"2026-01-02T00:00:00.000Z"}"#,
    br#"{"id":"pl-act","seed":"t-20","template":"t","name":"active plan","status":"active","revision":1,"sections":{"steps":[]},"children":[],"createdAt":"2026-01-03T00:00:00.000Z","updatedAt":"2026-01-03T00:00:00.000Z"}"#,
    br#"{"id":"pl-done","seed":"t-21","template":"t","status":"done","revision":3,"sections":{},"children":["t-12"],"outcome":"success","createdAt":"2026-01-04T00:00:00.000Z","updatedAt":"2026-01-04T00:00:00.000Z"}"#,
    br#"{"id":"pl-new8","seed":"t-8","template":"t","name":"newer draft","status":"draft","revision":1,"sections":{},"children":[],"createdAt":"2026-02-01T00:00:00.000Z","updatedAt":"2026-02-01T00:00:00.000Z"}"#,
    br#"{"id":"pl-old8","seed":"t-8","template":"t","name":"older approved","status":"approved","revision":1,"sections":{},"children":[],"createdAt":"2026-01-15T00:00:00.000Z","updatedAt":"2026-01-15T00:00:00.000Z"}"#,
    br#"{"id":"pl-tie-a","seed":"t-10","template":"t","name":"tie approved","status":"approved","revision":1,"sections":{},"children":["t-10"],"createdAt":"2026-01-10T00:00:00.000Z","updatedAt":"2026-01-10T00:00:00.000Z"}"#,
    br#"{"id":"pl-tie-b","seed":"t-10","template":"t","name":"tie draft","status":"draft","revision":1,"sections":{},"children":[],"createdAt":"2026-01-10T00:00:00.000Z","updatedAt":"2026-01-10T00:00:00.000Z"}"#,
    b"not json",
    br#"{"id":"pl-bad","seed":"t-1","status":5,"name":["x"],"children":"t-3","updatedAt":7}"#,
    br#"{"id":"pl-cut","seed":"t-9","template":"t","status":"approved","children":[]"#,
];

/// Tickets with plan context; each is open unless it says otherwise.
const PLAN_TICKETS: &[&[u8]] = &[
    br#"{"id":"t-1","title":"Seeded by the wrong-typed plan","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","requires_plan":true}"#,
    br#"{"id":"t-2","title":"Names the wrong-typed plan","status":"open","type":"task","priority":2,"createdAt":"2026-01-02T00:00:00.000Z","updatedAt":"2026-01-02T00:00:00.000Z","plan_id":"pl-bad"}"#,
    br#"{"id":"t-3","title":"Draft plan child, blocked","status":"open","type":"task","priority":2,"createdAt":"2026-01-03T00:00:00.000Z","updatedAt":"2026-01-03T00:00:00.000Z","blockedBy":["t-4"],"plan_id":"pl-draft","plan_step_index":0}"#,
    br#"{"id":"t-4","title":"Plain blocker","status":"open","type":"task","priority":1,"createdAt":"2026-01-04T00:00:00.000Z","updatedAt":"2026-01-04T00:00:00.000Z","blocks":["t-3","t-15","t-16"]}"#,
    br#"{"id":"t-5","title":"Requires a plan, has none","status":"open","type":"feature","priority":2,"createdAt":"2026-01-05T00:00:00.000Z","updatedAt":"2026-01-05T00:00:00.000Z","requires_plan":true}"#,
    br#"{"id":"t-6","title":"Seeded by a draft","status":"open","type":"feature","priority":2,"createdAt":"2026-01-06T00:00:00.000Z","updatedAt":"2026-01-06T00:00:00.000Z","requires_plan":true}"#,
    br#"{"id":"t-7","title":"Seeded by an approved plan","status":"open","type":"feature","priority":2,"createdAt":"2026-01-07T00:00:00.000Z","updatedAt":"2026-01-07T00:00:00.000Z","requires_plan":true}"#,
    br#"{"id":"t-8","title":"Seeded twice, newer draft","status":"open","type":"feature","priority":2,"createdAt":"2026-01-08T00:00:00.000Z","updatedAt":"2026-01-08T00:00:00.000Z","requires_plan":true}"#,
    br#"{"id":"t-9","title":"Seeded only by an unreadable line","status":"open","type":"feature","priority":2,"createdAt":"2026-01-09T00:00:00.000Z","updatedAt":"2026-01-09T00:00:00.000Z","requires_plan":true}"#,
    br#"{"id":"t-10","title":"Seeded twice, tie","status":"open","type":"feature","priority":2,"createdAt":"2026-01-10T00:00:00.000Z","updatedAt":"2026-01-10T00:00:00.000Z","requires_plan":true}"#,
    br#"{"id":"t-11","title":"Active plan, extension plan_status","status":"open","type":"task","priority":3,"createdAt":"2026-01-11T00:00:00.000Z","updatedAt":"2026-01-11T00:00:00.000Z","plan_id":"pl-act","extensions":{"plan_status":"stale"}}"#,
    br#"{"id":"t-12","title":"Done plan, stored plan context","status":"open","type":"task","priority":3,"createdAt":"2026-01-12T00:00:00.000Z","updatedAt":"2026-01-12T00:00:00.000Z","plan_id":"pl-done","zeta":1,"plan_status":"stale","plan_children":["old"],"alpha":"a"}"#,
    br#"{"id":"t-13","title":"Approved plan child","status":"open","type":"task","priority":3,"createdAt":"2026-01-13T00:00:00.000Z","updatedAt":"2026-01-13T00:00:00.000Z","plan_id":"pl-appr"}"#,
    br#"{"id":"t-14","title":"Names a missing plan","status":"open","type":"task","priority":3,"createdAt":"2026-01-14T00:00:00.000Z","updatedAt":"2026-01-14T00:00:00.000Z","plan_id":"pl-missing"}"#,
    br#"{"id":"t-15","title":"Active plan child, blocked","status":"open","type":"task","priority":3,"createdAt":"2026-01-15T00:00:00.000Z","updatedAt":"2026-01-15T00:00:00.000Z","blockedBy":["t-4"],"plan_id":"pl-act"}"#,
    br#"{"id":"t-16","title":"Draft plan child in progress, blocked","status":"in_progress","type":"task","priority":3,"createdAt":"2026-01-16T00:00:00.000Z","updatedAt":"2026-01-16T00:00:00.000Z","blockedBy":["t-4"],"plan_id":"pl-draft"}"#,
    br#"{"id":"t-17","title":"Closed with a plan","status":"closed","type":"task","priority":3,"createdAt":"2026-01-17T00:00:00.000Z","updatedAt":"2026-01-17T00:00:00.000Z","plan_id":"pl-appr","closedAt":"2026-01-17T00:00:00.000Z"}"#,
];

const PLAN_STORE: Store = Store::Standard {
    issues: PLAN_TICKETS,
    plans: PLAN_LINES,
};

/// Go's `ready --json` on [`PLAN_STORE`] keeps the wrong-typed line pl-bad
/// and skips the lines it cannot read (plan.json plans_jsonl_settlement):
/// the run exits 0, t-1 (seeded by pl-bad) is ready, t-2 (plan_id pl-bad)
/// carries `"plan_children": []` and no plan_status, and t-9 (seeded only
/// by the cut-off line) is not ready. Also the seeding rules the store was
/// written for: t-3 (SD205), t-7 and t-10 are ready, t-5, t-6 and t-8 not.
fn go_keeps_pl_bad_and_skips_unreadable_lines(run: &Run) -> Vec<String> {
    let mut found = Vec::new();
    if run.exit != 0 {
        found.push(format!(
            "ready --json exited {}, want 0: {}",
            run.exit,
            excerpt(&run.stderr)
        ));
    }
    let Some(issues) = parse_json(&run.stdout)
        .and_then(|value| value.get("issues").and_then(Value::as_array).cloned())
    else {
        found.push(format!(
            "ready --json printed no issues array: {}",
            excerpt(&run.stdout)
        ));
        return found;
    };
    let by_id = |id: &str| {
        issues
            .iter()
            .find(|issue| issue.get("id").and_then(Value::as_str) == Some(id))
    };
    for id in ["t-1", "t-3", "t-7", "t-10"] {
        if by_id(id).is_none() {
            found.push(format!("{id} is not ready"));
        }
    }
    for id in ["t-5", "t-6", "t-8", "t-9"] {
        if by_id(id).is_some() {
            found.push(format!("{id} is ready"));
        }
    }
    match by_id("t-2") {
        None => found.push("t-2 is not ready".to_owned()),
        Some(issue) => {
            if issue.get("plan_children") != Some(&Value::Array(Vec::new())) {
                found.push(format!(
                    "t-2 (plan_id pl-bad) has plan_children {:?}, want []",
                    issue.get("plan_children")
                ));
            }
            if let Some(status) = issue.get("plan_status") {
                found.push(format!("t-2 (plan_id pl-bad) has plan_status {status}"));
            }
        }
    }
    found
}

#[test]
#[ignore = "requires LUUNA_MISSIONS_REFERENCE and LUUNA_MISSIONS_GO"]
fn plan_context_in_ready_and_blocked_matches_go() {
    compare_all("plan_context_in_ready_and_blocked_matches_go", |_| {
        let mut cases = Vec::new();
        for values in [
            &["ready", "--json"][..],
            &["ready"],
            &["ready", "--format", "compact"],
            &["blocked", "--json"],
            &["blocked"],
            &["blocked", "--format", "compact"],
        ] {
            let mut planned = case("plans", PLAN_STORE, values);
            if values == ["ready", "--json"] {
                planned.go = GoExpect::Check(go_keeps_pl_bad_and_skips_unreadable_lines);
            }
            cases.push(planned);
        }
        cases
    });
}

// ---------------------------------------------------------------------------
// lossy_rewrites_are_refused_where_go_drops_data
// ---------------------------------------------------------------------------

/// What one A8 case shows.
enum Loss {
    /// Go exits 0 and its `issues.jsonl` after the run no longer holds
    /// `lost`; Rust exits 1 with `refusal` as Go prints a command error with
    /// `--json`, nothing on stderr, and the whole project unchanged, file
    /// modes included.
    Refused {
        lost: &'static [u8],
        refusal: &'static str,
    },
    /// Go exits `go_exit` and its `issues.jsonl` after the run still holds
    /// every `kept` sequence; Rust equals Go.
    Kept {
        go_exit: i32,
        kept: &'static [&'static [u8]],
    },
}

/// One A8 case: the command (with `--json`), the lines of `issues.jsonl`
/// (with an empty `plans.jsonl`), and what it shows.
struct LossCase {
    name: &'static str,
    args: &'static [&'static str],
    issues: &'static [&'static [u8]],
    expect: Loss,
}

/// Go's command error with the literal `--json` token (`App.commandError`,
/// internal/cli/app.go): an indented JSON object without HTML escaping,
/// then a newline.
fn command_error_json(command: &str, error: &str) -> Vec<u8> {
    let string = |text: &str| serde_json::to_string(text).expect("a JSON string");
    format!(
        "{{\n  \"success\": false,\n  \"command\": {},\n  \"error\": {}\n}}\n",
        string(command),
        string(error)
    )
    .into_bytes()
}

/// Every difference between two snapshots of one project, without any
/// normalisation.
fn unchanged_differences(
    before: &BTreeMap<String, Entry>,
    after: &BTreeMap<String, Entry>,
) -> Vec<String> {
    let mut found = Vec::new();
    for name in before.keys().filter(|name| !after.contains_key(*name)) {
        found.push(format!("{name} was removed"));
    }
    for name in after.keys().filter(|name| !before.contains_key(*name)) {
        found.push(format!("{name} was created"));
    }
    for (name, entry) in before {
        let Some(now) = after.get(name) else {
            continue;
        };
        if entry.mode != now.mode {
            found.push(format!("{name} mode {:o}, was {:o}", now.mode, entry.mode));
        }
        match (&entry.kind, &now.kind) {
            (EntryKind::File(a), EntryKind::File(b)) => {
                found.extend(exact_difference(name, a, b));
            }
            (a, b) if a != b => found.push(format!("{name} is {b:?}, was {a:?}")),
            _ => {}
        }
    }
    found
}

fn loss_case(go: &GoTool, case: &LossCase, report: &mut Report) {
    let store = tickets(case.issues);
    let values = args(case.args);
    let env = child_env(None);
    let mut detail = serde_json::Map::new();
    detail.insert("args".to_owned(), Value::from(values.clone()));
    detail.insert(
        "issues_before".to_owned(),
        Value::from(shown(&jsonl(case.issues))),
    );

    let go_project = project(store);
    let go_run = match run_go(go.binary(), go_project.root(), &values, b"", &env) {
        Ok(run) => run,
        Err(error) => {
            report.case(case.name, detail, vec![format!("go: {error}")]);
            return;
        }
    };
    let go_after = fs::read(go_project.root().join(ISSUES)).unwrap_or_default();
    detail.insert("go_exit".to_owned(), Value::from(go_run.exit));
    detail.insert("go_stdout".to_owned(), Value::from(shown(&go_run.stdout)));
    detail.insert("go_stderr".to_owned(), Value::from(shown(&go_run.stderr)));
    detail.insert("go_issues_after".to_owned(), Value::from(shown(&go_after)));

    let rust_project = project(store);
    let rust_before = rust_project.snapshot();
    let rust_run = run_rust(rust_project.root(), &values, b"", &env);

    let mut differences = Vec::new();
    match &case.expect {
        Loss::Refused { lost, refusal } => {
            if go_run.exit != 0 {
                differences.push(format!("go: exit {}, want 0", go_run.exit));
            }
            if contains(&go_after, lost) {
                differences.push(format!(
                    "go: issues.jsonl after the run still holds {}",
                    shown(lost)
                ));
            }
            match &rust_run {
                Ok(run) => {
                    let mut found = Vec::new();
                    if run.exit != 1 {
                        found.push(format!("exit {}, want 1", run.exit));
                    }
                    let want = command_error_json(case.args[0], refusal);
                    found.extend(exact_difference("stdout", &want, &run.stdout));
                    found.extend(exact_difference("stderr", b"", &run.stderr));
                    found.extend(unchanged_differences(
                        &rust_before,
                        &rust_project.snapshot(),
                    ));
                    differences.extend(found.into_iter().map(|found| format!("rust: {found}")));
                }
                Err(error) => differences.push(format!("rust: {error}")),
            }
        }
        Loss::Kept { go_exit, kept } => {
            if go_run.exit != *go_exit {
                differences.push(format!("go: exit {}, want {go_exit}", go_run.exit));
            }
            for bytes in *kept {
                if !contains(&go_after, bytes) {
                    differences.push(format!(
                        "go: issues.jsonl after the run lost {}",
                        shown(bytes)
                    ));
                }
            }
            match &rust_run {
                Ok(run) => {
                    let go_norm = normaliser(&go_project);
                    let rust_norm = normaliser(&rust_project);
                    let mut found = run_differences(&go_run, &go_norm, run, &rust_norm);
                    found.extend(tree_differences(
                        &go_project.snapshot(),
                        &go_norm,
                        &rust_project.snapshot(),
                        &rust_norm,
                    ));
                    differences.extend(found.into_iter().map(|found| format!("rust: {found}")));
                }
                Err(error) => differences.push(format!("rust: {error}")),
            }
        }
    }
    if let Ok(run) = &rust_run {
        detail.insert("rust_exit".to_owned(), Value::from(run.exit));
        detail.insert("rust_stdout".to_owned(), Value::from(shown(&run.stdout)));
        detail.insert("rust_stderr".to_owned(), Value::from(shown(&run.stderr)));
    }
    report.case(case.name, detail, differences);
}

#[test]
#[ignore = "requires LUUNA_MISSIONS_REFERENCE and LUUNA_MISSIONS_GO"]
fn lossy_rewrites_are_refused_where_go_drops_data() {
    let clone = reference::reference_from_env();
    guarded(
        "lossy_rewrites_are_refused_where_go_drops_data",
        &clone,
        |report| {
            let go = reference::go_tool(&clone);
            report.record_go(go);
            let count = |prefix: &str| {
                LOSS_CASES
                    .iter()
                    .filter(|case| case.name.starts_with(prefix))
                    .count()
            };
            let counts = (count("file "), count("field "), count("control "));
            report.set(
                "case_counts",
                serde_json::json!({"file": counts.0, "field": counts.1, "control": counts.2}),
            );
            assert_eq!(
                counts,
                (24, 47, 22),
                "file-level cases, field cases and field controls"
            );
            for case in LOSS_CASES {
                loss_case(go, case, report);
            }
        },
    );
}

/// The 24 file-level cases, the 47 field cases and the 22 field controls of
/// plan.json go_comparison_tests.lossy_rewrites_are_refused_where_go_drops_data,
/// on the stores of go_data_loss_cases.variant_stores: tickets p-t, p-b, p-c
/// and p-o on lines 1 to 4 (file-level cases: p-t and p-b, then the lossy
/// lines 3 and 4). Each refusal is go_data_loss_cases.refusal_text with its
/// line, ticket and field written in.
const LOSS_CASES: &[LossCase] = &[
    LossCase {
        name: "file (a) malformed line: close",
        args: &["close", "p-t", "--json"],
        issues: &[
            br#"{"id":"p-t","title":"tkt-T","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","labels":["a"],"blockedBy":["p-b"]}"#,
            br#"{"id":"p-b","title":"tkt-B","status":"closed","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","blocks":["p-t"]}"#,
            br#"{"id":"p-m","title":"malformed line","status":"open""#,
        ],
        expect: Loss::Refused {
            lost: br#"{"id":"p-m","title":"malformed line","status":"open""#,
            refusal: "Refusing to rewrite issues.jsonl: line 3 is not a ticket record the Go tool can read, and rewriting the file would drop it. Nothing was written.",
        },
    },
    LossCase {
        name: "file (b) one id twice: close",
        args: &["close", "p-t", "--json"],
        issues: &[
            br#"{"id":"p-t","title":"tkt-T","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","labels":["a"],"blockedBy":["p-b"]}"#,
            br#"{"id":"p-b","title":"tkt-B","status":"closed","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","blocks":["p-t"]}"#,
            br#"{"id":"p-d","title":"dup first copy","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"id":"p-d","title":"dup second copy","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
        ],
        expect: Loss::Refused {
            lost: br#"{"id":"p-d","title":"dup first copy","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            refusal: "Refusing to rewrite issues.jsonl: lines 3 and 4 both hold the ticket \"p-d\", and rewriting the file would keep only the last one. Nothing was written.",
        },
    },
    LossCase {
        name: "file (b) two records without an id: close",
        args: &["close", "p-t", "--json"],
        issues: &[
            br#"{"id":"p-t","title":"tkt-T","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","labels":["a"],"blockedBy":["p-b"]}"#,
            br#"{"id":"p-b","title":"tkt-B","status":"closed","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","blocks":["p-t"]}"#,
            br#"{"title":"no id first","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"title":"no id second","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
        ],
        expect: Loss::Refused {
            lost: br#"{"title":"no id first","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            refusal: "Refusing to rewrite issues.jsonl: lines 3 and 4 both hold a record without an id, and rewriting the file would keep only the last one. Nothing was written.",
        },
    },
    LossCase {
        name: "file (c) repeated key: close",
        args: &["close", "p-t", "--json"],
        issues: &[
            br#"{"id":"p-t","title":"tkt-T","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","labels":["a"],"blockedBy":["p-b"]}"#,
            br#"{"id":"p-b","title":"tkt-B","status":"closed","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","blocks":["p-t"]}"#,
            br#"{"id":"p-r","title":"repeat first","title":"repeat last","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
        ],
        expect: Loss::Refused {
            lost: br#""title":"repeat first""#,
            refusal: "Refusing to rewrite issues.jsonl: the record on line 3 repeats the key \"title\", and rewriting the file would keep only its last value. Nothing was written.",
        },
    },
    LossCase {
        name: "file (a) malformed line: dep add",
        args: &["dep", "add", "p-t", "p-b", "--json"],
        issues: &[
            br#"{"id":"p-t","title":"tkt-T","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","labels":["a"],"blockedBy":["p-b"]}"#,
            br#"{"id":"p-b","title":"tkt-B","status":"closed","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","blocks":["p-t"]}"#,
            br#"{"id":"p-m","title":"malformed line","status":"open""#,
        ],
        expect: Loss::Refused {
            lost: br#"{"id":"p-m","title":"malformed line","status":"open""#,
            refusal: "Refusing to rewrite issues.jsonl: line 3 is not a ticket record the Go tool can read, and rewriting the file would drop it. Nothing was written.",
        },
    },
    LossCase {
        name: "file (b) one id twice: dep add",
        args: &["dep", "add", "p-t", "p-b", "--json"],
        issues: &[
            br#"{"id":"p-t","title":"tkt-T","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","labels":["a"],"blockedBy":["p-b"]}"#,
            br#"{"id":"p-b","title":"tkt-B","status":"closed","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","blocks":["p-t"]}"#,
            br#"{"id":"p-d","title":"dup first copy","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"id":"p-d","title":"dup second copy","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
        ],
        expect: Loss::Refused {
            lost: br#"{"id":"p-d","title":"dup first copy","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            refusal: "Refusing to rewrite issues.jsonl: lines 3 and 4 both hold the ticket \"p-d\", and rewriting the file would keep only the last one. Nothing was written.",
        },
    },
    LossCase {
        name: "file (b) two records without an id: dep add",
        args: &["dep", "add", "p-t", "p-b", "--json"],
        issues: &[
            br#"{"id":"p-t","title":"tkt-T","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","labels":["a"],"blockedBy":["p-b"]}"#,
            br#"{"id":"p-b","title":"tkt-B","status":"closed","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","blocks":["p-t"]}"#,
            br#"{"title":"no id first","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"title":"no id second","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
        ],
        expect: Loss::Refused {
            lost: br#"{"title":"no id first","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            refusal: "Refusing to rewrite issues.jsonl: lines 3 and 4 both hold a record without an id, and rewriting the file would keep only the last one. Nothing was written.",
        },
    },
    LossCase {
        name: "file (c) repeated key: dep add",
        args: &["dep", "add", "p-t", "p-b", "--json"],
        issues: &[
            br#"{"id":"p-t","title":"tkt-T","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","labels":["a"],"blockedBy":["p-b"]}"#,
            br#"{"id":"p-b","title":"tkt-B","status":"closed","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","blocks":["p-t"]}"#,
            br#"{"id":"p-r","title":"repeat first","title":"repeat last","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
        ],
        expect: Loss::Refused {
            lost: br#""title":"repeat first""#,
            refusal: "Refusing to rewrite issues.jsonl: the record on line 3 repeats the key \"title\", and rewriting the file would keep only its last value. Nothing was written.",
        },
    },
    LossCase {
        name: "file (a) malformed line: dep remove",
        args: &["dep", "remove", "p-t", "p-b", "--json"],
        issues: &[
            br#"{"id":"p-t","title":"tkt-T","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","labels":["a"],"blockedBy":["p-b"]}"#,
            br#"{"id":"p-b","title":"tkt-B","status":"closed","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","blocks":["p-t"]}"#,
            br#"{"id":"p-m","title":"malformed line","status":"open""#,
        ],
        expect: Loss::Refused {
            lost: br#"{"id":"p-m","title":"malformed line","status":"open""#,
            refusal: "Refusing to rewrite issues.jsonl: line 3 is not a ticket record the Go tool can read, and rewriting the file would drop it. Nothing was written.",
        },
    },
    LossCase {
        name: "file (b) one id twice: dep remove",
        args: &["dep", "remove", "p-t", "p-b", "--json"],
        issues: &[
            br#"{"id":"p-t","title":"tkt-T","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","labels":["a"],"blockedBy":["p-b"]}"#,
            br#"{"id":"p-b","title":"tkt-B","status":"closed","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","blocks":["p-t"]}"#,
            br#"{"id":"p-d","title":"dup first copy","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"id":"p-d","title":"dup second copy","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
        ],
        expect: Loss::Refused {
            lost: br#"{"id":"p-d","title":"dup first copy","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            refusal: "Refusing to rewrite issues.jsonl: lines 3 and 4 both hold the ticket \"p-d\", and rewriting the file would keep only the last one. Nothing was written.",
        },
    },
    LossCase {
        name: "file (b) two records without an id: dep remove",
        args: &["dep", "remove", "p-t", "p-b", "--json"],
        issues: &[
            br#"{"id":"p-t","title":"tkt-T","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","labels":["a"],"blockedBy":["p-b"]}"#,
            br#"{"id":"p-b","title":"tkt-B","status":"closed","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","blocks":["p-t"]}"#,
            br#"{"title":"no id first","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"title":"no id second","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
        ],
        expect: Loss::Refused {
            lost: br#"{"title":"no id first","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            refusal: "Refusing to rewrite issues.jsonl: lines 3 and 4 both hold a record without an id, and rewriting the file would keep only the last one. Nothing was written.",
        },
    },
    LossCase {
        name: "file (c) repeated key: dep remove",
        args: &["dep", "remove", "p-t", "p-b", "--json"],
        issues: &[
            br#"{"id":"p-t","title":"tkt-T","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","labels":["a"],"blockedBy":["p-b"]}"#,
            br#"{"id":"p-b","title":"tkt-B","status":"closed","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","blocks":["p-t"]}"#,
            br#"{"id":"p-r","title":"repeat first","title":"repeat last","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
        ],
        expect: Loss::Refused {
            lost: br#""title":"repeat first""#,
            refusal: "Refusing to rewrite issues.jsonl: the record on line 3 repeats the key \"title\", and rewriting the file would keep only its last value. Nothing was written.",
        },
    },
    LossCase {
        name: "file (a) malformed line: unblock --all",
        args: &["unblock", "p-t", "--all", "--json"],
        issues: &[
            br#"{"id":"p-t","title":"tkt-T","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","labels":["a"],"blockedBy":["p-b"]}"#,
            br#"{"id":"p-b","title":"tkt-B","status":"closed","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","blocks":["p-t"]}"#,
            br#"{"id":"p-m","title":"malformed line","status":"open""#,
        ],
        expect: Loss::Refused {
            lost: br#"{"id":"p-m","title":"malformed line","status":"open""#,
            refusal: "Refusing to rewrite issues.jsonl: line 3 is not a ticket record the Go tool can read, and rewriting the file would drop it. Nothing was written.",
        },
    },
    LossCase {
        name: "file (b) one id twice: unblock --all",
        args: &["unblock", "p-t", "--all", "--json"],
        issues: &[
            br#"{"id":"p-t","title":"tkt-T","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","labels":["a"],"blockedBy":["p-b"]}"#,
            br#"{"id":"p-b","title":"tkt-B","status":"closed","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","blocks":["p-t"]}"#,
            br#"{"id":"p-d","title":"dup first copy","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"id":"p-d","title":"dup second copy","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
        ],
        expect: Loss::Refused {
            lost: br#"{"id":"p-d","title":"dup first copy","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            refusal: "Refusing to rewrite issues.jsonl: lines 3 and 4 both hold the ticket \"p-d\", and rewriting the file would keep only the last one. Nothing was written.",
        },
    },
    LossCase {
        name: "file (b) two records without an id: unblock --all",
        args: &["unblock", "p-t", "--all", "--json"],
        issues: &[
            br#"{"id":"p-t","title":"tkt-T","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","labels":["a"],"blockedBy":["p-b"]}"#,
            br#"{"id":"p-b","title":"tkt-B","status":"closed","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","blocks":["p-t"]}"#,
            br#"{"title":"no id first","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"title":"no id second","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
        ],
        expect: Loss::Refused {
            lost: br#"{"title":"no id first","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            refusal: "Refusing to rewrite issues.jsonl: lines 3 and 4 both hold a record without an id, and rewriting the file would keep only the last one. Nothing was written.",
        },
    },
    LossCase {
        name: "file (c) repeated key: unblock --all",
        args: &["unblock", "p-t", "--all", "--json"],
        issues: &[
            br#"{"id":"p-t","title":"tkt-T","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","labels":["a"],"blockedBy":["p-b"]}"#,
            br#"{"id":"p-b","title":"tkt-B","status":"closed","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","blocks":["p-t"]}"#,
            br#"{"id":"p-r","title":"repeat first","title":"repeat last","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
        ],
        expect: Loss::Refused {
            lost: br#""title":"repeat first""#,
            refusal: "Refusing to rewrite issues.jsonl: the record on line 3 repeats the key \"title\", and rewriting the file would keep only its last value. Nothing was written.",
        },
    },
    LossCase {
        name: "file (a) malformed line: label add",
        args: &["label", "add", "p-t", "zz", "--json"],
        issues: &[
            br#"{"id":"p-t","title":"tkt-T","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","labels":["a"],"blockedBy":["p-b"]}"#,
            br#"{"id":"p-b","title":"tkt-B","status":"closed","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","blocks":["p-t"]}"#,
            br#"{"id":"p-m","title":"malformed line","status":"open""#,
        ],
        expect: Loss::Refused {
            lost: br#"{"id":"p-m","title":"malformed line","status":"open""#,
            refusal: "Refusing to rewrite issues.jsonl: line 3 is not a ticket record the Go tool can read, and rewriting the file would drop it. Nothing was written.",
        },
    },
    LossCase {
        name: "file (b) one id twice: label add",
        args: &["label", "add", "p-t", "zz", "--json"],
        issues: &[
            br#"{"id":"p-t","title":"tkt-T","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","labels":["a"],"blockedBy":["p-b"]}"#,
            br#"{"id":"p-b","title":"tkt-B","status":"closed","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","blocks":["p-t"]}"#,
            br#"{"id":"p-d","title":"dup first copy","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"id":"p-d","title":"dup second copy","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
        ],
        expect: Loss::Refused {
            lost: br#"{"id":"p-d","title":"dup first copy","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            refusal: "Refusing to rewrite issues.jsonl: lines 3 and 4 both hold the ticket \"p-d\", and rewriting the file would keep only the last one. Nothing was written.",
        },
    },
    LossCase {
        name: "file (b) two records without an id: label add",
        args: &["label", "add", "p-t", "zz", "--json"],
        issues: &[
            br#"{"id":"p-t","title":"tkt-T","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","labels":["a"],"blockedBy":["p-b"]}"#,
            br#"{"id":"p-b","title":"tkt-B","status":"closed","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","blocks":["p-t"]}"#,
            br#"{"title":"no id first","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"title":"no id second","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
        ],
        expect: Loss::Refused {
            lost: br#"{"title":"no id first","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            refusal: "Refusing to rewrite issues.jsonl: lines 3 and 4 both hold a record without an id, and rewriting the file would keep only the last one. Nothing was written.",
        },
    },
    LossCase {
        name: "file (c) repeated key: label add",
        args: &["label", "add", "p-t", "zz", "--json"],
        issues: &[
            br#"{"id":"p-t","title":"tkt-T","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","labels":["a"],"blockedBy":["p-b"]}"#,
            br#"{"id":"p-b","title":"tkt-B","status":"closed","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","blocks":["p-t"]}"#,
            br#"{"id":"p-r","title":"repeat first","title":"repeat last","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
        ],
        expect: Loss::Refused {
            lost: br#""title":"repeat first""#,
            refusal: "Refusing to rewrite issues.jsonl: the record on line 3 repeats the key \"title\", and rewriting the file would keep only its last value. Nothing was written.",
        },
    },
    LossCase {
        name: "file (a) malformed line: label remove",
        args: &["label", "remove", "p-t", "a", "--json"],
        issues: &[
            br#"{"id":"p-t","title":"tkt-T","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","labels":["a"],"blockedBy":["p-b"]}"#,
            br#"{"id":"p-b","title":"tkt-B","status":"closed","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","blocks":["p-t"]}"#,
            br#"{"id":"p-m","title":"malformed line","status":"open""#,
        ],
        expect: Loss::Refused {
            lost: br#"{"id":"p-m","title":"malformed line","status":"open""#,
            refusal: "Refusing to rewrite issues.jsonl: line 3 is not a ticket record the Go tool can read, and rewriting the file would drop it. Nothing was written.",
        },
    },
    LossCase {
        name: "file (b) one id twice: label remove",
        args: &["label", "remove", "p-t", "a", "--json"],
        issues: &[
            br#"{"id":"p-t","title":"tkt-T","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","labels":["a"],"blockedBy":["p-b"]}"#,
            br#"{"id":"p-b","title":"tkt-B","status":"closed","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","blocks":["p-t"]}"#,
            br#"{"id":"p-d","title":"dup first copy","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"id":"p-d","title":"dup second copy","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
        ],
        expect: Loss::Refused {
            lost: br#"{"id":"p-d","title":"dup first copy","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            refusal: "Refusing to rewrite issues.jsonl: lines 3 and 4 both hold the ticket \"p-d\", and rewriting the file would keep only the last one. Nothing was written.",
        },
    },
    LossCase {
        name: "file (b) two records without an id: label remove",
        args: &["label", "remove", "p-t", "a", "--json"],
        issues: &[
            br#"{"id":"p-t","title":"tkt-T","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","labels":["a"],"blockedBy":["p-b"]}"#,
            br#"{"id":"p-b","title":"tkt-B","status":"closed","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","blocks":["p-t"]}"#,
            br#"{"title":"no id first","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"title":"no id second","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
        ],
        expect: Loss::Refused {
            lost: br#"{"title":"no id first","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            refusal: "Refusing to rewrite issues.jsonl: lines 3 and 4 both hold a record without an id, and rewriting the file would keep only the last one. Nothing was written.",
        },
    },
    LossCase {
        name: "file (c) repeated key: label remove",
        args: &["label", "remove", "p-t", "a", "--json"],
        issues: &[
            br#"{"id":"p-t","title":"tkt-T","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","labels":["a"],"blockedBy":["p-b"]}"#,
            br#"{"id":"p-b","title":"tkt-B","status":"closed","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","blocks":["p-t"]}"#,
            br#"{"id":"p-r","title":"repeat first","title":"repeat last","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
        ],
        expect: Loss::Refused {
            lost: br#""title":"repeat first""#,
            refusal: "Refusing to rewrite issues.jsonl: the record on line 3 repeats the key \"title\", and rewriting the file would keep only its last value. Nothing was written.",
        },
    },
    LossCase {
        name: "field V1 L-add",
        args: &["label", "add", "p-t", "zz", "--json"],
        issues: &[
            br#"{"id":"p-t","title":"tkt-T","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","labels":"urgent"}"#,
            br#"{"id":"p-b","title":"tkt-B","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"id":"p-c","title":"tkt-C","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"id":"p-o","title":"tkt-O","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
        ],
        expect: Loss::Refused {
            lost: br#""labels":"urgent""#,
            refusal: "Refusing to rewrite issues.jsonl: line 1 (ticket \"p-t\") holds a \"labels\" value that is not a list of strings, and changing the field would replace that value. Nothing was written.",
        },
    },
    LossCase {
        name: "field V1 D-add-by",
        args: &["dep", "add", "p-t", "p-b", "--json"],
        issues: &[
            br#"{"id":"p-t","title":"tkt-T","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","blockedBy":"p-c"}"#,
            br#"{"id":"p-b","title":"tkt-B","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"id":"p-c","title":"tkt-C","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"id":"p-o","title":"tkt-O","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
        ],
        expect: Loss::Refused {
            lost: br#""blockedBy":"p-c""#,
            refusal: "Refusing to rewrite issues.jsonl: line 1 (ticket \"p-t\") holds a \"blockedBy\" value that is not a list of strings, and changing the field would replace that value. Nothing was written.",
        },
    },
    LossCase {
        name: "field V1 D-add-blocks",
        args: &["dep", "add", "p-t", "p-b", "--json"],
        issues: &[
            br#"{"id":"p-t","title":"tkt-T","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"id":"p-b","title":"tkt-B","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","blocks":"p-c"}"#,
            br#"{"id":"p-c","title":"tkt-C","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"id":"p-o","title":"tkt-O","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
        ],
        expect: Loss::Refused {
            lost: br#""blocks":"p-c""#,
            refusal: "Refusing to rewrite issues.jsonl: line 2 (ticket \"p-b\") holds a \"blocks\" value that is not a list of strings, and changing the field would replace that value. Nothing was written.",
        },
    },
    LossCase {
        name: "field V2 L-add",
        args: &["label", "add", "p-t", "zz", "--json"],
        issues: &[
            br#"{"id":"p-t","title":"tkt-T","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","labels":["a",5]}"#,
            br#"{"id":"p-b","title":"tkt-B","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"id":"p-c","title":"tkt-C","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"id":"p-o","title":"tkt-O","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
        ],
        expect: Loss::Refused {
            lost: br#""labels":["a",5]"#,
            refusal: "Refusing to rewrite issues.jsonl: line 1 (ticket \"p-t\") holds an element of \"labels\" that is not a string, and changing the field would write it as an empty string. Nothing was written.",
        },
    },
    LossCase {
        name: "field V2 L-remove",
        args: &["label", "remove", "p-t", "a", "--json"],
        issues: &[
            br#"{"id":"p-t","title":"tkt-T","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","labels":["a",5]}"#,
            br#"{"id":"p-b","title":"tkt-B","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"id":"p-c","title":"tkt-C","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"id":"p-o","title":"tkt-O","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
        ],
        expect: Loss::Refused {
            lost: br#""labels":["a",5]"#,
            refusal: "Refusing to rewrite issues.jsonl: line 1 (ticket \"p-t\") holds an element of \"labels\" that is not a string, and changing the field would write it as an empty string. Nothing was written.",
        },
    },
    LossCase {
        name: "field V2 D-add-by",
        args: &["dep", "add", "p-t", "p-b", "--json"],
        issues: &[
            br#"{"id":"p-t","title":"tkt-T","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","blockedBy":["p-c",5]}"#,
            br#"{"id":"p-b","title":"tkt-B","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"id":"p-c","title":"tkt-C","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"id":"p-o","title":"tkt-O","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
        ],
        expect: Loss::Refused {
            lost: br#""blockedBy":["p-c",5]"#,
            refusal: "Refusing to rewrite issues.jsonl: line 1 (ticket \"p-t\") holds an element of \"blockedBy\" that is not a string, and changing the field would write it as an empty string. Nothing was written.",
        },
    },
    LossCase {
        name: "field V2 D-add-blocks",
        args: &["dep", "add", "p-t", "p-b", "--json"],
        issues: &[
            br#"{"id":"p-t","title":"tkt-T","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"id":"p-b","title":"tkt-B","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","blocks":["p-c",5]}"#,
            br#"{"id":"p-c","title":"tkt-C","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"id":"p-o","title":"tkt-O","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
        ],
        expect: Loss::Refused {
            lost: br#""blocks":["p-c",5]"#,
            refusal: "Refusing to rewrite issues.jsonl: line 2 (ticket \"p-b\") holds an element of \"blocks\" that is not a string, and changing the field would write it as an empty string. Nothing was written.",
        },
    },
    LossCase {
        name: "field V2 D-remove-by",
        args: &["dep", "remove", "p-t", "p-b", "--json"],
        issues: &[
            br#"{"id":"p-t","title":"tkt-T","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","blockedBy":["p-b",5]}"#,
            br#"{"id":"p-b","title":"tkt-B","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","blocks":["p-t"]}"#,
            br#"{"id":"p-c","title":"tkt-C","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"id":"p-o","title":"tkt-O","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
        ],
        expect: Loss::Refused {
            lost: br#""blockedBy":["p-b",5]"#,
            refusal: "Refusing to rewrite issues.jsonl: line 1 (ticket \"p-t\") holds an element of \"blockedBy\" that is not a string, and changing the field would write it as an empty string. Nothing was written.",
        },
    },
    LossCase {
        name: "field V2 D-remove-blocks",
        args: &["dep", "remove", "p-t", "p-b", "--json"],
        issues: &[
            br#"{"id":"p-t","title":"tkt-T","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","blockedBy":["p-b"]}"#,
            br#"{"id":"p-b","title":"tkt-B","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","blocks":["p-t",5]}"#,
            br#"{"id":"p-c","title":"tkt-C","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"id":"p-o","title":"tkt-O","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
        ],
        expect: Loss::Refused {
            lost: br#""blocks":["p-t",5]"#,
            refusal: "Refusing to rewrite issues.jsonl: line 2 (ticket \"p-b\") holds an element of \"blocks\" that is not a string, and changing the field would write it as an empty string. Nothing was written.",
        },
    },
    LossCase {
        name: "field V2 U-from",
        args: &["unblock", "p-t", "--from", "p-b", "--json"],
        issues: &[
            br#"{"id":"p-t","title":"tkt-T","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","blockedBy":["p-b",5]}"#,
            br#"{"id":"p-b","title":"tkt-B","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"id":"p-c","title":"tkt-C","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"id":"p-o","title":"tkt-O","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
        ],
        expect: Loss::Refused {
            lost: br#""blockedBy":["p-b",5]"#,
            refusal: "Refusing to rewrite issues.jsonl: line 1 (ticket \"p-t\") holds an element of \"blockedBy\" that is not a string, and changing the field would write it as an empty string. Nothing was written.",
        },
    },
    LossCase {
        name: "field V2 U-all",
        args: &["unblock", "p-t", "--all", "--json"],
        issues: &[
            br#"{"id":"p-t","title":"tkt-T","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","blockedBy":["p-b",5]}"#,
            br#"{"id":"p-b","title":"tkt-B","status":"closed","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"id":"p-c","title":"tkt-C","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"id":"p-o","title":"tkt-O","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
        ],
        expect: Loss::Refused {
            lost: br#""blockedBy":["p-b",5]"#,
            refusal: "Refusing to rewrite issues.jsonl: line 1 (ticket \"p-t\") holds an element of \"blockedBy\" that is not a string, and changing the field would write it as an empty string. Nothing was written.",
        },
    },
    LossCase {
        name: "field V2 U-blocks",
        args: &["unblock", "p-t", "--from", "p-b", "--json"],
        issues: &[
            br#"{"id":"p-t","title":"tkt-T","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","blockedBy":["p-b"]}"#,
            br#"{"id":"p-b","title":"tkt-B","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","blocks":["p-t",5]}"#,
            br#"{"id":"p-c","title":"tkt-C","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"id":"p-o","title":"tkt-O","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
        ],
        expect: Loss::Refused {
            lost: br#""blocks":["p-t",5]"#,
            refusal: "Refusing to rewrite issues.jsonl: line 2 (ticket \"p-b\") holds an element of \"blocks\" that is not a string, and changing the field would write it as an empty string. Nothing was written.",
        },
    },
    LossCase {
        name: "field V2 C-dependent",
        args: &["close", "p-b", "--json"],
        issues: &[
            br#"{"id":"p-t","title":"tkt-T","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","blockedBy":["p-b",5]}"#,
            br#"{"id":"p-b","title":"tkt-B","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","blocks":["p-t"]}"#,
            br#"{"id":"p-c","title":"tkt-C","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"id":"p-o","title":"tkt-O","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
        ],
        expect: Loss::Refused {
            lost: br#""blockedBy":["p-b",5]"#,
            refusal: "Refusing to rewrite issues.jsonl: line 1 (ticket \"p-t\") holds an element of \"blockedBy\" that is not a string, and changing the field would write it as an empty string. Nothing was written.",
        },
    },
    LossCase {
        name: "field V3 L-add",
        args: &["label", "add", "p-t", "zz", "--json"],
        issues: &[
            br#"{"id":"p-t","title":"tkt-T","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","labels":["a",null]}"#,
            br#"{"id":"p-b","title":"tkt-B","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"id":"p-c","title":"tkt-C","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"id":"p-o","title":"tkt-O","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
        ],
        expect: Loss::Refused {
            lost: br#""labels":["a",null]"#,
            refusal: "Refusing to rewrite issues.jsonl: line 1 (ticket \"p-t\") holds an element of \"labels\" that is not a string, and changing the field would write it as an empty string. Nothing was written.",
        },
    },
    LossCase {
        name: "field V3 L-remove",
        args: &["label", "remove", "p-t", "a", "--json"],
        issues: &[
            br#"{"id":"p-t","title":"tkt-T","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","labels":["a",null]}"#,
            br#"{"id":"p-b","title":"tkt-B","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"id":"p-c","title":"tkt-C","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"id":"p-o","title":"tkt-O","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
        ],
        expect: Loss::Refused {
            lost: br#""labels":["a",null]"#,
            refusal: "Refusing to rewrite issues.jsonl: line 1 (ticket \"p-t\") holds an element of \"labels\" that is not a string, and changing the field would write it as an empty string. Nothing was written.",
        },
    },
    LossCase {
        name: "field V3 D-add-by",
        args: &["dep", "add", "p-t", "p-b", "--json"],
        issues: &[
            br#"{"id":"p-t","title":"tkt-T","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","blockedBy":["p-c",null]}"#,
            br#"{"id":"p-b","title":"tkt-B","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"id":"p-c","title":"tkt-C","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"id":"p-o","title":"tkt-O","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
        ],
        expect: Loss::Refused {
            lost: br#""blockedBy":["p-c",null]"#,
            refusal: "Refusing to rewrite issues.jsonl: line 1 (ticket \"p-t\") holds an element of \"blockedBy\" that is not a string, and changing the field would write it as an empty string. Nothing was written.",
        },
    },
    LossCase {
        name: "field V3 D-add-blocks",
        args: &["dep", "add", "p-t", "p-b", "--json"],
        issues: &[
            br#"{"id":"p-t","title":"tkt-T","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"id":"p-b","title":"tkt-B","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","blocks":["p-c",null]}"#,
            br#"{"id":"p-c","title":"tkt-C","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"id":"p-o","title":"tkt-O","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
        ],
        expect: Loss::Refused {
            lost: br#""blocks":["p-c",null]"#,
            refusal: "Refusing to rewrite issues.jsonl: line 2 (ticket \"p-b\") holds an element of \"blocks\" that is not a string, and changing the field would write it as an empty string. Nothing was written.",
        },
    },
    LossCase {
        name: "field V3 D-remove-by",
        args: &["dep", "remove", "p-t", "p-b", "--json"],
        issues: &[
            br#"{"id":"p-t","title":"tkt-T","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","blockedBy":["p-b",null]}"#,
            br#"{"id":"p-b","title":"tkt-B","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","blocks":["p-t"]}"#,
            br#"{"id":"p-c","title":"tkt-C","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"id":"p-o","title":"tkt-O","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
        ],
        expect: Loss::Refused {
            lost: br#""blockedBy":["p-b",null]"#,
            refusal: "Refusing to rewrite issues.jsonl: line 1 (ticket \"p-t\") holds an element of \"blockedBy\" that is not a string, and changing the field would write it as an empty string. Nothing was written.",
        },
    },
    LossCase {
        name: "field V3 D-remove-blocks",
        args: &["dep", "remove", "p-t", "p-b", "--json"],
        issues: &[
            br#"{"id":"p-t","title":"tkt-T","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","blockedBy":["p-b"]}"#,
            br#"{"id":"p-b","title":"tkt-B","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","blocks":["p-t",null]}"#,
            br#"{"id":"p-c","title":"tkt-C","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"id":"p-o","title":"tkt-O","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
        ],
        expect: Loss::Refused {
            lost: br#""blocks":["p-t",null]"#,
            refusal: "Refusing to rewrite issues.jsonl: line 2 (ticket \"p-b\") holds an element of \"blocks\" that is not a string, and changing the field would write it as an empty string. Nothing was written.",
        },
    },
    LossCase {
        name: "field V3 U-from",
        args: &["unblock", "p-t", "--from", "p-b", "--json"],
        issues: &[
            br#"{"id":"p-t","title":"tkt-T","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","blockedBy":["p-b",null]}"#,
            br#"{"id":"p-b","title":"tkt-B","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"id":"p-c","title":"tkt-C","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"id":"p-o","title":"tkt-O","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
        ],
        expect: Loss::Refused {
            lost: br#""blockedBy":["p-b",null]"#,
            refusal: "Refusing to rewrite issues.jsonl: line 1 (ticket \"p-t\") holds an element of \"blockedBy\" that is not a string, and changing the field would write it as an empty string. Nothing was written.",
        },
    },
    LossCase {
        name: "field V3 U-all",
        args: &["unblock", "p-t", "--all", "--json"],
        issues: &[
            br#"{"id":"p-t","title":"tkt-T","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","blockedBy":["p-b",null]}"#,
            br#"{"id":"p-b","title":"tkt-B","status":"closed","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"id":"p-c","title":"tkt-C","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"id":"p-o","title":"tkt-O","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
        ],
        expect: Loss::Refused {
            lost: br#""blockedBy":["p-b",null]"#,
            refusal: "Refusing to rewrite issues.jsonl: line 1 (ticket \"p-t\") holds an element of \"blockedBy\" that is not a string, and changing the field would write it as an empty string. Nothing was written.",
        },
    },
    LossCase {
        name: "field V3 U-blocks",
        args: &["unblock", "p-t", "--from", "p-b", "--json"],
        issues: &[
            br#"{"id":"p-t","title":"tkt-T","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","blockedBy":["p-b"]}"#,
            br#"{"id":"p-b","title":"tkt-B","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","blocks":["p-t",null]}"#,
            br#"{"id":"p-c","title":"tkt-C","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"id":"p-o","title":"tkt-O","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
        ],
        expect: Loss::Refused {
            lost: br#""blocks":["p-t",null]"#,
            refusal: "Refusing to rewrite issues.jsonl: line 2 (ticket \"p-b\") holds an element of \"blocks\" that is not a string, and changing the field would write it as an empty string. Nothing was written.",
        },
    },
    LossCase {
        name: "field V3 C-dependent",
        args: &["close", "p-b", "--json"],
        issues: &[
            br#"{"id":"p-t","title":"tkt-T","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","blockedBy":["p-b",null]}"#,
            br#"{"id":"p-b","title":"tkt-B","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","blocks":["p-t"]}"#,
            br#"{"id":"p-c","title":"tkt-C","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"id":"p-o","title":"tkt-O","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
        ],
        expect: Loss::Refused {
            lost: br#""blockedBy":["p-b",null]"#,
            refusal: "Refusing to rewrite issues.jsonl: line 1 (ticket \"p-t\") holds an element of \"blockedBy\" that is not a string, and changing the field would write it as an empty string. Nothing was written.",
        },
    },
    LossCase {
        name: "field V4 L-add",
        args: &["label", "add", "p-t", "zz", "--json"],
        issues: &[
            b"{\"id\":\"p-t\",\"title\":\"tkt-T\",\"status\":\"open\",\"type\":\"task\",\"priority\":2,\"createdAt\":\"2026-01-01T00:00:00.000Z\",\"updatedAt\":\"2026-01-01T00:00:00.000Z\",\"labels\":[\"a\",\"\xff\"]}",
            br#"{"id":"p-b","title":"tkt-B","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"id":"p-c","title":"tkt-C","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"id":"p-o","title":"tkt-O","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
        ],
        expect: Loss::Refused {
            lost: b"\"labels\":[\"a\",\"\xff\"]",
            refusal: "Refusing to rewrite issues.jsonl: line 1 (ticket \"p-t\") holds an element of \"labels\" with invalid UTF-8 or a lone surrogate escape, and changing the field would write it as U+FFFD. Nothing was written.",
        },
    },
    LossCase {
        name: "field V4 L-remove",
        args: &["label", "remove", "p-t", "a", "--json"],
        issues: &[
            b"{\"id\":\"p-t\",\"title\":\"tkt-T\",\"status\":\"open\",\"type\":\"task\",\"priority\":2,\"createdAt\":\"2026-01-01T00:00:00.000Z\",\"updatedAt\":\"2026-01-01T00:00:00.000Z\",\"labels\":[\"a\",\"\xff\"]}",
            br#"{"id":"p-b","title":"tkt-B","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"id":"p-c","title":"tkt-C","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"id":"p-o","title":"tkt-O","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
        ],
        expect: Loss::Refused {
            lost: b"\"labels\":[\"a\",\"\xff\"]",
            refusal: "Refusing to rewrite issues.jsonl: line 1 (ticket \"p-t\") holds an element of \"labels\" with invalid UTF-8 or a lone surrogate escape, and changing the field would write it as U+FFFD. Nothing was written.",
        },
    },
    LossCase {
        name: "field V4 D-add-by",
        args: &["dep", "add", "p-t", "p-b", "--json"],
        issues: &[
            b"{\"id\":\"p-t\",\"title\":\"tkt-T\",\"status\":\"open\",\"type\":\"task\",\"priority\":2,\"createdAt\":\"2026-01-01T00:00:00.000Z\",\"updatedAt\":\"2026-01-01T00:00:00.000Z\",\"blockedBy\":[\"p-c\",\"\xff\"]}",
            br#"{"id":"p-b","title":"tkt-B","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"id":"p-c","title":"tkt-C","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"id":"p-o","title":"tkt-O","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
        ],
        expect: Loss::Refused {
            lost: b"\"blockedBy\":[\"p-c\",\"\xff\"]",
            refusal: "Refusing to rewrite issues.jsonl: line 1 (ticket \"p-t\") holds an element of \"blockedBy\" with invalid UTF-8 or a lone surrogate escape, and changing the field would write it as U+FFFD. Nothing was written.",
        },
    },
    LossCase {
        name: "field V4 D-add-blocks",
        args: &["dep", "add", "p-t", "p-b", "--json"],
        issues: &[
            br#"{"id":"p-t","title":"tkt-T","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            b"{\"id\":\"p-b\",\"title\":\"tkt-B\",\"status\":\"open\",\"type\":\"task\",\"priority\":2,\"createdAt\":\"2026-01-01T00:00:00.000Z\",\"updatedAt\":\"2026-01-01T00:00:00.000Z\",\"blocks\":[\"p-c\",\"\xff\"]}",
            br#"{"id":"p-c","title":"tkt-C","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"id":"p-o","title":"tkt-O","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
        ],
        expect: Loss::Refused {
            lost: b"\"blocks\":[\"p-c\",\"\xff\"]",
            refusal: "Refusing to rewrite issues.jsonl: line 2 (ticket \"p-b\") holds an element of \"blocks\" with invalid UTF-8 or a lone surrogate escape, and changing the field would write it as U+FFFD. Nothing was written.",
        },
    },
    LossCase {
        name: "field V4 D-remove-by",
        args: &["dep", "remove", "p-t", "p-b", "--json"],
        issues: &[
            b"{\"id\":\"p-t\",\"title\":\"tkt-T\",\"status\":\"open\",\"type\":\"task\",\"priority\":2,\"createdAt\":\"2026-01-01T00:00:00.000Z\",\"updatedAt\":\"2026-01-01T00:00:00.000Z\",\"blockedBy\":[\"p-b\",\"\xff\"]}",
            br#"{"id":"p-b","title":"tkt-B","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","blocks":["p-t"]}"#,
            br#"{"id":"p-c","title":"tkt-C","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"id":"p-o","title":"tkt-O","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
        ],
        expect: Loss::Refused {
            lost: b"\"blockedBy\":[\"p-b\",\"\xff\"]",
            refusal: "Refusing to rewrite issues.jsonl: line 1 (ticket \"p-t\") holds an element of \"blockedBy\" with invalid UTF-8 or a lone surrogate escape, and changing the field would write it as U+FFFD. Nothing was written.",
        },
    },
    LossCase {
        name: "field V4 D-remove-blocks",
        args: &["dep", "remove", "p-t", "p-b", "--json"],
        issues: &[
            br#"{"id":"p-t","title":"tkt-T","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","blockedBy":["p-b"]}"#,
            b"{\"id\":\"p-b\",\"title\":\"tkt-B\",\"status\":\"open\",\"type\":\"task\",\"priority\":2,\"createdAt\":\"2026-01-01T00:00:00.000Z\",\"updatedAt\":\"2026-01-01T00:00:00.000Z\",\"blocks\":[\"p-t\",\"\xff\"]}",
            br#"{"id":"p-c","title":"tkt-C","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"id":"p-o","title":"tkt-O","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
        ],
        expect: Loss::Refused {
            lost: b"\"blocks\":[\"p-t\",\"\xff\"]",
            refusal: "Refusing to rewrite issues.jsonl: line 2 (ticket \"p-b\") holds an element of \"blocks\" with invalid UTF-8 or a lone surrogate escape, and changing the field would write it as U+FFFD. Nothing was written.",
        },
    },
    LossCase {
        name: "field V4 U-from",
        args: &["unblock", "p-t", "--from", "p-b", "--json"],
        issues: &[
            b"{\"id\":\"p-t\",\"title\":\"tkt-T\",\"status\":\"open\",\"type\":\"task\",\"priority\":2,\"createdAt\":\"2026-01-01T00:00:00.000Z\",\"updatedAt\":\"2026-01-01T00:00:00.000Z\",\"blockedBy\":[\"p-b\",\"\xff\"]}",
            br#"{"id":"p-b","title":"tkt-B","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"id":"p-c","title":"tkt-C","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"id":"p-o","title":"tkt-O","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
        ],
        expect: Loss::Refused {
            lost: b"\"blockedBy\":[\"p-b\",\"\xff\"]",
            refusal: "Refusing to rewrite issues.jsonl: line 1 (ticket \"p-t\") holds an element of \"blockedBy\" with invalid UTF-8 or a lone surrogate escape, and changing the field would write it as U+FFFD. Nothing was written.",
        },
    },
    LossCase {
        name: "field V4 U-all",
        args: &["unblock", "p-t", "--all", "--json"],
        issues: &[
            b"{\"id\":\"p-t\",\"title\":\"tkt-T\",\"status\":\"open\",\"type\":\"task\",\"priority\":2,\"createdAt\":\"2026-01-01T00:00:00.000Z\",\"updatedAt\":\"2026-01-01T00:00:00.000Z\",\"blockedBy\":[\"p-b\",\"\xff\"]}",
            br#"{"id":"p-b","title":"tkt-B","status":"closed","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"id":"p-c","title":"tkt-C","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"id":"p-o","title":"tkt-O","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
        ],
        expect: Loss::Refused {
            lost: b"\"blockedBy\":[\"p-b\",\"\xff\"]",
            refusal: "Refusing to rewrite issues.jsonl: line 1 (ticket \"p-t\") holds an element of \"blockedBy\" with invalid UTF-8 or a lone surrogate escape, and changing the field would write it as U+FFFD. Nothing was written.",
        },
    },
    LossCase {
        name: "field V4 U-blocks",
        args: &["unblock", "p-t", "--from", "p-b", "--json"],
        issues: &[
            br#"{"id":"p-t","title":"tkt-T","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","blockedBy":["p-b"]}"#,
            b"{\"id\":\"p-b\",\"title\":\"tkt-B\",\"status\":\"open\",\"type\":\"task\",\"priority\":2,\"createdAt\":\"2026-01-01T00:00:00.000Z\",\"updatedAt\":\"2026-01-01T00:00:00.000Z\",\"blocks\":[\"p-t\",\"\xff\"]}",
            br#"{"id":"p-c","title":"tkt-C","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"id":"p-o","title":"tkt-O","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
        ],
        expect: Loss::Refused {
            lost: b"\"blocks\":[\"p-t\",\"\xff\"]",
            refusal: "Refusing to rewrite issues.jsonl: line 2 (ticket \"p-b\") holds an element of \"blocks\" with invalid UTF-8 or a lone surrogate escape, and changing the field would write it as U+FFFD. Nothing was written.",
        },
    },
    LossCase {
        name: "field V4 C-dependent",
        args: &["close", "p-b", "--json"],
        issues: &[
            b"{\"id\":\"p-t\",\"title\":\"tkt-T\",\"status\":\"open\",\"type\":\"task\",\"priority\":2,\"createdAt\":\"2026-01-01T00:00:00.000Z\",\"updatedAt\":\"2026-01-01T00:00:00.000Z\",\"blockedBy\":[\"p-b\",\"\xff\"]}",
            br#"{"id":"p-b","title":"tkt-B","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","blocks":["p-t"]}"#,
            br#"{"id":"p-c","title":"tkt-C","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"id":"p-o","title":"tkt-O","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
        ],
        expect: Loss::Refused {
            lost: b"\"blockedBy\":[\"p-b\",\"\xff\"]",
            refusal: "Refusing to rewrite issues.jsonl: line 1 (ticket \"p-t\") holds an element of \"blockedBy\" with invalid UTF-8 or a lone surrogate escape, and changing the field would write it as U+FFFD. Nothing was written.",
        },
    },
    LossCase {
        name: "field V5 L-add",
        args: &["label", "add", "p-t", "zz", "--json"],
        issues: &[
            br#"{"id":"p-t","title":"tkt-T","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","labels":["a","\ud800"]}"#,
            br#"{"id":"p-b","title":"tkt-B","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"id":"p-c","title":"tkt-C","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"id":"p-o","title":"tkt-O","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
        ],
        expect: Loss::Refused {
            lost: br#""labels":["a","\ud800"]"#,
            refusal: "Refusing to rewrite issues.jsonl: line 1 (ticket \"p-t\") holds an element of \"labels\" with invalid UTF-8 or a lone surrogate escape, and changing the field would write it as U+FFFD. Nothing was written.",
        },
    },
    LossCase {
        name: "field V5 L-remove",
        args: &["label", "remove", "p-t", "a", "--json"],
        issues: &[
            br#"{"id":"p-t","title":"tkt-T","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","labels":["a","\ud800"]}"#,
            br#"{"id":"p-b","title":"tkt-B","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"id":"p-c","title":"tkt-C","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"id":"p-o","title":"tkt-O","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
        ],
        expect: Loss::Refused {
            lost: br#""labels":["a","\ud800"]"#,
            refusal: "Refusing to rewrite issues.jsonl: line 1 (ticket \"p-t\") holds an element of \"labels\" with invalid UTF-8 or a lone surrogate escape, and changing the field would write it as U+FFFD. Nothing was written.",
        },
    },
    LossCase {
        name: "field V5 D-add-by",
        args: &["dep", "add", "p-t", "p-b", "--json"],
        issues: &[
            br#"{"id":"p-t","title":"tkt-T","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","blockedBy":["p-c","\ud800"]}"#,
            br#"{"id":"p-b","title":"tkt-B","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"id":"p-c","title":"tkt-C","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"id":"p-o","title":"tkt-O","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
        ],
        expect: Loss::Refused {
            lost: br#""blockedBy":["p-c","\ud800"]"#,
            refusal: "Refusing to rewrite issues.jsonl: line 1 (ticket \"p-t\") holds an element of \"blockedBy\" with invalid UTF-8 or a lone surrogate escape, and changing the field would write it as U+FFFD. Nothing was written.",
        },
    },
    LossCase {
        name: "field V5 D-add-blocks",
        args: &["dep", "add", "p-t", "p-b", "--json"],
        issues: &[
            br#"{"id":"p-t","title":"tkt-T","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"id":"p-b","title":"tkt-B","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","blocks":["p-c","\ud800"]}"#,
            br#"{"id":"p-c","title":"tkt-C","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"id":"p-o","title":"tkt-O","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
        ],
        expect: Loss::Refused {
            lost: br#""blocks":["p-c","\ud800"]"#,
            refusal: "Refusing to rewrite issues.jsonl: line 2 (ticket \"p-b\") holds an element of \"blocks\" with invalid UTF-8 or a lone surrogate escape, and changing the field would write it as U+FFFD. Nothing was written.",
        },
    },
    LossCase {
        name: "field V5 D-remove-by",
        args: &["dep", "remove", "p-t", "p-b", "--json"],
        issues: &[
            br#"{"id":"p-t","title":"tkt-T","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","blockedBy":["p-b","\ud800"]}"#,
            br#"{"id":"p-b","title":"tkt-B","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","blocks":["p-t"]}"#,
            br#"{"id":"p-c","title":"tkt-C","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"id":"p-o","title":"tkt-O","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
        ],
        expect: Loss::Refused {
            lost: br#""blockedBy":["p-b","\ud800"]"#,
            refusal: "Refusing to rewrite issues.jsonl: line 1 (ticket \"p-t\") holds an element of \"blockedBy\" with invalid UTF-8 or a lone surrogate escape, and changing the field would write it as U+FFFD. Nothing was written.",
        },
    },
    LossCase {
        name: "field V5 D-remove-blocks",
        args: &["dep", "remove", "p-t", "p-b", "--json"],
        issues: &[
            br#"{"id":"p-t","title":"tkt-T","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","blockedBy":["p-b"]}"#,
            br#"{"id":"p-b","title":"tkt-B","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","blocks":["p-t","\ud800"]}"#,
            br#"{"id":"p-c","title":"tkt-C","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"id":"p-o","title":"tkt-O","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
        ],
        expect: Loss::Refused {
            lost: br#""blocks":["p-t","\ud800"]"#,
            refusal: "Refusing to rewrite issues.jsonl: line 2 (ticket \"p-b\") holds an element of \"blocks\" with invalid UTF-8 or a lone surrogate escape, and changing the field would write it as U+FFFD. Nothing was written.",
        },
    },
    LossCase {
        name: "field V5 U-from",
        args: &["unblock", "p-t", "--from", "p-b", "--json"],
        issues: &[
            br#"{"id":"p-t","title":"tkt-T","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","blockedBy":["p-b","\ud800"]}"#,
            br#"{"id":"p-b","title":"tkt-B","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"id":"p-c","title":"tkt-C","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"id":"p-o","title":"tkt-O","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
        ],
        expect: Loss::Refused {
            lost: br#""blockedBy":["p-b","\ud800"]"#,
            refusal: "Refusing to rewrite issues.jsonl: line 1 (ticket \"p-t\") holds an element of \"blockedBy\" with invalid UTF-8 or a lone surrogate escape, and changing the field would write it as U+FFFD. Nothing was written.",
        },
    },
    LossCase {
        name: "field V5 U-all",
        args: &["unblock", "p-t", "--all", "--json"],
        issues: &[
            br#"{"id":"p-t","title":"tkt-T","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","blockedBy":["p-b","\ud800"]}"#,
            br#"{"id":"p-b","title":"tkt-B","status":"closed","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"id":"p-c","title":"tkt-C","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"id":"p-o","title":"tkt-O","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
        ],
        expect: Loss::Refused {
            lost: br#""blockedBy":["p-b","\ud800"]"#,
            refusal: "Refusing to rewrite issues.jsonl: line 1 (ticket \"p-t\") holds an element of \"blockedBy\" with invalid UTF-8 or a lone surrogate escape, and changing the field would write it as U+FFFD. Nothing was written.",
        },
    },
    LossCase {
        name: "field V5 U-blocks",
        args: &["unblock", "p-t", "--from", "p-b", "--json"],
        issues: &[
            br#"{"id":"p-t","title":"tkt-T","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","blockedBy":["p-b"]}"#,
            br#"{"id":"p-b","title":"tkt-B","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","blocks":["p-t","\ud800"]}"#,
            br#"{"id":"p-c","title":"tkt-C","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"id":"p-o","title":"tkt-O","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
        ],
        expect: Loss::Refused {
            lost: br#""blocks":["p-t","\ud800"]"#,
            refusal: "Refusing to rewrite issues.jsonl: line 2 (ticket \"p-b\") holds an element of \"blocks\" with invalid UTF-8 or a lone surrogate escape, and changing the field would write it as U+FFFD. Nothing was written.",
        },
    },
    LossCase {
        name: "field V5 C-dependent",
        args: &["close", "p-b", "--json"],
        issues: &[
            br#"{"id":"p-t","title":"tkt-T","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","blockedBy":["p-b","\ud800"]}"#,
            br#"{"id":"p-b","title":"tkt-B","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","blocks":["p-t"]}"#,
            br#"{"id":"p-c","title":"tkt-C","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"id":"p-o","title":"tkt-O","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
        ],
        expect: Loss::Refused {
            lost: br#""blockedBy":["p-b","\ud800"]"#,
            refusal: "Refusing to rewrite issues.jsonl: line 1 (ticket \"p-t\") holds an element of \"blockedBy\" with invalid UTF-8 or a lone surrogate escape, and changing the field would write it as U+FFFD. Nothing was written.",
        },
    },
    LossCase {
        name: "field V6 L-add",
        args: &["label", "add", "p-t", "zz", "--json"],
        issues: &[
            b"{\"id\":\"p-t\",\"title\":\"tkt-T\",\"status\":\"open\",\"type\":\"task\",\"priority\":2,\"createdAt\":\"2026-01-01T00:00:00.000Z\",\"updatedAt\":\"2026-01-01T00:00:00.000Z\",\"labels\":[\"\xff\",\"\xfe\"]}",
            br#"{"id":"p-b","title":"tkt-B","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"id":"p-c","title":"tkt-C","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"id":"p-o","title":"tkt-O","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
        ],
        expect: Loss::Refused {
            lost: b"\"labels\":[\"\xff\",\"\xfe\"]",
            refusal: "Refusing to rewrite issues.jsonl: line 1 (ticket \"p-t\") holds an element of \"labels\" with invalid UTF-8 or a lone surrogate escape, and changing the field would write it as U+FFFD. Nothing was written.",
        },
    },
    LossCase {
        name: "field (source) labels 5: label add",
        args: &["label", "add", "p-t", "zz", "--json"],
        issues: &[
            br#"{"id":"p-t","title":"tkt-T","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","labels":5}"#,
            br#"{"id":"p-b","title":"tkt-B","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"id":"p-c","title":"tkt-C","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"id":"p-o","title":"tkt-O","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
        ],
        expect: Loss::Refused {
            lost: br#""labels":5"#,
            refusal: "Refusing to rewrite issues.jsonl: line 1 (ticket \"p-t\") holds a \"labels\" value that is not a list of strings, and changing the field would replace that value. Nothing was written.",
        },
    },
    LossCase {
        name: "field (source) labels [\"a\",true]: label add",
        args: &["label", "add", "p-t", "zz", "--json"],
        issues: &[
            br#"{"id":"p-t","title":"tkt-T","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","labels":["a",true]}"#,
            br#"{"id":"p-b","title":"tkt-B","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"id":"p-c","title":"tkt-C","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"id":"p-o","title":"tkt-O","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
        ],
        expect: Loss::Refused {
            lost: br#""labels":["a",true]"#,
            refusal: "Refusing to rewrite issues.jsonl: line 1 (ticket \"p-t\") holds an element of \"labels\" that is not a string, and changing the field would write it as an empty string. Nothing was written.",
        },
    },
    LossCase {
        name: "field (source) labels [\"a\",{}]: label add",
        args: &["label", "add", "p-t", "zz", "--json"],
        issues: &[
            br#"{"id":"p-t","title":"tkt-T","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","labels":["a",{}]}"#,
            br#"{"id":"p-b","title":"tkt-B","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"id":"p-c","title":"tkt-C","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"id":"p-o","title":"tkt-O","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
        ],
        expect: Loss::Refused {
            lost: br#""labels":["a",{}]"#,
            refusal: "Refusing to rewrite issues.jsonl: line 1 (ticket \"p-t\") holds an element of \"labels\" that is not a string, and changing the field would write it as an empty string. Nothing was written.",
        },
    },
    LossCase {
        name: "control V1 L-remove",
        args: &["label", "remove", "p-t", "a", "--json"],
        issues: &[
            br#"{"id":"p-t","title":"tkt-T","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","labels":"urgent"}"#,
            br#"{"id":"p-b","title":"tkt-B","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"id":"p-c","title":"tkt-C","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"id":"p-o","title":"tkt-O","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
        ],
        expect: Loss::Kept {
            go_exit: 0,
            kept: &[br#""labels":"urgent""#],
        },
    },
    LossCase {
        name: "control V1 D-remove-by",
        args: &["dep", "remove", "p-t", "p-b", "--json"],
        issues: &[
            br#"{"id":"p-t","title":"tkt-T","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","blockedBy":"p-b"}"#,
            br#"{"id":"p-b","title":"tkt-B","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","blocks":["p-t"]}"#,
            br#"{"id":"p-c","title":"tkt-C","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"id":"p-o","title":"tkt-O","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
        ],
        expect: Loss::Kept {
            go_exit: 0,
            kept: &[br#""blockedBy":"p-b""#],
        },
    },
    LossCase {
        name: "control V1 D-remove-blocks",
        args: &["dep", "remove", "p-t", "p-b", "--json"],
        issues: &[
            br#"{"id":"p-t","title":"tkt-T","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","blockedBy":["p-b"]}"#,
            br#"{"id":"p-b","title":"tkt-B","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","blocks":"p-t"}"#,
            br#"{"id":"p-c","title":"tkt-C","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"id":"p-o","title":"tkt-O","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
        ],
        expect: Loss::Kept {
            go_exit: 0,
            kept: &[br#""blocks":"p-t""#],
        },
    },
    LossCase {
        name: "control V1 U-from",
        args: &["unblock", "p-t", "--from", "p-b", "--json"],
        issues: &[
            br#"{"id":"p-t","title":"tkt-T","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","blockedBy":"p-b"}"#,
            br#"{"id":"p-b","title":"tkt-B","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"id":"p-c","title":"tkt-C","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"id":"p-o","title":"tkt-O","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
        ],
        expect: Loss::Kept {
            go_exit: 1,
            kept: &[br#""blockedBy":"p-b""#],
        },
    },
    LossCase {
        name: "control V1 U-all",
        args: &["unblock", "p-t", "--all", "--json"],
        issues: &[
            br#"{"id":"p-t","title":"tkt-T","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","blockedBy":"p-b"}"#,
            br#"{"id":"p-b","title":"tkt-B","status":"closed","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"id":"p-c","title":"tkt-C","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"id":"p-o","title":"tkt-O","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
        ],
        expect: Loss::Kept {
            go_exit: 0,
            kept: &[br#""blockedBy":"p-b""#],
        },
    },
    LossCase {
        name: "control V1 U-blocks",
        args: &["unblock", "p-t", "--from", "p-b", "--json"],
        issues: &[
            br#"{"id":"p-t","title":"tkt-T","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","blockedBy":["p-b"]}"#,
            br#"{"id":"p-b","title":"tkt-B","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","blocks":"p-t"}"#,
            br#"{"id":"p-c","title":"tkt-C","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"id":"p-o","title":"tkt-O","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
        ],
        expect: Loss::Kept {
            go_exit: 0,
            kept: &[br#""blocks":"p-t""#],
        },
    },
    LossCase {
        name: "control V1 C-dependent",
        args: &["close", "p-b", "--json"],
        issues: &[
            br#"{"id":"p-t","title":"tkt-T","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","blockedBy":"p-b"}"#,
            br#"{"id":"p-b","title":"tkt-B","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","blocks":["p-t"]}"#,
            br#"{"id":"p-c","title":"tkt-C","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"id":"p-o","title":"tkt-O","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
        ],
        expect: Loss::Kept {
            go_exit: 0,
            kept: &[br#""blockedBy":"p-b""#],
        },
    },
    LossCase {
        name: "control V6 L-remove",
        args: &["label", "remove", "p-t", "a", "--json"],
        issues: &[
            b"{\"id\":\"p-t\",\"title\":\"tkt-T\",\"status\":\"open\",\"type\":\"task\",\"priority\":2,\"createdAt\":\"2026-01-01T00:00:00.000Z\",\"updatedAt\":\"2026-01-01T00:00:00.000Z\",\"labels\":[\"\xff\",\"\xfe\"]}",
            br#"{"id":"p-b","title":"tkt-B","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"id":"p-c","title":"tkt-C","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"id":"p-o","title":"tkt-O","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
        ],
        expect: Loss::Kept {
            go_exit: 0,
            kept: &[b"\"labels\":[\"\xff\",\"\xfe\"]"],
        },
    },
    LossCase {
        name: "control label remove zz on [\"a\",5]",
        args: &["label", "remove", "p-t", "zz", "--json"],
        issues: &[
            br#"{"id":"p-t","title":"tkt-T","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","labels":["a",5]}"#,
            br#"{"id":"p-b","title":"tkt-B","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"id":"p-c","title":"tkt-C","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"id":"p-o","title":"tkt-O","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
        ],
        expect: Loss::Kept {
            go_exit: 0,
            kept: &[br#""labels":["a",5]"#],
        },
    },
    LossCase {
        name: "control label add a on [\"a\",5]",
        args: &["label", "add", "p-t", "a", "--json"],
        issues: &[
            br#"{"id":"p-t","title":"tkt-T","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","labels":["a",5]}"#,
            br#"{"id":"p-b","title":"tkt-B","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"id":"p-c","title":"tkt-C","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"id":"p-o","title":"tkt-O","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
        ],
        expect: Loss::Kept {
            go_exit: 0,
            kept: &[br#""labels":["a",5]"#],
        },
    },
    LossCase {
        name: "control V1 Control-label-O",
        args: &["label", "add", "p-o", "zz", "--json"],
        issues: &[
            br#"{"id":"p-t","title":"tkt-T","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","labels":"urgent","blocks":"a","blockedBy":"a"}"#,
            br#"{"id":"p-b","title":"tkt-B","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"id":"p-c","title":"tkt-C","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"id":"p-o","title":"tkt-O","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
        ],
        expect: Loss::Kept {
            go_exit: 0,
            kept: &[br#""labels":"urgent""#, br#""blocks":"a""#, br#""blockedBy":"a""#],
        },
    },
    LossCase {
        name: "control V2 Control-label-O",
        args: &["label", "add", "p-o", "zz", "--json"],
        issues: &[
            br#"{"id":"p-t","title":"tkt-T","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","labels":["a",5],"blocks":["a",5],"blockedBy":["a",5]}"#,
            br#"{"id":"p-b","title":"tkt-B","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"id":"p-c","title":"tkt-C","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"id":"p-o","title":"tkt-O","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
        ],
        expect: Loss::Kept {
            go_exit: 0,
            kept: &[br#""labels":["a",5]"#, br#""blocks":["a",5]"#, br#""blockedBy":["a",5]"#],
        },
    },
    LossCase {
        name: "control V3 Control-label-O",
        args: &["label", "add", "p-o", "zz", "--json"],
        issues: &[
            br#"{"id":"p-t","title":"tkt-T","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","labels":["a",null],"blocks":["a",null],"blockedBy":["a",null]}"#,
            br#"{"id":"p-b","title":"tkt-B","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"id":"p-c","title":"tkt-C","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"id":"p-o","title":"tkt-O","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
        ],
        expect: Loss::Kept {
            go_exit: 0,
            kept: &[br#""labels":["a",null]"#, br#""blocks":["a",null]"#, br#""blockedBy":["a",null]"#],
        },
    },
    LossCase {
        name: "control V4 Control-label-O",
        args: &["label", "add", "p-o", "zz", "--json"],
        issues: &[
            b"{\"id\":\"p-t\",\"title\":\"tkt-T\",\"status\":\"open\",\"type\":\"task\",\"priority\":2,\"createdAt\":\"2026-01-01T00:00:00.000Z\",\"updatedAt\":\"2026-01-01T00:00:00.000Z\",\"labels\":[\"a\",\"\xff\"],\"blocks\":[\"a\",\"\xff\"],\"blockedBy\":[\"a\",\"\xff\"]}",
            br#"{"id":"p-b","title":"tkt-B","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"id":"p-c","title":"tkt-C","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"id":"p-o","title":"tkt-O","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
        ],
        expect: Loss::Kept {
            go_exit: 0,
            kept: &[b"\"labels\":[\"a\",\"\xff\"]", b"\"blocks\":[\"a\",\"\xff\"]", b"\"blockedBy\":[\"a\",\"\xff\"]"],
        },
    },
    LossCase {
        name: "control V5 Control-label-O",
        args: &["label", "add", "p-o", "zz", "--json"],
        issues: &[
            br#"{"id":"p-t","title":"tkt-T","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","labels":["a","\ud800"],"blocks":["a","\ud800"],"blockedBy":["a","\ud800"]}"#,
            br#"{"id":"p-b","title":"tkt-B","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"id":"p-c","title":"tkt-C","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"id":"p-o","title":"tkt-O","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
        ],
        expect: Loss::Kept {
            go_exit: 0,
            kept: &[br#""labels":["a","\ud800"]"#, br#""blocks":["a","\ud800"]"#, br#""blockedBy":["a","\ud800"]"#],
        },
    },
    LossCase {
        name: "control V6 Control-label-O",
        args: &["label", "add", "p-o", "zz", "--json"],
        issues: &[
            b"{\"id\":\"p-t\",\"title\":\"tkt-T\",\"status\":\"open\",\"type\":\"task\",\"priority\":2,\"createdAt\":\"2026-01-01T00:00:00.000Z\",\"updatedAt\":\"2026-01-01T00:00:00.000Z\",\"labels\":[\"\xff\",\"\xfe\"],\"blocks\":[\"\xff\",\"\xfe\"],\"blockedBy\":[\"\xff\",\"\xfe\"]}",
            br#"{"id":"p-b","title":"tkt-B","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"id":"p-c","title":"tkt-C","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"id":"p-o","title":"tkt-O","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
        ],
        expect: Loss::Kept {
            go_exit: 0,
            kept: &[b"\"labels\":[\"\xff\",\"\xfe\"]", b"\"blocks\":[\"\xff\",\"\xfe\"]", b"\"blockedBy\":[\"\xff\",\"\xfe\"]"],
        },
    },
    LossCase {
        name: "control V1 Control-close-T",
        args: &["close", "p-t", "--json"],
        issues: &[
            br#"{"id":"p-t","title":"tkt-T","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","labels":"urgent","blocks":"a","blockedBy":"a"}"#,
            br#"{"id":"p-b","title":"tkt-B","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"id":"p-c","title":"tkt-C","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"id":"p-o","title":"tkt-O","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
        ],
        expect: Loss::Kept {
            go_exit: 0,
            kept: &[br#""labels":"urgent""#, br#""blocks":"a""#, br#""blockedBy":"a""#],
        },
    },
    LossCase {
        name: "control V2 Control-close-T",
        args: &["close", "p-t", "--json"],
        issues: &[
            br#"{"id":"p-t","title":"tkt-T","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","labels":["a",5],"blocks":["a",5],"blockedBy":["a",5]}"#,
            br#"{"id":"p-b","title":"tkt-B","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"id":"p-c","title":"tkt-C","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"id":"p-o","title":"tkt-O","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
        ],
        expect: Loss::Kept {
            go_exit: 0,
            kept: &[br#""labels":["a",5]"#, br#""blocks":["a",5]"#, br#""blockedBy":["a",5]"#],
        },
    },
    LossCase {
        name: "control V3 Control-close-T",
        args: &["close", "p-t", "--json"],
        issues: &[
            br#"{"id":"p-t","title":"tkt-T","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","labels":["a",null],"blocks":["a",null],"blockedBy":["a",null]}"#,
            br#"{"id":"p-b","title":"tkt-B","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"id":"p-c","title":"tkt-C","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"id":"p-o","title":"tkt-O","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
        ],
        expect: Loss::Kept {
            go_exit: 0,
            kept: &[br#""labels":["a",null]"#, br#""blocks":["a",null]"#, br#""blockedBy":["a",null]"#],
        },
    },
    LossCase {
        name: "control V4 Control-close-T",
        args: &["close", "p-t", "--json"],
        issues: &[
            b"{\"id\":\"p-t\",\"title\":\"tkt-T\",\"status\":\"open\",\"type\":\"task\",\"priority\":2,\"createdAt\":\"2026-01-01T00:00:00.000Z\",\"updatedAt\":\"2026-01-01T00:00:00.000Z\",\"labels\":[\"a\",\"\xff\"],\"blocks\":[\"a\",\"\xff\"],\"blockedBy\":[\"a\",\"\xff\"]}",
            br#"{"id":"p-b","title":"tkt-B","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"id":"p-c","title":"tkt-C","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"id":"p-o","title":"tkt-O","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
        ],
        expect: Loss::Kept {
            go_exit: 0,
            kept: &[b"\"labels\":[\"a\",\"\xff\"]", b"\"blocks\":[\"a\",\"\xff\"]", b"\"blockedBy\":[\"a\",\"\xff\"]"],
        },
    },
    LossCase {
        name: "control V5 Control-close-T",
        args: &["close", "p-t", "--json"],
        issues: &[
            br#"{"id":"p-t","title":"tkt-T","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","labels":["a","\ud800"],"blocks":["a","\ud800"],"blockedBy":["a","\ud800"]}"#,
            br#"{"id":"p-b","title":"tkt-B","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"id":"p-c","title":"tkt-C","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"id":"p-o","title":"tkt-O","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
        ],
        expect: Loss::Kept {
            go_exit: 0,
            kept: &[br#""labels":["a","\ud800"]"#, br#""blocks":["a","\ud800"]"#, br#""blockedBy":["a","\ud800"]"#],
        },
    },
    LossCase {
        name: "control V6 Control-close-T",
        args: &["close", "p-t", "--json"],
        issues: &[
            b"{\"id\":\"p-t\",\"title\":\"tkt-T\",\"status\":\"open\",\"type\":\"task\",\"priority\":2,\"createdAt\":\"2026-01-01T00:00:00.000Z\",\"updatedAt\":\"2026-01-01T00:00:00.000Z\",\"labels\":[\"\xff\",\"\xfe\"],\"blocks\":[\"\xff\",\"\xfe\"],\"blockedBy\":[\"\xff\",\"\xfe\"]}",
            br#"{"id":"p-b","title":"tkt-B","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"id":"p-c","title":"tkt-C","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            br#"{"id":"p-o","title":"tkt-O","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
        ],
        expect: Loss::Kept {
            go_exit: 0,
            kept: &[b"\"labels\":[\"\xff\",\"\xfe\"]", b"\"blocks\":[\"\xff\",\"\xfe\"]", b"\"blockedBy\":[\"\xff\",\"\xfe\"]"],
        },
    },
];
