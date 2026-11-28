//! Presentation state over the shared command bridge; no domain writes or HTTP live here.
mod choices;
mod events;
pub mod guide;
mod keys;
mod navigation;
mod records;
pub mod render;
use crate::{
    app::{AppState, FormField, Mode},
    protocol::BridgeEvent,
};
pub use choices::*;
use crossterm::event::{KeyCode, KeyEvent};
pub use events::*;
pub use keys::*;
pub use navigation::*;
pub use records::*;
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
/// The sections in menu order: label, screen, and the keymap action that opens it ("" for none).
/// The number keys follow this order, so the menu shows the key that opens each row.
pub const NAV: &[(&str, Screen, &str)] = &[
    ("Home", Screen::Home, "home"),
    ("Projects", Screen::Projects, "projects"),
    ("Orbit", Screen::Orbit, "orbit"),
    ("Missions", Screen::Missions, "missions"),
    ("Atlas", Screen::Memory, "memory"),
    ("Beacon", Screen::Prompts, "prompts"),
    ("Runs", Screen::Runs, "runs"),
    ("Set up a project", Screen::Setup, ""),
    ("Commands", Screen::Commands, "commands"),
    ("Help", Screen::Help, "help"),
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
/// Each screen's keys, as (keymap action, what it does). The footer shows them with the key
/// the developer actually has bound, preferring one that works without Fn on a Mac.
pub fn hint_items(screen: Screen) -> &'static [(&'static str, &'static str)] {
    match screen {
        Screen::Home => &[
            ("open", "open"),
            ("select", "set up a project"),
            ("reload", "refresh"),
        ],
        Screen::Projects => &[
            ("open", "open"),
            ("new", "add project"),
            ("select", "guided setup"),
            ("reload", "refresh"),
        ],
        Screen::Orbit => &[
            ("components", "run command"),
            ("reload", "run recipe"),
            ("select", "sync"),
            ("baseline", "sandbox"),
        ],
        Screen::Project => &[
            ("baseline", "baseline"),
            ("discover", "discover"),
            ("confirm", "confirm"),
            ("components", "components"),
            ("enable", "enable"),
        ],
        Screen::Missions => &[
            ("right", "column"),
            ("open", "open"),
            ("plan", "plan"),
            ("go-run", "run"),
            ("new", "new ticket"),
        ],
        Screen::Ticket => &[
            ("edit", "edit"),
            ("plan", "plan"),
            ("go-run", "run"),
            ("source", "source"),
            ("confirm", "reconcile"),
        ],
        Screen::Memory => &[
            ("open", "open"),
            ("new", "new"),
            ("select", "starter set"),
            ("search", "search"),
            ("user-scope", "user memory"),
        ],
        Screen::MemoryDetail => &[
            ("select", "use in plan"),
            ("edit", "correct"),
            ("plan", "promote"),
            ("discover", "forget"),
        ],
        Screen::Prompts => &[("open", "preview"), ("new", "new"), ("search", "search")],
        Screen::PromptDetail => &[("select", "use in plan"), ("edit", "new version")],
        Screen::Runs => &[("open", "open"), ("reload", "refresh")],
        Screen::RunDetail => &[
            ("right", "evidence"),
            ("follow", "follow"),
            ("plan", "plan"),
            ("approve", "actions"),
            ("confirm", "cancel run"),
        ],
        Screen::Plan => &[
            ("approve", "approve"),
            ("implement", "implement"),
            ("go-run", "run"),
        ],
        Screen::Help => &[("theme", "theme"), ("keyboard", "enhanced keys")],
        Screen::Setup => &[],
        Screen::Commands => &[("open", "run"), ("next", "filter"), ("search", "search")],
    }
}

/// The footer line for a screen: its own keys, then the menu and Help.
pub fn hints(screen: Screen) -> String {
    let keymap = crate::keys::keymap();
    hint_items(screen)
        .iter()
        .chain([("menu", "menu"), ("help", "help")].iter())
        .map(|(action, label)| match *action {
            // Help's own settings keys are fixed letters, not keymap actions.
            "theme" => format!("t {label}"),
            "keyboard" => format!("e {label}"),
            "right" => format!("←→ {label}"),
            _ => format!("{} {label}", keymap.hint(action)),
        })
        .collect::<Vec<_>>()
        .join("  ")
}

/// Orbit outcomes are the developer's own commands, never evidence (ADR 0013, ADR 0036):
/// `succeeded` is deliberately neutral, so only passed evidence ever reads as success.
pub fn orbit_tone(status: &str) -> &'static str {
    match status {
        "failed" | "blocked" => "danger",
        _ => "neutral",
    }
}
