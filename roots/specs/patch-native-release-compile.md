# Patch: fix E0716 in `process_dimensions` so native release can build

Scope expansion of `specs/chore-native-release.md` (run `9f32a7c1`, task C-roots). That spec's "Known blocker" section names this failure. Until it is fixed, `cargo test`, `cargo build --release`, `scripts/release/package.sh` and `.github/workflows/release.yml` cannot succeed, so this runtime-source edit is explicitly justified.

## Problem

`crates/roots-cli/src/interactive/terminal.rs:569`, unix `process_dimensions`:

```rust
let mut values = String::from_utf8_lossy(&result.stdout)
    .split_whitespace()
    .filter_map(|part| part.parse::<usize>().ok());
Some((values.next()?, values.next()?))
```

The temporary `Cow<str>` from `String::from_utf8_lossy` is dropped at the end of the `let` statement. The `split_whitespace()` iterator stored in `values` still borrows it, which causes E0716 with the pinned Rust 1.99.1.

## Fix (minimal, behavior-preserving)

Bind the `Cow` to a local inside the same closure, then iterate over it:

```rust
.and_then(|result| {
    let text = String::from_utf8_lossy(&result.stdout);
    let mut values = text
        .split_whitespace()
        .filter_map(|part| part.parse::<usize>().ok());
    Some((values.next()?, values.next()?))
});
```

Parsing, the `(rows, columns)` order, the swap to `(columns, rows)`, the `env_dimensions` fallback and the stty invocation all stay exactly the same. There is no redesign, no helper extraction, and no change to the windows or other-platform variants.

## Regression test

No new test. The function shells out to `stty size` with inherited stdin, so a direct unit test would need either a refactor (extracting a parser) or a TTY. Both are out of scope for a minimal patch. The regression guard is that `cargo build`/`cargo test --workspace --locked` compile this unix path on the macOS and Linux CI and release runners. If a reviewer wants an explicit test later, it should come as a separate change that extracts a pure `parse_stty_size(&[u8]) -> Option<(usize, usize)>`.

## Out of scope

- `Cargo.toml`, `Cargo.lock`, `rust-toolchain.toml`, `ci.yml`, release scripts and workflow.
- Toolchain or global dependency downloads, publication, tags, commits and pushes.

## Step by Step Tasks

1. Re-read `crates/roots-cli/src/interactive/terminal.rs` lines 559–575 and confirm the code still matches the snippet above.
2. Replace the closure body at lines 568–573 with the fix above. That means adding one `let text = ...;` line and changing the `values` initializer to borrow `text`. Touch no other lines.
3. Run `cargo fmt --all -- --check` and fix formatting in the edited lines only if needed.
4. Run the validation commands below using the locally installed pinned toolchain only. If `1.99.1` is not installed, stop and report; do not install it.
5. In `specs/chore-native-release.md`, update the "Known blocker" section to say it is resolved by `specs/patch-native-release-compile.md`. Keep the original description for history.
6. Leave all changes uncommitted for review.

## Validation Commands

```sh
# Pinned toolchain present locally (must not trigger a download)
rustup run 1.99.1 rustc --version

# Formatting, lint, compile, tests
cargo fmt --all -- --check
cargo clippy --workspace --locked --all-targets -- -D warnings
cargo build --workspace --locked
cargo test --workspace --locked

# Release script offline tests
sh scripts/release/test-release-scripts.sh

# Real archive build and check for the host target
OUT="$(mktemp -d)"
sh scripts/release/package.sh --version 0.0.0-local --out "$OUT"
sh scripts/release/check-archive.sh "$OUT"/*.tar.gz
(cd "$OUT" && shasum -a 256 -c SHA256SUMS)

# Diff is confined to the one function plus the spec note
git diff --stat -- crates/
git diff -- crates/roots-cli/src/interactive/terminal.rs

# No publication steps were enabled
grep -nE 'gh release|action-gh-release|npm publish|cargo publish|attest-build-provenance' .github/workflows/release.yml
```

Expected results:
- Every command exits 0.
- `git diff --stat -- crates/` shows only `terminal.rs`, with about 2 lines changed.
- The grep prints only commented lines.
- `check-archive.sh` runs `--version` and `--help` through both `luuna-roots` and `lrt` with identical output.

SPEC: specs/patch-native-release-compile.md
