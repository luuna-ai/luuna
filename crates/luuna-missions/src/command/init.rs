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
    fn init_without_config_yaml_truncates_the_data_files() {
        // Go writes every data file again when config.yaml is missing.
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join(MISSIONS_DIR);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("issues.jsonl"), "{\"id\":\"x\"}\n").unwrap();
        let outcome = init(temp.path(), &[]);
        assert!(text(&outcome.stdout).starts_with("✓ Initialized"));
        assert_eq!(fs::read(dir.join("issues.jsonl")).unwrap(), b"");
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
}
