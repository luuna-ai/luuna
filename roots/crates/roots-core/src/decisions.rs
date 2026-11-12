//! Human decisions for pending agent proposals.
//!
//! This is the non-interactive portion shared by `accept`, `reject`, and the
//! later terminal review flow.  It intentionally keeps proposal locking ahead
//! of graph locking, as the TypeScript implementation does.

use crate::errors::{RootsError, RootsResult};
use crate::events::{EventFields, append_event, make_event};
use crate::graph::{read_graph, update_graph};
use crate::graph_rules::check_edge;
use crate::ids::{IdPrefix, generate_id, hex_set};
use crate::model::{
    EdgeRecord, EdgeRel, IdeaStatus, NodeStatus, ProposalKind, ProposalRecord, ProposalStatus,
};
use crate::paths::RootsPaths;
use crate::proposals::{is_expired, moot_reason, parse_link_rel, read_proposals};
use crate::store::update_table;
use crate::time::iso_at;
use serde::Serialize;
use serde_json::{Map, Value};
use time::OffsetDateTime;

#[derive(Clone, Debug, Default)]
pub struct AcceptOptions {
    pub rel: Option<String>,
    pub keep: Option<String>,
    pub reason: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct NodeStatusChange {
    pub node: String,
    pub from: String,
    pub to: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ThinkHandoff {
    pub node: String,
    pub guidance: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct Decision {
    pub id: String,
    pub action: String,
    pub proposals: Vec<ProposalRecord>,
    pub edge: Option<EdgeRecord>,
    #[serde(rename = "statusChanges")]
    pub status_changes: Vec<NodeStatusChange>,
    pub think: Option<ThinkHandoff>,
}

#[derive(Clone, Debug)]
struct Outcome {
    decided: Vec<ProposalRecord>,
    edge: Option<EdgeRecord>,
    changes: Vec<NodeStatusChange>,
    think: Option<ThinkHandoff>,
}

struct DecisionEdgeDraft<'a> {
    from: &'a str,
    to: &'a str,
    rel: EdgeRel,
    by: &'a str,
    proposed_by: &'a str,
    proposal: &'a str,
    now: OffsetDateTime,
}

fn proposal_prefix(id: &str) -> RootsResult<String> {
    let normalized = id.trim().to_lowercase();
    match crate::ids::prefix_of(&normalized) {
        Some("p") => Ok(normalized),
        Some("s") => Err(RootsError::usage(
            "sprout decisions are handled by `luuna-roots adopt` or the interactive `luuna-roots tend`",
        )),
        _ => Err(RootsError::usage(format!(
            "expected a proposal (p-xxxx) or sprout (s-xxxx) id, got \"{id}\""
        ))),
    }
}

fn find_pending<'a>(rows: &'a [ProposalRecord], id: &str) -> RootsResult<&'a ProposalRecord> {
    let Some(proposal) = rows.iter().find(|proposal| proposal.id == id) else {
        return Err(RootsError::not_found(format!(
            "no proposal {id}; `luuna-roots tend` lists pending proposals"
        )));
    };
    if proposal.status != ProposalStatus::Pending {
        let who = proposal
            .decided_by
            .as_deref()
            .map(|actor| format!(" by {actor}"))
            .unwrap_or_default();
        let status = match proposal.status {
            ProposalStatus::Pending => "pending",
            ProposalStatus::Accepted => "accepted",
            ProposalStatus::Rejected => "rejected",
            ProposalStatus::Expired => "expired",
        };
        return Err(RootsError::conflict(format!(
            "{id} is already {status}{who}"
        )));
    }
    Ok(proposal)
}

fn same_pair(left: &ProposalRecord, right: &ProposalRecord) -> bool {
    (left.from == right.from && left.to == right.to)
        || (left.from == right.to && left.to == right.from)
}

fn proposal_group(
    rows: &[ProposalRecord],
    proposal: &ProposalRecord,
    now: OffsetDateTime,
) -> Vec<String> {
    if proposal.kind != ProposalKind::Edge {
        return vec![proposal.id.clone()];
    }
    let mut ids = vec![proposal.id.clone()];
    ids.extend(
        rows.iter()
            .filter(|other| {
                other.id != proposal.id
                    && other.kind == ProposalKind::Edge
                    && other.status == ProposalStatus::Pending
                    && !is_expired(other, now)
                    && same_pair(other, proposal)
            })
            .map(|other| other.id.clone()),
    );
    ids
}

fn mark(
    proposal: &mut ProposalRecord,
    status: ProposalStatus,
    by: &str,
    now: OffsetDateTime,
    reason: Option<&str>,
) {
    proposal.status = status;
    proposal.decided_by = Some(by.to_owned());
    proposal.decided_at = Some(iso_at(now));
    proposal.decision_reason = reason
        .map(str::trim)
        .filter(|reason| !reason.is_empty())
        .map(ToOwned::to_owned);
}

fn insert_edge(
    paths: &RootsPaths,
    draft: DecisionEdgeDraft<'_>,
) -> RootsResult<(EdgeRecord, Option<NodeStatusChange>)> {
    update_graph(paths, |graph| {
        if let Some(message) = check_edge(graph, draft.from, draft.to, draft.rel) {
            let mut detail = Map::new();
            detail.insert("from".to_owned(), Value::String(draft.from.to_owned()));
            detail.insert("to".to_owned(), Value::String(draft.to.to_owned()));
            detail.insert(
                "rel".to_owned(),
                Value::String(draft.rel.as_str().to_owned()),
            );
            return Err(RootsError::validation(message).with_detail(detail));
        }
        let taken = hex_set(graph.edges.iter().map(|edge| edge.id.as_str()));
        let edge = EdgeRecord {
            record_type: "edge".to_owned(),
            id: generate_id(IdPrefix::Edge, &taken),
            from: draft.from.to_owned(),
            to: draft.to.to_owned(),
            rel: draft.rel,
            by: draft.by.to_owned(),
            proposed_by: Some(draft.proposed_by.to_owned()),
            proposal: Some(draft.proposal.to_owned()),
            created_at: iso_at(draft.now),
        };
        graph.edges.push(edge.clone());
        let change = if draft.rel == EdgeRel::Replaces {
            graph
                .nodes
                .iter_mut()
                .find(|node| node.id == draft.to)
                .and_then(|node| match node.status {
                    NodeStatus::Idea(status) if status != IdeaStatus::Composted => {
                        node.status = NodeStatus::Idea(IdeaStatus::Composted);
                        node.updated_at = Some(iso_at(draft.now));
                        Some(NodeStatusChange {
                            node: node.id.clone(),
                            from: status.as_str().to_owned(),
                            to: IdeaStatus::Composted.as_str().to_owned(),
                        })
                    }
                    _ => None,
                })
        } else {
            None
        };
        Ok((true, (edge, change)))
    })
}

fn compost_node(
    paths: &RootsPaths,
    id: &str,
    now: OffsetDateTime,
) -> RootsResult<Option<NodeStatusChange>> {
    update_graph(paths, |graph| {
        let Some(node) = graph.nodes.iter_mut().find(|node| node.id == id) else {
            return Err(RootsError::conflict(format!("node {id} disappeared")));
        };
        let NodeStatus::Idea(status) = node.status else {
            return Err(RootsError::conflict(format!(
                "{id} is a sprout; sprouts are adopted or rejected"
            )));
        };
        if status == IdeaStatus::Composted {
            return Ok((false, None));
        }
        node.status = NodeStatus::Idea(IdeaStatus::Composted);
        node.updated_at = Some(iso_at(now));
        Ok((
            true,
            Some(NodeStatusChange {
                node: id.to_owned(),
                from: status.as_str().to_owned(),
                to: IdeaStatus::Composted.as_str().to_owned(),
            }),
        ))
    })
}

fn choose_relation(
    group: &[ProposalRecord],
    raw: Option<&str>,
) -> RootsResult<(EdgeRel, ProposalRecord)> {
    let first = group.first();
    let with_relation = group.iter().find(|proposal| proposal.rel.is_some());
    let mention = group.iter().find(|proposal| proposal.rel.is_none());
    let relation = match raw {
        None => with_relation
            .and_then(|proposal| proposal.rel)
            .ok_or_else(|| {
                RootsError::usage(format!(
                    "{} links {} and {} but names no relation; pass --rel serves|tension|replaces",
                    first
                        .map(|proposal| proposal.id.as_str())
                        .unwrap_or_default(),
                    first
                        .and_then(|proposal| proposal.from.as_deref())
                        .unwrap_or_default(),
                    first
                        .and_then(|proposal| proposal.to.as_deref())
                        .unwrap_or_default(),
                ))
            })?,
        Some(raw) => parse_link_rel(raw)?,
    };
    let source = if raw.is_none() {
        with_relation
    } else {
        group
            .iter()
            .find(|proposal| proposal.rel == Some(relation))
            .or(mention)
    }
    .ok_or_else(|| {
        let original = with_relation
            .and_then(|proposal| proposal.rel)
            .map(EdgeRel::as_str)
            .unwrap_or_default();
        RootsError::usage(format!(
            "{} proposes `{original}`, not `{}`; reject it and run `luuna-roots link {} {} {}` instead",
            with_relation.map(|proposal| proposal.id.as_str()).unwrap_or_default(),
            relation.as_str(),
            first.and_then(|proposal| proposal.from.as_deref()).unwrap_or_default(),
            first.and_then(|proposal| proposal.to.as_deref()).unwrap_or_default(),
            relation.as_str(),
        ))
    })?
    .clone();
    Ok((relation, source))
}

fn survivor(proposal: &ProposalRecord, raw: Option<&str>) -> RootsResult<(String, String)> {
    let ends = [
        proposal.from.as_deref().unwrap_or_default(),
        proposal.to.as_deref().unwrap_or_default(),
    ];
    let Some(raw) = raw else {
        return Err(RootsError::usage(format!(
            "merging needs the idea that survives: --keep {} or --keep {}",
            ends[0], ends[1]
        )));
    };
    let keep = raw.trim().to_lowercase();
    let hit = ends
        .iter()
        .find(|candidate| keep == **candidate || format!("r-{keep}") == **candidate)
        .ok_or_else(|| {
            RootsError::usage(format!(
                "--keep must be {} or {}, got \"{raw}\"",
                ends[0], ends[1]
            ))
        })?;
    let other = ends
        .iter()
        .find(|candidate| **candidate != *hit)
        .copied()
        .unwrap_or_default();
    Ok(((*hit).to_owned(), other.to_owned()))
}

fn expire_pending(paths: &RootsPaths, now: OffsetDateTime) -> RootsResult<()> {
    let graph = read_graph(paths)?;
    let expired = update_table::<ProposalRecord, _, _>(&paths.proposals, |mut rows| {
        let mut changed = false;
        let mut output = Vec::new();
        for proposal in &mut rows {
            if proposal.status != ProposalStatus::Pending {
                continue;
            }
            let reason = if is_expired(proposal, now) {
                Some("ttl".to_owned())
            } else {
                moot_reason(proposal, &graph, now).map(|reason| format!("moot: {reason}"))
            };
            if let Some(reason) = reason {
                proposal.status = ProposalStatus::Expired;
                changed = true;
                output.push((proposal.clone(), reason));
            }
        }
        Ok((rows, changed, output))
    })?;
    for (proposal, reason) in expired {
        let mut extra = Map::new();
        extra.insert("proposal".to_owned(), Value::String(proposal.id.clone()));
        extra.insert(
            "kind".to_owned(),
            Value::String(
                match proposal.kind {
                    ProposalKind::Edge => "edge",
                    ProposalKind::Split => "split",
                    ProposalKind::Merge => "merge",
                    ProposalKind::Compost => "compost",
                }
                .to_owned(),
            ),
        );
        if proposal.kind == ProposalKind::Edge {
            extra.insert(
                "rel".to_owned(),
                proposal
                    .rel
                    .map(|relation| Value::String(relation.as_str().to_owned()))
                    .unwrap_or(Value::Null),
            );
        }
        extra.insert("reason".to_owned(), Value::String(reason));
        append_event(
            paths,
            &make_event(
                "expire",
                "roots",
                EventFields {
                    node: proposal.from.clone(),
                    // Proposal audit events always carry `refs`, including an
                    // empty array for split/compost records. This is an
                    // observable historical JSONL shape.
                    refs: Some(
                        proposal
                            .to
                            .clone()
                            .map_or_else(Vec::new, |value| vec![value]),
                    ),
                    extra,
                    ..Default::default()
                },
            ),
        )?;
    }
    Ok(())
}

fn decision_events(by: &str, outcome: &Outcome, primary: &str) -> Vec<crate::model::EventRecord> {
    let mut events = Vec::new();
    for proposal in &outcome.decided {
        let mut extra = Map::new();
        extra.insert("proposal".to_owned(), Value::String(proposal.id.clone()));
        extra.insert(
            "kind".to_owned(),
            Value::String(
                match proposal.kind {
                    ProposalKind::Edge => "edge",
                    ProposalKind::Split => "split",
                    ProposalKind::Merge => "merge",
                    ProposalKind::Compost => "compost",
                }
                .to_owned(),
            ),
        );
        if proposal.kind == ProposalKind::Edge {
            extra.insert(
                "rel".to_owned(),
                proposal
                    .rel
                    .map(|relation| Value::String(relation.as_str().to_owned()))
                    .unwrap_or(Value::Null),
            );
        }
        if let Some(edge) = &outcome.edge {
            if edge.proposal.as_deref() == Some(proposal.id.as_str()) {
                extra.insert("edge".to_owned(), Value::String(edge.id.clone()));
                extra.insert(
                    "edgeRel".to_owned(),
                    Value::String(edge.rel.as_str().to_owned()),
                );
            }
        }
        if let Some(reason) = &proposal.decision_reason {
            extra.insert("reason".to_owned(), Value::String(reason.clone()));
        }
        if proposal.id != primary {
            extra.insert("with".to_owned(), Value::String(primary.to_owned()));
        }
        let event_type = if proposal.status == ProposalStatus::Accepted {
            "accept"
        } else {
            "reject"
        };
        events.push(make_event(
            event_type,
            by,
            EventFields {
                node: proposal.from.clone(),
                refs: Some(
                    proposal
                        .to
                        .clone()
                        .map_or_else(Vec::new, |value| vec![value]),
                ),
                extra,
                ..Default::default()
            },
        ));
    }
    for change in &outcome.changes {
        let mut extra = Map::new();
        if let Some(edge) = &outcome.edge {
            extra.insert("edge".to_owned(), Value::String(edge.id.clone()));
        } else {
            extra.insert("proposal".to_owned(), Value::String(primary.to_owned()));
        }
        extra.insert("from".to_owned(), Value::String(change.from.clone()));
        extra.insert("to".to_owned(), Value::String(change.to.clone()));
        events.push(make_event(
            if outcome.edge.is_some() {
                "status"
            } else {
                "compost"
            },
            by,
            EventFields {
                node: Some(change.node.clone()),
                extra,
                ..Default::default()
            },
        ));
    }
    events
}

/// Accept one pending proposal, deciding any pending edge suggestions for the
/// same unordered pair as one card.
pub fn accept_proposal(
    paths: &RootsPaths,
    by: &str,
    raw_id: &str,
    options: AcceptOptions,
    now: Option<OffsetDateTime>,
) -> RootsResult<Decision> {
    let id = proposal_prefix(raw_id)?;
    let now = now.unwrap_or_else(OffsetDateTime::now_utc);
    expire_pending(paths, now)?;
    let outcome = update_table::<ProposalRecord, _, _>(&paths.proposals, |mut rows| {
        let selected = find_pending(&rows, &id)?.clone();
        let group_ids = proposal_group(&rows, &selected, now);
        let outcome = match selected.kind {
            ProposalKind::Edge => {
                let group = group_ids
                    .iter()
                    .filter_map(|identifier| {
                        rows.iter().find(|proposal| proposal.id == *identifier)
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                let (relation, source) = choose_relation(&group, options.rel.as_deref())?;
                let (edge, status) = insert_edge(
                    paths,
                    DecisionEdgeDraft {
                        from: source.from.as_deref().unwrap_or_default(),
                        to: source.to.as_deref().unwrap_or_default(),
                        rel: relation,
                        by,
                        proposed_by: &source.by,
                        proposal: &source.id,
                        now,
                    },
                )?;
                let mut decided = Vec::new();
                for identifier in group_ids {
                    if let Some(proposal) =
                        rows.iter_mut().find(|proposal| proposal.id == identifier)
                    {
                        let reason = if proposal.rel.is_none() || proposal.rel == Some(relation) {
                            options.reason.as_deref()
                        } else {
                            Some(match relation {
                                EdgeRel::Serves => "human chose serves",
                                EdgeRel::Tension => "human chose tension",
                                EdgeRel::Replaces => "human chose replaces",
                                EdgeRel::Derives => "human chose derives",
                            })
                        };
                        let status_value =
                            if proposal.rel.is_none() || proposal.rel == Some(relation) {
                                ProposalStatus::Accepted
                            } else {
                                ProposalStatus::Rejected
                            };
                        mark(proposal, status_value, by, now, reason);
                        decided.push(proposal.clone());
                    }
                }
                Outcome {
                    decided,
                    edge: Some(edge),
                    changes: status.into_iter().collect(),
                    think: None,
                }
            }
            ProposalKind::Merge => {
                let (keep, other) = survivor(&selected, options.keep.as_deref())?;
                let (edge, status) = insert_edge(
                    paths,
                    DecisionEdgeDraft {
                        from: &keep,
                        to: &other,
                        rel: EdgeRel::Replaces,
                        by,
                        proposed_by: &selected.by,
                        proposal: &selected.id,
                        now,
                    },
                )?;
                let proposal = rows
                    .iter_mut()
                    .find(|proposal| proposal.id == id)
                    .expect("selected proposal still exists");
                mark(
                    proposal,
                    ProposalStatus::Accepted,
                    by,
                    now,
                    options.reason.as_deref(),
                );
                Outcome {
                    decided: vec![proposal.clone()],
                    edge: Some(edge),
                    changes: status.into_iter().collect(),
                    think: None,
                }
            }
            ProposalKind::Split => {
                let node = selected.from.as_deref().unwrap_or_default();
                let graph = read_graph(paths)?;
                let state = graph.nodes.iter().find(|item| item.id == node);
                if state.is_none()
                    || matches!(
                        state.map(|item| item.status),
                        Some(NodeStatus::Idea(IdeaStatus::Composted))
                    )
                {
                    return Err(RootsError::conflict(format!(
                        "{node} is {}; reject {} instead",
                        if state.is_none() { "gone" } else { "composted" },
                        selected.id
                    )));
                }
                let proposal = rows
                    .iter_mut()
                    .find(|proposal| proposal.id == id)
                    .expect("selected proposal still exists");
                mark(
                    proposal,
                    ProposalStatus::Accepted,
                    by,
                    now,
                    options.reason.as_deref(),
                );
                Outcome {
                    decided: vec![proposal.clone()],
                    edge: None,
                    changes: Vec::new(),
                    think: Some(ThinkHandoff {
                        node: node.to_owned(),
                        guidance: selected.reason.unwrap_or_default(),
                    }),
                }
            }
            ProposalKind::Compost => {
                let node = selected.from.as_deref().unwrap_or_default();
                let status = compost_node(paths, node, now)?;
                let proposal = rows
                    .iter_mut()
                    .find(|proposal| proposal.id == id)
                    .expect("selected proposal still exists");
                mark(
                    proposal,
                    ProposalStatus::Accepted,
                    by,
                    now,
                    options.reason.as_deref(),
                );
                Outcome {
                    decided: vec![proposal.clone()],
                    edge: None,
                    changes: status.into_iter().collect(),
                    think: None,
                }
            }
        };
        Ok((rows, true, outcome))
    })?;
    for event in decision_events(by, &outcome, &id) {
        append_event(paths, &event)?;
    }
    Ok(Decision {
        id,
        action: "accept".to_owned(),
        proposals: outcome.decided,
        edge: outcome.edge,
        status_changes: outcome.changes,
        think: outcome.think,
    })
}

/// Reject one pending proposal/card.  The rows are intentionally retained as
/// permanent memory so an agent cannot immediately file the same proposal.
pub fn reject_proposal(
    paths: &RootsPaths,
    by: &str,
    raw_id: &str,
    reason: Option<&str>,
    now: Option<OffsetDateTime>,
) -> RootsResult<Decision> {
    let id = proposal_prefix(raw_id)?;
    let now = now.unwrap_or_else(OffsetDateTime::now_utc);
    expire_pending(paths, now)?;
    let outcome = update_table::<ProposalRecord, _, _>(&paths.proposals, |mut rows| {
        let selected = find_pending(&rows, &id)?.clone();
        let group_ids = proposal_group(&rows, &selected, now);
        let mut decided = Vec::new();
        for identifier in group_ids {
            if let Some(proposal) = rows.iter_mut().find(|proposal| proposal.id == identifier) {
                mark(proposal, ProposalStatus::Rejected, by, now, reason);
                decided.push(proposal.clone());
            }
        }
        Ok((
            rows,
            true,
            Outcome {
                decided,
                edge: None,
                changes: Vec::new(),
                think: None,
            },
        ))
    })?;
    for event in decision_events(by, &outcome, &id) {
        append_event(paths, &event)?;
    }
    Ok(Decision {
        id,
        action: "reject".to_owned(),
        proposals: outcome.decided,
        edge: None,
        status_changes: Vec::new(),
        think: None,
    })
}

/// Read-only helper retained for `scan`/presentation code.
pub fn pending_rows(paths: &RootsPaths) -> RootsResult<Vec<ProposalRecord>> {
    read_proposals(paths)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::graph_records;
    use crate::model::{Graph, NodeKind, NodeRecord};
    use crate::store::write_jsonl_file;
    use std::fs;

    fn sandbox(name: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!(
            "roots-decisions-{name}-{}",
            crate::ids::random_hex(8)
        ));
        fs::create_dir_all(&root).expect("create sandbox");
        root
    }

    fn pending_edge(id: &str, from: &str, to: &str, relation: Option<EdgeRel>) -> ProposalRecord {
        ProposalRecord {
            id: id.to_owned(),
            kind: ProposalKind::Edge,
            from: Some(from.to_owned()),
            to: Some(to.to_owned()),
            rel: relation,
            reason: Some("why".to_owned()),
            cites: Vec::new(),
            by: "agent:test".to_owned(),
            status: ProposalStatus::Pending,
            created_at: "2026-01-01T00:00:00Z".to_owned(),
            expires_at: Some("2099-01-01T00:00:00Z".to_owned()),
            decided_by: None,
            decided_at: None,
            decision_reason: None,
        }
    }

    fn setup_pair(name: &str) -> (std::path::PathBuf, RootsPaths, String, String) {
        let root = sandbox(name);
        let paths = crate::paths::roots_paths(&root);
        fs::create_dir_all(&paths.human).expect("human tree");
        let a = NodeRecord::new(
            "r-a1b2".to_owned(),
            NodeKind::Idea,
            "one".to_owned(),
            NodeStatus::Idea(IdeaStatus::Planted),
            "human:test".to_owned(),
            "2026-01-01T00:00:00Z".to_owned(),
        );
        let b = NodeRecord::new(
            "r-c3d4".to_owned(),
            NodeKind::Idea,
            "two".to_owned(),
            NodeStatus::Idea(IdeaStatus::Planted),
            "human:test".to_owned(),
            "2026-01-01T00:00:00Z".to_owned(),
        );
        let graph = Graph {
            nodes: vec![a, b],
            edges: Vec::new(),
        };
        write_jsonl_file(&paths.graph, &graph_records(&graph)).expect("write graph");
        (root, paths, "r-a1b2".to_owned(), "r-c3d4".to_owned())
    }

    #[test]
    fn rejects_non_proposal_ids_before_store_access() {
        let root = std::env::temp_dir();
        let paths = crate::paths::roots_paths(root);
        assert!(
            accept_proposal(
                &paths,
                "human:test",
                "r-a1b2",
                AcceptOptions::default(),
                None
            )
            .is_err()
        );
    }

    #[test]
    fn accepts_an_edge_and_rejects_the_rest_of_its_card() {
        let (root, paths, a, b) = setup_pair("accept");
        let mention = pending_edge("p-a1b2", &a, &b, None);
        let agent = pending_edge("p-c3d4", &a, &b, Some(EdgeRel::Tension));
        write_jsonl_file(&paths.proposals, &[mention, agent]).expect("write proposals");
        let decision = accept_proposal(
            &paths,
            "human:test",
            "p-a1b2",
            AcceptOptions {
                rel: Some("serves".to_owned()),
                ..Default::default()
            },
            Some(OffsetDateTime::UNIX_EPOCH),
        )
        .expect("accept card");
        assert_eq!(
            decision.edge.as_ref().map(|edge| edge.rel),
            Some(EdgeRel::Serves)
        );
        assert_eq!(decision.proposals.len(), 2);
        assert_eq!(decision.proposals[0].status, ProposalStatus::Accepted);
        assert_eq!(decision.proposals[1].status, ProposalStatus::Rejected);
        fs::remove_dir_all(root).expect("clean sandbox");
    }

    #[test]
    fn rejects_a_whole_edge_card_with_permanent_rows() {
        let (root, paths, a, b) = setup_pair("reject");
        let first = pending_edge("p-a1b2", &a, &b, Some(EdgeRel::Tension));
        let second = pending_edge("p-c3d4", &a, &b, None);
        write_jsonl_file(&paths.proposals, &[first, second]).expect("write proposals");
        let decision = reject_proposal(
            &paths,
            "human:test",
            "p-a1b2",
            Some("not related"),
            Some(OffsetDateTime::UNIX_EPOCH),
        )
        .expect("reject card");
        assert!(
            decision
                .proposals
                .iter()
                .all(|proposal| proposal.status == ProposalStatus::Rejected)
        );
        assert!(
            decision
                .proposals
                .iter()
                .all(|proposal| proposal.decision_reason.as_deref() == Some("not related"))
        );
        fs::remove_dir_all(root).expect("clean sandbox");
    }

    #[test]
    fn accepts_a_merge_when_keep_uses_the_short_hex() {
        let (root, paths, a, b) = setup_pair("merge-short-keep");
        let mut merge = pending_edge("p-a1b2", &a, &b, None);
        merge.kind = ProposalKind::Merge;
        write_jsonl_file(&paths.proposals, &[merge]).expect("write proposals");

        let decision = accept_proposal(
            &paths,
            "human:test",
            "p-a1b2",
            AcceptOptions {
                keep: Some("a1b2".to_owned()),
                ..Default::default()
            },
            Some(OffsetDateTime::UNIX_EPOCH),
        )
        .expect("accept merge with short id");

        assert_eq!(
            decision.edge.as_ref().map(|edge| edge.from.as_str()),
            Some(a.as_str())
        );
        assert_eq!(
            decision.edge.as_ref().map(|edge| edge.to.as_str()),
            Some(b.as_str())
        );
        assert_eq!(
            read_graph(&paths)
                .expect("read graph")
                .nodes
                .iter()
                .find(|node| node.id == b)
                .map(|node| node.status),
            Some(NodeStatus::Idea(IdeaStatus::Composted))
        );
        fs::remove_dir_all(root).expect("clean sandbox");
    }

    #[test]
    fn expiry_audit_events_retain_a_null_relation_for_mentions() {
        let (root, paths, a, b) = setup_pair("expiry-event-rel");
        let mut mention = pending_edge("p-a1b2", &a, &b, None);
        mention.expires_at = Some("1970-01-01T00:00:00Z".to_owned());
        write_jsonl_file(&paths.proposals, &[mention]).expect("write proposals");

        let error = reject_proposal(
            &paths,
            "human:test",
            "p-c3d4",
            None,
            Some(OffsetDateTime::UNIX_EPOCH),
        )
        .expect_err("expiration happens before lookup");
        assert!(error.message.contains("no proposal p-c3d4"));
        let events = crate::events::read_events(&paths).expect("read expiration event");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].extra.get("rel"), Some(&Value::Null));
        fs::remove_dir_all(root).expect("clean sandbox");
    }

    #[test]
    fn single_node_decisions_keep_the_empty_refs_audit_field() {
        let (root, paths, a, _) = setup_pair("split-empty-refs");
        let mut split = pending_edge("p-a1b2", &a, "", None);
        split.kind = ProposalKind::Split;
        split.to = None;
        write_jsonl_file(&paths.proposals, &[split]).expect("write proposals");

        accept_proposal(
            &paths,
            "human:test",
            "p-a1b2",
            AcceptOptions::default(),
            Some(OffsetDateTime::UNIX_EPOCH),
        )
        .expect("accept split");

        let events = crate::events::read_events(&paths).expect("read decision event");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].refs, Some(Vec::new()));
        fs::remove_dir_all(root).expect("clean sandbox");
    }
}
