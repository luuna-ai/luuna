//! The workbench's command service, in process. It speaks the versioned presentation protocol the
//! workbench was built on (crate::protocol: catalog, invoke, read, guide, reply, abort, shutdown
//! requests; catalog, result, output, prompt, event, connection, error and guide events), runs
//! every action through the same command table as the command line, polls the runtime's health,
//! and redacts the service token from every event. It replaces the former Node bridge process with
//! identical semantics: limits (four reads, one mutation), prompts for approval hashes, documents
//! and confirmations, and the outcome classes the workbench explains after a failure.
use crate::client::{Client, Controller, Signal};
use crate::commands::{self, by_name, read_only, usage, FlagValue, Fut, Input, Io};
use crate::failures::{describe_failure, Failure};
use crate::format::{format, plain};
use crate::js::J;
use crate::prime::GuideUi;
use crate::protocol::{BridgeEvent, ClientCommand, EventKind, VERSION};
use serde_json::{json, Map, Value};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

const READS: usize = 4;
const MUTATIONS: usize = 1;
const DISPLAY: usize = 256 * 1024;
const HEALTH: Duration = Duration::from_millis(2000);
const HEALTH_TIMEOUT: Duration = Duration::from_millis(2000);

struct Pending {
    id: String,
    kind: &'static str,
    choices: Option<usize>,
    answer: oneshot::Sender<Value>,
}

struct Task {
    controller: Controller,
    prompt: Option<Pending>,
    mutation: bool,
}

#[derive(Default)]
struct State {
    active: HashMap<String, Task>,
    controls: HashSet<String>,
    reads: usize,
    mutations: usize,
    closed: bool,
    prompts: u64,
    connection: Option<&'static str>,
    health_running: bool,
}

struct Service {
    token: String,
    url: Option<String>,
    secrets: Vec<String>,
    state: Mutex<State>,
    events: mpsc::UnboundedSender<Result<BridgeEvent, String>>,
}

fn protocol(code: &str, message: &str) -> Failure {
    Failure::Protocol {
        code: code.into(),
        message: message.into(),
    }
}

impl Service {
    fn redact_text(&self, text: &str) -> String {
        self.secrets.iter().fold(text.to_string(), |text, secret| {
            text.split(secret.as_str())
                .collect::<Vec<_>>()
                .join("[redacted]")
        })
    }
    fn redact(&self, value: Value) -> Value {
        match value {
            Value::String(text) => Value::String(self.redact_text(&text)),
            Value::Array(items) => {
                Value::Array(items.into_iter().map(|v| self.redact(v)).collect())
            }
            Value::Object(map) => Value::Object(
                map.into_iter()
                    .map(|(k, v)| (self.redact_text(&k), self.redact(v)))
                    .collect(),
            ),
            other => other,
        }
    }
    fn emit(&self, id: &str, event: EventKind, fields: Value) {
        let fields = match self.redact(fields) {
            Value::Object(map) => map,
            _ => Map::new(),
        };
        let _ = self.events.send(Ok(BridgeEvent {
            v: VERSION,
            id: self.redact_text(id),
            event,
            fields,
        }));
    }
    fn error(&self, id: &str, code: &str, message: &str, outcome: &str, exit_code: u8) {
        self.emit(id, EventKind::Error, json!({"code": code, "message": plain(message), "outcome": outcome, "exit_code": exit_code}));
    }
    fn client(&self) -> Result<Client, Failure> {
        let url = self
            .url
            .clone()
            .map(|u| crate::builders::js_trim(&u))
            .filter(|u| !u.is_empty())
            .unwrap_or_else(|| crate::cli::DEFAULT_URL.into());
        if crate::builders::js_trim(&self.token).is_empty() {
            return Err(Failure::Usage("Set LUUNA_API_TOKEN to the runtime's service token (or run interactively to type it).".into()));
        }
        Client::new(&url, &self.token)
    }
    fn close(&self) {
        let mut state = self.state.lock().unwrap();
        state.closed = true;
        for task in state.active.values() {
            task.controller.abort();
        }
    }

    /// Asks the workbench a question for `request` and waits for its reply (or the request's abort).
    async fn question(
        self: &Arc<Self>,
        request: &str,
        kind: &'static str,
        event: EventKind,
        mut frame: Value,
        choices: Option<usize>,
        signal: &Signal,
    ) -> Result<Value, Failure> {
        if signal.aborted() {
            return Err(Failure::Aborted);
        }
        let (answer, receiver) = oneshot::channel();
        let prompt_id = {
            let mut state = self.state.lock().unwrap();
            state.prompts += 1;
            let prompt_id = format!("prompt-{}", state.prompts);
            let Some(task) = state.active.get_mut(request) else {
                return Err(Failure::Aborted);
            };
            task.prompt = Some(Pending {
                id: prompt_id.clone(),
                kind,
                choices,
                answer,
            });
            prompt_id
        };
        if let Value::Object(map) = &mut frame {
            map.insert("prompt_id".into(), json!(prompt_id));
        }
        self.emit(request, event, frame);
        let result = signal
            .race(async { receiver.await.map_err(|_| Failure::Aborted) })
            .await;
        if let Some(task) = self.state.lock().unwrap().active.get_mut(request) {
            if task.prompt.as_ref().is_some_and(|p| p.id == prompt_id) {
                task.prompt = None;
            }
        }
        result
    }

    async fn health(self: &Arc<Self>) {
        {
            let mut state = self.state.lock().unwrap();
            if state.closed || state.health_running {
                return;
            }
            state.health_running = true;
        }
        let (controller, signal) = Controller::new();
        let result = match self.client() {
            Ok(client) => {
                match tokio::time::timeout(HEALTH_TIMEOUT, client.get("/health", &signal)).await {
                    Ok(result) => result.map(|_| ()),
                    Err(_) => {
                        controller.abort();
                        Err(Failure::Aborted)
                    }
                }
            }
            Err(failure) => Err(failure),
        };
        let mut state = self.state.lock().unwrap();
        state.health_running = false;
        if state.closed {
            return;
        }
        match result {
            Ok(()) => {
                if state.connection != Some("connected") {
                    drop(state);
                    self.emit("connection", EventKind::Connection, json!({"state": "connected", "message": "Runtime connected. Reload authoritative views."}));
                    self.state.lock().unwrap().connection = Some("connected");
                }
            }
            Err(failure) => {
                if state.connection != Some("disconnected") {
                    drop(state);
                    self.emit("connection", EventKind::Connection, json!({"state": "disconnected", "message": plain(&describe_failure(&failure))}));
                    self.state.lock().unwrap().connection = Some("disconnected");
                }
            }
        }
    }

    fn validate(&self, request: &ClientCommand) -> Result<(), Failure> {
        let id_ok = !request.id.is_empty()
            && request.id.len() <= 128
            && request
                .id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "_.:-".contains(c));
        if request.v != VERSION || !id_ok {
            return Err(protocol(
                "invalid_request",
                "Expected protocol v1 and a nonempty ASCII correlation id (up to 128 characters).",
            ));
        }
        let fields: &[&str] = match request.op.as_str() {
            "catalog" | "shutdown" => &[],
            "invoke" => &["name", "args", "flags", "stdin_document"],
            "read" => &["name", "args", "flags"],
            "guide" => &["name", "flags"],
            "reply" => &["request_id", "prompt_id", "value"],
            "abort" => &["request_id"],
            _ => return Err(protocol("invalid_operation", "Unknown bridge operation.")),
        };
        if request.fields.keys().any(|k| !fields.contains(&k.as_str())) {
            return Err(protocol("invalid_request", "Unexpected protocol field."));
        }
        let state = self.state.lock().unwrap();
        if state.active.contains_key(&request.id) || state.controls.contains(&request.id) {
            return Err(protocol(
                "duplicate_id",
                "That correlation id is already active.",
            ));
        }
        if state.closed {
            return Err(protocol("closed", "The bridge is closed."));
        }
        Ok(())
    }

    /// The catalog the workbench builds its menus from: the actions and the projections.
    fn catalog(&self) -> Value {
        let table = commands::table();
        let commands: Vec<Value> = table
            .commands
            .iter()
            .chain(table.projections.iter())
            .map(|command| {
                let mut entry = match command.raw.to_serde() {
                    Value::Object(map) => map,
                    _ => Map::new(),
                };
                entry.remove("example");
                // How a result marks the command failed is the client's own rule, not catalog data.
                entry.remove("failed");
                if let Some(Value::Object(flags)) = entry.get_mut("flags") {
                    flags.remove("acknowledge-hash");
                }
                entry.insert("read_only".into(), json!(read_only(command)));
                Value::Object(entry)
            })
            .collect();
        json!({ "commands": commands })
    }

    async fn dispatch(self: Arc<Self>, request: ClientCommand) {
        let id = if !request.id.is_empty()
            && request.id.len() <= 128
            && request
                .id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "_.:-".contains(c))
        {
            request.id.clone()
        } else {
            "protocol".into()
        };
        if let Err(failure) = self.validate(&request) {
            let (code, message) = match failure {
                Failure::Protocol { code, message } => (code, message),
                other => ("invalid_request".into(), other.message()),
            };
            self.error(&id, &code, &message, "not_executed", 2);
            return;
        }
        match request.op.as_str() {
            "catalog" => self.emit(&id, EventKind::Catalog, self.catalog()),
            "reply" => {
                let target = request
                    .fields
                    .get("request_id")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let prompt_id = request
                    .fields
                    .get("prompt_id")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let value = request.fields.get("value").cloned().unwrap_or(Value::Null);
                let pending = {
                    let mut state = self.state.lock().unwrap();
                    let matches = state
                        .active
                        .get(&target)
                        .and_then(|t| t.prompt.as_ref())
                        .is_some_and(|p| p.id == prompt_id);
                    if !matches {
                        None
                    } else {
                        let prompt = state.active.get(&target).unwrap().prompt.as_ref().unwrap();
                        let valid = match prompt.kind {
                            "confirm" => value.is_boolean(),
                            "choice" => {
                                value.as_u64().is_some_and(|i| {
                                    prompt.choices.is_some_and(|n| (i as usize) < n)
                                }) && value.is_u64()
                            }
                            "checklist" => value.as_array().is_some_and(|items| {
                                Some(items.len()) == prompt.choices
                                    && items.iter().all(Value::is_boolean)
                            }),
                            _ => value.is_string(),
                        };
                        if valid {
                            state.active.get_mut(&target).unwrap().prompt.take()
                        } else {
                            drop(state);
                            self.error(&id, "invalid_reply", "Confirmation answers are booleans, choices an option index, checklists one boolean per option, text answers strings.", "not_executed", 2);
                            return;
                        }
                    }
                };
                match pending {
                    None => self.error(
                        &id,
                        "stale_prompt",
                        "This prompt is no longer awaiting an answer.",
                        "not_executed",
                        2,
                    ),
                    Some(pending) => {
                        let _ = pending.answer.send(value);
                        self.emit(
                            &id,
                            EventKind::Result,
                            json!({"value": {"replied": true}, "formatted": "", "code": 0}),
                        );
                    }
                }
            }
            "abort" => {
                let target = request
                    .fields
                    .get("request_id")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let found = self
                    .state
                    .lock()
                    .unwrap()
                    .active
                    .get(&target)
                    .map(|t| t.controller.abort())
                    .is_some();
                if found {
                    self.emit(
                        &id,
                        EventKind::Result,
                        json!({"value": {"aborted": target}, "formatted": "", "code": 0}),
                    );
                } else {
                    self.error(
                        &id,
                        "unknown_request",
                        "This request is not active.",
                        "not_executed",
                        2,
                    );
                }
            }
            "shutdown" => {
                self.close();
                self.emit(
                    &id,
                    EventKind::Result,
                    json!({"value": {"closed": true}, "formatted": "", "code": 0}),
                );
            }
            "guide" => self.guide(id, request).await,
            _ => self.invoke(id, request).await,
        }
    }

    /// Registers a request within the limits; its signal fires on `abort` or shutdown.
    fn reserve(&self, id: &str, mutation: bool) -> Result<Signal, Failure> {
        let mut state = self.state.lock().unwrap();
        if if mutation {
            state.mutations >= MUTATIONS
        } else {
            state.reads >= READS
        } {
            return Err(protocol(
                "busy",
                "The bridge already has its maximum concurrent requests.",
            ));
        }
        if mutation {
            state.mutations += 1;
        } else {
            state.reads += 1;
        }
        let controller = Controller::default();
        let signal = controller.signal();
        state.active.insert(
            id.into(),
            Task {
                controller,
                prompt: None,
                mutation,
            },
        );
        Ok(signal)
    }

    fn release(&self, id: &str) {
        let mut state = self.state.lock().unwrap();
        if let Some(task) = state.active.remove(id) {
            if task.mutation {
                state.mutations -= 1;
            } else {
                state.reads -= 1;
            }
        }
    }

    async fn invoke(self: Arc<Self>, id: String, request: ClientCommand) {
        let name = request
            .fields
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let executed = Arc::new(AtomicBool::new(false));
        let mutation_sent = Arc::new(AtomicBool::new(false));
        let mut completed = false;
        let mut code: u8 = 2;
        let mut registered = false;
        let mut signal = Signal::never();
        let result: Result<(Value, String), (Failure, Option<String>)> = async {
            let command = by_name(&name).ok_or_else(|| (protocol("unknown_command", "Unknown catalog command."), None))?;
            let is_read = read_only(command);
            let flags_value = request.fields.get("flags").cloned().unwrap_or(json!({}));
            let has_output = flags_value.as_object().is_some_and(|f| f.contains_key("output"));
            if request.op == "read" && (!is_read || has_output) {
                return Err((protocol("read_only", "Read operations admit GET commands and projections without file output only."), None));
            }
            let args_value = request.fields.get("args").cloned().unwrap_or(json!([]));
            let args: Option<Vec<String>> = args_value.as_array().and_then(|items| items.iter().map(|v| v.as_str().map(str::to_string)).collect());
            let (Some(args), Some(flags)) = (args, flags_value.as_object().cloned()) else {
                return Err((Failure::Usage("Arguments must be strings and flags an object.".into()), None));
            };
            let document = match request.fields.get("stdin_document") {
                None => None,
                Some(Value::String(text)) => Some(text.clone()),
                Some(_) => return Err((Failure::Usage("stdin_document must be text.".into()), None)),
            };
            let mut input = Input { args, flags: Vec::new() };
            for (flag, value) in &flags {
                let Some(definition) = command.flag(flag).filter(|_| flag != "acknowledge-hash") else {
                    return Err((Failure::Usage("Unknown or non-interactive-only flag. Approval requires a fresh typed hash prompt.".into()), None));
                };
                let parsed = if definition.boolean {
                    value.as_bool().map(FlagValue::Bool)
                } else if let Some(text) = value.as_str() {
                    Some(FlagValue::Str(text.into()))
                } else if definition.repeat {
                    value.as_array().and_then(|items| items.iter().map(|v| v.as_str().map(str::to_string)).collect::<Option<Vec<_>>>()).map(FlagValue::List)
                } else {
                    None
                };
                let Some(parsed) = parsed else {
                    return Err((Failure::Usage(format!("Invalid value type for --{flag}.")), None));
                };
                input.flags.push((flag.clone(), parsed));
            }
            let mutation = !is_read || has_output;
            let task_signal = self.reserve(&id, mutation).map_err(|f| (f, None))?;
            registered = true;
            signal = task_signal.clone();
            let io = BridgeIo { service: self.clone(), request: id.clone(), command: command.name.clone(), signal: task_signal.clone(), document, mutation_sent: mutation_sent.clone() };
            let client = self.client().map_err(|f| (f, None))?.for_bridge(mutation_sent.clone());
            if task_signal.aborted() {
                return Err((Failure::Aborted, None));
            }
            // The shared core (crate::cli::execute) with the run marked as executed.
            let expected = command.args.len();
            let outcome: Result<Option<J>, Failure> = async {
                if input.args.len() < expected {
                    return Err(Failure::Usage(format!("{} needs {expected} argument(s). Usage: {}", command.name, usage(command))));
                }
                if input.args.len() > expected && !command.rest {
                    return Err(Failure::Usage(format!("{} takes {expected} argument(s); unexpected: {}. Usage: {}", command.name, input.args[expected..].join(" "), usage(command))));
                }
                if let Some(question) = &command.confirm {
                    if !io.confirm(question.clone()).await? {
                        return Ok(None);
                    }
                }
                executed.store(true, Ordering::SeqCst);
                commands::run(command, &client, &input, &io).await.map(Some)
            }
            .await;
            if task_signal.aborted() {
                return Err((Failure::Aborted, None));
            }
            let value = match outcome {
                Ok(value) => {
                    code = 0;
                    if value.is_some() {
                        completed = true;
                    }
                    // A result that marks the command failed (an Orbit run with a failed or blocked
                    // repository) exits 1 without a refusal, as the original bridge reported it.
                    match commands::failed(command, value.as_ref().unwrap_or(&J::Undefined)) {
                        Ok(false) => {}
                        Ok(true) => {
                            code = 1;
                            return Err((Failure::error("The command failed."), None));
                        }
                        Err(failure) => {
                            code = 1;
                            let stderr = format!("{}\n", crate::cli::refusal_text(command, &failure));
                            let stderr = crate::js::slice16(&plain(&stderr), 0, Some(DISPLAY as isize));
                            return Err((failure, Some(stderr)));
                        }
                    }
                    value
                }
                Err(failure) => {
                    let stderr = match &failure {
                        Failure::Usage(message) => {
                            code = 2;
                            format!("{message}\n")
                        }
                        other => {
                            code = 1;
                            format!("{}\n", crate::cli::refusal_text(command, other))
                        }
                    };
                    let stderr = crate::js::slice16(&plain(&stderr), 0, Some(DISPLAY as isize));
                    return Err((failure, Some(stderr)));
                }
            };
            let formatted = match &value {
                None => "Cancelled.".to_string(),
                Some(value) => match format(&command.id, value, false) {
                    Ok(text) => plain(&text),
                    Err(_) => return Err((protocol("invalid_result", "The command completed, but its response cannot be formatted. Inspect authoritative state before repeating an action."), None)),
                },
            };
            Ok((value.map(|v| v.to_serde()).unwrap_or(Value::Null), formatted))
        }
        .await;
        match result {
            Ok((value, formatted)) => self.emit(
                &id,
                EventKind::Result,
                json!({"value": value, "formatted": formatted, "code": 0}),
            ),
            Err((failure, stderr)) => {
                let transport = matches!(failure, Failure::Type(_) | Failure::Aborted)
                    || matches!(&failure, Failure::Protocol { code, .. } if code == "output_backpressure" || code == "invalid_response");
                let outcome = if completed {
                    "completed"
                } else if registered && mutation_sent.load(Ordering::SeqCst) && transport {
                    "unknown"
                } else if registered && signal.aborted() {
                    "aborted"
                } else if executed.load(Ordering::SeqCst) {
                    "refused"
                } else {
                    "not_executed"
                };
                let failure_code = match &failure {
                    Failure::Protocol { code, .. } => code.clone(),
                    Failure::Aborted => "aborted".into(),
                    _ if code == 2 => "usage".into(),
                    _ => "command_failed".into(),
                };
                let message = stderr
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| describe_failure(&failure));
                let exit_code = if failure == Failure::Aborted {
                    1
                } else if code == 2 {
                    2
                } else {
                    1
                };
                self.error(&id, &failure_code, &message, outcome, exit_code);
            }
        }
        if registered {
            self.release(&id);
        }
    }

    async fn guide(self: Arc<Self>, id: String, request: ClientCommand) {
        let name = request
            .fields
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let flags = request.fields.get("flags").cloned().unwrap_or(json!({}));
        let mut registered = false;
        let mut signal = Signal::never();
        let result: Result<Value, Failure> = async {
            if name != "init" {
                return Err(protocol("unknown_command", "Unknown guide."));
            }
            let Some(map) = flags.as_object().filter(|m| {
                m.iter()
                    .all(|(k, v)| (k == "path" || k == "project") && v.is_string())
            }) else {
                return Err(Failure::Usage(
                    "The guide takes --path and --project as text.".into(),
                ));
            };
            let flags: Vec<(String, String)> = map
                .iter()
                .map(|(k, v)| (k.clone(), v.as_str().unwrap().to_string()))
                .collect();
            let task_signal = self.reserve(&id, true)?;
            registered = true;
            signal = task_signal.clone();
            let client = self.client()?;
            let ui = BridgeGuide {
                service: self.clone(),
                request: id.clone(),
                signal: task_signal.clone(),
            };
            let runner =
                crate::components::ComponentRunner::locate(&crate::components::install_root());
            let cwd = std::env::current_dir().unwrap_or_default();
            let program = std::env::var("LUUNA_CLI_NAME")
                .ok()
                .map(|n| crate::builders::js_trim(&n))
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| crate::help::DEFAULT_NAME.into());
            let outcome = crate::prime::prime(
                &client,
                &ui,
                &runner,
                &flags,
                &cwd,
                task_signal.clone(),
                &program,
            )
            .await?;
            let mut value = json!({"guide": name, "code": outcome.code});
            if let Some(project) = outcome.project {
                value["project"] = json!(project);
            }
            if let Some(ticket) = outcome.ticket {
                value["ticket"] = json!(ticket);
            }
            Ok(value)
        }
        .await;
        match result {
            Ok(value) => self.emit(
                &id,
                EventKind::Result,
                json!({"value": value, "formatted": "", "code": 0}),
            ),
            Err(failure) => {
                let (code, exit) = match &failure {
                    Failure::Protocol { code, .. } => (code.clone(), 2),
                    Failure::Aborted => ("aborted".into(), 1),
                    Failure::Usage(_) => ("usage".into(), 2),
                    _ => ("command_failed".into(), 1),
                };
                let transport = matches!(failure, Failure::Type(_) | Failure::Aborted);
                let outcome = if !registered {
                    "not_executed"
                } else if transport {
                    "unknown"
                } else if signal.aborted() {
                    "aborted"
                } else {
                    "refused"
                };
                self.error(&id, &code, &describe_failure(&failure), outcome, exit);
            }
        }
        if registered {
            self.release(&id);
        }
    }
}

/// A command's io inside the workbench: prompts become prompt events, notes and plans output events.
struct BridgeIo {
    service: Arc<Service>,
    request: String,
    command: String,
    signal: Signal,
    document: Option<String>,
    mutation_sent: Arc<AtomicBool>,
}

impl Io for BridgeIo {
    fn signal(&self) -> Signal {
        self.signal.clone()
    }
    fn interactive(&self) -> bool {
        true
    }
    fn read_file(&self, path: &str) -> Result<String, Failure> {
        std::fs::read(path)
            .map(|b| String::from_utf8_lossy(&b).into_owned())
            .map_err(|e| commands::fs_failure(&e, path, false))
    }
    fn write_file(&self, path: &str, text: &str) -> Result<(), Failure> {
        self.mutation_sent.store(true, Ordering::SeqCst);
        std::fs::write(path, text).map_err(|e| commands::fs_failure(&e, path, true))
    }
    fn read_stdin(&self) -> Fut<'_, Result<String, Failure>> {
        Box::pin(async move {
            match &self.document {
                Some(document) => Ok(document.clone()),
                None => {
                    self.ask_text("document", "Enter the complete document for standard input")
                        .await
                }
            }
        })
    }
    fn prompt(&self, question: String) -> Fut<'_, Result<String, Failure>> {
        let kind = if self.command == "approve" {
            "hash"
        } else {
            "text"
        };
        Box::pin(async move { self.ask_text(kind, &question).await })
    }
    fn confirm(&self, question: String) -> Fut<'_, Result<bool, Failure>> {
        Box::pin(async move {
            let answer = self
                .service
                .question(
                    &self.request,
                    "confirm",
                    EventKind::Prompt,
                    json!({"kind": "confirm", "question": plain(&question)}),
                    None,
                    &self.signal,
                )
                .await?;
            Ok(answer.as_bool().unwrap_or(false))
        })
    }
    fn show(&self, text: &str) -> Result<(), Failure> {
        let clean = self.service.redact_text(&plain(text));
        if self.command == "approve" && clean.len() > DISPLAY {
            return Err(protocol(
                "plan_too_large",
                "The complete plan cannot fit in the review window. Approval is disabled.",
            ));
        }
        self.service.emit(
            &self.request,
            EventKind::Output,
            json!({"kind": "plan", "text": clean}),
        );
        Ok(())
    }
    fn note(&self, text: &str) {
        let clean = self.service.redact_text(&plain(text));
        self.service.emit(
            &self.request,
            EventKind::Output,
            json!({"kind": "note", "text": clean}),
        );
    }
    fn emit(&self, event: &J) {
        self.service.emit(
            &self.request,
            EventKind::Event,
            json!({"record": event.to_serde()}),
        );
    }
}

impl BridgeIo {
    async fn ask_text(&self, kind: &'static str, question: &str) -> Result<String, Failure> {
        let answer = self
            .service
            .question(
                &self.request,
                kind,
                EventKind::Prompt,
                json!({"kind": kind, "question": plain(question)}),
                None,
                &self.signal,
            )
            .await?;
        Ok(answer.as_str().unwrap_or("").to_string())
    }
}

/// The guide's frames for the workbench's guide mode.
struct BridgeGuide {
    service: Arc<Service>,
    request: String,
    signal: Signal,
}

impl GuideUi for BridgeGuide {
    fn send(&self, frame: Value) {
        self.service.emit(&self.request, EventKind::Guide, frame);
    }
    fn ask(&self, kind: &'static str, frame: Value) -> Fut<'_, Result<Value, Failure>> {
        Box::pin(async move {
            let choices = frame.get("options").and_then(Value::as_array).map(Vec::len);
            self.service
                .question(
                    &self.request,
                    kind,
                    EventKind::Guide,
                    frame,
                    choices,
                    &self.signal,
                )
                .await
        })
    }
}

/// The workbench's handle on the service: send requests, receive events, shut down.
pub struct Bridge {
    service: Arc<Service>,
    requests: Option<mpsc::UnboundedSender<ClientCommand>>,
    receiver: mpsc::UnboundedReceiver<Result<BridgeEvent, String>>,
    tasks: Vec<JoinHandle<()>>,
}

impl Bridge {
    /// Starts the service for `token` and the runtime at `url` (the default when None).
    pub fn start(token: &str, url: Option<&str>) -> Bridge {
        let (events, receiver) = mpsc::unbounded_channel();
        let mut secrets: Vec<String> = Vec::new();
        if !token.is_empty() {
            let escaped = |text: &str| {
                let quoted = J::str(text).stringify().unwrap_or_default();
                quoted[1..quoted.len() - 1].to_string()
            };
            for secret in [
                token.to_string(),
                plain(token),
                escaped(token),
                escaped(&plain(token)),
            ] {
                if !secret.is_empty() && !secrets.contains(&secret) {
                    secrets.push(secret);
                }
            }
            secrets.sort_by_key(|s| std::cmp::Reverse(s.len()));
        }
        let service = Arc::new(Service {
            token: token.to_string(),
            url: url.map(str::to_string),
            secrets,
            state: Mutex::new(State::default()),
            events,
        });
        let (requests, mut incoming) = mpsc::unbounded_channel::<ClientCommand>();
        let dispatcher = {
            let service = service.clone();
            tokio::spawn(async move {
                while let Some(request) = incoming.recv().await {
                    let control = matches!(
                        request.op.as_str(),
                        "catalog" | "reply" | "abort" | "shutdown"
                    );
                    let service = service.clone();
                    if control {
                        service.dispatch(request).await;
                    } else {
                        tokio::spawn(service.dispatch(request));
                    }
                }
            })
        };
        let health = {
            let service = service.clone();
            tokio::spawn(async move {
                let mut tick =
                    tokio::time::interval_at(tokio::time::Instant::now() + HEALTH, HEALTH);
                loop {
                    tick.tick().await;
                    if service.state.lock().unwrap().closed {
                        break;
                    }
                    let service = service.clone();
                    tokio::spawn(async move { service.health().await });
                }
            })
        };
        Bridge {
            service,
            requests: Some(requests),
            receiver,
            tasks: vec![dispatcher, health],
        }
    }

    pub fn send(&self, request: &ClientCommand) -> Result<(), String> {
        request.encode()?;
        self.requests
            .as_ref()
            .ok_or("Bridge is closed.")?
            .send(request.clone())
            .map_err(|_| "Bridge input queue is full or closed; request was not sent.".into())
    }
    pub fn try_recv(&mut self) -> Option<Result<BridgeEvent, String>> {
        self.receiver.try_recv().ok()
    }
    pub async fn recv(&mut self) -> Option<Result<BridgeEvent, String>> {
        self.receiver.recv().await
    }
    pub async fn shutdown(&mut self) {
        self.service.close();
        self.requests.take();
        for task in self.tasks.drain(..) {
            task.abort();
            let _ = task.await;
        }
    }
}

impl Drop for Bridge {
    fn drop(&mut self) {
        self.service.close();
        for task in &self.tasks {
            task.abort();
        }
    }
}
