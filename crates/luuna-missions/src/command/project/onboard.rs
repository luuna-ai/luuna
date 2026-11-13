//! Go's agent-instructions onboarding, including whole-file legacy marker updates.
use crate::{
    command::{options, output::Envelope, Context, Failure},
    config,
    error::Error,
    gopath,
};
use std::{
    fs, io,
    path::{Path, PathBuf},
};
const CLI_VERSION: &str = "0.6.0";
const START: &str = "<!-- missions:start -->";
const END: &str = "<!-- missions:end -->";
const SCHEMA: &str = "<!-- missions-onboard-schema:8 -->";
fn snippet() -> String {
    let version = CLI_VERSION;
    let schema = SCHEMA;
    format!(
        r###"## Issue Tracking (Missions)
<!-- missions-onboard:v{version} -->
{schema}

This project uses [Missions](https://github.com/luuna-ai/luuna-missions) v{version} (`lm`) for git-native issue tracking. Run `luuna-missions prime` at session start for the workflow and live state (in-progress and ready issues), or run `luuna-missions setup claude` once to do it automatically.

- `luuna-missions ready` — Find unblocked work
- `luuna-missions update <id> --claim` — Claim work (atomic)
- `luuna-missions create --title "..." [--from <id>]` — File work
- `luuna-missions close <id>` — Complete work
- `luuna-missions plan prompt <id>` — Decompose large or ambiguous work
- `luuna-missions sync` — Commit `.luuna/missions/` changes

Never hand-edit `.luuna/missions/*.jsonl`; `luuna-missions` locks and validates the store."###
    )
}
fn wrap() -> String {
    format!("{START}\n{}\n{END}", snippet())
}
fn markers(content: &str) -> String {
    content.replace("<!-- seeds", "<!-- missions")
}
fn status(content: &str) -> &'static str {
    let c = markers(content);
    if !c.contains(START) || !c.contains(END) {
        "missing"
    } else if c.contains("<!-- missions-onboard-v:") {
        "outdated"
    } else if c.contains(SCHEMA) {
        "current"
    } else {
        "outdated"
    }
}
fn target(root: &Path) -> Result<(PathBuf, bool), Failure> {
    for name in ["CLAUDE.md", ".claude/CLAUDE.md", "AGENTS.md"] {
        let p = root.join(name);
        match fs::metadata(&p) {
            Ok(_) => return Ok((p, true)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => (),
            Err(e) => return Err(Error::io("stat", p, e).into()),
        }
    }
    Ok((root.join("CLAUDE.md"), false))
}
fn write(path: &Path, bytes: &[u8]) -> Result<(), Failure> {
    fs::write(path, bytes).map_err(|e| Error::io("open", path, e).into())
}
pub(super) fn run(c: &mut Context<'_>, args: &[String]) -> Result<(), Failure> {
    let o = options::parse(args, &[], &["stdout", "check", "json"])?;
    if !o.positionals.is_empty() {
        return Err(Failure::message("too many arguments for 'onboard'"));
    }
    let store = c.store()?;
    let root = config::project_root(store.dir());
    let (path, exists) = target(&root)?;
    let file = gopath::text(&path);
    if o.has("check") {
        let state = if exists {
            status(&String::from_utf8_lossy(
                &fs::read(&path).map_err(|e| Error::io("open", &path, e))?,
            ))
        } else {
            "missing"
        };
        if o.has("json") {
            let out = Envelope::new(true, "onboard").string("status", state);
            c.json(if exists {
                out.string("file", &file)
            } else {
                out.raw("file", b"null")
            });
        } else {
            c.stdout.extend_from_slice(
                if exists {
                    format!("Status: {state} ({file})\n")
                } else {
                    "Status: missing (no CLAUDE.md found)\n".into()
                }
                .as_bytes(),
            );
        }
        return Ok(());
    }
    if o.has("stdout") {
        c.stdout
            .extend_from_slice(format!("{}\n", wrap()).as_bytes());
        return Ok(());
    }
    let action = if !exists {
        write(&path, format!("{}\n", wrap()).as_bytes())?;
        "created"
    } else {
        let raw = fs::read(&path).map_err(|e| Error::io("open", &path, e))?;
        // Go strings preserve invalid bytes outside the markers. Match ASCII markers on bytes.
        let content = String::from_utf8_lossy(&raw);
        match status(&content) {
            "current" => "unchanged",
            "outdated" => {
                let upgraded = replace_markers(&raw);
                let start = find(&upgraded, START.as_bytes());
                let end = find(&upgraded, END.as_bytes());
                if !matches!((start,end),(Some(a),Some(b)) if b>=a) {
                    let message=format!("Found missions markers in {file} but could not replace the section (markers may be malformed, out of order, or duplicated). Fix the file manually or remove the missions:start/missions:end block and re-run.");
                    if o.has("json") {
                        c.exit_code = 1;
                        c.json(
                            Envelope::new(false, "onboard")
                                .string("action", "failed")
                                .string("file", &file)
                                .string("error", &message),
                        );
                        return Ok(());
                    }
                    return Err(Failure::message(message));
                }
                let mut next = upgraded[..start.unwrap()].to_vec();
                next.extend_from_slice(wrap().as_bytes());
                next.extend_from_slice(&upgraded[end.unwrap() + END.len()..]);
                write(&path, &next)?;
                "updated"
            }
            _ => {
                let mut next = raw;
                next.extend_from_slice(if next.ends_with(b"\n") {
                    b"\n"
                } else {
                    b"\n\n"
                });
                next.extend_from_slice(format!("{}\n", wrap()).as_bytes());
                write(&path, &next)?;
                "appended"
            }
        }
    };
    if o.has("json") {
        c.json(
            Envelope::new(true, "onboard")
                .string("action", action)
                .string("file", &file),
        );
    } else {
        let message = match action {
            "created" => format!("Created {file} with missions section"),
            "updated" => format!("Updated missions section in {file}"),
            "appended" => format!("Added missions section to {file}"),
            _ => "Missions section is already up to date".into(),
        };
        c.success(message.as_bytes());
    }
    Ok(())
}
fn find(raw: &[u8], needle: &[u8]) -> Option<usize> {
    raw.windows(needle.len()).position(|w| w == needle)
}
fn replace_markers(raw: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut tail = raw;
    while let Some(i) = find(tail, b"<!-- seeds") {
        out.extend_from_slice(&tail[..i]);
        out.extend_from_slice(b"<!-- missions");
        tail = &tail[i + 10..];
    }
    out.extend_from_slice(tail);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config_tree::tree::test_support::{invoke, project, store};
    #[test]
    fn the_snippet_carries_version_0_6_0_and_schema_8() {
        let t = project("project: demo\nversion: \"1\"\n");
        let out = invoke(t.path(), &["onboard", "--stdout"]);
        let text = String::from_utf8(out.stdout).unwrap();
        assert!(text.contains("<!-- missions-onboard:v0.6.0 -->"));
        assert!(text.contains("<!-- missions-onboard-schema:8 -->"));
        assert!(text.contains("v0.6.0 (`lm`)"));
        assert!(!t.path().join("CLAUDE.md").exists());
    }
    #[test]
    fn check_reports_missing_outdated_current() {
        let t = project("project: demo\nversion: \"1\"\n");
        for(raw,state)in [(None,"missing"),(Some("<!-- seeds:start -->\nold\n<!-- seeds:end -->"),"outdated"),(Some("<!-- missions:start -->\n<!-- missions-onboard-schema:8 -->\n<!-- missions:end -->"),"current")]{if let Some(raw)=raw{fs::write(t.path().join("CLAUDE.md"),raw).unwrap();}let out=invoke(t.path(),&["onboard","--check","--json"]);let v:serde_json::Value=serde_json::from_slice(&out.stdout).unwrap();assert_eq!(v["status"],state);assert_eq!(out.exit_code,0);}
        fs::write(
            t.path().join("CLAUDE.md"),
            b"before\xff<!-- seeds:other -->\n<!-- seeds:start -->old<!-- seeds:end -->after\xff",
        )
        .unwrap();
        assert_eq!(invoke(t.path(), &["onboard"]).exit_code, 0);
        let raw = fs::read(t.path().join("CLAUDE.md")).unwrap();
        assert!(raw.starts_with(b"before\xff<!-- missions:other -->\n"));
        assert!(raw.ends_with(b"after\xff"));
    }
    #[test]
    fn onboard_leaves_the_parent_of_the_temporary_project_unchanged() {
        let p = tempfile::tempdir().unwrap();
        let mut before = vec![];
        for name in ["sentinel", "CLAUDE.md", "AGENTS.md"] {
            let path = p.path().join(name);
            fs::write(&path, name.as_bytes()).unwrap();
            before.push((
                path.clone(),
                fs::read(&path).unwrap(),
                fs::metadata(&path).unwrap().permissions(),
            ));
        }
        let root = p.path().join("project");
        store(&root, "project: demo\nversion: \"1\"\n");
        assert_eq!(invoke(&root, &["onboard"]).exit_code, 0);
        assert!(root.join("CLAUDE.md").is_file());
        for (path, bytes, mode) in before {
            assert_eq!(fs::read(&path).unwrap(), bytes);
            assert_eq!(fs::metadata(path).unwrap().permissions(), mode);
        }
        assert_eq!(fs::read_dir(p.path()).unwrap().count(), 4);
    }
}
