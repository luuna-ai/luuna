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
pub fn hints(screen: Screen) -> &'static str {
    match screen {
        Screen::Home => "Enter project  s set up a project  F3 Projects  F5 refresh",
        Screen::Projects => "Enter open  n register  s guided setup  F5 refresh",
        Screen::Orbit => "x exec  r recipe  s sync  b sandbox  F5 refresh",
        Screen::Project => "b baseline  d discover  c confirm  x components  y activate",
        Screen::Missions => "←→ columns  Enter ticket  p plan  g follow run  n new  r refresh",
        Screen::Ticket => "e edit  p plan  g run  o source  c reconcile",
        Screen::Memory => "Enter inspect  n new  s starter set  / search  u user memory",
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

/// Orbit outcomes are the developer's own commands, never evidence (ADR 0013, ADR 0036):
/// `succeeded` is deliberately neutral, so only passed evidence ever reads as success.
pub fn orbit_tone(status: &str) -> &'static str {
    match status {
        "failed" | "blocked" => "danger",
        _ => "neutral",
    }
}
