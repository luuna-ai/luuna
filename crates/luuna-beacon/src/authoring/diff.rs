//! Section diffs preserve Go's duplicate-section and failed-scan behavior.
use super::AuthoringError as E;
use crate::{gofmt, store, Beacon, BeaconError, Prompt};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChangeKind {
    Added,
    Modified,
    Removed,
}
impl ChangeKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Added => "added",
            Self::Modified => "modified",
            Self::Removed => "removed",
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SectionChange {
    pub section: String,
    pub kind: ChangeKind,
}
impl Beacon {
    pub fn diff(
        &self,
        name: &str,
        from: &str,
        to: &str,
    ) -> Result<Vec<SectionChange>, BeaconError> {
        let records: Vec<Prompt> = store::read_jsonl(&self.prompts_path()?)?;
        let (mut first, mut second) = (None, None);
        for record in &records {
            if record.name != name {
                continue;
            }
            let mut version = 0;
            if let Some(v) = gofmt::sscan_int(from) {
                version = v
            }
            if record.version == version {
                first = Some(record)
            }
            if let Some(v) = gofmt::sscan_int(to) {
                version = v
            }
            if record.version == version {
                second = Some(record)
            }
        }
        let (first, second) = (
            first.ok_or(E::PromptVersionsNotFound)?,
            second.ok_or(E::PromptVersionsNotFound)?,
        );
        let mut old: Vec<(&str, &str)> = Vec::new();
        for section in first.sections.as_deref().unwrap_or(&[]) {
            if let Some(existing) = old.iter_mut().find(|(name, _)| *name == section.name) {
                existing.1 = section.body.as_str()
            } else {
                old.push((section.name.as_str(), section.body.as_str()))
            }
        }
        let mut changes = Vec::new();
        for section in second.sections.as_deref().unwrap_or(&[]) {
            let kind = match old
                .iter()
                .position(|(name, _)| *name == section.name)
                .map(|at| old.remove(at).1)
            {
                None => Some(ChangeKind::Added),
                Some(body) if body != section.body => Some(ChangeKind::Modified),
                _ => None,
            };
            if let Some(kind) = kind {
                changes.push(SectionChange {
                    section: section.name.clone(),
                    kind,
                })
            }
        }
        for (name, _) in old {
            changes.push(SectionChange {
                section: name.into(),
                kind: ChangeKind::Removed,
            })
        }
        Ok(changes)
    }
}
