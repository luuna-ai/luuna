# Luuna Roots repository guidance

Luuna Roots is the Luuna-maintained, MIT-licensed adaptation of jayminwest/roots. Preserve upstream attribution, human authorship, and accepted-graph semantics. Read `README.md` and `UPSTREAM.md` before changing storage or behavior.

## Component ownership

- Roots stores human-authored project intent under `.luuna/roots/` and agent questions/proposals in its agent-owned area.
- Missions owns tickets and plans under `.luuna/missions/`.
- Atlas owns verified expertise under `.luuna/atlas/`.
- Beacon owns reusable prompts under `.luuna/beacon/`.
- Luuna's Rust runtime owns approvals, scope, checks, review, persistence, and publication for managed runs.

No agent may write `.luuna/roots/human/`, accept its own proposal, or claim that a prompt or Roots record grants runtime permission. The CLI's human actor checks and Claude hook are useful defenses but are not substitutes for Luuna runtime policy.

## Code layout

- `crates/roots-cli/src/bin/luuna-roots.rs` and `crates/roots-cli/src/bin/lrt.rs` start the two native binaries; `lib.rs` owns dispatch and output boundaries.
- `crates/roots-cli/src/registry.rs` defines the public command order, help, flags, and handlers. Command modules own command wiring; `interactive/` owns terminal, editor, think, tend, and flow interaction.
- `crates/roots-core/src/paths.rs` resolves the Roots store, `store.rs` owns JSONL/lock/atomic-write behavior, `actor.rs` enforces identity, and `hook_guard.rs` implements the Claude hook protocol.
- `crates/roots-core/src/integrations.rs` is the compatibility adapter for read-only Luuna Missions and Atlas records. Preserve the inherited seeds-link/mulch-link behavior when changing it.
- `tests/compat/` freezes reference CLI and storage behavior; `specs/rust-parity-matrix.md` records rewrite evidence.

For any path or format change, inspect readers, writers, hook guards, CLI help, compatibility fixtures, and affected callers together. Preserve legacy data through an explicit migration instead of treating a renamed directory as an automatic upgrade. Do not hand-edit project JSONL records.
