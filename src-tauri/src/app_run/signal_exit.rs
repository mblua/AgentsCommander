//! Signal exit (#5275): the desktop app's answer to SIGTERM, SIGHUP and SIGINT.
//! Includes the listener that logs every delivery and the watchdog that bounds
//! the shutdown. Does not include the exit claim (`QuitGate`) or the exit itself.

use std::sync::mpsc;
use std::time::Duration;

use tokio::signal::unix::{signal, Signal, SignalKind};

/// Bound from the first signal to process exit: the 40.5 s bounded shutdown
/// budget plus margin for its two unbounded waits. Under systemd's 90 s.
pub(crate) const SIGNAL_EXIT_DEADLINE: Duration = Duration::from_secs(60);

/// Cap on the watchdog's report step, so a held logger cannot block the bound.
const REPORT_CAP: Duration = Duration::from_secs(2);

/// One received terminating signal.
pub(crate) struct SignalExit {
    pub(crate) name: &'static str,
    pub(crate) signo: i32,
}

const SIGTERM: SignalExit = SignalExit {
    name: "SIGTERM",
    signo: libc::SIGTERM,
};
const SIGHUP: SignalExit = SignalExit {
    name: "SIGHUP",
    signo: libc::SIGHUP,
};
const SIGINT: SignalExit = SignalExit {
    name: "SIGINT",
    signo: libc::SIGINT,
};

/// The three registered signal streams.
pub(crate) struct Streams {
    term: Signal,
    hup: Signal,
    int: Signal,
}

impl Streams {
    /// Next delivery. Pends forever if the runtime's signal driver is gone.
    async fn next(&mut self) -> &'static SignalExit {
        tokio::select! {
            Some(()) = self.term.recv() => &SIGTERM,
            Some(()) = self.hup.recv() => &SIGHUP,
            Some(()) = self.int.recv() => &SIGINT,
            else => std::future::pending().await,
        }
    }
}

/// Sync; needs a tokio runtime context. Returns only after all three are
/// registered: this is the readiness point. Err if any registration fails.
pub(crate) fn install() -> std::io::Result<Streams> {
    Ok(Streams {
        term: signal(SignalKind::terminate())?,
        hup: signal(SignalKind::hangup())?,
        int: signal(SignalKind::interrupt())?,
    })
}

/// Never returns. First hit: warn log, then `first(&s)`. Later hits: warn log,
/// then `repeat(&s)`.
pub(crate) async fn run<F, R>(mut streams: Streams, first: F, mut repeat: R)
where
    F: FnOnce(&SignalExit) + Send,
    R: FnMut(&SignalExit) + Send,
{
    let mut first = Some(first);
    loop {
        let hit = streams.next().await;
        if let Some(first) = first.take() {
            log::warn!(
                "[signal] received {} (signo={}); starting orderly shutdown",
                hit.name,
                hit.signo
            );
            first(hit);
        } else {
            log::warn!(
                "[signal] received {} again; shutdown already in progress, ignored",
                hit.name
            );
            repeat(hit);
        }
    }
}

/// Starts the named thread `signal-exit` with its own current-thread runtime, then
/// `install()` and `run(streams, first, |_| {})`. The receiver gets `true` once all
/// handlers are registered (readiness) or `false` on failure; callers may drop it.
pub(crate) fn spawn_listener<F>(first: F) -> mpsc::Receiver<bool>
where
    F: FnOnce(&SignalExit) + Send + 'static,
{
    let (ready, ready_rx) = mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name("signal-exit".into())
        .spawn(move || {
            let rt = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(e) => {
                    log::warn!("[signal] handler install failed: {e}");
                    let _ = ready.send(false);
                    return;
                }
            };
            let _enter = rt.enter();
            let streams = match install() {
                Ok(streams) => streams,
                Err(e) => {
                    log::warn!("[signal] handler install failed: {e}");
                    let _ = ready.send(false);
                    return;
                }
            };
            let _ = ready.send(true);
            rt.block_on(run(streams, first, |_| {}));
        });
    if let Err(e) = spawned {
        log::warn!("[signal] handler install failed: {e}");
    }
    ready_rx
}

/// Starts a detached thread that, after `after`, runs `report` for at most
/// `REPORT_CAP` and then ends the process with `128 + signo`.
pub(crate) fn spawn_watchdog<P>(signo: i32, after: Duration, report: P)
where
    P: FnOnce() + Send + 'static,
{
    let spawned = std::thread::Builder::new()
        .name("signal-exit-watchdog".into())
        .spawn(move || {
            std::thread::sleep(after);
            let (done, finished) = mpsc::channel::<()>();
            // A failed spawn drops `done`, so the wait below ends at once.
            let _reporter = std::thread::Builder::new()
                .name("signal-exit-report".into())
                .spawn(move || {
                    report();
                    let _ = done.send(());
                });
            let _ = finished.recv_timeout(REPORT_CAP);
            // SAFETY: `_exit` is async-signal-safe and takes no pointers. It ends
            // the process without atexit handlers or destructors, on purpose: the
            // main and GTK/WebKit threads are still running, and no Rust invariant
            // here relies on a destructor running.
            unsafe { libc::_exit(128 + signo) }
        });
    if let Err(e) = spawned {
        log::error!("[signal] watchdog thread spawn failed: {e}");
    }
}

#[cfg(test)]
mod tests;
