//! Location and names of the persistent `.luuna/roots/` store.

use crate::errors::{RootsError, RootsResult};
use crate::model::NodeKind;
use std::fs;
use std::path::{Path, PathBuf};

pub const ROOTS_DIR: &str = ".luuna/roots";

/// Native path construction for the store root.
///
/// `ROOTS_DIR` remains a slash-delimited public compatibility string, but
/// passing that string straight to `Path::join` leaves a mixed separator in
/// Windows display/error paths. Node's `path.join(root, ROOTS_DIR)` normalizes
/// the components, so keep the two directory segments separate here.
fn roots_dir(root: &Path) -> PathBuf {
    root.join(".luuna").join("roots")
}

/// Absolute paths belonging to one Roots project.
///
/// Field names mirror the TypeScript `RootsPaths` object.  Keep the names and
/// layout stable: command handlers and hook guards address these paths
/// directly, and old `.roots` directories are intentionally not consulted.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RootsPaths {
    pub root: PathBuf,
    pub dir: PathBuf,
    pub config: PathBuf,
    pub graph: PathBuf,
    pub proposals: PathBuf,
    pub questions: PathBuf,
    pub events: PathBuf,
    pub headings: PathBuf,
    pub gitignore: PathBuf,
    pub human: PathBuf,
    pub agent: PathBuf,
    pub sprouts: PathBuf,
    pub notes: PathBuf,
}

fn absolute(path: &Path) -> PathBuf {
    // Match Node's `resolve`: make a lexical absolute path without resolving
    // symlinks or adding Windows extended-path prefixes.
    let source = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(path)
    };
    let mut normalized = PathBuf::new();
    for component in source.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                let _ = normalized.pop();
            }
            std::path::Component::Normal(part) => normalized.push(part),
            std::path::Component::RootDir | std::path::Component::Prefix(_) => {
                normalized.push(component.as_os_str());
            }
        }
    }
    normalized
}

#[must_use]
pub fn roots_paths(project_root: impl AsRef<Path>) -> RootsPaths {
    let root = absolute(project_root.as_ref());
    let dir = roots_dir(&root);
    let human = dir.join("human");
    let agent = dir.join("agent");
    RootsPaths {
        root,
        config: dir.join("config.yaml"),
        graph: dir.join("graph.jsonl"),
        proposals: dir.join("proposals.jsonl"),
        questions: dir.join("questions.jsonl"),
        events: dir.join("events.jsonl"),
        headings: dir.join("headings.jsonl"),
        gitignore: dir.join(".gitignore"),
        sprouts: agent.join("sprouts"),
        notes: agent.join("notes"),
        human,
        agent,
        dir,
    }
}

fn is_dir(path: &Path) -> bool {
    fs::metadata(path).is_ok_and(|metadata| metadata.is_dir())
}

/// Walk upward from `start` to the nearest directory with a configured Roots
/// store.  A directory without `config.yaml` is not an initialized project.
#[must_use]
pub fn find_project_root(start: impl AsRef<Path>) -> Option<PathBuf> {
    let mut current = absolute(start.as_ref());
    loop {
        let roots = roots_dir(&current);
        if is_dir(&roots) && roots.join("config.yaml").is_file() {
            return Some(current);
        }
        let parent = current.parent()?;
        if parent == current {
            return None;
        }
        current = parent.to_path_buf();
    }
}

pub fn require_roots_paths(cwd: impl AsRef<Path>) -> RootsResult<RootsPaths> {
    let cwd = absolute(cwd.as_ref());
    find_project_root(&cwd)
        .map(roots_paths)
        .ok_or_else(|| RootsError::not_initialized(cwd.display()))
}

#[must_use]
pub fn kind_dir(paths: &RootsPaths, kind: NodeKind) -> &Path {
    match kind {
        NodeKind::Idea => &paths.human,
        NodeKind::Sprout => &paths.sprouts,
    }
}

#[must_use]
pub const fn prose_file_name(kind: NodeKind) -> &'static str {
    match kind {
        NodeKind::Idea => "idea.md",
        NodeKind::Sprout => "sprout.md",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn sandbox(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "roots-core-paths-{name}-{}",
            crate::ids::random_hex(8)
        ));
        fs::create_dir_all(&path).expect("create test root");
        path
    }

    #[test]
    fn finds_the_nearest_initialized_root() {
        let root = sandbox("find");
        let nested = root.join("a").join("b");
        let roots = root.join(".luuna").join("roots");
        fs::create_dir_all(&roots).expect("create roots");
        fs::write(roots.join("config.yaml"), "project: test\n").expect("write config");
        fs::create_dir_all(&nested).expect("create nested");

        assert_eq!(find_project_root(&nested).as_deref(), Some(root.as_path()));
        assert_eq!(
            roots_paths(&root).sprouts,
            root.join(".luuna")
                .join("roots")
                .join("agent")
                .join("sprouts")
        );
        fs::remove_dir_all(root).expect("clean test root");
    }

    #[cfg(windows)]
    #[test]
    fn paths_use_native_separators_for_the_store_root() {
        let root = sandbox("native-separators");
        let paths = roots_paths(&root);

        assert_eq!(paths.dir, root.join(".luuna").join("roots"));
        assert!(!paths.dir.display().to_string().contains(".luuna/roots"));

        fs::remove_dir_all(root).expect("clean test root");
    }
}
