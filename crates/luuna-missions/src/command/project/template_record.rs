//! Go's Template/TemplateStep record-state merge, keeping raw unchanged members.
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
fn field(out: &mut Vec<u8>, key: &str, value: &[u8]) {
    if out.len() > 1 {
        out.push(b',');
    }
    out.extend(json::quoted(key));
    out.push(b':');
    out.extend_from_slice(value);
}
/// Borrowed JSON array elements, without decoding or recursively walking values.
pub(super) fn array(raw: &[u8]) -> Option<Vec<&[u8]>> {
    if raw.first() != Some(&b'[') || !json::is_valid(raw) {
        return None;
    }
    let mut result = Vec::new();
    let (mut start, mut i, mut depth) = (1, 1, 0usize);
    while i < raw.len() {
        match raw[i] {
            b'"' => {
                i = string_end(raw, i);
                continue;
            }
            b'[' | b'{' => depth += 1,
            b']' if depth == 0 => {
                let part = crate::store::trim_space(&raw[start..i]);
                if !part.is_empty() {
                    result.push(part);
                }
                return Some(result);
            }
            b']' | b'}' => depth -= 1,
            b',' if depth == 0 => {
                result.push(crate::store::trim_space(&raw[start..i]));
                start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    None
}
pub(super) fn string_end(raw: &[u8], start: usize) -> usize {
    let mut i = start + 1;
    while i < raw.len() {
        match raw[i] {
            b'\\' => i += 2,
            b'"' => return i + 1,
            _ => i += 1,
        }
    }
    raw.len()
}
#[derive(Debug, Clone, Default)]
pub(super) struct TemplateStep {
    pub title: String,
    pub ticket_type: String,
    pub priority: Option<i64>,
    pub plan_template: String,
    state: Option<State>,
}
impl TemplateStep {
    pub fn new(title: String, ticket_type: String, priority: i64) -> Self {
        Self {
            title,
            ticket_type,
            priority: Some(priority),
            ..Self::default()
        }
    }
    fn read(raw: &[u8]) -> Option<Self> {
        let original = json::parse_object(raw).ok()?;
        let mut s = Self {
            title: string(original.get("title")),
            ticket_type: string(original.get("type")),
            plan_template: string(original.get("plan_template")),
            priority: original.get("priority").filter(|v| *v != b"null").map(|v| {
                std::str::from_utf8(v)
                    .ok()
                    .and_then(|s| s.parse::<i64>().ok())
                    .unwrap_or(0)
            }),
            state: None,
        };
        let baseline = json::parse_object(&s.wire()).ok()?;
        s.state = Some(State { original, baseline });
        Some(s)
    }
    fn wire(&self) -> Vec<u8> {
        let mut out = vec![b'{'];
        field(&mut out, "title", &json::quoted(&self.title));
        if !self.ticket_type.is_empty() {
            field(&mut out, "type", &json::quoted(&self.ticket_type));
        }
        if let Some(p) = self.priority {
            field(&mut out, "priority", p.to_string().as_bytes());
        }
        if !self.plan_template.is_empty() {
            field(
                &mut out,
                "plan_template",
                &json::quoted(&self.plan_template),
            );
        }
        out.push(b'}');
        out
    }
    fn marshal(&self) -> Result<Vec<u8>> {
        let current = self.wire();
        match &self.state {
            Some(s) => s.merge(&current, &["title", "type", "priority", "plan_template"]),
            None => Ok(current),
        }
    }
}
#[derive(Debug, Clone, Default)]
pub(super) struct Template {
    pub id: String,
    pub name: String,
    pub steps: Option<Vec<TemplateStep>>,
    state: Option<State>,
}
impl Template {
    pub fn new(id: String, name: String) -> Self {
        Self {
            id,
            name,
            steps: Some(Vec::new()),
            state: None,
        }
    }
    pub fn steps(&self) -> &[TemplateStep] {
        self.steps.as_deref().unwrap_or_default()
    }
    fn wire(&self) -> Result<Vec<u8>> {
        let mut out = vec![b'{'];
        field(&mut out, "id", &json::quoted(&self.id));
        field(&mut out, "name", &json::quoted(&self.name));
        let steps = match &self.steps {
            None => b"null".to_vec(),
            Some(steps) => {
                let mut out = vec![b'['];
                for (i, s) in steps.iter().enumerate() {
                    if i > 0 {
                        out.push(b',');
                    }
                    out.extend(
                        json::reformat(&s.marshal()?, json::Format::MARSHAL).map_err(encoding)?,
                    );
                }
                out.push(b']');
                out
            }
        };
        field(&mut out, "steps", &steps);
        out.push(b'}');
        Ok(out)
    }
    pub fn marshal(&self) -> Result<Vec<u8>> {
        let current = self.wire()?;
        match &self.state {
            Some(s) => s.merge(&current, &["id", "name", "steps"]),
            None => Ok(current),
        }
    }
}
impl Record for Template {
    fn from_line(line: &[u8]) -> Option<Self> {
        if !json::is_valid(line) {
            return None;
        }
        let original = json::parse_object(line).ok()?;
        let steps = original.get("steps").and_then(array).map(|items| {
            items
                .into_iter()
                .map(|item| TemplateStep::read(item).unwrap_or_default())
                .collect()
        });
        let mut s = Self {
            id: string(original.get("id")),
            name: string(original.get("name")),
            steps,
            state: None,
        };
        let baseline = json::parse_object(&s.wire().ok()?).ok()?;
        s.state = Some(State { original, baseline });
        Some(s)
    }
    fn record_id(&self) -> &str {
        &self.id
    }
    fn to_line(&self) -> Result<Vec<u8>> {
        let raw = self.marshal()?;
        json::reformat(&raw, json::Format::MARSHAL).map_err(|e| Error::Encode {
            message: if json::exceeds_max_depth(&raw) {
                json::marshaler_too_deep("model.Template")
            } else {
                format!("json: error calling MarshalJSON: {e}")
            },
        })
    }
}
