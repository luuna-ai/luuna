# ADR 0045: per-project store mode and one store resolver

Date: 2026-11-12. Status: accepted; runtime-mode behaviour is unchanged. Builds on [ADR 0003](0003-ticket-authority.md), [ADR 0014](0014-runtime-component-workflows.md), [ADR 0016](0016-storage-backup-and-schema-compatibility.md), [ADR 0021](0021-memory-promotion-and-project-export.md) and [ADR 0028](0028-luuna-project-component-layout.md), which all remain in force.

## Context

Every project's component data (the Missions, Atlas and Beacon stores) lives today in the runtime data directory: `<data_dir>/components/projects/<id>` per project and `<data_dir>/components/user` for user memory ([ADR 0014](0014-runtime-component-workflows.md) decision 2, [ADR 0028](0028-luuna-project-component-layout.md)). The runtime reached that location from several call sites, each building the path itself. A project cannot say where its data should live, and a later choice of a repository-side store would have to be added to every one of those call sites.

A choice of store must be recorded where a developer can read it, must not create a second copy that can drift, and must never make the runtime guess: [ADR 0003](0003-ticket-authority.md) allows one writable ticket authority, and [ADR 0014](0014-runtime-component-workflows.md) forbids adopting developer stores automatically. The project is still the id in the route ([ADR 0005](0005-project-scoped-ticket-api.md)).

## Decision

1. **One resolver decides the store.** `crates/luuna-runtime/src/stores.rs` is the only place that decides which store a project's component calls use. Every runtime caller goes through it. The project is still the id in the route ([ADR 0005](0005-project-scoped-ticket-api.md)); the resolver adds no new route, action or client method.
2. **The mode is recorded in a readable project file.** Each project's store mode is the human-readable file `<data_dir>/projects/<project_id>/store.json`: pretty-printed JSON, two-space indent, trailing newline, with exactly these keys in this order: `schema_version` (integer 1), `project_id`, `mode` (`runtime`, `repository` or `both`), `repository` (required for `repository` and `both`, absent for `runtime`) and `writable` (`runtime` or `repository`; required for `both`, absent otherwise). Unknown keys, a missing key, a `schema_version` other than 1, and a `project_id` that differs from the folder name are refused. No file means runtime. SQLite holds no copy of the mode and the resolver caches nothing, so there is nothing to rebuild; a later index of the mode must be rebuildable from these files. The file lives in the data directory, outside the component workspace that the component tools own. Examples:

   ```json
   {
     "schema_version": 1,
     "project_id": "demo",
     "mode": "runtime"
   }
   ```

   ```json
   {
     "schema_version": 1,
     "project_id": "demo",
     "mode": "repository",
     "repository": "app-web"
   }
   ```

   ```json
   {
     "schema_version": 1,
     "project_id": "demo",
     "mode": "both",
     "repository": "app-web",
     "writable": "runtime"
   }
   ```

3. **Runtime mode is today's layout, unchanged.** The workspace is `<data_dir>/components/projects/<id>` and the user store `<data_dir>/components/user` serves every project whatever its mode ([ADR 0014](0014-runtime-component-workflows.md) decision 2, [ADR 0028](0028-luuna-project-component-layout.md)). A project with no file, or a file saying `runtime`, behaves exactly as before.
4. **Repository and both modes, and an unusable file, are refused.** When a store is resolved, a project whose file says `repository` or `both` is refused with 409, and so is a file that cannot be used: a link or other non-regular file, a file over 4096 bytes, an unreadable shape, an unknown key, a newer version, or a wrong project id. A link or non-directory at `<data_dir>/projects` or at the project folder is refused the same way. Other I/O errors on the file answer 500. No component data is read or written when the runtime refuses, and there is no fallback to the runtime store. On some memory routes an idempotency record of the requested operation is already kept in the database before the store is resolved; that behaviour is unchanged. No product action writes the file yet: storage can write it atomically (private permissions on Unix), tests do so, and a developer can write it by hand. A project that is not enabled for components keeps its legacy ticket source, and enabling components for it is refused before any state is recorded.
5. **The file is not part of the manifest or the export.** The mode is not added to the project manifest and is neither exported nor imported ([ADR 0006](0006-repository-import.md), [ADR 0021](0021-memory-promotion-and-project-export.md)): the manifest and its hash are a fixed stored format, and the store mode is a per-machine choice that exports keep out. An imported project starts in runtime mode unless a file already exists for its id.
6. **Choices for later changes.** These are recorded now so later work needs no format change. The `repository` field names one registered repository of the project, because a project has one component root and [ADR 0003](0003-ticket-authority.md) forbids two writable ticket authorities; this build checks only that the value is a well-formed id, and a later change checks it against the registry. Selecting `repository` or `both` is an explicit developer action and the only thing that authorizes the runtime to use that checkout's `.luuna` stores; nothing is adopted without it. The runtime never commits repository-side store files; the developer does. In `both` mode the writable side is authoritative, the other side receives a copy of changed records after each successful write and is never read as authority, and the default writable side is `runtime`. Runtime execution state (plans, approvals, runs, attempts, evidence, events, publication and the component operation ledger) stays authoritative in SQLite; only records that have a file form are candidates to become rebuildable from files, and per-project settings with no file form today may move to readable files beside `store.json`. A mode change is refused unless the target side's stores are absent or empty; records are never merged or moved automatically.

## Consequences

- No database migration and no new dependency. The schema version is unchanged, and older builds keep opening the database.
- Every component call now reads at most one small file before it resolves a store. A hand-written `repository` or `both` file makes those calls for that project answer 409 until the file is removed; this is the intended fail-closed behaviour.
- A complete backup is still a copy of the data directory ([ADR 0016](0016-storage-backup-and-schema-compatibility.md)) and now includes the project files, while `luuna-runtime backup` snapshots the database only. Restoring a snapshot keeps whatever `store.json` files are on disk.
- The capability catalog lists `refused` for the actions that resolve a project store: `components.status`, `memory.list`, `prompts.list`, `prompts.render` and `tickets.source`, in addition to the actions that already listed it. No action, route or client is added.
- A newer file version is refused by older builds, so a build never guesses at a format it does not know.
- Later work (repository-side stores, `both` mode, selecting a mode as a product action, and making SQLite a rebuildable index of file-backed records) builds on this file and resolver and records its own decisions.
