# Native behavior-fixture manifest

This directory is a Windows reference corpus captured from the TypeScript/Bun
CLI before the Rust cutover. `crates/roots-cli/tests/behavior_fixtures.rs`
executes the Rust binary only; running the test never needs Bun, Node, or a
checkout of the reference implementation.

The input states were created through the reference CLI and its supported
store APIs. The runner restores the omitted zero-byte JSONL files and the
40-byte evidence artifact because Git cannot retain those empty directories
and because the artifact is intentionally shared by several cases. It does
not hand-author Roots records at test time.

## Allowed normalizers

Only values intentionally nondeterministic in the public CLI are normalized:

- temporary project roots and the project basename during `init`;
- generated question, proposal, edge, sprout, and heading IDs;
- generated dates, timestamps, and expiry timestamps; and
- the date portion of generated agent-note filenames.

All other stdout JSON shape/order, stderr, exit status, JSONL order, prose,
report bytes, hook bytes, and settings bytes are asserted exactly.

## Coverage

| Fixture test | Commands and behavior captured |
| --- | --- |
| `windows_init_preserves_reference_store_bytes` | `init`; exact config, `.gitattributes`, Roots `.gitignore`, empty stores, and init event. |
| `windows_setup_guard_environment_and_legacy_fixtures_match_reference` | `setup claude` dry-run/install/idempotent/remove/remove-idempotent; `guard` JSON and hook protocol; human/agent ownership rejection; `LUUNA_ROOTS_*` precedence and empty-value masking; `ROOTS_*` aliases; current-only store discovery. |
| `windows_mutation_fixtures_match_the_reference_cli` | `mv`, `link`, `unlink`, `tier`, `ask`, `note`, `propose`, `accept`, `sprout`, `reject`, `scan`, `status`, `compost`, and the planted `commit` error; final graph/events/proposals/questions/sprout bytes. |
| `windows_read_and_heading_fixtures_match_the_reference_cli` | `context --no-repo`, `prime --scope`, `prime --hook` outside a project, `drift`, `show`, `blame`, `list --all`, `queue`, `log --limit`, `view --from`, `view --sprouts`, `view --html --out`, Markdown `view --out`, `verify`, and `heading`. |
| `windows_malformed_jsonl_last_wins_fixture_matches_reference` | tolerant malformed JSONL read, duplicate record last-wins semantics, first-seen ordering, atomic compacting `mv`, and no leftover lock/temp files. |

## Store and ownership boundaries

The source implementation deliberately does not discover a legacy `.roots`
store as a Roots project. The corpus therefore asserts the reference
`not_initialized` result for a legacy-only directory rather than inventing a
bidirectional storage migration. The hook still guards both `.roots/human/`
and `.luuna/roots/human/`, and both paths are exercised.

The suite also proves that `ROOTS_AGENT` blocks human structure commands,
while `LUUNA_ROOTS_AGENT` identifies an agent without granting it human-write
authority. A present-but-empty `LUUNA_ROOTS_AGENT` or `LUUNA_ROOTS_USER`
masks the legacy alias exactly as the reference does.

## Deliberately excluded commands

`plant`, `adopt`, `think`, `flow`, and `tend` require an interactive terminal,
editor, or pane/session lifecycle and are outside this noninteractive corpus.
Their command-specific Rust tests remain responsible for those flows. In
particular, this fixture corpus does not claim `tend` JSON parity; add that
only after its reviewer-owned interactive implementation has a stable
reference scenario.

Run the native gate with:

```powershell
cargo test -p roots-cli --test behavior_fixtures
```
