# Native release packaging: change summary

This change (run `9f32a7c1`, task C-roots) adds checksum-verified native archives of `luuna-roots` and its `lrt` alias for darwin/linux × arm64/amd64. **Nothing is published.** For operating details, see [RELEASING.md](RELEASING.md).

## What was added

| Path | Role |
| --- | --- |
| `scripts/release/package.sh` | Validates `--version` (semver, no `v`, no build metadata) and `--out` (absent or an empty, non-symlink directory). Accepts an optional allowlisted `--target`. Runs `cargo build --release --locked -p roots-cli` and writes a deterministic `luuna-roots_<version>_<os>_<arch>.tar.gz` containing only `bin/luuna-roots` and `bin/lrt`, with `0755` modes, uid/gid 0, a fixed mtime and `gzip -n`. Writes `SHA256SUMS`, `provenance.json`, `LICENSE` and `UPSTREAM.md` next to the archive. |
| `scripts/release/git-state.sh` | Sourced helper that records the HEAD commit and the dirty/untracked state. An unknown state is recorded as `null`, never as clean. |
| `scripts/release/check-archive.sh` | Before any extraction, verifies the name, the required sibling `SHA256SUMS` and the exact two-entry listing. Extracts into a private directory and checks again. On a host-matching archive, runs `--version`, `--help` and `--version --json` through both names and requires identical output. `--no-exec` skips only the run step. |
| `scripts/release/test-release-scripts.sh` | Offline tests with stub archives. They cover argument validation, checksum mismatch, a missing alias, extra/directory/link entries, bad modes, traversal and absolute paths, a corrupt archive and CLI misuse. |
| `.github/workflows/release.yml` | Triggered only by `workflow_dispatch` with a `version` input; `permissions: contents: read`. Builds natively on `macos-14`, `macos-15-intel`, `ubuntu-latest` and `ubuntu-24.04-arm`. Steps: host-triple check, `cargo test`, script tests, package, check, then `upload-artifact` only. |
| `docs/RELEASING.md` | Dry run, version semantics, attribution, owner gates and limits. |

`Cargo.toml`, `Cargo.lock`, `rust-toolchain.toml` (pinned `1.99.1`) and `ci.yml` are unchanged.

## Minimal pre-existing runtime patches

Commit `6c4b7a9` did not build on unix, so two separately specified, behavior-preserving patches were needed:

- **Compile** ([spec](../specs/patch-native-release-compile.md)): E0716 in `process_dimensions`, `crates/roots-cli/src/interactive/terminal.rs`. The `String::from_utf8_lossy` `Cow` is now bound to a local before `split_whitespace()`.
- **Lint** ([spec](../specs/patch-native-release-clippy.md)): this was exposed once the code compiled. The signal-handler casts in `terminal.rs` now go through `as *const () as usize` (`function_casts_as_integer`), and a `needless_return` was removed in `think_session.rs`. The FFI address and the return values are unchanged. No `#[allow]` or lint configuration was added.

The crate diff touches only these two files: 6 lines added and 5 removed.

## Validation and local proof

These checks passed locally with the pinned `1.99.1` toolchain on **darwin/arm64 only**:

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --locked --all-targets --all-features -- -D warnings`
- `cargo build --workspace --locked`, with no `function_casts_as_integer` warnings
- `cargo test --workspace --locked --all-features`
- `sh scripts/release/test-release-scripts.sh`
- `package.sh --version 0.0.0-local`, then `check-archive.sh` on the host archive and `shasum -a 256 -c SHA256SUMS`

**Not run:** `release.yml` and `ci.yml` on GitHub, and any darwin/amd64 or linux build. Those three targets are therefore unproven until the owner dispatches the workflow.

## No publishing

The workflow uploads workflow artifacts only. It has no tags, GitHub Releases, `cargo publish`, npm or signing step. The attestation step is commented out. To confirm, run `grep -nE 'gh release|action-gh-release|npm publish|cargo publish|attest-build-provenance' .github/workflows/release.yml`; it must print only commented lines.

## Owner gates

These decisions belong to the repository owner. Each is described under [Owner gates](RELEASING.md#owner-gates):

- Dispatching the workflow
- Tagging and creating GitHub Releases
- Enabling build-provenance attestations, which need `id-token: write` and `attestations: write`
- Signing and notarization
- Bumping `[workspace.package] version`

The binary's `--version` reports the internal `CARGO_PKG_VERSION` (currently `0.0.0`), not the archive version. Making the two match is also an owner action.

## Provenance is self-reported

`provenance.json` (schema `luuna-component-provenance/1`) records:

- `archive_version` and `internal_version`
- the commit, `worktree_dirty`, and the actual `rustc`/`cargo` versions

The build script writes this file itself. It is **not** an attestation or a signature, so it does not prove where or how the archive was built. See [What a passing local run does not prove](RELEASING.md#what-a-passing-local-run-does-not-prove).
