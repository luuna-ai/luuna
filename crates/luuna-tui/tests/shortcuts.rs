//! Every default keyboard shortcut, pressed on the screen where it means something, against the
//! catalog the workbench really receives (`commands.json`), so a renamed command or a key that
//! never reaches its handler fails here instead of silently doing nothing.
use crossterm::event::KeyEvent;
use luuna_tui::{
    app::{AppState, Mode},
    commands,
    keys::{parse, ACTIONS},
    protocol::CommandDef,
    views::{self, Screen},
};
use serde_json::{json, Value};

/// The catalog exactly as the in-process service sends it (see `bridge.rs`).
fn catalog() -> Vec<CommandDef> {
    let table = commands::table();
    table
        .commands
        .iter()
        .chain(table.projections.iter())
        .map(|command| {
            let mut entry = command.raw.to_serde();
            if let Value::Object(map) = &mut entry {
                map.remove("example");
                map.remove("failed");
                map.insert("read_only".into(), json!(commands::read_only(command)));
            }
            serde_json::from_value(entry).expect("catalog entry")
        })
        .collect()
}

fn hash() -> String {
    format!("sha256:{}", "a".repeat(64))
}

fn app(screen: Screen) -> AppState {
    let mut app = AppState::default();
    app.commands = catalog();
    app.mode = if screen == Screen::Commands {
        Mode::Catalog
    } else {
        Mode::Workbench
    };
    app.connection = "connected".into();
    for (name, value) in [
        ("project", "demo"),
        ("ticket", "ticket-1"),
        ("run", "run-1"),
        ("plan", "plan-1"),
    ] {
        app.context.insert(name.into(), value.into());
    }
    let ticket = json!({"id":"ticket-1","project_id":"demo","title":"Greeting","intent":"Greet","revision":3,"acceptance_criteria":["Print hello"],"repository_ids":["api"],"type":"task","priority":2,"labels":[]});
    let run = json!({"id":"run-1","project_id":"demo","state":"plan_ready","metadata":{"kind":"workflow","plan_id":"plan-1","ticket_id":"ticket-1"}});
    let memory = json!({"id":"mem-1","scope":"project","content":"Use structured errors","provenance":"notes","hash":hash()});
    let prompt = json!({"name":"review","version":2,"hash":hash(),"body":"Review it."});
    let plan = json!({"id":"plan-1","ticket_id":"ticket-1","revision":1,"hash":hash(),"tasks":[]});
    let project = json!({"project":{"id":"demo","name":"Demo","revision":2},"repositories":[{"repository_id":"api","local_path":"/fixture/api"}],"manifest":{"hash":hash(),"revision":2}});
    let snapshots = [
        (
            "projects",
            json!([{"id":"demo","name":"Demo","revision":2}]),
        ),
        (
            "home",
            json!({"projects":[{"id":"demo","name":"Demo","revision":2,"repositories":[]}],"blockers":[]}),
        ),
        ("project", project.clone()),
        (
            "board",
            json!({"columns":[{"column":{"id":"0","title":"Backlog"},"cards":[{"title":"Greeting","ticket_id":"ticket-1","project_id":"demo","kind":"ticket","run_id":"run-1","state":"plan_ready"}]},{"column":{"id":"1","title":"Plan"},"cards":[]}]}),
        ),
        ("show-ticket", ticket.clone()),
        ("memory", json!([memory.clone()])),
        ("prompts", json!([{"name":"review","version":2}])),
        ("render-prompt", prompt.clone()),
        ("runs", json!([run.clone()])),
        ("run", run.clone()),
        ("show-plan", plan.clone()),
        (
            "orbit",
            json!({"repositories":[{"repository_id":"api","status":"present"}]}),
        ),
    ];
    for (name, value) in snapshots {
        app.snapshots.insert(name.into(), value);
    }
    app.workbench.screen = screen;
    app.workbench.selected = match screen {
        Screen::Ticket => ticket,
        Screen::MemoryDetail => memory,
        Screen::PromptDetail => prompt,
        Screen::RunDetail => run,
        Screen::Plan => plan,
        Screen::Project => project,
        _ => Value::Null,
    };
    app
}

fn chord(spec: &str) -> KeyEvent {
    let c = parse(spec).unwrap();
    KeyEvent::new(c.code, c.modifiers)
}

fn press(app: &mut AppState, spec: &str) {
    app.key(chord(spec));
}

fn form(app: &AppState) -> Option<&str> {
    app.form
        .as_ref()
        .filter(|_| app.mode == Mode::Form)
        .map(|f| f.command.name.as_str())
}

fn reads(app: &AppState) -> Vec<String> {
    app.queue
        .iter()
        .filter(|q| q.op == "read")
        .filter_map(|q| q.fields.get("name").and_then(Value::as_str))
        .map(str::to_owned)
        .collect()
}

const TOP: [Screen; 8] = [
    Screen::Home,
    Screen::Projects,
    Screen::Orbit,
    Screen::Missions,
    Screen::Memory,
    Screen::Prompts,
    Screen::Runs,
    Screen::Help,
];

#[test]
fn section_shortcuts_open_their_screen_from_everywhere() {
    for from in TOP
        .into_iter()
        .chain([Screen::Ticket, Screen::Plan, Screen::Commands])
    {
        for (keys, to) in [
            (["f1", "?"], Screen::Help),
            (["f2", ":"], Screen::Commands),
            (["1", "1"], Screen::Home),
            (["f3", "2"], Screen::Projects),
            (["3", "3"], Screen::Orbit),
            (["f4", "4"], Screen::Missions),
            (["f6", "5"], Screen::Memory),
            (["f7", "6"], Screen::Prompts),
            (["f8", "7"], Screen::Runs),
        ] {
            for spec in keys {
                let mut a = app(from);
                press(&mut a, spec);
                assert_eq!(a.workbench.screen, to, "{spec} from {from:?}");
            }
        }
    }
}

#[test]
fn the_menu_and_footer_show_keys_that_open_what_they_say() {
    let keymap = luuna_tui::keys::keymap();
    for (label, screen, action) in views::NAV {
        if action.is_empty() {
            continue;
        }
        let shown = keymap.hint(action);
        assert!(
            !shown.starts_with('F'),
            "{label} shows {shown}, which needs Fn on a Mac"
        );
        let mut a = app(Screen::Help);
        press(&mut a, &shown.to_lowercase());
        assert_eq!(a.workbench.screen, *screen, "{label} key {shown}");
    }
    // Every key a footer names is bound, and is the one shown.
    for screen in TOP.into_iter().chain([
        Screen::Project,
        Screen::Ticket,
        Screen::MemoryDetail,
        Screen::PromptDetail,
        Screen::RunDetail,
        Screen::Plan,
        Screen::Commands,
    ]) {
        let footer = views::hints(screen);
        assert!(!footer.contains("unbound"), "{screen:?}: {footer}");
        assert!(!footer.contains("F1"), "{screen:?}: {footer}");
    }
}

#[test]
fn help_settings_keys_save_the_theme_and_keyboard_setting() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tui.json");
    std::env::set_var("LUUNA_TUI_CONFIG", &path);
    let mut a = app(Screen::Help);
    press(&mut a, "t");
    let saved: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(saved["theme"]["preset"], "terminal", "{}", a.status);
    press(&mut a, "e");
    let saved: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(saved["keyboard_enhancement"], false, "{}", a.status);
    assert!(!a.quit);
    std::env::remove_var("LUUNA_TUI_CONFIG");
}

#[test]
fn the_menu_opens_moves_opens_a_section_and_closes() {
    for spec in ["m", "f10"] {
        let mut a = app(Screen::Home);
        press(&mut a, spec);
        assert!(a.workbench.nav, "{spec} opens the menu");
        press(&mut a, spec);
        assert!(!a.workbench.nav, "{spec} closes the menu");
    }
    let mut a = app(Screen::Home);
    press(&mut a, "m");
    press(&mut a, "esc");
    assert!(!a.workbench.nav, "Esc closes the menu");
    press(&mut a, "m");
    let target = views::NAV
        .iter()
        .position(|(_, s, _)| *s == Screen::Runs)
        .unwrap();
    for _ in 0..target {
        press(&mut a, "j");
    }
    press(&mut a, "enter");
    assert_eq!(a.workbench.screen, Screen::Runs);
    assert!(!a.workbench.nav);
}

#[test]
fn quit_and_interrupt() {
    for spec in ["q", "ctrl+c"] {
        for screen in TOP {
            let mut a = app(screen);
            press(&mut a, spec);
            assert!(a.quit, "{spec} on {screen:?}");
        }
    }
}

#[test]
fn modified_letters_never_fire_single_letter_actions() {
    // Ctrl+S submits forms; on a screen it must not also start setup or change a selection.
    for spec in ["ctrl+s", "ctrl+enter"] {
        let mut a = app(Screen::Home);
        press(&mut a, spec);
        assert_eq!(a.mode, Mode::Workbench, "{spec} on Home started setup");
        let mut a = app(Screen::MemoryDetail);
        press(&mut a, spec);
        assert!(a.workbench.memory_ids.is_empty(), "{spec} selected memory");
    }
    // Ctrl+U clears a field; on Atlas it must not flip user memory.
    let mut a = app(Screen::Memory);
    press(&mut a, "ctrl+u");
    assert!(!a.workbench.include_user);
    // Unbound Alt+letter chords are not the plain letter.
    let mut a = app(Screen::Ticket);
    press(&mut a, "alt+e");
    assert_eq!(form(&a), None);
    let mut a = app(Screen::Home);
    press(&mut a, "alt+q");
    assert!(!a.quit);
}

#[test]
fn moving_and_paging_select_rows() {
    let mut a = app(Screen::Runs);
    a.snapshots.insert(
        "runs".into(),
        Value::Array(
            (0..20)
                .map(|i| json!({"id": format!("run-{i}"), "state": "completed"}))
                .collect(),
        ),
    );
    for (spec, cursor) in [
        ("down", 1),
        ("j", 2),
        ("ctrl+n", 3),
        ("up", 2),
        ("k", 1),
        ("ctrl+p", 0),
        ("pagedown", 8),
        ("ctrl+f", 16),
        ("alt+up", 8),
        ("pageup", 0),
        ("end", 19),
        ("home", 0),
        (">", 19),
        ("<", 0),
    ] {
        press(&mut a, spec);
        assert_eq!(a.workbench.cursor, cursor, "{spec}");
    }
}

#[test]
fn board_and_evidence_tabs_move_with_left_and_right() {
    let mut a = app(Screen::Missions);
    for (spec, column) in [("right", 1), ("l", 0), ("left", 1), ("h", 0)] {
        press(&mut a, spec);
        assert_eq!(a.workbench.column, column, "{spec}");
    }
    let mut a = app(Screen::RunDetail);
    press(&mut a, "right");
    assert_eq!(a.workbench.evidence, 1);
    a.active_request = None;
    press(&mut a, "h");
    assert_eq!(a.workbench.evidence, 0);
}

#[test]
fn every_screen_action_opens_its_command() {
    for (screen, spec, command) in [
        (Screen::Projects, "n", "register"),
        (Screen::Project, "b", "baseline"),
        (Screen::Project, "d", "discover"),
        (Screen::Project, "c", "confirm"),
        (Screen::Project, "x", "components"),
        (Screen::Project, "y", "enable-components"),
        (Screen::Orbit, "x", "orbit-exec"),
        (Screen::Orbit, "r", "orbit-run"),
        (Screen::Orbit, "s", "orbit-sync"),
        (Screen::Missions, "n", "create-ticket"),
        (Screen::Missions, "p", "plan"),
        (Screen::Ticket, "e", "edit-ticket"),
        (Screen::Ticket, "p", "plan"),
        (Screen::Ticket, "o", "ticket-source"),
        (Screen::Ticket, "c", "reconcile-ticket"),
        (Screen::Memory, "n", "remember"),
        (Screen::Memory, "s", "memory-starter"),
        (Screen::MemoryDetail, "e", "correct-memory"),
        (Screen::MemoryDetail, "p", "promote"),
        (Screen::MemoryDetail, "d", "forget"),
        (Screen::Prompts, "n", "save-prompt"),
        (Screen::PromptDetail, "e", "save-prompt"),
        (Screen::RunDetail, "f", "follow"),
        (Screen::RunDetail, "c", "cancel"),
        (Screen::Plan, "a", "approve"),
        (Screen::Plan, "i", "implement"),
    ] {
        let mut a = app(screen);
        press(&mut a, spec);
        assert_eq!(
            form(&a),
            Some(command),
            "{spec} on {screen:?} (status: {:?})",
            a.status
        );
    }
}

#[test]
fn every_screen_navigation_key_goes_where_it_says() {
    for (screen, spec, to, read) in [
        (Screen::Home, "enter", Screen::Project, "project"),
        (Screen::Projects, "enter", Screen::Project, "project"),
        (Screen::Missions, "enter", Screen::Ticket, "show-ticket"),
        (Screen::Missions, "g", Screen::RunDetail, "run"),
        (Screen::Ticket, "g", Screen::RunDetail, "run"),
        (Screen::Plan, "g", Screen::RunDetail, "run"),
        (Screen::Memory, "enter", Screen::MemoryDetail, ""),
        (
            Screen::Prompts,
            "enter",
            Screen::PromptDetail,
            "render-prompt",
        ),
        (Screen::Runs, "enter", Screen::RunDetail, "run"),
        (Screen::RunDetail, "p", Screen::Plan, "show-plan"),
        (Screen::Project, "esc", Screen::Projects, "projects"),
        (Screen::Ticket, "esc", Screen::Missions, "board"),
        (Screen::Plan, "esc", Screen::Ticket, "show-ticket"),
        (Screen::RunDetail, "esc", Screen::Runs, "runs"),
        (Screen::MemoryDetail, "esc", Screen::Memory, "memory"),
        (Screen::PromptDetail, "esc", Screen::Prompts, "prompts"),
    ] {
        let mut a = app(screen);
        press(&mut a, spec);
        assert_eq!(a.workbench.screen, to, "{spec} on {screen:?}");
        if !read.is_empty() {
            assert!(
                reads(&a).iter().any(|r| r == read),
                "{spec} on {screen:?} reads {read}: {:?}",
                reads(&a)
            );
        }
    }
    let mut a = app(Screen::RunDetail);
    press(&mut a, "a");
    assert_eq!(a.mode, Mode::Catalog);
    assert_eq!(a.filter, "run");
}

#[test]
fn refresh_keys_reread_the_screen() {
    for spec in ["f5", "ctrl+r", "r"] {
        for (screen, read) in [
            (Screen::Home, "home"),
            (Screen::Missions, "board"),
            (Screen::Runs, "runs"),
            (Screen::Memory, "memory"),
        ] {
            let mut a = app(screen);
            press(&mut a, spec);
            assert!(
                reads(&a).iter().any(|r| r == read),
                "{spec} on {screen:?}: {:?}",
                reads(&a)
            );
        }
    }
}

#[test]
fn local_selection_keys() {
    let mut a = app(Screen::MemoryDetail);
    press(&mut a, "s");
    assert!(a.workbench.memory_ids.contains("mem-1"));
    press(&mut a, "s");
    assert!(a.workbench.memory_ids.is_empty());
    press(&mut a, "s");
    press(&mut a, "z");
    assert!(a.workbench.memory_ids.is_empty(), "z clears the context");

    let mut a = app(Screen::PromptDetail);
    press(&mut a, "s");
    assert!(a.workbench.prompt_selection.is_some());

    let mut a = app(Screen::Memory);
    press(&mut a, "u");
    assert!(a.workbench.include_user);
}

#[test]
fn search_takes_typed_text_until_enter_or_esc() {
    for screen in [Screen::Memory, Screen::Prompts, Screen::Commands] {
        let mut a = app(screen);
        press(&mut a, "/");
        assert!(a.workbench.searching, "/ on {screen:?}");
        for spec in ["q", "1", "m", "?"] {
            press(&mut a, spec);
        }
        assert_eq!(a.workbench.query, "q1m?");
        assert!(!a.quit);
        press(&mut a, "backspace");
        assert_eq!(a.workbench.query, "q1m");
        press(&mut a, "esc");
        assert!(!a.workbench.searching);
    }
}

#[test]
fn form_keys_move_pick_edit_and_submit() {
    let mut a = app(Screen::Memory);
    press(&mut a, "n");
    assert_eq!(form(&a), Some("remember"));
    let fields = |a: &AppState| a.form.as_ref().unwrap().fields.len();
    let cursor = |a: &AppState| a.form.as_ref().unwrap().cursor;
    let count = fields(&a);
    press(&mut a, "tab");
    assert_eq!(cursor(&a), 1);
    press(&mut a, "down");
    assert_eq!(cursor(&a), 2);
    press(&mut a, "up");
    assert_eq!(cursor(&a), 1);
    press(&mut a, "backtab");
    assert_eq!(cursor(&a), 0);
    press(&mut a, "backtab");
    assert_eq!(cursor(&a), count - 1, "Shift+Tab wraps");
    press(&mut a, "ctrl+n");
    assert_eq!(cursor(&a), 0, "Ctrl+N moves like Down");

    // A field with choices cycles with Left/Right; letters stay text everywhere.
    let scope = a
        .form
        .as_ref()
        .unwrap()
        .fields
        .iter()
        .position(|f| f.name == "scope")
        .unwrap();
    a.form.as_mut().unwrap().cursor = scope;
    a.form.as_mut().unwrap().fields[scope].value.clear();
    press(&mut a, "right");
    let picked = a.form.as_ref().unwrap().fields[scope].value.clone();
    assert!(!picked.is_empty(), "Right picks a scope");
    press(&mut a, "right");
    assert_ne!(a.form.as_ref().unwrap().fields[scope].value, picked);
    press(&mut a, "left");
    assert_eq!(a.form.as_ref().unwrap().fields[scope].value, picked);

    let content = a
        .form
        .as_ref()
        .unwrap()
        .fields
        .iter()
        .position(|f| f.name == "content")
        .unwrap();
    a.form.as_mut().unwrap().cursor = content;
    for spec in [
        "h",
        "l",
        "q",
        "alt+enter",
        "j",
        "shift+enter",
        "k",
        "ctrl+j",
        "x",
    ] {
        press(&mut a, spec);
    }
    assert_eq!(
        a.form.as_ref().unwrap().fields[content].value,
        "hlq\nj\nk\nx"
    );
    press(&mut a, "backspace");
    press(&mut a, "ctrl+h");
    assert_eq!(a.form.as_ref().unwrap().fields[content].value, "hlq\nj\nk");
    press(&mut a, "ctrl+u");
    assert_eq!(a.form.as_ref().unwrap().fields[content].value, "");
    assert!(!a.quit);

    // Ctrl+S submits (here it is refused: required fields are empty, and the form stays).
    press(&mut a, "ctrl+s");
    assert_eq!(a.mode, Mode::Form);
    assert!(!a.status.is_empty(), "a refused submit says why");
    press(&mut a, "esc");
    assert_eq!(a.mode, Mode::Workbench);
    assert!(a.form.is_none());
}

#[test]
fn every_action_in_the_keymap_is_exercised_here() {
    // Keep this file honest: a new action needs a test above.
    let covered = [
        "interrupt",
        "quit",
        "back",
        "help",
        "home",
        "orbit",
        "commands",
        "projects",
        "missions",
        "refresh",
        "memory",
        "prompts",
        "runs",
        "menu",
        "up",
        "down",
        "left",
        "right",
        "page-up",
        "page-down",
        "top",
        "bottom",
        "next",
        "previous",
        "open",
        "newline",
        "submit",
        "clear",
        "delete",
        "search",
        "reload",
        "new",
        "edit",
        "select",
        "plan",
        "discover",
        "baseline",
        "components",
        "enable",
        "source",
        "confirm",
        "go-run",
        "approve",
        "implement",
        "follow",
        "user-scope",
        "clear-context",
    ];
    for (name, ..) in ACTIONS {
        assert!(covered.contains(name), "{name} has no shortcut test");
    }
}

#[test]
fn a_yes_no_runtime_question_is_answered_with_y_or_n() {
    use luuna_tui::app::{PendingPrompt, PromptKind};
    for (key, answer) in [("y", true), ("n", false)] {
        let mut app = app(Screen::Home);
        app.prompt = Some(PendingPrompt {
            request_id: "r".into(),
            prompt_id: "p".into(),
            kind: PromptKind::Confirm,
            question: "Continue?".into(),
            value: String::new(),
        });
        let queued = app.queue.len();
        press(&mut app, "enter");
        press(&mut app, "x");
        assert!(
            app.prompt.is_some(),
            "Enter or another letter does not answer"
        );
        assert_eq!(app.queue.len(), queued);
        press(&mut app, key);
        assert!(app.prompt.is_none(), "{key} answers at once");
        let reply = app.queue.back().unwrap();
        assert_eq!(reply.op, "reply");
        assert_eq!(reply.fields["value"], answer);
    }
}
