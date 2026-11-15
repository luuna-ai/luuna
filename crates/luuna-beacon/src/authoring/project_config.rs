//! Lenient config loading preserves file order for deterministic routing.
use super::{config_edit, yaml_value::Node, YamlValue};
use crate::{Beacon, BeaconError};
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmitTarget {
    pub name: String,
    pub dir: String,
    pub default: bool,
    pub tags: Vec<String>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectConfig {
    pub project: String,
    pub version: String,
    pub targets: Option<Vec<EmitTarget>>,
}
impl Beacon {
    pub fn config_targets(&self) -> Result<ProjectConfig, BeaconError> {
        let (_, doc) = config_edit::read(self)?;
        Ok(load(&doc.value))
    }
}
fn load(v: &YamlValue) -> ProjectConfig {
    let root = v.root;
    let string = |id, name| v.field(id, name).and_then(|id| v.string(id));
    let mut config = ProjectConfig {
        project: string(root, "project")
            .filter(|s| !s.is_empty())
            .unwrap_or("luuna-beacon")
            .into(),
        version: string(root, "version")
            .filter(|s| !s.is_empty())
            .unwrap_or("1")
            .into(),
        targets: None,
    };
    if let Some(fields) = v.field(root, "targets").and_then(|id| v.string_fields(id)) {
        let mut targets = Vec::new();
        for &(key, id) in fields {
            let Some(name) = v.string(key) else { continue };
            if v.string_fields(id).is_none() {
                continue;
            }
            let default = v.field(id, "default").is_some_and(|id| {
                matches!(v.nodes[id], Node::Bool(true)) || v.string(id) == Some("true")
            });
            let tags = v
                .field(id, "tags")
                .and_then(|id| {
                    if let Node::Seq(ids) = &v.nodes[id] {
                        Some(
                            ids.iter()
                                .filter_map(|id| v.string(*id).map(str::to_owned))
                                .collect(),
                        )
                    } else {
                        None
                    }
                })
                .unwrap_or_default();
            targets.push(EmitTarget {
                name: name.into(),
                dir: string(id, "dir").unwrap_or(name).into(),
                default,
                tags,
            })
        }
        config.targets = Some(targets);
        return config;
    }
    let dir = string(root, "emitDir").unwrap_or("");
    let by_tag = v
        .field(root, "emitDirByTag")
        .and_then(|id| v.string_fields(id));
    if dir.is_empty() && by_tag.is_none() {
        return config;
    }
    let dir = if dir.is_empty() { "agents" } else { dir };
    let mut targets = vec![EmitTarget {
        name: "default".into(),
        dir: dir.into(),
        default: true,
        tags: Vec::new(),
    }];
    let target_name = regex::Regex::new("[^A-Za-z0-9]+").expect("literal pattern");
    for &(key, id) in by_tag.unwrap_or(&[]) {
        let (Some(tag), Some(dir)) = (v.string(key), v.string(id)) else {
            continue;
        };
        if dir.is_empty() {
            continue;
        }
        if let Some(target) = targets.iter_mut().find(|t| t.dir == dir) {
            target.tags.push(tag.into());
            continue;
        }
        let base = target_name
            .replace_all(dir, "-")
            .trim_matches('-')
            .to_owned();
        let mut name = if base.is_empty() {
            "target".into()
        } else {
            base
        };
        while targets.iter().any(|t| t.name == name) {
            name.push_str("-2")
        }
        targets.push(EmitTarget {
            name,
            dir: dir.into(),
            default: false,
            tags: vec![tag.into()],
        })
    }
    config.targets = Some(targets);
    config
}
