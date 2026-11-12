//! Deterministic mention scanning and non-strict mention proposal filing.
//!
//! Mentions are advisory proposals owned by `roots:mention`; they never add
//! an accepted graph edge until a human decides the card.

use crate::claims::read_claim;
use crate::config::RootsConfig;
use crate::errors::RootsResult;
use crate::events::{EventFields, append_event, make_event};
use crate::ids::{IdPrefix, generate_id, hex_set};
use crate::model::{
    Citation, Graph, NodeKind, NodeStatus, ProposalKind, ProposalRecord, ProposalStatus,
};
use crate::paths::RootsPaths;
use crate::proposals::{
    MENTION_ACTOR, ProposalDraft, expires_at, find_rejection, is_expired, moot_reason,
    pending_proposals,
};
use crate::store::update_table;
use crate::time::iso_at;
use serde::Serialize;
use serde_json::{Map, Value};
use time::OffsetDateTime;

const STOPWORDS: &[&str] = &[
    "a", "an", "the", "and", "or", "but", "if", "then", "so", "to", "of", "in", "on", "at", "by",
    "for", "with", "from", "into", "onto", "over", "under", "is", "are", "was", "were", "be",
    "been", "being", "it", "its", "this", "that", "these", "those", "there", "here", "we", "i",
    "you", "he", "she", "they", "them", "our", "my", "your", "their", "not", "no", "do", "does",
    "did", "has", "have", "had", "can", "could", "should", "would", "will", "just", "very", "all",
    "any", "some", "each", "every", "more", "most", "less", "as", "than", "too", "also", "only",
    "about", "up", "down", "out", "when", "where", "what", "which", "who", "why", "how", "while",
    "always", "never",
];

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ChangedLine {
    pub line: usize,
    pub text: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct MentionTarget {
    pub id: String,
    pub slug: String,
    pub statement: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct Mention {
    pub target: String,
    pub line: usize,
    pub text: String,
    pub via: String,
    #[serde(rename = "match")]
    pub matched: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct MentionSkip {
    pub to: Option<String>,
    pub reason: String,
    pub message: String,
}

#[derive(Clone, Debug, Default)]
pub struct MentionFileResult {
    pub filed: Vec<ProposalRecord>,
    pub skipped: Vec<MentionSkip>,
    pub expired: Vec<ProposalRecord>,
}

#[derive(Clone, Debug)]
pub struct MentionFiling {
    pub mentions: Vec<Mention>,
    pub result: MentionFileResult,
}

#[must_use]
pub fn tokens(text: &str) -> Vec<String> {
    let mut output = Vec::new();
    let mut current = String::new();
    for character in text.chars() {
        if character.is_ascii_alphanumeric() {
            current.push(character.to_ascii_lowercase());
        } else if !current.is_empty() {
            output.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        output.push(current);
    }
    output
}

fn is_content(word: &str) -> bool {
    word.len() > 1 && !STOPWORDS.contains(&word)
}

#[must_use]
pub fn slug_phrase(slug: &str) -> Option<Vec<String>> {
    let mut parts = slug
        .split('-')
        .filter(|part| !part.is_empty())
        .map(|part| part.to_ascii_lowercase())
        .collect::<Vec<_>>();
    if parts.len() >= 3
        && parts
            .last()
            .is_some_and(|part| part.bytes().all(|byte| byte.is_ascii_digit()))
    {
        parts.pop();
    }
    (parts.len() >= 2 && parts.iter().any(|part| is_content(part))).then_some(parts)
}

#[must_use]
pub fn statement_phrases(statement: &str) -> Vec<Vec<String>> {
    let words = tokens(statement);
    words
        .windows(3)
        .filter(|gram| gram.iter().filter(|word| is_content(word)).count() >= 2)
        .map(|gram| gram.to_vec())
        .collect()
}

fn contains_phrase(words: &[String], phrase: &[String]) -> bool {
    words.windows(phrase.len()).any(|window| window == phrase)
}

fn id_in_line(line: &str, id: &str) -> bool {
    let line = line.to_ascii_lowercase();
    let id = id.to_ascii_lowercase();
    let mut offset = 0;
    while let Some(found) = line[offset..].find(&id) {
        let start = offset + found;
        let end = start + id.len();
        let before = line[..start].chars().next_back();
        let after = line[end..].chars().next();
        let valid_before =
            before.is_none_or(|character| !character.is_ascii_alphanumeric() && character != '-');
        let valid_after = after.is_none_or(|character| !character.is_ascii_alphanumeric());
        if valid_before && valid_after {
            return true;
        }
        offset = end;
    }
    false
}

fn target_match(target: &MentionTarget, line: &ChangedLine) -> Option<(String, String)> {
    if id_in_line(&line.text, &target.id) {
        return Some(("id".to_owned(), target.id.clone()));
    }
    let words = tokens(&line.text);
    if let Some(phrase) = slug_phrase(&target.slug) {
        if contains_phrase(&words, &phrase) {
            return Some(("slug".to_owned(), phrase.join(" ")));
        }
    }
    statement_phrases(&target.statement)
        .into_iter()
        .find(|phrase| contains_phrase(&words, phrase))
        .map(|phrase| ("phrase".to_owned(), phrase.join(" ")))
}

/// The first mentioning line for every live target other than `self_id`.
#[must_use]
pub fn find_mentions(
    lines: &[ChangedLine],
    targets: &[MentionTarget],
    self_id: &str,
) -> Vec<Mention> {
    let mut output = Vec::new();
    for target in targets {
        if target.id == self_id {
            continue;
        }
        for line in lines {
            let Some((via, matched)) = target_match(target, line) else {
                continue;
            };
            output.push(Mention {
                target: target.id.clone(),
                line: line.line,
                text: line.text.clone(),
                via,
                matched,
            });
            break;
        }
    }
    output
}

pub fn mention_targets(paths: &RootsPaths, graph: &Graph) -> RootsResult<Vec<MentionTarget>> {
    let mut output = Vec::new();
    for node in &graph.nodes {
        if node.kind != NodeKind::Idea
            || matches!(
                node.status,
                NodeStatus::Idea(crate::model::IdeaStatus::Composted)
            )
        {
            continue;
        }
        let claim = read_claim(paths, graph, node)?;
        if !claim.statement.is_empty() {
            output.push(MentionTarget {
                id: node.id.clone(),
                slug: node.slug.clone(),
                statement: claim.statement,
            });
        }
    }
    Ok(output)
}

#[must_use]
pub fn all_lines(text: &str) -> Vec<ChangedLine> {
    text.replace("\r\n", "\n")
        .split('\n')
        .enumerate()
        .filter_map(|(index, line)| {
            (!line.trim().is_empty()).then_some(ChangedLine {
                line: index + 1,
                text: line.to_owned(),
            })
        })
        .collect()
}

fn draft_for(from: &str, mention: &Mention, target: &MentionTarget) -> ProposalDraft {
    ProposalDraft {
        kind: ProposalKind::Edge,
        from: Some(from.to_owned()),
        to: Some(target.id.clone()),
        rel: None,
        reason: Some(format!(
            "mentions {} ({}: \"{}\")",
            target.slug, mention.via, mention.matched
        )),
        cites: vec![
            Citation {
                node: from.to_owned(),
                quote: mention.text.trim().to_owned(),
            },
            Citation {
                node: target.id.clone(),
                quote: target.statement.clone(),
            },
        ],
        by: MENTION_ACTOR.to_owned(),
    }
}

fn same_pair(proposal: &ProposalRecord, draft: &ProposalDraft) -> bool {
    (proposal.from == draft.from && proposal.to == draft.to)
        || (proposal.from == draft.to && proposal.to == draft.from)
}

fn mention_duplicate(
    rows: &[ProposalRecord],
    draft: &ProposalDraft,
    now: OffsetDateTime,
) -> Option<String> {
    pending_proposals(rows, now, None)
        .into_iter()
        .find(|proposal| {
            proposal.kind == ProposalKind::Edge
                && same_pair(proposal, draft)
                && proposal.by == MENTION_ACTOR
        })
        .map(|proposal| format!("already proposed as {}", proposal.id))
}

fn edge_exists(graph: &Graph, draft: &ProposalDraft) -> bool {
    graph.edges.iter().any(|edge| {
        (edge.from == draft.from.as_deref().unwrap_or_default()
            && edge.to == draft.to.as_deref().unwrap_or_default())
            || (edge.from == draft.to.as_deref().unwrap_or_default()
                && edge.to == draft.from.as_deref().unwrap_or_default())
    })
}

fn invalid_citation(paths: &RootsPaths, graph: &Graph, draft: &ProposalDraft) -> Option<String> {
    if draft.cites.len() < 2 {
        return Some("edge proposals need at least 2 citation(s)".to_owned());
    }
    for endpoint in [draft.from.as_deref(), draft.to.as_deref()]
        .into_iter()
        .flatten()
    {
        if !draft.cites.iter().any(|cite| cite.node == endpoint) {
            return Some(format!("cite at least one quote from {endpoint}"));
        }
    }
    for citation in &draft.cites {
        let Some(node) = graph.nodes.iter().find(|node| node.id == citation.node) else {
            return Some(format!("cited node {} does not exist", citation.node));
        };
        if citation.quote.trim().is_empty() {
            return Some(format!("empty cite quote for {}", citation.node));
        }
        match read_claim(paths, graph, node) {
            Ok(claim) if claim.text.contains(&citation.quote) => {}
            _ => return Some(format!("cite quote not found in {}", citation.node)),
        }
    }
    None
}

fn expire_rows(
    rows: &mut [ProposalRecord],
    graph: &Graph,
    now: OffsetDateTime,
) -> Vec<(ProposalRecord, String)> {
    let mut expired = Vec::new();
    for proposal in rows {
        if proposal.status != ProposalStatus::Pending {
            continue;
        }
        let reason = if is_expired(proposal, now) {
            Some("ttl".to_owned())
        } else {
            moot_reason(proposal, graph, now).map(|message| format!("moot: {message}"))
        };
        if let Some(reason) = reason {
            proposal.status = ProposalStatus::Expired;
            expired.push((proposal.clone(), reason));
        }
    }
    expired
}

fn expire_event(paths: &RootsPaths, proposal: &ProposalRecord, reason: String) -> RootsResult<()> {
    let mut extra = Map::new();
    extra.insert("proposal".to_owned(), Value::String(proposal.id.clone()));
    extra.insert("kind".to_owned(), Value::String("edge".to_owned()));
    extra.insert("rel".to_owned(), Value::Null);
    extra.insert("reason".to_owned(), Value::String(reason));
    append_event(
        paths,
        &make_event(
            "expire",
            "roots",
            EventFields {
                node: proposal.from.clone(),
                refs: proposal.to.clone().map(|id| vec![id]),
                extra,
                ..Default::default()
            },
        ),
    )
}

fn propose_event(paths: &RootsPaths, proposal: &ProposalRecord) -> RootsResult<()> {
    let mut extra = Map::new();
    extra.insert("proposal".to_owned(), Value::String(proposal.id.clone()));
    extra.insert("kind".to_owned(), Value::String("edge".to_owned()));
    // An automatic mention deliberately has no relation yet; JSON null is
    // observable in the event contract.
    extra.insert("rel".to_owned(), Value::Null);
    append_event(
        paths,
        &make_event(
            "propose",
            proposal.by.clone(),
            EventFields {
                node: proposal.from.clone(),
                refs: proposal.to.clone().map(|id| vec![id]),
                extra,
                ..Default::default()
            },
        ),
    )
}

/// File automatic mention proposals non-strictly.  Invalid/duplicate/rejected
/// drafts are returned as skips so `scan` can remain an idempotent operation.
pub fn file_mentions(
    paths: &RootsPaths,
    config: &RootsConfig,
    graph: &Graph,
    from: &str,
    lines: &[ChangedLine],
    now: OffsetDateTime,
) -> RootsResult<MentionFiling> {
    let targets = mention_targets(paths, graph)?;
    let mentions = find_mentions(lines, &targets, from);
    if mentions.is_empty() {
        return Ok(MentionFiling {
            mentions,
            result: MentionFileResult::default(),
        });
    }
    let drafts = mentions
        .iter()
        .filter_map(|mention| {
            targets
                .iter()
                .find(|target| target.id == mention.target)
                .map(|target| draft_for(from, mention, target))
        })
        .collect::<Vec<_>>();
    let (filed, skipped, expired) =
        update_table::<ProposalRecord, _, _>(&paths.proposals, |mut rows| {
            let expired = expire_rows(&mut rows, graph, now);
            let mut pending = pending_proposals(&rows, now, None).len() as u64;
            let mut filed = Vec::new();
            let mut skipped = Vec::new();
            for draft in &drafts {
                let skip = if let Some(message) = invalid_citation(paths, graph, draft) {
                    Some(("invalid", message))
                } else if let Some(rejected) = find_rejection(&rows, draft) {
                    Some((
                        "rejected",
                        format!("a human already rejected this ({})", rejected.id),
                    ))
                } else if let Some(message) = mention_duplicate(&rows, draft, now) {
                    Some(("duplicate", message))
                } else if edge_exists(graph, draft) {
                    Some(("exists", "these nodes are already linked".to_owned()))
                } else if pending >= config.limits.proposals {
                    Some((
                        "cap",
                        format!(
                            "pending proposals are at the limit ({}); run `luuna-roots tend`",
                            config.limits.proposals
                        ),
                    ))
                } else {
                    None
                };
                if let Some((reason, message)) = skip {
                    skipped.push(MentionSkip {
                        to: draft.to.clone(),
                        reason: reason.to_owned(),
                        message,
                    });
                    continue;
                }
                let taken = hex_set(rows.iter().map(|row| row.id.as_str()));
                let record = ProposalRecord {
                    id: generate_id(IdPrefix::Proposal, &taken),
                    kind: ProposalKind::Edge,
                    from: draft.from.clone(),
                    to: draft.to.clone(),
                    rel: None,
                    reason: draft.reason.clone(),
                    cites: draft.cites.clone(),
                    by: MENTION_ACTOR.to_owned(),
                    status: ProposalStatus::Pending,
                    created_at: iso_at(now),
                    expires_at: Some(expires_at(now, config.limits.proposal_ttl_days)),
                    decided_by: None,
                    decided_at: None,
                    decision_reason: None,
                };
                rows.push(record.clone());
                filed.push(record);
                pending += 1;
            }
            Ok((
                rows,
                !expired.is_empty() || !filed.is_empty(),
                (filed, skipped, expired),
            ))
        })?;
    for (proposal, reason) in &expired {
        expire_event(paths, proposal, reason.clone())?;
    }
    for proposal in &filed {
        propose_event(paths, proposal)?;
    }
    Ok(MentionFiling {
        mentions,
        result: MentionFileResult {
            filed,
            skipped,
            expired: expired.into_iter().map(|(proposal, _)| proposal).collect(),
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_ids_slugs_and_statement_phrases_on_word_boundaries() {
        let target = MentionTarget {
            id: "r-a1b2".into(),
            slug: "offline-sync".into(),
            statement: "Sync works offline without data loss".into(),
        };
        let lines = all_lines("r-a1b2\noffline sync is useful\nsync works offline for us\n");
        let mentions = find_mentions(&lines, &[target], "r-z9y8");
        assert_eq!(mentions[0].via, "id");
        assert!(!id_in_line("x-r-a1b2", "r-a1b2"));
    }
}
