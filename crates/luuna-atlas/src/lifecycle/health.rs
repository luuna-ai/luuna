//! Corpus validation and shared time/health calculations.
use crate::{
    actions::record::{domain_names, text, Action},
    config::Config,
    json::{self, Object, Value},
    records,
    registry::Registry,
    schema, timefmt, Result,
};
use chrono::{DateTime, TimeZone, Utc};
pub(crate) fn object<const N: usize>(fields: [(&str, Value); N]) -> Object {
    let mut out = Object::new();
    for (key, value) in fields {
        out.insert_unordered(key, value);
    }
    out
}
pub fn zero_time() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(1, 1, 1, 0, 0, 0)
        .single()
        .expect("literal Go zero time")
}
pub fn date(raw: &str) -> DateTime<Utc> {
    timefmt::parse_date(raw).unwrap_or_else(zero_time)
}
pub fn hours(now: DateTime<Utc>, then: DateTime<Utc>) -> f64 {
    let diff = now.signed_duration_since(then);
    let nanos = i128::from(diff.num_seconds()) * 1_000_000_000 + i128::from(diff.subsec_nanos());
    let nanos = nanos.clamp(i128::from(i64::MIN), i128::from(i64::MAX)) as i64;
    (nanos / 3_600_000_000_000) as f64 + (nanos % 3_600_000_000_000) as f64 / 3600e9
}
pub fn go_int(value: f64) -> i64 {
    if !value.is_finite() || value < i64::MIN as f64 || value >= i64::MAX as f64 {
        i64::MIN
    } else {
        value as i64
    }
}
pub fn is_record_stale(record: &Object, now: DateTime<Utc>, shelf: &Object) -> bool {
    let class = text(record.get("classification"));
    if class == "foundational" {
        return false;
    }
    let at = date(&text(record.get("recorded_at")));
    if at == zero_time() {
        return false;
    }
    let age = (hours(now, at) / 24.0).floor();
    match class.as_str() {
        "tactical" => age > records::number_default(shelf.get("tactical"), 14.0),
        "observational" => age > records::number_default(shelf.get("observational"), 30.0),
        _ => false,
    }
}
pub fn calculate_domain_health(
    rows: &[Object],
    max_entries: f64,
    shelf: &Object,
    registry: &Registry,
    now: DateTime<Utc>,
) -> Object {
    let mut types = Object::new();
    for name in registry.names() {
        types.insert_unordered(name, 0_i64);
    }
    let mut classes = object([
        ("foundational", 0_i64.into()),
        ("tactical", 0_i64.into()),
        ("observational", 0_i64.into()),
    ]);
    let mut stale = 0;
    let mut oldest = String::new();
    let mut newest = String::new();
    for row in rows {
        for (target, key) in [(&mut types, "type"), (&mut classes, "classification")] {
            let kind = text(row.get(key));
            let count = records::number(target.get(&kind)) + 1.0;
            target.insert_unordered(kind, count);
        }
        if is_record_stale(row, now, shelf) {
            stale += 1;
        }
        let recorded = text(row.get("recorded_at"));
        if oldest.is_empty() || recorded < oldest {
            oldest = recorded.clone();
        }
        if newest.is_empty() || recorded > newest {
            newest = recorded;
        }
    }
    let util = if max_entries > 0.0 {
        (rows.len() as f64 / max_entries * 100.0).round()
    } else {
        0.0
    };
    object([
        ("governance_utilization", util.into()),
        ("stale_count", Value::from(stale)),
        ("type_distribution", types.into()),
        ("classification_distribution", classes.into()),
        (
            "oldest_timestamp",
            if oldest.is_empty() {
                Value::Null
            } else {
                oldest.into()
            },
        ),
        (
            "newest_timestamp",
            if newest.is_empty() {
                Value::Null
            } else {
                newest.into()
            },
        ),
    ])
}
pub struct ValidationMessage {
    pub domain: String,
    pub line: usize,
    pub message: String,
}
impl ValidationMessage {
    fn value(&self) -> Value {
        let mut o = Object::new();
        o.insert("domain", self.domain.clone());
        o.insert("line", self.line as i64);
        o.insert("message", self.message.clone());
        Value::Object(o)
    }
}
pub struct ValidationReport {
    pub total_records: usize,
    pub errors: Vec<ValidationMessage>,
    pub warnings: Vec<ValidationMessage>,
}
pub(crate) fn validate_corpus(
    a: &Action<'_>,
    cfg: &Config,
    sync: bool,
) -> Result<ValidationReport> {
    let mut report = ValidationReport {
        total_records: 0,
        errors: Vec::new(),
        warnings: Vec::new(),
    };
    for domain in domain_names(cfg) {
        let path = a.path(&domain)?;
        let Ok(data) = std::fs::read(path) else {
            continue;
        };
        for (index, line) in data.split(|b| *b == b'\n').enumerate() {
            let line = Value::from_go_bytes(line.to_vec());
            let line = line.as_str().expect("Go string view").trim();
            if line.is_empty() {
                continue;
            }
            report.total_records += 1;
            let error = |message: String| ValidationMessage {
                domain: domain.clone(),
                line: index + 1,
                message,
            };
            let mut value = match json::parse(line) {
                Ok(value) => value,
                Err(_) => {
                    report
                        .errors
                        .push(error("Invalid JSON: failed to parse".into()));
                    continue;
                }
            };
            let row = records::obj(Some(&value));
            if let Some(kind) = row.get("type").and_then(Value::as_str) {
                if let Some(def) = a.registry.get(kind) {
                    let aliases = def.aliases.clone();
                    if let Some(row) = value.as_object_mut() {
                        crate::registry::apply_aliases(row, &aliases);
                    }
                } else {
                    if a.ctx.globals.allow_unknown_types && !sync {
                        continue;
                    }
                    let id = if let Some(id) = row.get("id").and_then(Value::as_str) {
                        format!(" (id={id})")
                    } else {
                        String::new()
                    };
                    report.errors.push(error(format!("Unknown record type {}{id}. Register it under custom_types in config.yaml or remove the record.",records::go_quote(kind))));
                    continue;
                }
            }
            let row = records::obj(Some(&value));
            let legacy = row.contains_key("outcome") && !row.contains_key("outcomes");
            if legacy && !sync {
                let tick = char::from(96);
                report.warnings.push(error(format!("Legacy \"outcome\" field (singular); run {tick}luuna-atlas doctor --fix{tick} to migrate to \"outcomes[]\"")));
            } else {
                let issues = schema::validate_record_value(&value, &a.registry)?;
                if !issues.is_empty() {
                    report.errors.push(error(format!(
                        "Schema validation failed: {}",
                        schema::issues_text(&issues)
                    )));
                    continue;
                }
            }
            if a.ctx.globals.allow_domain_mismatch && !sync {
                continue;
            }
            let kind = text(row.get("type"));
            let kind = if kind.is_empty() { "<unknown>" } else { &kind };
            let id = text(row.get("id"));
            let id = if id.is_empty() {
                String::new()
            } else {
                format!(" (id={id})")
            };
            let rules = records::obj(records::obj(cfg.get("domains")).get(&domain));
            let allowed = records::list(rules.get("allowed_types"));
            if !allowed.is_empty() && !allowed.iter().any(|t| t == kind) {
                let tail = if sync {
                    " Fix the record or relax allowed_types in config.yaml."
                } else {
                    ""
                };
                report.errors.push(error(format!(
                    "type {}{id} is not allowed in domain {}. Allowed types: {}.{tail}",
                    records::go_quote(kind),
                    records::go_quote(&domain),
                    allowed.join(", ")
                )));
                if sync {
                    continue;
                }
            }
            let missing = schema::find_missing_domain_fields(
                row,
                &records::list(rules.get("required_fields")),
            );
            if !missing.is_empty() {
                let tail = if sync {
                    " Fix the record or relax required_fields in config.yaml."
                } else {
                    ""
                };
                report.errors.push(error(format!(
                    "record{id} missing required field(s) {} in domain {}.{tail}",
                    missing
                        .iter()
                        .map(|f| records::go_quote(f))
                        .collect::<Vec<_>>()
                        .join(", "),
                    records::go_quote(&domain)
                )));
            }
        }
    }
    Ok(report)
}
pub(crate) fn run_validate(a: &mut Action<'_>) -> Result<()> {
    let cfg = a.config()?;
    let report = validate_corpus(a, &cfg, false)?;
    let valid = report.errors.is_empty();
    if a.ctx.globals.json {
        let out = object([
            ("success", valid.into()),
            ("command", "validate".into()),
            ("valid", valid.into()),
            ("totalRecords", (report.total_records as i64).into()),
            ("totalErrors", (report.errors.len() as i64).into()),
            ("totalWarnings", (report.warnings.len() as i64).into()),
            (
                "errors",
                Value::Array(report.errors.iter().map(ValidationMessage::value).collect()),
            ),
            (
                "warnings",
                Value::Array(
                    report
                        .warnings
                        .iter()
                        .map(ValidationMessage::value)
                        .collect(),
                ),
            ),
        ]);
        a.output
            .stdout
            .push_str(&crate::CommandOutput::emit(&out).stdout);
    } else {
        let domains = domain_names(&cfg);
        let mut diagnostics = report
            .errors
            .iter()
            .map(|e| (e, false))
            .chain(report.warnings.iter().map(|e| (e, true)))
            .collect::<Vec<_>>();
        diagnostics.sort_by_key(|(e, w)| {
            (
                domains.iter().position(|d| d == &e.domain).unwrap_or(0),
                e.line,
                !*w,
            )
        });
        for (entry, warning) in diagnostics {
            let color = if warning { "yellow" } else { "red" };
            let prefix = format!("{}:{} - ", entry.domain, entry.line);
            if let Some(issues) = entry.message.strip_prefix("Schema validation failed: ") {
                a.output.stderr.push_str(&format!(
                    "{}\n",
                    a.styled(color, &format!("{prefix}Schema validation failed:"))
                ));
                for issue in issues.split("; ") {
                    a.output
                        .stderr
                        .push_str(&format!("{}\n", a.styled(color, &format!("  {issue}"))));
                }
            } else {
                a.output.stderr.push_str(&format!(
                    "{}\n",
                    a.styled(color, &format!("{prefix}{}", entry.message))
                ));
            }
        }
        let warning = if valid && !report.warnings.is_empty() {
            format!(", {} warning(s)", report.warnings.len())
        } else {
            String::new()
        };
        let color = if !valid {
            "red"
        } else if !report.warnings.is_empty() {
            "yellow"
        } else {
            "green"
        };
        a.output.stdout.push_str(&format!(
            "{}\n",
            a.styled(
                color,
                &format!(
                    "{} records validated, {} errors found{warning}",
                    report.total_records,
                    report.errors.len()
                )
            )
        ));
    }
    if !valid {
        a.output.exit_code = 1;
    }
    Ok(())
}
