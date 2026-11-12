//! Append-only event audit log helpers.

use crate::errors::RootsResult;
use crate::model::{Actor, EventRecord};
use crate::paths::RootsPaths;
use crate::store::{LockOptions, append_jsonl_file, dedup_by, read_jsonl_file, with_lock};
use crate::time::iso_now;
use serde_json::{Map, Value};
use std::collections::HashSet;

#[derive(Clone, Debug, Default)]
pub struct EventFields {
    pub at: Option<String>,
    pub node: Option<String>,
    pub refs: Option<Vec<String>>,
    pub extra: Map<String, Value>,
}

#[must_use]
pub fn make_event(
    event_type: impl Into<String>,
    by: impl Into<Actor>,
    fields: EventFields,
) -> EventRecord {
    EventRecord {
        event_type: event_type.into(),
        by: by.into(),
        at: fields.at.unwrap_or_else(iso_now),
        node: fields.node,
        refs: fields.refs,
        extra: fields.extra,
    }
}

pub fn append_event(paths: &RootsPaths, event: &EventRecord) -> RootsResult<()> {
    with_lock(
        &paths.events,
        || append_jsonl_file(&paths.events, std::slice::from_ref(event)),
        LockOptions::default(),
    )
}

/// Events lack IDs, so only exact JSON object forms collapse after a union
/// merge. Serde JSON preserves insertion order for this build.
pub fn read_events(paths: &RootsPaths) -> RootsResult<Vec<EventRecord>> {
    read_jsonl_file(&paths.events).map(|events| {
        dedup_by(events, |event| {
            serde_json::to_string(event).unwrap_or_default()
        })
    })
}

#[must_use]
pub fn events_for<'a>(events: &'a [EventRecord], id: &str) -> Vec<&'a EventRecord> {
    events
        .iter()
        .filter(|event| {
            event.node.as_deref() == Some(id)
                || event
                    .refs
                    .as_ref()
                    .is_some_and(|refs| refs.iter().any(|reference| reference == id))
        })
        .collect()
}

#[must_use]
pub fn contributors(events: &[EventRecord], author: Option<&str>) -> Vec<Actor> {
    let mut seen = HashSet::new();
    let mut output = Vec::new();
    if let Some(author) = author {
        seen.insert(author.to_owned());
        output.push(author.to_owned());
    }
    for event in events {
        if event.by == "roots" || event.by.starts_with("roots:") {
            continue;
        }
        if seen.insert(event.by.clone()) {
            output.push(event.by.clone());
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn contributor_order_excludes_roots_bookkeeping() {
        let events = vec![
            make_event(
                "plant",
                "roots",
                EventFields {
                    at: Some("a".into()),
                    ..Default::default()
                },
            ),
            make_event(
                "link",
                "agent:model",
                EventFields {
                    at: Some("b".into()),
                    ..Default::default()
                },
            ),
            make_event(
                "mv",
                "human:jay",
                EventFields {
                    at: Some("c".into()),
                    ..Default::default()
                },
            ),
        ];
        assert_eq!(
            contributors(&events, Some("human:jay")),
            ["human:jay", "agent:model"]
        );
    }
}
