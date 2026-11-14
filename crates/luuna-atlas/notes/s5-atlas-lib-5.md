# Notes for s5-atlas-lib-5: doctor, audit, setup, onboard and the CLI front end

Copied from the plan s5-atlas-lib-slots (plan.md) so that this plan starts from them. Paths below are under `crates/luuna-atlas/`.

## Ownership

- Owns: `src/tooling/**` (doctor, audit, setup, onboard, completions, parser, dispatcher, help, version, the upgrade/update stubs, and the bridge stdin wrapper); `tests/parity_tooling.rs`, `tests/fixtures/s5-atlas-lib-5/` and this file.
- Foundation files it may still edit: none expected.
- Needs on main: s5-atlas-lib-2, -3 and -4 (the dispatcher routes to all of them).

## Ownership notes

- The front end must call `git::resolve_store_root` before any `run_*` and pass the user's cwd separately.
- Build `options::Env::from_process`, and run the parser before calling `run_*`.
- Reproduce Go's parser rejection of `search --format ids`.
- `help.go:15` reuses -2's `style::stdout_is_terminal`, so this plan needs no `libc`.

## Assumptions that name this plan

- AS12: the whole CLI surface is in scope; this plan owns the parser, dispatcher, help, completions, setup, onboard, stubs and wrapper.
- AS13: port the human paths faithfully on top of -2's style and renderers.
- AS14: copy Go's version output, including name "@luuna/atlas-cli" and runtime "bun", unless the runtime expects other values; then provide those and list the difference.
- AS16: read help data through -3's embedded `contract.json` module.

## Go data loss (A8)

None of this plan's own cases is known. doctor and setup keep the indented first key refusal through `config::read_config` (GDL1).

GDL1 (observed): a `config.yaml` whose first key is indented and a later key is less indented (the indented first key case, for example `  b: 1` then `a: 2`). Go's yaml.v3 reads the later key as a second document, so `config show` prints the config without `a` and `add` rewrites the file without `a: 2`. The port refuses with `unsupported YAML at line 2: content after the end of the document is not supported` (`yaml.rs:399-404`), exits 1 and leaves the file untouched.

Comments on rewrite (decided for the owner, AS19 confirmed; orchestration ledger, needs-owner-wave-2.md, decided item 3): loss of comments alone on a rewrite of `config.yaml` is copied from Go and is listed in the hand-off as a known loss. Loss of keys, values, documents, lines, records or body text is refused with the file untouched.

Every other Go behaviour is still copied (A4 differences), suspected defects included. Only the cases named here are deliberate differences from Go.

## Carried from the foundation reviews

- CF1: `json::Value` derives `Debug` and `PartialEq` (`json.rs:46`), both recursive per nesting level, and `Integer(100) != Number(100.0)`. No `{:?}` and no `==` on Values from user data in non-test code; compare numbers as Go does.
- CF5: Go's JSONPretty prints an empty line with exit 0 past 10000 levels; the crate copies it (`output.rs:198-199`). Keep copying and list it in the hand-off as a suspected Go defect.

## Implementation decisions and demonstrated A8 cases

The front end reads the exact embedded contract and preserves root/global/local option precedence, help/version short circuits, custom registry choices and flag descriptions. Compatibility version metadata retains `@luuna/atlas-cli` and `runtime: bun`; the native core itself has no Bun dependency. User-selected JavaScript recipe extensions still use their specified extension runtime.

Fresh pinned Go probes show that legacy onboarding without its terminal command/fence can erase following unrelated instructions; modified Cursor rule installation overwrites the whole body; replacing Claude hook groups can discard extra group or handler keys; JSON settings rewrites discard duplicate values and normalize retained negative zero; and doctor repair permanently deletes invalid or stale records. These prospective losses refuse clearly before stored-data writes. Doctor prepares the complete repair before applying any writes. Explicit removal of a provider integration remains intentional.

Malformed/reversed pre-commit markers in Go can duplicate text rather than discard it. That defect is copied. Malformed JSONL aborts doctor before its nominal invalid-line fix, also copied. Doctor summaries describe original checks even after successful repairs. Shared JSON pretty-print depth and compatibility metadata defects remain as documented by earlier slices.

One borrowed crate-private raw-array accessor supports iterative numeric-sidecar traversal without serializing away special numbers. No representation, parser, writer or existing tests are changed by that accessor.

Further comparison cases cover exact Decoder.Token diagnostics, relative bridge paths, failed transport zero values and input mutation, legacy migration notices, empty tracker status/title values, raw YAML threshold keys and ignored-domain identities, surgical edits to invalid UTF-8 instruction bytes, symlink/mode behavior, and explicit whole-file API replacements. Whole-file writeExpertiseFile requests intentionally replace their target; loss guards protect retained values and incidental rewrites rather than blocking the caller's explicit replacement.

The init dispatcher refuses the demonstrated Go loss of unreadable existing .gitattributes rules before creating initialization files. Read-only checks and explicit integration removal retain their Go behavior. Doctor's invalid non-comment JSONL aborts before repair; comments are permitted to disappear on rewrite as the owner directed.
