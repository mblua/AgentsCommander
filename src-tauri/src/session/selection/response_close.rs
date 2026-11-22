//! Response-close effect records and the registry that retains them.
//!
//! Owns `ResponseCloseRegistration`, `ResponseEffectRegistry` and `ResponseEffectOwner`,
//! which record each close stage, the native exit and the telegram/readers `StopLane`
//! stops, and answer `discard_quiescence` with a `DiscardHold` reason. It never blocks
//! and runs no close itself; the close flow lives in `commands::session::destroy`.

use crate::resource_monitor::registry::termination::CleanupFailure;
use std::sync::atomic::Ordering;

use std::sync::{Arc, Mutex};

use crate::pty::manager::PtyManager;
use crate::session::session::SessionStatus;

#[derive(Clone)]
pub(crate) struct ResponseCloseRegistration {
    pub tracker: crate::pty::manager::ResponseCloseState,
    pub member: crate::pty::manager::ResponseRouteIdentity,
    pub coordinator: crate::pty::manager::ResponseRouteIdentity,
    pub carrier: crate::config::sessions_persistence::PublishedCarrier,
    pub diagnostic_last_poll_outcome: Arc<std::sync::atomic::AtomicU64>,
}

impl ResponseCloseRegistration {
    pub(super) fn diagnostic_poll_outcome(&self, code: u64, outcome: &'static str) {
        if log::log_enabled!(target: "agentscommander_lib::response_close_diag", log::Level::Debug)
            && self
                .diagnostic_last_poll_outcome
                .swap(code, Ordering::Relaxed)
                != code
        {
            log::debug!(
                target: "agentscommander_lib::response_close_diag",
                "response_close_diag id=unknown stage=poll_terminal outcome={} member={} \
                 coordinator={} site={}",
                outcome,
                self.member.session_id,
                self.coordinator.session_id,
                line!()
            );
        }
    }
}

pub(super) fn response_registration_row<'a>(
    current: &'a crate::session::manager::CarrierSnapshot,
    identity: &crate::pty::manager::ResponseRouteIdentity,
) -> Option<&'a crate::session::session::Session> {
    current.rows.iter().find(|row| {
        row.id == identity.session_id
            && row.token == identity.token
            && row.backend_kind == identity.backend
            && current.versions.get(&row.id) == Some(&identity.carrier_version)
            && !current.pending_ids.contains(&row.id)
            && !matches!(row.status, SessionStatus::Exited(_))
    })
}

pub(super) fn response_registration_route(
    pty: &Arc<Mutex<PtyManager>>,
    identity: &crate::pty::manager::ResponseRouteIdentity,
    row: &crate::session::session::Session,
) -> bool {
    let Ok(route) = PtyManager::snapshot_route_proof(pty, identity.session_id) else {
        return false;
    };
    route.backend_kind() == identity.backend
        && route.saved_cwd_matches_path(std::path::Path::new(&row.working_directory))
        && route.matches_requester_route(identity.backend, route.saved_cwd(), route.saved_replica())
}

pub struct ResponseEffectRegistry {
    #[cfg(test)]
    forced_session: Mutex<Option<uuid::Uuid>>,
    owners: &'static std::sync::OnceLock<
        Mutex<std::collections::HashMap<uuid::Uuid, Arc<ResponseEffectOwner>>>,
    >,
}

impl ResponseEffectRegistry {
    pub(crate) fn new() -> Self {
        Self {
            #[cfg(test)]
            forced_session: Mutex::new(None),
            // Retain the former static's owner records for the process lifetime.
            owners: Box::leak(Box::new(std::sync::OnceLock::new())),
        }
    }

    pub(crate) fn insert_if_absent(
        &self,
        id: uuid::Uuid,
        owner: Arc<ResponseEffectOwner>,
    ) -> Result<bool, ()> {
        let mut owners = self
            .owners
            .get_or_init(Default::default)
            .lock()
            .map_err(|_| ())?;
        if owners.contains_key(&id) {
            return Ok(false);
        }
        #[cfg(test)]
        {
            let mut forced = self.forced_session.lock().unwrap();
            if *forced == Some(id) {
                owner.forced_settlement.store(true, Ordering::Release);
                *forced = None;
            }
        }
        owners.insert(id, owner);
        Ok(true)
    }

    pub(crate) fn remove(&self, id: uuid::Uuid) -> Result<(), ()> {
        let mut owners = self
            .owners
            .get_or_init(Default::default)
            .lock()
            .map_err(|_| ())?;
        owners.remove(&id);
        Ok(())
    }

    pub(crate) fn snapshot(&self) -> Option<Vec<Arc<ResponseEffectOwner>>> {
        self.owners
            .get_or_init(Default::default)
            .lock()
            .ok()
            .map(|owners| owners.values().cloned().collect())
    }

    #[cfg(test)]
    pub(crate) fn force_next_settlement_for_test(&self, id: uuid::Uuid) {
        assert!(self.forced_session.lock().unwrap().replace(id).is_none());
    }
}

#[derive(Clone)]
enum ResponseEffectState<T> {
    NotStarted,
    Running,
    Completed(Result<T, String>),
    Ambiguous,
}

enum ResponseGroupWorker {
    #[cfg(test)]
    Legacy(
        tokio::task::JoinHandle<Result<crate::resource_monitor::types::ResourceKillResult, String>>,
    ),
    Diagnostic(
        tokio::task::JoinHandle<
            Result<crate::resource_monitor::types::ResourceKillResult, CleanupFailure>,
        >,
    ),
}

/// #3317 - a close stop whose completion is observed by a tokio task.
#[derive(Clone, Copy)]
pub(crate) enum StopLane {
    Telegram,
    Readers,
}

impl StopLane {
    fn index(self) -> usize {
        match self {
            Self::Telegram => 1,
            Self::Readers => 2,
        }
    }
}

/// #3317 - the exact-operation completion evidence of one close lane.
enum LaneEvidence {
    Missing,
    Quiescent,
    Ambiguous,
    Native(std::thread::JoinHandle<Result<(), String>>),
    Task(tokio::task::JoinHandle<bool>),
}

impl LaneEvidence {
    /// Turns a finished handle into its verdict. Never blocks: `join` and `now_or_never`
    /// run only after `is_finished()`.
    fn settle(&mut self) {
        let finished = match self {
            Self::Native(handle) => handle.is_finished(),
            Self::Task(handle) => handle.is_finished(),
            Self::Missing | Self::Quiescent | Self::Ambiguous => false,
        };
        if !finished {
            return;
        }
        *self = match std::mem::replace(self, Self::Ambiguous) {
            Self::Native(handle) => match handle.join() {
                Ok(Ok(())) => Self::Quiescent,
                _ => Self::Ambiguous,
            },
            Self::Task(mut handle) => match futures::FutureExt::now_or_never(&mut handle) {
                Some(Ok(true)) => Self::Quiescent,
                Some(Ok(false) | Err(_)) => Self::Ambiguous,
                None => Self::Task(handle),
            },
            settled => settled,
        };
    }
}

/// #3317 - why `discard_quiescence` does not prove the close quiescent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DiscardHold {
    OwnerRecordUnreadable,
    OwnerNotSealed,
    StageRunning,
    StageAmbiguous,
    StageMissing,
    KillNotOk,
    GroupRunning,
    GroupAmbiguous,
    NativeExitMissing,
    NativeExitRunning,
    NativeExitAmbiguous,
    TelegramMissing,
    TelegramRunning,
    TelegramAmbiguous,
    ReadersMissing,
    ReadersRunning,
    ReadersAmbiguous,
    OwnerUnavailable,
    OwnerIdentityMismatch,
    TrackerCloseMismatch,
    CarrierRetireRejected,
    NotExplicitDiscard,
}

impl DiscardHold {
    pub(crate) fn code(self) -> &'static str {
        match self {
            Self::OwnerRecordUnreadable => "owner_record_unreadable",
            Self::OwnerNotSealed => "owner_not_sealed",
            Self::StageRunning => "stage_running",
            Self::StageAmbiguous => "stage_ambiguous",
            Self::StageMissing => "stage_missing",
            Self::KillNotOk => "kill_not_ok",
            Self::GroupRunning => "group_running",
            Self::GroupAmbiguous => "group_ambiguous",
            Self::NativeExitMissing => "native_exit_missing",
            Self::NativeExitRunning => "native_exit_running",
            Self::NativeExitAmbiguous => "native_exit_ambiguous",
            Self::TelegramMissing => "telegram_missing",
            Self::TelegramRunning => "telegram_running",
            Self::TelegramAmbiguous => "telegram_ambiguous",
            Self::ReadersMissing => "readers_missing",
            Self::ReadersRunning => "readers_running",
            Self::ReadersAmbiguous => "readers_ambiguous",
            Self::OwnerUnavailable => "owner_unavailable",
            Self::OwnerIdentityMismatch => "owner_identity_mismatch",
            Self::TrackerCloseMismatch => "tracker_close_mismatch",
            Self::CarrierRetireRejected => "carrier_retire_rejected",
            Self::NotExplicitDiscard => "not_explicit_discard",
        }
    }
}

/// #3317 - per lane (native, telegram, readers): the missing, running and ambiguous holds.
const LANE_HOLDS: [[DiscardHold; 3]; 3] = [
    [
        DiscardHold::NativeExitMissing,
        DiscardHold::NativeExitRunning,
        DiscardHold::NativeExitAmbiguous,
    ],
    [
        DiscardHold::TelegramMissing,
        DiscardHold::TelegramRunning,
        DiscardHold::TelegramAmbiguous,
    ],
    [
        DiscardHold::ReadersMissing,
        DiscardHold::ReadersRunning,
        DiscardHold::ReadersAmbiguous,
    ],
];

/// #3317 - the complete stage sequence of a close that reached the auxiliary effects:
/// `kill` from `execute_response_close_core`, then the order `run_response_close_auxiliary`
/// writes. Any change to that writer's order must change this list.
const DISCARD_STAGES: [&str; 8] = [
    "kill", "detach", "telegram", "manager", "persist", "readers", "window", "events",
];

struct ResponseEffectRecord {
    stages: Vec<(&'static str, ResponseEffectState<()>)>,
    group: Option<ResponseEffectState<crate::resource_monitor::types::ResourceKillResult>>,
    group_worker: Option<ResponseGroupWorker>,
    sealed: bool,
    /// #3317 - index 0 native exit, 1 telegram stop, 2 readers stop.
    lanes: [LaneEvidence; 3],
}

pub(crate) struct ResponseEffectOwner {
    #[cfg(test)]
    forced_settlement: std::sync::atomic::AtomicBool,
    ticket: crate::pty::manager::ResponseCloseTicket,
    registration: ResponseCloseRegistration,
    backend: Arc<dyn crate::pty::backend::PtyBackend>,
    record: Mutex<ResponseEffectRecord>,
}

impl ResponseEffectOwner {
    /// Recorded completion only; this says nothing about native quiescence.
    pub(crate) fn diagnostic_terminal(&self) -> Option<bool> {
        let record = self.record.lock().ok()?;
        Some(
            record.sealed
                && record
                    .stages
                    .iter()
                    .all(|(_, s)| matches!(s, ResponseEffectState::Completed(_)))
                && record
                    .group
                    .as_ref()
                    .is_none_or(|s| matches!(s, ResponseEffectState::Completed(_))),
        )
    }

    #[cfg(test)]
    pub(crate) fn forced_settlement_for_test(&self) -> bool {
        self.forced_settlement.load(Ordering::Acquire)
    }

    #[cfg(test)]
    pub(crate) fn poison_record_for_test(self: &Arc<Self>) {
        let owner = self.clone();
        assert!(std::thread::spawn(move || {
            let _guard = owner.record.lock().unwrap();
            panic!("issue_3248 deliberate owner record poison");
        })
        .join()
        .is_err());
    }

    /// #3317 - sets a task lane back to `Missing` so a test can call the real
    /// `record_task_stop`; it never writes `Task`/`Quiescent` itself.
    #[cfg(test)]
    pub(crate) fn clear_task_lane_for_test(&self, lane: StopLane) {
        self.record.lock().unwrap().lanes[lane.index()] = LaneEvidence::Missing;
    }

    pub(crate) fn start(&self, name: &'static str) -> Option<usize> {
        let mut record = self.record.lock().ok()?;
        if record.sealed || record.stages.len() == record.stages.capacity() {
            return None;
        }
        let index = record.stages.len();
        record.stages.push((name, ResponseEffectState::NotStarted));
        record.stages[index].1 = ResponseEffectState::Running;
        Some(index)
    }

    pub(crate) fn complete(&self, index: usize, outcome: Result<(), String>) -> Option<()> {
        let mut record = self.record.lock().ok()?;
        let (_, stage) = record.stages.get_mut(index)?;
        if !matches!(stage, ResponseEffectState::Running) {
            return None;
        }
        *stage = ResponseEffectState::Completed(outcome);
        Some(())
    }

    pub(crate) fn terminal(&self) -> bool {
        let Ok(record) = self.record.lock() else {
            return false;
        };
        record.sealed
            && record
                .stages
                .iter()
                .all(|(_, stage)| matches!(stage, ResponseEffectState::Completed(_)))
            && record
                .group
                .as_ref()
                .is_none_or(|stage| matches!(stage, ResponseEffectState::Completed(_)))
    }

    pub(crate) fn kill_failed(&self) -> bool {
        self.record.lock().is_ok_and(|record| {
            record.stages.iter().any(|(name, stage)| {
                *name == "kill" && matches!(stage, ResponseEffectState::Completed(Err(_)))
            })
        })
    }

    pub(crate) fn seal(&self) -> Option<()> {
        self.record.lock().ok()?.sealed = true;
        Some(())
    }

    /// #3317 - records what the kill proved about the native child; first record wins.
    pub(crate) fn record_native_exit(&self, exit: crate::pty::backend::NativeExit) -> Option<()> {
        use crate::pty::backend::NativeExit;
        let mut record = self.record.lock().ok()?;
        let lane = &mut record.lanes[0];
        if !matches!(lane, LaneEvidence::Missing) {
            return None;
        }
        *lane = match exit {
            NativeExit::Observed => LaneEvidence::Quiescent,
            NativeExit::Reaping(handle) => LaneEvidence::Native(handle),
            NativeExit::Unobserved => LaneEvidence::Missing,
        };
        Some(())
    }

    /// #3317 - records the stop observation of a task lane; first record wins.
    pub(crate) fn record_task_stop(
        &self,
        lane: StopLane,
        stop: Result<Option<tokio::task::JoinHandle<bool>>, ()>,
    ) -> Option<()> {
        let mut record = self.record.lock().ok()?;
        let lane = &mut record.lanes[lane.index()];
        if !matches!(lane, LaneEvidence::Missing) {
            return None;
        }
        *lane = match stop {
            Ok(None) => LaneEvidence::Quiescent,
            Ok(Some(handle)) => LaneEvidence::Task(handle),
            Err(()) => LaneEvidence::Missing,
        };
        Some(())
    }

    /// #3317 - `Ok` only when the sealed record proves every close effect finished and
    /// every lane quiescent. Never blocks; settled lanes are stored back.
    pub(crate) fn discard_quiescence(&self) -> Result<(), DiscardHold> {
        let Ok(mut record) = self.record.lock() else {
            return Err(DiscardHold::OwnerRecordUnreadable);
        };
        if !record.sealed {
            return Err(DiscardHold::OwnerNotSealed);
        }
        let stages = &record.stages;
        if stages.iter().any(|(_, stage)| {
            matches!(
                stage,
                ResponseEffectState::NotStarted | ResponseEffectState::Running
            )
        }) {
            return Err(DiscardHold::StageRunning);
        }
        if stages
            .iter()
            .any(|(_, stage)| matches!(stage, ResponseEffectState::Ambiguous))
        {
            return Err(DiscardHold::StageAmbiguous);
        }
        if !stages.iter().map(|(name, _)| *name).eq(DISCARD_STAGES) {
            return Err(DiscardHold::StageMissing);
        }
        if !matches!(
            stages.first(),
            Some((_, ResponseEffectState::Completed(Ok(()))))
        ) {
            return Err(DiscardHold::KillNotOk);
        }
        if record.group_worker.is_some()
            || matches!(record.group, Some(ResponseEffectState::Running))
        {
            return Err(DiscardHold::GroupRunning);
        }
        if matches!(record.group, Some(ResponseEffectState::Ambiguous)) {
            return Err(DiscardHold::GroupAmbiguous);
        }
        for (lane, [missing, running, ambiguous]) in record.lanes.iter_mut().zip(LANE_HOLDS) {
            lane.settle();
            match lane {
                LaneEvidence::Quiescent => {}
                LaneEvidence::Missing => return Err(missing),
                LaneEvidence::Native(_) | LaneEvidence::Task(_) => return Err(running),
                LaneEvidence::Ambiguous => return Err(ambiguous),
            }
        }
        Ok(())
    }

    pub(crate) fn new(
        ticket: crate::pty::manager::ResponseCloseTicket,
        registration: ResponseCloseRegistration,
        backend: Arc<dyn crate::pty::backend::PtyBackend>,
    ) -> Self {
        Self {
            #[cfg(test)]
            forced_settlement: std::sync::atomic::AtomicBool::new(false),
            ticket,
            registration,
            backend,
            record: Mutex::new(ResponseEffectRecord {
                stages: Vec::with_capacity(16),
                group: None,
                group_worker: None,
                sealed: false,
                lanes: [
                    LaneEvidence::Missing,
                    LaneEvidence::Missing,
                    LaneEvidence::Missing,
                ],
            }),
        }
    }

    pub(crate) fn ticket(&self) -> &crate::pty::manager::ResponseCloseTicket {
        &self.ticket
    }

    pub(crate) fn registration(&self) -> &ResponseCloseRegistration {
        &self.registration
    }

    pub(crate) fn backend(&self) -> &Arc<dyn crate::pty::backend::PtyBackend> {
        &self.backend
    }

    #[cfg(test)]
    pub(crate) fn start_group(
        &self,
        start: impl FnOnce() -> tokio::task::JoinHandle<
            Result<crate::resource_monitor::types::ResourceKillResult, String>,
        >,
    ) -> Option<()> {
        let mut record = self.record.lock().ok()?;
        record.group = Some(ResponseEffectState::Running);
        record.group_worker = Some(ResponseGroupWorker::Legacy(start()));
        Some(())
    }

    pub(crate) fn start_group_diagnostic(
        &self,
        start: impl FnOnce() -> tokio::task::JoinHandle<
            Result<crate::resource_monitor::types::ResourceKillResult, CleanupFailure>,
        >,
    ) -> Option<()> {
        let mut record = self.record.lock().ok()?;
        record.group = Some(ResponseEffectState::Running);
        record.group_worker = Some(ResponseGroupWorker::Diagnostic(start()));
        Some(())
    }

    #[cfg(test)]
    pub(crate) fn poll_group(
        &self,
        cx: &mut std::task::Context<'_>,
        id: uuid::Uuid,
    ) -> std::task::Poll<Option<()>> {
        self.poll_group_diagnostic(cx, id)
            .map(|outcome| outcome.ok().flatten())
    }

    pub(crate) fn poll_group_diagnostic(
        &self,
        cx: &mut std::task::Context<'_>,
        id: uuid::Uuid,
    ) -> std::task::Poll<Result<Option<()>, CleanupFailure>> {
        // The JoinHandle stays in the retained owner while it is polled. Losing
        // this waiter does not drop the only handle or fabricate a group outcome.
        use std::future::Future;
        let Ok(mut record) = self.record.lock() else {
            return std::task::Poll::Pending;
        };
        let Some(worker) = record.group_worker.as_mut() else {
            return std::task::Poll::Pending;
        };
        let outcome = match worker {
            #[cfg(test)]
            ResponseGroupWorker::Legacy(worker) => std::pin::Pin::new(worker)
                .poll(cx)
                .map(|joined| joined.map(|result| result.map_err(CleanupFailure::Ordinary))),
            ResponseGroupWorker::Diagnostic(worker) => std::pin::Pin::new(worker).poll(cx),
        };
        match outcome {
            std::task::Poll::Pending => std::task::Poll::Pending,
            std::task::Poll::Ready(Ok(outcome)) => {
                let (stored, veto) = match outcome {
                    Ok(result) => (Ok(result), None),
                    Err(CleanupFailure::Ordinary(error)) => {
                        log::warn!(
                            "[session] resource cleanup failed session={id} source=ResponseClose: \
                             {error}"
                        );
                        (Err(error), None)
                    }
                    Err(error @ CleanupFailure::EvidenceVeto { .. }) => {
                        (Err("not_called:evidence_veto".into()), Some(error))
                    }
                };
                record.group = Some(ResponseEffectState::Completed(stored));
                let _ = record.group_worker.take();
                std::task::Poll::Ready(match veto {
                    Some(error) => Err(error),
                    None => Ok(Some(())),
                })
            }
            std::task::Poll::Ready(Err(error)) => {
                record.group = Some(ResponseEffectState::Ambiguous);
                let _ = record.group_worker.take();
                log::warn!(
                    "[session] resource cleanup task failed session={id} source=ResponseClose: \
            {error}"
                );
                std::task::Poll::Ready(Ok(None))
            }
        }
    }
}
