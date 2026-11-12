//! Read-only Missions and Atlas integration readers.
//!
//! Roots never owns either store.  These helpers deliberately keep the
//! JavaScript reader's permissive JSONL behaviour: malformed records are
//! ignored, duplicate ids are last-wins, and unknown fields remain harmless.

use crate::model::{Graph, IdeaStatus, NodeKind, NodeRecord, NodeStatus};
use crate::store::{dedup_by, read_jsonl_values};
use serde::Serialize;
use serde_json::Value;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

pub const MISSIONS_DIR: &str = ".luuna/missions";
pub const ATLAS_EXPERTISE_DIR: &str = ".luuna/atlas/expertise";

#[derive(Clone, Debug)]
pub struct MissionsIssue {
    pub id: String,
    pub title: String,
    pub status: String,
    pub value: serde_json::Map<String, Value>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct LinkedIssue {
    pub id: String,
    pub title: String,
    pub status: String,
    pub closed: bool,
    pub via: String,
}

#[derive(Clone, Debug)]
pub struct ReadyToBuild {
    pub node: NodeRecord,
    pub issues: Vec<LinkedIssue>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct AtlasLearning {
    pub id: Option<String>,
    pub domain: String,
    #[serde(rename = "type")]
    pub record_type: String,
    pub summary: String,
}

#[derive(Clone, Debug)]
struct AtlasRecord {
    learning: AtlasLearning,
    cites: Vec<String>,
}

fn value_string(value: Option<&Value>) -> String {
    value
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .unwrap_or_default()
}

fn token_char(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')
}

/// Extract exact, lowercase Roots ids in first-seen order.
#[must_use]
pub fn cited_idea_ids(text: &str) -> Vec<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    let mut index = 0;
    while index + 2 <= bytes.len() {
        let starts = bytes.get(index) == Some(&b'r') && bytes.get(index + 1) == Some(&b'-');
        let left_clear = index == 0 || !token_char(bytes[index - 1]);
        if !starts || !left_clear {
            index += 1;
            continue;
        }
        let mut end = index + 2;
        while end < bytes.len()
            && (bytes[end].is_ascii_digit() || matches!(bytes[end], b'a'..=b'f'))
        {
            end += 1;
        }
        let hex_len = end - (index + 2);
        let right_clear = end == bytes.len() || !token_char(bytes[end]);
        if (4..=8).contains(&hex_len) && right_clear {
            let id = &text[index..end];
            if seen.insert(id.to_owned()) {
                out.push(id.to_owned());
            }
            index = end;
        } else {
            index += 1;
        }
    }
    out
}

fn intent_list(value: Option<&Value>) -> Option<Vec<String>> {
    match value {
        Some(Value::String(text)) => Some(cited_idea_ids(text)),
        Some(Value::Array(values)) => Some(
            values
                .iter()
                .filter_map(Value::as_str)
                .flat_map(cited_idea_ids)
                .collect(),
        ),
        _ => None,
    }
}

fn explicit_intent(issue: &MissionsIssue) -> Option<Vec<String>> {
    if let Some(intent) = intent_list(issue.value.get("intent")) {
        return Some(intent);
    }
    issue
        .value
        .get("extensions")
        .and_then(Value::as_object)
        .and_then(|extension| intent_list(extension.get("intent")))
}

fn issue_text(issue: &MissionsIssue) -> String {
    let labels = issue
        .value
        .get("labels")
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default();
    [
        issue.title.as_str(),
        issue
            .value
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or_default(),
        labels.as_str(),
    ]
    .join("\n")
}

/// Read Mission issues from the project-owned Missions store.
pub fn read_missions_issues(root: &Path) -> Vec<MissionsIssue> {
    let file = root.join(MISSIONS_DIR).join("issues.jsonl");
    let Ok(rows) = read_jsonl_values(file) else {
        return Vec::new();
    };
    let rows = rows
        .into_iter()
        .filter_map(|value| value.as_object().cloned())
        .filter(|row| {
            row.get("id")
                .and_then(Value::as_str)
                .is_some_and(|id| !id.is_empty())
        })
        .collect::<Vec<_>>();
    dedup_by(rows, |row| {
        row.get("id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned()
    })
    .into_iter()
    .map(|value| MissionsIssue {
        id: value_string(value.get("id")),
        title: value_string(value.get("title")),
        status: value
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("open")
            .to_owned(),
        value,
    })
    .collect()
}

/// The ids an issue explicitly or textually links to, and how that link was found.
#[must_use]
pub fn issue_links(issue: &MissionsIssue) -> (Vec<String>, &'static str) {
    if let Some(ids) = explicit_intent(issue) {
        return (ids, "intent");
    }
    (cited_idea_ids(&issue_text(issue)), "mention")
}

#[must_use]
pub fn linked_issues(issues: &[MissionsIssue], idea_id: &str) -> Vec<LinkedIssue> {
    issues
        .iter()
        .filter_map(|issue| {
            let (ids, via) = issue_links(issue);
            ids.contains(&idea_id.to_owned()).then(|| LinkedIssue {
                id: issue.id.clone(),
                title: issue.title.clone(),
                status: issue.status.clone(),
                closed: issue.status == "closed",
                via: via.to_owned(),
            })
        })
        .collect()
}

#[must_use]
pub fn ready_to_mark_built(graph: &Graph, issues: &[MissionsIssue]) -> Vec<ReadyToBuild> {
    if issues.is_empty() {
        return Vec::new();
    }
    graph
        .nodes
        .iter()
        .filter(|node| {
            node.kind == NodeKind::Idea
                && matches!(node.status, NodeStatus::Idea(IdeaStatus::Committed))
        })
        .filter_map(|node| {
            let linked = linked_issues(issues, &node.id);
            (!linked.is_empty() && linked.iter().all(|issue| issue.closed)).then(|| ReadyToBuild {
                node: node.clone(),
                issues: linked,
            })
        })
        .collect()
}

fn strings_in(value: &Value, depth: usize, output: &mut Vec<String>) {
    match value {
        Value::String(text) => output.push(text.clone()),
        Value::Array(values) if depth < 6 => {
            for value in values {
                strings_in(value, depth + 1, output);
            }
        }
        Value::Object(values) if depth < 6 => {
            for value in values.values() {
                strings_in(value, depth + 1, output);
            }
        }
        _ => {}
    }
}

fn short_line(text: &str) -> String {
    let line = text
        .lines()
        .find(|line| !line.trim().is_empty())
        .map(str::trim)
        .unwrap_or_default();
    let chars = line.chars().collect::<Vec<_>>();
    if chars.len() <= 100 {
        line.to_owned()
    } else {
        format!("{}…", chars[..99].iter().collect::<String>())
    }
}

fn atlas_domain_files(root: &Path) -> Vec<PathBuf> {
    let directory = root.join(ATLAS_EXPERTISE_DIR);
    let Ok(entries) = fs::read_dir(directory) else {
        return Vec::new();
    };
    let mut files = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "jsonl")
        })
        .collect::<Vec<_>>();
    files.sort();
    files
}

fn atlas_record_key(row: &serde_json::Map<String, Value>) -> String {
    row.get("id")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| serde_json::to_string(row).unwrap_or_default())
}

fn read_atlas_domain(file: &Path) -> Vec<AtlasRecord> {
    let domain = file
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_owned();
    let Ok(rows) = read_jsonl_values(file) else {
        return Vec::new();
    };
    let rows = rows
        .into_iter()
        .filter_map(|value| value.as_object().cloned())
        .filter(|row| !row.contains_key("archived_at"))
        .collect::<Vec<_>>();
    dedup_by(rows, atlas_record_key)
        .into_iter()
        .map(|row| {
            let raw = ["name", "title", "description", "content", "rationale"]
                .iter()
                .find_map(|key| row.get(*key).and_then(Value::as_str))
                .unwrap_or_default();
            let mut strings = Vec::new();
            strings_in(&Value::Object(row.clone()), 0, &mut strings);
            AtlasRecord {
                learning: AtlasLearning {
                    id: row.get("id").and_then(Value::as_str).map(ToOwned::to_owned),
                    domain: domain.clone(),
                    record_type: value_string(row.get("type")).if_empty("?"),
                    summary: short_line(raw),
                },
                cites: cited_idea_ids(&strings.join("\n")),
            }
        })
        .collect()
}

trait IfEmpty {
    fn if_empty(self, fallback: &str) -> Self;
}

impl IfEmpty for String {
    fn if_empty(self, fallback: &str) -> Self {
        if self.is_empty() {
            fallback.to_owned()
        } else {
            self
        }
    }
}

/// Atlas expertise records that cite any Roots idea, grouped by sorted domain file.
#[must_use]
pub fn read_atlas_citing_records(root: &Path) -> Vec<(AtlasLearning, Vec<String>)> {
    atlas_domain_files(root)
        .iter()
        .flat_map(|file| read_atlas_domain(file))
        .filter(|record| !record.cites.is_empty())
        .map(|record| (record.learning, record.cites))
        .collect()
}

#[must_use]
pub fn atlas_learnings_for(root: &Path, idea_id: &str) -> Vec<AtlasLearning> {
    read_atlas_citing_records(root)
        .into_iter()
        .filter(|(_, cites)| cites.iter().any(|id| id == idea_id))
        .map(|(learning, _)| learning)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    fn sandbox(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "roots-integrations-{name}-{}",
            crate::ids::random_hex(8)
        ));
        fs::create_dir_all(&root).expect("create sandbox");
        root
    }

    #[test]
    fn finds_only_exact_idea_id_tokens() {
        assert_eq!(
            cited_idea_ids("fixes r-a1b2; r-c3d4 and r-a1b2 again"),
            ["r-a1b2", "r-c3d4"]
        );
        assert!(cited_idea_ids("sr-a1b2 r-a1b2-x r-A1B2").is_empty());
    }

    #[test]
    fn reads_missions_and_atlas_without_mutating_their_project_owned_stores() {
        let root = sandbox("project-readers");
        let missions = root.join(MISSIONS_DIR);
        let atlas = root.join(ATLAS_EXPERTISE_DIR);
        fs::create_dir_all(&missions).expect("missions directory");
        fs::create_dir_all(&atlas).expect("atlas directory");
        let issues_file = missions.join("issues.jsonl");
        let expertise_file = atlas.join("delivery.jsonl");
        let issue_rows = concat!(
            "{\"id\":\"m-1\",\"title\":\"old title r-a1b2\",\"status\":\"closed\"}\n",
            "not json\n",
            "{\"id\":\"m-1\",\"title\":\"current issue\",\"status\":\"in_progress\",\"intent\":[\"r-b2c3\"]}\n",
            "{\"id\":\"m-2\",\"title\":\"mentions r-a1b2\",\"description\":\"ship it\",\"status\":\"closed\"}\n",
            "{\"id\":\"m-3\",\"title\":\"direct intent wins\",\"description\":\"r-a1b2\",\"intent\":[\"r-c3d4\"],\"status\":\"closed\"}\n",
            "{\"id\":\"m-4\",\"title\":\"extension intent\",\"extensions\":{\"intent\":[\"r-a1b2\"]},\"status\":\"closed\"}\n"
        );
        let atlas_rows = concat!(
            "{\"id\":\"a-1\",\"type\":\"lesson\",\"name\":\"old r-a1b2\"}\n",
            "{\"id\":\"a-1\",\"type\":\"lesson\",\"description\":\"Latest finding r-a1b2\"}\n",
            "{\"id\":\"a-hidden\",\"type\":\"lesson\",\"content\":\"r-a1b2\",\"archived_at\":\"2026-01-01\"}\n",
            "{\"id\":\"a-near\",\"type\":\"lesson\",\"content\":\"r-a1b2-x is not an exact citation\"}\n"
        );
        fs::write(&issues_file, issue_rows).expect("write issues");
        fs::write(&expertise_file, atlas_rows).expect("write expertise");

        let issues_before = fs::read_to_string(&issues_file).expect("read issues before");
        let atlas_before = fs::read_to_string(&expertise_file).expect("read expertise before");
        let issues = read_missions_issues(&root);
        let linked = linked_issues(&issues, "r-a1b2");
        assert_eq!(issues.len(), 4, "last issue row wins without reordering");
        assert_eq!(linked.len(), 2);
        assert_eq!(linked[0].id, "m-2");
        assert_eq!(linked[0].via, "mention");
        assert!(linked[0].closed);
        assert_eq!(linked[1].id, "m-4");
        assert_eq!(linked[1].via, "intent");
        assert!(linked[1].closed);
        assert!(
            !linked.iter().any(|issue| issue.id == "m-3"),
            "direct intent is authoritative over title/description mentions"
        );

        let learnings = atlas_learnings_for(&root, "r-a1b2");
        assert_eq!(learnings.len(), 1);
        assert_eq!(learnings[0].domain, "delivery");
        assert_eq!(learnings[0].id.as_deref(), Some("a-1"));
        assert_eq!(learnings[0].summary, "Latest finding r-a1b2");
        assert_eq!(
            fs::read_to_string(&issues_file).expect("read issues after"),
            issues_before
        );
        assert_eq!(
            fs::read_to_string(&expertise_file).expect("read expertise after"),
            atlas_before
        );
        fs::remove_dir_all(root).expect("clean sandbox");
    }
}
