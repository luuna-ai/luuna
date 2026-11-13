//! Markdown import checks all known data-loss boundaries before appending.
use super::{yaml_decode, yaml_value::Node, AuthoringError as E, YamlValue};
use crate::{gojson, prompts, store, Beacon, BeaconError, Prompt, Section};
use std::{collections::BTreeMap, fs, io::Read};
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImportMarkdown {
    pub path: String,
    pub name: String,
    pub tags: Vec<String>,
    pub no_split: bool,
}
#[derive(Debug, Clone, PartialEq)]
pub struct Imported {
    pub prompt: Prompt,
    pub sections: usize,
}
impl Beacon {
    pub fn import_markdown(&self, input: &ImportMarkdown) -> Result<Imported, BeaconError> {
        self.migrate_legacy()?;
        if input.name.is_empty() {
            return Err(BeaconError::NameRequired);
        }
        let file = if std::path::Path::new(&input.path).is_absolute() {
            std::path::PathBuf::from(&input.path)
        } else {
            self.project_root().join(&input.path)
        };
        let mut source =
            fs::File::open(&file).map_err(|e| BeaconError::os("open", &input.path, e))?;
        let mut bytes = Vec::new();
        source
            .read_to_end(&mut bytes)
            .map_err(|e| BeaconError::os("read", &input.path, e))?;
        let text = String::from_utf8_lossy(&bytes);
        let mut body = text.as_ref();
        let mut frontmatter = YamlValue::null();
        frontmatter.nodes[0] = Node::Map(Vec::new());
        let mut later = false;
        let mut null_key = false;
        let trimmed = body.strip_prefix('\u{feff}').unwrap_or(body);
        let suspicious = body.starts_with("---\n") && body.contains("\n---\r\n")
            || (body.starts_with('\u{feff}') && trimmed.starts_with("---"))
            || body.starts_with("---\r\n")
            || body.starts_with("---\n---\n");
        if body.starts_with("---\n") {
            if let Some(end) = body[4..].find("\n---\n") {
                let doc = yaml_decode::decode(&body[4..end + 4], true)
                    .map_err(|e| E::InvalidFrontmatter(e.to_string()))?;
                if !doc.value.is_null() && doc.value.fields(doc.value.root).is_none() {
                    let kind = match doc.value.nodes[doc.value.root] {
                        Node::Seq(_) => "seq",
                        Node::Bool(_) => "bool",
                        Node::Int(_) | Node::Uint(_) => "int",
                        Node::Float(_) => "float",
                        _ => "str",
                    };
                    return Err(E::InvalidFrontmatter(format!("yaml: unmarshal errors:\n  line 1: cannot unmarshal !!{kind}{} into map[string]interface {{}}",doc.value.string(doc.value.root).map(|s|format!(" `{s}`")).unwrap_or_default())).into());
                }
                frontmatter = doc.value;
                later = doc.later;
                null_key = doc.null_key;
                body = &body[end + 9..];
            }
        }
        let mut sections = Vec::<Section>::new();
        let mut intro = String::new();
        if input.no_split {
            sections.push(Section {
                name: "body".into(),
                body: body.trim().into(),
                required: None,
            })
        } else {
            for line in body.split('\n') {
                if let Some(name) = line.strip_prefix("## ") {
                    sections.push(Section {
                        name: name.trim().into(),
                        ..Section::default()
                    })
                } else if let Some(section) = sections.last_mut() {
                    if !section.body.is_empty() {
                        section.body.push('\n')
                    }
                    section.body.push_str(line)
                } else {
                    intro.push_str(line);
                    intro.push('\n')
                }
            }
        }
        if sections.is_empty() {
            sections.push(Section {
                name: "body".into(),
                body: body.trim().into(),
                required: None,
            });
            intro.clear()
        }
        // Refusals occur at the final pre-append boundary, after Go's errors.
        if suspicious {
            return Err(E::ImportUnrecognisedFrontmatter.into());
        }
        if !intro
            .trim_matches(|c: char| c.is_ascii_whitespace())
            .is_empty()
        {
            return Err(E::ImportTextBeforeFirstHeading.into());
        }
        let root = frontmatter.root;
        let description = match frontmatter.field(root, "description") {
            Some(id) => Some(
                frontmatter
                    .string(id)
                    .ok_or(E::ImportDescriptionNotString)?
                    .to_owned(),
            ),
            None => None,
        };
        if null_key {
            return Err(E::ImportNullFrontmatterKey.into());
        }
        if later {
            return Err(E::ImportLaterFrontmatterDocuments.into());
        }
        if let Some(id) = frontmatter.field(root, "name") {
            let rendered = frontmatter.subtree(id).to_yaml();
            let name = if matches!(frontmatter.nodes[id], Node::Seq(_) | Node::Map(_)) {
                rendered
            } else {
                rendered.trim_end_matches('\n').into()
            };
            let same = frontmatter.string(id) == Some(input.name.as_str());
            if !same {
                let name = frontmatter.string(id).map(str::to_owned).unwrap_or(name);
                return Err(E::ImportFrontmatterNameDiffers(name).into());
            }
        }
        let path = self.prompts_path()?;
        let records: Vec<Prompt> = store::read_jsonl(&path)?;
        if prompts::current(records)
            .iter()
            .any(|p| p.name == input.name && p.status != "archived")
        {
            return Err(BeaconError::PromptNameExists(input.name.clone()));
        }
        let config = self.config_targets()?;
        let now = self.now_text();
        let count = sections.len();
        let mut prompt = Prompt {
            id: format!("{}-{}", config.project, self.next_id_hex()),
            name: input.name.clone(),
            version: 1,
            sections: Some(sections),
            tags: input.tags.clone(),
            status: "active".into(),
            created_at: now.clone(),
            updated_at: now,
            description: description.unwrap_or_default(),
            ..Prompt::default()
        };
        let fields = frontmatter
            .fields(root)
            .unwrap_or(&[])
            .iter()
            .copied()
            .filter(|(key, _)| !matches!(frontmatter.string(*key), Some("name" | "description")))
            .collect::<Vec<_>>();
        frontmatter.nodes[root] = Node::Map(fields);
        // Go marshals the complete record before the first store write. Preserve
        // its first encoding error, including the full JSON pointer and offset.
        let skeleton = prompt.to_go_json();
        let position = skeleton.find(",\"status\":").expect("Prompt has status");
        let prefix = format!("{},\"frontmatter\":", &skeleton[..position]);
        let encoded = super::yaml_json::encode_in(&frontmatter, &prefix, "/frontmatter")?;
        let raw = gojson::parse(encoded.as_bytes()).map_err(BeaconError::Yaml)?;
        let parsed = gojson::to_any(&raw);
        let mut map = BTreeMap::new();
        let mapped = gojson::map_into(&mut map, &raw);
        drain(raw);
        mapped.map_err(BeaconError::Yaml)?;
        let parsed = parsed.map_err(BeaconError::Yaml)?;
        // RD21 names the first differing value in sorted map-key order.
        for id in 0..frontmatter.nodes.len() {
            if let Node::Map(fields) = &frontmatter.nodes[id] {
                let mut fields = fields.clone();
                fields
                    .sort_by(|(a, _), (b, _)| frontmatter.string(*a).cmp(&frontmatter.string(*b)));
                frontmatter.nodes[id] = Node::Map(fields)
            }
        }
        let after = YamlValue::from_json(&parsed);
        if let Some(path) = frontmatter.difference(&after) {
            return Err(E::ImportReadBack(path).into());
        }
        prompt.frontmatter = map;
        store::append_jsonl(&path, &prompt)?;
        Ok(Imported {
            prompt,
            sections: count,
        })
    }
}
fn drain(value: gojson::GoValue) {
    let mut work = vec![value];
    while let Some(v) = work.pop() {
        match v {
            gojson::GoValue::Array(v) => work.extend(v),
            gojson::GoValue::Object(v) => work.extend(v.into_iter().map(|(_, v)| v)),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture(text: &str) -> (tempfile::TempDir, Beacon, ImportMarkdown) {
        let dir = tempfile::tempdir().unwrap();
        let b = Beacon::open(dir.path()).unwrap();
        b.init().unwrap();
        fs::write(dir.path().join("input.md"), text).unwrap();
        (
            dir,
            b,
            ImportMarkdown {
                path: "input.md".into(),
                name: "p".into(),
                ..ImportMarkdown::default()
            },
        )
    }
    #[test]
    fn headingless_body_trims() {
        let (_d, b, i) = fixture("  hello\n");
        assert_eq!(
            b.import_markdown(&i).unwrap().prompt.sections.unwrap()[0].body,
            "hello"
        )
    }
    #[test]
    fn headings_split() {
        let (_d, b, i) = fixture("## one\nfirst\n## two\nsecond\n");
        let out = b.import_markdown(&i).unwrap();
        assert_eq!(out.sections, 2);
        assert_eq!(out.prompt.sections.unwrap()[0].body, "first")
    }
    #[test]
    fn no_split_keeps_heading() {
        let (_d, b, mut i) = fixture("## one\nfirst\n");
        i.no_split = true;
        assert_eq!(
            b.import_markdown(&i).unwrap().prompt.sections.unwrap()[0].body,
            "## one\nfirst"
        )
    }
    #[test]
    fn intro_refusal_is_before_store_read() {
        let (_d, b, i) = fixture("lost intro\n## one\nfirst\n");
        fs::write(b.store_dir().join("prompts.jsonl"), "invalid").unwrap();
        assert!(matches!(
            b.import_markdown(&i),
            Err(BeaconError::Authoring(E::ImportTextBeforeFirstHeading))
        ))
    }
    #[test]
    fn description_refusal_preserves_store() {
        let (_d, b, i) = fixture("---\ndescription: 12\n---\nbody\n");
        assert!(matches!(
            b.import_markdown(&i),
            Err(BeaconError::Authoring(E::ImportDescriptionNotString))
        ));
        assert_eq!(fs::read(b.store_dir().join("prompts.jsonl")).unwrap(), b"")
    }
    #[test]
    fn precise_integer_refuses_json_rounding() {
        let (_d, b, i) = fixture("---\nextra: 9007199254740993\n---\nbody\n");
        assert!(
            matches!(b.import_markdown(&i),Err(BeaconError::Authoring(E::ImportReadBack(ref p)))if p=="extra")
        );
        assert_eq!(fs::read(b.store_dir().join("prompts.jsonl")).unwrap(), b"")
    }
    #[test]
    fn datetime_survives_json_as_string() {
        let (_d, b, i) = fixture("---\nextra: 2001-12-14\n---\nbody\n");
        let out = b.import_markdown(&i).unwrap();
        assert_eq!(
            out.prompt.frontmatter["extra"].as_str(),
            Some("2001-12-14T00:00:00Z")
        )
    }
}
