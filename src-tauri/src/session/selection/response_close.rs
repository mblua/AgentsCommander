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

struct ResponseEffectRecord {
    stages: Vec<(&'static str, ResponseEffectState<()>)>,
    group: Option<ResponseEffectState<crate::resource_monitor::types::ResourceKillResult>>,
    group_worker: Option<
        tokio::task::JoinHandle<Result<crate::resource_monitor::types::ResourceKillResult, String>>,
    >,
    sealed: bool,
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
        self.record.lock().ok().is_some_and(|record| {
            record.stages.iter().any(|(name, stage)| {
                *name == "kill" && matches!(stage, ResponseEffectState::Completed(Err(_)))
            })
        })
    }

    pub(crate) fn seal(&self) -> Option<()> {
        self.record.lock().ok()?.sealed = true;
        Some(())
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

    pub(crate) fn start_group(
        &self,
        start: impl FnOnce() -> tokio::task::JoinHandle<
            Result<crate::resource_monitor::types::ResourceKillResult, String>,
        >,
    ) -> Option<()> {
        let mut record = self.record.lock().ok()?;
        record.group = Some(ResponseEffectState::Running);
        record.group_worker = Some(start());
        Some(())
    }

    pub(crate) fn poll_group(
        &self,
        cx: &mut std::task::Context<'_>,
        id: uuid::Uuid,
    ) -> std::task::Poll<Option<()>> {
        // The JoinHandle stays in the retained owner while it is polled. Losing
        // this waiter does not drop the only handle or fabricate a group outcome.
        use std::future::Future;
        let Ok(mut record) = self.record.lock() else {
            return std::task::Poll::Pending;
        };
        let Some(worker) = record.group_worker.as_mut() else {
            return std::task::Poll::Pending;
        };
        match std::pin::Pin::new(worker).poll(cx) {
            std::task::Poll::Pending => std::task::Poll::Pending,
            std::task::Poll::Ready(Ok(outcome)) => {
                if let Err(error) = &outcome {
                    log::warn!(
                        "[session] resource cleanup failed session={} source=ResponseClose: {}",
                        id,
                        error
                    );
                }
                record.group = Some(ResponseEffectState::Completed(outcome));
                let _ = record.group_worker.take();
                std::task::Poll::Ready(Some(()))
            }
            std::task::Poll::Ready(Err(error)) => {
                record.group = Some(ResponseEffectState::Ambiguous);
                let _ = record.group_worker.take();
                log::warn!(
                    "[session] resource cleanup task failed session={} source=ResponseClose: {}",
                    id,
                    error
                );
                std::task::Poll::Ready(None)
            }
        }
    }
}
