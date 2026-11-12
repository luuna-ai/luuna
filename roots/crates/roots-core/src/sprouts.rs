//! Agent-owned sprout creation, duplicate memory, and expiry.

use crate::boundary::agent_write_exclusive;
use crate::claims::read_claim;
use crate::config::load_config;
use crate::errors::{RootsError, RootsResult};
use crate::events::{EventFields, append_event, make_event};
use crate::graph::{read_graph, update_graph};
use crate::ids::{IdPrefix, generate_id, hex_set};
use crate::model::{Graph, NodeKind, NodeRecord, NodeStatus, SproutStatus};
use crate::paths::{RootsPaths, prose_file_name};
use crate::proposals::expires_at;
use crate::prose::{canonical_node_dir, content_hash, scan_node_dirs};
use crate::slug::{is_valid_slug, slugify, unique_slug};
use crate::tier::require_tier;
use crate::time::iso_at;
use serde_json::{Map, Value};
use std::collections::BTreeSet;
use std::path::PathBuf;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

pub const SPROUT_STATEMENT_MAX_LENGTH: usize = 200;
pub const SPROUT_BODY_MAX_BYTES: usize = 64 * 1024;

#[derive(Clone, Debug)]
pub struct SproutInput<'a> {
    pub statement: &'a str,
    pub body: Option<&'a str>,
    pub slug: Option<&'a str>,
    pub by: &'a str,
    pub now: Option<OffsetDateTime>,
}

#[derive(Clone, Debug)]
pub struct SproutResult {
    pub node: NodeRecord,
    pub file: PathBuf,
    pub statement: String,
    pub expired: Vec<NodeRecord>,
}

fn parsed_at(value: &str) -> Option<OffsetDateTime> {
    OffsetDateTime::parse(value, &Rfc3339).ok()
}

#[must_use]
pub fn statement_key(text: &str) -> String {
    text.chars()
        .flat_map(char::to_lowercase)
        .map(|character| {
            if character.is_alphanumeric() {
                character
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[must_use]
pub fn is_sprout_overdue(node: &NodeRecord, now: OffsetDateTime) -> bool {
    node.kind == NodeKind::Sprout
        && matches!(node.status, NodeStatus::Sprout(SproutStatus::Open))
        && node
            .expires_at
            .as_deref()
            .and_then(parsed_at)
            .is_some_and(|expires| expires <= now)
}

#[must_use]
pub fn is_open_sprout(node: &NodeRecord, now: OffsetDateTime) -> bool {
    node.kind == NodeKind::Sprout
        && matches!(node.status, NodeStatus::Sprout(SproutStatus::Open))
        && !is_sprout_overdue(node, now)
}

#[must_use]
pub fn open_sprouts(graph: &Graph, now: OffsetDateTime) -> Vec<&NodeRecord> {
    let mut rows: Vec<_> = graph
        .nodes
        .iter()
        .filter(|node| is_open_sprout(node, now))
        .collect();
    rows.sort_by_key(|node| node.expires_at.clone().unwrap_or_else(|| "~".to_owned()));
    rows
}

pub fn check_sprout_statement(raw: &str) -> RootsResult<String> {
    if raw.trim().contains(['\r', '\n']) {
        return Err(RootsError::validation(
            "a sprout statement is one line; put the rest in --file",
        ));
    }
    let statement = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    if statement.is_empty() {
        return Err(RootsError::usage("the sprout statement is empty"));
    }
    let length = statement.encode_utf16().count();
    if length > SPROUT_STATEMENT_MAX_LENGTH {
        return Err(RootsError::validation(format!(
            "statement is {length} characters; the limit is {SPROUT_STATEMENT_MAX_LENGTH}. One claim, one sentence"
        )));
    }
    Ok(statement)
}

fn check_body(raw: Option<&str>) -> RootsResult<String> {
    let body = raw.unwrap_or("").trim().to_owned();
    let bytes = body.len();
    if bytes > SPROUT_BODY_MAX_BYTES {
        return Err(RootsError::validation(format!(
            "sprout body is {bytes} bytes; the limit is {SPROUT_BODY_MAX_BYTES}. Attach long material with `luuna-roots note`"
        )));
    }
    Ok(body)
}

#[must_use]
pub fn sprout_text(statement: &str, body: &str) -> String {
    if body.is_empty() {
        format!("{statement}\n")
    } else {
        format!("{statement}\n\n{body}\n")
    }
}

fn find_duplicate(
    paths: &RootsPaths,
    graph: &Graph,
    statement: &str,
    now: OffsetDateTime,
) -> RootsResult<Option<NodeRecord>> {
    let key = statement_key(statement);
    for node in &graph.nodes {
        if node.kind == NodeKind::Sprout {
            let blocking = matches!(
                node.status,
                NodeStatus::Sprout(
                    SproutStatus::Open | SproutStatus::Adopted | SproutStatus::Rejected
                )
            );
            if !blocking || is_sprout_overdue(node, now) {
                continue;
            }
        }
        if statement_key(&read_claim(paths, graph, node)?.statement) == key {
            return Ok(Some(node.clone()));
        }
    }
    Ok(None)
}

fn duplicate_error(node: &NodeRecord) -> RootsError {
    let message = if node.kind == NodeKind::Idea {
        format!(
            "duplicate sprout: the human already has this idea ({} {})",
            node.id, node.slug
        )
    } else if matches!(node.status, NodeStatus::Sprout(SproutStatus::Rejected)) {
        let suffix = node
            .decision_reason
            .as_ref()
            .map(|reason| format!(": \"{reason}\""))
            .unwrap_or_default();
        format!(
            "duplicate sprout: the human rejected it ({}{suffix}); rejections are permanent",
            node.id
        )
    } else {
        format!(
            "duplicate sprout: it is already sprout {} ({})",
            node.id,
            node.status.as_str()
        )
    };
    let mut detail = Map::new();
    detail.insert("duplicate".into(), Value::String(node.id.clone()));
    RootsError::validation(message).with_detail(detail)
}

fn choose_slug(graph: &Graph, statement: &str, requested: Option<&str>) -> RootsResult<String> {
    let taken: BTreeSet<_> = graph.nodes.iter().map(|node| node.slug.clone()).collect();
    if let Some(requested) = requested {
        if !is_valid_slug(requested) {
            return Err(RootsError::usage(format!(
                "invalid slug \"{requested}\" (try \"{}\")",
                slugify(requested).if_empty_then("my-idea")
            )));
        }
        if taken.contains(requested) {
            return Err(RootsError::conflict(format!(
                "slug \"{requested}\" is already in use"
            )));
        }
        return Ok(requested.to_owned());
    }
    Ok(unique_slug(
        &{
            let slug = slugify(statement);
            if slug.is_empty() {
                "idea".to_owned()
            } else {
                slug
            }
        },
        &taken,
    ))
}

trait IfEmptyThen {
    fn if_empty_then(self, fallback: &str) -> String;
}
impl IfEmptyThen for String {
    fn if_empty_then(self, fallback: &str) -> String {
        if self.is_empty() {
            fallback.to_owned()
        } else {
            self
        }
    }
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

fn expire_in_graph(graph: &mut Graph, now: OffsetDateTime) -> Vec<NodeRecord> {
    let mut out = Vec::new();
    for node in &mut graph.nodes {
        if !is_sprout_overdue(node, now) {
            continue;
        }
        node.status = NodeStatus::Sprout(SproutStatus::Expired);
        node.updated_at = Some(iso_at(now));
        out.push(node.clone());
    }
    out
}

fn log_expired(paths: &RootsPaths, rows: &[NodeRecord]) -> RootsResult<()> {
    for node in rows {
        let mut extra = Map::new();
        extra.insert("kind".into(), Value::String("sprout".into()));
        if let Some(expires) = &node.expires_at {
            extra.insert("expiresAt".into(), Value::String(expires.clone()));
        }
        append_event(
            paths,
            &make_event(
                "expire",
                "roots",
                EventFields {
                    node: Some(node.id.clone()),
                    extra,
                    ..Default::default()
                },
            ),
        )?;
    }
    Ok(())
}

pub fn expire_sprouts(
    paths: &RootsPaths,
    now: Option<OffsetDateTime>,
) -> RootsResult<Vec<NodeRecord>> {
    let now = now.unwrap_or_else(OffsetDateTime::now_utc);
    let expired = update_graph(paths, |graph| {
        let rows = expire_in_graph(graph, now);
        Ok((!rows.is_empty(), rows))
    })?;
    log_expired(paths, &expired)?;
    Ok(expired)
}

/// Permanently reject one open sprout.  This is the mutation behind the
/// `reject s-…` command and the `tend` card; putting it here keeps the event,
/// expiry, and graph-lock order identical for both callers.
pub fn reject_sprout(
    paths: &RootsPaths,
    by: &str,
    raw_id: &str,
    reason: Option<&str>,
    now: Option<OffsetDateTime>,
) -> RootsResult<NodeRecord> {
    let id = raw_id.trim().to_lowercase();
    if crate::ids::prefix_of(&id) != Some("s") {
        return Err(RootsError::usage(format!(
            "expected a proposal (p-xxxx) or sprout (s-xxxx) id, got \"{raw_id}\""
        )));
    }
    let now = now.unwrap_or_else(OffsetDateTime::now_utc);
    expire_sprouts(paths, Some(now))?;
    let why = reason
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);
    let node = update_graph(paths, |graph| {
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
    if let Some(reason) = why {
        extra.insert("reason".to_owned(), Value::String(reason));
    }
    append_event(
        paths,
        &make_event(
            "reject",
            by,
            EventFields {
                node: Some(id),
                extra,
                ..Default::default()
            },
        ),
    )?;
    Ok(node)
}

pub fn file_sprout(paths: &RootsPaths, input: SproutInput<'_>) -> RootsResult<SproutResult> {
    if !input.by.starts_with("agent:") {
        return Err(RootsError::guard("only agents file sprouts"));
    }
    let now = input.now.unwrap_or_else(OffsetDateTime::now_utc);
    let config = load_config(paths)?;
    require_tier(&config, 2, "`luuna-roots sprout`", None)?;
    let statement = check_sprout_statement(input.statement)?;
    let body = check_body(input.body)?;
    if let Some(node) = find_duplicate(paths, &read_graph(paths)?, &statement, now)? {
        return Err(duplicate_error(&node));
    }
    let text = sprout_text(&statement, &body);
    let (node, file, expired) = update_graph(paths, |graph| {
        let expired = expire_in_graph(graph, now);
        if let Some(node) = find_duplicate(paths, graph, &statement, now)? {
            return Err(duplicate_error(&node));
        }
        let open = open_sprouts(graph, now).len() as u64;
        if open >= config.limits.sprouts {
            let mut detail = Map::new();
            detail.insert("open".into(), Value::Number(open.into()));
            detail.insert("cap".into(), Value::Number(config.limits.sprouts.into()));
            return Err(RootsError::validation(format!("{open} sprout{} open (limit {}, limits.sprouts); wait for the human to adopt or reject them in `luuna-roots tend`", if open == 1 { " is" } else { "s are" }, config.limits.sprouts)).with_detail(detail));
        }
        let created = iso_at(now);
        let node = NodeRecord {
            record_type: "node".into(),
            id: generate_id(IdPrefix::Sprout, &taken_hex(paths, graph)?),
            kind: NodeKind::Sprout,
            slug: choose_slug(graph, &statement, input.slug)?,
            status: NodeStatus::Sprout(SproutStatus::Open),
            author: input.by.to_owned(),
            created_at: created.clone(),
            updated_at: Some(created),
            tier: None,
            expires_at: Some(expires_at(now, config.limits.sprout_ttl_days)),
            decided_by: None,
            decided_at: None,
            decision_reason: None,
            endorses: None,
        };
        let file = canonical_node_dir(paths, &node).join(prose_file_name(NodeKind::Sprout));
        agent_write_exclusive(paths, &file, text.as_bytes())?;
        graph.nodes.push(node.clone());
        Ok((true, (node, file, expired)))
    })?;
    log_expired(paths, &expired)?;
    let mut extra = Map::new();
    extra.insert("slug".into(), Value::String(node.slug.clone()));
    extra.insert("hash".into(), Value::String(content_hash(&text)));
    if let Some(expires) = &node.expires_at {
        extra.insert("expiresAt".into(), Value::String(expires.clone()));
    }
    append_event(
        paths,
        &make_event(
            "sprout",
            input.by,
            EventFields {
                node: Some(node.id.clone()),
                extra,
                ..Default::default()
            },
        ),
    )?;
    Ok(SproutResult {
        node,
        file,
        statement,
        expired,
    })
}
