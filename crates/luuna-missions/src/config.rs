//! Reader for the `config.yaml` subset the Go tool writes.
//!
//! Go reads `config.yaml` with yaml.v3 and writes it with its own minimal
//! emitter (`internal/config/tree.go:57-129`). This plan reads only the forms
//! that emitter produces (`project`, `version`, `max_plan_depth`), with a
//! hand-written reader; anything else is rejected with an explicit error
//! until the full `config` command is ported in s4-missions-lib-3.
//!
//! Ported in task t3.
