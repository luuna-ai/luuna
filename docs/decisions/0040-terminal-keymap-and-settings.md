# ADR 0040: terminal key bindings and settings file

Date: 2026-10-23. Status: accepted at the owner's request. Extends [ADR 0029](0029-rust-terminal-workbench.md) and [ADR 0037](0037-single-rust-terminal-client.md). No product rule, catalog entry or runtime behaviour changes.

## Context

The owner reported that on a Mac many workbench keystrokes did nothing. The workbench matched raw keys in each handler, and several of those keys do not reach a program on macOS terminals:

- **F1–F12** (Help, Commands, Projects, Missions, Refresh, Atlas, Beacon, Runs, Menu) are taken by macOS for brightness, Mission Control and volume unless Fn is held.
- **Option is not Alt** by default in Terminal.app or iTerm2: Option+Enter sends a plain Enter, so a form field could not take a new line. Instead the form advanced or submitted.
- **Home/End/PageUp/PageDown** need Fn+arrows, and Terminal.app keeps those for its own scrollback.
- Separately, typed characters were refused whenever Ctrl was held. On Windows, AltGr arrives as Ctrl+Alt, so characters such as `@`, `{` or `\` on many European layouts could not be typed into a form, prompt, search or guide answer.

The owner also asked for the terminal's colors and keys to be configurable in a file.

## Decision

1. **One keymap, canonical keys.** `crates/luuna-tui/src/keys.rs` names every action (45 of them) with its *canonical* key: the key its handler has always matched. `AppState::key` first calls `Keymap::translate`, which turns the pressed chord into the canonical key of the action it is bound to. A canonical key that the settings move elsewhere is dropped, and any other key passes through unchanged. Handlers are untouched, so a binding change cannot alter what an action does or which guards it passes.
2. **Text stays text.** While a form, prompt, search or guide question is open, an unmodified character (or Ctrl+Alt, which is AltGr) is always inserted. Bindings only apply to chords and named keys, so `q`, `?` or digits never fire actions while typing. `keys::is_text` is the one rule for every text field and fixes AltGr.
3. **Mac-reachable defaults.** Every function key has a plain-key alternative (`?` Help, `:` Commands, `1`–`7` Home/Projects/Orbit/Missions/Atlas/Beacon/Runs in menu order (amended 2026-11-26: previously `1`–`5` without Home and Orbit, which disagreed with the menu's numbering), `m` menu, `Ctrl+R` refresh). New line also takes `Ctrl+J` and `Shift+Enter`, paging takes `Ctrl+B`/`Ctrl+F` and `Alt+↑/↓`, top/bottom take `<`/`>`, and Backspace also takes `Ctrl+H`. Matching ignores Shift on characters and the case of Ctrl/Alt letters, which terminals report inconsistently.
4. **Unambiguous key reports where offered.** When the terminal answers the capability query, the workbench pushes the kitty keyboard protocol's disambiguate flag (kitty, WezTerm, Ghostty, foot, iTerm2 with CSI u). Shift+Enter and Ctrl+Enter then arrive as themselves. The flag is popped on exit, suspend (editor hand-off) and panic. It is never pushed on a terminal that did not answer. `keyboard_enhancement: false` or `LUUNA_TUI_KEYBOARD_ENHANCEMENT=0` turns it off.
5. **Settings file.** The file is JSON (`serde_json` is already a dependency, so `Cargo.lock` is unchanged). It is read once when the workbench starts, from `$LUUNA_TUI_CONFIG`, `$XDG_CONFIG_HOME/luuna/tui.json`, `%APPDATA%\luuna\tui.json` on Windows, or `~/.config/luuna/tui.json` (macOS and Linux). It holds:
   - `keys`: action → list of keys. Listing an action replaces its defaults, and `[]` unbinds it.
   - `theme`: `preset` (`pascal`, the default since 2026-11-26: the navy Quick Access Pascal scheme of the owner's 13 Nov feedback; `classic`, the original blue workbench; or `terminal`, which keeps the terminal's own colors), `color` (forces color on or off), `styles` (`base`, `panel`, `accent`, `selected`, `chrome`, `mnemonic`, `title`, each with `fg`/`bg`/`bold`) and `tones` (`success`, `active`, `danger`, `attention`, `interrupted`, `neutral`). Colors are `#rrggbb`, `#rgb`, a 0–255 index, a name or `reset`.
   - `keyboard_enhancement`.

   A bad value is skipped with a warning in the status line and on Help, and the rest still applies. Unknown fields are refused, so a typo is reported rather than ignored. `luuna-tui --print-config` prints every default as a complete file, and `luuna-tui --config-path` prints where it is read. Help lists every action with its current keys and the file's path and state.
6. **The six tones keep their words.** Tone colors are configurable, but the state words stay authoritative (ADR 0013), so a palette can never make a state unreadable.

## Consequences

Keys and colors no longer live in the handlers and `Theme`. A new key-driven action must be added to `keys::ACTIONS` as well as its handler, or it cannot be rebound and Help will not list it. Hints still name the default keys. One-shot command mode (`lu <command>`) does not read the file.

## Evidence

On Linux in the cloud workspace: `cargo test -p luuna-tui` passes, including the real-PTY lifecycle and guide tests, which run with keyboard enhancement on and no answering terminal. The tests also cover the keymap: defaults with no conflicts, the macOS alternatives, text in fields, AltGr, rebinding and unbinding, warnings, and parse/label round trips. They cover the theme (color parsing, presets, overrides, the classic palette equal to the original styles), the settings file (the printed defaults load without warnings; malformed files fall back), and an app-level test that `?` and `m` open Help and the menu while `?q`, Ctrl+J and AltGr `@` are typed into a form. `cargo clippy -p luuna-tui --all-targets` is clean. **Not established:** no run on a Mac terminal (Terminal.app, iTerm2) or on Windows Terminal with an AltGr layout. Those manual checks remain.
