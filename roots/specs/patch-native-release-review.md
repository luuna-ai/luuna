# Patch: release-script blockers found in final review

Scope: `scripts/release/` and `docs/RELEASING.md` only, for `specs/chore-native-release.md` (run `9f32a7c1`, task C-roots). The runtime patches (`patch-native-release-compile.md`, `patch-native-release-clippy.md`) were reviewed, preserve behavior, and must not be touched again.

## Blockers

1. **The checksum is optional.** In `scripts/release/check-archive.sh:78-85`, the `SHA256SUMS` check runs only `if [ -f "$sums" ]`. If the file is missing, an archive passes with no integrity check at all. This contradicts the chore spec ("Checks the name, the sibling `SHA256SUMS`…") and `docs/RELEASING.md:52`. The offline tests cover a matching sum and a mismatch, but not a missing file.
   - Fix: make `SHA256SUMS` mandatory. Fail with exit 1 if it is absent, is a symlink, or is not a regular file. Also fail if the archive name appears more than once (`expected` must be exactly one 64-hex value). Keep `--no-exec` unrelated to checksum enforcement.
   - Tests in `test-release-scripts.sh`: missing `SHA256SUMS` exits 1; a duplicate or malformed entry exits 1. Every existing "good" stub case must write a matching `SHA256SUMS` first.

2. **Provenance hides untracked and unknown state.** `scripts/release/package.sh:175-178` computes `worktree_dirty` with `git status --porcelain --untracked-files=no ... || true`.
   - Untracked files are ignored. For example, an untracked `.cargo/config.toml` or `build.rs` input can change the build, and right now this whole release tooling is untracked. Yet `worktree_dirty` can be `false`, and `docs/RELEASING.md:74` tells owners to trust `worktree_dirty: false`.
   - A git failure (no repo, no git binary, unsafe-directory refusal) is reported as `false` ("clean") instead of unknown.
   - Fix: run `git status --porcelain --untracked-files=normal`, which still excludes ignored files such as `target/` and `.tac/runs/`. Emit `true` or `false` only when the command succeeds, and emit `null` when git is unavailable or fails. Treat `commit` the same way, which it already does. Optionally add `"untracked_present": true|false|null`. Update `docs/RELEASING.md` to say that `null` means unknown and must not be accepted as clean.
   - Test: a fixture where git is unavailable (for example `PATH` without git, or `GIT_DIR` pointing at a non-repo) yields `"worktree_dirty": null`. Keep it offline. If the build step cannot be stubbed, factor the provenance computation so it can be exercised without cargo, and keep the change minimal.

3. **The archive mtime depends on the timezone.** `package.sh:140` uses `touch -t 198001010000`, which is interpreted in the local timezone. Archives built on hosts with different `TZ` therefore differ, which breaks the "fixed mtime / reproducible" claim. GitHub runners happen to use UTC, but local dry runs elsewhere do not.
   - Fix: `TZ=UTC touch -t 198001010000 ...`. Equivalently, export `TZ=UTC` for the touch/tar steps.
   - Test, if feasible offline: run the touch under two `TZ` values and assert the same epoch. If not feasible, record that it is a manual check, and do not claim it passed.

## Non-blocking notes (do not fix in this patch unless trivial)

- `release.yml` uses `dtolnay/rust-toolchain@master`, and actions are pinned by tag rather than SHA. `rust-toolchain.toml` still forces `1.99.1`, and the host-triple check enforces native runners. Consider SHA pinning as a later hardening step.
- `inputs.version` reaches shell only through `env:`, so there is no injection. Its uses in `with:` and in the artifact name are non-shell, and `package.sh` validates it before upload.

## Validation

```sh
sh scripts/release/test-release-scripts.sh
OUT="$(mktemp -d)"; sh scripts/release/package.sh --version 0.0.0-local --out "$OUT"
sh scripts/release/check-archive.sh "$OUT"/*.tar.gz
(cd "$OUT" && shasum -a 256 -c SHA256SUMS)
rm "$OUT/SHA256SUMS"; sh scripts/release/check-archive.sh "$OUT"/*.tar.gz; test $? -eq 1
grep -n worktree_dirty "$OUT/provenance.json"   # true here: untracked release files exist
git diff --stat -- crates/                       # unchanged from the reviewed runtime patches
```

Make at most three repair attempts per failure. Do not commit, push, or publish. Do not touch `.tac`.
