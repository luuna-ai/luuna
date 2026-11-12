//! Terminal boundary for one `tend` card at a time.
//!
//! Ordinary decisions keep the raw/alternate terminal session alive while the
//! command applies their effect and draws the next card.  Only `$EDITOR` and
//! nested `think` handoffs stop that session, then restart it on return.

use super::tend_render::{TendView, render_tend};
use super::terminal::{Terminal, frame};
use roots_core::color::Colors;
use roots_core::errors::{RootsError, RootsResult};
use roots_core::graph::{find_node, read_graph};
use roots_core::model::{NodeRecord, ProposalRecord};
use roots_core::paths::RootsPaths;
use roots_core::proposals::{expire_proposals, pending_proposals, read_proposals};
use roots_core::prose::read_node_prose;
use roots_core::sprouts::{expire_sprouts, open_sprouts};
use roots_core::tend::{
    CardNode, TendCard, TendEffect, TendInput, TendState, initial_tend, proposal_cards, sort_cards,
    sprout_card, tend_step,
};
use roots_core::time_crate::OffsetDateTime;
use std::sync::mpsc::{self, Receiver, Sender};

#[derive(Clone, Debug)]
pub struct TendStart {
    pub expired: Vec<ProposalRecord>,
    pub expired_sprouts: Vec<NodeRecord>,
    pub state: TendState,
}

/// Collect cards without expiring or mutating any store rows.
pub fn collect_cards(paths: &RootsPaths, now: OffsetDateTime) -> RootsResult<Vec<TendCard>> {
    let graph = read_graph(paths)?;
    let proposals = read_proposals(paths)?;
    let pending = pending_proposals(&proposals, now, Some(&graph));
    let mut cards = proposal_cards(&pending, |id| CardNode {
        id: id.to_owned(),
        slug: find_node(&graph, id)
            .map(|node| node.slug.clone())
            .unwrap_or_else(|| id.to_owned()),
    })
    .into_iter()
    .map(TendCard::Proposal)
    .collect::<Vec<_>>();
    for node in open_sprouts(&graph, now) {
        let prose = read_node_prose(paths, node, None)?;
        cards.push(sprout_card(
            node.id.clone(),
            node.slug.clone(),
            node.author.clone(),
            node.expires_at.clone(),
            prose.statement,
            prose.body,
        ));
    }
    Ok(sort_cards(cards))
}

/// Expire before constructing cards, exactly as an interactive `tend` pass
/// does.  The JSON path intentionally calls [`collect_cards`] directly.
pub fn start_tend(paths: &RootsPaths, now: OffsetDateTime) -> RootsResult<TendStart> {
    let expired = expire_proposals(paths, Some(now))?;
    let expired_sprouts = expire_sprouts(paths, Some(now))?;
    let cards = collect_cards(paths, now)?;
    Ok(TendStart {
        expired,
        expired_sprouts,
        state: initial_tend(cards),
    })
}

enum Event {
    Input(String),
    Resize,
}

fn draw(
    terminal: &mut dyn Terminal,
    state: &TendState,
    expired: usize,
    expired_sprouts: usize,
    colors: Colors,
    now: OffsetDateTime,
) {
    terminal.write(&frame(&render_tend(
        TendView {
            state,
            now,
            expired,
            expired_sprouts,
        },
        terminal.columns(),
        colors,
        terminal.rows(),
    )));
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TerminalTurn {
    /// A command side effect has been selected. Ordinary effects retain the
    /// raw terminal; editor/think handoffs stop and later restart it.
    Effect(Box<TendEffect>),
    /// The person quit or reached the end of the cards.
    Finished,
}

/// One raw terminal lifetime.  It remains active across ordinary decisions;
/// only explicit editor / nested-think handoffs call [`Self::stop`] and
/// [`Self::restart`].
pub struct TendDriver {
    sender: Sender<Event>,
    receiver: Receiver<Event>,
    active: bool,
}

impl TendDriver {
    fn attach(&mut self, terminal: &mut dyn Terminal) {
        let input_sender = self.sender.clone();
        let resize_sender = self.sender.clone();
        terminal.start(
            Box::new(move |data| {
                let _ = input_sender.send(Event::Input(data));
            }),
            Some(Box::new(move || {
                let _ = resize_sender.send(Event::Resize);
            })),
        );
        self.active = true;
    }

    /// Enter raw mode once and draw the initial review card.
    #[must_use]
    pub fn start(
        terminal: &mut dyn Terminal,
        state: &TendState,
        expired: usize,
        expired_sprouts: usize,
        colors: Colors,
        now: OffsetDateTime,
    ) -> Self {
        let (sender, receiver) = mpsc::channel();
        let mut driver = Self {
            sender,
            receiver,
            active: false,
        };
        driver.attach(terminal);
        driver.render(terminal, state, expired, expired_sprouts, colors, now);
        driver
    }

    /// Redraw without changing the terminal mode.
    pub fn render(
        &self,
        terminal: &mut dyn Terminal,
        state: &TendState,
        expired: usize,
        expired_sprouts: usize,
        colors: Colors,
        now: OffsetDateTime,
    ) {
        draw(terminal, state, expired, expired_sprouts, colors, now);
    }

    /// Leave raw/alternate-screen mode for an editor or nested session.
    pub fn stop(&mut self, terminal: &mut dyn Terminal) {
        if self.active {
            terminal.stop();
            self.active = false;
        }
    }

    /// Resume the same input receiver after a handoff completes.
    pub fn restart(
        &mut self,
        terminal: &mut dyn Terminal,
        state: &TendState,
        expired: usize,
        expired_sprouts: usize,
        colors: Colors,
        now: OffsetDateTime,
    ) {
        self.attach(terminal);
        self.render(terminal, state, expired, expired_sprouts, colors, now);
    }

    /// Wait for a card decision without leaving raw mode for normal keys.
    pub fn next(
        &mut self,
        terminal: &mut dyn Terminal,
        state: &mut TendState,
        expired: usize,
        expired_sprouts: usize,
        colors: Colors,
        now: OffsetDateTime,
    ) -> RootsResult<TerminalTurn> {
        debug_assert!(
            self.active,
            "tend terminal must be active while reading keys"
        );
        loop {
            let event = self
                .receiver
                .recv()
                .map_err(|_| RootsError::error("interactive terminal input closed"));
            let event = match event {
                Ok(event) => event,
                Err(error) => {
                    self.stop(terminal);
                    return Err(error);
                }
            };
            match event {
                Event::Resize => {
                    self.render(terminal, state, expired, expired_sprouts, colors, now)
                }
                Event::Input(data) => {
                    let step = tend_step(state.clone(), TendInput::Data(data));
                    *state = step.state;
                    self.render(terminal, state, expired, expired_sprouts, colors, now);
                    if let Some(effect) = step.effects.into_iter().next() {
                        return Ok(TerminalTurn::Effect(Box::new(effect)));
                    }
                    if state.ended {
                        self.stop(terminal);
                        return Ok(TerminalTurn::Finished);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interactive::terminal::FakeTerminal;
    use roots_core::model::{Citation, EdgeRel, ProposalKind, ProposalStatus};
    use roots_core::tend::{CardNode, proposal_cards};
    use std::thread;

    fn state() -> TendState {
        let proposal = ProposalRecord {
            id: "p-0001".to_owned(),
            kind: ProposalKind::Edge,
            from: Some("r-a".to_owned()),
            to: Some("r-b".to_owned()),
            rel: Some(EdgeRel::Tension),
            reason: Some("why".to_owned()),
            cites: vec![Citation {
                node: "r-a".to_owned(),
                quote: "a".to_owned(),
            }],
            by: "agent:test".to_owned(),
            status: ProposalStatus::Pending,
            created_at: "2026-01-01T00:00:00Z".to_owned(),
            expires_at: None,
            decided_by: None,
            decided_at: None,
            decision_reason: None,
        };
        initial_tend(
            proposal_cards(&[&proposal], |id| CardNode {
                id: id.to_owned(),
                slug: id.to_owned(),
            })
            .into_iter()
            .map(TendCard::Proposal)
            .collect(),
        )
    }

    #[test]
    fn fake_terminal_keeps_raw_mode_for_an_ordinary_effect() {
        let (terminal, control) = FakeTerminal::new(70, 24);
        let now = OffsetDateTime::now_utc();
        let join = thread::spawn(move || {
            let mut terminal = terminal;
            let mut state = state();
            let mut driver =
                TendDriver::start(&mut terminal, &state, 0, 0, Colors::new(false), now);
            let turn = driver
                .next(&mut terminal, &mut state, 0, 0, Colors::new(false), now)
                .unwrap();
            (turn, state, terminal, driver)
        });
        while !control.started() {
            thread::yield_now();
        }
        control.resize();
        control.send("y");
        let (turn, state, mut terminal, mut driver) = join.join().unwrap();
        assert!(matches!(turn, TerminalTurn::Effect(_)));
        assert!(state.busy);
        assert!(!control.stopped());
        assert!(control.screen().contains("working…"));
        driver.stop(&mut terminal);
        assert!(control.stopped());
    }
}
