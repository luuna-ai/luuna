# ADR 0060: Ticket files

Date: 2026-11-26. Status: accepted under the owner's migration charter, step 1, slice 7c. Extends ADR 0047, ADR 0055 and ADR 0057. It changes no table, no column and no execution authority.

## Decision

Each project's tickets now have a file: `<data_dir>/project-records/<id>/tickets.json`. It holds schema version 1, the project id, and one entry per ticket in creation order. Each entry has the idempotency key, the ticket as stored, and, for a ticket handed to Missions, its delivery binding: the Missions source id and source revision once known, whether a create was started, and the original content first offered to Missions. Those are the rows of `tickets`, `ticket_sources` and `ticket_delivery_attempts`, so a rebuilt project keeps its ticket ids, revisions, timestamps and idempotency keys, and a retried Missions delivery still knows that a create may already have happened.

The database is still the writer. Every ticket writer (create, revise, reserve, begin and complete a Missions delivery, reconcile a Missions projection) commits first. The store then takes a fresh writer lease and replaces the file from the database through a private temporary file and a rename. So the file can fall behind the database after a crash, but it never moves ahead of it. A failed replacement is not an error for the caller, because the change is already committed. The next open repairs it.

At open, after the project, component, event and discovery records are indexed, each ticket file is validated in full. Tickets must belong to this project, pass ticket validation, and have unique ids, idempotency keys and Missions source ids. A Missions ticket must carry its binding. Unknown fields, a newer schema version, linked or oversized files (over 64 MiB) and more than 1000 tickets are refused. Luuna never writes a file it would refuse: a project whose snapshot would pass 64 MiB keeps its tickets in the database alone, and its file is left as it last stood until the snapshot fits again. Then the file and the database are compared ticket by ticket:

- If they are equal, nothing changes.
- A missing file, or a file behind the database, is rewritten from the database. "Behind" means every ticket in the file is also in the database at the same or an earlier point: the same id, project, creation time and key, a revision no later, and a delivery stage no later (none, reserved, started, bound).
- A database behind the file takes the file's tickets and bindings. This covers a lost database, which is the purpose of this slice. Imported tickets must name repositories registered for the project.
- Anything else refuses the open before any row or file changes. Examples are an edited ticket at the same revision, a changed Missions source id or original content, a ticket id that belongs to another project, and a file moved into another project's folder.

At one revision, the only change allowed is binding the delivery, which turns the ticket's source into Missions. Every other change to a ticket bumps its revision, so this order is total for tickets that Luuna wrote.

## Why a whole-file snapshot

Tickets change in place, unlike events. The database keeps only the current revision of each ticket, and binding a Missions delivery updates a ticket without bumping its revision. An append-only log would need a revision history the database does not keep. Rewriting one project's file is bounded by the 1000-ticket ceiling and happens only on ticket writes, not on run events.

## Consequences

Losing SQLite no longer loses a project's tickets or their Missions links. For a Missions-backed ticket, the Missions store stays the authority for its content. The rebuilt row is the last projection Luuna made, and the next reconcile brings it forward as before. Plans, approvals, runs and plan requests still live only in SQLite, so a rebuilt ticket has no plans until it is planned again. These files are private runtime metadata. They are not exports and grant no authority. Edit them only while the runtime is stopped; an edit that disagrees with the database refuses the next open. Snapshot restore already sets the whole `project-records/` folder aside, so a restored database backfills its own ticket files. Memory-only stores write no files.

With this slice, every record the store-index plan names now has a file form or is SQLite-only by design (run lifecycle, plans, approvals, evidence, publication and the component operation ledger). The remaining store slices are 4b, 5 and 6 (repository and both modes, and choosing a mode), plus the optional offline reindex command (7e).

## Evidence

`crates/luuna-storage/tests/ticket_index.rs` covers: every ticket writer keeps the file equal to the database; each Missions delivery step is in the file before the next one starts, so a database lost after a started create still refuses a blind second create; rebuild after losing SQLite, with idempotent create and continuing revisions; backfill of a missing file; a file behind is rewritten and a database behind is brought forward; refusal of same-revision edits, a changed source id, a missing binding, another project's ticket, duplicate keys, unknown fields, a newer format, non-JSON and a misplaced file; linked files; snapshot restore; and memory-only stores. It uses synthetic data only.
