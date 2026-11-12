//! Go's lexical path functions (`path/filepath`: `Clean`, `Dir`, `Base`,
//! `Abs`), which the Go tool uses for store discovery and in its output.
//!
//! They work on the path text only and never touch the file system, so a
//! symbolic link is not resolved, as in Go.

use std::ffi::OsStr;
use std::path::{Component, Path, PathBuf};

/// Go's `filepath.Clean`: repeated separators, `.` elements and `..` after
/// a normal element are removed lexically; an empty result is `.`.
pub(crate) fn clean(path: &Path) -> PathBuf {
    let mut prefix = PathBuf::new();
    let mut rooted = false;
    let mut parts: Vec<&OsStr> = Vec::new();
    // How many leading `..` elements a relative path keeps.
    let mut kept_parents = 0;
    for component in path.components() {
        match component {
            Component::Prefix(value) => prefix.push(value.as_os_str()),
            Component::RootDir => rooted = true,
            Component::CurDir => {}
            Component::ParentDir => {
                if parts.len() > kept_parents {
                    parts.pop();
                } else if !rooted {
                    parts.push(OsStr::new(".."));
                    kept_parents += 1;
                }
            }
            Component::Normal(name) => parts.push(name),
        }
    }
    let mut out = prefix;
    if rooted {
        out.push(Component::RootDir.as_os_str());
    }
    for part in parts {
        out.push(part);
    }
    if out.as_os_str().is_empty() {
        out.push(".");
    }
    out
}

/// Go's `filepath.Dir`: everything but the last element, cleaned. The
/// directory of the root is the root, and of a single element is `.`.
pub(crate) fn dir(path: &Path) -> PathBuf {
    let cleaned = clean(path);
    match cleaned.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
        Some(_) => PathBuf::from("."),
        None => cleaned,
    }
}

/// Go's `filepath.Base` of a cleaned path: the last element, the root for
/// the root, and `.` for an empty path.
pub(crate) fn base(path: &Path) -> PathBuf {
    let cleaned = clean(path);
    match cleaned.file_name() {
        Some(name) => PathBuf::from(name),
        None => cleaned,
    }
}

/// Go's `filepath.Abs`: a relative path is joined to the process's working
/// directory; the result is cleaned.
pub(crate) fn abs(path: &Path) -> std::io::Result<PathBuf> {
    if path.is_absolute() {
        return Ok(clean(path));
    }
    Ok(clean(&std::env::current_dir()?.join(path)))
}

/// The bytes Go would print for `path`. On Unix a path is raw bytes, as a
/// Go string is; elsewhere it is converted lossily.
pub(crate) fn bytes(path: &Path) -> Vec<u8> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        path.as_os_str().as_bytes().to_vec()
    }
    #[cfg(not(unix))]
    {
        path.to_string_lossy().into_owned().into_bytes()
    }
}

/// `path` as text for JSON output. Go's encoder replaces invalid UTF-8 with
/// U+FFFD; this does the same for each invalid sequence.
pub(crate) fn text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cleaned(path: &str) -> String {
        clean(Path::new(path)).to_string_lossy().into_owned()
    }

    #[test]
    fn clean_follows_go() {
        assert_eq!(cleaned(""), ".");
        assert_eq!(cleaned("/"), "/");
        assert_eq!(cleaned("//a//b/./c/"), "/a/b/c");
        assert_eq!(cleaned("/a/b/../c"), "/a/c");
        assert_eq!(cleaned("/../a"), "/a");
        assert_eq!(cleaned("a/../.."), "..");
        assert_eq!(cleaned("../../a/../b"), "../../b");
        assert_eq!(cleaned("./a"), "a");
        assert_eq!(cleaned("a/.."), ".");
    }

    #[test]
    fn dir_and_base_follow_go() {
        let dir_of = |path: &str| dir(Path::new(path)).to_string_lossy().into_owned();
        let base_of = |path: &str| base(Path::new(path)).to_string_lossy().into_owned();
        assert_eq!(dir_of("/a/b/.luuna/missions"), "/a/b/.luuna");
        assert_eq!(dir_of("/a"), "/");
        assert_eq!(dir_of("/"), "/");
        assert_eq!(dir_of("a"), ".");
        assert_eq!(dir_of("/a/b/"), "/a");
        assert_eq!(base_of("/a/project/"), "project");
        assert_eq!(base_of("/"), "/");
        assert_eq!(base_of(""), ".");
    }
}
