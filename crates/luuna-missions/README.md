# luuna-missions

A native Rust library that reads and writes a Missions store (`.luuna/missions/`) exactly as the Go `luuna-missions` tool does. It is ported from the Go tool at commit `2fbbf8e5b6a238ea47543e1dad112fe1716faec4`. See [UPSTREAM.md](UPSTREAM.md) for provenance, the license notice, and the Go behaviour copied on purpose.

This crate is a library only. It has no binary, and nothing in the workspace uses it yet. The package name matches the name of the Go executable, but the crate provides no `luuna-missions` command.

## What it does

- **`Store`** opens a store directory that you name, or finds one by walking up from a working directory as Go does. It reads tickets and takes Go-compatible lock files.
- **`Ticket`** is one record of `issues.jsonl`. The API calls records tickets; on disk they keep Go's names. A ticket keeps its raw JSON, its key order and any unknown fields.
- **`command::run`** runs one command as a library call. It takes an `Invocation` (arguments, working directory, stdin, environment, and an optional explicit store directory) and returns an `Outcome` with the stdout, stderr and exit code Go would produce. Errors that Go prints to its user come back as an `Outcome` with exit code 1. `Err` means the library failed, or the request needs behaviour that is not ported yet (`Error::Unsupported`, which names the plan that ports it).

This plan (s4-missions-lib) ports the commands luuna-runtime uses: `init`, `create`, `show`, `list`, `search`, `update` and `block`. Later plans port the rest:

- **s4-missions-lib-2:** close, ready, blocked, stats, dep, label, unblock, help, version and completions.
- **s4-missions-lib-3:** config, tpl, doctor, sync, migrate-from-beads, the legacy `.seeds` layout, prime, onboard, setup claude, guard and upgrade.
- **s4-missions-lib-4:** plans.

Until each one is ported, it returns `Error::Unsupported`. The crate never silently differs from Go.

## What callers should know

- **Refused until a later plan:** help and version output (`--help`, `-h`, `--version`, `-v`, or no command at all), unknown commands, and an environment with `LUUNA_MISSIONS_DEBUG=1` or `LUUNA_MISSIONS_LOG_LEVEL` set to `debug` or `trace`. Go writes debug records to stdout in that last case (SD8). All of these return `Error::Unsupported`.
- **Environment.** `Invocation.env` is the whole environment. Discovery runs `git rev-parse --git-common-dir` with it, and looks git up on its `PATH`. With no `PATH`, git is not found, as in Go, and a linked worktree keeps its own store.
- **Explicit store directory.** `Invocation::with_store_dir` replaces discovery for every command. `init` never discovers a store: it accepts an explicit directory only when that directory is `<cwd>/.luuna/missions`, and returns `Error::Unsupported` for any other.
- **Plans.** A status change (`update --status` or `--claim`) on a ticket that has a `plan_id`, that is the seed of a plan or that is one of its children returns `Error::Unsupported` naming s4-missions-lib-4, before anything is written. Go would run its plan transition hook there. Other edits of such a ticket, and a status set to the value it already has, are applied. Reading is refused the same way where the output would print plan context: `show`, `list` and `search` of a ticket whose `plan_id` names a plan in `plans.jsonl`, in the markdown, plain and json formats, and `show pl-<id>` for an id that is not a ticket. The `ids` and `compact` formats print no plan context and are ported for such tickets.
- **`config.yaml`** is read only in the forms Go writes for top-level values. Nested values, comments and other YAML return `Error::Unsupported` until s4-missions-lib-3.
- **File system errors** return `Error::Io`. Go would print them with its own wording, which this crate does not copy.
- **Not verified on this host:** Windows and macOS behaviour, including how git is looked up on Windows.

## Tests

The unit tests and two harness self-checks run with the workspace:

```sh
cargo test --locked -p luuna-missions
```

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
tests/parity.rs      fixture parity and runtime argument shapes
tests/format_roundtrip.rs  store interoperability with the Go tool
tests/support/       the comparison harness
```
