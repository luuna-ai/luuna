//! The command line's contract, against the built client and a mock runtime: parity with the
//! capability catalog (every entry has one command that issues its method and route), connection
//! and refusal sentences that never show a server body, usage errors, the approval acknowledgment
//! (full hash or an 8+ digit prefix), short-id resolution, the SSE follower's resumption, and help
//! that needs no token.
mod support;
use serde_json::{json, Value};
use support::{json, run_client, serve, status, Reply, Request};

const HASH: &str = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const TOKEN: &str = "test-token-0123456789";

fn plan() -> Value {
    json!({"id":"plan-1","revision":1,"ticket_id":"ticket-1","ticket_revision":1,"hash":HASH,"base_commits":{"app":"abc"},"tasks":[{"paths":["src"],"operations":["read","write"]}],"budgets":{"max_attempts":2,"timeout_seconds":600,"cost_usd_micros":1000000}})
}
fn run_record() -> Value {
    json!({"id":"run-1","project_id":"walkthrough","state":"completed","revision":3,"metadata":{"kind":"workflow"},"created_at":"2026-10-21T00:00:00Z","updated_at":"2026-10-21T00:00:00Z"})
}

/// Answers with records every command can use (the original parity test's mock).
fn records(request: &Request) -> Reply {
    let path = request.path.as_str();
    let ends = |suffix: &str| path.ends_with(suffix);
    let parts: Vec<&str> = path.trim_start_matches("/api/v1/").split('/').collect();
    if ends("/stream") {
        let after: u64 = request
            .header("Last-Event-ID")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        let event = json!({"schema_version":"0.1","event_id":"e1","sequence":after + 1,"timestamp":"2026-10-21T00:00:00Z","project_id":"walkthrough","run_id":"run-1","type":"run.completed","payload":{}});
        return Reply {
            status: 200,
            content_type: "text/event-stream",
            body: format!("event: run_event\nid: {}\ndata: {event}\n\n", after + 1),
        };
    }
    if path.contains("/artifacts/") {
        return Reply {
            status: 200,
            content_type: "text/plain",
            body: "Ran 1 test\n\nOK\n".into(),
        };
    }
    match parts.as_slice() {
        ["plans", _] => return json(plan()),
        ["plans", _, "approvals"] => {
            return json(
                json!({"approval":{"plan_id":"plan-1","plan_revision":1,"plan_hash":HASH,"approved_by":"developer"},"run":run_record()}),
            )
        }
        ["runs", _] => return json(run_record()),
        ["runs", _, "candidate"] => {
            return json(
                json!({"evidence":{"candidate":{"revision":1,"hash":HASH,"plan_hash":HASH,"repository_commits":{}},"status":"pending","results":[]},"active":false,"repair_allowed":false,"revalidate_allowed":false}),
            )
        }
        ["projects", _] if request.method == "GET" => {
            return json(
                json!({"project":{"id":"walkthrough","name":"Walkthrough","revision":0},"repositories":[{"repository_id":"app","local_path":"/srv/app","observation":{"identity":{},"dirty":{}}}],"manifest":null,"discovery":null}),
            )
        }
        ["projects", _, "memory", "starter"] => {
            return json(json!({"memory_empty":true,"facts":[
                {"key":"branch:app","repository_id":"app","content":"In app, base every change on `main`.","provenance":"Luuna's starter scan of the confirmed manifest (revision 1), repository app","evidence":["manifest"],"recorded":false,"idempotency_key":"starter-branch:app-0001"},
                {"key":"lockfile:app:Cargo.lock","repository_id":"app","content":"In app, build and test with `--locked`.","provenance":"Luuna's starter scan of repository app: Cargo.lock is present","evidence":["Cargo.lock"],"recorded":false,"idempotency_key":"starter-lockfile:app:Cargo.lock-0002"},
                {"key":"guide:app:README.md","repository_id":"app","content":"In app, read `README.md`.","provenance":"scan","evidence":["README.md"],"recorded":true,"idempotency_key":"starter-guide:app:README.md-0003"}
            ]}))
        }
        ["tickets", _] if request.method == "GET" => {
            return json(
                json!({"id":"ticket-1","revision":1,"source":"luuna_sqlite_temporary","title":"t","intent":"i","acceptance_criteria":[],"repository_ids":["app"]}),
            )
        }
        _ => {}
    }
    if ends("/events") {
        return json(json!([]));
    }
    if ends("/plans") && request.method == "POST" {
        return json(json!({"plan":plan(),"run":run_record()}));
    }
    json(json!({}))
}

fn table() -> Value {
    serde_json::from_str(include_str!("../commands.json")).unwrap()
}
fn catalog() -> Value {
    serde_json::from_str(include_str!("../../../schemas/capability-catalog.json")).unwrap()
}
fn route_matches(route: &str, path: &str) -> bool {
    let a: Vec<&str> = route.split('/').collect();
    let b: Vec<&str> = path.split('/').collect();
    a.len() == b.len() && a.iter().zip(&b).all(|(x, y)| x.starts_with('{') || x == y)
}
fn argv_of(command: &Value) -> Vec<String> {
    let mut argv = vec![command["name"].as_str().unwrap().to_string()];
    argv.extend(
        command["example"]["args"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string()),
    );
    for (flag, value) in command["example"]["flags"].as_object().unwrap() {
        let items = match value {
            Value::Array(items) => items.clone(),
            other => vec![other.clone()],
        };
        for item in items {
            if item == Value::Bool(true) {
                argv.push(format!("--{flag}"));
            } else {
                argv.push(format!("--{flag}"));
                argv.push(
                    item.as_str()
                        .map(str::to_string)
                        .unwrap_or_else(|| item.to_string()),
                );
            }
        }
    }
    argv
}

#[test]
fn every_catalog_entry_has_one_command_that_issues_its_method_and_route() {
    let table = table();
    let catalog = catalog();
    let capabilities = catalog["capabilities"].as_array().unwrap();
    let commands = table["commands"].as_array().unwrap();
    assert_eq!(commands.len(), capabilities.len());
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(directory.path().join("luuna-walkthrough-export.json"), json!({"schema_version":"0.1","kind":"luuna-project-export","export_hash":format!("sha256:{}", "b".repeat(64)),"project":{"id":"walkthrough"}}).to_string()).unwrap();
    for capability in capabilities {
        let command = commands
            .iter()
            .find(|c| c["id"] == capability["id"])
            .unwrap_or_else(|| panic!("no command for {}", capability["id"]));
        assert_eq!(command["name"], capability["tui"]);
        let mock = serve(records);
        let mut argv = argv_of(command);
        argv.push("--json".into());
        let argv: Vec<&str> = argv.iter().map(String::as_str).collect();
        let (code, out, err) = run_client(
            &argv,
            &mock.url(),
            Some(TOKEN),
            &json!({"schema_version":"0.1","id":"walkthrough"}).to_string(),
            directory.path(),
        );
        let name = capability["tui"].as_str().unwrap();
        assert_eq!(code, 0, "{name} failed: {err}");
        let requests = mock.take();
        let method = capability["method"].as_str().unwrap();
        let route = capability["route"].as_str().unwrap();
        assert!(
            requests
                .iter()
                .any(|r| r.method == method && route_matches(route, &r.path)),
            "{name} issued {:?}, not {method} {route}",
            requests
                .iter()
                .map(|r| format!("{} {}", r.method, r.path))
                .collect::<Vec<_>>()
        );
        for request in &requests {
            assert_eq!(
                request.header("Authorization"),
                Some(format!("Bearer {TOKEN}").as_str())
            );
            assert!(!request.query.contains(TOKEN));
        }
        assert!(!out.trim().is_empty(), "{name}: a --json result is printed");
        for line in out.trim().lines() {
            serde_json::from_str::<Value>(line)
                .unwrap_or_else(|_| panic!("{name}: not JSON: {line}"));
        }
    }
}

#[test]
fn bodies_match_the_shared_builders() {
    let directory = tempfile::tempdir().unwrap();
    let mock = serve(records);
    let (code, _, err) = run_client(
        &[
            "register",
            "Walkthrough",
            "app",
            "/srv/walkthrough",
            "--id",
            "walkthrough",
            "--repository",
            "web=/srv/web",
            "--json",
        ],
        &mock.url(),
        Some(TOKEN),
        "",
        directory.path(),
    );
    assert_eq!(code, 0, "{err}");
    assert_eq!(
        mock.take()[0].json(),
        json!({"name":"Walkthrough","id":"walkthrough","repositories":[{"id":"app","path":"/srv/walkthrough"},{"id":"web","path":"/srv/web"}]})
    );
    let (code, _, err) = run_client(
        &[
            "plan",
            "ticket-1",
            "--task",
            "api=src>lib",
            "--task",
            "lib=lib;role=repository_lead",
            "--workers",
            "2",
            "--attempts",
            "3",
            "--key",
            "k4",
        ],
        &mock.url(),
        Some(TOKEN),
        "",
        directory.path(),
    );
    assert_eq!(code, 0, "{err}");
    let posted = mock
        .take()
        .into_iter()
        .find(|r| r.method == "POST")
        .unwrap();
    assert_eq!(
        posted.json(),
        json!({"tasks":[{"id":"api","repository_id":"api","paths":["src"],"operations":["read","write"],"depends_on":["lib"]},{"id":"lib","repository_id":"lib","paths":["lib"],"operations":["read"],"role":"repository_lead"}],"budgets":{"max_attempts":3,"max_workers":2},"idempotency_key":"k4"})
    );
    // Key order is part of the body: it is the order the original builder wrote.
    assert!(posted.body.starts_with(r#"{"tasks":[{"id":"api","repository_id":"api","paths":["src"],"operations":["read","write"],"depends_on":["lib"]}"#), "{}", posted.body);
}

#[test]
fn memory_starter_records_only_the_facts_named_to_keep() {
    let directory = tempfile::tempdir().unwrap();
    let mock = serve(records);
    // Without --keep and without a terminal, the set is only listed.
    let (code, out, err) = run_client(
        &["memory-starter", "walkthrough"],
        &mock.url(),
        Some(TOKEN),
        "",
        directory.path(),
    );
    assert_eq!(code, 0, "{err}");
    assert!(mock.take().iter().all(|r| r.method == "GET"));
    assert!(out.contains("proposed  lockfile:app:Cargo.lock"), "{out}");
    assert!(out.contains("recorded  guide:app:README.md"), "{out}");
    let (code, out, err) = run_client(
        &[
            "memory-starter",
            "walkthrough",
            "--keep",
            "lockfile:app:Cargo.lock",
        ],
        &mock.url(),
        Some(TOKEN),
        "",
        directory.path(),
    );
    assert_eq!(code, 0, "{err}");
    let posted: Vec<_> = mock
        .take()
        .into_iter()
        .filter(|r| r.method == "POST")
        .collect();
    assert_eq!(posted.len(), 1);
    assert_eq!(posted[0].path, "/api/v1/projects/walkthrough/memory");
    assert_eq!(
        posted[0].json(),
        json!({"idempotency_key":"starter-lockfile:app:Cargo.lock-0002","scope":"project","content":"In app, build and test with `--locked`.","provenance":"Luuna's starter scan of repository app: Cargo.lock is present"})
    );
    assert!(out.contains("kept"), "{out}");
    // `all` keeps every fact not yet recorded; an unknown key sends nothing.
    let (code, _, err) = run_client(
        &["memory-starter", "walkthrough", "--keep", "all"],
        &mock.url(),
        Some(TOKEN),
        "",
        directory.path(),
    );
    assert_eq!(code, 0, "{err}");
    assert_eq!(mock.take().iter().filter(|r| r.method == "POST").count(), 2);
    let (code, _, err) = run_client(
        &["memory-starter", "walkthrough", "--keep", "nope"],
        &mock.url(),
        Some(TOKEN),
        "",
        directory.path(),
    );
    assert_ne!(code, 0);
    assert!(err.contains("No proposed fact has the key nope"), "{err}");
    assert!(mock.take().iter().all(|r| r.method == "GET"));
}

#[test]
fn connection_and_refusal_sentences() {
    let directory = tempfile::tempdir().unwrap();
    let dir = directory.path();
    let (code, _, err) = run_client(&["health"], "http://127.0.0.1:1", None, "", dir);
    assert_eq!(code, 2);
    assert!(err.contains("LUUNA_API_TOKEN"));
    let (code, _, err) = run_client(&["health"], "http://127.0.0.1:1", Some(TOKEN), "", dir);
    assert_eq!(code, 1);
    assert!(
        err.starts_with("Connection refused: The runtime did not answer."),
        "{err}"
    );
    let server = serve(|_| Reply {
        status: 500,
        content_type: "text/html",
        body: "<html>stack trace with /secret/path</html>".into(),
    });
    let (code, _, err) = run_client(&["projects"], &server.url(), Some(TOKEN), "", dir);
    assert_eq!(code, 1);
    assert!(
        err.starts_with(
            "Project listing refused: The runtime reported an internal error (HTTP 500)."
        ),
        "{err}"
    );
    assert!(!err.contains("stack trace"));
    let rejected = serve(|_| status(401, ""));
    let (_, _, err) = run_client(&["projects"], &rejected.url(), Some(TOKEN), "", dir);
    assert!(
        err.contains("rejected this token") && err.contains("LUUNA_API_TOKEN"),
        "{err}"
    );
    let busy = serve(|_| {
        status(
            409,
            r#"{"error":"repository app already has an active writer; one writer per repository"}"#,
        )
    });
    let (code, out, err) = run_client(&["implement", "run-1"], &busy.url(), Some(TOKEN), "", dir);
    assert_eq!(code, 1);
    assert!(err.starts_with("Implementation refused: repository app already has an active writer; one writer per repository Another workflow's writer holds this repository."), "{err}");
    assert!(out.is_empty());
    let (_, out, _) = run_client(
        &["implement", "run-1", "--json"],
        &busy.url(),
        Some(TOKEN),
        "",
        dir,
    );
    assert!(serde_json::from_str::<Value>(out.trim()).unwrap()["error"]
        .as_str()
        .unwrap()
        .starts_with("Implementation refused:"));
    let (code, _, err) = run_client(&["health"], "http://example.com", Some(TOKEN), "", dir);
    assert_eq!(code, 1);
    assert!(err.contains("loopback"), "{err}");
}

#[test]
fn approval_needs_the_complete_plan_hash() {
    let directory = tempfile::tempdir().unwrap();
    let mock = serve(records);
    let (code, _, err) = run_client(
        &["approve", "plan-1"],
        &mock.url(),
        Some(TOKEN),
        "",
        directory.path(),
    );
    assert_eq!(code, 2);
    assert!(err.contains(&format!("--acknowledge-hash {HASH}")), "{err}");
    let wrong = format!("sha256:{}", "c".repeat(64));
    let (code, _, err) = run_client(
        &["approve", "plan-1", "--acknowledge-hash", &wrong],
        &mock.url(),
        Some(TOKEN),
        "",
        directory.path(),
    );
    assert_eq!(code, 1);
    assert!(
        err.starts_with("Approval refused: The acknowledged hash does not match plan plan-1 r1"),
        "{err}"
    );
    assert!(
        !mock.take().iter().any(|r| r.path.ends_with("/approvals")),
        "nothing is sent for a mismatched hash"
    );
    let (code, out, err) = run_client(
        &[
            "approve",
            "plan-1",
            "--acknowledge-hash",
            HASH,
            "--approved-by",
            "dev",
        ],
        &mock.url(),
        Some(TOKEN),
        "",
        directory.path(),
    );
    assert_eq!(code, 0, "{err}");
    let sent = mock
        .take()
        .into_iter()
        .find(|r| r.path.ends_with("/approvals"))
        .unwrap();
    assert_eq!(
        sent.json(),
        json!({"plan_revision":1,"plan_hash":HASH,"approved_by":"dev"})
    );
    assert!(out.contains("Approved plan plan-1 r1 by developer"));
}

#[test]
fn usage_errors_and_argument_parsing() {
    let directory = tempfile::tempdir().unwrap();
    let mock = serve(records);
    let run = |argv: &[&str]| run_client(argv, &mock.url(), Some(TOKEN), "", directory.path());
    let (code, _, err) = run(&["artifact", "run-1"]);
    assert_eq!(code, 2);
    assert!(
        err.contains("artifact needs 2 argument(s). Usage: artifact <run> <artifact-id>"),
        "{err}"
    );
    let (code, _, err) = run(&["frobnicate"]);
    assert_eq!(code, 2);
    assert!(err.contains("Unknown command frobnicate"));
    let (code, _, err) = run(&["create-ticket", "walkthrough", "--bogus", "x"]);
    assert_eq!(code, 2);
    assert!(err.contains("Unknown option --bogus"));
    let (code, _, err) = run(&["create-ticket", "walkthrough", "--title"]);
    assert_eq!(code, 2);
    assert!(err.contains("--title needs a value."));
    let (code, _, err) = run(&[
        "create-ticket",
        "walkthrough",
        "--title",
        "T",
        "--intent",
        "I",
        "--criterion",
        "a",
        "--criterion=b",
        "--repository",
        "app",
        "--key",
        "k",
        "--json",
    ]);
    assert_eq!(code, 0, "{err}");
    let body = mock
        .take()
        .into_iter()
        .find(|r| r.method == "POST")
        .unwrap()
        .json();
    assert_eq!(body["acceptance_criteria"], json!(["a", "b"]));
    let (code, out, _) = run(&[]);
    assert_eq!(code, 2);
    assert!(out.contains("luuna-tui <command>"));
}

#[test]
fn follow_resumes_with_last_event_id_and_prints_each_event() {
    let directory = tempfile::tempdir().unwrap();
    let mock = serve(records);
    let (code, out, err) = run_client(
        &["follow", "run-1", "--after", "0", "--json"],
        &mock.url(),
        Some(TOKEN),
        "",
        directory.path(),
    );
    assert_eq!(code, 0, "{err}");
    let lines: Vec<Value> = out
        .trim()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(lines[0]["sequence"], 1);
    assert_eq!(
        lines.last().unwrap(),
        &json!({"run_id":"run-1","state":"completed","last_sequence":1,"events_followed":1})
    );
    let stream = mock
        .take()
        .into_iter()
        .find(|r| r.path.ends_with("/stream"))
        .unwrap();
    assert_eq!(stream.header("Accept"), Some("text/event-stream"));
    let (code, _, _) = run_client(
        &["follow", "run-1", "--after", "7"],
        &mock.url(),
        Some(TOKEN),
        "",
        directory.path(),
    );
    assert_eq!(code, 0);
    assert_eq!(
        mock.take()
            .into_iter()
            .find(|r| r.path.ends_with("/stream"))
            .unwrap()
            .header("Last-Event-ID"),
        Some("7")
    );
}

#[test]
fn help_lists_every_command_once_and_needs_no_token() {
    let directory = tempfile::tempdir().unwrap();
    let dead = "http://127.0.0.1:1";
    let table = table();
    let (code, overview, _) = run_client(&["help"], dead, None, "", directory.path());
    assert_eq!(code, 0);
    for command in table["commands"]
        .as_array()
        .unwrap()
        .iter()
        .chain(table["projections"].as_array().unwrap())
        .chain(table["guides"].as_array().unwrap())
    {
        let name = command["name"].as_str().unwrap();
        let rows = overview
            .lines()
            .filter(|l| {
                // A command row starts at column 2; wrapped descriptions are indented further.
                l.strip_prefix("  ").is_some_and(|r| {
                    !r.starts_with(' ') && r.split_whitespace().next() == Some(name)
                })
            })
            .count();
        assert_eq!(rows, 1, "{name} is listed once");
        let (code, page, _) = run_client(&[name, "--help"], dead, None, "", directory.path());
        assert_eq!(code, 0);
        for flag in command["flags"].as_object().unwrap().keys() {
            assert!(page.contains(&format!("--{flag}")), "{name} --{flag}");
        }
    }
    assert!(overview.contains("luuna-tui missions"));
    let (code, _, err) = run_client(&["aprove"], dead, None, "", directory.path());
    assert_eq!(code, 2);
    assert!(err.contains("Did you mean approve?"));
    let (code, _, err) = run_client(&["init"], dead, Some(TOKEN), "", directory.path());
    assert_eq!(code, 2);
    assert!(
        err.contains("init is a guided setup and needs a terminal"),
        "{err}"
    );
    let (code, out, _) = run_client(&["catalog"], dead, None, "", directory.path());
    assert_eq!(code, 0);
    assert_eq!(serde_json::from_str::<Value>(&out).unwrap(), catalog());
}

/// Orbit (ADR 0036): the typed command travels as an argument array, a failed repository exits 1
/// after printing every repository, and `sbx` shows the plan or refuses an unavailable sandbox.
#[test]
fn orbit_commands_and_the_sandbox_plan() {
    let directory = tempfile::tempdir().unwrap();
    let mock = serve(|request: &Request| {
        let path = request.path.as_str();
        if path.ends_with("/orbit/sandbox") {
            let available = request.query.contains("network=none");
            return json(
                json!({"version":1,"project_id":"luuna","backend":"bwrap","available":available,"reason":if available { Value::Null } else { json!("bwrap missing") },"workdir":"/home/dev/github","host_root":null,"home":"/data/home","repositories":[{"repository_id":"luuna","host_path":"/home/dev/github/luuna","sandbox_path":"/home/dev/github/luuna"}],"network":"none","container_name":null,"notes":[]}),
            );
        }
        json(
            json!({"id":"x","kind":"recipe","command":["test"],"results":[{"repository_id":"app","status":"succeeded","exit_code":0,"reason":null,"stdout_excerpt":"ok","stderr_excerpt":""},{"repository_id":"lib","status":"failed","exit_code":1,"reason":"exit code 1","stdout_excerpt":"1 failed","stderr_excerpt":""}]}),
        )
    });
    let run = |argv: &[&str]| run_client(argv, &mock.url(), Some(TOKEN), "", directory.path());
    let (code, _, err) = run(&[
        "orbit-exec",
        "luuna",
        r#"git commit -m "two words""#,
        "--repository",
        "atlas",
        "--repository",
        "beacon",
        "--parallel",
        "--timeout",
        "60",
    ]);
    assert_eq!(code, 1, "the mock's lib repository failed: {err}");
    let posted = mock.take().remove(0);
    assert_eq!(posted.path, "/api/v1/projects/luuna/orbit/exec");
    assert_eq!(
        posted.body,
        r#"{"executable":"git","args":["commit","-m","two words"],"repositories":["atlas","beacon"],"parallel":true,"timeout_seconds":60}"#
    );
    let (code, _, err) = run(&["orbit-exec", "luuna", "   "]);
    assert_eq!(code, 1);
    assert!(err.contains("Type a command"), "{err}");
    assert!(mock.take().is_empty(), "an empty command is never sent");
    let (code, out, _) = run(&["orbit-run", "p", "test"]);
    assert_eq!(code, 1);
    assert!(
        out.contains("── app  succeeded exit 0\n   ok\n── lib  failed exit 1\n   1 failed"),
        "{out}"
    );
    // The sandboxed command's own flags stay its own, with or without the `--`.
    for argv in [
        &[
            "sbx",
            "luuna",
            "--network",
            "none",
            "--plan-only",
            "--",
            "pi",
            "--model",
            "x",
        ][..],
        &[
            "sbx",
            "luuna",
            "--network",
            "none",
            "--plan-only",
            "pi",
            "--model",
            "x",
        ][..],
    ] {
        let (code, out, err) = run(argv);
        assert_eq!(code, 0, "{err}");
        assert!(
            out.starts_with("sandbox luuna · bwrap · available"),
            "{out}"
        );
        assert!(
            out.contains("open it with: lu sbx luuna --network none"),
            "{out}"
        );
    }
    let (code, _, err) = run(&["sbx", "luuna"]);
    assert_eq!(code, 1);
    assert!(
        err.contains("Project sandbox refused: bwrap missing"),
        "{err}"
    );
    let (code, _, err) = run(&["sbx", "luuna", "--network", "wide"]);
    assert_eq!(code, 2);
    assert!(err.contains("--network takes open or none."), "{err}");
}

#[test]
fn a_trailing_command_is_taken_verbatim() {
    let command = luuna_tui::commands::by_name("sbx").unwrap();
    for argv in [
        vec!["luuna", "--network", "none", "--", "pi", "--model", "x"],
        vec!["luuna", "--network", "none", "pi", "--model", "x"],
    ] {
        let argv: Vec<String> = argv.into_iter().map(String::from).collect();
        let input = luuna_tui::cli::parse_arguments(&argv, Some(command)).unwrap();
        assert_eq!(input.args, ["luuna", "pi", "--model", "x"]);
        assert_eq!(input.flags.len(), 1);
    }
}

const TICKET_A: &str = "ticket-3f2a91c0-1b2c-4d5e-8f90-0123456789ab";
const TICKET_B: &str = "ticket-3f2a0000-1b2c-4d5e-8f90-0123456789ab";
const RUN_NEW: &str = "workflow-8b1e0000-1b2c-4d5e-8f90-0123456789ab";
const RUN_OLD: &str = "workflow-77770000-1b2c-4d5e-8f90-0123456789ab";
const MEMORY: &str = "mem-5e5e0000-1b2c-4d5e-8f90-0123456789ab";

/// A runtime whose records carry generated identities, for short-id resolution (ADR 0042).
fn generated(request: &Request) -> Reply {
    let path = request.path.trim_start_matches("/api/v1");
    let ticket = |id: &str, title: &str| json!({"id":id,"revision":1,"source":"luuna_sqlite_temporary","title":title,"intent":"i","acceptance_criteria":[],"repository_ids":["app"]});
    let run = |id: &str, at: &str| json!({"id":id,"project_id":"walkthrough","state":"completed","revision":1,"metadata":{"kind":"workflow","plan_id":format!("plan-{TICKET_A}"),"plan_revision":1},"created_at":at,"updated_at":at});
    let plan_a = json!({"id":format!("plan-{TICKET_A}"),"revision":1,"ticket_id":TICKET_A,"ticket_revision":1,"hash":HASH,"base_commits":{"app":"abc"},"tasks":[],"budgets":{}});
    match path {
        "/projects" => json(json!([{"id":"walkthrough","name":"Walkthrough","revision":1}])),
        "/projects/walkthrough/tickets" => json(json!([
            ticket(TICKET_A, "Greeting"),
            ticket(TICKET_B, "Farewell")
        ])),
        "/runs" => json(json!([
            run(RUN_OLD, "2026-10-20T00:00:00Z"),
            run(RUN_NEW, "2026-10-21T00:00:00Z")
        ])),
        p if p == format!("/tickets/{TICKET_A}") => {
            let mut record = ticket(TICKET_A, "Greeting");
            record["project_id"] = json!("walkthrough");
            json(record)
        }
        "/projects/walkthrough/memory" => json(json!([
            {"id":MEMORY,"scope":"project","content":"Use structured errors","hash":HASH}
        ])),
        p if p == format!("/runs/{RUN_NEW}") => json(run(RUN_NEW, "2026-10-21T00:00:00Z")),
        p if p == format!("/plans/plan-{TICKET_A}") => json(plan_a),
        p if p == format!("/plans/plan-{TICKET_A}/approvals") => json(
            json!({"approval":{"plan_id":format!("plan-{TICKET_A}"),"plan_revision":1,"plan_hash":HASH,"approved_by":"developer"},"run":run(RUN_NEW, "2026-10-21T00:00:00Z")}),
        ),
        _ => status(404, r#"{"error":"not found"}"#),
    }
}

#[test]
fn short_ids_name_exactly_one_record_before_anything_is_sent() {
    let directory = tempfile::tempdir().unwrap();
    let mock = serve(generated);
    let run = |argv: &[&str]| run_client(argv, &mock.url(), Some(TOKEN), "", directory.path());

    let (code, out, err) = run(&["show-ticket", "3f2a91"]);
    assert_eq!(code, 0, "{err}");
    assert!(
        out.starts_with("3f2a91c0  r1"),
        "lists and records lead with the short id: {out}"
    );
    assert!(mock
        .take()
        .iter()
        .any(|r| r.path == format!("/api/v1/tickets/{TICKET_A}")));

    let (code, _, err) = run(&["show-ticket", "3f2a"]);
    assert_eq!(code, 1);
    assert!(
        err.contains("matches 2 tickets") && err.contains("3f2a91c0 Greeting"),
        "{err}"
    );
    assert!(
        !mock
            .take()
            .iter()
            .any(|r| r.path.starts_with("/api/v1/tickets/")),
        "nothing is read for an ambiguous id"
    );

    let (code, _, err) = run(&["show-ticket", "ticket-9999"]);
    assert_eq!(code, 1);
    assert!(
        err.contains("No ticket has the short id ticket-9999"),
        "{err}"
    );

    let (code, out, err) = run(&["run", "latest"]);
    assert_eq!(code, 0, "{err}");
    assert!(
        out.starts_with("8b1e0000  completed  workflow · plan 3f2a91c0 r1"),
        "{out}"
    );
    assert!(mock
        .take()
        .iter()
        .any(|r| r.path == format!("/api/v1/runs/{RUN_NEW}")));

    let (code, _, err) = run(&["approve", "3f2a91", "--acknowledge-hash", "aaaaaaa"]);
    assert_eq!(code, 1, "seven hash digits do not acknowledge a hash");
    assert!(
        err.starts_with("Approval refused: The acknowledged hash does not match"),
        "{err}"
    );
    assert!(!mock.take().iter().any(|r| r.path.ends_with("/approvals")));

    let (code, out, err) = run(&["approve", "3f2a91", "--acknowledge-hash", "aaaaaaaa"]);
    assert_eq!(code, 0, "{err}");
    assert!(out.starts_with("Approved plan 3f2a91c0 r1"), "{out}");
    let sent = mock
        .take()
        .into_iter()
        .find(|r| r.path.ends_with("/approvals"))
        .unwrap();
    assert_eq!(
        sent.path,
        format!("/api/v1/plans/plan-{TICKET_A}/approvals")
    );
    assert_eq!(
        sent.json()["plan_hash"],
        HASH,
        "the complete hash is what reaches the runtime"
    );

    // A prefix naming another kind of record is refused, not matched against runs.
    let (code, _, err) = run(&["cancel", "ticket-8b1e"]);
    assert_eq!(code, 1);
    assert!(
        err.contains("ticket-8b1e names a ticket, not a run"),
        "{err}"
    );
    assert!(mock.take().is_empty(), "nothing is read or sent for it");

    // The flags that take the ids lists print accept their short ids too.
    let _ = run(&["runs", "--before", "7777"]);
    let listed = mock.take();
    assert!(
        listed
            .iter()
            .any(|r| r.path == "/api/v1/runs" && r.query.contains(&format!("before={RUN_OLD}"))),
        "{:?}",
        listed
            .iter()
            .map(|r| (&r.path, &r.query))
            .collect::<Vec<_>>()
    );
    let _ = run(&["plan", "3f2a91", "--memory", "5e5e"]);
    let planned = mock
        .take()
        .into_iter()
        .find(|r| r.method == "POST")
        .expect("the plan request is sent");
    assert_eq!(
        planned.json()["component_context"]["memory_ids"],
        json!([MEMORY])
    );
}
