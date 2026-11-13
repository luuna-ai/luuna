# Contributing to Luuna

Start with [the current architecture](docs/CURRENT-ARCHITECTURE.md) and [AGENTS.md](AGENTS.md). Luuna is establishing an executable foundation; roadmap descriptions are requirements and proposals, not completed features.

## Scope a change

Use a bounded behavior with a clear exit criterion. M0 establishes contracts, durable state, deterministic approvals, and meaningful rejection tests. M1 proves a real Pi RPC session, API streaming, cancellation, errors, and persistence. Describe limitations honestly and keep later orchestration or distribution work separate.

Preserve user changes, historical design references, upstream license notices, and existing behavior. Keep credentials and runtime data outside version control. Avoid modifying component internals while building Luuna adapters. Do not run nested package hook installers against the root repository.

## Set up and check

There is no CI/CD in this repository ([ADR 0041](docs/decisions/0041-no-repository-ci.md)). Nothing runs on push or on a pull request; the checks below are run by the contributor, on the hosts available, and the results are recorded with the change. Name what was not run.

Use Rust 1.90.0 from `rust-toolchain.toml` and committed lockfiles. Rust tests also need a host `git` (2.40 or newer) on `PATH` for the read-only repository inspection fixtures. Run from the repository root:

```sh
cargo fmt --all -- --check
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
```

Use the desktop package's pinned dependencies and scripts for TypeScript checking, frontend builds, and Tauri development. Native Tauri compilation needs platform libraries and tools; a successful web build does not prove that the native shell builds or launches.

The frontend workspace uses Node 24.19.0 (pinned in `.nvmrc`) or newer and npm 11.9.0. Run from the repository root:

```sh
npm ci --ignore-scripts
npm test
npm run typecheck
npm run build
```

Run `npm run components:setup` with Bun 1.4.3 after cloning, or clone with `--recurse-submodules` and then install frozen component dependencies. `npm run components:verify` checks exact pins; `npm run test:components` exercises real CLI JSON operations on disposable stores. Until the migration changes of [ADR 0044](docs/decisions/0044-one-repository-and-agent-shipping.md) bring them into this repository, the component sources live in their public `luuna-ai` repositories and are consumed as these pinned submodules.

Luuna Roots lives in `roots/`. Its library, roots-core, is a member of the root Cargo workspace, so the root `cargo fmt`, `cargo clippy` and `cargo test` commands build, lint and test it. `roots/Cargo.toml` remains the workspace of roots-cli, with `roots/Cargo.lock`. Run the Roots workspace checks from the repository root:

```sh
RUSTUP_TOOLCHAIN=1.90.0 cargo fmt --manifest-path roots/Cargo.toml --all -- --check
RUSTUP_TOOLCHAIN=1.90.0 cargo clippy --manifest-path roots/Cargo.toml --workspace --all-targets --all-features --locked -- -D warnings
RUSTUP_TOOLCHAIN=1.90.0 cargo test --manifest-path roots/Cargo.toml --workspace --all-features --locked
RUSTUP_TOOLCHAIN=1.90.0 cargo test --manifest-path roots/Cargo.toml -p roots-cli --test compat --locked
RUSTUP_TOOLCHAIN=1.90.0 cargo build --manifest-path roots/Cargo.toml --workspace --release --locked
```

The `compat` and `behavior_fixtures` tests of `roots-cli` run only on Windows; on other hosts they compile to zero tests. `roots/rust-toolchain.toml` pins 1.99.1, which these commands do not use: they set `RUSTUP_TOOLCHAIN=1.90.0`, Luuna's pinned toolchain, and results on 1.99.1 are not verified.

The remaining checks, each run where its prerequisites exist: `node scripts/check-hygiene.mjs` (no tracked session histories, auth stores, real `.env` files or dependencies); the terminal client against a built runtime, `npm run test:tui-terminal` and `node scripts/tui-interactive-audit.mjs --runtime <path> --components-root .` (tmux on Linux); the native desktop shell, `cargo check --locked --manifest-path apps/desktop/src-tauri/Cargo.toml`, on macOS and Windows.

`npm test` includes component pin rejection tests, JSON-schema fixtures and API-client tests. These are separate from the Rust tests and component CLI suites. Each maintained CLI has its own Bun lockfile and package scripts; follow its package README rather than running its hook installer against the root repository.

Run the schema/fixture suite when changing contracts, state transitions, or protocol handling. Keep fixtures synthetic. A deterministic Pi fixture validates framing and lifecycle logic; a real provider smoke test validates the pinned engine/model profile. Provider credentials belong in private configuration or environment bindings, never in source or test logs. Mark provider/platform checks as unavailable when their prerequisites are absent.

The installer in `packaging/installer/` has its own checks. They need only a POSIX shell and standard utilities, never use the network or the real home, and are not part of `npm test`:

```sh
sh packaging/installer/tests/run.sh
TEST_SHELL=dash sh packaging/installer/tests/run.sh
TEST_SHELL=bash sh packaging/installer/tests/run.sh
sh packaging/installer/tests/check-manifests.sh
shellcheck -s sh packaging/installer/install.sh packaging/installer/install-manifest.sh packaging/installer/tests/*.sh
```

For the prepared Linux cloud machine, `bash scripts/setup-cloud.sh` reproduces the pinned Rust/Bun/Pi tools, frozen component installs, npm workspace, and Rust build. Tool caches live under `/workspace/.tools` and `/workspace/.cache`, outside the checkout. Native desktop system libraries and provider credentials are separate prerequisites.

Run inherited component suites one at a time, separately from builds. On this cloud host, use `TMPDIR=/var/tmp bun test --timeout 30000` from a component directory because the platform's `/tmp/.git` marker interferes with upstream hook fixtures. Keep package-specific assertions intact when investigating failures.

## Run the connectivity demo

The runtime serves the tool-free demo profile described here as well as repository import, baselines, approved writers, candidate checks and review; writers and checks run natively with your host permissions, and nothing publishes or merges changes. Start the service separately from the desktop so that closing the client does not terminate it.

Install the pinned Pi package `@earendil-works/pi-coding-agent@1.3.0` in a private local tool location and make its executable available. See [Pi's official repository](https://github.com/earendil-works/pi) for provider configuration. Its package executable is `dist/bundle/cli.js`. Install ripgrep (`rg`) and fd (`fd`, or Debian's `fdfind`) on PATH as well: Luuna runs Pi offline, so Pi cannot download them, and writers and reviewers are not offered the `grep` or `find` tools without them. The service launches a fresh isolated tool-free session: it does not read the developer's ordinary Pi configuration, OAuth session, extensions, skills, or project instructions. Supply supported provider credentials securely through environment bindings. The cloud network policy must also allow the selected provider's HTTPS destination. Allow `api.anthropic.com` or `api.openai.com`, as appropriate, in environment settings before testing access.

| Setting | Purpose and default |
| --- | --- |
| `LUUNA_API_TOKEN` | Required private bearer token of at least 32 visible ASCII characters, with no spaces or line breaks; provide the same token to the client |
| `LUUNA_PI_COMMAND` | Pi executable, default `pi`; may be an absolute path. On Windows an npm `pi.cmd` shim found through PATH and PATHEXT is started as `node.exe` with its `dist/bundle/cli.js`; other batch launchers are refused, so point this at `node.exe` and pass `cli.js` in `LUUNA_PI_ARGS_JSON` for any other layout |
| `LUUNA_PI_ARGS_JSON` | Optional JSON array of launcher arguments, useful with a direct Node executable |
| `LUUNA_PI_PROVIDER`, `LUUNA_PI_MODEL` | Explicit provider/model defaults; may also be selected per run in the client |
| `LUUNA_PI_<PROVIDER>_API_KEY` | Optional alias for any supported provider key, such as `LUUNA_PI_ANTHROPIC_API_KEY`, for hosts that reserve the plain name; mapped to Pi's standard variable with precedence over a direct key |
| `HTTP_PROXY`, `HTTPS_PROXY`, `ALL_PROXY`, `NO_PROXY` | Preserved for provider routing, including lowercase variants and `NODE_USE_ENV_PROXY`; TLS verification remains enabled |
| Provider-specific API key | Required for the selected provider; use private environment settings |
| `LUUNA_DATA_DIR` | Private local state/artifacts directory, default `.luuna`; keep it short on Windows (for example `%USERPROFILE%\.luuna`) because managed worktrees below it count toward the 260-character path limit |
| `LUUNA_PORT` | Loopback service port, default `4242` |
| `LUUNA_RUN_TIMEOUT_SECONDS` | Positive run deadline, default `600` |

Supported inherited key names currently include `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `GEMINI_API_KEY`, `GOOGLE_API_KEY`, `MISTRAL_API_KEY`, `GROQ_API_KEY`, `XAI_API_KEY`, and `OPENROUTER_API_KEY`. An existing OAuth login alone does not configure the default isolated demo. For explicitly opted-in OpenAI OAuth, use `tools/pi/oauth-launch.mjs` as Node's launcher argument, followed by `--luuna-auth-file <absolute-private-auth.json>` and `--luuna-pi-root <absolute-pinned-package-directory>`, and set `LUUNA_PI_AUTH_FILE` to the same auth file for diagnostics and redaction. This shares only the selected OpenAI grant and Pi's locked refresh, with no token copies or API-key fallback; see [ADR 0049](docs/decisions/0049-opt-in-openai-oauth.md). Never copy credentials into repository configuration or diagnostic output.

With the necessary private settings present, launch from the repository root:

```sh
cargo run --locked -p luuna-runtime
```

In another terminal, run `npm run dev` and open `http://127.0.0.1:1420`, or use `npm run desktop` after installing the native [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/). Connect to `http://127.0.0.1:4242` with the private token. Select an explicit provider/model, send a harmless prompt, inspect the durable event history, and try cancellation. The service offers cursor replay and SSE; the initial UI uses replay polling for reconnection. Closing the UI is a disconnect. Stopping the service interrupts owned work; restart reconciles incomplete attempts rather than claiming completion.

Without a provider key, fixture/protocol checks and the UI connection remain useful, but no successful genuine model run can be claimed. A no-credential error is evidence for the failure path only.

With a built runtime and the private settings exported, `node scripts/m1-smoke.mjs --mode real --provider <provider> --model <model>` runs the M1 checklist end to end and writes a sanitized report (no token or credential values). `--mode fixture` exercises the harness itself against the synthetic fixture and is labeled as such. `node scripts/m3-smoke.mjs` does the same for the M3 checklist (approved writer, required checks, fresh independent review, seeded failure and repair, blocking findings, cancellation, hard-kill recovery) on a disposable Python fixture repository; it needs Python 3 and Git on the host and `--pi`/`--pi-args-json` when Pi is launched through Node. `node scripts/mvp-ui-smoke.mjs` drives the browser client (headless Chromium through `playwright-core`, the Vite dev server on port 1420) against a built runtime and the synthetic fixture through the [MVP walkthrough](docs/MVP-WALKTHROUGH.md), including restart and shutdown recovery, refused actions, keyboard operation and an accessibility audit; it is client and lifecycle evidence only. `node scripts/backup-recovery-drill.mjs` rehearses the backup, loss, restore and newer-schema refusal procedure of ADR 0016 against a disposable data directory with the real maintenance commands, then runs the storage suite's upgrade drill. `node scripts/tui-smoke.mjs` drives the same walkthrough through the terminal client's non-interactive mode. Keep every report outside Git.

On Windows, `powershell -NoProfile -File scripts/package-windows.ps1` builds an NSIS development installer carrying the desktop, the release runtime, the helper scripts and the pinned Pi tree (Node.js, Git and Python stay host prerequisites; `luuna-runtime doctor` reports them). It is unsigned unless `-CertificateThumbprint` is supplied, and an unsigned installer is a development distribution, not release evidence.

## Review and evidence

Explain the observable behavior changed, why it matters, and commands actually run. Include meaningful negative cases for authorization, stale state, interrupted work, or protocol failure when those boundaries change. Do not add tests that only restate trivial implementation details.

Review must examine requirements, the diff, current code, and check evidence independently of an implementation chat. Runtime changes must preserve exact-plan approval, candidate-bound checks, private artifacts, bounded retries, and, in a developer's own repositories, human-controlled merging: Luuna's runtime has no automatic merge operation in v1. A skipped or failed required check is not passing evidence.

Record material architecture changes in `docs/decisions/` and update `docs/CURRENT-ARCHITECTURE.md`. Package publication requires a deliberate release and verified credentials. New original contributions are MIT licensed; preserve derived components' upstream notices. See [SECURITY.md](SECURITY.md) for sensitive findings.

Changes to this repository itself ship under [ADR 0044](docs/decisions/0044-one-repository-and-agent-shipping.md). During the migration, agents work under a plan audited against the owner's signed migration charter, review the changes, and push, open pull requests and merge only through the ship gate. Agent reviews together with the local check evidence described above are the only gate on a merge until the CI pipeline planned for after the migration exists.
