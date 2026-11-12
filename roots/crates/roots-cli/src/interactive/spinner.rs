//! A narrow status-line abstraction for slow agent work.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

pub trait StatusLine: Send {
    fn stop(&mut self, final_line: Option<&str>);
}

const FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const CLEAR_LINE: &str = "\r\x1b[2K";

#[must_use]
pub fn fit_line(line: &str, columns: Option<usize>) -> String {
    let Some(columns) = columns.filter(|columns| *columns >= 2) else {
        return line.to_owned();
    };
    let characters = line.chars().collect::<Vec<_>>();
    if characters.len() < columns {
        line.to_owned()
    } else {
        format!("{}…", characters[..columns - 2].iter().collect::<String>())
    }
}

pub struct Spinner {
    write: Arc<dyn Fn(String) + Send + Sync>,
    animate: bool,
    stopped: Arc<AtomicBool>,
    /// Serialize ticker writes with `stop` so a stale frame cannot appear
    /// after the final outcome line.
    write_gate: Arc<std::sync::Mutex<()>>,
}

impl StatusLine for Spinner {
    fn stop(&mut self, final_line: Option<&str>) {
        let Ok(_gate) = self.write_gate.lock() else {
            return;
        };
        if self.stopped.swap(true, Ordering::SeqCst) {
            return;
        }
        if self.animate {
            (self.write)(CLEAR_LINE.to_owned());
        }
        if let Some(final_line) = final_line {
            (self.write)(format!("{final_line}\n"));
        }
    }
}

/// Start a status indicator. The writer is shared so the background ticker
/// never borrows a command's `Io` beyond this call.
#[must_use]
pub fn start_status(
    write: Arc<dyn Fn(String) + Send + Sync>,
    text: String,
    animate: bool,
    columns: Option<Arc<dyn Fn() -> usize + Send + Sync>>,
) -> Spinner {
    let stopped = Arc::new(AtomicBool::new(false));
    let write_gate = Arc::new(std::sync::Mutex::new(()));
    if !animate {
        write(format!("{text}\n"));
        return Spinner {
            write,
            animate,
            stopped,
            write_gate,
        };
    }
    let tick_write = Arc::clone(&write);
    let tick_stopped = Arc::clone(&stopped);
    let tick_gate = Arc::clone(&write_gate);
    thread::spawn(move || {
        let started = Instant::now();
        let mut frame = 0_usize;
        while !tick_stopped.load(Ordering::SeqCst) {
            let line = format!(
                "{} {} {}s",
                FRAMES[frame % FRAMES.len()],
                text,
                started.elapsed().as_secs()
            );
            if let Ok(_gate) = tick_gate.lock()
                && !tick_stopped.load(Ordering::SeqCst)
            {
                tick_write(format!(
                    "{CLEAR_LINE}{}",
                    fit_line(&line, columns.as_ref().map(|get| get()))
                ));
            }
            frame += 1;
            thread::sleep(Duration::from_millis(100));
        }
    });
    Spinner {
        write,
        animate,
        stopped,
        write_gate,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[test]
    fn width_fitting_and_plain_mode_match_the_status_contract() {
        assert!(fit_line("⠋ asking about a long idea 0s", Some(20)).ends_with('…'));
        let lines = Arc::new(Mutex::new(Vec::new()));
        let target = Arc::clone(&lines);
        let write: Arc<dyn Fn(String) + Send + Sync> = Arc::new(move |line| {
            target.lock().unwrap().push(line);
        });
        let mut status = start_status(write, "hi".into(), false, None);
        status.stop(None);
        assert_eq!(&*lines.lock().unwrap(), &["hi\n"]);
    }

    #[test]
    fn animated_stop_clears_once_and_keeps_the_outcome_last() {
        let lines = Arc::new(Mutex::new(Vec::new()));
        let target = Arc::clone(&lines);
        let write: Arc<dyn Fn(String) + Send + Sync> = Arc::new(move |line| {
            target.lock().unwrap().push(line);
        });
        let mut status = start_status(write, "asking".into(), true, Some(Arc::new(|| 80)));
        status.stop(Some("! agent.command failed; continuing without it"));
        // Give a concurrently scheduled first tick a chance to prove it is
        // held behind the stop gate rather than appended after the outcome.
        thread::sleep(Duration::from_millis(120));
        let lines = lines.lock().unwrap();
        assert_eq!(
            lines.last().map(String::as_str),
            Some("! agent.command failed; continuing without it\n")
        );
        assert_eq!(
            lines
                .iter()
                .filter(|line| line.contains("agent.command failed"))
                .count(),
            1
        );
        assert!(lines.iter().any(|line| line == CLEAR_LINE));
    }
}
