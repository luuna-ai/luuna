//! Pure cards and reducer for the human `tend` review queue.
//!
//! Persistence and terminal ownership intentionally live outside this module:
//! it is useful to both the JSON listing and the interactive session, while
//! keeping key handling deterministic and directly testable.

use crate::model::{EdgeRel, ProposalKind, ProposalRecord};
use crate::proposals::MENTION_ACTOR;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

pub const EXPIRING_SOON_DAYS: i64 = 3;
const DAY_NANOS: i128 = 86_400_000_000_000;

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CardNode {
    pub id: String,
    pub slug: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CardCite {
    pub node: String,
    pub quote: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProposalCard {
    #[serde(rename = "type")]
    pub card_type: String,
    pub id: String,
    pub ids: Vec<String>,
    pub kind: ProposalKind,
    pub from: CardNode,
    pub to: Option<CardNode>,
    pub rel: Option<EdgeRel>,
    pub pick_rel: bool,
    pub sources: Vec<String>,
    pub reason: Option<String>,
    pub notes: Vec<String>,
    pub cites: Vec<CardCite>,
    pub expires_at: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SproutCard {
    #[serde(rename = "type")]
    pub card_type: String,
    pub id: String,
    pub ids: Vec<String>,
    pub slug: String,
    pub statement: String,
    pub body: String,
    pub author: String,
    pub expires_at: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(untagged)]
pub enum TendCard {
    Proposal(ProposalCard),
    Sprout(SproutCard),
}

impl TendCard {
    #[must_use]
    pub fn id(&self) -> &str {
        match self {
            Self::Proposal(card) => &card.id,
            Self::Sprout(card) => &card.id,
        }
    }

    #[must_use]
    pub fn ids(&self) -> &[String] {
        match self {
            Self::Proposal(card) => &card.ids,
            Self::Sprout(card) => &card.ids,
        }
    }

    #[must_use]
    pub fn expires_at(&self) -> Option<&str> {
        match self {
            Self::Proposal(card) => card.expires_at.as_deref(),
            Self::Sprout(card) => card.expires_at.as_deref(),
        }
    }
}

#[must_use]
pub fn sprout_card(
    id: impl Into<String>,
    slug: impl Into<String>,
    author: impl Into<String>,
    expires_at: Option<String>,
    statement: impl Into<String>,
    body: impl Into<String>,
) -> TendCard {
    let id = id.into();
    TendCard::Sprout(SproutCard {
        card_type: "sprout".to_owned(),
        ids: vec![id.clone()],
        id,
        slug: slug.into(),
        statement: statement.into(),
        body: body.into(),
        author: author.into(),
        expires_at,
    })
}

fn is_mention(proposal: &ProposalRecord) -> bool {
    proposal.by == MENTION_ACTOR
}

fn pair_key(proposal: &ProposalRecord) -> String {
    let mut pair = [
        proposal.from.as_deref().unwrap_or_default(),
        proposal.to.as_deref().unwrap_or_default(),
    ];
    pair.sort_unstable();
    format!("{}|{}", pair[0], pair[1])
}

fn soonest(group: &[&ProposalRecord]) -> Option<String> {
    group
        .iter()
        .filter_map(|proposal| proposal.expires_at.clone())
        .min()
}

fn unique_cites(group: &[&ProposalRecord]) -> Vec<CardCite> {
    let mut seen = BTreeSet::new();
    let mut cites = Vec::new();
    for citation in group.iter().flat_map(|proposal| proposal.cites.iter()) {
        let key = format!("{}\0{}", citation.node, citation.quote);
        if seen.insert(key) {
            cites.push(CardCite {
                node: citation.node.clone(),
                quote: citation.quote.clone(),
            });
        }
    }
    cites
}

fn card_for_group<F>(group: &[&ProposalRecord], node: &F) -> ProposalCard
where
    F: Fn(&str) -> CardNode,
{
    let primary = group
        .iter()
        .copied()
        .find(|proposal| !is_mention(proposal))
        .unwrap_or_else(|| group[0]);
    let mut ids = vec![primary.id.clone()];
    ids.extend(
        group
            .iter()
            .copied()
            .filter(|proposal| proposal.id != primary.id)
            .map(|proposal| proposal.id.clone()),
    );
    let mut sources = Vec::new();
    for proposal in group {
        if !sources.contains(&proposal.by) {
            sources.push(proposal.by.clone());
        }
    }
    let agent_reason = group
        .iter()
        .copied()
        .find(|proposal| !is_mention(proposal))
        .and_then(|proposal| proposal.reason.clone());
    ProposalCard {
        card_type: "proposal".to_owned(),
        id: primary.id.clone(),
        ids,
        kind: primary.kind,
        from: node(primary.from.as_deref().unwrap_or_default()),
        to: primary.to.as_deref().map(node),
        rel: (primary.kind == ProposalKind::Edge)
            .then_some(primary.rel)
            .flatten(),
        pick_rel: primary.kind == ProposalKind::Edge
            && group.iter().any(|proposal| proposal.rel.is_none()),
        sources,
        reason: agent_reason,
        notes: group
            .iter()
            .copied()
            .filter(|proposal| is_mention(proposal))
            .map(|proposal| {
                proposal
                    .reason
                    .clone()
                    .unwrap_or_else(|| "mention".to_owned())
            })
            .collect(),
        cites: unique_cites(group),
        expires_at: soonest(group),
    }
}

/// Build one card for each independently decidable proposal group.
#[must_use]
pub fn proposal_cards<F>(pending: &[&ProposalRecord], node: F) -> Vec<ProposalCard>
where
    F: Fn(&str) -> CardNode,
{
    let mut groups: Vec<(String, Vec<&ProposalRecord>)> = Vec::new();
    for proposal in pending {
        let key = if proposal.kind == ProposalKind::Edge {
            format!("edge|{}", pair_key(proposal))
        } else {
            proposal.id.clone()
        };
        if let Some((_, group)) = groups.iter_mut().find(|(existing, _)| *existing == key) {
            group.push(*proposal);
        } else {
            groups.push((key, vec![*proposal]));
        }
    }
    let mut cards = groups
        .iter()
        .map(|(_, group)| card_for_group(group, &node))
        .collect::<Vec<_>>();
    cards.sort_by(|left, right| {
        left.expires_at
            .as_deref()
            .unwrap_or("~")
            .cmp(right.expires_at.as_deref().unwrap_or("~"))
    });
    cards
}

/// Soonest expiry first, retaining input order for matching expiries.
#[must_use]
pub fn sort_cards(mut cards: Vec<TendCard>) -> Vec<TendCard> {
    cards.sort_by(|left, right| {
        left.expires_at()
            .unwrap_or("~")
            .cmp(right.expires_at().unwrap_or("~"))
    });
    cards
}

/// `ceil((expires - now) / one_day)`, never below zero.
#[must_use]
pub fn days_left(expires_at: Option<&str>, now: OffsetDateTime) -> Option<i64> {
    let expires = OffsetDateTime::parse(expires_at?, &Rfc3339).ok()?;
    let diff = (expires - now).whole_nanoseconds();
    if diff <= 0 {
        return Some(0);
    }
    Some(((diff + DAY_NANOS - 1) / DAY_NANOS) as i64)
}

#[must_use]
pub fn expiring_soon(card: &TendCard, now: OffsetDateTime) -> bool {
    days_left(card.expires_at(), now).is_some_and(|days| days <= EXPIRING_SOON_DAYS)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TendAction {
    Accept {
        rel: Option<EdgeRel>,
        keep: Option<String>,
    },
    Reject {
        reason: Option<String>,
    },
    Skip,
    Quit,
    Endorse {
        text: String,
        editor: bool,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CardKeyAction {
    Tend(TendAction),
    Reason,
    Thoughts,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CardKey {
    pub key: char,
    pub label: String,
    pub action: CardKeyAction,
}

fn reject_keys() -> Vec<CardKey> {
    vec![
        CardKey {
            key: 'n',
            label: "reject".to_owned(),
            action: CardKeyAction::Tend(TendAction::Reject { reason: None }),
        },
        CardKey {
            key: 'r',
            label: "reject w/ reason".to_owned(),
            action: CardKeyAction::Reason,
        },
        CardKey {
            key: 's',
            label: "skip".to_owned(),
            action: CardKeyAction::Tend(TendAction::Skip),
        },
    ]
}

/// Keys shown for a card, in their visual and behavioral order.
#[must_use]
pub fn card_keys(card: &TendCard) -> Vec<CardKey> {
    match card {
        TendCard::Sprout(_) => {
            let mut keys = vec![
                CardKey {
                    key: 't',
                    label: "accept w/ thoughts".to_owned(),
                    action: CardKeyAction::Thoughts,
                },
                CardKey {
                    key: 'y',
                    label: "rewrite as my idea".to_owned(),
                    action: CardKeyAction::Tend(TendAction::Accept {
                        rel: None,
                        keep: None,
                    }),
                },
            ];
            keys.extend(reject_keys());
            keys
        }
        TendCard::Proposal(card) => {
            let merge_target = if card.kind == ProposalKind::Merge {
                card.to.as_ref()
            } else {
                None
            };
            let mut keys = if let Some(to) = merge_target {
                vec![
                    CardKey {
                        key: '1',
                        label: format!("keep {}", card.from.slug),
                        action: CardKeyAction::Tend(TendAction::Accept {
                            rel: None,
                            keep: Some(card.from.id.clone()),
                        }),
                    },
                    CardKey {
                        key: '2',
                        label: format!("keep {}", to.slug),
                        action: CardKeyAction::Tend(TendAction::Accept {
                            rel: None,
                            keep: Some(to.id.clone()),
                        }),
                    },
                ]
            } else if card.kind != ProposalKind::Edge {
                vec![CardKey {
                    key: 'y',
                    label: if card.kind == ProposalKind::Split {
                        "accept + think now"
                    } else {
                        "accept"
                    }
                    .to_owned(),
                    action: CardKeyAction::Tend(TendAction::Accept {
                        rel: None,
                        keep: None,
                    }),
                }]
            } else {
                let mut edge_keys = Vec::new();
                if let Some(rel) = card.rel {
                    edge_keys.push(CardKey {
                        key: 'y',
                        label: format!("accept {}", rel.as_str()),
                        action: CardKeyAction::Tend(TendAction::Accept {
                            rel: None,
                            keep: None,
                        }),
                    });
                }
                if !card.pick_rel {
                    vec![CardKey {
                        key: 'y',
                        label: "accept".to_owned(),
                        action: CardKeyAction::Tend(TendAction::Accept {
                            rel: None,
                            keep: None,
                        }),
                    }]
                } else {
                    for (key, rel) in [
                        ('1', EdgeRel::Serves),
                        ('2', EdgeRel::Tension),
                        ('3', EdgeRel::Replaces),
                    ] {
                        edge_keys.push(CardKey {
                            key,
                            label: rel.as_str().to_owned(),
                            action: CardKeyAction::Tend(TendAction::Accept {
                                rel: Some(rel),
                                keep: None,
                            }),
                        });
                    }
                    edge_keys
                }
            };
            keys.extend(reject_keys());
            keys
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TendOutcome {
    Accepted,
    Rejected,
    Skipped,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TendState {
    pub cards: Vec<TendCard>,
    pub index: usize,
    pub reason: Option<String>,
    pub thoughts: Option<String>,
    pub busy: bool,
    pub outcomes: BTreeMap<String, TendOutcome>,
    pub notice: Option<String>,
    pub error: Option<String>,
    pub ended: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TendInput {
    Data(String),
    Done {
        result: Result<(TendOutcome, Option<String>), String>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TendEffect {
    pub card: TendCard,
    pub action: TendAction,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TendStep {
    pub state: TendState,
    pub effects: Vec<TendEffect>,
}

#[must_use]
pub fn initial_tend(cards: Vec<TendCard>) -> TendState {
    TendState {
        ended: cards.is_empty(),
        cards,
        index: 0,
        reason: None,
        thoughts: None,
        busy: false,
        outcomes: BTreeMap::new(),
        notice: None,
        error: None,
    }
}

#[must_use]
pub fn current_card(state: &TendState) -> Option<&TendCard> {
    (!state.ended)
        .then(|| state.cards.get(state.index))
        .flatten()
}

fn advance(
    mut state: TendState,
    card: &TendCard,
    outcome: TendOutcome,
    notice: Option<String>,
) -> TendState {
    state.index += 1;
    state.busy = false;
    state.outcomes.insert(card.id().to_owned(), outcome);
    state.notice = notice;
    state.error = None;
    state.ended = state.index >= state.cards.len();
    state
}

fn act(state: TendState, card: &TendCard, action: TendAction) -> TendStep {
    if action == TendAction::Quit {
        let mut state = state;
        state.ended = true;
        return TendStep {
            state,
            effects: vec![],
        };
    }
    if action == TendAction::Skip {
        return TendStep {
            state: advance(state, card, TendOutcome::Skipped, None),
            effects: vec![],
        };
    }
    let mut state = state;
    state.busy = true;
    state.error = None;
    state.reason = None;
    state.thoughts = None;
    TendStep {
        state,
        effects: vec![TendEffect {
            card: card.clone(),
            action,
        }],
    }
}

fn edited(mut text: String, character: char) -> String {
    if matches!(character, '\u{7f}' | '\u{8}') {
        text.pop();
    } else if !character.is_control() {
        text.push(character);
    }
    text
}

fn on_reason_char(state: TendState, card: &TendCard, character: char) -> TendStep {
    let text = state.reason.clone().unwrap_or_default();
    if matches!(character, '\r' | '\n') {
        let reason = text.trim();
        if reason.is_empty() {
            let mut state = state;
            state.reason = None;
            state.notice = Some("no reason; not rejected".to_owned());
            return TendStep {
                state,
                effects: vec![],
            };
        }
        return act(
            state,
            card,
            TendAction::Reject {
                reason: Some(reason.to_owned()),
            },
        );
    }
    if character == '\u{1b}' {
        let mut state = state;
        state.reason = None;
        state.notice = Some("cancelled".to_owned());
        return TendStep {
            state,
            effects: vec![],
        };
    }
    let mut state = state;
    state.reason = Some(edited(text, character));
    TendStep {
        state,
        effects: vec![],
    }
}

fn on_thought_char(state: TendState, card: &TendCard, character: char) -> TendStep {
    let text = state.thoughts.clone().unwrap_or_default();
    if character == '\u{1b}' {
        let mut state = state;
        state.thoughts = None;
        state.notice = Some("cancelled".to_owned());
        return TendStep {
            state,
            effects: vec![],
        };
    }
    if character == '\u{5}' {
        return act(
            state,
            card,
            TendAction::Endorse {
                text: text.trim().to_owned(),
                editor: true,
            },
        );
    }
    if matches!(character, '\r' | '\n') {
        if text.trim().is_empty() {
            let mut state = state;
            state.error = Some("write a thought first (or ctrl-e for $EDITOR)".to_owned());
            return TendStep {
                state,
                effects: vec![],
            };
        }
        return act(
            state,
            card,
            TendAction::Endorse {
                text: text.trim().to_owned(),
                editor: false,
            },
        );
    }
    let mut state = state;
    state.thoughts = Some(edited(text, character));
    state.error = None;
    TendStep {
        state,
        effects: vec![],
    }
}

fn on_card_key(state: TendState, card: &TendCard, character: char) -> TendStep {
    let key = character.to_ascii_lowercase();
    if key == 'q' {
        return act(state, card, TendAction::Quit);
    }
    let Some(hit) = card_keys(card).into_iter().find(|entry| entry.key == key) else {
        return TendStep {
            state,
            effects: vec![],
        };
    };
    match hit.action {
        CardKeyAction::Reason => {
            let mut state = state;
            state.reason = Some(String::new());
            state.notice = None;
            state.error = None;
            TendStep {
                state,
                effects: vec![],
            }
        }
        CardKeyAction::Thoughts => {
            let mut state = state;
            state.thoughts = Some(String::new());
            state.notice = None;
            state.error = None;
            TendStep {
                state,
                effects: vec![],
            }
        }
        CardKeyAction::Tend(action) => act(state, card, action),
    }
}

/// Split terminal input into characters and discard arrow / escape sequences.
#[must_use]
pub fn input_chars(data: &str) -> Vec<char> {
    let chars = data.chars().collect::<Vec<_>>();
    let mut out = Vec::new();
    let mut index = 0;
    while index < chars.len() {
        if chars[index] == '\u{1b}' && chars.get(index + 1) == Some(&'[') {
            // JS advances by two inside a `for` loop, thereby also consuming
            // the escape sequence's final byte on the next increment.
            index += 3;
        } else {
            out.push(chars[index]);
            index += 1;
        }
    }
    out
}

fn on_char(state: TendState, character: char) -> TendStep {
    let Some(card) = current_card(&state).cloned() else {
        return TendStep {
            state,
            effects: vec![],
        };
    };
    if matches!(character, '\u{3}' | '\u{4}') {
        return act(state, &card, TendAction::Quit);
    }
    if state.thoughts.is_some() {
        on_thought_char(state, &card, character)
    } else if state.reason.is_some() {
        on_reason_char(state, &card, character)
    } else {
        on_card_key(state, &card, character)
    }
}

fn on_data(state: TendState, data: &str) -> TendStep {
    let mut state = state;
    let mut effects = Vec::new();
    for character in input_chars(data) {
        if state.ended || state.busy {
            break;
        }
        let step = on_char(state, character);
        state = step.state;
        effects.extend(step.effects);
    }
    TendStep { state, effects }
}

#[must_use]
pub fn tend_step(state: TendState, input: TendInput) -> TendStep {
    if state.ended {
        return TendStep {
            state,
            effects: vec![],
        };
    }
    match input {
        TendInput::Data(_data) if state.busy => TendStep {
            state,
            effects: vec![],
        },
        TendInput::Data(data) => on_data(state, &data),
        TendInput::Done { result: Err(error) } => {
            let mut state = state;
            state.busy = false;
            state.error = Some(error);
            TendStep {
                state,
                effects: vec![],
            }
        }
        TendInput::Done {
            result: Ok((outcome, notice)),
        } => {
            let Some(card) = current_card(&state).cloned() else {
                return TendStep {
                    state,
                    effects: vec![],
                };
            };
            TendStep {
                state: advance(state, &card, outcome, notice),
                effects: vec![],
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
pub struct TendTally {
    pub accepted: usize,
    pub rejected: usize,
    pub skipped: usize,
    pub left: usize,
}

#[must_use]
pub fn tend_tally(state: &TendState) -> TendTally {
    let mut tally = TendTally::default();
    for card in &state.cards {
        match state.outcomes.get(card.id()) {
            Some(TendOutcome::Accepted) => tally.accepted += 1,
            Some(TendOutcome::Rejected) => tally.rejected += 1,
            Some(TendOutcome::Skipped) => tally.skipped += 1,
            None => tally.left += 1,
        }
    }
    tally
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Citation, ProposalStatus};

    fn proposal(id: &str, rel: Option<EdgeRel>, by: &str) -> ProposalRecord {
        ProposalRecord {
            id: id.to_owned(),
            kind: ProposalKind::Edge,
            from: Some("r-a".to_owned()),
            to: Some("r-b".to_owned()),
            rel,
            reason: Some("why".to_owned()),
            cites: vec![Citation {
                node: "r-a".to_owned(),
                quote: "quote".to_owned(),
            }],
            by: by.to_owned(),
            status: ProposalStatus::Pending,
            created_at: "2026-09-28T11:30:00Z".to_owned(),
            expires_at: Some("2026-10-09T11:30:00Z".to_owned()),
            decided_by: None,
            decided_at: None,
            decision_reason: None,
        }
    }

    fn cards() -> Vec<TendCard> {
        let agent = proposal("p-agent", Some(EdgeRel::Tension), "agent:test");
        let mention = proposal("p-mention", None, MENTION_ACTOR);
        proposal_cards(&[&agent, &mention], |id| CardNode {
            id: id.to_owned(),
            slug: id.to_owned(),
        })
        .into_iter()
        .map(TendCard::Proposal)
        .collect()
    }

    #[test]
    fn mention_and_agent_share_card_and_pick_relation() {
        let card = cards().pop().expect("card");
        let TendCard::Proposal(card) = &card else {
            panic!("proposal")
        };
        assert_eq!(card.ids, ["p-agent", "p-mention"]);
        assert!(card.pick_rel);
        assert_eq!(card_keys(&TendCard::Proposal(card.clone())).len(), 7);
    }

    #[test]
    fn reducer_preserves_reason_and_errors() {
        let state = initial_tend(cards());
        let step = tend_step(state, TendInput::Data("rtoo vague\r".to_owned()));
        assert_eq!(step.effects.len(), 1);
        assert_eq!(
            step.effects[0].action,
            TendAction::Reject {
                reason: Some("too vague".to_owned())
            }
        );
        let failed = tend_step(
            step.state,
            TendInput::Done {
                result: Err("cycle".to_owned()),
            },
        );
        assert_eq!(failed.state.index, 0);
        assert_eq!(failed.state.error.as_deref(), Some("cycle"));
    }

    #[test]
    fn input_skips_arrow_escape_sequence() {
        assert_eq!(input_chars("\u{1b}[Ay"), vec!['y']);
    }
}
