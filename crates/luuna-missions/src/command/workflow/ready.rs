//! `ready` (Go's `runReady`, `internal/cli/issue_read.go:516-591`) and the
//! rendering `ready` and `blocked` share (Go's `renderIssues`,
//! `issue_read.go:379-436`).
//!
//! Suspected defects of Go copied: SD205 (a ticket whose plan is a draft is
//! listed even when blocked), SD206 (a `requires_plan` ticket is judged only
//! by the plan it seeds), SD207 (`--sort` and `--priority` errors come after
//! the store is read) and SD217 (see `PlanIndex::annotated_json`).

use super::schedule::{now_nanos, scheduled_out};
use crate::command::output::{format_mode, Envelope};
use crate::command::plan_index::{self, PlanIndex};
use crate::command::read::{
    closed_ids, compact_ticket, effectively_blocked, filter_tickets, one_line_ticket, parse_limit,
    println, query_options, sort_tickets, truncate, ISSUE_WITH_PLAN,
};
use crate::command::shared::validate_type;
use crate::command::{Context, Failure};
use crate::model::Ticket;

/// The `json` branch of Go's `renderIssues`, for `ready` and `blocked`:
/// every record goes through `issueWithPlan.MarshalJSON`.
pub(super) fn render_json(
    context: &mut Context<'_>,
    command: &str,
    tickets: &[Ticket],
    plans: &PlanIndex,
) -> Result<(), Failure> {
    let mut issues = vec![b'['];
    for (index, ticket) in tickets.iter().enumerate() {
        if index > 0 {
            issues.push(b',');
        }
        issues.extend_from_slice(&plans.annotated_json(ticket)?);
    }
    issues.push(b']');
    context.json_with_records(
        Envelope::new(true, command)
            .raw("issues", &issues)
            .integer("count", i64::try_from(tickets.len()).unwrap_or(i64::MAX)),
        ISSUE_WITH_PLAN,
    )?;
    Ok(())
}

/// Go's `renderIssues` for `ready`.
fn render(
    context: &mut Context<'_>,
    mode: &str,
    tickets: &[Ticket],
    all: &[Ticket],
    plans: &PlanIndex,
) -> Result<(), Failure> {
    if mode == "json" {
        return render_json(context, "ready", tickets, plans);
    }
    let closed = closed_ids(all);
    for ticket in tickets {
        match mode {
            "ids" => println(context, &ticket.id),
            "compact" => println(context, &compact_ticket(ticket, &closed)),
            _ => {
                let line = one_line_ticket(ticket, &closed) + &plans.plan_suffix(ticket);
                println(context, &line);
            }
        }
    }
    if mode == "ids" || mode == "compact" {
        return Ok(());
    }
    if tickets.is_empty() {
        println(context, "No ready issues.");
    } else {
        let line = format!("\n{} ready issue(s)", tickets.len());
        println(context, &line);
    }
    Ok(())
}

/// Go's `runReady`.
pub(super) fn run(context: &mut Context<'_>, args: &[String]) -> Result<(), Failure> {
    let options = query_options(args, &[], &["respect-schedule"])?;
    if !options.positionals.is_empty() {
        return Err(Failure::message("too many arguments for 'ready'"));
    }
    let mode = format_mode(&options).map_err(Failure::message)?;
    validate_type(options.value("type")).map_err(Failure::message)?;
    let limit = parse_limit(options.value("limit")).map_err(Failure::message)?;
    let store = context.store()?;
    let all = store.read_tickets()?;
    let plans = plan_index::load(&store)?;
    let closed = closed_ids(&all);
    let mut candidates = Vec::new();
    for ticket in &all {
        if ticket.status != "open" {
            continue;
        }
        if ticket.requires_plan {
            // SD206: only the plan that this ticket seeds counts.
            match plans.status_for_seed(ticket.id()) {
                None | Some("draft") => continue,
                Some(_) => {}
            }
        } else if plans.status_for_ticket(ticket) == Some("draft") {
            // SD205: listed even when blocked.
            candidates.push(ticket.clone());
            continue;
        }
        if !effectively_blocked(ticket, &closed) {
            candidates.push(ticket.clone());
        }
    }
    let mut tickets = filter_tickets(candidates, &options).map_err(Failure::message)?;
    if options.has("respect-schedule") {
        let now = now_nanos();
        tickets.retain(|ticket| !scheduled_out(ticket, now));
    }
    let sort = match options.value("sort") {
        "" => "priority",
        other => other,
    };
    sort_tickets(&mut tickets, sort).map_err(Failure::message)?;
    truncate(&mut tickets, limit);
    render(context, mode, &tickets, &all, &plans)
}

/// A project fixture shared by the tests of `ready` and `blocked`.
#[cfg(test)]
pub(super) mod fixture {
    use std::fs;
    use std::path::Path;

    use crate::command::{run, Invocation};

    /// `(id, status, priority, createdAt, extra members)`.
    const TICKETS: [(&str, &str, i64, &str, &str); 12] = [
        ("a", "open", 2, "c1", ""),
        ("b", "open", 1, "c2", r#","blockedBy":["a"]"#),
        ("c", "in_progress", 0, "c3", ""),
        (
            "d",
            "open",
            2,
            "c4",
            r#","blockedBy":["z"],"labels":["x","y"],"assignee":"me""#,
        ),
        (
            "e",
            "open",
            3,
            "c5",
            r#","plan_id":"pl-d","blockedBy":["a"]"#,
        ),
        ("f", "open", 0, "c6", r#","requires_plan":true"#),
        (
            "g",
            "open",
            1,
            "c7",
            r#","requires_plan":true,"plan_id":"pl-g""#,
        ),
        ("h", "open", 0, "c8", r#","requires_plan":true"#),
        (
            "i",
            "open",
            2,
            "c9",
            r#","extensions":{"scheduledFor":"2999-01-01"}"#,
        ),
        ("j", "open", 4, "c0", r#","plan_id":"pl-a""#),
        ("k", "in_progress", 1, "cb", r#","blockedBy":["a","zz"]"#),
        ("z", "closed", 2, "cz", ""),
    ];

    const PLANS: &str = concat!(
        r#"{"id":"pl-d","seed":"s1","template":"t","status":"draft","children":["e"],"updatedAt":"1"}"#,
        "\n",
        r#"{"id":"pl-g","seed":"g","template":"t","status":"approved","children":[],"updatedAt":"1"}"#,
        "\n",
        r#"{"id":"pl-h","seed":"h","template":"t","status":"draft","children":[],"updatedAt":"1"}"#,
        "\n",
        r#"{"id":"pl-a","seed":"s2","template":"t","status":"executing","updatedAt":"1"}"#,
        "\n",
    );

    /// A project holding [`TICKETS`] and [`PLANS`].
    pub(in crate::command::workflow) fn project() -> tempfile::TempDir {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join(".luuna/missions");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("config.yaml"), "project: \"p\"\nversion: \"1\"\n").unwrap();
        let mut issues = String::new();
        for (id, status, priority, created, extra) in TICKETS {
            issues.push_str(&format!(
                r#"{{"id":"{id}","title":"T {id}","status":"{status}","type":"task","priority":{priority},"createdAt":"{created}","updatedAt":"u"{extra}}}"#
            ));
            issues.push('\n');
        }
        fs::write(dir.join("issues.jsonl"), issues).unwrap();
        fs::write(dir.join("plans.jsonl"), PLANS).unwrap();
        temp
    }

    /// `(stdout, stderr, exit code)` of a run in `root`.
    pub(in crate::command::workflow) fn go(root: &Path, args: &[&str]) -> (String, String, i32) {
        let outcome = run(&Invocation::new(root, args.iter().copied())).unwrap();
        (
            String::from_utf8(outcome.stdout).unwrap(),
            String::from_utf8(outcome.stderr).unwrap(),
            outcome.exit_code,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::fixture::{go, project};

    #[test]
    fn selection_follows_go() {
        let temp = project();
        let root = temp.path();
        // Open tickets whose blockers are closed or absent, sorted by
        // priority then newest first; `e` is listed although blocked (SD205);
        // `f` and `h` need a non-draft plan seeded by them, `g` has one.
        let text = "- g · T g   [High · task] [plan approved]\n\
                    - i · T i   [Medium · task]\n\
                    - d · T d   [Medium · task] · @me {x, y}\n\
                    - a · T a   [Medium · task]\n\
                    ! e · T e   [Low · task] [blocked] [plan in draft — run luuna-missions plan submit]\n\
                    - j · T j   [Backlog · task] [plan executing]\n\
                    \n6 ready issue(s)\n";
        assert_eq!(go(root, &["ready"]), (text.to_owned(), String::new(), 0));
        assert_eq!(go(root, &["ready", "--format", "markdown"]).0, text);
        assert_eq!(go(root, &["ready", "--format=plain"]).0, text);
        assert_eq!(
            go(root, &["ready", "--format", "ids"]).0,
            "g\ni\nd\na\ne\nj\n"
        );
        assert_eq!(
            go(root, &["ready", "--format", "compact"]).0,
            "g High open T g\ni Medium open T i\nd Medium open T d\na Medium open T a\ne Low blocked T e\nj Backlog open T j\n"
        );
        assert_eq!(
            go(root, &["ready", "--sort", "id", "--format", "ids"]).0,
            "a\nd\ne\ng\ni\nj\n"
        );
        // --respect-schedule drops `i`; the limit applies after it.
        assert_eq!(
            go(root, &["ready", "--respect-schedule", "--limit", "3"]).0,
            "- g · T g   [High · task] [plan approved]\n\
             - d · T d   [Medium · task] · @me {x, y}\n\
             - a · T a   [Medium · task]\n\
             \n3 ready issue(s)\n"
        );
        assert_eq!(go(root, &["ready", "--limit", "0"]).0, "No ready issues.\n");
        assert_eq!(
            go(root, &["ready", "--priority", "P0"]).0,
            "No ready issues.\n"
        );
        assert_eq!(
            go(root, &["ready", "--priority", "0", "--format", "ids"]).0,
            ""
        );
        assert_eq!(
            go(
                root,
                &[
                    "ready",
                    "--label",
                    "x",
                    "--assignee",
                    "me",
                    "--format",
                    "ids"
                ]
            )
            .0,
            "d\n"
        );
        // JSON: plan context after the record's own members (g, e, j), none
        // for tickets that name no plan.
        assert_eq!(
            go(root, &["ready", "--json", "--priority", "3"]).0,
            "{\n  \"success\": true,\n  \"command\": \"ready\",\n  \"issues\": [\n    {\n      \"id\": \"e\",\n      \"title\": \"T e\",\n      \"status\": \"open\",\n      \"type\": \"task\",\n      \"priority\": 3,\n      \"createdAt\": \"c5\",\n      \"updatedAt\": \"u\",\n      \"plan_id\": \"pl-d\",\n      \"blockedBy\": [\n        \"a\"\n      ],\n      \"plan_status\": \"draft\",\n      \"plan_children\": [\n        \"e\"\n      ]\n    }\n  ],\n  \"count\": 1\n}\n"
        );
        assert_eq!(
            go(root, &["ready", "--json", "--priority", "1"]).0,
            "{\n  \"success\": true,\n  \"command\": \"ready\",\n  \"issues\": [\n    {\n      \"id\": \"g\",\n      \"title\": \"T g\",\n      \"status\": \"open\",\n      \"type\": \"task\",\n      \"priority\": 1,\n      \"createdAt\": \"c7\",\n      \"updatedAt\": \"u\",\n      \"requires_plan\": true,\n      \"plan_id\": \"pl-g\",\n      \"plan_status\": \"approved\",\n      \"plan_children\": []\n    }\n  ],\n  \"count\": 1\n}\n"
        );
        assert_eq!(
            go(root, &["ready", "--json", "--priority", "2", "--label", "q"]).0,
            "{\n  \"success\": true,\n  \"command\": \"ready\",\n  \"issues\": [],\n  \"count\": 0\n}\n"
        );
    }

    #[test]
    fn validation_order_follows_go() {
        let temp = project();
        let outside = tempfile::tempdir().unwrap();
        let error = |root: &std::path::Path, args: &[&str]| {
            let (stdout, stderr, code) = go(root, args);
            assert_eq!((stdout.as_str(), code), ("", 1), "{args:?}");
            stderr
        };
        for root in [temp.path(), outside.path()] {
            // Option parsing, then the positionals, the format, the type and
            // the limit, all before the project is looked up.
            assert_eq!(
                error(root, &["ready", "--bogus"]),
                "error: unknown option '--bogus'\n"
            );
            assert_eq!(
                error(root, &["ready", "--json", "--bogus"]),
                "error: unknown option '--bogus'\n"
            );
            assert_eq!(
                error(root, &["ready", "--limit"]),
                "Error: option '--limit' argument missing\n"
            );
            assert_eq!(
                error(root, &["ready", "extra", "--format", "x"]),
                "Error: too many arguments for 'ready'\n"
            );
            assert_eq!(
                error(root, &["ready", "--format", "x", "--type", "y"]),
                "Error: Invalid --format value: x. Valid: markdown|compact|plain|ids|json\n"
            );
            assert_eq!(
                error(root, &["ready", "--type", "y", "--limit", "z"]),
                "Error: Invalid --type value: y. Valid: task|bug|feature|epic\n"
            );
            assert_eq!(
                error(root, &["ready", "--limit", "z", "--sort", "bad"]),
                "Error: Invalid --limit \"z\": must be a non-negative integer\n"
            );
        }
        // The sort and the priority are checked after the store is read
        // (SD207): outside a project the project error wins.
        let not_in_project = "Error: Not in a missions project. Run `luuna-missions init` first.\n";
        for args in [
            &["ready", "--sort", "bad"][..],
            &["ready", "--priority", "9"],
            &["ready", "--priority", "9", "--sort", "bad"],
        ] {
            assert_eq!(error(outside.path(), args), not_in_project, "{args:?}");
        }
        assert_eq!(
            error(temp.path(), &["ready", "--sort", "bad"]),
            "Error: Invalid --sort value: bad. Valid: priority|created|updated|id\n"
        );
        assert_eq!(
            error(temp.path(), &["ready", "--priority", "9"]),
            "Error: Invalid priority \"9\": must be 0-4 or P0-P4\n"
        );
        // The priority is checked before the sort.
        assert_eq!(
            error(temp.path(), &["ready", "--priority", "9", "--sort", "bad"]),
            "Error: Invalid priority \"9\": must be 0-4 or P0-P4\n"
        );
        // With --json among the arguments the error is an envelope.
        let (stdout, stderr, code) = go(outside.path(), &["ready", "--json", "--limit", "z"]);
        assert_eq!(
            stdout,
            "{\n  \"success\": false,\n  \"command\": \"ready\",\n  \"error\": \"Invalid --limit \\\"z\\\": must be a non-negative integer\"\n}\n"
        );
        assert_eq!((stderr.as_str(), code), ("", 1));
    }
}
