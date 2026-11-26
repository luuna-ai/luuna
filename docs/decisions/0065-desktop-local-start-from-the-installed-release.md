# ADR 0065: The desktop's local start uses the installed release

Date: 2026-11-26. Status: accepted. Found by the first clean install on macOS before v0.1.0. Builds on [ADR 0011](0011-windows-native-bootstrap.md), [ADR 0048](0048-one-posix-installer.md) and [ADR 0050](0050-first-run-terminal-setup.md).

## Context

On macOS the desktop started its runtime only through `~/.config/luuna/launch.py`, a personal helper that neither the installer nor `lu init` provides. On a clean install the app showed "Local startup refused" and named personal `just` recipes. It also ignored the private settings `lu init` writes, so the desktop and the terminal could not share a runtime. Linux had no local start at all.

`lu init` also saved nothing when the provider key was skipped, so the next run generated a new service token and reinstalled Pi.

## Decision

1. **A fixed command in the installed release.** `luuna-tui desktop-connection` loads the private settings (`environment.json` beside the terminal's settings), starts the installed `luuna-runtime` with them when nothing answers on `http://127.0.0.1:4242`, and prints one JSON line, `{"url":…,"token":…}`, on stdout. It takes no arguments, refuses a `LUUNA_URL` other than the default, prints errors on stderr only, and does not run the doctor gate. Starting is the same code `lu init` uses (`setup::start_runtime`).
2. **The desktop finds it through the installer's layout only.** On macOS and Linux the desktop reads `current` in the install home (`LUUNA_INSTALL_HOME`, else `${XDG_DATA_HOME:-$HOME/.local/share}/luuna-install`; relative values are ignored), accepts only a bare version made of letters, digits, `.`, `-` and `+` that does not start with `.`, and runs `versions/<version>/bin/luuna-tui desktop-connection`. Nothing from the view chooses the executable, its arguments or the endpoint. The connection still arrives on an anonymous pipe and passes the same check as before: the default URL and a 64-character hex token.
3. **The personal helper stays as a fallback.** On macOS, when no release is installed and `~/.config/luuna/launch.py` exists, the desktop uses it as before. Otherwise it says to install a release and run `lu init`, or to use the manual connection form. Failure messages name `lu init` and `lu doctor` instead of personal recipes.
4. **`lu init` keeps what it has prepared.** The generated token, data folder and Pi binding are saved before the provider key is asked for, so skipping the key loses nothing and a rerun reuses them.

## Consequences

A clean install on macOS or Linux opens the desktop connected to the same runtime and token as `lu`. Windows is unchanged. The desktop still never starts a runtime that is not the installed release's or the personal helper's.

## Evidence

A desktop unit test covers finding the terminal: no `current` file, the XDG and override homes, the default under `~/.local/share`, a relative override, and versions that could leave `versions/`. The terminal's existing setup tests still pass; the start code moved into `start_runtime` unchanged.
