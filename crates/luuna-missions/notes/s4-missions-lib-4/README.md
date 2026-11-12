# Notes of s4-missions-lib-4

These notes belong to s4-missions-lib-4. It edits them, and only it does. Paths are relative to `crates/luuna-missions/`.

## Moved from the README

- **s4-missions-lib-4:** plans.
- **Plans.** A status change (`update --status` or `--claim`) on a ticket that has a `plan_id`, that is the seed of a plan or that is one of its children returns `Error::Unsupported` naming s4-missions-lib-4, before anything is written. Go would run its plan transition hook there. Other edits of such a ticket, and a status set to the value it already has, are applied. Reading is refused the same way where the output would print plan context: `show`, `list` and `search` of a ticket whose `plan_id` names a plan in `plans.jsonl`, in the markdown, plain and json formats, and `show pl-<id>` for an id that is not a ticket. The `ids` and `compact` formats print no plan context and are ported for such tickets.

## Files this plan owns

- `src/command/plans/mod.rs`
- `tests/later_plans/s4_missions_lib_4.rs`
- `tests/support/fixtures/s4_missions_lib_4.rs`

Check functions it owns and replaces (each is called by a unit test whose inputs and name stay as they are):

- `command::plans::checks::command`, `pub(in crate::command) fn command(name: &str)`. Caller: `command::tests::every_other_go_command_answers_as_its_plan_states (plan)`.
  - Until ported: Matches on the module's constant for `name`. Deferred arm: `run(<name> --json` in `std::env::temp_dir()`) is Unsupported naming PLAN_4 with Display `the <name> command is not supported yet; it is ported in s4-missions-lib-4`. Ported arm: `unreachable!`.
  - Once ported: The Deferred arm is kept unchanged; the Ported arm asserts Go's exact outcome of `<name> --json` in a `tempfile::tempdir()` without a store, per name, as literals.
- `command::plans::checks::status_change_seams`, `pub(in crate::command) fn status_change_seams()`. Caller: `command::tests::the_status_change_seams_answer_as_their_owner_states`.
  - Until ported: `check_status_change` is `Ok` for tickets without `plan_id` and an empty `plans.jsonl`, and Unsupported naming PLAN_4 for a ticket with `plan_id` and for a non-empty `plans.jsonl`; `apply_status_change` is `Ok` and changes no file in the store.
  - Once ported: For the same literal inputs, Go's `planTransitionHook` result (`internal/cli/issue_mutate.go:14-20`, `:62-67`): `Ok` or Go's error text, and the literal `plans.jsonl` after.

A ported branch never asserts only `is_ok()`, and an input with no expectation panics.
