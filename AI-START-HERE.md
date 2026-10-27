# Luuna implementation handoff

## Latest: terminal key bindings and settings, and no repository CI (October 26, 2026)

[ADR 0040](docs/decisions/0040-terminal-keymap-and-settings.md): the terminal
workbench's keys go through one keymap (`crates/luuna-tui/src/keys.rs`) with
macOS-reachable defaults (`?`, `:`, `1`–`5`, `m`, `Ctrl+J`, `Ctrl+B`/`Ctrl+F`…) beside
the function keys, AltGr characters type correctly, and keys, colors and the kitty
keyboard flag are read from `~/.config/luuna/tui.json` (`luuna-tui --print-config`).
Evidence: `cargo test -p luuna-tui` including the PTY tests, clippy and fmt on Linux;
no Mac or Windows run yet.

[ADR 0041](docs/decisions/0041-no-repository-ci.md): **this repository has no
CI/CD.** The GitHub Actions workflow is removed and nothing replaces it. Do not add
workflows or treat a hosted check as a blocker or as evidence; run the local checks
in [CONTRIBUTING.md](CONTRIBUTING.md) and record what ran. The runtime's CI
observation on published pull requests (ADR 0018) is a product feature and is
unchanged.

## Latest: short references and picked input (October 23, 2026; merged October 26, 2026)

[ADR 0042](docs/decisions/0042-short-references-and-picked-input.md) removes copying generated ids
and hashes from every client. Terminal lists show short ids (first eight hex digits of a record's
UUID) and every ticket, plan, run and memory argument accepts one, a unique 4+ digit prefix or
`latest` (runs), resolved to the exact id before anything is sent (`crates/luuna-tui/src/refs.rs`).
Hash arguments accept an 8+ digit prefix of the hash just read; interactive approval takes a typed
`approve` after the displayed plan and hash. Workbench forms offer known records and accepted
values as Left/Right choices. The desktop's `ShortId` (`@luuna/ui`) shows short forms and copies
the whole value; "derived from advisory task" is a select, and repository ids come from the folder
name. Drafted as ADR 0040 (PR #21) and renumbered 0042 at merge. The merge (PR #22) also fixed
what a review of the combined tree found: a workbench form resolves a reference once among loaded
records and sends that exact id (never keeping a short id or `latest` as context), the bridge loop
no longer drops the event it dequeues at its per-wake limit, unbinding left/right also stops their
modified forms, and the desktop's Repository ID label is exactly that again (the browser smokes
select it). PR #18 (Stripe-inspired desktop UI) was closed unmerged: its commits were already on
`main` and its direction was superseded by ADR 0033 and 0035. Evidence: the local checks listed in
[ADR 0042](docs/decisions/0042-short-references-and-picked-input.md#evidence) on the Linux cloud
container; no Mac, Windows, live-provider or native-window run.

**Known, pre-existing, not caused by these changes:** on a 4-CPU host, 2 or 3 `luuna-runtime`
tests fail per full run, different ones each time. Most fail with `409 workflow already has active
work` right after the run reports a settled state. Pre-PR `ca230fb` failed at the same rate. Cause:
`spawn_supervised` (`crates/luuna-runtime/src/lib.rs`) removes a run's `active` entry only after the
worker, which has already written the settled state, drops, while the tests' `settled()` helpers
poll only the state. Rerun a failing test alone before treating it as a regression. The fix belongs
in the runtime or those helpers, not in client changes.

## Latest: Orbit and the project sandbox (October 22, 2026)

[ADR 0036](docs/decisions/0036-orbit-and-project-sandbox.md) adds Orbit (status, exec,
recipe runs and sync across a project's registered repositories; never evidence) and
the project sandbox: the runtime computes a plan, `luuna-sbx` (new crate
`luuna-sandbox`) applies it with bubblewrap, Seatbelt or a Windows AppContainer, and
`lu sbx <project>` opens it with the terminal attached. `lu` is the short name of
`luuna`. Both clients gained an Orbit view. Evidence on this Windows host: the
AppContainer denied a sibling repository, `Documents` and the parent listing while
reading and writing the project's repositories through `lu sbx` against a live
runtime; bubblewrap was exercised in WSL Ubuntu with the Linux build of the
launcher. The macOS profile is unit-tested only. Nothing was committed.

## Latest: M4–M8 slices, the Windows recovery drill and client parity (October 21, 2026)

On top of the PR 11 audit branch, this change set lands bounded, tested slices of
every remaining milestone on the Windows host, each recorded with what it does
not establish: the MVP gate's backup/migration recovery drill
(`scripts/backup-recovery-drill.mjs`, 31/31), multi-repository coordination
([ADR 0017](docs/decisions/0017-multi-repository-coordination.md), three-repository
fixture, 9 scenarios), GitHub publication with an intent ledger and `luuna-runtime
doctor` ([ADR 0018](docs/decisions/0018-publication-and-doctor.md), fake-GitHub
scenarios only), verified stack profiles, host capability probing and approved
scaffolding ([ADR 0019](docs/decisions/0019-stack-profiles-and-scaffolding.md)),
the capability catalog, shared failure explanations and the terminal client
([ADR 0020](docs/decisions/0020-capability-catalog-and-tui.md), `scripts/tui-smoke.mjs`),
memory promotion and the scope-safe project export
([ADR 0021](docs/decisions/0021-memory-promotion-and-project-export.md)), an unsigned
NSIS development installer (`scripts/package-windows.ps1`) and the M8 report
([RELEASE-EVIDENCE.md](docs/RELEASE-EVIDENCE.md), `scripts/release-evidence.mjs`).
Read the per-milestone sections at the end of
[FOUNDATION-STATUS.md](docs/FOUNDATION-STATUS.md) for the exact commands and
counts on this host. Storage migrations 10 and 11 were added; `reqwest`/`rustls`
are new runtime dependencies. Nothing was committed, merged, installed or published.

**Next assignment:** evidence on hosts this one is not, never a shortcut around
them: (1) a Mac run of the native smoke, the drill and the packaging; (2) a
clean Windows machine installing the produced installer and completing the
fixture walkthrough with `luuna-runtime doctor`; (3) a real GitHub publication
with a token the owner supplies (push, linked PRs, CI observation, a human
merge); (4) capable runners for Go, Flutter, Android and iOS profiles and
scaffolds; (5) a genuine-provider `m3-smoke` run and a genuine multi-repository
feature on the current tree; (6) a real-project pilot with the measurements
`docs/11-acceptance.md` asks for. Keep merging human; keep the v1 label ungranted
until [RELEASE-EVIDENCE.md](docs/RELEASE-EVIDENCE.md) has no blocking row.

## Earlier: PR 11 review follow-up (October 21, 2026)

The audit branch includes fixes for all 14 confirmed review issues and Windows
acceptance-harness corrections. Existing main history, component pins, installed
executables, credentials and live database contents were preserved. PR 11 was
merged into `main` (merge commit `bf538cc`, October 21, 2026); no installation of
this revision is recorded. Native/browser and fixture checks passed;
strict genuine-provider M3 remains open (final run 59/61 because the blocking
probe's writer produced no candidate). One existing Windows-disabled stress test
was not run. Read [the exact revisions, coverage and failures](docs/PR-11-WINDOWS-VERIFICATION.md)
before claiming full Windows acceptance. The older successful M3 results below
apply to their recorded revisions, not automatically to PR 11.


You are continuing an executable foundation, not starting an empty repository. Read [AGENTS.md](AGENTS.md), [CURRENT-ARCHITECTURE.md](docs/CURRENT-ARCHITECTURE.md), [FOUNDATION-STATUS.md](docs/FOUNDATION-STATUS.md), and [IMPLEMENTATION-ROADMAP.md](docs/IMPLEMENTATION-ROADMAP.md) before changing code. Inspect the current working tree, manifests and relevant implementation/tests; prior test counts are audit evidence, not a substitute for checks after your changes.

## Settled decisions

Performance follow-up (October 21, 2026): ADR 0015 adds bounded blocking request
dispatch, paged session history, bounded desktop replay, workflow snapshot reuse
and Windows check-process ownership. See [PERFORMANCE-REVIEW.md](docs/PERFORMANCE-REVIEW.md)
for current-tree tests, optimized Windows installation verification and explicit
scaling limits. Performance commit `839e46a` and harness fix `974ff0e` are
in `main` through PR 11. Strict Windows M3 acceptance passed on clean `974ff0e`:
68/68 fixture and 61/61 genuine-provider checks. See the exact commands,
scope and retained failure evidence in [Windows verification](docs/WINDOWS-VERIFICATION.md#strict-m3-acceptance-result).
The installed Windows development paths now contain verified optimized release
binaries; the app is open and its history/credential identity was preserved.
Never treat cached display data as authority for an action.

Windows development evidence (October 21, 2026, after the Linux handoff below):
authenticated shutdown, DPAPI provider setup, the bundled Tauri/WebView2 app and
automatic runtime startup/connection are implemented and verified. See
[Windows verification](docs/WINDOWS-VERIFICATION.md) and ADRs 0010/0011 for
commands, results and remaining limits. The historical Linux host statements
below do not describe the later Windows run. Signed installation, installed
service supervision remain open; strict clean-revision Windows M3 acceptance
has since passed as recorded above.

The component-enabled Windows development build is now installed and connected.
Final bundled native checks passed 27 component, 46 lifecycle/recovery and 17
automatic startup assertions. The user selected unsigned development and turned
Smart App Control Off; the exact final runtime then started successfully with
existing history and credentials preserved. Component CLIs were real; the new
Pi workflow tests were synthetic. See the [final native evidence](docs/FOUNDATION-STATUS.md#final-windows-native-verification-and-completed-development-upgrade).

- Tauri/React desktop first; one independent Rust runtime owns policy and persistent state. Browser/TUI capability parity remains required for full v1.
- Pi RPC is the engine. Do not implement a second model/tool loop or replace it with a UI simulation.
- **Missions, Atlas and Beacon have independent public repositories** under the `luuna-ai` organization, consumed as exact-commit Git submodules under `components/` (ADR 0012). Run `npm run components:setup` after cloning and `npm run test:components` to verify real CLI/JSON consumption. Keep manifest pins and gitlinks synchronized. Preserve compatible formats, legacy aliases and upstream MIT notices. Product/new contributions are MIT. The bounded runtime adapter slice is implemented in ADR 0014; broader M6 parity and platform exits remain open.
- Explicit project component activation initializes private managed stores and migrates legacy tickets to Missions authority without changing their IDs/revisions. SQLite then stores a projection; external edits require expected-revision reconciliation and a new plan. Legacy projects stay explicitly temporary until activation. Atlas memory and Beacon prompt resources enter worker context only after selection and exact snapshot approval. Luuna owns execution state, plans, approvals and evidence. See [ADR 0014](docs/decisions/0014-runtime-component-workflows.md).
- The canonical create-ticket route is `POST /api/v1/projects/{id}/tickets`, with an idempotency key and either legacy temporary storage or explicitly enabled Missions authority; do not add an ambiguous global create route.
- Service lifetime is independent of client lifetime. A window close/disconnect is not cancellation. Packaged Mac/Windows supervision remains to be proved.

The current architecture and accepted ADRs resolve conflicts in older proposals. Original [R01–R15 requirements](docs/02-requirements.md) and [A01–A34 acceptance criteria](docs/11-acceptance.md) remain full-v1 obligations unless explicitly revised. The desktop/Pi proposal and numbered reference chapters supply rationale and history. Never select an older conflicting example silently.

## What actually exists

| Boundary | Current implementation | Remaining gap |
| --- | --- | --- |
| `luuna-domain` | Plan hashing, permissions/revisions, transitions and candidate/review guards | Runtime consumers must preserve these invariants |
| `luuna-storage` | Transactional SQLite plans/approvals, durable run/events, restart interruption, project registry with immutable manifest revisions, machine registry, project event stream, baselines with immutable check results, legacy temporary tickets and enabled Missions projections/delivery intents, component activation/operation ledgers, plan workflows, worktree leases (one active per repository), attempts, `begin_implementation` (approval + ticket revision + current manifest and base + passed settled baseline in one transaction), candidate evidence generations with the re-validation gate for a review that left no verdict | Native-host M3 evidence; broader memory retention/privacy policy |
| `luuna-executor` | Supervised recipe execution: executable/argument arrays, capability probes, secret-stripped environment, process-group ownership, deadlines, cancellation, hashed private artifacts, empty-suite detection, environment fingerprint | Container profile; broader runner summaries; Windows job objects |
| `luuna-git` / `luuna-discovery` | Read-only inspection (identity, base, dirty state, redacted remote), recipe proposals with evidence/questions (never executing repository text), and Luuna-owned detached worktrees with changed-path listing and candidate commits | Relationship discovery (M4); more stack profiles (M5); baselines inside leased worktrees |
| `luuna-engine-pi` | Pinned real-process transport, command correlation, bounded framing, stop/reap, protocol fixtures, a tool-free profile and a writer profile (named file tools, explicit scope-guard extension, task prompt); genuine OpenRouter-backed runs proved on Linux (October 17, 2026) | Complete supported-host (Mac/Windows) lifecycle evidence; other providers/models as they are adopted |
| `luuna-runtime` | Authenticated loopback run API, replay/SSE, cancel/timeout, exclusive state ownership; project register/discover/confirm routes; baseline runs with artifact access by identity; tickets, plan revisions, exact-hash approvals, transactional scheduling and one scoped writer in a leased worktree; candidate validation, fresh independent review, bounded repair and in-place re-validation (`revalidate_allowed`) | Native-host M3 evidence; publisher (M7) |
| Desktop/API client | Real browser-tested connection, history, run/cancel, cursor reconnect, project import/discover/confirm panel, Run baseline with live evidence, ticket/plan/approve/implement workflow with lease and candidate display; outage display with self-reconnection and panel reload, refusal messages with the next allowed action, keyboard operation and live regions; the committed `scripts/mvp-ui-smoke.mjs` Chromium smoke; Tauri shell | Native GUI proof and packaged service lifecycle |
| Components | Independent public pinned Missions/Atlas/Beacon repositories; bounded native CLI runner, explicit private-store activation, Missions migration/reconciliation, scoped Atlas memory and versioned Beacon prompts selected as exact plan snapshots | Broader M6 browser/TUI parity, retention/privacy policy and supported-platform integration evidence |

The recorded Linux foundation checks passed: **177 Rust tests, 22 API-client tests, 10 desktop tests, 5 scope-guard tests, 20 schema checks, and 3,092 component tests** (October 21, 2026; the component suites were last run on October 17), plus Rust formatting/lint and frontend type/build checks. A genuine Pi version/RPC handshake and real missing-key failure, cancellation, hard-kill restart and browser reconnect paths were exercised through API and Chromium. **A genuine model response was proved on October 17, 2026** (OpenRouter `openai/gpt-4o-mini`, 19/19 real-mode smoke checks, browser run/cancel/reload replay); **no native GUI launch has been proved.** See the [status report](docs/FOUNDATION-STATUS.md).

Do not redo completed scaffolding, relax guards, or label the tool-free demo an approved coding workflow. Read the current [status report](docs/FOUNDATION-STATUS.md) for updated evidence and blockers.

## Current component workflow handoff (October 21, 2026)

The application now has actual runtime adapters for `luuna-missions`, `luuna-atlas` and `luuna-beacon`, beyond the extraction consumer smoke. Enable components explicitly per project through the component panel/API. Activation writes only private managed stores, migrates legacy ticket identity/revisions and freezes incomplete projects until retry/reconciliation. Ticket GET/list show the last reconciled projection; `/tickets/{id}/source` observes actual source freshness and `/reconcile` requires the displayed revision. External title/description/status/dependency edits block stale work and need a new plan after reconciliation. Create operations record durable intent before invoking a CLI and reconcile uncertain writes by stable identity; missing/duplicate uncertain identities fail closed.

Memory has explicit project/process/user scope and provenance; user memory is explicitly retrieved/selected. Prompt preview includes the current version and content hash; updates need the expected version. Select resources before creating a plan: exact content/provenance/versions/hashes are part of its immutable canonical approval. Runtime freshness checks refuse stale selected resources and ticket assumptions before work or candidate evidence is consumed. Imported personal/development stores are never automatically read. The CLI runner uses pinned sources/Bun, bounded stdin/output and owned native processes; it does not create an OS sandbox.

See [ADR 0014](docs/decisions/0014-runtime-component-workflows.md) and [the component workflow evidence](docs/FOUNDATION-STATUS.md#runtime-component-workflows-october-21-2026) for current commands and actual results. The focused integration tests invoke real component CLIs in temporary private stores; synthetic Pi runs remain synthetic. The later Windows evidence above establishes native component behavior and startup for the recorded build; a fresh genuine-provider component-context exit remains unverified. Retain the existing native/platform and release gates below, and do not mark all of M6/full v1 complete.

## Next session handoff (October 21, 2026, latest): M3 re-confirmed with a real provider; the desktop design system is in

Two things landed on top of the Windows and component-extraction work. (1) A real-provider `m3-smoke` run on the MVP code passed 61/61 (OpenRouter `openai/gpt-4.1`, pinned Pi 1.3.0 with verified provenance); M3 still closes only with native-host acceptance (see the [status report](docs/FOUNDATION-STATUS.md#m3-genuine-provider-re-run-on-the-mvp-code-october-21-2026)). (2) The desktop client follows one token-based design system: [docs/DESIGN-SYSTEM.md](docs/DESIGN-SYSTEM.md) and [ADR 0013](docs/decisions/0013-desktop-design-system.md). Six tones are mapped from the API unions in `packages/ui/src/tone/tone.ts`, only passed evidence is green, interruption is never failure, and both colour schemes meet WCAG AA. `npm test` enforces the system (`packages/ui/src/design/design-system.test.ts`), and `scripts/mvp-ui-smoke.mjs` audits rendered contrast in both schemes and the 760 px width.

Component catalog for the owned `@luuna/ui` library: [docs/COMPONENT-LIBRARY.md](docs/COMPONENT-LIBRARY.md) (ADR 0034).

When adding a screen (Kanban, plan graph, memory, settings): register it in the design test, render every state through `StatusBadge` with a tone from `tone.ts`, compose the shared components before writing screen CSS, keep the screen sheet token-only and shallowly scoped, and rerun the desktop smoke. The next assignment below is unchanged. Note also that GitHub Actions currently refuses this private repository's jobs for an account billing reason, so CI evidence is local until that is restored.

## Next session handoff (October 21, 2026, later): the cloud-doable MVP gate is closed on Linux; the native-host smoke is what remains

Two more changes landed after the re-validation gate below. (1) An orderly runtime shutdown (Ctrl+C, SIGTERM) now records stopped work as `interrupted` with the reason "interrupted by runtime shutdown", never as `cancelled`, so it resumes after restart exactly like work stopped by a crash (ADR 0004 implementation note; demo runs, baselines, writers, candidate checks and reviews; four runtime tests). (2) The desktop client shows a lost runtime as an outage, reconnects by itself and reloads every panel when the runtime answers, reports every refusal as "<Action> refused: <reason>" plus the next allowed step (`apps/desktop/src/failures.ts`, tested), and is operable by keyboard (skip link, focusable scroll regions, live regions). `node scripts/mvp-ui-smoke.mjs` drives the real client in headless Chromium through [docs/MVP-WALKTHROUGH.md](docs/MVP-WALKTHROUGH.md) on two generated fixture repositories with the synthetic Pi: wrong token, refused registration, import, baseline, ticket, plan, approval, writer, diff, invalid review and re-validation, a hard kill and an orderly shutdown during checks with reconnection and resumption, a hard kill during review with the budget refusal, an accessibility audit and disconnect; it needs port 1420 free. [docs/MVP-EVIDENCE.md](docs/MVP-EVIDENCE.md) is the requirement/evidence report naming the MVP omissions. Results are in the [status report](docs/FOUNDATION-STATUS.md#mvp-usability-gate-client-hardening-shutdown-interruption-and-the-desktop-smoke-october-21-2026).

**Next assignment:** (1) the native Mac/Windows smoke on capable hosts: build, launch the Tauri shell against the service, connect with the token, run `scripts/m3-smoke.mjs --mode fixture` and `scripts/mvp-ui-smoke.mjs` there, stop and restart the service, and record the result; this closes M3 and the MVP gate and cannot be done in the cloud container. (2) Done October 21, 2026: a real-provider `m3-smoke` run on `fd42d59` passed 61/61 with `openai/gpt-4.1` through OpenRouter ([status report](docs/FOUNDATION-STATUS.md#m3-genuine-provider-re-run-on-the-mvp-code-october-21-2026)); repeat it after any runtime or reviewer-prompt change. (3) The backup/migration recovery drill from the roadmap's MVP gate. (4) Then M4 per the roadmap. Keep publication and merging deferred; do not count the synthetic smokes as provider evidence. A writer interrupted mid-session still needs a new plan revision (its worktree is preserved); allowing it to resume under the same approval is a candidate improvement, not a defect in the recorded evidence.

### Earlier handoff (October 21, 2026): re-validation after an invalid review is in; native exits and the rest of the MVP usability gate are next

The MVP usability gap recorded on October 17 is closed on Linux: a `failed` candidate whose required checks all passed in its current validation generation and whose reviewer left no verdict (invalid or self-inconsistent output, a refused session) can be re-validated in place through the existing `POST /api/v1/runs/{id}/validate` with the exact hash. Storage admits `Failed -> Validating` only under that condition and the existing headroom check, opening generation `n+1` of the same candidate revision under the same approval and budgets; the checks run again, a fresh reviewer session starts, no writer attempt is spent, and `GET …/candidate` reports the decision as `revalidate_allowed`, which the desktop turns into a "Re-validate and review" control. A failed, blocked or interrupted check, any recorded verdict, and reserved spend from a reviewer lost mid-turn all keep their existing paths (ADR 0009, point 7). The test-count heuristic now recognises Python `unittest` summaries, so the empty-suite guard applies to that runner. Evidence, commands and the fixture-mode harness run are in the [status report](docs/FOUNDATION-STATUS.md#mvp-usability-gate-re-validation-after-an-invalid-review-october-21-2026); the harness scenario is fixture-only because no real provider can be made to return invalid reviewer output on demand, and the real-mode probes assert only that reviewed and check-failed candidates are not re-validated.

**Next assignment:** (1) the native Mac/Windows client/service smoke on capable hosts, unchanged from below; it is the last M3 exit and cannot be done in the cloud container. (2) Continue the MVP usability gate against the working APIs: keyboard access and clear blocked/failure messages in the desktop workflow panel, desktop-to-service reconnection and explicit service shutdown exercised through the client, and the documented representative fixture workflow another developer can follow end to end. (3) Keep publication, merging and multi-repository scheduling deferred. Do not reuse fixture verdicts as provider evidence; a real-provider `m3-smoke` run after this change is still owed before the next provider claim.

### Earlier handoff (October 17, 2026, later): M3 provider exit closed on Linux; native exits and the MVP usability gate are next

The M3 provider exit is closed on Linux. `node scripts/m3-smoke.mjs --mode real --provider openrouter --model openai/gpt-4.1 --pi node --pi-args-json '["…/dist/cli.js"]'` passed every check (46, now 61 with the runtime-exit, build-identity, pinned-compiler, running-image, restart-binary, hard-kill-exit, Pi-provenance, clean-checkout, held-handle, stray-process, post-run checkout, credential-free verification and post-reap developer-checkout checks): it drove the merged M3 runtime through a genuine Pi 1.3.0 writer, real `unittest`/policy checks, a fresh independent reviewer, a seeded check failure repaired as revision 2 with fresh evidence, bounded automatic repair pausing at the attempt budget, cancellation and hard-kill recovery with conservative spend reservation, on a disposable fixture repository, with the developer checkout untouched and zero credential values in any artifact. Sixty real runs across three OpenRouter models, the last on the final code of this change set (runs 11, 15, 24, 25, 28, 36, 37, 52 and 53 recorded a writer that claimed an edit it had not made, runs 54 and 59 a repair writer that introduced a syntax error the checks caught, failed closed by the runtime; run 49 stopped at a harness npm-configuration defect before any provider session), are tabulated in the [status report](docs/FOUNDATION-STATUS.md#m3-genuine-provider-evidence-october-17-2026); read the "observed provider behaviour" paragraph before trusting any single run.

Two things changed because of the runs: repair feedback now quotes the tail of each failed check's log (a repair writer that only saw an exit code guessed wrong), and the reviewer prompt names every required finding field. Small models still violate the reviewer JSON contract sometimes; the runtime fails those candidates closed and the only recovery today is a repair revision.

**Next assignment:** (1) run the native Mac/Windows client/service smoke on capable hosts (launch, connect with the token, run the M3 harness against the packaged service, stop/restart the service, confirm orderly shutdown and interrupted-work reconciliation); this cannot be done in the cloud container and is the last M3 exit. (2) Open the MVP usability gate with the gap recorded above: allow a fresh validation generation for a candidate whose required checks passed but whose reviewer produced invalid or inconsistent output, without a new revision or new budget, and expose it in the client; recognise Python `unittest` output in the test-count heuristic. (3) Keep publication, merging and multi-repository scheduling deferred. Do not reuse fixture verdicts as provider evidence.

### Earlier handoff (October 17, 2026): verify the M3 provider and native exits

M2c and its six review corrections are merged in PR #3. The M3 branch implements exact candidate commit/tree evidence, genuine required-command execution in owned worktrees, a fresh read-only Pi reviewer, structured findings, manual and optional bounded automatic repairs, durable aggregate usage and conservative restart recovery. The desktop exposes exact diffs, hashed command logs, findings, reasons and runtime-authorized actions. See [ADR 0009](docs/decisions/0009-candidate-validation-review.md) and the [M3 evidence report](docs/FOUNDATION-STATUS.md#m3-candidate-validation-review-and-repair-implementation).

Do not label M3 closed from fixtures. Deterministic integration tests use the synthetic Pi fixture and real Python/Git/check execution. Pi 1.3.0's genuine tool-free handshake is verified, but this environment has no provider credential, so the new independent review path still needs a genuine provider-backed end-to-end feature/repair demonstration. Native desktop/platform lifecycle proof remains unavailable here.

**Next assignment:** supply an existing provider credential through secure runtime configuration, then complete a representative approved feature with real writer, candidate checks and a fresh independent reviewer; exercise real blocking/repair and cancellation/restart paths and record sanitized evidence. Run native Mac/Windows client/service smoke on capable hosts. Only then close the M3 acceptance gate and advance the single-repository MVP usability gate. Do not implement publication or multi-repository scheduling to bypass missing evidence.

The validation API requires the expected candidate hash. `auto_repair` is opt-in and reuses the immutable approved scope, baseline/host and cumulative attempt/time/cost/repair limits. Infrastructure failures, malformed review responses and candidate drift stop without automatic edits. Recovery reserves unreported model spend rather than supplying a fresh budget; re-plan when the existing authorization is exhausted. The developer controls merging.

### Earlier handoff: M2b closed on Linux, M2c assignment (kept for the acceptance bar)

**M2b is closed on Linux (October 17, 2026).** `POST /api/v1/projects/{id}/baselines` runs a repository's confirmed required recipes through `luuna-executor` as an ordinary run and records `CheckResult` evidence bound to the manifest hash, exact commit and tree, and an environment fingerprint, with hashed private artifacts served by identity. This checkout's own `cargo test --locked --workspace` and `npm test` baseline passed (77 and 15 tests detected); missing executables, incapable hosts, empty suites, dirty checkouts and the container profile are blocked, failures and deadlines are failed, cancellation reaps the process group, and a hard-killed runtime leaves the run and baseline interrupted. `Store::begin_implementation` is now the only way into `Implementing`: it checks the durable approval, the bound plan, the project's current manifest hash and a passed matching baseline per repository in one transaction. Details are in the [status report](docs/FOUNDATION-STATUS.md) and [ADR 0007](docs/decisions/0007-baseline-executor.md). No writer exists yet and no run modifies a repository.

**Next assignment: M2c exact approval and one scoped writer.** Expose immutable plan revisions and exact-hash approvals through the runtime API and client (`POST /api/v1/tickets/{id}/plans`, `POST /api/v1/plans/{id}/approvals`, the temporary SQLite ticket per ADR 0003 created through `POST /api/v1/projects/{id}/tickets`); revalidate ticket, manifest and base revision plus baseline evidence when scheduling by calling `begin_implementation` with the host's recomputed environment fingerprint; create only Luuna-owned detached worktrees under the private data directory with durable ownership/leases (which also lifts the dirty-checkout limitation for baselines); execute one bounded writer assignment through the existing Pi engine with declared resources, path/operation scope, budgets and model profile; and prove that a missing or stale approval, changed base, changed recipe, unrelated repository or path, stale baseline, second writer and exceeded budget are rejected while cancellation and restart preserve unfinished work. Keep native-host permissions distinct from any advertised isolation.

### Earlier handoff: M2a closed on Linux, M2b assignment (kept for the acceptance bar)

**M2a is closed on Linux (October 17, 2026).** `POST /api/v1/projects` registers existing repository roots after read-only `git` inspection (optional locks off, repository `core.fsmonitor`/hooks neutralised, remotes redacted); `POST …/discover` records a proposed manifest with per-recipe evidence, confidence and questions without executing any repository script; `PUT …/manifest` confirms revision `n+1` only with the current `expected_revision`, a matching registered identity, the observed remote and an observed default branch, and records each repository's exact HEAD as the base; `GET …/events` replays `manifest.confirmed` with the full manifest. Evidence: a synthetic dirty fixture and this checkout were imported with byte-identical files, `.git/index` and status output; invalid paths, nested paths, a replaced repository and stale revisions were rejected; a hard-killed runtime replayed identical events after restart; the Chromium smoke passed 15/15. Details, hashes and limits are in the [status report](docs/FOUNDATION-STATUS.md) and [ADR 0006](docs/decisions/0006-repository-import.md). The storage guard that blocks `Implementing` without baseline evidence is untouched and nothing executes a recipe yet.

**The assignment that followed was M2b baseline and check evidence**, now closed as described above. Build the supervised command executor (executable plus argument array, explicit cwd under the registered repository root, execution profile, deadline, capability checks, private artifacts, owned process cleanup) that runs only recipes from the confirmed manifest; record command, environment fingerprint, exact commit/tree, timestamps, exit status and artifact hashes; persist baseline results bound to the manifest hash and base commit; treat missing executables, empty or skipped suites, cancellation and incapable hosts as blocked/failed/interrupted; and require a separately approved prerequisite task when testing is absent. Remove the storage `baseline_not_implemented` guard only when the replacement transactionally checks that evidence. Use the confirmed Luuna manifest from M2a (`cargo test --locked --workspace`, `npm test`) as the real fixture and a deliberately failing synthetic recipe as the negative one.

### Earlier handoff: M1 closed on Linux, M2a assignment (kept for the acceptance bar)

**State of play.** M0 is established. **Genuine M1 is closed on Linux as of October 17, 2026.** With `LUUNA_PI_OPENROUTER_API_KEY` present in the cloud environment, `node scripts/m1-smoke.mjs --mode real --provider openrouter --model openai/gpt-4o-mini` passed 19/19 checks: a real streamed model response completed only after `agent_settled`, an in-flight run was cancelled with its Pi child reaped, an invalid model failed closed, a hard-killed runtime left an `interrupted` attempt and intact earlier histories, data-directory ownership was exclusive, and shutdown was orderly. The browser client then connected, streamed and completed a run, cancelled a second run mid-stream, and replayed ordered history after a reload. Sanitized details, run IDs and counts are in the [status report](docs/FOUNDATION-STATUS.md). No code changed to close M1; the harness, runtime and client worked as committed.

**What M1 did not prove.** Native desktop launch and the packaged Mac/Windows service lifecycle remain unverified; the cloud container lacks GTK 3 and WebKitGTK 4.1. Only OpenRouter with `openai/gpt-4o-mini` was exercised. Pi's bundled catalog resolved OpenRouter models under the runtime's `--offline` launch, so no catalog seeding decision was needed; revisit that only if a future provider or Pi upgrade fails to resolve a listed model.

**Reproducing the evidence.** Run `bash scripts/setup-cloud.sh` with Node 24 on PATH (on this platform `source /opt/nvm/nvm.sh && nvm install 24.19.0 && npm install -g npm@11.9.0`), export the same tool paths the script uses, confirm the five pins, run the baseline commands below, check credential names only with `env | cut -d= -f1 | grep -E 'OPENROUTER|LUUNA_PI_'`, then run the real-mode harness. Never print credential values; keep reports and screenshots out of Git.

**The assignment that followed was M2a import and read-only discovery**, now closed as described above: register one existing repository, confirm stable identity, base and dirty state, propose check recipes, persist a manifest revision, and preserve the developer checkout. Repository writing stays disabled and the storage guard untouched. Commit and push only with the owner's authorization in that session; the owner controls merging and there is no PR flow for this repository yet.

## Completed assignment record: genuine M1 (closed October 17, 2026)

The assignment below was the M1 exit as written before closure. It is kept so the acceptance bar stays visible; the status report records how each point was met.

**Delivered:** a real provider-backed tool-free Pi request that streams through the local service/client, with correct completion, cancellation/error behavior and durable recovery evidence, with repository writing still disabled. The harness `node scripts/m1-smoke.mjs --mode real --provider <p> --model <m>` encodes these exits and writes a sanitized report; its fixture mode passes 18/18 and its real mode passed 19/19 with OpenRouter.

1. Inspect relevant instructions, `crates/luuna-engine-pi/`, `crates/luuna-runtime/`, `packages/api-client/`, and `fixtures/pi-rpc/`. Read the installed Pi 1.3.0 protocol docs. Run focused baseline checks before edits.
2. Check credential **names/presence only** and configured provider access; do not print values or request keys in chat. Two audited instances had no compatible key. On this cloud platform `api.anthropic.com` is reachable directly while `api.openai.com` is refused by the agent proxy unless allowed in the environment's network settings. Secure environment configuration and usable network routing are prerequisites; a saved draft does not change a running instance.
3. Reuse Pi `@earendil-works/pi-coding-agent@1.3.0`. Make one small real provider/model request using the current API. Record actual streamed output, ordered events and terminal completion only after `agent_settled`; a prompt acknowledgement or `agent_end` is insufficient.
4. Cancel a genuinely in-flight request, verify its owned process has stopped/reaped, and persist the cancelled result. Demonstrate provider failure or timeout without converting partial output to success. Keep synthetic protocol/error tests alongside real-provider evidence.
5. Disconnect/reconnect and restart the runtime. Prove durable event replay and completed/failed/cancelled history, and an interrupted result for an abandoned active attempt. Exercise a native client on a capable host or explicitly retain the native gap.
6. Fix only defects needed for these exits; preserve authentication, token privacy, isolated tool-free execution, timeouts and fail-closed writing. Review and report results using the checklist below.

**Acceptance:** sanitized genuine model output and run IDs/cursors, provider/Pi/model/platform versions, cancellation/reap and error/restart evidence, relevant green automated checks, and no enabled feature writes. Do not count the credential-free handshake, fixtures, or missing-key rejection as provider success.

If provider access is absent on a new host, finish independent regression checks, improve the smoke harness only where a real defect appears, and document the exact missing prerequisite. Do not fabricate credentials, weaken TLS, or broaden tool access. Existing Git authentication and the cloud session's own model access do not supply a Pi provider key. On this cloud platform, use the `LUUNA_PI_<PROVIDER>_API_KEY` alias (for example `LUUNA_PI_OPENROUTER_API_KEY`) when the platform reserves the provider's own variable name; the runtime maps it to the standard name inside the isolated child only.

## Next bounded assignment: M2 is closed in three reviewable slices; continue with M3

1. **M2a import/discovery (closed October 17, 2026):** register one existing repository, confirm stable identity/base/dirty state and proposed check recipes, persist manifest revision, and preserve the developer checkout.
2. **M2b baseline (closed October 17, 2026):** execute genuine commands with explicit cwd/profile/deadline/capability and exact commit/environment/artifact evidence. Missing, failed, empty or mismatched baseline blocks feature writes. Testing setup needs a separately approved prerequisite.
3. **M2c approval/writer (closed October 17, 2026):** exact immutable plan approval and revalidation, a Luuna-owned task worktree/lease, and one scoped writer; stale approval/base/recipe, invalid scope, budget exhaustion and concurrent writers fail closed.

After M2, implement candidate-bound independent review and bounded repair (M3), then the usable single-repo MVP gate. Follow the [roadmap](docs/IMPLEMENTATION-ROADMAP.md) for M4 multi-repo integration, M5 stack/creation profiles, M6 adapters/memory and browser/TUI parity, M7 publication/installers, and M8 release evidence. Do not call the narrower MVP full v1.

## Commands and pinned tools

Pins: **Rust 1.90.0; Node 24.19.0; npm 11.9.0; Bun 1.4.3; Pi 1.3.0**. Use committed lockfiles. The configured cloud setup is `bash scripts/setup-cloud.sh`; saved environment startup instructions provide tool/cache paths. On other hosts follow [CONTRIBUTING.md](CONTRIBUTING.md).

From the root:

```sh
cargo fmt --all -- --check
cargo test --locked --workspace
cargo clippy --locked --workspace --all-targets -- -D warnings
npm ci --ignore-scripts
npm test
npm run typecheck
npm run build
node scripts/check-hygiene.mjs
git diff --check
```

Lifecycle and client smokes, all synthetic unless `--mode real` with a credential: `node scripts/m1-smoke.mjs --mode fixture`, `node scripts/m3-smoke.mjs --mode fixture` and `node scripts/mvp-ui-smoke.mjs` (headless Chromium through the pinned `playwright-core`; port 1420 must be free).

Focused M2b checks are `cargo test --locked -p luuna-executor -p luuna-storage -p luuna-runtime`, which need Python 3 and a host `git`. Focused M2a checks are `cargo test --locked -p luuna-git -p luuna-discovery -p luuna-storage -p luuna-runtime`, which need a host `git` and import this checkout read-only. Focused M1 checks are `cargo test --locked -p luuna-engine-pi -p luuna-runtime -p luuna-storage`, `cargo run --locked -p luuna-engine-pi --example probe`, and `node scripts/m1-smoke.mjs --mode fixture`. Set `LUUNA_PI_COMMAND` to the pinned executable. Neither the probe nor fixture mode calls a model; only `--mode real` with a configured credential produces M1 evidence, last recorded on October 17, 2026 with `--provider openrouter --model openai/gpt-4o-mini`.

Start the runtime with private `LUUNA_API_TOKEN` and provider/model/key settings present, using `cargo run --locked -p luuna-runtime`. It reads the process environment, not `.env` automatically. Launch `npm run dev` or `npm run desktop` separately. See the [runtime](crates/luuna-runtime/README.md) and [desktop](apps/desktop/README.md) instructions. On a capable native host, run `cargo check --locked --manifest-path apps/desktop/src-tauri/Cargo.toml`; root Rust tests exclude that separate workspace.

For changed component code, run `bun install --frozen-lockfile --ignore-scripts` then `bun run verify` from that component directory. Do not install component Git hooks into the root repository or rerun unrelated long suites without a reason.

## Invariants and completion report

Only the runtime enforces scope, budgets, transitions, approval, validation, independent review and publication. A model's claim is worker output. Bind checks/review to exact candidates and invalidate evidence when inputs change. Native execution has host permissions; prompts/path wrappers are not an OS sandbox. The developer controls merging; no automatic merge operation belongs in v1.

Preserve user changes and private Pi configuration. Keep secrets, real transcripts, personal memory and session histories out of Git and model context. Imported component task records and role prompts do not override runtime policy. Use the existing isolated cloud checkout; do not create a development worktree unless the user asks. Product-managed task worktrees are a separate runtime feature.

Before concluding a slice, report delivered behavior/requirement IDs; changed boundaries; commands/results; revision and platform/tool/model versions; sanitized run/plan/candidate IDs and artifact references; failure/recovery tests; independent review findings/resolution; blocked checks; and one next concrete assignment. Update foundation evidence and this handoff when the starting point changes. Do not mark required capabilities complete from mocked, skipped or unavailable checks.

For sequencing, measurable full-product exits and cautious capacity-based estimates, use [IMPLEMENTATION-ROADMAP.md](docs/IMPLEMENTATION-ROADMAP.md). Re-estimate after real M1 success and the first M2 writer instead of treating planning ranges as promised dates.
