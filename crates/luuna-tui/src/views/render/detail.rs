//! Record detail screens (project, ticket, memory, prompt, plan) and the run evidence view.
use super::*;

pub(super) fn detail_lines(screen: Screen, record: &Value, theme: Theme) -> Vec<Line<'static>> {
    if record.is_null() {
        return vec![Line::raw(format!(
            "Nothing loaded yet. {} refreshes.",
            crate::keys::keymap().hint("refresh")
        ))];
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
            lines.push(field("Ticket", short_id(&text(record, "id")), theme));
            append_fields(
                &mut lines,
                record,
                &[
                    ("id", "Full id"),
                    ("revision", "Revision"),
                    ("purpose", "Purpose"),
                    ("project_id", "Project"),
                ],
                theme,
            );
            lines.push(field(
                "Type",
                record.get("type").and_then(Value::as_str).unwrap_or("task"),
                theme,
            ));
            lines.push(field(
                "Priority",
                format!(
                    "P{}",
                    record.get("priority").and_then(Value::as_u64).unwrap_or(2)
                ),
                theme,
            ));
            let labels: Vec<String> = array(record, "labels")
                .iter()
                .filter_map(Value::as_str)
                .map(safe_text)
                .collect();
            if !labels.is_empty() {
                lines.push(field("Labels", labels.join(", "), theme));
            }
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
            lines.push(heading(&short_id(&text(record, "id")), theme));
            lines.push(field("Full id", text(record, "id"), theme));
            append_fields(
                &mut lines,
                record,
                &[
                    ("scope", "Scope"),
                    ("provenance", "Provenance"),
                    ("hash", "Hash"),
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
        }
        Screen::Prompts | Screen::PromptDetail => {
            lines.push(heading(&text(record, "name"), theme));
            append_fields(
                &mut lines,
                record,
                &[
                    ("version", "Version"),
                    ("hash", "Hash"),
                    ("provenance", "Provenance"),
                ],
                theme,
            );
            lines.push(Line::raw(""));
            if record.get("body").is_some() {
                lines.push(heading("BODY", theme));
                for line in text(record, "body").lines() {
                    lines.push(Line::raw(line.to_owned()));
                }
            } else {
                lines.push(Line::styled("Enter shows this version.", theme.muted()));
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
                lines.push(heading("DETAILS", theme));
                structured(metadata, &mut lines, 0, theme);
            }
            lines.push(Line::raw(""));
            lines.push(Line::styled(
                "Completed means the run ended; only passed checks are evidence.",
                theme.muted(),
            ));
        }
        Screen::Plan => {
            lines.push(heading("REVIEW THIS PLAN BEFORE APPROVING IT", theme));
            lines.push(field("Plan", short_id(&text(record, "id")), theme));
            append_fields(
                &mut lines,
                record,
                &[
                    ("id", "Full id"),
                    ("revision", "Revision"),
                    ("hash", "Plan hash"),
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
                "To approve: press a, then type approve (or the hash's first 8 digits). The full hash is sent.",
                theme.tone("attention"),
            ));
            lines.push(Line::raw("Approving does not start work: i implements."));
        }
        _ => structured(record, &mut lines, 0, theme),
    }
    lines
}
pub(super) fn append_fields(
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
pub(super) fn structured(
    value: &Value,
    lines: &mut Vec<Line<'static>>,
    depth: usize,
    theme: Theme,
) {
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
pub(super) fn scalar(value: &Value) -> String {
    match value {
        Value::String(s) => safe_text(s),
        Value::Null => "not recorded".into(),
        _ => value.to_string(),
    }
}
pub(super) fn render_run(frame: &mut Frame, app: &mut AppState, area: Rect, theme: Theme) {
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
            lines.push(Line::raw("Nothing loaded for this tab yet."));
        } else {
            structured(&record, &mut lines, 0, theme);
        }
    }
    if command == "events" && !app.events.is_empty() {
        lines.push(heading("LIVE EVENTS", theme));
        for event in &app.events {
            structured(event, &mut lines, 0, theme);
        }
    }
    paragraph(frame, lines, body, &mut app.scroll, theme);
}
