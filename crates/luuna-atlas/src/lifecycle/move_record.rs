//! Domain moves with retained-row preflight before target append.
use super::{
    archive::{info, preflight_live},
    health::object,
};
use crate::{
    actions::record::{boolean, domain_names, id_text, message, payload, strings, text, Action},
    hooks,
    json::{Object, Value},
    records, schema, store, Result,
};
fn kept(a: &Action<'_>, path: &std::path::Path, rows: &[Object], id: &str) -> Result<Vec<Object>> {
    let removed = rows
        .iter()
        .enumerate()
        .filter(|(_, r)| text(r.get("id")) == id)
        .map(|(i, _)| i)
        .collect::<Vec<_>>();
    let mut kept = rows
        .iter()
        .enumerate()
        .filter(|(i, _)| !removed.contains(i))
        .map(|(_, r)| r.clone())
        .collect::<Vec<_>>();
    preflight_live(a, path, rows, &mut kept, &removed)?;
    Ok(kept)
}
pub(crate) fn run(a: &mut Action<'_>, args: &[String], flags: &[(String, String)]) -> Result<()> {
    let (source, id, target) = (&args[0], &args[1], &args[2]);
    let cfg = a.config()?;
    if source == target {
        return Err(message(
            "Source and target domain are the same — nothing to move.",
        ));
    }
    for domain in [source, target] {
        if !records::obj(cfg.get("domains")).contains_key(domain) {
            let names = domain_names(&cfg).join(", ");
            return Err(message(format!(
                "Domain {} not found in config. Available domains: {}",
                records::go_quote(domain),
                if names.is_empty() { "(none)" } else { &names }
            )));
        }
    }
    let source_path = a.path(source)?;
    let target_path = a.path(target)?;
    let rows = store::with_file_lock(&source_path, || {
        store::read_records(&source_path, &a.registry, a.ctx.globals.allow_unknown_types)
    })?;
    let index = records::resolve_record_id(&rows, id)?;
    let mut r = rows[index].clone();
    let rid = id_text(&r, id);
    let tick = char::from(96);
    if text(r.get("status")) == "archived" {
        return Err(message(format!(
            "Record {rid} is archived. Run {tick}luuna-atlas restore {rid}{tick} first."
        )));
    }
    let domain_cfg = records::obj(records::obj(cfg.get("domains")).get(target));
    let allowed = records::list(domain_cfg.get("allowed_types"));
    let typ = text(r.get("type"));
    if !allowed.is_empty() && !allowed.contains(&typ) && !boolean(flags, "force") {
        return Err(message(format!("Type {} is not in target domain {} allowed_types ({}). Pass --force to override, or adjust config.yaml.",records::go_quote(&typ),records::go_quote(target),allowed.join(", "))));
    }
    let missing =
        schema::find_missing_domain_fields(&r, &records::list(domain_cfg.get("required_fields")));
    if !missing.is_empty() {
        return Err(message(format!("Record is missing field(s) required by target domain {}: {}. Edit the record ({tick}luuna-atlas edit {rid}{tick}) before moving.",records::go_quote(target),missing.iter().map(|v|records::go_quote(v)).collect::<Vec<_>>().join(", "))));
    }
    let issues = schema::validate_record(&r, &a.registry)?;
    if !issues.is_empty() {
        return Err(message(format!(
            "Record fails schema validation: {}. Edit the record before moving.",
            schema::issues_text(&issues)
        )));
    }
    let mut incoming = Vec::new();
    if let Ok(entries) = std::fs::read_dir(source_path.parent().unwrap()) {
        let mut entries = entries.filter_map(|e| e.ok()).collect::<Vec<_>>();
        entries.sort_by_key(|e| e.file_name());
        for entry in entries {
            let name = entry.file_name().to_string_lossy().into_owned();
            let p = entry.path();
            if !name.ends_with(".jsonl") || p == source_path || p == target_path {
                continue;
            }
            let Ok(others) = store::read_records(&p, &a.registry, true) else {
                continue;
            };
            for other in others {
                for field in ["relates_to", "supersedes"] {
                    if records::list(other.get(field)).contains(&text(r.get("id"))) {
                        incoming.push(Value::Object(object([
                            ("domain", name.trim_end_matches(".jsonl").into()),
                            ("id", other.get("id").cloned().unwrap_or(Value::Null)),
                            ("field", field.into()),
                        ])));
                    }
                }
            }
        }
    }
    if boolean(flags, "dry-run") {
        if a.ctx.globals.json {
            a.json(
                "move",
                object([
                    ("dryRun", true.into()),
                    ("sourceDomain", source.clone().into()),
                    ("targetDomain", target.clone().into()),
                    ("record", info(&r, a).into()),
                    ("incomingReferences", Value::Array(incoming.clone())),
                ]),
            );
        } else {
            a.human(format!(
                "{} {} {} {}: {}",
                a.styled("yellow", "[DRY RUN]"),
                a.styled("brand", &format!("Would move {typ}")),
                a.styled("accent", &rid),
                a.styled("brand", &format!("from {source} → {target}")),
                records::record_summary(&r, &a.registry)
            ));
            if !incoming.is_empty() {
                a.human(a.styled("dim",&format!("  {} inbound reference(s) detected; ID is preserved so links remain valid.",incoming.len())));
            }
        }
        return Ok(());
    }
    let pre = hooks::run_hooks(
        a.ctx,
        Some(&cfg),
        "pre-record",
        &payload(target, &r, None),
        &mut a.output.stderr,
    )?;
    if pre.blocked {
        return Err(message(if pre.block_reason.is_empty() {
            "pre-record hook blocked the move".into()
        } else {
            pre.block_reason
        }));
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
                return Err(message(format!(
                    "pre-record hook produced an invalid record: {}",
                    schema::issues_text(&issues)
                )));
            }
            r = mutated.clone();
        }
    }
    let moved_id = text(r.get("id"));
    if rows
        .iter()
        .filter(|row| text(row.get("id")) == moved_id)
        .count()
        > 1
    {
        return Err(message("Refusing to move: duplicate source ids would remove additional stored records after appending only one target record. Files are unchanged."));
    }
    // A hook may change the ID. Go then removes every matching source row,
    // including unrelated rows. Refuse that loss before the target append.
    if moved_id != text(rows[index].get("id"))
        && rows.iter().any(|row| text(row.get("id")) == moved_id)
    {
        return Err(message("Refusing to move: the hook changed the id to another source record; unrelated stored data would be removed. Files are unchanged."));
    }
    records::check_lossless_rewrite(&source_path, &a.registry, &[])?;
    records::check_retained_numbers(&source_path, [(&rows[index], &r)])?;
    kept(a, &source_path, &rows, &moved_id)?;
    store::with_file_lock(&target_path, || {
        store::append_record(&target_path, &mut r, &a.registry)?;
        store::with_file_lock(&source_path, || {
            let current =
                store::read_records(&source_path, &a.registry, a.ctx.globals.allow_unknown_types)?;
            let mut retained = kept(a, &source_path, &current, &text(r.get("id")))?;
            store::write_records(&source_path, &mut retained, &a.registry)
        })
    })?;
    let post = hooks::run_hooks(
        a.ctx,
        Some(&cfg),
        "post-record",
        &payload(target, &r, Some("created")),
        &mut a.output.stderr,
    )?;
    let mut warnings = pre.warnings;
    warnings.extend(post.warnings);
    if a.ctx.globals.json {
        let mut data = object([
            ("sourceDomain", source.clone().into()),
            ("targetDomain", target.clone().into()),
            ("record", info(&r, a).into()),
            ("incomingReferences", Value::Array(incoming.clone())),
        ]);
        if !warnings.is_empty() {
            data.insert_unordered("warnings", strings(warnings));
        }
        a.json("move", data);
    } else {
        a.human(format!(
            "{} {} {} {}: {}",
            a.styled("brand", "✓"),
            a.styled("brand", &format!("Moved {}", text(r.get("type")))),
            a.styled("accent", &id_text(&r, "")),
            a.styled("brand", &format!("from {source} → {target}")),
            records::record_summary(&r, &a.registry)
        ));
        if !incoming.is_empty() {
            a.human(a.styled("yellow",&format!("  {} inbound reference(s) found; ID preserved so existing links still resolve:",incoming.len())));
            for row in incoming {
                let row = row.as_object().unwrap();
                let id = text(row.get("id"));
                let id = if id.is_empty() { "(no id)" } else { &id };
                a.human(a.styled(
                    "dim",
                    &format!(
                        "    {}/{id} via {}",
                        text(row.get("domain")),
                        text(row.get("field"))
                    ),
                ));
            }
        }
        for w in warnings {
            a.human(a.styled("yellow", &format!("  warning: {w}")));
        }
    }
    Ok(())
}
