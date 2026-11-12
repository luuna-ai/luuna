//! Shared helpers for the tests that compare this crate with the Go tool:
//! building and running the Go binary at the recorded commit (`go`) and the
//! normalisation every comparison applies (`normalise`).
#![allow(dead_code)]
pub mod case;
pub mod go;
pub mod normalise;
pub mod payload;
