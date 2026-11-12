# Feature: Full Rust rewrite of the Luuna Roots CLI

## Metadata

- Type: feature
- Source: User request: completely rewrite the existing CLI in Rust without changing functionality, interactions, or commands.
- Date: 2026-10-22
- Reference implementation: former TypeScript/Bun CLI at Git commit ce22500;
  removed after the recorded pre-cutover oracle gate passed.
- Migration evidence: see `specs/rust-parity-matrix.md`; it distinguishes
  Windows reference verification from native-only and configured-CI coverage.

## Feature Description

Replace the complete TypeScript/Bun implementation of Luuna Roots with a Rust implementation while retaining the observable behavior of the existing luuna-roots and lrt executables. The finished binary must operate on the identical .luuna/roots data format, accept the identical command and flag surface, retain legacy environment compatibility, preserve the human/agent boundary behavior, and produce compatible human, JSON, error, and interactive output.

The rewrite is a compatibility migration, not a redesign. The former
TypeScript executable was retained as the behavioral oracle until the recorded
Rust compatibility gate proved the supported behavior, then removed at
cutover.

## User Story

As a Luuna project user or harness author, I want to replace the Bun-based Roots CLI with a native Rust binary so that the command line interface remains familiar and data-compatible while distribution, runtime integration, and cross-platform support improve.

## Solution Statement

Create a Cargo workspace with a reusable roots-core library and a roots-cli binary crate. Port behavior in vertical slices from pure parsing/data logic through command handlers and interactive terminal workflows, using the current executable and TypeScript tests as a black-box and white-box compatibility contract. Keep the existing JSONL store layout and Claude hook contract unchanged; do not add automatic .roots migration or make Roots an authorization mechanism.

Rust is chosen because Luuna's managed runtime is already Rust-oriented, the result can be delivered as a single native binary, and its type system is well suited to the CLI's identity, graph, boundary, and persistence invariants. The implementation must not use a general CLI framework where its help, error, or flag behavior would diverge from the current custom parser.

## Out of Scope

- Changing any user-visible command name, alias, positional argument, flag, default, help text, exit-code contract, JSON envelope, storage record shape, or interaction key binding.
- Changing Roots ownership boundaries or treating a Roots record, prompt, or hook as Luuna runtime authorization.
- Automatically importing or mutating legacy upstream .roots projects.
- Replacing Missions, Atlas, Beacon, or the Luuna Rust runtime; Roots remains a read-only consumer of Missions and Atlas.
- Adding a luuna roots dispatcher, npm publication, cloud service, telemetry, or a desktop UI.
- The TypeScript oracle was not deleted until Rust compatibility tests and
  migration acceptance criteria were green.

## Compatibility Contract

### Executables and invocation

- Preserve executable names luuna-roots and lrt.
- Preserve root --help, -h, --version, and -v behavior, version value, command ordering, group headings, color behavior, unknown-command suggestions, arity errors, and global --json, --quiet/-q, --help/-h behavior.
- Preserve custom argument parsing: --flag value, --flag=value, repeatable string flags, -- to terminate parsing, non-bundled short flags, and existing handling of negative numbers and lone dashes.
- Preserve one-document JSON output envelopes and typed error codes/exit codes: 0 success, 1 internal/cancelled, 2 usage/ambiguous, 3 missing/not initialized, 4 guard, 5 conflict, 6 validation.
- Preserve current platform-specific behavior until intentionally covered by a compatibility fixture. Do not silently normalize legacy wording, path separators, shell behavior, or old roots branding that the reference executable exposes.

### Store and data compatibility

- Read and write the current .luuna/roots/config.yaml, graph.jsonl, proposals.jsonl, questions.jsonl, events.jsonl, headings.jsonl, human, and agent trees without schema migration.
- Preserve IDs, slug generation, timestamps, hash behavior, node and edge rules, append/compaction semantics, malformed-line tolerance, last-record-wins deduplication, locks, stale lock reclaim policy, atomic writes, .gitattributes union merge rule, and event ordering.
- Preserve directory reconciliation behavior, including the narrow graph/event write triggered when a human has manually renamed a node directory.
- Preserve legacy environment names ROOTS_USER and ROOTS_AGENT alongside LUUNA_ROOTS_USER and LUUNA_ROOTS_AGENT.

### Policy and integration compatibility

- Preserve human/agent actor identity rules, TTY gates, agent-owned write confinement, hash ledger checks, Claude hook input/output protocol, and setup claude merge/remove semantics.
- Preserve the fact that these are CLI and host-hook defenses, not Luuna runtime permission.
- Preserve read-only Missions links from .luuna/missions/issues.jsonl and Atlas links from .luuna/atlas/expertise/*.jsonl. Keep inherited adapter names seeds-link and mulch-link conceptually compatible even if Rust module names are clearer.

## Parity Matrix

| Surface | Reference modules | Rust target | Required proof |
| --- | --- | --- | --- |
| Entrypoint, registry, output, errors | src/index.ts, cli.ts, registry.ts, args.ts, output.ts, errors.ts | crates/roots-cli/src/bin/{luuna-roots,lrt}.rs, lib.rs, registry.rs, roots-core args/output/errors | Golden help, version, parser, JSON, stderr, color, and exit-code tests |
| Identity and policy gates | actor.ts, guard.ts, boundary.ts, hook-guard.ts | roots-core actor, guard, boundary, hook modules | Human/agent identity, TTY, traversal, symlink, hook protocol tests |
| Configuration and paths | config.ts, yaml.ts, paths.ts, workspace.ts | roots-core config, paths, workspace | Existing config and upward-discovery fixtures pass unchanged |
| Persistence and graph | store.ts, graph.ts, edges.ts, types.ts, events.ts | roots-core store, graph, model, events | Cross-language read/write fixture comparison and corruption tests |
| Prose and lifecycle | prose.ts, node-create.ts, lifecycle.ts, adopt.ts, endorse.ts | roots-core prose, node, lifecycle, adoption | Same IDs, slugs, records, prose, statuses, and events |
| Questions/proposals/sprouts | questions.ts, proposals.ts, sprouts.ts, rules.ts, mentions.ts | roots-core questions, proposals, sprouts, rules, mentions | Unit and black-box command parity |
| Context, presentation, history | context.ts, prime.ts, view.ts, view-html.ts, blame.ts, format.ts | roots-core render, context, prime, view, blame | Markdown/HTML/JSON snapshots |
| Interactive sessions | think-*.ts, tend-*.ts, flow-*.ts, terminal.ts, editor*.ts, watch.ts, spinner.ts | roots-cli interactive modules | PTY golden frames, key-sequence scenarios, file-watch and cancellation tests |
| Agent execution | agent-runner.ts, agent-phase.ts, delegation.ts, heading-run.ts | roots-core agent runner and roots-cli process glue | stdin/env/cwd/timeout/process-tree fixtures |
| Integrations and drift | seeds-link.ts, mulch-link.ts, drift.ts, git-trust.ts | roots-core missions, atlas, drift, git modules | JSONL integration fixtures and Git repository scenarios |
| Developer tooling | package.json, scripts, Bun tests, GitHub CI | Cargo manifests, Rust tests, CI/scripts/docs | Locked Cargo build, lint, tests, coverage, CI parity |

### Command and interaction parity

| Group | Commands that must remain available | Compatibility requirements |
| --- | --- | --- |
| Setup | init; setup claude; guard | Existing directory layout, .gitattributes, settings merge, hook JSON and deny/allow output |
| Human capture | plant; think; flow; adopt; mv | TTY/editor/split behavior, keyboard interactions, session and prose/event side effects |
| Human structure | tend; accept; reject; link; unlink; scan; commit; status; compost; tier | Proposal cards, decisions, graph/lifecycle mutations, exact guard behavior |
| Agent facing | context; prime; drift; ask; note; propose; sprout; heading | Tier enforcement, cited content, agent identity, read packets, artifact paths, hook-safe drift |
| Read | show; blame; list; queue; log; view; verify | Text, HTML, JSON, warnings, errors, exit codes, and intentional reconciliation writes |

The implementation must port every command-specific flag, including all --as, --session, --cite, --reason, --scope, --from, --out, --html, --sprouts, --all, --anchors, --orphans, --kind, --status, --no-split, --endorse, --keep, --rel, --project, --user, --remove, --dry-run, --hook, --diff, --file, --name, --question, --flow, and --next variants. The command source files and their current --help output are the authoritative exact flag contract.

## Relevant Files

### Existing reference files

- AGENTS.md - ownership, data-format, and runtime-boundary rules that the rewrite must preserve.
- README.md and UPSTREAM.md - user-facing scope, upstream attribution, data compatibility, and distribution constraints.
- package.json - current executable names, version, source-install commands, and quality gates to replace deliberately.
- src/index.ts, src/cli.ts, src/register-all.ts, src/registry.ts - current executable and command registry.
- src/commands/*.ts - complete 33-command public behavior.
- src/actor.ts, src/guard.ts, src/boundary.ts, src/hook-guard.ts - actor and filesystem boundary behavior.
- src/paths.ts, src/config.ts, src/store.ts, src/graph.ts, src/types.ts, src/events.ts - persistence contract.
- src/think-*.ts, src/tend-*.ts, src/flow-*.ts, src/terminal.ts, src/watch.ts - interactive contract.
- src/seeds-link.ts and src/mulch-link.ts - read-only Missions and Atlas adapters.
- src/**/*.test.ts and scripts/**/*.test.ts - behavioral test corpus to port or turn into black-box fixtures.
- .github/workflows/ci.yml, scripts/hooks/pre-commit, RUNBOOK.md, CONTRIBUTING.md, SECURITY.md - developer/release workflow to rewrite after functional parity.

### New Rust files and directories

- Cargo.toml (new) - locked workspace manifest and shared lint policy.
- Cargo.lock (new) - committed dependency lock.
- crates/roots-core/Cargo.toml (new) - pure domain/store library.
- crates/roots-core/src/lib.rs and modules (new) - model, store, config, graph, rules, integrations, rendering, agent execution.
- crates/roots-cli/Cargo.toml (new) - executable crate.
- crates/roots-cli/src/bin/luuna-roots.rs and src/bin/lrt.rs (new) - luuna-roots and lrt entrypoints.
- crates/roots-cli/src/commands/*.rs (new) - one Rust command module per existing command surface.
- crates/roots-cli/src/interactive/*.rs (new) - think, tend, flow, terminal, editor, watch, and spinner behavior.
- crates/roots-cli/tests/{help_fixtures,compat,behavior_fixtures}.rs and tests/compat/** (new) - permanent black-box help, behavior, data, output, hook, and storage compatibility corpus.
- rust-toolchain.toml (new) - pinned stable toolchain for reproducible local/CI builds.

## Implementation Plan

### Foundation

Create a root Cargo workspace with roots-core and roots-cli crates. Keep the TypeScript implementation intact while the Rust binary is developed, and make the Rust binary available under separate temporary test paths so neither implementation overwrites the other's executable during migration.

Use custom command parsing and help rendering rather than clap-derived formatting. Prefer serde/serde_json/serde_yaml for format compatibility, thiserror for typed errors, chrono/time for ISO timestamps, sha2 for hashes, rand for IDs/temp names, regex for current text rules, walkdir for scans, crossterm plus portable-pty for interactive terminal behavior, notify for file watches, and tempfile/assert_cmd/insta/proptest for testing. Use platform-gated process and terminal code; retain the reference Unix shell behavior where it is already part of the contract.

### Core

Port pure behavior before commands: models, IDs, slugs, argument parsing, output envelopes, errors, paths, YAML config, JSONL storage, graph updates/invariants, events, prose resolution, questions, proposals, sprouts, lifecycle, citation validation, notes, mention scans, deterministic rules, hashes, and integration readers.

For each ported module, create Rust unit tests using fixtures copied from current TypeScript test cases. Build a black-box fixture corpus by running the TypeScript reference executable over deterministic repositories before deleting it. The Rust implementation must read stores produced by TypeScript and TypeScript must be able to read stores produced by Rust during the migration period.

### Integration

Port all 33 handlers through a registry that reproduces their exact ordering, usage, flag names, errors, and JSON. Port Claude settings merging and guard protocol before enabling setup claude. Port view, HTML, prime, context, queue, log, show, blame, verify, Missions, Atlas, Git drift, and optional agent-command execution. Then port think, tend, flow, editor split handling, watches, agent background phases, and terminal rendering with PTY tests.

Only after the Rust suite proves parity may the repository delete TypeScript source, Bun manifests/lockfiles, Bun-only scripts, and Bun CI setup. Rewrite README, runbook, contributor, security, hook, and CI instructions for Cargo without changing the documented CLI commands or the upstream attribution.

## Step by Step Tasks

1. Record a compatibility baseline before code replacement.
   - Capture root and command help, --version, representative success/error JSON, stdout/stderr, exit codes, store snapshots, hook decisions, view HTML/Markdown, and interactive PTY frames from the TypeScript reference.
   - Build fixtures for empty, malformed, merged, symlink, legacy-environment, Missions, Atlas, git, agent, proposal, sprout, endorsement, and lifecycle repositories.
   - Add a generated-fixture manifest with the reference commit and command invocation for each artifact.

2. Establish the Rust workspace without changing behavior.
   - Add root Cargo.toml, Cargo.lock, rust-toolchain.toml, roots-core, and roots-cli.
   - Define two binary targets named luuna-roots and lrt that invoke the same CLI runtime.
   - Add cargo fmt, clippy, test, release build, test coverage, and cross-platform checks to developer/CI plans, while retaining TypeScript until parity is established.

3. Port the compatibility shell first.
   - Implement typed error/exit handling, custom argv parsing, command registry, color policy, JSON envelopes, help renderer, version behavior, and deterministic suggestion logic.
   - Add golden tests for every root/command help page and parser edge case.

4. Port the persistent domain layer with byte-and-record compatibility.
   - Implement .luuna/roots path discovery, config parsing/rendering, JSONL parse/serialize/dedup, advisory locking, stale lock recovery, atomic writes, graph access, events, IDs, slugs, prose, node creation, and workspace reconciliation.
   - Test bidirectional TypeScript/Rust fixture compatibility and preserve the union-merge behavior.

5. Port non-interactive command logic in dependency order.
   - Implement init, list, show, log, link, unlink, lifecycle commands, tier, scan, context, prime, drift, ask, note, propose, sprout, heading, blame, view, verify, and guard.
   - Port actor identity, boundary validation, citation checks, rule engine, renderers, Mission/Atlas readers, Git trust, and Claude settings merge alongside the commands that depend on them.
   - Verify every command against JSON and text fixtures, including failures and guard cases.

6. Port human editing and interaction flows.
   - Implement editor invocation, split-pane negotiation, terminal raw mode, screen rendering, polling/watch behavior, spinner behavior, and test-friendly terminal abstraction.
   - Port plant, think, adopt, tend, and flow with all key bindings, cancellation semantics, agent delegation, background phases, and events.
   - Use portable PTY integration tests for representative terminal dimensions and key sequences; preserve no-TTY guards precisely.

7. Port configured-agent execution safely and compatibly.
   - Reproduce stdin Markdown packets, actor/session/node environment shaping, cwd, timeout, output capture/tail behavior, process-tree termination, and soft failure behavior.
   - Keep the agent submission boundary CLI-mediated; agent stdout must not become accepted structure.
   - Test Unix shell behavior and platform-specific fallback behavior explicitly rather than changing it silently.

8. Harden policy and data parity.
   - Run all existing TypeScript scenarios against the Rust executable through a differential harness.
   - Validate no command ever lets an agent write .luuna/roots/human or accept its own proposal.
   - Verify intentional legacy behavior such as ROOTS_* aliases, directory reconciliation, malformed JSONL tolerance, and permanent rejection memory.

9. Cut over tooling only after parity.
   - Replace package.json/Bun scripts and lockfiles with Cargo manifests, cargo commands, Rust test configuration, Rust pre-commit hook, and GitHub Actions Rust setup.
   - Update source-install, runbook, contributor, security, and CI documentation. Retain license, attribution, upstream remote/history, and compatibility notice.
   - Remove TypeScript source only after a clean Rust compatibility report proves every command and fixture is covered.

10. Perform full release-grade validation.
    - Run Linux, Windows, and macOS CI matrix validation for non-interactive behavior.
    - Run Linux PTY tests for editor/shell/multiplexer behavior; label any OS-specific reference behavior explicitly.
    - Confirm a legacy .luuna/roots fixture works unchanged with the final Rust binary, then inspect a final diff for unintended data-format or policy changes.

## Testing Strategy

- Unit: Port all pure TypeScript unit tests for parser, actor, config, storage, graph, rules, IDs, slugs, renderers, integration scanners, boundary logic, and verification.
- Integration: Execute every one of the 33 commands against fixture projects; assert stdout, stderr, JSON, exit code, on-disk tree, JSONL records, and events.
- Differential: Capture TypeScript reference results from ce22500 and compare Rust results over deterministic fixtures. Normalize only nondeterministic timestamps, generated IDs, and platform-declared path separators through an explicit manifest rule; never normalize user-visible semantic changes.
- Interactive / E2E: Use portable PTY tests for plant, think, adopt, tend, and flow, with editor stubs, watcher synchronization, terminal-size golden frames, and cancellation paths.
- Cross-platform: Run native Linux, Windows, and macOS non-interactive tests. Exercise symlink security tests on a Windows runner with Developer Mode/elevation or record an explicit infrastructure limitation rather than calling them passed.
- Compatibility: Open existing .luuna/roots fixtures made by TypeScript, perform an operation in Rust, and verify their records remain readable and semantically unchanged.

## Acceptance evidence and boundaries

- [x] The Rust registry exposes both `luuna-roots` and `lrt`, all 33 command
  names, and the frozen root/command-help bytes. The live reference-help
  sweep passed on Windows before the source oracle was removed.
- [x] Native Windows behavior fixtures compare representative non-interactive
  success and failure output, JSON envelopes, exit classes, settings, guard
  protocol, legacy environment masking, data bytes, and malformed JSONL
  last-wins behavior against reference-derived artifacts.
- [x] The retained fixture corpus proves existing `.luuna/roots/` stores can
  be initialized, read, and mutated without a schema migration; direct unit
  tests cover parser, paths, graph, locking, atomic writes, boundaries, and
  read-only Missions/Atlas integration.
- [x] Native tests cover human/agent confinement, proposal decisions, agent
  provenance, no-self-acceptance, setup/guard semantics, and representative
  editor/terminal/session behavior for plant, think, adopt, tend, and flow.
- [x] Native Cargo manifests, locked dependencies, and the local hook build
  both executable names without a Bun or Node runtime dependency; configured
  CI is set to run the same native build.
- [x] The documentation retains MIT/upstream attribution and explicitly says
  legacy `.roots/` stores are not migrated automatically.
- [ ] Linux and macOS execution is intentionally not claimed from this Windows
  checkout. The CI matrix is configured to provide that evidence after it
  runs.
- [ ] A real reference-PTY differential run is intentionally not claimed;
  fake-terminal, editor, watcher, and session tests are the currently
  executed interactive evidence.

The completed behavior matrix is deliberately evidence-scoped rather than a
claim that every possible argument combination or host was exhaustively
executed. See `specs/rust-parity-matrix.md` for command-level status and the
permanent fixture manifest.

## Validation commands

Before the completed cutover, the following direct oracle gate and permanent
native fixtures passed on Windows:

```powershell
cargo test -p roots-cli --test reference_help_parity -- --ignored --nocapture
cargo test -p roots-cli --test behavior_fixtures
cargo test -p roots-cli --test help_fixtures
cargo test -p roots-cli --test compat
```

After removing the source oracle and its temporary parity test, the
native-only release gate passed on Windows:

```powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo build --workspace --release --locked
cargo run -p roots-cli --bin luuna-roots -- --help
cargo run -p roots-cli --bin lrt -- --version
git diff --check
```

## Notes

- This is a high-risk semantic migration because the current application includes a custom parser, event-sourced JSONL behavior, filesystem boundary defenses, external readers, and full-screen interactive workflows. Mechanical translation is insufficient.
- Existing tests are a starting point, not proof of full compatibility; they have known native Windows and source-adaptation gaps. The compatibility fixture corpus is mandatory.
- Cargo dependency versions must be committed in Cargo.lock. Avoid unpinned equivalent tooling such as network-fetched cargo install commands in CI.
- Do not use automatic serialization changes that alter JSON object key order, omitted fields, whitespace, YAML rendering, generated IDs, or time formatting without fixture evidence.
- The final documentation may change installation commands from Bun to Cargo/released binaries, but it must preserve user-facing executable names and command examples.
