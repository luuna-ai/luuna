# ADR 0056: command groups in the terminal client

Date: 2026-11-26. Status: accepted under the owner's signed migration charter (step 6).

## Context

The terminal client has about sixty flat commands (`tickets`, `create-ticket`, `show-plan`, `remember`, …). Step 6 of the migration charter regroups them under `ticket`, `memory`, `prompt`, `intent`, `plan` and `run` and keeps the old names working. [ADR 0054](0054-luuna-intent.md) already introduced `intent <verb>` for the eight Roots commands.

## Decision

1. `luuna <group> <verb>` runs exactly the flat command the verb names, with the same arguments, flags, exit codes and `--json` output. The table is `COMMAND_GROUPS` in `crates/luuna-tui/src/help.rs`:
   - `ticket`: list, create, show, edit, source, reconcile
   - `plan`: create (`plan`), list, show, approve
   - `run`: list, show (`run`), events, follow, implement, candidate, diff, validate, repair, tasks, attempts, worktrees, artifact, cancel
   - `memory`: list (`memory`), add (`remember`), correct, promote, forget, retention, set-retention, apply-retention, export (`export-user-memory`)
   - `prompt`: list, save, render
   - `intent`: unchanged from ADR 0054.
2. Every flat name stays a command; nothing is deprecated or renamed, and the capability catalog's `tui` names are unchanged.
3. `plan`, `run` and `memory` are flat commands as well. For those three a group verb is recognised only as the first argument, and anything else (an id, `--help`, nothing) reaches the flat command exactly as before. `ticket` and `prompt` are new words: bare, they print the group page and exit 2; with `--help`, or as `help ticket`, they print it and exit 0; an unknown verb is a usage error (exit 2).
4. `help <group> <verb>` prints the flat command's page. Group pages and help need no runtime and no token.
5. The overview keeps its existing groups and layout.

## Consequences

Scripts that use flat names keep working. `crates/luuna-tui/tests/command_groups.rs` checks every verb against its flat command (requests, exit code and `--json` output), help equivalence, the local group pages, and that `plan`, `run` and `memory` still reach their flat commands.
