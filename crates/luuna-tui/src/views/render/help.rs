//! The in-workbench help screen.
use super::*;

pub(super) fn render_help(frame: &mut Frame, app: &mut AppState, area: Rect, theme: Theme) {
    let k = |action: &str| crate::keys::keymap().hint(action);
    let mut lines = vec![heading("LUUNA", theme), Line::raw("")];
    for (title, body) in [
        (
            "GET AROUND",
            format!(
                "{} opens the menu. Number keys jump to a section ({} Home, {} Projects, {} Missions). ↑↓ select, Enter opens, Esc goes back. {} lists every command. {} refreshes. {} quits; the runtime keeps running.",
                k("menu"), k("home"), k("projects"), k("missions"), k("commands"), k("refresh"), k("quit")
            ),
        ),
        (
            "FORMS",
            format!(
                "Tab moves between fields. ←→ picks a choice, or type its first letter. {} adds a line. {} submits, Esc cancels.",
                k("newline"), k("submit")
            ),
        ),
        (
            "MISSIONS",
            "←→ moves between columns. Enter opens a ticket. The board shows recorded state; moving work happens through plan, approve and implement.".to_string(),
        ),
        (
            "ORBIT",
            "Runs your own commands in every repository at once: x a command, r a recipe, s sync, b the sandbox plan. Results are never evidence.".to_string(),
        ),
        (
            "ATLAS AND BEACON",
            "Pick memory and a prompt for your next plan with s; z clears the picks. User memory is only listed after u.".to_string(),
        ),
        (
            "PLANS",
            "Read the whole plan first. To approve, type approve or the hash's first 8 digits; the full hash is sent. Approving does not start work: i implements.".to_string(),
        ),
        (
            "RUNS",
            "Esc stops waiting here; it never cancels a run. c on a run cancels it. Interrupted is not failed, and completed is not passed.".to_string(),
        ),
    ] {
        lines.push(heading(title, theme));
        lines.push(Line::raw(body));
        lines.push(Line::raw(""));
    }
    lines.push(heading("STATES", theme));
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
    lines.push(Line::raw(""));
    lines.push(heading("ALL KEYS", theme));
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
        "t switches the theme, e turns enhanced keyboard reports on or off. Both save now and apply when you reopen Luuna.",
    ));
    lines.push(Line::raw(
        "Other settings: lu config set <name> <value>. lu --print-config lists them.",
    ));
    for warning in crate::config::loaded()
        .map(|l| l.warnings.as_slice())
        .unwrap_or_default()
    {
        lines.push(Line::styled(format!("! {warning}"), theme.tone("danger")));
    }
    paragraph(frame, lines, area, &mut app.scroll, theme);
}
