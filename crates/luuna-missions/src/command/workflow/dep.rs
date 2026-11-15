//! Go's dependency commands. Both mutation paths keep the A8 check under the lock.
use super::{lossy, render::string_array};
use crate::command::{
    options,
    output::Envelope,
    read::{closed_ids, one_line_ticket, println},
    shared::{ticket_index, unique_strings, without_string},
    Context, Failure,
};
use crate::{
    model,
    store::{DataFile, Store},
};
fn change(store: &Store, id: &str, blocker: &str, add: bool) -> Result<(), Failure> {
    if add && id == blocker {
        return Err(Failure::message(format!(
            "Cannot add a self-dependency: {id}"
        )));
    }
    let _lock = store.lock(DataFile::Issues)?;
    let before = store.read_tickets()?;
    let i = ticket_index(&before, id)
        .ok_or_else(|| Failure::message(format!("Issue not found: {id}")))?;
    let j = ticket_index(&before, blocker)
        .ok_or_else(|| Failure::message(format!("Issue not found: {blocker}")))?;
    let mut after = before.clone();
    let now = model::now_iso();
    if add {
        let mut list = after[i].blocked_by.take().unwrap_or_default();
        list.push(blocker.into());
        after[i].blocked_by = Some(unique_strings(list));
        let mut list = after[j].blocks.take().unwrap_or_default();
        list.push(id.into());
        after[j].blocks = Some(unique_strings(list));
    } else {
        after[i].blocked_by = without_string(after[i].blocked_by.take(), blocker);
        after[j].blocks = without_string(after[j].blocks.take(), id);
    }
    after[i].updated_at = now.clone();
    after[j].updated_at = now;
    lossy::refuse_lossy_ticket_changes(store, &before, &after)?;
    store.write_tickets(&after)?;
    Ok(())
}
pub(super) fn run(context: &mut Context<'_>, args: &[String]) -> Result<(), Failure> {
    let opts = options::parse(args, &[], &["json"])?;
    let p = &opts.positionals;
    if p.is_empty() {
        return Err(Failure::message(
            "Usage: luuna-missions dep <add|remove|list> <issue> [depends-on]",
        ));
    }
    let cmd = p[0].as_str();
    let store = context.store()?;
    if cmd == "list" {
        if p.len() < 2 {
            return Err(Failure::message("Usage: luuna-missions dep list <issue>"));
        }
        let tickets = store.read_tickets()?;
        let i = ticket_index(&tickets, &p[1])
            .ok_or_else(|| Failure::message(format!("Issue not found: {}", p[1])))?;
        let issue = &tickets[i];
        let by = issue.blocked_by.as_deref().unwrap_or_default();
        let blocks = issue.blocks.as_deref().unwrap_or_default();
        if opts.has("json") {
            context.json(
                Envelope::new(true, "dep list")
                    .string("issueId", &p[1])
                    .raw("blockedBy", &string_array(by))
                    .raw("blocks", &string_array(blocks)),
            );
            return Ok(());
        }
        println(context, &format!("{} dependencies:", p[1]));
        let closed = closed_ids(&tickets);
        for (name, ids) in [("Blocked by", by), ("Blocks", blocks)] {
            if ids.is_empty() {
                continue;
            }
            println(context, &format!("  {name}:"));
            for id in ids {
                let line = ticket_index(&tickets, id).map_or_else(
                    || format!("{id} (not found)"),
                    |at| one_line_ticket(&tickets[at], &closed),
                );
                println(context, &format!("    {line}"));
            }
        }
        if by.is_empty() && blocks.is_empty() {
            println(context, "  No dependencies.");
        }
        return Ok(());
    }
    if cmd != "add" && cmd != "remove" {
        return Err(Failure::message(format!(
            "Unknown dep subcommand: {cmd}. Use add, remove, or list."
        )));
    }
    if p.len() < 3 {
        return Err(Failure::message(format!(
            "Usage: luuna-missions dep {cmd} <issue> <depends-on>"
        )));
    }
    change(&store, &p[1], &p[2], cmd == "add")?;
    if opts.has("json") {
        context.json(
            Envelope::new(true, &format!("dep {cmd}"))
                .string("issueId", &p[1])
                .string("dependsOnId", &p[2]),
        );
    } else {
        println(
            context,
            &format!(
                "{} dependency: {} → {}",
                if cmd == "add" { "Added" } else { "Removed" },
                p[1],
                p[2]
            ),
        );
    }
    Ok(())
}

#[cfg(test)]
pub(super) mod test_support {
    use crate::command::{run, Invocation, Outcome};
    use std::{fs, path::Path};
    pub fn project(content: &[u8]) -> tempfile::TempDir {
        let temp = tempfile::tempdir().unwrap();
        let d = temp.path().join(".luuna/missions");
        fs::create_dir_all(&d).unwrap();
        fs::write(d.join("config.yaml"), "project: \"p\"\nversion: \"1\"\n").unwrap();
        fs::write(d.join("issues.jsonl"), content).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(d.join("issues.jsonl"), fs::Permissions::from_mode(0o640)).unwrap();
        }
        temp
    }
    pub fn record(id: &str, status: &str, extra: &[u8]) -> Vec<u8> {
        [format!(r#"{{"id":"{id}","title":"T {id}","status":"{status}","type":"task","priority":2,"createdAt":"c","updatedAt":"u""#).as_bytes(),extra,b"}\n"].concat()
    }
    pub fn base() -> Vec<u8> {
        [
            record("t", "open", br#","blockedBy":["b"],"labels":["a"]"#),
            record("b", "closed", br#","blocks":["t"]"#),
            record("o", "open", b""),
        ]
        .concat()
    }
    pub fn invoke(root: &Path, args: &[&str]) -> Outcome {
        run(&Invocation::new(root, args.iter().copied())).unwrap()
    }
    pub fn bytes(root: &Path) -> Vec<u8> {
        fs::read(root.join(".luuna/missions/issues.jsonl")).unwrap()
    }
    pub fn value(root: &Path, id: &str) -> serde_json::Value {
        let ticket = bytes(root)
            .split(|b| *b == b'\n')
            .filter_map(crate::Ticket::from_json)
            .find(|t| t.id == id)
            .unwrap();
        let mut value = serde_json::json!({"id":ticket.id,"updatedAt":ticket.updated_at,"closedAt":ticket.closed_at,"closeReason":ticket.close_reason});
        for (key, list) in [
            ("labels", ticket.labels),
            ("blockedBy", ticket.blocked_by),
            ("blocks", ticket.blocks),
        ] {
            if let Some(list) = list {
                value[key] = serde_json::json!(list);
            }
        }
        value
    }
    fn listing(root: &Path) -> Vec<String> {
        let mut names = fs::read_dir(root.join(".luuna/missions"))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        names.sort();
        names
    }
    fn mode(root: &Path) -> u32 {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::metadata(root.join(".luuna/missions/issues.jsonl"))
                .unwrap()
                .permissions()
                .mode()
        }
        #[cfg(not(unix))]
        {
            let _ = root;
            0
        }
    }
    pub fn unchanged_refusal(input: &[u8], args: &[&str], message: &str) {
        for json in [false, true] {
            let temp = project(input);
            let root = temp.path();
            let names = listing(root);
            let permissions = mode(root);
            let mut args = args.to_vec();
            if json {
                args.push("--json");
            }
            let out = invoke(root, &args);
            assert_eq!(out.exit_code, 1, "{args:?}");
            if json {
                assert!(out.stderr.is_empty());
                let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
                assert_eq!(
                    v,
                    serde_json::json!({"success":false,"command":args[0],"error":message})
                );
            } else {
                assert!(out.stdout.is_empty());
                assert_eq!(out.stderr, format!("Error: {message}\n").as_bytes());
            }
            assert_eq!(bytes(root), input);
            assert_eq!(mode(root), permissions);
            assert_eq!(listing(root), names);
        }
    }
    pub fn file_refusals(args: &[&str]) {
        let cases:[(&[u8],&str);4]=[
            (b"not json\n","Refusing to rewrite issues.jsonl: line 1 is not a ticket record the Go tool can read, and rewriting the file would drop it. Nothing was written."),
            (b"{\"id\":\"x\"}\n{\"id\":\"x\"}\n","Refusing to rewrite issues.jsonl: lines 1 and 2 both hold the ticket \"x\", and rewriting the file would keep only the last one. Nothing was written."),
            (b"{}\n{}\n","Refusing to rewrite issues.jsonl: lines 1 and 2 both hold a record without an id, and rewriting the file would keep only the last one. Nothing was written."),
            (b"{\"id\":\"x\",\"id\":\"y\"}\n","Refusing to rewrite issues.jsonl: the record on line 1 repeats the key \"id\", and rewriting the file would keep only its last value. Nothing was written.")];
        for (bad, text) in cases {
            let input = [bad, &base()].concat();
            unchanged_refusal(&input, args, text);
            let clean = project(&base());
            assert_eq!(invoke(clean.path(), args).exit_code, 0);
        }
    }
    fn cell(args: &[&str], field: &str, on_blocker: bool, value: &[u8]) -> Vec<u8> {
        let mut t = br#","blockedBy":["b"],"labels":["a"]"#.to_vec();
        let mut b = br#","blocks":["t"]"#.to_vec();
        let extra = [b",\"", field.as_bytes(), b"\":", value].concat();
        if on_blocker {
            b = extra;
        } else if field == "labels" {
            t = [br#","blockedBy":["b"]"#.as_slice(), &extra].concat();
        } else {
            t = [br#","labels":["a"]"#.as_slice(), &extra].concat();
        }
        [
            record("t", "open", &t),
            record(
                "b",
                if args.contains(&"--all") {
                    "closed"
                } else {
                    "open"
                },
                &b,
            ),
            record("o", "open", b""),
        ]
        .concat()
    }
    pub fn field_refusals(args: &[&str], field: &str, on_blocker: bool, allow_not_list: bool) {
        let related = if field == "labels" {
            "a"
        } else if args.get(1) == Some(&"add") {
            "c"
        } else if on_blocker {
            "t"
        } else {
            "b"
        };
        let mut cases: Vec<(Vec<u8>, &str)> = vec![
            (
                format!("[\"{related}\",5]").into_bytes(),
                "holds an element of",
            ),
            (
                format!("[\"{related}\",null]").into_bytes(),
                "holds an element of",
            ),
            (
                [format!("[\"{related}\",\"").as_bytes(), b"\xff\"]"].concat(),
                "invalid",
            ),
            (format!(r#"["{related}","\ud800"]"#).into_bytes(), "invalid"),
        ];
        if allow_not_list {
            cases.push((format!("\"{related}\"").into_bytes(), "notlist"));
        }
        if field == "labels" && args.get(1) == Some(&"add") {
            cases.push((b"[\"\xff\",\"\xfe\"]".to_vec(), "invalid"));
        }
        for (value, kind) in cases {
            let input = cell(args, field, on_blocker, &value);
            let line = if on_blocker { 2 } else { 1 };
            let id = if on_blocker { "b" } else { "t" };
            let why=match kind{"notlist"=>format!("holds a \"{field}\" value that is not a list of strings, and changing the field would replace that value"),"invalid"=>format!("holds an element of \"{field}\" with invalid UTF-8 or a lone surrogate escape, and changing the field would write it as U+FFFD"),_=>format!("holds an element of \"{field}\" that is not a string, and changing the field would write it as an empty string")};
            unchanged_refusal(&input,args,&format!("Refusing to rewrite issues.jsonl: line {line} (ticket \"{id}\") {why}. Nothing was written."));
            let clean = project(&cell(
                args,
                field,
                on_blocker,
                format!("[\"{related}\"]").as_bytes(),
            ));
            assert_eq!(invoke(clean.path(), args).exit_code, 0);
        }
    }
    pub fn control(args: &[&str], field: &str, on_blocker: bool, raw: &[u8], exit: i32) {
        let input = cell(args, field, on_blocker, raw);
        let temp = project(&input);
        let out = invoke(temp.path(), args);
        assert_eq!(out.exit_code, exit);
        assert!(!String::from_utf8_lossy(&out.stderr).contains("Refusing"));
        let needle = [b"\"", field.as_bytes(), b"\":", raw].concat();
        assert!(bytes(temp.path())
            .windows(needle.len())
            .any(|s| s == needle));
        if exit == 0 {
            let id = if args[0] == "close" { "b" } else { "t" };
            assert_ne!(value(temp.path(), id)["updatedAt"], "u");
        } else {
            assert_eq!(out.stderr, b"Error: t is not blocked by b\n");
            assert_eq!(bytes(temp.path()), input);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::*;
    #[test]
    fn add_and_remove_update_both_tickets() {
        let t = project(&base());
        let root = t.path();
        assert_eq!(
            invoke(root, &["dep", "add", "o", "t"]).stdout,
            "Added dependency: o → t\n".as_bytes()
        );
        assert_eq!(value(root, "o")["blockedBy"], serde_json::json!(["t"]));
        assert_eq!(value(root, "t")["blocks"], serde_json::json!(["o"]));
        assert_eq!(value(root, "o")["updatedAt"], value(root, "t")["updatedAt"]);
        assert_eq!(invoke(root, &["dep", "remove", "o", "t"]).exit_code, 0);
        assert!(value(root, "o").get("blockedBy").is_none());
    }
    #[test]
    fn list_prints_go_s_text() {
        let t = project(&base());
        assert_eq!(
            invoke(t.path(), &["dep", "list", "o"]).stdout,
            b"o dependencies:\n  No dependencies.\n"
        );
        assert_eq!(
            invoke(t.path(), &["dep", "list", "t"]).stdout,
            "t dependencies:\n  Blocked by:\n    x b · T b   [Medium · task]\n".as_bytes()
        );
    }
    #[test]
    fn errors_follow_go_s_order() {
        let empty = tempfile::tempdir().unwrap();
        assert_eq!(
            invoke(empty.path(), &["dep", "nope"]).stderr,
            b"Error: Not in a missions project. Run `luuna-missions init` first.\n"
        );
        let t = project(&base());
        assert_eq!(
            invoke(t.path(), &["dep", "nope"]).stderr,
            b"Error: Unknown dep subcommand: nope. Use add, remove, or list.\n"
        );
        assert_eq!(
            invoke(t.path(), &["dep", "add", "t", "t"]).stderr,
            b"Error: Cannot add a self-dependency: t\n"
        );
    }
    #[test]
    fn add_refuses_a_lossy_issues_file_unchanged() {
        file_refusals(&["dep", "add", "t", "b"]);
    }
    #[test]
    fn remove_refuses_a_lossy_issues_file_unchanged() {
        file_refusals(&["dep", "remove", "t", "b"]);
    }
    #[test]
    fn add_refuses_a_list_go_cannot_rewrite_unchanged() {
        for (field, blocker) in [("blockedBy", false), ("blocks", true)] {
            field_refusals(&["dep", "add", "t", "b"], field, blocker, true);
        }
    }
    #[test]
    fn remove_refuses_a_list_go_cannot_rewrite_unchanged() {
        for (field, blocker) in [("blockedBy", false), ("blocks", true)] {
            field_refusals(&["dep", "remove", "t", "b"], field, blocker, false);
        }
    }
    #[test]
    fn lists_dep_leaves_unchanged_are_kept_byte_for_byte() {
        control(
            &["dep", "remove", "t", "b"],
            "blockedBy",
            false,
            br#""b""#,
            0,
        );
        control(&["dep", "remove", "t", "b"], "blocks", true, br#""t""#, 0);
        control(
            &["dep", "add", "t", "b"],
            "blockedBy",
            false,
            br#"["b",5]"#,
            0,
        );
    }
}
