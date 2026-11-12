//! Background heading phase for Flow.

use super::flow::FlowTrail;
use super::prompt_contract::HEADING_INSTRUCTION;
use super::think_session::{BackgroundJob, agent_actor_for, agent_environment, run_agent_command};
use roots_core::actor::Environment;
use roots_core::claims::read_claim;
use roots_core::config::RootsConfig;
use roots_core::errors::RootsResult;
use roots_core::graph::read_graph;
use roots_core::headings::{active_heading, read_headings};
use roots_core::model::{Graph, HeadingStatus, NodeKind, NodeRecord, NodeStatus};
use roots_core::paths::RootsPaths;
use roots_core::questions::read_questions;
use roots_core::tier::effective_tier;
use roots_core::time_crate::OffsetDateTime;
use std::collections::BTreeSet;
use std::sync::mpsc;
use std::thread;

#[must_use]
pub fn heading_enabled(config: &RootsConfig) -> bool {
    config.flow_heading && config.tier >= 1 && config.agent.command.is_some()
}

fn visible(config: &RootsConfig, node: &NodeRecord) -> bool {
    node.kind == NodeKind::Idea
        && !matches!(
            node.status,
            NodeStatus::Idea(roots_core::model::IdeaStatus::Composted)
        )
        && effective_tier(config, Some(node)).tier >= 1
}

fn trail_lines(trail: &FlowTrail) -> Vec<String> {
    let mut lines = trail
        .entries
        .iter()
        .map(|entry| {
            format!(
                "- {} {} ({}): {} answered, {} open{}",
                entry.id,
                entry.slug,
                entry.status,
                entry.answered,
                entry.open,
                if trail.current.as_deref() == Some(entry.id.as_str()) {
                    "  ← just now"
                } else {
                    ""
                }
            )
        })
        .collect::<Vec<_>>();
    if !trail.planted.is_empty() {
        lines.push(format!("- planted: {}", trail.planted.join(", ")));
    }
    if !trail.adopted.is_empty() {
        lines.push(format!(
            "- adopted from sprouts: {}",
            trail.adopted.join(", ")
        ));
    }
    if trail.accepted > 0 {
        lines.push(format!("- accepted {} proposal(s)", trail.accepted));
    }
    if lines.is_empty() {
        lines.push("(nothing yet)".to_owned());
    }
    lines
}

fn idea_blocks(
    paths: &RootsPaths,
    graph: &Graph,
    nodes: &[NodeRecord],
) -> RootsResult<Vec<String>> {
    let mut lines = Vec::new();
    for node in nodes {
        let claim = read_claim(paths, graph, node)?;
        lines.extend([
            format!("### {} {} ({})", node.id, node.slug, node.status.as_str()),
            String::new(),
            "```markdown".to_owned(),
            claim.text.trim_end().to_owned(),
            "```".to_owned(),
            String::new(),
        ]);
    }
    Ok(lines)
}

fn edge_lines(graph: &Graph, ids: &BTreeSet<String>) -> Vec<String> {
    let lines = graph
        .edges
        .iter()
        .filter(|edge| ids.contains(&edge.from) && ids.contains(&edge.to))
        .map(|edge| format!("- {} {} {}", edge.from, edge.rel.as_str(), edge.to))
        .collect::<Vec<_>>();
    if lines.is_empty() {
        vec!["(none)".to_owned()]
    } else {
        lines
    }
}

pub fn heading_packet(
    paths: &RootsPaths,
    config: &RootsConfig,
    trail: &FlowTrail,
    flow: &str,
    now: OffsetDateTime,
) -> RootsResult<String> {
    let graph = read_graph(paths)?;
    let live = graph
        .nodes
        .iter()
        .filter(|node| visible(config, node))
        .cloned()
        .collect::<Vec<_>>();
    let ids = live
        .iter()
        .map(|node| node.id.clone())
        .collect::<BTreeSet<_>>();
    let touched = trail
        .entries
        .iter()
        .map(|entry| entry.id.clone())
        .collect::<BTreeSet<_>>();
    let headings = read_headings(paths)?;
    let current = active_heading(&headings, flow);
    let questions = read_questions(paths)?
        .into_iter()
        .filter(|question| {
            ids.contains(&question.node) && roots_core::attention::is_due(question, now)
        })
        .collect::<Vec<_>>();
    let others = live
        .iter()
        .filter(|node| !touched.contains(&node.id))
        .collect::<Vec<_>>();
    let dismissed = headings
        .iter()
        .filter(|heading| heading.status == HeadingStatus::Dismissed)
        .map(|heading| format!("- {}", heading.text))
        .collect::<Vec<_>>();
    let touched_nodes = live
        .iter()
        .filter(|node| touched.contains(&node.id))
        .cloned()
        .collect::<Vec<_>>();
    let other_lines = if others.is_empty() {
        vec!["(none)".to_owned()]
    } else {
        others
            .iter()
            .map(|node| {
                read_claim(paths, &graph, node).map(|claim| {
                    format!(
                        "- {} {} ({}): \"{}\"",
                        node.id,
                        node.slug,
                        node.status.as_str(),
                        claim.statement
                    )
                })
            })
            .collect::<RootsResult<Vec<_>>>()?
    };
    let question_lines = if questions.is_empty() {
        vec!["(none)".to_owned()]
    } else {
        questions
            .iter()
            .map(|question| format!("- {} on {}: {}", question.id, question.node, question.text))
            .collect()
    };
    let mut lines = vec!["## Trail (this flow)".to_owned(), String::new()];
    lines.extend(trail_lines(trail));
    lines.extend([
        "".to_owned(),
        "## Ideas thought about this flow".to_owned(),
        String::new(),
    ]);
    lines.extend(idea_blocks(paths, &graph, &touched_nodes)?);
    lines.extend(["## Other ideas".to_owned(), String::new()]);
    lines.extend(other_lines);
    lines.extend(["".to_owned(), "## Open questions".to_owned(), String::new()]);
    lines.extend(question_lines);
    lines.extend([
        "".to_owned(),
        "## Links (accepted)".to_owned(),
        String::new(),
    ]);
    lines.extend(edge_lines(&graph, &ids));
    lines.extend([
        "".to_owned(),
        "## Current heading".to_owned(),
        String::new(),
    ]);
    lines.push(
        current
            .map(|heading| heading.text)
            .unwrap_or_else(|| "(none)".to_owned()),
    );
    lines.extend([
        "".to_owned(),
        "## Dismissed headings".to_owned(),
        String::new(),
    ]);
    if dismissed.is_empty() {
        lines.push("(none)".to_owned());
    } else {
        lines.extend(dismissed);
    }
    lines.push(String::new());
    Ok(lines.join("\n"))
}

#[must_use]
pub fn heading_prompt(flow: &str, actor: &str) -> String {
    HEADING_INSTRUCTION
        .replace("{{flow}}", flow)
        .replace("{{actor}}", actor)
}

pub struct HeadingJobInput {
    pub paths: RootsPaths,
    pub config: RootsConfig,
    pub flow: String,
    pub trail: FlowTrail,
    pub environment: Environment,
    pub timeout_ms: Option<u64>,
    pub now: OffsetDateTime,
}

pub fn start_heading_phase(input: HeadingJobInput) -> RootsResult<Option<BackgroundJob>> {
    let Some(command) = input.config.agent.command.as_deref() else {
        return Ok(None);
    };
    if !heading_enabled(&input.config) || input.trail.entries.is_empty() {
        return Ok(None);
    }
    let actor = agent_actor_for(command);
    let packet = heading_packet(
        &input.paths,
        &input.config,
        &input.trail,
        &input.flow,
        input.now,
    )?;
    let prompt = format!("{}\n---\n\n{packet}", heading_prompt(&input.flow, &actor));
    let environment = agent_environment(&input.environment, &actor, None, None, Some(&input.flow));
    let timeout = input
        .timeout_ms
        .unwrap_or(input.config.agent.timeout_seconds.saturating_mul(1000));
    let root = input.paths.root;
    let command = command.to_owned();
    let (sender, done) = mpsc::channel();
    thread::spawn(move || {
        let run = run_agent_command(&command, &root, &prompt, &environment, timeout);
        let problem = (run.status != "ok")
            .then(|| format!("agent.command {}", run.error.as_deref().unwrap_or("failed")));
        let _ = sender.send(problem);
    });
    Ok(Some(BackgroundJob {
        label: "reading the session for a heading".to_owned(),
        done,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands_capture::plant_text;
    use roots_core::config::default_config;
    use roots_core::paths::roots_paths;
    use std::fs;

    fn root() -> std::path::PathBuf {
        let root =
            std::env::temp_dir().join(format!("roots-heading-{}", roots_core::ids::random_hex(8)));
        fs::create_dir_all(&root).unwrap();
        let paths = roots_paths(&root);
        fs::create_dir_all(&paths.human).unwrap();
        fs::create_dir_all(&paths.agent).unwrap();
        for file in [
            &paths.graph,
            &paths.questions,
            &paths.events,
            &paths.headings,
        ] {
            fs::write(file, "").unwrap();
        }
        root
    }

    #[test]
    fn packet_and_prompt_keep_the_heading_contract() {
        let root = root();
        let paths = roots_paths(&root);
        let planted = plant_text(
            &paths,
            "Offline work survives reconnects.\n\nMore detail.\n",
            "human:test",
            None,
        )
        .unwrap()
        .unwrap();
        let trail = FlowTrail {
            entries: vec![super::super::flow::TrailEntry {
                id: planted.node.id.clone(),
                slug: planted.node.slug.clone(),
                status: "planted".to_owned(),
                sessions: 1,
                answered: 2,
                open: 0,
            }],
            current: Some(planted.node.id.clone()),
            planted: Vec::new(),
            adopted: Vec::new(),
            accepted: 0,
            rejected: 0,
        };
        let mut config = default_config("test");
        config.agent.command = Some("agent".to_owned());
        let packet =
            heading_packet(&paths, &config, &trail, "fl-a", OffsetDateTime::now_utc()).unwrap();
        assert!(packet.contains("## Trail (this flow)"));
        assert!(packet.contains("## Ideas thought about this flow"));
        assert!(packet.contains(&planted.node.id));
        let prompt = heading_prompt("fl-a", "agent:agent");
        assert!(prompt.contains("flow\nsession fl-a"));
        assert!(prompt.contains("--as agent:agent"));
        assert!(!prompt.contains("{{"));
        let _ = fs::remove_dir_all(root);
    }
}
