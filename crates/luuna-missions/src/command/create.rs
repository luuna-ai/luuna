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

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use super::super::{run, Invocation, Outcome};

    fn project() -> tempfile::TempDir {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join(".luuna/missions");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("config.yaml"),
            "project: \"proj\"\nversion: \"1\"\n",
        )
        .unwrap();
        temp
    }

    fn create(root: &Path, args: &[&str]) -> Outcome {
        let mut all = vec!["create"];
        all.extend_from_slice(args);
        run(&Invocation::new(root, all)).unwrap()
    }

    fn issues(root: &Path) -> String {
        fs::read_to_string(root.join(".luuna/missions/issues.jsonl")).unwrap()
    }

    fn failed(outcome: &Outcome, message: &str) {
        assert_eq!(outcome.exit_code, 1);
        assert_eq!(
            String::from_utf8_lossy(&outcome.stderr),
            format!("Error: {message}\n")
        );
    }

    #[test]
    fn a_ticket_is_appended_with_every_flag() {
        let temp = project();
        let outcome = create(
            temp.path(),
            &[
                "--title",
                "  Hello  ",
                "--type=bug",
                "--priority",
                "p3",
                "--label",
                "A, b,,a",
                "--assignee",
                "me",
                "--body",
                "text",
                "--intent",
                "R-ABCD,r-1234",
                "--intent",
                "r-abcd",
            ],
        );
        assert_eq!(outcome.exit_code, 0);
        let stdout = String::from_utf8(outcome.stdout).unwrap();
        let id = stdout.strip_prefix("✓ Created ").unwrap().trim_end();
        assert!(id.starts_with("proj-") && id.len() == 9, "{id}");
        let line = issues(temp.path());
        assert!(line.starts_with(&format!(
            "{{\"id\":\"{id}\",\"title\":\"Hello\",\"status\":\"open\",\"type\":\"bug\",\"priority\":3,"
        )));
        assert!(line.ends_with(
            ",\"assignee\":\"me\",\"description\":\"text\",\"labels\":[\"a\",\"b\",\"a\"],\"intent\":[\"r-abcd\",\"r-1234\"]}\n"
        ), "{line}");
    }

    #[test]
    fn from_must_name_an_existing_ticket() {
        let temp = project();
        let first = create(temp.path(), &["--title", "One", "--json"]);
        let text = String::from_utf8(first.stdout).unwrap();
        let id = text.split('"').nth(9).unwrap().to_owned();
        let outcome = create(
            temp.path(),
            &["--title", "Two", "--from", &format!(" {id} ")],
        );
        assert_eq!(outcome.exit_code, 0);
        assert!(issues(temp.path()).contains(&format!("\"discoveredFrom\":\"{id}\"")));
        let before = issues(temp.path());
        failed(
            &create(temp.path(), &["--title", "Three", "--from", "nope"]),
            "--from issue not found: nope",
        );
        assert_eq!(issues(temp.path()), before);
    }

    #[test]
    fn invalid_input_is_refused_with_gos_messages() {
        let temp = project();
        failed(&create(temp.path(), &[]), "--title is required");
        failed(
            &create(temp.path(), &["--title", " "]),
            "--title is required",
        );
        failed(
            &create(temp.path(), &["--title", "t", "x"]),
            "too many arguments for 'create'",
        );
        failed(
            &create(temp.path(), &["--title", "t", "--type", "story"]),
            "Invalid --type value: story. Valid: task|bug|feature|epic",
        );
        failed(
            &create(temp.path(), &["--title", "t", "--priority", "P5"]),
            "--priority must be 0-4 or P0-P4",
        );
        failed(
            &create(temp.path(), &["--title", "t", "--intent", "idea"]),
            "Invalid --intent value: idea. Expected a roots idea id like r-a1b2",
        );
        assert_eq!(issues_if_any(temp.path()), "");
    }

    fn issues_if_any(root: &Path) -> String {
        fs::read_to_string(root.join(".luuna/missions/issues.jsonl")).unwrap_or_default()
    }
}
