# Releasing Luuna Roots native archives

This repository builds native archives for the public Luuna suite installer. **Nothing here publishes anything.** Tags, GitHub Releases, crates.io, npm, signing and attestations are owner decisions. They are listed under [Owner gates](#owner-gates).

## What an archive contains

`luuna-roots_<version>_<os>_<arch>.tar.gz` holds exactly two entries. Both are regular files with mode `0755`, owned `0/0`, and the archive has no directory or link entries:

- `bin/luuna-roots`, the primary executable
- `bin/lrt`, the public alias

`lrt` is a separate binary built from the same `roots_cli::run` entry point. Neither binary branches on `argv[0]`, so both names give identical output for the same arguments. `check-archive.sh` verifies this on the host platform.

These files sit next to the archive but are **not** inside it, because the installer's archive allowlist is strict:

- `SHA256SUMS`
- `provenance.json`
- `LICENSE`, the MIT license with Jaymin West's original copyright
- `UPSTREAM.md`, the upstream attribution

Anyone redistributing the archive must ship `LICENSE` alongside it.

| Target triple | os/arch | Runner |
| --- | --- | --- |
| `aarch64-apple-darwin` | darwin/arm64 | `macos-14` |
| `x86_64-apple-darwin` | darwin/amd64 | `macos-15-intel` |
| `x86_64-unknown-linux-gnu` | linux/amd64 | `ubuntu-latest` |
| `aarch64-unknown-linux-gnu` | linux/arm64 | `ubuntu-24.04-arm` |

Each target is built natively on its own runner, so no cross toolchain is needed. The workflow fails if the runner's host triple differs from the target. Linux binaries are glibc-linked; musl and Alpine are not claimed. Windows is not a release target.

## Versions

- **Archive version**: the `--version` / workflow input. It names the archive and is recorded in `provenance.json` as `archive_version`.
- **Internal version**: what `luuna-roots --version` prints. It is compiled from `CARGO_PKG_VERSION`, which comes from `[workspace.package] version` in `Cargo.toml` and is currently `0.0.0`. The scripts cannot inject it. They record it as `internal_version`, and they never edit `Cargo.toml`. Bumping it so the two versions match is an owner release action.

## Dry run locally

Run these from the repository root:

```sh
cargo test --workspace --locked
sh scripts/release/test-release-scripts.sh          # offline script tests, no cargo
out=$(mktemp -d)
sh scripts/release/package.sh --version 0.0.0-local --out "$out"
sh scripts/release/check-archive.sh "$out"/luuna-roots_0.0.0-local_*.tar.gz
```

`package.sh` builds for the host triple by default. It respects `CARGO_TARGET_DIR` and refuses an `--out` directory that is non-empty, a symlink or a file. It sets both entries' mtime to 1980-01-01 00:00 UTC (`TZ=UTC touch`), so the host time zone does not change the archive bytes.

`check-archive.sh` works in this order:
1. Validates the name and checksum, and lists the archive. The sibling `SHA256SUMS` is **required**: the check fails if it is missing, a symlink or not a regular file, or if it does not list the archive exactly once with a 64-character lowercase hex digest that matches. `--no-exec` does not relax this. The listing must hold only the two expected regular `0755` files.
2. Extracts the archive into a private temporary directory.
3. If the archive matches the host platform, runs `--version`, `--help` and `--version --json` through both names.

`--no-exec` skips step 3.

The toolchain is pinned by `rust-toolchain.toml` (`1.99.1`). The workflow installs the same toolchain with `dtolnay/rust-toolchain`. Locally, rustup installs the pinned toolchain on first use if it is missing. To avoid that download, use an already-installed toolchain by setting `RUSTUP_TOOLCHAIN`. That build is then not the pinned toolchain, and `provenance.json` records the actual `rustc`/`cargo` versions.

## Workflow

`.github/workflows/release.yml` runs only through `workflow_dispatch`, with a `version` input. It has `permissions: contents: read`. On each runner it does the following:
1. Confirms the runner's host triple matches the target.
2. Runs `cargo test --workspace --locked`.
3. Runs the offline script tests.
4. Runs `package.sh`, then `check-archive.sh`.
5. Uploads the output directory with `actions/upload-artifact` (14-day retention).

There is no tag trigger, release step, registry publish, or attestation. The attestation step is present but commented out as an owner gate.

## Owner gates

1. Run `release.yml` via `workflow_dispatch` and download the four artifacts.
2. On each claimed platform, verify `SHA256SUMS`, re-run `check-archive.sh`, and review `provenance.json`. Confirm that `commit` is the intended commit and that `worktree_dirty` is `false`.
   - `worktree_dirty` is `true` when tracked files changed **or** untracked, non-ignored files exist, because an untracked file such as `.cargo/config.toml` or `build.rs` can change the build. `untracked_present` reports the untracked part on its own. Ignored files such as `target/` do not count.
   - `null` in `commit`, `worktree_dirty` or `untracked_present` means **unknown**: git was missing, the directory was not a repository, git refused an unsafe directory, or git failed. Never accept `null` as clean.
3. *(Owner)* Decide on macOS signing and notarization and on GitHub attestations (`id-token: write`, `attestations: write`). Until then, `provenance.json` is self-reported metadata, not a verifiable attestation.
4. *(Owner)* Bump `Cargo.toml` if the internal version should match. Then create the tag and the GitHub Release, and upload the archives together with `SHA256SUMS`, `LICENSE` and `UPSTREAM.md`.
5. Add the hub manifest (`luuna-ai-os/releases/<v>/manifest.tsv`) through a reviewed PR in the hub repository.
6. crates.io and npm publication remain separate, later decisions.

## What a passing local run does not prove

- It proves the host platform only. The other three targets are proven only when `release.yml` runs on GitHub runners.
- It does not prove behavior when a browser download is quarantined by Gatekeeper.
- It does not prove byte-identical archives across hosts. The offline tests check that the pinned mtime is the same epoch under different `TZ` values, but the compiled binaries themselves are not checked for reproducibility.
