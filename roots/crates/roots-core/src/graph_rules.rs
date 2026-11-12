//! Pure accepted-graph invariants used before an edge proposal is filed.

use crate::graph::find_node;
use crate::model::{EdgeRecord, EdgeRel, Graph, NodeKind, NodeStatus};

pub const LINK_RELS: [&str; 3] = ["serves", "tension", "replaces"];

#[must_use]
pub fn is_link_rel(raw: &str) -> bool {
    LINK_RELS.contains(&raw)
}

#[must_use]
pub fn same_edge_key(left: (&str, &str, EdgeRel), right: (&str, &str, EdgeRel)) -> bool {
    left.2 == right.2
        && ((left.0 == right.0 && left.1 == right.1)
            || (left.2 == EdgeRel::Tension && left.0 == right.1 && left.1 == right.0))
}

fn would_cycle(edges: &[EdgeRecord], from: &str, to: &str, rel: EdgeRel) -> bool {
    let mut seen = std::collections::BTreeSet::new();
    let mut stack = vec![to.to_owned()];
    while let Some(node) = stack.pop() {
        if node == from {
            return true;
        }
        if !seen.insert(node.clone()) {
            continue;
        }
        for edge in edges {
            if edge.rel == rel && edge.from == node {
                stack.push(edge.to.clone());
            }
        }
    }
    false
}

/// Return the first invariant failure, or `None` when a human could create it.
#[must_use]
pub fn check_edge(graph: &Graph, from: &str, to: &str, rel: EdgeRel) -> Option<String> {
    if from == to {
        return Some(format!("an idea cannot {} itself ({from})", rel.as_str()));
    }
    let Some(source) = find_node(graph, from) else {
        return Some(format!("node {from} does not exist"));
    };
    let Some(target) = find_node(graph, to) else {
        return Some(format!("node {to} does not exist"));
    };
    if source.kind != NodeKind::Idea {
        return Some(format!(
            "{} {} is a sprout; `{}` connects ideas only",
            source.id,
            source.slug,
            rel.as_str()
        ));
    }
    if target.kind != NodeKind::Idea {
        return Some(format!(
            "{} {} is a sprout; `{}` connects ideas only",
            target.id,
            target.slug,
            rel.as_str()
        ));
    }
    if matches!(
        source.status,
        NodeStatus::Idea(crate::model::IdeaStatus::Composted)
    ) {
        return Some(format!("{} {} is composted", source.id, source.slug));
    }
    if matches!(
        target.status,
        NodeStatus::Idea(crate::model::IdeaStatus::Composted)
    ) && rel != EdgeRel::Replaces
    {
        return Some(format!("{} {} is composted", target.id, target.slug));
    }
    if let Some(edge) = graph
        .edges
        .iter()
        .find(|edge| same_edge_key((&edge.from, &edge.to, edge.rel), (from, to, rel)))
    {
        return Some(format!(
            "{from} {} {to} already exists ({})",
            rel.as_str(),
            edge.id
        ));
    }
    if matches!(rel, EdgeRel::Serves | EdgeRel::Replaces)
        && would_cycle(&graph.edges, from, to, rel)
    {
        return Some(format!(
            "{from} {} {to} would create a `{}` cycle ({} must stay acyclic)",
            rel.as_str(),
            rel.as_str(),
            rel.as_str()
        ));
    }
    None
}
