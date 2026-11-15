//! The command line: `luuna-tui <command> [args] [--flag value] [--json]`, one command per catalog
//! entry, for scripts and tests, plus the help, the catalog and the setup guide. Without a command
//! and with a terminal, the interactive workbench opens. Exit codes: 0 done, 1 refused or failed,
//! 2 usage.
use crate::client::{Client, Controller, Signal};
use crate::commands::{self, by_name, fs_failure, usage, Command, FlagValue, Fut, Input, Io};
use crate::failures::{explain_failure, Failure};
use crate::format::{colour_enabled, format};
use crate::help::{self, command_help, help_command, overview, reference, suggestions, Options};
use crate::js::J;
use std::io::{IsTerminal, Read, Write};

pub const DEFAULT_URL: &str = "http://127.0.0.1:4242";

/// What the command line needs from its process: the environment and the three streams.
pub struct Process {
    pub env: Vec<(String, String)>,
    pub stdin_terminal: bool,
    pub stdout_terminal: bool,
    pub columns: Option<usize>,
    pub stdout: Box<dyn Write + Send>,
    pub stderr: Box<dyn Write + Send>,
    pub stdin: Box<dyn Read + Send>,
}

impl Process {
    pub fn real() -> Process {
        let stdout_terminal = std::io::stdout().is_terminal();
        Process {
            env: std::env::vars().collect(),
            stdin_terminal: std::io::stdin().is_terminal(),
            stdout_terminal,
            columns: if stdout_terminal {
                crossterm::terminal::size().ok().map(|(c, _)| c as usize)
            } else {
                None
            },
            stdout: Box::new(std::io::stdout()),
            stderr: Box::new(std::io::stderr()),
            stdin: Box::new(std::io::stdin()),
        }
    }
    pub fn var(&self, name: &str) -> Option<&str> {
        self.env
            .iter()
            .rev()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }
    fn out(&mut self, text: &str) {
        let _ = self.stdout.write_all(text.as_bytes());
        let _ = self.stdout.flush();
    }
    fn err(&mut self, text: &str) {
        let _ = self.stderr.write_all(text.as_bytes());
        let _ = self.stderr.flush();
    }
}

/// What `main` decided when it did not finish by itself: open the workbench (optionally the guide).
#[derive(Debug, PartialEq)]
pub enum Next {
    Exit(i32),
    Workbench {
        guide: Option<Vec<(String, String)>>,
    },
}

/// Splits argv into positional arguments and `--flag value` / `--flag=value` / boolean flags.
pub fn parse_arguments(argv: &[String], command: Option<&Command>) -> Result<Input, Failure> {
    let mut input = Input::default();
    let mut index = 0;
    while index < argv.len() {
        let token = &argv[index];
        if token == "--" {
            input.args.extend(argv[index + 1..].iter().cloned());
            break;
        }
        // A command with a trailing command (`sbx <project> pi --model x`) takes every word after its
        // own arguments verbatim, so the result is the same when a launcher drops the `--`.
        if let Some(command) = command {
            if command.rest && !token.starts_with("--") && input.args.len() >= command.args.len() {
                input.args.extend(argv[index..].iter().cloned());
                break;
            }
        }
        let Some(stripped) = token.strip_prefix("--") else {
            input.args.push(token.clone());
            index += 1;
            continue;
        };
        let (name, inline) = match stripped.find('=') {
            Some(at) => (&stripped[..at], Some(stripped[at + 1..].to_string())),
            None => (stripped, None),
        };
        if name == "json" {
            input.set("json", FlagValue::Bool(true));
            index += 1;
            continue;
        }
        let Some(definition) = command.and_then(|c| c.flag(name)) else {
            return Err(Failure::Usage(format!(
                "Unknown option --{name} for {}. Usage: {}",
                command.map(|c| c.name.as_str()).unwrap_or("this command"),
                command
                    .map(usage)
                    .unwrap_or_else(|| "luuna-tui <command>".into())
            )));
        };
        let value = if definition.boolean {
            FlagValue::Bool(match &inline {
                None => true,
                Some(text) => !["false", "no", "0"].contains(&text.as_str()),
            })
        } else if let Some(text) = inline {
            FlagValue::Str(text)
        } else {
            if index + 1 >= argv.len() {
                return Err(Failure::Usage(format!("--{name} needs a value.")));
            }
            index += 1;
            FlagValue::Str(argv[index].clone())
        };
        if definition.repeat {
            let mut items = match input.flag(name) {
                Some(FlagValue::List(items)) => items.clone(),
                _ => Vec::new(),
            };
            items.push(value.text());
            input.set(name, FlagValue::List(items));
        } else {
            input.set(name, value);
        }
        index += 1;
    }
    Ok(input)
}

/// Reads the hidden token on a terminal: never echoed, never logged; Ctrl-C exits 130.
pub(crate) fn read_secret(question: &str, process: &mut Process) -> Result<String, u8> {
    process.out(&format!("{question}: "));
    if !process.stdin_terminal {
        let mut text = String::new();
        let _ = process.stdin.read_to_string(&mut text);
        return Ok(text
            .split('\n')
            .next()
            .unwrap_or("")
            .trim_end_matches('\r')
            .to_string());
    }
    use crossterm::event::{read, Event, KeyCode, KeyEventKind, KeyModifiers};
    let was_raw = crossterm::terminal::is_raw_mode_enabled().unwrap_or(false);
    let _ = crossterm::terminal::enable_raw_mode();
    let mut value = String::new();
    let result = loop {
        match read() {
            Ok(Event::Key(key)) if key.kind != KeyEventKind::Release => match key.code {
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    break Err(130)
                }
                KeyCode::Enter => break Ok(value),
                KeyCode::Backspace => {
                    value.pop();
                }
                KeyCode::Char(c) => value.push(c),
                _ => {}
            },
            Ok(Event::Paste(text)) => value.push_str(&text),
            Ok(_) => {}
            Err(_) => break Ok(value),
        }
    };
    if !was_raw {
        let _ = crossterm::terminal::disable_raw_mode();
    }
    process.out("\n");
    result
}

/// The client for `LUUNA_URL` and `LUUNA_API_TOKEN`; the token is prompted (hidden) on a terminal.
pub fn connect(process: &mut Process) -> Result<Result<Client, Failure>, u8> {
    let url = process
        .var("LUUNA_URL")
        .map(crate::builders::js_trim)
        .filter(|u| !u.is_empty())
        .unwrap_or_else(|| DEFAULT_URL.to_string());
    let mut token = process.var("LUUNA_API_TOKEN").unwrap_or("").to_string();
    if crate::builders::js_trim(&token).is_empty() {
        if !process.stdin_terminal {
            return Ok(Err(Failure::Usage("Set LUUNA_API_TOKEN to the runtime's service token (or run interactively to type it).".into())));
        }
        token = read_secret("Service token (hidden)", process)?;
    }
    Ok(Client::new(&url, &token))
}

/// The command line's io: plain streams, no prompts (approval needs `--acknowledge-hash`).
struct CliIo {
    signal: Signal,
    json: bool,
    colour: bool,
    env: Vec<(String, String)>,
    sandbox: std::sync::Mutex<Option<i32>>,
    stdout: std::sync::Mutex<Box<dyn Write + Send>>,
    stderr: std::sync::Mutex<Box<dyn Write + Send>>,
    stdin: std::sync::Mutex<Option<Box<dyn Read + Send>>>,
}

impl Io for CliIo {
    fn signal(&self) -> Signal {
        self.signal.clone()
    }
    fn interactive(&self) -> bool {
        false
    }
    fn read_file(&self, path: &str) -> Result<String, Failure> {
        std::fs::read(path)
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
            .map_err(|e| fs_failure(&e, path, false))
    }
    fn write_file(&self, path: &str, text: &str) -> Result<(), Failure> {
        std::fs::write(path, text).map_err(|e| fs_failure(&e, path, true))
    }
    fn read_stdin(&self) -> Fut<'_, Result<String, Failure>> {
        Box::pin(async move {
            let mut text = Vec::new();
            if let Some(stdin) = self.stdin.lock().unwrap().as_mut() {
                let _ = stdin.read_to_end(&mut text);
            }
            Ok(String::from_utf8_lossy(&text).into_owned())
        })
    }
    fn prompt(&self, _question: String) -> Fut<'_, Result<String, Failure>> {
        Box::pin(async { Ok(String::new()) })
    }
    fn confirm(&self, _question: String) -> Fut<'_, Result<bool, Failure>> {
        Box::pin(async { Ok(true) })
    }
    fn show(&self, text: &str) -> Result<(), Failure> {
        let mut out = self.stdout.lock().unwrap();
        let _ = writeln!(out, "{text}");
        let _ = out.flush();
        Ok(())
    }
    fn note(&self, text: &str) {
        let mut err = self.stderr.lock().unwrap();
        let _ = writeln!(err, "{text}");
        let _ = err.flush();
    }
    fn can_launch(&self) -> bool {
        true
    }
    fn launch(
        &self,
        plan: J,
        command: Vec<String>,
        grants: Vec<String>,
    ) -> Fut<'_, Result<i32, Failure>> {
        Box::pin(async move {
            let code = crate::sandbox::launch(&plan, &command, &grants, &self.env).await?;
            *self.sandbox.lock().unwrap() = Some(code);
            Ok(code)
        })
    }
    fn sandbox_exit(&self) -> Option<i32> {
        *self.sandbox.lock().unwrap()
    }
    fn emit(&self, event: &J) {
        let line = if self.json {
            event.stringify().unwrap_or_default()
        } else {
            format("runs.events", &J::Arr(vec![event.clone()]), self.colour).unwrap_or_default()
        };
        let mut out = self.stdout.lock().unwrap();
        let _ = writeln!(out, "{line}");
        let _ = out.flush();
    }
}

/// Runs one command; returns the exit code. Output goes to stdout, refusals to stderr.
/// The shared core of a command: the argument count, the interactive confirmation (None when the
/// developer declined) and the run itself.
pub async fn execute(
    command: &Command,
    input: &Input,
    client: &Client,
    io: &dyn Io,
) -> Result<Option<J>, Failure> {
    let expected = command.args.len();
    if input.args.len() < expected {
        return Err(Failure::Usage(format!(
            "{} needs {expected} argument(s). Usage: {}",
            command.name,
            usage(command)
        )));
    }
    if input.args.len() > expected && !command.rest {
        return Err(Failure::Usage(format!(
            "{} takes {expected} argument(s); unexpected: {}. Usage: {}",
            command.name,
            input.args[expected..].join(" "),
            usage(command)
        )));
    }
    if let Some(question) = &command.confirm {
        if io.interactive() && !io.confirm(question.clone()).await? {
            return Ok(None);
        }
    }
    commands::run(command, client, input, io).await.map(Some)
}

/// The sentence a failed command prints: a client refusal names itself, anything else is explained.
pub fn refusal_text(command: &Command, failure: &Failure) -> String {
    match failure {
        Failure::Refusal(message) => format!("{} refused: {message}", command.action),
        other => explain_failure(&command.action, other),
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn run_command(
    command: &Command,
    input: &Input,
    client: &Client,
    io: &dyn Io,
    json: bool,
    colour: bool,
    out: &mut dyn FnMut(&str),
    err: &mut dyn FnMut(&str),
) -> i32 {
    let outcome: Result<i32, Failure> = async {
        let result = match execute(command, input, client, io).await? {
            None => {
                out("Cancelled.\n");
                J::Undefined
            }
            // A sandboxed command ran; its exit code becomes the client's.
            Some(_) if io.sandbox_exit().is_some() => return Ok(io.sandbox_exit().unwrap()),
            Some(result) if json => {
                out(&format!(
                    "{}\n",
                    result.stringify().unwrap_or_else(|| "null".into())
                ));
                result
            }
            Some(result) => {
                let text = format(&command.id, &result, colour)?;
                if !text.is_empty() {
                    out(&format!("{text}\n"));
                }
                result
            }
        };
        Ok(if commands::failed(command, &result)? {
            1
        } else {
            0
        })
    }
    .await;
    match outcome {
        Ok(code) => code,
        Err(Failure::Usage(message)) => {
            err(&format!("{message}\n"));
            2
        }
        Err(failure) => {
            let refusal = refusal_text(command, &failure);
            if json {
                out(&format!(
                    "{}\n",
                    crate::obj![("error", refusal.clone())].stringify().unwrap()
                ));
            }
            err(&format!("{refusal}\n"));
            1
        }
    }
}

/// The command line's entry. Returns the exit code, or that the workbench (or its guide) must open.
pub async fn main(argv: Vec<String>, process: &mut Process) -> Next {
    let name = argv.first().cloned();
    if name.is_none() && process.stdin_terminal && process.stdout_terminal {
        return Next::Workbench { guide: None };
    }
    let rest: Vec<String> = argv.iter().skip(1).cloned().collect();
    let program = process
        .var("LUUNA_CLI_NAME")
        .map(crate::builders::js_trim)
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| help::DEFAULT_NAME.into());
    let options = Options {
        name: program.clone(),
        colour: colour_enabled(
            process.var("NO_COLOR"),
            process.var("TERM"),
            process.stdout_terminal,
        ),
        width: if process.stdout_terminal {
            process.columns
        } else {
            None
        },
    };
    let unknown = |process: &mut Process, word: &str| {
        let close = suggestions(word);
        process.err(&format!(
            "Unknown command {word}.{} Run {program} help for the commands.\n",
            if close.is_empty() {
                String::new()
            } else {
                format!(" Did you mean {}?", close.join(", "))
            }
        ));
        Next::Exit(2)
    };
    match name.as_deref() {
        None | Some("--help") | Some("-h") => {
            process.out(&overview(&options));
            return Next::Exit(if name.is_none() { 2 } else { 0 });
        }
        Some("help") => {
            let Some(topic) = rest.first() else {
                process.out(&overview(&options));
                return Next::Exit(0);
            };
            if topic == "--all" || topic == "all" {
                process.out(&reference(&options));
                return Next::Exit(0);
            }
            let described = help_command(topic);
            match described {
                Some(command) if rest.len() == 1 => {
                    process.out(&command_help(command, &options));
                    return Next::Exit(0);
                }
                _ => return unknown(process, &rest.join(" ")),
            }
        }
        Some("catalog") => {
            process.out(&format!(
                "{}\n",
                commands::table().catalog.stringify_pretty().unwrap()
            ));
            return Next::Exit(0);
        }
        _ => {}
    }
    let name = name.unwrap();
    let Some(command) = help_command(&name) else {
        return unknown(process, &name);
    };
    // `<command> --help` needs no token; a `--help` after `--` is an argument.
    let before = rest
        .iter()
        .position(|t| t == "--")
        .map(|at| &rest[..at])
        .unwrap_or(&rest[..]);
    if before.iter().any(|t| t == "--help" || t == "-h") {
        process.out(&command_help(command, &options));
        return Next::Exit(0);
    }
    let input = match parse_arguments(&rest, Some(command)) {
        Ok(input) => input,
        Err(failure) => {
            process.err(&format!("{}\n", failure.message()));
            return Next::Exit(2);
        }
    };
    if command.is_guide() {
        if !(process.stdin_terminal && process.stdout_terminal) {
            process.err(&format!("{program} init is a guided setup and needs a terminal. Without one, run its steps as commands: register, discover, confirm, baseline, enable-components, create-ticket ({program} help).\n"));
            return Next::Exit(2);
        }
        let flags = input
            .flags
            .iter()
            .filter(|(n, _)| n != "json")
            .map(|(n, v)| (n.clone(), v.text()))
            .collect();
        return Next::Workbench { guide: Some(flags) };
    }
    let client = match connect(process) {
        Err(code) => return Next::Exit(code.into()),
        Ok(Ok(client)) => client,
        Ok(Err(failure)) => {
            let usage = matches!(failure, Failure::Usage(_));
            let message = if usage {
                failure.message()
            } else {
                explain_failure("Connection", &failure)
            };
            process.err(&format!("{message}\n"));
            return Next::Exit(if usage { 2 } else { 1 });
        }
    };
    let command = by_name(&name).unwrap_or(command);
    let (controller, signal) = Controller::new();
    let interrupt = tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            controller.abort();
        }
    });
    let json = input.flag("json").is_some();
    let colour = options.colour;
    let stdin = std::mem::replace(&mut process.stdin, Box::new(std::io::empty()));
    let io = CliIo {
        signal,
        json,
        colour,
        env: process.env.clone(),
        sandbox: std::sync::Mutex::new(None),
        stdout: std::sync::Mutex::new(std::mem::replace(
            &mut process.stdout,
            Box::new(std::io::sink()),
        )),
        stderr: std::sync::Mutex::new(std::mem::replace(
            &mut process.stderr,
            Box::new(std::io::sink()),
        )),
        stdin: std::sync::Mutex::new(Some(stdin)),
    };
    let mut out = |text: &str| {
        let mut stdout = io.stdout.lock().unwrap();
        let _ = stdout.write_all(text.as_bytes());
        let _ = stdout.flush();
    };
    let mut err = |text: &str| {
        let mut stderr = io.stderr.lock().unwrap();
        let _ = stderr.write_all(text.as_bytes());
        let _ = stderr.flush();
    };
    let code = run_command(
        command, &input, &client, &io, json, colour, &mut out, &mut err,
    )
    .await;
    interrupt.abort();
    Next::Exit(code)
}
