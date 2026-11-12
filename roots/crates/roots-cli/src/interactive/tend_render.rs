//! Rendering for the interactive human `tend` review cards.

use super::screen::{truncate, wrap_text};
use roots_core::color::{Colors, visible_length};
use roots_core::presentation::edge_label;
use roots_core::tend::{
    ProposalCard, SproutCard, TendCard, TendState, card_keys, current_card, days_left,
    expiring_soon,
};
use roots_core::time_crate::OffsetDateTime;

pub const CARD_MIN_WIDTH: usize = 40;
pub const CARD_MAX_WIDTH: usize = 100;

#[derive(Clone, Debug)]
pub struct TendView<'a> {
    pub state: &'a TendState,
    pub now: OffsetDateTime,
    pub expired: usize,
    pub expired_sprouts: usize,
}

fn source_label(sources: &[String]) -> String {
    sources
        .iter()
        .map(|source| {
            source
                .strip_prefix("agent:")
                .map_or_else(|| source.clone(), |agent| format!("[agent] {agent}"))
        })
        .map(|source| {
            if source == "roots:mention" {
                "mention".to_owned()
            } else {
                source
            }
        })
        .collect::<Vec<_>>()
        .join(" + ")
}

fn relation_line(card: &ProposalCard) -> String {
    let left = &card.from.slug;
    let right = card.to.as_ref().map_or("", |node| node.slug.as_str());
    match card.kind {
        roots_core::model::ProposalKind::Edge => match card.rel {
            None => format!("{left}  ─?─  {right}"),
            Some(rel) => {
                let (label, arrow) = edge_label(rel, "out");
                format!("{left}  {arrow} {label} {arrow}  {right}")
            }
        },
        roots_core::model::ProposalKind::Merge => format!("merge  {left}  ⇄  {right}"),
        roots_core::model::ProposalKind::Split => format!("split  {left}"),
        roots_core::model::ProposalKind::Compost => format!("compost  {left}"),
    }
}

fn sprout_body(card: &SproutCard, inner: usize, colors: Colors) -> Vec<String> {
    let mut lines = wrap_text(
        if card.statement.is_empty() {
            "(empty)"
        } else {
            &card.statement
        },
        inner,
    )
    .into_iter()
    .map(|line| colors.bold(&line))
    .collect::<Vec<_>>();
    let body = card
        .body
        .split('\n')
        .filter(|line| !line.trim().is_empty())
        .flat_map(|line| wrap_text(line, inner))
        .collect::<Vec<_>>();
    if !body.is_empty() {
        lines.push(String::new());
        lines.extend(body);
    }
    lines.push(String::new());
    lines.extend(
        wrap_text(
            "[t] keeps this as the agent's text, quoted, plus your thoughts · [y] you rewrite it (nothing copied)",
            inner,
        )
        .into_iter()
        .map(|line| colors.dim(&line)),
    );
    lines
}

fn proposal_body(card: &ProposalCard, inner: usize, colors: Colors) -> Vec<String> {
    let relation = relation_line(card);
    let mut lines = (if relation.chars().count() <= inner {
        vec![relation]
    } else {
        wrap_text(&relation, inner)
    })
    .into_iter()
    .map(|line| colors.bold(&line))
    .collect::<Vec<_>>();
    lines.push(String::new());
    if let Some(reason) = &card.reason {
        lines.extend(wrap_text(&format!("\"{reason}\""), inner));
    }
    for note in &card.notes {
        lines.extend(
            wrap_text(note, inner)
                .into_iter()
                .map(|line| colors.dim(&line)),
        );
    }
    for cite in &card.cites {
        lines.extend(
            wrap_text(
                &format!("{}: \"{}\"", cite.node, cite.quote),
                inner.saturating_sub(2),
            )
            .into_iter()
            .map(|line| format!("  {line}")),
        );
    }
    if card.pick_rel {
        if let Some(rel) = card.rel {
            lines.push(String::new());
            lines.push(colors.dim(&format!("suggested: {}; or pick one", rel.as_str())));
        }
    }
    lines
}

fn card_body(card: &TendCard, inner: usize, colors: Colors) -> Vec<String> {
    match card {
        TendCard::Proposal(card) => proposal_body(card, inner, colors),
        TendCard::Sprout(card) => sprout_body(card, inner, colors),
    }
}

fn key_lines(card: &TendCard, inner: usize, colors: Colors) -> Vec<String> {
    let mut rows = Vec::new();
    let mut current = String::new();
    let mut used = 0;
    for key in card_keys(card) {
        let plain = format!("[{}] {}", key.key, key.label);
        let width = plain.chars().count();
        if used > 0 && used + 2 + width > inner {
            rows.push(current);
            current = String::new();
            used = 0;
        }
        if used > 0 {
            current.push_str("  ");
        }
        current.push_str(&colors.cyan(&format!("[{}]", key.key)));
        current.push(' ');
        current.push_str(&key.label);
        used += if used > 0 { 2 } else { 0 } + width;
    }
    if !current.is_empty() {
        rows.push(current);
    }
    rows
}

fn box_line(content: &str, inner: usize) -> String {
    let text = if visible_length(content) > inner {
        // All content that can overflow in normal use is unstyled.  This
        // keeps the no-colour and test terminal layout exact; styled content
        // is normally already wrapped before it reaches the box.
        truncate(content, inner)
    } else {
        content.to_owned()
    };
    format!(
        "│ {text}{} │",
        " ".repeat(inner.saturating_sub(visible_length(&text)))
    )
}

fn header(card: &TendCard, now: OffsetDateTime, width: usize, colors: Colors) -> String {
    let days = days_left(card.expires_at(), now);
    let head = match card {
        TendCard::Sprout(card) => format!("sprout {}", card.id),
        TendCard::Proposal(card) => format!("proposal {}", card.ids.join(" + ")),
    };
    let sources = match card {
        TendCard::Sprout(card) => vec![card.author.clone()],
        TendCard::Proposal(card) => card.sources.clone(),
    };
    let tail = days.map_or_else(String::new, |days| format!(" ── expires in {days}d"));
    let room =
        width as isize - 8 - head.chars().count() as isize - tail.chars().count() as isize - 4;
    let source = if room > 3 {
        format!(" ── {}", truncate(&source_label(&sources), room as usize))
    } else {
        String::new()
    };
    let title = truncate(&format!(" {head}{source}{tail} "), width.saturating_sub(4));
    let fill = "─".repeat(width.saturating_sub(3 + title.chars().count()));
    let title = if expiring_soon(card, now) {
        colors.yellow(&title)
    } else {
        title
    };
    format!(
        "{}{}{}",
        colors.dim("┌─"),
        title,
        colors.dim(&format!("{fill}┐"))
    )
}

fn clamp_body(body: Vec<String>, room: isize, colors: Colors) -> Vec<String> {
    if room >= 0 && body.len() <= room as usize {
        return body;
    }
    let keep = std::cmp::max(1, room - 1) as usize;
    let more = body.len().saturating_sub(keep);
    let mut result = body.into_iter().take(keep).collect::<Vec<_>>();
    result.push(colors.dim(&format!(
        "… {more} more line{}",
        if more == 1 { "" } else { "s" }
    )));
    result
}

/// Render the current card at the requested dimensions.
#[must_use]
pub fn render_tend(view: TendView<'_>, columns: usize, colors: Colors, rows: usize) -> Vec<String> {
    let width = columns.clamp(CARD_MIN_WIDTH, CARD_MAX_WIDTH);
    let inner = width - 4;
    let Some(card) = current_card(view.state) else {
        let mut top = vec![format!(
            "{}{}",
            colors.bold("luuna-roots tend"),
            colors.dim(&format!(
                "  card {}/{}",
                view.state.index + 1,
                view.state.cards.len()
            ))
        )];
        if view.expired > 0 {
            top.push(colors.dim(&format!(
                "{} proposal{} expired",
                view.expired,
                if view.expired == 1 { "" } else { "s" }
            )));
        }
        if view.expired_sprouts > 0 {
            top.push(colors.dim(&format!(
                "{} sprout{} expired",
                view.expired_sprouts,
                if view.expired_sprouts == 1 { "" } else { "s" }
            )));
        }
        return top;
    };
    let mut top = vec![format!(
        "{}{}",
        colors.bold("luuna-roots tend"),
        colors.dim(&format!(
            "  card {}/{}",
            view.state.index + 1,
            view.state.cards.len()
        ))
    )];
    if view.expired > 0 {
        top.push(colors.dim(&format!(
            "{} proposal{} expired",
            view.expired,
            if view.expired == 1 { "" } else { "s" }
        )));
    }
    if view.expired_sprouts > 0 {
        top.push(colors.dim(&format!(
            "{} sprout{} expired",
            view.expired_sprouts,
            if view.expired_sprouts == 1 { "" } else { "s" }
        )));
    }
    let mut content = card_body(card, inner, colors);
    let mut keys = vec![String::new()];
    keys.extend(key_lines(card, inner, colors));
    let mut footer = Vec::new();
    if let Some(reason) = &view.state.reason {
        footer.push(format!("reason: {reason}█"));
        footer.push(colors.dim("[enter] reject  [esc] cancel"));
    }
    if let Some(thoughts) = &view.state.thoughts {
        footer.extend(wrap_text(&format!("thoughts: {thoughts}█"), width));
        footer.push(
            colors.dim("[enter] accept with thoughts  [ctrl-e] continue in $EDITOR  [esc] cancel"),
        );
    }
    if view.state.busy {
        footer.push(colors.dim("working…"));
    }
    if let Some(error) = &view.state.error {
        footer.push(colors.red(&format!("! {error}")));
    } else if let Some(notice) = &view.state.notice {
        footer.push(colors.green(&format!("✓ {notice}")));
    }
    footer.push(colors.dim("[q] quit"));
    let fixed = top.len() + 1 + 2 + keys.len() + footer.len();
    if rows > 0 {
        content = clamp_body(content, rows as isize - fixed as isize, colors);
    }
    let used = fixed + content.len();
    let filler = std::iter::repeat_n(String::new(), rows.saturating_sub(used)).collect::<Vec<_>>();
    let mut output = top;
    output.push(String::new());
    output.push(header(card, view.now, width, colors));
    output.extend(
        content
            .into_iter()
            .chain(filler)
            .chain(keys)
            .map(|line| box_line(&line, inner)),
    );
    output.push(colors.dim(&format!("└{}┘", "─".repeat(width - 2))));
    output.extend(footer);
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use roots_core::model::{Citation, ProposalKind, ProposalRecord, ProposalStatus};
    use roots_core::tend::{CardNode, TendCard, initial_tend, proposal_cards};

    fn card() -> TendCard {
        let proposal = ProposalRecord {
            id: "p-0001".to_owned(),
            kind: ProposalKind::Edge,
            from: Some("r-a".to_owned()),
            to: Some("r-b".to_owned()),
            rel: Some(roots_core::model::EdgeRel::Tension),
            reason: Some("offline writes conflict".to_owned()),
            cites: vec![Citation {
                node: "r-a".to_owned(),
                quote: "offline".to_owned(),
            }],
            by: "agent:opus".to_owned(),
            status: ProposalStatus::Pending,
            created_at: "2026-09-28T00:00:00Z".to_owned(),
            expires_at: Some("2026-10-09T00:00:00Z".to_owned()),
            decided_by: None,
            decided_at: None,
            decision_reason: None,
        };
        TendCard::Proposal(
            proposal_cards(&[&proposal], |id| CardNode {
                id: id.to_owned(),
                slug: if id == "r-a" {
                    "offline-sync"
                } else {
                    "server"
                }
                .to_owned(),
            })[0]
                .clone(),
        )
    }

    #[test]
    fn fills_height_and_keeps_quit_on_last_line() {
        let state = initial_tend(vec![card()]);
        let now = OffsetDateTime::parse(
            "2026-09-28T12:00:00Z",
            &time::format_description::well_known::Rfc3339,
        )
        .unwrap();
        let lines = render_tend(
            TendView {
                state: &state,
                now,
                expired: 2,
                expired_sprouts: 0,
            },
            70,
            Colors::new(false),
            40,
        );
        assert_eq!(lines.len(), 40);
        assert_eq!(lines.last().map(String::as_str), Some("[q] quit"));
        assert!(
            lines
                .iter()
                .any(|line| line.contains("offline-sync  ↔ tension ↔  server"))
        );
        assert!(
            lines
                .iter()
                .filter(|line| line.starts_with('│'))
                .all(|line| line.chars().count() == 70)
        );
    }
}
