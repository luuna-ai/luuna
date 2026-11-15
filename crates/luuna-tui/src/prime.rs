//! `prime`: a guided setup of one project with the Luuna suite. It walks the developer through the
//! cataloged actions the commands run (registration, discovery, manifest confirmation, baseline,
//! component activation, a first memory and ticket) and the components' own agent onboarding
//! (repo-local stores, CLAUDE.md/AGENTS.md instructions, optional Claude Code hooks, and the context
//! each component primes an agent with), one step at a time, with defaults from the current Git
//! checkout. It is a guide, not a product action: every runtime step is a cataloged command run
//! through `commands::run` (so the bodies are the commands' bodies), every decision and refusal is
//! the runtime's, and anything skipped stays available as its plain command. The workbench's guide
//! mode (crate::guide, views::guide) shows the frames this sends.
use crate::builders::js_trim;
use crate::client::{is_terminal, Client, Signal};
use crate::commands::{by_id, fs_failure, FlagValue, Fut, Input, Io};
use crate::components::{instructions_file, Runner};
use crate::failures::{baseline_step, explain_failure, Failure};
use crate::js::{self, J};
use crate::text::{truncate, wrap};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

pub const STEPS: [&str; 8] = [
    "Runtime",
    "Project",
    "Discovery",
    "Manifest",
    "Baseline",
    "Suite",
    "Agents",
    "Ticket",
];
const POLL_MS: u64 = 1000;

/// The arguments a component run takes, per component.
type ArgsFor = Box<dyn Fn(&crate::components::Component) -> Vec<String> + Send + Sync>;
/// The guide face's width: the workbench wraps its own rows; this only bounds long lines.
const WIDTH: usize = 160;

/// The marker styles the workbench turns into spans: U+E000 style U+E001 text U+E002.
pub mod style {
    fn mark(style: &str, text: &str) -> String {
        format!("\u{E000}{style}\u{E001}{}\u{E002}", super::unmarked(text))
    }
    pub fn bold(text: &str) -> String {
        mark("bold", text)
    }
    pub fn dim(text: &str) -> String {
        mark("dim", text)
    }
    pub fn cyan(text: &str) -> String {
        mark("cyan", text)
    }
    pub fn green(text: &str) -> String {
        mark("green", text)
    }
    pub fn red(text: &str) -> String {
        mark("red", text)
    }
    pub fn yellow(text: &str) -> String {
        mark("yellow", text)
    }
    pub fn magenta(text: &str) -> String {
        mark("magenta", text)
    }
}
use style as s;

/// Text with any marker characters removed, so text the guide did not style cannot style itself.
pub fn unmarked(text: &str) -> String {
    text.chars()
        .filter(|c| !('\u{E000}'..='\u{E002}').contains(c))
        .collect()
}

/// The workbench side of the guide: frames out, answers back (crate::bridge implements it).
pub trait GuideUi: Send + Sync {
    /// One `guide` frame.
    fn send(&self, frame: Value);
    /// A question frame; resolves with the developer's answer (text, index, marks or "saved").
    fn ask(&self, kind: &'static str, frame: Value) -> Fut<'_, Result<Value, Failure>>;
}

/// Presentation calls over the frames, worded as the original guide's bridge face.
struct Face<'a> {
    ui: &'a dyn GuideUi,
}

impl Face<'_> {
    fn print(&self, text: &str) {
        self.ui.send(json!({"ui": "line", "text": text}));
    }
    fn banner(&self) {
        self.ui.send(json!({"ui": "banner"}));
    }
    fn step(&self, index: usize, states: &[&str], blurb: &str) {
        self.ui.send(json!({"ui": "step", "index": index, "steps": STEPS, "states": states, "blurb": unmarked(blurb)}));
    }
    fn spin(&self, text: &str) {
        self.ui.send(json!({"ui": "spin", "text": unmarked(text)}));
    }
    fn spin_update(&self, text: &str) {
        self.spin(text);
    }
    fn spin_stop(&self, line: Option<&str>) {
        self.ui.send(json!({"ui": "spin_stop"}));
        if let Some(line) = line {
            self.print(line);
        }
    }
    fn panel(&self, title: &str, text: &str) {
        self.ui.send(json!({"ui": "panel", "title": unmarked(title), "text": js::slice16(&unmarked(text), 0, Some(48 * 1024))}));
    }
    async fn ask(
        &self,
        label: &str,
        initial: &str,
        optional: bool,
        validate: &(dyn Fn(&str) -> Option<String> + Sync),
    ) -> Result<String, Failure> {
        loop {
            let answer = self
                .ui
                .ask(
                    "text",
                    json!({"ui": "ask", "label": label, "initial": initial, "optional": optional}),
                )
                .await?;
            let answer = match js_trim(
                &answer
                    .as_str()
                    .map(str::to_string)
                    .unwrap_or_else(|| answer.to_string()),
            ) {
                a if a.is_empty() => initial.to_string(),
                a => a,
            };
            match validate(&answer) {
                None => {
                    let shown = if unmarked(&answer).is_empty() {
                        s::dim("(none)")
                    } else {
                        unmarked(&answer)
                    };
                    self.print(&format!(
                        "  {} {label} {} {shown}",
                        s::cyan("?"),
                        s::dim("›")
                    ));
                    return Ok(answer);
                }
                Some(problem) => self.print(&format!("    {} {problem}", s::red("✖"))),
            }
        }
    }
    async fn choose<T: Clone>(
        &self,
        label: &str,
        options: &[(String, String, T)],
        initial: usize,
    ) -> Result<T, Failure> {
        let frame = json!({"ui": "choose", "label": label, "initial": initial, "options": options.iter().map(|(l, h, _)| json!({"label": unmarked(l), "hint": unmarked(h)})).collect::<Vec<_>>()});
        let index = self.ui.ask("choice", frame).await?.as_u64().unwrap_or(0) as usize;
        let chosen = options.get(index).unwrap_or(&options[0]);
        self.print(&format!(
            "  {} {label} {} {}",
            s::green("✔"),
            s::dim("·"),
            chosen.0
        ));
        Ok(chosen.2.clone())
    }
    async fn checklist(
        &self,
        label: &str,
        options: &[(String, String, bool)],
    ) -> Result<Vec<bool>, Failure> {
        let frame = json!({"ui": "checklist", "label": label, "options": options.iter().map(|(l, h, c)| json!({"label": unmarked(l), "hint": unmarked(h), "checked": c})).collect::<Vec<_>>()});
        let marks = self.ui.ask("checklist", frame).await?;
        let marks: Vec<bool> = (0..options.len())
            .map(|i| marks.get(i).and_then(Value::as_bool).unwrap_or(false))
            .collect();
        let chosen: Vec<&str> = options
            .iter()
            .zip(&marks)
            .filter(|(_, m)| **m)
            .map(|((l, _, _), _)| l.as_str())
            .collect();
        self.print(&format!(
            "  {} {label} {} {}",
            s::green("✔"),
            s::dim("·"),
            if chosen.is_empty() {
                "none".to_string()
            } else {
                chosen.join(", ")
            }
        ));
        Ok(marks)
    }
    async fn edit(&self, path: &str) -> Result<bool, Failure> {
        Ok(self
            .ui
            .ask("edit", json!({"ui": "edit", "path": path}))
            .await?
            .as_str()
            == Some("saved"))
    }
    fn finish(
        &self,
        title: String,
        summary: &str,
        next: Vec<(String, String)>,
        states: &[&str],
        project: &str,
    ) {
        self.ui.send(json!({"ui": "finish", "title": title, "summary": unmarked(summary), "next": next.iter().map(|(a, b)| json!([a, b])).collect::<Vec<_>>(), "steps": STEPS, "states": states, "project": project}));
    }
}

/// The guide's io for the commands it runs: files only, no prompts.
struct GuideIo {
    signal: Signal,
}

impl Io for GuideIo {
    fn signal(&self) -> Signal {
        self.signal.clone()
    }
    fn interactive(&self) -> bool {
        false
    }
    fn read_file(&self, path: &str) -> Result<String, Failure> {
        std::fs::read(path)
            .map(|b| String::from_utf8_lossy(&b).into_owned())
            .map_err(|e| fs_failure(&e, path, false))
    }
    fn write_file(&self, path: &str, text: &str) -> Result<(), Failure> {
        std::fs::write(path, text).map_err(|e| fs_failure(&e, path, true))
    }
    fn read_stdin(&self) -> Fut<'_, Result<String, Failure>> {
        Box::pin(async { Err(Failure::Usage("The guide reads no standard input.".into())) })
    }
    fn prompt(&self, _question: String) -> Fut<'_, Result<String, Failure>> {
        Box::pin(async { Ok(String::new()) })
    }
    fn confirm(&self, _question: String) -> Fut<'_, Result<bool, Failure>> {
        Box::pin(async { Ok(true) })
    }
    fn show(&self, _text: &str) -> Result<(), Failure> {
        Ok(())
    }
    fn note(&self, _text: &str) {}
    fn emit(&self, _event: &J) {}
}

/// The Git root of `cwd`, or None outside a checkout.
pub fn git_root(cwd: &Path) -> Option<PathBuf> {
    let mut command = std::process::Command::new("git");
    command
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(cwd)
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(windows)]
    std::os::windows::process::CommandExt::creation_flags(&mut command, 0x08000000);
    let output = command.output().ok()?;
    if !output.status.success() {
        return None;
    }
    Some(resolve(cwd, String::from_utf8_lossy(&output.stdout).trim()))
}

/// `path.isAbsolute` for the platform.
pub fn is_absolute(value: &str) -> bool {
    if cfg!(windows) {
        let bytes = value.as_bytes();
        value.starts_with(['/', '\\'])
            || (bytes.len() >= 3
                && bytes[0].is_ascii_alphabetic()
                && bytes[1] == b':'
                && (bytes[2] == b'/' || bytes[2] == b'\\'))
    } else {
        value.starts_with('/')
    }
}

/// `path.resolve(base, value)`: lexically normalized, with the platform's separators.
pub fn resolve(base: &Path, value: &str) -> PathBuf {
    let joined = if is_absolute(value) {
        PathBuf::from(value)
    } else {
        base.join(value)
    };
    let mut out = PathBuf::new();
    for component in joined.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// `path.basename(value)`: the last segment, ignoring trailing separators.
pub fn basename(value: &str) -> String {
    let trimmed = value.trim_end_matches(|c| c == '/' || (cfg!(windows) && c == '\\'));
    let at = trimmed
        .rfind(|c| c == '/' || (cfg!(windows) && c == '\\'))
        .map(|i| i + 1)
        .unwrap_or(0);
    let name = &trimmed[at..];
    if cfg!(windows) && at == 0 && name.len() == 2 && name.ends_with(':') {
        return String::new();
    }
    name.to_string()
}

fn same_path(a: &str, b: &Path) -> bool {
    let normal = |p: &Path| {
        p.to_string_lossy()
            .trim_end_matches(['\\', '/'])
            .to_string()
    };
    let (x, y) = (normal(&resolve(Path::new(""), a)), normal(b));
    if cfg!(windows) {
        x.to_lowercase() == y.to_lowercase()
    } else {
        x == y
    }
}

fn slug(text: &str) -> String {
    let lower = text.to_lowercase();
    let mut out = String::new();
    let mut dash = false;
    for c in lower.chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            out.push(c);
            dash = false;
        } else if !dash {
            out.push('-');
            dash = true;
        }
    }
    let out = out
        .trim_start_matches(|c: char| !c.is_ascii_lowercase())
        .trim_end_matches('-');
    let out: String = out.chars().take(40).collect();
    if out.is_empty() {
        "app".into()
    } else {
        out
    }
}

fn short_hash(hash: &J) -> String {
    match hash {
        J::Str(text) if js::len16(text) > 20 => format!("{}…", js::slice16(text, 0, Some(19))),
        other => other.js_or_empty(),
    }
}

fn tone(text: &str, kind: &str) -> String {
    match kind {
        "success" => s::green(text),
        "active" => s::cyan(text),
        "danger" => s::red(text),
        "attention" => s::yellow(text),
        "interrupted" => s::magenta(text),
        _ => s::dim(text),
    }
}

fn baseline_tone(status: &str) -> &'static str {
    match status {
        "passed" => "success",
        "failed" | "blocked" => "danger",
        "cancelled" => "neutral",
        "interrupted" => "interrupted",
        "running" => "active",
        _ => "neutral",
    }
}

fn valid_id(value: &str) -> bool {
    let mut chars = value.chars();
    chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// The guide's outcome: the exit code and what it set up.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Outcome {
    pub code: i32,
    pub project: Option<String>,
    pub ticket: Option<String>,
}

enum Exit {
    Left,
    Failed(Failure),
}

impl From<Failure> for Exit {
    fn from(failure: Failure) -> Self {
        Exit::Failed(failure)
    }
}

struct Guide<'a> {
    face: Face<'a>,
    client: &'a Client,
    io: GuideIo,
    states: Vec<&'static str>,
    done: std::sync::Mutex<Vec<String>>,
    name: String,
}

impl Guide<'_> {
    async fn act(
        &self,
        id: &str,
        args: Vec<String>,
        flags: Vec<(String, FlagValue)>,
    ) -> Result<J, Failure> {
        let command = by_id(id).expect("a cataloged command");
        crate::commands::run(command, self.client, &Input { args, flags }, &self.io).await
    }
    fn push_done(&self, text: String) {
        self.done.lock().unwrap().push(text);
    }
    fn begin(&mut self, index: usize, blurb: &str) {
        self.states[index] = "current";
        self.face.step(index, &self.states, blurb);
    }
    fn finish(&mut self, index: usize, outcome: &'static str) {
        self.states[index] = outcome;
    }
    fn ok(&self, text: &str) {
        self.face.print(&format!("  {} {text}", s::green("✔")));
    }
    fn warn(&self, text: &str) {
        for (line, row) in wrap(text, WIDTH - 6).into_iter().enumerate() {
            self.face.print(&format!(
                "  {} {row}",
                if line > 0 {
                    " ".to_string()
                } else {
                    s::yellow("!")
                }
            ));
        }
    }
    fn note(&self, text: &str) {
        for row in wrap(text, WIDTH - 6) {
            self.face.print(&format!("    {}", s::dim(&row)));
        }
    }
    /// Runs `work`; a refusal is explained with the shared failure text and the developer picks
    /// retry, skip or leave. Ok(None) is a skipped step.
    async fn attempt<F, Fu>(
        &self,
        label: &str,
        mut work: F,
        skippable: bool,
    ) -> Result<Option<J>, Exit>
    where
        F: FnMut() -> Fu,
        Fu: std::future::Future<Output = Result<J, Failure>>,
    {
        loop {
            match work().await {
                Ok(value) => return Ok(Some(value)),
                Err(failure) => {
                    if self.io.signal.aborted()
                        || failure == Failure::Aborted && self.io.signal.aborted()
                    {
                        return Err(Exit::Left);
                    }
                    let message = match &failure {
                        Failure::Refusal(m) | Failure::Usage(m) => format!("{label} refused: {m}"),
                        other => explain_failure(label, other),
                    };
                    for (line, row) in wrap(&message, WIDTH - 6).into_iter().enumerate() {
                        self.face.print(&format!(
                            "  {} {row}",
                            if line > 0 {
                                " ".to_string()
                            } else {
                                s::red("✖")
                            }
                        ));
                    }
                    let mut options = vec![("Try again".to_string(), String::new(), "retry")];
                    if skippable {
                        options.push(("Skip this step".into(), String::new(), "skip"));
                    }
                    options.push(("Leave the guide".into(), String::new(), "quit"));
                    match self
                        .face
                        .choose("What now?", &options, 0)
                        .await
                        .map_err(|f| self.left_or(f))?
                    {
                        "quit" => return Err(Exit::Left),
                        "skip" => return Ok(None),
                        _ => {}
                    }
                }
            }
        }
    }
    fn left_or(&self, failure: Failure) -> Exit {
        if self.io.signal.aborted() || failure == Failure::Aborted {
            Exit::Left
        } else {
            Exit::Failed(failure)
        }
    }
}

/// Runs the guide over `ui` with `client`; `runner` runs the component CLIs.
pub async fn prime(
    client: &Client,
    ui: &dyn GuideUi,
    runner: &dyn Runner,
    flags: &[(String, String)],
    cwd: &Path,
    signal: Signal,
    name: &str,
) -> Result<Outcome, Failure> {
    let mut guide = Guide {
        face: Face { ui },
        client,
        io: GuideIo {
            signal: signal.clone(),
        },
        states: STEPS.iter().map(|_| "pending").collect(),
        done: std::sync::Mutex::new(Vec::new()),
        name: name.to_string(),
    };
    let mut outcome = Outcome::default();
    let result = run(&mut guide, runner, flags, cwd, &mut outcome).await;
    match result {
        Ok(code) => {
            outcome.code = code;
            Ok(outcome)
        }
        Err(exit) => {
            let left = match exit {
                Exit::Left => true,
                Exit::Failed(failure) => {
                    if !signal.aborted() {
                        return Err(failure);
                    }
                    true
                }
            };
            debug_assert!(left);
            guide.face.print("");
            guide.note(&format!("Left the guide. Nothing was undone; run {} init again to continue where the runtime's records stand.", guide.name));
            outcome.code = 130;
            Ok(outcome)
        }
    }
}

fn flag<'a>(flags: &'a [(String, String)], name: &str) -> Option<&'a str> {
    flags
        .iter()
        .rev()
        .find(|(n, _)| n == name)
        .map(|(_, v)| v.as_str())
        .filter(|v| !v.is_empty())
}

async fn run(
    g: &mut Guide<'_>,
    runner: &dyn Runner,
    flags: &[(String, String)],
    cwd: &Path,
    outcome: &mut Outcome,
) -> Result<i32, Exit> {
    g.face.banner();

    // ── 1 runtime ──────────────────────────────────────────────────────────
    g.begin(0, "Luuna's runtime owns every decision: approvals, baselines, validation and publication. The guide only asks it.");
    g.face.spin("Connecting to the runtime");
    let health = match g.act("health", vec![], vec![]).await {
        Ok(health) => health,
        Err(failure) => {
            if g.io.signal.aborted() {
                return Err(Exit::Left);
            }
            g.face.spin_stop(Some(&format!(
                "  {} {}",
                s::red("✖"),
                explain_failure("Connection", &failure)
            )));
            g.note(&format!(
                "Start Luuna (Start Luuna.cmd), then run {} init again.",
                g.name
            ));
            return Ok(1);
        }
    };
    g.face.spin_stop(None);
    let status = health.get("status").js();
    let build = health.get("build_identity");
    g.ok(&format!(
        "Runtime {} · Pi {}{}",
        tone(&status, if status == "ok" { "success" } else { "danger" }),
        health.get("pi_version").js(),
        if build.truthy() {
            s::dim(&format!(" · build {}", short_hash(build)))
        } else {
            String::new()
        }
    ));
    if health.get("native_execution").truthy() {
        g.warn("Approved writers and checks run natively with your host permissions. Prompts restrict tools; they are not a sandbox.");
    }
    g.finish(0, "done");

    // ── 2 project ──────────────────────────────────────────────────────────
    g.begin(1, "Registration records where each repository lives on this machine. Paths stay in the local registry; the manifest never carries them.");
    let field = |value: &J, key: &str| -> Result<J, Failure> {
        if value.nullish() {
            Err(Failure::type_error())
        } else {
            Ok(value.get(key).clone())
        }
    };
    let mut view: J;
    if let Some(project) = flag(flags, "project") {
        let project = project.to_string();
        view = g
            .attempt(
                "Project inspection",
                || g.act("projects.show", vec![project.clone()], vec![]),
                false,
            )
            .await?
            .unwrap_or(J::Null);
        let p = field(&view, "project")?;
        g.ok(&format!(
            "Continuing with {} {}",
            s::bold(&field(&p, "name")?.js()),
            s::dim(&format!("({})", field(&p, "id")?.js()))
        ));
    } else {
        let root = match flag(flags, "path") {
            Some(path) => Some(resolve(cwd, path)),
            None => git_root(cwd),
        };
        g.face
            .spin("Looking for this checkout among registered projects");
        let found: Result<Option<J>, Failure> = async {
            let projects = g.act("projects.list", vec![], vec![]).await?;
            if let Some(root) = &root {
                let list = match &projects {
                    J::Arr(items) => items.clone(),
                    J::Str(text) => text.chars().map(|c| J::str(c.to_string())).collect(),
                    _ => return Err(Failure::type_error()),
                };
                for project in list {
                    let candidate = g
                        .act("projects.show", vec![field(&project, "id")?.js()], vec![])
                        .await?;
                    let registrations = field(&candidate, "repositories")?;
                    let items = registrations.as_arr().ok_or_else(Failure::type_error)?;
                    let mut matched = false;
                    for registration in items {
                        if same_path(&field(registration, "local_path")?.js(), root) {
                            matched = true;
                            break;
                        }
                    }
                    if matched {
                        return Ok(Some(candidate));
                    }
                }
            }
            Ok(None)
        }
        .await;
        g.face.spin_stop(None);
        let found = found.map_err(|f| g.left_or(f))?;
        let root_text = root.as_ref().map(|r| r.to_string_lossy().into_owned());
        if let Some(candidate) = found {
            view = candidate;
            let p = field(&view, "project")?;
            g.ok(&format!(
                "{} is already registered as {} {}",
                root_text.clone().unwrap_or_default(),
                s::bold(&field(&p, "name")?.js()),
                s::dim(&format!("({})", field(&p, "id")?.js()))
            ));
            let options = [
                (
                    "Continue with this project".to_string(),
                    String::new(),
                    "continue",
                ),
                ("Leave the guide".to_string(), String::new(), "quit"),
            ];
            if g.face
                .choose("Continue setting it up?", &options, 0)
                .await
                .map_err(|f| g.left_or(f))?
                == "quit"
            {
                return Err(Exit::Left);
            }
        } else {
            if let Some(root) = &root_text {
                g.note(&format!("Found a Git checkout at {root}."));
            }
            let absolute = |value: &str| {
                (!is_absolute(value))
                    .then(|| "Give the absolute path of the repository root.".to_string())
            };
            let repository_root = g
                .face
                .ask(
                    "Repository root",
                    root_text.as_deref().unwrap_or(""),
                    false,
                    &absolute,
                )
                .await
                .map_err(|f| g.left_or(f))?;
            let base = basename(&repository_root);
            let project_name = g
                .face
                .ask("Project name", &base, false, &|v: &str| {
                    v.is_empty().then(|| "A project needs a name.".to_string())
                })
                .await
                .map_err(|f| g.left_or(f))?;
            let repository_id = g
                .face
                .ask("Repository id", &slug(&base), false, &|v: &str| {
                    (!valid_id(v)).then(|| {
                        "Use lowercase letters, digits and dashes, starting with a letter."
                            .to_string()
                    })
                })
                .await
                .map_err(|f| g.left_or(f))?;
            let mut extra: Vec<String> = Vec::new();
            loop {
                let options = [
                    ("No, one is enough for now".to_string(), String::new(), "no"),
                    ("Add another repository".to_string(), String::new(), "add"),
                ];
                if g.face
                    .choose("Does this project span another repository?", &options, 0)
                    .await
                    .map_err(|f| g.left_or(f))?
                    != "add"
                {
                    break;
                }
                let other_root = g
                    .face
                    .ask("Its repository root", "", false, &absolute)
                    .await
                    .map_err(|f| g.left_or(f))?;
                let taken = extra.clone();
                let first = repository_id.clone();
                let other_id = g
                    .face
                    .ask(
                        "Its repository id",
                        &slug(&basename(&other_root)),
                        false,
                        &move |v: &str| {
                            (!(valid_id(v)
                                && v != first
                                && !taken.iter().any(|row| row.starts_with(&format!("{v}=")))))
                            .then(|| {
                                "Use a new id: lowercase letters, digits and dashes.".to_string()
                            })
                        },
                    )
                    .await
                    .map_err(|f| g.left_or(f))?;
                extra.push(format!("{other_id}={other_root}"));
            }
            let flags_for = if extra.is_empty() {
                vec![]
            } else {
                vec![("repository".to_string(), FlagValue::List(extra.clone()))]
            };
            view = g
                .attempt(
                    "Registration",
                    || {
                        g.act(
                            "projects.register",
                            vec![
                                project_name.clone(),
                                repository_id.clone(),
                                repository_root.clone(),
                            ],
                            flags_for.clone(),
                        )
                    },
                    false,
                )
                .await?
                .unwrap_or(J::Null);
            let p = field(&view, "project")?;
            g.ok(&format!(
                "Registered {} {}",
                s::bold(&field(&p, "name")?.js()),
                s::dim(&format!("({})", field(&p, "id")?.js()))
            ));
            g.push_done(format!("registered {}", field(&p, "id")?.js()));
        }
    }
    for registration in field(&view, "repositories")?
        .as_arr()
        .ok_or_else(Failure::type_error)?
        .clone()
    {
        let observation = registration.get("observation");
        let dirty_of = |k: &str| observation.get("dirty").get(k).as_f64().unwrap_or(0.0);
        g.face.print(&format!(
            "    {}  {}",
            s::cyan(&field(&registration, "repository_id")?.js()),
            field(&registration, "local_path")?.js()
        ));
        if observation.truthy() {
            let dirty = dirty_of("staged") + dirty_of("unstaged") + dirty_of("untracked");
            let remote = observation.get("identity").get("remote");
            g.note(&format!(
                "{} @ {} · {}{}",
                observation.get("branch").or(&J::str("detached HEAD")).js(),
                short_hash(observation.get("head_commit")),
                if remote.nullish() {
                    "no portable origin".to_string()
                } else {
                    remote.js()
                },
                if dirty != 0.0 {
                    format!(" · {} uncommitted change(s)", js::number_to_string(dirty))
                } else {
                    " · clean".to_string()
                }
            ));
        }
    }
    let project_id = field(&field(&view, "project")?, "id")?.js();
    outcome.project = Some(project_id.clone());
    g.finish(1, "done");

    // ── 3 discovery ────────────────────────────────────────────────────────
    g.begin(2, "Discovery reads the repositories and proposes a manifest: stack profiles and the checks that prove a change. It runs nothing and authorizes nothing.");
    let mut discovery: Option<J> = None;
    let mut rediscover = true;
    if view.get("manifest").truthy() {
        let manifest = view.get("manifest");
        g.ok(&format!(
            "Manifest r{} is already confirmed {}",
            manifest.get("revision").js(),
            s::dim(&short_hash(manifest.get("hash")))
        ));
        let options = [
            (
                "Keep the confirmed manifest".to_string(),
                String::new(),
                false,
            ),
            ("Run discovery again".to_string(), String::new(), true),
        ];
        rediscover = g
            .face
            .choose("Propose a new revision?", &options, 0)
            .await
            .map_err(|f| g.left_or(f))?;
    }
    if rediscover {
        g.face.spin("Discovering (read-only)");
        let found = g
            .attempt(
                "Discovery",
                || async {
                    let result = g
                        .act("projects.discover", vec![project_id.clone()], vec![])
                        .await;
                    g.face.spin_stop(None);
                    result
                },
                true,
            )
            .await?;
        discovery = found;
        if let Some(d) = &discovery {
            for repository in d.get("repositories").items() {
                let profiles = repository.get("profiles");
                g.face.print(&format!(
                    "    {}  {}",
                    s::cyan(&repository.get("repository_id").js()),
                    if profiles.items().is_empty() {
                        s::dim("no stack profile recognized")
                    } else {
                        crate::js::J::Arr(profiles.items().to_vec())
                            .items()
                            .iter()
                            .map(J::js)
                            .collect::<Vec<_>>()
                            .join(", ")
                    }
                ));
                for proposed in repository.get("proposed_checks").items() {
                    let check = proposed.get("check");
                    let mut parts = vec![check.get("executable").js_or_empty()];
                    parts.extend(check.get("args").items().iter().map(J::js_or_empty));
                    let required = check.get("required").truthy();
                    let missing = proposed.get("missing_tools");
                    g.face.print(&truncate(
                        &format!(
                            "      {} {} {}  {}{}",
                            if required {
                                s::green("■")
                            } else {
                                s::dim("□")
                            },
                            js::pad_end(&check.get("id").js(), 12),
                            parts.join(" "),
                            s::dim(&format!(
                                "{} · {}",
                                if required { "required" } else { "optional" },
                                proposed.get("confidence").js()
                            )),
                            if proposed.get("capability").as_str() == Some("missing") {
                                format!(
                                    "  {}",
                                    s::yellow(&format!(
                                        "missing {}",
                                        if missing.nullish() {
                                            "tools".to_string()
                                        } else {
                                            missing
                                                .items()
                                                .iter()
                                                .map(J::js_or_empty)
                                                .collect::<Vec<_>>()
                                                .join(", ")
                                        }
                                    ))
                                )
                            } else {
                                String::new()
                            }
                        ),
                        WIDTH - 1,
                    ));
                }
                if repository.get("proposed_checks").items().is_empty() {
                    g.note(
                        "No checks proposed: a baseline will offer the testing-setup prerequisite.",
                    );
                }
            }
            for asked in d.get("questions").items() {
                let repository = asked.get("repository_id");
                g.warn(&format!(
                    "{}{}: {}",
                    if repository.truthy() {
                        format!("{} · ", repository.js())
                    } else {
                        String::new()
                    },
                    asked.get("topic").js(),
                    asked.get("detail").js()
                ));
            }
            g.finish(2, "done");
        } else {
            g.finish(2, "skipped");
        }
    } else {
        g.finish(2, "skipped");
    }

    // ── 4 manifest ─────────────────────────────────────────────────────────
    g.begin(3, "Confirming records one exact manifest revision. Plans, baselines and approvals bind to it; a later change makes a new revision.");
    if let Some(d) = &discovery {
        let mut draft = d.get("proposed_manifest").clone();
        let mut edited = false;
        let problems: Vec<String> = d
            .get("manifest_problems")
            .items()
            .iter()
            .map(J::js)
            .collect();
        let policy = draft.get("policy");
        g.face.print(&format!(
            "    {}  approval {} · publish {} · merge {}",
            s::dim("policy"),
            policy.get("approval").or(&J::str("plan")).js(),
            policy.get("publish").or(&J::str("automatic_pr")).js(),
            s::bold(&policy.get("merge").or(&J::str("human")).js())
        ));
        for repository in draft.get("repositories").items() {
            let remote = repository.get("remote");
            let required = repository
                .get("checks")
                .items()
                .iter()
                .filter(|c| c.get("required").truthy())
                .count();
            g.face.print(&truncate(
                &format!(
                    "    {}  {} · {} · {} · {} required check(s)",
                    s::cyan(&repository.get("id").js()),
                    if remote.truthy() {
                        remote.js()
                    } else {
                        s::yellow("no remote")
                    },
                    repository.get("default_branch").js(),
                    repository.get("execution").js(),
                    required
                ),
                WIDTH - 1,
            ));
        }
        for problem in &problems {
            g.warn(&format!("Needs an edit: {problem}"));
        }
        let directory = std::env::temp_dir().join(format!(
            "luuna-prime-{}",
            &crate::commands::random_uuid()[..6]
        ));
        let _ = std::fs::create_dir_all(&directory);
        let file = directory.join(format!("{project_id}-manifest.json"));
        let file_text = file.to_string_lossy().into_owned();
        let result: Result<(), Exit> = async {
            loop {
                let missing: Vec<usize> = draft.get("repositories").items().iter().enumerate().filter(|(_, r)| !r.get("checks").items().iter().any(|c| c.get("required").truthy())).map(|(i, _)| i).collect();
                let mut options: Vec<(String, String, &str)> = vec![("Confirm it".into(), "the runtime validates it".into(), "confirm")];
                if !missing.is_empty() {
                    let names: Vec<String> = missing.iter().map(|i| draft.get("repositories").at(*i).get("id").js()).collect();
                    options.push(("Add a required check".into(), format!("{} has none yet", names.join(", ")), "check"));
                }
                options.push(("Review it here".into(), "the complete JSON".into(), "review"));
                options.push(("Edit it first".into(), "opens your editor".into(), "edit"));
                options.push(("Skip for now".into(), format!("{} confirm {project_id} <file> later", g.name), "skip"));
                let initial = if !missing.is_empty() { 1 } else if !problems.is_empty() && !edited { 3 } else { 0 };
                let label = if edited { "Your edited manifest is ready" } else { "The proposed manifest" };
                match g.face.choose(label, &options, initial).await.map_err(|f| g.left_or(f))? {
                    "skip" => {
                        g.finish(3, "skipped");
                        return Ok(());
                    }
                    "check" => {
                        let target = if missing.len() > 1 {
                            let choices: Vec<(String, String, usize)> = missing.iter().map(|i| (draft.get("repositories").at(*i).get("id").js(), String::new(), *i)).collect();
                            g.face.choose("Which repository?", &choices, 0).await.map_err(|f| g.left_or(f))?
                        } else {
                            missing[0]
                        };
                        let repository = draft.get("repositories").at(target).clone();
                        let suggested = repository
                            .get("profiles")
                            .items()
                            .iter()
                            .find_map(|p| match p.as_str() {
                                Some("python") => Some("python -m unittest -v"),
                                Some("node" | "node-typescript" | "react" | "nextjs") => Some("npm test"),
                                Some("rust") => Some("cargo test"),
                                Some("go") => Some("go test ./..."),
                                _ => None,
                            })
                            .unwrap_or("");
                        let line = g.face.ask("Command that runs the tests", suggested, false, &|v: &str| js_trim(v).is_empty().then(|| "Give the command, for example python -m unittest -v.".to_string())).await.map_err(|f| g.left_or(f))?;
                        let words: Vec<String> = js_trim(&line).split_whitespace().map(str::to_string).collect();
                        let existing = repository.get("checks").items().to_vec();
                        let id = if existing.iter().any(|c| c.get("id").as_str() == Some("tests")) { format!("tests-{}", existing.len() + 1) } else { "tests".to_string() };
                        let mut checks = existing;
                        checks.push(crate::obj![("id", id.clone()), ("executable", words[0].clone()), ("args", J::Arr(words[1..].iter().cloned().map(J::Str).collect())), ("required", true), ("timeout_seconds", 600.0), ("capabilities", J::Arr(vec![]))]);
                        if let J::Obj(entries) = &mut draft {
                            if let Some((_, J::Arr(repositories))) = entries.iter_mut().find(|(k, _)| k == "repositories") {
                                repositories[target].set("checks", J::Arr(checks));
                            }
                        }
                        edited = true;
                        g.ok(&format!("Added required check {} to {}: {}", s::bold(&id), repository.get("id").js(), s::cyan(&js_trim(&line))));
                        g.note("The runtime checks it when you confirm, and the baseline runs it on your checkout.");
                    }
                    "review" => g.face.panel(&format!("Manifest draft for {project_id}"), &draft.stringify_pretty().unwrap_or_default()),
                    "edit" => {
                        let _ = std::fs::write(&file, format!("{}\n", draft.stringify_pretty().unwrap_or_default()));
                        g.note(&format!("Editing {file_text}. Save and close the editor to come back."));
                        g.face.edit(&file_text).await.map_err(|f| g.left_or(f))?;
                        let options = [("Read my saved edits".to_string(), String::new(), true), ("Keep the previous draft".to_string(), String::new(), false)];
                        if !g.face.choose("Done editing?", &options, 0).await.map_err(|f| g.left_or(f))? {
                            continue;
                        }
                        match std::fs::read(&file).ok().and_then(|b| js::parse(&String::from_utf8_lossy(&b)).ok()) {
                            Some(value) => {
                                draft = value;
                                edited = true;
                                g.ok("Read your edits.");
                            }
                            None => g.warn("That file is not valid JSON; the previous draft is kept."),
                        }
                    }
                    _ => {
                        let _ = std::fs::write(&file, format!("{}\n", draft.stringify_pretty().unwrap_or_default()));
                        let confirmed = g.attempt("Manifest confirmation", || g.act("projects.manifest.confirm", vec![project_id.clone(), file_text.clone()], vec![]), true).await?;
                        let Some(confirmed) = confirmed else {
                            g.finish(3, "skipped");
                            return Ok(());
                        };
                        view = confirmed;
                        let manifest = view.get("manifest");
                        g.ok(&format!("Manifest r{} confirmed {}", manifest.get("revision").js(), s::dim(&short_hash(manifest.get("hash")))));
                        g.push_done(format!("manifest r{}", manifest.get("revision").js()));
                        g.finish(3, "done");
                        return Ok(());
                    }
                }
            }
        }
        .await;
        let _ = std::fs::remove_dir_all(&directory);
        result?;
    } else if view.get("manifest").truthy() {
        g.ok(&format!(
            "Keeping manifest r{}.",
            view.get("manifest").get("revision").js()
        ));
        g.finish(3, "done");
    } else {
        g.note("No proposal to confirm.");
        g.finish(3, "skipped");
    }

    // ── 5 baseline ─────────────────────────────────────────────────────────
    g.begin(4, "A baseline runs the confirmed required checks on your checkout and records the exact evidence. Feature plans need a usable testing setup.");
    if !view.get("manifest").truthy() {
        g.note(&format!(
            "A baseline needs a confirmed manifest. Later: {} baseline {project_id} <repository>.",
            g.name
        ));
        g.finish(4, "skipped");
    } else {
        let repositories: Vec<String> = view
            .get("repositories")
            .items()
            .iter()
            .map(|r| r.get("repository_id").js())
            .collect();
        let options = [
            (
                "Run it now".to_string(),
                "checks run with your permissions".to_string(),
                true,
            ),
            ("Skip for now".to_string(), String::new(), false),
        ];
        let label = format!(
            "Run the baseline{}?",
            if repositories.len() > 1 {
                format!(" for {} repositories", repositories.len())
            } else {
                String::new()
            }
        );
        if !g
            .face
            .choose(&label, &options, 0)
            .await
            .map_err(|f| g.left_or(f))?
        {
            g.finish(4, "skipped");
        } else {
            for repository in &repositories {
                let registration = view
                    .get("repositories")
                    .items()
                    .iter()
                    .find(|row| row.get("repository_id").as_str() == Some(repository.as_str()));
                if let Some(root) = registration
                    .and_then(|row| row.get("local_path").as_str())
                    .filter(|root| Path::new(root).is_dir())
                {
                    let root = Path::new(root);
                    let changes =
                        crate::setup::git(root, &["status", "--short", "--untracked-files=normal"])
                            .await
                            .map_err(|error| g.left_or(Failure::error(error.to_string())))?;
                    if !changes.trim().is_empty() {
                        g.face.panel("Uncommitted changes", &changes);
                        let stash = g.face.choose("The baseline needs a clean checkout. How would you like to continue?", &[
                            ("Stash changes and continue".to_string(), "saves staged, unstaged and untracked files; restore them later with git stash pop".to_string(), true),
                            ("Leave my changes and skip this baseline".to_string(), "clean up in your Git client, then run the baseline again".to_string(), false),
                        ], 1).await.map_err(|failure| g.left_or(failure))?;
                        if !stash {
                            continue;
                        }
                        crate::setup::git(
                            root,
                            &[
                                "stash",
                                "push",
                                "--include-untracked",
                                "--message",
                                "Luuna setup: saved before baseline",
                            ],
                        )
                        .await
                        .map_err(|error| g.left_or(Failure::error(error.to_string())))?;
                        g.note("Your changes are saved in Git's stash. Luuna will not restore or discard them automatically. Restore them later with git stash pop.");
                    }
                }
                let started = g
                    .attempt(
                        "Baseline",
                        || {
                            g.act(
                                "baselines.start",
                                vec![project_id.clone(), repository.clone()],
                                vec![],
                            )
                        },
                        true,
                    )
                    .await?;
                let Some(mut run) = started else { continue };
                g.face.spin(&format!(
                    "Baseline {repository} · {}",
                    run.get("state").js()
                ));
                let polled: Result<(), Failure> = async {
                    while !is_terminal(&run.get("state").js()) {
                        g.io.signal.sleep(POLL_MS).await?;
                        run = g.act("runs.show", vec![run.get("id").js()], vec![]).await?;
                        g.face.spin_update(&format!(
                            "Baseline {repository} · {}",
                            run.get("state").js()
                        ));
                    }
                    Ok(())
                }
                .await;
                g.face.spin_stop(None);
                polled.map_err(|f| g.left_or(f))?;
                let listed = g
                    .act("baselines.list", vec![project_id.clone()], vec![])
                    .await
                    .map_err(|f| g.left_or(f))?;
                let evidence = listed
                    .items()
                    .iter()
                    .find(|b| b.get("run_id") == run.get("id"))
                    .cloned();
                let Some(evidence) = evidence else {
                    g.warn(&format!(
                        "Run {} ended {} without recorded evidence. Inspect it: {} run {}",
                        run.get("id").js(),
                        run.get("state").js(),
                        g.name,
                        run.get("id").js()
                    ));
                    continue;
                };
                let status = evidence.get("status").js();
                g.face.print(&format!(
                    "  {} {} baseline {} {}",
                    if status == "passed" {
                        s::green("✔")
                    } else {
                        s::red("✖")
                    },
                    s::cyan(repository),
                    tone(&status, baseline_tone(&status)),
                    s::dim(&format!(
                        "· {}/{} required · {}",
                        evidence.get("results").items().len(),
                        evidence.get("required_checks").items().len(),
                        run.get("id").js()
                    ))
                ));
                for check in evidence.get("results").items() {
                    let check_status = check.get("status").js();
                    let exit = check.get("exit_code");
                    let tests = check.get("tests_detected");
                    g.face.print(&truncate(
                        &format!(
                            "      {}  {}{}{}",
                            tone(&check_status, baseline_tone(&check_status)),
                            check.get("recipe_id").js(),
                            if exit.nullish() {
                                String::new()
                            } else {
                                s::dim(&format!(" exit {}", exit.js()))
                            },
                            if tests.is_undefined() {
                                String::new()
                            } else {
                                s::dim(&format!(" · {} test(s)", tests.js()))
                            }
                        ),
                        WIDTH - 1,
                    ));
                }
                let reason = evidence.get("reason");
                if reason.truthy() {
                    g.note(&reason.js());
                    let step = baseline_step(&status, &reason.js());
                    if !step.is_empty() && !evidence.get("prerequisite").truthy() {
                        g.note(&format!("next: {step}"));
                    }
                }
                g.push_done(format!("baseline {repository} {status}"));
            }
            g.finish(4, "done");
            view = g
                .act("projects.show", vec![project_id.clone()], vec![])
                .await
                .map_err(|f| g.left_or(f))?;
        }
    }

    // ── 6 suite ────────────────────────────────────────────────────────────
    g.begin(5, "The suite keeps the project's work in managed stores the runtime owns: Missions holds tickets and plans, Atlas holds memory with provenance, Beacon holds versioned prompts.");
    let suite = [
        ("Missions", "tickets and structured plans"),
        ("Atlas", "expertise and project memory"),
        ("Beacon", "composable, versioned prompts"),
    ];
    let show_suite = |g: &Guide<'_>, enabled: bool| {
        for (brand, what) in suite {
            g.face.print(&format!(
                "    {} {} {what}",
                if enabled {
                    s::green("●")
                } else {
                    s::dim("○")
                },
                s::bold(&js::pad_end(brand, 9))
            ));
        }
    };
    let mut status = g
        .attempt(
            "Component status",
            || g.act("components.status", vec![project_id.clone()], vec![]),
            true,
        )
        .await?;
    if status.as_ref().is_some_and(|st| st.get("enabled").truthy()) {
        let st = status.as_ref().unwrap();
        show_suite(g, true);
        let path = st.get("workspace_path");
        g.ok(&format!(
            "Already enabled{}",
            if path.truthy() {
                s::dim(&format!(" · {}", path.js()))
            } else {
                String::new()
            }
        ));
        g.finish(5, "done");
    } else if let Some(st) = status.clone() {
        show_suite(g, false);
        if st.get("reason").truthy() {
            g.note(&st.get("reason").js());
        }
        let options = [
            (
                "Enable Missions, Atlas and Beacon".to_string(),
                String::new(),
                true,
            ),
            (
                "Skip for now".to_string(),
                format!("{} enable-components {project_id}", g.name),
                false,
            ),
        ];
        if g.face
            .choose("Enable the suite for this project?", &options, 0)
            .await
            .map_err(|f| g.left_or(f))?
        {
            g.face.spin("Initializing the managed stores");
            let enabled = g
                .attempt(
                    "Component activation",
                    || g.act("components.enable", vec![project_id.clone()], vec![]),
                    true,
                )
                .await;
            g.face.spin_stop(None);
            let enabled = enabled?;
            let skipped = enabled.is_none();
            let current = enabled.unwrap_or(st.clone());
            if current.get("enabled").truthy() {
                show_suite(g, true);
                let path = current.get("workspace_path");
                g.ok(&format!(
                    "Suite enabled{}",
                    if path.truthy() {
                        s::dim(&format!(" · {}", path.js()))
                    } else {
                        String::new()
                    }
                ));
                g.push_done("suite enabled".into());
            } else {
                if current.get("reason").truthy() {
                    g.warn(&current.get("reason").js());
                }
                // ADR 0014: an unfinished activation freezes ticket actions until it is reconciled.
                if skipped || current.get("reason").truthy() {
                    g.note(&format!("While activation is unfinished the runtime freezes ticket actions. Fix the component toolchain (npm run components:verify names what differs), then {} enable-components {project_id} retries it.", g.name));
                }
            }
            status = Some(current);
        }
        let enabled = status.as_ref().is_some_and(|st| st.get("enabled").truthy());
        g.finish(5, if enabled { "done" } else { "skipped" });
    } else {
        g.finish(5, "skipped");
    }
    if status.as_ref().is_some_and(|st| st.get("enabled").truthy()) {
        let options = [
            ("Not now".to_string(), String::new(), false),
            (
                "Record a project memory".to_string(),
                "for example how tests run".to_string(),
                true,
            ),
        ];
        if g.face
            .choose("Teach Atlas one thing about this project?", &options, 0)
            .await
            .map_err(|f| g.left_or(f))?
        {
            let content = g
                .face
                .ask("What should agents know?", "", false, &|v: &str| {
                    v.is_empty()
                        .then(|| "Write the memory, or press Esc to leave.".to_string())
                })
                .await
                .map_err(|f| g.left_or(f))?;
            let provenance = g
                .face
                .ask(
                    "Where does this come from?",
                    "the developer, during setup",
                    false,
                    &|_: &str| None,
                )
                .await
                .map_err(|f| g.left_or(f))?;
            let remembered = g
                .attempt(
                    "Memory creation",
                    || {
                        g.act(
                            "memory.create",
                            vec![
                                project_id.clone(),
                                "project".into(),
                                content.clone(),
                                provenance.clone(),
                            ],
                            vec![],
                        )
                    },
                    true,
                )
                .await?;
            if let Some(record) = remembered {
                g.ok(&format!(
                    "Remembered {}",
                    s::dim(&format!(
                        "{} · {}",
                        record.get("id").js(),
                        short_hash(record.get("hash"))
                    ))
                ));
                g.push_done("one project memory".into());
            }
        }
    }

    // ── 7 agents ───────────────────────────────────────────────────────────
    g.begin(6, "Coding agents that open a repository directly (Claude Code, Codex, Pi) read CLAUDE.md or AGENTS.md at session start. Onboarding adds a section per component telling them to run its prime command; priming prints the tickets, expertise and prompts they work from. These repo-local stores live in .luuna/ inside the checkout, separate from the runtime's managed stores.");
    let tools = runner.available();
    if tools.is_empty() {
        g.warn("The Missions, Atlas and Beacon CLIs are not installed beside Luuna.");
        g.note(&format!("Install them with scripts/install-cli.ps1, then run {} init --project {project_id} to onboard the agents.", g.name));
        g.finish(6, "skipped");
    } else {
        let repositories = view.get("repositories").items().to_vec();
        let mut targets = repositories.clone();
        if repositories.len() > 1 {
            let options: Vec<(String, String, bool)> = repositories
                .iter()
                .enumerate()
                .map(|(i, r)| {
                    (
                        r.get("repository_id").js(),
                        r.get("local_path").js(),
                        i == 0,
                    )
                })
                .collect();
            let marks = g
                .face
                .checklist("Onboard agents in which repositories?", &options)
                .await
                .map_err(|f| g.left_or(f))?;
            targets = repositories
                .iter()
                .zip(&marks)
                .filter(|(_, m)| **m)
                .map(|(r, _)| r.clone())
                .collect();
        }
        let mut primed = 0;
        for registration in &targets {
            let root = registration.get("local_path").js();
            let root_path = PathBuf::from(&root);
            let (instructions, created) = instructions_file(&root_path, &|p: &Path| p.exists());
            g.face.print(&format!(
                "    {}  {root}",
                s::cyan(&registration.get("repository_id").js())
            ));
            let present: Vec<bool> = tools
                .iter()
                .map(|t| root_path.join(t.store).exists())
                .collect();
            for (tool, here) in tools.iter().zip(&present) {
                let store_text = if cfg!(windows) {
                    tool.store.replace('/', "\\")
                } else {
                    tool.store.to_string()
                };
                let _ = store_text;
                g.face.print(&format!(
                    "      {} {} {} {}",
                    if *here {
                        s::green("●")
                    } else {
                        s::dim("○")
                    },
                    s::bold(&js::pad_end(tool.brand, 9)),
                    tool.what,
                    s::dim(&if *here {
                        tool.store.to_string()
                    } else {
                        format!("{} not created", tool.store)
                    })
                ));
            }
            let missing: Vec<_> = tools
                .iter()
                .zip(&present)
                .filter(|(_, here)| !**here)
                .map(|(t, _)| t.clone())
                .collect();
            let mut options: Vec<(String, String, bool, &str)> = Vec::new();
            if !missing.is_empty() {
                options.push((
                    "Create the repo-local stores".into(),
                    format!(
                        "{} · .gitattributes merge rules",
                        missing
                            .iter()
                            .map(|t| t.store)
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                    true,
                    "init",
                ));
            }
            options.push((
                format!("Add session-start instructions to {instructions}"),
                if created {
                    "creates the file".into()
                } else {
                    "appends marked sections".into()
                },
                true,
                "onboard",
            ));
            if tools.iter().any(|t| t.hooks) {
                options.push((
                    "Install Claude Code hooks".into(),
                    ".claude/settings.json: prime on session start, guard store edits".into(),
                    false,
                    "hooks",
                ));
            }
            options.push((
                "Show what each agent is primed with".into(),
                "read-only".into(),
                true,
                "prime",
            ));
            let list: Vec<(String, String, bool)> = options
                .iter()
                .map(|(l, h, c, _)| (l.clone(), h.clone(), *c))
                .collect();
            let marks = g
                .face
                .checklist(
                    &format!(
                        "Set up the agents in {}",
                        registration.get("repository_id").js()
                    ),
                    &list,
                )
                .await
                .map_err(|f| g.left_or(f))?;
            let wanted: Vec<&str> = options
                .iter()
                .zip(&marks)
                .filter(|(_, m)| **m)
                .map(|(o, _)| o.3)
                .collect();
            let run_all =
                |label: String, list: Vec<crate::components::Component>, args: ArgsFor| {
                    let g = &*g;
                    let root = root.clone();
                    async move {
                        g.face.spin(&label);
                        let mut outcomes = Vec::new();
                        for tool in list {
                            let result = runner.run(tool.name, args(&tool), &root).await;
                            outcomes.push((tool, result));
                        }
                        g.face.spin_stop(None);
                        outcomes
                    }
                };
            let report =
                |g: &Guide<'_>,
                 outcomes: &[(crate::components::Component, crate::components::Outcome)],
                 success: &str| {
                    for (tool, result) in outcomes {
                        if result.code == 0 {
                            g.ok(&format!("{} {success}", tool.brand));
                        } else {
                            let text = if result.stderr.is_empty() {
                                &result.stdout
                            } else {
                                &result.stderr
                            };
                            let last = text
                                .trim()
                                .split('\n')
                                .next_back()
                                .unwrap_or("")
                                .to_string();
                            g.warn(&format!(
                                "{}: {}",
                                tool.brand,
                                if last.is_empty() {
                                    format!("exit {}", result.code)
                                } else {
                                    last
                                }
                            ));
                        }
                    }
                    outcomes.iter().all(|(_, r)| r.code == 0)
                };
            let fixed = |args: &'static [&'static str]| -> ArgsFor {
                Box::new(move |_| args.iter().map(|a| a.to_string()).collect())
            };
            if wanted.contains(&"init") {
                let outcomes = run_all(
                    "Creating the repo-local stores".into(),
                    missing.clone(),
                    fixed(&["init"]),
                )
                .await;
                report(g, &outcomes, "store created");
            }
            if wanted.contains(&"onboard") {
                let previews = run_all(
                    "Preparing the instructions".into(),
                    tools.clone(),
                    fixed(&["onboard", "--stdout"]),
                )
                .await;
                let text = previews
                    .iter()
                    .filter(|(_, r)| r.code == 0)
                    .map(|(_, r)| r.stdout.trim().to_string())
                    .collect::<Vec<_>>()
                    .join("\n\n");
                if !text.is_empty() {
                    g.face.panel(&format!("Sections for {instructions}"), &text);
                }
                let failed: Vec<_> = previews
                    .iter()
                    .filter(|(_, r)| r.code != 0)
                    .cloned()
                    .collect();
                if !failed.is_empty() {
                    report(g, &failed, "");
                }
                if !text.is_empty() {
                    let options = [
                        (
                            "Write the sections".to_string(),
                            "marked, so a re-run updates in place".to_string(),
                            true,
                        ),
                        ("Not now".to_string(), String::new(), false),
                    ];
                    if g.face
                        .choose(&format!("Write them to {instructions}?"), &options, 0)
                        .await
                        .map_err(|f| g.left_or(f))?
                    {
                        let writable: Vec<_> = tools
                            .iter()
                            .filter(|t| {
                                previews
                                    .iter()
                                    .any(|(p, r)| p.name == t.name && r.code == 0)
                            })
                            .cloned()
                            .collect();
                        let outcomes = run_all(
                            format!("Writing {instructions}"),
                            writable,
                            fixed(&["onboard"]),
                        )
                        .await;
                        if report(g, &outcomes, &format!("section in {instructions}")) {
                            g.push_done(format!(
                                "{instructions} in {}",
                                registration.get("repository_id").js()
                            ));
                        }
                    }
                }
            }
            if wanted.contains(&"hooks") {
                let outcomes = run_all(
                    "Installing Claude Code hooks".into(),
                    tools.iter().filter(|t| t.hooks).cloned().collect(),
                    fixed(&["setup", "claude"]),
                )
                .await;
                if report(g, &outcomes, "hooks in .claude/settings.json") {
                    g.push_done("Claude Code hooks".into());
                }
            }
            if wanted.contains(&"prime") {
                let outcomes = run_all(
                    "Priming".into(),
                    tools.clone(),
                    Box::new(|t: &crate::components::Component| {
                        t.prime.iter().map(|a| a.to_string()).collect()
                    }),
                )
                .await;
                for (tool, result) in &outcomes {
                    if result.code == 0 && !result.stdout.trim().is_empty() {
                        g.face.panel(
                            &format!("{} primes an agent with", tool.brand),
                            result.stdout.trim(),
                        );
                    } else {
                        let text = if result.stderr.is_empty() {
                            &result.stdout
                        } else {
                            &result.stderr
                        };
                        let last = text
                            .trim()
                            .split('\n')
                            .next_back()
                            .unwrap_or("")
                            .to_string();
                        g.warn(&format!(
                            "{} prime: {}",
                            tool.brand,
                            if last.is_empty() {
                                format!("exit {}", result.code)
                            } else {
                                last
                            }
                        ));
                    }
                }
                primed += 1;
            }
            if !wanted.is_empty() {
                g.note("These files are uncommitted changes in the checkout. Review and commit them when ready (each tool's sync command commits its store).");
            }
        }
        if primed > 0 {
            g.push_done(format!(
                "agents primed in {primed} repositor{}",
                if primed == 1 { "y" } else { "ies" }
            ));
        }
        g.finish(
            6,
            if targets.is_empty() {
                "skipped"
            } else {
                "done"
            },
        );
    }

    // ── 8 ticket ───────────────────────────────────────────────────────────
    g.begin(7, "A ticket states what should change and how to tell it is done. A plan for it is then recorded, approved by its exact hash, and implemented.");
    let mut ticket: Option<J> = None;
    let offer = view.get("prerequisites").items().first().cloned();
    if let Some(offer) = &offer {
        g.warn(&format!(
            "Testing setup needed for {}: {}",
            offer.get("repository_id").js(),
            offer.get("reason").js()
        ));
        g.note("Feature plans are refused until it exists. The prerequisite is a testing_setup ticket whose plan proposes the check recipes.");
    }
    let mut options: Vec<(String, String, &str)> = Vec::new();
    if let Some(offer) = &offer {
        options.push((
            format!(
                "Create the testing-setup ticket for {}",
                offer.get("repository_id").js()
            ),
            String::new(),
            "setup",
        ));
    }
    options.push((
        if offer.is_some() {
            "Create a feature ticket anyway"
        } else {
            "Create a ticket"
        }
        .to_string(),
        String::new(),
        "feature",
    ));
    options.push((
        "Skip for now".into(),
        format!("{} create-ticket {project_id} …", g.name),
        "skip",
    ));
    let kind = g
        .face
        .choose(
            if offer.is_some() {
                "Open the testing-setup ticket?"
            } else {
                "Open the first ticket?"
            },
            &options,
            0,
        )
        .await
        .map_err(|f| g.left_or(f))?;
    if kind == "skip" {
        g.finish(7, "skipped");
    } else {
        let repository_ids: Vec<String> = view
            .get("repositories")
            .items()
            .iter()
            .map(|r| r.get("repository_id").js())
            .collect();
        let repository = if kind == "setup" {
            offer.as_ref().unwrap().get("repository_id").js()
        } else if repository_ids.len() > 1 {
            let choices: Vec<(String, String, String)> = repository_ids
                .iter()
                .map(|id| (id.clone(), String::new(), id.clone()))
                .collect();
            g.face
                .choose("Which repository?", &choices, 0)
                .await
                .map_err(|f| g.left_or(f))?
        } else {
            repository_ids.first().cloned().unwrap_or_default()
        };
        let setup = kind == "setup";
        let title = g
            .face
            .ask(
                "Title",
                &if setup {
                    format!("Establish a testing setup for {repository}")
                } else {
                    String::new()
                },
                false,
                &|v: &str| v.is_empty().then(|| "A ticket needs a title.".to_string()),
            )
            .await
            .map_err(|f| g.left_or(f))?;
        let intent = g.face.ask("Intent: what should change and why", &if setup { format!("Give {repository} required checks that execute its tests, so baselines and validation have evidence.") } else { String::new() }, false, &|v: &str| v.is_empty().then(|| "A ticket needs an intent.".to_string())).await.map_err(|f| g.left_or(f))?;
        let mut criteria: Vec<String> = Vec::new();
        loop {
            let initial = if criteria.is_empty() && setup {
                "The proposed required checks pass and execute at least one test".to_string()
            } else {
                String::new()
            };
            let first = criteria.is_empty();
            let value = g
                .face
                .ask(
                    if first {
                        "Acceptance criterion"
                    } else {
                        "Another criterion"
                    },
                    &initial,
                    !first,
                    &move |text: &str| {
                        (text.is_empty() && first).then(|| {
                            "A ticket needs at least one acceptance criterion.".to_string()
                        })
                    },
                )
                .await
                .map_err(|f| g.left_or(f))?;
            if value.is_empty() {
                break;
            }
            criteria.push(value);
        }
        let mut ticket_flags = vec![
            ("title".to_string(), FlagValue::Str(title.clone())),
            ("intent".to_string(), FlagValue::Str(intent.clone())),
            ("criterion".to_string(), FlagValue::List(criteria.clone())),
            ("repository".to_string(), FlagValue::Str(repository.clone())),
        ];
        if setup {
            ticket_flags.push((
                "purpose".to_string(),
                FlagValue::Str("testing_setup".into()),
            ));
        }
        let created = g
            .attempt(
                "Ticket creation",
                || {
                    g.act(
                        "tickets.create",
                        vec![project_id.clone()],
                        ticket_flags.clone(),
                    )
                },
                true,
            )
            .await?;
        ticket = created;
        match &ticket {
            Some(t) => {
                g.ok(&format!(
                    "Ticket {} r{} {}",
                    s::bold(&t.get("id").js()),
                    t.get("revision").js(),
                    s::dim(&format!("· {}", t.get("source").js()))
                ));
                g.push_done(format!("ticket {}", t.get("id").js()));
                g.finish(7, "done");
            }
            None => g.finish(7, "skipped"),
        }
    }

    // ── done ───────────────────────────────────────────────────────────────
    outcome.ticket = ticket.as_ref().map(|t| t.get("id").js());
    let manifest = view.get("manifest");
    let title = format!(
        "{} {}  {}",
        s::bold(&view.get("project").get("name").js()),
        s::dim(&format!("({project_id})")),
        if manifest.truthy() {
            format!("manifest r{}", manifest.get("revision").js())
        } else {
            s::yellow("manifest unconfirmed")
        }
    );
    let done = g.done.lock().unwrap().clone();
    let summary = if done.is_empty() {
        "Nothing changed; every step was already in place or skipped.".to_string()
    } else {
        done.join(" · ")
    };
    let mut next = Vec::new();
    match &outcome.ticket {
        Some(id) => next.push((
            format!("{} plan {id}", g.name),
            "record a plan, then approve it by its exact hash".to_string(),
        )),
        None => next.push((
            format!(
                "{} create-ticket {project_id} --title … --intent … --criterion …",
                g.name
            ),
            "open a ticket".to_string(),
        )),
    }
    next.push((
        format!("{} home", g.name),
        "what needs your attention".into(),
    ));
    next.push((
        format!("{} board", g.name),
        "tickets by workflow state".into(),
    ));
    next.push((g.name.clone(), "the interactive workbench".into()));
    next.push((format!("{} help", g.name), "every command".into()));
    g.face.finish(title, &summary, next, &g.states, &project_id);
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_and_slugs_follow_the_node_helpers() {
        assert_eq!(slug("My App!"), "my-app");
        assert_eq!(slug("123"), "app");
        assert_eq!(
            basename(if cfg!(windows) {
                r"C:\work\demo\"
            } else {
                "/work/demo/"
            }),
            "demo"
        );
        assert!(is_absolute(if cfg!(windows) { r"C:\x" } else { "/x" }));
        assert!(!is_absolute("x"));
        assert_eq!(unmarked("a\u{E000}b\u{E001}c\u{E002}"), "abc");
        assert_eq!(s::green("ok"), "\u{E000}green\u{E001}ok\u{E002}");
    }
}
