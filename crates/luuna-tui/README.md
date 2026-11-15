# Luuna terminal client (`luuna-tui`, `lu`, `luuna`)

One Rust executable for the Luuna runtime: the scriptable command line and the interactive
workbench. It exposes the same product actions as the desktop client through the shared
capability catalog (`schemas/capability-catalog.json`, ADR 0020) and explains every refusal with
the shared sentence `<Action> refused: <reason> <next step>`. The runtime alone authorizes
execution, approval, validation and publication. Since [ADR 0037](../../docs/decisions/0037-single-rust-terminal-client.md)
the client has no Node process: it was a Node command line plus a Node bridge behind the Rust
renderer, and is now this crate alone, with byte-identical commands and requests. Human output has
since changed only where [ADR 0042](../../docs/decisions/0042-short-references-and-picked-input.md)
shows short ids and hash prefixes; `--json` output is unchanged.

```sh
cargo build --locked -p luuna-tui                # target/debug/luuna-tui[.exe]; `npm run tui -- …` runs it too
export LUUNA_URL=http://127.0.0.1:4242          # loopback only; this is the default
export LUUNA_API_TOKEN=...                       # never echoed; prompted (hidden) on a terminal when unset
luuna-tui                                        # interactive workbench (on a terminal)
luuna-tui help                                   # the overview: commands grouped by workflow, the component CLIs, flags, environment
luuna-tui plan --help                            # one command's page: arguments, every flag, the runtime route, an example (also `help plan`)
luuna-tui help --all                             # the complete reference
luuna-tui init                                   # guided project setup (terminal only)
luuna-tui projects --json                        # one cataloged action for scripts; --json prints the runtime's record unchanged
luuna-tui approve 3f2a91c0 --acknowledge-hash <first 8+ hash digits>   # short ids and hash prefixes work (ADR 0042)
luuna-tui run latest                             # the newest run; ticket, plan, run and memory ids accept their short id
luuna-tui follow <run>                           # live events over SSE, resumed after an outage from the last sequence
luuna-tui home                                   # Home summary: projects, validation health, blockers (a projection, not an action)
luuna-tui board --project <id>                   # Kanban projection of the recorded run states; nothing is dragged
luuna-tui doctor                                 # Settings: environment and credential presence by name; values are never shown
luuna-tui catalog                                # the capability catalog, as embedded
```

On Windows, `bin\luuna.cmd` (and `bin/luuna` for Git Bash; the installed `lu.cmd` calls it) run
the client through `scripts/start-terminal.ps1`, which supplies `LUUNA_URL` and the token in memory
(ADR 0011) and names the client `lu` in its help (`LUUNA_CLI_NAME`). Every help form needs no
token and starts no runtime. `lu missions|atlas|beacon …` (and `lu help missions`) run the
component CLIs themselves. `-DataDirectory <dir>` is accepted only by name.

Exit codes: 0 done, 1 refused or failed, 2 usage. Transport failures and 5xx responses are plain
sentences; a server body is never shown.

## Layout

| Module | What it is |
| --- | --- |
| `cli.rs` | The command line: argument parsing, connection, hidden token entry, exit codes |
| `commands.rs` + `commands.json` | The command table (data, also read by `scripts/check-contracts.mjs`) and each command's behaviour |
| `client.rs` | Loopback URL check, bearer-token HTTP/1.1, refusal mapping, SSE follower with `Last-Event-ID` |
| `failures.rs` | The shared failure explanations and next steps |
| `builders.rs`, `projections.rs` | The registration and plan request builders; the Home and Board projections |
| `format.rs`, `help.rs`, `text.rs` | Human output with the six tones, the help, width-aware wrapping |
| `js.rs` | A JSON value with JavaScript semantics (key order, `undefined`, number printing, UTF-16 lengths) |
| `bridge.rs` | The workbench's in-process command service (the versioned protocol of `protocol.rs`) |
| `prime.rs`, `components.rs` | `lu init`'s flow and the component CLI runner |
| `app.rs`, `views/`, `ui.rs`, `guide.rs` | The workbench's state and screens, including the guide mode |
| `keys.rs`, `config.rs`, `theme.rs` | The keymap, the settings file and the colors (ADR 0040) |
| `refs.rs` | Short ids, unique prefixes, `latest` and hash prefixes, resolved before anything is sent (ADR 0042) |

`js.rs` exists so output stays exactly what the original JavaScript client printed: objects keep
insertion order with integer-like keys first, missing fields are `undefined` and omitted from
JSON, numbers print like `Number.prototype.toString`, and string lengths count UTF-16 units.
serde_json's order-preserving feature is deliberately not enabled: it would change ordering for
the whole workspace, including the runtime's canonical hashing.

## `init`

`init` is a guide, not a catalog action. On a terminal it opens the workbench's full-screen guide
mode. Eight steps: runtime, project (an already registered checkout is continued; `--project <id>`
names one), discovery, manifest (review, add a required check, edit in `$EDITOR`, confirm),
baseline, suite (managed stores, an optional first Atlas memory), agents, and the first ticket.
Each runtime step runs a cataloged command, so it sends the same bodies as the commands and every
refusal is the runtime's, offered as retry, skip or leave. The agents step runs the component CLIs
in the checkout only as ticked (repo-local stores, CLAUDE.md/AGENTS.md sections previewed before
they are written, optional Claude Code hooks, each component's primed context); they run with
Bun's own executable, no shell, and never receive the runtime's URL or token.

## Workbench

Luuna opens on Home. `s` on Home or Projects (or "Set up a project" in the menu) starts the guide.
F3 or 1 Projects, F4 or 2 Missions, F6 or 3 Atlas, F7 or 4 Beacon, F8 or 5 Runs, F2 or `:` the
complete command catalog, F1 or `?` Help; F10 or `m` opens navigation, Tab changes focus, arrows
(or h/j/k/l) select, Enter inspects, Esc returns, F5 or Ctrl-R refreshes. Every function key has a plain-key
alternative because macOS terminals keep F1-F12 unless Fn is held ([ADR 0040](../../docs/decisions/0040-terminal-keymap-and-settings.md)).
Forms: Tab or Up/Down move fields, Left/Right pick from a field's choices (repositories, records,
scopes, yes/no; typing still works), Ctrl-U clears, Alt-Enter, Shift-Enter or Ctrl-J adds a line (on
a Mac, Option-Enter sends a plain Enter), Ctrl-S submits. Keys, colors and the kitty keyboard flag
are read from `$LUUNA_TUI_CONFIG`, else `$XDG_CONFIG_HOME/luuna/tui.json`, else
`%APPDATA%\luuna\tui.json` on Windows, else `~/.config/luuna/tui.json`; `luuna-tui --print-config`
prints every default and `--config-path` the file in use. Help lists the keys as configured. Approval shows the complete
plan and hash and takes a typed `approve` (or the hash's first 8 digits), never a prefilled answer;
a terminal too small to show the plan cannot approve. Lists show short ids, and every ticket, plan,
run and memory argument accepts one, as do `runs --before` and `plan --memory` (ADR 0042). A
workbench form resolves a short id or `latest` among the records it has loaded and sends that exact
id; one it cannot resolve is sent as typed for the command to resolve or refuse. Esc stops local
waiting; the `cancel` action requests runtime cancellation. Quitting
leaves the runtime running.

Frames are event-driven and capped at `LUUNA_TUI_FPS` (default 60, 30-144); an idle workbench does
not redraw. DEC 2026 synchronized output is on by default (`LUUNA_TUI_SYNC=0` turns it off); large
scroll jumps ease over about 120 ms (`LUUNA_TUI_ANIMATION=0` makes them instant). `NO_COLOR` and
`TERM=dumb` disable colour. The in-process service keeps the former bridge's limits: four reads and
one mutation at a time, health polled every two seconds, the token redacted from every event.

## Evidence

```sh
cargo test --locked -p luuna-tui
cargo clippy --locked -p luuna-tui --all-targets -- -D warnings
npm run test:contracts
```

`tests/cli.rs` runs the built client against a mock runtime: every catalog entry's command issues
its method and route, request bodies, refusal sentences, usage errors, approval by full hash or an
8+ digit prefix (a 7-digit prefix is refused), short-id and `latest` resolution, and SSE resumption. `tests/guide_flow.rs` drives `init` with scripted answers, `tests/bridge_service.rs`
the workbench's service, and `tests/terminal_lifecycle.rs` and `tests/guide_pty.rs` run the binary
in a real PTY (ConPTY on Windows). The end-to-end harnesses are `node scripts/tui-smoke.mjs` and
`node scripts/tui-native-audit.mjs` against a disposable synthetic runtime. These are fixture and
local-host checks, not genuine-provider or other-platform evidence.

## First-run setup and settings

Run `lu init` interactively to prepare Luuna-owned pinned tools, enter a missing provider key without echo, start or connect to the runtime, and continue project setup. The default new credential is an OpenRouter key; an existing supported provider environment binding or explicitly configured Pi OAuth auth file is reused. Existing global tools and Pi sessions are preserved. Private credentials live outside repositories in the user configuration directory's `luuna/private` folder (0700 directory / 0600 credential file on Unix). Publication credentials remain optional.

`lu config` shows the user settings path. For example, `lu config set keyboard_enhancement false` persists a change; Help also offers theme and keyboard toggles. Reopen the workbench to apply it. Project `.luuna/tui.json` files overlay only their listed settings, stopping at the checkout root. Invalid edits leave the user file unchanged.

Before a baseline, the guide offers to stash staged, unstaged and untracked changes, or leave them alone and skip the baseline. The default is to leave them alone. Stashed work stays in Git until you restore it with `git stash pop`; Luuna never discards or automatically restores it.
