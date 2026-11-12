//! Human-facing `luuna-roots flow` command wiring.
//!
//! Flow deliberately owns one terminal card while its inline steps use the
//! same terminal during temporary hand-offs.  The persistent work remains in
//! the core and capture modules; this file supplies the concrete editor,
//! Think, and Tend adapters.

use crate::commands_capture::{
    AdoptInput, complete_adoption, plant_text, require_open_sprout, sprout_reference,
};
use crate::editor::{
    EditorPane, SplitCommand, SplitLaunch, compose_in_temp_editor, editor_hint, launch_in_split,
    resolve_pager, show_in_editor_pane,
};
use crate::interactive::flow::FlowTrail;
use crate::interactive::flow_session::{FlowDeps, FlowSteps, run_flow};
use crate::interactive::screen::ScreenGuidance;
use crate::interactive::spinner::{StatusLine, start_status};
use crate::interactive::tend_session::{TendDriver, TerminalTurn, start_tend};
use crate::interactive::terminal::Terminal;
use crate::interactive::think_session::{
    BackgroundReporter, SessionSummary, ThinkDeps, run_think_session,
};
use crate::registry::CommandContext;
use roots_core::actor::{ActorContext, Environment};
use roots_core::args::flag_bool;
use roots_core::color::Colors;
use roots_core::config::{RootsConfig, load_config};
use roots_core::decisions::{AcceptOptions, accept_proposal, reject_proposal};
use roots_core::errors::{ErrorCode, RootsError, RootsResult};
use roots_core::model::NodeKind;
use roots_core::prose::read_node_prose;
use roots_core::resolve::{ResolveOptions, resolve_node};
use roots_core::serde_json::{Map, Value, to_value};
use roots_core::sprouts::reject_sprout;
use roots_core::tend::{TendAction, TendEffect, TendInput, TendOutcome, tend_step, tend_tally};
use roots_core::time_crate::OffsetDateTime;
use roots_core::workspace::open_workspace;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub const FLOW_DESCRIPTION: &str = "Moves from idea to idea without dropping to the shell. Between think sessions a card\n\
shows what this flow did (trail), the agent's heading ([agent], cited, never in views),\n\
agent work still running, the inbox and the next idea. Keys: [enter] the recommended\n\
step (review the inbox, else think the next idea)  [n] think the next idea  [o] other\n\
idea  [p] plant  [x] dismiss the heading  [q] end the flow. Agent questions arrive\n\
while you write; links and the heading are worked out after each session. Under tmux\n\
with vim/nvim one editor pane follows the flow. With an id, starts by thinking it.";

fn relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .display()
        .to_string()
}

fn tally_line(summary: &SessionSummary) -> String {
    let tally = &summary.tally;
    let mut parts = [
        (tally.answered, "answered"),
        (tally.dismissed, "dismissed"),
        (tally.snoozed, "snoozed"),
        (tally.skipped, "skipped"),
    ]
    .into_iter()
    .filter(|(count, _)| *count > 0)
    .map(|(count, label)| format!("{count} {label}"))
    .collect::<Vec<_>>();
    if tally.delegated > 0 {
        parts.push(format!("{} handed to the agent", tally.delegated));
    }
    if tally.unasked > 0 {
        parts.push(format!("{} left for next time", tally.unasked));
    }
    if parts.is_empty() {
        "no questions".to_owned()
    } else {
        parts.join(", ")
    }
}

struct CommandFlowSteps {
    paths: roots_core::paths::RootsPaths,
    by: String,
    environment: Environment,
    cwd: PathBuf,
    colors: Colors,
    split: bool,
    pane: EditorPane,
    write_status: Arc<dyn Fn(String) + Send + Sync>,
}

impl CommandFlowSteps {
    fn run_think(
        &mut self,
        terminal: &mut dyn Terminal,
        id: &str,
        guidance: Option<ScreenGuidance>,
        background: Option<BackgroundReporter>,
    ) -> RootsResult<SessionSummary> {
        let workspace = open_workspace(&self.cwd)?;
        let node = resolve_node(
            &workspace.graph.nodes,
            id,
            ResolveOptions {
                kind: Some(NodeKind::Idea),
            },
        )?
        .clone();
        let prose = read_node_prose(&workspace.paths, &node, Some(&workspace.dirs))?;
        let file = prose
            .path
            .as_deref()
            .map(|path| relative(&workspace.paths.root, path))
            .unwrap_or_default();
        let launch = self.split.then(|| {
            show_in_editor_pane(
                &self.environment,
                &mut self.pane,
                &file,
                &workspace.paths.root,
                None,
            )
        });
        let config = load_config(&workspace.paths)?;
        run_think_session(ThinkDeps {
            paths: workspace.paths,
            config,
            node,
            by: self.by.clone(),
            terminal,
            colors: self.colors,
            editor_hint: editor_hint(launch, &file),
            watch: Default::default(),
            environment: self.environment.clone(),
            guidance,
            agent_timeout_ms: None,
            // Flow itself tracks background runs in its card, while an
            // accepted split within Tend remains a normal Think session.
            agent_status: None,
            background,
        })
    }

    fn reference_block(
        &self,
        sprout: &roots_core::model::NodeRecord,
        text: &str,
        launch: Option<SplitLaunch>,
        endorse: bool,
    ) {
        let who = sprout
            .author
            .strip_prefix("agent:")
            .unwrap_or(&sprout.author);
        let title = format!(
            "[agent] sprout {} {} ({who}) · reference only",
            sprout.id, sprout.slug
        );
        let mut lines = vec![format!("── {title} ──")];
        lines.extend(text.trim_end().split('\n').map(ToOwned::to_owned));
        lines.push("──".to_owned());
        if launch.is_some_and(|result| result.ok) {
            let mux = launch
                .expect("checked successful launch")
                .mux
                .expect("successful mux");
            lines.push(format!(
                "(also open read-only in a {} pane; quit it with q)",
                mux.as_str()
            ));
        }
        lines.push(
            if endorse {
                "Endorsing: write your thoughts on the sprout in the editor; the sprout stays the agent's."
            } else {
                "Write the idea in your own words in the editor. Do not paste the sprout."
            }
            .to_owned(),
        );
        lines.push("Save an empty file to cancel.".to_owned());
        lines.push(String::new());
        (self.write_status)(format!("{}\n", lines.join("\n")));
    }

    fn adopt_sprout(&mut self, id: &str, endorse: Option<&str>) -> RootsResult<String> {
        let sprout = require_open_sprout(&self.paths, id, None)?;
        let prose = read_node_prose(&self.paths, &sprout, None)?;
        let relative_file = prose
            .path
            .as_deref()
            .map(|path| relative(&self.paths.root, path))
            .unwrap_or_default();
        let launch = if self.split && !relative_file.is_empty() {
            Some(launch_in_split(
                &self.environment,
                SplitCommand {
                    command: &resolve_pager(&self.environment),
                    file: &relative_file,
                    cwd: &self.paths.root,
                    focus: false,
                },
                None,
            ))
        } else {
            None
        };
        let reference = if prose.text.is_empty() {
            "(sprout.md missing)"
        } else {
            &prose.text
        };
        let endorsement = endorse.is_some();
        self.reference_block(&sprout, reference, launch, endorsement);
        let buffer =
            crate::editor::reference_buffer(&sprout_reference(&sprout, reference, endorsement));
        let initial = endorse.map_or(buffer.clone(), |thoughts| format!("{thoughts}{buffer}"));
        let text = compose_in_temp_editor(&initial, &self.cwd, &self.environment)?;
        let result = complete_adoption(
            &self.paths,
            AdoptInput {
                sprout: &sprout.id,
                text: &text,
                by: &self.by,
                slug: None,
                now: None,
                endorse: endorsement,
            },
        )?;
        let verb = if result.idea.endorses.is_some() {
            "endorsed"
        } else {
            "adopted"
        };
        Ok(format!(
            "{} {verb} as {} {}",
            result.sprout.id, result.idea.id, result.idea.slug
        ))
    }

    fn apply_tend_effect(
        &mut self,
        terminal: &mut dyn Terminal,
        driver: &mut TendDriver,
        effect: TendEffect,
    ) -> RootsResult<(TendOutcome, Option<String>, bool)> {
        let now = OffsetDateTime::now_utc();
        match (effect.card, effect.action) {
            (roots_core::tend::TendCard::Proposal(card), TendAction::Reject { reason }) => {
                let decision = reject_proposal(
                    &self.paths,
                    &self.by,
                    &card.id,
                    reason.as_deref(),
                    Some(now),
                )?;
                Ok((
                    TendOutcome::Rejected,
                    Some(format!("{} {}", decision.id, decision.action)),
                    false,
                ))
            }
            (roots_core::tend::TendCard::Proposal(card), TendAction::Accept { rel, keep }) => {
                let decision = accept_proposal(
                    &self.paths,
                    &self.by,
                    &card.id,
                    AcceptOptions {
                        rel: rel.map(|relation| relation.as_str().to_owned()),
                        keep,
                        reason: None,
                    },
                    Some(now),
                )?;
                let notice = decision.edge.as_ref().map_or_else(
                    || format!("{} {}", decision.id, decision.action),
                    |edge| {
                        format!(
                            "{}: {} {} {}",
                            edge.id,
                            edge.from,
                            edge.rel.as_str(),
                            edge.to
                        )
                    },
                );
                if let Some(think) = decision.think {
                    driver.stop(terminal);
                    let summary = self.run_think(
                        terminal,
                        &think.node,
                        Some(ScreenGuidance {
                            label: "split this idea".to_owned(),
                            source: "agent, accepted".to_owned(),
                            text: think.guidance,
                        }),
                        None,
                    )?;
                    return Ok((
                        TendOutcome::Accepted,
                        Some(format!(
                            "think session {}: {}",
                            summary.session,
                            tally_line(&summary)
                        )),
                        true,
                    ));
                }
                Ok((TendOutcome::Accepted, Some(notice), false))
            }
            (roots_core::tend::TendCard::Sprout(card), TendAction::Reject { reason }) => {
                let sprout = reject_sprout(
                    &self.paths,
                    &self.by,
                    &card.id,
                    reason.as_deref(),
                    Some(now),
                )?;
                Ok((
                    TendOutcome::Rejected,
                    Some(format!("{} {}", sprout.id, sprout.status.as_str())),
                    false,
                ))
            }
            (roots_core::tend::TendCard::Sprout(card), TendAction::Accept { .. }) => {
                driver.stop(terminal);
                let notice = self.adopt_sprout(&card.id, None)?;
                Ok((TendOutcome::Accepted, Some(notice), true))
            }
            (roots_core::tend::TendCard::Sprout(card), TendAction::Endorse { text, editor }) => {
                if editor {
                    driver.stop(terminal);
                    let thoughts = (!text.is_empty()).then(|| format!("{text}\n"));
                    let notice = self.adopt_sprout(&card.id, thoughts.as_deref())?;
                    return Ok((TendOutcome::Accepted, Some(notice), true));
                }
                let adoption = complete_adoption(
                    &self.paths,
                    AdoptInput {
                        sprout: &card.id,
                        text: &format!("{text}\n"),
                        by: &self.by,
                        slug: None,
                        now: Some(now),
                        endorse: true,
                    },
                )?;
                Ok((
                    TendOutcome::Accepted,
                    Some(format!(
                        "{} endorsed as {} {}",
                        adoption.sprout.id, adoption.idea.id, adoption.idea.slug
                    )),
                    false,
                ))
            }
            (_, action) => Err(RootsError::error(format!(
                "nothing to decide for {action:?}"
            ))),
        }
    }

    fn run_tend(&mut self, terminal: &mut dyn Terminal) -> RootsResult<String> {
        let mut run = start_tend(&self.paths, OffsetDateTime::now_utc())?;
        let cards = run.state.cards.len();
        if run.state.ended {
            return Ok("nothing to tend".to_owned());
        }
        let mut driver = TendDriver::start(
            terminal,
            &run.state,
            run.expired.len(),
            run.expired_sprouts.len(),
            self.colors,
            OffsetDateTime::now_utc(),
        );
        while !run.state.ended {
            match driver.next(
                terminal,
                &mut run.state,
                run.expired.len(),
                run.expired_sprouts.len(),
                self.colors,
                OffsetDateTime::now_utc(),
            )? {
                TerminalTurn::Finished => break,
                TerminalTurn::Effect(effect) => {
                    let applied = self.apply_tend_effect(terminal, &mut driver, *effect);
                    let (completed, handoff) = match applied {
                        Ok((outcome, notice, did_handoff)) => (Ok((outcome, notice)), did_handoff),
                        Err(problem) if problem.code == ErrorCode::Cancelled => (
                            Ok((
                                TendOutcome::Skipped,
                                Some("adoption cancelled; nothing written".to_owned()),
                            )),
                            true,
                        ),
                        Err(problem) => (Err(problem.message), true),
                    };
                    run.state = tend_step(run.state, TendInput::Done { result: completed }).state;
                    if run.state.ended {
                        driver.stop(terminal);
                    } else if handoff {
                        driver.restart(
                            terminal,
                            &run.state,
                            run.expired.len(),
                            run.expired_sprouts.len(),
                            self.colors,
                            OffsetDateTime::now_utc(),
                        );
                    } else {
                        driver.render(
                            terminal,
                            &run.state,
                            run.expired.len(),
                            run.expired_sprouts.len(),
                            self.colors,
                            OffsetDateTime::now_utc(),
                        );
                    }
                }
            }
        }
        let tally = tend_tally(&run.state);
        Ok(format!(
            "tended {cards}: {} accepted, {} rejected, {} skipped",
            tally.accepted, tally.rejected, tally.skipped
        ))
    }
}

impl FlowSteps for CommandFlowSteps {
    fn think(
        &mut self,
        terminal: &mut dyn Terminal,
        id: &str,
        background: BackgroundReporter,
    ) -> RootsResult<SessionSummary> {
        self.run_think(terminal, id, None, Some(background))
    }

    fn tend(&mut self, terminal: &mut dyn Terminal) -> RootsResult<String> {
        self.run_tend(terminal)
    }

    fn plant(&mut self, _terminal: &mut dyn Terminal) -> RootsResult<Option<String>> {
        (self.write_status)(
            "Write the idea in your editor: one claim on the first line. Empty cancels.\n"
                .to_owned(),
        );
        let text = compose_in_temp_editor("", &self.cwd, &self.environment)?;
        Ok(plant_text(&self.paths, &text, &self.by, None)?.map(|plant| plant.node.id))
    }

    fn tally(&self, summary: &SessionSummary) -> String {
        tally_line(summary)
    }
}

fn trail_lines(context: &CommandContext<'_>, trail: &FlowTrail) -> Vec<String> {
    let mut lines = trail
        .entries
        .iter()
        .map(|entry| {
            format!(
                "  {} {}  {}",
                context.out.colors.id(&entry.id),
                entry.slug,
                context.out.colors.dim(&format!(
                    "{} answered · {} open · {}",
                    entry.answered, entry.open, entry.status
                ))
            )
        })
        .collect::<Vec<_>>();
    let extras = [
        (!trail.planted.is_empty()).then(|| format!("planted {}", trail.planted.join(", "))),
        (!trail.adopted.is_empty()).then(|| format!("adopted {}", trail.adopted.join(", "))),
        (trail.accepted > 0).then(|| format!("accepted {}", trail.accepted)),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>();
    if !extras.is_empty() {
        lines.push(format!("  {}", context.out.colors.dim(&extras.join(" · "))));
    }
    lines
}

/// `luuna-roots flow [<id>]`.
pub fn flow_command(mut context: CommandContext<'_>) -> RootsResult<()> {
    let environment = context.io.environment();
    roots_core::guard::require_tty(&environment, context.io.stdin_is_tty(), "flow")?;
    let by = roots_core::guard::require_human_command(
        ActorContext::new(context.io.cwd(), &environment),
        "flow",
    )?;
    if context.io.terminal().is_none() {
        return Err(RootsError::guard(
            "`luuna-roots flow` needs an interactive terminal",
        ));
    }
    let workspace = open_workspace(context.io.cwd())?;
    let start = context
        .args
        .first()
        .map(|raw| {
            resolve_node(
                &workspace.graph.nodes,
                raw,
                ResolveOptions {
                    kind: Some(NodeKind::Idea),
                },
            )
            .map(|node| node.id.clone())
        })
        .transpose()?;
    let config: RootsConfig = load_config(&workspace.paths)?;
    let write_status = context.io.agent_status_writer();
    let stdout_is_tty = context.io.stdout_is_tty();
    let mut steps = CommandFlowSteps {
        paths: workspace.paths.clone(),
        by: by.clone(),
        environment: environment.clone(),
        cwd: context.io.cwd().to_path_buf(),
        colors: context.out.colors,
        split: !flag_bool(context.flags, "no-split"),
        pane: EditorPane::default(),
        write_status: Arc::clone(&write_status),
    };
    let (mut summary, columns) = {
        let terminal = context
            .io
            .terminal()
            .ok_or_else(|| RootsError::guard("`luuna-roots flow` needs an interactive terminal"))?;
        let columns = terminal.columns_reader();
        let summary = run_flow(
            FlowDeps {
                paths: workspace.paths.clone(),
                config,
                by,
                colors: context.out.colors,
                environment,
                start,
                refresh_ms: None,
                agent_timeout_ms: None,
            },
            terminal,
            &mut steps,
        )?;
        (summary, columns)
    };
    if !summary.running.is_empty() {
        let mut status = start_status(
            write_status,
            format!("waiting for the agent: {}…", summary.running.join(", ")),
            stdout_is_tty,
            Some(columns),
        );
        summary.wait_for_background();
        status.stop(None);
    }
    context.io.flush_agent_status();
    let mut body = Map::new();
    body.insert("flow".to_owned(), Value::String(summary.flow.clone()));
    body.insert(
        "trail".to_owned(),
        to_value(&summary.trail).map_err(|problem| RootsError::error(problem.to_string()))?,
    );
    body.insert(
        "problems".to_owned(),
        Value::Array(
            summary
                .problems
                .iter()
                .cloned()
                .map(Value::String)
                .collect(),
        ),
    );
    context.out.result(context.io, body);
    context.out.success(
        context.io,
        &format!("flow {} ended", context.out.colors.id(&summary.flow)),
    );
    let trail_lines = trail_lines(&context, &summary.trail);
    context.out.lines(context.io, &trail_lines);
    for problem in &summary.problems {
        context.out.warn(context.io, problem);
    }
    crate::commands_capture::print_next(&mut context, &workspace.paths);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interactive::terminal::FakeTerminal;
    use crate::io::MemoryIo;
    use crate::run_cli;
    use roots_core::config::{default_config, write_config};
    use roots_core::errors::{EXIT_GUARD, EXIT_NOT_FOUND};
    use roots_core::paths::roots_paths;
    use std::collections::BTreeMap;
    use std::fs;

    fn initialized(label: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!(
            "roots-flow-command-{label}-{}",
            roots_core::ids::random_hex(8)
        ));
        fs::create_dir_all(&root).unwrap();
        let paths = roots_paths(&root);
        fs::create_dir_all(&paths.human).unwrap();
        fs::create_dir_all(&paths.agent).unwrap();
        fs::create_dir_all(&paths.sprouts).unwrap();
        fs::create_dir_all(&paths.notes).unwrap();
        write_config(&paths, &default_config("flow-test")).unwrap();
        for file in [
            &paths.graph,
            &paths.events,
            &paths.questions,
            &paths.proposals,
            &paths.headings,
        ] {
            fs::write(file, "").unwrap();
        }
        root
    }

    fn human_io(root: &Path) -> MemoryIo {
        let mut io = MemoryIo::new(root);
        io.env = BTreeMap::from([
            ("ROOTS_USER".to_owned(), "Test Human".to_owned()),
            ("NO_COLOR".to_owned(), "1".to_owned()),
        ]);
        io
    }

    #[test]
    fn flow_requires_a_tty_even_for_json() {
        let root = initialized("tty");
        let mut io = human_io(&root);
        io.stdin_is_tty = false;
        let code = run_cli(&["flow".to_owned(), "--json".to_owned()], &mut io);
        assert_eq!(code, EXIT_GUARD);
        assert!(io.stdout.contains("interactive terminal"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn flow_rejects_an_unknown_start_id_before_opening_the_card() {
        let root = initialized("unknown");
        let mut io = human_io(&root);
        io.set_terminal(FakeTerminal::new(70, 24).0);
        let code = run_cli(&["flow".to_owned(), "r-missing".to_owned()], &mut io);
        assert_eq!(code, EXIT_NOT_FOUND);
        assert!(io.stderr.contains("r-missing"), "{}", io.stderr);
        let _ = fs::remove_dir_all(root);
    }
}
