//! The Orbit screen: repository status rows, command outcomes and the sandbox plan.
use super::*;

pub(super) fn orbit_branch(repository: &Value) -> String {
    if repository.get("present") == Some(&Value::Bool(false)) {
        return "-".into();
    }
    match text(repository, "branch") {
        branch if branch.is_empty() => "(detached)".into(),
        branch => branch,
    }
}
pub(super) fn orbit_upstream(repository: &Value) -> String {
    match repository.get("upstream").filter(|v| v.is_object()) {
        Some(upstream) => format!("↑{} ↓{}", text(upstream, "ahead"), text(upstream, "behind")),
        None => "no upstream".into(),
    }
}
pub(super) fn orbit_count(dirty: &Value, key: &str) -> u64 {
    dirty.get(key).and_then(Value::as_u64).unwrap_or(0)
}
pub(super) fn orbit_changes(repository: &Value) -> String {
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
pub(super) fn orbit_recipes(repository: &Value) -> String {
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
pub(super) fn orbit_problem(repository: &Value) -> String {
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
pub(super) fn orbit_status(state: &str, theme: Theme) -> Span<'static> {
    let tone = orbit_tone(state);
    let mark = if tone == "danger" { "!" } else { "-" };
    Span::styled(format!("{mark} {}", safe_text(state)), theme.tone(tone))
}
pub(super) fn orbit_row(repository: &Value, width: usize, theme: Theme) -> Vec<Line<'static>> {
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
pub(super) fn render_orbit(frame: &mut Frame, app: &mut AppState, area: Rect, theme: Theme) {
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
pub(super) fn orbit_detail_lines(
    app: &AppState,
    records: &[Value],
    theme: Theme,
) -> Vec<Line<'static>> {
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
pub(super) fn tail(lines: &mut Vec<Line<'static>>, label: &str, output: &str, theme: Theme) {
    let rows = output.lines().collect::<Vec<_>>();
    if rows.iter().all(|row| row.trim().is_empty()) {
        return;
    }
    lines.push(Line::styled(format!("  {label} (tail)"), theme.muted()));
    for row in &rows[rows.len().saturating_sub(6)..] {
        lines.push(Line::raw(format!("    {}", safe_text(row))));
    }
}
pub(super) fn orbit_result_lines(
    lines: &mut Vec<Line<'static>>,
    kind: &str,
    record: &Value,
    theme: Theme,
) {
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
pub(super) fn sandbox_lines(
    lines: &mut Vec<Line<'static>>,
    plan: &Value,
    project: &str,
    theme: Theme,
) {
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
