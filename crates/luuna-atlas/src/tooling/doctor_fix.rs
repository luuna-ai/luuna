//! Plan every repair before writing, then recheck each input under its file lock.
use super::doctor::{path_exists, Check};
use crate::{
    actions::record::{domain_names, message, text, Action},
    config::{self, Config},
    json::{self, Object, Value},
    lifecycle::health,
    records, schema, store, Result,
};
use std::{collections::BTreeMap, fs, path::PathBuf};
enum Write {
    Bytes(Vec<u8>),
    Records(Vec<Object>),
    Config(Config),
}
struct Change {
    path: PathBuf,
    before: Option<Vec<u8>>,
    write: Write,
}
fn bytes(path: &std::path::Path) -> Result<Option<Vec<u8>>> {
    match fs::read(path) {
        Ok(b) => Ok(Some(b)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(message(format!(
            "Refusing to repair unreadable {}: {e}",
            path.display()
        ))),
    }
}
fn encode(rows: &[Object]) -> Result<String> {
    let mut out = String::new();
    for r in rows {
        out += &json::to_compact_string(&Value::Object(r.clone()))?;
        out.push('\n');
    }
    Ok(out)
}
pub(super) fn run(a: &Action<'_>, checks: &[Check], cfg: &Config) -> Result<Vec<String>> {
    let mut fixed = Vec::new();
    let mut changes = Vec::new();
    let mut virtual_files: BTreeMap<PathBuf, Option<Vec<u8>>> = BTreeMap::new();
    let domains = domain_names(cfg);
    let mut cfg = cfg.clone();
    let dir = a.ctx.root.join(".luuna/atlas");
    for check in checks.iter().filter(|c| c.status != "pass" && c.fixable) {
        match check.name {
            "jsonl-integrity" | "legacy-outcome" => {
                for d in &domains {
                    let path = a.path(d)?;
                    let before = if let Some(b) = virtual_files.get(&path) {
                        b.clone()
                    } else {
                        bytes(&path)?
                    };
                    let Some(raw) = &before else { continue };
                    let mut out = Vec::new();
                    let mut changed = 0;
                    for line in raw.split(|b| *b == b'\n') {
                        let projected = super::text_bytes::encode(line);
                        let line = projected.trim();
                        if line.is_empty() {
                            continue;
                        }
                        let raw_line = super::text_bytes::decode(line);
                        let parsed = json::parse_bytes(&raw_line);
                        if check.name == "jsonl-integrity" {
                            if parsed.is_err() {
                                if !line.starts_with('#') {
                                    return Err(message("Refusing to remove stored invalid JSON lines during doctor repair; no files were changed."));
                                }
                                changed += 1;
                            } else {
                                out.push(raw_line);
                            }
                            continue;
                        }
                        let mut r = parsed
                            .as_ref()
                            .ok()
                            .and_then(Value::as_object)
                            .cloned()
                            .unwrap_or_default();
                        if r.get("outcome").is_some_and(|v| !v.is_null())
                            && !r.contains_key("outcomes")
                        {
                            records::check_lossless_rewrite(&path, &a.registry, &[])?;
                            let legacy = records::obj(r.get("outcome"));
                            let mut o = Object::new();
                            for field in ["status", "duration", "test_results", "agent"] {
                                if let Some(v) = legacy.get(field) {
                                    o.insert_unordered(field, v.clone());
                                }
                            }
                            r.remove("outcome");
                            r.insert_unordered("outcomes", Value::Array(vec![o.into()]));
                            records::check_retained_numbers(&path, [(&r, &r)])?;
                            out.push(json::to_compact_string(&r.into())?.into_bytes());
                            changed += 1;
                        } else {
                            out.push(raw_line)
                        }
                    }
                    if changed > 0 {
                        let mut body = Vec::new();
                        for line in out {
                            body.extend_from_slice(&line);
                            body.push(b'\n');
                        }
                        virtual_files.insert(path.clone(), Some(body.clone()));
                        changes.push(Change {
                            path,
                            before,
                            write: Write::Bytes(body),
                        });
                        fixed.push(if check.name=="jsonl-integrity"{format!("Removed {changed} invalid JSON line(s) from {d}")}else{format!("Migrated {changed} legacy \"outcome\" field(s) to \"outcomes[]\" in {d}")});
                    }
                }
            }
            "schema-validation" | "stale-records" | "file-anchors" => {
                for d in &domains {
                    let path = a.path(d)?;
                    let before = if let Some(b) = virtual_files.get(&path) {
                        b.clone()
                    } else {
                        bytes(&path)?
                    };
                    let rows = store::read_records(&path, &a.registry, true)?;
                    let mut next = Vec::new();
                    let mut changed = 0;
                    for r in &rows {
                        if check.name == "schema-validation"
                            && a.registry.get(&text(r.get("type"))).is_some()
                            && !schema::validate_record(r, &a.registry)?.is_empty()
                        {
                            return Err(message(format!("Refusing to delete schema-invalid records during doctor repair in {d}; no files were changed.")));
                        }
                        if check.name == "stale-records"
                            && health::is_record_stale(
                                r,
                                chrono::Utc::now(),
                                records::obj(
                                    records::obj(cfg.get("classification_defaults"))
                                        .get("shelf_life"),
                                ),
                            )
                        {
                            return Err(message(format!("Refusing to permanently delete stale records during doctor repair in {d}; use explicit archive/prune instead. No files were changed.")));
                        }
                        let mut nr = r.clone();
                        if check.name == "file-anchors" {
                            for field in ["files", "dir_anchors"] {
                                if !nr.contains_key(field) {
                                    continue;
                                }
                                let anchors = records::list(nr.get(field));
                                let kept: Vec<_> = anchors
                                    .iter()
                                    .filter(|p| path_exists(&a.ctx.cwd, p))
                                    .cloned()
                                    .collect();
                                if kept.len() < anchors.len() {
                                    if !matches!(nr.get(field),Some(Value::Array(values)) if values.iter().all(|v|v.string_bytes().is_some()))
                                    {
                                        return Err(message(format!("Refusing to discard non-string {field} data while removing broken anchors in {d}; no files were changed.")));
                                    }
                                    changed += anchors.len() - kept.len();
                                    if kept.is_empty() {
                                        nr.remove(field);
                                    } else {
                                        nr.insert_unordered(
                                            field,
                                            Value::Array(
                                                kept.into_iter().map(Value::String).collect(),
                                            ),
                                        );
                                    }
                                }
                            }
                            let mut ev = records::obj(nr.get("evidence")).clone();
                            let file = text(ev.get("file"));
                            if !file.is_empty() && !path_exists(&a.ctx.cwd, &file) {
                                changed += 1;
                                ev.remove("file");
                                nr.insert_unordered("evidence", ev);
                            }
                        }
                        next.push(nr);
                    }
                    if changed > 0 {
                        records::check_lossless_rewrite(&path, &a.registry, &[])?;
                        records::check_retained_ids(&path, rows.iter().zip(next.iter()))?;
                        records::check_retained_numbers(&path, rows.iter().zip(next.iter()))?;
                        records::assign_record_ids(&mut next, &a.registry)?;
                        let body = encode(&next)?;
                        virtual_files.insert(path.clone(), Some(body.into_bytes()));
                        changes.push(Change {
                            path,
                            before,
                            write: Write::Records(next),
                        });
                        fixed.push(format!("Removed {changed} broken file anchor(s) from {d}"));
                    }
                }
            }
            "orphaned-domains" => {
                let mut entries: Vec<_> = fs::read_dir(dir.join("expertise"))
                    .into_iter()
                    .flatten()
                    .filter_map(std::result::Result::ok)
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .collect();
                entries.sort();
                let mut current = records::obj(cfg.get("domains")).clone();
                let mut replaced = Vec::new();
                for file in entries {
                    if !file.ends_with(".jsonl") {
                        continue;
                    }
                    let domain = file.replacen(".jsonl", "", 1);
                    if !current.contains_key(&domain) {
                        current.insert(&domain, Object::new());
                        replaced.push(vec!["domains".into(), domain.clone()]);
                        fixed.push(format!(
                            "Added orphaned domain {} to config",
                            records::go_quote(&domain)
                        ));
                    }
                }
                cfg.insert_unordered("domains", current);
                for d in domain_names(&cfg) {
                    let path = a.path(&d)?;
                    if !path.exists() {
                        let before = bytes(&path)?;
                        changes.push(Change {
                            path: path.clone(),
                            before,
                            write: Write::Bytes(Vec::new()),
                        });
                        virtual_files.insert(path, None);
                        fixed.push(format!(
                            "Created missing JSONL file for domain {}",
                            records::go_quote(&d)
                        ));
                    }
                }
                if !fixed.is_empty() {
                    config::check_lossless_rewrite(&dir, &cfg, &replaced)?;
                    let path = dir.join("config.yaml");
                    let before = bytes(&path)?;
                    changes.push(Change {
                        path,
                        before,
                        write: Write::Config(cfg.clone()),
                    });
                }
            }
            _ => (),
        }
    }
    // The entire prospective repair has passed the data-loss checks. Refuse races rather
    // than overwriting another writer's newer input.
    for c in changes {
        let write = || -> Result<()> {
            if bytes(&c.path)? != c.before {
                return Err(message(format!(
                    "Refusing to repair {}: input changed during preparation.",
                    c.path.display()
                )));
            }
            match c.write {
                Write::Bytes(ref text) => crate::commands::write_file(&c.path, text),
                Write::Records(ref rows) => {
                    store::write_records(&c.path, &mut rows.clone(), &a.registry)
                }
                Write::Config(ref cfg) => config::write_config(&dir, cfg),
            }
        };
        if matches!(c.write, Write::Config(_)) {
            write()?
        } else {
            store::with_file_lock(&c.path, write)?
        }
    }
    Ok(fixed)
}
