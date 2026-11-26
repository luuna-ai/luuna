//! `lu init`'s flow (crate::prime) against a mock runtime, with scripted answers and a fake
//! component runner: the cataloged actions it runs and their bodies, retry/skip/leave after a
//! refusal, the agent onboarding only as chosen, and the frames the workbench shows.
mod support;
use luuna_tui::client::{Client, Controller};
use luuna_tui::commands::Fut;
use luuna_tui::components::{Component, Outcome, Runner, COMPONENTS};
use luuna_tui::failures::Failure;
use luuna_tui::prime::{prime, GuideUi};
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use support::{json, serve, status, Mock, Reply, Request};

const HASH: &str = "sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd";
const ROOT: &str = if cfg!(windows) {
    r"C:\work\demo"
} else {
    "/work/demo"
};

#[derive(Clone)]
enum Answer {
    Text(&'static str),
    Default,
    Pick(&'static str),
    Marks(Vec<bool>),
    Leave,
}

struct Scripted {
    answers: Mutex<VecDeque<Answer>>,
    frames: Mutex<Vec<Value>>,
    signal: luuna_tui::client::Signal,
    leave: Controller,
}

impl Scripted {
    fn new(answers: Vec<Answer>) -> Arc<Scripted> {
        let leave = Controller::default();
        Arc::new(Scripted {
            answers: Mutex::new(answers.into()),
            frames: Mutex::new(Vec::new()),
            signal: leave.signal(),
            leave,
        })
    }
    fn lines(&self) -> String {
        let strip = |t: &str| {
            let mut out = String::new();
            let mut skipping = false;
            for c in t.chars() {
                match c {
                    '\u{E000}' => skipping = true,
                    '\u{E001}' => skipping = false,
                    '\u{E002}' => {}
                    c if !skipping => out.push(c),
                    _ => {}
                }
            }
            out
        };
        self.frames
            .lock()
            .unwrap()
            .iter()
            .filter_map(|f| f["text"].as_str().map(strip))
            .collect::<Vec<_>>()
            .join("\n")
    }
    fn panels(&self) -> Vec<String> {
        self.frames
            .lock()
            .unwrap()
            .iter()
            .filter(|f| f["ui"] == "panel")
            .map(|f| f["title"].as_str().unwrap().to_string())
            .collect()
    }
}

impl GuideUi for Scripted {
    fn send(&self, frame: Value) {
        self.frames.lock().unwrap().push(frame);
    }
    fn ask(&self, kind: &'static str, frame: Value) -> Fut<'_, Result<Value, Failure>> {
        Box::pin(async move {
            let answer = self
                .answers
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| panic!("no scripted answer for {frame}"));
            self.frames.lock().unwrap().push(frame.clone());
            Ok(match (kind, answer) {
                (_, Answer::Leave) => {
                    self.leave.abort();
                    return Err(Failure::Aborted);
                }
                ("text", Answer::Text(t)) => json!(t),
                ("text", Answer::Default) => json!(""),
                ("choice", Answer::Pick(label)) => {
                    let options = frame["options"].as_array().unwrap();
                    json!(options
                        .iter()
                        .position(|o| o["label"].as_str().unwrap().contains(label))
                        .unwrap_or_else(|| panic!("{label} not offered in {frame}")))
                }
                ("checklist", Answer::Marks(marks)) => json!(marks),
                ("checklist", Answer::Default) => json!(frame["options"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|o| o["checked"].as_bool().unwrap())
                    .collect::<Vec<_>>()),
                (kind, _) => panic!("scripted answer does not fit a {kind} question: {frame}"),
            })
        })
    }
}

/// Component CLIs that record each run and answer like the real ones.
struct FakeComponents {
    installed: bool,
    runs: Mutex<Vec<String>>,
}

impl Runner for FakeComponents {
    fn available(&self) -> Vec<Component> {
        if self.installed {
            COMPONENTS.to_vec()
        } else {
            Vec::new()
        }
    }
    fn run<'a>(&'a self, name: &'a str, args: Vec<String>, cwd: &'a str) -> Fut<'a, Outcome> {
        Box::pin(async move {
            assert_eq!(cwd, ROOT, "components run in the checkout");
            self.runs
                .lock()
                .unwrap()
                .push(format!("{name} {}", args.join(" ")));
            let stdout = match args[0].as_str() {
                "onboard" => format!("<!-- luuna-{name}:start -->\n## {name}\nRun luuna-{name} prime.\n<!-- luuna-{name}:end -->\n"),
                "prime" => format!("# {name} quick reference\n"),
                _ => String::new(),
            };
            Outcome {
                code: 0,
                stdout,
                stderr: String::new(),
            }
        })
    }
}

/// A runtime with one project to register; confirmation can be refused once.
fn runtime(refuse_confirm_once: bool) -> Mock {
    let state = Arc::new(Mutex::new((false, refuse_confirm_once, 0)));
    serve(move |request: &Request| -> Reply {
        let mut state = state.lock().unwrap();
        let view = json!({"project":{"id":"demo","name":"demo","revision": if state.0 {1} else {0}},"repositories":[{"project_id":"demo","repository_id":"demo","local_path":ROOT,"observation":{"branch":"main","head_commit":"abc123","identity":{"remote":"git@example.com:demo.git"},"dirty":{"staged":0,"unstaged":0,"untracked":0}}}],"manifest": if state.0 { json!({"revision":1,"hash":HASH}) } else { Value::Null },"discovery":null});
        let manifest = json!({"schema_version":"1","id":"demo","name":"demo","revision":1,"repositories":[{"id":"demo","remote":"git@example.com:demo.git","default_branch":"main","profiles":["node"],"execution":"native","checks":[{"id":"unit","executable":"npm","args":["test"],"required":true,"timeout_seconds":600,"capabilities":[]}]}],"relationships":[],"policy":{"approval":"plan","publish":"automatic_pr","merge":"human"}});
        match (request.method.as_str(), request.path.as_str()) {
            ("GET", "/api/v1/health") => {
                json(json!({"status":"ok","pi_version":"1.3.0","native_execution":true}))
            }
            ("GET", "/api/v1/projects") => json(json!([])),
            ("POST", "/api/v1/projects") | ("GET", "/api/v1/projects/demo") => json(view),
            ("POST", "/api/v1/projects/demo/discover") => json(
                json!({"id":"d1","repositories":[{"repository_id":"demo","profiles":["node"],"proposed_checks":[{"check":manifest["repositories"][0]["checks"][0],"confidence":"observed","evidence":["package.json"]}]}],"proposed_manifest":manifest,"manifest_problems":[],"questions":[]}),
            ),
            ("PUT", "/api/v1/projects/demo/manifest") => {
                if state.1 {
                    state.1 = false;
                    return status(409, r#"{"error":"manifest revision changed"}"#);
                }
                state.0 = true;
                json(
                    json!({"project":{"id":"demo","name":"demo","revision":1},"repositories":view["repositories"],"manifest":{"revision":1,"hash":HASH}}),
                )
            }
            ("POST", "/api/v1/projects/demo/baselines") => {
                json(json!({"id":"run-b","state":"queued"}))
            }
            ("GET", "/api/v1/runs/run-b") => {
                state.2 += 1;
                json(json!({"id":"run-b","state": if state.2 < 2 {"running"} else {"completed"}}))
            }
            ("GET", "/api/v1/projects/demo/baselines") => json(
                json!([{"run_id":"run-b","status":"passed","manifest_revision":1,"required_checks":["unit"],"results":[{"recipe_id":"unit","status":"passed","exit_code":0,"tests_detected":4}]}]),
            ),
            ("GET", "/api/v1/projects/demo/components") => json(json!({"enabled":false})),
            ("POST", "/api/v1/projects/demo/components") => {
                json(json!({"enabled":true,"workspace_path":"/data/components/projects/demo"}))
            }
            ("POST", "/api/v1/projects/demo/memory") => {
                json(json!({"id":"mem-1","hash":HASH,"scope":"project"}))
            }
            ("POST", "/api/v1/projects/demo/tickets") => {
                json(json!({"id":"ticket-1","revision":1,"source":"missions"}))
            }
            other => panic!("unexpected request {other:?}"),
        }
    })
}

async fn run_guide(
    mock: &Mock,
    ui: &Arc<Scripted>,
    components: &FakeComponents,
) -> luuna_tui::prime::Outcome {
    let client = Client::new(&mock.url(), "guide-test-token").unwrap();
    let flags = vec![("path".to_string(), ROOT.to_string())];
    prime(
        &client,
        ui.as_ref(),
        components,
        &flags,
        std::path::Path::new(ROOT),
        ui.signal.clone(),
        "lu",
    )
    .await
    .unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn the_guide_runs_every_step_with_the_commands_bodies() {
    let mock = runtime(true);
    let components = FakeComponents {
        installed: true,
        runs: Mutex::new(Vec::new()),
    };
    let ui = Scripted::new(vec![
        Answer::Pick("This checkout"),
        Answer::Default,
        Answer::Default,
        Answer::Pick("No, one"),
        Answer::Pick("Review"),
        Answer::Pick("Confirm"),
        Answer::Pick("Try again"),
        Answer::Pick("Run it now"),
        Answer::Pick("Enable"),
        Answer::Pick("Record"),
        Answer::Text("Tests run with npm test"),
        Answer::Default,
        Answer::Marks(vec![true, true, false, true]),
        Answer::Pick("Write the sections"),
        Answer::Pick("Create a ticket"),
        Answer::Text("Greet"),
        Answer::Text("Print the greeting"),
        Answer::Text("It prints hello"),
        Answer::Default,
    ]);
    let outcome = run_guide(&mock, &ui, &components).await;
    assert_eq!(outcome.code, 0, "{}", ui.lines());
    assert_eq!(outcome.project.as_deref(), Some("demo"));
    assert_eq!(outcome.ticket.as_deref(), Some("ticket-1"));
    let requests = mock.take();
    let body = |method: &str, path: &str| {
        requests
            .iter()
            .find(|r| r.method == method && r.path == path)
            .unwrap()
            .json()
    };
    assert_eq!(
        body("POST", "/api/v1/projects"),
        json!({"name":"demo","repositories":[{"id":"demo","path":ROOT}]})
    );
    assert_eq!(
        requests.iter().filter(|r| r.method == "PUT").count(),
        2,
        "the refused confirmation was retried"
    );
    assert_eq!(
        body("POST", "/api/v1/projects/demo/baselines"),
        json!({"repository_id":"demo"})
    );
    let memory = body("POST", "/api/v1/projects/demo/memory");
    assert_eq!(
        (
            memory["scope"].clone(),
            memory["content"].clone(),
            memory["provenance"].clone()
        ),
        (
            json!("project"),
            json!("Tests run with npm test"),
            json!("the developer, during setup")
        )
    );
    let ticket = body("POST", "/api/v1/projects/demo/tickets");
    assert_eq!(
        (
            ticket["title"].clone(),
            ticket["acceptance_criteria"].clone(),
            ticket["repository_ids"].clone()
        ),
        (json!("Greet"), json!(["It prints hello"]), json!(["demo"]))
    );
    assert_eq!(
        *components.runs.lock().unwrap(),
        [
            "missions init",
            "atlas init",
            "beacon init",
            "missions onboard --stdout",
            "atlas onboard --stdout",
            "beacon onboard --stdout",
            "missions onboard",
            "atlas onboard",
            "beacon onboard",
            "missions prime --compact",
            "atlas prime",
            "beacon prime --compact"
        ]
    );
    let lines = ui.lines();
    for expected in [
        "Manifest confirmation refused: manifest revision changed",
        "demo baseline passed",
        "not a sandbox",
        "uncommitted changes",
    ] {
        assert!(lines.contains(expected), "missing {expected:?} in\n{lines}");
    }
    assert!(ui.panels().contains(&"Sections for CLAUDE.md".to_string()));
    assert!(ui
        .panels()
        .contains(&"Missions primes an agent with".to_string()));
    let finish = ui
        .frames
        .lock()
        .unwrap()
        .iter()
        .find(|f| f["ui"] == "finish")
        .cloned()
        .unwrap();
    assert_eq!(
        finish["states"],
        json!(["done", "done", "done", "done", "done", "done", "done", "done"])
    );
    assert_eq!(
        finish["next"][0],
        json!([
            "lu plan ticket-1",
            "record a plan, then approve it by its exact hash"
        ])
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn skipping_runs_nothing_optional_and_leaving_returns_130() {
    let mock = runtime(false);
    let components = FakeComponents {
        installed: true,
        runs: Mutex::new(Vec::new()),
    };
    let ui = Scripted::new(vec![
        Answer::Pick("This checkout"),
        Answer::Default,
        Answer::Default,
        Answer::Pick("No, one"),
        Answer::Pick("Skip"),
        Answer::Pick("Skip"),
        Answer::Marks(vec![false, false, false, false]),
        Answer::Pick("Skip"),
    ]);
    let outcome = run_guide(&mock, &ui, &components).await;
    assert_eq!(outcome.code, 0, "{}", ui.lines());
    assert!(
        components.runs.lock().unwrap().is_empty(),
        "nothing runs in the checkout unless chosen"
    );
    assert!(!mock.take().iter().any(|r| r.method == "PUT"
        || r.path.ends_with("/baselines")
        || (r.method == "POST" && r.path.ends_with("/components"))));
    assert!(ui.lines().contains("A baseline needs a confirmed manifest"));
    let leave = Scripted::new(vec![
        Answer::Pick("This checkout"),
        Answer::Default,
        Answer::Default,
        Answer::Leave,
    ]);
    let outcome = run_guide(&runtime(false), &leave, &components).await;
    assert_eq!(outcome.code, 130);
    assert!(leave.lines().contains("Nothing was undone"));
}

#[tokio::test(flavor = "multi_thread")]
async fn missing_component_clis_skip_the_agents_step_with_the_install_hint() {
    let mock = runtime(false);
    let components = FakeComponents {
        installed: false,
        runs: Mutex::new(Vec::new()),
    };
    let ui = Scripted::new(vec![
        Answer::Pick("This checkout"),
        Answer::Default,
        Answer::Default,
        Answer::Pick("No, one"),
        Answer::Pick("Skip"),
        Answer::Pick("Skip"),
        Answer::Pick("Skip"),
    ]);
    let outcome = run_guide(&mock, &ui, &components).await;
    assert_eq!(outcome.code, 0);
    assert!(ui.lines().contains("install-cli.ps1"));
    let finish = ui
        .frames
        .lock()
        .unwrap()
        .iter()
        .find(|f| f["ui"] == "finish")
        .cloned()
        .unwrap();
    assert_eq!(finish["states"][6], "skipped");
}
