# Notes of s4-missions-lib-3

These notes belong to s4-missions-lib-3. It edits them, and only it does. Paths are relative to `crates/luuna-missions/`.

## Moved from the README

- **s4-missions-lib-3:** config, tpl, doctor, sync, migrate-from-beads, the legacy `.seeds` layout, prime, onboard, setup claude, guard and upgrade.
- **`config.yaml`** is read only in the forms Go writes for top-level values. Nested values, comments and other YAML return `Error::Unsupported` until s4-missions-lib-3.

## Files this plan owns

- `src/command/project/mod.rs`
- `src/config_tree.rs`
- `tests/later_plans/s4_missions_lib_3.rs`
- `tests/support/fixtures/s4_missions_lib_3.rs`

Check functions it owns and replaces (each is called by a unit test whose inputs and name stay as they are):

- `command::project::checks::command`, `pub(in crate::command) fn command(name: &str)`. Caller: `command::tests::every_other_go_command_answers_as_its_plan_states (10 names)`.
  - Until ported: Matches on the module's constant for `name`. Deferred arm: `run(<name> --json` in `std::env::temp_dir()`) is Unsupported naming PLAN_3 with Display `the <name> command is not supported yet; it is ported in s4-missions-lib-3`. Ported arm: `unreachable!`.
  - Once ported: The Deferred arm is kept unchanged; the Ported arm asserts Go's exact outcome of `<name> --json` in a `tempfile::tempdir()` without a store, per name, as literals.
- `config_tree::checks::outside_subset`, `pub(crate) fn outside_subset(missions_dir: &Path)`. Caller: `config::tests::reading_outside_the_subset_answers_as_config_tree_states (writes `project:\n  nested: x\n` first)`.
  - Until ported: `read_config(missions_dir)` is Unsupported naming PLAN_3, as at the foundation.
  - Once ported: `Ok(Config)` equal to what Go's `ReadConfig` returns for that file (project "missions", version "1", max_plan_depth None), as literals.
- `config_tree::checks::legacy_found`, `pub(crate) fn legacy_found(root: &Path, start: &Path)`. Caller: `config::tests::a_legacy_seeds_entry_answers_as_config_tree_states (four cases)`.
  - Until ported: `find_dir(start, ProcessEnv::Inherit)` is Unsupported naming PLAN_3 with a feature containing `.seeds`, as at the foundation, and `root/.seeds` still exists with the same file type.
  - Once ported: Go's `MigrateLegacyLayout` result for that case (`legacy.go:34-95`): the move and its stderr line through `with_stderr_notices`, or Go's exact error text as `Error::Command`, as literals.
- `config_tree::checks::init_with_legacy_folder`, `pub(crate) fn init_with_legacy_folder(root: &Path)`. Caller: `command::init::tests::init_refuses_other_store_dirs_and_answers_legacy_folders_via_config_tree (root holds a .seeds directory)`.
  - Until ported: `run(Invocation::new(root, ["init"]))` is Unsupported naming PLAN_3, and `root/.luuna` does not exist, as at the foundation.
  - Once ported: Go's result of `init` in that root: `Ok` with the literal stdout, stderr (including `Migrated legacy .seeds/ to .luuna/missions/ in <root>`) and exit code, and the resulting files.

A ported branch never asserts only `is_ok()`, and an input with no expectation panics.
