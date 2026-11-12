//! Stateful terminal driver for a Flow session.

use super::flow::{
    FlowAction, FlowCard, FlowTrail, HeadingView, build_trail, flow_key, initial_card,
    parse_flow_keys, rank_picks,
};
use super::flow_render::{FlowFrame, render_flow_card};
use super::heading_run::{HeadingJobInput, heading_enabled, start_heading_phase};
use super::tend_session::collect_cards;
use super::terminal::{InputCallback, ResizeCallback, Terminal, frame};
use super::think_session::{
    BackgroundJob, BackgroundReporter, SessionSummary, start_session_proposals,
};
use roots_core::actor::Environment;
use roots_core::attention::{collect_attention, rank_attention};
use roots_core::color::Colors;
use roots_core::config::RootsConfig;
use roots_core::errors::{RootsError, RootsResult};
use roots_core::events::{EventFields, append_event, make_event, read_events};
use roots_core::graph::{find_node, read_graph};
use roots_core::headings::{active_heading, dismiss_heading, read_headings};
use roots_core::ids::{IdPrefix, generate_id, hex_set};
use roots_core::paths::RootsPaths;
use roots_core::questions::read_questions;
use roots_core::serde_json::Value;
use roots_core::time::iso_at;
use roots_core::time_crate::OffsetDateTime;
use std::collections::BTreeSet;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub trait FlowSteps {
    fn think(
        &mut self,
        terminal: &mut dyn Terminal,
        id: &str,
        background: BackgroundReporter,
    ) -> RootsResult<SessionSummary>;

    fn tend(&mut self, terminal: &mut dyn Terminal) -> RootsResult<String>;

    fn plant(&mut self, terminal: &mut dyn Terminal) -> RootsResult<Option<String>>;

    fn tally(&self, summary: &SessionSummary) -> String;
}

pub struct FlowDeps {
    pub paths: RootsPaths,
    pub config: RootsConfig,
    pub by: String,
    pub colors: Colors,
    pub environment: Environment,
    pub start: Option<String>,
    pub refresh_ms: Option<u64>,
    pub agent_timeout_ms: Option<u64>,
}

pub struct FlowSummary {
    pub flow: String,
    pub trail: FlowTrail,
    pub pending: Vec<BackgroundJob>,
    pub running: Vec<String>,
    pub problems: Vec<String>,
}

impl FlowSummary {
    /// Await detached agent runs after Flow has recorded flow.end. This mirrors
    /// the command's final status spinner and updates the returned problems.
    pub fn wait_for_background(&mut self) {
        for job in self.pending.drain(..) {
            if let Ok(Some(problem)) = job.done.recv() {
                self.problems.push(problem);
            }
        }
    }
}

#[derive(Clone, Debug)]
enum Event {
    Input(String),
    Resize,
}

struct TrackedJob {
    label: String,
    heading: bool,
    done: Receiver<Option<String>>,
}

struct Live {
    flow: String,
    started: Instant,
    card: FlowCard,
    jobs: Vec<TrackedJob>,
    problems: Vec<String>,
    preferred: Option<String>,
    last_frame: String,
    reported: Arc<Mutex<Vec<BackgroundJob>>>,
}

fn now() -> OffsetDateTime {
    OffsetDateTime::now_utc()
}

fn new_flow_id(paths: &RootsPaths) -> RootsResult<String> {
    let events = read_events(paths)?;
    let taken = events
        .iter()
        .filter_map(|event| event.extra.get("flow").and_then(Value::as_str))
        .collect::<Vec<_>>();
    Ok(generate_id(IdPrefix::Flow, &hex_set(taken)))
}

fn attach(terminal: &mut dyn Terminal, sender: &Sender<Event>) {
    let inputs = sender.clone();
    let resizes = sender.clone();
    let input: InputCallback = Box::new(move |data| {
        let _ = inputs.send(Event::Input(data));
    });
    let resize: ResizeCallback = Box::new(move || {
        let _ = resizes.send(Event::Resize);
    });
    terminal.start(input, Some(resize));
}

fn enqueue_job(live: &mut Live, job: BackgroundJob, heading: bool) {
    live.jobs.push(TrackedJob {
        label: job.label,
        heading,
        done: job.done,
    });
}

fn ingest_reported(live: &mut Live) {
    let mut jobs = live
        .reported
        .lock()
        .map(|mut queued| std::mem::take(&mut *queued))
        .unwrap_or_default();
    for job in jobs.drain(..) {
        enqueue_job(live, job, false);
    }
}

fn harvest_jobs(live: &mut Live) -> bool {
    let mut changed = false;
    let mut pending = Vec::new();
    for job in live.jobs.drain(..) {
        match job.done.try_recv() {
            Ok(problem) => {
                changed = true;
                if let Some(problem) = problem {
                    live.problems.push(problem.clone());
                    live.card.notice = Some(format!("! {problem} ({})", job.label));
                }
            }
            Err(TryRecvError::Empty) => pending.push(job),
            Err(TryRecvError::Disconnected) => {
                changed = true;
                let problem = "agent job ended before reporting".to_owned();
                live.problems.push(problem.clone());
                live.card.notice = Some(format!("! {problem} ({})", job.label));
            }
        }
    }
    live.jobs = pending;
    changed
}

fn heading_view(deps: &FlowDeps, live: &Live, trail: &FlowTrail) -> RootsResult<HeadingView> {
    if let Some(heading) = active_heading(&read_headings(&deps.paths)?, &live.flow) {
        return Ok(HeadingView::Shown(Box::new(heading)));
    }
    if live.jobs.iter().any(|job| job.heading) {
        return Ok(HeadingView::Waiting);
    }
    if !heading_enabled(&deps.config) || trail.entries.is_empty() {
        Ok(HeadingView::Off)
    } else {
        Ok(HeadingView::None)
    }
}

fn refresh_card(deps: &FlowDeps, live: &mut Live) -> RootsResult<()> {
    ingest_reported(live);
    let graph = read_graph(&deps.paths)?;
    let trail = build_trail(
        &read_events(&deps.paths)?,
        &graph,
        &read_questions(&deps.paths)?,
        &live.flow,
        now(),
    );
    let heading = heading_view(deps, live, &trail)?;
    let ranked = rank_attention(collect_attention(&deps.paths, &graph, &deps.config, now())?);
    let touched = trail
        .entries
        .iter()
        .map(|entry| entry.id.clone())
        .collect::<BTreeSet<_>>();
    let next = match &heading {
        HeadingView::Shown(heading) => heading.next.as_deref(),
        _ => None,
    };
    let mut picks = rank_picks(&ranked, &touched, &graph, next);
    if let Some(preferred) = &live.preferred {
        if let Some(node) = find_node(&graph, preferred) {
            if !touched.contains(&node.id) {
                picks.retain(|pick| pick.id != node.id);
                picks.insert(
                    0,
                    super::flow::FlowPick {
                        id: node.id.clone(),
                        slug: node.slug.clone(),
                        reason: "just planted".to_owned(),
                        from_heading: false,
                    },
                );
            }
        }
    }
    let selected = (live.card.pick > 0)
        .then(|| {
            live.card
                .picks
                .get(live.card.pick)
                .map(|pick| pick.id.clone())
        })
        .flatten();
    let pick = selected
        .as_deref()
        .and_then(|id| picks.iter().position(|candidate| candidate.id == id))
        .unwrap_or(0);
    live.card.trail = trail;
    live.card.heading = heading;
    live.card.jobs = live.jobs.iter().map(|job| job.label.clone()).collect();
    live.card.inbox = collect_cards(&deps.paths, now())?.len();
    live.card.picks = picks;
    live.card.pick = pick;
    Ok(())
}

fn render(deps: &FlowDeps, live: &mut Live, terminal: &mut dyn Terminal) {
    let minutes = live.started.elapsed().as_secs() / 60;
    let lines = render_flow_card(
        &live.card,
        &FlowFrame {
            project: deps.config.project.clone(),
            minutes,
        },
        terminal.columns(),
        deps.colors,
        terminal.rows(),
    );
    let text = frame(&lines);
    if text != live.last_frame {
        live.last_frame = text.clone();
        terminal.write(&text);
    }
}

fn refresh(deps: &FlowDeps, live: &mut Live, terminal: &mut dyn Terminal) {
    if let Err(problem) = refresh_card(deps, live) {
        live.card.notice = Some(format!("! refresh failed: {}", problem.message));
    }
    render(deps, live, terminal);
}

fn resume_card(
    deps: &FlowDeps,
    live: &mut Live,
    terminal: &mut dyn Terminal,
    sender: &Sender<Event>,
) {
    live.last_frame.clear();
    attach(terminal, sender);
    refresh(deps, live, terminal);
}

fn after_session(deps: &FlowDeps, live: &mut Live, summary: &SessionSummary) -> RootsResult<()> {
    let graph = read_graph(&deps.paths)?;
    let node = find_node(&graph, &summary.node).cloned();
    if let (Some(node), Some(changed)) = (node, summary.changed_lines.clone()) {
        if let Some(job) = start_session_proposals(
            deps.paths.clone(),
            deps.config.clone(),
            node,
            summary.session.clone(),
            changed,
            deps.environment.clone(),
            deps.agent_timeout_ms,
        )? {
            enqueue_job(live, job, false);
        }
    }
    if !heading_enabled(&deps.config) {
        return Ok(());
    }
    let trail = build_trail(
        &read_events(&deps.paths)?,
        &read_graph(&deps.paths)?,
        &read_questions(&deps.paths)?,
        &live.flow,
        now(),
    );
    if let Some(job) = start_heading_phase(HeadingJobInput {
        paths: deps.paths.clone(),
        config: deps.config.clone(),
        flow: live.flow.clone(),
        trail,
        environment: deps.environment.clone(),
        timeout_ms: deps.agent_timeout_ms,
        now: now(),
    })? {
        enqueue_job(live, job, true);
    }
    Ok(())
}

fn do_think(
    deps: &FlowDeps,
    live: &mut Live,
    terminal: &mut dyn Terminal,
    sender: &Sender<Event>,
    steps: &mut dyn FlowSteps,
    id: String,
) -> RootsResult<()> {
    terminal.stop();
    let reporter: BackgroundReporter = {
        let reported = Arc::clone(&live.reported);
        Arc::new(move |job| {
            if let Ok(mut jobs) = reported.lock() {
                jobs.push(job);
            }
        })
    };
    let result = steps.think(terminal, &id, reporter);
    resume_card(deps, live, terminal, sender);
    let summary = result?;
    ingest_reported(live);
    if live.preferred.as_deref() == Some(id.as_str()) {
        live.preferred = None;
    }
    let slug = find_node(&read_graph(&deps.paths)?, &id)
        .map(|node| node.slug.clone())
        .unwrap_or(id);
    live.card.notice = Some(format!("{slug} done · {}", steps.tally(&summary)));
    after_session(deps, live, &summary)?;
    refresh(deps, live, terminal);
    Ok(())
}

fn perform(
    deps: &FlowDeps,
    live: &mut Live,
    terminal: &mut dyn Terminal,
    sender: &Sender<Event>,
    steps: &mut dyn FlowSteps,
    action: FlowAction,
) -> RootsResult<()> {
    if !matches!(action, FlowAction::Dismiss { .. }) {
        live.card.pick = 0;
        live.card.picks.clear();
    }
    match action {
        FlowAction::Think { id } => do_think(deps, live, terminal, sender, steps, id),
        FlowAction::Tend => {
            terminal.stop();
            let result = steps.tend(terminal);
            resume_card(deps, live, terminal, sender);
            live.card.notice = Some(result?);
            refresh(deps, live, terminal);
            Ok(())
        }
        FlowAction::Plant => {
            terminal.stop();
            let result = steps.plant(terminal);
            resume_card(deps, live, terminal, sender);
            let id = result?;
            if let Some(id) = &id {
                live.preferred = Some(id.clone());
                live.card.notice = Some(format!("planted {id}"));
            } else {
                live.card.notice = Some("nothing planted".to_owned());
            }
            refresh(deps, live, terminal);
            Ok(())
        }
        FlowAction::Dismiss { heading } => {
            dismiss_heading(&deps.paths, &heading, &deps.by, Some(now()))?;
            live.card.notice = Some("heading dismissed; the agent won't say it again".to_owned());
            refresh(deps, live, terminal);
            Ok(())
        }
        FlowAction::Quit => Ok(()),
    }
}

fn flow_extra(flow: &str) -> roots_core::serde_json::Map<String, Value> {
    let mut extra = roots_core::serde_json::Map::new();
    extra.insert("flow".to_owned(), Value::String(flow.to_owned()));
    extra
}

pub fn run_flow(
    deps: FlowDeps,
    terminal: &mut dyn Terminal,
    steps: &mut dyn FlowSteps,
) -> RootsResult<FlowSummary> {
    let flow = new_flow_id(&deps.paths)?;
    append_event(
        &deps.paths,
        &make_event(
            "flow.start",
            &deps.by,
            EventFields {
                extra: flow_extra(&flow),
                ..Default::default()
            },
        ),
    )?;
    let (sender, receiver) = mpsc::channel();
    let mut live = Live {
        flow: flow.clone(),
        started: Instant::now(),
        card: initial_card(),
        jobs: Vec::new(),
        problems: Vec::new(),
        preferred: None,
        last_frame: String::new(),
        reported: Arc::new(Mutex::new(Vec::new())),
    };
    let result = (|| -> RootsResult<()> {
        attach(terminal, &sender);
        refresh(&deps, &mut live, terminal);
        if let Some(id) = deps.start.clone() {
            if let Err(problem) = do_think(&deps, &mut live, terminal, &sender, steps, id) {
                live.card.notice = Some(format!("! {}", problem.message));
                refresh(&deps, &mut live, terminal);
            }
        }
        let wait = Duration::from_millis(deps.refresh_ms.unwrap_or(1_000).max(1));
        while !live.card.ended {
            if harvest_jobs(&mut live) {
                refresh(&deps, &mut live, terminal);
            }
            match receiver.recv_timeout(wait) {
                Ok(Event::Resize) => {
                    live.last_frame.clear();
                    refresh(&deps, &mut live, terminal);
                }
                Ok(Event::Input(data)) => {
                    for key in parse_flow_keys(&data) {
                        if live.card.ended {
                            break;
                        }
                        let step = flow_key(live.card.clone(), key);
                        live.card = step.card;
                        match step.action {
                            Some(FlowAction::Quit) => break,
                            Some(action) => {
                                if let Err(problem) =
                                    perform(&deps, &mut live, terminal, &sender, steps, action)
                                {
                                    live.card.notice = Some(format!("! {}", problem.message));
                                    refresh(&deps, &mut live, terminal);
                                }
                            }
                            None => render(&deps, &mut live, terminal),
                        }
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => refresh(&deps, &mut live, terminal),
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(RootsError::error("interactive terminal input closed"));
                }
            }
        }
        Ok(())
    })();
    terminal.stop();
    let end = append_event(
        &deps.paths,
        &make_event(
            "flow.end",
            &deps.by,
            EventFields {
                at: Some(iso_at(now())),
                extra: flow_extra(&flow),
                ..Default::default()
            },
        ),
    );
    result?;
    end?;
    ingest_reported(&mut live);
    let _ = harvest_jobs(&mut live);
    let graph = read_graph(&deps.paths)?;
    let trail = build_trail(
        &read_events(&deps.paths)?,
        &graph,
        &read_questions(&deps.paths)?,
        &flow,
        now(),
    );
    let running = live
        .jobs
        .iter()
        .map(|job| job.label.clone())
        .collect::<Vec<_>>();
    let pending = live
        .jobs
        .drain(..)
        .map(|job| BackgroundJob {
            label: job.label,
            done: job.done,
        })
        .collect();
    Ok(FlowSummary {
        flow,
        trail,
        pending,
        running,
        problems: live.problems,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands_capture::plant_text;
    use crate::interactive::session::SessionTally;
    use crate::interactive::terminal::{FakeTerminal, FakeTerminalHandle};
    use roots_core::config::{default_config, write_config};
    use roots_core::events::{append_event, make_event, read_events};
    use roots_core::headings::{HeadingInput, file_heading, read_headings};
    use roots_core::model::Citation;
    use roots_core::paths::roots_paths;
    use roots_core::proposals::read_proposals;
    use roots_core::store::write_jsonl_file;
    use roots_core::time_crate::OffsetDateTime;
    use std::fs;
    use std::path::Path;
    use std::sync::{Arc, Mutex};
    use std::thread;
    use std::time::{Duration, Instant};

    fn sandbox(label: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!(
            "roots-flow-session-{label}-{}",
            roots_core::ids::random_hex(8)
        ));
        fs::create_dir_all(&root).unwrap();
        root
    }

    fn initialize(root: &Path) -> RootsPaths {
        let paths = roots_paths(root);
        fs::create_dir_all(&paths.human).unwrap();
        fs::create_dir_all(&paths.agent).unwrap();
        fs::create_dir_all(&paths.sprouts).unwrap();
        fs::create_dir_all(&paths.notes).unwrap();
        write_config(&paths, &default_config("flow-test")).unwrap();
        for file in [
            &paths.graph,
            &paths.events,
            &paths.questions,
            &paths.proposals,
            &paths.headings,
        ] {
            fs::write(file, "").unwrap();
        }
        paths
    }

    fn wait_for(control: &FakeTerminalHandle, needle: &str) {
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline {
            if control.screen().contains(needle) {
                return;
            }
            thread::sleep(Duration::from_millis(5));
        }
        panic!("did not see {needle:?}; screen was:\n{}", control.screen());
    }

    fn summary(node: &str) -> SessionSummary {
        SessionSummary {
            session: "s-1111".to_owned(),
            node: node.to_owned(),
            reason: "quit".to_owned(),
            questions: Vec::new(),
            tally: SessionTally::default(),
            changed: false,
            hash: "sha256:test".to_owned(),
            status_change: None,
            mentions: None,
            mention_error: None,
            agent: super::super::think_session::AgentSummary {
                mode: "off".to_owned(),
                tier: 0,
                actor: None,
                status: None,
                error: None,
                asked: 0,
                arrived: 0,
            },
            proposals: None,
            delegation: None,
            changed_lines: Some(Vec::new()),
        }
    }

    struct Steps {
        paths: RootsPaths,
        by: String,
        thoughts: Arc<Mutex<Vec<String>>>,
        plant: bool,
        tended: bool,
    }

    impl FlowSteps for Steps {
        fn think(
            &mut self,
            _terminal: &mut dyn Terminal,
            id: &str,
            background: BackgroundReporter,
        ) -> RootsResult<SessionSummary> {
            self.thoughts.lock().unwrap().push(id.to_owned());
            append_event(
                &self.paths,
                &make_event(
                    "session.end",
                    &self.by,
                    EventFields {
                        node: Some(id.to_owned()),
                        extra: roots_core::serde_json::Map::from_iter([(
                            "answered".to_owned(),
                            Value::Number(0_u64.into()),
                        )]),
                        ..Default::default()
                    },
                ),
            )?;
            let flow = read_events(&self.paths)?
                .iter()
                .find(|event| event.event_type == "flow.start")
                .and_then(|event| event.extra.get("flow"))
                .and_then(Value::as_str)
                .unwrap()
                .to_owned();
            let graph = read_graph(&self.paths)?;
            let _node = find_node(&graph, id).unwrap();
            file_heading(
                &self.paths,
                &default_config("flow-test"),
                HeadingInput {
                    flow: &flow,
                    text: "Stay with this idea.",
                    cites: vec![Citation {
                        node: id.to_owned(),
                        quote: "Keep the first idea in view.".to_owned(),
                    }],
                    next: None,
                    by: "agent:test",
                    now: Some(OffsetDateTime::now_utc()),
                },
            )?;
            let (sender, done) = mpsc::channel();
            sender
                .send(Some("fixture agent failed".to_owned()))
                .unwrap();
            background(BackgroundJob {
                label: "fixture question run".to_owned(),
                done,
            });
            Ok(summary(id))
        }

        fn tend(&mut self, _terminal: &mut dyn Terminal) -> RootsResult<String> {
            self.tended = true;
            Ok("tended 1: 1 accepted, 0 rejected, 0 skipped".to_owned())
        }

        fn plant(&mut self, _terminal: &mut dyn Terminal) -> RootsResult<Option<String>> {
            self.plant = true;
            Ok(plant_text(
                &self.paths,
                "A second idea keeps the trail moving.\n",
                &self.by,
                None,
            )?
            .map(|result| result.node.id))
        }

        fn tally(&self, _summary: &SessionSummary) -> String {
            "no questions".to_owned()
        }
    }

    #[test]
    fn supplied_id_thinks_before_card_then_dismisses_plants_and_quits() {
        let root = sandbox("lifecycle");
        let paths = initialize(&root);
        let first = plant_text(&paths, "Keep the first idea in view.\n", "human:test", None)
            .unwrap()
            .unwrap();
        let thoughts = Arc::new(Mutex::new(Vec::new()));
        let (terminal, control) = FakeTerminal::new(78, 28);
        let paths_for_thread = paths.clone();
        let thoughts_for_thread = Arc::clone(&thoughts);
        let first_id = first.node.id.clone();
        let join = thread::spawn(move || {
            let mut terminal = terminal;
            let mut steps = Steps {
                paths: paths_for_thread.clone(),
                by: "human:test".to_owned(),
                thoughts: thoughts_for_thread,
                plant: false,
                tended: false,
            };
            let result = run_flow(
                FlowDeps {
                    paths: paths_for_thread,
                    config: default_config("flow-test"),
                    by: "human:test".to_owned(),
                    colors: Colors::new(false),
                    environment: Environment::new(),
                    start: Some(first_id),
                    refresh_ms: Some(20),
                    agent_timeout_ms: Some(10),
                },
                &mut terminal,
                &mut steps,
            );
            (result, steps.plant)
        });
        wait_for(&control, "Stay with this idea.");
        control.send("x");
        wait_for(&control, "heading dismissed");
        control.send("p");
        wait_for(&control, "planted r-");
        control.send("q");
        let (summary, planted) = join.join().unwrap();
        let summary = summary.unwrap();
        assert!(planted);
        assert_eq!(&*thoughts.lock().unwrap(), &[first.node.id]);
        assert_eq!(summary.trail.entries.len(), 1);
        assert_eq!(summary.trail.planted.len(), 1);
        assert!(
            summary
                .problems
                .iter()
                .any(|problem| problem == "fixture agent failed")
        );
        assert!(
            read_headings(&paths)
                .unwrap()
                .iter()
                .all(|heading| heading.status != roots_core::model::HeadingStatus::Active)
        );
        let events = read_events(&paths).unwrap();
        assert_eq!(
            events
                .iter()
                .filter(|event| event.event_type == "flow.start")
                .count(),
            1
        );
        assert_eq!(
            events
                .iter()
                .filter(|event| event.event_type == "flow.end")
                .count(),
            1
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn enter_reviews_the_inbox_before_thinking_a_pick() {
        let root = sandbox("inbox-first");
        let paths = initialize(&root);
        let first = plant_text(&paths, "Keep the first idea in view.\n", "human:test", None)
            .unwrap()
            .unwrap();
        let proposal = roots_core::model::ProposalRecord {
            id: "p-a1b2".to_owned(),
            kind: roots_core::model::ProposalKind::Compost,
            from: Some(first.node.id.clone()),
            to: None,
            rel: None,
            reason: Some("fixture".to_owned()),
            cites: Vec::new(),
            by: "agent:test".to_owned(),
            status: roots_core::model::ProposalStatus::Pending,
            created_at: "2026-01-01T00:00:00Z".to_owned(),
            expires_at: None,
            decided_by: None,
            decided_at: None,
            decision_reason: None,
        };
        write_jsonl_file(&paths.proposals, &[proposal]).unwrap();
        assert_eq!(read_proposals(&paths).unwrap().len(), 1);
        let (terminal, control) = FakeTerminal::new(78, 28);
        let paths_for_thread = paths.clone();
        let thoughts = Arc::new(Mutex::new(Vec::new()));
        let thoughts_for_thread = Arc::clone(&thoughts);
        let join = thread::spawn(move || {
            let mut terminal = terminal;
            let mut steps = Steps {
                paths: paths_for_thread.clone(),
                by: "human:test".to_owned(),
                thoughts: thoughts_for_thread,
                plant: false,
                tended: false,
            };
            let result = run_flow(
                FlowDeps {
                    paths: paths_for_thread,
                    config: default_config("flow-test"),
                    by: "human:test".to_owned(),
                    colors: Colors::new(false),
                    environment: Environment::new(),
                    start: None,
                    refresh_ms: Some(20),
                    agent_timeout_ms: Some(10),
                },
                &mut terminal,
                &mut steps,
            );
            (result, steps.tended)
        });
        wait_for(&control, "review inbox (1)");
        control.send("\r");
        wait_for(&control, "tended 1: 1 accepted");
        control.send("q");
        let (summary, tended) = join.join().unwrap();
        assert!(summary.is_ok());
        assert!(tended);
        assert!(thoughts.lock().unwrap().is_empty());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn no_configured_agent_keeps_flow_headings_off() {
        let config = default_config("flow-test");
        assert!(!heading_enabled(&config));
        let mut disabled = config.clone();
        disabled.agent.command = Some("fixture-agent".to_owned());
        disabled.flow_heading = false;
        assert!(!heading_enabled(&disabled));
    }
}
