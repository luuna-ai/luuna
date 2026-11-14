//! Outcome reads and locked appends.
use crate::{
    actions::record::{get, has, id_text, number_flag, text, Action},
    json::{Object, Value},
    records, store, timefmt, Result,
};
fn status(a: &Action<'_>, value: &str) -> String {
    a.styled(
        match value {
            "success" => "green",
            "failure" => "red",
            _ => "yellow",
        },
        value,
    )
}
pub(crate) fn run(a: &mut Action<'_>, args: &[String], flags: &[(String, String)]) -> Result<()> {
    let domain = &args[0];
    let id = &args[1];
    let cfg = a.config()?;
    if !a.domain(&cfg, domain, "outcome") {
        return Ok(());
    }
    let path = a.path(domain)?;
    if get(flags, "status").is_empty() {
        let rows = store::read_records(&path, &a.registry, a.ctx.globals.allow_unknown_types)?;
        let index = records::resolve_record_id(&rows, id)?;
        let r = &rows[index];
        let outcomes = records::record_list(r.get("outcomes"));
        if a.ctx.globals.json {
            let mut data = Object::new();
            data.insert_unordered("domain", domain.clone());
            data.insert_unordered(
                "outcomes",
                Value::Array(
                    outcomes
                        .iter()
                        .map(|v| Value::Object((*v).clone()))
                        .collect(),
                ),
            );
            if let Some(v) = r.get("id") {
                data.insert_unordered("id", v.clone());
            }
            a.json("outcome", data);
        } else if outcomes.is_empty() {
            a.human(a.styled("dim", "No outcomes recorded for this record."));
        } else {
            a.human(format!(
                "{} for {} ({}):",
                a.styled("brand", "Outcomes"),
                a.styled("accent", &id_text(r, id)),
                outcomes.len()
            ));
            for (index, o) in outcomes.iter().enumerate() {
                let agent = text(o.get("agent"));
                let agent = if agent.is_empty() {
                    String::new()
                } else {
                    a.styled("dim", &format!(" ({agent})"))
                };
                a.human(format!(
                    "  {} {}{agent}",
                    a.styled("dim", &format!("{}.", index + 1)),
                    status(a, &text(o.get("status")))
                ));
                for (key, label) in [
                    ("duration", "duration"),
                    ("test_results", "tests"),
                    ("notes", "notes"),
                    ("recorded_at", "recorded"),
                ] {
                    if let Some(value) = o
                        .get(key)
                        .filter(|v| !v.is_null() && v.as_str() != Some(""))
                    {
                        let suffix = if key == "duration" { "ms" } else { "" };
                        a.human(a.styled(
                            "dim",
                            &format!("     {label}: {}{suffix}", text(Some(value))),
                        ));
                    }
                }
            }
        }
        return Ok(());
    }
    store::with_file_lock(&path, || {
        let mut rows = store::read_records(&path, &a.registry, a.ctx.globals.allow_unknown_types)?;
        let index = records::resolve_record_id(&rows, id)?;
        let before = rows.clone();
        let mut r = rows[index].clone();
        let mut o = Object::new();
        o.insert("status", get(flags, "status"));
        o.insert("recorded_at", timefmt::now_iso());
        if has(flags, "duration") {
            o.insert(
                "duration",
                number_flag(&get(flags, "duration"), "duration")?,
            );
        }
        for key in ["agent", "notes", "test-results"] {
            let value = get(flags, key);
            if !value.is_empty() {
                o.insert(key.replace('-', "_"), value);
            }
        }
        records::check_outcomes_collection(&path, &r, true)?;
        let mut outcomes = records::record_list(r.get("outcomes"))
            .into_iter()
            .cloned()
            .map(Value::Object)
            .collect::<Vec<_>>();
        outcomes.push(Value::Object(o.clone()));
        let total = outcomes.len();
        let mut order = r.keys().map(str::to_owned).collect::<Vec<_>>();
        if !r.contains_key("outcomes") {
            order.push("outcomes".into());
        }
        r.insert_unordered("outcomes", Value::Array(outcomes));
        r.remember_order(order);
        rows[index] = r;
        records::check_lossless_rewrite(&path, &a.registry, &[])?;
        records::check_retained_ids(&path, before.iter().zip(rows.iter()))?;
        records::check_retained_numbers(&path, before.iter().zip(rows.iter()))?;
        records::assign_record_ids(&mut rows, &a.registry)?;
        store::write_records(&path, &mut rows, &a.registry)?;
        let r = &rows[index];
        if a.ctx.globals.json {
            let mut data = Object::new();
            data.insert("action", "appended");
            data.insert("domain", domain.clone());
            if let Some(v) = r.get("id") {
                data.insert("id", v.clone());
            }
            data.insert("outcome", o);
            data.insert("total_outcomes", total as i64);
            a.json("outcome", data);
        } else {
            let agent = text(o.get("agent"));
            let agent = if agent.is_empty() {
                String::new()
            } else {
                a.styled("dim", &format!(" ({agent})"))
            };
            a.human(format!(
                "{} {} {}{agent} on {}",
                a.styled("brand", "✓"),
                a.styled("brand", "Outcome recorded:"),
                status(a, &text(o.get("status"))),
                a.styled("accent", &id_text(r, id))
            ));
        }
        Ok(())
    })
}
