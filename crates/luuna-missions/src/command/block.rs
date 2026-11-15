//! `block <id> --by <blocker>` (`internal/cli/issue_links.go:13-35, 161-190`).

use super::output::Envelope;
use super::shared::{ticket_index, unique_strings};
use super::{options, Context, Failure};
use crate::model;
use crate::store::DataFile;

/// Go's `runBlock`.
pub(super) fn run(context: &mut Context<'_>, args: &[String]) -> Result<(), Failure> {
    let options = options::parse(args, &["by"], &["json"])?;
    if options.positionals.is_empty() || !options.has("by") || options.value("by").is_empty() {
        return Err(Failure::message(
            "Usage: luuna-missions block <id> --by <blocker-id>",
        ));
    }
    let id = options.positionals[0].as_str();
    let blocker = options.value("by");
    if id == blocker {
        return Err(Failure::message(format!(
            "Cannot block an issue by itself: {id}"
        )));
    }
    let store = context.store()?;
    add_dependency(&store, id, blocker)?;
    if options.has("json") {
        context.json(
            Envelope::new(true, "block")
                .string("issueId", id)
                .string("blockerId", blocker),
        );
    } else {
        // Written directly, so `--quiet` does not hide it.
        context
            .stdout
            .extend_from_slice(format!("{id} is now blocked by {blocker}\n").as_bytes());
    }
    Ok(())
}

/// Go's `addDependency`.
pub(super) fn add_dependency(
    store: &crate::store::Store,
    issue_id: &str,
    blocker_id: &str,
) -> Result<(), Failure> {
    if issue_id == blocker_id {
        return Err(Failure::message(format!(
            "Cannot add a self-dependency: {issue_id}"
        )));
    }
    let _lock = store.lock(DataFile::Issues)?;
    let mut tickets = store.read_tickets()?;
    let issue = ticket_index(&tickets, issue_id);
    let blocker = ticket_index(&tickets, blocker_id);
    let Some(issue) = issue else {
        return Err(Failure::message(format!("Issue not found: {issue_id}")));
    };
    let Some(blocker) = blocker else {
        return Err(Failure::message(format!("Issue not found: {blocker_id}")));
    };
    let original_blocked_by = tickets[issue].blocked_by.clone();
    let original_blocks = tickets[blocker].blocks.clone();
    let now = model::now_iso();
    let mut blocked_by = tickets[issue].blocked_by.take().unwrap_or_default();
    blocked_by.push(blocker_id.to_owned());
    tickets[issue].blocked_by = Some(unique_strings(blocked_by));
    let mut blocks = tickets[blocker].blocks.take().unwrap_or_default();
    blocks.push(issue_id.to_owned());
    tickets[blocker].blocks = Some(unique_strings(blocks));
    tickets[issue].updated_at.clone_from(&now);
    tickets[blocker].updated_at = now;
    let mut changed = Vec::new();
    if original_blocked_by != tickets[issue].blocked_by {
        changed.push((issue_id, super::update::lossy::Field::BlockedBy));
    }
    if original_blocks != tickets[blocker].blocks {
        changed.push((blocker_id, super::update::lossy::Field::Blocks));
    }
    super::update::lossy::refuse_lossy_issues_rewrite(store, &changed)?;
    store.write_tickets(&tickets)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use super::super::{run, Invocation, Outcome};

    fn record(id: &str) -> String {
        format!(
            r#"{{"id":"{id}","title":"T","status":"open","type":"task","priority":2,"createdAt":"c","updatedAt":"u"}}"#
        )
    }

    fn project() -> tempfile::TempDir {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join(".luuna/missions");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("config.yaml"), "project: \"p\"\nversion: \"1\"\n").unwrap();
        fs::write(
            dir.join("issues.jsonl"),
            format!("{}\n{}\n", record("a"), record("b")),
        )
        .unwrap();
        temp
    }

    fn block(root: &Path, args: &[&str]) -> Outcome {
        let mut all = vec!["block"];
        all.extend_from_slice(args);
        run(&Invocation::new(root, all)).unwrap()
    }

    fn issues(root: &Path) -> String {
        fs::read_to_string(root.join(".luuna/missions/issues.jsonl")).unwrap()
    }

    #[test]
    fn blocking_links_both_tickets_once() {
        let temp = project();
        for _ in 0..2 {
            let outcome = block(temp.path(), &["a", "--by", "b"]);
            assert_eq!(outcome.exit_code, 0);
            assert_eq!(
                String::from_utf8(outcome.stdout).unwrap(),
                "a is now blocked by b\n"
            );
        }
        let text = issues(temp.path());
        assert_eq!(text.matches("\"blockedBy\":[\"b\"]").count(), 1, "{text}");
        assert_eq!(text.matches("\"blocks\":[\"a\"]").count(), 1, "{text}");
        // The message is not a success line, so --quiet keeps it.
        let outcome = block(temp.path(), &["a", "--by", "b", "-q"]);
        assert_eq!(outcome.stdout, b"a is now blocked by b\n");
    }

    #[test]
    fn invalid_requests_change_nothing() {
        let temp = project();
        let before = issues(temp.path());
        for (args, message) in [
            (
                &["a"][..],
                "Usage: luuna-missions block <id> --by <blocker-id>",
            ),
            (
                &["a", "--by", ""],
                "Usage: luuna-missions block <id> --by <blocker-id>",
            ),
            (&["a", "--by", "a"], "Cannot block an issue by itself: a"),
            (&["a", "--by", "zz"], "Issue not found: zz"),
            (&["zz", "--by", "a"], "Issue not found: zz"),
        ] {
            let outcome = block(temp.path(), args);
            assert_eq!(outcome.exit_code, 1, "{args:?}");
            assert_eq!(
                String::from_utf8(outcome.stderr).unwrap(),
                format!("Error: {message}\n"),
                "{args:?}"
            );
            assert_eq!(issues(temp.path()), before, "{args:?}");
        }
    }
    fn snapshot(root: &Path) -> (Vec<u8>, fs::Permissions) {
        let p = root.join(".luuna/missions/issues.jsonl");
        (
            fs::read(&p).unwrap(),
            fs::metadata(p).unwrap().permissions(),
        )
    }
    fn assert_refused(root: &Path, text: &str, json: bool) {
        let before = snapshot(root);
        let out = block(
            root,
            if json {
                &["a", "--by", "b", "--json"]
            } else {
                &["a", "--by", "b"]
            },
        );
        assert_eq!(out.exit_code, 1);
        if json {
            let expected = format!(
                "{{\n  \"success\": false,\n  \"command\": \"block\",\n  \"error\": {}\n}}\n",
                serde_json::to_string(text).unwrap()
            );
            assert_eq!(out.stdout, expected.as_bytes());
            assert!(out.stderr.is_empty());
        } else {
            assert!(out.stdout.is_empty());
            assert_eq!(out.stderr, format!("Error: {text}\n").as_bytes());
        }
        assert_eq!(snapshot(root), before);
        assert!(fs::read_dir(root.join(".luuna/missions"))
            .unwrap()
            .all(|entry| {
                let n = entry.unwrap().file_name().to_string_lossy().into_owned();
                !n.contains(".lock") && !n.contains(".tmp")
            }));
    }
    #[test]
    fn a_lossy_issues_file_is_refused_unchanged() {
        for tail in [
            b"{not json\n".as_slice(),
            b"[1]\n",
            b"null\n",
            b"{\"id\":\"other\",\"extensions\":{\"n\":1e999}}\n",
        ] {
            let t = project();
            let p = t.path().join(".luuna/missions/issues.jsonl");
            let mut raw = fs::read(&p).unwrap();
            raw.extend_from_slice(tail);
            fs::write(p, raw).unwrap();
            assert_refused(t.path(),"Refusing to rewrite issues.jsonl: line 3 is not a ticket record the Go tool can read, and rewriting the file would drop it. Nothing was written.",true);
        }
        for (tail,text) in [(b"{\"id\":\"a\"}\n".as_slice(),"Refusing to rewrite issues.jsonl: lines 1 and 3 both hold the ticket \"a\", and rewriting the file would keep only the last one. Nothing was written."),(b"{}\n{}\n","Refusing to rewrite issues.jsonl: lines 3 and 4 both hold a record without an id, and rewriting the file would keep only the last one. Nothing was written."),(b"{\"id\":\"c\",\"x\":1,\"x\":2}\n","Refusing to rewrite issues.jsonl: the record on line 3 repeats the key \"x\", and rewriting the file would keep only its last value. Nothing was written.")]{let t=project();let p=t.path().join(".luuna/missions/issues.jsonl");let mut raw=fs::read(&p).unwrap();raw.extend_from_slice(tail);fs::write(p,raw).unwrap();assert_refused(t.path(),text,false);}
        let t = project();
        let p = t.path().join(".luuna/missions/issues.jsonl");
        let raw =
            issues(t.path()).replacen("\"title\":\"T\"", "\"title\":\"first\",\"title\":\"T\"", 1);
        fs::write(p, raw).unwrap();
        assert_refused(t.path(),"Refusing to rewrite issues.jsonl: the record on line 1 repeats the key \"title\", and rewriting the file would keep only its last value. Nothing was written.",false);
    }
    #[test]
    fn lists_go_cannot_rewrite_are_refused_unchanged() {
        for (key, id, line) in [("blockedBy", "a", 1), ("blocks", "b", 2)] {
            for (raw, kind) in [
                (b"5".as_slice(), 'd'),
                (br#"{"a":1}"#, 'd'),
                (br#""x""#, 'd'),
                (br#"["c",5]"#, 'e'),
                (br#"["c",null]"#, 'e'),
                (b"[\"c\",\"\xff\"]", 'f'),
                (br#"["c","\ud800"]"#, 'f'),
            ] {
                let t = project();
                let record = |name: &str| {
                    let line = record(name);
                    if name == id {
                        [
                            &line.as_bytes()[..line.len() - 1],
                            format!(",\"{key}\":").as_bytes(),
                            raw,
                            b"}\n",
                        ]
                        .concat()
                    } else {
                        (line + "\n").into_bytes()
                    }
                };
                fs::write(
                    t.path().join(".luuna/missions/issues.jsonl"),
                    [record("a"), record("b")].concat(),
                )
                .unwrap();
                let why=match kind{'d'=>format!("a \"{key}\" value that is not a list of strings, and changing the field would replace that value"),'e'=>format!("an element of \"{key}\" that is not a string, and changing the field would write it as an empty string"),_=>format!("an element of \"{key}\" with invalid UTF-8 or a lone surrogate escape, and changing the field would write it as U+FFFD")};
                let text=format!("Refusing to rewrite issues.jsonl: line {line} (ticket \"{id}\") holds {why}. Nothing was written.");
                assert_refused(t.path(), &text, kind == 'd');
            }
        }
    }
    #[test]
    fn lists_go_rewrites_exactly_are_written_as_go_writes_them() {
        for (raw, expected) in [
            (b"null".as_slice(), serde_json::json!(["b"])),
            (br#"["b"]"#, serde_json::json!(["b"])),
            (br#"["c","c"]"#, serde_json::json!(["c", "b"])),
        ] {
            let t = project();
            let a = record("a");
            let data = [
                &a.as_bytes()[..a.len() - 1],
                b",\"blockedBy\":",
                raw,
                b"}\n",
                record("b").as_bytes(),
                b"\n",
            ]
            .concat();
            fs::write(t.path().join(".luuna/missions/issues.jsonl"), data).unwrap();
            assert_eq!(block(t.path(), &["a", "--by", "b"]).exit_code, 0);
            let values = issues(t.path())
                .lines()
                .map(|s| serde_json::from_str::<serde_json::Value>(s).unwrap())
                .collect::<Vec<_>>();
            assert_eq!(values[0]["blockedBy"], expected);
            assert_eq!(values[1]["blocks"], serde_json::json!(["a"]));
        }
    }
    #[test]
    fn block_leaves_the_parent_of_the_temporary_project_unchanged() {
        let parent = tempfile::tempdir().unwrap();
        let root = parent.path().join("project");
        let dir = root.join(".luuna/missions");
        fs::create_dir_all(&dir).unwrap();
        fs::create_dir(parent.path().join(".luuna")).unwrap();
        fs::write(parent.path().join("sentinel"), b"parent data").unwrap();
        fs::write(parent.path().join(".gitattributes"), b"parent attrs").unwrap();
        let snapshot_parent = || {
            let mut entries = fs::read_dir(parent.path())
                .unwrap()
                .filter_map(|e| {
                    let e = e.unwrap();
                    let n = e.file_name().to_string_lossy().into_owned();
                    if n == "project" {
                        return None;
                    }
                    let m = e.metadata().unwrap();
                    let bytes = if m.is_dir() {
                        assert_eq!(fs::read_dir(e.path()).unwrap().count(), 0);
                        vec![]
                    } else {
                        fs::read(e.path()).unwrap()
                    };
                    Some((n, bytes, m.permissions()))
                })
                .collect::<Vec<_>>();
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            entries
        };
        let before = snapshot_parent();
        fs::write(dir.join("config.yaml"), b"project: p\nversion: \"1\"\n").unwrap();
        let data = format!("{}\n{}\n", record("a"), record("b"));
        fs::write(dir.join("issues.jsonl"), format!("{data}unreadable\n")).unwrap();
        assert_eq!(block(&root, &["a", "--by", "b"]).exit_code, 1);
        fs::write(dir.join("issues.jsonl"), data).unwrap();
        assert_eq!(block(&root, &["a", "--by", "b"]).exit_code, 0);
        assert_eq!(snapshot_parent(), before);
    }
}
