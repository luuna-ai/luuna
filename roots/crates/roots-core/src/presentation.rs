//! Read-side context and document builders used by the public CLI.
//!
//! This module intentionally contains no terminal colors or process I/O.  It
//! produces deterministic data and Markdown/HTML text while the CLI owns the
//! command envelope and human-facing styling.

use crate::blame::{answer_blocks, build_blame};
use crate::claims::{EndorsedClaim, read_claim};
use crate::config::RootsConfig;
use crate::errors::RootsResult;
use crate::graph::find_node;
use crate::integrations::{
    AtlasLearning, LinkedIssue, atlas_learnings_for, linked_issues, read_missions_issues,
};
use crate::model::{
    EdgeRel, Graph, NodeKind, NodeRecord, ProposalKind, ProposalRecord, ProposalStatus,
    QuestionRecord, QuestionStatus,
};
use crate::notes::{NoteEntry, list_notes};
use crate::paths::RootsPaths;
use crate::proposals::read_proposals;
use crate::questions::{QUESTION_MAX_LENGTH, open_agent_questions, read_questions};
use crate::sprouts::open_sprouts;
use crate::tier::{TierSource, effective_tier};
use crate::time_crate::OffsetDateTime;
use serde::Serialize;
use serde::ser::{SerializeMap, Serializer};
use serde_json::{Map, Value, to_value};
use std::collections::{HashMap, HashSet};
use std::path::Path;

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct EndorsedView {
    pub sprout: String,
    pub slug: String,
    pub by: String,
    pub statement: String,
    pub body: String,
    pub changed: bool,
}

fn endorsed_view(graph: &Graph, endorsed: &EndorsedClaim) -> EndorsedView {
    let sprout = find_node(graph, &endorsed.sprout);
    EndorsedView {
        sprout: endorsed.sprout.clone(),
        slug: sprout.map(|node| node.slug.clone()).unwrap_or_default(),
        by: sprout
            .map(|node| node.author.clone())
            .unwrap_or_else(|| "agent:?".to_owned()),
        statement: endorsed.statement.clone(),
        body: endorsed.body.clone(),
        changed: endorsed.changed,
    }
}

#[must_use]
pub fn endorsed_label(endorsed: &EndorsedView, endorsed_by: Option<&str>) -> String {
    let by = endorsed.by.strip_prefix("agent:").unwrap_or(&endorsed.by);
    let human = endorsed_by
        .map(|actor| format!(" · endorsed by {actor}"))
        .unwrap_or_else(|| " · endorsed".to_owned());
    let changed = if endorsed.changed {
        " · CHANGED since endorsement"
    } else {
        ""
    };
    format!("[agent] {by} · {}{human}{changed}", endorsed.sprout)
}

#[must_use]
pub fn endorsed_markdown(endorsed: &EndorsedView, endorsed_by: Option<&str>) -> Vec<String> {
    let text = if endorsed.body.is_empty() {
        endorsed.statement.clone()
    } else {
        format!("{}\n\n{}", endorsed.statement, endorsed.body)
    };
    let quoted = (if text.is_empty() {
        "(sprout.md missing)"
    } else {
        &text
    })
    .split('\n')
    .map(|line| {
        if line.is_empty() {
            ">".to_owned()
        } else {
            format!("> {line}")
        }
    })
    .collect::<Vec<_>>();
    let mut output = vec![
        format!("> _{}_", endorsed_label(endorsed, endorsed_by)),
        ">".to_owned(),
    ];
    output.extend(quoted);
    output
}

#[must_use]
pub fn relative_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}

#[must_use]
pub fn edge_label(rel: EdgeRel, direction: &str) -> (&'static str, &'static str) {
    if rel == EdgeRel::Tension {
        return ("tension", "↔");
    }
    if direction == "out" {
        return (rel.as_str(), "→");
    }
    match rel {
        EdgeRel::Serves => ("served by", "←"),
        EdgeRel::Replaces => ("replaced by", "←"),
        EdgeRel::Derives => ("adopted as", "←"),
        EdgeRel::Tension => ("tension", "↔"),
    }
}

// ── Context ──────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextNode {
    pub id: String,
    pub kind: NodeKind,
    pub slug: String,
    pub status: crate::model::NodeStatus,
    pub author: String,
    pub created_at: String,
    pub updated_at: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct TierPacket {
    pub tier: u8,
    pub name: String,
    pub source: String,
    pub config: u8,
}

#[derive(Clone, Debug, Serialize)]
pub struct ContextNeighbor {
    pub edge: String,
    pub rel: EdgeRel,
    pub direction: String,
    pub id: String,
    pub slug: String,
    pub kind: Option<NodeKind>,
    pub status: Option<crate::model::NodeStatus>,
    pub statement: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextQuestion {
    pub id: String,
    pub text: String,
    pub by: String,
    pub status: QuestionStatus,
    pub created_at: String,
    pub findings: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct ContextRejection {
    pub id: String,
    pub kind: ProposalKind,
    pub from: Option<String>,
    pub to: Option<String>,
    pub rel: Option<EdgeRel>,
    pub reason: Option<String>,
    pub decision_reason: Option<String>,
    pub by: String,
}

/// Edge mention proposals intentionally retain `rel: null`; unrelated
/// proposal kinds omit a relation just as their JavaScript objects do.
impl Serialize for ContextRejection {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("id", &self.id)?;
        map.serialize_entry("kind", &self.kind)?;
        if let Some(value) = &self.from {
            map.serialize_entry("from", value)?;
        }
        if let Some(value) = &self.to {
            map.serialize_entry("to", value)?;
        }
        if self.kind == ProposalKind::Edge {
            map.serialize_entry("rel", &self.rel)?;
        }
        if let Some(value) = &self.reason {
            map.serialize_entry("reason", value)?;
        }
        if let Some(value) = &self.decision_reason {
            map.serialize_entry("decisionReason", value)?;
        }
        map.serialize_entry("by", &self.by)?;
        map.end()
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct RepoState {
    pub branch: Option<String>,
    pub commits: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextLimits {
    pub questions_per_session: u64,
    pub agent_questions_open: usize,
    pub asks_remaining: u64,
    pub question_max_length: usize,
}

#[derive(Clone, Debug, Serialize)]
pub struct ContextPacket {
    pub project: String,
    pub node: ContextNode,
    pub statement: String,
    pub body: String,
    pub endorsed: Option<EndorsedView>,
    pub thoughts: Option<String>,
    pub path: Option<String>,
    pub tier: TierPacket,
    pub session: Option<String>,
    pub neighbors: Vec<ContextNeighbor>,
    pub questions: Vec<ContextQuestion>,
    #[serde(rename = "rejectedProposals")]
    pub rejected_proposals: Vec<ContextRejection>,
    pub notes: Vec<NoteEntry>,
    pub seeds: Vec<LinkedIssue>,
    pub mulch: Vec<AtlasLearning>,
    pub limits: ContextLimits,
    pub repo: Option<RepoState>,
}

fn context_neighbors(
    paths: &RootsPaths,
    graph: &Graph,
    node: &NodeRecord,
) -> RootsResult<Vec<ContextNeighbor>> {
    graph
        .edges
        .iter()
        .filter(|edge| edge.from == node.id || edge.to == node.id)
        .map(|edge| {
            let direction = if edge.from == node.id { "out" } else { "in" };
            let other_id = if direction == "out" {
                &edge.to
            } else {
                &edge.from
            };
            let other = find_node(graph, other_id);
            let statement = other
                .map(|other| read_claim(paths, graph, other).map(|claim| claim.statement))
                .transpose()?
                .unwrap_or_default();
            Ok(ContextNeighbor {
                edge: edge.id.clone(),
                rel: edge.rel,
                direction: direction.to_owned(),
                id: other_id.clone(),
                slug: other
                    .map(|other| other.slug.clone())
                    .unwrap_or_else(|| "?".to_owned()),
                kind: other.map(|other| other.kind),
                status: other.map(|other| other.status),
                statement,
            })
        })
        .collect()
}

fn involves(proposal: &ProposalRecord, id: &str) -> bool {
    proposal.from.as_deref() == Some(id)
        || proposal.to.as_deref() == Some(id)
        || proposal.cites.iter().any(|citation| citation.node == id)
}

fn context_rejections(rows: &[ProposalRecord], id: &str) -> Vec<ContextRejection> {
    rows.iter()
        .filter(|proposal| proposal.status == ProposalStatus::Rejected && involves(proposal, id))
        .map(|proposal| ContextRejection {
            id: proposal.id.clone(),
            kind: proposal.kind,
            from: proposal.from.clone(),
            to: proposal.to.clone(),
            rel: proposal.rel,
            reason: proposal.reason.clone(),
            decision_reason: proposal.decision_reason.clone(),
            by: proposal.by.clone(),
        })
        .collect()
}

/// Build the structured packet for a single idea or sprout.  Repository state
/// is supplied by the CLI because it is process-dependent and failsafe.
pub fn build_context(
    paths: &RootsPaths,
    graph: &Graph,
    config: &RootsConfig,
    node: &NodeRecord,
    session: Option<String>,
    repo: Option<RepoState>,
) -> RootsResult<ContextPacket> {
    let claim = read_claim(paths, graph, node)?;
    let endorsed = claim
        .endorsed
        .as_ref()
        .map(|value| endorsed_view(graph, value));
    let mut questions = read_questions(paths)?;
    questions.retain(|question| question.node == node.id);
    questions.sort_by(|left, right| left.created_at.cmp(&right.created_at));
    let all_questions = read_questions(paths)?;
    let effective = effective_tier(config, Some(node));
    let open = open_agent_questions(&all_questions, &node.id).len();
    let cap = config.limits.questions_per_session;
    let issues = read_missions_issues(&paths.root);
    Ok(ContextPacket {
        project: config.project.clone(),
        node: ContextNode {
            id: node.id.clone(),
            kind: node.kind,
            slug: node.slug.clone(),
            status: node.status,
            author: node.author.clone(),
            created_at: node.created_at.clone(),
            updated_at: node.updated_at.clone(),
        },
        statement: claim.statement.clone(),
        body: if endorsed
            .as_ref()
            .is_some_and(|value| !value.statement.is_empty())
        {
            endorsed
                .as_ref()
                .map(|value| value.body.clone())
                .unwrap_or_default()
        } else {
            claim.prose.body.clone()
        },
        endorsed,
        thoughts: claim
            .endorsed
            .as_ref()
            .map(|_| claim.prose.text.trim().to_owned()),
        path: claim
            .prose
            .path
            .as_deref()
            .map(|path| relative_path(&paths.root, path)),
        tier: TierPacket {
            tier: effective.tier,
            name: effective.name.to_owned(),
            source: match effective.source {
                TierSource::Idea => "idea",
                TierSource::Config => "config",
            }
            .to_owned(),
            config: config.tier,
        },
        session,
        neighbors: context_neighbors(paths, graph, node)?,
        questions: questions
            .into_iter()
            .map(|question| ContextQuestion {
                id: question.id,
                text: question.text,
                by: question.by,
                status: question.status,
                created_at: question.created_at,
                findings: question
                    .findings
                    .unwrap_or_default()
                    .into_iter()
                    .map(|finding| finding.note)
                    .collect(),
            })
            .collect(),
        rejected_proposals: context_rejections(&read_proposals(paths)?, &node.id),
        notes: list_notes(paths, node),
        seeds: linked_issues(&issues, &node.id),
        mulch: atlas_learnings_for(&paths.root, &node.id),
        limits: ContextLimits {
            questions_per_session: cap,
            agent_questions_open: open,
            asks_remaining: cap.saturating_sub(open as u64),
            question_max_length: QUESTION_MAX_LENGTH,
        },
        repo,
    })
}

fn quote(text: &str) -> String {
    text.split('\n')
        .map(|line| {
            if line.is_empty() {
                ">".to_owned()
            } else {
                format!("> {line}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn section(title: &str, lines: Vec<String>, empty: &str) -> Vec<String> {
    let mut output = vec![String::new(), format!("## {title}"), String::new()];
    if lines.is_empty() {
        output.push(empty.to_owned());
    } else {
        output.extend(lines);
    }
    output
}

fn date_part(value: &str) -> &str {
    value.get(..10).unwrap_or(value)
}

/// Compact agent-facing Markdown context packet.
#[must_use]
pub fn render_context_markdown(packet: &ContextPacket) -> String {
    let source = if packet.tier.source == "idea" {
        "per-idea override"
    } else {
        "project default"
    };
    let mut lines = vec![
        format!(
            "# luuna-roots context: {} {}",
            packet.node.id, packet.node.slug
        ),
        String::new(),
        format!("- project: {}", packet.project),
        format!(
            "- status: {} · planted {} by {}",
            packet.node.status.as_str(),
            date_part(&packet.node.created_at),
            packet.node.author
        ),
        format!(
            "- agent tier: {} ({}, {source})",
            packet.tier.tier, packet.tier.name
        ),
    ];
    if let Some(session) = &packet.session {
        lines.push(format!("- think session: {session}"));
    }
    if let Some(path) = &packet.path {
        lines.push(format!("- file: {path} (human-owned: never edit it)"));
    }
    if let Some(endorsed) = &packet.endorsed {
        lines.extend([
            String::new(),
            "## Idea".to_owned(),
            String::new(),
            "The human endorsed this agent text; it is the idea's claim:".to_owned(),
            String::new(),
        ]);
        lines.extend(endorsed_markdown(endorsed, Some(&packet.node.author)));
        lines.extend([
            String::new(),
            "Their thoughts on it (idea.md):".to_owned(),
            String::new(),
            packet
                .thoughts
                .clone()
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "(empty)".to_owned()),
        ]);
    } else {
        lines.extend([
            String::new(),
            "## Idea".to_owned(),
            String::new(),
            quote(if packet.statement.is_empty() {
                "(empty)"
            } else {
                &packet.statement
            }),
        ]);
        if !packet.body.is_empty() {
            lines.extend([String::new(), packet.body.clone()]);
        }
    }
    lines.extend(section(
        "Neighbors",
        packet
            .neighbors
            .iter()
            .map(|neighbor| {
                let (label, arrow) = edge_label(neighbor.rel, &neighbor.direction);
                let status = neighbor
                    .status
                    .map(|status| format!(" ({})", status.as_str()))
                    .unwrap_or_default();
                let statement = if neighbor.statement.is_empty() {
                    String::new()
                } else {
                    format!(": \"{}\"", neighbor.statement)
                };
                format!(
                    "- {label} {arrow} {} {}{status}{statement}",
                    neighbor.id, neighbor.slug
                )
            })
            .collect(),
        "(none)",
    ));
    lines.extend(section(
        "Prior questions (never repeat these; dismissed = the human refused it)",
        packet
            .questions
            .iter()
            .map(|question| {
                let findings = if question.findings.is_empty() {
                    String::new()
                } else {
                    format!("; findings: {}", question.findings.join(", "))
                };
                format!(
                    "- [{}] {} ({}, {}{findings})",
                    question.status.as_str(),
                    question.text,
                    question.id,
                    question.by
                )
            })
            .collect(),
        "(none yet)",
    ));
    let delegated = packet
        .questions
        .iter()
        .filter(|question| question.status == QuestionStatus::Delegated)
        .map(|question| format!("- {}: {}", question.id, question.text))
        .collect::<Vec<_>>();
    if !delegated.is_empty() {
        lines.extend(section(
            &format!("Delegated questions (research these; attach findings with `luuna-roots note {} --question <q-id> --file <md>`)", packet.node.id),
            delegated,
            "",
        ));
    }
    lines.extend(section(
        "Rejected proposals (do not suggest these again)",
        packet
            .rejected_proposals
            .iter()
            .map(|rejection| {
                let pair = match (&rejection.from, &rejection.to) {
                    (Some(from), Some(to)) => format!(
                        " {from} {} {to}",
                        rejection.rel.map(EdgeRel::as_str).unwrap_or("?")
                    ),
                    _ => String::new(),
                };
                let reason = rejection
                    .reason
                    .as_ref()
                    .map(|reason| format!(" (\"{reason}\")"))
                    .unwrap_or_default();
                let why = rejection
                    .decision_reason
                    .as_ref()
                    .map(|reason| format!("; human said: \"{reason}\""))
                    .unwrap_or_default();
                format!(
                    "- {} {}{pair}{reason}{why}",
                    rejection.id,
                    match rejection.kind {
                        ProposalKind::Edge => "edge",
                        ProposalKind::Split => "split",
                        ProposalKind::Merge => "merge",
                        ProposalKind::Compost => "compost",
                    }
                )
            })
            .collect(),
        "(none)",
    ));
    lines.extend(section(
        "Agent notes",
        packet
            .notes
            .iter()
            .map(|note| format!("- {}", note.path))
            .collect(),
        "(none)",
    ));
    if !packet.seeds.is_empty() {
        lines.extend(section(
            "Missions issues (the work that realizes this idea)",
            packet
                .seeds
                .iter()
                .map(|issue| format!("- {} [{}] {}", issue.id, issue.status, issue.title))
                .collect(),
            "",
        ));
    }
    if !packet.mulch.is_empty() {
        lines.extend(section(
            "Atlas expertise citing this idea",
            packet
                .mulch
                .iter()
                .map(|learning| {
                    format!(
                        "- {} {}/{}: {}",
                        learning.id.as_deref().unwrap_or("(no id)"),
                        learning.domain,
                        learning.record_type,
                        learning.summary
                    )
                })
                .collect(),
            "",
        ));
    }
    if let Some(repo) = &packet.repo {
        lines.extend([String::new(), "## Repo".to_owned(), String::new()]);
        lines.push(format!(
            "- branch: {}",
            repo.branch.as_deref().unwrap_or("(detached)")
        ));
        if repo.commits.is_empty() {
            lines.push("- (no commits yet)".to_owned());
        }
        lines.extend(repo.commits.iter().map(|commit| format!("- {commit}")));
    }
    lines.extend([
        String::new(),
        "## Limits".to_owned(),
        String::new(),
        format!(
            "- open agent questions on this idea: {} of {}",
            packet.limits.agent_questions_open, packet.limits.questions_per_session
        ),
        format!("- asks remaining: {}", packet.limits.asks_remaining),
        format!(
            "- a question is one line, at most {} characters, ending with \"?\"",
            packet.limits.question_max_length
        ),
    ]);
    format!("{}\n", lines.join("\n"))
}

// ── Prime and serves trees ───────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct ServesTree {
    pub node: NodeRecord,
    pub reference: bool,
    pub children: Vec<ServesTree>,
}

#[must_use]
pub fn is_live_idea(node: Option<&NodeRecord>) -> bool {
    node.is_some_and(|node| node.kind == NodeKind::Idea && node.status.as_str() != "composted")
}

fn by_created(left: &NodeRecord, right: &NodeRecord) -> std::cmp::Ordering {
    left.created_at.cmp(&right.created_at)
}

fn serves_targets(graph: &Graph, id: &str) -> Vec<NodeRecord> {
    graph
        .edges
        .iter()
        .filter(|edge| edge.rel == EdgeRel::Serves && edge.from == id)
        .filter_map(|edge| find_node(graph, &edge.to))
        .filter(|node| is_live_idea(Some(node)))
        .cloned()
        .collect()
}

fn served_by(graph: &Graph, id: &str) -> Vec<NodeRecord> {
    let mut nodes = graph
        .edges
        .iter()
        .filter(|edge| edge.rel == EdgeRel::Serves && edge.to == id)
        .filter_map(|edge| find_node(graph, &edge.from))
        .filter(|node| is_live_idea(Some(node)))
        .cloned()
        .collect::<Vec<_>>();
    nodes.sort_by(by_created);
    nodes
}

fn serves_walk(graph: &Graph, node: NodeRecord, seen: &mut HashSet<String>) -> ServesTree {
    if !seen.insert(node.id.clone()) {
        return ServesTree {
            node,
            reference: true,
            children: Vec::new(),
        };
    }
    let children = served_by(graph, &node.id)
        .into_iter()
        .map(|child| serves_walk(graph, child, seen))
        .collect();
    ServesTree {
        node,
        reference: false,
        children,
    }
}

#[must_use]
pub fn serves_forest(graph: &Graph, roots: &[NodeRecord]) -> Vec<ServesTree> {
    let mut seen = HashSet::new();
    roots
        .iter()
        .cloned()
        .map(|root| serves_walk(graph, root, &mut seen))
        .collect()
}

fn flatten_forest<'a>(trees: &'a [ServesTree], output: &mut Vec<&'a ServesTree>) {
    for tree in trees {
        output.push(tree);
        flatten_forest(&tree.children, output);
    }
}

fn serves_ancestors(graph: &Graph, node: &NodeRecord) -> Vec<NodeRecord> {
    let mut output = Vec::new();
    let mut seen = HashSet::from([node.id.clone()]);
    let mut frontier = vec![node.clone()];
    while !frontier.is_empty() {
        let mut next = Vec::new();
        for item in frontier {
            for target in serves_targets(graph, &item.id) {
                if seen.insert(target.id.clone()) {
                    output.push(target.clone());
                    next.push(target);
                }
            }
        }
        frontier = next;
    }
    output
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PrimeEndorses {
    pub sprout: String,
    pub by: String,
    pub statement: String,
    pub thoughts: String,
    pub changed: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct PrimeNode {
    pub id: String,
    pub slug: String,
    pub status: crate::model::NodeStatus,
    pub statement: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tier: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub endorses: Option<PrimeEndorses>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub r#ref: Option<bool>,
    pub children: Vec<PrimeNode>,
}

#[derive(Clone, Debug, Serialize)]
pub struct PrimeTension {
    pub edge: String,
    pub a: String,
    pub b: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct PrimeOverride {
    pub id: String,
    pub slug: String,
    pub tier: u8,
}

#[derive(Clone, Debug, Serialize)]
pub struct PrimeData {
    pub project: String,
    pub tier: PrimeTier,
    #[serde(rename = "questionsPerSession")]
    pub questions_per_session: u64,
    pub scope: Option<String>,
    pub ideas: usize,
    pub committed: usize,
    pub serves: Vec<PrimeServes>,
    pub tree: Vec<PrimeNode>,
    pub tensions: Vec<PrimeTension>,
    pub overrides: Vec<PrimeOverride>,
}

/// `prime` exposes the regular effective-tier payload.  Context packets add a
/// config echo, but the historical prime JSON does not.
#[derive(Clone, Debug, Serialize)]
pub struct PrimeTier {
    pub tier: u8,
    pub name: String,
    pub source: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct PrimeServes {
    pub id: String,
    pub slug: String,
}

fn to_prime_node(paths: &RootsPaths, graph: &Graph, tree: &ServesTree) -> RootsResult<PrimeNode> {
    let claim = read_claim(paths, graph, &tree.node)?;
    let endorses = claim.endorsed.as_ref().map(|endorsed| {
        let value = endorsed_view(graph, endorsed);
        PrimeEndorses {
            sprout: value.sprout,
            by: value.by,
            statement: value.statement,
            thoughts: claim.prose.text.trim().to_owned(),
            changed: value.changed,
        }
    });
    Ok(PrimeNode {
        id: tree.node.id.clone(),
        slug: tree.node.slug.clone(),
        status: tree.node.status,
        statement: claim.statement,
        tier: tree.node.tier,
        endorses,
        r#ref: tree.reference.then_some(true),
        children: tree
            .children
            .iter()
            .map(|child| to_prime_node(paths, graph, child))
            .collect::<RootsResult<Vec<_>>>()?,
    })
}

fn collect_prime_ids(nodes: &[PrimeNode], into: &mut HashSet<String>) {
    for node in nodes {
        into.insert(node.id.clone());
        collect_prime_ids(&node.children, into);
    }
}

fn prime_anchors(graph: &Graph) -> Vec<NodeRecord> {
    let mut nodes = graph
        .nodes
        .iter()
        .filter(|node| is_live_idea(Some(node)) && serves_targets(graph, &node.id).is_empty())
        .cloned()
        .collect::<Vec<_>>();
    nodes.sort_by(by_created);
    nodes
}

/// Accepted graph data for agent priming.
pub fn build_prime(
    paths: &RootsPaths,
    graph: &Graph,
    config: &RootsConfig,
    scope: Option<&NodeRecord>,
) -> RootsResult<PrimeData> {
    let roots = scope.map_or_else(|| prime_anchors(graph), |node| vec![node.clone()]);
    let forest = serves_forest(graph, &roots);
    let tree = forest
        .iter()
        .map(|tree| to_prime_node(paths, graph, tree))
        .collect::<RootsResult<Vec<_>>>()?;
    let mut within = HashSet::new();
    if scope.is_some() {
        collect_prime_ids(&tree, &mut within);
    }
    let live = graph
        .nodes
        .iter()
        .filter(|node| is_live_idea(Some(node)))
        .collect::<Vec<_>>();
    let tier = effective_tier(config, None);
    Ok(PrimeData {
        project: config.project.clone(),
        tier: PrimeTier {
            tier: tier.tier,
            name: tier.name.to_owned(),
            source: "config".to_owned(),
        },
        questions_per_session: config.limits.questions_per_session,
        scope: scope.map(|node| node.id.clone()),
        ideas: live.len(),
        committed: live
            .iter()
            .filter(|node| node.status.as_str() == "committed")
            .count(),
        serves: scope
            .map(|node| {
                serves_ancestors(graph, node)
                    .into_iter()
                    .map(|node| PrimeServes {
                        id: node.id,
                        slug: node.slug,
                    })
                    .collect()
            })
            .unwrap_or_default(),
        tree,
        tensions: graph
            .edges
            .iter()
            .filter(|edge| edge.rel == EdgeRel::Tension)
            .filter(|edge| {
                is_live_idea(find_node(graph, &edge.from))
                    && is_live_idea(find_node(graph, &edge.to))
            })
            .filter(|edge| {
                scope.is_none() || within.contains(&edge.from) || within.contains(&edge.to)
            })
            .map(|edge| PrimeTension {
                edge: edge.id.clone(),
                a: edge.from.clone(),
                b: edge.to.clone(),
            })
            .collect(),
        overrides: live
            .iter()
            .filter_map(|node| {
                node.tier.map(|tier| PrimeOverride {
                    id: node.id.clone(),
                    slug: node.slug.clone(),
                    tier,
                })
            })
            .collect(),
    })
}

fn truncate(text: &str, max: usize) -> String {
    let chars = text.chars().collect::<Vec<_>>();
    if chars.len() <= max {
        text.to_owned()
    } else {
        format!(
            "{}…",
            chars[..max.saturating_sub(1)].iter().collect::<String>()
        )
    }
}

fn one_line(text: &str) -> String {
    let compact = text.lines().map(str::trim).collect::<Vec<_>>().join(" ");
    let value = truncate(&compact, 160);
    if value.is_empty() {
        "(none)".to_owned()
    } else {
        value
    }
}

fn prime_tree_lines(nodes: &[PrimeNode], depth: usize, output: &mut Vec<String>) {
    let pad = "  ".repeat(depth);
    for node in nodes {
        if node.r#ref.is_some() {
            output.push(format!("{pad}- {} {} (see above)", node.id, node.slug));
            continue;
        }
        let tier = node
            .tier
            .map(|tier| format!(" · tier {tier}"))
            .unwrap_or_default();
        let statement = if node.statement.is_empty() {
            "(empty)"
        } else {
            &node.statement
        };
        output.push(format!(
            "{pad}- {} {} [{}{tier}]: {statement}",
            node.id,
            node.slug,
            node.status.as_str()
        ));
        if let Some(endorses) = &node.endorses {
            let changed = if endorses.changed {
                "; sprout changed since"
            } else {
                ""
            };
            output.push(format!(
                "{pad}  endorses {} ([agent] text a human accepted{changed}); thoughts: {}",
                endorses.sprout,
                one_line(&endorses.thoughts)
            ));
        }
        prime_tree_lines(&node.children, depth + 1, output);
    }
}

fn prime_slug(graph: &Graph, id: &str) -> String {
    find_node(graph, id)
        .map(|node| node.slug.clone())
        .unwrap_or_else(|| id.to_owned())
}

/// Markdown sent to an agent on session start.
#[must_use]
pub fn render_prime_markdown(graph: &Graph, data: &PrimeData) -> String {
    let mut lines = vec![
        "# Roots: project intent".to_owned(),
        String::new(),
        "> Humans write ideas; agents ask questions and propose structure. Agents never write human intent.".to_owned(),
        "> Only `committed` ideas are intent the human stands behind; `planted`/`shaping` are in progress.".to_owned(),
        String::new(),
        format!("Project: {} · {} ideas ({} committed) · agent tier {} ({})", data.project, data.ideas, data.committed, data.tier.tier, data.tier.name),
        String::new(),
        if let Some(scope) = &data.scope { format!("## Scope: {scope}") } else { "## Intent (anchors first; nested ideas serve the one above)".to_owned() },
        String::new(),
    ];
    if !data.serves.is_empty() {
        lines.push(format!(
            "Serves: {}",
            data.serves
                .iter()
                .map(|item| format!("{} {}", item.id, item.slug))
                .collect::<Vec<_>>()
                .join(" → ")
        ));
        lines.push(String::new());
    }
    if data.tree.is_empty() {
        lines.push("(no ideas yet)".to_owned());
    } else {
        prime_tree_lines(&data.tree, 0, &mut lines);
    }
    if !data.tensions.is_empty() {
        lines.extend([String::new(), "## Tensions".to_owned(), String::new()]);
        lines.extend(data.tensions.iter().map(|tension| {
            format!(
                "- {} {} ↔ {} {}",
                tension.a,
                prime_slug(graph, &tension.a),
                tension.b,
                prime_slug(graph, &tension.b)
            )
        }));
    }
    lines.extend([
        String::new(),
        "## Using roots as an agent".to_owned(),
        String::new(),
        "- Human ideas live in `.luuna/roots/human/`. Never create, edit or delete files there, and never".to_owned(),
        "  pass `--as human:*`. Humans write intent; you ask questions and (at tier ≥ 2) propose.".to_owned(),
        "- Identify yourself on every write: `--as agent:<model>` or `ROOTS_AGENT=agent:<model>`.".to_owned(),
        "- Ask questions specific to one idea; never generic ones. Read `luuna-roots context <id>` first:".to_owned(),
        "  dismissed questions there are ones the human refused.".to_owned(),
        format!("- Each idea takes at most {} open agent questions (limits.questionsPerSession).", data.questions_per_session),
        "- Proposals must cite the human's own words. Rejected proposals are permanent: never refile".to_owned(),
        "  them (`luuna-roots context <id>` lists them). Nothing you propose is linked until a human accepts.".to_owned(),
        "- Every command accepts `--json`.".to_owned(),
        String::new(),
        "- `luuna-roots context <id>`: one idea in full: prose, neighbors, prior + dismissed questions".to_owned(),
        "- `luuna-roots show <id> · luuna-roots list · luuna-roots queue · luuna-roots log <id>`: read the graph".to_owned(),
        "- `luuna-roots ask <id> \"<question>\" --as agent:<model>`: queue a question for the human (tier ≥ 1)".to_owned(),
        "- `luuna-roots propose edge <a> <b> <serves|tension|replaces> --reason \"<why>\" --cite <a>:<quote> --cite <b>:<quote> --as agent:<model>`: propose a link (tier ≥ 2); quotes are exact substrings of each idea".to_owned(),
        "- `luuna-roots propose split <id> | merge <a> <b> | compost <id> --reason ... --cite <id>:<quote>`: propose restructuring (tier ≥ 2); a human decides in `luuna-roots tend`".to_owned(),
        "- `luuna-roots sprout \"<one-line claim>\" [--file <md>] --as agent:<model>`: propose a new idea in the agent tree (tier ≥ 2); a human adopts it in their own words or rejects it".to_owned(),
        "- `luuna-roots note <id> --file <path> [--name <n>] --as agent:<model>`: attach an artifact (research, diagram) under .luuna/roots/agent/notes/ (tier ≥ 1)".to_owned(),
        "- `luuna-roots drift [--diff <rev>]`: ideas your changes touch; ask whether each still holds (tier 3; the Stop hook runs it)".to_owned(),
        String::new(),
        format!("Agent tier: {} ({}). Tiers widen what you may suggest, never what you may write:", data.tier.tier, data.tier.name),
    ]);
    for (tier, rule) in [
        (0, "off: nothing; only deterministic questions"),
        (1, "ask: `luuna-roots ask`, `luuna-roots note`"),
        (2, "propose: tier 1 + proposals and sprouts"),
        (
            3,
            "observe: tier 2 + `luuna-roots drift` from harness hooks on repo activity",
        ),
    ] {
        let current = if tier == data.tier.tier {
            "  ← this project"
        } else {
            ""
        };
        lines.push(format!("- {tier} {rule}{current}"));
    }
    if !data.overrides.is_empty() {
        lines.extend([
            String::new(),
            "Per-idea overrides (these win over the project tier):".to_owned(),
        ]);
        lines.extend(data.overrides.iter().map(|override_| {
            format!(
                "- {} {}: tier {}",
                override_.id, override_.slug, override_.tier
            )
        }));
    }
    format!("{}\n", lines.join("\n"))
}

// ── View ─────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Serialize)]
pub struct ViewQuestion {
    pub id: String,
    pub node: String,
    pub text: String,
    pub by: String,
    pub agent: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct ViewBlock {
    pub question: Option<ViewQuestion>,
    pub text: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct ViewIdea {
    pub id: String,
    pub slug: String,
    pub status: crate::model::NodeStatus,
    pub statement: String,
    pub body: String,
    pub blocks: Vec<ViewBlock>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub endorsed: Option<EndorsedView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ViewNode {
    pub id: String,
    pub slug: String,
    pub status: crate::model::NodeStatus,
    pub statement: String,
    pub body: String,
    pub blocks: Vec<ViewBlock>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub endorsed: Option<EndorsedView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    #[serde(rename = "ref")]
    pub reference: bool,
    pub children: Vec<ViewNode>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ViewTension {
    pub edge: String,
    pub a: String,
    pub b: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ViewSprout {
    pub id: String,
    pub slug: String,
    pub statement: String,
    pub body: String,
    pub by: String,
    pub expires_at: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ViewEdge {
    pub from: String,
    pub to: String,
    pub rel: String,
}

#[derive(Clone, Debug)]
pub struct ViewData {
    pub project: String,
    pub from: Option<String>,
    pub tree: Vec<ViewNode>,
    pub unanchored: Vec<ViewIdea>,
    pub tensions: Vec<ViewTension>,
    pub questions: Vec<ViewQuestion>,
    pub sprouts: Option<Vec<ViewSprout>>,
    /// In deterministic insertion order, as JavaScript Object.keys would see it.
    pub ideas: Vec<ViewIdea>,
    pub edges: Vec<ViewEdge>,
}

fn view_question(question: &QuestionRecord) -> ViewQuestion {
    ViewQuestion {
        id: question.id.clone(),
        node: question.node.clone(),
        text: question.text.clone(),
        by: question.by.clone(),
        agent: question.by.starts_with("agent:"),
    }
}

fn read_view_idea(
    paths: &RootsPaths,
    graph: &Graph,
    events: &[crate::model::EventRecord],
    questions: &[QuestionRecord],
    node: &NodeRecord,
) -> RootsResult<ViewIdea> {
    let claim = read_claim(paths, graph, node)?;
    let blame = build_blame(&node.id, &claim.prose.text, events, questions);
    let mut blocks = answer_blocks(&blame)
        .into_iter()
        .map(|block| ViewBlock {
            question: block
                .question
                .as_deref()
                .and_then(|id| questions.iter().find(|question| question.id == id))
                .map(view_question),
            text: block.text,
        })
        .collect::<Vec<_>>();
    let endorsed = claim
        .endorsed
        .as_ref()
        .map(|value| endorsed_view(graph, value));
    if endorsed.is_some() {
        let mut thoughts = Vec::new();
        if !claim.prose.statement.is_empty() {
            thoughts.push(ViewBlock {
                question: None,
                text: claim.prose.statement.clone(),
            });
        }
        thoughts.append(&mut blocks);
        blocks = thoughts;
    }
    Ok(ViewIdea {
        id: node.id.clone(),
        slug: node.slug.clone(),
        status: node.status,
        statement: claim.statement,
        body: claim.prose.body,
        blocks,
        endorsed,
        author: node.endorses.as_ref().map(|_| node.author.clone()),
    })
}

fn view_anchors(graph: &Graph) -> Vec<NodeRecord> {
    let mut anchors = graph
        .nodes
        .iter()
        .filter(|node| {
            is_live_idea(Some(node))
                && matches!(node.status.as_str(), "committed" | "shaping")
                && serves_targets(graph, &node.id).is_empty()
        })
        .cloned()
        .collect::<Vec<_>>();
    anchors.sort_by(by_created);
    anchors
}

fn find_view_idea<'a>(ideas: &'a [ViewIdea], id: &str) -> Option<&'a ViewIdea> {
    ideas.iter().find(|idea| idea.id == id)
}

fn to_view_node(ideas: &[ViewIdea], tree: &ServesTree) -> ViewNode {
    let idea = find_view_idea(ideas, &tree.node.id)
        .cloned()
        .unwrap_or(ViewIdea {
            id: tree.node.id.clone(),
            slug: tree.node.slug.clone(),
            status: tree.node.status,
            statement: String::new(),
            body: String::new(),
            blocks: Vec::new(),
            endorsed: None,
            author: None,
        });
    ViewNode {
        id: idea.id,
        slug: idea.slug,
        status: idea.status,
        statement: idea.statement,
        body: if tree.reference {
            String::new()
        } else {
            idea.body
        },
        blocks: if tree.reference {
            Vec::new()
        } else {
            idea.blocks
        },
        endorsed: idea.endorsed,
        author: idea.author,
        reference: tree.reference,
        children: tree
            .children
            .iter()
            .map(|child| to_view_node(ideas, child))
            .collect(),
    }
}

fn view_tensions(graph: &Graph, ids: &HashSet<String>) -> Vec<ViewTension> {
    let mut tensions = graph
        .edges
        .iter()
        .filter(|edge| edge.rel == EdgeRel::Tension)
        .filter(|edge| {
            is_live_idea(find_node(graph, &edge.from)) && is_live_idea(find_node(graph, &edge.to))
        })
        .filter(|edge| ids.contains(&edge.from) || ids.contains(&edge.to))
        .cloned()
        .collect::<Vec<_>>();
    tensions.sort_by(|left, right| left.created_at.cmp(&right.created_at));
    tensions
        .into_iter()
        .map(|edge| ViewTension {
            edge: edge.id,
            a: edge.from,
            b: edge.to,
        })
        .collect()
}

fn view_questions(rows: &[QuestionRecord], order: &[String]) -> Vec<ViewQuestion> {
    let ranks = order
        .iter()
        .enumerate()
        .map(|(index, id)| (id.clone(), index))
        .collect::<HashMap<_, _>>();
    let mut questions = rows
        .iter()
        .filter(|question| {
            question.status == QuestionStatus::Open && ranks.contains_key(&question.node)
        })
        .cloned()
        .collect::<Vec<_>>();
    questions.sort_by(|left, right| {
        ranks[&left.node]
            .cmp(&ranks[&right.node])
            .then_with(|| left.created_at.cmp(&right.created_at))
            .then_with(|| left.id.cmp(&right.id))
    });
    questions.iter().map(view_question).collect()
}

fn sprouts_for(
    paths: &RootsPaths,
    graph: &Graph,
    now: OffsetDateTime,
) -> RootsResult<Vec<ViewSprout>> {
    let mut sprouts = open_sprouts(graph, now)
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    sprouts.sort_by(by_created);
    sprouts
        .into_iter()
        .map(|sprout| {
            let claim = read_claim(paths, graph, &sprout)?;
            Ok(ViewSprout {
                id: sprout.id,
                slug: sprout.slug,
                statement: claim.prose.statement,
                body: claim.prose.body,
                by: sprout.author,
                expires_at: sprout.expires_at,
            })
        })
        .collect()
}

fn view_edges(graph: &Graph, ids: &HashSet<String>) -> Vec<ViewEdge> {
    graph
        .edges
        .iter()
        .filter(|edge| matches!(edge.rel, EdgeRel::Serves | EdgeRel::Tension))
        .filter(|edge| ids.contains(&edge.from) && ids.contains(&edge.to))
        .map(|edge| ViewEdge {
            from: edge.from.clone(),
            to: edge.to.clone(),
            rel: edge.rel.as_str().to_owned(),
        })
        .collect()
}

/// Build the accepted-graph document.  The caller supplies `from` only after
/// resolving and validating it at the command boundary.
///
/// These source inputs intentionally remain explicit: they mirror the
/// independent persisted records the TypeScript compatibility contract reads.
#[allow(clippy::too_many_arguments)]
pub fn build_view(
    paths: &RootsPaths,
    graph: &Graph,
    config: &RootsConfig,
    questions: &[QuestionRecord],
    from: Option<&NodeRecord>,
    sprouts: bool,
    events: &[crate::model::EventRecord],
    now: OffsetDateTime,
) -> RootsResult<ViewData> {
    let roots = from.map_or_else(|| view_anchors(graph), |node| vec![node.clone()]);
    let forest = serves_forest(graph, &roots);
    let mut flat = Vec::new();
    flatten_forest(&forest, &mut flat);
    let printed = flat
        .into_iter()
        .filter(|tree| !tree.reference)
        .collect::<Vec<_>>();
    let reached = printed
        .iter()
        .map(|tree| tree.node.id.clone())
        .collect::<HashSet<_>>();
    let mut rest = if from.is_some() {
        Vec::new()
    } else {
        graph
            .nodes
            .iter()
            .filter(|node| is_live_idea(Some(node)) && !reached.contains(&node.id))
            .cloned()
            .collect::<Vec<_>>()
    };
    rest.sort_by(by_created);
    let mut ideas = Vec::new();
    for node in printed.iter().map(|tree| &tree.node).chain(rest.iter()) {
        ideas.push(read_view_idea(paths, graph, events, questions, node)?);
    }
    // `inDoc` is captured before adding the outside endpoint of a tension.
    // The TypeScript renderer enriches `ideas` afterwards so tension prose can
    // be shown, but its questions and graph edges remain scoped to the
    // original printed document.
    let initial_document_order = ideas.iter().map(|idea| idea.id.clone()).collect::<Vec<_>>();
    let initial_document = initial_document_order
        .iter()
        .cloned()
        .collect::<HashSet<_>>();
    let tensions = view_tensions(graph, &initial_document);
    for tension in &tensions {
        for id in [&tension.a, &tension.b] {
            if find_view_idea(&ideas, id).is_none() {
                if let Some(node) = find_node(graph, id) {
                    ideas.push(read_view_idea(paths, graph, events, questions, node)?);
                }
            }
        }
    }
    let unanchored = rest
        .iter()
        .filter_map(|node| find_view_idea(&ideas, &node.id).cloned())
        .collect();
    Ok(ViewData {
        project: config.project.clone(),
        from: from.map(|node| node.id.clone()),
        tree: forest
            .iter()
            .map(|tree| to_view_node(&ideas, tree))
            .collect(),
        unanchored,
        tensions,
        questions: view_questions(questions, &initial_document_order),
        sprouts: sprouts
            .then(|| sprouts_for(paths, graph, now))
            .transpose()?,
        ideas,
        edges: view_edges(graph, &initial_document),
    })
}

/// JSON record shape for a view packet, including JavaScript-style `ideas` object.
pub fn view_data_value(data: &ViewData) -> RootsResult<Value> {
    let mut map = Map::new();
    map.insert(
        "project".to_owned(),
        to_value(&data.project)
            .map_err(|error| crate::errors::RootsError::error(error.to_string()))?,
    );
    map.insert(
        "from".to_owned(),
        to_value(&data.from)
            .map_err(|error| crate::errors::RootsError::error(error.to_string()))?,
    );
    map.insert(
        "tree".to_owned(),
        to_value(&data.tree)
            .map_err(|error| crate::errors::RootsError::error(error.to_string()))?,
    );
    map.insert(
        "unanchored".to_owned(),
        to_value(&data.unanchored)
            .map_err(|error| crate::errors::RootsError::error(error.to_string()))?,
    );
    map.insert(
        "tensions".to_owned(),
        to_value(&data.tensions)
            .map_err(|error| crate::errors::RootsError::error(error.to_string()))?,
    );
    map.insert(
        "questions".to_owned(),
        to_value(&data.questions)
            .map_err(|error| crate::errors::RootsError::error(error.to_string()))?,
    );
    map.insert(
        "sprouts".to_owned(),
        to_value(&data.sprouts)
            .map_err(|error| crate::errors::RootsError::error(error.to_string()))?,
    );
    let mut ideas = Map::new();
    for idea in &data.ideas {
        ideas.insert(
            idea.id.clone(),
            to_value(idea).map_err(|error| crate::errors::RootsError::error(error.to_string()))?,
        );
    }
    map.insert("ideas".to_owned(), Value::Object(ideas));
    map.insert(
        "edges".to_owned(),
        to_value(&data.edges)
            .map_err(|error| crate::errors::RootsError::error(error.to_string()))?,
    );
    Ok(Value::Object(map))
}

pub const VIEW_HEADER: &str = "<!-- Generated by `luuna-roots view` from .luuna/roots/. Do not edit by hand: ideas change in `luuna-roots think`, structure in `luuna-roots tend` / `luuna-roots link`. -->";

fn statement_of(statement: &str) -> &str {
    if statement.is_empty() {
        "(empty)"
    } else {
        statement
    }
}

#[must_use]
pub fn question_label(question: &ViewQuestion) -> String {
    let who = if question.agent {
        format!(
            "[agent] {}",
            question.by.strip_prefix("agent:").unwrap_or(&question.by)
        )
    } else {
        question.by.clone()
    };
    format!("**Q:** {} _(`{}` · {who})_", question.text, question.id)
}

fn block_lines(node: &ViewIdea) -> Vec<String> {
    let mut lines = Vec::new();
    for block in &node.blocks {
        if let Some(question) = &block.question {
            lines.extend([format!("> {}", question_label(question)), String::new()]);
        }
        lines.extend([block.text.clone(), String::new()]);
    }
    lines
}

fn node_lines(node: &ViewNode, depth: usize) -> Vec<String> {
    if node.reference {
        return vec![
            format!(
                "↑ Also serves this: **{}** (`{}`), shown above.",
                statement_of(&node.statement),
                node.id
            ),
            String::new(),
        ];
    }
    let hashes = "#".repeat(std::cmp::min(2 + depth, 6));
    let mut lines = vec![
        format!("{hashes} {}", statement_of(&node.statement)),
        String::new(),
        format!("`{}` · {} · {}", node.id, node.slug, node.status.as_str()),
        String::new(),
    ];
    if let Some(endorsed) = &node.endorsed {
        lines.extend(endorsed_markdown(endorsed, node.author.as_deref()));
        lines.push(String::new());
    }
    let idea = ViewIdea {
        id: node.id.clone(),
        slug: node.slug.clone(),
        status: node.status,
        statement: node.statement.clone(),
        body: node.body.clone(),
        blocks: node.blocks.clone(),
        endorsed: node.endorsed.clone(),
        author: node.author.clone(),
    };
    lines.extend(block_lines(&idea));
    for child in &node.children {
        lines.extend(node_lines(child, depth + 1));
    }
    lines
}

fn idea_ref(data: &ViewData, id: &str) -> String {
    let statement = find_view_idea(&data.ideas, id)
        .map(|idea| idea.statement.as_str())
        .unwrap_or_default();
    format!("**{}** (`{id}`)", statement_of(statement))
}

fn sprout_quote(text: &str) -> Vec<String> {
    text.split('\n')
        .map(|line| {
            if line.is_empty() {
                ">".to_owned()
            } else {
                format!("> {line}")
            }
        })
        .collect()
}

/// Deterministic Markdown culmination document.
#[must_use]
pub fn render_view_markdown(data: &ViewData) -> String {
    let title = data
        .from
        .as_ref()
        .map(|id| format!("# {}: intent from `{id}`", data.project))
        .unwrap_or_else(|| format!("# {}: intent", data.project));
    let mut lines = vec![VIEW_HEADER.to_owned(), String::new(), title, String::new()];
    if data.tree.is_empty() {
        lines.extend([
            "(no committed or shaping ideas yet)".to_owned(),
            String::new(),
        ]);
    }
    for node in &data.tree {
        lines.extend(node_lines(node, 0));
    }
    if !data.unanchored.is_empty() {
        lines.extend([
            "## Not yet under an anchor".to_owned(),
            String::new(),
            "Live ideas outside the tree above: planted ideas, and ideas that serve only unshaped ones.".to_owned(),
            String::new(),
        ]);
        lines.extend(data.unanchored.iter().map(|idea| {
            format!(
                "- {} (`{}` · {})",
                statement_of(&idea.statement),
                idea.id,
                idea.status.as_str()
            )
        }));
        lines.push(String::new());
    }
    if !data.tensions.is_empty() {
        lines.extend(["## Tensions".to_owned(), String::new()]);
        lines.extend(data.tensions.iter().map(|tension| {
            format!(
                "- {} ↔ {}",
                idea_ref(data, &tension.a),
                idea_ref(data, &tension.b)
            )
        }));
        lines.push(String::new());
    }
    if !data.questions.is_empty() {
        lines.extend(["## Open questions".to_owned(), String::new()]);
        lines.extend(data.questions.iter().map(|question| {
            let who = if question.agent {
                format!("[agent] {}", question.by)
            } else {
                question.by.clone()
            };
            let slug = find_view_idea(&data.ideas, &question.node)
                .map(|idea| idea.slug.as_str())
                .unwrap_or_default();
            format!("- `{}` {slug}: {} _({who})_", question.node, question.text)
        }));
        lines.push(String::new());
    }
    if let Some(sprouts) = &data.sprouts {
        lines.extend([
            "## Agent proposals (not accepted)".to_owned(),
            String::new(),
            "_Written by agents. Not part of this project's intent unless a human adopts one in their own words._".to_owned(),
            String::new(),
        ]);
        if sprouts.is_empty() {
            lines.extend(["(no open sprouts)".to_owned(), String::new()]);
        }
        for sprout in sprouts {
            lines.extend([
                format!("### [agent] {}", statement_of(&sprout.statement)),
                String::new(),
                format!(
                    "`{}` · {} · proposed by {}",
                    sprout.id, sprout.slug, sprout.by
                ),
                String::new(),
            ]);
            if !sprout.body.is_empty() {
                lines.extend(sprout_quote(&sprout.body));
                lines.push(String::new());
            }
        }
    }
    while lines.last().is_some_and(String::is_empty) {
        let _ = lines.pop();
    }
    format!("{}\n", lines.join("\n"))
}

// ── HTML rendering ───────────────────────────────────────────────────────

#[must_use]
pub fn esc(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

#[derive(Clone, Debug)]
struct LayoutNode {
    id: String,
    label: String,
    x: f64,
    y: f64,
    agent: bool,
}

const BOX_W: f64 = 150.0;
const BOX_H: f64 = 34.0;
const GAP_X: f64 = 24.0;
const ROW_H: f64 = 84.0;
const PAD: f64 = 20.0;

fn view_depths(data: &ViewData) -> HashMap<String, usize> {
    let mut targets = HashMap::<String, Vec<String>>::new();
    for edge in &data.edges {
        if edge.rel == "serves" {
            targets
                .entry(edge.from.clone())
                .or_default()
                .push(edge.to.clone());
        }
    }
    fn depth(
        id: &str,
        targets: &HashMap<String, Vec<String>>,
        values: &mut HashMap<String, usize>,
        visiting: &mut HashSet<String>,
    ) -> usize {
        if let Some(value) = values.get(id) {
            return *value;
        }
        if !visiting.insert(id.to_owned()) {
            return 0;
        }
        let children = targets.get(id).cloned().unwrap_or_default();
        let value = if children.is_empty() {
            0
        } else {
            1 + children
                .iter()
                .map(|target| depth(target, targets, values, visiting))
                .max()
                .unwrap_or(0)
        };
        visiting.remove(id);
        values.insert(id.to_owned(), value);
        value
    }
    let mut values = HashMap::new();
    for idea in &data.ideas {
        let _ = depth(&idea.id, &targets, &mut values, &mut HashSet::new());
    }
    values
}

fn layout_view(data: &ViewData) -> (f64, f64, Vec<LayoutNode>) {
    let depths = view_depths(data);
    let mut rows: Vec<Vec<(String, String, bool)>> = Vec::new();
    for idea in &data.ideas {
        let row = *depths.get(&idea.id).unwrap_or(&0);
        while rows.len() <= row {
            rows.push(Vec::new());
        }
        rows[row].push((idea.id.clone(), idea.slug.clone(), false));
    }
    if let Some(sprouts) = &data.sprouts {
        if !sprouts.is_empty() {
            rows.push(
                sprouts
                    .iter()
                    .map(|sprout| (sprout.id.clone(), format!("[agent] {}", sprout.slug), true))
                    .collect(),
            );
        }
    }
    let widest = std::cmp::max(1, rows.iter().map(Vec::len).max().unwrap_or(0));
    let width = PAD * 2.0 + widest as f64 * BOX_W + (widest.saturating_sub(1)) as f64 * GAP_X;
    let mut nodes = Vec::new();
    for (row_index, row) in rows.iter().enumerate() {
        let row_width = row.len() as f64 * BOX_W + row.len().saturating_sub(1) as f64 * GAP_X;
        let start = (width - row_width) / 2.0;
        for (index, (id, label, agent)) in row.iter().enumerate() {
            nodes.push(LayoutNode {
                id: id.clone(),
                label: label.clone(),
                x: start + index as f64 * (BOX_W + GAP_X),
                y: PAD + row_index as f64 * ROW_H,
                agent: *agent,
            });
        }
    }
    let height = PAD * 2.0 + std::cmp::max(1, rows.len()) as f64 * ROW_H - (ROW_H - BOX_H);
    (width, height, nodes)
}

fn trim_number(value: f64) -> String {
    if value.fract() == 0.0 {
        format!("{value:.0}")
    } else {
        value.to_string()
    }
}

fn html_truncate(value: &str, max: usize) -> String {
    truncate(value, max)
}

fn graph_svg(data: &ViewData) -> String {
    let (width, height, nodes) = layout_view(data);
    let positions = nodes
        .iter()
        .map(|node| (node.id.clone(), node))
        .collect::<HashMap<_, _>>();
    let mut output = vec![
        format!("<svg viewBox=\"0 0 {} {}\" width=\"{}\" height=\"{}\" role=\"img\" aria-label=\"Idea graph\">", trim_number(width), trim_number(height), trim_number(width), trim_number(height)),
        "<defs><marker id=\"arrow\" viewBox=\"0 0 10 10\" refX=\"10\" refY=\"5\" markerWidth=\"7\" markerHeight=\"7\" orient=\"auto-start-reverse\"><path d=\"M0,0 L10,5 L0,10 z\"/></marker></defs>".to_owned(),
    ];
    for edge in &data.edges {
        let (Some(from), Some(to)) = (positions.get(&edge.from), positions.get(&edge.to)) else {
            continue;
        };
        if edge.rel == "serves" {
            output.push(format!("<line class=\"serves\" x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\" marker-end=\"url(#arrow)\"/>", trim_number(from.x + BOX_W / 2.0), trim_number(from.y), trim_number(to.x + BOX_W / 2.0), trim_number(to.y + BOX_H)));
        } else {
            let middle_x = (from.x + to.x) / 2.0 + BOX_W / 2.0;
            let middle_y = from.y.min(to.y) - 18.0;
            output.push(format!(
                "<path class=\"tension\" d=\"M{},{} Q{},{} {},{}\"><title>tension</title></path>",
                trim_number(from.x + BOX_W / 2.0),
                trim_number(from.y),
                trim_number(middle_x),
                trim_number(middle_y),
                trim_number(to.x + BOX_W / 2.0),
                trim_number(to.y)
            ));
        }
    }
    for node in nodes {
        let title = if node.agent {
            data.sprouts
                .as_ref()
                .and_then(|sprouts| sprouts.iter().find(|sprout| sprout.id == node.id))
                .map(|sprout| sprout.statement.as_str())
                .unwrap_or_default()
        } else {
            find_view_idea(&data.ideas, &node.id)
                .map(|idea| idea.statement.as_str())
                .unwrap_or_default()
        };
        // `nodeSvg` in the reference joins its five fragments without
        // internal newlines; the graph renderer then separates whole nodes.
        output.push(format!(
            "<a href=\"#{}\" class=\"node{}\" data-id=\"{}\"><title>{}</title><rect x=\"{}\" y=\"{}\" width=\"150\" height=\"34\" rx=\"6\"/><text x=\"{}\" y=\"{}\">{}</text></a>",
            esc(&node.id),
            if node.agent { " agent" } else { "" },
            esc(&node.id),
            esc(&format!("{}: {}", node.id, statement_of(title))),
            trim_number(node.x),
            trim_number(node.y),
            trim_number(node.x + BOX_W / 2.0),
            trim_number(node.y + BOX_H / 2.0 + 4.0),
            esc(&html_truncate(&node.label, 22))
        ));
    }
    output.push("</svg>".to_owned());
    output.join("\n")
}

fn prose_html(text: &str) -> String {
    if text.is_empty() {
        return String::new();
    }
    let mut paragraphs = Vec::new();
    let mut current = Vec::new();
    for line in text.split('\n') {
        if line.trim().is_empty() {
            if !current.is_empty() {
                paragraphs.push(current.join("\n"));
                current.clear();
            }
        } else {
            current.push(line);
        }
    }
    if !current.is_empty() {
        paragraphs.push(current.join("\n"));
    }
    paragraphs
        .into_iter()
        .map(|paragraph| format!("<p>{}</p>", esc(&paragraph)))
        .collect::<Vec<_>>()
        .join("\n")
}

fn endorsed_html(idea: &ViewIdea) -> String {
    let Some(endorsed) = &idea.endorsed else {
        return String::new();
    };
    let text = if endorsed.body.is_empty() {
        endorsed.statement.clone()
    } else {
        format!("{}\n\n{}", endorsed.statement, endorsed.body)
    };
    [
        format!(
            "<blockquote class=\"endorsed\"><p class=\"by\">{}</p>",
            esc(&endorsed_label(endorsed, idea.author.as_deref()))
        ),
        prose_html(if text.is_empty() {
            "(sprout.md missing)"
        } else {
            &text
        }),
        "</blockquote>".to_owned(),
    ]
    .join("\n")
}

fn blocks_html(idea: &ViewIdea) -> String {
    idea.blocks.iter().map(|block| {
        let Some(question) = &block.question else { return prose_html(&block.text); };
        let who = if question.agent { format!("[agent] {}", question.by) } else { question.by.clone() };
        let class = if question.agent { "q agent" } else { "q" };
        let label = format!("<p class=\"{class}\"><b>Q:</b> {} <span class=\"by\">(<code>{}</code> · {})</span></p>", esc(&question.text), esc(&question.id), esc(&who));
        format!("{label}\n{}", prose_html(&block.text))
    }).collect::<Vec<_>>().join("\n")
}

fn event_html(event: &crate::model::EventRecord) -> String {
    let agent = event.by.starts_with("agent:");
    let who = if agent {
        format!("[agent] {}", event.by)
    } else {
        event.by.clone()
    };
    let when = event
        .at
        .replace('T', " ")
        .strip_suffix('Z')
        .map(|value| format!("{value} UTC"))
        .unwrap_or_else(|| event.at.replace('T', " "));
    format!(
        "<li{}><time>{}</time> <b>{}</b> <span class=\"by\">{}</span></li>",
        if agent { " class=\"agent\"" } else { "" },
        esc(&when),
        esc(&event.event_type),
        esc(&who)
    )
}

fn timeline_html(events: &[crate::model::EventRecord]) -> String {
    if events.is_empty() {
        return String::new();
    }
    let mut lines = vec![format!(
        "<details class=\"timeline\"><summary>Timeline ({})</summary><ol>",
        events.len()
    )];
    lines.extend(events.iter().map(event_html));
    lines.push("</ol></details>".to_owned());
    lines.join("\n")
}

fn meta_html(idea: &ViewIdea) -> String {
    format!(
        "<p class=\"meta\"><code>{}</code> · {} · <span class=\"status s-{}\">{}</span></p>",
        esc(&idea.id),
        esc(&idea.slug),
        esc(idea.status.as_str()),
        esc(idea.status.as_str())
    )
}

fn node_html(
    node: &ViewNode,
    depth: usize,
    timelines: &dyn Fn(&str) -> Vec<crate::model::EventRecord>,
) -> String {
    if node.reference {
        return format!(
            "<p class=\"ref\">↑ Also serves this: <a href=\"#{}\">{}</a> (<code>{}</code>), shown above.</p>",
            esc(&node.id),
            esc(statement_of(&node.statement)),
            esc(&node.id)
        );
    }
    let heading = std::cmp::min(2 + depth, 6);
    let idea = ViewIdea {
        id: node.id.clone(),
        slug: node.slug.clone(),
        status: node.status,
        statement: node.statement.clone(),
        body: node.body.clone(),
        blocks: node.blocks.clone(),
        endorsed: node.endorsed.clone(),
        author: node.author.clone(),
    };
    let mut lines = vec![
        format!(
            "<section class=\"idea\" id=\"{}\" data-id=\"{}\">",
            esc(&node.id),
            esc(&node.id)
        ),
        format!(
            "<h{heading}>{}</h{heading}>",
            esc(statement_of(&node.statement))
        ),
        meta_html(&idea),
        endorsed_html(&idea),
        blocks_html(&idea),
        timeline_html(&timelines(&node.id)),
    ];
    lines.extend(
        node.children
            .iter()
            .map(|child| node_html(child, depth + 1, timelines)),
    );
    lines.push("</section>".to_owned());
    lines.join("\n")
}

fn idea_link(data: &ViewData, id: &str) -> String {
    let statement = find_view_idea(&data.ideas, id)
        .map(|idea| idea.statement.as_str())
        .unwrap_or_default();
    format!(
        "<a href=\"#{}\">{}</a> (<code>{}</code>)",
        esc(id),
        esc(statement_of(statement)),
        esc(id)
    )
}

const CSS: &str = r#"
:root{--bg:#fbfaf7;--fg:#1f2328;--muted:#6b6f76;--line:#d0d4da;--accent:#6b4fbb;--tension:#c2410c;--card:#fff}
@media (prefers-color-scheme:dark){:root{--bg:#16181c;--fg:#e6e6e6;--muted:#9aa0a6;--line:#3a3f47;--accent:#b69cff;--tension:#fb923c;--card:#1f2228}}
*{box-sizing:border-box}
body{margin:0;background:var(--bg);color:var(--fg);font:16px/1.55 system-ui,-apple-system,"Segoe UI",sans-serif}
main{max-width:860px;margin:0 auto;padding:24px 16px 64px}
h1{font-size:1.7rem;margin:0 0 4px}
.graph{overflow-x:auto;border:1px solid var(--line);border-radius:8px;background:var(--card);margin:16px 0 32px}
svg{display:block;margin:0 auto}
.endorsed{margin:8px 0;border-left:3px solid var(--line);padding-left:12px;color:var(--muted)}
svg .node rect{fill:var(--card);stroke:var(--accent);stroke-width:1.5}
svg .node text{fill:var(--fg);font-size:12px;text-anchor:middle}
svg .node.agent{opacity:.5}
svg .node.agent rect{stroke:var(--muted);stroke-dasharray:4 3}
svg .node.hot rect{stroke-width:3}
svg line.serves{stroke:var(--muted);stroke-width:1.2}
svg marker path{fill:var(--muted)}
svg path.tension{fill:none;stroke:var(--tension);stroke-width:1.5;stroke-dasharray:5 4}
section.idea{border-left:2px solid var(--line);padding-left:14px;margin:18px 0}
p{white-space:pre-wrap}
.meta{color:var(--muted);font-size:.85rem;margin-top:-6px}
.status{font-weight:600}
.ref{color:var(--muted);font-style:italic}
.q{color:var(--muted);border-left:3px solid var(--line);padding-left:10px;margin-bottom:4px}
code{font-size:.85em}
a{color:var(--accent)}
.timeline{font-size:.85rem;color:var(--muted);margin:6px 0}
.timeline ol{margin:4px 0;padding-left:20px}
.agent{opacity:.6}
.agent-zone{border:1px dashed var(--muted);border-radius:8px;padding:4px 16px;margin-top:32px;opacity:.75}
.agent-zone .agent{opacity:1}
.badge{font-size:.75em;color:var(--muted);border:1px solid var(--muted);border-radius:4px;padding:0 4px}
.by{color:var(--muted)}
.hot{outline:2px solid var(--accent);outline-offset:4px}
"#;

const JS: &str = r#"
(function(){
  var all=[].slice.call(document.querySelectorAll('[data-id]'));
  function set(id,on){all.forEach(function(el){if(el.getAttribute('data-id')===id){el.classList.toggle('hot',on)}})}
  all.forEach(function(el){
    el.addEventListener('mouseenter',function(){set(el.getAttribute('data-id'),true)});
    el.addEventListener('mouseleave',function(){set(el.getAttribute('data-id'),false)});
  });
})();
"#;

const CSP: &str =
    "default-src 'none'; style-src 'unsafe-inline'; script-src 'unsafe-inline'; img-src data:";

fn sprout_html(sprout: &ViewSprout) -> String {
    [
        format!(
            "<section class=\"sprout agent\" id=\"{}\" data-id=\"{}\">",
            esc(&sprout.id),
            esc(&sprout.id)
        ),
        format!(
            "<h3><span class=\"badge\">[agent]</span> {}</h3>",
            esc(statement_of(&sprout.statement))
        ),
        format!(
            "<p class=\"meta\"><code>{}</code> · {} · proposed by {}</p>",
            esc(&sprout.id),
            esc(&sprout.slug),
            esc(&sprout.by)
        ),
        prose_html(&sprout.body),
        "</section>".to_owned(),
    ]
    .join("\n")
}

fn sprouts_html(sprouts: Option<&Vec<ViewSprout>>) -> String {
    let Some(sprouts) = sprouts else {
        return String::new();
    };
    let mut lines = vec![
        "<div class=\"agent-zone\">".to_owned(),
        "<h2>Agent proposals (not accepted)</h2>".to_owned(),
        "<p><em>Written by agents. Not part of this project's intent unless a human adopts one in their own words.</em></p>".to_owned(),
        if sprouts.is_empty() {
            "<p>(no open sprouts)</p>".to_owned()
        } else {
            String::new()
        },
    ];
    lines.extend(sprouts.iter().map(sprout_html));
    lines.push("</div>".to_owned());
    lines.join("\n")
}

/// Self-contained, escaped static HTML presentation of a view.
#[must_use]
pub fn render_view_html(
    data: &ViewData,
    timelines: &dyn Fn(&str) -> Vec<crate::model::EventRecord>,
) -> String {
    let title = data
        .from
        .as_ref()
        .map(|id| format!("{}: intent from {id}", data.project))
        .unwrap_or_else(|| format!("{}: intent", data.project));
    let mut lines = vec![
        "<!doctype html>".to_owned(),
        "<html lang=\"en\">".to_owned(),
        "<head>".to_owned(),
        "<meta charset=\"utf-8\">".to_owned(),
        format!("<meta http-equiv=\"Content-Security-Policy\" content=\"{CSP}\">"),
        "<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">".to_owned(),
        format!("<title>{}</title>", esc(&title)),
        format!("<style>{CSS}</style>"),
        "</head>".to_owned(),
        "<body><main>".to_owned(),
        format!("<h1>{}</h1>", esc(&title)),
        "<p class=\"meta\">Generated by <code>luuna-roots view --html</code>. Human prose and human-accepted structure; agent content is marked [agent].</p>".to_owned(),
        format!("<div class=\"graph\">{}</div>", graph_svg(data)),
    ];
    if data.tree.is_empty() {
        lines.push("<p>(no committed or shaping ideas yet)</p>".to_owned());
    }
    lines.extend(data.tree.iter().map(|node| node_html(node, 0, timelines)));
    if !data.unanchored.is_empty() {
        lines.push("<h2>Not yet under an anchor</h2>".to_owned());
        lines.push("<ul>".to_owned());
        for idea in &data.unanchored {
            lines.push(format!(
                "<li id=\"{}\" data-id=\"{}\">{} (<code>{}</code> · {}){}",
                esc(&idea.id),
                esc(&idea.id),
                esc(statement_of(&idea.statement)),
                esc(&idea.id),
                esc(idea.status.as_str()),
                timeline_html(&timelines(&idea.id))
            ));
            // The timeline belongs inside the li, exactly as the reference renderer does.
            let last = lines.last_mut().expect("pushed above");
            last.push_str("</li>");
        }
        lines.push("</ul>".to_owned());
    }
    if !data.tensions.is_empty() {
        lines.extend(["<h2>Tensions</h2>".to_owned(), "<ul>".to_owned()]);
        lines.extend(data.tensions.iter().map(|tension| {
            format!(
                "<li>{} ↔ {}</li>",
                idea_link(data, &tension.a),
                idea_link(data, &tension.b)
            )
        }));
        lines.push("</ul>".to_owned());
    }
    if !data.questions.is_empty() {
        lines.extend(["<h2>Open questions</h2>".to_owned(), "<ul>".to_owned()]);
        for question in &data.questions {
            let who = if question.agent {
                format!("[agent] {}", question.by)
            } else {
                question.by.clone()
            };
            lines.push(format!(
                "<li{}><a href=\"#{}\"><code>{}</code></a> {} <span class=\"by\">({})</span></li>",
                if question.agent {
                    " class=\"agent\""
                } else {
                    ""
                },
                esc(&question.node),
                esc(&question.node),
                esc(&question.text),
                esc(&who)
            ));
        }
        lines.push("</ul>".to_owned());
    }
    lines.push(sprouts_html(data.sprouts.as_ref()));
    lines.extend([
        "</main>".to_owned(),
        format!("<script>{JS}</script>"),
        "</body></html>".to_owned(),
    ]);
    format!(
        "{}\n",
        lines
            .into_iter()
            .filter(|line| !line.is_empty())
            .collect::<Vec<_>>()
            .join("\n")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::default_config;
    use crate::model::{Graph, IdeaStatus};
    use crate::paths::roots_paths;
    use crate::prose::canonical_node_dir;
    use crate::serde_json::{Value, to_value};
    use std::fs;
    use std::path::PathBuf;

    fn sandbox(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "roots-presentation-{name}-{}",
            crate::ids::random_hex(8)
        ));
        fs::create_dir_all(&root).expect("create sandbox");
        root
    }

    fn open_question(id: &str, node: &str) -> QuestionRecord {
        QuestionRecord {
            id: id.to_owned(),
            node: node.to_owned(),
            text: format!("Question for {node}"),
            by: "agent:fixture".to_owned(),
            status: QuestionStatus::Open,
            created_at: "2026-01-01T00:00:00Z".to_owned(),
            r#ref: None,
            answered_by: None,
            answered_at: None,
            dismissed_by: None,
            dismissed_at: None,
            delegated_by: None,
            delegated_at: None,
            findings: None,
            snoozed_until: None,
            session: None,
        }
    }

    fn edge(id: &str, from: &str, to: &str, rel: EdgeRel) -> crate::model::EdgeRecord {
        crate::model::EdgeRecord {
            record_type: "edge".to_owned(),
            id: id.to_owned(),
            from: from.to_owned(),
            to: to.to_owned(),
            rel,
            by: "human:fixture".to_owned(),
            proposed_by: None,
            proposal: None,
            created_at: "2026-01-01T00:00:00Z".to_owned(),
        }
    }

    fn write_idea(paths: &RootsPaths, node: &NodeRecord, statement: &str) {
        let file = canonical_node_dir(paths, node).join("idea.md");
        fs::create_dir_all(file.parent().expect("idea parent")).expect("create idea directory");
        fs::write(file, format!("{statement}\n")).expect("write idea");
    }

    #[test]
    fn context_rejections_and_prime_tier_keep_the_legacy_json_shape() {
        let mention = ContextRejection {
            id: "p-a1b2".to_owned(),
            kind: ProposalKind::Edge,
            from: Some("r-a1b2".to_owned()),
            to: Some("r-b2c3".to_owned()),
            rel: None,
            reason: Some("mentioned".to_owned()),
            decision_reason: None,
            by: "human:casey".to_owned(),
        };
        let mention = to_value(mention).expect("serialize mention proposal");
        assert_eq!(mention["rel"], Value::Null);
        assert_eq!(mention["reason"], "mentioned");

        let compost = ContextRejection {
            id: "p-b2c3".to_owned(),
            kind: ProposalKind::Compost,
            from: Some("r-a1b2".to_owned()),
            to: None,
            rel: None,
            reason: None,
            decision_reason: Some("not yet".to_owned()),
            by: "human:casey".to_owned(),
        };
        let compost = to_value(compost).expect("serialize compost proposal");
        assert!(compost.get("rel").is_none());
        assert_eq!(compost["decisionReason"], "not yet");

        let tier = to_value(PrimeTier {
            tier: 2,
            name: "propose".to_owned(),
            source: "config".to_owned(),
        })
        .expect("serialize prime tier");
        assert_eq!(tier["tier"], 2);
        assert_eq!(tier["name"], "propose");
        assert!(tier.get("config").is_none());
    }

    #[test]
    fn endorsement_quote_marks_the_agent_and_preserves_changed_state() {
        let endorsed = EndorsedView {
            sprout: "s-a1b2".to_owned(),
            slug: "offline-mode".to_owned(),
            by: "agent:opus".to_owned(),
            statement: "Keep data local".to_owned(),
            body: "Then sync deliberately.".to_owned(),
            changed: true,
        };
        assert_eq!(
            endorsed_markdown(&endorsed, Some("human:casey")).join("\n"),
            "> _[agent] opus · s-a1b2 · endorsed by human:casey · CHANGED since endorsement_\n>\n> Keep data local\n>\n> Then sync deliberately."
        );
    }

    #[test]
    fn view_markdown_html_and_json_keep_agent_content_separate_and_escaped() {
        let data = ViewData {
            project: "Spec <&>".to_owned(),
            from: None,
            tree: Vec::new(),
            unanchored: Vec::new(),
            tensions: Vec::new(),
            questions: Vec::new(),
            sprouts: Some(vec![ViewSprout {
                id: "s-a1b2".to_owned(),
                slug: "draft".to_owned(),
                statement: "<draft>".to_owned(),
                body: "Agent <body>".to_owned(),
                by: "agent:opus".to_owned(),
                expires_at: Some("2026-10-30T00:00:00Z".to_owned()),
            }]),
            ideas: Vec::new(),
            edges: Vec::new(),
        };

        let markdown = render_view_markdown(&data);
        assert!(markdown.starts_with(VIEW_HEADER));
        assert!(markdown.contains("## Agent proposals (not accepted)"));
        assert!(markdown.contains("### [agent] <draft>"));
        assert!(markdown.contains("> Agent <body>"));

        let html = render_view_html(&data, &|_| Vec::new());
        assert!(html.contains("<h1>Spec &lt;&amp;&gt;: intent</h1>"));
        assert!(html.contains("</p>\n\n<section class=\"sprout agent\""));
        assert!(html.contains("<h3><span class=\"badge\">[agent]</span> &lt;draft&gt;</h3>"));
        assert!(html.contains("<p>Agent &lt;body&gt;</p>"));

        let value = view_data_value(&data).expect("view json");
        assert_eq!(value["sprouts"][0]["expiresAt"], "2026-10-30T00:00:00Z");
        assert!(
            value["ideas"]
                .as_object()
                .is_some_and(|ideas| ideas.is_empty())
        );
    }

    #[test]
    fn scoped_view_adds_tension_prose_but_not_that_endpoint_to_questions_or_graph_edges() {
        let root = sandbox("scoped-tension");
        let paths = roots_paths(&root);
        let scope = NodeRecord::new(
            "r-a1b2".to_owned(),
            NodeKind::Idea,
            "scope".to_owned(),
            crate::model::NodeStatus::Idea(IdeaStatus::Planted),
            "human:fixture".to_owned(),
            "2026-01-01T00:00:00Z".to_owned(),
        );
        let child = NodeRecord::new(
            "r-b2c3".to_owned(),
            NodeKind::Idea,
            "child".to_owned(),
            crate::model::NodeStatus::Idea(IdeaStatus::Planted),
            "human:fixture".to_owned(),
            "2026-01-01T00:00:01Z".to_owned(),
        );
        let endpoint = NodeRecord::new(
            "r-c3d4".to_owned(),
            NodeKind::Idea,
            "outside".to_owned(),
            crate::model::NodeStatus::Idea(IdeaStatus::Planted),
            "human:fixture".to_owned(),
            "2026-01-01T00:00:02Z".to_owned(),
        );
        for (node, statement) in [
            (&scope, "Scoped claim"),
            (&child, "Child claim"),
            (&endpoint, "Outside tension claim"),
        ] {
            write_idea(&paths, node, statement);
        }
        let graph = Graph {
            nodes: vec![scope.clone(), child.clone(), endpoint.clone()],
            edges: vec![
                edge("e-serves", &child.id, &scope.id, EdgeRel::Serves),
                edge("e-tension", &scope.id, &endpoint.id, EdgeRel::Tension),
            ],
        };
        let data = build_view(
            &paths,
            &graph,
            &default_config("test"),
            &[
                open_question("q-a1b2", &scope.id),
                open_question("q-c3d4", &endpoint.id),
            ],
            Some(&scope),
            false,
            &[],
            OffsetDateTime::now_utc(),
        )
        .expect("build scoped view");

        assert_eq!(data.ideas.len(), 3, "tension endpoint still has prose");
        assert_eq!(data.tensions.len(), 1);
        assert_eq!(data.questions.len(), 1);
        assert_eq!(data.questions[0].id, "q-a1b2");
        assert_eq!(data.edges.len(), 1);
        assert_eq!(data.edges[0].rel, "serves");
        assert_eq!(data.edges[0].from, child.id);
        assert_eq!(data.edges[0].to, scope.id);
        fs::remove_dir_all(root).expect("clean sandbox");
    }
}
