# Notes of s4-missions-lib-2

These notes belong to s4-missions-lib-2. It edits them, and only it does. Paths are relative to `crates/luuna-missions/`.

## Moved from the README

- **s4-missions-lib-2:** close, ready, blocked, stats, dep, label, unblock, help, version and completions.
- **Refused until a later plan:** help and version output (`--help`, `-h`, `--version`, `-v`, or no command at all), unknown commands, and an environment with `LUUNA_MISSIONS_DEBUG=1` or `LUUNA_MISSIONS_LOG_LEVEL` set to `debug` or `trace`. Go writes debug records to stdout in that last case (SD8). All of these return `Error::Unsupported`.

## Files this plan owns

- `src/command/app.rs`
- `src/command/workflow/mod.rs`
- `src/command/plan_index.rs`
- `tests/later_plans/s4_missions_lib_2.rs`
- `tests/support/fixtures/s4_missions_lib_2.rs`

Check functions it owns and replaces (each is called by a unit test whose inputs and name stay as they are):

- `command::app::checks::answer`, `pub(in crate::command) fn answer(args: &[&str])`. Caller: `command::tests::help_version_and_unknown_commands_answer_as_app_states (15 argument lists, in mod.rs)`.
  - Until ported: At the slots commit, `run(Invocation::new(std::env::temp_dir(), args))` is `Err(Error::Unsupported)` naming PLAN_2, as at the foundation, and its Display text is `<feature> is not supported yet; it is ported in s4-missions-lib-2` with <feature>: [] and [-q]: `help output (no command was given)`; [--version], [-v], [init -v]: `--version (version output)`; [list --json --version], [nope --json -v]: `--version --json (version output)`; [--help], [-h], [--json --help]: `--help (help output)`; [init --help]: `init --help (command help output)`; [list -h]: `list --help (command help output)`; [nope], [inti --json], [--]: `the unknown command "<name>" (Go's unknown-command message and suggestion)`.
  - Once ported: `Ok(Outcome)` with Go's exact stdout, stderr and exit code for that input, as literals, run in a `tempfile::tempdir()` without a store.
- `command::app::checks::debug_environment`, `pub(in crate::command) fn debug_environment(key: &str, value: &str)`. Caller: `command::tests::debug_logging_environments_answer_as_app_states (three pairs, in mod.rs)`.
  - Until ported: `run(init --json` in `std::env::temp_dir()` with env {key: value}`)` is Unsupported naming PLAN_2, as at the foundation.
  - Once ported: On unix, a command that fails in a `tempfile::tempdir()` without a store returns `Ok` with one debug record line, whose fixed keys and values are checked literally, followed by Go's envelope; on non-unix, still Unsupported naming PLAN_2.
- `command::workflow::checks::command`, `pub(in crate::command) fn command(name: &str)`. Caller: `command::tests::every_other_go_command_answers_as_its_plan_states (8 names)`.
  - Until ported: Matches on the module's constant for `name`. Deferred arm: `run(<name> --json` in `std::env::temp_dir()`) is Unsupported naming PLAN_2 with Display `the <name> command is not supported yet; it is ported in s4-missions-lib-2`. Ported arm: `unreachable!`.
  - Once ported: The Deferred arm is kept unchanged; the Ported arm asserts Go's exact outcome of `<name> --json` in a `tempfile::tempdir()` without a store, per name, as literals.
- `command::plan_index::checks::plans_file`, `pub(in crate::command) fn plans_file()`. Caller: `command::tests::the_plan_index_answers_as_its_owner_states`.
  - Until ported: In a tempdir store: a missing `plans.jsonl` and one holding only whitespace give an index whose `status_for_ticket` and `status_for_seed` are `None`. Kept forever. A literal `plans.jsonl` with two plan records gives Unsupported naming PLAN_2.
  - Once ported: The first part unchanged; for the same literal records, the statuses Go's `loadPlanIndex` gives (`internal/cli/issue_read.go:87-114`), as literals.

A ported branch never asserts only `is_ok()`, and an input with no expectation panics.
