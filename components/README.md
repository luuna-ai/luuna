# Luuna components

Luuna maintains three MIT-licensed distributions with space-themed identities. Each retains its upstream implementation and record formats behind a separate integration boundary.

| Component | Responsibility | Primary CLI | Short alias | Origin | Data format |
| --- | --- | --- | --- | --- | --- |
| [Luuna Missions](https://github.com/luuna-ai/luuna-missions/blob/6a4a3c3fb80679a77bbf1d7d2da05033fc15460c/README.md) | Tickets and structured plans | luuna-missions | lms | Seeds 0.6.0 | .luuna/missions/ |
| [Luuna Atlas](https://github.com/luuna-ai/luuna-atlas/blob/27477c313a3049b1579dfdc32c39da600726721f/README.md) | Expertise and project memory | luuna-atlas | lat | Mulch 0.11.0 | .luuna/atlas/ |
| [Luuna Beacon](https://github.com/luuna-ai/luuna-beacon/blob/4b01d37522ef448ada78be08cc7a24f7808dfbbb/README.md) | Composable prompts and instructions | luuna-beacon | lbc | Canopy 0.2.6 | .luuna/beacon/ |

[manifest.json](manifest.json) records package names, versions, entrypoints, and compatible data paths for tooling. Packages are private and unpublished. Source lives in the public organization repositories [Luuna Missions](https://github.com/luuna-ai/luuna-missions), [Luuna Atlas](https://github.com/luuna-ai/luuna-atlas), and [Luuna Beacon](https://github.com/luuna-ai/luuna-beacon). These directories are exact-commit Git submodules. The manifest commit and Git gitlink must agree; see [ADR 0012](../docs/decisions/0012-independent-component-repositories.md).

The existing automation boundary is CLI execution with structured JSON output (consult each command's help for its output contract). Atlas additionally exports a TypeScript library through src/index.ts and src/api.ts. Missions and Beacon expose executable CLI entrypoints; importing their package main as a service API would execute command-line code and is not a supported integration.

Luuna's runtime invokes these pinned CLIs through adapters for managed tickets, expertise and prompts. It retains approval, validation and publication policy ownership; component records do not grant runtime authority. Components do not write directly to Luuna's database. See [the current architecture](../docs/CURRENT-ARCHITECTURE.md) for activation and reconciliation boundaries.

Run `npm run components:setup` after cloning Luuna (or use `git clone --recurse-submodules`). Run `npm run components:verify` to check pins and `npm run test:components` for real isolated ticket, knowledge and prompt CLI operations. `npm run components:run -- luuna-missions -- --help` invokes a pinned CLI from your current project directory (including npm --prefix use via INIT_CWD), preserving stdin, stdout, stderr and the component exit status; use luuna-atlas or luuna-beacon for the other tools. This developer launcher is not a runtime adapter and grants no Luuna approval.

`npm run components:stage -- --out <dir> --bun <bun executable>` writes the portable layout the Windows installer ships (the installed trees without Git metadata, `components/dist-manifest.json` with each pin's commit and tree digest, and the pinned Bun); the runtime verifies that layout by content instead of by gitlinks, see [ADR 0024](../docs/decisions/0024-portable-component-distribution.md) and [docs/PACKAGING.md](../docs/PACKAGING.md).

Each component has its own frozen Bun lockfile and can be installed and tested from its directory. Install with `bun install --frozen-lockfile --ignore-scripts`, then run `bun run verify`. The root frontend npm workspace does not resolve or publish these Bun distributions. Preferred executable names are `luuna-missions`/`lms`, `luuna-atlas`/`lat`, and `luuna-beacon`/`lbc`; `lm`, `la`, and `lb` remain package compatibility aliases. The upstream aliases (`sd`, `seeds`, `ml`, `mulch`, `cn`) and `SEEDS_*`/`MULCH_*`/`CANOPY_*` variables are removed in favour of `LUUNA_MISSIONS_*`, `LUUNA_ATLAS_*` and `LUUNA_BEACON_*`. Project data lives under `.luuna/` (`.luuna/missions/`, `.luuna/atlas/` with `config.yaml`, `.luuna/beacon/`); the CLIs and the runtime migrate legacy `.seeds/`, `.mulch/` and `.canopy/` stores automatically. See [ADR 0028](../docs/decisions/0028-luuna-project-component-layout.md).

Upstream LICENSE files, authorship, versions, and provenance are retained in every component. Original source and imported authoring records were preserved outside this checkout before removal. Session histories, authoring stores, and upstream automation instructions are not Luuna project state.

## Updating a component

Make and test source changes in its own repository. After its independent CI passes, fetch and check out the reviewed full commit in Luuna's submodule, update the matching commit in manifest.json, and run components:setup, components:verify and test:components. Commit the gitlink and manifest together for review. Setup uses the committed pin, never --remote; dirty checkouts or incompatible CLI contracts fail visibly. No npm registry publication is required. brodynelly owns component CI and release decisions.

Public component access requires no Luuna credential, deploy key, provider token or private task history. Luuna remains private. Extraction itself did not change ticket authority. The later runtime slice now adds explicit private-store activation, Missions migration/reconciliation and selected Atlas/Beacon plan resources; see [ADR 0014](../docs/decisions/0014-runtime-component-workflows.md). Broader M6 parity and supported-platform evidence remain open.
