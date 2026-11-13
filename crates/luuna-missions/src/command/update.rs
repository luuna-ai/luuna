//! `update` (`internal/cli/issue_mutate.go:135-312`).
//!
//! Status changes hold plans then issues locks and preflight the prospective
//! plan transition before writing either file. Other edits retain Go behavior.

use std::collections::BTreeMap;

use super::output::Envelope;
use super::shared::{
    parse_intent, parse_priority, split_labels, ticket_index, unique_strings, validate_status,
    validate_type, without_string,
};
use super::{options, Context, Failure};
use crate::error::Result;
use crate::json_error;
use crate::model::{self, Value};
use crate::store::DataFile;

/// Go's `runUpdate`.
pub(super) fn run(context: &mut Context<'_>, args: &[String]) -> Result<(), Failure> {
    let options = options::parse(
        args,
        &[
            "status",
            "title",
            "assignee",
            "description",
            "desc",
            "body",
            "type",
            "priority",
            "add-label",
            "remove-label",
            "set-labels",
            "extensions",
            "as",
            "intent",
        ],
        &["clear-extensions", "claim", "json"],
    )?;
    if options.positionals.is_empty() {
        return Err(Failure::message(
            "Usage: luuna-missions update <id> [flags]",
        ));
    }
    if options.positionals.len() > 1 {
        return Err(Failure::message("too many arguments for 'update'"));
    }
    let id = options.positionals[0].as_str();
    let claim = options.has("claim");
    let mut claimant = String::new();
    if claim {
        if options.has("status") || options.has("assignee") {
            return Err(Failure::message(
                "--claim cannot be combined with --status or --assignee",
            ));
        }
        claimant = options.value("as").trim().to_owned();
        if claimant.is_empty() {
            claimant = context.env("USER").unwrap_or_default().trim().to_owned();
        }
        if claimant.is_empty() {
            return Err(Failure::message(
                "--claim needs an identity: pass --as <agent>",
            ));
        }
    }
    if options.has("extensions") && options.has("clear-extensions") {
        return Err(Failure::message(
            "--extensions and --clear-extensions are mutually exclusive",
        ));
    }
    if options.has("status") {
        validate_status(options.value("status")).map_err(Failure::message)?;
    }
    if options.has("type") {
        validate_type(options.value("type")).map_err(Failure::message)?;
    }
    let mut priority = 0;
    if options.has("priority") {
        priority = parse_priority(options.value("priority")).map_err(Failure::message)?;
    }
    let mut intent = None;
    if options.has("intent") {
        intent = parse_intent(options.repeated("intent")).map_err(Failure::message)?;
    }
    let mut patch = BTreeMap::new();
    if options.has("extensions") {
        patch = extensions_patch(options.value("extensions")).map_err(Failure::message)?;
    }
    let store = context.store()?;
    let status_changing = options.has("status") || claim;
    let updated = {
        // Go takes plans.jsonl before issues.jsonl. The guards are dropped
        // in the opposite order.
        let _plans = status_changing
            .then(|| store.lock(DataFile::Plans))
            .transpose()?;
        let _issues = store.lock(DataFile::Issues)?;
        let mut tickets = store.read_tickets()?;
        let Some(index) = ticket_index(&tickets, id) else {
            return Err(Failure::message(format!("Issue not found: {id}")));
        };
        let ticket = &mut tickets[index];
        let old_status = ticket.status.clone();
        if claim {
            if ticket.status != "open" {
                let by = if ticket.assignee.is_empty() {
                    String::new()
                } else {
                    format!(" by {}", ticket.assignee)
                };
                return Err(Failure::message(format!(
                    "Cannot claim {id}: status is {}{by}",
                    ticket.status
                )));
            }
            if !ticket.assignee.is_empty() && ticket.assignee != claimant {
                return Err(Failure::message(format!(
                    "Cannot claim {id}: already assigned to {}",
                    ticket.assignee
                )));
            }
            "in_progress".clone_into(&mut ticket.status);
            ticket.assignee.clone_from(&claimant);
        }
        if options.has("status") {
            options.value("status").clone_into(&mut ticket.status);
            if ticket.status != "closed" {
                ticket.closed_at.clear();
                ticket.close_reason.clear();
            }
        }
        if options.has("title") {
            let title = options.value("title").trim();
            if title.is_empty() {
                return Err(Failure::message("--title must not be empty"));
            }
            title.clone_into(&mut ticket.title);
        }
        if options.has("assignee") {
            options.value("assignee").clone_into(&mut ticket.assignee);
        }
        if options.has("description") {
            options
                .value("description")
                .clone_into(&mut ticket.description);
        } else if options.has("desc") {
            options.value("desc").clone_into(&mut ticket.description);
        } else if options.has("body") {
            options.value("body").clone_into(&mut ticket.description);
        }
        if options.has("type") {
            options.value("type").clone_into(&mut ticket.ticket_type);
        }
        if options.has("priority") {
            ticket.priority = priority;
        }
        if options.has("intent") {
            ticket.intent = intent;
        }
        if options.has("clear-extensions") {
            ticket.extensions = None;
        } else if options.has("extensions") {
            let extensions = ticket.extensions.get_or_insert_with(BTreeMap::new);
            extensions.extend(patch);
            if extensions.is_empty() {
                ticket.extensions = None;
            }
        }
        if options.has("set-labels") {
            ticket.labels = split_labels(options.value("set-labels"));
        }
        if options.has("add-label") {
            let mut labels = ticket.labels.take().unwrap_or_default();
            labels.extend(split_labels(options.value("add-label")).unwrap_or_default());
            ticket.labels = Some(unique_strings(labels));
        }
        if options.has("remove-label") {
            for label in split_labels(options.value("remove-label")).unwrap_or_default() {
                ticket.labels = without_string(ticket.labels.take(), &label);
            }
        }
        ticket.updated_at = model::now_iso();
        let updated = ticket.clone();
        if status_changing && old_status != updated.status {
            super::plans::check_status_change(&store, &[&updated])?;
            super::workflow::lossy::refuse_lossy_issues_rewrite(&store)?;
        }
        store.write_tickets(&tickets)?;
        if status_changing && old_status != updated.status {
            super::plans::apply_status_change(
                &store,
                std::slice::from_ref(&updated.id),
                &tickets,
                &updated.updated_at,
            )?;
        }
        updated
    };
    if options.has("json") {
        let record = updated.marshal_json()?;
        context.json_with_records(
            Envelope::new(true, "update").raw("issue", &record),
            model::GO_TYPE,
        )?;
    } else {
        context.success(format!("Updated {id}").as_bytes());
    }
    Ok(())
}

/// Go's `parseExtensionsPatch`.
fn extensions_patch(raw: &str) -> Result<BTreeMap<String, Value>, String> {
    let bytes = raw.as_bytes();
    if let Some(reason) = json_error::unmarshal_error(raw) {
        return Err(format!("--extensions must be valid JSON: {reason}"));
    }
    match Value::decode(bytes) {
        Ok(value) => value.into_object().ok_or_else(|| {
            "--extensions must be a JSON object (not array, null, or scalar)".to_owned()
        }),
        Err(error) => Err(format!("--extensions must be valid JSON: {error}")),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use super::super::{run, Invocation, Outcome};
    use super::*;

    fn record(extra: &str) -> String {
        format!(
            r#"{{"id":"p-1","title":"T","status":"open","type":"task","priority":2,"createdAt":"c","updatedAt":"u"{extra}}}"#
        )
    }

    fn project(issues: &[String], plans: &str) -> tempfile::TempDir {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join(".luuna/missions");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("config.yaml"), "project: \"p\"\nversion: \"1\"\n").unwrap();
        let mut content = issues.join("\n");
        if !content.is_empty() {
            content.push('\n');
        }
        fs::write(dir.join("issues.jsonl"), content).unwrap();
        fs::write(dir.join("plans.jsonl"), plans).unwrap();
        temp
    }

    fn update(root: &Path, args: &[&str]) -> Result<Outcome> {
        let mut all = vec!["update"];
        all.extend_from_slice(args);
        run(&Invocation::new(root, all))
    }

    fn files(root: &Path) -> (Vec<u8>, Vec<u8>) {
        let dir = root.join(".luuna/missions");
        (
            fs::read(dir.join("issues.jsonl")).unwrap(),
            fs::read(dir.join("plans.jsonl")).unwrap(),
        )
    }

    fn leftover_locks(root: &Path) -> Vec<String> {
        fs::read_dir(root.join(".luuna/missions"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| name.contains(".lock") || name.contains(".tmp"))
            .collect()
    }

    #[test]
    fn a_status_change_on_a_ticket_with_a_plan_id_is_refused() {
        let temp = project(&[record(r#","plan_id":"pl-1""#)], "");
        for args in [
            &["p-1", "--status", "in_progress"][..],
            &["p-1", "--status", "closed", "--title", "other"],
            &["p-1", "--claim", "--as", "me"],
        ] {
            super::super::plans::foundation_checks::update(temp.path(), args, 0);
        }
    }

    #[test]
    fn a_status_change_on_the_seed_of_a_plan_is_refused() {
        let plan = r#"{"id":"pl-1","seed":"p-1","template":"t","status":"draft","children":[]}"#;
        let temp = project(&[record("")], &format!("{plan}\n"));
        super::super::plans::foundation_checks::update(
            temp.path(),
            &["p-1", "--status", "closed", "--json"],
            1,
        );
    }

    #[test]
    fn a_child_of_a_plan_is_refused_too() {
        let plan =
            r#"{"id":"pl-1","seed":"s","template":"t","status":"active","children":["p-1"]}"#;
        let temp = project(&[record("")], &format!("{plan}\n"));
        super::super::plans::foundation_checks::update(
            temp.path(),
            &["p-1", "--status", "in_progress"],
            2,
        );
    }

    #[test]
    fn other_edits_on_a_plan_ticket_and_unchanged_statuses_are_allowed() {
        let plan = r#"{"id":"pl-1","seed":"p-1","template":"t","status":"draft","children":[]}"#;
        let temp = project(&[record(r#","plan_id":"pl-1""#)], &format!("{plan}\n"));
        let outcome = update(temp.path(), &["p-1", "--title", "New"]).unwrap();
        assert_eq!(outcome.exit_code, 0);
        let outcome = update(temp.path(), &["p-1", "--status", "open"]).unwrap();
        assert_eq!(outcome.exit_code, 0);
        let (issues, _) = files(temp.path());
        assert!(String::from_utf8(issues)
            .unwrap()
            .contains("\"title\":\"New\""));
    }

    #[test]
    fn a_status_change_without_a_plan_is_applied() {
        let temp = project(&[record("")], "");
        let outcome = update(temp.path(), &["p-1", "--status", "in_progress", "--json"]).unwrap();
        assert_eq!(outcome.exit_code, 0);
        let stdout = String::from_utf8(outcome.stdout).unwrap();
        assert!(stdout.contains("\"status\": \"in_progress\""), "{stdout}");
        assert_eq!(leftover_locks(temp.path()), Vec::<String>::new());
    }

    #[test]
    fn label_edits_follow_go() {
        let temp = project(&[record(r#","labels":["a","b"]"#)], "");
        let labels = |root: &Path| -> String {
            let (issues, _) = files(root);
            String::from_utf8(issues).unwrap()
        };
        update(temp.path(), &["p-1", "--add-label", "B, c"]).unwrap();
        assert!(labels(temp.path()).contains(r#""labels":["a","b","c"]"#));
        update(temp.path(), &["p-1", "--remove-label", "a,c"]).unwrap();
        assert!(labels(temp.path()).contains(r#""labels":["b"]"#));
        update(
            temp.path(),
            &["p-1", "--set-labels", "x,y", "--remove-label", "x"],
        )
        .unwrap();
        assert!(labels(temp.path()).contains(r#""labels":["y"]"#));
        update(temp.path(), &["p-1", "--remove-label", "y"]).unwrap();
        assert!(!labels(temp.path()).contains("labels"));
        // An add of nothing leaves an empty, non-nil list, written last.
        update(temp.path(), &["p-1", "--add-label", ","]).unwrap();
        assert!(
            labels(temp.path()).ends_with(",\"labels\":[]}\n"),
            "{}",
            labels(temp.path())
        );
    }

    #[test]
    fn extension_patches_are_checked_as_go_checks_them() {
        for (patch, message) in [
            ("{", "--extensions must be valid JSON: unexpected end of JSON input"),
            ("", "--extensions must be valid JSON: unexpected end of JSON input"),
            (
                "{a:1}",
                "--extensions must be valid JSON: invalid character 'a' looking for beginning of object key string",
            ),
            (
                "{\"a\":1e999}",
                "--extensions must be valid JSON: json: cannot unmarshal number 1e999 into Go struct field .a of type float64",
            ),
            ("[1]", "--extensions must be a JSON object (not array, null, or scalar)"),
            ("null", "--extensions must be a JSON object (not array, null, or scalar)"),
            ("3", "--extensions must be a JSON object (not array, null, or scalar)"),
        ] {
            assert_eq!(extensions_patch(patch).unwrap_err(), message, "{patch}");
        }
        let patch = extensions_patch(r#"{"a":1,"a":2,"b":{"c":null}}"#).unwrap();
        assert_eq!(patch.len(), 2);
        assert_eq!(patch["a"], Value::Number(2.0));
    }
    fn loss_snapshot(root: &Path) -> Vec<(Vec<u8>, fs::Permissions)> {
        ["issues.jsonl", "plans.jsonl"]
            .into_iter()
            .map(|name| {
                let path = root.join(".luuna/missions").join(name);
                (
                    fs::read(&path).unwrap(),
                    fs::metadata(path).unwrap().permissions(),
                )
            })
            .collect()
    }
    fn raw_record(extra: &[u8]) -> Vec<u8> {
        let raw = record("");
        [&raw.as_bytes()[..raw.len() - 1], extra, b"}\n"].concat()
    }
    fn loss_project(data: &[u8]) -> tempfile::TempDir {
        let t = project(&[record("")], "");
        let path = t.path().join(".luuna/missions/issues.jsonl");
        fs::write(&path, data).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(path, fs::Permissions::from_mode(0o640)).unwrap();
        }
        t
    }
    fn assert_loss(root: &Path, flags: &[&str], text: &str, json: bool) {
        let before = loss_snapshot(root);
        let mut args = vec!["p-1"];
        args.extend(flags);
        if json {
            args.push("--json")
        }
        let out = update(root, &args).unwrap();
        assert_eq!(out.exit_code, 1, "{args:?}: {out:?}");
        if json {
            let expected = format!(
                "{{\n  \"success\": false,\n  \"command\": \"update\",\n  \"error\": {}\n}}\n",
                serde_json::to_string(text).unwrap()
            );
            assert_eq!(out.stdout, expected.as_bytes());
            assert!(out.stderr.is_empty());
        } else {
            assert!(out.stdout.is_empty());
            assert_eq!(out.stderr, format!("Error: {text}\n").as_bytes());
        }
        assert_eq!(loss_snapshot(root), before);
        assert!(leftover_locks(root).is_empty());
    }
    #[test]
    fn a_lossy_issues_file_is_refused_unchanged() {
        for (i, tail) in [
            b"{not json\n".as_slice(),
            b"[1]\n",
            b"null\n",
            b"{\"id\":\"other\",\"extensions\":{\"n\":1e999}}\n",
        ]
        .into_iter()
        .enumerate()
        {
            let t = loss_project(&[raw_record(b""), tail.to_vec()].concat());
            assert_loss(t.path(),&["--title","Changed"],"Refusing to rewrite issues.jsonl: line 2 is not a ticket record the Go tool can read, and rewriting the file would drop it. Nothing was written.",i==0);
        }
        for (data,text) in [([raw_record(b""),b"{\"id\":\"p-1\"}\n".to_vec()].concat(),"Refusing to rewrite issues.jsonl: lines 1 and 2 both hold the ticket \"p-1\", and rewriting the file would keep only the last one. Nothing was written."),([raw_record(b""),b"{}\n{}\n".to_vec()].concat(),"Refusing to rewrite issues.jsonl: lines 2 and 3 both hold a record without an id, and rewriting the file would keep only the last one. Nothing was written."),(raw_record(br#", "title":"again""#),"Refusing to rewrite issues.jsonl: the record on line 1 repeats the key \"title\", and rewriting the file would keep only its last value. Nothing was written.")]{let t=loss_project(&data);assert_loss(t.path(),&["--title","Changed"],text,false);}
    }
    #[test]
    fn labels_go_cannot_rewrite_are_refused_unchanged() {
        for (raw, kind) in [
            (br#""urgent""#.as_slice(), 'd'),
            (b"5", 'd'),
            (br#"{"x":1}"#, 'd'),
            (br#"["a",5]"#, 'e'),
            (br#"["a",{}]"#, 'e'),
            (br#"["a",null]"#, 'e'),
            (b"[\"a\",\"\xff\"]", 'f'),
            (br#"["a","\ud800"]"#, 'f'),
        ] {
            let extra = [b",\"labels\":".as_slice(), raw].concat();
            let text=match kind{'d'=>"a \"labels\" value that is not a list of strings, and changing the field would replace that value",'e'=>"an element of \"labels\" that is not a string, and changing the field would write it as an empty string",_=>"an element of \"labels\" with invalid UTF-8 or a lone surrogate escape, and changing the field would write it as U+FFFD"};
            let text=format!("Refusing to rewrite issues.jsonl: line 1 (ticket \"p-1\") holds {text}. Nothing was written.");
            for remove in [false, true] {
                if remove && kind == 'd' {
                    continue;
                }
                let t = loss_project(&raw_record(&extra));
                assert_loss(
                    t.path(),
                    &[
                        if remove {
                            "--remove-label"
                        } else {
                            "--add-label"
                        },
                        if remove { "a" } else { "zz" },
                    ],
                    &text,
                    !remove && kind == 'd',
                );
            }
        }
    }
    #[test]
    fn extensions_go_cannot_rewrite_are_refused_unchanged() {
        for (raw,why) in [(br#""x""#.as_slice(),"an \"extensions\" value that is not an object, and changing the field would replace that value"),(b"[1,2]","an \"extensions\" value that is not an object, and changing the field would replace that value"),(b"5","an \"extensions\" value that is not an object, and changing the field would replace that value"),(b"{\"a\":\"\xff\"}","a key or string in \"extensions\" with invalid UTF-8 or a lone surrogate escape, and changing the field would write it as U+FFFD"),(b"{\"\xff\":1}","a key or string in \"extensions\" with invalid UTF-8 or a lone surrogate escape, and changing the field would write it as U+FFFD"),(br#"{"a":"\ud800"}"#,"a key or string in \"extensions\" with invalid UTF-8 or a lone surrogate escape, and changing the field would write it as U+FFFD"),(br#"{"\ud800":1}"#,"a key or string in \"extensions\" with invalid UTF-8 or a lone surrogate escape, and changing the field would write it as U+FFFD"),(br#"{"n":9007199254740993}"#,"the number 9007199254740993 in \"extensions\", and changing the field would write it as 9007199254740992"),(br#"{"o":{"n":9007199254740993}}"#,"the number 9007199254740993 in \"extensions\", and changing the field would write it as 9007199254740992"),(br#"{"a":1,"a":2}"#,"an object in \"extensions\" that repeats the key \"a\", and changing the field would keep only its last value")]{let extra=[b",\"extensions\":".as_slice(),raw].concat();let t=loss_project(&raw_record(&extra));let text=format!("Refusing to rewrite issues.jsonl: line 1 (ticket \"p-1\") holds {why}. Nothing was written.");assert_loss(t.path(),&["--extensions",r#"{"k":1}"#],&text,raw==b"5");}
    }
    #[test]
    fn values_go_reads_back_the_same_are_written_as_go_writes_them() {
        for (extra, flags) in [
            (br#", "labels":null"#.as_slice(), vec!["--add-label", "zz"]),
            (br#", "labels":["a","a"]"#, vec!["--add-label", "zz"]),
            (br#", "labels":["a",5]"#, vec!["--remove-label", "nope"]),
            (br#", "labels":"urgent""#, vec!["--set-labels", "x"]),
            (br#", "extensions":"x""#, vec!["--clear-extensions"]),
            (
                br#", "extensions":{"n":9007199254740993}"#,
                vec!["--clear-extensions"],
            ),
            (
                br#", "extensions":{"k":1,"n":9007199254740993}"#,
                vec!["--extensions", r#"{"k":1}"#],
            ),
        ] {
            let t = loss_project(&raw_record(extra));
            let mut args = vec!["p-1"];
            args.extend(flags);
            let out = update(t.path(), &args).unwrap();
            assert_eq!(out.exit_code, 0, "{out:?}");
            assert!(leftover_locks(t.path()).is_empty());
            let value = String::from_utf8(files(t.path()).0).unwrap();
            if args.contains(&"--clear-extensions") {
                if extra == br#", "extensions":"x""# {
                    assert!(value.contains("\"extensions\":\"x\""));
                } else {
                    assert!(!value.contains("\"extensions\""));
                }
            } else if args.contains(&"--set-labels") {
                assert!(value.contains("\"labels\":[\"x\"]"));
            } else if args.contains(&"--add-label") {
                assert!(value.contains(if extra.ends_with(b"null") {
                    "\"labels\":[\"zz\"]"
                } else {
                    "\"labels\":[\"a\",\"zz\"]"
                }));
            } else if args.contains(&"--extensions") {
                assert!(value.contains("9007199254740993"));
            } else {
                assert!(value.contains("\"labels\":[\"a\",5]"));
            }
        }
        let t = loss_project(&raw_record(br#", "extensions":{"n":9007199254740993}"#));
        let before = loss_snapshot(t.path());
        let out = update(
            t.path(),
            &["p-1", "--clear-extensions", "--extensions", r#"{"k":1}"#],
        )
        .unwrap();
        assert_eq!(out.exit_code, 1);
        assert_eq!(
            out.stderr,
            b"Error: --extensions and --clear-extensions are mutually exclusive\n"
        );
        assert_eq!(loss_snapshot(t.path()), before);
        for raw in [
            b"null".as_slice(),
            br#"{"a":null}"#,
            br#"{"n":1.50}"#,
            br#"{"n":1e21}"#,
            br#"{"a":"<&>"}"#,
            br#"{"b":1,"a":2}"#,
        ] {
            let t = loss_project(&raw_record(&[b",\"extensions\":".as_slice(), raw].concat()));
            let out = update(t.path(), &["p-1", "--extensions", r#"{"k":1}"#]).unwrap();
            assert_eq!(out.exit_code, 0);
            let value: serde_json::Value = serde_json::from_slice(&files(t.path()).0).unwrap();
            assert_eq!(value["extensions"]["k"], 1);
        }
    }
    #[test]
    fn extensions_go_cannot_read_answer_issue_not_found() {
        let t = loss_project(&raw_record(br#", "extensions":{"n":1e999}"#));
        let before = loss_snapshot(t.path());
        let out = update(t.path(), &["p-1", "--extensions", r#"{"k":1}"#]).unwrap();
        assert_eq!(out.exit_code, 1);
        assert_eq!(out.stderr, b"Error: Issue not found: p-1\n");
        assert_eq!(loss_snapshot(t.path()), before);
    }
    fn parent_files(root: &Path) -> BTreeMap<String, (Vec<u8>, fs::Permissions, bool)> {
        fs::read_dir(root)
            .unwrap()
            .filter_map(|entry| {
                let entry = entry.unwrap();
                let name = entry.file_name().into_string().unwrap();
                if name == "project" {
                    return None;
                }
                let meta = entry.metadata().unwrap();
                let bytes = if meta.is_dir() {
                    let mut children = fs::read_dir(entry.path())
                        .unwrap()
                        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                        .collect::<Vec<_>>();
                    children.sort();
                    children.join("\n").into_bytes()
                } else {
                    fs::read(entry.path()).unwrap()
                };
                Some((name, (bytes, meta.permissions(), meta.is_dir())))
            })
            .collect()
    }
    #[test]
    fn update_leaves_the_parent_of_the_temporary_project_unchanged() {
        let parent = tempfile::tempdir().unwrap();
        let root = parent.path().join("project");
        let dir = root.join(".luuna/missions");
        fs::create_dir_all(&dir).unwrap();
        fs::create_dir(parent.path().join(".luuna")).unwrap();
        fs::write(parent.path().join("sentinel"), b"parent data").unwrap();
        fs::write(parent.path().join(".gitattributes"), b"parent attrs").unwrap();
        fs::write(dir.join("config.yaml"), b"project: p\nversion: \"1\"\n").unwrap();
        fs::write(dir.join("plans.jsonl"), b"").unwrap();
        let parent_before = parent_files(parent.path());
        let before = fs::read(parent.path().join("sentinel")).unwrap();
        let attrs = fs::read(parent.path().join(".gitattributes")).unwrap();
        let parent_mode = fs::metadata(parent.path()).unwrap().permissions();
        fs::write(
            dir.join("issues.jsonl"),
            [raw_record(b""), b"unreadable\n".to_vec()].concat(),
        )
        .unwrap();
        assert_eq!(
            update(&root, &["p-1", "--title", "Changed"])
                .unwrap()
                .exit_code,
            1
        );
        fs::write(dir.join("issues.jsonl"), raw_record(b"")).unwrap();
        assert_eq!(
            update(&root, &["p-1", "--title", "Changed"])
                .unwrap()
                .exit_code,
            0
        );
        assert_eq!(fs::read(parent.path().join("sentinel")).unwrap(), before);
        assert_eq!(
            fs::read(parent.path().join(".gitattributes")).unwrap(),
            attrs
        );
        assert_eq!(
            fs::metadata(parent.path()).unwrap().permissions(),
            parent_mode
        );
        assert_eq!(parent_files(parent.path()), parent_before);
        assert_eq!(fs::read_dir(parent.path()).unwrap().count(), 4);
        assert_eq!(
            fs::read_dir(parent.path().join(".luuna")).unwrap().count(),
            0
        );
    }
}
