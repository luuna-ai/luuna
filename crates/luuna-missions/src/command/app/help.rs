//! Go's command help (`internal/cli/help.go`, `internal/cli/commands.go`,
//! `internal/cli/app.go:274-307`): the root help text, the 57 embedded help
//! snapshots and the choice among them.

use std::fmt::Write as _;

/// The Go tool's version (`internal/cli`'s `App.Version`).
pub(super) const VERSION: &str = "0.6.0";

/// The 26 commands Go registers, in Go's order: name, arguments and
/// description (`internal/cli/commands.go:8-33`).
pub(super) const COMMANDS: &[(&str, &str, &str)] = &[
    (
        "init",
        "",
        "Initialize .luuna/missions/ in current directory",
    ),
    ("create", "", "Create a new issue"),
    ("show", "<id> [ids]", "Show one or more issues"),
    ("list", "", "List issues with filters"),
    ("ready", "", "Show open issues with no unresolved blockers"),
    ("search", "<query>", "Full-text search title + description"),
    ("update", "<id>", "Update issue fields"),
    ("close", "<id> [ids]", "Close one or more issues"),
    ("dep", "", "Manage issue dependencies"),
    ("label", "", "Manage issue labels"),
    ("blocked", "", "Show all blocked issues"),
    ("stats", "", "Project statistics"),
    ("sync", "", "Stage and commit .luuna/missions/ changes"),
    ("doctor", "", "Check project health and data integrity"),
    ("tpl", "", "Manage issue templates (molecules)"),
    (
        "migrate-from-beads",
        "",
        "Migrate issues from beads issue tracker",
    ),
    ("prime", "", "Output AI agent context"),
    (
        "onboard",
        "",
        "Add missions section to CLAUDE.md / AGENTS.md",
    ),
    (
        "upgrade",
        "",
        "Update Luuna Missions through a reviewed checkout",
    ),
    ("completions", "<shell>", "Output shell completion script"),
    ("block", "<id>", "Add a blocker to an issue"),
    ("unblock", "<id>", "Remove blockers from an issue"),
    ("plan", "", "Plan management"),
    (
        "config",
        "",
        "Read, write, and inspect .luuna/missions/config.yaml",
    ),
    (
        "setup",
        "<target>",
        "Install Claude Code hooks: prime on session start, guard .luuna/missions/*.jsonl edits",
    ),
    (
        "guard",
        "",
        "PreToolUse hook: deny hand edits to .luuna/missions/*.jsonl (reads hook JSON on stdin)",
    ),
];

/// The global options listed in the root help (`App.Help`).
const OPTIONS: &[(&str, &str)] = &[
    ("-h, --help", "Show help"),
    ("-v, --version", "Print version"),
    (
        "--format <mode>",
        "Output format (markdown|compact|plain|ids|json)",
    ),
    ("--json", "Alias for --format json"),
    ("-q, --quiet", "Suppress non-error output"),
    ("--verbose", "Extra diagnostic output"),
    ("--timing", "Show command execution time"),
];

/// Go's help snapshots (`help/*.txt`), keyed by file name without `.txt`.
pub(super) const SNAPSHOTS: &[(&str, &str)] = &[
    (
        "block",
        include_str!("../../../assets/s4-missions-lib-2/help/block.txt"),
    ),
    (
        "blocked",
        include_str!("../../../assets/s4-missions-lib-2/help/blocked.txt"),
    ),
    (
        "close",
        include_str!("../../../assets/s4-missions-lib-2/help/close.txt"),
    ),
    (
        "completions",
        include_str!("../../../assets/s4-missions-lib-2/help/completions.txt"),
    ),
    (
        "config",
        include_str!("../../../assets/s4-missions-lib-2/help/config.txt"),
    ),
    (
        "config__schema",
        include_str!("../../../assets/s4-missions-lib-2/help/config__schema.txt"),
    ),
    (
        "config__set",
        include_str!("../../../assets/s4-missions-lib-2/help/config__set.txt"),
    ),
    (
        "config__show",
        include_str!("../../../assets/s4-missions-lib-2/help/config__show.txt"),
    ),
    (
        "config__unset",
        include_str!("../../../assets/s4-missions-lib-2/help/config__unset.txt"),
    ),
    (
        "create",
        include_str!("../../../assets/s4-missions-lib-2/help/create.txt"),
    ),
    (
        "dep",
        include_str!("../../../assets/s4-missions-lib-2/help/dep.txt"),
    ),
    (
        "dep__add",
        include_str!("../../../assets/s4-missions-lib-2/help/dep__add.txt"),
    ),
    (
        "dep__list",
        include_str!("../../../assets/s4-missions-lib-2/help/dep__list.txt"),
    ),
    (
        "dep__remove",
        include_str!("../../../assets/s4-missions-lib-2/help/dep__remove.txt"),
    ),
    (
        "doctor",
        include_str!("../../../assets/s4-missions-lib-2/help/doctor.txt"),
    ),
    (
        "guard",
        include_str!("../../../assets/s4-missions-lib-2/help/guard.txt"),
    ),
    (
        "init",
        include_str!("../../../assets/s4-missions-lib-2/help/init.txt"),
    ),
    (
        "label",
        include_str!("../../../assets/s4-missions-lib-2/help/label.txt"),
    ),
    (
        "label__add",
        include_str!("../../../assets/s4-missions-lib-2/help/label__add.txt"),
    ),
    (
        "label__list",
        include_str!("../../../assets/s4-missions-lib-2/help/label__list.txt"),
    ),
    (
        "label__list-all",
        include_str!("../../../assets/s4-missions-lib-2/help/label__list-all.txt"),
    ),
    (
        "label__remove",
        include_str!("../../../assets/s4-missions-lib-2/help/label__remove.txt"),
    ),
    (
        "list",
        include_str!("../../../assets/s4-missions-lib-2/help/list.txt"),
    ),
    (
        "migrate-from-beads",
        include_str!("../../../assets/s4-missions-lib-2/help/migrate-from-beads.txt"),
    ),
    (
        "onboard",
        include_str!("../../../assets/s4-missions-lib-2/help/onboard.txt"),
    ),
    (
        "plan",
        include_str!("../../../assets/s4-missions-lib-2/help/plan.txt"),
    ),
    (
        "plan__adopt",
        include_str!("../../../assets/s4-missions-lib-2/help/plan__adopt.txt"),
    ),
    (
        "plan__create",
        include_str!("../../../assets/s4-missions-lib-2/help/plan__create.txt"),
    ),
    (
        "plan__edit",
        include_str!("../../../assets/s4-missions-lib-2/help/plan__edit.txt"),
    ),
    (
        "plan__list",
        include_str!("../../../assets/s4-missions-lib-2/help/plan__list.txt"),
    ),
    (
        "plan__outcome",
        include_str!("../../../assets/s4-missions-lib-2/help/plan__outcome.txt"),
    ),
    (
        "plan__prompt",
        include_str!("../../../assets/s4-missions-lib-2/help/plan__prompt.txt"),
    ),
    (
        "plan__release",
        include_str!("../../../assets/s4-missions-lib-2/help/plan__release.txt"),
    ),
    (
        "plan__reorder",
        include_str!("../../../assets/s4-missions-lib-2/help/plan__reorder.txt"),
    ),
    (
        "plan__review",
        include_str!("../../../assets/s4-missions-lib-2/help/plan__review.txt"),
    ),
    (
        "plan__show",
        include_str!("../../../assets/s4-missions-lib-2/help/plan__show.txt"),
    ),
    (
        "plan__submit",
        include_str!("../../../assets/s4-missions-lib-2/help/plan__submit.txt"),
    ),
    (
        "plan__templates",
        include_str!("../../../assets/s4-missions-lib-2/help/plan__templates.txt"),
    ),
    (
        "plan__validate",
        include_str!("../../../assets/s4-missions-lib-2/help/plan__validate.txt"),
    ),
    (
        "prime",
        include_str!("../../../assets/s4-missions-lib-2/help/prime.txt"),
    ),
    (
        "ready",
        include_str!("../../../assets/s4-missions-lib-2/help/ready.txt"),
    ),
    (
        "search",
        include_str!("../../../assets/s4-missions-lib-2/help/search.txt"),
    ),
    (
        "setup",
        include_str!("../../../assets/s4-missions-lib-2/help/setup.txt"),
    ),
    (
        "show",
        include_str!("../../../assets/s4-missions-lib-2/help/show.txt"),
    ),
    (
        "stats",
        include_str!("../../../assets/s4-missions-lib-2/help/stats.txt"),
    ),
    (
        "sync",
        include_str!("../../../assets/s4-missions-lib-2/help/sync.txt"),
    ),
    (
        "tpl",
        include_str!("../../../assets/s4-missions-lib-2/help/tpl.txt"),
    ),
    (
        "tpl__create",
        include_str!("../../../assets/s4-missions-lib-2/help/tpl__create.txt"),
    ),
    (
        "tpl__list",
        include_str!("../../../assets/s4-missions-lib-2/help/tpl__list.txt"),
    ),
    (
        "tpl__pour",
        include_str!("../../../assets/s4-missions-lib-2/help/tpl__pour.txt"),
    ),
    (
        "tpl__show",
        include_str!("../../../assets/s4-missions-lib-2/help/tpl__show.txt"),
    ),
    (
        "tpl__status",
        include_str!("../../../assets/s4-missions-lib-2/help/tpl__status.txt"),
    ),
    (
        "tpl__step",
        include_str!("../../../assets/s4-missions-lib-2/help/tpl__step.txt"),
    ),
    (
        "tpl__step__add",
        include_str!("../../../assets/s4-missions-lib-2/help/tpl__step__add.txt"),
    ),
    (
        "unblock",
        include_str!("../../../assets/s4-missions-lib-2/help/unblock.txt"),
    ),
    (
        "update",
        include_str!("../../../assets/s4-missions-lib-2/help/update.txt"),
    ),
    (
        "upgrade",
        include_str!("../../../assets/s4-missions-lib-2/help/upgrade.txt"),
    ),
];

/// One line of the root help: two spaces, `entry` padded to 18 runes (at
/// least two spaces after it), then `text`.
fn row(out: &mut String, entry: &str, text: &str) {
    let padding = 18usize.saturating_sub(entry.chars().count()).max(2);
    let _ = writeln!(out, "  {entry}{}{text}", " ".repeat(padding));
}

/// Go's `App.Help`.
pub(super) fn root() -> String {
    let mut out = format!(
        "Luuna Missions v{VERSION} \u{2014} Git-native issue tracking\n\n\
         Usage: luuna-missions <command> [options]\n\nCommands:\n"
    );
    for (name, arguments, description) in COMMANDS {
        if arguments.is_empty() {
            row(&mut out, name, description);
        } else {
            row(&mut out, &format!("{name} {arguments}"), description);
        }
    }
    out.push_str("\nOptions:\n");
    for (option, text) in OPTIONS {
        row(&mut out, option, text);
    }
    out.push_str("\nRun 'luuna-missions <command> --help' for command-specific help.\n");
    out
}

/// Go's `helpSnapshot`: the snapshot named by the leading arguments that
/// are not options (at most three), shortened one argument at a time until
/// one exists. Returns its name and text.
pub(super) fn snapshot(args: &[String]) -> Option<(&'static str, &'static str)> {
    let mut parts: Vec<&str> = Vec::new();
    for arg in args {
        if arg.starts_with('-') {
            break;
        }
        parts.push(arg);
        if parts.len() == 3 {
            break;
        }
    }
    while !parts.is_empty() {
        let key = parts.join("__");
        if let Some(found) = SNAPSHOTS.iter().find(|(name, _)| *name == key) {
            return Some(*found);
        }
        parts.pop();
    }
    None
}

/// The help of the known command `name` when no snapshot exists (Go's
/// fallback, `app.go:216-220`; no command has its own `Help` text).
pub(super) fn fallback(name: &str) -> String {
    let (name, arguments, description) = COMMANDS
        .iter()
        .find(|(command, _, _)| *command == name)
        .copied()
        .unwrap_or((name, "", ""));
    format!("Usage: luuna-missions {name} {arguments}\n\n{description}\n")
}
