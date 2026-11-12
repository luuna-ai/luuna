# Notes of plan s3-beacon-lib-2

## Ownership

Files this plan owns (paths under `crates/luuna-beacon/`):

- `src/authoring/**`, including its own alias guard and depth limit
- `tests/support/authoring.rs`
- `tests/parity_authoring.rs` and any `tests/authoring_*.rs`
- `tests/fixtures/s3-beacon-lib-2/`
- `notes/s3-beacon-lib-2.md`

- Foundation files it may still edit: none.
- Plans it needs on `main`: none.

## Numbering

- Differences from the Go tool: RD3 to RD49.
- Suspected Go defects: GD7 to GD49.

## Differences from the Go tool

None recorded yet.

## Refused where the Go tool loses data (A8)

Where the Go tool silently loses the user's data, the port refuses the input with a clear error and leaves the file untouched; each such case is listed here.

None recorded yet.

## Suspected Go defects

None recorded yet.

## Test files

- `tests/parity_authoring.rs`

```
LUUNA_BEACON_REFERENCE=<clone> LUUNA_BEACON_GO=<go> cargo test -p luuna-beacon --locked --test parity_authoring -- --ignored
```
