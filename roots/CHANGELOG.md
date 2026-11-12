# Changelog

All notable changes to Roots are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

First implementation of roots (stages 1-7 of the original design spec).

### Changed
- Rewrote the CLI from TypeScript/Bun to the native Cargo workspace (`roots-core` and
  `roots-cli`) while preserving executable names, command and flag surface, JSONL format,
  actor/guard boundaries, interactive behavior, and legacy environment aliases.
- Replaced Bun quality tooling with Cargo formatting, Clippy, workspace tests, locked release
  builds, and a configured Windows/macOS/Linux CI matrix.
- SPEC.md removed. Direction now lives in seeds issues and roots ideas.
- Herdr split-pane support removed; `think` and `adopt` open split panes only inside tmux or
  zellij. Fixes `tend` → adopt opening a second pane under herdr.

### Fixed
- Endorsed ideas use the endorsed sprout statement as their claim, not the human's thoughts.
  `list`, `view` headings, `show`, `prime`, `context`, rule questions and mention targets used
  to print the thoughts ("lgtm") as the idea. `--cite` now also matches the endorsed text.
  `show`/`context` JSON add `thoughts` (idea.md); `prime` prints them after `endorses`.
- `think`'s agent status line no longer prints a new line per frame in narrow panes (cut to the
  terminal width).
- `think` and `tend` screens fill the terminal height, with keys pinned to the bottom.
- `think` no longer reads arrow keys and other escape sequences as key presses (`ESC [ A`
  contained an `a`).
- `think`, `tend` and the flow card never grow taller than the terminal. The top border used to
  scroll off when the content overflowed. Links fold, older trail entries fold ("… N earlier"),
  and long card bodies end with "… N more lines".

### Added
- **Endorse a sprout (accept with thoughts).** `roots adopt <s-id> --endorse` and `[t]` on a
  `tend` sprout card accept the sprout as written. idea.md holds only the human's thoughts; the
  idea records `endorses: {sprout, hash}` (sprout.md pinned at adoption). `view`, `show`,
  `prime` and `context` quote the sprout with an `[agent] … · endorsed by …` label and flag it
  if sprout.md changed since. The slug comes from the sprout statement.
- `tend` `[t]`: type thoughts under the card, enter endorses, ctrl-e continues in `$EDITOR`.
  Sprout cards show the whole body (clamped to the terminal), not 6 lines.
- `adopt` puts the sprout in the editor buffer, quoted below a git-style scissors line that is
  removed on save, so the reference is visible in any terminal (no split pane needed).
- **`[a] ask agent` in think (tier ≥ 1).** Hands the current question to the agent. After the
  session `agent.command` researches it and attaches findings with
  `roots note <id> --question <q-id> --file <md>`, which reopens the question. Next session the
  question comes first with the findings' first paragraph under it. The human still answers in
  `idea.md`. A run without findings returns the question (`undelegate`, by roots). In `flow` the
  run is background work on the card, and ideas with findings rank first ("agent findings
  ready"). New status `delegated`, field `findings`, events `delegate` / `undelegate`.
- **`roots flow [<id>]`.** One continuous session: think → transition card → review inbox → next
  idea, without dropping to the shell. The card shows the trail (read back from `events.jsonl`),
  running agent work, the inbox and the next pick. `[enter]` does the recommended step. The agent
  never blocks: questions arrive during the session, and proposals and the heading run in the
  background after it. Under tmux with vi/vim/nvim, one editor pane is reused across sessions.
  New events `flow.start` / `flow.end`.
- **Agent headings (`roots heading`, tier ≥ 1).** During a flow the agent may file a heading:
  at most 2 sentences and 280 characters, every `r-` id cited with an exact quote. The heading is
  shown only on the flow card as `[agent]` and never in `view` or `prime`. `[x]` dismisses it;
  dismissed text is refused later and goes into the next packet. Stored in `headings.jsonl`
  (`heading` / `heading.dismiss` events). `flow.heading` in config turns it off. The agent run
  gets `ROOTS_FLOW`.
- **`next:` hints.** `plant`, `think`, `adopt`, `tend`, `accept`, `reject` and `flow` end with a
  dim `next:` line naming one step: review the inbox, think about an idea, or plant one.
- **`roots blame <id>`.** Prints `idea.md` with the question each line answered in the margin.
  `answer` events now record `lines` (hashes of the lines each save added), matched by content.
  Resolves the SPEC open question on session granularity. Older answers without hashes are
  placed by replaying their session's spans when `idea.md` is unchanged since that session;
  otherwise they show as untracked.
- **`roots view` labels answered paragraphs** with a quoted `**Q:**` line (question, id, asker),
  in Markdown and `--html`.
- **Core (stage 1; historical former Bun implementation).** `init`, `plant`,
  `show`, `list`, `mv`, `log`. `.roots/` layout with
  CLI-owned `graph.jsonl` and append-only `events.jsonl` (one event with `by` per mutation).
  IDs `r-xxxx` resolve by id, hex, slug or unique prefix; a manual directory rename is fixed up
  on the next read. JSONL with advisory locks, atomic writes and last-wins dedup (seeds pattern).
  `--json` envelope on every command, did-you-mean for unknown commands, and zero runtime deps
  in that former Bun implementation.
- **Think loop, tier 0 (stage 2).** `think [<id>]`: two-pane session that watches `idea.md` for
  saves; a save that changes the file answers the current question. Keys `[d]` dismiss,
  `[z]` snooze, `[s]` skip, `[q]` end. Opens `$EDITOR` in a split inside tmux, zellij or herdr.
  Deterministic question rules (`missing-done`, `missing-scope`, `too-big`, `orphan`,
  `tension-open`, `stale`) with thresholds in `config.yaml`. Session end records the content hash.
  `queue` ranks what needs thinking.
- **Agent questions, tier 1 (stage 3).** `ask`, `context` (the per-idea packet), and
  `agent.command`: `think` runs the configured agent with the packet before the session (with a
  timeout) or waits for a harness to call `roots ask`. Per-idea tier override with `tier`.
- **Structure, tier 2 (stage 4).** `propose edge|split|merge|compost` with exact-substring
  citation checks, caps, TTL and permanent rejections. `tend` (one card per decision, one
  keystroke each), `accept`, `reject`, `link`, `unlink`, `commit`, `status`, `compost`.
  Deterministic mention detection at session end and in `scan` files link proposals.
  Edge invariants (`serves`/`replaces`/`derives` acyclic; `replaces` composts its target).
- **Sprouts and notes (stage 5).** `sprout` (agent tree, capped, expiring), `adopt` (the human
  rewrites the sprout in their own words; a `derives` edge keeps the lineage), `note` (agent
  artifacts under `agent/notes/<hex>/`).
- **Views and boundary (stage 6).** `view` (deterministic Markdown; `--html` page with graph and
  timelines; optional `ROOTS.md`), `verify` (human/agent boundary, graph invariants, hash
  ledger), `guard` (PreToolUse hook), `setup claude` (installs SessionStart `roots prime` and the
  guard hook; a Stop `roots drift` hook at tier 3).
- **Integrations, tier 3 (stage 7).** Read-only seeds link (`sd create --intent r-xxxx`; `show`
  lists linked issues; `queue` suggests `built` when all close), mulch learnings that cite an
  `r-` id in `show`, `drift [--diff <rev>]`, and `prime [--scope <id>]`.
- **Historical former Bun tooling:** Fleet `check:all` gate suite (`bun run verify`): lint, typecheck, AGENTS.md accuracy,
  duplication, unused deps, file-size and debt-marker ratchets, coverage floors, CI parity.
  GitHub CI, Dependabot, issue/PR templates, labels.

### Changed
- Pending proposals that became moot (an endpoint idea composted or gone, or an endpoint sprout
  no longer open) are now handled like overdue ones: read paths (`queue`, `tend`, `show`, the
  agent proposal budget) hide them at once, and the next write that expires proposals (`tend`,
  `accept`, `reject`, filing) marks them `expired` with an `expire` event whose `reason` is
  `moot: <why>` (overdue ones log `reason: ttl`).

### Fixed
- Flaky `fs.watch` test: the rename test started writing before macOS FSEvents was live
  (about 1 run in 10 under load). Tests now wait until the watcher reports a change.
