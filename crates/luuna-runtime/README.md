# Local runtime connectivity foundation

The independent runtime is authenticated and loopback-only. It owns repository import, baseline checks, immutable approval, scoped writers, candidate-bound validation, fresh read-only review and bounded repair. The tool-free connectivity demo remains available as a separate profile: those sessions use a private empty cwd/config with repository tools disabled. Writer and reviewer scope guards are runtime policy, not an OS sandbox. Genuine-provider M3 acceptance remains separate from fixture coverage.

Install the supported `@earendil-works/pi-coding-agent@1.3.0` distribution.
The launcher verifies `--version` before each run. Configure these environment
variables locally; keep credentials out of repository files:

| Variable | Purpose |
| --- | --- |
| `LUUNA_API_TOKEN` | Required random token of at least 32 visible ASCII characters, with no spaces or line breaks; frontend enters it locally |
| `LUUNA_PI_COMMAND` | Pi executable, default `pi`; may be an absolute Node executable. On Windows a bare name is searched through PATH, executables first and then PATHEXT, and an npm `pi.cmd` shim is started as the Node executable and `dist/bundle/cli.js` it names; other `.cmd`/`.bat` launchers are refused |
| `LUUNA_GIT_COMMAND` | Host `git` used for read-only repository inspection, default `git` |
| `LUUNA_PI_ARGS_JSON` | Optional JSON array of launcher arguments, e.g. absolute `dist/cli.js` path |
| `LUUNA_PI_PROVIDER` / `LUUNA_PI_MODEL` | Optional explicit defaults; UI can supply them per run |
| `LUUNA_PI_<PROVIDER>_API_KEY` | Optional alias for any supported provider key (for example `LUUNA_PI_ANTHROPIC_API_KEY`, `LUUNA_PI_OPENAI_API_KEY`) for hosts that reserve the plain name; it becomes the child's standard variable, takes precedence over a direct key, and is never passed through under the alias name |
| `HTTP_PROXY`, `HTTPS_PROXY`, `ALL_PROXY`, `NO_PROXY` | Preserved for provider routing, including lowercase variants and `NODE_USE_ENV_PROXY`; existing TLS trust variables remain enabled |
| `LUUNA_DATA_DIR` | Private durable state directory, default `.luuna`; on Unix it and its `worktrees` directory are made owner-only (0700) at startup, and a directory owned by another user is refused; keep it short on Windows, where worktrees below it count toward the 260-character path limit |
| `LUUNA_PORT` | Loopback port, default `4242` |
| `LUUNA_RUN_TIMEOUT_SECONDS` | Positive operation deadline, default `600`; version probe has a separate 15s deadline |
| `LUUNA_DEFAULT_STORE_MODE` | Where a newly registered project keeps its tickets, memory and prompts: `repository` (default; the first checkout's `.luuna/` stores, when they hold no records yet) or `runtime` (the private store under the data directory); see [ADR 0067](../../docs/decisions/0067-repository-store-by-default.md) |

A setting that cannot be used stops startup with a message naming the variable
and the form it expects (for example `LUUNA_PORT must be a whole number from 0
to 65535 (got "abc")`); the token's value is never printed.

Run `cargo run -p luuna-runtime`. Supply the selected provider's API key through
its standard environment variable, such as `ANTHROPIC_API_KEY` or
`OPENAI_API_KEY`; the runtime never accepts keys through the API. The current
demo inherits only an explicit list of direct provider key variables. OAuth,
cloud platform authentication, user Pi profiles, and model settings files are
not integrated. Missing provider credentials produce a persisted failed run.

`GET /api/v1/health` is public. Its `pi_version` field reports the required
supported version; the CLI is checked when starting a run. `native_execution: true`
states that approved writers and required checks run with the developer's host
permissions (the tool-free demo is one run profile; scope guards are not a sandbox). Its `build_identity`
field is the content digest of the sources the binary was built from, and
`build_rustc` names the compiler release and commit that built it
(`build_rustc_sha256` the SHA-256 of that compiler executable;
`build_rustc_wrapper` the names of any compiler wrapper variables Cargo used, or `none`;
`build_flags` the profile, opt-level, debug setting, whether RUSTFLAGS were set, the names of
any profile, linker or C toolchain override variables, and how many Cargo config files declare
an override). The route is unauthenticated, so it carries names and none/set status only, never
flag values, wrapper paths or config file paths. All other endpoints
require `Authorization: Bearer <token>` (the scheme name is case-insensitive).
Every 4xx/5xx response body is `{"error": "<reason>"}`, including malformed JSON,
unknown or mistyped fields, a missing content type, a body over 256 KiB, a bad
query and unknown routes or methods. A 500 says only that a storage or process
operation failed; the cause is written to the runtime's stderr with the request it
failed, never with payloads or credentials.

- `POST /api/v1/shutdown`: idempotently request an orderly service stop (202).
  Already admitted scheduling requests drain (at most five seconds; a request
  that never finishes its body cannot hold the stop open, and work it starts
  later begins already signalled to stop); new mutations receive
  503 `{"error":"runtime is shutting down"}`. Owned work settles as interrupted,
  preserving its evidence, every event stream ends, and main exits cleanly once
  connections drain (exit status 1 if a connection is still open ten seconds
  after owned work settled). This is separate from cancellation and from closing
  a client window.

- `GET /api/v1/runs?limit=N`: persisted run records, newest first; `limit` (1..=10000) returns the most recent N.
- `POST /api/v1/runs`: `{ "prompt": "Hello", "provider": "...", "model": "..." }`.
- `GET /api/v1/runs/{id}`: current run state.
- `GET /api/v1/runs/{id}/events?after=0&limit=N`: durable event replay by sequence; the optional `limit`
  (1..=10000) returns the first N events after the cursor, so a client pages from the last sequence it
  received. A cursor above 2^63-1 is a 400.
- `GET /api/v1/runs/{id}/stream?after=0`: SSE replay then live delivery; accepts `Last-Event-ID`. The
  stream ends when the run can never change again: a demo or baseline run at any terminal state, a
  workflow only at `completed`, `cancelled` or `reviewed` with no worker attached. A `failed` or
  `interrupted` workflow stays followed, because repair, re-validation and automatic repair continue on
  the same run id. Every stream ends when shutdown begins.
- `POST /api/v1/runs/{id}/cancel`: request abort; terminal cancellation follows process reaping.
  Repeating it, or cancelling a run whose worker already returned, answers with the settled run (200)
  or 409 for an idle run with no worker; it never fails with 500.
- `GET /api/v1/projects`, `GET /api/v1/projects/{id}`: registered projects, machine-registry
  repositories, the current confirmed manifest and the latest discovery.
- `POST /api/v1/projects`: `{ "id"?: "slug", "name": "...", "repositories": [{ "id": "app", "path": "/abs/root" }] }`
  inspects each repository root read-only (identity, HEAD, branch, dirty state) and registers it.
  Non-repositories, nested paths, bare or unborn repositories are rejected with 422, as are a
  repository inside `LUUNA_DATA_DIR` and one that contains it; a checkout already registered by
  any project is rejected with 409. Git's own refusals (for example dubious ownership) are reported.
- `POST /api/v1/projects/{id}/discover`: re-observes the registered repositories and records a
  proposed manifest with per-recipe evidence, confidence and open questions. It reads manifest
  files only; it never runs a repository script. Evidence from files with uncommitted changes is
  only `inferred`.
- `PUT /api/v1/projects/{id}/manifest`: `{ "expected_revision": n, "manifest": {...} }` confirms
  revision `n+1` after re-checking identity, remote (by equivalence) and default branch (current,
  local, origin's HEAD or an `origin/<name>` remote-tracking branch), and records the exact HEAD
  commit per repository as the base. Stale revisions and changed identities return 409.
- `POST /api/v1/projects/{id}/repositories/{repository_id}/rebind`: `{ "local_path"?: "/abs/root" }`
  explicitly re-points a registered repository at the checkout now at that path (default: the
  registered path), records `repository.rebound` with the old and new identity, and returns the
  project. It is the recovery every identity-mismatch error names; it is refused (409) while a
  run of the project is queued, running, exploring or implementing.
- `GET /api/v1/projects/{id}/events?after=0&limit=100`: durable project stream (`run_id` is
  `project:{id}`); `manifest.confirmed` carries the complete manifest for replay. `limit`
  (1..=10000) is optional; a cursor above 9223372036854775807 is a 400.

- `POST /api/v1/projects/{id}/baselines`: `{ "repository_id": "app", "include_optional": false }`
  starts a baseline run (202) that executes the confirmed required recipes for that
  repository on a clean checkout at its exact HEAD. It is an ordinary run: follow it with
  `/runs/{id}/events` or `/stream`, cancel it with `/runs/{id}/cancel`. A dirty checkout
  (the reason names the paths; Git-ignored files never count), missing executable or
  dependency (exit 127, `ModuleNotFoundError`, an unresolved Node package), incapable host,
  empty or skip-only suite or container profile is recorded as blocked; failures and
  deadlines are failed; nothing is ever promoted to passed. A second baseline for a
  repository whose baseline is still running is refused with 409 naming that run.
- `GET /api/v1/projects/{id}/baselines`: recorded baselines with their check results,
  bound to manifest hash, commit, tree and environment fingerprint. Each result's
  `artifact_refs` (and the keys of `artifact_hashes`) are `{recipe}.{stdout|stderr}`;
  baselines recorded before 2026-10-21 carry `artifact:{run}:{recipe}:{stream}` refs keyed
  `stdout`/`stderr`; their logs are served under the new identity when the recipe ID is
  lower-case (other IDs now map to escaped file names, so case-insensitive filesystems
  and Windows device names cannot mix logs up).
- `GET /api/v1/runs/{id}/artifacts/{recipe}.{stdout|stderr}`: the private check log by
  record identity (text, capped at 16 MiB).

- `POST /api/v1/projects/{id}/tickets`: `{ "idempotency_key", "title", "intent", "acceptance_criteria": [...], "repository_ids": [...] }`
  creates a legacy temporary ticket or an enabled project's Missions-backed ticket (201; 200 when the key repeats with the same content,
  409 when it repeats with different content). `GET` lists them; `GET /api/v1/tickets/{id}`
  reads one, with `usage` (`workflows`, `attempts`, `elapsed_seconds`, `cost_usd_micros`,
  `cost_unreported`, `repairs`) summed over every workflow of its plan revisions. This is the
  only ticket creation route (ADR 0005).
- `GET /api/v1/projects/{id}/components` reports activation/toolchain state. `POST` initializes
  private managed component stores and migrates legacy ticket IDs/revisions without importing
  developer or personal stores. Incomplete activation freezes affected actions until reconciled.
- `GET /api/v1/tickets/{id}/source` observes authoritative Missions freshness/status/dependencies;
  `POST /api/v1/tickets/{id}/reconcile` with `{ "expected_revision" }` advances a changed projection.
  Ticket GET/list retain the last reconciled view; external changes block stale work and require
  a new plan after reconciliation. Missing components fail closed for enabled projects.
- `GET/POST /api/v1/projects/{id}/memory` retrieves/creates Atlas memory with explicit scope and
  provenance. User memory retrieval needs `include_user=true`; creation requires
  `{ "idempotency_key", "scope": "project|process|user", "content", "provenance" }`.
  `DELETE /api/v1/projects/{id}/memory/{memory_id}?expected_hash=...` checks the displayed hash.
  `POST /api/v1/projects/{id}/memory/{memory_id}/promote` with `{ "idempotency_key", "expected_hash", "to_scope": "project|user" }`
  widens a memory one step (process to project, project to user), creating the wider record with
  provenance naming its origin and deleting the narrower one under the intent ledger (ADR 0021).
- `GET /api/v1/projects/{id}/export`: the confirmed manifest, ticket projections, project and
  process memory and current prompts, with `omitted` naming what never leaves this host. The
  export is refused (409) rather than redacted when any string would carry user memory, a
  credential value, the data directory, a registered checkout path, the home directory or an
  absolute path of this host's shapes.
- `POST /api/v1/projects/scaffold/propose`: `{ "profile", "name", "parent_dir" }` returns a hashed
  `ScaffoldProposal` (files, baseline recipes, generator and network steps, tool probes,
  `ready|blocked`) and writes nothing. `POST /api/v1/projects/scaffold` with
  `{ "proposal_hash", "idempotency_key", "confirm": true, "confirm_network"? }` writes the files,
  commits, runs the baseline recipe through the supervised executor, registers the project, runs
  discovery and records `project.scaffolded`; a failing baseline is returned as such (ADR 0019).
  Discovery responses carry `host_capabilities` and each proposed check its `capability` and
  `missing_tools`.
- `POST /api/v1/runs/{id}/publish`: `{ "expected_hash", "idempotency_key" }` publishes a `reviewed`
  candidate with a `passed` review to GitHub (push of the exact commit to
  `luuna/<ticket>/<hash prefix>`, one linked pull request per repository), once per key, under a
  durable intent ledger. `GET /api/v1/runs/{id}/publication` returns the record with CI and merge
  observation refreshed; `POST /api/v1/runs/{id}/publication/reconcile` retries only missing
  steps. There is no merge operation (ADR 0018). Needs `LUUNA_GITHUB_TOKEN`; `LUUNA_GITHUB_API_URL`
  selects the REST base for tests.
- `GET/POST /api/v1/projects/{id}/prompts` lists/creates Beacon prompts. Creation/update requires
  `{ "idempotency_key", "name", "body", "expected_version"? }`; an existing name needs its current
  expected version. `GET /api/v1/projects/{id}/prompts/{name}/render?version=...` previews exact
  current content/version/hash. A stale version is refused.
- `POST /api/v1/tickets/{id}/plans`: `{ "task": { "repository_id", "paths": [...], "operations": [...] }, "permissions"?, "budgets"?, "component_context"?, "idempotency_key"? }`
  records the next immutable plan revision bound by the runtime to the ticket revision, the
  current manifest hash, its recorded base commit and its recipes, and opens the workflow run
  (`plan_ready`). Returns `{ plan, run, ticket_usage }` (201); `ticket_usage` is the ticket's
  `usage` above, so an approver sees what earlier revisions spent before approving fresh
  budgets. An `idempotency_key` (1..=128 visible ASCII characters) replays: the same key and
  request return the plan and workflow it created (200), a different request is 409. Earlier
  revisions still `plan_ready` or `approved` can never be approved or implemented again and are
  settled `cancelled` with a `plan.superseded` event. `GET` lists a ticket's plans; `GET /api/v1/plans/{id}`
  reads the latest revision. Optional selection is `{ "memory_ids": [...], "prompt": { "name", "version", "hash" }? }`;
  the runtime resolves exact content/provenance snapshots into the canonical immutable plan.
  Approval and worker/evidence boundaries recheck selected source freshness. The application
  sends only explicitly selected, approved content to the writer/reviewer (ADR 0014).
- `POST /api/v1/plans/{id}/approvals`: `{ "plan_revision", "plan_hash", "approved_by" }` approves
  exactly that hash; permissions are copied from the plan. Returns `{ approval, run }` with the
  run `approved`; a stale hash or revision returns 409.
- `POST /api/v1/runs/{id}/implement`: `{ "task_id"?, "provider"?, "model"? }` schedules the
  writer (202) after revalidating the approval, ticket revision, manifest, base commit, passed
  baseline, host fingerprint, repository HEAD, lease exclusivity and attempt budget; any refusal
  is 409/422 with the reason and leaves the run `approved`. One writer per repository is
  re-checked in the transaction that admits the writer, so a concurrent second workflow gets
  409 rather than a 202 whose writer then fails. The writer runs the pinned Pi inside a
  Luuna-owned detached worktree under the data directory with file tools only and the scope
  guard loaded; in-scope changes become a candidate commit and the run enters `validating`.
- `GET /api/v1/runs/{id}/worktrees`, `GET /api/v1/runs/{id}/attempts`: the run's worktree
  leases (path, base, state, candidate commit) and its attempts. A settled attempt keeps the
  provider, model, host fingerprint, Pi version and `trigger` (`implement`, `repair`,
  `automatic_repair`) it was admitted with beside its reason and usage.

Repository inspection runs `git` with optional locks disabled and repository-controlled
`core.fsmonitor`/hooks neutralised, so the developer's index and working tree stay
byte-identical. Remote URLs are redacted before storage. Registering or confirming a
project enables no write or plan transition. Feature implementation can be entered only
through the storage gate that checks the durable approval, the current manifest and a
passed, matching baseline per repository in one transaction (see ADR 0007); the first
writer that uses that gate is M2c.

Only one runtime can own a data directory. Startup recovery marks abandoned
active attempts interrupted after obtaining that exclusive ownership. A
Ctrl+C or SIGTERM shutdown (on Windows also Ctrl+Break, console close and system
shutdown, best effort because Windows ends the process within seconds; use the
endpoint above from a supervisor) closes admission, signals owned workers to
stop, waits for supervised cleanup (process groups reaped, at most five seconds),
and records the stopped work as `interrupted` with the reason "interrupted by
runtime shutdown …", the same resumable state a hard kill leaves after restart;
`cancelled` is reserved for the developer's cancel request (ADR 0004). Every
step is bounded, so the process always exits; a second Ctrl+C or SIGTERM exits
at once (status 130) without waiting, leaving any unsettled work to restart
recovery. Shutdown start, the number of signalled runs and the outcome are
written to stderr.

Every worker (demo run, baseline, writer, validation with its automatic
repairs) runs under a supervisor. If a worker panics, its run settles `failed`
with the reason "internal error: the runtime worker stopped unexpectedly", its
slot is released, and unreported spend is reserved as restart recovery reserves
it: a running attempt takes the remaining approved time and cost, a candidate
stage keeps its elapsed time (a review also the remaining cost) and an active
lease is abandoned with its files preserved. Deadlines derived from an approved
budget saturate instead of overflowing the platform clock.
At most four runs (demo, baseline, writer or validation) may be active at once. Prompt acknowledgements and low-level
`agent_end` events do not complete a run: Pi 1.3.0's `agent_settled` is required.
Completion means the connectivity request finished, not a feature passed
validation or a publication was approved.

Each attempt records a `pi.spawned` event with the owned child's PID, the
required Pi version, and the names (never values) of provider credentials the
child received. The terminal `run.*` event is persisted only after that child
has been stopped and reaped, so a PID that is still alive after a terminal event
is a defect. On Unix, `SIGTERM` and Ctrl+C both take the orderly shutdown path.

Session system prompts reach Pi as a private file (`--system-prompt <path>`),
never as argv text. Pi runs offline, so its `grep` and `find` tools work only
where ripgrep (`rg`) and fd (`fd` or `fdfind`) are already on PATH; writers and
reviewers omit them otherwise, and `pi.spawned`/`review.started` list the
`omitted_tools` with the reason. When Pi exits or stops reading, the failure
reason includes its exit status and a redacted 2 KiB tail of its stderr (for
example an unknown provider). Records over 1 MiB fail the session, except the
session events whose size grows with the run (`agent_end` and `turn_end` repeat
every tool result): those are read as a bounded summary marked `luuna_summary`.

`node scripts/m1-smoke.mjs --mode real --provider <p> --model <m>` drives a
built runtime binary through the full M1 checklist (authentication, SSE
streaming, settlement before completion, in-flight cancellation with reaping,
provider failure, hard-kill restart interruption, exclusive ownership, orderly
shutdown) and writes a sanitized `report.md`/`report.json`. `--mode fixture`
runs the same checklist against the synthetic fixture and is never provider
evidence.

`cargo test -p luuna-runtime -p luuna-engine-pi` uses the labeled synthetic
Python protocol fixture in `fixtures/pi-rpc`. Python 3 is required for these
subprocess tests. The genuine credential-free RPC probe is documented there.

## Candidate checks, independent review and repair (M3)

After a writer records a candidate, `GET /api/v1/runs/{id}/candidate` returns its exact commit/tree/hash, validation generation, checks, structured review, active state and the runtime's decisions: `validate_allowed`, `repair_allowed`, `revalidate_allowed` and `decision_reasons` (`validate`, `repair`, `revalidate`: the refusal the route would give, or `null`), computed by the same storage gates the routes apply; repository, host and component-source checks stay with the routes. It also reports `usage`: the whole workflow (writer attempts, validation and review) against the approved plan as `{ attempts, max_attempts, elapsed_seconds, timeout_seconds, cost_usd_micros, cost_budget_usd_micros, cost_unreported, repairs, max_repairs }`. `cost_unreported` is true once a writer or reviewer reported tokens without a cost (an unpriced, custom or local model; a `usage.cost_unreported` event records it): such a model is not refused, but the cost budget is enforced against Pi's price table only and does not meter that spend. Pi's compaction (summarization) usage is charged with the session's messages. `GET /api/v1/runs/{id}/diff` returns the diff between the frozen base and candidate commits with repository-selected execution drivers, text conversion, filters and `-diff`/`binary` attributes neutralised (binary or non-UTF-8 content is summarised by blob ID), `truncated`, every changed path in `files` with how it is shown (`text`, `summary`, `omitted`), `attention_paths` for changed `.gitattributes`/`.gitignore`/`.gitmodules`, and `worktree_drift` when the owned worktree no longer holds the candidate.

`POST /api/v1/runs/{id}/validate` accepts `{ "candidate_hash": "sha256:…", "provider": "…", "model": "…", "auto_repair": false }`. Provider/model may use server defaults. Each validation generation first verifies the owned worktree's HEAD, tree and tracked content and removes every untracked and ignored path (`candidate.worktree_cleaned` records them), so checks and review see exactly the candidate tree. A worktree changed outside Luuna, or tracked content a check rewrites, fails the candidate with a `candidate content drift:` reason naming the paths and next step (repair or the next plan revision); such a candidate is never re-validated in place. Required checks execute in the owned worktree through the supervised executor. A fresh Pi process with read-only tools reviews requirements/code/check records and a diff bounded to 256 KiB of whole file sections plus every changed path, without the implementation conversation; its tool calls are recorded as `review.tool_call` events (tool name and path only), and output that is not a valid, self-consistent review (a `changes_required` or `blocked` verdict must name a finding) is kept as the private artifact `<reviewer session>.stdout` and leaves no verdict. A valid pass reaches `reviewed`; infrastructure failures, invalid output, stale identities and blocking findings never pass. The same request re-validates a `failed` candidate in place when every required recipe passed in its current generation and no review verdict was recorded (invalid or self-inconsistent reviewer output, a refused reviewer session): generation `n+1` of the same revision opens under the same approval and budgets, the checks run again, a fresh reviewer starts with the generation named in its prompt, and no writer attempt is spent. A failed, blocked or interrupted check, a recorded verdict of any kind, and reserved spend from a reviewer lost mid-turn are refused with 409/422, as for repair. The test-count heuristic recognises cargo, go, pytest, vitest/jest and Python `unittest` summaries; a passed command that reports zero tests is blocked as an empty suite.

`POST /api/v1/runs/{id}/repair` accepts the same candidate hash and optional model settings. The repair gate rechecks current approval, ticket/manifest/base, passed host-matching baseline and all aggregate budgets. Repairs create a fresh candidate revision and require fresh checks/review. The repair writer's prompt quotes the previous candidate's reason, check records, structured review and the last 2 KiB of each failed check's recorded stdout/stderr, at most 16 KiB in total measured after JSON encoding with the last entry cut from the front to fit, from at most 32 artifacts and 64 MiB read per repair on a blocking thread, each opened without following links and labelled from the check result's recorded `truncated_artifacts` when the executor cut it at its 16 MiB cap (with every configured provider credential value of any length and the service token redacted longest-first from the whole artifact before the tail is cut, PEM private-key blocks and bearer/API-token-shaped lines removed best effort, and only when the artifact still matches the digest recorded with the check result), because the writer has no shell and cannot read private artifacts; a repair that only saw an exit code could not act on it. Set `auto_repair: true` on validation to opt into bounded repair cycles for ordinary failed commands or `changes_required` reviews; infrastructure failures stop without automatic edits. The existing cancel endpoint covers the active pipeline, including automatic repairs. Candidate log artifact IDs include `c{revision}-g{generation}-{recipe}.{stdout|stderr}`.

While a validation or review stage runs, its durable heartbeat (`stage_checkpoint_at`) is stamped every five seconds; restart recovery charges an interrupted stage up to its last heartbeat plus one interval, so time the runtime was down is not charged, and the stage needs a fresh generation. A reviewer's provider spend is unknown from the durable `review.prompt_sent` marker, written just before its prompt, until `review.usage_settled`; only a restart in that window reserves the remaining approved cost budget, so recovery can require a new plan/approval rather than retry with fresh budget. A reviewer interrupted before its prompt can simply be validated again. Reviewer usage is persisted when a message completes, the session ends or the budget check fails, not per streaming record, and a provider error that Pi retries itself (`agent_end` with `willRetry`) does not fail the review. An automatic repair whose writer fails pauses with that writer's own reason (`automatic repair writer failed: …`) and whether another repair may follow; automatic repair writers run on the validation request's provider and model. A repair writer is told it starts from the previous candidate's commit, and quoted check output names files relative to the repository root rather than the previous attempt's worktree. Native host permissions remain unchanged. No publication or merge endpoint is introduced. Real-provider M3 acceptance is recorded separately from deterministic fixture evidence.

`node scripts/m3-smoke.mjs --mode real --provider <p> --model <m>` drives a built runtime binary through the M3 checklist on a disposable fixture repository: register/confirm/baseline with a real `unittest` check, an approved feature → required check → fresh independent reviewer, a seeded failing check → manual repair revision 2 → fresh evidence, a blocking-review probe under bounded `auto_repair`, cancellation while the reviewer is in flight (child reaped), a hard-kill restart during review (interrupted, spend reserved, re-validation refused), developer-checkout integrity and orderly shutdown; in fixture mode only, a sixth scenario re-validates a candidate whose synthetic reviewer returned invalid output after passed checks (no real provider can be made to do that on demand). It writes a sanitized `report.md`/`report.json` that records credential names only and is scanned for leaked values. `--mode fixture` runs the same checklist against the synthetic fixture and is labeled synthetic; it is never provider evidence. The harness builds the runtime itself from the committed tree (`cargo build --locked -p luuna-runtime` with the pinned toolchain's own cargo and rustc, into a new empty target directory for every run, no `--config`, an allowlisted environment; `--runtime` is refused), starts it once without any provider credential to verify its provenance, and gives credentials only to that verified binary: `build.rs` embeds a SHA-256 over every build input, `/health` reports it as `build_identity`, and each run fails unless it equals the digest of the checkout's build inputs and those inputs have no modified or untracked file. On Linux the leak scan and quarantine resolve paths through held directory handles; elsewhere the run records that they are unavailable and refuses recursive cleanup. On Linux the harness also re-executes itself under a `python3` child subreaper and kills every process of the run that survives the runtime before the leak scan (elsewhere containment cannot be established, so that check fails and the scan is marked untrusted), then re-checks that the Luuna checkout is still the recorded revision, clean, with the same build inputs, Git metadata, history (every object reachable from any ref, verified by `git fsck --full`) and ignored files (outside `target/`, `node_modules/` and `dist/`); a Cargo config file that mentions `paths`, `include`, `replace-with`, `patch`, `profile`, `rustc`, `linker` or `env`, or contains a backslash, fails the build-provenance check, as does a binary whose reported `build_rustc_sha256` differs from the pinned toolchain's `rustc` (located with `rustup which`), so evidence builds use a `CARGO_HOME` without such config.

## Backup and restore

The database (`<data>/luuna.sqlite3`) runs in WAL mode: recent commits can sit in `luuna.sqlite3-wal` until a checkpoint. Never copy only the main file while the runtime runs, and never put an older main file back beside a `-wal`; SQLite would replay the newer log onto it and corrupt it. Use these commands. Each takes `--data-dir <dir>`; the default is the service's own (`LUUNA_DATA_DIR`, else `.luuna`). See [ADR 0016](../../docs/decisions/0016-storage-backup-and-schema-compatibility.md).

- `luuna-runtime backup --output <file>` writes a consistent snapshot while the runtime is running. It never overwrites a file, and it prints the schema version, size and SHA-256.
- `luuna-runtime restore --input <file>` needs the runtime stopped; it is refused while a runtime owns the data directory. It verifies the snapshot's integrity and schema version, moves the current database and any `-wal`/`-shm` to `<data>/backups/replaced-<timestamp>/`, preserves `<data>/project-records/` in the same replaced folder without changing its bytes, and installs the snapshot. The next start backfills project files from the restored database; work that was active in the snapshot is recorded `interrupted`, never successful.
- `luuna-runtime check` reports integrity (`PRAGMA quick_check`) and the schema version without writing. It is safe while the runtime runs and exits non-zero on a problem.

What a snapshot covers: the database snapshot holds projects, manifests, plans, approvals, runs, events, leases, attempts and the check/review evidence records. Project record files, check artifacts, worktrees, attempt directories and managed component stores are files in the data directory. With a deleted database, `<data>/project-records/<id>/project.json` rebuilds registrations, repository bindings, confirmed manifests and retention settings. The sibling `components.json` rebuilds component activation and memory bindings, including timestamps and tombstones, without native content or operation receipts ([ADR 0055](../../docs/decisions/0055-component-record-files.md)). These files do not recover execution state or other SQLite-only records, and a database snapshot cannot restore native bodies that were later edited or deleted. A complete backup is a copy of the whole data directory taken after an orderly stop (`POST /api/v1/shutdown`, Ctrl+C or the stop script), which checkpoints the log. To restore a complete copy, stop the runtime and replace the directory as a whole.

Schema compatibility: at startup the runtime refuses a database that fails `quick_check` or that was written by a newer schema. The message names the file and `luuna-runtime restore`. Before migrating an existing database to a newer schema, the runtime saves `<data>/backups/pre-migration-v<old>-<timestamp>.sqlite3`. These snapshots are never deleted automatically. Startup also refuses a damaged, newer or incompatible project file with a `project file` error; preserve the folder and repair the file or restore a complete backup rather than deleting records to force startup ([ADR 0047](../../docs/decisions/0047-project-files-and-rebuildable-index.md)). To step back to the build that wrote one, stop the runtime, run `luuna-runtime restore --input <snapshot>`, then start that older build.

Startup recovery skips a record it cannot decode instead of refusing to start. It logs the table and ID to stderr, never the content. Where the record has a state column, that column is settled: the record is interrupted or abandoned, never resumed.

## Diagnostics

`luuna-runtime doctor [--data-dir <dir>] [--json]` reports the host prerequisites (Git, Python, Node, the pinned Pi probe without a model call), the data directory and database, `LUUNA_API_TOKEN` and `LUUNA_GITHUB_TOKEN` presence, configured provider credential names, the service port and the component toolchain. It exits 1 when Git, Pi or a writable data directory is missing or an existing database is unusable, and never prints a credential value (ADR 0018).
