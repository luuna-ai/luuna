//! Status hooks preflight the same prospective transition later persisted by Go.
use super::{common::load, guard, record::Plan};
use crate::{
    error::Result,
    model::Ticket,
    store::{self, DataFile, Store},
};
use std::collections::{HashMap, HashSet};
fn transition(plans: &mut [Plan], ids: &[String], issues: &[Ticket], now: &str) -> bool {
    let targets: HashSet<_> = ids.iter().collect();
    let by_id: HashMap<_, _> = issues.iter().map(|t| (&t.id, t)).collect();
    let mut changed = false;
    for p in plans {
        if p.status == "draft" || !p.children().iter().any(|id| targets.contains(id)) {
            continue;
        }
        let children: Vec<_> = p
            .children()
            .iter()
            .filter_map(|id| by_id.get(id).copied())
            .collect();
        if children.is_empty() {
            continue;
        }
        let next = if children.iter().all(|c| c.status == "closed") {
            "done"
        } else if p.status == "active"
            || p.status == "done"
            || children.iter().any(|c| c.status == "in_progress")
        {
            "active"
        } else {
            "approved"
        };
        if p.status != next {
            p.status = next.into();
            p.updated_at = now.into();
            changed = true
        }
    }
    changed
}
pub(super) fn check(store: &Store, tickets: &[&Ticket]) -> Result<()> {
    let mut issues = store.read_tickets()?;
    let ids = tickets.iter().map(|t| t.id.clone()).collect::<Vec<_>>();
    for t in tickets {
        if let Some(i) = issues.iter().position(|old| old.id == t.id) {
            issues[i] = (*t).clone();
        }
    }
    let mut plans = load(store)?;
    if transition(&mut plans, &ids, &issues, "") {
        guard::plan_rewrite(store)?
    }
    Ok(())
}
pub(super) fn apply(store: &Store, ids: &[String], issues: &[Ticket], now: &str) -> Result<()> {
    let mut plans = load(store)?;
    if transition(&mut plans, ids, issues, now) {
        guard::plan_rewrite(store)?;
        store::write_records(&store.path(DataFile::Plans), &plans)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        command::{run, Invocation},
        config_tree::tree::test_support::{invoke, project},
        store::Record,
    };
    use std::fs;
    fn plan(status: &str) -> Plan {
        Plan::from_line(format!(r#"{{"id":"pl-a","seed":"parent","status":"{status}","children":["a","missing"],"updatedAt":"old"}}"#).as_bytes()).unwrap()
    }
    fn ticket(status: &str) -> Ticket {
        let mut t = Ticket::default();
        t.id = "a".into();
        t.status = status.into();
        t
    }
    #[test]
    fn lifecycle_truth_table_keeps_go_sticky_active_and_done_rules() {
        for (before, status, expected) in [
            ("approved", "open", "approved"),
            ("approved", "in_progress", "active"),
            ("approved", "closed", "done"),
            ("active", "open", "active"),
            ("done", "open", "active"),
            ("unknown", "open", "approved"),
            ("draft", "closed", "draft"),
        ] {
            let mut p = vec![plan(before)];
            let changed = transition(&mut p, &["a".into()], &[ticket(status)], "now");
            assert_eq!(p[0].status, expected);
            assert_eq!(changed, before != expected);
            assert_eq!(p[0].updated_at, if changed { "now" } else { "old" });
        }
    }
    #[test]
    fn targets_use_children_not_seed_or_plan_id_and_missing_children_do_not_block_done() {
        let mut p = vec![plan("approved")];
        assert!(!transition(
            &mut p,
            &["parent".into()],
            &[ticket("closed")],
            "now"
        ));
        assert!(!transition(
            &mut p,
            &["absent".into()],
            &[ticket("closed")],
            "now"
        ));
        assert!(transition(
            &mut p,
            &["a".into()],
            &[ticket("closed")],
            "now"
        ));
        assert_eq!(p[0].status, "done");
        let mut p = vec![plan("approved")];
        assert!(!transition(&mut p, &["missing".into()], &[], "now"));
    }
    fn setup(status: &str, tail: &[u8]) -> tempfile::TempDir {
        let t = project("project: demo\nversion: \"1\"\n");
        let d = t.path().join(".luuna/missions");
        fs::write(d.join("issues.jsonl"),b"{\"id\":\"a\",\"status\":\"open\",\"blocks\":[\"parent\"]}\n{\"id\":\"parent\",\"status\":\"open\",\"blockedBy\":[\"a\"]}\n").unwrap();
        let mut raw = plan(status).marshal().unwrap();
        raw.push(b'\n');
        raw.extend_from_slice(tail);
        fs::write(d.join("plans.jsonl"), raw).unwrap();
        t
    }
    fn files(root: &std::path::Path) -> Vec<(Vec<u8>, fs::Permissions)> {
        ["issues.jsonl", "plans.jsonl"]
            .into_iter()
            .map(|n| {
                let p = root.join(".luuna/missions").join(n);
                (
                    fs::read(&p).unwrap(),
                    fs::metadata(p).unwrap().permissions(),
                )
            })
            .collect()
    }
    #[test]
    fn close_preflight_refuses_lossy_plan_before_either_write() {
        let t = setup("approved", b"unreadable\n");
        let before = files(t.path());
        let out = invoke(t.path(), &["close", "a", "--json"]);
        assert_eq!(out.exit_code, 1);
        assert!(String::from_utf8(out.stdout)
            .unwrap()
            .contains("Nothing was written"));
        assert_eq!(files(t.path()), before);
    }
    #[test]
    fn update_preflight_refuses_lossy_plan_and_issue_rewrite() {
        for file in ["plans.jsonl", "issues.jsonl"] {
            let t = setup("approved", b"");
            let p = t.path().join(".luuna/missions").join(file);
            let mut raw = fs::read(&p).unwrap();
            raw.extend_from_slice(b"unreadable\n");
            fs::write(p, raw).unwrap();
            let before = files(t.path());
            let out = invoke(
                t.path(),
                &["update", "a", "--status", "in_progress", "--json"],
            );
            assert_eq!(out.exit_code, 1);
            assert_eq!(files(t.path()), before);
        }
    }
    #[test]
    fn no_transition_preserves_unreadable_plan_lines_and_raw_sections() {
        let t = setup("draft", b"unreadable\n");
        let plans = t.path().join(".luuna/missions/plans.jsonl");
        let before = fs::read(&plans).unwrap();
        assert_eq!(invoke(t.path(), &["close", "a"]).exit_code, 0);
        assert_eq!(fs::read(plans).unwrap(), before);
    }
    #[test]
    fn closing_and_reopening_transitions_plan_and_retains_raw_fields() {
        let t = setup("approved", b"");
        let path = t.path().join(".luuna/missions/plans.jsonl");
        let raw = String::from_utf8(fs::read(&path).unwrap())
            .unwrap()
            .replace(
                "\"sections\":null",
                "\"sections\":{\"n\":9007199254740993},\"future\":true",
            );
        fs::write(&path, raw).unwrap();
        assert_eq!(invoke(t.path(), &["close", "a"]).exit_code, 0);
        let raw = String::from_utf8(fs::read(&path).unwrap()).unwrap();
        assert!(raw.contains("\"status\":\"done\""));
        assert!(raw.contains("9007199254740993"));
        let store = Store::open(t.path().join(".luuna/missions")).unwrap();
        assert_eq!(store.read_tickets().unwrap()[1].blocked_by, None);
        assert_eq!(
            invoke(t.path(), &["update", "a", "--status", "open"]).exit_code,
            0
        );
        assert_eq!(load(&store).unwrap()[0].status, "active");
    }
    #[test]
    fn unsupported_status_hook_errors_propagate_through_dispatch() {
        let t = setup("approved", b"");
        let path = t.path().join(".luuna/missions/plans.jsonl");
        fs::remove_file(&path).unwrap();
        fs::create_dir(&path).unwrap();
        assert!(run(&Invocation::new(
            t.path(),
            ["update", "a", "--status", "in_progress"]
        ))
        .is_err());
    }
}
