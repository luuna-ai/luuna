//! The command-line help: an overview grouped by what the developer is doing, a page per command
//! (arguments, flags, the cataloged route and an example), and the complete reference. Everything
//! is read from the command table and the catalog, so a new command is documented by its own
//! descriptions; the groups below only decide where it is listed (an unlisted command lands in
//! "Other commands", never nowhere). Presentation only: nothing here decides what is allowed.
use crate::commands::{self, Command};
use crate::js::{self, J};
use crate::text::{visible_width, wrap};

pub const DEFAULT_NAME: &str = "luuna-tui";

/// Display groups, in workflow order. Each command is listed once.
pub const HELP_GROUPS: [(&str, &str, &[&str]); 11] = [
    (
        "Start here",
        "Is the runtime up, and what needs attention?",
        &["init", "health", "home", "board", "doctor"],
    ),
    (
        "Projects",
        "Register repositories, confirm their manifest and record a baseline.",
        &[
            "projects",
            "project",
            "register",
            "discover",
            "confirm",
            "baseline",
            "baselines",
            "rebind",
            "store-mode",
            "set-store-mode",
            "project-events",
            "propose-scaffold",
            "scaffold",
            "export",
            "import",
        ],
    ),
    (
        "Orbit and sandbox",
        "Every repository of a project at once, and a shell that reaches only them (ADR 0036).",
        &["orbit", "orbit-exec", "orbit-run", "orbit-sync", "sbx"],
    ),
    (
        "Tickets",
        "Work items, kept in the project's authoritative source (Missions when enabled).",
        &[
            "tickets",
            "create-ticket",
            "show-ticket",
            "edit-ticket",
            "ticket-source",
            "reconcile-ticket",
        ],
    ),
    (
        "Plans and approval",
        "Immutable plan revisions; approval binds to one exact hash.",
        &["plan", "plans", "show-plan", "approve"],
    ),
    (
        "Runs and candidates",
        "Implement, follow, validate and review a candidate.",
        &[
            "runs",
            "run",
            "events",
            "follow",
            "implement",
            "candidate",
            "diff",
            "validate",
            "repair",
            "tasks",
            "attempts",
            "worktrees",
            "artifact",
            "cancel",
        ],
    ),
    (
        "Publication",
        "Push the reviewed commits and open pull requests; merging stays yours.",
        &[
            "publish",
            "publication",
            "reconcile-publication",
            "publication-repair",
        ],
    ),
    (
        "Memory (Atlas)",
        "Scoped project knowledge with provenance; changes bind to exact hashes.",
        &[
            "components",
            "enable-components",
            "memory",
            "remember",
            "correct-memory",
            "promote",
            "forget",
            "retention",
            "set-retention",
            "apply-retention",
            "export-user-memory",
        ],
    ),
    (
        "Prompts (Beacon)",
        "Versioned prompts selected into plans.",
        &["prompts", "save-prompt", "render-prompt"],
    ),
    (
        "Intent (Roots)",
        "Accepted intent is decided by a person. intent <verb> and intent-<verb> are the same command.",
        &["intent-show", "intent-init", "intent-proposals", "intent-questions", "intent-sprouts", "intent-accept", "intent-reject", "intent-reject-sprout"],
    ),
    (
        "Runtime",
        "Isolated sessions and the service itself.",
        &["start-session", "shutdown"],
    ),
];

/// The local intent group page; it needs no runtime or token.
pub fn intent_group(options: &Options) -> String {
    let (title, blurb, names) = HELP_GROUPS
        .iter()
        .find(|(title, _, _)| *title == "Intent (Roots)")
        .expect("intent help group");
    let lines = names
        .iter()
        .filter_map(|name| help_command(name))
        .map(|command| format!("  {} {}", options.name, commands::usage(command)))
        .collect::<Vec<_>>();
    format!("{title}\n{blurb}\n\n{}\n", lines.join("\n"))
}

/// The component CLIs the launcher runs directly (scripts/start-terminal.ps1); their own help is authoritative.
pub const COMPONENT_CLIS: [(&str, &str, &str, &str, &[&str]); 3] = [
    (
        "missions",
        "lms",
        "Tickets and structured plans",
        ".luuna/missions/",
        &[
            "init", "create", "list", "ready", "show", "update", "close", "dep", "plan", "sync",
            "doctor",
        ],
    ),
    (
        "atlas",
        "lat",
        "Expertise and project memory",
        ".luuna/atlas/",
        &[
            "init", "add", "record", "query", "search", "prime", "status", "prune", "sync",
            "doctor",
        ],
    ),
    (
        "beacon",
        "lbc",
        "Composable prompts",
        ".luuna/beacon/",
        &[
            "init", "create", "list", "show", "update", "render", "history", "diff", "pin", "sync",
        ],
    ),
];

fn context_arg(name: &str) -> Option<&'static str> {
    Some(match name {
        "project" => "project id (see projects)",
        "ticket" => "ticket id (see tickets <project>)",
        "plan" => "plan id (see plans <ticket>)",
        "run" => "run id (see runs)",
        _ => return None,
    })
}

#[derive(Clone, Debug)]
pub struct Options {
    pub name: String,
    pub colour: bool,
    pub width: Option<usize>,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            name: DEFAULT_NAME.into(),
            colour: false,
            width: None,
        }
    }
}

fn style(options: &Options, text: &str, kind: &str) -> String {
    let code = match kind {
        "heading" => "1",
        "name" => "36",
        "dim" => "2",
        "accent" => "33",
        _ => "",
    };
    if options.colour && !code.is_empty() {
        format!("\u{1b}[{code}m{text}\u{1b}[0m")
    } else {
        text.to_string()
    }
}

/// The commands the help lists, in the original map's order: actions, projections, the guide.
pub fn help_commands() -> Vec<&'static Command> {
    let table = commands::table();
    table
        .commands
        .iter()
        .chain(table.projections.iter())
        .chain(table.guides.iter())
        .collect()
}

pub fn help_command(name: &str) -> Option<&'static Command> {
    help_commands().into_iter().find(|c| c.name == name)
}

fn title_of(command: &Command) -> String {
    let entry = commands::capability(&command.id);
    if let Some(title) = entry.get("title").as_str() {
        return title.to_string();
    }
    if let Some(title) = &command.title {
        return title.clone();
    }
    format!("{}: reads {}", command.action, command.projects.join(", "))
}

/// The groups with every command placed; commands no group names are collected last.
pub fn grouped_commands() -> Vec<(String, String, Vec<&'static Command>)> {
    let mut placed = std::collections::HashSet::new();
    let mut groups: Vec<(String, String, Vec<&'static Command>)> = HELP_GROUPS
        .iter()
        .map(|(title, blurb, names)| {
            (
                title.to_string(),
                blurb.to_string(),
                names.iter().filter_map(|n| help_command(n)).collect(),
            )
        })
        .collect();
    for (_, _, commands) in &groups {
        for command in commands {
            placed.insert(command.name.clone());
        }
    }
    let rest: Vec<_> = help_commands()
        .into_iter()
        .filter(|c| !placed.contains(&c.name))
        .collect();
    if !rest.is_empty() {
        groups.push(("Other commands".into(), String::new(), rest));
    }
    groups
}

/// `text` wrapped to `width` with every row after the first indented by `indent` columns.
fn hanging(prefix: &str, text: &str, width: usize, indent: usize) -> Vec<String> {
    let room = 20.max(width as isize - indent as isize) as usize;
    let rows = wrap(text, room);
    if visible_width(prefix) > indent {
        let mut out = vec![prefix.to_string()];
        out.extend(
            rows.into_iter()
                .map(|row| format!("{}{row}", " ".repeat(indent))),
        );
        return out;
    }
    let first = format!("{prefix}{}", " ".repeat(indent - visible_width(prefix)));
    rows.into_iter()
        .enumerate()
        .map(|(index, row)| {
            if index == 0 {
                format!("{first}{row}")
            } else {
                format!("{}{row}", " ".repeat(indent))
            }
        })
        .collect()
}

/// `text` indented two columns, its continuation rows four.
fn indented(text: &str, width: usize) -> Vec<String> {
    wrap(text, width.saturating_sub(4))
        .into_iter()
        .enumerate()
        .map(|(index, row)| format!("{}{row}", if index > 0 { "    " } else { "  " }))
        .collect()
}

/// A section heading with its blurb beside it when both fit, else the blurb wrapped below it.
fn heading(options: &Options, title: &str, blurb: &str, width: usize) -> Vec<String> {
    if blurb.is_empty() {
        return vec![style(options, title, "heading")];
    }
    if js::len16(title) + 2 + visible_width(blurb) <= width {
        return vec![format!(
            "{}  {}",
            style(options, title, "heading"),
            style(options, blurb, "dim")
        )];
    }
    let mut out = vec![style(options, title, "heading")];
    out.extend(
        wrap(blurb, width.saturating_sub(2))
            .into_iter()
            .map(|row| format!("  {}", style(options, &row, "dim"))),
    );
    out
}

fn positional(command: &Command) -> String {
    command
        .args
        .iter()
        .map(|a| format!("<{}>", a.name))
        .collect::<Vec<_>>()
        .join(" ")
}

fn quote(value: &str) -> String {
    let plain = !value.is_empty()
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "_@%+=:,./\\-".contains(c));
    if plain {
        value.to_string()
    } else {
        format!("\"{}\"", value.replace('"', "\\\""))
    }
}

/// `String(value).replace(/sha256:[a-f0-9]{64}/g, "sha256:<hash>")`
fn shown_value(value: &str) -> String {
    let mut out = String::new();
    let mut rest = value;
    while let Some(at) = rest.find("sha256:") {
        let after = &rest[at + 7..];
        let hex = after
            .bytes()
            .take(64)
            .take_while(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(b))
            .count();
        if hex == 64 {
            out.push_str(&rest[..at]);
            out.push_str("sha256:<hash>");
            rest = &after[64..];
        } else {
            out.push_str(&rest[..at + 7]);
            rest = after;
        }
    }
    out.push_str(rest);
    out
}

/// A runnable-looking example from the command's parity example.
pub fn example_line(command: &Command, name: &str) -> String {
    let mut parts = vec![name.to_string(), command.name.clone()];
    parts.extend(
        command
            .example
            .get("args")
            .items()
            .iter()
            .map(|v| quote(&shown_value(&v.js()))),
    );
    if let Some(flags) = command.example.get("flags").as_obj() {
        for (flag, value) in flags {
            if flag == "key" {
                continue;
            }
            let items: Vec<J> = match value {
                J::Arr(items) => items.clone(),
                other => vec![other.clone()],
            };
            for item in items {
                if item == J::Bool(true) {
                    parts.push(format!("--{flag}"));
                } else {
                    parts.push(format!("--{flag} {}", quote(&shown_value(&item.js()))));
                }
            }
        }
    }
    parts.join(" ")
}

fn width_of(options: &Options) -> usize {
    options.width.unwrap_or(100).clamp(0, 110).max(48)
}

/// The overview printed by `help`, `-h` and `--help`.
pub fn overview(options: &Options) -> String {
    let name = options.name.as_str();
    let width = width_of(options);
    let s = |text: &str, kind: &str| style(options, text, kind);
    let mut out: Vec<String> = Vec::new();
    out.push(format!(
        "{} {}",
        s("Luuna", "heading"),
        s("— terminal client for the Luuna runtime", "dim")
    ));
    out.push(String::new());
    out.push(s("USAGE", "heading"));
    for (left, right) in [
        (
            name.to_string(),
            "open the interactive workbench (keyboard driven)",
        ),
        (
            format!("{name} <command> [args] [flags]"),
            "run one action against the runtime",
        ),
        (
            format!("{name} <command> --help"),
            "arguments, flags and an example for one command",
        ),
        (
            format!("{name} help --all"),
            "the complete reference, every flag included",
        ),
        (
            format!("{name} missions|atlas|beacon …"),
            "run a component CLI directly (see COMPONENTS)",
        ),
    ] {
        out.extend(hanging(
            &format!("  {}", s(&left, "name")),
            right,
            width,
            38,
        ));
    }
    let groups = grouped_commands();
    let widest = groups
        .iter()
        .flat_map(|(_, _, commands)| {
            commands
                .iter()
                .map(|c| visible_width(&format!("{} {}", c.name, positional(c))))
        })
        .max()
        .unwrap_or(0);
    let column = 34.min(4 + widest);
    for (title, blurb, commands) in &groups {
        out.push(String::new());
        out.extend(heading(options, &title.to_uppercase(), blurb, width));
        for command in commands {
            let args = positional(command);
            out.extend(hanging(
                &format!(
                    "  {}{}",
                    s(&command.name, "name"),
                    if args.is_empty() {
                        String::new()
                    } else {
                        format!(" {args}")
                    }
                ),
                &title_of(command),
                width,
                column,
            ));
        }
    }
    out.push(String::new());
    out.extend(heading(
        options,
        "COMPONENTS",
        "The pinned CLIs, run directly in the current directory. No runtime, no approval.",
        width,
    ));
    for (component, alias, what, store, key) in COMPONENT_CLIS {
        out.extend(hanging(
            &format!(
                "  {} {}",
                s(&format!("{name} {component}"), "name"),
                s(&format!("({alias})"), "dim")
            ),
            &format!("{what} in {store}"),
            width,
            column,
        ));
        out.extend(hanging("", &s(&key.join("\u{a0}· "), "dim"), width, column));
    }
    out.extend(hanging("", &format!("Each has many more commands: {name} missions --help. Tickets edited outside Luuna are brought back with reconcile-ticket."), width, 2));
    out.push(String::new());
    out.push(s("FLAGS", "heading"));
    out.extend(hanging(
        &format!("  {}", s("--json", "name")),
        "print the runtime's record unchanged (for scripts)",
        width,
        column,
    ));
    out.extend(hanging(
        &format!("  {}", s("-h, --help", "name")),
        "show help; after a command, that command's help",
        width,
        column,
    ));
    out.push(String::new());
    out.push(s("ENVIRONMENT", "heading"));
    out.extend(hanging(
        &format!("  {}", s("LUUNA_URL", "name")),
        "runtime address, loopback only (default http://127.0.0.1:4242)",
        width,
        column,
    ));
    out.extend(hanging(
        &format!("  {}", s("LUUNA_API_TOKEN", "name")),
        "service token; never echoed, prompted hidden on a terminal when missing",
        width,
        column,
    ));
    out.extend(hanging(
        &format!("  {}", s("NO_COLOR", "name")),
        "set to print without colour",
        width,
        column,
    ));
    out.push(String::new());
    out.extend(wrap(&format!("Every command is one action of schemas/capability-catalog.json ({name} catalog prints it)."), width).into_iter().map(|row| s(&row, "dim")));
    out.push(String::new());
    out.join("\n")
}

/// The page of one command: what it does, its arguments and flags, the route it calls and an example.
pub fn command_help(command: &Command, options: &Options) -> String {
    let name = options.name.as_str();
    let width = width_of(options);
    let s = |text: &str, kind: &str| style(options, text, kind);
    let entry = commands::capability(&command.id);
    let mut out: Vec<String> = Vec::new();
    let args = positional(command);
    let head = format!(
        "{args}{}",
        if command.flags.is_empty() {
            ""
        } else {
            " [flags]"
        }
    );
    out.extend(
        hanging(
            &s(&format!("{name} {}", command.name), "name"),
            head.trim(),
            width,
            visible_width(&format!("{name} {} ", command.name)),
        )
        .into_iter()
        .map(|row| row.trim_end().to_string()),
    );
    out.push(String::new());
    out.extend(hanging("  ", &title_of(command), width, 2));
    if let Some(detail) = &command.detail {
        out.push(String::new());
        out.extend(hanging("  ", detail, width, 2));
    }
    if let Some(confirm) = &command.confirm {
        out.extend(hanging(
            "  ",
            &s(&format!("Asks first: {confirm}"), "accent"),
            width,
            2,
        ));
    }
    if !command.args.is_empty() {
        out.push(String::new());
        out.push(s("ARGUMENTS", "heading"));
        let column = 30.min(
            4 + command
                .args
                .iter()
                .map(|a| js::len16(&a.name) + 2)
                .max()
                .unwrap_or(0),
        );
        for arg in &command.args {
            let description = arg
                .description
                .clone()
                .or_else(|| context_arg(&arg.name).map(str::to_string))
                .unwrap_or_else(|| arg.name.clone());
            out.extend(hanging(
                &format!("  <{}>", arg.name),
                &description,
                width,
                column,
            ));
        }
    }
    out.push(String::new());
    out.push(s("FLAGS", "heading"));
    let mut labels: Vec<(String, String, bool)> = command
        .flags
        .iter()
        .map(|(flag, definition)| {
            (
                if definition.boolean {
                    format!("--{flag}")
                } else {
                    format!("--{flag} <value>")
                },
                definition.description.clone().unwrap_or_default(),
                definition.repeat,
            )
        })
        .collect();
    let has_uses = command.raw.has("uses");
    if !has_uses {
        labels.push((
            "--json".into(),
            "print the runtime's record unchanged".into(),
            false,
        ));
    }
    let column = 30.min(
        4 + labels
            .iter()
            .map(|(label, _, _)| js::len16(label))
            .max()
            .unwrap_or(0),
    );
    for (label, description, repeat) in &labels {
        let suffix = if *repeat && !description.contains("repeatable") {
            " (repeatable)"
        } else {
            ""
        };
        out.extend(hanging(
            &format!("  {}", s(label, "name")),
            &format!("{description}{suffix}"),
            width,
            column,
        ));
    }
    if entry.is_obj() {
        out.push(String::new());
        out.push(s("RUNTIME", "heading"));
        out.extend(indented(
            &format!(
                "{} {}  {}",
                entry.get("method").js(),
                entry.get("route").js(),
                s(&format!("· catalog {}", entry.get("id").js()), "dim")
            ),
            width,
        ));
    } else if command.raw.has("projects") || has_uses {
        out.push(String::new());
        out.push(s(
            if has_uses {
                "RUNS THESE ACTIONS"
            } else {
                "READS"
            },
            "heading",
        ));
        let list = if has_uses {
            &command.uses
        } else {
            &command.projects
        };
        out.extend(indented(&list.join(", "), width));
    }
    out.push(String::new());
    out.push(s("EXAMPLE", "heading"));
    out.extend(indented(&example_line(command, name), width));
    out.push(String::new());
    out.join("\n")
}

/// Every command page, grouped as in the overview.
pub fn reference(options: &Options) -> String {
    let mut parts = vec![overview(options)];
    for (title, _, commands) in grouped_commands() {
        parts.push(format!(
            "{}\n",
            style(options, &format!("━━ {title} "), "heading")
        ));
        for command in commands {
            parts.push(command_help(command, options));
        }
    }
    parts.join("\n")
}

/// The closest command names to a mistyped one (edit distance at most 2, or a shared prefix).
pub fn suggestions(word: &str) -> Vec<String> {
    let distance = |a: &[u16], b: &[u16]| {
        let mut row: Vec<usize> = (0..=b.len()).collect();
        for i in 1..=a.len() {
            let mut previous = row[0];
            row[0] = i;
            for j in 1..=b.len() {
                let kept = row[j];
                row[j] = (row[j] + 1)
                    .min(row[j - 1] + 1)
                    .min(previous + usize::from(a[i - 1] != b[j - 1]));
                previous = kept;
            }
        }
        row[b.len()] as isize
    };
    let word16: Vec<u16> = word.encode_utf16().collect();
    let mut scored: Vec<(isize, String)> = help_commands()
        .into_iter()
        .map(|c| {
            let candidate16: Vec<u16> = c.name.encode_utf16().collect();
            let bonus = if c.name.starts_with(word) || word.starts_with(c.name.as_str()) {
                2
            } else {
                0
            };
            (distance(&word16, &candidate16) - bonus, c.name.clone())
        })
        .filter(|(score, _)| *score <= 2)
        .collect();
    scored.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
    scored.into_iter().take(3).map(|(_, name)| name).collect()
}
