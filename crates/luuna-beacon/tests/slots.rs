//! The open slots the later library plans fill: `Step::Plan` and
//! `Done::Plan` in the harness, the per-plan normalisation hooks, and the
//! public error types. None of these tests needs Go or the reference clone.

mod support;

use luuna_beacon::{Beacon, BeaconError, ListFilter};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use support::case::{describe, lower, run_lib, PlanStep, Step};
use support::normalise::sort_map_ordered_arrays;
use support::payload::{self, Done, PlanDone};

/// A canonicalised project root in a temporary folder. Not `fresh_root`,
/// which builds the Go tool.
fn root() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("create a temporary project root");
    let path = dir.path().canonicalize().expect("canonical project root");
    (dir, path)
}

struct Documents {
    documents: Vec<Value>,
    code: i32,
}

impl PlanDone for Documents {
    fn documents(&self) -> Vec<Value> {
        self.documents.clone()
    }

    fn exit_code(&self) -> i32 {
        self.code
    }
}

/// A step that lists the prompts after `init` and reports their count.
struct Stats;

impl PlanStep for Stats {
    fn argv(&self) -> Option<Vec<String>> {
        Some(vec!["stats".to_owned()])
    }

    fn call(&self, beacon: &Beacon) -> Result<Done, BeaconError> {
        let count = beacon.list(&ListFilter::default())?.len();
        Ok(Done::Plan(Box::new(Documents {
            documents: vec![json!({"success": true, "command": "stats", "count": count})],
            code: 0,
        })))
    }

    fn apply_raw(&self, _root: &Path) {
        unreachable!("not a raw step");
    }

    fn describe_raw(&self) -> String {
        unreachable!("not a raw step");
    }
}

struct ExitOne;

impl PlanStep for ExitOne {
    fn argv(&self) -> Option<Vec<String>> {
        Some(vec!["stats".to_owned()])
    }

    fn call(&self, _beacon: &Beacon) -> Result<Done, BeaconError> {
        Ok(Done::Plan(Box::new(Documents {
            documents: vec![json!({"success": false, "command": "stats"})],
            code: 1,
        })))
    }

    fn apply_raw(&self, _root: &Path) {
        unreachable!("not a raw step");
    }

    fn describe_raw(&self) -> String {
        unreachable!("not a raw step");
    }
}

struct Failing;

impl PlanStep for Failing {
    fn argv(&self) -> Option<Vec<String>> {
        Some(vec!["stats".to_owned()])
    }

    fn call(&self, _beacon: &Beacon) -> Result<Done, BeaconError> {
        Err(BeaconError::PromptNotFound("x".to_owned()))
    }

    fn apply_raw(&self, _root: &Path) {
        unreachable!("not a raw step");
    }

    fn describe_raw(&self) -> String {
        unreachable!("not a raw step");
    }
}

struct Raw;

impl PlanStep for Raw {
    fn argv(&self) -> Option<Vec<String>> {
        None
    }

    fn call(&self, _beacon: &Beacon) -> Result<Done, BeaconError> {
        unreachable!("a raw step has no library call");
    }

    fn apply_raw(&self, _root: &Path) {}

    fn describe_raw(&self) -> String {
        "raw plan step".to_owned()
    }
}

#[test]
fn plan_step_runs_through_run_lib() {
    let (_dir, path) = root();
    let beacon = Beacon::open(&path).expect("open");
    beacon.init().expect("init");
    let run = run_lib(&beacon, &Step::Plan(Box::new(Stats)));
    assert_eq!(run.code, 0);
    assert!(run.ok);
    assert_eq!(
        run.documents,
        vec![json!({"success": true, "command": "stats", "count": 0})]
    );
}

#[test]
fn plan_done_exit_code_is_routed() {
    let (_dir, path) = root();
    let beacon = Beacon::open(&path).expect("open");
    let run = run_lib(&beacon, &Step::Plan(Box::new(ExitOne)));
    assert_eq!(run.code, 1);
}

#[test]
fn plan_step_errors_become_error_documents() {
    let (_dir, path) = root();
    let beacon = Beacon::open(&path).expect("open");
    let run = run_lib(&beacon, &Step::Plan(Box::new(Failing)));
    assert_eq!(
        run.documents,
        vec![payload::error_document("stats", "prompt \"x\" not found")]
    );
    assert_eq!(run.code, 1);
}

#[test]
fn raw_plan_step_is_described_by_its_plan() {
    let step = Step::Plan(Box::new(Raw));
    assert!(lower(&step).argv.is_none());
    assert_eq!(describe(&step), "raw plan step");
}

#[test]
fn foundation_documents_still_sort_and_plan_hooks_do_nothing() {
    let mut list = json!({
        "success": true,
        "command": "list",
        "prompts": [{"name": "b"}, {"name": "a"}],
        "count": 2,
    });
    sort_map_ordered_arrays(&mut list, "luuna-beacon");
    assert_eq!(list["prompts"], json!([{"name": "a"}, {"name": "b"}]));

    let stats =
        json!({"success": true, "command": "stats", "items": [{"name": "b"}, {"name": "a"}]});
    let mut sorted = stats.clone();
    sort_map_ordered_arrays(&mut sorted, "luuna-beacon");
    assert_eq!(sorted, stats);

    let error = payload::error_document("schema", "schema \"x\" not found");
    let mut sorted = error.clone();
    sort_map_ordered_arrays(&mut sorted, "luuna-beacon");
    assert_eq!(sorted, error);
}

#[test]
fn plan_error_types_are_public() {
    fn takes(
        _authoring: Option<luuna_beacon::AuthoringError>,
        _artifacts: Option<luuna_beacon::ArtifactsError>,
    ) {
    }
    takes(None, None);
}
