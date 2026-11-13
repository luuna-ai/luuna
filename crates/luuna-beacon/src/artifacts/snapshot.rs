//! Snapshots of a source folder or file, and the checks on a stored record
//! (`internal/artifact/artifact.go:18-170`): the digest, the manifest, the
//! name rule, the walk with its limits, `Verify`, `Decode` and `New`.

use super::base64;
use super::model::{Artifact, ArtifactFile};
use super::ArtifactsError;
use crate::error::BeaconError;
use crate::gojson::{self, ObjectWriter};
use crate::Beacon;
use sha2::{Digest as _, Sha256};
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::Read as _;
use std::path::Path;

/// The largest file a snapshot stores.
const MAX_FILE_BYTES: u64 = 4 * 1024 * 1024;
/// The most files a snapshot stores.
const MAX_FILES: usize = 256;

/// `Digest`: lowercase hexadecimal SHA-256.
pub(crate) fn digest(value: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let sum = Sha256::digest(value);
    let mut out = String::with_capacity(64);
    for byte in sum {
        out.push(char::from(HEX[usize::from(byte >> 4)]));
        out.push(char::from(HEX[usize::from(byte & 15)]));
    }
    out
}

/// `validName`: 1 to 80 bytes of lowercase letters, digits, dots, dashes and
/// underscores, the first a letter or digit.
pub(crate) fn valid_name(name: &str) -> bool {
    if name.is_empty() || name.len() > 80 {
        return false;
    }
    name.chars().enumerate().all(|(index, character)| {
        let alphanumeric = character.is_ascii_lowercase() || character.is_ascii_digit();
        alphanumeric || (index > 0 && matches!(character, '.' | '-' | '_'))
    })
}

/// `manifestBytes`: the JSON array of `{path, sha256, executable}` that the
/// record's own digest covers. No files is `[]`.
pub(crate) fn manifest_bytes(files: &[ArtifactFile]) -> Vec<u8> {
    let mut out = String::from("[");
    for (index, file) in files.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        let mut object = ObjectWriter::new(&mut out);
        object.string("path", &file.path);
        object.string("sha256", &file.sha256);
        gojson::write_bool(object.key("executable"), file.executable);
        object.finish();
    }
    out.push(']');
    out.into_bytes()
}

/// `Decode`: base64 only for the encoding `base64`; every other value is
/// taken as the text itself (GD57).
pub(crate) fn decode(file: &ArtifactFile) -> Result<Vec<u8>, ArtifactsError> {
    if file.encoding == "base64" {
        return base64::decode(file.content.as_bytes());
    }
    Ok(file.content.clone().into_bytes())
}

/// `Verify`: kind, name, version and file count, then each file's encoding
/// and digest, then the manifest digest, in Go's order.
pub(crate) fn verify(record: &Artifact) -> Result<(), ArtifactsError> {
    if !matches!(record.kind.as_str(), "skill" | "agent" | "bundle") {
        return Err(ArtifactsError::InvalidKind);
    }
    if !valid_name(&record.name) {
        return Err(ArtifactsError::InvalidArtifactName(record.name.clone()));
    }
    if record.version < 1 || record.files.is_empty() {
        return Err(ArtifactsError::InvalidArtifactRecord);
    }
    for file in &record.files {
        let data = decode(file)?;
        if digest(&data) != file.sha256 {
            return Err(ArtifactsError::HashMismatch(file.path.clone()));
        }
    }
    if digest(&manifest_bytes(&record.files)) != record.sha256 {
        return Err(ArtifactsError::ManifestMismatch);
    }
    Ok(())
}

/// `New`: a record with a fresh id and creation time from the handle (RD53),
/// and the digest of the manifest.
pub(crate) fn new_artifact(
    beacon: &Beacon,
    kind: &str,
    name: &str,
    files: Vec<ArtifactFile>,
    version: i64,
) -> Artifact {
    Artifact {
        id: format!("artifact-{}", beacon.next_id_hex()),
        kind: kind.to_owned(),
        name: name.to_owned(),
        version,
        sha256: digest(&manifest_bytes(&files)),
        files,
        created_at: beacon.now_text(),
    }
}

fn os_bytes(text: &OsStr) -> Vec<u8> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        text.as_bytes().to_vec()
    }
    #[cfg(not(unix))]
    {
        text.to_string_lossy().into_owned().into_bytes()
    }
}

fn os_from_bytes(bytes: Vec<u8>) -> OsString {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        OsString::from_vec(bytes)
    }
    #[cfg(not(unix))]
    {
        OsString::from(String::from_utf8_lossy(&bytes).into_owned())
    }
}

/// The text with each invalid byte replaced by U+FFFD, as Go's `range` over
/// a string and `encoding/json` do (one replacement per byte).
fn go_lossy(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len());
    let mut rest = bytes;
    loop {
        match std::str::from_utf8(rest) {
            Ok(valid) => {
                out.push_str(valid);
                return out;
            }
            Err(error) => {
                let (valid, after) = rest.split_at(error.valid_up_to());
                out.push_str(std::str::from_utf8(valid).unwrap_or_default());
                out.push('\u{FFFD}');
                rest = &after[1..];
            }
        }
    }
}

/// Go's `filepath.Base` for a `/`-separated path.
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

/// Whether any dot-separated part of the lower-cased base name contains a
/// credential word (GD56).
fn credential_like(logical: &[u8]) -> bool {
    let base = String::from_utf8_lossy(go_base(logical)).to_lowercase();
    base.split('.')
        .any(|part| part.contains("secret") || part.contains("token") || part.contains("password"))
}

fn executable(access: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::metadata(access).is_ok_and(|info| info.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        let _ = access;
        false
    }
}

/// `encode`: reads one file (at most 4 MiB plus one byte, RD51), applies
/// Go's checks in Go's order (read error, size, credential name), then RD56,
/// then chooses `utf8` or `base64`.
fn encode(logical: &[u8], access: &Path, display: &Path) -> Result<ArtifactFile, BeaconError> {
    let mut file =
        fs::File::open(access).map_err(|error| BeaconError::os("open", display, error))?;
    let mut content = Vec::new();
    (&mut file)
        .take(MAX_FILE_BYTES + 1)
        .read_to_end(&mut content)
        .map_err(|error| BeaconError::os("read", display, error))?;
    drop(file);
    if content.len() as u64 > MAX_FILE_BYTES {
        return Err(ArtifactsError::TooLarge(go_lossy(logical)).into());
    }
    if credential_like(logical) {
        return Err(ArtifactsError::CredentialLike(go_lossy(logical)).into());
    }
    let Ok(path) = String::from_utf8(logical.to_vec()) else {
        return Err(ArtifactsError::NonUtf8Path(go_lossy(logical)).into());
    };
    let sha256 = digest(&content);
    let (encoding, text) = match String::from_utf8(content) {
        Ok(text) => ("utf8", text),
        Err(error) => ("base64", base64::encode(error.as_bytes())),
    };
    Ok(ArtifactFile {
        path,
        encoding: encoding.to_owned(),
        content: text,
        sha256,
        executable: executable(access),
    })
}

/// One directory being walked: its remaining entries in lexical order, and
/// the relative path of the directory (empty for the source itself).
struct Frame {
    entries: std::vec::IntoIter<(OsString, fs::FileType)>,
    relative: OsString,
}

fn read_sorted(access: &Path, display: &Path) -> Result<Frame, BeaconError> {
    let reader = fs::read_dir(access).map_err(|error| BeaconError::os("open", display, error))?;
    let mut entries = Vec::new();
    for entry in reader {
        let entry = entry.map_err(|error| BeaconError::os("readdirent", display, error))?;
        let name = entry.file_name();
        let kind = entry
            .file_type()
            .map_err(|error| BeaconError::os("lstat", display.join(&name), error))?;
        entries.push((name, kind));
    }
    entries.sort_by(|left, right| os_bytes(&left.0).cmp(&os_bytes(&right.0)));
    Ok(Frame {
        entries: entries.into_iter(),
        relative: OsString::new(),
    })
}

fn has_file(files: &[ArtifactFile], path: &str) -> bool {
    files.iter().any(|file| file.path == path)
}

/// `Snapshot`. `source` is shown in messages as given; it is read under
/// `root` when relative (RD50). The walk is iterative, in `WalkDir`'s
/// lexical order (AS59); only top-level `.git`, `node_modules` and `.luuna`
/// are skipped (GD54).
pub(crate) fn snapshot(
    kind: &str,
    name: &str,
    root: &Path,
    source: &Path,
) -> Result<Vec<ArtifactFile>, BeaconError> {
    if !valid_name(name) {
        return Err(ArtifactsError::InvalidName.into());
    }
    let base = root.join(source);
    let info =
        fs::symlink_metadata(&base).map_err(|error| BeaconError::os("lstat", source, error))?;
    if info.file_type().is_symlink() {
        return Err(ArtifactsError::SourceSymlink.into());
    }
    if !info.is_dir() {
        let logical = os_bytes(source.as_os_str());
        let file = encode(go_base(&logical), &base, source)?;
        return Ok(vec![file]);
    }

    let mut files: Vec<ArtifactFile> = Vec::new();
    let mut stack = vec![read_sorted(&base, source)?];
    while let Some(frame) = stack.last_mut() {
        let Some((name, file_type)) = frame.entries.next() else {
            stack.pop();
            continue;
        };
        let mut relative = frame.relative.clone();
        if !relative.is_empty() {
            relative.push("/");
        }
        relative.push(&name);
        let relative_bytes = os_bytes(&relative);
        if file_type.is_symlink() {
            return Err(ArtifactsError::SourceSymlinkAt(go_lossy(&relative_bytes)).into());
        }
        if file_type.is_dir() {
            if matches!(
                relative_bytes.as_slice(),
                b".git" | b"node_modules" | b".luuna"
            ) {
                continue;
            }
            let mut child = read_sorted(&base.join(&relative), &source.join(&relative))?;
            child.relative = relative;
            stack.push(child);
            continue;
        }
        if files.len() >= MAX_FILES {
            return Err(ArtifactsError::FileLimit.into());
        }
        files.push(encode(
            &relative_bytes,
            &base.join(&relative),
            &source.join(&relative),
        )?);
    }
    files.sort_by(|left, right| left.path.cmp(&right.path));
    if kind == "skill" && !has_file(&files, "SKILL.md") {
        return Err(ArtifactsError::SkillNeedsSkillMd.into());
    }
    if kind == "bundle" && !has_file(&files, ".claude-plugin/plugin.json") {
        return Err(ArtifactsError::BundleNeedsPlugin.into());
    }
    Ok(files)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::ffi::OsStrExt;

    fn tree(entries: &[(&[u8], &[u8])]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for (path, contents) in entries {
            let target = dir.path().join(OsStr::from_bytes(path));
            fs::create_dir_all(target.parent().unwrap()).unwrap();
            fs::write(target, contents).unwrap();
        }
        dir
    }

    fn paths(files: &[ArtifactFile]) -> Vec<&str> {
        files.iter().map(|file| file.path.as_str()).collect()
    }

    fn error_text(result: Result<Vec<ArtifactFile>, BeaconError>) -> String {
        result.expect_err("expected an error").to_string()
    }

    #[test]
    fn walk_order_and_top_level_skips() {
        let dir = tree(&[
            (b"SKILL.md", b"s"),
            (b"b/z.md", b"1"),
            (b"a.b", b"2"),
            (b"a/c.md", b"3"),
            (b".git/config", b"4"),
            (b"node_modules/x.js", b"5"),
            (b".luuna/beacon/x", b"6"),
            (b"sub/.git/config", b"7"),
            (b"sub/node_modules/y.js", b"8"),
            (b"sub/.luuna/z", b"9"),
        ]);
        let files = snapshot("skill", "n", dir.path(), Path::new("")).unwrap();
        // Sorted by path as text: '.' sorts before '/'.
        assert_eq!(
            paths(&files),
            [
                "SKILL.md",
                "a.b",
                "a/c.md",
                "b/z.md",
                "sub/.git/config",
                "sub/.luuna/z",
                "sub/node_modules/y.js"
            ]
        );
        // A top-level `.git` file (not a folder) is stored (GD54).
        let dir = tree(&[(b"SKILL.md", b"s"), (b".git", b"gitdir: x")]);
        let files = snapshot("skill", "n", dir.path(), Path::new("")).unwrap();
        assert_eq!(paths(&files), [".git", "SKILL.md"]);
        // The 257th file in walk order is refused before it is read (GD63).
        let dir = tempfile::tempdir().unwrap();
        for index in 0..257 {
            fs::write(dir.path().join(format!("f{index:03}")), "x").unwrap();
        }
        assert_eq!(
            error_text(snapshot("agent", "n", dir.path(), Path::new(""))),
            "artifact file limit exceeded"
        );
        fs::remove_file(dir.path().join("f256")).unwrap();
        assert_eq!(
            snapshot("agent", "n", dir.path(), Path::new(""))
                .unwrap()
                .len(),
            256
        );
        // Requirements, and none for a single file (GD55).
        let dir = tree(&[(b"a.md", b"a")]);
        assert_eq!(
            error_text(snapshot("skill", "n", dir.path(), Path::new(""))),
            "skill must contain SKILL.md"
        );
        assert_eq!(
            error_text(snapshot("bundle", "n", dir.path(), Path::new(""))),
            "bundle must contain .claude-plugin/plugin.json"
        );
        assert_eq!(
            snapshot("skill", "n", dir.path(), Path::new("a.md")).unwrap()[0].path,
            "a.md"
        );
        assert_eq!(
            error_text(snapshot("skill", "Bad", dir.path(), Path::new("a.md"))),
            "artifact name must be 1-80 lowercase letters, digits, dots, dashes, or underscores, beginning with a letter or digit"
        );
        // Symlinks, the source itself or inside.
        let dir = tree(&[(b"SKILL.md", b"s"), (b"real", b"r")]);
        std::os::unix::fs::symlink("real", dir.path().join("link")).unwrap();
        assert_eq!(
            error_text(snapshot("skill", "n", dir.path(), Path::new(""))),
            "source symlinks are not allowed: link"
        );
        assert_eq!(
            error_text(snapshot("skill", "n", dir.path(), Path::new("link"))),
            "source symlinks are not allowed"
        );
        assert!(
            error_text(snapshot("skill", "n", dir.path(), Path::new("missing")))
                .starts_with("lstat missing: ")
        );
    }

    #[test]
    fn credential_names_follow_go() {
        for refused in [
            "api.token",
            "SECRET",
            "my-secret.txt",
            "a.Password.b",
            "tokens",
            "x.passwords.y",
            "dir/.secret",
        ] {
            assert!(credential_like(refused.as_bytes()), "{refused}");
        }
        for allowed in ["SKILL.md", "sec.ret", "to.ken", "pass.word", "secret/ok.md"] {
            assert!(!credential_like(allowed.as_bytes()), "{allowed}");
        }
        // Only the base name is checked: a folder name is not (GD56).
        let dir = tree(&[(b"SKILL.md", b"s"), (b"secrets/notes.md", b"n")]);
        assert!(snapshot("skill", "n", dir.path(), Path::new("")).is_ok());
        let dir = tree(&[(b"SKILL.md", b"s"), (b"docs/api.token", b"n")]);
        assert_eq!(
            error_text(snapshot("skill", "n", dir.path(), Path::new(""))),
            "credential-like file rejected: docs/api.token"
        );
        // The size check comes before the name check; a 4 MiB file is stored.
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("big.token"),
            vec![b'a'; 4 * 1024 * 1024 + 1],
        )
        .unwrap();
        assert_eq!(
            error_text(snapshot("agent", "n", dir.path(), Path::new(""))),
            "artifact file exceeds 4 MiB: big.token"
        );
        fs::write(dir.path().join("big.token"), vec![b'a'; 4 * 1024 * 1024]).unwrap();
        assert_eq!(
            error_text(snapshot("agent", "n", dir.path(), Path::new(""))),
            "credential-like file rejected: big.token"
        );
    }

    #[test]
    fn utf8_validity_matches_go() {
        let cases: &[(&[u8], bool)] = &[
            (b"", true),
            (b"plain", true),
            ("é🌙".as_bytes(), true),
            (b"\xff", false),
            (b"\xc0\x80", false),
            (b"\xed\xa0\x80", false),
            (b"\xf4\x90\x80\x80", false),
            (b"ok\xe2\x82", false),
            (b"\xef\xbf\xbd", true),
        ];
        let dir = tempfile::tempdir().unwrap();
        for (content, valid) in cases {
            fs::write(dir.path().join("f.bin"), content).unwrap();
            let files = snapshot("agent", "n", dir.path(), Path::new("")).unwrap();
            assert_eq!(files[0].encoding, if *valid { "utf8" } else { "base64" });
            assert_eq!(files[0].sha256, digest(content));
            assert_eq!(decode(&files[0]).unwrap(), *content);
        }
        // Any other encoding value is read as the text itself (GD57).
        let odd = ArtifactFile {
            encoding: "rot13".into(),
            content: "AAAA".into(),
            ..ArtifactFile::default()
        };
        assert_eq!(decode(&odd).unwrap(), b"AAAA");
    }

    #[test]
    fn manifest_of_no_files_is_the_empty_array() {
        assert_eq!(manifest_bytes(&[]), b"[]");
        assert_eq!(
            digest(&manifest_bytes(&[])),
            "4f53cda18c2baa0c0354bb5f9a3ecbe5ed12ab4d8e11ba873c2f11161202b945"
        );
        let file = ArtifactFile {
            path: "a<b>.md".into(),
            sha256: "ff".into(),
            executable: true,
            ..ArtifactFile::default()
        };
        assert_eq!(
            String::from_utf8(manifest_bytes(&[file.clone(), file])).unwrap(),
            "[{\"path\":\"a\\u003cb\\u003e.md\",\"sha256\":\"ff\",\"executable\":true},\
             {\"path\":\"a\\u003cb\\u003e.md\",\"sha256\":\"ff\",\"executable\":true}]"
        );
        assert_eq!(
            digest(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn non_utf8_logical_path_is_refused() {
        let dir = tree(&[(b"SKILL.md", b"s"), (b"caf\xe9.md", b"c")]);
        assert_eq!(
            error_text(snapshot("skill", "n", dir.path(), Path::new(""))),
            "refusing to store a path that is not valid UTF-8: caf\u{FFFD}.md"
        );
        let dir = tree(&[(b"\xffdir/a.md", b"o")]);
        assert_eq!(
            error_text(snapshot("agent", "n", dir.path(), Path::new(""))),
            "refusing to store a path that is not valid UTF-8: \u{FFFD}dir/a.md"
        );
        let dir = tree(&[(b"\xfe.md", b"o")]);
        assert_eq!(
            error_text(snapshot(
                "agent",
                "n",
                dir.path(),
                &Path::new("").join(OsStr::from_bytes(b"\xfe.md"))
            )),
            "refusing to store a path that is not valid UTF-8: \u{FFFD}.md"
        );
        // Go's own checks for the file come first.
        let dir = tree(&[(b"\xff.token", b"t")]);
        assert_eq!(
            error_text(snapshot("agent", "n", dir.path(), Path::new(""))),
            "credential-like file rejected: \u{FFFD}.token"
        );
        assert_eq!(go_lossy(b"a\xe2\x82b"), "a\u{FFFD}\u{FFFD}b");
    }

    #[test]
    fn verify_follows_go_order_and_new_builds_a_verifiable_record() {
        let beacon = Beacon::open(Path::new("/x")).unwrap();
        let dir = tree(&[(b"SKILL.md", b"s\n"), (b"b.bin", b"\xff\x00")]);
        let files = snapshot("skill", "n", dir.path(), Path::new("")).unwrap();
        let record = new_artifact(&beacon, "skill", "n", files, 1);
        assert!(record.id.starts_with("artifact-"));
        assert!(verify(&record).is_ok());

        let mut bad = record.clone();
        bad.kind = "other".into();
        bad.name = "Bad".into();
        assert!(matches!(verify(&bad), Err(ArtifactsError::InvalidKind)));
        bad.kind = "skill".into();
        assert!(matches!(
            verify(&bad),
            Err(ArtifactsError::InvalidArtifactName(_))
        ));
        let mut bad = record.clone();
        bad.version = 0;
        assert!(matches!(
            verify(&bad),
            Err(ArtifactsError::InvalidArtifactRecord)
        ));
        let mut bad = record.clone();
        bad.files[1].content = "!!!!".into();
        assert!(matches!(
            verify(&bad),
            Err(ArtifactsError::Base64 { offset: 0 })
        ));
        let mut bad = record.clone();
        bad.files[0].content = "t\n".into();
        assert!(matches!(verify(&bad), Err(ArtifactsError::HashMismatch(_))));
        let mut bad = record;
        bad.files[0].executable = true;
        assert!(matches!(
            verify(&bad),
            Err(ArtifactsError::ManifestMismatch)
        ));
    }
}
