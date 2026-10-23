use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::Manager;
use terminal_snapshot_renderer::{
    canonical_timestamp, encode_host_failure_payload, TerminalSnapshotFormat,
    TerminalSnapshotReasonCode, MAX_REQUEST_BYTES, MAX_TRANSPORT_BYTES,
};
use uuid::Uuid;

const REQUEST_KIND: &str = "terminal-snapshot";
const REQUEST_VERSION: u32 = 1;
const REQUEST_DIRECTORY: &str = "terminal-snapshot-requests";
const RESPONSE_DIRECTORY: &str = "terminal-snapshot-responses";
const RESPONSE_TTL: Duration = Duration::from_secs(60);
const MAX_DIRECTORY_ENTRIES: usize = 4_096;
const MAX_PER_POLL: usize = 256;
const CURSOR_CAP: usize = 4_096;

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct HostTerminalSnapshotRequest {
    pub kind: String,
    pub version: u32,
    pub request_id: String,
    pub token: String,
    pub from: String,
    pub to: String,
    pub format: TerminalSnapshotFormat,
    pub issued_at: String,
    pub expires_at: String,
    pub nonce: String,
    pub confirmation_tag: String,
}

impl std::fmt::Debug for HostTerminalSnapshotRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("HostTerminalSnapshotRequest")
            .field("version", &self.version)
            .field("format", &self.format)
            .finish_non_exhaustive()
    }
}

impl HostTerminalSnapshotRequest {
    fn validate_correlation(
        &self,
        filename_request_id: &str,
    ) -> Result<Uuid, TerminalSnapshotReasonCode> {
        let request_id = terminal_snapshot_renderer::validate_uuid(&self.request_id, Some(4))
            .map_err(|_| TerminalSnapshotReasonCode::InvalidRequest)?;
        if self.request_id != filename_request_id {
            return Err(TerminalSnapshotReasonCode::InvalidRequest);
        }
        terminal_snapshot_renderer::validate_hex(&self.nonce, 64)
            .map_err(|_| TerminalSnapshotReasonCode::InvalidRequest)?;
        terminal_snapshot_renderer::validate_hex(&self.confirmation_tag, 64)
            .map_err(|_| TerminalSnapshotReasonCode::InvalidRequest)?;
        if confirmation_tag(self) != self.confirmation_tag {
            return Err(TerminalSnapshotReasonCode::InvalidRequest);
        }
        Ok(request_id)
    }

    pub(crate) fn validate(
        &self,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<(Uuid, Uuid, chrono::DateTime<chrono::Utc>), TerminalSnapshotReasonCode> {
        if self.kind != REQUEST_KIND || self.version != REQUEST_VERSION {
            return Err(TerminalSnapshotReasonCode::InvalidRequest);
        }
        let request_id = terminal_snapshot_renderer::validate_uuid(&self.request_id, Some(4))
            .map_err(|_| TerminalSnapshotReasonCode::InvalidRequest)?;
        let token = terminal_snapshot_renderer::validate_uuid(&self.token, Some(4))
            .map_err(|_| TerminalSnapshotReasonCode::InvalidRequest)?;
        terminal_snapshot_renderer::validate_requester_identity(&self.from, true)
            .map_err(|_| TerminalSnapshotReasonCode::InvalidRequest)?;
        terminal_snapshot_renderer::validate_target_syntax(&self.to)
            .map_err(|_| TerminalSnapshotReasonCode::InvalidRequest)?;
        terminal_snapshot_renderer::validate_hex(&self.nonce, 64)
            .map_err(|_| TerminalSnapshotReasonCode::InvalidRequest)?;
        terminal_snapshot_renderer::validate_hex(&self.confirmation_tag, 64)
            .map_err(|_| TerminalSnapshotReasonCode::InvalidRequest)?;
        let issued = terminal_snapshot_renderer::validate_timestamp(&self.issued_at)
            .map_err(|_| TerminalSnapshotReasonCode::InvalidRequest)?;
        let expires = terminal_snapshot_renderer::validate_timestamp(&self.expires_at)
            .map_err(|_| TerminalSnapshotReasonCode::InvalidRequest)?;
        let lifetime = expires.signed_duration_since(issued);
        let lifetime_ms = lifetime.num_milliseconds();
        if !(5_000..=30_000).contains(&lifetime_ms)
            || lifetime_ms % 1_000 != 0
            || issued > now + chrono::Duration::seconds(5)
            || now >= expires
            || confirmation_tag(self) != self.confirmation_tag
        {
            return Err(TerminalSnapshotReasonCode::InvalidRequest);
        }
        Ok((request_id, token, expires))
    }
}

pub(crate) fn confirmation_tag(request: &HostTerminalSnapshotRequest) -> String {
    use sha2::{Digest, Sha256};
    let format = request.format.to_string();
    let fields: [&[u8]; 8] = [
        b"ac-terminal-snapshot-confirmation-v1",
        request.request_id.as_bytes(),
        request.nonce.as_bytes(),
        request.from.as_bytes(),
        request.to.as_bytes(),
        format.as_bytes(),
        request.issued_at.as_bytes(),
        request.expires_at.as_bytes(),
    ];
    let mut digest = Sha256::new();
    for field in fields {
        digest.update((field.len() as u64).to_be_bytes());
        digest.update(field);
    }
    format!("{:x}", digest.finalize())
}

const SCANNER_TASK_ABORTABLE: u8 = 0;
const SCANNER_TASK_OWNED: u8 = 1;
const SCANNER_TASK_ABORTING: u8 = 2;
const SCANNER_TASK_TERMINAL: u8 = 3;
const SCANNER_TASK_FINALIZER: u8 = 4;
const SCANNER_TASK_CAP: usize = 8_192;
const SCANNER_ABORT_GRACE: std::time::Duration = std::time::Duration::from_millis(200);

type ScannerTaskCleanup = std::sync::Arc<dyn Fn() + Send + Sync + 'static>;

#[cfg(test)]
pub(crate) struct SnapshotScannerTaskStartControl {
    entered: std::sync::Mutex<bool>,
    changed: std::sync::Condvar,
    release: tokio::sync::Semaphore,
}

#[cfg(test)]
impl SnapshotScannerTaskStartControl {
    pub(crate) fn new() -> std::sync::Arc<Self> {
        std::sync::Arc::new(Self {
            entered: std::sync::Mutex::new(false),
            changed: std::sync::Condvar::new(),
            release: tokio::sync::Semaphore::new(0),
        })
    }

    pub(crate) fn wait_until_entered(&self) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        let mut entered = self.entered.lock().expect("scanner start control state");
        while !*entered {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            assert!(
                !remaining.is_zero(),
                "scanner task did not reach its pre-service barrier"
            );
            let (next, timeout) = self
                .changed
                .wait_timeout(entered, remaining)
                .expect("scanner start control wait");
            entered = next;
            assert!(
                !timeout.timed_out() || *entered,
                "scanner task did not reach its pre-service barrier"
            );
        }
    }

    pub(crate) fn release(&self) {
        self.release.add_permits(1);
    }

    async fn enter(&self) {
        {
            let mut entered = self.entered.lock().expect("scanner start control state");
            *entered = true;
            self.changed.notify_all();
        }
        if let Ok(permit) = self.release.acquire().await {
            permit.forget();
        }
    }
}

#[derive(Clone)]
struct ScannerTaskOwnership {
    phase: std::sync::Arc<std::sync::atomic::AtomicU8>,
}

impl ScannerTaskOwnership {
    fn new() -> Self {
        Self {
            phase: std::sync::Arc::new(std::sync::atomic::AtomicU8::new(SCANNER_TASK_ABORTABLE)),
        }
    }

    fn mark_owned(&self) -> bool {
        self.phase
            .compare_exchange(
                SCANNER_TASK_ABORTABLE,
                SCANNER_TASK_OWNED,
                std::sync::atomic::Ordering::AcqRel,
                std::sync::atomic::Ordering::Acquire,
            )
            .is_ok()
    }

    fn mark_finalizer(&self) {
        self.phase
            .store(SCANNER_TASK_FINALIZER, std::sync::atomic::Ordering::Release);
    }
}

struct ScannerTaskRecord {
    audit_request_id: Option<String>,
    phase: std::sync::Arc<std::sync::atomic::AtomicU8>,
    handle: tauri::async_runtime::JoinHandle<()>,
    cleanup: ScannerTaskCleanup,
}

#[derive(Default)]
struct ScannerShutdownState {
    sealed: bool,
    tasks: HashMap<Uuid, ScannerTaskRecord>,
    #[cfg(test)]
    task_start_controls:
        std::collections::VecDeque<std::sync::Arc<SnapshotScannerTaskStartControl>>,
}

#[derive(Clone)]
pub(crate) struct SnapshotScannerShutdownOwner {
    inner: std::sync::Arc<std::sync::Mutex<ScannerShutdownState>>,
}

static ACTIVE_SCANNER_SHUTDOWN_OWNER: std::sync::OnceLock<
    std::sync::Mutex<Option<SnapshotScannerShutdownOwner>>,
> = std::sync::OnceLock::new();

impl Default for SnapshotScannerShutdownOwner {
    fn default() -> Self {
        let owner = Self {
            inner: std::sync::Arc::new(std::sync::Mutex::new(ScannerShutdownState::default())),
        };
        let active = ACTIVE_SCANNER_SHUTDOWN_OWNER.get_or_init(|| std::sync::Mutex::new(None));
        match active.lock() {
            Ok(mut slot) => *slot = Some(owner.clone()),
            Err(poisoned) => *poisoned.into_inner() = Some(owner.clone()),
        }
        owner
    }
}

#[derive(Default)]
pub(crate) struct SnapshotScannerDrainResult {
    pub(crate) terminal: bool,
    pub(crate) joined: usize,
    pub(crate) aborted: usize,
    pub(crate) retained: Vec<String>,
}

impl SnapshotScannerShutdownOwner {
    pub(crate) fn active() -> Option<Self> {
        let active = ACTIVE_SCANNER_SHUTDOWN_OWNER.get()?;
        match active.lock() {
            Ok(slot) => slot.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }

    /// (#1399) An owner that does NOT claim `ACTIVE_SCANNER_SHUTDOWN_OWNER`.
    /// For scanners that will never run a task and must never displace the
    /// long-lived poller's owner on the shutdown path.
    pub(crate) fn unregistered() -> Self {
        Self {
            inner: std::sync::Arc::new(std::sync::Mutex::new(ScannerShutdownState::default())),
        }
    }

    fn lock_state(&self) -> std::sync::MutexGuard<'_, ScannerShutdownState> {
        match self.inner.lock() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    pub(crate) fn seal(&self) {
        self.lock_state().sealed = true;
    }

    pub(crate) fn is_sealed(&self) -> bool {
        self.lock_state().sealed
    }

    #[cfg(test)]
    pub(crate) fn task_count(&self) -> usize {
        self.lock_state().tasks.len()
    }

    #[cfg(test)]
    pub(crate) fn install_next_task_start_control(
        &self,
        control: std::sync::Arc<SnapshotScannerTaskStartControl>,
    ) {
        self.lock_state().task_start_controls.push_back(control);
    }

    #[cfg(test)]
    fn take_next_task_start_control(
        &self,
    ) -> Option<std::sync::Arc<SnapshotScannerTaskStartControl>> {
        self.lock_state().task_start_controls.pop_front()
    }

    #[cfg(test)]
    async fn join_all_for_test(&self) {
        let tasks = {
            let mut state = self.lock_state();
            std::mem::take(&mut state.tasks)
        };
        for (_, task) in tasks {
            task.handle.await.expect("terminal snapshot mailbox task");
            (task.cleanup)();
        }
    }

    fn spawn_tracked<F>(
        &self,
        audit_request_id: Option<String>,
        phase: std::sync::Arc<std::sync::atomic::AtomicU8>,
        cleanup: ScannerTaskCleanup,
        future: F,
    ) -> bool
    where
        F: std::future::Future<Output = ()> + Send + 'static,
    {
        let mut state = self.lock_state();
        if state.sealed || state.tasks.len() >= SCANNER_TASK_CAP {
            return false;
        }
        let task_phase = phase.clone();
        let handle = tauri::async_runtime::spawn(async move {
            future.await;
            task_phase.store(SCANNER_TASK_TERMINAL, std::sync::atomic::Ordering::Release);
        });
        state.tasks.insert(
            Uuid::new_v4(),
            ScannerTaskRecord {
                audit_request_id,
                phase,
                handle,
                cleanup,
            },
        );
        true
    }

    pub(crate) async fn seal_and_drain_until(
        &self,
        deadline: tokio::time::Instant,
    ) -> SnapshotScannerDrainResult {
        self.seal();
        let tasks = {
            let mut state = self.lock_state();
            std::mem::take(&mut state.tasks)
        };
        let mut result = SnapshotScannerDrainResult::default();
        let mut retained = HashMap::new();

        for (task_id, mut task) in tasks {
            let grace_deadline =
                std::cmp::min(deadline, tokio::time::Instant::now() + SCANNER_ABORT_GRACE);
            if let Ok(joined) = tokio::time::timeout_at(grace_deadline, &mut task.handle).await {
                if joined.is_err() {
                    log::error!("[terminal-snapshot] stage=host_task code=internal");
                }
                (task.cleanup)();
                result.joined += 1;
                continue;
            }

            if task
                .phase
                .compare_exchange(
                    SCANNER_TASK_ABORTABLE,
                    SCANNER_TASK_ABORTING,
                    std::sync::atomic::Ordering::AcqRel,
                    std::sync::atomic::Ordering::Acquire,
                )
                .is_ok()
            {
                if let Some(request_id) = &task.audit_request_id {
                    record_host_ingress_failure(
                        Some(request_id.clone()),
                        None,
                        TerminalSnapshotReasonCode::AuthorityChanged,
                    );
                }
                task.handle.abort();
                result.aborted += 1;
                match tokio::time::timeout_at(deadline, &mut task.handle).await {
                    Ok(_) => {
                        task.phase
                            .store(SCANNER_TASK_TERMINAL, std::sync::atomic::Ordering::Release);
                        (task.cleanup)();
                        result.joined += 1;
                    }
                    Err(_) => {
                        result
                            .retained
                            .push("reason=terminal-snapshot-aborting state=retained".to_string());
                        retained.insert(task_id, task);
                    }
                }
                continue;
            }

            match tokio::time::timeout_at(deadline, &mut task.handle).await {
                Ok(joined) => {
                    if joined.is_err() {
                        log::error!("[terminal-snapshot] stage=host_task code=internal");
                    }
                    (task.cleanup)();
                    result.joined += 1;
                }
                Err(_) => {
                    let reason = if task.phase.load(std::sync::atomic::Ordering::Acquire)
                        == SCANNER_TASK_FINALIZER
                    {
                        "terminal-snapshot-finalizer"
                    } else {
                        "terminal-snapshot-service"
                    };
                    result
                        .retained
                        .push(format!("reason={reason} state=retained"));
                    retained.insert(task_id, task);
                }
            }
        }

        result.terminal = retained.is_empty();
        if !retained.is_empty() {
            self.lock_state().tasks.extend(retained);
        }
        result
    }
}

#[cfg(test)]
mod scanner_shutdown_tests;

#[derive(Default)]
pub(crate) struct SnapshotMailboxScanner {
    shutdown_owner: SnapshotScannerShutdownOwner,
    cursors: HashMap<crate::path_identity::FileObjectId, String>,
    observed: std::collections::HashSet<crate::path_identity::FileObjectId>,
    startup_sweep_complete: bool,
    #[cfg(test)]
    pending_tasks: Vec<tauri::async_runtime::JoinHandle<()>>,
}

pub(crate) fn verified_requester_root_from_discovered_path(path: &Path) -> Option<PathBuf> {
    if let Ok(root) = crate::config::root_agent::verify_live_root_agent_path(path) {
        return Some(root.canonical_path);
    }
    crate::config::teams::verify_pty_input_replica_cwd(path)
        .ok()
        .map(|identity| identity.replica_root)
}

impl SnapshotMailboxScanner {
    /// (#1399) For a delivery-only poller. Reuses whatever owner is currently
    /// registered, so no second owner is created, and falls back to a
    /// non-registering owner rather than to `Default`, whose hand-written impl
    /// writes itself into the process-global `ACTIVE_SCANNER_SHUTDOWN_OWNER`
    /// slot and would leave the long-lived poller's real owner unreachable on
    /// the shutdown drain.
    ///
    /// (R8) Deliberately "whatever is currently registered" and not "the live
    /// owner": `active()` returns whatever registered LAST, and while #1403 is
    /// open that is an `api::dispatcher` throwaway roughly twice a second, not
    /// the long-lived poller's owner. Harmless, because a delivery poller never
    /// spawns a scanner task and so never uses the owner it holds.
    ///
    /// Fields are spelled out, never `..Default::default()`: struct-update
    /// syntax would evaluate the registering `Default` for `shutdown_owner`
    /// and hijack the global before the explicit field overwrote it. The
    /// `unwrap_or_else(unregistered)` arm exists on the structural argument
    /// (registration must be impossible on every path), not because it is
    /// commonly reached.
    pub(crate) fn delivery_only() -> Self {
        Self {
            shutdown_owner: SnapshotScannerShutdownOwner::active()
                .unwrap_or_else(SnapshotScannerShutdownOwner::unregistered),
            cursors: HashMap::new(),
            observed: std::collections::HashSet::new(),
            startup_sweep_complete: false,
            #[cfg(test)]
            pending_tasks: Vec::new(),
        }
    }

    pub(crate) fn begin_cycle(&mut self) {
        self.observed.clear();
    }

    pub(crate) fn startup_sweep_pending(&self) -> bool {
        !self.startup_sweep_complete
    }

    pub(crate) fn finish_startup_sweep(&mut self) {
        self.startup_sweep_complete = true;
    }

    pub(crate) fn startup_sweep_root(
        &mut self,
        state: &crate::pty::terminal_snapshot::TerminalSnapshotState,
        requester_root: &Path,
    ) {
        let Ok(root_identity) = crate::path_identity::verify_directory(requester_root) else {
            return;
        };
        let local = requester_root.join(crate::config::agent_local_dir_name());
        let outbox = local.join("outbox");
        let directories = [
            (outbox.join(REQUEST_DIRECTORY), false),
            (local.join(RESPONSE_DIRECTORY), true),
        ];
        for (directory, response_directory) in directories {
            let Ok(directory_identity) = crate::path_identity::verify_directory(&directory) else {
                continue;
            };
            if !private_directory_mode(&directory)
                || !crate::path_identity::is_verified_descendant(
                    &directory_identity,
                    &root_identity,
                )
            {
                continue;
            }
            let Ok(entries) = std::fs::read_dir(&directory) else {
                continue;
            };
            for (index, entry) in entries.enumerate() {
                if index >= MAX_DIRECTORY_ENTRIES {
                    log::warn!("[terminal-snapshot] stage=cleanup code=directory_capacity");
                    break;
                }
                let Ok(entry) = entry else {
                    break;
                };
                let path = entry.path();
                let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
                    continue;
                };
                if !protocol_cleanup_name(name, response_directory)
                    || !private_regular_file_mode(&path)
                {
                    continue;
                }
                let Some(age) = std::fs::symlink_metadata(&path)
                    .ok()
                    .filter(|metadata| metadata.is_file())
                    .and_then(|metadata| metadata.modified().ok())
                    .and_then(|modified| modified.elapsed().ok())
                else {
                    continue;
                };
                let Ok(identity) = crate::path_identity::verify_regular_file(&path) else {
                    continue;
                };
                if age >= RESPONSE_TTL {
                    safe_remove(&path, &identity);
                    continue;
                }
                let Ok(reservation) = state.reserve_existing_artifact(
                    &directory,
                    &directory_identity,
                    identity.object_id,
                ) else {
                    continue;
                };
                let ttl = RESPONSE_TTL.saturating_sub(age);
                let _ = reservation.commit_with_ttl(path, identity, ttl);
            }
        }
    }

    pub(crate) fn finish_cycle(&mut self) {
        self.cursors
            .retain(|object, _| self.observed.contains(object));
    }

    #[cfg(test)]
    pub(crate) fn shutdown_owner_for_test(&self) -> SnapshotScannerShutdownOwner {
        self.shutdown_owner.clone()
    }

    #[cfg(test)]
    pub(crate) async fn join_pending_tasks_for_test(&mut self) {
        self.shutdown_owner.join_all_for_test().await;
        for task in std::mem::take(&mut self.pending_tasks) {
            task.await.expect("terminal snapshot mailbox task");
        }
    }

    pub(crate) fn scan_root<R: tauri::Runtime>(
        &mut self,
        app: &tauri::AppHandle<R>,
        requester_root: &Path,
    ) {
        let Ok(root_identity) = crate::path_identity::verify_directory(requester_root) else {
            return;
        };
        let local = requester_root.join(crate::config::agent_local_dir_name());
        let outbox = local.join("outbox");
        let request_directory = outbox.join(REQUEST_DIRECTORY);
        let response_directory = local.join(RESPONSE_DIRECTORY);
        if !request_directory.is_dir() || !response_directory.is_dir() {
            return;
        }
        let (
            Ok(local_identity),
            Ok(outbox_identity),
            Ok(request_directory_identity),
            Ok(response_directory_identity),
        ) = (
            crate::path_identity::verify_directory(&local),
            crate::path_identity::verify_directory(&outbox),
            crate::path_identity::verify_directory(&request_directory),
            crate::path_identity::verify_directory(&response_directory),
        )
        else {
            return;
        };
        if !private_directory_mode(&request_directory)
            || !private_directory_mode(&response_directory)
            || !crate::path_identity::is_verified_descendant(&local_identity, &root_identity)
            || !crate::path_identity::is_verified_descendant(&outbox_identity, &local_identity)
            || !crate::path_identity::is_verified_descendant(
                &request_directory_identity,
                &outbox_identity,
            )
            || !crate::path_identity::is_verified_descendant(
                &response_directory_identity,
                &local_identity,
            )
        {
            return;
        }
        if self.cursors.len() >= CURSOR_CAP
            && !self
                .cursors
                .contains_key(&request_directory_identity.object_id)
        {
            return;
        }
        self.observed.insert(request_directory_identity.object_id);
        sweep_directory(&request_directory, &request_directory_identity, false);
        sweep_directory(&response_directory, &response_directory_identity, true);
        let cursor = self
            .cursors
            .get(&request_directory_identity.object_id)
            .cloned()
            .unwrap_or_default();
        let mut next = std::collections::BTreeSet::new();
        let mut wrapped = std::collections::BTreeSet::new();
        let Ok(entries) = std::fs::read_dir(&request_directory) else {
            return;
        };
        for (index, entry) in entries.enumerate() {
            if index >= MAX_DIRECTORY_ENTRIES {
                log::warn!("[terminal-snapshot] stage=publish code=directory_capacity");
                return;
            }
            let Ok(entry) = entry else {
                return;
            };
            let Ok(name) = entry.file_name().into_string() else {
                continue;
            };
            if protocol_request_name(&name).is_none() {
                continue;
            }
            let candidates = if name.as_str() > cursor.as_str() {
                &mut next
            } else {
                &mut wrapped
            };
            candidates.insert(name);
            if candidates.len() > MAX_PER_POLL {
                candidates.pop_last();
            }
        }
        let selected: Vec<String> = if next.is_empty() {
            wrapped.into_iter().collect()
        } else {
            next.into_iter().collect()
        };
        if let Some(last) = selected.last() {
            self.cursors
                .insert(request_directory_identity.object_id, last.clone());
        }

        let Some(snapshot_state) =
            app.try_state::<Arc<crate::pty::terminal_snapshot::TerminalSnapshotState>>()
        else {
            return;
        };
        for name in selected {
            if self.shutdown_owner.is_sealed() {
                break;
            }
            let source_key = format!(
                "host:{:016x}:{:016x}",
                request_directory_identity.object_id.volume,
                request_directory_identity.object_id.file
            );
            let Ok(ingress) = snapshot_state.try_admit_ingress(source_key) else {
                break;
            };
            let request_id = protocol_request_name(&name).unwrap_or_default().to_string();
            let request_path = request_directory.join(&name);
            let processing = request_directory.join(format!(
                ".{}.{}.terminal-snapshot-processing",
                request_id,
                Uuid::new_v4()
            ));
            if !private_regular_file_mode(&request_path) {
                continue;
            }
            let Ok(original) = crate::path_identity::verify_regular_file(&request_path) else {
                continue;
            };
            let Ok(reservation) = snapshot_state.reserve_existing_artifact(
                &request_directory,
                &request_directory_identity,
                original.object_id,
            ) else {
                break;
            };
            if crate::path_identity::publish_new_file_atomic(&request_path, &processing).is_err() {
                continue;
            }
            let Ok(claimed) = crate::path_identity::verify_regular_file(&processing) else {
                continue;
            };
            if !crate::path_identity::same_object(&original, &claimed) {
                continue;
            }
            if reservation
                .commit(processing.clone(), claimed.clone())
                .is_err()
            {
                safe_remove(&processing, &claimed);
                continue;
            }
            let snapshot_state = snapshot_state.inner().clone();
            let cleanup_state = Arc::clone(&snapshot_state);
            let cleanup_processing = processing.clone();
            let cleanup_claimed = claimed.clone();
            let cleanup_request_directory = request_directory.clone();
            let cleanup_request_identity = request_directory_identity.clone();
            let cleanup_response_directory = response_directory.clone();
            let cleanup_response_identity = response_directory_identity.clone();
            let cleanup: ScannerTaskCleanup = std::sync::Arc::new(move || {
                cleanup_tracked_artifact(&cleanup_state, &cleanup_processing, &cleanup_claimed);
                sweep_directory(&cleanup_request_directory, &cleanup_request_identity, false);
                sweep_directory(
                    &cleanup_response_directory,
                    &cleanup_response_identity,
                    true,
                );
            });
            let app = app.clone();
            let response_directory = response_directory.clone();
            let expected_root = root_identity.clone();
            let ownership = ScannerTaskOwnership::new();
            let phase = ownership.phase.clone();
            let audit_request_id = request_id.clone();
            #[cfg(test)]
            let task_start_control = self.shutdown_owner.take_next_task_start_control();
            let future = async move {
                #[cfg(test)]
                if let Some(control) = task_start_control {
                    control.enter().await;
                }
                let processed = crate::logging::catch_payload_future(process_claimed(
                    ownership,
                    &app,
                    snapshot_state,
                    processing,
                    response_directory,
                    expected_root,
                    claimed,
                    request_id,
                    ingress,
                ))
                .await;
                if processed.is_err() {
                    log::error!("[terminal-snapshot] stage=host_task code=internal");
                }
            };
            if !self.shutdown_owner.spawn_tracked(
                Some(audit_request_id.clone()),
                phase,
                cleanup.clone(),
                future,
            ) {
                let reason = if self.shutdown_owner.is_sealed() {
                    TerminalSnapshotReasonCode::AuthorityChanged
                } else {
                    TerminalSnapshotReasonCode::RateLimited
                };
                record_host_ingress_failure(Some(audit_request_id), None, reason);
                cleanup();
                break;
            }
        }
    }
}

fn private_directory_mode(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::symlink_metadata(path)
            .ok()
            .is_some_and(|metadata| metadata.permissions().mode() & 0o777 == 0o700)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        true
    }
}

fn private_regular_file_mode(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::symlink_metadata(path)
            .ok()
            .is_some_and(|metadata| metadata.permissions().mode() & 0o777 == 0o600)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        true
    }
}

fn protocol_request_name(name: &str) -> Option<&str> {
    let id = name.strip_suffix(".json")?;
    terminal_snapshot_renderer::validate_uuid(id, Some(4)).ok()?;
    Some(id)
}

#[allow(clippy::too_many_arguments)]
async fn process_claimed<R: tauri::Runtime>(
    scanner_ownership: ScannerTaskOwnership,
    app: &tauri::AppHandle<R>,
    snapshot_state: Arc<crate::pty::terminal_snapshot::TerminalSnapshotState>,
    processing: PathBuf,
    response_directory: PathBuf,
    expected_root: crate::path_identity::VerifiedPathIdentity,
    claimed: crate::path_identity::VerifiedPathIdentity,
    filename_request_id: String,
    ingress: tokio::sync::OwnedSemaphorePermit,
) {
    if !scanner_ownership.mark_owned() {
        return;
    }
    #[cfg(test)]
    snapshot_state.run_host_cancellation_hook(
        crate::pty::terminal_snapshot::TerminalSnapshotHostCancellationStage::Processing,
    );
    let read = crate::path_identity::read_bounded_regular(&processing, MAX_REQUEST_BYTES);
    let (bytes, identity) = match read {
        Ok(value) if crate::path_identity::same_object(&claimed, &value.1) => value,
        Ok(_) => {
            record_host_ingress_failure(
                Some(filename_request_id.clone()),
                None,
                TerminalSnapshotReasonCode::ResponseUnavailable,
            );
            return;
        }
        Err(_) => {
            remove_tracked_processing(&snapshot_state, &processing, &claimed);
            record_host_ingress_failure(
                Some(filename_request_id.clone()),
                None,
                TerminalSnapshotReasonCode::InvalidRequest,
            );
            return;
        }
    };
    let request: HostTerminalSnapshotRequest =
        match terminal_snapshot_renderer::decode_bounded(&bytes, MAX_REQUEST_BYTES) {
            Ok(request) => request,
            Err(_) => {
                remove_tracked_processing(&snapshot_state, &processing, &identity);
                record_host_ingress_failure(
                    Some(filename_request_id.clone()),
                    None,
                    TerminalSnapshotReasonCode::InvalidRequest,
                );
                return;
            }
        };
    remove_tracked_processing(&snapshot_state, &processing, &identity);
    let request_id = match request.validate_correlation(&filename_request_id) {
        Ok(request_id) => request_id,
        Err(reason) => {
            record_host_ingress_failure(Some(filename_request_id), None, reason);
            return;
        }
    };
    let (_, token, expires_at) = match request.validate(chrono::Utc::now()) {
        Ok(validated) => validated,
        Err(reason) => {
            publish_correlated_ingress_failure(
                &snapshot_state,
                &response_directory,
                request_id,
                &request.confirmation_tag,
                Some(request.format),
                reason,
            );
            return;
        }
    };
    let publish_error = |reason| {
        publish_correlated_ingress_failure(
            &snapshot_state,
            &response_directory,
            request_id,
            &request.confirmation_tag,
            Some(request.format),
            reason,
        );
    };
    let Some(session_manager) =
        app.try_state::<Arc<tokio::sync::RwLock<crate::session::manager::SessionManager>>>()
    else {
        publish_error(TerminalSnapshotReasonCode::ServiceUnavailable);
        return;
    };
    let Some(pty_manager) =
        app.try_state::<Arc<std::sync::Mutex<crate::pty::manager::PtyManager>>>()
    else {
        publish_error(TerminalSnapshotReasonCode::ServiceUnavailable);
        return;
    };
    let Some(settings) = app.try_state::<crate::config::settings::SettingsState>() else {
        publish_error(TerminalSnapshotReasonCode::ServiceUnavailable);
        return;
    };
    let Some(restore) = app.try_state::<Arc<crate::RestoreInProgress>>() else {
        publish_error(TerminalSnapshotReasonCode::ServiceUnavailable);
        return;
    };
    let Some(purge) = app.try_state::<Arc<crate::session::purge_guard::PurgeGuard>>() else {
        publish_error(TerminalSnapshotReasonCode::ServiceUnavailable);
        return;
    };
    let remaining = match expires_at
        .signed_duration_since(chrono::Utc::now())
        .to_std()
        .ok()
        .filter(|remaining| !remaining.is_zero())
    {
        Some(remaining) => remaining,
        None => {
            publish_error(TerminalSnapshotReasonCode::SnapshotTimeout);
            return;
        }
    };
    let Some(monotonic_deadline) = std::time::Instant::now().checked_add(remaining) else {
        publish_error(TerminalSnapshotReasonCode::Internal);
        return;
    };
    let context = crate::pty::terminal_snapshot::TerminalSnapshotServiceContext {
        session_manager: session_manager.inner().clone(),
        pty_manager: pty_manager.inner().clone(),
        settings: settings.inner().clone(),
        restore: restore.inner().clone(),
        purge: purge.inner().clone(),
    };
    let service_request = crate::pty::terminal_snapshot::TerminalSnapshotServiceRequest {
        request_id,
        target: request.to.clone(),
        format: request.format,
        source_plane: crate::pty::terminal_snapshot::TerminalSnapshotSourcePlane::HostCli,
        host_authorization_deadline: Some((monotonic_deadline, expires_at)),
    };
    let audit = crate::pty::terminal_snapshot::TerminalSnapshotAuditGuard::pre_admission(
        crate::pty::terminal_snapshot::TerminalSnapshotSourcePlane::HostCli,
    );
    audit.accept_request(&service_request);
    let admission = snapshot_state
        .pre_admit_requester(
            &context,
            crate::pty::terminal_snapshot::TerminalSnapshotRequesterSelector::Host {
                token,
                expected_root,
                claimed_from: request.from.clone(),
            },
            crate::pty::terminal_snapshot::TerminalSnapshotSourcePlane::HostCli,
            service_request.host_authorization_deadline,
            audit.clone(),
        )
        .await;
    let admission = match admission {
        Ok(admission) => admission,
        Err(reason) => {
            drop(ingress);
            audit.wait_for_retained_owners(1).await;
            let published = publish_trusted_failure(
                &snapshot_state,
                &response_directory,
                request_id,
                &request.confirmation_tag,
                reason,
            );
            audit.finalize_failure(if published.is_ok() {
                reason
            } else {
                TerminalSnapshotReasonCode::ResponseUnavailable
            });
            return;
        }
    };
    drop(ingress);
    let prepared = snapshot_state
        .prepare_with_admission(admission, service_request)
        .await;
    let prepared = match prepared {
        Ok(prepared) => prepared,
        Err(reason) => {
            audit.wait_for_retained_owners(1).await;
            let published = publish_trusted_failure(
                &snapshot_state,
                &response_directory,
                request_id,
                &request.confirmation_tag,
                reason,
            );
            audit.finalize_failure(if published.is_ok() {
                reason
            } else {
                TerminalSnapshotReasonCode::ResponseUnavailable
            });
            return;
        }
    };
    let (payload, finalization) = prepared.into_parts();
    let response_expires_at =
        canonical_timestamp(chrono::Utc::now() + chrono::Duration::seconds(60));
    let success_bytes = finalization
        .build_host_response(
            payload,
            request_id.to_string(),
            request.confirmation_tag.clone(),
            response_expires_at,
        )
        .await;
    audit.wait_for_retained_owners(2).await;
    #[cfg(test)]
    snapshot_state.run_host_cancellation_hook(
        crate::pty::terminal_snapshot::TerminalSnapshotHostCancellationStage::ResponseBytesReady,
    );
    let response_directory_for_publish = response_directory.clone();
    let state_for_publish = Arc::clone(&snapshot_state);
    let confirmation_tag = request.confirmation_tag.clone();
    scanner_ownership.mark_finalizer();
    let task = match success_bytes {
        Ok(success_bytes) => tokio::task::spawn_blocking(move || {
            crate::logging::catch_payload_unwind(|| {
                finalization.finalize_host(success_bytes, |outcome| {
                    publish_host_outcome(
                        &state_for_publish,
                        &response_directory_for_publish,
                        request_id,
                        &confirmation_tag,
                        outcome,
                    )
                })
            })
        }),
        Err(reason) => tokio::task::spawn_blocking(move || {
            crate::logging::catch_payload_unwind(|| {
                finalization.fail_host(reason, |reason| {
                    publish_trusted_failure(
                        &state_for_publish,
                        &response_directory_for_publish,
                        request_id,
                        &confirmation_tag,
                        reason,
                    )
                })
            })
        }),
    };
    match crate::logging::collapse_payload_task(task.await) {
        Ok(_) => {}
        Err(_) => {
            log::error!("[terminal-snapshot] stage=host_finalizer_task code=internal");
        }
    }
}

fn record_host_ingress_failure(
    request_id: Option<String>,
    format: Option<TerminalSnapshotFormat>,
    reason: TerminalSnapshotReasonCode,
) {
    let status = match reason {
        TerminalSnapshotReasonCode::InvalidRequest
        | TerminalSnapshotReasonCode::RequesterUnavailable
        | TerminalSnapshotReasonCode::TerminalSnapshotsDisabled
        | TerminalSnapshotReasonCode::NotAuthorized => "rejected",
        _ => "failed",
    };
    crate::api::audit::record_terminal_snapshot(
        &crate::api::audit::TerminalSnapshotAuditMetadata {
            event: "terminal_snapshot".to_string(),
            request_id,
            requester_fqn: None,
            target_fqn: None,
            source_plane: "host_cli".to_string(),
            format: format.map(|format| format.to_string()),
            selected_session_id: None,
            selected_backend: None,
            rows: None,
            columns: None,
            sequence: None,
            captured_at: None,
            payload_bytes: None,
            accepted_at: None,
            completed_at: canonical_timestamp(chrono::Utc::now()),
            status: status.to_string(),
            reason_code: Some(reason.as_str().to_string()),
        },
    );
}

fn publish_correlated_ingress_failure(
    state: &crate::pty::terminal_snapshot::TerminalSnapshotState,
    response_directory: &Path,
    request_id: Uuid,
    confirmation_tag: &str,
    format: Option<TerminalSnapshotFormat>,
    reason: TerminalSnapshotReasonCode,
) {
    let published = publish_trusted_failure(
        state,
        response_directory,
        request_id,
        confirmation_tag,
        reason,
    );
    record_host_ingress_failure(
        Some(request_id.to_string()),
        format,
        if published.is_ok() {
            reason
        } else {
            TerminalSnapshotReasonCode::ResponseUnavailable
        },
    );
}

fn publish_host_outcome(
    state: &crate::pty::terminal_snapshot::TerminalSnapshotState,
    response_directory: &Path,
    request_id: Uuid,
    confirmation_tag: &str,
    outcome: Result<Vec<u8>, TerminalSnapshotReasonCode>,
) -> Result<(), TerminalSnapshotReasonCode> {
    #[cfg(test)]
    state.run_host_cancellation_hook(
        crate::pty::terminal_snapshot::TerminalSnapshotHostCancellationStage::BeforePublish,
    );
    match outcome {
        Ok(bytes) => publish_response_bytes(state, response_directory, request_id, &bytes),
        Err(reason) => publish_trusted_failure(
            state,
            response_directory,
            request_id,
            confirmation_tag,
            reason,
        ),
    }
}

fn publish_trusted_failure(
    state: &crate::pty::terminal_snapshot::TerminalSnapshotState,
    response_directory: &Path,
    request_id: Uuid,
    confirmation_tag: &str,
    reason: TerminalSnapshotReasonCode,
) -> Result<(), TerminalSnapshotReasonCode> {
    let expires_at = canonical_timestamp(chrono::Utc::now() + chrono::Duration::seconds(60));
    let bytes = encode_host_failure_payload(
        &request_id.to_string(),
        confirmation_tag,
        &expires_at,
        reason,
    )
    .map_err(|_| TerminalSnapshotReasonCode::ResponseUnavailable)?;
    publish_response_bytes(state, response_directory, request_id, &bytes)
}

fn publish_response_bytes(
    state: &crate::pty::terminal_snapshot::TerminalSnapshotState,
    response_directory: &Path,
    request_id: Uuid,
    bytes: &[u8],
) -> Result<(), TerminalSnapshotReasonCode> {
    if bytes.is_empty() || bytes.len() > MAX_TRANSPORT_BYTES {
        return Err(TerminalSnapshotReasonCode::ResponseUnavailable);
    }
    let validated_directory = crate::path_identity::verify_directory(response_directory)
        .map_err(|_| TerminalSnapshotReasonCode::ResponseUnavailable)?;
    #[cfg(test)]
    state.run_response_publication_hook(
        crate::pty::terminal_snapshot::TerminalSnapshotResponsePublicationStage::BeforeDirectoryRetention,
        response_directory,
        response_directory,
    );
    let retained_directory = crate::path_identity::retain_directory(response_directory)
        .map_err(|_| TerminalSnapshotReasonCode::ResponseUnavailable)?;
    if !crate::path_identity::same_object(&validated_directory, retained_directory.identity()) {
        return Err(TerminalSnapshotReasonCode::ResponseUnavailable);
    }
    let directory_identity = retained_directory.identity().clone();
    let reservation = state
        .reserve_artifact(response_directory, &directory_identity)
        .map_err(|_| TerminalSnapshotReasonCode::ResponseUnavailable)?;
    let temporary = response_directory.join(format!(
        ".{}.{}.terminal-snapshot-response-tmp",
        request_id,
        Uuid::new_v4()
    ));
    let destination = response_directory.join(format!("{request_id}.json"));
    if destination.parent() != Some(response_directory) || destination.exists() {
        return Err(TerminalSnapshotReasonCode::ResponseUnavailable);
    }
    #[cfg(test)]
    state.run_response_publication_hook(
        crate::pty::terminal_snapshot::TerminalSnapshotResponsePublicationStage::BeforeTemporaryCreate,
        &temporary,
        &destination,
    );
    let mut file = retained_directory
        .create_new_private_file(&temporary)
        .map_err(|_| TerminalSnapshotReasonCode::ResponseUnavailable)?;
    let temporary_identity = retained_directory
        .verify_opened_regular_file(&temporary, &file, true)
        .map_err(|_| TerminalSnapshotReasonCode::ResponseUnavailable)?;
    #[cfg(test)]
    state.run_response_publication_hook(
        crate::pty::terminal_snapshot::TerminalSnapshotResponsePublicationStage::BeforeTemporaryCommit,
        &temporary,
        &destination,
    );
    if reservation
        .commit(temporary.clone(), temporary_identity.clone())
        .is_err()
    {
        retained_directory.remove_regular_file_if_same(&temporary, &temporary_identity);
        return Err(TerminalSnapshotReasonCode::ResponseUnavailable);
    }
    #[cfg(test)]
    state.run_response_publication_hook(
        crate::pty::terminal_snapshot::TerminalSnapshotResponsePublicationStage::AfterTemporaryCommit,
        &temporary,
        &destination,
    );
    if retained_directory.verify_current().is_err() {
        cleanup_tracked_artifact(state, &temporary, &temporary_identity);
        return Err(TerminalSnapshotReasonCode::ResponseUnavailable);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if file
            .set_permissions(std::fs::Permissions::from_mode(0o600))
            .is_err()
            || file
                .metadata()
                .ok()
                .is_none_or(|metadata| metadata.permissions().mode() & 0o777 != 0o600)
        {
            cleanup_tracked_artifact(state, &temporary, &temporary_identity);
            return Err(TerminalSnapshotReasonCode::ResponseUnavailable);
        }
    }
    if file.write_all(bytes).and_then(|_| file.flush()).is_err() {
        cleanup_tracked_artifact(state, &temporary, &temporary_identity);
        return Err(TerminalSnapshotReasonCode::ResponseUnavailable);
    }
    #[cfg(test)]
    let file_sync_failed = state.take_response_publication_failure(
        crate::pty::terminal_snapshot::TerminalSnapshotResponsePublicationFailure::FileSync,
    );
    #[cfg(not(test))]
    let file_sync_failed = false;
    if file_sync_failed || file.sync_all().is_err() {
        cleanup_tracked_artifact(state, &temporary, &temporary_identity);
        return Err(TerminalSnapshotReasonCode::ResponseUnavailable);
    }
    #[cfg(test)]
    state.run_response_publication_hook(
        crate::pty::terminal_snapshot::TerminalSnapshotResponsePublicationStage::AfterTemporaryWrite,
        &temporary,
        &destination,
    );
    let written_identity = match retained_directory
        .verify_current()
        .and_then(|_| retained_directory.verify_opened_regular_file(&temporary, &file, false))
    {
        Ok(identity)
            if crate::path_identity::same_object(&identity, &temporary_identity)
                && crate::path_identity::is_verified_descendant(&identity, &directory_identity) =>
        {
            identity
        }
        _ => {
            cleanup_tracked_artifact(state, &temporary, &temporary_identity);
            return Err(TerminalSnapshotReasonCode::ResponseUnavailable);
        }
    };
    #[cfg(test)]
    state.run_response_publication_hook(
        crate::pty::terminal_snapshot::TerminalSnapshotResponsePublicationStage::BeforeAtomicRename,
        &temporary,
        &destination,
    );
    if retained_directory
        .publish_new_file_atomic(&temporary, &destination)
        .is_err()
    {
        cleanup_tracked_artifact(state, &temporary, &written_identity);
        return Err(TerminalSnapshotReasonCode::ResponseUnavailable);
    }
    #[cfg(test)]
    state.run_response_publication_hook(
        crate::pty::terminal_snapshot::TerminalSnapshotResponsePublicationStage::AfterAtomicRename,
        &temporary,
        &destination,
    );
    let mut published = reconcile_published_response(
        state,
        &retained_directory,
        &destination,
        &file,
        &written_identity,
        &directory_identity,
    )?;
    #[cfg(test)]
    state.run_response_publication_hook(
        crate::pty::terminal_snapshot::TerminalSnapshotResponsePublicationStage::AfterFinalVerification,
        &temporary,
        &destination,
    );
    if published.is_some() {
        published = reconcile_published_response(
            state,
            &retained_directory,
            &destination,
            &file,
            &written_identity,
            &directory_identity,
        )?;
    }
    if published.is_some() {
        published = match state.relocate_artifact(&written_identity, &destination) {
            Ok(published) => published,
            Err(_) => {
                #[cfg(not(unix))]
                cleanup_tracked_artifact(state, &destination, &written_identity);
                return Err(TerminalSnapshotReasonCode::ResponseUnavailable);
            }
        };
    }
    #[cfg(test)]
    state.run_response_publication_hook(
        crate::pty::terminal_snapshot::TerminalSnapshotResponsePublicationStage::AfterRegistryCommit,
        &temporary,
        &destination,
    );
    if published.is_some() {
        published = reconcile_published_response(
            state,
            &retained_directory,
            &destination,
            &file,
            &written_identity,
            &directory_identity,
        )?;
    }
    #[cfg(test)]
    let parent_sync_failed = state.take_response_publication_failure(
        crate::pty::terminal_snapshot::TerminalSnapshotResponsePublicationFailure::ParentSync,
    );
    #[cfg(not(test))]
    let parent_sync_failed = false;
    if !parent_sync_failed {
        retained_directory.sync_best_effort();
    }
    #[cfg(test)]
    state.run_response_publication_hook(
        crate::pty::terminal_snapshot::TerminalSnapshotResponsePublicationStage::AfterParentSync,
        &temporary,
        &destination,
    );
    if published.is_some() {
        let _ = reconcile_published_response(
            state,
            &retained_directory,
            &destination,
            &file,
            &written_identity,
            &directory_identity,
        )?;
    }
    Ok(())
}

fn reconcile_published_response(
    state: &crate::pty::terminal_snapshot::TerminalSnapshotState,
    retained_directory: &crate::path_identity::RetainedDirectory,
    destination: &Path,
    file: &std::fs::File,
    expected: &crate::path_identity::VerifiedPathIdentity,
    directory: &crate::path_identity::VerifiedPathIdentity,
) -> Result<Option<crate::path_identity::VerifiedPathIdentity>, TerminalSnapshotReasonCode> {
    match retained_directory
        .verify_current()
        .and_then(|_| retained_directory.verify_opened_regular_file(destination, file, false))
    {
        Ok(identity)
            if crate::path_identity::same_object(expected, &identity)
                && crate::path_identity::is_verified_descendant(&identity, directory) =>
        {
            Ok(Some(identity))
        }
        #[cfg(unix)]
        Err(_) => {
            if cleanup_tracked_artifact(state, destination, expected)
                == crate::pty::terminal_snapshot::TerminalSnapshotArtifactCleanupOutcome::AlreadyAbsent
            {
                Ok(None)
            } else {
                Err(TerminalSnapshotReasonCode::ResponseUnavailable)
            }
        }
        #[cfg(not(unix))]
        Err(_)
            if retained_directory.child_is_absent(destination)
                || crate::path_identity::opened_file_is_delete_pending(file) =>
        {
            state.untrack_artifact(expected);
            Ok(None)
        }
        _ => {
            cleanup_tracked_artifact(state, destination, expected);
            Err(TerminalSnapshotReasonCode::ResponseUnavailable)
        }
    }
}

fn remove_tracked_processing(
    state: &crate::pty::terminal_snapshot::TerminalSnapshotState,
    path: &Path,
    expected: &crate::path_identity::VerifiedPathIdentity,
) {
    cleanup_tracked_artifact(state, path, expected);
}

fn cleanup_tracked_artifact(
    state: &crate::pty::terminal_snapshot::TerminalSnapshotState,
    path: &Path,
    expected: &crate::path_identity::VerifiedPathIdentity,
) -> crate::pty::terminal_snapshot::TerminalSnapshotArtifactCleanupOutcome {
    state.cleanup_artifact(path, expected)
}

#[cfg(test)]
fn path_is_absent(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
}

fn safe_remove(path: &Path, expected: &crate::path_identity::VerifiedPathIdentity) -> bool {
    crate::path_identity::remove_regular_file_if_same(path, expected)
}

fn sweep_directory(
    directory: &Path,
    expected_directory: &crate::path_identity::VerifiedPathIdentity,
    response_directory: bool,
) {
    let Ok(retained_directory) = crate::path_identity::retain_directory(directory) else {
        return;
    };
    if !crate::path_identity::same_object(retained_directory.identity(), expected_directory) {
        return;
    }
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in entries.take(MAX_DIRECTORY_ENTRIES).filter_map(Result::ok) {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
            continue;
        };
        if !protocol_cleanup_name(name, response_directory) {
            continue;
        }
        let old = std::fs::symlink_metadata(&path)
            .ok()
            .filter(|metadata| metadata.is_file())
            .and_then(|metadata| metadata.modified().ok())
            .and_then(|modified| modified.elapsed().ok())
            .is_some_and(|age| age >= RESPONSE_TTL);
        if !old {
            continue;
        }
        let Ok(identity) = crate::path_identity::verify_regular_file(&path) else {
            continue;
        };
        safe_remove(&path, &identity);
    }
    let _ = retained_directory.verify_current();
}

fn protocol_cleanup_name(name: &str, response_directory: bool) -> bool {
    if protocol_request_name(name).is_some() {
        return true;
    }
    let Some(name) = name.strip_prefix('.') else {
        return false;
    };
    let suffixes: &[(&str, bool)] = if response_directory {
        &[(".terminal-snapshot-response-tmp", false)]
    } else {
        &[
            (".terminal-snapshot-request-tmp", true),
            (".terminal-snapshot-processing", false),
            (".terminal-snapshot-cancelled", true),
        ]
    };
    for (suffix, nonce_is_hex) in suffixes {
        let Some(prefix) = name.strip_suffix(suffix) else {
            continue;
        };
        let Some((request_id, nonce)) = prefix.split_once('.') else {
            return false;
        };
        if terminal_snapshot_renderer::validate_uuid(request_id, Some(4)).is_err() {
            return false;
        }
        return if *nonce_is_hex {
            terminal_snapshot_renderer::validate_hex(nonce, 64).is_ok()
        } else {
            terminal_snapshot_renderer::validate_uuid(nonce, Some(4)).is_ok()
        };
    }
    false
}

#[cfg(test)]
mod tests;
