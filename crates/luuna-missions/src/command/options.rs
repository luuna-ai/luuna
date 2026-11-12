//! The hand-written, Commander-compatible option parser
//! (`internal/cli/options.go`), with its quirks copied: an unknown option is
//! never printed as JSON (SD6), and a space-separated value beginning with
//! `--` is reported as missing (SD7).
//!
//! Ported in task t3.
