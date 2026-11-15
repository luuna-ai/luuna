//! Native deletion, including dry-run locking and atomic multi-ID resolution.
use super::record::{self, message, text};
use crate::{
    json::{Object, Value},
    options::Context,
    records, store, CommandOutput,
};
#[derive(Clone, Debug, Default)]
pub struct DeleteOptions {
    pub records: Option<String>,
    pub all_except: Option<String>,
    pub dry_run: bool,
}
fn info(r: &Object, registry: &crate::registry::Registry) -> Object {
    let mut out = Object::new();
    out.insert_unordered("id", r.get("id").cloned().unwrap_or(Value::Null));
    out.insert_unordered("type", r.get("type").cloned().unwrap_or(Value::Null));
    out.insert_unordered("summary", records::record_summary(r, registry));
    out
}
pub fn run_delete(
    ctx: &Context,
    domain: &str,
    id: Option<&str>,
    options: &DeleteOptions,
) -> CommandOutput {
    record::run(ctx, "delete", |a| {
        let cfg = a.config()?;
        if !a.domain(&cfg, domain, "delete") {
            return Ok(());
        }
        let modes = usize::from(id.is_some())
            + usize::from(options.records.is_some())
            + usize::from(options.all_except.is_some());
        if modes == 0 {
            return Err(message(if a.ctx.globals.json {
                "Must provide a record ID, --records, or --all-except."
            } else {
                "must provide a record ID, --records, or --all-except."
            }));
        }
        if modes > 1 {
            return Err(message(if a.ctx.globals.json {
                "Cannot combine a record ID with --records or --all-except. Use only one mode."
            } else {
                "cannot combine a record ID with --records or --all-except. Use only one mode."
            }));
        }
        let keep_mode = options.all_except.is_some();
        let ids = if let Some(id) = id {
            vec![id.to_owned()]
        } else {
            let values = records::split_csv(
                options
                    .all_except
                    .as_deref()
                    .or(options.records.as_deref())
                    .unwrap(),
            );
            if values.is_empty() {
                return Err(message(if keep_mode {
                    "--all-except requires at least one ID to keep."
                } else {
                    "--records requires at least one ID."
                }));
            }
            values
        };
        let path = a.path(domain)?;
        store::with_file_lock(&path, || {
            let current =
                store::read_records(&path, &a.registry, a.ctx.globals.allow_unknown_types)?;
            let mut selected = std::collections::HashSet::new();
            for id in ids {
                selected.insert(records::resolve_record_id(&current, &id)?);
            }
            let mut deleted = Vec::new();
            let mut kept = Vec::new();
            let mut removed = Vec::new();
            let mut retained = Vec::new();
            for (index, r) in current.iter().enumerate() {
                let remove = if keep_mode {
                    !selected.contains(&index)
                } else {
                    selected.contains(&index)
                };
                if remove {
                    removed.push(index);
                    deleted.push(r.clone());
                } else {
                    retained.push(r);
                    kept.push(r.clone());
                }
            }
            if !options.dry_run {
                records::check_lossless_rewrite(&path, &a.registry, &removed)?;
                records::check_retained_ids(&path, retained.iter().copied().zip(kept.iter()))?;
                records::check_retained_numbers(&path, retained.iter().copied().zip(kept.iter()))?;
                records::assign_record_ids(&mut kept, &a.registry)?;
                store::write_records(&path, &mut kept, &a.registry)?;
            }
            if a.ctx.globals.json {
                if id.is_some() && !options.dry_run {
                    let mut data = info(&deleted[0], &a.registry);
                    data.insert_unordered("domain", domain);
                    a.json("delete", data);
                } else {
                    let mut data = Object::new();
                    data.insert("domain", domain);
                    data.insert("dryRun", options.dry_run);
                    data.insert(
                        "deleted",
                        Value::Array(
                            deleted
                                .iter()
                                .map(|r| Value::Object(info(r, &a.registry)))
                                .collect(),
                        ),
                    );
                    data.insert("kept", kept.len() as i64);
                    a.json("delete", data);
                }
            } else {
                for r in &deleted {
                    let mut prefix = String::new();
                    let mut verb = "Deleted";
                    if options.dry_run {
                        prefix = format!("{} ", a.styled("yellow", "[DRY RUN]"));
                        verb = "Would delete";
                    }
                    let rid = text(r.get("id"));
                    let rid = if rid.is_empty() {
                        String::new()
                    } else {
                        format!(" {}", a.styled("accent", &rid))
                    };
                    let mark = if id.is_some() && !options.dry_run {
                        format!("{} ", a.styled("brand", "✓"))
                    } else {
                        String::new()
                    };
                    a.human(format!(
                        "{mark}{prefix}{}{rid} {}: {}",
                        a.styled("brand", &format!("{verb} {}", text(r.get("type")))),
                        a.styled("brand", &format!("from {domain}")),
                        records::record_summary(r, &a.registry)
                    ));
                }
                if id.is_none() && !options.dry_run && deleted.len() > 1 {
                    a.human(a.styled(
                        "brand",
                        &format!("✓ Deleted {} records from {domain}", deleted.len()),
                    ));
                }
            }
            Ok(())
        })
    })
}
