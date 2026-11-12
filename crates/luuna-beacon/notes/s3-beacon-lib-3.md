# Notes of plan s3-beacon-lib-3

## Ownership

Files this plan owns (paths under `crates/luuna-beacon/`):

- `src/artifacts/**`, including its Artifact model, base64 codec and `sync.rs`
- `tests/support/artifacts.rs`
- `tests/parity_artifacts.rs` and any `tests/artifacts_*.rs`
- `tests/fixtures/s3-beacon-lib-3/`
- `notes/s3-beacon-lib-3.md`

- Foundation files it may still edit: none.
- Plans it needs on `main`: none.

## Numbering

- Differences from the Go tool: RD50 to RD99.
- Suspected Go defects: GD50 to GD99.

## Differences from the Go tool

None recorded yet.

## Refused where the Go tool loses data (A8)

Where the Go tool silently loses the user's data, the port refuses the input with a clear error and leaves the file untouched; each such case is listed here.

None recorded yet.

## Suspected Go defects

None recorded yet.

## Test files

- `tests/parity_artifacts.rs`

```
LUUNA_BEACON_REFERENCE=<clone> LUUNA_BEACON_GO=<go> cargo test -p luuna-beacon --locked --test parity_artifacts -- --ignored
```
