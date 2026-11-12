//! `create` (`internal/cli/issue_create.go:102-206`).

use std::collections::HashSet;

use super::output::Envelope;
use super::shared::{parse_intent, parse_priority, split_labels, TYPES};
use super::{options, Context, Failure};
use crate::config;
use crate::model::{self, Ticket};
use crate::store::DataFile;

/// Go's `runCreate`.
pub(super) fn run(context: &mut Context<'_>, args: &[String]) -> Result<(), Failure> {
    let options = options::parse(
        args,
        &[
            "title",
            "type",
            "priority",
            "assignee",
            "description",
            "desc",
            "body",
            "labels",
            "label",
            "from",
            "intent",
        ],
        &["json"],
    )?;
    if !options.positionals.is_empty() {
        return Err(Failure::message("too many arguments for 'create'"));
    }
    let title = options.value("title").trim();
    if title.is_empty() {
        return Err(Failure::message("--title is required"));
    }
    let ticket_type = match options.value("type") {
        "" => "task",
        other => other,
    };
    if !TYPES.contains(&ticket_type) {
        return Err(Failure::message(format!(
            "Invalid --type value: {ticket_type}. Valid: task|bug|feature|epic"
        )));
    }
    let priority = parse_priority(options.value("priority")).map_err(Failure::message)?;
    let labels_value = if options.has("labels") {
        options.value("labels")
    } else {
        options.value("label")
    };
    let labels = split_labels(labels_value);
    let description = if options.has("description") {
        options.value("description")
    } else if options.has("desc") {
        options.value("desc")
    } else {
        options.value("body")
    };
    let intent = parse_intent(options.repeated("intent")).map_err(Failure::message)?;
    let from = options.value("from").trim();
    let store = context.store()?;
    let project = config::read_config(store.dir())?;
    let id = {
        let _lock = store.lock(DataFile::Issues)?;
        let tickets = store.read_tickets()?;
        let ids: HashSet<String> = tickets
            .iter()
            .map(|ticket| ticket.id().to_owned())
            .collect();
        if !from.is_empty() && !ids.contains(from) {
            return Err(Failure::message(format!("--from issue not found: {from}")));
        }
        let id = model::generate_id(&project.project, &ids);
        let now = model::now_iso();
        let mut ticket = Ticket::default();
        ticket.id.clone_from(&id);
        title.clone_into(&mut ticket.title);
        "open".clone_into(&mut ticket.status);
        ticket_type.clone_into(&mut ticket.ticket_type);
        ticket.priority = priority;
        ticket.created_at.clone_from(&now);
        ticket.updated_at = now;
        options.value("assignee").clone_into(&mut ticket.assignee);
        description.clone_into(&mut ticket.description);
        ticket.labels = labels;
        from.clone_into(&mut ticket.discovered_from);
        ticket.intent = intent;
        store.append_ticket(&ticket)?;
        id
    };
    if options.has("json") {
        context.json(Envelope::new(true, "create").string("id", &id));
    } else {
        context.success(format!("Created {id}").as_bytes());
    }
    Ok(())
}
