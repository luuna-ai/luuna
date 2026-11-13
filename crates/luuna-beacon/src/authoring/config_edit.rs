//! Config mutations validate and compare a complete read-back before acquiring a write lock.
use super::{
    yaml_decode::{self, Decoded},
    yaml_value::Node,
    AuthoringError as E, YamlValue,
};
use crate::{paths, store, Beacon, BeaconError};
use std::{fs, sync::Arc};
#[derive(Debug, Clone, PartialEq)]
pub struct ConfigShow {
    pub raw: YamlValue,
    pub text: Vec<u8>,
}
/// The reference command's intentionally shallow schema.
pub fn config_schema() -> serde_json::Value {
    serde_json::json!({"$schema":"https://json-schema.org/draft/2020-12/schema","type":"object","required":["project","version"],"properties":{"project":{"type":"string","minLength":1},"version":{"type":"string","minLength":1},"targets":{"type":"object"}},"additionalProperties":false})
}
pub(super) fn read(beacon: &Beacon) -> Result<(Vec<u8>, Decoded), BeaconError> {
    beacon.migrate_legacy()?;
    let path = paths::config_file(beacon.project_root());
    let bytes = fs::read(&path).map_err(|e| BeaconError::os("open", &path, e))?;
    let text = std::str::from_utf8(&bytes)
        .map_err(|_| BeaconError::Yaml("invalid leading UTF-8 octet".into()))?;
    let doc = yaml_decode::decode(text, true)?;
    if !doc.value.is_null() && doc.value.fields(doc.value.root).is_none() {
        return Err(BeaconError::Yaml(
            "unmarshal errors:\n  line 1: cannot unmarshal !!seq into map[string]interface {}"
                .into(),
        ));
    }
    Ok((bytes, doc))
}
impl Beacon {
    pub fn config_show(&self) -> Result<ConfigShow, BeaconError> {
        let (text, doc) = read(self)?;
        Ok(ConfigShow {
            raw: doc.value,
            text,
        })
    }
    pub fn config_get(&self, path: &str) -> Result<YamlValue, BeaconError> {
        let (_, doc) = read(self)?;
        let mut id = doc.value.root;
        for part in path.split('.') {
            id = doc
                .value
                .field(id, part)
                .ok_or_else(|| E::PathNotFound(path.into()))?
        }
        Ok(doc.value.subtree(id))
    }
    pub fn config_set(&self, path: &str, replacement: &str) -> Result<YamlValue, BeaconError> {
        let (_, mut doc) = read(self)?;
        let new = yaml_decode::decode(replacement, false)?;
        let parts: Vec<_> = path.split('.').collect();
        if parts[0].is_empty() {
            return Err(E::ConfigPathEmpty.into());
        }
        if doc.value.is_null() {
            return Err(E::GoPanic("assignment to entry in nil map".into()).into());
        }
        let mut current = doc.value.root;
        for &part in &parts[..parts.len() - 1] {
            current = match doc.value.field(current, part) {
                Some(id) if doc.value.fields(id).is_some() => id,
                Some(_) => return Err(E::NotAnObject(part.into()).into()),
                None => {
                    let id = doc.value.push(Node::Map(Vec::new()));
                    put(&mut doc.value, current, part, id);
                    id
                }
            }
        }
        let replacement = doc.value.append(&new.value);
        put(&mut doc.value, current, parts[parts.len() - 1], replacement);
        validate(&doc.value)?;
        save(self, &doc, new.later)?;
        Ok(new.value)
    }
    pub fn config_unset(&self, path: &str) -> Result<bool, BeaconError> {
        let (_, mut doc) = read(self)?;
        let parts: Vec<_> = path.split('.').collect();
        let mut current = doc.value.root;
        for &part in &parts[..parts.len() - 1] {
            let Some(id) = doc.value.field(current, part) else {
                return Ok(false);
            };
            if doc.value.fields(id).is_none() {
                return Ok(false);
            }
            current = id
        }
        let Some(fields) = doc.value.fields(current) else {
            return Ok(false);
        };
        let Some(at) = fields
            .iter()
            .position(|(key, _)| doc.value.string(*key) == Some(parts[parts.len() - 1]))
        else {
            return Ok(false);
        };
        if let Node::Map(fields) = &mut doc.value.nodes[current] {
            fields.remove(at);
        }
        validate(&doc.value)?;
        save(self, &doc, false)?;
        Ok(true)
    }
}
fn put(value: &mut YamlValue, parent: usize, name: &str, id: usize) {
    let existing = value.fields(parent).and_then(|fields| {
        fields
            .iter()
            .position(|(key, _)| value.string(*key) == Some(name))
    });
    if let Some(at) = existing {
        if let Node::Map(fields) = &mut value.nodes[parent] {
            fields[at].1 = id
        }
    } else {
        let key = value.push(Node::String(Arc::from(name.as_bytes())));
        if let Node::Map(fields) = &mut value.nodes[parent] {
            fields.push((key, id))
        }
    }
}
fn save(beacon: &Beacon, doc: &Decoded, new_later: bool) -> Result<(), BeaconError> {
    if doc.later {
        return Err(E::ConfigLaterDocuments("config.yaml").into());
    }
    if new_later {
        return Err(E::ConfigLaterDocuments("the value").into());
    }
    if doc.null_key {
        return Err(E::ConfigNullKey.into());
    }
    let text = doc.value.to_yaml();
    let after = yaml_decode::decode(&text, true)?.value;
    if let Some(path) = doc.value.difference(&after) {
        return Err(E::ConfigReadBack(path).into());
    }
    let path = paths::config_file(beacon.project_root());
    let _lock = store::acquire(&path)?;
    store::atomic_write(&path, text.as_bytes())
}
fn validate(value: &YamlValue) -> Result<(), BeaconError> {
    let fail = |s: String| BeaconError::from(E::ConfigValidation(s));
    let root = value.root;
    for name in ["project", "version"] {
        if value
            .field(root, name)
            .and_then(|id| value.string(id))
            .is_none_or(|s| s.trim().is_empty())
        {
            return Err(fail(format!("{name} must be a non-empty string")));
        }
    }
    for &(key, _) in value.fields(root).unwrap_or(&[]) {
        let key = value.string(key).unwrap_or("");
        if !["project", "version", "targets"].contains(&key) {
            return Err(fail(format!(
                "unknown property {}",
                crate::gofmt::quote(key)
            )));
        }
    }
    if let Some(targets) = value.field(root, "targets") {
        let fields = value
            .fields(targets)
            .ok_or_else(|| fail("targets must be an object".into()))?;
        for &(key, target) in fields {
            let name = value.string(key).unwrap_or("");
            if value.string_fields(target).is_none() {
                return Err(fail(format!("targets.{name} must be an object")));
            }
            if value
                .field(target, "dir")
                .and_then(|id| value.string(id))
                .is_none_or(|s| s.trim().is_empty())
            {
                return Err(fail(format!(
                    "targets.{name}.dir must be a non-empty string"
                )));
            }
            if let Some(id) = value.field(target, "default") {
                if !matches!(value.nodes[id], Node::Bool(_)) {
                    return Err(fail(format!("targets.{name}.default must be boolean")));
                }
            }
            if let Some(id) = value.field(target, "tags") {
                let Node::Seq(tags) = &value.nodes[id] else {
                    return Err(fail(format!("targets.{name}.tags must be an array")));
                };
                if tags.iter().any(|id| value.string(*id).is_none()) {
                    return Err(fail(format!("targets.{name}.tags must contain strings")));
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture(text: &str) -> (tempfile::TempDir, Beacon) {
        let dir = tempfile::tempdir().unwrap();
        let beacon = Beacon::open(dir.path()).unwrap();
        beacon.init().unwrap();
        fs::write(beacon.store_dir().join("config.yaml"), text).unwrap();
        (dir, beacon)
    }
    const BASE: &str = "project: p\nversion: \"1\"\ntargets: {}\n";
    #[test]
    fn get_nested_and_missing() {
        let (_dir, b) = fixture("project: p\nversion: '1'\ntargets: {a: {dir: output}}\n");
        assert_eq!(
            b.config_get("targets.a.dir").unwrap().to_go_json().unwrap(),
            "\"output\""
        );
        assert!(matches!(
            b.config_get("targets.missing"),
            Err(BeaconError::Authoring(E::PathNotFound(_)))
        ))
    }
    #[test]
    fn set_value_and_roundtrip() {
        let (_dir, b) = fixture(BASE);
        assert_eq!(
            b.config_set("targets.a", "{dir: output, default: true}")
                .unwrap()
                .to_go_json()
                .unwrap(),
            "{\"default\":true,\"dir\":\"output\"}"
        );
        assert_eq!(b.config_get("targets.a.dir").unwrap().to_yaml(), "output\n")
    }
    #[test]
    fn unset_absent_does_not_rewrite() {
        let (_dir, b) = fixture(BASE);
        assert!(!b.config_unset("targets.missing").unwrap());
        assert_eq!(b.config_show().unwrap().text, BASE.as_bytes())
    }
    #[test]
    fn invalid_set_preserves_bytes() {
        let (_dir, b) = fixture(BASE);
        assert!(b.config_set("project", "[]").is_err());
        assert_eq!(b.config_show().unwrap().text, BASE.as_bytes())
    }
    #[test]
    fn negative_zero_preserves_bytes() {
        let text = "project: p\nversion: '1'\ntargets: {a: {dir: output, extra: -0.0}}\n";
        let (_dir, b) = fixture(text);
        let err = b.config_set("project", "q").unwrap_err();
        assert!(
            matches!(err,BeaconError::Authoring(E::ConfigReadBack(ref p)) if p=="targets.a.extra")
        );
        assert_eq!(b.config_show().unwrap().text, text.as_bytes())
    }
    #[test]
    fn extra_document_preserves_bytes() {
        let text = format!("{BASE}---\nsecond: value\n");
        let (_dir, b) = fixture(&text);
        assert!(matches!(
            b.config_set("project", "q"),
            Err(BeaconError::Authoring(E::ConfigLaterDocuments(
                "config.yaml"
            )))
        ));
        assert_eq!(b.config_show().unwrap().text, text.as_bytes())
    }
    #[test]
    fn null_key_preserves_bytes() {
        let text = format!("{BASE}null: important\n");
        let (_dir, b) = fixture(&text);
        assert!(matches!(
            b.config_set("project", "q"),
            Err(BeaconError::Authoring(E::ConfigNullKey))
        ));
        assert_eq!(b.config_show().unwrap().text, text.as_bytes())
    }
    #[test]
    fn target_order_and_legacy_routes() {
        let(_dir,b)=fixture("project: p\nversion: '1'\ntargets:\n  z: {dir: output, default: 'true', tags: [one, 2]}\n  a: {dir: other}\n");
        let targets = b.config_targets().unwrap().targets.unwrap();
        assert_eq!(
            targets.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(),
            ["z", "a"]
        );
        assert!(targets[0].default);
        assert_eq!(targets[0].tags, ["one"])
    }
}
