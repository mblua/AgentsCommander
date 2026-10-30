//! Startup restore decisions and the restore task.

pub(crate) mod native;

use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;

use crate::config::sessions_persistence;
use crate::config::settings::SettingsState;

use crate::app_state::RestoreInProgress;
use crate::{commands, config};

/// Decide whether a persisted session should be restored with a live PTY
/// at app startup, or deferred (created as a dormant `Exited(0)` record).
///
/// Inputs:
///   - `setting_on`: value of `AppSettings::restore_coordinator_wake_state`.
///   - `is_coord`: whether the agent FQN derived from `ps.working_directory`
///     is a coordinator of any discovered team.
///   - `persisted_status`: `PersistedSession::status` as snapshotted at the
///     last app shutdown. `None` means the snapshot was taken by an older
///     binary that did not record status. Treat `None` as **awake** for
///     forward-compat — better to wake a coord the user expected to be
///     awake than silently leave it dormant on first launch after upgrade.
///
/// Returns true ⇒ restore with PTY; false ⇒ defer (dormant).
pub(crate) fn should_wake_on_restore(
    setting_on: bool,
    is_coord: bool,
    persisted_status: Option<&crate::session::session::SessionStatus>,
) -> bool {
    if !setting_on {
        return false; // Setting OFF: defer everything.
    }
    if !is_coord {
        return false; // Non-coord: always deferred under the new policy.
    }
    match persisted_status {
        Some(crate::session::session::SessionStatus::Exited(_)) => false, // asleep at shutdown
        Some(_) | None => true, // awake at shutdown (or unknown → fail-open)
    }
}

/// (#1793) The non-coordinator arm of the restore wake decision. #248's
/// `should_wake_on_restore` covers coordinators and is left unchanged. This is
/// the ONLY rule that can wake a non-coordinator, and the two arms are disjoint
/// on `is_coord`, so they can never both fire for one row.
pub(crate) fn should_wake_working_agent_on_restore(
    resume_agents_on: bool,
    is_coord: bool,
    persisted_working: bool,
) -> bool {
    resume_agents_on && !is_coord && persisted_working
}

pub(crate) fn restore_session_should_wake(
    archived_session: bool,
    setting_on: bool,
    resume_agents_on: bool,
    is_coord: bool,
    persisted_status: Option<&crate::session::session::SessionStatus>,
    persisted_working: bool,
) -> bool {
    !archived_session
        && (should_wake_on_restore(setting_on, is_coord, persisted_status)
            || should_wake_working_agent_on_restore(resume_agents_on, is_coord, persisted_working))
}

pub(crate) fn restore_session_should_become_active(
    was_active: bool,
    archived_session: bool,
) -> bool {
    was_active && !archived_session
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PersistedActiveFlagNormalization {
    Zero,
    One { index: usize },
    Multiple { identities: Vec<String> },
}

/// Normalize persisted canonical-selection intent before any restore side
/// effect. A single flag remains authoritative. Multiple flags are corrupt
/// input, so all are cleared and final restore uses the documented first
/// eligible live attached fallback.
pub(crate) fn normalize_persisted_active_flags(
    sessions: &mut [crate::config::sessions_persistence::PersistedSession],
) -> PersistedActiveFlagNormalization {
    let flagged = sessions
        .iter()
        .enumerate()
        .filter(|(_, session)| session.was_active)
        .map(|(index, session)| {
            (
                index,
                format!("{}:{}@{}", index, session.name, session.working_directory),
            )
        })
        .collect::<Vec<_>>();
    match flagged.as_slice() {
        [] => PersistedActiveFlagNormalization::Zero,
        [(index, _)] => PersistedActiveFlagNormalization::One { index: *index },
        _ => {
            let identities = flagged.into_iter().map(|(_, identity)| identity).collect();
            for session in sessions {
                session.was_active = false;
            }
            PersistedActiveFlagNormalization::Multiple { identities }
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct RestoreObserverStartBarrier {
    phase: AtomicU8,
}

impl RestoreObserverStartBarrier {
    pub(crate) fn mark_restore_admitted(&self) -> Result<(), String> {
        self.phase
            .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| ())
            .map_err(|phase| format!("restore observer barrier admission from phase {phase}"))
    }

    pub(crate) fn mark_restore_complete(&self) -> Result<(), String> {
        self.phase
            .compare_exchange(1, 2, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| ())
            .map_err(|phase| format!("restore observer barrier completion from phase {phase}"))
    }

    pub(crate) fn start(&self, producer: &str, start: impl FnOnce()) -> Result<(), String> {
        if self.phase.load(Ordering::Acquire) != 2 {
            return Err(format!(
                "startup producer {producer} attempted before restore completion"
            ));
        }
        start();
        Ok(())
    }
}

/// (#630) Resolve coordinator status for a restore decision, backstopping a
/// transient empty `discover_teams()` with the snapshot's persisted
/// `is_coordinator`. When live discovery returned teams we trust it. Only when
/// discovery came back EMPTY do we fall back, so a real coordinator is not
/// silently downgraded to "deferred" because project paths were not ready at
/// cold start. The woken session's `is_coordinator` is recomputed in
/// `create_session_inner`, so a stale backstop cannot poison identity.
pub(crate) fn resolve_is_coord_for_restore(
    live_is_coord: bool,
    teams_empty: bool,
    persisted_is_coord: bool,
) -> bool {
    live_is_coord || (teams_empty && persisted_is_coord)
}

/// (#630/#631) Bridge the persisted `start_fresh_on_restore` intent into the
/// restore wake path's `skip_auto_resume` argument. The two are value-identical
/// (both `true` => start fresh / suppress `--continue`); this is the single
/// named seam the wake path reads. Anti-revert guard: CI runs
/// `cargo clippy --all-targets -- -D warnings`, so reverting the call site to a
/// hardcoded value leaves this function unused and the `dead_code` lint fails the
/// build. The unit test `wake_path_passes_persisted_fresh_intent` pins this
/// bridge's own behavior but would not, by itself, catch a reverted call site.
pub(crate) fn skip_auto_resume_for_restore(start_fresh_on_restore: bool) -> bool {
    start_fresh_on_restore
}

pub(crate) fn should_wake_root_agent_on_restore(
    persisted_status: Option<&crate::session::session::SessionStatus>,
) -> bool {
    match persisted_status {
        Some(crate::session::session::SessionStatus::Exited(_)) => false,
        Some(crate::session::session::SessionStatus::Active)
        | Some(crate::session::session::SessionStatus::Running)
        | Some(crate::session::session::SessionStatus::Idle)
        | None => true,
    }
}

pub(crate) fn should_auto_create_root_agent_on_first_restore(
    settings: &crate::config::settings::AppSettings,
    last_coding_agent: Option<&str>,
) -> bool {
    commands::session::resolve_root_agent_command(settings, None, last_coding_agent).is_ok()
}

/// #1341 - releases the restore selection barrier and completes the observer
/// barrier. `complete()` is the normal path; `Drop` is the backstop for a
/// panic anywhere in the spawned restore task, so the selection coordinator
/// (which queues every webview session command behind the restore barrier)
/// can never wedge.
pub(crate) struct RestoreCompletionGuard {
    barrier: Option<crate::session::selection::RestoreBarrierGuard>,
    observer: Arc<RestoreObserverStartBarrier>,
    completed: bool,
}

impl RestoreCompletionGuard {
    pub(crate) fn new(
        barrier: crate::session::selection::RestoreBarrierGuard,
        observer: Arc<RestoreObserverStartBarrier>,
    ) -> Self {
        Self {
            barrier: Some(barrier),
            observer,
            completed: false,
        }
    }

    fn complete(&mut self) {
        if let Some(barrier) = self.barrier.take() {
            barrier.finish();
        }
        if let Err(e) = self.observer.mark_restore_complete() {
            log::error!("[restore] observer barrier completion failed: {}", e);
        }
        self.completed = true;
    }
}

impl Drop for RestoreCompletionGuard {
    fn drop(&mut self) {
        if !self.completed {
            log::error!(
                "[restore] restore task ended abnormally; force-releasing the selection barrier"
            );
            if let Some(barrier) = self.barrier.take() {
                barrier.finish();
            }
        }
    }
}
pub(crate) fn load_startup_sessions(
    settings: &config::settings::AppSettings,
    load: impl FnOnce(&[String]) -> Vec<sessions_persistence::PersistedSession>,
) -> Result<Vec<sessions_persistence::PersistedSession>, String> {
    let paths = sessions_persistence::session_retention_project_paths(settings)?;
    Ok(load(&paths))
}

pub(crate) trait RestoreStartupTask:
    std::future::Future<Output = Result<(), Self::Error>> + Send + Unpin + 'static
{
    type Error;

    fn spawn_owned(future: futures::future::BoxFuture<'static, ()>) -> Self;
}

/// Shared admission and completion boundary for native startup restore.
/// Callbacks are invoked inside the unwind boundary, including synchronous panics.
pub(crate) fn spawn_restore_startup<J: RestoreStartupTask>(
    restore_barrier: crate::session::selection::RestoreBarrierGuard,
    restore_observer_barrier: Arc<RestoreObserverStartBarrier>,
    restore_flag: Arc<RestoreInProgress>,
    settings_state: SettingsState,
    body: Box<dyn FnOnce() -> futures::future::BoxFuture<'static, ()> + Send + 'static>,
    continuation: Box<dyn FnOnce() -> futures::future::BoxFuture<'static, ()> + Send + 'static>,
) -> J {
    use futures::FutureExt;
    J::spawn_owned(Box::pin(async move {
        struct RestoreGuard(Arc<RestoreInProgress>);
        impl Drop for RestoreGuard {
            fn drop(&mut self) {
                self.0 .0.store(false, std::sync::atomic::Ordering::SeqCst);
            }
        }
        let restore_guard = RestoreGuard(restore_flag);
        let mut completion = RestoreCompletionGuard::new(restore_barrier, restore_observer_barrier);
        let caught_body = std::panic::AssertUnwindSafe(async move {
            let read_guard = match settings_state.current_read().await {
                Ok(guard) => guard,
                Err(error) => {
                    log::debug!(
                        "[restore] settings unavailable; automatic restore skipped: {error}"
                    );
                    return;
                }
            };
            drop(read_guard);
            (body)().await;
        });
        if let Err(panic) = caught_body.catch_unwind().await {
            log::error!(
                "[restore] restore task panicked (partial restore; retried next boot): {:?}",
                panic
            );
        }
        drop(restore_guard);
        completion.complete();
        let tail = std::panic::AssertUnwindSafe(async move { (continuation)().await });
        if let Err(panic) = tail.catch_unwind().await {
            log::error!(
                "[restore] post-restore startup panicked (services after the panic point not \
                    started): {:?}",
                panic
            );
        }
    }))
}
