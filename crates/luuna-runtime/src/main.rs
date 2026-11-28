use luuna_runtime::{serve, AppState, ServiceSettings, StartupError, Stopped};
use std::{
    net::{Ipv4Addr, SocketAddrV4},
    time::Duration,
};

mod admin;

/// How long open connections may drain once owned work has settled. The
/// admission drain and the settle wait before it are bounded separately, so
/// an orderly stop always ends.
const DRAIN_DEADLINE: Duration = Duration::from_secs(10);

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // `backup`, `restore`, `check` and `doctor` maintain or diagnose the data
    // directory and host, then exit.
    if let Some(outcome) = admin::dispatch(std::env::args_os().skip(1)) {
        return outcome;
    }
    let settings = ServiceSettings::from_env(|name| std::env::var_os(name))?;
    let state = AppState::open(settings.runtime)
        .map_err(|error| StartupError(error.to_string()))?
        .with_publication(settings.publication)
        .with_default_store(settings.default_store);
    let address = SocketAddrV4::new(Ipv4Addr::LOCALHOST, settings.port);
    let listener = tokio::net::TcpListener::bind(address)
        .await
        .map_err(|error| {
            StartupError(format!(
                "cannot listen on {address}: {error}; set LUUNA_PORT to a free port"
            ))
        })?;
    let address = listener.local_addr()?;
    eprintln!("Luuna runtime listening on http://{address}");
    let state = state.with_listen_port(address.port());
    watch_termination_signals(state.clone());
    match serve(listener, state, DRAIN_DEADLINE).await? {
        Stopped::Drained => {
            eprintln!("luuna-runtime: stopped");
            Ok(())
        }
        Stopped::DeadlineExceeded => {
            eprintln!(
                "luuna-runtime: connections still open {}s after owned work settled; exiting",
                DRAIN_DEADLINE.as_secs()
            );
            std::process::exit(1);
        }
    }
}

/// The first termination request (Ctrl+C; SIGTERM on Unix; Ctrl+Break,
/// console close or system shutdown on Windows) takes the orderly shutdown
/// path: owned workers stop and interruption is persisted. A second one exits
/// at once, as the default action would have.
fn watch_termination_signals(state: AppState) {
    tokio::spawn(async move {
        let mut signals = match TerminationSignals::install() {
            Ok(signals) => signals,
            Err(error) => {
                eprintln!(
                    "luuna-runtime: cannot listen for termination signals ({error}); stop the service with POST /api/v1/shutdown"
                );
                return;
            }
        };
        signals.recv().await;
        eprintln!(
            "luuna-runtime: termination requested; stopping owned work (repeat to exit at once)"
        );
        state.request_shutdown();
        drop(state);
        signals.recv().await;
        eprintln!("luuna-runtime: second termination request; exiting without waiting");
        std::process::exit(130);
    });
}

#[cfg(unix)]
struct TerminationSignals {
    interrupt: tokio::signal::unix::Signal,
    terminate: tokio::signal::unix::Signal,
}

#[cfg(unix)]
impl TerminationSignals {
    fn install() -> std::io::Result<Self> {
        use tokio::signal::unix::{signal, SignalKind};
        Ok(Self {
            interrupt: signal(SignalKind::interrupt())?,
            terminate: signal(SignalKind::terminate())?,
        })
    }

    async fn recv(&mut self) {
        tokio::select! {
            _ = self.interrupt.recv() => {},
            _ = self.terminate.recv() => {},
        }
    }
}

/// Console close and system shutdown give the process only a few seconds
/// before Windows ends it, so the orderly path is best effort there; a
/// supervisor should use `POST /api/v1/shutdown` (ADR 0010). Logoff is not
/// handled: a service would receive it for any user's session.
#[cfg(windows)]
struct TerminationSignals {
    ctrl_c: tokio::signal::windows::CtrlC,
    ctrl_break: tokio::signal::windows::CtrlBreak,
    ctrl_close: tokio::signal::windows::CtrlClose,
    ctrl_shutdown: tokio::signal::windows::CtrlShutdown,
}

#[cfg(windows)]
impl TerminationSignals {
    fn install() -> std::io::Result<Self> {
        use tokio::signal::windows::{ctrl_break, ctrl_c, ctrl_close, ctrl_shutdown};
        Ok(Self {
            ctrl_c: ctrl_c()?,
            ctrl_break: ctrl_break()?,
            ctrl_close: ctrl_close()?,
            ctrl_shutdown: ctrl_shutdown()?,
        })
    }

    async fn recv(&mut self) {
        tokio::select! {
            _ = self.ctrl_c.recv() => {},
            _ = self.ctrl_break.recv() => {},
            _ = self.ctrl_close.recv() => {},
            _ = self.ctrl_shutdown.recv() => {},
        }
    }
}

#[cfg(not(any(unix, windows)))]
struct TerminationSignals;

#[cfg(not(any(unix, windows)))]
impl TerminationSignals {
    fn install() -> std::io::Result<Self> {
        Ok(Self)
    }

    async fn recv(&mut self) {
        let _ = tokio::signal::ctrl_c().await;
    }
}
