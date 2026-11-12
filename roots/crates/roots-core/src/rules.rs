//! Deterministic question-rule evaluation used by think and queue.

use crate::config::QuestionThresholds;
use crate::model::{EdgeRecord, NodeKind, NodeRecord, QuestionRecord, QuestionStatus};
use crate::prose::parse_prose;
use crate::time_crate::OffsetDateTime;
use serde::Serialize;

pub const RULES: [&str; 6] = [
    "stale",
    "missing-done",
    "missing-scope",
    "too-big",
    "tension-open",
    "orphan",
];

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RuleCandidate {
    pub rule: String,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub r#ref: Option<String>,
}

pub struct RuleContext<'a> {
    pub node: &'a NodeRecord,
    pub text: &'a str,
    pub edges: Vec<&'a EdgeRecord>,
    pub lookup: &'a dyn Fn(&str) -> Option<&'a NodeRecord>,
    pub session_number: u64,
    pub last_touched: &'a str,
    pub now: OffsetDateTime,
    pub thresholds: &'a QuestionThresholds,
}

#[must_use]
pub fn days_since(iso: &str, now: OffsetDateTime) -> f64 {
    let parsed = OffsetDateTime::parse(iso, &time::format_description::well_known::Rfc3339);
    parsed
        .map(|at| (now - at).whole_nanoseconds() as f64 / 86_400_000_000_000_f64)
        .unwrap_or(0.0)
}

#[must_use]
pub fn has_done_language(text: &str) -> bool {
    // The JavaScript regex accepts these common inflections.  Splitting on
    // word-like chunks gives the same practical matching without a regex dep.
    let lower = text.to_lowercase();
    let words = lower
        .split(|character: char| !character.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>();
    words.iter().any(|word| {
        matches!(
            *word,
            "done"
                | "finished"
                | "complete"
                | "completed"
                | "success"
                | "successful"
                | "ship"
                | "ships"
                | "shipped"
        )
    }) || lower.contains("good enough")
}

#[must_use]
pub fn has_scope_language(text: &str) -> bool {
    let lower = text.to_lowercase();
    lower
        .split(|character: char| !character.is_alphanumeric() && character != '\'')
        .any(|word| matches!(word, "not" | "won't" | "won’t" | "non" | "goals"))
        || lower.contains("won't")
        || lower.contains("won’t")
        || lower.contains("out of scope")
        || lower.contains("non-goal")
}

#[must_use]
pub fn body_line_count(text: &str) -> usize {
    parse_prose(text)
        .body
        .lines()
        .filter(|line| !line.trim().is_empty())
        .count()
}

#[must_use]
pub fn looks_too_big(text: &str, max_body_lines: u64) -> bool {
    parse_prose(text)
        .statement
        .to_lowercase()
        .split(|character: char| !character.is_alphanumeric())
        .any(|word| word == "and")
        || body_line_count(text) > max_body_lines as usize
}

fn content_rules(context: &RuleContext<'_>) -> Vec<RuleCandidate> {
    let mut output = Vec::new();
    if context.session_number >= context.thresholds.done_after_sessions
        && !has_done_language(context.text)
    {
        output.push(RuleCandidate {
            rule: "missing-done".to_owned(),
            text: "What would make this done?".to_owned(),
            r#ref: None,
        });
    }
    if !has_scope_language(context.text) {
        output.push(RuleCandidate {
            rule: "missing-scope".to_owned(),
            text: "What is this not trying to solve?".to_owned(),
            r#ref: None,
        });
    }
    if looks_too_big(context.text, context.thresholds.big_body_lines) {
        output.push(RuleCandidate {
            rule: "too-big".to_owned(),
            text: "Is this one idea or two?".to_owned(),
            r#ref: None,
        });
    }
    output
}

fn tension_rules(context: &RuleContext<'_>) -> Vec<RuleCandidate> {
    context
        .edges
        .iter()
        .filter(|edge| edge.rel.as_str() == "tension")
        .filter_map(|edge| {
            let id = if edge.from == context.node.id {
                &edge.to
            } else {
                &edge.from
            };
            let other = (context.lookup)(id)?;
            (other.status.as_str() != "composted").then(|| RuleCandidate {
                rule: "tension-open".to_owned(),
                text: format!("Which of these wins when it conflicts with {}?", other.slug),
                r#ref: Some(edge.id.clone()),
            })
        })
        .collect()
}

fn time_rules(context: &RuleContext<'_>) -> Vec<RuleCandidate> {
    let mut output = Vec::new();
    if context.node.status.as_str() == "committed"
        && days_since(context.last_touched, context.now)
            >= context.thresholds.stale_after_days as f64
    {
        output.push(RuleCandidate {
            rule: "stale".to_owned(),
            text: "Is this still true?".to_owned(),
            r#ref: None,
        });
    }
    if context.edges.is_empty()
        && days_since(&context.node.created_at, context.now)
            >= context.thresholds.orphan_after_days as f64
    {
        output.push(RuleCandidate {
            rule: "orphan".to_owned(),
            text: "What does this serve?".to_owned(),
            r#ref: None,
        });
    }
    output
}

#[must_use]
pub fn evaluate_rules(context: &RuleContext<'_>) -> Vec<RuleCandidate> {
    if context.node.kind != NodeKind::Idea || context.node.status.as_str() == "composted" {
        return Vec::new();
    }
    let mut candidates = content_rules(context);
    candidates.extend(tension_rules(context));
    candidates.extend(time_rules(context));
    candidates.sort_by_key(|candidate| {
        RULES
            .iter()
            .position(|rule| *rule == candidate.rule)
            .unwrap_or(RULES.len())
    });
    candidates
}

#[must_use]
pub fn rule_actor(rule: &str) -> String {
    format!("roots:{rule}")
}

#[must_use]
pub fn rule_of(question: &QuestionRecord) -> Option<&str> {
    let name = question.by.strip_prefix("roots:")?;
    RULES.contains(&name).then_some(name)
}

fn question_key(node: &str, by: &str, reference: Option<&str>) -> String {
    format!("{node}|{by}|{}", reference.unwrap_or_default())
}

fn time_rule(rule: &str) -> bool {
    matches!(rule, "orphan" | "stale")
}

fn threshold_days(rule: &str, thresholds: &QuestionThresholds) -> u64 {
    if rule == "stale" {
        thresholds.stale_after_days
    } else {
        thresholds.orphan_after_days
    }
}

fn blocks(question: &QuestionRecord, candidate: &RuleCandidate, context: &RuleContext<'_>) -> bool {
    if question.status != QuestionStatus::Answered {
        return true;
    }
    if !time_rule(&candidate.rule) {
        return true;
    }
    let at = question
        .answered_at
        .as_deref()
        .unwrap_or(&question.created_at);
    days_since(at, context.now) < threshold_days(&candidate.rule, context.thresholds) as f64
}

/// Rule candidates that are not already permanently or temporarily covered.
#[must_use]
pub fn new_candidates(
    context: &RuleContext<'_>,
    existing: &[QuestionRecord],
) -> Vec<RuleCandidate> {
    evaluate_rules(context)
        .into_iter()
        .filter(|candidate| {
            let key = question_key(
                &context.node.id,
                &rule_actor(&candidate.rule),
                candidate.r#ref.as_deref(),
            );
            !existing
                .iter()
                .filter(|question| question.node == context.node.id)
                .filter(|question| {
                    question_key(&question.node, &question.by, question.r#ref.as_deref()) == key
                })
                .any(|question| blocks(question, candidate, context))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::DEFAULT_QUESTION_THRESHOLDS;
    use crate::model::{IdeaStatus, NodeStatus};

    #[test]
    fn content_language_and_line_thresholds_are_detected() {
        assert!(has_done_language("This is good enough to ship."));
        assert!(has_scope_language("Non-goals: no sync."));
        assert!(looks_too_big("A and B", 30));
        let node = NodeRecord::new(
            "r-a1b2".into(),
            NodeKind::Idea,
            "idea".into(),
            NodeStatus::Idea(IdeaStatus::Planted),
            "human:t".into(),
            "2026-01-01T00:00:00Z".into(),
        );
        let lookup = |_id: &str| None;
        let candidates = evaluate_rules(&RuleContext {
            node: &node,
            text: "A claim",
            edges: vec![],
            lookup: &lookup,
            session_number: 1,
            last_touched: &node.created_at,
            now: OffsetDateTime::now_utc(),
            thresholds: &DEFAULT_QUESTION_THRESHOLDS,
        });
        assert_eq!(candidates[0].rule, "missing-done");
    }
}
