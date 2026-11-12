//! Parity with the Go `luuna-missions` tool at the reference commit.
//!
//! The ignored tests need a clone of luuna-ai/luuna-missions at
//! 2fbbf8e5b6a238ea47543e1dad112fe1716faec4 and a Go toolchain:
//!
//! ```text
//! LUUNA_MISSIONS_REFERENCE=<clone> LUUNA_MISSIONS_GO=<go> \
//! LUUNA_MISSIONS_GOPROXY=<proxy list> \
//! cargo test --locked -p luuna-missions --test parity -- --ignored
//! ```
//!
//! Each fixture is replayed through Go and through Rust in separate
//! temporary projects. Go must match the fixture, Rust must match the
//! fixture, and Rust must match Go byte for byte after normalisation. Each
//! test writes `target/tmp/missions-parity/report-<test>.json`.

mod support;

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde_json::{json, Value};

use support::fixtures::{self, DEFERRED, IN_SCOPE, IN_SCOPE_COUNT};
use support::reference::{self, check_reference, FIXTURE_TOTAL, REFERENCE_COMMIT};
use support::{
    child_env, commit_all, git_read, guarded, run_differences, run_go, run_rust, system_temp_dir,
    tree_differences, workspace_root, Normaliser, TempProject,
};

#[test]
#[ignore = "requires LUUNA_MISSIONS_REFERENCE (the Go reference clone)"]
fn fixture_scope_is_complete() {
    let clone = reference::reference_from_env();
    guarded("fixture_scope_is_complete", &clone, |report| {
        let files = fixtures::testdata_files(&clone);
        if files.len() != FIXTURE_TOTAL {
            report.fail(format!(
                "expected {FIXTURE_TOTAL} fixtures in {}, found {}",
                clone.testdata().display(),
                files.len()
            ));
        }
        let mut owners: BTreeMap<&str, Vec<String>> = BTreeMap::new();
        let mut in_scope = 0;
        let mut tests = BTreeSet::new();
        for group in IN_SCOPE {
            if !group.test.starts_with("parity_") || !tests.insert(group.test) {
                report.fail(format!(
                    "group test name {} is not a unique parity_* name",
                    group.test
                ));
            }
            in_scope += group.files.len();
            for file in group.files {
                owners.entry(file).or_default().push(group.test.to_owned());
            }
        }
        if in_scope != IN_SCOPE_COUNT {
            report.fail(format!(
                "{in_scope} fixtures in scope, expected {IN_SCOPE_COUNT}"
            ));
        }
        for (plan, list, expected) in DEFERRED {
            if list.len() != *expected {
                report.fail(format!(
                    "{} fixtures deferred to {plan}, expected {expected}",
                    list.len()
                ));
            }
            for file in *list {
                owners.entry(file).or_default().push((*plan).to_owned());
            }
        }
        let total: usize = in_scope
            + DEFERRED
                .iter()
                .map(|(_, list, _)| list.len())
                .sum::<usize>();
        if total != FIXTURE_TOTAL {
            report.fail(format!(
                "the lists name {total} fixtures, expected {FIXTURE_TOTAL}"
            ));
        }
        for file in &files {
            match owners.get(file.as_str()).map(Vec::len) {
                Some(1) => {}
                Some(_) => report.fail(format!(
                    "{file} is in more than one list: {:?}",
                    owners[file.as_str()]
                )),
                None => report.fail(format!("{file} is in no list")),
            }
        }
        for file in owners.keys() {
            if !files.iter().any(|name| name == file) {
                report.fail(format!("{file} is listed but not in the reference"));
            }
        }
        for file in &files {
            fixtures::load(&clone, file);
        }
        for group in IN_SCOPE {
            for file in group.files {
                let fixture = fixtures::load(&clone, file);
                let command = fixture.args.iter().find(|arg| !arg.starts_with('-'));
                if command.map(String::as_str) != Some(group.command) {
                    report.fail(format!(
                        "{file} runs {command:?}, but its group {} is for {}",
                        group.test, group.command
                    ));
                }
            }
        }
        report.set(
            "counts",
            json!({
                "total": files.len(),
                "in_scope": in_scope,
                "deferred": DEFERRED
                    .iter()
                    .map(|(plan, list, _)| ((*plan).to_owned(), Value::from(list.len())))
                    .collect::<serde_json::Map<_, _>>(),
            }),
        );
    });
}

#[test]
#[ignore = "requires LUUNA_MISSIONS_REFERENCE and LUUNA_MISSIONS_GO"]
fn parity_init() {
    support::replay_group("parity_init");
}

#[test]
#[ignore = "requires LUUNA_MISSIONS_REFERENCE and LUUNA_MISSIONS_GO"]
fn parity_create() {
    support::replay_group("parity_create");
}

#[test]
#[ignore = "requires LUUNA_MISSIONS_REFERENCE and LUUNA_MISSIONS_GO"]
fn parity_update() {
    support::replay_group("parity_update");
}

#[test]
#[ignore = "requires LUUNA_MISSIONS_REFERENCE and LUUNA_MISSIONS_GO"]
fn parity_block() {
    support::replay_group("parity_block");
}

#[test]
#[ignore = "requires LUUNA_MISSIONS_REFERENCE and LUUNA_MISSIONS_GO"]
fn parity_show() {
    support::replay_group("parity_show");
}

#[test]
#[ignore = "requires LUUNA_MISSIONS_REFERENCE and LUUNA_MISSIONS_GO"]
fn parity_list() {
    support::replay_group("parity_list");
}

#[test]
#[ignore = "requires LUUNA_MISSIONS_REFERENCE and LUUNA_MISSIONS_GO"]
fn parity_search() {
    support::replay_group("parity_search");
}

/// One call luuna-runtime makes today (crates/luuna-runtime/src/missions.rs).
/// `{a}` and `{b}` stand for the ids that the create steps labelled `a` and
/// `b` returned.
struct Shape {
    args: Vec<String>,
    creates: Option<&'static str>,
}

fn shape(args: &[&str]) -> Shape {
    Shape {
        args: support::args(args),
        creates: None,
    }
}

fn creating(label: &'static str, args: &[&str]) -> Shape {
    Shape {
        args: support::args(args),
        creates: Some(label),
    }
}

/// The Luuna intent envelope the runtime writes as a description, with
/// `<`, `>`, `&` and quotes.
fn envelope(ticket: &str, intent: &str) -> String {
    json!({
        "luuna_ticket_schema": 1,
        "ticket_id": ticket,
        "project_id": "prj-parity",
        "intent": intent,
        "acceptance_criteria": ["a < b", "c > d & \"e\""],
        "repository_ids": ["luuna"],
    })
    .to_string()
}

fn runtime_shapes() -> Vec<Shape> {
    let label = "--label=luuna-ticket-tkt-parity";
    let first = format!(
        "--description={}",
        envelope("tkt-parity", "Ship <b>parity</b> & \"keep\" it")
    );
    let second = format!(
        "--description={}",
        envelope("tkt-parity", "Second <i>ticket</i> & more")
    );
    let edited = format!(
        "--description={}",
        envelope("tkt-parity", "Edited <intent> & \"quotes\"")
    );
    vec![
        shape(&["init", "--json"]),
        creating(
            "a",
            &[
                "create",
                "--title=Runtime ticket A",
                &first,
                "--labels=luuna-ticket-tkt-parity",
                "--json",
            ],
        ),
        creating(
            "b",
            &[
                "create",
                "--title=Runtime ticket B",
                &second,
                "--labels=luuna-ticket-tkt-parity",
                "--json",
            ],
        ),
        shape(&[
            "list",
            label,
            "--all",
            "--limit=10001",
            "--sort=id",
            "--json",
        ]),
        shape(&[
            "search",
            "tkt-parity",
            "--limit=10001",
            "--sort=id",
            "--json",
        ]),
        shape(&["show", "{a}", "--json"]),
        shape(&[
            "update",
            "{a}",
            "--title=Runtime ticket A edited",
            &edited,
            "--json",
        ]),
        shape(&["update", "{b}", "--status=closed", "--json"]),
        shape(&["search", "{b}", "--limit=10001", "--sort=id", "--json"]),
        shape(&[
            "search",
            "tkt-parity",
            "--limit=10001",
            "--sort=id",
            "--json",
        ]),
        shape(&[
            "list",
            label,
            "--all",
            "--limit=10001",
            "--sort=id",
            "--json",
        ]),
        shape(&["update", "{a}", "--set-labels=", "--json"]),
        shape(&["update", "{a}", "--description=", "--json"]),
        shape(&["block", "{a}", "--by", "{b}", "--json"]),
        shape(&["show", "{a}", "--json"]),
        shape(&["show", "{b}", "--json"]),
        shape(&["list", "--all", "--limit=10001", "--sort=id", "--json"]),
    ]
}

fn created_id(stdout: &[u8]) -> Option<String> {
    support::parse_json(stdout)?
        .get("id")?
        .as_str()
        .map(str::to_owned)
}

/// Runs the runtime's argument shapes on one store: each step starts from the
/// store Go left after the previous step, copied into one project for Go and
/// one for Rust, and the two results are compared byte for byte after
/// normalisation (root, ids, timestamps), including the whole project tree.
#[test]
#[ignore = "requires LUUNA_MISSIONS_REFERENCE and LUUNA_MISSIONS_GO"]
fn runtime_argument_shapes_match_go() {
    let clone = reference::reference_from_env();
    guarded("runtime_argument_shapes_match_go", &clone, |report| {
        let go = reference::go_tool(&clone);
        report.record_go(go);
        let env = child_env(None);
        let mut base = TempProject::new();
        let mut ids: Vec<(String, String)> = Vec::new();
        for (index, step) in runtime_shapes().into_iter().enumerate() {
            let args: Vec<String> = step
                .args
                .iter()
                .map(|arg| {
                    ids.iter().fold(arg.clone(), |arg, (label, id)| {
                        arg.replace(&format!("{{{label}}}"), id)
                    })
                })
                .collect();
            let case = format!("step {}: {}", index + 1, step.args.join(" "));
            let go_project = TempProject::new();
            go_project.copy_tree_from(&base);
            let rust_project = TempProject::new();
            rust_project.copy_tree_from(&base);
            let go_run = run_go(go.binary(), go_project.root(), &args, b"", &env);
            let rust_run = run_rust(rust_project.root(), &args, b"", &env);
            let mut differences = Vec::new();
            let mut detail = serde_json::Map::new();
            detail.insert("args".to_owned(), Value::from(args.clone()));
            let go_run = match go_run {
                Ok(run) => run,
                Err(error) => {
                    report.case(&case, detail, vec![format!("go: {error}")]);
                    return;
                }
            };
            let go_success = support::parse_json(&go_run.stdout)
                .and_then(|value| value.get("success").and_then(Value::as_bool));
            if go_run.exit != 0 || go_success != Some(true) {
                differences.push(format!(
                    "go: the runtime's shape failed (exit {}): {}{}",
                    go_run.exit,
                    support::excerpt(&go_run.stdout),
                    support::excerpt(&go_run.stderr)
                ));
            }
            let mut go_ids: Vec<(String, String, String)> = ids
                .iter()
                .map(|(l, id)| (l.clone(), id.clone(), id.clone()))
                .collect();
            let mut rust_ids = go_ids.clone();
            let go_created = step.creates.and_then(|_| created_id(&go_run.stdout));
            if let (Some(label), Some(id)) = (step.creates, &go_created) {
                go_ids.push((label.to_owned(), id.clone(), id.clone()));
            }
            match &rust_run {
                Ok(rust_run) => {
                    if let Some(label) = step.creates {
                        if let Some(id) = created_id(&rust_run.stdout) {
                            rust_ids.push((label.to_owned(), id.clone(), id));
                        }
                    }
                    let go_norm = Normaliser::new()
                        .replace(go_project.root().to_string_lossy().as_bytes(), "<PROJECT>")
                        .ids(go_ids);
                    let rust_norm = Normaliser::new()
                        .replace(
                            rust_project.root().to_string_lossy().as_bytes(),
                            "<PROJECT>",
                        )
                        .ids(rust_ids);
                    differences.extend(run_differences(&go_run, &go_norm, rust_run, &rust_norm));
                    differences.extend(tree_differences(
                        &go_project.snapshot(),
                        &go_norm,
                        &rust_project.snapshot(),
                        &rust_norm,
                    ));
                }
                Err(error) => differences.push(format!("rust: {error}")),
            }
            report.case(&case, detail, differences);
            if let (Some(label), Some(id)) = (step.creates, go_created) {
                ids.push((label.to_owned(), id));
            }
            base = go_project;
        }
    });
}

/// The reference check fails hard on a missing path, a folder that is not a
/// git clone, and a clone at another commit. The commit it needs is made in
/// a temporary repository this test creates.
#[test]
fn reference_check_rejects_missing_or_wrong_clone() {
    let holder = tempfile::tempdir_in(system_temp_dir()).expect("temporary directory");

    let missing = holder.path().join("missing");
    let error = check_reference(&missing).expect_err("a missing path is rejected");
    assert!(error.contains("does not exist"), "{error}");

    let plain = holder.path().join("plain");
    std::fs::create_dir(&plain).expect("plain folder");
    std::fs::write(plain.join("README.md"), "not a clone\n").expect("file");
    let error = check_reference(&plain).expect_err("a non-git folder is rejected");
    assert!(error.contains("not a git clone"), "{error}");

    let empty = TempProject::new();
    let error = check_reference(empty.root()).expect_err("a clone without commits is rejected");
    assert!(error.contains("not a usable git clone"), "{error}");

    let other = TempProject::new();
    other.write_files([("internal/regression/testdata/001.json", b"{}".to_vec())]);
    commit_all(other.root(), "unrelated history");
    let head = git_read(other.root(), &["rev-parse", "HEAD"]).expect("HEAD");
    assert_ne!(head.trim(), REFERENCE_COMMIT);
    let error = check_reference(other.root()).expect_err("another commit is rejected");
    assert!(error.contains("not at the reference commit"), "{error}");
    assert!(error.contains(head.trim()), "{error}");
}

/// Test projects are created under the system temp folder, outside the luuna
/// worktree and the reference clone, and each is its own git repository.
#[test]
fn temp_projects_live_outside_every_repository() {
    let project = TempProject::new();
    let root = project.root();
    assert!(root.is_absolute());
    assert!(
        root.starts_with(system_temp_dir()),
        "{} is not under {}",
        root.display(),
        system_temp_dir().display()
    );
    let worktree = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("luuna worktree");
    assert_eq!(worktree, workspace_root());
    assert!(
        !root.starts_with(&worktree),
        "{} is inside {}",
        root.display(),
        worktree.display()
    );
    if let Some(clone) = reference::configured_path().and_then(|path| path.canonicalize().ok()) {
        assert!(
            !root.starts_with(&clone),
            "{} is inside {}",
            root.display(),
            clone.display()
        );
    }
    let toplevel = git_read(root, &["rev-parse", "--show-toplevel"]).expect("git toplevel");
    assert_eq!(Path::new(toplevel.trim()), root);
    assert!(
        project.snapshot().is_empty(),
        "a new project holds only .git"
    );
}
