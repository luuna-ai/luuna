# ADR 0059: A starter set for empty project memory

Date: 2026-11-26. Status: accepted by the owner (feedback F1: propose facts from a scan and confirm each one; no agents). Extends ADR 0014 and ADR 0026. It changes no stored format.

## Decision

When a project has no Atlas memory, Luuna offers to build a small starter set. `GET /api/v1/projects/{id}/memory/starter` returns proposed facts, and `memory_empty` says whether the project has any project or process memory yet. Each fact has:

- a stable `key`;
- the repository it is about;
- its content and provenance;
- the evidence it rests on (`manifest`, or a repository-relative path);
- `recorded`, which is true when an existing memory already holds exactly that content.

The route records nothing. It refuses with 409 until a manifest is confirmed.

The facts come from two read-only sources:

- **The confirmed manifest:**
  - each required check, as a rule to run it before proposing a change;
  - each repository's default branch;
  - each confirmed relationship between repositories.
- **File presence in each registered checkout:**
  - lockfiles (`Cargo.lock`, `package-lock.json`, `pnpm-lock.yaml`, `yarn.lock`, `bun.lock`, `go.sum`, `uv.lock`, `poetry.lock`);
  - pinned toolchains (`rust-toolchain(.toml)`, `.nvmrc`, `.python-version`);
  - guides (`AGENTS.md`, `CLAUDE.md`, `CONTRIBUTING.md`, `README.md`, `docs/decisions/`).

Luuna only checks whether these files exist. It never reads their text, never follows a link, and stops at 40 facts. This keeps docs/06's rule that repository content never becomes instructions. No agent and no model runs.

The developer decides about each fact on its own, and only facts they keep are recorded:

- The desktop shows a "Build a starter set" control in an empty memory view, with Keep and Skip for each fact.
- In the terminal workbench, `memory-starter <project>` asks about each fact. The command line never prompts (as for approvals), so `lu memory-starter <project>` only lists the facts unless it passes `--keep <key>` (repeatable) or `--keep all`. Because `--keep` saves, the workbench admits and limits the command as a mutation and never replays it on reconnection.
- In the workbench, `s` on the Atlas screen starts the same review.

A kept fact goes through the ordinary `POST …/memory` route with scope `project`, the fact's own provenance and the idempotency key the starter set gives the fact: `starter-<key>-<hash of the key, content and provenance>`, with `-2`, `-3`… added once a memory kept under the previous key has been deleted. A retry replays the same request, while a fact deleted and kept again is recorded again rather than answered from the old delivery. So it is an ordinary Atlas convention record in the `luuna-project` domain, with Luuna's identity envelope, under the same intent ledger as any other memory. It can be corrected, promoted, deleted and selected for a plan like any other record.

## Consequences

A new project can start with grounded and reviewable memory without any automation writing to it. The set is deliberately shallow: it says what the manifest and the file layout show, not what the code means. Deeper facts are still recorded by the developer, or learned through review.

## Evidence

- Unit tests cover the proposal order and content, refusing linked files and directories, and repositories that are not registered.
- A real-Atlas runtime test covers the 409 before confirmation, proposing without writing, keeping one fact and the `recorded` marks.
- A CLI test covers listing, `--keep <key>`, `--keep all` and an unknown key.
- The workbench snapshot shows the new key.
