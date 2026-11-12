# Notes for s5-atlas-lib-2: record, edit, delete, search, show, renderers and the building blocks

Copied from the plan s5-atlas-lib-slots (plan.md) so that this plan starts from them. Paths below are under `crates/luuna-atlas/`.

## Ownership

- Owns: `src/actions/**` (record, edit, delete, search, show, the API typed operations, and the human renderers in `src/actions/render.rs`); the building blocks `src/options.rs`, `src/git.rs`, `src/hooks.rs`, `src/schema.rs`, `src/records.rs`, `src/timefmt.rs`, `src/evidence.rs`, `src/style.rs`, `src/assets.rs`, `src/confirm.rs`; `tests/parity_runtime_actions.rs`, `tests/fixtures/s5-atlas-lib-2/` and this file.
- Foundation files it may still edit: none. `json_parse_error`, `write_file_atomic` and `rename_replacing` are `pub(crate)` after the slots plan.
- Needs on main: the foundation and the slots plan only.

## Ownership notes

- `json_parse_error`, `write_file_atomic` and `rename_replacing` in `src/store.rs` are `pub(crate)` after the slots plan; `store.rs` stays frozen.
- `libc` is declared (unix only); `regex` is not. Hand-write every Go regexp.
- `-2` reads config through `config::read_config` and so keeps the indented first key refusal (GDL1).
- The slots plan's list of statements in the -2 plan that it supersedes is in its plan.md, section 'Corrections s5-atlas-lib-2 needs when its base is set'.

## Assumptions that name this plan

- AS13: human output and colour are in scope; `style.rs` is ported faithfully (it ignores `NO_COLOR` as Go does) together with the renderers in `src/actions/render.rs`.
- AS17: hooks (`sh -c`, setsid, group SIGKILL, diagnostics) are ported faithfully in `hooks.rs`, using `libc`. Tests never reach the network.

## Go data loss (A8)

None of this plan's own cases is known. This plan inherits GDL1 through `config::read_config` and must not add a second YAML reader that bypasses it.

GDL1 (observed): a `config.yaml` whose first key is indented and a later key is less indented (the indented first key case, for example `  b: 1` then `a: 2`). Go's yaml.v3 reads the later key as a second document, so `config show` prints the config without `a` and `add` rewrites the file without `a: 2`. The port refuses with `unsupported YAML at line 2: content after the end of the document is not supported` (`yaml.rs:399-404`), exits 1 and leaves the file untouched.

Comments on rewrite (decided for the owner, AS19 confirmed; orchestration ledger, needs-owner-wave-2.md, decided item 3): loss of comments alone on a rewrite of `config.yaml` is copied from Go and is listed in the hand-off as a known loss. Loss of keys, values, documents, lines, records or body text is refused with the file untouched.

Every other Go behaviour is still copied (A4 differences), suspected defects included. Only the cases named here are deliberate differences from Go.

## Carried from the foundation reviews

- CF1: `json::Value` derives `Debug` and `PartialEq` (`json.rs:46`), both recursive per nesting level, and `Integer(100) != Number(100.0)`. No `{:?}` and no `==` on Values from user data in non-test code; compare numbers as Go does. In particular a test must not assume `Integer(100)` equals `Number(100.0)`.
- CF2: config and file writes follow symlinks as Go's `os.WriteFile` does (`config.rs:279`, `commands.rs:219`). Copy it, do not fix it, list it in the hand-off as a suspected Go defect.
- CF5: Go's JSONPretty prints an empty line with exit 0 past 10000 levels; the crate copies it (`output.rs:198-199`). Keep copying and list it in the hand-off as a suspected Go defect.
- CF6: Go's `Number()` returns 0 for a uint64, so a port of Number must not reuse `as_f64` unchanged. Applies to `records::number` and `number_default`.
