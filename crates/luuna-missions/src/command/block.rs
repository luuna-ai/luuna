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
pub(super) fn add_dependency(
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

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use super::super::{run, Invocation, Outcome};

    fn record(id: &str) -> String {
        format!(
            r#"{{"id":"{id}","title":"T","status":"open","type":"task","priority":2,"createdAt":"c","updatedAt":"u"}}"#
        )
    }

    fn project() -> tempfile::TempDir {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join(".luuna/missions");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("config.yaml"), "project: \"p\"\nversion: \"1\"\n").unwrap();
        fs::write(
            dir.join("issues.jsonl"),
            format!("{}\n{}\n", record("a"), record("b")),
        )
        .unwrap();
        temp
    }

    fn block(root: &Path, args: &[&str]) -> Outcome {
        let mut all = vec!["block"];
        all.extend_from_slice(args);
        run(&Invocation::new(root, all)).unwrap()
    }

    fn issues(root: &Path) -> String {
        fs::read_to_string(root.join(".luuna/missions/issues.jsonl")).unwrap()
    }

    #[test]
    fn blocking_links_both_tickets_once() {
        let temp = project();
        for _ in 0..2 {
            let outcome = block(temp.path(), &["a", "--by", "b"]);
            assert_eq!(outcome.exit_code, 0);
            assert_eq!(
                String::from_utf8(outcome.stdout).unwrap(),
                "a is now blocked by b\n"
            );
        }
        let text = issues(temp.path());
        assert_eq!(text.matches("\"blockedBy\":[\"b\"]").count(), 1, "{text}");
        assert_eq!(text.matches("\"blocks\":[\"a\"]").count(), 1, "{text}");
        // The message is not a success line, so --quiet keeps it.
        let outcome = block(temp.path(), &["a", "--by", "b", "-q"]);
        assert_eq!(outcome.stdout, b"a is now blocked by b\n");
    }

    #[test]
    fn invalid_requests_change_nothing() {
        let temp = project();
        let before = issues(temp.path());
        for (args, message) in [
            (
                &["a"][..],
                "Usage: luuna-missions block <id> --by <blocker-id>",
            ),
            (
                &["a", "--by", ""],
                "Usage: luuna-missions block <id> --by <blocker-id>",
            ),
            (&["a", "--by", "a"], "Cannot block an issue by itself: a"),
            (&["a", "--by", "zz"], "Issue not found: zz"),
            (&["zz", "--by", "a"], "Issue not found: zz"),
        ] {
            let outcome = block(temp.path(), args);
            assert_eq!(outcome.exit_code, 1, "{args:?}");
            assert_eq!(
                String::from_utf8(outcome.stderr).unwrap(),
                format!("Error: {message}\n"),
                "{args:?}"
            );
            assert_eq!(issues(temp.path()), before, "{args:?}");
        }
    }
}
