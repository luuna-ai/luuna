# Upstream notes of s4-missions-lib-3

## Suspected Go defects found by s4-missions-lib-3

Ids: SD300 to SD399.

| Id | Behaviour | Evidence | Ported in |
| --- | --- | --- | --- |

## Deliberate differences from Go (charter A8, Go data loss)

Ids: DL300 to DL399. Where the Go tool silently loses the user's data, this plan refuses the input with a clear error and leaves the file untouched; every other Go behaviour is copied.

(none yet)

## Assets copied from the reference

Assets go in `crates/luuna-missions/assets/s4-missions-lib-3/`.

(none yet)

## Scalar read-back clarification

A literal `<<` used as a scalar value is preserved by the pinned Go tool: `config set project << --json` writes `project: <<`, and `config show --json` returns the string `<<`. It is copied because there is no value loss. This differs from `<<` used as a mapping merge key, where a rewrite must still preserve the key and value or refuse. The emitter read-back tests cover these roles separately.
