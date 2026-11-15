//! Go's stats formats, including insertion-order ties and string-sorted JSON keys.
use super::render::sorted_int_map;
use crate::command::{
    options,
    output::{format_mode, Envelope},
    read::{closed_ids, effectively_blocked, println},
    Context, Failure,
};
use std::collections::BTreeMap;
pub(super) fn run(context: &mut Context<'_>, args: &[String]) -> Result<(), Failure> {
    let opts = options::parse(args, &["format"], &["json"])?;
    let mode = format_mode(&opts).map_err(Failure::message)?;
    if !opts.positionals.is_empty() {
        return Err(Failure::message("too many arguments for 'stats'"));
    }
    let tickets = context.store()?.read_tickets()?;
    let closed_ids = closed_ids(&tickets);
    let (mut open, mut progress, mut closed, mut blocked) = (0, 0, 0, 0);
    let (mut types, mut priorities, mut labels) = (
        BTreeMap::<String, i64>::new(),
        BTreeMap::<i64, i64>::new(),
        BTreeMap::<String, i64>::new(),
    );
    let (mut type_order, mut label_order) = (Vec::new(), Vec::new());
    for t in &tickets {
        match t.status.as_str() {
            "open" => open += 1,
            "in_progress" => progress += 1,
            "closed" => closed += 1,
            _ => {}
        }
        if t.status != "closed" && effectively_blocked(t, &closed_ids) {
            blocked += 1;
        }
        if !types.contains_key(&t.ticket_type) {
            type_order.push(t.ticket_type.clone());
        }
        *types.entry(t.ticket_type.clone()).or_default() += 1;
        *priorities.entry(t.priority).or_default() += 1;
        for label in t.labels.as_deref().unwrap_or_default() {
            if !labels.contains_key(label) {
                label_order.push(label.clone());
            }
            *labels.entry(label.clone()).or_default() += 1;
        }
    }
    label_order.sort_by(|a, b| labels[b].cmp(&labels[a]));
    if mode == "json" {
        let priority_map = priorities
            .iter()
            .map(|(k, v)| (k.to_string(), *v))
            .collect();
        let data = Envelope::object()
            .integer("total", tickets.len() as i64)
            .integer("open", open)
            .integer("inProgress", progress)
            .integer("closed", closed)
            .integer("blocked", blocked)
            .raw("byType", &sorted_int_map(&types))
            .raw("byPriority", &sorted_int_map(&priority_map))
            .raw("byLabel", &sorted_int_map(&labels))
            .render();
        context.json(Envelope::new(true, "stats").raw("stats", &data));
        return Ok(());
    }
    if mode == "ids" {
        return Ok(());
    }
    if mode == "compact" {
        println(
            context,
            &format!(
                "total={} open={open} in_progress={progress} closed={closed} blocked={blocked}",
                tickets.len()
            ),
        );
        if !types.is_empty() {
            println(
                context,
                &format!(
                    "by_type: {}",
                    type_order
                        .iter()
                        .map(|k| format!("{k}={}", types[k]))
                        .collect::<Vec<_>>()
                        .join(" ")
                ),
            );
        }
        if !priorities.is_empty() {
            println(
                context,
                &format!(
                    "by_priority: {}",
                    priorities
                        .iter()
                        .map(|(k, v)| format!("P{k}={v}"))
                        .collect::<Vec<_>>()
                        .join(" ")
                ),
            );
        }
        if !labels.is_empty() {
            println(
                context,
                &format!(
                    "by_label: {}",
                    label_order
                        .iter()
                        .map(|k| format!("{k}={}", labels[k]))
                        .collect::<Vec<_>>()
                        .join(" ")
                ),
            );
        }
        return Ok(());
    }
    println(context,&format!("Project Statistics\n  Total:       {}\n  Open:        {open}\n  In progress: {progress}\n  Closed:      {closed}\n  Blocked:     {blocked}\n\nBy Type",tickets.len()));
    for k in type_order {
        println(context, &format!("  {k:<10} {}", types[&k]));
    }
    if !priorities.is_empty() {
        println(context, "\nBy Priority");
        for (k, v) in priorities {
            let label = match k {
                0 => "Critical",
                1 => "High",
                2 => "Medium",
                3 => "Low",
                4 => "Backlog",
                _ => "",
            };
            println(context, &format!("  P{k} {label:<10} {v}"));
        }
    }
    if !labels.is_empty() {
        println(context, "\nBy Label");
        for k in label_order {
            println(context, &format!("  {k:<15} {}", labels[&k]));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::dep::test_support::*;
    #[test]
    fn counts_follow_go() {
        let input = [
            record("t", "open", br#","labels":["z","a","a"],"blockedBy":["b"]"#),
            record("b", "in_progress", br#","labels":["z"]"#),
            record("c", "closed", b""),
        ]
        .concat();
        let t = project(&input);
        let out = invoke(t.path(), &["stats", "--json"]);
        assert_eq!(out.exit_code, 0);
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&out.stdout).unwrap(),
            serde_json::json!({"success":true,"command":"stats","stats":{"total":3,"open":1,"inProgress":1,"closed":1,"blocked":1,"byType":{"task":3},"byPriority":{"2":3},"byLabel":{"a":2,"z":2}}})
        );
        let empty = project(b"");
        let out = invoke(empty.path(), &["stats", "--json"]);
        let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(
            v["stats"],
            serde_json::json!({"total":0,"open":0,"inProgress":0,"closed":0,"blocked":0,"byType":{},"byPriority":{},"byLabel":{}})
        );
    }
    #[test]
    fn text_and_compact_follow_go() {
        let input = [
            record("t", "open", br#","labels":["z","a","a"],"blockedBy":["b"]"#),
            record("b", "in_progress", br#","labels":["z"]"#),
            record("c", "closed", b""),
        ]
        .concat();
        let t = project(&input);
        assert_eq!(invoke(t.path(),&["stats","--format","compact"]).stdout,b"total=3 open=1 in_progress=1 closed=1 blocked=1\nby_type: task=3\nby_priority: P2=3\nby_label: z=2 a=2\n");
        let expected=b"Project Statistics\n  Total:       3\n  Open:        1\n  In progress: 1\n  Closed:      1\n  Blocked:     1\n\nBy Type\n  task       3\n\nBy Priority\n  P2 Medium     3\n\nBy Label\n  z               2\n  a               2\n";
        for args in [
            &["stats"][..],
            &["stats", "--format", "plain"],
            &["stats", "-q"],
        ] {
            assert_eq!(invoke(t.path(), args).stdout, expected);
        }
        assert!(invoke(t.path(), &["stats", "--format", "ids"])
            .stdout
            .is_empty());
        let empty = project(b"");
        assert_eq!(invoke(empty.path(),&["stats"]).stdout,b"Project Statistics\n  Total:       0\n  Open:        0\n  In progress: 0\n  Closed:      0\n  Blocked:     0\n\nBy Type\n");
    }
    #[test]
    fn by_priority_keys_sort_as_strings() {
        let input=b"{\"id\":\"a\",\"priority\":2}\n{\"id\":\"b\",\"priority\":10}\n{\"id\":\"c\",\"priority\":-1}\n";
        let t = project(input);
        let out = invoke(t.path(), &["stats", "--json"]);
        let text = String::from_utf8(out.stdout).unwrap();
        assert!(text.contains(
            "\"byPriority\": {\n      \"-1\": 1,\n      \"10\": 1,\n      \"2\": 1\n    }"
        ));
        assert!(
            String::from_utf8(invoke(t.path(), &["stats", "--format", "compact"]).stdout)
                .unwrap()
                .contains("by_priority: P-1=1 P2=1 P10=1\n")
        );
    }
}
