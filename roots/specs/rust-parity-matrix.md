# Rust rewrite parity matrix

This is the migration control sheet for the Rust rewrite. **Verified** means a
native result was compared with a captured or live TypeScript/Bun reference
artifact on Windows. **Covered** means the native implementation has focused
unit or fake-terminal regression coverage but does not yet claim a reference
PTY differential run. The source oracle was retained through the pre-removal
gate below. That gate passed on Windows, and the source oracle has now been
removed from the repository.

## Evidence boundary

- The permanent Windows behavior corpus (`tests/compat/behavior/`) contains
  88 reference-derived artifacts. Its five black-box suites run the native
  `luuna-roots` binary and compare raw stdout, stderr, exit status, JSONL,
  settings, hook, prose, Markdown, and HTML bytes. Only documented generated
  IDs/timestamps/temporary paths are normalized.
- Permanent help fixtures cover root help and all 33 command help pages.
  Before cutover, the equivalent live source-oracle sweep also passed.
- The native `compat` fixtures cover both executable names plus core
  initialization/storage behavior. Core and command tests cover actor,
  boundary, integration, renderer, editor, watcher, terminal, Think, Tend,
  and Flow reducers/lifecycles.
- This evidence was executed on Windows. The configured Linux/macOS/Windows
  CI matrix is not evidence that Linux or macOS has been executed locally.
  No unexecuted real-PTY/editor or cross-platform behavior is marked
  verified here.

## Cross-cutting contract

| Surface | Native implementation | Current evidence |
| --- | --- | --- |
| Executable names, parser, help, JSON envelopes, exit codes | complete | **verified:** `luuna-roots`/`lrt`, root plus 33 command help, parser/error/JSON behavior fixtures |
| `.luuna/roots/` paths, YAML, JSONL, locks, graph, prose, events | complete | **verified:** exact init and mutation/malformed/last-wins fixtures; **covered:** lock identity and atomic-write unit tests |
| Actor, TTY, boundary, hook, Claude settings policy | complete | **verified:** setup/guard settings and hook bytes, legacy aliases/masking and ownership errors; **covered:** interactive TTY paths |
| Roots/Missions/Atlas ownership boundaries | complete | **covered:** tolerant, read-only Missions/Atlas integration tests with no-store-mutation assertions |
| Interactive terminal/editor/agent lifecycle | complete | **covered:** fake-terminal/editor/watch/agent cancellation, Think/Tend/Flow session tests; no reference PTY claim |

## Commands

| Command | Native status | Current evidence |
| --- | --- | --- |
| `init` | complete | **verified:** exact layout/config/event/.gitattributes fixture |
| `setup claude` | complete | **verified:** dry-run/install/idempotent/remove settings and hook-byte fixtures |
| `guard` | complete | **verified:** JSON/hook protocol and legacy protected-tree fixtures |
| `plant` | complete | **covered:** native editor/TTY/capture regression tests |
| `think` | complete | **covered:** native reducer/session/editor/watcher/agent lifecycle tests |
| `flow` | complete | **covered:** native reducer/session/trail/background-agent/heading lifecycle tests |
| `adopt` | complete | **covered:** native adoption/editor/endorsement regression tests |
| `mv` | complete | **verified:** behavior and malformed-last-wins compaction fixtures |
| `tend` | complete | **covered:** native cards/decision/session/terminal lifecycle tests |
| `accept` | complete | **verified:** behavior fixture; **covered:** decision-rule tests |
| `reject` | complete | **verified:** behavior fixture; **covered:** decision-rule tests |
| `link` | complete | **verified:** behavior fixture |
| `scan` | complete | **verified:** behavior fixture; **covered:** mention/hash-ledger tests |
| `unlink` | complete | **verified:** behavior fixture |
| `commit` | complete | **verified:** planted-state error fixture; **covered:** lifecycle rules |
| `status` | complete | **verified:** behavior fixture |
| `compost` | complete | **verified:** behavior fixture |
| `tier` | complete | **verified:** behavior fixture |
| `context` | complete | **verified:** behavior fixture |
| `prime` | complete | **verified:** scope/hook behavior fixtures |
| `drift` | complete | **verified:** behavior fixture; **covered:** Git and integration scenarios |
| `ask` | complete | **verified:** behavior fixture |
| `note` | complete | **verified:** behavior fixture |
| `propose` | complete | **verified:** behavior fixture |
| `sprout` | complete | **verified:** behavior fixture |
| `heading` | complete | **verified:** persistence fixture; **covered:** Flow heading lifecycle |
| `show` | complete | **verified:** behavior fixture |
| `blame` | complete | **verified:** behavior fixture |
| `list` | complete | **verified:** behavior fixture |
| `queue` | complete | **verified:** behavior fixture |
| `log` | complete | **verified:** behavior fixture |
| `view` | complete | **verified:** Markdown/HTML/stdout and exact `--out` bytes |
| `verify` | complete | **verified:** behavior fixture; **covered:** malformed/boundary validation tests |

## Completed pre-removal and post-removal gates

The following source-oracle and permanent-fixture gates passed on Windows
before the TypeScript/Bun reference and its temporary test were removed:

```powershell
cargo test -p roots-cli --test reference_help_parity -- --ignored --nocapture
cargo test -p roots-cli --test behavior_fixtures
cargo test -p roots-cli --test help_fixtures
cargo test -p roots-cli --test compat
```

After removal, the native-only release gate passed on Windows:

```powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo build --workspace --release --locked
cargo run -p roots-cli --bin luuna-roots -- --help
cargo run -p roots-cli --bin lrt -- --version
git diff --check
```

The final repository must retain the permanent native fixture corpus, both
executable names, explicit upstream attribution, and the legacy-format/no-
automatic-migration boundary. Do not treat configured CI or unrun PTY tests as
completed cross-platform verification.
