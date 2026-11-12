//! Pure `think` session reducer.
//!
//! Saves answer the current question, while keys move it through the durable
//! question states. Persistence is deliberately left to `think_session`.

use roots_core::blame::DiffSpan;
use roots_core::model::QuestionRecord;
use roots_core::serde::Serialize;
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Outcome {
    Answered,
    Dismissed,
    Snoozed,
    Skipped,
    Delegated,
}

impl Outcome {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Answered => "answered",
            Self::Dismissed => "dismissed",
            Self::Snoozed => "snoozed",
            Self::Skipped => "skipped",
            Self::Delegated => "delegated",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionKey {
    Dismiss,
    Snooze,
    Skip,
    Quit,
    Delegate,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EndReason {
    Completed,
    Quit,
}

impl EndReason {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Quit => "quit",
        }
    }
}

#[derive(Clone, Debug)]
pub struct SessionState {
    pub questions: Vec<QuestionRecord>,
    pub index: usize,
    pub outcomes: BTreeMap<String, Outcome>,
    pub saves: usize,
    pub ended: Option<EndReason>,
    pub notice: Option<String>,
    pub can_delegate: bool,
}

#[derive(Clone, Debug)]
pub enum SessionInput {
    Save {
        span: DiffSpan,
        lines: Vec<String>,
    },
    Key(SessionKey),
    Arrive {
        questions: Vec<QuestionRecord>,
        cap: usize,
    },
}

#[derive(Clone, Debug)]
pub enum SessionEffect {
    Answer {
        question: QuestionRecord,
        span: DiffSpan,
        lines: Vec<String>,
    },
    Dismiss {
        question: QuestionRecord,
    },
    Snooze {
        question: QuestionRecord,
    },
    Delegate {
        question: QuestionRecord,
    },
    End {
        reason: EndReason,
    },
}

#[derive(Clone, Debug)]
pub struct Step {
    pub state: SessionState,
    pub effects: Vec<SessionEffect>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct SessionTally {
    pub answered: usize,
    pub dismissed: usize,
    pub snoozed: usize,
    pub skipped: usize,
    pub delegated: usize,
    pub unasked: usize,
}

#[must_use]
pub fn initial_state(questions: Vec<QuestionRecord>, can_delegate: bool) -> SessionState {
    SessionState {
        questions,
        index: 0,
        outcomes: BTreeMap::new(),
        saves: 0,
        ended: None,
        notice: None,
        can_delegate,
    }
}

#[must_use]
pub fn current_question(state: &SessionState) -> Option<&QuestionRecord> {
    state
        .ended
        .is_none()
        .then(|| state.questions.get(state.index))
        .flatten()
}

fn past(outcome: Outcome) -> &'static str {
    match outcome {
        Outcome::Answered => "answered",
        Outcome::Dismissed => "dismissed (won't be asked again)",
        Outcome::Snoozed => "snoozed",
        Outcome::Skipped => "skipped (asked again next session)",
        Outcome::Delegated => "handed to the agent (its findings come back here)",
    }
}

fn resolve(
    state: &SessionState,
    question: &QuestionRecord,
    outcome: Outcome,
    detail: &str,
) -> Step {
    let index = state.index + 1;
    let completed = index >= state.questions.len();
    let mut outcomes = state.outcomes.clone();
    outcomes.insert(question.id.clone(), outcome);
    let next = SessionState {
        index,
        outcomes,
        ended: completed.then_some(EndReason::Completed),
        notice: Some(format!("{}{}", past(outcome), detail)),
        ..state.clone()
    };
    Step {
        state: next,
        effects: completed
            .then_some(SessionEffect::End {
                reason: EndReason::Completed,
            })
            .into_iter()
            .collect(),
    }
}

fn save(state: &SessionState, span: DiffSpan, lines: Vec<String>, label: &str) -> Step {
    let saved = SessionState {
        saves: state.saves + 1,
        ..state.clone()
    };
    let Some(question) = current_question(state).cloned() else {
        return Step {
            state: SessionState {
                notice: Some(format!("saved ({label})")),
                ..saved
            },
            effects: Vec::new(),
        };
    };
    let mut step = resolve(&saved, &question, Outcome::Answered, &format!(" ({label})"));
    let mut effects = vec![SessionEffect::Answer {
        question,
        span,
        lines,
    }];
    effects.append(&mut step.effects);
    step.effects = effects;
    step
}

fn key(state: &SessionState, key: SessionKey) -> Step {
    if key == SessionKey::Quit {
        return Step {
            state: SessionState {
                ended: Some(EndReason::Quit),
                notice: None,
                ..state.clone()
            },
            effects: vec![SessionEffect::End {
                reason: EndReason::Quit,
            }],
        };
    }
    let Some(question) = current_question(state).cloned() else {
        return Step {
            state: SessionState {
                notice: Some("no question to act on; [q] ends the session".to_owned()),
                ..state.clone()
            },
            effects: Vec::new(),
        };
    };
    if key == SessionKey::Skip {
        return resolve(state, &question, Outcome::Skipped, "");
    }
    if key == SessionKey::Delegate && !state.can_delegate {
        return Step {
            state: SessionState {
                notice: Some("agents are off for this idea (tier 0)".to_owned()),
                ..state.clone()
            },
            effects: Vec::new(),
        };
    }
    let (outcome, effect) = match key {
        SessionKey::Dismiss => (
            Outcome::Dismissed,
            SessionEffect::Dismiss {
                question: question.clone(),
            },
        ),
        SessionKey::Snooze => (
            Outcome::Snoozed,
            SessionEffect::Snooze {
                question: question.clone(),
            },
        ),
        SessionKey::Delegate => (
            Outcome::Delegated,
            SessionEffect::Delegate {
                question: question.clone(),
            },
        ),
        SessionKey::Skip | SessionKey::Quit => unreachable!("handled above"),
    };
    let mut step = resolve(state, &question, outcome, "");
    let mut effects = vec![effect];
    effects.append(&mut step.effects);
    step.effects = effects;
    step
}

fn is_rule(question: &QuestionRecord) -> bool {
    question.by == "roots" || question.by.starts_with("roots:")
}

fn first_unreached(state: &SessionState) -> usize {
    if current_question(state).is_some() {
        state.index + 1
    } else {
        state.index
    }
}

fn arrive_one(state: &SessionState, question: QuestionRecord, cap: usize) -> Option<SessionState> {
    if state.questions.iter().any(|old| old.id == question.id) {
        return None;
    }
    let from = first_unreached(state);
    let mut questions = state.questions.clone();
    if questions.len() >= cap {
        let bump = (from..questions.len())
            .rev()
            .find(|index| is_rule(&questions[*index]));
        let bump = bump?;
        questions.remove(bump);
    }
    let at = (from..questions.len())
        .find(|index| is_rule(&questions[*index]))
        .unwrap_or(questions.len());
    questions.insert(at, question);
    Some(SessionState {
        questions,
        ..state.clone()
    })
}

fn arrive(state: &SessionState, incoming: Vec<QuestionRecord>, cap: usize) -> Step {
    let mut result = state.clone();
    let mut queued = 0;
    for question in incoming {
        if let Some(next) = arrive_one(&result, question, cap) {
            result = next;
            queued += 1;
        }
    }
    if queued == 0 {
        return Step {
            state: state.clone(),
            effects: Vec::new(),
        };
    }
    result.notice = Some(format!(
        "an agent asked {queued} new question{}",
        if queued == 1 { "" } else { "s" }
    ));
    Step {
        state: result,
        effects: Vec::new(),
    }
}

/// Advance one reducer input. Inputs after an end are deliberately ignored.
#[must_use]
pub fn step(state: &SessionState, input: SessionInput, label: &str) -> Step {
    if state.ended.is_some() {
        return Step {
            state: state.clone(),
            effects: Vec::new(),
        };
    }
    match input {
        SessionInput::Save { span, lines } => save(state, span, lines, label),
        SessionInput::Key(input_key) => key(state, input_key),
        SessionInput::Arrive { questions, cap } => arrive(state, questions, cap),
    }
}

#[must_use]
pub fn tally(state: &SessionState) -> SessionTally {
    let mut result = SessionTally::default();
    for question in &state.questions {
        match state.outcomes.get(&question.id) {
            Some(Outcome::Answered) => result.answered += 1,
            Some(Outcome::Dismissed) => result.dismissed += 1,
            Some(Outcome::Snoozed) => result.snoozed += 1,
            Some(Outcome::Skipped) => result.skipped += 1,
            Some(Outcome::Delegated) => result.delegated += 1,
            None => result.unasked += 1,
        }
    }
    result
}

/// Convert raw terminal bytes to the small key vocabulary used by `think`.
/// CSI/SS3/Alt escape sequences are discarded before ordinary letters are
/// considered, so an arrow key cannot accidentally trigger `a`.
#[must_use]
pub fn parse_keys(data: &str) -> Vec<SessionKey> {
    let bytes = data.as_bytes();
    let mut clean = String::new();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != 0x1b {
            let character = data[index..].chars().next().expect("UTF-8 boundary");
            clean.push(character);
            index += character.len_utf8();
            continue;
        }
        index += 1;
        match bytes.get(index).copied() {
            Some(b'[') => {
                index += 1;
                while index < bytes.len() {
                    let byte = bytes[index];
                    index += 1;
                    if (0x40..=0x7e).contains(&byte) {
                        break;
                    }
                }
            }
            Some(b'O') => {
                index = (index + 2).min(bytes.len());
            }
            Some(_) => {
                index += 1;
            }
            None => {}
        }
    }
    clean
        .chars()
        .filter_map(|character| match character.to_ascii_lowercase() {
            'd' => Some(SessionKey::Dismiss),
            'z' => Some(SessionKey::Snooze),
            's' => Some(SessionKey::Skip),
            'q' => Some(SessionKey::Quit),
            'a' => Some(SessionKey::Delegate),
            _ if matches!(character, '\u{3}' | '\u{4}') => Some(SessionKey::Quit),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use roots_core::model::QuestionStatus;

    fn question(id: &str, by: &str) -> QuestionRecord {
        QuestionRecord {
            id: id.to_owned(),
            node: "r-1".to_owned(),
            text: format!("{id}?"),
            by: by.to_owned(),
            status: QuestionStatus::Open,
            created_at: "".to_owned(),
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
    fn saves_and_keys_emit_durable_effects_in_order() {
        let one = question("q-1", "roots:missing-done");
        let two = question("q-2", "roots:missing-scope");
        let state = initial_state(vec![one.clone(), two.clone()], false);
        let saved = step(
            &state,
            SessionInput::Save {
                span: DiffSpan {
                    from: 2,
                    to: 2,
                    removed: 0,
                },
                lines: Vec::new(),
            },
            "line 2",
        );
        assert!(matches!(saved.effects[0], SessionEffect::Answer { .. }));
        let dismissed = step(&saved.state, SessionInput::Key(SessionKey::Dismiss), "");
        assert!(matches!(
            dismissed.effects[0],
            SessionEffect::Dismiss { .. }
        ));
        assert!(matches!(
            dismissed.effects[1],
            SessionEffect::End {
                reason: EndReason::Completed
            }
        ));
        assert_eq!(tally(&dismissed.state).answered, 1);
        assert_eq!(tally(&dismissed.state).dismissed, 1);
    }

    #[test]
    fn arrivals_go_after_current_before_unreached_rules() {
        let state = initial_state(
            vec![
                question("r1", "roots:missing-done"),
                question("r2", "roots:missing-scope"),
            ],
            true,
        );
        let next = step(
            &state,
            SessionInput::Arrive {
                questions: vec![question("a1", "agent:test")],
                cap: 3,
            },
            "",
        );
        assert_eq!(
            next.state
                .questions
                .iter()
                .map(|question| question.id.as_str())
                .collect::<Vec<_>>(),
            ["r1", "a1", "r2"]
        );
    }

    #[test]
    fn parse_keys_discards_terminal_escape_sequences() {
        assert_eq!(
            parse_keys("dZxs\u{3}q\x1b[1;5Aa\x1bOA\x1ba"),
            vec![
                SessionKey::Dismiss,
                SessionKey::Snooze,
                SessionKey::Skip,
                SessionKey::Quit,
                SessionKey::Quit,
                SessionKey::Delegate,
            ]
        );
    }
}
