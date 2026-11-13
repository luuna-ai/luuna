//! Go's runClose, with A8 checks before the single rewrite.
use super::{lossy, render::string_array};
use crate::command::{
    options,
    output::Envelope,
    plans,
    shared::{ticket_index, without_string},
    Context, Failure,
};
use crate::{model, store::DataFile};
pub(super) fn run(context: &mut Context<'_>, args: &[String]) -> Result<(), Failure> {
    let opts = options::parse(args, &["reason"], &["json"])?;
    let ids = &opts.positionals;
    if ids.is_empty() {
        return Err(Failure::message(
            "Usage: luuna-missions close <id> [ids...] [--reason text]",
        ));
    }
    let store = context.store()?;
    let now = model::now_iso();
    let _plans = store.lock(DataFile::Plans)?;
    let _issues = store.lock(DataFile::Issues)?;
    let before = store.read_tickets()?;
    let selected = ids
        .iter()
        .map(|id| {
            ticket_index(&before, id)
                .map(|i| &before[i])
                .ok_or_else(|| Failure::message(format!("Issue not found: {id}")))
        })
        .collect::<Result<Vec<_>, _>>()?;
    plans::check_status_change(&store, &selected)?;
    let mut after = before.clone();
    for id in ids {
        let i = ticket_index(&after, id).expect("all ids checked");
        after[i].status = "closed".into();
        after[i].closed_at = now.clone();
        after[i].updated_at = now.clone();
        if !opts.value("reason").is_empty() {
            after[i].close_reason = opts.value("reason").into();
        }
        for blocked in after[i].blocks.clone().unwrap_or_default() {
            if let Some(j) = ticket_index(&after, &blocked) {
                after[j].blocked_by = without_string(after[j].blocked_by.take(), id);
                after[j].updated_at = now.clone();
            }
        }
    }
    lossy::refuse_lossy_ticket_changes(&store, &before, &after)?;
    store.write_tickets(&after)?;
    plans::apply_status_change(&store, ids, &after, &now)?;
    if opts.has("json") {
        context.json(Envelope::new(true, "close").raw("closed", &string_array(ids)));
    } else {
        for id in ids {
            let reason = opts.value("reason");
            context.success(
                (if reason.is_empty() {
                    format!("Closed {id}")
                } else {
                    format!("Closed {id} — {reason}")
                })
                .as_bytes(),
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::dep::test_support::*;
    #[test]
    fn closing_updates_dependents_and_takes_both_locks() {
        use crate::store::{DataFile, Store};
        use std::{fs, sync::mpsc, thread, time::Duration};
        let t = project(&base());
        let root = t.path().to_owned();
        let store = Store::open(root.join(".luuna/missions")).unwrap();
        let lock = store.lock(DataFile::Plans).unwrap();
        let (tx, rx) = mpsc::channel();
        let path = root.clone();
        let handle = thread::spawn(move || {
            tx.send(invoke(
                &path,
                &["close", "b", "b", "--reason", "done", "--json"],
            ))
            .unwrap();
        });
        thread::sleep(Duration::from_millis(30));
        assert!(rx.try_recv().is_err());
        assert!(!root.join(".luuna/missions/issues.jsonl.lock").exists());
        drop(lock);
        let out = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        handle.join().unwrap();
        assert_eq!(out.exit_code, 0);
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&out.stdout).unwrap()["closed"],
            serde_json::json!(["b", "b"])
        );
        assert_eq!(value(&root, "b")["closeReason"], "done");
        assert_eq!(
            value(&root, "t")["updatedAt"],
            value(&root, "b")["closedAt"]
        );
        assert!(value(&root, "t").get("blockedBy").is_none());
        assert_eq!(
            fs::read_dir(root.join(".luuna/missions")).unwrap().count(),
            2
        );
    }
    #[test]
    fn a_missing_id_writes_nothing() {
        let input = base();
        let t = project(&input);
        assert_eq!(
            invoke(t.path(), &["close", "t", "missing"]).stderr,
            b"Error: Issue not found: missing\n"
        );
        assert_eq!(bytes(t.path()), input);
    }
    #[test]
    fn plan_involvement_is_refused_through_the_seam() {
        use std::fs;
        let input = base();
        let t = project(&input);
        fs::write(t.path().join(".luuna/missions/plans.jsonl"), b"{}\n").unwrap();
        let error = crate::command::run(&crate::command::Invocation::new(t.path(), ["close", "b"]))
            .unwrap_err();
        assert!(error.to_string().contains("s4-missions-lib-4"));
        assert_eq!(bytes(t.path()), input);
        assert_eq!(
            fs::read_dir(t.path().join(".luuna/missions"))
                .unwrap()
                .count(),
            3
        );
    }
    #[test]
    fn a_lossy_issues_file_is_refused_unchanged() {
        file_refusals(&["close", "b"]);
    }
    #[test]
    fn a_dependent_list_go_cannot_rewrite_is_refused_unchanged() {
        field_refusals(&["close", "b"], "blockedBy", false, false);
    }
    #[test]
    fn lists_close_leaves_unchanged_are_written_as_go_writes_them() {
        control(&["close", "b"], "blockedBy", false, br#""b""#, 0);
        for raw in [
            br#""urgent""#.as_slice(),
            br#"["a",5]"#,
            br#"["a",null]"#,
            b"[\"a\",\"\xff\"]",
            br#"["a","\ud800"]"#,
            b"[\"\xff\",\"\xfe\"]",
        ] {
            let input = [
                record("t", "open", &[b",\"labels\":", raw].concat()),
                record("b", "open", b""),
            ]
            .concat();
            let t = project(&input);
            assert_eq!(invoke(t.path(), &["close", "t"]).exit_code, 0);
            let needle = [b"\"labels\":", raw].concat();
            assert!(bytes(t.path()).windows(needle.len()).any(|s| s == needle));
        }
    }
}
