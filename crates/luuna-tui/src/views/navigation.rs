//! Moving between screens, loading what each one shows, and choosing a project.
use super::*;

pub fn choose_project(app: &mut AppState, id: &str) {
    if project(app) != id {
        app.context.clear();
        app.context.insert("project".into(), id.into());
        app.snapshots.clear();
        app.snapshot_requests.clear();
        app.workbench.memory_ids.clear();
        app.workbench.prompt_selection = None;
        app.workbench.include_user = false;
        app.workbench.orbit_detail = None;
        app.workbench.selected = Value::Null;
        app.workbench.query.clear();
        app.last_read = None;
    }
}
pub fn open(app: &mut AppState, screen: Screen) {
    if screen == Screen::Setup {
        app.workbench.nav = false;
        guide::begin(app, "init", Map::new());
        return;
    }
    if app.active_request.is_some() {
        app.status = "Wait for this request, or Esc to stop waiting.".into();
        return;
    }
    app.workbench.screen = screen;
    app.workbench.cursor = 0;
    app.scroll.reset();
    app.workbench.nav = false;
    app.workbench.query.clear();
    app.workbench.error = None;
    app.workbench.selected_id = None;
    app.mode = if screen == Screen::Commands {
        Mode::Catalog
    } else {
        Mode::Workbench
    };
    if screen == Screen::Commands {
        app.filter.clear();
        app.selected = 0;
    } else {
        load(app);
    }
}
pub(super) fn read(app: &mut AppState, name: &str, args: Vec<String>, flags: Map<String, Value>) {
    app.invoke(name, args, flags, true);
    app.mode = Mode::Workbench;
}
pub fn load(app: &mut AppState) {
    if app.active_request.is_some() {
        return;
    }
    let mut flags = Map::new();
    let p = project(app);
    let (name, args) = match app.workbench.screen {
        Screen::Home => ("home", vec![]),
        Screen::Projects => ("projects", vec![]),
        Screen::Project => ("project", vec![p.clone()]),
        Screen::Orbit => ("orbit", vec![p.clone()]),
        Screen::Missions => {
            if !p.is_empty() {
                flags.insert("project".into(), json!(p));
            }
            ("board", vec![])
        }
        Screen::Memory => {
            flags.insert("include-user".into(), json!(app.workbench.include_user));
            if !app.workbench.query.is_empty() {
                flags.insert("query".into(), json!(app.workbench.query));
            }
            ("memory", vec![p.clone()])
        }
        Screen::Prompts => ("prompts", vec![p.clone()]),
        Screen::Runs => {
            if !p.is_empty() {
                flags.insert("project".into(), json!(p));
            }
            ("runs", vec![])
        }
        Screen::Ticket => (
            "show-ticket",
            vec![app.context.get("ticket").cloned().unwrap_or_default()],
        ),
        Screen::PromptDetail => (
            "render-prompt",
            vec![
                p.clone(),
                text(&app.workbench.selected, "name"),
                text(&app.workbench.selected, "version"),
            ],
        ),
        Screen::RunDetail => (
            EVIDENCE[app.workbench.evidence].0,
            vec![app.context.get("run").cloned().unwrap_or_default()],
        ),
        Screen::Plan => (
            "show-plan",
            vec![app.context.get("plan").cloned().unwrap_or_default()],
        ),
        _ => return,
    };
    if args.iter().any(String::is_empty) {
        app.status = "Select a project or record first (F3 Projects).".into();
        return;
    }
    app.workbench.selected_id = rows(app).get(app.workbench.cursor).map(identity);
    read(app, name, args, flags);
}
pub(super) fn inspect(app: &mut AppState) {
    let v = selected(app);
    if v.is_null() {
        return;
    }
    match app.workbench.screen {
        Screen::Home | Screen::Projects => {
            let id = text(&v, "id");
            choose_project(app, &id);
            open(app, Screen::Project);
        }
        Screen::Missions => {
            let p = text(&v, "project_id");
            if !p.is_empty() {
                choose_project(app, &p);
            }
            let ticket = text(&v, "ticket_id");
            if ticket.is_empty() {
                app.context
                    .insert("repository".into(), text(&v, "repository_id"));
                open(app, Screen::Project);
            } else {
                app.context.remove("plan");
                app.context.insert("ticket".into(), ticket);
                let run = text(&v, "run_id");
                if !run.is_empty() {
                    app.context.insert("run".into(), run);
                } else {
                    app.context.remove("run");
                }
                open(app, Screen::Ticket);
            }
        }
        Screen::Memory => {
            app.workbench.selected = v;
            open(app, Screen::MemoryDetail);
        }
        Screen::Prompts => {
            app.workbench.selected = v;
            open(app, Screen::PromptDetail);
        }
        Screen::Runs => {
            app.context.remove("ticket");
            app.context.insert("run".into(), text(&v, "id"));
            let plan = text(&v["metadata"], "plan_id");
            if !plan.is_empty() {
                app.context.insert("plan".into(), plan);
            } else {
                app.context.remove("plan");
            }
            app.workbench.evidence = 0;
            open(app, Screen::RunDetail);
        }
        _ => {}
    }
}
pub fn back(app: &mut AppState) {
    if app.active_request.is_some() {
        app.abort();
        return;
    }
    if app.workbench.searching {
        app.workbench.searching = false;
        return;
    }
    if app.workbench.nav {
        app.workbench.nav = false;
        return;
    }
    if app.mode != Mode::Workbench {
        app.mode = Mode::Workbench;
        app.form = None;
        return;
    }
    let screen = match app.workbench.screen {
        Screen::Project => Screen::Projects,
        Screen::Ticket => Screen::Missions,
        Screen::MemoryDetail => Screen::Memory,
        Screen::PromptDetail => Screen::Prompts,
        Screen::RunDetail => Screen::Runs,
        Screen::Plan => Screen::Ticket,
        _ => {
            app.workbench.nav = true;
            return;
        }
    };
    open(app, screen);
}
