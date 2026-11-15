//! Go's runUnblock; --all keeps Go's precedence over --from.
use super::{lossy, render::string_array};
use crate::command::{
    options,
    output::Envelope,
    read::{closed_ids, println},
    shared::{ticket_index, without_string},
    Context, Failure,
};
use crate::{model, store::DataFile};
pub(super) fn run(context: &mut Context<'_>, args: &[String]) -> Result<(), Failure> {
    let opts = options::parse(args, &["from"], &["all", "json"])?;
    if opts.positionals.is_empty() || (!opts.has("all") && !opts.has("from")) {
        return Err(Failure::message(
            "Usage: luuna-missions unblock <id> [--from <blocker-id> | --all]",
        ));
    }
    if opts.has("from") && opts.value("from").is_empty() {
        return Err(Failure::message("--from requires a blocker ID"));
    }
    let id = &opts.positionals[0];
    let store = context.store()?;
    let mut removed = Vec::new();
    {
        let _lock = store.lock(DataFile::Issues)?;
        let before = store.read_tickets()?;
        let i = ticket_index(&before, id)
            .ok_or_else(|| Failure::message(format!("Issue not found: {id}")))?;
        let current = before[i].blocked_by.as_deref().unwrap_or_default();
        if opts.has("all") {
            let closed = closed_ids(&before);
            removed.extend(
                current
                    .iter()
                    .filter(|bid| closed.contains(bid.as_str()))
                    .cloned(),
            );
        } else {
            let bid = opts.value("from");
            if !current.iter().any(|v| v == bid) {
                return Err(Failure::message(format!("{id} is not blocked by {bid}")));
            }
            removed.push(bid.into());
        }
        let mut after = before.clone();
        let now = model::now_iso();
        after[i].updated_at = now.clone();
        for bid in &removed {
            after[i].blocked_by = without_string(after[i].blocked_by.take(), bid);
            if let Some(j) = ticket_index(&after, bid) {
                after[j].blocks = without_string(after[j].blocks.take(), id);
                after[j].updated_at = now.clone();
            }
        }
        lossy::refuse_lossy_ticket_changes(&store, &before, &after)?;
        store.write_tickets(&after)?;
    }
    if opts.has("json") {
        context.json(
            Envelope::new(true, "unblock")
                .string("issueId", id)
                .raw("removed", &string_array(&removed)),
        );
    } else if removed.is_empty() {
        println(context, &format!("No closed blockers to remove from {id}."));
    } else {
        for bid in removed {
            println(context, &format!("{id} unblocked from {bid}"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::dep::test_support::*;
    #[test]
    fn all_removes_only_closed_blockers() {
        let t = project(&base());
        assert_eq!(
            invoke(t.path(), &["unblock", "t", "--all", "--from", "ignored"]).stdout,
            b"t unblocked from b\n"
        );
        assert!(value(t.path(), "t").get("blockedBy").is_none());
        assert!(value(t.path(), "b").get("blocks").is_none());
        let out = invoke(t.path(), &["unblock", "t", "--all"]);
        assert_eq!(out.stdout, b"No closed blockers to remove from t.\n");
    }
    #[test]
    fn from_requires_the_link() {
        let t = project(&base());
        assert_eq!(
            invoke(t.path(), &["unblock", "t", "--from", "o"]).stderr,
            b"Error: t is not blocked by o\n"
        );
        assert_eq!(bytes(t.path()), base());
        assert_eq!(
            invoke(t.path(), &["unblock", "t", "--from="]).stderr,
            b"Error: --from requires a blocker ID\n"
        );
    }
    #[test]
    fn a_lossy_issues_file_is_refused_unchanged() {
        file_refusals(&["unblock", "t", "--all"]);
    }
    #[test]
    fn a_list_go_cannot_rewrite_is_refused_unchanged() {
        for args in [
            &["unblock", "t", "--from", "b"][..],
            &["unblock", "t", "--all"],
        ] {
            field_refusals(args, "blockedBy", false, false);
        }
        field_refusals(&["unblock", "t", "--from", "b"], "blocks", true, false);
    }
    #[test]
    fn lists_unblock_leaves_unchanged_are_kept_byte_for_byte() {
        control(
            &["unblock", "t", "--from", "b"],
            "blockedBy",
            false,
            br#""b""#,
            1,
        );
        control(&["unblock", "t", "--all"], "blockedBy", false, br#""b""#, 0);
        control(
            &["unblock", "t", "--from", "b"],
            "blocks",
            true,
            br#""t""#,
            0,
        );
        let input = [
            record("t", "open", b",\"blockedBy\":[\"b\",\"\xff\"]"),
            record("b", "open", b""),
        ]
        .concat();
        let t = project(&input);
        assert_eq!(invoke(t.path(), &["unblock", "t", "--all"]).exit_code, 0);
        assert!(bytes(t.path()).windows(5).any(|s| s == b"\"\xff\"]}"));
    }
}
