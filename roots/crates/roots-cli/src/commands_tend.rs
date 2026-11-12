//! Human-facing `luuna-roots tend` command.
//!
//! The pure card/reducer layer is in `roots-core::tend`; this module owns only
//! command guards, the editor / think hand-offs, and the human summary.

use crate::commands_capture::{
    AdoptInput, AdoptionOptions, AdoptionResult, complete_adoption, print_next, run_adoption,
};
use crate::editor::{editor_hint, launch_editor_split};
use crate::interactive::screen::ScreenGuidance;
use crate::interactive::tend_session::{TendDriver, TerminalTurn, collect_cards, start_tend};
use crate::interactive::think_session::{
    AgentStatus, SessionSummary, ThinkDeps, run_think_session,
};
use crate::registry::CommandContext;
use roots_core::actor::ActorContext;
use roots_core::args::flag_bool;
use roots_core::config::load_config;
use roots_core::decisions::{AcceptOptions, Decision, accept_proposal, reject_proposal};
use roots_core::errors::{ErrorCode, RootsError, RootsResult};
use roots_core::model::{EdgeRecord, NodeKind, NodeRecord, ProposalStatus};
use roots_core::proposals::kind_name;
use roots_core::resolve::{ResolveOptions, resolve_node};
use roots_core::serde_json::{Map, Value, to_value};
use roots_core::sprouts::reject_sprout;
use roots_core::tend::{
    TendAction, TendCard, TendEffect, TendInput, TendOutcome, TendState, days_left, expiring_soon,
    tend_step, tend_tally,
};
use roots_core::time_crate::OffsetDateTime;
use roots_core::workspace::{Workspace, open_workspace};
use std::path::Path;
use std::sync::Arc;

pub const TEND_DESCRIPTION: &str = "One card per proposal (a mention and an agent proposal for the same pair share a\n\
card), soonest expiry first. Keys: [y] accept  [n] reject  [r] reject with a reason\n\
[s] skip  [q] quit. Mention cards: [1] serves [2] tension [3] replaces. Merge cards:\n\
[1]/[2] pick the idea that survives. Accepting a split opens a think session on the\n\
idea with the split shown as guidance. Sprout cards: [y] adopts it (you write the\n\
idea yourself in $EDITOR; nothing is copied), [n]/[r] reject it for good.\n\
--json lists the cards and decides nothing.";

fn relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .display()
        .to_string()
}

fn card_line(context: &CommandContext<'_>, card: &TendCard) -> String {
    match card {
        TendCard::Sprout(card) => format!(
            "{} sprout {}: {}  {}",
            context.out.colors.id(&card.id),
            card.slug,
            card.statement,
            context.out.colors.dim(&card.author)
        ),
        TendCard::Proposal(card) => {
            let pair = card.to.as_ref().map_or_else(
                || card.from.slug.clone(),
                |to| {
                    format!(
                        "{} ─{}─ {}",
                        card.from.slug,
                        card.rel.map_or("?", |rel| rel.as_str()),
                        to.slug
                    )
                },
            );
            format!(
                "{} {} {}  {}",
                context.out.colors.id(&card.ids.join("+")),
                kind_name(card.kind),
                pair,
                context.out.colors.dim(&card.sources.join(", "))
            )
        }
    }
}

fn card_json(card: &TendCard, now: OffsetDateTime) -> RootsResult<Value> {
    let mut value = to_value(card).map_err(|error| RootsError::error(error.to_string()))?;
    let object = value
        .as_object_mut()
        .ok_or_else(|| RootsError::error("could not serialize tend card"))?;
    object.insert(
        "expiresInDays".to_owned(),
        days_left(card.expires_at(), now).map_or(Value::Null, |days| Value::Number(days.into())),
    );
    object.insert(
        "expiringSoon".to_owned(),
        Value::Bool(expiring_soon(card, now)),
    );
    Ok(value)
}

fn list_cards(context: CommandContext<'_>) -> RootsResult<()> {
    let workspace = open_workspace(context.io.cwd())?;
    let now = OffsetDateTime::now_utc();
    let cards = collect_cards(&workspace.paths, now)?;
    let mut body = Map::new();
    body.insert(
        "cards".to_owned(),
        Value::Array(
            cards
                .iter()
                .map(|card| card_json(card, now))
                .collect::<RootsResult<Vec<_>>>()?,
        ),
    );
    context.out.result(context.io, body);
    for card in &cards {
        let line = card_line(&context, card);
        context.out.line(context.io, &line);
    }
    if cards.is_empty() {
        context.out.line(context.io, "Nothing to tend.");
    }
    Ok(())
}

#[derive(Clone, Debug)]
struct SproutDecision {
    action: String,
    node: NodeRecord,
    idea: Option<NodeRecord>,
    edge: Option<EdgeRecord>,
    file: Option<String>,
}

#[derive(Clone, Debug)]
enum RecordedDecision {
    Proposal(Box<Decision>),
    Sprout(Box<SproutDecision>),
}

fn sprout_decision(action: &str, result: AdoptionResult, root: &Path) -> SproutDecision {
    SproutDecision {
        action: action.to_owned(),
        node: result.sprout,
        idea: Some(result.idea),
        edge: Some(result.edge),
        file: Some(relative(root, &result.file)),
    }
}

fn rejected_sprout(node: NodeRecord) -> SproutDecision {
    SproutDecision {
        action: "reject".to_owned(),
        node,
        idea: None,
        edge: None,
        file: None,
    }
}

fn outcome_of(decision: &RecordedDecision) -> TendOutcome {
    match decision {
        RecordedDecision::Proposal(decision) if decision.action == "accept" => {
            TendOutcome::Accepted
        }
        RecordedDecision::Sprout(decision) if decision.action == "accept" => TendOutcome::Accepted,
        _ => TendOutcome::Rejected,
    }
}

fn notice_of(decision: &RecordedDecision) -> String {
    match decision {
        RecordedDecision::Proposal(decision) => {
            if let Some(edge) = &decision.edge {
                return format!(
                    "{}: {} {} {}",
                    edge.id,
                    edge.from,
                    edge.rel.as_str(),
                    edge.to
                );
            }
            let composted = decision
                .status_changes
                .iter()
                .map(|change| format!("{} composted", change.node))
                .collect::<Vec<_>>();
            if composted.is_empty() {
                format!("{} {}", decision.id, decision.action)
            } else {
                composted.join(", ")
            }
        }
        RecordedDecision::Sprout(decision) => {
            if let Some(idea) = &decision.idea {
                let verb = if idea.endorses.is_some() {
                    "endorsed"
                } else {
                    "adopted"
                };
                format!("{} {verb} as {} {}", decision.node.id, idea.id, idea.slug)
            } else {
                format!("{} {}", decision.node.id, decision.node.status.as_str())
            }
        }
    }
}

fn proposal_lines(context: &CommandContext<'_>, decision: &Decision) -> Vec<String> {
    let mut lines = Vec::new();
    for proposal in &decision.proposals {
        let mark = if proposal.status == ProposalStatus::Accepted {
            context.out.colors.green("✓ accepted")
        } else {
            context.out.colors.yellow("✗ rejected")
        };
        let why = proposal
            .decision_reason
            .as_deref()
            .map(|reason| context.out.colors.dim(&format!(" ({reason})")))
            .unwrap_or_default();
        lines.push(format!(
            "{mark} {} {}{why}",
            context.out.colors.id(&proposal.id),
            kind_name(proposal.kind)
        ));
    }
    if let Some(edge) = &decision.edge {
        lines.push(format!(
            "  {} {} {} {} {}",
            context.out.colors.green("+"),
            context.out.colors.id(&edge.id),
            edge.from,
            edge.rel.as_str(),
            edge.to
        ));
    }
    for change in &decision.status_changes {
        lines.push(format!("  {} {} → {}", change.node, change.from, change.to));
    }
    if let Some(think) = &decision.think {
        lines.push(format!(
            "  do the split yourself: `luuna-roots think {}`",
            think.node
        ));
        lines.push(
            context
                .out
                .colors
                .dim(&format!("  suggested: {}", think.guidance)),
        );
    }
    lines
}

fn sprout_lines(context: &CommandContext<'_>, decision: &SproutDecision) -> Vec<String> {
    if let Some(idea) = &decision.idea {
        let mut lines = vec![format!(
            "{} {} as {} {}",
            context.out.colors.green("✓ adopted"),
            context.out.colors.id(&decision.node.id),
            context.out.colors.id(&idea.id),
            idea.slug
        )];
        if let Some(edge) = &decision.edge {
            lines.push(format!(
                "  {} {} {} derives {}",
                context.out.colors.green("+"),
                context.out.colors.id(&edge.id),
                edge.from,
                edge.to
            ));
        }
        if let Some(file) = &decision.file {
            lines.push(context.out.colors.dim(&format!("  {file}")));
        }
        return lines;
    }
    let why = decision
        .node
        .decision_reason
        .as_deref()
        .map(|reason| context.out.colors.dim(&format!(" ({reason})")))
        .unwrap_or_default();
    vec![format!(
        "{} {} sprout {}{why}",
        context
            .out
            .colors
            .yellow(&format!("✗ {}", decision.node.status.as_str())),
        context.out.colors.id(&decision.node.id),
        decision.node.slug
    )]
}

fn decision_lines(context: &CommandContext<'_>, decision: &RecordedDecision) -> Vec<String> {
    match decision {
        RecordedDecision::Proposal(decision) => proposal_lines(context, decision),
        RecordedDecision::Sprout(decision) => sprout_lines(context, decision),
    }
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

fn run_split_think(
    context: &mut CommandContext<'_>,
    by: &str,
    id: &str,
    guidance: &str,
    split: bool,
) -> RootsResult<String> {
    let workspace = open_workspace(context.io.cwd())?;
    let node = resolve_node(
        &workspace.graph.nodes,
        id,
        ResolveOptions {
            kind: Some(NodeKind::Idea),
        },
    )?
    .clone();
    let prose = roots_core::prose::read_node_prose(&workspace.paths, &node, Some(&workspace.dirs))?;
    let file = prose
        .path
        .as_deref()
        .map(|path| relative(&workspace.paths.root, path))
        .unwrap_or_default();
    let environment = context.io.environment();
    let launch =
        split.then(|| launch_editor_split(&environment, &file, &workspace.paths.root, None));
    let config = load_config(&workspace.paths)?;
    let hint = editor_hint(launch, &file);
    // The terminal exposes a detached, resize-aware reader so the status line
    // retains the same width behavior as the source while think owns its
    // exclusive terminal borrow.
    let status_columns = context
        .io
        .terminal()
        .map(|terminal| terminal.columns_reader())
        .unwrap_or_else(|| Arc::new(|| 80));
    let agent_status = AgentStatus {
        write: context.io.agent_status_writer(),
        animate: context.io.stdout_is_tty(),
        columns: Some(status_columns),
    };
    let result = {
        let terminal = context.io.terminal().ok_or_else(|| {
            RootsError::guard("`luuna-roots think` needs an interactive terminal")
        })?;
        run_think_session(ThinkDeps {
            paths: workspace.paths.clone(),
            config,
            node: node.clone(),
            by: by.to_owned(),
            terminal,
            colors: context.out.colors,
            editor_hint: hint,
            watch: Default::default(),
            environment,
            guidance: Some(ScreenGuidance {
                label: "split this idea".to_owned(),
                source: "agent, accepted".to_owned(),
                text: guidance.to_owned(),
            }),
            agent_status: Some(agent_status),
            background: None,
            agent_timeout_ms: None,
        })
    };
    context.io.flush_agent_status();
    let summary = result?;
    Ok(format!(
        "think session {} on {}: {}",
        summary.session,
        node.slug,
        tally_line(&summary)
    ))
}

enum EffectResult {
    Done(TendOutcome, Option<String>),
    /// The proposal is already persisted and recorded; its nested think
    /// session must run with `tend`'s raw terminal temporarily stopped.
    Think {
        outcome: TendOutcome,
        node: String,
        guidance: String,
    },
    /// Adoption opens the human editor, so no decision exists until it saves.
    Adopt {
        id: String,
        endorse: Option<String>,
    },
}

fn execute_without_handoff(
    workspace: &Workspace,
    by: &str,
    effect: TendEffect,
    decisions: &mut Vec<RecordedDecision>,
) -> RootsResult<EffectResult> {
    let now = OffsetDateTime::now_utc();
    let decision = match (&effect.card, effect.action) {
        (TendCard::Proposal(card), TendAction::Reject { reason }) => {
            RecordedDecision::Proposal(Box::new(reject_proposal(
                &workspace.paths,
                by,
                &card.id,
                reason.as_deref(),
                Some(now),
            )?))
        }
        (TendCard::Proposal(card), TendAction::Accept { rel, keep }) => {
            RecordedDecision::Proposal(Box::new(accept_proposal(
                &workspace.paths,
                by,
                &card.id,
                AcceptOptions {
                    rel: rel.map(|value| value.as_str().to_owned()),
                    keep,
                    reason: None,
                },
                Some(now),
            )?))
        }
        (TendCard::Sprout(card), TendAction::Reject { reason }) => {
            RecordedDecision::Sprout(Box::new(rejected_sprout(reject_sprout(
                &workspace.paths,
                by,
                &card.id,
                reason.as_deref(),
                Some(now),
            )?)))
        }
        (TendCard::Sprout(card), TendAction::Accept { .. }) => {
            return Ok(EffectResult::Adopt {
                id: card.id.clone(),
                endorse: None,
            });
        }
        (TendCard::Sprout(card), TendAction::Endorse { text, editor }) => {
            if editor {
                return Ok(EffectResult::Adopt {
                    id: card.id.clone(),
                    endorse: Some(if text.is_empty() {
                        String::new()
                    } else {
                        format!("{text}\n")
                    }),
                });
            }
            let adoption = complete_adoption(
                &workspace.paths,
                AdoptInput {
                    sprout: &card.id,
                    text: &format!("{text}\n"),
                    by,
                    slug: None,
                    now: Some(now),
                    endorse: true,
                },
            )?;
            RecordedDecision::Sprout(Box::new(sprout_decision(
                "accept",
                adoption,
                &workspace.paths.root,
            )))
        }
        (_, action) => {
            return Err(RootsError::error(format!(
                "nothing to decide for {}",
                action_name(&action)
            )));
        }
    };
    let outcome = outcome_of(&decision);
    if let RecordedDecision::Proposal(proposal) = &decision {
        if let Some(think) = &proposal.think {
            // The decision is deliberately recorded before the nested session,
            // mirroring JS: an editor/think error leaves the accepted card
            // visible with the error rather than rolling back audited state.
            decisions.push(decision.clone());
            return Ok(EffectResult::Think {
                outcome,
                node: think.node.clone(),
                guidance: think.guidance.clone(),
            });
        }
    }
    let notice = Some(notice_of(&decision));
    decisions.push(decision);
    Ok(EffectResult::Done(outcome, notice))
}

fn execute_handoff(
    context: &mut CommandContext<'_>,
    workspace: &Workspace,
    by: &str,
    split: bool,
    handoff: EffectResult,
    decisions: &mut Vec<RecordedDecision>,
) -> RootsResult<(TendOutcome, Option<String>)> {
    match handoff {
        EffectResult::Think {
            outcome,
            node,
            guidance,
        } => run_split_think(context, by, &node, &guidance, split)
            .map(|message| (outcome, Some(message))),
        EffectResult::Adopt { id, endorse } => {
            let adoption = run_adoption(
                context.io,
                &workspace.paths,
                &id,
                AdoptionOptions {
                    by,
                    split,
                    slug: None,
                    spawn: None,
                    endorse: endorse.as_deref(),
                },
            )?;
            let decision = RecordedDecision::Sprout(Box::new(sprout_decision(
                "accept",
                adoption,
                &workspace.paths.root,
            )));
            let outcome = outcome_of(&decision);
            let notice = Some(notice_of(&decision));
            decisions.push(decision);
            Ok((outcome, notice))
        }
        EffectResult::Done(_, _) => Err(RootsError::error("internal tend hand-off mismatch")),
    }
}

fn action_name(action: &TendAction) -> &'static str {
    match action {
        TendAction::Accept { .. } => "accept",
        TendAction::Reject { .. } => "reject",
        TendAction::Skip => "skip",
        TendAction::Quit => "quit",
        TendAction::Endorse { .. } => "endorse",
    }
}

fn print_summary(
    context: &mut CommandContext<'_>,
    expired: usize,
    expired_sprouts: usize,
    cards: usize,
    state: &TendState,
    decisions: &[RecordedDecision],
) {
    if expired > 0 {
        context.out.info(
            context.io,
            &context
                .out
                .colors
                .dim(&format!("{expired} proposal(s) expired unreviewed")),
        );
    }
    if expired_sprouts > 0 {
        context.out.info(
            context.io,
            &context
                .out
                .colors
                .dim(&format!("{expired_sprouts} sprout(s) expired unreviewed")),
        );
    }
    if cards == 0 {
        context.out.line(context.io, "Nothing to tend.");
        return;
    }
    for decision in decisions {
        let lines = decision_lines(context, decision);
        context.out.lines(context.io, &lines);
    }
    let tally = tend_tally(state);
    let mut parts = vec![
        format!("{} accepted", tally.accepted),
        format!("{} rejected", tally.rejected),
        format!("{} skipped", tally.skipped),
    ];
    if tally.left > 0 {
        parts.push(format!("{} left", tally.left));
    }
    context.out.success(
        context.io,
        &format!(
            "tended {cards} card{}: {}",
            if cards == 1 { "" } else { "s" },
            parts.join(", ")
        ),
    );
}

/// `luuna-roots tend [--no-split]`.
pub fn tend_command(mut context: CommandContext<'_>) -> RootsResult<()> {
    if context.out.json {
        return list_cards(context);
    }
    let environment = context.io.environment();
    roots_core::guard::require_tty(&environment, context.io.stdin_is_tty(), "tend")?;
    let by = roots_core::guard::require_human_command(
        ActorContext::new(context.io.cwd(), &environment),
        "tend",
    )?;
    if context.io.terminal().is_none() {
        return Err(RootsError::guard(
            "`luuna-roots tend` needs an interactive terminal",
        ));
    }
    let workspace = open_workspace(context.io.cwd())?;
    let split = !flag_bool(context.flags, "no-split");
    let now = OffsetDateTime::now_utc();
    let mut run = start_tend(&workspace.paths, now)?;
    let cards = run.state.cards.len();
    let mut decisions = Vec::new();
    let mut driver = if run.state.ended {
        None
    } else {
        let terminal = context
            .io
            .terminal()
            .ok_or_else(|| RootsError::guard("`luuna-roots tend` needs an interactive terminal"))?;
        Some(TendDriver::start(
            terminal,
            &run.state,
            run.expired.len(),
            run.expired_sprouts.len(),
            context.out.colors,
            OffsetDateTime::now_utc(),
        ))
    };
    while !run.state.ended {
        let turn = {
            let terminal = context.io.terminal().ok_or_else(|| {
                RootsError::guard("`luuna-roots tend` needs an interactive terminal")
            })?;
            driver.as_mut().expect("live tend driver").next(
                terminal,
                &mut run.state,
                run.expired.len(),
                run.expired_sprouts.len(),
                context.out.colors,
                OffsetDateTime::now_utc(),
            )?
        };
        if let TerminalTurn::Effect(effect) = turn {
            let effect_result = execute_without_handoff(&workspace, &by, *effect, &mut decisions);
            let mut did_handoff = false;
            let completed = match effect_result {
                Ok(EffectResult::Done(outcome, notice)) => Ok((outcome, notice)),
                Ok(handoff) => {
                    did_handoff = true;
                    {
                        let terminal = context.io.terminal().ok_or_else(|| {
                            RootsError::guard("`luuna-roots tend` needs an interactive terminal")
                        })?;
                        driver.as_mut().expect("live tend driver").stop(terminal);
                    }
                    execute_handoff(
                        &mut context,
                        &workspace,
                        &by,
                        split,
                        handoff,
                        &mut decisions,
                    )
                }
                Err(error) => Err(error),
            };
            let input = match completed {
                Ok((outcome, notice)) => TendInput::Done {
                    result: Ok((outcome, notice)),
                },
                Err(error) if error.code == ErrorCode::Cancelled => TendInput::Done {
                    result: Ok((
                        TendOutcome::Skipped,
                        Some("adoption cancelled; nothing written".to_owned()),
                    )),
                },
                Err(error) => TendInput::Done {
                    result: Err(error.message),
                },
            };
            run.state = tend_step(run.state, input).state;
            let terminal = context.io.terminal().ok_or_else(|| {
                RootsError::guard("`luuna-roots tend` needs an interactive terminal")
            })?;
            let driver = driver.as_mut().expect("live tend driver");
            if run.state.ended {
                driver.stop(terminal);
            } else if did_handoff {
                driver.restart(
                    terminal,
                    &run.state,
                    run.expired.len(),
                    run.expired_sprouts.len(),
                    context.out.colors,
                    OffsetDateTime::now_utc(),
                );
            } else {
                driver.render(
                    terminal,
                    &run.state,
                    run.expired.len(),
                    run.expired_sprouts.len(),
                    context.out.colors,
                    OffsetDateTime::now_utc(),
                );
            }
        }
    }
    print_summary(
        &mut context,
        run.expired.len(),
        run.expired_sprouts.len(),
        cards,
        &run.state,
        &decisions,
    );
    print_next(&mut context, &workspace.paths);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interactive::terminal::{FakeTerminal, FakeTerminalHandle};
    use crate::io::MemoryIo;
    use crate::run_cli;
    use roots_core::config::{default_config, write_config};
    use roots_core::events::read_events;
    use roots_core::graph::{graph_records, read_graph};
    use roots_core::model::{
        Citation, EdgeRel, Graph, IdeaStatus, NodeKind, NodeRecord, NodeStatus, ProposalKind,
        ProposalStatus,
    };
    use roots_core::paths::roots_paths;
    use roots_core::proposals::read_proposals;
    use roots_core::store::write_jsonl_file;
    use std::collections::BTreeMap;
    use std::fs;
    use std::thread;
    use std::time::{Duration, Instant};

    fn sandbox(label: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!(
            "roots-tend-{label}-{}",
            roots_core::ids::random_hex(8)
        ));
        fs::create_dir_all(&root).expect("sandbox");
        root
    }

    fn initialize(root: &Path) -> roots_core::paths::RootsPaths {
        let paths = roots_paths(root);
        fs::create_dir_all(&paths.human).expect("human tree");
        fs::create_dir_all(&paths.agent).expect("agent tree");
        fs::create_dir_all(&paths.sprouts).expect("sprout tree");
        fs::create_dir_all(&paths.notes).expect("notes tree");
        write_config(&paths, &default_config("tend-test")).expect("config");
        for file in [
            &paths.graph,
            &paths.proposals,
            &paths.events,
            &paths.questions,
            &paths.headings,
        ] {
            fs::write(file, "").expect("empty store");
        }
        paths
    }

    fn idea(id: &str, slug: &str) -> NodeRecord {
        NodeRecord::new(
            id.to_owned(),
            NodeKind::Idea,
            slug.to_owned(),
            NodeStatus::Idea(IdeaStatus::Planted),
            "human:test".to_owned(),
            "2026-01-01T00:00:00Z".to_owned(),
        )
    }

    fn write_idea_prose(paths: &roots_core::paths::RootsPaths, node: &NodeRecord) {
        let directory = roots_core::prose::canonical_node_dir(paths, node);
        fs::create_dir_all(&directory).expect("idea directory");
        fs::write(directory.join("idea.md"), "Keep work safe while offline.\n")
            .expect("idea prose");
    }

    fn proposal(
        id: &str,
        kind: ProposalKind,
        from: &str,
        to: Option<&str>,
        rel: Option<EdgeRel>,
    ) -> roots_core::model::ProposalRecord {
        roots_core::model::ProposalRecord {
            id: id.to_owned(),
            kind,
            from: Some(from.to_owned()),
            to: to.map(ToOwned::to_owned),
            rel,
            reason: Some("review this".to_owned()),
            cites: vec![Citation {
                node: from.to_owned(),
                quote: "claim".to_owned(),
            }],
            by: "agent:opus".to_owned(),
            status: ProposalStatus::Pending,
            created_at: "2026-01-01T00:00:00Z".to_owned(),
            expires_at: None,
            decided_by: None,
            decided_at: None,
            decision_reason: None,
        }
    }

    fn human_io(root: &Path) -> MemoryIo {
        let mut io = MemoryIo::new(root);
        io.env = BTreeMap::from([
            ("ROOTS_USER".to_owned(), "Test Human".to_owned()),
            ("NO_COLOR".to_owned(), "1".to_owned()),
        ]);
        io
    }

    fn wait_for(control: &FakeTerminalHandle, needle: &str) {
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline {
            if control.screen().contains(needle) {
                return;
            }
            thread::sleep(Duration::from_millis(5));
        }
        panic!("did not see {needle:?}; screen was:\n{}", control.screen());
    }

    #[test]
    fn json_lists_null_card_fields_without_tty_or_mutation() {
        let root = sandbox("json");
        let paths = initialize(&root);
        let a = idea("r-a1b2", "offline-sync");
        write_jsonl_file(
            &paths.graph,
            &graph_records(&Graph {
                nodes: vec![a.clone()],
                edges: vec![],
            }),
        )
        .expect("graph");
        let mut row = proposal("p-0001", ProposalKind::Compost, &a.id, None, None);
        row.reason = None;
        write_jsonl_file(&paths.proposals, &[row.clone()]).expect("proposal");
        let mut io = human_io(&root);
        io.stdin_is_tty = false;
        assert_eq!(
            run_cli(&["tend".to_owned(), "--json".to_owned()], &mut io),
            0
        );
        let body: Value = roots_core::serde_json::from_str(&io.stdout).expect("json envelope");
        let card = &body["cards"][0];
        assert_eq!(card["type"], "proposal");
        assert_eq!(card["to"], Value::Null);
        assert_eq!(card["rel"], Value::Null);
        assert_eq!(card["reason"], Value::Null);
        assert_eq!(card["expiresAt"], Value::Null);
        assert_eq!(card["expiresInDays"], Value::Null);
        assert_eq!(card["expiringSoon"], false);
        assert_eq!(
            read_proposals(&paths).expect("read proposal")[0].status,
            ProposalStatus::Pending
        );
        assert!(read_events(&paths).expect("events").is_empty());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn terminal_accept_and_reasoned_reject_keep_one_raw_session_and_summarize() {
        let root = sandbox("decide");
        let paths = initialize(&root);
        let a = idea("r-a1b2", "offline-sync");
        let b = idea("r-c3d4", "server-truth");
        write_jsonl_file(
            &paths.graph,
            &graph_records(&Graph {
                nodes: vec![a.clone(), b.clone()],
                edges: vec![],
            }),
        )
        .expect("graph");
        let edge = proposal(
            "p-1e2d",
            ProposalKind::Edge,
            &a.id,
            Some(&b.id),
            Some(EdgeRel::Tension),
        );
        let compost = proposal("p-c0a1", ProposalKind::Compost, &b.id, None, None);
        write_jsonl_file(&paths.proposals, &[edge.clone(), compost.clone()]).expect("proposals");
        let (terminal, control) = FakeTerminal::new(72, 26);
        let mut io = human_io(&root);
        io.set_terminal(terminal);
        let join = thread::spawn(move || {
            let code = run_cli(&["tend".to_owned()], &mut io);
            (code, io)
        });
        wait_for(&control, "card 1/2");
        control.send("y");
        wait_for(&control, "card 2/2");
        control.send("rstill true\r");
        let (code, io) = join.join().expect("command thread");
        assert_eq!(code, 0, "{}", io.stderr);
        assert!(
            io.stdout
                .contains("tended 2 cards: 1 accepted, 1 rejected, 0 skipped")
        );
        let graph = read_graph(&paths).expect("graph after");
        assert_eq!(graph.edges.len(), 1);
        assert_eq!(graph.edges[0].rel, EdgeRel::Tension);
        assert_eq!(
            read_proposals(&paths)
                .expect("proposals after")
                .iter()
                .find(|row| row.id == compost.id)
                .and_then(|row| row.decision_reason.as_deref()),
            Some("still true")
        );
        // The FakeTerminal test in tend_session proves this means one active
        // raw lifetime; here the command-level flow proves the frame advances.
        assert!(control.frames().len() >= 3);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn split_accept_hands_off_to_think_and_returns_to_tend_summary() {
        let root = sandbox("split");
        let paths = initialize(&root);
        let a = idea("r-a1b2", "offline-sync");
        write_jsonl_file(
            &paths.graph,
            &graph_records(&Graph {
                nodes: vec![a.clone()],
                edges: vec![],
            }),
        )
        .expect("graph");
        write_idea_prose(&paths, &a);
        let mut split = proposal("p-5a17", ProposalKind::Split, &a.id, None, None);
        split.reason = Some("offline edits / no data loss".to_owned());
        write_jsonl_file(&paths.proposals, &[split]).expect("proposal");
        let (terminal, control) = FakeTerminal::new(72, 26);
        let mut io = human_io(&root);
        io.set_terminal(terminal);
        let join = thread::spawn(move || {
            let code = run_cli(&["tend".to_owned(), "--no-split".to_owned()], &mut io);
            (code, io)
        });
        wait_for(&control, "accept + think now");
        control.send("y");
        wait_for(&control, "split this idea");
        wait_for(&control, "offline edits / no data loss");
        control.send("q");
        let (code, io) = join.join().expect("command thread");
        assert_eq!(code, 0, "{}", io.stderr);
        assert!(io.stdout.contains("tended 1 card: 1 accepted"));
        let events = read_events(&paths).expect("events");
        let accept = events
            .iter()
            .position(|event| event.event_type == "accept")
            .expect("accept event");
        let session = events
            .iter()
            .position(|event| event.event_type == "session.start")
            .expect("session start");
        assert!(accept < session);
        let _ = fs::remove_dir_all(root);
    }
}
