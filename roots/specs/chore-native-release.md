# Chore: native release archives for Luuna Roots (C-roots)

This spec is scoped from the hub plan `luuna-ai-os/specs/feature-native-cli-distribution.md` (run `9f32a7c1`). It covers task **C-roots** only. Hub tasks (H0–H7) and the other component tasks belong to other repositories and are not touched here.

## Goal

Give `luuna-roots` a reproducible, checksum-verified native packaging path for darwin/linux × arm64/amd64. The output is uploaded only as workflow artifacts and nothing is published.

## Public executable set

The archive contains `bin/luuna-roots` (primary) and `bin/lrt` (alias), and nothing else. Both are built from `crates/roots-cli/src/bin/{luuna-roots,lrt}.rs`. Each calls `roots_cli::run(args[1..])` and does not branch on `argv[0]`, so the alias preserves arguments and behavior.

## Changes

| File | Purpose |
| --- | --- |
| `scripts/release/package.sh` | POSIX packaging script. It validates `--version` as semver (no `v`, no build metadata, ≤64 chars) and requires `--out` to be absent or an empty non-symlink directory. `--target` is optional and allowlisted. It then runs `cargo build --release --locked -p roots-cli --target <triple>` and writes a deterministic ustar archive (two file entries, `0755`, uid/gid 0, fixed mtime, `gzip -n`), `SHA256SUMS`, `provenance.json`, `LICENSE` and `UPSTREAM.md`. |
| `scripts/release/check-archive.sh` | Checks the name, the sibling `SHA256SUMS`, and that the entry list is exactly the two names, all before any extraction. Every entry must be a regular `-rwxr-xr-x` file, with no links or directories. It then extracts privately and re-checks. On a host-matching archive it runs `--version`/`--help`/`--version --json` through both names and requires identical output. |
| `scripts/release/test-release-scripts.sh` | Offline tests using stub archives. They cover argument validation, a good archive, checksum mismatch, missing alias, extra entry, directory entry, non-exec and world-writable modes, symlink, hardlink, traversal, absolute path, bad platform name, corrupt archive, and CLI misuse. |
| `.github/workflows/release.yml` | `workflow_dispatch` only, with a `version` input. Sets `permissions: contents: read`. Native matrix: `macos-14` (arm64), `macos-15-intel` (amd64), `ubuntu-latest`, `ubuntu-24.04-arm`. Steps: host-triple check, `cargo test --workspace --locked`, script tests, package, check, then `upload-artifact`. The attestation step is commented out as an owner gate. |
| `docs/RELEASING.md` | Local dry run, version semantics, attribution, the owner gates, and the limits of what a run proves. |

Existing CI (`ci.yml`), runtime sources, `Cargo.toml` and `Cargo.lock` are unchanged. `.tac/runs/` was already ignored.

## Deviations from the hub plan

- **Runners.** `macos-13` is retired, so darwin/amd64 uses `macos-15-intel`. darwin/arm64 uses `macos-14`.
- **Versions.** The version is not injected into the binary. `roots_core::VERSION` is `env!("CARGO_PKG_VERSION")`, a compile-time workspace value, and no runtime override symbol exists, so none was invented. Provenance records `archive_version` and `internal_version` separately.
- **Attribution.** `LICENSE` and `UPSTREAM.md` ship next to the archive, not inside it, so the strict `bin/` allowlist holds.

## Validation

- `sh scripts/release/test-release-scripts.sh`
- `cargo test --workspace --locked`
- `sh scripts/release/package.sh --version 0.0.0-local --out "$(mktemp -d)"`, then `sh scripts/release/check-archive.sh <archive>`
- `grep -nE 'gh release|action-gh-release|npm publish|cargo publish|attest-build-provenance' .github/workflows/release.yml` must print only commented lines.

## Known blocker (outside this chore's scope)

At commit `6c4b7a9`, `roots-cli` does not compile on unix with the pinned Rust 1.99.1. The error is E0716 at `crates/roots-cli/src/interactive/terminal.rs:569`: a temporary `String::from_utf8_lossy(..)` is borrowed by `split_whitespace()` after the temporary is dropped. Because of this, `cargo test`, `cargo build --release`, `package.sh` and `release.yml` all fail until the runtime source is fixed. A fix is to bind the `Cow` to a `let` first. Runtime sources are out of scope for this chore, so the fix is left to a separate change.

**Status: resolved by `specs/patch-native-release-compile.md`.** It binds the `Cow` to a local before `split_whitespace()`. With pinned 1.99.1 on darwin-arm64, these now pass in the working tree: `cargo fmt --check`, `cargo build --workspace --locked`, `cargo test --workspace --locked`, `test-release-scripts.sh`, `package.sh`, `check-archive.sh` and `shasum -c`.

**Remaining blocker (newly exposed, out of scope for both specs):** `cargo clippy --workspace --locked --all-targets -- -D warnings` still fails. These lints were hidden while E0716 stopped compilation:
- `function_casts_as_integer` at `crates/roots-cli/src/interactive/terminal.rs:291-293`, for `quit_signal as usize` / `resize_signal as usize`. The suggested fix is `as *const () as usize`. `cargo build` reports the same three lints as warnings.
- `clippy::needless_return` at `crates/roots-cli/src/interactive/think_session.rs:1007`.

CI's clippy step stays red until a separate change fixes them.

**Status (2026-10-23): resolved by `specs/patch-native-release-clippy.md`.** Both lints are fixed without changing behavior. These checks were run locally with the pinned `1.99.1` toolchain on darwin/arm64 and passed: `cargo fmt --all -- --check`, `cargo clippy --workspace --locked --all-targets --all-features -- -D warnings`, `cargo build --workspace --locked` (0 `function_casts_as_integer` warnings), `cargo test --workspace --locked --all-features`, `sh scripts/release/test-release-scripts.sh`, `package.sh --version 0.0.0-local` plus `check-archive.sh` on the host archive, and `shasum -a 256 -c SHA256SUMS`. The CI workflow itself was not run.

## Toolchain pin limits

`rust-toolchain.toml` pins `1.99.1`. The workflow installs the same version. Local runs resolve the pin through rustup. A build with any other toolchain is not pinned, but `provenance.json` records the actual `rustc`/`cargo` versions either way.
