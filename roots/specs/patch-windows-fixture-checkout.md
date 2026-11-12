# Patch: keep compat fixture bytes exact on Windows checkout

Branch `fix/windows-fixture-checkout`. Follows `specs/chore-native-release.md` / `specs/patch-native-release-clippy.md`, which turned the native Windows CI job green enough to reach the behavior fixtures.

## Problem

CI run `39958151128` (`Native Cargo (windows-latest)`, step `Test`) fails all 5 tests in `crates/roots-cli/tests/behavior_fixtures.rs`:

| Test | Assertion site | Symptom |
|---|---|---|
| `windows_init_preserves_reference_store_bytes` | `behavior_fixtures.rs:1112` | `config project name`: actual `config.yaml` is LF, expected fixture is CRLF |
| `windows_malformed_jsonl_last_wins_fixture_matches_reference` | `:431` | `JSONL snapshot …\.luuna/roots\graph.jsonl`: actual LF, expected CRLF |
| `windows_setup_guard_environment_and_legacy_fixtures_match_reference` | `:440` | `file snapshot …\.claude/settings.json`: actual bytes `…10…`, expected `…13, 10…` |
| `windows_read_and_heading_fixtures_match_the_reference_cli` | `:440` | `file snapshot …\report.md`: same LF vs CRLF |
| `windows_mutation_fixtures_match_the_reference_cli` | `:394` | `scan.json` stdout: every idea `changed: true`, expected `changed: false` |

Source: `/tmp/luuna-roots-ci-failed-39958151128.log` lines 139–192.

Root cause: the Windows runner's Git has `core.autocrlf=true`. On checkout it rewrites LF to CRLF in every text-detected blob, including `tests/compat/**` expected outputs and input stores. The CLI writes LF, matching the reference, so byte comparisons against CRLF expected files fail. The `scan` failure has the same cause. Input idea bodies are checked out as CRLF, so their content hashes no longer match the hash ledger recorded from the LF blobs, and every idea reports `changed: true`.

The product is correct. The fixtures as committed (Git blobs) are correct. Only the checkout transformation is wrong.

Current state:
- Root `.gitattributes` has one line: `.luuna/roots/*.jsonl merge=union`. It sets no `text`/`eol` attribute.
- `tests/compat/behavior/input/{current,heading,read}/.gitattributes` are fixture content. Each contains only `.luuna/roots/*.jsonl merge=union` and sets no `text`/`eol`. Git merges attributes per attribute, so they will not override a root `-text`. Confirm this in task 1.
- A working-tree grep for CR line endings under `tests/compat` on macOS finds no files, so no fixture blob currently contains intentional CRLF. The rule must still preserve CRLF if a future fixture adds it on purpose.

## Fix (smallest)

Append one line to the root `.gitattributes`:

```gitattributes
tests/compat/** -text
```

Resulting file:

```gitattributes
.luuna/roots/*.jsonl merge=union
tests/compat/** -text
```

`-text` turns off end-of-line conversion on checkout and on commit for every path under `tests/compat/`, including hidden paths such as `.luuna/roots/*.jsonl`, `.claude/settings.json`, and nested `.gitattributes`. The working tree then matches the blob byte for byte, whatever `core.autocrlf` or `core.eol` is set to. Diffs still render as text, because `-text` does not imply `-diff`. Merging keeps the existing `merge=union` driver for `.luuna/roots/*.jsonl`. `-text` does not imply `binary` (`-diff -merge -text`), so merge behavior is unchanged.

Rejected alternatives:
- `eol=lf` / `text eol=lf`: forces normalization. It would rewrite any intentional CRLF fixture to LF on commit/checkout and change what is frozen.
- `* -text`, a global `core.autocrlf=false` in CI, or `actions/checkout` settings: broader than needed. Developer checkouts would still break, and non-fixture files would be affected.
- `binary`: also turns off diff and union merge.
- Changing the test harness to normalize CRLF: weakens the intended strict byte comparison.

## Out of scope

- Any change to fixture files, expected outputs, product code (`crates/**`), test code, `ci.yml`, release scripts/workflow, `Cargo.*`, or docs. No skipped, `#[ignore]`d, or `cfg`-gated tests.
- Renormalizing the index (`git add --renormalize`). Blobs are already LF and correct, so the commit should only change `.gitattributes`.
- A permanent checkout-regression test harness. Proof comes from the reproduction below plus native Windows PR CI.
- Commits, pushes, and PRs in this planning step.

## Step by Step Tasks

1. Preflight (read-only):
   - `git status` is clean on `fix/windows-fixture-checkout`. `cat .gitattributes` shows exactly the single `merge=union` line.
   - `cat tests/compat/behavior/input/*/.gitattributes` shows only `merge=union` lines, with no `text`, `eol`, `crlf`, or `binary`. If any nested file sets one of these, stop and report: a nested rule takes precedence over the root for its subtree, and the fix would need revisiting.
   - Run the blob CR census (validation section A). Record how many fixture blobs contain CR. Expected: 0. Any nonzero count is intentional CRLF that the fix must preserve, and the reproduction verifies it.
2. Reproduce the failure and the fix in a throwaway sandbox (validation section B). Run it before editing the real `.gitattributes` and again after:
   - Control: clone with the **original** attributes and `core.autocrlf=false`. The working tree equals the blobs.
   - Failure: clone with the **original** attributes and `core.autocrlf=true`. The working tree differs from the blobs for LF fixture files, including hidden `.luuna/**` and `.claude/**` paths, and the files named in the CI log.
   - Fix: clone with the **new** rule under both `core.autocrlf=true` and `false`. The working tree equals the blobs for every file under `tests/compat/`, including any CRLF blobs from task 1.
   - Record the exact commands and their mismatch counts in the PR description.
3. Use Edit to append `tests/compat/** -text` to the root `.gitattributes` (keep the trailing newline). Change no other file.
4. Confirm the attribute resolves on representative paths, including hidden ones (validation section C).
5. Confirm no fixture blobs change by renormalizing a temporary copy of the index and checking its fixture diff against HEAD (section D). `git add --renormalize --dry-run` lists all candidate paths even when their blobs are unchanged; its output is not a pass/fail check. The real index stays untouched.
6. Run the local quality gates (validation section D). Make at most three repair attempts for any single failure. Do not edit fixtures or product code to make anything pass; if a gate fails for an unrelated reason, report it as a blocker.
7. Leave the change uncommitted for review. After approval, commit `.gitattributes` and this patch spec (e.g. `fix(ci): keep compat fixture bytes exact on checkout`), push, and open a PR. The **native Windows proof is the PR's `Native Cargo (windows-latest)` job**: `behavior_fixtures` must report `5 passed; 0 failed`, with no new ignores. Link that run in the PR, next to failing run `39958151128`.

## Validation Commands

Run from the repo root on macOS/Linux, using Bash for the shell snippets. `$REPO` is the absolute repo path. Sandboxes are under `mktemp -d` and are deleted afterward.

### A. Blob CR census (original fixture blobs, independent of working-tree conversion)

```sh
REPO="$(git rev-parse --show-toplevel)"
git -C "$REPO" ls-files -z tests/compat | while IFS= read -r -d '' f; do
  if git -C "$REPO" cat-file -p "HEAD:$f" | LC_ALL=C grep -q $'\r'; then echo "CR-blob: $f"; fi
done | tee /tmp/roots-cr-blobs.txt
wc -l < /tmp/roots-cr-blobs.txt          # expect 0 today; any listed file is intentional CRLF
git -C "$REPO" ls-files tests/compat | grep -c '/\.'   # hidden-path fixtures present (.luuna, .claude, .gitattributes)
```

### B. Deterministic checkout reproduction (sandbox; never touches the real repo config)

```sh
REPO="$(git rev-parse --show-toplevel)"
SB="$(mktemp -d)"

# compare every tests/compat working-tree file to its blob; print mismatch count
compare() {  # $1 = clone dir
  n=0
  while IFS= read -r -d '' f; do
    if ! git -C "$1" cat-file -p "HEAD:$f" | cmp -s - "$1/$f"; then echo "  differs: $f"; n=$((n+1)); fi
  done < <(git -C "$1" ls-files -z tests/compat)
  echo "MISMATCHES=$n"
}

# Original-rule commit = current HEAD. New-rule commit = HEAD + the one-line change, built in a sandbox clone.
git clone -q --no-checkout "$REPO" "$SB/src"
git -C "$SB/src" checkout -q HEAD
printf '%s\n' '.luuna/roots/*.jsonl merge=union' 'tests/compat/** -text' > "$SB/src/.gitattributes"
git -C "$SB/src" -c user.name=sandbox -c user.email=sandbox@invalid commit -qam 'sandbox: tests/compat -text'
ORIG="$(git -C "$REPO" rev-parse HEAD)"
NEW="$(git -C "$SB/src" rev-parse HEAD)"

for rule in ORIG NEW; do
  for acrlf in false true; do
    d="$SB/$rule-$acrlf"
    git -c core.autocrlf=$acrlf clone -q --no-checkout "$SB/src" "$d"
    git -C "$d" config core.autocrlf $acrlf
    git -C "$d" config --unset core.eol 2>/dev/null || true
    git -C "$d" checkout -q "$( [ $rule = ORIG ] && echo "$ORIG" || echo "$NEW" )"
    echo "== rule=$rule core.autocrlf=$acrlf"; compare "$d"
  done
done

# Spot-check the exact files named in the CI log (hidden paths included)
for rule in ORIG NEW; do
  d="$SB/$rule-true"
  git -C "$d" ls-files tests/compat | grep -E '(config\.yaml|graph\.jsonl|\.claude/settings\.json|report\.md)$' | head -8 |
    while read -r f; do printf '%s %s CR-lines=%s\n' "$rule" "$f" "$(LC_ALL=C grep -c $'\r' "$d/$f")"; done
done

rm -rf "$SB"
```

Expected:
- `rule=ORIG core.autocrlf=false` → `MISMATCHES=0` (control).
- `rule=ORIG core.autocrlf=true` → `MISMATCHES>0`, including `.luuna/…` and `.claude/settings.json` paths. Spot-check `ORIG … CR-lines>0` (reproduces the Windows failure).
- `rule=NEW core.autocrlf=false` and `rule=NEW core.autocrlf=true` → `MISMATCHES=0`. Spot-check `NEW … CR-lines=0`, or equal to the blob's CR count for any file from section A.

Note: `core.autocrlf=true` on macOS/Linux converts on checkout exactly as it does on Windows. This reproduction is deterministic, but it does not replace the native Windows CI run.

### C. Attribute resolution in the real repo (after task 3)

```sh
git check-attr text merge -- \
  tests/compat/behavior/input/read/.luuna/roots/graph.jsonl \
  tests/compat/behavior/input/read/.gitattributes \
  $(git ls-files tests/compat | grep -E '\.claude/settings\.json$|report\.md$|config\.yaml$' | head -4) \
  .luuna/roots/graph.jsonl crates/roots-cli/src/lib.rs
```

Expected: every `tests/compat/**` path reports `text: unset`. The fixture `graph.jsonl` also reports `merge: union`. `.luuna/roots/graph.jsonl` reports `merge: union`, `text: unspecified`. `crates/…/lib.rs` reports `text: unspecified` (unchanged).

### D. Local quality gates

```sh
idx="$(mktemp)"
cp "$(git rev-parse --git-path index)" "$idx"
GIT_INDEX_FILE="$idx" git add --renormalize -- tests/compat
GIT_INDEX_FILE="$idx" git diff --cached --exit-code -- tests/compat  # expect no fixture diff
rm "$idx"
git status --porcelain                           # only .gitattributes and this new patch spec
git diff --stat                                  # .gitattributes: rule plus explanatory comments

cargo fmt --all -- --check
cargo clippy --workspace --locked --all-targets --all-features -- -D warnings
cargo test --workspace --locked --all-features
cargo test -p roots-cli --locked --test behavior_fixtures   # host run; Windows-only tests are cfg'd, so expect them absent or passing, never ignored
sh scripts/release/test-release-scripts.sh

# No test was skipped or ignored by this change
git diff -- crates tests | wc -l                 # expect 0
```

### E. Native Windows proof (after approved push; PR CI only)

- PR CI job `Native Cargo (windows-latest)`, step `Test`: `behavior_fixtures` reports `test result: ok. 5 passed; 0 failed; 0 ignored`, and each of the 5 `windows_*` tests listed above reports `... ok`.
- All other CI jobs (fmt, clippy, Linux/macOS tests, release script tests) stay green.

Expected overall: sections A–D pass locally, and section E is confirmed on the PR before merge. The only existing file changed is `.gitattributes`, adding the fixture rule and explanatory comments; this patch spec is added alongside it.

SPEC: specs/patch-windows-fixture-checkout.md
