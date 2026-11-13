//! Embedded reference workflows and plan-aware project state.
use crate::{
    command::{options, output::Envelope, plan_index, read, Context, Failure},
    config,
    error::{Error, Result},
    json,
    model::Ticket,
    store::Store,
};
use std::fs;
const ASSETS: [&[u8]; 4] = [
    include_bytes!("../../../assets/s4-missions-lib-3/prime_full.json"),
    include_bytes!("../../../assets/s4-missions-lib-3/prime_compact.json"),
    include_bytes!("../../../assets/s4-missions-lib-3/prime_full_verify.json"),
    include_bytes!("../../../assets/s4-missions-lib-3/prime_compact_verify.json"),
];
struct State {
    progress: Vec<Ticket>,
    ready: Vec<Ticket>,
    count: usize,
}
fn load(store: &Store) -> Result<State> {
    let issues = store.read_tickets()?;
    let plans = plan_index::load(store)?;
    let closed = read::closed_ids(&issues);
    let mut progress = vec![];
    let mut ready = vec![];
    for issue in &issues {
        if issue.status == "in_progress" {
            progress.push(issue.clone());
        }
        if issue.status != "open" {
            continue;
        }
        if issue.requires_plan {
            if matches!(plans.status_for_seed(&issue.id), None | Some("draft")) {
                continue;
            }
        } else if plans.status_for_ticket(issue) == Some("draft") {
            ready.push(issue.clone());
            continue;
        }
        if !read::effectively_blocked(issue, &closed) {
            ready.push(issue.clone());
        }
    }
    read::sort_tickets(&mut progress, "priority").map_err(|message| Error::Command { message })?;
    read::sort_tickets(&mut ready, "priority").map_err(|message| Error::Command { message })?;
    let count = ready.len();
    ready.truncate(5);
    Ok(State {
        progress,
        ready,
        count,
    })
}
fn refs(issues: &[Ticket]) -> Vec<u8> {
    let mut out = vec![b'['];
    for (i, t) in issues.iter().enumerate() {
        if i > 0 {
            out.push(b',');
        }
        out.extend_from_slice(b"{\"id\":");
        json::quote(&mut out, &t.id, false);
        out.extend_from_slice(b",\"title\":");
        json::quote(&mut out, &t.title, false);
        out.extend_from_slice(format!(",\"priority\":{}", t.priority).as_bytes());
        if !t.assignee.is_empty() {
            out.extend_from_slice(b",\"assignee\":");
            json::quote(&mut out, &t.assignee, false);
        }
        out.push(b'}');
    }
    out.push(b']');
    out
}
fn state_json(s: &State) -> Vec<u8> {
    let mut out = b"{\"inProgress\":".to_vec();
    out.extend_from_slice(&refs(&s.progress));
    out.extend_from_slice(b",\"ready\":");
    out.extend_from_slice(&refs(&s.ready));
    out.extend_from_slice(format!(",\"readyCount\":{}}}", s.count).as_bytes());
    out
}
fn line(t: &Ticket) -> String {
    format!(
        "- {} P{} {}{}",
        t.id,
        t.priority,
        t.title,
        if t.assignee.is_empty() {
            String::new()
        } else {
            format!(" (@{})", t.assignee)
        }
    )
}
fn render(s: &State) -> String {
    let mut lines = vec!["## Current State".into(), String::new()];
    if !s.progress.is_empty() {
        lines.push("In progress:".into());
        lines.extend(s.progress.iter().map(line));
        lines.push(String::new());
    }
    if s.count == 0 {
        lines.push("Ready: none".into());
    } else {
        lines.push(if s.count > s.ready.len() {
            format!("Ready (top {} of {}):", s.ready.len(), s.count)
        } else {
            "Ready:".into()
        });
        lines.extend(s.ready.iter().map(line));
    }
    lines.push(String::new());
    lines.join("\n")
}
fn go_text(bytes: &[u8]) -> String {
    let mut text = String::new();
    let mut i = 0;
    while i < bytes.len() {
        let (ch, n) = json::decode_char(&bytes[i..]);
        text.push(ch.unwrap_or('\u{fffd}'));
        i += n;
    }
    text
}
fn optional<T>(result: Result<T>) -> Result<Option<T>> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(e @ Error::Unsupported { .. }) => Err(e),
        Err(_) => Ok(None),
    }
}
pub(super) fn run(c: &mut Context<'_>, args: &[String]) -> std::result::Result<(), Failure> {
    let o = options::parse(args, &[], &["compact", "export", "json"])?;
    if !o.positionals.is_empty() {
        return Err(Failure::message("too many arguments for 'prime'"));
    }
    let store = optional(c.store())?;
    let root = store
        .as_ref()
        .map(|s| config::project_root(s.dir()))
        .unwrap_or_else(|| c.cwd.clone());
    let verify = fs::metadata(root.join("go.mod")).is_ok_and(|m| m.is_file());
    // Use this crate's Go map decoder: serde_json object iteration order is a
    // workspace-unified feature and must not change our wire bytes.
    let mut fixture =
        json::Value::decode(ASSETS[usize::from(o.has("compact")) + 2 * usize::from(verify)])
            .expect("embedded reference fixture")
            .into_object()
            .expect("reference object");
    let mut content = fixture
        .remove("content")
        .and_then(json::Value::into_string)
        .expect("reference content")
        .into_bytes();
    let mut sections = Vec::new();
    fixture
        .remove("sections")
        .expect("reference sections")
        .encode(&mut sections, false)
        .expect("reference values");
    let mut state = None;
    if !o.has("export") {
        if let Some(s) = &store {
            if let Ok(custom) = fs::read(s.dir().join("PRIME.md")) {
                content = custom;
                sections = b"null".to_vec();
            }
            state = optional(load(s))?;
        }
    }
    if o.has("json") {
        let mut out = Envelope::new(true, "prime").raw("sections", &sections);
        if !o.has("export") {
            out = out.raw(
                "state",
                &state
                    .as_ref()
                    .map(state_json)
                    .unwrap_or_else(|| b"null".to_vec()),
            );
        }
        c.json_with_records(out.string("content", &go_text(&content)), "cli.primeState")?;
    } else {
        c.stdout.extend_from_slice(&content);
        if let Some(s) = state {
            if !content.is_empty() && !content.ends_with(b"\n\n") {
                c.stdout.extend_from_slice(if content.ends_with(b"\n") {
                    b"\n"
                } else {
                    b"\n\n"
                });
            }
            c.stdout.extend_from_slice(render(&s).as_bytes());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config_tree::tree::test_support::{invoke, project};
    #[test]
    fn an_unsupported_refusal_is_not_swallowed() {
        let e = optional::<()>(Err(Error::unsupported("future plan state", "later"))).unwrap_err();
        assert!(matches!(e, Error::Unsupported { .. }));
        assert!(optional::<()>(Err(Error::NotInProject)).unwrap().is_none());
    }
    #[test]
    fn go_project_verification_guidance() {
        for compact in [false, true] {
            let t = project("project: demo\nversion: \"1\"\n");
            let args = if compact {
                vec!["prime", "--compact"]
            } else {
                vec!["prime"]
            };
            let out = invoke(t.path(), &args);
            assert_eq!(out.exit_code, 0);
            assert!(!String::from_utf8(out.stdout)
                .unwrap()
                .contains("go test ./..."));
            fs::write(
                t.path().join("go.mod"),
                b"module example.invalid/project\n\ngo 1.22\n",
            )
            .unwrap();
            let out = invoke(t.path(), &args);
            assert_eq!(out.exit_code, 0);
            assert!(String::from_utf8(out.stdout)
                .unwrap()
                .contains("go test ./..."));
        }
    }
    #[test]
    fn works_without_a_store() {
        let t = tempfile::tempdir().unwrap();
        let out = invoke(t.path(), &["prime", "--json"]);
        assert_eq!(out.exit_code, 0);
        let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(v["state"], serde_json::Value::Null);
        assert_eq!(
            v["content"],
            serde_json::from_slice::<serde_json::Value>(ASSETS[0]).unwrap()["content"]
        );
        assert_eq!(fs::read_dir(t.path()).unwrap().count(), 0);
    }
}
