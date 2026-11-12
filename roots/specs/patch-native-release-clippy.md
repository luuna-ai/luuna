# Patch: clear pre-existing lints exposed by the native build

Scope expansion of `specs/chore-native-release.md` (run `9f32a7c1`, task C-roots), following `specs/patch-native-release-compile.md`. The "Remaining blocker" section of the chore spec names these lints. They were hidden while E0716 stopped compilation. Now they make `cargo clippy ... -D warnings` fail, so full checks and CI's clippy step stay red. This small runtime-source edit is explicitly approved to unblock those checks. Behavior must stay exactly the same.

## Problems

1. `function_casts_as_integer` (rustc lint, warn by default, so `-D warnings` makes it an error) at `crates/roots-cli/src/interactive/terminal.rs:291-293`, inside `install_signal_handlers` (unix):

   ```rust
   signal(SIGHUP, quit_signal as usize),
   signal(SIGTERM, quit_signal as usize),
   signal(SIGWINCH, resize_signal as usize),
   ```

   `quit_signal` and `resize_signal` are `extern "C" fn(i32)` items (`terminal.rs:164`, `:169`). The FFI `signal` is declared as `fn signal(signum: i32, handler: usize) -> usize` (`terminal.rs:175`).

2. `clippy::needless_return` at `crates/roots-cli/src/interactive/think_session.rs:1007`, in `exit_status_parts`:

   ```rust
   #[cfg(unix)]
   {
       use std::os::unix::process::ExitStatusExt;
       return (status.code(), status.signal().map(signal_name));
   }
   #[cfg(not(unix))]
   {
       (status.code(), None)
   }
   ```

## Fix (minimal, behavior-preserving)

1. `terminal.rs:291-293`: cast each function item through a thin data pointer before converting it to an integer, as the lint suggests:

   ```rust
   signal(SIGHUP, quit_signal as *const () as usize),
   signal(SIGTERM, quit_signal as *const () as usize),
   signal(SIGWINCH, resize_signal as *const () as usize),
   ```

   The function address and the resulting `usize` are the same as before. This only makes the cast explicit. Do not change the FFI declaration, the `usize::MAX` (`SIG_ERR`) checks, `SignalHandlers`, `restore_signal_handlers`, or the handler functions.

2. `think_session.rs:1007`: remove `return` and the trailing `;` so the `#[cfg(unix)]` block ends in a tail expression:

   ```rust
   #[cfg(unix)]
   {
       use std::os::unix::process::ExitStatusExt;
       (status.code(), status.signal().map(signal_name))
   }
   ```

   After cfg stripping, exactly one block remains as the function's tail expression on each platform. The non-unix block already relies on this. The returned values do not change.

No other lines change. There is no refactor, no new `#[allow]`, and no lint configuration change.

## Regression test

No new test. Neither change alters observable behavior: the same address is passed to `signal(2)`, and the same tuple is returned. Testing signal installation would need process-level signal delivery, which is out of scope. The regression guard is `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings` in CI (`ci.yml`) and in the validation below, plus the existing workspace tests that cover `exit_status_parts`/`status_error` callers.

## Out of scope

- Any other lint, file, or refactor. Leave the existing, already-reviewed E0716 fix in `terminal.rs` (`process_dimensions`) unchanged.
- `Cargo.toml`, `Cargo.lock`, `rust-toolchain.toml`, `ci.yml`, release scripts and workflow, docs.
- Toolchain or global dependency installs, `.tac` state or logs, publication, tags, commits, pushes, PRs.

## Step by Step Tasks

1. Re-read `terminal.rs` lines 160–302 and `think_session.rs` lines 1003–1013, and confirm they match the snippets above. If they have drifted, stop and report.
2. Use Edit to apply fix 1 to the three `signal(...)` argument casts in `terminal.rs`.
3. Use Edit to apply fix 2 to `think_session.rs:1007`.
4. Run `cargo fmt --all -- --check`. If it fails, fix formatting in the edited lines only.
5. Run the validation commands below with the locally installed pinned toolchain only. If `1.99.1` is missing, stop and report; do not install it. If clippy reports any lint other than these two, do not fix it here. Report it as a remaining blocker. Make at most three repair attempts for any single failure.
6. In `specs/chore-native-release.md`, under "Remaining blocker", add a status line saying it is resolved by `specs/patch-native-release-clippy.md`. Keep the original text for history. Record only the checks that actually passed, and do not list a check as passing unless it was run.
7. Leave all changes uncommitted for review.

## Validation Commands

```sh
# Pinned toolchain present locally (must not trigger a download)
rustup run 1.99.1 rustc --version

# Formatting, lint, compile, tests
cargo fmt --all -- --check
cargo clippy --workspace --locked --all-targets --all-features -- -D warnings
cargo build --workspace --locked 2>&1 | grep -c 'function_casts_as_integer' || true   # expect 0
cargo test --workspace --locked --all-features

# Release script offline tests
sh scripts/release/test-release-scripts.sh

# Real archive build and check for the host target
OUT="$(mktemp -d)"
sh scripts/release/package.sh --version 0.0.0-local --out "$OUT"
sh scripts/release/check-archive.sh "$OUT"/*.tar.gz
(cd "$OUT" && shasum -a 256 -c SHA256SUMS)

# Diff confined to the two lint sites (plus the earlier E0716 fix in terminal.rs)
git diff --stat -- crates/
git diff -- crates/roots-cli/src/interactive/think_session.rs
git diff -- crates/roots-cli/src/interactive/terminal.rs

# No publication steps were enabled
grep -nE 'gh release|action-gh-release|npm publish|cargo publish|attest-build-provenance' .github/workflows/release.yml
```

Expected results:
- Every command exits 0, and the build emits no `function_casts_as_integer` warnings.
- `git diff --stat -- crates/` lists only `terminal.rs` (the E0716 lines plus 3 changed cast lines) and `think_session.rs` (1 changed line).
- The grep prints only commented lines.
- `check-archive.sh` runs `--version` and `--help` through both `luuna-roots` and `lrt` with identical output.

SPEC: specs/patch-native-release-clippy.md
