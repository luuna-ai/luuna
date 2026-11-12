//! Human capture commands: `plant` and `adopt`.
//!
//! The persistent mutation stays deliberately close to the TypeScript command
//! contract.  Human prose is written only after the TTY and actor gates pass;
//! sprout prose is only ever read as a quoted reference during adoption.

use crate::editor::{
    Spawner, SplitCommand, SplitLaunch, launch_in_split, open_editor, reference_buffer,
    resolve_pager, strip_reference,
};
use crate::io::Io;
use crate::registry::CommandContext;
use roots_core::actor::{ActorContext, resolve_human_actor};
use roots_core::args::{flag_bool, flag_string};
use roots_core::attention::{collect_attention, findings_ready, rank_attention};
use roots_core::config::load_config;
use roots_core::errors::{RootsError, RootsResult};
use roots_core::events::{EventFields, append_event, make_event};
use roots_core::graph::{find_node, read_graph, update_graph};
use roots_core::ids::{IdPrefix, generate_id, hex_set, random_hex};
use roots_core::model::{
    EdgeRecord, EdgeRel, Endorsement, Graph, IdeaStatus, NodeKind, NodeRecord, NodeStatus,
    SproutStatus,
};
use roots_core::paths::{RootsPaths, prose_file_name};
use roots_core::proposals::{pending_proposals, read_proposals};
use roots_core::prose::{
    canonical_node_dir, content_hash, parse_prose, read_node_prose, scan_node_dirs,
};
use roots_core::resolve::{ResolveOptions, resolve_node};
use roots_core::serde_json::{Map, Value, to_value};
use roots_core::slug::{is_valid_slug, slugify, unique_slug};
use roots_core::sprouts::{expire_sprouts, is_sprout_overdue, open_sprouts};
use roots_core::time::iso_at;
use roots_core::time_crate::OffsetDateTime;
use roots_core::workspace::open_workspace;
use std::collections::BTreeSet;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

fn error(error: impl std::fmt::Display) -> RootsError {
    RootsError::error(error.to_string())
}

fn human_actor(io: &dyn Io) -> RootsResult<String> {
    let environment = io.environment();
    resolve_human_actor(ActorContext::new(io.cwd(), &environment))
}

fn human_command_actor(io: &dyn Io, command: &str) -> RootsResult<String> {
    let environment = io.environment();
    roots_core::guard::require_human_command(ActorContext::new(io.cwd(), &environment), command)
}

fn relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .display()
        .to_string()
}

fn json<T: roots_core::serde::Serialize>(value: T) -> RootsResult<Value> {
    to_value(value).map_err(error)
}

fn plural(count: usize, word: &str) -> String {
    format!("{count} {word}{}", if count == 1 { "" } else { "s" })
}

/// The single recommended follow-up that the legacy CLI prints after a human
/// capture command.  It intentionally never turns a successful mutation into
/// an error if queue inspection has an unrelated read failure.
pub(crate) fn print_next(context: &mut CommandContext<'_>, paths: &RootsPaths) {
    if context.out.json || context.out.quiet {
        return;
    }
    let next = (|| -> RootsResult<(String, String)> {
        let now = OffsetDateTime::now_utc();
        let graph = read_graph(paths)?;
        let proposal_rows = read_proposals(paths)?;
        let proposals = pending_proposals(&proposal_rows, now, Some(&graph));
        let sprouts = open_sprouts(&graph, now);
        if !proposals.is_empty() || !sprouts.is_empty() {
            let mut parts = Vec::new();
            if !sprouts.is_empty() {
                parts.push(plural(sprouts.len(), "sprout"));
            }
            if !proposals.is_empty() {
                parts.push(plural(proposals.len(), "proposal"));
            }
            return Ok((
                "luuna-roots tend".to_owned(),
                format!("{} waiting for review", parts.join(", ")),
            ));
        }
        let config = load_config(paths)?;
        let ranked = rank_attention(collect_attention(paths, &graph, &config, now)?);
        if let Some(pick) = ranked.first() {
            let open = pick.due.len();
            let reason = if findings_ready(pick) > 0 {
                "agent findings ready".to_owned()
            } else if open > 0 {
                plural(open, "open question")
            } else {
                plural(pick.candidates.len(), "new question")
            };
            return Ok((
                format!("luuna-roots think {}", pick.node.id),
                format!("{}: {reason}", pick.node.slug),
            ));
        }
        Ok((
            "luuna-roots plant \"<one sentence>\"".to_owned(),
            "nothing needs thinking".to_owned(),
        ))
    })();
    if let Ok((command, reason)) = next {
        context.out.info(
            context.io,
            &context
                .out
                .colors
                .dim(&format!("next: {command} — {reason}")),
        );
    }
}

fn make_temp_editor_dir() -> RootsResult<PathBuf> {
    for _ in 0..20 {
        let directory = std::env::temp_dir().join(format!("roots-plant-{}", random_hex(12)));
        match fs::create_dir(&directory) {
            Ok(()) => return Ok(directory),
            Err(problem) if problem.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(problem) => return Err(error(problem)),
        }
    }
    Err(RootsError::error(
        "could not create a temporary directory for the editor",
    ))
}

/// Open a temporary `idea.md` with the requested initial buffer and return the
/// human part after stripping any reference below the scissors line.
pub fn compose_in_editor(io: &dyn Io, initial: &str) -> RootsResult<String> {
    let directory = make_temp_editor_dir()?;
    let file = directory.join("idea.md");
    let result = (|| {
        fs::write(&file, initial).map_err(error)?;
        let environment = io.environment();
        open_editor(&file, io.cwd(), &environment)?;
        let text = fs::read_to_string(&file).map_err(error)?;
        Ok(strip_reference(&text))
    })();
    let _ = fs::remove_dir_all(directory);
    result
}

fn choose_slug(graph: &Graph, statement: &str, requested: Option<&str>) -> RootsResult<String> {
    let taken: BTreeSet<String> = graph.nodes.iter().map(|node| node.slug.clone()).collect();
    if let Some(requested) = requested {
        if !is_valid_slug(requested) {
            let suggestion = {
                let slug = slugify(requested);
                if slug.is_empty() {
                    "my-idea".to_owned()
                } else {
                    slug
                }
            };
            return Err(RootsError::usage(format!(
                "invalid slug \"{requested}\" (try \"{suggestion}\")"
            )));
        }
        if taken.contains(requested) {
            return Err(RootsError::conflict(format!(
                "slug \"{requested}\" is already in use"
            )));
        }
        return Ok(requested.to_owned());
    }
    let base = {
        let slug = slugify(statement);
        if slug.is_empty() {
            "idea".to_owned()
        } else {
            slug
        }
    };
    Ok(unique_slug(&base, &taken))
}

fn taken_hex(paths: &RootsPaths, graph: &Graph) -> RootsResult<BTreeSet<String>> {
    let mut taken = hex_set(
        graph
            .nodes
            .iter()
            .map(|node| node.id.as_str())
            .chain(graph.edges.iter().map(|edge| edge.id.as_str())),
    );
    let dirs = scan_node_dirs(paths)?;
    taken.extend(dirs.idea.keys().cloned());
    taken.extend(dirs.sprout.keys().cloned());
    Ok(taken)
}

/// Inputs shared by `plant` and the persistent half of `adopt`.
pub struct PlantInput<'a> {
    pub text: &'a str,
    pub statement: &'a str,
    pub slug: Option<&'a str>,
    pub by: &'a str,
    pub now: Option<OffsetDateTime>,
}

/// Create one human-owned idea while the graph lock is held.
fn create_idea(
    paths: &RootsPaths,
    graph: &mut Graph,
    input: &PlantInput<'_>,
) -> RootsResult<(NodeRecord, PathBuf)> {
    if !input.by.starts_with("human:") {
        return Err(RootsError::guard("an idea is authored by human:*"));
    }
    let at = iso_at(input.now.unwrap_or_else(OffsetDateTime::now_utc));
    let node = NodeRecord {
        record_type: "node".to_owned(),
        id: generate_id(IdPrefix::Idea, &taken_hex(paths, graph)?),
        kind: NodeKind::Idea,
        slug: choose_slug(graph, input.statement, input.slug)?,
        status: NodeStatus::Idea(IdeaStatus::Planted),
        author: input.by.to_owned(),
        created_at: at.clone(),
        updated_at: Some(at),
        tier: None,
        expires_at: None,
        decided_by: None,
        decided_at: None,
        decision_reason: None,
        endorses: None,
    };
    let directory = canonical_node_dir(paths, &node);
    // A generated identifier makes an existing directory a conflict rather
    // than an excuse to recurse into or remove a human-owned directory.
    fs::create_dir(&directory).map_err(error)?;
    let file = directory.join(prose_file_name(NodeKind::Idea));
    let write_result = (|| -> RootsResult<()> {
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&file)
            .map_err(error)?;
        output.write_all(input.text.as_bytes()).map_err(error)
    })();
    if let Err(problem) = write_result {
        let _ = fs::remove_dir_all(&directory);
        return Err(problem);
    }
    graph.nodes.push(node.clone());
    Ok((node, file))
}

#[derive(Clone, Debug)]
pub struct PlantResult {
    pub node: NodeRecord,
    pub file: PathBuf,
    pub statement: String,
}

/// Create the idea and its prose under the graph lock.
pub fn plant_idea(paths: &RootsPaths, input: PlantInput<'_>) -> RootsResult<(NodeRecord, PathBuf)> {
    update_graph(paths, |graph| {
        let result = create_idea(paths, graph, &input)?;
        Ok((true, result))
    })
}

/// Persist `text` as a planted idea and append its audit event.  Empty prose
/// is intentionally reported to the caller as `None` so interactive callers
/// can use their specific cancellation message.
pub fn plant_text(
    paths: &RootsPaths,
    text: &str,
    by: &str,
    slug: Option<&str>,
) -> RootsResult<Option<PlantResult>> {
    let parsed = parse_prose(text);
    if parsed.statement.is_empty() {
        return Ok(None);
    }
    let (node, file) = plant_idea(
        paths,
        PlantInput {
            text,
            statement: &parsed.statement,
            slug,
            by,
            now: None,
        },
    )?;
    let mut extra = Map::new();
    extra.insert("slug".to_owned(), Value::String(node.slug.clone()));
    extra.insert("hash".to_owned(), Value::String(content_hash(text)));
    append_event(
        paths,
        &make_event(
            "plant",
            by,
            EventFields {
                node: Some(node.id.clone()),
                extra,
                ..Default::default()
            },
        ),
    )?;
    Ok(Some(PlantResult {
        node,
        file,
        statement: parsed.statement,
    }))
}

fn text_from_args(args: &[String]) -> RootsResult<String> {
    let statement = args.join(" ");
    let statement = statement.trim();
    if statement.is_empty() {
        return Err(RootsError::usage("statement is empty"));
    }
    Ok(format!("{statement}\n"))
}

/// `luuna-roots plant [<statement>]`.
pub fn plant_command(mut context: CommandContext<'_>) -> RootsResult<()> {
    let environment = context.io.environment();
    roots_core::guard::require_tty(&environment, context.io.stdin_is_tty(), "plant")?;
    let by = human_actor(context.io)?;
    let workspace = open_workspace(context.io.cwd())?;
    let text = if context.args.is_empty() {
        compose_in_editor(context.io, "")?
    } else {
        text_from_args(context.args)?
    };
    let planted = plant_text(
        &workspace.paths,
        &text,
        &by,
        flag_string(context.flags, "slug"),
    )?
    .ok_or_else(|| RootsError::cancelled("empty idea; nothing planted"))?;
    let path = relative(&workspace.paths.root, &planted.file);
    let mut body = Map::new();
    body.insert("id".to_owned(), Value::String(planted.node.id.clone()));
    body.insert("slug".to_owned(), Value::String(planted.node.slug.clone()));
    body.insert(
        "status".to_owned(),
        Value::String(planted.node.status.as_str().to_owned()),
    );
    body.insert("author".to_owned(), Value::String(by));
    body.insert("path".to_owned(), Value::String(path.clone()));
    body.insert("statement".to_owned(), Value::String(planted.statement));
    context.out.result(context.io, body);
    context.out.success(
        context.io,
        &format!(
            "planted {} {}",
            context.out.colors.id(&planted.node.id),
            planted.node.slug
        ),
    );
    context
        .out
        .info(context.io, &format!("  {}", context.out.colors.dim(&path)));
    print_next(&mut context, &workspace.paths);
    Ok(())
}

fn open_sprout_in(graph: &Graph, id: &str, now: OffsetDateTime) -> RootsResult<NodeRecord> {
    let Some(node) = find_node(graph, id) else {
        return Err(RootsError::not_found(format!(
            "no sprout {id}; `luuna-roots list --kind sprout` lists them"
        )));
    };
    if node.kind != NodeKind::Sprout {
        return Err(RootsError::not_found(format!(
            "no sprout {id}; `luuna-roots list --kind sprout` lists them"
        )));
    }
    if is_sprout_overdue(node, now) {
        let mut detail = Map::new();
        detail.insert("status".to_owned(), Value::String("expired".to_owned()));
        return Err(RootsError::conflict(format!("{id} has expired")).with_detail(detail));
    }
    if node.status != NodeStatus::Sprout(SproutStatus::Open) {
        let who = node
            .decided_by
            .as_deref()
            .map(|by| format!(" by {by}"))
            .unwrap_or_default();
        let mut detail = Map::new();
        detail.insert(
            "status".to_owned(),
            Value::String(node.status.as_str().to_owned()),
        );
        return Err(
            RootsError::conflict(format!("{id} is already {}{who}", node.status.as_str()))
                .with_detail(detail),
        );
    }
    Ok(node.clone())
}

/// Expire overdue sprouts first, then require this one to remain open.
pub fn require_open_sprout(
    paths: &RootsPaths,
    id: &str,
    now: Option<OffsetDateTime>,
) -> RootsResult<NodeRecord> {
    let now = now.unwrap_or_else(OffsetDateTime::now_utc);
    expire_sprouts(paths, Some(now))?;
    open_sprout_in(&read_graph(paths)?, id, now)
}

pub struct AdoptInput<'a> {
    pub sprout: &'a str,
    pub text: &'a str,
    pub by: &'a str,
    pub slug: Option<&'a str>,
    pub now: Option<OffsetDateTime>,
    pub endorse: bool,
}

#[derive(Clone, Debug)]
pub struct AdoptionResult {
    pub idea: NodeRecord,
    pub sprout: NodeRecord,
    pub edge: EdgeRecord,
    pub file: PathBuf,
    pub statement: String,
}

#[derive(Clone, Debug)]
struct EndorsedSprout {
    statement: String,
    hash: String,
}

fn endorsement(paths: &RootsPaths, sprout: &NodeRecord) -> RootsResult<EndorsedSprout> {
    let prose = read_node_prose(paths, sprout, None)?;
    if !prose.exists {
        return Err(RootsError::not_found(format!(
            "{} has no sprout.md to endorse",
            sprout.id
        )));
    }
    Ok(EndorsedSprout {
        statement: prose.statement,
        hash: content_hash(&prose.text),
    })
}

fn insert_derives(
    graph: &mut Graph,
    idea: &NodeRecord,
    sprout: &NodeRecord,
    by: &str,
    now: OffsetDateTime,
) -> RootsResult<EdgeRecord> {
    if idea.kind != NodeKind::Idea || sprout.kind != NodeKind::Sprout {
        return Err(RootsError::validation(
            "`derives` goes from an idea (r-) to a sprout (s-)",
        ));
    }
    if let Some(edge) = graph
        .edges
        .iter()
        .find(|edge| edge.from == idea.id && edge.to == sprout.id && edge.rel == EdgeRel::Derives)
    {
        return Err(RootsError::validation(format!(
            "{} derives {} already exists ({})",
            idea.id, sprout.id, edge.id
        )));
    }
    let taken = hex_set(graph.edges.iter().map(|edge| edge.id.as_str()));
    let edge = EdgeRecord {
        record_type: "edge".to_owned(),
        id: generate_id(IdPrefix::Edge, &taken),
        from: idea.id.clone(),
        to: sprout.id.clone(),
        rel: EdgeRel::Derives,
        by: by.to_owned(),
        proposed_by: None,
        proposal: None,
        created_at: iso_at(now),
    };
    graph.edges.push(edge.clone());
    Ok(edge)
}

fn adoption_in_graph(
    paths: &RootsPaths,
    graph: &mut Graph,
    input: &AdoptInput<'_>,
    statement: &str,
    created_directory: &mut Option<PathBuf>,
) -> RootsResult<AdoptionResult> {
    let now = input.now.unwrap_or_else(OffsetDateTime::now_utc);
    let sprout = open_sprout_in(graph, input.sprout, now)?;
    let endorsed = input
        .endorse
        .then(|| endorsement(paths, &sprout))
        .transpose()?;
    let slug_source = endorsed
        .as_ref()
        .map(|item| item.statement.as_str())
        .unwrap_or(statement);
    let (idea, file) = create_idea(
        paths,
        graph,
        &PlantInput {
            text: input.text,
            statement: slug_source,
            slug: input.slug,
            by: input.by,
            now: Some(now),
        },
    )?;
    *created_directory = file.parent().map(Path::to_path_buf);
    let edge = match insert_derives(graph, &idea, &sprout, input.by, now) {
        Ok(edge) => edge,
        Err(problem) => {
            if let Some(directory) = created_directory.take() {
                let _ = fs::remove_dir_all(directory);
            }
            return Err(problem);
        }
    };
    let adopted_sprout = {
        let Some(current) = graph.nodes.iter_mut().find(|node| node.id == sprout.id) else {
            if let Some(directory) = created_directory.take() {
                let _ = fs::remove_dir_all(directory);
            }
            return Err(RootsError::conflict(format!("{} disappeared", sprout.id)));
        };
        current.status = NodeStatus::Sprout(SproutStatus::Adopted);
        current.decided_by = Some(input.by.to_owned());
        current.decided_at = Some(iso_at(now));
        current.updated_at = current.decided_at.clone();
        current.clone()
    };
    if let Some(endorsement) = endorsed {
        if let Some(idea_record) = graph.nodes.iter_mut().find(|node| node.id == idea.id) {
            idea_record.endorses = Some(Endorsement {
                sprout: sprout.id.clone(),
                hash: endorsement.hash,
            });
        }
    }
    let idea = graph
        .nodes
        .iter()
        .find(|node| node.id == idea.id)
        .cloned()
        .ok_or_else(|| RootsError::conflict("new idea disappeared"))?;
    Ok(AdoptionResult {
        idea,
        sprout: adopted_sprout,
        edge,
        file,
        statement: statement.to_owned(),
    })
}

/// Persist a human rewrite (or endorsement thoughts), add the immutable
/// `derives` lineage edge, and mark the sprout adopted in one graph write.
pub fn complete_adoption(paths: &RootsPaths, input: AdoptInput<'_>) -> RootsResult<AdoptionResult> {
    if !input.by.starts_with("human:") {
        return Err(RootsError::guard("only a human adopts a sprout"));
    }
    let parsed = parse_prose(input.text);
    if parsed.statement.is_empty() {
        return Err(RootsError::cancelled(format!(
            "empty idea; {} was not adopted and nothing was written",
            input.sprout
        )));
    }
    let mut created_directory = None;
    let result = update_graph(paths, |graph| {
        let result = adoption_in_graph(
            paths,
            graph,
            &input,
            &parsed.statement,
            &mut created_directory,
        )?;
        Ok((true, result))
    });
    let result = match result {
        Ok(result) => result,
        Err(problem) => {
            if let Some(directory) = created_directory.take() {
                let _ = fs::remove_dir_all(directory);
            }
            return Err(problem);
        }
    };
    let mut planted_extra = Map::new();
    planted_extra.insert("slug".to_owned(), Value::String(result.idea.slug.clone()));
    planted_extra.insert("hash".to_owned(), Value::String(content_hash(input.text)));
    planted_extra.insert("sprout".to_owned(), Value::String(result.sprout.id.clone()));
    append_event(
        paths,
        &make_event(
            "plant",
            input.by,
            EventFields {
                node: Some(result.idea.id.clone()),
                extra: planted_extra,
                ..Default::default()
            },
        ),
    )?;
    let mut adopted_extra = Map::new();
    adopted_extra.insert("idea".to_owned(), Value::String(result.idea.id.clone()));
    adopted_extra.insert("edge".to_owned(), Value::String(result.edge.id.clone()));
    if let Some(endorsement) = &result.idea.endorses {
        adopted_extra.insert("mode".to_owned(), Value::String("endorse".to_owned()));
        adopted_extra.insert(
            "sproutHash".to_owned(),
            Value::String(endorsement.hash.clone()),
        );
    }
    append_event(
        paths,
        &make_event(
            "adopt",
            input.by,
            EventFields {
                node: Some(result.sprout.id.clone()),
                refs: Some(vec![result.idea.id.clone()]),
                extra: adopted_extra,
                ..Default::default()
            },
        ),
    )?;
    Ok(result)
}

pub struct AdoptionOptions<'a> {
    pub by: &'a str,
    pub split: bool,
    pub slug: Option<&'a str>,
    pub spawn: Option<&'a Spawner>,
    /// `Some` means endorse, with the human's existing thoughts prefilled.
    pub endorse: Option<&'a str>,
}

fn reference_title(sprout: &NodeRecord) -> String {
    let who = sprout
        .author
        .strip_prefix("agent:")
        .unwrap_or(&sprout.author);
    format!(
        "[agent] sprout {} {} ({who}) · reference only",
        sprout.id, sprout.slug
    )
}

const REWRITE_HINT: &str =
    "# Write the idea in your own words ABOVE the line. Nothing above it cancels.";
const ENDORSE_HINT: &str = "# Endorsing: the sprout below is kept as the agent's text (quoted, labeled). Write\n# your thoughts on it ABOVE the line; they are the human part. Nothing above cancels.";

/// Build the quoted reference placed below the editor's scissors marker.
#[must_use]
pub fn sprout_reference(sprout: &NodeRecord, text: &str, endorse: bool) -> Vec<String> {
    let mut output = vec![
        if endorse {
            ENDORSE_HINT.to_owned()
        } else {
            REWRITE_HINT.to_owned()
        },
        "#".to_owned(),
        format!("# {}", reference_title(sprout)),
        String::new(),
    ];
    output.extend(text.trim_end().split('\n').map(|line| {
        if line.is_empty() {
            ">".to_owned()
        } else {
            format!("> {line}")
        }
    }));
    output
}

fn reference_block(
    io: &mut dyn Io,
    sprout: &NodeRecord,
    text: &str,
    launch: Option<SplitLaunch>,
    endorse: bool,
) {
    let mut lines = vec![format!("── {} ──", reference_title(sprout))];
    lines.extend(text.trim_end().split('\n').map(ToOwned::to_owned));
    lines.push("──".to_owned());
    if launch.is_some_and(|result| result.ok) {
        let mux = launch.expect("checked launch").mux.expect("successful mux");
        lines.push(format!(
            "(also open read-only in a {} pane; quit it with q)",
            mux.as_str()
        ));
    }
    lines.push(
        if endorse {
            "Endorsing: write your thoughts on the sprout in the editor; the sprout stays the agent's."
        } else {
            "Write the idea in your own words in the editor. Do not paste the sprout."
        }
        .to_owned(),
    );
    lines.push("Save an empty file to cancel.".to_owned());
    lines.push(String::new());
    io.stderr(&format!("{}\n", lines.join("\n")));
}

/// The interactive adoption shared by `adopt` and later interactive review
/// commands.  It never copies sprout prose into `idea.md`.
pub fn run_adoption(
    io: &mut dyn Io,
    paths: &RootsPaths,
    sprout_id: &str,
    options: AdoptionOptions<'_>,
) -> RootsResult<AdoptionResult> {
    let sprout = require_open_sprout(paths, sprout_id, None)?;
    let prose = read_node_prose(paths, &sprout, None)?;
    let relative_file = prose
        .path
        .as_deref()
        .map(|file| relative(&paths.root, file))
        .unwrap_or_default();
    let environment = io.environment();
    let launch = if options.split && !relative_file.is_empty() {
        Some(launch_in_split(
            &environment,
            SplitCommand {
                command: &resolve_pager(&environment),
                file: &relative_file,
                cwd: &paths.root,
                focus: false,
            },
            options.spawn,
        ))
    } else {
        None
    };
    let reference = if prose.text.is_empty() {
        "(sprout.md missing)"
    } else {
        &prose.text
    };
    let endorse = options.endorse.is_some();
    reference_block(io, &sprout, reference, launch, endorse);
    let buffer = reference_buffer(&sprout_reference(&sprout, reference, endorse));
    let initial = match options.endorse {
        Some(thoughts) => format!("{thoughts}{buffer}"),
        None => buffer,
    };
    let text = compose_in_editor(io, &initial)?;
    complete_adoption(
        paths,
        AdoptInput {
            sprout: &sprout.id,
            text: &text,
            by: options.by,
            slug: options.slug,
            now: None,
            endorse,
        },
    )
}

/// `luuna-roots adopt <sprout-id>`.
pub fn adopt_command(mut context: CommandContext<'_>) -> RootsResult<()> {
    let environment = context.io.environment();
    roots_core::guard::require_tty(&environment, context.io.stdin_is_tty(), "adopt")?;
    let by = human_command_actor(context.io, "adopt")?;
    let workspace = open_workspace(context.io.cwd())?;
    let requested = context.args.first().map(String::as_str).unwrap_or_default();
    let sprout = resolve_node(
        &workspace.graph.nodes,
        requested,
        ResolveOptions {
            kind: Some(NodeKind::Sprout),
        },
    )?;
    let result = run_adoption(
        context.io,
        &workspace.paths,
        &sprout.id,
        AdoptionOptions {
            by: &by,
            split: !flag_bool(context.flags, "no-split"),
            slug: flag_string(context.flags, "slug"),
            spawn: None,
            endorse: flag_bool(context.flags, "endorse").then_some(""),
        },
    )?;
    let path = relative(&workspace.paths.root, &result.file);
    let mut body = Map::new();
    body.insert("id".to_owned(), Value::String(result.idea.id.clone()));
    body.insert("slug".to_owned(), Value::String(result.idea.slug.clone()));
    body.insert("path".to_owned(), Value::String(path.clone()));
    body.insert("sprout".to_owned(), json(&result.sprout)?);
    body.insert("edge".to_owned(), json(&result.edge)?);
    context.out.result(context.io, body);
    let verb = if result.idea.endorses.is_some() {
        "endorsed"
    } else {
        "adopted"
    };
    context.out.success(
        context.io,
        &format!(
            "{verb} {} as {} {}",
            context.out.colors.id(&sprout.id),
            context.out.colors.id(&result.idea.id),
            result.idea.slug
        ),
    );
    context.out.info(
        context.io,
        &format!(
            "  {}",
            context.out.colors.dim(&format!(
                "{path} · {} derives {}",
                result.edge.id, sprout.id
            ))
        ),
    );
    print_next(&mut context, &workspace.paths);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::MemoryIo;
    use crate::run_cli;
    use roots_core::config::{default_config, write_config};
    use roots_core::events::read_events;
    use roots_core::paths::roots_paths;
    use roots_core::sprouts::{SproutInput, file_sprout};
    use std::collections::BTreeMap;

    fn sandbox(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "roots-capture-{name}-{}",
            roots_core::ids::random_hex(8)
        ));
        fs::create_dir_all(&root).expect("create sandbox");
        root
    }

    fn initialize(root: &Path) -> RootsPaths {
        let paths = roots_paths(root);
        fs::create_dir_all(&paths.human).expect("human tree");
        fs::create_dir_all(&paths.sprouts).expect("sprouts tree");
        fs::create_dir_all(&paths.notes).expect("notes tree");
        write_config(&paths, &default_config("test")).expect("config");
        for file in [
            &paths.graph,
            &paths.events,
            &paths.proposals,
            &paths.questions,
        ] {
            fs::write(file, "").expect("store");
        }
        paths
    }

    fn human_io(root: &Path) -> MemoryIo {
        let mut io = MemoryIo::new(root);
        io.env = BTreeMap::from([
            ("ROOTS_USER".to_owned(), "Test Human".to_owned()),
            ("NO_COLOR".to_owned(), "1".to_owned()),
        ]);
        io
    }

    #[test]
    fn plant_text_creates_human_prose_node_and_event() {
        let root = sandbox("plant");
        let paths = initialize(&root);
        let created = plant_text(
            &paths,
            "\nOffline sync keeps work safe.\n\nMore detail.\n",
            "human:test",
            None,
        )
        .expect("plant")
        .expect("nonempty");
        assert_eq!(created.node.kind, NodeKind::Idea);
        assert_eq!(created.node.status, NodeStatus::Idea(IdeaStatus::Planted));
        assert_eq!(created.statement, "Offline sync keeps work safe.");
        assert_eq!(
            fs::read_to_string(&created.file).expect("prose"),
            "\nOffline sync keeps work safe.\n\nMore detail.\n"
        );
        let event = read_events(&paths)
            .expect("events")
            .pop()
            .expect("plant event");
        assert_eq!(event.event_type, "plant");
        assert_eq!(event.node.as_deref(), Some(created.node.id.as_str()));
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn adoption_rewrites_or_endorses_without_copying_agent_prose() {
        let root = sandbox("adopt");
        let paths = initialize(&root);
        let sprout = file_sprout(
            &paths,
            SproutInput {
                statement: "Conflicts get a merge UI",
                body: Some("Agent body: users lose edits."),
                slug: None,
                by: "agent:opus",
                now: None,
            },
        )
        .expect("sprout")
        .node;
        let adopted = complete_adoption(
            &paths,
            AdoptInput {
                sprout: &sprout.id,
                text: "Humans resolve conflicts by hand\n\nIn my words.\n",
                by: "human:test",
                slug: None,
                now: None,
                endorse: false,
            },
        )
        .expect("adopt");
        assert_eq!(adopted.edge.rel, EdgeRel::Derives);
        assert_eq!(adopted.edge.to, sprout.id);
        assert_eq!(
            adopted.sprout.status,
            NodeStatus::Sprout(SproutStatus::Adopted)
        );
        assert!(
            !fs::read_to_string(&adopted.file)
                .expect("idea")
                .contains("Agent body")
        );

        let another = file_sprout(
            &paths,
            SproutInput {
                statement: "Keep history visible",
                body: Some("Agent wording."),
                slug: None,
                by: "agent:opus",
                now: None,
            },
        )
        .expect("second sprout")
        .node;
        let endorsed = complete_adoption(
            &paths,
            AdoptInput {
                sprout: &another.id,
                text: "Yes, with a manual audit trail\n",
                by: "human:test",
                slug: None,
                now: None,
                endorse: true,
            },
        )
        .expect("endorse");
        assert_eq!(
            endorsed.idea.endorses.as_ref().map(|item| &item.sprout),
            Some(&another.id)
        );
        assert!(
            endorsed
                .idea
                .endorses
                .as_ref()
                .is_some_and(|item| item.hash.starts_with("sha256:"))
        );
        let events = read_events(&paths).expect("events");
        assert_eq!(events.last().expect("adopt event").extra["mode"], "endorse");
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn capture_handlers_enforce_tty_human_guards_and_json_shape() {
        let root = sandbox("handler");
        initialize(&root);
        let mut no_tty = human_io(&root);
        no_tty.stdin_is_tty = false;
        assert_eq!(
            run_cli(
                &["plant".into(), "One claim".into(), "--json".into()],
                &mut no_tty
            ),
            roots_core::errors::EXIT_GUARD
        );

        let mut io = human_io(&root);
        assert_eq!(
            run_cli(
                &[
                    "plant".into(),
                    "One".into(),
                    "claim".into(),
                    "--json".into(),
                ],
                &mut io,
            ),
            0,
            "{}",
            io.stderr
        );
        let result: Value = roots_core::serde_json::from_str(&io.stdout).expect("json");
        assert_eq!(result["statement"], "One claim");
        assert_eq!(result["author"], "human:test-human");
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn quoted_reference_is_below_scissors_and_never_becomes_human_text() {
        let sprout = NodeRecord::new(
            "s-a1b2".to_owned(),
            NodeKind::Sprout,
            "merge-ui".to_owned(),
            NodeStatus::Sprout(SproutStatus::Open),
            "agent:opus".to_owned(),
            "now".to_owned(),
        );
        let reference = sprout_reference(&sprout, "Agent line\n\nBody", false);
        let buffer = reference_buffer(&reference);
        assert!(buffer.contains("[agent] sprout s-a1b2 merge-ui (opus) · reference only"));
        assert!(buffer.contains("> Agent line"));
        assert_eq!(strip_reference(&format!("Mine\n{buffer}")), "Mine\n");
    }
}
