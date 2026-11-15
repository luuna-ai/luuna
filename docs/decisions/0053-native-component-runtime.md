# ADR 0053: Native Atlas and terminal component dispatch

Status: accepted, November 15, 2026. Completes the native adapter portion of charter step 5; follows ADRs 0051 and 0052.

## Decision

Runtime component discovery selects compiled Atlas, Beacon and Missions libraries. It does not inspect a source checkout, resolve Bun or execute a component bridge. Atlas runtime operations use explicit private store roots, finite command admission, serialized blocking jobs and the existing runtime policy and intent ledgers. Configured executable Atlas hooks are refused for runtime operations.

Terminal component commands use native frontends with explicit cwd, environment, stdin and independent stdout, stderr and exit status. `luuna atlas`, `luuna beacon` and `luuna missions` dispatch before loading private runtime settings. Historical executable names emitted by component hooks and completions are recognized when this executable is installed under those names. Packaging those names remains release work.

Managed Atlas calls opt into thread-local YAML expansion/text budgets that restore the previous policy on completion or unwind. Raw library comparisons retain their Go behavior. The private raw IPC mode is refused by managed terminal admission because it accepts independent file paths. Managed terminal hooks, recipes and Git commands use finite child capture and clean up owned process groups before returning on Unix. Other platforms refuse managed child execution until equivalent ownership is implemented; native operations without child commands remain available. These limits are resource controls, not an operating-system sandbox. Runtime connection and provider environment bindings are removed from component terminal child environments.

Historical source and portable distribution integrity checks remain available through `DistributionAudit`, separate from native availability. Existing pins, manifest digests and integrity assertions remain intact. `components:setup`, `components:verify` and `components:run` default to the native terminal build; explicit `audit-legacy` and `setup-legacy` retain historical distribution tooling. First-run setup prepares Pi and runtime settings without installing a component toolchain.

## Validation and limits

The wiring plan requires workspace fmt, Clippy, tests, hygiene/frontend checks, all Atlas reference comparisons and Beacon terminal comparisons, plus bounded hostile-input and child-cleanup checks. Actual results belong in the plan evidence. Native macOS and Windows execution, release archives, installation aliases and consolidated CI remain separate obligations. Compatibility version metadata may still describe the historical tool; backend availability is reported independently.
