//! Preserved foundation inputs delegate their evolving plan expectations here.
use crate::command::{run, Invocation};
use std::{fs, path::Path};
pub(in crate::command) fn read(root: &Path, args: &[&str]) {
    let cases: serde_json::Value = serde_json::from_str(include_str!(
        "../../../assets/s4-missions-lib-4/foundation-read-go.json"
    ))
    .unwrap();
    let wanted = serde_json::to_value(args).unwrap();
    let expected = cases
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["args"] == wanted)
        .expect("every preserved input has a literal Go result");
    let out = run(&Invocation::new(root, args.iter().copied())).unwrap();
    assert_eq!(
        out.exit_code,
        expected["exit"].as_i64().unwrap() as i32,
        "{args:?}"
    );
    assert_eq!(
        out.stdout,
        expected["stdout"].as_str().unwrap().as_bytes(),
        "{args:?}"
    );
    assert_eq!(
        out.stderr,
        expected["stderr"].as_str().unwrap().as_bytes(),
        "{args:?}"
    );
}

fn files(root: &Path) -> (Vec<u8>, Vec<u8>) {
    let dir = root.join(".luuna/missions");
    (
        fs::read(dir.join("issues.jsonl")).unwrap(),
        fs::read(dir.join("plans.jsonl")).unwrap(),
    )
}
pub(in crate::command) fn update(root: &Path, args: &[&str], _kind: u8) {
    let before = files(root);
    let mut all = vec!["update"];
    all.extend(args);
    let out = run(&Invocation::new(root, all)).unwrap();
    if args.contains(&"--claim") {
        assert_eq!(out.exit_code, 1);
        assert_eq!(out.stdout, b"");
        assert_eq!(out.stderr, b"Error: Cannot claim p-1: status is closed\n");
        assert_eq!(files(root), before);
        return;
    }
    assert_eq!(out.exit_code, 0, "{args:?}");
    let dir = root.join(".luuna/missions");
    let store = crate::store::Store::open(&dir).unwrap();
    let tickets = store.read_tickets().unwrap();
    assert_eq!(tickets.len(), 1);
    let t = &tickets[0];
    assert_eq!(t.id, "p-1");
    let status = if args.contains(&"closed") {
        "closed"
    } else {
        "in_progress"
    };
    assert_eq!(t.status, status);
    assert_ne!(t.updated_at, "u");
    if args.contains(&"--title") {
        assert_eq!(t.title, "other")
    }
    if args.contains(&"--claim") {
        assert_eq!(t.assignee, "me")
    }
    assert_eq!(files(root).1, before.1);
    assert!(fs::read_dir(dir).unwrap().all(|e| {
        let name = e.unwrap().file_name().to_string_lossy().into_owned();
        !name.contains(".lock") && !name.contains(".tmp")
    }));
}

pub(in crate::command) fn close(root: &Path, input: &[u8]) {
    let dir = root.join(".luuna/missions");
    let mode = fs::metadata(dir.join("issues.jsonl"))
        .unwrap()
        .permissions();
    let out = run(&Invocation::new(root, ["close", "b"])).unwrap();
    let expected: serde_json::Value = serde_json::from_str(include_str!(
        "../../../assets/s4-missions-lib-4/foundation-close-go.json"
    ))
    .unwrap();
    assert_eq!(out.exit_code, 0);
    assert_eq!(out.stdout, expected["stdout"].as_str().unwrap().as_bytes());
    assert_eq!(out.stderr, expected["stderr"].as_str().unwrap().as_bytes());
    assert_eq!(
        fs::read(dir.join("plans.jsonl")).unwrap(),
        expected["plans"].as_str().unwrap().as_bytes()
    );
    let after = fs::read_to_string(dir.join("issues.jsonl")).unwrap();
    assert_ne!(after.as_bytes(), input);
    let store = crate::store::Store::open(&dir).unwrap();
    let tickets = store.read_tickets().unwrap();
    let time = &tickets.iter().find(|t| t.id == "b").unwrap().closed_at;
    chrono::DateTime::parse_from_rfc3339(time).unwrap();
    assert_eq!(
        after.replace(time, "<time>"),
        expected["issues"].as_str().unwrap()
    );
    assert_eq!(
        fs::metadata(dir.join("issues.jsonl"))
            .unwrap()
            .permissions(),
        mode
    );
    assert_eq!(fs::read_dir(dir).unwrap().count(), 3);
}
