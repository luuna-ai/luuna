//! Process entrypoint and compatibility dispatcher for `luuna-roots` and `lrt`.

pub mod commands;
pub mod commands_agent;
pub mod commands_capture;
pub mod commands_flow;
pub mod commands_integrity;
pub mod commands_read;
pub mod commands_review;
pub mod commands_tend;
pub mod commands_think;
pub mod editor;
pub mod interactive;
pub mod io;
pub mod output;
pub mod registry;

use crate::io::{Io, ProcessIo};
use crate::output::{Output, OutputOptions};
use crate::registry::{
    COMMANDS, CommandContext, CommandDef, find_command, flags_for, render_command_help,
    render_root_help,
};
use roots_core::args::parse_args;
use roots_core::color::{Colors, color_enabled};
use roots_core::errors::{EXIT_OK, ErrorCode, RootsError};
use roots_core::output::{error_envelope, pretty_json};
use roots_core::serde_json::{Map, Value};
use roots_core::suggestions::closest;
use roots_core::{NAME, VERSION};

/// Run the command-line interface against the real process streams.
pub fn run(argv: Vec<String>) -> i32 {
    let mut io = ProcessIo::from_process();
    run_cli(&argv, &mut io)
}

/// Run the CLI against injectable process I/O.
pub fn run_cli(argv: &[String], io: &mut dyn Io) -> i32 {
    // This deliberately inspects raw argv. An error before `parse_args` still
    // uses JSON if the exact `--json` token appeared anywhere in the command.
    let json = argv.iter().any(|argument| argument == "--json");
    let Some(first) = argv.first().map(String::as_str) else {
        return print_root_help(io, json);
    };
    let rest = &argv[1..];

    if first == "--version" || first == "-v" {
        return print_version(io, json);
    }
    if first == "--help" || first == "-h" || first == "--json" {
        return print_root_help(io, json);
    }
    if first == "help" {
        if let Some(target) = rest.first().and_then(|name| find_command(name)) {
            // Equivalent to dispatching `["--help"]`, but this branch cannot
            // fail and preserves the legacy plain-text help behavior.
            io.stdout(&render_command_help(target, colors_for(io)));
            return EXIT_OK;
        }
        return print_root_help(io, json);
    }

    let definition = find_command(first);
    let outcome = match definition {
        Some(definition) => run_command(definition, rest, io),
        None => Err(unknown_command(first)),
    };
    match outcome {
        Ok(()) => EXIT_OK,
        Err(error) => report_error(
            io,
            &error,
            definition.map(|command| command.name).or(Some(first)),
            json,
        ),
    }
}

fn colors_for(io: &dyn Io) -> Colors {
    Colors::new(color_enabled(
        io.env("NO_COLOR"),
        io.env("FORCE_COLOR"),
        io.stdout_is_tty(),
    ))
}

fn print_version(io: &mut dyn Io, json: bool) -> i32 {
    if json {
        let mut body = Map::new();
        body.insert("success".to_owned(), Value::Bool(true));
        body.insert("command".to_owned(), Value::String("version".to_owned()));
        body.insert("name".to_owned(), Value::String(NAME.to_owned()));
        body.insert("version".to_owned(), Value::String(VERSION.to_owned()));
        io.stdout(&pretty_json(&Value::Object(body)));
    } else {
        io.stdout(&format!("{VERSION}\n"));
    }
    EXIT_OK
}

fn print_root_help(io: &mut dyn Io, json: bool) -> i32 {
    if json {
        let commands = COMMANDS
            .iter()
            .map(|command| {
                let mut item = Map::new();
                item.insert("name".to_owned(), Value::String(command.name.to_owned()));
                item.insert("usage".to_owned(), Value::String(command.usage.to_owned()));
                item.insert(
                    "summary".to_owned(),
                    Value::String(command.summary.to_owned()),
                );
                Value::Object(item)
            })
            .collect();
        let mut body = Map::new();
        body.insert("success".to_owned(), Value::Bool(true));
        body.insert("command".to_owned(), Value::String("help".to_owned()));
        body.insert("version".to_owned(), Value::String(VERSION.to_owned()));
        body.insert("commands".to_owned(), Value::Array(commands));
        io.stdout(&pretty_json(&Value::Object(body)));
    } else {
        io.stdout(&render_root_help(COMMANDS, VERSION, colors_for(io)));
    }
    EXIT_OK
}

fn run_command(
    definition: &CommandDef,
    argv: &[String],
    io: &mut dyn Io,
) -> Result<(), RootsError> {
    let specifications = flags_for(definition);
    let parsed = parse_args(argv, &specifications)?;
    // Command help is checked before Output creation. Thus a trailing --json
    // does not turn help into JSON or disable TTY color.
    if roots_core::args::flag_bool(&parsed.flags, "help") {
        io.stdout(&render_command_help(definition, colors_for(io)));
        return Ok(());
    }
    check_arity(definition, parsed.positionals.len())?;
    let output = Output::new(
        io,
        OutputOptions {
            command: definition.name,
            json: roots_core::args::flag_bool(&parsed.flags, "json"),
            quiet: roots_core::args::flag_bool(&parsed.flags, "quiet"),
        },
    );
    (definition.run)(CommandContext {
        command: definition.name,
        io,
        out: &output,
        args: &parsed.positionals,
        flags: &parsed.flags,
    })
}

fn check_arity(definition: &CommandDef, count: usize) -> Result<(), RootsError> {
    if count < definition.min_args {
        return Err(RootsError::usage(format!(
            "missing argument; usage: roots {}",
            definition.usage
        )));
    }
    if definition.max_args.is_some_and(|maximum| count > maximum) {
        return Err(RootsError::usage(format!(
            "too many arguments; usage: roots {}",
            definition.usage
        )));
    }
    Ok(())
}

fn unknown_command(name: &str) -> RootsError {
    let names: Vec<&str> = COMMANDS.iter().map(|command| command.name).collect();
    let suggestion = closest(name, &names, 2);
    let suffix = suggestion
        .map(|value| format!(". Did you mean `roots {value}`?"))
        .unwrap_or_default();
    let mut detail = Map::new();
    detail.insert(
        "suggestion".to_owned(),
        suggestion.map_or(Value::Null, |value| Value::String(value.to_owned())),
    );
    RootsError::usage(format!("unknown command: {name}{suffix}")).with_detail(detail)
}

fn report_error(io: &mut dyn Io, error: &RootsError, command: Option<&str>, json: bool) -> i32 {
    if json {
        io.stdout(&error_envelope(
            command,
            &error.message,
            error.code,
            error.detail.as_ref(),
        ));
    } else {
        io.stderr(&format!(
            "{} {}\n",
            colors_for(io).red("error:"),
            error.message
        ));
    }
    error.exit_code
}

/// Make it possible for command crates to turn caught non-Roots failures into
/// the stable generic error form at their own boundary.
pub fn internal_error(message: impl Into<String>) -> RootsError {
    RootsError::new(message, ErrorCode::Error, roots_core::errors::EXIT_ERROR)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::MemoryIo;
    use roots_core::errors::EXIT_USAGE;
    use roots_core::serde_json::Value;
    use std::path::PathBuf;

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|part| (*part).to_owned()).collect()
    }

    fn memory() -> MemoryIo {
        MemoryIo::new(PathBuf::from("/tmp/roots-cli-test"))
    }

    #[test]
    fn version_and_root_help_follow_the_legacy_shapes() {
        let mut io = memory();
        assert_eq!(run_cli(&argv(&["--version"]), &mut io), 0);
        assert_eq!(io.stdout, "0.0.0\n");

        let mut io = memory();
        assert_eq!(run_cli(&argv(&["--version", "--json"]), &mut io), 0);
        let value: Value = roots_core::serde_json::from_str(&io.stdout).unwrap();
        assert_eq!(value["command"], "version");
        assert_eq!(value["name"], "@luuna/roots-cli");

        let mut io = memory();
        assert_eq!(run_cli(&argv(&["--help", "--json"]), &mut io), 0);
        let value: Value = roots_core::serde_json::from_str(&io.stdout).unwrap();
        assert_eq!(value["commands"].as_array().unwrap().len(), 33);
        assert_eq!(value["commands"][0]["name"], "init");
    }

    #[test]
    fn unknown_command_and_parse_errors_preserve_json_and_exit_codes() {
        let mut io = memory();
        assert_eq!(run_cli(&argv(&["lst"]), &mut io), EXIT_USAGE);
        assert!(io.stderr.contains("Did you mean `roots list`?"));

        let mut io = memory();
        assert_eq!(run_cli(&argv(&["shw", "--json"]), &mut io), EXIT_USAGE);
        let value: Value = roots_core::serde_json::from_str(&io.stdout).unwrap();
        assert_eq!(value["command"], "shw");
        assert_eq!(value["code"], "usage");
        assert_eq!(value["suggestion"], "show");

        let mut io = memory();
        assert_eq!(run_cli(&argv(&["list", "--orphan"]), &mut io), EXIT_USAGE);
        assert!(io.stderr.contains("Did you mean --orphans?"));
    }

    #[test]
    fn command_help_precedes_json_and_implemented_commands_report_real_errors() {
        let mut io = memory();
        io.stdout_is_tty = true;
        assert_eq!(run_cli(&argv(&["list", "--help", "--json"]), &mut io), 0);
        assert!(io.stdout.contains("Usage:"));
        assert!(!io.stdout.trim_start().starts_with('{'));

        let mut io = memory();
        assert_eq!(
            run_cli(&argv(&["list", "--json"]), &mut io),
            roots_core::errors::EXIT_NOT_FOUND
        );
        let value: Value = roots_core::serde_json::from_str(&io.stdout).unwrap();
        assert_eq!(value["command"], "list");
        assert_eq!(value["code"], "not_initialized");
    }

    #[test]
    fn arity_errors_are_checked_before_handlers() {
        let mut io = memory();
        assert_eq!(run_cli(&argv(&["show"]), &mut io), EXIT_USAGE);
        assert!(
            io.stderr
                .contains("missing argument; usage: roots show <id>")
        );

        let mut io = memory();
        assert_eq!(run_cli(&argv(&["show", "one", "two"]), &mut io), EXIT_USAGE);
        assert!(
            io.stderr
                .contains("too many arguments; usage: roots show <id>")
        );
    }
}
