//! Graph JSONL access and pure accepted-graph queries.

use crate::errors::RootsResult;
use crate::model::{EdgeRecord, EdgeRel, Graph, GraphRecord, IdeaStatus, NodeRecord, NodeStatus};
use crate::paths::RootsPaths;
use crate::store::{LockOptions, dedup_by_id, read_jsonl_file, with_lock, write_jsonl_file};

#[must_use]
pub fn graph_from_records(records: Vec<GraphRecord>) -> Graph {
    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    for record in records {
        match record {
            GraphRecord::Node(node) => nodes.push(node),
            GraphRecord::Edge(edge) => edges.push(edge),
        }
    }
    Graph {
        nodes: dedup_by_id(nodes),
        edges: dedup_by_id(edges),
    }
}

pub fn read_graph(paths: &RootsPaths) -> RootsResult<Graph> {
    read_jsonl_file(&paths.graph).map(graph_from_records)
}

#[must_use]
pub fn graph_records(graph: &Graph) -> Vec<GraphRecord> {
    graph
        .nodes
        .iter()
        .cloned()
        .map(GraphRecord::Node)
        .chain(graph.edges.iter().cloned().map(GraphRecord::Edge))
        .collect()
}

pub fn update_graph<R, F>(paths: &RootsPaths, action: F) -> RootsResult<R>
where
    F: FnOnce(&mut Graph) -> RootsResult<(bool, R)>,
{
    with_lock(
        &paths.graph,
        || {
            let mut graph = read_graph(paths)?;
            let (write, result) = action(&mut graph)?;
            if write {
                write_jsonl_file(&paths.graph, &graph_records(&graph))?;
            }
            Ok(result)
        },
        LockOptions::default(),
    )
}

pub fn replace_node(graph: &mut Graph, node: NodeRecord) {
    if let Some(index) = graph.nodes.iter().position(|current| current.id == node.id) {
        graph.nodes[index] = node;
    } else {
        graph.nodes.push(node);
    }
}

#[must_use]
pub fn find_node<'a>(graph: &'a Graph, id: &str) -> Option<&'a NodeRecord> {
    graph.nodes.iter().find(|node| node.id == id)
}

#[must_use]
pub fn outgoing_edges<'a>(graph: &'a Graph, id: &str) -> Vec<&'a EdgeRecord> {
    graph.edges.iter().filter(|edge| edge.from == id).collect()
}

#[must_use]
pub fn incoming_edges<'a>(graph: &'a Graph, id: &str) -> Vec<&'a EdgeRecord> {
    graph.edges.iter().filter(|edge| edge.to == id).collect()
}

#[must_use]
pub fn has_any_edge(graph: &Graph, id: &str) -> bool {
    graph
        .edges
        .iter()
        .any(|edge| edge.from == id || edge.to == id)
}

#[must_use]
pub fn is_anchor(graph: &Graph, node: &NodeRecord) -> bool {
    !matches!(node.status, NodeStatus::Idea(IdeaStatus::Composted))
        && matches!(node.kind, crate::model::NodeKind::Idea)
        && !graph
            .edges
            .iter()
            .any(|edge| edge.from == node.id && edge.rel == EdgeRel::Serves)
}

#[must_use]
pub fn is_orphan(graph: &Graph, node: &NodeRecord) -> bool {
    !has_any_edge(graph, &node.id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{IdeaStatus, NodeKind};

    #[test]
    fn deduplicates_nodes_and_edges_independently() {
        let node = |slug: &str| {
            NodeRecord::new(
                "r-a1b2".to_owned(),
                NodeKind::Idea,
                slug.to_owned(),
                NodeStatus::Idea(IdeaStatus::Planted),
                "human:test".to_owned(),
                "now".to_owned(),
            )
        };
        let graph = graph_from_records(vec![
            GraphRecord::Node(node("old")),
            GraphRecord::Node(node("new")),
        ]);
        assert_eq!(graph.nodes.len(), 1);
        assert_eq!(graph.nodes[0].slug, "new");
    }
}
