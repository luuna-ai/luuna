//! Compaction analysis, automatic merges and explicit replacements.
use super::{
    archive,
    delete_domain::confirm,
    health::{date, is_record_stale, object},
    Options,
};
use crate::{
    actions::record::{boolean, domain_names, get, has, message, strings, text, Action},
    config::Config,
    hooks,
    json::{Object, Value},
    records,
    registry::Registry,
    schema, store, timefmt, Result,
};
use chrono::Utc;
#[derive(Clone)]
struct CandidateRecord {
    id: Option<String>,
    summary: String,
    recorded_at: String,
}
impl CandidateRecord {
    fn value(&self) -> Value {
        let mut o = Object::new();
        if let Some(id) = &self.id {
            o.insert("id", id.clone());
        }
        o.insert("summary", self.summary.clone());
        o.insert("recorded_at", self.recorded_at.clone());
        o.into()
    }
}
#[derive(Clone)]
struct Candidate {
    domain: String,
    typ: String,
    records: Vec<CandidateRecord>,
}
impl Candidate {
    fn value(&self) -> Value {
        let mut o = Object::new();
        o.insert("domain", self.domain.clone());
        o.insert("type", self.typ.clone());
        o.insert(
            "records",
            Value::Array(self.records.iter().map(CandidateRecord::value).collect()),
        );
        o.into()
    }
}
fn domains(domain: &str, cfg: &Config) -> Result<Vec<String>> {
    if domain.is_empty() {
        return Ok(domain_names(cfg));
    }
    if !records::obj(cfg.get("domains")).contains_key(domain) {
        return Err(message(format!(
            "Domain {} not found in config.",
            records::go_quote(domain)
        )));
    }
    Ok(vec![domain.into()])
}
fn collect(a: &Action<'_>, domain: &str, cfg: &Config, min: usize) -> Result<Vec<Candidate>> {
    let shelf = records::obj(records::obj(cfg.get("classification_defaults")).get("shelf_life"));
    let now = Utc::now();
    let mut out = Vec::new();
    for d in domains(domain, cfg)? {
        let rows =
            store::read_records(&a.path(&d)?, &a.registry, a.ctx.globals.allow_unknown_types)?;
        let mut groups: Vec<(String, Vec<&Object>)> = Vec::new();
        for r in &rows {
            let typ = text(r.get("type"));
            let mut key = typ.clone();
            if let Some(def) = a.registry.get(&typ) {
                if def.compact == "merge_outcomes" {
                    key.push_str("::");
                    key.push_str(&text(r.get(&def.id_key)));
                }
            }
            if let Some((_, rows)) = groups.iter_mut().find(|(k, _)| k == &key) {
                rows.push(r);
            } else {
                groups.push((key, vec![r]));
            }
        }
        for (_, group) in groups {
            if group.len() < 2
                || (!group.iter().any(|r| is_record_stale(r, now, shelf)) && group.len() < min)
            {
                continue;
            }
            out.push(Candidate {
                domain: d.clone(),
                typ: text(group[0].get("type")),
                records: group
                    .into_iter()
                    .map(|r| CandidateRecord {
                        id: r.get("id").and_then(Value::as_str).map(str::to_owned),
                        summary: records::record_summary(r, &a.registry),
                        recorded_at: text(r.get("recorded_at")),
                    })
                    .collect(),
            });
        }
    }
    Ok(out)
}
fn positive(raw: &str, default: usize, flag: &str) -> Result<usize> {
    if raw.is_empty() {
        return Ok(default);
    }
    let value = raw
        .parse::<u64>()
        .ok()
        .filter(|n| *n > 0 && *n < (1 << 53) && raw.bytes().all(|b| b.is_ascii_digit()));
    value.map(|n| n as usize).ok_or_else(|| {
        message(format!(
            "--{flag} must be a positive integer (got {}).",
            records::go_quote(raw)
        ))
    })
}
fn source_ids(rows: &[Object]) -> Vec<String> {
    rows.iter()
        .map(|r| text(r.get("id")))
        .filter(|s| !s.is_empty())
        .collect()
}
fn drop_id(r: &mut Object) {
    let keys = r
        .keys()
        .filter(|k| *k != "id")
        .map(str::to_owned)
        .collect::<Vec<_>>();
    r.remove("id");
    r.remember_order(keys);
}
pub fn merge_records(rows: &[Object], registry: &Registry) -> Result<Object> {
    let Some(first) = rows.first() else {
        return Err(message("Cannot merge empty record list"));
    };
    let typ = text(first.get("type"));
    let def = registry
        .get(&typ)
        .ok_or_else(|| message(format!("Unknown record type: {typ}")))?;
    let tick = char::from(96);
    if def.compact == "manual" {
        return Err(message(format!("Type {} has compact strategy \"manual\" — use {tick}luuna-atlas compact --apply{tick} to compact manually.",records::go_quote(&def.name))));
    }
    let tags = records::unique(
        &rows
            .iter()
            .flat_map(|r| records::list(r.get("tags")))
            .collect::<Vec<_>>(),
    );
    let files = records::unique(
        &rows
            .iter()
            .flat_map(|r| {
                if def.extracts_files {
                    records::list(r.get(&def.files_field))
                } else {
                    Vec::new()
                }
            })
            .collect::<Vec<_>>(),
    );
    let mut result = match def.compact.as_str() {
        "concat" => {
            let mut r = Object::new();
            r.insert("type", def.name.clone());
            r.insert("classification", "foundational");
            r.insert("recorded_at", timefmt::now_iso());
            r.insert("supersedes", strings(source_ids(rows)));
            if !tags.is_empty() {
                r.insert("tags", strings(tags));
            }
            if def.extracts_files && !files.is_empty() {
                r.insert(&def.files_field, strings(files));
            }
            for field in &def.required {
                let values = rows
                    .iter()
                    .filter_map(|r| r.get(field).and_then(Value::as_str))
                    .collect::<Vec<_>>();
                let value = if field == "name" || field == "title" {
                    let mut longest = "";
                    for v in values {
                        if v.encode_utf16().count() > longest.encode_utf16().count() {
                            longest = v;
                        }
                    }
                    longest.to_owned()
                } else {
                    values.join("\n\n")
                };
                r.insert(field, value);
            }
            r
        }
        "keep_latest" | "merge_outcomes" => {
            let mut sorted = rows.iter().collect::<Vec<_>>();
            sorted.sort_by_key(|r| std::cmp::Reverse(date(&text(r.get("recorded_at")))));
            let mut r = sorted[0].clone();
            let mut keys = r.keys().map(str::to_owned).collect::<Vec<_>>();
            r.insert_unordered("classification", "foundational");
            r.insert_unordered("recorded_at", timefmt::now_iso());
            r.insert_unordered("supersedes", strings(source_ids(rows)));
            if tags.is_empty() {
                r.remove("tags");
            } else {
                r.insert_unordered("tags", strings(tags));
            }
            if def.extracts_files && !files.is_empty() {
                r.insert_unordered(&def.files_field, strings(files));
            }
            keys.extend([
                "classification".into(),
                "recorded_at".into(),
                "supersedes".into(),
                "tags".into(),
                def.files_field.clone(),
            ]);
            r.remember_order(keys);
            drop_id(&mut r);
            if def.compact == "merge_outcomes" {
                let all = rows
                    .iter()
                    .flat_map(|r| records::record_list(r.get("outcomes")))
                    .cloned()
                    .map(Value::Object)
                    .collect::<Vec<_>>();
                if !all.is_empty() {
                    r.insert_unordered("outcomes", Value::Array(all));
                }
            }
            r
        }
        _ => {
            return Err(message(format!(
                "Unknown compact strategy: {}",
                def.compact
            )))
        }
    };
    let keys = result.keys().map(str::to_owned).collect::<Vec<_>>();
    result.remember_order(keys);
    let id = records::generate_record_id(&result, registry)?;
    result.insert_unordered("id", id);
    Ok(result)
}
fn hook_replacement(
    a: &mut Action<'_>,
    cfg: &Config,
    domain: &str,
    typ: &str,
    sources: &[Object],
    warnings: bool,
) -> Result<Option<Object>> {
    let mut payload = Object::new();
    payload.insert("domain", domain);
    payload.insert("type", typ);
    payload.insert(
        "records",
        Value::Array(sources.iter().cloned().map(Value::Object).collect()),
    );
    let hook = hooks::run_hooks(
        a.ctx,
        Some(cfg),
        "pre-compact",
        &payload.into(),
        &mut a.output.stderr,
    )?;
    if hook.blocked {
        return Err(message(if hook.block_reason.is_empty() {
            "pre-compact hook blocked the compaction".into()
        } else {
            hook.block_reason
        }));
    }
    if warnings && !a.ctx.globals.json {
        for warning in hook.warnings {
            a.warning(&warning);
        }
    }
    if hook.ran_any {
        if let Some(raw) = hook
            .payload
            .as_object()
            .and_then(|p| p.get("replacement"))
            .and_then(Value::as_object)
        {
            let mut candidate = raw.clone();
            if records::list(candidate.get("supersedes")).is_empty() {
                candidate.insert_unordered("supersedes", strings(source_ids(sources)));
            }
            drop_id(&mut candidate);
            let issues = schema::validate_record(&candidate, &a.registry)?;
            if !issues.is_empty() {
                return Err(message(format!(
                    "pre-compact hook produced an invalid record: {}",
                    schema::issues_text(&issues)
                )));
            }
            let id = records::generate_record_id(&candidate, &a.registry)?;
            candidate.insert_unordered("id", id);
            return Ok(Some(candidate));
        }
    }
    Ok(None)
}
fn preflight(
    a: &Action<'_>,
    domain: &str,
    before: &[Object],
    kept: &mut [Object],
    removed: &[usize],
    sources: &[Object],
) -> Result<()> {
    let path = a.path(domain)?;
    archive::preflight_live(a, &path, before, kept, removed)?;
    // Source records remain stored in the archive, including fields discarded
    // by the selected replacement strategy.
    records::check_lossless_rewrite(&path, &a.registry, &[])?;
    records::check_retained_numbers(&path, sources.iter().map(|r| (r, r)))?;
    let path = archive::path(a, domain)?;
    let existing = store::read_archive(&path, &a.registry)?;
    archive::preflight_archive(a, &path, &existing, &[])
}
fn append_archive(a: &Action<'_>, domain: &str, sources: &[Object]) -> Result<()> {
    if sources.is_empty() {
        return Ok(());
    }
    let path = archive::path(a, domain)?;
    std::fs::create_dir_all(path.parent().unwrap()).map_err(|source| {
        crate::AtlasError::PathOp {
            op: "mkdir",
            path: path.parent().unwrap().into(),
            source,
        }
    })?;
    store::with_file_lock(&path, || {
        let mut rows = store::read_archive(&path, &a.registry)?;
        archive::preflight_archive(a, &path, &rows, &[])?;
        rows.extend(archive::stamp(sources, "compacted"));
        store::write_archive(&path, &rows)
    })
}
pub(crate) fn run(a: &mut Action<'_>, args: &[String], options: &Options) -> Result<()> {
    let domain = args.first().map(String::as_str).unwrap_or_default();
    let flags = &options.flags;
    if boolean(flags, "analyze") {
        return analyze(a, domain);
    }
    if boolean(flags, "auto") {
        return auto(a, domain, options);
    }
    if boolean(flags, "apply") {
        if domain.is_empty() {
            return Err(message("Domain is required for --apply."));
        }
        return apply(a, domain, flags);
    }
    Err(message("Specify --analyze, --auto, or --apply."))
}
fn analyze(a: &mut Action<'_>, domain: &str) -> Result<()> {
    let cfg = a.config()?;
    let candidates = collect(a, domain, &cfg, 3)?;
    if a.ctx.globals.json {
        a.json(
            "compact",
            object([
                ("action", "analyze".into()),
                (
                    "candidates",
                    Value::Array(candidates.iter().map(Candidate::value).collect()),
                ),
            ]),
        );
        return Ok(());
    }
    if candidates.is_empty() {
        a.human(a.styled("brand", "No compaction candidates found."));
        return Ok(());
    }
    let total = candidates.iter().map(|c| c.records.len()).sum::<usize>();
    a.output.stdout.push_str(&format!(
        "{}\n{}\n",
        a.styled("bold", "\nCompaction candidates:\n"),
        a.styled(
            "dim",
            &format!(
                "Found {} groups ({total} records that could be compacted)\n",
                candidates.len()
            )
        )
    ));
    let mut last = "";
    for c in &candidates {
        if c.domain != last {
            if !last.is_empty() {
                a.output.stdout.push('\n');
            }
            a.output.stdout.push_str(&format!(
                "{}\n",
                a.styled("bold", &format!("{}:", c.domain))
            ));
            last = &c.domain;
        }
        a.output.stdout.push_str(&format!(
            "  {} ({} records)\n",
            a.styled("cyan", &c.typ),
            c.records.len()
        ));
        print_rows(a, c, "    ");
    }
    a.output.stdout.push('\n');
    for line in [
        "To compact manually:",
        "  luuna-atlas compact <domain> --apply --records <ids> --type <type> [fields...]",
        "\nTo compact automatically:",
        "  luuna-atlas compact --auto [--dry-run]",
    ] {
        a.output
            .stdout
            .push_str(&format!("{}\n", a.styled("dim", line)));
    }
    Ok(())
}
fn print_rows(a: &mut Action<'_>, c: &Candidate, prefix: &str) {
    for r in c.records.iter().take(3) {
        let id =
            r.id.as_ref()
                .map(|id| a.styled("accent", id))
                .unwrap_or_else(|| a.styled("dim", "(no id)"));
        a.output
            .stdout
            .push_str(&format!("{prefix}{id}: {}\n", r.summary));
    }
    if c.records.len() > 3 {
        a.output.stdout.push_str(&format!(
            "{}\n",
            a.styled(
                "dim",
                &format!("{prefix}... and {} more", c.records.len() - 3)
            )
        ));
    }
}
fn summary(
    a: &mut Action<'_>,
    heading: &str,
    verb: &str,
    process: &[Candidate],
    all: &[Candidate],
) {
    let total = process.iter().map(|c| c.records.len()).sum::<usize>();
    a.output.stdout.push_str(&format!(
        "{}\n  {} groups {verb} compacted\n  {total} records → {} records\n\n",
        a.styled("bold", &format!("\n{heading}:\n")),
        process.len(),
        process.len()
    ));
    for c in process {
        a.output.stdout.push_str(&format!(
            "{} ({} records)\n",
            a.styled("cyan", &format!("{}/{}", c.domain, c.typ)),
            c.records.len()
        ));
        print_rows(a, c, "  ");
        a.output.stdout.push('\n');
    }
    if all.len() > process.len() {
        a.output.stdout.push_str(&format!(
            "{}\n",
            a.styled(
                "yellow",
                &format!(
                    "Note: {} additional groups skipped due to --max-records limit\n",
                    all.len() - process.len()
                )
            )
        ));
    }
}
fn auto(a: &mut Action<'_>, domain: &str, options: &Options) -> Result<()> {
    let cfg = a.config()?;
    let flags = &options.flags;
    let min = positive(&get(flags, "min-group"), 5, "min-group")?;
    let max = positive(&get(flags, "max-records"), 50, "max-records")?;
    let all = collect(a, domain, &cfg, min)?;
    let dry = boolean(flags, "dry-run");
    let action = if dry { "dry-run" } else { "auto" };
    if all.is_empty() {
        if a.ctx.globals.json {
            a.json(
                "compact",
                object([
                    ("action", action.into()),
                    ("compacted", 0_i64.into()),
                    ("results", Value::Array(Vec::new())),
                ]),
            );
        } else {
            a.human(a.styled("brand", "No compaction candidates found."));
        }
        return Ok(());
    }
    let mut process = Vec::new();
    let mut total = 0;
    for c in &all {
        if total + c.records.len() > max {
            break;
        }
        total += c.records.len();
        process.push(c.clone());
    }
    if !a.ctx.globals.json && !dry {
        summary(a, "Compaction summary", "will be", &process, &all);
    }
    if dry {
        if a.ctx.globals.json {
            let groups = process
                .iter()
                .map(|c| {
                    let mut o = Object::new();
                    o.insert("domain", c.domain.clone());
                    o.insert("type", c.typ.clone());
                    o.insert("count", c.records.len() as i64);
                    o.insert(
                        "records",
                        Value::Array(c.records.iter().map(CandidateRecord::value).collect()),
                    );
                    Value::Object(o)
                })
                .collect();
            a.json(
                "compact",
                object([
                    ("action", "dry-run".into()),
                    ("wouldCompact", (total as i64).into()),
                    ("groups", Value::Array(groups)),
                ]),
            );
        } else {
            summary(a, "Dry-run preview", "would be", &process, &all);
            a.success(&format!(
                "Dry-run complete. Would compact {total} records across {} groups.",
                process.len()
            ));
            a.human(a.styled("dim", "  Run without --dry-run to apply changes."));
        }
        return Ok(());
    }
    if !a.ctx.globals.json
        && !boolean(flags, "yes")
        && !confirm(a, "Proceed with compaction?", &options.input)
    {
        a.output.stdout.push_str(&format!(
            "{}\n",
            a.styled("yellow", "Compaction cancelled.")
        ));
        return Ok(());
    }
    let mut domain_order = Vec::new();
    for c in &process {
        if !domain_order.contains(&c.domain) {
            domain_order.push(c.domain.clone());
        }
    }
    let mut total_compacted = 0;
    let mut results = Vec::new();
    let mut warnings = Vec::new();
    let mut prepared = Vec::new();
    for domain in domain_order {
        let path = a.path(&domain)?;
        let before = store::read_records(&path, &a.registry, a.ctx.globals.allow_unknown_types)?;
        let mut current = before.clone();
        let mut sources_all = Vec::new();
        for candidate in process.iter().filter(|c| c.domain == domain) {
            let sources = current
                .iter()
                .filter(|r| {
                    text(r.get("type")) == candidate.typ
                        && candidate.records.iter().any(|cr| match &cr.id {
                            None => r.get("id").is_none_or(Value::is_null),
                            Some(id) => *id == text(r.get("id")),
                        })
                })
                .cloned()
                .collect::<Vec<_>>();
            if sources.len() < 2 {
                continue;
            }
            let replacement =
                match hook_replacement(a, &cfg, &domain, &candidate.typ, &sources, false).and_then(
                    |r| {
                        r.map(Ok)
                            .unwrap_or_else(|| merge_records(&sources, &a.registry))
                    },
                ) {
                    Ok(r) => r,
                    Err(error) => {
                        warnings.push(format!("{domain}/{}: {error}", candidate.typ));
                        continue;
                    }
                };
            let remove = sources
                .iter()
                .map(|r| text(r.get("id")))
                .collect::<Vec<_>>();
            current.retain(|r| !remove.contains(&text(r.get("id"))));
            current.push(replacement);
            total_compacted += sources.len();
            let mut result = Object::new();
            result.insert("domain", domain.clone());
            result.insert("type", candidate.typ.clone());
            result.insert("count", sources.len() as i64);
            results.push(result);
            sources_all.extend(sources);
        }
        if current.len() != before.len() {
            let removed = before
                .iter()
                .enumerate()
                .filter(|(_, r)| {
                    sources_all
                        .iter()
                        .any(|s| text(s.get("id")) == text(r.get("id")))
                })
                .map(|(i, _)| i)
                .collect::<Vec<_>>();
            let retained_count = before.len() - removed.len();
            preflight(
                a,
                &domain,
                &before,
                &mut current[..retained_count],
                &removed,
                &sources_all,
            )?;
            records::assign_record_ids(&mut current, &a.registry)?;
        }
        prepared.push((domain, before.len(), current, sources_all));
    }
    for (domain, old_len, mut current, sources) in prepared {
        if current.len() != old_len {
            let path = a.path(&domain)?;
            store::with_file_lock(&path, || {
                store::write_records(&path, &mut current, &a.registry)
            })?;
        }
        append_archive(a, &domain, &sources)?;
    }
    if a.ctx.globals.json {
        let mut data = object([
            ("action", "auto".into()),
            ("compacted", (total_compacted as i64).into()),
            (
                "results",
                Value::Array(results.into_iter().map(Value::Object).collect()),
            ),
        ]);
        if !warnings.is_empty() {
            data.insert_unordered("warnings", strings(warnings));
        }
        a.json("compact", data);
    } else {
        a.human(format!(
            "\n{} {}",
            a.styled("brand", "✓"),
            a.styled(
                "brand",
                &format!(
                    "Auto-compacted {total_compacted} records across {} groups",
                    results.len()
                )
            )
        ));
        for r in results {
            a.human(a.styled(
                "dim",
                &format!(
                    "  {}/{}: {} records → 1",
                    text(r.get("domain")),
                    text(r.get("type")),
                    text(r.get("count"))
                ),
            ));
        }
        if !a.ctx.globals.quiet {
            for w in warnings {
                a.warning(&w);
            }
        }
    }
    Ok(())
}
fn apply(a: &mut Action<'_>, domain: &str, flags: &[(String, String)]) -> Result<()> {
    let cfg = a.config()?;
    domains(domain, &cfg)?;
    if !has(flags, "records") {
        return Err(message("--records is required for --apply."));
    }
    let path = a.path(domain)?;
    let mut to_archive = Vec::new();
    store::with_file_lock(&path, || {
        let before = store::read_records(&path, &a.registry, a.ctx.globals.allow_unknown_types)?;
        let mut indices = Vec::new();
        for id in records::split_csv(&get(flags, "records")) {
            indices.push(records::resolve_record_id(&before, &id)?);
        }
        if indices.len() < 2 {
            return Err(message("Compaction requires at least 2 records."));
        }
        let mut typ = get(flags, "type");
        if typ.is_empty() {
            typ = text(before[indices[0]].get("type"));
        }
        if typ.is_empty() {
            typ = "convention".into();
        }
        let sources = indices
            .iter()
            .map(|i| before[*i].clone())
            .collect::<Vec<_>>();
        let replacement = if let Some(r) = hook_replacement(a, &cfg, domain, &typ, &sources, true)?
        {
            r
        } else {
            let mut r = Object::new();
            r.insert("type", typ.clone());
            r.insert_unordered("classification", "foundational");
            r.insert_unordered("recorded_at", timefmt::now_iso());
            let mut order = vec!["type".to_owned()];
            if typ == "convention" {
                let content = if has(flags, "content") {
                    get(flags, "content")
                } else {
                    get(flags, "description")
                };
                if content.is_empty() {
                    return Err(message(
                        "Replacement convention requires --content or --description.",
                    ));
                }
                r.insert_unordered("content", content);
                order.push("content".into());
            } else {
                let fields: &[&str] = match typ.as_str() {
                    "pattern" | "reference" | "guide" => &["name", "description"],
                    "failure" => &["description", "resolution"],
                    "decision" => &["title", "rationale"],
                    _ => {
                        return Err(message(format!(
                            "Unknown record type {}.",
                            records::go_quote(&typ)
                        )))
                    }
                };
                let mut missing = false;
                for field in fields {
                    let value = get(flags, field);
                    if value.is_empty() {
                        missing = true;
                    }
                    r.insert_unordered(*field, value);
                    order.push((*field).into());
                }
                if missing {
                    return Err(message(format!(
                        "Replacement {typ} requires {}.",
                        fields
                            .iter()
                            .map(|f| format!("--{f}"))
                            .collect::<Vec<_>>()
                            .join(" and ")
                    )));
                }
            }
            let ids = source_ids(&sources);
            if !ids.is_empty() {
                r.insert_unordered("supersedes", strings(ids));
            }
            order.extend([
                "classification".into(),
                "recorded_at".into(),
                "supersedes".into(),
            ]);
            r.remember_order(order);
            let id = records::generate_record_id(&r, &a.registry)?;
            r.insert_unordered("id", id);
            let issues = schema::validate_record(&r, &a.registry)?;
            if !issues.is_empty() {
                return Err(message(format!(
                    "Replacement record failed validation: {}",
                    schema::issues_text(&issues)
                )));
            }
            r
        };
        let mut remaining = before
            .iter()
            .enumerate()
            .filter(|(i, _)| !indices.contains(i))
            .map(|(_, r)| r.clone())
            .collect::<Vec<_>>();
        preflight(a, domain, &before, &mut remaining, &indices, &sources)?;
        remaining.push(replacement.clone());
        records::assign_record_ids(&mut remaining, &a.registry)?;
        store::write_records(&path, &mut remaining, &a.registry)?;
        to_archive = sources;
        if a.ctx.globals.json {
            a.json(
                "compact",
                object([
                    ("action", "applied".into()),
                    ("domain", domain.into()),
                    ("removed", (indices.len() as i64).into()),
                    ("replacement", replacement.into()),
                ]),
            );
        } else {
            a.success(&format!(
                "Compacted {} {typ} records into 1 in {domain}",
                indices.len()
            ));
        }
        Ok(())
    })?;
    append_archive(a, domain, &to_archive)
}
