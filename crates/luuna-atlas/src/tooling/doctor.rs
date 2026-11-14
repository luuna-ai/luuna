//! Health checks preserve Go's diagnostic order and pre-fix summary.
use crate::{
    actions::record::{boolean, domain_names, text, Action},
    json::{self, Object, Value},
    lifecycle::health,
    records, registry, schema, store, Result,
};
use std::{collections::BTreeMap, fs, path::Path};
#[derive(Clone)]
pub(super) struct Check {
    pub name: &'static str,
    pub status: &'static str,
    pub message: String,
    pub fixable: bool,
    pub details: Vec<String>,
}
impl Check {
    fn new(
        name: &'static str,
        status: &'static str,
        message: impl Into<String>,
        fixable: bool,
        details: Vec<String>,
    ) -> Self {
        Self {
            name,
            status,
            message: message.into(),
            fixable,
            details,
        }
    }
    fn value(&self) -> Value {
        let mut o = Object::new();
        o.insert("name", self.name);
        o.insert("status", self.status);
        o.insert("message", self.message.clone());
        o.insert("fixable", self.fixable);
        o.insert(
            "details",
            Value::Array(self.details.iter().cloned().map(Value::String).collect()),
        );
        o.remember_order(["name", "status", "message", "fixable", "details"]);
        o.into()
    }
}
fn check(
    name: &'static str,
    pass: &str,
    fail: &'static str,
    count_label: &str,
    fixable: bool,
    details: Vec<String>,
) -> Check {
    if details.is_empty() {
        Check::new(name, "pass", pass, fixable, details)
    } else {
        Check::new(
            name,
            fail,
            format!("{} {count_label}", details.len()),
            fixable,
            details,
        )
    }
}
struct Line {
    domain: String,
    n: usize,
    raw: Option<Object>,
    valid: bool,
}
pub(super) fn path_exists(cwd: &Path, path: &str) -> bool {
    let p = Path::new(path);
    if p.is_absolute() {
        p.exists()
    } else {
        cwd.join(p).exists()
    }
}
fn entries(dir: &Path) -> Vec<String> {
    let mut names: Vec<_> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(std::result::Result::ok)
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}
pub(crate) fn run(a: &mut Action<'_>, flags: &[(String, String)]) -> Result<()> {
    let config_check = if !a.ctx.root.join(".luuna/atlas").exists() {
        Check::new(
            "config",
            "fail",
            "No .luuna/atlas/ directory found",
            false,
            Vec::new(),
        )
    } else {
        match a.config() {
            Ok(_) => Check::new("config", "pass", "Config is valid", false, Vec::new()),
            Err(e) => Check::new(
                "config",
                "fail",
                format!("Config error: {e}"),
                false,
                Vec::new(),
            ),
        }
    };
    if config_check.status == "fail" {
        if a.ctx.globals.json {
            let mut o = Object::new();
            o.insert("success", false);
            o.insert("command", "doctor");
            o.insert("checks", Value::Array(vec![config_check.value()]));
            let mut sum = Object::new();
            sum.insert_unordered("pass", 0);
            sum.insert_unordered("warn", 0);
            sum.insert_unordered("fail", 1);
            o.insert("summary", sum);
            a.output.stdout += &crate::CommandOutput::emit(&o).stdout;
        } else {
            a.human("Atlas Doctor");
            a.output.stderr += &format!(
                "  {} {}\n",
                a.styled("red", "✗"),
                a.styled("red", &config_check.message)
            );
            a.human("\n0 passed, 0 warnings, 1 failed");
        }
        a.output.exit_code = 1;
        return Ok(());
    }
    let cfg = a.config()?;
    let domains = domain_names(&cfg);
    let mut checks = vec![config_check];
    let mut lines = Vec::new();
    for domain in &domains {
        let Ok(path) = a.path(domain) else { continue };
        let Ok(bytes) = fs::read(path) else { continue };
        for (n, line) in bytes.split(|b| *b == b'\n').enumerate() {
            let line = String::from_utf8_lossy(line);
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let parsed = json::parse(line);
            lines.push(Line {
                domain: domain.clone(),
                n: n + 1,
                raw: parsed.as_ref().ok().and_then(Value::as_object).cloned(),
                valid: parsed.is_ok(),
            });
        }
    }
    let details = lines
        .iter()
        .filter(|l| !l.valid)
        .map(|l| format!("{}:{} - Invalid JSON", l.domain, l.n))
        .collect();
    checks.push(check(
        "jsonl-integrity",
        "All JSONL lines are valid JSON",
        "fail",
        "invalid JSON line(s) found",
        true,
        details,
    ));
    let details = lines
        .iter()
        .filter(|l| {
            l.valid
                && l.raw
                    .as_ref()
                    .is_some_and(|r| r.contains_key("outcome") && !r.contains_key("outcomes"))
        })
        .map(|l| {
            format!(
                "{}:{} - legacy \"outcome\" field (singular); should be \"outcomes[]\"",
                l.domain, l.n
            )
        })
        .collect();
    checks.push(check(
        "legacy-outcome",
        "No legacy \"outcome\" fields on disk",
        "warn",
        "record(s) with legacy \"outcome\" field on disk",
        true,
        details,
    ));
    let mut details = Vec::new();
    for l in lines.iter_mut().filter(|l| l.valid) {
        let mut raw = l.raw.clone().unwrap_or_default();
        if let Some(t) = raw.get("type").and_then(Value::as_str) {
            if let Some(def) = a.registry.get(t) {
                let aliases = def.aliases.clone();
                registry::apply_aliases(&mut raw, &aliases);
            } else {
                continue;
            }
        }
        let issues = schema::validate_record(&raw, &a.registry)?;
        if !issues.is_empty() {
            details.push(format!(
                "{}:{} - {}",
                l.domain,
                l.n,
                schema::issues_text(&issues)
            ));
        }
        if l.raw.is_some() {
            l.raw = Some(raw);
        }
    }
    checks.push(check(
        "schema-validation",
        "All records pass schema validation",
        "fail",
        "record(s) failed schema validation",
        true,
        details,
    ));
    let details = lines
        .iter()
        .filter_map(|l| {
            let r = l.raw.as_ref()?;
            let typ = r.get("type")?.as_str()?;
            if a.registry.get(typ).is_some() {
                return None;
            }
            let id = r
                .get("id")
                .and_then(Value::as_str)
                .map(|s| format!(" [{s}]"))
                .unwrap_or_default();
            Some(format!(
                "{}:{}{id} - unknown type {}",
                l.domain,
                l.n,
                records::go_quote(typ)
            ))
        })
        .collect();
    checks.push(check(
        "unknown-types",
        "All record types are registered",
        "fail",
        "record(s) with unregistered types",
        false,
        details,
    ));
    let mut domain_rows = BTreeMap::new();
    let mut counts: BTreeMap<String, usize> = a
        .registry
        .names()
        .iter()
        .map(|s| (s.to_string(), 0))
        .collect();
    for d in &domains {
        let rows = store::read_records(&a.path(d)?, &a.registry, true)?;
        for r in &rows {
            if let Some(n) = counts.get_mut(&text(r.get("type"))) {
                *n += 1;
            }
        }
        domain_rows.insert(d.clone(), rows);
    }
    let (mut builtins, mut customs, mut disabled) = (0, 0, 0);
    let mut details = Vec::new();
    for name in a.registry.names() {
        let def = a.registry.get(name).unwrap();
        let mut label = if def.builtin {
            builtins += 1;
            "built-in".to_owned()
        } else {
            customs += 1;
            "custom".to_owned()
        };
        if def.disabled {
            label += ", disabled";
            disabled += 1;
        }
        let n = counts[name];
        details.push(format!(
            "{name} ({label}): {n} record{}",
            if n == 1 { "" } else { "s" }
        ));
    }
    let mut msg = format!(
        "{} type(s) registered: {builtins} built-in, {customs} custom",
        builtins + customs
    );
    if disabled > 0 {
        msg += &format!(", {disabled} disabled")
    }
    checks.push(Check::new("type-registry", "pass", msg, false, details));
    let mut details = Vec::new();
    for d in &domains {
        for issue in schema::find_incompatible_required_fields(&cfg, d, &a.registry) {
            details.push(format!("domain {} requires field {}, but no allowed type accepts it (allowed: {}). Declare a custom_type with {} in required/optional, or remove {} from required_fields.",records::go_quote(d),records::go_quote(&issue.field),issue.allowed_types.join(", "),records::go_quote(&issue.field),records::go_quote(&issue.field)));
        }
    }
    checks.push(check(
        "domain-rules-compatibility",
        "All domain required_fields are compatible with allowed types",
        "fail",
        "domain required_field(s) incompatible with allowed types",
        false,
        details,
    ));
    let (mut conformance, mut violations) = (Vec::new(), Vec::new());
    let (mut total_records, mut total_bad) = (0, 0);
    for d in &domains {
        let allowed = schema::get_allowed_types(&cfg, d);
        let required = schema::get_required_fields(&cfg, d);
        let (mut total, mut bad) = (0, 0);
        for l in lines.iter().filter(|l| l.domain == *d && l.valid) {
            let Some(raw) = &l.raw else { continue };
            total += 1;
            let typ = raw
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or("<unknown>");
            let id = raw.get("id").and_then(Value::as_str).unwrap_or("<no-id>");
            let mut record_bad = false;
            if !allowed.is_empty() && !allowed.iter().any(|t| t == typ) {
                record_bad = true;
                violations.push(format!(
                    "{d}:{} [{id}] type {} not in allowed_types [{}]",
                    l.n,
                    records::go_quote(typ),
                    allowed.join(", ")
                ));
            }
            let missing = schema::find_missing_domain_fields(raw, &required);
            if !missing.is_empty() {
                record_bad = true;
                violations.push(format!(
                    "{d}:{} [{id}] ({typ}) missing required field(s): {}",
                    l.n,
                    missing.join(", ")
                ));
            }
            if record_bad {
                bad += 1;
            }
        }
        conformance.push(format!(
            "{d} ({}): {}/{} conforming, {bad} violation{}",
            if allowed.is_empty() && required.is_empty() {
                "no rules"
            } else {
                "rules"
            },
            total - bad,
            total,
            if bad == 1 { "" } else { "s" }
        ));
        total_records += total;
        total_bad += bad;
    }
    checks.push(Check::new(
        "domain-conformance",
        "pass",
        format!(
            "{}/{} record(s) conform to domain rules",
            total_records - total_bad,
            total_records
        ),
        false,
        conformance,
    ));
    checks.push(if violations.is_empty() {
        Check::new(
            "domain-violations",
            "pass",
            "All records conform to domain rules",
            false,
            Vec::new(),
        )
    } else {
        Check::new(
            "domain-violations",
            "fail",
            format!("{total_bad} record(s) violate domain rules (allowed_types / required_fields)"),
            false,
            violations,
        )
    });
    let shelf = records::obj(records::obj(cfg.get("classification_defaults")).get("shelf_life"));
    let now = chrono::Utc::now();
    let mut details = Vec::new();
    for d in &domains {
        for r in &domain_rows[d] {
            if health::is_record_stale(r, now, shelf) {
                details.push(format!(
                    "{d}: stale {} ({})",
                    text(r.get("type")),
                    text(r.get("classification"))
                ));
            }
        }
    }
    checks.push(check(
        "stale-records",
        "No stale records",
        "warn",
        "stale record(s) found",
        true,
        details,
    ));
    let expertise = a.ctx.root.join(".luuna/atlas/expertise");
    let mut details = Vec::new();
    for file in entries(&expertise) {
        if file.ends_with(".jsonl") {
            let d = file.replacen(".jsonl", "", 1);
            if !domains.contains(&d) {
                details.push(format!(
                    "File {} exists but domain {} is not in config",
                    records::go_quote(&file),
                    records::go_quote(&d)
                ));
            }
        }
    }
    for d in &domains {
        if !a.path(d)?.exists() {
            details.push(format!(
                "Domain {} in config but no JSONL file exists",
                records::go_quote(d)
            ));
        }
    }
    checks.push(check(
        "orphaned-domains",
        "No orphaned domains",
        "warn",
        "orphaned domain issue(s)",
        true,
        details,
    ));
    let mut details = Vec::new();
    for d in &domains {
        let rows = &domain_rows[d];
        for n in 1..rows.len() {
            if let Some(duplicate) = records::find_duplicate(&rows[..n], &rows[n], &a.registry) {
                details.push(format!(
                    "{d}: duplicate {} at index {} (matches #{})",
                    text(rows[n].get("type")),
                    n + 1,
                    duplicate + 1
                ));
            }
        }
    }
    checks.push(check(
        "duplicates",
        "No duplicates",
        "warn",
        "duplicate record(s) found",
        false,
        details,
    ));
    let mut details = Vec::new();
    for d in &domains {
        for r in &domain_rows[d] {
            let id = text(r.get("id"));
            let id = if id.is_empty() { id } else { format!("[{id}]") };
            for (key, label) in [
                ("files", "files[] path not found"),
                ("dir_anchors", "dir_anchors[] path not found"),
            ] {
                for p in records::list(r.get(key)) {
                    if !path_exists(&a.ctx.cwd, &p) {
                        details.push(format!("{d}{id}: {label}: {p}"));
                    }
                }
            }
            let p = text(records::obj(r.get("evidence")).get("file"));
            if !p.is_empty() && !path_exists(&a.ctx.cwd, &p) {
                details.push(format!("{d}{id}: evidence.file not found: {p}"));
            }
        }
    }
    checks.push(check(
        "file-anchors",
        "All file anchors resolve to existing paths",
        "warn",
        "broken file anchor(s) found",
        true,
        details,
    ));
    let mut details = Vec::new();
    let mut worst = "pass";
    let gov = records::obj(cfg.get("governance"));
    for d in &domains {
        let n = domain_rows[d].len();
        for (field, label, status) in [
            ("hard_limit", "over hard limit of", "fail"),
            ("warn_entries", "over warn threshold of", "warn"),
            ("max_entries", "approaching limit of", "warn"),
        ] {
            if n as f64 >= records::number(gov.get(field)) {
                details.push(format!(
                    "{d}: {n} records ({label} {})",
                    fmt_value(gov.get(field).unwrap_or(&Value::Null))
                ));
                if worst != "fail" {
                    worst = status
                }
                break;
            }
        }
    }
    checks.push(if details.is_empty() {
        Check::new(
            "governance",
            "pass",
            "All domains within governance limits",
            false,
            details,
        )
    } else {
        Check::new(
            "governance",
            worst,
            format!("{} domain(s) over governance thresholds", details.len()),
            false,
            details,
        )
    });
    let anchor = records::obj(cfg.get("decay")).get("anchor_validity");
    let decay = if anchor.is_none() || matches!(anchor, Some(Value::Null)) {
        Check::new(
            "decay-config",
            "pass",
            "decay.anchor_validity uses defaults",
            false,
            Vec::new(),
        )
    } else {
        let errors = anchor_errors(records::obj(anchor));
        if errors.is_empty() {
            Check::new(
                "decay-config",
                "pass",
                "decay.anchor_validity is valid",
                false,
                Vec::new(),
            )
        } else {
            Check::new(
                "decay-config",
                "fail",
                format!(
                    "decay.anchor_validity is invalid ({} issue{})",
                    errors.len(),
                    if errors.len() == 1 { "" } else { "s" }
                ),
                false,
                errors,
            )
        }
    };
    checks.push(decay);
    let scripts: Vec<_> = records::list(records::obj(cfg.get("hooks")).get("pre-compact"))
        .into_iter()
        .filter(|s| !s.trim().is_empty())
        .collect();
    let msg = if scripts.is_empty() {
        "compact-summarizer: not configured (mechanical merge in use)".into()
    } else {
        format!(
            "compact-summarizer: {} pre-compact hook{} registered",
            scripts.len(),
            if scripts.len() == 1 { "" } else { "s" }
        )
    };
    checks.push(Check::new(
        "compact-summarizer",
        "pass",
        msg,
        false,
        scripts,
    ));
    checks.push(Check::new(
        "upgrade",
        "pass",
        "Version 0.11.0 (unable to check registry)",
        false,
        Vec::new(),
    ));
    let fixed = if boolean(flags, "fix") {
        super::doctor_fix::run(a, &checks, &cfg)?
    } else {
        Vec::new()
    };
    emit(a, &checks, &fixed, boolean(flags, "fix"));
    Ok(())
}
fn emit(a: &mut Action<'_>, checks: &[Check], fixed: &[String], fix: bool) {
    let mut summary = Object::new();
    for status in ["pass", "warn", "fail"] {
        summary.insert_unordered(
            status,
            checks.iter().filter(|c| c.status == status).count() as i64,
        );
    }
    if a.ctx.globals.json {
        let mut out = Object::new();
        out.insert("success", !checks.iter().any(|c| c.status == "fail"));
        out.insert("command", "doctor");
        out.insert(
            "checks",
            Value::Array(checks.iter().map(Check::value).collect()),
        );
        out.insert("summary", summary.clone());
        if fix {
            out.insert(
                "fixed",
                Value::Array(fixed.iter().cloned().map(Value::String).collect()),
            );
        }
        a.output.stdout += &crate::CommandOutput::emit(&out).stdout;
    } else {
        a.human("Atlas Doctor");
        for c in checks {
            let (style, icon) = match c.status {
                "warn" => ("yellow", "!"),
                "fail" => ("red", "✗"),
                _ => ("brand", "✓"),
            };
            a.human(format!("  {} {}", a.styled(style, icon), c.message));
            if c.status != "pass" {
                for d in &c.details {
                    a.human(format!("      {d}"));
                }
            }
        }
        a.human(format!(
            "\n{} passed, {} warning(s), {} failed",
            text(summary.get("pass")),
            text(summary.get("warn")),
            text(summary.get("fail"))
        ));
        if !fixed.is_empty() {
            a.human(format!("\n{}", a.styled("brand", "Fixed:")));
            for f in fixed {
                a.human(format!("  {} {f}", a.styled("brand", "✓")));
            }
        }
    }
    if checks.iter().any(|c| c.status == "fail") {
        a.output.exit_code = 1;
    }
}

fn anchor_errors(cfg: &Object) -> Vec<String> {
    let mut errors = Vec::new();
    for field in ["threshold", "grace_days"] {
        let Some(value) = cfg.get(field) else {
            continue;
        };
        let n = records::number(Some(value));
        if !(matches!(value, Value::Number(_))
            || matches!(value,Value::Integer(v) if *v<=i64::MAX as i128 && *v>=i64::MIN as i128))
            || !n.is_finite()
        {
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

fn fmt_value(value: &Value) -> String {
    enum Part<'a> {
        Value(&'a Value),
        Text(&'a str),
    }
    let mut out = String::new();
    let mut stack = vec![Part::Value(value)];
    while let Some(part) = stack.pop() {
        match part {
            Part::Text(text) => out.push_str(text),
            Part::Value(Value::Null) => out.push_str("<nil>"),
            Part::Value(Value::Array(items)) => {
                out.push('[');
                stack.push(Part::Text("]"));
                for (index, item) in items.iter().enumerate().rev() {
                    stack.push(Part::Value(item));
                    if index > 0 {
                        stack.push(Part::Text(" "));
                    }
                }
            }
            Part::Value(Value::Object(map)) => {
                out.push_str("map[");
                stack.push(Part::Text("]"));
                let mut entries = map.iter().collect::<Vec<_>>();
                entries.sort_by(|(a, _), (b, _)| map.key_bytes(a).cmp(map.key_bytes(b)));
                for (index, (key, value)) in entries.into_iter().enumerate().rev() {
                    stack.push(Part::Value(value));
                    stack.push(Part::Text(":"));
                    stack.push(Part::Text(map.key_text(key)));
                    if index > 0 {
                        stack.push(Part::Text(" "));
                    }
                }
            }
            Part::Value(value) => out.push_str(&records::format_js(Some(value))),
        }
    }
    out
}
