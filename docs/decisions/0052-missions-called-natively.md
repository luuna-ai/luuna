# ADR 0052: Call Missions natively

Status: accepted for the Missions slice of ADR 0044.

## Decision

Runtime ticket operations call the complete Missions Rust library through luuna-components. No Missions executable, Go toolchain, Bun runtime or source checkout is discovered or launched. The shared compatibility facade retains installation identity validation before native dispatch. Atlas remains on the legacy adapter until its own wiring change; full activation still requires Atlas.

Managed runtime calls accept init/create/show/list/search/update/block, the operations used by ticket projection and reconciliation. They preserve authoritative store resolution, layout adoption, directory link checks, durable operation ledgers and revision conflicts. An explicit store directory prevents discovery of a developer store through inherited environment. The managed command environment is empty. Integration errors remain redacted.

Before execution, managed input is bounded to 64 MiB across config, tickets, templates, plans and ignore files, plus root attributes during initialization. Files must be regular and not links. JSONL has at most 100,000 records total, each at most 4 MiB and within the bounded JSON parser's depth. Arguments retain 128 entries, 256 KiB per argument and 4 MiB combined. Responses are at most 4 MiB and never truncated. A response refusal after a completed write does not assert rollback; durable operation markers reconcile uncertain completion.

The terminal component runner exposes Missions independently of any legacy installation and passes the complete native command contract through an explicit environment with runtime connection credentials removed. It preserves stdout, stderr and exit status, including clear library data-loss refusals. Terminal output is bounded at 4 MiB; the raw library keeps its Go-compatible input contract. Additional top-level command names and regrouping belong to the regrouping plan.

Native work runs on an awaited blocking job. A job-owned gate survives cancellation of its caller; subsequent native calls wait for the actual writer. There is no timeout that falsely claims a native writer has stopped. This executes with host permissions and is not a sandbox.

## Compatibility

The library remains authoritative for Go-compatible records, flags, file locking and A8 refusals. The runtime boundary deliberately rejects oversized, deeply structured or nonregular managed input before mutation. These API limits do not alter raw library compatibility. Optional Atlas transport in raw plan commands remains until Atlas wiring.

Legacy manifests, submodules and packaging remain for Atlas. This change grants no fallback from Missions to the old adapter and no authority to merge a developer's repository.
