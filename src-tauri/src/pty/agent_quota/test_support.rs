use super::{
    AgentQuotaEngine, AgentQuotaPayload, AgentQuotaSource, ContextSessionLiveness, QuotaEventSink,
    QuotaRowsSource, QuotaSourceProvider, ScreenRowsRead, SourceSpec,
};
use futures::future::BoxFuture;
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use uuid::Uuid;

type ReadHook = Box<dyn Fn(Uuid) + Send + Sync>;

type LivenessHook = Arc<dyn Fn(Uuid) + Send + Sync>;

/// Rows are THREE-state and scripted, with an independent liveness script: a
/// two-state fake cannot express "a live child we could not ask".
#[derive(Default)]
pub(crate) struct RowsFake {
    /// Per session, consumed front first. Empty = `Unavailable`.
    reads: Mutex<HashMap<Uuid, Vec<ScreenRowsRead>>>,
    /// Per session, sticky. Unset = `Unavailable`.
    liveness: Mutex<HashMap<Uuid, ContextSessionLiveness>>,
    rows_calls: Mutex<Vec<Uuid>>,
    liveness_calls: Mutex<Vec<Uuid>>,
    on_rows_read: Mutex<Option<ReadHook>>,
    pub(super) on_liveness: Mutex<Option<LivenessHook>>,
}

impl RowsFake {
    pub(crate) fn push(&self, id: Uuid, read: ScreenRowsRead) {
        self.reads.lock().unwrap().entry(id).or_default().push(read);
    }

    pub(crate) fn set_liveness(&self, id: Uuid, liveness: ContextSessionLiveness) {
        self.liveness.lock().unwrap().insert(id, liveness);
    }

    /// Every `get_screen_rows` call, in order.
    pub(crate) fn rows_calls(&self) -> Vec<Uuid> {
        self.rows_calls.lock().unwrap().clone()
    }

    /// Every `get_session_liveness` call, in order.
    pub(crate) fn liveness_calls(&self) -> Vec<Uuid> {
        self.liveness_calls.lock().unwrap().clone()
    }

    /// Runs inside `get_screen_rows`, after the read is taken: the mid-tick window.
    pub(crate) fn on_rows_read(&self, hook: impl Fn(Uuid) + Send + Sync + 'static) {
        *self.on_rows_read.lock().unwrap() = Some(Box::new(hook));
    }
}

impl QuotaRowsSource for RowsFake {
    fn get_screen_rows(&self, id: Uuid) -> ScreenRowsRead {
        self.rows_calls.lock().unwrap().push(id);
        let read = match self.reads.lock().unwrap().get_mut(&id) {
            Some(script) if !script.is_empty() => script.remove(0),
            _ => ScreenRowsRead::Unavailable,
        };
        if let Some(hook) = self.on_rows_read.lock().unwrap().as_ref() {
            hook(id);
        }
        read
    }

    fn get_session_liveness(&self, id: Uuid) -> ContextSessionLiveness {
        self.liveness_calls.lock().unwrap().push(id);
        let hook = self.on_liveness.lock().unwrap().clone();
        if let Some(hook) = hook {
            hook(id);
        }
        self.liveness
            .lock()
            .unwrap()
            .get(&id)
            .copied()
            .unwrap_or(ContextSessionLiveness::Unavailable)
    }
}

#[derive(Default)]
pub(crate) struct SourcesFake {
    specs: Mutex<HashMap<String, AgentQuotaSource>>,
    calls: AtomicUsize,
    hang: std::sync::atomic::AtomicBool,
    pub(super) unavailable: std::sync::atomic::AtomicBool,
}

impl SourcesFake {
    /// Account key == agent id: one agent, one account.
    pub(crate) fn configure(&self, agent_id: &str, spec: SourceSpec) {
        self.configure_on(agent_id, agent_id, spec);
    }

    /// #2566 - an agent on a named account key, for the shared-command cases.
    pub(crate) fn configure_on(&self, agent_id: &str, account_key: &str, spec: SourceSpec) {
        self.specs.lock().unwrap().insert(
            agent_id.to_string(),
            AgentQuotaSource {
                account_key: account_key.to_string(),
                spec,
            },
        );
    }

    /// What the adapter does for a disabled or removed entry: the engine sees none.
    pub(crate) fn disable(&self, agent_id: &str) {
        self.specs.lock().unwrap().remove(agent_id);
    }

    pub(crate) fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    /// From now on `sources()` counts the call and never resolves: a tick stuck
    /// in flight.
    pub(crate) fn hang(&self) {
        self.hang.store(true, Ordering::SeqCst);
    }
}

impl QuotaSourceProvider for SourcesFake {
    fn sources(&self) -> BoxFuture<'_, Option<HashMap<String, AgentQuotaSource>>> {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if self.hang.load(Ordering::SeqCst) {
                std::future::pending::<()>().await;
            }
            if self.unavailable.load(Ordering::Relaxed) {
                None
            } else {
                Some(self.specs.lock().unwrap().clone())
            }
        })
    }
}

#[derive(Default)]
pub(crate) struct SinkFake {
    emitted: Mutex<Vec<AgentQuotaPayload>>,
}

impl SinkFake {
    pub(crate) fn emitted(&self) -> Vec<AgentQuotaPayload> {
        self.emitted.lock().unwrap().clone()
    }
}

impl QuotaEventSink for SinkFake {
    fn emit(&self, payload: AgentQuotaPayload) {
        self.emitted.lock().unwrap().push(payload);
    }
}

/// Everything a test needs, built and wired, mirroring the `Harness` in
/// `watchers/mod.rs`'s tests. The fakes stay reachable as FIELDS: a builder
/// that returns only the engine cannot assert "rows were never read".
pub(crate) struct QuotaHarness {
    pub(crate) engine: Arc<AgentQuotaEngine>,
    pub(crate) rows: Arc<RowsFake>,
    pub(crate) sources: Arc<SourcesFake>,
    pub(crate) sink: Arc<SinkFake>,
}

impl QuotaHarness {
    pub(crate) fn new() -> Self {
        let rows = Arc::new(RowsFake::default());
        let sources = Arc::new(SourcesFake::default());
        let sink = Arc::new(SinkFake::default());
        let engine = AgentQuotaEngine::new(
            Arc::clone(&rows) as Arc<dyn QuotaRowsSource>,
            Arc::clone(&sources) as Arc<dyn QuotaSourceProvider>,
            Arc::clone(&sink) as Arc<dyn QuotaEventSink>,
        );
        Self {
            engine,
            rows,
            sources,
            sink,
        }
    }
}
