//! Forms read like forms: plain labels in catalog order, rare options last, and fixed choices
//! picked with arrows or a letter instead of typed from memory.
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use luuna_tui::{
    app::{AppState, Mode},
    commands,
    protocol::CommandDef,
};
use serde_json::{json, Value};

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
            }
            serde_json::from_value(entry).unwrap()
        })
        .collect()
}

fn open(name: &str) -> AppState {
    let mut app = AppState::default();
    app.commands = catalog();
    app.mode = Mode::Workbench;
    app.context.insert("project".into(), "demo".into());
    app.snapshots
        .insert("projects".into(), json!([{"id":"demo","name":"Demo"}]));
    app.begin(name);
    assert_eq!(app.mode, Mode::Form, "{name} opens a form");
    app
}

fn names(app: &AppState) -> Vec<String> {
    app.form
        .as_ref()
        .unwrap()
        .fields
        .iter()
        .map(|f| f.name.clone())
        .collect()
}

fn focus(app: &mut AppState, name: &str) {
    let form = app.form.as_mut().unwrap();
    form.cursor = form.fields.iter().position(|f| f.name == name).unwrap();
}

fn value(app: &AppState, name: &str) -> String {
    let form = app.form.as_ref().unwrap();
    form.fields
        .iter()
        .find(|f| f.name == name)
        .unwrap()
        .value
        .clone()
}

fn press(app: &mut AppState, code: KeyCode) {
    app.key(KeyEvent::new(code, KeyModifiers::NONE));
}

#[test]
fn fields_follow_the_catalog_order_with_rare_options_last() {
    let app = open("create-ticket");
    let order = names(&app);
    assert_eq!(&order[..4], ["project", "title", "intent", "criterion"]);
    assert_eq!(order.last().unwrap(), "key");
    let form = app.form.as_ref().unwrap();
    let first_advanced = form.fields.iter().position(|f| f.advanced).unwrap();
    assert!(form.fields[first_advanced..].iter().all(|f| f.advanced));
}

#[test]
fn every_form_has_plain_labels() {
    for command in catalog() {
        let mut app = AppState::default();
        app.commands = catalog();
        app.mode = Mode::Workbench;
        app.begin(&command.name);
        let Some(form) = app.form.as_ref() else {
            continue;
        };
        for field in &form.fields {
            assert!(
                !field.label.starts_with("--") && !field.label.is_empty(),
                "{}: {:?}",
                command.name,
                field.label
            );
            assert!(
                !field.label.contains("(yes/no)"),
                "{}: {:?} repeats its choices",
                command.name,
                field.label
            );
        }
    }
}

#[test]
fn ticket_type_and_priority_are_picked_not_typed() {
    let mut app = open("create-ticket");
    focus(&mut app, "type");
    press(&mut app, KeyCode::Right);
    assert_eq!(value(&app, "type"), "task");
    press(&mut app, KeyCode::Char('b'));
    assert_eq!(value(&app, "type"), "bug", "a letter jumps to its choice");
    press(&mut app, KeyCode::Char('z'));
    assert_eq!(
        value(&app, "type"),
        "bug",
        "a letter no choice starts with is ignored"
    );
    press(&mut app, KeyCode::Backspace);
    assert_eq!(
        value(&app, "type"),
        "",
        "Backspace clears back to the default"
    );

    focus(&mut app, "priority");
    press(&mut app, KeyCode::Char('p'));
    assert_eq!(value(&app, "priority"), "P0");
    press(&mut app, KeyCode::Char('p'));
    assert_eq!(
        value(&app, "priority"),
        "P1",
        "the same letter steps through"
    );
    press(&mut app, KeyCode::Char('b'));
    assert_eq!(value(&app, "priority"), "P4", "labels match too: backlog");

    focus(&mut app, "title");
    for c in "bug".chars() {
        press(&mut app, KeyCode::Char(c));
    }
    assert_eq!(value(&app, "title"), "bug", "text fields still take text");
    let (_, flags) = app.form.as_ref().unwrap().input().unwrap();
    assert_eq!(flags["priority"], "P4");
    assert!(flags.get("type").is_none(), "a cleared choice is not sent");
}

#[test]
fn typing_a_whole_choice_picks_it() {
    let mut app = open("remember");
    focus(&mut app, "scope");
    for c in "process".chars() {
        press(&mut app, KeyCode::Char(c));
    }
    assert_eq!(value(&app, "scope"), "process");
    let mut app = open("create-ticket");
    focus(&mut app, "priority");
    for c in "P3".chars() {
        press(&mut app, KeyCode::Char(c));
    }
    assert_eq!(value(&app, "priority"), "P3");
}

#[test]
fn scopes_and_yes_no_are_picked_with_a_letter() {
    let mut app = open("remember");
    focus(&mut app, "scope");
    press(&mut app, KeyCode::Char('u'));
    assert_eq!(value(&app, "scope"), "user");

    let mut app = open("rebind");
    focus(&mut app, "confirm");
    press(&mut app, KeyCode::Char('y'));
    assert_eq!(value(&app, "confirm"), "yes");
    press(&mut app, KeyCode::Char('n'));
    assert_eq!(value(&app, "confirm"), "no");
}

#[test]
fn deleting_memory_asks_with_a_yes_no_choice() {
    let mut app = AppState::default();
    app.commands = catalog();
    app.mode = Mode::Workbench;
    app.context.insert("project".into(), "demo".into());
    let memory = json!({"id":"mem-1","scope":"project","content":"x","hash":format!("sha256:{}", "a".repeat(64))});
    app.snapshots
        .insert("memory".into(), json!([memory.clone()]));
    app.workbench.screen = luuna_tui::views::Screen::MemoryDetail;
    app.workbench.selected = memory;
    press(&mut app, KeyCode::Char('d'));
    assert_eq!(app.form.as_ref().unwrap().command.name, "forget");
    focus(&mut app, "@confirm");
    assert!(app.form.as_ref().unwrap().input().is_err());
    press(&mut app, KeyCode::Char('y'));
    assert_eq!(value(&app, "@confirm"), "yes");
    assert!(app.form.as_ref().unwrap().input().is_ok());
}
