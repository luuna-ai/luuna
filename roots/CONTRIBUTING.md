# Contributing to Roots

Thanks for your interest in contributing to Roots. Direction lives in Missions issues and Roots ideas; if a change conflicts with recorded intent, raise it in the issue or PR rather than quietly choosing one interpretation.

## Getting started

1. Fork and clone the repository.
2. Install the Rust toolchain pinned by `rust-toolchain.toml` (Rust `1.99.1`, rustfmt, and Clippy).
3. Point Git at the repository hook:

   ```sh
   git config core.hooksPath scripts/hooks
   ```

4. Create a branch such as `fix/description-of-change`.

## Build and test commands

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo test -p roots-cli --test compat
cargo build --workspace --release --locked
```

The first four commands are the local quality gate and CI contract. The compatibility test executes frozen observable fixtures; update it only after comparing a deliberate, documented CLI change with the reference contract.

## Coding conventions

- Keep public CLI behavior compatible: executable names, command names, flags, help, JSON envelopes, exit codes, storage records, and interaction keys are part of the API.
- Prefer a focused module over an overgrown command handler. `roots-core` owns reusable domain and persistence behavior; `roots-cli` owns argv, process I/O, and terminal integration.
- Use `cargo fmt`; do not hand-format around it. Keep warnings clean under the repository Clippy command.
- Keep the invariants in [AGENTS.md](AGENTS.md): agent commands never write under `.luuna/roots/human/`, human commands need a TTY where specified, every mutation logs one event, and nothing auto-accepts.
- Keep dependencies purposeful and locked in `Cargo.lock`. Do not replace the custom compatibility parser/help renderer with a framework that changes observable output.

## Testing

- Put pure/domain tests next to their Rust module; use integration tests for executable behavior.
- Use real temporary directories for storage and command tests. Add a CLI fixture for every newly observable behavior or regression.
- For terminal changes, use the fake terminal lifecycle tests and preserve raw-mode handoff, resize, cancellation, and background-job behavior.
- Run the full workspace gate before a PR. If a platform-specific behavior cannot run locally, state the platform and what was actually executed; do not call a catalog-only check cross-platform verification.

## Pull requests and issues

- Keep one concern per PR, include tests, and explain what changed and why.
- Report security issues through the process in [SECURITY.md](SECURITY.md), not a public issue.
- Contributions are licensed under the [MIT License](LICENSE).
