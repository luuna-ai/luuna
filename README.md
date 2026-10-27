# Luuna

Luuna is a local-first software engineering workbench for individual developers. A Tauri desktop client controls an independent Rust runtime that supervises Pi RPC workers, records durable evidence, and enforces approvals before implementation. The developer controls merging.

This repository is being established as an executable foundation. The design documents describe the intended product; they are not evidence that the complete workflow, desktop distribution, provider integration, or full v1 has shipped. Deterministic fixture tests and real provider smoke tests have different purposes and must be reported separately.

The runtime provides repository import, supervised baselines, immutable plan approval and scoped writers in owned worktrees. The M3 implementation adds candidate-bound checks, fresh read-only reviewer sessions and bounded manual/optional automatic repair, with durable evidence and recovery. Deterministic integration and browser checks pass; genuine-provider M3 acceptance and native desktop/platform proof remain open. Publication is not implemented. The desktop shell connects to a separately started service.

**Repository decision, October 26, 2026:** [Luuna Missions](https://github.com/luuna-ai/luuna-missions), [Luuna Atlas](https://github.com/luuna-ai/luuna-atlas), and [Luuna Beacon](https://github.com/luuna-ai/luuna-beacon) are maintained in the `luuna-ai` organization and consumed through pinned Git submodules at the existing `components/` paths. Run `npm run components:setup` after cloning. See [ADR 0012](docs/decisions/0012-independent-component-repositories.md).

## Start here

1. Read [the current architecture](docs/CURRENT-ARCHITECTURE.md) for the decisions that govern new work.
2. Read [AGENTS.md](AGENTS.md) and [CONTRIBUTING.md](CONTRIBUTING.md) before changing the repository.
3. Use [AI-START-HERE.md](AI-START-HERE.md) for the implementation handoff and milestone exit criteria.
4. Consult the [decision records](docs/decisions/) when a historical proposal disagrees with the current architecture.
5. Use the [implementation roadmap](docs/IMPLEMENTATION-ROADMAP.md) for the next bounded assignment, milestone exits, and planning estimates.

M0–M2 have recorded Linux evidence. The next acceptance work is the genuine-provider M3 feature/check/review/repair flow and native-platform proof; see the handoff and status report. Fixture sessions establish deterministic failure behavior, not provider compatibility. Adapters, publication and client parity follow those gates.

## Repository map

| Path | Purpose |
| --- | --- |
| `apps/desktop/` | Tauri and React/TypeScript desktop foundation |
| `crates/` | Rust domain, storage, service, and engine boundaries |
| `schemas/` | Versioned Luuna domain contracts, distinct from Pi protocol records |
| `fixtures/`, `tests/` | Deterministic protocol, contract, and integration checks |
| `agents/` | Process-role contracts; these do not override runtime policy |
| `docs/CURRENT-ARCHITECTURE.md` | Authoritative technical baseline |
| `docs/decisions/` | Architecture decisions and explicit release requirements |
| `components/missions/` | Luuna Missions, the maintained Seeds-derived task CLI |
| `components/atlas/` | Luuna Atlas, the maintained Mulch-derived knowledge CLI |
| `components/beacon/` | Luuna Beacon, the maintained Canopy-derived prompt CLI |

Only paths present in the checkout are executable components. The architecture includes later boundaries that need not be empty crates or placeholder packages today.

## Development

Use Rust 1.90.0, as pinned in `rust-toolchain.toml`, and committed lockfiles. Rust validation starts with:

```sh
cargo fmt --all -- --check
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
```

Consult [CONTRIBUTING.md](CONTRIBUTING.md) for frontend and service commands. Real Pi runs require a tested Pi version and an existing provider/model configuration. Keep credentials in private user configuration or environment bindings; never place them in fixtures or source control.

For frontend and schema checks, use Node 24.19.0 (pinned in `.nvmrc`) or newer and npm 11.9.0:

```sh
npm ci --ignore-scripts
npm test
npm run typecheck
npm run build
```

With a private `LUUNA_API_TOKEN` of at least 32 bytes set, start the service with `cargo run --locked -p luuna-runtime`. Run `npm run dev` for the browser development client or `npm run desktop` for the native shell after installing Tauri prerequisites. Use the same private token in the client's connection form. See the [runtime configuration](CONTRIBUTING.md#run-the-connectivity-demo) for Pi/provider prerequisites and current limitations.

Linux cloud checks cannot establish Mac or Windows installation support. A frontend build cannot establish a working Tauri distribution. Report the commands and platforms actually exercised.

## Homebrew packaging status

A source-build formula draft for the Luuna command-line suite is staged in
[`packaging/homebrew-luuna`](packaging/homebrew-luuna/README.md). It is not a
published tap, so there is no working `brew tap` or `brew install` command yet.
The draft installs the runtime, terminal client and pinned Missions, Atlas and
Beacon CLIs; it does not install either desktop app. See the draft's desktop
release matrix for the artifacts and platform evidence required before casks
can be added.

## Terminal workbench

Build the terminal client with `npm run build:tui` (Rust 1.90.0, locked dependencies),
then run `npm run tui` with a local runtime and private `LUUNA_API_TOKEN`. A missing
token is entered without echo. `npm run tui -- help`, `npm run tui -- init` (guided
project setup) and named commands such as `npm run tui -- projects --json` run the
same executable.

The interactive application uses the DOS palette and component layout from
`luuna-ai`; its actions run in process, without Node. The checkout launcher
resolves `target/debug/luuna-tui.exe` on Windows, or `luuna-tui` on Unix, and honors
an explicit `CARGO_TARGET_DIR`. Installed Windows distributions carry their own
native executable. See [the terminal client](crates/luuna-tui/README.md),
[ADR 0029](docs/decisions/0029-rust-terminal-workbench.md) and
[ADR 0037](docs/decisions/0037-single-rust-terminal-client.md).
The [native terminal verification guide](docs/TERMINAL-VERIFICATION.md) describes
the pinned build, real terminal audit and the limits of fixture evidence.

## Orbit and the project sandbox

`lu` is the short name of `luuna` (`bin/lu`, `bin/lu.cmd`). Orbit acts on every
registered repository of a project at once; the sandbox opens a shell or agent
that can reach only those repositories, enforced by the operating system
([ADR 0036](docs/decisions/0036-orbit-and-project-sandbox.md)):

```sh
lu orbit luuna                                # branch, upstream, changes, recipes per repository
lu orbit-exec luuna "git status --short" --parallel
lu orbit-run luuna test                       # a confirmed manifest recipe where it is defined
lu orbit-sync luuna                           # clone missing checkouts, fetch present ones
lu sbx luuna                                  # a shell that sees only the project's repositories
lu sbx luuna -- pi                            # or any agent CLI; --network none, --plan-only
```

Build the launcher once with `cargo build --locked -p luuna-sandbox`. It uses
bubblewrap on Linux, `sandbox-exec` on macOS and an AppContainer on Windows; the
desktop's Orbit view shows the same status, commands and sandbox plan. Orbit
results are the developer's commands, never check evidence.

## Design references and licensing

The [desktop/Pi RPC proposal](LUUNA-Desktop-Pi-RPC-Full-Development-Proposal.md) supplies detailed rationale. The thirteen numbered chapters under `docs/`, [the complete reference](LUUNA-COMPLETE-REFERENCE.md), diagrams, examples, HTML, and PDF remain historical source material. Their original browser/TUI delivery order and conflicting API examples are superseded by the current architecture and decisions. Browser/TUI capability parity remains a full-v1 obligation.

[FOUNDATION-STATUS.md](docs/FOUNDATION-STATUS.md) records the executable foundation checks and remaining milestones; [MVP-WALKTHROUGH.md](docs/MVP-WALKTHROUGH.md) is the end-to-end single-repository workflow and [MVP-EVIDENCE.md](docs/MVP-EVIDENCE.md) names what that MVP proves and omits. [VALIDATION-REPORT.md](VALIDATION-REPORT.md) concerns the original documentation pack.

Luuna uses the [MIT license](LICENSE). The branded component distributions preserve upstream copyright and MIT notices; see [THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md). They are maintained in this monorepo until a verified repository extraction is useful; their presence does not establish independently hosted or published packages. See [SECURITY.md](SECURITY.md) for handling sensitive findings.
