//! The workbench's in-process command service (crate::bridge) against a mock runtime: the
//! catalog, reads and their formatting, the approval hash prompt and its typed replies, refusal
//! outcomes, request limits, guide validation, and token redaction in every event.
mod support;
use luuna_tui::bridge::Bridge;
use luuna_tui::protocol::{BridgeEvent, ClientCommand};
use serde_json::{json, Value};
use std::time::Duration;
use support::{json, serve, status, Request};

const HASH: &str = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const TOKEN: &str = "bridge-test-token-0123456789";

fn plan() -> Value {
    json!({"id":"plan-1","revision":1,"ticket_id":"ticket-1","ticket_revision":1,"hash":HASH,"base_commits":{"app":"abc"},"tasks":[{"paths":["src"],"operations":["read","write"]}],"budgets":{"max_attempts":2,"timeout_seconds":600,"cost_usd_micros":1000000}})
}

fn mock() -> support::Mock {
    serve(
        |request: &Request| match (request.method.as_str(), request.path.as_str()) {
            ("GET", "/api/v1/health") => {
                json(json!({"status":"ok","pi_version":"1.3.0","build_identity":TOKEN}))
            }
            ("GET", "/api/v1/plans/plan-1") => json(plan()),
            ("POST", "/api/v1/plans/plan-1/approvals") => json(
                json!({"approval":{"plan_id":"plan-1","plan_revision":1,"plan_hash":HASH,"approved_by":"developer"},"run":{"id":"run-1","state":"approved","metadata":{"kind":"workflow"}}}),
            ),
            ("POST", "/api/v1/runs/run-1/implement") => {
                status(409, r#"{"error":"workflow already has active work"}"#)
            }
            _ => json(json!([])),
        },
    )
}

async fn next(bridge: &mut Bridge, id: &str) -> BridgeEvent {
    loop {
        let event = tokio::time::timeout(Duration::from_secs(10), bridge.recv())
            .await
            .expect("an event in time")
            .expect("the bridge is open")
            .expect("a valid event");
        if event.id == id {
            return event;
        }
    }
}

fn request(id: &str, op: &str, fields: Value) -> ClientCommand {
    ClientCommand::new(id, op, fields)
}

#[tokio::test(flavor = "multi_thread")]
async fn catalog_reads_and_redaction() {
    let mock = mock();
    let mut bridge = Bridge::start(TOKEN, Some(&mock.url()));
    bridge.send(&request("c1", "catalog", json!({}))).unwrap();
    let catalog = next(&mut bridge, "c1").await;
    let commands = catalog.fields["commands"].as_array().unwrap();
    assert_eq!(commands.len(), 76, "74 actions and 2 projections");
    // Orbit (ADR 0036): status and the sandbox plan are reads; exec, run and sync are mutations.
    for (name, read_only) in [
        ("orbit", true),
        ("sbx", true),
        ("orbit-exec", false),
        ("orbit-run", false),
        ("orbit-sync", false),
    ] {
        let command = commands.iter().find(|c| c["name"] == name).unwrap();
        assert_eq!(command["read_only"], read_only, "{name}");
        assert!(command.get("failed").is_none(), "{name}");
    }
    assert_eq!(
        commands.iter().find(|c| c["name"] == "sbx").unwrap()["rest"],
        true
    );
    let approve = commands.iter().find(|c| c["name"] == "approve").unwrap();
    assert!(
        approve["flags"].get("acknowledge-hash").is_none(),
        "approval needs a fresh typed hash"
    );
    assert_eq!(
        commands.iter().find(|c| c["name"] == "projects").unwrap()["read_only"],
        true
    );
    assert!(commands.iter().all(|c| c.get("example").is_none()));
    bridge
        .send(&request(
            "r1",
            "read",
            json!({"name":"health","args":[],"flags":{}}),
        ))
        .unwrap();
    let result = next(&mut bridge, "r1").await;
    assert_eq!(result.event, "result");
    assert_eq!(
        result.fields["formatted"],
        "runtime ok · Pi 1.3.0 · native execution no · build [redacted]"
    );
    assert_eq!(result.fields["value"]["build_identity"], "[redacted]");
    bridge
        .send(&request(
            "r2",
            "read",
            json!({"name":"implement","args":["run-1"],"flags":{}}),
        ))
        .unwrap();
    let refused = next(&mut bridge, "r2").await;
    assert_eq!(
        (refused.event.as_str(), refused.fields["code"].as_str()),
        ("error", Some("read_only"))
    );
    bridge
        .send(&request(
            "r3",
            "invoke",
            json!({"name":"implement","args":["run-1"],"flags":{}}),
        ))
        .unwrap();
    let refused = next(&mut bridge, "r3").await;
    assert_eq!(refused.fields["code"], "command_failed");
    assert_eq!(refused.fields["outcome"], "refused");
    assert!(refused.fields["message"]
        .as_str()
        .unwrap()
        .starts_with("Implementation refused: workflow already has active work"));
    bridge
        .send(&request(
            "r4",
            "invoke",
            json!({"name":"plan","args":["t"],"flags":{"acknowledge-hash":"x"}}),
        ))
        .unwrap();
    assert_eq!(next(&mut bridge, "r4").await.fields["code"], "usage");
    bridge.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn approval_prompts_for_a_typed_acknowledgment_and_validates_replies() {
    let mock = mock();
    let mut bridge = Bridge::start(TOKEN, Some(&mock.url()));
    bridge
        .send(&request(
            "a1",
            "invoke",
            json!({"name":"approve","args":["plan-1"],"flags":{}}),
        ))
        .unwrap();
    let mut plan_shown = String::new();
    let prompt = loop {
        let event = next(&mut bridge, "a1").await;
        match event.event.as_str() {
            "output" => plan_shown.push_str(event.fields["text"].as_str().unwrap()),
            "prompt" => break event,
            other => panic!("unexpected {other}"),
        }
    };
    assert!(plan_shown.contains(&format!("Immutable plan hash: {HASH}")));
    assert_eq!(prompt.fields["kind"], "hash");
    let prompt_id = prompt.fields["prompt_id"].as_str().unwrap().to_string();
    bridge
        .send(&request(
            "x1",
            "reply",
            json!({"request_id":"a1","prompt_id":prompt_id,"value":true}),
        ))
        .unwrap();
    assert_eq!(
        next(&mut bridge, "x1").await.fields["code"],
        "invalid_reply"
    );
    bridge
        .send(&request(
            "x2",
            "reply",
            json!({"request_id":"a1","prompt_id":"stale","value":"x"}),
        ))
        .unwrap();
    assert_eq!(next(&mut bridge, "x2").await.fields["code"], "stale_prompt");
    assert!(
        prompt.fields["question"]
            .as_str()
            .unwrap()
            .contains("Type approve"),
        "the developer types a word, not the hash"
    );
    bridge
        .send(&request(
            "x3",
            "reply",
            json!({"request_id":"a1","prompt_id":prompt_id,"value":"approve"}),
        ))
        .unwrap();
    assert_eq!(next(&mut bridge, "x3").await.event, "result");
    let done = next(&mut bridge, "a1").await;
    assert_eq!(done.event, "result", "{:?}", done.fields);
    assert!(done.fields["formatted"]
        .as_str()
        .unwrap()
        .starts_with("Approved plan plan-1 r1 by developer"));
    let sent = mock
        .take()
        .into_iter()
        .find(|r| r.path.ends_with("/approvals"))
        .unwrap();
    assert_eq!(sent.json()["plan_hash"], HASH);
    bridge.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn guides_and_protocol_errors_are_refused_before_anything_runs() {
    let mock = mock();
    let mut bridge = Bridge::start(TOKEN, Some(&mock.url()));
    bridge
        .send(&request("g1", "guide", json!({"name":"prime","flags":{}})))
        .unwrap();
    assert_eq!(
        next(&mut bridge, "g1").await.fields["code"],
        "unknown_command"
    );
    bridge
        .send(&request(
            "g2",
            "guide",
            json!({"name":"init","flags":{"evil":"x"}}),
        ))
        .unwrap();
    assert_eq!(next(&mut bridge, "g2").await.fields["code"], "usage");
    bridge.send(&request("p1", "nope", json!({}))).unwrap();
    assert_eq!(
        next(&mut bridge, "p1").await.fields["code"],
        "invalid_operation"
    );
    bridge
        .send(&request(
            "p2",
            "read",
            json!({"name":"health","args":[],"flags":{},"extra":1}),
        ))
        .unwrap();
    assert_eq!(
        next(&mut bridge, "p2").await.fields["code"],
        "invalid_request"
    );
    bridge
        .send(&request("p3", "abort", json!({"request_id":"none"})))
        .unwrap();
    assert_eq!(
        next(&mut bridge, "p3").await.fields["code"],
        "unknown_request"
    );
    assert!(mock.take().is_empty(), "nothing reached the runtime");
    bridge.send(&request("s1", "shutdown", json!({}))).unwrap();
    assert_eq!(
        next(&mut bridge, "s1").await.fields["value"]["closed"],
        true
    );
    bridge
        .send(&request(
            "s2",
            "read",
            json!({"name":"health","args":[],"flags":{}}),
        ))
        .unwrap();
    assert_eq!(next(&mut bridge, "s2").await.fields["code"], "closed");
    bridge.shutdown().await;
}
