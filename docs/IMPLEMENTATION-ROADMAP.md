# Implementation roadmap and AI handoff

Date: 2026-10-16. This is the implementation sequence from the audited foundation to a usable MVP and full v1. It supplements [the current architecture](CURRENT-ARCHITECTURE.md), not a claim that the planned capabilities already exist.

## Starting point

The three maintained CLI components (Missions, Atlas, Beacon) are consumed today as exact-commit Git submodules under `components/` (ADR 0012). [ADR 0044](decisions/0044-one-repository-and-agent-shipping.md) supersedes ADR 0012: Luuna becomes one repository, Missions, Atlas and Beacon are ported to native Rust with parity tests and Roots is built in, each by its own migration step. Missions, Atlas, and Beacon retain independent package/API boundaries until those ports, and keep compatible data formats and upstream MIT notices. Outside those migration steps, do not create repositories, remove component source, or rewrite these tools to advance a milestone.

[FOUNDATION-STATUS.md](FOUNDATION-STATUS.md) records the observed evidence. Its current Linux baseline is 101 Rust tests, 18 API-client tests, 4 scope-guard tests, 20 positive/negative schema checks, and 3,092 component tests passing, with Rust formatting/lint and frontend type/build checks passing, plus an 18-check synthetic smoke-harness run, the 19-check genuine OpenRouter run, the 15-check M2a browser smoke, a genuine passing baseline of this checkout, the 8-check M2b browser smoke and the genuine M2c writer runs recorded in the status report. Counts describe that audit, not a permanent expected total; future changes must rerun appropriate checks.

| Area | Implemented and checked | Partial or not yet demonstrated |
| --- | --- | --- |
| Repository | MIT/notices, cleaned current tracking, contributor rules, pinned tools/locks; no CI/CD by decision (ADR 0041), checks run locally and are recorded | Historical session files remain in earlier commits; distribution needs separate evidence |
| Domain/storage | Immutable plan hashing, exact approval/revision/project guards, candidate/review guards, transactional state/events, durable SQLite runs, restart interruption | A real baseline executor is absent; storage deliberately rejects entry to feature implementation |
| Pi/runtime | Actual Pi 1.3.0 version/RPC handshake, authenticated loopback run API, replay/SSE, timeout/cancel/reap, malformed/error fixtures, durable failed/cancelled attempts | Genuine provider-backed output observed on Linux on October 17, 2026 (OpenRouter `openai/gpt-4o-mini`, 19/19 real-mode smoke checks); Mac/Windows service lifecycle and native client remain unverified |
| Frontend | Typed client, real service connection, run/history/cancel/reconnect, private in-memory token; browser smoke exercised | Tauri shell exists; native GUI launch, installer, and Mac/Windows results are unverified |
| Maintained CLIs | Independent public Missions, Atlas, Beacon repositories with passing standalone CI, compatible legacy aliases and exact-commit submodule consumption (ADR 0012); ADR 0044 supersedes it as the rule | Runtime ticket/knowledge/prompt adapters are not integrated; packages remain private/unpublished |
| Product workflow | Foundational contracts and negative tests; M2a read-only import, discovery proposals and confirmed manifest revisions with replay; M2b supervised baseline execution with commit/tree/environment-bound evidence and the transactional implementation gate; M2c temporary tickets, immutable plans, exact-hash approvals, Luuna-owned worktree leases and one scoped writer (October 17, 2026) | Candidate validation and independent review (M3), multi-repo scheduling, Missions/Atlas/Beacon adapters, publication, TUI parity and release installation remain planned |

M0 is substantially established, with remaining API/capability/threat-model details completed alongside their first executable consumers. It is not proof of the operational workflow. M1 is closed on Linux as of October 17, 2026; its native-host step is carried by the MVP native-lifecycle item. M2a, M2b and M2c closed later the same day (see the status report). M3 and the MVP gate closed on Linux and Windows, and the M4–M8 slices recorded under each milestone below landed on October 21, 2026 with the gaps each status names; the next assignments are evidence on hosts this one is not (Mac, capable mobile runners, a clean machine, real GitHub), never a shortcut around them.

## What counts as a usable product

A **usable single-repository MVP** imports a real repository, discovers and confirms a meaningful baseline, prepares an exact plan, gets human approval, implements a bounded change in a Luuna-owned worktree, runs real required checks, obtains fresh independent review, and preserves evidence through cancellation or restart. The desktop must show the actual state and allow recovery. It must be demonstrated on the chosen Mac/Windows targets before those targets are advertised.

The MVP can deliver reviewable local commits/diffs without automatic PRs, multi-repository orchestration, all stack profiles, complete memory integration, or a TUI. Those omissions must be explicit.

**Full v1** additionally satisfies all R01–R15 and the [acceptance matrix](11-acceptance.md): multi-repository roles and integration, project creation and supported profiles, scoped memory and component adapters, browser/TUI parity, linked automatic PRs, human-only merging, and supported fresh-host installation. Optional remote workers, NAS hosting, GPU inference, semantic retrieval, team/SaaS features, and a full IDE are later work.

## Milestones and measurable exits

Follow the existing M0–M8 naming. Desktop work accompanies real runtime slices rather than becoming a separate simulated product.

### M1 — Finish genuine engine compatibility

**Depends on:** existing foundation, a supported model/provider, private credentials, and usable provider network access.

1. Inspect the installed pinned Pi protocol docs and current adapter. Reuse the credential-free probe and synthetic fixtures.
2. Complete one small genuine provider-backed request through the authenticated local API. Observe actual text/event streaming and the correct `agent_settled` completion semantics.
3. Cancel another genuine in-flight request and show that the owned process exits/reaps and the final state is cancelled. Exercise provider failure/timeout without promoting partial output to success.
4. Disconnect/reconnect the client and stop/restart the service. Replay ordered persisted events, retain completed/failed/cancelled history, and mark an abandoned active attempt interrupted.
5. Run the native desktop against the same service on a capable host; record any platform gap rather than calling a web build a native launch.

**Exit:** sanitized provider/Pi/model/platform evidence, real output, cancellation/error traces, persistent run IDs/cursors, and unchanged privacy/authentication protections. Missing-key failure already has evidence; it does not substitute for model success. Tool-free success closes connectivity, not repository-writing readiness. Map to R03/R15.

### M2a — Import and read-only discovery

**Depends on:** M1's stable lifecycle and event contract. Read-only fixture/API work can proceed while provider access is being configured.

Register an existing single repository, establish stable identity/remote/base commit, detect dirty state, discover candidate stack/check recipes, and ask the developer to confirm uncertain repository facts. Persist project/manifest revisions and expose them through authenticated API/client contracts. Use canonical `POST /api/v1/projects/{id}/tickets` when ticket creation is introduced.

**Exit:** a real and a synthetic repository import without changing their contents; dirty files remain byte-identical; an invalid or mismatched repository is rejected; replay reconstructs the confirmed map. Discovery proposes commands rather than silently executing arbitrary repository text. This begins R04 and supplies M2 inputs.

**Status (October 17, 2026): closed on Linux.** `luuna-git`, `luuna-discovery`, the project registry in `luuna-storage`, the five project routes, the client and the desktop panel exist with synthetic and real (this checkout) evidence; see [ADR 0006](decisions/0006-repository-import.md) and the status report. Relationships between repositories are accepted by the contract but not discovered; that is M4.

### M2b — Baseline and check evidence

**Depends on:** confirmed repository identity and check recipes from M2a.

Build a supervised command executor with executable/argument arrays, explicit cwd/profile, timeouts, capability checks, private artifacts, and process ownership. Record command, environment, exact commit/tree, timestamps, exit/status, and artifact hashes. Establish required baseline evidence before feature writes. Missing testing setup creates a bounded prerequisite workflow requiring its own approval.

**Exit:** genuine passing and deliberately failing fixture baselines; missing executable, empty/missing test setup, cancelled command, and incapable host remain blocked/failed/interrupted as appropriate. A baseline from another commit/environment cannot unlock a writer. Retain the fail-closed storage guard until this evidence is checked transactionally. Map to R08 and support R13.

**Status (October 17, 2026): closed on Linux.** `luuna-executor`, baseline contracts, storage migration 3 with `begin_implementation`, the baseline routes, client and desktop controls exist; this checkout's own `cargo test --locked --workspace` and `npm test` baseline passed with bound evidence, and the failure, blocked, cancelled and interrupted paths are tested; see [ADR 0007](decisions/0007-baseline-executor.md) and the status report. A dirty checkout is blocked until M2c worktrees exist. **October 22, 2026:** the missing-testing-setup prerequisite workflow (an offered setup ticket, an approved `testing_setup` plan admitted without a baseline, a real-test pass rule) exists; see [ADR 0022](decisions/0022-testing-setup-prerequisite.md).

### M2c — Exact approval and one scoped writer

**Depends on:** M2b; existing plan/approval domain and SQLite guards.

Expose immutable plan revisions and exact-hash approvals through runtime/API/client. Revalidate ticket/manifest/base revision and baseline evidence when scheduling. Create only Luuna-owned branches/worktrees with durable ownership/leases. Execute one bounded writer assignment with declared resources, operation scope, budgets, and model profile. Separate native-host permissions from any advertised isolation mechanism.

**Exit:** one approved fixture feature changes only its managed worktree; the developer checkout is preserved. Missing/stale approval, changed base, changed recipe, unrelated repository/path, stale baseline, second writer, and exceeded budget are rejected. Cancellation/restart preserves unfinished work and invalidates uncertain evidence. Map to R07/R08. A prompt that promises to obey paths is not an OS security boundary.

**Status (October 17, 2026): closed on Linux.** Temporary local tickets (ADR 0003/0005), runtime-built immutable plan revisions, exact-hash approvals, `begin_implementation` revalidation of ticket, manifest, base, baseline and host fingerprint, worktree leases with one active lease per repository, attempts counted against the budget, the Pi writer profile with the Luuna scope guard, post-session scope judgement of the worktree diff and candidate commits exist; every exit rejection, cancellation and restart is tested, and a genuine pinned-Pi writer through a real provider is recorded in the status report; see [ADR 0008](decisions/0008-approval-worktrees-writer.md). The Missions adapter replaces the temporary ticket in M6; baselines still run on the developer checkout, so a dirty checkout still blocks a baseline.

### M3 — Candidate checks, independent review, repair and recovery

**Depends on:** a real M2 writer and command evidence.

Freeze exact candidate commit/tree identities; bind required checks and structured reviewer results to them. Start the reviewer in a new session with requirements, current code/diff, and evidence, excluding the implementer's conversation. Classify infrastructure failures separately, aggregate attempt/time/cost usage, bound repairs, and pause usefully when exhausted. Every repair/manual edit invalidates affected evidence and review.

**Exit:** a seeded regression is blocked; a stale candidate/review cannot pass; repair exhaustion persists with artifacts; restart does not reset budgets or duplicate work; cancellation covers owned descendants when shell tools are enabled. Demonstrate a complete approved single-repo feature with real checks and independent review. Map to R09/R10.

**Implementation status (October 17, 2026):** candidate command checks, fresh read-only Pi reviewer sessions, structured review bindings, bounded manual/opt-in automatic repair, aggregate budgets, restart recovery and desktop diff/evidence views are implemented and fixture-tested. **The genuine-provider exit is closed on Linux** by `scripts/m3-smoke.mjs` through OpenRouter with pinned Pi 1.3.0: real writer, real required checks, fresh independent reviewer, seeded failure → repair r2 → fresh evidence, bounded automatic repair pausing at the attempt budget, cancellation and hard-kill recovery with spend reservation. A real run on the MVP code (`fd42d59`, October 21, 2026) passed 61/61 again. Native Mac/Windows proof remains open, so M3 is not declared fully closed. See [ADR 0009](decisions/0009-candidate-validation-review.md) and the status report.

### MVP gate — Make the single-repository workflow usable

**Depends on:** M1–M3. Develop screens against those working APIs, not alternative frontend state machines.

Complete project import, baseline, plan approval, live execution, diff/evidence/review, history, cancel and recovery flows. Test keyboard access and clear blocked/failure messages. Exercise desktop-to-service reconnection and explicit service shutdown. Smoke native Mac/Windows hosts with declared prerequisites; test backup/migration recovery and document manual service startup limitations.

**Exit:** another developer follows the documented setup and completes the representative fixture workflow without undocumented database edits or fake data. Publish a requirement/evidence report naming MVP omissions. A native package may be a development distribution; signed fresh-host installation remains M7 evidence.

**Status (October 21, 2026): the cloud-doable part is closed on Linux; the native-host part remains.** A candidate whose required checks passed but whose reviewer left no verdict can be re-validated in place as a fresh generation of the same revision under the same approval and budgets (ADR 0009, point 7). An orderly shutdown now records interruption, never cancellation (ADR 0004 note). The desktop client shows outages, reconnects and reloads by itself, names the next allowed action with every refusal and recorded reason, and is operable by keyboard; `node scripts/mvp-ui-smoke.mjs` drives it through the documented [walkthrough](MVP-WALKTHROUGH.md) with recovery, refusal, keyboard and accessibility checks, and [MVP-EVIDENCE.md](MVP-EVIDENCE.md) is the requirement/evidence report naming the omissions. **Later on October 21, 2026, on Windows:** the backup/migration recovery drill is scripted (`node scripts/backup-recovery-drill.mjs`, 31/31 on this host, walkthrough step 7b) and the Windows native smoke is covered by the PR 11 bundled-WebView2 lifecycle run plus `scripts/package-windows.ps1`; the Mac smoke remains open for lack of a host.

### M4 — Multi-repository coordination

**Depends on:** M3's reliable single-repository execution and candidate guards.

Add confirmed repository relationships, task DAG scheduling, scoped project/repository/specialist roles, leases, bounded concurrency, dependency overrides, and cross-repository candidate sets. Preserve one authoritative runtime.

**Exit:** an API/shared-package/consumer fixture produces three exact candidate commits, independent work runs within limits, incompatible consumer integration blocks completion, and partial failure/restart preserves the graph and evidence. Map to R05/R06; single-repository check success cannot stand in for integration success.

**Status (October 21, 2026): closed with the synthetic writer on Windows.** Validated task graphs, relationship proposals and confirmation, durable per-task state, the dependency-ordered scheduler with `max_workers`, cross-repository candidate sets with bindings, per-repository validation with a combined review diff, integration blocking, and resumption by task exist and are tested by the three-repository fixture (`crates/luuna-runtime/tests/multirepo.rs`, 9 scenarios); see [ADR 0017](decisions/0017-multi-repository-coordination.md) and the status report. Open: delegated roles (P42), dependency overrides (P44), repair of a multi-repository set (re-planning by decision) and a genuine-provider multi-repository feature. **October 22, 2026:** P42 and P44 are implemented for v1 ([ADR 0023](decisions/0023-delegated-roles-and-dependency-overrides.md)); a genuine-provider multi-repository feature remains open.

### M5 — New projects and verified stack profiles

**Depends on:** M2's executor/baseline contracts; end-to-end acceptance uses M3/M4 as appropriate.

Add approved scaffolding and check profiles for React/Next.js/TypeScript, Node, Go, Python, Flutter, native Android and iOS. Probe host capabilities; let the developer confirm discovered recipes.

**Exit:** every advertised profile has a genuine scaffold/import baseline on a capable runner. Required Apple/device/emulator checks have real evidence or explicitly block that workflow. Do not call an unavailable platform passing. Map to R04/R13.

**Status (October 21, 2026): implemented; closed only for the profiles this Windows host can run.** Host capability probing, capability-annotated discovery for every advertised profile, and hashed, confirmed scaffolding with a real baseline exist ([ADR 0019](decisions/0019-stack-profiles-and-scaffolding.md)); Node, TypeScript, Python and Rust scaffolds ran passing baselines here, Go, Flutter, Android and iOS are proved only as correctly blocked proposals and annotated discoveries. Capable-runner evidence for those four and the Mac-only checks remain the exit.

### M6 — Integrated components, scoped memory and client parity

**Depends on:** stable project/ticket/run contracts; client parity acceptance depends on the implemented product capabilities.

Integrate Missions tickets/dependencies with serialized revision checks; migrate any temporary SQLite ticket authority into a reconciled projection. Add Atlas user/project/process memory with provenance, isolation, promotion/deletion and lexical retrieval. Render versioned Beacon prompts into explicit Pi resources. Use compatible APIs/data formats rather than independent writable copies or imported historical task records.

Complete browser and TUI actions against a shared capability catalog, alongside the desktop. Include plans, approval, repository views, diff/review, settings, memory, cancel, recovery and publication controls.

**Exit:** external Missions edits cause a detected conflict; stale prompts/schema mismatches fail visibly; private memory never crosses project/export scope; every cataloged action and failure path has parity evidence; reconnect yields the same authoritative state. Map to R02/R14/R15.

**Status (October 21, 2026): closed for the cataloged actions on Windows and Linux.** Missions migration and reconciliation and selected Atlas/Beacon context (ADR 0014), memory promotion and the scope-safe export (ADR 0021), the capability catalog, shared failure explanations and the terminal client with parity tests and `scripts/tui-smoke.mjs` (ADR 0020) exist; see the status report. Open: a keyboard audit and tones for the interactive TUI, import of an export, memory retention policy. **October 22, 2026:** import of an export, ticket editing, memory correction, personal export, retention policy and behaviour-level multi-repository client parity exist ([ADR 0021](decisions/0021-memory-promotion-and-project-export.md) amendment, [ADR 0026](decisions/0026-ticket-and-memory-controls-and-client-parity.md)); later the same day the experience surfaces of [ADR 0027](decisions/0027-v1-advertised-scope.md) point 4 (Home, Kanban, Help, editor handoff, settings as the doctor report) and the interactive TUI audit of point 5 (`scripts/tui-interactive-audit.mjs` with the terminal tones, [ADR 0020](decisions/0020-capability-catalog-and-tui.md) third amendment) were delivered, and the six points were accepted on the maintainer's best estimate.

### M7 — Publication and portable installation

**Depends on:** M3 evidence gates, M4 candidate sets, and a verified publication credential mechanism.

Implement push/linked PR creation, durable idempotency and remote reconciliation, partial-success recovery, CI observation and human-controlled merge tracking. Ship tested installers/doctor diagnostics and a host-specific independent-service/token/update strategy. Current permission to push this repository is not proof the future product publisher exists.

**Exit:** approved three-repo candidates create linked PRs once, timeout-after-success and partial publication recover without duplicates, failing CI is distinct from integration, and no automatic merge path exists. Clean supported Mac/Windows computers install, configure and run the fixture without NAS/cloud dependence. Map to R01/R11/R12.

**Status (October 21, 2026): implemented against a fake GitHub API and a local remote; installer built unsigned on Windows.** Publication with a durable intent ledger, reconciliation, CI and merge observation and no merge path, plus `luuna-runtime doctor`, exist and pass six scenarios ([ADR 0018](decisions/0018-publication-and-doctor.md)); `scripts/package-windows.ps1` produces an NSIS development installer carrying the runtime, helper scripts and the pinned Pi tree. Open: real GitHub evidence, a clean-machine Windows installation, Mac packaging, signing. **October 22, 2026:** publication revalidates every input before pushing with a deterministic pre-push policy, CI-failure repair and linked three-repository publication are tested against the fake API, and the package stages the portable components, Bun and the terminal client ([ADR 0018](decisions/0018-publication-and-doctor.md) amendments, [ADR 0024](decisions/0024-portable-component-distribution.md)); real GitHub, the clean-machine test, Mac and signing remain open.

### M8 — Full-v1 release evidence

**Depends on:** M4–M7 and every advertised capability's prerequisites.

Run all A01–A34 scenarios, supported-host/version matrix, real-project pilot, failure/recovery drills, migrations/backups, dependency/security review and performance measurements. Inspect resource growth, cancellation latency, event delivery and usage accounting; define targets from observed measurements.

**Exit:** every R01–R15 requirement has passing linked evidence on relevant hosts; unresolved required checks block the v1 label. Document limitations, credential/retention boundaries, installer provenance and reproducible recovery. No required path depends on synthetic model output.

**Status (October 21, 2026): report produced; the v1 label is not granted.** [RELEASE-EVIDENCE.md](RELEASE-EVIDENCE.md) links every requirement and scenario to the evidence that exists, `node scripts/release-evidence.mjs` runs the checks and harnesses into one requirement-mapped report, and the blocking items (Mac, clean-machine installation, real GitHub, capable runners for Flutter/Android/iOS, a genuine run on the current tree, a real-project pilot) are named there.

## Requirement coverage

| Requirement | Delivery/evidence ownership |
| --- | --- |
| R01 Local Mac/Windows operation | MVP native smoke, M7 fresh-host installs, M8 release matrix |
| R02 Browser/TUI capability parity | M6 catalog and parity suite, M8 |
| R03 Pi with selected models | M1 genuine engine/model compatibility, repeated after upgrades |
| R04 Import and creation | M2a import; M5 approved project scaffolding |
| R05 Multiple repositories | M4 confirmed graph and integration candidate |
| R06 Scoped coordination roles | M4 scheduler/assignment traces |
| R07 Approval before feature writes | M2c runtime negative cases, maintained through M8 |
| R08 Meaningful passing baseline | M2b prerequisite/baseline evidence |
| R09 Real checks and fresh review | M3 candidate-bound command/reviewer results |
| R10 Bounded repair and useful pause | M3 aggregate budgets and recovery drills |
| R11 Automatic linked PRs | M7 actual remote publication/reconciliation |
| R12 Developer controls merging | M7/M8 no automatic merge operation |
| R13 Supported stack profiles | M5 capable-host matrix; unavailable required checks block |
| R14 Scoped memory | M6 privacy/provenance/deletion tests |
| R15 Runtime outlives clients | M1 reconnect/restart, MVP native lifecycle, M7 packaged service |

## Implementation process and evidence

Make each M2 substage and each subsequent bounded behavior independently reviewable. Before coding, inspect current code, applicable instructions, relevant schema/API/role contract, and baseline evidence. Describe the behavior, affected files/boundaries, negative cases, and exit command; do not ask the user to repeat accepted architecture decisions.

Implement runtime policy and durable transitions before connecting client actions. Use synthetic fixtures for deterministic failure cases and real tools/providers for compatibility evidence. Run relevant checks, get independent review, fix material findings, and update status and handoff before advancing. Add crates/packages when a real implementation needs them. Until the migration steps of ADR 0044 replace them, consume the components through the exact-commit submodules first set by ADR 0012.

Each completion report must identify:

- Revision and dirty-state context; delivered behavior and requirement IDs.
- Commands actually run, exit results, counts, OS/architecture and pinned tool/provider/model versions.
- Run/attempt IDs, event cursors, exact plan/candidate hashes, and sanitized artifact locations.
- Negative/failure/recovery cases exercised; independent reviewer findings and resolution.
- Passing, failing, blocked and unavailable checks separately, with the next concrete assignment.

Commit only sanitized summaries, synthetic fixtures and useful reproducible tests. Keep tokens, keys, personal paths, private provider transcripts and raw local state out of Git. Record private evidence references without copying secrets. A documentation report cannot replace missing execution evidence.

## Reproducible starting commands

Pins: Rust **1.90.0**, Node **24.19.0**, npm **11.9.0**, Bun **1.4.3**, Pi **1.3.0**. Root and native Cargo workspaces have separate locks; root npm, Pi and each Bun component also have their own committed locks.

For the configured cloud checkout, `bash scripts/setup-cloud.sh` installs frozen dependencies and builds the runtime without component lifecycle hooks. Use the saved environment startup instructions for tool/cache paths. On other hosts follow [CONTRIBUTING.md](../CONTRIBUTING.md) and platform prerequisites rather than assuming `/workspace` exists.

From the repository root:

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

For a changed component, run in its own directory:

```sh
bun install --frozen-lockfile --ignore-scripts
bun run verify
```

Run the native check on a capable host; root Rust checks exclude that workspace:

```sh
cargo check --locked --manifest-path apps/desktop/src-tauri/Cargo.toml
```

With `LUUNA_PI_COMMAND` pointing to the installed pinned executable:

```sh
cargo run --locked -p luuna-engine-pi --example probe
```

That probe makes **no model request**. For genuine M1 proof, securely set the provider credential (or its `LUUNA_PI_<NAME>` alias) in the runtime environment, build the runtime, and run `node scripts/m1-smoke.mjs --mode real --provider <provider> --model <model>`, which starts the runtime itself and writes a sanitized report; or start `cargo run --locked -p luuna-runtime` manually with `LUUNA_API_TOKEN`, provider and model set. Launch `npm run dev` or `npm run desktop` separately. The runtime does not automatically load `.env`. See the [runtime README](../crates/luuna-runtime/README.md), [desktop README](../apps/desktop/README.md), and [Pi fixtures](../fixtures/pi-rpc/README.md).

## Planning ranges, assumptions and critical path

These are engineering estimates from this foundation, **not promised dates**. Assume one experienced full-time engineer using AI assistance, about 40 scheduled hours/week with **30 focused engineering hours**, a stable model/provider budget, Mac and Windows capable hosts, and timely feedback. AI assistance is included in the assumption; it does not remove integration, review, native packaging, or failure-testing work. Waiting for credentials, network changes, host access, signing, or external service approval adds elapsed time.

The earlier desktop proposal estimated **372–600 hours from scratch** for a meaningful single-repository MVP. Some bootstrap work now exists and its model path was proved on October 17, 2026; its native paths remain unproven. Use the following remaining-work range provisionally and re-estimate after the first real M2 writer.

| Remaining work to a usable single-repo MVP | Focused hours before contingency |
| --- | ---: |
| Genuine M1 compatibility and evidence (Linux closed October 17, 2026; native-host step remains) | 0–8 |
| M2c approval revalidation, worktree ownership and scoped writer (closed on Linux October 17, 2026; native-host proof remains) | 0–8 |
| M3 independent review, repair and recovery | 64–104 |
| MVP client completion, native smoke and end-to-end hardening | 64–104 |
| Subtotal | 128–224 |
| Technical uncertainty allowance, approximately 20% | 26–45 |
| **Planning range** | **154–269** |

That is about **4–7 full 40-hour engineering weeks**, or **5–9 elapsed workweeks at 30 focused hours/week** under these assumptions. A connectivity preview can arrive sooner; it is not the usable MVP defined above.

| Additional work after the MVP to full v1 | Focused hours before contingency |
| --- | ---: |
| M4 multi-repository scheduler and integration | 96–160 |
| M5 creation and complete stack-profile matrix | 128–224 |
| M6 component/memory integration and browser/TUI parity | 128–224 |
| M7 linked publication, service packaging and installers | 96–168 |
| M8 pilot, acceptance, security/recovery and release hardening | 80–144 |
| Subtotal | 528–920 |
| Technical uncertainty allowance, approximately 20% | 106–184 |
| **Additional planning range** | **640–1,110** |

The combined remaining full-v1 range is roughly **790–1,380 focused hours**, about **20–35 full engineering weeks** or **26–46 elapsed workweeks at the assumed focused capacity**. Treat the upper range seriously: native/mobile profiles, TUI parity, independent service installation and publication recovery are substantial product work. Requirements changes or missing platforms can exceed this range.

The execution critical path is **genuine M1 → import/baseline → exact approval/scoped writer → candidate checks/fresh review → reliable multi-repo integration**. Full-v1 release also depends on client parity, profile/platform proof and publication/installers. Stack/client contracts can be prepared alongside core work once stable, but one engineer's parallel tasks do not create extra capacity. Re-estimate from measured milestone throughput and preserve required gates; if time is short, narrow the MVP scope explicitly rather than declaring full v1 early.
