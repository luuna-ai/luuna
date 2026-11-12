//! Advisory agent headings for live `flow` sessions.

use crate::config::RootsConfig;
use crate::errors::{RootsError, RootsResult};
use crate::events::{EventFields, append_event, make_event, read_events};
use crate::graph::read_graph;
use crate::ids::{IdPrefix, generate_id, hex_set};
use crate::model::{Citation, HeadingRecord, HeadingStatus, IdeaStatus, NodeKind, NodeStatus};
use crate::paths::RootsPaths;
use crate::prose::read_node_prose;
use crate::questions::read_questions;
use crate::store::{read_table, update_table};
use crate::tier::require_tier;
use crate::time::{iso_at, iso_now};
use serde_json::{Map, Value};
use std::collections::BTreeSet;
use time::OffsetDateTime;

pub const HEADING_MAX_LENGTH: usize = 280;
pub const HEADING_MAX_SENTENCES: usize = 2;

#[derive(Clone, Debug)]
pub struct HeadingInput<'a> {
    pub flow: &'a str,
    pub text: &'a str,
    pub cites: Vec<Citation>,
    pub next: Option<&'a str>,
    pub by: &'a str,
    pub now: Option<OffsetDateTime>,
}

pub fn read_headings(paths: &RootsPaths) -> RootsResult<Vec<HeadingRecord>> {
    read_table(&paths.headings)
}

#[must_use]
pub fn active_heading(rows: &[HeadingRecord], flow: &str) -> Option<HeadingRecord> {
    rows.iter()
        .filter(|heading| heading.flow == flow && heading.status == HeadingStatus::Active)
        .max_by(|left, right| left.created_at.cmp(&right.created_at))
        .cloned()
}

#[must_use]
pub fn is_live_flow(paths: &RootsPaths, flow: &str) -> bool {
    let mut started = false;
    let Ok(events) = read_events(paths) else {
        return false;
    };
    for event in events {
        if event.extra.get("flow").and_then(Value::as_str) != Some(flow) {
            continue;
        }
        if event.event_type == "flow.start" {
            started = true;
        }
        if event.event_type == "flow.end" {
            return false;
        }
    }
    started
}

#[must_use]
pub fn sentence_count(text: &str) -> usize {
    let chars: Vec<_> = text.chars().collect();
    if chars.is_empty() {
        return 0;
    }
    let mut count = 0;
    let mut start = 0;
    for index in 0..chars.len().saturating_sub(1) {
        if matches!(chars[index], '.' | '!' | '?') && chars[index + 1].is_whitespace() {
            if !chars[start..=index]
                .iter()
                .collect::<String>()
                .trim()
                .is_empty()
            {
                count += 1;
            }
            start = index + 1;
        }
    }
    if !chars[start..].iter().collect::<String>().trim().is_empty() {
        count += 1;
    }
    count
}

pub fn clean_heading_text(raw: &str) -> RootsResult<String> {
    let text = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.is_empty() {
        return Err(RootsError::validation("the heading is empty"));
    }
    let length = text.encode_utf16().count();
    if length > HEADING_MAX_LENGTH {
        return Err(RootsError::validation(format!(
            "the heading is {length} characters; keep it under {HEADING_MAX_LENGTH}"
        )));
    }
    if sentence_count(&text) > HEADING_MAX_SENTENCES {
        return Err(RootsError::validation(format!(
            "a heading is at most {HEADING_MAX_SENTENCES} sentences"
        )));
    }
    Ok(text)
}

fn live_idea<'a>(
    graph: &'a crate::model::Graph,
    id: &str,
    what: &str,
) -> RootsResult<&'a crate::model::NodeRecord> {
    let node = graph
        .nodes
        .iter()
        .find(|node| node.id == id)
        .ok_or_else(|| RootsError::not_found(format!("{what} {id} does not exist")))?;
    if node.kind != NodeKind::Idea {
        return Err(RootsError::validation(format!(
            "{what} {id} is a sprout, not an idea"
        )));
    }
    if matches!(node.status, NodeStatus::Idea(IdeaStatus::Composted)) {
        return Err(RootsError::validation(format!("{what} {id} is composted")));
    }
    Ok(node)
}

#[must_use]
pub fn cited_idea_ids(text: &str) -> Vec<String> {
    ids_with_prefix(text, b'r')
}

fn ids_with_prefix(text: &str, prefix: u8) -> Vec<String> {
    let bytes = text.as_bytes();
    let mut found = Vec::new();
    let mut index = 0;
    while index + 6 <= bytes.len() {
        let previous_ok = index == 0 || !is_word_dash(bytes[index - 1]);
        if previous_ok && bytes[index] == prefix && bytes[index + 1] == b'-' {
            let mut end = index + 2;
            while end < bytes.len()
                && (bytes[end].is_ascii_digit() || matches!(bytes[end], b'a'..=b'f'))
            {
                end += 1;
            }
            let length = end - (index + 2);
            if (4..=8).contains(&length) && (end == bytes.len() || !is_word_dash(bytes[end])) {
                let id = text[index..end].to_owned();
                if !found.contains(&id) {
                    found.push(id);
                }
                index = end;
                continue;
            }
        }
        index += 1;
    }
    found
}

fn is_word_dash(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')
}

fn check_cites(
    paths: &RootsPaths,
    graph: &crate::model::Graph,
    config: &RootsConfig,
    cites: &[Citation],
) -> RootsResult<()> {
    if cites.is_empty() {
        return Err(RootsError::validation(
            "cite at least one idea: --cite <id>:<exact words>",
        ));
    }
    for cite in cites {
        let node = live_idea(graph, &cite.node, "cited idea")?;
        require_tier(config, 1, "`luuna-roots heading`", Some(node))?;
        if !read_node_prose(paths, node, None)?
            .text
            .contains(&cite.quote)
        {
            return Err(RootsError::validation(format!(
                "cite quote not found in {}",
                cite.node
            )));
        }
    }
    Ok(())
}

fn check_refs(paths: &RootsPaths, text: &str, cites: &[Citation]) -> RootsResult<()> {
    let cited: BTreeSet<_> = cites.iter().map(|cite| cite.node.as_str()).collect();
    let missing = cited_idea_ids(text)
        .into_iter()
        .filter(|id| !cited.contains(id.as_str()))
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        return Err(RootsError::validation(format!(
            "the heading names {}; cite each idea it names",
            missing.join(", ")
        )));
    }
    let known: BTreeSet<_> = read_questions(paths)?
        .into_iter()
        .map(|question| question.id)
        .collect();
    let missing_questions = ids_with_prefix(text, b'q')
        .into_iter()
        .filter(|id| !known.contains(id))
        .collect::<Vec<_>>();
    if !missing_questions.is_empty() {
        return Err(RootsError::not_found(format!(
            "no such question: {}",
            missing_questions.join(", ")
        )));
    }
    Ok(())
}

pub fn file_heading(
    paths: &RootsPaths,
    config: &RootsConfig,
    input: HeadingInput<'_>,
) -> RootsResult<HeadingRecord> {
    require_tier(config, 1, "`luuna-roots heading`", None)?;
    if !is_live_flow(paths, input.flow) {
        return Err(RootsError::guard(format!(
            "{} is not a running flow session",
            input.flow
        )));
    }
    let text = clean_heading_text(input.text)?;
    let graph = read_graph(paths)?;
    check_cites(paths, &graph, config, &input.cites)?;
    check_refs(paths, &text, &input.cites)?;
    if let Some(next) = input.next {
        require_tier(
            config,
            1,
            "`luuna-roots heading --next`",
            Some(live_idea(&graph, next, "--next")?),
        )?;
    }
    let at = input.now.map(iso_at).unwrap_or_else(iso_now);
    let heading = update_table::<HeadingRecord, _, _>(&paths.headings, |mut rows| {
        if rows
            .iter()
            .any(|heading| heading.status == HeadingStatus::Dismissed && heading.text == text)
        {
            return Err(RootsError::validation(
                "a human dismissed this heading before; say something new",
            ));
        }
        let taken = hex_set(rows.iter().map(|heading| heading.id.as_str()));
        let heading = HeadingRecord {
            id: generate_id(IdPrefix::Heading, &taken),
            flow: input.flow.to_owned(),
            text: text.clone(),
            cites: input.cites.clone(),
            next: input.next.map(ToOwned::to_owned),
            by: input.by.to_owned(),
            status: HeadingStatus::Active,
            created_at: at.clone(),
            dismissed_by: None,
            dismissed_at: None,
        };
        rows.push(heading.clone());
        Ok((rows, true, heading))
    })?;
    let mut refs = heading
        .cites
        .iter()
        .map(|cite| cite.node.clone())
        .collect::<Vec<_>>();
    if let Some(next) = &heading.next {
        refs.push(next.clone());
    }
    let mut unique = BTreeSet::new();
    refs.retain(|reference| unique.insert(reference.clone()));
    let mut extra = Map::new();
    extra.insert("heading".into(), Value::String(heading.id.clone()));
    extra.insert("flow".into(), Value::String(heading.flow.clone()));
    append_event(
        paths,
        &make_event(
            "heading",
            input.by,
            EventFields {
                at: Some(at),
                refs: Some(refs),
                extra,
                ..Default::default()
            },
        ),
    )?;
    Ok(heading)
}

pub fn dismiss_heading(
    paths: &RootsPaths,
    id: &str,
    by: &str,
    now: Option<OffsetDateTime>,
) -> RootsResult<HeadingRecord> {
    let at = now.map(iso_at).unwrap_or_else(iso_now);
    let heading = update_table::<HeadingRecord, _, _>(&paths.headings, |mut rows| {
        let heading = rows
            .iter_mut()
            .find(|heading| heading.id == id)
            .ok_or_else(|| RootsError::not_found(format!("no heading {id}")))?;
        heading.status = HeadingStatus::Dismissed;
        heading.dismissed_by = Some(by.to_owned());
        heading.dismissed_at = Some(at.clone());
        let result = heading.clone();
        Ok((rows, true, result))
    })?;
    let mut extra = Map::new();
    extra.insert("heading".into(), Value::String(id.to_owned()));
    extra.insert("flow".into(), Value::String(heading.flow.clone()));
    append_event(
        paths,
        &make_event(
            "heading.dismiss",
            by,
            EventFields {
                at: Some(at),
                extra,
                ..Default::default()
            },
        ),
    )?;
    Ok(heading)
}
