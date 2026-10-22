use super::notice::{
    InternalNoticeGuard, InternalSystemNotice, InternalSystemTarget, WakeDeliveryOrigin,
};
use super::settle::{observe_pty_spawn_scope, raw_privileged_probe, OutboxClassification};
#[cfg(test)]
use crate::phone::mailbox::ConsumptionVerdict;
use crate::phone::types::OutboxMessage;
use crate::pty::backend::SessionBackendKind;
use crate::pty::manager::PtyManager;
use crate::session::session::Session;
use crate::session::session::SessionStatus;
#[cfg(test)]
use crate::HashMap;
#[cfg(test)]
use std::collections::VecDeque;
use std::path::Path;
use std::sync::Arc;
use std::sync::Mutex;
use tauri::Manager;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

/// The notice is the largest variant by design: the remote-activity facts are
/// eight fields because the text and its validation need all eight, and boxing
/// them would add an allocation per notice to buy nothing measurable. The peer
/// arm is a borrow, so it is small either way.
#[allow(clippy::large_enum_variant)]
pub(crate) enum WakeDelivery<'a> {
    Peer {
        message: &'a OutboxMessage,
        origin: WakeDeliveryOrigin,
        /// #2232 phase 7: display-only attribution for a Co-managed-origin
        /// message. False for every pre-existing caller.
        comanaged: bool,
        response: Option<&'a ResponseWakeWork>,
    },
    InternalSystem {
        target: InternalSystemTarget,
        notice: InternalSystemNotice,
        cancellation: CancellationToken,
        guard: InternalNoticeGuard,
    },
}

pub(crate) async fn pty_spawn_scope_may_resume<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    target: &crate::config::teams::VerifiedPtyInputIdentity,
) -> bool {
    use crate::config::sessions_persistence::CreateCarrierScope;
    matches!(
        observe_pty_spawn_scope(app, target).await,
        (CreateCarrierScope::Foreign { .. }, _)
            | (
                CreateCarrierScope::Protected {
                    preparing_owner: None,
                    ..
                },
                Some(_),
            )
    )
}

pub(crate) fn pty_resume_metadata_matches(
    original: &crate::phone::types::PtyInputResult,
    current: &crate::phone::types::PtyInputResult,
    claimed: &crate::api::message_store::ClaimedPtyInputOperation,
) -> bool {
    !current.terminal
        && !original.terminal
        && original.status == crate::phone::types::PtyInputPublicStatus::Queued
        && original.issued_at.is_some()
        && original.queued_at.is_some()
        && original.payload_bytes.is_some()
        && original.payload_sha256.is_some()
        && current.injection_id == original.injection_id
        && current.op_id == original.op_id
        && current.sender == original.sender
        && current.target == original.target
        && current.source_plane == original.source_plane
        && current.expires_at == original.expires_at
        && current.status == crate::phone::types::PtyInputPublicStatus::Queued
        && current.injection_id == claimed.injection_id
        && current.op_id.as_deref() == Some(claimed.op_id.as_str())
        && current.sender.as_deref() == Some(claimed.sender_fqn.as_str())
        && current.target.as_deref() == Some(claimed.target_fqn.as_str())
        && current.source_plane == Some(claimed.source_plane)
        && current.expires_at.as_deref() == Some(claimed.expires_at.as_str())
        && current.version == original.version
        && current.payload_bytes == original.payload_bytes
        && current.payload_sha256 == original.payload_sha256
        && current.issued_at == original.issued_at
        && current.queued_at == original.queued_at
        && current.selected_session_id.is_none()
        && current.selected_backend.is_none()
        && current.actuating_at.is_none()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PtyClaimRefresh {
    Current,
    Terminal,
}

#[derive(Clone, Copy)]
pub(crate) enum PtyReacquireGoal {
    Spawn,
    Published(Uuid),
}

pub(crate) async fn reacquire_pty_dispatch_target<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    source: (
        &Arc<crate::api::message_store::MessageStore>,
        &crate::phone::types::PtyInputResult,
        &crate::api::message_store::ClaimedPtyInputOperation,
    ),
    target: (
        &Arc<crate::api::message_store::PtyInputTargetGate>,
        &crate::config::teams::VerifiedPtyInputIdentity,
        PtyReacquireGoal,
    ),
    deadline: (
        chrono::DateTime<chrono::Utc>,
        &tokio_util::sync::CancellationToken,
    ),
    heartbeat: &mut crate::api::message_store::PreparationHeartbeatGuard,
) -> Option<(
    crate::api::message_store::PtyInputStripeGuard,
    crate::api::message_store::PtyInputTargetGuard,
)> {
    use crate::phone::types::PtyInputReasonCode as C;
    let (store, initial, claimed) = source;
    let (gate, target, goal) = target;
    let (expires, shutdown) = deadline;
    let acquired = await_pty_input_before_deadline(expires, shutdown, async {
        if refresh_pty_claim_for_resume(store, initial, claimed).await? == PtyClaimRefresh::Terminal
        {
            return Ok(None);
        }
        if let Some(coordinator) =
            app.try_state::<crate::session::selection::SelectionCoordinator>()
        {
            coordinator
                .wait_for_restore_release()
                .await
                .map_err(|_| C::StoreTransient)?;
        }
        loop {
            let ready = match goal {
                PtyReacquireGoal::Spawn => pty_spawn_scope_may_resume(app, target).await,
                PtyReacquireGoal::Published(id) => {
                    !crate::config::sessions_persistence::unconfirmed_replacement_work_waits(
                        &target.canonical_fqn,
                        id,
                    )
                }
            };
            if ready {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        let stripe = gate
            .acquire_target_lock(&target.canonical_fqn)
            .await
            .map_err(|_| C::StoreTransient)?;
        let exact = gate.acquire_exact(&target.canonical_fqn).await;
        if refresh_pty_claim_for_resume(store, initial, claimed).await? == PtyClaimRefresh::Terminal
        {
            return Ok(None);
        }
        Ok::<_, C>(Some((stripe, exact)))
    })
    .await;
    match acquired {
        Ok(Ok(Some(guards))) => Some(guards),
        Ok(Ok(None)) | Err(PtyInputPreBoundaryStop::Shutdown) => {
            heartbeat.finish().await;
            None
        }
        Ok(Err(code)) => {
            finish_pty_input_before_boundary(
                store,
                heartbeat,
                &claimed.injection_id,
                &claimed.lease_owner,
                code,
            )
            .await;
            None
        }
        Err(PtyInputPreBoundaryStop::Expired) => {
            reject_pty_input_before_boundary(store, heartbeat, &claimed.injection_id, C::Expired)
                .await;
            None
        }
    }
}

pub(crate) async fn refresh_pty_claim_for_resume(
    store: &crate::api::message_store::MessageStore,
    original: &crate::phone::types::PtyInputResult,
    claimed: &crate::api::message_store::ClaimedPtyInputOperation,
) -> Result<PtyClaimRefresh, crate::phone::types::PtyInputReasonCode> {
    use crate::phone::types::PtyInputReasonCode as C;
    let current = store
        .query_pty_input_by_injection_offloaded(claimed.injection_id.clone())
        .await
        .map_err(|_| C::StoreTransient)?
        .ok_or(C::AuthorityChanged)?;
    if current.terminal {
        return Ok(PtyClaimRefresh::Terminal);
    }
    if !pty_resume_metadata_matches(original, &current, claimed) {
        return Err(C::AuthorityChanged);
    }
    match store
        .renew_pty_input_lease_offloaded(
            claimed.injection_id.clone(),
            claimed.lease_owner.clone(),
            chrono::Utc::now(),
        )
        .await
    {
        Ok(true) => Ok(PtyClaimRefresh::Current),
        _ => {
            // The original claimed-expiry parse already validated this value.
            let expires = crate::phone::types::parse_canonical_pty_timestamp(&claimed.expires_at)
                .map_err(|_| C::AuthorityChanged)?;
            Err(pty_input_lease_failure_code(expires))
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PtyInputPreBoundaryStop {
    Expired,
    Shutdown,
}

pub(crate) async fn await_pty_input_before_deadline<F, T>(
    expires_at: chrono::DateTime<chrono::Utc>,
    shutdown: &tokio_util::sync::CancellationToken,
    future: F,
) -> Result<T, PtyInputPreBoundaryStop>
where
    F: std::future::Future<Output = T>,
{
    let remaining = expires_at
        .signed_duration_since(chrono::Utc::now())
        .to_std()
        .map_err(|_| PtyInputPreBoundaryStop::Expired)?;
    tokio::select! {
        biased;
        _ = shutdown.cancelled() => Err(PtyInputPreBoundaryStop::Shutdown),
        _ = tokio::time::sleep(remaining) => Err(PtyInputPreBoundaryStop::Expired),
        output = future => Ok(output),
    }
}

pub(crate) fn pty_input_lease_failure_code(
    expires_at: chrono::DateTime<chrono::Utc>,
) -> crate::phone::types::PtyInputReasonCode {
    if expires_at <= chrono::Utc::now() {
        crate::phone::types::PtyInputReasonCode::Expired
    } else {
        crate::phone::types::PtyInputReasonCode::LeaseLost
    }
}

pub(crate) async fn reject_pty_input_before_boundary(
    store: &crate::api::message_store::MessageStore,
    heartbeat: &mut crate::api::message_store::PreparationHeartbeatGuard,
    injection_id: &str,
    code: crate::phone::types::PtyInputReasonCode,
) -> bool {
    heartbeat.finish().await;
    match store
        .terminalize_pty_input_offloaded(
            injection_id.to_string(),
            crate::phone::types::PtyInputPublicStatus::Rejected,
            Some(code),
            chrono::Utc::now(),
        )
        .await
    {
        Ok(result) => {
            crate::api::audit::record_pty_input_result("terminal", &result);
            result.terminal
        }
        Err(_) => {
            log::warn!(
                "[pty-input] rejection persistence failed id={} code=terminal_store_failed",
                injection_id
            );
            false
        }
    }
}

pub(crate) async fn finish_pty_input_before_boundary(
    store: &crate::api::message_store::MessageStore,
    heartbeat: &mut crate::api::message_store::PreparationHeartbeatGuard,
    injection_id: &str,
    lease_owner: &str,
    code: crate::phone::types::PtyInputReasonCode,
) -> bool {
    use crate::phone::types::PtyInputReasonCode as C;
    heartbeat.finish().await;
    if matches!(
        code,
        C::RestoreInProgress
            | C::PurgeInProgress
            | C::SessionRace
            | C::LeaseLost
            | C::SpawnFailedSafe
            | C::StoreTransient
            | C::MenuGuardBlocked
    ) {
        if store
            .retry_pty_input_offloaded(
                injection_id.to_string(),
                lease_owner.to_string(),
                code,
                chrono::Utc::now(),
            )
            .await
            .is_ok()
        {
            return true;
        }
        reject_pty_input_before_boundary(store, heartbeat, injection_id, C::StoreTransient).await
    } else {
        reject_pty_input_before_boundary(store, heartbeat, injection_id, code).await
    }
}

pub(crate) fn decode_outbox_snapshot(bytes: &[u8]) -> Result<String, ()> {
    if bytes.starts_with(&[0xff, 0xfe]) {
        let body = &bytes[2..];
        if !body.len().is_multiple_of(2) {
            return Err(());
        }
        let units: Vec<u16> = body
            .as_chunks::<2>()
            .0
            .iter()
            .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
            .collect();
        String::from_utf16(&units).map_err(|_| ())
    } else if bytes.starts_with(&[0xfe, 0xff]) {
        let body = &bytes[2..];
        if !body.len().is_multiple_of(2) {
            return Err(());
        }
        let units: Vec<u16> = body
            .as_chunks::<2>()
            .0
            .iter()
            .map(|chunk| u16::from_be_bytes([chunk[0], chunk[1]]))
            .collect();
        String::from_utf16(&units).map_err(|_| ())
    } else if bytes.starts_with(&[0xef, 0xbb, 0xbf]) {
        std::str::from_utf8(&bytes[3..])
            .map(str::to_owned)
            .map_err(|_| ())
    } else {
        std::str::from_utf8(bytes)
            .map(str::to_owned)
            .map_err(|_| ())
    }
}

pub(crate) fn classify_outbox_document(path: &Path) -> OutboxClassification {
    let (bytes, identity) = match crate::path_identity::read_bounded_regular(
        path,
        crate::phone::types::PTY_INPUT_HOST_ENVELOPE_MAX_BYTES,
    ) {
        Ok(snapshot) => snapshot,
        Err(_) => return OutboxClassification::InvalidDocument,
    };
    let raw_privileged = raw_privileged_probe(&bytes);
    let content = match decode_outbox_snapshot(&bytes) {
        Ok(content) => content,
        Err(()) => {
            return if raw_privileged {
                OutboxClassification::PrivilegedCandidate { bytes, identity }
            } else {
                OutboxClassification::InvalidDocument
            };
        }
    };
    let scanned = match crate::path_identity::scan_json_document(content.as_bytes()) {
        Ok(scanned) => scanned,
        Err(_) => {
            return if raw_privileged || raw_privileged_probe(content.as_bytes()) {
                OutboxClassification::PrivilegedCandidate { bytes, identity }
            } else {
                OutboxClassification::InvalidDocument
            };
        }
    };
    if scanned.privileged_candidate {
        OutboxClassification::PrivilegedCandidate { bytes, identity }
    } else {
        // Standard duplicate-key behavior remains unchanged. The retained
        // snapshot is passed to the legacy decoder without another file read.
        OutboxClassification::Standard(content)
    }
}

pub(crate) fn write_atomic_pty_metadata(
    destination: &Path,
    bytes: &[u8],
) -> Result<(), &'static str> {
    let parent = destination.parent().ok_or("unsafe_path")?;
    let parent_identity =
        crate::path_identity::verify_directory(parent).map_err(|_| "unsafe_path")?;
    let destination_identity = match std::fs::symlink_metadata(destination) {
        Ok(_) => Some(
            crate::path_identity::verify_regular_file(destination).map_err(|_| "unsafe_path")?,
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(_) => return Err("unsafe_path"),
    };
    let file_name = destination
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or("unsafe_path")?;
    let temp = parent.join(format!(".{file_name}.{}.pty-input-tmp", Uuid::new_v4()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(0x0002_0000);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
        };
        options
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let result = (|| {
        let mut file = options.open(&temp).map_err(|_| "artifact_unclaimed")?;
        use std::io::Write;
        file.write_all(bytes).map_err(|_| "artifact_unclaimed")?;
        file.flush().map_err(|_| "artifact_unclaimed")?;
        file.sync_all().map_err(|_| "artifact_unclaimed")?;
        crate::path_identity::verify_opened_regular_file(&temp, &file, false)
            .map_err(|_| "unsafe_path")?;
        drop(file);

        let current_parent =
            crate::path_identity::verify_directory(parent).map_err(|_| "unsafe_path")?;
        if !crate::path_identity::same_object(&parent_identity, &current_parent) {
            return Err("unsafe_path");
        }
        match &destination_identity {
            Some(expected) => {
                let current = crate::path_identity::verify_regular_file(destination)
                    .map_err(|_| "unsafe_path")?;
                if !crate::path_identity::same_object(expected, &current) {
                    return Err("unsafe_path");
                }
            }
            None => match std::fs::symlink_metadata(destination) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                _ => return Err("unsafe_path"),
            },
        }
        crate::config::root_agent::atomic_replace_existing(&temp, destination)
            .map_err(|_| "artifact_unclaimed")?;
        let (published, _) = crate::path_identity::read_bounded_regular(
            destination,
            crate::phone::types::PTY_INPUT_METADATA_MAX_BYTES,
        )
        .map_err(|_| "unsafe_path")?;
        if published != bytes {
            return Err("artifact_unclaimed");
        }
        if let Ok(parent_handle) = std::fs::File::open(parent) {
            if let Err(error) = parent_handle.sync_all() {
                log::debug!(
                    "[pty-input] metadata parent sync failed code=artifact_unclaimed \
                    error_kind={:?}",
                    error.kind()
                );
            }
        }
        Ok(())
    })();
    if result.is_err() {
        if let Err(error) = std::fs::remove_file(&temp) {
            if error.kind() != std::io::ErrorKind::NotFound {
                log::debug!(
                    "[pty-input] metadata temp cleanup failed code=artifact_unclaimed \
                    error_kind={:?}",
                    error.kind()
                );
            }
        }
    }
    result
}

/// The MailboxPoller runs as a background tokio task. It polls outbox directories
/// for all known agent repos, validates messages, and delivers them according to mode.
#[cfg(test)]
pub(crate) type MailboxAttachCalls = Arc<Mutex<Vec<(Uuid, Option<String>)>>>;

#[cfg(test)]
#[derive(Clone, Default)]
pub(crate) struct MailboxTestHooks {
    /// Consumed only by an actual accepted replacement; None keeps ticket mode.
    pub(crate) replacement_fixture:
        Arc<Mutex<Option<Arc<crate::session::selection::ReplacementFixtureControl>>>>,
    pub(crate) pty_presence: Arc<Mutex<HashMap<Uuid, bool>>>,
    pub(crate) inject_results: Arc<Mutex<VecDeque<Result<(), String>>>>,
    /// (#1001 PR1 / G6) Scripted consumption verdicts, mirroring
    /// `inject_results`. When non-empty, the `inject_wake_into_pty` hook arm
    /// pops one after a successful inject and runs the SHARED `verdict_to_result`
    /// - so the AC3 hooked test exercises the real conversion, not a copy.
    pub(crate) consumption_results: Arc<Mutex<VecDeque<ConsumptionVerdict>>>,
    pub(crate) inject_calls: Arc<Mutex<Vec<Uuid>>>,
    pub(crate) settle_calls: Arc<Mutex<Vec<Uuid>>>,
    /// Sessions whose hooked live-settle step removes the SessionManager
    /// record, deterministically exercising the post-preflight race path.
    pub(crate) remove_session_on_settle: Arc<Mutex<std::collections::HashSet<Uuid>>>,
    /// (#1399 T9) Sessions whose hooked inject panics instead of returning,
    /// reproducing the poisoned-PtyManager-lock unwind on the real delivery
    /// path so the `WakeOutcomeReport` guard is testable.
    pub(crate) panic_on_inject: Arc<Mutex<std::collections::HashSet<Uuid>>>,
    /// Sessions that record hook observability but continue through the real
    /// command branch and canonical injector instead of scripted injection.
    pub(crate) real_inject_sessions: Arc<Mutex<std::collections::HashSet<Uuid>>>,
    pub(crate) internal_payloads: Arc<Mutex<Vec<String>>>,
    pub(crate) internal_bookkeeping: Arc<Mutex<Vec<InternalSystemBookkeeping>>>,
    pub(crate) internal_live_settle_gate: Arc<Mutex<Option<tokio::sync::oneshot::Receiver<()>>>>,
    pub(crate) internal_live_settle_entered: Arc<tokio::sync::Notify>,
    pub(crate) internal_spawn_gate: Arc<Mutex<Option<tokio::sync::oneshot::Receiver<()>>>>,
    pub(crate) internal_spawn_started: Arc<tokio::sync::Notify>,
    pub(crate) destroy_calls: Arc<Mutex<Vec<Uuid>>>,
    pub(crate) destroy_results: Arc<Mutex<VecDeque<Result<(), String>>>>,
    pub(crate) spawn_calls: Arc<Mutex<Vec<MailboxSpawnCall>>>,
    pub(crate) attach_calls: MailboxAttachCalls,
    pub(crate) events: Arc<Mutex<Vec<MailboxTestEvent>>>,
    /// (#747) `is_coordinator` for sessions created by the test spawn hook
    /// (default false, matching the historical harness). Production
    /// `create_session_inner` recomputes the flag from teams discovery, so a
    /// wake that respawns a coordinator target yields a coordinator record;
    /// tests exercising the raised-hand carry set this to mirror that.
    pub(crate) spawn_is_coordinator: Arc<Mutex<bool>>,
    /// (#2411) One-shot error for the test spawn arm, returned after the real
    /// coordinator-create clear runs, mirroring a create that fails after it.
    pub(crate) internal_spawn_error: Arc<Mutex<Option<String>>>,
}

#[cfg(test)]
impl MailboxTestHooks {
    pub(crate) fn replacement_fixture_control(
        &self,
    ) -> Option<Arc<crate::session::selection::ReplacementFixtureControl>> {
        self.replacement_fixture
            .lock()
            .expect("replacement fixture control")
            .clone()
    }
}

#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InternalSystemBookkeeping {
    pub(crate) session_id: Uuid,
    pub(crate) post_boundary: bool,
    pub(crate) silence_touch: bool,
    pub(crate) set_last_prompt: bool,
    pub(crate) peer_event: bool,
    pub(crate) response_watcher: bool,
    pub(crate) consumption_verdict: bool,
    pub(crate) mailbox_archive: bool,
}

#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MailboxSpawnCall {
    pub(crate) to: String,
    pub(crate) session_name: String,
    pub(crate) cwd: String,
    pub(crate) shell: String,
    pub(crate) shell_args: Vec<String>,
    pub(crate) skip_auto_resume: bool,
    /// #1635-2 D5: true when the wake carried dispatch flags and the spawn
    /// suppressed the config.json tooling writes. The test-hook boundary is
    /// the observable decision surface (the hook short-circuits to the
    /// in-memory `mgr.create_session` and never writes config.json).
    pub(crate) skip_tooling_save: bool,
}

#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MailboxTestEvent {
    Inject(Uuid),
    Destroy(Uuid),
    Spawn(MailboxSpawnCall),
    Attach {
        session_id: Uuid,
        bot_id: Option<String>,
    },
}

pub(crate) fn response_diag_emit_lazy(
    stage: &'static str,
    outcome: impl FnOnce() -> &'static str,
    id: impl FnOnce() -> Option<Uuid>,
    site: u32,
) {
    if log::log_enabled!(
        target: "agentscommander_lib::response_close_diag",
        log::Level::Debug
    ) {
        if let Some(id) = id() {
            log::debug!(target: "agentscommander_lib::response_close_diag", "response_close_diag \
                stage={} outcome={} site={} id={}", stage, outcome(), site, id);
        } else {
            log::debug!(target: "agentscommander_lib::response_close_diag", "response_close_diag \
                stage={} outcome={} site={} id=unknown", stage, outcome(), site);
        }
    }
}

pub(crate) fn response_diag_option_lazy<T>(
    value: Option<T>,
    stage: &'static str,
    outcome: &'static str,
    id: impl FnOnce() -> Option<Uuid>,
    site: u32,
) -> Option<T> {
    match value {
        Some(value) => Some(value),
        None => {
            response_diag_emit_lazy(stage, || outcome, id, site);
            None
        }
    }
}

pub(crate) fn response_diag_result_ref<T>(
    value: &Result<T, crate::pty::manager::ResponseCloseError>,
    stage: &'static str,
    id: impl FnOnce() -> Option<Uuid>,
    site: u32,
) {
    if log::log_enabled!(
        target: "agentscommander_lib::response_close_diag",
        log::Level::Debug
    ) {
        let outcome = match value {
            Ok(_) => "ok",
            Err(crate::pty::manager::ResponseCloseError::Unavailable) => "unavailable",
            Err(crate::pty::manager::ResponseCloseError::Unknown) => "unknown",
            Err(crate::pty::manager::ResponseCloseError::Conflict) => "conflict",
            Err(crate::pty::manager::ResponseCloseError::ClosingDeferred) => "closing_deferred",
        };
        response_diag_emit_lazy(stage, || outcome, id, site);
    }
}

pub(crate) fn response_diag_debug_only(action: impl FnOnce()) {
    if log::log_enabled!(
        target: "agentscommander_lib::response_close_diag",
        log::Level::Debug
    ) {
        action();
    }
}

pub(crate) fn current_response_row<'a>(
    current: &'a crate::session::manager::CarrierSnapshot,
    identity: &crate::pty::manager::ResponseRouteIdentity,
) -> Option<&'a Session> {
    current.rows.iter().find(|row| {
        row.id == identity.session_id
            && row.token == identity.token
            && row.backend_kind == identity.backend
            && !current.pending_ids.contains(&row.id)
            && !matches!(row.status, SessionStatus::Exited(_))
            && current.versions.get(&row.id) == Some(&identity.carrier_version)
    })
}

/// Both frozen route lifecycles and the caller's current interior manager read
/// remain live through the pure tracker transition. The coordinator keeps its
/// actual backend; the member alone must be a local interactive process.
pub(crate) fn with_verified_response_pair<T>(
    current: &crate::session::manager::CarrierSnapshot,
    pty: &Arc<Mutex<PtyManager>>,
    state: &crate::pty::manager::ResponseCloseState,
    member: &crate::pty::manager::ResponseRouteIdentity,
    coordinator: &crate::pty::manager::ResponseRouteIdentity,
    publish: impl FnOnce(&Session, &Session) -> T,
) -> Option<T> {
    response_diag_debug_only(|| {
        log::debug!(target: "agentscommander_lib::response_close_diag", "response_close_diag \
            site={} outcome=observed stage=verified_pair scope=stage_current id=unknown member={} \
            coordinator={} current_member_version={:?} current_coordinator_version={:?} \
            captured_member_version={} captured_coordinator_version={} \
            token_backend_role_physical_checks=not_evaluated",
            line!(),
            member.session_id,
            coordinator.session_id,
            current.versions.get(&member.session_id),
            current.versions.get(&coordinator.session_id),
            member.carrier_version,
            coordinator.carrier_version
        );
    });
    let member_row = response_diag_option_local!(
        current_response_row(current, member),
        "verified_pair",
        "current_member_rejected",
        None::<Uuid>
    )?;
    let coordinator_row = response_diag_option_local!(
        current_response_row(current, coordinator),
        "verified_pair",
        "current_coordinator_rejected",
        None::<Uuid>
    )?;
    if member.backend != SessionBackendKind::LocalProcess
        || member_row.is_root_agent
        || member_row.is_coordinator
        || member_row.agent_kind.is_none()
        || !coordinator_row.is_coordinator
        || coordinator_row.is_root_agent
    {
        response_diag_local!("verified_pair", "role_or_backend", None::<Uuid>);
        return None;
    }
    let member_route = response_diag_option_local!(
        PtyManager::snapshot_route_proof(pty, member.session_id).ok(),
        "verified_pair",
        "member_route_proof_missing",
        None::<Uuid>
    )?;
    let coordinator_route = response_diag_option_local!(
        PtyManager::snapshot_route_proof(pty, coordinator.session_id).ok(),
        "verified_pair",
        "coordinator_route_proof_missing",
        None::<Uuid>
    )?;
    if member_route.backend_kind() != SessionBackendKind::LocalProcess
        || coordinator_route.backend_kind() != coordinator.backend
        || !crate::path_identity::verify_directory(std::path::Path::new(
            &member_row.working_directory,
        ))
        .is_ok_and(|actual| crate::path_identity::same_object(&actual, member_route.saved_cwd()))
        || !crate::path_identity::verify_directory(std::path::Path::new(
            &coordinator_row.working_directory,
        ))
        .is_ok_and(|actual| {
            crate::path_identity::same_object(&actual, coordinator_route.saved_cwd())
        })
    {
        response_diag_local!("verified_pair", "backend_or_cwd_mismatch", None::<Uuid>);
        return None;
    }
    let member_authority = response_diag_option_local!(
        PtyManager::authority_route_proof(pty, member.session_id).ok(),
        "verified_pair",
        "member_authority_proof_missing",
        None::<Uuid>
    )?;
    let coordinator_authority = response_diag_option_local!(
        PtyManager::authority_route_proof(pty, coordinator.session_id).ok(),
        "verified_pair",
        "coordinator_authority_proof_missing",
        None::<Uuid>
    )?;
    let _member_guard = response_diag_option_local!(
        member_authority
            .lock_verified(
                member.backend,
                member_route.saved_cwd(),
                member_route.saved_replica(),
            )
            .ok(),
        "verified_pair",
        "member_authority_lock_rejected",
        None::<Uuid>
    )?;
    let _coordinator_guard = response_diag_option_local!(
        coordinator_authority
            .lock_verified(
                coordinator.backend,
                coordinator_route.saved_cwd(),
                coordinator_route.saved_replica(),
            )
            .ok(),
        "verified_pair",
        "coordinator_authority_lock_rejected",
        None::<Uuid>
    )?;
    if state.generation(member.session_id) != Some(member.generation)
        || state.generation(coordinator.session_id) != Some(coordinator.generation)
    {
        response_diag_local!("verified_pair", "generation_mismatch", None::<Uuid>);
        return None;
    }
    response_diag_debug_only(|| {
        log::debug!(target: "agentscommander_lib::response_close_diag", "response_close_diag \
            site={} outcome=observed stage=verified_pair scope=stage_current member={} \
            coordinator={} captured_member_version={} captured_coordinator_version={} \
            current_member_version={:?} current_coordinator_version={:?} \
            token_backend_role_physical_generation_checks=true",
            line!(),
            member.session_id,
            coordinator.session_id,
            member.carrier_version,
            coordinator.carrier_version,
            current.versions.get(&member.session_id),
            current.versions.get(&coordinator.session_id)
        );
    });
    response_diag_local!("verified_pair", "callback_reached", None::<Uuid>);
    Some(publish(member_row, coordinator_row))
}

#[derive(Clone)]
pub(crate) struct ResponseRequestCandidate {
    pub(crate) context: crate::pty::manager::ResponseDeliveryContext,
    pub(crate) member: crate::pty::manager::ResponseRouteIdentity,
    pub(crate) coordinator: crate::pty::manager::ResponseRouteIdentity,
    pub(crate) initial: crate::config::sessions_persistence::InitialCarrierIntent,
}

pub(crate) struct ResponseCandidateAttempt {
    pub(crate) state: crate::pty::manager::ResponseCloseState,
    pub(crate) candidate: Mutex<Option<ResponseRequestCandidate>>,
    pub(crate) armed: std::sync::atomic::AtomicBool,
}

pub(crate) struct ResponseCandidateDisposal(pub(crate) Arc<ResponseCandidateAttempt>);

impl Drop for ResponseCandidateDisposal {
    fn drop(&mut self) {
        match self.0.candidate.lock() {
            Ok(candidate) => {
                response_diag_local!("candidate_disposal", "invoked", None::<Uuid>);
                if self
                    .0
                    .armed
                    .swap(false, std::sync::atomic::Ordering::AcqRel)
                {
                    if let Some(candidate) = candidate.as_ref() {
                        response_diag_local!(
                            "mark_request_submitted",
                            "false_invocation",
                            None::<Uuid>
                        );
                        response_diag_debug_only(|| {
                            log::debug!(
                                target: "agentscommander_lib::response_close_diag",
                                "response_close_diag stage=candidate_disposal \
                                 outcome=false_submission_invoked site={} id=unknown member={} \
                                 coordinator={}",
                                line!(),
                                candidate.member.session_id,
                                candidate.coordinator.session_id
                            );
                        });
                        self.0
                            .state
                            .mark_request_submitted(&candidate.context, false);
                    } else {
                        response_diag_local!(
                            "candidate_disposal",
                            "candidate_absent",
                            None::<Uuid>
                        );
                    }
                } else {
                    response_diag_local!("candidate_disposal", "attempt_disarmed", None::<Uuid>);
                }
            }
            Err(_) => {
                response_diag_local!("candidate_disposal", "candidate_lock_failure", None::<Uuid>);
                self.0.state.fail_closed()
            }
        }
    }
}

impl ResponseWakeWork {
    pub(crate) fn candidate_disposal(&self) -> ResponseCandidateDisposal {
        let attempt = Arc::new(ResponseCandidateAttempt {
            state: self.state.clone(),
            candidate: Mutex::new(None),
            armed: std::sync::atomic::AtomicBool::new(true),
        });
        *self
            .attempt
            .lock()
            .unwrap_or_else(|poison| poison.into_inner()) = Some(attempt.clone());
        ResponseCandidateDisposal(attempt)
    }
}

#[derive(Clone)]
pub(crate) struct ResponseWakeWork {
    pub(crate) diagnostic_message_id: Option<Uuid>,
    pub(crate) state: crate::pty::manager::ResponseCloseState,
    pub(crate) target: String,
    pub(crate) key: String,
    pub(crate) digest: String,
    pub(crate) attempt: Arc<Mutex<Option<Arc<ResponseCandidateAttempt>>>>,
    pub(crate) return_context: Option<crate::pty::manager::ReturnObservationContext>,
    pub(crate) return_routes: Option<(
        crate::pty::manager::ResponseRouteIdentity,
        crate::pty::manager::ResponseRouteIdentity,
    )>,
    // P05 creates the nonce-bound context at the actual pre-write boundary.
    #[allow(dead_code)]
    pub(crate) delivery_context: Option<crate::pty::manager::ResponseDeliveryContext>,
}
