# Luuna Roots architecture

Luuna Roots is a local, native Rust CLI for preserving **human-authored project
intent**. It is a Luuna-maintained, MIT-licensed adaptation of
[jayminwest/roots](https://github.com/jayminwest/roots); upstream attribution,
the accepted-graph model, and the observable CLI contract remain part of the
product. This document maps the system. For exact flags and output, the
installed command help (`luuna-roots <command> --help`) is authoritative.

## System map

```text
human / Claude Code / configured agent
                 │
                 ▼
      luuna-roots (or compatible alias: lrt)
                 │
                 ▼
       roots-cli: argv, I/O, terminal sessions
                 │
                 ▼
 roots-core: domain rules, identity, JSONL persistence, guard
                 │
                 ▼
          <project>/.luuna/roots/
      ┌──────────┴──────────┐
      ▼                     ▼
human-owned intent     agent-owned questions,
and accepted graph     proposals, sprouts, notes, headings

Roots reads (never writes) .luuna/missions/ and .luuna/atlas/.
Luuna's runtime is separate: it owns managed-run approval, scope, checks,
review, persistence, and publication.
```

`crates/roots-cli` exposes the two binaries, command registry, human-facing
output, editors, and terminal runtime. `crates/roots-core` owns the reusable
domain model, paths, actor checks, storage, graph rules, integrations, and
verification. This split keeps process/terminal behavior out of persistence
and makes the public CLI contract testable without a real terminal.

## Public CLI surface

Both `luuna-roots` and `lrt` dispatch the same 33 commands in this stable help
order. Every command accepts `--json`; human-facing interactive commands
instead require a terminal when run interactively.

| Group | Commands | Use |
| --- | --- | --- |
| Setup | `init`, `setup`, `guard` | Initialize `.luuna/roots/`; install/remove Claude Code hooks; make the hook decision for an incoming PreToolUse payload. |
| Capture | `plant`, `think`, `flow`, `adopt`, `mv` | Create human ideas; work through questions; combine thinking/review/capture; turn a sprout into a human idea; rename an item without changing its stable ID. |
| Structure | `tend`, `accept`, `reject`, `link`, `scan`, `unlink`, `commit`, `status`, `compost`, `tier` | Review proposals/sprouts; make human graph decisions; discover mention proposals; manage edges, lifecycle, retirement, and per-idea agent tier. |
| Agent | `context`, `prime`, `drift`, `ask`, `note`, `propose`, `sprout`, `heading` | Read accepted context and drift; ask questions, attach findings, submit proposals/sprouts, or update a live Flow heading at the configured tier. |
| Read | `show`, `blame`, `list`, `queue`, `log`, `view`, `verify` | Inspect a node, line/question attribution, lists, attention queue, events, compiled human view, and boundary/graph integrity. |

Typical human loop: `init` → `plant` → `queue` → `think` → `tend` → `view`.
`flow` keeps that loop in one terminal session. Agents use `context` or `prime`
before `ask`, `note`, `propose`, `sprout`, or `heading`; a human always accepts,
rejects, or rewrites the resulting intent.

## Store and consistency model

`init` creates `.luuna/roots/` in the current directory and adds a union merge
driver entry for the JSONL files. Other commands discover the nearest ancestor
containing `.luuna/roots/config.yaml`.

| Location | Owner and contents |
| --- | --- |
| `config.yaml` | CLI configuration: project name, tier, configured agent command/time limit, question thresholds, limits, and view/flow switches. |
| `graph.jsonl` | CLI-owned node and edge records. IDs are stable; slugs/directories may change. |
| `questions.jsonl`, `proposals.jsonl`, `headings.jsonl` | Agent-facing records and their human decisions/dismissals. |
| `events.jsonl` | Append-only audit events, including the required actor for mutations. |
| `human/<hex>-<slug>/idea.md` | Human-authored idea prose and approved intent. Assets live with the human tree. |
| `agent/sprouts/<hex>-<slug>/sprout.md` | Agent-proposed, expiring candidate ideas. |
| `agent/notes/<hex>/` | Agent-attached evidence/artifacts; never a replacement for human prose. |

JSONL readers retain compatibility behavior: blank, malformed, and incompatible
rows are ignored on tolerant reads; duplicate IDs resolve last-wins. Writers
use sibling temporary files plus rename and advisory sidecar locks with stale
lock reclamation. `verify` is the repair/diagnostic entry point: it validates
authors, graph invariants, references, human-tree paths/symlinks, hash ledger,
directories, and JSONL/event shape. Do not hand-edit records to alter a
decision; use the CLI so its event is recorded.

## Ownership, identity, and authority boundaries

- **Human ownership:** human commands write idea prose and make final graph
  decisions. Interactive human content writes require a TTY and a resolved
  human actor from `LUUNA_ROOTS_USER`, legacy `ROOTS_USER`, or Git
  `user.name`.
- **Agent ownership:** agent commands resolve `--as agent:<name>`,
  `LUUNA_ROOTS_AGENT`, or legacy `ROOTS_AGENT`; they can write questions,
  proposals, sprouts, notes, and Flow headings only in the agent-owned store.
  They cannot write `.luuna/roots/human/` or accept their own work.
- **CLI bookkeeping:** Roots-generated records are attributed to `roots` or a
  named Roots rule. Every Roots-store mutation creates an audit event; external
  harness-settings changes (for example, `setup claude`) are not Roots events.
- **Runtime authority:** neither a Roots record, a prompt, a CLI actor, nor a
  Claude hook grants a Luuna-managed run permission. Luuna's Rust runtime is
  the separate authority for approval, scope, checks, review, persistence, and
  publication.

`LUUNA_ROOTS_*` is the preferred environment namespace. The historical
`ROOTS_*` aliases remain supported for upstream/harness compatibility, including
the legacy `ROOTS_AGENT` human-command refusal behavior. Defined blank Luuna
variables intentionally mask the legacy alias, matching the former CLI.

`setup claude` merges a `prime` SessionStart hook, a `guard` PreToolUse hook,
and (at tier 3) a `drift` Stop hook into the selected Claude settings. `guard`
blocks supported Edit/Write-style targets and best-effort shell writes that
resolve into the human tree, including legacy `.roots/human/` references. It
is deliberately fail-open for malformed hook payloads and is a useful harness
defense, not a sandbox or a replacement for runtime policy.

Old upstream `.roots/` stores are not silently read or moved. Migrate them
explicitly; the legacy path is recognized by the guard only for protection.

## Integrations

Roots has read-only adapters, not cross-component ownership:

- **Missions:** reads `.luuna/missions/issues.jsonl`. An issue can link an idea
  explicitly through `intent`/`extensions.intent`, or by an exact `r-` ID
  mention. `show`, `context`, and `queue` surface the link; Roots never changes
  a Mission issue or marks an idea built automatically.
- **Atlas:** reads non-archived JSONL expertise records under
  `.luuna/atlas/expertise/` that cite an exact Roots ID. It surfaces concise
  learning context but does not write Atlas.
- **Beacon:** owns reusable prompts elsewhere; Roots neither initializes nor
  writes it.

Integration readers are tolerant and last-wins for duplicate IDs so that an
unrelated malformed record does not make Roots a writer or administrator of a
different component's store.

## Interactive runtime

`think`, `tend`, and `flow` own one raw terminal/alternate-screen lifecycle,
redraw on resize, and restore the terminal on completion or cancellation. The
runtime is separated into terminal I/O, reducer, renderer, file watcher, and
command orchestration so those lifecycles are independently testable.

- `think` watches human prose saves, resolves questions, and uses `d` dismiss,
  `z` snooze, `s` skip, `a` delegate to the configured agent when allowed,
  and `q`/Ctrl-C/Ctrl-D to end. The configured agent command receives a
  compatibility prompt and session environment; timeouts/failures do not turn
  it into a human author.
- `tend` presents proposal/sprout cards, including accept/reject/reason,
  relation-selection, adoption, and endorsement paths. It can hand off to
  editor/think work without losing the review sequence.
- `flow` combines thinking, review, and planting. It keeps agent work in the
  background and renders the audit trail/inbox/heading. Its keys are Enter
  (recommended step), `n` (think), `o` (another pick), `p` (plant), `x`
  (dismiss heading), and `q`/Ctrl-C/Ctrl-D (end).

Interactive behavior is part of compatibility: editor handoff, raw-mode
cleanup, watcher debounce/replacement handling, resize redraws, cancellation,
and background jobs must remain observable-equivalent to the former CLI.

## Tooling and delivery

The workspace pins Rust `1.99.1` in `rust-toolchain.toml` and commits
`Cargo.lock`. `cargo` is the native development interface:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo build --workspace --release --locked
```

`scripts/hooks/pre-commit` runs the local Cargo gate when configured with
`git config core.hooksPath scripts/hooks`. GitHub Actions is configured to run
the same format, Clippy, test, and locked release-build sequence on Linux,
macOS, and Windows. That matrix is configuration, not evidence that all three
hosts have been executed from a local checkout; CI results remain the
cross-platform verification record.

The compatibility suite and parity matrix exist to compare native observable
behavior—names, flags, help, JSON envelopes, exit status, JSONL records,
interactive keys, and boundary failures—against the retained reference
fixtures captured before the legacy TypeScript/Bun implementation files were
retired.
