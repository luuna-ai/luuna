# Bug: dropping valid Beacon frontmatter exhausts the stack

## Metadata
- Type: bug
- Source: s3-beacon-lib-3 open escalation and owner consolidation request
- Date: 2026-11-13

## Bug Description
The artifacts candidate aborts on its required 9,999-level frontmatter doctor test. A new probe on current main completes parsing and encoding but aborts when the returned prompt is dropped. serde_json::Value recursively owns its child containers.

## Steps to Reproduce
1. Build `cargo test -p luuna-beacon --locked --test depth_safety --no-run`.
2. Run `prlimit --as=4294967296 timeout 120 cargo test -p luuna-beacon --locked --test depth_safety -- --ignored --nocapture`.
- Expected: valid input is preserved and prompts are safely released on the default stack.
- Actual: SIGABRT while dropping the returned prompts.

## Root Cause Analysis
`model.rs::Prompt::frontmatter` directly owns recursive serde_json values. Its derived Clone and PartialEq also recurse. The JSON parser/writer already walk iteratively, so changing them to accept shallower input would hide the ownership failure and break Go parity.

## Solution Statement
Add a small JsonValue owner with iterative drop, clone and equality. Keep serde_json access through borrowing and dereferencing. Prompt frontmatter maps own these values; decoder construction and duplicate replacement use the same owner for cleanup. Stored bytes, command output, the Go nesting limit, and existing test assertions remain unchanged. This unpublished Rust crate's value field type changes; converting a serde_json value uses `.into()`.

## Out of Scope
No lower nesting limit, extra stack-size environment setting, ignored failing test, Go behavior change or dependency change. Native runtime wiring remains a later plan.

## Relevant Files
- `crates/luuna-beacon/src/owned_json.rs` (new): stack-safe value ownership.
- `crates/luuna-beacon/src/lib.rs`: export the owner.
- `crates/luuna-beacon/src/model.rs`: owned frontmatter values.
- `crates/luuna-beacon/src/gojson.rs`: safe decoding and generic borrowed map writing.
- `crates/luuna-beacon/src/prompts.rs`: explicit ownership conversions at insert sites.
- `crates/luuna-beacon/src/payload.rs`: fixture construction conversion only if required.
- `crates/luuna-beacon/src/resolve.rs`: three test-fixture inserted-value conversions only.
- `crates/luuna-beacon/tests/depth_safety.rs` (new): streamed hostile regression.
- `crates/luuna-beacon/notes/s3-beacon-depth-repair.md` (new): evidence and API change.

## Step by Step Tasks
1. Preserve the reproduced pre-fix abort logs and the unchanged artifacts hostile test.
2. Implement bounded-stack ownership without changing field contents or numeric representations.
3. Adapt fixture construction in model/payload tests mechanically: only convert inserted JSON values into the owner. Keep every assertion unchanged and commit these fixture conversions separately with the compiler-compatibility reason.
4. Extend the depth regression to clone, compare, encode and drop, including a decode-error cleanup case.
5. Run ordinary checks and Beacon Go comparisons, with hostile runs under the owner's resource cap.
6. Merge this prerequisite on its own branch and PR; recheck main before integrating the artifact candidate.

## Validation Commands

Configure `LUUNA_BEACON_REFERENCE` and `LUUNA_BEACON_GO` to the read-only reference clone and pinned Go executable before the comparison checks.
- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --locked -- -D warnings`
- `cargo test --workspace --locked --no-fail-fast`
- `node scripts/check-hygiene.mjs`
- `npm ci && npm test`
- `cargo test -p luuna-beacon --locked --test go_reference -- --ignored`
- `cargo test -p luuna-beacon --locked --test parity -- --ignored`
- `cargo test -p luuna-beacon --locked --test format -- --ignored`
- `prlimit --as=4294967296 timeout 120 cargo test -p luuna-beacon --locked --test depth_safety -- --ignored --nocapture`

## Notes
This plan repairs the existing foundation rather than relaxing the artifact plan's hostile test. Whole-workspace serde_json feature unification can change stack behavior; test the crate alone as well as the whole workspace. Go clones remain read-only.

Compiler evidence before fixture conversion: five E0308 diagnostics in model/payload/resolve test fixtures require explicit value conversion for the new owner. These changes add `.into()` only at fixture insertion sites; no assertion changes.
