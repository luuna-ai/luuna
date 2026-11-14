use crate::{
    actions::record::{message, text},
    json::{Object, Value},
    records, Result,
};
use std::path::Path;
const SPECS: [(&str, &str, &str); 2] = [
    ("SessionStart", "", "luuna-atlas prime"),
    (
        "PreToolUse",
        "Read|Edit|Write|MultiEdit|NotebookEdit|Bash",
        "luuna-atlas hook",
    ),
];
fn group(matcher: &str, command: &str) -> Value {
    let mut handler = Object::new();
    handler.insert("type", "command");
    handler.insert("command", command);
    handler.remember_order(["type", "command"]);
    let mut group = Object::new();
    group.insert("matcher", matcher);
    group.insert("hooks", Value::Array(vec![Value::Object(handler)]));
    group.remember_order(["matcher", "hooks"]);
    Value::Object(group)
}
fn ascii_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r' | '\x0c')
}
fn handler(v: &Value) -> bool {
    let command = text(records::obj(Some(v)).get("command"));
    let command = command.trim_start_matches(ascii_space);
    for name in ["luuna-atlas", "mulch", "ml"] {
        if let Some(rest) = command.strip_prefix(name) {
            let trimmed = rest.trim_start_matches(ascii_space);
            if trimmed.len() == rest.len() {
                continue;
            }
            for op in ["prime", "hook"] {
                if let Some(tail) = trimmed.strip_prefix(op) {
                    if tail.is_empty() || tail.starts_with(ascii_space) {
                        return true;
                    }
                }
            }
        }
    }
    false
}
fn groups(hooks: &Object, event: &str) -> Result<Vec<Value>> {
    match hooks.get(event) {
        None => Ok(Vec::new()),
        Some(Value::Array(a)) => Ok(a.clone()),
        _ => Err(message(format!(
            "`hooks.{event}` in Claude settings is not an array; nothing was written"
        ))),
    }
}
fn hooks(settings: &Object) -> Result<Object> {
    match settings.get("hooks") {
        None => Ok(Object::new()),
        Some(Value::Object(o)) => Ok(o.clone()),
        _ => Err(message(
            "`hooks` in Claude settings is not an object; nothing was written",
        )),
    }
}
fn read(path: &Path) -> Result<Option<Object>> {
    if !path.exists() {
        return Ok(None);
    }
    let bytes = std::fs::read(path).unwrap_or_default();
    let raw = String::from_utf8_lossy(&bytes);
    if raw.trim().is_empty() {
        return Ok(Some(Object::new()));
    }
    let value = super::json_input::parse(&bytes).map_err(|e| {
        message(format!(
            "Failed to parse Claude settings at {}: {e}",
            path.display()
        ))
    })?;
    match value {
        Value::Object(ref o) => Ok(Some(o.clone())),
        _ => Err(message(format!(
            "Claude settings at {} is not a JSON object",
            path.display()
        ))),
    }
}
fn installed(groups: &[Value], matcher: &str, command: &str) -> bool {
    let ours: Vec<_> = groups
        .iter()
        .filter(|g| {
            records::any_list(records::obj(Some(g)).get("hooks"))
                .iter()
                .any(handler)
        })
        .collect();
    ours.len() == 1 && records::go_deep_equal(ours[0], &group(matcher, command))
}
/// Returns the compatibility result and prospective write. No file is written here.
pub(super) fn prepare(cwd: &Path, action: &str) -> Result<(bool, String, Option<String>)> {
    let path = cwd.join(".claude/settings.json");
    let settings = read(&path)?;
    if action == "check" {
        let Some(settings) = settings else {
            return Ok((false, "Claude settings.json not found.".into(), None));
        };
        let hooks = hooks(&settings)?;
        let mut missing = Vec::new();
        for (event, matcher, command) in SPECS {
            if !installed(&groups(&hooks, event)?, matcher, command) {
                missing.push(event)
            }
        }
        return Ok(if missing.is_empty() {
            (true, "Claude hooks are installed and correct.".into(), None)
        } else {
            (
                false,
                format!("Missing hooks for: {}.", missing.join(", ")),
                None,
            )
        });
    }
    if settings.is_none() && action == "remove" {
        return Ok((
            true,
            "Claude settings.json not found; nothing to remove.".into(),
            None,
        ));
    }
    let mut settings = settings.unwrap_or_default();
    let mut hooks = hooks(&settings)?;
    let mut order: Vec<String> = hooks.keys().map(str::to_owned).collect();
    let events: Vec<String> = if action == "remove" {
        order.clone()
    } else {
        SPECS.iter().map(|s| s.0.to_owned()).collect()
    };
    let mut changed = false;
    for event in events {
        let old = groups(&hooks, &event)?;
        let spec = SPECS.iter().find(|s| s.0 == event);
        if action != "remove" && spec.is_some_and(|s| installed(&old, s.1, s.2)) {
            continue;
        }
        let mut updated = Vec::new();
        for g in &old {
            let o = records::obj(Some(g));
            let handlers = records::any_list(o.get("hooks"));
            let kept: Vec<_> = handlers.iter().filter(|h| !handler(h)).cloned().collect();
            for removed in handlers.iter().filter(|h| handler(h)) {
                let removed = records::obj(Some(removed));
                if removed.keys().any(|k| k != "type" && k != "command") {
                    return Err(message("Refusing to discard extra fields from an Atlas hook handler; nothing was written."));
                }
            }
            if kept.len() == handlers.len() {
                updated.push(g.clone())
            } else if !kept.is_empty() {
                let mut next = o.clone();
                next.insert_unordered("hooks", Value::Array(kept));
                updated.push(Value::Object(next))
            } else {
                if o.keys().any(|k| k != "matcher" && k != "hooks") {
                    return Err(message("Refusing to discard extra fields from a Claude hook group while replacing its Atlas handlers; nothing was written."));
                }
                for h in handlers {
                    let h = records::obj(Some(h));
                    if h.keys().any(|k| k != "type" && k != "command") {
                        return Err(message("Refusing to discard extra fields from an Atlas hook handler; nothing was written."));
                    }
                }
            }
        }
        if action != "remove" {
            if let Some(s) = spec {
                updated.push(group(s.1, s.2))
            }
        }
        if records::go_deep_equal(&Value::Array(old), &Value::Array(updated.clone())) {
            continue;
        }
        changed = true;
        if updated.is_empty() {
            hooks.remove(&event);
        } else {
            hooks.insert_unordered(&event, Value::Array(updated));
            if !order.contains(&event) {
                order.push(event)
            }
        }
    }
    hooks.remember_order(order.iter().map(String::as_str));
    let next = if changed {
        if hooks.is_empty() {
            settings.remove("hooks");
        } else {
            settings.insert_unordered("hooks", hooks);
        }
        {
            super::json_rewrite::check(&path, &settings)?;
            Some(crate::CommandOutput::json(&Value::Object(settings)).stdout)
        }
    } else {
        None
    };
    let text = if action == "remove" {
        if changed {
            "Removed luuna-atlas hooks from Claude settings."
        } else {
            "No luuna-atlas hooks found in Claude settings."
        }
    } else if changed {
        "Installed Claude hooks: SessionStart `luuna-atlas prime`, PreToolUse `luuna-atlas hook` (file-anchored records + .luuna/atlas/expertise guard)."
    } else {
        "Claude hooks already installed."
    };
    Ok((true, text.into(), next))
}
