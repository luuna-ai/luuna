# Memory and context

## Three uses

| Use | Purpose | Default scope |
| --- | --- | --- |
| User memory | Preferences, goals, working context, and personal defaults | Private to the developer, across projects |
| Project knowledge | Decisions, conventions, interface relationships, verified pitfalls | One project or repository |
| Process lessons | Reusable investigation, implementation, and validation methods | Project-local until explicitly promoted |

These are the proposed Luuna-ML boundaries. Upstream Mulch provides structured expertise storage; private personal memory and Luuna-specific scope resolution are product extensions, not assumed upstream behavior. See S06.

## Personal memory

Examples include preferred progress verbosity, default model profiles, and usual review preferences. Store personal memory privately in application data. Do not commit it to shared repositories. Project policy can override a personal default when the developer has approved that policy.

Do not infer a durable personal fact from a temporary failure or an agent's interpretation. Explicit user edits can create or correct user memory. Provide inspect, edit, export, delete, and disable controls through both clients.

A developer can move an export between computers. Automatic sync is deferred. Running the same product on two computers does not imply shared memory or shared active runs.

## Project knowledge and process lessons

Project knowledge includes architecture rationale, conventions, verified failure/resolution pairs, and references to important code. Attach evidence, scope, and freshness markers. A record can be superseded without erasing its historical evidence.

A project with no memory yet is offered a starter set ([ADR 0059](decisions/0059-starter-memory.md)). The runtime proposes a few facts from the confirmed manifest and from which well-known files exist in each checkout; it never reads their text and runs no agent. The developer keeps or skips each fact one at a time. A kept fact is recorded as an ordinary project memory with its provenance.

Process lessons describe how to investigate a class of changes. A lesson can reference a project-specific example without becoming a universal rule. Promotion removes confidential implementation details and requires an explicit developer action.

Mulch's typed records and scoped retrieval are useful foundations. Luuna adds host/project boundaries, provenance checks, and context budgets. Canopy remains the source for executable process instructions; a learned suggestion does not automatically edit approved agent prompts.

## Context construction

For each assignment, construct a context manifest listing user-memory IDs, project-memory IDs, process prompt versions, ticket revision, plan hash, allowed repositories, file references, and token estimate. Scope retrieval by task, affected paths, and role.

Order the material so runtime policy and approved scope remain explicit. Current repository instructions and execution evidence determine current implementation behavior. Stale memory is a lead to investigate, not proof.

Start with lexical code search, manifests, symbols where available, and file references. Embeddings are optional derived indexes and can be rebuilt. They are not the source of truth for plans, tickets, code, or personal memory.

## Writeback

Before writeback, classify an insight as user memory, project knowledge, process lesson, or run-only detail. Save run-only details with the task. Preserve reusable lessons only when evidence supports them. Avoid storing entire chat transcripts as expertise records.

The runtime records provenance and validates a proposed memory mutation. Project/repository records use their upstream adapter; private user records use the private store. Memory changes do not bypass Git review policies.

## Boundaries

Never place credentials, environment dumps, raw access tokens, or unrelated project content into model context. Context assembled for one client project excludes other projects by default. Redaction must happen before event streaming, artifact publication, or retrieval indexing.

Provide record-level deletion and index invalidation. Retention policy distinguishes raw transcripts, execution logs, durable expertise, and user memory. Deleting memory removes derived retrieval entries and identifies any retained immutable audit references.
