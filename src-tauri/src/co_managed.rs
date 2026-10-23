//! Co-managed supervisor (#2232 phase 7).

pub(crate) mod route;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::config::settings::SettingsState;
use crate::session::manager::SessionManager;
use tauri::Manager;

use crate::{capture, config};

// ═══════════════════════════════════════════════════════════════════════════
// #2232 phase 7: the Co-managed supervisor
//
// Lives in a private root module: it joins the crate-root SCC, so the arcs the
// effect needs (`config::teams`, `phone::messaging`, `phone::mailbox`,
// `capture::*`) stay internal to the cycle or SCC-to-leaf (phase 7 section 4).
// ═══════════════════════════════════════════════════════════════════════════

/// Attempts per trigger: the first plus two retries (plan section 5.1).
pub(crate) const CO_MANAGED_MAX_ATTEMPTS: u32 = 3;
/// Backoff before retry 2 and retry 3, in milliseconds. The length is derived
/// from `CO_MANAGED_MAX_ATTEMPTS` so the two cannot drift: raising the attempt
/// bound without extending the backoff table is a compile error, not a runtime
/// `index out of bounds`.
pub(crate) const CO_MANAGED_RETRY_BACKOFF_MS: [u64; CO_MANAGED_MAX_ATTEMPTS as usize - 1] =
    [25, 50];
/// Excerpt cap in UTF-8 bytes, following the 500-byte trim the bridge logger
/// already uses (`telegram/output.rs:403`). The excerpt is persisted and
/// travels over IPC and `list-peers`, so it cannot be uncapped. It is never a
/// multi-byte character cut in half.
pub(crate) const CO_MANAGED_EXCERPT_BYTES: usize = 500;

/// A supervisor input. (a) is the orchestrator's idle edge, emitted from the
/// real `IdleDetector` callback; (b) is a slot transition, which the slot
/// watcher forwards for every record, invalidation and clearing. #2756 U2a
/// `Recovery` re-offers a candidate that a not-ready trigger left pending, once
/// the room is `Ready`; it takes the `SlotChanged` path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CoManagedTrigger {
    IdleEdge(uuid::Uuid),
    SlotChanged(uuid::Uuid),
    Recovery(uuid::Uuid),
}

impl CoManagedTrigger {
    pub(crate) fn session_id(self) -> uuid::Uuid {
        match self {
            Self::IdleEdge(id) | Self::SlotChanged(id) | Self::Recovery(id) => id,
        }
    }
}

/// #2756 U2a: a candidate dropped because the room was not `Ready`. Both legs
/// must match the slot for a recovery: `seq`, and the retained `Arc`, which
/// keeps the allocation alive so `Arc::ptr_eq` cannot alias a new record.
pub(crate) struct PendingRecovery {
    pub(crate) seq: u64,
    pub(crate) record: Arc<capture::record::CapturedRecord>,
}

#[derive(Default)]
pub(crate) struct CoManagedSupervisorState {
    /// Sessions whose last cycle ended in contention, keyed to the slot
    /// sequence that candidate had. The same sequence must not re-trigger.
    contended: HashMap<String, u64>,
    /// Sessions whose slot already has a watcher task.
    watching: HashSet<String>,
    /// #2756 U2a: candidates dropped while not `Ready`, awaiting recovery.
    pub(crate) pending_recovery: HashMap<String, PendingRecovery>,
}

#[cfg(test)]
#[derive(Default)]
pub(crate) struct CoManagedTestHooks {
    steps: std::sync::Mutex<Vec<&'static str>>,
    commit_calls: std::sync::atomic::AtomicUsize,
    /// Runs once inside `commit_co_managed_with_retries`, after a successful commit and before the
    /// caller consumes the slot. One-shot: the only way a test can offer a newer record in that
    /// window.
    after_commit: std::sync::Mutex<Option<Box<dyn FnOnce() + Send>>>,
}

#[cfg(test)]
impl CoManagedTestHooks {
    fn record_step(&self, step: &'static str) {
        self.steps
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(step);
    }

    pub(crate) fn steps(&self) -> Vec<&'static str> {
        self.steps.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub(crate) fn commit_calls(&self) -> usize {
        self.commit_calls.load(std::sync::atomic::Ordering::Acquire)
    }

    pub(crate) fn set_after_commit(&self, hook: Box<dyn FnOnce() + Send>) {
        *self.after_commit.lock().unwrap_or_else(|e| e.into_inner()) = Some(hook);
    }

    fn take_after_commit(&self) -> Option<Box<dyn FnOnce() + Send>> {
        self.after_commit
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
    }
}

/// App-wide supervisor handle. Cheap to clone; every clone shares the armed
/// flags, the contention state and the single trigger channel.
#[derive(Clone)]
pub(crate) struct CoManagedSupervisorHandle {
    pub(crate) armed: Arc<capture::armed::ArmedFlags>,
    state: Arc<Mutex<CoManagedSupervisorState>>,
    triggers: tokio::sync::mpsc::UnboundedSender<CoManagedTrigger>,
    #[cfg(test)]
    test_hooks: Option<Arc<CoManagedTestHooks>>,
}

impl CoManagedSupervisorHandle {
    pub(crate) fn new() -> (Self, tokio::sync::mpsc::UnboundedReceiver<CoManagedTrigger>) {
        let (triggers, rx) = tokio::sync::mpsc::unbounded_channel();
        (
            Self {
                armed: Arc::new(capture::armed::ArmedFlags::new()),
                state: Arc::new(Mutex::new(CoManagedSupervisorState::default())),
                triggers,
                #[cfg(test)]
                test_hooks: None,
            },
            rx,
        )
    }

    #[cfg(test)]
    pub(crate) fn with_test_hooks(
        hooks: Arc<CoManagedTestHooks>,
    ) -> (Self, tokio::sync::mpsc::UnboundedReceiver<CoManagedTrigger>) {
        let (mut handle, rx) = Self::new();
        handle.test_hooks = Some(hooks);
        (handle, rx)
    }

    fn notify_idle_edge(&self, session_id: uuid::Uuid) -> bool {
        self.triggers
            .send(CoManagedTrigger::IdleEdge(session_id))
            .is_ok()
    }

    pub(crate) fn notify_slot_changed(&self, session_id: uuid::Uuid) -> bool {
        self.triggers
            .send(CoManagedTrigger::SlotChanged(session_id))
            .is_ok()
    }

    pub(crate) fn notify_recovery(&self, session_id: uuid::Uuid) -> bool {
        self.triggers
            .send(CoManagedTrigger::Recovery(session_id))
            .is_ok()
    }

    pub(crate) fn lock_state(&self) -> std::sync::MutexGuard<'_, CoManagedSupervisorState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub(crate) fn is_watching(&self, id: &str) -> bool {
        self.lock_state().watching.contains(id)
    }

    pub(crate) fn mark_watching(&self, id: &str) {
        self.lock_state().watching.insert(id.to_string());
    }

    pub(crate) fn mark_unwatched(&self, id: &str) {
        let mut state = self.lock_state();
        state.watching.remove(id);
        state.pending_recovery.remove(id);
    }

    fn mark_pending_recovery(
        &self,
        id: &str,
        seq: u64,
        record: Arc<capture::record::CapturedRecord>,
    ) {
        self.lock_state()
            .pending_recovery
            .insert(id.to_string(), PendingRecovery { seq, record });
    }

    fn is_contended(&self, id: &str, seq: u64) -> bool {
        self.lock_state().contended.get(id) == Some(&seq)
    }

    pub(crate) fn mark_contended(&self, id: &str, seq: u64) {
        self.lock_state().contended.insert(id.to_string(), seq);
    }

    fn clear_contended(&self, id: &str) {
        self.lock_state().contended.remove(id);
    }

    fn record_step(&self, step: &'static str) {
        #[cfg(test)]
        if let Some(hooks) = &self.test_hooks {
            hooks.record_step(step);
        }
        let _ = step;
    }

    fn record_commit_call(&self) {
        #[cfg(test)]
        if let Some(hooks) = &self.test_hooks {
            hooks
                .commit_calls
                .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        }
    }

    /// Test-only seam: see `CoManagedTestHooks::after_commit`.
    fn run_after_commit_hook(&self) {
        #[cfg(test)]
        if let Some(hooks) = &self.test_hooks {
            if let Some(hook) = hooks.take_after_commit() {
                hook();
            }
        }
    }
}

/// The synchronous first half of the idle edge (plan section 9.1). Emits
/// `session_idle` **first**, with the Co-managed decision in the same payload,
/// then hands the edge to the supervisor. A separate later event would always
/// paint waiting first, which is what this signature exists to prevent.
pub(crate) fn emit_session_idle_edge<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    armed: &capture::armed::ArmedFlags,
    supervisor: Option<&CoManagedSupervisorHandle>,
    session_id: uuid::Uuid,
) -> bool {
    let comanaged = armed.is_armed(&session_id.to_string());
    let _ = tauri::Emitter::emit(
        app,
        "session_idle",
        serde_json::json!({ "id": session_id.to_string(), "comanaged": comanaged }),
    );
    if let Some(supervisor) = supervisor {
        let _ = supervisor.notify_idle_edge(session_id);
    }
    comanaged
}

/// `session_comanaged_state`, emitted with `emit` (every window), never
/// `emit_to`, so a detached terminal window receives it too (plan 9.2).
pub(crate) fn emit_co_managed_state<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    session_id: uuid::Uuid,
    active: bool,
    reason: Option<&str>,
) {
    let _ = tauri::Emitter::emit(
        app,
        "session_comanaged_state",
        serde_json::json!({
            "id": session_id.to_string(),
            "active": active,
            "reason": reason,
        }),
    );
}

pub(crate) fn co_managed_state_reason(
    state: &Result<crate::config::co_managed::CoManagedState, String>,
) -> String {
    use crate::config::co_managed::{CoManagedState, OffReason};
    match state {
        Ok(CoManagedState::Ready) => "Ready".to_string(),
        Ok(CoManagedState::Off { reason }) => match reason {
            OffReason::GlobalSwitchOff => "GlobalSwitchOff".to_string(),
            OffReason::NotAnOrchestrator => "NotAnOrchestrator".to_string(),
            OffReason::UnsupportedProvider { agent } => format!("UnsupportedProvider({agent})"),
            OffReason::RoomFlagOff => "RoomFlagOff".to_string(),
            OffReason::NoApiKey => "NoApiKey".to_string(),
            OffReason::NoCatalogFile => "NoCatalogFile".to_string(),
            OffReason::CatalogUnreadable => "CatalogUnreadable".to_string(),
        },
        Err(error) => format!("EffectiveStateError({error})"),
    }
}

pub(crate) fn abstain_reason_label(reason: &capture::state::AbstainReason) -> String {
    use capture::state::AbstainReason;
    match reason {
        AbstainReason::PreconditionsStale => "PreconditionsStale".to_string(),
        AbstainReason::LockBusy => "LockBusy".to_string(),
        AbstainReason::LockUnavailable(error) => format!("LockUnavailable({error})"),
        AbstainReason::PreconditionsRejected => "PreconditionsRejected".to_string(),
        AbstainReason::SlotChanged => "SlotChanged".to_string(),
        AbstainReason::AlreadyConsumed => "AlreadyConsumed".to_string(),
        AbstainReason::BudgetExhausted => "BudgetExhausted".to_string(),
    }
}

/// The user-visible label for a candidate. `provider_final == false` is every
/// Claude candidate (epic 3.3), and no rendering may ever call it a final
/// message or imply the user approved anything.
pub(crate) fn co_managed_candidate_label(provider_final: bool) -> &'static str {
    if provider_final {
        "final message"
    } else {
        "latest captured assistant text at the idle edge"
    }
}

/// The 500-byte cap, UTF-8 safe: never cut a multi-byte character in half.
pub(crate) fn utf8_safe_excerpt(text: &str, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text.to_string();
    }
    let mut end = max_bytes;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_string()
}

pub(crate) fn co_managed_user_message(
    reason: &str,
    candidate: Option<(&str, bool, Option<&Path>)>,
) -> String {
    let mut out = format!("Co-managed: {reason}");
    if let Some((text, provider_final, path)) = candidate {
        let excerpt = utf8_safe_excerpt(text, CO_MANAGED_EXCERPT_BYTES);
        out.push_str(&format!(
            "\n{} ({} bytes); excerpt is {} of {} bytes:\n{}",
            co_managed_candidate_label(provider_final),
            text.len(),
            excerpt.len(),
            text.len(),
            excerpt,
        ));
        if let Some(path) = path {
            out.push_str(&format!("\nMessage file: {}", path.display()));
        }
    }
    out
}

/// Surface a user-facing communication on the session. The slot is single:
/// `raise_hand` and `set_blocked_menu` overwrite it, and when that happens the
/// pointer is lost, not the file, which stays findable in `messaging/`.
pub(crate) async fn surface_co_managed_user_message<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    session_id: uuid::Uuid,
    message: String,
) {
    let Some(manager) = app.try_state::<Arc<tokio::sync::RwLock<SessionManager>>>() else {
        return;
    };
    let outcome = {
        let guard = manager.read().await;
        guard
            .set_co_managed(session_id, message, chrono::Utc::now())
            .await
    };
    if let Some((changed, communication)) = outcome {
        if changed {
            crate::session::selection::publish_session_communication(
                app,
                session_id,
                Some(&communication),
            );
        }
    }
}

/// Read the six `AppSettings` Jev fields into the leaf's plain value type.
pub(crate) async fn co_managed_jev_settings<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
) -> Result<capture::jev::JevSettings, config::settings::SettingsOwnerError> {
    let settings = app.state::<SettingsState>();
    let guard = settings.current_read().await?;
    Ok(capture::jev::JevSettings {
        api_key: guard.jev_api_key.clone(),
        model: guard.jev_model.clone(),
        endpoint: guard.jev_endpoint.clone(),
        timeout_secs: guard.jev_timeout_secs,
        threshold: guard.jev_threshold,
        margin: guard.jev_margin,
    })
}

/// Load and validate the room's catalog through the leaf loader. A missing
/// path or a missing file is `Catalog::missing()`, which `classify` turns into
/// an abstention with `NoCatalogFile`.
pub(crate) fn co_managed_catalog(room_root: &Path) -> capture::catalog::Catalog {
    let config = crate::config::co_managed::load_config(room_root);
    let Some(path) = config.catalog_path.as_deref() else {
        return capture::catalog::Catalog::missing();
    };
    let candidate = Path::new(path);
    let resolved = if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        room_root.join(candidate)
    };
    capture::catalog::Catalog::load(&resolved)
}

// ── the routed body is a pointer, not the text (plan section 7) ─────────────

/// Write the message file into the sending orchestrator's own room
/// `messaging/`, then return `(path, pointer)`. The pointer is the canonical
/// `Process this inter-agent message: <abs path>` body.
///
/// `ensure_workgroup_root_is_authoritative` is called explicitly: the CLI did
/// that for every file it wrote, and an in-process caller does not inherit it.
pub(crate) fn write_co_managed_message_file(
    room_root: &Path,
    from_fqn: &str,
    to_fqn: &str,
    content: &str,
) -> Result<(PathBuf, String), String> {
    crate::phone::messaging::ensure_workgroup_root_is_authoritative(room_root)?;
    let dir = crate::phone::messaging::messaging_dir(room_root).map_err(|e| e.to_string())?;
    let from_short = crate::phone::messaging::agent_short_name(from_fqn);
    let to_short = crate::phone::messaging::agent_short_name(to_fqn);
    let slug = crate::phone::messaging::sanitize_slug("co-managed").map_err(|e| e.to_string())?;
    let base =
        crate::phone::messaging::build_filename(chrono::Utc::now(), &from_short, &to_short, &slug);
    let (path, mut file) =
        crate::phone::messaging::create_message_file(&dir, &base).map_err(|e| e.to_string())?;
    use std::io::Write;
    let written = (|| -> Result<(), String> {
        file.write_all(content.as_bytes())
            .map_err(|e| format!("message file write failed: {e}"))?;
        file.flush()
            .map_err(|e| format!("message file flush failed: {e}"))
    })();
    if let Err(error) = written {
        let _ = std::fs::remove_file(&path);
        return Err(error);
    }
    let abs = std::fs::canonicalize(&path).map_err(|e| e.to_string())?;
    // UNC-strip at the single emission site, exactly as `cli/send` does.
    let abs_str = abs.to_string_lossy();
    let abs_display = abs_str.trim_start_matches(r"\\?\");
    Ok((
        path,
        crate::phone::messaging::format_file_notification(abs_display),
    ))
}

/// The queue envelope: a wake carrying the pointer, no token, no action field.
pub(crate) fn write_co_managed_queue_message(
    room_root: &Path,
    from_fqn: &str,
    to_fqn: &str,
    pointer: &str,
) -> Result<PathBuf, String> {
    let dir = crate::config::co_managed::queue_dir(room_root);
    std::fs::create_dir_all(&dir).map_err(|e| format!("queue dir create failed: {e}"))?;
    let envelope = crate::phone::types::OutboxMessage {
        id: uuid::Uuid::new_v4().to_string(),
        token: None,
        from: from_fqn.to_string(),
        to: to_fqn.to_string(),
        body: pointer.to_string(),
        mode: "wake".to_string(),
        get_output: false,
        request_id: None,
        sender_agent: None,
        preferred_agent: "auto".to_string(),
        requested_profile: None,
        effective_agent_id: None,
        effective_profile: None,
        profile_fallback_applied: false,
        dispatch_not_applied: None,
        priority: "normal".to_string(),
        timestamp: chrono::Utc::now().to_rfc3339(),
        command: None,
        action: None,
        target: None,
        force: None,
        timeout_secs: None,
        switch_coding_agent: None,
        switch_profile: None,
        dry_run: None,
        quiet_period_ms: None,
        pty_input: None,
    };
    let bytes = serde_json::to_vec_pretty(&envelope)
        .map_err(|e| format!("queue envelope serialize failed: {e}"))?;
    let path = dir.join(format!("{}.json", envelope.id));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    let mut file = options
        .open(&path)
        .map_err(|e| format!("queue envelope create failed: {}", e))?;
    use std::io::Write;
    file.write_all(&bytes)
        .map_err(|e| format!("queue envelope write failed: {e}"))?;
    file.flush()
        .map_err(|e| format!("queue envelope flush failed: {e}"))?;
    Ok(path)
}

/// Create the message file, then enqueue the pointer. The file exists before
/// the queue entry so the pointer resolves when the recipient reads it. The
/// line budget is the CLI's formula with the **composed** sender, which is
/// longer; a body that does not fit is rejected with a visible reason and is
/// never truncated.
pub(crate) fn route_co_managed_wake(
    handle: &CoManagedSupervisorHandle,
    room_root: &Path,
    from_fqn: &str,
    to_fqn: &str,
    content: &str,
) -> Result<Option<String>, String> {
    let (path, pointer) = write_co_managed_message_file(room_root, from_fqn, to_fqn, content)?;
    handle.record_step("messaging_file");
    let display_from = crate::phone::messaging::compose_sender_for_comanaged_origin(from_fqn);
    let overhead = crate::phone::messaging::PTY_WRAP_FIXED + display_from.len();
    if pointer.len() + overhead > crate::phone::messaging::PTY_SAFE_MAX {
        // Nothing was delivered; the file was only just created by us, so it is
        // removed rather than left as a never-referenced orphan.
        let _ = std::fs::remove_file(&path);
        return Err(format!(
            "notification exceeds PTY-safe length (body {} + overhead {} > {}); shorten slug or \
                move the room to a shallower path",
            pointer.len(),
            overhead,
            crate::phone::messaging::PTY_SAFE_MAX
        ));
    }
    let archive_warning = (|| -> Result<(), String> {
        let source = std::fs::canonicalize(&path).map_err(|e| e.to_string())?;
        let snapshot = std::fs::read(&source).map_err(|e| e.to_string())?;
        let basename = source
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or("invalid message basename")?;
        let ac_root = room_root.parent().ok_or("room has no Project AC Root")?;
        crate::phone::messaging::archive_message_snapshot(ac_root, &source, basename, &snapshot)?;
        Ok(())
    })()
    .err()
    .map(|error| format!("Message archive warning: {}: {error}", path.display()));
    write_co_managed_queue_message(room_root, from_fqn, to_fqn, &pointer).map_err(|error| {
        match &archive_warning {
            Some(warning) => format!("{error}; {warning}"),
            None => error,
        }
    })?;
    handle.record_step("queue_file");
    Ok(archive_warning)
}

// ── routing decision ────────────────────────────────────────────────────────

pub(crate) enum CoManagedRoute {
    /// Text for the user, never routed. `write_candidate_file` is false only in
    /// the secret case, where phase 6 forbids any file.
    User {
        reason: String,
        write_candidate_file: bool,
    },
    /// A wake carrying a pointer. Spending budget is decided by the caller.
    Wake {
        to: String,
        content: String,
        reason: String,
    },
}

impl CoManagedRoute {
    fn kind(&self) -> capture::state::EffectKind {
        match self {
            Self::User { .. } => capture::state::EffectKind::TextToUser,
            Self::Wake { .. } => capture::state::EffectKind::Automatic,
        }
    }
}
