//! Read-only queue construction for ideas that need a human think session.

use crate::actor::is_human_actor;
use crate::claims::read_claim;
use crate::config::RootsConfig;
use crate::events::read_events;
use crate::graph::find_node;
use crate::model::{Graph, NodeKind, NodeRecord, QuestionRecord, QuestionStatus};
use crate::paths::RootsPaths;
use crate::questions::read_questions;
use crate::rules::{RuleCandidate, RuleContext, new_candidates};
use crate::time_crate::{OffsetDateTime, format_description::well_known::Rfc3339};

#[derive(Clone, Debug)]
pub struct NodeActivity {
    pub sessions: u64,
    pub last_touched: String,
}

#[derive(Clone, Debug)]
pub struct IdeaAttention {
    pub node: NodeRecord,
    pub statement: String,
    pub due: Vec<QuestionRecord>,
    pub candidates: Vec<RuleCandidate>,
    pub activity: NodeActivity,
}

#[must_use]
pub fn is_due(question: &QuestionRecord, now: OffsetDateTime) -> bool {
    if question.status == QuestionStatus::Open {
        return true;
    }
    if question.status != QuestionStatus::Snoozed {
        return false;
    }
    match question.snoozed_until.as_deref() {
        None => true,
        Some(value) => OffsetDateTime::parse(value, &Rfc3339)
            .map(|until| until <= now)
            // `Date.parse` produces NaN for invalid values; NaN is not due.
            .unwrap_or(false),
    }
}

#[must_use]
pub fn has_findings(question: &QuestionRecord) -> bool {
    question
        .findings
        .as_ref()
        .is_some_and(|findings| !findings.is_empty())
}

#[must_use]
pub fn due_questions(
    rows: &[QuestionRecord],
    node: &str,
    now: OffsetDateTime,
) -> Vec<QuestionRecord> {
    let mut output = rows
        .iter()
        .filter(|question| question.node == node && is_due(question, now))
        .cloned()
        .collect::<Vec<_>>();
    output.sort_by(|left, right| {
        has_findings(right)
            .cmp(&has_findings(left))
            .then_with(|| left.created_at.cmp(&right.created_at))
    });
    output
}

#[must_use]
pub fn node_activity(events: &[crate::model::EventRecord], node: &NodeRecord) -> NodeActivity {
    let mut sessions = 0;
    let mut last_touched = node
        .updated_at
        .clone()
        .unwrap_or_else(|| node.created_at.clone());
    for event in events {
        if event.node.as_deref() != Some(node.id.as_str()) {
            continue;
        }
        if event.event_type == "session.end" {
            sessions += 1;
        }
        if is_human_actor(&event.by) && event.at > last_touched {
            last_touched = event.at.clone();
        }
    }
    NodeActivity {
        sessions,
        last_touched,
    }
}

#[must_use]
pub fn is_queueable(node: &NodeRecord) -> bool {
    node.kind == NodeKind::Idea
        && matches!(node.status.as_str(), "planted" | "shaping" | "committed")
}

#[must_use]
pub fn needs(attention: &IdeaAttention) -> usize {
    attention.due.len() + attention.candidates.len()
}

#[must_use]
pub fn findings_ready(attention: &IdeaAttention) -> usize {
    attention
        .due
        .iter()
        .filter(|question| has_findings(question))
        .count()
}

fn status_rank(node: &NodeRecord) -> usize {
    match node.status.as_str() {
        "planted" => 0,
        "shaping" => 1,
        "committed" => 2,
        _ => 9,
    }
}

/// Queueable ideas with an existing due question or a fresh deterministic rule candidate.
pub fn collect_attention(
    paths: &RootsPaths,
    graph: &Graph,
    config: &RootsConfig,
    now: OffsetDateTime,
) -> crate::errors::RootsResult<Vec<IdeaAttention>> {
    let questions = read_questions(paths)?;
    let events = read_events(paths)?;
    let mut output = Vec::new();
    for node in &graph.nodes {
        if !is_queueable(node) {
            continue;
        }
        let claim = read_claim(paths, graph, node)?;
        let activity = node_activity(&events, node);
        let lookup = |id: &str| find_node(graph, id);
        let context = RuleContext {
            node,
            text: &claim.text,
            edges: graph
                .edges
                .iter()
                .filter(|edge| edge.from == node.id || edge.to == node.id)
                .collect(),
            lookup: &lookup,
            session_number: activity.sessions + 1,
            last_touched: &activity.last_touched,
            now,
            thresholds: &config.questions,
        };
        output.push(IdeaAttention {
            node: node.clone(),
            statement: claim.statement,
            due: due_questions(&questions, &node.id, now),
            candidates: if claim.prose.exists {
                new_candidates(&context, &questions)
            } else {
                Vec::new()
            },
            activity,
        });
    }
    Ok(output)
}

/// Stable, most-needed-first attention ordering shared by queue and think.
#[must_use]
pub fn rank_attention(mut items: Vec<IdeaAttention>) -> Vec<IdeaAttention> {
    items.retain(|item| is_queueable(&item.node) && needs(item) > 0);
    items.sort_by(|left, right| {
        (!right.due.is_empty())
            .cmp(&(!left.due.is_empty()))
            .then_with(|| (findings_ready(right) > 0).cmp(&(findings_ready(left) > 0)))
            .then_with(|| needs(right).cmp(&needs(left)))
            .then_with(|| status_rank(&left.node).cmp(&status_rank(&right.node)))
            .then_with(|| left.activity.last_touched.cmp(&right.activity.last_touched))
            .then_with(|| left.node.id.cmp(&right.node.id))
    });
    items
}
