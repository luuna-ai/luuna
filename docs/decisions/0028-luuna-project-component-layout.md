# ADR 0028: Luuna project layout and removal of upstream identities from the component CLIs

Date: 2026-10-22. Status: accepted; amends ADR 0002 (compatibility aliases and
compatible data paths), ADR 0012 (primary and legacy aliases, compatible data
directories) and ADR 0014 and ADR 0024 (the managed store layout they
describe). The same branch pins the component commits that implement it
(Missions `bda5f31`, Atlas `3296243`, Beacon `e103e01`).
ADR 0012, which this record amends, is superseded by
[ADR 0044](0044-one-repository-and-agent-shipping.md); the layout decided
here is unchanged.

## Context

Missions, Atlas and Beacon began as rebrands of Seeds, Mulch and Canopy and
kept the upstream data directories, legacy executable names and environment
variables so that existing integrations stayed usable. They are now
independently maintained Luuna distributions (ADR 0012). Leaving three
differently named dot-directories in every project, plus `sd`, `seeds`, `ml`,
`mulch`, `cn` and `SEEDS_*`, `MULCH_*`, `CANOPY_*` variables, kept the upstream
identities in the product surface without serving a Luuna user.

## Decision

1. **One project data root.** Project data lives under `.luuna/`:
   `.luuna/missions/` (`config.yaml`, `issues.jsonl`), `.luuna/atlas/`
   (`config.yaml`, which replaces `mulch.config.yaml`) and `.luuna/beacon/`
   (`config.yaml`). JSONL record formats are unchanged.
2. **Legacy aliases and variables are removed.** Missions drops `sd` and
   `seeds`, Atlas drops `ml` and `mulch`, Beacon drops `cn`. The environment
   variables are `LUUNA_MISSIONS_*`, `LUUNA_ATLAS_*` and `LUUNA_BEACON_*`. The
   kept names are `luuna-missions`/`lm`, `luuna-atlas`/`la` and
   `luuna-beacon`/`lb`. The manifest keeps `cli.legacy` as an empty list so
   the schema is stable, and the verifiers still check any alias listed there.
3. **Legacy stores migrate automatically.** Each component CLI moves a legacy
   directory to its new location with one stderr notice and fails when both
   exist. The runtime does the same for managed workspaces created by earlier
   versions (`luuna-components::adopt_legacy_layout`) before any existence
   check or call: if the new directory is absent and the legacy one exists it
   is renamed, Atlas's config file is renamed, and a repeat is a no-op. A store
   present at both locations is never merged. The legacy names live in one
   constant (`LEGACY_LAYOUT`) so the shim can be deleted later.
4. **Link safety is unchanged and extended.** `.luuna` itself must be a real
   directory, never a link or file, in addition to the existing no-link walk of
   the component store; a linked legacy directory is refused, not followed.
5. **Attribution is retained by design.** Licence files, `UPSTREAM.md`,
   `THIRD-PARTY-NOTICES.md` and the README origin sections keep the Seeds,
   Mulch and Canopy names, versions and copyright notices (ADR 0002). The
   manifest `upstream` and `sourceVersion` fields stay as provenance.

## Consequences

- The runtime's managed workspaces (`<data_dir>/components/projects/<id>`)
  hold `.luuna/missions`, `.luuna/atlas` and `.luuna/beacon`. This is distinct
  from the runtime data directory, whose default is also `.luuna` relative to
  the working directory. The existing refusal of a data directory inside a
  registered checkout (`projects.rs`) compares the data
  directory path with the repository root only; a component store at
  `.luuna/<component>` inside a checkout is not that case and is not
  inspected by it. Runtime data and component stores never share a directory
  because managed stores sit below `components/`.
- The root `.gitignore` already ignores `.luuna/`, which covers all three
  stores; the old per-upstream entries are removed.
- `npm run components:verify` fails until the three submodule pins are
  updated to commits that publish the new bin mappings and layout; the pins
  are changed separately from this record.
- Integration snippets using the removed aliases or variables must be
  updated; there is no compatibility alias.

## Preferred short aliases (2026-10-22 amendment)

The preferred short commands are now `lms` for Missions, `lat` for Atlas and
`lbc` for Beacon. Their package binaries retain `lm`, `la` and `lb` for
compatibility with Luuna installations created before this preference change.
The manifest `cli.short` records the preferred alias, while `cli.legacy`
continues to describe removed upstream identities rather than these Luuna
compatibility aliases.
