//! Opening a Roots project and reconciling deliberate directory renames.

use crate::errors::RootsResult;
use crate::events::{EventFields, append_event, make_event};
use crate::graph::{read_graph, update_graph};
use crate::model::{Graph, NodeRecord};
use crate::paths::{RootsPaths, require_roots_paths};
use crate::prose::{NodeDirs, dir_candidates, dir_name_for, dir_slug, scan_node_dirs};
use crate::slug::is_valid_slug;
use crate::time::iso_now;
use serde_json::{Map, Value};
use std::path::Path;

pub const DIR_RENAME_ACTOR: &str = "roots:dir-rename";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SlugFix {
    pub id: String,
    pub old_slug: String,
    pub new_slug: String,
    pub dir: String,
}

#[derive(Clone, Debug)]
pub struct Workspace {
    pub paths: RootsPaths,
    pub graph: Graph,
    pub dirs: NodeDirs,
    pub fixes: Vec<SlugFix>,
}

fn slug_from_disk(node: &NodeRecord, dirs: &NodeDirs, graph: &Graph) -> Option<SlugFix> {
    let names = dir_candidates(dirs, node);
    if names.len() != 1 || names.iter().any(|name| name == &dir_name_for(node)) {
        return None;
    }
    let dir = names.first()?.clone();
    let slug = dir_slug(&dir);
    if !is_valid_slug(slug)
        || slug == node.slug
        || graph
            .nodes
            .iter()
            .any(|other| other.id != node.id && other.slug == slug)
    {
        return None;
    }
    Some(SlugFix {
        id: node.id.clone(),
        old_slug: node.slug.clone(),
        new_slug: slug.to_owned(),
        dir,
    })
}

#[must_use]
pub fn detect_slug_fixes(graph: &Graph, dirs: &NodeDirs) -> Vec<SlugFix> {
    graph
        .nodes
        .iter()
        .filter_map(|node| slug_from_disk(node, dirs, graph))
        .collect()
}

pub fn reconcile_dirs(paths: &RootsPaths) -> RootsResult<Vec<SlugFix>> {
    let fixes = update_graph(paths, |graph| {
        let found = detect_slug_fixes(graph, &scan_node_dirs(paths)?);
        let at = iso_now();
        for fix in &found {
            if let Some(node) = graph.nodes.iter_mut().find(|node| node.id == fix.id) {
                node.slug = fix.new_slug.clone();
                node.updated_at = Some(at.clone());
            }
        }
        Ok((!found.is_empty(), found))
    })?;
    for fix in &fixes {
        let mut extra = Map::new();
        extra.insert("oldSlug".to_owned(), Value::String(fix.old_slug.clone()));
        extra.insert("newSlug".to_owned(), Value::String(fix.new_slug.clone()));
        extra.insert("dir".to_owned(), Value::String(fix.dir.clone()));
        append_event(
            paths,
            &make_event(
                "mv",
                DIR_RENAME_ACTOR,
                EventFields {
                    node: Some(fix.id.clone()),
                    extra,
                    ..Default::default()
                },
            ),
        )?;
    }
    Ok(fixes)
}

pub fn open_workspace(cwd: impl AsRef<Path>) -> RootsResult<Workspace> {
    let paths = require_roots_paths(cwd)?;
    let graph = read_graph(&paths)?;
    let dirs = scan_node_dirs(&paths)?;
    let fixes = if detect_slug_fixes(&graph, &dirs).is_empty() {
        Vec::new()
    } else {
        reconcile_dirs(&paths)?
    };
    Ok(Workspace {
        graph: read_graph(&paths)?,
        dirs: scan_node_dirs(&paths)?,
        paths,
        fixes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{IdeaStatus, NodeKind, NodeStatus};
    use std::collections::BTreeMap;

    #[test]
    fn only_unique_valid_manual_renames_are_reconciled() {
        let node = NodeRecord::new(
            "r-a1b2".into(),
            NodeKind::Idea,
            "old".into(),
            NodeStatus::Idea(IdeaStatus::Planted),
            "human:test".into(),
            "now".into(),
        );
        let graph = Graph {
            nodes: vec![node],
            edges: vec![],
        };
        let mut idea = BTreeMap::new();
        idea.insert("a1b2".into(), vec!["a1b2-new".into()]);
        let dirs = NodeDirs {
            idea,
            sprout: BTreeMap::new(),
        };
        assert_eq!(detect_slug_fixes(&graph, &dirs)[0].new_slug, "new");
    }
}
