//! Read-only context and presentation commands.
//!
//! This module owns the CLI boundary for the context/prime/show/blame/queue/
//! view family.  Pure document construction stays in `roots-core`; terminal
//! color and the stable JSON command envelope stay here.

use crate::registry::CommandContext;
use roots_core::args::{flag_bool, flag_string};
use roots_core::attention::{collect_attention, is_due, rank_attention};
use roots_core::blame::build_blame;
use roots_core::boundary::is_human_path;
use roots_core::color::{Colors, pad_end};
use roots_core::config::load_config;
use roots_core::errors::{RootsError, RootsResult};
use roots_core::events::{contributors, events_for, read_events};
use roots_core::graph::find_node;
use roots_core::integrations::{
    atlas_learnings_for, linked_issues, read_missions_issues, ready_to_mark_built,
};
use roots_core::model::{
    EdgeRecord, EdgeRel, NodeKind, NodeRecord, NodeStatus, ProposalKind, ProposalRecord,
    QuestionRecord, QuestionStatus, SproutStatus,
};
use roots_core::notes::{NoteEntry, list_notes};
use roots_core::presentation::{
    EndorsedView, RepoState, build_context, build_prime, build_view, edge_label, endorsed_markdown,
    relative_path, render_context_markdown, render_prime_markdown, render_view_html,
    render_view_markdown, view_data_value,
};
use roots_core::proposals::{pending_proposals, read_proposals};
use roots_core::prose::read_node_prose;
use roots_core::questions::read_questions;
use roots_core::resolve::{ResolveOptions, resolve_node};
use roots_core::rules::rule_of;
use roots_core::serde_json::{Map, Value, to_value};
use roots_core::sprouts::{is_sprout_overdue, open_sprouts};
use roots_core::store::atomic_write;
use roots_core::time::short_time;
use roots_core::time_crate::{OffsetDateTime, format_description::well_known::Rfc3339};
use roots_core::workspace::open_workspace;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const EXPIRING_SOON_DAYS: i64 = 3;
const GUTTER: usize = 8;
const EDGE_LABEL_WIDTH: usize = 11;
const ROOTS_MD: &str = "ROOTS.md";

fn json<T: roots_core::serde::Serialize>(value: T) -> RootsResult<Value> {
    to_value(value).map_err(|error| RootsError::error(error.to_string()))
}

fn status_color(colors: Colors, status: NodeStatus) -> String {
    match status.as_str() {
        "planted" => colors.green(status.as_str()),
        "shaping" => colors.cyan(status.as_str()),
        "committed" => colors.magenta(status.as_str()),
        "built" => colors.blue(status.as_str()),
        "open" => colors.yellow(status.as_str()),
        _ => colors.dim(status.as_str()),
    }
}

fn format_node_ref(colors: Colors, node: &NodeRecord) -> String {
    let reference = format!("{} {}", colors.id(&node.id), node.slug);
    if node.kind == NodeKind::Sprout {
        format!("{} {}", colors.dim("[agent]"), colors.dim(&reference))
    } else {
        reference
    }
}

fn proposal_kind_name(kind: ProposalKind) -> &'static str {
    match kind {
        ProposalKind::Edge => "edge",
        ProposalKind::Split => "split",
        ProposalKind::Merge => "merge",
        ProposalKind::Compost => "compost",
    }
}

fn git(root: &Path, args: &[&str], environment: roots_core::actor::Environment) -> Option<String> {
    let mut child = Command::new("git")
        .args(args)
        .current_dir(root)
        .envs(environment)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let start = Instant::now();
    loop {
        match child.try_wait().ok()? {
            Some(_) => break,
            None if start.elapsed() >= Duration::from_secs(2) => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            None => thread::sleep(Duration::from_millis(10)),
        }
    }
    let output = child.wait_with_output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn repo_state(root: &Path, environment: roots_core::actor::Environment) -> Option<RepoState> {
    if git(
        root,
        &["rev-parse", "--is-inside-work-tree"],
        environment.clone(),
    )
    .as_deref()
        != Some("true")
    {
        return None;
    }
    let branch = git(
        root,
        &["symbolic-ref", "--short", "HEAD"],
        environment.clone(),
    )
    .filter(|value| !value.is_empty());
    let commits = git(root, &["log", "-n5", "--format=%h %s"], environment)
        .map(|value| {
            value
                .lines()
                .filter(|line| !line.is_empty())
                .map(ToOwned::to_owned)
                .collect()
        })
        .unwrap_or_default();
    Some(RepoState { branch, commits })
}

/// `luuna-roots context <id>`.
pub fn context_command(context: CommandContext<'_>) -> RootsResult<()> {
    let workspace = open_workspace(context.io.cwd())?;
    let node = resolve_node(
        &workspace.graph.nodes,
        context.args.first().map(String::as_str).unwrap_or_default(),
        ResolveOptions::default(),
    )?;
    let session = if let Some(value) = flag_string(context.flags, "session") {
        (!value.is_empty()).then(|| value.to_owned())
    } else {
        context
            .io
            .env("ROOTS_SESSION")
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
    };
    let repo = (!flag_bool(context.flags, "no-repo"))
        .then(|| repo_state(&workspace.paths.root, context.io.environment()))
        .flatten();
    let packet = build_context(
        &workspace.paths,
        &workspace.graph,
        &load_config(&workspace.paths)?,
        node,
        session,
        repo,
    )?;
    let data = json(&packet)?;
    let Value::Object(data) = data else {
        return Err(RootsError::error(
            "context packet did not serialize to an object",
        ));
    };
    context.out.result(context.io, data);
    if !context.out.json {
        context.io.stdout(&render_context_markdown(&packet));
    }
    Ok(())
}

/// `luuna-roots prime [--scope <id>]`.
pub fn prime_command(context: CommandContext<'_>) -> RootsResult<()> {
    if flag_bool(context.flags, "hook")
        && roots_core::paths::find_project_root(context.io.cwd()).is_none()
    {
        let mut data = Map::new();
        data.insert("project".to_owned(), Value::Null);
        data.insert(
            "skipped".to_owned(),
            Value::String("not a roots project".to_owned()),
        );
        context.out.result(context.io, data);
        return Ok(());
    }
    let workspace = open_workspace(context.io.cwd())?;
    let scope = flag_string(context.flags, "scope")
        .map(|query| {
            resolve_node(
                &workspace.graph.nodes,
                query,
                ResolveOptions {
                    kind: Some(NodeKind::Idea),
                },
            )
        })
        .transpose()?;
    let data = build_prime(
        &workspace.paths,
        &workspace.graph,
        &load_config(&workspace.paths)?,
        scope,
    )?;
    let body = json(&data)?;
    let Value::Object(body) = body else {
        return Err(RootsError::error(
            "prime data did not serialize to an object",
        ));
    };
    context.out.result(context.io, body);
    if !context.out.json {
        context
            .io
            .stdout(&render_prime_markdown(&workspace.graph, &data));
    }
    Ok(())
}

#[derive(Clone, Debug, roots_core::serde::Serialize)]
struct ShowNeighborNode {
    id: String,
    kind: NodeKind,
    slug: String,
    status: NodeStatus,
    statement: String,
}

#[derive(Clone, Debug, roots_core::serde::Serialize)]
struct ShowNeighbor {
    edge: EdgeRecord,
    direction: String,
    node: Option<ShowNeighborNode>,
}

struct ShowView {
    node: NodeRecord,
    notes: Vec<NoteEntry>,
    seeds: Vec<roots_core::integrations::LinkedIssue>,
    mulch: Vec<roots_core::integrations::AtlasLearning>,
    now: OffsetDateTime,
    prose: roots_core::prose::NodeProse,
    endorsed: Option<EndorsedView>,
    edges: Vec<ShowNeighbor>,
    questions: Vec<QuestionRecord>,
    proposals: Vec<ProposalRecord>,
    graph: roots_core::model::Graph,
    contributors: Vec<String>,
    path: Option<String>,
}

fn view_sprout(node: &NodeRecord, now: OffsetDateTime) -> NodeRecord {
    let mut view = node.clone();
    if is_sprout_overdue(&view, now) {
        view.status = NodeStatus::Sprout(SproutStatus::Expired);
    }
    view
}

fn show_neighbors(
    paths: &roots_core::paths::RootsPaths,
    graph: &roots_core::model::Graph,
    node: &NodeRecord,
) -> RootsResult<Vec<ShowNeighbor>> {
    graph
        .edges
        .iter()
        // The public command intentionally makes two passes: outgoing then
        // incoming, rather than retaining interleaved graph-file order.
        .filter(|edge| edge.from == node.id)
        .chain(graph.edges.iter().filter(|edge| edge.to == node.id))
        .map(|edge| {
            let direction = if edge.from == node.id { "out" } else { "in" };
            let other_id = if direction == "out" {
                &edge.to
            } else {
                &edge.from
            };
            let other = find_node(graph, other_id);
            let adjacent = other
                .map(|other| {
                    let prose = read_node_prose(paths, other, None)?;
                    Ok(ShowNeighborNode {
                        id: other.id.clone(),
                        kind: other.kind,
                        slug: other.slug.clone(),
                        status: other.status,
                        statement: prose.statement,
                    })
                })
                .transpose()?;
            Ok(ShowNeighbor {
                edge: edge.clone(),
                direction: direction.to_owned(),
                node: adjacent,
            })
        })
        .collect()
}

fn read_endorsed_view(
    paths: &roots_core::paths::RootsPaths,
    graph: &roots_core::model::Graph,
    node: &NodeRecord,
) -> RootsResult<Option<EndorsedView>> {
    let claim = roots_core::claims::read_claim(paths, graph, node)?;
    Ok(claim.endorsed.as_ref().map(|endorsed| {
        let sprout = find_node(graph, &endorsed.sprout);
        EndorsedView {
            sprout: endorsed.sprout.clone(),
            slug: sprout.map(|row| row.slug.clone()).unwrap_or_default(),
            by: sprout
                .map(|row| row.author.clone())
                .unwrap_or_else(|| "agent:?".to_owned()),
            statement: endorsed.statement.clone(),
            body: endorsed.body.clone(),
            changed: endorsed.changed,
        }
    }))
}

fn claim_statement(own: &str, endorsed: Option<&EndorsedView>) -> String {
    endorsed
        .filter(|value| !value.statement.is_empty())
        .map(|value| value.statement.clone())
        .unwrap_or_else(|| own.to_owned())
}

fn days_left(expires_at: Option<&str>, now: OffsetDateTime) -> Option<i64> {
    let expiry = OffsetDateTime::parse(expires_at?, &Rfc3339).ok()?;
    const DAY_NANOS: i128 = 86_400_000_000_000;
    let nanos = (expiry - now).whole_nanoseconds();
    let days = if nanos >= 0 {
        (nanos + DAY_NANOS - 1) / DAY_NANOS
    } else {
        nanos / DAY_NANOS
    };
    Some(std::cmp::max(0, days) as i64)
}

fn format_show_edge(colors: Colors, neighbor: &ShowNeighbor) -> String {
    let (label, arrow) = edge_label(neighbor.edge.rel, &neighbor.direction);
    let other_id = if neighbor.direction == "out" {
        &neighbor.edge.to
    } else {
        &neighbor.edge.from
    };
    let target = neighbor.node.as_ref().map_or_else(
        || format!("{} {}", colors.id(other_id), colors.red("(missing)")),
        |node| {
            let value = NodeRecord {
                record_type: "node".to_owned(),
                id: node.id.clone(),
                kind: node.kind,
                slug: node.slug.clone(),
                status: node.status,
                author: String::new(),
                created_at: String::new(),
                updated_at: None,
                tier: None,
                expires_at: None,
                decided_by: None,
                decided_at: None,
                decision_reason: None,
                endorses: None,
            };
            format_node_ref(colors, &value)
        },
    );
    let via = neighbor
        .edge
        .proposed_by
        .as_ref()
        .map(|by| format!(", proposed by {by}"))
        .unwrap_or_default();
    format!(
        "{}{} {target}{}",
        pad_end(label, EDGE_LABEL_WIDTH),
        arrow,
        colors.dim(&format!("  ({}{})", neighbor.edge.id, via))
    )
}

fn show_question_lines(colors: Colors, questions: &[QuestionRecord]) -> Vec<String> {
    let open = questions
        .iter()
        .filter(|question| question.status == QuestionStatus::Open)
        .collect::<Vec<_>>();
    if questions.is_empty() {
        return Vec::new();
    }
    let mut lines = vec![colors.bold(&format!(
        "Questions ({} open, {} total)",
        open.len(),
        questions.len()
    ))];
    for question in open {
        let tag = if question.by.starts_with("agent:") {
            colors.dim(" [agent]")
        } else {
            String::new()
        };
        lines.push(format!(
            "  ? {} {}{}",
            colors.id(&question.id),
            question.text,
            tag
        ));
    }
    lines
}

fn show_proposal_lines(
    colors: Colors,
    graph: &roots_core::model::Graph,
    node: &NodeRecord,
    proposals: &[ProposalRecord],
) -> Vec<String> {
    if proposals.is_empty() {
        return Vec::new();
    }
    let mut lines = vec![format!(
        "{}{}",
        colors.bold(&format!("Pending proposals ({})", proposals.len())),
        colors.dim("  (luuna-roots tend)")
    )];
    for proposal in proposals {
        let other_id = if proposal.from.as_deref() == Some(node.id.as_str()) {
            proposal.to.as_deref()
        } else {
            proposal.from.as_deref()
        };
        let other = other_id
            .map(|id| {
                format!(
                    " {}",
                    find_node(graph, id)
                        .map(|node| node.slug.as_str())
                        .unwrap_or(id)
                )
            })
            .unwrap_or_default();
        let what = if proposal.kind == ProposalKind::Edge {
            format!(
                "{}{}",
                proposal.rel.map(EdgeRel::as_str).unwrap_or("?"),
                other
            )
        } else {
            format!("{}{}", proposal_kind_name(proposal.kind), other)
        };
        lines.push(format!(
            "  {} {what}  {}",
            colors.id(&proposal.id),
            colors.dim(&proposal.by)
        ));
    }
    lines
}

fn sprout_meta(colors: Colors, node: &NodeRecord, now: OffsetDateTime) -> Vec<String> {
    if node.kind != NodeKind::Sprout {
        return Vec::new();
    }
    let mut lines = vec![format!(
        "{} {} {}",
        colors.dim("author"),
        node.author,
        colors.dim("[agent]")
    )];
    match node.status {
        NodeStatus::Sprout(SproutStatus::Open) => {
            if let Some(days) = days_left(node.expires_at.as_deref(), now) {
                let date = node
                    .expires_at
                    .as_deref()
                    .and_then(|value| value.get(..10))
                    .unwrap_or_default();
                lines.push(format!("{} in {days}d ({date})", colors.dim("expires")));
            }
            lines.push(colors.dim(&format!(
                "  adopt: luuna-roots adopt {} · reject: luuna-roots reject {}",
                node.id, node.id
            )));
        }
        NodeStatus::Sprout(SproutStatus::Expired) => {
            let date = node
                .expires_at
                .as_deref()
                .and_then(|value| value.get(..10))
                .unwrap_or_default();
            lines.push(format!("{} {date}", colors.dim("expired")));
        }
        _ => {
            if let Some(by) = &node.decided_by {
                let why = node
                    .decision_reason
                    .as_ref()
                    .map(|reason| format!(" (\"{reason}\")"))
                    .unwrap_or_default();
                let at = node
                    .decided_at
                    .as_deref()
                    .and_then(|value| value.get(..10))
                    .unwrap_or_default();
                lines.push(format!(
                    "{} by {by} {at}{why}",
                    colors.dim(node.status.as_str())
                ));
            }
        }
    }
    lines
}

fn show_note_lines(colors: Colors, notes: &[NoteEntry]) -> Vec<String> {
    if notes.is_empty() {
        return Vec::new();
    }
    let mut lines = vec![colors.bold(&format!("Agent notes ({})", notes.len()))];
    lines.extend(notes.iter().map(|note| {
        format!(
            "  {} {}  {}",
            colors.dim("[agent]"),
            note.path,
            colors.dim(&format!("{} B", note.bytes))
        )
    }));
    lines
}

fn show_seed_lines(
    colors: Colors,
    issues: &[roots_core::integrations::LinkedIssue],
) -> Vec<String> {
    if issues.is_empty() {
        return Vec::new();
    }
    let closed = issues.iter().filter(|issue| issue.closed).count();
    let mut lines = vec![colors.bold(&format!(
        "Missions issues ({closed}/{} closed)",
        issues.len()
    ))];
    for issue in issues {
        let mark = if issue.closed {
            colors.green("✓")
        } else {
            "○".to_owned()
        };
        let via = if issue.via == "mention" {
            colors.dim("  (mentions)")
        } else {
            String::new()
        };
        lines.push(format!(
            "  {mark} {} {} {}{via}",
            colors.id(&issue.id),
            colors.dim(&format!("[{}]", issue.status)),
            issue.title
        ));
    }
    lines
}

fn show_mulch_lines(
    colors: Colors,
    learnings: &[roots_core::integrations::AtlasLearning],
) -> Vec<String> {
    if learnings.is_empty() {
        return Vec::new();
    }
    let mut lines = vec![colors.bold(&format!("Atlas expertise ({})", learnings.len()))];
    for learning in learnings {
        let id = learning
            .id
            .as_ref()
            .map(|id| format!("{} ", colors.id(id)))
            .unwrap_or_default();
        lines.push(format!(
            "  {id}{}  {}",
            colors.dim(&format!("{}/{}", learning.domain, learning.record_type)),
            learning.summary
        ));
    }
    lines
}

fn show_prose_lines(colors: Colors, view: &ShowView) -> Vec<String> {
    let mut lines = Vec::new();
    if !view.prose.exists {
        lines.push(colors.red("(prose file missing)"));
    }
    if let Some(endorsed) = &view.endorsed {
        lines.push(format!(
            "\"{}\"",
            claim_statement(&view.prose.statement, Some(endorsed))
        ));
        lines.push(String::new());
        lines.extend(
            endorsed_markdown(endorsed, Some(&view.node.author))
                .into_iter()
                .map(|line| colors.dim(&line)),
        );
        if !view.prose.text.trim().is_empty() {
            lines.extend([
                String::new(),
                colors.dim("thoughts"),
                view.prose.text.trim().to_owned(),
            ]);
        }
        return lines;
    }
    if view.prose.exists {
        lines.push(format!("\"{}\"", view.prose.statement));
    }
    if !view.prose.body.is_empty() {
        lines.extend([
            String::new(),
            if view.node.kind == NodeKind::Sprout {
                colors.dim(&view.prose.body)
            } else {
                view.prose.body.clone()
            },
        ]);
    }
    lines
}

fn render_show(colors: Colors, view: &ShowView) -> Vec<String> {
    let agent = if view.node.kind == NodeKind::Sprout {
        format!("{} ", colors.dim("[agent]"))
    } else {
        String::new()
    };
    let mut lines = vec![format!(
        "{agent}{}  {}  {}",
        colors.id(&colors.bold(&view.node.id)),
        view.node.slug,
        status_color(colors, view.node.status)
    )];
    lines.extend(show_prose_lines(colors, view));
    let blocks = vec![
        sprout_meta(colors, &view.node, view.now),
        view.edges
            .iter()
            .map(|edge| format_show_edge(colors, edge))
            .collect(),
        show_note_lines(colors, &view.notes),
        show_seed_lines(colors, &view.seeds),
        show_mulch_lines(colors, &view.mulch),
        show_question_lines(colors, &view.questions),
        show_proposal_lines(colors, &view.graph, &view.node, &view.proposals),
    ];
    for block in blocks {
        if !block.is_empty() {
            lines.push(String::new());
            lines.extend(block);
        }
    }
    lines.push(String::new());
    lines.push(format!(
        "{}    {}",
        colors.dim("by"),
        view.contributors.join(", ")
    ));
    if let Some(path) = &view.path {
        lines.push(format!("{}  {path}", colors.dim("file")));
    }
    if let Some(tier) = view.node.tier {
        lines.push(format!(
            "{}  {tier} (per-idea override; `luuna-roots tier {} default` clears it)",
            colors.dim("tier"),
            view.node.id
        ));
    }
    lines
}

/// `luuna-roots show <id>`.
pub fn show_command(context: CommandContext<'_>) -> RootsResult<()> {
    let workspace = open_workspace(context.io.cwd())?;
    let now = OffsetDateTime::now_utc();
    let resolved = resolve_node(
        &workspace.graph.nodes,
        context.args.first().map(String::as_str).unwrap_or_default(),
        ResolveOptions::default(),
    )?;
    let node = view_sprout(resolved, now);
    let prose = read_node_prose(&workspace.paths, &node, Some(&workspace.dirs))?;
    let events = read_events(&workspace.paths)?;
    let idea = node.kind == NodeKind::Idea;
    let questions = read_questions(&workspace.paths)?
        .into_iter()
        .filter(|question| question.node == node.id)
        .collect::<Vec<_>>();
    let proposal_rows = read_proposals(&workspace.paths)?;
    let proposals = pending_proposals(&proposal_rows, now, Some(&workspace.graph))
        .into_iter()
        .filter(|proposal| {
            proposal.from.as_deref() == Some(node.id.as_str())
                || proposal.to.as_deref() == Some(node.id.as_str())
        })
        .cloned()
        .collect::<Vec<_>>();
    let issues = read_missions_issues(&workspace.paths.root);
    let node_events = events_for(&events, &node.id)
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    let view = ShowView {
        node: node.clone(),
        now,
        notes: if idea {
            list_notes(&workspace.paths, &node)
        } else {
            Vec::new()
        },
        seeds: if idea {
            linked_issues(&issues, &node.id)
        } else {
            Vec::new()
        },
        mulch: if idea {
            atlas_learnings_for(&workspace.paths.root, &node.id)
        } else {
            Vec::new()
        },
        prose: prose.clone(),
        endorsed: if idea {
            read_endorsed_view(&workspace.paths, &workspace.graph, &node)?
        } else {
            None
        },
        edges: show_neighbors(&workspace.paths, &workspace.graph, &node)?,
        questions,
        proposals,
        graph: workspace.graph.clone(),
        contributors: contributors(&node_events, Some(&node.author)),
        path: prose
            .path
            .as_deref()
            .map(|path| relative_path(&workspace.paths.root, path)),
    };
    let mut body = Map::new();
    body.insert("node".to_owned(), json(&view.node)?);
    body.insert(
        "statement".to_owned(),
        Value::String(claim_statement(
            &view.prose.statement,
            view.endorsed.as_ref(),
        )),
    );
    let body_text = view
        .endorsed
        .as_ref()
        .filter(|endorsed| !endorsed.statement.is_empty())
        .map(|endorsed| endorsed.body.clone())
        .unwrap_or_else(|| view.prose.body.clone());
    body.insert("body".to_owned(), Value::String(body_text));
    body.insert("endorsed".to_owned(), json(&view.endorsed)?);
    body.insert(
        "thoughts".to_owned(),
        view.endorsed
            .as_ref()
            .map(|_| Value::String(view.prose.text.trim().to_owned()))
            .unwrap_or(Value::Null),
    );
    body.insert("path".to_owned(), json(&view.path)?);
    body.insert("missing".to_owned(), Value::Bool(!view.prose.exists));
    body.insert("edges".to_owned(), json(&view.edges)?);
    body.insert("questions".to_owned(), json(&view.questions)?);
    body.insert("proposals".to_owned(), json(&view.proposals)?);
    body.insert("contributors".to_owned(), json(&view.contributors)?);
    body.insert("notes".to_owned(), json(&view.notes)?);
    body.insert("seeds".to_owned(), json(&view.seeds)?);
    body.insert("mulch".to_owned(), json(&view.mulch)?);
    context.out.result(context.io, body);
    context
        .out
        .lines(context.io, &render_show(context.out.colors, &view));
    Ok(())
}

fn blame_file_lines(colors: Colors, blame: &roots_core::blame::Blame) -> Vec<String> {
    let mut previous: Option<&str> = None;
    blame
        .lines
        .iter()
        .map(|line| {
            let label = if line.question.as_deref() != previous {
                line.question.as_deref().unwrap_or_default()
            } else {
                ""
            };
            previous = line.question.as_deref();
            let mark = if line.question.is_some() {
                colors.cyan("│")
            } else {
                colors.dim("│")
            };
            format!(
                "{}{} {}",
                pad_end(&colors.id(label), GUTTER),
                mark,
                line.text
            )
            .trim_end()
            .to_owned()
        })
        .collect()
}

fn blame_answer_lines(colors: Colors, blame: &roots_core::blame::Blame) -> Vec<String> {
    if blame.answers.is_empty() {
        return vec![colors.dim("No answered questions yet. Run `luuna-roots think`.")];
    }
    let mut lines = vec![colors.bold("Questions answered")];
    for answer in &blame.answers {
        let where_ = if answer.lines > 0 {
            format!(
                "{} line{}",
                answer.lines,
                if answer.lines == 1 { "" } else { "s" }
            )
        } else {
            "no lines left (revised since)".to_owned()
        };
        lines.push(format!(
            "{}{}",
            pad_end(&colors.id(&answer.question), GUTTER),
            answer.text
        ));
        lines.push(format!(
            "{}{}",
            " ".repeat(GUTTER),
            colors.dim(&format!(
                "{} · {} · {where_}",
                answer.asked_by,
                short_time(&answer.at)
            ))
        ));
    }
    if blame.untracked > 0 {
        let plural = if blame.untracked == 1 { "" } else { "s" };
        lines.push(colors.dim(&format!(
            "{} answer{plural} predate line tracking and cannot be placed",
            blame.untracked
        )));
    }
    lines
}

/// `luuna-roots blame <id>`.
pub fn blame_command(context: CommandContext<'_>) -> RootsResult<()> {
    let workspace = open_workspace(context.io.cwd())?;
    let node = resolve_node(
        &workspace.graph.nodes,
        context.args.first().map(String::as_str).unwrap_or_default(),
        ResolveOptions {
            kind: Some(NodeKind::Idea),
        },
    )?;
    let prose = read_node_prose(&workspace.paths, node, Some(&workspace.dirs))?;
    let blame = build_blame(
        &node.id,
        &prose.text,
        &read_events(&workspace.paths)?,
        &read_questions(&workspace.paths)?,
    );
    let path = prose
        .path
        .as_deref()
        .map(|file| relative_path(&workspace.paths.root, file));
    let mut body = Map::new();
    body.insert("node".to_owned(), Value::String(node.id.clone()));
    body.insert("slug".to_owned(), Value::String(node.slug.clone()));
    body.insert("path".to_owned(), json(&path)?);
    let value = json(&blame)?;
    let Value::Object(extra) = value else {
        return Err(RootsError::error("blame did not serialize to an object"));
    };
    body.extend(extra);
    context.out.result(context.io, body);
    let mut lines = vec![
        format!(
            "{}  {}",
            format_node_ref(context.out.colors, node),
            context
                .out
                .colors
                .dim(path.as_deref().unwrap_or("(idea.md missing)"))
        ),
        String::new(),
    ];
    lines.extend(blame_file_lines(context.out.colors, &blame));
    lines.push(String::new());
    lines.extend(blame_answer_lines(context.out.colors, &blame));
    context.out.lines(context.io, &lines);
    Ok(())
}

fn slug_of(graph: &roots_core::model::Graph, id: Option<&str>) -> String {
    id.and_then(|id| find_node(graph, id).map(|node| node.slug.clone()))
        .unwrap_or_else(|| "?".to_owned())
}

fn question_source(question: &QuestionRecord) -> String {
    rule_of(question).map(ToOwned::to_owned).unwrap_or_else(|| {
        if question.by.starts_with("agent:") {
            "agent".to_owned()
        } else {
            question.by.clone()
        }
    })
}

fn is_soon(expires_at: Option<&str>, now: OffsetDateTime) -> bool {
    days_left(expires_at, now).is_some_and(|days| days <= EXPIRING_SOON_DAYS)
}

fn queue_think_lines(
    colors: Colors,
    ranked: &[roots_core::attention::IdeaAttention],
) -> Vec<String> {
    if ranked.is_empty() {
        return Vec::new();
    }
    let mut lines = vec![format!(
        "{}{}",
        colors.bold("Think next"),
        colors.dim("  (luuna-roots think)")
    )];
    for attention in ranked {
        let parts = [
            (!attention.due.is_empty()).then(|| format!("{} open", attention.due.len())),
            (!attention.candidates.is_empty())
                .then(|| format!("{} new", attention.candidates.len())),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" · ");
        lines.push(format!(
            "  {} {}  {}  {}",
            colors.id(&attention.node.id),
            attention.node.slug,
            status_color(colors, attention.node.status),
            colors.dim(&parts)
        ));
    }
    lines
}

fn queue_question_lines(
    colors: Colors,
    graph: &roots_core::model::Graph,
    questions: &[QuestionRecord],
) -> Vec<String> {
    if questions.is_empty() {
        return Vec::new();
    }
    let mut lines = vec![colors.bold(&format!("Open questions ({})", questions.len()))];
    let mut last = "";
    for question in questions {
        if question.node != last {
            lines.push(format!(
                "  {} {}",
                colors.id(&question.node),
                slug_of(graph, Some(&question.node))
            ));
        }
        last = &question.node;
        lines.push(format!(
            "    ? {} {}  {}",
            colors.id(&question.id),
            question.text,
            colors.dim(&format!("[{}]", question_source(question)))
        ));
    }
    lines
}

fn queue_proposal_line(
    colors: Colors,
    graph: &roots_core::model::Graph,
    proposal: &ProposalRecord,
    now: OffsetDateTime,
) -> String {
    let what = if proposal.kind == ProposalKind::Edge {
        format!(
            "{} ─{}─ {}",
            slug_of(graph, proposal.from.as_deref()),
            proposal.rel.map(EdgeRel::as_str).unwrap_or("?"),
            slug_of(graph, proposal.to.as_deref())
        )
    } else {
        format!(
            "{} {}{}",
            proposal_kind_name(proposal.kind),
            slug_of(graph, proposal.from.as_deref()),
            proposal
                .to
                .as_deref()
                .map(|id| format!(" + {}", slug_of(graph, Some(id))))
                .unwrap_or_default()
        )
    };
    let days = days_left(proposal.expires_at.as_deref(), now);
    let expiry = days
        .map(|days| format!("expires in {days}d"))
        .unwrap_or_default();
    let expiry = if days.is_some_and(|days| days <= EXPIRING_SOON_DAYS) {
        colors.yellow(&expiry)
    } else {
        colors.dim(&expiry)
    };
    format!(
        "  {} {what}  {} {expiry}",
        colors.id(&proposal.id),
        colors.dim(&format!("{},", proposal.by))
    )
}

fn queue_proposal_lines(
    colors: Colors,
    graph: &roots_core::model::Graph,
    proposals: &[ProposalRecord],
    now: OffsetDateTime,
) -> Vec<String> {
    if proposals.is_empty() {
        return Vec::new();
    }
    let soon = proposals
        .iter()
        .filter(|proposal| is_soon(proposal.expires_at.as_deref(), now))
        .count();
    let title = format!(
        "Pending proposals ({}{})",
        proposals.len(),
        if soon > 0 {
            format!(", {soon} expiring soon")
        } else {
            String::new()
        }
    );
    let mut lines = vec![format!(
        "{}{}",
        colors.bold(&title),
        colors.dim("  (luuna-roots tend)")
    )];
    lines.extend(
        proposals
            .iter()
            .map(|proposal| queue_proposal_line(colors, graph, proposal, now)),
    );
    lines
}

fn queue_sprout_lines(colors: Colors, sprouts: &[NodeRecord], now: OffsetDateTime) -> Vec<String> {
    if sprouts.is_empty() {
        return Vec::new();
    }
    let soon = sprouts
        .iter()
        .filter(|sprout| is_soon(sprout.expires_at.as_deref(), now))
        .count();
    let title = format!(
        "Open sprouts ({}{})",
        sprouts.len(),
        if soon > 0 {
            format!(", {soon} expiring soon")
        } else {
            String::new()
        }
    );
    let mut lines = vec![format!(
        "{}{}",
        colors.bold(&title),
        colors.dim("  (luuna-roots tend · luuna-roots adopt <id>)")
    )];
    for sprout in sprouts {
        let days = days_left(sprout.expires_at.as_deref(), now);
        let expiry = days
            .map(|days| format!("expires in {days}d"))
            .unwrap_or_default();
        let expiry = if is_soon(sprout.expires_at.as_deref(), now) {
            colors.yellow(&expiry)
        } else {
            colors.dim(&expiry)
        };
        lines.push(format!(
            "  {} {} {}  {} {expiry}",
            colors.dim("[agent]"),
            colors.id(&sprout.id),
            sprout.slug,
            colors.dim(&format!("{},", sprout.author))
        ));
    }
    lines
}

fn queue_built_lines(
    colors: Colors,
    ready: &[roots_core::integrations::ReadyToBuild],
) -> Vec<String> {
    if ready.is_empty() {
        return Vec::new();
    }
    let mut lines = vec![format!(
        "{}{}",
        colors.bold(&format!("Ready to mark built ({})", ready.len())),
        colors.dim("  (all linked seeds issues closed)")
    )];
    for item in ready {
        let count = item.issues.len();
        lines.push(format!(
            "  {} {}  {} luuna-roots status {} built",
            colors.id(&item.node.id),
            item.node.slug,
            colors.dim(&format!(
                "{count} issue{} closed ·",
                if count == 1 { "" } else { "s" }
            )),
            item.node.id
        ));
    }
    lines
}

/// `luuna-roots queue`.
pub fn queue_command(context: CommandContext<'_>) -> RootsResult<()> {
    let workspace = open_workspace(context.io.cwd())?;
    let now = OffsetDateTime::now_utc();
    let config = load_config(&workspace.paths)?;
    let ranked = rank_attention(collect_attention(
        &workspace.paths,
        &workspace.graph,
        &config,
        now,
    )?);
    let mut questions = read_questions(&workspace.paths)?
        .into_iter()
        .filter(|question| is_due(question, now))
        .collect::<Vec<_>>();
    questions.sort_by(|left, right| {
        left.node
            .cmp(&right.node)
            .then_with(|| left.created_at.cmp(&right.created_at))
    });
    let proposal_rows = read_proposals(&workspace.paths)?;
    let mut proposals = pending_proposals(&proposal_rows, now, Some(&workspace.graph))
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    proposals.sort_by(|left, right| {
        left.expires_at
            .as_deref()
            .unwrap_or_default()
            .cmp(right.expires_at.as_deref().unwrap_or_default())
    });
    let sprouts = open_sprouts(&workspace.graph, now)
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    let ready = ready_to_mark_built(
        &workspace.graph,
        &read_missions_issues(&workspace.paths.root),
    );
    let mut body = Map::new();
    body.insert(
        "think".to_owned(),
        Value::Array(
            ranked
                .iter()
                .map(|attention| {
                    let mut item = Map::new();
                    item.insert("id".to_owned(), Value::String(attention.node.id.clone()));
                    item.insert(
                        "slug".to_owned(),
                        Value::String(attention.node.slug.clone()),
                    );
                    item.insert(
                        "status".to_owned(),
                        json(attention.node.status).unwrap_or(Value::Null),
                    );
                    item.insert(
                        "open".to_owned(),
                        Value::Array(
                            attention
                                .due
                                .iter()
                                .map(|question| Value::String(question.id.clone()))
                                .collect(),
                        ),
                    );
                    item.insert(
                        "candidates".to_owned(),
                        Value::Array(
                            attention
                                .candidates
                                .iter()
                                .map(|candidate| Value::String(candidate.rule.clone()))
                                .collect(),
                        ),
                    );
                    Value::Object(item)
                })
                .collect(),
        ),
    );
    body.insert("questions".to_owned(), json(&questions)?);
    body.insert("proposals".to_owned(), json(&proposals)?);
    body.insert("sprouts".to_owned(), json(&sprouts)?);
    body.insert(
        "built".to_owned(),
        Value::Array(
            ready
                .iter()
                .map(|item| {
                    let mut value = Map::new();
                    value.insert("id".to_owned(), Value::String(item.node.id.clone()));
                    value.insert("slug".to_owned(), Value::String(item.node.slug.clone()));
                    value.insert(
                        "issues".to_owned(),
                        Value::Array(
                            item.issues
                                .iter()
                                .map(|issue| Value::String(issue.id.clone()))
                                .collect(),
                        ),
                    );
                    value.insert(
                        "command".to_owned(),
                        Value::String(format!("luuna-roots status {} built", item.node.id)),
                    );
                    Value::Object(value)
                })
                .collect(),
        ),
    );
    let expiring = proposals
        .iter()
        .filter(|proposal| is_soon(proposal.expires_at.as_deref(), now))
        .map(|proposal| Value::String(proposal.id.clone()))
        .chain(
            sprouts
                .iter()
                .filter(|sprout| is_soon(sprout.expires_at.as_deref(), now))
                .map(|sprout| Value::String(sprout.id.clone())),
        )
        .collect();
    body.insert("expiring".to_owned(), Value::Array(expiring));
    context.out.result(context.io, body);
    let sections = vec![
        queue_built_lines(context.out.colors, &ready),
        queue_think_lines(context.out.colors, &ranked),
        queue_question_lines(context.out.colors, &workspace.graph, &questions),
        queue_proposal_lines(context.out.colors, &workspace.graph, &proposals, now),
        queue_sprout_lines(context.out.colors, &sprouts, now),
    ]
    .into_iter()
    .filter(|section| !section.is_empty())
    .collect::<Vec<_>>();
    if sections.is_empty() {
        context.out.line(context.io, "Nothing needs attention.");
    } else {
        let mut lines = Vec::new();
        for (index, section) in sections.into_iter().enumerate() {
            if index > 0 {
                lines.push(String::new());
            }
            lines.extend(section);
        }
        context.out.lines(context.io, &lines);
    }
    Ok(())
}

fn lexical_absolute(cwd: &Path, path: &str) -> PathBuf {
    let source = Path::new(path);
    let source = if source.is_absolute() {
        source.to_path_buf()
    } else {
        cwd.join(source)
    };
    let mut output = PathBuf::new();
    for component in source.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                let _ = output.pop();
            }
            Component::Normal(part) => output.push(part),
            Component::RootDir | Component::Prefix(_) => output.push(component.as_os_str()),
        }
    }
    output
}

fn write_if_changed(file: &Path, text: &str) -> RootsResult<bool> {
    if file.exists()
        && fs::read_to_string(file).map_err(|error| RootsError::error(error.to_string()))? == text
    {
        return Ok(false);
    }
    atomic_write(file, text)?;
    Ok(true)
}

/// `luuna-roots view [--from <id>] [--sprouts] [--html] [--out <file>]`.
pub fn view_command(context: CommandContext<'_>) -> RootsResult<()> {
    let workspace = open_workspace(context.io.cwd())?;
    let config = load_config(&workspace.paths)?;
    let questions = read_questions(&workspace.paths)?;
    let html = flag_bool(context.flags, "html");
    let target = flag_string(context.flags, "out");
    let out_file = target.map(|target| lexical_absolute(context.io.cwd(), target));
    if let Some(file) = &out_file {
        if is_human_path(&workspace.paths, file) {
            return Err(RootsError::guard(format!(
                "refusing to write {}: it is inside .luuna/roots/human/",
                target.unwrap_or_default()
            )));
        }
    }
    let from = flag_string(context.flags, "from")
        .map(|query| {
            resolve_node(
                &workspace.graph.nodes,
                query,
                ResolveOptions {
                    kind: Some(NodeKind::Idea),
                },
            )
        })
        .transpose()?;
    if from.is_some_and(|node| node.status.as_str() == "composted") {
        let node = from.expect("checked above");
        return Err(RootsError::usage(format!(
            "{} {} is composted; the view leaves composted ideas out",
            node.id, node.slug
        )));
    }
    let events = read_events(&workspace.paths)?;
    let now = OffsetDateTime::now_utc();
    let data = build_view(
        &workspace.paths,
        &workspace.graph,
        &config,
        &questions,
        from,
        flag_bool(context.flags, "sprouts"),
        &events,
        now,
    )?;
    let text = if html {
        render_view_html(&data, &|id| {
            let mut events = events_for(&events, id)
                .into_iter()
                .cloned()
                .collect::<Vec<_>>();
            events.sort_by(|left, right| left.at.cmp(&right.at));
            events
        })
    } else {
        render_view_markdown(&data)
    };
    let wrote = if config.view_write {
        let canonical = build_view(
            &workspace.paths,
            &workspace.graph,
            &config,
            &questions,
            None,
            false,
            &events,
            now,
        )?;
        write_if_changed(
            &workspace.paths.root.join(ROOTS_MD),
            &render_view_markdown(&canonical),
        )?
    } else {
        false
    };
    if let Some(file) = &out_file {
        atomic_write(file, &text)?;
    }
    let value = view_data_value(&data)?;
    let Value::Object(mut body) = value else {
        return Err(RootsError::error(
            "view data did not serialize to an object",
        ));
    };
    body.insert(
        "format".to_owned(),
        Value::String(if html { "html" } else { "markdown" }.to_owned()),
    );
    body.insert(
        "out".to_owned(),
        out_file
            .as_ref()
            .map(|file| Value::String(file.display().to_string()))
            .unwrap_or(Value::Null),
    );
    body.insert(
        "rootsMd".to_owned(),
        if config.view_write {
            let mut root = Map::new();
            root.insert("path".to_owned(), Value::String(ROOTS_MD.to_owned()));
            root.insert("written".to_owned(), Value::Bool(wrote));
            Value::Object(root)
        } else {
            Value::Null
        },
    );
    if out_file.is_none() {
        body.insert("text".to_owned(), Value::String(text.clone()));
    }
    context.out.result(context.io, body);
    if context.out.json {
        return Ok(());
    }
    if out_file.is_none() {
        context.io.stdout(&text);
    } else {
        context
            .out
            .success(context.io, &format!("wrote {}", target.unwrap_or_default()));
    }
    if wrote && !context.out.quiet {
        context.io.stderr(&format!(
            "{} updated {ROOTS_MD}\n",
            context.out.colors.green("✓")
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::io::MemoryIo;
    use crate::run_cli;
    use roots_core::config::{default_config, write_config};
    use roots_core::graph::graph_records;
    use roots_core::model::Graph;
    use roots_core::paths::roots_paths;
    use roots_core::serde_json::Value;
    use roots_core::store::write_jsonl_file;
    use std::fs;
    use std::path::{Path, PathBuf};

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|part| (*part).to_owned()).collect()
    }

    fn sandbox(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "roots-read-command-{name}-{}",
            roots_core::ids::random_hex(8)
        ));
        fs::create_dir_all(&root).expect("create sandbox");
        root
    }

    fn empty_project(root: &Path) {
        let paths = roots_paths(root);
        write_config(&paths, &default_config("read-test")).expect("write config");
        write_jsonl_file(&paths.graph, &graph_records(&Graph::default())).expect("write graph");
    }

    #[test]
    fn prime_hook_skips_cleanly_outside_a_roots_project() {
        let root = sandbox("hook");
        let mut io = MemoryIo::new(&root);
        assert_eq!(run_cli(&argv(&["prime", "--hook", "--json"]), &mut io), 0);
        let body: Value = roots_core::serde_json::from_str(&io.stdout).expect("hook json");
        assert_eq!(body["command"], "prime");
        assert!(body["project"].is_null());
        assert_eq!(body["skipped"], "not a roots project");
        assert!(io.stderr.is_empty());
        fs::remove_dir_all(root).expect("clean sandbox");
    }

    #[test]
    fn view_writes_html_outside_human_intent_and_refuses_the_human_tree() {
        let root = sandbox("view-out");
        empty_project(&root);

        let target = root.join("public").join("roots.html");
        let mut html = MemoryIo::new(&root);
        assert_eq!(
            run_cli(
                &argv(&[
                    "view",
                    "--html",
                    "--sprouts",
                    "--out",
                    "public/roots.html",
                    "--json",
                ]),
                &mut html,
            ),
            0,
            "{}",
            html.stderr
        );
        let result: Value = roots_core::serde_json::from_str(&html.stdout).expect("view json");
        assert_eq!(result["format"], "html");
        assert_eq!(result["out"], target.display().to_string());
        assert!(
            result.get("text").is_none(),
            "out omits duplicate text payload"
        );
        let document = fs::read_to_string(&target).expect("written html");
        assert!(document.starts_with("<!doctype html>"));
        assert!(document.contains("Agent proposals (not accepted)"));

        let blocked = root
            .join(".luuna")
            .join("roots")
            .join("human")
            .join("unsafe.md");
        let mut denied = MemoryIo::new(&root);
        assert_eq!(
            run_cli(
                &argv(&["view", "--out", ".luuna/roots/human/unsafe.md", "--json",]),
                &mut denied,
            ),
            roots_core::errors::EXIT_GUARD
        );
        let error: Value = roots_core::serde_json::from_str(&denied.stdout).expect("guard json");
        assert_eq!(error["code"], "guard");
        assert!(
            error["error"]
                .as_str()
                .is_some_and(|message| { message.contains("inside .luuna/roots/human/") })
        );
        assert!(!blocked.exists());
        fs::remove_dir_all(root).expect("clean sandbox");
    }
}
