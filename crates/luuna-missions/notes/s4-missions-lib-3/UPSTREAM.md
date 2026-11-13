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

## Additional observed loss: DL309

`tpl step add` re-encodes the keys of each existing step in the changed template (`internal/model/json.go:125-185`, `:371-392`; reference `2fbbf8e`). A key containing invalid UTF-8 or a lone surrogate escape changes to U+FFFD, whereas the CLI previously returned the raw key inside the unchanged steps. The port refuses before writing: `Refusing to rewrite <path>: line <n> holds template "<id>" whose steps item <k> has a key with invalid UTF-8 or an unpaired surrogate escape, and the Go tool would write it as U+FFFD. Nothing was written.` Test: `command::project::tpl::tests::step_add_refuses_a_key_go_would_reencode_unchanged`, including byte/mode preservation and an unrelated-template control. This extends the original loss list for newly demonstrated A8 loss.

Duplicate keys inside an existing step are DL300 only when that template's steps change. Go retains an unrelated template's raw steps, including repeated step keys; the port keeps that control. Nested unknown values that Go keeps raw remain unchanged.
