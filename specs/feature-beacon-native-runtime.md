# Native Beacon runtime integration

Connect the existing native Beacon library to every runtime Beacon call and to the shared component facade. No Beacon request may fall back to the Go/Bun CLI. Preserve prompt creation/update reconciliation, version and schema/source hashes, approval invalidation, managed workspace link checks and the bounded JSON response contract.

Use a standalone native entrypoint independent of toolchain discovery; the legacy facade retains installation identity checks before selecting that native entrypoint. Missions and Atlas keep their legacy paths until their own plans. Existing manifest/submodule assets remain until the final adapter removal.

The native dispatcher accepts only the operations consumed by current runtime/integration contracts. It runs filesystem work on an awaited blocking task, rejects unsupported arguments and oversized/deep/nonregular inputs before mutation, caps combined input at 64 MiB, records at 100,000, individual records at 4 MiB, and output at 4 MiB and preserves Go JSON/runtime payload semantics. A timed-out future must never leave a native writer detached. Prompt-only requests work for enabled stores without Bun; memory requests retain Atlas availability checks.

Implement the component adapter and focused boundary tests, wire runtime call/availability handling, add real authenticated API and context-staleness coverage without legacy components, update architecture/ADR, then run every required workspace/frontend and Beacon comparison/hostile check. Run the unchanged existing real component integration suites serially with pinned prerequisites. Self-review, open a plan PR, squash merge, and repeat all required checks on clean main.

No frozen test, reference clone, publication guard or owner data is modified. Native macOS proof remains in the later CI slice.
