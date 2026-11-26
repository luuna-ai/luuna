//! Terminal-cell adaptation of LUUNA-AI's blue desktop and framed workbench.
//! This module only displays bridge snapshots; it never decides runtime policy.
mod board;
mod collection;
mod detail;
mod help;
mod home;
mod orbit;
use super::{hints, orbit_tone, rows, selected, snapshot, Screen, EVIDENCE, NAV};
use crate::{
    app::AppState,
    protocol::safe_text,
    theme::{state_tone, Theme},
};
use board::*;
use collection::*;
use detail::*;
use help::*;
use home::*;
use orbit::*;
use ratatui::{
    layout::{Constraint, Layout, Rect},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, List, ListItem, ListState, Paragraph, Wrap},
    Frame,
};
use serde_json::Value;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

pub fn render(frame: &mut Frame, app: &mut AppState, area: Rect, theme: Theme) {
    let content = if area.width >= 100 {
        let [navigation, content] =
            Layout::horizontal([Constraint::Percentage(26), Constraint::Percentage(74)])
                .areas(area);
        render_navigation(frame, app, navigation, theme);
        content
    } else if app.workbench.nav {
        render_navigation(frame, app, area, theme);
        return;
    } else {
        area
    };
    let content = if let Some(error) = &app.workbench.error {
        let [notice, rest] =
            Layout::vertical([Constraint::Length(2), Constraint::Min(1)]).areas(content);
        frame.render_widget(
            Paragraph::new(format!("! {}", safe_text(error)))
                .style(theme.tone("danger"))
                .wrap(Wrap { trim: false }),
            notice,
        );
        rest
    } else {
        content
    };
    let screen = app.workbench.screen;
    let title = match screen {
        Screen::Home => "Home / workspace health",
        Screen::Projects => "Projects / repositories",
        Screen::Project => "Project / recorded identity",
        Screen::Orbit => "Orbit / project repositories",
        Screen::Missions => "Missions / workflow board",
        Screen::Ticket => "Missions / ticket",
        Screen::Memory => "Atlas / scoped memory",
        Screen::MemoryDetail => "Atlas / record",
        Screen::Prompts => "Beacon / versioned prompts",
        Screen::PromptDetail => "Beacon / exact preview",
        Screen::Runs => "Runs / recorded workflows",
        Screen::RunDetail => "Run / evidence",
        Screen::Plan => "Plan / immutable snapshot",
        Screen::Commands => "Commands",
        Screen::Help => "Help / keyboard reference",
        Screen::Setup => "Setup",
    };
    let block = panel(title, !app.workbench.nav, theme);
    let inside = block.inner(content);
    frame.render_widget(block, content);
    if inside.height == 0 || inside.width == 0 {
        return;
    }
    let [body, keys] = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(inside);
    frame.render_widget(Paragraph::new(hints(screen)).style(theme.accent()), keys);
    match screen {
        Screen::Missions => render_board(frame, app, body, theme),
        Screen::Home => render_home(frame, app, body, theme),
        Screen::Projects | Screen::Memory | Screen::Prompts | Screen::Runs => {
            render_collection(frame, app, body, theme)
        }
        Screen::RunDetail => render_run(frame, app, body, theme),
        Screen::Orbit => render_orbit(frame, app, body, theme),
        Screen::Help => render_help(frame, app, body, theme),
        Screen::Commands => {
            paragraph(
                frame,
                vec![Line::raw("F2 opens all runtime commands.")],
                body,
                &mut app.scroll,
                theme,
            );
        }
        _ => {
            let record = selected(app);
            let mut lines = detail_lines(screen, &record, theme);
            if matches!(screen, Screen::Ticket | Screen::Plan) {
                append_context(&mut lines, app, theme);
            }
            if screen == Screen::MemoryDetail {
                lines.insert(
                    0,
                    field(
                        "Unsent context",
                        if app.workbench.memory_ids.contains(&text(&record, "id")) {
                            "selected / s removes"
                        } else {
                            "not selected / s selects"
                        },
                        theme,
                    ),
                );
            }
            if screen == Screen::PromptDetail {
                let identity = format!(
                    "{}@{}#{}",
                    text(&record, "name"),
                    text(&record, "version"),
                    text(&record, "hash")
                );
                lines.insert(
                    0,
                    field(
                        "Unsent context",
                        if app.workbench.prompt_selection.as_deref() == Some(identity.as_str()) {
                            "selected / s removes"
                        } else {
                            "not selected / s selects"
                        },
                        theme,
                    ),
                );
            }
            paragraph(frame, lines, body, &mut app.scroll, theme);
        }
    }
}

fn panel(title: &str, focus: bool, theme: Theme) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Double)
        .border_style(if focus {
            theme.focused()
        } else {
            theme.muted()
        })
        .title(Line::styled(
            format!(" {}{} ", if focus { "> " } else { "" }, safe_text(title)),
            if focus { theme.title() } else { theme.muted() },
        ))
        .style(theme.panel())
}

fn render_navigation(frame: &mut Frame, app: &AppState, area: Rect, theme: Theme) {
    let block = panel("LUUNA", app.workbench.nav, theme);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let context_height = if inner.height >= 18 { 7 } else { 0 };
    let [list_area, context_area] =
        Layout::vertical([Constraint::Min(1), Constraint::Length(context_height)]).areas(inner);
    let items = NAV
        .iter()
        .enumerate()
        .map(|(i, (label, screen))| {
            let active = *screen == app.workbench.screen
                || match screen {
                    Screen::Projects => app.workbench.screen == Screen::Project,
                    Screen::Missions => {
                        matches!(app.workbench.screen, Screen::Ticket | Screen::Plan)
                    }
                    Screen::Memory => app.workbench.screen == Screen::MemoryDetail,
                    Screen::Prompts => app.workbench.screen == Screen::PromptDetail,
                    Screen::Runs => app.workbench.screen == Screen::RunDetail,
                    _ => false,
                };
            ListItem::new(Line::from(vec![
                Span::styled(format!(" {} ", i + 1), theme.accent()),
                Span::styled(
                    format!("{label}{}", if active { " *" } else { "" }),
                    if active { theme.title() } else { theme.panel() },
                ),
            ]))
        })
        .collect::<Vec<_>>();
    let mut state = ListState::default().with_selected(if app.workbench.nav {
        Some(app.workbench.nav_cursor)
    } else {
        None
    });
    frame.render_stateful_widget(
        List::new(items)
            .highlight_style(theme.selected())
            .highlight_symbol(">"),
        list_area,
        &mut state,
    );
    if context_height > 0 {
        let mut lines = vec![
            heading("PROJECT", theme),
            Line::raw(safe_text(
                app.context
                    .get("project")
                    .map(String::as_str)
                    .unwrap_or("Choose a project"),
            )),
            Line::raw(""),
            heading("UNSENT PLAN CONTEXT", theme),
            Line::raw(format!(
                "{} memory selection(s)",
                app.workbench.memory_ids.len()
            )),
        ];
        lines.push(Line::raw(
            app.workbench
                .prompt_selection
                .as_deref()
                .map(safe_text)
                .unwrap_or_else(|| "No prompt selected".into()),
        ));
        lines.push(Line::styled("F10 menu / Tab focus", theme.muted()));
        frame.render_widget(
            Paragraph::new(lines).wrap(Wrap { trim: false }),
            context_area,
        );
    }
}

fn paragraph(
    frame: &mut Frame,
    lines: Vec<Line<'static>>,
    area: Rect,
    scroll: &mut crate::scroll::ScrollState,
    theme: Theme,
) {
    let lines = cell_lines(lines, area.width);
    let max_scroll = lines
        .len()
        .saturating_sub(usize::from(area.height))
        .min(usize::from(u16::MAX)) as u16;
    let scroll = scroll.clamp(max_scroll);
    frame.render_widget(
        Paragraph::new(lines)
            .style(theme.panel())
            .scroll((scroll, 0)),
        area,
    );
}

// Wrap styled spans into terminal rows: complete hashes stay readable and End
// has an exact bound, including wide Unicode and newline-delimited evidence.
fn cell_lines(lines: Vec<Line<'static>>, width: u16) -> Vec<Line<'static>> {
    let width = usize::from(width.max(1));
    let mut result = Vec::new();
    for line in lines {
        let mut row = Line::default().style(line.style);
        let mut used = 0;
        for span in line.spans {
            let mut part = String::new();
            for character in span.content.chars() {
                let cells = character.width().unwrap_or(0);
                if character == '\n' || used + cells > width {
                    row.spans
                        .push(Span::styled(std::mem::take(&mut part), span.style));
                    result.push(row);
                    row = Line::default().style(line.style);
                    used = 0;
                    if character == '\n' {
                        continue;
                    }
                }
                part.push(character);
                used += cells;
            }
            row.spans.push(Span::styled(part, span.style));
        }
        result.push(row);
    }
    result
}

fn append_context(lines: &mut Vec<Line<'static>>, app: &AppState, theme: Theme) {
    lines.push(Line::raw(""));
    lines.push(heading("UNSENT CONTEXT / NOT THE RECORDED PLAN", theme));
    if app.workbench.memory_ids.is_empty() {
        lines.push(Line::raw("No memory selected."));
    }
    for id in &app.workbench.memory_ids {
        lines.push(field("Memory", id, theme));
    }
    lines.push(field(
        "Exact prompt",
        app.workbench
            .prompt_selection
            .as_deref()
            .unwrap_or("none selected"),
        theme,
    ));
    lines.push(Line::styled(
        "s toggles a record / z clears unsent context",
        theme.muted(),
    ));
}

fn heading(text: &str, theme: Theme) -> Line<'static> {
    Line::styled(safe_text(text), theme.title())
}

/// `P1 · bug · ui, docs`: a ticket's priority, type and labels, leaving out
/// the defaults (P2, task, none) the record omits. Empty when all are default.
pub(crate) fn triage(record: &Value) -> String {
    let mut parts = Vec::new();
    if let Some(priority) = record.get("priority").and_then(Value::as_u64) {
        parts.push(format!("P{priority}"));
    }
    if let Some(kind) = record.get("type").and_then(Value::as_str) {
        parts.push(safe_text(kind));
    }
    let labels: Vec<String> = array(record, "labels")
        .iter()
        .filter_map(Value::as_str)
        .map(safe_text)
        .collect();
    if !labels.is_empty() {
        parts.push(labels.join(", "));
    }
    parts.join(" · ")
}

fn field(label: &str, value: impl AsRef<str>, theme: Theme) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{label}: "), theme.muted()),
        Span::raw(safe_text(value.as_ref())),
    ])
}

fn text(value: &Value, key: &str) -> String {
    match value.get(key) {
        Some(Value::String(value)) => safe_text(value),
        Some(Value::Number(value)) => value.to_string(),
        Some(Value::Bool(value)) => value.to_string(),
        _ => String::new(),
    }
}

fn array<'a>(value: &'a Value, key: &str) -> &'a [Value] {
    value
        .get(key)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
}

fn status(state: &str, theme: Theme) -> Line<'static> {
    let tone = state_tone(state);
    let mark = match tone {
        "success" => "+",
        "active" => ">",
        "danger" => "!",
        "attention" => "?",
        "interrupted" => "~",
        _ => "-",
    };
    Line::styled(format!("{mark} {}", safe_text(state)), theme.tone(tone))
}

fn empty(app: &AppState) -> Vec<Line<'static>> {
    vec![
        Line::raw(if app.active_request.is_some() {
            "Reading recorded runtime state..."
        } else {
            "No records in this view."
        }),
        Line::raw(""),
        Line::raw("F5 refreshes. F2 opens available commands."),
    ]
}

/// Truncate (with an ellipsis) and pad to exactly `width` terminal cells.
fn cell(value: &str, width: usize) -> String {
    let value = safe_text(value);
    if value.width() <= width {
        let pad = width - value.width();
        return format!("{value}{}", " ".repeat(pad));
    }
    let mut out = String::new();
    let mut used = 0;
    for character in value.chars() {
        let cells = character.width().unwrap_or(0);
        if used + cells + 1 > width {
            break;
        }
        out.push(character);
        used += cells;
    }
    if width > 0 {
        out.push('…');
        used += 1;
    }
    format!("{out}{}", " ".repeat(width.saturating_sub(used)))
}

/// Clip a styled line to `width` cells so narrow rows never spill past the list.
fn clip(line: Line<'static>, width: usize) -> Line<'static> {
    let mut used = 0;
    let mut spans = Vec::new();
    for span in line.spans {
        let mut part = String::new();
        let mut full = true;
        for character in span.content.chars() {
            let cells = character.width().unwrap_or(0);
            if used + cells > width {
                full = false;
                break;
            }
            part.push(character);
            used += cells;
        }
        spans.push(Span::styled(part, span.style));
        if !full {
            break;
        }
    }
    Line::from(spans).style(line.style)
}

fn short_id(id: &str) -> String {
    crate::refs::short_id(id)
}

/// A run's list title: what kind of run it is, its short id and what it is about, instead of the
/// generated identity alone (ADR 0042). The detail pane still shows the complete identity.
pub fn run_title(record: &Value) -> String {
    let metadata = record.get("metadata").cloned().unwrap_or(Value::Null);
    let short = short_id(&text(record, "id"));
    match text(&metadata, "kind").as_str() {
        "workflow" => format!(
            "workflow {short} · plan {} r{}",
            short_id(&text(&metadata, "plan_id")),
            text(&metadata, "plan_revision")
        ),
        "baseline" => format!("baseline {short} · {}", text(&metadata, "repository_id")),
        _ => {
            let prompt = text(&metadata, "prompt");
            let prompt: String = prompt
                .lines()
                .next()
                .unwrap_or("")
                .chars()
                .take(40)
                .collect();
            if prompt.is_empty() {
                format!("session {short}")
            } else {
                format!("session {short} · {prompt}")
            }
        }
    }
}
