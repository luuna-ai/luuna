//! `init`: creates `.luuna/missions/` and the merge=union lines in the
//! project's `.gitattributes` (`internal/cli/issue_create.go:19-50`,
//! `internal/config/config.go:115-172`).
//!
//! As in Go, `init` works in the working directory and never discovers a
//! store above it. It accepts only `--json` and the global switches, so
//! `--format` is an unknown option here.
//!
//! An invocation with an explicit store directory is accepted only when
//! that directory is `<cwd>/.luuna/missions`, the one Go's `init` creates;
//! any other is [`Error::Unsupported`](crate::Error::Unsupported), because Go
//! has no way to initialise a store elsewhere and this crate does not guess
//! one.

use std::fs;
use std::io;
use std::path::Path;

use super::output::Envelope;
use super::{options, Context, Failure};
use crate::config::{self, CONFIG_FILE, MISSIONS_DIR};
use crate::error::Error;
use crate::gopath;

/// Go's `runInit`.
pub(super) fn run(context: &mut Context<'_>, args: &[String]) -> Result<(), Failure> {
    let options = options::parse(args, &[], &["json"])?;
    if !options.positionals.is_empty() {
        return Err(Failure::message("too many arguments for 'init'"));
    }
    let root = context.cwd.clone();
    let dir = root.join(MISSIONS_DIR);
    if let Some(explicit) = context.explicit_store_dir() {
        let explicit =
            gopath::abs(explicit).map_err(|error| Error::io("getwd", explicit, error))?;
        if explicit != dir {
            return Err(Error::unsupported(
                format!(
                    "init with the explicit store directory {} (Go's init creates only {})",
                    explicit.display(),
                    dir.display()
                ),
                "a later plan that defines it (Go has no such option)",
            )
            .into());
        }
    }
    config::refuse_legacy_layout(&root)?;
    let config_path = dir.join(CONFIG_FILE);
    let already = match fs::metadata(&config_path) {
        Ok(_) => true,
        Err(error) if error.kind() == io::ErrorKind::NotFound => false,
        Err(error) => return Err(Error::io("stat", config_path, error).into()),
    };
    if !already {
        refuse_lossy_init(&dir)?;
    }
    config::init(&root)?;
    if options.has("json") {
        context.json(Envelope::new(true, "init").string("dir", &gopath::text(&dir)));
    } else if already {
        let mut message = b"Already initialized: ".to_vec();
        message.extend_from_slice(&gopath::bytes(&dir));
        context.success(&message);
    } else {
        let mut message = b"Initialized .luuna/missions/ in ".to_vec();
        message.extend_from_slice(&gopath::bytes(&root));
        context.success(&message);
    }
    Ok(())
}

/// Finds the first nonempty/nonstandard line with constant memory, including
/// UTF-8 whitespace split across read boundaries. No entire line is buffered.
fn init_content_line(mut input: impl std::io::Read, ignore: bool) -> io::Result<Option<usize>> {
    let mut buffer = [0u8; 8192];
    let mut unit = [0u8; 4];
    let mut used = 0;
    let mut line = 1;
    let mut word = 0;
    let mut trailing = false;
    loop {
        let count = input.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        for byte in &buffer[..count] {
            let ch = if used == 0 && byte.is_ascii() {
                char::from(*byte)
            } else {
                unit[used] = *byte;
                used += 1;
                match std::str::from_utf8(&unit[..used]) {
                    Ok(text) => {
                        let ch = text.chars().next().expect("one UTF-8 scalar");
                        used = 0;
                        ch
                    }
                    Err(error) if error.error_len().is_none() && used < 4 => continue,
                    Err(_) => return Ok(Some(line)),
                }
            };
            if ch == '\n' {
                if ignore && word != 0 && word != 6 {
                    return Ok(Some(line));
                }
                line += 1;
                word = 0;
                trailing = false;
                continue;
            }
            if ch.is_whitespace() {
                if word > 0 {
                    trailing = true
                }
                continue;
            }
            if !ignore || trailing || !ch.is_ascii() || b"*.lock".get(word) != Some(&(ch as u8)) {
                return Ok(Some(line));
            }
            word += 1;
        }
    }
    Ok((used > 0 || ignore && word != 0 && word != 6).then_some(line))
}
/// Read-only A8 preflight; follows links to regular files Go would overwrite.
fn refuse_lossy_init(dir: &Path) -> Result<(), Failure> {
    for name in [
        "issues.jsonl",
        "templates.jsonl",
        "plans.jsonl",
        ".gitignore",
    ] {
        let path = dir.join(name);
        let metadata = fs::metadata(&path).ok();
        if metadata.as_ref().is_some_and(|m| !m.is_file()) {
            continue;
        }
        let line = match fs::File::open(&path)
            .and_then(|file| init_content_line(file, name == ".gitignore"))
        {
            Ok(line) => line,
            Err(_) => {
                if metadata.is_some_and(|m| m.len() > 0) {
                    return Err(Failure::message(format!("Refusing to initialize: .luuna/missions/{name} is nonempty but cannot be read while config.yaml is missing, so initializing could overwrite unchecked user data. Nothing was written.")));
                }
                continue;
            }
        };
        if let Some(line) = line {
            let message = if name == ".gitignore" {
                format!("Refusing to initialize: line {line} of .luuna/missions/.gitignore holds an entry other than *.lock, and initializing would replace the file with *.lock alone. Nothing was written.")
            } else {
                format!("Refusing to initialize: line {line} of .luuna/missions/{name} holds data and config.yaml is missing, and initializing would empty the file. Nothing was written.")
            };
            return Err(Failure::message(message));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::super::{run, Invocation, Outcome};
    use super::*;

    const ATTRIBUTES: &str = ".luuna/missions/issues.jsonl merge=union\n.luuna/missions/templates.jsonl merge=union\n.luuna/missions/plans.jsonl merge=union\n";

    fn init(cwd: &Path, args: &[&str]) -> Outcome {
        let mut all = vec!["init"];
        all.extend_from_slice(args);
        run(&Invocation::new(cwd, all)).unwrap()
    }

    fn text(bytes: &[u8]) -> String {
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    #[cfg(unix)]
    fn mode(path: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        fs::metadata(path).unwrap().permissions().mode() & 0o7777
    }

    #[test]
    fn init_creates_the_files_go_creates() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("my project");
        fs::create_dir(&root).unwrap();
        let outcome = init(&root, &[]);
        assert_eq!(outcome.exit_code, 0);
        assert_eq!(
            text(&outcome.stdout),
            format!("✓ Initialized .luuna/missions/ in {}\n", root.display())
        );
        assert!(outcome.stderr.is_empty());
        let dir = root.join(MISSIONS_DIR);
        let read = |name: &str| fs::read_to_string(dir.join(name)).unwrap();
        assert_eq!(
            read(CONFIG_FILE),
            "project: \"my project\"\nversion: \"1\"\nmax_plan_depth: 3\n"
        );
        for name in ["issues.jsonl", "templates.jsonl", "plans.jsonl"] {
            assert_eq!(read(name), "", "{name}");
        }
        assert_eq!(read(".gitignore"), "*.lock\n");
        assert_eq!(
            fs::read_to_string(root.join(".gitattributes")).unwrap(),
            ATTRIBUTES
        );
        assert_eq!(
            crate::config::read_config(&dir).unwrap().project,
            "my project"
        );
        #[cfg(unix)]
        {
            // Go's modes, less the same umask: compare with files made the
            // same way here.
            use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
            let reference = temp.path().join("reference");
            fs::DirBuilder::new()
                .mode(0o755)
                .create(&reference)
                .unwrap();
            let file = reference.join("file");
            fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o644)
                .open(&file)
                .unwrap();
            for path in [root.join(".luuna"), dir.clone()] {
                assert_eq!(mode(&path), mode(&reference), "{}", path.display());
            }
            for name in [CONFIG_FILE, "issues.jsonl", ".gitignore"] {
                assert_eq!(mode(&dir.join(name)), mode(&file), "{name}");
            }
            assert_eq!(mode(&root.join(".gitattributes")), mode(&file));
        }
    }

    #[test]
    fn init_twice_keeps_the_store_and_says_so() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        init(root, &["-q"]);
        let dir = root.join(MISSIONS_DIR);
        fs::write(dir.join("issues.jsonl"), "kept\n").unwrap();
        fs::write(dir.join(CONFIG_FILE), "project: parity\nversion: \"1\"\n").unwrap();
        let outcome = init(root, &[]);
        assert_eq!(
            text(&outcome.stdout),
            format!("✓ Already initialized: {}\n", dir.display())
        );
        assert_eq!(
            fs::read_to_string(dir.join("issues.jsonl")).unwrap(),
            "kept\n"
        );
        assert_eq!(
            fs::read_to_string(dir.join(CONFIG_FILE)).unwrap(),
            "project: parity\nversion: \"1\"\n"
        );
        // --json prints the same envelope either way.
        let outcome = init(root, &["--json"]);
        assert_eq!(
            text(&outcome.stdout),
            format!(
                "{{\n  \"success\": true,\n  \"command\": \"init\",\n  \"dir\": \"{}\"\n}}\n",
                dir.display()
            )
        );
        // -q silences only the human line.
        assert_eq!(init(root, &["-q"]), Outcome::default());
    }

    #[test]
    fn init_backfills_missing_merge_lines() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        fs::write(
            root.join(".gitattributes"),
            "*.png binary\n.luuna/missions/templates.jsonl merge=union",
        )
        .unwrap();
        init(root, &[]);
        assert_eq!(
            fs::read_to_string(root.join(".gitattributes")).unwrap(),
            "*.png binary\n.luuna/missions/templates.jsonl merge=union\n.luuna/missions/issues.jsonl merge=union\n.luuna/missions/plans.jsonl merge=union\n"
        );
        // An initialised store still gets missing lines back.
        fs::write(root.join(".gitattributes"), "").unwrap();
        init(root, &[]);
        assert_eq!(
            fs::read_to_string(root.join(".gitattributes")).unwrap(),
            ATTRIBUTES
        );
    }

    #[test]
    fn init_without_config_yaml_refuses_to_empty_the_data_files() {
        // Go writes every data file again when config.yaml is missing and
        // empties one that holds data (SD22). The port refuses and writes
        // nothing (A8, A10; s4-missions-lib-refuse DL118).
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join(MISSIONS_DIR);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("issues.jsonl"), "{\"id\":\"x\"}\n").unwrap();
        #[cfg(unix)]
        let before = mode(&dir.join("issues.jsonl"));
        let outcome = init(temp.path(), &[]);
        assert_eq!(outcome.exit_code, 1);
        assert!(outcome.stdout.is_empty());
        assert_eq!(
            text(&outcome.stderr),
            "Error: Refusing to initialize: line 1 of .luuna/missions/issues.jsonl holds data and config.yaml is missing, and initializing would empty the file. Nothing was written.\n"
        );
        assert_eq!(
            fs::read(dir.join("issues.jsonl")).unwrap(),
            b"{\"id\":\"x\"}\n"
        );
        #[cfg(unix)]
        assert_eq!(mode(&dir.join("issues.jsonl")), before);
        let names: Vec<String> = fs::read_dir(&dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["issues.jsonl".to_owned()]);
        assert!(!temp.path().join(".gitattributes").exists());
    }

    #[test]
    fn init_errors_follow_go() {
        let temp = tempfile::tempdir().unwrap();
        let outcome = init(temp.path(), &["extra"]);
        assert_eq!(outcome.exit_code, 1);
        assert_eq!(
            text(&outcome.stderr),
            "Error: too many arguments for 'init'\n"
        );
        let outcome = init(temp.path(), &["extra", "--json"]);
        assert_eq!(
            text(&outcome.stdout),
            "{\n  \"success\": false,\n  \"command\": \"init\",\n  \"error\": \"too many arguments for 'init'\"\n}\n"
        );
        // init has no --format: an unknown option, never JSON (SD6).
        let outcome = init(temp.path(), &["--format", "json", "--json"]);
        assert_eq!(outcome.exit_code, 1);
        assert!(outcome.stdout.is_empty());
        assert_eq!(text(&outcome.stderr), "error: unknown option '--format'\n");
        assert!(!temp.path().join(".luuna").exists());
    }

    #[test]
    fn init_refuses_other_store_dirs_and_answers_legacy_folders_via_config_tree() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        // An explicit store directory other than the one Go's init creates.
        let invocation = Invocation::new(root, ["init"]).with_store_dir(root.join("elsewhere"));
        assert!(matches!(run(&invocation), Err(Error::Unsupported { .. })));
        assert!(!root.join("elsewhere").exists() && !root.join(".luuna").exists());
        // The one it creates is accepted.
        let invocation =
            Invocation::new(root, ["init", "--json"]).with_store_dir(root.join(MISSIONS_DIR));
        assert_eq!(run(&invocation).unwrap().exit_code, 0);
        // A legacy .seeds folder (s4-missions-lib-3).
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir(temp.path().join(".seeds")).unwrap();
        crate::config_tree::checks::init_with_legacy_folder(temp.path());
    }
    fn empty_store() -> tempfile::TempDir {
        let t = tempfile::tempdir().unwrap();
        fs::create_dir_all(t.path().join(MISSIONS_DIR)).unwrap();
        t
    }
    fn init_tree(
        root: &Path,
    ) -> std::collections::BTreeMap<String, (Vec<u8>, fs::Permissions, u8)> {
        fn walk(
            root: &Path,
            path: &Path,
            out: &mut std::collections::BTreeMap<String, (Vec<u8>, fs::Permissions, u8)>,
        ) {
            let meta = fs::symlink_metadata(path).unwrap();
            let (bytes, kind) = if meta.file_type().is_symlink() {
                (
                    fs::read_link(path)
                        .unwrap()
                        .to_string_lossy()
                        .as_bytes()
                        .to_vec(),
                    2,
                )
            } else if meta.is_dir() {
                (vec![], 1)
            } else {
                (fs::read(path).unwrap(), 0)
            };
            out.insert(
                path.strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned(),
                (bytes, meta.permissions(), kind),
            );
            if kind == 1 {
                for entry in fs::read_dir(path).unwrap() {
                    walk(root, &entry.unwrap().path(), out)
                }
            }
        }
        let mut out = std::collections::BTreeMap::new();
        walk(root, root, &mut out);
        out
    }
    fn text_error(error: Failure) -> String {
        match error {
            Failure::Message(s) => s,
            other => panic!("expected message, got {other:?}"),
        }
    }
    fn assert_init_loss(root: &Path, message: &str, json: bool) {
        let before = init_tree(root);
        let out = init(root, if json { &["--json"] } else { &[] });
        assert_eq!(out.exit_code, 1);
        if json {
            let expected = format!(
                "{{\n  \"success\": false,\n  \"command\": \"init\",\n  \"error\": {}\n}}\n",
                serde_json::to_string(message).unwrap()
            );
            assert_eq!(out.stdout, expected.as_bytes());
            assert!(out.stderr.is_empty());
        } else {
            assert!(out.stdout.is_empty());
            assert_eq!(out.stderr, format!("Error: {message}\n").as_bytes());
        }
        assert_eq!(init_tree(root), before);
    }
    #[test]
    fn init_content_rules_follow_what_go_would_lose() {
        for file in ["issues.jsonl", "templates.jsonl", "plans.jsonl"] {
            for (raw, line) in [
                (b"\n  \n".as_slice(), None),
                (b"{\"id\":\"x\"}", Some(1)),
                (b"\n{}", Some(2)),
            ] {
                let t = empty_store();
                let d = t.path().join(MISSIONS_DIR);
                fs::write(d.join(file), raw).unwrap();
                if let Some(line) = line {
                    assert_eq!(text_error(refuse_lossy_init(&d).unwrap_err()),format!("Refusing to initialize: line {line} of .luuna/missions/{file} holds data and config.yaml is missing, and initializing would empty the file. Nothing was written."));
                } else {
                    refuse_lossy_init(&d).unwrap();
                }
            }
        }
        for raw in [b"".as_slice(), b"*.lock\n", b"*.lock", b"\n *.lock \r\n"] {
            let t = empty_store();
            let d = t.path().join(MISSIONS_DIR);
            fs::write(d.join(".gitignore"), raw).unwrap();
            refuse_lossy_init(&d).unwrap();
        }
        let t = empty_store();
        let d = t.path().join(MISSIONS_DIR);
        fs::write(d.join(".gitignore"), b"*.lock\nmine\n").unwrap();
        assert!(text_error(refuse_lossy_init(&d).unwrap_err()).contains("line 2"));
        let t = empty_store();
        let d = t.path().join(MISSIONS_DIR);
        fs::create_dir(d.join("issues.jsonl")).unwrap();
        refuse_lossy_init(&d).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let t = empty_store();
            let d = t.path().join(MISSIONS_DIR);
            let p = d.join("issues.jsonl");
            fs::write(&p, b"").unwrap();
            fs::set_permissions(&p, fs::Permissions::from_mode(0o200)).unwrap();
            refuse_lossy_init(&d).unwrap();
        }
        for (raw, ignore, want) in [
            (
                "\u{85}\u{a0}\u{1680}\u{2000}\u{2028}\u{202f}\u{205f}\u{3000}".as_bytes(),
                false,
                None,
            ),
            (b"\n  kept", false, Some(2)),
            (b"*.lock  \n", true, None),
            (b"* .lock", true, Some(1)),
            (b"*.lockx", true, Some(1)),
            (b"\xc2", false, Some(1)),
        ] {
            assert_eq!(
                init_content_line(std::io::Cursor::new(raw), ignore).unwrap(),
                want
            );
        }
        let mut boundary = vec![b' '; 8191];
        boundary.extend_from_slice("\u{85}\nkept".as_bytes());
        assert_eq!(
            init_content_line(std::io::Cursor::new(boundary), false).unwrap(),
            Some(2)
        );
        let base = std::env::var_os("LUUNA_MISSIONS_TEST_DISK_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        fs::create_dir_all(&base).unwrap();
        let large = tempfile::tempdir_in(&base).unwrap();
        let d = large.path().join(MISSIONS_DIR);
        fs::create_dir_all(&d).unwrap();
        let p = d.join("issues.jsonl");
        {
            use std::io::Write;
            let mut file = fs::File::create(&p).unwrap();
            let spaces = [b' '; 8192];
            for _ in 0..2048 {
                file.write_all(&spaces).unwrap();
            }
            file.write_all(b"kept").unwrap();
        }
        assert!(text_error(refuse_lossy_init(&d).unwrap_err()).contains("line 1"));
        assert_eq!(fs::metadata(p).unwrap().len(), 16 * 1024 * 1024 + 4);
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            if Path::new("/dev/zero").exists() {
                let t = empty_store();
                let d = t.path().join(MISSIONS_DIR);
                symlink("/dev/zero", d.join("issues.jsonl")).unwrap();
                refuse_lossy_init(&d).unwrap();
                assert_eq!(
                    fs::read_link(d.join("issues.jsonl")).unwrap(),
                    Path::new("/dev/zero")
                );
            }
        }
    }
    #[test]
    fn init_refuses_each_file_go_would_empty_or_replace_unchanged() {
        for file in ["templates.jsonl", "plans.jsonl", ".gitignore"] {
            let t = empty_store();
            let p = t.path().join(MISSIONS_DIR).join(file);
            fs::write(&p, b"kept data\n").unwrap();
            let message = if file == ".gitignore" {
                "Refusing to initialize: line 1 of .luuna/missions/.gitignore holds an entry other than *.lock, and initializing would replace the file with *.lock alone. Nothing was written.".into()
            } else {
                format!("Refusing to initialize: line 1 of .luuna/missions/{file} holds data and config.yaml is missing, and initializing would empty the file. Nothing was written.")
            };
            assert_init_loss(t.path(), &message, file == "plans.jsonl");
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for file in [
                "issues.jsonl",
                "templates.jsonl",
                "plans.jsonl",
                ".gitignore",
            ] {
                let t = empty_store();
                let p = t.path().join(MISSIONS_DIR).join(file);
                fs::write(&p, b"private user data\n").unwrap();
                fs::set_permissions(&p, fs::Permissions::from_mode(0o200)).unwrap();
                let unreadable = fs::read(&p).is_err();
                let out = init(t.path(), &[]);
                assert_eq!(out.exit_code, 1);
                if unreadable {
                    assert_eq!(out.stderr,format!("Error: Refusing to initialize: .luuna/missions/{file} is nonempty but cannot be read while config.yaml is missing, so initializing could overwrite unchecked user data. Nothing was written.\n").as_bytes());
                }
                assert_eq!(mode(&p), 0o200);
                assert_eq!(fs::read_dir(p.parent().unwrap()).unwrap().count(), 1);
                assert!(!t.path().join(".gitattributes").exists());
                fs::set_permissions(&p, fs::Permissions::from_mode(0o600)).unwrap();
                assert_eq!(fs::read(&p).unwrap(), b"private user data\n");
            }
        }
    }
    #[test]
    fn init_refuses_through_symbolic_links_unchanged() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            for file in ["issues.jsonl", ".gitignore"] {
                let t = empty_store();
                let outside = tempfile::tempdir().unwrap();
                let target = outside.path().join("data");
                fs::write(&target, b"kept\n").unwrap();
                symlink(&target, t.path().join(MISSIONS_DIR).join(file)).unwrap();
                let root_before = init_tree(t.path());
                let outside_before = init_tree(outside.path());
                let out = init(t.path(), &[]);
                assert_eq!(out.exit_code, 1);
                assert_eq!(init_tree(t.path()), root_before);
                assert_eq!(init_tree(outside.path()), outside_before);
            }
            let t = tempfile::tempdir().unwrap();
            let outside = tempfile::tempdir().unwrap();
            fs::create_dir(t.path().join(".luuna")).unwrap();
            fs::write(outside.path().join("issues.jsonl"), b"kept\n").unwrap();
            symlink(outside.path(), t.path().join(MISSIONS_DIR)).unwrap();
            let a = init_tree(t.path());
            let b = init_tree(outside.path());
            assert_eq!(init(t.path(), &["--json"]).exit_code, 1);
            assert_eq!(init_tree(t.path()), a);
            assert_eq!(init_tree(outside.path()), b);
        }
    }
    #[test]
    fn init_keeps_doing_what_go_does_without_loss() {
        for (file, raw) in [
            ("issues.jsonl", b"\n  \n".as_slice()),
            (".gitignore", b"*.lock\n"),
        ] {
            let t = empty_store();
            fs::write(t.path().join(MISSIONS_DIR).join(file), raw).unwrap();
            assert_eq!(init(t.path(), &[]).exit_code, 0);
            assert!(t.path().join(MISSIONS_DIR).join("config.yaml").exists());
        }
        let t = empty_store();
        let d = t.path().join(MISSIONS_DIR);
        fs::write(d.join("config.yaml"), b"project: p\nversion: \"1\"\n").unwrap();
        fs::write(d.join("issues.jsonl"), b"kept\n").unwrap();
        assert_eq!(init(t.path(), &[]).exit_code, 0);
        assert_eq!(fs::read(d.join("issues.jsonl")).unwrap(), b"kept\n");
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            let t = empty_store();
            let d = t.path().join(MISSIONS_DIR);
            symlink(t.path().join("missing/config.yaml"), d.join("config.yaml")).unwrap();
            let before = init_tree(t.path());
            assert!(matches!(
                run(&Invocation::new(t.path(), ["init"])),
                Err(Error::Io { .. })
            ));
            assert_eq!(init_tree(t.path()), before);
        }
    }
    #[test]
    fn init_leaves_the_parent_of_the_temporary_project_unchanged() {
        let parent = tempfile::tempdir().unwrap();
        let root = parent.path().join("project");
        fs::create_dir(&root).unwrap();
        fs::create_dir(parent.path().join(".luuna")).unwrap();
        fs::write(parent.path().join("sentinel"), b"parent data").unwrap();
        fs::write(parent.path().join(".gitattributes"), b"parent attrs\n").unwrap();
        let parent_snapshot = || {
            let mut entries = init_tree(parent.path());
            entries.retain(|p, _| p != "project" && !p.starts_with("project/"));
            entries
        };
        let before = parent_snapshot();
        let d = root.join(MISSIONS_DIR);
        fs::create_dir_all(&d).unwrap();
        fs::write(d.join("issues.jsonl"), b"kept\n").unwrap();
        assert_eq!(init(&root, &[]).exit_code, 1);
        assert_eq!(parent_snapshot(), before);
        fs::write(d.join("issues.jsonl"), b"").unwrap();
        assert_eq!(init(&root, &[]).exit_code, 0);
        assert_eq!(parent_snapshot(), before);
    }
}
