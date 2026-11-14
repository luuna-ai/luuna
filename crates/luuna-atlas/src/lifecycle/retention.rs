//! Shared retention analysis. Graph traversals use explicit stacks.
use super::health::{date, go_int, hours, is_record_stale, object, zero_time};
use crate::{
    actions::record::{domain_names, message, text, Action},
    git,
    json::{Object, Value},
    options::Context,
    records, store, timefmt, Result,
};
use chrono::{DateTime, Utc};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};
#[derive(Clone, Debug)]
pub struct Anchor {
    pub kind: String,
    pub path: String,
}
impl Anchor {
    pub fn value(&self) -> Value {
        let mut out = Object::new();
        out.insert("kind", self.kind.clone());
        out.insert("path", self.path.clone());
        out.into()
    }
}
#[derive(Clone, Debug)]
pub struct AnchorValidity {
    pub total: usize,
    pub valid: usize,
    pub broken: Vec<Anchor>,
    pub valid_fraction: Option<f64>,
}
pub fn record_anchors(r: &Object) -> Vec<Anchor> {
    let mut out = Vec::new();
    for (field, kind) in [("files", "file"), ("dir_anchors", "dir")] {
        out.extend(
            records::list(r.get(field))
                .into_iter()
                .filter(|p| !p.is_empty())
                .map(|path| Anchor {
                    kind: kind.into(),
                    path,
                }),
        );
    }
    let path = text(records::obj(r.get("evidence")).get("file"));
    if !path.is_empty() {
        out.push(Anchor {
            kind: "evidence_file".into(),
            path,
        });
    }
    out
}
pub fn anchor_validity(r: &Object, cwd: &Path) -> AnchorValidity {
    let anchors = record_anchors(r);
    let total = anchors.len();
    let broken = anchors
        .into_iter()
        .filter(|a| !cwd.join(&a.path).exists())
        .collect::<Vec<_>>();
    let valid = total - broken.len();
    AnchorValidity {
        total,
        valid,
        broken,
        valid_fraction: if total == 0 {
            None
        } else {
            Some(valid as f64 / total as f64)
        },
    }
}
pub fn passed_anchor_grace(r: &Object, now: DateTime<Utc>, grace: f64) -> bool {
    let at = date(&text(r.get("recorded_at")));
    at != zero_time() && hours(now, at) / 24.0 > grace
}
/// Strong components via iterative Kosaraju; self edges are ignored by Go.
pub fn collect_superseded_ids(
    domains: &[(String, Vec<Object>)],
) -> (BTreeSet<String>, BTreeSet<String>) {
    let mut graph: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut reverse: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut edges = Vec::new();
    for (_, rows) in domains {
        for r in rows {
            let id = text(r.get("id"));
            if id.is_empty() {
                continue;
            }
            for target in records::list(r.get("supersedes")) {
                if target == id {
                    continue;
                }
                graph.entry(id.clone()).or_default().push(target.clone());
                graph.entry(target.clone()).or_default();
                reverse.entry(target.clone()).or_default().push(id.clone());
                edges.push((id.clone(), target));
            }
        }
    }
    let mut seen = BTreeSet::new();
    let mut order = Vec::new();
    for root in graph.keys() {
        if !seen.insert(root.clone()) {
            continue;
        }
        let mut stack = vec![(root.clone(), 0)];
        while let Some((node, index)) = stack.last_mut() {
            let children = &graph[node];
            if *index < children.len() {
                let child = children[*index].clone();
                *index += 1;
                if seen.insert(child.clone()) {
                    stack.push((child, 0));
                }
            } else {
                order.push(node.clone());
                stack.pop();
            }
        }
    }
    seen.clear();
    let mut cycles = BTreeSet::new();
    for root in order.into_iter().rev() {
        if !seen.insert(root.clone()) {
            continue;
        }
        let mut component = Vec::new();
        let mut stack = vec![root];
        while let Some(node) = stack.pop() {
            component.push(node.clone());
            for child in reverse.get(&node).into_iter().flatten() {
                if seen.insert(child.clone()) {
                    stack.push(child.clone());
                }
            }
        }
        if component.len() > 1 {
            cycles.extend(component);
        }
    }
    let superseded = edges
        .into_iter()
        .filter_map(|(_, target)| {
            if cycles.contains(&target) {
                None
            } else {
                Some(target)
            }
        })
        .collect();
    (superseded, cycles)
}
#[derive(Clone, Debug)]
pub struct StaleReason {
    pub kind: String,
    pub detail: String,
}
impl StaleReason {
    pub fn value(&self) -> Value {
        let mut out = Object::new();
        out.insert("kind", self.kind.clone());
        out.insert("detail", self.detail.clone());
        out.into()
    }
}
#[derive(Clone, Debug)]
pub struct StaleEntry {
    pub record: Object,
    pub reasons: Vec<StaleReason>,
}
pub fn find_stale_records(
    rows: &[Object],
    ctx: &Context,
    now: DateTime<Utc>,
    shelf: &Object,
) -> Vec<StaleEntry> {
    let mut order = Vec::new();
    let mut reasons: BTreeMap<String, Vec<StaleReason>> = BTreeMap::new();
    let mut by_id = BTreeMap::new();
    let mut candidates = Vec::new();
    for r in rows {
        let id = text(r.get("id"));
        if id.is_empty() {
            continue;
        }
        by_id.insert(id.clone(), r);
        let valid = anchor_validity(r, &ctx.cwd);
        let mut local = Vec::new();
        if !valid.broken.is_empty() {
            local.push(StaleReason {
                kind: "anchors_missing".into(),
                detail: format!(
                    "missing anchors: {}",
                    valid
                        .broken
                        .iter()
                        .map(|a| a.path.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            });
        }
        if is_record_stale(r, now, shelf) && records::confirmation_score(r) == 0.0 {
            let age = go_int((hours(now, date(&text(r.get("recorded_at")))) / 24.0).floor());
            local.push(StaleReason {
                kind: "shelf_life".into(),
                detail: format!(
                    "{}, {age}d old, never confirmed",
                    text(r.get("classification"))
                ),
            });
        }
        if !local.is_empty() {
            if !reasons.contains_key(&id) {
                order.push(id.clone());
            }
            reasons.insert(id, local);
        }
        if !text(records::obj(r.get("evidence")).get("commit")).is_empty()
            && valid.broken.is_empty()
            && valid.total > 0
        {
            candidates.push(r);
        }
    }
    if !candidates.is_empty() {
        let mut oldest = now;
        let mut paths = Vec::new();
        for r in &candidates {
            let at = date(&text(r.get("recorded_at")));
            if at != zero_time() && at < oldest {
                oldest = at;
            }
            paths.extend(record_anchors(r).into_iter().map(|a| a.path));
        }
        let since = format!(
            "--since={}",
            timefmt::iso(oldest - chrono::Duration::days(1))
        );
        let history = git::git(&ctx.cwd, &ctx.env, &["log", "--format=%H", &since]);
        let mut args = vec![
            "log".into(),
            "--no-renames".into(),
            "--relative".into(),
            "--name-only".into(),
            "--format=%H".into(),
            since,
            "--".into(),
        ];
        args.extend(records::unique(&paths));
        let raw = git::git(
            &ctx.cwd,
            &ctx.env,
            &args.iter().map(String::as_str).collect::<Vec<_>>(),
        );
        if let (Ok(history), Ok(raw)) = (history, raw) {
            let commits = history.split_whitespace().collect::<Vec<_>>();
            let mut changes: BTreeMap<String, Vec<String>> = BTreeMap::new();
            let mut current = String::new();
            for line in raw.split('\n') {
                if line.is_empty() {
                    continue;
                }
                if line.len() == 40
                    && line
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                {
                    current = line.into();
                    changes.insert(current.clone(), Vec::new());
                } else if !current.is_empty() {
                    changes
                        .entry(current.clone())
                        .or_default()
                        .push(line.into());
                }
            }
            for r in candidates {
                let sha = text(records::obj(r.get("evidence")).get("commit"));
                let Some(index) = commits.iter().position(|h| h.starts_with(&sha)) else {
                    continue;
                };
                let anchors = record_anchors(r);
                let mut touches = Vec::new();
                let mut count = 0;
                for commit in &commits[..index] {
                    let hits = changes
                        .get(*commit)
                        .into_iter()
                        .flatten()
                        .filter(|file| {
                            anchors.iter().any(|a| {
                                let anchor = a.path.strip_prefix("./").unwrap_or(&a.path);
                                if a.kind == "dir" {
                                    git::file_lives_under_dir(file, anchor)
                                } else {
                                    file.as_str() == anchor
                                }
                            })
                        })
                        .cloned()
                        .collect::<Vec<_>>();
                    if !hits.is_empty() {
                        count += 1;
                        touches.extend(hits);
                        touches = records::unique(&touches);
                    }
                }
                if count >= 5 {
                    let id = text(r.get("id"));
                    if !reasons.contains_key(&id) {
                        order.push(id.clone());
                    }
                    let first = touches.first().cloned().unwrap_or_default();
                    let more = if touches.len() > 1 {
                        format!(" (+{} more)", touches.len() - 1)
                    } else {
                        String::new()
                    };
                    let short = String::from_utf8_lossy(&sha.as_bytes()[..sha.len().min(7)]);
                    reasons.entry(id).or_default().push(StaleReason {
                        kind: "anchors_changed".into(),
                        detail: format!("{first}{more} changed in {count} commits since {short}"),
                    });
                }
            }
        }
    }
    order
        .into_iter()
        .map(|id| StaleEntry {
            record: by_id[&id].clone(),
            reasons: reasons.remove(&id).unwrap(),
        })
        .collect()
}
pub fn prime_stale_ids(entries: &[StaleEntry]) -> BTreeSet<String> {
    entries
        .iter()
        .filter(|e| {
            e.reasons
                .iter()
                .any(|r| r.kind == "anchors_changed" || r.kind == "anchors_missing")
        })
        .map(|e| text(e.record.get("id")))
        .collect()
}
pub(crate) fn run_stale(a: &mut Action<'_>, args: &[String]) -> Result<()> {
    let cfg = a.config()?;
    let domains = if args.is_empty() {
        domain_names(&cfg)
    } else {
        args.to_vec()
    };
    let mut loaded = Vec::new();
    for domain in domains {
        if !records::obj(cfg.get("domains")).contains_key(&domain) {
            return Err(message(format!(
                "Domain {} not found in config.",
                records::go_quote(&domain)
            )));
        }
        let rows = store::read_records(
            &a.path(&domain)?,
            &a.registry,
            a.ctx.globals.allow_unknown_types,
        )?;
        loaded.push((domain, rows));
    }
    let (superseded, _) = collect_superseded_ids(&loaded);
    let all = loaded
        .iter()
        .flat_map(|(_, rows)| rows)
        .filter(|r| !superseded.contains(&text(r.get("id"))))
        .cloned()
        .collect::<Vec<_>>();
    let shelf = records::obj(records::obj(cfg.get("classification_defaults")).get("shelf_life"));
    let reasons = find_stale_records(&all, a.ctx, Utc::now(), shelf)
        .into_iter()
        .map(|e| (text(e.record.get("id")), e.reasons))
        .collect::<BTreeMap<_, _>>();
    let mut rows = Vec::new();
    for (domain, records) in loaded {
        for r in records {
            let id = text(r.get("id"));
            if let Some(reasons) = reasons
                .get(&id)
                .filter(|r| !r.is_empty() && !id.is_empty() && !superseded.contains(&id))
            {
                rows.push(object([
                    ("id", id.into()),
                    ("domain", domain.clone().into()),
                    ("type", r.get("type").cloned().unwrap_or(Value::Null)),
                    (
                        "reasons",
                        Value::Array(reasons.iter().map(StaleReason::value).collect()),
                    ),
                ]));
            }
        }
    }
    if a.ctx.globals.json {
        a.json(
            "stale",
            object([(
                "records",
                Value::Array(rows.into_iter().map(Value::Object).collect()),
            )]),
        );
        return Ok(());
    }
    if rows.is_empty() {
        a.human("No stale records.");
        return Ok(());
    }
    for r in &rows {
        let details = r
            .get("reasons")
            .and_then(Value::as_array)
            .unwrap()
            .iter()
            .map(|v| text(records::obj(Some(v)).get("detail")))
            .collect::<Vec<_>>()
            .join("; ");
        a.output.stdout.push_str(&format!(
            "{} ({}, {}) {details}\n",
            text(r.get("id")),
            text(r.get("domain")),
            text(r.get("type"))
        ));
    }
    let tick = char::from(96);
    a.output.stdout.push_str(&format!("\n{} stale record(s). Check with {tick}luuna-atlas show <id>{tick}; refresh via {tick}luuna-atlas edit{tick}, confirm via {tick}luuna-atlas outcome{tick}, or replace with {tick}luuna-atlas record --supersedes <id>{tick}.\n",rows.len()));
    Ok(())
}
