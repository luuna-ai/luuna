//! What the workbench does after a bridge result: follow-on screens and adopted identities.
use super::*;

/// Called after the core protocol reducer, with the original request identity captured first.
pub fn received(app: &mut AppState, event: &BridgeEvent, command: Option<&str>, was_read: bool) {
    if event.event == "catalog" && !app.workbench.initialized && !app.commands.is_empty() {
        app.workbench.initialized = true;
        open(app, Screen::Home);
        return;
    }
    let Some(command) = command else {
        return;
    };
    if event.event == "error" {
        app.workbench.error = Some(app.status.clone());
        return;
    }
    if event.event != "result" {
        return;
    }
    app.workbench.error = None;
    if let Some(id) = app.workbench.selected_id.take() {
        if let Some(index) = rows(app).iter().position(|v| identity(v) == id) {
            app.workbench.cursor = index;
        }
    }
    if app.mode == Mode::Workbench {
        app.workbench.cursor = app.workbench.cursor.min(rows(app).len().saturating_sub(1));
    }
    let v = event.fields.get("value").cloned().unwrap_or(Value::Null);
    adopt_identity(app, command, &v);
    // A successful authoritative run read is the only link used by its Plan action.
    if command == "run" && text(&v, "id") == app.context.get("run").cloned().unwrap_or_default() {
        app.context.remove("plan");
        app.context.remove("ticket");
        for (name, key) in [("plan", "plan_id"), ("ticket", "ticket_id")] {
            let id = text(&v["metadata"], key);
            if !id.is_empty() {
                app.context.insert(name.into(), id);
            }
        }
    }
    if was_read {
        return;
    }
    match command {
        "register" | "import" => open(app, Screen::Projects),
        // The removed project is no longer listed, so nothing points at it.
        "remove-project" => {
            for key in ["project", "ticket", "plan", "run"] {
                app.context.remove(key);
            }
            open(app, Screen::Projects);
        }
        "confirm" | "enable-components" | "rebind" => open(app, Screen::Project),
        "plan" => {
            let p = v.get("plan").unwrap_or(&v);
            let id = text(p, "id");
            let run = text(&v["run"], "id");
            if !id.is_empty() {
                app.context.insert("plan".into(), id);
            }
            if !run.is_empty() {
                app.context.insert("run".into(), run);
            }
            if app.context.contains_key("plan") {
                open(app, Screen::Plan);
            }
        }
        // The proposal just shown is what the scaffold form confirms; it is never typed (ADR 0042).
        "propose-scaffold" => {
            let hash = text(&v, "hash");
            if !hash.is_empty() {
                app.context.insert("proposal-hash".into(), hash);
            }
        }
        "create-ticket" | "edit-ticket" | "reconcile-ticket" => open(app, Screen::Missions),
        "remember" | "correct-memory" | "forget" | "promote" => open(app, Screen::Memory),
        "save-prompt" => open(app, Screen::Prompts),
        "approve" => open(app, Screen::Plan),
        "implement" | "cancel" | "validate" | "repair" => open(app, Screen::RunDetail),
        // Show the per-repository outcome in the Orbit view and re-read the status.
        "orbit-exec" | "orbit-run" | "orbit-sync" => {
            app.workbench.orbit_detail = Some(command.to_owned());
            open(app, Screen::Orbit);
        }
        _ => {}
    }
    if app.active_request.is_some() {
        let preferred = match command {
            "create-ticket" | "edit-ticket" | "reconcile-ticket" => {
                let id = text(&v, "id");
                if id.is_empty() {
                    app.context.get("ticket").cloned().unwrap_or_default()
                } else {
                    id
                }
            }
            "remember" | "correct-memory" => text(&v, "id"),
            "promote" => text(&v["promoted"], "id"),
            "save-prompt" => text(&v, "name"),
            _ => String::new(),
        };
        if !preferred.is_empty() {
            app.workbench.selected_id = Some(preferred);
        }
    }
}
/// A read or run action sent with a short id or `latest` that nothing loaded resolved (ADR 0042):
/// the record the runtime returned is the one it named (run actions return the run they acted
/// on), so its exact identity becomes the context and the snapshot's provenance. The context was
/// cleared when the reference was sent.
pub(super) fn adopt_identity(app: &mut AppState, command: &str, v: &Value) {
    let name = match command {
        "run" | "cancel" | "validate" | "repair" | "implement" => "run",
        "show-ticket" => "ticket",
        "show-plan" => "plan",
        _ => return,
    };
    let id = text(v, "id");
    let Some(request) = app.snapshot_requests.get_mut(command) else {
        return;
    };
    let Some(sent) = request
        .fields
        .get_mut("args")
        .and_then(Value::as_array_mut)
        .and_then(|args| args.first_mut())
    else {
        return;
    };
    let typed = sent.as_str().unwrap_or("").to_string();
    let named = (name == "run" && is_latest(&typed))
        || (crate::refs::is_short_reference(&typed)
            && crate::refs::unique_match(&typed, std::slice::from_ref(&id)).is_some());
    if id.is_empty() || !named || app.context.contains_key(name) {
        return;
    }
    *sent = json!(id);
    // A refresh repeats the read by the exact identity, not the reference it was typed as.
    let request_id = request.id.clone();
    if let Some(last) = app.last_read.as_mut().filter(|r| r.id == request_id) {
        if let Some(first) = last
            .fields
            .get_mut("args")
            .and_then(Value::as_array_mut)
            .and_then(|args| args.first_mut())
        {
            *first = json!(id);
        }
    }
    app.context.insert(name.into(), id);
}
