# Roots Operations Runbook

Procedures only. For conventions see [AGENTS.md](AGENTS.md); for project direction use Missions and Roots records. Roots is pre-release (`0.0.0`): there is no crates.io publication, signing, or rollback workflow yet.

## 1. A quality gate or CI job fails

Run the failing command directly from the checkout:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo build --workspace --release --locked
```

- Formatting failure: run `cargo fmt --all`, inspect the diff, and rerun the check.
- Clippy failure: make the behavior-preserving code change; do not hide a warning with a broad lint suppression.
- Test failure: rerun the narrow package/test name first, then the workspace test. Compatibility failures require comparing stdout, stderr, exit status, and store snapshots; do not update snapshots to hide a behavioral regression.
- Locked build failure: inspect `Cargo.toml`, `Cargo.lock`, and the pinned toolchain together. Do not regenerate the lock casually.

For an intermittent terminal test, rerun the specific test repeatedly and wait for a screen/state predicate rather than adding arbitrary sleeps. The interactive runtime deliberately separates terminal handoffs, watcher polling, and background agent jobs.

## 2. A project's `.luuna/roots/` looks wrong

1. Run `luuna-roots verify` (exit 6 on validation errors). It reports boundary breaks, bad authors, graph invariant breaks, dangling references, malformed JSONL rows, and human files changed outside a recorded session.
2. **Merge conflict in `*.jsonl`:** `luuna-roots init` adds `.luuna/roots/*.jsonl merge=union` to `.gitattributes`. Keep both rows, then run `luuna-roots verify`; reads retain the last occurrence of duplicate records.
3. **A command waits on a lock:** a crashed process may have left `<file>.lock`. Stale locks are reclaimed after the configured timeout by the store. If it persists after confirming no Roots process owns it, remove only that exact lock file.
4. **Human prose changed outside a session:** if a human made the edit, commit it or run a `luuna-roots think` session. If an agent made it, restore the human file from Git and investigate the boundary failure.
5. Never hand-edit `graph.jsonl`, `proposals.jsonl`, or `events.jsonl` to change decisions. Use the CLI so each change has the required event.

## 3. Roll back a repository change

Roots never commits for the user. Revert the offending commit with `git revert <sha>`, run the full Cargo gate, and open a PR. Project Roots data is normal Git history; restore it with `git checkout <sha> -- .luuna/roots/` when that is the intended recovery.
