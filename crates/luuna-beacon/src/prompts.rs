//! Prompt operations: `create`, `update`, `archive`, `list`, `show` and
//! `history` (`cli.go:196-453`, `586-619`) and the lookups they share,
//! `current` and `find` (`cli.go:2033-2077`).
//!
//! Frontmatter values can be nested 10000 levels deep, and `serde_json::Value`
//! overflows a small stack when cloned or compared at that depth. Nothing
//! here clones or compares a frontmatter value: records are read once and
//! moved into the result.

use crate::beacon::Beacon;
use crate::error::BeaconError;
use crate::model::{Prompt, Section};
use crate::{config, gofmt, store};
use serde_json::Value;
use std::collections::HashMap;

/// The input of [`Beacon::create`]. Mirrors the flags of `create`
/// (`cli.go:196-251`).
#[derive(Debug, Clone, Default)]
pub struct CreatePrompt {
    pub name: String,
    pub description: String,
    pub extends: String,
    pub mixins: Vec<String>,
    pub tags: Vec<String>,
    pub schema: String,
    pub emit_as: String,
    pub emit_dir: String,
    /// Empty means `active`.
    pub status: String,
    pub sections: Vec<Section>,
    pub frontmatter: Vec<(String, String)>,
}

/// The input of [`Beacon::update`]. Fields are applied in the order of
/// `cli.go:269-364`. `Some("")` for `name` or `status` means unchanged, as in
/// Go.
#[derive(Debug, Clone, Default)]
pub struct UpdatePrompt {
    pub name: Option<String>,
    pub description: Option<String>,
    pub status: Option<String>,
    pub extends: Option<String>,
    pub schema: Option<String>,
    pub emit_as: Option<String>,
    pub emit_dir: Option<String>,
    pub add_tags: Vec<String>,
    pub remove_tags: Vec<String>,
    pub sections: Vec<Section>,
    pub remove_sections: Vec<String>,
    pub add_mixins: Vec<String>,
    pub remove_mixins: Vec<String>,
    pub frontmatter: Vec<(String, String)>,
    pub remove_frontmatter: Vec<String>,
}

/// The filter of [`Beacon::list`]. `None` and `Some("")` both mean no filter,
/// as an empty flag value does in Go.
#[derive(Debug, Clone, Default)]
pub struct ListFilter {
    pub status: Option<String>,
    pub tag: Option<String>,
    pub extends: Option<String>,
    pub mixin: Option<String>,
}

fn set(filter: &Option<String>) -> Option<&str> {
    filter.as_deref().filter(|text| !text.is_empty())
}

fn contains(values: &[String], wanted: &str) -> bool {
    values.iter().any(|value| value == wanted)
}

fn without(values: &mut Vec<String>, unwanted: &str) {
    values.retain(|value| value != unwanted);
}

/// Go's `current`: the highest version of each id (the first in file order on
/// a tie), then, per name, the highest version across ids, a tie going to the
/// greater `updatedAt` compared as plain text (GD2). Records keep the order in
/// which their names first appear. Go builds the result from map iteration,
/// so its order is unspecified; a full tie on name, version and `updatedAt`
/// keeps the earlier record.
pub(crate) fn current(records: Vec<Prompt>) -> Vec<Prompt> {
    let mut by_id: Vec<Prompt> = Vec::new();
    let mut id_slot: HashMap<String, usize> = HashMap::new();
    for prompt in records {
        match id_slot.get(&prompt.id) {
            Some(&slot) => {
                if prompt.version > by_id[slot].version {
                    by_id[slot] = prompt;
                }
            }
            None => {
                id_slot.insert(prompt.id.clone(), by_id.len());
                by_id.push(prompt);
            }
        }
    }
    let mut by_name: Vec<Prompt> = Vec::new();
    let mut name_slot: HashMap<String, usize> = HashMap::new();
    for prompt in by_id {
        match name_slot.get(&prompt.name) {
            Some(&slot) => {
                let existing = &by_name[slot];
                if prompt.version > existing.version
                    || (prompt.version == existing.version
                        && prompt.updated_at > existing.updated_at)
                {
                    by_name[slot] = prompt;
                }
            }
            None => {
                name_slot.insert(prompt.name.clone(), by_name.len());
                by_name.push(prompt);
            }
        }
    }
    by_name
}

/// Go's `find`, returning the position of the selected record. A reference
/// is `name` or `name@N`; the cut is at the first `@`. With a version the
/// record must have exactly that version (the number is read as `Sscanf("%d")`
/// reads it). Among candidates the highest version wins, a tie going to the
/// greater `updatedAt` as text, and a full tie to the earlier record.
pub(crate) fn find(records: &[Prompt], reference: &str) -> Result<usize, BeaconError> {
    let (name, raw_version) = match reference.split_once('@') {
        Some((name, version)) => (name, Some(version)),
        None => (reference, None),
    };
    let wanted = raw_version.map(gofmt::sscan_int);
    let mut selected: Option<usize> = None;
    for (index, prompt) in records.iter().enumerate() {
        if prompt.name != name {
            continue;
        }
        if let Some(wanted) = wanted {
            if wanted != Some(prompt.version) {
                continue;
            }
        }
        let better = match selected {
            None => true,
            Some(best) => {
                let best = &records[best];
                prompt.version > best.version
                    || (prompt.version == best.version && prompt.updated_at > best.updated_at)
            }
        };
        if better {
            selected = Some(index);
        }
    }
    selected.ok_or_else(|| BeaconError::PromptNotFound(reference.to_owned()))
}

/// Keeps non-empty section names, as `parseNamedSections` does.
fn named(sections: &[Section]) -> impl Iterator<Item = &Section> {
    sections.iter().filter(|section| !section.name.is_empty())
}

impl Beacon {
    /// Go's `create`.
    pub fn create(&self, input: &CreatePrompt) -> Result<Prompt, BeaconError> {
        if input.name.is_empty() {
            return Err(BeaconError::NameRequired);
        }
        let path = self.prompts_path()?;
        let records: Vec<Prompt> = store::read_jsonl(&path)?;
        let active = current(records);
        if active
            .iter()
            .any(|prompt| prompt.name == input.name && prompt.status != "archived")
        {
            return Err(BeaconError::PromptNameExists(input.name.clone()));
        }
        if !input.extends.is_empty() && find(&active, &input.extends).is_err() {
            return Err(BeaconError::ParentNotFound(input.extends.clone()));
        }
        for mixin in &input.mixins {
            if find(&active, mixin).is_err() {
                return Err(BeaconError::MixinNotFound(mixin.clone()));
            }
        }
        let config = config::load(self.project_root())?;
        let sections: Vec<Section> = named(&input.sections).cloned().collect();
        let mut frontmatter = std::collections::BTreeMap::new();
        for (key, value) in &input.frontmatter {
            if !key.is_empty() {
                frontmatter.insert(key.clone(), Value::String(value.clone()));
            }
        }
        let now = self.now_text();
        let status = if input.status.is_empty() {
            "active".to_owned()
        } else {
            input.status.clone()
        };
        let prompt = Prompt {
            id: format!("{}-{}", config.project, self.next_id_hex()),
            name: input.name.clone(),
            description: input.description.clone(),
            version: 1,
            sections: Some(sections),
            extends: input.extends.clone(),
            mixins: input.mixins.clone(),
            tags: input.tags.clone(),
            schema: input.schema.clone(),
            emit_as: input.emit_as.clone(),
            emit_dir: input.emit_dir.clone(),
            frontmatter,
            status,
            created_at: now.clone(),
            updated_at: now,
            ..Prompt::default()
        };
        if prompt.status != "active" && prompt.status != "draft" {
            return Err(BeaconError::InvalidStatus(prompt.status));
        }
        store::append_jsonl(&path, &prompt)?;
        Ok(prompt)
    }

    /// Go's `update`: appends the next version of the current record that
    /// `reference` selects (current records only, so `name@N` works only for
    /// the current version). `createdAt` is carried over.
    pub fn update(&self, reference: &str, input: &UpdatePrompt) -> Result<Prompt, BeaconError> {
        let path = self.prompts_path()?;
        let records: Vec<Prompt> = store::read_jsonl(&path)?;
        let mut active = current(records);
        let index = find(&active, reference)?;
        let mut next = active.swap_remove(index);
        drop(active);
        next.version = next.version.wrapping_add(1);
        next.updated_at = self.now_text();
        if let Some(name) = input.name.as_deref().filter(|name| !name.is_empty()) {
            next.name = name.to_owned();
        }
        if let Some(description) = &input.description {
            next.description.clone_from(description);
        }
        if let Some(status) = input.status.as_deref().filter(|status| !status.is_empty()) {
            if status != "active" && status != "draft" && status != "archived" {
                return Err(BeaconError::InvalidStatus(status.to_owned()));
            }
            next.status = status.to_owned();
        }
        if let Some(extends) = &input.extends {
            next.extends.clone_from(extends);
        }
        if let Some(schema) = &input.schema {
            next.schema.clone_from(schema);
        }
        if let Some(emit_as) = &input.emit_as {
            next.emit_as.clone_from(emit_as);
        }
        if let Some(emit_dir) = &input.emit_dir {
            next.emit_dir.clone_from(emit_dir);
        }
        for tag in &input.add_tags {
            if !contains(&next.tags, tag) {
                next.tags.push(tag.clone());
            }
        }
        for tag in &input.remove_tags {
            without(&mut next.tags, tag);
        }
        for section in named(&input.sections) {
            let mut replaced = false;
            if let Some(existing) = next.sections.as_mut() {
                for target in existing.iter_mut().filter(|t| t.name == section.name) {
                    target.body.clone_from(&section.body);
                    replaced = true;
                }
            }
            if !replaced {
                next.sections
                    .get_or_insert_with(Vec::new)
                    .push(section.clone());
            }
        }
        for name in &input.remove_sections {
            if next.extends.is_empty() {
                let kept: Vec<Section> = next
                    .sections
                    .take()
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|section| &section.name != name)
                    .collect();
                next.sections = Some(kept);
            } else {
                let mut found = false;
                if let Some(existing) = next.sections.as_mut() {
                    for target in existing.iter_mut().filter(|t| &t.name == name) {
                        target.body.clear();
                        found = true;
                    }
                }
                if !found {
                    next.sections.get_or_insert_with(Vec::new).push(Section {
                        name: name.clone(),
                        body: String::new(),
                        required: None,
                    });
                }
            }
        }
        for mixin in &input.add_mixins {
            if !contains(&next.mixins, mixin) {
                next.mixins.push(mixin.clone());
            }
        }
        for mixin in &input.remove_mixins {
            without(&mut next.mixins, mixin);
        }
        for (key, value) in &input.frontmatter {
            if !key.is_empty() {
                next.frontmatter
                    .insert(key.clone(), Value::String(value.clone()));
            }
        }
        for key in &input.remove_frontmatter {
            next.frontmatter.remove(key);
        }
        store::append_jsonl(&path, &next)?;
        Ok(next)
    }

    /// Go's `archive`: appends the next version with status `archived`.
    pub fn archive(&self, reference: &str) -> Result<Prompt, BeaconError> {
        let path = self.prompts_path()?;
        let records: Vec<Prompt> = store::read_jsonl(&path)?;
        let mut active = current(records);
        let index = find(&active, reference)?;
        let mut prompt = active.swap_remove(index);
        drop(active);
        if prompt.status == "archived" {
            return Err(BeaconError::AlreadyArchived(reference.to_owned()));
        }
        prompt.version = prompt.version.wrapping_add(1);
        prompt.status = "archived".to_owned();
        prompt.updated_at = self.now_text();
        store::append_jsonl(&path, &prompt)?;
        Ok(prompt)
    }

    /// Go's `list`: current records, archived ones only when asked for by
    /// status, sorted by name (Go's order is unspecified; AS6).
    pub fn list(&self, filter: &ListFilter) -> Result<Vec<Prompt>, BeaconError> {
        let path = self.prompts_path()?;
        let records: Vec<Prompt> = store::read_jsonl(&path)?;
        let status = set(&filter.status);
        let mut result: Vec<Prompt> = current(records)
            .into_iter()
            .filter(|prompt| match status {
                None => prompt.status != "archived",
                Some(status) => prompt.status == status,
            })
            .filter(|prompt| set(&filter.tag).is_none_or(|tag| contains(&prompt.tags, tag)))
            .filter(|prompt| set(&filter.extends).is_none_or(|parent| prompt.extends == parent))
            .filter(|prompt| set(&filter.mixin).is_none_or(|mixin| contains(&prompt.mixins, mixin)))
            .collect();
        result.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(result)
    }

    /// Go's `show`: `name` selects the current record; `name@N` searches
    /// every stored version.
    pub fn show(&self, reference: &str) -> Result<Prompt, BeaconError> {
        let path = self.prompts_path()?;
        let records: Vec<Prompt> = store::read_jsonl(&path)?;
        let mut lookup = if reference.contains('@') {
            records
        } else {
            current(records)
        };
        let index = find(&lookup, reference)?;
        Ok(lookup.swap_remove(index))
    }

    /// Go's `history`: every stored version of the prompt named before any
    /// `@`, newest version first, at most `limit` of them.
    pub fn history(&self, reference: &str, limit: usize) -> Result<Vec<Prompt>, BeaconError> {
        let path = self.prompts_path()?;
        let records: Vec<Prompt> = store::read_jsonl(&path)?;
        let name = reference.split('@').next().unwrap_or_default();
        let mut items: Vec<Prompt> = records
            .into_iter()
            .filter(|prompt| prompt.name == name)
            .collect();
        items.sort_by_key(|prompt| std::cmp::Reverse(prompt.version));
        items.truncate(limit);
        if items.is_empty() {
            return Err(BeaconError::PromptNotFound(reference.to_owned()));
        }
        Ok(items)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Clock;
    use std::sync::Arc;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    fn record(id: &str, name: &str, version: i64, updated_at: &str) -> Prompt {
        Prompt {
            id: id.to_owned(),
            name: name.to_owned(),
            version,
            status: "active".to_owned(),
            updated_at: updated_at.to_owned(),
            ..Prompt::default()
        }
    }

    fn section(name: &str, body: &str) -> Section {
        Section {
            name: name.to_owned(),
            body: body.to_owned(),
            required: None,
        }
    }

    struct Fixed(u64);
    impl Clock for Fixed {
        fn now(&self) -> SystemTime {
            UNIX_EPOCH + Duration::from_secs(self.0)
        }
    }

    fn ready() -> (tempfile::TempDir, Beacon) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let beacon = Beacon::open(&root)
            .unwrap()
            .with_clock(Arc::new(Fixed(1_704_164_645)));
        beacon.init().unwrap();
        (dir, beacon)
    }

    #[test]
    fn current_prefers_the_highest_version_then_the_later_updated_at() {
        let records = vec![
            record("a", "x", 1, "2024-01-01T00:00:00Z"),
            record("a", "x", 3, "2024-01-01T00:00:00Z"),
            record("a", "x", 2, "2024-02-01T00:00:00Z"),
            // Another id with the same name: same version, later text wins.
            record("b", "x", 3, "2024-01-01T00:00:00.5Z"),
            // A tie on id, version and time keeps the first record.
            record("c", "y", 1, "t"),
            record("c", "y", 1, "t"),
        ];
        let mut records = records;
        records[5].description = "second".to_owned();
        let result = current(records);
        assert_eq!(result.len(), 2);
        let x = result.iter().find(|p| p.name == "x").unwrap();
        // GD2: "…00Z" vs "…00.5Z" compares '.' (0x2e) with 'Z' (0x5a), so the
        // fraction-less text is greater although it is the earlier instant.
        assert_eq!((x.id.as_str(), x.version), ("a", 3));
        let y = result.iter().find(|p| p.name == "y").unwrap();
        assert_eq!(y.description, "");
    }

    #[test]
    fn find_honours_name_at_version() {
        let records = vec![
            record("a", "p", 1, "t1"),
            record("a", "p", 2, "t2"),
            record("a", "p", 2, "t3"),
            record("b", "q", 1, "t"),
        ];
        assert_eq!(find(&records, "p").unwrap(), 2);
        assert_eq!(find(&records, "p@1").unwrap(), 0);
        assert_eq!(find(&records, "p@2").unwrap(), 2);
        assert_eq!(find(&records, "p@2xyz").unwrap(), 2);
        assert_eq!(
            find(&records, "p@3").unwrap_err().to_string(),
            r#"prompt "p@3" not found"#
        );
        assert_eq!(
            find(&records, "p@x").unwrap_err().to_string(),
            r#"prompt "p@x" not found"#
        );
        assert!(find(&records, "p@").is_err());
        assert!(find(&records, "missing").is_err());
    }

    #[test]
    fn update_applies_fields_in_go_order() {
        let (_dir, beacon) = ready();
        beacon
            .create(&CreatePrompt {
                name: "base".into(),
                ..Default::default()
            })
            .unwrap();
        let created = beacon
            .create(&CreatePrompt {
                name: "p".into(),
                description: "d".into(),
                tags: vec!["t1".into()],
                sections: vec![section("A", "a"), section("B", "b"), section("", "skipped")],
                frontmatter: vec![("k".into(), "v".into()), (String::new(), "no".into())],
                ..Default::default()
            })
            .unwrap();
        assert_eq!(created.sections.as_ref().unwrap().len(), 2);
        assert_eq!(created.frontmatter.len(), 1);

        let beacon = beacon.with_clock(Arc::new(Fixed(1_704_164_700)));
        let updated = beacon
            .update(
                "p",
                &UpdatePrompt {
                    name: Some(String::new()),
                    status: Some(String::new()),
                    description: Some(String::new()),
                    extends: Some("base".into()),
                    add_tags: vec!["t2".into(), "t1".into()],
                    remove_tags: vec!["t1".into()],
                    // Replaced in place, then appended.
                    sections: vec![section("A", "a2"), section("C", "c")],
                    // The parent was set earlier in the same update, so this
                    // blanks the body instead of deleting the section.
                    remove_sections: vec!["B".into(), "Z".into()],
                    add_mixins: vec!["base".into(), "base".into()],
                    frontmatter: vec![("k".into(), "v2".into()), ("n".into(), "m".into())],
                    remove_frontmatter: vec!["n".into()],
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(updated.version, 2);
        assert_eq!(updated.name, "p");
        assert_eq!(updated.status, "active");
        assert_eq!(updated.description, "");
        assert_eq!(updated.created_at, created.created_at);
        assert_ne!(updated.updated_at, created.updated_at);
        assert_eq!(updated.tags, vec!["t2".to_owned()]);
        assert_eq!(updated.mixins, vec!["base".to_owned()]);
        assert_eq!(
            updated.sections.unwrap(),
            vec![
                section("A", "a2"),
                section("B", ""),
                section("C", "c"),
                section("Z", ""),
            ]
        );
        assert_eq!(updated.frontmatter.len(), 1);
        assert_eq!(updated.frontmatter["k"], Value::String("v2".into()));

        let error = beacon
            .update(
                "p",
                &UpdatePrompt {
                    status: Some("gone".into()),
                    ..Default::default()
                },
            )
            .unwrap_err();
        assert_eq!(error.to_string(), "invalid status: gone");
        assert_eq!(beacon.show("p").unwrap().version, 2);
    }

    #[test]
    fn remove_section_blanks_when_extending_and_deletes_otherwise() {
        let (_dir, beacon) = ready();
        beacon
            .create(&CreatePrompt {
                name: "base".into(),
                ..Default::default()
            })
            .unwrap();
        for (name, extends) in [("plain", ""), ("child", "base")] {
            beacon
                .create(&CreatePrompt {
                    name: name.into(),
                    extends: extends.into(),
                    sections: vec![section("A", "a"), section("B", "b")],
                    ..Default::default()
                })
                .unwrap();
        }
        let remove = UpdatePrompt {
            remove_sections: vec!["A".into(), "Missing".into()],
            ..Default::default()
        };
        let plain = beacon.update("plain", &remove).unwrap();
        assert_eq!(plain.sections.unwrap(), vec![section("B", "b")]);
        let child = beacon.update("child", &remove).unwrap();
        assert_eq!(
            child.sections.unwrap(),
            vec![section("A", ""), section("B", "b"), section("Missing", "")]
        );
        // With no section left, a plain prompt keeps an empty list, not null.
        let emptied = beacon
            .update(
                "plain",
                &UpdatePrompt {
                    remove_sections: vec!["B".into()],
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(emptied.sections, Some(Vec::new()));
    }

    #[test]
    fn history_sorts_newest_first() {
        let (_dir, beacon) = ready();
        beacon
            .create(&CreatePrompt {
                name: "h".into(),
                ..Default::default()
            })
            .unwrap();
        for text in ["one", "two"] {
            beacon
                .update(
                    "h",
                    &UpdatePrompt {
                        description: Some(text.into()),
                        ..Default::default()
                    },
                )
                .unwrap();
        }
        beacon.archive("h").unwrap();
        let versions: Vec<i64> = beacon
            .history("h", 20)
            .unwrap()
            .iter()
            .map(|p| p.version)
            .collect();
        assert_eq!(versions, vec![4, 3, 2, 1]);
        let limited = beacon.history("h@2", 2).unwrap();
        assert_eq!(limited.len(), 2);
        assert_eq!(limited[0].version, 4);
        assert_eq!(
            beacon.history("h", 0).unwrap_err().to_string(),
            r#"prompt "h" not found"#
        );
        assert!(beacon.history("nope", 5).is_err());
        assert_eq!(
            beacon.archive("h").unwrap_err().to_string(),
            r#"prompt "h" is already archived"#
        );
    }
}
