//! The Home screen: every project with its repositories and next step.
use super::*;

pub(super) fn render_home(frame: &mut Frame, app: &mut AppState, area: Rect, theme: Theme) {
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
                if projects.len() > 1 {
                    format!(
                        "> {} ({} of {})",
                        text(&projects[current], "name"),
                        current + 1,
                        projects.len()
                    )
                } else {
                    format!("> {}", text(&projects[current], "name"))
                },
                theme.selected(),
            ),
            Line::styled(
                if projects.len() > 1 {
                    "↑↓ switches project"
                } else {
                    ""
                },
                theme.muted(),
            ),
        ]),
        selector,
    );
    let mut lines = Vec::new();
    for project in projects.iter().skip(current).take(1) {
        lines.push(field("Project", text(project, "id"), theme));
        lines.push(field("Manifest revision", text(project, "revision"), theme));
        if !text(project, "manifest_hash").is_empty() {
            lines.push(field(
                "Manifest hash",
                text(project, "manifest_hash"),
                theme,
            ));
        }
        let count = |key: &str, one: &str, many: &str| {
            let n = project.get(key).and_then(Value::as_u64).unwrap_or(0);
            format!("{n} {}", if n == 1 { one } else { many })
        };
        lines.push(field(
            "Work",
            format!(
                "{}, {}",
                count("tickets", "ticket", "tickets"),
                count("active_workflows", "active run", "active runs")
            ),
            theme,
        ));
        append_repositories(&mut lines, array(project, "repositories"), theme);
        lines.push(Line::raw(""));
    }
    lines.push(heading("NEEDS ATTENTION", theme));
    let blockers = array(&home, "blockers");
    if blockers.is_empty() {
        lines.push(Line::styled("Nothing needs you right now.", theme.muted()));
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
pub(super) fn append_repositories(
    lines: &mut Vec<Line<'static>>,
    repositories: &[Value],
    theme: Theme,
) {
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
                ("manifest_revision", "Manifest revision"),
                ("commit", "Commit"),
                ("reason", "Reason"),
            ] {
                if !text(baseline, key).is_empty() {
                    lines.push(field(label, text(baseline, key), theme));
                }
            }
        } else {
            lines.push(Line::styled("Baseline: none yet", theme.muted()));
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
