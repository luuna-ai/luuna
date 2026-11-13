# Native plan lifecycle

This plan completes native `plan templates`, `prompt`, `submit`, `show`, `list`, `validate`, `edit`, `create`, `adopt`, `reorder`, `release`, `review` and `outcome`. Ticket show/list/search gain Go plan context; status updates and close run native lifecycle hooks. The foundational crate README describes earlier plan states; these notes supersede its deferred plan limitations. Runtime/CLI integration is the separate Missions wiring plan.

`Invocation` supplies cwd, stdin, environment and optional explicit store. No Missions command invokes Go or Bun. Optional Atlas queries and decision recording retain Go's external executable contract until native Atlas wiring. They use only the supplied environment; recording runs at the project root after local persistence and remains best effort.

Plan records preserve raw unchanged known fields, unknown fields and original key order. Explicit blueprint submission replaces sections. Section edits protect retained data before re-encoding. Plans lock before issues. Preflight inspects both prospective files before writes; existing Go I/O failure ordering remains. Edit writes the plan before locking issues, as Go does: it preflights first, then rechecks fresh issue data under the lock. If unsafe concurrent data appears there, its error explicitly says the plan was saved and child propagation was refused, leaving that issue file untouched.

Lifecycle transitions consider affected child ids, not the seed or its plan_id. Draft plans stay draft. Existing children all closed yield done; active/done or any in-progress child yield active; otherwise approved. Missing children do not prevent done when at least one existing child remains. A no-op transition does not rewrite plans, including unreadable unrelated lines.

Tree expansion and rendering use iterative arenas. More than 100000 graph nodes or 128 MiB buffered native plan output returns explicit Unsupported; JSON keeps Go's depth checks. Cycles never recurse on the call stack. Go panic shapes in list and custom-schema submission are explicit refusals, detailed in UPSTREAM.md.

## Validation and ownership

The 45 allocated fixtures stay frozen in 24 command groups. Owned unit tests cover raw records, loss guards, validation, optional Atlas, rendering, mutations, concurrency and transitions. The final gate runs all workspace checks, 60 Missions Go comparisons, five format comparisons, the 24 plan groups, and capped hostile plan tests. Native macOS execution belongs to the later CI work; it is not inferred from Linux tests.

Only the read/update and one close stub expectation delegate into owned checks; their names and inputs remain. Literal pinned-Go output replaces those retired refusals. All other shared tests and harness files stay unchanged. The read index additionally shares full Plan acceptance so known unmarshal/baseline failures are skipped exactly as Go, while unknown root values stay raw. Onboard and CLI version equality is checked through public dispatch.

See UPSTREAM.md for source hashes, license provenance, copied defects and A8 differences.
