//! The in-workbench help screen.
use super::*;

pub(super) fn render_help(frame: &mut Frame, app: &mut AppState, area: Rect, theme: Theme) {
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
