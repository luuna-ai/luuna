# ADR 0054: Luuna intent

Date: 2026-11-15. Status: accepted under the owner's signed migration charter (step 2).

## Context

ADR 0046 imported Roots with history and gave the runtime a native roots-core wrapper. It left the product routes, initialization and source of human names to this slice. This decision supersedes no ADR and preserves Roots' stored formats.

## Decision

1. Expose eight bearer-authenticated actions under `/api/v1/projects/{id}/intent`: GET and POST on that path (show and initialize); GET `/proposals`, `/questions` and `/sprouts`; POST `/proposals/{proposal_id}/accept` and `/reject`; and POST `/sprouts/{sprout_id}/reject`. Reads return native settings/graph or native record arrays. Decisions retain Roots' camelCase wire fields, including `decidedBy` and `statusChanges`.
2. Each human request requires `by`, supplied explicitly by the caller. The human name is a label the caller asserts behind the bearer token, protected only by that token, as a plan approval's `approved_by` is, and not proof of a person. Agents do not hold the token. No name comes from environment variables, Git configuration, the OS user or checkout files. The runtime rejects `agent:`, `roots` and `roots:` values; request types refuse unknown fields. Clients default to `developer`, trim the label, enforce the 200 UTF-8 byte limit, and refuse reserved identities before sending.
3. Routes use `RootsStore` and the single project store resolver. Roots does not require enabling the other components or an external toolchain. Uninitialized stores answer 409. Initialization is runtime-mode only: resolve the project, take the component lock, inspect existing managed directories/files, create private absent entries, append one native `init` event when the event file is empty, and create `config.yaml` last from Roots' own default renderer using the project id. Existing configuration answers 409 without changing bytes. A retry after an interrupted initialization does not append another event when one already exists.
4. Initialization refuses links/reparse points, wrong directory types and nonregular existing files, including managed parent directories and the event file. It writes the Roots folders `human`, `agent/sprouts` and `agent/notes`, empty graph/proposals/questions/events files where absent, and `.gitignore` containing `*.lock` and `*.tmp.*`. It creates no headings file or `.gitattributes`, runs no Git and adopts no checkout data. Filesystem work and roots-core run on the blocking pool. Route error messages are fixed text so lexical and canonical host paths cannot leak through core errors.
5. The capability catalog names each action once. The typed API client and desktop panel expose all eight. The panel sits outside component activation, has an explicit Your name field, offers initialization after a not-set-up refusal, displays proposal reasons/citations, and cancels stale requests on project/client changes. Runtime decisions remain authoritative.
6. `luuna intent <verb>` and `intent-<verb>` are the same terminal command: show, init, proposals, questions, sprouts, accept, reject and reject-sprout. The Intent (Roots) help group lists all eight. Bare `intent` prints the group and exits 2; group/command help works without a token. Existing command names and help behavior remain compatible. Required-by catalog references are R02 and the current architecture's Pi and maintained components section.
7. Agent operations remain in-process only; no route can propose, ask or file a sprout as an agent. Agent identities come only from `AgentActor::for_run` and are bound to a recorded project/run. CODEOWNERS was corrected by the history import; this slice changes no ownership file. Plant/adopt, drift/verify and other standalone Roots operations are outside this product surface.

## Consequences

Step 6 can replace this small group dispatcher with the common command grouping mechanism while retaining both names. Plant, adopt, agent paths, drift and verify need later designs. Until then, new content comes from the standalone Roots CLI or native runtime code. `AgentActor::for_run` continues accepting a recorded run of any status, including a finished or cancelled run; this decision adds no caller or agent route.

Repository and both modes remain refused by the resolver at this stage. Initialization refuses links; the existing read and decision paths still follow a link at `.luuna/roots` through `RootsStore::for_project`. Creating such a link already requires write access to the data directory. That carried behavior is not a sandbox guarantee. Execution approvals, baselines, reviews and publication policies are unchanged.

The actor/environment, route, client and terminal suites use temporary synthetic stores and no model. Successful checks establish those tested flows, not provider connectivity or an installed release.
