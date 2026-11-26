//! Human-readable rendering of runtime records. Every state is shown as the runtime's unchanged
//! text (the same words the desktop badges show); the runtime's reasons and the shared next step
//! follow the state. Nothing here decides what is allowed: it prints the runtime's decisions.
//! Property reads follow the original: where it read a field of a missing value (a TypeError), the
//! port fails the same way, so the command reports the same refusal.
use crate::failures::{baseline_step, next_step, AllowedActions, Failure};
use crate::js::{self, J};

type R<T> = Result<T, Failure>;

static EMPTY_ARR: J = J::Arr(Vec::new());
static EMPTY_OBJ: J = J::Obj(Vec::new());

fn type_error() -> Failure {
    Failure::type_error()
}

/// `value.key` (null and undefined throw).
fn f<'a>(value: &'a J, key: &str) -> R<&'a J> {
    if value.nullish() {
        Err(type_error())
    } else {
        Ok(value.get(key))
    }
}

/// `value?.key`
fn o<'a>(value: &'a J, key: &str) -> &'a J {
    value.get(key)
}

/// `${value}`
fn s(value: &J) -> String {
    value.js()
}

/// `${value ?? fallback}`
fn or(value: &J, fallback: &str) -> String {
    if value.nullish() {
        fallback.to_string()
    } else {
        value.js()
    }
}

/// `value.length` (null and undefined throw; values without a length give undefined).
fn len(value: &J) -> R<J> {
    match value {
        J::Undefined | J::Null => Err(type_error()),
        J::Arr(items) => Ok(J::from(items.len())),
        J::Str(text) => Ok(J::from(js::len16(text))),
        _ => Ok(J::Undefined),
    }
}

/// `value?.length`
fn olen(value: &J) -> J {
    len(value).unwrap_or(J::Undefined)
}

/// `value?.length ?? 0` as text.
fn count(value: &J) -> String {
    or(&olen(value), "0")
}

/// `[...].join(separator)` of JavaScript values: null and undefined join as empty.
fn join_values(items: &[J], separator: &str) -> String {
    items
        .iter()
        .map(J::js_or_empty)
        .collect::<Vec<_>>()
        .join(separator)
}

/// `value.join(separator)`: only arrays have it.
fn join(value: &J, separator: &str) -> R<String> {
    match value {
        J::Arr(items) => Ok(join_values(items, separator)),
        _ => Err(type_error()),
    }
}

/// `value?.join(separator)`
fn ojoin(value: &J, separator: &str) -> R<J> {
    if value.nullish() {
        Ok(J::Undefined)
    } else {
        join(value, separator).map(J::Str)
    }
}

/// `value.map(fn)`: only arrays have it.
fn map<T>(value: &J, mut each: impl FnMut(&J) -> R<T>) -> R<Vec<T>> {
    match value {
        J::Arr(items) => items.iter().map(&mut each).collect(),
        _ => Err(type_error()),
    }
}

/// `for (const item of value)` / `...value`: arrays and strings iterate.
fn iterate(value: &J) -> R<Vec<J>> {
    match value {
        J::Arr(items) => Ok(items.clone()),
        J::Str(text) => Ok(text.chars().map(|c| J::str(c.to_string())).collect()),
        _ => Err(type_error()),
    }
}

/// `value.length ? value.map(fn).join("\n") : empty`
fn listed(value: &J, empty: &str, each: impl FnMut(&J) -> R<String>) -> R<String> {
    if len(value)?.truthy() {
        Ok(map(value, each)?.join("\n"))
    } else {
        Ok(empty.to_string())
    }
}

/// `Object.entries(value)` for the shapes met here.
fn entries(value: &J) -> R<Vec<(String, J)>> {
    match value {
        J::Undefined | J::Null => Err(type_error()),
        J::Obj(entries) => Ok(entries.clone()),
        J::Arr(items) => Ok(items
            .iter()
            .enumerate()
            .map(|(i, v)| (i.to_string(), v.clone()))
            .collect()),
        J::Str(text) => Ok(text
            .chars()
            .enumerate()
            .map(|(i, c)| (i.to_string(), J::str(c.to_string())))
            .collect()),
        _ => Ok(Vec::new()),
    }
}

/// `value > 0` in JavaScript.
fn positive(value: &J) -> bool {
    value.to_number() > 0.0
}

/// SGR parameters per tone: green is earned, cyan is the runtime working, rose is failed or blocked
/// evidence, amber waits on the developer, dim is recorded without being evidence, plum interrupts.
fn sgr(tone: &str) -> Option<&'static str> {
    Some(match tone {
        "success" => "32",
        "active" => "36",
        "danger" => "31",
        "attention" => "33",
        "neutral" => "2",
        "interrupted" => "35",
        _ => return None,
    })
}

pub fn run_tone(state: &str) -> Option<&'static str> {
    Some(match state {
        "queued" | "completed" | "cancelled" | "discovered" => "neutral",
        "running" | "exploring" | "implementing" | "validating" | "reviewing" => "active",
        "failed" | "testing_blocked" => "danger",
        "interrupted" => "interrupted",
        "plan_ready" | "approved" | "paused" => "attention",
        "reviewed" => "success",
        _ => return None,
    })
}
pub fn candidate_tone(state: &str) -> Option<&'static str> {
    Some(match state {
        "pending" | "cancelled" => "neutral",
        "validating" | "reviewing" => "active",
        "reviewed" => "success",
        "failed" | "testing_blocked" => "danger",
        "paused" => "attention",
        "interrupted" => "interrupted",
        _ => return None,
    })
}
pub fn check_tone(state: &str) -> Option<&'static str> {
    Some(match state {
        "passed" => "success",
        "failed" | "blocked" => "danger",
        "cancelled" => "neutral",
        "interrupted" => "interrupted",
        _ => return None,
    })
}
pub fn baseline_tone(state: &str) -> Option<&'static str> {
    if state == "running" {
        Some("active")
    } else {
        check_tone(state)
    }
}
pub fn verdict_tone(state: &str) -> Option<&'static str> {
    Some(match state {
        "passed" => "success",
        "changes_required" | "blocked" => "danger",
        _ => return None,
    })
}
pub fn severity_tone(state: &str) -> Option<&'static str> {
    Some(match state {
        "info" => "neutral",
        "warning" => "attention",
        "blocking" => "danger",
        _ => return None,
    })
}
pub fn lease_tone(state: &str) -> Option<&'static str> {
    Some(match state {
        "active" => "active",
        "released" => "neutral",
        "abandoned" => "interrupted",
        _ => return None,
    })
}

/// Whether colour may be used: only on a terminal, never with NO_COLOR set or TERM=dumb.
pub fn colour_enabled(no_color: Option<&str>, term: Option<&str>, is_terminal: bool) -> bool {
    is_terminal && no_color.is_none_or(str::is_empty) && term != Some("dumb")
}

/// The text in its tone's colour when `colour` is on; the unchanged text otherwise.
pub fn paint(text: &str, tone: Option<&str>, colour: bool) -> String {
    match (colour, tone.and_then(sgr)) {
        (true, Some(code)) => format!("\u{1b}[{code}m{text}\u{1b}[0m"),
        _ => text.to_string(),
    }
}

/// `p(value, table[value])`
fn p(value: &J, table: fn(&str) -> Option<&'static str>, colour: bool) -> String {
    paint(&value.js(), table(&value.js()), colour)
}

/// A generated identity as its short id (ADR 0042): what lists show and what every argument accepts.
fn sid(value: &J) -> String {
    crate::refs::short_id(&s(value))
}

/// A content hash as `sha256:` and its first twelve digits, which acknowledge it as an argument.
fn shash(value: &J) -> String {
    let full = s(value);
    let short = crate::refs::short_hash(&full);
    if short == full {
        full
    } else {
        format!("{short}…")
    }
}

/// `short(hash)`
fn short(hash: &J) -> String {
    match hash {
        J::Str(text) if js::len16(text) > 20 => format!("{}…", js::slice16(text, 0, Some(19))),
        other => or(other, ""),
    }
}

/// `when(value)`: the ISO timestamp with a space and without fractional seconds.
fn when(value: &J) -> String {
    let J::Str(text) = value else {
        return String::new();
    };
    let replaced = text.replacen('T', " ", 1);
    // .replace(/\.\d+Z$/, "Z")
    if let Some(stripped) = replaced.strip_suffix('Z') {
        if let Some(dot) = stripped.rfind('.') {
            let digits = &stripped[dot + 1..];
            if !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()) {
                return format!("{}Z", &stripped[..dot]);
            }
        }
    }
    replaced
}

/// `dollars(micros)`
fn dollars(micros: &J) -> String {
    let number = if micros.nullish() {
        0.0
    } else {
        micros.to_number()
    };
    format!("${}", js::to_fixed(number / 1_000_000.0, 4))
}

pub fn run_line(run: &J, colour: bool) -> R<String> {
    let metadata = f(run, "metadata")?;
    let kind = o(metadata, "kind");
    let what = if kind.as_str() == Some("baseline") {
        format!(
            "baseline · {} · manifest r{}",
            or(f(metadata, "repository_id")?, ""),
            or(f(metadata, "manifest_revision")?, "?")
        )
    } else if kind.as_str() == Some("workflow") {
        format!(
            "workflow · plan {} r{}",
            crate::refs::short_id(&or(f(metadata, "plan_id")?, "")),
            or(f(metadata, "plan_revision")?, "?")
        )
    } else if let Some(prompt) = o(metadata, "prompt").as_str() {
        format!("session · {}", js::slice16(prompt, 0, Some(60)))
    } else {
        "session".to_string()
    };
    Ok(format!(
        "{}  {}  {what}  {}",
        sid(f(run, "id")?),
        p(f(run, "state")?, run_tone, colour),
        when(f(run, "created_at")?)
    ))
}

pub fn event_line(event: &J) -> R<String> {
    let payload = f(event, "payload")?;
    let reason = match o(payload, "reason") {
        J::Str(reason) => format!("  reason: {reason}"),
        _ => String::new(),
    };
    let state = match o(payload, "state") {
        J::Str(state) => format!("  {state}"),
        _ => String::new(),
    };
    Ok(format!(
        "{}  {}  {}{state}{reason}",
        js::pad_start(&s(f(event, "sequence")?), 5),
        when(f(event, "timestamp")?),
        s(f(event, "type")?)
    ))
}

fn check_line(result: &J, colour: bool) -> R<String> {
    let exit = f(result, "exit_code")?;
    let tests = f(result, "tests_detected")?;
    let reason = f(result, "reason")?;
    let refs = f(result, "artifact_refs")?;
    let logs = if olen(refs).truthy() {
        format!(" · logs: {}", join(refs, ", ")?)
    } else {
        String::new()
    };
    Ok(format!(
        "  {}: {}{}{}{}{logs}",
        s(f(result, "recipe_id")?),
        p(f(result, "status")?, check_tone, colour),
        if exit.nullish() {
            String::new()
        } else {
            format!(" (exit {})", s(exit))
        },
        if tests.is_undefined() {
            String::new()
        } else {
            format!(" · {} test(s) detected", s(tests))
        },
        if reason.truthy() {
            format!(" · {}", s(reason))
        } else {
            String::new()
        },
    ))
}

fn allowed(value: &J) -> Option<bool> {
    match value {
        J::Bool(b) => Some(*b),
        J::Undefined | J::Null => None,
        // nextStep only treats exactly `false` as a refusal.
        _ => Some(true),
    }
}

/// What the runtime admits for this candidate now, from its own decisions.
fn decisions(view: &J) -> R<Vec<String>> {
    let mut out = Vec::new();
    if f(view, "active")?.truthy() {
        out.push("The runtime is running this workflow's checks, reviewer or repair; cancellation is available.".to_string());
    }
    for (name, field) in [
        ("validate", "validate_allowed"),
        ("revalidate", "revalidate_allowed"),
        ("repair", "repair_allowed"),
    ] {
        let value = f(view, field)?;
        if value == &J::Bool(true) {
            out.push(format!("{name} is available"));
        } else if value == &J::Bool(false) {
            let reason = o(f(view, "decision_reasons")?, name);
            if reason.truthy() {
                out.push(format!("{name} is not available: {}", s(reason)));
            }
        }
    }
    Ok(out)
}

fn override_line(entry: &J) -> R<String> {
    Ok(format!(
        "{} {} <- {} ({})",
        s(f(entry, "action")?),
        s(f(entry, "consumer")?),
        s(f(entry, "producer")?),
        s(f(entry, "justification")?)
    ))
}

/// `reason.toLowerCase()` before the next step: only strings have it.
fn reason_text(reason: &J) -> R<String> {
    match reason {
        J::Str(text) => Ok(text.clone()),
        _ => Err(type_error()),
    }
}

pub fn format_candidate(view: &J, colour: bool) -> R<String> {
    let evidence = f(view, "evidence")?;
    if !evidence.truthy() {
        let mut lines = vec!["No candidate is recorded for this run.".to_string()];
        if f(view, "active")?.truthy() {
            lines.push("The runtime is running the scoped writer.".into());
        }
        return Ok(lines.join("\n"));
    }
    let reasons = f(view, "decision_reasons")?;
    let reasons = if reasons.nullish() {
        J::obj()
    } else {
        reasons.clone()
    };
    let reason_of = |name: &str| {
        let value = o(&reasons, name);
        value.truthy().then(|| value.js())
    };
    let actions = AllowedActions {
        validate: allowed(f(view, "validate_allowed")?),
        revalidate: allowed(f(view, "revalidate_allowed")?),
        repair: allowed(f(view, "repair_allowed")?),
        reasons: [
            reason_of("validate"),
            reason_of("revalidate"),
            reason_of("repair"),
        ],
    };
    let candidate = f(evidence, "candidate")?;
    let mut lines = vec![
        format!(
            "Candidate r{}  {}",
            s(f(candidate, "revision")?),
            p(f(evidence, "status")?, candidate_tone, colour)
        ),
        format!("candidate hash: {}", s(f(candidate, "hash")?)),
        format!("approved plan hash: {}", s(f(candidate, "plan_hash")?)),
    ];
    for (id, commit) in entries(f(candidate, "repository_commits")?)? {
        lines.push(format!("{id} commit: {}", s(&commit)));
    }
    let results = f(evidence, "results")?;
    lines.push(format!("checks ({} result(s)):", s(&len(results)?)));
    lines.extend(map(results, |r| check_line(r, colour))?);
    let review = f(evidence, "review")?;
    if review.truthy() {
        let findings = f(review, "findings")?;
        lines.push(format!(
            "independent review: {} ({} finding(s); session {})",
            p(f(review, "verdict")?, verdict_tone, colour),
            s(&len(findings)?),
            s(f(review, "reviewer_session")?)
        ));
        lines.extend(map(findings, |finding| {
            Ok(format!(
                "  {} {}:{} {}",
                p(f(finding, "severity")?, severity_tone, colour),
                s(f(finding, "path")?),
                s(f(finding, "line")?),
                s(f(finding, "explanation")?)
            ))
        })?);
    } else {
        lines.push("independent review: none recorded".into());
    }
    let reason = f(evidence, "reason")?;
    if reason.truthy() {
        lines.push(format!("reason: {}", s(reason)));
        let step = next_step(&reason_text(reason)?, Some(&actions));
        if !step.is_empty() {
            lines.push(format!("next: {step}"));
        }
    }
    lines.extend(decisions(view)?);
    let usage = f(view, "usage")?;
    if usage.truthy() {
        lines.push(format!(
            "usage: {} of {} writer attempt(s) · {}s of {}s · {} of {} · {} repair(s){}",
            s(f(usage, "attempts")?),
            s(f(usage, "max_attempts")?),
            s(f(usage, "elapsed_seconds")?),
            s(f(usage, "timeout_seconds")?),
            dollars(f(usage, "cost_usd_micros")?),
            dollars(f(usage, "cost_budget_usd_micros")?),
            s(f(usage, "repairs")?),
            if f(usage, "cost_unreported")?.truthy() {
                " · some model spend was reported without a cost"
            } else {
                ""
            }
        ));
    }
    let overrides = f(view, "dependency_overrides")?;
    if olen(overrides).truthy() {
        lines.push(format!(
            "dependency overrides: {}",
            map(overrides, override_line)?.join("; ")
        ));
    }
    let tasks = f(view, "tasks")?;
    if olen(tasks).truthy() {
        lines.push("tasks:".into());
        for task in map(tasks, task_lines)? {
            lines.extend(task);
        }
    }
    let prerequisite = f(view, "prerequisite")?;
    if prerequisite.truthy() {
        lines.extend(handoff_lines(prerequisite)?);
    }
    Ok(lines.join("\n"))
}

/// The testing-setup handoff of a setup workflow's candidate view, with `next` in the runtime's words.
pub fn handoff_lines(handoff: &J) -> R<Vec<String>> {
    let commit = f(handoff, "candidate_commit")?;
    let mut lines = vec![
        format!(
            "testing setup for {}: {}{}",
            s(f(handoff, "repository_id")?),
            if f(handoff, "reviewed")?.truthy() {
                "reviewed with a passed verdict"
            } else {
                "not yet reviewed with a passed verdict"
            },
            if commit.truthy() {
                format!(" · candidate commit {}", s(commit))
            } else {
                String::new()
            }
        ),
        "proposed required checks (confirm these in the next manifest revision after merging):"
            .to_string(),
    ];
    let checks = f(handoff, "checks")?;
    let checks = if checks.nullish() {
        J::Arr(Vec::new())
    } else {
        checks.clone()
    };
    lines.extend(map(&checks, |check| {
        let args = f(check, "args")?;
        let mut command = vec![f(check, "executable")?.clone()];
        command.extend(iterate(if args.nullish() { &EMPTY_ARR } else { args })?);
        let cwd = f(check, "cwd")?;
        let capabilities = f(check, "capabilities")?;
        Ok(format!(
            "  {}: {}{} · {}s{}",
            s(f(check, "id")?),
            join_values(&command, " "),
            if cwd.truthy() && cwd.as_str() != Some(".") {
                format!(" (in {})", s(cwd))
            } else {
                String::new()
            },
            s(f(check, "timeout_seconds")?),
            if olen(capabilities).truthy() {
                format!(" · {}", join(capabilities, ", ")?)
            } else {
                String::new()
            }
        ))
    })?);
    lines.push(format!("next: {}", s(f(handoff, "next")?)));
    Ok(lines)
}

/// The runtime's testing-setup offer (ADR 0022): its reason and its next step, unchanged.
pub fn offer_lines(prerequisite: &J, indent: &str) -> R<Vec<String>> {
    Ok(vec![
        format!(
            "{indent}testing setup needed for {}: {}",
            s(f(prerequisite, "repository_id")?),
            s(f(prerequisite, "reason")?)
        ),
        format!("{indent}next: {}", s(f(prerequisite, "next")?)),
    ])
}

/// One task record with its assignment trace and, for an advisory task, its structured result.
pub fn task_lines(task: &J) -> R<Vec<String>> {
    let role = f(task, "role")?;
    let depends = f(task, "depends_on")?;
    let commit = f(task, "candidate_commit")?;
    let attempt = f(task, "attempt_id")?;
    let reason = f(task, "reason")?;
    let mut lines = vec![format!(
        "  {}  {}  {} · role {}{}{}{}{}",
        s(f(task, "task_id")?),
        s(f(task, "state")?),
        s(f(task, "repository_id")?),
        or(role, "implementer"),
        if olen(depends).truthy() {
            format!(" · after {}", join(depends, ", ")?)
        } else {
            String::new()
        },
        if commit.truthy() {
            format!(" · commit {}", s(commit))
        } else {
            String::new()
        },
        if attempt.truthy() {
            format!(" · attempt {}", s(attempt))
        } else {
            String::new()
        },
        if reason.truthy() {
            format!(" · {}", s(reason))
        } else {
            String::new()
        },
    )];
    let assigned = f(task, "assigned_by")?;
    if assigned.truthy() {
        lines.push(format!(
            "    assigned by {} · approval {} at {} · plan {} r{} · {}",
            s(f(assigned, "approved_by")?),
            s(f(assigned, "approval_id")?),
            when(f(assigned, "approved_at")?),
            s(f(assigned, "plan_id")?),
            s(f(assigned, "plan_revision")?),
            s(f(assigned, "plan_hash")?)
        ));
    }
    let derived = f(task, "derived_from_task")?;
    if derived.truthy() {
        lines.push(format!("    derived from task {}", s(derived)));
    }
    let overrides = f(task, "dependency_overrides")?;
    if olen(overrides).truthy() {
        lines.push(format!(
            "    dependency overrides: {}",
            map(overrides, override_line)?.join("; ")
        ));
    }
    let result = f(task, "advisory_result")?;
    if result.truthy() {
        let (findings, proposed, risks) = (
            f(result, "findings")?,
            f(result, "proposed_tasks")?,
            f(result, "risks")?,
        );
        lines.push(format!(
            "    advisory result: {} finding(s), {} proposed task(s), {} risk(s)",
            count(findings),
            count(proposed),
            count(risks)
        ));
        let empty = J::Arr(Vec::new());
        for finding in iterate(if findings.nullish() { &empty } else { findings })? {
            lines.push(format!("      finding: {}", s(&finding)));
        }
        for item in iterate(if proposed.nullish() { &empty } else { proposed })? {
            let depends = f(&item, "depends_on")?;
            let paths = f(&item, "paths")?;
            let rationale = f(&item, "rationale")?;
            lines.push(format!(
                "      proposed task {} ({}, {}){}{}{}",
                s(f(&item, "id")?),
                s(f(&item, "repository_id")?),
                or(f(&item, "role")?, "implementer"),
                if olen(depends).truthy() {
                    format!(" after {}", join(depends, ", ")?)
                } else {
                    String::new()
                },
                if olen(paths).truthy() {
                    format!(" · paths {}", join(paths, ", ")?)
                } else {
                    String::new()
                },
                if rationale.truthy() {
                    format!(" · {}", s(rationale))
                } else {
                    String::new()
                }
            ));
        }
        for risk in iterate(if risks.nullish() { &empty } else { risks })? {
            lines.push(format!("      risk: {}", s(&risk)));
        }
        lines.push(format!("      a proposed task becomes real only through a plan revision you create (plan --task …;derived_from={}) and approve", s(f(task, "task_id")?)));
    }
    Ok(lines)
}

fn format_baseline(baseline: &J, colour: bool) -> R<String> {
    let reason = f(baseline, "reason")?;
    let status = f(baseline, "status")?;
    let step = if reason.truthy() {
        let state = status.js();
        if state == "passed" || state == "running" || state == "interrupted" || state == "cancelled"
        {
            baseline_step(&state, "")
        } else {
            baseline_step(&state, &reason_text(reason)?)
        }
    } else {
        String::new()
    };
    let prerequisite = f(baseline, "prerequisite")?;
    let mut lines = vec![format!(
        "{}  {}  manifest r{} · commit {} · {}/{} required results",
        s(f(baseline, "run_id")?),
        p(status, baseline_tone, colour),
        s(f(baseline, "manifest_revision")?),
        short(o(f(baseline, "identity")?, "commit")),
        count(f(baseline, "results")?),
        count(f(baseline, "required_checks")?)
    )];
    if reason.truthy() {
        lines.push(format!(
            "  reason: {}{}",
            s(reason),
            if !step.is_empty() && !prerequisite.truthy() {
                format!("  next: {step}")
            } else {
                String::new()
            }
        ));
    }
    if prerequisite.truthy() {
        lines.extend(offer_lines(prerequisite, "  ")?);
    }
    let results = f(baseline, "results")?;
    if !results.nullish() {
        lines.extend(map(results, |r| check_line(r, colour))?);
    }
    Ok(lines.join("\n"))
}

/// `Object.values(value ?? {})[0] ?? ""`
fn first_value(value: &J) -> String {
    let first = match value {
        J::Undefined | J::Null => J::Undefined,
        J::Obj(entries) => entries
            .first()
            .map(|(_, v)| v.clone())
            .unwrap_or(J::Undefined),
        J::Arr(items) => items.first().cloned().unwrap_or(J::Undefined),
        J::Str(text) => text
            .chars()
            .next()
            .map(|c| J::str(c.to_string()))
            .unwrap_or(J::Undefined),
        _ => J::Undefined,
    };
    or(&first, "")
}

fn format_project(view: &J) -> R<String> {
    let repositories = f(view, "repositories")?;
    let registration = if repositories.nullish() {
        J::Undefined
    } else {
        first_item(repositories)
    };
    let observation = o(&registration, "observation");
    let project = f(view, "project")?;
    let mut lines = vec![format!(
        "{} ({})  {}",
        s(f(project, "name")?),
        s(f(project, "id")?),
        if positive(f(project, "revision")?) {
            format!("manifest r{}", s(f(project, "revision")?))
        } else {
            "unconfirmed".into()
        }
    )];
    if registration.truthy() {
        lines.push(format!(
            "repository {} · {}",
            s(f(&registration, "repository_id")?),
            s(f(&registration, "local_path")?)
        ));
    } else {
        lines.push("no registered repository".into());
    }
    if observation.truthy() {
        let dirty = f(observation, "dirty")?;
        lines.push(format!(
            "remote: {} · HEAD {} on {} · dirty {} staged, {} unstaged, {} untracked",
            or(
                o(f(observation, "identity")?, "remote"),
                "no portable origin"
            ),
            short(f(observation, "head_commit")?),
            or(f(observation, "branch")?, "detached HEAD"),
            or(o(dirty, "staged"), "0"),
            or(o(dirty, "unstaged"), "0"),
            or(o(dirty, "untracked"), "0")
        ));
    }
    let manifest = f(view, "manifest")?;
    lines.push(if manifest.truthy() {
        format!(
            "confirmed manifest: revision {} · {} · base {}",
            s(f(manifest, "revision")?),
            s(f(manifest, "hash")?),
            first_value(f(manifest, "base_commits")?)
        )
    } else {
        "confirmed manifest: none".into()
    });
    let discovery = f(view, "discovery")?;
    lines.push(if discovery.truthy() {
        format!(
            "discovery: proposal for revision {} ({} problem(s), {} question(s))",
            s(o(f(discovery, "proposed_manifest")?, "revision")),
            count(f(discovery, "manifest_problems")?),
            count(f(discovery, "questions")?)
        )
    } else {
        "discovery: none stored".into()
    });
    let prerequisites = f(view, "prerequisites")?;
    if olen(prerequisites).truthy() {
        lines.push("prerequisites (a feature plan is refused until each is established):".into());
        for offer in map(prerequisites, |p| offer_lines(p, "  "))? {
            lines.extend(offer);
        }
    }
    Ok(lines.join("\n"))
}

/// `value?.[0]` for an array or a string.
fn first_item(value: &J) -> J {
    match value {
        J::Arr(items) => items.first().cloned().unwrap_or(J::Undefined),
        J::Str(text) => text
            .chars()
            .next()
            .map(|c| J::str(c.to_string()))
            .unwrap_or(J::Undefined),
        J::Obj(_) => value.get("0").clone(),
        _ => J::Undefined,
    }
}

/// The Home summary: projects, validation health per repository, blockers and the create-ticket shortcut.
pub fn format_home(summary: &J, colour: bool) -> R<String> {
    let projects = f(summary, "projects")?;
    if !len(projects)?.truthy() {
        return Ok("No projects registered. Register a repository (register <name> <repository-id> <path>) to start.".into());
    }
    let mut lines = Vec::new();
    for project in iterate(projects)? {
        lines.push(format!(
            "{} ({})  {} · {} ticket(s) · {} active workflow(s)",
            s(f(&project, "name")?),
            s(f(&project, "id")?),
            if positive(f(&project, "revision")?) {
                format!("manifest r{}", s(f(&project, "revision")?))
            } else {
                "unconfirmed".into()
            },
            s(f(&project, "tickets")?),
            s(f(&project, "active_workflows")?)
        ));
        for repository in iterate(f(&project, "repositories")?)? {
            let baseline = f(&repository, "baseline")?;
            let health = if baseline.truthy() {
                let reason = f(baseline, "reason")?;
                format!(
                    "baseline {} · manifest r{} · commit {}{}",
                    p(f(baseline, "status")?, baseline_tone, colour),
                    s(f(baseline, "manifest_revision")?),
                    short(f(baseline, "commit")?),
                    if reason.truthy() {
                        format!(" · {}", s(reason))
                    } else {
                        String::new()
                    }
                )
            } else {
                "no baseline".into()
            };
            lines.push(format!(
                "  {}: {health}{}",
                s(f(&repository, "repository_id")?),
                if f(&repository, "prerequisite")?.truthy() {
                    " · testing setup needed"
                } else {
                    ""
                }
            ));
        }
    }
    let blockers = f(summary, "blockers")?;
    lines.push(if len(blockers)?.truthy() {
        format!("blockers ({}):", s(&len(blockers)?))
    } else {
        "blockers: none".into()
    });
    for blocker in iterate(blockers)? {
        let run = f(&blocker, "run_id")?;
        lines.push(format!(
            "  {} · {}: {}{}",
            s(f(&blocker, "project_id")?),
            s(f(&blocker, "title")?),
            s(f(&blocker, "detail")?),
            if run.truthy() {
                format!(" (run {})", sid(run))
            } else {
                String::new()
            }
        ));
    }
    lines.push("create a ticket: create-ticket <project> --title … --intent … --criterion … [--purpose testing_setup]".into());
    Ok(lines.join("\n"))
}

/// The Kanban projection: every column with its read-only cards, then the note that nothing is dragged.
pub fn format_board(board: &J, colour: bool) -> R<String> {
    let mut lines = Vec::new();
    for entry in iterate(f(board, "columns")?)? {
        let column = f(&entry, "column")?;
        let cards = f(&entry, "cards")?;
        lines.push(format!(
            "{} ({})  {}",
            s(f(column, "title")?),
            s(&len(cards)?),
            s(f(column, "description")?)
        ));
        for card in iterate(cards)? {
            if f(&card, "kind")?.as_str() == Some("prerequisite") {
                lines.push(format!(
                    "  [prerequisite] {} · {} · {}",
                    s(f(&card, "title")?),
                    s(f(&card, "project_id")?),
                    or(f(&card, "reason")?, "")
                ));
            } else {
                let purpose = f(&card, "purpose")?;
                let run = f(&card, "run_id")?;
                lines.push(format!(
                    "  [ticket] {} · {} · {} r{}{}{}",
                    s(f(&card, "title")?),
                    s(f(&card, "project_id")?),
                    sid(f(&card, "ticket_id")?),
                    s(f(&card, "ticket_revision")?),
                    if purpose.truthy() {
                        format!(" · {}", reason_text(purpose)?.replace('_', " "))
                    } else {
                        String::new()
                    },
                    if run.truthy() {
                        format!(
                            " · run {} {} (plan r{})",
                            sid(run),
                            p(f(&card, "state")?, run_tone, colour),
                            s(f(&card, "plan_revision")?)
                        )
                    } else {
                        " · no plan yet".into()
                    }
                ));
            }
        }
    }
    lines.push(s(f(board, "note")?));
    Ok(lines.join("\n"))
}

/// The doctor report as `luuna-runtime doctor` prints it.
pub fn orbit_tone(status: &str) -> Option<&'static str> {
    Some(match status {
        "succeeded" => "success",
        "failed" | "blocked" => "danger",
        "cancelled" | "skipped" => "neutral",
        _ => return None,
    })
}

/// `${count} ${count === 1 ? one : many}`
fn plural(count: &J, one: &str, many: &str) -> String {
    format!(
        "{} {}",
        s(count),
        if count.as_f64() == Some(1.0) {
            one
        } else {
            many
        }
    )
}

/// JavaScript's `a + b`: strings and objects concatenate as text, everything else adds as numbers.
fn add(a: &J, b: &J) -> J {
    let text = |v: &J| matches!(v, J::Str(_) | J::Obj(_) | J::Arr(_));
    if text(a) || text(b) {
        J::Str(format!("{}{}", a.js(), b.js()))
    } else {
        J::num(a.to_number() + b.to_number())
    }
}

/// `a !== b` for values read from JSON: objects and arrays are never the same value.
fn strictly_different(a: &J, b: &J) -> bool {
    match (a, b) {
        (J::Undefined, J::Undefined) | (J::Null, J::Null) => false,
        (J::Bool(x), J::Bool(y)) => x != y,
        (J::Str(x), J::Str(y)) => x != y,
        (J::Num(_), J::Num(_)) => a.as_f64() != b.as_f64(),
        _ => true,
    }
}

/// `value.map(fn)` spread into a list (`...(value ?? []).map(fn)`).
fn spread_map(value: &J, each: impl FnMut(&J) -> R<String>) -> R<Vec<String>> {
    if value.nullish() {
        Ok(Vec::new())
    } else {
        map(value, each)
    }
}

/// One line per repository: branch, upstream distance, uncommitted changes and recipes.
pub fn format_orbit(view: &J, colour: bool) -> R<String> {
    let repositories = f(view, "repositories")?.or(&EMPTY_ARR).clone();
    let revision = f(view, "manifest_revision")?;
    let mut lines = vec![format!(
        "{} ({}) · {} · {}",
        s(f(view, "name")?),
        s(f(view, "project_id")?),
        plural(&len(&repositories)?, "repository", "repositories"),
        if revision.nullish() {
            "manifest unconfirmed".to_string()
        } else {
            format!("manifest r{}", s(revision))
        }
    )];
    for repository in iterate(&repositories)? {
        if !f(&repository, "present")?.truthy() {
            lines.push(format!(
                "  {}  {}  {}  (orbit-sync clones it)",
                s(f(&repository, "repository_id")?),
                paint("missing", Some("attention"), colour),
                s(f(&repository, "local_path")?)
            ));
            continue;
        }
        let dirty_state = f(&repository, "dirty")?;
        let dirty = if dirty_state.truthy() {
            let sum = add(
                &add(
                    &add(dirty_state.get("staged"), dirty_state.get("unstaged")),
                    dirty_state.get("untracked"),
                ),
                dirty_state.get("conflicted"),
            );
            sum
        } else {
            J::num(0)
        };
        let upstream = f(&repository, "upstream")?;
        let upstream = if upstream.truthy() {
            format!(
                "  {} ↑{} ↓{}",
                s(upstream.get("name")),
                s(upstream.get("ahead")),
                s(upstream.get("behind"))
            )
        } else {
            "  no upstream".to_string()
        };
        let recipes = f(&repository, "recipes")?;
        let recipes = if olen(recipes).truthy() {
            format!("  recipes: {}", join(recipes, ", ")?)
        } else {
            String::new()
        };
        lines.push(format!(
            "  {}  {}{}  {}  {}{}",
            s(f(&repository, "repository_id")?),
            or(f(&repository, "branch")?, "(detached)"),
            upstream,
            if dirty.truthy() {
                paint(&format!("{} changed", s(&dirty)), Some("attention"), colour)
            } else {
                paint("clean", Some("neutral"), colour)
            },
            s(f(&repository, "local_path")?),
            recipes
        ));
        let error = f(&repository, "error")?;
        if error.truthy() {
            lines.push(format!("    {}", paint(&s(error), Some("danger"), colour)));
        }
    }
    Ok(lines.join("\n"))
}

/// `text.trimEnd().split(/\r?\n/)`
fn trimmed_lines(text: &str) -> Vec<String> {
    text.trim_end_matches(|c: char| c.is_whitespace() || c == '\u{feff}')
        .split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line).to_string())
        .collect()
}

/// Per repository: the outcome, then the tail of its output.
pub fn format_orbit_execution(execution: &J, colour: bool) -> R<String> {
    let results = f(execution, "results")?.or(&EMPTY_ARR).clone();
    let command = f(execution, "command")?;
    let command = if command.nullish() {
        String::new()
    } else {
        join(command, " ")?
    };
    let mut lines = vec![format!(
        "{} {} · {} · not evidence",
        or(f(execution, "kind")?, "orbit"),
        command,
        plural(&len(&results)?, "repository", "repositories")
    )];
    for result in iterate(&results)? {
        let status = f(&result, "status")?;
        let action = f(&result, "action")?;
        let exit_code = f(&result, "exit_code")?;
        let reason = f(&result, "reason")?;
        let exit = J::str(format!("exit code {}", s(exit_code)));
        lines.push(format!(
            "── {}  {}{}{}{}",
            s(f(&result, "repository_id")?),
            paint(&s(status), orbit_tone(&s(status)), colour),
            if action.truthy() {
                format!(" ({})", s(action))
            } else {
                String::new()
            },
            if exit_code.nullish() {
                String::new()
            } else {
                format!(" exit {}", s(exit_code))
            },
            if reason.truthy() && strictly_different(reason, &exit) {
                format!(" · {}", s(reason))
            } else {
                String::new()
            }
        ));
        for text in [f(&result, "stdout_excerpt")?, f(&result, "stderr_excerpt")?] {
            if !text.truthy() {
                continue;
            }
            let J::Str(text) = text else {
                return Err(type_error());
            };
            if crate::builders::js_trim(text).is_empty() {
                continue;
            }
            lines.push(
                trimmed_lines(text)
                    .iter()
                    .map(|line| format!("   {line}"))
                    .collect::<Vec<_>>()
                    .join("\n"),
            );
        }
    }
    Ok(lines.join("\n"))
}

/// What the sandbox reaches, and the limits that come with it.
pub fn format_sandbox_plan(plan: &J, colour: bool) -> R<String> {
    let reason = f(plan, "reason")?;
    let mut lines = vec![
        format!(
            "sandbox {} · {} · {}{}",
            s(f(plan, "project_id")?),
            s(f(plan, "backend")?),
            if f(plan, "available")?.truthy() {
                paint("available", Some("success"), colour)
            } else {
                paint("unavailable", Some("danger"), colour)
            },
            if reason.truthy() {
                format!(" · {}", s(reason))
            } else {
                String::new()
            }
        ),
        format!("  starts in {}", s(f(plan, "workdir")?)),
    ];
    lines.extend(spread_map(f(plan, "repositories")?, |r| {
        let (host, sandbox) = (f(r, "host_path")?, f(r, "sandbox_path")?);
        Ok(format!(
            "  read-write {}: {}{}",
            s(f(r, "repository_id")?),
            s(host),
            if strictly_different(sandbox, host) {
                format!(" (as {})", s(sandbox))
            } else {
                String::new()
            }
        ))
    })?);
    lines.push(format!("  home {}", s(f(plan, "home")?)));
    lines.push(format!("  network {}", s(f(plan, "network")?)));
    lines.extend(spread_map(f(plan, "notes")?, |note| {
        Ok(format!("  note: {}", s(note)))
    })?);
    lines.push(format!(
        "  open it with: lu sbx {}{}",
        s(f(plan, "project_id")?),
        if f(plan, "network")?.as_str() == Some("none") {
            " --network none"
        } else {
            ""
        }
    ));
    Ok(lines.join("\n"))
}

pub fn format_doctor(report: &J) -> R<String> {
    let pad = |value: &J, width: usize| match value {
        J::Str(text) => Ok(js::pad_end(text, width)),
        _ => Err(type_error()),
    };
    let mut lines = map(f(report, "items")?, |item| {
        Ok(format!(
            "{} {} {}{}",
            pad(f(item, "status")?, 8)?,
            pad(f(item, "name")?, 20)?,
            s(f(item, "detail")?),
            if f(item, "required")?.truthy() {
                " (required)"
            } else {
                ""
            }
        ))
    })?;
    lines.push(format!(
        "doctor: {}",
        if f(report, "ok")?.truthy() {
            "ok"
        } else {
            "FAILED"
        }
    ));
    Ok(lines.join("\n"))
}

fn format_ticket(ticket: &J) -> R<String> {
    let usage = f(ticket, "usage")?;
    let usage_text = if usage.truthy() {
        format!(
            " · {} workflow(s), {} attempt(s), {} repair(s), {}s, {}",
            s(f(usage, "workflows")?),
            s(f(usage, "attempts")?),
            s(f(usage, "repairs")?),
            s(f(usage, "elapsed_seconds")?),
            dollars(f(usage, "cost_usd_micros")?)
        )
    } else {
        String::new()
    };
    Ok(format!(
        "{}  r{}  {}  {}{usage_text}\n  {}\n  criteria: {}",
        sid(f(ticket, "id")?),
        s(f(ticket, "revision")?),
        s(f(ticket, "source")?),
        s(f(ticket, "title")?),
        s(f(ticket, "intent")?),
        join(f(ticket, "acceptance_criteria")?, " | ")?
    ))
}

fn format_discovery(discovery: &J) -> R<String> {
    let repositories = f(discovery, "repositories")?;
    let mut checks = Vec::new();
    for repository in map(
        if repositories.nullish() {
            &EMPTY_ARR
        } else {
            repositories
        },
        |r| Ok(r.clone()),
    )? {
        let proposed = f(&repository, "proposed_checks")?;
        for line in map(
            if proposed.nullish() {
                &EMPTY_ARR
            } else {
                proposed
            },
            |proposed| {
                let check = f(proposed, "check")?;
                Ok(format!(
                    "  {}: {} {}  {} · {} · {}",
                    s(f(&repository, "repository_id")?),
                    s(f(check, "executable")?),
                    join(f(check, "args")?, " ")?,
                    if f(check, "required")?.truthy() {
                        "required"
                    } else {
                        "optional"
                    },
                    s(f(proposed, "confidence")?),
                    join(f(proposed, "evidence")?, " · ")?
                ))
            },
        )? {
            checks.push(line);
        }
    }
    let mut lines = vec![format!(
        "Discovery {} proposes manifest revision {}",
        s(f(discovery, "id")?),
        s(o(f(discovery, "proposed_manifest")?, "revision"))
    )];
    let questions = f(discovery, "questions")?;
    if olen(questions).truthy() {
        lines.push("open questions:".into());
        lines.extend(map(questions, |q| {
            Ok(format!("  {}: {}", s(f(q, "topic")?), s(f(q, "detail")?)))
        })?);
    }
    let problems = f(discovery, "manifest_problems")?;
    if olen(problems).truthy() {
        lines.push(format!(
            "proposal needs edits before confirmation: {}",
            join(problems, "; ")?
        ));
    }
    lines.push("proposed checks:".into());
    if checks.is_empty() {
        lines.push("  none".into());
    } else {
        lines.extend(checks);
    }
    lines.push("proposed manifest (edit, then confirm it with `confirm <project> <file>`):".into());
    lines.push(
        f(discovery, "proposed_manifest")?
            .stringify_pretty()
            .unwrap_or_default(),
    );
    Ok(lines.join("\n"))
}

fn format_plan_record(plan: &J) -> R<String> {
    let base = f(plan, "base_commits")?;
    let base_entries = entries(if base.nullish() { &EMPTY_OBJ } else { base })?;
    let task = first_item(f(plan, "tasks")?);
    let budgets = f(plan, "budgets")?;
    Ok([
        format!(
            "Plan {} revision {}  ticket {} r{}",
            sid(f(plan, "id")?),
            s(f(plan, "revision")?),
            sid(f(plan, "ticket_id")?),
            s(f(plan, "ticket_revision")?)
        ),
        format!("immutable plan hash: {}", s(f(plan, "hash")?)),
        format!(
            "base: {} · paths {} · operations {}",
            base_entries
                .iter()
                .map(|(id, commit)| format!("{id} {}", s(commit)))
                .collect::<Vec<_>>()
                .join(", "),
            or(&ojoin(o(&task, "paths"), ", ")?, ""),
            or(&ojoin(o(&task, "operations"), ", ")?, "")
        ),
        format!(
            "budgets: {} attempt(s), {}s, {}",
            s(o(budgets, "max_attempts")),
            s(o(budgets, "timeout_seconds")),
            dollars(o(budgets, "cost_usd_micros"))
        ),
        "complete plan:".into(),
        plan.stringify_pretty().unwrap_or_default(),
    ]
    .join("\n"))
}

/// Renders one command result for a terminal; `id` is the catalog id; `colour` paints the tones.
pub fn format(id: &str, value: &J, colour: bool) -> R<String> {
    if value.nullish() {
        return Ok(String::new());
    }
    let v = value;
    Ok(match id {
        "doctor" => format_doctor(v)?,
        "health" => format!(
            "runtime {} · Pi {} · native execution {} · build {}",
            paint(&s(f(v, "status")?), Some(if f(v, "status")?.as_str() == Some("ok") { "success" } else { "danger" }), colour),
            s(f(v, "pi_version")?),
            if f(v, "native_execution")?.truthy() { "yes" } else { "no" },
            or(f(v, "build_identity")?, "unknown")
        ),
        "projects.list" => listed(v, "No projects registered.", |project| {
            Ok(format!("{}  {}  {}", s(f(project, "id")?), if positive(f(project, "revision")?) { format!("manifest r{}", s(f(project, "revision")?)) } else { "unconfirmed".into() }, s(f(project, "name")?)))
        })?,
        "projects.show" | "projects.register" | "projects.manifest.confirm" | "projects.repositories.rebind" => format_project(v)?,
        "projects.discover" => format_discovery(v)?,
        "projects.events" | "runs.events" => listed(v, "No events.", event_line)?,
        "baselines.start" | "runs.start" | "runs.show" | "runs.cancel" | "runs.implement" | "runs.validate" | "runs.repair" => run_line(v, colour)?,
        "baselines.list" => listed(v, "No baselines recorded.", |b| format_baseline(b, colour))?,
        "runs.list" => listed(v, "No runs.", |run| run_line(run, colour))?,
        "components.status" | "components.enable" => {
            let (path, reason) = (f(v, "workspace_path")?, f(v, "reason")?);
            format!(
                "components {}{}{}",
                if f(v, "enabled")?.truthy() { "enabled" } else { "not enabled" },
                if path.truthy() { format!(" · {}", s(path)) } else { String::new() },
                if reason.truthy() { format!(" · {}", s(reason)) } else { String::new() }
            )
        }
        "projects.storemode.show" | "projects.storemode.set" => {
            let (mode, repository, problem) = (f(v, "mode")?, f(v, "repository")?, f(v, "problem")?);
            let choices = map(f(v, "choices")?, |c| {
                let repository = f(c, "repository")?;
                Ok(if repository.truthy() { format!("repository {}", s(repository)) } else { s(f(c, "mode")?) })
            })?;
            format!(
                "store mode {}{}{}\n  can be changed to: {}",
                if mode.truthy() { s(mode) } else { "unreadable".into() },
                if repository.truthy() { format!(" · repository {}", s(repository)) } else { String::new() },
                if problem.truthy() { format!("\n  not usable now: {}", s(problem)) } else { String::new() },
                choices.join(", ")
            )
        }
        "memory.list" => listed(v, "No memory records in this view.", |r| {
            Ok(format!("{}  {}  {}\n  {}\n  provenance: {}", sid(f(r, "id")?), s(f(r, "scope")?), shash(f(r, "hash")?), s(f(r, "content")?), s(f(r, "provenance")?)))
        })?,
        "memory.create" => format!("{}  {}  {}", s(f(v, "id")?), s(f(v, "scope")?), s(f(v, "hash")?)),
        "memory.delete" => if f(v, "deleted")?.truthy() { "Memory deleted." } else { "Memory not deleted." }.into(),
        "prompts.list" => listed(v, "No prompts.", |prompt| {
            let hash = f(prompt, "hash")?;
            Ok(format!("{}  version {}{}", s(f(prompt, "name")?), s(f(prompt, "version")?), if hash.truthy() { format!("  {}", s(hash)) } else { String::new() }))
        })?,
        "prompts.create" | "prompts.render" => format!("{}  version {}  {}\n{}", s(f(v, "name")?), s(f(v, "version")?), s(f(v, "hash")?), s(f(v, "body")?)),
        "intent.show" | "intent.init" => format!("{}  tier {}  {} nodes  {} edges", s(f(f(v, "settings")?, "project")?), s(f(f(v, "settings")?, "tier")?), count(f(v, "nodes")?), count(f(v, "edges")?)),
        "intent.proposals" | "intent.questions" | "intent.sprouts" => listed(v, "No pending intent records.", |r| {
            Ok(format!("{}  {}  {}  {}", s(f(r, "id")?), or(o(r, "kind"), &or(o(r, "node"), &or(o(r, "slug"), ""))), s(f(r, "status")?), or(o(r, "by"), &or(o(r, "author"), ""))))
        })?,
        "intent.proposals.accept" | "intent.proposals.reject" => listed(f(v, "proposals")?, "No proposals decided.", |r| Ok(format!("{}  {}  {}", s(f(r, "id")?), s(f(r, "status")?), s(f(r, "decidedBy")?))))?,
        "intent.sprouts.reject" => format!("{}  {}  {}", s(f(v, "id")?), s(f(v, "status")?), s(f(v, "decidedBy")?)),
        "tickets.create" | "tickets.show" | "tickets.reconcile" | "tickets.update" => format_ticket(v)?,
        "memory.update" => format!("{}  {}  {}\n  {}\n  provenance: {}", s(f(v, "id")?), s(f(v, "scope")?), s(f(v, "hash")?), s(f(v, "content")?), s(f(v, "provenance")?)),
        "memory.user.export" => {
            let memory = f(v, "memory")?;
            format!(
                "{} private user memory record(s) exported at {} · omits {}\n{}",
                s(&len(memory)?),
                s(f(v, "exported_at")?),
                join(f(v, "omitted")?, ", ")?,
                map(memory, |r| Ok(format!("{}  {}\n  {}\n  provenance: {}", s(f(r, "id")?), s(f(r, "hash")?), s(f(r, "content")?), s(f(r, "provenance")?))))?.join("\n")
            )
        }
        "memory.retention.show" | "memory.retention.set" => {
            let days = |value: &J| if value == &J::Null { "kept".to_string() } else { format!("{} day(s)", s(value)) };
            format!(
                "process memory: {} · project memory: {} · user memory is never subject to a project policy",
                days(f(v, "process_max_age_days")?),
                days(f(v, "project_max_age_days")?)
            )
        }
        "memory.retention.apply" => {
            let deleted = f(v, "deleted")?;
            if len(deleted)?.truthy() {
                format!(
                    "Deleted {} expired record(s) at {}: {}",
                    s(&len(deleted)?),
                    s(f(v, "applied_at")?),
                    map(deleted, |r| Ok(format!("{} ({}, created {})", s(f(r, "id")?), s(f(r, "scope")?), s(f(r, "created_at")?))))?.join(", ")
                )
            } else {
                format!("No memory record exceeded the retention policy at {}.", s(f(v, "applied_at")?))
            }
        }
        "tickets.list" => listed(v, "No tickets.", format_ticket)?,
        "tickets.source" => {
            let workspace = f(v, "workspace")?;
            format!(
                "source {} · id {} · revision {} · reconciliation {}{}",
                or(f(v, "source")?, "unknown"),
                or(f(v, "source_id")?, "none"),
                or(f(v, "source_revision")?, "none"),
                if f(v, "reconciliation_required")?.truthy() { "REQUIRED" } else { "not required" },
                if workspace.truthy() { format!(" · {}", s(workspace)) } else { String::new() }
            )
        }
        "plans.create" => format!("{}\nworkflow run: {}", format_plan_record(f(v, "plan")?)?, run_line(f(v, "run")?, colour)?),
        "plans.list" => listed(v, "No plan revisions.", |plan| Ok(format!("{}  r{}  {}", sid(f(plan, "id")?), s(f(plan, "revision")?), shash(f(plan, "hash")?))))?,
        "plans.show" => format_plan_record(v)?,
        "plans.approve" => {
            let approval = f(v, "approval")?;
            format!(
                "Approved plan {} r{} by {} (hash {})\nworkflow run: {}",
                sid(f(approval, "plan_id")?),
                s(f(approval, "plan_revision")?),
                s(f(approval, "approved_by")?),
                s(f(approval, "plan_hash")?),
                run_line(f(v, "run")?, colour)?
            )
        }
        "runs.stream" => format!("Followed {}: {} event(s), last sequence {}, run {}.", sid(f(v, "run_id")?), s(f(v, "events_followed")?), s(f(v, "last_sequence")?), s(f(v, "state")?)),
        "runs.candidate" => format_candidate(v, colour)?,
        "runs.diff" => {
            let mut items = vec![J::str(format!("Diff for {}  {} · base {} · candidate commit {}", s(f(v, "candidate_hash")?), s(f(v, "repository_id")?), s(f(v, "base_commit")?), s(f(v, "commit")?)))];
            let diff = f(v, "diff")?;
            let truncated = f(v, "truncated")?.truthy();
            items.push(if diff.truthy() { diff.clone() } else { J::str(if truncated { "Diff omitted because it exceeds the display limit." } else { "No changes." }) });
            if truncated {
                items.push(J::str("This diff is truncated. It does not show all candidate changes."));
            }
            let drift = f(v, "worktree_drift")?;
            if drift.truthy() {
                items.push(drift.clone());
            }
            let files = f(v, "files")?;
            if files.truthy() {
                items.extend(map(files, |file| Ok(J::str(format!("  {} · {} · {}", s(f(file, "path")?), s(f(file, "status")?), s(f(file, "shown")?)))))?);
            }
            join_values(&items, "\n")
        }
        "runs.worktrees" => listed(v, "No worktree leases.", |lease| {
            let (commit, reason) = (f(lease, "candidate_commit")?, f(lease, "reason")?);
            Ok(format!(
                "{}  {}  {}  {}{}{}",
                s(f(lease, "id")?),
                p(f(lease, "state")?, lease_tone, colour),
                s(f(lease, "repository_id")?),
                s(f(lease, "path")?),
                if commit.truthy() { format!(" · candidate {}", s(commit)) } else { String::new() },
                if reason.truthy() { format!(" · {}", s(reason)) } else { String::new() }
            ))
        })?,
        "runs.attempts" => listed(v, "No writer attempts recorded.", |attempt| {
            let finished = f(attempt, "finished_at")?;
            Ok(format!(
                "attempt {}  {}  task {}  {}{}",
                s(f(attempt, "number")?),
                s(f(attempt, "state")?),
                s(f(attempt, "task_id")?),
                when(f(attempt, "started_at")?),
                if finished.truthy() { format!(" → {}", when(finished)) } else { String::new() }
            ))
        })?,
        "runs.tasks" => {
            if len(v)?.truthy() {
                map(v, task_lines)?.into_iter().flatten().collect::<Vec<_>>().join("\n")
            } else {
                "No task records: a single-task implementer plan runs the M2c writer path and records no task graph.".into()
            }
        }
        "projection.home" => format_home(v, colour)?,
        "projection.board" => format_board(v, colour)?,
        "artifacts.read" => if v.truthy() { s(v) } else { "Empty log.".into() },
        "orbit.status" => format_orbit(v, colour)?,
        "orbit.exec" | "orbit.run" | "orbit.sync" => format_orbit_execution(v, colour)?,
        "orbit.sandbox" => format_sandbox_plan(v, colour)?,
        "shutdown" => format!("Runtime {}. Runs, candidates and worktrees stay durable; start it again with the same data directory and token.", s(f(v, "status")?)),
        _ => match v {
            J::Str(text) => text.clone(),
            other => other.stringify_pretty().unwrap_or_default(),
        },
    })
}

/// The SGR-free text a frame or a pane shows (control characters removed).
pub fn plain(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            match chars.peek() {
                Some(']') => {
                    // OSC: up to BEL or ESC \ (or the end).
                    chars.next();
                    while let Some(n) = chars.next() {
                        if n == '\u{7}' {
                            break;
                        }
                        if n == '\u{1b}' {
                            if chars.peek() == Some(&'\\') {
                                chars.next();
                            }
                            break;
                        }
                    }
                    continue;
                }
                Some('[') => {
                    chars.next();
                    while chars.peek().is_some_and(|n| ('0'..='?').contains(n)) {
                        chars.next();
                    }
                    while chars.peek().is_some_and(|n| (' '..='/').contains(n)) {
                        chars.next();
                    }
                    if chars.peek().is_some_and(|n| ('@'..='~').contains(n)) {
                        chars.next();
                        continue;
                    }
                    continue;
                }
                _ => continue,
            }
        }
        let code = c as u32;
        if code <= 0x08 || (0x0b..=0x1f).contains(&code) || (0x7f..=0x9f).contains(&code) {
            continue;
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_render_like_the_original() {
        let run = js::parse(r#"{"id":"run-1","state":"running","metadata":{"kind":"baseline","repository_id":"app","manifest_revision":2},"created_at":"2026-10-21T10:00:00.123Z"}"#).unwrap();
        assert_eq!(
            run_line(&run, false).unwrap(),
            "run-1  running  baseline · app · manifest r2  2026-10-21 10:00:00Z"
        );
        assert_eq!(
            run_line(&run, true).unwrap(),
            "run-1  \u{1b}[36mrunning\u{1b}[0m  baseline · app · manifest r2  2026-10-21 10:00:00Z"
        );
        assert_eq!(
            format("projects.list", &J::obj(), false).unwrap(),
            "No projects registered."
        );
        assert!(format("projects.list", &J::str("x"), false).is_err());
        assert_eq!(
            format("artifacts.read", &J::str(""), false).unwrap(),
            "Empty log."
        );
        assert_eq!(
            format("projects.storemode.show", &js::parse(r#"{"mode":"repository","repository":"api","usable":false,"problem":"no checkout","choices":[{"mode":"runtime","repository":null},{"mode":"repository","repository":"api"}]}"#).unwrap(), false).unwrap(),
            "store mode repository · repository api\n  not usable now: no checkout\n  can be changed to: runtime, repository api"
        );
        assert_eq!(format("memory.retention.show", &js::parse(r#"{"process_max_age_days":null,"project_max_age_days":30}"#).unwrap(), false).unwrap(), "process memory: kept · project memory: 30 day(s) · user memory is never subject to a project policy");
        assert_eq!(dollars(&J::Num(1234.0)), "$0.0012");
        assert_eq!(plain("a\u{1b}[31mb\u{1b}[0mc\u{7}"), "abc");
    }
}
