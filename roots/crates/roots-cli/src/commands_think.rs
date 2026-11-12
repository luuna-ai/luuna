//! Human-facing `luuna-roots think` command wiring.

use crate::commands_capture::print_next;
use crate::editor::{SplitLaunch, editor_hint, launch_editor_split};
use crate::interactive::think_session::{
    AgentStatus, SessionSummary, ThinkDeps, run_think_session,
};
use crate::registry::CommandContext;
use roots_core::actor::{ActorContext, resolve_human_actor};
use roots_core::args::flag_bool;
use roots_core::attention::{collect_attention, rank_attention};
use roots_core::config::load_config;
use roots_core::errors::{RootsError, RootsResult};
use roots_core::model::{IdeaStatus, NodeKind, NodeRecord, NodeStatus};
use roots_core::resolve::{ResolveOptions, resolve_node};
use roots_core::serde_json::{Map, Value, to_value};
use roots_core::time_crate::OffsetDateTime;
use roots_core::workspace::{Workspace, open_workspace};
use std::path::Path;

const DESCRIPTION: &str = "Shows one question at a time about the idea. Write in idea.md in your editor:\n\
a save that changes the file answers the current question and moves on.\n\
Keys: [d] dismiss (never ask again)  [z] snooze  [s] skip  [q] end session.\n\
[a] ask agent (tier ≥ 1): after the session agent.command researches the question\n\
and attaches findings; it comes back with them, and you still answer in idea.md.\n\
Inside tmux or zellij, $EDITOR opens in a split pane; otherwise open the\n\
printed path in another pane. With no id, think picks the idea most in need:\n\
ideas with open questions first, then the most pending questions, planted before\n\
shaping before committed, least recently touched first (see `luuna-roots queue`).";

pub const THINK_DESCRIPTION: &str = DESCRIPTION;

fn relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .display()
        .to_string()
}

fn pick_node(workspace: &Workspace, query: Option<&str>) -> RootsResult<Option<NodeRecord>> {
    if let Some(query) = query {
        let node = resolve_node(
            &workspace.graph.nodes,
            query,
            ResolveOptions {
                kind: Some(NodeKind::Idea),
            },
        )?;
        if node.status == NodeStatus::Idea(IdeaStatus::Composted) {
            return Err(RootsError::usage(format!(
                "{} is composted; there is nothing left to think about",
                node.id
            )));
        }
        return Ok(Some(node.clone()));
    }
    let config = load_config(&workspace.paths)?;
    Ok(rank_attention(collect_attention(
        &workspace.paths,
        &workspace.graph,
        &config,
        OffsetDateTime::now_utc(),
    )?)
    .first()
    .map(|attention| attention.node.clone()))
}

fn plural(count: usize, word: &str) -> String {
    format!("{count} {word}{}", if count == 1 { "" } else { "s" })
}

fn launch_json(launch: Option<SplitLaunch>) -> Value {
    let Some(launch) = launch else {
        return Value::Null;
    };
    let mut item = Map::new();
    item.insert(
        "mux".to_owned(),
        launch
            .mux
            .map(|mux| Value::String(mux.as_str().to_owned()))
            .unwrap_or(Value::Null),
    );
    item.insert("ok".to_owned(), Value::Bool(launch.ok));
    Value::Object(item)
}

fn summary_map(summary: &SessionSummary) -> RootsResult<Map<String, Value>> {
    let value = to_value(summary).map_err(|problem| RootsError::error(problem.to_string()))?;
    value
        .as_object()
        .cloned()
        .ok_or_else(|| RootsError::error("could not serialize think summary"))
}

fn tally_line(summary: &SessionSummary) -> String {
    let tally = &summary.tally;
    let parts = [
        (tally.answered, "answered"),
        (tally.dismissed, "dismissed"),
        (tally.snoozed, "snoozed"),
        (tally.skipped, "skipped"),
    ]
    .into_iter()
    .filter(|(count, _)| *count > 0)
    .map(|(count, label)| format!("{count} {label}"))
    .chain((tally.delegated > 0).then(|| format!("{} handed to the agent", tally.delegated)))
    .chain((tally.unasked > 0).then(|| format!("{} left for next time", tally.unasked)))
    .collect::<Vec<_>>();
    if parts.is_empty() {
        "no questions".to_owned()
    } else {
        parts.join(", ")
    }
}

fn print_summary(
    context: &mut CommandContext<'_>,
    workspace: &Workspace,
    node: &NodeRecord,
    summary: &SessionSummary,
) {
    context.out.success(
        context.io,
        &format!(
            "session {} on {} {} ended",
            context.out.colors.id(&summary.session),
            context.out.colors.id(&node.id),
            node.slug
        ),
    );
    let unchanged = (!summary.changed).then(|| context.out.colors.dim(" · idea.md unchanged"));
    context.out.info(
        context.io,
        &format!("  {}{}", tally_line(summary), unchanged.unwrap_or_default()),
    );
    if let Some(change) = &summary.status_change {
        context.out.info(
            context.io,
            &format!("  status {} → {}", change.from, change.to),
        );
    }
    if let Some(mentions) = &summary.mentions {
        if !mentions.result.filed.is_empty() {
            let refs = mentions
                .result
                .filed
                .iter()
                .map(|proposal| {
                    let target = proposal
                        .to
                        .as_deref()
                        .and_then(|id| workspace.graph.nodes.iter().find(|node| node.id == id))
                        .map(|node| node.slug.as_str())
                        .unwrap_or_default();
                    format!("{} → {target}", context.out.colors.id(&proposal.id))
                })
                .collect::<Vec<_>>()
                .join(", ");
            context.out.info(
                context.io,
                &format!(
                    "  {} filed: {refs}",
                    plural(mentions.result.filed.len(), "mention proposal")
                ),
            );
        }
    }
    if let Some(problem) = &summary.mention_error {
        context
            .out
            .warn(context.io, &format!("mention scan failed: {problem}"));
    }
    if let Some(proposals) = &summary.proposals {
        if !proposals.filed.is_empty() {
            context.out.info(
                context.io,
                &format!(
                    "  {} proposed {}: {}",
                    proposals.actor,
                    plural(proposals.filed.len(), "link"),
                    proposals.filed.join(", ")
                ),
            );
        }
    }
    let mention_count = summary
        .mentions
        .as_ref()
        .map_or(0, |mentions| mentions.result.filed.len());
    let proposal_count = summary
        .proposals
        .as_ref()
        .map_or(0, |proposals| proposals.filed.len());
    if mention_count + proposal_count > 0 {
        context.out.info(
            context.io,
            &context
                .out
                .colors
                .dim("  review them with `luuna-roots tend`"),
        );
    }
    if let Some(delegation) = &summary.delegation {
        let mut parts = Vec::new();
        if !delegation.found.is_empty() {
            parts.push(format!(
                "{} researched {}: findings wait under the question next session",
                delegation.actor,
                delegation.found.join(", ")
            ));
        }
        if !delegation.returned.is_empty() {
            parts.push(format!(
                "{} came back without findings",
                delegation.returned.join(", ")
            ));
        }
        if let Some(problem) = &delegation.error {
            parts.push(format!("research run: {problem}"));
        }
        if !parts.is_empty() {
            context
                .out
                .info(context.io, &format!("  {}", parts.join("; ")));
        }
    }
    if summary.agent.mode == "command" && summary.agent.status.as_deref() == Some("ok") {
        context.out.info(
            context.io,
            &format!(
                "  {} asked {}",
                summary.agent.actor.as_deref().unwrap_or("agent"),
                plural(summary.agent.asked, "question")
            ),
        );
    }
    if summary.agent.arrived > 0 {
        context.out.info(
            context.io,
            &format!(
                "  {} arrived during the session",
                plural(summary.agent.arrived, "agent question")
            ),
        );
    }
    if let Some(problem) = &summary.agent.error {
        context.out.warn(
            context.io,
            &format!("agent.command {problem}; the session used roots' own questions"),
        );
    }
}

fn nothing_queued(context: &mut CommandContext<'_>) {
    let mut body = Map::new();
    body.insert("session".to_owned(), Value::Null);
    body.insert(
        "reason".to_owned(),
        Value::String("nothing-queued".to_owned()),
    );
    context.out.result(context.io, body);
    context
        .out
        .line(context.io, "Nothing needs thinking right now.");
    context.out.info(
        context.io,
        &context
            .out
            .colors
            .dim("Run `luuna-roots think <id>` to think about a specific idea."),
    );
}

/// `luuna-roots think [<id>]`.
pub fn think_command(mut context: CommandContext<'_>) -> RootsResult<()> {
    let environment = context.io.environment();
    roots_core::guard::require_tty(&environment, context.io.stdin_is_tty(), "think")?;
    let by = resolve_human_actor(ActorContext::new(context.io.cwd(), &environment))?;
    let workspace = open_workspace(context.io.cwd())?;
    let Some(node) = pick_node(&workspace, context.args.first().map(String::as_str))? else {
        nothing_queued(&mut context);
        return Ok(());
    };
    // Preserve TypeScript's explicit terminal capability error independently
    // from the TTY guard. A test IO can advertise a TTY but omit a terminal.
    let status_columns = context
        .io
        .terminal()
        .map(|terminal| terminal.columns_reader());
    if status_columns.is_none() {
        return Err(RootsError::guard(
            "`luuna-roots think` needs an interactive terminal",
        ));
    }
    let prose = roots_core::prose::read_node_prose(&workspace.paths, &node, Some(&workspace.dirs))?;
    let file = prose
        .path
        .as_deref()
        .map(|path| relative(&workspace.paths.root, path))
        .unwrap_or_default();
    let split = (!flag_bool(context.flags, "no-split"))
        .then(|| launch_editor_split(&environment, &file, &workspace.paths.root, None));
    let config = load_config(&workspace.paths)?;
    let hint = editor_hint(split, &file);
    let agent_status = AgentStatus {
        write: context.io.agent_status_writer(),
        animate: context.io.stdout_is_tty(),
        // A cloneable reader preserves the source spinner's narrow-terminal
        // truncation while the session exclusively borrows the terminal.
        columns: status_columns,
    };
    let result = {
        let terminal = context.io.terminal().ok_or_else(|| {
            RootsError::guard("`luuna-roots think` needs an interactive terminal")
        })?;
        run_think_session(ThinkDeps {
            paths: workspace.paths.clone(),
            config,
            node: node.clone(),
            by,
            terminal,
            colors: context.out.colors,
            editor_hint: hint,
            watch: Default::default(),
            environment,
            guidance: None,
            agent_timeout_ms: None,
            agent_status: Some(agent_status),
            background: None,
        })
    };
    context.io.flush_agent_status();
    let summary = result?;
    let mut body = summary_map(&summary)?;
    body.insert("slug".to_owned(), Value::String(node.slug.clone()));
    body.insert("path".to_owned(), Value::String(file));
    body.insert("split".to_owned(), launch_json(split));
    context.out.result(context.io, body);
    print_summary(&mut context, &workspace, &node, &summary);
    print_next(&mut context, &workspace.paths);
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::commands_capture::plant_text;
    use crate::interactive::terminal::FakeTerminal;
    use crate::io::MemoryIo;
    use crate::run_cli;
    use roots_core::config::{default_config, write_config};
    use roots_core::paths::roots_paths;
    use std::collections::BTreeMap;
    use std::fs;

    fn initialized() -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!(
            "roots-think-command-{}",
            roots_core::ids::random_hex(8)
        ));
        let paths = roots_paths(&root);
        fs::create_dir_all(&paths.human).unwrap();
        fs::create_dir_all(&paths.agent).unwrap();
        write_config(&paths, &default_config("test")).unwrap();
        for file in [
            &paths.graph,
            &paths.questions,
            &paths.events,
            &paths.proposals,
        ] {
            fs::write(file, "").unwrap();
        }
        root
    }

    #[test]
    fn no_queue_is_a_successful_friendly_noop() {
        let root = initialized();
        let mut io = MemoryIo::new(&root);
        io.env = BTreeMap::from([("ROOTS_USER".to_owned(), "Test".to_owned())]);
        io.set_terminal(FakeTerminal::new(60, 20).0);
        assert_eq!(
            run_cli(&["think".to_owned(), "--json".to_owned()], &mut io),
            0
        );
        assert!(io.stdout.contains("nothing-queued"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn tty_guard_precedes_session_side_effects() {
        let root = initialized();
        let paths = roots_paths(&root);
        let planted = plant_text(&paths, "Sync works offline\n", "human:test", None)
            .unwrap()
            .unwrap();
        let mut io = MemoryIo::new(&root);
        io.stdin_is_tty = false;
        io.env = BTreeMap::from([("ROOTS_USER".to_owned(), "Test".to_owned())]);
        io.set_terminal(FakeTerminal::new(60, 20).0);
        assert_eq!(run_cli(&["think".to_owned(), planted.node.id], &mut io), 4);
        assert!(io.stderr.contains("needs an interactive terminal"));
        let _ = fs::remove_dir_all(root);
    }
}
