# luuna-atlas

A native Rust port of the Luuna Atlas store (.luuna/atlas/). Store roots,
working directories, environment, command flags and input are explicit parameters.
The runtime adapter is still assigned to the later wire plan.

## Commands covered

The foundation provides init, add and config show. The actions modules provide
record, bulk, edit, delete, search and show. The lifecycle module provides query,
rank, ready, status, stale, validate, outcome, move, archive, restore, delete-domain,
prune, compact and config schema/show-path/set/unset. Typed recording APIs are also
available. Git context and configured raw hooks follow the reference behavior.
Prime, learn, diff, sync, active-work context and hook guard/injection are available
under the prime module. The public CLI parser/dispatcher and setup/doctor/audit/bridge
remain assigned to the tooling plan. Coverage and deliberate preservation refusals are recorded in notes/.

## Reference

The Go tool is the reference. The crate is checked against this commit of the Luuna Atlas
clone, and against nothing else:

    c4db80c3f2e622afbdd84f09077fd73b2a92d199

The same commit is the constant in `tests/support/reference.rs`. The reference clone is
read-only. Every git call the harness makes against it sets `GIT_OPTIONAL_LOCKS=0`, so the
clone's index is never refreshed.

## Running the comparison

Two environment variables name the reference:

- `LUUNA_ATLAS_REFERENCE`: the path of the clone at the commit above, with a clean status
  (`git status --porcelain --ignored` empty).
- `LUUNA_ATLAS_GO`: the `go` executable; `go version` must print `go1.27.0`.

If either is missing, wrong or the clone is dirty, the harness panics and names the variable.

The `parity_*` and `format_*` tests are `#[ignore]`d, so a plain `cargo test` reports them as
ignored rather than passed. Run them explicitly, from the workspace root:

    LUUNA_ATLAS_REFERENCE=/path/to/luuna-atlas LUUNA_ATLAS_GO=/path/to/go cargo test -p luuna-atlas --locked --test parity -- --ignored
    LUUNA_ATLAS_REFERENCE=/path/to/luuna-atlas LUUNA_ATLAS_GO=/path/to/go cargo test -p luuna-atlas --locked --test format_compat -- --ignored

Adjust the two paths for your host. `tests/harness.rs` is not ignored and needs only host git.

The harness builds the Go tool offline (vendor mode, `GOPROXY=off`, `GOTOOLCHAIN=local`).
All build output (`GOCACHE`, `GOTMPDIR`, `GOPATH`, `GOMODCACHE`, `HOME` and the binary) lives
under `<workspace>/target/atlas-reference/<commit>/`, which is git-ignored and never
committed. Nothing is written into the clone. Each comparison runs the Go tool (with
`--json --quiet`) and the crate in twin temporary projects, each created with `git init`,
with a cleared environment (`NO_COLOR=1`, `TERM=dumb`, `TZ=UTC`, `LC_ALL=C`, fixed git
identity and date).

## Normalisation

Exactly three rules are applied before output is compared:

1. Each project's absolute path, raw and canonical, becomes `<PROJECT>`.
2. A timestamp in the exact Go form `YYYY-MM-DDTHH:MM:SS.mmmZ`, dated after test start minus
   60 seconds, becomes `<GENERATED-TIMESTAMP>`, whether it is the whole value or embedded.
3. CRLF becomes LF.

Nothing else is normalised. Stdout, stderr, file bytes, exit codes and the file tree (minus
`.git`: paths, file or folder type, permission bits) are compared exactly. A leftover `*.lock`
or `*.tmp.*` file fails the test.

## YAML support

The reader supports block and flow collections, quoted/plain/block scalars,
anchors, aliases, merge keys, explicit/custom tags, binary strings, complex keys,
directives and the first YAML document. It retains invalid UTF-8 binary bytes and
mapping-key identity while JSON uses Go's replacement-character view. The emitter
preserves the reference's formatting and binary encoding.

The graph reader uses exact granit-parser 2.1.0 with the reference's 10000-level
flow/block limits. Cyclic aliases receive a clear refusal. Rewrites preflight
reader-visible content, including additional documents and unsupported lossy
normalizations. Lossless alias/merge expansion is accepted; comments and formatting
changes follow Go. See notes/s5-atlas-lib-3.md for limitations and evidence.

## License and attribution

MIT. This crate is derived from Luuna Atlas and Mulch, both MIT licensed. See
`crates/luuna-atlas/LICENSE` for the license text and copyright notice.
