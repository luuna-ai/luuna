//! Retention execution and its reporting.
use super::{
    archive,
    health::{is_record_stale, object},
    retention::{anchor_validity, collect_superseded_ids, passed_anchor_grace, AnchorValidity},
};
use crate::{
    actions::record::{boolean, domain_names, message, strings, text, Action},
    hooks,
    json::{self, Object, Value},
    records, store, timefmt, Result,
};
use chrono::{DateTime, Utc};
use std::collections::BTreeMap;
struct Changes {
    kept: Vec<Object>,
    archived: Vec<Object>,
    removed: Vec<usize>,
    actions: Vec<Object>,
    result: Option<Object>,
}
fn next_tier(tier: &str) -> &str {
    match tier {
        "foundational" => "tactical",
        "tactical" => "observational",
        _ => "",
    }
}
fn anchor_errors(cfg: &Object) -> Vec<String> {
    let mut errors = Vec::new();
    for field in ["threshold", "grace_days"] {
        let Some(value) = cfg.get(field) else {
            continue;
        };
        let n = records::number(Some(value));
        if !matches!(value, Value::Number(_) | Value::Integer(_)) || !n.is_finite() {
            let pretty = json::to_indented_string(value).unwrap_or_default();
            errors.push(format!("{field} must be a finite number (got {pretty})"));
            continue;
        }
        if field == "threshold" && !(0.0..=1.0).contains(&n) {
            errors.push(format!(
                "threshold must be between 0 and 1 (got {})",
                text(Some(value))
            ));
        }
        if field == "grace_days" && n < 0.0 {
            errors.push(format!(
                "grace_days must be >= 0 (got {})",
                text(Some(value))
            ));
        }
    }
    errors
}
struct Policy<'a> {
    a: &'a Action<'a>,
    now: DateTime<Utc>,
    shelf: Object,
    validity: BTreeMap<String, AnchorValidity>,
    threshold: f64,
    grace: f64,
    superseded: std::collections::BTreeSet<String>,
    flags: &'a [(String, String)],
}
impl Policy<'_> {
    fn anchor_hit(&self, r: &Object) -> Option<AnchorValidity> {
        if !boolean(self.flags, "check-anchors") || !passed_anchor_grace(r, self.now, self.grace) {
            return None;
        }
        let id = text(r.get("id"));
        let v = if id.is_empty() {
            anchor_validity(r, &self.a.ctx.cwd)
        } else {
            self.validity.get(&id)?.clone()
        };
        if v.valid_fraction.is_some_and(|f| f < self.threshold) {
            Some(v)
        } else {
            None
        }
    }
    fn changes(&self, domain: &str, rows: &[Object]) -> Changes {
        let mut c = Changes {
            kept: Vec::new(),
            archived: Vec::new(),
            removed: Vec::new(),
            actions: Vec::new(),
            result: None,
        };
        let (mut pruned, mut demoted, mut anchor_count, mut sup_count) = (0, 0, 0, 0);
        for (index, r) in rows.iter().enumerate() {
            let from = text(r.get("classification"));
            if is_record_stale(r, self.now, &self.shelf) {
                pruned += 1;
                c.archived.push(r.clone());
                c.removed.push(index);
                let mut action = object([
                    ("domain", domain.into()),
                    ("type", r.get("type").cloned().unwrap_or(Value::Null)),
                    ("from", from.clone().into()),
                    ("to", "archived".into()),
                    ("reasons", strings(vec!["stale".into()])),
                ]);
                if let Some(id) = r.get("id") {
                    action.insert_unordered("id", id.clone());
                }
                action.remember_order(["domain", "id", "type", "from", "to", "reasons"]);
                c.actions.push(action);
                continue;
            }
            let id = text(r.get("id"));
            let sup = !id.is_empty() && self.superseded.contains(&id);
            let anchor = self.anchor_hit(r);
            if !sup && anchor.is_none() {
                c.kept.push(r.clone());
                continue;
            }
            let mut reasons = Vec::new();
            if sup {
                reasons.push("superseded".into());
            }
            if anchor.is_some() {
                reasons.push("anchor_decay".into());
            }
            let to = if boolean(self.flags, "aggressive") {
                ""
            } else {
                next_tier(&from)
            };
            let mut action = object([
                ("domain", domain.into()),
                ("type", r.get("type").cloned().unwrap_or(Value::Null)),
                ("from", from.clone().into()),
                ("reasons", strings(reasons.clone())),
            ]);
            if let Some(id) = r.get("id") {
                action.insert_unordered("id", id.clone());
            }
            if let Some(v) = &anchor {
                let mut value = object([
                    ("valid_fraction", v.valid_fraction.unwrap().into()),
                    ("valid", (v.valid as i64).into()),
                    ("total", (v.total as i64).into()),
                    (
                        "broken",
                        Value::Array(v.broken.iter().map(|a| a.value()).collect()),
                    ),
                ]);
                value.remember_order(["valid_fraction", "valid", "total", "broken"]);
                action.insert_unordered("anchors", value);
            }
            if to.is_empty() {
                pruned += 1;
                let mut r = r.clone();
                r.insert_unordered("archive_reason", reasons.join("+"));
                c.archived.push(r);
                c.removed.push(index);
                action.insert_unordered("to", "archived");
            } else {
                let mut r = r.clone();
                let mut keys = r.keys().map(str::to_owned).collect::<Vec<_>>();
                r.insert_unordered("classification", to);
                if sup {
                    r.insert_unordered("supersession_demoted_at", timefmt::iso(self.now));
                    sup_count += 1;
                }
                if anchor.is_some() {
                    r.insert_unordered("anchor_decay_demoted_at", timefmt::iso(self.now));
                    anchor_count += 1;
                }
                keys.extend([
                    "classification".into(),
                    "supersession_demoted_at".into(),
                    "anchor_decay_demoted_at".into(),
                ]);
                r.remember_order(keys);
                c.kept.push(r);
                demoted += 1;
                action.insert_unordered("to", to);
            }
            action.remember_order(["domain", "id", "type", "from", "to", "reasons", "anchors"]);
            c.actions.push(action);
        }
        if pruned > 0 || demoted > 0 {
            let mut result = object([
                ("domain", domain.into()),
                ("before", (rows.len() as i64).into()),
                ("pruned", pruned.into()),
                ("demoted", demoted.into()),
                ("anchor_demoted", anchor_count.into()),
                ("supersession_demoted", sup_count.into()),
                ("after", (c.kept.len() as i64).into()),
            ]);
            result.remember_order([
                "domain",
                "before",
                "pruned",
                "demoted",
                "anchor_demoted",
                "supersession_demoted",
                "after",
            ]);
            c.result = Some(result);
        }
        c
    }
    fn preflight(&self, domain: &str, rows: &[Object], c: &mut Changes) -> Result<()> {
        let live = self.a.path(domain)?;
        archive::preflight_live(self.a, &live, rows, &mut c.kept, &c.removed)?;
        if !boolean(self.flags, "hard") && !c.archived.is_empty() {
            for index in &c.removed {
                let old = &rows[*index];
                if let Some(value) = old.get("archive_reason").filter(|v| !v.is_null()) {
                    if let Some(new) = c
                        .archived
                        .iter()
                        .find(|r| text(r.get("id")) == text(old.get("id")))
                    {
                        if new.get("archive_reason") != Some(value) {
                            return Err(message("Refusing to prune: automatic archival would replace a stored archive_reason. Files are unchanged."));
                        }
                    }
                }
            }
            records::check_lossless_rewrite(&live, &self.a.registry, &[])?;
            records::check_retained_numbers(&live, c.archived.iter().map(|r| (r, r)))?;
            let path = archive::path(self.a, domain)?;
            let rows = store::read_archive(&path, &self.a.registry)?;
            archive::preflight_archive(self.a, &path, &rows, &[])?;
        }
        Ok(())
    }
}
pub(crate) fn run(a: &mut Action<'_>, flags: &[(String, String)]) -> Result<()> {
    let cfg = a.config()?;
    let now = Utc::now();
    let shelf = records::obj(records::obj(cfg.get("classification_defaults")).get("shelf_life"));
    let anchor_cfg = records::obj(records::obj(cfg.get("decay")).get("anchor_validity"));
    let errors = anchor_errors(anchor_cfg);
    if !errors.is_empty() {
        return Err(message(format!(
            "Invalid decay.anchor_validity config: {}. Edit .luuna/atlas/config.yaml.",
            errors.join("; ")
        )));
    }
    let mut loaded = Vec::new();
    for d in domain_names(&cfg) {
        let rows =
            store::read_records(&a.path(&d)?, &a.registry, a.ctx.globals.allow_unknown_types)?;
        loaded.push((d, rows));
    }
    let (superseded, cycles) = collect_superseded_ids(&loaded);
    if !cycles.is_empty() && !a.ctx.globals.json && !a.ctx.globals.quiet {
        let tick = char::from(96);
        a.warning(&format!("supersession cycle detected for {} record(s); cycle members will not be demoted. Run {tick}luuna-atlas doctor{tick} for details.",cycles.len()));
    }
    let mut validity = BTreeMap::new();
    if boolean(flags, "check-anchors") {
        for (_, rows) in &loaded {
            for r in rows {
                let id = text(r.get("id"));
                if !id.is_empty() {
                    validity.insert(id, anchor_validity(r, &a.ctx.cwd));
                }
            }
        }
    }
    let mut candidates = Vec::new();
    let mut candidate_domains = Vec::new();
    {
        let policy = Policy {
            a,
            now,
            shelf: shelf.clone(),
            validity: validity.clone(),
            threshold: records::number_default(anchor_cfg.get("threshold"), 0.5),
            grace: records::number_default(anchor_cfg.get("grace_days"), 7.0),
            superseded: superseded.clone(),
            flags,
        };
        for (d, rows) in &loaded {
            let (mut stale, mut demote, mut anchor_decay) = (Vec::new(), Vec::new(), Vec::new());
            for r in rows {
                if is_record_stale(r, now, shelf) {
                    stale.push(Value::Object(r.clone()));
                    continue;
                }
                let id = text(r.get("id"));
                if !id.is_empty() && superseded.contains(&id) {
                    demote.push(Value::Object(r.clone()));
                }
                if policy.anchor_hit(r).is_some() {
                    anchor_decay.push(Value::Object(r.clone()));
                }
            }
            if !stale.is_empty() || !demote.is_empty() || !anchor_decay.is_empty() {
                let mut c = Object::new();
                c.insert("domain", d.clone());
                c.insert("stale", Value::Array(stale));
                c.insert("demote", Value::Array(demote));
                c.insert("anchor_decay", Value::Array(anchor_decay));
                candidates.push(Value::Object(c));
                candidate_domains.push(d.clone());
            }
        }
    }
    let dry = boolean(flags, "dry-run");
    if !dry && !candidates.is_empty() {
        let hook = hooks::run_hooks(
            a.ctx,
            Some(&cfg),
            "pre-prune",
            &Value::Object(object([("candidates", Value::Array(candidates))])),
            &mut a.output.stderr,
        )?;
        if hook.blocked {
            return Err(message(if hook.block_reason.is_empty() {
                "pre-prune hook blocked".into()
            } else {
                hook.block_reason
            }));
        }
        if !a.ctx.globals.json {
            for warning in hook.warnings {
                a.warning(&warning);
            }
        }
    }
    // Preflight every candidate domain before the first file write.
    let mut prepared = Vec::new();
    {
        let policy = Policy {
            a,
            now,
            shelf: shelf.clone(),
            validity: validity.clone(),
            threshold: records::number_default(anchor_cfg.get("threshold"), 0.5),
            grace: records::number_default(anchor_cfg.get("grace_days"), 7.0),
            superseded: superseded.clone(),
            flags,
        };
        for d in candidate_domains {
            let path = a.path(&d)?;
            let rows = store::read_records(&path, &a.registry, a.ctx.globals.allow_unknown_types)?;
            let mut c = policy.changes(&d, &rows);
            if !dry && c.result.is_some() {
                policy.preflight(&d, &rows, &mut c)?;
            }
            prepared.push((d, c));
        }
    }
    let mut results = Vec::new();
    let mut actions = Vec::new();
    for (d, original) in prepared {
        let path = a.path(&d)?;
        let mut c = store::with_file_lock(&path, || {
            let rows = store::read_records(&path, &a.registry, a.ctx.globals.allow_unknown_types)?;
            let policy = Policy {
                a,
                now,
                shelf: shelf.clone(),
                validity: validity.clone(),
                threshold: records::number_default(anchor_cfg.get("threshold"), 0.5),
                grace: records::number_default(anchor_cfg.get("grace_days"), 7.0),
                superseded: superseded.clone(),
                flags,
            };
            let mut c = if dry {
                original
            } else {
                policy.changes(&d, &rows)
            };
            if !dry && c.result.is_some() {
                policy.preflight(&d, &rows, &mut c)?;
                store::write_records(&path, &mut c.kept, &a.registry)?;
            }
            Ok(c)
        })?;
        if let Some(result) = c.result.take() {
            if !dry && !boolean(flags, "hard") && !c.archived.is_empty() {
                let path = archive::path(a, &d)?;
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
                    rows.extend(archive::stamp(&c.archived, "stale"));
                    store::write_archive(&path, &rows)
                })?;
            }
            results.push(result);
            actions.extend(c.actions);
        }
    }
    report(a, flags, results, actions);
    Ok(())
}
fn report(
    a: &mut Action<'_>,
    flags: &[(String, String)],
    results: Vec<Object>,
    actions: Vec<Object>,
) {
    let count = |key: &str| {
        results
            .iter()
            .map(|r| records::number(r.get(key)) as i64)
            .sum::<i64>()
    };
    let (p, d, ac, s) = (
        count("pruned"),
        count("demoted"),
        count("anchor_demoted"),
        count("supersession_demoted"),
    );
    let dry = boolean(flags, "dry-run");
    let hard = boolean(flags, "hard");
    if a.ctx.globals.json {
        let mut data = object([
            ("dryRun", dry.into()),
            ("hard", hard.into()),
            ("aggressive", boolean(flags, "aggressive").into()),
            ("checkAnchors", boolean(flags, "check-anchors").into()),
            ("totalPruned", p.into()),
            ("totalDemoted", d.into()),
            ("totalAnchorDemoted", ac.into()),
            ("totalSupersessionDemoted", s.into()),
            (
                "results",
                Value::Array(results.into_iter().map(Value::Object).collect()),
            ),
        ]);
        if boolean(flags, "explain") {
            data.insert_unordered(
                "explanations",
                Value::Array(
                    actions
                        .into_iter()
                        .filter(|v| records::list(v.get("reasons")).iter().any(|r| r != "stale"))
                        .map(Value::Object)
                        .collect(),
                ),
            );
        }
        a.json("prune", data);
        return;
    }
    if p == 0 && d == 0 {
        a.human(a.styled(
            "brand",
            "No stale or superseded records found. All records are current.",
        ));
        return;
    }
    let label = match (dry, hard) {
        (true, true) => "Would delete",
        (true, false) => "Would archive",
        (false, true) => "Deleted",
        (false, false) => "Archived",
    };
    let demote = if dry { "Would demote" } else { "Demoted" };
    let prefix = if dry {
        a.styled("yellow", "[DRY RUN] ")
    } else {
        String::new()
    };
    if !a.ctx.globals.quiet {
        for r in &results {
            let (p, d) = (
                records::number(r.get("pruned")) as i64,
                records::number(r.get("demoted")) as i64,
            );
            let body = if p > 0 && d > 0 {
                format!(
                    "{label} {}, demoted {}",
                    a.styled("red", &p.to_string()),
                    a.styled("yellow", &d.to_string())
                )
            } else if p > 0 {
                format!("{label} {}", a.styled("red", &p.to_string()))
            } else {
                format!("{demote} {}", a.styled("yellow", &d.to_string()))
            };
            a.human(format!(
                "{prefix}{}: {body} of {} records ({} remaining)",
                a.styled("cyan", &text(r.get("domain"))),
                text(r.get("before")),
                text(r.get("after"))
            ));
            for action in actions
                .iter()
                .filter(|v| text(v.get("domain")) == text(r.get("domain")))
            {
                let id = text(action.get("id"));
                a.human(format!(
                    "  {} [{}]: {} → {} ({})",
                    a.styled("dim", if id.is_empty() { "(no id)" } else { &id }),
                    text(action.get("type")),
                    text(action.get("from")),
                    text(action.get("to")),
                    records::list(action.get("reasons")).join(" + ")
                ));
                if boolean(flags, "explain") {
                    if let Some(anchors) = action.get("anchors").and_then(Value::as_object) {
                        a.human(format!(
                            "      anchors: {}/{} valid ({:.0}%)",
                            text(anchors.get("valid")),
                            text(anchors.get("total")),
                            records::number(anchors.get("valid_fraction")) * 100.0
                        ));
                        if let Some(broken) = anchors.get("broken").and_then(Value::as_array) {
                            for b in broken {
                                let b = records::obj(Some(b));
                                a.human(format!(
                                    "        {} {}: {}",
                                    a.styled("red", "✗"),
                                    text(b.get("kind")),
                                    text(b.get("path"))
                                ));
                            }
                        }
                    }
                }
            }
        }
    }
    let mut totals = Vec::new();
    if p > 0 {
        totals.push(format!(
            "{} {p} stale record{}",
            label.to_lowercase(),
            if p == 1 { "" } else { "s" }
        ));
    }
    if d > 0 {
        let mut breakdown = Vec::new();
        if s > 0 {
            breakdown.push(format!("{s} superseded"));
        }
        if ac > 0 {
            breakdown.push(format!("{ac} anchor-decayed"));
        }
        let (suffix, tag) = if breakdown.len() > 1 {
            (format!(" ({})", breakdown.join(", ")), "")
        } else if s > 0 {
            (String::new(), "superseded ")
        } else if ac > 0 {
            (String::new(), "anchor-decayed ")
        } else {
            (String::new(), "")
        };
        totals.push(format!(
            "{} {d} {tag}record{}{suffix}",
            demote.to_lowercase(),
            if d == 1 { "" } else { "s" }
        ));
    }
    let separator = if a.ctx.globals.quiet { "" } else { "\n" };
    a.output.stdout.push_str(&format!(
        "{separator}{prefix}{}\n",
        a.styled("bold", &format!("Total: {}.", totals.join("; ")))
    ));
    if !a.ctx.globals.quiet && !hard && !dry && p > 0 {
        let tick = char::from(96);
        a.human(a.styled("dim",&format!("Records moved to .luuna/atlas/archive/. Restore with {tick}luuna-atlas restore <id>{tick} or use {tick}--hard{tick} next time to permanently delete.")));
    }
}
