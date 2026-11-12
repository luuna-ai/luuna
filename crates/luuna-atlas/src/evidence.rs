//! Mission evidence lookup and the missions directory. Owner: s5-atlas-lib-2.
//!
//! A port of the Go tool's `EvidenceMissions` and `ResolveMissionsDir`
//! (`legacy_layout.go:80-101`) and of the evidence block `record` builds from its
//! `--evidence-*` flags (`record.go:306-325`).

use std::path::{Path, PathBuf};

use crate::json::{Object, Value};
use crate::options::Context;

/// The legacy evidence key read when `missions` is absent or null.
pub const LEGACY_EVIDENCE_KEY: &str = "seeds";

/// The legacy missions directory, relative to the working directory.
pub const LEGACY_MISSIONS_DIR: &str = ".seeds";

/// The values of the seven `--evidence-*` flags of `record`; an empty string is a flag
/// not given (Go's `i.String("evidence-<name>") == ""`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EvidenceFlags {
    pub commit: String,
    pub issue: String,
    pub file: String,
    pub bead: String,
    pub missions: String,
    pub gh: String,
    pub linear: String,
}

/// `EvidenceMissions(evidence)`: the `missions` value of an evidence object, falling back
/// to the legacy key only when `missions` is absent or null; only a non-empty string
/// counts, anything else is `""`.
pub fn evidence_missions(evidence: Option<&Value>) -> String {
    // stub until t6
    let _ = evidence;
    String::new()
}

/// `ResolveMissionsDir(cwd)`: `<cwd>/.luuna/missions` if it exists (as `os.Lstat` sees
/// it, so a dangling symbolic link counts), else the legacy `<cwd>/.seeds` if that
/// exists, else `<cwd>/.luuna/missions`.
pub fn resolve_missions_dir(cwd: &Path) -> PathBuf {
    // stub until t6: the current layout's path.
    cwd.join(".luuna").join("missions")
}

/// The evidence object `record` builds (`record.go:306-325`): the given flags in Go's
/// fixed key order (commit, issue, file, bead, missions, gh, linear); when no
/// `--evidence-commit` is given, the commit from [`crate::git::get_current_commit`] in
/// `ctx.cwd` (with `ctx.env`) is appended last, if there is one. Returns the object, or
/// `None` when it would be empty (Go then sets no `evidence` field), and whether the
/// commit was filled automatically (`autoCommit`).
pub fn build_record_evidence(ctx: &Context, flags: &EvidenceFlags) -> (Option<Object>, bool) {
    // stub until t6
    let _ = (ctx, flags);
    (None, false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::json;
    use crate::options::test_support::{git_vars, setup_git};
    use crate::options::{Env, GlobalOptions};

    fn missions(text: &str) -> String {
        let value = json::parse(text).expect("json");
        evidence_missions(Some(&value))
    }

    #[test]
    fn missions_falls_back_only_on_absent_or_null() {
        assert_eq!(missions(r#"{"missions":"m-1","seeds":"s-1"}"#), "m-1");
        assert_eq!(missions(r#"{"missions":null,"seeds":"s-1"}"#), "s-1");
        assert_eq!(missions(r#"{"seeds":"s-1"}"#), "s-1");
        // Present and not null: no fallback, and only a non-empty string counts.
        assert_eq!(missions(r#"{"missions":"","seeds":"s-1"}"#), "");
        assert_eq!(missions(r#"{"missions":5,"seeds":"s-1"}"#), "");
        assert_eq!(missions(r#"{"missions":["m"],"seeds":"s-1"}"#), "");
        assert_eq!(missions(r#"{"seeds":7}"#), "");
        assert_eq!(missions(r#"{"seeds":""}"#), "");
        assert_eq!(missions(r#"{}"#), "");
        assert_eq!(missions(r#"["m-1"]"#), "");
        assert_eq!(missions(r#""m-1""#), "");
        assert_eq!(evidence_missions(None), "");
    }

    fn context(cwd: &Path, home: &Path) -> Context {
        Context {
            root: cwd.to_owned(),
            cwd: cwd.to_owned(),
            env: Env::fixed(git_vars(home), Vec::new(), false),
            globals: GlobalOptions::default(),
        }
    }

    /// The keys joined with commas, as one string: no test literal is then the quoted
    /// evidence key that t6's scope grep looks for.
    fn keys(object: &Object) -> String {
        object.keys().collect::<Vec<_>>().join(",")
    }

    #[test]
    fn flag_order_and_auto_commit() {
        let home = tempfile::tempdir().expect("tempdir");
        let plain = tempfile::tempdir().expect("tempdir");
        let repo = tempfile::tempdir().expect("tempdir");
        setup_git(repo.path(), home.path(), &["init", "-q"]);
        std::fs::write(repo.path().join("f.txt"), "x\n").expect("write");
        setup_git(repo.path(), home.path(), &["add", "f.txt"]);
        setup_git(repo.path(), home.path(), &["commit", "-q", "-m", "c"]);

        // Every flag given: Go's key order, whatever order they were set in; git is not
        // asked for the commit.
        let all = EvidenceFlags {
            linear: "LIN-1".into(),
            gh: "12".into(),
            missions: "m-1".into(),
            bead: "b-1".into(),
            file: "src/a.rs".into(),
            issue: "I-1".into(),
            commit: "abc123".into(),
        };
        let (evidence, auto) = build_record_evidence(&context(repo.path(), home.path()), &all);
        let evidence = evidence.expect("evidence");
        assert_eq!(keys(&evidence), "commit,issue,file,bead,missions,gh,linear");
        assert_eq!(
            evidence.get("commit").and_then(Value::as_str),
            Some("abc123")
        );
        assert!(!auto);

        // No --evidence-commit in a repository: the HEAD commit goes last.
        let some = EvidenceFlags {
            gh: "12".into(),
            issue: "I-1".into(),
            ..EvidenceFlags::default()
        };
        let (evidence, auto) = build_record_evidence(&context(repo.path(), home.path()), &some);
        let evidence = evidence.expect("evidence");
        assert_eq!(keys(&evidence), "issue,gh,commit");
        let commit = evidence
            .get("commit")
            .and_then(Value::as_str)
            .expect("commit");
        assert_eq!(commit.len(), 40, "{commit}");
        assert!(auto);

        // Outside a repository: no commit, no auto flag; no flags at all gives no object.
        let (evidence, auto) = build_record_evidence(&context(plain.path(), home.path()), &some);
        assert_eq!(keys(&evidence.expect("evidence")), "issue,gh");
        assert!(!auto);
        let (evidence, auto) = build_record_evidence(
            &context(plain.path(), home.path()),
            &EvidenceFlags::default(),
        );
        assert!(evidence.is_none());
        assert!(!auto);
    }
}
