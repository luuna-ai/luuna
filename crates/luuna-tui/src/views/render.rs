//! Terminal-cell adaptation of LUUNA-AI's blue desktop and framed workbench.
//! This module only displays bridge snapshots; it never decides runtime policy.
use super::{hints, orbit_tone, rows, selected, snapshot, Screen, EVIDENCE, NAV};
use crate::{
    app::AppState,
    protocol::safe_text,
    theme::{state_tone, Theme},
};
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

fn render_home(frame: &mut Frame, app: &mut AppState, area: Rect, theme: Theme) {
    let home = snapshot(app, "home");
    let projects = array(&home, "projects");
    if projects.is_empty() {
        paragraph(frame, empty(app), area, &mut app.scroll, theme);
        return;
    }
    let current = app.workbench.cursor.min(projects.len() - 1);
    let [selector, area] =
        Layout::vertical([Constraint::Length(2), Constraint::Min(1)]).areas(area);
    frame.render_widget(
        Paragraph::new(vec![
            Line::styled(
                format!(
                    "> {} [{}/{}]",
                    text(&projects[current], "name"),
                    current + 1,
                    projects.len()
                ),
                theme.selected(),
            ),
            Line::styled(
                "Up/Down project / Enter opens / PgUp/PgDn details",
                theme.muted(),
            ),
        ]),
        selector,
    );
    let mut lines = vec![
        heading("YOUR DEVELOPMENT WORKSPACE", theme),
        Line::raw("Repository health and work waiting for your attention."),
        Line::raw(""),
    ];
    for (index, project) in projects.iter().enumerate().skip(current).take(1) {
        lines.push(Line::styled(
            format!(
                "{} {}",
                if index == app.workbench.cursor {
                    ">"
                } else {
                    " "
                },
                text(project, "name")
            ),
            if index == app.workbench.cursor {
                theme.selected()
            } else {
                theme.title()
            },
        ));
        lines.push(field("Project", text(project, "id"), theme));
        lines.push(field("Manifest revision", text(project, "revision"), theme));
        lines.push(field(
            "Manifest hash",
            text(project, "manifest_hash"),
            theme,
        ));
        lines.push(field(
            "Work",
            format!(
                "{} tickets / {} active workflows",
                text(project, "tickets"),
                text(project, "active_workflows")
            ),
            theme,
        ));
        append_repositories(&mut lines, array(project, "repositories"), theme);
        lines.push(Line::raw(""));
    }
    lines.push(heading("NEEDS ATTENTION", theme));
    let blockers = array(&home, "blockers");
    if blockers.is_empty() {
        lines.push(Line::styled("No recorded blockers.", theme.muted()));
    }
    for blocker in blockers {
        lines.push(field("Project", text(blocker, "project_id"), theme));
        lines.push(heading(&text(blocker, "title"), theme));
        let state = text(blocker, "state");
        if !state.is_empty() {
            lines.push(status(&state, theme));
        }
        lines.push(Line::raw(text(blocker, "detail")));
        lines.push(Line::raw(""));
    }
    paragraph(frame, lines, area, &mut app.scroll, theme);
}

fn append_repositories(lines: &mut Vec<Line<'static>>, repositories: &[Value], theme: Theme) {
    for repository in repositories {
        lines.push(field(
            "Repository",
            text(repository, "repository_id"),
            theme,
        ));
        lines.push(field("Checkout", text(repository, "local_path"), theme));
        if let Some(baseline) = repository.get("baseline").filter(|v| !v.is_null()) {
            lines.push(status(&text(baseline, "status"), theme));
            for (key, label) in [
                ("run_id", "Baseline run"),
                ("manifest_revision", "Baseline manifest revision"),
                ("commit", "Commit"),
                ("reason", "Reason"),
            ] {
                if !text(baseline, key).is_empty() {
                    lines.push(field(label, text(baseline, key), theme));
                }
            }
        } else {
            lines.push(Line::styled(
                "Baseline: not recorded in this view",
                theme.muted(),
            ));
        }
        if let Some(prerequisite) = repository.get("prerequisite").filter(|v| !v.is_null()) {
            lines.push(field(
                "Testing prerequisite",
                text(prerequisite, "reason"),
                theme,
            ));
        }
    }
}

fn render_collection(frame: &mut Frame, app: &AppState, area: Rect, theme: Theme) {
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
            "Search: {}{}\nUser memory: {} (explicit choice)",
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
            "Search: {}{}\nVersioned source; preview before selection",
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

fn render_board(frame: &mut Frame, app: &AppState, area: Rect, theme: Theme) {
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
                    let state = text(card, "state");
                    if !state.is_empty() {
                        lines.push(status(&state, theme));
                    }
                    if !text(card, "run_id").is_empty() {
                        lines.push(field("Run", short_id(&text(card, "run_id")), theme));
                    }
                }
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

fn detail_lines(screen: Screen, record: &Value, theme: Theme) -> Vec<Line<'static>> {
    if record.is_null() {
        return vec![Line::raw("No recorded detail yet. F5 refreshes this view.")];
    }
    let mut lines = Vec::new();
    match screen {
        Screen::Projects | Screen::Project => {
            let project = record.get("project").unwrap_or(record);
            lines.push(heading(&text(project, "name"), theme));
            append_fields(
                &mut lines,
                project,
                &[("id", "Project"), ("revision", "Manifest revision")],
                theme,
            );
            if let Some(manifest) = record.get("manifest").filter(|v| !v.is_null()) {
                append_fields(&mut lines, manifest, &[("hash", "Manifest hash")], theme);
            }
            lines.push(Line::raw(""));
            lines.push(heading("REPOSITORIES", theme));
            append_repositories(&mut lines, array(record, "repositories"), theme);
            if !array(record, "prerequisites").is_empty() {
                lines.push(heading("TESTING PREREQUISITES", theme));
                for prerequisite in array(record, "prerequisites") {
                    append_fields(
                        &mut lines,
                        prerequisite,
                        &[("repository_id", "Repository"), ("reason", "Reason")],
                        theme,
                    );
                }
            }
        }
        Screen::Ticket => {
            lines.push(heading(&text(record, "title"), theme));
            lines.push(field("Short id", short_id(&text(record, "id")), theme));
            append_fields(
                &mut lines,
                record,
                &[
                    ("id", "Ticket"),
                    ("revision", "Expected revision"),
                    ("purpose", "Purpose"),
                    ("project_id", "Project"),
                ],
                theme,
            );
            lines.push(Line::raw(""));
            lines.push(heading("INTENT", theme));
            lines.push(Line::raw(text(record, "intent")));
            lines.push(Line::raw(""));
            lines.push(heading("ACCEPTANCE CRITERIA", theme));
            for criterion in array(record, "acceptance_criteria") {
                lines.push(Line::raw(format!(
                    "[ ] {}",
                    safe_text(criterion.as_str().unwrap_or(""))
                )));
            }
            lines.push(heading("REPOSITORIES", theme));
            structured(&record["repository_ids"], &mut lines, 0, theme);
            for key in ["source", "usage"] {
                if let Some(value) = record.get(key) {
                    lines.push(heading(&key.to_uppercase(), theme));
                    structured(value, &mut lines, 0, theme);
                }
            }
        }
        Screen::Memory | Screen::MemoryDetail => {
            lines.push(heading(&text(record, "id"), theme));
            lines.push(field("Short id", short_id(&text(record, "id")), theme));
            append_fields(
                &mut lines,
                record,
                &[
                    ("scope", "Scope"),
                    ("provenance", "Provenance"),
                    ("hash", "Exact hash"),
                    ("created_at", "Recorded"),
                ],
                theme,
            );
            lines.push(Line::raw(""));
            lines.push(heading("CONTENT", theme));
            for line in text(record, "content").lines() {
                lines.push(Line::raw(line.to_owned()));
            }
            lines.push(Line::raw(""));
            lines.push(Line::styled(
                "Context selection is unsent until you submit a plan.",
                theme.muted(),
            ));
        }
        Screen::Prompts | Screen::PromptDetail => {
            lines.push(heading(&text(record, "name"), theme));
            append_fields(
                &mut lines,
                record,
                &[
                    ("version", "Version"),
                    ("hash", "Exact hash"),
                    ("provenance", "Provenance"),
                ],
                theme,
            );
            lines.push(Line::raw(""));
            if record.get("body").is_some() {
                lines.push(heading("RENDERED BODY", theme));
                for line in text(record, "body").lines() {
                    lines.push(Line::raw(line.to_owned()));
                }
            } else {
                lines.push(Line::styled(
                    "Enter previews this exact version.",
                    theme.muted(),
                ));
            }
        }
        Screen::Runs | Screen::RunDetail => {
            lines.push(heading(&run_title(record), theme));
            lines.push(field("Run", text(record, "id"), theme));
            lines.push(status(&text(record, "state"), theme));
            append_fields(
                &mut lines,
                record,
                &[
                    ("project_id", "Project"),
                    ("revision", "Revision"),
                    ("created_at", "Created"),
                    ("updated_at", "Updated"),
                ],
                theme,
            );
            if let Some(metadata) = record.get("metadata") {
                lines.push(heading("RECORDED CONTEXT", theme));
                structured(metadata, &mut lines, 0, theme);
            }
            lines.push(Line::raw(""));
            lines.push(Line::styled(
                "completed is a settled run, not passed validation.",
                theme.muted(),
            ));
        }
        Screen::Plan => {
            lines.push(heading("IMMUTABLE PLAN / REVIEW BEFORE APPROVAL", theme));
            lines.push(field("Short id", short_id(&text(record, "id")), theme));
            append_fields(
                &mut lines,
                record,
                &[
                    ("id", "Plan"),
                    ("revision", "Revision"),
                    ("hash", "Complete plan hash"),
                    ("project_id", "Project"),
                    ("ticket_id", "Ticket"),
                    ("ticket_revision", "Ticket revision"),
                    ("manifest_hash", "Manifest hash"),
                ],
                theme,
            );
            lines.push(Line::raw(""));
            // Preserve every plan field, including future extensions, as readable
            // labelled evidence. Never shorten hashes or hide approved scope.
            for (key, value) in record.as_object().into_iter().flatten() {
                if [
                    "id",
                    "revision",
                    "hash",
                    "project_id",
                    "ticket_id",
                    "ticket_revision",
                    "manifest_hash",
                ]
                .contains(&key.as_str())
                {
                    continue;
                }
                lines.push(heading(&key.replace('_', " ").to_uppercase(), theme));
                structured(value, &mut lines, 0, theme);
                lines.push(Line::raw(""));
            }
            lines.push(Line::styled(
                "Approval: after reviewing this plan, type approve (or the hash's first 8 digits) into a blank prompt; the complete hash is sent.",
                theme.tone("attention"),
            ));
            lines.push(Line::raw("Implementation is a separate explicit action."));
        }
        _ => structured(record, &mut lines, 0, theme),
    }
    lines
}

fn append_fields(
    lines: &mut Vec<Line<'static>>,
    value: &Value,
    fields: &[(&str, &str)],
    theme: Theme,
) {
    for (key, label) in fields {
        let value = text(value, key);
        if !value.is_empty() {
            lines.push(field(label, value, theme));
        }
    }
}

/// Structured evidence preserves exact values without a JSON punctuation wall.
fn structured(value: &Value, lines: &mut Vec<Line<'static>>, depth: usize, theme: Theme) {
    let indent = "  ".repeat(depth.min(8));
    match value {
        Value::Object(values) => {
            for (key, value) in values {
                let label = format!("{indent}{}", safe_text(key).replace('_', " "));
                if value.is_array() || value.is_object() {
                    lines.push(Line::styled(format!("{label}:"), theme.accent()));
                    structured(value, lines, depth + 1, theme);
                } else {
                    let display = scalar(value);
                    if ["state", "status", "verdict", "severity"].contains(&key.as_str()) {
                        lines.push(Line::from(vec![
                            Span::styled(format!("{label}: "), theme.muted()),
                            Span::styled(display.clone(), theme.tone(state_tone(&display))),
                        ]));
                    } else {
                        lines.push(field(&label, display, theme));
                    }
                }
            }
        }
        Value::Array(values) => {
            if values.is_empty() {
                lines.push(Line::styled(
                    format!("{indent}(none recorded)"),
                    theme.muted(),
                ));
            }
            for (index, value) in values.iter().enumerate() {
                if value.is_object() || value.is_array() {
                    lines.push(Line::styled(
                        format!("{indent}[{}]", index + 1),
                        theme.muted(),
                    ));
                    structured(value, lines, depth + 1, theme);
                } else {
                    lines.push(Line::raw(format!("{indent}- {}", scalar(value))));
                }
            }
        }
        _ => lines.push(Line::raw(format!("{indent}{}", scalar(value)))),
    }
}

fn scalar(value: &Value) -> String {
    match value {
        Value::String(s) => safe_text(s),
        Value::Null => "not recorded".into(),
        _ => value.to_string(),
    }
}

fn render_run(frame: &mut Frame, app: &mut AppState, area: Rect, theme: Theme) {
    let [tabs, body] = Layout::vertical([Constraint::Length(2), Constraint::Min(1)]).areas(area);
    let index = app.workbench.evidence.min(EVIDENCE.len().saturating_sub(1));
    let (command, label) = EVIDENCE[index];
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                format!(" < {} / {}: {label} > ", index + 1, EVIDENCE.len()),
                theme.selected(),
            ),
            Span::styled(" Left/Right evidence", theme.muted()),
        ])),
        tabs,
    );
    let record = if index == 0 {
        selected(app)
    } else {
        snapshot(app, command)
    };
    let mut lines = if index == 0 {
        detail_lines(Screen::RunDetail, &record, theme)
    } else {
        vec![heading(&label.to_uppercase(), theme)]
    };
    if index != 0 {
        if record.is_null() {
            lines.push(Line::raw(
                "No evidence loaded. F5 refreshes the selected tab.",
            ));
        } else {
            structured(&record, &mut lines, 0, theme);
        }
    }
    if command == "events" && !app.events.is_empty() {
        lines.push(heading("LIVE EVENT STREAM", theme));
        for event in &app.events {
            structured(event, &mut lines, 0, theme);
        }
    }
    paragraph(frame, lines, body, &mut app.scroll, theme);
}

fn render_help(frame: &mut Frame, app: &mut AppState, area: Rect, theme: Theme) {
    let mut lines = vec![
        heading("LUUNA / TERMINAL WORKBENCH", theme),
        Line::raw("A local runtime workbench, operated with the keyboard."),
        Line::raw(""),
    ];
    for (title, body) in [
        ("NAVIGATE", "F10 opens the menu. Tab changes focus. Arrow keys select; Enter opens. Escape returns. F1 opens Help; F2 opens every command; F5 refreshes. PageUp/PageDown scroll details. Ctrl+C quits the client. Every key has an alternative for keyboards without function keys (macOS takes F1-F12 unless Fn is held): see KEYS below."),
        ("MISSIONS", "Left/Right pages all eight runtime workflow columns. Cards open tickets or testing prerequisites. This is a read-only projection: a card never moves a workflow or bypasses approval."),
        ("ORBIT", "Every repository of the project at once. x runs one command in each, r runs a manifest recipe, s fetches present checkouts and clones missing ones; each opens a form first. These are your own commands, never evidence. b shows the sandbox plan; open the sandbox outside the workbench with lu sbx <project>."),
        ("ATLAS AND BEACON", "Browse scoped memory and versioned prompts. User memory requires explicit inclusion. Select context deliberately; it stays unsent until a plan is submitted. Project changes clear these choices."),
        ("FORMS", "Tab and Shift+Tab move fields. Alt+Enter (or Ctrl+J; Shift+Enter where the terminal reports it) inserts a line. Ctrl+S submits. Escape cancels. Left/Right pick from a field's choices (repositories, records, scopes, yes/no); typing still works. Letters, including q, are text while editing. Shared commands retain their confirmations and stale-version guards."),
        ("PLANS AND EVIDENCE", "Read the complete plan and hash. Approval starts with a blank prompt: type approve (or the hash's first 8 digits) to approve exactly the plan shown; the complete hash is what is sent. Implementation remains separate. Run evidence tabs retain exact runtime state and reasons."),
        ("SERVICE LIFETIME", "Leaving a view or closing this client never cancels the underlying run. Cancel and shutdown are explicitly named commands. Interrupted work is distinct from failed checks; completed alone is not passed evidence."),
    ] { lines.push(heading(title, theme)); lines.push(Line::raw(body)); lines.push(Line::raw("")); }
    lines.push(heading("STATE LEGEND / WORDS REMAIN AUTHORITATIVE", theme));
    for state in [
        "passed",
        "running",
        "failed",
        "plan_ready",
        "completed",
        "interrupted",
    ] {
        lines.push(status(state, theme));
    }
    lines.push(Line::raw(
        "z clears unsent plan context. s toggles the inspected record.",
    ));
    lines.push(Line::raw(""));
    lines.push(heading("KEYS / AS CONFIGURED", theme));
    let keymap = crate::keys::keymap();
    // The keys column is as wide as the longest binding list, so a description never runs into it.
    let width = crate::keys::ACTIONS
        .iter()
        .map(|(name, ..)| keymap.all(name).chars().count() + 2)
        .max()
        .unwrap_or(0)
        .max(26);
    for (name, _, _, about) in crate::keys::ACTIONS {
        lines.push(Line::raw(format!(
            "{:<14}{:<width$}{about}",
            name,
            keymap.all(name)
        )));
    }
    lines.push(Line::raw(""));
    lines.push(heading("SETTINGS", theme));
    let path = crate::config::path()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "no home directory; set LUUNA_TUI_CONFIG".into());
    let state = match crate::config::loaded() {
        Some(loaded) if loaded.found => "loaded",
        Some(_) => "not present; defaults in use",
        None => "defaults in use",
    };
    lines.push(Line::raw(format!("{path} ({state})")));
    if let Some(project) = crate::config::loaded().and_then(|loaded| loaded.project_path.as_ref()) {
        lines.push(Line::raw(format!(
            "Project overrides: {}",
            project.display()
        )));
    }

    lines.push(Line::raw(
        "Press c to switch theme or k to toggle enhanced keyboard reports. Changes save immediately; reopen to apply.",
    ));
    lines.push(Line::raw(
        "For any setting: lu config set <name> <JSON value>. lu --print-config lists defaults.",
    ));
    for warning in crate::config::loaded()
        .map(|l| l.warnings.as_slice())
        .unwrap_or_default()
    {
        lines.push(Line::styled(format!("! {warning}"), theme.tone("danger")));
    }
    paragraph(frame, lines, area, &mut app.scroll, theme);
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

fn orbit_branch(repository: &Value) -> String {
    if repository.get("present") == Some(&Value::Bool(false)) {
        return "-".into();
    }
    match text(repository, "branch") {
        branch if branch.is_empty() => "(detached)".into(),
        branch => branch,
    }
}

fn orbit_upstream(repository: &Value) -> String {
    match repository.get("upstream").filter(|v| v.is_object()) {
        Some(upstream) => format!("↑{} ↓{}", text(upstream, "ahead"), text(upstream, "behind")),
        None => "no upstream".into(),
    }
}

fn orbit_count(dirty: &Value, key: &str) -> u64 {
    dirty.get(key).and_then(Value::as_u64).unwrap_or(0)
}

fn orbit_changes(repository: &Value) -> String {
    let Some(dirty) = repository.get("dirty").filter(|v| v.is_object()) else {
        return "-".into();
    };
    let total: u64 = ["staged", "unstaged", "untracked", "conflicted"]
        .iter()
        .map(|key| orbit_count(dirty, key))
        .sum();
    match total {
        0 => "clean".into(),
        1 => "1 change".into(),
        n => format!("{n} changes"),
    }
}

fn orbit_recipes(repository: &Value) -> String {
    let recipes = array(repository, "recipes")
        .iter()
        .filter_map(Value::as_str)
        .map(safe_text)
        .collect::<Vec<_>>();
    if recipes.is_empty() {
        "-".into()
    } else {
        recipes.join(", ")
    }
}

/// "missing", the inspection error, or nothing; both are shown in the danger tone.
fn orbit_problem(repository: &Value) -> String {
    let error = text(repository, "error");
    if repository.get("present") == Some(&Value::Bool(false)) {
        if error.is_empty() {
            "missing".into()
        } else {
            format!("missing: {error}")
        }
    } else {
        error
    }
}

fn orbit_status(state: &str, theme: Theme) -> Span<'static> {
    let tone = orbit_tone(state);
    let mark = if tone == "danger" { "!" } else { "-" };
    Span::styled(format!("{mark} {}", safe_text(state)), theme.tone(tone))
}

fn orbit_row(repository: &Value, width: usize, theme: Theme) -> Vec<Line<'static>> {
    let problem = orbit_problem(repository);
    let conflicted = repository
        .get("dirty")
        .is_some_and(|dirty| orbit_count(dirty, "conflicted") > 0);
    let changes_tone = if conflicted {
        theme.tone("danger")
    } else {
        theme.panel()
    };
    if width >= 70 {
        let state = width.saturating_sub(69).max(1);
        vec![Line::from(vec![
            Span::styled(
                cell(&text(repository, "repository_id"), 16) + " ",
                theme.title(),
            ),
            Span::raw(cell(&orbit_branch(repository), 14) + " "),
            Span::raw(cell(&orbit_upstream(repository), 11) + " "),
            Span::styled(cell(&orbit_changes(repository), 11) + " ", changes_tone),
            Span::raw(cell(&orbit_recipes(repository), 12) + " "),
            Span::styled(cell(&problem, state), theme.tone("danger")),
        ])]
    } else {
        let mut second = vec![
            Span::raw(format!("  {} · ", orbit_upstream(repository))),
            Span::styled(orbit_changes(repository), changes_tone),
            Span::raw(format!(" · {}", orbit_recipes(repository))),
        ];
        if !problem.is_empty() {
            second.push(Span::styled(format!(" · {problem}"), theme.tone("danger")));
        }
        let first = format!(
            "{}  {}",
            text(repository, "repository_id"),
            orbit_branch(repository)
        );
        vec![
            Line::styled(cell(&first, width), theme.title()),
            clip(Line::from(second), width),
        ]
    }
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

fn render_orbit(frame: &mut Frame, app: &mut AppState, area: Rect, theme: Theme) {
    let view = snapshot(app, "orbit");
    let records = rows(app);
    let width = usize::from(area.width).saturating_sub(2);
    let wide = width >= 70;
    let per_row = if wide { 1 } else { 2 };
    let header = u16::from(wide);
    let wanted = u16::try_from(records.len().max(1) * per_row)
        .unwrap_or(u16::MAX)
        .saturating_add(header);
    let table_height = wanted.min(area.height.saturating_sub(1) / 2).max(1);
    let [summary, table, detail] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(table_height),
        Constraint::Min(1),
    ])
    .areas(area);
    let line = if view.is_null() {
        "Orbit status not loaded / F5 reads it".to_owned()
    } else {
        let mut parts = vec![
            text(&view, "name"),
            format!("{} repositories", records.len()),
        ];
        if !text(&view, "manifest_revision").is_empty() {
            parts.push(format!("manifest r{}", text(&view, "manifest_revision")));
        }
        parts.push(format!("observed {}", text(&view, "observed_at")));
        parts.join(" / ")
    };
    frame.render_widget(
        Paragraph::new(cell(&line, usize::from(summary.width))).style(theme.muted()),
        summary,
    );
    if records.is_empty() {
        frame.render_widget(
            Paragraph::new(if app.active_request.is_some() {
                "Reading Orbit status..."
            } else {
                "No repositories in this view."
            })
            .style(theme.muted()),
            table,
        );
    } else {
        let [heading_area, list_area] =
            Layout::vertical([Constraint::Length(header), Constraint::Min(1)]).areas(table);
        if wide {
            let columns = format!(
                "  {}{}{}{}{}STATE",
                cell("REPOSITORY", 17),
                cell("BRANCH", 15),
                cell("UPSTREAM", 12),
                cell("CHANGES", 12),
                cell("RECIPES", 13),
            );
            frame.render_widget(
                Paragraph::new(cell(&columns, usize::from(heading_area.width)))
                    .style(theme.muted()),
                heading_area,
            );
        }
        let items = records
            .iter()
            .map(|repository| ListItem::new(orbit_row(repository, width, theme)))
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
    }
    let lines = orbit_detail_lines(app, &records, theme);
    paragraph(frame, lines, detail, &mut app.scroll, theme);
}

fn orbit_detail_lines(app: &AppState, records: &[Value], theme: Theme) -> Vec<Line<'static>> {
    let project = app.context.get("project").cloned().unwrap_or_default();
    let mut lines = Vec::new();
    if let Some(kind) = app.workbench.orbit_detail.as_deref() {
        let record = snapshot(app, kind);
        if !record.is_null() {
            if kind == "sbx" {
                sandbox_lines(&mut lines, &record, &project, theme);
            } else {
                orbit_result_lines(&mut lines, kind, &record, theme);
            }
            lines.push(Line::raw(""));
        }
    }
    let index = app.workbench.cursor.min(records.len().saturating_sub(1));
    let Some(repository) = records.get(index) else {
        if lines.is_empty() {
            lines = empty(app);
        }
        return lines;
    };
    lines.push(heading(&text(repository, "repository_id"), theme));
    lines.push(field("Checkout", text(repository, "local_path"), theme));
    if repository.get("present") == Some(&Value::Bool(false)) {
        lines.push(Line::styled(
            "! checkout missing / s sync clones it from the registered remote",
            theme.tone("danger"),
        ));
    } else {
        lines.push(field("Branch", orbit_branch(repository), theme));
        append_fields(&mut lines, repository, &[("head_commit", "HEAD")], theme);
        let upstream = match repository.get("upstream").filter(|v| v.is_object()) {
            Some(upstream) => format!("{} {}", text(upstream, "name"), orbit_upstream(repository)),
            None => "no upstream".into(),
        };
        lines.push(field("Upstream", upstream, theme));
        append_fields(&mut lines, repository, &[("remote", "Remote")], theme);
        if let Some(dirty) = repository.get("dirty").filter(|v| v.is_object()) {
            lines.push(field(
                "Changes",
                format!(
                    "{} staged / {} unstaged / {} untracked / {} conflicted",
                    orbit_count(dirty, "staged"),
                    orbit_count(dirty, "unstaged"),
                    orbit_count(dirty, "untracked"),
                    orbit_count(dirty, "conflicted")
                ),
                theme,
            ));
            for path in array(dirty, "paths").iter().filter_map(Value::as_str) {
                lines.push(Line::raw(format!("  {}", safe_text(path))));
            }
            if dirty.get("truncated") == Some(&Value::Bool(true)) {
                lines.push(Line::styled("  (more paths not listed)", theme.muted()));
            }
        }
    }
    lines.push(field("Recipes", orbit_recipes(repository), theme));
    let error = text(repository, "error");
    if !error.is_empty() {
        lines.push(Line::styled(format!("! {error}"), theme.tone("danger")));
    }
    lines
}

fn tail(lines: &mut Vec<Line<'static>>, label: &str, output: &str, theme: Theme) {
    let rows = output.lines().collect::<Vec<_>>();
    if rows.iter().all(|row| row.trim().is_empty()) {
        return;
    }
    lines.push(Line::styled(format!("  {label} (tail)"), theme.muted()));
    for row in &rows[rows.len().saturating_sub(6)..] {
        lines.push(Line::raw(format!("    {}", safe_text(row))));
    }
}

fn orbit_result_lines(lines: &mut Vec<Line<'static>>, kind: &str, record: &Value, theme: Theme) {
    lines.push(heading(&format!("LAST ORBIT ACTION / {kind}"), theme));
    let command = match record.get("command") {
        Some(Value::Array(parts)) => parts.iter().map(scalar).collect::<Vec<_>>().join(" "),
        Some(value) => scalar(value),
        None => String::new(),
    };
    if !command.is_empty() {
        lines.push(field("Command", command, theme));
    }
    append_fields(lines, record, &[("id", "Execution")], theme);
    lines.push(Line::styled(
        "Your own commands in each checkout: never evidence or a gate.",
        theme.muted(),
    ));
    for result in array(record, "results") {
        lines.push(Line::from(vec![
            orbit_status(&text(result, "status"), theme),
            Span::raw(format!(" {}", text(result, "repository_id"))),
        ]));
        append_fields(
            lines,
            result,
            &[
                ("action", "  Action"),
                ("exit_code", "  Exit code"),
                ("reason", "  Reason"),
            ],
            theme,
        );
        tail(lines, "stdout", &text(result, "stdout_excerpt"), theme);
        tail(lines, "stderr", &text(result, "stderr_excerpt"), theme);
    }
}

fn sandbox_lines(lines: &mut Vec<Line<'static>>, plan: &Value, project: &str, theme: Theme) {
    lines.push(heading("SANDBOX PLAN / NOT OPENED HERE", theme));
    append_fields(lines, plan, &[("backend", "Backend")], theme);
    let available = plan.get("available") == Some(&Value::Bool(true));
    lines.push(Line::from(vec![
        Span::styled("Available: ", theme.muted()),
        Span::styled(
            if available { "yes" } else { "no" },
            theme.tone(if available { "neutral" } else { "attention" }),
        ),
    ]));
    append_fields(
        lines,
        plan,
        &[
            ("reason", "Reason"),
            ("network", "Network"),
            ("workdir", "Starts in"),
            ("home", "Sandbox home"),
        ],
        theme,
    );
    for repository in array(plan, "repositories") {
        lines.push(field(
            &format!("  {}", text(repository, "repository_id")),
            format!(
                "{} -> {}",
                text(repository, "host_path"),
                text(repository, "sandbox_path")
            ),
            theme,
        ));
    }
    for note in array(plan, "notes").iter().filter_map(Value::as_str) {
        lines.push(Line::styled(
            format!("- {}", safe_text(note)),
            theme.muted(),
        ));
    }
    lines.push(Line::styled(
        format!(
            "Open it outside the workbench: lu sbx {}",
            safe_text(project)
        ),
        theme.accent(),
    ));
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
