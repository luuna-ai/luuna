//! `block <id> --by <blocker>` (`internal/cli/issue_links.go:13-35, 161-190`).

use super::output::Envelope;
use super::shared::{ticket_index, unique_strings};
use super::{options, Context, Failure};
use crate::model;
use crate::store::DataFile;

/// Go's `runBlock`.
pub(super) fn run(context: &mut Context<'_>, args: &[String]) -> Result<(), Failure> {
    let options = options::parse(args, &["by"], &["json"])?;
    if options.positionals.is_empty() || !options.has("by") || options.value("by").is_empty() {
        return Err(Failure::message(
            "Usage: luuna-missions block <id> --by <blocker-id>",
        ));
    }
    let id = options.positionals[0].as_str();
    let blocker = options.value("by");
    if id == blocker {
        return Err(Failure::message(format!(
            "Cannot block an issue by itself: {id}"
        )));
    }
    let store = context.store()?;
    add_dependency(&store, id, blocker)?;
    if options.has("json") {
        context.json(
            Envelope::new(true, "block")
                .string("issueId", id)
                .string("blockerId", blocker),
        );
    } else {
        // Written directly, so `--quiet` does not hide it.
        context
            .stdout
            .extend_from_slice(format!("{id} is now blocked by {blocker}\n").as_bytes());
    }
    Ok(())
}

/// Go's `addDependency`.
fn add_dependency(
    store: &crate::store::Store,
    issue_id: &str,
    blocker_id: &str,
) -> Result<(), Failure> {
    if issue_id == blocker_id {
        return Err(Failure::message(format!(
            "Cannot add a self-dependency: {issue_id}"
        )));
    }
    let _lock = store.lock(DataFile::Issues)?;
    let mut tickets = store.read_tickets()?;
    let issue = ticket_index(&tickets, issue_id);
    let blocker = ticket_index(&tickets, blocker_id);
    let Some(issue) = issue else {
        return Err(Failure::message(format!("Issue not found: {issue_id}")));
    };
    let Some(blocker) = blocker else {
        return Err(Failure::message(format!("Issue not found: {blocker_id}")));
    };
    let now = model::now_iso();
    let mut blocked_by = tickets[issue].blocked_by.take().unwrap_or_default();
    blocked_by.push(blocker_id.to_owned());
    tickets[issue].blocked_by = Some(unique_strings(blocked_by));
    let mut blocks = tickets[blocker].blocks.take().unwrap_or_default();
    blocks.push(issue_id.to_owned());
    tickets[blocker].blocks = Some(unique_strings(blocks));
    tickets[issue].updated_at.clone_from(&now);
    tickets[blocker].updated_at = now;
    store.write_tickets(&tickets)?;
    Ok(())
}
