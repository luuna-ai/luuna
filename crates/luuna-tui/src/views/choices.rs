//! Values a form field can pick from, and resolving short references to loaded records (ADR 0042).
use super::*;

/// Records and values already on screen that a form field can pick from (ADR 0042), as
/// `(value, label)`. Record values are exact identities, which the form shows by their short ids;
/// nothing here reads from the runtime.
pub(super) fn known_choices(app: &AppState, command: &str, field: &str) -> Vec<(String, String)> {
    let pair = |value: String, label: String| (value, label);
    let fixed = |values: &[&str]| {
        values
            .iter()
            .map(|v| ((*v).to_string(), String::new()))
            .collect::<Vec<_>>()
    };
    let project_id = project(app);
    let repositories = || {
        let mut seen = Vec::new();
        let view = snapshot(app, "project");
        let mut sources = array(view["repositories"].clone());
        for home in array(snapshot(app, "home")["projects"].clone()) {
            if text(&home, "id") == project_id {
                sources.extend(array(home["repositories"].clone()));
            }
        }
        for orbit in array(snapshot(app, "orbit")["repositories"].clone()) {
            sources.push(orbit);
        }
        for repository in sources {
            let id = text(&repository, "repository_id");
            if !id.is_empty() && !seen.iter().any(|(v, _): &(String, String)| *v == id) {
                seen.push(pair(id, text(&repository, "local_path")));
            }
        }
        seen
    };
    let cards = || {
        let mut out = Vec::new();
        for column in array(snapshot(app, "board")["columns"].clone()) {
            for card in array(column["cards"].clone()) {
                if !text(&card, "ticket_id").is_empty() {
                    out.push(card);
                }
            }
        }
        out
    };
    match field {
        "project" => {
            let mut out: Vec<(String, String)> = array(snapshot(app, "projects"))
                .iter()
                .map(|p| p.get("project").unwrap_or(p))
                .map(|p| pair(text(p, "id"), text(p, "name")))
                .collect();
            for p in array(snapshot(app, "home")["projects"].clone()) {
                if !out.iter().any(|(v, _)| *v == text(&p, "id")) {
                    out.push(pair(text(&p, "id"), text(&p, "name")));
                }
            }
            out
        }
        "repository" | "repository-id" if command != "register" && command != "import" => {
            repositories()
        }
        // Record choices carry the exact identity, which the workbench context must hold; the
        // form shows each by its short id and label (ADR 0042).
        "ticket" => cards()
            .iter()
            .map(|c| pair(text(c, "ticket_id"), text(c, "title")))
            .collect(),
        "plan" => cards()
            .iter()
            .filter(|c| !text(c, "run_id").is_empty())
            .map(|c| {
                pair(
                    format!("plan-{}", text(c, "ticket_id")),
                    format!("plan of {}", text(c, "title")),
                )
            })
            .collect(),
        // `latest` is offered only beside the loaded runs it is resolved among, naming the run.
        "run" => {
            let mut out = Vec::new();
            if let Some(newest) = newest_loaded_run(app) {
                let short = crate::refs::short_id(&text(&newest, "id"));
                out.push(pair("latest".into(), format!("the newest run, {short}")));
            }
            out.extend(
                array(snapshot(app, "runs"))
                    .iter()
                    .map(|r| pair(text(r, "id"), render::run_title(r))),
            );
            out
        }
        "memory-id" => array(snapshot(app, "memory"))
            .iter()
            .map(|m| {
                let content: String = text(m, "content")
                    .lines()
                    .next()
                    .unwrap_or("")
                    .chars()
                    .take(40)
                    .collect();
                pair(text(m, "id"), format!("[{}] {content}", text(m, "scope")))
            })
            .collect(),
        "scope" => fixed(&["project", "process", "user"]),
        "to-scope" => fixed(&["project", "user"]),
        "profile" => fixed(&[
            "node",
            "node-typescript",
            "react",
            "nextjs",
            "python",
            "go",
            "rust",
            "flutter",
            "android",
            "ios",
        ]),
        "purpose" => vec![
            pair("feature".into(), "default".into()),
            pair("testing_setup".into(), "testing prerequisite".into()),
        ],
        "type" if command.ends_with("ticket") => fixed(&["task", "bug", "feature", "epic"]),
        "priority" if command.ends_with("ticket") => [
            ("P0", "critical"),
            ("P1", "high"),
            ("P2", "normal"),
            ("P3", "low"),
            ("P4", "backlog"),
        ]
        .iter()
        .map(|(v, l)| pair((*v).into(), (*l).into()))
        .collect(),
        "mode" if command == "set-store-mode" => fixed(&["runtime", "repository"]),
        "network" if command == "sbx" => fixed(&["open", "none"]),
        "role" => fixed(&["implementer", "repository_lead", "project_orchestrator"]),
        "recipe" => {
            let mut out = Vec::new();
            let orbit = snapshot(app, "orbit");
            let mut lists = vec![orbit["recipes"].clone()];
            lists.extend(
                array(orbit["repositories"].clone())
                    .into_iter()
                    .map(|r| r["recipes"].clone()),
            );
            for list in lists {
                for recipe in array(list) {
                    let id = recipe
                        .as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| text(&recipe, "id"));
                    if !id.is_empty() && !out.iter().any(|(v, _): &(String, String)| *v == id) {
                        out.push(pair(id, String::new()));
                    }
                }
            }
            out
        }
        "task" if command == "implement" => array(snapshot(app, "tasks"))
            .iter()
            .map(|t| {
                pair(
                    text(t, "task_id"),
                    format!("{} · {}", text(t, "repository_id"), text(t, "state")),
                )
            })
            .filter(|(v, _)| !v.is_empty())
            .collect(),
        _ => Vec::new(),
    }
}
/// The exact identity a context value names among the records the workbench has loaded, so the
/// request, the context and the snapshot's provenance all name one record (ADR 0042). `None` for a
/// short id or `latest` that nothing loaded resolves: the command then resolves it against the
/// runtime (or refuses it), and the context waits for the record the runtime returns.
pub fn exact_context(app: &AppState, name: &str, value: &str) -> Option<String> {
    if !matches!(name, "ticket" | "plan" | "run") {
        return Some(value.to_string());
    }
    if name == "run" && is_latest(value) {
        return newest_loaded_run(app).map(|r| text(&r, "id"));
    }
    if !crate::refs::is_short_reference(value) {
        return Some(value.to_string());
    }
    // A prefix naming another kind (`ticket-8b1e` for a run) stays unresolved, so the command
    // refuses it rather than this matching a loaded record of the argument's kind.
    let allowed: &[&str] = match name {
        "ticket" => &["ticket"],
        "plan" => &["plan", "ticket"],
        _ => &["run"],
    };
    if !crate::refs::kind_matches(value, allowed) {
        return None;
    }
    let ids: Vec<String> = known_choices(app, "", name)
        .into_iter()
        .map(|(id, _)| id)
        .collect();
    crate::refs::unique_match(value, &ids)
}
pub(super) fn is_latest(value: &str) -> bool {
    value.trim().eq_ignore_ascii_case("latest")
}
/// The newest loaded run, which a form's `latest` names, when the loaded list is a first page
/// (no `--before` or `--limit`); a paged list does not know the newest run.
pub(super) fn newest_loaded_run(app: &AppState) -> Option<Value> {
    let flags = app
        .snapshot_requests
        .get("runs")
        .and_then(|r| r.fields.get("flags"))
        .cloned()
        .unwrap_or(Value::Null);
    if flags.get("before").is_some() || flags.get("limit").is_some() {
        return None;
    }
    array(snapshot(app, "runs"))
        .into_iter()
        .max_by_key(|r| text(r, "created_at"))
}
/// Fields whose value is always one of their choices, so typing a letter picks one.
const FIXED: &[&str] = &[
    "scope", "to-scope", "profile", "purpose", "role", "type", "priority", "mode", "network",
];
/// Gives each field of a new form the choices the workbench already knows, and fills a required
/// argument that has exactly one possible value (the only repository, the only project).
pub fn offer_choices(app: &AppState, form: &mut crate::app::ActionForm) {
    let command = form.command.name.clone();
    for field in &mut form.fields {
        if !field.choices.is_empty() || field.name.starts_with('@') {
            continue;
        }
        field.choices = known_choices(app, &command, &field.name)
            .into_iter()
            .filter(|(value, _)| !value.is_empty())
            .collect();
        field.fixed = !field.choices.is_empty() && FIXED.contains(&field.name.as_str());
        let only_record = field.choices.len() == 1
            && !matches!(field.name.as_str(), "scope" | "to-scope" | "run");
        if field.argument && field.value.is_empty() && only_record {
            field.value = field.choices[0].0.clone();
        }
    }
}
