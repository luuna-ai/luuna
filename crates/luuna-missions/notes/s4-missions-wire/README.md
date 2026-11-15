# Missions wiring

The runtime and terminal component runner now use this crate. ADR 0052 describes the bounded managed API and full terminal command contract. Earlier foundation README statements that nothing uses the library are historical.

The runtime retains authoritative store resolution, approval invalidation, source fingerprints and durable delivery reconciliation. Native failures never fall back to the old component CLI. The terminal retains native output and clear A8 errors; runtime connection credentials are excluded from the command environment.

Sync checks relative PATH entries against the invocation working directory, matching Go's refusal when a local executable would otherwise be selected. It does not change the process working directory.
