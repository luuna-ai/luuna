//! Human renderers shared with later native commands.
use crate::{
    json::{Object, Value},
    registry::{Registry, TypeDefinition},
};
use chrono::{DateTime, Utc};
use std::collections::{BTreeMap, BTreeSet};
#[derive(Default)]
pub struct FormatOptions {
    pub format: String,
    pub full: bool,
    pub annotations: BTreeMap<String, String>,
    pub stale_ids: BTreeSet<String>,
}
use super::record::text;
use crate::{records, timefmt};
pub fn xml_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}
pub fn format_links(record: &Object) -> String {
    let mut parts = Vec::new();
    for (key, label) in [("relates_to", "relates to"), ("supersedes", "supersedes")] {
        let values = records::list(record.get(key));
        if !values.is_empty() {
            parts.push(format!("{label}: {}", values.join(", ")));
        }
    }
    if parts.is_empty() {
        String::new()
    } else {
        format!(" [{}]", parts.join("; "))
    }
}
pub fn format_evidence(value: &Value) -> String {
    let e = records::obj(Some(value));
    let mut parts = Vec::new();
    for key in ["commit", "date", "issue", "file"] {
        let s = text(e.get(key));
        if !s.is_empty() {
            parts.push(format!("{key}: {s}"));
        }
    }
    if parts.is_empty() {
        String::new()
    } else {
        format!(" [{}]", parts.join(", "))
    }
}
pub fn format_record_meta(record: &Object, full: bool) -> String {
    if !full {
        return format_links(record);
    }
    let mut parts = vec![format!(
        "({}){}",
        text(record.get("classification")),
        format_evidence(record.get("evidence").unwrap_or(&Value::Null))
    )];
    let tags = records::list(record.get("tags"));
    if !tags.is_empty() {
        parts.push(format!("[tags: {}]", tags.join(", ")));
    }
    format!(" {}{}", parts.join(" "), format_links(record))
}
fn id_tag(record: &Object) -> String {
    let id = text(record.get("id"));
    if id.is_empty() {
        String::new()
    } else {
        format!("[{id}] ")
    }
}
fn slice(text: &str, n: usize) -> String {
    String::from_utf16_lossy(&text.encode_utf16().take(n).collect::<Vec<_>>())
}
pub fn format_record_index(record: &Object, registry: &Registry, stale: bool) -> String {
    let first = |keys: &[&str]| {
        keys.iter()
            .map(|k| text(record.get(k)))
            .find(|s| !s.is_empty())
            .unwrap_or_default()
    };
    let head = first(&["content", "name", "title"]);
    let body = first(&["description", "rationale"]);
    let s = if !head.is_empty() && !body.is_empty() {
        format!("{head}: {body}")
    } else if head.is_empty() {
        body
    } else {
        head
    };
    let s = if s.is_empty() {
        records::record_summary(record, registry)
    } else {
        s
    };
    // Go regexp's ASCII whitespace class is narrower than Unicode whitespace.
    let mut compact = String::new();
    let mut whitespace = false;
    for c in s.chars() {
        if matches!(c, ' ' | '\t' | '\n' | '\r' | '\x0c') {
            if !whitespace {
                compact.push(' ');
            }
            whitespace = true;
        } else {
            compact.push(c);
            whitespace = false;
        }
    }
    let s = compact.trim();
    let s = if s.encode_utf16().count() > 100 {
        format!("{}...", slice(s, 100))
    } else {
        s.into()
    };
    let id = text(record.get("id"));
    let id = if id.is_empty() {
        String::new()
    } else {
        format!("{id} ")
    };
    let field = registry
        .get(&text(record.get("type")))
        .filter(|d| !d.files_field.is_empty())
        .map(|d| d.files_field.as_str())
        .unwrap_or("files");
    let mut anchors = records::list(record.get(field));
    anchors.extend(records::list(record.get("dir_anchors")));
    let mut suffix = String::new();
    if !anchors.is_empty() {
        suffix = format!(" ({}", anchors[..anchors.len().min(3)].join(", "));
        if anchors.len() > 3 {
            suffix.push_str(&format!(", +{}", anchors.len() - 3));
        }
        suffix.push(')');
    }
    let score = records::confirmation_score(record);
    if score > 0.0 {
        suffix.push_str(&format!(" ★{score}"));
    }
    if stale {
        suffix.push_str(" (stale?)");
    }
    format!("- {id}[{}] {s}{suffix}", text(record.get("type")))
}
pub fn format_markdown_records(
    def: &TypeDefinition,
    records: &[Object],
    full: bool,
    registry: &Registry,
    annotations: &BTreeMap<String, String>,
) -> String {
    if records.is_empty() {
        return String::new();
    }
    let mut lines = vec![format!("### {}", def.section_title)];
    for r in records {
        let mut line = format!("- {}", id_tag(r));
        match def.name.as_str() {
            "convention" => line.push_str(&format!(
                "{}{}",
                text(r.get("content")),
                format_record_meta(r, full)
            )),
            "pattern" | "reference" | "guide" => {
                line.push_str(&format!(
                    "**{}**: {}",
                    text(r.get("name")),
                    text(r.get("description"))
                ));
                let files = records::list(r.get("files"));
                if !files.is_empty() {
                    line.push_str(&format!(" ({})", files.join(", ")));
                }
                line.push_str(&format_record_meta(r, full));
            }
            "failure" => line.push_str(&format!(
                "{}{}",
                text(r.get("description")),
                format_record_meta(r, full)
            )),
            "decision" => line.push_str(&format!(
                "**{}**: {}{}",
                text(r.get("title")),
                text(r.get("rationale")),
                format_record_meta(r, full)
            )),
            _ => {
                line.push_str(&format!("**{}**", records::record_summary(r, registry)));
                let mut details = Vec::new();
                for f in def.required.iter().chain(&def.optional) {
                    if def.extracts_files && f == &def.files_field {
                        continue;
                    }
                    if let Some(v) = r.get(f).filter(|v| !matches!(v, Value::Null)) {
                        let s = records::format_js(Some(v));
                        if !s.is_empty() {
                            details.push(format!("{f}: {s}"));
                        }
                    }
                }
                if !details.is_empty() {
                    line.push_str(&format!(" — {}", details.join("; ")));
                }
                if def.extracts_files {
                    let files = records::list(r.get(&def.files_field));
                    if !files.is_empty() {
                        line.push_str(&format!(" ({})", files.join(", ")));
                    }
                }
                line.push_str(&format_record_meta(r, full));
            }
        }
        if let Some(why) = annotations
            .get(&text(r.get("id")))
            .filter(|why| !why.is_empty())
        {
            line.push_str(&format!(" — {why}"));
        }
        lines.push(line);
        if def.name == "failure" {
            lines.push(format!("  → {}", text(r.get("resolution"))));
        }
    }
    lines.join("\n")
}
pub fn format_plain_records(
    def: &TypeDefinition,
    records: &[Object],
    annotations: &BTreeMap<String, String>,
) -> Vec<String> {
    if !def.builtin {
        return Vec::new();
    }
    let mut lines = vec![format!("{}:", def.section_title)];
    for r in records {
        let mut line = format!("  - {}", id_tag(r));
        match def.name.as_str() {
            "convention" => line.push_str(&text(r.get("content"))),
            "pattern" | "reference" | "guide" => {
                line.push_str(&format!(
                    "{}: {}",
                    text(r.get("name")),
                    text(r.get("description"))
                ));
                let files = records::list(r.get("files"));
                if !files.is_empty() {
                    line.push_str(&format!(" ({})", files.join(", ")));
                }
            }
            "failure" => line.push_str(&text(r.get("description"))),
            "decision" => line.push_str(&format!(
                "{}: {}",
                text(r.get("title")),
                text(r.get("rationale"))
            )),
            _ => {}
        }
        line.push_str(&format_links(r));
        if let Some(why) = annotations
            .get(&text(r.get("id")))
            .filter(|s| !s.is_empty())
        {
            line.push_str(&format!("  ({why})"));
        }
        lines.push(line);
        if def.name == "failure" {
            lines.push(format!("    Fix: {}", text(r.get("resolution"))));
        }
    }
    lines.push(String::new());
    lines
}
pub fn format_xml_record(
    r: &Object,
    def: &TypeDefinition,
    annotations: &BTreeMap<String, String>,
) -> Vec<String> {
    let typ = text(r.get("type"));
    let id = text(r.get("id"));
    let id = if id.is_empty() {
        String::new()
    } else {
        format!(" id=\"{}\"", xml_escape(&id))
    };
    let why = annotations
        .get(&text(r.get("id")))
        .filter(|s| !s.is_empty())
        .map(|s| format!(" why=\"{}\"", xml_escape(s).replace('"', "&quot;")))
        .unwrap_or_default();
    let mut lines = vec![format!(
        "  <{typ}{id} classification=\"{}\"{why}>",
        text(r.get("classification"))
    )];
    let mut fields: Vec<&str> = match typ.as_str() {
        "convention" => {
            lines.push(format!("    {}", xml_escape(&text(r.get("content")))));
            vec![]
        }
        "pattern" | "reference" | "guide" => vec!["name", "description"],
        "failure" => vec!["description", "resolution"],
        "decision" => vec!["title", "rationale"],
        _ => def
            .required
            .iter()
            .chain(&def.optional)
            .map(String::as_str)
            .collect(),
    };
    if matches!(typ.as_str(), "pattern" | "reference" | "guide")
        && !records::list(r.get("files")).is_empty()
    {
        fields.push("files");
    }
    for f in fields {
        if let Some(v) = r.get(f).filter(|v| !matches!(v, Value::Null)) {
            let value = if matches!(v, Value::Array(_)) {
                records::list(Some(v))
                    .iter()
                    .map(|s| xml_escape(s))
                    .collect::<Vec<_>>()
                    .join(", ")
            } else {
                xml_escape(&records::format_js(Some(v)))
            };
            lines.push(format!("    <{f}>{value}</{f}>"));
        }
    }
    let tags = records::list(r.get("tags"));
    if !tags.is_empty() {
        lines.push(format!(
            "    <tags>{}</tags>",
            tags.iter()
                .map(|s| xml_escape(s))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    for f in ["relates_to", "supersedes"] {
        let values = records::list(r.get(f));
        if !values.is_empty() {
            lines.push(format!("    <{f}>{}</{f}>", values.join(", ")));
        }
    }
    for value in records::any_list(r.get("outcomes")) {
        let o = records::obj(Some(value));
        let duration = o
            .get("duration")
            .map(|v| format!(" duration=\"{}\"", records::format_js(Some(v))))
            .unwrap_or_default();
        let agent = text(o.get("agent"));
        let agent = if agent.is_empty() {
            String::new()
        } else {
            format!(" agent=\"{}\"", xml_escape(&agent))
        };
        lines.push(format!(
            "    <outcome status=\"{}\"{duration}{agent}>{}</outcome>",
            text(o.get("status")),
            xml_escape(&text(o.get("test_results")))
        ));
    }
    lines.push(format!("  </{typ}>"));
    lines
}
pub fn format_domain(
    domain: &str,
    records: &[Object],
    last_updated: Option<DateTime<Utc>>,
    registry: &Registry,
    opts: &FormatOptions,
) -> String {
    let format = if opts.format.is_empty() {
        "markdown"
    } else {
        &opts.format
    };
    let updated = last_updated
        .map(|t| timefmt::format_time_ago(t, Utc::now()))
        .unwrap_or_default();
    if format == "compact" {
        let suffix = if updated.is_empty() {
            String::new()
        } else {
            format!(", updated {updated}")
        };
        let mut lines = vec![format!("## {domain} ({} records{suffix})", records.len())];
        lines.extend(records.iter().map(|r| {
            format_record_index(r, registry, opts.stale_ids.contains(&text(r.get("id"))))
        }));
        return lines.join("\n");
    }
    if format == "xml" {
        let suffix = if updated.is_empty() {
            String::new()
        } else {
            format!(" updated=\"{updated}\"")
        };
        let mut lines = vec![format!(
            "<domain name=\"{}\" entries=\"{}\"{suffix}>",
            xml_escape(domain),
            records.len()
        )];
        for r in records {
            if let Some(def) = registry.get(&text(r.get("type"))) {
                lines.extend(format_xml_record(r, def, &opts.annotations));
            }
        }
        lines.push("</domain>".into());
        return lines.join("\n");
    }
    let suffix = if updated.is_empty() {
        String::new()
    } else if format == "plain" {
        format!(" (updated {updated})")
    } else {
        format!(", updated {updated}")
    };
    let mut lines = vec![
        if format == "plain" {
            format!("[{domain}] {} records{suffix}", records.len())
        } else {
            format!("## {domain} ({} records{suffix})", records.len())
        },
        String::new(),
    ];
    let mut sections = Vec::new();
    for name in registry.names() {
        let def = registry.get(name).expect("registry order names a type");
        let subset = records
            .iter()
            .filter(|r| text(r.get("type")) == name)
            .cloned()
            .collect::<Vec<_>>();
        if subset.is_empty() {
            continue;
        }
        if format == "plain" {
            lines.extend(format_plain_records(def, &subset, &opts.annotations));
        } else {
            sections.push(format_markdown_records(
                def,
                &subset,
                opts.full,
                registry,
                &opts.annotations,
            ));
        }
    }
    if format == "plain" {
        lines
            .join("\n")
            .trim_end_matches([' ', '\t', '\r', '\n'])
            .into()
    } else {
        lines.push(sections.join("\n\n"));
        lines.join("\n")
    }
}
pub fn format_type_records(records: &[Object], registry: &Registry, full: bool) -> String {
    records
        .first()
        .and_then(|r| registry.get(&text(r.get("type"))))
        .map(|def| format_markdown_records(def, records, full, registry, &BTreeMap::new()))
        .unwrap_or_default()
}
