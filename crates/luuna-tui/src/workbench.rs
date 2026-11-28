//! The interactive workbench: the terminal session, the token screen, and the event loop that
//! feeds keys and bridge events to [`AppState`] and draws frames on the frame clock.
use crate::{
    app::AppState,
    bridge::Bridge,
    frame::{sync_from, FrameClock},
    input::{Input, INPUT_QUEUE},
    protocol::{safe_text, BridgeEvent, QUEUE_SIZE},
    scroll::ScrollMotion,
    terminal::{draw_frame, install_panic_restore, Shutdown, TerminalGuard},
    theme::Theme,
    ui,
};
use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::{
    style::Modifier,
    widgets::{Block, Borders, Paragraph, Wrap},
};
use std::{
    io::{self, IsTerminal},
    time::{Duration, Instant},
};

/// Shutdown marker/signal check cadence; it never draws.
const SHUTDOWN_POLL: Duration = Duration::from_millis(100);

/// Opens the workbench on this terminal (optionally straight into the setup guide) and runs it
/// until the developer quits; the exit code is 0 on a normal quit.
pub async fn run(guide: Option<Vec<(String, String)>>) -> io::Result<u8> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        eprintln!("The Luuna workbench needs a terminal.");
        return Ok(2);
    }
    install_panic_restore();
    let shutdown = Shutdown::install()?;
    let settings = crate::config::load_and_install();
    let guard = TerminalGuard::enter_with(settings.keyboard_enhancement)?;
    // Declared after `guard` so early returns join the input thread before restoration.
    let mut input = Input::spawn()?;
    let mut terminal = guard.screen()?;
    let sync = sync_from(std::env::var("LUUNA_TUI_SYNC").ok().as_deref());
    let mut clock = FrameClock::from_env();
    let theme = Theme::from_env();
    let mut token = std::env::var("LUUNA_API_TOKEN").unwrap_or_default();
    if token.trim().is_empty() {
        match secret(
            &mut terminal,
            theme,
            &shutdown,
            &mut input,
            sync,
            &mut clock,
        )
        .await?
        {
            Some(value) => token = value,
            None => {
                drop(input);
                drop(terminal);
                drop(guard);
                return Ok(0);
            }
        }
    }
    let url = std::env::var("LUUNA_URL").ok();
    let mut bridge = Bridge::start(&token, url.as_deref());
    token.clear();
    let mut app = AppState::default();
    app.set_scroll_motion(ScrollMotion::from_env());
    app.start();
    if !settings.warnings.is_empty() {
        let shown = settings
            .path
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        app.status = safe_text(&format!(
            "Settings {shown}: {}",
            settings.warnings.join("; ")
        ));
    }
    if let Some(flags) = guide {
        let flags = flags
            .into_iter()
            .map(|(k, v)| (k, serde_json::Value::String(v)))
            .collect();
        crate::views::guide::begin(&mut app, "init", flags);
    }
    let mut transport_stopped = false;
    let mut bridge_open = true;
    clock.mark_dirty();
    let mut shutdown_tick = tokio::time::interval(SHUTDOWN_POLL);
    shutdown_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let result = loop {
        if shutdown.requested() {
            break Ok(0);
        }
        // The guide asked for an editor: hand it the terminal, then take the terminal back.
        if let Some((prompt_id, path)) = app.take_editor() {
            input.pause();
            let saved = crate::terminal::suspend().is_ok()
                && tokio::task::block_in_place(|| run_editor(&path));
            if let Err(error) = crate::terminal::resume(&mut terminal) {
                break Err(error);
            }
            if let Err(error) = input.resume() {
                break Err(error);
            }
            app.editor_closed(prompt_id, saved);
            clock.mark_dirty();
        }
        while let Some(request) = app.queue.pop_front() {
            clock.mark_dirty();
            if let Err(error) = bridge.send(&request) {
                app.transport_failed(&error);
                break;
            }
        }
        if app.bridge_failed && !transport_stopped {
            bridge.shutdown().await;
            transport_stopped = true;
            clock.mark_dirty();
        }
        if app.animating() {
            clock.request_animation_frame();
        }
        let now = Instant::now();
        if clock.should_draw(now) {
            app.advance_animations(clock.frame_delta(now));
            if let Err(error) = draw_frame(&mut terminal, sync, |frame| {
                ui::render(frame, &mut app, theme)
            }) {
                break Err(error);
            }
            clock.frame_drawn(now);
            // A drawn frame clears the request; an animation still running asks for the next one.
            if app.animating() {
                clock.request_animation_frame();
            }
        }
        if app.quit {
            break Ok(0);
        }
        let deadline = clock
            .next_deadline(Instant::now())
            .map(tokio::time::Instant::from_std);
        tokio::select! {
            biased;
            event = input.recv() => match event {
                Some(Ok(event)) => {
                    handle_input(&mut app, &mut clock, event);
                    let mut failed = None;
                    for _ in 0..INPUT_QUEUE {
                        match input.try_recv() {
                            Some(Ok(event)) => handle_input(&mut app, &mut clock, event),
                            Some(Err(error)) => {
                                failed = Some(error);
                                break;
                            }
                            None => break,
                        }
                    }
                    if let Some(error) = failed {
                        break Err(error);
                    }
                }
                Some(Err(error)) => break Err(error),
                None => break Err(io::Error::other("Terminal input closed")),
            },
            event = bridge.recv(), if bridge_open && !transport_stopped => {
                clock.mark_dirty();
                match event {
                    Some(first) => drain(first, || bridge.try_recv(), |result| apply_bridge(&mut app, result)),
                    None => {
                        bridge_open = false;
                        if !app.bridge_failed {
                            app.transport_failed("Bridge event channel closed. Inspect runtime state before retrying an in-flight mutation.");
                        }
                    }
                }
            },
            _ = tokio::time::sleep_until(deadline.unwrap_or_else(tokio::time::Instant::now)), if deadline.is_some() => {},
            _ = shutdown_tick.tick() => {},
        }
    };
    bridge.shutdown().await;
    drop(input);
    drop(terminal);
    drop(guard);
    result
}

/// Runs $VISUAL, $EDITOR or the platform default on the guide's draft file and waits for it.
fn run_editor(path: &str) -> bool {
    let file = std::path::Path::new(path);
    if !file.is_file() || path.contains('"') {
        return false;
    }
    let chosen = ["VISUAL", "EDITOR"]
        .iter()
        .filter_map(|name| std::env::var(name).ok())
        .find(|value| !value.trim().is_empty());
    let status = match chosen {
        Some(editor) if cfg!(windows) => std::process::Command::new("cmd")
            .args(["/C", &format!("{editor} \"{path}\"")])
            .status(),
        Some(editor) => std::process::Command::new("sh")
            .args(["-c", &format!("{editor} \"$1\""), "editor", path])
            .status(),
        None if cfg!(windows) => std::process::Command::new("notepad.exe").arg(file).status(),
        None => std::process::Command::new("vi").arg(file).status(),
    };
    status.is_ok_and(|status| status.success())
}
fn handle_input(app: &mut AppState, clock: &mut FrameClock, event: Event) {
    match event {
        Event::Key(key) if key.kind != KeyEventKind::Release => {
            app.key(key);
            clock.mark_dirty();
        }
        Event::Paste(text) => {
            app.paste(&text);
            clock.mark_dirty();
        }
        Event::Resize(..) => clock.mark_dirty(),
        _ => {}
    }
}
/// Applies `first` and at most `QUEUE_SIZE` more queued events, so terminal input still runs when
/// the producer is busy. An event leaves the channel only to be applied; the rest wait for the next
/// wake, and applying stops at the first event that ends the transport.
fn drain<E>(first: E, mut next: impl FnMut() -> Option<E>, mut apply: impl FnMut(E) -> bool) {
    if !apply(first) {
        return;
    }
    for _ in 0..QUEUE_SIZE {
        let Some(event) = next() else { break };
        if !apply(event) {
            break;
        }
    }
}
/// Returns false when the transport failed and draining should stop.
fn apply_bridge(app: &mut AppState, result: Result<BridgeEvent, String>) -> bool {
    match result {
        Ok(event) => {
            app.apply(event);
            true
        }
        Err(error) => {
            app.transport_failed(&error);
            false
        }
    }
}
async fn secret(
    terminal: &mut crate::terminal::Screen,
    theme: Theme,
    shutdown: &Shutdown,
    input: &mut Input,
    sync: bool,
    clock: &mut FrameClock,
) -> io::Result<Option<String>> {
    let mut value = String::new();
    let mut shutdown_tick = tokio::time::interval(SHUTDOWN_POLL);
    shutdown_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    clock.mark_dirty();
    loop {
        if shutdown.requested() {
            return Ok(None);
        }
        let now = Instant::now();
        if clock.should_draw(now) {
            draw_frame(terminal, sync, |frame| {
                let text=format!("LUUNA / LOCAL RUNTIME\n\nService token (hidden): {}\n\nEnter connects · Esc quits\nThe token remains in process memory and the owned bridge environment.","•".repeat(value.chars().count().min(32)));
                frame.render_widget(
                    Paragraph::new(text)
                        .block(Block::default().borders(Borders::ALL).title(" Connection "))
                        .style(theme.panel().add_modifier(Modifier::BOLD))
                        .wrap(Wrap { trim: false }),
                    frame.area(),
                );
            })?;
            clock.frame_drawn(now);
        }
        let deadline = clock
            .next_deadline(Instant::now())
            .map(tokio::time::Instant::from_std);
        let event = tokio::select! {
            biased;
            event = input.recv() => match event {
                Some(event) => event?,
                None => return Err(io::Error::other("Terminal input closed")),
            },
            _ = tokio::time::sleep_until(deadline.unwrap_or_else(tokio::time::Instant::now)), if deadline.is_some() => continue,
            _ = shutdown_tick.tick() => continue,
        };
        match event {
            Event::Key(key) if key.kind != KeyEventKind::Release => {
                clock.mark_dirty();
                match key.code {
                    KeyCode::Esc => return Ok(None),
                    KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        return Ok(None)
                    }
                    KeyCode::Enter if !value.trim().is_empty() => return Ok(Some(value)),
                    KeyCode::Backspace => {
                        value.pop();
                    }
                    KeyCode::Char(c) if crate::keys::is_text(&key) && value.len() < 8192 => {
                        value.push(c)
                    }
                    _ => {}
                }
            }
            Event::Paste(text) => {
                clock.mark_dirty();
                let text = safe_text(&text);
                if !text.contains('\n') && value.len() + text.len() <= 8192 {
                    value.push_str(&text);
                }
            }
            Event::Resize(..) => clock.mark_dirty(),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    #[test]
    fn every_queued_bridge_event_is_applied_in_order_across_wakes() {
        for total in [1, QUEUE_SIZE, QUEUE_SIZE + 1, QUEUE_SIZE + 2, 100] {
            let mut queue: VecDeque<usize> = (0..total).collect();
            let mut applied = Vec::new();
            while let Some(first) = queue.pop_front() {
                let before = applied.len();
                drain(
                    first,
                    || queue.pop_front(),
                    |event| {
                        applied.push(event);
                        true
                    },
                );
                assert!(applied.len() - before <= QUEUE_SIZE + 1, "bounded per wake");
            }
            assert_eq!(applied, (0..total).collect::<Vec<_>>(), "{total} events");
        }
    }

    #[test]
    fn draining_stops_at_an_event_that_ends_the_transport_and_takes_nothing_more() {
        let mut queue: VecDeque<usize> = (1..10).collect();
        let mut applied = Vec::new();
        drain(
            0,
            || queue.pop_front(),
            |event| {
                applied.push(event);
                event != 3
            },
        );
        assert_eq!(applied, vec![0, 1, 2, 3]);
        assert_eq!(
            queue.front(),
            Some(&4),
            "nothing past the stop was dequeued"
        );
    }
}
