//! Go's legacy `.seeds` rules (`internal/config/legacy.go`): the migration
//! of a `.seeds/` store to `.luuna/missions/` that discovery runs at every
//! directory it visits, the rewrite of `.gitattributes` and `.gitignore`
//! that goes with it, and the renames of old keys and markers.
//!
//! The migration never follows a symbolic link: `.seeds`, `.luuna` and the
//! two ignore files are looked at with `lstat`, a linked `.seeds` or
//! `.luuna` is refused and a linked ignore file is skipped. Go's two regular
//! expressions are hand-written scanners with Go's ASCII `\s` (space, `\t`,
//! `\n`, `\f`, `\r`).

use std::fs;
use std::io;
use std::path::Path;

use super::TreeValue;
use crate::config::{LEGACY_DIR, MISSIONS_DIR};
use crate::error::{Error, Result};
use crate::gopath;

/// Go's `MigrateLegacyLayout` for the `.seeds` entry `legacy` of
/// `project_root`. In Go's order: a present `.luuna/missions` is the "Both"
/// refusal, a linked `.seeds` or `.luuna` the symlink refusal, a `.seeds`
/// that is not a directory is left alone; otherwise `.luuna` is created
/// (0755), `.seeds` is renamed to `.luuna/missions`, the ignore files are
/// rewritten and the notice Go prints to stderr is raised.
pub(crate) fn migrate(project_root: &Path, legacy: &Path) -> Result<()> {
    let Some(legacy_meta) = lstat(legacy)? else {
        return Ok(());
    };
    let luuna = project_root.join(".luuna");
    let target = project_root.join(MISSIONS_DIR);
    let root = gopath::text(project_root);
    if lstat(&target)?.is_some() {
        return Err(Error::Command {
            message: format!(
                "Both {LEGACY_DIR}/ and {MISSIONS_DIR}/ exist in {root}. Resolve manually: keep {MISSIONS_DIR}/ and remove {LEGACY_DIR}/, or the reverse."
            ),
        });
    }
    let luuna_is_link = lstat(&luuna)?.is_some_and(|meta| meta.file_type().is_symlink());
    if legacy_meta.file_type().is_symlink() || luuna_is_link {
        return Err(Error::Command {
            message: format!(
                "Refusing to migrate {LEGACY_DIR}/ in {root}: {LEGACY_DIR} or .luuna is a symlink. Move it to {MISSIONS_DIR}/ manually."
            ),
        });
    }
    if !legacy_meta.is_dir() {
        return Ok(());
    }
    mkdir_all(&luuna)?;
    fs::rename(legacy, &target).map_err(|error| Error::io("rename", legacy, error))?;
    rewrite_ignore_files(project_root)?;
    let mut notice = format!("Migrated legacy {LEGACY_DIR}/ to {MISSIONS_DIR}/ in ").into_bytes();
    notice.extend_from_slice(&gopath::bytes(project_root));
    notice.push(b'\n');
    super::notice(&notice);
    Ok(())
}

/// Go's `rewriteIgnoreFiles`: `.gitattributes`, then `.gitignore`. An absent
/// or linked file is skipped; a file whose text changes is written again,
/// keeping its permission bits.
fn rewrite_ignore_files(root: &Path) -> Result<()> {
    for name in [".gitattributes", ".gitignore"] {
        let path = root.join(name);
        let Some(meta) = lstat(&path)? else {
            continue;
        };
        if meta.file_type().is_symlink() {
            continue;
        }
        // Read whole, as Go's os.ReadFile does.
        let before = fs::read(&path).map_err(|error| Error::io("read", &path, error))?;
        let after = replace_legacy_paths(&before);
        if after != before {
            write_keeping_mode(&path, &after, &meta)?;
        }
    }
    Ok(())
}

/// Go's `legacyIgnorePattern.ReplaceAllString(text,
/// "${1}.luuna/missions${2}")` for `(?m)(^|[\s!])\.seeds(/|\s|$)`.
///
/// Matches are leftmost and do not overlap: the search resumes after the
/// character a match consumed, and `^` holds at the start of the text or
/// after `\n` (so in `.seeds .seeds` only the first is replaced, as in Go).
/// Every byte the pattern reads is ASCII, so scanning bytes is scanning
/// Go's runes, and bytes outside a match, valid UTF-8 or not, are copied.
pub(crate) fn replace_legacy_paths(text: &[u8]) -> Vec<u8> {
    const NAME: &[u8] = b".seeds";
    let mut out = Vec::with_capacity(text.len());
    let mut copied = 0;
    let mut at = 0;
    while at < text.len() {
        let line_start = at == 0 || text[at - 1] == b'\n';
        // Group 1: `^` (empty) or one of `[\s!]`.
        let name_at = if line_start && text[at..].starts_with(NAME) {
            at
        } else if (is_go_space(text[at]) || text[at] == b'!') && text[at + 1..].starts_with(NAME) {
            at + 1
        } else {
            at += 1;
            continue;
        };
        // Group 2: `/`, then `\s`, then `$`, in that order; a `\n` is taken
        // by `\s`, so `$` is only the end of the text.
        let after = name_at + NAME.len();
        let end = match text.get(after) {
            None => after,
            Some(&byte) if byte == b'/' || is_go_space(byte) => after + 1,
            Some(_) => {
                at += 1;
                continue;
            }
        };
        out.extend_from_slice(&text[copied..name_at]);
        out.extend_from_slice(MISSIONS_DIR.as_bytes());
        out.extend_from_slice(&text[after..end]);
        copied = end;
        at = end;
    }
    out.extend_from_slice(&text[copied..]);
    out
}

/// Go's `UpgradeLegacyMarkers`.
#[allow(
    dead_code,
    reason = "called by onboard and setup in t9 and by plans in s4-missions-lib-4"
)]
pub(crate) fn upgrade_legacy_markers(text: &str) -> String {
    text.replace("<!-- seeds:", "<!-- missions:")
        .replace("<!-- seeds-", "<!-- missions-")
}

/// Go's `UpgradeLegacyConfigKeys`: in every mapping, `mulch_source` becomes
/// `atlas_source`, or is dropped when the mapping already has
/// `atlas_source`. The renamed key keeps the old key's place. Recursive:
/// a [`TreeValue`] is at most 1000 levels deep.
#[allow(
    dead_code,
    reason = "called by read_tree and load_plan_templates in t4 and t5"
)]
pub(crate) fn upgrade_legacy_config_keys(value: TreeValue) -> TreeValue {
    match value {
        TreeValue::Seq(items) => {
            TreeValue::Seq(items.into_iter().map(upgrade_legacy_config_keys).collect())
        }
        TreeValue::Map(entries) => {
            let has_new = entries.iter().any(|(key, _)| key == "atlas_source");
            let mut out = Vec::with_capacity(entries.len());
            for (key, item) in entries {
                if key == "mulch_source" {
                    if !has_new {
                        out.push(("atlas_source".to_owned(), upgrade_legacy_config_keys(item)));
                    }
                    continue;
                }
                out.push((key, upgrade_legacy_config_keys(item)));
            }
            TreeValue::Map(out)
        }
        other => other,
    }
}

/// Go's `LegacyHookCommandRE`, `^\s*(?:sd|seeds)\s+(prime|guard)(\s|$)`:
/// the subcommand (`prime` or `guard`, the first group) when `command`
/// matches, as `FindStringSubmatch(command)[1]`; `None` when it does not.
#[allow(dead_code, reason = "called by setup and upgrade in t9")]
pub(crate) fn legacy_hook_command(command: &str) -> Option<&str> {
    let rest = command.trim_start_matches(|ch: char| ch.is_ascii() && is_go_space(ch as u8));
    let rest = rest
        .strip_prefix("sd")
        .or_else(|| rest.strip_prefix("seeds"))?;
    let after = rest.trim_start_matches(|ch: char| ch.is_ascii() && is_go_space(ch as u8));
    if after.len() == rest.len() {
        return None;
    }
    let name = ["prime", "guard"]
        .into_iter()
        .find(|name| after.starts_with(name))?;
    match after.as_bytes().get(name.len()) {
        Some(&byte) if !is_go_space(byte) => None,
        _ => Some(&after[..name.len()]),
    }
}

/// Go's `\s` in RE2: `[\t\n\f\r ]` (not `\v`).
fn is_go_space(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\n' | b'\x0c' | b'\r')
}

/// Go's `lstatExists`: the entry's own metadata, never following a link.
fn lstat(path: &Path) -> Result<Option<fs::Metadata>> {
    match fs::symlink_metadata(path) {
        Ok(meta) => Ok(Some(meta)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(Error::io("lstat", path, error)),
    }
}

/// Go's `os.MkdirAll(dir, 0755)`.
fn mkdir_all(dir: &Path) -> Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o755);
    builder
        .create(dir)
        .map_err(|error| Error::io("mkdir", dir, error))
}

/// Go's `os.WriteFile(path, content, info.Mode().Perm())` on an existing
/// file: truncated and written; its mode is used only if the file has to be
/// created again.
fn write_keeping_mode(path: &Path, content: &[u8], meta: &fs::Metadata) -> Result<()> {
    use std::io::Write;
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        options.mode(meta.permissions().mode() & 0o777);
    }
    #[cfg(not(unix))]
    let _ = meta;
    let mut file = options
        .open(path)
        .map_err(|error| Error::io("open", path, error))?;
    file.write_all(content)
        .map_err(|error| Error::io("write", path, error))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::config::{find_dir, refuse_legacy_layout, ProcessEnv, CONFIG_FILE};
    use crate::config_tree::capture_stderr_notices;

    /// A temporary folder under the system temp folder.
    fn temp_project() -> tempfile::TempDir {
        tempfile::Builder::new()
            .prefix("legacy-")
            .tempdir_in(std::env::temp_dir())
            .unwrap()
    }

    /// Go's two refusal texts for `root`.
    fn both(root: &Path) -> String {
        format!("Both .seeds/ and .luuna/missions/ exist in {}. Resolve manually: keep .luuna/missions/ and remove .seeds/, or the reverse.", gopath::text(root))
    }

    fn refusing(root: &Path) -> String {
        format!("Refusing to migrate .seeds/ in {}: .seeds or .luuna is a symlink. Move it to .luuna/missions/ manually.", gopath::text(root))
    }

    fn notice_for(root: &Path) -> Vec<u8> {
        format!(
            "Migrated legacy .seeds/ to .luuna/missions/ in {}\n",
            gopath::text(root)
        )
        .into_bytes()
    }

    /// The migration hook in `root`, its notices captured.
    fn migrate_in(root: &Path) -> (Result<()>, Vec<u8>) {
        capture_stderr_notices(|| migrate(root, &root.join(LEGACY_DIR)))
    }

    fn command_text(result: Result<()>) -> String {
        match result {
            Err(Error::Command { message }) => message,
            other => panic!("expected Go's refusal, got {other:?}"),
        }
    }

    #[cfg(unix)]
    fn mode(path: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        fs::symlink_metadata(path).unwrap().permissions().mode() & 0o7777
    }

    #[cfg(unix)]
    fn set_mode(path: &Path, mode: u32) {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
    }

    /// Every entry below `dir`, sorted, with its kind, link target and
    /// bytes; links are not followed.
    fn snapshot(dir: &Path) -> Vec<(PathBuf, String, Vec<u8>)> {
        let mut out = Vec::new();
        let mut pending = vec![dir.to_path_buf()];
        while let Some(current) = pending.pop() {
            for entry in fs::read_dir(&current).unwrap() {
                let path = entry.unwrap().path();
                let meta = fs::symlink_metadata(&path).unwrap();
                let relative = path.strip_prefix(dir).unwrap().to_path_buf();
                if meta.file_type().is_symlink() {
                    let target = fs::read_link(&path).unwrap();
                    out.push((relative, format!("link {}", target.display()), Vec::new()));
                } else if meta.is_dir() {
                    out.push((relative, "dir".to_owned(), Vec::new()));
                    pending.push(path);
                } else {
                    out.push((relative, "file".to_owned(), fs::read(&path).unwrap()));
                }
            }
        }
        out.sort();
        out
    }

    #[test]
    fn legacy_move_rejects_conflicts_and_rewrites_managed_paths() {
        // TestLegacyMoveRejectsConflictsAndRewritesManagedPaths
        // (internal/config/config_test.go:68).
        let temp = temp_project();
        let root = temp.path();
        let legacy = root.join(LEGACY_DIR);
        fs::create_dir(&legacy).unwrap();
        fs::write(legacy.join(CONFIG_FILE), "project: demo\nversion: '1'\n").unwrap();
        fs::write(
            root.join(".gitattributes"),
            ".seeds/issues.jsonl merge=union\n",
        )
        .unwrap();
        let (result, notices) = migrate_in(root);
        result.unwrap();
        assert_eq!(notices, notice_for(root));
        assert_eq!(
            fs::read(root.join(MISSIONS_DIR).join(CONFIG_FILE)).unwrap(),
            b"project: demo\nversion: '1'\n"
        );
        assert_eq!(
            fs::read(root.join(".gitattributes")).unwrap(),
            b".luuna/missions/issues.jsonl merge=union\n"
        );
        assert!(
            !root.join(".gitignore").exists(),
            "an absent file stays absent"
        );
        fs::create_dir(&legacy).unwrap();
        let (result, notices) = migrate_in(root);
        assert_eq!(command_text(result), both(root));
        assert!(notices.is_empty());
        assert!(legacy.is_dir());
        // The same through discovery's entry point.
        assert_eq!(command_text(refuse_legacy_layout(root)), both(root));
        // A .seeds that is not a directory is left alone, with no .luuna.
        let temp = temp_project();
        let root = temp.path();
        fs::write(root.join(LEGACY_DIR), "x").unwrap();
        let before = snapshot(root);
        let (result, notices) = migrate_in(root);
        result.unwrap();
        assert!(notices.is_empty());
        assert_eq!(snapshot(root), before);
        // ... unless .luuna/missions exists: Both comes first.
        fs::create_dir_all(root.join(MISSIONS_DIR)).unwrap();
        assert_eq!(command_text(migrate_in(root).0), both(root));
        // No .seeds: nothing to do.
        let temp = temp_project();
        let (result, notices) = migrate_in(temp.path());
        result.unwrap();
        assert!(notices.is_empty());
        assert!(snapshot(temp.path()).is_empty());
        // An existing .luuna directory is kept; the store moves into it.
        let temp = temp_project();
        let root = temp.path();
        fs::create_dir_all(root.join(".luuna/atlas")).unwrap();
        fs::create_dir(root.join(LEGACY_DIR)).unwrap();
        fs::write(root.join(LEGACY_DIR).join("issues.jsonl"), "{}\n").unwrap();
        migrate_in(root).0.unwrap();
        assert_eq!(
            snapshot(root),
            vec![
                (PathBuf::from(".luuna"), "dir".to_owned(), Vec::new()),
                (PathBuf::from(".luuna/atlas"), "dir".to_owned(), Vec::new()),
                (
                    PathBuf::from(".luuna/missions"),
                    "dir".to_owned(),
                    Vec::new()
                ),
                (
                    PathBuf::from(".luuna/missions/issues.jsonl"),
                    "file".to_owned(),
                    b"{}\n".to_vec()
                ),
            ]
        );
    }

    #[cfg(unix)]
    #[test]
    fn ignore_file_rewrite_matches_go_s_pattern() {
        // Each pair was printed by Go's regexp for
        // `(?m)(^|[\s!])\.seeds(/|\s|$)` and `${1}.luuna/missions${2}`.
        let cases: &[(&[u8], &[u8])] = &[
            (
                b".seeds/issues.jsonl merge=union\n",
                b".luuna/missions/issues.jsonl merge=union\n",
            ),
            (
                b"!.seeds\n.seeds/x\n",
                b"!.luuna/missions\n.luuna/missions/x\n",
            ),
            (b".seeds", b".luuna/missions"),
            (b".seeds .seeds", b".luuna/missions .seeds"),
            (b".seeds\n.seeds\n", b".luuna/missions\n.luuna/missions\n"),
            (b"a.seeds/x", b"a.seeds/x"),
            (b"x .seeds\tx", b"x .luuna/missions\tx"),
            (b".seedsx", b".seedsx"),
            (b"!.seeds!", b"!.seeds!"),
            (b"\r.seeds\r\n", b"\r.luuna/missions\r\n"),
            (b".seeds\x0b", b".seeds\x0b"),
            (b"\x0c.seeds\x0c", b"\x0c.luuna/missions\x0c"),
            (b".seeds/.seeds/", b".luuna/missions/.seeds/"),
            (b" .seeds", b" .luuna/missions"),
            (b"x\n.seeds", b"x\n.luuna/missions"),
            (b"!!.seeds", b"!!.luuna/missions"),
            ("\u{e9}.seeds".as_bytes(), "\u{e9}.seeds".as_bytes()),
            (b"\xff\n.seeds/\xfe", b"\xff\n.luuna/missions/\xfe"),
            (b".SEEDS", b".SEEDS"),
            (b"..seeds", b"..seeds"),
            (b" .seeds  .seeds", b" .luuna/missions  .luuna/missions"),
            (b"\x0b.seeds ", b"\x0b.seeds "),
            (b".seeds\r.seeds", b".luuna/missions\r.seeds"),
            (b"", b""),
            (b"\n\n.seeds", b"\n\n.luuna/missions"),
        ];
        for (input, expected) in cases {
            assert_eq!(
                replace_legacy_paths(input),
                *expected,
                "{:?}",
                String::from_utf8_lossy(input)
            );
        }
        // In files, during a migration (go-first-1.txt:162-165): both files
        // rewritten, each keeping its mode; a file without a match is not
        // written (read-only here, so writing it would fail).
        let temp = temp_project();
        let root = temp.path();
        fs::create_dir(root.join(LEGACY_DIR)).unwrap();
        fs::write(
            root.join(".gitattributes"),
            ".seeds/issues.jsonl merge=union\n",
        )
        .unwrap();
        fs::write(root.join(".gitignore"), "!.seeds\n.seeds/x\n").unwrap();
        set_mode(&root.join(".gitignore"), 0o600);
        set_mode(&root.join(".gitattributes"), 0o664);
        migrate_in(root).0.unwrap();
        assert_eq!(
            fs::read(root.join(".gitattributes")).unwrap(),
            b".luuna/missions/issues.jsonl merge=union\n"
        );
        assert_eq!(
            fs::read(root.join(".gitignore")).unwrap(),
            b"!.luuna/missions\n.luuna/missions/x\n"
        );
        assert_eq!(mode(&root.join(".gitignore")), 0o600);
        assert_eq!(mode(&root.join(".gitattributes")), 0o664);
        let temp = temp_project();
        let root = temp.path();
        fs::create_dir(root.join(LEGACY_DIR)).unwrap();
        fs::write(root.join(".gitignore"), "target/\n.seedsx\n").unwrap();
        set_mode(&root.join(".gitignore"), 0o444);
        migrate_in(root).0.unwrap();
        assert_eq!(
            fs::read(root.join(".gitignore")).unwrap(),
            b"target/\n.seedsx\n"
        );
        assert_eq!(mode(&root.join(".gitignore")), 0o444);
    }

    #[test]
    fn upgrade_legacy_config_keys_renames_or_drops_mulch_source() {
        let s = |text: &str| TreeValue::Str(text.to_owned());
        let map = |entries: Vec<(&str, TreeValue)>| {
            TreeValue::Map(
                entries
                    .into_iter()
                    .map(|(key, value)| (key.to_owned(), value))
                    .collect(),
            )
        };
        // Renamed in place, recursively, also inside sequences and inside
        // the renamed value.
        let input = map(vec![
            ("project", s("p")),
            (
                "plan_templates",
                map(vec![(
                    "small",
                    map(vec![(
                        "sections",
                        TreeValue::Seq(vec![
                            map(vec![
                                ("mulch_source", s("a")),
                                ("required", TreeValue::Bool(true)),
                            ]),
                            s("mulch_source"),
                        ]),
                    )]),
                )]),
            ),
            ("mulch_source", map(vec![("mulch_source", s("inner"))])),
            ("max", TreeValue::Int(3)),
        ]);
        let expected = map(vec![
            ("project", s("p")),
            (
                "plan_templates",
                map(vec![(
                    "small",
                    map(vec![(
                        "sections",
                        TreeValue::Seq(vec![
                            map(vec![
                                ("atlas_source", s("a")),
                                ("required", TreeValue::Bool(true)),
                            ]),
                            s("mulch_source"),
                        ]),
                    )]),
                )]),
            ),
            ("atlas_source", map(vec![("atlas_source", s("inner"))])),
            ("max", TreeValue::Int(3)),
        ]);
        assert_eq!(upgrade_legacy_config_keys(input), expected);
        // With both keys, the new one is kept and the old one dropped,
        // whichever comes first.
        for input in [
            map(vec![("mulch_source", s("old")), ("atlas_source", s("new"))]),
            map(vec![("atlas_source", s("new")), ("mulch_source", s("old"))]),
        ] {
            assert_eq!(
                upgrade_legacy_config_keys(input),
                map(vec![("atlas_source", s("new"))])
            );
        }
        // Other values are returned as they are.
        for value in [
            TreeValue::Null,
            TreeValue::Bool(false),
            TreeValue::Int(-1),
            TreeValue::Uint(u64::MAX),
            TreeValue::Float(2.5),
            s("mulch_source"),
            TreeValue::Other("map[interface {}]interface {}"),
            map(vec![]),
        ] {
            assert_eq!(upgrade_legacy_config_keys(value.clone()), value);
        }
    }

    #[test]
    fn upgrade_legacy_markers_replaces_every_occurrence() {
        assert_eq!(
            upgrade_legacy_markers(
                "<!-- seeds:start -->\nx <!-- seeds-onboard-v:1 --> y\n<!-- seeds:end --><!-- seeds:end -->\n"
            ),
            "<!-- missions:start -->\nx <!-- missions-onboard-v:1 --> y\n<!-- missions:end --><!-- missions:end -->\n"
        );
        for unchanged in [
            "",
            "<!--seeds:start -->",
            "<!-- Seeds:start -->",
            "<!-- seeds start -->",
            "seeds:",
            "<!-- missions:start -->",
        ] {
            assert_eq!(upgrade_legacy_markers(unchanged), unchanged);
        }
    }

    #[test]
    fn legacy_hook_command_pattern_matches_go() {
        // Each pair was printed by Go's FindStringSubmatch for
        // `^\s*(?:sd|seeds)\s+(prime|guard)(\s|$)` (group 1, or no match).
        let cases: &[(&str, Option<&str>)] = &[
            ("sd prime", Some("prime")),
            ("seeds guard --x", Some("guard")),
            ("  sd  prime", Some("prime")),
            ("\tseeds\tguard\n", Some("guard")),
            ("sd prime-x", None),
            ("sdprime", None),
            ("sd primer", None),
            ("lms prime", None),
            ("xsd prime", None),
            ("sd guard", Some("guard")),
            ("sd prime ", Some("prime")),
            ("seeds\nprime", Some("prime")),
            ("sd\x0bprime", None),
            ("SD prime", None),
            ("sd  prime\r", Some("prime")),
            ("s prime", None),
            ("sd prime\x0b", None),
            ("", None),
            ("\x0b sd prime", None),
            ("seeds  guard\x0cx", Some("guard")),
            ("sd sd prime", None),
        ];
        for (command, expected) in cases {
            assert_eq!(legacy_hook_command(command), *expected, "{command:?}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_seeds_or_luuna_is_refused_and_nothing_moves() {
        use std::os::unix::fs::symlink;
        // .seeds a link to a directory holding a file (Bb4).
        let temp = temp_project();
        let root = temp.path().join("root");
        fs::create_dir(&root).unwrap();
        let elsewhere = temp.path().join("elsewhere");
        fs::create_dir(&elsewhere).unwrap();
        fs::write(elsewhere.join("issues.jsonl"), "{\"id\":\"x\"}\n").unwrap();
        symlink(&elsewhere, root.join(LEGACY_DIR)).unwrap();
        let before = snapshot(temp.path());
        let (result, notices) = capture_stderr_notices(|| refuse_legacy_layout(&root));
        assert_eq!(command_text(result), refusing(&root));
        assert!(notices.is_empty());
        assert_eq!(snapshot(temp.path()), before);
        assert_eq!(fs::read_link(root.join(LEGACY_DIR)).unwrap(), elsewhere);
        // A dangling .seeds link (Bb4b).
        let temp = temp_project();
        symlink("elsewhere", temp.path().join(LEGACY_DIR)).unwrap();
        let before = snapshot(temp.path());
        assert_eq!(
            command_text(migrate_in(temp.path()).0),
            refusing(temp.path())
        );
        assert_eq!(snapshot(temp.path()), before);
        // .luuna a link, .seeds a directory holding a file (Bb5).
        let temp = temp_project();
        let root = temp.path().join("root");
        fs::create_dir(&root).unwrap();
        let elsewhere = temp.path().join("elsewhere-l");
        fs::create_dir(&elsewhere).unwrap();
        fs::write(elsewhere.join("kept"), "kept\n").unwrap();
        symlink(&elsewhere, root.join(".luuna")).unwrap();
        fs::create_dir(root.join(LEGACY_DIR)).unwrap();
        fs::write(root.join(LEGACY_DIR).join("config.yaml"), "project: p\n").unwrap();
        let before = snapshot(temp.path());
        let (result, notices) = capture_stderr_notices(|| refuse_legacy_layout(&root));
        assert_eq!(command_text(result), refusing(&root));
        assert!(notices.is_empty());
        assert_eq!(snapshot(temp.path()), before);
        assert!(!elsewhere.join("missions").exists());
        // .luuna a link and no .seeds (Bb5b): nothing to migrate.
        fs::remove_dir_all(root.join(LEGACY_DIR)).unwrap();
        let before = snapshot(temp.path());
        refuse_legacy_layout(&root).unwrap();
        assert_eq!(snapshot(temp.path()), before);
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_ignore_files_are_skipped_and_left_unchanged() {
        use std::os::unix::fs::symlink;
        let temp = temp_project();
        let root = temp.path().join("root");
        fs::create_dir(&root).unwrap();
        let outside = temp.path().join("outside-ignore");
        fs::write(&outside, ".seeds/\n").unwrap();
        set_mode(&outside, 0o640);
        symlink(&outside, root.join(".gitignore")).unwrap();
        symlink("missing-attributes", root.join(".gitattributes")).unwrap();
        fs::create_dir(root.join(LEGACY_DIR)).unwrap();
        let (result, notices) = migrate_in(&root);
        result.unwrap();
        assert_eq!(notices, notice_for(&root));
        assert!(root.join(MISSIONS_DIR).is_dir());
        assert_eq!(fs::read(&outside).unwrap(), b".seeds/\n");
        assert_eq!(mode(&outside), 0o640);
        assert_eq!(fs::read_link(root.join(".gitignore")).unwrap(), outside);
        assert_eq!(
            fs::read_link(root.join(".gitattributes")).unwrap(),
            Path::new("missing-attributes")
        );
        assert!(!root.join("missing-attributes").exists());
    }

    #[cfg(unix)]
    #[test]
    fn migration_from_a_temporary_project_leaves_its_parent_unchanged() {
        // Discovery walks above the project here (no config.yaml is found,
        // go-first-1.txt:151-154), so the parent P holds known files that a
        // migration in the wrong folder would change.
        let parent = temp_project();
        let p = parent.path();
        fs::write(p.join("sentinel"), "sentinel bytes\n").unwrap();
        fs::write(p.join(".gitignore"), ".seeds/\n").unwrap();
        set_mode(&p.join("sentinel"), 0o640);
        set_mode(&p.join(".gitignore"), 0o644);
        let project = p.join("project");
        fs::create_dir(&project).unwrap();
        fs::create_dir(project.join(LEGACY_DIR)).unwrap();
        let child = project.join("child");
        fs::create_dir(&child).unwrap();
        let entries = |dir: &Path| {
            let mut names: Vec<_> = fs::read_dir(dir)
                .unwrap()
                .map(|entry| entry.unwrap().file_name())
                .filter(|name| name != "project")
                .collect();
            names.sort();
            names
        };
        let entries_before = entries(p);
        let files_before = [
            (
                fs::read(p.join("sentinel")).unwrap(),
                mode(&p.join("sentinel")),
            ),
            (
                fs::read(p.join(".gitignore")).unwrap(),
                mode(&p.join(".gitignore")),
            ),
        ];
        let (result, notices) = capture_stderr_notices(|| find_dir(&child, ProcessEnv::Inherit));
        assert!(matches!(result, Err(Error::NotInProject)), "{result:?}");
        // The migration happened in P/project only.
        assert_eq!(notices, notice_for(&project));
        assert!(project.join(MISSIONS_DIR).is_dir());
        assert!(!project.join(LEGACY_DIR).exists());
        assert!(child.is_dir());
        // P is as it was.
        assert_eq!(entries(p), entries_before);
        let files_after = [
            (
                fs::read(p.join("sentinel")).unwrap(),
                mode(&p.join("sentinel")),
            ),
            (
                fs::read(p.join(".gitignore")).unwrap(),
                mode(&p.join(".gitignore")),
            ),
        ];
        assert_eq!(files_after, files_before);
        assert_eq!(files_after[1].0, b".seeds/\n");
        assert!(fs::symlink_metadata(p.join(".luuna")).is_err());
    }
}
