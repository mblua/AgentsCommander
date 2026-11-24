use crate::api::message_store::PtyInputTargetOwnership;
use crate::app_state::RestoreInProgress;
use crate::config::agent_command::AgentSpawnCommand;
use crate::config::coordinator_clocks::CoordinatorClocksState;
use crate::config::sessions_persistence::CreationGateEnforcement;
use crate::config::settings::SettingsState;
use crate::phone::events::{MailboxDeliveryEvents, MailboxRequestEvents, PtyInputEvents};
use crate::phone::mailbox::browser::snapshot::WebBrowserSnapshot;
use crate::phone::mailbox::pty_observation::WakeDeliveryState;
use crate::phone::mailbox::response_observation::{
    ResponseAdmissionState, ResponseCompletionState,
};
use crate::pty::backend::{PtyViewport, ResolvedAgentHostShell};
use crate::pty::inject::host::InjectionHost;
use crate::pty::inject::BoundaryMetadataOutcome;
use crate::pty::manager::PtyManager;
use crate::pty::menu_guard::MenuGuardEvents;
use crate::session::create_contracts::{
    CreateSelectionIntent, OwnedReplacementCreateRequest, OwnedReplacementCreateStep,
    PtyTargetCreateStep,
};
use crate::session::maintenance::PendingSelfClear;
#[cfg(test)]
use crate::session::manager::PendingCreateBinding;
use crate::session::manager::SessionManager;
use crate::session::purge_guard::PurgeGuard;
use crate::session::selection::{
    OwnedReplacementContinuation, SelectionCoordinator, TrustedRestartIntent,
};
use crate::session::session::{SessionCommunication, SessionInfo, SessionRepo};
use crate::session::warnings::SessionWarningEvents;
use crate::shutdown::ShutdownSignal;
use crate::{AppOutbox, MasterToken};
use futures::future::BoxFuture;
use std::future::Future;
use std::sync::{Arc, Mutex};
use tokio::sync::RwLock;
use uuid::Uuid;

/// Settlement of the native coordinator create ticket used by mailbox tests.
#[cfg(test)]
pub(crate) trait CoordinatorTicket {
    fn succeeded(self);
    fn failed(self);
}

pub(crate) struct MailboxSessionCreate<'a> {
    pub(crate) session_mgr: &'a Arc<RwLock<SessionManager>>,
    pub(crate) pty_mgr: &'a Arc<Mutex<PtyManager>>,
    pub(crate) shell: String,
    pub(crate) shell_args: Vec<String>,
    pub(crate) cwd: String,
    pub(crate) session_name: Option<String>,
    pub(crate) agent_id: Option<String>,
    pub(crate) agent_label: Option<String>,
    pub(crate) skip_tooling_save: bool,
    pub(crate) git_repos: Vec<SessionRepo>,
    pub(crate) skip_auto_resume: bool,
    pub(crate) resolved_spawn: Option<AgentSpawnCommand>,
    pub(crate) resolved_agent_host_shell: Option<ResolvedAgentHostShell>,
    pub(crate) viewport: Option<PtyViewport>,
    pub(crate) selection_intent: CreateSelectionIntent,
}

#[cfg(test)]
#[derive(Clone)]
pub(crate) struct WakeCreateProbe {
    pub(crate) budget: std::time::Duration,
    pub(crate) observe:
        Arc<dyn Fn(&'static str, Option<Uuid>) -> BoxFuture<'static, bool> + Send + Sync>,
    pub(crate) prepared: Arc<Mutex<Option<PendingCreateBinding>>>,
    hold_input: Arc<std::sync::atomic::AtomicBool>,
}

#[cfg(test)]
type WakeCreateEvent = (
    &'static str,
    Option<Uuid>,
    tokio::sync::oneshot::Sender<bool>,
);

#[cfg(test)]
pub(crate) struct WakeCreateEvents(tokio::sync::mpsc::UnboundedReceiver<WakeCreateEvent>);

#[cfg(test)]
impl WakeCreateEvents {
    pub(super) async fn recv(&mut self) -> Option<WakeCreateEvent> {
        self.0.recv().await
    }

    pub(super) async fn at(
        &mut self,
        stage: &str,
        wait: std::time::Duration,
    ) -> (Option<Uuid>, tokio::sync::oneshot::Sender<bool>) {
        let (actual, binding, resume) = tokio::time::timeout(wait, self.recv())
            .await
            .expect("wake fixture deadline")
            .unwrap();
        assert_eq!(actual, stage);
        (binding, resume)
    }
}

#[cfg(test)]
struct WakeDeadlineWarnings(Mutex<Vec<String>>);

#[cfg(test)]
static WAKE_DEADLINE_WARNINGS: WakeDeadlineWarnings = WakeDeadlineWarnings(Mutex::new(Vec::new()));

#[cfg(test)]
impl log::Log for WakeDeadlineWarnings {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        metadata.level() == log::Level::Warn
    }

    fn log(&self, record: &log::Record<'_>) {
        if self.enabled(record.metadata()) {
            let message = record.args().to_string();
            if message.contains("#3263 wake ended without input")
                && message.contains("stage=deadline")
            {
                self.0.lock().unwrap().push(message);
            }
        }
    }

    fn flush(&self) {}
}

#[cfg(test)]
impl WakeCreateProbe {
    pub(crate) fn pause_at(
        budget: std::time::Duration,
        cuts: &[(&'static str, usize)],
    ) -> (Self, WakeCreateEvents) {
        log::set_logger(&WAKE_DEADLINE_WARNINGS).unwrap();
        log::set_max_level(log::LevelFilter::Warn);
        let (events, receiver) = tokio::sync::mpsc::unbounded_channel();
        let hold_input = cuts.iter().any(|&(stage, _)| stage == "input");
        let cuts = Mutex::new(std::collections::VecDeque::from(cuts.to_vec()));
        let observe = Arc::new(move |stage, binding| -> BoxFuture<'static, bool> {
            let events = events.clone();
            let mut cuts = cuts.lock().unwrap();
            let pause = cuts
                .front_mut()
                .filter(|cut| cut.0 == stage)
                .is_some_and(|cut| {
                    cut.1 -= 1;
                    cut.1 == 0
                });
            if pause {
                cuts.pop_front();
            }
            Box::pin(async move {
                if !pause {
                    return false;
                }
                let (resume, wait) = tokio::sync::oneshot::channel();
                events.send((stage, binding, resume)).unwrap();
                wait.await.unwrap_or(false)
            })
        });
        let prepared = Arc::new(Mutex::new(None));
        (
            Self {
                budget,
                observe,
                prepared,
                hold_input: Arc::new(std::sync::atomic::AtomicBool::new(hold_input)),
            },
            WakeCreateEvents(receiver),
        )
    }

    pub(super) async fn observe_input_dispatch<F: Future>(
        app: &impl MailboxHost,
        session_id: Uuid,
        injection: F,
    ) -> F::Output {
        use std::sync::atomic::Ordering;

        let Some(probe) = app
            .wake_create_probe()
            .filter(|probe| probe.hold_input.swap(false, Ordering::Relaxed))
        else {
            return injection.await;
        };
        // Poll the canonical injector with its actual per-session writer occupied.
        let writer = PtyManager::acquire_input_writer(app.pty_routes(), session_id)
            .await
            .unwrap();
        let mut injection = Box::pin(injection);
        assert!(
            futures::poll!(injection.as_mut()).is_pending(),
            "peer injection must wait for the occupied PTY writer"
        );
        (probe.observe)("input", Some(session_id)).await;
        drop(writer);
        injection.await
    }

    pub(super) async fn assert_input_pending(app: &impl MailboxHost, id: Uuid) {
        let mut writer = std::pin::pin!(PtyManager::acquire_input_writer(app.pty_routes(), id));
        assert!(
            futures::poll!(writer.as_mut()).is_pending(),
            "the real PTY writer must be occupied at the input cut"
        );
    }

    pub(super) fn delay_backend_return(&self, app: &impl MailboxHost) {
        use crate::pty::backend::SessionBackendKind;

        let mut pty = app.pty_routes().lock().unwrap();
        let backend = pty.backend_for_kind(SessionBackendKind::LocalProcess);
        *pty = PtyManager::new_for_test(Arc::new(DelayedWakeBackend {
            backend,
            probe: self.clone(),
        }));
    }

    pub(super) fn assert_native_pending(
        app: &impl MailboxHost,
        owner: &Mutex<Option<OwnedReplacementContinuation>>,
        id: Uuid,
    ) {
        use crate::pty::backend::SessionBackendKind;

        let owner = owner.lock().unwrap();
        let owner = owner.as_ref().unwrap();
        let state = owner.fixture_observe_publication();
        assert_eq!(state.binding.session_id(), id);
        assert!(owner.fixture_wake_source_observation().active);
        assert!(owner.fixture_wake_rollback().backend_submitted);
        assert!(!state.terminal && !state.publication);
        assert_eq!(app.selection_opt().unwrap().fixture_root_count(), Some(1));
        let pty = app.pty_routes().lock().unwrap();
        assert!(
            pty.backend_for_kind(SessionBackendKind::LocalProcess)
                .has_session(id),
            "native spawn must already have started"
        );
        assert!(pty.backend_kind(id).is_none(), "spawn result still pending");
    }

    pub(crate) async fn assert_bound_conflict(
        &self,
        app: &impl MailboxHost,
        pending_before: &std::collections::HashSet<Uuid>,
    ) {
        let binding = self.prepared.lock().unwrap().unwrap();
        let manager = app.pty_sessions().read().await.clone();
        assert!(pending_before.contains(&binding.session_id()));
        assert!(manager.get_pending_session(binding).await.is_some());
        let capacity = app.selection_opt().unwrap().port_test_state();
        assert_eq!(capacity.queue_capacity, capacity.expected_queue_capacity);
        assert_eq!(
            capacity.admission_available,
            capacity.expected_admission_capacity
        );
    }

    pub(crate) fn assert_deadline_warning(reason: &str, diagnostic: &str) {
        assert!(reason.contains(diagnostic), "{reason}");
        let warnings = std::mem::take(&mut *WAKE_DEADLINE_WARNINGS.0.lock().unwrap());
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].ends_with(reason), "{warnings:?}");
    }
}

#[cfg(test)]
struct DelayedWakeBackend {
    backend: Arc<dyn crate::pty::backend::PtyBackend>,
    probe: WakeCreateProbe,
}

#[cfg(test)]
impl crate::pty::backend::PtyBackend for DelayedWakeBackend {
    fn as_any(&self) -> &dyn std::any::Any {
        // Existing mailbox fixture helpers seed and inspect the wrapped fake.
        self.backend.as_any()
    }

    fn spawn(
        &self,
        spec: crate::pty::backend::BackendSpawnSpec,
    ) -> BoxFuture<'_, Result<(), crate::errors::AppError>> {
        Box::pin(async move {
            let id = spec.id;
            let result = self.backend.spawn(spec).await;
            (self.probe.observe)("late_native", Some(id)).await;
            result
        })
    }

    fn write(
        &self,
        authority: &crate::pty::manager::BackendWriteAuthority,
        id: Uuid,
        data: &[u8],
    ) -> Result<(), crate::errors::AppError> {
        self.backend.write(authority, id, data)
    }

    fn resize(&self, id: Uuid, cols: u16, rows: u16) -> Result<(), crate::errors::AppError> {
        self.backend.resize(id, cols, rows)
    }

    fn kill(&self, id: Uuid) -> Result<(), crate::errors::AppError> {
        self.backend.kill(id)
    }

    fn has_session(&self, id: Uuid) -> bool {
        self.backend.has_session(id)
    }

    fn get_screen_snapshot(&self, id: Uuid) -> Option<crate::pty::output::PtyScreenSnapshot> {
        self.backend.get_screen_snapshot(id)
    }

    fn get_pty_size(&self, id: Uuid) -> Option<(u16, u16)> {
        self.backend.get_pty_size(id)
    }

    fn get_screen_rows(&self, id: Uuid) -> crate::pty::context_scrape::ScreenRowsRead {
        self.backend.get_screen_rows(id)
    }

    fn register_response_watcher(&self, id: Uuid, request: String, dir: std::path::PathBuf) {
        self.backend.register_response_watcher(id, request, dir);
    }

    fn terminate_job_for_session(&self, id: Uuid) -> bool {
        self.backend.terminate_job_for_session(id)
    }

    fn kill_all_jobs(&self) -> (usize, usize) {
        self.backend.kill_all_jobs()
    }
}

/// Native capabilities available to captured mailbox work.
pub(crate) trait MailboxHost:
    WakeDeliveryState
    + ResponseAdmissionState
    + ResponseCompletionState
    + crate::phone::terminal_snapshot::TerminalSnapshotHost
    + crate::phone::terminal_snapshot::task::SnapshotTaskHost
    + InjectionHost
    + PtyInputEvents
    + MailboxDeliveryEvents
    + MailboxRequestEvents
    + MenuGuardEvents
    + SessionWarningEvents
    + Clone
    + Send
    + Sync
    + 'static
{
    #[cfg(test)]
    type CoordinatorTicket: CoordinatorTicket;

    #[cfg(test)]
    fn begin_coordinator_create(&self, cwd: &str) -> Option<Self::CoordinatorTicket>;

    fn master_required(&self) -> &MasterToken;

    fn selection_opt(&self) -> Option<&SelectionCoordinator>;

    fn restore_required(&self) -> &Arc<RestoreInProgress>;

    fn purge_required(&self) -> &Arc<PurgeGuard>;

    fn pending_clear(&self) -> &Arc<PendingSelfClear>;

    fn clocks(&self) -> Option<&CoordinatorClocksState>;

    fn shutdown_opt(&self) -> Option<&ShutdownSignal>;

    fn outbox(&self) -> &AppOutbox;

    #[cfg(test)]
    fn wake_create_probe(&self) -> Option<WakeCreateProbe> {
        None
    }

    fn create_session_inner<'a>(
        &'a self,
        request: MailboxSessionCreate<'a>,
    ) -> impl Future<Output = Result<SessionInfo, String>> + Send + 'a;

    fn create_session_inner_for_wake<'a>(
        &'a self,
        request: MailboxSessionCreate<'a>,
        _deadline: Option<tokio::time::Instant>,
    ) -> impl Future<Output = Result<SessionInfo, String>> + Send + 'a {
        self.create_session_inner(request)
    }

    fn create_session_inner_with_pty_target_ownership<'a, 'guard>(
        &'a self,
        request: MailboxSessionCreate<'a>,
        target_ownership: &'a PtyInputTargetOwnership<'guard>,
    ) -> impl Future<Output = Result<PtyTargetCreateStep, String>> + Send + 'a
    where
        'guard: 'a;

    fn create_owned_response_replacement<'a, 'guard>(
        &'a self,
        request: OwnedReplacementCreateRequest,
        continuation: OwnedReplacementContinuation,
        target: &'a PtyInputTargetOwnership<'guard>,
    ) -> impl Future<Output = Result<OwnedReplacementCreateStep, String>> + Send + 'a
    where
        'guard: 'a;

    fn background_destroy_session_inner<'a>(
        &'a self,
        uuid: Uuid,
    ) -> impl Future<Output = Result<(), String>> + Send + 'a;

    #[allow(
        clippy::too_many_arguments,
        reason = "host trait seam mirrors restart's managers, profile, intent and gate"
    )]
    fn restart_session_inner_with_intent<'a>(
        &'a self,
        _session_mgr: &'a Arc<RwLock<SessionManager>>,
        _pty_mgr: &'a Arc<Mutex<PtyManager>>,
        _settings: &'a SettingsState,
        uuid: Uuid,
        agent_id: Option<String>,
        requested_profile: Option<String>,
        skip_auto_resume: Option<bool>,
        activate_after: bool,
        intent: TrustedRestartIntent,
        communication_override: Option<SessionCommunication>,
        enforcement: CreationGateEnforcement,
    ) -> impl Future<Output = Result<SessionInfo, String>> + Send + 'a;

    fn attach_persisted_telegram_if_configured<'a>(
        &'a self,
        session_id: Uuid,
        bot_id: Option<&'a str>,
    ) -> impl Future<Output = ()> + Send + 'a;

    fn stamp_fresh_boundary_to_session<'a>(
        &'a self,
        session_id: Uuid,
    ) -> impl Future<Output = BoundaryMetadataOutcome> + Send + 'a;

    fn note_post_boundary_content_to_session<'a>(
        &'a self,
        session_id: Uuid,
    ) -> impl Future<Output = BoundaryMetadataOutcome> + Send + 'a;

    fn spawn_background(&self, future: BoxFuture<'static, ()>);
}

/// Synchronous browser effects used by mailbox policy.
pub(crate) trait BrowserHost: MailboxHost {
    fn agent_navigate(&self, url: &str) -> Result<WebBrowserSnapshot, String>;
    fn reload(&self) -> Result<WebBrowserSnapshot, String>;
    fn snapshot(&self) -> WebBrowserSnapshot;
}

#[cfg(test)]
mod tests;
