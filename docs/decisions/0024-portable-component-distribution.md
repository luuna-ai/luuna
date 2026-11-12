# ADR 0024: portable component and terminal-client distribution

Date: 2026-10-22. Status: accepted for the Windows development installer;
extends ADR 0011 (native bootstrap), ADR 0012 (pinned component
consumption) and ADR 0018 (`doctor`). Addresses audit finding 8 (R01, M7:
the installer did not deliver the integrated product) without changing any
component's source, licence or data contract. (Amended by ADR 0028 for the
data paths and aliases of the staged components.)
ADR 0012 is superseded by [ADR 0044](0044-one-repository-and-agent-shipping.md);
the source-checkout verification and submodule layout this record relies on
(decision 2, decision 4) remain in place until later migration changes
replace them.

## Context

`scripts/package-windows.ps1` staged the desktop, the release runtime, the
helper scripts and the pinned Pi tree, but not the Missions, Atlas and Beacon
trees, Bun, or the terminal client. `luuna-components::Components::discover`
resolved the compile-time source checkout unless `LUUNA_COMPONENTS_ROOT` was
set, and its verifier proved identity only through Git metadata: the staged
gitlink, the committed `components/manifest.json`, the `.gitmodules` URL and a
clean submodule checkout at the pinned commit. A machine that received only
the installer therefore had a runtime that reported the component toolchain
unavailable and no way to run `luuna-tui`. Copying a build checkout to a
target is not a distribution.

## Decision

1. **A portable component layout, verified by content.** `scripts/stage-
   components.mjs stage --out <dir> --root <checkout> --bun <exe>` verifies
   every pin against the gitlinks exactly as `components.mjs verify` does,
   runs `bun install --frozen-lockfile --ignore-scripts` in each component
   checkout with the same root-hooks check as `components.mjs setup`, copies
   each resulting tree without its `.git` link to `<dir>/components/<name>`,
   copies `components/manifest.json` byte for byte, stages the Bun executable
   as `<dir>/bun/bun[.exe]` and writes `<dir>/components/dist-manifest.json`:
   `schemaVersion: 1`, `kind: luuna-portable-components`, the SHA-256 of the
   manifest it was staged with, the Luuna commit, the Bun version with the
   executable's SHA-256, and per component the pinned commit, path, package
   and a tree digest. The digest is deterministic: one line per regular file
   (`<relative path>\0file\0<sha256>`) or symbolic link
   (`<relative path>\0link\0<sha256 of the target>`), paths with `/`
   separators sorted by UTF-8 bytes, joined by `\n` and hashed; empty
   directories contribute nothing and any other entry kind is refused. The
   copy is equivalent to the set-up checkout because the install ran in that
   checkout with the frozen lockfile and no lifecycle scripts, and the digest
   is taken over the copy that ships, not the source.
2. **The runtime accepts both layouts and fails closed on either.**
   `Components::discover` resolves the root from `LUUNA_COMPONENTS_ROOT`,
   else a `components/dist-manifest.json` beside the running executable (the
   packaged `luuna/` folder of ADR 0011), else the compile-time checkout; Bun
   from `LUUNA_BUN`, else the staged `bun/bun[.exe]` of a portable root, else
   the checkout's `.tools` copy, else `PATH`. When the distribution manifest
   is present, discovery checks its schema, the manifest hash, the Bun
   version, that every pin's commit and path agree, that the Bun in use is the
   staged file with the recorded hash, and recomputes every component's tree
   digest; every call revalidates the Bun version and the component's digest
   before running it. A difference is `ComponentError::Distribution`, naming
   the component and the mismatch; nothing degrades to an unverified tree.
   Without the distribution manifest the source-checkout verification of
   ADR 0012 applies unchanged, including on a developer's build checkout.
3. **Diagnostics name the layout.** `luuna-runtime doctor` reports the
   `components` item as the layout kind and root (`source checkout at …` or
   `portable layout at … (distribution manifest digests verified)`) and a
   separate `bun` item with the version and executable path, or the reason
   neither was found. Both stay advisory: the service starts without
   components and refuses component workflows at use, as before.
4. **The Windows bootstrap selects the packaged layout.**
   `scripts/windows-common.ps1` recognises a packaged root (distribution
   manifest and `bun\bun.exe` beside the runtime) and
   `scripts/ensure-windows-runtime.ps1` sets `LUUNA_COMPONENTS_ROOT` and
   `LUUNA_BUN` for the runtime it starts, restoring the caller's environment
   afterwards; a development checkout keeps its submodules and `.tools` Bun.
   The Tauri command of ADR 0011 is unchanged: it already prefers the
   packaged `luuna/` folder and invokes only the fixed helper.
5. **The installer carries the integrated product.** `package-windows.ps1`
   installs Bun 1.4.3 with the same `npm install bun@1.4.3` the setup script
   uses, stages the component layout into the bundle (and refuses to continue
   when the staged or the bundled copy does not verify), and stages the
   terminal client as it exists in the checkout: `packages/tui`,
   `@luuna/api-client` under `node_modules`, and
   `schemas/capability-catalog.json`, with `Luuna Terminal.cmd` running
   `scripts/start-terminal.ps1`, which obtains the connection from the fixed
   startup helper in memory and starts `luuna-tui` on the host Node. Node.js
   24.19 or newer remains a documented host prerequisite (Pi and the
   TypeScript client); no Node is bundled. `-SkipComponents` builds the
   earlier, component-less installer for debugging only.

## Consequences

- Portable identity is content identity: the digest proves the shipped bytes
  are the staged bytes, and the recorded commit links them to the pinned
  public repository, but a portable layout cannot prove by itself that those
  bytes came from that commit. That proof is made on the packaging host,
  where the stager requires the gitlink, manifest and clean checkout to
  agree before staging; the package report records the digests and commits.
- Discovery of a portable layout hashes every staged tree once at startup and
  again before every component call. The trees are small CLI packages; the
  bound is two gigabytes and 200,000 entries per tree.
- The staged Bun must be the one in use: `LUUNA_BUN` pointing elsewhere is
  refused when the distribution manifest records an executable.
- A packaged installation does not change ticket authority, approval, or any
  ADR 0014 guard; it only supplies the verified pinned tools.

## What remains

- **The clean-machine test has not been executed.** `docs/PACKAGING.md`
  records the procedure (fresh Windows host with only the documented
  prerequisites, install, `doctor`, enable components, a Missions/Atlas/Beacon
  workflow, the terminal client) and states explicitly that it has not been
  run in this change; the packager's report records
  `clean_machine_test: not executed`. Until it is, the installer remains a
  development distribution with unverified portable component evidence.
- macOS and Linux packaging have no bootstrap or packager; the portable
  layout and verifier are platform-neutral, the installer is not.
- Signed installers and a host update policy (ADR 0018) are unchanged.
