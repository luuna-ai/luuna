//! Prime selection, budget and output.
use super::{
    active::{self, ActiveContext},
    format::{self, ManifestDomain},
};
use crate::{
    actions::{
        record::{boolean, domain_names, get, has, many, message, text, Action},
        render::{self, FormatOptions},
    },
    config::Config,
    git, hooks,
    json::{self, Object, Value},
    lifecycle::{
        health::{date, go_int, hours, object, zero_time},
        retention::{collect_superseded_ids, find_stale_records, prime_stale_ids},
    },
    records,
    registry::Registry,
    store, timefmt, Result,
};
use chrono::{DateTime, Utc};
use std::collections::{BTreeMap, BTreeSet};
#[derive(Clone)]
pub struct LoadedDomain {
    pub domain: String,
    pub records: Vec<Object>,
    pub updated: Option<DateTime<Utc>>,
}
pub fn tokens(text: &str) -> usize {
    text.encode_utf16().count().div_ceil(4)
}
pub fn estimate(r: &Object, registry: &Registry) -> String {
    let typ = text(r.get("type"));
    if registry.get(&typ).is_none() {
        format!("[{typ}]")
    } else {
        render::format_record_index(r, registry, false)
    }
}
pub fn trust(r: &Object, cfg: &Config) -> f64 {
    let weights = records::obj(records::obj(cfg.get("prime")).get("tier_weights"));
    let class = text(r.get("classification"));
    let base = match class.as_str() {
        "foundational" => 50.0,
        "tactical" => 20.0,
        "observational" => 10.0,
        _ => 0.0,
    };
    records::confirmation_score(r) * records::number_default(weights.get("star"), 100.0)
        + records::number_default(weights.get(&class), base)
}
pub fn sort(rows: &[Object], cfg: &Config) -> Vec<Object> {
    let mut rows = rows.to_vec();
    super::go_sort::stable(&mut rows, |a, b| {
        let af = text(a.get("type")) == "failure";
        let bf = text(b.get("type")) == "failure";
        if af != bf {
            af
        } else {
            trust(a, cfg) > trust(b, cfg)
        }
    });
    rows
}
pub fn budget(
    loaded: &[LoadedDomain],
    limit: usize,
    registry: &Registry,
    files: Option<&[String]>,
    stale: &BTreeSet<String>,
) -> (Vec<LoadedDomain>, usize, usize) {
    struct Candidate {
        domain: usize,
        index: usize,
        key: [f64; 5],
        cost: usize,
        pinned: bool,
    }
    let mut all = Vec::new();
    for (di, d) in loaded.iter().enumerate() {
        for (ri, r) in d.records.iter().enumerate() {
            let failure = text(r.get("type")) == "failure";
            let anchored = files.is_some_and(|files| git::matches_file_anchors(r, files));
            let tier = match (anchored, failure) {
                (true, true) => 0,
                (true, false) => 1,
                (false, true) => 2,
                (false, false) => 3,
            };
            let class = match text(r.get("classification")).as_str() {
                "foundational" => 0.0,
                "tactical" => 1.0,
                "observational" => 2.0,
                _ => -1.0,
            };
            all.push(Candidate {
                domain: di,
                index: ri,
                key: [
                    tier as f64,
                    if stale.contains(&text(r.get("id"))) {
                        1.0
                    } else {
                        0.0
                    },
                    -records::confirmation_score(r),
                    class,
                    -(date(&text(r.get("recorded_at"))).timestamp_millis() as f64),
                ],
                cost: tokens(&estimate(r, registry)),
                pinned: tier == 0,
            });
        }
    }
    all.sort_by(|a, b| {
        for k in 0..5 {
            if a.key[k] != b.key[k] {
                return a.key[k]
                    .partial_cmp(&b.key[k])
                    .unwrap_or(std::cmp::Ordering::Equal);
            }
        }
        std::cmp::Ordering::Equal
    });
    let mut keep = BTreeSet::new();
    let mut used = 0;
    for c in all {
        if c.pinned || used + c.cost <= limit {
            keep.insert((c.domain, c.index));
            used += c.cost;
        }
    }
    let (mut out, mut dropped, mut dropped_domains) = (Vec::new(), 0, 0);
    for (di, d) in loaded.iter().enumerate() {
        let rows = d
            .records
            .iter()
            .enumerate()
            .filter(|(ri, _)| keep.contains(&(di, *ri)))
            .map(|(_, r)| r.clone())
            .collect::<Vec<_>>();
        let n = d.records.len() - rows.len();
        dropped += n;
        if n > 0 {
            dropped_domains += 1;
        }
        if !rows.is_empty() {
            out.push(LoadedDomain {
                domain: d.domain.clone(),
                records: rows,
                updated: d.updated,
            });
        }
    }
    (out, dropped, dropped_domains)
}
pub fn annotation(r: &Object, ctx: Option<&ActiveContext>, now: DateTime<Utc>) -> String {
    if let Some(ctx) = ctx {
        for f in records::list(r.get("files")) {
            if git::file_matches_any(&f, &ctx.changed_files) {
                return format!("why: file match ({f})");
            }
        }
        for d in records::list(r.get("dir_anchors")) {
            for f in &ctx.changed_files {
                if git::file_lives_under_dir(f, &d) {
                    return format!("why: file match ({d})");
                }
            }
        }
        for tracker in ["missions", "gh", "linear", "bead"] {
            if let Some(active) = ctx.trackers.get(tracker).filter(|v| !v.is_empty()) {
                if *active == text(records::obj(r.get("evidence")).get(tracker)) {
                    return format!("why: in-progress {tracker}:{active}");
                }
            }
        }
    }
    let score = records::confirmation_score(r);
    if score > 0.0 {
        return format!("why: ★{} confirmations", text(Some(&Value::Number(score))));
    }
    let at = date(&text(r.get("recorded_at")));
    if at != zero_time() {
        let days = go_int((hours(now, at) / 24.0).floor());
        if days < 7 {
            return if days <= 0 {
                "why: recorded today".into()
            } else {
                format!("why: recorded {days}d ago")
            };
        }
    }
    "why: applies broadly (no anchors)".into()
}
fn output(a: &mut Action<'_>, value: String, flags: &[(String, String)]) -> Result<()> {
    let export = get(flags, "export");
    if !export.is_empty() {
        let path = a.ctx.cwd.join(&export);
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&path)
            .map_err(|source| crate::AtlasError::PathOp {
                op: "open",
                path: path.clone(),
                source,
            })?;
        file.write_all(format!("{value}\n").as_bytes())
            .map_err(|source| crate::AtlasError::PathOp {
                op: "write",
                path,
                source,
            })?;
        if !a.ctx.globals.json {
            a.human(format!("✓ Exported to {export}"));
        }
    } else if !value.is_empty() {
        a.output.stdout.push_str(&value);
        a.output.stdout.push('\n');
    }
    Ok(())
}
fn pretty(value: Value) -> String {
    json::to_indented_string(&value).unwrap_or_default()
}
fn known(a: &mut Action<'_>, cfg: &Config, domain: &str, excluded: bool) -> bool {
    if records::obj(cfg.get("domains")).contains_key(domain) {
        return true;
    }
    let names = domain_names(cfg).join(", ");
    let names = if names.is_empty() && !excluded {
        "(none)"
    } else {
        &names
    };
    a.error(
        "prime",
        &format!(
            "{} {} not found in config. Available domains: {names}",
            if excluded {
                "Excluded domain"
            } else {
                "Domain"
            },
            records::go_quote(domain)
        ),
    );
    if !a.ctx.globals.json {
        let tick = char::from(96);
        a.output.stderr.push_str(&format!("Hint: Run {tick}luuna-atlas add {domain}{tick} to create this domain, or check .luuna/atlas/config.yaml\n"));
    }
    false
}
pub(crate) fn run(a: &mut Action<'_>, args: &[String], flags: &[(String, String)]) -> Result<()> {
    let cfg = a.config()?;
    let format = if !a.ctx.globals.format.is_empty() {
        a.ctx.globals.format.clone()
    } else if boolean(flags, "full") {
        "markdown".into()
    } else {
        "compact".into()
    };
    if boolean(flags, "manifest") && boolean(flags, "full") {
        return Err(message("Cannot combine --manifest with --full."));
    }
    if boolean(flags, "dry-run") && boolean(flags, "manifest") {
        return Err(message("Cannot combine --dry-run with --manifest. Manifest mode lists domains, not records; --dry-run previews which records would be primed."));
    }
    let requested = records::unique(
        &args
            .iter()
            .cloned()
            .chain(many(flags, "domain"))
            .collect::<Vec<_>>(),
    );
    let excluded = many(flags, "exclude-domain");
    let scoped = !requested.is_empty()
        || !excluded.is_empty()
        || boolean(flags, "context")
        || !many(flags, "files").is_empty();
    if boolean(flags, "manifest") && scoped {
        let tick = char::from(96);
        return Err(message(format!("--manifest cannot be combined with scoping arguments. Manifest mode lists available domains; use {tick}luuna-atlas prime <domain>{tick} or {tick}luuna-atlas prime --files <path>{tick} to load records.")));
    }
    for d in &requested {
        if !known(a, &cfg, d, false) {
            return Ok(());
        }
    }
    for d in &excluded {
        if !known(a, &cfg, d, true) {
            return Ok(());
        }
    }
    let domains = if requested.is_empty() {
        domain_names(&cfg)
    } else {
        requested
    };
    let target = domains
        .into_iter()
        .filter(|d| !excluded.contains(d))
        .collect::<Vec<_>>();
    let files = if boolean(flags, "context") {
        if !git::is_git_repo(&a.ctx.cwd, &a.ctx.env) {
            return Err(message("Not in a git repository. --context requires git."));
        }
        let files = git::get_changed_files(&a.ctx.cwd, &a.ctx.env, "HEAD~1");
        if files.is_empty() {
            if a.ctx.globals.json {
                a.error("prime", "No changed files found. Nothing to filter by.");
                a.output.exit_code = 0;
            } else {
                a.output
                    .stdout
                    .push_str("No changed files found. Nothing to filter by.\n");
            }
            return Ok(());
        }
        Some(files)
    } else {
        let files = many(flags, "files");
        if files.is_empty() {
            None
        } else {
            Some(files)
        }
    };
    let raw = get(flags, "budget");
    let limit = if raw.is_empty() {
        4000
    } else {
        raw.parse::<i64>()
            .ok()
            .filter(|n| *n > 0 && raw.bytes().all(|b| b.is_ascii_digit()))
            .map(|n| n as usize)
            .ok_or_else(|| {
                message(format!(
                    "--budget must be a positive integer (got {}).",
                    records::go_quote(&raw)
                ))
            })?
    };
    let budget_enabled = !a.ctx.globals.json && (!has(flags, "limit") || boolean(flags, "limit"));
    let mut mode = text(records::obj(cfg.get("prime")).get("default_mode"));
    if boolean(flags, "manifest") {
        mode = "manifest".into();
    }
    if boolean(flags, "full") {
        mode = "full".into();
    }
    let manifest = mode == "manifest"
        && !scoped
        && !boolean(flags, "dry-run")
        && !boolean(flags, "records-only");
    let mut loaded = Vec::new();
    let mut domain_records = Vec::new();
    for domain in target {
        let path = a.path(&domain)?;
        let rows = store::read_records(&path, &a.registry, a.ctx.globals.allow_unknown_types)?;
        domain_records.push((domain.clone(), rows.clone()));
        loaded.push(LoadedDomain {
            domain,
            records: rows,
            updated: timefmt::file_mod_time(&path),
        });
    }
    let (superseded, _) = collect_superseded_ids(&domain_records);
    for d in &mut loaded {
        d.records
            .retain(|r| !superseded.contains(&text(r.get("id"))));
    }
    let mut active = None;
    if !manifest
        && !scoped
        && !a.ctx.globals.json
        && !boolean(flags, "all")
        && files.is_none()
        && git::is_git_repo(&a.ctx.cwd, &a.ctx.env)
    {
        let work = active::resolve_active_work(a.ctx, &BTreeMap::new());
        let ctx = ActiveContext {
            changed_files: git::get_active_files(&a.ctx.cwd, &a.ctx.env),
            trackers: BTreeMap::from([
                ("missions".into(), work.missions),
                ("gh".into(), work.gh),
                ("linear".into(), work.linear),
                ("bead".into(), work.bead),
            ]),
        };
        if active::has_signal(&ctx) {
            active = Some(ctx);
        }
    }
    if let Some(files) = &files {
        for d in &mut loaded {
            d.records = git::filter_by_context(&d.records, files);
        }
        if !a.ctx.globals.json {
            loaded.retain(|d| !d.records.is_empty());
        }
    } else if let Some(active) = active.as_ref().filter(|_| !manifest) {
        let total = loaded.iter().map(|d| d.records.len()).sum::<usize>();
        for d in &mut loaded {
            d.records = active::filter_by_active_context(&d.records, active);
        }
        let kept = loaded.iter().map(|d| d.records.len()).sum::<usize>();
        let mut sources = Vec::new();
        if !active.changed_files.is_empty() {
            sources.push("git status".into());
        }
        let trackers = ["missions", "gh", "linear", "bead"]
            .into_iter()
            .filter_map(|t| {
                active
                    .trackers
                    .get(t)
                    .filter(|s| !s.is_empty())
                    .map(|s| format!("{t}:{s}"))
            })
            .collect::<Vec<_>>();
        if !trackers.is_empty() {
            sources.push(format!("active-work ({})", trackers.join(", ")));
        }
        a.output.stderr.push_str(&format!("prime: scoped to {kept} of {total} records based on {}; run with --all for the full corpus\n",sources.join(" + ")));
    }
    let contract =
        if !a.ctx.globals.json && !boolean(flags, "dry-run") && !boolean(flags, "records-only") {
            format::format_project_contract(&cfg, &a.registry, &format)
        } else {
            String::new()
        };
    if manifest {
        let domains = loaded
            .iter()
            .map(|d| ManifestDomain {
                domain: d.domain.clone(),
                count: d.records.len(),
                updated: d.updated,
                type_counts: format::compute_type_counts(&d.records),
            })
            .collect::<Vec<_>>();
        let gov = records::obj(cfg.get("governance"));
        let mut value = if a.ctx.globals.json {
            pretty(format::build_manifest_payload(&domains, gov).into())
        } else {
            let mut value = format::format_manifest(&domains, gov, &format, Utc::now());
            if format != "plain" {
                let footer = format::session_end_reminder(&format, &cfg);
                if !footer.is_empty() {
                    value.push_str("\n\n");
                    value.push_str(&footer);
                }
            }
            value
        };
        if !contract.is_empty() {
            value = format!("{contract}\n\n{value}");
        }
        return output(a, value, flags);
    }
    let mut all = Vec::new();
    for d in &mut loaded {
        d.records = sort(&d.records, &cfg);
        all.extend(d.records.clone());
    }
    let annotation_context = files
        .as_ref()
        .map(|f| ActiveContext {
            changed_files: f.clone(),
            trackers: BTreeMap::new(),
        })
        .or(active);
    let stale = if a.ctx.globals.json {
        BTreeSet::new()
    } else {
        let shelf =
            records::obj(records::obj(cfg.get("classification_defaults")).get("shelf_life"));
        prime_stale_ids(&find_stale_records(&all, a.ctx, Utc::now(), shelf))
    };
    if boolean(flags, "dry-run") {
        let selected = if budget_enabled {
            budget(&loaded, limit, &a.registry, files.as_deref(), &stale).0
        } else {
            loaded.clone()
        };
        let mut rows = Vec::new();
        let mut total = 0;
        for d in selected {
            for r in d.records {
                let cost = tokens(&estimate(&r, &a.registry));
                total += cost;
                rows.push(Value::Object(object([
                    ("id", text(r.get("id")).into()),
                    ("type", r.get("type").cloned().unwrap_or(Value::Null)),
                    ("domain", d.domain.clone().into()),
                    ("tokens", (cost as i64).into()),
                ])));
            }
        }
        let value = object([
            ("wouldPrime", Value::Array(rows)),
            ("totalTokens", (total as i64).into()),
            (
                "budgetUsed",
                if budget_enabled {
                    Value::Number(total as f64 / limit as f64)
                } else {
                    Value::Null
                },
            ),
            (
                "budgetTotal",
                if budget_enabled {
                    Value::Integer(limit as i128)
                } else {
                    Value::Null
                },
            ),
        ]);
        return output(a, pretty(value.into()), flags);
    }
    let hook_domains = loaded
        .iter()
        .map(|d| {
            Value::Object(object([
                ("domain", d.domain.clone().into()),
                (
                    "records",
                    Value::Array(d.records.iter().cloned().map(Value::Object).collect()),
                ),
            ]))
        })
        .collect();
    let hook = hooks::run_hooks(
        a.ctx,
        Some(&cfg),
        "pre-prime",
        &Value::Object(object([("domains", Value::Array(hook_domains))])),
        &mut a.output.stderr,
    )?;
    if hook.blocked {
        return Err(message(if hook.block_reason.is_empty() {
            "pre-prime hook blocked output".into()
        } else {
            hook.block_reason
        }));
    }
    if !a.ctx.globals.json {
        for warning in hook.warnings {
            a.warning(&warning);
        }
    }
    if hook.ran_any {
        let mut mutated = BTreeMap::new();
        if let Some(domains) = records::obj(Some(&hook.payload))
            .get("domains")
            .and_then(Value::as_array)
        {
            for d in domains {
                let d = records::obj(Some(d));
                if let (Some(domain), Some(rows)) = (
                    d.get("domain").and_then(Value::as_str),
                    d.get("records").and_then(Value::as_array),
                ) {
                    // Go RecordList includes array rows as records with a raw sidecar.
                    let rows = rows
                        .iter()
                        .filter_map(|r| match r {
                            Value::Object(o) => Some(o.clone()),
                            Value::Array(items) => Some(Object::from_record_array(items.clone())),
                            _ => None,
                        })
                        .collect::<Vec<_>>();
                    mutated.insert(domain.to_owned(), rows);
                }
            }
        }
        for d in &mut loaded {
            if let Some(rows) = mutated.remove(&d.domain) {
                d.records = rows;
            }
        }
    }
    for d in &mut loaded {
        d.records = sort(&d.records, &cfg);
    }
    if a.ctx.globals.json {
        let rows = loaded
            .into_iter()
            .filter(|d| files.is_none() || !d.records.is_empty())
            .map(|d| {
                Value::Object(object([
                    ("domain", d.domain.into()),
                    ("entry_count", (d.records.len() as i64).into()),
                    (
                        "records",
                        Value::Array(d.records.into_iter().map(Value::Object).collect()),
                    ),
                ]))
            })
            .collect();
        return output(
            a,
            pretty(Value::Object(object([
                ("type", "expertise".into()),
                ("domains", Value::Array(rows)),
            ]))),
            flags,
        );
    }
    let (loaded, dropped, dropped_domains) = if budget_enabled {
        budget(&loaded, limit, &a.registry, files.as_deref(), &stale)
    } else {
        (loaded, 0, 0)
    };
    let mut sections = Vec::new();
    for d in loaded {
        let mut annotations = BTreeMap::new();
        for r in &d.records {
            let id = text(r.get("id"));
            if !id.is_empty() {
                let mut note = annotation(r, annotation_context.as_ref(), Utc::now());
                if stale.contains(&id) {
                    note.push_str("; stale?");
                }
                annotations.insert(id, note);
            }
        }
        sections.push(render::format_domain(
            &d.domain,
            &d.records,
            d.updated,
            &a.registry,
            &FormatOptions {
                format: format.clone(),
                full: boolean(flags, "full") || a.ctx.globals.verbose,
                annotations,
                stale_ids: stale.clone(),
            },
        ));
    }
    let mut value = if boolean(flags, "records-only") {
        sections.join("\n\n")
    } else {
        format::format_prime(&sections, &format)
    };
    if dropped > 0 {
        let domain_part = if dropped_domains > 0 {
            format!(
                " across {dropped_domains} domain{}",
                if dropped_domains == 1 { "" } else { "s" }
            )
        } else {
            String::new()
        };
        value.push_str(&format!(
            "\n\n... and {dropped} more record{}{domain_part} (use --budget <n> to show more)",
            if dropped == 1 { "" } else { "s" }
        ));
    }
    if format != "plain" && !boolean(flags, "records-only") {
        let reminder = format::session_end_reminder(&format, &cfg);
        if !reminder.is_empty() {
            value.push_str("\n\n");
            value.push_str(&reminder);
        }
    }
    if !contract.is_empty() {
        value = format!("{contract}\n\n{value}");
    }
    output(a, value, flags)
}
