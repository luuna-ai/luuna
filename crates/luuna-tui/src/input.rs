//! Dedicated blocking terminal-input thread. It is the sole consumer of crossterm's
//! global event source. A thread was chosen over `EventStream` because that needs the
//! `event-stream` feature and a stream-extension dependency.
//!
//! crossterm's `use-dev-tty` feature is turned on in `Cargo.toml` because, on Unix, its
//! event source then polls the terminal on every call and reads it before the resize
//! signal, so a key typed during a resize is delivered at once instead of waiting for
//! the next key.
//!
//! Dropping `Input` stops and joins the thread so no input is consumed after the
//! terminal is restored.

use crossterm::event::{self, Event, KeyEventKind};
use std::{
    io,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread::JoinHandle,
    time::Duration,
};
use tokio::sync::mpsc;

pub const INPUT_QUEUE: usize = 64;
/// Only bounds how quickly the thread notices the stop flag; input is delivered on arrival.
pub const INPUT_POLL: Duration = Duration::from_millis(50);

/// Forward only non-Release keys, paste and resize; mouse/focus events are dropped.
fn forwards(event: &Event) -> bool {
    match event {
        Event::Key(key) => key.kind != KeyEventKind::Release,
        Event::Paste(_) | Event::Resize(..) => true,
        _ => false,
    }
}

pub struct Input {
    receiver: mpsc::Receiver<io::Result<Event>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Input {
    pub fn spawn() -> io::Result<Self> {
        let (sender, receiver) = mpsc::channel(INPUT_QUEUE);
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let thread = std::thread::Builder::new()
            .name("luuna-tui-input".into())
            .spawn(move || {
                while !flag.load(Ordering::Acquire) {
                    match event::poll(INPUT_POLL) {
                        Ok(true) => match event::read() {
                            Ok(event) => {
                                if forwards(&event) && sender.blocking_send(Ok(event)).is_err() {
                                    return;
                                }
                            }
                            Err(error) => {
                                let _ = sender.blocking_send(Err(error));
                                return;
                            }
                        },
                        Ok(false) => {}
                        Err(error) => {
                            let _ = sender.blocking_send(Err(error));
                            return;
                        }
                    }
                }
            })?;
        Ok(Self {
            receiver,
            stop,
            thread: Some(thread),
        })
    }

    /// Stops reading the terminal (and joins the thread) so a child process can own it.
    pub fn pause(&mut self) {
        self.stop.store(true, Ordering::Release);
        self.receiver.close();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }

    /// Reads the terminal again after `pause`, on a fresh thread and queue.
    pub fn resume(&mut self) -> io::Result<()> {
        let fresh = Self::spawn()?;
        drop(std::mem::replace(self, fresh));
        Ok(())
    }

    pub async fn recv(&mut self) -> Option<io::Result<Event>> {
        self.receiver.recv().await
    }

    pub fn try_recv(&mut self) -> Option<io::Result<Event>> {
        self.receiver.try_recv().ok()
    }
}

impl Drop for Input {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        // Unblock a pending blocking_send, then wait for the thread to exit.
        self.receiver.close();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{
        KeyCode, KeyEvent, KeyEventState, KeyModifiers, MouseEvent, MouseEventKind,
    };

    fn key(kind: KeyEventKind) -> Event {
        Event::Key(KeyEvent {
            code: KeyCode::Char('a'),
            modifiers: KeyModifiers::NONE,
            kind,
            state: KeyEventState::NONE,
        })
    }

    #[test]
    fn filter_forwards_only_press_repeat_paste_and_resize() {
        assert!(forwards(&key(KeyEventKind::Press)));
        assert!(forwards(&key(KeyEventKind::Repeat)));
        assert!(!forwards(&key(KeyEventKind::Release)));
        assert!(forwards(&Event::Paste("x".into())));
        assert!(forwards(&Event::Resize(1, 1)));
        assert!(!forwards(&Event::FocusGained));
        assert!(!forwards(&Event::FocusLost));
        assert!(!forwards(&Event::Mouse(MouseEvent {
            kind: MouseEventKind::Moved,
            column: 0,
            row: 0,
            modifiers: KeyModifiers::NONE,
        })));
    }

    /// A thread blocked in `blocking_send` on a full queue must be released by drop.
    #[test]
    fn dropping_input_with_a_full_queue_joins_without_hanging() {
        let (sender, receiver) = mpsc::channel(1);
        sender.try_send(Ok(Event::Resize(1, 1))).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let blocked = std::thread::spawn(move || sender.blocking_send(Ok(Event::Resize(2, 2))));
        std::thread::sleep(Duration::from_millis(50));
        let input = Input {
            receiver,
            stop,
            thread: Some(blocked_as_unit(blocked)),
        };
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            drop(input);
            let _ = done_tx.send(());
        });
        done_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("dropping Input hung while the queue was full");
    }

    fn blocked_as_unit(
        handle: JoinHandle<Result<(), mpsc::error::SendError<io::Result<Event>>>>,
    ) -> JoinHandle<()> {
        std::thread::spawn(move || {
            let _ = handle.join();
        })
    }
}
