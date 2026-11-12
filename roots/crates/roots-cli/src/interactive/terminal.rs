//! Raw-key terminal abstraction used by `think` (and later `tend` / `flow`).
//!
//! The trait deliberately has no dependency on a concrete terminal crate. A
//! test can inject [`FakeTerminal`], while [`ProcessTerminal`] owns the small
//! amount of process-global console state needed by the CLI.

use roots_core::color::strip_ansi;
use std::io::{self, IsTerminal, Read, Write};
#[cfg(any(test, unix))]
use std::sync::atomic::AtomicBool;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

pub const ALT_ON: &str = "\x1b[?1049h";
pub const ALT_OFF: &str = "\x1b[?1049l";
pub const HIDE_CURSOR: &str = "\x1b[?25l";
pub const SHOW_CURSOR: &str = "\x1b[?25h";
pub const HOME: &str = "\x1b[H";
pub const CLEAR: &str = "\x1b[2J";

/// Draw one full-screen frame. CRLF is intentional: raw terminal mode does
/// not perform the usual output newline translation.
#[must_use]
pub fn frame(lines: &[String]) -> String {
    format!("{HOME}{CLEAR}{}", lines.join("\r\n"))
}

pub type InputCallback = Box<dyn FnMut(String) + Send + 'static>;
pub type ResizeCallback = Box<dyn FnMut() + Send + 'static>;

/// The tiny terminal surface needed by interactive Roots commands.
pub trait Terminal: Send {
    fn columns(&self) -> usize;
    /// Cloneable width reader for detached status lines. Unlike borrowing the
    /// terminal itself, this remains valid while an interactive session holds
    /// its exclusive mutable terminal borrow.
    fn columns_reader(&self) -> Arc<dyn Fn() -> usize + Send + Sync> {
        let columns = self.columns();
        Arc::new(move || columns)
    }
    /// Zero means the height is not known; renderers use natural height.
    fn rows(&self) -> usize;
    fn write(&mut self, text: &str);
    /// Begin raw key delivery and enter the alternate screen.
    fn start(&mut self, on_input: InputCallback, on_resize: Option<ResizeCallback>);
    /// Restore cursor/screen/input state. Safe to call more than once.
    fn stop(&mut self);
}

type SharedCallback = Arc<Mutex<Option<InputCallback>>>;
type SharedResizeCallback = Arc<Mutex<Option<ResizeCallback>>>;
type Generation = u64;
type GenerationState = Arc<AtomicU64>;

/// A terminal can leave an OS input read blocked after `stop()`: closing the
/// alternate screen cannot cancel that read portably.  Every `start()`
/// therefore gets a monotonically increasing generation.  A reader or resize
/// monitor from an earlier generation must never deliver into a callback that
/// belongs to a later handoff.
fn begin_generation(next: &mut Generation, current: &GenerationState) -> Generation {
    *next = next.wrapping_add(1);
    // Zero means that no generation is active.  Skipping it also keeps the
    // comparison valid if a very long-lived process wraps the counter.
    if *next == 0 {
        *next = 1;
    }
    current.store(*next, Ordering::SeqCst);
    *next
}

fn generation_is_current(current: &GenerationState, generation: Generation) -> bool {
    current.load(Ordering::SeqCst) == generation
}

fn cancel_generation(current: &GenerationState, generation: Generation) {
    let _ = current.compare_exchange(generation, 0, Ordering::SeqCst, Ordering::SeqCst);
}

/// Deliver only while this exact raw-terminal generation is still current.
/// The second check happens while the callback slot is locked: `stop()` clears
/// that slot before a replacement `start()` can install its callback, so a
/// stale reader cannot race through to the replacement session.
fn deliver_input(
    current: &GenerationState,
    generation: Generation,
    callback: &SharedCallback,
    data: String,
) {
    if !generation_is_current(current, generation) {
        return;
    }
    if let Ok(mut slot) = callback.lock() {
        if !generation_is_current(current, generation) {
            return;
        }
        if let Some(callback) = slot.as_mut() {
            callback(data);
        }
    }
}

/// The resize callback shares the same handoff boundary as input.  In
/// particular, an old monitor must not redraw a newly restarted interactive
/// screen after an editor or nested-session handoff.
fn deliver_resize(
    current: &GenerationState,
    generation: Generation,
    callback: &SharedResizeCallback,
) {
    if !generation_is_current(current, generation) {
        return;
    }
    if let Ok(mut slot) = callback.lock() {
        if !generation_is_current(current, generation) {
            return;
        }
        if let Some(callback) = slot.as_mut() {
            callback();
        }
    }
}

/// Match the source terminal's routing rule: use stdout unless stdout is
/// redirected while stderr is an interactive terminal. That preserves useful
/// machine-readable stdout while keeping a usable full-screen session.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum OutputTarget {
    Stdout,
    Stderr,
}

const fn output_target(stdout_is_tty: bool, stderr_is_tty: bool) -> OutputTarget {
    if stdout_is_tty || !stderr_is_tty {
        OutputTarget::Stdout
    } else {
        OutputTarget::Stderr
    }
}

fn process_output_target() -> OutputTarget {
    output_target(io::stdout().is_terminal(), io::stderr().is_terminal())
}

// `signal` handlers have to stay deliberately boring: setting an atomic is
// async-signal-safe, while locking a callback or writing a terminal is not.
// The terminal's ordinary helper thread observes these flags and delivers the
// corresponding callback outside signal context.  That is the Rust analogue
// of Node's SIGTERM/SIGHUP/SIGWINCH event listeners.
#[cfg(unix)]
static QUIT_SIGNAL: AtomicBool = AtomicBool::new(false);
#[cfg(unix)]
static RESIZE_SIGNAL: AtomicBool = AtomicBool::new(false);

#[cfg(unix)]
const SIGHUP: i32 = 1;
#[cfg(unix)]
const SIGTERM: i32 = 15;
#[cfg(unix)]
const SIGWINCH: i32 = 28;

#[cfg(unix)]
extern "C" fn quit_signal(_: i32) {
    QUIT_SIGNAL.store(true, Ordering::SeqCst);
}

#[cfg(unix)]
extern "C" fn resize_signal(_: i32) {
    RESIZE_SIGNAL.store(true, Ordering::SeqCst);
}

#[cfg(unix)]
unsafe extern "C" {
    fn signal(signum: i32, handler: usize) -> usize;
}

#[cfg(unix)]
#[derive(Clone, Copy, Debug)]
struct SignalHandlers {
    hup: Option<usize>,
    term: Option<usize>,
    winch: Option<usize>,
}

/// Native process terminal. On Unix it uses `stty` to enter raw mode; on
/// Windows it uses the console input mode APIs. Input is read on a helper
/// thread so session code can continue polling its watched prose file.
pub struct ProcessTerminal {
    active: bool,
    output: OutputTarget,
    callback: SharedCallback,
    resize_callback: SharedResizeCallback,
    /// Current raw-terminal generation.  A reader from a stopped generation
    /// can remain blocked in stdin, but it cannot become current again when a
    /// later handoff restarts the terminal.
    current_generation: GenerationState,
    next_generation: Generation,
    active_generation: Option<Generation>,
    dimensions: Arc<Mutex<(usize, usize)>>,
    #[cfg(unix)]
    saved_stty: Option<String>,
    #[cfg(unix)]
    signal_handlers: Option<SignalHandlers>,
    #[cfg(windows)]
    saved_input_mode: Option<u32>,
    #[cfg(windows)]
    saved_output_mode: Option<u32>,
}

impl std::fmt::Debug for ProcessTerminal {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ProcessTerminal")
            .field("active", &self.active)
            .finish_non_exhaustive()
    }
}

impl Default for ProcessTerminal {
    fn default() -> Self {
        Self::new()
    }
}

impl ProcessTerminal {
    #[must_use]
    pub fn new() -> Self {
        let output = process_output_target();
        Self {
            active: false,
            output,
            callback: Arc::new(Mutex::new(None)),
            resize_callback: Arc::new(Mutex::new(None)),
            current_generation: Arc::new(AtomicU64::new(0)),
            next_generation: 0,
            active_generation: None,
            dimensions: Arc::new(Mutex::new(process_dimensions(output))),
            #[cfg(unix)]
            saved_stty: None,
            #[cfg(unix)]
            signal_handlers: None,
            #[cfg(windows)]
            saved_input_mode: None,
            #[cfg(windows)]
            saved_output_mode: None,
        }
    }

    #[cfg(unix)]
    fn enable_raw(&mut self) {
        use std::process::Command;
        let saved = Command::new("stty")
            .arg("-g")
            .stdin(std::process::Stdio::inherit())
            .output()
            .ok()
            .filter(|output| output.status.success())
            .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned());
        if let Some(state) = &saved {
            let _ = Command::new("stty")
                .args(["raw", "-echo"])
                .stdin(std::process::Stdio::inherit())
                .status();
            self.saved_stty = Some(state.clone());
        }
    }

    #[cfg(unix)]
    fn disable_raw(&mut self) {
        use std::process::Command;
        if let Some(state) = self.saved_stty.take() {
            let _ = Command::new("stty")
                .arg(state)
                .stdin(std::process::Stdio::inherit())
                .status();
        }
    }

    #[cfg(unix)]
    fn install_signal_handlers(&mut self) {
        // A process terminal is unique in normal CLI use.  Reset a stale flag
        // before installing handlers so a signal from an earlier session does
        // not immediately end the next one.
        QUIT_SIGNAL.store(false, Ordering::SeqCst);
        RESIZE_SIGNAL.store(false, Ordering::SeqCst);
        // The old action is restored by `stop`; the signal monitor below is
        // the only code that calls session callbacks.
        let (hup, term, winch) = unsafe {
            (
                signal(SIGHUP, quit_signal as *const () as usize),
                signal(SIGTERM, quit_signal as *const () as usize),
                signal(SIGWINCH, resize_signal as *const () as usize),
            )
        };
        let handlers = SignalHandlers {
            hup: (hup != usize::MAX).then_some(hup),
            term: (term != usize::MAX).then_some(term),
            winch: (winch != usize::MAX).then_some(winch),
        };
        self.signal_handlers = Some(handlers);
    }

    #[cfg(unix)]
    fn restore_signal_handlers(&mut self) {
        let Some(handlers) = self.signal_handlers.take() else {
            return;
        };
        unsafe {
            if let Some(handler) = handlers.hup {
                signal(SIGHUP, handler);
            }
            if let Some(handler) = handlers.term {
                signal(SIGTERM, handler);
            }
            if let Some(handler) = handlers.winch {
                signal(SIGWINCH, handler);
            }
        }
    }

    #[cfg(windows)]
    fn enable_raw(&mut self) {
        let handle = unsafe { get_std_handle(STD_INPUT_HANDLE) };
        let mut mode = 0_u32;
        if handle.is_null() || unsafe { get_console_mode(handle, &mut mode) } == 0 {
            return;
        }
        self.saved_input_mode = Some(mode);
        // Disable processed, line, and echo input. Ctrl-C then arrives as the
        // byte parsed by the session reducer instead of terminating Roots.
        let raw = mode & !(ENABLE_PROCESSED_INPUT | ENABLE_LINE_INPUT | ENABLE_ECHO_INPUT);
        let _ = unsafe { set_console_mode(handle, raw) };
        let output = unsafe { get_std_handle(output_handle_id(self.output)) };
        if !output.is_null() {
            let mut out_mode = 0_u32;
            if unsafe { get_console_mode(output, &mut out_mode) } != 0 {
                self.saved_output_mode = Some(out_mode);
                let _ = unsafe {
                    set_console_mode(output, out_mode | ENABLE_VIRTUAL_TERMINAL_PROCESSING)
                };
            }
        }
    }

    #[cfg(windows)]
    fn disable_raw(&mut self) {
        if let Some(mode) = self.saved_input_mode.take() {
            let handle = unsafe { get_std_handle(STD_INPUT_HANDLE) };
            if !handle.is_null() {
                let _ = unsafe { set_console_mode(handle, mode) };
            }
        }
        if let Some(mode) = self.saved_output_mode.take() {
            let output = unsafe { get_std_handle(output_handle_id(self.output)) };
            if !output.is_null() {
                let _ = unsafe { set_console_mode(output, mode) };
            }
        }
    }

    #[cfg(not(any(unix, windows)))]
    fn enable_raw(&mut self) {}

    #[cfg(not(any(unix, windows)))]
    fn disable_raw(&mut self) {}
}

impl Drop for ProcessTerminal {
    fn drop(&mut self) {
        self.stop();
    }
}

impl Terminal for ProcessTerminal {
    fn columns(&self) -> usize {
        self.dimensions.lock().map(|size| size.0).unwrap_or(80)
    }

    fn columns_reader(&self) -> Arc<dyn Fn() -> usize + Send + Sync> {
        let dimensions = Arc::clone(&self.dimensions);
        Arc::new(move || dimensions.lock().map(|size| size.0).unwrap_or(80))
    }

    fn rows(&self) -> usize {
        self.dimensions.lock().map(|size| size.1).unwrap_or(0)
    }

    fn write(&mut self, text: &str) {
        match self.output {
            OutputTarget::Stdout => {
                let mut output = io::stdout().lock();
                let _ = output.write_all(text.as_bytes());
                let _ = output.flush();
            }
            OutputTarget::Stderr => {
                let mut output = io::stderr().lock();
                let _ = output.write_all(text.as_bytes());
                let _ = output.flush();
            }
        }
    }

    fn start(&mut self, on_input: InputCallback, on_resize: Option<ResizeCallback>) {
        if self.active {
            return;
        }
        self.active = true;
        let generation = begin_generation(&mut self.next_generation, &self.current_generation);
        self.active_generation = Some(generation);
        if io::stdin().is_terminal() {
            self.enable_raw();
        }
        if let Ok(mut slot) = self.callback.lock() {
            *slot = Some(on_input);
        }
        if let Ok(mut slot) = self.resize_callback.lock() {
            *slot = on_resize;
        }
        if io::stdin().is_terminal() {
            #[cfg(unix)]
            self.install_signal_handlers();
        }
        self.write(&(ALT_ON.to_owned() + HIDE_CURSOR));

        let callback = Arc::clone(&self.callback);
        let callback_for_monitor = Arc::clone(&self.callback);
        let current_generation = Arc::clone(&self.current_generation);
        thread::spawn(move || {
            let mut input = io::stdin().lock();
            let mut bytes = [0_u8; 64];
            while generation_is_current(&current_generation, generation) {
                let Ok(count) = input.read(&mut bytes) else {
                    break;
                };
                // A stopped reader may return after a replacement session has
                // started.  Its captured generation is now stale, so it must
                // not deliver this data to the replacement callback.
                if !generation_is_current(&current_generation, generation) {
                    break;
                }
                if count == 0 {
                    break;
                }
                let data = String::from_utf8_lossy(&bytes[..count]).into_owned();
                deliver_input(&current_generation, generation, &callback, data);
            }
        });

        // SIGWINCH is delivered via the atomic flag on Unix.  The same
        // monitor also polls native dimensions so Windows consoles and
        // terminal emulators that do not generate a resize event still redraw
        // promptly.  Polling is intentionally modest and stops with the
        // session; it is also useful for a resize that occurred just before
        // raw mode was entered.
        let resize = Arc::clone(&self.resize_callback);
        let dimensions = Arc::clone(&self.dimensions);
        let monitor_generation = Arc::clone(&self.current_generation);
        let output = self.output;
        thread::spawn(move || {
            let mut previous = dimensions.lock().map(|value| *value).unwrap_or((80, 0));
            while generation_is_current(&monitor_generation, generation) {
                thread::sleep(Duration::from_millis(80));
                // Do not let a monitor which slept through a stop/restart
                // consume a signal or redraw the next session.
                if !generation_is_current(&monitor_generation, generation) {
                    break;
                }
                let signalled = {
                    #[cfg(unix)]
                    {
                        RESIZE_SIGNAL.swap(false, Ordering::SeqCst)
                    }
                    #[cfg(not(unix))]
                    {
                        false
                    }
                };
                let quit = {
                    #[cfg(unix)]
                    {
                        QUIT_SIGNAL.swap(false, Ordering::SeqCst)
                    }
                    #[cfg(not(unix))]
                    {
                        false
                    }
                };
                // Unix is event-driven through SIGWINCH; querying `stty`
                // only after that signal avoids repeatedly spawning a helper
                // process.  Windows uses the native console buffer API, so a
                // small polling fallback is inexpensive and covers hosts
                // without a resize event subscription.
                #[cfg(unix)]
                let current = if signalled {
                    process_dimensions(output)
                } else {
                    previous
                };
                #[cfg(not(unix))]
                let current = process_dimensions(output);
                let changed = current != previous;
                if changed {
                    previous = current;
                    if let Ok(mut stored) = dimensions.lock() {
                        *stored = current;
                    }
                }
                if quit {
                    deliver_input(
                        &monitor_generation,
                        generation,
                        &callback_for_monitor,
                        "\x03".to_owned(),
                    );
                }
                if signalled || changed {
                    deliver_resize(&monitor_generation, generation, &resize);
                }
            }
        });
    }

    fn stop(&mut self) {
        if !self.active {
            return;
        }
        self.active = false;
        if let Some(generation) = self.active_generation.take() {
            cancel_generation(&self.current_generation, generation);
        }
        if let Ok(mut slot) = self.callback.lock() {
            *slot = None;
        }
        if let Ok(mut slot) = self.resize_callback.lock() {
            *slot = None;
        }
        #[cfg(unix)]
        self.restore_signal_handlers();
        self.disable_raw();
        self.write(&(SHOW_CURSOR.to_owned() + ALT_OFF));
    }
}

fn env_dimensions() -> (usize, usize) {
    let columns = std::env::var("COLUMNS")
        .ok()
        .and_then(|value| value.parse().ok())
        .filter(|value: &usize| *value > 0)
        .unwrap_or(80);
    let rows = std::env::var("LINES")
        .ok()
        .and_then(|value| value.parse().ok())
        .filter(|value: &usize| *value > 0)
        .unwrap_or(0);
    (columns, rows)
}

#[cfg(unix)]
fn process_dimensions(_output: OutputTarget) -> (usize, usize) {
    use std::process::{Command, Stdio};
    let parsed = Command::new("stty")
        .arg("size")
        .stdin(Stdio::inherit())
        .output()
        .ok()
        .filter(|result| result.status.success())
        .and_then(|result| {
            let text = String::from_utf8_lossy(&result.stdout);
            let mut values = text
                .split_whitespace()
                .filter_map(|part| part.parse::<usize>().ok());
            Some((values.next()?, values.next()?))
        });
    parsed.map_or_else(env_dimensions, |(rows, columns)| (columns, rows))
}

#[cfg(windows)]
fn output_handle_id(output: OutputTarget) -> i32 {
    match output {
        OutputTarget::Stdout => STD_OUTPUT_HANDLE,
        OutputTarget::Stderr => STD_ERROR_HANDLE,
    }
}

#[cfg(windows)]
fn process_dimensions(output: OutputTarget) -> (usize, usize) {
    let handle = unsafe { get_std_handle(output_handle_id(output)) };
    let mut info = ConsoleScreenBufferInfo::default();
    if !handle.is_null() && unsafe { get_console_screen_buffer_info(handle, &mut info) } != 0 {
        let columns =
            (i32::from(info.window.right) - i32::from(info.window.left) + 1).max(1) as usize;
        let rows = (i32::from(info.window.bottom) - i32::from(info.window.top) + 1).max(1) as usize;
        return (columns, rows);
    }
    env_dimensions()
}

#[cfg(not(any(unix, windows)))]
fn process_dimensions(_output: OutputTarget) -> (usize, usize) {
    env_dimensions()
}

#[cfg(windows)]
const STD_INPUT_HANDLE: i32 = -10;
#[cfg(windows)]
const STD_OUTPUT_HANDLE: i32 = -11;
#[cfg(windows)]
const STD_ERROR_HANDLE: i32 = -12;
#[cfg(windows)]
const ENABLE_PROCESSED_INPUT: u32 = 0x0001;
#[cfg(windows)]
const ENABLE_LINE_INPUT: u32 = 0x0002;
#[cfg(windows)]
const ENABLE_ECHO_INPUT: u32 = 0x0004;
#[cfg(windows)]
const ENABLE_VIRTUAL_TERMINAL_PROCESSING: u32 = 0x0004;

#[cfg(windows)]
#[link(name = "Kernel32")]
unsafe extern "system" {
    #[link_name = "GetStdHandle"]
    fn get_std_handle(which: i32) -> *mut std::ffi::c_void;
    #[link_name = "GetConsoleMode"]
    fn get_console_mode(handle: *mut std::ffi::c_void, mode: *mut u32) -> i32;
    #[link_name = "SetConsoleMode"]
    fn set_console_mode(handle: *mut std::ffi::c_void, mode: u32) -> i32;
    #[link_name = "GetConsoleScreenBufferInfo"]
    fn get_console_screen_buffer_info(
        handle: *mut std::ffi::c_void,
        info: *mut ConsoleScreenBufferInfo,
    ) -> i32;
}

#[cfg(windows)]
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Coord {
    x: i16,
    y: i16,
}

#[cfg(windows)]
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct SmallRect {
    left: i16,
    top: i16,
    right: i16,
    bottom: i16,
}

#[cfg(windows)]
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct ConsoleScreenBufferInfo {
    size: Coord,
    cursor_position: Coord,
    attributes: u16,
    window: SmallRect,
    maximum_window_size: Coord,
}

struct FakeState {
    started: bool,
    stopped: bool,
    frames: Vec<String>,
    input: SharedCallback,
    resize: SharedResizeCallback,
    current_generation: GenerationState,
    next_generation: Generation,
    active_generation: Option<Generation>,
}

impl Default for FakeState {
    fn default() -> Self {
        Self {
            started: false,
            stopped: false,
            frames: Vec::new(),
            input: Arc::new(Mutex::new(None)),
            resize: Arc::new(Mutex::new(None)),
            current_generation: Arc::new(AtomicU64::new(0)),
            next_generation: 0,
            active_generation: None,
        }
    }
}

/// Cloneable control surface for an in-memory terminal used in deterministic
/// tests. It is intentionally separate from the terminal value installed in
/// an `Io`, so a test can send keys while the command blocks on another thread.
#[derive(Clone)]
pub struct FakeTerminalHandle {
    state: Arc<Mutex<FakeState>>,
}

impl FakeTerminalHandle {
    pub fn send(&self, data: &str) {
        let parts = self.state.lock().ok().and_then(|state| {
            state.active_generation.map(|generation| {
                (
                    Arc::clone(&state.current_generation),
                    Arc::clone(&state.input),
                    generation,
                )
            })
        });
        if let Some((current, input, generation)) = parts {
            deliver_input(&current, generation, &input, data.to_owned());
        }
    }

    pub fn resize(&self) {
        let parts = self.state.lock().ok().and_then(|state| {
            state.active_generation.map(|generation| {
                (
                    Arc::clone(&state.current_generation),
                    Arc::clone(&state.resize),
                    generation,
                )
            })
        });
        if let Some((current, resize, generation)) = parts {
            deliver_resize(&current, generation, &resize);
        }
    }

    /// Simulate an input reader that began in an earlier raw-terminal
    /// generation and did not return from a blocking read until after a
    /// handoff.  Tests use this to guard the ProcessTerminal race directly.
    #[cfg(test)]
    fn send_from_generation(&self, generation: Generation, data: &str) {
        let parts = self.state.lock().ok().map(|state| {
            (
                Arc::clone(&state.current_generation),
                Arc::clone(&state.input),
            )
        });
        if let Some((current, input)) = parts {
            deliver_input(&current, generation, &input, data.to_owned());
        }
    }

    /// As above, but for an old resize monitor instead of an input reader.
    #[cfg(test)]
    fn resize_from_generation(&self, generation: Generation) {
        let parts = self.state.lock().ok().map(|state| {
            (
                Arc::clone(&state.current_generation),
                Arc::clone(&state.resize),
            )
        });
        if let Some((current, resize)) = parts {
            deliver_resize(&current, generation, &resize);
        }
    }

    #[cfg(test)]
    fn active_generation(&self) -> Option<Generation> {
        self.state.lock().ok().and_then(|state| {
            state
                .active_generation
                .filter(|generation| generation_is_current(&state.current_generation, *generation))
        })
    }

    #[must_use]
    pub fn started(&self) -> bool {
        self.state.lock().is_ok_and(|state| state.started)
    }

    #[must_use]
    pub fn stopped(&self) -> bool {
        self.state.lock().is_ok_and(|state| state.stopped)
    }

    #[must_use]
    pub fn frames(&self) -> Vec<String> {
        self.state
            .lock()
            .map(|state| state.frames.clone())
            .unwrap_or_default()
    }

    #[must_use]
    pub fn screen(&self) -> String {
        let frame = self
            .state
            .lock()
            .ok()
            .and_then(|state| state.frames.last().cloned())
            .unwrap_or_default();
        strip_ansi(
            frame
                .strip_prefix(HOME)
                .unwrap_or(&frame)
                .strip_prefix(CLEAR)
                .unwrap_or(&frame),
        )
        .replace("\r\n", "\n")
    }
}

/// An in-memory terminal plus a cloneable external controller.
pub struct FakeTerminal {
    columns: usize,
    rows: usize,
    state: Arc<Mutex<FakeState>>,
}

impl FakeTerminal {
    #[must_use]
    pub fn new(columns: usize, rows: usize) -> (Self, FakeTerminalHandle) {
        let state = Arc::new(Mutex::new(FakeState::default()));
        (
            Self {
                columns,
                rows,
                state: Arc::clone(&state),
            },
            FakeTerminalHandle { state },
        )
    }
}

impl Terminal for FakeTerminal {
    fn columns(&self) -> usize {
        self.columns
    }

    fn rows(&self) -> usize {
        self.rows
    }

    fn write(&mut self, text: &str) {
        if text.starts_with(HOME) {
            if let Ok(mut state) = self.state.lock() {
                state.frames.push(text.to_owned());
            }
        }
    }

    fn start(&mut self, on_input: InputCallback, on_resize: Option<ResizeCallback>) {
        if let Ok(mut state) = self.state.lock() {
            if state.active_generation.is_some_and(|generation| {
                generation_is_current(&state.current_generation, generation)
            }) {
                return;
            }
            let current_generation = Arc::clone(&state.current_generation);
            let generation = begin_generation(&mut state.next_generation, &current_generation);
            state.started = true;
            state.stopped = false;
            state.active_generation = Some(generation);
            if let Ok(mut input) = state.input.lock() {
                *input = Some(on_input);
            }
            if let Ok(mut resize) = state.resize.lock() {
                *resize = on_resize;
            }
        }
    }

    fn stop(&mut self) {
        if let Ok(mut state) = self.state.lock() {
            state.stopped = true;
            if let Some(generation) = state.active_generation.take() {
                cancel_generation(&state.current_generation, generation);
            }
            if let Ok(mut input) = state.input.lock() {
                *input = None;
            }
            if let Ok(mut resize) = state.resize.lock() {
                *resize = None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_uses_the_alt_screen_redraw_prefix() {
        assert_eq!(
            frame(&["one".into(), "two".into()]),
            "\x1b[H\x1b[2Jone\r\ntwo"
        );
    }

    #[test]
    fn process_terminal_routes_to_stderr_only_when_stdout_is_redirected() {
        assert_eq!(output_target(true, true), OutputTarget::Stdout);
        assert_eq!(output_target(true, false), OutputTarget::Stdout);
        assert_eq!(output_target(false, false), OutputTarget::Stdout);
        assert_eq!(output_target(false, true), OutputTarget::Stderr);
    }

    #[test]
    fn terminal_width_reader_survives_the_terminal_borrow() {
        let (terminal, _) = FakeTerminal::new(37, 12);
        let columns = terminal.columns_reader();
        assert_eq!(columns(), 37);
    }

    #[test]
    fn fake_terminal_delivers_input_and_restores_once_stopped() {
        let (mut terminal, control) = FakeTerminal::new(50, 20);
        let seen = Arc::new(Mutex::new(Vec::new()));
        let target = Arc::clone(&seen);
        terminal.start(Box::new(move |key| target.lock().unwrap().push(key)), None);
        control.send("q");
        terminal.write(&frame(&["screen".into()]));
        terminal.stop();
        terminal.stop();
        assert_eq!(&*seen.lock().unwrap(), &["q"]);
        assert!(control.stopped());
        assert_eq!(control.screen(), "screen");
    }

    #[test]
    fn fake_terminal_delivers_resize_and_cleanup_clears_callbacks() {
        let (mut terminal, control) = FakeTerminal::new(50, 20);
        let resizes = Arc::new(AtomicBool::new(false));
        let target = Arc::clone(&resizes);
        terminal.start(
            Box::new(|_| {}),
            Some(Box::new(move || target.store(true, Ordering::SeqCst))),
        );
        control.resize();
        assert!(resizes.load(Ordering::SeqCst));
        terminal.stop();
        resizes.store(false, Ordering::SeqCst);
        control.resize();
        assert!(!resizes.load(Ordering::SeqCst));
        assert!(control.stopped());
    }

    #[test]
    fn fake_terminal_handoff_rejects_stale_generation_callbacks() {
        let (mut terminal, control) = FakeTerminal::new(50, 20);
        let input = Arc::new(Mutex::new(Vec::new()));
        let resizes = Arc::new(AtomicU64::new(0));

        terminal.start(Box::new(|_| {}), Some(Box::new(|| {})));
        let first = control.active_generation().expect("first generation");
        terminal.stop();

        let replacement_input = Arc::clone(&input);
        let replacement_resizes = Arc::clone(&resizes);
        terminal.start(
            Box::new(move |data| replacement_input.lock().unwrap().push(data)),
            Some(Box::new(move || {
                replacement_resizes.fetch_add(1, Ordering::SeqCst);
            })),
        );
        let replacement = control.active_generation().expect("replacement generation");
        assert_ne!(first, replacement);

        // This is the previously blocked reader / resize monitor waking after
        // a tend/think handoff.  It sees the replacement callback in the
        // shared slot, but its old generation must fence it out.
        control.send_from_generation(first, "stale");
        control.resize_from_generation(first);
        assert_eq!(&*input.lock().unwrap(), &[] as &[String]);
        assert_eq!(resizes.load(Ordering::SeqCst), 0);
        assert_eq!(control.active_generation(), Some(replacement));

        // The live generation still accepts its own events after the stale
        // callbacks were rejected.
        control.send("current");
        control.resize();
        assert_eq!(&*input.lock().unwrap(), &["current"]);
        assert_eq!(resizes.load(Ordering::SeqCst), 1);
        assert_eq!(control.active_generation(), Some(replacement));
    }

    #[cfg(unix)]
    #[test]
    fn unix_signal_handlers_only_request_safe_terminal_actions() {
        QUIT_SIGNAL.store(false, Ordering::SeqCst);
        RESIZE_SIGNAL.store(false, Ordering::SeqCst);
        quit_signal(SIGTERM);
        resize_signal(SIGWINCH);
        assert!(QUIT_SIGNAL.swap(false, Ordering::SeqCst));
        assert!(RESIZE_SIGNAL.swap(false, Ordering::SeqCst));
    }
}
