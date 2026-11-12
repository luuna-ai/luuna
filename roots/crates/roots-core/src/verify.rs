//! Read-only integrity checks for `luuna-roots verify`.
//!
//! The regular store readers intentionally tolerate malformed JSONL. Verify is
//! the exception: it scans raw rows with line numbers so it can report damage
//! without repairing, compacting, or otherwise changing user data.

use crate::actor::{ActorKind, Environment, parse_actor};
use crate::boundary::{is_inside, real_or_self};
use crate::errors::RootsResult;
use crate::graph::{find_node, graph_from_records};
use crate::ids::hex_of;
use crate::integrations::{linked_issues, read_missions_issues};
use crate::model::{
    EdgeRecord, EdgeRel, Endorsement, Graph, GraphRecord, IdeaStatus, NodeKind, NodeRecord,
    NodeStatus, SproutStatus,
};
use crate::paths::{RootsPaths, prose_file_name};
use crate::prose::{
    NodeDirs, content_hash, dir_candidates, dir_hex, dir_name_for, locate_node_dir, scan_node_dirs,
};
use serde::Serialize;
use serde_json::{Map, Value};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const IGNORED_NAMES: &[&str] = &[".DS_Store", ".gitkeep"];
const GIT_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_GIT_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warning,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct VerifyIssue {
    pub code: String,
    pub severity: Severity,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub edge: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
}

#[must_use]
pub fn issue(code: &str, severity: Severity, message: impl Into<String>) -> VerifyIssue {
    VerifyIssue {
        code: code.to_owned(),
        severity,
        message: message.into(),
        file: None,
        line: None,
        node: None,
        edge: None,
        id: None,
    }
}

#[derive(Clone, Debug)]
pub struct JsonlRow {
    pub line: usize,
    pub value: Map<String, Value>,
}

#[derive(Clone, Debug)]
pub struct VerifyEvent {
    pub event_type: Option<String>,
    pub by: Option<String>,
    pub node: Option<String>,
    pub hash: Option<String>,
}

#[derive(Clone, Debug)]
pub struct VerifyRow {
    pub id: String,
    pub value: Map<String, Value>,
}

#[derive(Clone, Debug)]
pub struct CommitInfo {
    pub sha: String,
    pub author_name: String,
    pub author_email: String,
    pub committer_name: String,
    pub committer_email: String,
    pub message: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FileState {
    Clean,
    Modified,
    Untracked,
}

#[derive(Clone, Debug)]
pub struct GitProbe {
    pub available: bool,
    root: PathBuf,
    environment: Environment,
}

/// Conservative identities that make a commit non-human for ledger purposes.
pub const AGENT_NAMES: &[&str] = &[
    "claude",
    "claude code",
    "codex",
    "copilot",
    "devin",
    "aider",
    "sapling",
    "warren",
];

/// Git invocation used only for read probes. It passes positional paths after
/// `--`, uses no shell, observes a timeout, and inherits precisely the
/// injected process environment.
fn git(root: &Path, environment: &Environment, args: &[&str]) -> Option<String> {
    let mut child = Command::new("git")
        .args(args)
        .current_dir(root)
        .env_clear()
        .envs(environment)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let stdout = child.stdout.take()?;
    let reader = thread::spawn(move || -> std::io::Result<(Vec<u8>, bool)> {
        let mut handle = stdout;
        let mut chunk = [0_u8; 8192];
        let mut output = Vec::new();
        let mut too_large = false;
        loop {
            let read = handle.read(&mut chunk)?;
            if read == 0 {
                break;
            }
            if output.len().saturating_add(read) <= MAX_GIT_BYTES {
                output.extend_from_slice(&chunk[..read]);
            } else {
                too_large = true;
            }
        }
        Ok((output, too_large))
    });
    let started = Instant::now();
    let status = loop {
        match child.try_wait().ok()? {
            Some(status) => break status,
            None if started.elapsed() >= GIT_TIMEOUT => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = reader.join();
                return None;
            }
            None => thread::sleep(Duration::from_millis(10)),
        }
    };
    let (output, too_large) = reader.join().ok()?.ok()?;
    if !status.success() || too_large {
        return None;
    }
    Some(String::from_utf8_lossy(&output).into_owned())
}

impl GitProbe {
    #[must_use]
    pub fn unavailable() -> Self {
        Self {
            available: false,
            root: PathBuf::new(),
            environment: Environment::new(),
        }
    }

    #[must_use]
    pub fn new(root: &Path, environment: &Environment) -> Self {
        let available = git(root, environment, &["rev-parse", "--is-inside-work-tree"])
            .is_some_and(|output| output.trim() == "true");
        Self {
            available,
            root: root.to_path_buf(),
            environment: environment.clone(),
        }
    }

    fn relative(&self, file: &Path) -> String {
        file.strip_prefix(&self.root)
            .unwrap_or(file)
            .display()
            .to_string()
    }

    #[must_use]
    pub fn file_state(&self, file: &Path) -> FileState {
        if !self.available {
            return FileState::Untracked;
        }
        let relative = self.relative(file);
        let tracked = git(
            &self.root,
            &self.environment,
            &["ls-files", "--error-unmatch", "--", &relative],
        )
        .is_some();
        if !tracked {
            return FileState::Untracked;
        }
        let status = git(
            &self.root,
            &self.environment,
            &["status", "--porcelain=v1", "--", &relative],
        );
        if status.is_some_and(|output| output.trim().is_empty()) {
            FileState::Clean
        } else {
            FileState::Modified
        }
    }

    #[must_use]
    pub fn last_commit(&self, file: &Path) -> Option<CommitInfo> {
        if !self.available {
            return None;
        }
        let relative = self.relative(file);
        let output = git(
            &self.root,
            &self.environment,
            &[
                "log",
                "-1",
                "--format=%H%x00%an%x00%ae%x00%cn%x00%ce%x00%B",
                "--",
                &relative,
            ],
        )?;
        if output.trim().is_empty() {
            return None;
        }
        let mut fields = output.split('\0');
        Some(CommitInfo {
            sha: fields.next().unwrap_or_default().to_owned(),
            author_name: fields.next().unwrap_or_default().to_owned(),
            author_email: fields.next().unwrap_or_default().to_owned(),
            committer_name: fields.next().unwrap_or_default().to_owned(),
            committer_email: fields.next().unwrap_or_default().to_owned(),
            message: fields.next().unwrap_or_default().to_owned(),
        })
    }
}

fn agent_name(name: &str) -> bool {
    let name = name.trim().to_ascii_lowercase();
    name.ends_with("[bot]") || AGENT_NAMES.contains(&name.as_str())
}

fn agent_email(email: &str) -> bool {
    let email = email.trim().to_ascii_lowercase();
    email == "noreply@anthropic.com" || email.contains("[bot]@")
}

fn split_ident(value: &str) -> (&str, &str) {
    let value = value.trim();
    let Some(open) = value.rfind('<') else {
        return (value, "");
    };
    let Some(close) = value.strip_suffix('>') else {
        return (value, "");
    };
    (close[..open].trim_end(), close[open + 1..].trim())
}

/// Why a commit is agent-authored, or `None` for a human-authored commit.
#[must_use]
pub fn is_agent_commit(commit: &CommitInfo) -> Option<String> {
    if agent_name(&commit.author_name) || agent_email(&commit.author_email) {
        return Some(format!(
            "authored by {} <{}>",
            commit.author_name, commit.author_email
        ));
    }
    if agent_name(&commit.committer_name) || agent_email(&commit.committer_email) {
        return Some(format!(
            "committed by {} <{}>",
            commit.committer_name, commit.committer_email
        ));
    }
    for line in commit.message.lines() {
        let Some((prefix, value)) = line.split_once(':') else {
            continue;
        };
        if !prefix.eq_ignore_ascii_case("co-authored-by") || value.trim().is_empty() {
            continue;
        }
        let identity = value.trim();
        let (name, email) = split_ident(identity);
        if agent_name(name) || agent_email(email) {
            return Some(format!("co-authored by {identity}"));
        }
    }
    None
}

#[derive(Clone, Debug)]
pub struct VerifyContext {
    pub paths: RootsPaths,
    pub graph: Graph,
    pub dirs: NodeDirs,
    pub events: Vec<VerifyEvent>,
    pub proposals: Vec<VerifyRow>,
    pub questions: Vec<VerifyRow>,
    pub git: GitProbe,
    pub read_issues: Vec<VerifyIssue>,
}

pub type VerifyCheck = fn(&VerifyContext) -> RootsResult<Vec<VerifyIssue>>;

fn relative(paths: &RootsPaths, file: &Path) -> String {
    file.strip_prefix(&paths.root)
        .unwrap_or(file)
        .display()
        .to_string()
}

fn preview(text: &str) -> String {
    let mut characters = text.chars();
    let first = characters.by_ref().take(40).collect::<String>();
    if characters.next().is_some() {
        let shortened = first.chars().take(39).collect::<String>();
        format!("{shortened}…")
    } else {
        first
    }
}

/// Parse JSONL while preserving 1-based line numbers and errors. Unlike the
/// regular tolerant reader, malformed/non-object rows become findings.
pub fn scan_jsonl(
    file: &Path,
    rel: &str,
    issues: &mut Vec<VerifyIssue>,
) -> RootsResult<Vec<JsonlRow>> {
    if !file.exists() {
        return Ok(Vec::new());
    }
    let content = fs::read_to_string(file)
        .map_err(|error| crate::errors::RootsError::error(error.to_string()))?;
    let mut rows = Vec::new();
    for (index, raw) in content.split('\n').enumerate() {
        let text = raw.trim();
        if text.is_empty() {
            continue;
        }
        match serde_json::from_str::<Value>(text) {
            Ok(Value::Object(value)) => rows.push(JsonlRow {
                line: index + 1,
                value,
            }),
            _ => {
                let mut found = issue(
                    "jsonl.parse",
                    Severity::Error,
                    format!(
                        "{rel}:{} is not a JSON object: {}",
                        index + 1,
                        preview(text)
                    ),
                );
                found.file = Some(rel.to_owned());
                found.line = Some(index + 1);
                issues.push(found);
            }
        }
    }
    Ok(rows)
}

fn nonempty(map: &Map<String, Value>, key: &str) -> Option<String> {
    map.get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn optional_string(map: &Map<String, Value>, key: &str) -> Option<String> {
    map.get(key).and_then(Value::as_str).map(ToOwned::to_owned)
}

fn idea_status(value: &str) -> Option<IdeaStatus> {
    match value {
        "planted" => Some(IdeaStatus::Planted),
        "shaping" => Some(IdeaStatus::Shaping),
        "committed" => Some(IdeaStatus::Committed),
        "built" => Some(IdeaStatus::Built),
        "composted" => Some(IdeaStatus::Composted),
        _ => None,
    }
}

fn sprout_status(value: &str) -> Option<SproutStatus> {
    match value {
        "open" => Some(SproutStatus::Open),
        "adopted" => Some(SproutStatus::Adopted),
        "rejected" => Some(SproutStatus::Rejected),
        "expired" => Some(SproutStatus::Expired),
        _ => None,
    }
}

fn edge_rel(value: &str) -> Option<EdgeRel> {
    match value {
        "serves" => Some(EdgeRel::Serves),
        "tension" => Some(EdgeRel::Tension),
        "replaces" => Some(EdgeRel::Replaces),
        "derives" => Some(EdgeRel::Derives),
        _ => None,
    }
}

fn node_problem(value: &Map<String, Value>) -> Option<String> {
    if ["id", "slug", "author", "createdAt"]
        .iter()
        .any(|key| nonempty(value, key).is_none())
    {
        return Some("node needs id, slug, author and createdAt".to_owned());
    }
    let kind = value.get("kind").and_then(Value::as_str);
    let Some(kind) = kind else {
        return Some(format!(
            "node {} has unknown kind \"{}\"",
            value.get("id").and_then(Value::as_str).unwrap_or_default(),
            js_value(value.get("kind"))
        ));
    };
    if !matches!(kind, "idea" | "sprout") {
        return Some(format!(
            "node {} has unknown kind \"{kind}\"",
            value.get("id").and_then(Value::as_str).unwrap_or_default()
        ));
    }
    let status = value
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let valid = if kind == "idea" {
        idea_status(status).is_some()
    } else {
        sprout_status(status).is_some()
    };
    (!valid).then(|| {
        format!(
            "{kind} {} has invalid status \"{}\"",
            nonempty(value, "id").unwrap_or_default(),
            js_value(value.get("status"))
        )
    })
}

fn edge_problem(value: &Map<String, Value>) -> Option<String> {
    if ["id", "from", "to", "by"]
        .iter()
        .any(|key| nonempty(value, key).is_none())
    {
        return Some("edge needs id, from, to and by".to_owned());
    }
    let rel = value.get("rel").and_then(Value::as_str).unwrap_or_default();
    edge_rel(rel).is_none().then(|| {
        format!(
            "edge {} has unknown rel \"{}\"",
            nonempty(value, "id").unwrap_or_default(),
            js_value(value.get("rel"))
        )
    })
}

fn graph_problem(value: &Map<String, Value>) -> Option<String> {
    match value.get("type").and_then(Value::as_str) {
        Some("node") => node_problem(value),
        Some("edge") => edge_problem(value),
        _ => Some(format!(
            "unknown record type \"{}\"",
            js_value(value.get("type"))
        )),
    }
}

fn endorsement(value: &Map<String, Value>) -> Option<Endorsement> {
    let value = value.get("endorses")?.as_object()?;
    Some(Endorsement {
        sprout: value.get("sprout")?.as_str()?.to_owned(),
        hash: value.get("hash")?.as_str()?.to_owned(),
    })
}

fn parse_graph_record(value: &Map<String, Value>) -> GraphRecord {
    if value.get("type").and_then(Value::as_str) == Some("node") {
        let kind_text = value
            .get("kind")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let kind = if kind_text == "idea" {
            NodeKind::Idea
        } else {
            NodeKind::Sprout
        };
        let status_text = value
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let status = match kind {
            NodeKind::Idea => {
                NodeStatus::Idea(idea_status(status_text).expect("validated idea status"))
            }
            NodeKind::Sprout => {
                NodeStatus::Sprout(sprout_status(status_text).expect("validated sprout status"))
            }
        };
        return GraphRecord::Node(NodeRecord {
            record_type: "node".to_owned(),
            id: nonempty(value, "id").expect("validated node id"),
            kind,
            slug: nonempty(value, "slug").expect("validated node slug"),
            status,
            author: nonempty(value, "author").expect("validated node author"),
            created_at: nonempty(value, "createdAt").expect("validated node timestamp"),
            updated_at: optional_string(value, "updatedAt"),
            tier: value
                .get("tier")
                .and_then(Value::as_u64)
                .and_then(|tier| u8::try_from(tier).ok()),
            expires_at: optional_string(value, "expiresAt"),
            decided_by: optional_string(value, "decidedBy"),
            decided_at: optional_string(value, "decidedAt"),
            decision_reason: optional_string(value, "decisionReason"),
            endorses: endorsement(value),
        });
    }
    GraphRecord::Edge(EdgeRecord {
        record_type: "edge".to_owned(),
        id: nonempty(value, "id").expect("validated edge id"),
        from: nonempty(value, "from").expect("validated edge source"),
        to: nonempty(value, "to").expect("validated edge target"),
        rel: edge_rel(value.get("rel").and_then(Value::as_str).unwrap_or_default())
            .expect("validated edge relation"),
        by: nonempty(value, "by").expect("validated edge actor"),
        proposed_by: optional_string(value, "proposedBy"),
        proposal: optional_string(value, "proposal"),
        created_at: optional_string(value, "createdAt").unwrap_or_default(),
    })
}

fn read_graph_checked(paths: &RootsPaths, issues: &mut Vec<VerifyIssue>) -> RootsResult<Graph> {
    let file = relative(paths, &paths.graph);
    let valid = scan_jsonl(&paths.graph, &file, issues)?
        .into_iter()
        .filter_map(|row| match graph_problem(&row.value) {
            Some(problem) => {
                let mut found = issue("record.invalid", Severity::Error, problem);
                found.file = Some(file.clone());
                found.line = Some(row.line);
                issues.push(found);
                None
            }
            None => Some(parse_graph_record(&row.value)),
        })
        .collect();
    Ok(graph_from_records(valid))
}

fn read_events_checked(
    paths: &RootsPaths,
    issues: &mut Vec<VerifyIssue>,
) -> RootsResult<Vec<VerifyEvent>> {
    let file = relative(paths, &paths.events);
    let events = scan_jsonl(&paths.events, &file, issues)?
        .into_iter()
        .map(|row| {
            if nonempty(&row.value, "by").is_none() {
                let mut found = issue(
                    "event.no-by",
                    Severity::Error,
                    format!("event \"{}\" has no `by`", js_value(row.value.get("type"))),
                );
                found.file = Some(file.clone());
                found.line = Some(row.line);
                issues.push(found);
            }
            VerifyEvent {
                event_type: optional_string(&row.value, "type"),
                by: optional_string(&row.value, "by"),
                node: optional_string(&row.value, "node"),
                hash: optional_string(&row.value, "hash"),
            }
        })
        .collect::<Vec<_>>();
    Ok(events)
}

fn read_table_checked(
    paths: &RootsPaths,
    table: &Path,
    issues: &mut Vec<VerifyIssue>,
) -> RootsResult<Vec<VerifyRow>> {
    let file = relative(paths, table);
    let mut rows = Vec::new();
    let mut positions = HashMap::new();
    for row in scan_jsonl(table, &file, issues)? {
        let Some(id) = nonempty(&row.value, "id") else {
            continue;
        };
        let record = VerifyRow {
            id: id.clone(),
            value: row.value,
        };
        if let Some(index) = positions.get(&id).copied() {
            rows[index] = record;
        } else {
            positions.insert(id, rows.len());
            rows.push(record);
        }
    }
    Ok(rows)
}

/// Load all data needed for verification without reconciling directory names
/// or modifying any store files.
pub fn load_verify_context(paths: RootsPaths, git: GitProbe) -> RootsResult<VerifyContext> {
    let mut read_issues = Vec::new();
    let graph = read_graph_checked(&paths, &mut read_issues)?;
    let dirs = scan_node_dirs(&paths)?;
    let events = read_events_checked(&paths, &mut read_issues)?;
    let proposals = read_table_checked(&paths, &paths.proposals, &mut read_issues)?;
    let questions = read_table_checked(&paths, &paths.questions, &mut read_issues)?;
    Ok(VerifyContext {
        paths,
        graph,
        dirs,
        events,
        proposals,
        questions,
        git,
        read_issues,
    })
}

fn js_value(value: Option<&Value>) -> String {
    match value {
        None => "undefined".to_owned(),
        Some(Value::Null) => "null".to_owned(),
        Some(Value::String(value)) => value.clone(),
        Some(Value::Bool(value)) => value.to_string(),
        Some(Value::Number(value)) => value.to_string(),
        Some(Value::Array(values)) => values
            .iter()
            .map(|value| js_value(Some(value)))
            .collect::<Vec<_>>()
            .join(","),
        Some(Value::Object(_)) => "[object Object]".to_owned(),
    }
}

fn actor_kind(value: Option<&Value>) -> Option<ActorKind> {
    value
        .and_then(Value::as_str)
        .and_then(parse_actor)
        .map(|actor| actor.kind)
}

fn check_authors(context: &VerifyContext) -> RootsResult<Vec<VerifyIssue>> {
    let mut output = Vec::new();
    for node in &context.graph.nodes {
        let required = match node.kind {
            NodeKind::Idea => ActorKind::Human,
            NodeKind::Sprout => ActorKind::Agent,
        };
        if parse_actor(&node.author).is_none_or(|actor| actor.kind != required) {
            let kind = node.kind.as_str();
            let want = match required {
                ActorKind::Human => "human",
                ActorKind::Agent => "agent",
                ActorKind::Roots => "roots",
            };
            let mut found = issue(
                &format!("author.{kind}"),
                Severity::Error,
                format!(
                    "{kind} {} has author \"{}\"; must be {want}:*",
                    node.id, node.author
                ),
            );
            found.node = Some(node.id.clone());
            output.push(found);
        }
    }
    for edge in &context.graph.edges {
        if parse_actor(&edge.by).is_none_or(|actor| actor.kind != ActorKind::Human) {
            let mut found = issue(
                "author.edge",
                Severity::Error,
                format!(
                    "edge {} was made by \"{}\"; only human:* makes edges",
                    edge.id, edge.by
                ),
            );
            found.edge = Some(edge.id.clone());
            output.push(found);
        }
    }
    for proposal in &context.proposals {
        let kind = actor_kind(proposal.value.get("by"));
        if !matches!(kind, Some(ActorKind::Agent | ActorKind::Roots)) {
            let mut found = issue(
                "author.proposal",
                Severity::Error,
                format!(
                    "proposal {} is by \"{}\"; must be agent:* or roots",
                    proposal.id,
                    js_value(proposal.value.get("by"))
                ),
            );
            found.id = Some(proposal.id.clone());
            output.push(found);
        }
    }
    Ok(output)
}

fn same_edge_key(left: &EdgeRecord, right: &EdgeRecord) -> bool {
    left.rel == right.rel
        && ((left.from == right.from && left.to == right.to)
            || (left.rel == EdgeRel::Tension && left.from == right.to && left.to == right.from))
}

fn would_cycle(edges: &[EdgeRecord], edge: &EdgeRecord, ignored: &str) -> bool {
    let mut seen = HashSet::new();
    let mut stack = vec![edge.to.clone()];
    while let Some(id) = stack.pop() {
        if id == edge.from {
            return true;
        }
        if !seen.insert(id.clone()) {
            continue;
        }
        for other in edges {
            if other.id != ignored && other.rel == edge.rel && other.from == id {
                stack.push(other.to.clone());
            }
        }
    }
    false
}

fn edge_structure(graph: &Graph, edge: &EdgeRecord) -> Option<String> {
    let from = find_node(graph, &edge.from)?;
    let to = find_node(graph, &edge.to)?;
    if edge.rel == EdgeRel::Derives {
        return (from.kind != NodeKind::Idea || to.kind != NodeKind::Sprout)
            .then(|| "`derives` goes from an idea (r-) to a sprout (s-)".to_owned());
    }
    if from.kind != NodeKind::Idea || to.kind != NodeKind::Idea {
        return Some(format!("`{}` on a sprout", edge.rel.as_str()));
    }
    if let Some(duplicate) = graph
        .edges
        .iter()
        .find(|other| other.id != edge.id && same_edge_key(other, edge))
    {
        return Some(format!("duplicate of {}", duplicate.id));
    }
    matches!(edge.rel, EdgeRel::Serves | EdgeRel::Replaces)
        .then(|| would_cycle(&graph.edges, edge, &edge.id))
        .filter(|cycle| *cycle)
        .map(|_| format!("`{}` cycle", edge.rel.as_str()))
}

fn check_graph(context: &VerifyContext) -> RootsResult<Vec<VerifyIssue>> {
    let mut output = Vec::new();
    for edge in &context.graph.edges {
        let message = if edge.from == edge.to {
            Some(format!("self edge on {}", edge.from))
        } else if find_node(&context.graph, &edge.from).is_none()
            || find_node(&context.graph, &edge.to).is_none()
        {
            Some(format!("endpoint missing ({} → {})", edge.from, edge.to))
        } else {
            edge_structure(&context.graph, edge).or_else(|| {
                (edge.rel == EdgeRel::Replaces
                    && find_node(&context.graph, &edge.to)
                        .is_some_and(|node| node.status != NodeStatus::Idea(IdeaStatus::Composted)))
                .then(|| {
                    format!(
                        "{} is replaced by {} but is not composted",
                        edge.to, edge.from
                    )
                })
            })
        };
        let Some(message) = message else {
            continue;
        };
        let code = if message.starts_with("endpoint missing") {
            "ref.dangling"
        } else {
            "graph.invariant"
        };
        let mut found = issue(
            code,
            Severity::Error,
            format!("edge {}: {message}", edge.id),
        );
        found.edge = Some(edge.id.clone());
        output.push(found);
    }
    Ok(output)
}

fn check_refs(context: &VerifyContext) -> RootsResult<Vec<VerifyIssue>> {
    let ids = context
        .graph
        .nodes
        .iter()
        .map(|node| node.id.as_str())
        .collect::<HashSet<_>>();
    let mut output = Vec::new();
    let mut missing = |owner: &str, reference: Option<&Value>, what: &str| {
        let Some(reference) = reference
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
        else {
            return;
        };
        if ids.contains(reference) {
            return;
        }
        let mut found = issue(
            "ref.dangling",
            Severity::Error,
            format!("{owner} {what} {reference}, which does not exist"),
        );
        found.id = Some(owner.to_owned());
        output.push(found);
    };
    for question in &context.questions {
        missing(&question.id, question.value.get("node"), "asks about");
    }
    for proposal in &context.proposals {
        missing(&proposal.id, proposal.value.get("from"), "refers to");
        missing(&proposal.id, proposal.value.get("to"), "refers to");
        if let Some(cites) = proposal.value.get("cites").and_then(Value::as_array) {
            for cite in cites {
                missing(
                    &proposal.id,
                    cite.as_object().and_then(|cite| cite.get("node")),
                    "cites",
                );
            }
        }
    }
    Ok(output)
}

fn lstat(path: &Path) -> Option<fs::Metadata> {
    fs::symlink_metadata(path).ok()
}

fn sorted_entries(path: &Path) -> RootsResult<Vec<(String, PathBuf)>> {
    let entries = fs::read_dir(path)
        .map_err(|error| crate::errors::RootsError::error(error.to_string()))?
        .filter_map(Result::ok)
        .map(|entry| {
            (
                entry.file_name().to_string_lossy().into_owned(),
                entry.path(),
            )
        })
        .collect::<Vec<_>>();
    let mut entries = entries;
    entries.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(entries)
}

fn label(node: &NodeRecord) -> String {
    format!("{} {}", node.id, node.slug)
}

fn node_dir_issues(context: &VerifyContext, node: &NodeRecord) -> RootsResult<Vec<VerifyIssue>> {
    let names = dir_candidates(&context.dirs, node);
    let directory = locate_node_dir(&context.paths, node, Some(&context.dirs))?;
    let Some(directory) = directory else {
        let mut found = issue(
            "dir.missing",
            Severity::Error,
            format!(
                "{} {} has no directory ({})",
                node.kind.as_str(),
                label(node),
                dir_name_for(node)
            ),
        );
        found.node = Some(node.id.clone());
        return Ok(vec![found]);
    };
    let mut output = Vec::new();
    if names.len() > 1 {
        let mut found = issue(
            "dir.duplicate",
            Severity::Warning,
            format!(
                "{} has {} directories: {}",
                label(node),
                names.len(),
                names.join(", ")
            ),
        );
        found.node = Some(node.id.clone());
        output.push(found);
    }
    let file = directory.join(prose_file_name(node.kind));
    if !file.exists() {
        let mut found = issue(
            "dir.missing",
            Severity::Error,
            format!(
                "{}: {} is missing",
                label(node),
                relative(&context.paths, &file)
            ),
        );
        found.node = Some(node.id.clone());
        found.file = Some(relative(&context.paths, &file));
        output.push(found);
    }
    Ok(output)
}

fn orphan_dirs(
    context: &VerifyContext,
    parent: &Path,
    kind: &str,
    hexes: &HashSet<String>,
) -> RootsResult<Vec<VerifyIssue>> {
    if !parent.exists() {
        return Ok(Vec::new());
    }
    let mut output = Vec::new();
    for (name, path) in sorted_entries(parent)? {
        if IGNORED_NAMES.contains(&name.as_str()) {
            continue;
        }
        if dir_hex(&name).is_some_and(|hex| hexes.contains(hex)) {
            continue;
        }
        let file = relative(&context.paths, &path);
        let mut found = issue(
            "dir.orphan",
            Severity::Warning,
            format!("{file} matches no {kind}"),
        );
        found.file = Some(file);
        output.push(found);
    }
    Ok(output)
}

fn check_dirs(context: &VerifyContext) -> RootsResult<Vec<VerifyIssue>> {
    let mut output = Vec::new();
    for node in &context.graph.nodes {
        output.extend(node_dir_issues(context, node)?);
    }
    for (kind, parent, description) in [
        (NodeKind::Sprout, &context.paths.sprouts, "sprout"),
        (NodeKind::Idea, &context.paths.notes, "idea"),
    ] {
        let hexes = context
            .graph
            .nodes
            .iter()
            .filter(|node| node.kind == kind)
            .map(|node| hex_of(&node.id).to_owned())
            .collect::<HashSet<_>>();
        output.extend(orphan_dirs(context, parent, description, &hexes)?);
    }
    Ok(output)
}

struct Walk<'a> {
    context: &'a VerifyContext,
    real_human: PathBuf,
    output: Vec<VerifyIssue>,
}

fn check_symlink(walk: &mut Walk<'_>, path: &Path) {
    let target = real_or_self(path);
    if path.exists() && is_inside(&walk.real_human, &target) {
        return;
    }
    let why = if path.exists() {
        format!("points outside .luuna/roots/human/ ({})", target.display())
    } else {
        "is dangling".to_owned()
    };
    let file = relative(&walk.context.paths, path);
    let mut found = issue(
        "boundary.symlink",
        Severity::Error,
        format!("{file} is a symlink that {why}"),
    );
    found.file = Some(file);
    walk.output.push(found);
}

fn all_allowed(_: &str, _: bool) -> bool {
    true
}

fn node_entry(name: &str, directory: bool) -> bool {
    (name == "idea.md" && !directory) || (name == "assets" && directory)
}

fn walk_tree(
    walk: &mut Walk<'_>,
    directory: &Path,
    allowed: fn(&str, bool) -> bool,
) -> RootsResult<()> {
    for (name, path) in sorted_entries(directory)? {
        let Some(metadata) = lstat(&path) else {
            continue;
        };
        if IGNORED_NAMES.contains(&name.as_str()) {
            continue;
        }
        if metadata.file_type().is_symlink() {
            check_symlink(walk, &path);
            continue;
        }
        if name.starts_with('.') {
            let file = relative(&walk.context.paths, &path);
            let mut found = issue(
                "boundary.hidden",
                Severity::Warning,
                format!("{file}: hidden file under .luuna/roots/human/"),
            );
            found.file = Some(file);
            walk.output.push(found);
            continue;
        }
        if !allowed(&name, metadata.is_dir()) {
            let file = relative(&walk.context.paths, &path);
            let mut found = issue(
                "boundary.stray",
                Severity::Error,
                format!("{file} is not an idea.md or assets/ of a known idea"),
            );
            found.file = Some(file);
            walk.output.push(found);
            continue;
        }
        if metadata.is_dir() {
            walk_tree(walk, &path, all_allowed)?;
        }
    }
    Ok(())
}

fn check_human_tree(context: &VerifyContext) -> RootsResult<Vec<VerifyIssue>> {
    let human = &context.paths.human;
    let Some(metadata) = lstat(human) else {
        return Ok(Vec::new());
    };
    if metadata.file_type().is_symlink() {
        let file = relative(&context.paths, human);
        let mut found = issue(
            "boundary.symlink",
            Severity::Error,
            ".luuna/roots/human is a symlink",
        );
        found.file = Some(file);
        return Ok(vec![found]);
    }
    let hexes = context
        .graph
        .nodes
        .iter()
        .filter(|node| node.kind == NodeKind::Idea)
        .map(|node| hex_of(&node.id).to_owned())
        .collect::<HashSet<_>>();
    let mut walk = Walk {
        context,
        real_human: real_or_self(human),
        output: Vec::new(),
    };
    for (name, path) in sorted_entries(human)? {
        let Some(entry) = lstat(&path) else {
            continue;
        };
        if IGNORED_NAMES.contains(&name.as_str()) {
            continue;
        }
        if entry.file_type().is_symlink() {
            check_symlink(&mut walk, &path);
            continue;
        }
        if entry.is_dir() && dir_hex(&name).is_some_and(|hex| hexes.contains(hex)) {
            walk_tree(&mut walk, &path, node_entry)?;
        } else if name.starts_with('.') {
            let file = relative(&context.paths, &path);
            let mut found = issue(
                "boundary.hidden",
                Severity::Warning,
                format!("{file}: hidden file under .luuna/roots/human/"),
            );
            found.file = Some(file);
            walk.output.push(found);
        } else {
            let file = relative(&context.paths, &path);
            let mut found = issue(
                "boundary.stray",
                Severity::Error,
                format!("{file} is not the directory of a known idea"),
            );
            found.file = Some(file);
            walk.output.push(found);
        }
    }
    Ok(walk.output)
}

/// The last human `plant`/`session.end` hash for an idea, in file order.
#[must_use]
pub fn last_trusted_hash(events: &[VerifyEvent], id: &str) -> Option<String> {
    let mut hash = None;
    for event in events {
        if event.node.as_deref() != Some(id)
            || !matches!(event.event_type.as_deref(), Some("plant" | "session.end"))
            || !event
                .by
                .as_deref()
                .is_some_and(|actor| actor.starts_with("human:"))
        {
            continue;
        }
        if let Some(value) = &event.hash {
            hash = Some(value.clone());
        }
    }
    hash
}

fn ledger_issue(
    context: &VerifyContext,
    node: &NodeRecord,
    file: &Path,
) -> RootsResult<Option<VerifyIssue>> {
    let relative_file = relative(&context.paths, file);
    let trusted = last_trusted_hash(&context.events, &node.id);
    if trusted.is_none() {
        let mut found = issue(
            "ledger.missing",
            Severity::Warning,
            format!(
                "{}: no recorded hash (plant or think session) to check idea.md against",
                label(node)
            ),
        );
        found.node = Some(node.id.clone());
        found.file = Some(relative_file);
        return Ok(Some(found));
    }
    let text = fs::read_to_string(file)
        .map_err(|error| crate::errors::RootsError::error(error.to_string()))?;
    if content_hash(&text) == trusted.expect("checked trusted hash") {
        return Ok(None);
    }
    let mut found = if !context.git.available {
        issue(
            "ledger.unverifiable",
            Severity::Warning,
            format!(
                "{}: idea.md changed since its last think session, and git is unavailable to check who changed it",
                label(node)
            ),
        )
    } else if context.git.file_state(file) != FileState::Clean {
        issue(
            "ledger.untrusted-edit",
            Severity::Error,
            format!(
                "{}: idea.md changed outside a think session and the change is not committed. If you wrote it, run `luuna-roots think {}` or commit it; otherwise restore it (git checkout)",
                label(node),
                node.id
            ),
        )
    } else {
        let commit = context.git.last_commit(file);
        let agent = commit
            .as_ref()
            .and_then(is_agent_commit)
            .unwrap_or_else(|| "no commit found".to_owned());
        if commit
            .as_ref()
            .is_some_and(|commit| is_agent_commit(commit).is_none())
        {
            return Ok(None);
        }
        issue(
            "ledger.agent-commit",
            Severity::Error,
            format!(
                "{}: idea.md changed outside a think session in commit {} ({}); agents never write human ideas",
                label(node),
                commit
                    .as_ref()
                    .map(|commit| commit.sha.chars().take(8).collect::<String>())
                    .unwrap_or_else(|| "?".to_owned()),
                agent
            ),
        )
    };
    found.node = Some(node.id.clone());
    found.file = Some(relative_file);
    Ok(Some(found))
}

fn check_ledger(context: &VerifyContext) -> RootsResult<Vec<VerifyIssue>> {
    let mut output = Vec::new();
    for node in context
        .graph
        .nodes
        .iter()
        .filter(|node| node.kind == NodeKind::Idea)
    {
        let directory = locate_node_dir(&context.paths, node, Some(&context.dirs))?;
        let Some(directory) = directory else {
            continue;
        };
        let file = directory.join("idea.md");
        if !lstat(&file).is_some_and(|metadata| metadata.is_file()) {
            continue;
        }
        if let Some(found) = ledger_issue(context, node, &file)? {
            output.push(found);
        }
    }
    Ok(output)
}

fn check_seeds_built(context: &VerifyContext) -> RootsResult<Vec<VerifyIssue>> {
    let issues = read_missions_issues(&context.paths.root);
    let mut output = Vec::new();
    for node in context.graph.nodes.iter().filter(|node| {
        node.kind == NodeKind::Idea && node.status == NodeStatus::Idea(IdeaStatus::Built)
    }) {
        let open = linked_issues(&issues, &node.id)
            .into_iter()
            .filter(|issue| !issue.closed)
            .collect::<Vec<_>>();
        if open.is_empty() {
            continue;
        }
        let mut found = issue(
            "seeds.open-on-built",
            Severity::Warning,
            format!(
                "{} is built but seeds issues are open: {}",
                node.id,
                open.iter()
                    .map(|issue| issue.id.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        );
        found.node = Some(node.id.clone());
        output.push(found);
    }
    Ok(output)
}

fn check_read_issues(context: &VerifyContext) -> RootsResult<Vec<VerifyIssue>> {
    Ok(context.read_issues.clone())
}

pub const VERIFY_CHECKS: &[VerifyCheck] = &[
    check_read_issues,
    check_authors,
    check_graph,
    check_refs,
    check_dirs,
    check_human_tree,
    check_ledger,
    check_seeds_built,
];

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerifyReport {
    pub issues: Vec<VerifyIssue>,
    pub errors: usize,
    pub warnings: usize,
}

pub fn run_verify(context: &VerifyContext, checks: &[VerifyCheck]) -> RootsResult<VerifyReport> {
    let mut issues = Vec::new();
    for check in checks {
        issues.extend(check(context)?);
    }
    let errors = issues
        .iter()
        .filter(|issue| issue.severity == Severity::Error)
        .count();
    Ok(VerifyReport {
        warnings: issues.len() - errors,
        issues,
        errors,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn commit() -> CommitInfo {
        CommitInfo {
            sha: "abcdef012345".to_owned(),
            author_name: "Jaymin West".to_owned(),
            author_email: "j@x.io".to_owned(),
            committer_name: "Jaymin West".to_owned(),
            committer_email: "j@x.io".to_owned(),
            message: "human change\n".to_owned(),
        }
    }

    #[test]
    fn agent_commit_detection_is_conservative() {
        assert!(is_agent_commit(&commit()).is_none());
        assert!(
            is_agent_commit(&CommitInfo {
                message: "x\n\nCo-Authored-By: Claude Opus <noreply@anthropic.com>\n".to_owned(),
                ..commit()
            })
            .is_some()
        );
        assert!(
            is_agent_commit(&CommitInfo {
                author_name: "Claude".to_owned(),
                ..commit()
            })
            .is_some()
        );
        assert!(
            is_agent_commit(&CommitInfo {
                author_name: "Claude Dupont".to_owned(),
                ..commit()
            })
            .is_none()
        );
    }

    #[test]
    fn trusted_hash_ignores_scans_and_agent_events() {
        let events = vec![
            VerifyEvent {
                event_type: Some("plant".to_owned()),
                by: Some("human:a".to_owned()),
                node: Some("r-a1b2".to_owned()),
                hash: Some("h1".to_owned()),
            },
            VerifyEvent {
                event_type: Some("scan".to_owned()),
                by: Some("human:a".to_owned()),
                node: Some("r-a1b2".to_owned()),
                hash: Some("h2".to_owned()),
            },
            VerifyEvent {
                event_type: Some("session.end".to_owned()),
                by: Some("agent:x".to_owned()),
                node: Some("r-a1b2".to_owned()),
                hash: Some("h3".to_owned()),
            },
        ];
        assert_eq!(last_trusted_hash(&events, "r-a1b2"), Some("h1".to_owned()));
    }
}
