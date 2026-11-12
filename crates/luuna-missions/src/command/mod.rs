//! The Go tool's commands as library calls.
//!
//! [`run`] takes an [`Invocation`] (the arguments Go's `main` would get,
//! the working directory, stdin, the environment and an optional explicit
//! store directory) and returns an [`Outcome`] with exactly the stdout,
//! stderr and exit code the Go tool produces. A failure that Go reports to
//! its user is an `Outcome` with exit code 1, not an `Err`. `Err` means the
//! invocation needs behaviour that is not ported yet
//! ([`Error::Unsupported`](crate::Error::Unsupported)) or that the library
//! itself failed (for example [`Error::Io`](crate::Error::Io), whose text is
//! not Go's); output already produced is then discarded, so a caller never
//! sees a partial or differing result.
//!
//! The dispatcher follows Go's `App.Run` (`internal/cli/app.go:139-249`):
//!
//! - Leading `-q`, `--quiet`, `--verbose` and `--timing` are skipped before
//!   the command name, and the quiet, verbose and timing settings are taken
//!   from the literal tokens anywhere in the arguments.
//! - A leading option that is not one of those is parsed alone, and an
//!   unknown one prints `error: unknown option '<arg>'` on stderr (SD6).
//! - An unknown option inside a command also prints on stderr, never JSON
//!   (SD6). Any other command error prints Go's JSON error envelope only if
//!   the literal `--json` token is present, and `Error: <message>` on stderr
//!   otherwise (SD5).
//! - `--timing` prints `⏱ <n>ms` (or `<x.xx>s`) on stderr after a command
//!   that did not fail.
//!
//! This plan ports init, create, show, list, search, update and block.
//! Returned as [`Error::Unsupported`](crate::Error::Unsupported) naming the
//! plan that ports them: every other command Go registers; help and version
//! output (`--help`, `-h`, `--version`, `-v`, and no command at all);
//! unknown commands (Go's suggestion); and an environment with
//! `LUUNA_MISSIONS_DEBUG=1` or `LUUNA_MISSIONS_LOG_LEVEL` set to `debug` or
//! `trace`, under which Go writes debug records to stdout (SD8).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::config::ProcessEnv;
use crate::error::{Error, Result, PLAN_2, PLAN_3, PLAN_4};
use crate::gopath;
use crate::store::Store;

mod block;
mod create;
mod init;
mod options;
mod output;
mod read;
mod shared;
mod update;

use options::OptionError;
use output::Envelope;

/// One run of a command.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Invocation {
    /// The arguments after the program name, as Go's `os.Args[1:]`.
    pub args: Vec<String>,
    /// The working directory, used for store discovery and in output. A
    /// relative path is taken from the process's working directory.
    pub cwd: PathBuf,
    /// The bytes on stdin.
    pub stdin: Vec<u8>,
    /// The complete environment the command sees, including the `PATH` used
    /// to find git during discovery. Nothing is inherited from the calling
    /// process.
    pub env: BTreeMap<String, String>,
    /// An explicit `.luuna/missions` directory, used instead of discovery.
    pub store_dir: Option<PathBuf>,
}

impl Invocation {
    /// An invocation of `args` in `cwd`, with empty stdin, an empty
    /// environment and store discovery.
    pub fn new<I, S>(cwd: impl Into<PathBuf>, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            args: args.into_iter().map(Into::into).collect(),
            cwd: cwd.into(),
            stdin: Vec::new(),
            env: BTreeMap::new(),
            store_dir: None,
        }
    }

    /// Sets the bytes on stdin.
    pub fn with_stdin(mut self, stdin: impl Into<Vec<u8>>) -> Self {
        self.stdin = stdin.into();
        self
    }

    /// Replaces the environment.
    pub fn with_env(mut self, env: BTreeMap<String, String>) -> Self {
        self.env = env;
        self
    }

    /// Uses `dir` as the store directory instead of discovering it.
    pub fn with_store_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.store_dir = Some(dir.into());
        self
    }
}

/// What a command printed and how it exited.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Outcome {
    /// Bytes written to stdout.
    pub stdout: Vec<u8>,
    /// Bytes written to stderr.
    pub stderr: Vec<u8>,
    /// The process exit code Go would use.
    pub exit_code: i32,
}

/// What a command handler sees and writes to (Go's `Context`).
pub(crate) struct Context<'a> {
    invocation: &'a Invocation,
    /// The working directory, absolute and cleaned (Go's `os.Getwd`).
    pub cwd: PathBuf,
    /// `-q` or `--quiet` appears among the arguments.
    pub quiet: bool,
    /// `--verbose` appears among the arguments. No ported command reads it,
    /// as in Go.
    #[allow(dead_code, reason = "Go records it and no command reads it")]
    pub verbose: bool,
    /// `--timing` appears among the arguments.
    pub timing: bool,
    /// The command name.
    #[allow(dead_code, reason = "no command ported so far reads it")]
    pub command: String,
    /// The exit code of a command that did not fail.
    pub exit_code: i32,
    /// Bytes for stdout.
    pub stdout: Vec<u8>,
    /// Bytes for stderr.
    pub stderr: Vec<u8>,
}

impl<'a> Context<'a> {
    fn new(invocation: &'a Invocation, command: &str) -> Result<Self> {
        let raw = &invocation.args;
        let cwd = gopath::abs(&invocation.cwd)
            .map_err(|error| Error::io("getwd", &invocation.cwd, error))?;
        Ok(Self {
            invocation,
            cwd,
            quiet: contains(raw, "--quiet") || contains(raw, "-q"),
            verbose: contains(raw, "--verbose"),
            timing: contains(raw, "--timing"),
            command: command.to_owned(),
            exit_code: 0,
            stdout: Vec::new(),
            stderr: Vec::new(),
        })
    }

    /// The store: the invocation's explicit store directory, or the one Go's
    /// `FindDir` finds from the working directory, with git run in the
    /// invocation's environment.
    pub fn store(&self) -> Result<Store> {
        match &self.invocation.store_dir {
            Some(dir) => Store::open(dir.clone()),
            None => Store::discover_with(&self.cwd, ProcessEnv::Exactly(&self.invocation.env)),
        }
    }

    /// The explicit store directory, if the invocation names one.
    pub fn explicit_store_dir(&self) -> Option<&Path> {
        self.invocation.store_dir.as_deref()
    }

    /// The bytes on stdin.
    #[allow(
        dead_code,
        reason = "no command ported so far reads it; the commands of s4-missions-lib-2 and -3 do"
    )]
    pub fn stdin(&self) -> &[u8] {
        &self.invocation.stdin
    }

    /// The environment variable `key`, as Go's `os.Getenv` would see it.
    pub fn env(&self, key: &str) -> Option<&str> {
        self.invocation.env.get(key).map(String::as_str)
    }

    /// Go's `Context.JSON`.
    pub fn json(&mut self, envelope: Envelope) {
        self.stdout.extend_from_slice(&envelope.render());
    }

    /// Go's `Context.Success`: `✓ <message>` on stdout unless quiet.
    pub fn success(&mut self, message: &[u8]) {
        if !self.quiet {
            self.stdout.extend_from_slice("✓ ".as_bytes());
            self.stdout.extend_from_slice(message);
            self.stdout.push(b'\n');
        }
    }

    /// Go's `Context.Warning`: `! <message>` on stderr unless quiet.
    #[allow(
        dead_code,
        reason = "no command ported so far reads it; the commands of s4-missions-lib-2 and -3 do"
    )]
    pub fn warning(&mut self, message: &[u8]) {
        if !self.quiet {
            self.stderr.extend_from_slice(b"! ");
            self.stderr.extend_from_slice(message);
            self.stderr.push(b'\n');
        }
    }
}

/// Why a command handler did not finish.
#[derive(Debug)]
pub(crate) enum Failure {
    /// Go's `unknownOptionError`: `error: <message>` on stderr, never JSON
    /// (SD6).
    UnknownOption(String),
    /// An error Go prints with its message: the JSON envelope with the
    /// literal `--json` token, `Error: <message>` on stderr otherwise (SD5).
    Message(String),
    /// Not ported, or the library failed: returned as `Err` from [`run`].
    Library(Error),
}

impl Failure {
    /// A command error with Go's `message`.
    pub fn message(message: impl Into<String>) -> Self {
        Self::Message(message.into())
    }
}

impl From<OptionError> for Failure {
    fn from(error: OptionError) -> Self {
        match error {
            OptionError::Unknown(_) => Self::UnknownOption(error.to_string()),
            OptionError::Missing(_) => Self::Message(error.to_string()),
        }
    }
}

impl From<Error> for Failure {
    /// Errors whose text is Go's become command errors; the rest are
    /// returned from [`run`].
    fn from(error: Error) -> Self {
        match error {
            Error::NotInProject | Error::LockTimeout { .. } => Self::Message(error.to_string()),
            other => Self::Library(other),
        }
    }
}

/// A command handler: the context and the arguments after the command name.
type Handler = fn(&mut Context<'_>, &[String]) -> std::result::Result<(), Failure>;

/// A command Go registers.
#[derive(Clone, Copy)]
enum Entry {
    /// Ported by this crate.
    Ported(Handler),
    /// Ported by the named plan.
    Deferred(&'static str),
}

/// Every command Go registers, in Go's order (`internal/cli/commands.go`,
/// `internal/plan`).
const COMMANDS: &[(&str, Entry)] = &[
    ("init", Entry::Ported(init::run)),
    ("create", Entry::Ported(create::run)),
    ("show", Entry::Ported(read::show)),
    ("list", Entry::Ported(read::list)),
    ("ready", Entry::Deferred(PLAN_2)),
    ("search", Entry::Ported(read::search)),
    ("update", Entry::Ported(update::run)),
    ("close", Entry::Deferred(PLAN_2)),
    ("dep", Entry::Deferred(PLAN_2)),
    ("label", Entry::Deferred(PLAN_2)),
    ("blocked", Entry::Deferred(PLAN_2)),
    ("stats", Entry::Deferred(PLAN_2)),
    ("sync", Entry::Deferred(PLAN_3)),
    ("doctor", Entry::Deferred(PLAN_3)),
    ("tpl", Entry::Deferred(PLAN_3)),
    ("migrate-from-beads", Entry::Deferred(PLAN_3)),
    ("prime", Entry::Deferred(PLAN_3)),
    ("onboard", Entry::Deferred(PLAN_3)),
    ("upgrade", Entry::Deferred(PLAN_3)),
    ("completions", Entry::Deferred(PLAN_2)),
    ("block", Entry::Ported(block::run)),
    ("unblock", Entry::Deferred(PLAN_2)),
    ("plan", Entry::Deferred(PLAN_4)),
    ("config", Entry::Deferred(PLAN_3)),
    ("setup", Entry::Deferred(PLAN_3)),
    ("guard", Entry::Deferred(PLAN_3)),
];

fn find(name: &str) -> Option<Entry> {
    COMMANDS
        .iter()
        .find(|(command, _)| *command == name)
        .map(|(_, entry)| *entry)
}

/// Go's `contains`: whether `value` is one of `args`.
fn contains(args: &[String], value: &str) -> bool {
    args.iter().any(|arg| arg == value)
}

/// Runs one command as the Go tool would.
pub fn run(invocation: &Invocation) -> Result<Outcome> {
    dispatch(invocation, find)
}

/// Refuses the logging environment (SD8), ported in s4-missions-lib-2.
fn refuse_debug_logging(env: &BTreeMap<String, String>) -> Result<()> {
    let debug = env
        .get("LUUNA_MISSIONS_DEBUG")
        .is_some_and(|value| value == "1");
    let level = env
        .get("LUUNA_MISSIONS_LOG_LEVEL")
        .filter(|value| matches!(value.as_str(), "debug" | "trace"));
    if debug || level.is_some() {
        return Err(Error::unsupported(
            "debug logging (LUUNA_MISSIONS_DEBUG=1 or LUUNA_MISSIONS_LOG_LEVEL=debug|trace, \
             which writes debug records to stdout, SD8)",
            PLAN_2,
        ));
    }
    Ok(())
}

/// Go's `App.Run` with the command table `find`.
fn dispatch(invocation: &Invocation, find: fn(&str) -> Option<Entry>) -> Result<Outcome> {
    refuse_debug_logging(&invocation.env)?;
    let raw = invocation.args.as_slice();
    let json_mode = contains(raw, "--json");
    let version = contains(raw, "--version") || contains(raw, "-v");
    if version && json_mode {
        return Err(Error::unsupported(
            "--version --json (version output)",
            PLAN_2,
        ));
    }
    let mut args = raw;
    while let Some(first) = args.first() {
        if !matches!(first.as_str(), "-q" | "--quiet" | "--verbose" | "--timing") {
            break;
        }
        args = &args[1..];
    }
    let Some(name) = args.first() else {
        return Err(Error::unsupported(
            "help output (no command was given)",
            PLAN_2,
        ));
    };
    let entry = find(name);
    if version && (name.starts_with('-') || entry.is_some()) {
        return Err(Error::unsupported("--version (version output)", PLAN_2));
    }
    if name == "--help"
        || name == "-h"
        || (name.starts_with('-') && (contains(args, "--help") || contains(args, "-h")))
    {
        return Err(Error::unsupported("--help (help output)", PLAN_2));
    }
    let mut outcome = Outcome::default();
    if name.starts_with('-') {
        if let Err(error) = options::parse(std::slice::from_ref(name), &[], &[]) {
            outcome.stderr = format!("error: {error}\n").into_bytes();
            outcome.exit_code = 1;
            return Ok(outcome);
        }
    }
    let Some(entry) = entry else {
        return Err(Error::unsupported(
            format!("the unknown command {name:?} (Go's unknown-command message and suggestion)"),
            PLAN_2,
        ));
    };
    if contains(&args[1..], "--help") || contains(&args[1..], "-h") {
        return Err(Error::unsupported(
            format!("{name} --help (command help output)"),
            PLAN_2,
        ));
    }
    let handler = match entry {
        Entry::Ported(handler) => handler,
        Entry::Deferred(plan) => {
            return Err(Error::unsupported(format!("the {name} command"), plan))
        }
    };
    let mut context = Context::new(invocation, name)?;
    let start = Instant::now();
    let result = handler(&mut context, &args[1..]);
    if context.timing && result.is_ok() {
        let line = format!("⏱ {}\n", elapsed(start.elapsed()));
        context.stderr.extend_from_slice(line.as_bytes());
    }
    outcome.exit_code = match result {
        Ok(()) => context.exit_code,
        Err(Failure::Library(error)) => return Err(error),
        Err(Failure::UnknownOption(message)) => {
            context
                .stderr
                .extend_from_slice(format!("error: {message}\n").as_bytes());
            1
        }
        Err(Failure::Message(message)) => {
            command_error(&mut context, name, json_mode, &message);
            1
        }
    };
    outcome.stdout = context.stdout;
    outcome.stderr = context.stderr;
    Ok(outcome)
}

/// Go's `commandError`: the JSON error envelope on stdout when the literal
/// `--json` token was given, `Error: <message>` on stderr otherwise (SD5).
fn command_error(context: &mut Context<'_>, name: &str, json_mode: bool, message: &str) {
    if json_mode {
        context.json(Envelope::new(false, name).string("error", message));
    } else {
        context
            .stderr
            .extend_from_slice(format!("Error: {message}\n").as_bytes());
    }
}

/// The `--timing` value: whole milliseconds below one second, seconds with
/// two decimals from one second on.
fn elapsed(duration: Duration) -> String {
    let seconds = duration.as_secs_f64();
    if duration >= Duration::from_secs(1) {
        format!("{seconds:.2}s")
    } else {
        format!("{:.0}ms", (seconds * 1000.0).round())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn invocation(args: &[&str]) -> Invocation {
        Invocation::new(std::env::temp_dir(), args.iter().copied())
    }

    fn text(bytes: &[u8]) -> &str {
        std::str::from_utf8(bytes).unwrap()
    }

    fn plan_of(error: Error) -> String {
        match error {
            Error::Unsupported { plan, .. } => plan,
            other => panic!("expected Unsupported, got {other}"),
        }
    }

    /// A command table with test commands that fail in each way.
    fn test_find(name: &str) -> Option<Entry> {
        fn fails(_: &mut Context<'_>, _: &[String]) -> std::result::Result<(), Failure> {
            Err(Failure::message("it <failed> & stopped"))
        }
        fn parses(_: &mut Context<'_>, args: &[String]) -> std::result::Result<(), Failure> {
            options::parse(args, &["format", "title"], &["json"])?;
            Err(Failure::message("parsed"))
        }
        fn succeeds(context: &mut Context<'_>, _: &[String]) -> std::result::Result<(), Failure> {
            context.success(b"done");
            context.warning(b"careful");
            Ok(())
        }
        fn unsupported(_: &mut Context<'_>, _: &[String]) -> std::result::Result<(), Failure> {
            Err(Error::unsupported("thing", "later").into())
        }
        match name {
            "fails" => Some(Entry::Ported(fails)),
            "parses" => Some(Entry::Ported(parses)),
            "succeeds" => Some(Entry::Ported(succeeds)),
            "unsupported" => Some(Entry::Ported(unsupported)),
            other => find(other),
        }
    }

    fn dispatched(args: &[&str]) -> Outcome {
        dispatch(&invocation(args), test_find).unwrap()
    }

    #[test]
    fn invocation_builder_sets_every_field() {
        let env = BTreeMap::from([("USER".to_owned(), "parity".to_owned())]);
        let invocation = Invocation::new("/work", ["list", "--json"])
            .with_stdin("input")
            .with_env(env.clone())
            .with_store_dir("/work/.luuna/missions");
        assert_eq!(invocation.args, ["list", "--json"]);
        assert_eq!(invocation.cwd, PathBuf::from("/work"));
        assert_eq!(invocation.stdin, b"input");
        assert_eq!(invocation.env, env);
        assert_eq!(
            invocation.store_dir,
            Some(PathBuf::from("/work/.luuna/missions"))
        );
    }

    #[test]
    fn the_json_error_envelope_needs_the_literal_json_token() {
        let outcome = dispatched(&["fails", "--json"]);
        assert_eq!(outcome.exit_code, 1);
        assert_eq!(
            text(&outcome.stdout),
            "{\n  \"success\": false,\n  \"command\": \"fails\",\n  \"error\": \"it <failed> & stopped\"\n}\n"
        );
        assert!(outcome.stderr.is_empty());
        // The token anywhere counts, even after the command's own options.
        let outcome = dispatched(&["--quiet", "fails", "x", "--json"]);
        assert_eq!(outcome.exit_code, 1);
        assert!(text(&outcome.stdout).contains("\"success\": false"));
    }

    #[test]
    fn a_failure_under_format_json_alone_prints_the_human_error() {
        // SD5.
        for args in [
            &["parses", "--format", "json"][..],
            &["parses", "--format=json"],
            &["fails"],
        ] {
            let outcome = dispatched(args);
            assert_eq!(outcome.exit_code, 1, "{args:?}");
            assert!(outcome.stdout.is_empty(), "{args:?}");
            assert!(
                text(&outcome.stderr).starts_with("Error: "),
                "{args:?}: {}",
                text(&outcome.stderr)
            );
        }
        let outcome = dispatched(&["fails"]);
        assert_eq!(text(&outcome.stderr), "Error: it <failed> & stopped\n");
    }

    #[test]
    fn an_unknown_option_is_never_printed_as_json() {
        // SD6: inside a command, and as the first argument.
        for args in [
            &["parses", "--nope", "--json"][..],
            &["parses", "-x", "--json"],
            &["--nope", "parses", "--json"],
            &["--json", "parses"],
        ] {
            let outcome = dispatched(args);
            assert_eq!(outcome.exit_code, 1, "{args:?}");
            assert!(outcome.stdout.is_empty(), "{args:?}");
            assert!(
                text(&outcome.stderr).starts_with("error: unknown option '"),
                "{args:?}"
            );
        }
        assert_eq!(
            text(&dispatched(&["--json", "parses"]).stderr),
            "error: unknown option '--json'\n"
        );
    }

    #[test]
    fn a_missing_value_is_an_ordinary_command_error() {
        // SD7: a value beginning with `--` is missing, and the error follows
        // the --json rule.
        let outcome = dispatched(&["parses", "--title", "--json"]);
        assert_eq!(outcome.exit_code, 1);
        assert!(text(&outcome.stdout).contains("option '--title' argument missing"));
        let outcome = dispatched(&["parses", "--title", "--format"]);
        assert_eq!(
            text(&outcome.stderr),
            "Error: option '--title' argument missing\n"
        );
    }

    #[test]
    fn global_switches_are_skipped_and_read_anywhere() {
        let outcome = dispatched(&["-q", "--verbose", "succeeds"]);
        assert_eq!(outcome, Outcome::default());
        let outcome = dispatched(&["succeeds", "--quiet"]);
        assert!(outcome.stdout.is_empty() && outcome.stderr.is_empty());
        let outcome = dispatched(&["succeeds"]);
        assert_eq!(text(&outcome.stdout), "✓ done\n");
        assert_eq!(text(&outcome.stderr), "! careful\n");
        let outcome = dispatched(&["--timing", "succeeds"]);
        assert_eq!(outcome.exit_code, 0);
        let stderr = text(&outcome.stderr);
        assert!(stderr.starts_with("! careful\n⏱ "), "{stderr}");
        assert!(stderr.ends_with("ms\n"), "{stderr}");
        // No timing line after a failure.
        assert!(!text(&dispatched(&["fails", "--timing"]).stderr).contains('⏱'));
    }

    #[test]
    fn the_timing_value_follows_go() {
        assert_eq!(elapsed(Duration::from_micros(400)), "0ms");
        assert_eq!(elapsed(Duration::from_micros(12_600)), "13ms");
        assert_eq!(elapsed(Duration::from_millis(999)), "999ms");
        assert_eq!(elapsed(Duration::from_millis(1_234)), "1.23s");
    }

    #[test]
    fn library_errors_are_returned_and_discard_output() {
        let error = dispatch(&invocation(&["unsupported", "--json"]), test_find).unwrap_err();
        assert_eq!(plan_of(error), "later");
    }

    #[test]
    fn the_ported_commands_are_registered() {
        for name in [
            "init", "create", "show", "list", "search", "update", "block",
        ] {
            assert!(
                matches!(find(name), Some(Entry::Ported(_))),
                "{name} is not ported"
            );
        }
    }

    #[test]
    fn every_other_go_command_is_unsupported_naming_its_plan() {
        let expected = [
            ("ready", PLAN_2),
            ("close", PLAN_2),
            ("dep", PLAN_2),
            ("label", PLAN_2),
            ("blocked", PLAN_2),
            ("stats", PLAN_2),
            ("completions", PLAN_2),
            ("unblock", PLAN_2),
            ("sync", PLAN_3),
            ("doctor", PLAN_3),
            ("tpl", PLAN_3),
            ("migrate-from-beads", PLAN_3),
            ("prime", PLAN_3),
            ("onboard", PLAN_3),
            ("upgrade", PLAN_3),
            ("config", PLAN_3),
            ("setup", PLAN_3),
            ("guard", PLAN_3),
            ("plan", PLAN_4),
        ];
        assert_eq!(COMMANDS.len(), expected.len() + 7);
        for (name, plan) in expected {
            let error = run(&invocation(&[name, "--json"])).unwrap_err();
            assert_eq!(plan_of(error), plan, "{name}");
        }
    }

    #[test]
    fn deferred_options_and_unknown_commands_are_unsupported() {
        for args in [
            &[][..],
            &["-q"],
            &["--version"],
            &["-v"],
            &["init", "-v"],
            &["list", "--json", "--version"],
            &["nope", "--json", "-v"],
            &["--help"],
            &["-h"],
            &["--json", "--help"],
            &["init", "--help"],
            &["list", "-h"],
            &["nope"],
            &["inti", "--json"],
            &["--"],
        ] {
            let error = run(&invocation(args)).unwrap_err();
            assert_eq!(plan_of(error), PLAN_2, "{args:?}");
        }
    }

    #[test]
    fn debug_logging_environments_are_unsupported() {
        // SD8, deferred to s4-missions-lib-2.
        for (key, value) in [
            ("LUUNA_MISSIONS_DEBUG", "1"),
            ("LUUNA_MISSIONS_LOG_LEVEL", "debug"),
            ("LUUNA_MISSIONS_LOG_LEVEL", "trace"),
        ] {
            let env = BTreeMap::from([(key.to_owned(), value.to_owned())]);
            let error = run(&invocation(&["init", "--json"]).with_env(env)).unwrap_err();
            assert_eq!(plan_of(error), PLAN_2, "{key}={value}");
        }
        // Other values do not log in Go and are accepted.
        for (key, value) in [
            ("LUUNA_MISSIONS_DEBUG", "0"),
            ("LUUNA_MISSIONS_LOG_LEVEL", "info"),
        ] {
            let env = BTreeMap::from([(key.to_owned(), value.to_owned())]);
            let outcome = dispatch(&invocation(&["fails"]).with_env(env), test_find).unwrap();
            assert_eq!(outcome.exit_code, 1, "{key}={value}");
        }
    }

    #[test]
    fn an_explicit_store_directory_overrides_discovery() {
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().join("project");
        let discovered = project.join(".luuna/missions");
        std::fs::create_dir_all(&discovered).unwrap();
        std::fs::write(discovered.join("config.yaml"), "project: p\n").unwrap();
        let elsewhere = temp.path().join("elsewhere");
        let deep = project.join("sub");
        std::fs::create_dir_all(&deep).unwrap();

        let plain = Invocation::new(&deep, ["list"]);
        let context = Context::new(&plain, "list").unwrap();
        assert_eq!(context.store().unwrap().dir(), discovered);

        // The explicit directory is used as it is, even without config.yaml
        // and with a store that discovery would find.
        let explicit = Invocation::new(&deep, ["list"]).with_store_dir(&elsewhere);
        let context = Context::new(&explicit, "list").unwrap();
        assert_eq!(context.store().unwrap().dir(), elsewhere);

        // Discovery from a directory without a store gives Go's message as
        // a command error, unless a directory is given.
        let outside = Invocation::new(temp.path(), ["list"]);
        let context = Context::new(&outside, "list").unwrap();
        let failure = Failure::from(context.store().unwrap_err());
        assert!(matches!(
            failure,
            Failure::Message(message)
                if message == "Not in a missions project. Run `luuna-missions init` first."
        ));
        let given = Invocation::new(temp.path(), ["list"]).with_store_dir(&discovered);
        let context = Context::new(&given, "list").unwrap();
        assert_eq!(context.store().unwrap().dir(), discovered);
    }

    #[test]
    fn the_context_reads_the_invocation() {
        let env = BTreeMap::from([("USER".to_owned(), "me".to_owned())]);
        let invocation = Invocation::new("/a/./b/../c/", ["x", "--timing", "-q"])
            .with_stdin("in")
            .with_env(env);
        let context = Context::new(&invocation, "x").unwrap();
        assert_eq!(context.cwd, PathBuf::from("/a/c"));
        assert!(context.quiet && context.timing && !context.verbose);
        assert_eq!(context.stdin(), b"in");
        assert_eq!(context.env("USER"), Some("me"));
        assert_eq!(context.env("HOME"), None);
    }
}
