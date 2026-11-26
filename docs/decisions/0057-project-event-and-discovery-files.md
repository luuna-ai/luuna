# ADR 0057: Project event and discovery files

Date: 2026-11-26. Status: accepted under the owner's migration charter, step 1, slice 7b. Extends ADR 0047 and ADR 0055. It changes no table, no column and no execution authority.

## Decision

Each project's event stream and its discovery proposals now each have an append-only file: `<data_dir>/project-records/<id>/events.jsonl` and `discoveries.jsonl`. Every line holds exactly the text of one database row: the event envelope for an event, the discovery payload for a discovery. Events are kept in sequence order, discoveries in the order they were recorded.

The database row is written first. Once the commit succeeds, the store takes a fresh writer lease and appends whatever the database holds past the file's last line. As a result, a file is always a prefix of the database. A failed append is not an error for the caller, because the change is already committed. The next open appends whatever is missing.

At open, after the project and component records are indexed, each log is validated in full. Every line must belong to this project. An event must be the next sequence of the project's stream, with its derived event id. A discovery must not repeat an id. Then:

- A file that is behind the database, missing or torn (a last line cut short before its newline) is rewritten from the database through a temporary file and a rename.
- A database that is behind the file imports the rest of the file. This covers a lost database, which is the purpose of this slice.
- A file whose shared prefix differs from the database refuses the open before any row or file changes. So do a malformed, linked, oversized or misplaced file, or a file naming an unregistered project.

Snapshot restore already sets the whole `project-records/` folder aside, so a restored database backfills its own logs on the next open. Memory-only stores write no files. Lines are not size-bounded on write, but at open a line over 1 MiB or a file over 256 MiB is refused.

## Consequences

Losing SQLite no longer loses project history or the latest discovery: the project event cursor and `latest_discovery` behave as before, and the next event continues the sequence. These files are private runtime metadata. They are not exports and do not authorize anything. Run events, plans, approvals, attempts, evidence, publication and the component operation ledger stay SQLite authority. Tickets and the Missions projections follow in [ADR 0060](0060-ticket-files.md). Edit these files only while the runtime is stopped; a hand-edited line that disagrees with the database refuses the next open.

## Evidence

`crates/luuna-storage/tests/project_log.rs` covers appends on write, rebuild after losing SQLite, backfill of missing files, catching up a file that is behind, importing a file that is ahead, torn tails at open and while open, refusal of edited, malformed, duplicated, shifted and cross-project lines, linked files, snapshot restore and memory-only stores. It uses synthetic data only.
