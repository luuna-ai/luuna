//! Native record lookup.
use super::{
    record::{self, domain_names, message, text},
    render,
};
use crate::{
    json::{Object, Value},
    options::Context,
    records, store, CommandOutput,
};
pub fn run_show(ctx: &Context, ids: &[String]) -> CommandOutput {
    record::run(ctx, "show", |a| {
        let cfg = a.config()?;
        let mut all = Vec::new();
        for domain in domain_names(&cfg) {
            for row in store::read_records(
                &a.path(&domain)?,
                &a.registry,
                a.ctx.globals.allow_unknown_types,
            )? {
                all.push((domain.clone(), row));
            }
        }
        let mut found = Vec::new();
        for id in ids {
            let prefix = format!("mx-{}", id.strip_prefix("mx-").unwrap_or(id));
            let exact = all
                .iter()
                .filter(|(_, r)| text(r.get("id")) == prefix)
                .collect::<Vec<_>>();
            let matches = if exact.is_empty() {
                all.iter()
                    .filter(|(_, r)| text(r.get("id")).starts_with(&prefix))
                    .collect::<Vec<_>>()
            } else {
                exact
            };
            if matches.is_empty() {
                let tick = char::from(96);
                return Err(message(format!("Record {} not found. Run {tick}luuna-atlas search{tick} or {tick}luuna-atlas prime{tick} to see record IDs.",records::go_quote(id))));
            }
            if matches.len() > 1 {
                return Err(message(format!("Ambiguous identifier {} matches {} records: {}. Use more characters to disambiguate.",records::go_quote(id),matches.len(),matches.iter().map(|(d,r)|format!("{} ({d})",text(r.get("id")))).collect::<Vec<_>>().join(", "))));
            }
            found.push(matches[0]);
        }
        if a.ctx.globals.json {
            let rows = found
                .iter()
                .map(|(d, r)| {
                    let mut o = Object::new();
                    o.insert("domain", d.clone());
                    o.insert("record", r.clone());
                    o.remember_order(["domain", "record"]);
                    Value::Object(o)
                })
                .collect();
            let mut o = Object::new();
            o.insert("records", Value::Array(rows));
            a.json("show", o);
        } else {
            let mut blocks = Vec::new();
            for (d, r) in found {
                let body = render::format_type_records(std::slice::from_ref(r), &a.registry, true);
                let body = if body.is_empty() {
                    crate::json::to_indented_string(&Value::Object(r.clone()))?
                } else {
                    body
                };
                blocks.push(format!("## {d}\n\n{body}"));
            }
            a.output
                .stdout
                .push_str(&format!("{}\n", blocks.join("\n\n")));
        }
        Ok(())
    })
}
