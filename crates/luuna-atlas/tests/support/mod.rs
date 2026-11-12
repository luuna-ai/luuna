//! The comparison harness shared by the parity, format and harness tests: locating and
//! building the Go reference tool (`reference`), normalising output (`normalize`), and
//! temporary twin projects (`project`). Each test binary uses part of it.
#![allow(dead_code)]

pub mod normalize;
pub mod project;
pub mod reference;
