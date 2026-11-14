# Prime, Git commands and hooks

Reference: Luuna Atlas c4db80c3f2e622afbdd84f09077fd73b2a92d199, Go 1.27.0.
This plan fills only the reserved prime module, its own tests and notes. Existing
foundation/lifecycle/action source, Cargo files and shared harnesses are unchanged.

## Typed surface

prime::run takes explicit Context, positional arguments and Options flags/input.
It implements prime, learn, diff, sync and hook. Submodules expose loaded domains,
UTF-16 token accounting, trust/sort/budget/annotations, manifest/project-contract
rendering, active-work resolution/filtering, diff parsing, guard matching and
file-anchored injection with explicit registry/time/session state.

Prime supports all formats, manifest/default modes, domains/files/active-work
selection, supersession removal, pinned failure budgets, hooks, dry-run and export.
JSON output follows Go's budget and direct-output rules. Export is an explicit
requested replacement, retaining truncate/link/mode semantics.

Active-work resolution uses in-progress Missions or legacy Seeds/Beads rows before
branch fallbacks, and the configured gh executable for PR numbers. Tests always
supply a stub gh ahead of host tools. ASCII word boundaries and leftmost
non-overlapping captures are hand-written; no regex dependency was added.

Sync preserves strict validation despite global escape flags, shared-main-store
worktree skips, staging and commit order, empty-message failure and captured Git
stderr. Diff preserves stdout even when Git fails, because the reference ignores
that exit status. All child environments come from Context.

Hooks use the reference's shell/file guard and 500-token injection budget. Pinned
failures survive budget exhaustion. Ignored state contains session IDs and usage
JSONL, with the original seven-day session expiry. Relative input cwd resolves
against the explicit invocation directory. Raw cache bytes remain distinct from
UTF-8 replacement characters. Expired symlinks are unlinked without removing their
targets, matching Go.

## Preserved reference defects and boundaries

* The shell guard uses simple whitespace/segment matching, not a shell parser.
  Multi-digit redirect descriptors, incomplete quoting and command exemptions
  retain the exact observed reference behavior.
* FindRepoRoot stops at the first directory reported as inside Git, including a
  subdirectory. Injection can therefore describe a file relative to that cwd and
  mark a broader anchor missing; the comparison deliberately preserves this.
* Diff keeps the previous expertise domain after an unrelated diff header, and
  projects non-object diff values to empty records. This read-only display defect
  is copied; stored JSONL arrays remain preserved by the shared raw sidecar.
* Quiet does not suppress all direct output paths in the reference. JSON prime
  ignores the human budget but still validates the supplied budget syntax.
* NaN trust weights produce non-total comparisons. Rust's stable sort disagreed
  with the actual comparison with 128 records Go result. An owned iterative adaptation of the pinned
  Go stable/symMerge/rotate algorithm now preserves that order, including exact
  left-before-right merge comparisons.
* Prime's too-deep JSONPretty result is successful empty output. The10000-level
  raw-array case preserves it, including on a 128 KiB native thread stack.
* Session expiry is the reference's intentional retention operation. Other hook
  failures remain fail-open; the raw hook lifecycle and managed background-child
  cleanup remain assigned to runtime wiring.

The single corrected positive custom-type fixture omitted Go's required dedup_key.
It was fixed in its own commit while retaining the expected successful behavior.
No existing frozen assertion was changed.

## Validation

The own comparison suite has 32 ordinary groups and two streamed hostile groups.
It covers all formats and contract/manifest variants, raw arrays/unknown types,
budgets, active trackers and branch matchers, real Git commits/diffs/worktrees,
strict sync failures, failing-command stdout/stderr, hooks/session expiry/raw cache
and invocation-directory isolation.

Hostile fixtures are generated on disk under target and executed only after
compilation/reference build, with 4 GiB address-space and 120-second limits. The long
shell case supplies 200000 wrappers through a file; native parsing and deep prime
output are also checked on 128 KiB threads. Ordinary workspace temporary roots stay
outside Git. Existing Atlas suites and full workspace/frontend gates are run
separately. WSL evidence does not establish native macOS/Windows execution.

## Attribution

The Go stable-sort adaptation in prime/go_sort.rs derives from Go 1.27
src/sort/zsortinterface.go, copyright 2022 The Go Authors. Its BSD redistribution
terms are retained in prime/GO-SORT-LICENSE. Binary releases must include that
notice alongside Atlas's MIT notice and the parser notices carried from lib3.
No dependency, lock package or version changed.
