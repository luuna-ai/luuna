//! Pure flow trail, next-pick, and card-key logic.

use roots_core::attention::{IdeaAttention, findings_ready, is_due};
use roots_core::model::{
    EventRecord, Graph, HeadingRecord, IdeaStatus, NodeKind, NodeRecord, QuestionRecord,
};
use roots_core::serde::Serialize;
use roots_core::serde_json::Value;
use roots_core::time_crate::OffsetDateTime;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrailEntry {
    pub id: String,
    pub slug: String,
    pub status: String,
    pub sessions: usize,
    pub answered: usize,
    pub open: usize,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FlowTrail {
    pub entries: Vec<TrailEntry>,
    pub current: Option<String>,
    pub planted: Vec<String>,
    pub adopted: Vec<String>,
    pub accepted: usize,
    pub rejected: usize,
}

#[must_use]
pub fn empty_trail() -> FlowTrail {
    FlowTrail {
        entries: Vec::new(),
        current: None,
        planted: Vec::new(),
        adopted: Vec::new(),
        accepted: 0,
        rejected: 0,
    }
}

#[must_use]
pub fn flow_events<'a>(events: &'a [EventRecord], flow: &str) -> Vec<&'a EventRecord> {
    let Some(start) = events.iter().position(|event| {
        event.event_type == "flow.start"
            && event.extra.get("flow").and_then(Value::as_str) == Some(flow)
    }) else {
        return Vec::new();
    };
    let by = &events[start].by;
    let mut output = Vec::new();
    for event in &events[start + 1..] {
        if event.event_type == "flow.end"
            && event.extra.get("flow").and_then(Value::as_str) == Some(flow)
        {
            break;
        }
        if &event.by == by {
            output.push(event);
        }
    }
    output
}

fn record_session(
    trail: &mut FlowTrail,
    positions: &mut BTreeMap<String, usize>,
    event: &EventRecord,
) {
    let Some(id) = event.node.as_ref() else {
        return;
    };
    let index = if let Some(index) = positions.get(id) {
        *index
    } else {
        let index = trail.entries.len();
        positions.insert(id.clone(), index);
        trail.entries.push(TrailEntry {
            id: id.clone(),
            slug: "?".to_owned(),
            status: "planted".to_owned(),
            sessions: 0,
            answered: 0,
            open: 0,
        });
        index
    };
    let entry = &mut trail.entries[index];
    entry.sessions += 1;
    entry.answered += event
        .extra
        .get("answered")
        .and_then(Value::as_u64)
        .unwrap_or_default() as usize;
    trail.current = Some(id.clone());
}

fn record_event(
    trail: &mut FlowTrail,
    positions: &mut BTreeMap<String, usize>,
    event: &EventRecord,
) {
    match event.event_type.as_str() {
        "session.end" => record_session(trail, positions, event),
        "plant" => {
            if let Some(id) = &event.node {
                if event.extra.get("sprout").is_some_and(Value::is_string) {
                    trail.adopted.push(id.clone());
                } else {
                    trail.planted.push(id.clone());
                }
            }
        }
        "accept" if event.extra.get("proposal").is_some_and(Value::is_string) => {
            trail.accepted += 1;
        }
        "reject" => trail.rejected += 1,
        _ => {}
    }
}

#[must_use]
pub fn build_trail(
    events: &[EventRecord],
    graph: &Graph,
    questions: &[QuestionRecord],
    flow: &str,
    now: OffsetDateTime,
) -> FlowTrail {
    let mut trail = empty_trail();
    let mut positions = BTreeMap::new();
    for event in flow_events(events, flow) {
        record_event(&mut trail, &mut positions, event);
    }
    for entry in &mut trail.entries {
        if let Some(node) = graph.nodes.iter().find(|node| node.id == entry.id) {
            entry.slug = node.slug.clone();
            entry.status = node.status.as_str().to_owned();
        }
        entry.open = questions
            .iter()
            .filter(|question| question.node == entry.id && is_due(question, now))
            .count();
    }
    trail
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FlowPick {
    pub id: String,
    pub slug: String,
    pub reason: String,
    pub from_heading: bool,
}

fn plural(count: usize, word: &str) -> String {
    format!("{count} {word}{}", if count == 1 { "" } else { "s" })
}

fn pick_reason(attention: &IdeaAttention) -> String {
    if findings_ready(attention) > 0 {
        "agent findings ready".to_owned()
    } else if !attention.due.is_empty() {
        plural(attention.due.len(), "open question")
    } else if attention.activity.sessions == 0 {
        "never thought".to_owned()
    } else {
        plural(attention.candidates.len(), "new question")
    }
}

fn to_pick(attention: &IdeaAttention, heading_next: Option<&str>) -> FlowPick {
    FlowPick {
        id: attention.node.id.clone(),
        slug: attention.node.slug.clone(),
        reason: pick_reason(attention),
        from_heading: Some(attention.node.id.as_str()) == heading_next,
    }
}

fn is_thinkable(node: &NodeRecord) -> bool {
    node.kind == NodeKind::Idea
        && matches!(
            node.status,
            roots_core::model::NodeStatus::Idea(
                IdeaStatus::Planted | IdeaStatus::Shaping | IdeaStatus::Committed
            )
        )
}

#[must_use]
pub fn rank_picks(
    ranked: &[IdeaAttention],
    touched: &BTreeSet<String>,
    graph: &Graph,
    heading_next: Option<&str>,
) -> Vec<FlowPick> {
    let mut picks = ranked
        .iter()
        .filter(|attention| !touched.contains(&attention.node.id))
        .map(|attention| to_pick(attention, heading_next))
        .chain(
            ranked
                .iter()
                .filter(|attention| touched.contains(&attention.node.id))
                .map(|attention| to_pick(attention, heading_next)),
        )
        .collect::<Vec<_>>();
    let Some(heading_next) = heading_next else {
        return picks;
    };
    if let Some(index) = picks.iter().position(|pick| pick.id == heading_next) {
        let hit = picks.remove(index);
        picks.insert(0, hit);
        return picks;
    }
    if let Some(node) = graph.nodes.iter().find(|node| node.id == heading_next) {
        if is_thinkable(node) {
            picks.insert(
                0,
                FlowPick {
                    id: node.id.clone(),
                    slug: node.slug.clone(),
                    reason: "the heading suggests it".to_owned(),
                    from_heading: true,
                },
            );
        }
    }
    picks
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HeadingView {
    Off,
    Waiting,
    None,
    Shown(Box<HeadingRecord>),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FlowCard {
    pub trail: FlowTrail,
    pub heading: HeadingView,
    pub jobs: Vec<String>,
    pub inbox: usize,
    pub picks: Vec<FlowPick>,
    pub pick: usize,
    pub notice: Option<String>,
    pub ended: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FlowKey {
    Enter,
    Think,
    Other,
    Plant,
    Dismiss,
    Quit,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FlowAction {
    Think { id: String },
    Tend,
    Plant,
    Dismiss { heading: String },
    Quit,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FlowStep {
    pub card: FlowCard,
    pub action: Option<FlowAction>,
}

#[must_use]
pub fn initial_card() -> FlowCard {
    FlowCard {
        trail: empty_trail(),
        heading: HeadingView::Off,
        jobs: Vec::new(),
        inbox: 0,
        picks: Vec::new(),
        pick: 0,
        notice: None,
        ended: false,
    }
}

#[must_use]
pub fn current_pick(card: &FlowCard) -> Option<&FlowPick> {
    card.picks.get(card.pick).or_else(|| card.picks.first())
}

#[must_use]
pub fn enter_action(card: &FlowCard) -> Option<FlowAction> {
    if card.inbox > 0 {
        return Some(FlowAction::Tend);
    }
    current_pick(card)
        .map(|pick| FlowAction::Think {
            id: pick.id.clone(),
        })
        .or(Some(FlowAction::Plant))
}

fn no_action(mut card: FlowCard, notice: &str) -> FlowStep {
    card.notice = Some(notice.to_owned());
    FlowStep { card, action: None }
}

#[must_use]
pub fn flow_key(card: FlowCard, key: FlowKey) -> FlowStep {
    match key {
        FlowKey::Quit => {
            let mut card = card;
            card.ended = true;
            FlowStep {
                card,
                action: Some(FlowAction::Quit),
            }
        }
        FlowKey::Enter => FlowStep {
            action: enter_action(&card),
            card,
        },
        FlowKey::Think => match current_pick(&card) {
            Some(pick) => FlowStep {
                action: Some(FlowAction::Think {
                    id: pick.id.clone(),
                }),
                card,
            },
            None => no_action(card, "nothing needs thinking; [p] plants"),
        },
        FlowKey::Other => {
            if card.picks.len() < 2 {
                no_action(card, "no other idea needs thinking")
            } else {
                let mut card = card;
                card.pick = (card.pick + 1) % card.picks.len();
                card.notice = None;
                FlowStep { card, action: None }
            }
        }
        FlowKey::Plant => FlowStep {
            card,
            action: Some(FlowAction::Plant),
        },
        FlowKey::Dismiss => {
            let heading = match &card.heading {
                HeadingView::Shown(heading) => Some(heading.id.clone()),
                _ => None,
            };
            match heading {
                Some(heading) => FlowStep {
                    card,
                    action: Some(FlowAction::Dismiss { heading }),
                },
                None => no_action(card, "no heading to dismiss"),
            }
        }
    }
}

#[must_use]
pub fn parse_flow_keys(data: &str) -> Vec<FlowKey> {
    let mut output = Vec::new();
    for character in data.chars() {
        match character {
            '\r' | '\n' => output.push(FlowKey::Enter),
            '\u{3}' | '\u{4}' => output.push(FlowKey::Quit),
            character if character.eq_ignore_ascii_case(&'n') => output.push(FlowKey::Think),
            character if character.eq_ignore_ascii_case(&'o') => output.push(FlowKey::Other),
            character if character.eq_ignore_ascii_case(&'p') => output.push(FlowKey::Plant),
            character if character.eq_ignore_ascii_case(&'x') => output.push(FlowKey::Dismiss),
            character if character.eq_ignore_ascii_case(&'q') => output.push(FlowKey::Quit),
            _ => {}
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use roots_core::model::{HeadingStatus, NodeStatus, QuestionStatus};

    fn node(id: &str, slug: &str, status: IdeaStatus) -> NodeRecord {
        NodeRecord::new(
            id.to_owned(),
            NodeKind::Idea,
            slug.to_owned(),
            NodeStatus::Idea(status),
            "human:test".to_owned(),
            "2026-01-01T00:00:00Z".to_owned(),
        )
    }

    fn event(kind: &str, by: &str, node: Option<&str>, extra: &[(&str, Value)]) -> EventRecord {
        EventRecord {
            event_type: kind.to_owned(),
            by: by.to_owned(),
            at: "2026-01-01T00:00:00Z".to_owned(),
            node: node.map(ToOwned::to_owned),
            refs: None,
            extra: extra
                .iter()
                .map(|(key, value)| ((*key).to_owned(), value.clone()))
                .collect(),
        }
    }

    fn question(id: &str, node: &str, status: QuestionStatus) -> QuestionRecord {
        QuestionRecord {
            id: id.to_owned(),
            node: node.to_owned(),
            text: "?".to_owned(),
            by: "agent:test".to_owned(),
            status,
            created_at: "2026-01-01T00:00:00Z".to_owned(),
            r#ref: None,
            answered_by: None,
            answered_at: None,
            dismissed_by: None,
            dismissed_at: None,
            delegated_by: None,
            delegated_at: None,
            findings: None,
            snoozed_until: None,
            session: None,
        }
    }

    #[test]
    fn trail_counts_only_this_flow_and_keeps_touch_order() {
        let graph = Graph {
            nodes: vec![
                node("r-aaaa", "principal", IdeaStatus::Shaping),
                node("r-bbbb", "no-secrets", IdeaStatus::Shaping),
            ],
            edges: Vec::new(),
        };
        let human = "human:test";
        let events = vec![
            event(
                "flow.start",
                human,
                None,
                &[("flow", Value::String("fl-1".to_owned()))],
            ),
            event(
                "session.end",
                human,
                Some("r-aaaa"),
                &[("answered", Value::Number(3_u64.into()))],
            ),
            event("sprout", "agent:test", Some("s-a"), &[]),
            event(
                "plant",
                human,
                Some("r-cccc"),
                &[("sprout", Value::String("s-a".to_owned()))],
            ),
            event(
                "accept",
                human,
                None,
                &[("proposal", Value::String("p-a".to_owned()))],
            ),
            event("plant", human, Some("r-dddd"), &[]),
            event(
                "session.end",
                human,
                Some("r-bbbb"),
                &[("answered", Value::Number(1_u64.into()))],
            ),
            event(
                "flow.end",
                human,
                None,
                &[("flow", Value::String("fl-1".to_owned()))],
            ),
            event(
                "session.end",
                human,
                Some("r-aaaa"),
                &[("answered", Value::Number(9_u64.into()))],
            ),
        ];
        let trail = build_trail(
            &events,
            &graph,
            &[question("q-a", "r-bbbb", QuestionStatus::Open)],
            "fl-1",
            OffsetDateTime::now_utc(),
        );
        assert_eq!(trail.entries[0].id, "r-aaaa");
        assert_eq!(trail.entries[0].answered, 3);
        assert_eq!(trail.entries[1].id, "r-bbbb");
        assert_eq!(trail.entries[1].open, 1);
        assert_eq!(trail.current.as_deref(), Some("r-bbbb"));
        assert_eq!(trail.adopted, ["r-cccc"]);
        assert_eq!(trail.planted, ["r-dddd"]);
        assert_eq!(trail.accepted, 1);
    }

    #[test]
    fn card_keys_preserve_inbox_pick_dismiss_and_quit_rules() {
        let mut card = initial_card();
        card.picks = vec![
            FlowPick {
                id: "r-a".to_owned(),
                slug: "a".to_owned(),
                reason: String::new(),
                from_heading: false,
            },
            FlowPick {
                id: "r-b".to_owned(),
                slug: "b".to_owned(),
                reason: String::new(),
                from_heading: false,
            },
        ];
        card.inbox = 2;
        assert_eq!(enter_action(&card), Some(FlowAction::Tend));
        card.inbox = 0;
        assert_eq!(
            enter_action(&card),
            Some(FlowAction::Think {
                id: "r-a".to_owned()
            })
        );
        let other = flow_key(card.clone(), FlowKey::Other).card;
        assert_eq!(
            flow_key(other, FlowKey::Think).action,
            Some(FlowAction::Think {
                id: "r-b".to_owned()
            })
        );
        assert_eq!(
            flow_key(initial_card(), FlowKey::Other)
                .card
                .notice
                .as_deref(),
            Some("no other idea needs thinking")
        );
        let mut shown = card;
        shown.heading = HeadingView::Shown(Box::new(HeadingRecord {
            id: "h-a".to_owned(),
            flow: "fl-a".to_owned(),
            text: "Heading".to_owned(),
            cites: Vec::new(),
            next: None,
            by: "agent:test".to_owned(),
            status: HeadingStatus::Active,
            created_at: "2026-01-01T00:00:00Z".to_owned(),
            dismissed_by: None,
            dismissed_at: None,
        }));
        assert_eq!(
            flow_key(shown, FlowKey::Dismiss).action,
            Some(FlowAction::Dismiss {
                heading: "h-a".to_owned()
            })
        );
        assert_eq!(
            parse_flow_keys("\rNox\u{3}zP\n"),
            vec![
                FlowKey::Enter,
                FlowKey::Think,
                FlowKey::Other,
                FlowKey::Dismiss,
                FlowKey::Quit,
                FlowKey::Plant,
                FlowKey::Enter,
            ]
        );
    }
}
