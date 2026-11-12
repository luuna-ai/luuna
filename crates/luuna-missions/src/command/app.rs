//! The dispatcher behaviour of Go's `App.Run` that s4-missions-lib-2 owns
//! (`internal/cli/app.go`, `internal/cli/help.go`,
//! `internal/cli/logging.go`): help and version output, unknown commands,
//! command help and the logging variables.
//!
//! `dispatch` calls these hooks in fixed places. At this commit each returns
//! exactly what the foundation's dispatcher returned there: an
//! [`Error::Unsupported`] naming s4-missions-lib-2, or no debug record.
//! s4-missions-lib-2 replaces their bodies and the `checks` it owns, and
//! does not edit `command/mod.rs`.

use std::collections::BTreeMap;

use super::{Context, Outcome};
use crate::error::{Error, Result, PLAN_2};

/// Refuses the logging environment (SD8), ported in s4-missions-lib-2.
pub(super) fn refuse_debug_logging(env: &BTreeMap<String, String>) -> Result<()> {
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

/// The answer to `--version` or `-v` together with the literal `--json`
/// token anywhere in the arguments.
pub(super) fn version_json() -> Result<Outcome> {
    Err(Error::unsupported(
        "--version --json (version output)",
        PLAN_2,
    ))
}

/// The answer when no command name follows the leading global switches.
pub(super) fn help_without_command() -> Result<Outcome> {
    Err(Error::unsupported(
        "help output (no command was given)",
        PLAN_2,
    ))
}

/// The answer to `--version` or `-v` before a command or after a known one.
pub(super) fn version() -> Result<Outcome> {
    Err(Error::unsupported("--version (version output)", PLAN_2))
}

/// The answer to `--help` or `-h` in place of a command name, or after a
/// leading option.
pub(super) fn help() -> Result<Outcome> {
    Err(Error::unsupported("--help (help output)", PLAN_2))
}

/// The answer to the unknown command `name`; `json_mode` is whether the
/// literal `--json` token was given.
pub(super) fn unknown_command(name: &str, _json_mode: bool) -> Result<Outcome> {
    Err(Error::unsupported(
        format!("the unknown command {name:?} (Go's unknown-command message and suggestion)"),
        PLAN_2,
    ))
}

/// The answer to `--help` or `-h` after a known command. `args` are the
/// arguments after the leading global switches, so `args[0]` is the command
/// name.
pub(super) fn command_help(args: &[String]) -> Result<Outcome> {
    let name = &args[0];
    Err(Error::unsupported(
        format!("{name} --help (command help output)"),
        PLAN_2,
    ))
}

/// The debug record Go writes to stdout before a command error under the
/// logging variables (`logDebugError`, `internal/cli/logging.go:26-51`).
/// None at this commit: the logging variables are refused before any
/// command runs.
pub(super) fn debug_error_record(_context: &Context<'_>, _name: &str, _message: &str) -> Vec<u8> {
    Vec::new()
}

/// The expectations of the dispatcher tests in `command/mod.rs`, whose
/// inputs stay there. While deferred, each asserts the refusal exactly as
/// the foundation made it; once s4-missions-lib-2 ports the behaviour, Go's
/// exact result for the same input, as literals. An input with no
/// expectation panics.
#[cfg(test)]
pub(super) mod checks {
    use std::collections::BTreeMap;

    use super::super::{run, Invocation};
    use crate::error::{Error, PLAN_2};

    fn invocation(args: &[&str]) -> Invocation {
        Invocation::new(std::env::temp_dir(), args.iter().copied())
    }

    fn plan_of(error: Error) -> String {
        match error {
            Error::Unsupported { plan, .. } => plan,
            other => panic!("expected Unsupported, got {other}"),
        }
    }

    /// `run(args)` in the system temp folder: Unsupported naming
    /// s4-missions-lib-2, with the feature the foundation's dispatcher names
    /// for that input.
    pub(in crate::command) fn answer(args: &[&str]) {
        let feature = match args {
            [] | ["-q"] => "help output (no command was given)",
            ["--version"] | ["-v"] | ["init", "-v"] => "--version (version output)",
            ["list", "--json", "--version"] | ["nope", "--json", "-v"] => {
                "--version --json (version output)"
            }
            ["--help"] | ["-h"] | ["--json", "--help"] => "--help (help output)",
            ["init", "--help"] => "init --help (command help output)",
            ["list", "-h"] => "list --help (command help output)",
            ["nope"] => {
                "the unknown command \"nope\" (Go's unknown-command message and suggestion)"
            }
            ["inti", "--json"] => {
                "the unknown command \"inti\" (Go's unknown-command message and suggestion)"
            }
            ["--"] => "the unknown command \"--\" (Go's unknown-command message and suggestion)",
            other => panic!("no expectation for {other:?}"),
        };
        let error = run(&invocation(args)).unwrap_err();
        assert_eq!(
            error.to_string(),
            format!("{feature} is not supported yet; it is ported in s4-missions-lib-2"),
            "{args:?}"
        );
        assert_eq!(plan_of(error), PLAN_2, "{args:?}");
    }

    /// `init --json` in the system temp folder with only `key=value` in the
    /// environment: Unsupported naming s4-missions-lib-2.
    pub(in crate::command) fn debug_environment(key: &str, value: &str) {
        match (key, value) {
            ("LUUNA_MISSIONS_DEBUG", "1")
            | ("LUUNA_MISSIONS_LOG_LEVEL", "debug")
            | ("LUUNA_MISSIONS_LOG_LEVEL", "trace") => {}
            other => panic!("no expectation for {other:?}"),
        }
        let env = BTreeMap::from([(key.to_owned(), value.to_owned())]);
        let error = run(&invocation(&["init", "--json"]).with_env(env)).unwrap_err();
        assert_eq!(plan_of(error), PLAN_2, "{key}={value}");
    }
}
