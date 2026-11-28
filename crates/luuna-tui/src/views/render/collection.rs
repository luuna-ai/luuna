//! List screens: projects, Atlas memory, Beacon prompts and runs.
use super::*;

pub(super) fn render_collection(frame: &mut Frame, app: &AppState, area: Rect, theme: Theme) {
    let records = rows(app);
    let screen = app.workbench.screen;
    let [search, body] = Layout::vertical([
        Constraint::Length(if matches!(screen, Screen::Memory | Screen::Prompts) {
            2
        } else {
            1
        }),
        Constraint::Min(1),
    ])
    .areas(area);
    let title = if screen == Screen::Memory {
        format!(
            "Search: {}{}\nUser memory: {} (u switches)",
            safe_text(&app.workbench.query),
            if app.workbench.searching { "_" } else { "" },
            if app.workbench.include_user {
                "included"
            } else {
                "excluded"
            }
        )
    } else if screen == Screen::Prompts {
        format!(
            "Search: {}{}\nEnter shows a version",
            safe_text(&app.workbench.query),
            if app.workbench.searching { "_" } else { "" }
        )
    } else {
        format!(
            "{} recorded {}",
            records.len(),
            if screen == Screen::Projects {
                "projects"
            } else {
                "runs"
            }
        )
    };
    frame.render_widget(Paragraph::new(title).style(theme.muted()), search);
    if records.is_empty() {
        paragraph(frame, empty(app), body, &mut Default::default(), theme);
        return;
    }
    let (list_area, preview_area) = if body.width >= 72 {
        let [left, right] =
            Layout::horizontal([Constraint::Percentage(40), Constraint::Percentage(60)])
                .areas(body);
        (left, Some(right))
    } else {
        (body, None)
    };
    let items = records
        .iter()
        .map(|record| {
            let mut lines = match screen {
                Screen::Projects => vec![
                    heading(
                        &text(record.get("project").unwrap_or(record), "name"),
                        theme,
                    ),
                    field(
                        "id",
                        text(record.get("project").unwrap_or(record), "id"),
                        theme,
                    ),
                ],
                // What a record says leads; its short id (ADR 0042) is what an argument accepts.
                Screen::Memory => vec![
                    heading(
                        &format!(
                            "[{}] {}",
                            text(record, "scope"),
                            text(record, "content").lines().next().unwrap_or("")
                        ),
                        theme,
                    ),
                    field("id", short_id(&text(record, "id")), theme),
                ],
                Screen::Prompts => vec![
                    heading(&text(record, "name"), theme),
                    field("version", text(record, "version"), theme),
                ],
                _ => vec![
                    heading(&run_title(record), theme),
                    status(&text(record, "state"), theme),
                ],
            };
            lines.push(Line::raw(""));
            ListItem::new(lines)
        })
        .collect::<Vec<_>>();
    let mut state =
        ListState::default().with_selected(Some(app.workbench.cursor.min(records.len() - 1)));
    frame.render_stateful_widget(
        List::new(items)
            .highlight_style(theme.selected())
            .highlight_symbol("> "),
        list_area,
        &mut state,
    );
    if let Some(preview_area) = preview_area {
        let block = Block::default()
            .borders(Borders::LEFT)
            .border_type(BorderType::Double)
            .border_style(theme.muted());
        let inside = block.inner(preview_area);
        frame.render_widget(block, preview_area);
        let record = &records[app.workbench.cursor.min(records.len() - 1)];
        paragraph(
            frame,
            detail_lines(screen, record, theme),
            inside,
            &mut Default::default(),
            theme,
        );
    }
}
