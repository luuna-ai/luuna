use crate::protocol::{
    bounded_append, safe_text, BridgeEvent, ClientCommand, CommandDef, MAX_EVENTS,
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, VecDeque};

use crate::form::{edit, MAX_INPUT};
pub use crate::form::{ActionForm, FormField};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Workbench,
    Catalog,
    Form,
    Result,
    /// The full-screen setup guide (`lu prime`), see crate::guide.
    Guide,
}

/// What a runtime question asks for, which decides how it is answered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PromptKind {
    /// Exact plan approval: the plan scrolls above and Enter needs the plan fully visible.
    Hash,
    /// A multi-line document: Enter adds a line, Ctrl+S (or Ctrl+Enter) submits.
    Document,
    /// Yes or no, answered as a boolean.
    Confirm,
    /// Any other single-line text answer.
    Text,
}

impl PromptKind {
    /// The kind a bridge `prompt` event names; unknown kinds are single-line text.
    pub fn from_wire(kind: &str) -> Self {
        match kind {
            "hash" => Self::Hash,
            "document" => Self::Document,
            "confirm" => Self::Confirm,
            _ => Self::Text,
        }
    }
}

#[derive(Clone, Debug)]
pub struct PendingPrompt {
    pub request_id: String,
    pub prompt_id: String,
    pub kind: PromptKind,
    pub question: String,
    pub value: String,
}

pub struct AppState {
    pub workbench: crate::views::WorkbenchState,
    pub commands: Vec<CommandDef>,
    pub selected: usize,
    pub filter: String,
    pub mode: Mode,
    pub form: Option<ActionForm>,
    pub prompt: Option<PendingPrompt>,
    pub context: BTreeMap<String, String>,
    pub snapshots: BTreeMap<String, Value>,
    pub snapshot_requests: BTreeMap<String, ClientCommand>,
    pub pending: BTreeMap<String, ClientCommand>,
    pub output: String,
    pub output_omitted: bool,
    pub events: VecDeque<Value>,
    pub scroll: crate::scroll::ScrollState,
    pub status: String,
    pub connection: String,
    pub active_request: Option<String>,
    pub queue: VecDeque<ClientCommand>,
    pub last_read: Option<ClientCommand>,
    pub quit: bool,
    pub bridge_failed: bool,
    pub approval_visible: bool,
    pub guide: Option<crate::guide::GuideState>,
    next_id: u64,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            workbench: crate::views::WorkbenchState::default(),
            commands: Vec::new(),
            selected: 0,
            filter: String::new(),
            mode: Mode::Catalog,
            form: None,
            prompt: None,
            context: BTreeMap::new(),
            snapshots: BTreeMap::new(),
            snapshot_requests: BTreeMap::new(),
            pending: BTreeMap::new(),
            output: String::new(),
            output_omitted: false,
            events: VecDeque::new(),
            scroll: crate::scroll::ScrollState::default(),
            status: "Connecting to the local runtime…".into(),
            connection: "connecting".into(),
            active_request: None,
            queue: VecDeque::new(),
            last_read: None,
            quit: false,
            bridge_failed: false,
            approval_visible: false,
            guide: None,
            next_id: 0,
        }
    }
}

impl AppState {
    /// Frame-clock hook: true while an eased scroll still needs frames at cadence.
    pub fn animating(&self) -> bool {
        self.scroll.animating() || self.guide.as_ref().is_some_and(|guide| guide.animating())
    }
    /// The editor a guide asked for, to run with the terminal suspended: (prompt id, path).
    pub fn take_editor(&mut self) -> Option<(String, String)> {
        self.guide.as_mut().and_then(|guide| guide.editor.clone())
    }
    /// Answers the guide's edit question after the main loop ran the editor.
    pub fn editor_closed(&mut self, prompt_id: String, saved: bool) {
        let id = self.id();
        if let Some(guide) = self.guide.as_mut() {
            let reply = guide.edited(id, prompt_id, saved);
            self.queue.push_back(reply);
        }
    }
    /// Frame-clock hook: advance animation state by the clamped time since the
    /// previous frame.
    pub fn advance_animations(&mut self, dt: std::time::Duration) {
        self.scroll.advance(dt);
    }
    pub fn set_scroll_motion(&mut self, motion: crate::scroll::ScrollMotion) {
        self.scroll.set_motion(motion);
    }
    pub fn id(&mut self) -> String {
        self.next_id += 1;
        format!("ui-{}", self.next_id)
    }
    pub fn start(&mut self) {
        let id = self.id();
        self.queue
            .push_back(ClientCommand::new(id, "catalog", json!({})));
    }
    pub fn filtered(&self) -> Vec<&CommandDef> {
        self.commands
            .iter()
            .filter(|c| self.filter.is_empty() || c.context == self.filter)
            .filter(|c| {
                self.workbench.query.is_empty()
                    || format!("{} {}", c.name, c.action)
                        .to_lowercase()
                        .contains(&self.workbench.query.to_lowercase())
            })
            .collect()
    }
    pub fn append(&mut self, text: &str) {
        self.output_omitted |= bounded_append(&mut self.output, text);
    }
    /// True (with the status saying so) once the bridge has stopped: nothing more can be sent.
    fn refuse_when_transport_stopped(&mut self) -> bool {
        if self.bridge_failed {
            self.status =
                "Transport stopped. Restart the terminal to reconnect; no action was sent.".into();
        }
        self.bridge_failed
    }
    pub fn begin(&mut self, name: &str) {
        if self.refuse_when_transport_stopped() {
            return;
        }
        if self.active_request.is_some() {
            self.status = "Finish or Esc-abort the current local action first.".into();
            return;
        }
        if let Some(command) = self.commands.iter().find(|c| c.name == name).cloned() {
            let mut form = ActionForm::new(command, &self.context);
            crate::views::offer_choices(self, &mut form);
            self.form = Some(form);
            self.mode = Mode::Form;
            self.status.clear();
            if self.form.as_ref().is_some_and(|f| f.fields.is_empty()) {
                self.submit();
            }
        }
    }
    pub fn invoke(
        &mut self,
        name: &str,
        args: Vec<String>,
        flags: Map<String, Value>,
        read_only: bool,
    ) {
        if self.refuse_when_transport_stopped() {
            return;
        }
        if self.active_request.is_some() {
            self.status = "Another action is still active.".into();
            return;
        }
        let id = self.id();
        let request = ClientCommand::new(
            &id,
            if read_only { "read" } else { "invoke" },
            json!({"name":name,"args":args,"flags":flags}),
        );
        if let Err(error) = request.encode() {
            self.status = error;
            return;
        }
        self.active_request = Some(id.clone());
        // A failed/replaced read must never leave another record's body actionable.
        self.snapshots.remove(name);
        self.snapshot_requests.remove(name);
        self.pending.insert(id, request.clone());
        if read_only {
            self.last_read = Some(request.clone());
        }
        self.output.clear();
        self.output_omitted = false;
        self.scroll.reset();
        self.events.clear();
        self.mode = Mode::Result;
        self.status = format!("{name} · waiting for the runtime · Esc stops waiting");
        self.queue.push_back(request);
    }
    pub fn submit(&mut self) {
        let Some(form) = &self.form else {
            return;
        };
        match form.input() {
            Ok((mut args, flags)) => {
                let document = form
                    .fields
                    .iter()
                    .find(|f| f.name == "@document")
                    .map(|f| f.value.clone());
                let name = form.command.name.clone();
                let read = form.command.read_only && !flags.contains_key("output");
                // One resolution (ADR 0042): a short id or `latest` that names a loaded record is
                // sent as that record's exact identity, so the request, the context and the
                // snapshot's provenance agree. One that nothing loaded resolves is sent as typed
                // for the command to resolve or refuse, and its context is cleared, never kept as
                // the reference, until a read returns the record.
                let mut contexts = Vec::new();
                for (arg, value) in form.command.args.iter().zip(args.iter_mut()) {
                    if arg.context {
                        let exact = crate::views::exact_context(self, &arg.name, value);
                        if let Some(exact) = &exact {
                            value.clone_from(exact);
                        }
                        contexts.push((arg.name.clone(), exact));
                    }
                }
                for (name, value) in contexts {
                    if name == "project" {
                        if let Some(value) = value {
                            crate::views::choose_project(self, &value);
                        }
                    } else {
                        // Clearing is a change too: the ticket, plan and run it linked go with it.
                        if value.is_none() || self.context.get(&name) != value.as_ref() {
                            match name.as_str() {
                                "ticket" => {
                                    self.context.remove("run");
                                    self.context.remove("plan");
                                }
                                "run" => {
                                    self.context.remove("ticket");
                                    self.context.remove("plan");
                                }
                                "plan" => {
                                    self.context.remove("ticket");
                                    self.context.remove("run");
                                }
                                _ => {}
                            }
                        }
                        match value {
                            Some(value) => self.context.insert(name, value),
                            None => self.context.remove(&name),
                        };
                    }
                }
                self.invoke(&name, args, flags, read);
                if let (Some(document), Some(id)) = (document, self.active_request.clone()) {
                    if let Some(request) = self.queue.iter_mut().find(|r| r.id == id) {
                        request
                            .fields
                            .insert("stdin_document".into(), json!(document));
                        if let Some(pending) = self.pending.get_mut(&id) {
                            *pending = request.clone();
                        }
                    }
                }
            }
            Err(error) => self.status = error,
        }
    }
    pub fn refresh(&mut self) {
        if self.active_request.is_some() {
            return;
        }
        if self.mode == Mode::Form {
            self.status = "Connection restored. Your unsent form is preserved; the runtime checks its revision when submitted.".into();
            return;
        }
        if self.mode == Mode::Workbench {
            crate::views::load(self);
            return;
        }
        if let Some(request) = self.last_read.clone() {
            let name = request
                .fields
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("");
            let args =
                serde_json::from_value(request.fields.get("args").cloned().unwrap_or(json!([])))
                    .unwrap_or_default();
            let flags = request
                .fields
                .get("flags")
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default();
            self.invoke(name, args, flags, true);
        }
    }
    pub fn abort(&mut self) {
        if let Some(request_id) = self.active_request.clone() {
            let id = self.id();
            self.queue.push_back(ClientCommand::new(
                id,
                "abort",
                json!({"request_id":request_id}),
            ));
            self.prompt = None;
            self.status = "Stopping local request; runtime work is not cancelled.".into();
        } else {
            self.mode = Mode::Catalog;
            self.form = None;
        }
    }
    pub fn transport_failed(&mut self, message: &str) {
        self.bridge_failed = true;
        self.connection = "offline".into();
        self.status = safe_text(message);
        self.prompt = None;
        self.active_request = None;
        self.pending.clear();
        self.queue.clear();
    }
    pub fn apply(&mut self, event: BridgeEvent) {
        if self
            .guide
            .as_ref()
            .is_some_and(|guide| guide.request_id == event.id)
        {
            self.apply_guide(event);
            return;
        }
        let notification = event.clone();
        let original = self.pending.get(&event.id).cloned();
        let origin_name = original
            .as_ref()
            .and_then(|r| r.fields.get("name"))
            .and_then(Value::as_str)
            .map(str::to_owned);
        let was_read = original.as_ref().is_some_and(|r| r.op == "read");
        match event.event.as_str() {
            "catalog" => {
                match serde_json::from_value::<Vec<CommandDef>>(
                    event.fields.get("commands").cloned().unwrap_or(Value::Null),
                ) {
                    Ok(commands) => {
                        self.commands = commands;
                        self.status = "Choose an action · F1 help · F2 catalog".into();
                    }
                    Err(_) => self.transport_failed("Bridge catalog is invalid."),
                }
            }
            "connection" => {
                let previous = self.connection.clone();
                self.connection = event.text("state").to_string();
                self.status = safe_text(event.text("message"));
                if self.connection == "connected"
                    && ["offline", "reconnecting", "disconnected"].contains(&previous.as_str())
                {
                    self.refresh();
                }
            }
            "prompt" if self.pending.contains_key(&event.id) => {
                self.mode = Mode::Result;
                if self.output_omitted && event.text("kind") == "hash" {
                    self.abort();
                    self.status =
                        "Approval stopped: the complete plan exceeds the display limit.".into();
                    return;
                }
                self.prompt = Some(PendingPrompt {
                    request_id: event.id.clone(),
                    prompt_id: event.text("prompt_id").into(),
                    kind: PromptKind::from_wire(event.text("kind")),
                    question: safe_text(event.text("question")),
                    value: String::new(),
                });
            }
            "output" if self.pending.contains_key(&event.id) => self.append(event.text("text")),
            "event" if self.pending.contains_key(&event.id) => {
                if let Some(record) = event.fields.get("record") {
                    self.events.push_back(record.clone());
                    if self.events.len() > MAX_EVENTS {
                        self.events.pop_front();
                        self.output_omitted = true;
                    }
                    self.append(&format!("{}\n", record));
                }
            }
            "result" | "error" => {
                let Some(request) = self.pending.remove(&event.id) else {
                    return;
                };
                if self.active_request.as_deref() == Some(&event.id) {
                    self.active_request = None;
                    self.prompt = None;
                }
                if event.event == "error" {
                    let message = format!("{}{}",safe_text(event.text("message")),match event.text("outcome") {
                        "unknown"=>" Outcome unknown: inspect authoritative state before retrying; the runtime may have completed this action.",
                        "completed"=>" Execution completed, but the response is unavailable. Inspect authoritative state; do not repeat the mutation.",
                        _=>"",
                    });
                    self.status = message.clone();
                    self.append(&format!("\n{message}\n"));
                } else {
                    let key = request
                        .fields
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or("result")
                        .to_string();
                    if self.snapshots.len() >= 16 && !self.snapshots.contains_key(&key) {
                        if let Some((removed, _)) = self.snapshots.pop_first() {
                            self.snapshot_requests.remove(&removed);
                        }
                    }
                    self.snapshot_requests.insert(key.clone(), request.clone());
                    self.snapshots.insert(
                        key,
                        event.fields.get("value").cloned().unwrap_or(Value::Null),
                    );
                    self.append(event.text("formatted"));
                    self.status =
                        "Runtime response received · F5 refresh reads · Esc catalog".into();
                }
            }
            _ => {}
        }
        crate::views::received(self, &notification, origin_name.as_deref(), was_read);
    }
    /// Frames of the running guide request: its presentation frames and how it ended.
    fn apply_guide(&mut self, event: BridgeEvent) {
        let Some(guide) = self.guide.as_mut() else {
            return;
        };
        match event.event.as_str() {
            "guide" => guide.apply(&event.fields),
            "result" | "error" => {
                if event.event == "result" {
                    guide.ended(event.fields.get("value").unwrap_or(&Value::Null));
                } else {
                    let outcome = match event.text("outcome") {
                        "unknown" | "completed" => {
                            " A step may have completed: inspect the project before repeating it."
                        }
                        _ => "",
                    };
                    guide.failed(&format!("{}{outcome}", event.text("message")));
                }
                self.pending.remove(&event.id);
                if self.active_request.as_deref() == Some(&event.id) {
                    self.active_request = None;
                }
                self.status = "Setup ended · Enter opens the workbench".into();
            }
            _ => {}
        }
    }
    /// True while keystrokes are typed text: a form, a prompt, a search, a guide question.
    pub fn text_entry(&self) -> bool {
        self.mode == Mode::Form
            || self.prompt.is_some()
            || self.workbench.searching
            || self.mode == Mode::Guide
                && self
                    .guide
                    .as_ref()
                    .is_some_and(|guide| guide.question.is_some())
    }
    pub fn key(&mut self, key: KeyEvent) {
        let Some(key) = crate::keys::keymap().translate(key, self.text_entry()) else {
            return;
        };
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.quit = true;
            return;
        }
        if self.mode == Mode::Guide {
            crate::views::guide::key(self, key);
            return;
        }
        // Outside forms and prompts a Ctrl or Alt letter is never the plain letter: Ctrl+S
        // must not start setup, nor Ctrl+U flip Atlas's user memory.
        if self.mode != Mode::Form
            && self.prompt.is_none()
            && matches!(key.code, KeyCode::Char(_))
            && key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
            && !(self.text_entry() && crate::keys::is_text(&key))
        {
            return;
        }
        if crate::views::key(self, key) {
            return;
        }
        if key.code == KeyCode::Esc {
            if self.prompt.is_some() || self.active_request.is_some() {
                self.abort();
            } else {
                crate::views::back(self);
            }
            return;
        }
        if matches!(key.code, KeyCode::PageUp | KeyCode::PageDown)
            && (self.mode == Mode::Result || self.prompt.is_some())
        {
            if key.code == KeyCode::PageUp {
                self.scroll.scroll_up(8);
            } else {
                self.scroll.scroll_down(8);
            }
            return;
        }
        if self.prompt.is_some() {
            self.prompt_key(key);
        } else if self.mode == Mode::Form {
            self.form_key(key);
        } else {
            self.catalog_key(key);
        }
    }
    /// Keys while a runtime question is open: scroll a plan under review, edit or send the answer.
    fn prompt_key(&mut self, key: KeyEvent) {
        let Some(prompt) = &mut self.prompt else {
            return;
        };
        if prompt.kind == PromptKind::Hash {
            match key.code {
                KeyCode::Up => {
                    self.scroll.scroll_up(1);
                    return;
                }
                KeyCode::Down => {
                    self.scroll.scroll_down(1);
                    return;
                }
                KeyCode::Home => {
                    self.scroll.to_top();
                    return;
                }
                KeyCode::End => {
                    self.scroll.to_bottom();
                    return;
                }
                _ => {}
            }
        }
        if prompt.kind == PromptKind::Confirm {
            // A yes/no question is answered with one key, never typed out.
            let answer = match key.code {
                KeyCode::Char('y' | 'Y') => true,
                KeyCode::Char('n' | 'N') => false,
                _ => {
                    self.status = "Press y for yes or n for no.".into();
                    return;
                }
            };
            self.answer(Value::Bool(answer));
            return;
        }
        let submit_document = prompt.kind == PromptKind::Document
            && key.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(key.code, KeyCode::Char('s') | KeyCode::Enter);
        if submit_document || key.code == KeyCode::Enter && prompt.kind != PromptKind::Document {
            if prompt.kind == PromptKind::Hash && !self.approval_visible {
                self.status = "Enlarge the terminal to review the plan before approval.".into();
                return;
            }
            let value = Value::String(prompt.value.clone());
            self.answer(value);
        } else {
            edit(&mut prompt.value, key, prompt.kind == PromptKind::Document);
        }
    }
    /// Sends the open runtime question's answer and closes it.
    fn answer(&mut self, value: Value) {
        let Some(prompt) = self.prompt.take() else {
            return;
        };
        let id = self.id();
        self.queue.push_back(ClientCommand::new(
            id,
            "reply",
            json!({"request_id":prompt.request_id,"prompt_id":prompt.prompt_id,"value":value}),
        ));
    }
    /// Keys in an action form: move between fields, pick a choice, edit, submit.
    fn form_key(&mut self, key: KeyEvent) {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('s') {
            self.submit();
            return;
        }
        if let Some(form) = &mut self.form {
            let count = form.fields.len();
            if count == 0 {
                return;
            }
            match key.code {
                KeyCode::Tab | KeyCode::Down => form.cursor = (form.cursor + 1) % count,
                KeyCode::BackTab | KeyCode::Up => form.cursor = (form.cursor + count - 1) % count,
                KeyCode::Left | KeyCode::Right
                    if form
                        .fields
                        .get(form.cursor)
                        .is_some_and(|f| !f.choices.is_empty()) =>
                {
                    if let Some(field) = form.fields.get_mut(form.cursor) {
                        field.cycle(key.code == KeyCode::Right);
                    }
                }
                KeyCode::Enter if !key.modifiers.contains(KeyModifiers::ALT) => {
                    if form.cursor + 1 == count {
                        self.submit();
                    } else {
                        form.cursor += 1;
                    }
                }
                _ => {
                    if let Some(field) = form.fields.get_mut(form.cursor) {
                        field.press(key);
                    }
                }
            }
        }
    }
    /// Keys in the command catalog and the result view.
    fn catalog_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('q') => self.quit = true,
            KeyCode::F(1) if self.active_request.is_none() => {
                self.mode = Mode::Result;
                self.output = HELP.into();
                self.scroll.reset();
            }
            KeyCode::F(2) if self.active_request.is_none() => {
                self.mode = Mode::Catalog;
                self.filter.clear();
            }
            KeyCode::F(5) | KeyCode::Char('r') => self.refresh(),
            KeyCode::Up => {
                if self.mode == Mode::Result {
                    self.scroll.scroll_up(1);
                } else {
                    self.selected = self.selected.saturating_sub(1);
                }
            }
            KeyCode::Down => {
                if self.mode == Mode::Result {
                    self.scroll.scroll_down(1);
                } else {
                    self.selected =
                        (self.selected + 1).min(self.filtered().len().saturating_sub(1));
                }
            }
            KeyCode::Home => {
                self.scroll.to_top();
                self.selected = 0;
            }
            KeyCode::End => {
                if self.mode == Mode::Catalog {
                    self.selected = self.filtered().len().saturating_sub(1);
                } else {
                    self.scroll.to_bottom();
                }
            }
            KeyCode::PageDown => {
                self.selected = (self.selected + 8).min(self.filtered().len().saturating_sub(1))
            }
            KeyCode::PageUp => self.selected = self.selected.saturating_sub(8),
            KeyCode::Tab if self.mode == Mode::Catalog => {
                const CONTEXTS: [&str; 6] = ["", "runtime", "project", "ticket", "plan", "run"];
                let next = (CONTEXTS.iter().position(|v| *v == self.filter).unwrap_or(0) + 1)
                    % CONTEXTS.len();
                self.filter = CONTEXTS[next].into();
                self.selected = 0;
            }
            KeyCode::Enter if self.mode == Mode::Catalog => {
                if let Some(name) = self.filtered().get(self.selected).map(|c| c.name.clone()) {
                    self.begin(&name);
                }
            }
            _ => {}
        }
    }
    pub fn paste(&mut self, text: &str) {
        if self.mode == Mode::Guide {
            if let Some(guide) = self.guide.as_mut() {
                guide.paste(text);
            }
            return;
        }
        let clean = safe_text(text);
        let target = if let Some(prompt) = &mut self.prompt {
            Some(&mut prompt.value)
        } else if let Some(form) = &mut self.form {
            if self.mode == Mode::Form {
                form.fields.get_mut(form.cursor).map(|f| &mut f.value)
            } else {
                None
            }
        } else {
            None
        };
        if let Some(target) = target {
            if target.len() + clean.len() <= MAX_INPUT {
                target.push_str(&clean);
            } else {
                self.status = "Input exceeds the 256 KiB form limit.".into();
            }
        }
    }
}

pub const HELP: &str = "LUUNA / TERMINAL WORKBENCH\n\nThe runtime owns projects, tickets, plans, execution and evidence.\nUse F2 (or :) to browse every catalog action. Tab filters its context.\nForms expose every argument and option. Empty options are omitted.\nTab/Up/Down change fields; Alt+Enter (or Ctrl+J) adds a line. Repeated flags use one value per line.\nLeft/Right pick from a field's choices (repositories, records, scopes, yes/no); typing still works.\nEnter advances; Ctrl+S submits. Boolean options take yes or no.\nDocument prompts: Enter adds a line, Ctrl+S submits.\n\nApproval displays the complete plan and hash; type approve (or its first 8 hash digits) to approve exactly that plan.\nTicket, plan, run and memory ids accept their short id: the first 4 or more characters lists show.\nEsc stops a LOCAL request or follower. It does not cancel runtime work.\nChoose the cancel action to request runtime cancellation.\nF5 (or Ctrl+R) refreshes the latest read. Reconnect never retries a mutation.\n\nSTATES\npassed/accepted: verified success\nrunning/implementing/validating: active\nfailed/rejected: failure\nblocked/pending: attention\ninterrupted/reconnecting: interrupted\ncompleted/cancelled: neutral\n\nHome and Board are read-only projections. Catalog actions expose durable evidence, artifacts, memory and publication status. Merging stays the developer's decision.\n\nPgUp/PgDn scroll; q or Ctrl+C exits and leaves the runtime running.";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{ArgumentDef, FlagDef};
    #[test]
    fn forms_keep_required_context_repeats_false_and_exact_text() {
        let command = CommandDef {
            name: "plan".into(),
            args: vec![ArgumentDef {
                name: "ticket".into(),
                context: true,
                ..Default::default()
            }],
            flags: BTreeMap::from([
                (
                    "task".into(),
                    FlagDef {
                        repeat: true,
                        ..Default::default()
                    },
                ),
                (
                    "confirm".into(),
                    FlagDef {
                        boolean: true,
                        ..Default::default()
                    },
                ),
            ]),
            ..Default::default()
        };
        let mut form = ActionForm::new(
            command,
            &BTreeMap::from([("ticket".into(), "ticket-1".into())]),
        );
        for field in &mut form.fields {
            if field.name == "task" {
                field.value = "api=src\nweb=lib>api".into();
            }
            if field.name == "confirm" {
                field.value = "no".into();
            }
        }
        let (args, flags) = form.input().unwrap();
        assert_eq!(args, ["ticket-1"]);
        assert_eq!(flags["confirm"], false);
        assert_eq!(flags["task"], json!(["api=src", "web=lib>api"]));
    }
    #[test]
    fn prompt_answers_are_fresh_and_escape_never_cancels_runtime() {
        let mut app = AppState::default();
        app.invoke("approve", vec!["p".into()], Map::new(), false);
        let id = app.active_request.clone().unwrap();
        app.apply(serde_json::from_value(json!({"v":1,"id":id,"event":"prompt","prompt_id":"hash1","kind":"hash","question":"hash?"})).unwrap());
        assert_eq!(app.prompt.as_ref().unwrap().value, "");
        app.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(app.queue.back().unwrap().op, "abort");
        assert!(app
            .queue
            .iter()
            .all(|q| q.fields.get("name") != Some(&json!("cancel"))));
    }
    #[test]
    fn mac_friendly_keys_open_screens_but_stay_text_in_forms() {
        let mut app = AppState {
            mode: Mode::Workbench,
            ..Default::default()
        };
        app.key(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::SHIFT));
        assert_eq!(app.workbench.screen, crate::views::Screen::Help);
        app.key(KeyEvent::new(KeyCode::Char('m'), KeyModifiers::NONE));
        assert!(app.workbench.nav);
        let command = CommandDef {
            name: "remember".into(),
            flags: BTreeMap::from([("content".into(), FlagDef::default())]),
            ..Default::default()
        };
        app.form = Some(ActionForm::new(command, &BTreeMap::new()));
        app.mode = Mode::Form;
        for key in [
            KeyEvent::new(KeyCode::Char('?'), KeyModifiers::SHIFT),
            KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE),
            KeyEvent::new(KeyCode::Char('j'), KeyModifiers::CONTROL),
            KeyEvent::new(
                KeyCode::Char('@'),
                KeyModifiers::CONTROL | KeyModifiers::ALT,
            ),
        ] {
            app.key(key);
        }
        assert_eq!(app.mode, Mode::Form);
        assert!(!app.quit);
        let value = &app.form.as_ref().unwrap().fields[0].value;
        assert_eq!(value, "?q\n@");
    }
    #[test]
    fn unbinding_down_also_removes_its_letter_alias() {
        let (map, _) = crate::keys::Keymap::build(&BTreeMap::from([("down".into(), vec![])]));
        let j = KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE);
        let down = KeyEvent::new(KeyCode::Down, KeyModifiers::NONE);
        assert_eq!(map.translate(down, false), None);
        // `j` reaches handlers only as itself, and no handler moves on a raw `j` any more.
        let mut app = AppState {
            mode: Mode::Catalog,
            selected: 1,
            ..Default::default()
        };
        app.key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE));
        assert_eq!(app.selected, 0);
        assert_eq!(map.translate(j, false), Some(j));
    }
    #[test]
    fn prompt_kinds_come_from_the_wire_and_unknown_ones_are_text() {
        assert_eq!(PromptKind::from_wire("hash"), PromptKind::Hash);
        assert_eq!(PromptKind::from_wire("document"), PromptKind::Document);
        assert_eq!(PromptKind::from_wire("confirm"), PromptKind::Confirm);
        assert_eq!(PromptKind::from_wire("text"), PromptKind::Text);
        assert_eq!(PromptKind::from_wire("something-new"), PromptKind::Text);
    }
    #[test]
    fn stale_prompt_cannot_replace_active_request() {
        let mut app = AppState::default();
        app.apply(serde_json::from_value(json!({"v":1,"id":"stale","event":"prompt","prompt_id":"p","kind":"hash","question":"hash?"})).unwrap());
        assert!(app.prompt.is_none());
    }
}
