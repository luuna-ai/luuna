//! Inheritance resolution (`resolve`, `mergeAtlas` and `mergeSections`,
//! `cli.go:455-585`) and `render` (`cli.go:620-640`).
//!
//! The walk follows Go's: a depth limit of 5, cycles detected by the key
//! `<id>@<version>` while a record is being resolved, the parent's sections
//! first, then each mixin left to right, then the focal record's own; an
//! overlay section with an empty body removes the section; frontmatter is
//! merged one level deep, a later source replacing a key; Atlas metadata is
//! merged only when the focal record sets `extends_atlas` to true. The same
//! walk collects the source names (assumption AS2).
//!
//! Frontmatter values are never cloned or compared: the walk records which
//! record each key comes from, and the values are moved out of the records
//! once at the end (a value can be nested 10000 levels deep).

use crate::beacon::Beacon;
use crate::error::BeaconError;
use crate::model::{Atlas, AtlasPrime, Prompt, Section};
use crate::payload::Rendered;
use crate::prompts::current;
use crate::{gofmt, store};
use std::collections::{BTreeMap, HashSet};

/// Go's `model.MaxInheritDepth`.
const MAX_INHERIT_DEPTH: usize = 5;

/// What resolving one record produced, with frontmatter values left in the
/// records.
struct Partial {
    /// The record that was resolved.
    chosen: usize,
    sections: Vec<Section>,
    /// Each frontmatter key and the record whose value it takes.
    frontmatter: BTreeMap<String, usize>,
    atlas: Option<Atlas>,
    sources: Vec<String>,
}

struct Walk<'a> {
    records: &'a [Prompt],
    visiting: HashSet<String>,
}

/// Appends the names of `more` not yet in `sources`, keeping their order.
fn add_sources(sources: &mut Vec<String>, more: Vec<String>) {
    for name in more {
        if !sources.contains(&name) {
            sources.push(name);
        }
    }
}

impl Walk<'_> {
    /// Go's `resolve`: the record named by `reference` (the text before the
    /// first `@`; the version, if any, is `version`), highest version first,
    /// the first record in `records` on a tie.
    fn resolve(
        &mut self,
        reference: &str,
        version: Option<i64>,
        depth: usize,
    ) -> Result<Partial, BeaconError> {
        if depth > MAX_INHERIT_DEPTH {
            return Err(BeaconError::MaxInheritanceDepth);
        }
        let name = reference.split('@').next().unwrap_or_default();
        let mut chosen: Option<usize> = None;
        for (index, candidate) in self.records.iter().enumerate() {
            if candidate.name != name {
                continue;
            }
            if version.is_some_and(|wanted| candidate.version != wanted) {
                continue;
            }
            if chosen.is_none_or(|best| candidate.version > self.records[best].version) {
                chosen = Some(index);
            }
        }
        let Some(chosen) = chosen else {
            return Err(BeaconError::PromptNotFound(reference.to_owned()));
        };
        let record = &self.records[chosen];
        let key = format!("{}@{}", record.id, record.version);
        if self.visiting.contains(&key) {
            return Err(BeaconError::CircularInheritance(record.name.clone()));
        }
        self.visiting.insert(key.clone());
        let result = self.merge(chosen, depth);
        self.visiting.remove(&key);
        result
    }

    fn merge(&mut self, chosen: usize, depth: usize) -> Result<Partial, BeaconError> {
        let records = self.records;
        let record = &records[chosen];
        let extends_atlas = record.extends_atlas == Some(true);
        let mut sources = vec![record.name.clone()];
        let mut sections: Vec<Section> = Vec::new();
        let mut frontmatter: BTreeMap<String, usize> = BTreeMap::new();
        let mut inherited_atlas: Option<Atlas> = None;
        if !record.extends.is_empty() {
            let parent = self.resolve(&record.extends, None, depth + 1)?;
            sections = parent.sections;
            frontmatter = parent.frontmatter;
            inherited_atlas = parent.atlas;
            add_sources(&mut sources, parent.sources);
        }
        for mixin_name in &record.mixins {
            let mixin = self.resolve(mixin_name, None, depth + 1)?;
            sections = merge_sections(sections, &mixin.sections);
            frontmatter.extend(mixin.frontmatter);
            if extends_atlas {
                inherited_atlas = merge_atlas(inherited_atlas, mixin.atlas);
            }
            add_sources(&mut sources, mixin.sources);
        }
        let own_sections = record.sections.as_deref().unwrap_or_default();
        let sections = merge_sections(sections, own_sections);
        for key in record.frontmatter.keys() {
            frontmatter.insert(key.clone(), chosen);
        }
        let atlas = if extends_atlas {
            merge_atlas(inherited_atlas, record.atlas.clone())
        } else {
            record.atlas.clone()
        };
        Ok(Partial {
            chosen,
            sections,
            frontmatter,
            atlas,
            sources,
        })
    }
}

/// Go's `mergeSections`: a copy of `base` in which each overlay section
/// replaces the last section of the same name or is appended; an overlay
/// with an empty body removes the last section of that name instead.
fn merge_sections(base: Vec<Section>, overlay: &[Section]) -> Vec<Section> {
    let mut result = base;
    for replacement in overlay {
        let found = result
            .iter()
            .rposition(|section| section.name == replacement.name);
        if replacement.body.is_empty() {
            if let Some(found) = found {
                result.remove(found);
            }
            continue;
        }
        match found {
            Some(found) => result[found] = replacement.clone(),
            None => result.push(replacement.clone()),
        }
    }
    result
}

/// Go's `mergeAtlas`: either side alone is kept as it is; with both, the
/// prime lists are joined without repeats (base first), and the overlay's
/// budget and `on_empty` win when set.
fn merge_atlas(base: Option<Atlas>, overlay: Option<Atlas>) -> Option<Atlas> {
    let (base, overlay) = match (base, overlay) {
        (None, overlay) => return overlay,
        (base, None) => return base,
        (Some(base), Some(overlay)) => (base, overlay),
    };
    let prime = if base.prime.is_some() || overlay.prime.is_some() {
        let mut prime = AtlasPrime::default();
        for side in [&base.prime, &overlay.prime].into_iter().flatten() {
            for domain in &side.domains {
                if !prime.domains.contains(domain) {
                    prime.domains.push(domain.clone());
                }
            }
            for file in &side.files {
                if !prime.files.contains(file) {
                    prime.files.push(file.clone());
                }
            }
        }
        Some(prime)
    } else {
        None
    };
    Some(Atlas {
        prime,
        budget: overlay.budget.or(base.budget),
        on_empty: if overlay.on_empty.is_empty() {
            base.on_empty
        } else {
            overlay.on_empty
        },
    })
}

/// Resolves `reference` over `records` (Go's `resolve(records, reference,
/// version, nil, 0)`) and builds the resolved record, moving frontmatter
/// values out of `records`.
pub(crate) fn resolve(
    mut records: Vec<Prompt>,
    reference: &str,
    version: Option<i64>,
) -> Result<Rendered, BeaconError> {
    let partial = Walk {
        records: &records,
        visiting: HashSet::new(),
    }
    .resolve(reference, version, 0)?;
    let mut frontmatter = BTreeMap::new();
    for (key, source) in partial.frontmatter {
        let value = records[source]
            .frontmatter
            .remove(&key)
            .expect("each key is taken from the record that holds it, once");
        frontmatter.insert(key, value);
    }
    let mut prompt = std::mem::take(&mut records[partial.chosen]);
    prompt.sections = Some(partial.sections);
    if !frontmatter.is_empty() {
        prompt.frontmatter = frontmatter;
    }
    prompt.atlas = partial.atlas;
    Ok(Rendered {
        prompt,
        resolved_from: partial.sources,
    })
}

impl Beacon {
    /// Go's `render`: resolves `reference` (`name` or `name@N`) over the
    /// current records only, so `name@N` works only for the current version.
    /// `render --format json` prints [`Rendered::prompt`];
    /// [`Rendered::to_payload`] is the `--json` document.
    pub fn render(&self, reference: &str) -> Result<Rendered, BeaconError> {
        let path = self.prompts_path()?;
        let records: Vec<Prompt> = store::read_jsonl(&path)?;
        let records = current(records);
        let (name, wanted) = match reference.split_once('@') {
            Some((name, text)) => match gofmt::sscan_int(text) {
                Some(version) => (name, Some(version)),
                None => return Err(BeaconError::InvalidVersion(text.to_owned())),
            },
            None => (reference, None),
        };
        resolve(records, name, wanted)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn section(name: &str, body: &str) -> Section {
        Section {
            name: name.to_owned(),
            body: body.to_owned(),
            required: None,
        }
    }

    fn record(id: &str, name: &str, version: i64) -> Prompt {
        Prompt {
            id: id.to_owned(),
            name: name.to_owned(),
            version,
            sections: Some(vec![]),
            ..Prompt::default()
        }
    }

    #[test]
    fn sections_merge_like_go() {
        let base = vec![section("A", "a"), section("B", "b"), section("A", "a2")];
        let merged = merge_sections(
            base,
            &[
                section("A", "x"),
                section("B", ""),
                section("C", "c"),
                section("Z", ""),
            ],
        );
        assert_eq!(
            merged,
            vec![section("A", "a"), section("A", "x"), section("C", "c")]
        );
    }

    #[test]
    fn atlas_merges_like_go() {
        let atlas = |domains: &[&str], budget: Option<f64>, on_empty: &str| Atlas {
            prime: Some(AtlasPrime {
                domains: domains.iter().map(|d| (*d).to_owned()).collect(),
                files: vec![],
            }),
            budget,
            on_empty: on_empty.to_owned(),
        };
        assert_eq!(merge_atlas(None, None), None);
        let merged = merge_atlas(
            Some(atlas(&["a", "b"], Some(1.0), "warn")),
            Some(atlas(&["b", "c"], None, "")),
        )
        .unwrap();
        assert_eq!(merged, atlas(&["a", "b", "c"], Some(1.0), "warn"));
        let neither = merge_atlas(
            Some(Atlas::default()),
            Some(Atlas {
                on_empty: "skip".to_owned(),
                ..Atlas::default()
            }),
        )
        .unwrap();
        assert_eq!(neither.prime, None);
        assert_eq!(neither.on_empty, "skip");
    }

    #[test]
    fn walk_order_cycles_depth_and_frontmatter() {
        let mut base = record("i1", "base", 1);
        base.frontmatter
            .insert("k".to_owned(), Value::String("base".to_owned()).into());
        base.frontmatter
            .insert("only".to_owned(), Value::String("base".to_owned()).into());
        let mut mix = record("i2", "mix", 1);
        mix.extends = "base".to_owned();
        mix.frontmatter
            .insert("k".to_owned(), Value::String("mix".to_owned()).into());
        let mut focal = record("i3", "focal", 1);
        focal.extends = "base".to_owned();
        focal.mixins = vec!["mix".to_owned()];
        let records = vec![base.clone(), mix.clone(), focal.clone()];
        let rendered = resolve(records, "focal", None).unwrap();
        assert_eq!(rendered.resolved_from, ["focal", "base", "mix"]);
        assert_eq!(rendered.prompt.frontmatter["k"], "mix");
        assert_eq!(rendered.prompt.frontmatter["only"], "base");
        assert_eq!(rendered.prompt.id, "i3");

        // A highest-version tie goes to the first record.
        let first = record("x1", "dup", 2);
        let second = record("x2", "dup", 2);
        let rendered = resolve(vec![first, second], "dup@9", None).unwrap();
        assert_eq!(rendered.prompt.id, "x1");
        // The text after `@` in a reference is ignored; the name must exist.
        assert_eq!(
            resolve(vec![record("x1", "dup", 1)], "ghost@9", None)
                .unwrap_err()
                .to_string(),
            "prompt \"ghost@9\" not found"
        );

        let mut a = record("a", "a", 1);
        a.extends = "b".to_owned();
        let mut b = record("b", "b", 1);
        b.extends = "a".to_owned();
        assert_eq!(
            resolve(vec![a, b], "a", None).unwrap_err().to_string(),
            "circular inheritance involving \"a\""
        );

        let chain: Vec<Prompt> = (0..7)
            .map(|index| {
                let mut prompt = record(&format!("c{index}"), &format!("c{index}"), 1);
                if index > 0 {
                    prompt.extends = format!("c{}", index - 1);
                }
                prompt
            })
            .collect();
        assert!(resolve(chain.clone(), "c5", None).is_ok());
        assert_eq!(
            resolve(chain, "c6", None).unwrap_err().to_string(),
            "maximum inheritance depth exceeded"
        );
    }
}
