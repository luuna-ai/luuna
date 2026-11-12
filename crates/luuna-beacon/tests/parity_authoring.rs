//! Owner: plan s3-beacon-lib-2. Reference tests of the authoring commands against the Go tool.
//!
//! Each Go-dependent test of the plan carries the ignore string
//! `#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with LUUNA_BEACON_REFERENCE=<clone> LUUNA_BEACON_GO=<go> cargo test -p luuna-beacon --locked --test parity_authoring -- --ignored"]`.

mod support;
