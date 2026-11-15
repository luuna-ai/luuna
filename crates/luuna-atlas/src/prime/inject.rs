//! File-anchored injection with explicit registry, clock and session state.
use super::{command::tokens, guard};
use crate::{
    actions::{record::text, render},
    config, git,
    json::{self, Object, Value},
    legacy,
    lifecycle::{
        health::{date, zero_time},
        retention::{collect_superseded_ids, find_stale_records, prime_stale_ids},
    },
    options::Context,
    records,
    registry::Registry,
    store, timefmt, AtlasError, Result,
};
use chrono::{DateTime, Utc};
use std::{
    collections::BTreeSet,
    io::Write,
    path::{Path, PathBuf},
};
pub const HOOK_BUDGET: usize = 500;
pub struct InjectRequest<'a> {
    pub file: &'a str,
    pub cwd: &'a Path,
    pub session: &'a str,
    pub tool: &'a str,
    pub now: Option<DateTime<Utc>>,
    pub registry: Option<&'a Registry>,
    pub allow_unknown_types: bool,
}
pub fn session_file(dir: &Path, session: &str) -> PathBuf {
    dir.join("sessions").join(format!(
        "{}.txt",
        session
            .chars()
            .map(
                |ch| if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
                    ch
                } else {
                    '_'
                }
            )
            .collect::<String>()
    ))
}
fn path_error(op: &'static str, path: &Path, source: std::io::Error) -> AtlasError {
    AtlasError::PathOp {
        op,
        path: path.to_owned(),
        source,
    }
}
fn now(req: &InjectRequest<'_>) -> DateTime<Utc> {
    req.now
        .filter(|t| *t != zero_time())
        .unwrap_or_else(Utc::now)
}
fn append(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| path_error("open", path, e))?;
    file.write_all(bytes)
        .map_err(|e| path_error("write", path, e))?;
    Ok(())
}
pub fn record_injection(
    dir: &Path,
    req: &InjectRequest<'_>,
    rel: &str,
    ids: &[String],
) -> Result<()> {
    let sessions = dir.join("sessions");
    std::fs::create_dir_all(&sessions).map_err(|e| path_error("mkdir", &sessions, e))?;
    let ignore = dir.join(".gitignore");
    if !ignore.exists() {
        std::fs::write(&ignore, b"*\n").map_err(|e| path_error("open", &ignore, e))?;
    }
    let now = now(req);
    if !req.session.is_empty() {
        let path = session_file(dir, req.session);
        if !path.exists() {
            let mut entries = std::fs::read_dir(&sessions)
                .map_err(|e| path_error("readdir", &sessions, e))?
                .collect::<std::io::Result<Vec<_>>>()
                .map_err(|e| path_error("readdir", &sessions, e))?;
            entries.sort_by_key(|e| e.file_name());
            for entry in entries {
                let p = entry.path();
                let metadata = std::fs::metadata(&p).map_err(|e| path_error("stat", &p, e))?;
                let modified = metadata.modified().map_err(|e| path_error("stat", &p, e))?;
                if now.signed_duration_since(DateTime::<Utc>::from(modified))
                    > chrono::Duration::days(7)
                {
                    let is_link = std::fs::symlink_metadata(&p)
                        .map_err(|e| path_error("lstat", &p, e))?
                        .file_type()
                        .is_symlink();
                    if metadata.is_dir() && !is_link {
                        std::fs::remove_dir(&p)
                    } else {
                        std::fs::remove_file(&p)
                    }
                    .map_err(|e| path_error("remove", &p, e))?;
                }
            }
        }
        append(&path, format!("{}\n", ids.join("\n")).as_bytes())?;
    }
    let mut line = Object::new();
    line.insert("ts", timefmt::iso(now));
    line.insert(
        "session",
        if req.session.is_empty() {
            Value::Null
        } else {
            req.session.into()
        },
    );
    line.insert("tool", req.tool);
    line.insert("files", Value::Array(vec![rel.into()]));
    line.insert(
        "ids",
        Value::Array(ids.iter().cloned().map(Value::from).collect()),
    );
    let raw = json::to_compact_string(&line.into())?;
    append(&dir.join("usage.jsonl"), format!("{raw}\n").as_bytes())
}
pub fn hook_budget(
    domains: &[(String, Vec<Object>)],
    registry: &Registry,
    stale: &BTreeSet<String>,
) -> Vec<Object> {
    let original = domains
        .iter()
        .flat_map(|(_, rows)| rows.iter().cloned())
        .collect::<Vec<_>>();
    let mut ranked = (0..original.len()).collect::<Vec<_>>();
    let class = |r: &Object| match text(r.get("classification")).as_str() {
        "tactical" => 1,
        "observational" => 2,
        _ => 0,
    };
    ranked.sort_by(|i, j| {
        let (a, b) = (&original[*i], &original[*j]);
        let (af, bf) = (
            text(a.get("type")) == "failure",
            text(b.get("type")) == "failure",
        );
        if af != bf {
            return bf.cmp(&af);
        }
        let (as_, bs) = (
            stale.contains(&text(a.get("id"))),
            stale.contains(&text(b.get("id"))),
        );
        if as_ != bs {
            return as_.cmp(&bs);
        }
        let (ac, bc) = (
            records::confirmation_score(a),
            records::confirmation_score(b),
        );
        if ac != bc {
            return if ac > bc {
                std::cmp::Ordering::Less
            } else {
                std::cmp::Ordering::Greater
            };
        }
        let (at, bt) = (class(a), class(b));
        if at != bt {
            return at.cmp(&bt);
        }
        date(&text(b.get("recorded_at"))).cmp(&date(&text(a.get("recorded_at"))))
    });
    let mut keep = BTreeSet::new();
    let mut used = 0;
    for index in ranked {
        let r = &original[index];
        let cost = tokens(&render::format_record_index(r, registry, false));
        let pinned = text(r.get("type")) == "failure";
        if pinned || used + cost <= HOOK_BUDGET {
            keep.insert(index);
            used += cost;
        }
    }
    let mut out = original
        .into_iter()
        .enumerate()
        .filter(|(i, _)| keep.contains(i))
        .map(|(_, r)| r)
        .collect::<Vec<_>>();
    let rank = |r: &Object| {
        let n = if text(r.get("type")) == "failure" {
            0
        } else {
            1
        };
        n + if stale.contains(&text(r.get("id"))) {
            2
        } else {
            0
        }
    };
    out.sort_by_key(rank);
    out
}
fn repo_relative(file: &str, cwd: &Path, ctx: &Context) -> String {
    let root = git::find_repo_root(cwd, &ctx.env);
    let path = Path::new(file);
    let path = if path.is_absolute() {
        path.to_owned()
    } else {
        cwd.join(path)
    };
    let Ok(path) = std::path::absolute(path) else {
        return String::new();
    };
    let path = crate::commands::clean_path(&path);
    let Ok(rel) = path.strip_prefix(&root) else {
        return String::new();
    };
    let rel = rel.to_string_lossy().into_owned();
    if rel.is_empty() || rel == "." || rel.starts_with("..") || Path::new(&rel).is_absolute() {
        String::new()
    } else {
        rel
    }
}
pub fn inject_for_file(ctx: &Context, req: &InjectRequest<'_>) -> Result<String> {
    let cwd = if req.cwd.is_absolute() {
        req.cwd.to_owned()
    } else {
        ctx.cwd.join(req.cwd)
    };
    let request = InjectRequest {
        file: req.file,
        cwd: &cwd,
        session: req.session,
        tool: req.tool,
        now: req.now,
        registry: req.registry,
        allow_unknown_types: req.allow_unknown_types,
    };
    inject_absolute(ctx, &request)
}
fn inject_absolute(ctx: &Context, req: &InjectRequest<'_>) -> Result<String> {
    let root = git::resolve_store_root(req.cwd, &ctx.env);
    legacy::migrate_legacy_store(&root)?;
    let dir = root.join(".luuna/atlas");
    if !dir.join("config.yaml").exists() {
        return Ok(String::new());
    }
    let rel = repo_relative(req.file, req.cwd, ctx);
    if rel.is_empty() || rel.starts_with(".luuna/atlas/") {
        return Ok(String::new());
    }
    let cfg = config::read_config(&dir)?;
    let builtin = Registry::from_config(None)?;
    let registry = req.registry.unwrap_or(&builtin);
    let mut seen = BTreeSet::new();
    let state = dir.join("state");
    if !req.session.is_empty() {
        if let Ok(raw) = std::fs::read(session_file(&state, req.session)) {
            for id in raw.split(|b| *b == b'\n') {
                if !id.is_empty() {
                    seen.insert(id.to_vec());
                }
            }
        }
    }
    let mut all = Vec::new();
    for domain in crate::actions::record::domain_names(&cfg) {
        config::validate_domain_name(&domain)?;
        let path = dir.join("expertise").join(format!("{domain}.jsonl"));
        let rows = store::read_records(&path, registry, req.allow_unknown_types)?;
        all.push((domain, rows));
    }
    let (superseded, _) = collect_superseded_ids(&all);
    let mut scoped = Vec::new();
    let mut flat = Vec::new();
    for (domain, rows) in all {
        let rows = rows
            .into_iter()
            .filter(|r| {
                let id = text(r.get("id"));
                !id.is_empty()
                    && !superseded.contains(&id)
                    && !seen.contains(id.as_bytes())
                    && git::matches_file_anchors(r, std::slice::from_ref(&rel))
            })
            .collect::<Vec<_>>();
        if !rows.is_empty() {
            flat.extend(rows.clone());
            scoped.push((domain, rows));
        }
    }
    if scoped.is_empty() {
        return Ok(String::new());
    }
    let cwd = git::find_repo_root(req.cwd, &ctx.env);
    let local = Context {
        root,
        cwd,
        env: ctx.env.clone(),
        globals: ctx.globals.clone(),
    };
    let shelf = records::obj(records::obj(cfg.get("classification_defaults")).get("shelf_life"));
    let stale = prime_stale_ids(&find_stale_records(&flat, &local, now(req), shelf));
    let rows = hook_budget(&scoped, registry, &stale);
    if rows.is_empty() {
        return Ok(String::new());
    }
    let ids = rows.iter().map(|r| text(r.get("id"))).collect::<Vec<_>>();
    let lines = rows
        .iter()
        .map(|r| render::format_record_index(r, registry, stale.contains(&text(r.get("id")))))
        .collect::<Vec<_>>();
    record_injection(&state, req, &rel, &ids)?;
    let tick = char::from(96);
    Ok(format!("luuna-atlas: records anchored to {rel} ({tick}luuna-atlas show <id>{tick} for the full record)\n{}",lines.join("\n")))
}
pub fn hook_output(
    raw: &[u8],
    ctx: &Context,
    registry: Option<&Registry>,
    allow_unknown: bool,
) -> String {
    let Ok(input) = json::parse_bytes(raw) else {
        return String::new();
    };
    let input = records::obj(Some(&input));
    let event = text(input.get("hook_event_name"));
    if !event.is_empty() && event != "PreToolUse" {
        return String::new();
    }
    let cwd = text(input.get("cwd"));
    let cwd = if cwd.is_empty() {
        ctx.cwd.clone()
    } else {
        ctx.cwd.join(cwd)
    };
    let mut extra = Object::new();
    extra.insert("hookEventName", "PreToolUse");
    let reason = guard::guard_decision(input, &cwd);
    if !reason.is_empty() {
        extra.insert("permissionDecision", "deny");
        extra.insert("permissionDecisionReason", reason);
    } else {
        let tool = text(input.get("tool_name"));
        let key = if tool == "Read" {
            Some("file_path")
        } else {
            guard::file_tool_field(&tool)
        };
        let Some(key) = key else { return String::new() };
        let file = text(records::obj(input.get("tool_input")).get(key));
        if file.is_empty() {
            return String::new();
        }
        let session = text(input.get("session_id"));
        let request = InjectRequest {
            file: &file,
            cwd: &cwd,
            session: &session,
            tool: &tool,
            now: None,
            registry,
            allow_unknown_types: allow_unknown,
        };
        let context = inject_for_file(ctx, &request).unwrap_or_default();
        if context.is_empty() {
            return String::new();
        }
        extra.insert("additionalContext", context);
    }
    let mut out = Object::new();
    out.insert("hookSpecificOutput", extra);
    json::to_compact_string(&out.into())
        .map(|s| format!("{s}\n"))
        .unwrap_or_default()
}
