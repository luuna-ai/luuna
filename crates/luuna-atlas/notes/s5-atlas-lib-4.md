# Notes for s5-atlas-lib-4: prime, learn, diff, sync and hook

Copied from the plan s5-atlas-lib-slots (plan.md) so that this plan starts from them. Paths below are under `crates/luuna-atlas/`.

## Ownership

- Owns: `src/prime/**` (prime, learn, diff, sync, active-work context with `gh pr view`, FormatPrime and the manifest, and `hook` with hand-written matchers); `tests/parity_prime_git.rs`, `tests/fixtures/s5-atlas-lib-4/` and this file.
- Foundation files it may still edit: none expected.
- Needs on main: s5-atlas-lib-2 (git, hooks, scores, summary, renderers, assets, style) and s5-atlas-lib-3 (`find_stale_records`, `collect_superseded_ids`, `prime_stale_ids`, `validate_corpus`).

## Ownership notes

- The human renderers are -2's (`src/actions/render.rs`); this plan calls them.
- `regex` is not a dependency. This plan hand-writes the `hooks_guard.go:12-17` shell patterns, `hooks_inject.go:24`, `git_commands.go:211` and the `git.go:274-302` tracker patterns with Go's ASCII classes and leftmost-first semantics (hand-written matchers), and covers `shellRedirectRE` and the `\b` tracker patterns with parity cases. If it concludes it cannot, it needs a revised plan that edits the frozen `Cargo.toml` and `Cargo.lock`.
- Tests never call the real `gh` (AS17).

## Assumptions that name this plan

- AS12: `hook` belongs to this plan.
- AS13: port the human paths faithfully on top of -2's renderers and style.
- AS17: `gh pr view` is part of ResolveActiveWork (`git.go:259-302`) and is ported faithfully; tests run with a PATH that has no `gh` or a stub `gh`.

## Go data loss (A8)

None of this plan's own cases is known. It keeps the indented first key refusal through `config::read_config` (GDL1) and must not bypass it.

GDL1 (observed): a `config.yaml` whose first key is indented and a later key is less indented (the indented first key case, for example `  b: 1` then `a: 2`). Go's yaml.v3 reads the later key as a second document, so `config show` prints the config without `a` and `add` rewrites the file without `a: 2`. The port refuses with `unsupported YAML at line 2: content after the end of the document is not supported` (`yaml.rs:399-404`), exits 1 and leaves the file untouched.

Comments on rewrite (decided for the owner, AS19 confirmed; orchestration ledger, needs-owner-wave-2.md, decided item 3): loss of comments alone on a rewrite of `config.yaml` is copied from Go and is listed in the hand-off as a known loss. Loss of keys, values, documents, lines, records or body text is refused with the file untouched.

Every other Go behaviour is still copied (A4 differences), suspected defects included. Only the cases named here are deliberate differences from Go.

## Carried from the foundation reviews

- CF1: `json::Value` derives `Debug` and `PartialEq` (`json.rs:46`), both recursive per nesting level, and `Integer(100) != Number(100.0)`. No `{:?}` and no `==` on Values from user data in non-test code; compare numbers as Go does.
- CF5: Go's JSONPretty prints an empty line with exit 0 past 10000 levels; the crate copies it (`output.rs:198-199`). Keep copying and list it in the hand-off as a suspected Go defect.
