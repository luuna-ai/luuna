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
