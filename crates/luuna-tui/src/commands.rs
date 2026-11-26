//! One command per catalog entry (schemas/capability-catalog.json), the two read-only projections
//! and the setup guide. The table itself (names, arguments, flags, descriptions, prompts and the
//! parity examples) is data in `commands.json`, which scripts/check-contracts.mjs also reads; the
//! behaviour of each command is here, ported statement by statement from the original client so
//! the requests, refusals and outputs are unchanged.
//!
//! Adding a product action: add its catalog entry, then one object in commands.json whose `id` is
//! the catalog id and whose `name` is the catalog's `tui` command, and its arm in `run`.
use crate::builders::{self, js_trim, Override, TaskDraft};
use crate::client::{encode_component, form_encode, Client, Signal};
use crate::failures::{describe_failure, Failure};
use crate::js::{self, safe_integer, J};
use crate::obj;
use std::future::Future;
use std::pin::Pin;
use std::sync::OnceLock;

pub type Fut<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Text limits the runtime enforces, in UTF-8 bytes (the same table as the desktop).
pub const LIMIT_TITLE: usize = 200;
pub const LIMIT_INTENT: usize = 16_000;
pub const LIMIT_CRITERION: usize = 16_000;
pub const LIMIT_APPROVER: usize = 200;

/// The command table and the capability catalog, parsed once.
pub struct Table {
    pub commands: Vec<Command>,
    pub projections: Vec<Command>,
    pub guides: Vec<Command>,
    pub catalog: J,
    pub catalog_text: &'static str,
}

#[derive(Clone, Debug)]
pub struct Arg {
    pub name: String,
    pub description: Option<String>,
    pub prompt: Option<String>,
    pub context: bool,
}

#[derive(Clone, Debug)]
pub struct Flag {
    pub description: Option<String>,
    pub prompt: Option<String>,
    pub boolean: bool,
    pub repeat: bool,
    pub multiline: bool,
}

#[derive(Clone, Debug)]
pub struct Command {
    pub id: String,
    pub name: String,
    pub action: String,
    pub context: String,
    pub title: Option<String>,
    pub detail: Option<String>,
    pub confirm: Option<String>,
    pub args: Vec<Arg>,
    pub flags: Vec<(String, Flag)>,
    /// Takes every word after its own arguments verbatim (`sbx <project> pi --model x`).
    pub rest: bool,
    /// How a result marks the command failed (exit 1): `orbit` when a repository failed or was blocked.
    pub failed: Option<String>,
    /// The parity example: positional arguments and flags.
    pub example: J,
    /// Projections: the catalog ids they read. Guides: the catalog ids they run.
    pub projects: Vec<String>,
    pub uses: Vec<String>,
    /// The command's own entry in commands.json, for the bridge's catalog.
    pub raw: J,
}

impl Command {
    pub fn flag(&self, name: &str) -> Option<&Flag> {
        self.flags.iter().find(|(n, _)| n == name).map(|(_, f)| f)
    }
    pub fn is_projection(&self) -> bool {
        self.id.starts_with("projection.")
    }
    pub fn is_guide(&self) -> bool {
        self.id.starts_with("guide.")
    }
}

/// The ticket types Missions accepts, and so the runtime (ADR 0058).
const TICKET_TYPES: [&str; 4] = ["task", "bug", "feature", "epic"];

/// `0`..`4` or `P0`..`P4`.
pub(crate) fn ticket_priority(value: &str) -> Option<u8> {
    let value = js_trim(value);
    let digits = value.strip_prefix(['P', 'p']).unwrap_or(&value);
    (digits.len() == 1)
        .then(|| digits.parse::<u8>().ok())
        .flatten()
        .filter(|p| *p <= 4)
}

/// `--type`, `--priority` and `--label` (and `--clear-labels` on an edit) as
/// the runtime's `type`, `priority` and `labels` (ADR 0058). Empty values keep
/// the default on a create and the current value on an edit. Labels are
/// lowercased, as Missions stores them.
fn ticket_triage(input: &Input, create: bool) -> Result<Vec<(&'static str, J)>, Failure> {
    let mut fields = Vec::new();
    if let Some(kind) = input.text_if("type").filter(|k| !js_trim(k).is_empty()) {
        let kind = js_trim(&kind).to_lowercase();
        if !TICKET_TYPES.contains(&kind.as_str()) {
            return Err(usage_error("--type is task, bug, feature or epic."));
        }
        if !(create && kind == "task") {
            fields.push(("type", J::str(kind)));
        }
    }
    if let Some(priority) = input.text_if("priority").filter(|p| !js_trim(p).is_empty()) {
        let Some(priority) = ticket_priority(&priority) else {
            return Err(usage_error(
                "--priority is P0 (critical) to P4 (backlog), or 0 to 4.",
            ));
        };
        if !(create && priority == 2) {
            fields.push(("priority", J::Num(f64::from(priority))));
        }
    }
    let labels: Vec<String> = input
        .list("label")
        .iter()
        .flat_map(|item| item.split(','))
        .map(|label| js_trim(label).to_lowercase())
        .filter(|label| !label.is_empty())
        .fold(Vec::new(), |mut seen, label| {
            if !seen.contains(&label) {
                seen.push(label);
            }
            seen
        });
    if let Some(bad) = labels.iter().find(|l| {
        l.len() > 64
            || l.chars().any(|c| c.is_whitespace() || c.is_control())
            || l.starts_with("luuna-ticket-")
    }) {
        return Err(usage_error(format!(
            "The label {bad:?} is not allowed: use at most 64 bytes without spaces, and not the luuna-ticket- prefix."
        )));
    }
    if labels.len() > 16 {
        return Err(usage_error("A ticket takes at most 16 labels."));
    }
    if !create && input.truthy("clear-labels") {
        if !labels.is_empty() {
            return Err(usage_error(
                "--clear-labels and --label cannot be combined.",
            ));
        }
        fields.push(("labels", J::Arr(vec![])));
    } else if !labels.is_empty() {
        fields.push(("labels", J::Arr(labels.into_iter().map(J::Str).collect())));
    }
    Ok(fields)
}

fn strings(value: &J) -> Vec<String> {
    value.items().iter().map(J::js).collect()
}

fn command_of(raw: &J) -> Command {
    let text = |key: &str| raw.get(key).as_str().map(str::to_string);
    Command {
        id: raw.get("id").js(),
        name: raw.get("name").js(),
        action: raw.get("action").js(),
        context: raw.get("context").js(),
        title: text("title"),
        detail: text("detail"),
        confirm: text("confirm"),
        args: raw
            .get("args")
            .items()
            .iter()
            .map(|arg| Arg {
                name: arg.get("name").js(),
                description: arg.get("description").as_str().map(str::to_string),
                prompt: arg.get("prompt").as_str().map(str::to_string),
                context: arg.get("context") == &J::Bool(true),
            })
            .collect(),
        flags: raw
            .get("flags")
            .as_obj()
            .map(|entries| {
                entries
                    .iter()
                    .map(|(name, flag)| {
                        (
                            name.clone(),
                            Flag {
                                description: flag.get("description").as_str().map(str::to_string),
                                prompt: flag.get("prompt").as_str().map(str::to_string),
                                boolean: flag.get("boolean") == &J::Bool(true),
                                repeat: flag.get("repeat") == &J::Bool(true),
                                multiline: flag.get("multiline") == &J::Bool(true),
                            },
                        )
                    })
                    .collect()
            })
            .unwrap_or_default(),
        rest: raw.get("rest") == &J::Bool(true),
        failed: text("failed"),
        example: raw.get("example").clone(),
        projects: strings(raw.get("projects")),
        uses: strings(raw.get("uses")),
        raw: raw.clone(),
    }
}

pub fn table() -> &'static Table {
    static TABLE: OnceLock<Table> = OnceLock::new();
    TABLE.get_or_init(|| {
        let data =
            js::parse(include_str!("../commands.json")).expect("commands.json is valid JSON");
        let catalog_text = include_str!("../../../schemas/capability-catalog.json");
        Table {
            commands: data
                .get("commands")
                .items()
                .iter()
                .map(command_of)
                .collect(),
            projections: data
                .get("projections")
                .items()
                .iter()
                .map(command_of)
                .collect(),
            guides: data.get("guides").items().iter().map(command_of).collect(),
            catalog: js::parse(catalog_text).expect("the capability catalog is valid JSON"),
            catalog_text,
        }
    })
}

/// Every command the command line reaches: the cataloged actions and the projections.
pub fn by_name(name: &str) -> Option<&'static Command> {
    let table = table();
    table
        .commands
        .iter()
        .chain(table.projections.iter())
        .find(|c| c.name == name)
}

pub fn by_id(id: &str) -> Option<&'static Command> {
    table().commands.iter().find(|c| c.id == id)
}

/// The catalog entry of an action.
pub fn capability(id: &str) -> &'static J {
    table()
        .catalog
        .get("capabilities")
        .items()
        .iter()
        .find(|c| c.get("id").as_str() == Some(id))
        .unwrap_or(&js::UNDEFINED)
}

/// A read: a projection, or an action whose route is a GET.
pub fn read_only(command: &Command) -> bool {
    // memory-starter lists with GET but `--keep` saves facts with POST, so it
    // is admitted, limited and never replayed as a mutation.
    command.is_projection()
        || (capability(&command.id).get("method").as_str() == Some("GET")
            && !COMPOSITE_WRITES.contains(&command.id.as_str()))
}

/// Commands whose catalog capability reads but that can also write.
const COMPOSITE_WRITES: [&str; 1] = ["memory.starter"];

/// The positional usage line of a command.
pub fn usage(command: &Command) -> String {
    let positional = command
        .args
        .iter()
        .map(|a| format!("<{}>", a.name))
        .collect::<Vec<_>>()
        .join(" ");
    let flags = command
        .flags
        .iter()
        .map(|(name, flag)| {
            if flag.boolean {
                format!("[--{name}]")
            } else {
                format!("[--{name} <value>{}]", if flag.repeat { "…" } else { "" })
            }
        })
        .collect::<Vec<_>>()
        .join(" ");
    let mut line = command.name.clone();
    if !positional.is_empty() {
        line.push(' ');
        line.push_str(&positional);
    }
    if !flags.is_empty() {
        line.push(' ');
        line.push_str(&flags);
    }
    line
}

/// A flag's value as the command line or the bridge gave it.
#[derive(Clone, Debug, PartialEq)]
pub enum FlagValue {
    Bool(bool),
    Str(String),
    List(Vec<String>),
}

impl FlagValue {
    /// JavaScript truthiness of the value.
    pub fn truthy(&self) -> bool {
        match self {
            FlagValue::Bool(b) => *b,
            FlagValue::Str(s) => !s.is_empty(),
            FlagValue::List(_) => true,
        }
    }
    /// `String(value)`.
    pub fn text(&self) -> String {
        match self {
            FlagValue::Bool(b) => b.to_string(),
            FlagValue::Str(s) => s.clone(),
            FlagValue::List(items) => items.join(","),
        }
    }
    pub fn to_j(&self) -> J {
        match self {
            FlagValue::Bool(b) => J::Bool(*b),
            FlagValue::Str(s) => J::str(s.clone()),
            FlagValue::List(items) => J::Arr(items.iter().cloned().map(J::Str).collect()),
        }
    }
}

/// Positional arguments and flags, in the order they were given.
#[derive(Clone, Debug, Default)]
pub struct Input {
    pub args: Vec<String>,
    pub flags: Vec<(String, FlagValue)>,
}

impl Input {
    pub fn flag(&self, name: &str) -> Option<&FlagValue> {
        self.flags.iter().find(|(n, _)| n == name).map(|(_, v)| v)
    }
    pub fn truthy(&self, name: &str) -> bool {
        self.flag(name).is_some_and(FlagValue::truthy)
    }
    /// `flags[name] ? String(flags[name]) : undefined`
    pub fn text_if(&self, name: &str) -> Option<String> {
        self.flag(name).filter(|v| v.truthy()).map(FlagValue::text)
    }
    pub fn set(&mut self, name: &str, value: FlagValue) {
        match self.flags.iter_mut().find(|(n, _)| n == name) {
            Some(entry) => entry.1 = value,
            None => self.flags.push((name.to_string(), value)),
        }
    }
    /// `list(flags[name])`: undefined is empty, an array is itself, anything else one item.
    pub fn list(&self, name: &str) -> Vec<String> {
        match self.flag(name) {
            None => Vec::new(),
            Some(FlagValue::List(items)) => items.clone(),
            Some(other) => vec![other.text()],
        }
    }
}

/// What a command may ask of its environment: the command line and the workbench's bridge differ.
pub trait Io: Send + Sync {
    fn signal(&self) -> Signal;
    fn interactive(&self) -> bool;
    fn read_file(&self, path: &str) -> Result<String, Failure>;
    fn write_file(&self, path: &str, text: &str) -> Result<(), Failure>;
    fn read_stdin(&self) -> Fut<'_, Result<String, Failure>>;
    fn prompt(&self, question: String) -> Fut<'_, Result<String, Failure>>;
    fn confirm(&self, question: String) -> Fut<'_, Result<bool, Failure>>;
    fn show(&self, text: &str) -> Result<(), Failure>;
    fn note(&self, text: &str);
    fn emit(&self, event: &J);
    /// Whether this environment can open a sandbox with the terminal attached (`lu sbx`); the
    /// workbench cannot, so there the command returns the plan.
    fn can_launch(&self) -> bool {
        false
    }
    /// Opens the sandbox a plan describes and resolves with the sandboxed command's exit code.
    fn launch(
        &self,
        _plan: J,
        _command: Vec<String>,
        _grants: Vec<String>,
    ) -> Fut<'_, Result<i32, Failure>> {
        Box::pin(async { Err(Failure::error("The sandbox needs a terminal.")) })
    }
    /// The exit code of the sandboxed command, once `launch` ran; it becomes the client's.
    fn sandbox_exit(&self) -> Option<i32> {
        None
    }
}

/// An Orbit run in which some repository failed or was blocked exits 1, so scripts can chain on it:
/// `(execution?.results ?? []).some((result) => result.status === "failed" || result.status === "blocked")`.
pub fn orbit_failed(execution: &J) -> Result<bool, Failure> {
    match execution.get("results") {
        J::Undefined | J::Null => Ok(false),
        J::Arr(results) => {
            for result in results {
                if result.nullish() {
                    return Err(Failure::type_error());
                }
                if matches!(result.get("status").as_str(), Some("failed" | "blocked")) {
                    return Ok(true);
                }
            }
            Ok(false)
        }
        _ => Err(Failure::type_error()),
    }
}

/// Whether a command's result marks it failed (`command.failed?.(result)`).
pub fn failed(command: &Command, result: &J) -> Result<bool, Failure> {
    match command.failed.as_deref() {
        Some("orbit") => orbit_failed(result),
        _ => Ok(false),
    }
}

/// `orbitCommand(line)`: splits a command line typed by the developer into executable and
/// arguments as `/"([^"]*)"|'([^']*)'|(\S+)/g` does (quotes group words); no shell runs it.
pub fn orbit_command(line: &str) -> Result<(String, Vec<String>), String> {
    let chars: Vec<char> = line.chars().collect();
    let space = |c: char| c.is_whitespace() || c == '\u{feff}';
    let mut words = Vec::new();
    let mut at = 0;
    while at < chars.len() {
        let c = chars[at];
        if c == '"' || c == '\'' {
            if let Some(close) = chars[at + 1..].iter().position(|&d| d == c) {
                words.push(chars[at + 1..at + 1 + close].iter().collect::<String>());
                at += close + 2;
                continue;
            }
        }
        if space(c) {
            at += 1;
            continue;
        }
        let start = at;
        while at < chars.len() && !space(chars[at]) {
            at += 1;
        }
        words.push(chars[start..at].iter().collect::<String>());
    }
    if words.is_empty() {
        return Err("Type a command to run in each repository.".into());
    }
    let executable = words.remove(0);
    Ok((executable, words))
}

/// `--repository` flags as the repository ids an Orbit request selects.
fn selected_repositories(input: &Input, request: &mut J) {
    let selected = input.list("repository");
    if !selected.is_empty() {
        request.set(
            "repositories",
            J::Arr(selected.into_iter().map(J::Str).collect()),
        );
    }
}

/// Node's messages for the file errors the client meets (`ENOENT: no such file or directory, open 'x'`).
pub fn fs_failure(error: &std::io::Error, path: &str, write: bool) -> Failure {
    let (code, text) = match error.kind() {
        std::io::ErrorKind::NotFound => ("ENOENT", "no such file or directory"),
        std::io::ErrorKind::PermissionDenied => ("EACCES", "permission denied"),
        std::io::ErrorKind::IsADirectory => ("EISDIR", "illegal operation on a directory"),
        std::io::ErrorKind::AlreadyExists => ("EEXIST", "file already exists"),
        _ => ("EIO", "i/o error"),
    };
    if code == "EISDIR" && !write {
        return Failure::error(format!("{code}: {text}, read"));
    }
    // On POSIX, Node names the path as given.
    if !cfg!(windows) {
        return Failure::error(format!("{code}: {text}, open '{path}'"));
    }
    // On Windows it names the resolved absolute path, with `.` and `..` removed.
    let resolved = std::path::absolute(path).map(|p| {
        let mut out = std::path::PathBuf::new();
        for component in p.components() {
            match component {
                std::path::Component::CurDir => {}
                std::path::Component::ParentDir => {
                    out.pop();
                }
                other => out.push(other.as_os_str()),
            }
        }
        out.to_string_lossy().into_owned()
    });
    Failure::error(format!(
        "{code}: {text}, open '{}'",
        resolved.unwrap_or_else(|_| path.to_string())
    ))
}

/// A random version 4 UUID (`crypto.randomUUID()`), seeded by the OS through `RandomState`.
pub fn random_uuid() -> String {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let word = || {
        let mut hasher = RandomState::new().build_hasher();
        hasher.write_u64(COUNTER.fetch_add(1, Ordering::Relaxed));
        hasher.write_u128(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0),
        );
        hasher.finish()
    };
    let mut bytes = [0u8; 16];
    bytes[..8].copy_from_slice(&word().to_le_bytes());
    bytes[8..].copy_from_slice(&word().to_le_bytes());
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    )
}

fn usage_error(message: impl Into<String>) -> Failure {
    Failure::Usage(message.into())
}

fn refusal(message: impl Into<String>) -> Failure {
    Failure::Refusal(message.into())
}

/// `integer(value, name, { min })`: None when absent; a UsageError when not an integer ≥ min.
fn integer(value: Option<&FlagValue>, name: &str, min: f64) -> Result<Option<f64>, Failure> {
    let Some(value) = value else { return Ok(None) };
    let number = match value {
        FlagValue::Bool(b) => f64::from(u8::from(*b)),
        FlagValue::Str(s) => js::string_to_number(s),
        FlagValue::List(items) => J::Arr(items.iter().cloned().map(J::Str).collect()).to_number(),
    };
    if !safe_integer(number) || number < min {
        return Err(usage_error(format!(
            "--{name} must be an integer of at least {}.",
            js::number_to_string(min)
        )));
    }
    Ok(Some(number))
}

/// `integer()` of a positional argument (always present).
fn integer_arg(value: &str, name: &str, min: f64) -> Result<f64, Failure> {
    Ok(integer(Some(&FlagValue::Str(value.to_string())), name, min)?.unwrap())
}

fn model_settings(input: &Input, request: &mut J) {
    if let Some(provider) = input.text_if("provider") {
        request.set("provider", J::str(provider));
    }
    if let Some(model) = input.text_if("model") {
        request.set("model", J::str(model));
    }
}

fn key(input: &Input) -> String {
    input.text_if("key").unwrap_or_else(random_uuid)
}

fn bytes(text: &str) -> usize {
    text.len()
}

/// `value.field` on a value that must be an object-like value (null and undefined throw).
fn field<'a>(value: &'a J, key: &str) -> Result<&'a J, Failure> {
    if value.nullish() {
        Err(Failure::type_error())
    } else {
        Ok(value.get(key))
    }
}

/// `value.length` where value must not be nullish.
fn length(value: &J) -> Result<J, Failure> {
    match value {
        J::Arr(items) => Ok(J::from(items.len())),
        J::Str(s) => Ok(J::from(js::len16(s))),
        J::Undefined | J::Null => Err(Failure::type_error()),
        _ => Ok(J::Undefined),
    }
}

/// Reads a document argument: a file path, or `-` for standard input.
async fn document(io: &dyn Io, source: &str) -> Result<String, Failure> {
    if source == "-" {
        io.read_stdin().await
    } else {
        io.read_file(source)
    }
}

/// `pageQuery(page, separator)`.
fn page_query(limit: Option<f64>, separator: &str) -> Result<String, Failure> {
    match limit {
        None => Ok(String::new()),
        Some(limit) => {
            if !safe_integer(limit) || !(1.0..=crate::client::MAX_PAGE).contains(&limit) {
                return Err(Failure::error("limit must be an integer from 1 to 10000."));
            }
            Ok(format!("{separator}limit={}", js::number_to_string(limit)))
        }
    }
}

/// `readEventPages(read, after, limit)`: pages from a cursor until a page comes back empty.
async fn read_event_pages(
    client: &Client,
    base: &str,
    after: f64,
    limit: f64,
    signal: &Signal,
) -> Result<J, Failure> {
    let mut events = Vec::new();
    let mut cursor = after;
    loop {
        if !safe_integer(cursor) || cursor < 0.0 {
            return Err(Failure::error("Invalid event cursor."));
        }
        let page = client
            .get(
                &format!(
                    "{base}?after={}{}",
                    js::number_to_string(cursor),
                    page_query(Some(limit), "&")?
                ),
                signal,
            )
            .await?;
        let items = match &page {
            J::Arr(items) => items.clone(),
            J::Str(s) => s.chars().map(|c| J::str(c.to_string())).collect(),
            _ => return Err(Failure::type_error()),
        };
        events.extend(items.iter().cloned());
        let last = items
            .last()
            .map(|item| item.get("sequence").clone())
            .unwrap_or(J::Undefined);
        if last.is_undefined() {
            return Ok(J::Arr(events));
        }
        // `last <= cursor` compares like JavaScript; a non-numeric cursor fails the next page's check.
        let number = last.to_number();
        if number <= cursor {
            return Ok(J::Arr(events));
        }
        if !matches!(last, J::Num(_)) {
            return Err(Failure::error("Invalid event cursor."));
        }
        cursor = number;
    }
}

async fn candidate_hash(
    client: &Client,
    run: &str,
    input: &Input,
    signal: &Signal,
) -> Result<J, Failure> {
    let typed = input
        .flag("candidate-hash")
        .map(|value| js_trim(&value.text()));
    if let Some(value) = &typed {
        if full_hash(&J::str(value.clone())) {
            return Ok(J::str(value.clone()));
        }
    }
    let view = client
        .get(
            &format!("/runs/{}/candidate", encode_component(run)),
            signal,
        )
        .await?;
    let evidence = field(&view, "evidence")?;
    if !evidence.truthy() {
        return Err(refusal(
            "No candidate is recorded for this run yet; the writer has not settled.",
        ));
    }
    let current = field(field(evidence, "candidate")?, "hash")?.clone();
    // A typed prefix (ADR 0042) acknowledges only the current candidate it begins; anything else
    // is sent as typed, and the runtime refuses a hash that is not the current candidate's.
    Ok(match typed {
        Some(value) => J::str(crate::refs::complete_hash(&value, &current.js())),
        None => current,
    })
}

/// /^sha256:[a-f0-9]{64}$/ on `String(value)`.
fn full_hash(value: &J) -> bool {
    let text = value.js();
    text.strip_prefix("sha256:").is_some_and(|h| {
        h.len() == 64
            && h.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}

/// `value.startsWith("sha256:")`, which throws on a non-string.
fn starts_with_sha(value: &J, expr: &str) -> Result<bool, Failure> {
    match value {
        J::Str(s) => Ok(s.starts_with("sha256:")),
        other => Err(builders::not_callable(other, expr, "startsWith")),
    }
}

fn format_plan(plan: &J) -> String {
    format!(
        "Plan {} revision {}\nImmutable plan hash: {}\n{}",
        plan.get("id").js(),
        plan.get("revision").js(),
        plan.get("hash").js(),
        plan.stringify_pretty()
            .unwrap_or_else(|| "undefined".into())
    )
}

/// The exact hash the developer acknowledged, from `--acknowledge-hash` or typed interactively.
async fn acknowledged_hash(io: &dyn Io, input: &Input, plan: &J) -> Result<String, Failure> {
    if let Some(value) = input.flag("acknowledge-hash") {
        return Ok(js_trim(&value.text()));
    }
    if !io.interactive() {
        return Err(usage_error(format!(
            "Approval requires the plan hash. Review plan {} r{} (show-plan {}), then approve it with --acknowledge-hash {} (its first 8 hash digits are enough)",
            field(plan, "id")?.js(),
            plan.get("revision").js(),
            plan.get("id").js(),
            plan.get("hash").js()
        )));
    }
    io.show(&format_plan(plan))?;
    let hash = plan.get("hash").js();
    let typed = js_trim(
        &io.prompt(approval_question(
            &plan.get("id").js(),
            &plan.get("revision").js(),
            &hash,
        ))
        .await?,
    );
    // The typed word stands for the complete hash displayed above, as the desktop's checkbox does
    // for the hash beside it (ADR 0042); the exact hash read with the plan is what is sent.
    Ok(
        if typed.eq_ignore_ascii_case("approve") || typed.eq_ignore_ascii_case("yes") {
            hash
        } else {
            typed
        },
    )
}

/// `Math.round`.
fn round(value: f64) -> f64 {
    let floor = value.floor();
    if value - floor >= 0.5 {
        floor + 1.0
    } else {
        floor
    }
}

/// Every project with its view, baselines and tickets (four projects in flight at most).
async fn read_projects(
    client: &Client,
    only: Option<&str>,
    signal: &Signal,
) -> Result<(Vec<J>, Vec<(String, J)>, Vec<(String, J)>), Failure> {
    let listed = client.get("/projects", signal).await?;
    let all = listed.as_arr().ok_or_else(Failure::type_error)?;
    let mut projects = Vec::new();
    for project in all {
        if only.is_none() || field(project, "id")?.as_str() == only {
            projects.push(project.clone());
        }
    }
    if let Some(only) = only {
        if projects.is_empty() {
            return Err(refusal(format!("The runtime has no project {only}.")));
        }
    }
    let mut views = Vec::new();
    let mut baselines = Vec::new();
    let mut tickets = Vec::new();
    for chunk in projects.chunks(4) {
        let reads = chunk.iter().map(|project| async move {
            let id = field(project, "id")?.js();
            let encoded = encode_component(&id);
            let (view_path, baselines_path, tickets_path) = (
                format!("/projects/{encoded}"),
                format!("/projects/{encoded}/baselines"),
                format!("/projects/{encoded}/tickets"),
            );
            let (view, recorded, listed) = tokio::try_join!(
                client.get(&view_path, signal),
                client.get(&baselines_path, signal),
                client.get(&tickets_path, signal)
            )?;
            Ok::<_, Failure>((id, view, recorded, listed))
        });
        for result in futures_join_all(reads).await {
            let (id, view, recorded, listed) = result?;
            views.push(view);
            baselines.push((id.clone(), recorded));
            tickets.push((id, listed));
        }
    }
    Ok((views, baselines, tickets))
}

/// Awaits every future concurrently, keeping their order.
async fn futures_join_all<F: Future>(futures: impl IntoIterator<Item = F>) -> Vec<F::Output> {
    let handles: Vec<_> = futures.into_iter().map(Box::pin).collect();
    let mut outputs = Vec::with_capacity(handles.len());
    let mut pinned = handles;
    let mut slots: Vec<Option<F::Output>> = pinned.iter().map(|_| None).collect();
    std::future::poll_fn(|cx| {
        let mut pending = false;
        for (index, future) in pinned.iter_mut().enumerate() {
            if slots[index].is_none() {
                match future.as_mut().poll(cx) {
                    std::task::Poll::Ready(value) => slots[index] = Some(value),
                    std::task::Poll::Pending => pending = true,
                }
            }
        }
        if pending {
            std::task::Poll::Pending
        } else {
            std::task::Poll::Ready(())
        }
    })
    .await;
    for slot in slots {
        outputs.push(slot.expect("every future finished"));
    }
    outputs
}

/// Runs one command against the runtime; the result is what the command prints (formatted or JSON).
pub async fn run(
    command: &Command,
    client: &Client,
    input: &Input,
    io: &dyn Io,
) -> Result<J, Failure> {
    let signal = io.signal();
    let s = &signal;
    // Short ids and hash prefixes name exactly one record before anything is sent (ADR 0042).
    let resolved = crate::refs::resolve(command, client, input, s).await?;
    let input = &resolved;
    let a = |i: usize| input.args.get(i).cloned().unwrap_or_default();
    let e = |i: usize| encode_component(&a(i));
    match command.id.as_str() {
        "health" => client.get("/health", s).await,
        "doctor" => client.get("/doctor", s).await,
        "runs.list" => {
            if input.truthy("before") || input.truthy("project") {
                let mut pairs = vec![("limit", "200".to_string())];
                if let Some(before) = input.text_if("before") {
                    pairs.push(("before", before));
                }
                if let Some(project) = input.text_if("project") {
                    pairs.push(("project", project));
                }
                let borrowed: Vec<(&str, &str)> =
                    pairs.iter().map(|(k, v)| (*k, v.as_str())).collect();
                client
                    .get(&format!("/runs?{}", form_encode(&borrowed)), s)
                    .await
            } else {
                let limit = integer(input.flag("limit"), "limit", 1.0)?.unwrap_or(200.0);
                client
                    .get(&format!("/runs{}", page_query(Some(limit), "?")?), s)
                    .await
            }
        }
        "runs.start" => {
            let mut request = obj![("prompt", a(0))];
            model_settings(input, &mut request);
            client.post("/runs", Some(&request), s).await
        }
        "shutdown" => client.post("/shutdown", None, s).await,
        "projects.list" => client.get("/projects", s).await,
        "projects.register" => {
            let mut rows = vec![(a(1), a(2))];
            for spec in input.list("repository") {
                let spec = js_trim(&spec);
                // /^([^=]+)=(.+)$/
                let parsed = spec.split_once('=').filter(|(id, rest)| {
                    !id.is_empty()
                        && !rest.is_empty()
                        && !rest.contains(['\n', '\r', '\u{2028}', '\u{2029}'])
                });
                let Some((id, path)) = parsed else {
                    return Err(usage_error("--repository takes <id>=<absolute path>"));
                };
                rows.push((id.to_string(), path.to_string()));
            }
            let request =
                builders::register_project_request(&a(0), &rows, input.text_if("id").as_deref())
                    .map_err(|f| refusal(f.message()))?;
            client.post("/projects", Some(&request), s).await
        }
        "projects.scaffold.propose" => {
            client
                .post(
                    "/projects/scaffold/propose",
                    Some(&obj![
                        ("profile", a(0)),
                        ("name", a(1)),
                        ("parent_dir", a(2))
                    ]),
                    s,
                )
                .await
        }
        "projects.scaffold" => {
            let mut request = obj![
                ("proposal_hash", a(0)),
                ("idempotency_key", key(input)),
                ("confirm", true)
            ];
            if input.truthy("confirm-network") {
                request.set("confirm_network", J::Bool(true));
            }
            client.post("/projects/scaffold", Some(&request), s).await
        }
        "projects.show" => client.get(&format!("/projects/{}", e(0)), s).await,
        "projects.discover" => {
            client
                .post(&format!("/projects/{}/discover", e(0)), None, s)
                .await
        }
        "projects.manifest.confirm" => {
            let manifest = match document(io, &a(1))
                .await
                .ok()
                .and_then(|text| js::parse(&text).ok())
            {
                Some(value) => value,
                None => return Err(refusal("The manifest draft is not valid JSON.")),
            };
            if !manifest.is_obj() && !manifest.is_arr() {
                return Err(refusal("The manifest draft must be a JSON object."));
            }
            let revision = match integer(input.flag("revision"), "revision", 0.0)? {
                Some(revision) => J::Num(revision),
                None => field(
                    field(
                        &client.get(&format!("/projects/{}", e(0)), s).await?,
                        "project",
                    )?,
                    "revision",
                )?
                .clone(),
            };
            if !revision
                .as_f64()
                .is_some_and(|r| safe_integer(r) && r >= 0.0)
            {
                return Err(Failure::error("Invalid manifest revision."));
            }
            let body = obj![("expected_revision", revision), ("manifest", manifest)];
            client
                .request(
                    "PUT",
                    &format!("/projects/{}/manifest", e(0)),
                    Some(&body),
                    s,
                )
                .await
        }
        "projects.repositories.rebind" => {
            if !input.truthy("confirm") {
                return Err(refusal("Rebinding re-points plans, baselines and approvals to another checkout identity. Repeat with --confirm to state that the move or the new remote is intended."));
            }
            let body = match input.text_if("path") {
                Some(path) => obj![("local_path", path)],
                None => J::obj(),
            };
            client
                .post(
                    &format!("/projects/{}/repositories/{}/rebind", e(0), e(1)),
                    Some(&body),
                    s,
                )
                .await
        }
        "projects.export" => {
            let exported = client.get(&format!("/projects/{}/export", e(0)), s).await?;
            if let Some(output) = input.text_if("output") {
                io.write_file(
                    &output,
                    &format!(
                        "{}\n",
                        exported
                            .stringify_pretty()
                            .unwrap_or_else(|| "undefined".into())
                    ),
                )?;
                return Ok(obj![
                    ("written", output),
                    ("omitted", field(&exported, "omitted")?.clone()),
                    ("tickets", length(field(&exported, "tickets")?)?),
                    ("memory", length(field(&exported, "memory")?)?),
                    ("prompts", length(field(&exported, "prompts")?)?),
                ]);
            }
            Ok(exported)
        }
        "projects.import" => {
            let exported = match document(io, &a(0))
                .await
                .ok()
                .and_then(|text| js::parse(&text).ok())
            {
                Some(value) => value,
                None => return Err(refusal("The export document is not valid JSON.")),
            };
            if !exported.truthy()
                || !(exported.is_obj() || exported.is_arr())
                || exported.get("kind").as_str() != Some("luuna-project-export")
            {
                return Err(refusal("The document is not a Luuna project export; pass the JSON that `export` wrote on the other machine."));
            }
            let key = match input.text_if("key") {
                Some(key) => key,
                None => {
                    let hash = exported.get("export_hash");
                    let text = if hash.nullish() {
                        String::new()
                    } else {
                        hash.js()
                    };
                    let slice = js::slice16(&text, 7, Some(31));
                    format!(
                        "import-{}",
                        if slice.is_empty() {
                            random_uuid()
                        } else {
                            slice
                        }
                    )
                }
            };
            if js_trim(&key).is_empty() {
                return Err(Failure::error("An idempotency key is required."));
            }
            let body = obj![
                ("export", exported),
                (
                    "repositories",
                    J::Arr(vec![obj![("id", a(1)), ("path", a(2))]])
                ),
                ("idempotency_key", key)
            ];
            client.post("/projects/import", Some(&body), s).await
        }
        "projects.events" => {
            let after = integer(input.flag("after"), "after", 0.0)?.unwrap_or(0.0);
            let limit = integer(input.flag("limit"), "limit", 1.0)?.unwrap_or(1000.0);
            read_event_pages(
                client,
                &format!("/projects/{}/events", e(0)),
                after,
                limit,
                s,
            )
            .await
        }
        "baselines.start" => {
            let mut body = obj![("repository_id", a(1))];
            if input.truthy("include-optional") {
                body.set("include_optional", J::Bool(true));
            }
            client
                .post(&format!("/projects/{}/baselines", e(0)), Some(&body), s)
                .await
        }
        "baselines.list" => {
            client
                .get(&format!("/projects/{}/baselines", e(0)), s)
                .await
        }
        "components.status" => {
            client
                .get(&format!("/projects/{}/components", e(0)), s)
                .await
        }
        "projects.storemode.show" => {
            client
                .get(&format!("/projects/{}/store-mode", e(0)), s)
                .await
        }
        "projects.storemode.set" => {
            let mode = input
                .flag("mode")
                .map(|v| v.text().trim().to_lowercase())
                .unwrap_or_default();
            let repository = input.flag("repository").map(|v| v.text().trim().to_owned());
            let body = match (mode.as_str(), repository) {
                ("runtime", None) => obj![("mode", J::Str("runtime".into()))],
                ("repository", Some(repository)) if !repository.is_empty() => obj![
                    ("mode", J::Str("repository".into())),
                    ("repository", J::Str(repository))
                ],
                ("repository", _) => {
                    return Err(usage_error(
                        "--mode repository needs --repository <registered repository id>.",
                    ))
                }
                ("runtime", Some(_)) => {
                    return Err(usage_error(
                        "--repository applies only to --mode repository.",
                    ))
                }
                _ => return Err(usage_error("--mode is runtime or repository.")),
            };
            client
                .request(
                    "PUT",
                    &format!("/projects/{}/store-mode", e(0)),
                    Some(&body),
                    s,
                )
                .await
        }
        "components.enable" => {
            client
                .post(&format!("/projects/{}/components", e(0)), None, s)
                .await
        }
        "memory.list" => {
            let mut parameters = Vec::new();
            if let Some(query) = input.text_if("query") {
                parameters.push(format!("q={}", encode_component(&query)));
            }
            if input.truthy("include-user") {
                parameters.push("include_user=true".into());
            }
            let search = if parameters.is_empty() {
                String::new()
            } else {
                format!("?{}", parameters.join("&"))
            };
            client
                .get(&format!("/projects/{}/memory{search}", e(0)), s)
                .await
        }
        "memory.create" => {
            if !["project", "process", "user"].contains(&a(1).as_str()) {
                return Err(usage_error("The memory scope is project, process or user."));
            }
            let key = key(input);
            if js_trim(&key).is_empty() {
                return Err(Failure::error("An idempotency key is required."));
            }
            let body = obj![
                ("idempotency_key", key),
                ("scope", a(1)),
                ("content", a(2)),
                ("provenance", a(3))
            ];
            client
                .post(&format!("/projects/{}/memory", e(0)), Some(&body), s)
                .await
        }
        "memory.delete" => {
            if !a(2).starts_with("sha256:") {
                return Err(Failure::error("Delete memory by its exact content hash."));
            }
            client
                .request(
                    "DELETE",
                    &format!("/projects/{}/memory/{}?expected_hash={}", e(0), e(1), e(2)),
                    None,
                    s,
                )
                .await
        }
        "memory.promote" => {
            if !["project", "user"].contains(&a(3).as_str()) {
                return Err(usage_error(
                    "Memory promotes to project (from process) or to user (from project).",
                ));
            }
            let body = obj![
                ("idempotency_key", key(input)),
                ("expected_hash", a(2)),
                ("to_scope", a(3))
            ];
            if !a(2).starts_with("sha256:") {
                return Err(Failure::error("Promote memory by its exact content hash."));
            }
            client
                .post(
                    &format!("/projects/{}/memory/{}/promote", e(0), e(1)),
                    Some(&body),
                    s,
                )
                .await
        }
        "memory.update" => {
            if js_trim(&a(3)).is_empty() {
                return Err(usage_error("The corrected content is empty."));
            }
            let key = key(input);
            if !a(2).starts_with("sha256:") {
                return Err(Failure::error("Correct memory by its exact content hash."));
            }
            if js_trim(&key).is_empty() {
                return Err(Failure::error("An idempotency key is required."));
            }
            let body = obj![
                ("idempotency_key", key),
                ("expected_hash", a(2)),
                ("content", a(3))
            ];
            client
                .request(
                    "PUT",
                    &format!("/projects/{}/memory/{}", e(0), e(1)),
                    Some(&body),
                    s,
                )
                .await
        }
        "memory.user.export" => {
            let exported = client.get("/memory/user/export", s).await?;
            if let Some(output) = input.text_if("output") {
                io.write_file(
                    &output,
                    &format!(
                        "{}\n",
                        exported
                            .stringify_pretty()
                            .unwrap_or_else(|| "undefined".into())
                    ),
                )?;
                return Ok(obj![
                    ("written", output),
                    ("omitted", field(&exported, "omitted")?.clone()),
                    ("memory", length(field(&exported, "memory")?)?)
                ]);
            }
            Ok(exported)
        }
        "memory.starter" => {
            // ADR 0059: the runtime proposes; only the facts the developer keeps are recorded,
            // each as an ordinary project memory with the fact's own provenance.
            let mut set = client
                .get(&format!("/projects/{}/memory/starter", e(0)), s)
                .await?;
            let facts = set.get("facts").items().to_vec();
            let proposed = |key: &str| facts.iter().any(|f| f.get("key").as_str() == Some(key));
            // A fact key can hold a comma (a relationship contract may), so an
            // argument that names a whole proposed key is kept as written and
            // only other arguments are read as comma-separated lists.
            let wanted: Vec<String> = input
                .list("keep")
                .iter()
                .flat_map(|item| {
                    let whole = js_trim(item);
                    if proposed(&whole) {
                        vec![whole]
                    } else {
                        item.split(',').map(js_trim).collect()
                    }
                })
                .filter(|key| !key.is_empty())
                .collect();
            if let Some(unknown) = wanted
                .iter()
                .find(|key| key.as_str() != "all" && !proposed(key))
            {
                return Err(usage_error(format!(
                    "No proposed fact has the key {unknown}; run memory-starter without --keep to list them."
                )));
            }
            let mut kept = Vec::new();
            for fact in facts.iter().filter(|f| f.get("recorded") != &J::Bool(true)) {
                let key = fact.get("key").js();
                let keep = if wanted.is_empty() {
                    io.interactive()
                        && io
                            .confirm(format!(
                                "Keep this fact as project memory?\n  {}\n  ({})",
                                fact.get("content").js(),
                                fact.get("provenance").js()
                            ))
                            .await?
                } else {
                    wanted.iter().any(|w| w == "all" || *w == key)
                };
                if !keep {
                    continue;
                }
                let body = obj![
                    ("idempotency_key", fact.get("idempotency_key").clone()),
                    ("scope", "project"),
                    ("content", fact.get("content").clone()),
                    ("provenance", fact.get("provenance").clone())
                ];
                kept.push(
                    client
                        .post(&format!("/projects/{}/memory", e(0)), Some(&body), s)
                        .await?,
                );
            }
            set.set("kept", J::Arr(kept));
            Ok(set)
        }
        "memory.retention.show" => {
            client
                .get(&format!("/projects/{}/memory/retention", e(0)), s)
                .await
        }
        "memory.retention.set" => {
            let days = |name: &str| -> Result<J, Failure> {
                let value = input.flag(name);
                match value {
                    None => return Ok(J::Null),
                    Some(FlagValue::Str(text)) if text.is_empty() => return Ok(J::Null),
                    Some(v) if v.text().to_lowercase() == "none" => return Ok(J::Null),
                    _ => {}
                }
                let number = integer(value, name, 1.0)?.unwrap();
                if number > 3650.0 {
                    return Err(usage_error(format!("--{name} is at most 3650 days.")));
                }
                Ok(J::Num(number))
            };
            let body = obj![
                ("process_max_age_days", days("process-days")?),
                ("project_max_age_days", days("project-days")?)
            ];
            client
                .request(
                    "PUT",
                    &format!("/projects/{}/memory/retention", e(0)),
                    Some(&body),
                    s,
                )
                .await
        }
        "memory.retention.apply" => {
            let key = key(input);
            if js_trim(&key).is_empty() {
                return Err(Failure::error("An idempotency key is required."));
            }
            client
                .post(
                    &format!("/projects/{}/memory/retention/apply", e(0)),
                    Some(&obj![("idempotency_key", key)]),
                    s,
                )
                .await
        }
        "prompts.list" => client.get(&format!("/projects/{}/prompts", e(0)), s).await,
        "prompts.create" => {
            let key = key(input);
            let body_text = document(io, &a(2)).await?;
            let mut body = obj![
                ("idempotency_key", key.clone()),
                ("name", a(1)),
                ("body", body_text)
            ];
            if input.flag("expected-version").is_some() {
                body.set(
                    "expected_version",
                    J::Num(
                        integer(input.flag("expected-version"), "expected-version", 0.0)?.unwrap(),
                    ),
                );
            }
            if js_trim(&key).is_empty() {
                return Err(Failure::error("An idempotency key is required."));
            }
            client
                .post(&format!("/projects/{}/prompts", e(0)), Some(&body), s)
                .await
        }
        "prompts.render" => {
            let version = integer_arg(&a(2), "version", 1.0)?;
            client
                .get(
                    &format!(
                        "/projects/{}/prompts/{}/render?version={}",
                        e(0),
                        e(1),
                        js::number_to_string(version)
                    ),
                    s,
                )
                .await
        }
        "intent.show" => client.get(&format!("/projects/{}/intent", e(0)), s).await,
        "intent.proposals" => {
            client
                .get(&format!("/projects/{}/intent/proposals", e(0)), s)
                .await
        }
        "intent.questions" => {
            client
                .get(&format!("/projects/{}/intent/questions", e(0)), s)
                .await
        }
        "intent.sprouts" => {
            client
                .get(&format!("/projects/{}/intent/sprouts", e(0)), s)
                .await
        }
        "intent.init"
        | "intent.proposals.accept"
        | "intent.proposals.reject"
        | "intent.sprouts.reject" => {
            let by = js_trim(
                &input
                    .flag("by")
                    .map(FlagValue::text)
                    .unwrap_or_else(|| "developer".into()),
            );
            if by.is_empty()
                || bytes(&by) > LIMIT_APPROVER
                || by.starts_with("agent:")
                || by.starts_with("roots:")
                || by == "roots"
            {
                return Err(refusal("Supply your explicit human name, at most 200 UTF-8 bytes; agent and Roots identities are refused."));
            }
            let mut body = obj![("by", by)];
            for name in ["reason", "rel", "keep"] {
                if let Some(value) = input.flag(name) {
                    body.set(name, J::from(value.text()));
                }
            }
            let path = match command.id.as_str() {
                "intent.init" => format!("/projects/{}/intent", e(0)),
                "intent.proposals.accept" => {
                    format!("/projects/{}/intent/proposals/{}/accept", e(0), e(1))
                }
                "intent.proposals.reject" => {
                    format!("/projects/{}/intent/proposals/{}/reject", e(0), e(1))
                }
                _ => format!("/projects/{}/intent/sprouts/{}/reject", e(0), e(1)),
            };
            client.post(&path, Some(&body), s).await
        }
        "tickets.create" => {
            let title = js_trim(&input.flag("title").map(FlagValue::text).unwrap_or_default());
            let intent = js_trim(
                &input
                    .flag("intent")
                    .map(FlagValue::text)
                    .unwrap_or_default(),
            );
            let criteria: Vec<String> = input
                .list("criterion")
                .iter()
                .map(|l| js_trim(l))
                .filter(|l| !l.is_empty())
                .collect();
            if title.is_empty() || intent.is_empty() || criteria.is_empty() {
                return Err(usage_error(
                    "A ticket needs --title, --intent and at least one --criterion.",
                ));
            }
            let purpose = match input.flag("purpose") {
                None => "feature".to_string(),
                Some(v) if js_trim(&v.text()).is_empty() => "feature".to_string(),
                Some(v) => js_trim(&v.text()),
            };
            if purpose != "feature" && purpose != "testing_setup" {
                return Err(usage_error("--purpose is feature or testing_setup."));
            }
            if bytes(&title) > LIMIT_TITLE {
                return Err(refusal(format!(
                    "The title is {} bytes; the runtime accepts at most {LIMIT_TITLE} (UTF-8).",
                    bytes(&title)
                )));
            }
            if bytes(&intent) > LIMIT_INTENT {
                return Err(refusal(format!(
                    "The intent is {} bytes; the runtime accepts at most {LIMIT_INTENT} (UTF-8).",
                    bytes(&intent)
                )));
            }
            let longest = criteria.iter().map(|c| bytes(c)).max().unwrap_or(0);
            if longest > LIMIT_CRITERION {
                return Err(refusal(format!("The longest criterion is {longest} bytes; the runtime accepts at most {LIMIT_CRITERION} (UTF-8).")));
            }
            let repository = match input.text_if("repository") {
                Some(r) => J::str(r),
                None => {
                    let view = client.get(&format!("/projects/{}", e(0)), s).await?;
                    let repositories = field(&view, "repositories")?;
                    let first = if repositories.nullish() {
                        return Err(Failure::type_error());
                    } else {
                        repositories.at(0)
                    };
                    if first.nullish() {
                        J::Undefined
                    } else {
                        first.get("repository_id").clone()
                    }
                }
            };
            if !repository.truthy() {
                return Err(refusal(
                    "The project has no registered repository to scope the ticket to.",
                ));
            }
            let key = key(input);
            let mut body = obj![
                ("idempotency_key", key.clone()),
                ("title", title),
                ("intent", intent),
                (
                    "acceptance_criteria",
                    J::Arr(criteria.into_iter().map(J::Str).collect())
                ),
                ("repository_ids", J::Arr(vec![repository]))
            ];
            if purpose != "feature" {
                body.set("purpose", J::str(purpose));
            }
            for (key, value) in ticket_triage(input, true)? {
                body.set(key, value);
            }
            if js_trim(&key).is_empty() {
                return Err(Failure::error("An idempotency key is required."));
            }
            client
                .post(&format!("/projects/{}/tickets", e(0)), Some(&body), s)
                .await
        }
        "tickets.list" => client.get(&format!("/projects/{}/tickets", e(0)), s).await,
        "tickets.show" => client.get(&format!("/tickets/{}", e(0)), s).await,
        "tickets.source" => client.get(&format!("/tickets/{}/source", e(0)), s).await,
        "tickets.reconcile" => {
            let revision = match integer(input.flag("revision"), "revision", 1.0)? {
                Some(r) => J::Num(r),
                None => field(
                    &client.get(&format!("/tickets/{}", e(0)), s).await?,
                    "revision",
                )?
                .clone(),
            };
            if !revision
                .as_f64()
                .is_some_and(|r| safe_integer(r) && r >= 1.0)
            {
                return Err(Failure::error("Invalid ticket revision."));
            }
            client
                .post(
                    &format!("/tickets/{}/reconcile", e(0)),
                    Some(&obj![("expected_revision", revision)]),
                    s,
                )
                .await
        }
        "tickets.update" => {
            let title = input.flag("title").map(|v| js_trim(&v.text()));
            let intent = input.flag("intent").map(|v| js_trim(&v.text()));
            let criteria: Vec<String> = input
                .list("criterion")
                .iter()
                .map(|l| js_trim(l))
                .filter(|l| !l.is_empty())
                .collect();
            let repositories: Vec<String> = input
                .list("repository")
                .iter()
                .map(|l| js_trim(l))
                .filter(|l| !l.is_empty())
                .collect();
            if title.is_none() && intent.is_none() && criteria.is_empty() && repositories.is_empty()
            {
                return Err(usage_error(
                    "Name at least one change: --title, --intent, --criterion or --repository.",
                ));
            }
            if title.as_deref() == Some("") || intent.as_deref() == Some("") {
                return Err(usage_error(
                    "A title or intent cannot be emptied; omit the flag to keep the current value.",
                ));
            }
            if let Some(title) = &title {
                if bytes(title) > LIMIT_TITLE {
                    return Err(refusal(format!(
                        "The title is {} bytes; the runtime accepts at most {LIMIT_TITLE} (UTF-8).",
                        bytes(title)
                    )));
                }
            }
            if let Some(intent) = &intent {
                if bytes(intent) > LIMIT_INTENT {
                    return Err(refusal(format!("The intent is {} bytes; the runtime accepts at most {LIMIT_INTENT} (UTF-8).", bytes(intent))));
                }
            }
            let revision = match integer(input.flag("revision"), "revision", 1.0)? {
                Some(r) => J::Num(r),
                None => field(
                    &client.get(&format!("/tickets/{}", e(0)), s).await?,
                    "revision",
                )?
                .clone(),
            };
            if !revision
                .as_f64()
                .is_some_and(|r| safe_integer(r) && r >= 1.0)
            {
                return Err(Failure::error("Invalid ticket revision."));
            }
            let mut body = obj![("expected_revision", revision)];
            if let Some(title) = title {
                body.set("title", J::str(title));
            }
            if let Some(intent) = intent {
                body.set("intent", J::str(intent));
            }
            if !criteria.is_empty() {
                body.set(
                    "acceptance_criteria",
                    J::Arr(criteria.into_iter().map(J::Str).collect()),
                );
            }
            if !repositories.is_empty() {
                body.set(
                    "repository_ids",
                    J::Arr(repositories.into_iter().map(J::Str).collect()),
                );
            }
            for (key, value) in ticket_triage(input, false)? {
                body.set(key, value);
            }
            client
                .request("PUT", &format!("/tickets/{}", e(0)), Some(&body), s)
                .await
        }
        "plans.create" => plan(client, input, io, s).await,
        "plans.list" => client.get(&format!("/tickets/{}/plans", e(0)), s).await,
        "plans.show" => client.get(&format!("/plans/{}", e(0)), s).await,
        "plans.approve" => {
            let plan = client.get(&format!("/plans/{}", e(0)), s).await?;
            let acknowledged = acknowledged_hash(io, input, &plan).await?;
            if acknowledged.is_empty() {
                return Err(refusal(
                    "Approval cancelled: no plan hash was acknowledged.",
                ));
            }
            let hash = field(&plan, "hash")?;
            let acknowledged = crate::refs::complete_hash(&acknowledged, &hash.js());
            if hash.as_str() != Some(acknowledged.as_str()) {
                return Err(refusal(format!("The acknowledged hash does not match plan {} r{} ({}). Review the complete plan, then acknowledge its hash (the first 8 hash digits are enough).", plan.get("id").js(), plan.get("revision").js(), hash.js())));
            }
            let approver = input
                .flag("approved-by")
                .map(FlagValue::text)
                .unwrap_or_else(|| "developer".into());
            let approver = match js_trim(&approver) {
                t if t.is_empty() => "developer".to_string(),
                t => t,
            };
            if bytes(&approver) > LIMIT_APPROVER {
                return Err(refusal(format!("The approver name is {} bytes; the runtime accepts at most {LIMIT_APPROVER} (UTF-8).", bytes(&approver))));
            }
            if !acknowledged.starts_with("sha256:") {
                return Err(Failure::error("Approve the plan by its exact hash."));
            }
            let body = obj![
                ("plan_revision", plan.get("revision").clone()),
                ("plan_hash", acknowledged),
                ("approved_by", approver)
            ];
            client
                .post(
                    &format!(
                        "/plans/{}/approvals",
                        encode_component(&plan.get("id").js())
                    ),
                    Some(&body),
                    s,
                )
                .await
        }
        "runs.show" => client.get(&format!("/runs/{}", e(0)), s).await,
        "runs.events" => {
            let after = integer(input.flag("after"), "after", 0.0)?.unwrap_or(0.0);
            let limit = integer(input.flag("limit"), "limit", 1.0)?.unwrap_or(1000.0);
            read_event_pages(client, &format!("/runs/{}/events", e(0)), after, limit, s).await
        }
        "runs.stream" => {
            let start = integer(input.flag("after"), "after", 0.0)?.unwrap_or(0.0);
            let id = a(0);
            let last = std::sync::Mutex::new(J::Num(start));
            let count = std::sync::atomic::AtomicUsize::new(0);
            client
                .follow_events(
                    &id,
                    start,
                    s,
                    &mut |event| {
                        *last.lock().unwrap() = event.get("sequence").clone();
                        count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        io.emit(&event);
                    },
                    &mut |failure, _| {
                        let seen = last.lock().unwrap().js();
                        io.note(&format!("Runtime outage while following {id}: {} Reconnecting; replay resumes after sequence {seen}.", describe_failure(failure)));
                    },
                    &mut |_| {
                        let next = match &*last.lock().unwrap() {
                            J::Num(n) => js::number_to_string(n + 1.0),
                            other => format!("{}1", other.js()),
                        };
                        io.note(&format!("Reconnected to the runtime; following {id} from sequence {next}."));
                    },
                )
                .await?;
            let run = client.get(&format!("/runs/{}", e(0)), s).await?;
            let last = last.into_inner().unwrap();
            Ok(obj![
                ("run_id", id.clone()),
                ("state", field(&run, "state")?.clone()),
                ("last_sequence", last),
                ("events_followed", count.into_inner())
            ])
        }
        "runs.cancel" => {
            client
                .post(&format!("/runs/{}/cancel", e(0)), None, s)
                .await
        }
        "runs.implement" => {
            let mut body = J::obj();
            if let Some(task) = input.text_if("task") {
                body.set("task_id", J::str(task));
            }
            model_settings(input, &mut body);
            client
                .post(&format!("/runs/{}/implement", e(0)), Some(&body), s)
                .await
        }
        "runs.candidate" => client.get(&format!("/runs/{}/candidate", e(0)), s).await,
        "runs.diff" => client.get(&format!("/runs/{}/diff", e(0)), s).await,
        "runs.validate" | "runs.repair" | "runs.publication.repair" => {
            let hash = candidate_hash(client, &a(0), input, s).await?;
            let mut body = obj![("candidate_hash", hash.clone())];
            if command.id == "runs.validate" {
                body.set("auto_repair", J::Bool(input.truthy("auto-repair")));
            }
            model_settings(input, &mut body);
            if !full_hash(&hash) {
                return Err(Failure::error("Use the candidate's complete hash."));
            }
            let route = match command.id.as_str() {
                "runs.validate" => "validate",
                "runs.repair" => "repair",
                _ => "publication/repair",
            };
            client
                .post(&format!("/runs/{}/{route}", e(0)), Some(&body), s)
                .await
        }
        "runs.publish" => {
            let hash = candidate_hash(client, &a(0), input, s).await?;
            let body = obj![
                ("expected_hash", hash.clone()),
                ("idempotency_key", key(input))
            ];
            if !starts_with_sha(&hash, "request.expected_hash")? {
                return Err(Failure::error("Publish by the exact candidate hash."));
            }
            client
                .post(&format!("/runs/{}/publish", e(0)), Some(&body), s)
                .await
        }
        "runs.publication" => client.get(&format!("/runs/{}/publication", e(0)), s).await,
        "runs.publication.reconcile" => {
            client
                .post(&format!("/runs/{}/publication/reconcile", e(0)), None, s)
                .await
        }
        "runs.worktrees" => client.get(&format!("/runs/{}/worktrees", e(0)), s).await,
        "runs.attempts" => client.get(&format!("/runs/{}/attempts", e(0)), s).await,
        "runs.tasks" => client.get(&format!("/runs/{}/tasks", e(0)), s).await,
        // Orbit (ADR 0036): every repository of a project at once; the developer's commands, never evidence.
        "orbit.status" => client.get(&format!("/projects/{}/orbit", e(0)), s).await,
        "orbit.exec" => {
            let (executable, args) = orbit_command(&a(1)).map_err(refusal)?;
            let mut request = obj![
                ("executable", executable.clone()),
                ("args", J::Arr(args.into_iter().map(J::Str).collect()))
            ];
            selected_repositories(input, &mut request);
            if input.truthy("parallel") {
                request.set("parallel", J::Bool(true));
            }
            let timeout = integer(input.flag("timeout"), "timeout", 1.0)?;
            if let Some(timeout) = timeout {
                request.set("timeout_seconds", J::num(timeout));
            }
            if let Some(cwd) = input.text_if("cwd") {
                request.set("cwd", J::str(cwd));
            }
            if js_trim(&executable).is_empty() {
                return Err(Failure::error("Type a command to run in each repository."));
            }
            if timeout.is_some_and(|t| !(1.0..=3600.0).contains(&t)) {
                return Err(Failure::error("The timeout must be 1 to 3600 seconds."));
            }
            client
                .post(&format!("/projects/{}/orbit/exec", e(0)), Some(&request), s)
                .await
        }
        "orbit.run" => {
            let mut request = obj![("recipe", a(1))];
            selected_repositories(input, &mut request);
            if input.truthy("parallel") {
                request.set("parallel", J::Bool(true));
            }
            if js_trim(&a(1)).is_empty() {
                return Err(Failure::error("Choose a recipe to run."));
            }
            client
                .post(&format!("/projects/{}/orbit/run", e(0)), Some(&request), s)
                .await
        }
        "orbit.sync" => {
            let mut request = J::obj();
            selected_repositories(input, &mut request);
            if input.truthy("no-fetch") {
                request.set("fetch", J::Bool(false));
            }
            client
                .post(&format!("/projects/{}/orbit/sync", e(0)), Some(&request), s)
                .await
        }
        // `lu sbx <project> [-- command ...]`: the runtime's plan, applied by the native launcher with
        // the terminal attached. --plan-only (or --json) prints the plan instead of opening the sandbox.
        "orbit.sandbox" => {
            let network = input
                .flag("network")
                .map(FlagValue::text)
                .unwrap_or_else(|| "open".into());
            if network != "open" && network != "none" {
                return Err(usage_error("--network takes open or none."));
            }
            let plan = client
                .get(
                    &format!("/projects/{}/orbit/sandbox?network={network}", e(0)),
                    s,
                )
                .await?;
            if input.truthy("plan-only") || input.flag("json").is_some() || !io.can_launch() {
                return Ok(plan);
            }
            if plan.nullish() {
                return Err(Failure::type_error());
            }
            if !plan.get("available").truthy() {
                let reason = plan.get("reason");
                return Err(refusal(if reason.nullish() {
                    "The sandbox is not available on this host.".to_string()
                } else {
                    reason.js()
                }));
            }
            let Some(items) = plan.get("repositories").as_arr() else {
                return Err(Failure::type_error());
            };
            let mut reachable = Vec::new();
            for repository in items {
                if repository.nullish() {
                    return Err(Failure::type_error());
                }
                reachable.push(repository.get("repository_id").clone());
            }
            io.note(&format!(
                "Opening the {} sandbox ({}, network {}) in {}; only {} {} reachable. Exit the shell to leave it.",
                a(0),
                plan.get("backend").js(),
                plan.get("network").js(),
                plan.get("workdir").js(),
                reachable.iter().map(J::js_or_empty).collect::<Vec<_>>().join(", "),
                if items.len() == 1 { "is" } else { "are" }
            ));
            io.launch(
                plan.clone(),
                input.args[1..].to_vec(),
                input.list("grant-read"),
            )
            .await?;
            Ok(J::Undefined)
        }
        "artifacts.read" => client.artifact(&a(0), &a(1), s).await,
        "projection.home" | "projection.board" => {
            let only = input.text_if("project");
            let (projects, runs) = tokio::join!(
                read_projects(client, only.as_deref(), s),
                client.get("/runs", s)
            );
            let (views, baselines, tickets) = projects?;
            let runs = runs?;
            let runs = runs.as_arr().ok_or_else(Failure::type_error)?.clone();
            if command.id == "projection.home" {
                crate::projections::home_summary(&views, &baselines, &tickets, &runs)
            } else {
                let mut all = Vec::new();
                for (_, listed) in &tickets {
                    match listed {
                        J::Arr(items) => all.extend(items.iter().cloned()),
                        other => all.push(other.clone()),
                    }
                }
                crate::projections::board_projection(&views, &all, &runs)
            }
        }
        _ => Err(Failure::error(format!("Unknown command {}.", command.name))),
    }
}

/// The interactive approval question. What to type and how to cancel lead, so the workbench's
/// one-line prompt panel shows them on an 80-column terminal; the plan and its hash follow.
pub fn approval_question(plan_id: &str, revision: &str, hash: &str) -> String {
    format!(
        "Type approve (or 8+ hash digits) to approve plan {} r{revision} · empty cancels · hash {}… as shown above",
        crate::refs::short_id(plan_id),
        crate::refs::short_hash(hash)
    )
}

/// `plan <ticket>`: the plan request both clients send for the same tasks.
async fn plan(client: &Client, input: &Input, io: &dyn Io, s: &Signal) -> Result<J, Failure> {
    let ticket = input.args.first().cloned().unwrap_or_default();
    let repository = match input.text_if("repository") {
        Some(r) => J::str(r),
        None => {
            let view = client
                .get(&format!("/tickets/{}", encode_component(&ticket)), s)
                .await?;
            let ids = field(&view, "repository_ids")?;
            if ids.nullish() {
                return Err(Failure::type_error());
            }
            match ids {
                J::Str(text) => text
                    .chars()
                    .next()
                    .map(|c| J::str(c.to_string()))
                    .unwrap_or(J::Undefined),
                other => other.at(0).clone(),
            }
        }
    };
    let task_options = |suffix: &str| -> Result<Vec<(String, String)>, Failure> {
        let mut options: Vec<(String, String)> = Vec::new();
        for part in suffix.split(';').filter(|p| !p.is_empty()) {
            // /^(role|derived_from)=(.+)$/
            let parsed = part.split_once('=').filter(|(name, value)| {
                (*name == "role" || *name == "derived_from")
                    && !value.is_empty()
                    && !value.contains(['\n', '\r', '\u{2028}', '\u{2029}'])
            });
            let Some((name, value)) = parsed else {
                return Err(usage_error(
                    "--task options are ;role=<role> and ;derived_from=<task>",
                ));
            };
            let key = if name == "role" {
                "role"
            } else {
                "derived_from_task"
            };
            match options.iter_mut().find(|(k, _)| k == key) {
                Some(entry) => entry.1 = value.to_string(),
                None => options.push((key.to_string(), value.to_string())),
            }
        }
        Ok(options)
    };
    let mut overrides = Vec::new();
    for spec in input.list("override") {
        // /^([^:]+):([^:]+):(waive|add):(.+)$/s
        let mut parts = spec.splitn(3, ':');
        let consumer = parts.next().unwrap_or("");
        let producer = parts.next().unwrap_or("");
        let rest = parts.next().unwrap_or("");
        let (action, justification) = rest.split_once(':').unwrap_or(("", ""));
        if consumer.is_empty()
            || producer.is_empty()
            || !(action == "waive" || action == "add")
            || justification.is_empty()
        {
            return Err(usage_error(
                "--override takes <consumer>:<producer>:<waive|add>:<justification>",
            ));
        }
        overrides.push(Override {
            consumer: consumer.into(),
            producer: producer.into(),
            action: action.into(),
            justification: justification.into(),
        });
    }
    let mut checks = Vec::new();
    for spec in input.list("check") {
        // /^([A-Za-z0-9._-]+)=([^,@]+)((?:,[^,@]*)*)(?:@(\d+))?$/
        let invalid =
            || usage_error("--check takes <id>=<executable>[,<arg>,...][@<timeout seconds>]");
        let (id, rest) = spec.split_once('=').ok_or_else(invalid)?;
        if id.is_empty()
            || !id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "._-".contains(c))
        {
            return Err(invalid());
        }
        let (body, timeout) = match rest.rsplit_once('@') {
            Some((body, digits))
                if !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()) =>
            {
                (body, Some(digits))
            }
            _ => (rest, None),
        };
        if body.contains('@') {
            return Err(invalid());
        }
        let mut pieces = body.split(',');
        let executable = pieces.next().unwrap_or("");
        if executable.is_empty() {
            return Err(invalid());
        }
        let args: Vec<J> = pieces.filter(|p| !p.is_empty()).map(J::from).collect();
        let timeout = timeout.map(js::string_to_number).unwrap_or(600.0);
        checks.push(obj![
            ("id", id),
            ("executable", executable),
            ("args", J::Arr(args)),
            ("timeout_seconds", timeout)
        ]);
    }
    if let Some(source) = input.flag("checks") {
        let listed = match document(io, &source.text())
            .await
            .ok()
            .and_then(|text| js::parse(&text).ok())
        {
            Some(value) => value,
            None => return Err(refusal("The --checks document is not valid JSON.")),
        };
        let Some(items) = listed.as_arr().filter(|items| {
            items
                .iter()
                .all(|c| c.truthy() && (c.is_obj() || c.is_arr()))
        }) else {
            return Err(refusal("The --checks document must be a JSON array of check recipes (id, executable, args, cwd, timeout_seconds, capabilities)."));
        };
        for check in items {
            let mut copy = if check.is_obj() {
                check.clone()
            } else {
                J::obj()
            };
            let text = |value: &J| {
                if value.nullish() {
                    String::new()
                } else {
                    value.js()
                }
            };
            copy.set("id", J::str(text(check.get("id"))));
            copy.set("executable", J::str(text(check.get("executable"))));
            let timeout = check.get("timeout_seconds");
            copy.set(
                "timeout_seconds",
                J::Num(if timeout.is_undefined() {
                    600.0
                } else {
                    timeout.to_number()
                }),
            );
            checks.push(copy);
        }
    }
    let paths: Vec<String> = input
        .list("path")
        .iter()
        .flat_map(|value| value.split(',').map(str::to_string).collect::<Vec<_>>())
        .map(|p| js_trim(&p))
        .filter(|p| !p.is_empty())
        .collect();
    let mut budgets: builders::Budgets = Vec::new();
    if let Some(attempts) = input.flag("attempts") {
        budgets.push(("max_attempts", js::string_to_number(&attempts.text())));
    }
    if let Some(timeout) = input.flag("timeout") {
        budgets.push(("timeout_seconds", js::string_to_number(&timeout.text())));
    }
    if let Some(cost) = input.flag("cost-usd") {
        budgets.push((
            "cost_usd_micros",
            round(js::string_to_number(&cost.text()) * 1_000_000.0),
        ));
    }
    if input.flag("workers").is_some() {
        budgets.push((
            "max_workers",
            integer(input.flag("workers"), "workers", 1.0)?.unwrap(),
        ));
    }
    let mut component_context = None;
    if input.flag("memory").is_some() || input.flag("prompt").is_some() {
        let selected = match input.flag("prompt") {
            None => None,
            Some(value) => {
                let text = value.text();
                let parsed = parse_prompt_selection(&text);
                if parsed.is_none() {
                    return Err(usage_error(
                        "--prompt names an exact prompt as name@version#sha256:<hash>.",
                    ));
                }
                parsed
            }
        };
        let mut context = obj![(
            "memory_ids",
            J::Arr(input.list("memory").into_iter().map(J::Str).collect())
        )];
        if let Some((name, version, hash)) = selected {
            context.set(
                "prompt",
                obj![
                    ("name", name),
                    ("version", js::string_to_number(&version)),
                    ("hash", hash)
                ],
            );
        }
        component_context = Some(context);
    }
    let operations: Vec<String> = if input.truthy("read-only") {
        vec!["read".into()]
    } else {
        vec!["read".into(), "write".into()]
    };
    let criteria = input.list("criterion");
    let tasks = input.list("task");
    let drafts: Vec<TaskDraft> = if !tasks.is_empty() {
        let mut drafts = Vec::new();
        for spec in tasks {
            let Some((id, paths, depends, suffix)) = parse_task_spec(&spec) else {
                return Err(usage_error("--task takes <repository>=<path,...>[>dependency-task,...][;role=<role>][;derived_from=<task>]"));
            };
            let options = task_options(&suffix)?;
            let role = options
                .iter()
                .find(|(k, _)| k == "role")
                .map(|(_, v)| v.clone());
            let derived = options
                .iter()
                .find(|(k, _)| k == "derived_from_task")
                .map(|(_, v)| v.clone());
            drafts.push(TaskDraft {
                id: Some(id.clone()),
                repository_id: J::str(id),
                paths: paths.split(',').map(str::to_string).collect(),
                operations: if role.as_deref().is_some_and(|r| r != "implementer") {
                    vec!["read".into()]
                } else {
                    operations.clone()
                },
                acceptance_criteria: criteria.clone(),
                depends_on: depends
                    .unwrap_or_default()
                    .split(',')
                    .map(str::to_string)
                    .collect(),
                role,
                derived_from_task: derived,
            });
        }
        drafts
    } else {
        let role = input.text_if("role");
        vec![TaskDraft {
            id: None,
            repository_id: repository,
            paths,
            operations: if role.as_deref().is_some_and(|r| r != "implementer") {
                vec!["read".into()]
            } else {
                operations
            },
            acceptance_criteria: criteria,
            depends_on: Vec::new(),
            role,
            derived_from_task: input.text_if("derived-from"),
        }]
    };
    let mut request =
        builders::plan_request_for_tasks(&drafts, &budgets, component_context, &overrides, &checks)
            .map_err(|f| refusal(f.message()))?;
    let key = key(input);
    request.set("idempotency_key", J::str(key.clone()));
    if !builders::valid_idempotency_key(&key) {
        return Err(Failure::error(
            "An idempotency key is 1 to 128 visible ASCII characters.",
        ));
    }
    if let Some(problem) = builders::plan_budget_problem(request.get("budgets")) {
        return Err(Failure::error(problem));
    }
    let path = format!("/tickets/{}/plans", encode_component(&ticket));
    match client.post(&path, Some(&request), s).await {
        Err(Failure::Api {
            status: 422,
            from_runtime: false,
            ..
        }) => {
            // A runtime that predates plan idempotency rejects the unknown field while decoding.
            let mut legacy = J::obj();
            for (k, v) in request.as_obj().unwrap() {
                if k != "idempotency_key" {
                    legacy.set(k, v.clone());
                }
            }
            client.post(&path, Some(&legacy), s).await
        }
        other => other,
    }
}

/// `/^(.+)@(\d+)#(sha256:[a-f0-9]{64})$/`
fn parse_prompt_selection(text: &str) -> Option<(String, String, String)> {
    let (head, hash) = text.rsplit_once('#')?;
    let hex = hash.strip_prefix("sha256:")?;
    if hex.len() != 64
        || !hex
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return None;
    }
    let (name, version) = head.rsplit_once('@')?;
    if name.is_empty()
        || version.is_empty()
        || !version.chars().all(|c| c.is_ascii_digit())
        || name.contains(['\n', '\r', '\u{2028}', '\u{2029}'])
    {
        return None;
    }
    Some((name.to_string(), version.to_string(), hash.to_string()))
}

/// `/^([a-z][a-z0-9-]*)=([^>;]*)(?:>([^;]*))?((?:;.*)?)$/`: (id, paths, dependencies, options).
fn parse_task_spec(text: &str) -> Option<(String, String, Option<String>, String)> {
    let (id, rest) = text.split_once('=')?;
    let mut chars = id.chars();
    if !chars.next().is_some_and(|c| c.is_ascii_lowercase())
        || !chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        return None;
    }
    let paths_end = rest.find(['>', ';']).unwrap_or(rest.len());
    let paths = rest[..paths_end].to_string();
    let mut after = &rest[paths_end..];
    let mut depends = None;
    if let Some(stripped) = after.strip_prefix('>') {
        let end = stripped.find(';').unwrap_or(stripped.len());
        depends = Some(stripped[..end].to_string());
        after = &stripped[end..];
    }
    if !after.is_empty()
        && (!after.starts_with(';') || after.contains(['\n', '\r', '\u{2028}', '\u{2029}']))
    {
        return None;
    }
    Some((id.to_string(), paths, depends, after.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(flags: &[(&str, FlagValue)]) -> Input {
        Input {
            args: vec![],
            flags: flags
                .iter()
                .map(|(n, v)| ((*n).to_owned(), v.clone()))
                .collect(),
        }
    }

    #[test]
    fn ticket_triage_flags_become_missions_shaped_fields() {
        let s = |v: &str| FlagValue::Str(v.into());
        let fields = ticket_triage(
            &input(&[
                ("type", s("Bug")),
                ("priority", s("P1")),
                (
                    "label",
                    FlagValue::List(vec!["UI, docs".into(), "ui".into()]),
                ),
            ]),
            true,
        )
        .unwrap();
        assert_eq!(
            fields,
            vec![
                ("type", J::str("bug")),
                ("priority", J::Num(1.0)),
                ("labels", J::Arr(vec![J::str("ui"), J::str("docs")])),
            ]
        );
        // Defaults stay out of a create, but an edit sends them.
        let defaults = input(&[("type", s("task")), ("priority", s("2"))]);
        assert!(ticket_triage(&defaults, true).unwrap().is_empty());
        assert_eq!(ticket_triage(&defaults, false).unwrap().len(), 2);
        let clear = input(&[("clear-labels", FlagValue::Bool(true))]);
        assert_eq!(
            ticket_triage(&clear, false).unwrap(),
            vec![("labels", J::Arr(vec![]))]
        );
        for bad in [
            input(&[("type", s("story"))]),
            input(&[("priority", s("P5"))]),
            input(&[("priority", s("urgent"))]),
            input(&[("label", s("luuna-ticket-x"))]),
            input(&[("label", s("two words"))]),
        ] {
            assert!(ticket_triage(&bad, true).is_err(), "{bad:?}");
        }
        assert_eq!(ticket_priority("p0"), Some(0));
        assert_eq!(ticket_priority("4"), Some(4));
        assert_eq!(ticket_priority("P10"), None);
    }

    #[test]
    fn starter_memory_can_save_so_it_is_never_a_replayed_read() {
        assert!(!read_only(by_name("memory-starter").unwrap()));
        assert!(read_only(by_name("memory").unwrap()));
    }

    #[test]
    fn orbit_command_lines_split_like_the_regular_expression() {
        let split = |line: &str| orbit_command(line).unwrap();
        assert_eq!(
            split(r#"git commit -m "two words""#),
            (
                "git".into(),
                vec!["commit".into(), "-m".into(), "two words".into()]
            )
        );
        // Quotes group words only when closed; an open quote starts an ordinary word.
        assert_eq!(
            split(r#"echo "unclosed words"#),
            ("echo".into(), vec!["\"unclosed".into(), "words".into()])
        );
        assert_eq!(split(r#""" arg"#), (String::new(), vec!["arg".into()]));
        assert_eq!(
            split("echo 'a b' mid\"x y\"z"),
            (
                "echo".into(),
                vec!["a b".into(), "mid\"x".into(), "y\"z".into()]
            )
        );
        assert_eq!(
            orbit_command(
                " 	
"
            )
            .unwrap_err(),
            "Type a command to run in each repository."
        );
    }

    #[test]
    fn an_orbit_run_fails_when_a_repository_failed_or_was_blocked() {
        let execution = |status: &str| obj![("results", J::Arr(vec![obj![("status", status)]]))];
        assert!(orbit_failed(&execution("failed")).unwrap());
        assert!(orbit_failed(&execution("blocked")).unwrap());
        assert!(!orbit_failed(&execution("skipped")).unwrap());
        assert!(!orbit_failed(&J::Undefined).unwrap());
        assert!(!orbit_failed(&J::str("text")).unwrap());
        assert!(orbit_failed(&obj![("results", J::Arr(vec![J::Null]))]).is_err());
        assert!(orbit_failed(&obj![("results", "str")]).is_err());
    }

    #[test]
    fn the_table_matches_the_catalog_and_parses_specs_like_the_regular_expressions() {
        let table = table();
        assert_eq!(table.commands.len(), 75);
        assert_eq!(table.projections.len(), 2);
        for command in &table.commands {
            assert_eq!(
                capability(&command.id).get("tui").as_str(),
                Some(command.name.as_str()),
                "{}",
                command.id
            );
        }
        assert_eq!(
            usage(by_name("artifact").unwrap()),
            "artifact <run> <artifact-id>"
        );
        assert_eq!(
            parse_task_spec("api=src,lib>shared;role=implementer"),
            Some((
                "api".into(),
                "src,lib".into(),
                Some("shared".into()),
                ";role=implementer".into()
            ))
        );
        assert_eq!(
            parse_task_spec("api=src"),
            Some(("api".into(), "src".into(), None, String::new()))
        );
        assert_eq!(parse_task_spec("Api=src"), None);
        assert_eq!(
            parse_task_spec("api=src x"),
            Some(("api".into(), "src x".into(), None, String::new()))
        );
        assert!(
            parse_prompt_selection(&format!("guide@lines@2#sha256:{}", "a".repeat(64)))
                .is_some_and(|(n, v, _)| n == "guide@lines" && v == "2")
        );
        assert!(parse_prompt_selection("g@x#sha256:aa").is_none());
        let uuid = random_uuid();
        assert_eq!(uuid.len(), 36);
        assert_ne!(uuid, random_uuid());
    }
}
