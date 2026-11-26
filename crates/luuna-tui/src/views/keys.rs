//! The workbench screens' keys and the forms they open.
use super::*;

pub(super) fn prefill(app: &mut AppState, name: &str, values: Vec<(&str, String)>) {
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
pub(super) fn joined(v: &Value, key: &str) -> String {
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
pub(super) fn prompt_form(app: &mut AppState, update: bool) {
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
        KeyCode::F(9) => Some(Screen::Home),
        KeyCode::F(11) => Some(Screen::Orbit),
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
    if screen == Screen::Help && matches!(key.code, KeyCode::Char('t' | 'e')) {
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
            let (name, value) = if key.code == KeyCode::Char('e') {
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
                        ("type", text(&v, "type")),
                        (
                            "priority",
                            match text(&v, "priority") {
                                p if p.is_empty() => String::new(),
                                p => format!("P{p}"),
                            },
                        ),
                        ("label", joined(&v, "labels").replace('\n', ",")),
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
        // ADR 0059: review a starter set, keeping or skipping each proposed fact.
        KeyCode::Char('s') if screen == Screen::Memory => app.begin("memory-starter"),
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
        KeyCode::Char('p') if screen == Screen::Missions => {
            if board_card_context(app) {
                let context = plan_context(app);
                prefill(app, "plan", context);
            }
        }
        KeyCode::Char('g') if screen == Screen::Missions => {
            if board_card_context(app) {
                if app.context.contains_key("run") {
                    app.workbench.evidence = 0;
                    open(app, Screen::RunDetail);
                } else {
                    app.status = "This ticket has no run yet; p plans it.".into();
                }
            }
        }
        KeyCode::Char('p') => {
            let v = selected(app);
            match screen {
                Screen::Ticket => {
                    let context = plan_context(app);
                    prefill(app, "plan", context);
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
/// Scopes the workbench to the selected board card's ticket (and run), as Enter does,
/// without leaving the board. False for an empty board or a prerequisite card.
/// The plan form's memory and prompt flags from the context selected locally,
/// so a plan started from the ticket screen or a board card carries it alike.
pub(super) fn plan_context(app: &AppState) -> Vec<(&'static str, String)> {
    let memory = app
        .workbench
        .memory_ids
        .iter()
        .cloned()
        .collect::<Vec<_>>()
        .join("\n");
    let prompt = app.workbench.prompt_selection.clone().unwrap_or_default();
    vec![("memory", memory), ("prompt", prompt)]
}
pub(super) fn board_card_context(app: &mut AppState) -> bool {
    let v = selected(app);
    let ticket = text(&v, "ticket_id");
    if ticket.is_empty() {
        app.status = "Select a ticket card first.".into();
        return false;
    }
    let project = text(&v, "project_id");
    if !project.is_empty() {
        choose_project(app, &project);
    }
    app.context.remove("plan");
    app.context.insert("ticket".into(), ticket);
    match text(&v, "run_id") {
        run if run.is_empty() => app.context.remove("run"),
        run => app.context.insert("run".into(), run),
    };
    true
}
/// The recipe the Orbit run form starts with: the selected repository's first, else the project's.
pub(super) fn orbit_recipe(app: &AppState) -> String {
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
pub(super) fn orbit_key(app: &mut AppState, code: KeyCode) -> bool {
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
