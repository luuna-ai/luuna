# luuna-missions

A native Rust library that reads and writes a Missions store (`.luuna/missions/`) exactly as the Go `luuna-missions` tool does. It is ported from the Go tool at commit `2fbbf8e5b6a238ea47543e1dad112fe1716faec4`. See [UPSTREAM.md](UPSTREAM.md) for provenance, the license notice, and the Go behaviour copied on purpose.

This crate is a library only. It has no binary, and nothing in the workspace uses it yet. The package name matches the name of the Go executable, but the crate provides no `luuna-missions` command.

## What it does

- **`Store`** opens a store directory that you name, or finds one by walking up from a working directory as Go does. It reads tickets and takes Go-compatible lock files.
- **`Ticket`** is one record of `issues.jsonl`. The API calls records tickets; on disk they keep Go's names. A ticket keeps its raw JSON, its key order and any unknown fields.
- **`command::run`** runs one command as a library call. It takes an `Invocation` (arguments, working directory, stdin, environment, and an optional explicit store directory) and returns an `Outcome` with the stdout, stderr and exit code Go would produce. Errors that Go prints to its user come back as an `Outcome` with exit code 1. `Err` means the library failed, or the request needs behaviour that is not ported yet (`Error::Unsupported`, which names the plan that ports it).

This plan (s4-missions-lib) ports the commands luuna-runtime uses: `init`, `create`, `show`, `list`, `search`, `update` and `block`. Later plans port the rest. Each has its own notes, which hold what it ports, what stays refused until then, the files it owns and the contract of its check functions:

- [s4-missions-lib-2](notes/s4-missions-lib-2/README.md)
- [s4-missions-lib-3](notes/s4-missions-lib-3/README.md)
- [s4-missions-lib-4](notes/s4-missions-lib-4/README.md)

Later plans do not edit this README.md; each edits only its own notes. Until each one is ported, it returns `Error::Unsupported`. The crate never silently differs from Go.

## What callers should know

- **Environment.** `Invocation.env` is the whole environment. Discovery runs `git rev-parse --git-common-dir` with it, and looks git up on its `PATH`. With no `PATH`, git is not found, as in Go, and a linked worktree keeps its own store.
- **Explicit store directory.** `Invocation::with_store_dir` replaces discovery for every command. `init` never discovers a store: it accepts an explicit directory only when that directory is `<cwd>/.luuna/missions`, and returns `Error::Unsupported` for any other.
- **File system errors** return `Error::Io`. Go would print them with its own wording, which this crate does not copy.
- **Not verified on this host:** Windows and macOS behaviour, including how git is looked up on Windows.

## Tests

The unit tests and two harness self-checks run with the workspace:

```sh
cargo test --locked -p luuna-missions
```

The unit test `config::tests::a_linked_worktree_resolves_to_the_main_checkout_s_store` runs `git worktree add --orphan`, which needs git 2.42 or later. With an older git it fails, and so does `cargo test --workspace --locked`.

The comparison with Go is `#[ignore]`d. It needs a clone of luuna-ai/luuna-missions at the reference commit and a Go toolchain:

```sh
LUUNA_MISSIONS_REFERENCE=<clone> LUUNA_MISSIONS_GO=<go binary> \
LUUNA_MISSIONS_GOPROXY=<GOPROXY list, default https://proxy.golang.org> \
cargo test --locked -p luuna-missions --test parity -- --ignored

# same environment
cargo test --locked -p luuna-missions --test format_roundtrip -- --ignored
```

How the harness works:

- **Reference checks.** It fails if the clone is missing, is not a git clone, or is not at the reference commit. The only git commands it runs in the clone are `rev-parse HEAD` and `--no-optional-locks status --porcelain --ignored`. Before and after every run, it checks that the clone and this worktree are unchanged.
- **Go build.** It builds `./cmd/luuna-missions` from the clone into `target/tmp/missions-parity/bin`. Go's module cache, build cache, temp folder and GOPATH also live under `target/tmp/missions-parity`, which git ignores. The build runs with `GOFLAGS='-mod=readonly -modcacherw'`, `GOTOOLCHAIN=local`, `GOWORK=off` and `GOENV=off`.
- **Fixtures.** It replays the clone's regression fixtures in place (`internal/regression/testdata/*.json`) and never copies them. `tests/support/fixtures.rs` assigns all 194 fixtures to plans by file name.
- **Three comparisons per fixture.** Go must match the fixture and Rust must match the fixture, both under the Go harness's rules. Rust must also match Go byte for byte, after the Go harness's normalisation of project root, ids and timestamps. This last comparison covers stdout, stderr, the exit code, and every file and directory in the project tree, including permission bits.
- **Temporary projects.** Each case runs in a fresh git repository under the system temp folder, never inside a workspace repository.
- **Reports.** Each test writes `target/tmp/missions-parity/report-<test>.json`.

## Layout

```
src/lib.rs           public API
src/error.rs         Error, Result
src/model.rs         Ticket
src/store.rs         Store, DataFile, StoreLock: JSONL, atomic replace, locks
src/json.rs          Go-compatible JSON writing and raw-value preservation
src/config.rs        reader for the config.yaml subset Go writes
src/command/         Invocation, Outcome, run, and one module per command
src/command/app.rs, workflow/, plan_index.rs   slots owned by s4-missions-lib-2
src/command/project/, src/config_tree.rs        slots owned by s4-missions-lib-3
src/command/plans/                              slot owned by s4-missions-lib-4
notes/<plan>/        README.md and UPSTREAM.md of each later plan
tests/later_plans/   parity wrappers of the later plans, one file each
tests/support/fixtures/  fixture groups of the later plans, one file each
tests/parity.rs      fixture parity and runtime argument shapes
tests/format_roundtrip.rs  store interoperability with the Go tool
tests/support/       the comparison harness
```
