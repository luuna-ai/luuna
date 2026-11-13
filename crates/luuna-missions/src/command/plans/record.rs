//! Go Plan record state and persisted section declaration order.
use crate::{
    error::{Error, Result},
    json,
    store::Record,
};
use std::collections::HashSet;
#[derive(Debug, Clone)]
struct State {
    original: json::Members,
    baseline: json::Members,
}
impl State {
    fn merge(&self, current: &[u8], known: &[&str]) -> Result<Vec<u8>> {
        let values = json::parse_object(current).map_err(encoding)?;
        let mut out = vec![b'{'];
        let mut written = HashSet::new();
        for key in self.original.keys() {
            let original = self.original.get(key).unwrap();
            if !known.contains(&key.as_str()) {
                member(&mut out, &mut written, key, original);
                continue;
            }
            match (values.get(key), self.baseline.get(key)) {
                (None, None) => member(&mut out, &mut written, key, original),
                (Some(value), Some(baseline)) if value == baseline => {
                    member(&mut out, &mut written, key, original)
                }
                (Some(value), _) => member(&mut out, &mut written, key, value),
                (None, Some(_)) => {}
            }
        }
        for key in values.keys() {
            member(&mut out, &mut written, key, values.get(key).unwrap());
        }
        out.push(b'}');
        Ok(out)
    }
}
fn encoding(e: impl std::fmt::Display) -> Error {
    Error::Encode {
        message: format!("json: invalid record encoding: {e}"),
    }
}
fn member(out: &mut Vec<u8>, written: &mut HashSet<String>, key: &str, value: &[u8]) {
    if written.contains(key) || value.is_empty() {
        return;
    }
    if !written.is_empty() {
        out.push(b',');
    }
    out.extend(json::quoted(key));
    out.push(b':');
    out.extend(json::compact(value).unwrap_or_else(|_| value.to_vec()));
    written.insert(key.into());
}

fn string(raw: Option<&[u8]>) -> String {
    raw.filter(|v| v.first() == Some(&b'"'))
        .map(json::unquote)
        .unwrap_or_default()
}
fn strings(raw: Option<&[u8]>) -> Option<Vec<String>> {
    raw.and_then(|r| json::Value::decode(r).ok())
        .and_then(json::Value::into_array)
        .map(|a| {
            a.into_iter()
                .map(|v| v.into_string().unwrap_or_default())
                .collect()
        })
}
fn field(out: &mut Vec<u8>, key: &str, value: &[u8]) {
    if out.len() > 1 {
        out.push(b',');
    }
    out.extend(json::quoted(key));
    out.push(b':');
    out.extend_from_slice(value);
}
pub(super) fn string_array(values: Option<&[String]>) -> Vec<u8> {
    let Some(values) = values else {
        return b"null".to_vec();
    };
    let mut out = vec![b'['];
    for (i, s) in values.iter().enumerate() {
        if i > 0 {
            out.push(b',');
        }
        out.extend(json::quoted(s));
    }
    out.push(b']');
    out
}
const KNOWN: &[&str] = &[
    "id",
    "seed",
    "template",
    "name",
    "status",
    "revision",
    "sections",
    "children",
    "adoptedChildren",
    "outcome",
    "outcomeNote",
    "reviewedBy",
    "createdAt",
    "updatedAt",
];
#[derive(Debug, Clone, Default)]
pub(super) struct Plan {
    pub id: String,
    pub seed: String,
    pub template: String,
    pub name: String,
    pub status: String,
    pub revision: i64,
    pub sections: Option<std::collections::BTreeMap<String, json::Value>>,
    pub children: Option<Vec<String>>,
    pub adopted_children: Option<Vec<String>>,
    pub outcome: String,
    pub outcome_note: String,
    pub reviewed_by: String,
    pub created_at: String,
    pub updated_at: String,
    state: Option<State>,
    pub section_order: Vec<String>,
}
impl Plan {
    pub fn section_keys(&self) -> Vec<String> {
        let Some(sections) = &self.sections else {
            return vec![];
        };
        let mut keys = vec![];
        let mut seen = HashSet::new();
        let fallback = self
            .state
            .as_ref()
            .and_then(|s| s.original.get("sections"))
            .and_then(|raw| json::parse_object(raw).ok());
        let order = if self.section_order.is_empty() {
            fallback
                .as_ref()
                .map(json::Members::keys)
                .unwrap_or_default()
        } else {
            &self.section_order
        };
        for key in order {
            if sections.contains_key(key) && seen.insert(key.clone()) {
                keys.push(key.clone());
            }
        }
        keys.extend(sections.keys().filter(|k| !seen.contains(*k)).cloned());
        keys
    }
    pub fn section(&self, key: &str) -> Option<&json::Value> {
        self.sections.as_ref()?.get(key)
    }
    pub fn sections_wire(&self) -> Result<Vec<u8>> {
        let Some(sections) = &self.sections else {
            return Ok(b"null".to_vec());
        };
        let mut out = vec![b'{'];
        for (i, key) in self.section_keys().iter().enumerate() {
            if i > 0 {
                out.push(b',');
            }
            out.extend(json::quoted(key));
            out.push(b':');
            sections[key]
                .encode(&mut out, true)
                .map_err(|e| Error::Encode {
                    message: e.to_string(),
                })?;
        }
        out.push(b'}');
        Ok(out)
    }
    pub fn raw_field(&self, key: &str) -> Option<&[u8]> {
        self.state.as_ref()?.original.get(key)
    }
    pub fn field_changed(&self, key: &str) -> Result<bool> {
        let Some(state) = &self.state else {
            return Ok(false);
        };
        let wire = self.wire()?;
        let fields = json::parse_object(&wire).map_err(encoding)?;
        Ok(fields.get(key) != state.baseline.get(key))
    }
    pub fn raw_sections(&self) -> Option<&[u8]> {
        self.state.as_ref()?.original.get("sections")
    }
    pub fn sections_changed(&self) -> Result<bool> {
        let Some(state) = &self.state else {
            return Ok(false);
        };
        Ok(Some(self.sections_wire()?.as_slice()) != state.baseline.get("sections"))
    }
    fn wire(&self) -> Result<Vec<u8>> {
        let mut out = vec![b'{'];
        for (key, value) in [
            ("id", &self.id),
            ("seed", &self.seed),
            ("template", &self.template),
        ] {
            field(&mut out, key, &json::quoted(value));
        }
        if !self.name.is_empty() {
            field(&mut out, "name", &json::quoted(&self.name));
        }
        field(&mut out, "status", &json::quoted(&self.status));
        field(&mut out, "revision", self.revision.to_string().as_bytes());
        field(&mut out, "sections", &self.sections_wire()?);
        field(
            &mut out,
            "children",
            &string_array(self.children.as_deref()),
        );
        if self
            .adopted_children
            .as_ref()
            .is_some_and(|a| !a.is_empty())
        {
            field(
                &mut out,
                "adoptedChildren",
                &string_array(self.adopted_children.as_deref()),
            );
        }
        for (key, value) in [
            ("outcome", &self.outcome),
            ("outcomeNote", &self.outcome_note),
            ("reviewedBy", &self.reviewed_by),
        ] {
            if !value.is_empty() {
                field(&mut out, key, &json::quoted(value));
            }
        }
        field(&mut out, "createdAt", &json::quoted(&self.created_at));
        field(&mut out, "updatedAt", &json::quoted(&self.updated_at));
        out.push(b'}');
        Ok(out)
    }
    pub fn marshal(&self) -> Result<Vec<u8>> {
        let current = self.wire()?;
        match &self.state {
            Some(s) => s.merge(&current, KNOWN),
            None => Ok(current),
        }
    }
    pub fn children(&self) -> &[String] {
        self.children.as_deref().unwrap_or_default()
    }
    pub fn adopted(&self) -> &[String] {
        self.adopted_children.as_deref().unwrap_or_default()
    }
}
impl Record for Plan {
    fn from_line(line: &[u8]) -> Option<Self> {
        if !json::is_valid(line) {
            return None;
        }
        let original = json::parse_object(line).ok()?;
        let sections = original
            .get("sections")
            .and_then(|r| json::Value::decode(r).ok())
            .and_then(json::Value::into_object);
        let section_order = original
            .get("sections")
            .and_then(|r| json::parse_object(r).ok())
            .map(|m| m.keys().to_vec())
            .unwrap_or_default();
        let mut plan = Self {
            id: string(original.get("id")),
            seed: string(original.get("seed")),
            template: string(original.get("template")),
            name: string(original.get("name")),
            status: string(original.get("status")),
            revision: original
                .get("revision")
                .and_then(|r| std::str::from_utf8(r).ok())
                .and_then(|r| r.parse().ok())
                .unwrap_or_default(),
            sections,
            children: strings(original.get("children")),
            adopted_children: strings(original.get("adoptedChildren")),
            outcome: string(original.get("outcome")),
            outcome_note: string(original.get("outcomeNote")),
            reviewed_by: string(original.get("reviewedBy")),
            created_at: string(original.get("createdAt")),
            updated_at: string(original.get("updatedAt")),
            section_order,
            state: None,
        };
        let baseline = json::parse_object(&plan.wire().ok()?).ok()?;
        plan.state = Some(State { original, baseline });
        Some(plan)
    }
    fn record_id(&self) -> &str {
        &self.id
    }
    fn to_line(&self) -> Result<Vec<u8>> {
        let raw = self.marshal()?;
        json::reformat(&raw, json::Format::MARSHAL).map_err(|e| Error::Encode {
            message: if json::exceeds_max_depth(&raw) {
                json::marshaler_too_deep("model.Plan")
            } else {
                format!("json: error calling MarshalJSON: {e}")
            },
        })
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn read(raw: &str) -> Plan {
        Plan::from_line(raw.as_bytes()).unwrap()
    }
    #[test]
    fn new_plan_uses_go_wire_order_and_nil_slices() {
        let p = Plan::default();
        assert_eq!(p.marshal().unwrap(),br#"{"id":"","seed":"","template":"","status":"","revision":0,"sections":null,"children":null,"createdAt":"","updatedAt":""}"#);
    }
    #[test]
    fn raw_unknown_and_wrong_typed_values_survive_a_rename() {
        let mut p = read(
            r#"{"id":"pl-a","revision":"keep","sections":{"n":9007199254740993},"unknown":{"n":1e400},"name":"old"}"#,
        );
        p.name = "new".into();
        let raw = p.marshal().unwrap();
        assert!(raw.starts_with(br#"{"id":"pl-a","revision":"keep","sections":{"n":9007199254740993},"unknown":{"n":1e400},"name":"new""#));
        assert!(!p.sections_changed().unwrap());
    }
    #[test]
    fn section_change_reencodes_all_sections_in_declaration_order() {
        let mut p = read(r#"{"sections":{"z":9007199254740993,"a":"old"}}"#);
        p.sections
            .as_mut()
            .unwrap()
            .insert("a".into(), json::Value::String("new".into()));
        assert_eq!(
            p.sections_wire().unwrap(),
            br#"{"z":9007199254740992,"a":"new"}"#
        );
        assert!(p.sections_changed().unwrap());
    }
    #[test]
    fn new_sections_append_in_sorted_order() {
        let mut p = read(r#"{"sections":{"z":0}}"#);
        let m = p.sections.as_mut().unwrap();
        m.insert("b".into(), json::Value::Null);
        m.insert("a".into(), json::Value::Bool(true));
        assert_eq!(p.sections_wire().unwrap(), br#"{"z":0,"a":true,"b":null}"#);
    }
    #[test]
    fn explicit_section_order_controls_new_records() {
        let p = Plan {
            sections: Some(std::collections::BTreeMap::from([
                ("a".into(), json::Value::Null),
                ("b".into(), json::Value::Bool(true)),
            ])),
            section_order: vec!["b".into(), "b".into(), "missing".into()],
            ..Plan::default()
        };
        assert_eq!(p.section_keys(), ["b", "a"]);
    }
    #[test]
    fn nil_empty_and_wrong_typed_arrays_match_go() {
        let p = read(r#"{"children":["a",4,null],"adoptedChildren":[],"sections":null}"#);
        assert_eq!(p.children(), ["a", "", ""]);
        assert_eq!(p.adopted(), Vec::<String>::new());
        assert!(p
            .marshal()
            .unwrap()
            .starts_with(br#"{"children":["a",4,null],"adoptedChildren":[],"sections":null"#));
    }
    #[test]
    fn malformed_records_are_skipped_but_unknown_out_of_range_numbers_stay_raw() {
        for raw in ["null", "[]", "not json", r#"{"sections":{"n":1e400}}"#] {
            assert!(Plan::from_line(raw.as_bytes()).is_none());
        }
        assert!(Plan::from_line(br#"{"unknown":1e400}"#).is_some());
    }
    #[test]
    fn repeated_keys_keep_first_position_and_last_raw_value() {
        let p = read(r#"{"id":"old","z":1,"id":"new","sections":{"b":1,"b":2}}"#);
        assert!(p
            .marshal()
            .unwrap()
            .starts_with(br#"{"id":"new","z":1,"sections":{"b":1,"b":2}"#));
    }
    #[test]
    fn removing_an_optional_field_removes_its_original_member() {
        let mut p = read(r#"{"outcome":"success","reviewedBy":null}"#);
        p.outcome.clear();
        assert!(!String::from_utf8(p.marshal().unwrap())
            .unwrap()
            .contains("outcome"));
        assert!(String::from_utf8(p.marshal().unwrap())
            .unwrap()
            .contains("\"reviewedBy\":null"));
    }
    #[test]
    fn invalid_encoding_stays_raw_until_sections_change() {
        let raw = b"{\"sections\":{\"x\":\"\xff\",\"y\":\"\\ud800\"}}";
        let mut p = Plan::from_line(raw).unwrap();
        assert!(p.marshal().unwrap().windows(1).any(|b| b == b"\xff"));
        p.sections
            .as_mut()
            .unwrap()
            .insert("z".into(), json::Value::Null);
        let encoded = p.sections_wire().unwrap();
        assert_eq!(encoded, "{\"x\":\"�\",\"y\":\"�\",\"z\":null}".as_bytes());
    }
    fn streamed(depth: usize, sections: bool) -> (tempfile::TempDir, std::path::PathBuf) {
        use std::io::Write;
        let temp = if let Some(parent) = std::env::var_os("LUUNA_MISSIONS_TEST_DISK_DIR") {
            std::fs::create_dir_all(&parent).unwrap();
            tempfile::tempdir_in(parent).unwrap()
        } else {
            tempfile::tempdir().unwrap()
        };
        let dir = temp.path().join(".luuna/missions");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("config.yaml"), b"project: demo\nversion: \"1\"\n").unwrap();
        let path = dir.join("input.json");
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(if sections {
            b"{\"sections\":{\"x\":"
        } else {
            b"{\"unknown\":"
        })
        .unwrap();
        for _ in 0..depth {
            f.write_all(b"[").unwrap();
        }
        f.write_all(b"0").unwrap();
        for _ in 0..depth {
            f.write_all(b"]").unwrap();
        }
        f.write_all(if sections { b"}}" } else { b"}" }).unwrap();
        (temp, path)
    }
    #[test]
    fn deeply_nested_sections_are_safe_on_a_default_stack() {
        let (_t, path) = streamed(1000, true);
        std::thread::Builder::new()
            .stack_size(2 * 1024 * 1024)
            .spawn(move || {
                let bytes = std::fs::read(path).unwrap();
                let p = Plan::from_line(&bytes).unwrap();
                let copy = p.clone();
                assert!(copy.marshal().is_ok());
                drop(copy);
                drop(p);
            })
            .unwrap()
            .join()
            .unwrap();
    }
    #[test]
    fn deep_unknown_raw_values_stay_safe_and_unchanged() {
        let (_t, path) = streamed(9000, false);
        std::thread::Builder::new()
            .stack_size(2 * 1024 * 1024)
            .spawn(move || {
                let bytes = std::fs::read(path).unwrap();
                let p = Plan::from_line(&bytes).unwrap();
                let output = p.marshal().unwrap();
                assert!(output.starts_with(&bytes[..bytes.len() - 1]));
            })
            .unwrap()
            .join()
            .unwrap();
    }
    #[test]
    fn nesting_beyond_go_limit_is_rejected_iteratively() {
        let (_t, path) = streamed(10001, true);
        let bytes = std::fs::read(path).unwrap();
        assert!(Plan::from_line(&bytes).is_none());
    }
}
