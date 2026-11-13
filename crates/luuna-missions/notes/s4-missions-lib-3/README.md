# Notes of s4-missions-lib-3

These notes belong to s4-missions-lib-3. It edits them, and only it does. Paths are relative to `crates/luuna-missions/`.

## Moved from the README

- **s4-missions-lib-3:** config, tpl, doctor, sync, migrate-from-beads, the legacy `.seeds` layout, prime, onboard, setup claude, guard and upgrade.
- **`config.yaml`** now uses the guarded event reader, full tree emitter, validator and plan-template loader, including comments, nested maps, aliases and first-document semantics.

## Files this plan owns

- `src/command/project/mod.rs`
- `src/config_tree.rs`
- `tests/later_plans/s4_missions_lib_3.rs`
- `tests/support/fixtures/s4_missions_lib_3.rs`

Check functions it owns and replaces (each is called by a unit test whose inputs and name stay as they are):

- `command::project::checks::command`, `pub(in crate::command) fn command(name: &str)`. Caller: `command::tests::every_other_go_command_answers_as_its_plan_states (10 names)`.
  - Until ported: Matches on the module's constant for `name`. Deferred arm: `run(<name> --json)` in `std::env::temp_dir()` is Unsupported naming PLAN_3 with Display `the <name> command is not supported yet; it is ported in s4-missions-lib-3`. Ported arm: `unreachable!`.
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

## Assumptions and library boundaries

- **A301**: The reader decodes only the first YAML document and stops at its DocumentEnd; the alias guard counts only that document.
- **A302**: Notices: with_stderr_notices captures notices raised while a command runs and puts them before the outcome's stderr; on an Err result, and when no scope is open (Store::discover), they go to the process's stderr.
- **A303**: YAML error texts (U7): the two alias errors are reproduced exactly as Error::Command; every other input yaml.v3 rejects, or that the port cannot show it reads as yaml.v3 does, returns Error::Unsupported whose plan is YAML_ERROR_TEXTS_NOT_PORTED = "none (yaml.v3's error texts are not ported; see notes/s4-missions-lib-3/README.md)".
- **A304**: LoadPlanTemplates checks templates and sections in byte-sorted name order.
- **A305**: The library writes what Go writes outside the store: setup (.claude/settings.json), onboard (CLAUDE.md, .claude/CLAUDE.md or AGENTS.md), doctor --fix (.gitattributes), the legacy migration (moves .seeds, rewrites .gitattributes and .gitignore); it reads .beads/issues.jsonl and go.mod. The caller decides whether and when to call these commands.
- **A306**: doctor --fix uses a copy of the store's stale-lock reclaim in command/project/stale_lock.rs.
- **A307**: prime discards state-loading errors as Go does, except Error::Unsupported, which it returns.
- **A308**: onboard uses its own CLI_VERSION = "0.6.0"; the equality test with -2's constant belongs to s4-missions-lib-4.
- **A309**: guard takes stdin as bytes; empty stdin prints nothing and exits 0, which Go prints for a terminal.
- **A310**: sync runs git found through the invocation's PATH with exactly the invocation's environment and commits the whole index as Go does; upgrade only prints Go's refusal.
- **A312**: YAML nested deeper than 1000 levels, aliases expanded, is refused as Error::Unsupported (YAML_ERROR_TEXTS_NOT_PORTED) before any value is built; events are pulled one at a time.
- **A313**: DL306 is decided by re-reading the emitted text through decode_document and comparing with the tree written; numbers compare by value (float 2 written as 2 and read back as int 2 is the same value), NaN never equals.
- **A314**: Anchors, aliases and merge keys are written expanded by config set and unset, as Go writes them, and listed with comments as a known loss.
- **A315**: doctor --fix computes the whole fix run in memory in Go's order and refuses the whole run, writing nothing, if any rewrite would lose one of DL303's parts (i) to (v), each observed (logs/go-first-1.txt:86-95).
- **A316**: The repeated-key check of GDL2 looks at the levels Go decodes into maps (the record's top level and fields internal/model/json.go decodes field by field), not inside values Go keeps as raw JSON.
- **A317**: A number is lost through float64 when the value its JSON text denotes differs from the float64 Go parses it to (compared as exact decimals); a number such as 1.0 that Go writes as 1 keeps its value and is copied. This test is applied where a Go reader keeps the number's text (store files: doctor --fix, DL303 (iii)); in .claude/settings.json Go's only reader parses to float64, so the number is copied (A320).
- **A318**: Check-function literals and the go-first evidence come from the Go binary at 2fbbf8e, observed by a tester before this revision (GF1 to GF3 and the tpl step add runs, logs/go-first-1.txt and its four dumps), recorded in the ledger.
- **A319**: Refusal texts of DL300 to DL308 follow the slots refusal_shape: a command error naming the file, the line or path and the reason, ending with 'Nothing was written'; setup's follows Go's own setup errors ('; fix it by hand, nothing was written', setup.go:163).
- **A320**: What 'Go reads the same value' means (decision 6), applied the same way to every case: every reader the Go tool has for the file returns the same value before and after the rewrite. For issues.jsonl and templates.jsonl these are the CLI's store readers (show, list, tpl show, which print values Go keeps raw) and the decode into any that doctor --fix uses; for .claude/settings.json the only Go reader is setup's own decode into any. Where two readers disagree, the one that sees a difference governs. Result: V3, V7, V8, V9, V10 (decode equal, show different) are refused (DL303 (iii), (v)); W1, W4 to W8 (one reader, equal) are copied.
- **A321**: tpl step add treats a `steps` member that is null as Go's empty list and appends without a refusal; only a non-null, non-array steps or a non-object element is refused (DL308).
- **A322**: A repeated key is refused wherever a rewrite would drop its earlier member (DL300 (c), DL303 (iv), DL307), whatever Go reads back.

All ten project commands are native library entries. Setup writes project-local `.claude/settings.json`; onboard chooses the first existing supported instructions file; guard is a pure stdin/cwd decision; upgrade always refuses without network access; sync alone explicitly runs Git and preserves Go's whole-index commit behavior. Doctor computes and checks its complete fix plan before side effects. Legacy discovery can move `.seeds` and update project ignore files; callers decide when these operations are authorized. No command merges a developer repository.

YAML alias failures retain Go's exact messages. Other parser/type/error-text boundaries explicitly name `none (yaml.v3's error texts are not ported; see notes/s4-missions-lib-3/README.md)`. Expanded nesting beyond 1000 and yaml-rust2's own scanner bounds are refused. SD10's internal empty tpl subcommand seam refuses rather than panicking; no new externally reachable panic case is claimed.

The original planned 96 new unit tests are present. Two newly observed A8 losses add two more, bringing this plan to 98 new tests and the integrated library to 288. DL309 and DL310 extend the original nine entries because byte comparisons demonstrated additional loss; the signed plan and frozen fixtures were retained. The Unsupported propagation test exercises the actual error-filter helper, since the now-ported plan index no longer manufactures a deferred error.

Writing tests use disposable projects with their own store. The streamed 10 MiB beads-line test optionally uses `LUUNA_MISSIONS_TEST_DISK_DIR` as its tempfile parent so constrained hosts can keep the fixture on disk; ordinary tests use the system temp folder. No writing test discovers a checkout from its process working directory. Verification records compare `<workspace>/.claude/settings.json` hashes and worktree status before and after capped runs.

The copied W6/W7 settings normalization has a known interoperability limit: a JavaScript reader can distinguish a lone surrogate from the U+FFFD that Go writes. The signed A320 decision copies Go's sole settings reader; this is not a claim that all non-Go readers see identical values. Native Windows/macOS behavior remains subject to their platform CI; this host validates Linux.
