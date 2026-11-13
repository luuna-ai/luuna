//! Native search, including Go's raw archive reader and repeated score sorting.
use super::{
    api,
    record::{self, boolean, domain_names, get, has, message, text, Action},
    render,
};
use crate::{
    json::{Object, Value},
    options::Context,
    records, store, timefmt, AtlasStore, CommandOutput, Result,
};
#[derive(Clone, Debug, Default)]
pub struct SearchOptions {
    pub flags: Vec<(String, String)>,
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
fn archive(path: &std::path::Path) -> Result<Vec<Object>> {
    let Ok(data) = std::fs::read(path) else {
        return Ok(Vec::new());
    };
    let mut rows = Vec::new();
    for line in String::from_utf8_lossy(&data).split('\n') {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let value = crate::json::parse_bytes(line.as_bytes())
            .map_err(|_| message(store::json_parse_error(line.as_bytes())))?;
        rows.push(records::obj(Some(&value)).clone());
    }
    Ok(rows)
}
fn known(a: &mut Action<'_>, cfg: &Object, domain: &str) -> bool {
    if records::obj(cfg.get("domains")).contains_key(domain) {
        return true;
    }
    a.error(
        "search",
        &format!(
            "Domain {} not found in config. Available domains: {}",
            records::go_quote(domain),
            domain_names(cfg).join(", ")
        ),
    );
    if !a.ctx.globals.json {
        let tick = char::from(96);
        a.output.stderr.push_str(&format!("Hint: Run {tick}luuna-atlas add {domain}{tick} to create this domain, or check .luuna/atlas/config.yaml\n"));
    }
    false
}
pub fn run_search(ctx: &Context, query: Option<&str>, options: &SearchOptions) -> CommandOutput {
    record::run(ctx, "search", |a| {
        let q = query.unwrap_or_default();
        let flags = &options.flags;
        if q.is_empty()
            && ![
                "type",
                "domain",
                "tag",
                "classification",
                "file",
                "outcome-status",
            ]
            .iter()
            .any(|k| !get(flags, k).is_empty())
        {
            return Err(message("Provide a search query or use --type, --domain, --tag, --classification, --file, or --outcome-status to filter."));
        }
        let cfg = a.config()?;
        let mut domains = domain_names(&cfg);
        let domain = get(flags, "domain");
        if !domain.is_empty() {
            if !known(a, &cfg, &domain) {
                return Ok(());
            }
            domains = vec![domain];
        }
        let mut boost =
            records::number_default(records::obj(cfg.get("search")).get("boost_factor"), 0.1);
        if has(flags, "boost") && !boolean(flags, "boost") {
            boost = 0.0;
        }
        let archived = boolean(flags, "archived");
        let format = get(flags, "format");
        let format = if !format.is_empty() {
            format
        } else if !a.ctx.globals.format.is_empty() {
            a.ctx.globals.format.clone()
        } else {
            "markdown".into()
        };
        let label = if q.is_empty() {
            "matching filters".into()
        } else {
            format!("matching \"{q}\"")
        };
        let mut results = Vec::new();
        let mut sections = Vec::new();
        let mut ids = Vec::new();
        let mut total = 0;
        for domain in domains {
            let path = a.path(&domain)?;
            let mut live = filter(
                store::read_records(&path, &a.registry, a.ctx.globals.allow_unknown_types)?,
                flags,
            );
            if !q.is_empty() {
                live = records::search_records(&live, q, boost);
            }
            if boolean(flags, "sort-by-score") {
                live = records::sort_by_confirmation_score(&live);
            }
            let mut old = if archived {
                let path = AtlasStore::open(&a.ctx.root)?.archive_path(&domain)?;
                filter(archive(&path)?, flags)
            } else {
                Vec::new()
            };
            if !q.is_empty() {
                old = records::search_records(&old, q, boost);
            }
            if boolean(flags, "sort-by-score") {
                old = records::sort_by_confirmation_score(&old);
            }
            if live.is_empty() && old.is_empty() {
                continue;
            }
            total += live.len() + old.len();
            let mut entry = Object::new();
            entry.insert_unordered("domain", domain.clone());
            entry.insert_unordered(
                "matches",
                Value::Array(live.iter().cloned().map(Value::Object).collect()),
            );
            if archived {
                entry.insert_unordered(
                    "archived",
                    Value::Array(old.iter().cloned().map(Value::Object).collect()),
                );
            }
            results.push(Value::Object(entry));
            for r in live.iter().chain(&old) {
                let id = text(r.get("id"));
                if !id.is_empty() {
                    ids.push(id);
                }
            }
            if !live.is_empty() {
                sections.push(render::format_domain(
                    &domain,
                    &live,
                    timefmt::file_mod_time(&path),
                    &a.registry,
                    &render::FormatOptions {
                        format: format.clone(),
                        ..Default::default()
                    },
                ));
            }
            if !old.is_empty() {
                let suffix = if old.len() == 1 { "" } else { "s" };
                let mut lines = vec![format!(
                    "## {domain} (archived, {} record{suffix})",
                    old.len()
                )];
                for r in &old {
                    let date = text(r.get("archived_at"));
                    let date = if date.len() > 10 {
                        String::from_utf8_lossy(&date.as_bytes()[..10]).into_owned()
                    } else {
                        date
                    };
                    let date = if date.is_empty() {
                        "unknown".into()
                    } else {
                        date
                    };
                    let id = text(r.get("id"));
                    let id = if id.is_empty() {
                        String::new()
                    } else {
                        format!("{id} ")
                    };
                    let reason = text(r.get("archive_reason"));
                    let reason = if reason.is_empty() {
                        String::new()
                    } else {
                        format!(" {reason}")
                    };
                    lines.push(format!(
                        "- [ARCHIVED {date}{reason}] {id}[{}] {}",
                        text(r.get("type")),
                        records::record_summary(r, &a.registry)
                    ));
                }
                sections.push(lines.join("\n"));
            }
        }
        if a.ctx.globals.json {
            let mut o = Object::new();
            o.insert_unordered("query", query.map(Value::from).unwrap_or(Value::Null));
            o.insert_unordered("total", total as i64);
            o.insert_unordered("domains", Value::Array(results));
            a.json("search", o);
            return Ok(());
        }
        let empty = format!("No records {label} found.\n");
        if format == "ids" {
            a.output.stdout.push_str(&if ids.is_empty() {
                empty
            } else {
                format!("{}\n", ids.join("\n"))
            });
        } else if sections.is_empty() {
            a.output.stdout.push_str(&empty);
        } else {
            a.output.stdout.push_str(&format!(
                "{}\n\n{total} match{} found.\n",
                sections.join("\n\n"),
                if total == 1 { "" } else { "es" }
            ));
        }
        Ok(())
    })
}
