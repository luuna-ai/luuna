//! Resolution of the compact node references accepted by the CLI.

use crate::errors::{RootsError, RootsResult};
use crate::ids::hex_of;
use crate::model::{NodeKind, NodeRecord};
use crate::suggestions::closest;

#[derive(Clone, Copy, Debug, Default)]
pub struct ResolveOptions {
    pub kind: Option<NodeKind>,
}

fn description(node: &NodeRecord) -> String {
    format!("{} {}", node.id, node.slug)
}

#[must_use]
pub fn match_nodes<'a>(nodes: &'a [NodeRecord], raw: &str) -> Vec<&'a NodeRecord> {
    let query = raw.trim().to_lowercase();
    if query.is_empty() {
        return Vec::new();
    }
    let exact_id: Vec<_> = nodes.iter().filter(|node| node.id == query).collect();
    if !exact_id.is_empty() {
        return exact_id;
    }
    let exact_hex: Vec<_> = nodes
        .iter()
        .filter(|node| hex_of(&node.id) == query)
        .collect();
    if !exact_hex.is_empty() {
        return exact_hex;
    }
    let exact_slug: Vec<_> = nodes.iter().filter(|node| node.slug == query).collect();
    if !exact_slug.is_empty() {
        return exact_slug;
    }
    nodes
        .iter()
        .filter(|node| {
            node.id.starts_with(&query)
                || hex_of(&node.id).starts_with(&query)
                || node.slug.starts_with(&query)
        })
        .collect()
}

pub fn resolve_node<'a>(
    nodes: &'a [NodeRecord],
    query: &str,
    options: ResolveOptions,
) -> RootsResult<&'a NodeRecord> {
    if query.trim().is_empty() {
        return Err(RootsError::usage("empty node reference"));
    }
    let pool: Vec<&NodeRecord> = options.kind.map_or_else(
        || nodes.iter().collect(),
        |kind| nodes.iter().filter(|node| node.kind == kind).collect(),
    );
    let pool_values: Vec<NodeRecord> = pool.iter().map(|node| (*node).clone()).collect();
    let found = match_nodes(&pool_values, query);
    if found.len() == 1 {
        let wanted = found[0].id.as_str();
        return pool
            .into_iter()
            .find(|node| node.id == wanted)
            .ok_or_else(|| RootsError::error("node resolution lost the matched record"));
    }
    if found.len() > 1 {
        let ids = found.iter().map(|node| node.id.clone()).collect::<Vec<_>>();
        return Err(RootsError::ambiguous(
            format!(
                "\"{query}\" is ambiguous; it matches: {}",
                found
                    .iter()
                    .map(|node| description(node))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            ids,
        ));
    }
    if let Some(kind) = options.kind {
        let other = match_nodes(nodes, query);
        if other.len() == 1 {
            return Err(RootsError::usage(format!(
                "{} is a {}, not an {}",
                other[0].id,
                other[0].kind.as_str(),
                kind.as_str()
            )));
        }
    }
    let what = options.kind.map_or("node", NodeKind::as_str);
    let slugs: Vec<&str> = pool.iter().map(|node| node.slug.as_str()).collect();
    let suffix = closest(&query.to_lowercase(), &slugs, 3)
        .map(|hint| format!(" Did you mean {hint}?"))
        .unwrap_or_default();
    Err(RootsError::not_found(format!(
        "no {what} matches \"{query}\".{suffix}"
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{IdeaStatus, NodeKind, NodeStatus};
    fn node(id: &str, slug: &str) -> NodeRecord {
        NodeRecord::new(
            id.into(),
            NodeKind::Idea,
            slug.into(),
            NodeStatus::Idea(IdeaStatus::Planted),
            "human:a".into(),
            "now".into(),
        )
    }
    #[test]
    fn exact_forms_win_over_prefixes() {
        let nodes = vec![node("r-a1b2", "offline"), node("r-a1b3", "offline-2")];
        assert_eq!(
            resolve_node(&nodes, "a1b2", ResolveOptions::default())
                .unwrap()
                .id,
            "r-a1b2"
        );
        assert!(resolve_node(&nodes, "a1b", ResolveOptions::default()).is_err());
    }
}
