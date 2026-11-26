# ADR 0064: Native folder picking and the desktop project switcher

Date: 2026-11-26. Status: accepted. Answers owner feedback C1 (folder picking) and D1 (project first) for the desktop app. It extends [ADR 0039](0039-desktop-window-plugins.md).

## Decision

1. **A folder dialog, behind one command.** The desktop shell adds `tauri-plugin-dialog` (pinned, desktop-only), used from Rust only. The page gets one new command, `pick_folder`, which opens the native folder dialog over the main window and returns the chosen path, or nothing when it is cancelled. The capability grants `allow-pick-folder` and none of the dialog plugin's own commands, so the page still cannot open arbitrary file dialogs, read files or show message boxes.
2. **Dropped folders reach the page as data.** Folders dropped on the window are sent to the page as a `luuna:folders-dropped` event that lists their paths. This is the same `eval` + `CustomEvent` route the native menu uses, so no event permission is granted. The paths are JSON-encoded, so they reach the page as data, never as code. Dropped files are ignored.
3. **The page only fills the form.** In the register form, each repository row gets "Choose folder…", and dropped folders fill the rows without a path first, then add rows. A chosen or dropped path is registered like a typed one: the runtime inspects it and applies every registration rule. The browser build shows neither control.
4. **A project switcher in the top-left corner.** The top bar gains a `lead` slot, which holds the project switcher on every view. It lists the projects the Projects view read, shows the one open there, and opens another one there. It is disabled until a project exists.

## Consequences

Registering a project no longer means typing absolute paths in the desktop app. The terminal client's folder browser came earlier, in `lu init`. Home and Board still show every project at once. The switcher picks the project the Projects view works on and does not filter them.

## Evidence

Rust unit tests in `apps/desktop/src-tauri/src/folders.rs` check that only folders are passed on and that a path becomes JSON data. Desktop tests cover the native helpers (no dialog in a browser, drop events filtered and unsubscribed), how dropped folders fill the form, and the switcher's options and markup. A `@luuna/ui` test places the lead slot between the menu button and the palette trigger.
