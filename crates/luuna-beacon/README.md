# luuna-beacon

An independent Rust implementation of Beacon, the schema-driven record store
that originates in Canopy.

## Origin and licence

Beacon comes from Canopy, by Jaymin West, published under the MIT licence at
https://github.com/jayminwest/canopy. The licence text is kept unchanged in
`LICENSE-CANOPY` in this folder. The Go tool is maintained as
`luuna-ai/luuna-beacon`; this crate is measured against that tool at commit
`1c34973a7fba3d0ee1d2aa697f8e1a03cf57f927`.

No Go source is copied into this repository. The crate re-implements the
behaviour in Rust and is checked against the Go tool by tests that build and
run the Go tool as a black box.

## Design

- The store location is a parameter of the library; nothing is hard-coded.
- The crate is measured against the Go tool, so behaviour that looks odd is
  copied and pinned by tests rather than corrected.

## Reference tests

Three test files compare this crate with the Go tool. They are marked
`#[ignore]` because they need a checkout of the reference and a Go toolchain.
Set these variables:

- `LUUNA_BEACON_REFERENCE`: path to a checkout of `luuna-ai/luuna-beacon` at
  the commit above. The tests only read it.
- `LUUNA_BEACON_GO`: path to the `go` binary.
- `LUUNA_BEACON_GOPROXY`: module proxy for the Go build; the default is
  `https://proxy.golang.org`.

The Go build downloads modules into `target/` under the workspace and never
into the reference checkout. Run each file:

```
cargo test -p luuna-beacon --locked --test go_reference -- --ignored
cargo test -p luuna-beacon --locked --test parity -- --ignored
cargo test -p luuna-beacon --locked --test format -- --ignored
```

## Differences from the Go tool

Everywhere else this crate copies the Go tool exactly. It adds two things
the luuna runtime requires and the Go tool does not print. Both are extra
keys in a JSON document the command already prints. No key Go prints is
removed or changed, and nothing new is written to `.luuna/beacon`.

- `resolvedFrom` in the `render <name> --json` document
  (`Rendered::to_payload`): the names of every prompt the resolver used,
  focal prompt first, then the parent's own list, then each mixin's list
  from left to right. Each name is listed once and the list is never
  truncated. The runtime binds every source and its schema into its
  approval, so it needs the full list. `render --format json` and every
  other command are unchanged.
- Top-level `valid`, `errors` and `warnings` in the `validate <name> --json`
  document (`Validation::to_named_payload`), equal to its single entry in
  `results`. The runtime reads these fields from the named form.
  `validate --all` keeps Go's shape.

Lists that Go builds from map iteration, and therefore prints in no fixed
order, are sorted by name here: `list`, `validate --all` and `schema list`
(by name, then id).

## Suspected Go defects

These behaviours of the Go tool look like defects. The crate copies them,
does not fix them, and the tests pin each one.

- GD1: an append to a store file whose last line has no newline is glued
  onto that line, and later reads fail on it.
- GD2: ties on `updatedAt` are broken by comparing the timestamps as text.
  Timestamps whose trimmed fractions differ in length can therefore order
  wrongly.
- GD3: `validate` checks every stored record of a schema, not only the
  current one, so errors repeat once per schema version.
- GD4: `validate` silently skips a stored rule whose pattern does not
  compile.
- GD5: a named `validate` resolves over the whole log, while `render` and
  `show` use current records only. After a rename, `validate <old name>`
  still finds the old identity.
- GD6: in schema rule patterns, `\d`, `\w`, `\s` and `\b` match ASCII only,
  as in Go's `regexp`. This may surprise someone writing rules for
  non-ASCII text.
