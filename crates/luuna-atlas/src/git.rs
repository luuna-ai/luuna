//! The git runner, repository and changed-file helpers, directory anchors and the store root
//! resolver. Owner: s5-atlas-lib-2.
//!
//! A port of the Go tool's `git.go` (`Git` to `FilterByContext`), of `gitCommonDir`,
//! `IsInsideWorktree` and `resolveStoreRoot` (`config.go:43-80`) and of `FindRepoRoot`
//! (`hooks_inject.go:26-39`).
//!
//! `git` is found with [`Env::look_path`] on the given environment's `PATH` (P25) and runs
//! with exactly that environment's variables (D2). Git failures are errors here; the
//! helpers swallow them as the Go tool does. Output is read without `-z`, so git quotes
//! unusual paths (a suspected Go defect, kept).

use std::path::{Path, PathBuf};

use crate::error::{AtlasError, Result};
use crate::options::Env;
use crate::store::Record;

/// `Git(cwd, args...)`: runs `git args` in `cwd` with stdin from the null device, stdout
/// captured (non-UTF-8 bytes replaced by U+FFFD), stderr captured and dropped, no
/// timeout, and a child environment of exactly `env`'s variables. Git not found on
/// `env`'s `PATH`, a start failure or a non-zero exit is an error.
pub fn git(cwd: &Path, env: &Env, args: &[&str]) -> Result<String> {
    // stub until t6
    let _ = (cwd, env, args);
    Err(AtlasError::Message("not implemented: git::git".to_owned()))
}

/// `IsGitRepo`: `git rev-parse --is-inside-work-tree` succeeds.
pub fn is_git_repo(cwd: &Path, env: &Env) -> bool {
    // stub until t6
    let _ = (cwd, env);
    false
}

/// `GetCurrentCommit`: the trimmed `git rev-parse HEAD`, or `""` on any failure.
pub fn get_current_commit(cwd: &Path, env: &Env) -> String {
    // stub until t6
    let _ = (cwd, env);
    String::new()
}

/// `GetChangedFiles(cwd, since)`: the files of `git diff --name-only --end-of-options
/// <since>`, `--cached` and the working tree, de-duplicated and sorted by bytes; a failing
/// git call contributes nothing.
pub fn get_changed_files(cwd: &Path, env: &Env, since: &str) -> Vec<String> {
    // stub until t6
    let _ = (cwd, env, since);
    Vec::new()
}

/// `GetContextFiles`: staged and unstaged changed files, de-duplicated and sorted.
pub fn get_context_files(cwd: &Path, env: &Env) -> Vec<String> {
    // stub until t6
    let _ = (cwd, env);
    Vec::new()
}

/// `GetActiveFiles`: the paths of `git status --porcelain -uall` (the new name of a
/// rename), de-duplicated and sorted; empty on failure.
pub fn get_active_files(cwd: &Path, env: &Env) -> Vec<String> {
    // stub until t6
    let _ = (cwd, env);
    Vec::new()
}

/// `gitCommonDir`: the absolute `git rev-parse --git-common-dir` (joined to `cwd` when
/// relative), or `None` on failure or empty output.
pub fn git_common_dir(cwd: &Path, env: &Env) -> Option<PathBuf> {
    // stub until t6
    let _ = (cwd, env);
    None
}

/// `IsInsideWorktree`: the common directory ends in `.git` and its parent is not `cwd`.
pub fn is_inside_worktree(cwd: &Path, env: &Env) -> bool {
    // stub until t6
    let _ = (cwd, env);
    false
}

/// `resolveStoreRoot(cwd)`: from a linked worktree, the main checkout when it holds a
/// store (`.luuna/atlas/config.yaml` or a legacy store); otherwise `cwd`. For -5's front
/// end (D1: commands take the root as a parameter). A crafted `.git` can redirect it (a
/// suspected Go defect, kept).
pub fn resolve_store_root(cwd: &Path, env: &Env) -> PathBuf {
    // stub until t6: cwd, as Go returns when git fails.
    let _ = env;
    cwd.to_owned()
}

/// `FindRepoRoot(cwd)`: the nearest of the absolute `cwd` and its ancestors where
/// [`is_git_repo`] holds, else the absolute `cwd`.
pub fn find_repo_root(cwd: &Path, env: &Env) -> PathBuf {
    // stub until t6
    let _ = env;
    cwd.to_owned()
}

/// `FileMatchesAny(file, changed)`: equal, or one ends with `/` plus the other.
pub fn file_matches_any(file: &str, changed: &[String]) -> bool {
    // stub until t6
    let _ = (file, changed);
    false
}

/// `FileLivesUnderDir(file, dir)`: with backslash runs in `file` turned into `/` and
/// `dir` normalised, `dir` is empty, equals `file` or is a `/`-bounded prefix of it.
pub fn file_lives_under_dir(file: &str, dir: &str) -> bool {
    // stub until t6
    let _ = (file, dir);
    false
}

/// `NormalizeDirAnchor(input)`: refuses an absolute path (`/`, `\` or a drive letter) and
/// any `..` segment with Go's two messages (`git.go:115,118`); otherwise the trimmed
/// anchor with backslash runs turned into `/`, trailing slashes and one leading `./`
/// removed, and `.` as `""`.
pub fn normalize_dir_anchor(input: &str) -> Result<String> {
    // stub until t6
    let _ = input;
    Err(AtlasError::Message(
        "not implemented: git::normalize_dir_anchor".to_owned(),
    ))
}

/// `NormalizeDirAnchors(inputs)`: each normalised, empty results dropped, de-duplicated
/// in order; the first refusal is the error.
pub fn normalize_dir_anchors(inputs: &[String]) -> Result<Vec<String>> {
    // stub until t6
    let _ = inputs;
    Err(AtlasError::Message(
        "not implemented: git::normalize_dir_anchors".to_owned(),
    ))
}

/// `InferDirAnchors(files)`: every parent directory holding at least three of `files`,
/// sorted by bytes.
pub fn infer_dir_anchors(files: &[String]) -> Vec<String> {
    // stub until t6
    let _ = files;
    Vec::new()
}

/// `HasFileAnchors(r)`: the record lists `files` or `dir_anchors` strings.
pub fn has_file_anchors(record: &Record) -> bool {
    // stub until t6
    let _ = record;
    false
}

/// `MatchesFileAnchors(r, files)`: one of the record's `files` matches one of `files`
/// ([`file_matches_any`]) or one of `files` lives under one of its `dir_anchors`.
pub fn matches_file_anchors(record: &Record, files: &[String]) -> bool {
    // stub until t6
    let _ = (record, files);
    false
}

/// `FilterByContext(records, files)`: the records without file anchors and those whose
/// anchors match `files`, in order (cloned).
pub fn filter_by_context(records: &[Record], files: &[String]) -> Vec<Record> {
    // stub until t6
    let _ = (records, files);
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::options::test_support::{git_vars, setup_git, write_script};
    use std::ffi::OsString;
    use std::fs;

    fn strings(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn normalize_dir_anchor_refuses_absolute_and_parent() {
        for input in ["/abs", " /abs", "\\abs", "C:\\x", "c:/x"] {
            let err = normalize_dir_anchor(input).expect_err(input);
            assert_eq!(
                err.to_string(),
                format!(
                    "dir-anchor {input:?} is an absolute path. Use a project-root-relative path like \"src/utils\"."
                )
            );
        }
        for input in ["..", "a/../b", "a\\..\\b", "src/..", "../x"] {
            let err = normalize_dir_anchor(input).expect_err(input);
            assert_eq!(
                err.to_string(),
                format!(
                    "dir-anchor {input:?} contains \"..\" (parent traversal). Anchors must stay inside the project root."
                )
            );
        }
        let ok = |input: &str| normalize_dir_anchor(input).expect(input);
        assert_eq!(ok(" ./src/utils/ "), "src/utils");
        assert_eq!(ok("src\\\\utils\\"), "src/utils");
        assert_eq!(ok("."), "");
        assert_eq!(ok("./"), "");
        assert_eq!(ok(""), "");
        assert_eq!(ok("..foo/bar"), "..foo/bar");
        assert_eq!(
            normalize_dir_anchors(&strings(&["src/", "./src", "", ".", "lib"])).expect("ok"),
            strings(&["src", "lib"])
        );
        assert!(normalize_dir_anchors(&strings(&["src", "/abs"])).is_err());
    }

    #[test]
    fn infer_dir_anchors_needs_three() {
        let files = strings(&["src/a", "src/b", "src/c", "lib/a", "lib/b", "top", "/x"]);
        assert_eq!(infer_dir_anchors(&files), strings(&["src"]));
        let files = strings(&["b/1", "b/2", "b\\3", "a/x/1", "a/x/2", "a/x/3", "a/4"]);
        assert_eq!(infer_dir_anchors(&files), strings(&["a/x", "b"]));
        assert!(infer_dir_anchors(&[]).is_empty());
    }

    #[test]
    fn file_matches_any_boundary() {
        let changed = strings(&["src/a.ts", "lib/b.ts"]);
        assert!(file_matches_any("src/a.ts", &changed));
        assert!(file_matches_any("a.ts", &changed));
        assert!(!file_matches_any("rc/a.ts", &changed));
        assert!(file_matches_any("repo/lib/b.ts", &changed));
        assert!(!file_matches_any("xlib/b.ts", &changed));
        assert!(!file_matches_any("b", &changed));
        assert!(!file_matches_any("src/a.ts", &[]));
        assert!(file_lives_under_dir("src/x/y.rs", "src/x"));
        assert!(file_lives_under_dir("src\\x\\y.rs", "./src/x/"));
        assert!(!file_lives_under_dir("src/xy.rs", "src/x"));
        assert!(file_lives_under_dir("anything", "."));
        assert!(file_lives_under_dir("src/x", "src/x"));
    }

    fn env_in(home: &Path) -> Env {
        Env::fixed(git_vars(home), Vec::new(), false)
    }

    #[test]
    fn fixed_env_is_the_whole_child_env() {
        let home = tempfile::tempdir().expect("tempdir");
        let repo = tempfile::tempdir().expect("tempdir");
        setup_git(repo.path(), home.path(), &["init", "-q"]);
        // HOME comes from Env, so git reads the config file in the test's own home.
        fs::write(
            home.path().join(".gitconfig"),
            "[atlas]\n\tprobe = from-env-home\n",
        )
        .expect("write gitconfig");
        let env = env_in(home.path());
        let out = git(repo.path(), &env, &["config", "--get", "atlas.probe"]).expect("git");
        assert_eq!(out, "from-env-home\n");
        let out = git(repo.path(), &env, &["var", "GIT_AUTHOR_IDENT"]).expect("git");
        assert!(out.starts_with("Test <test@example.invalid> "), "{out}");
        // A variable Env does not hold is not seen, even if the test process had it:
        // without GIT_CONFIG_COUNT in Env, git sees no command-line config.
        let mut vars = git_vars(home.path());
        vars.push(("GIT_CONFIG_COUNT".into(), "1".into()));
        vars.push(("GIT_CONFIG_KEY_0".into(), "atlas.extra".into()));
        vars.push(("GIT_CONFIG_VALUE_0".into(), "yes".into()));
        let with = Env::fixed(vars, Vec::new(), false);
        let out = git(repo.path(), &with, &["config", "--get", "atlas.extra"]).expect("git");
        assert_eq!(out, "yes\n");
        assert!(git(repo.path(), &env, &["config", "--get", "atlas.extra"]).is_err());
        // A failing git is an error, stderr dropped.
        assert!(git(repo.path(), &env, &["no-such-subcommand"]).is_err());
    }

    #[test]
    fn git_is_found_on_env_path_only() {
        let empty = tempfile::tempdir().expect("tempdir");
        let stub_dir = tempfile::tempdir().expect("tempdir");
        let work = tempfile::tempdir().expect("tempdir");
        // No git on Env's PATH: an error, although the test process has git.
        let env = Env::fixed([("PATH", empty.path().as_os_str())], Vec::new(), false);
        assert!(git(work.path(), &env, &["--version"]).is_err());
        assert!(!is_git_repo(work.path(), &env));
        assert_eq!(get_current_commit(work.path(), &env), "");
        // A stub `git` on Env's PATH is the one that runs, in cwd.
        let marker = work.path().join("stub-ran");
        write_script(
            stub_dir.path(),
            "git",
            "printf 'stub %s\\n' \"$*\"; : > stub-ran",
        );
        let env = Env::fixed([("PATH", stub_dir.path().as_os_str())], Vec::new(), false);
        let out = git(work.path(), &env, &["rev-parse", "HEAD"]).expect("stub git");
        assert_eq!(out, "stub rev-parse HEAD\n");
        assert!(marker.exists());
        // A stub found only through a relative PATH element is not run.
        let rel: OsString =
            crate::options::test_support::relative_from_cwd(stub_dir.path()).into_os_string();
        fs::remove_file(&marker).expect("remove marker");
        let env = Env::fixed([("PATH", rel)], Vec::new(), false);
        assert!(git(work.path(), &env, &["--version"]).is_err());
        assert!(!marker.exists());
    }

    #[test]
    fn resolve_store_root_in_linked_worktree() {
        let home = tempfile::tempdir().expect("tempdir");
        let top = tempfile::tempdir().expect("tempdir");
        let top = top.path().canonicalize().expect("canonical");
        let main = top.join("main");
        fs::create_dir(&main).expect("mkdir main");
        setup_git(&main, home.path(), &["init", "-q"]);
        fs::write(main.join("f.txt"), "x\n").expect("write");
        setup_git(&main, home.path(), &["add", "f.txt"]);
        setup_git(&main, home.path(), &["commit", "-q", "-m", "c"]);
        setup_git(&main, home.path(), &["worktree", "add", "-q", "../wt"]);
        let wt = top.join("wt");
        let env = env_in(home.path());

        // Without a store in the main checkout, the worktree is its own root.
        assert_eq!(resolve_store_root(&wt, &env), wt);
        assert!(is_inside_worktree(&wt, &env));
        assert!(!is_inside_worktree(&main, &env));
        assert_eq!(git_common_dir(&wt, &env), Some(main.join(".git")));

        // With a store in the main checkout, the worktree resolves to it.
        fs::create_dir_all(main.join(".luuna").join("atlas")).expect("mkdir store");
        fs::write(
            main.join(".luuna").join("atlas").join("config.yaml"),
            "domains: {}\n",
        )
        .expect("write config");
        assert_eq!(resolve_store_root(&wt, &env), main);
        assert_eq!(resolve_store_root(&main, &env), main);
        // Outside any repository: cwd itself.
        let outside = top.join("outside");
        fs::create_dir(&outside).expect("mkdir");
        assert_eq!(resolve_store_root(&outside, &env), outside);
        assert_eq!(find_repo_root(&wt.join("."), &env), wt);
        assert_eq!(get_current_commit(&wt, &env).len(), 40);
    }
}
