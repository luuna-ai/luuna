//! Native project commands: config, templates, doctor, sync, beads migration,
//! prime, onboard, Claude setup, guard and upgrade. Handlers preserve the pinned
//! Go behavior except documented A8 data-loss refusals. Discovery and external
//! project-file writes are controlled by the caller's invocation and root.

#[cfg(test)]
use crate::error::PLAN_3;
mod config;
mod doctor;
mod doctor_fix;
mod guard;
mod lossy;
mod migrate_beads;
mod onboard;
mod prime;
mod setup;
mod stale_lock;
mod sync;
mod template_record;
mod tpl;
mod upgrade;

/// `sync`.
pub(super) const SYNC: super::Entry = super::Entry::Ported(sync::run);
/// `doctor`.
pub(super) const DOCTOR: super::Entry = super::Entry::Ported(doctor::run);
/// `tpl`.
pub(super) const TPL: super::Entry = super::Entry::Ported(tpl::run);
/// `migrate-from-beads`.
pub(super) const MIGRATE_FROM_BEADS: super::Entry = super::Entry::Ported(migrate_beads::run);
/// `prime`.
pub(super) const PRIME: super::Entry = super::Entry::Ported(prime::run);
/// `onboard`.
pub(super) const ONBOARD: super::Entry = super::Entry::Ported(onboard::run);
/// `upgrade`.
pub(super) const UPGRADE: super::Entry = super::Entry::Ported(upgrade::run);
/// `config`.
pub(super) const CONFIG: super::Entry = super::Entry::Ported(config::run);
/// `setup`.
pub(super) const SETUP: super::Entry = super::Entry::Ported(setup::run);
/// `guard`.
pub(super) const GUARD: super::Entry = super::Entry::Ported(guard::run);

/// The expectations of `every_other_go_command_answers_as_its_plan_states`
/// in `command/mod.rs` for this module's commands, whose names stay there.
#[cfg(test)]
pub(super) mod checks {
    use super::super::{run, Entry, Invocation};
    use crate::error::{Error, PLAN_3};

    fn invocation(args: &[&str]) -> Invocation {
        Invocation::new(std::env::temp_dir(), args.iter().copied())
    }

    fn plan_of(error: Error) -> String {
        match error {
            Error::Unsupported { plan, .. } => plan,
            other => panic!("expected Unsupported, got {other}"),
        }
    }

    /// `<name> --json` in the system temp folder. While the command is
    /// deferred: Unsupported naming s4-missions-lib-3. Once ported, Go's exact
    /// outcome in a folder without a store, as literals.
    pub(in crate::command) fn command(name: &str) {
        let entry = match name {
            "sync" => super::SYNC,
            "doctor" => super::DOCTOR,
            "tpl" => super::TPL,
            "migrate-from-beads" => super::MIGRATE_FROM_BEADS,
            "prime" => super::PRIME,
            "onboard" => super::ONBOARD,
            "upgrade" => super::UPGRADE,
            "config" => super::CONFIG,
            "setup" => super::SETUP,
            "guard" => super::GUARD,
            other => panic!("{other} is not a command of this module"),
        };
        match entry {
            Entry::Deferred(plan) => {
                assert_eq!(plan, PLAN_3, "{name}");
                let error = run(&invocation(&[name, "--json"])).unwrap_err();
                assert_eq!(
                    error.to_string(),
                    format!(
                        "the {name} command is not supported yet; it is ported in s4-missions-lib-3"
                    ),
                    "{name}"
                );
                assert_eq!(plan_of(error), plan, "{name}");
            }
            Entry::Ported(_) => ported(name),
        }
    }
    fn ported(name: &str) {
        match name {
            "setup" => {
                let temp = tempfile::tempdir().unwrap();
                let out = run(&Invocation::new(temp.path(), ["setup", "--json"])).unwrap();
                // Literal stdout observed with the pinned Go reference in an empty project.
                assert_eq!(
                    out.stdout.as_slice(),
                    r####"{
  "success": false,
  "command": "setup",
  "error": "Unknown setup target \"\"; supported: claude"
}
"####
                        .as_bytes()
                );
                assert_eq!(out.stderr, b"");
                assert_eq!(out.exit_code, 1);
            }
            "guard" => {
                let temp = tempfile::tempdir().unwrap();
                let out = run(&Invocation::new(temp.path(), ["guard", "--json"])).unwrap();
                // Literal stdout observed with the pinned Go reference in an empty project.
                assert_eq!(
                    out.stdout.as_slice(),
                    r####"{
  "success": false,
  "command": "guard",
  "error": "too many arguments for 'guard'"
}
"####
                        .as_bytes()
                );
                assert_eq!(out.stderr, b"");
                assert_eq!(out.exit_code, 1);
            }
            "onboard" => {
                let temp = tempfile::tempdir().unwrap();
                let out = run(&Invocation::new(temp.path(), ["onboard", "--json"])).unwrap();
                // Literal stdout observed with the pinned Go reference in an empty project.
                assert_eq!(
                    out.stdout.as_slice(),
                    r####"{
  "success": false,
  "command": "onboard",
  "error": "Not in a missions project. Run `luuna-missions init` first."
}
"####
                        .as_bytes()
                );
                assert_eq!(out.stderr, b"");
                assert_eq!(out.exit_code, 1);
            }
            "prime" => {
                let temp = tempfile::tempdir().unwrap();
                let out = run(&Invocation::new(temp.path(), ["prime", "--json"])).unwrap();
                // Literal stdout observed with the pinned Go reference in an empty project.
                assert_eq!(out.stdout.as_slice(),r####"{
  "success": true,
  "command": "prime",
  "sections": {
    "closeProtocol": {
      "footer": "Then push or open a PR as the repo's conventions say.",
      "steps": [
        "Close completed issues:    luuna-missions close <id1> <id2> ...",
        "File remaining work:       luuna-missions create --title \"...\" --from <id>",
        "Run quality gates:         the project's quality gates (tests, lint, typecheck)",
        "Commit issue changes:      luuna-missions sync"
      ],
      "warning": "Before saying \"done\", run this checklist:"
    },
    "commandGroups": [
      {
        "commands": [
          {
            "command": "luuna-missions ready",
            "description": "Show issues ready to work (no blockers)"
          },
          {
            "command": "luuna-missions list --status=in_progress",
            "description": "Active work"
          },
          {
            "command": "luuna-missions show <id> [<id2> ...]",
            "description": "Detailed issue view"
          },
          {
            "command": "luuna-missions search <query>",
            "description": "Full-text search"
          }
        ],
        "name": "Finding Work"
      },
      {
        "commands": [
          {
            "command": "luuna-missions create --title=\"...\" --type=task|bug|feature|epic --priority=2",
            "description": "New issue (priority 0-4, 0=critical)"
          },
          {
            "command": "luuna-missions update <id> --claim --as <agent>",
            "description": "Claim work atomically (fails if already taken)"
          },
          {
            "command": "luuna-missions create --title=\"...\" --from <id>",
            "description": "File work discovered while on <id> (non-blocking provenance)"
          },
          {
            "command": "luuna-missions close <id1> <id2> ...",
            "description": "Close one or more issues"
          }
        ],
        "name": "Creating & Updating"
      },
      {
        "commands": [
          {
            "command": "luuna-missions dep add <issue> <depends-on>",
            "description": "Add dependency"
          },
          {
            "command": "luuna-missions blocked",
            "description": "Show all blocked issues"
          }
        ],
        "name": "Dependencies & Blocking"
      },
      {
        "commands": [
          {
            "command": "luuna-missions label add <id> bug ui",
            "description": "Add labels to an issue"
          },
          {
            "command": "luuna-missions list --label=bug",
            "description": "Filter by label"
          }
        ],
        "name": "Labels"
      },
      {
        "commands": [
          {
            "command": "luuna-missions sync",
            "description": "Stage and commit .luuna/missions/ changes"
          },
          {
            "command": "luuna-missions doctor",
            "description": "Check for data integrity issues"
          }
        ],
        "name": "Sync & Project Health"
      },
      {
        "commands": [
          {
            "command": "luuna-missions plan prompt <seed-id>",
            "description": "Emit prompt JSON for the LLM to fill"
          },
          {
            "command": "luuna-missions plan submit <seed-id> --plan <file>",
            "description": "Validate + spawn children"
          },
          {
            "command": "luuna-missions plan show <pl-id>",
            "description": "Sections, children, nested sub-plans"
          }
        ],
        "name": "Planning",
        "notes": [
          "Use `luuna-missions plan` when work is large or ambiguous; submit spawns one child seed per step. For small, well-scoped tasks, just `luuna-missions create`."
        ]
      }
    ],
    "contextRecovery": "Run `luuna-missions prime` after compaction, clear, or new session",
    "mode": "full",
    "rules": [
      "**Track durable work in missions** (`luuna-missions create`, `luuna-missions ready`, `luuna-missions close`), not markdown task files. In-session checklists (TodoWrite) are fine.",
      "**Claim before starting**: `luuna-missions update <id> --claim` (atomic; fails if someone else has it)",
      "**Never hand-edit `.luuna/missions/*.jsonl`**: `luuna-missions` commands lock, validate, and dedup; hand edits skip all three"
    ],
    "title": "Missions Workflow Context",
    "workflows": [
      {
        "commands": [
          "luuna-missions ready                    # Find available work",
          "luuna-missions show <id>                # Review issue details",
          "luuna-missions update <id> --claim      # Claim it"
        ],
        "name": "Starting work"
      },
      {
        "commands": [
          "luuna-missions close <id1> <id2> ...    # Close all completed issues at once",
          "luuna-missions sync                     # Stage + commit .luuna/missions/"
        ],
        "name": "Completing work"
      }
    ]
  },
  "state": null,
  "content": "# Missions Workflow Context\n\n> **Context Recovery**: Run `luuna-missions prime` after compaction, clear, or new session\n\n# Session Close Protocol\n\nBefore saying \"done\", run this checklist:\n\n```\n[ ] 1. Close completed issues:    luuna-missions close <id1> <id2> ...\n[ ] 2. File remaining work:       luuna-missions create --title \"...\" --from <id>\n[ ] 3. Run quality gates:         the project's quality gates (tests, lint, typecheck)\n[ ] 4. Commit issue changes:      luuna-missions sync\n```\n\nThen push or open a PR as the repo's conventions say.\n\n## Core Rules\n- **Track durable work in missions** (`luuna-missions create`, `luuna-missions ready`, `luuna-missions close`), not markdown task files. In-session checklists (TodoWrite) are fine.\n- **Claim before starting**: `luuna-missions update <id> --claim` (atomic; fails if someone else has it)\n- **Never hand-edit `.luuna/missions/*.jsonl`**: `luuna-missions` commands lock, validate, and dedup; hand edits skip all three\n\n## Essential Commands\n\n### Finding Work\n- `luuna-missions ready` — Show issues ready to work (no blockers)\n- `luuna-missions list --status=in_progress` — Active work\n- `luuna-missions show <id> [<id2> ...]` — Detailed issue view\n- `luuna-missions search <query>` — Full-text search\n\n### Creating & Updating\n- `luuna-missions create --title=\"...\" --type=task|bug|feature|epic --priority=2` — New issue (priority 0-4, 0=critical)\n- `luuna-missions update <id> --claim --as <agent>` — Claim work atomically (fails if already taken)\n- `luuna-missions create --title=\"...\" --from <id>` — File work discovered while on <id> (non-blocking provenance)\n- `luuna-missions close <id1> <id2> ...` — Close one or more issues\n\n### Dependencies & Blocking\n- `luuna-missions dep add <issue> <depends-on>` — Add dependency\n- `luuna-missions blocked` — Show all blocked issues\n\n### Labels\n- `luuna-missions label add <id> bug ui` — Add labels to an issue\n- `luuna-missions list --label=bug` — Filter by label\n\n### Sync & Project Health\n- `luuna-missions sync` — Stage and commit .luuna/missions/ changes\n- `luuna-missions doctor` — Check for data integrity issues\n\n### Planning\nUse `luuna-missions plan` when work is large or ambiguous; submit spawns one child seed per step. For small, well-scoped tasks, just `luuna-missions create`.\n\n- `luuna-missions plan prompt <seed-id>` — Emit prompt JSON for the LLM to fill\n- `luuna-missions plan submit <seed-id> --plan <file>` — Validate + spawn children\n- `luuna-missions plan show <pl-id>` — Sections, children, nested sub-plans\n\n## Common Workflows\n\n**Starting work:**\n```bash\nluuna-missions ready                    # Find available work\nluuna-missions show <id>                # Review issue details\nluuna-missions update <id> --claim      # Claim it\n```\n\n**Completing work:**\n```bash\nluuna-missions close <id1> <id2> ...    # Close all completed issues at once\nluuna-missions sync                     # Stage + commit .luuna/missions/\n```\n"
}
"####.as_bytes());
                assert_eq!(out.stderr, b"");
                assert_eq!(out.exit_code, 0);
            }
            "config" | "tpl" => {
                let temp = tempfile::tempdir().unwrap();
                let out = run(&Invocation::new(temp.path(), [name, "--json"])).unwrap();
                assert_eq!(out.stdout, b"");
                assert_eq!(out.stderr, b"error: unknown option '--json'\n");
                assert_eq!(out.exit_code, 1);
            }
            "doctor" => {
                let temp = tempfile::tempdir().unwrap();
                let out = run(&Invocation::new(temp.path(), ["doctor", "--json"])).unwrap();
                assert_eq!(out.stdout,b"{\n  \"success\": false,\n  \"command\": \"doctor\",\n  \"error\": \"Not in a missions project. Run `luuna-missions init` first.\"\n}\n");
                assert_eq!(out.stderr, b"");
                assert_eq!(out.exit_code, 1);
            }
            "sync" => {
                let temp = tempfile::tempdir().unwrap();
                let out = run(&Invocation::new(temp.path(), ["sync", "--json"])).unwrap();
                assert_eq!(out.stdout,b"{\n  \"success\": false,\n  \"command\": \"sync\",\n  \"error\": \"Not in a missions project. Run `luuna-missions init` first.\"\n}\n");
                assert_eq!(out.stderr, b"");
                assert_eq!(out.exit_code, 1);
            }
            "migrate-from-beads" => {
                let temp = tempfile::tempdir().unwrap();
                let out = run(&Invocation::new(
                    temp.path(),
                    ["migrate-from-beads", "--json"],
                ))
                .unwrap();
                assert_eq!(out.stdout,b"{\n  \"success\": false,\n  \"command\": \"migrate-from-beads\",\n  \"error\": \"Not in a missions project. Run `luuna-missions init` first.\"\n}\n");
                assert_eq!(out.stderr, b"");
                assert_eq!(out.exit_code, 1);
            }
            "upgrade" => {
                let temp = tempfile::tempdir().unwrap();
                let out = run(&Invocation::new(temp.path(), ["upgrade", "--json"])).unwrap();
                assert_eq!(out.stdout,b"{\n  \"success\": false,\n  \"command\": \"upgrade\",\n  \"error\": \"Luuna Missions is not published; update through a reviewed Luuna checkout.\",\n  \"updated\": false\n}\n");
                assert_eq!(out.stderr, b"");
                assert_eq!(out.exit_code, 1);
            }
            _ => panic!("no Go literal for {name}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::{find, Entry};
    use super::*;

    #[test]
    fn its_routes_point_at_this_module() {
        for (name, constant) in [
            ("sync", SYNC),
            ("doctor", DOCTOR),
            ("tpl", TPL),
            ("migrate-from-beads", MIGRATE_FROM_BEADS),
            ("prime", PRIME),
            ("onboard", ONBOARD),
            ("upgrade", UPGRADE),
            ("config", CONFIG),
            ("setup", SETUP),
            ("guard", GUARD),
        ] {
            match (find(name), constant) {
                (Some(Entry::Deferred(routed)), Entry::Deferred(own)) => {
                    assert_eq!(routed, own, "{name}");
                    assert_eq!(own, PLAN_3, "{name}");
                }
                (Some(Entry::Ported(_)), Entry::Ported(_)) => {}
                (None, _) => panic!("{name} is not in the command table"),
                _ => panic!("{name}: the command table and this module differ in kind"),
            }
        }
    }
}
