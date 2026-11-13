//! `blocked` (Go's `runBlocked`, `internal/cli/issue_query.go:109-139`).
//!
//! Suspected defect of Go copied: SD208 (the tickets are listed in file
//! order, `in_progress` ones included, and `--limit` is an unknown option;
//! the foundation's SD9 said otherwise and is corrected in this plan's
//! notes).

use super::ready::render_json;
use crate::command::output::format_mode;
use crate::command::plan_index;
use crate::command::read::{
    closed_ids, compact_ticket, effectively_blocked, one_line_ticket, println,
};
use crate::command::{options, Context, Failure};
use crate::model::Ticket;

/// Go's `runBlocked`.
pub(super) fn run(context: &mut Context<'_>, args: &[String]) -> Result<(), Failure> {
    let options = options::parse(args, &["format"], &["json"])?;
    let mode = format_mode(&options).map_err(Failure::message)?;
    if !options.positionals.is_empty() {
        return Err(Failure::message("too many arguments for 'blocked'"));
    }
    let store = context.store()?;
    let all = store.read_tickets()?;
    let plans = plan_index::load(&store)?;
    let closed = closed_ids(&all);
    let blocked: Vec<Ticket> = all
        .iter()
        .filter(|ticket| ticket.status != "closed" && effectively_blocked(ticket, &closed))
        .cloned()
        .collect();
    if mode == "json" {
        return render_json(context, "blocked", &blocked, &plans);
    }
    if blocked.is_empty() && mode != "ids" && mode != "compact" {
        println(context, "No blocked issues.");
        return Ok(());
    }
    for ticket in &blocked {
        let line = match mode {
            "ids" => ticket.id.clone(),
            "compact" => compact_ticket(ticket, &closed),
            _ => one_line_ticket(ticket, &closed) + &plans.plan_suffix(ticket),
        };
        println(context, &line);
    }
    if mode == "ids" || mode == "compact" {
        return Ok(());
    }
    let line = format!("\n{} blocked issue(s)", blocked.len());
    println(context, &line);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::ready::fixture::{go, project};

    #[test]
    fn blocked_lists_in_file_order_with_in_progress() {
        let temp = project();
        let root = temp.path();
        // File order, not sorted; `k` is in progress (icon `>`) and `e` has
        // a draft plan.
        assert_eq!(
            go(root, &["blocked", "--format", "ids"]),
            ("b\ne\nk\n".to_owned(), String::new(), 0)
        );
        assert_eq!(
            go(root, &["blocked", "--format=compact"]).0,
            "b High blocked T b\ne Low blocked T e\nk High blocked T k\n"
        );
        assert_eq!(
            go(root, &["blocked", "--json"]).0,
            "{\n  \"success\": true,\n  \"command\": \"blocked\",\n  \"issues\": [\n    {\n      \"id\": \"b\",\n      \"title\": \"T b\",\n      \"status\": \"open\",\n      \"type\": \"task\",\n      \"priority\": 1,\n      \"createdAt\": \"c2\",\n      \"updatedAt\": \"u\",\n      \"blockedBy\": [\n        \"a\"\n      ]\n    },\n    {\n      \"id\": \"e\",\n      \"title\": \"T e\",\n      \"status\": \"open\",\n      \"type\": \"task\",\n      \"priority\": 3,\n      \"createdAt\": \"c5\",\n      \"updatedAt\": \"u\",\n      \"plan_id\": \"pl-d\",\n      \"blockedBy\": [\n        \"a\"\n      ],\n      \"plan_status\": \"draft\",\n      \"plan_children\": [\n        \"e\"\n      ]\n    },\n    {\n      \"id\": \"k\",\n      \"title\": \"T k\",\n      \"status\": \"in_progress\",\n      \"type\": \"task\",\n      \"priority\": 1,\n      \"createdAt\": \"cb\",\n      \"updatedAt\": \"u\",\n      \"blockedBy\": [\n        \"a\",\n        \"zz\"\n      ]\n    }\n  ],\n  \"count\": 3\n}\n"
        );
        // `--format json` alone gives the same payload.
        assert_eq!(
            go(root, &["blocked", "--format", "json"]).0,
            go(root, &["blocked", "--json"]).0
        );
        // --limit is not an option of blocked (SD208).
        assert_eq!(
            go(root, &["blocked", "--limit", "1"]),
            (
                String::new(),
                "error: unknown option '--limit'\n".to_owned(),
                1
            )
        );
    }

    #[test]
    fn text_modes_follow_go() {
        let temp = project();
        let root = temp.path();
        let text = "! b · T b   [High · task] [blocked]\n\
                    ! e · T e   [Low · task] [blocked] [plan in draft — run luuna-missions plan submit]\n\
                    > k · T k   [High · task] [blocked]\n\
                    \n3 blocked issue(s)\n";
        assert_eq!(go(root, &["blocked"]).0, text);
        assert_eq!(go(root, &["blocked", "--format", "plain"]).0, text);
        assert_eq!(go(root, &["blocked", "--format", "markdown"]).0, text);

        // Nothing blocked: the text modes say so, ids and compact print
        // nothing, json prints an empty list.
        let empty = tempfile::tempdir().unwrap();
        let dir = empty.path().join(".luuna/missions");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("config.yaml"), "project: \"p\"\nversion: \"1\"\n").unwrap();
        std::fs::write(
            dir.join("issues.jsonl"),
            "{\"id\":\"a\",\"title\":\"A\",\"status\":\"open\",\"type\":\"task\",\"priority\":2,\"createdAt\":\"c\",\"updatedAt\":\"u\"}\n",
        )
        .unwrap();
        let root = empty.path();
        assert_eq!(go(root, &["blocked"]).0, "No blocked issues.\n");
        assert_eq!(
            go(root, &["blocked", "--format", "plain"]).0,
            "No blocked issues.\n"
        );
        assert_eq!(go(root, &["blocked", "--format", "ids"]).0, "");
        assert_eq!(go(root, &["blocked", "--format", "compact"]).0, "");
        assert_eq!(
            go(root, &["blocked", "--json"]).0,
            "{\n  \"success\": true,\n  \"command\": \"blocked\",\n  \"issues\": [],\n  \"count\": 0\n}\n"
        );

        // Errors: the format is checked before the positionals, which come
        // before the project.
        let outside = tempfile::tempdir().unwrap();
        let error = |args: &[&str]| go(outside.path(), args).1;
        assert_eq!(
            error(&["blocked", "extra", "--format", "x"]),
            "Error: Invalid --format value: x. Valid: markdown|compact|plain|ids|json\n"
        );
        assert_eq!(
            error(&["blocked", "extra"]),
            "Error: too many arguments for 'blocked'\n"
        );
        assert_eq!(
            error(&["blocked"]),
            "Error: Not in a missions project. Run `luuna-missions init` first.\n"
        );
        assert_eq!(
            go(outside.path(), &["blocked", "extra", "--json"]).0,
            "{\n  \"success\": false,\n  \"command\": \"blocked\",\n  \"error\": \"too many arguments for 'blocked'\"\n}\n"
        );
    }
}
