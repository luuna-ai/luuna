# ADR 0012: independent public component repositories and pinned consumption

Date: 2026-10-21. Status: accepted by explicit owner instruction. Supersedes ADR 0002's hosted-extraction deferral; its compatibility and attribution requirements remain.

## Context

The owner requested completion of Missions, Atlas and Beacon extraction and continued Luuna consumption, then explicitly requested public repositories. Luuna has concurrent native-platform work in progress; that work must not be included in the extraction commit. Runtime component integration remains a separate M6 obligation.

## Decision

1. Maintain source in public `luuna-ai/luuna-missions`, `luuna-ai/luuna-atlas` and `luuna-ai/luuna-beacon`. The luuna-ai organization owns review, CI and release decisions. Registry packages remain private and unpublished; no npm release credentials or deploy keys are needed for Git source consumption.
2. Preserve the component-scoped maintained history using `git subtree split`, verified against the exact source subtree at Luuna commit `fd42d59c5f608b9120d9cdc2e2354b66f945a5ed`. The earlier pre-branding history remains in Luuna and is identified by UPSTREAM.md. Each repository records extraction provenance in MIGRATION.md. Do not publish monorepo history, authoring stores, sessions, credentials or unrelated Luuna source in these repositories.
3. Preserve TypeScript implementations, package versions, compatible data directories (amended by ADR 0028: `.luuna/<component>`, legacy aliases removed), JSON contracts, primary and legacy aliases, frozen Bun dependency graphs and exact upstream LICENSE bytes. Change only repository metadata, ownership, current instructions and standalone CI during extraction.
4. Consume each repository as an exact-commit Git submodule at its existing `components/<name>` path. `components/manifest.json` records the repository, immutable commit, CLI contract and license digest. The manifest commit and staged gitlink must agree. Setup checks out committed pins, installs with Bun 1.4.3 and `--frozen-lockfile --ignore-scripts`, and checks that root hooks configuration did not change. No floating-branch updates or registry lookalikes are used.
5. Each public repository runs its own full typecheck, instruction checks and Bun suite. Luuna CI installs the pinned submodules and exercises real CLI JSON ticket, knowledge and prompt operations on disposable stores on Linux, macOS and Windows. Consumer checks reject missing, dirty, moved or incompatible checkouts. Public source fetches need no private cross-repository credential.
6. Source copies become gitlinks only after destination identity, remote commit, preserved source/license/lock and actual destination installation have been verified. Retain local rollback copies until the migration is accepted. Changes to Luuna are reviewed through a PR; the developer controls merging.

## Integration boundary

The consumer launcher is a developer and CI tool. It does not replace Luuna's temporary SQLite ticket authority, reconcile external Missions edits, retrieve Atlas private memory, or turn Beacon output into Pi resources. Those M6 adapters must still implement revision, provenance, privacy and runtime approval guards. Extraction must not alter those authorities or weaken any existing policy.

## Validation

See `docs/COMPONENT-EXTRACTION.md` for exact pins, source and license comparisons, independent CI results, consumer checks and observed platform results. Original documentation delivery validation remains separate from this product/dependency evidence.
