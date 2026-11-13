# ADR 0050: private terminal setup and persisted settings

Status: accepted for the owner-authorized consolidation setup plan.

## Problem
The project guide previously required a configured, running runtime before it could help a new developer. There was no terminal action to persist the keymap, theme or keyboard options. A dirty baseline offered no local recovery.

## Decision
Interactive `lu init` prepares pinned Pi 1.3.0 and Bun 1.4.3 under the user configuration directory's `luuna/private/tools`, using the reviewed Pi lockfile and the existing component setup script. It never replaces global tools or reads ordinary Pi session history. Existing explicit tool environment settings take precedence. A missing provider credential is entered without echo; the default first-run provider is OpenRouter. Existing supported key bindings or the explicitly selected OAuth auth file need no new key.

The service token and saved tool/credential bindings live in `private/environment.json` beside the user settings. On Unix the directory is 0700 and the file is 0600. Symlink and overly permissive credential files are refused. Setup refuses to create private state under a Git checkout. A relaunch passes the private bindings to the client without mutating the environment of a multithreaded process. Explicit environment variables override saved bindings. The runtime is started from the sibling executable only when health is unavailable; an existing runtime must pass authenticated diagnostics and is never killed or reconfigured by setup. Runtime output stays in the private directory.

`lu config set <name> <JSON value>` validates and atomically updates the user settings, preserving unrelated fields and leaving invalid existing documents untouched. Help offers persisted theme and keyboard toggles. The next workbench launch reloads settings. A nearest `.luuna/tui.json`, bounded by the Git checkout root, overlays user settings without resetting unspecified keys. Project settings do not supply credentials.

Before starting a baseline, the guide displays local changes and offers explicit stash-and-continue, with skip selected initially. Stashing includes untracked files. It never discards changes or automatically restores the stash. The runtime still authoritatively refuses any baseline that is not clean.

## Limits and evidence
The setup applies to an interactive terminal; scripted commands retain their noninteractive behavior. GitHub publication remains optional. Native component wiring will remove the Bun setup branch. The Pi manifest and lockfile are embedded in the binary, so the binary-only release format requires no source-tree payload. Component setup remains development-only until native wiring removes its Bun dependency. Fixture tests prove persistence, invalid-edit preservation, credential permission checks, authenticated doctor routing and stash restoration; they do not prove provider access or native macOS behavior.
