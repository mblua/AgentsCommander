use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Instant;

use uuid::Uuid;

use crate::session::profile::CodingAgentKind;

use super::thresholds::Thresholds;
use super::{
    registry, ChildLiveness, SpawnRecord, SpawnRecordInit, SpawnWindow, RECENT_SPAWNS_CAP,
};

/// Count the spawns that happened in the window BEFORE this one, then record this
/// attempt. Call once per spawn, at the top of the spawn path.
pub fn note_spawn_attempt(cli: Option<CodingAgentKind>, thresholds: Thresholds) -> SpawnWindow {
    let window = thresholds.concurrency_window;
    let now = Instant::now();
    let mut registry = registry().lock().unwrap_or_else(|e| e.into_inner());

    while let Some((at, _)) = registry.recent.front() {
        if now.duration_since(*at) > window {
            registry.recent.pop_front();
        } else {
            break;
        }
    }

    let total = registry.recent.len();
    let same_cli = match cli {
        Some(kind) => registry
            .recent
            .iter()
            .filter(|(_, recent_cli)| *recent_cli == Some(kind))
            .count(),
        None => 0,
    };

    registry.recent.push_back((now, cli));
    if registry.recent.len() > RECENT_SPAWNS_CAP {
        registry.recent.pop_front();
    }

    SpawnWindow {
        window_ms: window.as_millis().min(u64::MAX as u128) as u64,
        total,
        same_cli,
    }
}

/// Register a spawned child and emit its spawn record.
pub fn register(init: SpawnRecordInit) -> Arc<SpawnRecord> {
    let record = Arc::new(SpawnRecord::new(init));
    record.log_spawn();
    registry()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .records
        .insert(record.session_id, Arc::clone(&record));
    record
}

/// Tag a session's child as stopped by AC (session kill, job terminate,
/// resource-monitor `kill_group`, shutdown) BEFORE any process is touched, so its exit
/// can never be misread as a spontaneous death. `pre_stop` is the liveness the caller
/// observed before touching anything: pass it whenever you have it, it is the only
/// non-racy witness of "was it already dead when we asked?". No-op for unknown sessions
/// (container transport, or a session with no local spawn record).
pub fn mark_ac_stop(
    session_id: Uuid,
    source: &str,
    pre_stop: Option<ChildLiveness>,
) -> Option<Arc<SpawnRecord>> {
    let record = registry()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .records
        .get(&session_id)
        .cloned()?;
    record.mark_ac_stop(source, pre_stop);
    Some(record)
}

pub fn forget(session_id: Uuid) {
    registry()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .records
        .remove(&session_id);
}

/// #1271 - read the spawn record for one session, when one exists. Test
/// postcondition seam (Finding E): lets a test assert that a rejected spawn
/// never recorded launch provenance, and lets the Windows integration
/// regression read the exact adapted argv and awaited child exit.
pub fn record_for(session_id: Uuid) -> Option<Arc<SpawnRecord>> {
    registry()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .records
        .get(&session_id)
        .cloned()
}

/// One monitor thread per local session. It owns two signals the PTY reader cannot
/// carry: the startup verdict at the deadline, and the exit of a child that dies
/// without AC asking (ConPTY keeps the pipe open past the death of the child, so reader
/// EOF is not a reliable witness). It ends when the child exits, when the session is
/// gone, or when the probe itself panics.
pub fn watch_child<P>(record: Arc<SpawnRecord>, probe: P) -> JoinHandle<()>
where
    P: Fn() -> ChildLiveness + Send + 'static,
{
    std::thread::spawn(move || loop {
        let tick = if record.in_startup_phase() {
            record.thresholds.startup_poll
        } else {
            record.thresholds.steady_poll
        };
        std::thread::sleep(tick);

        // Read the stop state BEFORE looking at the child: a stop that lands after this
        // point cannot be what killed a child the probe is about to find already dead.
        let stop_before = record.stop_snapshot();

        // portable-pty unwraps its own internals while polling a child. A poisoned
        // mutex in there must not take this thread down silently.
        let liveness = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(&probe)) {
            Ok(liveness) => liveness,
            Err(_) => {
                log::warn!(
                    "[pty] child-probe panicked for session {}; stopping its monitor",
                    record.session_id()
                );
                return;
            }
        };

        match liveness {
            ChildLiveness::Gone => {
                record.record_final_liveness(&liveness);
                return;
            }
            ChildLiveness::Exited { .. } => {
                record.record_final_liveness(&liveness);
                let cause = record.attribute_exit(stop_before);
                record.log_child_exit(cause, &liveness, "observed-by-monitor");
                return;
            }
            ChildLiveness::Alive | ChildLiveness::Unqueryable(_) => {
                if record.startup_deadline_reached() {
                    record.log_startup_report(&liveness);
                }
            }
        }
    })
}
