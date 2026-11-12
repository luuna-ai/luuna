//! Node-directory discovery and idea/sprout prose parsing.

use crate::errors::{RootsError, RootsResult};
use crate::ids::hex_of;
use crate::model::{NodeKind, NodeRecord};
use crate::paths::{RootsPaths, kind_dir, prose_file_name};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

pub type DirIndex = BTreeMap<String, Vec<String>>;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct NodeDirs {
    pub idea: DirIndex,
    pub sprout: DirIndex,
}

#[must_use]
pub fn dir_hex(name: &str) -> Option<&str> {
    let hex = name.split_once('-').map_or(name, |(hex, _)| hex);
    if (4..=8).contains(&hex.len())
        && hex
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        Some(hex)
    } else {
        None
    }
}

#[must_use]
pub fn dir_slug(name: &str) -> &str {
    name.split_once('-').map_or("", |(_, slug)| slug)
}

pub fn scan_dir(parent: impl AsRef<Path>) -> RootsResult<DirIndex> {
    let parent = parent.as_ref();
    if !parent.exists() {
        return Ok(BTreeMap::new());
    }
    let mut index = BTreeMap::<String, Vec<String>>::new();
    for entry in fs::read_dir(parent).map_err(|error| RootsError::error(error.to_string()))? {
        let entry = entry.map_err(|error| RootsError::error(error.to_string()))?;
        if !entry
            .file_type()
            .map_err(|error| RootsError::error(error.to_string()))?
            .is_dir()
        {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(hex) = dir_hex(&name) else {
            continue;
        };
        index.entry(hex.to_owned()).or_default().push(name);
    }
    for names in index.values_mut() {
        names.sort();
    }
    Ok(index)
}

pub fn scan_node_dirs(paths: &RootsPaths) -> RootsResult<NodeDirs> {
    Ok(NodeDirs {
        idea: scan_dir(&paths.human)?,
        sprout: scan_dir(&paths.sprouts)?,
    })
}

#[must_use]
pub fn dir_name_for(node: &NodeRecord) -> String {
    format!("{}-{}", hex_of(&node.id), node.slug)
}

#[must_use]
pub fn dir_candidates(dirs: &NodeDirs, node: &NodeRecord) -> Vec<String> {
    let map = match node.kind {
        NodeKind::Idea => &dirs.idea,
        NodeKind::Sprout => &dirs.sprout,
    };
    map.get(hex_of(&node.id)).cloned().unwrap_or_default()
}

pub fn locate_node_dir(
    paths: &RootsPaths,
    node: &NodeRecord,
    dirs: Option<&NodeDirs>,
) -> RootsResult<Option<PathBuf>> {
    let owned;
    let dirs = match dirs {
        Some(dirs) => dirs,
        None => {
            owned = scan_node_dirs(paths)?;
            &owned
        }
    };
    let names = dir_candidates(dirs, node);
    let canonical = dir_name_for(node);
    let name = names
        .iter()
        .find(|name| *name == &canonical)
        .or_else(|| names.first());
    Ok(name.map(|name| kind_dir(paths, node.kind).join(name)))
}

#[must_use]
pub fn canonical_node_dir(paths: &RootsPaths, node: &NodeRecord) -> PathBuf {
    kind_dir(paths, node.kind).join(dir_name_for(node))
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Prose {
    pub statement: String,
    pub body: String,
}

#[must_use]
pub fn parse_prose(text: &str) -> Prose {
    let lines = text.replace("\r\n", "\n");
    let mut lines = lines.split('\n');
    let Some(statement) = lines.find(|line| !line.trim().is_empty()) else {
        return Prose {
            statement: String::new(),
            body: String::new(),
        };
    };
    let mut body = lines.collect::<Vec<_>>().join("\n");
    while let Some(newline) = body.find('\n') {
        if body[..newline].trim().is_empty() {
            body.drain(..=newline);
        } else {
            break;
        }
    }
    Prose {
        statement: statement.trim().to_owned(),
        body: body.trim_end().to_owned(),
    }
}

#[must_use]
pub fn content_hash(text: &str) -> String {
    format!("sha256:{:x}", Sha256::digest(text.as_bytes()))
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NodeProse {
    pub path: Option<PathBuf>,
    pub exists: bool,
    pub text: String,
    pub statement: String,
    pub body: String,
}

pub fn read_node_prose(
    paths: &RootsPaths,
    node: &NodeRecord,
    dirs: Option<&NodeDirs>,
) -> RootsResult<NodeProse> {
    let path = locate_node_dir(paths, node, dirs)?
        .map(|directory| directory.join(prose_file_name(node.kind)));
    let Some(path) = path else {
        return Ok(NodeProse {
            path: None,
            exists: false,
            text: String::new(),
            statement: String::new(),
            body: String::new(),
        });
    };
    if !path.exists() {
        return Ok(NodeProse {
            path: Some(path),
            exists: false,
            text: String::new(),
            statement: String::new(),
            body: String::new(),
        });
    }
    let text = fs::read_to_string(&path).map_err(|error| RootsError::error(error.to_string()))?;
    let parsed = parse_prose(&text);
    Ok(NodeProse {
        path: Some(path),
        exists: true,
        text,
        statement: parsed.statement,
        body: parsed.body,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn prose_statement_and_hash_match_reference_rules() {
        assert_eq!(
            parse_prose("\n  A statement  \n\nBody\n\n").statement,
            "A statement"
        );
        assert_eq!(parse_prose("\n  A statement  \n\nBody\n\n").body, "Body");
        assert_eq!(
            content_hash("abc"),
            "sha256:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(dir_hex("a1b2-name"), Some("a1b2"));
        assert_eq!(dir_slug("a1b2-name"), "name");
    }
}
