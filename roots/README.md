# Luuna Roots

**Human-owned intent for Luuna projects.** Roots records why a project exists and where the developer wants it to go. Luuna Missions tracks work, Atlas stores verified expertise, Beacon stores reusable prompts, and the Luuna runtime controls approved execution. A Roots record does not grant an agent authority to change a human's goals.

Luuna Roots is a native Rust CLI with two equivalent executable names: `luuna-roots` and `lrt`. It is a Luuna-maintained, MIT-licensed adaptation of [jayminwest/roots](https://github.com/jayminwest/roots). The original history, MIT license, and attribution are preserved; see [UPSTREAM.md](UPSTREAM.md).

## Install from this checkout

The checked-in `rust-toolchain.toml` pins Rust `1.99.1`. Install that toolchain with rustup, then build the native binaries:

```sh
cargo build --release --locked
./target/release/luuna-roots --help
```

On Windows, run `target\\release\\luuna-roots.exe`. To install both names into Cargo's bin directory instead, use:

```sh
cargo install --path crates/roots-cli --locked
```

Neither command needs Bun or Node at runtime. The project is not published to crates.io yet; distribution and release signing remain a separate release decision.

## Project workflow

```sh
luuna-roots init
luuna-roots plant "The project works offline without losing user work"
luuna-roots queue
luuna-roots think <idea-id>
luuna-roots tend
luuna-roots view
```

`init` creates `.luuna/roots/` in the selected project. Human-authored prose lives under `.luuna/roots/human/`; CLI-owned graph, questions, proposals, headings, and events are JSONL. Agents may ask questions, attach findings, and propose structure through the CLI. A human accepts, rejects, changes status, and writes the intent. `luuna-roots prime` prints accepted context for task setup; it is project data, not an instruction that overrides Luuna policy.

Roots can read linked Missions issues and Atlas expertise records, but never writes either store. Link an idea by its `r-` ID in a Missions issue's `intent` or `extensions.intent` field, or by an exact ID mention. An Atlas record that cites the same ID appears with the idea. These links are advisory context; an approved Luuna plan must still bind immutable inputs and pass the runtime's checks and review.

Use `luuna-roots --help` and `luuna-roots <command> --help` for the exact command reference. Interactive human commands require a terminal and an identified human (`LUUNA_ROOTS_USER` or Git's `user.name`); agent commands use `--as agent:<name>` or `LUUNA_ROOTS_AGENT`. The legacy `ROOTS_*` variables remain supported for upstream and harness compatibility.

For the complete CLI, storage, integration, interactive, and tooling map, read [ARCHITECTURE.md](ARCHITECTURE.md).

## Boundaries

- The `.luuna/roots/human/` guard protects Claude Code's Edit/Write surface when `luuna-roots setup claude` is installed. It is a host hook, not a sandbox; Luuna's runtime remains the authority for managed sessions.
- The CLI does not initialize Missions, Atlas, or Beacon. Each component is independently usable.
- Existing upstream `.roots/` projects need an explicit migration; `init` does not silently move their data.
- Desktop applications are distributed separately from this CLI.

## Development

The Cargo workspace has `roots-core` for persistence and domain rules and `roots-cli` for the executable and interactive runtime. Before opening a PR, run:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo build --workspace --release --locked
```

See [AGENTS.md](AGENTS.md) for repository invariants and [CONTRIBUTING.md](CONTRIBUTING.md) for the contributor workflow.

## License

MIT. Copyright and attribution are in [LICENSE](LICENSE) and [UPSTREAM.md](UPSTREAM.md).
