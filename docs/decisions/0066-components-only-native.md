# ADR 0066: Components are only native

Date: 2026-11-26. Status: accepted. The owner asked for everything native and in one repository before v0.1.0. Completes ADR 0044 for Missions, Atlas and Beacon; follows ADR 0053. Supersedes ADR 0012 and ADR 0024 as practice, not only as rule.

## Context

Since ADR 0053 the runtime and the terminal client call compiled Atlas, Beacon and Missions libraries, and no runtime path used the old JavaScript components. The repository still carried them, though:
- three exact-commit submodules pointing at `luuna-ai/luuna-missions`, `luuna-atlas` and `luuna-beacon`;
- `components/manifest.json` with their pins and a Bun 1.4.3 runtime;
- the `DistributionAudit` API that verified a source checkout or a staged portable layout;
- `scripts/stage-components.mjs`, the legacy `audit-legacy`/`setup-legacy` helpers;
- Bun and component staging in the Windows packager and the Homebrew draft;
- an advisory `bun` item in `doctor`.

A clean macOS install still reported Bun, and archiving the old repositories would have left dangling submodules.

## Decision

1. **Removed:** `.gitmodules`, the `components/atlas`, `components/beacon` and `components/missions` gitlinks, `components/manifest.json`, `scripts/stage-components.mjs` and its test, `scripts/components.test.mjs`, the `components:stage` and `test:component-pins` npm scripts, and the `audit-legacy`/`setup-legacy` actions.
2. **`luuna-components` is only the native adapter.** `DistributionAudit`, `LayoutKind`, `DIST_MANIFEST`, `tree_digest`, the portable digest code, the Bun lookup and the child-process capture that only the audit used are gone. `Components::discover` and the per-store native calls are unchanged.
3. **`doctor` has no `bun` item.** Its last item is `components`, reporting the native libraries. A project that uses Bun in its own recipes still gets a `missing-tool` question from discovery's per-profile probes, as for any tool.
4. **Packaging carries no component trees or Bun.** The Windows packager drops `-SkipComponents`, the Bun install and the staged-tree verification. Its startup scripts no longer set `LUUNA_COMPONENTS_ROOT` or `LUUNA_BUN`. The Windows command-line installer (`scripts/install-cli.ps1`) and launcher (`scripts/start-terminal.ps1`) run `luuna <component>` instead of Bun and sibling checkouts. The Homebrew draft builds `luuna-runtime`, `luuna-tui` and `luuna-sbx` from one pinned commit at or after the native ports (`75e552a`, the v0.1.0 release code) and links the component names to `luuna-tui`; it has no resources and needs neither Bun nor Go.
5. **Developer setup is native.** `scripts/setup-cloud.sh` no longer installs Bun or initializes submodules. The Windows UI smoke edits a Missions ticket with the native `missions update` instead of the Bun entrypoint.
6. **Provenance stays in the repository.** `THIRD-PARTY-NOTICES.md` links the upstream MIT notices kept with each crate (`crates/luuna-atlas/LICENSE`, `crates/luuna-beacon/LICENSE-CANOPY`, `crates/luuna-missions/UPSTREAM.md`). `SOURCE-MANIFEST.json` names the crates.
7. **Saved settings stay readable.** Private settings files may still name `LUUNA_COMPONENTS_ROOT` and `LUUNA_BUN`; they are accepted and ignored, so no existing `environment.json` stops loading.

## Consequences

The old component repositories are no longer referenced by any build, test or packaging step, so they can be archived. A clone needs no `--recurse-submodules`. Historical documents that describe the submodules and the portable layout are records of their time and are not rewritten.

## Evidence

Workspace fmt, Clippy (`-D warnings`) and tests pass; the removed tests covered only the removed audit. The native component tests, the doctor tests (which now assert there is no `bun` item), `npm test` and the Homebrew packaging check pass. The Windows packager and smoke edits are not run here: they need a Windows host.
