//! Prompt relationships, with a bounded ancestor walk.
use crate::{prompts, store, Beacon, BeaconError, Prompt};
use std::collections::HashSet;
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptTree {
    pub name: String,
    pub ancestors: Vec<String>,
    pub mixins: Option<Vec<String>>,
    pub children: Vec<String>,
    pub mixin_users: Vec<String>,
}
impl Beacon {
    pub fn tree(&self, reference: &str) -> Result<PromptTree, BeaconError> {
        let records: Vec<Prompt> = store::read_jsonl(&self.prompts_path()?)?;
        let records = prompts::current(records);
        let prompt = &records[prompts::find(&records, reference)?];
        let mut ancestors = Vec::new();
        let mut seen = HashSet::new();
        let mut parent = prompt.extends.as_str();
        while !parent.is_empty() {
            if !seen.insert(parent) {
                return Err(BeaconError::CircularInheritance(parent.into()));
            }
            ancestors.push(parent.into());
            let Ok(id) = prompts::find(&records, parent) else {
                break;
            };
            parent = &records[id].extends;
        }
        let children = records
            .iter()
            .filter(|p| p.extends == prompt.name)
            .map(|p| p.name.clone())
            .collect::<Vec<_>>();

        let mixin_users = records
            .iter()
            .filter(|p| p.mixins.contains(&prompt.name))
            .map(|p| p.name.clone())
            .collect::<Vec<_>>();

        Ok(PromptTree {
            name: prompt.name.clone(),
            ancestors,
            mixins: if prompt.mixins.is_empty() {
                None
            } else {
                Some(prompt.mixins.clone())
            },
            children,
            mixin_users,
        })
    }
}
