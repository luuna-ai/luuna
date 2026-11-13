//! Go's explicit sync command, including its full-index commit behavior.
use crate::{
    command::{options, output::Envelope, Context, Failure},
    config, gopath, model,
    store::trim_space,
};
use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
};
#[cfg(unix)]
fn git(context: &Context<'_>) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    let path = context.env("PATH")?;
    if path.is_empty() {
        return None;
    }
    for dir in std::env::split_paths(path) {
        let dir = if dir.as_os_str().is_empty() {
            PathBuf::from(".")
        } else {
            dir
        };
        let path = dir.join("git");
        if let Ok(m) = std::fs::metadata(context.cwd.join(&path)) {
            if !m.is_dir() && m.permissions().mode() & 0o111 != 0 {
                return path.is_absolute().then_some(path);
            }
        }
    }
    None
}
#[cfg(not(unix))]
fn git(context: &Context<'_>) -> Option<PathBuf> {
    context.env("PATH")?;
    Some(PathBuf::from("git"))
}
fn capture(
    context: &Context<'_>,
    root: &Path,
    args: &[&str],
    with_c: bool,
) -> (bool, Vec<u8>, Vec<u8>) {
    let Some(git) = git(context) else {
        return (false, Vec::new(), Vec::new());
    };
    let mut command = Command::new(git);
    if with_c {
        command.arg("-C").arg(root);
    }
    command
        .args(args)
        .current_dir(root)
        .env_clear()
        .envs(&context.invocation.env)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    command.env("PWD", root);
    match command.output() {
        Ok(o) => (o.status.success(), o.stdout, o.stderr),
        Err(_) => (false, Vec::new(), Vec::new()),
    }
}
fn text(bytes: &[u8]) -> String {
    let mut out = String::new();
    let mut i = 0;
    while i < bytes.len() {
        let (c, n) = crate::json::decode_char(&bytes[i..]);
        out.push(c.unwrap_or('\u{fffd}'));
        i += n;
    }
    out
}
fn path(context: &Context<'_>, cwd: &Path, arg: &str) -> Option<PathBuf> {
    let (ok, out, _) = capture(context, cwd, &["rev-parse", arg], false);
    if !ok {
        return None;
    }
    let out = trim_space(&out);
    if out.is_empty() {
        return None;
    }
    let p = PathBuf::from(text(out));
    Some(gopath::clean(&if p.is_absolute() {
        p
    } else {
        cwd.join(p)
    }))
}
fn worktree(context: &Context<'_>) -> bool {
    let (Some(git), Some(common)) = (
        path(context, &context.cwd, "--git-dir"),
        path(context, &context.cwd, "--git-common-dir"),
    ) else {
        return false;
    };
    if cfg!(windows) {
        git.to_string_lossy().to_lowercase() != common.to_string_lossy().to_lowercase()
    } else {
        git != common
    }
}
pub(super) fn run(context: &mut Context<'_>, args: &[String]) -> Result<(), Failure> {
    let opts = options::parse(args, &[], &["status", "dry-run", "json"])?;
    if !opts.positionals.is_empty() {
        return Err(Failure::message("too many arguments for 'sync'"));
    }
    let store = context.store()?;
    let root = config::project_root(store.dir());
    if worktree(context) {
        let message =
            "Inside a git worktree — skipping commit. Issues are stored in the main repo.";
        if opts.has("json") {
            context.json(
                Envelope::new(true, "sync")
                    .boolean("committed", false)
                    .boolean("worktree", true)
                    .string("message", message),
            );
        } else {
            context.warning(message.as_bytes());
        }
        return Ok(());
    }
    let (ok, out, err) = capture(
        context,
        &root,
        &["status", "--porcelain", ".luuna/missions/"],
        true,
    );
    if !ok {
        return Err(Failure::message(format!(
            "git status failed: {}",
            text(trim_space(&err))
        )));
    }
    let changed = trim_space(&out);
    let changed_text = text(changed);
    if opts.has("status") {
        if opts.has("json") {
            context.json(
                Envelope::new(true, "sync")
                    .boolean("hasChanges", !changed.is_empty())
                    .string("changes", &changed_text),
            );
        } else if changed.is_empty() {
            context.success(b"No uncommitted .luuna/missions/ changes.");
        } else {
            context.success(b"Uncommitted .luuna/missions/ changes:");
            context.stdout.extend_from_slice(changed);
            context.stdout.push(b'\n');
        }
        return Ok(());
    }
    if changed.is_empty() {
        if opts.has("json") {
            context.json(
                Envelope::new(true, "sync")
                    .boolean("committed", false)
                    .string("message", "Nothing to commit"),
            );
        } else {
            context.success(b"No changes to commit.");
        }
        return Ok(());
    }
    let now = model::now_iso();
    let message = format!("missions: sync {}", &now[..10]);
    if opts.has("dry-run") {
        if opts.has("json") {
            context.json(
                Envelope::new(true, "sync")
                    .boolean("dryRun", true)
                    .boolean("wouldCommit", true)
                    .string("message", &message)
                    .string("changes", &changed_text),
            );
        } else {
            context.success("Dry run — would commit:".as_bytes());
            context.stdout.extend_from_slice(changed);
            context
                .stdout
                .extend_from_slice(format!("\nCommit message: {message}\n").as_bytes());
        }
        return Ok(());
    }
    let (ok, _, err) = capture(context, &root, &["add", ".luuna/missions/"], true);
    if !ok {
        return Err(Failure::message(format!("git add failed: {}", text(&err))));
    }
    let (ok, _, err) = capture(context, &root, &["commit", "-m", &message], true);
    if !ok {
        return Err(Failure::message(format!(
            "git commit failed: {}",
            text(&err)
        )));
    }
    if opts.has("json") {
        context.json(
            Envelope::new(true, "sync")
                .boolean("committed", true)
                .string("message", &message),
        );
    } else {
        context.success(format!("Committed: {message}").as_bytes());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::{run, Invocation, Outcome};
    use std::{collections::BTreeMap, fs};
    struct Repo {
        root: PathBuf,
        env: BTreeMap<String, String>,
    }
    impl Repo {
        fn at(root: PathBuf) -> Self {
            fs::create_dir_all(&root).unwrap();
            crate::config_tree::tree::test_support::store(&root, "project: demo\nversion: \"1\"\n");
            fs::write(root.join(".luuna/missions/issues.jsonl"), b"").unwrap();
            for dir in [".test-home", ".test-hooks", ".test-template"] {
                fs::create_dir_all(root.join(dir)).unwrap();
            }
            let env = BTreeMap::from([
                ("PATH".into(), std::env::var("PATH").unwrap()),
                (
                    "HOME".into(),
                    root.join(".test-home").to_string_lossy().into_owned(),
                ),
                (
                    "GIT_CONFIG_GLOBAL".into(),
                    root.join(".test-home/config")
                        .to_string_lossy()
                        .into_owned(),
                ),
                ("GIT_CONFIG_NOSYSTEM".into(), "1".into()),
                (
                    "GIT_TEMPLATE_DIR".into(),
                    root.join(".test-template").to_string_lossy().into_owned(),
                ),
                ("GIT_AUTHOR_NAME".into(), "Example Agent".into()),
                ("GIT_COMMITTER_NAME".into(), "Example Agent".into()),
                ("GIT_AUTHOR_EMAIL".into(), "agent@example.invalid".into()),
                ("GIT_COMMITTER_EMAIL".into(), "agent@example.invalid".into()),
                ("GIT_AUTHOR_DATE".into(), "2026-01-01T00:00:00Z".into()),
                ("GIT_COMMITTER_DATE".into(), "2026-01-01T00:00:00Z".into()),
                ("GIT_CONFIG_COUNT".into(), "2".into()),
                ("GIT_CONFIG_KEY_0".into(), "commit.gpgsign".into()),
                ("GIT_CONFIG_VALUE_0".into(), "false".into()),
                ("GIT_CONFIG_KEY_1".into(), "core.hooksPath".into()),
                (
                    "GIT_CONFIG_VALUE_1".into(),
                    root.join(".test-hooks").to_string_lossy().into_owned(),
                ),
            ]);
            let r = Self { root, env };
            r.git(&["init", "--initial-branch=main"]);
            r.git(&["add", ".luuna/missions/"]);
            r.git(&["commit", "-m", "base"]);
            r
        }
        fn git(&self, args: &[&str]) -> String {
            let out = Command::new("git")
                .args(args)
                .current_dir(&self.root)
                .env_clear()
                .envs(&self.env)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "git failed: {}",
                String::from_utf8_lossy(&out.stderr)
            );
            String::from_utf8(out.stdout).unwrap()
        }
        fn invoke(&self, cwd: &Path, args: &[&str]) -> Outcome {
            run(&Invocation::new(cwd, args.iter().copied()).with_env(self.env.clone())).unwrap()
        }
        fn dirty(&self) {
            fs::write(
                self.root.join(".luuna/missions/issues.jsonl"),
                b"{\"id\":\"a\"}\n",
            )
            .unwrap();
        }
    }
    #[test]
    fn sync_preserves_full_index_commit() {
        let temp = tempfile::tempdir().unwrap();
        let r = Repo::at(temp.path().join("project"));
        fs::write(r.root.join("outside.txt"), b"staged").unwrap();
        r.git(&["add", "outside.txt"]);
        r.dirty();
        let out = r.invoke(&r.root, &["sync", "--json"]);
        assert_eq!(out.exit_code, 0);
        let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(v["committed"], true);
        assert!(v["message"]
            .as_str()
            .unwrap()
            .starts_with("missions: sync "));
        let files = r.git(&["show", "--pretty=format:", "--name-only", "HEAD"]);
        assert!(files.contains("outside.txt"));
        assert!(files.contains(".luuna/missions/issues.jsonl"));
        assert!(r.git(&["status", "--porcelain"]).is_empty());
    }
    #[test]
    fn status_and_dry_run_change_nothing() {
        let temp = tempfile::tempdir().unwrap();
        let r = Repo::at(temp.path().join("project"));
        r.dirty();
        let head = r.git(&["rev-parse", "HEAD"]);
        let index = r.git(&["ls-files", "--stage"]);
        let out = r.invoke(&r.root, &["sync", "--status", "--json"]);
        let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(v["hasChanges"], true);
        assert_eq!(v["changes"], "M .luuna/missions/issues.jsonl");
        let out = r.invoke(&r.root, &["sync", "--dry-run", "--json"]);
        let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(v["wouldCommit"], true);
        assert_eq!(head, r.git(&["rev-parse", "HEAD"]));
        assert_eq!(index, r.git(&["ls-files", "--stage"]));
    }
    #[test]
    fn inside_a_linked_worktree_skips_the_commit() {
        let temp = tempfile::tempdir().unwrap();
        let r = Repo::at(temp.path().join("project"));
        let linked = temp.path().join("linked");
        r.git(&["worktree", "add", "-b", "linked", linked.to_str().unwrap()]);
        r.dirty();
        let head = r.git(&["rev-parse", "HEAD"]);
        let out = r.invoke(&linked, &["sync", "--json"]);
        let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(v["worktree"], true);
        assert_eq!(v["committed"], false);
        assert_eq!(head, r.git(&["rev-parse", "HEAD"]));
        assert_eq!(
            fs::read(r.root.join(".luuna/missions/issues.jsonl")).unwrap(),
            b"{\"id\":\"a\"}\n"
        );
    }
    #[cfg(unix)]
    #[test]
    fn git_failures_keep_go_s_trimming() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().unwrap();
        let mut r = Repo::at(temp.path().join("project"));
        let bin = r.root.join("bin");
        fs::create_dir(&bin).unwrap();
        let file = bin.join("git");
        fs::write(&file,r#"#!/bin/sh
if [ "$1" = rev-parse ]; then exit 1; fi
case "$3" in
status) if [ "$LUUNA_TEST_GIT_FAIL" = status ]; then printf '  STATUS  \n' >&2; exit 1; fi; printf ' M .luuna/missions/issues.jsonl\n';;
add) if [ "$LUUNA_TEST_GIT_FAIL" = add ]; then printf '  ADD  \n' >&2; exit 1; fi;;
commit) printf '  COMMIT  \n' >&2; exit 1;;
esac
"#).unwrap();
        fs::set_permissions(&file, fs::Permissions::from_mode(0o755)).unwrap();
        r.env
            .insert("PATH".into(), bin.to_string_lossy().into_owned());
        for (mode, expected) in [
            ("status", "Error: git status failed: STATUS\n"),
            ("add", "Error: git add failed:   ADD  \n\n"),
            ("commit", "Error: git commit failed:   COMMIT  \n\n"),
        ] {
            r.env.insert("LUUNA_TEST_GIT_FAIL".into(), mode.into());
            let out = r.invoke(&r.root, &["sync"]);
            assert_eq!(out.exit_code, 1);
            assert_eq!(out.stderr, expected.as_bytes());
        }
    }
    #[cfg(unix)]
    #[test]
    fn relative_path_executable_is_checked_at_invocation_cwd_and_refused() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().unwrap();
        let mut r = Repo::at(temp.path().join("project"));
        let bin = r.root.join("bin");
        fs::create_dir(&bin).unwrap();
        let fake = bin.join("git");
        fs::write(&fake, b"#!/bin/sh\nexit 0\n").unwrap();
        fs::set_permissions(fake, fs::Permissions::from_mode(0o755)).unwrap();
        let original = r.env.get("PATH").cloned().unwrap();
        r.env.insert("PATH".into(), format!("bin:{original}"));
        let out = r.invoke(&r.root, &["sync", "--status", "--json"]);
        assert_eq!(out.exit_code, 1);
        assert_eq!(out.stderr, b"");
        let value: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(value["error"], "git status failed: ");
        r.env.insert("PATH".into(), original);
        assert!(r.git(&["status", "--porcelain"]).contains("bin/"));
    }
    #[test]
    fn sync_leaves_the_parent_of_the_temporary_project_unchanged() {
        let parent = tempfile::tempdir().unwrap();
        let sentinel = parent.path().join("sentinel");
        fs::write(&sentinel, b"parent").unwrap();
        let mode = fs::metadata(&sentinel).unwrap().permissions();
        let r = Repo::at(parent.path().join("project"));
        r.dirty();
        assert_eq!(r.invoke(&r.root, &["sync"]).exit_code, 0);
        assert_eq!(fs::read(&sentinel).unwrap(), b"parent");
        assert_eq!(fs::metadata(sentinel).unwrap().permissions(), mode);
        assert_eq!(fs::read_dir(parent.path()).unwrap().count(), 2);
        assert!(!parent.path().join(".git").exists());
        assert!(!parent.path().join(".luuna").exists());
    }
}
