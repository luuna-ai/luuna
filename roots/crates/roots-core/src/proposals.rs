//! Agent proposal policy, expiration, duplicate detection, and JSONL storage.

use crate::claims::read_claim;
use crate::config::load_config;
use crate::errors::{RootsError, RootsResult};
use crate::events::{EventFields, append_event, make_event};
use crate::graph::read_graph;
use crate::graph_rules::{LINK_RELS, check_edge, is_link_rel};
use crate::ids::{IdPrefix, generate_id, hex_set};
use crate::model::{
    Citation, EdgeRel, Graph, IdeaStatus, NodeKind, ProposalKind, ProposalRecord, ProposalStatus,
};
use crate::paths::RootsPaths;
use crate::resolve::{ResolveOptions, resolve_node};
use crate::store::{read_table, update_table};
use crate::tier::require_tier;
use crate::time::iso_at;
use serde_json::{Map, Value};
use time::{Duration, OffsetDateTime, format_description::well_known::Rfc3339};

pub const REASON_MAX_LENGTH: usize = 280;
pub const MENTION_ACTOR: &str = "roots:mention";

#[derive(Clone, Debug)]
pub struct ProposalDraft {
    pub kind: ProposalKind,
    pub from: Option<String>,
    pub to: Option<String>,
    pub rel: Option<EdgeRel>,
    pub reason: Option<String>,
    pub cites: Vec<Citation>,
    pub by: String,
}

#[derive(Clone, Debug)]
pub struct ProposeInput<'a> {
    pub kind: ProposalKind,
    pub nodes: Vec<&'a str>,
    pub rel: Option<&'a str>,
    pub reason: Option<&'a str>,
    pub cites: Vec<&'a str>,
    pub mention: bool,
    pub by: &'a str,
    pub now: Option<OffsetDateTime>,
}

#[must_use]
pub fn proposal_kind(raw: &str) -> Option<ProposalKind> {
    match raw.to_lowercase().as_str() {
        "edge" => Some(ProposalKind::Edge),
        "split" => Some(ProposalKind::Split),
        "merge" => Some(ProposalKind::Merge),
        "compost" => Some(ProposalKind::Compost),
        _ => None,
    }
}

#[must_use]
pub const fn kind_name(kind: ProposalKind) -> &'static str {
    match kind {
        ProposalKind::Edge => "edge",
        ProposalKind::Split => "split",
        ProposalKind::Merge => "merge",
        ProposalKind::Compost => "compost",
    }
}

#[must_use]
pub const fn expected_nodes(kind: ProposalKind) -> usize {
    match kind {
        ProposalKind::Edge | ProposalKind::Merge => 2,
        ProposalKind::Split | ProposalKind::Compost => 1,
    }
}

pub fn clean_reason(raw: Option<&str>) -> RootsResult<String> {
    let value = raw
        .unwrap_or("")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if value.is_empty() {
        return Err(RootsError::usage(
            "--reason is required: say why, in one line",
        ));
    }
    let length = value.encode_utf16().count();
    if length > REASON_MAX_LENGTH {
        return Err(RootsError::validation(format!(
            "--reason is {length} characters; keep it under {REASON_MAX_LENGTH}"
        )));
    }
    Ok(value)
}

pub fn parse_cite(raw: &str, graph: &Graph) -> RootsResult<Citation> {
    let Some(colon) = raw.find(':') else {
        return Err(RootsError::usage(format!(
            "--cite must be <id>:<quote>, got \"{raw}\""
        )));
    };
    if colon == 0 {
        return Err(RootsError::usage(format!(
            "--cite must be <id>:<quote>, got \"{raw}\""
        )));
    }
    let node = resolve_node(&graph.nodes, raw[..colon].trim(), ResolveOptions::default())?;
    let quote = raw[colon + 1..].trim();
    let quote = if quote.len() >= 2 && quote.starts_with('"') && quote.ends_with('"') {
        &quote[1..quote.len() - 1]
    } else {
        quote
    };
    if quote.is_empty() {
        return Err(RootsError::usage(format!(
            "--cite {}: the quote is empty",
            node.id
        )));
    }
    Ok(Citation {
        node: node.id.clone(),
        quote: quote.to_owned(),
    })
}

pub fn parse_link_rel(raw: &str) -> RootsResult<EdgeRel> {
    let rel = raw.trim().to_lowercase();
    if rel == "derives" {
        return Err(RootsError::usage(
            "`derives` edges come only from `luuna-roots adopt`",
        ));
    }
    if !is_link_rel(&rel) {
        return Err(RootsError::usage(format!(
            "unknown relation \"{raw}\"; use {}",
            LINK_RELS.join(", ")
        )));
    }
    Ok(match rel.as_str() {
        "serves" => EdgeRel::Serves,
        "tension" => EdgeRel::Tension,
        "replaces" => EdgeRel::Replaces,
        _ => unreachable!(),
    })
}

pub fn read_proposals(paths: &RootsPaths) -> RootsResult<Vec<ProposalRecord>> {
    read_table(&paths.proposals)
}

fn parsed_at(value: &str) -> Option<OffsetDateTime> {
    OffsetDateTime::parse(value, &Rfc3339).ok()
}

#[must_use]
pub fn is_expired(proposal: &ProposalRecord, now: OffsetDateTime) -> bool {
    proposal.status == ProposalStatus::Pending
        && proposal
            .expires_at
            .as_deref()
            .and_then(parsed_at)
            .is_some_and(|expires| expires <= now)
}

fn sprout_still_open(node: &crate::model::NodeRecord, now: OffsetDateTime) -> bool {
    matches!(
        node.status,
        crate::model::NodeStatus::Sprout(crate::model::SproutStatus::Open)
    ) && node
        .expires_at
        .as_deref()
        .and_then(parsed_at)
        .is_none_or(|expires| expires > now)
}

#[must_use]
pub fn moot_reason(
    proposal: &ProposalRecord,
    graph: &Graph,
    now: OffsetDateTime,
) -> Option<String> {
    for id in [proposal.from.as_deref(), proposal.to.as_deref()]
        .into_iter()
        .flatten()
    {
        let Some(node) = graph.nodes.iter().find(|node| node.id == id) else {
            return Some(format!("{id} is gone"));
        };
        if node.kind == NodeKind::Idea
            && matches!(
                node.status,
                crate::model::NodeStatus::Idea(IdeaStatus::Composted)
            )
        {
            return Some(format!("{id} is composted"));
        }
        if node.kind == NodeKind::Sprout && !sprout_still_open(node, now) {
            return Some(format!("sprout {id} is no longer open"));
        }
    }
    None
}

#[must_use]
pub fn pending_proposals<'a>(
    rows: &'a [ProposalRecord],
    now: OffsetDateTime,
    graph: Option<&Graph>,
) -> Vec<&'a ProposalRecord> {
    rows.iter()
        .filter(|proposal| {
            proposal.status == ProposalStatus::Pending
                && !is_expired(proposal, now)
                && graph.is_none_or(|graph| moot_reason(proposal, graph, now).is_none())
        })
        .collect()
}

#[must_use]
pub fn expires_at(now: OffsetDateTime, days: u64) -> String {
    iso_at(now + Duration::days(days as i64))
}

fn same_pair(
    a_from: Option<&str>,
    a_to: Option<&str>,
    b_from: Option<&str>,
    b_to: Option<&str>,
) -> bool {
    (a_from == b_from && a_to == b_to) || (a_from == b_to && a_to == b_from)
}

fn same_target(proposal: &ProposalRecord, draft: &ProposalDraft) -> bool {
    let directed = matches!(
        draft.rel,
        Some(EdgeRel::Serves | EdgeRel::Replaces | EdgeRel::Derives)
    );
    if directed {
        proposal.from == draft.from && proposal.to == draft.to
    } else {
        same_pair(
            proposal.from.as_deref(),
            proposal.to.as_deref(),
            draft.from.as_deref(),
            draft.to.as_deref(),
        )
    }
}

#[must_use]
pub fn find_rejection<'a>(
    rows: &'a [ProposalRecord],
    draft: &ProposalDraft,
) -> Option<&'a ProposalRecord> {
    rows.iter().find(|proposal| {
        if proposal.status != ProposalStatus::Rejected || proposal.kind != draft.kind {
            return false;
        }
        if draft.kind == ProposalKind::Edge && draft.rel.is_none() {
            return same_pair(
                proposal.from.as_deref(),
                proposal.to.as_deref(),
                draft.from.as_deref(),
                draft.to.as_deref(),
            );
        }
        proposal.rel == draft.rel && same_target(proposal, draft)
    })
}

fn is_mention(proposal_by: &str) -> bool {
    proposal_by == MENTION_ACTOR
}

fn find_duplicate<'a>(
    rows: &'a [ProposalRecord],
    draft: &ProposalDraft,
    now: OffsetDateTime,
) -> Option<&'a ProposalRecord> {
    pending_proposals(rows, now, None)
        .into_iter()
        .find(|proposal| {
            proposal.kind == draft.kind
                && same_pair(
                    proposal.from.as_deref(),
                    proposal.to.as_deref(),
                    draft.from.as_deref(),
                    draft.to.as_deref(),
                )
                && is_mention(&proposal.by) == is_mention(&draft.by)
        })
}

fn edge_exists(graph: &Graph, draft: &ProposalDraft) -> bool {
    draft.kind == ProposalKind::Edge
        && graph.edges.iter().any(|edge| {
            same_pair(
                Some(&edge.from),
                Some(&edge.to),
                draft.from.as_deref(),
                draft.to.as_deref(),
            ) && draft.rel.is_none_or(|rel| edge.rel == rel)
        })
}

fn check_citations(paths: &RootsPaths, graph: &Graph, draft: &ProposalDraft) -> RootsResult<()> {
    let min = if matches!(draft.kind, ProposalKind::Edge | ProposalKind::Merge) {
        2
    } else {
        1
    };
    if draft.cites.len() < min {
        return Err(RootsError::validation(format!(
            "{} proposals need at least {min} citation(s)",
            kind_name(draft.kind)
        )));
    }
    let ends: Vec<_> = if matches!(draft.kind, ProposalKind::Edge | ProposalKind::Merge) {
        vec![draft.from.as_deref(), draft.to.as_deref()]
    } else {
        vec![draft.from.as_deref()]
    };
    for endpoint in ends.into_iter().flatten() {
        if !draft.cites.iter().any(|cite| cite.node == endpoint) {
            return Err(RootsError::validation(format!(
                "cite at least one quote from {endpoint}"
            )));
        }
    }
    for cite in &draft.cites {
        let Some(node) = graph.nodes.iter().find(|node| node.id == cite.node) else {
            return Err(RootsError::validation(format!(
                "cited node {} does not exist",
                cite.node
            )));
        };
        if cite.quote.trim().is_empty() {
            return Err(RootsError::validation(format!(
                "empty cite quote for {}",
                cite.node
            )));
        }
        if !read_claim(paths, graph, node)?.text.contains(&cite.quote) {
            return Err(RootsError::validation(format!(
                "cite quote not found in {}",
                cite.node
            )));
        }
    }
    Ok(())
}

fn expire_rows(
    rows: &mut [ProposalRecord],
    graph: &Graph,
    now: OffsetDateTime,
) -> Vec<(ProposalRecord, String)> {
    let mut expired = Vec::new();
    for proposal in rows {
        if proposal.status != ProposalStatus::Pending {
            continue;
        }
        let reason = if is_expired(proposal, now) {
            Some("ttl".to_owned())
        } else {
            moot_reason(proposal, graph, now).map(|reason| format!("moot: {reason}"))
        };
        if let Some(reason) = reason {
            proposal.status = ProposalStatus::Expired;
            expired.push((proposal.clone(), reason));
        }
    }
    expired
}

fn filing_event(
    paths: &RootsPaths,
    proposal: &ProposalRecord,
    event_type: &str,
    reason: Option<&str>,
) -> RootsResult<()> {
    let mut extra = Map::new();
    extra.insert("proposal".into(), Value::String(proposal.id.clone()));
    extra.insert(
        "kind".into(),
        Value::String(kind_name(proposal.kind).to_owned()),
    );
    if let Some(rel) = proposal.rel {
        extra.insert("rel".into(), Value::String(rel.as_str().to_owned()));
    }
    if let Some(reason) = reason {
        extra.insert("reason".into(), Value::String(reason.to_owned()));
    }
    append_event(
        paths,
        &make_event(
            event_type,
            if event_type == "expire" {
                "roots"
            } else {
                proposal.by.as_str()
            },
            EventFields {
                node: proposal.from.clone(),
                refs: proposal.to.clone().map(|id| vec![id]),
                extra,
                ..Default::default()
            },
        ),
    )
}

/// Expire every overdue or moot pending proposal and record one audit event
/// per row.  `tend` calls this before it constructs its review cards; keeping
/// it public avoids making interactive code reimplement proposal-table
/// mutation or silently omit the expiry events.
pub fn expire_proposals(
    paths: &RootsPaths,
    now: Option<OffsetDateTime>,
) -> RootsResult<Vec<ProposalRecord>> {
    let now = now.unwrap_or_else(OffsetDateTime::now_utc);
    let graph = read_graph(paths)?;
    let expired = update_table::<ProposalRecord, _, _>(&paths.proposals, |mut rows| {
        let expired = expire_rows(&mut rows, &graph, now);
        let changed = !expired.is_empty();
        Ok((rows, changed, expired))
    })?;
    for (proposal, reason) in &expired {
        filing_event(paths, proposal, "expire", Some(reason))?;
    }
    Ok(expired.into_iter().map(|(proposal, _)| proposal).collect())
}

pub fn file_proposals(
    paths: &RootsPaths,
    drafts: &[ProposalDraft],
    cap: u64,
    ttl_days: u64,
    now: Option<OffsetDateTime>,
) -> RootsResult<Vec<ProposalRecord>> {
    let now = now.unwrap_or_else(OffsetDateTime::now_utc);
    let graph = read_graph(paths)?;
    let (filed, expired) = update_table::<ProposalRecord, _, _>(&paths.proposals, |mut rows| {
        let expired = expire_rows(&mut rows, &graph, now);
        let pending = pending_proposals(&rows, now, None).len() as u64;
        let mut filed = Vec::new();
        for (pending, draft) in (pending..).zip(drafts) {
            check_citations(paths, &graph, draft)?;
            if let Some(rejected) = find_rejection(&rows, draft) {
                let mut detail = Map::new();
                detail.insert("reason".into(), Value::String("rejected".into()));
                return Err(RootsError::conflict(format!(
                    "a human already rejected this ({})",
                    rejected.id
                ))
                .with_detail(detail));
            }
            if let Some(duplicate) = find_duplicate(&rows, draft, now) {
                let mut detail = Map::new();
                detail.insert("reason".into(), Value::String("duplicate".into()));
                return Err(
                    RootsError::conflict(format!("already proposed as {}", duplicate.id))
                        .with_detail(detail),
                );
            }
            if edge_exists(&graph, draft) {
                let mut detail = Map::new();
                detail.insert("reason".into(), Value::String("exists".into()));
                return Err(
                    RootsError::conflict("these nodes are already linked").with_detail(detail)
                );
            }
            if pending >= cap {
                let mut detail = Map::new();
                detail.insert("reason".into(), Value::String("cap".into()));
                return Err(RootsError::conflict(format!(
                    "pending proposals are at the limit ({cap}); run `luuna-roots tend`"
                ))
                .with_detail(detail));
            }
            let taken = hex_set(rows.iter().map(|row| row.id.as_str()));
            let record = ProposalRecord {
                id: generate_id(IdPrefix::Proposal, &taken),
                kind: draft.kind,
                from: draft.from.clone(),
                to: draft.to.clone(),
                rel: if draft.kind == ProposalKind::Edge {
                    draft.rel
                } else {
                    None
                },
                reason: draft.reason.clone(),
                cites: draft.cites.clone(),
                by: draft.by.clone(),
                status: ProposalStatus::Pending,
                created_at: iso_at(now),
                expires_at: Some(expires_at(now, ttl_days)),
                decided_by: None,
                decided_at: None,
                decision_reason: None,
            };
            rows.push(record.clone());
            filed.push(record);
        }
        Ok((
            rows,
            !expired.is_empty() || !filed.is_empty(),
            (filed, expired),
        ))
    })?;
    for (proposal, reason) in expired {
        filing_event(paths, &proposal, "expire", Some(&reason))?;
    }
    for proposal in &filed {
        filing_event(paths, proposal, "propose", None)?;
    }
    Ok(filed)
}

fn live_idea(node: &crate::model::NodeRecord) -> RootsResult<()> {
    if node.kind != NodeKind::Idea {
        return Err(RootsError::validation(format!(
            "{} is a sprout; proposals are about human ideas",
            node.id
        )));
    }
    if matches!(
        node.status,
        crate::model::NodeStatus::Idea(IdeaStatus::Composted)
    ) {
        return Err(RootsError::validation(format!("{} is composted", node.id)));
    }
    Ok(())
}

pub fn propose_as_agent(
    paths: &RootsPaths,
    input: ProposeInput<'_>,
) -> RootsResult<ProposalRecord> {
    if !input.by.starts_with("agent:") {
        return Err(RootsError::guard("only agents file proposals"));
    }
    let config = load_config(paths)?;
    if input.nodes.len() != expected_nodes(input.kind) {
        return Err(RootsError::usage(format!(
            "propose {} takes {} idea{}",
            kind_name(input.kind),
            expected_nodes(input.kind),
            if expected_nodes(input.kind) == 1 {
                ""
            } else {
                "s"
            }
        )));
    }
    let graph = read_graph(paths)?;
    let endpoints = input
        .nodes
        .iter()
        .map(|query| {
            resolve_node(
                &graph.nodes,
                query,
                ResolveOptions {
                    kind: Some(NodeKind::Idea),
                },
            )
        })
        .collect::<RootsResult<Vec<_>>>()?;
    for node in &endpoints {
        require_tier(&config, 2, "`luuna-roots propose`", Some(node))?;
        live_idea(node)?;
    }
    if endpoints.len() == 2 && endpoints[0].id == endpoints[1].id {
        return Err(RootsError::validation(format!(
            "both ends are {}",
            endpoints[1].id
        )));
    }
    let cites = input
        .cites
        .iter()
        .map(|raw| parse_cite(raw, &graph))
        .collect::<RootsResult<Vec<_>>>()?;
    for cite in &cites {
        if let Some(node) = graph
            .nodes
            .iter()
            .find(|node| node.id == cite.node)
            .filter(|node| node.kind == NodeKind::Idea)
        {
            require_tier(&config, 2, "`luuna-roots propose`", Some(node))?;
        }
    }
    let mut draft = ProposalDraft {
        kind: input.kind,
        from: Some(endpoints[0].id.clone()),
        to: endpoints.get(1).map(|node| node.id.clone()),
        rel: None,
        reason: Some(clean_reason(input.reason)?),
        cites,
        by: input.by.to_owned(),
    };
    if input.kind == ProposalKind::Edge {
        let raw_rel = input
            .rel
            .ok_or_else(|| RootsError::usage("propose edge needs a relation"))?;
        let rel = parse_link_rel(raw_rel)?;
        if let Some(message) = check_edge(
            &graph,
            draft.from.as_deref().unwrap_or_default(),
            draft.to.as_deref().unwrap_or_default(),
            rel,
        ) {
            return Err(RootsError::validation(message));
        }
        draft.rel = Some(rel);
        if input.mention {
            let from_cited = draft
                .cites
                .iter()
                .any(|cite| Some(cite.node.as_str()) == draft.from.as_deref());
            let to_cited = draft
                .cites
                .iter()
                .any(|cite| Some(cite.node.as_str()) == draft.to.as_deref());
            if !from_cited && !to_cited {
                return Err(RootsError::validation(format!(
                    "--mention: cite the line that mentions the other idea (--cite {}:<line>)",
                    draft.from.as_deref().unwrap_or_default()
                )));
            }
            if from_cited != to_cited {
                let other = if from_cited {
                    draft.to.as_deref()
                } else {
                    draft.from.as_deref()
                }
                .unwrap_or_default();
                if let Some(node) = graph.nodes.iter().find(|node| node.id == other) {
                    let statement = read_claim(paths, &graph, node)?.statement;
                    if !statement.is_empty() {
                        draft.cites.push(Citation {
                            node: other.to_owned(),
                            quote: statement,
                        });
                    }
                }
            }
        }
    } else if input.mention {
        return Err(RootsError::usage(
            "--mention applies to `propose edge` only",
        ));
    }
    let filed = file_proposals(
        paths,
        &[draft],
        config.limits.proposals,
        config.limits.proposal_ttl_days,
        input.now,
    )?;
    filed
        .into_iter()
        .next()
        .ok_or_else(|| RootsError::validation("proposal was not filed"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_cites_on_the_first_colon() {
        assert_eq!(proposal_kind("EDGE"), Some(ProposalKind::Edge));
        assert_eq!(
            clean_reason(Some("  reason\n here ")).unwrap(),
            "reason here"
        );
        assert!(matches!(parse_link_rel("derives"), Err(error) if error.message.contains("adopt")));
    }
}
