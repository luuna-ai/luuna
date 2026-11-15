//! Event parser/scanner adapted from yaml-rust2 0.10.4 (MIT OR Apache-2.0).
//!
//! Local changes: module paths; standard VecDeque replaces the fixed lookahead
//! buffer; flow counter widened from u8 to u32; no invented EOF newline in
//! a block scalar; Unicode line-break folding; closing-quote indentation.
//! The authoring depth guard applies
//! yaml.v3's separate 10,000-level block and flow limits. Upstream debug output
//! and loader-oriented tests are excluded. See adjacent license files.
#![allow(dead_code)]
mod char_traits;
pub(super) mod parser;
pub(super) mod scanner;
