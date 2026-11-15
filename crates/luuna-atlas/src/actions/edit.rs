//! Native edit; ignored flags still cause Go's rewrite.
use super::record::{self, get, has, outcome, strings, text};
use crate::{
    json::{Object, Value},
    options::Context,
    records, schema, store, CommandOutput,
};
#[derive(Clone, Debug, Default)]
pub struct EditOptions {
    pub flags: Vec<(String, String)>,
}
pub fn run_edit(ctx: &Context, domain: &str, id: &str, options: &EditOptions) -> CommandOutput {
    record::run(ctx, "edit", |a| {
        let cfg = a.config()?;
        if !a.domain(&cfg, domain, "edit") {
            return Ok(());
        }
        let path = a.path(domain)?;
        store::with_file_lock(&path, || {
            let mut current =
                store::read_records(&path, &a.registry, a.ctx.globals.allow_unknown_types)?;
            let index = records::resolve_record_id(&current, id)?;
            let before = current.clone();
            let mut r = current[index].clone();
            let mut order: Vec<String> = r.keys().map(str::to_owned).collect();
            let mut set = |r: &mut Object, key: &str, value: Value| {
                if !r.contains_key(key) {
                    order.push(key.into());
                }
                r.insert_unordered(key, value);
            };
            let flags = &options.flags;
            if has(flags, "classification") {
                set(
                    &mut r,
                    "classification",
                    Value::from(get(flags, "classification")),
                );
            }
            for key in ["relates-to", "supersedes"] {
                if has(flags, key) {
                    set(
                        &mut r,
                        &key.replace('-', "_"),
                        strings(records::split_csv(&get(flags, key))),
                    );
                }
            }
            if let Some(next) = outcome(flags)? {
                records::check_outcomes_collection(&path, &r, true)?;
                let mut values: Vec<_> = records::record_list(r.get("outcomes"))
                    .into_iter()
                    .cloned()
                    .map(Value::Object)
                    .collect();
                values.push(Value::Object(next));
                set(&mut r, "outcomes", Value::Array(values));
            }
            if let Some(def) = a.registry.get(&text(r.get("type"))) {
                for field in def.required.iter().chain(&def.optional) {
                    let flag = field.replace('_', "-");
                    if has(flags, &flag) {
                        let value = if def.extracts_files && field == &def.files_field {
                            strings(records::split_csv(&get(flags, &flag)))
                        } else {
                            Value::from(get(flags, &flag))
                        };
                        set(&mut r, field, value);
                    }
                }
            }
            r.remember_order(order);
            let issues = schema::validate_record(&r, &a.registry)?;
            if !issues.is_empty() {
                if a.ctx.globals.json {
                    return Err(record::message(format!(
                        "Updated record failed schema validation: {}",
                        schema::issues_text(&issues)
                    )));
                }
                a.output.exit_code = 1;
                a.output.stderr.push_str(&format!(
                    "{}\n",
                    a.styled("red", "Error: updated record failed schema validation:")
                ));
                for issue in issues {
                    a.output.stderr.push_str(&format!(
                        "{}\n",
                        a.styled("red", &format!("  {} {}", issue.path, issue.message))
                    ));
                }
                return Ok(());
            }
            current[index] = r;
            records::check_lossless_rewrite(&path, &a.registry, &[])?;
            records::check_retained_ids(&path, before.iter().zip(current.iter()))?;
            records::check_retained_numbers(&path, before.iter().zip(current.iter()))?;
            records::assign_record_ids(&mut current, &a.registry)?;
            store::write_records(&path, &mut current, &a.registry)?;
            let r = &current[index];
            if a.ctx.globals.json {
                let mut fields = Object::new();
                fields.insert("domain", domain);
                fields.insert("id", r.get("id").cloned().unwrap_or(Value::Null));
                fields.insert("type", r.get("type").cloned().unwrap_or(Value::Null));
                fields.insert("record", r.clone());
                a.json("edit", fields);
            } else {
                let id = text(r.get("id"));
                let id = if id.is_empty() {
                    String::new()
                } else {
                    format!(" {}", a.styled("accent", &id))
                };
                a.human(format!(
                    "{} {}{id} {}",
                    a.styled("brand", "✓"),
                    a.styled("brand", &format!("Updated {}", text(r.get("type")))),
                    a.styled("brand", &format!("in {domain}"))
                ));
            }
            Ok(())
        })
    })
}
