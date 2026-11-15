//! filepath.Join retains the first component's root, unlike PathBuf::push.
use std::path::{Path, PathBuf};
pub(super) fn join(parts: &[&str]) -> String {
    let raw = parts
        .iter()
        .filter(|s| !s.is_empty())
        .copied()
        .collect::<Vec<_>>()
        .join("/");
    if raw.is_empty() {
        return String::new();
    }
    let absolute = raw.starts_with('/');
    let mut stack = Vec::new();
    for part in raw.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                if stack.last().is_some_and(|p| *p != "..") {
                    stack.pop();
                } else if !absolute {
                    stack.push(part)
                }
            }
            _ => stack.push(part),
        }
    }
    let text = stack.join("/");
    if absolute {
        format!("/{text}")
    } else if text.is_empty() {
        ".".into()
    } else {
        text
    }
}
pub(super) fn canonical_future(path: &Path) -> std::io::Result<PathBuf> {
    let mut ancestor = path;
    let mut missing = Vec::new();
    loop {
        match std::fs::canonicalize(ancestor) {
            Ok(mut result) => {
                for p in missing.into_iter().rev() {
                    result.push(p)
                }
                return Ok(result);
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let Some(name) = ancestor.file_name() else {
                    return Err(e);
                };
                missing.push(name.to_os_string());
                let Some(parent) = ancestor.parent() else {
                    return Err(e);
                };
                ancestor = parent
            }
            Err(e) => return Err(e),
        }
    }
}
