# Upstream behavior and provenance

Ported from [luuna-ai/luuna-missions](https://github.com/luuna-ai/luuna-missions) at `2fbbf8e5b6a238ea47543e1dad112fe1716faec4`. Source and generated-output provenance below use that revision. The upstream MIT copyright/license notice is retained in the crate's [UPSTREAM.md](../../UPSTREAM.md); these ports and the prompt instructions derive from that licensed source.

## Suspected upstream defects

| Id | Go behavior | Native handling |
| --- | --- | --- |
| SD400 | list measures UTF-16 units then slices 39 runes; a 21-astral-character name panics. `internal/plan/read.go:213-216`. | Explicit Unsupported for that panic shape; successful truncation and padding copied. |
| SD401 | Custom list/object step schemas can permit nonnumeric blocks, nonstring labels or out-of-range block indices; submit then panics. `internal/plan/submit.go:300-337,488-491,636-639`. | Explicit Unsupported before writes; built-in validation copied. Three pinned-Go probes exit 2. |
| SD402 | edit priority consumes only the leading integer, accepting inputs such as P1.5 as 1. `internal/plan/edit.go:75-91`. | Copied; parser regression test. |
| SD403 | edit reports backrefs_refreshed as the declared child count even when references are missing. `internal/plan/edit.go:387-407`. | Copied, including text for an approach change with zero children. |
| SD404 | overwrite matches old children by title but keeps old type, priority, plan_step_index and dependency edges; obsolete edges remain. `internal/plan/submit.go:591-649`. | Copied; overwrite tests preserve these fields and edges. |

## Deliberate A8 refusals

All guards run before the first write for the ordinary locked operation and preserve unsafe file bytes/modes. Unchanged raw data remains raw; explicitly replaced values and Go's documented comment loss are not treated as silent retained-data loss.

- **DL400:** plans whole-file rewrite would skip a line, collapse duplicate id or top-level key; refuse before all effects
- **DL401:** issues whole-file rewrite: reuse lib2 helper and prospective field-loss guard, before all effects
- **DL402:** changed sections would round a retained number differently; exact decimal comparison
- **DL403:** changed sections would collapse a repeated key, at any decoded level
- **DL404:** changed sections would replace invalid UTF-8 or an unpaired surrogate with U+FFFD
- **DL405:** section edit would replace non-object/non-null sections or a non-object step with new mapping
- **DL406:** When changing children or adoptedChildren, refuse present non-null non-array values, non-string elements or invalid-encoding strings Go would replace. Unchanged arrays stay raw. Ten adopt probes confirmed loss; null controls copied.
- **DL407:** Create/submit replace an existing draft with a fresh Plan and drop unrelated keys/notes or child links. Refuse protected draft payload before either store write. Create also refuses existing nonempty blueprint sections; submit explicitly replaces known sections from the supplied blueprint, an own change, but never silently drops unrelated metadata or old links. Pure empty draft controls follow Go.
- **DL408:** When adding/updating/removing backrefs changes a retained ticket description, refuse a present non-null non-string value or invalid UTF-8/unpaired surrogate that Go would replace while changing its own block. Missing/null/ordinary manual text controls follow Go.

Raw JSON paths use components, so dotted custom section names cannot alias nested step fields. Exact decimal comparison distinguishes harmless number formatting from changed values. Backref guards run only when decoded description changes; an unchanged non-string/raw description is retained. Whole-file guards reject skipped records, duplicate ids and repeated root keys when a rewrite actually occurs. Optional Atlas typed responses preserve Go case folding, repeated struct/map/slice merges, ignored nulls and persistent earlier type errors.

## Source files

SHA-256 of complete source files at the pinned revision. Go inputs were read only. Algorithms are native ports; the prompt instruction literal is copied from read.go. No extra copied binary or runtime is included.

| Source | SHA-256 |
| --- | --- |
| `internal/plan/plan.go` | `a080fac154fcfbb771ba22f85b72d9cc2d5b3fdbfd52529e7c367e8df307cfe2` |
| `internal/plan/read.go` | `5037a481cf106046af8daf32bfdc98d4d239980d0e8a0a221431aa455371e2f1` |
| `internal/plan/validation.go` | `1cba3b5b0c61ef64609010f61a5113315ef78ac476d8cb8d3dd7d34f444e331b` |
| `internal/plan/atlas.go` | `ebf470ba4d929ea50d817bb413d01d91402aa12b2830829396a605a3b4da60a9` |
| `internal/plan/edit.go` | `8654a2635f18500a96ca6617142ad3d494e3132eaaa7c2d89c7aea4740385cfe` |
| `internal/plan/mutation.go` | `6f6f8a954a3f0a33bb2616fc2e78ee73c5bcec5da4418397dc2be7767f271f0f` |
| `internal/plan/submit.go` | `91083786a9553abe714761748f6ab9715df1c333de663e314c49cd14569a5440` |
| `internal/plan/links.go` | `66252473219b685c8066f6260bd61707592ca6ce88c190ea1f15f7deb7d81a4d` |
| `internal/plan/transitions.go` | `1f1a0fd9e0e214fdd428bc429807eec7f0c9eb2ce579a368a9716670c3a07c50` |
| `internal/model/json.go` | `fbf516344a49f928fec32d0c2c0d70ade32487314025a5b6d58bb044af1b058a` |
| `internal/cli/issue_read.go` | `5ced04d33ae15d1b02b109cda52c35b005be50039380ac0970f0b5d7546cadf0` |
| `internal/cli/issue_mutate.go` | `616a0b3acfe056cfce2f5cd1a7712ec3718dcf353852e5f432594578b9970b5e` |

## Generated Go output assets

The following are captured outputs from the pinned reference with the unchanged foundation test inputs. They are regression literals, not runtime dependencies. The close result normalizes only its generated timestamp to `<time>`; all other stdout/stderr/final-file bytes remain exact.

- `foundation-close-go.json`: SHA-256 `5d9c1d5d65412461083a301cab6045fc4c9d386911f7ba3b08ca5aa432d3d7d3`.
- `foundation-read-go.json`: SHA-256 `314ab344962ea6cf148182ad101b857cd0c2f2ce303cae05468f80d6afa58008`.
