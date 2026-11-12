//! Process interaction behind the CLI boundary.
//!
//! Commands receive this small trait rather than reaching into process globals,
//! which keeps their observable behavior testable in-process just as the
//! original Bun implementation was.

use std::collections::BTreeMap;
use std::io::{self, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::interactive::terminal::{ProcessTerminal, Terminal};
use roots_core::actor::Environment;

/// The process services a Roots command may use.
pub trait Io {
    fn cwd(&self) -> &Path;
    fn env(&self, key: &str) -> Option<&str>;
    /// A complete, injected environment snapshot.
    ///
    /// Actor resolution deliberately passes this exact environment through to
    /// `git config user.name`; looking up only the Roots variables would break
    /// `PATH`, `HOME`, and test-specific `GIT_CONFIG_*` behavior.
    fn environment(&self) -> Environment;
    fn stdout_is_tty(&self) -> bool;
    fn stdin_is_tty(&self) -> bool;
    fn stdout(&mut self, text: &str);
    fn stderr(&mut self, text: &str);
    fn read_stdin(&mut self) -> io::Result<String>;
    /// Interactive commands take exclusive ownership of a terminal session
    /// through this optional capability.  Ordinary command I/O deliberately
    /// remains usable without one.
    fn terminal(&mut self) -> Option<&mut (dyn Terminal + 'static)> {
        None
    }
    /// Thread-safe stderr sink for an interactive status line. Process I/O
    /// writes immediately; deterministic test I/O buffers and flushes after a
    /// command returns, avoiding a borrowed `Io` across the terminal session.
    fn agent_status_writer(&self) -> Arc<dyn Fn(String) + Send + Sync> {
        Arc::new(|_| {})
    }
    fn flush_agent_status(&mut self) {}
}

/// Real stdin/stdout/stderr-backed command I/O.
pub struct ProcessIo {
    cwd: PathBuf,
    env: BTreeMap<String, String>,
    stdout_is_tty: bool,
    stdin_is_tty: bool,
    terminal: ProcessTerminal,
}

impl std::fmt::Debug for ProcessIo {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ProcessIo")
            .field("cwd", &self.cwd)
            .field("stdout_is_tty", &self.stdout_is_tty)
            .field("stdin_is_tty", &self.stdin_is_tty)
            .finish_non_exhaustive()
    }
}

impl ProcessIo {
    pub fn from_process() -> Self {
        Self {
            cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            env: std::env::vars().collect(),
            stdout_is_tty: io::stdout().is_terminal(),
            stdin_is_tty: io::stdin().is_terminal(),
            terminal: ProcessTerminal::new(),
        }
    }
}

impl Default for ProcessIo {
    fn default() -> Self {
        Self::from_process()
    }
}

impl Io for ProcessIo {
    fn cwd(&self) -> &Path {
        &self.cwd
    }

    fn env(&self, key: &str) -> Option<&str> {
        self.env.get(key).map(String::as_str)
    }

    fn environment(&self) -> Environment {
        self.env.clone()
    }

    fn stdout_is_tty(&self) -> bool {
        self.stdout_is_tty
    }

    fn stdin_is_tty(&self) -> bool {
        self.stdin_is_tty
    }

    fn stdout(&mut self, text: &str) {
        let mut stdout = io::stdout().lock();
        let _ = stdout.write_all(text.as_bytes());
        let _ = stdout.flush();
    }

    fn stderr(&mut self, text: &str) {
        let mut stderr = io::stderr().lock();
        let _ = stderr.write_all(text.as_bytes());
        let _ = stderr.flush();
    }

    fn read_stdin(&mut self) -> io::Result<String> {
        let mut input = String::new();
        io::stdin().read_to_string(&mut input)?;
        Ok(input)
    }

    fn terminal(&mut self) -> Option<&mut (dyn Terminal + 'static)> {
        Some(&mut self.terminal)
    }

    fn agent_status_writer(&self) -> Arc<dyn Fn(String) + Send + Sync> {
        Arc::new(|text| {
            let mut stderr = io::stderr().lock();
            let _ = stderr.write_all(text.as_bytes());
            let _ = stderr.flush();
        })
    }
}

/// Deterministic in-memory I/O for CLI and command tests.
pub struct MemoryIo {
    pub cwd: PathBuf,
    pub env: BTreeMap<String, String>,
    pub stdout_is_tty: bool,
    pub stdin_is_tty: bool,
    pub stdin: String,
    pub stdout: String,
    pub stderr: String,
    terminal: Option<Box<dyn Terminal>>,
    status_messages: Arc<Mutex<Vec<String>>>,
}

impl std::fmt::Debug for MemoryIo {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MemoryIo")
            .field("cwd", &self.cwd)
            .field("env", &self.env)
            .field("stdout_is_tty", &self.stdout_is_tty)
            .field("stdin_is_tty", &self.stdin_is_tty)
            .field("stdin", &self.stdin)
            .field("stdout", &self.stdout)
            .field("stderr", &self.stderr)
            .field("has_terminal", &self.terminal.is_some())
            .field(
                "pending_agent_status",
                &self
                    .status_messages
                    .lock()
                    .map(|messages| messages.len())
                    .unwrap_or_default(),
            )
            .finish()
    }
}

impl MemoryIo {
    pub fn new(cwd: impl Into<PathBuf>) -> Self {
        Self {
            cwd: cwd.into(),
            env: BTreeMap::new(),
            stdout_is_tty: false,
            stdin_is_tty: true,
            stdin: String::new(),
            stdout: String::new(),
            stderr: String::new(),
            terminal: None,
            status_messages: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Install a deterministic terminal for an interactive command test.
    pub fn set_terminal(&mut self, terminal: impl Terminal + 'static) {
        self.terminal = Some(Box::new(terminal));
    }
}

impl Io for MemoryIo {
    fn cwd(&self) -> &Path {
        &self.cwd
    }

    fn env(&self, key: &str) -> Option<&str> {
        self.env.get(key).map(String::as_str)
    }

    fn environment(&self) -> Environment {
        self.env.clone()
    }

    fn stdout_is_tty(&self) -> bool {
        self.stdout_is_tty
    }

    fn stdin_is_tty(&self) -> bool {
        self.stdin_is_tty
    }

    fn stdout(&mut self, text: &str) {
        self.stdout.push_str(text);
    }

    fn stderr(&mut self, text: &str) {
        self.stderr.push_str(text);
    }

    fn read_stdin(&mut self) -> io::Result<String> {
        Ok(self.stdin.clone())
    }

    fn terminal(&mut self) -> Option<&mut (dyn Terminal + 'static)> {
        self.terminal.as_deref_mut()
    }

    fn agent_status_writer(&self) -> Arc<dyn Fn(String) + Send + Sync> {
        let messages = Arc::clone(&self.status_messages);
        Arc::new(move |text| {
            if let Ok(mut messages) = messages.lock() {
                messages.push(text);
            }
        })
    }

    fn flush_agent_status(&mut self) {
        if let Ok(mut messages) = self.status_messages.lock() {
            for message in messages.drain(..) {
                self.stderr.push_str(&message);
            }
        }
    }
}
