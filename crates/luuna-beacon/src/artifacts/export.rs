//! `<kind> export` (`exportArtifact`, `cli.go:1206-1318`): a stored version
//! written out as a folder, or as one `.md` file for an agent.
//!
//! Every step copies Go, its defects included (GD51-GD53): the destination
//! is cleaned lexically and only the project root, `.luuna` and paths under
//! `.luuna` are refused; symbolic links are followed where Go's `Stat`,
//! `MkdirAll`, `WriteFile` and `ReadFile` follow them and replaced where
//! `Rename` replaces them. The one refusal Go does not make is RD57: a stored
//! path that leaves the stage onto a name where something already exists.

use super::model::{Artifact, ArtifactFile};
use super::snapshot;
use super::{ArtifactKind, ArtifactsError};
use crate::gojson::{self, GoValue, ObjectWriter};
use crate::paths;
use crate::{Beacon, BeaconError};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

/// The options of `<kind> export`. An empty `out` is an absent `--out`; an
/// empty `target` is an absent `--target`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ArtifactExport {
    pub out: String,
    pub target: String,
    pub force: bool,
}

/// A finished export: the version written and the absolute, cleaned
/// destination.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactExported {
    pub name: String,
    pub version: i64,
    pub path: PathBuf,
}

/// The name of the ownership marker written into a folder export.
const MARKER: &str = ".lbc-artifact.json";

impl Beacon {
    /// `<kind> export <reference> [--out <path>] [--target claude|codex]
    /// [--force]`.
    pub fn artifact_export(
        &self,
        kind: ArtifactKind,
        reference: Option<&str>,
        options: &ArtifactExport,
    ) -> Result<ArtifactExported, BeaconError> {
        let (_, record) = self.artifact_select(kind, reference)?;
        let root = go_clean(&path_bytes(self.project_root()));
        let destination = destination(&root, &record, options)?;
        let data_root = go_join(&root, b".luuna");
        let mut data_prefix = data_root.clone();
        data_prefix.push(b'/');
        if destination == root || destination == data_root || destination.starts_with(&data_prefix)
        {
            return Err(ArtifactsError::RefusingDataDirectory.into());
        }
        let single_agent = record.kind == "agent"
            && destination.len() >= 3
            && destination[destination.len() - 3..].eq_ignore_ascii_case(b".md");
        let destination = bytes_path(destination);
        if single_agent {
            self.export_agent_file(&record, &destination, options.force)?;
        } else {
            self.export_folder(&record, &destination, options.force)?;
        }
        Ok(ArtifactExported {
            name: record.name,
            version: record.version,
            path: destination,
        })
    }

    /// The single-file agent export, `cli.go:1233-1263`. With `--force` any
    /// existing file is replaced, with no ownership check (GD53, DL5).
    fn export_agent_file(
        &self,
        record: &Artifact,
        destination: &Path,
        force: bool,
    ) -> Result<(), BeaconError> {
        if fs::metadata(destination).is_ok() && !force {
            return Err(ArtifactsError::DestinationExists(destination.to_path_buf()).into());
        }
        if record.files.len() != 1 {
            return Err(ArtifactsError::AgentExportNeedsOneFile.into());
        }
        let data = snapshot::decode(&record.files[0])?;
        let bytes = path_bytes(destination);
        let parent = go_dir(&bytes);
        paths::make_dirs(&bytes_path(parent.clone()))?;
        let mut name = b".".to_vec();
        name.extend_from_slice(go_base(&bytes));
        name.extend_from_slice(b".lbc-stage-");
        name.extend_from_slice(self.next_id_hex().as_bytes());
        let stage = bytes_path(go_join(&parent, &name));
        paths::write_file(&stage, &data)?;
        let _stage = Cleanup::Remove(stage.clone());
        let mut backup = stage.clone().into_os_string();
        backup.push(".backup");
        let backup = PathBuf::from(backup);
        if fs::metadata(destination).is_ok() {
            rename(destination, &backup)?;
        }
        if let Err(error) = rename(&stage, destination) {
            let _ = fs::rename(&backup, destination);
            return Err(error);
        }
        go_remove(&backup);
        Ok(())
    }

    /// The folder export, `cli.go:1264-1316`. With `--force` an existing
    /// destination is replaced whole when its marker names this kind and
    /// name (DL4), followed through a link and replacing the link (GD53).
    fn export_folder(
        &self,
        record: &Artifact,
        destination: &Path,
        force: bool,
    ) -> Result<(), BeaconError> {
        let mut backup: Option<PathBuf> = None;
        if fs::metadata(destination).is_ok() {
            if !force {
                return Err(ArtifactsError::DestinationExists(destination.to_path_buf()).into());
            }
            let owned = fs::read(destination.join(MARKER))
                .is_ok_and(|marker| marker_names(&marker, &record.kind, &record.name));
            if !owned {
                return Err(ArtifactsError::NotManaged {
                    kind: record.kind.clone(),
                    name: record.name.clone(),
                }
                .into());
            }
            backup = Some(suffixed(destination, ".lbc-backup-", &self.next_id_hex()));
        }
        let stage = suffixed(destination, ".lbc-stage-", &self.next_id_hex());
        let stage_bytes = path_bytes(&stage);
        // RD57, before anything is created: the only refusal of GD51's escape.
        if let Some(existing) = escape_onto_existing(&stage_bytes, &record.files) {
            return Err(ArtifactsError::ExportWouldOverwrite(existing).into());
        }
        paths::make_dirs(&stage)?;
        let _stage = Cleanup::RemoveAll(stage.clone());
        for file in &record.files {
            let data = snapshot::decode(file)?;
            // GD51 copied: the stored path is joined with no containment
            // check, so `../` may write beside the stage. RD57 above has
            // already refused every such path that names an existing file.
            let path = go_join(&stage_bytes, file.path.as_bytes());
            paths::make_dirs(&bytes_path(go_dir(&path)))?;
            let path = bytes_path(path);
            paths::write_file(&path, &data)?;
            if file.executable {
                make_executable(&path);
            }
        }
        let mut marker = String::new();
        let mut object = ObjectWriter::new(&mut marker);
        object.string("kind", &record.kind);
        object.string("name", &record.name);
        object.finish();
        let _ = paths::write_file(&stage.join(MARKER), marker.as_bytes());
        if let Some(backup) = &backup {
            rename(destination, backup)?;
        }
        if let Err(error) = rename(&stage, destination) {
            if let Some(backup) = &backup {
                let _ = fs::rename(backup, destination);
            }
            return Err(error);
        }
        if let Some(backup) = &backup {
            go_remove_all(backup);
        }
        Ok(())
    }
}

/// The destination before the data-directory check: `--out`, else the
/// default for `--target` and the kind, joined to the root when relative,
/// then cleaned (`cli.go:1207-1224`).
fn destination(
    root: &[u8],
    record: &Artifact,
    options: &ArtifactExport,
) -> Result<Vec<u8>, ArtifactsError> {
    let mut destination = options.out.as_bytes().to_vec();
    if destination.is_empty() {
        let base: &[&[u8]] = match (options.target.as_str(), record.kind.as_str()) {
            ("claude", "skill") => &[b".claude", b"skills"],
            ("codex", "skill") => &[b".agents", b"skills"],
            ("claude", "agent") => &[b".claude", b"agents"],
            _ => return Err(ArtifactsError::OutRequired),
        };
        let mut name = record.name.as_bytes().to_vec();
        if record.kind == "agent" {
            name.extend_from_slice(b".md");
        }
        destination = base
            .iter()
            .fold(root.to_vec(), |path, part| go_join(&path, part));
        destination = go_join(&destination, &name);
    }
    if destination.first() != Some(&b'/') {
        destination = go_join(root, &destination);
    }
    Ok(go_clean(&destination))
}

/// RD57: the first stored path whose lexical join to the stage falls
/// outside it onto a name where something exists (a file, a folder, or a
/// link, dangling or not). An escape onto a free name is left to be written
/// as Go writes it.
fn escape_onto_existing(stage: &[u8], files: &[ArtifactFile]) -> Option<PathBuf> {
    let mut inside = stage.to_vec();
    inside.push(b'/');
    files.iter().find_map(|file| {
        let joined = go_join(stage, file.path.as_bytes());
        if joined == stage || joined.starts_with(&inside) {
            return None;
        }
        let joined = bytes_path(joined);
        fs::symlink_metadata(&joined).is_ok().then_some(joined)
    })
}

/// Whether a marker decodes as Go's `json.Unmarshal` into a
/// `map[string]string` without error and names `kind` and `name`: `null` is
/// an empty map, a `null` value is the empty string, a later repeated key
/// wins, keys match exactly, and any other value type is an error.
fn marker_names(marker: &[u8], kind: &str, name: &str) -> bool {
    let Ok(value) = gojson::parse(marker) else {
        return false;
    };
    let mut owner: HashMap<&str, &str> = HashMap::new();
    match &value {
        GoValue::Null => {}
        GoValue::Object(entries) => {
            for (key, item) in entries {
                match item {
                    GoValue::String(text) => owner.insert(key, text),
                    GoValue::Null => owner.insert(key, ""),
                    _ => return false,
                };
            }
        }
        _ => return false,
    }
    owner.get("kind").copied().unwrap_or("") == kind
        && owner.get("name").copied().unwrap_or("") == name
}

/// `path` with `<label><hex>` appended to its last element.
fn suffixed(path: &Path, label: &str, hex: &str) -> PathBuf {
    let mut text = path.as_os_str().to_owned();
    text.push(label);
    text.push(hex);
    PathBuf::from(text)
}

fn rename(from: &Path, to: &Path) -> Result<(), BeaconError> {
    fs::rename(from, to).map_err(|source| BeaconError::Rename {
        from: from.to_path_buf(),
        to: to.to_path_buf(),
        source,
    })
}

/// Go's `os.Chmod(path, 0o755)` with its error ignored.
fn make_executable(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o755));
    }
    #[cfg(not(unix))]
    let _ = path;
}

/// Go's `os.Remove` with its error ignored: a file or link, else an empty
/// folder.
fn go_remove(path: &Path) {
    if fs::remove_file(path).is_err() {
        let _ = fs::remove_dir(path);
    }
}

/// Go's `os.RemoveAll` with its error ignored: a link is removed, not
/// followed.
fn go_remove_all(path: &Path) {
    match fs::symlink_metadata(path) {
        Ok(info) if info.is_dir() => {
            let _ = fs::remove_dir_all(path);
        }
        Ok(_) => {
            let _ = fs::remove_file(path);
        }
        Err(_) => {}
    }
}

/// Go's deferred removal of the stage, run however the export ends.
enum Cleanup {
    Remove(PathBuf),
    RemoveAll(PathBuf),
}

impl Drop for Cleanup {
    fn drop(&mut self) {
        match self {
            Cleanup::Remove(path) => go_remove(path),
            Cleanup::RemoveAll(path) => go_remove_all(path),
        }
    }
}

fn path_bytes(path: &Path) -> Vec<u8> {
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

fn bytes_path(bytes: Vec<u8>) -> PathBuf {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        PathBuf::from(std::ffi::OsString::from_vec(bytes))
    }
    #[cfg(not(unix))]
    {
        PathBuf::from(String::from_utf8_lossy(&bytes).into_owned())
    }
}

/// Go's `filepath.Clean` on Unix: the shortest lexically equal path, with
/// no `.` element, no `..` after a named element, and no `..` at the start
/// of a rooted path.
fn go_clean(path: &[u8]) -> Vec<u8> {
    if path.is_empty() {
        return b".".to_vec();
    }
    let rooted = path[0] == b'/';
    let mut out: Vec<u8> = Vec::with_capacity(path.len());
    let mut dotdot = 0;
    if rooted {
        out.push(b'/');
        dotdot = 1;
    }
    for element in path.split(|byte| *byte == b'/') {
        match element {
            b"" | b"." => {}
            b".." => {
                if out.len() > dotdot {
                    while out.len() > dotdot && out.last() != Some(&b'/') {
                        out.pop();
                    }
                    if out.len() > dotdot {
                        out.pop();
                    }
                } else if !rooted {
                    if !out.is_empty() {
                        out.push(b'/');
                    }
                    out.extend_from_slice(b"..");
                    dotdot = out.len();
                }
            }
            name => {
                if out.len() != usize::from(rooted) {
                    out.push(b'/');
                }
                out.extend_from_slice(name);
            }
        }
    }
    if out.is_empty() {
        out.push(b'.');
    }
    out
}

/// Go's `filepath.Join` of two elements: the non-empty ones joined with `/`,
/// then cleaned; empty when both are empty.
fn go_join(first: &[u8], second: &[u8]) -> Vec<u8> {
    match (first.is_empty(), second.is_empty()) {
        (true, true) => Vec::new(),
        (true, false) => go_clean(second),
        (false, true) => go_clean(first),
        (false, false) => {
            let mut joined = Vec::with_capacity(first.len() + 1 + second.len());
            joined.extend_from_slice(first);
            joined.push(b'/');
            joined.extend_from_slice(second);
            go_clean(&joined)
        }
    }
}

/// Go's `filepath.Dir`: everything before the last `/`, cleaned.
fn go_dir(path: &[u8]) -> Vec<u8> {
    match path.iter().rposition(|byte| *byte == b'/') {
        Some(slash) => go_clean(&path[..=slash]),
        None => b".".to_vec(),
    }
}

/// Go's `filepath.Base`.
fn go_base(path: &[u8]) -> &[u8] {
    if path.is_empty() {
        return b".";
    }
    let mut end = path.len();
    while end > 0 && path[end - 1] == b'/' {
        end -= 1;
    }
    if end == 0 {
        return b"/";
    }
    let trimmed = &path[..end];
    match trimmed.iter().rposition(|byte| *byte == b'/') {
        Some(slash) => &trimmed[slash + 1..],
        None => trimmed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(bytes: Vec<u8>) -> String {
        String::from_utf8(bytes).unwrap()
    }

    #[test]
    fn clean_join_dir_and_base_follow_go() {
        for (input, cleaned) in [
            ("", "."),
            ("abc", "abc"),
            ("abc/./def/", "abc/def"),
            ("a/b/c/../..", "a"),
            ("a/../../b", "../b"),
            ("../../x", "../../x"),
            ("a/..", "."),
            ("/../abc", "/abc"),
            ("//a//b/./c/", "/a/b/c"),
            ("/", "/"),
            ("/a/../..", "/"),
            ("abc/../../..", "../.."),
        ] {
            assert_eq!(text(go_clean(input.as_bytes())), cleaned, "{input:?}");
        }
        assert_eq!(text(go_join(b"/s", b"../x")), "/x");
        assert_eq!(text(go_join(b"/s", b"/etc/x")), "/s/etc/x");
        assert_eq!(text(go_join(b"/s", b"")), "/s");
        assert_eq!(text(go_join(b"", b"")), "");
        assert_eq!(text(go_dir(b"/a/b.md")), "/a");
        assert_eq!(text(go_dir(b"/b.md")), "/");
        assert_eq!(go_base(b"/a/b.md"), b"b.md");
        assert_eq!(go_base(b"/"), b"/");
    }

    #[test]
    fn markers_are_read_as_a_go_string_map() {
        let owns = |marker: &str| marker_names(marker.as_bytes(), "skill", "s");
        assert!(owns(r#"{"kind":"skill","name":"s"}"#));
        assert!(owns(
            r#"{"name":"s","kind":"x","kind":"skill","extra":"y"}"#
        ));
        assert!(!owns(r#"{"kind":"skill","name":"s","kind":null}"#));
        assert!(
            !owns(r#"{"Kind":"skill","name":"s"}"#),
            "map keys are exact"
        );
        assert!(!owns(r#"{"kind":"skill","name":"s","n":1}"#));
        assert!(!owns(r#"{"kind":"skill","name":"s","n":{}}"#));
        assert!(!owns("null"));
        assert!(!owns(r#"["skill","s"]"#));
        assert!(!owns(r#"{"kind":"skill","name":"s"} x"#));
        assert!(!owns(""));
    }

    #[test]
    fn only_escapes_onto_existing_names_are_found() {
        let dir = tempfile::tempdir().unwrap();
        let base = path_bytes(&fs::canonicalize(dir.path()).unwrap());
        let stage = go_join(&base, b"dest.lbc-stage-1");
        fs::write(bytes_path(go_join(&base, b"taken")), b"user").unwrap();
        fs::create_dir(bytes_path(go_join(&base, b"folder"))).unwrap();
        let file = |path: &str| ArtifactFile {
            path: path.to_owned(),
            ..ArtifactFile::default()
        };
        let found = |paths: &[&str]| {
            let files: Vec<ArtifactFile> = paths.iter().map(|path| file(path)).collect();
            escape_onto_existing(&stage, &files)
        };
        for inside in [
            "SKILL.md",
            "a/../b",
            "/abs",
            "",
            ".",
            "../dest.lbc-stage-1/x",
        ] {
            assert_eq!(found(&[inside]), None, "{inside:?}");
        }
        assert_eq!(found(&["../free"]), None, "GD51 stays copied");
        assert_eq!(
            found(&["SKILL.md", "../free", "x/../../taken"]),
            Some(bytes_path(go_join(&base, b"taken")))
        );
        assert_eq!(
            found(&["../folder"]),
            Some(bytes_path(go_join(&base, b"folder")))
        );
        assert!(!bytes_path(stage).exists(), "nothing is created");
    }

    #[test]
    fn default_destinations_and_relative_outs() {
        let record = |kind: &str| Artifact {
            kind: kind.to_owned(),
            name: "n".to_owned(),
            ..Artifact::default()
        };
        let pick = |kind: &str, out: &str, target: &str| {
            let options = ArtifactExport {
                out: out.to_owned(),
                target: target.to_owned(),
                force: false,
            };
            destination(b"/r", &record(kind), &options).map(text)
        };
        assert_eq!(pick("skill", "", "claude").unwrap(), "/r/.claude/skills/n");
        assert_eq!(pick("skill", "", "codex").unwrap(), "/r/.agents/skills/n");
        assert_eq!(
            pick("agent", "", "claude").unwrap(),
            "/r/.claude/agents/n.md"
        );
        assert_eq!(pick("skill", "o/../p/", "claude").unwrap(), "/r/p");
        assert_eq!(pick("bundle", "/x//y/", "").unwrap(), "/x/y");
        assert!(pick("agent", "", "codex").is_err());
        assert!(pick("bundle", "", "claude").is_err());
    }
}
