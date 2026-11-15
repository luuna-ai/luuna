//! Counts current records, including archived prompts and repeated tags.
use crate::{prompts, store, Beacon, BeaconError, Prompt};
use std::collections::BTreeMap;
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptStats {
    pub total: usize,
    pub by_status: BTreeMap<String, usize>,
    pub by_tag: BTreeMap<String, usize>,
}
impl Beacon {
    pub fn stats(&self) -> Result<PromptStats, BeaconError> {
        let records: Vec<Prompt> = store::read_jsonl(&self.prompts_path()?)?;
        let records = prompts::current(records);
        let mut result = PromptStats {
            total: records.len(),
            by_status: BTreeMap::new(),
            by_tag: BTreeMap::new(),
        };
        for prompt in records {
            *result.by_status.entry(prompt.status).or_default() += 1;
            for tag in prompt.tags {
                *result.by_tag.entry(tag).or_default() += 1
            }
        }
        Ok(result)
    }
}
