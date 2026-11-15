//! Go's read-only Beads input mapping and guarded issues-store rewrite.
use super::lossy;
use crate::{
    command::{options, output::Envelope, read::println, Context, Failure},
    config,
    error::Error,
    json::Value as V,
    json_error,
    model::{self, Ticket},
    store::{self, DataFile},
};
use std::{
    collections::{BTreeMap, HashSet},
    fs,
};
fn string(record: &BTreeMap<String, V>, names: &[&str]) -> String {
    for name in names {
        if let Some(V::String(s)) = record.get(*name) {
            return s.clone();
        }
    }
    String::new()
}
fn strings(record: &BTreeMap<String, V>, names: &[&str]) -> Option<Vec<String>> {
    for name in names {
        if let Some(V::Array(items)) = record.get(*name) {
            return Some(
                items
                    .iter()
                    .filter_map(|v| {
                        if let V::String(s) = v {
                            Some(s.clone())
                        } else {
                            None
                        }
                    })
                    .collect(),
            );
        }
    }
    None
}
fn map(record: &BTreeMap<String, V>) -> Option<Ticket> {
    let id = string(record, &["id"]);
    let title = string(record, &["title"]);
    if id.is_empty() || title.is_empty() {
        return None;
    }
    let now = model::now_iso();
    let status = match string(record, &["status"]).as_str() {
        "in_progress" | "in-progress" => "in_progress",
        "closed" | "done" | "complete" => "closed",
        _ => "open",
    }
    .into();
    let kind = string(record, &["issue_type", "type"]);
    let kind = if ["bug", "feature", "epic"].contains(&kind.as_str()) {
        kind
    } else {
        "task".into()
    };
    let priority = if let Some(V::Number(n)) = record.get("priority") {
        if *n >= i64::MIN as f64 && *n < 9_223_372_036_854_775_808.0 {
            *n as i64
        } else {
            i64::MIN
        }
    } else {
        2
    };
    let created = string(record, &["created_at", "createdAt"]);
    let updated = string(record, &["updated_at", "updatedAt"]);
    let mut ticket = Ticket::default();
    ticket.id = id;
    ticket.title = title;
    ticket.status = status;
    ticket.ticket_type = kind;
    ticket.priority = priority;
    ticket.created_at = if created.is_empty() {
        now.clone()
    } else {
        created
    };
    ticket.updated_at = if updated.is_empty() { now } else { updated };
    ticket.assignee = string(record, &["owner", "assignee"]);
    ticket.description = string(record, &["description"]);
    ticket.close_reason = string(record, &["close_reason", "closeReason"]);
    ticket.blocked_by = strings(record, &["blocked_by", "blockedBy"]);
    ticket.blocks = strings(record, &["blocks"]);
    ticket.closed_at = string(record, &["closed_at", "closedAt"]);
    Some(ticket)
}
pub(super) fn run(context: &mut Context<'_>, args: &[String]) -> Result<(), Failure> {
    let opts = options::parse(args, &[], &["json"])?;
    if !opts.positionals.is_empty() {
        return Err(Failure::message(
            "too many arguments for 'migrate-from-beads'",
        ));
    }
    let store = context.store()?;
    let path = config::project_root(store.dir()).join(".beads/issues.jsonl");
    let bytes = fs::read(&path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            Failure::message(format!("Beads issues not found at: {}", path.display()))
        } else {
            Error::io("read", &path, e).into()
        }
    })?;
    let mut mapped = Vec::new();
    let mut skipped = 0;
    for raw in bytes.split(|b| *b == b'\n') {
        if raw.len() >= 10 * 1024 * 1024 {
            return Err(Failure::message("bufio.Scanner: token too long"));
        }
        let raw = store::trim_space(raw);
        if raw.is_empty() {
            continue;
        }
        if json_error::unmarshal_error(&String::from_utf8_lossy(raw)).is_some() {
            continue;
        }
        match V::decode(raw) {
            Ok(V::Null) => skipped += 1,
            Ok(value) => {
                if let Some(object) = value.into_object() {
                    if let Some(t) = map(&object) {
                        mapped.push(t);
                    } else {
                        skipped += 1;
                    }
                }
            }
            Err(_) => {}
        }
    }
    let mut written = 0;
    {
        let _lock = store.lock(DataFile::Issues)?;
        let mut issues = store.read_tickets()?;
        let ids = issues.iter().map(|t| t.id.clone()).collect::<HashSet<_>>();
        for issue in mapped {
            if !ids.contains(&issue.id) {
                issues.push(issue);
                written += 1;
            }
        }
        lossy::refuse_lossy_rewrite::<Ticket>(&store.path(DataFile::Issues))?;
        store.write_tickets(&issues)?;
    }
    if opts.has("json") {
        context.json(
            Envelope::new(true, "migrate-from-beads")
                .integer("written", written)
                .integer("skipped", skipped),
        );
    } else {
        println(context, &format!("Migrated {written} issues from beads."));
        if skipped > 0 {
            println(context, &format!("Skipped {skipped} malformed issues."));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config_tree::tree::test_support::{invoke, project, store as make_store};
    use std::path::Path;
    fn beads(root: &Path, bytes: &[u8]) {
        fs::create_dir_all(root.join(".beads")).unwrap();
        fs::write(root.join(".beads/issues.jsonl"), bytes).unwrap();
    }
    #[test]
    fn maps_fields_as_go_does() {
        let t = project("project: demo\nversion: \"1\"\n");
        let input=b"{\"id\":\"a\",\"title\":\"Title\",\"status\":\"in-progress\",\"issue_type\":\"bug\",\"priority\":1.9,\"created_at\":\"c\",\"updated_at\":\"u\",\"owner\":\"agent\",\"description\":\"Body\",\"close_reason\":\"why\",\"blocked_by\":[\"b\",5],\"blocks\":[],\"closed_at\":\"old\"}\nNOT JSON\n{}\nnull\n[1]\n";
        beads(t.path(), input);
        let out = invoke(t.path(), &["migrate-from-beads", "--json"]);
        assert_eq!(out.stdout,b"{\n  \"success\": true,\n  \"command\": \"migrate-from-beads\",\n  \"written\": 1,\n  \"skipped\": 2\n}\n");
        let rows =
            store::read_records::<Ticket>(&t.path().join(".luuna/missions/issues.jsonl")).unwrap();
        assert_eq!(rows.len(), 1);
        let a = &rows[0];
        assert_eq!(
            (&a.id, &a.title, &a.status, &a.ticket_type),
            (
                &"a".into(),
                &"Title".into(),
                &"in_progress".into(),
                &"bug".into()
            )
        );
        assert_eq!(a.priority, 1);
        assert_eq!(a.created_at, "c");
        assert_eq!(a.updated_at, "u");
        assert_eq!(a.assignee, "agent");
        assert_eq!(a.blocked_by, Some(vec!["b".into()]));
        assert_eq!(a.blocks, Some(Vec::new()));
        assert_eq!(a.close_reason, "why");
        assert_eq!(a.closed_at, "old");
        assert_eq!(
            fs::read(t.path().join(".beads/issues.jsonl")).unwrap(),
            input
        );
    }
    #[test]
    fn duplicate_beads_ids_are_both_appended() {
        let t = project("project: demo\nversion: \"1\"\n");
        beads(
            t.path(),
            b"{\"id\":\"a\",\"title\":\"one\"}\n{\"id\":\"a\",\"title\":\"two\"}\n",
        );
        let out = invoke(t.path(), &["migrate-from-beads", "--json"]);
        let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(v["written"], 2);
        let data = fs::read(t.path().join(".luuna/missions/issues.jsonl")).unwrap();
        assert_eq!(
            data.split(|b| *b == b'\n')
                .filter(|line| !line.is_empty())
                .count(),
            2
        );
    }
    /// Streams exactly10 MiB+1 bytes to one disposable file, with8 KiB memory.
    #[test]
    fn an_over_long_line_fails_with_go_s_text() {
        use std::io::Write;
        let temp = if let Some(dir) = std::env::var_os("LUUNA_MISSIONS_TEST_DISK_DIR") {
            fs::create_dir_all(&dir).unwrap();
            tempfile::tempdir_in(dir).unwrap()
        } else {
            tempfile::tempdir().unwrap()
        };
        make_store(temp.path(), "project: demo\nversion: \"1\"\n");
        fs::create_dir(temp.path().join(".beads")).unwrap();
        let mut file = fs::File::create(temp.path().join(".beads/issues.jsonl")).unwrap();
        let block = [b'x'; 8192];
        for _ in 0..1280 {
            file.write_all(&block).unwrap();
        }
        file.write_all(b"x").unwrap();
        drop(file);
        let out = invoke(temp.path(), &["migrate-from-beads"]);
        assert_eq!(out.exit_code, 1);
        assert_eq!(out.stderr, b"Error: bufio.Scanner: token too long\n");
        assert!(!temp.path().join(".luuna/missions/issues.jsonl").exists());
    }
    #[test]
    fn migrate_refuses_a_lossy_rewrite_and_the_file_is_unchanged() {
        let args = &["migrate-from-beads"];
        for mode in [0o644, 0o600] {
            for (bad, why) in [
                (
                    b"bad\n".as_slice(),
                    "line 1 is not a record the tool can read",
                ),
                (
                    b"{\"id\":\"a\"}\n{\"id\":\"a\"}\n",
                    "line 2 repeats the id \"a\" of line 1",
                ),
                (
                    b"{\"id\":\"a\",\"id\":\"b\"}\n",
                    "line 1 repeats the key \"id\"",
                ),
            ] {
                let t = project("project: demo\nversion: \"1\"\n");
                beads(t.path(), b"");
                let path = t.path().join(".luuna/missions/issues.jsonl");
                fs::write(&path, bad).unwrap();
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();
                }
                #[cfg(not(unix))]
                let _ = mode;
                let permissions = fs::metadata(&path).unwrap().permissions();
                let out = invoke(t.path(), args);
                assert_eq!(out.exit_code, 1);
                assert_eq!(out.stderr,format!("Error: Refusing to rewrite {}: {why}, and the Go tool would drop it without a message. Nothing was written.\n",path.display()).as_bytes());
                assert_eq!(fs::read(&path).unwrap(), bad);
                assert_eq!(fs::metadata(&path).unwrap().permissions(), permissions);
                assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 2);
            }
        }
    }
    #[test]
    fn migrate_leaves_the_parent_of_the_temporary_project_unchanged() {
        let parent = tempfile::tempdir().unwrap();
        fs::write(parent.path().join("sentinel"), b"parent").unwrap();
        beads(parent.path(), b"{\"id\":\"parent\",\"title\":\"Parent\"}\n");
        let input = fs::read(parent.path().join(".beads/issues.jsonl")).unwrap();
        let mode = fs::metadata(parent.path().join(".beads/issues.jsonl"))
            .unwrap()
            .permissions();
        let root = parent.path().join("project");
        make_store(&root, "project: demo\nversion: \"1\"\n");
        beads(&root, b"{\"id\":\"child\",\"title\":\"Child\"}\n");
        assert_eq!(invoke(&root, &["migrate-from-beads"]).exit_code, 0);
        assert_eq!(
            fs::read(parent.path().join(".beads/issues.jsonl")).unwrap(),
            input
        );
        assert_eq!(
            fs::metadata(parent.path().join(".beads/issues.jsonl"))
                .unwrap()
                .permissions(),
            mode
        );
        assert_eq!(fs::read(parent.path().join("sentinel")).unwrap(), b"parent");
        assert_eq!(fs::read_dir(parent.path()).unwrap().count(), 3);
        assert!(!parent.path().join(".luuna").exists());
    }
}
