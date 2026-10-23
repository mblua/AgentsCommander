use super::{
    lock_mutex_until, ContainerShutdownReport, CONTAINER_SHUTDOWN_DROP_GRACE,
    CONTAINER_SHUTDOWN_FALLBACK_CAPACITY, CONTAINER_SHUTDOWN_IDLE_RECHECK, CONTAINER_SHUTDOWN_POLL,
    CONTAINER_SHUTDOWN_QUEUE_CAPACITY, CONTAINER_SHUTDOWN_WORKER_CAPACITY,
};
use crate::pty::container_runtime::ContainerRuntimeControl;
use std::collections::{HashMap, VecDeque};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::{Arc, Condvar, Mutex, OnceLock, TryLockError};
use std::time::{Duration, Instant};
use uuid::Uuid;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(super) enum ContainerShutdownPhase {
    #[default]
    Accepting,
    Draining,
    GlobalSweep,
}

#[cfg(test)]
impl ContainerShutdownPhase {
    pub(super) fn is_sealed(self) -> bool {
        self != Self::Accepting
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ContainerShutdownWorkProvenance {
    PreSealProducer,
    GlobalSweep,
}

impl ContainerShutdownWorkProvenance {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::PreSealProducer => "preSealProducer",
            Self::GlobalSweep => "globalSweep",
        }
    }
}

#[derive(Default)]
pub(super) struct ContainerShutdownWorkState {
    pub(super) phase: ContainerShutdownPhase,
    pub(super) active_producers: usize,
    pub(super) queued: VecDeque<ContainerShutdownWork>,
    pub(super) retained: VecDeque<ContainerShutdownWork>,
    pub(super) active: HashMap<u64, ContainerShutdownWorkContext>,
    pub(super) active_fallbacks: usize,
    pub(super) next_task_id: u64,
    pub(super) terminating: bool,
    pub(super) worker_count: usize,
    pub(super) sweep_deadline: Option<Instant>,
    #[cfg(test)]
    pub(super) fail_worker_spawn: bool,
}

pub(super) struct ContainerShutdownWorkRegistry {
    pub(super) shared: Arc<ContainerShutdownWorkShared>,
    pub(super) workers: Mutex<Vec<std::thread::JoinHandle<()>>>,
}

pub(super) struct ContainerShutdownProducer {
    pub(super) registry: Arc<ContainerShutdownWorkRegistry>,
}

pub(super) struct ContainerShutdownWorkShared {
    pub(super) state: Mutex<ContainerShutdownWorkState>,
    pub(super) state_changed: Condvar,
    pub(super) work_available: Condvar,
    pub(super) control: ContainerRuntimeControl,
}

#[derive(Clone)]
pub(super) struct ContainerShutdownWorkContext {
    pub(super) session_id: Option<Uuid>,
    pub(super) reason: &'static str,
    pub(super) provenance: ContainerShutdownWorkProvenance,
}

pub(super) struct ContainerShutdownWork {
    pub(super) id: u64,
    pub(super) context: ContainerShutdownWorkContext,
    pub(super) control: ContainerRuntimeControl,
    pub(super) run: Box<dyn FnOnce(&ContainerRuntimeControl) + Send + 'static>,
}

impl Default for ContainerShutdownWorkRegistry {
    fn default() -> Self {
        Self {
            shared: Arc::new(ContainerShutdownWorkShared {
                state: Mutex::new(ContainerShutdownWorkState::default()),
                state_changed: Condvar::new(),
                work_available: Condvar::new(),
                control: ContainerRuntimeControl::default(),
            }),
            workers: Mutex::new(Vec::new()),
        }
    }
}

pub(super) fn decrement_shutdown_worker_count(
    state: &mut ContainerShutdownWorkState,
    shared: &ContainerShutdownWorkShared,
) {
    if state.worker_count == 0 {
        log::error!("[container-transport] shutdown worker count underflow");
    } else {
        state.worker_count -= 1;
    }
    shared.state_changed.notify_all();
}

impl ContainerShutdownWorkRegistry {
    pub(super) fn register_producer(self: &Arc<Self>) -> Option<ContainerShutdownProducer> {
        let mut state = self
            .shared
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if state.phase != ContainerShutdownPhase::Accepting {
            return None;
        }
        state.active_producers += 1;
        Some(ContainerShutdownProducer {
            registry: Arc::clone(self),
        })
    }

    pub(super) fn spawn_producer_owned<F>(
        &self,
        session_id: Option<Uuid>,
        reason: &'static str,
        work: F,
    ) where
        F: FnOnce(&ContainerRuntimeControl) + Send + 'static,
    {
        let control = self.shared.control.clone();
        let context = ContainerShutdownWorkContext {
            session_id,
            reason,
            provenance: ContainerShutdownWorkProvenance::PreSealProducer,
        };
        let mut work = Some(Box::new(work) as Box<dyn FnOnce(&ContainerRuntimeControl) + Send>);
        let worker_count = self.ensure_workers(&context);
        let queued = if worker_count == 0 {
            false
        } else {
            let mut state = self
                .shared
                .state
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if state.terminating || state.queued.len() >= CONTAINER_SHUTDOWN_QUEUE_CAPACITY {
                false
            } else {
                let id = state.next_task_id;
                state.next_task_id = state.next_task_id.wrapping_add(1);
                match work.take() {
                    Some(run) => {
                        state.queued.push_back(ContainerShutdownWork {
                            id,
                            context: context.clone(),
                            control: control.clone(),
                            run,
                        });
                        self.shared.work_available.notify_one();
                        true
                    }
                    None => {
                        log::error!(
                            "[container-transport] shutdown work ownership missing before \
                                admission session={} reason={}",
                            context
                                .session_id
                                .map(|id| id.to_string())
                                .unwrap_or_else(|| "none".to_string()),
                            context.reason
                        );
                        false
                    }
                }
            }
        };
        if queued {
            return;
        }

        log::warn!(
            "[container-transport] drain-owned shutdown work admission unavailable; using \
                serialized cooperative fallback session={} reason={} workers={} queueCapacity={}",
            context
                .session_id
                .map(|id| id.to_string())
                .unwrap_or_else(|| "none".to_string()),
            context.reason,
            worker_count,
            CONTAINER_SHUTDOWN_QUEUE_CAPACITY
        );
        if let Some(work) = work.take() {
            self.run_producer_fallback(context, control, work, worker_count);
        }
    }

    pub(super) fn run_producer_fallback(
        &self,
        context: ContainerShutdownWorkContext,
        control: ContainerRuntimeControl,
        work: Box<dyn FnOnce(&ContainerRuntimeControl) + Send + 'static>,
        worker_count: usize,
    ) {
        let mut work = Some(work);
        let id = {
            let mut state = self
                .shared
                .state
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            loop {
                let deadline = control.shutdown_deadline();
                if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
                    let id = state.next_task_id;
                    state.next_task_id = state.next_task_id.wrapping_add(1);
                    if let Some(run) = work.take() {
                        state.retained.push_back(ContainerShutdownWork {
                            id,
                            context: context.clone(),
                            control: control.clone(),
                            run,
                        });
                    }
                    self.shared.work_available.notify_one();
                    self.shared.state_changed.notify_all();
                    log::error!(
                        "[container-transport] drain-owned cooperative fallback retained at the \
                            absolute deadline session={} reason={} workers={} state=retained",
                        context
                            .session_id
                            .map(|id| id.to_string())
                            .unwrap_or_else(|| "none".to_string()),
                        context.reason,
                        worker_count
                    );
                    return;
                }
                if state.active_fallbacks < CONTAINER_SHUTDOWN_FALLBACK_CAPACITY {
                    let id = state.next_task_id;
                    state.next_task_id = state.next_task_id.wrapping_add(1);
                    state.active.insert(id, context.clone());
                    state.active_fallbacks += 1;
                    break id;
                }
                state = if let Some(deadline) = deadline {
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    match self.shared.state_changed.wait_timeout(state, remaining) {
                        Ok((state, _)) => state,
                        Err(error) => error.into_inner().0,
                    }
                } else {
                    self.shared
                        .state_changed
                        .wait(state)
                        .unwrap_or_else(|error| error.into_inner())
                };
            }
        };
        if let Some(work) = work.take() {
            run_container_shutdown_work(work, &control, &context);
        }
        let mut state = self
            .shared
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        state.active.remove(&id);
        if state.active_fallbacks == 0 {
            log::error!(
                "[container-transport] synchronous shutdown work count underflow session={} \
                    reason={}",
                context
                    .session_id
                    .map(|id| id.to_string())
                    .unwrap_or_else(|| "none".to_string()),
                context.reason
            );
        } else {
            state.active_fallbacks -= 1;
        }
        self.shared.state_changed.notify_all();
    }

    pub(super) fn start_global_sweep_epoch(&self, deadline: Instant) -> bool {
        if Instant::now() >= deadline {
            return false;
        }
        let Some(mut state) = lock_mutex_until(&self.shared.state, deadline) else {
            return false;
        };
        state.phase = ContainerShutdownPhase::GlobalSweep;
        state.terminating = false;
        state.sweep_deadline = Some(deadline);
        drop(state);
        self.shared.work_available.notify_all();
        let context = ContainerShutdownWorkContext {
            session_id: None,
            reason: "global-sweep-epoch",
            provenance: ContainerShutdownWorkProvenance::GlobalSweep,
        };
        self.ensure_workers(&context) > 0
    }

    pub(super) fn has_owned_work_until(&self, deadline: Instant) -> Option<bool> {
        lock_mutex_until(&self.shared.state, deadline).map(|state| {
            state.active_producers > 0
                || !state.queued.is_empty()
                || !state.retained.is_empty()
                || !state.active.is_empty()
                || state.worker_count > 0
        })
    }

    pub(super) fn spawn_global_sweep_owned_with_control<F>(
        &self,
        session_id: Option<Uuid>,
        reason: &'static str,
        control: ContainerRuntimeControl,
        deadline: Instant,
        work: F,
    ) -> bool
    where
        F: FnOnce(&ContainerRuntimeControl) + Send + 'static,
    {
        if Instant::now() >= deadline {
            return false;
        }
        let context = ContainerShutdownWorkContext {
            session_id,
            reason,
            provenance: ContainerShutdownWorkProvenance::GlobalSweep,
        };
        if self.ensure_workers(&context) == 0 {
            return false;
        }
        let Some(mut state) = lock_mutex_until(&self.shared.state, deadline) else {
            return false;
        };
        if state.phase != ContainerShutdownPhase::GlobalSweep
            || state.terminating
            || state.queued.len() >= CONTAINER_SHUTDOWN_QUEUE_CAPACITY
            || Instant::now() >= deadline
        {
            return false;
        }
        let id = state.next_task_id;
        state.next_task_id = state.next_task_id.wrapping_add(1);
        state.queued.push_back(ContainerShutdownWork {
            id,
            context,
            control,
            run: Box::new(work),
        });
        self.shared.work_available.notify_one();
        true
    }

    pub(super) fn ensure_workers(&self, context: &ContainerShutdownWorkContext) -> usize {
        let mut workers = self
            .workers
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let mut retained = Vec::with_capacity(workers.len());
        for worker in workers.drain(..) {
            if worker.is_finished() {
                if worker.join().is_err() {
                    {
                        let mut state = self
                            .shared
                            .state
                            .lock()
                            .unwrap_or_else(|error| error.into_inner());
                        decrement_shutdown_worker_count(&mut state, &self.shared);
                    }
                    log::error!(
                        "[container-transport] shutdown worker exited after panic session={} \
                            reason={}",
                        context
                            .session_id
                            .map(|id| id.to_string())
                            .unwrap_or_else(|| "none".to_string()),
                        context.reason
                    );
                }
            } else {
                retained.push(worker);
            }
        }
        *workers = retained;

        let deficit = {
            let state = self
                .shared
                .state
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if state.terminating {
                0
            } else {
                CONTAINER_SHUTDOWN_WORKER_CAPACITY.saturating_sub(state.worker_count)
            }
        };
        for _ in 0..deficit {
            #[cfg(test)]
            if self
                .shared
                .state
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .fail_worker_spawn
            {
                log::warn!(
                    "[container-transport] injected shutdown worker spawn failure session={} \
                        reason={}",
                    context
                        .session_id
                        .map(|id| id.to_string())
                        .unwrap_or_else(|| "none".to_string()),
                    context.reason
                );
                break;
            }

            let shared = Arc::clone(&self.shared);
            let worker_index = workers.len();
            {
                let mut state = self
                    .shared
                    .state
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                state.worker_count += 1;
            }
            match std::thread::Builder::new()
                .name(format!("ac-container-shutdown-{worker_index}"))
                .spawn(move || container_shutdown_worker(shared))
            {
                Ok(worker) => workers.push(worker),
                Err(error) => {
                    {
                        let mut state = self
                            .shared
                            .state
                            .lock()
                            .unwrap_or_else(|error| error.into_inner());
                        decrement_shutdown_worker_count(&mut state, &self.shared);
                    }
                    log::warn!(
                        "[container-transport] shutdown worker spawn failed session={} reason={} \
                            worker={} error={}",
                        context
                            .session_id
                            .map(|id| id.to_string())
                            .unwrap_or_else(|| "none".to_string()),
                        context.reason,
                        worker_index,
                        error
                    );
                    break;
                }
            }
        }
        self.shared
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .worker_count
    }

    pub(super) fn begin_shutdown(&self, deadline: Instant) -> bool {
        self.shared.control.request_shutdown(deadline);
        let Some(mut state) = lock_mutex_until(&self.shared.state, deadline) else {
            self.shared.work_available.notify_all();
            self.shared.state_changed.notify_all();
            log::error!(
                "[container-transport] shutdown state lock reached absolute deadline \
                    reason=shutdown-begin state=retained"
            );
            return false;
        };
        if state.phase == ContainerShutdownPhase::Accepting {
            state.phase = ContainerShutdownPhase::Draining;
        }
        self.shared.work_available.notify_all();
        self.shared.state_changed.notify_all();
        true
    }

    pub(super) fn seal_and_drain_until(&self, deadline: Instant) -> ContainerShutdownReport {
        if !self.begin_shutdown(deadline) {
            return ContainerShutdownReport {
                terminal: false,
                retained: vec!["reason=shutdown-begin state=retained".to_string()],
            };
        }
        let mut drained = false;
        loop {
            let Some(mut state) = lock_mutex_until(&self.shared.state, deadline) else {
                break;
            };
            let work_drained = state.active_producers == 0
                && state.queued.is_empty()
                && state.retained.is_empty()
                && state.active.is_empty();
            if work_drained {
                state.terminating = true;
                self.shared.work_available.notify_all();
            }
            drained = work_drained && state.worker_count == 0;
            drop(state);
            if drained {
                break;
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            std::thread::sleep(CONTAINER_SHUTDOWN_POLL.min(remaining));
        }

        let mut worker_lock_retained = false;
        if let Ok(mut workers) = self.workers.try_lock() {
            let mut unfinished = Vec::new();
            for worker in workers.drain(..) {
                if worker.is_finished() {
                    if worker.join().is_err() {
                        {
                            let mut state = self
                                .shared
                                .state
                                .lock()
                                .unwrap_or_else(|error| error.into_inner());
                            decrement_shutdown_worker_count(&mut state, &self.shared);
                        }
                        log::error!(
                            "[container-transport] shutdown worker panicked during bounded join"
                        );
                    }
                } else {
                    unfinished.push(worker);
                }
            }
            *workers = unfinished;
        } else {
            worker_lock_retained = true;
        }

        let state = match self.shared.state.try_lock() {
            Ok(state) => state,
            Err(TryLockError::Poisoned(error)) => error.into_inner(),
            Err(TryLockError::WouldBlock) => {
                let mut retained =
                    vec!["reason=shutdown-state-snapshot state=retained".to_string()];
                if worker_lock_retained {
                    retained.push("workers=unknown state=retained".to_string());
                }
                log::error!(
                    "[container-transport] shared shutdown deadline retained ownership because the \
                        state snapshot lock is unavailable"
                );
                return ContainerShutdownReport {
                    terminal: false,
                    retained,
                };
            }
        };
        let mut retained = state
            .active
            .values()
            .chain(state.queued.iter().map(|work| &work.context))
            .chain(state.retained.iter().map(|work| &work.context))
            .map(|context| {
                format!(
                    "session={} reason={} provenance={} state=retained",
                    context
                        .session_id
                        .map(|id| id.to_string())
                        .unwrap_or_else(|| "none".to_string()),
                    context.reason,
                    context.provenance.as_str()
                )
            })
            .collect::<Vec<_>>();
        if state.active_producers > 0 {
            retained.push(format!(
                "producers={} state=retained",
                state.active_producers
            ));
        }
        if state.worker_count > 0 || worker_lock_retained {
            retained.push(format!("workers={} state=retained", state.worker_count));
        }
        let terminal = drained && retained.is_empty();
        if !terminal {
            log::error!(
                "[container-transport] shared shutdown deadline retained ownership producers={} \
                    queued={} retained={} active={} workers={} work=[{}]",
                state.active_producers,
                state.queued.len(),
                state.retained.len(),
                state.active.len(),
                state.worker_count,
                retained.join(", ")
            );
        }
        ContainerShutdownReport { terminal, retained }
    }

    #[cfg(test)]
    pub(super) fn inject_worker_spawn_failure(&self) {
        self.shared
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .fail_worker_spawn = true;
    }

    #[cfg(test)]
    pub(super) fn clear_worker_spawn_failure(&self) {
        self.shared
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .fail_worker_spawn = false;
    }

    #[cfg(test)]
    pub(super) fn worker_count(&self) -> usize {
        self.shared
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .worker_count
    }

    #[cfg(test)]
    pub(super) fn snapshot(&self) -> (bool, usize, usize) {
        let state = self
            .shared
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        (
            state.phase.is_sealed(),
            state.active_producers,
            state.queued.len() + state.retained.len() + state.active.len(),
        )
    }

    #[cfg(test)]
    pub(super) fn observation(&self) -> (bool, usize, usize, usize) {
        let state = self
            .shared
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        (
            state.phase.is_sealed(),
            state.active_producers,
            state.queued.len() + state.retained.len() + state.active.len(),
            state.worker_count,
        )
    }
}

pub(super) static PROCESS_RETAINED_CONTAINER_WORKERS: OnceLock<
    Mutex<Vec<std::thread::JoinHandle<()>>>,
> = OnceLock::new();

impl ContainerShutdownWorkRegistry {
    pub(super) fn join_idle_workers_until(
        &self,
        grace: Instant,
    ) -> Vec<std::thread::JoinHandle<()>> {
        let mut workers = self
            .workers
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        loop {
            let mut unfinished = Vec::with_capacity(workers.len());
            for worker in workers.drain(..) {
                if worker.is_finished() {
                    if worker.join().is_err() {
                        {
                            let mut state = self
                                .shared
                                .state
                                .lock()
                                .unwrap_or_else(|error| error.into_inner());
                            decrement_shutdown_worker_count(&mut state, &self.shared);
                        }
                        log::error!(
                            "[container-transport] shutdown worker panicked during registry drop"
                        );
                    }
                } else {
                    unfinished.push(worker);
                }
            }
            *workers = unfinished;
            let busy = match lock_mutex_until(&self.shared.state, grace) {
                Some(state) => state.active.len().saturating_sub(state.active_fallbacks),
                None => break,
            };
            if workers.len() <= busy {
                break;
            }
            let remaining = grace.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            std::thread::sleep(CONTAINER_SHUTDOWN_POLL.min(remaining));
        }
        std::mem::take(&mut *workers)
    }
}

impl Drop for ContainerShutdownWorkRegistry {
    fn drop(&mut self) {
        self.shared
            .control
            .request_shutdown(Instant::now() + Duration::from_secs(1));
        {
            let mut state = self
                .shared
                .state
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if state.phase == ContainerShutdownPhase::Accepting {
                state.phase = ContainerShutdownPhase::Draining;
            }
            state.terminating = true;
            self.shared.work_available.notify_all();
            self.shared.state_changed.notify_all();
        }
        let retained = PROCESS_RETAINED_CONTAINER_WORKERS.get_or_init(|| Mutex::new(Vec::new()));
        {
            let mut retained = retained.lock().unwrap_or_else(|error| error.into_inner());
            let mut unfinished = Vec::with_capacity(retained.len());
            for worker in retained.drain(..) {
                if worker.is_finished() {
                    if worker.join().is_err() {
                        log::error!(
                            "[container-transport] shutdown worker panicked during retained \
                                registry drop"
                        );
                    }
                } else {
                    unfinished.push(worker);
                }
            }
            *retained = unfinished;
        }
        let grace = Instant::now() + CONTAINER_SHUTDOWN_DROP_GRACE;
        let remaining = self.join_idle_workers_until(grace);
        let retained_count = remaining.len();
        retained
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .extend(remaining);
        log::warn!(
            "[container-transport] registry drop retained {} unfinished shutdown workers",
            retained_count
        );
    }
}

pub(super) fn container_shutdown_worker(shared: Arc<ContainerShutdownWorkShared>) {
    loop {
        let work = {
            let mut state = shared
                .state
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            loop {
                if let Some(work) = state
                    .queued
                    .pop_front()
                    .or_else(|| state.retained.pop_front())
                {
                    state.active.insert(work.id, work.context.clone());
                    shared.state_changed.notify_all();
                    break Some(work);
                }
                if state.terminating {
                    decrement_shutdown_worker_count(&mut state, &shared);
                    break None;
                }
                let effective_deadline = if state.phase == ContainerShutdownPhase::GlobalSweep {
                    state.sweep_deadline
                } else {
                    shared.control.shutdown_deadline()
                };
                match effective_deadline {
                    Some(deadline) if Instant::now() >= deadline => {
                        if state.active_producers == 0 {
                            if state.phase == ContainerShutdownPhase::Accepting {
                                state.phase = ContainerShutdownPhase::Draining;
                            }
                            state.terminating = true;
                            shared.work_available.notify_all();
                            shared.state_changed.notify_all();
                            decrement_shutdown_worker_count(&mut state, &shared);
                            break None;
                        }
                        state = shared
                            .work_available
                            .wait_timeout(state, CONTAINER_SHUTDOWN_IDLE_RECHECK)
                            .unwrap_or_else(|error| error.into_inner())
                            .0;
                    }
                    Some(deadline) => {
                        let remaining = deadline.saturating_duration_since(Instant::now());
                        state = shared
                            .work_available
                            .wait_timeout(state, remaining)
                            .unwrap_or_else(|error| error.into_inner())
                            .0;
                    }
                    None => {
                        state = shared
                            .work_available
                            .wait_timeout(state, CONTAINER_SHUTDOWN_IDLE_RECHECK)
                            .unwrap_or_else(|error| error.into_inner())
                            .0;
                    }
                }
            }
        };
        let Some(work) = work else {
            return;
        };
        let id = work.id;
        run_container_shutdown_work(work.run, &work.control, &work.context);
        let mut state = shared
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        state.active.remove(&id);
        shared.state_changed.notify_all();
    }
}

pub(super) fn run_container_shutdown_work(
    work: Box<dyn FnOnce(&ContainerRuntimeControl) + Send + 'static>,
    control: &ContainerRuntimeControl,
    context: &ContainerShutdownWorkContext,
) {
    if catch_unwind(AssertUnwindSafe(|| work(control))).is_err() {
        log::error!(
            "[container-transport] shutdown work panicked session={} reason={} provenance={}",
            context
                .session_id
                .map(|id| id.to_string())
                .unwrap_or_else(|| "none".to_string()),
            context.reason,
            context.provenance.as_str()
        );
    }
}

impl ContainerShutdownProducer {
    pub(super) fn spawn_owned<F>(&self, session_id: Option<Uuid>, reason: &'static str, work: F)
    where
        F: FnOnce(&ContainerRuntimeControl) + Send + 'static,
    {
        self.registry.spawn_producer_owned(session_id, reason, work);
    }
}

impl Drop for ContainerShutdownProducer {
    fn drop(&mut self) {
        let mut state = self
            .registry
            .shared
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if state.active_producers == 0 {
            log::error!("[container-transport] shutdown-work producer count underflow");
            return;
        }
        state.active_producers -= 1;
        if state.active_producers == 0 {
            self.registry.shared.work_available.notify_all();
            self.registry.shared.state_changed.notify_all();
        }
    }
}
