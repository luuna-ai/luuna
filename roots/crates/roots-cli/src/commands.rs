//! Implementations of Roots' non-interactive setup, structure, and read
//! commands.
//!
//! These handlers deliberately mirror the TypeScript command boundary rather
//! than exposing a new Rust-facing UX.  The shared core owns JSONL and path
//! mechanics; this module keeps the command ordering, guards, event payloads,
//! and terminal rendering together so they can be compared directly with the
//! legacy CLI.

use crate::io::Io;
use crate::registry::CommandContext;
use roots_core::actor::{ActorContext, resolve_human_actor, roots_actor};
use roots_core::args::{flag_bool, flag_int, flag_string};
use roots_core::color::{pad_end, visible_length};
use roots_core::config::{Tier, default_config, load_config, render_initial_config};
use roots_core::errors::{RootsError, RootsResult};
use roots_core::events::{EventFields, append_event, events_for, make_event, read_events};
use roots_core::graph::{has_any_edge, is_anchor, update_graph};
use roots_core::graph_rules::{LINK_RELS, check_edge, is_link_rel};
use roots_core::ids::{IdPrefix, generate_id, hex_set};
use roots_core::model::{
    EdgeRecord, EdgeRel, EventRecord, Graph, IDEA_STATUSES, IdeaStatus, NodeKind, NodeRecord,
    NodeStatus, SPROUT_STATUSES,
};
use roots_core::paths::{ROOTS_DIR, RootsPaths, roots_paths};
use roots_core::prose::{canonical_node_dir, locate_node_dir, read_node_prose, scan_node_dirs};
use roots_core::resolve::{ResolveOptions, resolve_node};
use roots_core::serde_json::{Map, Value, to_value};
use roots_core::slug::{is_valid_slug, slugify};
use roots_core::tier::{EffectiveTier, TierSource, effective_tier, parse_tier};
use roots_core::time::{iso_now, short_time};
use roots_core::time_crate::OffsetDateTime;
use roots_core::time_crate::format_description::well_known::Rfc3339;
use roots_core::workspace::open_workspace;
use std::cmp::Ordering;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

pub const GITATTRIBUTES_LINE: &str = ".luuna/roots/*.jsonl merge=union";
pub const ROOTS_GITIGNORE: &str = "*.lock\n*.tmp.*\n";

fn error(error: impl std::fmt::Display) -> RootsError {
    RootsError::error(error.to_string())
}

fn human_actor(io: &dyn Io) -> RootsResult<String> {
    let environment = io.environment();
    resolve_human_actor(ActorContext::new(io.cwd(), &environment))
}

fn human_structure_actor(io: &dyn Io, command: &str) -> RootsResult<String> {
    let environment = io.environment();
    roots_core::guard::require_human_command(ActorContext::new(io.cwd(), &environment), command)
}

fn refuse_agent_session(io: &dyn Io, command: &str) -> RootsResult<()> {
    let environment = io.environment();
    roots_core::guard::refuse_agent_session(&environment, command)
}

fn json<T: roots_core::serde::Serialize>(value: T) -> RootsResult<Value> {
    to_value(value).map_err(error)
}

fn string(value: impl Into<String>) -> Value {
    Value::String(value.into())
}

fn number(value: impl Into<u64>) -> Value {
    Value::Number(value.into().into())
}

fn path_relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .display()
        .to_string()
}

// Node's `path.relative` emits the host's separator.  Keep the stable
// `path: ".luuna/roots"` result field untouched, but preserve native spelling
// for the historical `created` entries returned by `init`.
fn created_path_relative(root: &Path, path: &Path) -> String {
    let relative = path_relative(root, path);
    #[cfg(windows)]
    {
        relative.replace('/', "\\")
    }
    #[cfg(not(windows))]
    {
        relative
    }
}

fn root_basename(paths: &RootsPaths) -> String {
    paths
        .root
        .file_name()
        .and_then(|part| part.to_str())
        .unwrap_or("")
        .to_owned()
}

/// Append the union merge rule to `<root>/.gitattributes`, returning whether
/// this invocation added it.
pub fn ensure_gitattributes(root: &Path) -> RootsResult<bool> {
    let file = root.join(".gitattributes");
    let existing = if file.exists() {
        fs::read_to_string(&file).map_err(error)?
    } else {
        String::new()
    };
    if existing
        .lines()
        .any(|line| line.trim() == GITATTRIBUTES_LINE)
    {
        return Ok(false);
    }
    let separator = if existing.is_empty() || existing.ends_with('\n') {
        ""
    } else {
        "\n"
    };
    fs::write(file, format!("{existing}{separator}{GITATTRIBUTES_LINE}\n")).map_err(error)?;
    Ok(true)
}

fn write_new(path: &Path, content: &str) -> RootsResult<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(error)?;
    file.write_all(content.as_bytes()).map_err(error)
}

fn create_layout(paths: &RootsPaths, project: &str) -> RootsResult<Vec<PathBuf>> {
    let mut created = Vec::new();
    for directory in [&paths.dir, &paths.human, &paths.sprouts, &paths.notes] {
        if !directory.exists() {
            created.push(directory.clone());
        }
        fs::create_dir_all(directory).map_err(error)?;
    }
    let files: Vec<(&Path, String)> = vec![
        (
            &paths.config,
            render_initial_config(&default_config(project)),
        ),
        (&paths.graph, String::new()),
        (&paths.proposals, String::new()),
        (&paths.questions, String::new()),
        (&paths.events, String::new()),
        (&paths.gitignore, ROOTS_GITIGNORE.to_owned()),
    ];
    for (file, contents) in files {
        if file.exists() {
            continue;
        }
        write_new(file, &contents)?;
        created.push(file.to_path_buf());
    }
    Ok(created)
}

/// `luuna-roots init`.
pub fn init_command(context: CommandContext<'_>) -> RootsResult<()> {
    let paths = roots_paths(context.io.cwd());
    if paths.config.exists() {
        let mut detail = Map::new();
        detail.insert("path".to_owned(), string(paths.dir.display().to_string()));
        return Err(
            RootsError::conflict(format!("already initialized: {}", paths.dir.display()))
                .with_detail(detail),
        );
    }
    let basename = root_basename(&paths);
    let project = if basename.is_empty() {
        "project"
    } else {
        &basename
    };
    let created = create_layout(&paths, project)?;
    let gitattributes = ensure_gitattributes(&paths.root)?;

    // Initialization intentionally falls back to Roots bookkeeping when it
    // cannot establish a human identity, rather than refusing setup.
    let by = human_actor(context.io).unwrap_or_else(|_| roots_actor(None));
    let mut event_extra = Map::new();
    event_extra.insert("project".to_owned(), string(basename));
    append_event(
        &paths,
        &make_event(
            "init",
            by,
            EventFields {
                extra: event_extra,
                ..Default::default()
            },
        ),
    )?;

    let mut result = Map::new();
    result.insert("path".to_owned(), string(ROOTS_DIR));
    result.insert("root".to_owned(), string(paths.root.display().to_string()));
    result.insert(
        "created".to_owned(),
        Value::Array(
            created
                .iter()
                .enumerate()
                .map(|(index, path)| {
                    string(if index == 0 {
                        created_path_relative(&paths.root, path)
                    } else {
                        path_relative(&paths.root, path)
                    })
                })
                .collect(),
        ),
    );
    result.insert(
        "gitattributes".to_owned(),
        string(if gitattributes { "added" } else { "present" }),
    );
    context.out.result(context.io, result);
    context.out.success(
        context.io,
        &format!("initialized {ROOTS_DIR}/ in {}", paths.root.display()),
    );
    if gitattributes {
        context.out.info(
            context.io,
            &format!("  added `{GITATTRIBUTES_LINE}` to .gitattributes"),
        );
    }
    context.out.info(
        context.io,
        "  next: luuna-roots plant \"<one sentence of intent>\"",
    );
    Ok(())
}

fn validate_slug(slug: &str) -> RootsResult<()> {
    if is_valid_slug(slug) {
        return Ok(());
    }
    let hint = slugify(slug);
    let suffix = if hint.is_empty() {
        String::new()
    } else {
        format!(" (try \"{hint}\")")
    };
    Err(RootsError::usage(format!(
        "invalid slug \"{slug}\"{suffix}; use kebab-case"
    )))
}

#[derive(Clone, Debug)]
struct MoveResult {
    old_slug: String,
    from_dir: Option<PathBuf>,
    to_dir: PathBuf,
}

/// Rename a node slug under the graph lock, rolling its directory back if the
/// graph write fails after the filesystem rename.
fn move_node(paths: &RootsPaths, id: &str, new_slug: &str) -> RootsResult<MoveResult> {
    let mut renamed: Option<(PathBuf, PathBuf)> = None;
    let outcome = update_graph(paths, |graph| {
        let Some(index) = graph.nodes.iter().position(|node| node.id == id) else {
            return Err(RootsError::not_found(format!("node {id} disappeared")));
        };
        if graph.nodes[index].slug == new_slug {
            return Err(RootsError::usage(format!(
                "{id} is already named \"{new_slug}\""
            )));
        }
        if let Some(clash) = graph.nodes.iter().find(|node| node.slug == new_slug) {
            return Err(RootsError::conflict(format!(
                "slug \"{new_slug}\" is already used by {}",
                clash.id
            )));
        }

        let old_node = graph.nodes[index].clone();
        let dirs = scan_node_dirs(paths)?;
        let from_dir = locate_node_dir(paths, &old_node, Some(&dirs))?;
        let old_slug = old_node.slug.clone();
        graph.nodes[index].slug = new_slug.to_owned();
        graph.nodes[index].updated_at = Some(iso_now());
        let node = graph.nodes[index].clone();
        let to_dir = canonical_node_dir(paths, &node);
        if let Some(from) = &from_dir {
            if *from != to_dir {
                if to_dir.exists() {
                    return Err(RootsError::conflict(format!(
                        "directory already exists: {}",
                        to_dir.display()
                    )));
                }
                fs::rename(from, &to_dir).map_err(error)?;
                renamed = Some((from.clone(), to_dir.clone()));
            }
        }
        Ok((
            true,
            MoveResult {
                old_slug,
                from_dir,
                to_dir,
            },
        ))
    });
    if outcome.is_err() {
        if let Some((from, to)) = renamed {
            let _ = fs::rename(to, from);
        }
    }
    outcome
}

/// `luuna-roots mv <id> <new-slug>`.
pub fn mv_command(context: CommandContext<'_>) -> RootsResult<()> {
    // This legacy command resolves a human author but intentionally does not
    // invoke `refuseAgentSession`; that asymmetry is observable behavior.
    let by = human_actor(context.io)?;
    let query = context.args.first().map(String::as_str).unwrap_or_default();
    let new_slug = context.args.get(1).map(String::as_str).unwrap_or_default();
    validate_slug(new_slug)?;
    let workspace = open_workspace(context.io.cwd())?;
    let target = resolve_node(&workspace.graph.nodes, query, ResolveOptions::default())?.clone();
    let moved = move_node(&workspace.paths, &target.id, new_slug)?;
    let mut extra = Map::new();
    extra.insert("oldSlug".to_owned(), string(moved.old_slug.clone()));
    extra.insert("newSlug".to_owned(), string(new_slug));
    append_event(
        &workspace.paths,
        &make_event(
            "mv",
            by,
            EventFields {
                node: Some(target.id.clone()),
                extra,
                ..Default::default()
            },
        ),
    )?;
    if moved.from_dir.is_none() {
        context.out.warn(
            context.io,
            &format!(
                "no directory found for {}; updated the slug only",
                target.id
            ),
        );
    }
    let mut result = Map::new();
    result.insert("id".to_owned(), string(target.id.clone()));
    result.insert("oldSlug".to_owned(), string(moved.old_slug.clone()));
    result.insert("slug".to_owned(), string(new_slug));
    result.insert(
        "path".to_owned(),
        string(path_relative(&workspace.paths.root, &moved.to_dir)),
    );
    context.out.result(context.io, result);
    context.out.success(
        context.io,
        &format!(
            "renamed {} {} → {}",
            context.out.colors.id(&target.id),
            moved.old_slug,
            new_slug
        ),
    );
    Ok(())
}

fn parse_link_rel(raw: &str) -> RootsResult<EdgeRel> {
    let relation = raw.trim().to_lowercase();
    if relation == "derives" {
        return Err(RootsError::usage(
            "`derives` edges come only from `luuna-roots adopt`",
        ));
    }
    if !is_link_rel(&relation) {
        return Err(RootsError::usage(format!(
            "unknown relation \"{raw}\"; use {}",
            LINK_RELS.join(", ")
        )));
    }
    match relation.as_str() {
        "serves" => Ok(EdgeRel::Serves),
        "tension" => Ok(EdgeRel::Tension),
        "replaces" => Ok(EdgeRel::Replaces),
        _ => unreachable!("is_link_rel accepted a relation absent from EdgeRel"),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct StatusChange {
    from: IdeaStatus,
    to: IdeaStatus,
}

fn compost_in_place(node: &mut NodeRecord) -> Option<StatusChange> {
    let NodeStatus::Idea(status) = node.status else {
        return None;
    };
    if status == IdeaStatus::Composted {
        return None;
    }
    node.status = NodeStatus::Idea(IdeaStatus::Composted);
    node.updated_at = Some(iso_now());
    Some(StatusChange {
        from: status,
        to: IdeaStatus::Composted,
    })
}

#[derive(Clone, Debug)]
struct EdgeAdded {
    edge: EdgeRecord,
    composted: Option<StatusChange>,
}

fn add_edge(
    paths: &RootsPaths,
    from: &str,
    to: &str,
    rel: EdgeRel,
    by: &str,
) -> RootsResult<EdgeAdded> {
    update_graph(paths, |graph| {
        if let Some(message) = check_edge(graph, from, to, rel) {
            let mut detail = Map::new();
            detail.insert("from".to_owned(), string(from));
            detail.insert("to".to_owned(), string(to));
            detail.insert("rel".to_owned(), string(rel.as_str()));
            return Err(RootsError::validation(message).with_detail(detail));
        }
        let ids = hex_set(graph.edges.iter().map(|edge| edge.id.as_str()));
        let edge = EdgeRecord {
            record_type: "edge".to_owned(),
            id: generate_id(IdPrefix::Edge, &ids),
            from: from.to_owned(),
            to: to.to_owned(),
            rel,
            by: by.to_owned(),
            proposed_by: None,
            proposal: None,
            created_at: iso_now(),
        };
        graph.edges.push(edge.clone());
        let composted = if rel == EdgeRel::Replaces {
            graph
                .nodes
                .iter_mut()
                .find(|node| node.id == to)
                .and_then(compost_in_place)
        } else {
            None
        };
        Ok((true, EdgeAdded { edge, composted }))
    })
}

fn status_event(
    by: &str,
    node: &str,
    change: StatusChange,
    mut extra: Map<String, Value>,
    event_type: &str,
) -> EventRecord {
    let mut fields = Map::new();
    fields.insert("from".to_owned(), string(change.from.as_str()));
    fields.insert("to".to_owned(), string(change.to.as_str()));
    fields.append(&mut extra);
    make_event(
        event_type,
        by,
        EventFields {
            node: Some(node.to_owned()),
            extra: fields,
            ..Default::default()
        },
    )
}

fn edge_label(rel: EdgeRel, direction: &str) -> (&'static str, &'static str) {
    match rel {
        EdgeRel::Tension => ("tension", "↔"),
        EdgeRel::Serves if direction == "out" => ("serves", "→"),
        EdgeRel::Replaces if direction == "out" => ("replaces", "→"),
        EdgeRel::Derives if direction == "out" => ("derives", "→"),
        EdgeRel::Serves => ("served by", "←"),
        EdgeRel::Replaces => ("replaced by", "←"),
        EdgeRel::Derives => ("adopted as", "←"),
    }
}

/// `luuna-roots link <a> <b> <relation>`.
pub fn link_command(context: CommandContext<'_>) -> RootsResult<()> {
    let by = human_structure_actor(context.io, "link")?;
    let qa = context.args.first().map(String::as_str).unwrap_or_default();
    let qb = context.args.get(1).map(String::as_str).unwrap_or_default();
    let raw_rel = context.args.get(2).map(String::as_str).unwrap_or_default();
    let rel = parse_link_rel(raw_rel)?;
    let workspace = open_workspace(context.io.cwd())?;
    let a = resolve_node(
        &workspace.graph.nodes,
        qa,
        ResolveOptions {
            kind: Some(NodeKind::Idea),
        },
    )?
    .clone();
    let b = resolve_node(
        &workspace.graph.nodes,
        qb,
        ResolveOptions {
            kind: Some(NodeKind::Idea),
        },
    )?
    .clone();
    let added = add_edge(&workspace.paths, &a.id, &b.id, rel, &by)?;
    let mut link_extra = Map::new();
    link_extra.insert("edge".to_owned(), string(added.edge.id.clone()));
    link_extra.insert("rel".to_owned(), string(rel.as_str()));
    append_event(
        &workspace.paths,
        &make_event(
            "link",
            by.clone(),
            EventFields {
                node: Some(a.id.clone()),
                refs: Some(vec![b.id.clone()]),
                extra: link_extra,
                ..Default::default()
            },
        ),
    )?;
    if let Some(change) = added.composted {
        let mut status_extra = Map::new();
        status_extra.insert("edge".to_owned(), string(added.edge.id.clone()));
        append_event(
            &workspace.paths,
            &status_event(&by, &b.id, change, status_extra, "status"),
        )?;
    }

    let mut result = Map::new();
    result.insert("id".to_owned(), string(added.edge.id.clone()));
    result.insert("edge".to_owned(), json(&added.edge)?);
    let composted = added.composted.map(|change| {
        let mut item = Map::new();
        item.insert("node".to_owned(), string(b.id.clone()));
        item.insert("from".to_owned(), string(change.from.as_str()));
        item.insert("to".to_owned(), string(change.to.as_str()));
        Value::Object(item)
    });
    result.insert("composted".to_owned(), composted.unwrap_or(Value::Null));
    context.out.result(context.io, result);
    let (label, arrow) = edge_label(rel, "out");
    context.out.success(
        context.io,
        &format!(
            "{} {} {arrow} {label} {arrow} {}",
            context.out.colors.id(&added.edge.id),
            a.slug,
            b.slug
        ),
    );
    if let Some(change) = added.composted {
        context.out.info(
            context.io,
            &format!("  {} {} {} → composted", b.id, b.slug, change.from.as_str()),
        );
    }
    Ok(())
}

fn remove_edge(paths: &RootsPaths, raw: &str) -> RootsResult<EdgeRecord> {
    let id = raw.trim().to_lowercase();
    update_graph(paths, |graph| {
        let Some(index) = graph
            .edges
            .iter()
            .position(|edge| edge.id == id || edge.id == format!("e-{id}"))
        else {
            return Err(RootsError::not_found(format!(
                "no edge {raw}; `luuna-roots show <id>` lists edge ids"
            )));
        };
        let edge = graph.edges[index].clone();
        if edge.rel == EdgeRel::Derives {
            return Err(RootsError::usage(format!(
                "{} is a `derives` edge (adoption lineage); it stays",
                edge.id
            )));
        }
        graph.edges.remove(index);
        Ok((true, edge))
    })
}

/// `luuna-roots unlink <edge-id>`.
pub fn unlink_command(context: CommandContext<'_>) -> RootsResult<()> {
    let by = human_structure_actor(context.io, "unlink")?;
    let workspace = open_workspace(context.io.cwd())?;
    let edge = remove_edge(
        &workspace.paths,
        context.args.first().map(String::as_str).unwrap_or_default(),
    )?;
    let mut extra = Map::new();
    extra.insert("edge".to_owned(), string(edge.id.clone()));
    extra.insert("rel".to_owned(), string(edge.rel.as_str()));
    if let Some(proposal) = &edge.proposal {
        extra.insert("proposal".to_owned(), string(proposal.clone()));
    }
    append_event(
        &workspace.paths,
        &make_event(
            "unlink",
            by,
            EventFields {
                node: Some(edge.from.clone()),
                refs: Some(vec![edge.to.clone()]),
                extra,
                ..Default::default()
            },
        ),
    )?;
    let mut result = Map::new();
    result.insert("id".to_owned(), string(edge.id.clone()));
    result.insert("edge".to_owned(), json(&edge)?);
    context.out.result(context.io, result);
    context.out.success(
        context.io,
        &format!(
            "removed {} {} {} {}",
            context.out.colors.id(&edge.id),
            edge.from,
            edge.rel.as_str(),
            edge.to
        ),
    );
    Ok(())
}

fn parse_idea_status(raw: &str) -> RootsResult<IdeaStatus> {
    let status = raw.trim().to_lowercase();
    match status.as_str() {
        "planted" => Ok(IdeaStatus::Planted),
        "shaping" => Ok(IdeaStatus::Shaping),
        "committed" => Ok(IdeaStatus::Committed),
        "built" => Ok(IdeaStatus::Built),
        "composted" => Ok(IdeaStatus::Composted),
        _ => Err(RootsError::usage(format!(
            "unknown status \"{raw}\"; statuses are {}",
            IDEA_STATUSES.join(", ")
        ))),
    }
}

fn transition_error(node: &NodeRecord, to: IdeaStatus) -> Option<String> {
    let NodeStatus::Idea(from) = node.status else {
        return Some(format!(
            "{} is a sprout; sprouts are adopted or rejected",
            node.id
        ));
    };
    let allowed: &[IdeaStatus] = match to {
        IdeaStatus::Committed => &[IdeaStatus::Shaping],
        IdeaStatus::Built => &[IdeaStatus::Committed],
        IdeaStatus::Composted => &[
            IdeaStatus::Planted,
            IdeaStatus::Shaping,
            IdeaStatus::Committed,
            IdeaStatus::Built,
        ],
        IdeaStatus::Shaping => {
            return Some(
                "`shaping` is set by the first `luuna-roots think` session, not by hand".to_owned(),
            );
        }
        IdeaStatus::Planted => return Some("`planted` cannot be set by hand".to_owned()),
    };
    if allowed.contains(&from) {
        return None;
    }
    if from == IdeaStatus::Composted {
        return Some(format!(
            "{} is composted; composted ideas stay composted",
            node.id
        ));
    }
    if to == IdeaStatus::Committed && from == IdeaStatus::Planted {
        return Some(format!(
            "{} is only planted; shape it with `luuna-roots think {}` before committing",
            node.id, node.id
        ));
    }
    Some(format!(
        "{} is {}; it can move to {} only from {}",
        node.id,
        from.as_str(),
        to.as_str(),
        allowed
            .iter()
            .map(|status| status.as_str())
            .collect::<Vec<_>>()
            .join(" or ")
    ))
}

fn set_idea_status(
    paths: &RootsPaths,
    id: &str,
    to: IdeaStatus,
) -> RootsResult<Option<StatusChange>> {
    update_graph(paths, |graph| {
        let Some(node) = graph.nodes.iter_mut().find(|node| node.id == id) else {
            return Err(RootsError::conflict(format!("node {id} disappeared")));
        };
        if node.status.as_str() == to.as_str() {
            return Ok((false, None));
        }
        if let Some(message) = transition_error(node, to) {
            let mut detail = Map::new();
            detail.insert("from".to_owned(), string(node.status.as_str()));
            detail.insert("to".to_owned(), string(to.as_str()));
            return Err(RootsError::conflict(message).with_detail(detail));
        }
        let NodeStatus::Idea(from) = node.status else {
            unreachable!("transition_error accepted a sprout")
        };
        node.status = NodeStatus::Idea(to);
        node.updated_at = Some(iso_now());
        Ok((true, Some(StatusChange { from, to })))
    })
}

fn seeds_hint(
    paths: &RootsPaths,
    config: &roots_core::config::RootsConfig,
    node: &NodeRecord,
) -> Option<String> {
    if !paths.root.join(".luuna/missions").exists() {
        return None;
    }
    if effective_tier(config, Some(node)).tier < 2 {
        return None;
    }
    Some(format!(
        "Create seeds issues? Link each one to this idea with {} in its description (e.g. sd create --title \"...\" --description \"Realizes {}\"); an agent can draft them from `luuna-roots context {}`.",
        node.id, node.id, node.id
    ))
}

fn format_status(colors: roots_core::color::Colors, status: NodeStatus) -> String {
    match status {
        NodeStatus::Idea(IdeaStatus::Planted) => colors.green("planted"),
        NodeStatus::Idea(IdeaStatus::Shaping) => colors.cyan("shaping"),
        NodeStatus::Idea(IdeaStatus::Committed) => colors.magenta("committed"),
        NodeStatus::Idea(IdeaStatus::Built) => colors.blue("built"),
        NodeStatus::Sprout(roots_core::model::SproutStatus::Open) => colors.yellow("open"),
        status => colors.dim(status.as_str()),
    }
}

fn change_lifecycle(
    context: CommandContext<'_>,
    command: &str,
    to: IdeaStatus,
    reason: Option<&str>,
) -> RootsResult<()> {
    let by = human_structure_actor(context.io, command)?;
    let workspace = open_workspace(context.io.cwd())?;
    let node = resolve_node(
        &workspace.graph.nodes,
        context.args.first().map(String::as_str).unwrap_or_default(),
        ResolveOptions {
            kind: Some(NodeKind::Idea),
        },
    )?
    .clone();
    let done = set_idea_status(&workspace.paths, &node.id, to)?;
    if let Some(change) = done {
        let mut extra = Map::new();
        if let Some(reason) = reason {
            extra.insert("reason".to_owned(), string(reason));
        }
        append_event(
            &workspace.paths,
            &status_event(
                &by,
                &node.id,
                change,
                extra,
                if to == IdeaStatus::Composted {
                    "compost"
                } else {
                    "status"
                },
            ),
        )?;
    }
    let hint = if done.is_some() && to == IdeaStatus::Committed {
        let config = load_config(&workspace.paths)?;
        seeds_hint(&workspace.paths, &config, &node)
    } else {
        None
    };
    let mut result = Map::new();
    result.insert("id".to_owned(), string(node.id.clone()));
    result.insert("changed".to_owned(), Value::Bool(done.is_some()));
    result.insert(
        "from".to_owned(),
        string(done.map_or(to.as_str(), |change| change.from.as_str())),
    );
    result.insert("to".to_owned(), string(to.as_str()));
    if to == IdeaStatus::Committed {
        result.insert(
            "seedsHint".to_owned(),
            hint.clone().map_or(Value::Null, Value::String),
        );
    }
    context.out.result(context.io, result);
    let reference = format!("{} {}", context.out.colors.id(&node.id), node.slug);
    if let Some(change) = done {
        context.out.success(
            context.io,
            &format!(
                "{reference} {} → {}",
                change.from.as_str(),
                format_status(context.out.colors, NodeStatus::Idea(to))
            ),
        );
        if let Some(hint) = hint {
            context.out.info(context.io, &context.out.colors.dim(&hint));
        }
    } else {
        context.out.info(
            context.io,
            &format!(
                "{reference} is already {}",
                format_status(context.out.colors, NodeStatus::Idea(to))
            ),
        );
    }
    Ok(())
}

/// `luuna-roots commit <id>`.
pub fn commit_command(context: CommandContext<'_>) -> RootsResult<()> {
    change_lifecycle(context, "commit", IdeaStatus::Committed, None)
}

/// `luuna-roots status <id> <status>`.
pub fn status_command(context: CommandContext<'_>) -> RootsResult<()> {
    let status = parse_idea_status(context.args.get(1).map(String::as_str).unwrap_or_default())?;
    change_lifecycle(context, "status", status, None)
}

/// `luuna-roots compost <id> [--reason <text>]`.
pub fn compost_command(context: CommandContext<'_>) -> RootsResult<()> {
    let reason = flag_string(context.flags, "reason");
    change_lifecycle(context, "compost", IdeaStatus::Composted, reason)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct TierChange {
    from: Option<Tier>,
    to: Option<Tier>,
}

fn set_tier_override(
    paths: &RootsPaths,
    id: &str,
    tier: Option<Tier>,
) -> RootsResult<Option<TierChange>> {
    update_graph(paths, |graph| {
        let Some(node) = graph.nodes.iter_mut().find(|node| node.id == id) else {
            return Err(RootsError::not_found(format!("node {id} disappeared")));
        };
        let from = node.tier.filter(|value| *value <= 3);
        if from == tier {
            return Ok((false, None));
        }
        node.tier = tier;
        node.updated_at = Some(iso_now());
        Ok((true, Some(TierChange { from, to: tier })))
    })
}

fn tier_source_name(source: TierSource) -> &'static str {
    match source {
        TierSource::Idea => "idea",
        TierSource::Config => "config",
    }
}

fn tier_fields(result: &mut Map<String, Value>, effective: EffectiveTier) {
    result.insert("tier".to_owned(), number(effective.tier));
    result.insert("name".to_owned(), string(effective.name));
    result.insert(
        "source".to_owned(),
        string(tier_source_name(effective.source)),
    );
}

/// `luuna-roots tier <id> [<0-3>|default]`.
pub fn tier_command(context: CommandContext<'_>) -> RootsResult<()> {
    let query = context.args.first().map(String::as_str).unwrap_or_default();
    let raw = context.args.get(1).map(String::as_str);
    let workspace = open_workspace(context.io.cwd())?;
    let node = resolve_node(
        &workspace.graph.nodes,
        query,
        ResolveOptions {
            kind: Some(NodeKind::Idea),
        },
    )?
    .clone();
    let config = load_config(&workspace.paths)?;
    if raw.is_none() {
        let effective = effective_tier(&config, Some(&node));
        let mut result = Map::new();
        result.insert("id".to_owned(), string(node.id.clone()));
        tier_fields(&mut result, effective);
        result.insert("config".to_owned(), number(config.tier));
        context.out.result(context.io, result);
        let via = if effective.source == TierSource::Idea {
            "per-idea override"
        } else {
            "from config.yaml"
        };
        context.out.line(
            context.io,
            &format!(
                "{} {}  tier {} ({}, {via})",
                context.out.colors.id(&node.id),
                node.slug,
                effective.tier,
                effective.name
            ),
        );
        return Ok(());
    }
    refuse_agent_session(context.io, "tier")?;
    let by = human_actor(context.io)?;
    let raw = raw.expect("checked above");
    let tier = if raw.trim().eq_ignore_ascii_case("default") {
        None
    } else {
        Some(parse_tier(raw)?)
    };
    let change = set_tier_override(&workspace.paths, &node.id, tier)?;
    if let Some(change) = change {
        let mut extra = Map::new();
        extra.insert("from".to_owned(), change.from.map_or(Value::Null, number));
        extra.insert("to".to_owned(), change.to.map_or(Value::Null, number));
        append_event(
            &workspace.paths,
            &make_event(
                "tier",
                by,
                EventFields {
                    node: Some(node.id.clone()),
                    extra,
                    ..Default::default()
                },
            ),
        )?;
    }
    let effective = if let Some(tier) = tier {
        EffectiveTier {
            tier,
            name: roots_core::tier::tier_name(tier),
            source: TierSource::Idea,
        }
    } else {
        effective_tier(&config, None)
    };
    let mut result = Map::new();
    result.insert("id".to_owned(), string(node.id.clone()));
    result.insert("changed".to_owned(), Value::Bool(change.is_some()));
    result.insert("override".to_owned(), tier.map_or(Value::Null, number));
    tier_fields(&mut result, effective);
    context.out.result(context.io, result);
    let label = format!(
        "tier {} ({}{})",
        effective.tier,
        effective.name,
        if tier.is_none() {
            ", from config.yaml"
        } else {
            ""
        }
    );
    if change.is_none() {
        context.out.info(
            context.io,
            &format!(
                "{} {} is already at {label}",
                context.out.colors.id(&node.id),
                node.slug
            ),
        );
    } else {
        context.out.success(
            context.io,
            &format!(
                "{} {} → {label}",
                context.out.colors.id(&node.id),
                node.slug
            ),
        );
    }
    Ok(())
}

#[derive(Clone, Debug)]
struct ListFilter {
    status: Option<String>,
    kind: Option<NodeKind>,
    orphans: bool,
    anchors: bool,
    all: bool,
}

fn parse_list_filter(context: &CommandContext<'_>) -> RootsResult<ListFilter> {
    let status = flag_string(context.flags, "status").map(ToOwned::to_owned);
    if let Some(status) = &status {
        if !IDEA_STATUSES.contains(&status.as_str()) && !SPROUT_STATUSES.contains(&status.as_str())
        {
            return Err(RootsError::usage(format!(
                "invalid --status \"{status}\" (valid: {}|{})",
                IDEA_STATUSES.join("|"),
                SPROUT_STATUSES.join("|")
            )));
        }
    }
    let kind = match flag_string(context.flags, "kind") {
        None => None,
        Some("idea") => Some(NodeKind::Idea),
        Some("sprout") => Some(NodeKind::Sprout),
        Some(raw) => {
            return Err(RootsError::usage(format!(
                "invalid --kind \"{raw}\" (valid: idea|sprout)"
            )));
        }
    };
    Ok(ListFilter {
        status,
        kind,
        orphans: flag_bool(context.flags, "orphans"),
        anchors: flag_bool(context.flags, "anchors"),
        all: flag_bool(context.flags, "all"),
    })
}

fn expires_at_or_before_now(value: &str, now: OffsetDateTime) -> bool {
    OffsetDateTime::parse(value, &Rfc3339).is_ok_and(|when| when <= now)
}

fn sprout_view(node: &NodeRecord, now: OffsetDateTime) -> NodeRecord {
    let mut viewed = node.clone();
    if viewed.kind == NodeKind::Sprout
        && viewed.status.as_str() == "open"
        && viewed
            .expires_at
            .as_deref()
            .is_some_and(|expiry| expires_at_or_before_now(expiry, now))
    {
        viewed.status = NodeStatus::Sprout(roots_core::model::SproutStatus::Expired);
    }
    viewed
}

fn retired(node: &NodeRecord) -> bool {
    match node.kind {
        NodeKind::Idea => node.status.as_str() == "composted",
        NodeKind::Sprout => node.status.as_str() != "open",
    }
}

fn filter_nodes(graph: &Graph, filter: &ListFilter) -> Vec<NodeRecord> {
    let mut rows: Vec<NodeRecord> = graph
        .nodes
        .iter()
        .filter(|node| {
            filter
                .status
                .as_ref()
                .is_none_or(|status| node.status.as_str() == status)
        })
        .filter(|node| filter.status.is_some() || filter.all || !retired(node))
        .filter(|node| filter.kind.is_none_or(|kind| node.kind == kind))
        .filter(|node| !filter.orphans || !has_any_edge(graph, &node.id))
        .filter(|node| !filter.anchors || is_anchor(graph, node))
        .cloned()
        .collect();
    rows.sort_by(|left, right| match (left.kind, right.kind) {
        (NodeKind::Idea, NodeKind::Sprout) => Ordering::Less,
        (NodeKind::Sprout, NodeKind::Idea) => Ordering::Greater,
        _ => left.created_at.cmp(&right.created_at),
    });
    rows
}

fn claim_statement(
    paths: &RootsPaths,
    graph: &Graph,
    node: &NodeRecord,
    dirs: &roots_core::prose::NodeDirs,
) -> RootsResult<String> {
    let own = read_node_prose(paths, node, Some(dirs))?.statement;
    let Some(endorsement) = &node.endorses else {
        return Ok(own);
    };
    let Some(sprout) = graph
        .nodes
        .iter()
        .find(|candidate| candidate.id == endorsement.sprout)
    else {
        return Ok(own);
    };
    let endorsed = read_node_prose(paths, sprout, Some(dirs))?.statement;
    Ok(if endorsed.is_empty() { own } else { endorsed })
}

fn format_node_ref(colors: roots_core::color::Colors, node: &NodeRecord) -> String {
    let reference = format!("{} {}", colors.id(&node.id), node.slug);
    if node.kind == NodeKind::Sprout {
        format!("{} {}", colors.dim("[agent]"), colors.dim(&reference))
    } else {
        reference
    }
}

fn truncate(value: &str, max: usize) -> String {
    let chars: Vec<char> = value.chars().collect();
    if chars.len() <= max {
        value.to_owned()
    } else {
        format!(
            "{}…",
            chars[..max.saturating_sub(1)].iter().collect::<String>()
        )
    }
}

fn days_left(expiry: &str, now: OffsetDateTime) -> Option<i64> {
    let expiry = OffsetDateTime::parse(expiry, &Rfc3339).ok()?;
    let nanos = (expiry - now).whole_nanoseconds();
    const DAY_NANOS: i128 = 86_400_000_000_000;
    let days = if nanos >= 0 {
        (nanos + DAY_NANOS - 1) / DAY_NANOS
    } else {
        nanos / DAY_NANOS
    };
    Some(std::cmp::max(0, days) as i64)
}

fn expiry_suffix(
    colors: roots_core::color::Colors,
    node: &NodeRecord,
    now: OffsetDateTime,
) -> String {
    if node.kind != NodeKind::Sprout || node.status.as_str() != "open" {
        return String::new();
    }
    node.expires_at
        .as_deref()
        .and_then(|expiry| days_left(expiry, now))
        .map(|days| colors.dim(&format!("  (expires in {days}d)")))
        .unwrap_or_default()
}

fn render_list_rows(
    context: &CommandContext<'_>,
    rows: &[(NodeRecord, String)],
    now: OffsetDateTime,
) -> Vec<String> {
    let references: Vec<String> = rows
        .iter()
        .map(|(node, _)| format_node_ref(context.out.colors, node))
        .collect();
    let width = references
        .iter()
        .map(|reference| visible_length(reference))
        .max()
        .unwrap_or(0)
        + 2;
    rows.iter()
        .zip(references)
        .map(|((node, statement), reference)| {
            let status = pad_end(&format_status(context.out.colors, node.status), 11);
            let text = truncate(
                if statement.is_empty() {
                    "(no statement)"
                } else {
                    statement
                },
                72,
            );
            let statement = if node.kind == NodeKind::Sprout {
                context.out.colors.dim(&text)
            } else {
                text
            };
            format!(
                "{}{}{}{}",
                pad_end(&reference, width),
                status,
                statement,
                expiry_suffix(context.out.colors, node, now)
            )
        })
        .collect()
}

/// `luuna-roots list`.
pub fn list_command(context: CommandContext<'_>) -> RootsResult<()> {
    let filter = parse_list_filter(&context)?;
    let workspace = open_workspace(context.io.cwd())?;
    let now = OffsetDateTime::now_utc();
    let graph = Graph {
        nodes: workspace
            .graph
            .nodes
            .iter()
            .map(|node| sprout_view(node, now))
            .collect(),
        edges: workspace.graph.edges.clone(),
    };
    let nodes = filter_nodes(&graph, &filter);
    let rows = nodes
        .into_iter()
        .map(|node| {
            let statement =
                claim_statement(&workspace.paths, &workspace.graph, &node, &workspace.dirs)?;
            Ok((node, statement))
        })
        .collect::<RootsResult<Vec<_>>>()?;
    let mut result = Map::new();
    result.insert("count".to_owned(), number(rows.len() as u64));
    let mut output_nodes = Vec::new();
    for (node, statement) in &rows {
        let mut value = match json(node)? {
            Value::Object(value) => value,
            _ => unreachable!("node record serializes as an object"),
        };
        value.insert("statement".to_owned(), string(statement.clone()));
        output_nodes.push(Value::Object(value));
    }
    result.insert("nodes".to_owned(), Value::Array(output_nodes));
    context.out.result(context.io, result);
    if rows.is_empty() {
        context.out.line(
            context.io,
            if workspace.graph.nodes.is_empty() {
                "No ideas yet. Plant one: luuna-roots plant \"<one sentence>\""
            } else {
                "No matches."
            },
        );
        return Ok(());
    }
    let rendered = render_list_rows(&context, &rows, now);
    context.out.lines(context.io, &rendered);
    Ok(())
}

fn describe_extra(event: &EventRecord) -> String {
    if event.event_type == "mv"
        && let Some(Value::String(old_slug)) = event.extra.get("oldSlug")
    {
        let new_slug = event
            .extra
            .get("newSlug")
            .map(js_string)
            .unwrap_or_else(|| "undefined".to_owned());
        return format!("{old_slug} → {new_slug}");
    }
    event
        .extra
        .iter()
        .filter(|(key, value)| {
            key.as_str() != "hash" && matches!(value, Value::String(_) | Value::Number(_))
        })
        .map(|(key, value)| format!("{key}={}", js_string(value)))
        .collect::<Vec<_>>()
        .join(" ")
}

fn js_string(value: &Value) -> String {
    match value {
        Value::String(value) => value.clone(),
        Value::Number(value) => value.to_string(),
        Value::Bool(value) => value.to_string(),
        Value::Null => "null".to_owned(),
        Value::Array(_) => "".to_owned(),
        Value::Object(_) => "[object Object]".to_owned(),
    }
}

fn render_event(context: &CommandContext<'_>, graph: &Graph, event: &EventRecord) -> String {
    let subject = event.node.as_ref().map_or_else(String::new, |id| {
        graph.nodes.iter().find(|node| node.id == *id).map_or_else(
            || context.out.colors.id(id),
            |node| format!("{} {}", context.out.colors.id(id), node.slug),
        )
    });
    let extra = describe_extra(event);
    let mut pieces = vec![
        context.out.colors.dim(&short_time(&event.at)),
        pad_end(&context.out.colors.bold(&event.event_type), 14),
        pad_end(&subject, 26),
        context.out.colors.cyan(&event.by),
    ];
    if !extra.is_empty() {
        pieces.push(context.out.colors.dim(&extra));
    } else {
        pieces.push(String::new());
    }
    pieces.join("  ").trim_end().to_owned()
}

/// `luuna-roots log [<id>] [--limit <n>]`.
pub fn log_command(context: CommandContext<'_>) -> RootsResult<()> {
    let limit = flag_int(context.flags, "limit")?;
    let workspace = open_workspace(context.io.cwd())?;
    let mut events = read_events(&workspace.paths)?;
    let node = match context.args.first() {
        Some(query) => {
            Some(resolve_node(&workspace.graph.nodes, query, ResolveOptions::default())?.clone())
        }
        None => None,
    };
    if let Some(node) = &node {
        events = events_for(&events, &node.id).into_iter().cloned().collect();
    }
    if let Some(limit) = limit {
        events = if limit == 0 {
            Vec::new()
        } else {
            let count = usize::try_from(limit).unwrap_or(usize::MAX);
            let start = events.len().saturating_sub(count);
            events[start..].to_vec()
        };
    }
    let mut result = Map::new();
    result.insert(
        "node".to_owned(),
        node.map_or(Value::Null, |node| string(node.id)),
    );
    result.insert("count".to_owned(), number(events.len() as u64));
    result.insert("events".to_owned(), json(&events)?);
    context.out.result(context.io, result);
    if events.is_empty() {
        context.out.line(context.io, "No events.");
        return Ok(());
    }
    let rendered: Vec<String> = events
        .iter()
        .map(|event| render_event(&context, &workspace.graph, event))
        .collect();
    context.out.lines(context.io, &rendered);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::MemoryIo;
    use crate::run_cli;
    use roots_core::events::read_events;
    use roots_core::graph::read_graph;
    use roots_core::serde_json::Value;
    use std::collections::BTreeMap;

    fn sandbox(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "roots-cli-commands-{name}-{}",
            roots_core::ids::random_hex(8)
        ));
        fs::create_dir_all(&path).expect("create test root");
        path
    }

    fn io(root: PathBuf) -> MemoryIo {
        let mut io = MemoryIo::new(root);
        io.env = BTreeMap::from([
            ("ROOTS_USER".to_owned(), "Test Human".to_owned()),
            ("NO_COLOR".to_owned(), "1".to_owned()),
        ]);
        io
    }

    fn run(root: &Path, parts: &[&str]) -> (i32, MemoryIo) {
        let mut state = io(root.to_path_buf());
        let argv = parts
            .iter()
            .map(|part| (*part).to_owned())
            .collect::<Vec<_>>();
        let exit = run_cli(&argv, &mut state);
        (exit, state)
    }

    fn init(root: &Path) {
        let (exit, output) = run(root, &["init"]);
        assert_eq!(exit, 0, "{}", output.stderr);
    }

    fn plant_graph_node(root: &Path, id: &str, slug: &str, statement: &str) {
        let paths = roots_paths(root);
        let node = NodeRecord::new(
            id.to_owned(),
            NodeKind::Idea,
            slug.to_owned(),
            NodeStatus::Idea(IdeaStatus::Planted),
            "human:test-human".to_owned(),
            "2026-10-22T00:00:00Z".to_owned(),
        );
        let directory = canonical_node_dir(&paths, &node);
        fs::create_dir_all(&directory).expect("node directory");
        fs::write(directory.join("idea.md"), format!("{statement}\n")).expect("prose");
        update_graph(&paths, |graph| {
            graph.nodes.push(node);
            Ok((true, ()))
        })
        .expect("write graph");
    }

    #[test]
    fn init_created_directory_uses_the_host_relative_separator() {
        let root = sandbox("created-relative");
        let (exit, output) = run(&root, &["init", "--json"]);
        assert_eq!(exit, 0, "{}", output.stderr);
        let body: Value = roots_core::serde_json::from_str(&output.stdout).expect("init JSON");
        let created = body["created"][0].as_str().expect("created directory");
        #[cfg(windows)]
        assert_eq!(created, ".luuna\\roots");
        #[cfg(not(windows))]
        assert_eq!(created, ".luuna/roots");
        assert_eq!(body["path"], ROOTS_DIR);
        fs::remove_dir_all(root).expect("clean test root");
    }

    #[test]
    fn init_mv_link_and_log_keep_the_observable_store_contract() {
        let root = sandbox("flow");
        init(&root);
        let paths = roots_paths(&root);
        assert!(paths.config.exists());
        assert_eq!(read_events(&paths).expect("events").len(), 1);
        plant_graph_node(&root, "r-a1b2", "alpha", "Alpha");
        plant_graph_node(&root, "r-b2c3", "beta", "Beta");

        let (exit, moved) = run(&root, &["mv", "alpha", "alpha-2", "--json"]);
        assert_eq!(exit, 0, "{}", moved.stderr);
        assert!(moved.stdout.contains("\"oldSlug\": \"alpha\""));
        assert_eq!(read_graph(&paths).expect("graph").nodes[0].slug, "alpha-2");

        let (exit, linked) = run(&root, &["link", "alpha-2", "beta", "serves", "--json"]);
        assert_eq!(exit, 0, "{}", linked.stderr);
        assert!(linked.stdout.contains("\"rel\": \"serves\""));
        let edge = read_graph(&paths).expect("graph").edges[0].id.clone();

        let (exit, unlinked) = run(&root, &["unlink", &edge, "--json"]);
        assert_eq!(exit, 0, "{}", unlinked.stderr);
        assert!(unlinked.stdout.contains(&edge));

        let (exit, log) = run(&root, &["log", "--limit", "1", "--json"]);
        assert_eq!(exit, 0, "{}", log.stderr);
        assert!(log.stdout.contains("\"count\": 1"));
        fs::remove_dir_all(root).expect("clean test root");
    }

    #[test]
    fn lifecycle_tier_and_list_preserve_human_guards_and_filters() {
        let root = sandbox("lifecycle");
        init(&root);
        let paths = roots_paths(&root);
        plant_graph_node(&root, "r-a1b2", "alpha", "Alpha");
        let (exit, early) = run(&root, &["commit", "r-a1b2", "--json"]);
        assert_eq!(exit, roots_core::errors::EXIT_CONFLICT);
        assert!(early.stdout.contains("only planted"));

        update_graph(&paths, |graph| {
            graph.nodes[0].status = NodeStatus::Idea(IdeaStatus::Shaping);
            Ok((true, ()))
        })
        .expect("shape node");
        let (exit, committed) = run(&root, &["commit", "r-a1b2", "--json"]);
        assert_eq!(exit, 0, "{}", committed.stderr);
        assert!(committed.stdout.contains("\"to\": \"committed\""));

        let (exit, tier) = run(&root, &["tier", "r-a1b2", "0", "--json"]);
        assert_eq!(exit, 0, "{}", tier.stderr);
        assert!(tier.stdout.contains("\"tier\": 0"));
        let (exit, listed) = run(&root, &["list", "--status", "committed", "--json"]);
        assert_eq!(exit, 0, "{}", listed.stderr);
        assert!(listed.stdout.contains("\"alpha\""));
        fs::remove_dir_all(root).expect("clean test root");
    }
}
