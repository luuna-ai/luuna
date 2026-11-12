//! JSON writing as Go's `encoding/json` does it.
//!
//! Records are written compactly with `json.Marshal`, which escapes `<`, `>`,
//! `&`, U+2028 and U+2029; command output goes through a `json.Encoder` with
//! HTML escaping off and two-space indentation. Unchanged record values keep
//! their raw bytes (serde_json `RawValue`) and their original key order.
//!
//! Ported in task t2.
