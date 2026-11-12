//! Filesystem boundary between human-written intent and agent-owned material.
//!
//! These checks deliberately guard accidental mixing and symlink redirection;
//! they are not a substitute for the Luuna runtime's authorization policy.

use crate::errors::{RootsError, RootsResult};
use crate::paths::RootsPaths;
use std::ffi::OsStr;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Component, Path, PathBuf};

fn lexical_normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    let absolute = path.is_absolute();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => out.push(prefix.as_os_str()),
            Component::RootDir => out.push(Path::new(std::path::MAIN_SEPARATOR_STR)),
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() && !absolute {
                    out.push("..");
                }
            }
            Component::Normal(part) => out.push(part),
        }
    }
    out
}

fn absolute(path: &Path) -> PathBuf {
    if path.is_absolute() {
        lexical_normalize(path)
    } else {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        lexical_normalize(&cwd.join(path))
    }
}

fn absolute_from(base: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        absolute(path)
    } else {
        lexical_normalize(&absolute(base).join(path))
    }
}

fn component_eq(left: &OsStr, right: &OsStr) -> bool {
    #[cfg(windows)]
    {
        left.to_string_lossy()
            .eq_ignore_ascii_case(&right.to_string_lossy())
    }
    #[cfg(not(windows))]
    {
        left == right
    }
}

fn components_equal(left: Component<'_>, right: Component<'_>) -> bool {
    match (left, right) {
        (Component::Prefix(left), Component::Prefix(right)) => {
            component_eq(left.as_os_str(), right.as_os_str())
        }
        (Component::RootDir, Component::RootDir) | (Component::CurDir, Component::CurDir) => true,
        (Component::ParentDir, Component::ParentDir) => true,
        (Component::Normal(left), Component::Normal(right)) => component_eq(left, right),
        _ => false,
    }
}

fn path_eq(left: &Path, right: &Path) -> bool {
    let left = absolute(left);
    let right = absolute(right);
    let mut left_parts = left.components();
    let mut right_parts = right.components();
    loop {
        match (left_parts.next(), right_parts.next()) {
            (None, None) => return true,
            (Some(left), Some(right)) if components_equal(left, right) => {}
            _ => return false,
        }
    }
}

/// True when `p` is `parent` or is lexically nested under it.
#[must_use]
pub fn is_inside(parent: impl AsRef<Path>, p: impl AsRef<Path>) -> bool {
    let parent = absolute(parent.as_ref());
    let target = absolute(p.as_ref());
    let parent_parts = parent.components();
    let mut target_parts = target.components();
    for parent_part in parent_parts {
        let Some(target_part) = target_parts.next() else {
            return false;
        };
        if !components_equal(parent_part, target_part) {
            return false;
        }
    }
    true
}

/// Canonicalize an existing path, otherwise return its absolute lexical form.
#[must_use]
pub fn real_or_self(path: impl AsRef<Path>) -> PathBuf {
    fs::canonicalize(path.as_ref()).unwrap_or_else(|_| absolute(path.as_ref()))
}

/// The canonical deepest existing ancestor of a target, if an ancestor exists.
#[must_use]
pub fn real_ancestor(path: impl AsRef<Path>) -> Option<PathBuf> {
    let mut current = absolute(path.as_ref());
    loop {
        if current.exists() {
            return Some(real_or_self(&current));
        }
        let parent = current.parent()?.to_path_buf();
        if parent == current {
            return None;
        }
        current = parent;
    }
}

fn is_symlink(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_symlink())
}

/// Whether a path is inside human-owned intent lexically or after symlink
/// resolution.  Relative inputs are interpreted from the project root.
#[must_use]
pub fn is_human_path(paths: &RootsPaths, path: impl AsRef<Path>) -> bool {
    let absolute = absolute_from(&paths.root, path.as_ref());
    if is_inside(&paths.human, &absolute) {
        return true;
    }
    real_ancestor(&absolute).is_some_and(|real| is_inside(real_or_self(&paths.human), real))
}

fn relative_from(root: &Path, target: &Path) -> PathBuf {
    let root = absolute(root);
    let target = absolute(target);
    let mut root_parts = root.components();
    let mut target_parts = target.components();
    loop {
        let checkpoint = target_parts.clone();
        match (root_parts.next(), target_parts.next()) {
            (Some(left), Some(right)) if components_equal(left, right) => {}
            (None, _) => return checkpoint.as_path().to_path_buf(),
            _ => return target,
        }
    }
}

fn tail_from(parent: &Path, target: &Path) -> Option<Vec<PathBuf>> {
    let parent = absolute(parent);
    let target = absolute(target);
    let parent_parts = parent.components();
    let mut target_parts = target.components();
    for parent_part in parent_parts {
        let target_part = target_parts.next()?;
        if !components_equal(parent_part, target_part) {
            return None;
        }
    }
    Some(
        target_parts
            .map(|part| PathBuf::from(part.as_os_str()))
            .collect(),
    )
}

fn refusal(target: &Path, why: &str) -> RootsError {
    RootsError::guard(format!(
        "refusing to write {}: {why} (agents write only under .luuna/roots/agent/)",
        target.display()
    ))
}

/// Ensure an agent write stays exclusively under `.luuna/roots/agent/`.
pub fn assert_agent_writable(paths: &RootsPaths, target: impl AsRef<Path>) -> RootsResult<()> {
    let target = target.as_ref();
    // The JavaScript implementation resolves a write target against process
    // cwd, not project root.  Keep that behavior for callers that construct
    // absolute paths before invoking this boundary.
    let target_absolute = absolute(target);
    let agent = absolute(&paths.agent);
    if is_inside(&paths.human, &target_absolute) {
        return Err(refusal(target, "it is inside .luuna/roots/human/"));
    }
    if !is_inside(&agent, &target_absolute) || path_eq(&target_absolute, &agent) {
        return Err(refusal(target, "it is outside .luuna/roots/agent/"));
    }

    if is_symlink(&agent) {
        return Err(refusal(target, ".luuna/roots/agent is a symlink"));
    }
    let Some(parts) = tail_from(&agent, &target_absolute) else {
        return Err(refusal(target, "it is outside .luuna/roots/agent/"));
    };
    let mut current = agent.clone();
    for part in parts {
        current.push(part);
        if is_symlink(&current) {
            return Err(refusal(
                target,
                &format!(
                    "{} is a symlink",
                    relative_from(&paths.root, &current).display()
                ),
            ));
        }
    }

    let real = real_ancestor(&target_absolute);
    let Some(real) = real else {
        return Err(refusal(target, "it resolves outside .luuna/roots/agent/"));
    };
    if !is_inside(real_or_self(&agent), &real) {
        return Err(refusal(target, "it resolves outside .luuna/roots/agent/"));
    }
    if is_inside(real_or_self(&paths.human), &real) {
        return Err(refusal(target, "it resolves into .luuna/roots/human/"));
    }
    Ok(())
}

/// Create an agent-owned directory after checking the boundary before and
/// after the recursive mkdir operation.
pub fn agent_mkdir(paths: &RootsPaths, dir: impl AsRef<Path>) -> RootsResult<()> {
    let dir = dir.as_ref();
    assert_agent_writable(paths, dir)?;
    fs::create_dir_all(dir).map_err(|error| {
        RootsError::error(format!("failed to create {}: {error}", dir.display()))
    })?;
    assert_agent_writable(paths, dir)
}

/// Exclusively create a new agent-owned file.  The final component is checked
/// for a symlink and opened with `create_new`, so it never overwrites or follows
/// an existing link.
pub fn agent_write_exclusive(
    paths: &RootsPaths,
    file: impl AsRef<Path>,
    data: impl AsRef<[u8]>,
) -> RootsResult<()> {
    let file = file.as_ref();
    let parent = file.parent().ok_or_else(|| {
        RootsError::guard(format!(
            "refusing to write {}: it is outside .luuna/roots/agent/ (agents write only under .luuna/roots/agent/)",
            file.display()
        ))
    })?;
    agent_mkdir(paths, parent)?;
    assert_agent_writable(paths, file)?;
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(file)
        .map_err(|error| {
            RootsError::error(format!("failed to write {}: {error}", file.display()))
        })?;
    output
        .write_all(data.as_ref())
        .map_err(|error| RootsError::error(format!("failed to write {}: {error}", file.display())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::roots_paths;

    fn sandbox(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "roots-boundary-{name}-{}",
            crate::ids::random_hex(8)
        ));
        fs::create_dir_all(root.join(".luuna/roots/human")).expect("create human directory");
        fs::create_dir_all(root.join(".luuna/roots/agent/notes")).expect("create agent directory");
        root
    }

    #[test]
    fn accepts_only_nested_agent_targets() {
        let root = sandbox("nested");
        let paths = roots_paths(&root);
        assert!(assert_agent_writable(&paths, paths.notes.join("a1b2/x.md")).is_ok());
        assert!(assert_agent_writable(&paths, paths.human.join("x.md")).is_err());
        assert!(assert_agent_writable(&paths, paths.agent.join("../graph.jsonl")).is_err());
        assert!(assert_agent_writable(&paths, &paths.agent).is_err());
        fs::remove_dir_all(root).expect("clean test root");
    }

    #[test]
    fn exclusive_writes_do_not_overwrite() {
        let root = sandbox("exclusive");
        let paths = roots_paths(&root);
        let target = paths.notes.join("a1b2/note.md");
        agent_write_exclusive(&paths, &target, b"first").expect("first write");
        assert!(agent_write_exclusive(&paths, &target, b"second").is_err());
        assert_eq!(fs::read(&target).expect("read target"), b"first");
        fs::remove_dir_all(root).expect("clean test root");
    }

    #[test]
    fn detects_human_paths_lexically() {
        let root = sandbox("human");
        let paths = roots_paths(&root);
        assert!(is_human_path(&paths, ".luuna/roots/human/a/idea.md"));
        assert!(!is_human_path(&paths, ".luuna/roots/agent/notes/a/x.md"));
        fs::remove_dir_all(root).expect("clean test root");
    }

    #[test]
    fn refuses_an_agent_tree_symlink_when_the_platform_allows_one() {
        let root = sandbox("symlink");
        let paths = roots_paths(&root);
        let redirect = paths.notes.join("redirect");
        let created = {
            #[cfg(unix)]
            {
                std::os::unix::fs::symlink(&paths.human, &redirect).is_ok()
            }
            #[cfg(windows)]
            {
                std::os::windows::fs::symlink_dir(&paths.human, &redirect).is_ok()
            }
            #[cfg(not(any(unix, windows)))]
            {
                false
            }
        };
        if created {
            assert!(assert_agent_writable(&paths, redirect.join("x.md")).is_err());
        }
        fs::remove_dir_all(root).expect("clean test root");
    }
}
