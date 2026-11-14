//! Prime/manifest/contract renderers, preserving the reference order.
use crate::{
    actions::{record::text, render::xml_escape},
    assets,
    config::Config,
    json::{Object, Value},
    lifecycle::{health::object, read::manifest_status_suffix},
    records,
    registry::Registry,
    timefmt,
};
use chrono::{DateTime, Utc};
pub const TYPE_ORDER: [&str; 6] = [
    "pattern",
    "convention",
    "failure",
    "decision",
    "reference",
    "guide",
];
pub const QUICK_REFERENCE: [(&str, &str); 6] = [
    (
        "luuna-atlas prime <domain>",
        "load full records for one domain",
    ),
    (
        "luuna-atlas prime --files <path>",
        "load records relevant to specific files",
    ),
    (
        "luuna-atlas search \"<query>\"",
        "search records across domains",
    ),
    (
        "luuna-atlas record <domain> --type <type> --description \"...\"",
        "store an insight",
    ),
    (
        "luuna-atlas learn",
        "discover what to record from changed files",
    ),
    (
        "luuna-atlas sync",
        "validate, stage, and commit .luuna/atlas/ changes",
    ),
];
pub fn format_prime(sections: &[String], format: &str) -> String {
    let format = if format.is_empty() {
        "markdown"
    } else {
        format
    };
    if sections.is_empty() {
        return assets::get(&format!("prime_empty_{format}")).into();
    }
    let sep = if format == "xml" { "\n" } else { "\n\n" };
    assets::get(&format!("prime_{format}")).replace("__ATLAS_SECTIONS__", &sections.join(sep))
}
pub fn session_end_reminder(format: &str, cfg: &Config) -> String {
    let close = records::obj(records::obj(cfg.get("prime")).get("session_close"));
    let custom = text(close.get("custom"));
    if !custom.is_empty() {
        return custom;
    }
    let style = text(close.get("style"));
    let style = if style.is_empty() {
        "conditional"
    } else {
        &style
    };
    let key = format!("close_{style}_{format}");
    if assets::KEYS.contains(&key.as_str()) {
        assets::get(&key).into()
    } else {
        assets::get(&format!("close_conditional_{format}")).into()
    }
}
pub fn xml_attr_escape(text: &str) -> String {
    xml_escape(text).replace('"', "&quot;")
}
pub fn compute_type_counts(rows: &[Object]) -> Object {
    let mut counts = Object::new();
    let mut order = Vec::new();
    for r in rows {
        let typ = text(r.get("type"));
        if !counts.contains_key(&typ) {
            order.push(typ.clone());
        }
        let n = records::number(counts.get(&typ)) as i64 + 1;
        counts.insert_unordered(typ, n);
    }
    counts.remember_order(order);
    counts
}
fn plural(n: usize, noun: &str) -> String {
    format!("{noun}{}", if n == 1 { "" } else { "s" })
}
fn type_counts(counts: &Object) -> String {
    let mut parts = Vec::new();
    for typ in TYPE_ORDER
        .into_iter()
        .chain(counts.keys().filter(|t| !TYPE_ORDER.contains(t)))
    {
        let n = records::number(counts.get(typ)) as usize;
        if n > 0 {
            parts.push(format!("{n} {}", plural(n, typ)));
        }
    }
    parts.join(", ")
}
#[derive(Clone)]
pub struct ManifestDomain {
    pub domain: String,
    pub count: usize,
    pub updated: Option<DateTime<Utc>>,
    pub type_counts: Object,
}
fn health(n: usize, g: &Object) -> &'static str {
    if n as f64 >= records::number(g.get("hard_limit")) {
        "over_hard_limit"
    } else if n as f64 >= records::number(g.get("warn_entries")) {
        "over_warn_threshold"
    } else if n as f64 >= records::number(g.get("max_entries")) {
        "approaching_limit"
    } else {
        "ok"
    }
}
pub fn build_manifest_payload(domains: &[ManifestDomain], g: &Object) -> Object {
    let entries = domains
        .iter()
        .map(|d| {
            let mut h = object([
                ("status", health(d.count, g).into()),
                (
                    "max_entries",
                    g.get("max_entries").cloned().unwrap_or(Value::Null),
                ),
                (
                    "warn_entries",
                    g.get("warn_entries").cloned().unwrap_or(Value::Null),
                ),
                (
                    "hard_limit",
                    g.get("hard_limit").cloned().unwrap_or(Value::Null),
                ),
            ]);
            h.remember_order(["status", "max_entries", "warn_entries", "hard_limit"]);
            let mut entry = Object::new();
            entry.insert("domain", d.domain.clone());
            entry.insert("count", d.count as i64);
            entry.insert(
                "lastUpdated",
                d.updated
                    .map(timefmt::iso)
                    .map(Value::from)
                    .unwrap_or(Value::Null),
            );
            entry.insert("type_counts", d.type_counts.clone());
            entry.insert("health", h);
            Value::Object(entry)
        })
        .collect();
    let quick = QUICK_REFERENCE
        .into_iter()
        .map(|(command, description)| {
            Value::Object(object([
                ("command", command.into()),
                ("description", description.into()),
            ]))
        })
        .collect();
    let mut out = Object::new();
    out.insert("type", "manifest");
    out.insert("quick_reference", Value::Array(quick));
    out.insert("domains", Value::Array(entries));
    out
}
pub fn format_manifest(
    domains: &[ManifestDomain],
    g: &Object,
    format: &str,
    now: DateTime<Utc>,
) -> String {
    let tick = char::from(96);
    let mut lines = Vec::new();
    if format == "xml" {
        lines.extend(["<manifest>".into(),format!("  <description>Manifest mode lists available domains. Load records on demand with {tick}luuna-atlas prime &lt;domain&gt;{tick} or {tick}luuna-atlas prime --files &lt;path&gt;{tick}.</description>"),"  <quick_reference>".into()]);
        for (command, description) in QUICK_REFERENCE {
            lines.push(format!(
                "    <command name=\"{}\">{}</command>",
                xml_attr_escape(command),
                xml_escape(description)
            ));
        }
        lines.extend(["  </quick_reference>".into(), "  <domains>".into()]);
        for d in domains {
            let updated = d
                .updated
                .map(|t| format!(" updated=\"{}\"", timefmt::format_time_ago(t, now)))
                .unwrap_or_default();
            let suffix = manifest_status_suffix(d.count, g).trim();
            let status = if suffix.is_empty() {
                String::new()
            } else {
                format!(" status=\"{}\"", xml_attr_escape(suffix))
            };
            lines.push(format!(
                "    <domain name=\"{}\" entries=\"{}\"{updated}{status}>",
                xml_attr_escape(&d.domain),
                d.count
            ));
            for typ in TYPE_ORDER {
                let n = records::number(d.type_counts.get(typ)) as i64;
                if n > 0 {
                    lines.push(format!("      <type_count type=\"{typ}\" count=\"{n}\" />"));
                }
            }
            lines.push("    </domain>".into());
        }
        lines.extend(["  </domains>".into(), "</manifest>".into()]);
        return lines.join("\n");
    }
    let plain = format == "plain";
    if plain {
        lines.extend(["Project Expertise Manifest (via Atlas)".into(),"======================================".into(),"".into(),format!("Manifest mode lists available domains. Load records on demand with {tick}luuna-atlas prime <domain>{tick} or {tick}luuna-atlas prime --files <path>{tick}."),"".into(),"Quick Reference:".into()]);
    } else {
        lines.extend(["# Project Expertise Manifest (via Atlas)".into(),"".into(),format!("> Manifest mode lists available domains. Load records on demand with {tick}luuna-atlas prime <domain>{tick} or {tick}luuna-atlas prime --files <path>{tick}."),"".into(),"## Quick Reference".into(),"".into()]);
    }
    for (command, description) in QUICK_REFERENCE {
        lines.push(if plain {
            format!("  - {command} — {description}")
        } else {
            format!("- {tick}{command}{tick} — {description}")
        });
    }
    lines.extend([
        "".into(),
        if plain {
            "Available Domains:"
        } else {
            "## Available Domains"
        }
        .into(),
    ]);
    if !plain {
        lines.push("".into());
    }
    if domains.is_empty() {
        lines.push(if plain{format!("  No expertise recorded yet. Use {tick}luuna-atlas add <domain>{tick} and {tick}luuna-atlas record{tick} to get started.")}else{format!("No expertise recorded yet. Use {tick}luuna-atlas add <domain>{tick} to create a domain, then {tick}luuna-atlas record{tick} to add records.")});
    }
    for d in domains {
        let types = type_counts(&d.type_counts);
        let types = if types.is_empty() {
            String::new()
        } else {
            format!(" ({types})")
        };
        let updated = d
            .updated
            .map(|t| format!(" — updated {}", timefmt::format_time_ago(t, now)))
            .unwrap_or_default();
        let prefix = if plain {
            format!("  - {}: ", d.domain)
        } else {
            format!("- **{}**: ", d.domain)
        };
        lines.push(format!(
            "{prefix}{} {}{types}{updated}{}",
            d.count,
            plural(d.count, "record"),
            manifest_status_suffix(d.count, g)
        ));
    }
    lines.join("\n")
}
pub fn format_project_contract(cfg: &Config, registry: &Registry, format: &str) -> String {
    let tick = char::from(96);
    let custom = registry
        .names()
        .into_iter()
        .filter_map(|name| {
            registry.get(name).filter(|d| !d.builtin).map(|d| {
                let parent = text(
                    records::obj(records::obj(cfg.get("custom_types")).get(name)).get("extends"),
                );
                (name, parent, d.required.clone(), d.optional.clone())
            })
        })
        .collect::<Vec<_>>();
    let disabled = records::list(cfg.get("disabled_types"));
    let domains = crate::actions::record::domain_names(cfg)
        .into_iter()
        .filter_map(|name| {
            let dc = records::obj(records::obj(cfg.get("domains")).get(&name));
            let allowed = records::list(dc.get("allowed_types"));
            let required = records::list(dc.get("required_fields"));
            if allowed.is_empty() && required.is_empty() {
                None
            } else {
                Some((name, allowed, required))
            }
        })
        .collect::<Vec<_>>();
    let hooks = [
        "pre-record",
        "post-record",
        "pre-prime",
        "pre-prune",
        "pre-compact",
    ]
    .into_iter()
    .filter(|event| !records::list(records::obj(cfg.get("hooks")).get(event)).is_empty())
    .collect::<Vec<_>>();
    if custom.is_empty() && disabled.is_empty() && domains.is_empty() && hooks.is_empty() {
        return String::new();
    }
    let mut lines = Vec::new();
    if format == "xml" {
        lines.push("<contract>".into());
        if !custom.is_empty() {
            lines.push("  <custom_types>".into());
            for (name, parent, required, optional) in &custom {
                let ext = if parent.is_empty() {
                    String::new()
                } else {
                    format!(" extends=\"{}\"", xml_attr_escape(parent))
                };
                lines.push(format!(
                    "    <type name=\"{}\"{ext}>",
                    xml_attr_escape(name)
                ));
                if !required.is_empty() {
                    lines.push(format!(
                        "      <required>{}</required>",
                        xml_escape(&required.join(", "))
                    ));
                }
                if !optional.is_empty() {
                    lines.push(format!(
                        "      <optional>{}</optional>",
                        xml_escape(&optional.join(", "))
                    ));
                }
                lines.push("    </type>".into());
            }
            lines.push("  </custom_types>".into());
        }
        if !disabled.is_empty() {
            lines.push(format!(
                "  <disabled_types>{}</disabled_types>",
                xml_escape(&disabled.join(", "))
            ));
        }
        if !domains.is_empty() {
            lines.push("  <domains>".into());
            for (name, allowed, required) in &domains {
                let mut attrs = String::new();
                if !allowed.is_empty() {
                    attrs.push_str(&format!(
                        " allowed=\"{}\"",
                        xml_attr_escape(&allowed.join(", "))
                    ));
                }
                if !required.is_empty() {
                    attrs.push_str(&format!(
                        " required=\"{}\"",
                        xml_attr_escape(&required.join(", "))
                    ));
                }
                lines.push(format!(
                    "    <domain name=\"{}\"{attrs} />",
                    xml_attr_escape(name)
                ));
            }
            lines.push("  </domains>".into());
        }
        if !hooks.is_empty() {
            lines.push(format!(
                "  <hooks>{}</hooks>",
                xml_escape(&hooks.join(", "))
            ));
        }
        lines.push("</contract>".into());
        return lines.join("\n");
    }
    let plain = format == "plain";
    if plain {
        lines.extend([
            "Project Contract (write-side gates)".into(),
            "===================================".into(),
        ]);
    } else {
        lines.extend([
            "## Project Contract".into(),
            "".into(),
            format!("Write-side gates {tick}luuna-atlas record{tick} enforces in this project."),
            "".into(),
        ]);
    }
    if !custom.is_empty() {
        if plain {
            lines.extend(["".into(), "Custom types:".into()]);
        } else {
            lines.push("**Custom types**:".into());
        }
        for (name, parent, required, optional) in &custom {
            let ext = if parent.is_empty() {
                String::new()
            } else if plain {
                format!(" (extends {parent})")
            } else {
                format!(" (extends {tick}{parent}{tick})")
            };
            let mut parts = Vec::new();
            if !required.is_empty() {
                parts.push(format!("required: {}", required.join(", ")));
            }
            if !optional.is_empty() {
                parts.push(format!("optional: {}", optional.join(", ")));
            }
            let suffix = if parts.is_empty() {
                String::new()
            } else {
                format!("; {}", parts.join("; "))
            };
            lines.push(if plain {
                format!("  - {name}{ext}{suffix}")
            } else {
                format!("- {tick}{name}{tick}{ext}{suffix}")
            });
        }
        if !plain {
            lines.push("".into());
        }
    }
    if !disabled.is_empty() {
        if plain {
            lines.extend([
                "".into(),
                format!(
                    "Disabled types: {} (writes emit a deprecation warning)",
                    disabled.join(", ")
                ),
            ]);
        } else {
            lines.extend([
                format!(
                    "**Disabled types**: {} (writes emit a deprecation warning)",
                    disabled
                        .iter()
                        .map(|n| format!("{tick}{n}{tick}"))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                "".into(),
            ]);
        }
    }
    if !domains.is_empty() {
        if plain {
            lines.extend(["".into(), "Per-domain rules:".into()]);
        } else {
            lines.push("**Per-domain rules**:".into());
        }
        for (name, allowed, required) in &domains {
            let mut parts = Vec::new();
            if !allowed.is_empty() {
                parts.push(format!(
                    "{}{}",
                    if plain {
                        "allowed: "
                    } else {
                        "allowed types — "
                    },
                    allowed.join(", ")
                ));
            }
            if !required.is_empty() {
                parts.push(format!(
                    "{}{}",
                    if plain {
                        "required: "
                    } else {
                        "required fields — "
                    },
                    required.join(", ")
                ));
            }
            lines.push(if plain {
                format!("  - {name}: {}", parts.join("; "))
            } else {
                format!("- {tick}{name}{tick}: {}", parts.join("; "))
            });
        }
        if !plain {
            lines.push("".into());
        }
    }
    if !hooks.is_empty() {
        if plain {
            lines.extend(["".into(), format!("Active hooks: {}", hooks.join(", "))]);
        } else {
            lines.push(format!("**Active hooks**: {}", hooks.join(", ")));
        }
    }
    lines
        .join("\n")
        .trim_end_matches(['\r', '\n', ' ', '\t'])
        .into()
}
