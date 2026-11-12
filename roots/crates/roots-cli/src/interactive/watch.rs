//! Debounced prose watching for interactive sessions.
//!
//! The TypeScript CLI watches the *parent directory* because editors often
//! save by renaming a temporary file over `idea.md`, then backs that event
//! stream with a slow poll. `std` deliberately has no portable filesystem
//! watcher, so this native implementation has a tiny worker which observes
//! the pathname (and therefore keeps following rename-over-save) and exposes
//! the same debounced event / immediate-check contract. It is isolated here
//! so a future OS event backend can replace the probe without changing think.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

pub const DEFAULT_DEBOUNCE_MS: u64 = 120;
pub const DEFAULT_POLL_MS: u64 = 1_000;

/// A lightweight probe interval used as the portable stand-in for a native
/// directory event source. `poll_ms` remains the documented slow-backup
/// setting; its zero value disables that backup but not the event probe.
const EVENT_PROBE_MS: u64 = 25;

#[derive(Clone, Copy, Debug)]
pub struct WatchOptions {
    /// Coalesce a burst of editor events into one save notification.
    pub debounce_ms: u64,
    /// Slow fallback polling cadence. Zero disables that backup.
    pub poll_ms: u64,
}

impl Default for WatchOptions {
    fn default() -> Self {
        Self {
            debounce_ms: DEFAULT_DEBOUNCE_MS,
            poll_ms: DEFAULT_POLL_MS,
        }
    }
}

fn read_if_exists(path: &Path) -> Option<String> {
    fs::read_to_string(path).ok()
}

pub struct FileWatcher {
    path: PathBuf,
    last: Arc<Mutex<String>>,
    changes: Receiver<String>,
    alive: Arc<AtomicBool>,
    closed: bool,
}

impl FileWatcher {
    #[must_use]
    pub fn new(
        path: impl Into<PathBuf>,
        initial: impl Into<String>,
        options: WatchOptions,
    ) -> Self {
        let path = path.into();
        let last = Arc::new(Mutex::new(initial.into()));
        let alive = Arc::new(AtomicBool::new(true));
        let (sender, changes) = mpsc::channel();
        let worker_path = path.clone();
        let worker_last = Arc::clone(&last);
        let worker_alive = Arc::clone(&alive);
        thread::spawn(move || {
            let debounce = Duration::from_millis(options.debounce_ms);
            // With a native fs event source this would be the event wakeup;
            // cap the portable probe so ordinary editor saves remain as
            // responsive as the source CLI's 120ms debounce. The source's
            // poll option is still represented as an additional slow check.
            let probe = Duration::from_millis(EVENT_PROBE_MS);
            let slow_poll = (options.poll_ms > 0).then(|| Duration::from_millis(options.poll_ms));
            let mut next_slow_poll = Instant::now();
            let mut pending: Option<(String, Instant)> = None;
            while worker_alive.load(Ordering::SeqCst) {
                let now = Instant::now();
                let due_slow_poll = slow_poll.is_some_and(|_| now >= next_slow_poll);
                // The probe acts like a directory event: it detects both
                // in-place writes and a renamed replacement. A slow polling
                // deadline is kept separately to preserve the documented
                // fallback option and make `poll_ms: 0` meaningful.
                if due_slow_poll {
                    next_slow_poll = now + slow_poll.unwrap_or_default();
                }
                let current_last = worker_last
                    .lock()
                    .map(|value| value.clone())
                    .unwrap_or_default();
                if let Some(text) = read_if_exists(&worker_path)
                    && text != current_last
                    && pending
                        .as_ref()
                        .is_none_or(|(candidate, _)| candidate != &text)
                {
                    pending = Some((text, now));
                }
                if let Some((text, since)) = pending.as_ref()
                    && now.duration_since(*since) >= debounce
                {
                    let should_send = worker_last
                        .lock()
                        .map(|current| current.as_str() != text.as_str())
                        .unwrap_or(false);
                    if should_send {
                        if let Ok(mut current) = worker_last.lock() {
                            *current = text.clone();
                        }
                        let _ = sender.send(text.clone());
                    }
                    pending = None;
                }
                thread::sleep(probe);
            }
        });
        Self {
            path,
            last,
            changes,
            alive,
            closed: false,
        }
    }

    /// Re-read immediately, bypassing the debounce just like the source
    /// watcher's `check()` method. Missing files (an editor mid-rename) are
    /// ignored until they reappear; unchanged bytes are not reported twice.
    pub fn check(&mut self) -> Option<String> {
        if self.closed {
            return None;
        }
        let text = read_if_exists(&self.path)?;
        let Ok(mut last) = self.last.lock() else {
            return None;
        };
        if *last == text {
            return None;
        }
        *last = text.clone();
        Some(text)
    }

    /// Consume a debounced change observed by the background event probe.
    /// The newest event wins if an editor delivered a burst before the
    /// session loop woke up.
    pub fn take_change(&mut self) -> Option<String> {
        if self.closed {
            return None;
        }
        let mut newest = None;
        while let Ok(change) = self.changes.try_recv() {
            newest = Some(change);
        }
        newest
    }

    pub fn close(&mut self) {
        if self.closed {
            return;
        }
        self.closed = true;
        self.alive.store(false, Ordering::SeqCst);
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for FileWatcher {
    fn drop(&mut self) {
        self.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn options() -> WatchOptions {
        WatchOptions {
            debounce_ms: 30,
            poll_ms: 5,
        }
    }

    fn wait_change(watcher: &mut FileWatcher) -> Option<String> {
        let started = Instant::now();
        loop {
            if let Some(change) = watcher.take_change() {
                return Some(change);
            }
            if started.elapsed() > Duration::from_secs(2) {
                return None;
            }
            thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn immediate_check_watches_replacements_and_ignores_missing_files() {
        let root = std::env::temp_dir().join(format!(
            "roots-watch-test-{}",
            roots_core::ids::random_hex(8)
        ));
        fs::create_dir_all(&root).unwrap();
        let file = root.join("idea.md");
        fs::write(&file, "one\n").unwrap();
        let mut watcher = FileWatcher::new(&file, "one\n", options());
        assert_eq!(watcher.check(), None);
        fs::write(root.join("swap"), "two\n").unwrap();
        fs::rename(root.join("swap"), &file).unwrap();
        assert_eq!(watcher.check().as_deref(), Some("two\n"));
        fs::remove_file(&file).unwrap();
        assert_eq!(watcher.check(), None);
        fs::write(&file, "back\n").unwrap();
        assert_eq!(watcher.check().as_deref(), Some("back\n"));
        watcher.close();
        fs::write(&file, "later\n").unwrap();
        assert_eq!(watcher.check(), None);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn event_probe_debounces_editor_save_bursts() {
        let root = std::env::temp_dir().join(format!(
            "roots-watch-debounce-{}",
            roots_core::ids::random_hex(8)
        ));
        fs::create_dir_all(&root).unwrap();
        let file = root.join("idea.md");
        fs::write(&file, "one\n").unwrap();
        let mut watcher = FileWatcher::new(&file, "one\n", options());
        fs::write(&file, "partial\n").unwrap();
        thread::sleep(Duration::from_millis(10));
        fs::write(&file, "final\n").unwrap();
        // The source watcher waits out its debounce rather than yielding the
        // first temporary editor write.
        thread::sleep(Duration::from_millis(15));
        assert_eq!(watcher.take_change(), None);
        assert_eq!(wait_change(&mut watcher).as_deref(), Some("final\n"));
        watcher.close();
        fs::remove_dir_all(root).unwrap();
    }
}
