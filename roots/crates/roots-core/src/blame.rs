//! Read-side, line-level question attribution for human idea prose.

use crate::model::{EventRecord, QuestionRecord};
use crate::prose::content_hash;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct DiffSpan {
    pub from: i64,
    pub to: i64,
    pub removed: i64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct BlameLine {
    pub n: usize,
    pub text: String,
    pub question: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BlameAnswer {
    pub question: String,
    pub text: String,
    pub asked_by: String,
    pub answered_by: String,
    pub at: String,
    pub session: Option<String>,
    pub lines: usize,
    pub placed: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct Blame {
    pub lines: Vec<BlameLine>,
    pub answers: Vec<BlameAnswer>,
    pub untracked: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AnswerBlock {
    pub question: Option<String>,
    pub text: String,
}

/// 12 hexadecimal SHA-256 digits for a trimmed line.
#[must_use]
pub fn line_hash(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.trim().as_bytes()))[..12].to_owned()
}

#[must_use]
pub fn split_lines(text: &str) -> Vec<String> {
    if text.is_empty() {
        return Vec::new();
    }
    let normalized = text.replace("\r\n", "\n");
    let mut lines = normalized
        .split('\n')
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    if lines.last().is_some_and(String::is_empty) {
        let _ = lines.pop();
    }
    lines
}

fn value_i64(value: Option<&serde_json::Value>) -> Option<i64> {
    value.and_then(serde_json::Value::as_i64)
}

fn span_of(event: &EventRecord) -> Option<DiffSpan> {
    let span = event.extra.get("span")?.as_object()?;
    Some(DiffSpan {
        from: value_i64(span.get("from"))?,
        to: value_i64(span.get("to"))?,
        removed: value_i64(span.get("removed"))?,
    })
}

fn hashes_of(event: &EventRecord) -> Option<Vec<String>> {
    event.extra.get("lines")?.as_array().map(|values| {
        values
            .iter()
            .filter_map(serde_json::Value::as_str)
            .map(ToOwned::to_owned)
            .collect()
    })
}

fn string_extra(event: &EventRecord, key: &str) -> Option<String> {
    event
        .extra
        .get(key)
        .and_then(serde_json::Value::as_str)
        .map(ToOwned::to_owned)
}

/// Where a previous line ends after `span` is applied.
#[must_use]
pub fn shifted(n: i64, span: &DiffSpan) -> Vec<i64> {
    if n < span.from {
        return vec![n];
    }
    let old_end = span.from + span.removed - 1;
    if n > old_end {
        vec![n + span.to - span.from + 1 - span.removed]
    } else {
        Vec::new()
    }
}

/// Replay answer spans from one intact think session, with later answers winning.
#[must_use]
pub fn span_lines(answers: &[(String, DiffSpan)]) -> HashMap<i64, String> {
    let mut placed: Vec<(String, Vec<i64>)> = Vec::new();
    for (question, span) in answers {
        for (_, lines) in &mut placed {
            *lines = lines.iter().flat_map(|line| shifted(*line, span)).collect();
        }
        let own = (0..std::cmp::max(0, span.to - span.from + 1))
            .map(|offset| span.from + offset)
            .collect();
        placed.push((question.clone(), own));
    }
    let mut owner = HashMap::new();
    for (question, lines) in placed {
        for line in lines {
            owner.insert(line, question.clone());
        }
    }
    owner
}

/// Attribute the current prose lines to answer events without recording prose in JSONL.
#[must_use]
pub fn build_blame(
    node: &str,
    text: &str,
    events: &[EventRecord],
    questions: &[QuestionRecord],
) -> Blame {
    let hash = content_hash(text);
    let intact = events
        .iter()
        .filter(|event| {
            event.event_type == "session.end"
                && event.node.as_deref() == Some(node)
                && string_extra(event, "hash").as_deref() == Some(hash.as_str())
        })
        .filter_map(|event| string_extra(event, "session"))
        .collect::<HashSet<_>>();
    let mut owner = HashMap::<String, String>::new();
    let mut by_session: Vec<(String, Vec<(String, DiffSpan)>)> = Vec::new();
    let mut answers = Vec::new();

    for event in events {
        let question = string_extra(event, "question");
        if event.event_type != "answer" || event.node.as_deref() != Some(node) || question.is_none()
        {
            continue;
        }
        let question = question.unwrap_or_default();
        let asked = questions.iter().find(|row| row.id == question);
        let session = string_extra(event, "session");
        let placed = if let Some(hashes) = hashes_of(event) {
            for line in hashes {
                owner.insert(line, question.clone());
            }
            "lines"
        } else if let (Some(span), Some(session)) = (span_of(event), session.as_deref()) {
            if intact.contains(session) {
                if let Some((_, rows)) = by_session.iter_mut().find(|(id, _)| id == session) {
                    rows.push((question.clone(), span));
                } else {
                    by_session.push((session.to_owned(), vec![(question.clone(), span)]));
                }
                "span"
            } else {
                "none"
            }
        } else {
            "none"
        };
        answers.push(BlameAnswer {
            question,
            text: asked
                .map(|row| row.text.clone())
                .unwrap_or_else(|| "(question not found)".to_owned()),
            asked_by: asked
                .map(|row| row.by.clone())
                .unwrap_or_else(|| "unknown".to_owned()),
            answered_by: event.by.clone(),
            at: event.at.clone(),
            session,
            lines: 0,
            placed: placed.to_owned(),
        });
    }

    let mut by_line = HashMap::<i64, String>::new();
    for (_, session_answers) in by_session {
        for (line, question) in span_lines(&session_answers) {
            by_line.insert(line, question);
        }
    }
    let lines = split_lines(text)
        .into_iter()
        .enumerate()
        .map(|(index, line)| {
            let question = if line.trim().is_empty() {
                None
            } else {
                owner
                    .get(&line_hash(&line))
                    .cloned()
                    .or_else(|| by_line.get(&(index as i64 + 1)).cloned())
            };
            BlameLine {
                n: index + 1,
                text: line,
                question,
            }
        })
        .collect::<Vec<_>>();
    for answer in &mut answers {
        answer.lines = lines
            .iter()
            .filter(|line| line.question.as_deref() == Some(answer.question.as_str()))
            .count();
    }
    let untracked = answers
        .iter()
        .filter(|answer| answer.placed == "none")
        .count();
    Blame {
        lines,
        answers,
        untracked,
    }
}

/// Split prose body into runs owned by an answer; blank lines stay with the previous run.
#[must_use]
pub fn answer_blocks(blame: &Blame) -> Vec<AnswerBlock> {
    let Some(start) = blame
        .lines
        .iter()
        .position(|line| !line.text.trim().is_empty())
    else {
        return Vec::new();
    };
    let mut blocks: Vec<AnswerBlock> = Vec::new();
    for line in blame.lines.iter().skip(start + 1) {
        let blank = line.text.trim().is_empty();
        let need_new = blocks
            .last()
            .is_none_or(|current| !blank && current.question != line.question);
        if blank && blocks.is_empty() {
            continue;
        }
        if need_new {
            blocks.push(AnswerBlock {
                question: line.question.clone(),
                text: String::new(),
            });
        }
        let current = blocks.last_mut().expect("created above");
        if !current.text.is_empty() {
            current.text.push('\n');
        }
        current.text.push_str(&line.text);
    }
    for block in &mut blocks {
        block.text = block.text.trim_end().to_owned();
    }
    blocks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_hash_and_split_lines_follow_the_legacy_format() {
        assert_eq!(line_hash(" x "), line_hash("x"));
        assert_eq!(split_lines("a\r\nb\n"), ["a", "b"]);
    }

    #[test]
    fn replayed_spans_shift_older_lines() {
        let lines = span_lines(&[
            (
                "q-1".into(),
                DiffSpan {
                    from: 2,
                    to: 2,
                    removed: 0,
                },
            ),
            (
                "q-2".into(),
                DiffSpan {
                    from: 1,
                    to: 1,
                    removed: 0,
                },
            ),
        ]);
        assert_eq!(lines.get(&3).map(String::as_str), Some("q-1"));
        assert_eq!(lines.get(&1).map(String::as_str), Some("q-2"));
    }
}
