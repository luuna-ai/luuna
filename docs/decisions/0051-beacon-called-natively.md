# ADR 0051: Call Beacon natively

Status: accepted for the Beacon slice of ADR 0044.

## Decision

Runtime prompt operations call the existing `luuna-beacon` Rust library through `luuna-components::call_beacon_native`. They do not discover Bun, load component source, start a Beacon process or fall back to the legacy CLI. The shared `Components::call` facade also dispatches Beacon operations to this native implementation after retaining its existing installation identity checks. Its compatibility identity probe may still query Bun's version; runtime prompt calls bypass that facade.

The native adapter accepts the finite command shapes consumed by the current runtime: init/list/create/update/render/validate and schema create/list/show/rule-add. Unsupported commands or flags fail closed. The complete Beacon library remains the authority for record/version/schema semantics; the later command regrouping exposes additional authoring operations. Store selection stays with the existing runtime store resolver.

The native entrypoint preserves managed workspace, legacy layout and link checks. Before library execution it refuses nonregular store files, more than 64 MiB combined config/prompt/schema input (plus `.gitattributes` during initialization), more than 100,000 combined prompt/schema records, an individual JSONL record over 4 MiB, and JSON records deeper than the bounded API parser accepts. Arguments retain the existing 128-count/256 KiB-per-argument limits, with a 4 MiB combined argument budget. Responses remain bounded at 4 MiB; no input/output is truncated. These limits apply to managed runtime integration, not the raw library's Go compatibility contract. Record JSON is encoded using Go conventions and parsed through the bounded API representation before client values are constructed; arbitrary deep frontmatter is never recursively cloned into a response.

Filesystem work runs on an awaited blocking task. A native writer cannot be killed by dropping a future, so there is no fake timeout that reports it stopped while it is still writing. A native gate remains owned by the blocking job until it completes, including after a client disconnect; durable attempted-operation markers reconcile uncertain writes on retry. This is host execution, not a sandbox. Existing source/version/schema hashes, resource-selection checks and idempotency ledgers remain authoritative.

An enabled prompt store and prompt-only context selection/revalidation work without the remaining legacy toolchain. Fresh complete activation still initializes Missions and Atlas and therefore needs their adapters until their own ports land. Atlas memory requests retain their availability check. Installation manifests, submodules, packaging and existing identity tests remain until final adapter removal; none grants a fallback for a runtime Beacon operation.

## Verification

Native component tests use an isolated child with an empty PATH and no component checkout. They exercise literal long arguments, create/list/update/render, source chains, schema checks and a named pipe with a hard parent deadline. Additional tests cover unsupported inputs, links, deep records, oversized store files and response limits without modifying source data.

Authenticated in-process HTTP tests cover prompt idempotency, version conflicts, project separation, schema/source metadata staleness, refusal redaction and explicit activation, with no legacy component handle. Existing real component suites retain their assertions and continue to exercise the compatibility facade when pinned prerequisites are installed. Linux execution and native macOS CI evidence are reported separately.
