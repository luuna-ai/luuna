//! Setup/hook and human-review command handlers.
//!
//! This module deliberately keeps the process-facing behavior small and uses
//! the reusable policy/store primitives in `roots-core`.  It is kept separate
//! from the other command slices so the compatibility migration can land
//! independent command families without inventing a second dispatcher.

use crate::io::Io;
use crate::registry::CommandContext;
use roots_core::actor::{ActorContext, resolve_human_actor};
use roots_core::args::{flag_bool, flag_string};
use roots_core::claude_settings::{
    HookContext, RootsHookSpec, desired_hooks, installed_roots_hooks, sync_roots_hooks,
};
use roots_core::config::load_config;
use roots_core::decisions::{AcceptOptions, Decision, accept_proposal, reject_proposal};
use roots_core::errors::{RootsError, RootsResult};
use roots_core::events::{EventFields, append_event, make_event, read_events};
use roots_core::guard::refuse_agent_session;
use roots_core::hook_guard::{Decision as HookDecision, HookInput, decide_hook, deny_output};
use roots_core::mentions::{MentionFiling, all_lines, file_mentions};
use roots_core::model::{IdeaStatus, NodeKind, NodeStatus, ProposalStatus, SproutStatus};
use roots_core::paths::{find_project_root, roots_paths};
use roots_core::prose::{content_hash, read_node_prose};
use roots_core::resolve::{ResolveOptions, resolve_node};
use roots_core::serde_json;
use roots_core::serde_json::{Map, Value};
use roots_core::store::atomic_write;
use roots_core::time::iso_at;
use roots_core::workspace::open_workspace;
use std::fs;
use std::path::{Path, PathBuf};
use time::OffsetDateTime;

const SETUP_TARGETS: &[&str] = &["claude"];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Scope {
    Project,
    User,
}

impl Scope {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Project => "project",
            Self::User => "user",
        }
    }
}

fn scope_of(project: bool, user: bool) -> RootsResult<Scope> {
    if project && user {
        return Err(RootsError::usage("pass --project or --user, not both"));
    }
    Ok(if user { Scope::User } else { Scope::Project })
}

fn settings_path(io: &dyn Io, scope: Scope) -> RootsResult<PathBuf> {
    match scope {
        Scope::User => io
            .env("HOME")
            .filter(|home| !home.is_empty())
            .map(|home| Path::new(home).join(".claude").join("settings.json"))
            .ok_or_else(|| RootsError::usage("--user needs $HOME to find ~/.claude/settings.json")),
        Scope::Project => Ok(find_project_root(io.cwd())
            .unwrap_or_else(|| io.cwd().to_path_buf())
            .join(".claude")
            .join("settings.json")),
    }
}

fn read_settings(file: &Path) -> RootsResult<Map<String, Value>> {
    if !file.exists() {
        return Ok(Map::new());
    }
    let raw = fs::read_to_string(file).map_err(|error| RootsError::error(error.to_string()))?;
    if raw.trim().is_empty() {
        return Ok(Map::new());
    }
    let value: Value = serde_json::from_str(&raw).map_err(|error| {
        RootsError::validation(format!(
            "cannot parse {} ({}); fix it by hand, nothing was written",
            file.display(),
            error
        ))
    })?;
    value.as_object().cloned().ok_or_else(|| {
        RootsError::validation(format!(
            "{} is not a JSON object; nothing was written",
            file.display()
        ))
    })
}

fn project_tier(io: &dyn Io) -> RootsResult<Option<u8>> {
    let Some(root) = find_project_root(io.cwd()) else {
        return Ok(None);
    };
    Ok(Some(load_config(&roots_paths(root))?.tier))
}

fn refuse_agent_remove(io: &dyn Io) -> RootsResult<()> {
    let environment = io.environment();
    refuse_agent_session(&environment, "setup claude --remove")?;
    if io.env("CLAUDECODE").is_some_and(|value| !value.is_empty()) {
        return Err(RootsError::guard(
            "`luuna-roots setup claude --remove` would switch off the guard, and this looks like a Claude Code session (CLAUDECODE is set); run it yourself in a terminal",
        ));
    }
    Ok(())
}

fn describe(hook: &RootsHookSpec) -> String {
    match &hook.matcher {
        Some(matcher) => format!("{} [{}]: {}", hook.event.key(), matcher, hook.command),
        None => format!("{}: {}", hook.event.key(), hook.command),
    }
}

/// `luuna-roots setup claude`.
pub fn setup(context: CommandContext<'_>) -> RootsResult<()> {
    let target = context.args.first().map(String::as_str).unwrap_or_default();
    if !SETUP_TARGETS.contains(&target) {
        return Err(RootsError::usage(format!(
            "unknown setup target \"{target}\"; supported: {}",
            SETUP_TARGETS.join(", ")
        )));
    }
    let scope = scope_of(
        flag_bool(context.flags, "project"),
        flag_bool(context.flags, "user"),
    )?;
    let remove = flag_bool(context.flags, "remove");
    let dry_run = flag_bool(context.flags, "dry-run");
    if remove {
        refuse_agent_remove(context.io)?;
    }
    let file = settings_path(context.io, scope)?;
    let settings = read_settings(&file)?;
    let tier = if scope == Scope::Project {
        project_tier(context.io)?
    } else {
        None
    };
    let wanted = if remove {
        Vec::new()
    } else {
        desired_hooks(HookContext {
            tier,
            has_command: &|name| name == "drift",
        })
    };
    let merged = sync_roots_hooks(&settings, &wanted).map_err(|error| {
        RootsError::validation(format!(
            "{}: {}; nothing was written",
            file.display(),
            error
        ))
    })?;
    let text = format!(
        "{}\n",
        serde_json::to_string_pretty(&merged.settings)
            .map_err(|error| RootsError::error(error.to_string()))?
    );
    if merged.changed && !dry_run {
        atomic_write(&file, &text)?;
    }
    let hooks = installed_roots_hooks(&merged.settings);
    let mut result = Map::new();
    result.insert("target".to_owned(), Value::String(target.to_owned()));
    result.insert("scope".to_owned(), Value::String(scope.as_str().to_owned()));
    result.insert(
        "path".to_owned(),
        Value::String(file.to_string_lossy().into_owned()),
    );
    result.insert(
        "action".to_owned(),
        Value::String(if remove { "remove" } else { "install" }.to_owned()),
    );
    result.insert("changed".to_owned(), Value::Bool(merged.changed));
    result.insert("dryRun".to_owned(), Value::Bool(dry_run));
    result.insert(
        "hooks".to_owned(),
        Value::Array(
            hooks
                .iter()
                .map(|hook| serde_json::to_value(hook).expect("hook serializes"))
                .collect(),
        ),
    );
    if dry_run {
        result.insert(
            "settings".to_owned(),
            Value::Object(merged.settings.clone()),
        );
    }
    context.out.result(context.io, result);
    if dry_run {
        context.out.line(
            context.io,
            &format!("# {} (dry run, not written)", file.display()),
        );
        context.out.line(context.io, text.trim_end());
        return Ok(());
    }
    if merged.changed {
        let verb = if remove {
            "removed roots hooks from"
        } else {
            "installed roots hooks in"
        };
        context
            .out
            .success(context.io, &format!("{verb} {}", file.display()));
    } else {
        context.out.info(
            context.io,
            &format!("{}: already up to date", file.display()),
        );
    }
    for hook in hooks {
        context
            .out
            .info(context.io, &format!("  {}", describe(&hook)));
    }
    Ok(())
}

fn command_hook_input(raw: &str) -> Option<HookInput> {
    let value: Value = serde_json::from_str(raw).ok()?;
    value.as_object()?;
    Some(HookInput::from_value(&value))
}

fn guard_result(decision: &roots_core::hook_guard::GuardDecision) -> Map<String, Value> {
    let mut result = Map::new();
    result.insert(
        "decision".to_owned(),
        Value::String(decision.decision.as_str().to_owned()),
    );
    if let Some(reason) = &decision.reason {
        result.insert("reason".to_owned(), Value::String(reason.clone()));
    }
    if let Some(target) = &decision.target {
        result.insert("target".to_owned(), Value::String(target.clone()));
    }
    result
}

/// `luuna-roots guard`, Claude's fail-open PreToolUse hook entrypoint.
pub fn guard(context: CommandContext<'_>) -> RootsResult<()> {
    let raw = if context.io.stdin_is_tty() {
        String::new()
    } else {
        context
            .io
            .read_stdin()
            .map_err(|error| RootsError::error(error.to_string()))?
    };
    let Some(input) = command_hook_input(&raw) else {
        context
            .io
            .stderr("luuna-roots guard: no hook JSON on stdin; allowing\n");
        let mut result = Map::new();
        result.insert("decision".to_owned(), Value::String("allow".to_owned()));
        result.insert(
            "reason".to_owned(),
            Value::String("no hook input".to_owned()),
        );
        context.out.result(context.io, result);
        return Ok(());
    };
    let decision = decide_hook(&input, context.io.cwd());
    context.out.result(context.io, guard_result(&decision));
    if !context.out.json && decision.decision == HookDecision::Deny {
        context.io.stdout(&deny_output(
            decision
                .reason
                .as_deref()
                .unwrap_or("denied by luuna-roots guard"),
        ));
    }
    Ok(())
}

fn human_actor(io: &dyn Io) -> RootsResult<String> {
    let environment = io.environment();
    resolve_human_actor(ActorContext::new(io.cwd(), &environment))
}

fn human_structure_actor(io: &dyn Io, command: &str) -> RootsResult<String> {
    let environment = io.environment();
    roots_core::guard::require_human_command(ActorContext::new(io.cwd(), &environment), command)
}

fn value_object(value: Value) -> RootsResult<Map<String, Value>> {
    value
        .as_object()
        .cloned()
        .ok_or_else(|| RootsError::error("internal decision JSON was not an object"))
}

fn decision_result(decision: &Decision) -> RootsResult<Map<String, Value>> {
    value_object(
        serde_json::to_value(decision).map_err(|error| RootsError::error(error.to_string()))?,
    )
}

fn decision_lines(context: &CommandContext<'_>, decision: &Decision) -> Vec<String> {
    let mut lines = Vec::new();
    for proposal in &decision.proposals {
        let mark = if proposal.status == ProposalStatus::Accepted {
            context.out.colors.green("✓ accepted")
        } else {
            context.out.colors.yellow("✗ rejected")
        };
        let why = proposal
            .decision_reason
            .as_deref()
            .map(|reason| context.out.colors.dim(&format!(" ({reason})")))
            .unwrap_or_default();
        let kind = match proposal.kind {
            roots_core::model::ProposalKind::Edge => "edge",
            roots_core::model::ProposalKind::Split => "split",
            roots_core::model::ProposalKind::Merge => "merge",
            roots_core::model::ProposalKind::Compost => "compost",
        };
        lines.push(format!(
            "{mark} {} {kind}{why}",
            context.out.colors.id(&proposal.id)
        ));
    }
    if let Some(edge) = &decision.edge {
        lines.push(format!(
            "  {} {} {} {} {}",
            context.out.colors.green("+"),
            context.out.colors.id(&edge.id),
            edge.from,
            edge.rel.as_str(),
            edge.to
        ));
    }
    for change in &decision.status_changes {
        lines.push(format!("  {} {} → {}", change.node, change.from, change.to));
    }
    if let Some(think) = &decision.think {
        lines.push(format!(
            "  do the split yourself: `luuna-roots think {}`",
            think.node
        ));
        lines.push(
            context
                .out
                .colors
                .dim(&format!("  suggested: {}", think.guidance)),
        );
    }
    lines
}

fn sprout_id(raw: &str) -> Option<String> {
    let id = raw.trim().to_lowercase();
    (roots_core::ids::prefix_of(&id) == Some("s")).then_some(id)
}

fn reject_sprout(
    paths: &roots_core::paths::RootsPaths,
    by: &str,
    raw: &str,
    reason: Option<&str>,
) -> RootsResult<(roots_core::model::NodeRecord, String)> {
    let id = sprout_id(raw).ok_or_else(|| {
        RootsError::usage(format!(
            "expected a proposal (p-xxxx) or sprout (s-xxxx) id, got \"{raw}\""
        ))
    })?;
    let now = OffsetDateTime::now_utc();
    roots_core::sprouts::expire_sprouts(paths, Some(now))?;
    let why = reason
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);
    let node = roots_core::graph::update_graph(paths, |graph| {
        let Some(node) = graph.nodes.iter_mut().find(|node| node.id == id) else {
            return Err(RootsError::not_found(format!(
                "no sprout {id}; `luuna-roots list --kind sprout` lists them"
            )));
        };
        if node.kind != NodeKind::Sprout {
            return Err(RootsError::not_found(format!(
                "no sprout {id}; `luuna-roots list --kind sprout` lists them"
            )));
        }
        if node.status != NodeStatus::Sprout(SproutStatus::Open) {
            let who = node
                .decided_by
                .as_deref()
                .map(|actor| format!(" by {actor}"))
                .unwrap_or_default();
            let mut detail = Map::new();
            detail.insert(
                "status".to_owned(),
                Value::String(node.status.as_str().to_owned()),
            );
            return Err(RootsError::conflict(format!(
                "{id} is already {}{who}",
                node.status.as_str()
            ))
            .with_detail(detail));
        }
        node.status = NodeStatus::Sprout(SproutStatus::Rejected);
        node.decided_by = Some(by.to_owned());
        node.decided_at = Some(iso_at(now));
        node.updated_at = node.decided_at.clone();
        node.decision_reason = why.clone();
        Ok((true, node.clone()))
    })?;
    let mut extra = Map::new();
    extra.insert("sprout".to_owned(), Value::String(id.clone()));
    extra.insert("kind".to_owned(), Value::String("sprout".to_owned()));
    if let Some(reason) = &why {
        extra.insert("reason".to_owned(), Value::String(reason.clone()));
    }
    append_event(
        paths,
        &make_event(
            "reject",
            by,
            EventFields {
                node: Some(id.clone()),
                extra,
                ..Default::default()
            },
        ),
    )?;
    Ok((node, id))
}

fn sprout_rejection_output(id: &str, node: &roots_core::model::NodeRecord) -> Map<String, Value> {
    let mut body = Map::new();
    body.insert("id".to_owned(), Value::String(id.to_owned()));
    body.insert("action".to_owned(), Value::String("reject".to_owned()));
    body.insert("proposals".to_owned(), Value::Array(Vec::new()));
    body.insert("edge".to_owned(), Value::Null);
    body.insert("statusChanges".to_owned(), Value::Array(Vec::new()));
    body.insert("think".to_owned(), Value::Null);
    body.insert(
        "sprout".to_owned(),
        serde_json::to_value(node).expect("node serializes"),
    );
    body.insert("idea".to_owned(), Value::Null);
    body.insert("path".to_owned(), Value::Null);
    body
}

fn relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .display()
        .to_string()
}

fn adopted_sprout_output(
    id: &str,
    result: &crate::commands_capture::AdoptionResult,
    root: &Path,
) -> Map<String, Value> {
    let file = relative(root, &result.file);
    let mut body = Map::new();
    body.insert("id".to_owned(), Value::String(id.to_owned()));
    body.insert("action".to_owned(), Value::String("accept".to_owned()));
    body.insert("proposals".to_owned(), Value::Array(Vec::new()));
    body.insert(
        "edge".to_owned(),
        serde_json::to_value(&result.edge).expect("edge serializes"),
    );
    body.insert("statusChanges".to_owned(), Value::Array(Vec::new()));
    body.insert("think".to_owned(), Value::Null);
    body.insert(
        "sprout".to_owned(),
        serde_json::to_value(&result.sprout).expect("sprout serializes"),
    );
    body.insert(
        "idea".to_owned(),
        serde_json::to_value(&result.idea).expect("idea serializes"),
    );
    body.insert("path".to_owned(), Value::String(file));
    body
}

fn adopted_sprout_lines(
    context: &CommandContext<'_>,
    result: &crate::commands_capture::AdoptionResult,
    root: &Path,
) -> Vec<String> {
    let file = relative(root, &result.file);
    let mut lines = vec![format!(
        "{} {} as {} {}",
        context.out.colors.green("✓ adopted"),
        context.out.colors.id(&result.sprout.id),
        context.out.colors.id(&result.idea.id),
        result.idea.slug
    )];
    lines.push(format!(
        "  {} {} {} derives {}",
        context.out.colors.green("+"),
        context.out.colors.id(&result.edge.id),
        result.edge.from,
        result.edge.to
    ));
    if !file.is_empty() {
        lines.push(context.out.colors.dim(&format!("  {file}")));
    }
    lines
}

/// `luuna-roots accept <p-id|s-id>` for proposal cards and interactive sprout
/// adoption. The latter delegates to the shared editor/split-pane flow used by
/// `adopt`, preserving the same human-authored-content boundary.
pub fn accept(mut context: CommandContext<'_>) -> RootsResult<()> {
    let by = human_structure_actor(context.io, "accept")?;
    let workspace = open_workspace(context.io.cwd())?;
    let raw = context.args.first().map(String::as_str).unwrap_or_default();
    if let Some(id) = sprout_id(raw) {
        let environment = context.io.environment();
        if !roots_core::guard::has_interactive_tty(&environment, context.io.stdin_is_tty()) {
            return Err(RootsError::guard(format!(
                "accepting {id} means adopting it: you write the idea yourself in $EDITOR, so it needs an interactive terminal. Run `luuna-roots adopt {id}` in a terminal (or `luuna-roots reject {id}`)"
            )));
        }
        let result = crate::commands_capture::run_adoption(
            context.io,
            &workspace.paths,
            &id,
            crate::commands_capture::AdoptionOptions {
                by: &by,
                split: true,
                slug: None,
                spawn: None,
                endorse: None,
            },
        )?;
        context.out.result(
            context.io,
            adopted_sprout_output(&id, &result, &workspace.paths.root),
        );
        if !context.out.quiet {
            let lines = adopted_sprout_lines(&context, &result, &workspace.paths.root);
            context.out.lines(context.io, &lines);
        }
        crate::commands_capture::print_next(&mut context, &workspace.paths);
        return Ok(());
    }
    let decision = accept_proposal(
        &workspace.paths,
        &by,
        raw,
        AcceptOptions {
            rel: flag_string(context.flags, "rel").map(ToOwned::to_owned),
            keep: flag_string(context.flags, "keep").map(ToOwned::to_owned),
            reason: flag_string(context.flags, "reason").map(ToOwned::to_owned),
        },
        None,
    )?;
    context.out.result(context.io, decision_result(&decision)?);
    if !context.out.quiet {
        let lines = decision_lines(&context, &decision);
        context.out.lines(context.io, &lines);
    }
    crate::commands_capture::print_next(&mut context, &workspace.paths);
    Ok(())
}

/// `luuna-roots reject <p-id|s-id>`.
pub fn reject(mut context: CommandContext<'_>) -> RootsResult<()> {
    let by = human_structure_actor(context.io, "reject")?;
    let workspace = open_workspace(context.io.cwd())?;
    let raw = context.args.first().map(String::as_str).unwrap_or_default();
    let reason = flag_string(context.flags, "reason");
    if sprout_id(raw).is_some() {
        let (node, id) = reject_sprout(&workspace.paths, &by, raw, reason)?;
        context
            .out
            .result(context.io, sprout_rejection_output(&id, &node));
        if !context.out.quiet {
            let why = node
                .decision_reason
                .as_deref()
                .map(|reason| context.out.colors.dim(&format!(" ({reason})")))
                .unwrap_or_default();
            context.out.lines(
                context.io,
                &[format!(
                    "{} {} sprout {}{}",
                    context.out.colors.yellow("✗ rejected"),
                    context.out.colors.id(&node.id),
                    node.slug,
                    why
                )],
            );
        }
        crate::commands_capture::print_next(&mut context, &workspace.paths);
        return Ok(());
    }
    let decision = reject_proposal(&workspace.paths, &by, raw, reason, None)?;
    context.out.result(context.io, decision_result(&decision)?);
    if !context.out.quiet {
        let lines = decision_lines(&context, &decision);
        context.out.lines(context.io, &lines);
    }
    crate::commands_capture::print_next(&mut context, &workspace.paths);
    Ok(())
}

fn last_recorded_hash(events: &[roots_core::model::EventRecord], id: &str) -> Option<String> {
    let mut last = None;
    for event in events {
        if event.node.as_deref() != Some(id) {
            continue;
        }
        let field = match event.event_type.as_str() {
            "plant" | "session.end" => "hash",
            "scan" => "scannedHash",
            _ => continue,
        };
        if let Some(hash) = event.extra.get(field).and_then(Value::as_str) {
            last = Some(hash.to_owned());
        }
    }
    last
}

#[derive(Clone, Debug)]
struct ScanItem {
    node: roots_core::model::NodeRecord,
    changed: bool,
    filing: Option<MentionFiling>,
}

fn scan_item_json(item: &ScanItem) -> Map<String, Value> {
    let mut body = Map::new();
    body.insert("node".to_owned(), Value::String(item.node.id.clone()));
    body.insert("slug".to_owned(), Value::String(item.node.slug.clone()));
    body.insert("changed".to_owned(), Value::Bool(item.changed));
    body.insert("scanned".to_owned(), Value::Bool(item.filing.is_some()));
    body.insert(
        "mentions".to_owned(),
        item.filing.as_ref().map_or_else(
            || Value::Array(Vec::new()),
            |filing| {
                Value::Array(
                    filing
                        .mentions
                        .iter()
                        .map(|mention| serde_json::to_value(mention).expect("mention serializes"))
                        .collect(),
                )
            },
        ),
    );
    body.insert(
        "filed".to_owned(),
        item.filing.as_ref().map_or_else(
            || Value::Array(Vec::new()),
            |filing| {
                Value::Array(
                    filing
                        .result
                        .filed
                        .iter()
                        .map(|proposal| {
                            serde_json::to_value(proposal).expect("proposal serializes")
                        })
                        .collect(),
                )
            },
        ),
    );
    body.insert(
        "skipped".to_owned(),
        item.filing.as_ref().map_or_else(
            || Value::Array(Vec::new()),
            |filing| {
                Value::Array(
                    filing
                        .result
                        .skipped
                        .iter()
                        .map(|skip| {
                            let mut value = Map::new();
                            value.insert(
                                "to".to_owned(),
                                skip.to.clone().map_or(Value::Null, Value::String),
                            );
                            value.insert("reason".to_owned(), Value::String(skip.reason.clone()));
                            Value::Object(value)
                        })
                        .collect(),
                )
            },
        ),
    );
    body
}

/// `luuna-roots scan [<id>]`.
pub fn scan(context: CommandContext<'_>) -> RootsResult<()> {
    let by = human_actor(context.io)?;
    let workspace = open_workspace(context.io.cwd())?;
    let all = flag_bool(context.flags, "all");
    let targets = if let Some(query) = context.args.first() {
        vec![
            resolve_node(
                &workspace.graph.nodes,
                query,
                ResolveOptions {
                    kind: Some(NodeKind::Idea),
                },
            )?
            .clone(),
        ]
    } else {
        workspace
            .graph
            .nodes
            .iter()
            .filter(|node| {
                node.kind == NodeKind::Idea
                    && !matches!(node.status, NodeStatus::Idea(IdeaStatus::Composted))
            })
            .cloned()
            .collect()
    };
    let events = read_events(&workspace.paths)?;
    let now = OffsetDateTime::now_utc();
    let config = load_config(&workspace.paths)?;
    let mut items = Vec::new();
    for node in targets {
        let prose = read_node_prose(&workspace.paths, &node, Some(&workspace.dirs))?;
        if !prose.exists {
            items.push(ScanItem {
                node,
                changed: false,
                filing: None,
            });
            continue;
        }
        let hash = content_hash(&prose.text);
        let changed = last_recorded_hash(&events, &node.id).as_deref() != Some(&hash);
        if !changed && !all {
            items.push(ScanItem {
                node,
                changed,
                filing: None,
            });
            continue;
        }
        let filing = file_mentions(
            &workspace.paths,
            &config,
            &workspace.graph,
            &node.id,
            &all_lines(&prose.text),
            now,
        )?;
        let filed = filing
            .result
            .filed
            .iter()
            .map(|proposal| Value::String(proposal.id.clone()))
            .collect();
        if changed || !filing.result.filed.is_empty() {
            let mut extra = Map::new();
            extra.insert("scannedHash".to_owned(), Value::String(hash));
            extra.insert("proposals".to_owned(), Value::Array(filed));
            append_event(
                &workspace.paths,
                &make_event(
                    "scan",
                    by.clone(),
                    EventFields {
                        node: Some(node.id.clone()),
                        extra,
                        ..Default::default()
                    },
                ),
            )?;
        }
        items.push(ScanItem {
            node,
            changed,
            filing: Some(filing),
        });
    }
    let mut result = Map::new();
    result.insert(
        "ideas".to_owned(),
        Value::Array(
            items
                .iter()
                .map(|item| Value::Object(scan_item_json(item)))
                .collect(),
        ),
    );
    context.out.result(context.io, result);
    let scanned = items
        .iter()
        .filter(|item| item.filing.is_some())
        .collect::<Vec<_>>();
    let filed = scanned
        .iter()
        .flat_map(|item| {
            item.filing
                .as_ref()
                .into_iter()
                .flat_map(|filing| filing.result.filed.iter())
        })
        .collect::<Vec<_>>();
    for item in &scanned {
        for proposal in &item
            .filing
            .as_ref()
            .expect("scanned item has filing")
            .result
            .filed
        {
            context.out.line(
                context.io,
                &format!(
                    "{} {} {} ─?─ {}  {}",
                    context.out.colors.green("+"),
                    context.out.colors.id(&proposal.id),
                    item.node.slug,
                    proposal.to.as_deref().unwrap_or_default(),
                    context
                        .out
                        .colors
                        .dim(proposal.reason.as_deref().unwrap_or_default())
                ),
            );
        }
    }
    let summary = format!(
        "scanned {} changed idea{}; {} proposal{} filed",
        scanned.len(),
        if scanned.len() == 1 { "" } else { "s" },
        filed.len(),
        if filed.len() == 1 { "" } else { "s" }
    );
    context.out.success(context.io, &summary);
    if !filed.is_empty() {
        context.out.info(
            context.io,
            &context
                .out
                .colors
                .dim("review them with `luuna-roots tend`"),
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::MemoryIo;
    use crate::run_cli;
    use roots_core::config::{default_config, write_config};
    use roots_core::events::{EventFields, append_event, make_event, read_events};
    use roots_core::graph::{graph_records, read_graph};
    use roots_core::model::{Graph, NodeRecord};
    use roots_core::paths::roots_paths;
    use roots_core::proposals::read_proposals;
    use roots_core::serde_json::Value;
    use roots_core::store::write_jsonl_file;

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|part| (*part).to_owned()).collect()
    }

    fn sandbox(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "roots-review-command-{name}-{}",
            roots_core::ids::random_hex(8)
        ));
        fs::create_dir_all(&root).expect("create sandbox");
        root
    }

    #[test]
    fn guard_denies_human_intent_writes_and_keeps_hook_protocol_plain() {
        let root = sandbox("guard");
        let mut io = MemoryIo::new(&root);
        io.stdin_is_tty = false;
        io.stdin =
            r#"{"tool_name":"Edit","tool_input":{"file_path":".luuna/roots/human/a/idea.md"}}"#
                .to_owned();
        assert_eq!(run_cli(&argv(&["guard"]), &mut io), 0);
        assert!(io.stdout.contains("hookSpecificOutput"));
        assert!(io.stdout.contains("permissionDecision\":\"deny"));
        assert!(io.stderr.is_empty());
        fs::remove_dir_all(root).expect("clean sandbox");
    }

    #[test]
    fn guard_json_reports_a_fail_open_for_missing_input() {
        let root = sandbox("guard-json");
        let mut io = MemoryIo::new(&root);
        io.stdin_is_tty = false;
        assert_eq!(run_cli(&argv(&["guard", "--json"]), &mut io), 0);
        let body: Value = serde_json::from_str(&io.stdout).expect("JSON envelope");
        assert_eq!(body["decision"], "allow");
        assert_eq!(body["reason"], "no hook input");
        assert!(io.stderr.contains("no hook JSON"));
        fs::remove_dir_all(root).expect("clean sandbox");
    }

    #[test]
    fn setup_dry_run_preserves_the_settings_file_boundary() {
        let root = sandbox("setup");
        let mut io = MemoryIo::new(&root);
        assert_eq!(
            run_cli(&argv(&["setup", "claude", "--dry-run", "--json"]), &mut io),
            0
        );
        let body: Value = serde_json::from_str(&io.stdout).expect("JSON envelope");
        assert_eq!(body["action"], "install");
        assert_eq!(body["scope"], "project");
        assert_eq!(body["dryRun"], true);
        assert_eq!(body["hooks"].as_array().map(Vec::len), Some(2));
        assert!(!root.join(".claude/settings.json").exists());
        fs::remove_dir_all(root).expect("clean sandbox");
    }

    #[test]
    fn setup_install_is_idempotent_and_remove_keeps_user_hooks() {
        let root = sandbox("setup-install-remove");
        let settings = root.join(".claude/settings.json");
        fs::create_dir_all(settings.parent().expect("settings parent")).expect("claude directory");
        fs::write(
            &settings,
            r#"{"model":"opus","hooks":{"PreToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"other hook"}]}]}}"#,
        )
        .expect("initial settings");
        let mut install = MemoryIo::new(&root);
        assert_eq!(
            run_cli(&argv(&["setup", "claude", "--json"]), &mut install),
            0
        );
        let installed: Value = serde_json::from_str(&install.stdout).expect("install envelope");
        assert_eq!(installed["changed"], true);
        assert_eq!(installed["hooks"].as_array().map(Vec::len), Some(2));
        let saved: Value =
            serde_json::from_str(&fs::read_to_string(&settings).expect("saved settings"))
                .expect("saved json");
        assert_eq!(saved["model"], "opus");
        assert_eq!(
            saved["hooks"]["PreToolUse"][0]["hooks"][0]["command"],
            "other hook"
        );

        let mut rerun = MemoryIo::new(&root);
        assert_eq!(
            run_cli(&argv(&["setup", "claude", "--json"]), &mut rerun),
            0
        );
        let idempotent: Value = serde_json::from_str(&rerun.stdout).expect("rerun envelope");
        assert_eq!(idempotent["changed"], false);

        let mut remove = MemoryIo::new(&root);
        assert_eq!(
            run_cli(
                &argv(&["setup", "claude", "--remove", "--json"]),
                &mut remove
            ),
            0
        );
        let removed: Value = serde_json::from_str(&remove.stdout).expect("remove envelope");
        assert_eq!(removed["action"], "remove");
        assert_eq!(removed["changed"], true);
        assert_eq!(removed["hooks"].as_array().map(Vec::len), Some(0));
        let after: Value =
            serde_json::from_str(&fs::read_to_string(&settings).expect("after settings"))
                .expect("after json");
        assert_eq!(after["model"], "opus");
        assert_eq!(
            after["hooks"]["PreToolUse"][0]["hooks"][0]["command"],
            "other hook"
        );
        fs::remove_dir_all(root).expect("clean sandbox");
    }

    #[test]
    fn setup_remove_preserves_the_empty_hooks_object_and_pretty_json_bytes() {
        let root = sandbox("setup-remove-empty-hooks");
        let settings = root.join(".claude/settings.json");
        let mut install = MemoryIo::new(&root);
        assert_eq!(
            run_cli(&argv(&["setup", "claude", "--json"]), &mut install),
            0
        );

        let mut remove = MemoryIo::new(&root);
        assert_eq!(
            run_cli(
                &argv(&["setup", "claude", "--remove", "--json"]),
                &mut remove
            ),
            0
        );
        assert_eq!(
            fs::read_to_string(&settings).expect("removed settings"),
            "{\n  \"hooks\": {}\n}\n"
        );
        fs::remove_dir_all(root).expect("clean sandbox");
    }

    #[test]
    fn scan_files_one_mention_then_uses_its_hash_ledger_on_rerun() {
        let root = sandbox("scan");
        let paths = roots_paths(&root);
        write_config(&paths, &default_config("scan-test")).expect("write config");
        let source = NodeRecord::new(
            "r-a1b2".to_owned(),
            NodeKind::Idea,
            "source".to_owned(),
            NodeStatus::Idea(IdeaStatus::Planted),
            "human:test".to_owned(),
            "2026-01-01T00:00:00Z".to_owned(),
        );
        let target = NodeRecord::new(
            "r-c3d4".to_owned(),
            NodeKind::Idea,
            "offline-sync".to_owned(),
            NodeStatus::Idea(IdeaStatus::Planted),
            "human:test".to_owned(),
            "2026-01-01T00:00:00Z".to_owned(),
        );
        write_jsonl_file(
            &paths.graph,
            &graph_records(&Graph {
                nodes: vec![source.clone(), target.clone()],
                edges: Vec::new(),
            }),
        )
        .expect("write graph");

        let original = "Source thought\n";
        let edited = "Source thought\nThis depends on r-c3d4.\n";
        let target_text = "Offline sync survives a disconnected network\n";
        let source_file = paths.human.join("a1b2-source").join("idea.md");
        let target_file = paths.human.join("c3d4-offline-sync").join("idea.md");
        fs::create_dir_all(source_file.parent().expect("source parent")).expect("source directory");
        fs::create_dir_all(target_file.parent().expect("target parent")).expect("target directory");
        fs::write(&source_file, edited).expect("edited source prose");
        fs::write(&target_file, target_text).expect("target prose");
        for (node, text) in [(&source, original), (&target, target_text)] {
            let mut extra = Map::new();
            extra.insert("hash".to_owned(), Value::String(content_hash(text)));
            append_event(
                &paths,
                &make_event(
                    "plant",
                    "human:test",
                    EventFields {
                        node: Some(node.id.clone()),
                        extra,
                        ..Default::default()
                    },
                ),
            )
            .expect("write plant hash");
        }

        let mut first = MemoryIo::new(&root);
        first.env.insert("ROOTS_USER".to_owned(), "test".to_owned());
        assert_eq!(run_cli(&argv(&["scan", "--json"]), &mut first), 0);
        let first_body: Value = serde_json::from_str(&first.stdout).expect("scan envelope");
        let ideas = first_body["ideas"].as_array().expect("scan ideas");
        assert_eq!(ideas.len(), 2);
        assert_eq!(ideas[0]["node"], source.id);
        assert_eq!(ideas[0]["changed"], true);
        assert_eq!(ideas[0]["scanned"], true);
        assert_eq!(ideas[0]["mentions"].as_array().map(Vec::len), Some(1));
        assert_eq!(ideas[0]["filed"].as_array().map(Vec::len), Some(1));
        assert_eq!(ideas[1]["changed"], false);
        assert_eq!(ideas[1]["scanned"], false);

        let proposals = read_proposals(&paths).expect("read filed proposal");
        assert_eq!(proposals.len(), 1);
        assert_eq!(proposals[0].from.as_deref(), Some(source.id.as_str()));
        assert_eq!(proposals[0].to.as_deref(), Some(target.id.as_str()));
        assert_eq!(proposals[0].rel, None);
        let events = read_events(&paths).expect("read scan event");
        assert!(events.iter().any(|event| {
            event.event_type == "scan"
                && event.node.as_deref() == Some(source.id.as_str())
                && event.extra.get("scannedHash").is_some()
                && event.extra["proposals"].as_array().map(Vec::len) == Some(1)
        }));

        let mut second = MemoryIo::new(&root);
        second
            .env
            .insert("ROOTS_USER".to_owned(), "test".to_owned());
        assert_eq!(run_cli(&argv(&["scan", "--json"]), &mut second), 0);
        let second_body: Value = serde_json::from_str(&second.stdout).expect("rescan envelope");
        assert!(
            second_body["ideas"]
                .as_array()
                .expect("rescan ideas")
                .iter()
                .all(|idea| idea["changed"] == false && idea["scanned"] == false)
        );
        assert_eq!(
            read_graph(&paths)
                .expect("graph remains accepted-only")
                .edges
                .len(),
            0
        );
        fs::remove_dir_all(root).expect("clean sandbox");
    }

    #[test]
    fn accept_sprout_without_a_tty_keeps_the_adopt_handoff_error() {
        let root = sandbox("accept-sprout-no-tty");
        let paths = roots_paths(&root);
        write_config(&paths, &default_config("accept-sprout")).expect("write config");
        write_jsonl_file(&paths.graph, &graph_records(&Graph::default())).expect("write graph");
        let mut io = MemoryIo::new(&root);
        io.stdin_is_tty = false;
        io.env.insert("ROOTS_USER".to_owned(), "test".to_owned());

        assert_eq!(
            run_cli(&argv(&["accept", "s-9f3e", "--json"]), &mut io),
            roots_core::errors::EXIT_GUARD
        );
        let body: Value = serde_json::from_str(&io.stdout).expect("guard envelope");
        assert_eq!(body["code"], "guard");
        assert!(
            body["error"]
                .as_str()
                .is_some_and(|message| message.contains("luuna-roots adopt s-9f3e"))
        );
        fs::remove_dir_all(root).expect("clean sandbox");
    }

    #[test]
    fn reject_sprout_updates_the_graph_and_emits_a_permanent_rejection() {
        let root = sandbox("reject-sprout");
        let paths = roots_paths(&root);
        write_config(&paths, &default_config("reject-sprout")).expect("write config");
        let sprout = NodeRecord::new(
            "s-a1b2".to_owned(),
            NodeKind::Sprout,
            "merge-ui".to_owned(),
            NodeStatus::Sprout(SproutStatus::Open),
            "agent:test".to_owned(),
            "2026-01-01T00:00:00Z".to_owned(),
        );
        write_jsonl_file(
            &paths.graph,
            &graph_records(&Graph {
                nodes: vec![sprout],
                edges: Vec::new(),
            }),
        )
        .expect("write graph");
        let mut io = MemoryIo::new(&root);
        io.env.insert("ROOTS_USER".to_owned(), "test".to_owned());

        assert_eq!(
            run_cli(
                &argv(&["reject", "s-a1b2", "--reason", "not now", "--json"]),
                &mut io
            ),
            0
        );
        let body: Value = serde_json::from_str(&io.stdout).expect("rejection envelope");
        assert_eq!(body["sprout"]["status"], "rejected");
        assert_eq!(body["sprout"]["decisionReason"], "not now");
        let stored = read_graph(&paths)
            .expect("read graph")
            .nodes
            .into_iter()
            .next()
            .expect("sprout remains in graph");
        assert_eq!(stored.status, NodeStatus::Sprout(SproutStatus::Rejected));
        assert_eq!(stored.decided_by.as_deref(), Some("human:test"));
        let events = read_events(&paths).expect("read rejection event");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, "reject");
        assert_eq!(events[0].node.as_deref(), Some("s-a1b2"));
        assert_eq!(
            events[0].extra.get("reason"),
            Some(&Value::String("not now".to_owned()))
        );
        fs::remove_dir_all(root).expect("clean sandbox");
    }

    #[test]
    fn accept_sprout_on_a_tty_runs_the_shared_adoption_flow() {
        let root = sandbox("accept-sprout");
        let paths = roots_paths(&root);
        // `init` creates the human-owned tree before any adoption.  Mirror
        // that initialized workspace here rather than relying on adoption to
        // create a human-owned directory.
        fs::create_dir_all(&paths.human).expect("human directory");
        write_config(&paths, &default_config("accept-sprout")).expect("write config");
        let sprout = NodeRecord::new(
            "s-a1b2".to_owned(),
            NodeKind::Sprout,
            "conflicts-get-merge-ui".to_owned(),
            NodeStatus::Sprout(SproutStatus::Open),
            "agent:test".to_owned(),
            "2026-01-01T00:00:00Z".to_owned(),
        );
        write_jsonl_file(
            &paths.graph,
            &graph_records(&Graph {
                nodes: vec![sprout.clone()],
                edges: Vec::new(),
            }),
        )
        .expect("write graph");
        let sprout_file = paths
            .sprouts
            .join("a1b2-conflicts-get-merge-ui")
            .join("sprout.md");
        fs::create_dir_all(sprout_file.parent().expect("sprout parent")).expect("sprout directory");
        fs::write(
            &sprout_file,
            "Conflicts get a merge UI\n\nAgent reference text.\n",
        )
        .expect("sprout prose");
        let editor = {
            #[cfg(windows)]
            {
                // Use a real command file instead of an inline redirection:
                // `cmd /C`'s outer-quote parsing changes when the test suite
                // runs other commands concurrently.  This is the same
                // contract exercised by the native editor unit test.
                let script = root.join("write idea.cmd");
                fs::write(&script, "@echo My own claim> \"%~1\"\r\n")
                    .expect("write Windows editor");
                format!("\"{}\"", script.display())
            }
            #[cfg(not(windows))]
            {
                use std::os::unix::fs::PermissionsExt;

                let script = root.join("write-idea.sh");
                fs::write(&script, "#!/bin/sh\nprintf 'My own claim\\n' > \"$1\"\n")
                    .expect("write POSIX editor");
                let mut permissions = fs::metadata(&script)
                    .expect("editor metadata")
                    .permissions();
                permissions.set_mode(0o755);
                fs::set_permissions(&script, permissions).expect("make editor executable");
                format!("\"{}\"", script.display())
            }
        };
        let mut io = MemoryIo::new(&root);
        io.env.insert("ROOTS_USER".to_owned(), "test".to_owned());
        io.env.insert("EDITOR".to_owned(), editor);
        #[cfg(windows)]
        for key in ["SystemRoot", "PATH", "PATHEXT"] {
            if let Ok(value) = std::env::var(key) {
                io.env.insert(key.to_owned(), value);
            }
        }

        assert_eq!(
            run_cli(&argv(&["accept", &sprout.id, "--json"]), &mut io),
            0,
            "stdout: {}\nstderr: {}",
            io.stdout,
            io.stderr
        );
        let body: Value = serde_json::from_str(&io.stdout).expect("adoption envelope");
        assert_eq!(body["id"], sprout.id);
        assert_eq!(body["action"], "accept");
        assert_eq!(body["sprout"]["status"], "adopted");
        assert_eq!(body["idea"]["kind"], "idea");
        assert_eq!(body["idea"]["slug"], "my-own-claim");
        assert_eq!(body["edge"]["rel"], "derives");
        assert_eq!(body["edge"]["to"], sprout.id);
        assert!(body["path"].as_str().is_some_and(|path| !path.is_empty()));
        let graph = read_graph(&paths).expect("read adopted graph");
        assert_eq!(graph.edges.len(), 1);
        assert_eq!(
            graph
                .nodes
                .iter()
                .find(|node| node.id == sprout.id)
                .map(|node| node.status),
            Some(NodeStatus::Sprout(SproutStatus::Adopted))
        );
        let events = read_events(&paths).expect("read adoption events");
        assert_eq!(
            events
                .iter()
                .map(|event| event.event_type.as_str())
                .collect::<Vec<_>>(),
            vec!["plant", "adopt"]
        );
        fs::remove_dir_all(root).expect("clean sandbox");
    }
}
