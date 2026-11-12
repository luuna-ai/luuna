# Notes for s5-atlas-lib-3: lifecycle commands, YAML and config writers

Copied from the plan s5-atlas-lib-slots (plan.md) so that this plan starts from them. Paths below are under `crates/luuna-atlas/`.

## Ownership

- Owns: `src/lifecycle/**` (query, rank, ready, status with FormatStatus, stale, validate, outcome, move, archive, restore, delete-domain with RemoveDomain, prune, compact, retention, config set/unset/schema, `config show --path`, and the embedded `contract.json` data); `tests/parity_lifecycle.rs`, `tests/fixtures/s5-atlas-lib-3/` and this file.
- Foundation files it may still edit: `src/yaml.rs` (full YAML, keeping the GDL1 refusal); possibly `src/config.rs`; the README section 'Supported YAML subset'. These are the only foundation edits planned among -2 to -5. Two plans that edit the same foundation file cannot run together.
- Needs on main: s5-atlas-lib-2 (schema, scores, git, `run_hooks`, record summary, `confirm`, options, time helpers).

## Ownership notes

- Only delete-domain and compact call `confirm.rs` (delete never prompts).
- This plan owns the embedded `contract.json` data (AS16), inside `src/lifecycle/`.
- Edits `yaml.rs`, possibly `config.rs`, and the README YAML section.

## Assumptions that name this plan

- AS13: port the human paths faithfully on top of -2's `style.rs` and renderers.
- AS16: embed the data of `internal/atlas/contract.json` that the port needs (configSchema for config set/unset/schema, help text); `src/lifecycle/` owns the copy and -5 reads help data through it. If the data may not be copied, state the same data as Rust literals; the bytes are the same.

## Go data loss (A8)

GDL1 (observed): a `config.yaml` whose first key is indented and a later key is less indented (the indented first key case, for example `  b: 1` then `a: 2`). Go's yaml.v3 reads the later key as a second document, so `config show` prints the config without `a` and `add` rewrites the file without `a: 2`. The port refuses with `unsupported YAML at line 2: content after the end of the document is not supported` (`yaml.rs:399-404`), exits 1 and leaves the file untouched.
This plan keeps that refusal when it extends `yaml.rs` and adds a test that the file is unchanged after the refused command.

GDL2 (inferred, not built on): `config.yaml` using anchors, aliases and merge keys, rewritten by Go's `add` or `config set`/`config unset`. Today the crate refuses these files, so nothing is lost. When this plan adds anchors, aliases and merge keys, it first probes Go's rewrite on such files. Any case where a key or value present before is absent after is refused with a clear error, the file left untouched, and listed in the plan and hand-off as a deliberate difference. Where nothing is lost, it copies Go.

Comments on rewrite (decided for the owner, AS19 confirmed; orchestration ledger, needs-owner-wave-2.md, decided item 3): loss of comments alone on a rewrite of `config.yaml` is copied from Go and is listed in the hand-off as a known loss. Loss of keys, values, documents, lines, records or body text is refused with the file untouched.

Every other Go behaviour is still copied (A4 differences), suspected defects included. Only the cases named here are deliberate differences from Go.

## Carried from the foundation reviews

- CF1: `json::Value` derives `Debug` and `PartialEq` (`json.rs:46`), both recursive per nesting level, and `Integer(100) != Number(100.0)`. No `{:?}` and no `==` on Values from user data in non-test code; compare numbers as Go does.
- CF2: config and file writes follow symlinks as Go's `os.WriteFile` does (`config.rs:279`, `commands.rs:219`). Copy it, do not fix it, list it in the hand-off as a suspected Go defect. Applies to config set/unset: they follow symlinks as Go does (symlink).
- CF3: for some invalid `config.yaml` the inner yaml.v3 syntax-error text or line number differs from Go's. Match Go's syntax-error text or list each remaining case as a known difference; never change the exit code or write the file.
- CF4: `[a]:d` (no space after the colon) as a later key is reported as unsupported where Go gives a syntax error; on line 1 Go accepts it and the crate refuses it. Require whitespace or end of line after the colon in `refuse_flow_key`, or list it as a known difference.
- CF5: Go's JSONPretty prints an empty line with exit 0 past 10000 levels; the crate copies it (`output.rs:198-199`). Keep copying and list it in the hand-off as a suspected Go defect.
- CF6: Go's `Number()` returns 0 for a uint64, so a port of Number must not reuse `as_f64` unchanged. Applies to config values.
