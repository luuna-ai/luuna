//! Native stdin and batch recording.
use super::record::RecordOptions;
use crate::{options::Context, CommandOutput};
use std::path::PathBuf;
pub enum BulkInput {
    Stdin(Vec<u8>),
    Batch(PathBuf),
}
pub fn run_record_bulk(
    _ctx: &Context,
    _domain: &str,
    _input: BulkInput,
    _options: &RecordOptions,
) -> CommandOutput {
    CommandOutput::failure("record", "not implemented: bulk")
}
