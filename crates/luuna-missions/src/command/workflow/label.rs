//! Go's label operations and exact list/count ordering.
use super::{
    lossy,
    render::{sorted_int_map, string_array},
};
use crate::command::{
    options,
    output::Envelope,
    read::println,
    shared::{go_lower, ticket_index, unique_strings, without_string},
    Context, Failure,
};
use crate::{model, store::DataFile};
use std::collections::BTreeMap;
pub(super) fn run(context: &mut Context<'_>, args: &[String]) -> Result<(), Failure> {
    let opts = options::parse(args, &[], &["json"])?;
    let p = &opts.positionals;
    if p.is_empty() {
        return Err(Failure::message(
            "Usage: luuna-missions label <add|remove|list|list-all>",
        ));
    }
    let cmd = p[0].as_str();
    let store = context.store()?;
    if cmd == "list-all" {
        let mut counts: BTreeMap<String, i64> = BTreeMap::new();
        for t in store.read_tickets()? {
            for label in t.labels.unwrap_or_default() {
                *counts.entry(label).or_default() += 1;
            }
        }
        let labels: Vec<String> = counts.keys().cloned().collect();
        if opts.has("json") {
            context.json(
                Envelope::new(true, "label list-all")
                    .raw("labels", &string_array(&labels))
                    .raw("counts", &sorted_int_map(&counts)),
            );
        } else if labels.is_empty() {
            println(context, "No labels found.");
        } else {
            for label in &labels {
                println(context, &format!("  {label:<20} {}", counts[label]));
            }
            println(context, &format!("\n{} label(s)", labels.len()));
        }
        return Ok(());
    }
    if cmd == "list" {
        if p.len() < 2 {
            return Err(Failure::message("Usage: luuna-missions label list <issue>"));
        }
        let tickets = store.read_tickets()?;
        let i = ticket_index(&tickets, &p[1])
            .ok_or_else(|| Failure::message(format!("Issue not found: {}", p[1])))?;
        let labels = tickets[i].labels.as_deref().unwrap_or_default();
        if opts.has("json") {
            context.json(
                Envelope::new(true, "label list")
                    .string("issueId", &p[1])
                    .raw("labels", &string_array(labels)),
            );
        } else if labels.is_empty() {
            println(context, &format!("{} has no labels.", p[1]));
        } else {
            println(context, &format!("{} labels:", p[1]));
            for label in labels {
                println(context, &format!("  {label}"));
            }
        }
        return Ok(());
    }
    if cmd != "add" && cmd != "remove" {
        return Err(Failure::message(format!(
            "Unknown label subcommand: {cmd}. Use add, remove, list, or list-all."
        )));
    }
    if p.len() < 3 {
        return Err(Failure::message(format!(
            "Usage: luuna-missions label {cmd} <issue> <label> [<label2> ...]"
        )));
    }
    let labels = unique_strings(
        p[2..]
            .iter()
            .map(|s| go_lower(s.trim()))
            .filter(|s| !s.is_empty())
            .collect(),
    );
    if cmd == "add" && labels.is_empty() {
        return Err(Failure::message("No valid labels provided"));
    }
    {
        let _lock = store.lock(DataFile::Issues)?;
        let before = store.read_tickets()?;
        let i = ticket_index(&before, &p[1])
            .ok_or_else(|| Failure::message(format!("Issue not found: {}", p[1])))?;
        let mut after = before.clone();
        if cmd == "add" {
            let mut all = after[i].labels.take().unwrap_or_default();
            all.extend(labels.iter().cloned());
            after[i].labels = Some(unique_strings(all));
        } else {
            for label in &labels {
                after[i].labels = without_string(after[i].labels.take(), label);
            }
        }
        after[i].updated_at = model::now_iso();
        lossy::refuse_lossy_ticket_changes(&store, &before, &after)?;
        store.write_tickets(&after)?;
    }
    if opts.has("json") {
        context.json(
            Envelope::new(true, &format!("label {cmd}"))
                .string("issueId", &p[1])
                .raw("labels", &string_array(&labels)),
        );
    } else if cmd == "add" {
        context.success(format!("Added label(s) {} to {}", labels.join(", "), p[1]).as_bytes());
    } else {
        context.success(format!("Removed label(s) from {}", p[1]).as_bytes());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::dep::test_support::*;
    #[test]
    fn labels_are_trimmed_lowered_and_unique() {
        let t = project(&base());
        let out = invoke(
            t.path(),
            &["label", "add", "t", " API ", "api", "a,b", " ", "--json"],
        );
        assert_eq!(out.exit_code, 0);
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&out.stdout).unwrap()["labels"],
            serde_json::json!(["api", "a,b"])
        );
        assert_eq!(
            value(t.path(), "t")["labels"],
            serde_json::json!(["a", "api", "a,b"])
        );
    }
    #[test]
    fn list_all_counts_and_sorts() {
        let input = [
            record("t", "open", br#","labels":["z","a","a"]"#),
            record("b", "closed", br#","labels":["z"]"#),
        ]
        .concat();
        let t = project(&input);
        assert_eq!(
            invoke(t.path(), &["label", "list-all"]).stdout,
            b"  a                    2\n  z                    2\n\n2 label(s)\n"
        );
        let out = invoke(t.path(), &["label", "list-all", "--json"]);
        let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(v["labels"], serde_json::json!(["a", "z"]));
        assert_eq!(v["counts"], serde_json::json!({"a":2,"z":2}));
    }
    #[test]
    fn add_refuses_a_lossy_issues_file_unchanged() {
        file_refusals(&["label", "add", "t", "zz"]);
    }
    #[test]
    fn remove_refuses_a_lossy_issues_file_unchanged() {
        file_refusals(&["label", "remove", "t", "a"]);
    }
    #[test]
    fn add_refuses_a_list_go_cannot_rewrite_unchanged() {
        field_refusals(&["label", "add", "t", "zz"], "labels", false, true);
    }
    #[test]
    fn remove_refuses_a_list_go_cannot_rewrite_unchanged() {
        field_refusals(&["label", "remove", "t", "a"], "labels", false, false);
    }
    #[test]
    fn label_lists_left_unchanged_are_kept_byte_for_byte() {
        for raw in [br#""a""#.as_slice(), b"[\"\xff\",\"\xfe\"]", br#"["a",5]"#] {
            control(&["label", "remove", "t", "zz"], "labels", false, raw, 0);
        }
        control(
            &["label", "add", "t", "a"],
            "labels",
            false,
            br#"["a",5]"#,
            0,
        );
        for raw in [
            br#""urgent""#.as_slice(),
            br#"["a",5]"#,
            br#"["a",null]"#,
            b"[\"a\",\"\xff\"]",
            br#"["a","\ud800"]"#,
            b"[\"\xff\",\"\xfe\"]",
        ] {
            let input = [
                record("t", "open", &[b",\"labels\":", raw].concat()),
                record("o", "open", b""),
            ]
            .concat();
            let t = project(&input);
            assert_eq!(invoke(t.path(), &["label", "add", "o", "zz"]).exit_code, 0);
            let needle = [b"\"labels\":", raw].concat();
            assert!(bytes(t.path()).windows(needle.len()).any(|s| s == needle));
        }
    }
}
