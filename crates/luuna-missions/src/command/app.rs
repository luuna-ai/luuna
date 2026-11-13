//! The dispatcher behaviour of Go's `App.Run` that s4-missions-lib-2 owns
//! (`internal/cli/app.go`, `internal/cli/help.go`,
//! `internal/cli/logging.go`): help and version output, unknown commands,
//! command help and the logging variables.
//!
//! `dispatch` calls these hooks in fixed places, and does not change.

mod help;
mod logging;

use std::collections::BTreeMap;

use super::output::Envelope;
use super::{Context, Outcome};
use crate::error::Result;

/// The Go tool's version.
const VERSION: &str = help::VERSION;

/// Logging variables resolved as Go does; on the targets whose host name is
/// not read as Go reads it, a level that logs is refused (CF8).
pub(super) fn refuse_debug_logging(env: &BTreeMap<String, String>) -> Result<()> {
    logging::refuse_debug_logging(env)
}

/// An outcome with `stdout`, `stderr` and `exit_code`.
fn outcome(stdout: Vec<u8>, stderr: Vec<u8>, exit_code: i32) -> Result<Outcome> {
    Ok(Outcome {
        stdout,
        stderr,
        exit_code,
    })
}

/// Go's `goPlatform`: the operating system and architecture in the spelling
/// of the version JSON (`app.go:344`).
fn platform() -> String {
    let os = match std::env::consts::OS {
        "macos" => "darwin",
        "windows" => "win32",
        other => other,
    };
    let arch = match std::env::consts::ARCH {
        "x86_64" => "x64",
        "x86" => "386",
        "aarch64" => "arm64",
        "powerpc64" => "ppc64",
        "powerpc" => "ppc",
        other => other,
    };
    format!("{os}-{arch}")
}

/// The answer to `--version` or `-v` together with the literal `--json`
/// token anywhere in the arguments (`app.go:142-155`).
pub(super) fn version_json() -> Result<Outcome> {
    let envelope = Envelope::new(true, "version")
        .string("name", "@luuna/missions-cli")
        .string("version", VERSION)
        .string("runtime", "go")
        .string("platform", &platform());
    outcome(envelope.render(), Vec::new(), 0)
}

/// The answer when no command name follows the leading global switches:
/// the root help on stderr, exit 1 (SD219).
pub(super) fn help_without_command() -> Result<Outcome> {
    outcome(Vec::new(), help::root().into_bytes(), 1)
}

/// The answer to `--version` or `-v` before a command or after a known one.
pub(super) fn version() -> Result<Outcome> {
    outcome(format!("{VERSION}\n").into_bytes(), Vec::new(), 0)
}

/// The answer to `--help` or `-h` in place of a command name, or after a
/// leading option: the root help on stdout, exit 0.
pub(super) fn help() -> Result<Outcome> {
    outcome(help::root().into_bytes(), Vec::new(), 0)
}

/// Go's `levenshtein`: the edit distance in runes.
fn levenshtein(a: &str, b: &str) -> usize {
    let right: Vec<char> = b.chars().collect();
    let mut previous: Vec<usize> = (0..=right.len()).collect();
    for (i, left) in a.chars().enumerate() {
        let mut current = vec![i + 1];
        for (j, other) in right.iter().enumerate() {
            let cost = usize::from(left != *other);
            current.push(
                (current[j] + 1)
                    .min(previous[j + 1] + 1)
                    .min(previous[j] + cost),
            );
        }
        previous = current;
    }
    previous[right.len()]
}

/// Go's `suggest`: the closest command (the first on a tie) within a
/// distance of 2, and the message.
fn suggest(input: &str) -> (&'static str, String) {
    let mut best = "";
    let mut distance = usize::MAX;
    for (name, _, _) in help::COMMANDS {
        let candidate = levenshtein(input, name);
        if candidate < distance {
            best = name;
            distance = candidate;
        }
    }
    if distance <= 2 {
        (
            best,
            format!("Unknown command: {input}. Did you mean {best}?"),
        )
    } else {
        ("", format!("Unknown command: {input}"))
    }
}

/// The answer to the unknown command `name`; `json_mode` is whether the
/// literal `--json` token was given.
pub(super) fn unknown_command(name: &str, json_mode: bool) -> Result<Outcome> {
    let (suggestion, message) = suggest(name);
    if !json_mode {
        return outcome(Vec::new(), format!("{message}\n").into_bytes(), 1);
    }
    let mut envelope = Envelope::new(false, name).string("error", &message);
    if !suggestion.is_empty() {
        envelope = envelope.string("suggestion", suggestion);
    }
    outcome(envelope.render(), Vec::new(), 1)
}

/// The answer to `--help` or `-h` after a known command. `args` are the
/// arguments after the leading global switches, so `args[0]` is the command
/// name.
pub(super) fn command_help(args: &[String]) -> Result<Outcome> {
    let text =
        help::snapshot(args).map_or_else(|| help::fallback(&args[0]), |(_, text)| text.to_owned());
    outcome(text.into_bytes(), Vec::new(), 0)
}

/// The debug record Go writes to stdout before a command error under the
/// logging variables (`logDebugError`, `internal/cli/logging.go:26-51`).
pub(super) fn debug_error_record(context: &Context<'_>, name: &str, message: &str) -> Vec<u8> {
    logging::debug_error_record(context, name, message)
}

/// The expectations of the dispatcher tests in `command/mod.rs`, whose
/// inputs stay there: Go's exact result for each input, as literals. An
/// input with no expectation panics.
#[cfg(test)]
pub(super) mod checks {
    use std::collections::BTreeMap;

    use super::super::{run, Invocation};
    use super::logging::hostname;
    use crate::error::{Error, PLAN_2};

    /// Go's root help, the stdout of fixture 007.
    pub(super) const ROOT_HELP: &str = concat!(
            "Luuna Missions v0.6.0 — Git-native issue tracking\n",
            "\n",
            "Usage: luuna-missions <command> [options]\n",
            "\n",
            "Commands:\n",
            "  init              Initialize .luuna/missions/ in current directory\n",
            "  create            Create a new issue\n",
            "  show <id> [ids]   Show one or more issues\n",
            "  list              List issues with filters\n",
            "  ready             Show open issues with no unresolved blockers\n",
            "  search <query>    Full-text search title + description\n",
            "  update <id>       Update issue fields\n",
            "  close <id> [ids]  Close one or more issues\n",
            "  dep               Manage issue dependencies\n",
            "  label             Manage issue labels\n",
            "  blocked           Show all blocked issues\n",
            "  stats             Project statistics\n",
            "  sync              Stage and commit .luuna/missions/ changes\n",
            "  doctor            Check project health and data integrity\n",
            "  tpl               Manage issue templates (molecules)\n",
            "  migrate-from-beads  Migrate issues from beads issue tracker\n",
            "  prime             Output AI agent context\n",
            "  onboard           Add missions section to CLAUDE.md / AGENTS.md\n",
            "  upgrade           Update Luuna Missions through a reviewed checkout\n",
            "  completions <shell>  Output shell completion script\n",
            "  block <id>        Add a blocker to an issue\n",
            "  unblock <id>      Remove blockers from an issue\n",
            "  plan              Plan management\n",
            "  config            Read, write, and inspect .luuna/missions/config.yaml\n",
            "  setup <target>    Install Claude Code hooks: prime on session start, guard .luuna/missions/*.jsonl edits\n",
            "  guard             PreToolUse hook: deny hand edits to .luuna/missions/*.jsonl (reads hook JSON on stdin)\n",
            "\n",
            "Options:\n",
            "  -h, --help        Show help\n",
            "  -v, --version     Print version\n",
            "  --format <mode>   Output format (markdown|compact|plain|ids|json)\n",
            "  --json            Alias for --format json\n",
            "  -q, --quiet       Suppress non-error output\n",
            "  --verbose         Extra diagnostic output\n",
            "  --timing          Show command execution time\n",
            "\n",
            "Run 'luuna-missions <command> --help' for command-specific help.\n",
        );

    const INIT_HELP: &str = concat!(
        "Usage: luuna-missions init [options]\n",
        "\n",
        "Initialize .luuna/missions/ in current directory\n",
        "\n",
        "Options:\n",
        "  --json      Output as JSON\n",
        "  -h, --help  display help for command\n",
    );

    const LIST_HELP: &str = concat!(
        "Usage: luuna-missions list [options]\n",
        "\n",
        "List issues with filters\n",
        "\n",
        "Options:\n",
        "  --status <status>     Filter by status (open|in_progress|closed)\n",
        "  --type <type>         Filter by type (task|bug|feature|epic)\n",
        "  --assignee <name>     Filter by assignee\n",
        "  --all                 Include closed issues (default: only open/in_progress)\n",
        "  --label <labels>      Filter: must have ALL labels (comma-separated, AND)\n",
        "  --label-any <labels>  Filter: must have any label (comma-separated, OR)\n",
        "  --unlabeled           Filter: issues with no labels\n",
        "  --priority <levels>   Filter by priority (comma-separated, e.g. 0,1 or P0,P1)\n",
        "  --priority-max <n>    Filter to priority <= n (e.g. --priority-max 1 = P0+P1)\n",
        "  --limit <n>           Max issues to show (default: \"50\")\n",
        "  --sort <mode>         Sort order (priority|created|updated|id) (default:\n",
        "                        \"priority\")\n",
        "  --format <mode>       Output format (markdown|compact|plain|ids|json)\n",
        "  --json                Output as JSON (alias for --format json)\n",
        "  -h, --help            display help for command\n",
    );

    pub(super) const VERSION_JSON_PREFIX: &str = concat!(
        "{\n",
        "  \"success\": true,\n",
        "  \"command\": \"version\",\n",
        "  \"name\": \"@luuna/missions-cli\",\n",
        "  \"version\": \"0.6.0\",\n",
        "  \"runtime\": \"go\",\n",
        "  \"platform\": \""
    );

    /// A folder with no store to run in.
    fn folder() -> tempfile::TempDir {
        tempfile::tempdir().expect("a temporary folder")
    }

    fn plan_of(error: Error) -> String {
        match error {
            Error::Unsupported { plan, .. } => plan,
            other => panic!("expected Unsupported, got {other}"),
        }
    }

    /// `run(args)` in a folder without a store: Go's stdout, stderr and exit
    /// code for that input.
    pub(in crate::command) fn answer(args: &[&str]) {
        let platform = super::platform();
        let version_json = format!("{VERSION_JSON_PREFIX}{platform}\"\n}}\n");
        let (stdout, stderr, exit_code): (&str, &str, i32) = match args {
            [] | ["-q"] => ("", ROOT_HELP, 1),
            ["--version"] | ["-v"] | ["init", "-v"] => ("0.6.0\n", "", 0),
            ["list", "--json", "--version"] | ["nope", "--json", "-v"] => {
                (version_json.as_str(), "", 0)
            }
            ["--help"] | ["-h"] | ["--json", "--help"] => (ROOT_HELP, "", 0),
            ["init", "--help"] => (INIT_HELP, "", 0),
            ["list", "-h"] => (LIST_HELP, "", 0),
            ["nope"] => ("", "Unknown command: nope\n", 1),
            ["inti", "--json"] => (
                concat!(
                    "{\n",
                    "  \"success\": false,\n",
                    "  \"command\": \"inti\",\n",
                    "  \"error\": \"Unknown command: inti. Did you mean init?\",\n",
                    "  \"suggestion\": \"init\"\n",
                    "}\n"
                ),
                "",
                1,
            ),
            ["--"] => ("", "Unknown command: --\n", 1),
            other => panic!("no expectation for {other:?}"),
        };
        let dir = folder();
        let outcome = run(&Invocation::new(dir.path(), args.iter().copied()))
            .unwrap_or_else(|error| panic!("{args:?}: {error}"));
        assert_eq!(
            String::from_utf8_lossy(&outcome.stdout),
            stdout,
            "{args:?}: stdout"
        );
        assert_eq!(
            String::from_utf8_lossy(&outcome.stderr),
            stderr,
            "{args:?}: stderr"
        );
        assert_eq!(outcome.exit_code, exit_code, "{args:?}: exit code");
    }

    /// A command that fails in a folder without a store, with only
    /// `key=value` in the environment: on the targets whose host name is read
    /// as Go reads it, Go's debug record and then Go's error envelope; on
    /// the others, Unsupported naming s4-missions-lib-2.
    pub(in crate::command) fn debug_environment(key: &str, value: &str) {
        match (key, value) {
            ("LUUNA_MISSIONS_DEBUG", "1")
            | ("LUUNA_MISSIONS_LOG_LEVEL", "debug")
            | ("LUUNA_MISSIONS_LOG_LEVEL", "trace") => {}
            other => panic!("no expectation for {other:?}"),
        }
        let env = BTreeMap::from([(key.to_owned(), value.to_owned())]);
        let dir = folder();
        let result = run(&Invocation::new(dir.path(), ["show", "missing", "--json"]).with_env(env));
        if !super::logging::HOSTNAME_AS_GO {
            assert_eq!(plan_of(result.unwrap_err()), PLAN_2, "{key}={value}");
            return;
        }
        let outcome = result.unwrap_or_else(|error| panic!("{key}={value}: {error}"));
        assert_eq!(outcome.exit_code, 1, "{key}={value}");
        assert!(outcome.stderr.is_empty(), "{key}={value}");
        let stdout = String::from_utf8(outcome.stdout).expect("UTF-8 output");
        let (line, rest) = stdout.split_once('\n').expect("a record line");
        let record: serde_json::Value = serde_json::from_str(line).expect("a JSON record");
        let positions: Vec<usize> = [
            "\"level\":",
            "\"time\":",
            "\"pid\":",
            "\"hostname\":",
            "\"err\":",
            "\"cmd\":",
            "\"msg\":",
        ]
        .iter()
        .map(|key| line.find(key).expect("every fixed key"))
        .collect();
        assert!(positions.windows(2).all(|pair| pair[0] < pair[1]), "{line}");
        assert!(line.starts_with("{\"level\":20,\"time\":"), "{line}");
        assert_eq!(record.as_object().expect("an object").len(), 7);
        assert_eq!(record["level"], 20, "{key}={value}");
        assert!(record["time"].is_i64(), "{key}={value}");
        assert!(record["pid"].is_i64(), "{key}={value}");
        assert_eq!(record["hostname"], hostname().as_str(), "{key}={value}");
        assert_eq!(
            record["err"],
            serde_json::json!({
                "type": "Error",
                "message": "Not in a missions project. Run `luuna-missions init` first.",
                "stack": "Error: Not in a missions project. Run `luuna-missions init` first.",
            }),
            "{key}={value}"
        );
        assert_eq!(record["cmd"], "show", "{key}={value}");
        assert_eq!(record["msg"], "command failed", "{key}={value}");
        assert_eq!(
            rest,
            concat!(
                "{\n",
                "  \"success\": false,\n",
                "  \"command\": \"show\",\n",
                "  \"error\": \"Not in a missions project. Run `luuna-missions init` first.\"\n",
                "}\n"
            ),
            "{key}={value}"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    fn stdout_of(result: Result<Outcome>) -> String {
        let outcome = result.expect("an outcome");
        assert!(outcome.stderr.is_empty());
        assert_eq!(outcome.exit_code, 0);
        String::from_utf8(outcome.stdout).expect("UTF-8")
    }

    #[test]
    fn root_help_matches_go_byte_for_byte() {
        assert_eq!(help::root(), checks::ROOT_HELP);
        assert_eq!(stdout_of(help()), checks::ROOT_HELP);
        let without = help_without_command().expect("an outcome");
        assert!(without.stdout.is_empty());
        assert_eq!(without.stderr, checks::ROOT_HELP.as_bytes());
        assert_eq!(without.exit_code, 1);
    }

    #[test]
    fn the_help_table_lists_every_command_in_table_order() {
        let ours: Vec<&str> = help::COMMANDS.iter().map(|(name, _, _)| *name).collect();
        let registered: Vec<&str> = super::super::COMMANDS
            .iter()
            .map(|(name, _)| *name)
            .collect();
        assert_eq!(ours.len(), 26);
        assert_eq!(ours, registered);
    }

    #[test]
    fn help_snapshots_are_chosen_as_go_chooses_them() {
        let chosen = |values: &[&str]| help::snapshot(&args(values)).expect("a snapshot").0;
        assert_eq!(chosen(&["dep", "add", "--help"]), "dep__add");
        assert_eq!(
            chosen(&["tpl", "step", "add", "missing", "--help"]),
            "tpl__step__add"
        );
        assert_eq!(chosen(&["setup", "claude", "--help"]), "setup");
        assert_eq!(chosen(&["dep", "nope", "--help"]), "dep");
        assert_eq!(chosen(&["label", "list-all", "-h"]), "label__list-all");
        assert_eq!(chosen(&["init", "--help"]), "init");
        assert_eq!(chosen(&["plan", "show", "x", "--help"]), "plan__show");
        assert_eq!(chosen(&["config", "set", "k", "v", "-h"]), "config__set");
        assert!(help::snapshot(&args(&["--help"])).is_none());
        assert!(help::snapshot(&args(&["nope", "--help"])).is_none());
        let outcome = command_help(&args(&["dep", "add", "x", "--help"])).expect("an outcome");
        let expected = help::SNAPSHOTS
            .iter()
            .find(|(name, _)| *name == "dep__add")
            .expect("dep__add")
            .1;
        assert_eq!(outcome.stdout, expected.as_bytes());
        // Go's fallback when no snapshot exists.
        assert_eq!(
            help::fallback("show"),
            "Usage: luuna-missions show <id> [ids]\n\nShow one or more issues\n"
        );
    }

    #[test]
    fn every_help_asset_is_embedded_and_reachable() {
        assert_eq!(help::SNAPSHOTS.len(), 57);
        let names: Vec<&str> = help::SNAPSHOTS.iter().map(|(name, _)| *name).collect();
        let mut sorted = names.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted, names, "sorted and unique");
        for (name, text) in help::SNAPSHOTS {
            assert!(!text.is_empty(), "{name}");
            let parts: Vec<String> = name.split("__").map(str::to_owned).collect();
            assert!(parts.len() <= 3, "{name}");
            assert_eq!(help::snapshot(&parts), Some((*name, *text)), "{name}");
        }
    }

    #[test]
    fn version_output_follows_go() {
        assert_eq!(VERSION, "0.6.0");
        assert_eq!(stdout_of(version()), "0.6.0\n");
        let json = stdout_of(version_json());
        assert!(json.starts_with(checks::VERSION_JSON_PREFIX), "{json}");
        let platform = platform();
        assert_eq!(
            json.strip_prefix(checks::VERSION_JSON_PREFIX).unwrap(),
            format!("{platform}\"\n}}\n")
        );
        if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
            assert_eq!(platform, "linux-x64");
        }
        if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
            assert_eq!(platform, "linux-arm64");
        }
    }

    #[test]
    fn unknown_commands_get_go_s_suggestion() {
        assert_eq!(
            suggest("creat"),
            (
                "create",
                "Unknown command: creat. Did you mean create?".to_owned()
            )
        );
        assert_eq!(suggest("bloc").0, "block");
        assert_eq!(suggest("dap").0, "dep");
        // A tie keeps the first in table order: "blocke" is one edit from
        // blocked (10th) and from block (21st); "lab" is two edits from
        // label and from plan.
        assert_eq!(suggest("blocke").0, "blocked");
        assert_eq!(suggest("lab").0, "label");
        assert_eq!(levenshtein("kitten", "sitting"), 3);
        // Exactly two edits is still suggested; three is not.
        assert_eq!(suggest("inxx").0, "init");
        assert_eq!(
            suggest("xxxxxx"),
            ("", "Unknown command: xxxxxx".to_owned())
        );
        assert_eq!(suggest(""), ("", "Unknown command: ".to_owned()));
        // Multi-byte names count runes: one substitution, not two bytes.
        assert_eq!(levenshtein("é", "e"), 1);
        assert_eq!(levenshtein("日本", "日"), 1);
        assert_eq!(suggest("lisé").0, "list");
        // Json mode: the envelope, without a suggestion field when empty.
        let nothing = unknown_command("zzzzzz", true).expect("an outcome");
        assert_eq!(
            String::from_utf8(nothing.stdout).unwrap(),
            "{\n  \"success\": false,\n  \"command\": \"zzzzzz\",\n  \"error\": \"Unknown command: zzzzzz\"\n}\n"
        );
        assert_eq!(nothing.exit_code, 1);
        let plain = unknown_command("creat", false).expect("an outcome");
        assert!(plain.stdout.is_empty());
        assert_eq!(
            plain.stderr,
            b"Unknown command: creat. Did you mean create?\n"
        );
    }
}
