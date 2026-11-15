//! Query, rank, ready and status retain the Go command's direct output semantics.
use super::health::{self, date, go_int, object};
use crate::{
    actions::{
        api,
        record::{self, boolean, domain_names, get, has, message, text, Action},
        render,
    },
    config::Config,
    json::{Object, Value},
    records, store, timefmt, Result,
};
use chrono::{DateTime, TimeDelta, Utc};
fn raw(record: &Object, key: &str) -> Value {
    record.get(key).cloned().unwrap_or(Value::Null)
}
fn default(flags: &[(String, String)], key: &str, fallback: &str) -> String {
    if has(flags, key) {
        get(flags, key)
    } else {
        fallback.into()
    }
}
fn known(a: &mut Action<'_>, cfg: &Config, domain: &str, command: &str) -> bool {
    if records::obj(cfg.get("domains")).contains_key(domain) {
        return true;
    }
    let names = domain_names(cfg).join(", ");
    let names = if names.is_empty() { "(none)" } else { &names };
    a.error(
        command,
        &format!(
            "Domain {} not found in config. Available domains: {names}",
            records::go_quote(domain)
        ),
    );
    if !a.ctx.globals.json {
        let tick = char::from(96);
        a.output.stderr.push_str(&format!("Hint: Run {tick}luuna-atlas add {domain}{tick} to create this domain, or check .luuna/atlas/config.yaml\n"));
    }
    false
}
fn read_domain(a: &Action<'_>, domain: &str) -> Result<Vec<Object>> {
    store::read_records(
        &a.path(domain)?,
        &a.registry,
        a.ctx.globals.allow_unknown_types,
    )
}
fn all_records(a: &Action<'_>, domains: &[String]) -> Result<Vec<(String, Object)>> {
    let mut rows = Vec::new();
    for domain in domains {
        rows.extend(
            read_domain(a, domain)?
                .into_iter()
                .map(|r| (domain.clone(), r)),
        );
    }
    Ok(rows)
}
fn filter(rows: Vec<Object>, flags: &[(String, String)]) -> Vec<Object> {
    let mut rows = api::filter(
        rows,
        &get(flags, "type"),
        &get(flags, "tag"),
        &get(flags, "classification"),
        &get(flags, "file"),
    );
    let status = get(flags, "outcome-status");
    if !status.is_empty() {
        rows.retain(|r| {
            records::any_list(r.get("outcomes"))
                .iter()
                .any(|o| text(records::obj(Some(o)).get("status")) == status)
        });
    }
    if boolean(flags, "sort-by-score") {
        rows = records::sort_by_confirmation_score(&rows);
    }
    rows
}
fn format(a: &Action<'_>, flags: &[(String, String)]) -> String {
    let f = get(flags, "format");
    if !f.is_empty() {
        f
    } else if !a.ctx.globals.format.is_empty() {
        a.ctx.globals.format.clone()
    } else {
        "markdown".into()
    }
}
fn query(a: &mut Action<'_>, args: &[String], flags: &[(String, String)]) -> Result<()> {
    let cfg = a.config()?;
    let mut domains = domain_names(&cfg);
    if !boolean(flags, "all") {
        let Some(domain) = args.first() else {
            return Err(message(
                "Please specify a domain or use --all to query all domains.",
            ));
        };
        if !known(a, &cfg, domain, "query") {
            return Ok(());
        }
        domains = vec![domain.clone()];
    }
    if domains.is_empty() {
        if a.ctx.globals.json {
            a.json("query", object([("domains", Value::Array(Vec::new()))]));
        } else {
            let tick = char::from(96);
            a.output.stdout.push_str(&format!(
                "No domains configured. Run {tick}luuna-atlas add <domain>{tick} to get started.\n"
            ));
        }
        return Ok(());
    }
    let format = format(a, flags);
    let mut results = Vec::new();
    let mut sections = Vec::new();
    let mut ids = Vec::new();
    for domain in domains {
        let rows = filter(read_domain(a, &domain)?, flags);
        results.push(Value::Object(object([
            ("domain", domain.clone().into()),
            (
                "records",
                Value::Array(rows.iter().cloned().map(Value::Object).collect()),
            ),
        ])));
        if format == "ids" {
            for row in &rows {
                let id = text(row.get("id"));
                if !id.is_empty() {
                    ids.push(id);
                }
            }
        } else {
            sections.push(render::format_domain(
                &domain,
                &rows,
                timefmt::file_mod_time(&a.path(&domain)?),
                &a.registry,
                &render::FormatOptions {
                    format: format.clone(),
                    ..Default::default()
                },
            ));
        }
    }
    if a.ctx.globals.json {
        a.json("query", object([("domains", Value::Array(results))]));
    } else if format == "ids" {
        if !ids.is_empty() {
            a.output.stdout.push_str(&format!("{}\n", ids.join("\n")));
        }
    } else {
        a.output
            .stdout
            .push_str(&format!("{}\n", sections.join("\n\n")));
    }
    Ok(())
}
fn limit(raw: &str) -> Result<usize> {
    if !raw.is_empty() && raw.bytes().all(|b| b.is_ascii_digit()) {
        if let Ok(n) = raw.parse::<i64>() {
            if n > 0 {
                return Ok(n as usize);
            }
        }
    }
    Err(message(format!(
        "--limit must be a positive integer (got {}).",
        records::go_quote(raw)
    )))
}
fn rank(a: &mut Action<'_>, args: &[String], flags: &[(String, String)]) -> Result<()> {
    let limit = limit(&default(flags, "limit", "10"))?;
    let min = record::number_flag(&default(flags, "min-score", "0"), "min-score")?;
    let cfg = a.config()?;
    let mut domains = domain_names(&cfg);
    let scope = if let Some(domain) = args.first() {
        if !known(a, &cfg, domain, "rank") {
            return Ok(());
        }
        domains = vec![domain.clone()];
        domain.as_str()
    } else {
        "all domains"
    };
    let typ = get(flags, "type");
    let mut rows = all_records(a, &domains)?;
    rows.retain(|(_, r)| {
        (typ.is_empty() || text(r.get("type")) == typ) && records::confirmation_score(r) >= min
    });
    rows.sort_by(|(_, a), (_, b)| {
        records::confirmation_score(b)
            .partial_cmp(&records::confirmation_score(a))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    rows.truncate(limit);
    if a.ctx.globals.json {
        let entries = rows
            .iter()
            .map(|(d, r)| {
                Value::Object(object([
                    ("domain", d.clone().into()),
                    ("id", raw(r, "id")),
                    ("type", raw(r, "type")),
                    ("score", records::confirmation_score(r).into()),
                    ("summary", records::record_summary(r, &a.registry).into()),
                    ("record", r.clone().into()),
                ]))
            })
            .collect();
        a.json(
            "rank",
            object([
                ("count", (rows.len() as i64).into()),
                ("records", Value::Array(entries)),
            ]),
        );
        return Ok(());
    }
    if rows.is_empty() {
        a.output
            .stdout
            .push_str("No records match the ranking criteria.\n");
        return Ok(());
    }
    a.output.stdout.push_str(&format!(
        "Top {} record{} by confirmation score ({scope})\n\n",
        rows.len(),
        if rows.len() == 1 { "" } else { "s" }
    ));
    for (d, r) in &rows {
        let id = text(r.get("id"));
        let id = if id.is_empty() {
            String::new()
        } else {
            format!("{}  ", a.styled("accent", &id))
        };
        let score = format!("★{}", records::confirmation_score(r));
        let typ = format!("[{}]", text(r.get("type")));
        a.output.stdout.push_str(&format!(
            "  {id}{}{}{}{}\n",
            a.styled("green", &format!("{score:<6}")),
            a.styled("cyan", &format!("{d:<14}")),
            a.styled("yellow", &format!("{typ:<14}")),
            records::record_summary(r, &a.registry)
        ));
    }
    Ok(())
}
fn cutoff(raw: &str, now: DateTime<Utc>) -> Result<DateTime<Utc>> {
    let bytes = raw.as_bytes();
    if bytes.len() < 2
        || !bytes[..bytes.len() - 1].iter().all(u8::is_ascii_digit)
        || !matches!(bytes.last(), Some(b'h' | b'd' | b'w'))
    {
        return Err(message(format!(
            "Invalid duration: {}. Use format like \"24h\", \"7d\", \"2w\".",
            records::go_quote(raw)
        )));
    }
    // Every preceding byte is now known ASCII, so the final-unit split is a
    // UTF-8 boundary even when hostile input contains multibyte characters.
    let (digits, unit) = raw.split_at(bytes.len() - 1);
    let mut hours = digits.parse::<f64>().unwrap_or(f64::INFINITY);
    if unit == "d" {
        hours *= 24.0;
    }
    if unit == "w" {
        hours *= 168.0;
    }
    let nanos = go_int(hours * 3_600_000_000_000.0).wrapping_neg();
    Ok(now + TimeDelta::nanoseconds(nanos))
}
fn ready(a: &mut Action<'_>, flags: &[(String, String)]) -> Result<()> {
    let cfg = a.config()?;
    let limit = limit(&default(flags, "limit", "10"))?;
    let mut domains = domain_names(&cfg);
    let domain = get(flags, "domain");
    if !domain.is_empty() {
        if !records::obj(cfg.get("domains")).contains_key(&domain) {
            return Err(message(format!(
                "domain {} not found. Available: {}",
                records::go_quote(&domain),
                domains.join(", ")
            )));
        }
        domains = vec![domain];
    }
    let since = get(flags, "since");
    let now = Utc::now();
    let cutoff = if since.is_empty() {
        None
    } else {
        Some(cutoff(&since, now)?)
    };
    let mut rows = all_records(a, &domains)?;
    rows.sort_by(|(_, a), (_, b)| {
        date(&text(b.get("recorded_at"))).cmp(&date(&text(a.get("recorded_at"))))
    });
    rows.retain(|(_, r)| cutoff.is_none_or(|cutoff| date(&text(r.get("recorded_at"))) >= cutoff));
    rows.truncate(limit);
    if a.ctx.globals.json {
        let entries = rows
            .iter()
            .map(|(d, r)| {
                Value::Object(object([
                    ("domain", d.clone().into()),
                    ("id", raw(r, "id")),
                    ("type", raw(r, "type")),
                    ("recorded_at", raw(r, "recorded_at")),
                    ("summary", records::record_summary(r, &a.registry).into()),
                    ("record", r.clone().into()),
                ]))
            })
            .collect();
        a.json(
            "ready",
            object([
                ("count", (rows.len() as i64).into()),
                ("entries", Value::Array(entries)),
            ]),
        );
        return Ok(());
    }
    if rows.is_empty() {
        a.output
            .stdout
            .push_str("No recent expertise records found.\n");
        return Ok(());
    }
    let header = if since.is_empty() {
        format!("Recent Expertise (last {} records)", rows.len())
    } else {
        format!("Recent Expertise (last {since})")
    };
    a.output.stdout.push_str(&format!("{header}\n\n"));
    for (d, r) in &rows {
        let id = text(r.get("id"));
        let id = if id.is_empty() {
            String::new()
        } else {
            format!("{}  ", a.styled("accent", &id))
        };
        let ago = timefmt::format_time_ago(date(&text(r.get("recorded_at"))), Utc::now());
        let typ = format!("[{}]", text(r.get("type")));
        a.output.stdout.push_str(&format!(
            "  {id}{}{}{}{}\n",
            a.styled("dim", &format!("{ago:<10}")),
            a.styled("cyan", &format!("{d:<14}")),
            a.styled("yellow", &format!("{typ:<14}")),
            records::record_summary(r, &a.registry)
        ));
    }
    Ok(())
}
pub fn manifest_status_suffix(count: usize, gov: &Object) -> &'static str {
    let n = count as f64;
    if n >= records::number(gov.get("hard_limit")) {
        " ⚠ OVER HARD LIMIT — must decompose"
    } else if n >= records::number(gov.get("warn_entries")) {
        " ⚠ consider splitting domain"
    } else if n >= records::number(gov.get("max_entries")) {
        " — approaching limit"
    } else {
        ""
    }
}
pub struct DomainStatus {
    pub domain: String,
    pub count: usize,
    pub last_updated: Option<DateTime<Utc>>,
    pub oldest: Option<DateTime<Utc>>,
    pub newest: Option<DateTime<Utc>>,
    pub rotting_days: Option<i64>,
}
pub fn format_status(stats: &[DomainStatus], gov: &Object, now: DateTime<Utc>) -> String {
    let mut lines = vec!["Atlas Status".into(), "============".into(), String::new()];
    if stats.is_empty() {
        let tick = char::from(96);
        lines.push(format!(
            "No domains configured. Run {tick}luuna-atlas add <domain>{tick} to get started."
        ));
        return lines.join("\n");
    }
    for s in stats {
        let updated = s
            .last_updated
            .map(|t| timefmt::format_time_ago(t, now))
            .unwrap_or_else(|| "never".into());
        let range = if let (Some(old), Some(new)) = (s.oldest, s.newest) {
            let old = timefmt::format_time_ago(old, now);
            let new = timefmt::format_time_ago(new, now);
            if old == new {
                format!(" — recorded {old}")
            } else {
                format!(" — recorded {old} → {new}")
            }
        } else {
            String::new()
        };
        let rot = s
            .rotting_days
            .map(|d| format!(" ⚠ ROTTING (no writes in {d}d)"))
            .unwrap_or_default();
        lines.push(format!(
            "  {}: {} records (updated {updated}){range}{}{rot}",
            s.domain,
            s.count,
            manifest_status_suffix(s.count, gov)
        ));
    }
    lines.join("\n")
}
fn status(a: &mut Action<'_>) -> Result<()> {
    if !a.ctx.root.join(".luuna/atlas").exists() {
        let tick = char::from(96);
        return Err(message(format!(
            "No .luuna/atlas/ directory found. Run {tick}luuna-atlas init{tick} first."
        )));
    }
    let cfg = a.config()?;
    let gov = records::obj(cfg.get("governance"));
    let shelf = records::obj(records::obj(cfg.get("classification_defaults")).get("shelf_life"));
    let mut rows = Vec::new();
    let mut stats = Vec::new();
    for domain in domain_names(&cfg) {
        let records = read_domain(a, &domain)?;
        let now = Utc::now();
        let mut health = health::calculate_domain_health(
            &records,
            records::number(gov.get("max_entries")),
            shelf,
            &a.registry,
            now,
        );
        let old = text(health.get("oldest_timestamp"));
        let new = text(health.get("newest_timestamp"));
        let oldest = if old.is_empty() {
            None
        } else {
            Some(date(&old))
        };
        let newest = if new.is_empty() {
            None
        } else {
            Some(date(&new))
        };
        let days = newest.and_then(|at| {
            let age = (health::hours(now, at) / 24.0).floor() as i64;
            if age > go_int(records::number(shelf.get("observational"))) {
                Some(age)
            } else {
                None
            }
        });
        let modified = timefmt::file_mod_time(&a.path(&domain)?);
        let max = records::number(gov.get("max_entries"));
        let util = if max > 0.0 {
            go_int((records.len() as f64 / max * 100.0 + 0.5).floor())
        } else {
            0
        };
        health.insert_unordered("governance_utilization", util);
        rows.push(Value::Object(object([
            ("domain", domain.clone().into()),
            ("count", (records.len() as i64).into()),
            (
                "lastUpdated",
                modified
                    .map(|t| Value::from(timefmt::iso(t)))
                    .unwrap_or(Value::Null),
            ),
            (
                "oldest_recorded",
                oldest
                    .map(|t| Value::from(timefmt::iso(t)))
                    .unwrap_or(Value::Null),
            ),
            (
                "newest_recorded",
                newest
                    .map(|t| Value::from(timefmt::iso(t)))
                    .unwrap_or(Value::Null),
            ),
            ("rotting", days.is_some().into()),
            ("rotting_days", days.map(Value::from).unwrap_or(Value::Null)),
            ("health", health.into()),
        ])));
        stats.push(DomainStatus {
            domain,
            count: records.len(),
            last_updated: modified,
            oldest,
            newest,
            rotting_days: days,
        });
    }
    if a.ctx.globals.json {
        a.json(
            "status",
            object([
                ("domains", Value::Array(rows)),
                ("governance", gov.clone().into()),
                ("shelf_life", shelf.clone().into()),
            ]),
        );
    } else {
        a.output
            .stdout
            .push_str(&format!("{}\n", format_status(&stats, gov, Utc::now())));
    }
    Ok(())
}
pub(crate) fn run(
    a: &mut Action<'_>,
    command: &str,
    args: &[String],
    flags: &[(String, String)],
) -> Result<()> {
    match command {
        "query" => query(a, args, flags),
        "rank" => rank(a, args, flags),
        "ready" => ready(a, flags),
        "status" => status(a),
        "validate" => health::run_validate(a),
        _ => Err(message("Unknown read command")),
    }
}
