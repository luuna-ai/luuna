//! `update` (`internal/cli/issue_mutate.go:135-312`).
//!
//! A status change on a ticket that belongs to a plan, or that is the seed of
//! one, runs Go's plan transition hook, which this plan does not port. It is
//! refused with [`Error::Unsupported`] before anything is written.

use std::collections::BTreeMap;

use super::output::Envelope;
use super::shared::{
    parse_intent, parse_priority, split_labels, ticket_index, unique_strings, validate_status,
    validate_type, without_string,
};
use super::{options, Context, Failure};
use crate::error::{Error, Result, PLAN_4};
use crate::json_error;
use crate::model::{self, Ticket, Value};
use crate::store::{read_records, DataFile, Record, Store};

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
            refuse_plan_transition(&store, &updated)?;
        }
        store.write_tickets(&tickets)?;
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

/// The part of a `plans.jsonl` record the plan transition hook looks at.
struct PlanLinks {
    line: Vec<u8>,
    id: String,
    seed: String,
    children: Vec<String>,
}

impl Record for PlanLinks {
    fn from_line(line: &[u8]) -> Option<Self> {
        let members = Value::decode(line).ok().and_then(Value::into_object)?;
        let text = |key: &str| match members.get(key) {
            Some(Value::String(text)) => text.clone(),
            _ => String::new(),
        };
        let children = match members.get("children") {
            Some(Value::Array(items)) => items
                .iter()
                .map(|item| match item {
                    Value::String(text) => text.clone(),
                    _ => String::new(),
                })
                .collect(),
            _ => Vec::new(),
        };
        Some(Self {
            line: line.to_vec(),
            id: text("id"),
            seed: text("seed"),
            children,
        })
    }

    fn record_id(&self) -> &str {
        &self.id
    }

    fn to_line(&self) -> Result<Vec<u8>> {
        Ok(self.line.clone())
    }
}

/// Refuses a status change that Go's plan transition hook would act on: the
/// ticket has a `plan_id`, is the seed of a plan, or is one of its children.
/// Plan transitions are ported by s4-missions-lib-4. Nothing has been written
/// when this runs.
fn refuse_plan_transition(store: &Store, ticket: &Ticket) -> Result<()> {
    let plans: Vec<PlanLinks> = read_records(&store.path(DataFile::Plans))?;
    let involved = !ticket.plan_id.is_empty()
        || plans.iter().any(|plan| {
            plan.seed == ticket.id() || plan.children.iter().any(|child| child == ticket.id())
        });
    if involved {
        return Err(Error::unsupported(
            format!(
                "a status change on {}, which belongs to a plan or is the seed of one (plan transitions)",
                ticket.id()
            ),
            PLAN_4,
        ));
    }
    Ok(())
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
        let before = files(temp.path());
        for args in [
            &["p-1", "--status", "in_progress"][..],
            &["p-1", "--status", "closed", "--title", "other"],
            &["p-1", "--claim", "--as", "me"],
        ] {
            match update(temp.path(), args) {
                Err(Error::Unsupported { plan, .. }) => assert_eq!(plan, "s4-missions-lib-4"),
                other => panic!("{args:?}: expected Unsupported, got {other:?}"),
            }
            assert_eq!(files(temp.path()), before, "{args:?}");
            assert_eq!(leftover_locks(temp.path()), Vec::<String>::new());
        }
    }

    #[test]
    fn a_status_change_on_the_seed_of_a_plan_is_refused() {
        let plan = r#"{"id":"pl-1","seed":"p-1","template":"t","status":"draft","children":[]}"#;
        let temp = project(&[record("")], &format!("{plan}\n"));
        let before = files(temp.path());
        match update(temp.path(), &["p-1", "--status", "closed", "--json"]) {
            Err(Error::Unsupported { plan, feature }) => {
                assert_eq!(plan, "s4-missions-lib-4");
                assert!(feature.contains("p-1"), "{feature}");
            }
            other => panic!("expected Unsupported, got {other:?}"),
        }
        assert_eq!(files(temp.path()), before);
        assert_eq!(leftover_locks(temp.path()), Vec::<String>::new());
    }

    #[test]
    fn a_child_of_a_plan_is_refused_too() {
        let plan =
            r#"{"id":"pl-1","seed":"s","template":"t","status":"active","children":["p-1"]}"#;
        let temp = project(&[record("")], &format!("{plan}\n"));
        let before = files(temp.path());
        let result = update(temp.path(), &["p-1", "--status", "in_progress"]);
        assert!(
            matches!(result, Err(Error::Unsupported { .. })),
            "{result:?}"
        );
        assert_eq!(files(temp.path()), before);
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
}
