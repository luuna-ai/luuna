# Luuna desktop foundation

The React frontend connects to the independent Rust runtime. It lists persisted runs, starts a tool-free Pi conversation, displays text and raw events, reconnects through durable cursor replay, and requests cancellation. The Projects panel registers an existing repository root by absolute path, shows the runtime's read-only observation (redacted remote, HEAD, branch, dirty counts), requests a discovery proposal, lets you edit the proposed manifest JSON and confirms it with the current revision guard; the project event record replays from the runtime. Run baseline starts the confirmed required recipes on a clean checkout and lists the recorded evidence; the baseline run appears in the session history with its check events.

The workflow section creates a temporary local ticket, records a plan revision for the registered repository, presents the complete immutable plan and full hash for acknowledgment and approval, then schedules the writer. Writers change only Luuna-owned worktrees under the runtime's data directory. Inspect candidate diff shows the exact candidate hash and commit. Validate and review requests the approved recipes followed by a fresh independent reviewer session; check commands, environment fingerprints, artifact hashes/logs, structured findings and recorded reasons remain bound to that candidate. Repair candidate requests another bounded writer attempt under the existing approval. The optional automatic repair checkbox lets validation request repairs within those same approved budgets; it is off initially. Workflow provider/model fields apply to the next action and can use the runtime defaults. Active work exposes cancellation; an idle candidate awaiting validation does not. An interrupted validation can be retried with fresh check and review evidence. A failed candidate whose required checks passed but whose reviewer produced no verdict (invalid or self-inconsistent output) shows Re-validate and review when the runtime allows it: a fresh validation generation for the same candidate revision under the existing approval and budgets, spending no writer attempt. Publication remains a later milestone.

From the repository root:

```sh
npm ci
npm run dev
```

Open `http://127.0.0.1:1420`. The window is a sidebar of views (Home, Board, Projects, Sessions, Settings, Help; `Ctrl+1` to `Ctrl+6`) beside one content column, and every view but Help shows a connect page until a runtime answers ([ADR 0030](../../docs/decisions/0030-desktop-shell-and-tailwind.md)). Disconnect and Stop runtime live in Settings. Start the runtime separately using the [root development instructions](../../README.md), then enter its URL (default `http://127.0.0.1:4242`) and `LUUNA_API_TOKEN` in the connection form. Provider/model fields may use the runtime defaults. Model credentials belong in the runtime environment; they are never entered in this UI. Only HTTP loopback runtime addresses are accepted. The service token stays in memory and is discarded on disconnect or reload.

The live view polls the authenticated replay API using its last event sequence every 700 ms. Reconnecting repeats events safely without losing ordering or duplicating records. The persisted run state determines completion; text from Pi does not establish that a run or validation succeeded.

A runtime that stops answering is shown as an outage beside the connection state ("Reconnecting to runtime", with a note that runs, candidates and worktrees are durable); the client keeps polling, and when the runtime answers again it reloads every panel, so work the runtime marked `interrupted` on restart appears with its reason and the next allowed action. Every refused action is reported as "<Action> refused: <the runtime's reason>" followed by what to do next (re-plan, repair, re-validate, run a baseline, wait for active work, reconnect); transport failures and server errors are described in plain words and no server body is ever shown. Every control is reachable and operable by keyboard: a "Skip to main content" link is the first Tab stop, buttons and checkboxes work with Enter and Space, and the event lists, diff, log and plan panes are focusable scroll regions; connection and activity changes are announced through live regions.

Presentation follows the [desktop design system](../../docs/DESIGN-SYSTEM.md) ([ADR 0013](../../docs/decisions/0013-desktop-design-system.md)). Tokens live in `packages/ui/src/styles/tokens.css`; layout utilities come from Tailwind CSS v4, compiled at build time with a theme that only references those tokens (`packages/ui/src/styles/tailwind.css`, the `@luuna/ui` workspace, ADR 0034), and the client follows the operating system's light or dark scheme, light by default, dark from the OS or `data-theme`; the tokens match Mantine's default theme values (ADR 0035, values only), with Mantine's system font stack for text and the bundled Outfit font for headings. Every runtime state keeps its exact text and gains one of six tones: success, active, danger, attention, neutral or interrupted. Each tone has a colour and a shape mark, and only passed evidence and a live connection are green. Badges go through `StatusBadge`, and the state-to-tone maps in `packages/ui/src/tone/tone.ts` are exhaustive over the API types. `packages/ui/src/design/design-system.test.ts`, run by `npm test`, rejects literal colours and sizes outside the tokens. It also enforces WCAG AA contrast for every semantic pair in both schemes, the heading levels and dismissible alerts.

`node scripts/mvp-ui-smoke.mjs` drives this client in headless Chromium against a built runtime and the synthetic Pi fixture through the [MVP walkthrough](../../docs/MVP-WALKTHROUGH.md): keyboard connection, refused token and registration, import, baseline, ticket, plan, approval, writer, diff, an invalid review followed by re-validation, a hard kill and an orderly shutdown during checks with reconnection and resumption, a hard kill during review with the budget refusal, an accessibility audit, a presentation audit (WCAG AA contrast of every visible text run against its painted background in the dark and light schemes, scheme following, and no horizontal scroll at the 760 px window minimum) and disconnect. It needs the client port 1420 free (the runtime's CORS allowlist admits only that origin) and writes `report.md`, `report.json` and screenshots to `--out`. It is client and lifecycle evidence, never provider evidence.

For the native Tauri 2 window:

On the configured macOS development machine, the shared Just recipes build and
open a real app bundle: `just lu-gui-build`, `just lu-gui`, and
`just lu-gui-rebuild`. `just lu-gui-dev` uses Tauri's live development loop.
`script/build_and_run.sh` is the build/relaunch entrypoint and the local Codex
Run action calls it. The macOS native connection command invokes the fixed
`~/.config/luuna/launch.py` helper; it receives the local URL and service token
over a bounded anonymous pipe. The helper uses the same runtime.json, service
and credential sources as `just lu`, with no token pasted into the GUI or put
in launch arguments. This local profile requires the configured helper; it is
not a claim that a standalone distributed installer provisions it. Development
inspectors open only with `LUUNA_DESKTOP_DEVTOOLS=1`.

```sh
npm run desktop
```

This requires the pinned Rust toolchain and [Tauri system prerequisites](https://v2.tauri.app/start/prerequisites/). Linux additionally needs GTK 3, WebKitGTK 4.1 and their development packages; a headless cloud machine can use the browser frontend. The shell has no filesystem, process or shell plugin permissions. On Windows the window has no native title bar ([ADR 0038](../../docs/decisions/0038-custom-windows-title-bar.md)): `src-tauri/tauri.windows.conf.json` removes the frame (keeping the shadow), the top bar draws the title bar with in-app minimize, maximize or restore and close buttons, and `src-tauri/capabilities/windows-titlebar.json` grants only the window commands those need (`start-dragging`, `internal-toggle-maximize`, `minimize`, `toggle-maximize`, `is-maximized`, `close`). There is no native menu bar on Windows (every menu command is a page shortcut or in the `Ctrl+K` palette); macOS and Linux keep the native frame and menu. Runtime lifecycle is manual at this milestone and closing the desktop does not stop the service.

Validation:

```sh
npm run typecheck
npm run build
npm test
cargo check --locked --manifest-path apps/desktop/src-tauri/Cargo.toml
```

The native application is a separate Cargo workspace with its own checked-in lockfile, so headless Rust runtime checks do not require GUI libraries. Native installers and automatic runtime startup remain later work.
