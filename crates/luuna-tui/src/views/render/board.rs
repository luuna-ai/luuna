//! The Missions board: run-state columns of ticket cards.
use super::*;

pub(super) fn render_board(frame: &mut Frame, app: &AppState, area: Rect, theme: Theme) {
    let board = snapshot(app, "board");
    let columns = array(&board, "columns");
    if columns.is_empty() {
        paragraph(frame, empty(app), area, &mut Default::default(), theme);
        return;
    }
    let visible = usize::from(area.width / 27).clamp(1, 3).min(columns.len());
    let selected_column = app.workbench.column.min(columns.len() - 1);
    let start = (selected_column / visible) * visible;
    let end = (start + visible).min(columns.len());
    let [info, cards] = Layout::vertical([Constraint::Length(2), Constraint::Min(1)]).areas(area);
    frame.render_widget(Paragraph::new(format!("Read-only runtime projection / columns {}-{} of {}\nLeft/Right changes column; Enter inspects the card", start + 1, end, columns.len())).style(theme.muted()), info);
    let constraints = vec![Constraint::Ratio(1, (end - start) as u32); end - start];
    let areas = Layout::horizontal(constraints).split(cards);
    for (offset, column) in columns[start..end].iter().enumerate() {
        let records = array(column, "cards");
        let index = start + offset;
        let focused = index == selected_column;
        let name = text(&column["column"], "title");
        let block = panel(&format!("{name} ({})", records.len()), focused, theme);
        let inner = block.inner(areas[offset]);
        frame.render_widget(block, areas[offset]);
        if records.is_empty() {
            frame.render_widget(Paragraph::new("\n No cards").style(theme.muted()), inner);
            continue;
        }
        let items = records
            .iter()
            .map(|card| {
                let mut lines = vec![heading(&text(card, "title"), theme)];
                if text(card, "kind") == "prerequisite" {
                    lines.push(field("Repository", text(card, "repository_id"), theme));
                    lines.push(Line::raw(text(card, "reason")));
                } else {
                    lines.push(field("Ticket", short_id(&text(card, "ticket_id")), theme));
                    lines.push(field("Revision", text(card, "ticket_revision"), theme));
                    let triage = triage(card);
                    if !triage.is_empty() {
                        lines.push(field("Triage", triage, theme));
                    }
                    let state = text(card, "state");
                    if !state.is_empty() {
                        lines.push(status(&state, theme));
                    }
                    if !text(card, "run_id").is_empty() {
                        lines.push(field("Run", short_id(&text(card, "run_id")), theme));
                    }
                }
                lines.push(Line::styled(
                    crate::projections::next_step(card),
                    theme.accent(),
                ));
                lines.push(Line::styled("------------------------", theme.muted()));
                ListItem::new(lines)
            })
            .collect::<Vec<_>>();
        let mut state = ListState::default().with_selected(if focused {
            Some(app.workbench.cursor.min(records.len() - 1))
        } else {
            None
        });
        frame.render_stateful_widget(
            List::new(items)
                .highlight_style(theme.selected())
                .highlight_symbol(">"),
            inner,
            &mut state,
        );
    }
}
