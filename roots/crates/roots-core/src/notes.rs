//! Agent artifact attachments under `.luuna/roots/agent/notes/`.

use crate::boundary::{agent_write_exclusive, is_inside, real_or_self};
use crate::config::load_config;
use crate::errors::{RootsError, RootsResult};
use crate::events::{EventFields, append_event, make_event};
use crate::ids::hex_of;
use crate::model::{NodeKind, NodeRecord, NodeStatus, QuestionStatus};
use crate::paths::RootsPaths;
use crate::questions::{read_questions, reopen_with_finding};
use crate::slug::slugify;
use crate::tier::require_tier;
use crate::time::{iso_at, iso_now};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use time::OffsetDateTime;

pub const NOTE_MAX_BYTES: u64 = 1024 * 1024;
pub const FINDING_EXTS: [&str; 3] = ["md", "markdown", "txt"];

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct NoteEntry {
    pub name: String,
    pub path: String,
    pub bytes: u64,
}

#[derive(Clone, Debug)]
pub struct NoteInput<'a> {
    pub node: &'a NodeRecord,
    pub file: &'a str,
    pub cwd: &'a Path,
    pub name: Option<&'a str>,
    pub question: Option<&'a str>,
    pub by: &'a str,
    pub now: Option<OffsetDateTime>,
}

#[derive(Clone, Debug)]
pub struct NoteResult {
    pub note: NoteEntry,
    pub node: String,
    pub question: Option<String>,
}

#[must_use]
pub fn notes_dir(paths: &RootsPaths, node: &NodeRecord) -> PathBuf {
    paths.notes.join(hex_of(&node.id))
}

fn relative(paths: &RootsPaths, target: &Path) -> String {
    target
        .strip_prefix(&paths.root)
        .unwrap_or(target)
        .to_string_lossy()
        .into_owned()
}

pub fn list_notes(paths: &RootsPaths, node: &NodeRecord) -> Vec<NoteEntry> {
    let dir = notes_dir(paths, node);
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut notes = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                return None;
            }
            let metadata = entry.metadata().ok()?;
            metadata.is_file().then(|| NoteEntry {
                name,
                path: relative(paths, &entry.path()),
                bytes: metadata.len(),
            })
        })
        .collect::<Vec<_>>();
    notes.sort_by(|a, b| a.name.cmp(&b.name));
    notes
}

#[must_use]
pub fn note_ext(name: &str) -> String {
    let ext = Path::new(name)
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or("")
        .to_lowercase();
    if (1..=10).contains(&ext.len())
        && ext
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
    {
        ext
    } else {
        String::new()
    }
}

pub fn note_file_name(
    name: &str,
    now: OffsetDateTime,
    taken: &BTreeSet<String>,
    fallback_ext: &str,
) -> String {
    let own = note_ext(name);
    let ext = if own.is_empty() {
        fallback_ext.to_owned()
    } else {
        own.clone()
    };
    let stem = if own.is_empty() {
        name
    } else {
        name.strip_suffix(&format!(".{own}")).unwrap_or(name)
    };
    let base = format!("{}-{}", &iso_at(now)[..10], {
        let slug = slugify(stem);
        if slug.is_empty() {
            "note".to_owned()
        } else {
            slug
        }
    });
    let suffix = if ext.is_empty() {
        String::new()
    } else {
        format!(".{ext}")
    };
    for number in 1_u64.. {
        let candidate = if number == 1 {
            format!("{base}{suffix}")
        } else {
            format!("{base}-{number}{suffix}")
        };
        if !taken.contains(&candidate) {
            return candidate;
        }
    }
    unreachable!("u64 filename suffixes are not exhaustible")
}

fn absolute_from(cwd: &Path, file: &str) -> PathBuf {
    let source = Path::new(file);
    if source.is_absolute() {
        source.to_path_buf()
    } else {
        cwd.join(source)
    }
}

fn read_source(path: &Path) -> RootsResult<Vec<u8>> {
    let metadata = fs::metadata(path)
        .map_err(|_| RootsError::not_found(format!("no such file: {}", path.display())))?;
    if !metadata.is_file() {
        return Err(RootsError::usage(format!(
            "{} is not a regular file",
            path.display()
        )));
    }
    if metadata.len() > NOTE_MAX_BYTES {
        let mut detail = Map::new();
        detail.insert("bytes".into(), Value::Number(metadata.len().into()));
        detail.insert("limit".into(), Value::Number(NOTE_MAX_BYTES.into()));
        return Err(RootsError::validation(format!(
            "{} is {} bytes; notes are limited to {NOTE_MAX_BYTES} bytes (1 MiB)",
            path.display(),
            metadata.len()
        ))
        .with_detail(detail));
    }
    fs::read(path).map_err(|error| RootsError::error(error.to_string()))
}

fn check_target(node: &NodeRecord) -> RootsResult<()> {
    if node.kind != NodeKind::Idea {
        return Err(RootsError::validation(format!(
            "{} is a sprout; notes are attached to human ideas",
            node.id
        )));
    }
    if matches!(
        node.status,
        NodeStatus::Idea(crate::model::IdeaStatus::Composted)
    ) {
        return Err(RootsError::validation(format!("{} is composted", node.id)));
    }
    Ok(())
}

fn check_question(paths: &RootsPaths, node: &NodeRecord, id: &str) -> RootsResult<()> {
    let question = read_questions(paths)?
        .into_iter()
        .find(|question| question.id == id)
        .ok_or_else(|| RootsError::not_found(format!("no question {id}")))?;
    if question.node != node.id {
        return Err(RootsError::validation(format!(
            "{id} is a question on {}, not {}",
            question.node, node.id
        )));
    }
    if question.status != QuestionStatus::Delegated {
        return Err(RootsError::validation(format!(
            "{id} is {}; findings go only on questions the human handed to the agent ([a] in think)",
            match question.status {
                QuestionStatus::Open => "open",
                QuestionStatus::Answered => "answered",
                QuestionStatus::Dismissed => "dismissed",
                QuestionStatus::Snoozed => "snoozed",
                QuestionStatus::Delegated => "delegated",
            }
        )));
    }
    Ok(())
}

fn check_finding_name(name: &str) -> RootsResult<()> {
    if FINDING_EXTS.contains(&note_ext(name).as_str()) {
        Ok(())
    } else {
        Err(RootsError::validation(format!(
            "findings must be a text file (.{})",
            FINDING_EXTS.join(", .")
        )))
    }
}

pub fn attach_note(paths: &RootsPaths, input: NoteInput<'_>) -> RootsResult<NoteResult> {
    if !input.by.starts_with("agent:") {
        return Err(RootsError::guard("only agents attach notes"));
    }
    check_target(input.node)?;
    require_tier(
        &load_config(paths)?,
        1,
        "`luuna-roots note`",
        Some(input.node),
    )?;
    let source = absolute_from(input.cwd, input.file);
    if let Some(question) = input.question {
        check_question(paths, input.node, question)?;
    }
    if is_inside(&paths.dir, &source) || is_inside(&paths.dir, real_or_self(&source)) {
        return Err(RootsError::usage(
            "a note is an outside artifact; files under .luuna/roots/ cannot be attached",
        ));
    }
    let data = read_source(&source)?;
    let now = input.now.unwrap_or_else(OffsetDateTime::now_utc);
    let taken = list_notes(paths, input.node)
        .into_iter()
        .map(|note| note.name)
        .collect::<BTreeSet<_>>();
    let fallback = source
        .file_name()
        .and_then(|value| value.to_str())
        .map(note_ext)
        .unwrap_or_default();
    let display = input
        .name
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .or_else(|| {
            source
                .file_name()
                .map(|value| value.to_string_lossy().into_owned())
        })
        .unwrap_or_default();
    let name = note_file_name(&display, now, &taken, &fallback);
    if input.question.is_some() {
        check_finding_name(&name)?;
    }
    let target = notes_dir(paths, input.node).join(&name);
    agent_write_exclusive(paths, &target, &data)?;
    let note = NoteEntry {
        name,
        path: relative(paths, &target),
        bytes: data.len() as u64,
    };
    let at = input.now.map(iso_at).unwrap_or_else(iso_now);
    let reopened = input
        .question
        .map(|question| reopen_with_finding(paths, question, &note.path, input.by, &at))
        .transpose()?
        .unwrap_or(false);
    let mut extra = Map::new();
    extra.insert("file".into(), Value::String(note.path.clone()));
    extra.insert("bytes".into(), Value::Number(note.bytes.into()));
    extra.insert(
        "hash".into(),
        Value::String(format!("sha256:{:x}", Sha256::digest(&data))),
    );
    if reopened {
        if let Some(question) = input.question {
            extra.insert("question".into(), Value::String(question.to_owned()));
        }
    }
    append_event(
        paths,
        &make_event(
            "note",
            input.by,
            EventFields {
                at: Some(at),
                node: Some(input.node.id.clone()),
                extra,
                ..Default::default()
            },
        ),
    )?;
    Ok(NoteResult {
        note,
        node: input.node.id.clone(),
        question: reopened.then(|| input.question.unwrap_or_default().to_owned()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn names_sanitize_and_collision_suffix() {
        let now = OffsetDateTime::UNIX_EPOCH;
        assert_eq!(
            note_file_name("../../etc/passwd", now, &BTreeSet::new(), ""),
            "1970-01-01-etc-passwd"
        );
        assert_eq!(note_ext("x.tar.gz"), "gz");
    }
}
