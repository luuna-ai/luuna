//! Corpus health metrics, tracker citations and remediation suggestions.
use crate::{
    actions::record::{boolean, get, message, text, Action},
    config::Config,
    evidence,
    json::{self, Object, Value},
    lifecycle::health,
    records, store, Result,
};
use std::{collections::BTreeMap, fs};
const TRACKERS: [&str; 4] = ["missions", "gh", "linear", "bead"];
fn obj(fields: impl IntoIterator<Item = (&'static str, Value)>) -> Object {
    let mut out = Object::new();
    for (k, v) in fields {
        out.insert_unordered(k, v);
    }
    out
}
fn strings(a: Vec<String>) -> Value {
    Value::Array(a.into_iter().map(Value::String).collect())
}
fn n(v: usize) -> Value {
    Value::Integer(v as i128)
}
fn count(m: &BTreeMap<String, usize>) -> Object {
    let mut out = Object::new();
    for (k, v) in m {
        out.insert_unordered(k, *v as i64);
    }
    out
}
pub fn thresholds(cfg: &Config, domain: &str) -> Object {
    let mut out = obj([
        ("evidence_coverage", 0.5.into()),
        ("evidence_coverage_warn", 0.3.into()),
        ("floater_max", 0.2.into()),
        ("rule_density_min", 0.25.into()),
        ("rule_density_warn", 0.15.into()),
        ("max_records_per_domain", 200.into()),
        ("max_stale", 0.into()),
    ]);
    let audit = records::obj(cfg.get("audit"));
    for extra in [
        records::obj(audit.get("thresholds")),
        records::obj(records::obj(audit.get("per_domain")).get(domain)),
    ] {
        for (k, v) in extra.iter() {
            out.insert_go_key(extra.key_bytes(k), v.clone());
        }
    }
    let mut keys: Vec<_> = out.keys().map(str::to_owned).collect();
    keys.sort_by(|a, b| out.key_bytes(a).cmp(out.key_bytes(b)));
    out.remember_order(keys);
    out
}
pub fn floater(r: &Object) -> bool {
    let ev = records::obj(r.get("evidence"));
    evidence::evidence_missions(r.get("evidence")).is_empty()
        && ["gh", "linear", "bead", "commit"]
            .iter()
            .all(|k| text(ev.get(k)).is_empty())
        && records::list(r.get("relates_to")).is_empty()
}
pub fn metric(value: f64, pass: f64, warn: f64, low: bool) -> Object {
    let verdict = if if low { value <= pass } else { value >= pass } {
        "PASS"
    } else if if low { value <= warn } else { value >= warn } {
        "WARN"
    } else {
        "FAIL"
    };
    let mut o = obj([
        ("verdict", verdict.into()),
        ("value", value.into()),
        ("threshold", pass.into()),
    ]);
    if !low {
        o.insert_unordered("warn_threshold", warn);
    }
    o
}
#[derive(Clone)]
struct Citation {
    tracker: String,
    id: String,
    count: usize,
}
type Item = (String, Object);
fn prefix_bytes(s: &str, max: usize) -> Value {
    Value::from_go_bytes(s.as_bytes()[..s.len().min(max)].to_vec())
}
fn default(o: &Object, key: &str, fallback: &str) -> String {
    match o.get(key) {
        None | Some(Value::Null) => fallback.into(),
        v => text(v),
    }
}
fn compute(
    a: &Action<'_>,
    cfg: &Config,
    flags: &[(String, String)],
) -> Result<(Object, Vec<Item>)> {
    let mut ignored: Vec<Vec<u8>> =
        records::any_list(records::obj(cfg.get("audit")).get("ignore_domains"))
            .iter()
            .filter_map(|v| v.string_bytes().map(<[u8]>::to_vec))
            .collect();
    ignored.extend(
        get(flags, "ignore-domains")
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| s.as_bytes().to_vec()),
    );
    let mut seen = std::collections::BTreeSet::new();
    ignored.retain(|s| seen.insert(s.clone()));
    let scope = get(flags, "domain");
    let configured = records::obj(cfg.get("domains"));
    let domains: Vec<String> = configured
        .keys()
        .filter(|k| {
            if !scope.is_empty() {
                configured.key_bytes(k) == scope.as_bytes()
            } else {
                !ignored.iter().any(|s| s == configured.key_bytes(k))
            }
        })
        .map(|k| configured.key_text(k).to_owned())
        .collect();
    let mut items = Vec::new();
    for d in &domains {
        for r in store::read_records(&a.path(d)?, &a.registry, true)? {
            items.push((d.clone(), r));
        }
    }
    let total = items.len();
    let thresholds = thresholds(cfg, &scope);
    let mut per = Object::new();
    for d in &domains {
        per.insert_unordered(d, self::thresholds(cfg, d));
    }
    let mut type_counts = BTreeMap::new();
    let mut type_order = Vec::new();
    let mut with_tracker: BTreeMap<String, usize> =
        TRACKERS.iter().map(|s| (s.to_string(), 0)).collect();
    let (
        mut with_commit,
        mut with_relates,
        mut with_any,
        mut floaters,
        mut conv_total,
        mut conv_signal,
    ) = (0, 0, 0, 0, 0, 0);
    let mut citations: Vec<Citation> = Vec::new();
    for (_, r) in &items {
        let t = text(r.get("type"));
        if !type_counts.contains_key(&t) {
            type_order.push(t.clone());
        }
        *type_counts.entry(t.clone()).or_default() += 1;
        let ev = records::obj(r.get("evidence"));
        for tracker in TRACKERS {
            let value = text(ev.get(tracker));
            if value.is_empty() {
                continue;
            }
            *with_tracker.get_mut(tracker).unwrap() += 1;
            if let Some(c) = citations
                .iter_mut()
                .find(|c| c.tracker == tracker && c.id == value)
            {
                c.count += 1
            } else {
                citations.push(Citation {
                    tracker: tracker.into(),
                    id: value,
                    count: 1,
                })
            }
        }
        if !text(ev.get("commit")).is_empty() {
            with_commit += 1;
        }
        if !records::list(r.get("relates_to")).is_empty() {
            with_relates += 1;
        }
        if !evidence::evidence_missions(r.get("evidence")).is_empty()
            || ["gh", "linear", "bead", "commit"]
                .iter()
                .any(|k| !text(ev.get(k)).is_empty())
        {
            with_any += 1;
        }
        if floater(r) {
            floaters += 1;
        }
        if t == "convention" {
            conv_total += 1;
            if records::has_rule_signal(&text(r.get("content"))) {
                conv_signal += 1;
            }
        }
    }
    type_order.sort_by_key(|t| std::cmp::Reverse(type_counts[t]));
    let mix = type_order
        .iter()
        .map(|t| {
            Value::Object(obj([
                ("type", t.clone().into()),
                ("count", n(type_counts[t])),
                (
                    "pct",
                    n(if total > 0 {
                        100 * type_counts[t] / total
                    } else {
                        0
                    }),
                ),
            ]))
        })
        .collect();
    let mut index = BTreeMap::new();
    if let Ok(bytes) = fs::read(evidence::resolve_missions_dir(&a.ctx.cwd).join("issues.jsonl")) {
        for line in bytes.split(|b| *b == b'\n') {
            let line = String::from_utf8_lossy(line);
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Ok(v) = json::parse(line) {
                let row = records::obj(Some(&v));
                let id = text(row.get("id"));
                if !id.is_empty() {
                    index.insert(format!("missions:{id}"), row.clone());
                }
            }
        }
    }
    let (mut statuses, mut mission_statuses) = (BTreeMap::new(), BTreeMap::new());
    let (mut missing, mut mission_missing, mut mission_unique) = (0, 0, 0);
    for c in &citations {
        if let Some(hit) = index.get(&format!("{}:{}", c.tracker, c.id)) {
            let status = default(hit, "status", "?");
            *statuses.entry(status.clone()).or_default() += 1;
            if c.tracker == "missions" {
                *mission_statuses.entry(status).or_default() += 1;
            }
        } else if c.tracker == "missions" {
            missing += 1;
            mission_missing += 1;
        } else {
            *statuses.entry("unresolved".into()).or_default() += 1;
        }
        if c.tracker == "missions" {
            mission_unique += 1;
        }
    }
    let mut sorted = citations.clone();
    sorted.sort_by_key(|c| std::cmp::Reverse(c.count));
    let mut top = Vec::new();
    let mut mission_top = Vec::new();
    for c in &sorted {
        let hit = index.get(&format!("{}:{}", c.tracker, c.id));
        if top.len() < 8 {
            let status = hit.map_or_else(
                || {
                    if c.tracker == "missions" {
                        "MISSING".into()
                    } else {
                        "unresolved".into()
                    }
                },
                |h| default(h, "status", "?"),
            );
            let title = hit.map(|h| text(h.get("title"))).unwrap_or_default();
            let id = if c.tracker == "gh" {
                format!("gh#{}", c.id.strip_prefix('#').unwrap_or(&c.id))
            } else if c.tracker != "missions" {
                format!("{}:{}", c.tracker, c.id)
            } else {
                c.id.clone()
            };
            top.push(Value::Object(obj([
                ("id", id.into()),
                ("tracker", c.tracker.clone().into()),
                ("count", n(c.count)),
                ("status", status.into()),
                ("title", prefix_bytes(&title, 55)),
            ])));
        }
        if c.tracker == "missions" && mission_top.len() < 8 {
            let status = hit.map_or_else(|| "MISSING".into(), |h| default(h, "status", "MISSING"));
            let title = hit.map_or_else(|| "MISSING".into(), |h| default(h, "title", "MISSING"));
            mission_top.push(Value::Object(obj([
                ("id", c.id.clone().into()),
                ("count", n(c.count)),
                ("status", status.into()),
                ("title", prefix_bytes(&title, 55)),
            ])));
        }
    }
    let quality = if conv_total > 0 {
        Value::Object(obj([
            ("total", n(conv_total)),
            ("with_rule_signal", n(conv_signal)),
            ("likely_code_restatement", n(conv_total - conv_signal)),
        ]))
    } else {
        Value::Null
    };
    let mut groups: Vec<(String, Vec<&Object>)> = Vec::new();
    for (d, r) in &items {
        if let Some((_, rows)) = groups.iter_mut().find(|(name, _)| name == d) {
            rows.push(r)
        } else {
            groups.push((d.clone(), vec![r]))
        }
    }
    let mut by_domain = Vec::new();
    for (d, rows) in &groups {
        let mut counts = BTreeMap::new();
        let (mut first, mut last) = (String::new(), String::new());
        for r in rows {
            *counts.entry(text(r.get("type"))).or_default() += 1;
            let ts = text(r.get("recorded_at"));
            if !ts.is_empty() {
                if first.is_empty() || ts < first {
                    first = ts.clone()
                }
                if last.is_empty() || ts > last {
                    last = ts
                }
            }
        }
        let high = 100
            * (counts.get("failure").copied().unwrap_or(0)
                + counts.get("decision").copied().unwrap_or(0))
            / rows.len();
        by_domain.push(obj([
            ("domain", d.clone().into()),
            ("total", n(rows.len())),
            ("type_counts", count(&counts).into()),
            ("high_value_pct", n(high)),
            (
                "first_recorded_at",
                if first.is_empty() {
                    Value::Null
                } else {
                    first.into()
                },
            ),
            (
                "last_recorded_at",
                if last.is_empty() {
                    Value::Null
                } else {
                    last.into()
                },
            ),
        ]));
    }
    by_domain.sort_by_key(|r| std::cmp::Reverse(records::number(r.get("total")) as usize));
    let mut weak = Vec::new();
    let (mut max_records, mut max_stale) = (Vec::new(), Vec::new());
    let shelf = records::obj(records::obj(cfg.get("classification_defaults")).get("shelf_life"));
    for row in &by_domain {
        let d = text(row.get("domain"));
        if records::number(row.get("total")) >= 5.0
            && records::number(row.get("high_value_pct")) < 20.0
        {
            weak.push(d.clone())
        }
        let threshold = records::obj(per.get(&d));
        let rv = if records::number(row.get("total"))
            > records::number(threshold.get("max_records_per_domain"))
        {
            "WARN"
        } else {
            "PASS"
        };
        let stale = groups
            .iter()
            .find(|(name, _)| name == &d)
            .unwrap()
            .1
            .iter()
            .filter(|r| health::is_record_stale(r, chrono::Utc::now(), shelf))
            .count();
        let sv = if stale as f64 > records::number(threshold.get("max_stale")) {
            "WARN"
        } else {
            "PASS"
        };
        max_records.push(Value::Object(obj([
            ("domain", d.clone().into()),
            ("count", row.get("total").unwrap().clone()),
            ("verdict", rv.into()),
        ])));
        max_stale.push(Value::Object(obj([
            ("domain", d.into()),
            ("count", n(stale)),
            ("verdict", sv.into()),
        ])));
    }
    let ev_rate = if total > 0 {
        with_any as f64 / total as f64
    } else {
        0.0
    };
    let floater_rate = if total > 0 {
        floaters as f64 / total as f64
    } else {
        0.0
    };
    let ev_metric = metric(
        ev_rate,
        records::number(thresholds.get("evidence_coverage")),
        records::number(thresholds.get("evidence_coverage_warn")),
        false,
    );
    let floater_metric = metric(
        floater_rate,
        records::number(thresholds.get("floater_max")),
        2.0 * records::number(thresholds.get("floater_max")),
        true,
    );
    let rule_metric = if conv_total > 0 {
        Value::Object(metric(
            conv_signal as f64 / conv_total as f64,
            records::number(thresholds.get("rule_density_min")),
            records::number(thresholds.get("rule_density_warn")),
            false,
        ))
    } else {
        Value::Null
    };
    let (mut failures, mut warnings) = (Vec::new(), Vec::new());
    for (name, m) in [
        ("evidence_coverage", &ev_metric),
        ("rule_density", records::obj(Some(&rule_metric))),
        ("floater_rate", &floater_metric),
    ] {
        match text(m.get("verdict")).as_str() {
            "FAIL" => failures.push(name.into()),
            "WARN" => warnings.push(name.into()),
            _ => (),
        }
    }
    for (key, rows) in [
        ("max_records_per_domain", &max_records),
        ("max_stale", &max_stale),
    ] {
        for row in rows {
            let row = records::obj(Some(row));
            if text(row.get("verdict")) == "WARN" {
                warnings.push(format!("{key}[{}]", text(row.get("domain"))))
            }
        }
    }
    let report = obj([
        (
            "repo",
            a.ctx
                .cwd
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
                .into(),
        ),
        ("total_records", n(total)),
        ("domains", strings(domains)),
        (
            "ignored_domains",
            Value::Array(ignored.into_iter().map(Value::from_go_bytes).collect()),
        ),
        ("type_mix", Value::Array(mix)),
        (
            "evidence",
            obj([
                ("with_missions", n(with_tracker["missions"])),
                ("with_tracker", count(&with_tracker).into()),
                ("with_any_tracker", n(with_any)),
                ("with_commit", n(with_commit)),
                ("with_relates", n(with_relates)),
                ("floaters", n(floaters)),
            ])
            .into(),
        ),
        ("convention_quality", quality),
        (
            "mission_citations",
            obj([
                ("unique", n(mission_unique)),
                ("status_counts", count(&mission_statuses).into()),
                ("missing_in_missions", n(mission_missing)),
                ("top_cited", Value::Array(mission_top)),
            ])
            .into(),
        ),
        (
            "tracker_citations",
            obj([
                ("unique", n(citations.len())),
                ("status_counts", count(&statuses).into()),
                ("missing_in_index", n(missing)),
                ("top_cited", Value::Array(top)),
            ])
            .into(),
        ),
        (
            "by_domain",
            Value::Array(by_domain.into_iter().map(Value::Object).collect()),
        ),
        ("weak_domains", strings(weak)),
        ("thresholds", thresholds.into()),
        ("per_domain_thresholds", per.into()),
        (
            "signals",
            obj([
                ("evidence_coverage", ev_metric.into()),
                ("rule_density", rule_metric),
                ("floater_rate", floater_metric.into()),
                ("per_domain_max_records", Value::Array(max_records)),
                ("per_domain_max_stale", Value::Array(max_stale)),
            ])
            .into(),
        ),
        ("failures", strings(failures)),
        ("warnings", strings(warnings)),
    ]);
    Ok((report, items))
}

fn suggestions(items: &[Item], cfg: &Config) -> Object {
    let (mut archive, mut revise, mut attribute) = (Vec::new(), Vec::new(), Vec::new());
    let shelf = records::obj(records::obj(cfg.get("classification_defaults")).get("shelf_life"));
    for (_, r) in items {
        let id = text(r.get("id"));
        if id.is_empty() {
            continue;
        }
        if text(r.get("classification")) != "foundational"
            && health::is_record_stale(r, chrono::Utc::now(), shelf)
        {
            archive.push(id.clone())
        }
        if text(r.get("type")) == "convention" && !records::has_rule_signal(&text(r.get("content")))
        {
            revise.push(id.clone())
        }
        if floater(r) {
            attribute.push(id)
        }
    }
    let mut groups = Vec::new();
    for(action,ids,headline,rationale,commands)in [
 ("archive",archive.clone(),format!("{} stale records past shelf life",archive.len()),"These records are past their classification shelf life and are not foundational. Soft-archive them with `luuna-atlas prune` (or `luuna-atlas archive` once it lands in v0.10).",vec![format!("luuna-atlas prune --ids {} --dry-run",archive.join(",")),format!("luuna-atlas prune --ids {}",archive.join(","))]),
 ("revise",revise.clone(),format!("{} conventions lack rule-signal language",revise.len()),"Conventions without rule-signal words (because, must not, avoid, always, never, …) often restate code rather than encode a rule. Revise the content or downgrade to a note.",revise.iter().take(10).map(|id|format!("luuna-atlas edit {id} --content \"...\"")).collect()),
 ("attribute",attribute.clone(),format!("{} records have no evidence or relates_to link",attribute.len()),"Floater records lack any tracker (missions/gh/linear/bead), commit, or relates_to link. Attribute them so future audits and pruning can follow the trail.",attribute.iter().take(10).map(|id|format!("luuna-atlas edit {id} --evidence-missions <id>   # or --evidence-commit <sha> / --relates-to <mx-id>")).collect())]{if !ids.is_empty(){groups.push(Value::Object(obj([("action",action.into()),("headline",headline.into()),("rationale",rationale.into()),("record_ids",strings(ids)),("commands",strings(commands))])));}}
    obj([("groups", Value::Array(groups))])
}
fn number(o: &Object, k: &str) -> f64 {
    records::number(o.get(k))
}
fn integer(o: &Object, k: &str) -> i64 {
    number(o, k) as i64
}
fn pct(v: f64) -> String {
    format!("{}%", health::go_int((v * 100.0 + 0.5).floor()))
}
fn verdict(a: &Action<'_>, v: &str) -> String {
    a.styled(
        match v {
            "PASS" => "green",
            "WARN" => "yellow",
            _ => "red",
        },
        v,
    )
}
fn human(a: &Action<'_>, r: &Object) -> String {
    let mut lines = vec![
        a.styled(
            "brand",
            &a.styled(
                "bold",
                &format!("=== atlas-audit: {} ===", text(r.get("repo"))),
            ),
        ),
        String::new(),
        format!("total records: {}", integer(r, "total_records")),
    ];
    let ignored = records::list(r.get("ignored_domains"));
    if !ignored.is_empty() {
        lines.push(a.styled(
            "muted",
            &format!("(ignoring domains: {})", ignored.join(", ")),
        ))
    }
    let total = integer(r, "total_records");
    if total == 0 {
        lines.extend([
            String::new(),
            a.styled("muted", "no records — skipping metric computation"),
        ]);
        return lines.join("\n");
    }
    lines.extend([String::new(), "type mix:".into()]);
    for v in records::any_list(r.get("type_mix")) {
        let row = records::obj(Some(v));
        lines.push(format!(
            "  {}: {} ({}%)",
            text(row.get("type")),
            integer(row, "count"),
            integer(row, "pct")
        ));
    }
    let ev = records::obj(r.get("evidence"));
    lines.extend([
        String::new(),
        "evidence coverage:".into(),
        format!(
            "  with any tracker: {} ({}%)",
            integer(ev, "with_any_tracker"),
            integer(ev, "with_any_tracker") * 100 / total
        ),
    ]);
    let counts = records::obj(ev.get("with_tracker"));
    for t in TRACKERS {
        if number(counts, t) > 0.0 {
            lines.push(format!(
                "  - {t}: {} ({}%)",
                integer(counts, t),
                integer(counts, t) * 100 / total
            ));
        }
    }
    lines.extend([
        format!(
            "  with commit-ev: {} ({}%)",
            integer(ev, "with_commit"),
            integer(ev, "with_commit") * 100 / total
        ),
        format!("  with relates_to: {}", integer(ev, "with_relates")),
        format!(
            "  FLOATERS (no tracker/relates/commit): {}",
            integer(ev, "floaters")
        ),
    ]);
    let refs = records::obj(r.get("tracker_citations"));
    if number(refs, "unique") > 0.0 {
        lines.extend([
            String::new(),
            format!(
                "tracker-citation (unique ids cited: {}):",
                integer(refs, "unique")
            ),
        ]);
        let statuses = records::obj(refs.get("status_counts"));
        let mut keys: Vec<_> = statuses.keys().collect();
        keys.sort_by_key(|k| std::cmp::Reverse(integer(statuses, k)));
        for k in keys {
            lines.push(format!("  {k}: {}", integer(statuses, k)));
        }
        if number(refs, "missing_in_index") > 0.0 {
            lines.push(format!(
                "  not-found-in-missions.jsonl: {}",
                integer(refs, "missing_in_index")
            ));
        }
        lines.push(format!(
            "  closed-citation rate: {}%",
            (number(statuses, "closed") * 100.0 / number(refs, "unique")) as i64
        ));
    }
    let quality = records::obj(r.get("convention_quality"));
    if !quality.is_empty() {
        let rate = (number(quality, "with_rule_signal") * 100.0 / number(quality, "total")) as i64;
        lines.extend([
            String::new(),
            "convention quality:".into(),
            format!("  total conventions: {}", integer(quality, "total")),
            format!(
                "  with rule-signal word: {} ({rate}%)",
                integer(quality, "with_rule_signal")
            ),
            format!(
                "  likely code-restatement: {} ({}%)",
                integer(quality, "likely_code_restatement"),
                100 - rate
            ),
        ]);
    }
    lines.extend([
        String::new(),
        "per-domain mix (sorted by record count):".into(),
        format!(
            "  {:22} {:>5} {:>4} {:>4} {:>4} {:>4}  high-value%",
            "domain", "total", "fail", "dec", "pat", "conv"
        ),
    ]);
    let by = records::any_list(r.get("by_domain"));
    for d in by {
        let d = records::obj(Some(d));
        let c = records::obj(d.get("type_counts"));
        lines.push(format!(
            "  {:22} {:5} {:4} {:4} {:4} {:4}  {:3}%",
            text(d.get("domain")),
            integer(d, "total"),
            integer(c, "failure"),
            integer(c, "decision"),
            integer(c, "pattern"),
            integer(c, "convention"),
            integer(d, "high_value_pct")
        ));
    }
    let cited = records::any_list(refs.get("top_cited"));
    if !cited.is_empty() {
        lines.extend([String::new(), "top-cited tracker ids:".into()]);
        for v in cited {
            let row = records::obj(Some(v));
            lines.push(format!(
                "  {} cited={} status={}  {}",
                text(row.get("id")),
                integer(row, "count"),
                text(row.get("status")),
                text(row.get("title"))
            ));
        }
    }
    let mut dated: Vec<_> = by.iter().map(|v| records::obj(Some(v))).collect();
    dated.sort_by_key(|d| text(d.get("domain")));
    let mut has_date = false;
    for d in dated {
        let first = text(d.get("first_recorded_at"));
        if first.is_empty() {
            continue;
        }
        if !has_date {
            lines.extend([
                String::new(),
                "domain age (first → last recorded_at):".into(),
            ]);
            has_date = true
        }
        let last = text(d.get("last_recorded_at"));
        let first = text(Some(&prefix_bytes(&first, 10)));
        let last = text(Some(&prefix_bytes(&last, 10)));
        lines.push(format!(
            "  {:22} {first} → {last}  ({} records)",
            text(d.get("domain")),
            integer(d, "total")
        ));
    }
    let signals = records::obj(r.get("signals"));
    lines.extend([String::new(), a.styled("brand", "--- summary signals ---")]);
    for (key, label, op) in [
        ("evidence_coverage", "evidence coverage", "≥"),
        ("rule_density", "convention rule-density", "≥"),
        ("floater_rate", "floater rate", "≤"),
    ] {
        let m = records::obj(signals.get(key));
        if !m.is_empty() {
            lines.push(format!(
                "{label}: {}  {} (target {op}{})",
                pct(number(m, "value")),
                verdict(a, &text(m.get("verdict"))),
                pct(number(m, "threshold"))
            ));
        }
    }
    for (key, label) in [
        ("per_domain_max_records", "max_records_per_domain"),
        ("per_domain_max_stale", "max_stale"),
    ] {
        let parts: Vec<_> = records::any_list(signals.get(key))
            .iter()
            .map(|v| records::obj(Some(v)))
            .filter(|r| text(r.get("verdict")) != "PASS")
            .map(|r| format!("{}={}", text(r.get("domain")), text(r.get("count"))))
            .collect();
        if !parts.is_empty() {
            lines.push(format!(
                "{label}: {} over ({})  {}",
                parts.len(),
                parts.join(", "),
                verdict(a, "WARN")
            ));
        }
    }
    let weak = records::list(r.get("weak_domains"));
    lines.push(format!(
        "weak-domains (≥5 records, high-value <20%): {}  -> {}",
        weak.len(),
        json::to_compact_string(&strings(weak)).unwrap_or_default()
    ));
    let failures = records::list(r.get("failures"));
    let warnings = records::list(r.get("warnings"));
    lines.push(String::new());
    if failures.is_empty() && warnings.is_empty() {
        lines.push(a.styled("green", "✓ all signals PASS"))
    } else {
        if !failures.is_empty() {
            lines.push(format!(
                "{} {}",
                a.styled("red", "✗ FAIL:"),
                failures.join(", ")
            ))
        }
        if !warnings.is_empty() {
            lines.push(format!(
                "{} {}",
                a.styled("yellow", "! WARN:"),
                warnings.join(", ")
            ))
        }
    }
    lines.join("\n")
}
fn suggest_human(a: &Action<'_>, payload: &Object) -> String {
    let groups = records::any_list(payload.get("groups"));
    if groups.is_empty() {
        return a.styled("green", "✓ no remediation suggestions — corpus looks clean");
    }
    let mut lines = Vec::new();
    for v in groups {
        let g = records::obj(Some(v));
        lines.extend([
            String::new(),
            a.styled(
                "brand",
                &a.styled(
                    "bold",
                    &format!(
                        "[{}] {}",
                        text(g.get("action")).to_uppercase(),
                        text(g.get("headline"))
                    ),
                ),
            ),
            a.styled("muted", &text(g.get("rationale"))),
            String::new(),
            "  record ids:".into(),
        ]);
        let ids = records::list(g.get("record_ids"));
        let preview = &ids[..ids.len().min(12)];
        let tail = if ids.len() > preview.len() {
            format!(" … (+{} more)", ids.len() - preview.len())
        } else {
            String::new()
        };
        lines.extend([
            format!("    {}{tail}", preview.join(", ")),
            String::new(),
            "  copy-paste commands:".into(),
        ]);
        for c in records::list(g.get("commands")) {
            lines.push(format!("    {}", a.styled("cyan", &c)));
        }
    }
    lines.join("\n")
}
pub(crate) fn run(a: &mut Action<'_>, flags: &[(String, String)]) -> Result<()> {
    let json_mode = a.ctx.globals.json || boolean(flags, "ci");
    if !a.ctx.root.join(".luuna/atlas").exists() {
        if json_mode {
            a.output.stderr += &crate::CommandOutput::failure(
                "audit",
                "No .luuna/atlas/ directory found. Run `luuna-atlas init` first.",
            )
            .stderr;
            a.output.exit_code = 1;
            return Ok(());
        }
        return Err(message(
            "No .luuna/atlas/ directory found. Run `luuna-atlas init` first.",
        ));
    }
    let cfg = a.config()?;
    let domain = get(flags, "domain");
    if !domain.is_empty() && !records::obj(cfg.get("domains")).contains_key(&domain) {
        let error = format!("Domain {} not found in config.", records::go_quote(&domain));
        if json_mode {
            a.output.stderr += &crate::CommandOutput::failure("audit", &error).stderr;
            a.output.exit_code = 1;
            return Ok(());
        }
        return Err(message(error));
    }
    let (report, items) = compute(a, &cfg, flags)?;
    let suggestions = boolean(flags, "suggest").then(|| suggestions(&items, &cfg));
    if json_mode {
        let success = !boolean(flags, "ci") || records::list(report.get("failures")).is_empty();
        let mut out = obj([
            ("success", success.into()),
            ("command", "audit".into()),
            ("report", report.into()),
        ]);
        if let Some(s) = suggestions {
            out.insert_unordered("suggestions", s);
        }
        a.output.stdout += &crate::CommandOutput::emit(&out).stdout;
        if !success {
            a.output.exit_code = 1;
        }
    } else {
        a.output.stdout += &format!("{}\n", human(a, &report));
        if let Some(s) = suggestions {
            a.output.stdout += &format!(
                "\n{}\n{}\n",
                a.styled("brand", &a.styled("bold", "=== suggested remediations ===")),
                suggest_human(a, &s)
            );
        }
    }
    Ok(())
}
