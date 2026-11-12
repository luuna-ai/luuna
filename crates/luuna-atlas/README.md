# luuna-atlas

A native Rust port of the Luuna Atlas store (`.luuna/atlas/`). The crate reads and writes the
store byte for byte as the Go tool does. It takes the project root as a parameter, never runs
git, and is not wired into the runtime.

## Commands covered so far

Only `init`, `add` and `config show` (including the legacy `.mulch/` migration). The other
Atlas commands come in later plans.

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

    LUUNA_ATLAS_REFERENCE=/home/brodynelly/luuna-work/luuna-atlas LUUNA_ATLAS_GO=/home/brodynelly/.local/opt/go/bin/go cargo test -p luuna-atlas --locked --test parity -- --ignored
    LUUNA_ATLAS_REFERENCE=/home/brodynelly/luuna-work/luuna-atlas LUUNA_ATLAS_GO=/home/brodynelly/.local/opt/go/bin/go cargo test -p luuna-atlas --locked --test format_compat -- --ignored

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

## Supported YAML subset

No YAML crate is available, so `src/yaml.rs` reads and writes the subset that `config.yaml`
uses: block maps and sequences, empty and simple single-line flow collections, plain,
single-quoted and double-quoted scalars, literal and folded block scalars, comments, and one
leading `---`. Anchors, aliases, merge keys, tags, complex (`?`) keys, directives and several
documents are refused with the typed error `AtlasError::UnsupportedYaml`, which carries the
line number. Anything else it cannot read is a syntax error; the reader never guesses.

## License and attribution

MIT. This crate is derived from Luuna Atlas and Mulch, both MIT licensed. See
`crates/luuna-atlas/LICENSE` for the license text and copyright notice.
