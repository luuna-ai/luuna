//! Store locations under a project root and the legacy `.canopy` move
//! (`internal/paths/paths.go`). The root is always a parameter; nothing here
//! reads the process's working directory.

use crate::error::BeaconError;
use std::fs;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};

/// `<root>/.luuna/beacon`.
pub(crate) fn store_dir(root: &Path) -> PathBuf {
    root.join(".luuna").join("beacon")
}

/// `<root>/.luuna/beacon/config.yaml`.
pub(crate) fn config_file(root: &Path) -> PathBuf {
    store_dir(root).join("config.yaml")
}

/// `<root>/.luuna/beacon/prompts.jsonl`.
pub(crate) fn prompts_file(root: &Path) -> PathBuf {
    store_dir(root).join("prompts.jsonl")
}

/// `<root>/.luuna/beacon/schemas.jsonl`.
pub(crate) fn schemas_file(root: &Path) -> PathBuf {
    store_dir(root).join("schemas.jsonl")
}

/// `<root>/.luuna/beacon/artifacts.jsonl`.
pub(crate) fn artifacts_file(root: &Path) -> PathBuf {
    store_dir(root).join("artifacts.jsonl")
}

/// Go's `os.WriteFile(path, content, 0o644)`: create or truncate, then
/// write. The mode applies only to a new file.
pub(crate) fn write_file(path: &Path, content: &[u8]) -> Result<(), BeaconError> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o644);
    }
    let mut file = options
        .open(path)
        .map_err(|error| BeaconError::os("open", path, error))?;
    file.write_all(content)
        .map_err(|error| BeaconError::os("write", path, error))
}

/// Go's `os.MkdirAll(path, 0o755)`.
pub(crate) fn make_dirs(path: &Path) -> Result<(), BeaconError> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o755);
    }
    builder
        .create(path)
        .map_err(|error| BeaconError::os("mkdir", path, error))
}

/// Moves a legacy `<root>/.canopy` store to `<root>/.luuna/beacon` exactly as
/// `paths.MigrateLegacy` does, without printing: returns `true` where Go
/// prints `Migrated .canopy to .luuna/beacon`.
///
/// Nothing happens when `.canopy` does not exist. It is refused when
/// `.canopy` is a symbolic link, when `.luuna/beacon` exists (followed
/// through links), or when `.luuna` is a symbolic link. After the move,
/// `.canopy/` is replaced with `.luuna/beacon/` in `<root>/.gitattributes`
/// and `<root>/.gitignore`; a file that cannot be read is skipped and an
/// unchanged file is not written.
pub(crate) fn migrate_legacy(root: &Path) -> Result<bool, BeaconError> {
    let legacy = root.join(".canopy");
    let canonical = store_dir(root);
    let legacy_info = match fs::symlink_metadata(&legacy) {
        Ok(info) => info,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(BeaconError::os("lstat", &legacy, error)),
    };
    if legacy_info.file_type().is_symlink() {
        return Err(BeaconError::SymlinkedLegacy);
    }
    if fs::metadata(&canonical).is_ok() {
        return Err(BeaconError::BothLayouts);
    }
    let luuna = root.join(".luuna");
    if let Ok(info) = fs::symlink_metadata(&luuna) {
        if info.file_type().is_symlink() {
            return Err(BeaconError::SymlinkedLuuna);
        }
    }
    make_dirs(&luuna)?;
    fs::rename(&legacy, &canonical).map_err(|source| BeaconError::Rename {
        from: legacy.clone(),
        to: canonical.clone(),
        source,
    })?;
    for name in [".gitattributes", ".gitignore"] {
        let path = root.join(name);
        let Ok(content) = fs::read(&path) else {
            continue;
        };
        let next = replace_all(&content, b".canopy/", b".luuna/beacon/");
        if next != content {
            write_file(&path, &next)?;
        }
    }
    Ok(true)
}

/// `strings.ReplaceAll` on bytes (the content need not be UTF-8).
fn replace_all(content: &[u8], from: &[u8], to: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(content.len());
    let mut rest = content;
    while let Some(at) = rest.windows(from.len()).position(|window| window == from) {
        out.extend_from_slice(&rest[..at]);
        out.extend_from_slice(to);
        rest = &rest[at + from.len()..];
    }
    out.extend_from_slice(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh_root() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().expect("temporary folder");
        let path = dir.path().canonicalize().expect("canonical path");
        (dir, path)
    }

    #[test]
    fn store_paths_are_under_the_root() {
        let base = Path::new("/project");
        assert_eq!(store_dir(base), Path::new("/project/.luuna/beacon"));
        assert_eq!(
            config_file(base),
            Path::new("/project/.luuna/beacon/config.yaml")
        );
        assert_eq!(
            prompts_file(base),
            Path::new("/project/.luuna/beacon/prompts.jsonl")
        );
        assert_eq!(
            schemas_file(base),
            Path::new("/project/.luuna/beacon/schemas.jsonl")
        );
        assert_eq!(
            artifacts_file(base),
            Path::new("/project/.luuna/beacon/artifacts.jsonl")
        );
    }

    #[test]
    fn legacy_canopy_is_moved_and_references_rewritten() {
        let (_dir, root) = fresh_root();
        // Nothing to move: nothing happens.
        assert!(!migrate_legacy(&root).unwrap());
        assert!(!root.join(".luuna").exists());

        fs::create_dir(root.join(".canopy")).unwrap();
        fs::write(root.join(".canopy/prompts.jsonl"), b"{\"id\":\"a\"}\n").unwrap();
        fs::write(root.join(".canopy/config.yaml"), b"project: old\n").unwrap();
        fs::write(
            root.join(".gitattributes"),
            b".canopy/prompts.jsonl merge=union\n.canopy/schemas.jsonl merge=union",
        )
        .unwrap();
        fs::write(root.join(".gitignore"), b".canopy/*.lock\nbuild/\n\xff").unwrap();
        fs::write(root.join("README"), b".canopy/ stays here\n").unwrap();

        assert!(migrate_legacy(&root).unwrap());
        assert!(!root.join(".canopy").exists());
        assert_eq!(
            fs::read(root.join(".luuna/beacon/prompts.jsonl")).unwrap(),
            b"{\"id\":\"a\"}\n"
        );
        assert_eq!(
            fs::read(root.join(".luuna/beacon/config.yaml")).unwrap(),
            b"project: old\n"
        );
        assert_eq!(
            fs::read(root.join(".gitattributes")).unwrap(),
            b".luuna/beacon/prompts.jsonl merge=union\n.luuna/beacon/schemas.jsonl merge=union"
        );
        assert_eq!(
            fs::read(root.join(".gitignore")).unwrap(),
            b".luuna/beacon/*.lock\nbuild/\n\xff"
        );
        assert_eq!(
            fs::read(root.join("README")).unwrap(),
            b".canopy/ stays here\n"
        );
        // A second run has nothing left to move.
        assert!(!migrate_legacy(&root).unwrap());

        // Without .gitattributes or .gitignore the move still succeeds and
        // creates neither.
        let (_dir, root) = fresh_root();
        fs::create_dir(root.join(".canopy")).unwrap();
        assert!(migrate_legacy(&root).unwrap());
        assert!(root.join(".luuna/beacon").is_dir());
        assert!(!root.join(".gitattributes").exists());
        assert!(!root.join(".gitignore").exists());
    }

    #[test]
    fn both_layouts_present_is_refused() {
        let (_dir, root) = fresh_root();
        fs::create_dir(root.join(".canopy")).unwrap();
        fs::write(root.join(".canopy/config.yaml"), b"project: old\n").unwrap();
        fs::create_dir_all(root.join(".luuna/beacon")).unwrap();
        fs::write(root.join(".gitignore"), b".canopy/\n").unwrap();
        let error = migrate_legacy(&root).unwrap_err();
        assert_eq!(
            error.to_string(),
            "both .canopy and .luuna/beacon exist; merge or remove one before continuing"
        );
        // Nothing moved or rewritten.
        assert!(root.join(".canopy/config.yaml").exists());
        assert_eq!(fs::read(root.join(".gitignore")).unwrap(), b".canopy/\n");
        // A plain file at .luuna/beacon counts as present too.
        let (_dir, root) = fresh_root();
        fs::create_dir(root.join(".canopy")).unwrap();
        fs::create_dir(root.join(".luuna")).unwrap();
        fs::write(root.join(".luuna/beacon"), b"").unwrap();
        assert!(matches!(
            migrate_legacy(&root),
            Err(BeaconError::BothLayouts)
        ));
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_legacy_directory_is_refused() {
        use std::os::unix::fs::symlink;
        let (_dir, root) = fresh_root();
        fs::create_dir(root.join("elsewhere")).unwrap();
        symlink("elsewhere", root.join(".canopy")).unwrap();
        let error = migrate_legacy(&root).unwrap_err();
        assert_eq!(
            error.to_string(),
            "refusing to migrate symlinked legacy directory"
        );
        assert!(root.join(".canopy").exists());
        assert!(!root.join(".luuna").exists());
        // A dangling link is refused the same way.
        let (_dir, root) = fresh_root();
        symlink("missing", root.join(".canopy")).unwrap();
        assert!(matches!(
            migrate_legacy(&root),
            Err(BeaconError::SymlinkedLegacy)
        ));
        // A symlinked .luuna is refused too, and nothing is moved.
        let (_dir, root) = fresh_root();
        fs::create_dir(root.join(".canopy")).unwrap();
        fs::create_dir(root.join("target")).unwrap();
        symlink("target", root.join(".luuna")).unwrap();
        let error = migrate_legacy(&root).unwrap_err();
        assert_eq!(
            error.to_string(),
            "refusing to migrate into symlinked .luuna directory"
        );
        assert!(root.join(".canopy").is_dir());
    }

    #[test]
    fn replaces_every_occurrence_bytewise() {
        assert_eq!(replace_all(b"", b".canopy/", b"x"), b"");
        assert_eq!(
            replace_all(b".canopy/.canopy/a.canopy", b".canopy/", b"X/"),
            b"X/X/a.canopy"
        );
    }
}
