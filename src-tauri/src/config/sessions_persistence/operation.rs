//! Operation-owned tasks: registration, retained joins and independent result observations.
#![expect(
    dead_code,
    reason = "P01 additive task/probe contracts activate in later phases"
)]
use std::any::Any;
use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Mutex, MutexGuard};
use tokio::sync::{mpsc, oneshot, Notify};
use tokio::task::{JoinError, JoinHandle, JoinSet};
use uuid::Uuid;

pub(crate) type TaskId = Uuid;
type PanicPayload = Box<dyn Any + Send + 'static>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum TaskExitFailure {
    NotLaunched,
    Cancelled,
    Panicked,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TaskExit {
    pub(crate) task: TaskId,
    pub(crate) parent: Option<TaskId>,
    pub(crate) site: &'static str,
    pub(crate) outcome: Result<(), TaskExitFailure>,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum TaskRegistrationError {
    Closed,
    ParentNotLive,
}

#[derive(Clone, Debug)]
pub(crate) struct ProducerExitProof {
    operation: Uuid,
    exits: Vec<TaskExit>,
}

#[derive(Clone, Debug)]
pub(crate) struct TaskSetProof {
    pub(crate) operation: Uuid,
    pub(crate) exits: Vec<TaskExit>,
}

pub(crate) struct ClosedProducerSet {
    tasks: Arc<OwnedTaskSet>,
}

struct TaskRecord {
    parent: Option<TaskId>,
    site: &'static str,
    exit: Option<TaskExit>,
}

#[derive(Default)]
struct TaskState {
    records: HashMap<TaskId, TaskRecord>,
    source: Option<TaskId>,
    closing: bool,
    sealed: bool,
}

type JoinObserver = Box<dyn FnOnce(Result<(), JoinError>) + Send>;
struct InstalledTask {
    id: TaskId,
    handle: JoinHandle<()>,
    observe: JoinObserver,
}

pub(crate) struct OwnedTaskSet {
    operation: Uuid,
    state: Mutex<TaskState>,
    changed: Notify,
    installs: mpsc::UnboundedSender<InstalledTask>,
    #[cfg(test)]
    probe: Option<Arc<OperationProbe>>,
}

fn lock<T>(value: &Mutex<T>) -> MutexGuard<'_, T> {
    value.lock().unwrap_or_else(|error| error.into_inner())
}

impl OwnedTaskSet {
    pub(crate) fn new(operation: Uuid) -> Arc<Self> {
        Self::create(
            operation,
            #[cfg(test)]
            None,
        )
    }

    #[cfg(test)]
    pub(crate) fn new_with_probe(operation: Uuid, probe: Arc<OperationProbe>) -> Arc<Self> {
        Self::create(operation, Some(probe))
    }

    fn create(operation: Uuid, #[cfg(test)] probe: Option<Arc<OperationProbe>>) -> Arc<Self> {
        let (installs, receiver) = mpsc::unbounded_channel();
        let tasks = Arc::new(Self {
            operation,
            state: Mutex::new(TaskState::default()),
            changed: Notify::new(),
            installs,
            #[cfg(test)]
            probe,
        });
        // Weak ownership avoids an idle supervisor keeping the task set alive forever.
        tokio::spawn(Self::supervise(Arc::downgrade(&tasks), receiver));
        tasks
    }

    pub(crate) fn reserve(
        self: &Arc<Self>,
        parent: Option<TaskId>,
        site: &'static str,
    ) -> Result<ReservedTask, TaskRegistrationError> {
        let mut state = lock(&self.state);
        if state.sealed || (state.closing && parent.is_none()) {
            return Err(TaskRegistrationError::Closed);
        }
        if let Some(parent) = parent {
            if !state
                .records
                .get(&parent)
                .is_some_and(|record| record.exit.is_none())
            {
                return Err(TaskRegistrationError::ParentNotLive);
            }
        }
        let id = Uuid::new_v4();
        state.records.insert(
            id,
            TaskRecord {
                parent,
                site,
                exit: None,
            },
        );
        Ok(ReservedTask {
            tasks: self.clone(),
            id,
            launched: false,
        })
    }

    async fn supervise(
        tasks: std::sync::Weak<Self>,
        mut receiver: mpsc::UnboundedReceiver<InstalledTask>,
    ) {
        let mut joins = JoinSet::new();
        let mut accepting = true;
        loop {
            tokio::select! {
                installed = receiver.recv(), if accepting => match installed {
                    Some(installed) => {
                        joins.spawn(async move {
                            let result = installed.handle.await;
                            (installed.id, result, installed.observe)
                        });
                    }
                    None => accepting = false,
                },
                joined = joins.join_next(), if !joins.is_empty() => {
                    if let Some(Ok((id, result, observe))) = joined {
                        let outcome = match &result {
                            Ok(()) => Ok(()),
                            Err(error) if error.is_cancelled() => Err(TaskExitFailure::Cancelled),
                            Err(_) => Err(TaskExitFailure::Panicked),
                        };
                        if let Some(tasks) = tasks.upgrade() {
                            tasks.record_exit(id, outcome);
                        }
                        observe(result);
                    }
                }
                else => return,
            }
        }
    }

    fn record_exit(self: &Arc<Self>, id: TaskId, outcome: Result<(), TaskExitFailure>) {
        #[cfg(test)]
        let actually_joined = outcome != Err(TaskExitFailure::NotLaunched);
        {
            let mut state = lock(&self.state);
            let record = state.records.get_mut(&id).expect("registered task");
            record.exit = Some(TaskExit {
                task: id,
                parent: record.parent,
                site: record.site,
                outcome,
            });
            if state.closing && state.records.values().all(|record| record.exit.is_some()) {
                state.sealed = true;
            }
        }
        #[cfg(test)]
        if let Some(scope) = self.scope(id).filter(|_| actually_joined) {
            scope.record(ProbeEvent::ProducerJoined);
        }
        self.changed.notify_waiters();
    }

    fn scope(self: &Arc<Self>, id: TaskId) -> Option<OperationScope> {
        let state = lock(&self.state);
        let _record = state.records.get(&id)?;
        Some(OperationScope {
            tasks: self.clone(),
            task: Some(id),
            #[cfg(test)]
            context: ProbeContext {
                operation: self.operation,
                task: Some(id),
                parent: _record.parent,
                attempt: self.operation,
                site: _record.site,
            },
        })
    }

    pub(crate) fn close_producers(self: &Arc<Self>) -> ClosedProducerSet {
        let mut state = lock(&self.state);
        assert!(
            state.source.is_none_or(|id| {
                state
                    .records
                    .get(&id)
                    .is_some_and(|record| record.exit.is_some())
            }),
            "source must actually join before producer close"
        );
        state.closing = true;
        state.sealed = state.records.values().all(|record| record.exit.is_some());
        drop(state);
        self.changed.notify_waiters();
        ClosedProducerSet {
            tasks: self.clone(),
        }
    }

    pub(crate) async fn join_producers(
        self: &Arc<Self>,
    ) -> Result<ProducerExitProof, TaskExitFailure> {
        loop {
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            let source_joined = {
                let state = lock(&self.state);
                state.source.is_none_or(|id| {
                    state
                        .records
                        .get(&id)
                        .is_some_and(|record| record.exit.is_some())
                })
            };
            if source_joined {
                let closed = self.close_producers();
                let proof = Self::join_closed(&closed).await;
                if let Some(error) = proof
                    .exits
                    .iter()
                    .find_map(|exit| exit.outcome.clone().err())
                {
                    return Err(error);
                }
                return Ok(ProducerExitProof {
                    operation: proof.operation,
                    exits: proof.exits,
                });
            }
            changed.await;
        }
    }

    pub(crate) async fn join_closed(closed: &ClosedProducerSet) -> TaskSetProof {
        loop {
            let changed = closed.tasks.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            {
                let state = lock(&closed.tasks.state);
                if state.sealed {
                    return TaskSetProof {
                        operation: closed.tasks.operation,
                        exits: state
                            .records
                            .values()
                            .filter_map(|record| record.exit.clone())
                            .collect(),
                    };
                }
            }
            changed.await;
        }
    }
}

struct ResultCell<T> {
    value: Mutex<Option<T>>,
    failure: Mutex<Option<TaskExitFailure>>,
    panic: Mutex<Option<PanicPayload>>,
    changed: Notify,
}

impl<T> ResultCell<T> {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            value: Mutex::new(None),
            failure: Mutex::new(None),
            panic: Mutex::new(None),
            changed: Notify::new(),
        })
    }

    fn publish(&self, value: T) {
        *lock(&self.value) = Some(value);
        self.changed.notify_waiters();
    }

    fn observe_join(&self, result: Result<(), JoinError>) {
        if let Err(error) = result {
            let failure = if error.is_cancelled() {
                TaskExitFailure::Cancelled
            } else {
                *lock(&self.panic) = Some(error.into_panic());
                TaskExitFailure::Panicked
            };
            *lock(&self.failure) = Some(failure);
        }
        self.changed.notify_waiters();
    }
}

pub(crate) struct OwnedTaskTicket<T> {
    id: TaskId,
    cell: Arc<ResultCell<Arc<T>>>,
    tasks: Arc<OwnedTaskSet>,
}

impl<T: Send + Sync + 'static> OwnedTaskTicket<T> {
    pub(crate) fn task_id(&self) -> TaskId {
        self.id
    }

    pub(crate) async fn result(&self) -> Result<Arc<T>, TaskExitFailure> {
        loop {
            let changed = self.cell.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if let Some(value) = lock(&self.cell.value).clone() {
                return Ok(value);
            }
            if let Some(error) = lock(&self.cell.failure).clone() {
                return Err(error);
            }
            if let Some(error) = lock(&self.tasks.state)
                .records
                .get(&self.id)
                .and_then(|record| record.exit.as_ref())
                .and_then(|exit| exit.outcome.clone().err())
            {
                return Err(error);
            }
            changed.await;
        }
    }
}

pub(crate) struct ReservedTask {
    tasks: Arc<OwnedTaskSet>,
    id: TaskId,
    launched: bool,
}

impl Drop for ReservedTask {
    fn drop(&mut self) {
        if !self.launched {
            self.tasks
                .record_exit(self.id, Err(TaskExitFailure::NotLaunched));
        }
    }
}

impl ReservedTask {
    fn install<T: Send + 'static>(
        &mut self,
        handle: JoinHandle<()>,
        cell: Arc<ResultCell<T>>,
    ) -> bool {
        let failure_cell = cell.clone();
        let installed = InstalledTask {
            id: self.id,
            handle,
            observe: Box::new(move |result| cell.observe_join(result)),
        };
        self.launched = self.tasks.installs.send(installed).is_ok();
        if !self.launched {
            *lock(&failure_cell.failure) = Some(TaskExitFailure::NotLaunched);
            failure_cell.changed.notify_waiters();
        }
        self.launched
    }

    fn start(&self, permit: oneshot::Sender<()>, scope: OperationScope) {
        #[cfg(test)]
        {
            scope.record(ProbeEvent::ChildRegisteredBeforeLaunch);
            tokio::spawn(async move {
                scope.pause(ProbeEvent::ChildRegisteredBeforeLaunch).await;
                let _ = permit.send(());
            });
        }
        #[cfg(not(test))]
        {
            let _ = scope;
            let _ = permit.send(());
        }
    }

    pub(crate) fn launch<T, F>(mut self, future: F) -> OwnedTaskTicket<T>
    where
        T: Send + Sync + 'static,
        F: Future<Output = T> + Send + 'static,
    {
        let cell = ResultCell::new();
        let output = cell.clone();
        let scope = self.tasks.scope(self.id).expect("registered scope");
        let inside = scope.clone();
        let (permit, start) = oneshot::channel();
        let handle = tokio::spawn(SCOPE.scope(scope.clone(), async move {
            if start.await.is_err() {
                return;
            }
            output.publish(Arc::new(future.await));
            #[cfg(test)]
            inside.point(ProbeEvent::ProducerResultBeforeExit).await;
            #[cfg(not(test))]
            let _ = inside;
        }));
        if self.install(handle, cell.clone()) {
            self.start(permit, scope);
        }
        OwnedTaskTicket {
            id: self.id,
            cell,
            tasks: self.tasks.clone(),
        }
    }

    pub(crate) fn launch_blocking<T, F>(mut self, work: F) -> OwnedTaskTicket<T>
    where
        T: Send + Sync + 'static,
        F: FnOnce() -> T + Send + 'static,
    {
        let cell = ResultCell::new();
        let output = cell.clone();
        let scope = self.tasks.scope(self.id).expect("registered scope");
        let inside = scope.clone();
        let (permit, start) = oneshot::channel();
        let handle = tokio::task::spawn_blocking(move || {
            if start.blocking_recv().is_err() {
                return;
            }
            SCOPE.sync_scope(inside.clone(), || {
                output.publish(Arc::new(work()));
                #[cfg(test)]
                inside.point_blocking(ProbeEvent::ProducerResultBeforeExit);
            });
        });
        if self.install(handle, cell.clone()) {
            self.start(permit, scope);
        }
        OwnedTaskTicket {
            id: self.id,
            cell,
            tasks: self.tasks.clone(),
        }
    }

    async fn run_single<T, F>(mut self, future: F) -> T
    where
        T: Send + 'static,
        F: Future<Output = T> + Send + 'static,
    {
        let cell = ResultCell::new();
        let output = cell.clone();
        let scope = self.tasks.scope(self.id).expect("registered scope");
        let inside = scope.clone();
        let (permit, start) = oneshot::channel();
        let handle = tokio::spawn(SCOPE.scope(scope.clone(), async move {
            if start.await.is_err() {
                return;
            }
            output.publish(future.await);
            #[cfg(test)]
            inside.point(ProbeEvent::ProducerResultBeforeExit).await;
            #[cfg(not(test))]
            let _ = inside;
        }));
        if self.install(handle, cell.clone()) {
            self.start(permit, scope);
        }
        match Self::single_result(&cell).await {
            Ok(value) => value,
            Err(failure) => {
                let payload = lock(&cell.panic).take();
                if let Some(payload) = payload {
                    std::panic::resume_unwind(payload);
                }
                panic!("owned persistence task failed: {failure:?}");
            }
        }
    }

    pub(crate) async fn run_blocking_single<T, F>(mut self, work: F) -> Result<T, TaskExitFailure>
    where
        T: Send + 'static,
        F: FnOnce() -> T + Send + 'static,
    {
        let cell = ResultCell::new();
        let output = cell.clone();
        let scope = self.tasks.scope(self.id).expect("registered scope");
        let inside = scope.clone();
        let (permit, start) = oneshot::channel();
        let handle = tokio::task::spawn_blocking(move || {
            if start.blocking_recv().is_err() {
                return;
            }
            SCOPE.sync_scope(inside.clone(), || {
                output.publish(work());
                #[cfg(test)]
                inside.point_blocking(ProbeEvent::ProducerResultBeforeExit);
            });
        });
        if self.install(handle, cell.clone()) {
            self.start(permit, scope);
        }
        Self::single_result(&cell).await
    }

    async fn single_result<T>(cell: &ResultCell<T>) -> Result<T, TaskExitFailure> {
        loop {
            let changed = cell.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if let Some(value) = lock(&cell.value).take() {
                return Ok(value);
            }
            if let Some(failure) = lock(&cell.failure).clone() {
                return Err(failure);
            }
            changed.await;
        }
    }
}

tokio::task_local! { static SCOPE: OperationScope; }

#[derive(Clone)]
pub(crate) struct OperationScope {
    tasks: Arc<OwnedTaskSet>,
    task: Option<TaskId>,
    #[cfg(test)]
    context: ProbeContext,
}

impl OperationScope {
    pub(crate) fn current() -> Option<Self> {
        SCOPE.try_with(Clone::clone).ok()
    }

    pub(crate) async fn run<F: Future>(tasks: Arc<OwnedTaskSet>, future: F) -> F::Output {
        let scope = Self {
            #[cfg(test)]
            context: ProbeContext {
                operation: tasks.operation,
                task: None,
                parent: None,
                attempt: tasks.operation,
                site: "operation",
            },
            tasks,
            task: None,
        };
        SCOPE.scope(scope, future).await
    }

    pub(crate) fn reserve(
        &self,
        site: &'static str,
    ) -> Result<ReservedTask, TaskRegistrationError> {
        self.tasks.reserve(self.task, site)
    }

    pub(crate) async fn run_owned<T, F>(&self, future: F) -> T
    where
        T: Send + 'static,
        F: Future<Output = T> + Send + 'static,
    {
        self.reserve("run_owned_persist_core")
            .expect("live operation producer")
            .run_single(future)
            .await
    }

    #[cfg(test)]
    pub(crate) fn record(&self, event: ProbeEvent) {
        if let Some(probe) = &self.tasks.probe {
            probe.record(self.context.clone(), event);
        }
    }

    #[cfg(test)]
    pub(crate) async fn pause(&self, event: ProbeEvent) {
        if let Some(probe) = &self.tasks.probe {
            probe.pause(&self.context, event).await;
        }
    }

    #[cfg(test)]
    pub(crate) async fn point(&self, event: ProbeEvent) {
        self.record(event);
        self.pause(event).await;
    }

    #[cfg(test)]
    pub(crate) fn point_blocking(&self, event: ProbeEvent) {
        if let Some(probe) = &self.tasks.probe {
            probe.record(self.context.clone(), event);
            probe.pause_blocking(&self.context, event);
        }
    }
}

pub(crate) fn run_observed_owned_core<T, F>(
    tasks: Arc<OwnedTaskSet>,
    future: F,
) -> OwnedTaskTicket<T>
where
    T: Send + Sync + 'static,
    F: Future<Output = T> + Send + 'static,
{
    let reserved = tasks.reserve(None, "owned_source").expect("open operation");
    {
        let mut state = lock(&tasks.state);
        assert!(state.source.is_none(), "one owned source per operation");
        state.source = Some(reserved.id);
    }
    reserved.launch(future)
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum ProbeEvent {
    ChildRegisteredBeforeLaunch,
    ProducerResultBeforeExit,
    ProducerJoined,
    BlockingPrepareEntered,
    BlockingPrepareExited,
    ManagerReceiptFinalized,
    SettlementClaimed,
    BeforeAbort,
    AfterAbort,
    BeforeFinish,
    AfterFinish,
    NativeInstalled,
    NativeExitJoined,
    ReaderInstalled,
    ReaderExitJoined,
    RetirementExtracted,
    RetirementJoined,
    SourceDispositionStage,
    WorkerExited,
}

#[cfg(test)]
#[derive(Clone, Debug)]
pub(crate) struct ProbeContext {
    pub(crate) operation: Uuid,
    pub(crate) task: Option<TaskId>,
    pub(crate) parent: Option<TaskId>,
    pub(crate) attempt: Uuid,
    pub(crate) site: &'static str,
}

#[cfg(test)]
#[derive(Default)]
pub(crate) struct ProbeGate {
    released: Mutex<bool>,
    async_changed: Notify,
    blocking_changed: std::sync::Condvar,
}

#[cfg(test)]
impl ProbeGate {
    pub(crate) fn release(&self) {
        *lock(&self.released) = true;
        self.async_changed.notify_waiters();
        self.blocking_changed.notify_all();
    }

    async fn wait(&self) {
        loop {
            let changed = self.async_changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if *lock(&self.released) {
                return;
            }
            changed.await;
        }
    }

    fn wait_blocking(&self) {
        let mut released = lock(&self.released);
        while !*released {
            released = self
                .blocking_changed
                .wait(released)
                .unwrap_or_else(|error| error.into_inner());
        }
    }
}

#[cfg(test)]
#[derive(Default)]
pub(crate) struct OperationProbe {
    events: Mutex<Vec<(ProbeContext, ProbeEvent)>>,
    gates: Mutex<HashMap<(&'static str, ProbeEvent), Arc<ProbeGate>>>,
    changed: Notify,
}

#[cfg(test)]
impl OperationProbe {
    pub(crate) fn gate(&self, site: &'static str, event: ProbeEvent) -> Arc<ProbeGate> {
        let gate = Arc::new(ProbeGate::default());
        lock(&self.gates).insert((site, event), gate.clone());
        gate
    }

    pub(crate) fn record(&self, context: ProbeContext, event: ProbeEvent) {
        lock(&self.events).push((context, event));
        self.changed.notify_waiters();
    }

    pub(crate) async fn observed(&self, site: &'static str, event: ProbeEvent) -> ProbeContext {
        loop {
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if let Some((context, _)) = lock(&self.events)
                .iter()
                .find(|(context, found)| context.site == site && *found == event)
            {
                return context.clone();
            }
            changed.await;
        }
    }

    pub(crate) async fn pause(&self, context: &ProbeContext, event: ProbeEvent) {
        let gate = lock(&self.gates).get(&(context.site, event)).cloned();
        if let Some(gate) = gate {
            gate.wait().await;
        }
    }

    pub(crate) fn pause_blocking(&self, context: &ProbeContext, event: ProbeEvent) {
        let gate = lock(&self.gates).get(&(context.site, event)).cloned();
        if let Some(gate) = gate {
            gate.wait_blocking();
        }
    }
}

#[cfg(test)]
mod tests;
