# ADR 0055: Component records and their rebuildable index

Date: 2026-11-15. Status: accepted under the owner's migration charter, step 1, slice 7d. Extends ADR 0047; changes no existing stored format or execution authority.

## Decision

Component activation and memory-binding metadata live in `<data_dir>/project-records/<id>/components.json`; SQLite indexes `component_projects` and `component_memories`. Each project has its own file, so a write neither rewrites unrelated projects nor shares an installation-wide size ceiling. Schema version 1 has fixed `schema_version`, `project_id`, `projects` and `memories` fields. Activation rows retain project id, phase and update timestamp. Binding rows retain opaque id, creator project, scope, native source id, domain, creation timestamp and deletion tombstone. Rows are ordered by their primary keys; identities and timestamps are copied, not regenerated.

These are private runtime metadata files, not native component stores and not a second store-mode setting. They contain no Atlas content or provenance, provider material, operation keys or receipts. User memory remains in the one private native user store and still requires explicit inclusion/selection through the API. Mode selection remains solely in `projects/<id>/store.json`; these files are outside checkout stores and project exports. Edit metadata only while the runtime is stopped; an open store writes its current indexed state.

The five business writers (begin/finish activation, record/delete binding, and the binding-time test seam) hold an immediate SQLite transaction, render the affected project file, replace it privately and atomically, then commit SQL. A refused write rolls SQL back; a failed SQL commit restores the exact previous bytes or removes a newly created file. Memory-only stores perform no file I/O. File reads/writes are bounded to 8 MiB per project; links/reparse points, wrong file types, unknown/missing/wrongly typed fields, unnamed positional arrays, duplicate records/fields and newer versions are refused without quoting content.

Startup reads and validates both record forms before mutation, plans both indexes under one immediate transaction, and checks project foreign keys against existing registrations and valid project files. Binding ids must be globally unique across files and database projects. File-only records rebuild the index; database-only records backfill files; equal records are unchanged. File-ahead activation, new bindings, timestamps and tombstones can bring the index forward. A missing database row's history, changed immutable binding identity, enabled-to-initializing regression or resurrected tombstone refuses startup. A file cannot be moved to another project's folder to adopt its data.

The writer lease now stays held through missing-file backfill for both project and component records, then commits. This strengthens ADR 0047's backfill ordering: another Store cannot race a newer file write against an older startup backfill. Backfills copy only pre-existing database records, so they remain valid and retryable if the eventual commit fails. No table, column, Cargo dependency or schema-version change is needed.

## Restore and limits

The existing resumable restore already preserves the entire `project-records/` tree beside the replaced database. That includes component metadata, byte for byte. Restart backfills files from the restored database, retaining snapshot rollback semantics without changing restore output, intent or report formats.

A database snapshot contains the indexed metadata but does not contain native store bodies. Restoring older bindings cannot recreate a native record that was subsequently deleted or roll back an edited native body. A complete backup remains the entire data directory copied after an orderly stop, including native project/user stores and artifacts.

The component operation ledger remains authoritative in SQLite and is never rebuilt from these files. Plans, approvals, runs, attempts, execution events/evidence and publication also remain SQLite authority; these files grant no execution authorization. Project event history/discoveries and legacy tickets/Missions projections remain separate unfinished record-index slices. Losing SQLite still loses SQLite-only state and receipts; recovery of metadata is not a substitute for complete backups.

## Verification

Storage regressions cover exact scalar recovery, multiple projects, unrelated-file preservation, global id collisions, malformed/linked/oversized/newer records, immutable-history/tombstone refusal, write and deferred-commit failures, memory-only isolation and real snapshot rollback. A runtime test creates real native Atlas records in temporary stores, loses SQLite, and verifies unchanged record identities, hashes and bodies while user inclusion remains explicit. These use synthetic data and no model; platform and full-suite results are recorded separately for each implementing head.
