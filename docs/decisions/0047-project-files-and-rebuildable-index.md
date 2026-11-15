# ADR 0047: Project files and a rebuildable project index

Status: accepted, November 15, 2026. Implements the project-record portion of charter step 1 and ADR 0045 decision 6. Extends the restore procedure of ADR 0016 item 5; neither decision is superseded.

## Decision

Project registrations, their repository bindings and observations, every confirmed manifest revision, and the project memory-retention setting live in `<data_dir>/project-records/<id>/project.json`. SQLite indexes those four tables. The readable JSON document has schema version 1 and fixed `project`, `repositories`, `manifests` and `memory_retention` fields. Repositories are ordered by id and manifests by revision. Timestamps, hashes and identities are copied, never regenerated.

The separate `<data_dir>/projects/<id>/store.json` still selects the per-machine store mode under ADR 0045 and is not indexed in SQLite. The new folder avoids creating or adopting that mode folder when no mode file exists. The component stores themselves keep their existing formats and authority.

Each of the four project writers prepares the file from its SQL transaction, writes a private temporary file, syncs and atomically renames it, then commits the transaction. A refused file write rolls back SQL. A failed SQL commit restores the previous file. A crash after the file write but before commit is reconciled on the next open. Memory-only stores write no files. Links, wrong file types, files over 8 MiB, unknown/missing/wrongly typed fields, invalid identities and newer file versions are refused; errors identify the path and problem without quoting document content.

Every file-backed `Store::open` reads and validates all project files before reconciliation. A file-only project is indexed. A database-only project is backfilled to a file. Equal records need no change. If repository ids and project creation time agree, every existing immutable database manifest appears unchanged in the file, and no retention row is missing, the database can be brought forward from the file. Missing or differing database manifest history and incompatible records refuse startup before project changes. All index changes share one immediate transaction; backfilled files are then written. A rebuilt index has the same primary keys and scalar columns, with JSON columns equal as parsed values; whitespace and object-key order are not authority, while array order is significant.

If the database is missing, surviving WAL/SHM files are first preserved under `backups/orphaned-sidecars-<timestamp>/` and never replayed into a new database. Existing data folders without project files are backfilled. Integrity and newer-database-schema gates continue to run before migration. Files and index do not authorize execution: plans, approvals, runs, attempts, evidence, execution events, publication and the operation ledger remain authoritative in SQLite.

## Restore and recovery

`luuna-runtime restore` preserves the current `project-records/` directory beside the replaced database under `backups/replaced-<timestamp>/project-records/`, without changing its bytes. Only then is the staged snapshot installed. On the next start, the restored database backfills its project files, so a restore rolls project records back instead of immediately bringing them forward again. The printed restore output and intent/report formats are unchanged. A snapshot is still database-only; a complete backup copies the entire data directory after an orderly stop, including project files, component stores and artifacts.

An interrupted set-aside resumes if only the preserved directory exists. If both original and preserved project files exist, recovery refuses to overwrite either and keeps the intent and staged snapshot. Opening a `Store` explicitly on a replaced database treats its directory as a data folder: files reconcile there too, and missing project files may be backfilled. Such an open was already writable maintenance, not a read-only archive operation; the runtime does not open these preserved databases itself.

This slice does not rebuild project event history, discoveries, legacy tickets, component settings/bindings or any execution state. A rebuilt project with no project events starts at sequence 1 on its next event; no historical event is invented. Later slices address the remaining file-backed records and diagnostics. Losing the database still loses the SQLite-only state; keep complete backups.

## Verification

The project-file suite covers registration, missing/older/ahead databases, malformed and linked files, refused writes, memory-only stores, restore rollback and interrupted recovery. Existing restore and store-mode assertions remain unchanged. The backup recovery drill and full repository checks remain required evidence for the implementing commit; this decision does not itself establish a platform result.
