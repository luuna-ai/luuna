# Third-party source notices

Luuna's original code is MIT licensed. The maintained components originate from
Jaymin West's MIT-licensed os-eco tools:

| Luuna distribution | Upstream | Imported version | License |
| --- | --- | --- | --- |
| Luuna Missions | https://github.com/jayminwest/seeds | 0.6.0 | [MIT](https://github.com/brodynelly/luuna-missions/blob/5832801137dc1189915340c1f6076516b34e21b1/LICENSE) |
| Luuna Atlas | https://github.com/jayminwest/mulch | 0.11.0 | [MIT](https://github.com/brodynelly/luuna-atlas/blob/7c04005c5541e7efdb652d834728602a8dcb4d29/LICENSE) |
| Luuna Beacon | https://github.com/jayminwest/canopy | 0.2.6 | [MIT](https://github.com/brodynelly/luuna-beacon/blob/ae7741fe84df39c59a0081e8f56b13ef567bcce8/LICENSE) |
| Luuna Roots | https://github.com/jayminwest/roots | commit 9a7906b (no release version) | [MIT](roots/LICENSE) |

The upstream copyright notices remain in each component. Renaming a distribution
does not imply upstream endorsement or change the compatible data formats. The
initial import was a source copy; upstream commit history was not included in the
original Luuna commit. Original source remains recoverable from that Git history.

Luuna Roots, unlike the three components, was imported into `roots/` with its
complete Git history, including the upstream history. Its MIT license and Jaymin
West's copyright notice are in `roots/LICENSE`, and its provenance is in
`roots/UPSTREAM.md`.

Pi remains an external pinned dependency, not a rewritten or bundled agent engine.
Other dependencies retain their own licenses in their package distributions and
lockfiles. This file records source provenance, not a complete legal dependency audit.

## Design reference

- `docs/design-reference/ledger/` is a byte-for-byte copy of the project owner's Ledger design system (copied 2026-10-22, adopted by ADR 0033). The source contained no license file, so it is treated as the owner's own material and is **not** covered by Luuna's MIT grant.
- It is documentation and reference data only: nothing under it is built, imported, tested or shipped in the desktop app. See `docs/design-reference/README.md`.

## Bundled fonts

- **Outfit** is bundled: two variable woff2 subsets (latin and latin-ext, weights 100 to 900) in `packages/ui/src/assets/fonts/outfit/`, with the license text in `OFL.txt`. Copyright 2021 The Outfit Project Authors (https://github.com/Outfitio/Outfit-Fonts). Licensed under the SIL Open Font License, Version 1.1 (https://scripts.sil.org/OFL). It is used for headings, titles and display only (ADR 0035). The OFL permits bundling and redistribution with software provided the license stays with the font and the font is not sold by itself. Outfit is not covered by Luuna's MIT grant.
- **Mantine** (https://mantine.dev, `@mantine/core` 9.7.3, MIT): only design-token values (colours, type scale, radius, spacing, shadows, breakpoints) are used, recorded in `specs/mantine-theme-values.md` and `packages/ui/src/design/mantine-values.ts`. No Mantine source code or stylesheet is vendored, and no endorsement is implied.
- **Söhne** is a commercial typeface by Klim Type Foundry (https://klim.co.nz). It is **no longer bundled or referenced**: ADR 0035 removed every reference from the stylesheets and tests. The ten hinted TrueType files from the sole user's installed copy were deleted from the working tree on 2026-10-22 at the owner's direction. As checked on 2026-10-26, no commit reachable from `main` contains a Söhne font file. The only remaining copies are ten Söhne woff2 files in commit `097758f`, reachable from the branch `orchestrate/stripe-ui-20261022` (PR #18, closed as superseded) and GitHub's `refs/pull/18/*`. Deleting that branch, and asking GitHub to drop the pull request's refs, is the owner's decision.
- Söhne is **not** covered by Luuna's MIT license or any Luuna license grant. The `Test Söhne Mono` trial files must never be included.
- The Söhne files must be removed or properly licensed before any distribution, publication or packaging of the repository or desktop app. Pushing this repository to a shared or public remote counts as publication: while that branch exists on a remote, the woff2 files are published with it.
- Body and UI text use Mantine's system font stack; no system font is bundled.
