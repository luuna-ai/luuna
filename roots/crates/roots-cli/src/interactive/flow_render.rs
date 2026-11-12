//! Flow transition-card renderer.

use super::flow::{FlowCard, FlowTrail, HeadingView, current_pick, enter_action};
use super::screen::{screen_width, truncate, wrap_text};
use roots_core::color::{Colors, pad_end, visible_length};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FlowFrame {
    pub project: String,
    pub minutes: u64,
}

fn fit(text: String, width: usize) -> String {
    let plain_width = visible_length(&text);
    if plain_width >= width {
        text
    } else {
        format!("{text}{}", " ".repeat(width - plain_width))
    }
}

fn trail_rows(trail: &FlowTrail, inner: usize, colors: Colors, limit: usize) -> Vec<String> {
    let mut rows = vec![colors.bold("trail")];
    if trail.entries.is_empty() {
        rows.push(colors.dim("  nothing yet: pick an idea below"));
        return rows;
    }
    let counts = trail
        .entries
        .iter()
        .map(|entry| {
            let open = (entry.open > 0).then(|| format!("{} open", entry.open));
            [
                format!("{} answered", entry.answered),
                open.unwrap_or_default(),
            ]
            .into_iter()
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join(" · ")
        })
        .collect::<Vec<_>>();
    let count_width = counts
        .iter()
        .map(|text| text.chars().count())
        .max()
        .unwrap_or_default();
    let status_width = trail
        .entries
        .iter()
        .map(|entry| entry.status.chars().count())
        .max()
        .unwrap_or_default();
    let shown_start = if limit < trail.entries.len() {
        trail
            .entries
            .len()
            .saturating_sub(limit.saturating_sub(1).max(1))
    } else {
        0
    };
    if shown_start > 0 {
        rows.push(colors.dim(&format!("  … {shown_start} earlier")));
    }
    for (index, entry) in trail.entries.iter().enumerate().skip(shown_start) {
        let mark = if trail.current.as_deref() == Some(entry.id.as_str()) {
            "  ← last"
        } else {
            ""
        };
        let tail = format!(
            "  {:count_width$}  {:status_width$}{mark}",
            counts[index], entry.status,
        );
        let room = inner
            .saturating_sub(
                2 + entry.id.chars().count() + 1 + tail.chars().count() - mark.chars().count() + 8,
            )
            .max(4);
        rows.push(format!(
            "  {} {}{}",
            colors.id(&entry.id),
            pad_end(&truncate(&entry.slug, room), room),
            colors.dim(&tail)
        ));
    }
    let extra = [
        (!trail.adopted.is_empty()).then(|| format!("adopted {}", trail.adopted.join(", "))),
        (!trail.planted.is_empty()).then(|| format!("planted {}", trail.planted.join(", "))),
        (trail.accepted > 0).then(|| format!("accepted {}", trail.accepted)),
        (trail.rejected > 0).then(|| format!("rejected {}", trail.rejected)),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>();
    if !extra.is_empty() {
        rows.push(colors.dim(&format!("  + {}", extra.join(" · "))));
    }
    rows
}

fn heading_rows(card: &FlowCard, inner: usize, colors: Colors) -> Vec<String> {
    match &card.heading {
        HeadingView::Off => Vec::new(),
        HeadingView::Waiting => vec![
            String::new(),
            colors.dim(&format!(
                "── heading ── [agent] {}",
                "─".repeat(inner.saturating_sub(22))
            )),
            colors.dim("the agent is reading your session…"),
        ],
        HeadingView::None => vec![
            String::new(),
            colors.dim(&format!(
                "── heading ── [agent] {}",
                "─".repeat(inner.saturating_sub(22))
            )),
            colors.dim("(no heading yet)"),
        ],
        HeadingView::Shown(heading) => {
            let label = "── heading ── [agent] ";
            let tail = " [x] dismiss ──";
            let divider = format!(
                "{}{}{}",
                label,
                "─".repeat(inner.saturating_sub(label.chars().count() + tail.chars().count())),
                tail
            );
            let mut rows = vec![String::new(), colors.dim(&divider)];
            rows.extend(wrap_text(&heading.text, inner));
            rows
        }
    }
}

fn status_rows(card: &FlowCard, inner: usize, colors: Colors) -> Vec<String> {
    let mut rows = Vec::new();
    if !card.jobs.is_empty() {
        rows.push(colors.dim(&format!("… {}", card.jobs.join(" · "))));
    }
    if let Some(notice) = &card.notice {
        let bad = notice.starts_with("! ");
        let text = if bad {
            notice.clone()
        } else {
            format!("✓ {notice}")
        };
        rows.extend(wrap_text(&text, inner).into_iter().map(|line| {
            if bad {
                colors.yellow(&line)
            } else {
                colors.green(&line)
            }
        }));
    }
    if rows.is_empty() {
        rows
    } else {
        let mut output = vec![String::new()];
        output.extend(rows);
        output
    }
}

fn key(key: &str, label: &str, colors: Colors, note: Option<&str>) -> String {
    let mut row = format!("{} {label}", colors.cyan(&format!("[{key}]")));
    if let Some(note) = note {
        row.push(' ');
        row.push_str(&colors.yellow(note));
    }
    row
}

fn key_rows(card: &FlowCard, colors: Colors) -> Vec<String> {
    let mut rows = Vec::new();
    let enter = enter_action(card);
    let pick = current_pick(card);
    if matches!(enter, Some(super::flow::FlowAction::Tend)) {
        rows.push(key(
            "enter",
            &format!("review inbox ({})", card.inbox),
            colors,
            None,
        ));
    }
    if let Some(pick) = pick {
        let is_enter = matches!(enter, Some(super::flow::FlowAction::Think { .. }));
        let label = if is_enter { "think" } else { "next" };
        let keyboard = if is_enter { "enter" } else { "n" };
        rows.push(key(
            keyboard,
            &format!("{label}: {} {} · {}", pick.id, pick.slug, pick.reason),
            colors,
            pick.from_heading.then_some("← heading"),
        ));
        if card.picks.len() > 1 {
            rows.push(key(
                "o",
                &format!("other idea ({} more)", card.picks.len() - 1),
                colors,
                None,
            ));
        }
    }
    let mut last = key("p", "plant", colors, None);
    if matches!(card.heading, HeadingView::Shown(_)) {
        last.push_str("   ");
        last.push_str(&key("x", "dismiss heading", colors, None));
    }
    last.push_str("   ");
    last.push_str(&key("q", "end flow", colors, None));
    if pick.is_none() && card.inbox == 0 {
        rows.push(key("enter", "plant an idea", colors, None));
    }
    rows.push(last);
    rows
}

#[must_use]
pub fn render_flow_card(
    card: &FlowCard,
    frame: &FlowFrame,
    columns: usize,
    colors: Colors,
    rows: usize,
) -> Vec<String> {
    let width = screen_width(columns);
    let inner = width - 4;
    let keys = key_rows(card, colors);
    let mut rest = heading_rows(card, inner, colors);
    rest.extend(status_rows(card, inner, colors));
    rest.push(String::new());
    let mut trail = trail_rows(&card.trail, inner, colors, usize::MAX);
    let room = if rows > 0 {
        rows.saturating_sub(keys.len() + 2)
    } else {
        usize::MAX
    };
    if trail.len().saturating_add(rest.len()) > room {
        let extra = trail.len().saturating_sub(1 + card.trail.entries.len());
        let limit = room.saturating_sub(rest.len() + 1 + extra).max(1);
        trail = trail_rows(&card.trail, inner, colors, limit);
    }
    let mut content = trail;
    content.extend(rest);
    content.truncate(room);
    let inbox = if card.inbox > 0 {
        format!(" ── inbox {}", card.inbox)
    } else {
        String::new()
    };
    let title = truncate(
        &format!(
            " luuna-roots flow ── {} ── {}m{inbox} ",
            frame.project, frame.minutes
        ),
        width - 4,
    );
    let top = format!(
        "┌─{title}{}┐",
        "─".repeat(width.saturating_sub(3 + title.chars().count()))
    );
    let spare = rows.saturating_sub(content.len() + keys.len() + 2);
    content.extend(std::iter::repeat_n(String::new(), spare));
    content.extend(keys);
    let body = content
        .into_iter()
        .map(|row| format!("│ {} │", fit(row, inner)))
        .collect::<Vec<_>>();
    let mut output = vec![colors.dim(&top)];
    output.extend(body);
    output.push(colors.dim(&format!("└{}┘", "─".repeat(width - 2))));
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interactive::flow::{FlowPick, initial_card};

    #[test]
    fn card_shows_trail_heading_jobs_and_keys() {
        let mut card = initial_card();
        card.trail.entries.push(super::super::flow::TrailEntry {
            id: "r-a1b2".to_owned(),
            slug: "offline".to_owned(),
            status: "shaping".to_owned(),
            sessions: 1,
            answered: 2,
            open: 1,
        });
        card.trail.current = Some("r-a1b2".to_owned());
        card.jobs = vec!["reading the session for a heading".to_owned()];
        card.picks = vec![FlowPick {
            id: "r-a1b2".to_owned(),
            slug: "offline".to_owned(),
            reason: "1 open question".to_owned(),
            from_heading: false,
        }];
        let lines = render_flow_card(
            &card,
            &FlowFrame {
                project: "test".to_owned(),
                minutes: 2,
            },
            72,
            Colors::new(false),
            24,
        );
        let rendered = lines.join("\n");
        assert!(rendered.contains("luuna-roots flow ── test ── 2m"));
        assert!(rendered.contains("2 answered · 1 open"));
        assert!(rendered.contains("reading the session for a heading"));
        assert!(rendered.contains("[enter] think: r-a1b2"));
    }
}
