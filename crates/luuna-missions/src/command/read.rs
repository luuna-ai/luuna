//! `show`, `list` and `search` with every output format
//! (`internal/cli/issue_read.go`, `internal/cli/issue_query.go`).
//!
//! Plan context is not ported until s4-missions-lib-4. A ticket whose
//! `plan_id` names a record in `plans.jsonl` carries plan context: showing
//! or listing it in a format that prints the plan (markdown, plain and json)
//! returns [`Error::Unsupported`], as does `show pl-<id>` for an id that is
//! not a ticket, because Go runs `plan show` for it. The `ids` and `compact`
//! formats never print plan context, so they are ported for such tickets.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

use super::output::{format_mode, Envelope};
use super::shared::{go_lower, split_labels, validate_status, validate_type};
use super::{options, Context, Failure};
use crate::error::Result;
use crate::json::{self, Value};
use crate::json_error::go_quote;
use crate::model::{self, Ticket};

/// The sort modes of `list` and `search` (`issueSorts`).
pub(super) const SORTS: [&str; 4] = ["priority", "created", "updated", "id"];

/// The Go type whose `MarshalJSON` method writes each record of the `list`
/// and `search` payloads, as Go's errors name it.
pub(super) const ISSUE_WITH_PLAN: &str = "*cli.issueWithPlan";

use super::plans::context::ReadPlans as Plans;

/// Go's `PriorityLabels[priority]`, else the number.
pub(super) fn priority_label(priority: i64) -> String {
    match priority {
        0 => "Critical".to_owned(),
        1 => "High".to_owned(),
        2 => "Medium".to_owned(),
        3 => "Low".to_owned(),
        4 => "Backlog".to_owned(),
        other => other.to_string(),
    }
}

/// Go's `closedIDs`.
pub(super) fn closed_ids(tickets: &[Ticket]) -> HashSet<&str> {
    tickets
        .iter()
        .filter(|ticket| ticket.status == "closed")
        .map(Ticket::id)
        .collect()
}

/// Go's `effectivelyBlocked`: a blocker that is not closed (or not there).
pub(super) fn effectively_blocked(ticket: &Ticket, closed: &HashSet<&str>) -> bool {
    ticket
        .blocked_by
        .iter()
        .flatten()
        .any(|id| !closed.contains(id.as_str()))
}

/// Go's `compactIssue`.
pub(super) fn compact_ticket(ticket: &Ticket, closed: &HashSet<&str>) -> String {
    let status = if effectively_blocked(ticket, closed) {
        "blocked"
    } else {
        ticket.status.as_str()
    };
    format!(
        "{} {} {} {}",
        ticket.id,
        priority_label(ticket.priority),
        status,
        ticket.title
    )
}

/// Go's `oneLineIssue`.
pub(super) fn one_line_ticket(ticket: &Ticket, closed: &HashSet<&str>) -> String {
    let blocked = effectively_blocked(ticket, closed);
    let icon = if ticket.status == "closed" {
        "x"
    } else if ticket.status == "in_progress" {
        ">"
    } else if blocked {
        "!"
    } else {
        "-"
    };
    let mut line = format!(
        "{icon} {} · {}   [{} · {}]",
        ticket.id,
        ticket.title,
        priority_label(ticket.priority),
        ticket.ticket_type
    );
    if !ticket.assignee.is_empty() {
        line.push_str(" · @");
        line.push_str(&ticket.assignee);
    }
    if blocked {
        line.push_str(" [blocked]");
    }
    if let Some(labels) = ticket.labels.as_ref().filter(|labels| !labels.is_empty()) {
        line.push_str(" {");
        line.push_str(&labels.join(", "));
        line.push('}');
    }
    line
}

/// Go's `fmt.Sprint` of a float64 (`%v`): shortest digits, with an exponent
/// below 1e-4 and from 1e6 on.
pub(super) fn sprint_number(number: f64) -> String {
    if number.is_nan() {
        return "NaN".to_owned();
    }
    if number.is_infinite() {
        return if number > 0.0 { "+Inf" } else { "-Inf" }.to_owned();
    }
    if number == 0.0 {
        return if number.is_sign_negative() { "-0" } else { "0" }.to_owned();
    }
    let scientific = format!("{number:e}");
    let (mantissa, exponent) = scientific.split_once('e').unwrap_or((&scientific, "0"));
    let exponent: i32 = exponent.parse().unwrap_or(0);
    if !(-4..6).contains(&exponent) {
        let sign = if exponent < 0 { '-' } else { '+' };
        return format!("{mantissa}e{sign}{:02}", exponent.abs());
    }
    format!("{number}")
}

/// Go's `fmt.Sprint` of a value decoded into `any`: `[a b]` for an array,
/// `map[k:v l:w]` for an object. The tree is walked with a heap stack, so
/// a value nested as deep as the decoder accepts does not use the call
/// stack in proportion to its depth.
pub(super) fn sprint(value: &Value) -> String {
    enum Frame<'a> {
        Array(std::slice::Iter<'a, Value>, bool),
        Object(std::collections::btree_map::Iter<'a, String, Value>, bool),
    }
    fn open<'a>(out: &mut String, value: &'a Value, stack: &mut Vec<Frame<'a>>) {
        match value {
            Value::Null => out.push_str("<nil>"),
            Value::Bool(flag) => out.push_str(if *flag { "true" } else { "false" }),
            Value::Number(number) => out.push_str(&sprint_number(*number)),
            Value::String(text) => out.push_str(text),
            Value::Array(items) => {
                out.push('[');
                stack.push(Frame::Array(items.iter(), true));
            }
            Value::Object(map) => {
                out.push_str("map[");
                stack.push(Frame::Object(map.iter(), true));
            }
        }
    }
    let mut out = String::new();
    let mut stack = Vec::new();
    open(&mut out, value, &mut stack);
    while let Some(frame) = stack.last_mut() {
        let child = match frame {
            Frame::Array(items, first) => items.next().inspect(|_| {
                if !std::mem::replace(first, false) {
                    out.push(' ');
                }
            }),
            Frame::Object(entries, first) => entries.next().map(|(key, item)| {
                if !std::mem::replace(first, false) {
                    out.push(' ');
                }
                out.push_str(key);
                out.push(':');
                item
            }),
        };
        match child {
            Some(child) => open(&mut out, child, &mut stack),
            None => {
                stack.pop();
                out.push(']');
            }
        }
    }
    out
}

/// Go's `formatIssueFull`.
pub(super) fn format_ticket_full(ticket: &Ticket) -> String {
    let mut lines = vec![
        format!("{}  {}", ticket.id, ticket.status),
        format!("Title:    {}", ticket.title),
        format!(
            "Type:     {}   Priority: {}",
            ticket.ticket_type,
            priority_label(ticket.priority)
        ),
    ];
    if !ticket.assignee.is_empty() {
        lines.push(format!("Assignee: {}", ticket.assignee));
    }
    if let Some(labels) = ticket.labels.as_ref().filter(|labels| !labels.is_empty()) {
        lines.push(format!("Labels:   {}", labels.join(", ")));
    }
    if !ticket.discovered_from.is_empty() {
        lines.push(format!("From:     {}", ticket.discovered_from));
    }
    if let Some(intent) = &ticket.intent {
        let text = match intent {
            Value::Array(items) => items.iter().map(sprint).collect::<Vec<_>>().join(", "),
            other => sprint(other),
        };
        lines.push(format!("Intent:   {text}"));
    }
    if let Some(extensions) = ticket.extensions.as_ref().filter(|map| !map.is_empty()) {
        let mut parts = Vec::new();
        for (key, value) in extensions {
            let mut encoded = Vec::new();
            if value.encode(&mut encoded, false).is_ok() {
                parts.push(format!("{key}={}", String::from_utf8_lossy(&encoded)));
            }
        }
        lines.push(format!("Extensions: {}", parts.join(" ")));
    }
    if !ticket.description.is_empty() {
        lines.push(format!("\n{}", ticket.description));
    }
    if let Some(ids) = ticket.blocked_by.as_ref().filter(|ids| !ids.is_empty()) {
        lines.push(format!("Blocked by: {}", ids.join(", ")));
    }
    if let Some(ids) = ticket.blocks.as_ref().filter(|ids| !ids.is_empty()) {
        lines.push(format!("Blocks:     {}", ids.join(", ")));
    }
    if !ticket.convoy.is_empty() {
        lines.push(format!("Convoy:   {}", ticket.convoy));
    }
    if !ticket.close_reason.is_empty() {
        lines.push(format!("Reason:   {}", ticket.close_reason));
    }
    lines.push(format!("Created:  {}", ticket.created_at));
    lines.push(format!("Updated:  {}", ticket.updated_at));
    if !ticket.closed_at.is_empty() {
        lines.push(format!("Closed:   {}", ticket.closed_at));
    }
    lines.join("\n")
}

/// Go's `parseLimit`: empty is 50; digits only.
pub(super) fn parse_limit(value: &str) -> Result<i64, String> {
    if value.is_empty() {
        return Ok(50);
    }
    let invalid = || {
        format!(
            "Invalid --limit {}: must be a non-negative integer",
            go_quote(value)
        )
    };
    if !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(invalid());
    }
    value.parse::<i64>().map_err(|_| invalid())
}

/// Go's `parseFilterPriority`: `0` to `4`, with an optional `P` or `p`,
/// exactly as `strconv.Itoa` writes the number.
pub(super) fn parse_filter_priority(value: &str) -> Result<i64, String> {
    let token = value.trim();
    let digits = if token.starts_with(['p', 'P']) {
        &token[1..]
    } else {
        token
    };
    match digits {
        "0" => Ok(0),
        "1" => Ok(1),
        "2" => Ok(2),
        "3" => Ok(3),
        "4" => Ok(4),
        _ => Err(format!(
            "Invalid priority {}: must be 0-4 or P0-P4",
            go_quote(token)
        )),
    }
}

/// Go's `filterIssues`.
pub(super) fn filter_tickets(
    tickets: Vec<Ticket>,
    options: &options::Options,
) -> Result<Vec<Ticket>, String> {
    let mut priorities: HashSet<i64> = HashSet::new();
    if !options.value("priority").is_empty() {
        for value in options.value("priority").split(',') {
            if value.trim().is_empty() {
                continue;
            }
            priorities.insert(parse_filter_priority(value)?);
        }
    }
    let mut priority_max = -1;
    if options.has("priority-max") {
        priority_max = parse_filter_priority(options.value("priority-max"))?;
    }
    let required = split_labels(options.value("label")).unwrap_or_default();
    let any = split_labels(options.value("label-any")).unwrap_or_default();
    let kind = options.value("type");
    let assignee = options.value("assignee");
    let mut filtered = Vec::with_capacity(tickets.len());
    for ticket in tickets {
        if !kind.is_empty() && ticket.ticket_type != kind {
            continue;
        }
        if !assignee.is_empty() && ticket.assignee != assignee {
            continue;
        }
        let labels: HashSet<&str> = ticket.labels.iter().flatten().map(String::as_str).collect();
        if options.has("unlabeled") && !labels.is_empty() {
            continue;
        }
        if !required.iter().all(|label| labels.contains(label.as_str())) {
            continue;
        }
        if !any.is_empty() && !any.iter().any(|label| labels.contains(label.as_str())) {
            continue;
        }
        if !priorities.is_empty() && !priorities.contains(&ticket.priority) {
            continue;
        }
        if priority_max >= 0 && ticket.priority > priority_max {
            continue;
        }
        filtered.push(ticket);
    }
    Ok(filtered)
}

/// Go's `sortIssues`: a stable sort.
pub(super) fn sort_tickets(tickets: &mut [Ticket], mode: &str) -> Result<(), String> {
    if !SORTS.contains(&mode) {
        return Err(format!(
            "Invalid --sort value: {mode}. Valid: priority|created|updated|id"
        ));
    }
    tickets.sort_by(|a, b| -> Ordering {
        match mode {
            "priority" => a
                .priority
                .cmp(&b.priority)
                .then_with(|| b.created_at.cmp(&a.created_at)),
            "created" => b.created_at.cmp(&a.created_at),
            "updated" => b.updated_at.cmp(&a.updated_at),
            _ => a.id.cmp(&b.id),
        }
    });
    Ok(())
}

/// The options `list` and `search` share (`queryOptions`).
pub(super) fn query_options(
    args: &[String],
    extra_values: &[&str],
    extra_switches: &[&str],
) -> Result<options::Options, Failure> {
    let mut values = vec![
        "type",
        "assignee",
        "label",
        "label-any",
        "priority",
        "priority-max",
        "limit",
        "sort",
        "format",
    ];
    values.extend_from_slice(extra_values);
    let mut switches = vec!["json", "unlabeled"];
    switches.extend_from_slice(extra_switches);
    Ok(options::parse(args, &values, &switches)?)
}

/// The tickets as a compact JSON array of their records.
pub(super) fn records_array(tickets: &[Ticket]) -> Result<Vec<u8>, Failure> {
    let mut out = vec![b'['];
    for (index, ticket) in tickets.iter().enumerate() {
        if index > 0 {
            out.push(b',');
        }
        out.extend_from_slice(&ticket.marshal_json()?);
    }
    out.push(b']');
    Ok(out)
}

/// Writes `text` and a newline to stdout (Go's `Fprintln`).
pub(super) fn println(context: &mut Context<'_>, text: &str) {
    context.stdout.extend_from_slice(text.as_bytes());
    context.stdout.push(b'\n');
}

/// Go's `renderIssues` for `list`.
pub(super) fn render_list(
    context: &mut Context<'_>,
    mode: &str,
    tickets: &[Ticket],
    all: &[Ticket],
    plans: &Plans,
) -> Result<(), Failure> {
    let closed = closed_ids(all);
    if mode == "json" {
        let issues = plans.annotated(tickets)?;
        context.json_with_records(
            Envelope::new(true, "list")
                .raw("issues", &issues)
                .integer("count", i64::try_from(tickets.len()).unwrap_or(i64::MAX)),
            ISSUE_WITH_PLAN,
        )?;
        return Ok(());
    }
    for ticket in tickets {
        match mode {
            "ids" => println(context, &ticket.id),
            "compact" => println(context, &compact_ticket(ticket, &closed)),
            _ => {
                println(
                    context,
                    &(one_line_ticket(ticket, &closed) + &plans.suffix(ticket)),
                );
            }
        }
    }
    if mode == "ids" || mode == "compact" {
        return Ok(());
    }
    if tickets.is_empty() {
        println(context, "No issues found.");
    } else {
        let line = format!("\n{} issue(s)", tickets.len());
        println(context, &line);
    }
    Ok(())
}

/// Go's `runList`.
pub(super) fn list(context: &mut Context<'_>, args: &[String]) -> Result<(), Failure> {
    let options = query_options(args, &["status"], &["all"])?;
    if !options.positionals.is_empty() {
        return Err(Failure::message("too many arguments for 'list'"));
    }
    let mode = format_mode(&options).map_err(Failure::message)?;
    validate_status(options.value("status")).map_err(Failure::message)?;
    validate_type(options.value("type")).map_err(Failure::message)?;
    let limit = parse_limit(options.value("limit")).map_err(Failure::message)?;
    let store = context.store()?;
    let all = store.read_tickets()?;
    let plans = Plans::load(&store)?;
    let status = options.value("status");
    let candidates: Vec<Ticket> = all
        .iter()
        .filter(|ticket| status.is_empty() || ticket.status == status)
        .filter(|ticket| !(status.is_empty() && !options.has("all") && ticket.status == "closed"))
        .cloned()
        .collect();
    let mut tickets = filter_tickets(candidates, &options).map_err(Failure::message)?;
    let sort = match options.value("sort") {
        "" => "priority",
        other => other,
    };
    sort_tickets(&mut tickets, sort).map_err(Failure::message)?;
    truncate(&mut tickets, limit);
    render_list(context, mode, &tickets, &all, &plans)
}

/// `if limit < len(issues) { issues = issues[:limit] }`.
pub(super) fn truncate(tickets: &mut Vec<Ticket>, limit: i64) {
    if let Ok(limit) = usize::try_from(limit) {
        tickets.truncate(limit);
    }
}

/// Go's `runSearch`.
pub(super) fn search(context: &mut Context<'_>, args: &[String]) -> Result<(), Failure> {
    let options = query_options(args, &["status"], &[])?;
    let mode = format_mode(&options).map_err(Failure::message)?;
    let Some(query) = options.positionals.first() else {
        return Err(Failure::message(
            "Usage: luuna-missions search <query> [filters]",
        ));
    };
    validate_status(options.value("status")).map_err(Failure::message)?;
    validate_type(options.value("type")).map_err(Failure::message)?;
    let limit = parse_limit(options.value("limit")).map_err(Failure::message)?;
    let store = context.store()?;
    let all = store.read_tickets()?;
    let plans = Plans::load(&store)?;
    let needle = go_lower(query);
    let status = options.value("status");
    let candidates: Vec<Ticket> = all
        .iter()
        .filter(|ticket| status.is_empty() || ticket.status == status)
        .filter(|ticket| {
            go_lower(&ticket.title).contains(&needle)
                || go_lower(&ticket.description).contains(&needle)
        })
        .cloned()
        .collect();
    let mut tickets = filter_tickets(candidates, &options).map_err(Failure::message)?;
    let sort = match options.value("sort") {
        "" => "priority",
        other => other,
    };
    sort_tickets(&mut tickets, sort).map_err(Failure::message)?;
    truncate(&mut tickets, limit);
    if mode == "json" {
        let issues = plans.annotated(&tickets)?;
        context.json_with_records(
            Envelope::new(true, "search")
                .string("query", query)
                .raw("issues", &issues)
                .integer("count", i64::try_from(tickets.len()).unwrap_or(i64::MAX)),
            ISSUE_WITH_PLAN,
        )?;
        return Ok(());
    }
    let closed = closed_ids(&all);
    if tickets.is_empty() && mode != "ids" && mode != "compact" {
        println(context, &format!("No issues match {}.", go_quote(query)));
        return Ok(());
    }
    for ticket in &tickets {
        let line = match mode {
            "ids" => ticket.id.clone(),
            "compact" => compact_ticket(ticket, &closed),
            _ => one_line_ticket(ticket, &closed) + &plans.suffix(ticket),
        };
        println(context, &line);
    }
    if mode == "ids" || mode == "compact" {
        return Ok(());
    }
    println(context, &format!("\n{} match(es)", tickets.len()));
    Ok(())
}

/// Go's `runShow`.
pub(super) fn show(context: &mut Context<'_>, args: &[String]) -> Result<(), Failure> {
    let options = options::parse(args, &["format"], &["json"])?;
    let mode = format_mode(&options).map_err(Failure::message)?;
    if options.positionals.is_empty() {
        return Err(Failure::message("Usage: luuna-missions show <id> [ids...]"));
    }
    let store = context.store()?;
    let tickets = store.read_tickets()?;
    let plans = Plans::load(&store)?;
    let lookup: HashMap<&str, &Ticket> =
        tickets.iter().map(|ticket| (ticket.id(), ticket)).collect();
    if let [id] = options.positionals.as_slice() {
        let Some(ticket) = lookup.get(id.as_str()).copied() else {
            if id.starts_with("pl-") {
                if mode != "markdown" && mode != "json" {
                    context.exit_code = 1;
                    context.stderr.extend_from_slice(format!("✗ luuna-missions show {id}: --format {mode} is not supported for plan ids; pass --json or use 'luuna-missions plan show {id}'\n").as_bytes());
                    return Ok(());
                }
                let mut invocation = context.invocation.clone();
                invocation.args = vec!["plan".into(), "show".into(), id.clone()];
                if mode == "json" {
                    invocation.args.push("--json".into());
                }
                let out = super::run(&invocation)?;
                context.stdout.extend(out.stdout);
                context.stderr.extend(out.stderr);
                context.exit_code = out.exit_code;
                return Ok(());
            }
            return Err(Failure::message(format!("Issue not found: {id}")));
        };
        match mode {
            "json" => {
                let record = ticket.marshal_json()?;
                let mut out = Envelope::new(true, "show").raw("issue", &record);
                if let Some((plan, children)) = plans.single(ticket, &lookup)? {
                    out = out.raw("plan", &plan).raw("plan_children", &children);
                }
                context.json_with_records(out, model::GO_TYPE)?;
            }
            "ids" => println(context, &ticket.id),
            "compact" => println(context, &compact_ticket(ticket, &closed_ids(&tickets))),
            _ => {
                println(context, &format_ticket_full(ticket));
                context
                    .stdout
                    .extend(plans.block(ticket, &lookup).as_bytes());
            }
        }
        return Ok(());
    }
    // Multi-ID show keeps successful entries and reports every missing ID.
    let mut found: Vec<Ticket> = Vec::new();
    let mut failures: Vec<(&str, String)> = Vec::new();
    for id in &options.positionals {
        if let Some(ticket) = lookup.get(id.as_str()) {
            found.push((*ticket).clone());
        } else {
            let message = if id.starts_with("pl-") {
                format!(
                    "Plan id {id} not supported in multi-id 'luuna-missions show'; run 'luuna-missions plan show {id}' instead"
                )
            } else {
                format!("Issue not found: {id}")
            };
            failures.push((id, message));
        }
    }
    if !failures.is_empty() {
        context.exit_code = 1;
    }
    if mode == "json" {
        // Go builds this payload as a map, so its keys are sorted.
        let mut envelope = Envelope::object().string("command", "show");
        if !failures.is_empty() {
            let mut errors = vec![b'['];
            for (index, (id, message)) in failures.iter().enumerate() {
                if index > 0 {
                    errors.push(b',');
                }
                errors.extend_from_slice(b"{\"error\":");
                json::quote(&mut errors, message, false);
                errors.extend_from_slice(b",\"id\":");
                json::quote(&mut errors, id, false);
                errors.push(b'}');
            }
            errors.push(b']');
            envelope = envelope.raw("errors", &errors);
        }
        let issues = records_array(&found)?;
        let mut results = vec![b'['];
        for (index, ticket) in found.iter().enumerate() {
            if index > 0 {
                results.push(b',');
            }
            results.extend(plans.result(ticket, &lookup)?);
        }
        results.push(b']');
        context.json_with_records(
            envelope
                .raw("issues", &issues)
                .raw("results", &results)
                .boolean("success", failures.is_empty()),
            model::GO_TYPE,
        )?;
        return Ok(());
    }
    let closed = closed_ids(&tickets);
    let count = found.len();
    for (index, ticket) in found.iter().enumerate() {
        if mode == "markdown" && index > 0 {
            context
                .stdout
                .extend_from_slice(format!("\n{}\n\n", "─".repeat(60)).as_bytes());
        }
        let line = match mode {
            "ids" => ticket.id.clone(),
            "compact" => compact_ticket(ticket, &closed),
            _ => format_ticket_full(ticket) + &plans.block(ticket, &lookup),
        };
        println(context, &line);
        if mode == "plain" && index + 1 < count {
            println(context, "");
        }
    }
    for (id, message) in &failures {
        context
            .stderr
            .extend_from_slice(format!("✗ {id}: {message}\n").as_bytes());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;
    use std::path::Path;

    use super::super::{run, Invocation, Outcome};
    use super::*;

    fn record(id: &str, extra: &str) -> String {
        format!(
            r#"{{"id":"{id}","title":"T {id}","status":"open","type":"task","priority":2,"createdAt":"c","updatedAt":"u"{extra}}}"#
        )
    }

    fn project(issues: &[String], plans: &str) -> tempfile::TempDir {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join(".luuna/missions");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("config.yaml"), "project: \"p\"\nversion: \"1\"\n").unwrap();
        fs::write(dir.join("issues.jsonl"), format!("{}\n", issues.join("\n"))).unwrap();
        fs::write(dir.join("plans.jsonl"), plans).unwrap();
        temp
    }

    fn go(root: &Path, args: &[&str]) -> Result<Outcome> {
        run(&Invocation::new(root, args.iter().copied()))
    }

    fn text(bytes: &[u8]) -> String {
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    const PLAN: &str = r#"{"id":"pl-1","seed":"a","template":"t","status":"draft","children":[]}"#;

    #[test]
    fn plan_context_and_plan_ids_are_refused() {
        let temp = project(
            &[record("a", r#","plan_id":"pl-1""#), record("b", "")],
            &format!("{PLAN}\n"),
        );
        let root = temp.path();
        super::super::plans::foundation_checks::read(root, &["show", "a"]);
        super::super::plans::foundation_checks::read(root, &["show", "a", "--json"]);
        super::super::plans::foundation_checks::read(root, &["show", "a", "--format", "plain"]);
        super::super::plans::foundation_checks::read(root, &["show", "a", "b"]);
        super::super::plans::foundation_checks::read(root, &["show", "b", "a", "--json"]);
        super::super::plans::foundation_checks::read(root, &["list"]);
        super::super::plans::foundation_checks::read(root, &["list", "--json"]);
        super::super::plans::foundation_checks::read(root, &["search", "T", "--json"]);
        super::super::plans::foundation_checks::read(root, &["search", "T"]);
        super::super::plans::foundation_checks::read(root, &["show", "pl-1"]);
        super::super::plans::foundation_checks::read(root, &["show", "pl-1", "--json"]);
        super::super::plans::foundation_checks::read(root, &["show", "pl-zz", "--format", "ids"]);
        // Formats that never print plan context, a filter that hides the
        // ticket, and a plan id that names no plan are ported.
        assert_eq!(
            text(&go(root, &["list", "--format", "ids"]).unwrap().stdout),
            "a\nb\n"
        );
        assert_eq!(
            text(&go(root, &["show", "a", "--format", "ids"]).unwrap().stdout),
            "a\n"
        );
        assert_eq!(
            text(&go(root, &["list", "--limit", "0"]).unwrap().stdout),
            "No issues found.\n"
        );
        let temp = project(&[record("a", r#","plan_id":"pl-9""#)], &format!("{PLAN}\n"));
        let outcome = go(temp.path(), &["show", "a"]).unwrap();
        assert_eq!(outcome.exit_code, 0);
    }

    #[test]
    fn a_missing_ticket_and_multi_id_failures_follow_go() {
        let temp = project(&[record("a", "")], "");
        let root = temp.path();
        let outcome = go(root, &["show", "zz"]).unwrap();
        assert_eq!(outcome.exit_code, 1);
        assert_eq!(text(&outcome.stderr), "Error: Issue not found: zz\n");
        let outcome = go(root, &["show", "a", "pl-2", "zz", "--format", "ids"]).unwrap();
        assert_eq!(outcome.exit_code, 1);
        assert_eq!(text(&outcome.stdout), "a\n");
        assert_eq!(
            text(&outcome.stderr),
            "✗ pl-2: Plan id pl-2 not supported in multi-id 'luuna-missions show'; run 'luuna-missions plan show pl-2' instead\n✗ zz: Issue not found: zz\n"
        );
    }

    #[test]
    fn filters_report_go_messages() {
        let temp = project(&[record("a", "")], "");
        for (args, message) in [
            (
                &["list", "--limit", "x"][..],
                "Invalid --limit \"x\": must be a non-negative integer",
            ),
            (
                &["list", "--priority", "P9"],
                "Invalid priority \"P9\": must be 0-4 or P0-P4",
            ),
            (
                &["list", "--priority-max", ""],
                "Invalid priority \"\": must be 0-4 or P0-P4",
            ),
            (
                &["list", "--sort", "x"],
                "Invalid --sort value: x. Valid: priority|created|updated|id",
            ),
            (&["list", "x"], "too many arguments for 'list'"),
            (
                &["search"],
                "Usage: luuna-missions search <query> [filters]",
            ),
            (&["show"], "Usage: luuna-missions show <id> [ids...]"),
        ] {
            let outcome = go(temp.path(), args).unwrap();
            assert_eq!(outcome.exit_code, 1, "{args:?}");
            assert_eq!(
                text(&outcome.stderr),
                format!("Error: {message}\n"),
                "{args:?}"
            );
        }
    }

    #[test]
    fn go_number_printing() {
        for (number, expected) in [
            (1.0, "1"),
            (1.5, "1.5"),
            (100_000.0, "100000"),
            (1_000_000.0, "1e+06"),
            (1_234_567.0, "1.234567e+06"),
            (1e20, "1e+20"),
            (0.0001, "0.0001"),
            (0.00001, "1e-05"),
            (-3.0, "-3"),
            (0.0, "0"),
        ] {
            assert_eq!(sprint_number(number), expected);
        }
    }

    #[test]
    fn intents_and_extensions_print_as_go_prints_them() {
        let temp = project(
            &[record(
                "a",
                r#","intent":["r-aaaa",2,null],"extensions":{"z":{"b":1,"a":"<x>"},"k":[1,"s"]}"#,
            )],
            "",
        );
        let outcome = go(temp.path(), &["show", "a", "--format", "plain"]).unwrap();
        let shown = text(&outcome.stdout);
        assert!(shown.contains("Intent:   r-aaaa, 2, <nil>\n"), "{shown}");
        assert!(
            shown.contains("Extensions: k=[1,\"s\"] z={\"a\":\"<x>\",\"b\":1}\n"),
            "{shown}"
        );
    }

    #[test]
    fn sorting_and_limits_follow_go() {
        let temp = project(&[record("b", ""), record("a", "")], "");
        let ids = |args: &[&str]| text(&go(temp.path(), args).unwrap().stdout);
        // Equal priority and creation time keep the file order.
        assert_eq!(ids(&["list", "--format", "ids"]), "b\na\n");
        assert_eq!(ids(&["list", "--format", "ids", "--sort", "id"]), "a\nb\n");
        assert_eq!(ids(&["list", "--format", "ids", "--limit", "1"]), "b\n");
        assert_eq!(
            ids(&["search", "T A", "--format", "compact"]),
            "a Medium open T a\n"
        );
    }

    #[test]
    fn blocked_tickets_show_as_blocked() {
        let temp = project(&[record("a", r#","blockedBy":["b"]"#), record("b", "")], "");
        let outcome = go(temp.path(), &["list", "--format", "plain"]).unwrap();
        assert_eq!(
            text(&outcome.stdout),
            "! a · T a   [Medium · task] [blocked]\n- b · T b   [Medium · task]\n\n2 issue(s)\n"
        );
    }

    #[test]
    fn empty_containers_print_as_go_prints_them() {
        assert_eq!(sprint(&Value::Object(BTreeMap::new())), "map[]");
        assert_eq!(sprint(&Value::Array(Vec::new())), "[]");
    }
}
