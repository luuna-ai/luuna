//! Presentation state over the shared command bridge; no domain writes or HTTP live here.
pub mod guide;
pub mod render;
use crate::{
    app::{AppState, FormField, Mode},
    protocol::BridgeEvent,
};
use crossterm::event::{KeyCode, KeyEvent};
use serde_json::{json, Map, Value};
use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Screen {
    #[default]
    Home,
    Projects,
    Project,
    Orbit,
    Missions,
    Ticket,
    Memory,
    MemoryDetail,
    Prompts,
    PromptDetail,
    Runs,
    RunDetail,
    Plan,
    Commands,
    Help,
    /// Opens the setup guide (`prime`) instead of a workbench screen.
    Setup,
}
pub const NAV: &[(&str, Screen)] = &[
    ("Home", Screen::Home),
    ("Set up a project", Screen::Setup),
    ("Projects", Screen::Projects),
    ("Orbit", Screen::Orbit),
    ("Missions", Screen::Missions),
    ("Atlas", Screen::Memory),
    ("Beacon", Screen::Prompts),
    ("Runs", Screen::Runs),
    ("Commands", Screen::Commands),
    ("Help", Screen::Help),
];
pub const EVIDENCE: &[(&str, &str)] = &[
    ("run", "Overview"),
    ("candidate", "Candidate"),
    ("diff", "Diff"),
    ("tasks", "Tasks"),
    ("worktrees", "Worktrees"),
    ("attempts", "Attempts"),
    ("events", "Events"),
    ("publication", "Publication"),
];
#[derive(Default)]
pub struct WorkbenchState {
    pub screen: Screen,
    pub cursor: usize,
    pub column: usize,
    pub nav: bool,
    pub nav_cursor: usize,
    pub query: String,
    pub searching: bool,
    pub include_user: bool,
    pub memory_ids: BTreeSet<String>,
    pub prompt_selection: Option<String>,
    pub selected: Value,
    pub evidence: usize,
    pub error: Option<String>,
    pub refresh_after: bool,
    pub selected_id: Option<String>,
    pub initialized: bool,
    /// Which Orbit record the detail area shows first: `sbx` or the last Orbit mutation.
    pub orbit_detail: Option<String>,
}
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
fn array(value: Value) -> Vec<Value> {
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
fn identity(v: &Value) -> String {
    for k in ["id", "ticket_id", "name", "repository_id"] {
        let s = text(v, k);
        if !s.is_empty() {
            return s;
        }
    }
    String::new()
}
pub fn hints(screen: Screen) -> &'static str {
    match screen {
        Screen::Home => "Enter project  s set up a project  F3 Projects  F5 refresh",
        Screen::Projects => "Enter open  n register  s guided setup  F5 refresh",
        Screen::Orbit => "x exec  r recipe  s sync  b sandbox  F5 refresh",
        Screen::Project => "b baseline  d discover  c confirm  x components  y activate",
        Screen::Missions => "←→ columns  Enter ticket  n new  r refresh",
        Screen::Ticket => "e edit  p plan  g run  o source  c reconcile",
        Screen::Memory => "Enter inspect  n new  / search  u user memory",
        Screen::MemoryDetail => "e correct  d delete  p promote  s select context",
        Screen::Prompts => "Enter preview  n new  / search",
        Screen::PromptDetail => "e update  s select exact prompt",
        Screen::Runs => "Enter inspect  r refresh",
        Screen::RunDetail => "←→ evidence  f follow  c cancel  a actions  p plan",
        Screen::Plan => "a approve (type approve)  i implement  g run",
        Screen::Help => "c change theme  k keyboard reports  F10 menu  Esc back",
        Screen::Setup => "Guided setup",
        Screen::Commands => "Enter action  Tab context  / search",
    }
}
fn project(app: &AppState) -> String {
    app.context.get("project").cloned().unwrap_or_default()
}
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
fn read(app: &mut AppState, name: &str, args: Vec<String>, flags: Map<String, Value>) {
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
fn prefill(app: &mut AppState, name: &str, values: Vec<(&str, String)>) {
    app.begin(name);
    if let Some(form) = &mut app.form {
        for (key, value) in values {
            if let Some(field) = form.fields.iter_mut().find(|f| f.name == key) {
                field.value = value;
            }
        }
    }
    if matches!(name, "forget" | "promote") {
        if let Some(form) = &mut app.form {
            form.fields.push(FormField {
                name: "@confirm".into(),
                label: if name == "forget" {
                    "Confirm deletion (yes/no)"
                } else {
                    "Confirm wider memory scope (yes/no)"
                }
                .into(),
                hint: "Review the displayed record identity and exact hash. Type yes to continue."
                    .into(),
                value: String::new(),
                argument: false,
                boolean: false,
                repeated: false,
                choices: vec![("yes".into(), String::new()), ("no".into(), String::new())],
            });
        }
    }
}
fn joined(v: &Value, key: &str) -> String {
    v[key]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}
fn prompt_form(app: &mut AppState, update: bool) {
    let v = selected(app);
    prefill(
        app,
        "save-prompt",
        vec![
            (
                "name",
                if update {
                    text(&v, "name")
                } else {
                    String::new()
                },
            ),
            ("body", "-".into()),
            (
                "expected-version",
                if update {
                    text(&v, "version")
                } else {
                    String::new()
                },
            ),
        ],
    );
    if let Some(form) = &mut app.form {
        // A UI-only document field travels through the existing stdin_document channel.
        form.fields.push(FormField {
            name: "@document".into(),
            label: "Prompt body".into(),
            hint: "Alt+Enter newline · Ctrl+S submits the explicit versioned write".into(),
            value: if update {
                text(&v, "body")
            } else {
                String::new()
            },
            argument: false,
            boolean: false,
            repeated: false,
            choices: Vec::new(),
        });
        form.fields.retain(|f| f.name != "body");
        // Keep positional body argument in its original position but hide it only visually.
        let position = form.fields.iter().filter(|f| f.argument).count();
        form.fields.insert(
            position,
            FormField {
                name: "body".into(),
                label: "Body input (- uses form below)".into(),
                hint: "Keep - to use the inline prompt body".into(),
                value: "-".into(),
                argument: true,
                boolean: false,
                repeated: false,
                choices: Vec::new(),
            },
        );
    }
}
fn inspect(app: &mut AppState) {
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
pub fn key(app: &mut AppState, key: KeyEvent) -> bool {
    if app.mode == Mode::Form || app.prompt.is_some() {
        return false;
    }
    if app.workbench.searching {
        match key.code {
            KeyCode::Esc => app.workbench.searching = false,
            KeyCode::Enter => {
                app.workbench.searching = false;
                if app.mode != Mode::Catalog {
                    load(app);
                }
            }
            KeyCode::Backspace => {
                app.workbench.query.pop();
            }
            KeyCode::Char(c) if crate::keys::is_text(&key) => app.workbench.query.push(c),
            _ => {}
        }
        app.workbench.cursor = 0;
        app.selected = 0;
        return true;
    }
    if key.code == KeyCode::Esc {
        back(app);
        return true;
    }
    let shortcut = match key.code {
        KeyCode::F(1) => Some(Screen::Help),
        KeyCode::F(2) => Some(Screen::Commands),
        KeyCode::F(3) => Some(Screen::Projects),
        KeyCode::F(4) => Some(Screen::Missions),
        KeyCode::F(6) => Some(Screen::Memory),
        KeyCode::F(7) => Some(Screen::Prompts),
        KeyCode::F(8) => Some(Screen::Runs),
        _ => None,
    };
    if let Some(screen) = shortcut {
        open(app, screen);
        return true;
    }
    if key.code == KeyCode::F(10) {
        app.workbench.nav = !app.workbench.nav;
        app.mode = Mode::Workbench;
        return true;
    }
    if app.mode == Mode::Catalog && key.code == KeyCode::Char('/') {
        app.workbench.searching = true;
        return true;
    }
    if app.mode != Mode::Workbench {
        return false;
    }
    if key.code == KeyCode::Char('q') {
        app.quit = true;
        return true;
    }
    if app.workbench.nav {
        match key.code {
            KeyCode::Up => app.workbench.nav_cursor = app.workbench.nav_cursor.saturating_sub(1),
            KeyCode::Down => {
                app.workbench.nav_cursor = (app.workbench.nav_cursor + 1).min(NAV.len() - 1)
            }
            KeyCode::Enter => open(app, NAV[app.workbench.nav_cursor].1),
            KeyCode::Tab | KeyCode::BackTab => app.workbench.nav = false,
            _ => {}
        }
        return true;
    }
    let screen = app.workbench.screen;
    if screen == Screen::Help && matches!(key.code, KeyCode::Char('c' | 'k')) {
        let result = (|| {
            let path = crate::config::path()
                .ok_or_else(|| std::io::Error::other("Set HOME or LUUNA_TUI_CONFIG"))?;
            let text = match std::fs::read_to_string(&path) {
                Ok(text) => text,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => "{}".into(),
                Err(error) => return Err(error),
            };
            let (config, warnings) = crate::config::parse(&text);
            if !warnings.is_empty() {
                return Err(std::io::Error::other(warnings.join("; ")));
            }
            let (name, value) = if key.code == KeyCode::Char('k') {
                (
                    "keyboard_enhancement",
                    serde_json::json!(!config.keyboard_enhancement.unwrap_or(true)),
                )
            } else {
                (
                    "theme.preset",
                    serde_json::json!(if config.theme.preset.as_deref() == Some("terminal") {
                        "classic"
                    } else {
                        "terminal"
                    }),
                )
            };
            crate::config::set(&path, name, value)?;
            Ok::<_, std::io::Error>(())
        })();
        app.status = match result {
            Ok(()) => "Settings saved. Reopen the workbench to apply them.".into(),
            Err(error) => format!("Settings were not saved: {error}"),
        };
        return true;
    }
    if app.active_request.is_some() && key.code == KeyCode::Char('u') && screen == Screen::Memory {
        app.status =
            "Wait for the current memory read, or Esc to stop it before changing scope.".into();
        return true;
    }
    if matches!(
        screen,
        Screen::Ticket
            | Screen::MemoryDetail
            | Screen::PromptDetail
            | Screen::Plan
            | Screen::RunDetail
            | Screen::Project
    ) && matches!(
        key.code,
        KeyCode::Char('e' | 'd' | 'p' | 's' | 'a' | 'i' | 'c' | 'f' | 'b' | 'x' | 'y' | 'o' | 'g')
    ) && selected(app).is_null()
    {
        app.status =
            "Current record is not loaded. Refresh successfully before using its actions.".into();
        return true;
    }
    let count = rows(app).len();
    if screen == Screen::Orbit && orbit_key(app, key.code) {
        return true;
    }
    match key.code {
        KeyCode::Tab | KeyCode::BackTab => app.workbench.nav = true,
        KeyCode::Up => {
            app.workbench.cursor = app.workbench.cursor.saturating_sub(1);
            app.scroll.scroll_up(1);
        }
        KeyCode::Down => {
            app.workbench.cursor = (app.workbench.cursor + 1).min(count.saturating_sub(1));
            app.scroll.scroll_down(1);
        }
        KeyCode::PageDown => {
            app.workbench.cursor = (app.workbench.cursor + 8).min(count.saturating_sub(1));
            app.scroll.scroll_down(8);
        }
        KeyCode::PageUp => {
            app.workbench.cursor = app.workbench.cursor.saturating_sub(8);
            app.scroll.scroll_up(8);
        }
        KeyCode::Home => {
            app.workbench.cursor = 0;
            app.scroll.to_top();
        }
        KeyCode::End => {
            app.workbench.cursor = count.saturating_sub(1);
            app.scroll.to_bottom();
        }
        KeyCode::Left | KeyCode::Right if screen == Screen::Missions => {
            let n = snapshot(app, "board")["columns"]
                .as_array()
                .map(Vec::len)
                .unwrap_or(8)
                .max(1);
            app.workbench.column = if key.code == KeyCode::Right {
                (app.workbench.column + 1) % n
            } else {
                (app.workbench.column + n - 1) % n
            };
            app.workbench.cursor = 0;
        }
        KeyCode::Left | KeyCode::Right if screen == Screen::RunDetail => {
            app.workbench.evidence = if key.code == KeyCode::Right {
                (app.workbench.evidence + 1) % EVIDENCE.len()
            } else {
                (app.workbench.evidence + EVIDENCE.len() - 1) % EVIDENCE.len()
            };
            load(app);
        }
        KeyCode::Enter => inspect(app),
        KeyCode::Char('z') => {
            app.workbench.memory_ids.clear();
            app.workbench.prompt_selection = None;
            app.status = "Unsent plan context cleared. Recorded plans are unchanged.".into();
        }
        KeyCode::F(5) | KeyCode::Char('r') => load(app),
        KeyCode::Char('/') if matches!(screen, Screen::Memory | Screen::Prompts) => {
            app.workbench.searching = true
        }
        KeyCode::Char('u') if screen == Screen::Memory => {
            app.workbench.include_user = !app.workbench.include_user;
            if !app.workbench.include_user {
                let ids: Vec<String> = array(snapshot(app, "memory"))
                    .iter()
                    .filter(|v| text(v, "scope") == "user")
                    .map(identity)
                    .collect();
                for id in ids {
                    app.workbench.memory_ids.remove(&id);
                }
            }
            load(app);
        }
        KeyCode::Char('n') => match screen {
            Screen::Projects => app.begin("register"),
            Screen::Missions => app.begin("create-ticket"),
            Screen::Memory => prefill(app, "remember", vec![("scope", "project".into())]),
            Screen::Prompts => prompt_form(app, false),
            _ => {}
        },
        KeyCode::Char('e') => {
            let v = selected(app);
            match screen {
                Screen::Ticket => prefill(
                    app,
                    "edit-ticket",
                    vec![
                        ("revision", text(&v, "revision")),
                        ("title", text(&v, "title")),
                        ("intent", text(&v, "intent")),
                        ("criterion", joined(&v, "acceptance_criteria")),
                        ("repository", joined(&v, "repository_ids")),
                    ],
                ),
                Screen::MemoryDetail => prefill(
                    app,
                    "correct-memory",
                    vec![
                        ("memory-id", text(&v, "id")),
                        ("hash", text(&v, "hash")),
                        ("content", text(&v, "content")),
                    ],
                ),
                Screen::PromptDetail => prompt_form(app, true),
                _ => {}
            }
        }
        KeyCode::Char('s') if matches!(screen, Screen::Home | Screen::Projects) => {
            guide::begin(app, "init", Map::new())
        }
        KeyCode::Char('s') => {
            let v = selected(app);
            match screen {
                Screen::MemoryDetail => {
                    let id = text(&v, "id");
                    if !id.is_empty() && !app.workbench.memory_ids.remove(&id) {
                        app.workbench.memory_ids.insert(id);
                    }
                    app.status = "Plan context updated locally. Nothing was sent.".into();
                }
                Screen::PromptDetail => {
                    let hash = text(&v, "hash");
                    if !hash.is_empty() {
                        let chosen =
                            format!("{}@{}#{}", text(&v, "name"), text(&v, "version"), hash);
                        app.workbench.prompt_selection =
                            if app.workbench.prompt_selection.as_ref() == Some(&chosen) {
                                None
                            } else {
                                Some(chosen)
                            };
                        app.status =
                            "Exact prompt selection updated locally. Nothing was sent.".into();
                    }
                }
                _ => {}
            }
        }
        KeyCode::Char('p') => {
            let v = selected(app);
            match screen {
                Screen::Ticket => {
                    let memory = app
                        .workbench
                        .memory_ids
                        .iter()
                        .cloned()
                        .collect::<Vec<_>>()
                        .join("\n");
                    let prompt = app.workbench.prompt_selection.clone().unwrap_or_default();
                    prefill(app, "plan", vec![("memory", memory), ("prompt", prompt)]);
                }
                Screen::MemoryDetail if text(&v, "scope") == "user" => {
                    app.status =
                        "User memory is already at the widest scope; no promotion is available."
                            .into();
                }
                Screen::MemoryDetail => prefill(
                    app,
                    "promote",
                    vec![
                        ("memory-id", text(&v, "id")),
                        ("hash", text(&v, "hash")),
                        (
                            "to-scope",
                            if text(&v, "scope") == "process" {
                                "project"
                            } else {
                                "user"
                            }
                            .into(),
                        ),
                    ],
                ),
                Screen::RunDetail => open(app, Screen::Plan),
                _ => {}
            }
        }
        KeyCode::Char('d') => {
            let v = selected(app);
            match screen {
                Screen::MemoryDetail => prefill(
                    app,
                    "forget",
                    vec![("memory-id", text(&v, "id")), ("hash", text(&v, "hash"))],
                ),
                Screen::Project => app.begin("discover"),
                _ => {}
            }
        }
        KeyCode::Char('b') if screen == Screen::Project => app.begin("baseline"),
        KeyCode::Char('x') if screen == Screen::Project => app.begin("components"),
        KeyCode::Char('y') if screen == Screen::Project => app.begin("enable-components"),
        KeyCode::Char('o') if screen == Screen::Ticket => app.begin("ticket-source"),
        KeyCode::Char('c') => match screen {
            Screen::Project => app.begin("confirm"),
            Screen::Ticket => prefill(
                app,
                "reconcile-ticket",
                vec![("revision", text(&selected(app), "revision"))],
            ),
            Screen::RunDetail => app.begin("cancel"),
            _ => {}
        },
        KeyCode::Char('g') if matches!(screen, Screen::Ticket | Screen::Plan) => {
            app.workbench.evidence = 0;
            open(app, Screen::RunDetail);
        }
        KeyCode::Char('a') => match screen {
            Screen::Plan => app.begin("approve"),
            Screen::RunDetail => {
                open(app, Screen::Commands);
                app.filter = "run".into();
            }
            _ => {}
        },
        KeyCode::Char('i') if screen == Screen::Plan => app.begin("implement"),
        KeyCode::Char('f') if screen == Screen::RunDetail => app.begin("follow"),
        _ => {}
    }
    true
}

/// Orbit outcomes are the developer's own commands, never evidence (ADR 0013, ADR 0036):
/// `succeeded` is deliberately neutral, so only passed evidence ever reads as success.
pub fn orbit_tone(status: &str) -> &'static str {
    match status {
        "failed" | "blocked" => "danger",
        _ => "neutral",
    }
}

/// The recipe the Orbit run form starts with: the selected repository's first, else the project's.
fn orbit_recipe(app: &AppState) -> String {
    let first = |v: &Value| {
        v["recipes"]
            .as_array()
            .and_then(|r| r.iter().find_map(Value::as_str))
            .map(str::to_owned)
    };
    rows(app)
        .get(app.workbench.cursor)
        .and_then(first)
        .or_else(|| first(&snapshot(app, "orbit")))
        .unwrap_or_default()
}

fn orbit_key(app: &mut AppState, code: KeyCode) -> bool {
    if !matches!(code, KeyCode::Char('x' | 'r' | 's' | 'b')) {
        return false;
    }
    let p = project(app);
    if p.is_empty() {
        app.status = "Select a project or record first (F3 Projects).".into();
        return true;
    }
    match code {
        KeyCode::Char('x') => prefill(app, "orbit-exec", vec![("project", p)]),
        KeyCode::Char('r') => {
            let recipe = orbit_recipe(app);
            prefill(app, "orbit-run", vec![("project", p), ("recipe", recipe)]);
        }
        KeyCode::Char('s') => prefill(app, "orbit-sync", vec![("project", p)]),
        _ => {
            if app.active_request.is_some() {
                app.status = "Wait for this request, or Esc to stop waiting.".into();
                return true;
            }
            let mut flags = Map::new();
            flags.insert("plan-only".into(), json!(true));
            app.workbench.orbit_detail = Some("sbx".into());
            read(app, "sbx", vec![p], flags);
        }
    }
    true
}

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

/// Records and values already on screen that a form field can pick from (ADR 0042), as
/// `(value, label)`. Record values are exact identities, which the form shows by their short ids;
/// nothing here reads from the runtime.
fn known_choices(app: &AppState, command: &str, field: &str) -> Vec<(String, String)> {
    let pair = |value: String, label: String| (value, label);
    let fixed = |values: &[&str]| {
        values
            .iter()
            .map(|v| ((*v).to_string(), String::new()))
            .collect::<Vec<_>>()
    };
    let project_id = project(app);
    let repositories = || {
        let mut seen = Vec::new();
        let view = snapshot(app, "project");
        let mut sources = array(view["repositories"].clone());
        for home in array(snapshot(app, "home")["projects"].clone()) {
            if text(&home, "id") == project_id {
                sources.extend(array(home["repositories"].clone()));
            }
        }
        for orbit in array(snapshot(app, "orbit")["repositories"].clone()) {
            sources.push(orbit);
        }
        for repository in sources {
            let id = text(&repository, "repository_id");
            if !id.is_empty() && !seen.iter().any(|(v, _): &(String, String)| *v == id) {
                seen.push(pair(id, text(&repository, "local_path")));
            }
        }
        seen
    };
    let cards = || {
        let mut out = Vec::new();
        for column in array(snapshot(app, "board")["columns"].clone()) {
            for card in array(column["cards"].clone()) {
                if !text(&card, "ticket_id").is_empty() {
                    out.push(card);
                }
            }
        }
        out
    };
    match field {
        "project" => {
            let mut out: Vec<(String, String)> = array(snapshot(app, "projects"))
                .iter()
                .map(|p| p.get("project").unwrap_or(p))
                .map(|p| pair(text(p, "id"), text(p, "name")))
                .collect();
            for p in array(snapshot(app, "home")["projects"].clone()) {
                if !out.iter().any(|(v, _)| *v == text(&p, "id")) {
                    out.push(pair(text(&p, "id"), text(&p, "name")));
                }
            }
            out
        }
        "repository" | "repository-id" if command != "register" && command != "import" => {
            repositories()
        }
        // Record choices carry the exact identity, which the workbench context must hold; the
        // form shows each by its short id and label (ADR 0042).
        "ticket" => cards()
            .iter()
            .map(|c| pair(text(c, "ticket_id"), text(c, "title")))
            .collect(),
        "plan" => cards()
            .iter()
            .filter(|c| !text(c, "run_id").is_empty())
            .map(|c| {
                pair(
                    format!("plan-{}", text(c, "ticket_id")),
                    format!("plan of {}", text(c, "title")),
                )
            })
            .collect(),
        // `latest` is offered only beside the loaded runs it is resolved among, naming the run.
        "run" => {
            let mut out = Vec::new();
            if let Some(newest) = newest_loaded_run(app) {
                let short = crate::refs::short_id(&text(&newest, "id"));
                out.push(pair("latest".into(), format!("the newest run, {short}")));
            }
            out.extend(
                array(snapshot(app, "runs"))
                    .iter()
                    .map(|r| pair(text(r, "id"), render::run_title(r))),
            );
            out
        }
        "memory-id" => array(snapshot(app, "memory"))
            .iter()
            .map(|m| {
                let content: String = text(m, "content")
                    .lines()
                    .next()
                    .unwrap_or("")
                    .chars()
                    .take(40)
                    .collect();
                pair(text(m, "id"), format!("[{}] {content}", text(m, "scope")))
            })
            .collect(),
        "scope" => fixed(&["project", "process", "user"]),
        "to-scope" => fixed(&["project", "user"]),
        "profile" => fixed(&[
            "node",
            "node-typescript",
            "react",
            "nextjs",
            "python",
            "go",
            "rust",
            "flutter",
            "android",
            "ios",
        ]),
        "purpose" => fixed(&["feature", "testing_setup"]),
        "role" => fixed(&["implementer", "repository_lead", "project_orchestrator"]),
        "recipe" => {
            let mut out = Vec::new();
            let orbit = snapshot(app, "orbit");
            let mut lists = vec![orbit["recipes"].clone()];
            lists.extend(
                array(orbit["repositories"].clone())
                    .into_iter()
                    .map(|r| r["recipes"].clone()),
            );
            for list in lists {
                for recipe in array(list) {
                    let id = recipe
                        .as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| text(&recipe, "id"));
                    if !id.is_empty() && !out.iter().any(|(v, _): &(String, String)| *v == id) {
                        out.push(pair(id, String::new()));
                    }
                }
            }
            out
        }
        "task" if command == "implement" => array(snapshot(app, "tasks"))
            .iter()
            .map(|t| {
                pair(
                    text(t, "task_id"),
                    format!("{} · {}", text(t, "repository_id"), text(t, "state")),
                )
            })
            .filter(|(v, _)| !v.is_empty())
            .collect(),
        _ => Vec::new(),
    }
}

/// The exact identity a context value names among the records the workbench has loaded, so the
/// request, the context and the snapshot's provenance all name one record (ADR 0042). `None` for a
/// short id or `latest` that nothing loaded resolves: the command then resolves it against the
/// runtime (or refuses it), and the context waits for the record the runtime returns.
pub fn exact_context(app: &AppState, name: &str, value: &str) -> Option<String> {
    if !matches!(name, "ticket" | "plan" | "run") {
        return Some(value.to_string());
    }
    if name == "run" && is_latest(value) {
        return newest_loaded_run(app).map(|r| text(&r, "id"));
    }
    if !crate::refs::is_short_reference(value) {
        return Some(value.to_string());
    }
    // A prefix naming another kind (`ticket-8b1e` for a run) stays unresolved, so the command
    // refuses it rather than this matching a loaded record of the argument's kind.
    let allowed: &[&str] = match name {
        "ticket" => &["ticket"],
        "plan" => &["plan", "ticket"],
        _ => &["run"],
    };
    if !crate::refs::kind_matches(value, allowed) {
        return None;
    }
    let ids: Vec<String> = known_choices(app, "", name)
        .into_iter()
        .map(|(id, _)| id)
        .collect();
    crate::refs::unique_match(value, &ids)
}

fn is_latest(value: &str) -> bool {
    value.trim().eq_ignore_ascii_case("latest")
}

/// The newest loaded run, which a form's `latest` names, when the loaded list is a first page
/// (no `--before` or `--limit`); a paged list does not know the newest run.
fn newest_loaded_run(app: &AppState) -> Option<Value> {
    let flags = app
        .snapshot_requests
        .get("runs")
        .and_then(|r| r.fields.get("flags"))
        .cloned()
        .unwrap_or(Value::Null);
    if flags.get("before").is_some() || flags.get("limit").is_some() {
        return None;
    }
    array(snapshot(app, "runs"))
        .into_iter()
        .max_by_key(|r| text(r, "created_at"))
}

/// A read or run action sent with a short id or `latest` that nothing loaded resolved (ADR 0042):
/// the record the runtime returned is the one it named (run actions return the run they acted
/// on), so its exact identity becomes the context and the snapshot's provenance. The context was
/// cleared when the reference was sent.
fn adopt_identity(app: &mut AppState, command: &str, v: &Value) {
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

/// Gives each field of a new form the choices the workbench already knows, and fills a required
/// argument that has exactly one possible value (the only repository, the only project).
pub fn offer_choices(app: &AppState, form: &mut crate::app::ActionForm) {
    let command = form.command.name.clone();
    for field in &mut form.fields {
        if !field.choices.is_empty() || field.name.starts_with('@') {
            continue;
        }
        field.choices = known_choices(app, &command, &field.name)
            .into_iter()
            .filter(|(value, _)| !value.is_empty())
            .collect();
        let only_record = field.choices.len() == 1
            && !matches!(field.name.as_str(), "scope" | "to-scope" | "run");
        if field.argument && field.value.is_empty() && only_record {
            field.value = field.choices[0].0.clone();
        }
    }
}
