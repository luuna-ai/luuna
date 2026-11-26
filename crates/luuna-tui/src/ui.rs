use crate::{
    app::{AppState, Mode},
    protocol::safe_text,
    theme::{state_tone, Theme},
};
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::Modifier,
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, List, ListItem, ListState, Paragraph},
    Frame,
};
use unicode_width::UnicodeWidthChar;

pub fn render(frame: &mut Frame, app: &mut AppState, theme: Theme) {
    let area = frame.area();
    app.approval_visible = false;
    frame.render_widget(Block::default().style(theme.base()), area);
    if area.width < 20 || area.height < 8 {
        frame.render_widget(
            Paragraph::new("LUUNA\nEnlarge terminal\nCtrl+C quits").style(theme.base()),
            area,
        );
        return;
    }
    if app.mode == Mode::Guide && app.guide.is_some() {
        crate::views::guide::render(frame, app, theme);
        return;
    }
    let [menu, header, body, status, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(area);
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" LUUNA ", theme.selected()),
            Span::styled(project_switcher(app), theme.accent()),
            Span::raw("  "),
            Span::styled(
                safe_text(&app.connection),
                theme.tone(state_tone(&app.connection)),
            ),
        ]))
        .style(theme.base()),
        header,
    );
    // The section bar shows the key that opens each section, so it never promises a key that
    // does nothing; it drops the rarer sections first when the terminal is narrow.
    let keymap = crate::keys::keymap();
    let mut menu_spans = Vec::new();
    let mut used = 0;
    for (label, _, action) in crate::views::NAV {
        if action.is_empty() {
            continue;
        }
        let key = keymap.hint(action);
        let width = key.chars().count() + label.chars().count() + 3;
        if used + width > usize::from(area.width) {
            break;
        }
        used += width;
        menu_spans.push(Span::styled(format!(" {key}"), theme.mnemonic()));
        menu_spans.push(Span::styled(format!(" {label} "), theme.chrome()));
    }
    let menu_line = Line::from(menu_spans);
    frame.render_widget(Paragraph::new(menu_line).style(theme.chrome()), menu);
    let content = if app.prompt.is_some() {
        let [result, prompt] = Layout::vertical([
            Constraint::Min(3),
            Constraint::Length(4.min(body.height.saturating_sub(3))),
        ])
        .areas(body);
        render_prompt(frame, app, prompt, theme);
        result
    } else {
        body
    };
    app.approval_visible = content.height > 2 && content.width > 2;
    match app.mode {
        Mode::Workbench => crate::views::render::render(frame, app, content, theme),
        Mode::Catalog => render_catalog(frame, app, content, theme),
        Mode::Form => render_form(frame, app, content, theme),
        Mode::Result | Mode::Guide => render_result(frame, app, content, theme),
    }
    let status_text = if app.workbench.searching {
        format!("Search: {}  Enter apply · Esc close", app.workbench.query)
    } else {
        safe_text(&app.status)
    };
    frame.render_widget(
        Paragraph::new(status_text).style(if app.workbench.error.is_some() {
            theme.tone("danger")
        } else {
            theme.muted()
        }),
        status,
    );
    let hint = if app.mode == Mode::Workbench {
        if app.workbench.nav {
            " ↑↓ section  Enter open  Esc close  q quit".to_string()
        } else {
            format!(" {}", crate::views::hints(app.workbench.screen))
        }
    } else if area.width < 55 {
        if app.prompt.is_some() {
            " Enter answer PgUp/Dn Esc".to_string()
        } else if app.mode == Mode::Form {
            " Tab field ^S submit Esc".to_string()
        } else if app.mode == Mode::Result {
            " ↑↓ PgUp/Dn Esc q".to_string()
        } else {
            " ↑↓ Enter Tab q".to_string()
        }
    } else if app.prompt.is_some() {
        " Enter answer · PgUp/PgDn plan · Esc abort".to_string()
    } else if app.mode == Mode::Form {
        " Tab next · ←→ pick · Ctrl+S submit · Alt+Enter new line · Esc cancel".to_string()
    } else if app.mode == Mode::Result {
        " ↑↓ PgUp/PgDn scroll · Esc back/abort · q quit".to_string()
    } else {
        " ↑↓ select · Enter open · Tab context · q quit".to_string()
    };
    frame.render_widget(Paragraph::new(hint).style(theme.chrome()), footer);
}

fn panel(title: impl Into<Line<'static>>, theme: Theme) -> Block<'static> {
    Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_type(BorderType::Double)
        .border_style(theme.focused())
        .style(theme.panel())
}

fn render_catalog(frame: &mut Frame, app: &mut AppState, area: Rect, theme: Theme) {
    let commands = app.filtered();
    let context = if app.filter.is_empty() {
        "all contexts"
    } else {
        &app.filter
    };
    let title = format!(" Actions / {context} / {} ", commands.len());
    let items = commands
        .iter()
        .map(|command| {
            ListItem::new(Line::from(vec![
                Span::styled(format!("{:<19} ", safe_text(&command.name)), theme.accent()),
                Span::raw(safe_text(&command.action)),
            ]))
        })
        .collect::<Vec<_>>();
    let list = List::new(items)
        .block(panel(title, theme))
        .highlight_style(theme.selected())
        .highlight_symbol("▸ ");
    app.selected = app.selected.min(commands.len().saturating_sub(1));
    let mut state = ListState::default().with_selected(Some(app.selected));
    frame.render_stateful_widget(list, area, &mut state);
}

fn render_form(frame: &mut Frame, app: &mut AppState, area: Rect, theme: Theme) {
    let Some(form) = &app.form else {
        return;
    };
    let block = panel(format!(" {} ", safe_text(&form.command.action)), theme);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let current = form.fields.get(form.cursor);
    let typing =
        current.is_some_and(|f| !f.fixed || f.choice_index().is_none() && !f.value.is_empty());
    let [fields_area, edit_area, hint_area] = Layout::vertical([
        Constraint::Min(1),
        Constraint::Length(match (typing, inner.height >= 12) {
            (false, _) => 0,
            (true, true) => 5,
            (true, false) => 1,
        }),
        Constraint::Length(2.min(inner.height.saturating_sub(2))),
    ])
    .areas(inner);
    // Labels in one column, as wide as the longest (capped), so values line up.
    let column = form
        .fields
        .iter()
        .map(|f| f.label.chars().count())
        .max()
        .unwrap_or(0)
        .min(30)
        .min(usize::from(inner.width) / 3)
        + 2;
    let mut rows: Vec<(Option<usize>, Line<'static>)> = Vec::new();
    for (index, field) in form.fields.iter().enumerate() {
        if field.advanced && !form.fields[..index].iter().any(|f| f.advanced) {
            rows.push((None, Line::raw("")));
            rows.push((None, Line::styled("  More options", theme.muted())));
        }
        let focused = index == form.cursor;
        let label: String = safe_text(&field.label)
            .chars()
            .take(column.saturating_sub(2))
            .collect();
        let mut spans = vec![
            Span::styled(
                if focused { "▸ " } else { "  " },
                if focused {
                    theme.title()
                } else {
                    theme.panel()
                },
            ),
            Span::styled(
                format!("{label:<column$}"),
                if focused {
                    theme.title()
                } else {
                    theme.panel()
                },
            ),
        ];
        let inline = field.fixed
            && field.choices.len() <= 6
            && (field.value.is_empty() || field.choice_index().is_some());
        if inline {
            // A short fixed list shows every choice, the picked one bracketed.
            let picked = field.choice_index();
            for (i, (value, _)) in field.choices.iter().enumerate() {
                let text = safe_text(value);
                spans.push(if Some(i) == picked {
                    Span::styled(format!("[{text}]"), theme.selected())
                } else {
                    Span::styled(format!(" {text} "), theme.muted())
                });
                spans.push(Span::raw(" "));
            }
        } else if field.value.is_empty() {
            spans.push(Span::styled(
                if field.argument {
                    "required"
                } else {
                    "optional"
                },
                theme.muted(),
            ));
            if focused && !field.choices.is_empty() {
                spans.push(Span::styled(
                    format!("  ←→ {} to pick from", field.choices.len()),
                    theme.muted(),
                ));
            }
        } else {
            let value = if field.choice_index().is_some() {
                safe_text(&crate::refs::short_id(&field.value))
            } else {
                safe_text(&field.value).replace('\n', " ↵ ")
            };
            let arrows = focused && field.choices.len() > 1;
            spans.push(Span::styled(
                if arrows {
                    format!("◂ {value} ▸")
                } else {
                    value
                },
                theme.panel(),
            ));
            // A picked record shows its name beside its short id (ADR 0042).
            if let Some(name) = field
                .choice_index()
                .map(|i| &field.choices[i].1)
                .filter(|l| !l.is_empty())
            {
                spans.push(Span::styled(
                    format!("  {}", safe_text(name)),
                    theme.muted(),
                ));
            }
        }
        let style = if focused {
            theme.selected()
        } else {
            theme.panel()
        };
        let mut line = Line::from(spans);
        if focused && !inline {
            line = line.style(style);
        }
        rows.push((Some(index), line));
    }
    let available = usize::from(fields_area.height).max(1);
    let at = rows
        .iter()
        .position(|(i, _)| *i == Some(form.cursor))
        .unwrap_or(0);
    let top = at.saturating_sub(available.saturating_sub(1));
    let lines: Vec<Line> = rows
        .into_iter()
        .skip(top)
        .take(available)
        .map(|(_, l)| l)
        .collect();
    frame.render_widget(Paragraph::new(lines).style(theme.panel()), fields_area);
    let Some(field) = current else {
        return;
    };
    if typing && edit_area.height > 0 {
        let value = wrapped_lines(
            &format!("> {}_", safe_text(&field.value)),
            edit_area.width.max(1),
        );
        let top = value.len().saturating_sub(usize::from(edit_area.height));
        frame.render_widget(
            Paragraph::new(
                value
                    .into_iter()
                    .skip(top)
                    .map(Line::raw)
                    .collect::<Vec<_>>(),
            )
            .style(theme.title()),
            edit_area,
        );
    }
    let how = if field.fixed {
        "←→ or a letter picks. "
    } else if !field.choices.is_empty() {
        "←→ picks, or type. "
    } else {
        ""
    };
    frame.render_widget(
        Paragraph::new(safe_text(&format!("{how}{}", field.hint)))
            .style(theme.muted())
            .wrap(ratatui::widgets::Wrap { trim: true }),
        hint_area,
    );
}

fn render_result(frame: &mut Frame, app: &mut AppState, area: Rect, theme: Theme) {
    let title = if app.active_request.is_some() {
        " Runtime / working · Esc stops waiting "
    } else {
        " Runtime / authoritative response "
    };
    let block = panel(title, theme);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let rows = wrapped_lines(&app.output, inner.width.max(1));
    let maximum = rows
        .len()
        .saturating_sub(usize::from(inner.height))
        .min(usize::from(u16::MAX)) as u16;
    let shown = app.scroll.clamp(maximum);
    let lines = rows
        .into_iter()
        .skip(usize::from(shown))
        .take(usize::from(inner.height))
        .map(Line::raw)
        .collect::<Vec<_>>();
    frame.render_widget(Paragraph::new(lines).style(theme.panel()), inner);
    if app.output_omitted && inner.height > 0 {
        frame.render_widget(
            Paragraph::new("Earlier display omitted; use events/artifact replay")
                .style(theme.tone("attention")),
            Rect::new(inner.x, inner.bottom() - 1, inner.width, 1),
        );
    }
}

fn render_prompt(frame: &mut Frame, app: &AppState, area: Rect, theme: Theme) {
    let Some(prompt) = &app.prompt else {
        return;
    };
    let title = if prompt.kind == "hash" {
        " Exact plan approval · review above "
    } else {
        " Runtime question "
    };
    let suffix = if prompt.kind == "document" {
        "^S submit"
    } else {
        "Enter submit"
    };
    let width = area.width.saturating_sub(2).max(1);
    let question = wrapped_lines(&prompt.question, width)
        .into_iter()
        .next()
        .unwrap_or_default();
    let input = wrapped_lines(&format!("> {}", safe_text(&prompt.value)), width)
        .into_iter()
        .last()
        .unwrap_or_default();
    let text = format!("{question}\n{input}");
    frame.render_widget(
        Paragraph::new(text)
            .block(panel(format!("{title}{suffix} "), theme))
            .style(theme.accent().add_modifier(Modifier::BOLD)),
        area,
    );
}

/// Cell-aware terminal wrapping is separate from payload storage, so API strings stay unchanged.
pub fn wrapped_lines(text: &str, width: u16) -> Vec<String> {
    let clean = safe_text(text);
    let width = usize::from(width).max(1);
    let mut rows = Vec::new();
    for source in clean.split('\n') {
        let mut row = String::new();
        let mut used = 0;
        for c in source.chars() {
            let cells = UnicodeWidthChar::width(c).unwrap_or(0);
            if cells > width {
                continue;
            }
            if used + cells > width {
                rows.push(std::mem::take(&mut row));
                used = 0;
            }
            row.push(c);
            used += cells;
        }
        rows.push(row);
    }
    rows
}

/// The project the workbench is scoped to, and the key that switches it (owner feedback D1).
pub fn project_switcher(app: &AppState) -> String {
    let key = crate::keys::keymap().hint("projects");
    match app.context.get("project").filter(|id| !id.is_empty()) {
        Some(id) => format!(" ▸ {} ({key} switch)", safe_text(id)),
        None => format!(" ▸ no project ({key} pick one)"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{CommandDef, MAX_OUTPUT};
    use ratatui::{backend::TestBackend, Terminal};
    use unicode_width::UnicodeWidthStr;
    #[test]
    fn every_size_renders_catalog_forms_results_and_prompt_without_overflow() {
        for (width, height) in [(40, 12), (60, 24), (80, 24), (20, 6), (8, 3)] {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            let mut app = AppState::default();
            app.commands = (0..61)
                .map(|i| CommandDef {
                    name: format!("action-{i}"),
                    action: "Long action title 界界界".repeat(5),
                    context: "runtime".into(),
                    ..Default::default()
                })
                .collect();
            app.selected = 60;
            terminal
                .draw(|f| render(f, &mut app, Theme { color: true }))
                .unwrap();
            let visible = terminal
                .backend()
                .buffer()
                .content
                .iter()
                .map(|cell| cell.symbol())
                .collect::<String>();
            if width >= 20 && height >= 8 {
                assert!(visible.contains("action-60"));
                assert!(visible.contains("q"));
            }
            app.mode = Mode::Result;
            app.output = "界 wide unbroken text ".repeat(1000);
            app.scroll.to_bottom();
            terminal
                .draw(|f| render(f, &mut app, Theme { color: false }))
                .unwrap();
            if width >= 20 && height >= 8 {
                assert!(app.scroll.displayed() < u16::MAX);
            }
            app.output = "x".repeat(MAX_OUTPUT);
            terminal
                .draw(|f| render(f, &mut app, Theme { color: true }))
                .unwrap();
            assert_eq!(
                terminal.backend().buffer().content.len(),
                usize::from(width) * usize::from(height)
            );
        }
    }
    #[test]
    fn wide_unicode_and_ansi_do_not_overflow() {
        for line in wrapped_lines("界界界\x1b]0;hidden\x07ab\ncombining e\u{301}", 4) {
            assert!(line.width() <= 4);
            assert!(!line.contains("hidden"));
        }
    }

    #[test]
    fn every_plan_line_is_reviewable_at_forty_by_twelve() {
        use crate::app::PendingPrompt;
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let mut terminal = Terminal::new(TestBackend::new(40, 12)).unwrap();
        let mut app = AppState::default();
        app.mode = Mode::Result;
        app.output = (0..30).map(|i| format!("plan-line-{i:02}\n")).collect();
        app.prompt = Some(PendingPrompt {
            request_id: "p".into(),
            prompt_id: "h".into(),
            kind: "hash".into(),
            question: "Type the complete hash".into(),
            value: String::new(),
        });
        for i in 0..30 {
            terminal
                .draw(|f| render(f, &mut app, Theme { color: false }))
                .unwrap();
            let visible = terminal
                .backend()
                .buffer()
                .content
                .iter()
                .map(|cell| cell.symbol())
                .collect::<String>();
            assert!(app.approval_visible);
            assert!(
                visible.contains(&format!("plan-line-{i:02}")),
                "missing line {i}"
            );
            app.key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        }
    }
}
