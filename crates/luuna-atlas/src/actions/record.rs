//! Native recording and shared command-output conventions.
use crate::{
    commands,
    config::{self, Config},
    git, hooks,
    json::{Object, Value},
    legacy,
    options::Context,
    records,
    registry::{Registry, TypeDefinition},
    schema, store, style, timefmt, AtlasError, AtlasStore, CommandOutput, Result,
};
pub(crate) use records::text_value as text;
use sha2::{Digest, Sha256};

#[derive(Clone, Debug, Default)]
pub struct RecordOptions {
    pub content: Option<String>,
    pub flags: Vec<(String, String)>,
}
pub(crate) fn get(flags: &[(String, String)], key: &str) -> String {
    flags
        .iter()
        .rev()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.clone())
        .unwrap_or_default()
}
pub(crate) fn has(flags: &[(String, String)], key: &str) -> bool {
    flags.iter().any(|(k, _)| k == key)
}
pub(crate) fn boolean(flags: &[(String, String)], key: &str) -> bool {
    has(flags, key) && get(flags, key) != "false"
}
pub(crate) fn many(flags: &[(String, String)], key: &str) -> Vec<String> {
    flags
        .iter()
        .filter(|(k, _)| k == key)
        .map(|(_, v)| v.clone())
        .collect()
}
pub(crate) fn strings(values: Vec<String>) -> Value {
    Value::Array(values.into_iter().map(Value::String).collect())
}
pub(crate) fn message(text: impl Into<String>) -> AtlasError {
    AtlasError::Message(text.into())
}
pub(crate) struct Action<'a> {
    pub ctx: &'a Context,
    pub registry: Registry,
    pub output: CommandOutput,
    pub level: i32,
}
impl Action<'_> {
    pub fn config(&self) -> Result<Config> {
        config::read_config(&self.ctx.root.join(".luuna/atlas"))
    }
    pub fn path(&self, domain: &str) -> Result<std::path::PathBuf> {
        AtlasStore::open(&self.ctx.root)?.expertise_path(domain)
    }
    pub fn json(&mut self, command: &str, mut fields: Object) {
        fields.insert_unordered("success", true);
        fields.insert_unordered("command", command);
        self.output
            .stdout
            .push_str(&CommandOutput::emit(&fields).stdout);
    }
    pub fn human(&mut self, line: impl AsRef<str>) {
        if !self.ctx.globals.quiet {
            self.output.stdout.push_str(line.as_ref());
            self.output.stdout.push('\n');
        }
    }
    pub fn styled(&self, name: &str, text: &str) -> String {
        style::style(self.level, name, text)
    }
    pub fn success(&mut self, text: &str) {
        self.human(format!(
            "{} {}",
            self.styled("brand", "✓"),
            self.styled("brand", text)
        ));
    }
    pub fn warning(&mut self, text: &str) {
        self.output
            .stderr
            .push_str(&style::warning_line(self.level, text));
    }
    pub fn error(&mut self, command: &str, text: &str) {
        self.output.exit_code = 1;
        if self.ctx.globals.json {
            self.output
                .stderr
                .push_str(&CommandOutput::failure(command, text).stderr);
        } else {
            self.output
                .stderr
                .push_str(&style::error_line(self.level, text));
        }
    }
}
pub(crate) fn run(
    ctx: &Context,
    command: &str,
    f: impl FnOnce(&mut Action<'_>) -> Result<()>,
) -> CommandOutput {
    // The foundation prelude does not export its registry. Repeat its small
    // migration/read sequence once, retaining that exact config snapshot.
    let (notice, cfg) = match legacy::migrate_legacy_store(&ctx.root) {
        Ok(notice) => (
            notice,
            config::read_config(&ctx.root.join(".luuna/atlas")).ok(),
        ),
        Err(_) => (None, None),
    };
    let registry = match Registry::from_config(cfg.as_ref()) {
        Ok(r) => r,
        Err(error) => {
            let out = if ctx.globals.json {
                CommandOutput::failure("init", &format!("Config error: {error}"))
            } else {
                let tick = char::from(96);
                CommandOutput::plain_failure(&format!("Config error: {error}\nEdit .luuna/atlas/config.yaml to resolve, or run {tick}luuna-atlas doctor{tick} for details."))
            };
            return commands::with_notice(notice, out);
        }
    };
    let mut action = Action {
        ctx,
        registry,
        level: style::color_level(&ctx.env),
        output: CommandOutput {
            exit_code: 0,
            stdout: String::new(),
            stderr: String::new(),
        },
    };
    if let Err(error) = f(&mut action) {
        action.error(command, &error.to_string());
    }
    commands::with_notice(notice, action.output)
}
pub(crate) fn is_named(def: &TypeDefinition) -> bool {
    if def.builtin {
        def.name != "convention" && def.name != "failure"
    } else {
        def.dedup_key != "content_hash"
    }
}
pub(crate) fn id_text(record: &Object, fallback: &str) -> String {
    let id = text(record.get("id"));
    if id.is_empty() {
        fallback.into()
    } else {
        id
    }
}
pub(crate) fn number_flag(raw: &str, flag: &str) -> Result<f64> {
    let bytes = raw.as_bytes();
    let mut i = 0;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }
    let digits = i > 0;
    if bytes.get(i) == Some(&b'.') {
        i += 1;
        let first = i;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
        }
        if i == first {
            i = usize::MAX;
        }
    }
    let value = raw
        .parse::<f64>()
        .ok()
        .filter(|n| n.is_finite() && *n >= 0.0);
    if let Some(value) = value.filter(|_| digits && i == bytes.len()) {
        Ok(value)
    } else {
        Err(message(format!(
            "--{flag} must be a non-negative number (got {}).",
            records::go_quote(raw)
        )))
    }
}
pub(crate) fn outcome(flags: &[(String, String)]) -> Result<Option<Object>> {
    let status = get(flags, "outcome-status");
    if status.is_empty() {
        return Ok(None);
    }
    let mut out = Object::new();
    out.insert_unordered("status", status);
    if has(flags, "outcome-duration") {
        out.insert_unordered(
            "duration",
            number_flag(&get(flags, "outcome-duration"), "outcome-duration")?,
        );
    }
    for key in ["test-results", "agent"] {
        let v = get(flags, &format!("outcome-{key}"));
        if !v.is_empty() {
            out.insert_unordered(key.replace('-', "_"), v);
        }
    }
    out.remember_order(["status", "duration", "test_results", "agent"]);
    Ok(Some(out))
}
pub(crate) fn utf16_slice(text: &str, n: usize) -> String {
    String::from_utf16_lossy(&text.encode_utf16().take(n).collect::<Vec<_>>())
}
fn derived_name(text: &str) -> String {
    let lower = records::go_lower(text);
    let mut words = Vec::new();
    let mut word = String::new();
    for ch in lower.chars() {
        if ch.is_ascii_lowercase() || ch.is_ascii_digit() {
            word.push(ch);
        } else if !word.is_empty() {
            words.push(std::mem::take(&mut word));
        }
    }
    if !word.is_empty() {
        words.push(word);
    }
    let mut name = words.into_iter().take(6).collect::<Vec<_>>().join("-");
    name.truncate(name.len().min(60));
    name = name.trim_end_matches('-').into();
    if name.is_empty() {
        name = "record".into();
    }
    let hash = Sha256::digest(text.trim().as_bytes());
    name.truncate(name.len().min(53));
    format!(
        "{}-{:02x}{:02x}{:02x}",
        name.trim_end_matches('-'),
        hash[0],
        hash[1],
        hash[2]
    )
}
fn derived_title(text: &str) -> String {
    let mut text = text.trim();
    for (index, ch) in text.char_indices() {
        if index > 0
            && matches!(ch, ' ' | '\n' | '\t')
            && matches!(text.as_bytes()[index - 1], b'.' | b'!' | b'?')
        {
            text = text[..index].trim();
            break;
        }
    }
    if text.encode_utf16().count() > 80 {
        format!(
            "{}...",
            utf16_slice(text, 77).trim_end_matches([' ', '\t', '\r', '\n'])
        )
    } else {
        text.into()
    }
}
pub(crate) fn retry(
    domain: &str,
    content: &str,
    options: &RecordOptions,
    missing: &[String],
) -> String {
    let flags = &options.flags;
    let quote = |s: &str| crate::json::to_compact_string(&Value::from(s)).unwrap();
    let mut parts = vec!["luuna-atlas record".into(), domain.into()];
    if !content.is_empty() {
        parts.push(quote(content));
    }
    let typ = get(flags, "type");
    if !typ.is_empty() {
        parts.push(format!("--type {typ}"));
    }
    let class = get(flags, "classification");
    if !class.is_empty() && class != "tactical" {
        parts.push(format!("--classification {class}"));
    }
    for field in [
        "name",
        "content",
        "description",
        "resolution",
        "title",
        "rationale",
        "files",
    ] {
        let value = get(flags, field);
        if !value.is_empty() {
            parts.push(format!("--{field} {}", quote(&value)));
        }
    }
    for value in many(flags, "dir-anchor") {
        parts.push(format!("--dir-anchor {}", quote(&value)));
    }
    let tags = get(flags, "tags");
    if !tags.is_empty() {
        parts.push(format!("--tags {}", quote(&tags)));
    }
    for field in [
        "commit", "issue", "file", "bead", "missions", "gh", "linear",
    ] {
        let value = get(flags, &format!("evidence-{field}"));
        if !value.is_empty() {
            parts.push(format!(
                "--evidence-{field} {}",
                if field == "commit" {
                    value
                } else {
                    quote(&value)
                }
            ));
        }
    }
    for field in missing {
        parts.push(format!(
            "--{} {}",
            field.replace('_', "-"),
            quote(&format!("<{field}>"))
        ));
    }
    parts.join(" ")
}
pub(crate) fn merge_outcomes(old: &Object, new: &Object) -> Object {
    let mut result = new.clone();
    let outcomes = records::record_list(old.get("outcomes"))
        .into_iter()
        .chain(records::record_list(new.get("outcomes")))
        .cloned()
        .map(Value::Object)
        .collect::<Vec<_>>();
    if !outcomes.is_empty() {
        result.insert_unordered("outcomes", Value::Array(outcomes));
    }
    result
}
pub(crate) fn payload(domain: &str, record: &Object, action: Option<&str>) -> Value {
    let mut p = Object::new();
    p.insert("domain", domain);
    p.insert("record", record.clone());
    if let Some(action) = action {
        p.insert("action", action);
    }
    Value::Object(p)
}
pub fn run_record(ctx: &Context, domain: &str, options: &RecordOptions) -> CommandOutput {
    run(ctx, "record", |a| record(a, domain, options))
}

fn record(a: &mut Action<'_>, domain: &str, options: &RecordOptions) -> Result<()> {
    let flags = &options.flags;
    let cfg = a.config()?;
    if !records::obj(cfg.get("domains")).contains_key(domain) {
        config::add_domain(&a.ctx.root.join(".luuna/atlas"), domain)?;
        a.success(&format!(
            "Auto-created domain {}",
            records::go_quote(domain)
        ));
    }
    let typ = get(flags, "type");
    if typ.is_empty() {
        return Err(message(format!(
            "--type is required ({})",
            a.registry.names().join(", ")
        )));
    }
    let def = a.registry.get(&typ).cloned().ok_or_else(|| {
        message(format!(
            "Unknown record type {}. Available: {}.",
            records::go_quote(&typ),
            a.registry.names().join(", ")
        ))
    })?;
    let content = options.content.as_deref().unwrap_or_default();
    let mut fields = flags.clone();
    let mut fill = |key: &str, value: String| {
        if get(&fields, key).is_empty() {
            fields.push((key.into(), value));
        }
    };
    if def.builtin {
        let description = get(flags, "description");
        let source = if description.trim().is_empty() {
            content
        } else {
            &description
        };
        if !source.trim().is_empty() {
            match typ.as_str() {
                "convention" if options.content.is_none() => fill("content", source.into()),
                "pattern" | "reference" | "guide" => fill("name", derived_name(source)),
                "decision" => {
                    fill("title", derived_title(source));
                    fill("rationale", source.into());
                }
                _ => {}
            }
        }
    }
    let class = get(flags, "classification");
    let mut r = Object::new();
    r.insert_unordered("type", typ.clone());
    r.insert_unordered(
        "classification",
        if class.is_empty() {
            "tactical".into()
        } else {
            class
        },
    );
    r.insert_unordered("recorded_at", timefmt::now_iso());
    let mut evidence = Object::new();
    let mut evidence_order = Vec::new();
    for key in [
        "commit", "issue", "file", "bead", "missions", "gh", "linear",
    ] {
        let v = get(flags, &format!("evidence-{key}"));
        if !v.is_empty() {
            evidence.insert_unordered(key, v);
            evidence_order.push(key);
        }
    }
    let mut auto_commit = false;
    if get(flags, "evidence-commit").is_empty() {
        let commit = git::get_current_commit(&a.ctx.cwd, &a.ctx.env);
        if !commit.is_empty() {
            evidence.insert_unordered("commit", commit);
            evidence_order.push("commit");
            auto_commit = true;
        }
    }
    if !evidence.is_empty() {
        evidence.remember_order(evidence_order);
        r.insert_unordered("evidence", evidence);
    }
    for key in ["tags", "relates-to", "supersedes"] {
        let list = records::split_csv(&get(flags, key));
        if !list.is_empty() {
            r.insert_unordered(key.replace('-', "_"), strings(list));
        }
    }
    if let Some(outcome) = outcome(flags)? {
        r.insert_unordered("outcomes", Value::Array(vec![Value::Object(outcome)]));
    }
    let mut dirs = git::normalize_dir_anchors(&many(flags, "dir-anchor"))?;
    if !dirs.is_empty() {
        dirs = records::unique(&dirs);
        dirs.sort();
    } else if !has(flags, "files") {
        dirs = git::infer_dir_anchors(&git::get_context_files(&a.ctx.cwd, &a.ctx.env));
    }
    if !dirs.is_empty() {
        r.insert_unordered("dir_anchors", strings(dirs));
    }
    let domain_cfg = records::obj(records::obj(cfg.get("domains")).get(domain));
    let allowed = records::list(domain_cfg.get("allowed_types"));
    if !a.ctx.globals.allow_domain_mismatch && !allowed.is_empty() && !allowed.contains(&typ) {
        a.error(
            "record",
            &format!(
                "type {} is not allowed in domain {}. Allowed types: {}.",
                records::go_quote(&typ),
                records::go_quote(domain),
                allowed.join(", ")
            ),
        );
        if !a.ctx.globals.json {
            let mut retry_options = options.clone();
            retry_options
                .flags
                .push(("type".into(), allowed[0].clone()));
            a.output.stderr.push_str(&format!(
                "{}\n",
                a.styled(
                    "dim",
                    &format!("  Retry: {}", retry(domain, content, &retry_options, &[]))
                )
            ));
        }
        return Ok(());
    }
    let disabled = if def.disabled {
        format!("type {} is disabled (declared in disabled_types). Records will still be written; consider migrating off this type.",records::go_quote(&typ))
    } else {
        String::new()
    };
    if !disabled.is_empty() && !a.ctx.globals.quiet && !a.ctx.globals.json {
        a.warning(&disabled);
    }
    let mut missing = Vec::new();
    for (required, names) in [(true, &def.required), (false, &def.optional)] {
        for field in names {
            let flag = field.replace('_', "-");
            let mut v = get(&fields, &flag);
            let fallback = def.builtin
                && ((typ == "convention" && field == "content")
                    || (["pattern", "reference", "guide"].contains(&typ.as_str())
                        && field == "description"));
            if v.is_empty() && fallback && options.content.is_some() && !has(flags, &flag) {
                v = content.into();
            }
            if v.is_empty() {
                if required {
                    missing.push(field.clone());
                }
                continue;
            }
            r.insert_unordered(
                field,
                if def.extracts_files && field == &def.files_field {
                    strings(records::split_csv(&v))
                } else {
                    Value::String(v)
                },
            );
        }
    }
    if def.extracts_files && !r.contains_key(&def.files_field) {
        let files = git::get_context_files(&a.ctx.cwd, &a.ctx.env);
        if !files.is_empty() {
            r.insert_unordered(&def.files_field, strings(files));
        }
    }
    let mut order: Vec<&str> = vec![
        "type",
        "classification",
        "recorded_at",
        "evidence",
        "tags",
        "relates_to",
        "supersedes",
        "outcomes",
        "dir_anchors",
    ];
    order.extend(def.required.iter().map(String::as_str));
    order.extend(def.optional.iter().map(String::as_str));
    if def.extracts_files {
        order.push(&def.files_field);
    }
    r.remember_order(order);
    if !missing.is_empty() {
        let base = format!(
            "{typ} records are missing required flag(s): {}.",
            missing
                .iter()
                .map(|f| format!("--{}", f.replace('_', "-")))
                .collect::<Vec<_>>()
                .join(", ")
        );
        let example = retry(domain, content, options, &missing);
        if a.ctx.globals.json {
            a.error("record", &format!("{base} Example: {example}"));
        } else {
            a.error("record", &base);
            a.output.stderr.push_str(&format!(
                "{}\n",
                a.styled("dim", &format!("  Retry: {example}"))
            ));
        }
        return Ok(());
    }
    let issues = schema::validate_record(&r, &a.registry)?;
    if !issues.is_empty() {
        if a.ctx.globals.json {
            a.error(
                "record",
                &format!(
                    "Schema validation failed: {}. Hint: {typ} records require: {}",
                    schema::issues_text(&issues),
                    def.required.join(", ")
                ),
            );
        } else {
            a.output.exit_code = 1;
            a.output.stderr.push_str(&format!(
                "{}\n",
                a.styled("red", "Error: record failed schema validation:")
            ));
            for issue in issues {
                a.output.stderr.push_str(&format!(
                    "{}\n",
                    a.styled("red", &format!("  {} {}", issue.path, issue.message))
                ));
            }
            a.output.stderr.push_str(&format!(
                "{}\n",
                a.styled(
                    "yellow",
                    &format!("Hint: {typ} records require: {}", def.required.join(", "))
                )
            ));
        }
        return Ok(());
    }
    if !a.ctx.globals.allow_domain_mismatch {
        let missing = schema::find_missing_domain_fields(
            &r,
            &records::list(domain_cfg.get("required_fields")),
        );
        if !missing.is_empty() {
            let quoted = missing
                .iter()
                .map(|v| crate::json::to_compact_string(&Value::from(v.clone())).unwrap())
                .collect::<Vec<_>>()
                .join(", ");
            a.error(
                "record",
                &format!(
                    "domain {} requires field(s) {quoted}.",
                    records::go_quote(domain)
                ),
            );
            if !a.ctx.globals.json {
                a.output.stderr.push_str(&format!(
                    "{}\n",
                    a.styled(
                        "dim",
                        &format!("  Retry: {}", retry(domain, content, options, &missing))
                    )
                ));
            }
            return Ok(());
        }
    }
    let path = a.path(domain)?;
    let existing = store::read_records(&path, &a.registry, a.ctx.globals.allow_unknown_types)?;
    let duplicate = records::find_duplicate(&existing, &r, &a.registry);
    let force = boolean(flags, "force");
    if let Some(index) = duplicate.filter(|_| !is_named(&def)) {
        let id = id_text(&existing[index], &format!("#{}", index + 1));
        return Err(message(format!("duplicate {typ} already exists in {domain} ({id}); not recorded. If it helped, confirm it instead: luuna-atlas outcome {domain} {id} --status success")));
    }
    let mut warnings = Vec::new();
    let mut quality_warnings = Vec::new();
    let quality = text(records::obj(cfg.get("quality")).get("level"));
    let quality = if quality.is_empty() { "warn" } else { &quality };
    if duplicate.is_none() && quality != "off" {
        let near = records::find_near_duplicates(&existing, &r, &a.registry);
        if !near.is_empty() && !force {
            return Err(message(near_message(domain, &near)));
        }
        let issues = records::check_record_quality(&r, auto_commit);
        if !issues.is_empty() && quality == "error" && !force {
            return Err(message(format!("record failed quality checks (quality.level: error): {} Pass --force to record anyway.",issues.join(" "))));
        }
        quality_warnings = issues
            .into_iter()
            .map(|s| format!("quality: {s}"))
            .collect();
    }
    if !disabled.is_empty() {
        warnings.push(disabled.clone());
    }
    warnings.extend(quality_warnings.iter().cloned());
    if boolean(flags, "dry-run") {
        let would = if duplicate.is_some() && !force {
            "updated"
        } else {
            "created"
        };
        if a.ctx.globals.json {
            let mut data = Object::new();
            data.insert("action", "dry-run");
            data.insert("wouldDo", would);
            data.insert("domain", domain);
            data.insert("type", typ);
            data.insert("record", r);
            if !warnings.is_empty() {
                data.insert("warnings", strings(warnings));
            }
            a.json("record", data);
        } else {
            a.success(&if would == "updated" {
                format!("Dry-run: Would update existing {typ} in {domain}")
            } else {
                format!("Dry-run: Would create {typ} in {domain}")
            });
            a.human(a.styled("dim", "  Run without --dry-run to apply changes."));
            for warning in quality_warnings {
                a.warning(&warning);
            }
        }
        return Ok(());
    }
    let pre = hooks::run_hooks(
        a.ctx,
        Some(&cfg),
        "pre-record",
        &payload(domain, &r, None),
        &mut a.output.stderr,
    )?;
    if pre.blocked {
        return Err(message(if pre.block_reason.is_empty() {
            "pre-record hook blocked the write".into()
        } else {
            pre.block_reason
        }));
    }
    warnings.extend(pre.warnings);
    if pre.ran_any {
        if let Some(mutated) = pre
            .payload
            .as_object()
            .and_then(|p| p.get("record"))
            .and_then(Value::as_object)
        {
            let issues = schema::validate_record(mutated, &a.registry)?;
            if !issues.is_empty() {
                return Err(message(format!(
                    "pre-record hook produced an invalid record: {}",
                    schema::issues_text(&issues)
                )));
            }
            r = mutated.clone();
        }
    }
    let mut action = "created";
    let mut index = None;
    store::with_file_lock(&path, || {
        let mut current =
            store::read_records(&path, &a.registry, a.ctx.globals.allow_unknown_types)?;
        if let Some(d) = records::find_duplicate(&current, &r, &a.registry)
            .filter(|_| !(force && is_named(&def)))
        {
            index = Some(d);
            if is_named(&def) {
                let before = current.clone();
                records::check_outcomes_collection(&path, &current[d], true)?;
                r = merge_outcomes(&current[d], &r);
                current[d] = r.clone();
                action = "updated";
                records::check_lossless_rewrite(&path, &a.registry, &[d])?;
                records::check_retained_ids(
                    &path,
                    before
                        .iter()
                        .enumerate()
                        .filter(|(index, _)| *index != d)
                        .map(|(index, record)| (record, &current[index])),
                )?;
                records::check_retained_numbers(&path, before.iter().zip(current.iter()))?;
                records::assign_record_ids(&mut current, &a.registry)?;
                store::write_records(&path, &mut current, &a.registry)?;
                r = current[d].clone();
                return Ok(());
            }
            action = "skipped";
            return Ok(());
        }
        store::append_record(&path, &mut r, &a.registry)
    })?;
    if action != "skipped" {
        let post = hooks::run_hooks(
            a.ctx,
            Some(&cfg),
            "post-record",
            &payload(domain, &r, Some(action)),
            &mut a.output.stderr,
        )?;
        warnings.extend(post.warnings);
    }
    if a.ctx.globals.json {
        let mut data = Object::new();
        data.insert("action", action);
        data.insert("domain", domain);
        data.insert("type", typ);
        if action != "skipped" {
            data.insert("record", r);
        }
        if let Some(index) = index {
            data.insert("index", (index + 1) as i64);
        }
        if !warnings.is_empty() {
            data.insert("warnings", strings(warnings));
        }
        a.json("record", data);
    } else {
        match action {
            "created"=>a.success(&format!("Recorded {typ} in {domain}")),
            "updated"=>a.success(&format!("Updated existing {typ} in {domain} (record #{})",index.unwrap()+1)),
            _=>a.human(a.styled("yellow",&format!("Duplicate {typ} already exists in {domain} (record #{}). Use --force to add anyway.",index.unwrap()+1))),
        }
        for warning in warnings {
            if warning != disabled {
                a.warning(&warning);
            }
        }
    }
    Ok(())
}
fn near_message(domain: &str, near: &[records::NearDuplicate]) -> String {
    let mut lines = vec![format!("similar record(s) already exist in {domain}:")];
    for n in near {
        let mut summary = String::new();
        for key in ["name", "title", "content", "description"] {
            let v = text(n.record.get(key));
            if !v.is_empty() {
                let mut was_space = false;
                for ch in v.chars() {
                    let space = matches!(ch, '\t' | '\n' | '\u{c}' | '\r' | ' ');
                    if !space || !was_space {
                        summary.push(if space { ' ' } else { ch });
                    }
                    was_space = space;
                }
                break;
            }
        }
        if summary.encode_utf16().count() > 100 {
            summary = format!("{}...", utf16_slice(&summary, 97));
        }
        let shared = if n.shared_anchors.is_empty() {
            String::new()
        } else {
            format!(", shares {}", n.shared_anchors.join(", "))
        };
        lines.push(format!(
            "  {} [{}] {summary} ({}% similar{shared})",
            id_text(&n.record, "?"),
            text(n.record.get("type")),
            (n.similarity * 100.0 + 0.5).floor() as i64
        ));
    }
    let id = id_text(&near[0].record, "<id>");
    lines.push(format!("Replace it: add --supersedes {id}. Update it: luuna-atlas edit {domain} {id}. Confirm it helped: luuna-atlas outcome {domain} {id} --status success. Record anyway: add --force."));
    lines.join("\n")
}

pub(crate) fn domain_names(cfg: &Config) -> Vec<String> {
    records::obj(cfg.get("domains"))
        .keys()
        .map(str::to_owned)
        .collect()
}
