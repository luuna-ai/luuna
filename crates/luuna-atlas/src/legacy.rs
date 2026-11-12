//! The one-way migration of a legacy `.mulch/` store to `.luuna/atlas/`.
//!
//! This follows `MigrateLegacyStore` in the Go tool's `legacy_layout.go`, step by step,
//! including the order of the checks and the wording of the refusals.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::Path;

use crate::error::{AtlasError, Result};

/// The legacy store directory.
pub const LEGACY_STORE_DIR: &str = ".mulch";

/// The legacy config file name inside the legacy store.
pub const LEGACY_CONFIG_FILE: &str = "mulch.config.yaml";

/// The line the Go tool prints on stderr after a migration (without its newline).
const NOTICE: &str =
    "luuna-atlas: moved legacy .mulch/ to .luuna/atlas/ (mulch.config.yaml is now config.yaml)";

/// The `errno` for "not a directory", which Go's `MkdirAll` reports when a path
/// component is a file.
const ENOTDIR: i32 = 20;

/// Moves `<project_root>/.mulch/` to `<project_root>/.luuna/atlas/` exactly as the Go
/// tool does. Returns the notice the Go tool prints on stderr when it migrated (without
/// the trailing newline), and `None` when there was nothing to migrate.
///
/// Refuses, with the Go tool's messages, when `.mulch` is a symbolic link, when `.luuna`
/// is a symbolic link, or when `.luuna/atlas` already exists. A `.mulch` that is a plain
/// file is left alone.
pub fn migrate_legacy_store(project_root: &Path) -> Result<Option<String>> {
    let old = project_root.join(LEGACY_STORE_DIR);
    let old_stat = match fs::symlink_metadata(&old) {
        Ok(stat) => stat,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(AtlasError::PathOp {
                op: "lstat",
                path: old,
                source,
            })
        }
    };
    let luuna = project_root.join(".luuna");
    let target = luuna.join("atlas");
    if old_stat.file_type().is_symlink() {
        return Err(AtlasError::Message(
            "Refusing to migrate .mulch: it is a symbolic link. Move its contents to .luuna/atlas manually."
                .to_owned(),
        ));
    }
    if !old_stat.is_dir() {
        return Ok(None);
    }
    if fs::symlink_metadata(&luuna).is_ok_and(|stat| stat.file_type().is_symlink()) {
        return Err(AtlasError::Message(
            "Refusing to migrate .mulch: .luuna is a symbolic link. Replace it with a real directory first."
                .to_owned(),
        ));
    }
    if lexists(&target) {
        return Err(AtlasError::Message(
            "Both .mulch/ and .luuna/atlas/ exist. Resolve this by hand: keep .luuna/atlas/ and remove or archive .mulch/ (or the reverse), then retry."
                .to_owned(),
        ));
    }
    make_dir_all(&luuna)?;
    fs::rename(&old, &target).map_err(|source| AtlasError::Rename {
        from: old.clone(),
        to: target.clone(),
        source,
    })?;
    let old_config = target.join(LEGACY_CONFIG_FILE);
    let new_config = target.join("config.yaml");
    if lexists(&old_config) && !lexists(&new_config) {
        fs::rename(&old_config, &new_config).map_err(|source| AtlasError::Rename {
            from: old_config.clone(),
            to: new_config.clone(),
            source,
        })?;
    }
    for name in [".gitattributes", ".gitignore"] {
        let path = project_root.join(name);
        let Ok(bytes) = fs::read(&path) else {
            continue;
        };
        let next = rewrite_legacy_paths(&bytes);
        if next != bytes {
            write_file(&path, &next)?;
        }
    }
    Ok(Some(NOTICE.to_owned()))
}

/// Go's `os.Lstat(path) == nil`.
fn lexists(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok()
}

/// Go's `os.MkdirAll(path, 0777)`.
fn make_dir_all(path: &Path) -> Result<()> {
    match fs::metadata(path) {
        Ok(stat) if stat.is_dir() => return Ok(()),
        Ok(_) => {
            return Err(AtlasError::PathOp {
                op: "mkdir",
                path: path.to_path_buf(),
                source: io::Error::from_raw_os_error(ENOTDIR),
            })
        }
        Err(_) => {}
    }
    fs::create_dir_all(path).map_err(|source| AtlasError::PathOp {
        op: "mkdir",
        path: path.to_path_buf(),
        source,
    })
}

/// Go's `os.WriteFile(path, data, 0666)`.
fn write_file(path: &Path, data: &[u8]) -> Result<()> {
    let op = |op: &'static str| {
        move |source| AtlasError::PathOp {
            op,
            path: path.to_path_buf(),
            source,
        }
    };
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)
        .map_err(op("open"))?;
    file.write_all(data).map_err(op("write"))
}

/// Go's `\s` (RE2): space, tab, newline, form feed, carriage return.
fn is_space(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\n' | 0x0c | b'\r')
}

/// `regexp.MustCompile("(?m)^(\\s*/?)\\.mulch/").ReplaceAllString(text, "${1}.luuna/atlas/")`.
///
/// An occurrence of `.mulch/` is replaced when, before it, an optional `/` is preceded
/// by a run of white space that starts at the start of the text or contains a newline
/// (the run is allowed to span lines, as `\s` matches a newline).
fn rewrite_legacy_paths(text: &[u8]) -> Vec<u8> {
    const OLD: &[u8] = b".mulch/";
    const NEW: &[u8] = b".luuna/atlas/";
    let mut out = Vec::with_capacity(text.len());
    let mut i = 0;
    // The end of the last replacement: matches do not overlap.
    let mut floor = 0;
    while i < text.len() {
        if text[i..].starts_with(OLD) && i >= floor && anchored(text, i, floor) {
            out.extend_from_slice(NEW);
            i += OLD.len();
            floor = i;
        } else {
            out.push(text[i]);
            i += 1;
        }
    }
    out
}

/// Whether the occurrence at `at` is preceded by `(?m)^\s*/?`, with the match starting
/// no earlier than `floor`.
fn anchored(text: &[u8], at: usize, floor: usize) -> bool {
    let mut end = at;
    if end > floor && text[end - 1] == b'/' {
        end -= 1;
    }
    let mut start = end;
    while start > floor && is_space(text[start - 1]) {
        start -= 1;
    }
    // A line start in [start, end]: the start of the text, or a position after a newline.
    if start == 0 {
        return true;
    }
    text[start..end].contains(&b'\n')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rewrite(text: &str) -> String {
        String::from_utf8(rewrite_legacy_paths(text.as_bytes())).unwrap()
    }

    #[test]
    fn rewrites_anchored_occurrences_only() {
        assert_eq!(rewrite(".mulch/a\n"), ".luuna/atlas/a\n");
        assert_eq!(rewrite("x\n  /.mulch/tmp\n"), "x\n  /.luuna/atlas/tmp\n");
        assert_eq!(rewrite("a.mulch/b\n.mulchy/\n"), "a.mulch/b\n.mulchy/\n");
        assert_eq!(rewrite("x .mulch/\n"), "x .mulch/\n");
        assert_eq!(rewrite("x\n\n\t.mulch/\n"), "x\n\n\t.luuna/atlas/\n");
        assert_eq!(rewrite("# .mulch/\n"), "# .mulch/\n");
        assert_eq!(rewrite(".mulch/.mulch/"), ".luuna/atlas/.mulch/");
        assert_eq!(
            rewrite("a\n.mulch/\n.mulch/"),
            "a\n.luuna/atlas/\n.luuna/atlas/"
        );
    }
}
