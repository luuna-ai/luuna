# ADR 0058: Ticket type, priority and labels

Date: 2026-11-26. Status: accepted by the owner. Extends ADR 0003 and ADR 0014. It changes no table or column and no stored record.

## Decision

A Luuna ticket can now carry the three triage fields of a Missions issue, with the same shape and values:

- `type`: `task`, `bug`, `feature` or `epic`. The default is `task`.
- `priority`: a whole number from 0 (critical) to 4 (backlog). The default is 2. The command line also accepts `P0` to `P4`.
- `labels`: up to 16 unique labels. Each label is 1 to 64 bytes, lowercase (Missions lowercases every label it stores), and contains no whitespace or comma. A label never starts with `luuna-ticket-`, which is reserved for the marker Luuna uses on the Missions records it delivers.

When a field holds its default, the ticket record leaves it out, so every ticket stored before this change keeps its exact bytes and hashes. The ticket schema, the typed client, the API's create and edit requests, `lu create-ticket` and `lu edit-ticket` (`--type`, `--priority`, `--label`, `--clear-labels`), the desktop ticket form and editor, and the terminal ticket view and board cards all carry them.

Changing type, priority or labels is an ordinary ticket edit: it advances the revision, so a plan made for the earlier revision has to be made again. This matches enabled projects, where Missions is the authority, because any change to the Missions record already changes the source fingerprint that plans bind to.

For an enabled project, the fields are real Missions fields, not part of Luuna's intent envelope. Delivery creates the issue with `--type`, `--priority` and `--labels`, with Luuna's marker first. An edit writes them with `--type`, `--priority` and `--set-labels`, keeping the marker. A reconcile reads them back from the issue and leaves out the marker. An edit recorded in the component ledger names these fields only when they differ from the defaults, so an uncertain edit recorded before this change still matches its retry.

## Consequences

Labels and priorities set with the Missions CLI directly are now visible in Luuna after a reconcile, and the reverse also holds. A label written in Missions with characters Luuna refuses (for example a space) makes the projection refuse to reconcile, the same way a malformed intent envelope already does.

## Evidence

- Domain tests cover the defaults that are left out, the bounds, and parsing.
- A runtime API test covers create, refusals and a triage-only edit.
- A real Missions test covers delivery, an edit through `--set-labels`, clearing back to the defaults, and an edit that changes nothing.
- The real export and import round trip now carries the three fields through Missions on both sides.
- The command line parser, the desktop edit request and the typed client are tested too.
