# ADR 0063: Removing a project

Date: 2026-11-26. Status: accepted. The owner chose "hide it, keep its records" over deleting Luuna's records. Builds on [ADR 0047](0047-project-files-and-rebuildable-index.md).

## Decision

1. **Removing hides; nothing is deleted.** `DELETE /api/v1/projects/{id}` sets `removed_at` on the project and records a `project.removed` project event. The action is cataloged as `projects.remove`, with client method `removeProject`. The desktop control is "Remove project" in the project panel, behind a confirmation checkbox. The terminal command is `remove-project <project> --confirm`. Its tickets, runs, evidence, events, discoveries, component records and stores, and its `project-records/<id>/` folder all stay where they are.
2. **A removed project is off every list.** `GET /api/v1/projects` leaves it out, and every route under `/api/v1/projects/{id}` answers 404, so neither client shows it or acts on it. Its runs leave the run listings too, though each stays readable by ID. Scaffolding a new project under its ID is refused with 409.
3. **Its folders are free.** A removed project's checkouts no longer count as registered, so another project may register them. Its ID stays taken, and a registration under that ID with other folders is refused with 409.
4. **Adding the same folders again brings it back.** A registration under the removed project's ID, with the same repository IDs at the same folders, restores it. So does a registration without an explicit ID whose folders a removed project registered in exactly that way. The project comes back with its name, manifest and records, records a `project.restored` event, and answers 200 rather than 201. If another project has since registered one of those folders, the usual one-registration-per-checkout refusal applies and it stays removed.
5. **Removal waits for running work.** It is refused with 409 while a run may be working in one of the project's checkouts: a queued or running baseline, or an exploring or implementing workflow, or while an Orbit command for the project is in flight. A run admitted in the same moment as a removal cannot slip past it: run admission checks for removal inside the same transaction and answers not found.
6. **The project file records the removal.** `project.json` gains an optional `project.removed_at`. It is absent for a listed project, so files written before this change are unchanged, and the file schema version stays 1. A rebuild from files keeps a removed project removed. Database migration 13 adds the `removed_at` column.

## Consequences

A developer can tidy the project list and reuse a folder without losing history. Deleting a project's records for good is still a manual step: stop the runtime and remove its `project-records/<id>/` folder and component stores. That is deliberate, because it cannot be undone.

## Evidence

`crates/luuna-runtime/tests/projects.rs` covers removal through the API with a real git checkout: authentication, 404 afterwards, the empty list, a second removal, a refused registration under the ID with another folder, restoring under the ID and without one, the event record, a refused scaffold under the removed ID, a new project taking the freed folder, and the removed project staying removed while that new project holds it. A storage test removes a project, checks that its runs leave the listings and no new run is admitted, rebuilds the database from files with another project now holding the same checkout, and checks the removal, the refused restore and the later restore. A runtime unit test checks that the Orbit in-flight count is released when each operation ends. The terminal test checks that `--confirm` is required and that one DELETE is sent. A desktop test checks the confirmation and wording, and the catalog contract checks cover the new action.
