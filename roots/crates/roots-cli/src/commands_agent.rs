//! Agent-facing mutation command handlers.
//!
//! These commands intentionally only create records in agent-owned storage or
//! append audit records.  They never write Roots' human prose.

use crate::registry::CommandContext;
use roots_core::actor::resolve_agent_actor;
use roots_core::args::{flag_bool, flag_list, flag_string};
use roots_core::config::load_config;
use roots_core::errors::{RootsError, RootsResult};
use roots_core::headings::{HeadingInput, file_heading};
use roots_core::model::{NodeKind, ProposalKind};
use roots_core::notes::{NoteInput, attach_note};
use roots_core::proposals::{
    ProposeInput, expected_nodes, kind_name, parse_cite, proposal_kind, propose_as_agent,
};
use roots_core::questions::{AskInput, ask_agent_question};
use roots_core::resolve::{ResolveOptions, resolve_node};
use roots_core::serde_json::{Map, Value, to_value};
use roots_core::sprouts::{SPROUT_BODY_MAX_BYTES, SproutInput, file_sprout};
use roots_core::time_crate::{OffsetDateTime, format_description::well_known::Rfc3339};
use roots_core::workspace::open_workspace;
use std::fs;
use std::path::{Path, PathBuf};

fn json<T: roots_core::serde::Serialize>(value: T) -> RootsResult<Value> {
    to_value(value).map_err(|error| RootsError::error(error.to_string()))
}

fn agent_actor(context: &CommandContext<'_>) -> RootsResult<String> {
    let environment = context.io.environment();
    resolve_agent_actor(flag_string(context.flags, "as"), &environment)
}

fn relative(root: &Path, file: &Path) -> String {
    file.strip_prefix(root)
        .unwrap_or(file)
        .display()
        .to_string()
}

fn days_left(value: Option<&str>) -> Option<i64> {
    let value = value?;
    let expiry = OffsetDateTime::parse(value, &Rfc3339).ok()?;
    let nanos = (expiry - OffsetDateTime::now_utc()).whole_nanoseconds();
    const DAY_NANOS: i128 = 86_400_000_000_000;
    let days = if nanos >= 0 {
        (nanos + DAY_NANOS - 1) / DAY_NANOS
    } else {
        nanos / DAY_NANOS
    };
    Some(std::cmp::max(0, days) as i64)
}

/// `luuna-roots ask <id> <question>`.
pub fn ask_command(context: CommandContext<'_>) -> RootsResult<()> {
    let by = agent_actor(&context)?;
    let query = context.args.first().map(String::as_str).unwrap_or_default();
    let question_text = context
        .args
        .iter()
        .skip(1)
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(" ");
    let workspace = open_workspace(context.io.cwd())?;
    let node = resolve_node(&workspace.graph.nodes, query, ResolveOptions::default())?.clone();
    let session = flag_string(context.flags, "session").or_else(|| context.io.env("ROOTS_SESSION"));
    let result = ask_agent_question(
        &workspace.paths,
        AskInput {
            node: &node,
            text: &question_text,
            by: &by,
            session,
            now: None,
        },
    )?;
    let mut body = Map::new();
    body.insert("id".into(), Value::String(result.question.id.clone()));
    body.insert("node".into(), Value::String(node.id.clone()));
    body.insert("question".into(), json(&result.question)?);
    body.insert(
        "session".into(),
        result.session.map_or(Value::Null, Value::String),
    );
    context.out.result(context.io, body);
    context.out.success(
        context.io,
        &format!(
            "asked {} on {} {}",
            context.out.colors.id(&result.question.id),
            context.out.colors.id(&node.id),
            node.slug
        ),
    );
    Ok(())
}

/// `luuna-roots note <id> --file <path>`.
pub fn note_command(context: CommandContext<'_>) -> RootsResult<()> {
    let by = agent_actor(&context)?;
    let file = flag_string(context.flags, "file")
        .ok_or_else(|| RootsError::usage("--file <path> is required"))?;
    let workspace = open_workspace(context.io.cwd())?;
    let query = context.args.first().map(String::as_str).unwrap_or_default();
    let node = resolve_node(
        &workspace.graph.nodes,
        query,
        ResolveOptions {
            kind: Some(NodeKind::Idea),
        },
    )?
    .clone();
    let result = attach_note(
        &workspace.paths,
        NoteInput {
            node: &node,
            file,
            cwd: context.io.cwd(),
            name: flag_string(context.flags, "name"),
            question: flag_string(context.flags, "question"),
            by: &by,
            now: None,
        },
    )?;
    let mut body = Map::new();
    body.insert("id".into(), Value::String(node.id.clone()));
    body.insert("note".into(), json(&result.note)?);
    body.insert(
        "question".into(),
        result.question.clone().map_or(Value::Null, Value::String),
    );
    context.out.result(context.io, body);
    let question = result
        .question
        .map(|id| {
            format!(
                " (findings for {}, back with the human)",
                context.out.colors.id(&id)
            )
        })
        .unwrap_or_default();
    context.out.success(
        context.io,
        &format!(
            "noted {} {}: {}{question}",
            context.out.colors.id(&node.id),
            node.slug,
            result.note.path
        ),
    );
    Ok(())
}

fn parse_proposal_shape(args: &[String]) -> RootsResult<(ProposalKind, Vec<&str>, Option<&str>)> {
    let raw = args.first().map(String::as_str).unwrap_or_default();
    let kind = proposal_kind(raw).ok_or_else(|| {
        RootsError::usage(format!(
            "unknown proposal kind \"{raw}\"; use edge, split, merge, compost"
        ))
    })?;
    let rest = &args[1..];
    let wanted = expected_nodes(kind) + usize::from(kind == ProposalKind::Edge);
    if rest.len() != wanted {
        let shape = match kind {
            ProposalKind::Edge => "edge <a> <b> <serves|tension|replaces>",
            ProposalKind::Split => "split <id>",
            ProposalKind::Merge => "merge <a> <b>",
            ProposalKind::Compost => "compost <id>",
        };
        return Err(RootsError::usage(format!(
            "usage: luuna-roots propose {shape} --reason <text> --cite <id>:<quote>"
        )));
    }
    if kind == ProposalKind::Edge {
        Ok((
            kind,
            vec![rest[0].as_str(), rest[1].as_str()],
            Some(rest[2].as_str()),
        ))
    } else {
        Ok((kind, rest.iter().map(String::as_str).collect(), None))
    }
}

/// `luuna-roots propose edge|split|merge|compost …`.
pub fn propose_command(context: CommandContext<'_>) -> RootsResult<()> {
    let by = agent_actor(&context)?;
    let (kind, nodes, rel) = parse_proposal_shape(context.args)?;
    let workspace = open_workspace(context.io.cwd())?;
    let proposal = propose_as_agent(
        &workspace.paths,
        ProposeInput {
            kind,
            nodes,
            rel,
            reason: flag_string(context.flags, "reason"),
            cites: flag_list(context.flags, "cite"),
            mention: flag_bool(context.flags, "mention"),
            by: &by,
            now: None,
        },
    )?;
    let mut body = Map::new();
    body.insert("id".into(), Value::String(proposal.id.clone()));
    body.insert("proposal".into(), json(&proposal)?);
    context.out.result(context.io, body);
    let what = match proposal.kind {
        ProposalKind::Edge => format!(
            "{} {} {}",
            proposal.from.as_deref().unwrap_or_default(),
            proposal.rel.map(|rel| rel.as_str()).unwrap_or_default(),
            proposal.to.as_deref().unwrap_or_default()
        ),
        _ => format!(
            "{} {}{}",
            kind_name(proposal.kind),
            proposal.from.as_deref().unwrap_or_default(),
            proposal
                .to
                .as_deref()
                .map(|to| format!(" {to}"))
                .unwrap_or_default()
        ),
    };
    context.out.success(
        context.io,
        &format!(
            "proposed {}: {what} (a human reviews it in `luuna-roots tend`)",
            context.out.colors.id(&proposal.id)
        ),
    );
    Ok(())
}

fn sprout_body(cwd: &Path, raw: Option<&str>) -> RootsResult<Option<String>> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    let path = if Path::new(raw).is_absolute() {
        PathBuf::from(raw)
    } else {
        cwd.join(raw)
    };
    let metadata = fs::metadata(&path)
        .map_err(|_| RootsError::not_found(format!("--file {raw}: no such file")))?;
    if !metadata.is_file() {
        return Err(RootsError::usage(format!(
            "--file {raw} is not a regular file"
        )));
    }
    if metadata.len() > SPROUT_BODY_MAX_BYTES as u64 {
        return Err(RootsError::validation(format!(
            "--file is {} bytes; the limit is {SPROUT_BODY_MAX_BYTES}. Attach long material with `luuna-roots note`",
            metadata.len()
        )));
    }
    fs::read_to_string(&path)
        .map(Some)
        .map_err(|error| RootsError::error(error.to_string()))
}

/// `luuna-roots sprout <statement> [--file <md>]`.
pub fn sprout_command(context: CommandContext<'_>) -> RootsResult<()> {
    let by = agent_actor(&context)?;
    let workspace = open_workspace(context.io.cwd())?;
    let statement = context
        .args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(" ");
    let body = sprout_body(context.io.cwd(), flag_string(context.flags, "file"))?;
    let result = file_sprout(
        &workspace.paths,
        SproutInput {
            statement: &statement,
            body: body.as_deref(),
            slug: flag_string(context.flags, "slug"),
            by: &by,
            now: None,
        },
    )?;
    let mut value = Map::new();
    value.insert("id".into(), Value::String(result.node.id.clone()));
    value.insert("slug".into(), Value::String(result.node.slug.clone()));
    value.insert("status".into(), json(result.node.status)?);
    value.insert("author".into(), Value::String(by));
    value.insert(
        "path".into(),
        Value::String(relative(&workspace.paths.root, &result.file)),
    );
    value.insert("statement".into(), Value::String(result.statement));
    value.insert(
        "expiresAt".into(),
        result
            .node
            .expires_at
            .clone()
            .map_or(Value::Null, Value::String),
    );
    context.out.result(context.io, value);
    context.out.success(
        context.io,
        &format!(
            "sprouted {} {}",
            context.out.colors.id(&result.node.id),
            result.node.slug
        ),
    );
    let expiry = days_left(result.node.expires_at.as_deref())
        .map(|days| format!("{days}d"))
        .unwrap_or_else(|| "unknown".to_owned());
    context.out.info(
        context.io,
        &format!(
            "  {}",
            context.out.colors.dim(&format!(
                "{} · expires in {expiry} · a human decides in `luuna-roots tend`",
                relative(&workspace.paths.root, &result.file)
            ))
        ),
    );
    Ok(())
}

/// `luuna-roots heading <text> --cite <id>:<quote> …`.
pub fn heading_command(context: CommandContext<'_>) -> RootsResult<()> {
    let by = agent_actor(&context)?;
    let flow = flag_string(context.flags, "flow")
        .or_else(|| context.io.env("ROOTS_FLOW"))
        .unwrap_or("")
        .trim()
        .to_owned();
    if flow.is_empty() {
        return Err(RootsError::usage(
            "no flow session: pass --flow <fl-id> or set ROOTS_FLOW",
        ));
    }
    let workspace = open_workspace(context.io.cwd())?;
    let next = flag_string(context.flags, "next")
        .map(|query| {
            resolve_node(
                &workspace.graph.nodes,
                query,
                ResolveOptions {
                    kind: Some(NodeKind::Idea),
                },
            )
            .map(|node| node.id.clone())
        })
        .transpose()?;
    let cites = flag_list(context.flags, "cite")
        .into_iter()
        .map(|raw| parse_cite(raw, &workspace.graph))
        .collect::<RootsResult<Vec<_>>>()?;
    let config = load_config(&workspace.paths)?;
    let text = context
        .args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(" ");
    let heading = file_heading(
        &workspace.paths,
        &config,
        HeadingInput {
            flow: &flow,
            text: &text,
            cites,
            next: next.as_deref(),
            by: &by,
            now: None,
        },
    )?;
    let mut body = Map::new();
    body.insert("id".into(), Value::String(heading.id.clone()));
    body.insert("heading".into(), json(&heading)?);
    context.out.result(context.io, body);
    context.out.success(
        context.io,
        &format!(
            "heading {} for {}",
            context.out.colors.id(&heading.id),
            context.out.colors.id(&flow)
        ),
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::MemoryIo;
    use crate::run_cli;
    use roots_core::config::{default_config, write_config};
    use roots_core::events::{EventFields, append_event, make_event, read_events};
    use roots_core::graph::{read_graph, update_graph};
    use roots_core::model::{IdeaStatus, NodeRecord, NodeStatus};
    use roots_core::paths::roots_paths;
    use roots_core::prose::canonical_node_dir;
    use roots_core::questions::read_questions;
    use std::collections::BTreeMap;

    fn sandbox(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "roots-agent-{name}-{}",
            roots_core::ids::random_hex(8)
        ));
        fs::create_dir_all(&root).expect("create test root");
        root
    }

    fn setup(root: &Path) {
        let paths = roots_paths(root);
        fs::create_dir_all(&paths.human).expect("human");
        fs::create_dir_all(&paths.sprouts).expect("sprouts");
        fs::create_dir_all(&paths.notes).expect("notes");
        write_config(&paths, &default_config("test")).expect("config");
        for file in [
            &paths.graph,
            &paths.proposals,
            &paths.questions,
            &paths.events,
            &paths.headings,
        ] {
            fs::write(file, "").expect("empty store");
        }
        for (id, slug, statement) in [
            ("r-a1b2", "alpha", "Alpha stays offline"),
            ("r-b2c3", "beta", "Beta resolves conflicts"),
        ] {
            let node = NodeRecord::new(
                id.into(),
                NodeKind::Idea,
                slug.into(),
                NodeStatus::Idea(IdeaStatus::Planted),
                "human:test".into(),
                "2026-10-22T00:00:00Z".into(),
            );
            let directory = canonical_node_dir(&paths, &node);
            fs::create_dir_all(&directory).expect("idea directory");
            fs::write(directory.join("idea.md"), format!("{statement}\n")).expect("prose");
            update_graph(&paths, |graph| {
                graph.nodes.push(node);
                Ok((true, ()))
            })
            .expect("graph");
        }
    }

    fn invoke(root: &Path, parts: &[&str]) -> (i32, MemoryIo) {
        let mut io = MemoryIo::new(root);
        io.env = BTreeMap::from([
            ("ROOTS_AGENT".to_owned(), "agent:test".to_owned()),
            ("NO_COLOR".to_owned(), "1".to_owned()),
        ]);
        let argv = parts
            .iter()
            .map(|part| (*part).to_owned())
            .collect::<Vec<_>>();
        let exit = run_cli(&argv, &mut io);
        (exit, io)
    }

    #[test]
    fn agent_mutations_write_only_agent_records_and_respect_the_wire_contract() {
        let root = sandbox("mutations");
        setup(&root);
        let paths = roots_paths(&root);
        fs::write(root.join("research.md"), "finding\n").expect("outside artifact");

        let (exit, ask) = invoke(&root, &["ask", "r-a1b2", "What", "fails?", "--json"]);
        assert_eq!(exit, 0, "{}", ask.stderr);
        assert_eq!(read_questions(&paths).expect("questions").len(), 1);

        let (exit, note) = invoke(
            &root,
            &["note", "r-a1b2", "--file", "research.md", "--json"],
        );
        assert_eq!(exit, 0, "{}", note.stderr);
        assert!(note.stdout.contains("agent\\\\notes") || note.stdout.contains("agent/notes"));

        let (exit, proposal) = invoke(
            &root,
            &[
                "propose",
                "edge",
                "r-a1b2",
                "r-b2c3",
                "tension",
                "--reason",
                "contradiction",
                "--cite",
                "r-a1b2:Alpha",
                "--cite",
                "r-b2c3:Beta",
                "--json",
            ],
        );
        assert_eq!(exit, 0, "{}", proposal.stderr);
        assert!(proposal.stdout.contains("\"kind\": \"edge\""));

        let (exit, sprout) = invoke(&root, &["sprout", "A", "new", "candidate", "--json"]);
        assert_eq!(exit, 0, "{}", sprout.stderr);
        assert!(
            read_graph(&paths)
                .expect("graph")
                .nodes
                .iter()
                .any(|node| node.kind == NodeKind::Sprout)
        );

        let mut extra = Map::new();
        extra.insert("flow".into(), Value::String("fl-a1b2".into()));
        append_event(
            &paths,
            &make_event(
                "flow.start",
                "human:test",
                EventFields {
                    extra,
                    ..Default::default()
                },
            ),
        )
        .expect("flow start");
        let (exit, heading) = invoke(
            &root,
            &[
                "heading",
                "Alpha",
                "first.",
                "--flow",
                "fl-a1b2",
                "--cite",
                "r-a1b2:Alpha",
                "--json",
            ],
        );
        assert_eq!(exit, 0, "{}", heading.stderr);
        assert!(
            read_events(&paths)
                .expect("events")
                .iter()
                .any(|event| event.event_type == "heading")
        );
        fs::remove_dir_all(root).expect("clean test root");
    }
}
