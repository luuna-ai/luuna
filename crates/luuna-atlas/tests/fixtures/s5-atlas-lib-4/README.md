# Prime/Git/hook fixtures

The owned parity_prime_git test runner builds isolated twin projects, supplies its
own executable gh stub and never changes the process environment or shared support.
Its source-generated fixtures cover Git commits/worktrees/failure stubs, manifests,
contracts, tracker branches, hook guards, sessions and raw array records.

Hostile fixture generation is streamed to target with separately assigned TMPDIR.
Compile both Rust tests and the pinned reference first, then run hostile groups
with prlimit 4 GiB and timeout 120. Do not assign checkout-backed TMPDIR globally to
ordinary workspace tests, whose repository-isolation checks need outside roots.
