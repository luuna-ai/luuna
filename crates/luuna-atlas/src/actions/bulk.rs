//! Native stdin and batch recording, including Go's partial-success rules.
use super::record::{
    self, boolean, is_named, message, payload, strings, text, Action, RecordOptions,
};
use crate::options::Context;
use crate::{
    config, hooks,
    json::{self, Object, Value},
    records, schema, store, timefmt, AtlasError, CommandOutput, Result,
};
use std::{fs, path::PathBuf};

pub enum BulkInput {
    Stdin(Vec<u8>),
    Batch(PathBuf),
}
pub fn run_record_bulk(
    ctx: &Context,
    domain: &str,
    input: BulkInput,
    options: &RecordOptions,
) -> CommandOutput {
    record::run(ctx, "record", |a| {
        let (bytes, action) = match input {
            BulkInput::Stdin(bytes) => (bytes, "stdin"),
            BulkInput::Batch(path) => {
                let actual = if path.is_absolute() || path.as_os_str().is_empty() {
                    path.clone()
                } else {
                    ctx.cwd.join(&path)
                };
                let bytes = fs::read(&actual).map_err(|source| {
                    if source.kind() == std::io::ErrorKind::NotFound {
                        message(format!("Batch file not found: {}", path.display()))
                    } else {
                        AtlasError::PathOp {
                            op: "open",
                            path,
                            source,
                        }
                    }
                })?;
                (bytes, "batch")
            }
        };
        bulk(a, domain, &bytes, action, options)
    })
}
fn bulk(
    a: &mut Action<'_>,
    domain: &str,
    input: &[u8],
    action: &str,
    options: &RecordOptions,
) -> Result<()> {
    let flags = &options.flags;
    let dry = boolean(flags, "dry-run");
    let cfg = a.config()?;
    if !records::obj(cfg.get("domains")).contains_key(domain) {
        config::add_domain(&a.ctx.root.join(".luuna/atlas"), domain)?;
    }
    let mut parsed = json::parse_bytes(input).map_err(|_| {
        message(format!(
            "Failed to parse JSON from stdin: {}",
            store::json_parse_error(input)
        ))
    })?;
    let items = if let Value::Array(items) = &mut parsed {
        std::mem::take(items)
    } else {
        vec![parsed]
    };
    let mut valid = Vec::new();
    let mut errors = Vec::new();
    let mut warnings = Vec::new();
    let domain_cfg = records::obj(records::obj(cfg.get("domains")).get(domain));
    for (index, mut item) in items.into_iter().enumerate() {
        let Some(obj) = item.as_object_mut() else {
            errors.push(format!("Record {index}:  must be object"));
            continue;
        };
        let mut r = std::mem::take(obj);
        if !r.contains_key("recorded_at") {
            r.insert("recorded_at", timefmt::now_iso());
        }
        if !r.contains_key("classification") {
            r.insert("classification", "tactical");
        }
        let issues = schema::validate_record(&r, &a.registry)?;
        if !issues.is_empty() {
            let hint = if let Some(def) = a.registry.get(&text(r.get("type"))) {
                format!(
                    ". Hint: {} records require: {}",
                    def.name,
                    def.required.join(", ")
                )
            } else {
                String::new()
            };
            errors.push(format!(
                "Record {index}: {}{hint}",
                schema::issues_text(&issues)
            ));
            continue;
        }
        if !a.ctx.globals.allow_domain_mismatch {
            let allowed = records::list(domain_cfg.get("allowed_types"));
            let typ = text(r.get("type"));
            if !allowed.is_empty() && !allowed.contains(&typ) {
                errors.push(format!(
                    "Record {index}: type {} is not allowed in domain {}. Allowed types: {}.",
                    records::go_quote(&typ),
                    records::go_quote(domain),
                    allowed.join(", ")
                ));
                continue;
            }
            let missing = schema::find_missing_domain_fields(
                &r,
                &records::list(domain_cfg.get("required_fields")),
            );
            if !missing.is_empty() {
                let missing = missing
                    .iter()
                    .map(|v| json::to_compact_string(&Value::from(v.clone())).unwrap())
                    .collect::<Vec<_>>()
                    .join(", ");
                errors.push(format!(
                    "Record {index}: domain {} requires field(s) {missing}.",
                    records::go_quote(domain)
                ));
                continue;
            }
        }
        valid.push(r);
    }
    let mut seen_disabled = std::collections::HashSet::new();
    for r in &valid {
        let typ = text(r.get("type"));
        if a.registry.get(&typ).is_some_and(|d| d.disabled) && seen_disabled.insert(typ.clone()) {
            let warning=format!("type {} is disabled (declared in disabled_types). Records will still be written; consider migrating off this type.",records::go_quote(&typ));
            if !a.ctx.globals.quiet {
                a.warning(&warning);
            }
            warnings.push(warning);
        }
    }
    let mut to_write = Vec::new();
    for (index, mut r) in valid.into_iter().enumerate() {
        if !dry {
            let pre = hooks::run_hooks(
                a.ctx,
                Some(&cfg),
                "pre-record",
                &payload(domain, &r, None),
                &mut a.output.stderr,
            )?;
            warnings.extend(pre.warnings);
            if pre.blocked {
                errors.push(format!(
                    "Record {index}: {}",
                    if pre.block_reason.is_empty() {
                        "pre-record hook blocked the write".into()
                    } else {
                        pre.block_reason
                    }
                ));
                continue;
            }
            if pre.ran_any {
                if let Some(mutated) = pre
                    .payload
                    .as_object()
                    .and_then(|p| p.get("record"))
                    .and_then(Value::as_object)
                {
                    let issues = schema::validate_record(mutated, &a.registry)?;
                    if !issues.is_empty() {
                        errors.push(format!(
                            "Record {index}: pre-record hook produced an invalid record: {}",
                            schema::issues_text(&issues)
                        ));
                        continue;
                    }
                    r = mutated.clone();
                }
            }
        }
        to_write.push(r);
    }
    let (mut created, mut updated, mut skipped) = (0usize, 0usize, 0usize);
    let mut post_payloads = Vec::new();
    let path = a.path(domain)?;
    let mut process = || -> Result<()> {
        let mut current =
            store::read_records(&path, &a.registry, a.ctx.globals.allow_unknown_types)?;
        let before = current.clone();
        let mut generations = vec![0usize; current.len()];
        let mut generation = 0usize;
        let mut pending: Vec<(usize, usize, Object, &str)> = Vec::new();
        let mut replaced = Vec::new();
        for mut r in to_write.iter().cloned() {
            let d = records::find_duplicate(&current, &r, &a.registry);
            let named = a.registry.get(&text(r.get("type"))).is_some_and(is_named);
            if let Some(d) = d.filter(|_| !(boolean(flags, "force") && named)) {
                if named {
                    updated += 1;
                    if !dry {
                        records::check_outcomes_collection(&path, &current[d], true)?;
                        let merged = record::merge_outcomes(&current[d], &r);
                        generation += 1;
                        generations[d] = generation;
                        current[d] = merged.clone();
                        if d < before.len() && !replaced.contains(&d) {
                            replaced.push(d);
                        }
                        pending.push((d, generation, merged, "updated"));
                    }
                } else {
                    skipped += 1;
                }
            } else {
                created += 1;
                if !dry {
                    if text(r.get("id")).is_empty() {
                        r.insert_unordered("id", store::generate_record_id(&r, &a.registry)?);
                    }
                    generation += 1;
                    generations.push(generation);
                    pending.push((current.len(), generation, r.clone(), "created"));
                    current.push(r);
                }
            }
        }
        if !dry && (created > 0 || updated > 0) {
            records::check_lossless_rewrite(&path, &a.registry, &replaced)?;
            records::check_retained_ids(
                &path,
                before
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| !replaced.contains(i))
                    .map(|(i, r)| (r, &current[i])),
            )?;
            records::check_retained_numbers(&path, before.iter().zip(current.iter()))?;
            records::assign_record_ids(&mut current, &a.registry)?;
            store::write_records(&path, &mut current, &a.registry)?;
            // Go's payload maps share the live slot only until that slot is
            // replaced again. Preserve earlier snapshots and only propagate
            // WriteRecords' ID assignment into the still-live final generation.
            for (index, generation, snapshot, action) in pending {
                let r = if generations[index] == generation {
                    &current[index]
                } else {
                    &snapshot
                };
                post_payloads.push(payload(domain, r, Some(action)));
            }
        }
        Ok(())
    };
    if !to_write.is_empty() {
        if dry {
            process()?;
        } else {
            store::with_file_lock(&path, process)?;
        }
    }
    for payload in post_payloads {
        let post = hooks::run_hooks(
            a.ctx,
            Some(&cfg),
            "post-record",
            &payload,
            &mut a.output.stderr,
        )?;
        warnings.extend(post.warnings);
    }
    if !errors.is_empty() {
        if a.ctx.globals.json {
            let old = a.output.exit_code;
            a.error(
                "record",
                &format!("Validation errors: {}", errors.join("; ")),
            );
            a.output.exit_code = old;
        } else {
            a.output
                .stderr
                .push_str(&format!("{}\n", a.styled("red", "Validation errors:")));
            for error in &errors {
                a.output
                    .stderr
                    .push_str(&format!("{}\n", a.styled("red", &format!("  {error}"))));
            }
        }
    }
    if a.ctx.globals.json {
        let mut result = Object::new();
        result.insert("success", errors.is_empty() || created + updated > 0);
        result.insert("command", "record");
        result.insert("action", if dry { "dry-run" } else { action });
        result.insert("domain", domain);
        result.insert("created", created as i64);
        result.insert("updated", updated as i64);
        result.insert("skipped", skipped as i64);
        result.insert("errors", strings(errors.clone()));
        result.insert("warnings", strings(warnings));
        a.output
            .stdout
            .push_str(&CommandOutput::emit(&result).stdout);
    } else if dry {
        let total = created + updated;
        if total > 0 || skipped > 0 {
            a.success(&format!(
                "Dry-run complete. Would process {total} record(s) in {domain}:"
            ));
            for (name, count) in [("Create", created), ("Update", updated), ("Skip", skipped)] {
                if count > 0 {
                    a.human(a.styled("dim", &format!("  {name}: {count}")));
                }
            }
            a.human(a.styled("dim", "  Run without --dry-run to apply changes."));
        } else {
            a.human(a.styled("yellow", "No records would be processed."));
        }
    } else {
        if created > 0 {
            a.success(&format!("Created {created} record(s) in {domain}"));
        }
        if updated > 0 {
            a.success(&format!("Updated {updated} record(s) in {domain}"));
        }
        if skipped > 0 {
            a.human(a.styled(
                "yellow",
                &format!("Skipped {skipped} duplicate(s) in {domain}"),
            ));
        }
    }
    if !errors.is_empty() && created + updated == 0 {
        a.output.exit_code = 1;
    }
    Ok(())
}
