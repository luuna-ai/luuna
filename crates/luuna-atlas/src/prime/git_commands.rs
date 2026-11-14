//! Learn/diff/sync with explicit Git subprocess output.
use crate::{
    actions::record::{boolean, domain_names, get, has, message, text, Action},
    git,
    json::{self, Object, Value},
    lifecycle::health::{object, validate_corpus},
    options::Context,
    records, Result,
};
use std::{
    collections::BTreeMap,
    process::{Command, Stdio},
};
struct GitOutput {
    stdout: String,
    error: Option<String>,
    exit_stderr: Option<String>,
}
fn view(bytes: Vec<u8>) -> String {
    Value::from_go_bytes(bytes).as_str().unwrap().to_owned()
}
fn capture(ctx: &Context, args: &[&str]) -> GitOutput {
    let Some(program) = ctx.env.look_path("git") else {
        return GitOutput {
            stdout: String::new(),
            error: Some("exec: \"git\": executable file not found in $PATH".into()),
            exit_stderr: None,
        };
    };
    let mut c = Command::new(program);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        c.arg0("git");
    }
    c.args(args)
        .current_dir(&ctx.cwd)
        .env_clear()
        .envs(ctx.env.vars().iter().map(|(k, v)| (k, v)))
        .env("PWD", &ctx.cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    match c.output() {
        Ok(out) => {
            let failed = !out.status.success();
            GitOutput {
                stdout: view(out.stdout),
                error: if failed {
                    Some(
                        out.status
                            .code()
                            .map(|code| format!("exit status {code}"))
                            .unwrap_or_else(|| "git was stopped by a signal".into()),
                    )
                } else {
                    None
                },
                exit_stderr: if failed { Some(view(out.stderr)) } else { None },
            }
        }
        Err(error) => GitOutput {
            stdout: String::new(),
            error: Some(format!("exec: git: {error}")),
            exit_stderr: None,
        },
    }
}
fn failure(a: &mut Action<'_>, message: &str) {
    a.output.exit_code = 1;
    if a.ctx.globals.json {
        a.error("sync", message);
    } else {
        a.output
            .stderr
            .push_str(&format!("{}\n", a.styled("red", message)));
    }
}
fn git_required(a: &mut Action<'_>, command: &str) -> Result<bool> {
    if git::is_git_repo(&a.ctx.cwd, &a.ctx.env) {
        return Ok(true);
    }
    if a.ctx.globals.json {
        return Err(message(
            "Not in a git repository. Run this command from within a git repository.",
        ));
    }
    a.error(command, "not in a git repository.");
    Ok(false)
}
pub(crate) fn sync(a: &mut Action<'_>, flags: &[(String, String)]) -> Result<()> {
    if !git_required(a, "sync")? {
        return Ok(());
    }
    let validate = !has(flags, "validate") || boolean(flags, "validate");
    if validate {
        let cfg = match a.config() {
            Ok(c) => c,
            Err(error) => {
                failure(a, &format!("Validation error: {error}"));
                return Ok(());
            }
        };
        a.registry = match crate::registry::Registry::from_config(Some(&cfg)) {
            Ok(r) => r,
            Err(error) => {
                failure(a, &format!("Validation error: {error}"));
                return Ok(());
            }
        };
        let report = match validate_corpus(a, &cfg, true) {
            Ok(r) => r,
            Err(error) => {
                failure(a, &format!("Validation error: {error}"));
                return Ok(());
            }
        };
        if !report.errors.is_empty() {
            if a.ctx.globals.json {
                let errors = report
                    .errors
                    .iter()
                    .map(|r| {
                        let mut o = Object::new();
                        o.insert("domain", r.domain.clone());
                        o.insert("line", r.line as i64);
                        o.insert("message", r.message.clone());
                        Value::Object(o)
                    })
                    .collect();
                let envelope = object([
                    ("success", false.into()),
                    ("command", "sync".into()),
                    ("validated", false.into()),
                    ("committed", false.into()),
                    ("errors", Value::Array(errors)),
                ]);
                a.output
                    .stdout
                    .push_str(&crate::CommandOutput::emit(&envelope).stdout);
            } else {
                a.output
                    .stderr
                    .push_str(&format!("{}\n", a.styled("red", "Validation failed:")));
                for r in report.errors {
                    a.output.stderr.push_str(&format!(
                        "{}\n",
                        a.styled("red", &format!("  {}:{} - {}", r.domain, r.line, r.message))
                    ));
                }
                a.output.stderr.push_str(&format!(
                    "{}\n",
                    a.styled(
                        "red",
                        "\nFix errors and retry, or use --no-validate to skip."
                    )
                ));
            }
            a.output.exit_code = 1;
            return Ok(());
        }
        if !a.ctx.globals.json {
            a.success(&format!("Validated {} records", report.total_records));
        }
    }
    if git::is_inside_worktree(&a.ctx.cwd, &a.ctx.env) {
        let msg="Skipped commit: running inside a git worktree. Changes are written to the main repo's .luuna/atlas/.";
        if a.ctx.globals.json {
            a.json(
                "sync",
                object([
                    ("validated", validate.into()),
                    ("committed", false.into()),
                    ("message", msg.into()),
                ]),
            );
        } else {
            a.human(format!("{} {msg}", a.styled("brand", "ℹ")));
        }
        return Ok(());
    }
    let status = capture(a.ctx, &["status", "--porcelain", ".luuna/atlas/"]).stdout;
    if status.trim().is_empty() {
        if a.ctx.globals.json {
            a.json(
                "sync",
                object([
                    ("validated", validate.into()),
                    ("committed", false.into()),
                    ("message", "No changes to commit".into()),
                ]),
            );
        } else {
            a.human("No .luuna/atlas/ changes to commit.");
        }
        return Ok(());
    }
    let msg = if has(flags, "message") {
        get(flags, "message")
    } else {
        "luuna-atlas: update expertise".into()
    };
    for args in [vec!["add", ".luuna/atlas/"], vec!["commit", "-m", &msg]] {
        let out = capture(a.ctx, &args);
        if let Some(error) = out.error {
            let error = out
                .exit_stderr
                .map(|stderr| format!("Command failed: git {}\n{stderr}", args.join(" ")))
                .unwrap_or(error);
            failure(a, &format!("Git error: {error}"));
            return Ok(());
        }
    }
    if a.ctx.globals.json {
        a.json(
            "sync",
            object([
                ("validated", validate.into()),
                ("committed", true.into()),
                ("message", msg.clone().into()),
            ]),
        );
    } else {
        a.success(&format!("Committed .luuna/atlas/ changes: \"{msg}\""));
    }
    Ok(())
}
pub(crate) fn learn(a: &mut Action<'_>, flags: &[(String, String)]) -> Result<()> {
    if !git_required(a, "learn")? {
        return Ok(());
    }
    if !a.ctx.root.join(".luuna/atlas").exists() {
        if a.ctx.globals.json {
            a.json(
                "learn",
                object([
                    ("changedFiles", Value::Array(Vec::new())),
                    ("suggestedDomains", Value::Array(Vec::new())),
                    ("unmatchedFiles", Value::Array(Vec::new())),
                    (
                        "message",
                        "No .luuna/atlas/ directory found — skipping".into(),
                    ),
                ]),
            );
        }
        return Ok(());
    }
    let files = git::get_changed_files(&a.ctx.cwd, &a.ctx.env, &get(flags, "since"));
    if files.is_empty() {
        if a.ctx.globals.json {
            a.json(
                "learn",
                object([
                    ("changedFiles", Value::Array(Vec::new())),
                    ("suggestedDomains", Value::Array(Vec::new())),
                    ("unmatchedFiles", Value::Array(Vec::new())),
                    ("message", "No changed files found".into()),
                ]),
            );
        } else {
            a.output
                .stdout
                .push_str("No changed files found. Nothing to learn from.\n");
        }
        return Ok(());
    }
    let cfg = a.config()?;
    let mut matches = Vec::new();
    let mut matched = std::collections::BTreeSet::new();
    for d in domain_names(&cfg) {
        let rows = crate::store::read_records(
            &a.path(&d)?,
            &a.registry,
            a.ctx.globals.allow_unknown_types,
        )?;
        let anchors = rows
            .iter()
            .flat_map(|r| {
                a.registry
                    .get(&text(r.get("type")))
                    .filter(|def| def.extracts_files)
                    .map(|def| records::list(r.get(&def.files_field)))
                    .unwrap_or_default()
            })
            .collect::<Vec<_>>();
        let hits = files
            .iter()
            .filter(|f| {
                anchors.iter().any(|anchor| {
                    *f == anchor || f.ends_with(anchor) || anchor.ends_with(f.as_str())
                })
            })
            .cloned()
            .collect::<Vec<_>>();
        if !hits.is_empty() {
            matched.extend(hits.clone());
            matches.push(object([
                ("domain", d.into()),
                ("matchCount", (hits.len() as i64).into()),
                ("files", crate::actions::record::strings(hits)),
            ]));
        }
    }
    matches.sort_by_key(|r| std::cmp::Reverse(records::number(r.get("matchCount")) as i64));
    let unmatched = files
        .iter()
        .filter(|f| !matched.contains(*f))
        .cloned()
        .collect::<Vec<_>>();
    if a.ctx.globals.json {
        a.json(
            "learn",
            object([
                ("changedFiles", crate::actions::record::strings(files)),
                (
                    "suggestedDomains",
                    Value::Array(matches.into_iter().map(Value::Object).collect()),
                ),
                ("unmatchedFiles", crate::actions::record::strings(unmatched)),
            ]),
        );
        return Ok(());
    }
    a.output.stdout.push_str(&format!(
        "{}\n{}\n",
        a.styled("bold", "\nSession learnings check\n"),
        a.styled("cyan", &format!("Changed files ({}):", files.len()))
    ));
    for f in files {
        a.output.stdout.push_str(&format!("  {f}\n"));
    }
    if !matches.is_empty() {
        a.output
            .stdout
            .push_str(&format!("{}\n", a.styled("cyan", "\nSuggested domains:")));
        for m in matches {
            let n = records::number(m.get("matchCount")) as i64;
            let label = if n == 1 {
                "file matches"
            } else {
                "files match"
            };
            a.output.stdout.push_str(&format!(
                "  {} ({n} {label} existing records)\n",
                a.styled("bold", &text(m.get("domain")))
            ));
        }
    }
    if !unmatched.is_empty() {
        a.output.stdout.push_str(&format!(
            "{}\n",
            a.styled("yellow", "\nUnmatched files (no domain association):")
        ));
        for f in unmatched {
            a.output.stdout.push_str(&format!("  {f}\n"));
        }
    }
    for line in [
        "\nRecord learnings with:",
        "  luuna-atlas record <domain> --type <type> --description \"...\"",
    ] {
        a.output
            .stdout
            .push_str(&format!("{}\n", a.styled("dim", line)));
    }
    a.output.stdout.push('\n');
    Ok(())
}
#[derive(Clone)]
pub struct DiffEntry {
    pub domain: String,
    pub added: Vec<Value>,
    pub removed: Vec<Value>,
}
impl DiffEntry {
    pub fn value(&self) -> Value {
        let mut o = Object::new();
        o.insert("domain", self.domain.clone());
        o.insert("added", Value::Array(self.added.clone()));
        o.insert("removed", Value::Array(self.removed.clone()));
        o.into()
    }
}
fn header_domain(line: &str) -> Option<String> {
    let prefix = ".luuna/atlas/expertise/";
    let mut remaining = line;
    while let Some(at) = remaining.find(prefix) {
        let tail = &remaining[at + prefix.len()..];
        let segment = tail.split('/').next().unwrap_or_default();
        if let Some(end) = segment.rfind(".jsonl").filter(|end| *end > 0) {
            return Some(segment[..end].into());
        }
        remaining = tail;
    }
    None
}
pub fn parse_expertise_diff(raw: &str) -> Vec<DiffEntry> {
    let mut entries: BTreeMap<String, DiffEntry> = BTreeMap::new();
    let mut domain = String::new();
    for line in raw.split('\n') {
        if line.starts_with("diff --git") {
            if let Some(name) = header_domain(line) {
                domain = name;
                entries.entry(domain.clone()).or_insert_with(|| DiffEntry {
                    domain: domain.clone(),
                    added: Vec::new(),
                    removed: Vec::new(),
                });
            }
            continue;
        }
        if line.starts_with("+++")
            || line.starts_with("---")
            || line.starts_with("@@")
            || domain.is_empty()
            || line.len() < 2
        {
            continue;
        }
        if !line.starts_with(['+', '-']) {
            continue;
        }
        let Ok(value) = json::parse(&line[1..]) else {
            continue;
        };
        let value = if matches!(value, Value::Object(_)) {
            value
        } else {
            Value::Object(Object::new())
        };
        let entry = entries.get_mut(&domain).unwrap();
        if line.starts_with('+') {
            entry.added.push(value);
        } else {
            entry.removed.push(value);
        }
    }
    entries
        .into_values()
        .filter(|e| !e.added.is_empty() || !e.removed.is_empty())
        .collect()
}
pub(crate) fn diff(a: &mut Action<'_>, flags: &[(String, String)]) -> Result<()> {
    if !git_required(a, "diff")? {
        return Ok(());
    }
    let since = if has(flags, "since") {
        get(flags, "since")
    } else {
        "HEAD~1".into()
    };
    let raw = capture(
        a.ctx,
        &[
            "diff",
            "--end-of-options",
            &since,
            "--",
            ".luuna/atlas/expertise/",
        ],
    )
    .stdout;
    let entries = parse_expertise_diff(&raw);
    if entries.is_empty() {
        if a.ctx.globals.json {
            a.json(
                "diff",
                object([
                    ("since", since.into()),
                    ("domains", Value::Array(Vec::new())),
                    ("message", "No expertise changes found.".into()),
                ]),
            );
        } else {
            a.output.stdout.push_str("No expertise changes found.\n");
        }
        return Ok(());
    }
    if a.ctx.globals.json {
        a.json(
            "diff",
            object([
                ("since", since.into()),
                (
                    "domains",
                    Value::Array(entries.iter().map(DiffEntry::value).collect()),
                ),
            ]),
        );
        return Ok(());
    }
    a.output.stdout.push_str(&format!(
        "{}\n",
        a.styled("bold", &format!("\nExpertise changes since {since}\n"))
    ));
    for d in entries {
        let n = d.added.len() + d.removed.len();
        a.human(a.styled(
            "cyan",
            &format!(
                "{} ({n} change{}):",
                d.domain,
                if n == 1 { "" } else { "s" }
            ),
        ));
        for (rows, sign, color) in [(d.added, "+", "green"), (d.removed, "-", "red")] {
            for value in rows {
                let r = records::obj(Some(&value));
                a.human(a.styled(
                    color,
                    &format!(
                        "  {sign} [{}] {} {}",
                        text(r.get("type")),
                        a.styled("accent", &text(r.get("id"))),
                        records::record_summary(r, &a.registry)
                    ),
                ));
            }
        }
        a.human("");
    }
    Ok(())
}
