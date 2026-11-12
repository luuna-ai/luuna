//! Imperative driver for one human `think` session.
//!
//! The reducer in [`super::session`] owns navigation; this module owns the
//! deliberately auditable side effects around it: questions/events, prose
//! watching, terminal cleanup, mention filing, status transition, and the
//! opt-in agent command boundary.  Keeping the terminal and watcher behind
//! small traits makes all of it usable with the in-memory terminal in tests.

use super::prompt_contract::{AGENT_INSTRUCTION, DELEGATE_INSTRUCTION, PROPOSE_INSTRUCTION};
use super::screen::{
    ScreenEdge, ScreenFinding, ScreenGuidance, ScreenQuestion, ScreenView, render_screen,
};
use super::session::{
    EndReason, SessionEffect, SessionInput, SessionKey, SessionState, SessionTally,
    current_question, initial_state, parse_keys, step, tally,
};
use super::spinner::{StatusLine, start_status};
use super::terminal::{Terminal, frame};
use super::watch::{FileWatcher, WatchOptions};
use roots_core::actor::Environment;
use roots_core::attention::{due_questions, node_activity};
use roots_core::blame::{DiffSpan, line_hash, split_lines};
use roots_core::claims::read_claim;
use roots_core::color::Colors;
use roots_core::config::RootsConfig;
use roots_core::errors::{RootsError, RootsResult};
use roots_core::events::{EventFields, append_event, make_event, read_events};
use roots_core::graph::{find_node, read_graph, update_graph};
use roots_core::ids::{IdPrefix, generate_id, hex_set};
use roots_core::mentions::{
    ChangedLine, Mention, MentionFiling, MentionSkip, file_mentions, mention_targets,
};
use roots_core::model::{
    Graph, IdeaStatus, NodeRecord, NodeStatus, ProposalRecord, QuestionRecord, QuestionStatus,
};
use roots_core::paths::RootsPaths;
use roots_core::presentation::{build_context, render_context_markdown};
use roots_core::proposals::{pending_proposals, read_proposals};
use roots_core::prose::{content_hash, parse_prose, read_node_prose};
use roots_core::questions::{open_agent_questions, read_questions};
use roots_core::rules::{RuleContext, new_candidates, rule_actor, rule_of};
use roots_core::serde::Serialize;
use roots_core::serde_json::{Map, Value, to_value};
use roots_core::store::update_table;
use roots_core::tier::effective_tier;
use roots_core::time::iso_at;
use roots_core::time_crate::{Duration as TimeDuration, OffsetDateTime};
use std::collections::{BTreeSet, HashSet};
use std::fs;
use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

#[cfg(unix)]
use std::os::unix::process::CommandExt;

#[cfg(unix)]
unsafe extern "C" {
    fn kill(pid: i32, signal: i32) -> i32;
}

/// Inputs supplied by the CLI command and reusable by future interactive
/// commands (notably `tend`/`flow`).
pub struct ThinkDeps<'a> {
    pub paths: RootsPaths,
    pub config: RootsConfig,
    pub node: NodeRecord,
    pub by: String,
    pub terminal: &'a mut dyn Terminal,
    pub colors: Colors,
    pub editor_hint: String,
    pub watch: WatchOptions,
    /// Full injected process environment used for an explicitly configured
    /// agent command. It is filtered before the child starts.
    pub environment: Environment,
    pub guidance: Option<ScreenGuidance>,
    /// Optional process-facing status line used while an explicitly
    /// configured agent command is running.
    pub agent_status: Option<AgentStatus>,
    /// When supplied, configured agent phases are detached and reported to
    /// this sink instead of blocking the interactive session. This is the
    /// source `flow` contract; `think` leaves it unset and awaits them.
    pub background: Option<BackgroundReporter>,
    /// Test-only override; production uses `agent.timeoutSeconds`.
    pub agent_timeout_ms: Option<u64>,
}

/// Detached status-line configuration. The writer deliberately owns no `Io`
/// borrow, so the terminal can be exclusively borrowed for the session while
/// an agent phase updates stderr.
#[derive(Clone)]
pub struct AgentStatus {
    pub write: Arc<dyn Fn(String) + Send + Sync>,
    pub animate: bool,
    pub columns: Option<Arc<dyn Fn() -> usize + Send + Sync>>,
}

/// One background agent phase handed to a higher-level interactive command.
/// `flow` owns the receiver and can render or await the eventual one-line
/// failure while this session continues to accept human input.
pub struct BackgroundJob {
    pub label: String,
    pub done: Receiver<Option<String>>,
}

pub type BackgroundReporter = Arc<dyn Fn(BackgroundJob) + Send + Sync>;

fn start_agent_status(deps: &ThinkDeps<'_>, text: String) -> Option<Box<dyn StatusLine>> {
    deps.agent_status.as_ref().map(|status| {
        Box::new(start_status(
            Arc::clone(&status.write),
            text,
            status.animate,
            status.columns.as_ref().map(Arc::clone),
        )) as Box<dyn StatusLine>
    })
}

fn start_background<F>(reporter: &BackgroundReporter, label: String, work: F)
where
    F: FnOnce() -> Option<String> + Send + 'static,
{
    let (sender, done) = mpsc::channel();
    thread::spawn(move || {
        let _ = sender.send(work());
    });
    reporter(BackgroundJob { label, done });
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionQuestionSummary {
    pub id: String,
    pub text: String,
    pub by: String,
    pub outcome: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusChange {
    pub from: String,
    pub to: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MentionResultSummary {
    pub filed: Vec<ProposalRecord>,
    pub skipped: Vec<MentionSkip>,
    pub expired: Vec<ProposalRecord>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MentionSummary {
    pub mentions: Vec<Mention>,
    pub result: MentionResultSummary,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSummary {
    pub mode: String,
    pub tier: u8,
    pub actor: Option<String>,
    pub status: Option<String>,
    pub error: Option<String>,
    pub asked: usize,
    pub arrived: usize,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProposalSummary {
    pub actor: String,
    pub status: String,
    pub error: Option<String>,
    pub filed: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DelegationSummary {
    pub actor: String,
    pub status: String,
    pub error: Option<String>,
    pub questions: Vec<String>,
    pub found: Vec<String>,
    pub returned: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSummary {
    pub session: String,
    pub node: String,
    pub reason: String,
    pub questions: Vec<SessionQuestionSummary>,
    pub tally: SessionTally,
    pub changed: bool,
    pub hash: String,
    pub status_change: Option<StatusChange>,
    pub mentions: Option<MentionSummary>,
    pub mention_error: Option<String>,
    pub agent: AgentSummary,
    pub proposals: Option<ProposalSummary>,
    pub delegation: Option<DelegationSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub changed_lines: Option<Vec<ChangedLine>>,
}

#[derive(Clone, Debug)]
struct AgentPhase {
    mode: &'static str,
    tier: u8,
    actor: Option<String>,
    status: Option<String>,
    error: Option<String>,
    asked: usize,
    background: bool,
}

#[derive(Clone, Debug)]
struct Live {
    session: String,
    state: SessionState,
    last_text: String,
    statement: String,
    edges: Vec<ScreenEdge>,
    agents_live: bool,
    seen: HashSet<String>,
}

#[derive(Clone, Copy, Debug)]
enum TerminalInput {
    Key(SessionKey),
    Resize,
}

fn error(problem: impl std::fmt::Display) -> RootsError {
    RootsError::error(problem.to_string())
}

fn value<T: Serialize>(item: T) -> RootsResult<Value> {
    to_value(item).map_err(error)
}

fn event_extra(question: Option<&str>, session: Option<&str>) -> Map<String, Value> {
    let mut extra = Map::new();
    if let Some(question) = question {
        extra.insert("question".to_owned(), Value::String(question.to_owned()));
    }
    if let Some(session) = session {
        extra.insert("session".to_owned(), Value::String(session.to_owned()));
    }
    extra
}

fn new_session_id(paths: &RootsPaths) -> RootsResult<String> {
    let events = read_events(paths)?;
    let taken = events
        .iter()
        .filter_map(|event| event.extra.get("session").and_then(Value::as_str))
        .collect::<Vec<_>>();
    Ok(generate_id(IdPrefix::Session, &hex_set(taken)))
}

fn is_agent(question: &QuestionRecord) -> bool {
    question.by.starts_with("agent:")
}

fn add_questions(
    paths: &RootsPaths,
    drafts: Vec<(String, String, String, Option<String>)>,
    session: &str,
    now: OffsetDateTime,
) -> RootsResult<Vec<QuestionRecord>> {
    if drafts.is_empty() {
        return Ok(Vec::new());
    }
    let at = iso_at(now);
    let created = update_table::<QuestionRecord, _, _>(&paths.questions, |mut rows| {
        let mut taken = hex_set(rows.iter().map(|question| question.id.as_str()));
        let mut made = Vec::new();
        for (node, text, by, reference) in &drafts {
            let id = generate_id(IdPrefix::Question, &taken);
            taken.extend(hex_set(std::iter::once(id.as_str())));
            let question = QuestionRecord {
                id,
                node: node.clone(),
                text: text.clone(),
                by: by.clone(),
                status: QuestionStatus::Open,
                created_at: at.clone(),
                r#ref: reference.clone(),
                answered_by: None,
                answered_at: None,
                dismissed_by: None,
                dismissed_at: None,
                delegated_by: None,
                delegated_at: None,
                findings: None,
                snoozed_until: None,
                session: Some(session.to_owned()),
            };
            rows.push(question.clone());
            made.push(question);
        }
        Ok((rows, true, made))
    })?;
    for question in &created {
        append_event(
            paths,
            &make_event(
                "ask",
                &question.by,
                EventFields {
                    at: Some(at.clone()),
                    node: Some(question.node.clone()),
                    extra: event_extra(Some(&question.id), Some(session)),
                    ..Default::default()
                },
            ),
        )?;
    }
    Ok(created)
}

/// Due questions first, then fresh rules up to the configured session cap.
pub fn prepare_questions(
    paths: &RootsPaths,
    config: &RootsConfig,
    graph: &Graph,
    node: &NodeRecord,
    session: &str,
    now: OffsetDateTime,
    agents: bool,
) -> RootsResult<Vec<QuestionRecord>> {
    let existing = read_questions(paths)?;
    let events = read_events(paths)?;
    let claim = read_claim(paths, graph, node)?;
    let activity = node_activity(&events, node);
    let lookup = |id: &str| find_node(graph, id);
    let context = RuleContext {
        node,
        text: &claim.text,
        edges: graph
            .edges
            .iter()
            .filter(|edge| edge.from == node.id || edge.to == node.id)
            .collect(),
        lookup: &lookup,
        session_number: activity.sessions + 1,
        last_touched: &activity.last_touched,
        now,
        thresholds: &config.questions,
    };
    let cap = config.limits.questions_per_session as usize;
    let due = due_questions(&existing, &node.id, now)
        .into_iter()
        .filter(|question| agents || !is_agent(question))
        .take(cap)
        .collect::<Vec<_>>();
    let remaining = cap.saturating_sub(due.len());
    let drafts = new_candidates(&context, &existing)
        .into_iter()
        .take(remaining)
        .map(|candidate| {
            (
                node.id.clone(),
                candidate.text,
                rule_actor(&candidate.rule),
                candidate.r#ref,
            )
        })
        .collect();
    let created = add_questions(paths, drafts, session, now)?;
    Ok(due.into_iter().chain(created).collect())
}

fn question_source(question: &QuestionRecord) -> String {
    rule_of(question).map(ToOwned::to_owned).unwrap_or_else(|| {
        if is_agent(question) {
            "agent".to_owned()
        } else {
            question.by.clone()
        }
    })
}

fn first_paragraph(text: &str) -> String {
    for block in text.replace("\r\n", "\n").split("\n\n") {
        let lines = block
            .lines()
            .map(str::trim)
            .filter(|line| {
                !line.is_empty()
                    && !line.starts_with('#')
                    && !line.starts_with("---")
                    && !line.starts_with("```")
                    && !line.starts_with("<!--")
            })
            .collect::<Vec<_>>();
        if !lines.is_empty() {
            return lines
                .join(" ")
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
        }
    }
    String::new()
}

fn finding_preview(paths: &RootsPaths, question: &QuestionRecord) -> Option<ScreenFinding> {
    let finding = question.findings.as_ref()?.last()?;
    let path = paths.root.join(&finding.note);
    let text = match fs::read_to_string(path) {
        Ok(content) => {
            let paragraph = first_paragraph(&content);
            if paragraph.is_empty() {
                "(empty note)".to_owned()
            } else if paragraph.chars().count() > 400 {
                let cut = paragraph.chars().take(399).collect::<String>();
                format!("{cut}…")
            } else {
                paragraph
            }
        }
        Err(_) => "(the note is missing)".to_owned(),
    };
    Some(ScreenFinding {
        text,
        path: finding.note.clone(),
    })
}

fn screen_edges(graph: &Graph, node: &NodeRecord) -> Vec<ScreenEdge> {
    graph
        .edges
        .iter()
        .filter(|edge| edge.from == node.id || edge.to == node.id)
        .map(|edge| {
            let outgoing = edge.from == node.id;
            let other = if outgoing { &edge.to } else { &edge.from };
            ScreenEdge {
                rel: edge.rel,
                outgoing,
                id: other.clone(),
                slug: find_node(graph, other)
                    .map(|item| item.slug.clone())
                    .unwrap_or_else(|| "?".to_owned()),
            }
        })
        .collect()
}

fn screen_view(deps: &ThinkDeps<'_>, live: &Live) -> ScreenView {
    let question = current_question(&live.state).map(|question| ScreenQuestion {
        id: question.id.clone(),
        text: question.text.clone(),
        finding: finding_preview(&deps.paths, question),
        source: question_source(question),
        index: live.state.index + 1,
        total: live.state.questions.len(),
    });
    ScreenView {
        id: deps.node.id.clone(),
        slug: deps.node.slug.clone(),
        status: deps.node.status.as_str().to_owned(),
        statement: live.statement.clone(),
        edges: live.edges.clone(),
        question,
        notice: live.state.notice.clone(),
        editor_hint: deps.editor_hint.clone(),
        can_delegate: live.state.can_delegate,
        guidance: deps.guidance.clone(),
    }
}

fn render(deps: &mut ThinkDeps<'_>, live: &Live) {
    let columns = deps.terminal.columns();
    let rows = deps.terminal.rows();
    let lines = render_screen(&screen_view(deps, live), columns, deps.colors, rows);
    deps.terminal.write(&frame(&lines));
}

fn patch_question<F>(paths: &RootsPaths, id: &str, patch: F) -> RootsResult<QuestionRecord>
where
    F: FnOnce(&mut QuestionRecord),
{
    update_table::<QuestionRecord, _, _>(&paths.questions, |mut rows| {
        let output = {
            let question = rows
                .iter_mut()
                .find(|question| question.id == id)
                .ok_or_else(|| RootsError::error(format!("question {id} disappeared")))?;
            patch(question);
            question.clone()
        };
        Ok((rows, true, output))
    })
}

fn persist_effect(
    paths: &RootsPaths,
    config: &RootsConfig,
    by: &str,
    session: &str,
    effect: &SessionEffect,
    now: OffsetDateTime,
) -> RootsResult<()> {
    let at = iso_at(now);
    match effect {
        SessionEffect::End { .. } => Ok(()),
        SessionEffect::Answer {
            question,
            span,
            lines,
        } => {
            let stored = patch_question(paths, &question.id, |item| {
                item.status = QuestionStatus::Answered;
                item.answered_by = Some(by.to_owned());
                item.answered_at = Some(at.clone());
                item.session = Some(session.to_owned());
                item.snoozed_until = None;
            })?;
            let mut extra = event_extra(Some(&question.id), Some(session));
            extra.insert("span".to_owned(), value(span)?);
            if !lines.is_empty() {
                extra.insert(
                    "lines".to_owned(),
                    Value::Array(lines.iter().cloned().map(Value::String).collect()),
                );
            }
            append_event(
                paths,
                &make_event(
                    "answer",
                    by,
                    EventFields {
                        at: Some(at),
                        node: Some(stored.node),
                        extra,
                        ..Default::default()
                    },
                ),
            )
        }
        SessionEffect::Dismiss { question } => {
            let stored = patch_question(paths, &question.id, |item| {
                item.status = QuestionStatus::Dismissed;
                item.dismissed_by = Some(by.to_owned());
                item.dismissed_at = Some(at.clone());
                item.session = Some(session.to_owned());
                item.snoozed_until = None;
            })?;
            append_event(
                paths,
                &make_event(
                    "dismiss",
                    by,
                    EventFields {
                        at: Some(at),
                        node: Some(stored.node),
                        extra: event_extra(Some(&question.id), Some(session)),
                        ..Default::default()
                    },
                ),
            )
        }
        SessionEffect::Snooze { question } => {
            let until = iso_at(now + TimeDuration::days(config.questions.snooze_days as i64));
            let stored = patch_question(paths, &question.id, |item| {
                item.status = QuestionStatus::Snoozed;
                item.snoozed_until = Some(until.clone());
                item.session = Some(session.to_owned());
            })?;
            let mut extra = event_extra(Some(&question.id), Some(session));
            extra.insert("until".to_owned(), Value::String(until));
            append_event(
                paths,
                &make_event(
                    "snooze",
                    by,
                    EventFields {
                        at: Some(at),
                        node: Some(stored.node),
                        extra,
                        ..Default::default()
                    },
                ),
            )
        }
        SessionEffect::Delegate { question } => {
            let stored = patch_question(paths, &question.id, |item| {
                item.status = QuestionStatus::Delegated;
                item.delegated_by = Some(by.to_owned());
                item.delegated_at = Some(at.clone());
                item.session = Some(session.to_owned());
                item.snoozed_until = None;
            })?;
            append_event(
                paths,
                &make_event(
                    "delegate",
                    by,
                    EventFields {
                        at: Some(at),
                        node: Some(stored.node),
                        extra: event_extra(Some(&question.id), Some(session)),
                        ..Default::default()
                    },
                ),
            )
        }
    }
}

fn dispatch(
    deps: &mut ThinkDeps<'_>,
    live: &mut Live,
    input: SessionInput,
    label: &str,
) -> RootsResult<()> {
    let next = step(&live.state, input, label);
    live.state = next.state;
    let now = OffsetDateTime::now_utc();
    for effect in &next.effects {
        persist_effect(
            &deps.paths,
            &deps.config,
            &deps.by,
            &live.session,
            effect,
            now,
        )?;
    }
    if live.state.ended.is_none() {
        render(deps, live);
    }
    Ok(())
}

fn diff_span(before: &str, after: &str) -> Option<DiffSpan> {
    let left = split_lines(before);
    let right = split_lines(after);
    let mut prefix = 0;
    while prefix < left.len() && prefix < right.len() && left[prefix] == right[prefix] {
        prefix += 1;
    }
    let mut suffix = 0;
    while suffix < left.len().saturating_sub(prefix)
        && suffix < right.len().saturating_sub(prefix)
        && left[left.len() - 1 - suffix] == right[right.len() - 1 - suffix]
    {
        suffix += 1;
    }
    let removed = left.len() - prefix - suffix;
    let added = right.len() - prefix - suffix;
    (removed != 0 || added != 0).then_some(DiffSpan {
        from: (prefix + 1) as i64,
        to: (prefix + added) as i64,
        removed: removed as i64,
    })
}

fn changed_lines(before: &str, after: &str) -> Vec<ChangedLine> {
    use std::collections::BTreeMap;
    let mut pool = BTreeMap::<String, usize>::new();
    for line in split_lines(before) {
        *pool.entry(line).or_default() += 1;
    }
    split_lines(after)
        .into_iter()
        .enumerate()
        .filter_map(|(index, text)| {
            let remaining = pool.entry(text.clone()).or_default();
            if *remaining > 0 {
                *remaining -= 1;
                None
            } else if text.trim().is_empty() {
                None
            } else {
                Some(ChangedLine {
                    line: index + 1,
                    text,
                })
            }
        })
        .collect()
}

fn format_span(span: &DiffSpan) -> String {
    if span.to < span.from {
        return format!(
            "deleted {} line{}",
            span.removed,
            if span.removed == 1 { "" } else { "s" }
        );
    }
    if span.from == span.to {
        format!("line {}", span.from)
    } else {
        format!("lines {}–{}", span.from, span.to)
    }
}

fn on_save(deps: &mut ThinkDeps<'_>, live: &mut Live, text: String) -> RootsResult<()> {
    let span = diff_span(&live.last_text, &text);
    let hashes = changed_lines(&live.last_text, &text)
        .into_iter()
        .map(|line| line_hash(&line.text))
        .collect::<Vec<_>>();
    live.last_text = text.clone();
    let Some(span) = span else {
        return Ok(());
    };
    live.statement = parse_prose(&text).statement;
    let label = format_span(&span);
    dispatch(
        deps,
        live,
        SessionInput::Save {
            span,
            lines: hashes,
        },
        &label,
    )
}

fn new_agent_questions(deps: &ThinkDeps<'_>, live: &mut Live) -> RootsResult<Vec<QuestionRecord>> {
    let questions = read_questions(&deps.paths)?;
    let mut arrived = Vec::new();
    for question in questions {
        if !live.seen.insert(question.id.clone()) {
            continue;
        }
        if question.node == deps.node.id
            && question.status == QuestionStatus::Open
            && is_agent(&question)
        {
            arrived.push(question);
        }
    }
    Ok(arrived)
}

fn receive_input(
    deps: &mut ThinkDeps<'_>,
    live: &mut Live,
    receiver: &Receiver<TerminalInput>,
) -> RootsResult<()> {
    // FileWatcher receives its own debounced event stream. Keep the input
    // loop responsive enough to consume that stream instead of making the
    // source watcher's slow poll interval double as an interaction delay.
    match receiver.recv_timeout(Duration::from_millis(25)) {
        Ok(TerminalInput::Key(key)) => dispatch(deps, live, SessionInput::Key(key), ""),
        Ok(TerminalInput::Resize) => {
            render(deps, live);
            Ok(())
        }
        Err(mpsc::RecvTimeoutError::Timeout) | Err(mpsc::RecvTimeoutError::Disconnected) => Ok(()),
    }
}

fn interact(
    deps: &mut ThinkDeps<'_>,
    live: &mut Live,
    prose_file: &std::path::Path,
) -> RootsResult<()> {
    let (sender, receiver) = mpsc::channel::<TerminalInput>();
    let input_sender = sender.clone();
    let resize_sender = sender;
    let mut prose_watcher = FileWatcher::new(prose_file, live.last_text.clone(), deps.watch);
    let initial_questions = fs::read_to_string(&deps.paths.questions).unwrap_or_default();
    let mut question_watcher =
        FileWatcher::new(&deps.paths.questions, initial_questions, deps.watch);
    deps.terminal.start(
        Box::new(move |data| {
            for key in parse_keys(&data) {
                let _ = input_sender.send(TerminalInput::Key(key));
            }
        }),
        Some(Box::new(move || {
            let _ = resize_sender.send(TerminalInput::Resize);
        })),
    );
    render(deps, live);
    let result = (|| -> RootsResult<()> {
        while live.state.ended.is_none() {
            if let Some(text) = prose_watcher.take_change() {
                on_save(deps, live, text)?;
            }
            if live.agents_live && question_watcher.take_change().is_some() {
                let arrived = new_agent_questions(deps, live)?;
                if !arrived.is_empty() {
                    let cap = deps.config.limits.questions_per_session as usize;
                    dispatch(
                        deps,
                        live,
                        SessionInput::Arrive {
                            questions: arrived,
                            cap,
                        },
                        "",
                    )?;
                }
            }
            if live.state.ended.is_none() {
                receive_input(deps, live, &receiver)?;
            }
        }
        // Capture a save that raced the quit key exactly once before the
        // session ledger is finalized.
        if let Some(text) = prose_watcher.check() {
            on_save(deps, live, text)?;
        }
        Ok(())
    })();
    prose_watcher.close();
    question_watcher.close();
    deps.terminal.stop();
    result
}

fn kebab(value: &str) -> String {
    roots_core::slug::slugify(value)
}

pub(crate) fn agent_actor_for(command: &str) -> String {
    let tokens = command.split_whitespace().collect::<Vec<_>>();
    let model = tokens.iter().enumerate().find_map(|(index, token)| {
        token
            .strip_prefix("--model=")
            .map(ToOwned::to_owned)
            .or_else(|| {
                (*token == "--model")
                    .then(|| tokens.get(index + 1).copied().unwrap_or("").to_owned())
            })
    });
    let program = tokens
        .first()
        .copied()
        .unwrap_or("")
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("");
    let name = kebab(model.as_deref().unwrap_or(program));
    format!("agent:{}", if name.is_empty() { "agent" } else { &name })
}

pub(crate) fn agent_environment(
    base: &Environment,
    actor: &str,
    session: Option<&str>,
    node: Option<&str>,
    flow: Option<&str>,
) -> Environment {
    let mut result = base.clone();
    for key in [
        "ROOTS_USER",
        "LUUNA_ROOTS_USER",
        "ROOTS_FORCE_TTY",
        "ROOTS_SESSION",
        "ROOTS_NODE",
        "ROOTS_FLOW",
    ] {
        result.remove(key);
    }
    result.insert("ROOTS_AGENT".to_owned(), actor.to_owned());
    result.insert("LUUNA_ROOTS_AGENT".to_owned(), actor.to_owned());
    if let Some(session) = session {
        result.insert("ROOTS_SESSION".to_owned(), session.to_owned());
    }
    if let Some(node) = node {
        result.insert("ROOTS_NODE".to_owned(), node.to_owned());
    }
    if let Some(flow) = flow {
        result.insert("ROOTS_FLOW".to_owned(), flow.to_owned());
    }
    result
}

#[derive(Clone, Debug)]
pub struct AgentRun {
    pub status: String,
    /// Process exit code when the shell exited normally. This mirrors the
    /// runner result even though the public think summary only needs status.
    pub exit_code: Option<i32>,
    /// Signal name when the child was terminated by a signal.
    pub signal: Option<String>,
    /// Wall-clock duration of the configured command in milliseconds.
    pub duration_ms: u64,
    pub error: Option<String>,
    /// Last few KB of combined stdout/stderr, retained for diagnostics while
    /// keeping agent chatter out of the human session transcript.
    pub output: String,
}

const OUTPUT_TAIL: usize = 4_000;
const KILL_GRACE_MS: u64 = 1_000;

fn output_tail(output: &Arc<Mutex<Vec<u8>>>) -> String {
    let Ok(output) = output.lock() else {
        return String::new();
    };
    let start = output.len().saturating_sub(OUTPUT_TAIL);
    String::from_utf8_lossy(&output[start..]).into_owned()
}

fn collect_stream<R>(mut stream: R, output: Arc<Mutex<Vec<u8>>>)
where
    R: Read,
{
    let mut buffer = [0_u8; 1024];
    loop {
        let Ok(count) = stream.read(&mut buffer) else {
            return;
        };
        if count == 0 {
            return;
        }
        if let Ok(mut collected) = output.lock() {
            collected.extend_from_slice(&buffer[..count]);
            let excess = collected.len().saturating_sub(OUTPUT_TAIL * 2);
            if excess > 0 {
                collected.drain(..excess);
            }
        }
    }
}

fn start_collector<R>(stream: R, output: Arc<Mutex<Vec<u8>>>, done: mpsc::Sender<()>)
where
    R: Read + Send + 'static,
{
    thread::spawn(move || {
        collect_stream(stream, output);
        let _ = done.send(());
    });
}

/// Give the pipe readers a short chance to consume the final buffered bytes
/// after the shell has exited.  We never join indefinitely: a grandchild can
/// retain a pipe even after its direct parent has gone away, which is why the
/// TypeScript runner also has a bounded post-kill path.
fn settle_collectors(done: &Receiver<()>, collectors: usize) {
    for _ in 0..collectors {
        if done.recv_timeout(Duration::from_millis(100)).is_err() {
            break;
        }
    }
}

#[cfg(unix)]
fn kill_group(pid: u32, signal: i32) {
    // `process_group(0)` below makes the child PID its group leader. A
    // negative PID addresses that full group, matching Node's
    // `process.kill(-pid, signal)` behavior.
    unsafe {
        let _ = kill(-(pid as i32), signal);
    }
}

#[cfg(windows)]
fn kill_group(pid: u32, force: bool) {
    let mut command = Command::new("taskkill");
    command.args(["/PID", &pid.to_string(), "/T"]);
    if force {
        command.arg("/F");
    }
    let _ = command.stdout(Stdio::null()).stderr(Stdio::null()).status();
}

#[cfg(not(any(unix, windows)))]
fn kill_group(_pid: u32, _force: bool) {}

fn timeout_error(timeout_ms: u64) -> String {
    let rendered = if timeout_ms >= 1000 {
        format!("{}s", (timeout_ms as f64 / 1000.0).round() as u64)
    } else {
        format!("{timeout_ms}ms")
    };
    format!("timed out after {rendered}")
}

#[cfg(unix)]
fn signal_name(signal: i32) -> String {
    match signal {
        1 => "SIGHUP".to_owned(),
        2 => "SIGINT".to_owned(),
        9 => "SIGKILL".to_owned(),
        15 => "SIGTERM".to_owned(),
        _ => format!("SIG{signal}"),
    }
}

fn exit_status_parts(status: &std::process::ExitStatus) -> (Option<i32>, Option<String>) {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        (status.code(), status.signal().map(signal_name))
    }
    #[cfg(not(unix))]
    {
        (status.code(), None)
    }
}

fn status_error(status: &std::process::ExitStatus) -> String {
    let (exit_code, signal) = exit_status_parts(status);
    if let Some(signal) = signal {
        return format!("killed by {signal}");
    }
    format!(
        "exited with code {}",
        exit_code.map_or_else(|| "null".to_owned(), |code| code.to_string())
    )
}

fn finish_agent_run(
    status: &str,
    exit_code: Option<i32>,
    signal: Option<String>,
    error: Option<String>,
    output: &Arc<Mutex<Vec<u8>>>,
    started: Instant,
) -> AgentRun {
    AgentRun {
        status: status.to_owned(),
        exit_code,
        signal,
        duration_ms: started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,
        error,
        output: output_tail(output),
    }
}

pub(crate) fn run_agent_command(
    command: &str,
    cwd: &std::path::Path,
    input: &str,
    environment: &Environment,
    timeout_ms: u64,
) -> AgentRun {
    let mut process = {
        // Preserve the legacy runner's explicit POSIX shell. On native
        // Windows this intentionally has the same ENOENT behavior as Bun's
        // `/bin/sh` spawn unless the host provides that compatibility path.
        let mut command_process = Command::new("/bin/sh");
        command_process.args(["-c", command]);
        command_process
    };
    #[cfg(unix)]
    process.process_group(0);
    process
        .current_dir(cwd)
        .env_clear()
        .envs(environment)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let started = Instant::now();
    let mut child = match process.spawn() {
        Ok(child) => child,
        Err(problem) => {
            return AgentRun {
                status: "error".to_owned(),
                exit_code: None,
                signal: None,
                duration_ms: started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,
                error: Some(problem.to_string()),
                output: String::new(),
            };
        }
    };
    let output = Arc::new(Mutex::new(Vec::new()));
    let (collector_done, collector_finished) = mpsc::channel();
    let mut collectors = 0_usize;
    if let Some(stdout) = child.stdout.take() {
        start_collector(stdout, Arc::clone(&output), collector_done.clone());
        collectors += 1;
    }
    if let Some(stderr) = child.stderr.take() {
        start_collector(stderr, Arc::clone(&output), collector_done.clone());
        collectors += 1;
    }
    drop(collector_done);
    if let Some(mut stdin) = child.stdin.take() {
        let input = input.to_owned();
        thread::spawn(move || {
            let _ = stdin.write_all(input.as_bytes());
        });
    }
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => {
                settle_collectors(&collector_finished, collectors);
                let (exit_code, signal) = exit_status_parts(&status);
                return finish_agent_run("ok", exit_code, signal, None, &output, started);
            }
            Ok(Some(status)) => {
                settle_collectors(&collector_finished, collectors);
                let (exit_code, signal) = exit_status_parts(&status);
                return finish_agent_run(
                    "failed",
                    exit_code,
                    signal,
                    Some(status_error(&status)),
                    &output,
                    started,
                );
            }
            Ok(None) if started.elapsed() >= Duration::from_millis(timeout_ms) => {
                #[cfg(unix)]
                kill_group(child.id(), 15);
                #[cfg(windows)]
                kill_group(child.id(), false);
                #[cfg(not(any(unix, windows)))]
                kill_group(child.id(), false);
                let grace = Instant::now();
                let stopped_status = loop {
                    match child.try_wait() {
                        Ok(Some(status)) => {
                            break Some(status);
                        }
                        Ok(None) if grace.elapsed() < Duration::from_millis(KILL_GRACE_MS) => {
                            thread::sleep(Duration::from_millis(15));
                        }
                        Ok(None) | Err(_) => {
                            #[cfg(unix)]
                            kill_group(child.id(), 9);
                            #[cfg(windows)]
                            kill_group(child.id(), true);
                            #[cfg(not(any(unix, windows)))]
                            {
                                let _ = child.kill();
                            }
                            break child.wait().ok();
                        }
                    }
                };
                settle_collectors(&collector_finished, collectors);
                let (exit_code, signal) = stopped_status
                    .as_ref()
                    .map(exit_status_parts)
                    .unwrap_or((None, Some("SIGKILL".to_owned())));
                return finish_agent_run(
                    "timeout",
                    exit_code,
                    signal,
                    Some(timeout_error(timeout_ms)),
                    &output,
                    started,
                );
            }
            Ok(None) => thread::sleep(Duration::from_millis(15)),
            Err(problem) => {
                return finish_agent_run(
                    "error",
                    None,
                    None,
                    Some(problem.to_string()),
                    &output,
                    started,
                );
            }
        }
    }
}

fn ask_prompt(node: &NodeRecord, actor: &str, asks: u64, context: &str) -> String {
    let head = AGENT_INSTRUCTION
        .replace("{{id}}", &node.id)
        .replace("{{actor}}", actor)
        .replace("{{asks}}", &asks.to_string())
        .replace(
            "{{max}}",
            &roots_core::questions::QUESTION_MAX_LENGTH.to_string(),
        );
    format!("{head}\n---\n\n{context}")
}

const SESSION_PROPOSALS_MAX: u64 = 3;

fn proposal_prompt(
    node: &NodeRecord,
    changed: &[ChangedLine],
    candidates: &[String],
    actor: &str,
    max: u64,
    context: &str,
) -> String {
    let head = PROPOSE_INSTRUCTION
        .replace("{{id}}", &node.id)
        .replace("{{slug}}", &node.slug)
        .replace("{{actor}}", actor)
        .replace("{{max}}", &max.to_string());
    let mut lines = vec!["## Changed lines (this session)".to_owned(), String::new()];
    lines.extend(
        changed
            .iter()
            .map(|line| format!("- line {}: {}", line.line, line.text)),
    );
    lines.extend([
        "".to_owned(),
        "## Other ideas (candidates)".to_owned(),
        String::new(),
    ]);
    if candidates.is_empty() {
        lines.push("(none)".to_owned());
    } else {
        lines.extend(candidates.iter().cloned());
    }
    lines.push(String::new());
    format!("{head}\n---\n\n{}\n{context}", lines.join("\n"))
}

fn delegation_prompt(
    node: &NodeRecord,
    questions: &[QuestionRecord],
    actor: &str,
    context: &str,
) -> String {
    let head = DELEGATE_INSTRUCTION
        .replace("{{id}}", &node.id)
        .replace("{{slug}}", &node.slug)
        .replace("{{actor}}", actor);
    let mut lines = vec!["## Delegated questions".to_owned(), String::new()];
    for question in questions {
        lines.push(format!("- {}: {}", question.id, question.text));
        lines.extend(
            question
                .findings
                .as_deref()
                .unwrap_or_default()
                .iter()
                .map(|finding| format!("  - earlier findings: {}", finding.note)),
        );
    }
    lines.push(String::new());
    format!("{head}\n---\n\n{}\n{context}", lines.join("\n"))
}

fn session_agent_phase(deps: &ThinkDeps<'_>, graph: &Graph, session: &str) -> AgentPhase {
    let effective = effective_tier(&deps.config, Some(&deps.node));
    if effective.tier < 1 {
        return AgentPhase {
            mode: "off",
            tier: effective.tier,
            actor: None,
            status: None,
            error: None,
            asked: 0,
            background: false,
        };
    }
    let Some(command) = deps.config.agent.command.as_deref() else {
        return AgentPhase {
            mode: "harness",
            tier: effective.tier,
            actor: None,
            status: None,
            error: None,
            asked: 0,
            background: false,
        };
    };
    let actor = agent_actor_for(command);
    let open = read_questions(&deps.paths)
        .map(|rows| open_agent_questions(&rows, &deps.node.id).len() as u64)
        .unwrap_or(0);
    if open >= deps.config.limits.questions_per_session {
        return AgentPhase {
            mode: "full",
            tier: effective.tier,
            actor: Some(actor),
            status: None,
            error: None,
            asked: 0,
            background: false,
        };
    }
    let context = build_context(
        &deps.paths,
        graph,
        &deps.config,
        &deps.node,
        Some(session.to_owned()),
        None,
    )
    .map(|packet| render_context_markdown(&packet))
    .unwrap_or_default();
    let asks = deps
        .config
        .limits
        .questions_per_session
        .saturating_sub(open);
    let prompt = ask_prompt(&deps.node, &actor, asks, &context);
    let environment = agent_environment(
        &deps.environment,
        &actor,
        Some(session),
        Some(&deps.node.id),
        None,
    );
    let timeout = deps
        .agent_timeout_ms
        .unwrap_or(deps.config.agent.timeout_seconds.saturating_mul(1000));
    if let Some(reporter) = &deps.background {
        let command = command.to_owned();
        let root = deps.paths.root.clone();
        let label = format!("{actor} asking about {}", deps.node.slug);
        start_background(reporter, label, move || {
            let run = run_agent_command(&command, &root, &prompt, &environment, timeout);
            (run.status != "ok")
                .then(|| format!("agent.command {}", run.error.as_deref().unwrap_or("failed")))
        });
        return AgentPhase {
            mode: "command",
            tier: effective.tier,
            actor: Some(actor),
            status: Some("background".to_owned()),
            error: None,
            asked: 0,
            background: true,
        };
    }
    let mut status = start_agent_status(
        deps,
        format!("asking {actor} for questions about {}…", deps.node.slug),
    );
    let run = run_agent_command(command, &deps.paths.root, &prompt, &environment, timeout);
    if let Some(status) = status.as_mut() {
        let final_line = (run.status != "ok").then(|| {
            format!(
                "! agent.command {}; continuing without it",
                run.error.as_deref().unwrap_or("failed")
            )
        });
        status.stop(final_line.as_deref());
    }
    let asked = read_questions(&deps.paths)
        .map(|rows| {
            rows.iter()
                .filter(|question| {
                    question.node == deps.node.id
                        && question.session.as_deref() == Some(session)
                        && is_agent(question)
                })
                .count()
        })
        .unwrap_or(0);
    AgentPhase {
        mode: "command",
        tier: effective.tier,
        actor: Some(actor),
        status: Some(run.status),
        error: run.error,
        asked,
        background: false,
    }
}

fn agent_summary(
    deps: &ThinkDeps<'_>,
    phase: &AgentPhase,
    session: &str,
    arrived: usize,
) -> AgentSummary {
    let asked = if phase.background {
        read_questions(&deps.paths)
            .map(|rows| {
                rows.iter()
                    .filter(|question| {
                        question.node == deps.node.id
                            && question.session.as_deref() == Some(session)
                            && is_agent(question)
                    })
                    .count()
            })
            .unwrap_or(phase.asked)
    } else {
        phase.asked
    };
    AgentSummary {
        mode: phase.mode.to_owned(),
        tier: phase.tier,
        actor: phase.actor.clone(),
        status: phase.status.clone(),
        error: phase.error.clone(),
        asked,
        arrived,
    }
}

/// Start the post-session mention-proposal pass used by `flow`.
///
/// A background think session deliberately does not run this phase itself:
/// Flow owns its job list and begins exactly one proposal pass after the
/// human has left the think screen. The synchronous `proposal_phase` below
/// remains the standalone `think` behavior.
pub(crate) fn start_session_proposals(
    paths: RootsPaths,
    config: RootsConfig,
    node: NodeRecord,
    session: String,
    changed: Vec<ChangedLine>,
    environment: Environment,
    timeout_override: Option<u64>,
) -> RootsResult<Option<BackgroundJob>> {
    let Some(command) = config.agent.command.as_deref() else {
        return Ok(None);
    };
    if changed.is_empty() || effective_tier(&config, Some(&node)).tier < 2 {
        return Ok(None);
    }
    let actor = agent_actor_for(command);
    let graph = read_graph(&paths)?;
    let rows = read_proposals(&paths)?;
    let pending = pending_proposals(&rows, OffsetDateTime::now_utc(), Some(&graph)).len() as u64;
    let max = SESSION_PROPOSALS_MAX.min(config.limits.proposals.saturating_sub(pending));
    let candidates = mention_targets(&paths, &graph)?
        .into_iter()
        .filter(|target| target.id != node.id)
        .map(|target| format!("- {} {}: \"{}\"", target.id, target.slug, target.statement))
        .collect::<Vec<_>>();
    if max == 0 || candidates.is_empty() {
        return Ok(None);
    }
    let context = build_context(&paths, &graph, &config, &node, Some(session.clone()), None)
        .map(|packet| render_context_markdown(&packet))
        .unwrap_or_default();
    let prompt = proposal_prompt(&node, &changed, &candidates, &actor, max, &context);
    let child_environment =
        agent_environment(&environment, &actor, Some(&session), Some(&node.id), None);
    let timeout = timeout_override.unwrap_or(config.agent.timeout_seconds.saturating_mul(1000));
    let label = format!("linking what changed in {}", node.slug);
    let command = command.to_owned();
    let root = paths.root;
    let (sender, done) = mpsc::channel();
    thread::spawn(move || {
        let run = run_agent_command(&command, &root, &prompt, &child_environment, timeout);
        let problem = (run.status != "ok")
            .then(|| format!("agent.command {}", run.error.as_deref().unwrap_or("failed")));
        let _ = sender.send(problem);
    });
    Ok(Some(BackgroundJob { label, done }))
}

fn proposal_phase(
    deps: &ThinkDeps<'_>,
    session: &str,
    changed: &[ChangedLine],
) -> Option<ProposalSummary> {
    // Flow owns the post-session proposal job. It starts that job after the
    // terminal hand-off so there is exactly one run for a completed session
    // and the Flow card can track its lifecycle.
    if deps.background.is_some() {
        return None;
    }
    let command = deps.config.agent.command.as_deref()?;
    if changed.is_empty() || effective_tier(&deps.config, Some(&deps.node)).tier < 2 {
        return None;
    }
    let actor = agent_actor_for(command);
    let graph = read_graph(&deps.paths).ok()?;
    let rows = read_proposals(&deps.paths).ok()?;
    let pending = pending_proposals(&rows, OffsetDateTime::now_utc(), Some(&graph)).len() as u64;
    let max = SESSION_PROPOSALS_MAX.min(deps.config.limits.proposals.saturating_sub(pending));
    let candidates = mention_targets(&deps.paths, &graph)
        .ok()?
        .into_iter()
        .filter(|target| target.id != deps.node.id)
        .map(|target| format!("- {} {}: \"{}\"", target.id, target.slug, target.statement))
        .collect::<Vec<_>>();
    if max == 0 || candidates.is_empty() {
        return None;
    }
    let before = rows
        .into_iter()
        .filter(|proposal| proposal.by == actor)
        .map(|proposal| proposal.id)
        .collect::<BTreeSet<_>>();
    let context = build_context(
        &deps.paths,
        &graph,
        &deps.config,
        &deps.node,
        Some(session.to_owned()),
        None,
    )
    .map(|packet| render_context_markdown(&packet))
    .unwrap_or_default();
    let prompt = proposal_prompt(&deps.node, changed, &candidates, &actor, max, &context);
    let environment = agent_environment(
        &deps.environment,
        &actor,
        Some(session),
        Some(&deps.node.id),
        None,
    );
    let timeout = deps
        .agent_timeout_ms
        .unwrap_or(deps.config.agent.timeout_seconds.saturating_mul(1000));
    let mut status = start_agent_status(
        deps,
        format!("asking {actor} to link what changed in {}…", deps.node.slug),
    );
    let run = run_agent_command(command, &deps.paths.root, &prompt, &environment, timeout);
    if let Some(status) = status.as_mut() {
        let final_line = (run.status != "ok").then(|| {
            format!(
                "! agent.command {}; no agent links",
                run.error.as_deref().unwrap_or("failed")
            )
        });
        status.stop(final_line.as_deref());
    }
    let filed = read_proposals(&deps.paths)
        .unwrap_or_default()
        .into_iter()
        .filter(|proposal| proposal.by == actor && !before.contains(&proposal.id))
        .map(|proposal| proposal.id)
        .collect();
    Some(ProposalSummary {
        actor,
        status: run.status,
        error: run.error,
        filed,
    })
}

fn delegated_questions(paths: &RootsPaths, node: &str) -> RootsResult<Vec<QuestionRecord>> {
    Ok(read_questions(paths)?
        .into_iter()
        .filter(|question| question.node == node && question.status == QuestionStatus::Delegated)
        .collect())
}

fn undelegate(
    paths: &RootsPaths,
    id: &str,
    reason: &str,
    now: OffsetDateTime,
) -> RootsResult<bool> {
    let question = update_table::<QuestionRecord, _, _>(&paths.questions, |mut rows| {
        let output = {
            let Some(question) = rows.iter_mut().find(|question| question.id == id) else {
                return Ok((rows, false, None));
            };
            if question.status != QuestionStatus::Delegated {
                return Ok((rows, false, None));
            }
            question.status = QuestionStatus::Open;
            question.clone()
        };
        Ok((rows, true, Some(output)))
    })?;
    let Some(question) = question else {
        return Ok(false);
    };
    let mut extra = event_extra(Some(id), None);
    extra.insert("reason".to_owned(), Value::String(reason.to_owned()));
    append_event(
        paths,
        &make_event(
            "undelegate",
            "roots",
            EventFields {
                at: Some(iso_at(now)),
                node: Some(question.node),
                extra,
                ..Default::default()
            },
        ),
    )?;
    Ok(true)
}

fn delegation_phase(deps: &ThinkDeps<'_>, session: &str) -> RootsResult<Option<DelegationSummary>> {
    let pending = delegated_questions(&deps.paths, &deps.node.id)?;
    if pending.is_empty()
        || effective_tier(&deps.config, Some(&deps.node)).tier < 1
        || deps.config.agent.command.is_none()
    {
        return Ok(None);
    }
    let command = deps.config.agent.command.as_deref().unwrap_or_default();
    let actor = agent_actor_for(command);
    let questions = pending
        .iter()
        .map(|question| question.id.clone())
        .collect::<Vec<_>>();
    let graph = read_graph(&deps.paths)?;
    let context = build_context(
        &deps.paths,
        &graph,
        &deps.config,
        &deps.node,
        Some(session.to_owned()),
        None,
    )
    .map(|packet| render_context_markdown(&packet))
    .unwrap_or_default();
    let prompt = delegation_prompt(&deps.node, &pending, &actor, &context);
    let pending_count = questions.len();
    let environment = agent_environment(
        &deps.environment,
        &actor,
        Some(session),
        Some(&deps.node.id),
        None,
    );
    let timeout = deps
        .agent_timeout_ms
        .unwrap_or(deps.config.agent.timeout_seconds.saturating_mul(1000));
    if let Some(reporter) = &deps.background {
        let command = command.to_owned();
        let root = deps.paths.root.clone();
        let paths = deps.paths.clone();
        let questions_for_run = questions.clone();
        let actor_for_run = actor.clone();
        let label = format!(
            "researching {pending_count} question{} on {}",
            if pending_count == 1 { "" } else { "s" },
            deps.node.slug
        );
        start_background(reporter, label, move || {
            let run = run_agent_command(&command, &root, &prompt, &environment, timeout);
            let reason = if run.status == "ok" {
                "no findings".to_owned()
            } else {
                format!("agent.command {}", run.error.as_deref().unwrap_or_default())
            };
            let mut returned = Vec::new();
            for id in &questions_for_run {
                match undelegate(&paths, id, &reason, OffsetDateTime::now_utc()) {
                    Ok(true) => returned.push(id.clone()),
                    Ok(false) => {}
                    Err(problem) => return Some(problem.message),
                }
            }
            if run.status != "ok" {
                Some(format!(
                    "agent.command {}",
                    run.error.as_deref().unwrap_or("failed")
                ))
            } else if returned.is_empty() {
                None
            } else {
                Some(format!(
                    "{} came back without findings",
                    returned.join(", ")
                ))
            }
        });
        return Ok(Some(DelegationSummary {
            actor: actor_for_run,
            status: "background".to_owned(),
            error: None,
            questions,
            found: Vec::new(),
            returned: Vec::new(),
        }));
    }
    let mut status = start_agent_status(
        deps,
        format!(
            "{actor} is researching {pending_count} question{} about {}…",
            if pending_count == 1 { "" } else { "s" },
            deps.node.slug
        ),
    );
    let run = run_agent_command(command, &deps.paths.root, &prompt, &environment, timeout);
    let reason = if run.status == "ok" {
        "no findings".to_owned()
    } else {
        format!("agent.command {}", run.error.clone().unwrap_or_default())
    };
    let mut returned = Vec::new();
    for id in &questions {
        if undelegate(&deps.paths, id, &reason, OffsetDateTime::now_utc())? {
            returned.push(id.clone());
        }
    }
    if let Some(status) = status.as_mut() {
        let final_line = (!returned.is_empty()).then(|| {
            format!(
                "! {} question(s) came back without findings",
                returned.len()
            )
        });
        status.stop(final_line.as_deref());
    }
    let returned_set = returned.iter().collect::<HashSet<_>>();
    let found = questions
        .iter()
        .filter(|id| !returned_set.contains(id))
        .cloned()
        .collect();
    Ok(Some(DelegationSummary {
        actor,
        status: run.status,
        error: run.error,
        questions,
        found,
        returned,
    }))
}

fn mark_shaping(
    deps: &ThinkDeps<'_>,
    session: &str,
    now: OffsetDateTime,
) -> RootsResult<Option<StatusChange>> {
    let at = iso_at(now);
    let change = update_graph(&deps.paths, |graph| {
        let Some(node) = graph.nodes.iter_mut().find(|node| node.id == deps.node.id) else {
            return Ok((false, None));
        };
        if node.status != NodeStatus::Idea(IdeaStatus::Planted) {
            return Ok((false, None));
        }
        node.status = NodeStatus::Idea(IdeaStatus::Shaping);
        node.updated_at = Some(at.clone());
        Ok((
            true,
            Some(StatusChange {
                from: "planted".to_owned(),
                to: "shaping".to_owned(),
            }),
        ))
    })?;
    if let Some(change) = &change {
        let mut extra = event_extra(None, Some(session));
        extra.insert("from".to_owned(), Value::String(change.from.clone()));
        extra.insert("to".to_owned(), Value::String(change.to.clone()));
        append_event(
            &deps.paths,
            &make_event(
                "status",
                &deps.by,
                EventFields {
                    at: Some(at),
                    node: Some(deps.node.id.clone()),
                    extra,
                    ..Default::default()
                },
            ),
        )?;
    }
    Ok(change)
}

fn mention_summary(filing: MentionFiling) -> MentionSummary {
    MentionSummary {
        mentions: filing.mentions,
        result: MentionResultSummary {
            filed: filing.result.filed,
            skipped: filing.result.skipped,
            expired: filing.result.expired,
        },
    }
}

fn finalize(
    deps: &ThinkDeps<'_>,
    live: &Live,
    start_text: &str,
    phase: &AgentPhase,
    initial: &HashSet<String>,
) -> RootsResult<SessionSummary> {
    let now = OffsetDateTime::now_utc();
    let prose = read_node_prose(&deps.paths, &deps.node, None)?;
    let final_text = if prose.exists {
        prose.text
    } else {
        live.last_text.clone()
    };
    let changed_lines = changed_lines(start_text, &final_text);
    let (mentions, mention_error) = if changed_lines.is_empty() {
        (None, None)
    } else {
        match read_graph(&deps.paths).and_then(|graph| {
            file_mentions(
                &deps.paths,
                &deps.config,
                &graph,
                &deps.node.id,
                &changed_lines,
                now,
            )
        }) {
            Ok(filing) => (Some(mention_summary(filing)), None),
            Err(problem) => (None, Some(problem.message)),
        }
    };
    let proposals = proposal_phase(deps, &live.session, &changed_lines);
    let status_change = mark_shaping(deps, &live.session, now)?;
    let session_tally = tally(&live.state);
    let reason = live.state.ended.unwrap_or(EndReason::Quit);
    let mut extra = event_extra(None, Some(&live.session));
    extra.insert("hash".to_owned(), Value::String(content_hash(&final_text)));
    extra.insert(
        "reason".to_owned(),
        Value::String(reason.as_str().to_owned()),
    );
    extra.insert("changed".to_owned(), Value::Bool(final_text != start_text));
    for (key, count) in [
        ("answered", session_tally.answered),
        ("dismissed", session_tally.dismissed),
        ("snoozed", session_tally.snoozed),
        ("skipped", session_tally.skipped),
        ("delegated", session_tally.delegated),
        ("unasked", session_tally.unasked),
    ] {
        extra.insert(key.to_owned(), Value::Number((count as u64).into()));
    }
    append_event(
        &deps.paths,
        &make_event(
            "session.end",
            &deps.by,
            EventFields {
                at: Some(iso_at(now)),
                node: Some(deps.node.id.clone()),
                extra,
                ..Default::default()
            },
        ),
    )?;
    let delegation = delegation_phase(deps, &live.session)?;
    let arrived = live
        .state
        .questions
        .iter()
        .filter(|question| !initial.contains(&question.id))
        .count();
    Ok(SessionSummary {
        session: live.session.clone(),
        node: deps.node.id.clone(),
        reason: reason.as_str().to_owned(),
        questions: live
            .state
            .questions
            .iter()
            .map(|question| SessionQuestionSummary {
                id: question.id.clone(),
                text: question.text.clone(),
                by: question.by.clone(),
                outcome: live
                    .state
                    .outcomes
                    .get(&question.id)
                    .map(|outcome| outcome.as_str().to_owned()),
            })
            .collect(),
        tally: session_tally,
        changed: final_text != start_text,
        hash: content_hash(&final_text),
        status_change,
        mentions,
        mention_error,
        agent: agent_summary(deps, phase, &live.session, arrived),
        proposals,
        delegation,
        // Flow starts the post-session proposal job itself after this handoff.
        // Give it the exact diff only in that detached mode; ordinary `think`
        // already consumed it synchronously and keeps the legacy JSON shape.
        changed_lines: deps.background.as_ref().map(|_| changed_lines),
    })
}

/// Run one terminal session and return its persisted audit summary.
pub fn run_think_session(mut deps: ThinkDeps<'_>) -> RootsResult<SessionSummary> {
    let prose = read_node_prose(&deps.paths, &deps.node, None)?;
    if !prose.exists || prose.path.is_none() {
        return Err(RootsError::not_found(format!(
            "idea.md for {} is missing",
            deps.node.id
        )));
    }
    let prose_file = prose.path.clone().expect("checked above");
    let session = new_session_id(&deps.paths)?;
    append_event(
        &deps.paths,
        &make_event(
            "session.start",
            &deps.by,
            EventFields {
                at: Some(iso_at(OffsetDateTime::now_utc())),
                node: Some(deps.node.id.clone()),
                extra: event_extra(None, Some(&session)),
                ..Default::default()
            },
        ),
    )?;
    let graph = read_graph(&deps.paths)?;
    let phase = session_agent_phase(&deps, &graph, &session);
    let agents = phase.mode != "off";
    let questions = prepare_questions(
        &deps.paths,
        &deps.config,
        &graph,
        &deps.node,
        &session,
        OffsetDateTime::now_utc(),
        agents,
    )?;
    let initial = questions
        .iter()
        .map(|question| question.id.clone())
        .collect::<HashSet<_>>();
    let seen = read_questions(&deps.paths)?
        .into_iter()
        .map(|question| question.id)
        .collect();
    let notice = if phase.background {
        phase
            .actor
            .as_ref()
            .map(|actor| format!("asking {actor} for questions; they show up here as they arrive"))
    } else if phase.mode == "command" {
        match phase.status.as_deref() {
            Some("ok") if phase.asked == 0 => {
                Some("the agent had no questions this time".to_owned())
            }
            Some("ok") => Some(format!(
                "the agent asked {} question{}",
                phase.asked,
                if phase.asked == 1 { "" } else { "s" }
            )),
            _ => phase.error.as_ref().map(|problem| {
                format!("agent unavailable ({problem}); using roots' own questions")
            }),
        }
    } else {
        None
    };
    let mut live = Live {
        session,
        state: SessionState {
            notice,
            ..initial_state(questions, agents)
        },
        last_text: prose.text.clone(),
        statement: prose.statement,
        edges: screen_edges(&graph, &deps.node),
        agents_live: agents,
        seen,
    };
    interact(&mut deps, &mut live, &prose_file)?;
    finalize(&deps, &live, &prose.text, &phase, &initial)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands_capture::plant_text;
    use crate::interactive::terminal::FakeTerminal;
    use roots_core::config::default_config;
    use roots_core::graph::read_graph;
    use roots_core::paths::roots_paths;
    use std::io::Cursor;
    use std::sync::mpsc;

    fn root() -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!(
            "roots-think-session-{}",
            roots_core::ids::random_hex(8)
        ));
        fs::create_dir_all(&root).unwrap();
        // Reuse the tested CLI initializer's on-disk contract through the
        // public setup helper would introduce a CLI dependency cycle, so set
        // up only the files this module reads.
        let paths = roots_paths(&root);
        fs::create_dir_all(&paths.human).unwrap();
        fs::create_dir_all(&paths.agent).unwrap();
        fs::write(
            &paths.config,
            roots_core::config::render_config(&default_config("test")),
        )
        .unwrap();
        for file in [
            &paths.graph,
            &paths.questions,
            &paths.events,
            &paths.proposals,
        ] {
            fs::write(file, "").unwrap();
        }
        root
    }

    #[test]
    fn fixed_agent_prompts_are_native_contracts() {
        // These literals live in `prompt_contract`, not in `src/`, so a
        // Rust-only distribution exercises the same contract it ships.
        assert!(AGENT_INSTRUCTION.contains("A dismissed question is one\n  the human refused"));
        assert!(PROPOSE_INSTRUCTION.contains("--mention \\\n    --reason"));
        assert!(DELEGATE_INSTRUCTION.contains("Delegated questions"));
    }

    #[test]
    fn agent_prompt_and_environment_preserve_the_runner_contract() {
        let root = root();
        let paths = roots_paths(&root);
        let _planted = plant_text(&paths, "Sync works offline\n", "human:test", None)
            .unwrap()
            .unwrap();
        let node = read_graph(&paths).unwrap().nodes.remove(0);
        let prompt = ask_prompt(&node, "agent:opus", 2, "# context");
        assert!(prompt.contains(&format!(
            "luuna-roots ask {} \"<your question>\" --as agent:opus",
            node.id
        )));
        assert!(prompt.contains("ask up to 2 questions"));
        assert!(prompt.contains("at most 280 characters"));
        assert!(!prompt.contains("{{"));

        let base = Environment::from([
            ("PATH".to_owned(), "test-path".to_owned()),
            ("ROOTS_USER".to_owned(), "human:old".to_owned()),
            ("LUUNA_ROOTS_USER".to_owned(), "human:old".to_owned()),
            ("ROOTS_FORCE_TTY".to_owned(), "1".to_owned()),
            ("ROOTS_SESSION".to_owned(), "stale".to_owned()),
            ("ROOTS_NODE".to_owned(), "stale".to_owned()),
            ("ROOTS_FLOW".to_owned(), "stale".to_owned()),
        ]);
        let agent = agent_environment(&base, "agent:opus", Some("ss-1"), Some("r-1"), None);
        assert_eq!(agent.get("PATH"), Some(&"test-path".to_owned()));
        assert_eq!(agent.get("ROOTS_AGENT"), Some(&"agent:opus".to_owned()));
        assert_eq!(
            agent.get("LUUNA_ROOTS_AGENT"),
            Some(&"agent:opus".to_owned())
        );
        assert_eq!(agent.get("ROOTS_SESSION"), Some(&"ss-1".to_owned()));
        assert_eq!(agent.get("ROOTS_NODE"), Some(&"r-1".to_owned()));
        assert!(!agent.contains_key("ROOTS_FLOW"));
        assert!(!agent.contains_key("ROOTS_USER"));
        assert!(!agent.contains_key("LUUNA_ROOTS_USER"));
        assert!(!agent.contains_key("ROOTS_FORCE_TTY"));

        let minimal = agent_environment(&base, "agent:opus", None, None, None);
        assert!(!minimal.contains_key("ROOTS_SESSION"));
        assert!(!minimal.contains_key("ROOTS_NODE"));
        assert!(!minimal.contains_key("ROOTS_FLOW"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn output_tail_collects_both_agent_streams_without_leaking_old_output() {
        let output = Arc::new(Mutex::new(Vec::new()));
        collect_stream(
            Cursor::new(b"stdout-before\n".to_vec()),
            Arc::clone(&output),
        );
        collect_stream(
            Cursor::new(b"stderr-before\n".to_vec()),
            Arc::clone(&output),
        );
        let combined = output_tail(&output);
        assert!(combined.contains("stdout-before"));
        assert!(combined.contains("stderr-before"));
        collect_stream(
            Cursor::new(vec![b'x'; OUTPUT_TAIL + 64]),
            Arc::clone(&output),
        );
        let tail = output_tail(&output);
        assert!(tail.ends_with(&"x".repeat(64)));
        assert!(tail.len() <= OUTPUT_TAIL);
    }

    #[cfg(unix)]
    fn shell_environment() -> Environment {
        std::env::vars().collect()
    }

    #[cfg(unix)]
    #[test]
    fn runner_reports_exit_code_and_combined_stdout_stderr_tail() {
        let root = root();
        let run = run_agent_command(
            "printf stdout; printf stderr >&2; exit 3",
            &root,
            "ignored",
            &shell_environment(),
            2_000,
        );
        assert_eq!(run.status, "failed");
        assert_eq!(run.exit_code, Some(3));
        assert_eq!(run.signal, None);
        assert_eq!(run.error.as_deref(), Some("exited with code 3"));
        assert!(run.output.contains("stdout"));
        assert!(run.output.contains("stderr"));
        let _ = fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn runner_timeout_terminates_the_entire_shell_process_group() {
        let root = root();
        let run = run_agent_command(
            "(sleep 0.25; printf leaked > leaked.txt) & sleep 30",
            &root,
            "",
            &shell_environment(),
            50,
        );
        assert_eq!(run.status, "timeout");
        assert_eq!(run.error.as_deref(), Some("timed out after 50ms"));
        assert!(run.duration_ms < 3_000);
        thread::sleep(Duration::from_millis(400));
        assert!(!root.join("leaked.txt").exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn save_and_dismiss_persist_question_events_and_restore_terminal() {
        let root = root();
        let paths = roots_paths(&root);
        let planted = plant_text(&paths, "Sync works offline\n", "human:test", None)
            .unwrap()
            .unwrap();
        let node = read_graph(&paths).unwrap().nodes.remove(0);
        let file = planted.file;
        let (terminal, control) = FakeTerminal::new(70, 30);
        let (done_tx, done_rx) = mpsc::channel();
        let paths_for_thread = paths.clone();
        let node_for_thread = node.clone();
        thread::spawn(move || {
            let mut terminal = terminal;
            let result = run_think_session(ThinkDeps {
                paths: paths_for_thread,
                config: default_config("test"),
                node: node_for_thread,
                by: "human:test".to_owned(),
                terminal: &mut terminal,
                colors: Colors::new(false),
                editor_hint: "Open idea.md in another pane.".to_owned(),
                watch: WatchOptions {
                    debounce_ms: 5,
                    poll_ms: 5,
                },
                environment: Environment::new(),
                guidance: None,
                agent_timeout_ms: Some(20),
                agent_status: None,
                background: None,
            });
            done_tx.send(result).unwrap();
        });
        let started = Instant::now();
        while !control.screen().contains("question 1/") {
            assert!(started.elapsed() < Duration::from_secs(2));
            thread::sleep(Duration::from_millis(5));
        }
        fs::write(&file, "Sync works offline\n\nDone means no lost work.\n").unwrap();
        let started = Instant::now();
        while !control.screen().contains("question 2/") {
            assert!(started.elapsed() < Duration::from_secs(2));
            thread::sleep(Duration::from_millis(5));
        }
        control.send("d");
        let summary = done_rx
            .recv_timeout(Duration::from_secs(3))
            .unwrap()
            .unwrap();
        assert_eq!(summary.tally.answered, 1);
        assert_eq!(summary.tally.dismissed, 1);
        assert_eq!(summary.status_change.unwrap().to, "shaping");
        assert!(summary.changed_lines.is_none());
        assert!(control.stopped());
        let questions = read_questions(&paths).unwrap();
        assert_eq!(questions.len(), 2);
        assert_eq!(questions[0].status, QuestionStatus::Answered);
        assert_eq!(questions[1].status, QuestionStatus::Dismissed);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn background_think_keeps_changed_lines_and_defers_proposals() {
        let root = root();
        let paths = roots_paths(&root);
        let planted = plant_text(&paths, "Sync works offline\n", "human:test", None)
            .unwrap()
            .unwrap();
        let node = read_graph(&paths).unwrap().nodes.remove(0);
        let (terminal, control) = FakeTerminal::new(70, 30);
        let jobs = Arc::new(Mutex::new(Vec::<BackgroundJob>::new()));
        let jobs_for_session = Arc::clone(&jobs);
        let reporter: BackgroundReporter = Arc::new(move |job| {
            jobs_for_session.lock().unwrap().push(job);
        });
        let (done_tx, done_rx) = mpsc::channel();
        let paths_for_thread = paths.clone();
        let node_for_thread = node.clone();
        let file = planted.file;
        thread::spawn(move || {
            let mut terminal = terminal;
            let mut config = default_config("test");
            // This deliberately missing command exits quickly through the
            // runner. It proves job dispatch without requiring an agent.
            config.agent.command = Some("roots-test-missing-agent".to_owned());
            let result = run_think_session(ThinkDeps {
                paths: paths_for_thread,
                config,
                node: node_for_thread,
                by: "human:test".to_owned(),
                terminal: &mut terminal,
                colors: Colors::new(false),
                editor_hint: "Open idea.md in another pane.".to_owned(),
                watch: WatchOptions {
                    debounce_ms: 5,
                    poll_ms: 5,
                },
                environment: Environment::new(),
                guidance: None,
                agent_timeout_ms: Some(100),
                agent_status: None,
                background: Some(reporter),
            });
            done_tx.send(result).unwrap();
        });
        let started = Instant::now();
        while !control.started() {
            assert!(started.elapsed() < Duration::from_secs(2));
            thread::sleep(Duration::from_millis(5));
        }
        fs::write(
            &file,
            "Sync works offline\n\nNo work disappears after reconnect.\n",
        )
        .unwrap();
        control.send("q");
        let summary = done_rx
            .recv_timeout(Duration::from_secs(3))
            .unwrap()
            .unwrap();
        assert!(summary.proposals.is_none());
        assert_eq!(
            summary
                .changed_lines
                .as_ref()
                .expect("background flow receives changed lines")[0]
                .text,
            "No work disappears after reconnect."
        );
        let mut jobs = jobs.lock().unwrap();
        assert_eq!(jobs.len(), 1, "only the question phase belongs to Think");
        let problem = jobs
            .remove(0)
            .done
            .recv_timeout(Duration::from_secs(2))
            .unwrap();
        assert!(problem.is_some());
        let _ = fs::remove_dir_all(root);
    }
}
