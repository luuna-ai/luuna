//! Reading the loaded snapshots: the rows a screen lists and the record it shows.
use super::*;

pub fn snapshot(app: &AppState, key: &str) -> Value {
    if let Some(request) = app.snapshot_requests.get(key) {
        let args = request.fields.get("args").and_then(Value::as_array);
        let arg = |i: usize| {
            args.and_then(|a| a.get(i))
                .and_then(Value::as_str)
                .unwrap_or("")
        };
        let context = |name: &str| app.context.get(name).map(String::as_str).unwrap_or("");
        let matches = match key {
            "project" | "memory" | "prompts" | "orbit" | "orbit-exec" | "orbit-run"
            | "orbit-sync" | "sbx" => arg(0) == context("project"),
            "show-ticket" => arg(0) == context("ticket"),
            "show-plan" => arg(0) == context("plan"),
            "run" | "candidate" | "diff" | "tasks" | "worktrees" | "attempts" | "events"
            | "publication" => arg(0) == context("run"),
            "render-prompt" => {
                arg(0) == context("project")
                    && arg(1) == text(&app.workbench.selected, "name")
                    && arg(2) == text(&app.workbench.selected, "version")
            }
            _ => true,
        };
        if !matches {
            return Value::Null;
        }
    }
    app.snapshots.get(key).cloned().unwrap_or(Value::Null)
}
pub(super) fn array(value: Value) -> Vec<Value> {
    value
        .as_array()
        .cloned()
        .or_else(|| value.get("runs").and_then(Value::as_array).cloned())
        .unwrap_or_default()
}
pub fn rows(app: &AppState) -> Vec<Value> {
    match app.workbench.screen {
        Screen::Home => array(snapshot(app, "home")["projects"].clone()),
        Screen::Projects => array(snapshot(app, "projects")),
        Screen::Orbit => array(snapshot(app, "orbit")["repositories"].clone()),
        Screen::Missions => {
            array(snapshot(app, "board")["columns"][app.workbench.column]["cards"].clone())
        }
        Screen::Memory => array(snapshot(app, "memory"))
            .into_iter()
            .filter(|record| app.workbench.include_user || text(record, "scope") != "user")
            .collect(),
        Screen::Prompts => array(snapshot(app, "prompts"))
            .into_iter()
            .filter(|r| {
                r.to_string()
                    .to_lowercase()
                    .contains(&app.workbench.query.to_lowercase())
            })
            .collect(),
        Screen::Runs => array(snapshot(app, "runs")),
        _ => Vec::new(),
    }
}
pub fn selected(app: &AppState) -> Value {
    let record = match app.workbench.screen {
        Screen::Project => snapshot(app, "project"),
        Screen::Ticket => snapshot(app, "show-ticket"),
        Screen::MemoryDetail => app.workbench.selected.clone(),
        Screen::PromptDetail => snapshot(app, "render-prompt"),
        Screen::RunDetail => snapshot(app, EVIDENCE[app.workbench.evidence].0),
        Screen::Plan => snapshot(app, "show-plan"),
        _ => rows(app)
            .get(app.workbench.cursor)
            .cloned()
            .unwrap_or(Value::Null),
    };
    let current = |name: &str| app.context.get(name).map(String::as_str).unwrap_or("");
    let valid = match app.workbench.screen {
        Screen::Ticket => text(&record, "id") == current("ticket") && !current("ticket").is_empty(),
        Screen::Plan => text(&record, "id") == current("plan") && !current("plan").is_empty(),
        Screen::RunDetail if app.workbench.evidence == 0 => {
            text(&record, "id") == current("run") && !current("run").is_empty()
        }
        Screen::Project => {
            text(record.get("project").unwrap_or(&record), "id") == current("project")
                && !current("project").is_empty()
        }
        Screen::PromptDetail => {
            text(&record, "name") == text(&app.workbench.selected, "name")
                && text(&record, "version") == text(&app.workbench.selected, "version")
        }
        _ => true,
    };
    if valid {
        record
    } else {
        Value::Null
    }
}
pub fn text(v: &Value, key: &str) -> String {
    v.get(key)
        .map(|s| {
            s.as_str().map(str::to_owned).unwrap_or_else(|| {
                if s.is_null() {
                    String::new()
                } else {
                    s.to_string()
                }
            })
        })
        .unwrap_or_default()
}
pub(super) fn identity(v: &Value) -> String {
    for k in ["id", "ticket_id", "name", "repository_id"] {
        let s = text(v, k);
        if !s.is_empty() {
            return s;
        }
    }
    String::new()
}
pub(super) fn project(app: &AppState) -> String {
    app.context.get("project").cloned().unwrap_or_default()
}
