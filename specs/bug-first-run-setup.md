# Bug: first-run setup stops before prerequisites are available

## Metadata
- Type: bug
- Source: owner consolidation request, feedback A
- Date: 2026-11-13

## Bug Description
The setup guide connects before preparing tools or credentials. Terminal configuration has a reader but no persisted edit operation.

## Steps to Reproduce
1. Launch `lu init` without a running runtime or service token.
2. Observe the token prompt or connection refusal before tool setup.
- Expected: prepare private tools and settings, ask only for a missing provider key, connect and continue.
- Actual: manual setup is required before the guide starts.

## Root Cause Analysis
`src/main.rs` delegates immediately to the CLI/workbench. `prime.rs::run` begins with a health request. `config.rs` only reads configuration.

## Solution Statement
Bootstrap interactive init before opening the workbench; persist a private environment outside repositories and relaunch with it. Explicit environment overrides saved values. Install reviewed pinned Pi and Bun into a user-owned tools directory, run existing component setup, start the sibling runtime, and validate health/doctor. Add atomic configuration updates preserving unrelated fields. Offer a confirmed stash before a dirty baseline without weakening runtime policy.

## Out of Scope
Native component wiring and later navigation changes remain separate plans. Do not modify provider accounts, global npm tools, or existing runtime processes.

## Relevant Files
- `crates/luuna-tui/src/setup.rs` (new): private settings, bootstrap and runtime lifecycle.
- `crates/luuna-tui/src/main.rs`: first-run entry and saved environment relaunch.
- `crates/luuna-tui/src/config.rs`: atomic settings edit.
- `crates/luuna-tui/src/prime.rs`: dirty checkout recovery.
- `crates/luuna-tui/src/lib.rs`: module registration.

## Step by Step Tasks
1. Add settings and bootstrap regression coverage using disposable folders and synthetic credentials.
2. Implement private files with restrictive permissions and atomic writes; never display secrets.
3. Prepare pinned tools with bounded commands and existing setup scripts, preserving explicit environment values.
4. Start only a missing local runtime and wait for authenticated health and doctor.
5. Persist configuration changes and provide an explicit stash-and-continue choice.
6. Run required workspace checks before opening the plan PR.

## Validation Commands
- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --locked -- -D warnings`
- `cargo test --workspace --locked --no-fail-fast`
- `node scripts/check-hygiene.mjs`
- `npm ci && npm test`

## Notes
The owner's direct-session workflow supersedes migration audit/ship-gate requirements for this session. No provider run is claimed from fixture checks. Bootstrap is interactive only and does not alter scripted CLI behavior without saved settings.
