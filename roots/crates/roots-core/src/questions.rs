//! Agent question persistence and the `ask` mutation policy.

use crate::config::load_config;
use crate::errors::{RootsError, RootsResult};
use crate::events::{EventFields, append_event, make_event, read_events};
use crate::ids::{IdPrefix, generate_id, hex_set};
use crate::model::{
    NodeKind, NodeRecord, NodeStatus, QuestionFinding, QuestionRecord, QuestionStatus,
};
use crate::paths::RootsPaths;
use crate::store::{read_table, update_table};
use crate::tier::require_tier;
use crate::time::{iso_at, iso_now};
use serde_json::{Map, Value};
use time::OffsetDateTime;

pub const QUESTION_MAX_LENGTH: usize = 280;

pub fn read_questions(paths: &RootsPaths) -> RootsResult<Vec<QuestionRecord>> {
    read_table(&paths.questions)
}

#[must_use]
pub fn clean_question(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[must_use]
pub fn question_fingerprint(text: &str) -> String {
    clean_question(text)
        .chars()
        .flat_map(char::to_lowercase)
        .map(|character| {
            if character.is_alphanumeric() {
                character
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[must_use]
pub fn open_agent_questions<'a>(rows: &'a [QuestionRecord], node: &str) -> Vec<&'a QuestionRecord> {
    rows.iter()
        .filter(|question| {
            question.node == node
                && question.by.starts_with("agent:")
                && matches!(
                    question.status,
                    QuestionStatus::Open | QuestionStatus::Snoozed
                )
        })
        .collect()
}

fn validation_with_number(message: String, key: &str, value: u64) -> RootsError {
    let mut detail = Map::new();
    detail.insert(key.to_owned(), Value::Number(value.into()));
    RootsError::validation(message).with_detail(detail)
}

pub fn check_question_text(raw: &str) -> RootsResult<String> {
    let text = clean_question(raw);
    if text.is_empty() {
        return Err(RootsError::usage("the question is empty"));
    }
    let length = text.encode_utf16().count();
    if length > QUESTION_MAX_LENGTH {
        return Err(RootsError::validation(format!(
            "question is {length} characters; the limit is {QUESTION_MAX_LENGTH}. Ask one specific thing"
        )));
    }
    if !text.ends_with('?') {
        return Err(RootsError::validation("a question must end with \"?\""));
    }
    Ok(text)
}

fn check_target(node: &NodeRecord) -> RootsResult<()> {
    if node.kind != NodeKind::Idea {
        return Err(RootsError::validation(format!(
            "{} is a sprout; questions are asked about human ideas",
            node.id
        )));
    }
    if matches!(
        node.status,
        NodeStatus::Idea(crate::model::IdeaStatus::Composted)
    ) {
        return Err(RootsError::validation(format!(
            "{} is composted; there is nothing left to ask about",
            node.id
        )));
    }
    Ok(())
}

fn duplicate<'a>(rows: &'a [QuestionRecord], node: &str, text: &str) -> Option<&'a QuestionRecord> {
    let fingerprint = question_fingerprint(text);
    rows.iter().find(|question| {
        question.node == node
            && matches!(
                question.status,
                QuestionStatus::Open | QuestionStatus::Snoozed | QuestionStatus::Dismissed
            )
            && question_fingerprint(&question.text) == fingerprint
    })
}

/// A requested session is advisory: it is attached only if it is currently a
/// started, un-ended think session on this exact node.
#[must_use]
pub fn live_session(paths: &RootsPaths, node: &str, session: Option<&str>) -> Option<String> {
    let session = session?.trim();
    if session.is_empty() {
        return None;
    }
    let mut started = false;
    let Ok(events) = read_events(paths) else {
        return None;
    };
    for event in events {
        let event_session = event.extra.get("session").and_then(Value::as_str);
        if event_session != Some(session) || event.node.as_deref() != Some(node) {
            continue;
        }
        if event.event_type == "session.start" {
            started = true;
        }
        if event.event_type == "session.end" {
            return None;
        }
    }
    started.then(|| session.to_owned())
}

#[derive(Clone, Debug)]
pub struct AskInput<'a> {
    pub node: &'a NodeRecord,
    pub text: &'a str,
    pub by: &'a str,
    pub session: Option<&'a str>,
    pub now: Option<OffsetDateTime>,
}

#[derive(Clone, Debug)]
pub struct AskResult {
    pub question: QuestionRecord,
    pub session: Option<String>,
}

pub fn ask_agent_question(paths: &RootsPaths, input: AskInput<'_>) -> RootsResult<AskResult> {
    if !input.by.starts_with("agent:") {
        return Err(RootsError::guard("only agents ask questions"));
    }
    let config = load_config(paths)?;
    check_target(input.node)?;
    require_tier(&config, 1, "`luuna-roots ask`", Some(input.node))?;
    let text = check_question_text(input.text)?;
    let cap = config.limits.questions_per_session;
    let first_rows = read_questions(paths)?;
    if let Some(existing) = duplicate(&first_rows, &input.node.id, &text) {
        let why = if existing.status == QuestionStatus::Dismissed {
            "the human dismissed it; do not ask it again".to_owned()
        } else {
            format!(
                "it is {}",
                match existing.status {
                    QuestionStatus::Open => "open",
                    QuestionStatus::Snoozed => "snoozed",
                    QuestionStatus::Answered => "answered",
                    QuestionStatus::Dismissed => "dismissed",
                    QuestionStatus::Delegated => "delegated",
                }
            )
        };
        let mut detail = Map::new();
        detail.insert("duplicate".into(), Value::String(existing.id.clone()));
        return Err(RootsError::validation(format!(
            "duplicate of {} (\"{}\"): {why}",
            existing.id, existing.text
        ))
        .with_detail(detail));
    }
    let existing_open = open_agent_questions(&first_rows, &input.node.id).len() as u64;
    if existing_open >= cap {
        return Err(validation_with_number(
            format!("{} already has {existing_open} open agent question{} (limit {cap}, limits.questionsPerSession); wait for the human to answer or dismiss them", input.node.id, if existing_open == 1 { "" } else { "s" }),
            "open", existing_open,
        ).with_detail({ let mut d = Map::new(); d.insert("open".into(), Value::Number(existing_open.into())); d.insert("cap".into(), Value::Number(cap.into())); d }));
    }
    let session = live_session(paths, &input.node.id, input.session);
    let at = input.now.map(iso_at).unwrap_or_else(iso_now);
    let question = update_table::<QuestionRecord, _, _>(&paths.questions, |mut rows| {
        let open = open_agent_questions(&rows, &input.node.id).len() as u64;
        if open >= cap {
            let mut detail = Map::new();
            detail.insert("open".into(), Value::Number(open.into()));
            detail.insert("cap".into(), Value::Number(cap.into()));
            return Err(RootsError::validation(format!("{} already has {open} open agent question{} (limit {cap}, limits.questionsPerSession); wait for the human to answer or dismiss them", input.node.id, if open == 1 { "" } else { "s" })).with_detail(detail));
        }
        if let Some(existing) = duplicate(&rows, &input.node.id, &text) {
            let mut detail = Map::new();
            detail.insert("duplicate".into(), Value::String(existing.id.clone()));
            let why = if existing.status == QuestionStatus::Dismissed {
                "the human dismissed it; do not ask it again".to_owned()
            } else {
                format!(
                    "it is {}",
                    match existing.status {
                        QuestionStatus::Open => "open",
                        QuestionStatus::Snoozed => "snoozed",
                        QuestionStatus::Answered => "answered",
                        QuestionStatus::Dismissed => "dismissed",
                        QuestionStatus::Delegated => "delegated",
                    }
                )
            };
            return Err(RootsError::validation(format!(
                "duplicate of {} (\"{}\"): {why}",
                existing.id, existing.text
            ))
            .with_detail(detail));
        }
        let taken = hex_set(rows.iter().map(|row| row.id.as_str()));
        let question = QuestionRecord {
            id: generate_id(IdPrefix::Question, &taken),
            node: input.node.id.clone(),
            text: text.clone(),
            by: input.by.to_owned(),
            status: QuestionStatus::Open,
            created_at: at.clone(),
            r#ref: None,
            answered_by: None,
            answered_at: None,
            dismissed_by: None,
            dismissed_at: None,
            delegated_by: None,
            delegated_at: None,
            findings: None,
            snoozed_until: None,
            session: session.clone(),
        };
        rows.push(question.clone());
        Ok((rows, true, question))
    })?;
    let mut extra = Map::new();
    extra.insert("question".into(), Value::String(question.id.clone()));
    if let Some(session) = &session {
        extra.insert("session".into(), Value::String(session.clone()));
    }
    append_event(
        paths,
        &make_event(
            "ask",
            input.by,
            EventFields {
                at: Some(at),
                node: Some(input.node.id.clone()),
                extra,
                ..Default::default()
            },
        ),
    )?;
    Ok(AskResult { question, session })
}

/// Attach a finding to a delegated question, reopening it for the human.
/// This is deliberately race-safe: a concurrent human action turns it into a
/// no-op, exactly as the original implementation does.
pub fn reopen_with_finding(
    paths: &RootsPaths,
    id: &str,
    note: &str,
    by: &str,
    at: &str,
) -> RootsResult<bool> {
    update_table::<QuestionRecord, _, _>(&paths.questions, |mut rows| {
        let Some(question) = rows.iter_mut().find(|question| question.id == id) else {
            return Ok((rows, false, false));
        };
        if question.status != QuestionStatus::Delegated {
            return Ok((rows, false, false));
        }
        question.status = QuestionStatus::Open;
        question
            .findings
            .get_or_insert_with(Vec::new)
            .push(QuestionFinding {
                note: note.to_owned(),
                by: by.to_owned(),
                at: at.to_owned(),
            });
        Ok((rows, true, true))
    })
}
