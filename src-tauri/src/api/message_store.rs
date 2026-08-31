//! Durable DB-backed send queue for the control-plane API.

use std::collections::{HashMap, HashSet};
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{DateTime, Utc};
use rusqlite::{params, OptionalExtension, TransactionBehavior};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::config::instance_artifacts::PTY_INPUT_LOCKS_DIR_NAME;

pub const DB_FILENAME: &str = crate::config::instance_artifacts::MESSAGE_BUS_DB_FILENAME;
pub const INLINE_BODY_MAX_BYTES: usize = 256 * 1024;
pub const DEFAULT_CONTENT_TYPE: &str = "text/markdown";
pub const STATUS_QUEUED: &str = "queued";
pub const STATUS_DELIVERING: &str = "delivering";
pub const STATUS_RETRY: &str = "retry";
pub const STATUS_DELIVERED: &str = "delivered";
pub const STATUS_POISONED: &str = "poisoned";

pub const PTY_STATUS_QUEUED: &str = "queued";
pub const PTY_STATUS_PREPARING: &str = "preparing";
pub const PTY_STATUS_RETRY: &str = "retry";
pub const PTY_STATUS_ACTUATING: &str = "actuating";
pub const PTY_STATUS_INJECTED: &str = "injected";
pub const PTY_STATUS_REJECTED: &str = "rejected";
pub const PTY_STATUS_INDETERMINATE: &str = "indeterminate";
pub const PTY_INPUT_MAX_NONTERMINAL_PER_SENDER: i64 = 16;
pub const PTY_INPUT_MAX_NONTERMINAL_GLOBAL: i64 = 512;
pub const PTY_INPUT_MAX_NONTERMINAL_BYTES: i64 = 16 * 1024 * 1024;
pub const PTY_INPUT_OPERATION_LOCK_STRIPES: usize = 4096;
pub const PTY_INPUT_TARGET_LOCK_STRIPES: usize = 1024;

#[derive(Debug, Error)]
pub enum MessageStoreError {
    #[error("config_dir is unavailable")]
    MissingConfigDir,
    #[error("message body exceeds 256 KiB")]
    BodyTooLarge,
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("blocking task failed: {0}")]
    BlockingTask(String),
    #[error("idempotency_conflict")]
    IdempotencyConflict,
    #[error("capacity_exceeded")]
    CapacityExceeded,
    #[error("operation_not_found")]
    OperationNotFound,
    #[error("invalid_state_transition")]
    InvalidTransition,
    #[error("actuation_commit_ambiguous")]
    ActuationCommitAmbiguous,
    #[error("store_corrupt")]
    StoreCorrupt,
    #[error("unsafe_store_path")]
    UnsafePath,
}

#[derive(Clone)]
pub struct MessageStore {
    path: PathBuf,
    conn: Arc<Mutex<rusqlite::Connection>>,
    maintenance_cursors: Arc<Mutex<PtyInputMaintenanceCursors>>,
    #[cfg(test)]
    test_faults: Arc<Mutex<PtyInputStoreTestFaults>>,
}

#[derive(Default)]
struct PtyInputMaintenanceCursors {
    runtime_recovery: Option<(String, String)>,
    admission_expiry: Option<(String, String)>,
    compact_before: Option<(String, String)>,
    compact_maintenance: Option<(String, String)>,
    due_container: Option<(String, String, String)>,
}

#[cfg(test)]
#[derive(Default)]
struct PtyInputStoreTestFaults {
    actuation_commit_ambiguous: bool,
}

#[derive(Debug, Clone)]
pub struct EnqueueRequest {
    pub sender_fqn: String,
    pub target_fqn: String,
    pub op_id: String,
    pub content_type: String,
    pub body: String,
    pub source_plane: String,
    pub source_ref: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnqueueResult {
    pub message_id: String,
    pub sender_fqn: String,
    pub target_fqn: String,
    pub op_id: String,
    pub status: String,
    pub duplicate: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeasedMessage {
    pub message_id: String,
    pub sender_fqn: String,
    pub target_fqn: String,
    pub op_id: String,
    pub content_type: String,
    pub body: String,
    pub attempt: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DuePtyInputCandidate {
    pub injection_id: String,
    pub target_fqn: String,
}

pub struct PtyInputEnqueueRequest {
    pub injection_id: String,
    pub sender_fqn: String,
    pub target_fqn: String,
    pub op_id: String,
    pub nonce_sha256: String,
    pub request_fingerprint: String,
    pub confirmation_tag: Option<String>,
    pub requested_agent_id: Option<String>,
    pub payload: Vec<u8>,
    pub source_plane: crate::phone::types::PtyInputSourcePlane,
    pub sender_incarnation_fingerprint: String,
    pub sender_identity_fingerprint: String,
    pub target_identity_fingerprint: String,
    pub authority_session_id: String,
    pub authority_client_id: Option<String>,
    pub authority_client_generation: Option<String>,
    pub issued_at: String,
    pub expires_at: String,
}

impl std::fmt::Debug for PtyInputEnqueueRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PtyInputEnqueueRequest")
            .field("injection_id", &self.injection_id)
            .field("sender_fqn", &self.sender_fqn)
            .field("target_fqn", &self.target_fqn)
            .field("op_id", &self.op_id)
            .field("payload_bytes", &self.payload.len())
            .field("payload_sha256", &sha256_hex(&self.payload))
            .field("source_plane", &self.source_plane)
            .finish()
    }
}

pub struct HostPtyInputRejectionRequest {
    pub injection_id: String,
    pub sender_fqn: String,
    pub target_fqn: String,
    pub op_id: String,
    pub nonce_sha256: String,
    pub request_fingerprint: String,
    pub confirmation_tag: String,
    pub sender_incarnation_fingerprint: String,
    pub payload_sha256: String,
    pub payload_bytes: u64,
    pub issued_at: String,
    pub expires_at: String,
    pub reason: crate::phone::types::PtyInputReasonCode,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PtyInputEnqueueResult {
    pub result: crate::phone::types::PtyInputResult,
    pub duplicate: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimedPtyInputOperation {
    pub injection_id: String,
    pub sender_fqn: String,
    pub target_fqn: String,
    pub op_id: String,
    pub source_plane: crate::phone::types::PtyInputSourcePlane,
    pub lease_owner: String,
    pub attempt: i64,
    pub authority_session_id: String,
    pub authority_client_id: Option<String>,
    pub authority_client_generation: Option<String>,
    pub sender_identity_fingerprint: String,
    pub target_identity_fingerprint: String,
    pub requested_agent_id: Option<String>,
    pub expires_at: String,
}

#[derive(Default)]
pub struct PtyInputActiveOperations {
    inner: Mutex<HashSet<String>>,
}

impl PtyInputActiveOperations {
    pub fn try_register(self: &Arc<Self>, injection_id: &str) -> Option<PtyInputActiveGuard> {
        let mut active = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        if !active.insert(injection_id.to_string()) {
            return None;
        }
        Some(PtyInputActiveGuard {
            owner: Arc::clone(self),
            injection_id: injection_id.to_string(),
        })
    }

    pub fn snapshot(&self) -> HashSet<String> {
        self.inner
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }
}

pub struct PtyInputActiveGuard {
    owner: Arc<PtyInputActiveOperations>,
    injection_id: String,
}

impl Drop for PtyInputActiveGuard {
    fn drop(&mut self) {
        self.owner
            .inner
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&self.injection_id);
    }
}

struct TargetLockEntry {
    gate: Arc<tokio::sync::Mutex<()>>,
    reservations: AtomicUsize,
}

#[derive(Default)]
pub struct PtyInputTargetLocks {
    entries: Mutex<HashMap<String, Arc<TargetLockEntry>>>,
}

pub struct PtyInputTargetGuard {
    owner: Arc<PtyInputTargetLocks>,
    target: String,
    entry: Arc<TargetLockEntry>,
    _guard: tokio::sync::OwnedMutexGuard<()>,
}

struct PtyInputTargetReservation {
    owner: Arc<PtyInputTargetLocks>,
    target: String,
    entry: Arc<TargetLockEntry>,
    transferred: bool,
}

fn release_target_reservation(
    owner: &Arc<PtyInputTargetLocks>,
    target: &str,
    entry: &Arc<TargetLockEntry>,
) {
    if entry.reservations.fetch_sub(1, Ordering::AcqRel) != 1 {
        return;
    }
    let mut entries = owner
        .entries
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    if entries
        .get(target)
        .is_some_and(|current| Arc::ptr_eq(current, entry))
        && entry.reservations.load(Ordering::Acquire) == 0
    {
        entries.remove(target);
    }
}

impl PtyInputTargetReservation {
    fn finish(mut self, guard: tokio::sync::OwnedMutexGuard<()>) -> PtyInputTargetGuard {
        self.transferred = true;
        PtyInputTargetGuard {
            owner: Arc::clone(&self.owner),
            target: self.target.clone(),
            entry: Arc::clone(&self.entry),
            _guard: guard,
        }
    }
}

impl Drop for PtyInputTargetReservation {
    fn drop(&mut self) {
        if !self.transferred {
            release_target_reservation(&self.owner, &self.target, &self.entry);
        }
    }
}

impl PtyInputTargetLocks {
    pub async fn acquire(self: &Arc<Self>, target: &str) -> PtyInputTargetGuard {
        let entry = {
            let mut entries = self
                .entries
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let entry = entries
                .entry(target.to_string())
                .or_insert_with(|| {
                    Arc::new(TargetLockEntry {
                        gate: Arc::new(tokio::sync::Mutex::new(())),
                        reservations: AtomicUsize::new(0),
                    })
                })
                .clone();
            entry.reservations.fetch_add(1, Ordering::AcqRel);
            entry
        };
        let reservation = PtyInputTargetReservation {
            owner: Arc::clone(self),
            target: target.to_string(),
            entry: Arc::clone(&entry),
            transferred: false,
        };
        let guard = Arc::clone(&entry.gate).lock_owned().await;
        reservation.finish(guard)
    }

    #[cfg(test)]
    fn entry_count(&self) -> usize {
        self.entries
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .len()
    }
}

impl Drop for PtyInputTargetGuard {
    fn drop(&mut self) {
        release_target_reservation(&self.owner, &self.target, &self.entry);
    }
}

/// DB-independent universal target ownership. The fixed target stripe lives
/// below the installation config directory, while the exact keyed guard is
/// shared by every create path in this process. This remains usable when the
/// privileged SQLite store is missing, corrupt, or otherwise unavailable.
pub struct PtyInputTargetGate {
    lock_root: PathBuf,
    exact_locks: Arc<PtyInputTargetLocks>,
}

impl PtyInputTargetGate {
    pub fn new(lock_root: PathBuf) -> Result<Self, MessageStoreError> {
        std::fs::create_dir_all(&lock_root)?;
        crate::path_identity::verify_directory(&lock_root)
            .map_err(|_| MessageStoreError::UnsafePath)?;
        Ok(Self {
            lock_root,
            exact_locks: Arc::new(PtyInputTargetLocks::default()),
        })
    }

    pub fn try_target_lock(
        &self,
        target_key: &str,
    ) -> Result<Option<PtyInputStripeGuard>, MessageStoreError> {
        try_stripe_lock_at(&self.lock_root, target_key, false)
    }

    pub async fn acquire_target_lock(
        &self,
        target_key: &str,
    ) -> Result<PtyInputStripeGuard, MessageStoreError> {
        loop {
            if let Some(guard) = self.try_target_lock(target_key)? {
                return Ok(guard);
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    pub async fn acquire_exact(&self, target_key: &str) -> PtyInputTargetGuard {
        self.exact_locks.acquire(target_key).await
    }

    pub(crate) fn target_ownership<'a>(
        &self,
        target: &'a str,
        stripe: &'a PtyInputStripeGuard,
        exact: &'a PtyInputTargetGuard,
    ) -> Result<PtyInputTargetOwnership<'a>, MessageStoreError> {
        if exact.target != target {
            return Err(MessageStoreError::StoreCorrupt);
        }
        Ok(PtyInputTargetOwnership {
            target,
            _stripe: stripe,
            _exact: exact,
        })
    }

    #[cfg(test)]
    pub(crate) fn exact_entry_count(&self) -> usize {
        self.exact_locks.entry_count()
    }
}

#[derive(Clone)]
pub struct PtyInputTargetGateState {
    pub gate: Result<Arc<PtyInputTargetGate>, String>,
}

impl PtyInputTargetGateState {
    pub fn at_config_dir() -> Self {
        let gate = crate::config::config_dir()
            .ok_or(MessageStoreError::MissingConfigDir)
            .and_then(PtyInputTargetGate::new)
            .map(Arc::new)
            .map_err(|_| "target_gate_unavailable".to_string());
        Self { gate }
    }

    pub fn for_root(lock_root: PathBuf) -> Self {
        let gate = PtyInputTargetGate::new(lock_root)
            .map(Arc::new)
            .map_err(|_| "target_gate_unavailable".to_string());
        Self { gate }
    }
}

#[derive(Clone)]
pub struct MessageStoreState {
    pub store: Result<Arc<MessageStore>, String>,
    pub active_operations: Arc<PtyInputActiveOperations>,
    pub target_locks: Arc<PtyInputTargetLocks>,
    pub target_gate: Result<Arc<PtyInputTargetGate>, String>,
}

impl MessageStoreState {
    pub fn initialize() -> Self {
        let target_gate = PtyInputTargetGateState::at_config_dir().gate;
        let store = MessageStore::at_config_dir()
            .map(Arc::new)
            .map_err(|_| "store_unavailable".to_string());
        let target_locks = target_gate
            .as_ref()
            .map(|gate| Arc::clone(&gate.exact_locks))
            .unwrap_or_else(|_| Arc::new(PtyInputTargetLocks::default()));
        Self {
            store,
            active_operations: Arc::new(PtyInputActiveOperations::default()),
            target_locks,
            target_gate,
        }
    }

    pub fn ready(store: Arc<MessageStore>) -> Self {
        let lock_root = store
            .path
            .parent()
            .map(Path::to_path_buf)
            .ok_or(MessageStoreError::UnsafePath)
            .and_then(PtyInputTargetGate::new)
            .map(Arc::new)
            .map_err(|_| "target_gate_unavailable".to_string());
        Self::with_store_and_target_gate(Ok(store), lock_root)
    }

    pub fn with_store_and_target_gate(
        store: Result<Arc<MessageStore>, String>,
        target_gate: Result<Arc<PtyInputTargetGate>, String>,
    ) -> Self {
        let target_locks = target_gate
            .as_ref()
            .map(|gate| Arc::clone(&gate.exact_locks))
            .unwrap_or_else(|_| Arc::new(PtyInputTargetLocks::default()));
        Self {
            store,
            active_operations: Arc::new(PtyInputActiveOperations::default()),
            target_locks,
            target_gate,
        }
    }

    pub fn target_gate_state(&self) -> PtyInputTargetGateState {
        PtyInputTargetGateState {
            gate: self.target_gate.clone(),
        }
    }
}

pub struct PtyInputStripeGuard {
    _file: File,
}

pub(crate) struct PtyInputTargetOwnership<'a> {
    target: &'a str,
    _stripe: &'a PtyInputStripeGuard,
    _exact: &'a PtyInputTargetGuard,
}

impl PtyInputTargetOwnership<'_> {
    pub(crate) fn proves(&self, target: &str) -> bool {
        self.target == target && self._exact.target == target
    }
}

pub struct PreparationHeartbeatGuard {
    cancellation: tokio_util::sync::CancellationToken,
    failed: Arc<std::sync::atomic::AtomicBool>,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl PreparationHeartbeatGuard {
    pub fn start(
        store: Arc<MessageStore>,
        injection_id: String,
        lease_owner: String,
        expires_at: DateTime<Utc>,
    ) -> Self {
        Self::start_with_interval(
            store,
            injection_id,
            lease_owner,
            expires_at,
            Duration::from_secs(30),
        )
    }

    fn start_with_interval(
        store: Arc<MessageStore>,
        injection_id: String,
        lease_owner: String,
        expires_at: DateTime<Utc>,
        heartbeat_interval: Duration,
    ) -> Self {
        let cancellation = tokio_util::sync::CancellationToken::new();
        let task_cancellation = cancellation.clone();
        let failed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let task_failed = Arc::clone(&failed);
        let task = tokio::spawn(async move {
            let mut interval = tokio::time::interval(heartbeat_interval);
            interval.tick().await;
            let until_expiry = expires_at
                .signed_duration_since(Utc::now())
                .to_std()
                .unwrap_or(Duration::ZERO);
            let expiry = tokio::time::sleep(until_expiry);
            tokio::pin!(expiry);
            loop {
                tokio::select! {
                    biased;
                    _ = task_cancellation.cancelled() => break,
                    _ = &mut expiry => {
                        task_failed.store(true, std::sync::atomic::Ordering::SeqCst);
                        break;
                    }
                    _ = interval.tick() => {
                        let renewal = store
                            .renew_pty_input_lease_offloaded(
                                injection_id.clone(),
                                lease_owner.clone(),
                                Utc::now(),
                            )
                            .await;
                        if !matches!(renewal, Ok(true)) {
                            task_failed.store(true, std::sync::atomic::Ordering::SeqCst);
                            break;
                        }
                    }
                }
            }
        });
        Self {
            cancellation,
            failed,
            task: Some(task),
        }
    }

    pub fn failed(&self) -> bool {
        self.failed.load(std::sync::atomic::Ordering::SeqCst)
    }

    pub async fn finish(&mut self) -> bool {
        self.cancellation.cancel();
        if let Some(task) = self.task.take() {
            if task.await.is_err() {
                self.failed.store(true, std::sync::atomic::Ordering::SeqCst);
            }
        }
        !self.failed()
    }
}

impl Drop for PreparationHeartbeatGuard {
    fn drop(&mut self) {
        self.cancellation.cancel();
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

fn sqlite_sidecar_paths(path: &Path) -> [PathBuf; 3] {
    let with_suffix = |suffix: &str| {
        let mut value = path.as_os_str().to_os_string();
        value.push(suffix);
        PathBuf::from(value)
    };
    [
        with_suffix("-wal"),
        with_suffix("-shm"),
        with_suffix("-journal"),
    ]
}

fn verify_existing_sqlite_files(path: &Path) -> Result<(), MessageStoreError> {
    for candidate in std::iter::once(path.to_path_buf()).chain(sqlite_sidecar_paths(path)) {
        match std::fs::symlink_metadata(&candidate) {
            Ok(_) => {
                crate::path_identity::verify_regular_file(&candidate)
                    .map_err(|_| MessageStoreError::UnsafePath)?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(MessageStoreError::Io(error)),
        }
    }
    Ok(())
}

fn try_stripe_lock_at(
    parent: &Path,
    value: &str,
    operation: bool,
) -> Result<Option<PtyInputStripeGuard>, MessageStoreError> {
    let parent_identity = crate::path_identity::verify_directory(parent)
        .map_err(|_| MessageStoreError::UnsafePath)?;
    let lock_dir = parent.join(PTY_INPUT_LOCKS_DIR_NAME);
    if !lock_dir.exists() {
        match std::fs::create_dir(&lock_dir) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(MessageStoreError::Io(error)),
        }
    }
    let lock_dir_identity = crate::path_identity::verify_directory(&lock_dir)
        .map_err(|_| MessageStoreError::UnsafePath)?;
    let digest = Sha256::digest(value.as_bytes());
    let (count, prefix, index) = if operation {
        (
            PTY_INPUT_OPERATION_LOCK_STRIPES,
            "operation",
            (((digest[0] as usize) << 4) | ((digest[1] as usize) >> 4)),
        )
    } else {
        (
            PTY_INPUT_TARGET_LOCK_STRIPES,
            "target",
            (((digest[0] as usize) << 2) | ((digest[1] as usize) >> 6)),
        )
    };
    let width = if operation { 4 } else { 3 };
    let index = index % count;
    let path = lock_dir.join(format!("{prefix}-{index:0width$x}.lock"));
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
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
    let file = options.open(&path)?;
    crate::path_identity::verify_opened_regular_file(&path, &file, true)
        .map_err(|_| MessageStoreError::UnsafePath)?;
    match file.try_lock() {
        Ok(()) => {
            crate::path_identity::verify_opened_regular_file(&path, &file, true)
                .map_err(|_| MessageStoreError::UnsafePath)?;
            let current_parent = crate::path_identity::verify_directory(parent)
                .map_err(|_| MessageStoreError::UnsafePath)?;
            let current_lock_dir = crate::path_identity::verify_directory(&lock_dir)
                .map_err(|_| MessageStoreError::UnsafePath)?;
            if !crate::path_identity::same_object(&parent_identity, &current_parent)
                || !crate::path_identity::same_object(&lock_dir_identity, &current_lock_dir)
            {
                return Err(MessageStoreError::UnsafePath);
            }
            Ok(Some(PtyInputStripeGuard { _file: file }))
        }
        Err(error) => {
            let error: std::io::Error = error.into();
            if error.kind() == std::io::ErrorKind::WouldBlock {
                Ok(None)
            } else {
                Err(MessageStoreError::Io(error))
            }
        }
    }
}

impl MessageStore {
    pub fn at_config_dir() -> Result<Self, MessageStoreError> {
        let dir = crate::config::config_dir().ok_or(MessageStoreError::MissingConfigDir)?;
        Self::open(dir.join(DB_FILENAME))
    }

    pub fn open(path: PathBuf) -> Result<Self, MessageStoreError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
            crate::path_identity::verify_directory(parent)
                .map_err(|_| MessageStoreError::UnsafePath)?;
        }
        verify_existing_sqlite_files(&path)?;
        let flags = rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE
            | rusqlite::OpenFlags::SQLITE_OPEN_CREATE
            | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX
            | rusqlite::OpenFlags::SQLITE_OPEN_NOFOLLOW;
        let conn = rusqlite::Connection::open_with_flags(&path, flags)?;
        conn.busy_timeout(Duration::from_millis(5000))?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let _: String = conn.pragma_query_value(None, "journal_mode", |row| row.get(0))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        verify_existing_sqlite_files(&path)?;
        let store = Self {
            path,
            conn: Arc::new(Mutex::new(conn)),
            maintenance_cursors: Arc::new(Mutex::new(PtyInputMaintenanceCursors::default())),
            #[cfg(test)]
            test_faults: Arc::new(Mutex::new(PtyInputStoreTestFaults::default())),
        };
        store.migrate()?;
        store.recover_pty_input_startup()?;
        Ok(store)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Test-only teardown: release the SQLite/WAL/SHM file handles held by the
    /// live connection even while other `Arc` clones of this store (managed
    /// app state, API state) still exist. The connection is replaced with an
    /// in-memory one so the stored `Mutex<Connection>` stays usable and the
    /// dropped file-backed connection closes its handles; callers must not use
    /// the store afterwards. `AcceptanceFixture` calls this before its
    /// temporary directory is removed, because on Windows a directory cannot
    /// be deleted while the SQLite connection keeps it open.
    #[cfg(test)]
    pub(crate) fn close_for_test(&self) {
        let mut guard = match self.conn.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        if let Ok(in_memory) = rusqlite::Connection::open_in_memory() {
            let _ = std::mem::replace(&mut *guard, in_memory);
        }
    }

    fn migrate(&self) -> Result<(), MessageStoreError> {
        let mut conn = self
            .conn
            .lock()
            .map_err(|_| MessageStoreError::StoreCorrupt)?;
        conn.pragma_update(None, "foreign_keys", "OFF")?;
        let tx = conn.transaction()?;
        tx.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS api_message_schema(
                version INTEGER PRIMARY KEY,
                applied_at TEXT NOT NULL
            );
            "#,
        )?;
        let current: Option<i64> =
            tx.query_row("SELECT MAX(version) FROM api_message_schema", [], |row| {
                row.get::<_, Option<i64>>(0)
            })?;
        let mut schema_version = current.unwrap_or(0);
        if schema_version > 3 {
            return Err(MessageStoreError::StoreCorrupt);
        }
        if schema_version < 1 {
            tx.execute_batch(
                r#"
                CREATE TABLE IF NOT EXISTS messages(
                    message_id TEXT PRIMARY KEY,
                    sender_fqn TEXT NOT NULL,
                    target_fqn TEXT NOT NULL,
                    op_id TEXT NOT NULL,
                    content_type TEXT NOT NULL,
                    body TEXT NOT NULL,
                    body_sha256 TEXT NOT NULL,
                    body_bytes INTEGER NOT NULL,
                    source_plane TEXT NOT NULL,
                    source_ref TEXT NULL,
                    status TEXT NOT NULL,
                    attempt INTEGER NOT NULL DEFAULT 0,
                    next_attempt_at TEXT NOT NULL,
                    lease_owner TEXT NULL,
                    lease_until TEXT NULL,
                    created_at TEXT NOT NULL,
                    delivered_at TEXT NULL,
                    last_error TEXT NULL,
                    UNIQUE(sender_fqn, op_id)
                );
                CREATE INDEX IF NOT EXISTS idx_messages_due
                    ON messages(status, next_attempt_at, lease_until);
                CREATE TABLE IF NOT EXISTS message_audit(
                    event_id TEXT PRIMARY KEY,
                    message_id TEXT NOT NULL,
                    status TEXT NOT NULL,
                    detail TEXT NULL,
                    at TEXT NOT NULL,
                    FOREIGN KEY(message_id) REFERENCES messages(message_id) ON DELETE CASCADE
                );
                "#,
            )?;
            tx.execute(
                "INSERT INTO api_message_schema(version, applied_at) VALUES(1, ?1)",
                [Utc::now().to_rfc3339()],
            )?;
            schema_version = 1;
        }
        if schema_version < 2 {
            tx.execute_batch(
                r#"
                CREATE TABLE pty_input_operations(
                    injection_id TEXT PRIMARY KEY,
                    sender_fqn TEXT NOT NULL,
                    target_fqn TEXT NOT NULL,
                    op_id TEXT NOT NULL,
                    nonce_sha256 TEXT NOT NULL,
                    request_fingerprint TEXT NOT NULL,
                    confirmation_tag TEXT NULL,
                    version INTEGER NOT NULL CHECK(version = 1),
                    enter_mode TEXT NOT NULL CHECK(enter_mode = 'agent-submit'),
                    requested_agent_id TEXT NULL,
                    payload BLOB NULL,
                    payload_sha256 TEXT NOT NULL,
                    payload_bytes INTEGER NOT NULL CHECK(payload_bytes BETWEEN 1 AND 65536),
                    source_plane TEXT NOT NULL CHECK(source_plane IN ('host_cli','container_api')),
                    sender_incarnation_fingerprint TEXT NOT NULL,
                    sender_identity_fingerprint TEXT NULL,
                    target_identity_fingerprint TEXT NULL,
                    authority_session_id TEXT NULL,
                    authority_client_id TEXT NULL,
                    authority_client_generation TEXT NULL,
                    status TEXT NOT NULL CHECK(status IN ('queued','preparing','retry','actuating','injected','rejected','indeterminate')),
                    attempt INTEGER NOT NULL DEFAULT 0 CHECK(attempt BETWEEN 0 AND 5),
                    next_attempt_at TEXT NOT NULL,
                    lease_owner TEXT NULL,
                    lease_until TEXT NULL,
                    selected_session_id TEXT NULL,
                    selected_backend TEXT NULL,
                    issued_at TEXT NOT NULL,
                    expires_at TEXT NOT NULL,
                    queued_at TEXT NOT NULL,
                    preparing_at TEXT NULL,
                    actuating_at TEXT NULL,
                    terminal_at TEXT NULL,
                    host_artifact_at TEXT NULL,
                    updated_at TEXT NOT NULL,
                    reason_code TEXT NULL,
                    reason_detail TEXT NULL,
                    UNIQUE(sender_fqn, op_id),
                    UNIQUE(sender_fqn, nonce_sha256),
                    CHECK(length(injection_id)=36
                          AND substr(injection_id,9,1)='-'
                          AND substr(injection_id,14,1)='-'
                          AND substr(injection_id,15,1)='4'
                          AND substr(injection_id,19,1)='-'
                          AND substr(injection_id,20,1) GLOB '[89ab]'
                          AND substr(injection_id,24,1)='-'
                          AND length(replace(injection_id,'-',''))=32
                          AND replace(injection_id,'-','') NOT GLOB '*[^0-9a-f]*'),
                    CHECK(length(op_id)=36
                          AND substr(op_id,9,1)='-'
                          AND substr(op_id,14,1)='-'
                          AND substr(op_id,15,1)='4'
                          AND substr(op_id,19,1)='-'
                          AND substr(op_id,20,1) GLOB '[89ab]'
                          AND substr(op_id,24,1)='-'
                          AND length(replace(op_id,'-',''))=32
                          AND replace(op_id,'-','') NOT GLOB '*[^0-9a-f]*'),
                    CHECK(authority_session_id IS NULL OR
                          (length(authority_session_id)=36
                           AND substr(authority_session_id,9,1)='-'
                           AND substr(authority_session_id,14,1)='-'
                           AND substr(authority_session_id,15,1)='4'
                           AND substr(authority_session_id,19,1)='-'
                           AND substr(authority_session_id,20,1) GLOB '[89ab]'
                           AND substr(authority_session_id,24,1)='-'
                           AND length(replace(authority_session_id,'-',''))=32
                           AND replace(authority_session_id,'-','') NOT GLOB '*[^0-9a-f]*')),
                    CHECK(authority_client_generation IS NULL OR
                          (length(authority_client_generation)=36
                           AND substr(authority_client_generation,9,1)='-'
                           AND substr(authority_client_generation,14,1)='-'
                           AND substr(authority_client_generation,15,1)='4'
                           AND substr(authority_client_generation,19,1)='-'
                           AND substr(authority_client_generation,20,1) GLOB '[89ab]'
                           AND substr(authority_client_generation,24,1)='-'
                           AND length(replace(authority_client_generation,'-',''))=32
                           AND replace(authority_client_generation,'-','') NOT GLOB '*[^0-9a-f]*')),
                    CHECK(selected_session_id IS NULL OR
                          (length(selected_session_id)=36
                           AND substr(selected_session_id,9,1)='-'
                           AND substr(selected_session_id,14,1)='-'
                           AND substr(selected_session_id,15,1)='4'
                           AND substr(selected_session_id,19,1)='-'
                           AND substr(selected_session_id,20,1) GLOB '[89ab]'
                           AND substr(selected_session_id,24,1)='-'
                           AND length(replace(selected_session_id,'-',''))=32
                           AND replace(selected_session_id,'-','') NOT GLOB '*[^0-9a-f]*')),
                    CHECK(length(nonce_sha256)=64 AND nonce_sha256 NOT GLOB '*[^0-9a-f]*'),
                    CHECK(length(request_fingerprint)=64 AND request_fingerprint NOT GLOB '*[^0-9a-f]*'),
                    CHECK(length(payload_sha256)=64 AND payload_sha256 NOT GLOB '*[^0-9a-f]*'),
                    CHECK(length(sender_incarnation_fingerprint)=64 AND sender_incarnation_fingerprint NOT GLOB '*[^0-9a-f]*'),
                    CHECK(confirmation_tag IS NULL OR (length(confirmation_tag)=64 AND confirmation_tag NOT GLOB '*[^0-9a-f]*')),
                    CHECK(sender_identity_fingerprint IS NULL OR (length(sender_identity_fingerprint)=64 AND sender_identity_fingerprint NOT GLOB '*[^0-9a-f]*')),
                    CHECK(target_identity_fingerprint IS NULL OR (length(target_identity_fingerprint)=64 AND target_identity_fingerprint NOT GLOB '*[^0-9a-f]*')),
                    CHECK(payload IS NULL OR length(payload)=payload_bytes),
                    CHECK(length(issued_at)=24 AND length(expires_at)=24 AND length(queued_at)=24
                          AND length(next_attempt_at)=24 AND length(updated_at)=24),
                    CHECK(issued_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9]Z'),
                    CHECK(expires_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9]Z'),
                    CHECK(queued_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9]Z'),
                    CHECK(next_attempt_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9]Z'),
                    CHECK(updated_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9]Z'),
                    CHECK(preparing_at IS NULL OR (length(preparing_at)=24 AND preparing_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9]Z')),
                    CHECK(actuating_at IS NULL OR (length(actuating_at)=24 AND actuating_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9]Z')),
                    CHECK(terminal_at IS NULL OR (length(terminal_at)=24 AND terminal_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9]Z')),
                    CHECK(host_artifact_at IS NULL OR (length(host_artifact_at)=24 AND host_artifact_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9]Z')),
                    CHECK((selected_session_id IS NULL) = (selected_backend IS NULL)),
                    CHECK(selected_backend IS NULL OR selected_backend IN ('localProcess','containerTransport')),
                    CHECK(reason_code IS NULL OR reason_code IN (
                        'invalid_envelope','mixed_payload','unsupported_version','invalid_enter_mode',
                        'invalid_id','invalid_nonce','invalid_timestamp','expired','invalid_target',
                        'invalid_text','payload_too_large','idempotency_conflict','capacity_exceeded',
                        'session_token_required','invalid_session_token','ambiguous_session_token',
                        'sender_session_not_live','sender_backend_not_local','sender_identity_invalid',
                        'sender_not_coordinator','root_identity_invalid','target_not_member',
                        'target_is_coordinator','target_out_of_scope','unsafe_path','api_scope_required',
                        'api_client_unbound','api_client_stale','api_binding_mismatch','authority_changed',
                        'busy','resize_unsettled','untracked_readiness','unsupported_session',
                        'nonpersistent_live_session','inconsistent_session','unsupported_profile',
                        'readiness_timeout','store_corrupt','restore_in_progress','purge_in_progress',
                        'session_race','lease_lost','spawn_failed_safe','store_transient',
                        'menu_guard_blocked',
                        'final_revalidation_failed','text_write_failed','required_enter_failed',
                        'daemon_restart_after_actuation','runtime_actuation_orphan','terminal_store_failed',
                        'redundant_enter_failed','boundary_metadata_failed','artifact_unclaimed'
                    )),
                    CHECK((reason_code IS NULL) = (reason_detail IS NULL)),
                    CHECK(
                        (status IN ('queued','preparing','retry')
                         AND payload IS NOT NULL
                         AND authority_session_id IS NOT NULL
                         AND sender_identity_fingerprint IS NOT NULL
                         AND target_identity_fingerprint IS NOT NULL
                         AND actuating_at IS NULL AND terminal_at IS NULL
                         AND selected_session_id IS NULL AND selected_backend IS NULL)
                        OR
                        (status='rejected' AND payload IS NULL AND requested_agent_id IS NULL
                         AND authority_session_id IS NULL AND authority_client_id IS NULL
                         AND authority_client_generation IS NULL
                         AND sender_identity_fingerprint IS NULL
                         AND target_identity_fingerprint IS NULL
                         AND actuating_at IS NULL AND terminal_at IS NOT NULL
                         AND selected_session_id IS NULL AND selected_backend IS NULL)
                        OR
                        (status IN ('actuating','injected','indeterminate')
                         AND payload IS NULL AND requested_agent_id IS NULL
                         AND authority_session_id IS NULL AND authority_client_id IS NULL
                         AND authority_client_generation IS NULL
                         AND sender_identity_fingerprint IS NULL
                         AND target_identity_fingerprint IS NULL
                         AND actuating_at IS NOT NULL
                         AND selected_session_id IS NOT NULL AND selected_backend IS NOT NULL)
                    ),
                    CHECK((source_plane='host_cli' AND confirmation_tag IS NOT NULL
                           AND authority_client_id IS NULL AND authority_client_generation IS NULL)
                       OR (source_plane='container_api' AND confirmation_tag IS NULL
                           AND ((status IN ('queued','preparing','retry')
                                 AND authority_client_id IS NOT NULL
                                 AND authority_client_generation IS NOT NULL)
                             OR (status IN ('actuating','injected','rejected','indeterminate')
                                 AND authority_client_id IS NULL
                                 AND authority_client_generation IS NULL)))),
                    CHECK((status IN ('injected','rejected','indeterminate')) = (terminal_at IS NOT NULL)),
                    CHECK((status = 'preparing') = (lease_owner IS NOT NULL AND lease_until IS NOT NULL)),
                    CHECK(status!='preparing' OR preparing_at IS NOT NULL),
                    CHECK(queued_at>=issued_at AND queued_at<expires_at),
                    CHECK(actuating_at IS NULL OR (actuating_at>=queued_at AND actuating_at<expires_at)),
                    CHECK(terminal_at IS NULL OR terminal_at>=queued_at),
                    CHECK(host_artifact_at IS NULL OR terminal_at IS NOT NULL),
                    CHECK((status IN ('queued','preparing','retry') AND
                           (reason_code IS NULL OR reason_code IN (
                             'restore_in_progress','purge_in_progress','session_race',
                             'lease_lost','spawn_failed_safe','store_transient',
                             'menu_guard_blocked')))
                       OR (status='actuating' AND reason_code IS NULL)
                       OR (status='injected' AND
                           (reason_code IS NULL OR reason_code IN (
                             'redundant_enter_failed','boundary_metadata_failed')))
                       OR (status='rejected' AND reason_code IS NOT NULL AND reason_code NOT IN (
                             'final_revalidation_failed','text_write_failed','required_enter_failed',
                             'daemon_restart_after_actuation','runtime_actuation_orphan',
                             'terminal_store_failed','redundant_enter_failed',
                             'boundary_metadata_failed','artifact_unclaimed'))
                       OR (status='indeterminate' AND reason_code IN (
                             'final_revalidation_failed','text_write_failed','required_enter_failed',
                             'daemon_restart_after_actuation','runtime_actuation_orphan',
                             'terminal_store_failed'))),
                    CHECK(source_plane != 'host_cli' OR injection_id=op_id)
                );
                CREATE INDEX idx_pty_input_due
                    ON pty_input_operations(source_plane, status, next_attempt_at, lease_until);
                CREATE TABLE pty_input_audit(
                    event_id TEXT PRIMARY KEY,
                    injection_id TEXT NOT NULL,
                    op_id TEXT NOT NULL,
                    sender_fqn TEXT NOT NULL,
                    target_fqn TEXT NOT NULL,
                    version INTEGER NOT NULL,
                    payload_bytes INTEGER NOT NULL,
                    payload_sha256 TEXT NOT NULL,
                    source_plane TEXT NOT NULL,
                    selected_session_id TEXT NULL,
                    selected_backend TEXT NULL,
                    status TEXT NOT NULL,
                    reason_code TEXT NULL,
                    at TEXT NOT NULL,
                    FOREIGN KEY(injection_id) REFERENCES pty_input_operations(injection_id) ON DELETE CASCADE,
                    CHECK(version=1),
                    CHECK(payload_bytes BETWEEN 1 AND 65536),
                    CHECK(length(payload_sha256)=64 AND payload_sha256 NOT GLOB '*[^0-9a-f]*'),
                    CHECK(source_plane IN ('host_cli','container_api')),
                    CHECK(status IN ('queued','preparing','retry','actuating','injected','rejected','indeterminate')),
                    CHECK((selected_session_id IS NULL) = (selected_backend IS NULL)),
                    CHECK(selected_backend IS NULL OR selected_backend IN ('localProcess','containerTransport')),
                    CHECK(length(at)=24 AND at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9]Z')
                );
                CREATE TABLE pty_input_tombstones(
                    injection_id TEXT PRIMARY KEY,
                    sender_fqn TEXT NOT NULL,
                    target_fqn TEXT NOT NULL,
                    op_id TEXT NOT NULL,
                    nonce_sha256 TEXT NOT NULL,
                    request_fingerprint TEXT NOT NULL,
                    confirmation_tag TEXT NULL,
                    sender_incarnation_fingerprint TEXT NOT NULL,
                    version INTEGER NOT NULL,
                    payload_sha256 TEXT NOT NULL,
                    payload_bytes INTEGER NOT NULL,
                    source_plane TEXT NOT NULL,
                    selected_session_id TEXT NULL,
                    selected_backend TEXT NULL,
                    issued_at TEXT NOT NULL,
                    expires_at TEXT NOT NULL,
                    queued_at TEXT NOT NULL,
                    actuating_at TEXT NULL,
                    terminal_at TEXT NOT NULL,
                    status TEXT NOT NULL CHECK(status IN ('injected','rejected','indeterminate')),
                    reason_code TEXT NULL,
                    reason_detail TEXT NULL,
                    UNIQUE(sender_fqn, op_id),
                    UNIQUE(sender_fqn, nonce_sha256),
                    CHECK(version=1),
                    CHECK(payload_bytes BETWEEN 1 AND 65536),
                    CHECK(length(injection_id)=36
                          AND substr(injection_id,9,1)='-'
                          AND substr(injection_id,14,1)='-'
                          AND substr(injection_id,15,1)='4'
                          AND substr(injection_id,19,1)='-'
                          AND substr(injection_id,20,1) GLOB '[89ab]'
                          AND substr(injection_id,24,1)='-'
                          AND length(replace(injection_id,'-',''))=32
                          AND replace(injection_id,'-','') NOT GLOB '*[^0-9a-f]*'),
                    CHECK(length(op_id)=36
                          AND substr(op_id,9,1)='-'
                          AND substr(op_id,14,1)='-'
                          AND substr(op_id,15,1)='4'
                          AND substr(op_id,19,1)='-'
                          AND substr(op_id,20,1) GLOB '[89ab]'
                          AND substr(op_id,24,1)='-'
                          AND length(replace(op_id,'-',''))=32
                          AND replace(op_id,'-','') NOT GLOB '*[^0-9a-f]*'),
                    CHECK(selected_session_id IS NULL OR
                          (length(selected_session_id)=36
                           AND substr(selected_session_id,9,1)='-'
                           AND substr(selected_session_id,14,1)='-'
                           AND substr(selected_session_id,15,1)='4'
                           AND substr(selected_session_id,19,1)='-'
                           AND substr(selected_session_id,20,1) GLOB '[89ab]'
                           AND substr(selected_session_id,24,1)='-'
                           AND length(replace(selected_session_id,'-',''))=32
                           AND replace(selected_session_id,'-','') NOT GLOB '*[^0-9a-f]*')),
                    CHECK(length(nonce_sha256)=64 AND nonce_sha256 NOT GLOB '*[^0-9a-f]*'),
                    CHECK(length(request_fingerprint)=64 AND request_fingerprint NOT GLOB '*[^0-9a-f]*'),
                    CHECK(length(sender_incarnation_fingerprint)=64 AND sender_incarnation_fingerprint NOT GLOB '*[^0-9a-f]*'),
                    CHECK(length(payload_sha256)=64 AND payload_sha256 NOT GLOB '*[^0-9a-f]*'),
                    CHECK(confirmation_tag IS NULL OR (length(confirmation_tag)=64 AND confirmation_tag NOT GLOB '*[^0-9a-f]*')),
                    CHECK((source_plane='host_cli' AND confirmation_tag IS NOT NULL AND injection_id=op_id)
                       OR (source_plane='container_api' AND confirmation_tag IS NULL)),
                    CHECK((selected_session_id IS NULL) = (selected_backend IS NULL)),
                    CHECK(selected_backend IS NULL OR selected_backend IN ('localProcess','containerTransport')),
                    CHECK(length(issued_at)=24 AND issued_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9]Z'),
                    CHECK(length(expires_at)=24 AND expires_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9]Z'),
                    CHECK(length(queued_at)=24 AND queued_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9]Z'),
                    CHECK(length(terminal_at)=24 AND terminal_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9]Z'),
                    CHECK(actuating_at IS NULL OR (length(actuating_at)=24 AND actuating_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9]Z')),
                    CHECK(queued_at>=issued_at AND queued_at<expires_at),
                    CHECK(actuating_at IS NULL OR (actuating_at>=queued_at AND actuating_at<expires_at)),
                    CHECK(terminal_at>=queued_at),
                    CHECK((reason_code IS NULL) = (reason_detail IS NULL)),
                    CHECK(reason_code IS NULL OR reason_code IN (
                        'invalid_envelope','mixed_payload','unsupported_version','invalid_enter_mode',
                        'invalid_id','invalid_nonce','invalid_timestamp','expired','invalid_target',
                        'invalid_text','payload_too_large','idempotency_conflict','capacity_exceeded',
                        'session_token_required','invalid_session_token','ambiguous_session_token',
                        'sender_session_not_live','sender_backend_not_local','sender_identity_invalid',
                        'sender_not_coordinator','root_identity_invalid','target_not_member',
                        'target_is_coordinator','target_out_of_scope','unsafe_path','api_scope_required',
                        'api_client_unbound','api_client_stale','api_binding_mismatch','authority_changed',
                        'busy','resize_unsettled','untracked_readiness','unsupported_session',
                        'nonpersistent_live_session','inconsistent_session','unsupported_profile',
                        'readiness_timeout','store_corrupt','restore_in_progress','purge_in_progress',
                        'session_race','lease_lost','spawn_failed_safe','store_transient',
                        'menu_guard_blocked',
                        'final_revalidation_failed','text_write_failed','required_enter_failed',
                        'daemon_restart_after_actuation','runtime_actuation_orphan','terminal_store_failed',
                        'redundant_enter_failed','boundary_metadata_failed','artifact_unclaimed'
                    )),
                    CHECK((status='injected' AND actuating_at IS NOT NULL
                           AND selected_session_id IS NOT NULL
                           AND (reason_code IS NULL OR reason_code IN (
                             'redundant_enter_failed','boundary_metadata_failed')))
                       OR (status='rejected' AND actuating_at IS NULL
                           AND selected_session_id IS NULL
                           AND reason_code IS NOT NULL AND reason_code NOT IN (
                             'final_revalidation_failed','text_write_failed','required_enter_failed',
                             'daemon_restart_after_actuation','runtime_actuation_orphan',
                             'terminal_store_failed','redundant_enter_failed',
                             'boundary_metadata_failed','artifact_unclaimed'))
                       OR (status='indeterminate' AND actuating_at IS NOT NULL
                           AND selected_session_id IS NOT NULL
                           AND reason_code IN (
                             'final_revalidation_failed','text_write_failed','required_enter_failed',
                             'daemon_restart_after_actuation','runtime_actuation_orphan',
                             'terminal_store_failed')))
                );
                "#,
            )?;
            tx.execute(
                "INSERT INTO api_message_schema(version, applied_at) VALUES(2, ?1)",
                [crate::phone::types::canonical_pty_timestamp(Utc::now())],
            )?;
            schema_version = 2;
        }
        if schema_version < 3 {
            tx.execute_batch(
                r#"
                CREATE TABLE pty_input_operations_v3(
                    injection_id TEXT PRIMARY KEY,
                    sender_fqn TEXT NOT NULL,
                    target_fqn TEXT NOT NULL,
                    op_id TEXT NOT NULL,
                    nonce_sha256 TEXT NOT NULL,
                    request_fingerprint TEXT NOT NULL,
                    confirmation_tag TEXT NULL,
                    version INTEGER NOT NULL CHECK(version = 1),
                    enter_mode TEXT NOT NULL CHECK(enter_mode = 'agent-submit'),
                    requested_agent_id TEXT NULL,
                    payload BLOB NULL,
                    payload_sha256 TEXT NOT NULL,
                    payload_bytes INTEGER NOT NULL CHECK(payload_bytes BETWEEN 1 AND 65536),
                    source_plane TEXT NOT NULL CHECK(source_plane IN ('host_cli','container_api')),
                    sender_incarnation_fingerprint TEXT NOT NULL,
                    sender_identity_fingerprint TEXT NULL,
                    target_identity_fingerprint TEXT NULL,
                    authority_session_id TEXT NULL,
                    authority_client_id TEXT NULL,
                    authority_client_generation TEXT NULL,
                    status TEXT NOT NULL CHECK(status IN ('queued','preparing','retry','actuating','injected','rejected','indeterminate')),
                    attempt INTEGER NOT NULL DEFAULT 0 CHECK(attempt BETWEEN 0 AND 5),
                    next_attempt_at TEXT NOT NULL,
                    lease_owner TEXT NULL,
                    lease_until TEXT NULL,
                    selected_session_id TEXT NULL,
                    selected_backend TEXT NULL,
                    issued_at TEXT NOT NULL,
                    expires_at TEXT NOT NULL,
                    queued_at TEXT NOT NULL,
                    preparing_at TEXT NULL,
                    actuating_at TEXT NULL,
                    terminal_at TEXT NULL,
                    host_artifact_at TEXT NULL,
                    updated_at TEXT NOT NULL,
                    reason_code TEXT NULL,
                    reason_detail TEXT NULL,
                    UNIQUE(sender_fqn, op_id),
                    UNIQUE(sender_fqn, nonce_sha256),
                    CHECK(length(injection_id)=36
                          AND substr(injection_id,9,1)='-'
                          AND substr(injection_id,14,1)='-'
                          AND substr(injection_id,15,1)='4'
                          AND substr(injection_id,19,1)='-'
                          AND substr(injection_id,20,1) GLOB '[89ab]'
                          AND substr(injection_id,24,1)='-'
                          AND length(replace(injection_id,'-',''))=32
                          AND replace(injection_id,'-','') NOT GLOB '*[^0-9a-f]*'),
                    CHECK(length(op_id)=36
                          AND substr(op_id,9,1)='-'
                          AND substr(op_id,14,1)='-'
                          AND substr(op_id,15,1)='4'
                          AND substr(op_id,19,1)='-'
                          AND substr(op_id,20,1) GLOB '[89ab]'
                          AND substr(op_id,24,1)='-'
                          AND length(replace(op_id,'-',''))=32
                          AND replace(op_id,'-','') NOT GLOB '*[^0-9a-f]*'),
                    CHECK(authority_session_id IS NULL OR
                          (length(authority_session_id)=36
                           AND substr(authority_session_id,9,1)='-'
                           AND substr(authority_session_id,14,1)='-'
                           AND substr(authority_session_id,15,1)='4'
                           AND substr(authority_session_id,19,1)='-'
                           AND substr(authority_session_id,20,1) GLOB '[89ab]'
                           AND substr(authority_session_id,24,1)='-'
                           AND length(replace(authority_session_id,'-',''))=32
                           AND replace(authority_session_id,'-','') NOT GLOB '*[^0-9a-f]*')),
                    CHECK(authority_client_generation IS NULL OR
                          (length(authority_client_generation)=36
                           AND substr(authority_client_generation,9,1)='-'
                           AND substr(authority_client_generation,14,1)='-'
                           AND substr(authority_client_generation,15,1)='4'
                           AND substr(authority_client_generation,19,1)='-'
                           AND substr(authority_client_generation,20,1) GLOB '[89ab]'
                           AND substr(authority_client_generation,24,1)='-'
                           AND length(replace(authority_client_generation,'-',''))=32
                           AND replace(authority_client_generation,'-','') NOT GLOB '*[^0-9a-f]*')),
                    CHECK(selected_session_id IS NULL OR
                          (length(selected_session_id)=36
                           AND substr(selected_session_id,9,1)='-'
                           AND substr(selected_session_id,14,1)='-'
                           AND substr(selected_session_id,15,1)='4'
                           AND substr(selected_session_id,19,1)='-'
                           AND substr(selected_session_id,20,1) GLOB '[89ab]'
                           AND substr(selected_session_id,24,1)='-'
                           AND length(replace(selected_session_id,'-',''))=32
                           AND replace(selected_session_id,'-','') NOT GLOB '*[^0-9a-f]*')),
                    CHECK(length(nonce_sha256)=64 AND nonce_sha256 NOT GLOB '*[^0-9a-f]*'),
                    CHECK(length(request_fingerprint)=64 AND request_fingerprint NOT GLOB '*[^0-9a-f]*'),
                    CHECK(length(payload_sha256)=64 AND payload_sha256 NOT GLOB '*[^0-9a-f]*'),
                    CHECK(length(sender_incarnation_fingerprint)=64 AND sender_incarnation_fingerprint NOT GLOB '*[^0-9a-f]*'),
                    CHECK(confirmation_tag IS NULL OR (length(confirmation_tag)=64 AND confirmation_tag NOT GLOB '*[^0-9a-f]*')),
                    CHECK(sender_identity_fingerprint IS NULL OR (length(sender_identity_fingerprint)=64 AND sender_identity_fingerprint NOT GLOB '*[^0-9a-f]*')),
                    CHECK(target_identity_fingerprint IS NULL OR (length(target_identity_fingerprint)=64 AND target_identity_fingerprint NOT GLOB '*[^0-9a-f]*')),
                    CHECK(payload IS NULL OR length(payload)=payload_bytes),
                    CHECK(length(issued_at)=24 AND length(expires_at)=24 AND length(queued_at)=24
                          AND length(next_attempt_at)=24 AND length(updated_at)=24),
                    CHECK(issued_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9]Z'),
                    CHECK(expires_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9]Z'),
                    CHECK(queued_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9]Z'),
                    CHECK(next_attempt_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9]Z'),
                    CHECK(updated_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9]Z'),
                    CHECK(preparing_at IS NULL OR (length(preparing_at)=24 AND preparing_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9]Z')),
                    CHECK(actuating_at IS NULL OR (length(actuating_at)=24 AND actuating_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9]Z')),
                    CHECK(terminal_at IS NULL OR (length(terminal_at)=24 AND terminal_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9]Z')),
                    CHECK(host_artifact_at IS NULL OR (length(host_artifact_at)=24 AND host_artifact_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9]Z')),
                    CHECK((selected_session_id IS NULL) = (selected_backend IS NULL)),
                    CHECK(selected_backend IS NULL OR selected_backend IN ('localProcess','containerTransport')),
                    CHECK(reason_code IS NULL OR reason_code IN (
                        'invalid_envelope','mixed_payload','unsupported_version','invalid_enter_mode',
                        'invalid_id','invalid_nonce','invalid_timestamp','expired','invalid_target',
                        'invalid_text','payload_too_large','idempotency_conflict','capacity_exceeded',
                        'session_token_required','invalid_session_token','ambiguous_session_token',
                        'sender_session_not_live','sender_backend_not_local','sender_identity_invalid',
                        'sender_not_coordinator','root_identity_invalid','target_not_member',
                        'target_is_coordinator','target_out_of_scope','unsafe_path','api_scope_required',
                        'api_client_unbound','api_client_stale','api_binding_mismatch','authority_changed',
                        'busy','resize_unsettled','untracked_readiness','unsupported_session',
                        'nonpersistent_live_session','inconsistent_session','unsupported_profile',
                        'readiness_timeout','store_corrupt','restore_in_progress','purge_in_progress',
                        'session_race','lease_lost','spawn_failed_safe','store_transient',
                        'menu_guard_blocked',
                        'final_revalidation_failed','text_write_failed','required_enter_failed',
                        'daemon_restart_after_actuation','runtime_actuation_orphan','terminal_store_failed',
                        'redundant_enter_failed','boundary_metadata_failed','artifact_unclaimed'
                    )),
                    CHECK((reason_code IS NULL) = (reason_detail IS NULL)),
                    CHECK(
                        (status IN ('queued','preparing','retry')
                         AND payload IS NOT NULL
                         AND authority_session_id IS NOT NULL
                         AND sender_identity_fingerprint IS NOT NULL
                         AND target_identity_fingerprint IS NOT NULL
                         AND actuating_at IS NULL AND terminal_at IS NULL
                         AND selected_session_id IS NULL AND selected_backend IS NULL)
                        OR
                        (status='rejected' AND payload IS NULL AND requested_agent_id IS NULL
                         AND authority_session_id IS NULL AND authority_client_id IS NULL
                         AND authority_client_generation IS NULL
                         AND sender_identity_fingerprint IS NULL
                         AND target_identity_fingerprint IS NULL
                         AND actuating_at IS NULL AND terminal_at IS NOT NULL
                         AND selected_session_id IS NULL AND selected_backend IS NULL)
                        OR
                        (status IN ('actuating','injected','indeterminate')
                         AND payload IS NULL AND requested_agent_id IS NULL
                         AND authority_session_id IS NULL AND authority_client_id IS NULL
                         AND authority_client_generation IS NULL
                         AND sender_identity_fingerprint IS NULL
                         AND target_identity_fingerprint IS NULL
                         AND actuating_at IS NOT NULL
                         AND selected_session_id IS NOT NULL AND selected_backend IS NOT NULL)
                    ),
                    CHECK((source_plane='host_cli' AND confirmation_tag IS NOT NULL
                           AND authority_client_id IS NULL AND authority_client_generation IS NULL)
                       OR (source_plane='container_api' AND confirmation_tag IS NULL
                           AND ((status IN ('queued','preparing','retry')
                                 AND authority_client_id IS NOT NULL
                                 AND authority_client_generation IS NOT NULL)
                             OR (status IN ('actuating','injected','rejected','indeterminate')
                                 AND authority_client_id IS NULL
                                 AND authority_client_generation IS NULL)))),
                    CHECK((status IN ('injected','rejected','indeterminate')) = (terminal_at IS NOT NULL)),
                    CHECK((status = 'preparing') = (lease_owner IS NOT NULL AND lease_until IS NOT NULL)),
                    CHECK(status!='preparing' OR preparing_at IS NOT NULL),
                    CHECK(queued_at>=issued_at AND queued_at<expires_at),
                    CHECK(actuating_at IS NULL OR (actuating_at>=queued_at AND actuating_at<expires_at)),
                    CHECK(terminal_at IS NULL OR terminal_at>=queued_at),
                    CHECK(host_artifact_at IS NULL OR terminal_at IS NOT NULL),
                    CHECK((status IN ('queued','preparing','retry') AND
                           (reason_code IS NULL OR reason_code IN (
                             'restore_in_progress','purge_in_progress','session_race',
                             'lease_lost','spawn_failed_safe','store_transient',
                             'menu_guard_blocked')))
                       OR (status='actuating' AND reason_code IS NULL)
                       OR (status='injected' AND
                           (reason_code IS NULL OR reason_code IN (
                             'redundant_enter_failed','boundary_metadata_failed')))
                       OR (status='rejected' AND reason_code IS NOT NULL AND reason_code NOT IN (
                             'final_revalidation_failed','text_write_failed','required_enter_failed',
                             'daemon_restart_after_actuation','runtime_actuation_orphan',
                             'terminal_store_failed','redundant_enter_failed',
                             'boundary_metadata_failed','artifact_unclaimed'))
                       OR (status='indeterminate' AND reason_code IN (
                             'final_revalidation_failed','text_write_failed','required_enter_failed',
                             'daemon_restart_after_actuation','runtime_actuation_orphan',
                             'terminal_store_failed'))),
                    CHECK(source_plane != 'host_cli' OR injection_id=op_id)
                );
                INSERT INTO pty_input_operations_v3 SELECT * FROM pty_input_operations;
                DROP TABLE pty_input_operations;
                ALTER TABLE pty_input_operations_v3 RENAME TO pty_input_operations;
                CREATE INDEX idx_pty_input_due
                    ON pty_input_operations(source_plane, status, next_attempt_at, lease_until);

                CREATE TABLE pty_input_tombstones_v3(
                    injection_id TEXT PRIMARY KEY,
                    sender_fqn TEXT NOT NULL,
                    target_fqn TEXT NOT NULL,
                    op_id TEXT NOT NULL,
                    nonce_sha256 TEXT NOT NULL,
                    request_fingerprint TEXT NOT NULL,
                    confirmation_tag TEXT NULL,
                    sender_incarnation_fingerprint TEXT NOT NULL,
                    version INTEGER NOT NULL,
                    payload_sha256 TEXT NOT NULL,
                    payload_bytes INTEGER NOT NULL,
                    source_plane TEXT NOT NULL,
                    selected_session_id TEXT NULL,
                    selected_backend TEXT NULL,
                    issued_at TEXT NOT NULL,
                    expires_at TEXT NOT NULL,
                    queued_at TEXT NOT NULL,
                    actuating_at TEXT NULL,
                    terminal_at TEXT NOT NULL,
                    status TEXT NOT NULL CHECK(status IN ('injected','rejected','indeterminate')),
                    reason_code TEXT NULL,
                    reason_detail TEXT NULL,
                    UNIQUE(sender_fqn, op_id),
                    UNIQUE(sender_fqn, nonce_sha256),
                    CHECK(version=1),
                    CHECK(payload_bytes BETWEEN 1 AND 65536),
                    CHECK(length(injection_id)=36
                          AND substr(injection_id,9,1)='-'
                          AND substr(injection_id,14,1)='-'
                          AND substr(injection_id,15,1)='4'
                          AND substr(injection_id,19,1)='-'
                          AND substr(injection_id,20,1) GLOB '[89ab]'
                          AND substr(injection_id,24,1)='-'
                          AND length(replace(injection_id,'-',''))=32
                          AND replace(injection_id,'-','') NOT GLOB '*[^0-9a-f]*'),
                    CHECK(length(op_id)=36
                          AND substr(op_id,9,1)='-'
                          AND substr(op_id,14,1)='-'
                          AND substr(op_id,15,1)='4'
                          AND substr(op_id,19,1)='-'
                          AND substr(op_id,20,1) GLOB '[89ab]'
                          AND substr(op_id,24,1)='-'
                          AND length(replace(op_id,'-',''))=32
                          AND replace(op_id,'-','') NOT GLOB '*[^0-9a-f]*'),
                    CHECK(selected_session_id IS NULL OR
                          (length(selected_session_id)=36
                           AND substr(selected_session_id,9,1)='-'
                           AND substr(selected_session_id,14,1)='-'
                           AND substr(selected_session_id,15,1)='4'
                           AND substr(selected_session_id,19,1)='-'
                           AND substr(selected_session_id,20,1) GLOB '[89ab]'
                           AND substr(selected_session_id,24,1)='-'
                           AND length(replace(selected_session_id,'-',''))=32
                           AND replace(selected_session_id,'-','') NOT GLOB '*[^0-9a-f]*')),
                    CHECK(length(nonce_sha256)=64 AND nonce_sha256 NOT GLOB '*[^0-9a-f]*'),
                    CHECK(length(request_fingerprint)=64 AND request_fingerprint NOT GLOB '*[^0-9a-f]*'),
                    CHECK(length(sender_incarnation_fingerprint)=64 AND sender_incarnation_fingerprint NOT GLOB '*[^0-9a-f]*'),
                    CHECK(length(payload_sha256)=64 AND payload_sha256 NOT GLOB '*[^0-9a-f]*'),
                    CHECK(confirmation_tag IS NULL OR (length(confirmation_tag)=64 AND confirmation_tag NOT GLOB '*[^0-9a-f]*')),
                    CHECK((source_plane='host_cli' AND confirmation_tag IS NOT NULL AND injection_id=op_id)
                       OR (source_plane='container_api' AND confirmation_tag IS NULL)),
                    CHECK((selected_session_id IS NULL) = (selected_backend IS NULL)),
                    CHECK(selected_backend IS NULL OR selected_backend IN ('localProcess','containerTransport')),
                    CHECK(length(issued_at)=24 AND issued_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9]Z'),
                    CHECK(length(expires_at)=24 AND expires_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9]Z'),
                    CHECK(length(queued_at)=24 AND queued_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9]Z'),
                    CHECK(length(terminal_at)=24 AND terminal_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9]Z'),
                    CHECK(actuating_at IS NULL OR (length(actuating_at)=24 AND actuating_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9]Z')),
                    CHECK(queued_at>=issued_at AND queued_at<expires_at),
                    CHECK(actuating_at IS NULL OR (actuating_at>=queued_at AND actuating_at<expires_at)),
                    CHECK(terminal_at>=queued_at),
                    CHECK((reason_code IS NULL) = (reason_detail IS NULL)),
                    CHECK(reason_code IS NULL OR reason_code IN (
                        'invalid_envelope','mixed_payload','unsupported_version','invalid_enter_mode',
                        'invalid_id','invalid_nonce','invalid_timestamp','expired','invalid_target',
                        'invalid_text','payload_too_large','idempotency_conflict','capacity_exceeded',
                        'session_token_required','invalid_session_token','ambiguous_session_token',
                        'sender_session_not_live','sender_backend_not_local','sender_identity_invalid',
                        'sender_not_coordinator','root_identity_invalid','target_not_member',
                        'target_is_coordinator','target_out_of_scope','unsafe_path','api_scope_required',
                        'api_client_unbound','api_client_stale','api_binding_mismatch','authority_changed',
                        'busy','resize_unsettled','untracked_readiness','unsupported_session',
                        'nonpersistent_live_session','inconsistent_session','unsupported_profile',
                        'readiness_timeout','store_corrupt','restore_in_progress','purge_in_progress',
                        'session_race','lease_lost','spawn_failed_safe','store_transient',
                        'menu_guard_blocked',
                        'final_revalidation_failed','text_write_failed','required_enter_failed',
                        'daemon_restart_after_actuation','runtime_actuation_orphan','terminal_store_failed',
                        'redundant_enter_failed','boundary_metadata_failed','artifact_unclaimed'
                    )),
                    CHECK((status='injected' AND actuating_at IS NOT NULL
                           AND selected_session_id IS NOT NULL
                           AND (reason_code IS NULL OR reason_code IN (
                             'redundant_enter_failed','boundary_metadata_failed')))
                       OR (status='rejected' AND actuating_at IS NULL
                           AND selected_session_id IS NULL
                           AND reason_code IS NOT NULL AND reason_code NOT IN (
                             'final_revalidation_failed','text_write_failed','required_enter_failed',
                             'daemon_restart_after_actuation','runtime_actuation_orphan',
                             'terminal_store_failed','redundant_enter_failed',
                             'boundary_metadata_failed','artifact_unclaimed'))
                       OR (status='indeterminate' AND actuating_at IS NOT NULL
                           AND selected_session_id IS NOT NULL
                           AND reason_code IN (
                             'final_revalidation_failed','text_write_failed','required_enter_failed',
                             'daemon_restart_after_actuation','runtime_actuation_orphan',
                             'terminal_store_failed')))
                );
                INSERT INTO pty_input_tombstones_v3 SELECT * FROM pty_input_tombstones;
                DROP TABLE pty_input_tombstones;
                ALTER TABLE pty_input_tombstones_v3 RENAME TO pty_input_tombstones;
                "#,
            )?;
            tx.execute(
                "INSERT INTO api_message_schema(version, applied_at) VALUES(3, ?1)",
                [crate::phone::types::canonical_pty_timestamp(Utc::now())],
            )?;
            let _ = schema_version;
        }
        tx.commit()?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        Ok(())
    }

    pub fn enqueue(&self, req: EnqueueRequest) -> Result<EnqueueResult, MessageStoreError> {
        if req.body.len() > INLINE_BODY_MAX_BYTES {
            return Err(MessageStoreError::BodyTooLarge);
        }

        let now = Utc::now().to_rfc3339();
        let message_id = uuid::Uuid::new_v4().to_string();
        let body_sha256 = sha256_hex(req.body.as_bytes());
        let body_bytes = req.body.len() as i64;

        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let inserted = tx.execute(
            r#"
            INSERT OR IGNORE INTO messages(
                message_id, sender_fqn, target_fqn, op_id, content_type, body,
                body_sha256, body_bytes, source_plane, source_ref, status,
                attempt, next_attempt_at, created_at
            )
            VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, 0, ?12, ?13)
            "#,
            params![
                message_id,
                req.sender_fqn,
                req.target_fqn,
                req.op_id,
                req.content_type,
                req.body,
                body_sha256,
                body_bytes,
                req.source_plane,
                req.source_ref,
                STATUS_QUEUED,
                now,
                now,
            ],
        )?;

        let result = tx.query_row(
            r#"
            SELECT message_id, sender_fqn, target_fqn, op_id, status
            FROM messages
            WHERE sender_fqn = ?1 AND op_id = ?2
            "#,
            params![req.sender_fqn, req.op_id],
            |row| {
                Ok(EnqueueResult {
                    message_id: row.get(0)?,
                    sender_fqn: row.get(1)?,
                    target_fqn: row.get(2)?,
                    op_id: row.get(3)?,
                    status: row.get(4)?,
                    duplicate: inserted == 0,
                })
            },
        )?;
        if inserted > 0 {
            insert_audit(&tx, &result.message_id, STATUS_QUEUED, None, &now)?;
        }
        tx.commit()?;
        Ok(result)
    }

    pub fn lease_due(
        &self,
        now: DateTime<Utc>,
        limit: usize,
        lease_for: Duration,
        lease_owner: &str,
    ) -> Result<Vec<LeasedMessage>, MessageStoreError> {
        let now_s = now.to_rfc3339();
        let lease_until =
            (now + chrono::Duration::from_std(lease_for).unwrap_or_default()).to_rfc3339();
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let ids = {
            let mut stmt = tx.prepare(
                r#"
                SELECT message_id
                FROM messages
                WHERE (
                    status IN (?1, ?2) AND next_attempt_at <= ?3
                ) OR (
                    status = ?4 AND lease_until IS NOT NULL AND lease_until <= ?3
                )
                ORDER BY created_at ASC
                LIMIT ?5
                "#,
            )?;
            let rows = stmt.query_map(
                params![
                    STATUS_QUEUED,
                    STATUS_RETRY,
                    &now_s,
                    STATUS_DELIVERING,
                    limit as i64
                ],
                |row| row.get::<_, String>(0),
            )?;
            rows.collect::<Result<Vec<_>, _>>()?
        };

        let mut leased = Vec::with_capacity(ids.len());
        for id in ids {
            let changed = tx.execute(
                r#"
                UPDATE messages
                SET status = ?1, lease_owner = ?2, lease_until = ?3, last_error = NULL
                WHERE message_id = ?4
                  AND (
                    (status IN (?5, ?6) AND next_attempt_at <= ?7)
                    OR (status = ?8 AND lease_until IS NOT NULL AND lease_until <= ?7)
                  )
                "#,
                params![
                    STATUS_DELIVERING,
                    lease_owner,
                    &lease_until,
                    &id,
                    STATUS_QUEUED,
                    STATUS_RETRY,
                    &now_s,
                    STATUS_DELIVERING,
                ],
            )?;
            if changed == 0 {
                continue;
            }
            let msg = tx.query_row(
                r#"
                SELECT message_id, sender_fqn, target_fqn, op_id, content_type, body, attempt
                FROM messages
                WHERE message_id = ?1
                "#,
                [id],
                |row| {
                    Ok(LeasedMessage {
                        message_id: row.get(0)?,
                        sender_fqn: row.get(1)?,
                        target_fqn: row.get(2)?,
                        op_id: row.get(3)?,
                        content_type: row.get(4)?,
                        body: row.get(5)?,
                        attempt: row.get(6)?,
                    })
                },
            )?;
            leased.push(msg);
        }
        tx.commit()?;
        Ok(leased)
    }

    pub fn mark_delivered(
        &self,
        message_id: &str,
        now: DateTime<Utc>,
    ) -> Result<(), MessageStoreError> {
        let now_s = now.to_rfc3339();
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        tx.execute(
            r#"
            UPDATE messages
            SET status = ?1, delivered_at = ?2, lease_owner = NULL, lease_until = NULL,
                last_error = NULL
            WHERE message_id = ?3
            "#,
            params![STATUS_DELIVERED, now_s, message_id],
        )?;
        insert_audit(&tx, message_id, STATUS_DELIVERED, None, &now_s)?;
        tx.commit()?;
        Ok(())
    }

    pub fn mark_delivery_failed(
        &self,
        message_id: &str,
        error: &str,
        now: DateTime<Utc>,
        max_attempts: i64,
    ) -> Result<String, MessageStoreError> {
        let now_s = now.to_rfc3339();
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let current_attempt: i64 = tx.query_row(
            "SELECT attempt FROM messages WHERE message_id = ?1",
            [message_id],
            |row| row.get(0),
        )?;
        let next_attempt = current_attempt + 1;
        let next_status = if next_attempt >= max_attempts {
            STATUS_POISONED
        } else {
            STATUS_RETRY
        };
        let backoff = retry_backoff_seconds(next_attempt);
        let next_attempt_at = (now + chrono::Duration::seconds(backoff)).to_rfc3339();
        tx.execute(
            r#"
            UPDATE messages
            SET status = ?1, attempt = ?2, next_attempt_at = ?3,
                lease_owner = NULL, lease_until = NULL, last_error = ?4
            WHERE message_id = ?5
            "#,
            params![
                next_status,
                next_attempt,
                next_attempt_at,
                error,
                message_id
            ],
        )?;
        insert_audit(&tx, message_id, next_status, Some(error), &now_s)?;
        tx.commit()?;
        Ok(next_status.to_string())
    }

    pub fn release_delivery_lease(
        &self,
        message_id: &str,
        reason: &str,
        now: DateTime<Utc>,
    ) -> Result<(), MessageStoreError> {
        let now_s = now.to_rfc3339();
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        tx.execute(
            r#"
            UPDATE messages
            SET status = ?1, next_attempt_at = ?2,
                lease_owner = NULL, lease_until = NULL, last_error = ?3
            WHERE message_id = ?4
            "#,
            params![STATUS_QUEUED, now_s, reason, message_id],
        )?;
        insert_audit(
            &tx,
            message_id,
            "lease-released-deferred",
            Some(reason),
            &now_s,
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn reap_terminal_before(&self, cutoff: DateTime<Utc>) -> Result<usize, MessageStoreError> {
        let cutoff_s = cutoff.to_rfc3339();
        let conn = self.conn.lock().unwrap();
        let deleted = conn.execute(
            r#"
            DELETE FROM messages
            WHERE (
                status = ?1 AND delivered_at IS NOT NULL AND delivered_at < ?2
            ) OR (
                status = ?3 AND next_attempt_at < ?2
            )
            "#,
            params![STATUS_DELIVERED, cutoff_s, STATUS_POISONED],
        )?;
        Ok(deleted)
    }

    fn recover_pty_input_startup(&self) -> Result<(), MessageStoreError> {
        let now = Utc::now();
        let now_s = crate::phone::types::canonical_pty_timestamp(now);
        let mut cursor: Option<(String, String)> = None;
        loop {
            let candidates = {
                let conn = self
                    .conn
                    .lock()
                    .map_err(|_| MessageStoreError::StoreCorrupt)?;
                let mut statement = conn.prepare(
                    r#"SELECT injection_id,status,expires_at,attempt,updated_at
                       FROM pty_input_operations
                       WHERE (status IN ('preparing','actuating')
                              OR (status IN ('queued','retry') AND expires_at <= ?1))
                         AND (?2 IS NULL OR updated_at > ?2
                              OR (updated_at = ?2 AND injection_id > ?3))
                       ORDER BY updated_at,injection_id LIMIT 64"#,
                )?;
                let (cursor_at, cursor_id) = cursor
                    .as_ref()
                    .map(|(at, id)| (Some(at.as_str()), Some(id.as_str())))
                    .unwrap_or((None, None));
                let rows = statement.query_map(params![now_s, cursor_at, cursor_id], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, i64>(3)?,
                        row.get::<_, String>(4)?,
                    ))
                })?;
                rows.collect::<Result<Vec<_>, _>>()?
            };
            let candidate_count = candidates.len();
            for (injection_id, status, expires_at, attempt, _) in &candidates {
                let Some(_operation_lock) = self.try_operation_lock(injection_id)? else {
                    continue;
                };
                if status == PTY_STATUS_ACTUATING {
                    self.terminalize_pty_input(
                        injection_id,
                        crate::phone::types::PtyInputPublicStatus::Indeterminate,
                        Some(crate::phone::types::PtyInputReasonCode::DaemonRestartAfterActuation),
                        now,
                    )?;
                } else if expires_at <= &now_s {
                    self.terminalize_pty_input(
                        injection_id,
                        crate::phone::types::PtyInputPublicStatus::Rejected,
                        Some(crate::phone::types::PtyInputReasonCode::Expired),
                        now,
                    )?;
                } else if status == PTY_STATUS_PREPARING && *attempt >= 5 {
                    self.terminalize_pty_input(
                        injection_id,
                        crate::phone::types::PtyInputPublicStatus::Rejected,
                        Some(crate::phone::types::PtyInputReasonCode::LeaseLost),
                        now,
                    )?;
                } else if status == PTY_STATUS_PREPARING {
                    let mut conn = self
                        .conn
                        .lock()
                        .map_err(|_| MessageStoreError::StoreCorrupt)?;
                    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
                    let changed = tx.execute(
                        r#"UPDATE pty_input_operations SET status='retry',lease_owner=NULL,
                             lease_until=NULL,next_attempt_at=?1,updated_at=?1,
                             reason_code=?2,reason_detail=?3
                           WHERE injection_id=?4 AND status='preparing' AND attempt < 5"#,
                        params![
                            now_s,
                            reason_code_string(crate::phone::types::PtyInputReasonCode::LeaseLost),
                            crate::phone::types::safe_detail(
                                crate::phone::types::PtyInputReasonCode::LeaseLost
                            ),
                            injection_id
                        ],
                    )?;
                    if changed != 1 {
                        return Err(MessageStoreError::InvalidTransition);
                    }
                    insert_pty_audit(
                        &tx,
                        injection_id,
                        PTY_STATUS_RETRY,
                        Some(crate::phone::types::PtyInputReasonCode::LeaseLost),
                        &now_s,
                    )?;
                    tx.commit()?;
                }
            }
            cursor = candidates
                .last()
                .map(|(id, _, _, _, updated_at)| (updated_at.clone(), id.clone()));
            if candidate_count < 64 {
                break;
            }
        }
        Ok(())
    }

    pub fn recover_pty_input_runtime(
        &self,
        active: &HashSet<String>,
        now: DateTime<Utc>,
    ) -> Result<usize, MessageStoreError> {
        let now_s = crate::phone::types::canonical_pty_timestamp(now);
        let orphan_cutoff =
            crate::phone::types::canonical_pty_timestamp(now - chrono::Duration::seconds(15));
        let cursor = self
            .maintenance_cursors
            .lock()
            .map_err(|_| MessageStoreError::StoreCorrupt)?
            .runtime_recovery
            .clone();
        let candidates = {
            let conn = self
                .conn
                .lock()
                .map_err(|_| MessageStoreError::StoreCorrupt)?;
            let mut statement = conn.prepare(
                r#"SELECT injection_id,status,expires_at,attempt,lease_until,updated_at
                   FROM pty_input_operations
                   WHERE ((status='actuating' AND actuating_at < ?1)
                      OR (status IN ('queued','preparing','retry') AND expires_at <= ?2)
                      OR (status='preparing' AND lease_until <= ?2))
                     AND (?3 IS NULL OR updated_at > ?3
                       OR (updated_at = ?3 AND injection_id > ?4))
                   ORDER BY updated_at,injection_id LIMIT 64"#,
            )?;
            let (cursor_at, cursor_id) = cursor
                .as_ref()
                .map(|(at, id)| (Some(at.as_str()), Some(id.as_str())))
                .unwrap_or((None, None));
            let rows = statement.query_map(
                params![orphan_cutoff, now_s, cursor_at, cursor_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, i64>(3)?,
                        row.get::<_, Option<String>>(4)?,
                        row.get::<_, String>(5)?,
                    ))
                },
            )?;
            rows.collect::<Result<Vec<_>, _>>()?
        };
        if candidates.is_empty() && cursor.is_some() {
            self.maintenance_cursors
                .lock()
                .map_err(|_| MessageStoreError::StoreCorrupt)?
                .runtime_recovery = None;
            return self.recover_pty_input_runtime(active, now);
        }
        self.maintenance_cursors
            .lock()
            .map_err(|_| MessageStoreError::StoreCorrupt)?
            .runtime_recovery = candidates
            .last()
            .map(|(id, _, _, _, _, updated_at)| (updated_at.clone(), id.clone()));
        let mut recovered = 0;
        for (injection_id, status, expires_at, attempt, lease_until, _) in candidates {
            if active.contains(&injection_id) {
                continue;
            }
            let Some(_operation_lock) = self.try_operation_lock(&injection_id)? else {
                continue;
            };
            if expires_at <= now_s && status != PTY_STATUS_ACTUATING {
                self.terminalize_pty_input(
                    &injection_id,
                    crate::phone::types::PtyInputPublicStatus::Rejected,
                    Some(crate::phone::types::PtyInputReasonCode::Expired),
                    now,
                )?;
                recovered += 1;
            } else if status == PTY_STATUS_ACTUATING {
                self.terminalize_pty_input(
                    &injection_id,
                    crate::phone::types::PtyInputPublicStatus::Indeterminate,
                    Some(crate::phone::types::PtyInputReasonCode::RuntimeActuationOrphan),
                    now,
                )?;
                recovered += 1;
            } else if status == PTY_STATUS_PREPARING
                && lease_until
                    .as_deref()
                    .is_some_and(|lease| lease <= now_s.as_str())
            {
                if attempt >= 5 {
                    self.terminalize_pty_input(
                        &injection_id,
                        crate::phone::types::PtyInputPublicStatus::Rejected,
                        Some(crate::phone::types::PtyInputReasonCode::LeaseLost),
                        now,
                    )?;
                } else {
                    let mut conn = self
                        .conn
                        .lock()
                        .map_err(|_| MessageStoreError::StoreCorrupt)?;
                    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
                    let changed = tx.execute(
                        r#"UPDATE pty_input_operations
                           SET status='retry',lease_owner=NULL,lease_until=NULL,
                               next_attempt_at=?1,updated_at=?1,reason_code=?2,reason_detail=?3
                           WHERE injection_id=?4 AND status='preparing' AND lease_until <= ?1"#,
                        params![
                            now_s,
                            reason_code_string(crate::phone::types::PtyInputReasonCode::LeaseLost),
                            crate::phone::types::safe_detail(
                                crate::phone::types::PtyInputReasonCode::LeaseLost
                            ),
                            injection_id
                        ],
                    )?;
                    if changed != 1 {
                        return Err(MessageStoreError::InvalidTransition);
                    }
                    insert_pty_audit(
                        &tx,
                        &injection_id,
                        PTY_STATUS_RETRY,
                        Some(crate::phone::types::PtyInputReasonCode::LeaseLost),
                        &now_s,
                    )?;
                    tx.commit()?;
                }
                recovered += 1;
            }
        }
        Ok(recovered)
    }

    pub fn try_operation_lock(
        &self,
        injection_id: &str,
    ) -> Result<Option<PtyInputStripeGuard>, MessageStoreError> {
        self.try_stripe_lock(injection_id, true)
    }

    pub fn try_target_lock(
        &self,
        target_fqn: &str,
    ) -> Result<Option<PtyInputStripeGuard>, MessageStoreError> {
        self.try_stripe_lock(target_fqn, false)
    }

    pub async fn acquire_target_lock(
        &self,
        target_fqn: &str,
    ) -> Result<PtyInputStripeGuard, MessageStoreError> {
        loop {
            if let Some(guard) = self.try_target_lock(target_fqn)? {
                return Ok(guard);
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    fn try_stripe_lock(
        &self,
        value: &str,
        operation: bool,
    ) -> Result<Option<PtyInputStripeGuard>, MessageStoreError> {
        let parent = self.path.parent().ok_or(MessageStoreError::UnsafePath)?;
        try_stripe_lock_at(parent, value, operation)
    }

    fn expire_pty_input_for_admission_page(
        &self,
        now: DateTime<Utc>,
    ) -> Result<(usize, usize), MessageStoreError> {
        let now_s = crate::phone::types::canonical_pty_timestamp(now);
        let cursor = self
            .maintenance_cursors
            .lock()
            .map_err(|_| MessageStoreError::StoreCorrupt)?
            .admission_expiry
            .clone();
        let candidates = {
            let conn = self
                .conn
                .lock()
                .map_err(|_| MessageStoreError::StoreCorrupt)?;
            let mut statement = conn.prepare(
                r#"SELECT injection_id,expires_at FROM pty_input_operations
                   WHERE status IN ('queued','preparing','retry') AND expires_at <= ?1
                     AND (?2 IS NULL OR expires_at > ?2
                       OR (expires_at = ?2 AND injection_id > ?3))
                   ORDER BY expires_at,injection_id LIMIT 64"#,
            )?;
            let (cursor_at, cursor_id) = cursor
                .as_ref()
                .map(|(at, id)| (Some(at.as_str()), Some(id.as_str())))
                .unwrap_or((None, None));
            let rows = statement.query_map(params![now_s, cursor_at, cursor_id], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?;
            rows.collect::<Result<Vec<_>, _>>()?
        };
        if candidates.is_empty() && cursor.is_some() {
            self.maintenance_cursors
                .lock()
                .map_err(|_| MessageStoreError::StoreCorrupt)?
                .admission_expiry = None;
            return self.expire_pty_input_for_admission_page(now);
        }
        self.maintenance_cursors
            .lock()
            .map_err(|_| MessageStoreError::StoreCorrupt)?
            .admission_expiry = candidates
            .last()
            .map(|(id, expires_at)| (expires_at.clone(), id.clone()));
        let scanned = candidates.len();
        let mut expired = 0;
        for (injection_id, _) in candidates {
            let Some(_operation_lock) = self.try_operation_lock(&injection_id)? else {
                continue;
            };
            self.terminalize_pty_input(
                &injection_id,
                crate::phone::types::PtyInputPublicStatus::Rejected,
                Some(crate::phone::types::PtyInputReasonCode::Expired),
                now,
            )?;
            expired += 1;
        }
        Ok((expired, scanned))
    }

    #[cfg(test)]
    fn expire_pty_input_for_admission(
        &self,
        now: DateTime<Utc>,
    ) -> Result<usize, MessageStoreError> {
        self.expire_pty_input_for_admission_page(now)
            .map(|(expired, _)| expired)
    }

    pub fn enqueue_pty_input(
        &self,
        req: PtyInputEnqueueRequest,
    ) -> Result<PtyInputEnqueueResult, MessageStoreError> {
        use crate::phone::types::{
            parse_canonical_pty_timestamp, parse_canonical_uuid_v4, PtyInputSourcePlane,
        };
        parse_canonical_uuid_v4(&req.injection_id).map_err(|_| MessageStoreError::StoreCorrupt)?;
        parse_canonical_uuid_v4(&req.op_id).map_err(|_| MessageStoreError::StoreCorrupt)?;
        let issued_at = parse_canonical_pty_timestamp(&req.issued_at)
            .map_err(|_| MessageStoreError::StoreCorrupt)?;
        let expires_at = parse_canonical_pty_timestamp(&req.expires_at)
            .map_err(|_| MessageStoreError::StoreCorrupt)?;
        if expires_at - issued_at != chrono::Duration::minutes(10) {
            return Err(MessageStoreError::StoreCorrupt);
        }
        let text =
            std::str::from_utf8(&req.payload).map_err(|_| MessageStoreError::StoreCorrupt)?;
        crate::pty::inject::validate_pty_input_text(text)
            .map_err(|_| MessageStoreError::StoreCorrupt)?;
        let source_plane = match req.source_plane {
            PtyInputSourcePlane::HostCli => "host_cli",
            PtyInputSourcePlane::ContainerApi => "container_api",
        };
        if !is_lower_hex_64(&req.nonce_sha256)
            || !is_lower_hex_64(&req.request_fingerprint)
            || !is_lower_hex_64(&req.sender_incarnation_fingerprint)
            || !is_lower_hex_64(&req.sender_identity_fingerprint)
            || !is_lower_hex_64(&req.target_identity_fingerprint)
            || crate::phone::types::parse_canonical_uuid_v4(&req.authority_session_id).is_err()
        {
            return Err(MessageStoreError::StoreCorrupt);
        }
        match req.source_plane {
            PtyInputSourcePlane::HostCli
                if req.confirmation_tag.as_deref().is_some_and(is_lower_hex_64)
                    && req.authority_client_id.is_none()
                    && req.authority_client_generation.is_none()
                    && req.injection_id == req.op_id => {}
            PtyInputSourcePlane::ContainerApi
                if req.confirmation_tag.is_none()
                    && req
                        .authority_client_id
                        .as_deref()
                        .is_some_and(|id| !id.is_empty())
                    && req
                        .authority_client_generation
                        .as_deref()
                        .is_some_and(|generation| {
                            crate::phone::types::parse_canonical_uuid_v4(generation).is_ok()
                        }) => {}
            _ => return Err(MessageStoreError::StoreCorrupt),
        }
        let payload_sha256 = sha256_hex(&req.payload);
        let payload_bytes =
            i64::try_from(req.payload.len()).map_err(|_| MessageStoreError::StoreCorrupt)?;
        let wall_now = Utc::now();
        for _ in 0..8 {
            let (_, scanned) = self.expire_pty_input_for_admission_page(wall_now)?;
            if scanned < 64 {
                break;
            }
        }
        // Host envelopes allow a small future skew. Queue at the issued instant
        // in that case so the row remains temporally coherent and is not due
        // before its authenticated issue time.
        let queued_at = wall_now.max(issued_at);
        let now = crate::phone::types::canonical_pty_timestamp(queued_at);

        let mut conn = self
            .conn
            .lock()
            .map_err(|_| MessageStoreError::StoreCorrupt)?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;

        if let Some(existing) = lookup_idempotency_row(&tx, &req.sender_fqn, &req.op_id)? {
            if existing.request_fingerprint != req.request_fingerprint
                || existing.sender_incarnation_fingerprint != req.sender_incarnation_fingerprint
            {
                return Err(MessageStoreError::IdempotencyConflict);
            }
            let result = load_pty_result(&tx, &existing.injection_id)?
                .ok_or(MessageStoreError::StoreCorrupt)?;
            tx.commit()?;
            return Ok(PtyInputEnqueueResult {
                result,
                duplicate: true,
            });
        }

        let (sender_count, global_count, aggregate_bytes): (i64, i64, i64) = tx.query_row(
            r#"
            SELECT
                COALESCE(SUM(CASE WHEN sender_fqn = ?1 THEN 1 ELSE 0 END), 0),
                COUNT(*),
                COALESCE(SUM(payload_bytes), 0)
            FROM pty_input_operations
            WHERE status IN ('queued','preparing','retry','actuating')
            "#,
            [&req.sender_fqn],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;
        if sender_count >= PTY_INPUT_MAX_NONTERMINAL_PER_SENDER
            || global_count >= PTY_INPUT_MAX_NONTERMINAL_GLOBAL
            || aggregate_bytes.saturating_add(payload_bytes) > PTY_INPUT_MAX_NONTERMINAL_BYTES
        {
            return Err(MessageStoreError::CapacityExceeded);
        }

        let collision: bool = tx.query_row(
            r#"
            SELECT EXISTS(
                SELECT 1 FROM pty_input_operations
                WHERE injection_id = ?1 OR (sender_fqn = ?2 AND nonce_sha256 = ?3)
                UNION ALL
                SELECT 1 FROM pty_input_tombstones
                WHERE injection_id = ?1 OR (sender_fqn = ?2 AND nonce_sha256 = ?3)
            )
            "#,
            params![req.injection_id, req.sender_fqn, req.nonce_sha256],
            |row| row.get(0),
        )?;
        if collision {
            return Err(MessageStoreError::IdempotencyConflict);
        }

        tx.execute(
            r#"
            INSERT INTO pty_input_operations(
                injection_id, sender_fqn, target_fqn, op_id, nonce_sha256,
                request_fingerprint, confirmation_tag, version, enter_mode,
                requested_agent_id, payload, payload_sha256, payload_bytes,
                source_plane, sender_incarnation_fingerprint,
                sender_identity_fingerprint, target_identity_fingerprint,
                authority_session_id, authority_client_id,
                authority_client_generation, status, attempt, next_attempt_at,
                issued_at, expires_at, queued_at, updated_at
            ) VALUES(
                ?1,?2,?3,?4,?5,?6,?7,1,'agent-submit',?8,?9,?10,?11,
                ?12,?13,?14,?15,?16,?17,?18,'queued',0,?19,?20,?21,?19,?19
            )
            "#,
            params![
                req.injection_id,
                req.sender_fqn,
                req.target_fqn,
                req.op_id,
                req.nonce_sha256,
                req.request_fingerprint,
                req.confirmation_tag,
                req.requested_agent_id,
                req.payload,
                payload_sha256,
                payload_bytes,
                source_plane,
                req.sender_incarnation_fingerprint,
                req.sender_identity_fingerprint,
                req.target_identity_fingerprint,
                req.authority_session_id,
                req.authority_client_id,
                req.authority_client_generation,
                now,
                req.issued_at,
                req.expires_at,
            ],
        )?;
        insert_pty_audit(&tx, &req.injection_id, PTY_STATUS_QUEUED, None, &now)?;
        let result =
            load_pty_result(&tx, &req.injection_id)?.ok_or(MessageStoreError::StoreCorrupt)?;
        tx.commit()?;
        Ok(PtyInputEnqueueResult {
            result,
            duplicate: false,
        })
    }

    pub fn record_host_pty_input_rejection(
        &self,
        req: HostPtyInputRejectionRequest,
        now: DateTime<Utc>,
    ) -> Result<crate::phone::types::PtyInputResult, MessageStoreError> {
        use crate::phone::types::{
            parse_canonical_pty_timestamp, parse_canonical_uuid_v4,
            pty_input_reason_allowed_for_status, PtyInputPublicStatus,
        };
        parse_canonical_uuid_v4(&req.injection_id).map_err(|_| MessageStoreError::StoreCorrupt)?;
        parse_canonical_uuid_v4(&req.op_id).map_err(|_| MessageStoreError::StoreCorrupt)?;
        let issued = parse_canonical_pty_timestamp(&req.issued_at)
            .map_err(|_| MessageStoreError::StoreCorrupt)?;
        let expires = parse_canonical_pty_timestamp(&req.expires_at)
            .map_err(|_| MessageStoreError::StoreCorrupt)?;
        if req.injection_id != req.op_id
            || expires - issued != chrono::Duration::minutes(10)
            || req.sender_fqn.is_empty()
            || req.sender_fqn.chars().any(char::is_control)
            || req.target_fqn.is_empty()
            || req.target_fqn.chars().any(char::is_control)
            || !is_lower_hex_64(&req.nonce_sha256)
            || !is_lower_hex_64(&req.request_fingerprint)
            || !is_lower_hex_64(&req.confirmation_tag)
            || !is_lower_hex_64(&req.sender_incarnation_fingerprint)
            || !is_lower_hex_64(&req.payload_sha256)
            || !(1..=crate::pty::backend::PTY_INPUT_MAX_BYTES as u64).contains(&req.payload_bytes)
            || !pty_input_reason_allowed_for_status(
                PtyInputPublicStatus::Rejected,
                Some(req.reason),
            )
        {
            return Err(MessageStoreError::StoreCorrupt);
        }
        let Some(_operation_lock) = self.try_operation_lock(&req.injection_id)? else {
            return Err(MessageStoreError::InvalidTransition);
        };
        let queued_at = req.issued_at.clone();
        let terminal_at = crate::phone::types::canonical_pty_timestamp(now.max(issued));
        let payload_bytes =
            i64::try_from(req.payload_bytes).map_err(|_| MessageStoreError::StoreCorrupt)?;
        let mut conn = self
            .conn
            .lock()
            .map_err(|_| MessageStoreError::StoreCorrupt)?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;

        if let Some(existing) = lookup_idempotency_row(&tx, &req.sender_fqn, &req.op_id)? {
            if existing.injection_id == req.injection_id
                && existing.request_fingerprint == req.request_fingerprint
                && existing.sender_incarnation_fingerprint == req.sender_incarnation_fingerprint
            {
                let result = load_pty_result(&tx, &existing.injection_id)?
                    .ok_or(MessageStoreError::StoreCorrupt)?;
                if result.status == PtyInputPublicStatus::Rejected
                    && result.reason.as_ref().map(|reason| reason.code) == Some(req.reason)
                {
                    tx.commit()?;
                    return Ok(result);
                }
            }
            return Err(MessageStoreError::IdempotencyConflict);
        }
        let collision: bool = tx.query_row(
            r#"SELECT EXISTS(
                 SELECT 1 FROM pty_input_operations
                 WHERE injection_id=?1 OR (sender_fqn=?2 AND nonce_sha256=?3)
                 UNION ALL
                 SELECT 1 FROM pty_input_tombstones
                 WHERE injection_id=?1 OR (sender_fqn=?2 AND nonce_sha256=?3)
               )"#,
            params![req.injection_id, req.sender_fqn, req.nonce_sha256],
            |row| row.get(0),
        )?;
        if collision {
            return Err(MessageStoreError::IdempotencyConflict);
        }
        tx.execute(
            r#"INSERT INTO pty_input_tombstones(
                 injection_id,sender_fqn,target_fqn,op_id,nonce_sha256,
                 request_fingerprint,confirmation_tag,sender_incarnation_fingerprint,
                 version,payload_sha256,payload_bytes,source_plane,
                 selected_session_id,selected_backend,issued_at,expires_at,queued_at,
                 actuating_at,terminal_at,status,reason_code,reason_detail
               ) VALUES(
                 ?1,?2,?3,?4,?5,?6,?7,?8,1,?9,?10,'host_cli',
                 NULL,NULL,?11,?12,?13,NULL,?14,'rejected',?15,?16
               )"#,
            params![
                req.injection_id,
                req.sender_fqn,
                req.target_fqn,
                req.op_id,
                req.nonce_sha256,
                req.request_fingerprint,
                req.confirmation_tag,
                req.sender_incarnation_fingerprint,
                req.payload_sha256,
                payload_bytes,
                req.issued_at,
                req.expires_at,
                queued_at,
                terminal_at,
                reason_code_string(req.reason),
                crate::phone::types::safe_detail(req.reason),
            ],
        )?;
        let result =
            load_pty_result(&tx, &req.injection_id)?.ok_or(MessageStoreError::StoreCorrupt)?;
        tx.commit()?;
        Ok(result)
    }

    pub fn due_pty_input_ids(
        &self,
        source_plane: crate::phone::types::PtyInputSourcePlane,
        now: DateTime<Utc>,
        limit: usize,
    ) -> Result<Vec<String>, MessageStoreError> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| MessageStoreError::StoreCorrupt)?;
        let now = crate::phone::types::canonical_pty_timestamp(now);
        let mut statement = conn.prepare(
            r#"SELECT injection_id FROM pty_input_operations
               WHERE source_plane=?1
                 AND ((status IN ('queued','retry') AND next_attempt_at <= ?2)
                   OR (status='preparing' AND lease_until <= ?2))
                 AND attempt < 5
               ORDER BY next_attempt_at,queued_at,injection_id LIMIT ?3"#,
        )?;
        let limit = i64::try_from(limit.min(64)).map_err(|_| MessageStoreError::StoreCorrupt)?;
        let rows = statement
            .query_map(params![source_plane_str(source_plane), now, limit], |row| {
                row.get::<_, String>(0)
            })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub(crate) fn due_container_pty_input_candidates_fair(
        &self,
        now: DateTime<Utc>,
        limit: usize,
    ) -> Result<Vec<DuePtyInputCandidate>, MessageStoreError> {
        let query_time = now;
        let now = crate::phone::types::canonical_pty_timestamp(now);
        let cursor = self
            .maintenance_cursors
            .lock()
            .map_err(|_| MessageStoreError::StoreCorrupt)?
            .due_container
            .clone();
        let rows = {
            let conn = self
                .conn
                .lock()
                .map_err(|_| MessageStoreError::StoreCorrupt)?;
            let mut statement = conn.prepare(
                r#"SELECT injection_id,target_fqn,next_attempt_at,queued_at
                   FROM pty_input_operations
                   WHERE source_plane='container_api'
                     AND ((status IN ('queued','retry') AND next_attempt_at <= ?1)
                       OR (status='preparing' AND lease_until <= ?1))
                     AND attempt < 5
                     AND (?2 IS NULL OR next_attempt_at > ?2
                       OR (next_attempt_at = ?2 AND queued_at > ?3)
                       OR (next_attempt_at = ?2 AND queued_at = ?3 AND injection_id > ?4))
                   ORDER BY next_attempt_at,queued_at,injection_id LIMIT ?5"#,
            )?;
            let (cursor_next, cursor_queued, cursor_id) = cursor
                .as_ref()
                .map(|(next, queued, id)| {
                    (
                        Some(next.as_str()),
                        Some(queued.as_str()),
                        Some(id.as_str()),
                    )
                })
                .unwrap_or((None, None, None));
            let limit =
                i64::try_from(limit.min(64)).map_err(|_| MessageStoreError::StoreCorrupt)?;
            let mapped = statement.query_map(
                params![now, cursor_next, cursor_queued, cursor_id, limit],
                |row| {
                    Ok((
                        DuePtyInputCandidate {
                            injection_id: row.get(0)?,
                            target_fqn: row.get(1)?,
                        },
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                },
            )?;
            mapped.collect::<Result<Vec<_>, _>>()?
        };
        if rows.is_empty() && cursor.is_some() {
            self.maintenance_cursors
                .lock()
                .map_err(|_| MessageStoreError::StoreCorrupt)?
                .due_container = None;
            return self.due_container_pty_input_candidates_fair(query_time, limit);
        }
        self.maintenance_cursors
            .lock()
            .map_err(|_| MessageStoreError::StoreCorrupt)?
            .due_container = rows.last().map(|(candidate, next, queued)| {
            (next.clone(), queued.clone(), candidate.injection_id.clone())
        });
        Ok(rows
            .into_iter()
            .map(|(candidate, _, _)| candidate)
            .collect())
    }

    pub fn claim_pty_input(
        &self,
        source_plane: crate::phone::types::PtyInputSourcePlane,
        exact_injection_id: Option<&str>,
        lease_owner: &str,
        now: DateTime<Utc>,
    ) -> Result<Option<ClaimedPtyInputOperation>, MessageStoreError> {
        let now_s = crate::phone::types::canonical_pty_timestamp(now);
        let lease_until =
            crate::phone::types::canonical_pty_timestamp(now + chrono::Duration::seconds(120));
        let source = source_plane_str(source_plane);
        let mut conn = self
            .conn
            .lock()
            .map_err(|_| MessageStoreError::StoreCorrupt)?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let candidate = if let Some(injection_id) = exact_injection_id {
            tx.query_row(
                r#"SELECT injection_id FROM pty_input_operations
                   WHERE injection_id=?1 AND source_plane=?2 AND attempt < 5
                     AND expires_at > ?3
                     AND ((status IN ('queued','retry') AND next_attempt_at <= ?3)
                       OR (status='preparing' AND lease_until <= ?3))"#,
                params![injection_id, source, now_s],
                |row| row.get::<_, String>(0),
            )
            .optional()?
        } else {
            tx.query_row(
                r#"SELECT injection_id FROM pty_input_operations
                   WHERE source_plane=?1 AND attempt < 5
                     AND expires_at > ?2
                     AND ((status IN ('queued','retry') AND next_attempt_at <= ?2)
                       OR (status='preparing' AND lease_until <= ?2))
                   ORDER BY next_attempt_at, queued_at, injection_id LIMIT 1"#,
                params![source, now_s],
                |row| row.get::<_, String>(0),
            )
            .optional()?
        };
        let Some(injection_id) = candidate else {
            tx.commit()?;
            return Ok(None);
        };
        let changed = tx.execute(
            r#"UPDATE pty_input_operations
               SET status='preparing', attempt=attempt+1, lease_owner=?1,
                   lease_until=MIN(?2,expires_at), preparing_at=?3, updated_at=?3
               WHERE injection_id=?4 AND attempt < 5 AND expires_at > ?3
                 AND ((status IN ('queued','retry') AND next_attempt_at <= ?3)
                   OR (status='preparing' AND lease_until <= ?3))"#,
            params![lease_owner, lease_until, now_s, injection_id],
        )?;
        if changed != 1 {
            tx.commit()?;
            return Ok(None);
        }
        insert_pty_audit(&tx, &injection_id, PTY_STATUS_PREPARING, None, &now_s)?;
        let claimed = tx.query_row(
            r#"SELECT injection_id,sender_fqn,target_fqn,op_id,source_plane,
                      lease_owner,attempt,authority_session_id,authority_client_id,
                      authority_client_generation,sender_identity_fingerprint,
                      target_identity_fingerprint,requested_agent_id,expires_at
               FROM pty_input_operations WHERE injection_id=?1"#,
            [&injection_id],
            claimed_from_row,
        )?;
        tx.commit()?;
        Ok(Some(claimed))
    }

    pub fn renew_pty_input_lease(
        &self,
        injection_id: &str,
        lease_owner: &str,
        now: DateTime<Utc>,
    ) -> Result<bool, MessageStoreError> {
        let now_s = crate::phone::types::canonical_pty_timestamp(now);
        let lease_until =
            crate::phone::types::canonical_pty_timestamp(now + chrono::Duration::seconds(120));
        let conn = self
            .conn
            .lock()
            .map_err(|_| MessageStoreError::StoreCorrupt)?;
        let changed = conn.execute(
            r#"UPDATE pty_input_operations
               SET lease_until=MIN(?1,expires_at),updated_at=?2
               WHERE injection_id=?3 AND status='preparing'
                 AND lease_owner=?4 AND lease_until > ?2 AND expires_at > ?2"#,
            params![lease_until, now_s, injection_id, lease_owner],
        )?;
        Ok(changed == 1)
    }

    pub fn begin_pty_actuating(
        &self,
        injection_id: &str,
        lease_owner: &str,
        selected_session_id: &str,
        selected_backend: &str,
        now: DateTime<Utc>,
    ) -> Result<Vec<u8>, MessageStoreError> {
        let now_s = crate::phone::types::canonical_pty_timestamp(now);
        let mut conn = self
            .conn
            .lock()
            .map_err(|_| MessageStoreError::StoreCorrupt)?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let row = tx
            .query_row(
                r#"SELECT payload,payload_sha256,payload_bytes,version,enter_mode,source_plane,
                          nonce_sha256,request_fingerprint,sender_incarnation_fingerprint,
                          sender_identity_fingerprint,target_identity_fingerprint,
                          authority_session_id,authority_client_id,authority_client_generation,
                          issued_at,expires_at
                   FROM pty_input_operations
                   WHERE injection_id=?1 AND status='preparing' AND lease_owner=?2
                     AND lease_until > ?3 AND expires_at > ?3"#,
                params![injection_id, lease_owner, now_s],
                actuating_row_from_row,
            )
            .optional()?
            .ok_or(MessageStoreError::InvalidTransition)?;
        let actual_payload_bytes =
            i64::try_from(row.payload.len()).map_err(|_| MessageStoreError::StoreCorrupt)?;
        let issued = crate::phone::types::parse_canonical_pty_timestamp(&row.issued_at)
            .map_err(|_| MessageStoreError::StoreCorrupt)?;
        let expires = crate::phone::types::parse_canonical_pty_timestamp(&row.expires_at)
            .map_err(|_| MessageStoreError::StoreCorrupt)?;
        let authority_valid = match row.source.as_str() {
            "host_cli" => {
                row.authority_client_id.is_none() && row.authority_client_generation.is_none()
            }
            "container_api" => {
                row.authority_client_id
                    .as_deref()
                    .is_some_and(|id| !id.is_empty())
                    && row
                        .authority_client_generation
                        .as_deref()
                        .is_some_and(|generation| {
                            crate::phone::types::parse_canonical_uuid_v4(generation).is_ok()
                        })
            }
            _ => false,
        };
        if row.version != 1
            || row.enter_mode != "agent-submit"
            || !authority_valid
            || actual_payload_bytes != row.payload_bytes
            || row.digest != sha256_hex(&row.payload)
            || !is_lower_hex_64(&row.digest)
            || !is_lower_hex_64(&row.nonce_sha256)
            || !is_lower_hex_64(&row.request_fingerprint)
            || !is_lower_hex_64(&row.sender_incarnation_fingerprint)
            || !row
                .sender_identity_fingerprint
                .as_deref()
                .is_some_and(is_lower_hex_64)
            || !row
                .target_identity_fingerprint
                .as_deref()
                .is_some_and(is_lower_hex_64)
            || !row
                .authority_session_id
                .as_deref()
                .is_some_and(|session_id| {
                    crate::phone::types::parse_canonical_uuid_v4(session_id).is_ok()
                })
            || expires - issued != chrono::Duration::minutes(10)
            || crate::phone::types::parse_canonical_uuid_v4(selected_session_id).is_err()
            || !matches!(selected_backend, "localProcess" | "containerTransport")
        {
            return Err(MessageStoreError::StoreCorrupt);
        }
        let payload = row.payload;
        let text = std::str::from_utf8(&payload).map_err(|_| MessageStoreError::StoreCorrupt)?;
        crate::pty::inject::validate_pty_input_text(text)
            .map_err(|_| MessageStoreError::StoreCorrupt)?;
        let changed = tx.execute(
            r#"UPDATE pty_input_operations
               SET status='actuating',actuating_at=?1,updated_at=?1,
                   selected_session_id=?2,selected_backend=?3,payload=NULL,
                   requested_agent_id=NULL,sender_identity_fingerprint=NULL,
                   target_identity_fingerprint=NULL,authority_session_id=NULL,
                   authority_client_id=NULL,authority_client_generation=NULL,
                   lease_owner=NULL,lease_until=NULL,reason_code=NULL,reason_detail=NULL
               WHERE injection_id=?4 AND status='preparing' AND lease_owner=?5"#,
            params![
                now_s,
                selected_session_id,
                selected_backend,
                injection_id,
                lease_owner
            ],
        )?;
        if changed != 1 {
            return Err(MessageStoreError::InvalidTransition);
        }
        insert_pty_audit(&tx, injection_id, PTY_STATUS_ACTUATING, None, &now_s)?;
        #[cfg(test)]
        {
            let inject_ambiguous = {
                let mut faults = self
                    .test_faults
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                std::mem::take(&mut faults.actuation_commit_ambiguous)
            };
            if inject_ambiguous {
                tx.commit()?;
                return Err(MessageStoreError::ActuationCommitAmbiguous);
            }
        }
        match tx.commit() {
            Ok(()) => Ok(payload),
            Err(error) => {
                let status = conn
                    .query_row(
                        "SELECT status FROM pty_input_operations WHERE injection_id=?1",
                        [injection_id],
                        |row| row.get::<_, String>(0),
                    )
                    .optional();
                match status {
                    Ok(Some(status)) if status == PTY_STATUS_ACTUATING => {
                        Err(MessageStoreError::ActuationCommitAmbiguous)
                    }
                    Ok(Some(status)) if status == PTY_STATUS_PREPARING => {
                        Err(MessageStoreError::Sqlite(error))
                    }
                    Ok(Some(_)) => Err(MessageStoreError::StoreCorrupt),
                    Ok(None) => Err(MessageStoreError::OperationNotFound),
                    Err(_) => Err(MessageStoreError::StoreCorrupt),
                }
            }
        }
    }

    pub fn retry_pty_input(
        &self,
        injection_id: &str,
        lease_owner: &str,
        code: crate::phone::types::PtyInputReasonCode,
        now: DateTime<Utc>,
    ) -> Result<(), MessageStoreError> {
        let now_s = crate::phone::types::canonical_pty_timestamp(now);
        let mut conn = self
            .conn
            .lock()
            .map_err(|_| MessageStoreError::StoreCorrupt)?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let attempt: i64 = tx.query_row(
            "SELECT attempt FROM pty_input_operations WHERE injection_id=?1 AND status='preparing' AND lease_owner=?2",
            params![injection_id, lease_owner],
            |row| row.get(0),
        )?;
        if attempt >= 5 {
            return terminalize_in_transaction(
                tx,
                injection_id,
                PTY_STATUS_REJECTED,
                Some(code),
                &now_s,
            );
        }
        let delay = retry_backoff_seconds(attempt).min(40);
        let next =
            crate::phone::types::canonical_pty_timestamp(now + chrono::Duration::seconds(delay));
        let changed = tx.execute(
            r#"UPDATE pty_input_operations SET status='retry',next_attempt_at=?1,
                   lease_owner=NULL,lease_until=NULL,updated_at=?2,
                   reason_code=?3,reason_detail=?4
               WHERE injection_id=?5 AND status='preparing' AND lease_owner=?6"#,
            params![
                next,
                now_s,
                reason_code_string(code),
                crate::phone::types::safe_detail(code),
                injection_id,
                lease_owner
            ],
        )?;
        if changed != 1 {
            return Err(MessageStoreError::InvalidTransition);
        }
        insert_pty_audit(&tx, injection_id, PTY_STATUS_RETRY, Some(code), &now_s)?;
        tx.commit()?;
        Ok(())
    }

    pub fn terminalize_pty_input(
        &self,
        injection_id: &str,
        status: crate::phone::types::PtyInputPublicStatus,
        reason: Option<crate::phone::types::PtyInputReasonCode>,
        now: DateTime<Utc>,
    ) -> Result<crate::phone::types::PtyInputResult, MessageStoreError> {
        let status = match status {
            crate::phone::types::PtyInputPublicStatus::Injected => PTY_STATUS_INJECTED,
            crate::phone::types::PtyInputPublicStatus::Rejected => PTY_STATUS_REJECTED,
            crate::phone::types::PtyInputPublicStatus::Indeterminate => PTY_STATUS_INDETERMINATE,
            _ => return Err(MessageStoreError::InvalidTransition),
        };
        let now_s = crate::phone::types::canonical_pty_timestamp(now);
        let mut conn = self
            .conn
            .lock()
            .map_err(|_| MessageStoreError::StoreCorrupt)?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        terminalize_in_transaction(tx, injection_id, status, reason, &now_s)?;
        drop(conn);
        self.query_pty_input_by_injection(injection_id)?
            .ok_or(MessageStoreError::StoreCorrupt)
    }

    pub fn query_pty_input_by_injection(
        &self,
        injection_id: &str,
    ) -> Result<Option<crate::phone::types::PtyInputResult>, MessageStoreError> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| MessageStoreError::StoreCorrupt)?;
        load_pty_result(&conn, injection_id)
    }

    pub fn host_confirmation_tag(
        &self,
        injection_id: &str,
    ) -> Result<Option<String>, MessageStoreError> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| MessageStoreError::StoreCorrupt)?;
        Ok(conn
            .query_row(
                r#"SELECT confirmation_tag FROM pty_input_operations WHERE injection_id=?1
                   UNION ALL
                   SELECT confirmation_tag FROM pty_input_tombstones WHERE injection_id=?1
                   LIMIT 1"#,
                [injection_id],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()?
            .flatten())
    }

    pub fn mark_host_artifact(
        &self,
        injection_id: &str,
        now: DateTime<Utc>,
    ) -> Result<(), MessageStoreError> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| MessageStoreError::StoreCorrupt)?;
        let changed = conn.execute(
            "UPDATE pty_input_operations SET host_artifact_at=?1 WHERE injection_id=?2 AND source_plane='host_cli' AND status IN ('injected','rejected','indeterminate')",
            params![crate::phone::types::canonical_pty_timestamp(now), injection_id],
        )?;
        if changed == 1 {
            return Ok(());
        }
        let compacted: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM pty_input_tombstones WHERE injection_id=?1 AND source_plane='host_cli')",
            [injection_id],
            |row| row.get(0),
        )?;
        if compacted {
            Ok(())
        } else {
            Err(MessageStoreError::InvalidTransition)
        }
    }

    pub fn query_pty_input(
        &self,
        sender_fqn: &str,
        op_id: &str,
        sender_incarnation_fingerprint: &str,
    ) -> Result<Option<crate::phone::types::PtyInputResult>, MessageStoreError> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| MessageStoreError::StoreCorrupt)?;
        let id = conn
            .query_row(
                r#"SELECT injection_id,sender_incarnation_fingerprint
                   FROM pty_input_operations WHERE sender_fqn=?1 AND op_id=?2
                   UNION ALL
                   SELECT injection_id,sender_incarnation_fingerprint
                   FROM pty_input_tombstones WHERE sender_fqn=?1 AND op_id=?2
                   LIMIT 1"#,
                params![sender_fqn, op_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?;
        let Some((id, incarnation)) = id else {
            return Ok(None);
        };
        if incarnation != sender_incarnation_fingerprint {
            return Ok(None);
        }
        load_pty_result(&conn, &id)
    }

    pub fn compact_pty_terminal_before(
        &self,
        cutoff: DateTime<Utc>,
        limit: usize,
    ) -> Result<usize, MessageStoreError> {
        let cutoff_time = cutoff;
        let cutoff = crate::phone::types::canonical_pty_timestamp(cutoff);
        let limit = i64::try_from(limit.min(64)).map_err(|_| MessageStoreError::StoreCorrupt)?;
        let cursor = self
            .maintenance_cursors
            .lock()
            .map_err(|_| MessageStoreError::StoreCorrupt)?
            .compact_before
            .clone();
        let candidates = {
            let conn = self
                .conn
                .lock()
                .map_err(|_| MessageStoreError::StoreCorrupt)?;
            let mut statement = conn.prepare(
                r#"SELECT injection_id,terminal_at FROM pty_input_operations
                   WHERE status IN ('injected','rejected','indeterminate')
                     AND terminal_at < ?1
                     AND (?2 IS NULL OR terminal_at > ?2
                       OR (terminal_at = ?2 AND injection_id > ?3))
                   ORDER BY terminal_at,injection_id LIMIT ?4"#,
            )?;
            let (cursor_at, cursor_id) = cursor
                .as_ref()
                .map(|(at, id)| (Some(at.as_str()), Some(id.as_str())))
                .unwrap_or((None, None));
            let rows = statement
                .query_map(params![cutoff, cursor_at, cursor_id, limit], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })?;
            rows.collect::<Result<Vec<_>, _>>()?
        };
        if candidates.is_empty() && cursor.is_some() {
            self.maintenance_cursors
                .lock()
                .map_err(|_| MessageStoreError::StoreCorrupt)?
                .compact_before = None;
            return self.compact_pty_terminal_before(cutoff_time, limit as usize);
        }
        self.maintenance_cursors
            .lock()
            .map_err(|_| MessageStoreError::StoreCorrupt)?
            .compact_before = candidates
            .last()
            .map(|(id, terminal_at)| (terminal_at.clone(), id.clone()));
        let mut deleted = 0;
        for (injection_id, _) in candidates {
            let Some(_operation_lock) = self.try_operation_lock(&injection_id)? else {
                continue;
            };
            let mut conn = self
                .conn
                .lock()
                .map_err(|_| MessageStoreError::StoreCorrupt)?;
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let eligible: bool = tx.query_row(
                r#"SELECT EXISTS(SELECT 1 FROM pty_input_operations
                   WHERE injection_id=?1 AND status IN ('injected','rejected','indeterminate')
                     AND terminal_at < ?2)"#,
                params![injection_id, cutoff],
                |row| row.get(0),
            )?;
            if !eligible {
                tx.commit()?;
                continue;
            }
            if !tombstone_matches(&tx, &injection_id)? {
                return Err(MessageStoreError::StoreCorrupt);
            }
            let changed = tx.execute(
                r#"DELETE FROM pty_input_operations
                   WHERE injection_id=?1 AND status IN ('injected','rejected','indeterminate')
                     AND terminal_at < ?2"#,
                params![injection_id, cutoff],
            )?;
            if changed != 1 {
                return Err(MessageStoreError::InvalidTransition);
            }
            tx.commit()?;
            deleted += 1;
        }
        Ok(deleted)
    }

    pub fn compact_pty_terminal_maintenance(
        &self,
        now: DateTime<Utc>,
        limit: usize,
    ) -> Result<usize, MessageStoreError> {
        let maintenance_time = now;
        let normal_cutoff =
            crate::phone::types::canonical_pty_timestamp(now - chrono::Duration::days(7));
        let unclaimed_cutoff =
            crate::phone::types::canonical_pty_timestamp(now - chrono::Duration::days(30));
        let limit = i64::try_from(limit.min(64)).map_err(|_| MessageStoreError::StoreCorrupt)?;
        let cursor = self
            .maintenance_cursors
            .lock()
            .map_err(|_| MessageStoreError::StoreCorrupt)?
            .compact_maintenance
            .clone();
        let candidates = {
            let conn = self
                .conn
                .lock()
                .map_err(|_| MessageStoreError::StoreCorrupt)?;
            let mut statement = conn.prepare(
                r#"SELECT injection_id,terminal_at FROM pty_input_operations
                   WHERE status IN ('injected','rejected','indeterminate')
                     AND ((source_plane!='host_cli' AND terminal_at < ?1)
                       OR (source_plane='host_cli' AND host_artifact_at IS NOT NULL AND terminal_at < ?1)
                       OR (source_plane='host_cli' AND host_artifact_at IS NULL AND terminal_at < ?2))
                     AND (?3 IS NULL OR terminal_at > ?3
                       OR (terminal_at = ?3 AND injection_id > ?4))
                   ORDER BY terminal_at,injection_id LIMIT ?5"#,
            )?;
            let (cursor_at, cursor_id) = cursor
                .as_ref()
                .map(|(at, id)| (Some(at.as_str()), Some(id.as_str())))
                .unwrap_or((None, None));
            let rows = statement.query_map(
                params![normal_cutoff, unclaimed_cutoff, cursor_at, cursor_id, limit],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )?;
            rows.collect::<Result<Vec<_>, _>>()?
        };
        if candidates.is_empty() && cursor.is_some() {
            self.maintenance_cursors
                .lock()
                .map_err(|_| MessageStoreError::StoreCorrupt)?
                .compact_maintenance = None;
            return self.compact_pty_terminal_maintenance(maintenance_time, limit as usize);
        }
        self.maintenance_cursors
            .lock()
            .map_err(|_| MessageStoreError::StoreCorrupt)?
            .compact_maintenance = candidates
            .last()
            .map(|(id, terminal_at)| (terminal_at.clone(), id.clone()));
        let mut compacted = 0;
        for (injection_id, _) in candidates {
            let Some(_operation_lock) = self.try_operation_lock(&injection_id)? else {
                continue;
            };
            let mut conn = self
                .conn
                .lock()
                .map_err(|_| MessageStoreError::StoreCorrupt)?;
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            if !tombstone_matches(&tx, &injection_id)? {
                return Err(MessageStoreError::StoreCorrupt);
            }
            let deleted = tx.execute(
                r#"DELETE FROM pty_input_operations
                   WHERE injection_id=?1 AND status IN ('injected','rejected','indeterminate')
                     AND ((source_plane!='host_cli' AND terminal_at < ?2)
                       OR (source_plane='host_cli' AND host_artifact_at IS NOT NULL AND terminal_at < ?2)
                       OR (source_plane='host_cli' AND host_artifact_at IS NULL AND terminal_at < ?3))"#,
                params![injection_id, normal_cutoff, unclaimed_cutoff],
            )?;
            if deleted != 1 {
                return Err(MessageStoreError::InvalidTransition);
            }
            tx.commit()?;
            compacted += 1;
        }
        Ok(compacted)
    }

    pub async fn enqueue_pty_input_offloaded(
        &self,
        req: PtyInputEnqueueRequest,
    ) -> Result<PtyInputEnqueueResult, MessageStoreError> {
        let store = self.clone();
        tokio::task::spawn_blocking(move || store.enqueue_pty_input(req))
            .await
            .map_err(|error| MessageStoreError::BlockingTask(error.to_string()))?
    }

    pub async fn record_host_pty_input_rejection_offloaded(
        &self,
        request: HostPtyInputRejectionRequest,
        now: DateTime<Utc>,
    ) -> Result<crate::phone::types::PtyInputResult, MessageStoreError> {
        let store = self.clone();
        tokio::task::spawn_blocking(move || store.record_host_pty_input_rejection(request, now))
            .await
            .map_err(|error| MessageStoreError::BlockingTask(error.to_string()))?
    }

    pub async fn query_pty_input_offloaded(
        &self,
        sender_fqn: String,
        op_id: String,
        sender_incarnation_fingerprint: String,
    ) -> Result<Option<crate::phone::types::PtyInputResult>, MessageStoreError> {
        let store = self.clone();
        tokio::task::spawn_blocking(move || {
            store.query_pty_input(&sender_fqn, &op_id, &sender_incarnation_fingerprint)
        })
        .await
        .map_err(|error| MessageStoreError::BlockingTask(error.to_string()))?
    }

    pub async fn recover_pty_input_runtime_offloaded(
        &self,
        active: HashSet<String>,
        now: DateTime<Utc>,
    ) -> Result<usize, MessageStoreError> {
        let store = self.clone();
        tokio::task::spawn_blocking(move || store.recover_pty_input_runtime(&active, now))
            .await
            .map_err(|error| MessageStoreError::BlockingTask(error.to_string()))?
    }

    pub(crate) async fn due_container_pty_input_candidates_fair_offloaded(
        &self,
        now: DateTime<Utc>,
        limit: usize,
    ) -> Result<Vec<DuePtyInputCandidate>, MessageStoreError> {
        let store = self.clone();
        tokio::task::spawn_blocking(move || {
            store.due_container_pty_input_candidates_fair(now, limit)
        })
        .await
        .map_err(|error| MessageStoreError::BlockingTask(error.to_string()))?
    }

    pub async fn claim_pty_input_offloaded(
        &self,
        source_plane: crate::phone::types::PtyInputSourcePlane,
        exact_injection_id: Option<String>,
        lease_owner: String,
        now: DateTime<Utc>,
    ) -> Result<Option<ClaimedPtyInputOperation>, MessageStoreError> {
        let store = self.clone();
        tokio::task::spawn_blocking(move || {
            store.claim_pty_input(
                source_plane,
                exact_injection_id.as_deref(),
                &lease_owner,
                now,
            )
        })
        .await
        .map_err(|error| MessageStoreError::BlockingTask(error.to_string()))?
    }

    pub async fn renew_pty_input_lease_offloaded(
        &self,
        injection_id: String,
        lease_owner: String,
        now: DateTime<Utc>,
    ) -> Result<bool, MessageStoreError> {
        let store = self.clone();
        tokio::task::spawn_blocking(move || {
            store.renew_pty_input_lease(&injection_id, &lease_owner, now)
        })
        .await
        .map_err(|error| MessageStoreError::BlockingTask(error.to_string()))?
    }

    pub async fn begin_pty_actuating_offloaded(
        &self,
        injection_id: String,
        lease_owner: String,
        selected_session_id: String,
        selected_backend: String,
        now: DateTime<Utc>,
    ) -> Result<Vec<u8>, MessageStoreError> {
        let store = self.clone();
        tokio::task::spawn_blocking(move || {
            store.begin_pty_actuating(
                &injection_id,
                &lease_owner,
                &selected_session_id,
                &selected_backend,
                now,
            )
        })
        .await
        .map_err(|error| MessageStoreError::BlockingTask(error.to_string()))?
    }

    pub async fn retry_pty_input_offloaded(
        &self,
        injection_id: String,
        lease_owner: String,
        code: crate::phone::types::PtyInputReasonCode,
        now: DateTime<Utc>,
    ) -> Result<(), MessageStoreError> {
        let store = self.clone();
        tokio::task::spawn_blocking(move || {
            store.retry_pty_input(&injection_id, &lease_owner, code, now)
        })
        .await
        .map_err(|error| MessageStoreError::BlockingTask(error.to_string()))?
    }

    pub async fn terminalize_pty_input_offloaded(
        &self,
        injection_id: String,
        status: crate::phone::types::PtyInputPublicStatus,
        reason: Option<crate::phone::types::PtyInputReasonCode>,
        now: DateTime<Utc>,
    ) -> Result<crate::phone::types::PtyInputResult, MessageStoreError> {
        let store = self.clone();
        tokio::task::spawn_blocking(move || {
            store.terminalize_pty_input(&injection_id, status, reason, now)
        })
        .await
        .map_err(|error| MessageStoreError::BlockingTask(error.to_string()))?
    }

    pub async fn query_pty_input_by_injection_offloaded(
        &self,
        injection_id: String,
    ) -> Result<Option<crate::phone::types::PtyInputResult>, MessageStoreError> {
        let store = self.clone();
        tokio::task::spawn_blocking(move || store.query_pty_input_by_injection(&injection_id))
            .await
            .map_err(|error| MessageStoreError::BlockingTask(error.to_string()))?
    }

    pub async fn host_confirmation_tag_offloaded(
        &self,
        injection_id: String,
    ) -> Result<Option<String>, MessageStoreError> {
        let store = self.clone();
        tokio::task::spawn_blocking(move || store.host_confirmation_tag(&injection_id))
            .await
            .map_err(|error| MessageStoreError::BlockingTask(error.to_string()))?
    }

    pub async fn mark_host_artifact_offloaded(
        &self,
        injection_id: String,
        now: DateTime<Utc>,
    ) -> Result<(), MessageStoreError> {
        let store = self.clone();
        tokio::task::spawn_blocking(move || store.mark_host_artifact(&injection_id, now))
            .await
            .map_err(|error| MessageStoreError::BlockingTask(error.to_string()))?
    }

    pub async fn compact_pty_terminal_maintenance_offloaded(
        &self,
        now: DateTime<Utc>,
        limit: usize,
    ) -> Result<usize, MessageStoreError> {
        let store = self.clone();
        tokio::task::spawn_blocking(move || store.compact_pty_terminal_maintenance(now, limit))
            .await
            .map_err(|error| MessageStoreError::BlockingTask(error.to_string()))?
    }

    pub async fn enqueue_offloaded(
        &self,
        req: EnqueueRequest,
    ) -> Result<EnqueueResult, MessageStoreError> {
        let store = self.clone();
        tokio::task::spawn_blocking(move || store.enqueue(req))
            .await
            .map_err(|e| MessageStoreError::BlockingTask(e.to_string()))?
    }

    pub async fn lease_due_offloaded(
        &self,
        now: DateTime<Utc>,
        limit: usize,
        lease_for: Duration,
        lease_owner: String,
    ) -> Result<Vec<LeasedMessage>, MessageStoreError> {
        let store = self.clone();
        tokio::task::spawn_blocking(move || store.lease_due(now, limit, lease_for, &lease_owner))
            .await
            .map_err(|e| MessageStoreError::BlockingTask(e.to_string()))?
    }

    pub async fn mark_delivered_offloaded(
        &self,
        message_id: String,
        now: DateTime<Utc>,
    ) -> Result<(), MessageStoreError> {
        let store = self.clone();
        tokio::task::spawn_blocking(move || store.mark_delivered(&message_id, now))
            .await
            .map_err(|e| MessageStoreError::BlockingTask(e.to_string()))?
    }

    pub async fn mark_delivery_failed_offloaded(
        &self,
        message_id: String,
        error: String,
        now: DateTime<Utc>,
        max_attempts: i64,
    ) -> Result<String, MessageStoreError> {
        let store = self.clone();
        tokio::task::spawn_blocking(move || {
            store.mark_delivery_failed(&message_id, &error, now, max_attempts)
        })
        .await
        .map_err(|e| MessageStoreError::BlockingTask(e.to_string()))?
    }

    pub async fn release_delivery_lease_offloaded(
        &self,
        message_id: String,
        reason: String,
        now: DateTime<Utc>,
    ) -> Result<(), MessageStoreError> {
        let store = self.clone();
        tokio::task::spawn_blocking(move || store.release_delivery_lease(&message_id, &reason, now))
            .await
            .map_err(|e| MessageStoreError::BlockingTask(e.to_string()))?
    }

    pub async fn reap_terminal_before_offloaded(
        &self,
        cutoff: DateTime<Utc>,
    ) -> Result<usize, MessageStoreError> {
        let store = self.clone();
        tokio::task::spawn_blocking(move || store.reap_terminal_before(cutoff))
            .await
            .map_err(|e| MessageStoreError::BlockingTask(e.to_string()))?
    }

    #[cfg(test)]
    fn inject_actuation_commit_ambiguous(&self) {
        self.test_faults
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .actuation_commit_ambiguous = true;
    }

    #[cfg(test)]
    fn count_by_status(&self, status: &str) -> i64 {
        self.conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM messages WHERE status = ?1",
                [status],
                |row| row.get(0),
            )
            .unwrap()
    }
}

fn source_plane_str(source: crate::phone::types::PtyInputSourcePlane) -> &'static str {
    match source {
        crate::phone::types::PtyInputSourcePlane::HostCli => "host_cli",
        crate::phone::types::PtyInputSourcePlane::ContainerApi => "container_api",
    }
}

fn parse_source_plane(
    source: &str,
) -> Result<crate::phone::types::PtyInputSourcePlane, rusqlite::Error> {
    match source {
        "host_cli" => Ok(crate::phone::types::PtyInputSourcePlane::HostCli),
        "container_api" => Ok(crate::phone::types::PtyInputSourcePlane::ContainerApi),
        _ => Err(rusqlite::Error::InvalidQuery),
    }
}

fn reason_code_string(code: crate::phone::types::PtyInputReasonCode) -> String {
    crate::phone::types::pty_input_reason_code_name(code).to_string()
}

fn parse_reason_code(
    code: Option<String>,
) -> Result<Option<crate::phone::types::PtyInputReasonCode>, rusqlite::Error> {
    code.map(|value| {
        serde_json::from_str(&format!("\"{value}\"")).map_err(|_| rusqlite::Error::InvalidQuery)
    })
    .transpose()
}

struct ActuatingRow {
    payload: Vec<u8>,
    digest: String,
    payload_bytes: i64,
    version: i64,
    enter_mode: String,
    source: String,
    nonce_sha256: String,
    request_fingerprint: String,
    sender_incarnation_fingerprint: String,
    sender_identity_fingerprint: Option<String>,
    target_identity_fingerprint: Option<String>,
    authority_session_id: Option<String>,
    authority_client_id: Option<String>,
    authority_client_generation: Option<String>,
    issued_at: String,
    expires_at: String,
}

fn actuating_row_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ActuatingRow> {
    Ok(ActuatingRow {
        payload: row.get(0)?,
        digest: row.get(1)?,
        payload_bytes: row.get(2)?,
        version: row.get(3)?,
        enter_mode: row.get(4)?,
        source: row.get(5)?,
        nonce_sha256: row.get(6)?,
        request_fingerprint: row.get(7)?,
        sender_incarnation_fingerprint: row.get(8)?,
        sender_identity_fingerprint: row.get(9)?,
        target_identity_fingerprint: row.get(10)?,
        authority_session_id: row.get(11)?,
        authority_client_id: row.get(12)?,
        authority_client_generation: row.get(13)?,
        issued_at: row.get(14)?,
        expires_at: row.get(15)?,
    })
}

struct IdempotencyRow {
    injection_id: String,
    request_fingerprint: String,
    sender_incarnation_fingerprint: String,
}

fn lookup_idempotency_row(
    conn: &rusqlite::Connection,
    sender: &str,
    op_id: &str,
) -> Result<Option<IdempotencyRow>, rusqlite::Error> {
    conn.query_row(
        r#"SELECT injection_id,request_fingerprint,sender_incarnation_fingerprint
           FROM pty_input_operations WHERE sender_fqn=?1 AND op_id=?2
           UNION ALL
           SELECT injection_id,request_fingerprint,sender_incarnation_fingerprint
           FROM pty_input_tombstones WHERE sender_fqn=?1 AND op_id=?2
           LIMIT 1"#,
        params![sender, op_id],
        |row| {
            Ok(IdempotencyRow {
                injection_id: row.get(0)?,
                request_fingerprint: row.get(1)?,
                sender_incarnation_fingerprint: row.get(2)?,
            })
        },
    )
    .optional()
}

fn result_from_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<crate::phone::types::PtyInputResult> {
    let version: i64 = row.get(0)?;
    let injection_id: String = row.get(1)?;
    let op_id: String = row.get(2)?;
    let sender: String = row.get(3)?;
    let status_raw: String = row.get(4)?;
    let payload_bytes: i64 = row.get(5)?;
    let payload_sha256: String = row.get(6)?;
    let source = parse_source_plane(&row.get::<_, String>(7)?)?;
    let selected_session_id: Option<String> = row.get(8)?;
    let selected_backend: Option<String> = row.get(9)?;
    let issued_at: String = row.get(10)?;
    let expires_at: String = row.get(11)?;
    let queued_at: String = row.get(12)?;
    let actuating_at: Option<String> = row.get(13)?;
    let terminal_at: Option<String> = row.get(14)?;
    let reason_detail: Option<String> = row.get(15)?;
    let reason_code = parse_reason_code(row.get(17)?)?;
    let target: String = row.get(18)?;
    let status = match status_raw.as_str() {
        "queued" | "preparing" | "retry" => crate::phone::types::PtyInputPublicStatus::Queued,
        "actuating" => crate::phone::types::PtyInputPublicStatus::Actuating,
        "injected" => crate::phone::types::PtyInputPublicStatus::Injected,
        "rejected" => crate::phone::types::PtyInputPublicStatus::Rejected,
        "indeterminate" => crate::phone::types::PtyInputPublicStatus::Indeterminate,
        _ => return Err(rusqlite::Error::InvalidQuery),
    };
    let timestamp_valid =
        |value: &str| crate::phone::types::parse_canonical_pty_timestamp(value).is_ok();
    if version != 1
        || crate::phone::types::parse_canonical_uuid_v4(&injection_id).is_err()
        || crate::phone::types::parse_canonical_uuid_v4(&op_id).is_err()
        || sender.is_empty()
        || target.is_empty()
        || !(1..=65_536).contains(&payload_bytes)
        || !is_lower_hex_64(&payload_sha256)
        || !timestamp_valid(&issued_at)
        || !timestamp_valid(&expires_at)
        || !timestamp_valid(&queued_at)
        || actuating_at
            .as_deref()
            .is_some_and(|value| !timestamp_valid(value))
        || terminal_at
            .as_deref()
            .is_some_and(|value| !timestamp_valid(value))
        || terminal_at.is_some() != status.is_terminal()
        || selected_session_id.is_some() != selected_backend.is_some()
        || selected_session_id.as_deref().is_some_and(|session_id| {
            crate::phone::types::parse_canonical_uuid_v4(session_id).is_err()
        })
        || selected_backend
            .as_deref()
            .is_some_and(|backend| !matches!(backend, "localProcess" | "containerTransport"))
        || reason_code.is_some() != reason_detail.is_some()
        || reason_code.is_some_and(|code| {
            reason_detail.as_deref() != Some(crate::phone::types::safe_detail(code))
        })
    {
        return Err(rusqlite::Error::InvalidQuery);
    }
    let payload_bytes = u64::try_from(payload_bytes).map_err(|_| rusqlite::Error::InvalidQuery)?;
    let result = crate::phone::types::PtyInputResult {
        version: 1,
        injection_id,
        op_id: Some(op_id),
        sender: Some(sender),
        target: Some(target),
        status,
        terminal: status.is_terminal(),
        payload_bytes: Some(payload_bytes),
        payload_sha256: Some(payload_sha256),
        source_plane: Some(source),
        selected_session_id,
        selected_backend,
        issued_at: Some(issued_at),
        expires_at: Some(expires_at),
        queued_at: Some(queued_at),
        actuating_at,
        terminal_at,
        reason: reason_code.map(crate::phone::types::PtyInputReason::from_code),
    };
    crate::phone::types::validate_enqueued_pty_input_result(&result)
        .map_err(|_| rusqlite::Error::InvalidQuery)?;
    Ok(result)
}

fn load_pty_result(
    conn: &rusqlite::Connection,
    injection_id: &str,
) -> Result<Option<crate::phone::types::PtyInputResult>, MessageStoreError> {
    const LIVE_QUERY: &str = r#"
        SELECT version,injection_id,op_id,sender_fqn,status,payload_bytes,
               payload_sha256,source_plane,selected_session_id,selected_backend,
               issued_at,expires_at,queued_at,actuating_at,terminal_at,
               reason_detail,updated_at,reason_code,target_fqn
        FROM pty_input_operations WHERE injection_id=?1
    "#;
    if let Some(result) = conn
        .query_row(LIVE_QUERY, [injection_id], result_from_row)
        .optional()?
    {
        return Ok(Some(result));
    }
    const TOMBSTONE_QUERY: &str = r#"
        SELECT version,injection_id,op_id,sender_fqn,status,payload_bytes,
               payload_sha256,source_plane,selected_session_id,selected_backend,
               issued_at,expires_at,queued_at,actuating_at,terminal_at,
               reason_detail,terminal_at,reason_code,target_fqn
        FROM pty_input_tombstones WHERE injection_id=?1
    "#;
    Ok(conn
        .query_row(TOMBSTONE_QUERY, [injection_id], result_from_row)
        .optional()?)
}

fn claimed_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ClaimedPtyInputOperation> {
    let source: String = row.get(4)?;
    Ok(ClaimedPtyInputOperation {
        injection_id: row.get(0)?,
        sender_fqn: row.get(1)?,
        target_fqn: row.get(2)?,
        op_id: row.get(3)?,
        source_plane: parse_source_plane(&source)?,
        lease_owner: row.get(5)?,
        attempt: row.get(6)?,
        authority_session_id: row
            .get::<_, Option<String>>(7)?
            .ok_or(rusqlite::Error::InvalidQuery)?,
        authority_client_id: row.get(8)?,
        authority_client_generation: row.get(9)?,
        sender_identity_fingerprint: row
            .get::<_, Option<String>>(10)?
            .ok_or(rusqlite::Error::InvalidQuery)?,
        target_identity_fingerprint: row
            .get::<_, Option<String>>(11)?
            .ok_or(rusqlite::Error::InvalidQuery)?,
        requested_agent_id: row.get(12)?,
        expires_at: row.get(13)?,
    })
}

fn insert_pty_audit(
    tx: &rusqlite::Transaction<'_>,
    injection_id: &str,
    status: &str,
    reason: Option<crate::phone::types::PtyInputReasonCode>,
    at: &str,
) -> Result<(), rusqlite::Error> {
    let inserted = tx.execute(
        r#"INSERT INTO pty_input_audit(
             event_id,injection_id,op_id,sender_fqn,target_fqn,version,
             payload_bytes,payload_sha256,source_plane,selected_session_id,
             selected_backend,status,reason_code,at
           ) SELECT ?1,injection_id,op_id,sender_fqn,target_fqn,version,
                    payload_bytes,payload_sha256,source_plane,selected_session_id,
                    selected_backend,?2,?3,?4
             FROM pty_input_operations WHERE injection_id=?5"#,
        params![
            uuid::Uuid::new_v4().to_string(),
            status,
            reason.map(reason_code_string),
            at,
            injection_id
        ],
    )?;
    if inserted != 1 {
        return Err(rusqlite::Error::InvalidQuery);
    }
    Ok(())
}

fn tombstone_matches(
    tx: &rusqlite::Transaction<'_>,
    injection_id: &str,
) -> Result<bool, rusqlite::Error> {
    tx.query_row(
        r#"SELECT EXISTS(
             SELECT 1 FROM pty_input_operations operation
             JOIN pty_input_tombstones tombstone
               ON tombstone.injection_id = operation.injection_id
             WHERE operation.injection_id=?1
               AND tombstone.sender_fqn=operation.sender_fqn
               AND tombstone.target_fqn=operation.target_fqn
               AND tombstone.op_id=operation.op_id
               AND tombstone.nonce_sha256=operation.nonce_sha256
               AND tombstone.request_fingerprint=operation.request_fingerprint
               AND tombstone.confirmation_tag IS operation.confirmation_tag
               AND tombstone.sender_incarnation_fingerprint=operation.sender_incarnation_fingerprint
               AND tombstone.version=operation.version
               AND tombstone.payload_sha256=operation.payload_sha256
               AND tombstone.payload_bytes=operation.payload_bytes
               AND tombstone.source_plane=operation.source_plane
               AND tombstone.selected_session_id IS operation.selected_session_id
               AND tombstone.selected_backend IS operation.selected_backend
               AND tombstone.issued_at=operation.issued_at
               AND tombstone.expires_at=operation.expires_at
               AND tombstone.queued_at=operation.queued_at
               AND tombstone.actuating_at IS operation.actuating_at
               AND tombstone.terminal_at=operation.terminal_at
               AND tombstone.status=operation.status
               AND tombstone.reason_code IS operation.reason_code
               AND tombstone.reason_detail IS operation.reason_detail
           )"#,
        [injection_id],
        |row| row.get(0),
    )
}

fn terminalize_in_transaction(
    tx: rusqlite::Transaction<'_>,
    injection_id: &str,
    status: &str,
    reason: Option<crate::phone::types::PtyInputReasonCode>,
    now: &str,
) -> Result<(), MessageStoreError> {
    let public_status = match status {
        PTY_STATUS_INJECTED => crate::phone::types::PtyInputPublicStatus::Injected,
        PTY_STATUS_REJECTED => crate::phone::types::PtyInputPublicStatus::Rejected,
        PTY_STATUS_INDETERMINATE => crate::phone::types::PtyInputPublicStatus::Indeterminate,
        _ => return Err(MessageStoreError::InvalidTransition),
    };
    if !crate::phone::types::pty_input_reason_allowed_for_status(public_status, reason) {
        return Err(MessageStoreError::InvalidTransition);
    }
    let current: (String, Option<String>, Option<String>) = tx
        .query_row(
            "SELECT status,reason_code,reason_detail FROM pty_input_operations WHERE injection_id=?1",
            [injection_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?
        .ok_or(MessageStoreError::OperationNotFound)?;
    let desired_reason_code = reason.map(crate::phone::types::pty_input_reason_code_name);
    let desired_reason_detail = reason.map(crate::phone::types::safe_detail);
    if matches!(
        current.0.as_str(),
        "injected" | "rejected" | "indeterminate"
    ) {
        if current.0 == status
            && current.1.as_deref() == desired_reason_code
            && current.2.as_deref() == desired_reason_detail
        {
            if !tombstone_matches(&tx, injection_id)? {
                return Err(MessageStoreError::StoreCorrupt);
            }
            tx.commit()?;
            return Ok(());
        }
        return Err(MessageStoreError::InvalidTransition);
    }
    let allowed = if status == PTY_STATUS_REJECTED {
        matches!(current.0.as_str(), "queued" | "preparing" | "retry")
    } else {
        current.0 == PTY_STATUS_ACTUATING
    };
    if !allowed {
        return Err(MessageStoreError::InvalidTransition);
    }
    let reason_code = desired_reason_code;
    let reason_detail = desired_reason_detail;
    let changed = tx.execute(
        r#"UPDATE pty_input_operations SET status=?1,terminal_at=?2,updated_at=?2,
               payload=NULL,requested_agent_id=NULL,sender_identity_fingerprint=NULL,
               target_identity_fingerprint=NULL,authority_session_id=NULL,
               authority_client_id=NULL,authority_client_generation=NULL,
               lease_owner=NULL,lease_until=NULL,reason_code=?3,reason_detail=?4
           WHERE injection_id=?5 AND status=?6"#,
        params![
            status,
            now,
            reason_code,
            reason_detail,
            injection_id,
            current.0
        ],
    )?;
    if changed != 1 {
        return Err(MessageStoreError::InvalidTransition);
    }
    insert_pty_audit(&tx, injection_id, status, reason, now)?;
    tx.execute(
        r#"INSERT INTO pty_input_tombstones(
             injection_id,sender_fqn,target_fqn,op_id,nonce_sha256,
             request_fingerprint,confirmation_tag,sender_incarnation_fingerprint,
             version,payload_sha256,payload_bytes,source_plane,selected_session_id,
             selected_backend,issued_at,expires_at,queued_at,actuating_at,
             terminal_at,status,reason_code,reason_detail
           ) SELECT injection_id,sender_fqn,target_fqn,op_id,nonce_sha256,
                    request_fingerprint,confirmation_tag,sender_incarnation_fingerprint,
                    version,payload_sha256,payload_bytes,source_plane,selected_session_id,
                    selected_backend,issued_at,expires_at,queued_at,actuating_at,
                    terminal_at,status,reason_code,reason_detail
             FROM pty_input_operations WHERE injection_id=?1
           ON CONFLICT(injection_id) DO NOTHING"#,
        [injection_id],
    )?;
    if !tombstone_matches(&tx, injection_id)? {
        return Err(MessageStoreError::StoreCorrupt);
    }
    tx.commit()?;
    Ok(())
}

pub fn retry_backoff_seconds(attempt: i64) -> i64 {
    let exponent = attempt.saturating_sub(1).min(5) as u32;
    5_i64.saturating_mul(2_i64.saturating_pow(exponent))
}

fn insert_audit(
    tx: &rusqlite::Transaction<'_>,
    message_id: &str,
    status: &str,
    detail: Option<&str>,
    at: &str,
) -> Result<(), rusqlite::Error> {
    tx.execute(
        r#"
        INSERT INTO message_audit(event_id, message_id, status, detail, at)
        VALUES(?1, ?2, ?3, ?4, ?5)
        "#,
        params![
            uuid::Uuid::new_v4().to_string(),
            message_id,
            status,
            detail,
            at
        ],
    )?;
    Ok(())
}

fn is_lower_hex_64(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(64);
    for byte in digest {
        out.push(char::from_digit((byte >> 4) as u32, 16).unwrap());
        out.push(char::from_digit((byte & 0x0f) as u32, 16).unwrap());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn store() -> MessageStore {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join(DB_FILENAME);
        let store = MessageStore::open(path).unwrap();
        std::mem::forget(dir);
        store
    }

    fn request(op: &str, body: &str) -> EnqueueRequest {
        EnqueueRequest {
            sender_fqn: "proj:wg-1/a".into(),
            target_fqn: "proj:wg-1/b".into(),
            op_id: op.into(),
            content_type: DEFAULT_CONTENT_TYPE.into(),
            body: body.into(),
            source_plane: "api_inline".into(),
            source_ref: None,
        }
    }

    fn pty_request(op_id: &str, text: &str) -> PtyInputEnqueueRequest {
        let now = Utc::now();
        PtyInputEnqueueRequest {
            injection_id: op_id.to_string(),
            sender_fqn: "proj:wg-1-team/lead".into(),
            target_fqn: "proj:wg-1-team/dev".into(),
            op_id: op_id.to_string(),
            nonce_sha256: sha256_hex(Uuid::new_v4().to_string().as_bytes()),
            request_fingerprint: sha256_hex(format!("request:{text}").as_bytes()),
            confirmation_tag: None,
            requested_agent_id: None,
            payload: text.as_bytes().to_vec(),
            source_plane: crate::phone::types::PtyInputSourcePlane::ContainerApi,
            sender_incarnation_fingerprint: "a".repeat(64),
            sender_identity_fingerprint: "b".repeat(64),
            target_identity_fingerprint: "c".repeat(64),
            authority_session_id: Uuid::new_v4().to_string(),
            authority_client_id: Some("client".into()),
            authority_client_generation: Some(Uuid::new_v4().to_string()),
            issued_at: crate::phone::types::canonical_pty_timestamp(now),
            expires_at: crate::phone::types::canonical_pty_timestamp(
                now + chrono::Duration::minutes(10),
            ),
        }
    }

    #[test]
    fn sqlite_identity_check_rejects_a_dangling_sidecar_link_when_supported() {
        let temp = tempfile::TempDir::new().unwrap();
        let database = temp.path().join(DB_FILENAME);
        let sidecar = sqlite_sidecar_paths(&database)[0].clone();
        let missing = temp.path().join("missing-wal");
        #[cfg(unix)]
        let linked = std::os::unix::fs::symlink(&missing, &sidecar).is_ok();
        #[cfg(windows)]
        let linked = std::os::windows::fs::symlink_file(&missing, &sidecar).is_ok();
        #[cfg(not(any(unix, windows)))]
        let linked = false;
        if linked {
            assert!(matches!(
                verify_existing_sqlite_files(&database),
                Err(MessageStoreError::UnsafePath)
            ));
        }
    }

    #[test]
    fn migration_creates_schema_and_enqueue_writes_row() {
        let store = store();
        let result = store.enqueue(request("op1", "hello")).unwrap();

        assert!(!result.message_id.is_empty());
        assert_eq!(result.status, STATUS_QUEUED);
        assert_eq!(store.count_by_status(STATUS_QUEUED), 1);
    }

    #[test]
    fn duplicate_sender_op_id_returns_existing_row() {
        let store = store();
        let first = store.enqueue(request("op1", "hello")).unwrap();
        let mut changed = request("op1", "changed");
        changed.target_fqn = "proj:wg-1/other".into();
        let second = store.enqueue(changed).unwrap();

        assert_eq!(first.message_id, second.message_id);
        assert_eq!(first.target_fqn, second.target_fqn);
        assert!(second.duplicate);
        assert_eq!(store.count_by_status(STATUS_QUEUED), 1);
    }

    #[test]
    fn lease_due_hides_active_lease_until_expiry_then_redelivers() {
        let store = store();
        let first = store.enqueue(request("op1", "hello")).unwrap();
        let now = Utc::now();

        let leased = store
            .lease_due(now, 10, Duration::from_secs(30), "worker")
            .unwrap();
        assert_eq!(leased.len(), 1);
        assert_eq!(leased[0].message_id, first.message_id);

        let none = store
            .lease_due(
                now + chrono::Duration::seconds(1),
                10,
                Duration::from_secs(30),
                "worker",
            )
            .unwrap();
        assert!(none.is_empty());

        let again = store
            .lease_due(
                now + chrono::Duration::seconds(31),
                10,
                Duration::from_secs(30),
                "worker",
            )
            .unwrap();
        assert_eq!(again.len(), 1);
        assert_eq!(again[0].message_id, first.message_id);
    }

    #[test]
    fn delivered_retry_poison_and_reaper_transitions() {
        let store = store();
        let delivered = store.enqueue(request("op1", "ok")).unwrap();
        let retry = store.enqueue(request("op2", "retry")).unwrap();
        let now = Utc::now();

        store.mark_delivered(&delivered.message_id, now).unwrap();
        assert_eq!(store.count_by_status(STATUS_DELIVERED), 1);

        let status = store
            .mark_delivery_failed(&retry.message_id, "boom", now, 2)
            .unwrap();
        assert_eq!(status, STATUS_RETRY);
        let status = store
            .mark_delivery_failed(&retry.message_id, "boom again", now, 2)
            .unwrap();
        assert_eq!(status, STATUS_POISONED);

        let deleted = store
            .reap_terminal_before(now + chrono::Duration::days(2))
            .unwrap();
        assert_eq!(deleted, 2);
    }

    #[test]
    fn oversize_body_rejected() {
        let store = store();
        let too_large = "x".repeat(INLINE_BODY_MAX_BYTES + 1);
        let err = store.enqueue(request("op1", &too_large)).unwrap_err();
        assert!(matches!(err, MessageStoreError::BodyTooLarge));
    }

    #[test]
    fn host_ingress_rejection_is_a_permanent_idempotent_tombstone() {
        let store = store();
        let injection_id = Uuid::new_v4().to_string();
        let now = Utc::now();
        let request = || HostPtyInputRejectionRequest {
            injection_id: injection_id.clone(),
            sender_fqn: "proj:wg-1-team/lead".to_string(),
            target_fqn: "proj:wg-1-team/dev".to_string(),
            op_id: injection_id.clone(),
            nonce_sha256: "a".repeat(64),
            request_fingerprint: "b".repeat(64),
            confirmation_tag: "c".repeat(64),
            sender_incarnation_fingerprint: "d".repeat(64),
            payload_sha256: sha256_hex(b"exact text"),
            payload_bytes: 10,
            issued_at: crate::phone::types::canonical_pty_timestamp(now),
            expires_at: crate::phone::types::canonical_pty_timestamp(
                now + chrono::Duration::minutes(10),
            ),
            reason: crate::phone::types::PtyInputReasonCode::InvalidSessionToken,
        };

        let first = store
            .record_host_pty_input_rejection(request(), now)
            .unwrap();
        let duplicate = store
            .record_host_pty_input_rejection(request(), now)
            .unwrap();
        assert_eq!(first, duplicate);
        assert_eq!(
            first.status,
            crate::phone::types::PtyInputPublicStatus::Rejected
        );
        assert_eq!(
            first.reason.as_ref().map(|reason| reason.code),
            Some(crate::phone::types::PtyInputReasonCode::InvalidSessionToken)
        );
        assert!(store
            .due_pty_input_ids(
                crate::phone::types::PtyInputSourcePlane::HostCli,
                now + chrono::Duration::seconds(1),
                64,
            )
            .unwrap()
            .is_empty());
    }

    #[test]
    fn future_skewed_pty_input_is_queued_at_issuance_and_not_claimed_early() {
        let store = store();
        let op_id = Uuid::new_v4().to_string();
        let now = Utc::now();
        let issued_at = now + chrono::Duration::seconds(20);
        let mut request = pty_request(&op_id, "exact text");
        request.issued_at = crate::phone::types::canonical_pty_timestamp(issued_at);
        request.expires_at =
            crate::phone::types::canonical_pty_timestamp(issued_at + chrono::Duration::minutes(10));

        store.enqueue_pty_input(request).unwrap();
        assert!(store
            .claim_pty_input(
                crate::phone::types::PtyInputSourcePlane::ContainerApi,
                Some(&op_id),
                "test-owner",
                now,
            )
            .unwrap()
            .is_none());
        assert!(store
            .claim_pty_input(
                crate::phone::types::PtyInputSourcePlane::ContainerApi,
                Some(&op_id),
                "test-owner",
                issued_at + chrono::Duration::seconds(1),
            )
            .unwrap()
            .is_some());
    }

    #[test]
    fn pty_input_actuating_clears_payload_and_tombstone_prevents_replay() {
        let store = store();
        let op_id = Uuid::new_v4().to_string();
        let request = pty_request(&op_id, "exact text");
        let first = store.enqueue_pty_input(request).unwrap();
        assert!(!first.duplicate);
        assert_eq!(
            first.result.status,
            crate::phone::types::PtyInputPublicStatus::Queued
        );

        let now = Utc::now();
        let claimed = store
            .claim_pty_input(
                crate::phone::types::PtyInputSourcePlane::ContainerApi,
                Some(&op_id),
                "lease",
                now,
            )
            .unwrap()
            .unwrap();
        assert_eq!(claimed.attempt, 1);
        let payload = store
            .begin_pty_actuating(
                &op_id,
                "lease",
                &Uuid::new_v4().to_string(),
                "localProcess",
                now,
            )
            .unwrap();
        assert_eq!(payload, b"exact text");
        assert_eq!(
            store
                .query_pty_input_by_injection(&op_id)
                .unwrap()
                .unwrap()
                .status,
            crate::phone::types::PtyInputPublicStatus::Actuating
        );
        let terminal = store
            .terminalize_pty_input(
                &op_id,
                crate::phone::types::PtyInputPublicStatus::Injected,
                None,
                now,
            )
            .unwrap();
        assert!(terminal.terminal);
        assert_eq!(
            store
                .compact_pty_terminal_before(now + chrono::Duration::days(8), 64)
                .unwrap(),
            1
        );
        let tombstone = store.query_pty_input_by_injection(&op_id).unwrap().unwrap();
        assert_eq!(
            tombstone.status,
            crate::phone::types::PtyInputPublicStatus::Injected
        );
        let duplicate = store
            .enqueue_pty_input(pty_request(&op_id, "exact text"))
            .unwrap();
        assert!(duplicate.duplicate);
        assert_eq!(duplicate.result.injection_id, op_id);
        let conflict = store
            .enqueue_pty_input(pty_request(&op_id, "changed text"))
            .unwrap_err();
        assert!(matches!(conflict, MessageStoreError::IdempotencyConflict));
    }

    #[test]
    fn permanent_incarnation_keeps_get_and_duplicate_stable_while_mutable_authority_is_pinned() {
        let store = store();
        let op_id = Uuid::new_v4().to_string();
        let first = pty_request(&op_id, "exact text");
        let incarnation = first.sender_incarnation_fingerprint.clone();
        let original_authority = first.sender_identity_fingerprint.clone();
        store.enqueue_pty_input(first).unwrap();

        let mut duplicate = pty_request(&op_id, "exact text");
        duplicate.sender_incarnation_fingerprint = incarnation.clone();
        duplicate.sender_identity_fingerprint = "d".repeat(64);
        duplicate.target_identity_fingerprint = "e".repeat(64);
        let duplicate = store.enqueue_pty_input(duplicate).unwrap();
        assert!(duplicate.duplicate);
        assert!(store
            .query_pty_input("proj:wg-1-team/lead", &op_id, &incarnation,)
            .unwrap()
            .is_some());
        assert!(store
            .query_pty_input("proj:wg-1-team/lead", &op_id, &"f".repeat(64),)
            .unwrap()
            .is_none());

        let claimed = store
            .claim_pty_input(
                crate::phone::types::PtyInputSourcePlane::ContainerApi,
                Some(&op_id),
                "lease",
                Utc::now() + chrono::Duration::seconds(1),
            )
            .unwrap()
            .unwrap();
        assert_eq!(
            claimed.sender_identity_fingerprint, original_authority,
            "an exact duplicate must not overwrite the queued authority snapshot"
        );
    }

    #[test]
    fn terminal_repetition_requires_identical_status_and_fixed_reason() {
        let store = store();
        let op_id = Uuid::new_v4().to_string();
        store
            .enqueue_pty_input(pty_request(&op_id, "exact text"))
            .unwrap();
        let now = Utc::now();
        store
            .claim_pty_input(
                crate::phone::types::PtyInputSourcePlane::ContainerApi,
                Some(&op_id),
                "lease",
                now,
            )
            .unwrap()
            .unwrap();
        store
            .begin_pty_actuating(
                &op_id,
                "lease",
                &Uuid::new_v4().to_string(),
                "localProcess",
                now,
            )
            .unwrap();

        let invalid_reason = store
            .terminalize_pty_input(
                &op_id,
                crate::phone::types::PtyInputPublicStatus::Injected,
                Some(crate::phone::types::PtyInputReasonCode::Busy),
                now,
            )
            .unwrap_err();
        assert!(matches!(
            invalid_reason,
            MessageStoreError::InvalidTransition
        ));

        store
            .terminalize_pty_input(
                &op_id,
                crate::phone::types::PtyInputPublicStatus::Injected,
                Some(crate::phone::types::PtyInputReasonCode::RedundantEnterFailed),
                now,
            )
            .unwrap();
        store
            .terminalize_pty_input(
                &op_id,
                crate::phone::types::PtyInputPublicStatus::Injected,
                Some(crate::phone::types::PtyInputReasonCode::RedundantEnterFailed),
                now,
            )
            .unwrap();
        let conflicting_repeat = store
            .terminalize_pty_input(
                &op_id,
                crate::phone::types::PtyInputPublicStatus::Injected,
                None,
                now,
            )
            .unwrap_err();
        assert!(matches!(
            conflicting_repeat,
            MessageStoreError::InvalidTransition
        ));
    }

    #[tokio::test]
    async fn exact_target_lock_entry_is_removed_after_last_reservation() {
        let locks = Arc::new(PtyInputTargetLocks::default());
        let first = locks.acquire("proj:wg-1-team/dev").await;
        assert_eq!(locks.entry_count(), 1);
        let locks_for_waiter = Arc::clone(&locks);
        let waiter =
            tokio::spawn(async move { locks_for_waiter.acquire("proj:wg-1-team/dev").await });
        tokio::task::yield_now().await;
        drop(first);
        let second = waiter.await.unwrap();
        assert_eq!(locks.entry_count(), 1);
        drop(second);
        assert_eq!(locks.entry_count(), 0);
    }

    #[tokio::test]
    async fn cancelled_target_waiter_releases_its_reservation() {
        let locks = Arc::new(PtyInputTargetLocks::default());
        let held = locks.acquire("proj:wg-1-team/dev").await;
        let locks_for_waiter = Arc::clone(&locks);
        let waiter =
            tokio::spawn(async move { locks_for_waiter.acquire("proj:wg-1-team/dev").await });
        tokio::task::yield_now().await;

        waiter.abort();
        let cancelled = waiter.await;
        assert!(matches!(cancelled, Err(error) if error.is_cancelled()));
        assert_eq!(
            locks.entry_count(),
            1,
            "the holder reservation remains, but the cancelled waiter must be gone"
        );

        drop(held);
        assert_eq!(locks.entry_count(), 0);
    }

    #[test]
    fn operation_stripes_are_stable_and_exclusive() {
        let store = store();
        let id = Uuid::new_v4().to_string();
        let first = store.try_operation_lock(&id).unwrap().unwrap();
        assert!(store.try_operation_lock(&id).unwrap().is_none());
        drop(first);
        assert!(store.try_operation_lock(&id).unwrap().is_some());
    }

    fn enqueue_claim_actuating(
        store: &MessageStore,
        sender_index: usize,
    ) -> (String, DateTime<Utc>) {
        let id = Uuid::new_v4().to_string();
        let mut request = pty_request(&id, "exact text");
        request.sender_fqn = format!("proj:wg-1-team/lead-{sender_index}");
        request.request_fingerprint =
            sha256_hex(format!("request:{sender_index}:{}", request.op_id).as_bytes());
        store.enqueue_pty_input(request).unwrap();
        let transition_at = Utc::now();
        store
            .claim_pty_input(
                crate::phone::types::PtyInputSourcePlane::ContainerApi,
                Some(&id),
                &format!("lease-{id}"),
                transition_at,
            )
            .unwrap()
            .unwrap();
        store
            .begin_pty_actuating(
                &id,
                &format!("lease-{id}"),
                &Uuid::new_v4().to_string(),
                "localProcess",
                transition_at,
            )
            .unwrap();
        (id, transition_at)
    }

    #[test]
    fn startup_recovery_marks_expired_actuating_indeterminate() {
        let store = store();
        let (id, _) = enqueue_claim_actuating(&store, 98);
        let now = Utc::now();
        let issued =
            crate::phone::types::canonical_pty_timestamp(now - chrono::Duration::minutes(20));
        let queued =
            crate::phone::types::canonical_pty_timestamp(now - chrono::Duration::minutes(19));
        let preparing =
            crate::phone::types::canonical_pty_timestamp(now - chrono::Duration::minutes(18));
        let actuating =
            crate::phone::types::canonical_pty_timestamp(now - chrono::Duration::minutes(17));
        let expires =
            crate::phone::types::canonical_pty_timestamp(now - chrono::Duration::minutes(10));
        store
            .conn
            .lock()
            .unwrap()
            .execute(
                r#"UPDATE pty_input_operations
                   SET issued_at=?1,expires_at=?2,queued_at=?3,preparing_at=?4,
                       actuating_at=?5,next_attempt_at=?3,updated_at=?5
                   WHERE injection_id=?6"#,
                params![issued, expires, queued, preparing, actuating, id],
            )
            .unwrap();

        store.recover_pty_input_startup().unwrap();
        let result = store.query_pty_input_by_injection(&id).unwrap().unwrap();
        assert_eq!(
            result.status,
            crate::phone::types::PtyInputPublicStatus::Indeterminate
        );
        assert_eq!(
            result.reason.unwrap().code,
            crate::phone::types::PtyInputReasonCode::DaemonRestartAfterActuation
        );
    }

    #[test]
    fn ambiguous_actuation_commit_returns_no_payload_and_preserves_no_replay_state() {
        let store = store();
        let id = Uuid::new_v4().to_string();
        store
            .enqueue_pty_input(pty_request(&id, "secret text"))
            .unwrap();
        let now = Utc::now();
        store
            .claim_pty_input(
                crate::phone::types::PtyInputSourcePlane::ContainerApi,
                Some(&id),
                "lease",
                now,
            )
            .unwrap()
            .unwrap();
        store.inject_actuation_commit_ambiguous();
        let error = store
            .begin_pty_actuating(
                &id,
                "lease",
                &Uuid::new_v4().to_string(),
                "containerTransport",
                now,
            )
            .unwrap_err();
        assert!(matches!(error, MessageStoreError::ActuationCommitAmbiguous));
        let result = store.query_pty_input_by_injection(&id).unwrap().unwrap();
        assert_eq!(
            result.status,
            crate::phone::types::PtyInputPublicStatus::Actuating
        );
        let payload_is_null: bool = store
            .conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT payload IS NULL FROM pty_input_operations WHERE injection_id=?1",
                [&id],
                |row| row.get(0),
            )
            .unwrap();
        assert!(payload_is_null);
    }

    #[test]
    fn runtime_recovery_requeues_expired_preparation_and_terminalizes_attempt_five() {
        let store = store();
        let retry_id = Uuid::new_v4().to_string();
        let final_id = Uuid::new_v4().to_string();
        for id in [&retry_id, &final_id] {
            store
                .enqueue_pty_input(pty_request(id, "exact text"))
                .unwrap();
        }
        let recovery_at = Utc::now() + chrono::Duration::seconds(1);
        for id in [&retry_id, &final_id] {
            store
                .claim_pty_input(
                    crate::phone::types::PtyInputSourcePlane::ContainerApi,
                    Some(id),
                    &format!("lease-{id}"),
                    recovery_at,
                )
                .unwrap()
                .unwrap();
        }
        let expired = crate::phone::types::canonical_pty_timestamp(
            recovery_at - chrono::Duration::seconds(1),
        );
        store
            .conn
            .lock()
            .unwrap()
            .execute(
                "UPDATE pty_input_operations SET lease_until=?1,attempt=5 WHERE injection_id=?2",
                params![expired, final_id],
            )
            .unwrap();
        store
            .conn
            .lock()
            .unwrap()
            .execute(
                "UPDATE pty_input_operations SET lease_until=?1 WHERE injection_id=?2",
                params![expired, retry_id],
            )
            .unwrap();

        assert_eq!(
            store
                .recover_pty_input_runtime(&HashSet::new(), recovery_at)
                .unwrap(),
            2
        );
        let retry = store
            .query_pty_input_by_injection(&retry_id)
            .unwrap()
            .unwrap();
        assert_eq!(
            retry.status,
            crate::phone::types::PtyInputPublicStatus::Queued
        );
        assert_eq!(
            retry.reason.unwrap().code,
            crate::phone::types::PtyInputReasonCode::LeaseLost
        );
        let final_result = store
            .query_pty_input_by_injection(&final_id)
            .unwrap()
            .unwrap();
        assert_eq!(
            final_result.status,
            crate::phone::types::PtyInputPublicStatus::Rejected
        );
        assert_eq!(
            final_result.reason.unwrap().code,
            crate::phone::types::PtyInputReasonCode::LeaseLost
        );
    }

    #[test]
    fn runtime_orphan_recovery_waits_for_local_and_cross_process_ownership() {
        let store = store();
        let (id, actuating_at) = enqueue_claim_actuating(&store, 0);
        let recovery_now = actuating_at + chrono::Duration::seconds(16);
        let mut active = HashSet::new();
        active.insert(id.clone());
        assert_eq!(
            store
                .recover_pty_input_runtime(&active, recovery_now)
                .unwrap(),
            0
        );
        let operation_lock = store.try_operation_lock(&id).unwrap().unwrap();
        assert_eq!(
            store
                .recover_pty_input_runtime(&HashSet::new(), recovery_now)
                .unwrap(),
            0
        );
        drop(operation_lock);
        assert_eq!(
            store
                .recover_pty_input_runtime(&HashSet::new(), recovery_now)
                .unwrap(),
            1
        );
        let result = store.query_pty_input_by_injection(&id).unwrap().unwrap();
        assert_eq!(
            result.status,
            crate::phone::types::PtyInputPublicStatus::Indeterminate
        );
        assert_eq!(
            result.reason.unwrap().code,
            crate::phone::types::PtyInputReasonCode::RuntimeActuationOrphan
        );
    }

    #[test]
    fn admission_expiry_advances_past_sixty_four_locked_rows() {
        let store = store();
        let now = Utc::now();
        let mut ids = Vec::new();
        for index in 0..65 {
            let id = Uuid::new_v4().to_string();
            let mut request = pty_request(&id, "exact text");
            request.sender_fqn = format!("proj:wg-1-team/lead-{index}");
            request.request_fingerprint = sha256_hex(format!("request:{index}").as_bytes());
            store.enqueue_pty_input(request).unwrap();
            ids.push(id);
        }
        let stripe = |id: &str| {
            let digest = Sha256::digest(id.as_bytes());
            ((usize::from(digest[0]) << 4) | (usize::from(digest[1]) >> 4))
                % PTY_INPUT_OPERATION_LOCK_STRIPES
        };
        let tail = ids
            .iter()
            .find(|candidate| {
                ids.iter()
                    .filter(|other| stripe(other) == stripe(candidate))
                    .count()
                    == 1
            })
            .expect("fixture includes a unique operation stripe")
            .clone();
        let issued =
            crate::phone::types::canonical_pty_timestamp(now - chrono::Duration::minutes(20));
        let queued =
            crate::phone::types::canonical_pty_timestamp(now - chrono::Duration::minutes(19));
        let expired =
            crate::phone::types::canonical_pty_timestamp(now - chrono::Duration::minutes(10));
        store
            .conn
            .lock()
            .unwrap()
            .execute(
                "UPDATE pty_input_operations SET issued_at=?1,queued_at=?2,expires_at=?3,next_attempt_at=?2,updated_at=?2",
                params![issued, queued, expired],
            )
            .unwrap();
        let tail_issued =
            crate::phone::types::canonical_pty_timestamp(now - chrono::Duration::minutes(19));
        let tail_queued =
            crate::phone::types::canonical_pty_timestamp(now - chrono::Duration::minutes(18));
        let tail_expired =
            crate::phone::types::canonical_pty_timestamp(now - chrono::Duration::minutes(9));
        {
            let conn = store.conn.lock().unwrap();
            conn.execute(
                "UPDATE pty_input_operations SET issued_at=?1,queued_at=?2,expires_at=?3,next_attempt_at=?2,updated_at=?2 WHERE injection_id=?4",
                params![tail_issued, tail_queued, tail_expired, tail],
            )
            .unwrap();
            conn.execute(
                "UPDATE pty_input_tombstones SET issued_at=?1,queued_at=?2,expires_at=?3 WHERE injection_id=?4",
                params![tail_issued, tail_queued, tail_expired, tail],
            )
            .unwrap();
        }
        let ordered = {
            let conn = store.conn.lock().unwrap();
            let mut statement = conn
                .prepare(
                    "SELECT injection_id FROM pty_input_operations ORDER BY expires_at,injection_id",
                )
                .unwrap();
            statement
                .query_map([], |row| row.get::<_, String>(0))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
        };
        assert_eq!(ordered[64], tail);
        let locks = ordered[..64]
            .iter()
            .filter_map(|id| store.try_operation_lock(id).unwrap())
            .collect::<Vec<_>>();

        assert_eq!(store.expire_pty_input_for_admission(now).unwrap(), 0);
        assert!(store.query_pty_input_by_injection(&tail).unwrap().is_some());
        assert_eq!(
            store.expire_pty_input_for_admission(now).unwrap(),
            1,
            "a fair admission cursor must reach the independent 65th row"
        );
        drop(locks);
        assert_eq!(
            store.expire_pty_input_for_admission(now).unwrap(),
            64,
            "released prefix rows are revisited immediately after cursor wrap"
        );
    }

    #[test]
    fn runtime_recovery_advances_past_sixty_four_active_rows() {
        let store = store();
        let mut ids = Vec::new();
        let mut latest_transition = Utc::now();
        for index in 0..65 {
            let (id, transition) = enqueue_claim_actuating(&store, index / 16);
            latest_transition = latest_transition.max(transition);
            ids.push(id);
        }
        let tail = ids.pop().unwrap();
        let latest = crate::phone::types::canonical_pty_timestamp(
            latest_transition + chrono::Duration::seconds(1),
        );
        store
            .conn
            .lock()
            .unwrap()
            .execute(
                "UPDATE pty_input_operations SET updated_at=?1,actuating_at=?1 WHERE injection_id=?2",
                params![latest, tail],
            )
            .unwrap();
        let active = ids.into_iter().collect::<HashSet<_>>();
        let recovery_at = latest_transition + chrono::Duration::seconds(20);

        assert_eq!(
            store
                .recover_pty_input_runtime(&active, recovery_at)
                .unwrap(),
            0
        );
        assert_eq!(
            store
                .recover_pty_input_runtime(&active, recovery_at)
                .unwrap(),
            1,
            "a fair runtime cursor must reach the independent 65th orphan"
        );
        let result = store.query_pty_input_by_injection(&tail).unwrap().unwrap();
        assert_eq!(
            result.reason.map(|reason| reason.code),
            Some(crate::phone::types::PtyInputReasonCode::RuntimeActuationOrphan)
        );
        assert_eq!(
            store
                .recover_pty_input_runtime(&active, recovery_at)
                .unwrap(),
            0,
            "active prefix rows remain skipped after the cursor wraps"
        );
        assert_eq!(
            store
                .recover_pty_input_runtime(&HashSet::new(), recovery_at)
                .unwrap(),
            64,
            "formerly active prefix rows are revisited after cursor wrap"
        );
    }

    #[test]
    fn due_container_cursor_advances_past_sixty_four_lock_contended_rows() {
        let store = store();
        let mut ids = Vec::new();
        for index in 0..65 {
            let id = Uuid::new_v4().to_string();
            let mut request = pty_request(&id, "exact text");
            request.sender_fqn = format!("proj:wg-1-team/lead-{index}");
            request.target_fqn = format!("proj:wg-1-team/dev-{index}");
            request.request_fingerprint = sha256_hex(format!("request:{index}").as_bytes());
            store.enqueue_pty_input(request).unwrap();
            ids.push(id);
        }
        let stripe = |id: &str| {
            let digest = Sha256::digest(id.as_bytes());
            ((usize::from(digest[0]) << 4) | (usize::from(digest[1]) >> 4))
                % PTY_INPUT_OPERATION_LOCK_STRIPES
        };
        let tail = ids
            .iter()
            .find(|candidate| {
                ids.iter()
                    .filter(|other| stripe(other) == stripe(candidate))
                    .count()
                    == 1
            })
            .unwrap()
            .clone();
        let base_at = Utc::now() + chrono::Duration::seconds(1);
        let base = crate::phone::types::canonical_pty_timestamp(base_at);
        let tail_next = crate::phone::types::canonical_pty_timestamp(
            base_at + chrono::Duration::milliseconds(1),
        );
        {
            let conn = store.conn.lock().unwrap();
            conn.execute(
                "UPDATE pty_input_operations SET queued_at=?1,next_attempt_at=?1,updated_at=?1",
                [&base],
            )
            .unwrap();
            conn.execute(
                "UPDATE pty_input_operations SET next_attempt_at=?1,updated_at=?1 WHERE injection_id=?2",
                params![tail_next, tail],
            )
            .unwrap();
        }
        let dispatch_at = base_at + chrono::Duration::seconds(1);
        let first_page = store
            .due_container_pty_input_candidates_fair(dispatch_at, 64)
            .unwrap();
        assert_eq!(first_page.len(), 64);
        assert!(first_page
            .iter()
            .all(|candidate| candidate.injection_id != tail));
        let locks = first_page
            .iter()
            .filter_map(|candidate| store.try_operation_lock(&candidate.injection_id).unwrap())
            .collect::<Vec<_>>();
        assert!(first_page.iter().all(|candidate| store
            .try_operation_lock(&candidate.injection_id)
            .unwrap()
            .is_none()));

        let second_page = store
            .due_container_pty_input_candidates_fair(dispatch_at, 64)
            .unwrap();
        assert_eq!(second_page.len(), 1);
        assert_eq!(second_page[0].injection_id, tail);
        assert!(store.try_operation_lock(&tail).unwrap().is_some());
        drop(locks);
    }

    #[test]
    fn startup_recovery_pages_past_sixty_four_locked_rows() {
        let store = store();
        let mut ids = Vec::new();
        for index in 0..65 {
            ids.push(enqueue_claim_actuating(&store, index / 16).0);
        }
        let stripe = |id: &str| {
            let digest = Sha256::digest(id.as_bytes());
            ((usize::from(digest[0]) << 4) | (usize::from(digest[1]) >> 4))
                % PTY_INPUT_OPERATION_LOCK_STRIPES
        };
        let unlocked = ids
            .iter()
            .find(|candidate| {
                let candidate_stripe = stripe(candidate);
                ids.iter()
                    .filter(|other| stripe(other) == candidate_stripe)
                    .count()
                    == 1
            })
            .expect("65 UUIDs must include an operation stripe without a collision")
            .clone();
        let latest =
            crate::phone::types::canonical_pty_timestamp(Utc::now() + chrono::Duration::hours(1));
        store
            .conn
            .lock()
            .unwrap()
            .execute(
                "UPDATE pty_input_operations SET updated_at=?1 WHERE injection_id=?2",
                params![latest, unlocked],
            )
            .unwrap();
        let ordered = {
            let conn = store.conn.lock().unwrap();
            let mut statement = conn
                .prepare(
                    "SELECT injection_id FROM pty_input_operations ORDER BY updated_at,injection_id",
                )
                .unwrap();
            statement
                .query_map([], |row| row.get::<_, String>(0))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
        };
        assert_eq!(ordered[64], unlocked);
        let locks = ordered[..64]
            .iter()
            .filter_map(|id| store.try_operation_lock(id).unwrap())
            .collect::<Vec<_>>();
        store.recover_pty_input_startup().unwrap();
        for id in &ordered[..64] {
            assert_eq!(
                store
                    .query_pty_input_by_injection(id)
                    .unwrap()
                    .unwrap()
                    .status,
                crate::phone::types::PtyInputPublicStatus::Actuating
            );
        }
        assert_eq!(
            store
                .query_pty_input_by_injection(&ordered[64])
                .unwrap()
                .unwrap()
                .status,
            crate::phone::types::PtyInputPublicStatus::Indeterminate
        );
        drop(locks);
        assert_eq!(ids.len(), 65);
    }

    #[test]
    fn startup_recovery_terminalizes_a_fifth_preparation_attempt() {
        let store = store();
        let id = Uuid::new_v4().to_string();
        store
            .enqueue_pty_input(pty_request(&id, "exact text"))
            .unwrap();
        store
            .claim_pty_input(
                crate::phone::types::PtyInputSourcePlane::ContainerApi,
                Some(&id),
                "lease",
                Utc::now() + chrono::Duration::seconds(1),
            )
            .unwrap()
            .unwrap();
        store
            .conn
            .lock()
            .unwrap()
            .execute(
                "UPDATE pty_input_operations SET attempt=5 WHERE injection_id=?1",
                [&id],
            )
            .unwrap();
        store.recover_pty_input_startup().unwrap();
        let result = store.query_pty_input_by_injection(&id).unwrap().unwrap();
        assert_eq!(
            result.status,
            crate::phone::types::PtyInputPublicStatus::Rejected
        );
        assert_eq!(
            result.reason.unwrap().code,
            crate::phone::types::PtyInputReasonCode::LeaseLost
        );
    }

    #[test]
    fn compaction_rejects_any_tombstone_mismatch() {
        let store = store();
        let (id, actuating_at) = enqueue_claim_actuating(&store, 0);
        let terminal_at = actuating_at + chrono::Duration::seconds(1);
        store
            .terminalize_pty_input(
                &id,
                crate::phone::types::PtyInputPublicStatus::Injected,
                None,
                terminal_at,
            )
            .unwrap();
        store
            .conn
            .lock()
            .unwrap()
            .execute(
                "UPDATE pty_input_tombstones SET target_fqn='tampered' WHERE injection_id=?1",
                [&id],
            )
            .unwrap();
        let error = store
            .compact_pty_terminal_before(terminal_at + chrono::Duration::days(8), 64)
            .unwrap_err();
        assert!(matches!(error, MessageStoreError::StoreCorrupt));
        let live_count: i64 = store
            .conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM pty_input_operations WHERE injection_id=?1",
                [&id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(live_count, 1);
    }

    #[test]
    fn compaction_advances_past_sixty_four_locked_terminal_rows() {
        let store = store();
        let mut ids = Vec::new();
        let mut cutoff = Utc::now();
        for index in 0..65 {
            let (id, actuating_at) = enqueue_claim_actuating(&store, index / 16);
            let terminal_at = actuating_at + chrono::Duration::seconds(1);
            store
                .terminalize_pty_input(
                    &id,
                    crate::phone::types::PtyInputPublicStatus::Injected,
                    None,
                    terminal_at,
                )
                .unwrap();
            cutoff = cutoff.max(terminal_at + chrono::Duration::days(8));
            ids.push(id);
        }
        let stripe = |id: &str| {
            let digest = Sha256::digest(id.as_bytes());
            ((usize::from(digest[0]) << 4) | (usize::from(digest[1]) >> 4))
                % PTY_INPUT_OPERATION_LOCK_STRIPES
        };
        let tail = ids
            .iter()
            .find(|candidate| {
                ids.iter()
                    .filter(|other| stripe(other) == stripe(candidate))
                    .count()
                    == 1
            })
            .expect("fixture includes a unique operation stripe")
            .clone();
        let latest_at = cutoff - chrono::Duration::days(8) + chrono::Duration::hours(1);
        cutoff = latest_at + chrono::Duration::days(8);
        let latest = crate::phone::types::canonical_pty_timestamp(latest_at);
        {
            let conn = store.conn.lock().unwrap();
            conn.execute(
                "UPDATE pty_input_operations SET terminal_at=?1,updated_at=?1 WHERE injection_id=?2",
                params![latest, tail],
            )
            .unwrap();
            conn.execute(
                "UPDATE pty_input_tombstones SET terminal_at=?1 WHERE injection_id=?2",
                params![latest, tail],
            )
            .unwrap();
        }
        let ordered = {
            let conn = store.conn.lock().unwrap();
            let mut statement = conn
                .prepare(
                    "SELECT injection_id FROM pty_input_operations ORDER BY terminal_at,injection_id",
                )
                .unwrap();
            statement
                .query_map([], |row| row.get::<_, String>(0))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
        };
        assert_eq!(ordered[64], tail);
        let locks = ordered[..64]
            .iter()
            .filter_map(|id| store.try_operation_lock(id).unwrap())
            .collect::<Vec<_>>();

        assert_eq!(store.compact_pty_terminal_before(cutoff, 64).unwrap(), 0);
        assert_eq!(
            store.compact_pty_terminal_before(cutoff, 64).unwrap(),
            1,
            "a fair compaction cursor must reach the independent 65th row"
        );
        drop(locks);
        assert_eq!(
            store.compact_pty_terminal_before(cutoff, 64).unwrap(),
            64,
            "released prefix rows are revisited immediately after cursor wrap"
        );
    }

    #[test]
    fn maintenance_compaction_advances_past_sixty_four_locked_terminal_rows() {
        let store = store();
        let mut ids = Vec::new();
        for index in 0..65 {
            let (id, actuating_at) = enqueue_claim_actuating(&store, index / 16);
            store
                .terminalize_pty_input(
                    &id,
                    crate::phone::types::PtyInputPublicStatus::Injected,
                    None,
                    actuating_at + chrono::Duration::seconds(1),
                )
                .unwrap();
            ids.push(id);
        }
        let stripe = |id: &str| {
            let digest = Sha256::digest(id.as_bytes());
            ((usize::from(digest[0]) << 4) | (usize::from(digest[1]) >> 4))
                % PTY_INPUT_OPERATION_LOCK_STRIPES
        };
        let tail = ids
            .iter()
            .find(|candidate| {
                ids.iter()
                    .filter(|other| stripe(other) == stripe(candidate))
                    .count()
                    == 1
            })
            .unwrap()
            .clone();
        let base_at = Utc::now() + chrono::Duration::seconds(1);
        let old = crate::phone::types::canonical_pty_timestamp(base_at);
        let tail_terminal_at = base_at + chrono::Duration::milliseconds(1);
        let tail_at = crate::phone::types::canonical_pty_timestamp(tail_terminal_at);
        let maintenance_at = tail_terminal_at + chrono::Duration::days(8);
        {
            let conn = store.conn.lock().unwrap();
            conn.execute(
                "UPDATE pty_input_operations SET terminal_at=?1,updated_at=?1",
                [&old],
            )
            .unwrap();
            conn.execute("UPDATE pty_input_tombstones SET terminal_at=?1", [&old])
                .unwrap();
            conn.execute(
                "UPDATE pty_input_operations SET terminal_at=?1,updated_at=?1 WHERE injection_id=?2",
                params![tail_at, tail],
            )
            .unwrap();
            conn.execute(
                "UPDATE pty_input_tombstones SET terminal_at=?1 WHERE injection_id=?2",
                params![tail_at, tail],
            )
            .unwrap();
        }
        let ordered = {
            let conn = store.conn.lock().unwrap();
            let mut statement = conn
                .prepare(
                    "SELECT injection_id FROM pty_input_operations ORDER BY terminal_at,injection_id",
                )
                .unwrap();
            statement
                .query_map([], |row| row.get::<_, String>(0))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
        };
        assert_eq!(ordered[64], tail);
        let locks = ordered[..64]
            .iter()
            .filter_map(|id| store.try_operation_lock(id).unwrap())
            .collect::<Vec<_>>();

        assert_eq!(
            store
                .compact_pty_terminal_maintenance(maintenance_at, 64)
                .unwrap(),
            0
        );
        assert_eq!(
            store
                .compact_pty_terminal_maintenance(maintenance_at, 64)
                .unwrap(),
            1,
            "runtime maintenance must reach the independent 65th row"
        );
        drop(locks);
        assert_eq!(
            store
                .compact_pty_terminal_maintenance(maintenance_at, 64)
                .unwrap(),
            64,
            "released maintenance prefix rows are revisited immediately after cursor wrap"
        );
    }

    #[test]
    fn compaction_is_bounded_to_sixty_four_rows_per_call() {
        let store = store();
        let mut latest_terminal = Utc::now();
        for index in 0..65 {
            let (id, actuating_at) = enqueue_claim_actuating(&store, index / 16);
            let terminal_at = actuating_at + chrono::Duration::seconds(1);
            store
                .terminalize_pty_input(
                    &id,
                    crate::phone::types::PtyInputPublicStatus::Injected,
                    None,
                    terminal_at,
                )
                .unwrap();
            latest_terminal = latest_terminal.max(terminal_at);
        }
        let cutoff = latest_terminal + chrono::Duration::days(8);
        assert_eq!(
            store
                .compact_pty_terminal_before(cutoff, usize::MAX)
                .unwrap(),
            64
        );
        assert_eq!(
            store
                .compact_pty_terminal_before(cutoff, usize::MAX)
                .unwrap(),
            1
        );
    }

    #[tokio::test]
    async fn heartbeat_finish_joins_and_prevents_late_renewal() {
        let store = Arc::new(store());
        let id = Uuid::new_v4().to_string();
        store
            .enqueue_pty_input(pty_request(&id, "exact text"))
            .unwrap();
        store
            .claim_pty_input(
                crate::phone::types::PtyInputSourcePlane::ContainerApi,
                Some(&id),
                "lease",
                Utc::now() + chrono::Duration::seconds(1),
            )
            .unwrap()
            .unwrap();
        let mut heartbeat = PreparationHeartbeatGuard::start_with_interval(
            Arc::clone(&store),
            id.clone(),
            "lease".to_string(),
            Utc::now() + chrono::Duration::minutes(5),
            Duration::from_millis(5),
        );
        tokio::time::sleep(Duration::from_millis(15)).await;
        assert!(heartbeat.finish().await);
        assert!(
            heartbeat.finish().await,
            "finishing an already joined heartbeat is safe"
        );
        let updated_before: String = store
            .conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT updated_at FROM pty_input_operations WHERE injection_id=?1",
                [&id],
                |row| row.get(0),
            )
            .unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;
        let updated_after: String = store
            .conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT updated_at FROM pty_input_operations WHERE injection_id=?1",
                [&id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(updated_before, updated_after);
        assert!(store
            .begin_pty_actuating(
                &id,
                "lease",
                &Uuid::new_v4().to_string(),
                "localProcess",
                Utc::now(),
            )
            .is_ok());
    }

    #[test]
    fn backoff_is_bounded_exponential() {
        assert_eq!(retry_backoff_seconds(1), 5);
        assert_eq!(retry_backoff_seconds(2), 10);
        assert_eq!(retry_backoff_seconds(6), 160);
        assert_eq!(retry_backoff_seconds(100), 160);
    }

    #[test]
    fn test_release_delivery_lease_resets_status_queued_and_preserves_attempts() {
        let store = store();
        let res = store.enqueue(request("op-rel-1", "hello")).unwrap();
        let id = res.message_id;

        // Lease the message
        let leased = store
            .lease_due(Utc::now(), 10, Duration::from_secs(60), "worker-1")
            .unwrap();
        assert_eq!(leased.len(), 1);
        assert_eq!(leased[0].message_id, id);

        // Fail once so attempt count is > 0
        let status = store
            .mark_delivery_failed(&id, "transient error", Utc::now(), 5)
            .unwrap();
        assert_eq!(status, STATUS_RETRY);

        // Lease again
        let leased = store
            .lease_due(
                Utc::now() + chrono::Duration::seconds(10),
                10,
                Duration::from_secs(60),
                "worker-1",
            )
            .unwrap();
        assert_eq!(leased.len(), 1);
        assert_eq!(leased[0].message_id, id);
        assert_eq!(leased[0].attempt, 1);

        // Release delivery lease (menu guard deferred)
        let now = Utc::now();
        store
            .release_delivery_lease(&id, "session blocked by interactive menu", now)
            .unwrap();

        // Verify status and attempt in DB
        let (status, attempt, lease_owner, lease_until, last_error): (
            String,
            i64,
            Option<String>,
            Option<String>,
            Option<String>,
        ) = store
            .conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT status, attempt, lease_owner, lease_until, last_error FROM messages WHERE message_id = ?1",
                [&id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
            )
            .unwrap();

        assert_eq!(status, STATUS_QUEUED);
        assert_eq!(attempt, 1); // Preserved!
        assert!(lease_owner.is_none());
        assert!(lease_until.is_none());
        assert_eq!(
            last_error,
            Some("session blocked by interactive menu".to_string())
        );

        // Check audit record
        let audit_status: String = store
            .conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT status FROM message_audit WHERE message_id = ?1 ORDER BY at DESC LIMIT 1",
                [&id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(audit_status, "lease-released-deferred");
    }

    #[test]
    fn test_retry_pty_input_menu_guard_blocked_persisted() {
        let store = store();
        let id = Uuid::new_v4().to_string();
        store.enqueue_pty_input(pty_request(&id, "hello")).unwrap();

        // Lease operation
        store
            .claim_pty_input(
                crate::phone::types::PtyInputSourcePlane::ContainerApi,
                Some(&id),
                "lease-1",
                Utc::now(),
            )
            .unwrap();

        // Retry with MenuGuardBlocked
        store
            .retry_pty_input(
                &id,
                "lease-1",
                crate::phone::types::PtyInputReasonCode::MenuGuardBlocked,
                Utc::now(),
            )
            .unwrap();

        let (status, reason_code, reason_detail): (String, Option<String>, Option<String>) = store
            .conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT status, reason_code, reason_detail FROM pty_input_operations WHERE injection_id = ?1",
                [&id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();

        assert_eq!(status, "retry");
        assert_eq!(reason_code, Some("menu_guard_blocked".to_string()));
        assert_eq!(
            reason_detail,
            Some("The target session is blocked by an interactive menu.".to_string())
        );
    }

    fn setup_v2_database(path: &std::path::Path) -> rusqlite::Connection {
        let conn = rusqlite::Connection::open(path).unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS api_message_schema(
                version INTEGER PRIMARY KEY,
                applied_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS messages(
                message_id TEXT PRIMARY KEY,
                sender_fqn TEXT NOT NULL,
                target_fqn TEXT NOT NULL,
                op_id TEXT NOT NULL,
                content_type TEXT NOT NULL,
                body TEXT NOT NULL,
                body_sha256 TEXT NOT NULL,
                body_bytes INTEGER NOT NULL,
                source_plane TEXT NOT NULL,
                source_ref TEXT NULL,
                status TEXT NOT NULL,
                attempt INTEGER NOT NULL DEFAULT 0,
                next_attempt_at TEXT NOT NULL,
                lease_owner TEXT NULL,
                lease_until TEXT NULL,
                created_at TEXT NOT NULL,
                delivered_at TEXT NULL,
                last_error TEXT NULL,
                UNIQUE(sender_fqn, op_id)
            );
            CREATE INDEX IF NOT EXISTS idx_messages_due
                ON messages(status, next_attempt_at, lease_until);
            CREATE TABLE IF NOT EXISTS message_audit(
                event_id TEXT PRIMARY KEY,
                message_id TEXT NOT NULL,
                status TEXT NOT NULL,
                detail TEXT NULL,
                at TEXT NOT NULL,
                FOREIGN KEY(message_id) REFERENCES messages(message_id) ON DELETE CASCADE
            );

            INSERT INTO api_message_schema(version, applied_at)

            VALUES(1, '2026-08-30T00:00:00.000Z');

            CREATE TABLE pty_input_operations(
                injection_id TEXT PRIMARY KEY,
                sender_fqn TEXT NOT NULL,
                target_fqn TEXT NOT NULL,
                op_id TEXT NOT NULL,
                nonce_sha256 TEXT NOT NULL,
                request_fingerprint TEXT NOT NULL,
                confirmation_tag TEXT NULL,
                version INTEGER NOT NULL CHECK(version = 1),
                enter_mode TEXT NOT NULL CHECK(enter_mode = 'agent-submit'),
                requested_agent_id TEXT NULL,
                payload BLOB NULL,
                payload_sha256 TEXT NOT NULL,
                payload_bytes INTEGER NOT NULL CHECK(payload_bytes BETWEEN 1 AND 65536),
                source_plane TEXT NOT NULL CHECK(source_plane IN ('host_cli','container_api')),
                sender_incarnation_fingerprint TEXT NOT NULL,
                sender_identity_fingerprint TEXT NULL,
                target_identity_fingerprint TEXT NULL,
                authority_session_id TEXT NULL,
                authority_client_id TEXT NULL,
                authority_client_generation TEXT NULL,
                status TEXT NOT NULL CHECK(status IN ('queued','preparing','retry','actuating','injected','rejected','indeterminate')),
                attempt INTEGER NOT NULL DEFAULT 0 CHECK(attempt BETWEEN 0 AND 5),
                next_attempt_at TEXT NOT NULL,
                lease_owner TEXT NULL,
                lease_until TEXT NULL,
                selected_session_id TEXT NULL,
                selected_backend TEXT NULL,
                issued_at TEXT NOT NULL,
                expires_at TEXT NOT NULL,
                queued_at TEXT NOT NULL,
                preparing_at TEXT NULL,
                actuating_at TEXT NULL,
                terminal_at TEXT NULL,
                host_artifact_at TEXT NULL,
                updated_at TEXT NOT NULL,
                reason_code TEXT NULL,
                reason_detail TEXT NULL,
                UNIQUE(sender_fqn, op_id),
                UNIQUE(sender_fqn, nonce_sha256),
                CHECK(length(injection_id)=36
                      AND substr(injection_id,9,1)='-'
                      AND substr(injection_id,14,1)='-'
                      AND substr(injection_id,15,1)='4'
                      AND substr(injection_id,19,1)='-'
                      AND substr(injection_id,20,1) GLOB '[89ab]'
                      AND substr(injection_id,24,1)='-'
                      AND length(replace(injection_id,'-',''))=32
                      AND replace(injection_id,'-','') NOT GLOB '*[^0-9a-f]*'),
                CHECK(length(op_id)=36
                      AND substr(op_id,9,1)='-'
                      AND substr(op_id,14,1)='-'
                      AND substr(op_id,15,1)='4'
                      AND substr(op_id,19,1)='-'
                      AND substr(op_id,20,1) GLOB '[89ab]'
                      AND substr(op_id,24,1)='-'
                      AND length(replace(op_id,'-',''))=32
                      AND replace(op_id,'-','') NOT GLOB '*[^0-9a-f]*'),
                CHECK(authority_session_id IS NULL OR
                      (length(authority_session_id)=36
                       AND substr(authority_session_id,9,1)='-'
                       AND substr(authority_session_id,14,1)='-'
                       AND substr(authority_session_id,15,1)='4'
                       AND substr(authority_session_id,19,1)='-'
                       AND substr(authority_session_id,20,1) GLOB '[89ab]'
                       AND substr(authority_session_id,24,1)='-'
                       AND length(replace(authority_session_id,'-',''))=32
                       AND replace(authority_session_id,'-','') NOT GLOB '*[^0-9a-f]*')),
                CHECK(authority_client_generation IS NULL OR
                      (length(authority_client_generation)=36
                       AND substr(authority_client_generation,9,1)='-'
                       AND substr(authority_client_generation,14,1)='-'
                       AND substr(authority_client_generation,15,1)='4'
                       AND substr(authority_client_generation,19,1)='-'
                       AND substr(authority_client_generation,20,1) GLOB '[89ab]'
                       AND substr(authority_client_generation,24,1)='-'
                       AND length(replace(authority_client_generation,'-',''))=32
                       AND replace(authority_client_generation,'-','') NOT GLOB '*[^0-9a-f]*')),
                CHECK(selected_session_id IS NULL OR
                      (length(selected_session_id)=36
                       AND substr(selected_session_id,9,1)='-'
                       AND substr(selected_session_id,14,1)='-'
                       AND substr(selected_session_id,15,1)='4'
                       AND substr(selected_session_id,19,1)='-'
                       AND substr(selected_session_id,20,1) GLOB '[89ab]'
                       AND substr(selected_session_id,24,1)='-'
                       AND length(replace(selected_session_id,'-',''))=32
                       AND replace(selected_session_id,'-','') NOT GLOB '*[^0-9a-f]*')),
                CHECK(length(nonce_sha256)=64 AND nonce_sha256 NOT GLOB '*[^0-9a-f]*'),
                CHECK(length(request_fingerprint)=64 AND request_fingerprint NOT GLOB '*[^0-9a-f]*'),
                CHECK(length(payload_sha256)=64 AND payload_sha256 NOT GLOB '*[^0-9a-f]*'),
                CHECK(length(sender_incarnation_fingerprint)=64 AND sender_incarnation_fingerprint NOT GLOB '*[^0-9a-f]*'),
                CHECK(confirmation_tag IS NULL OR (length(confirmation_tag)=64 AND confirmation_tag NOT GLOB '*[^0-9a-f]*')),
                CHECK(sender_identity_fingerprint IS NULL OR (length(sender_identity_fingerprint)=64 AND sender_identity_fingerprint NOT GLOB '*[^0-9a-f]*')),
                CHECK(target_identity_fingerprint IS NULL OR (length(target_identity_fingerprint)=64 AND target_identity_fingerprint NOT GLOB '*[^0-9a-f]*')),
                CHECK(payload IS NULL OR length(payload)=payload_bytes),
                CHECK(length(issued_at)=24 AND length(expires_at)=24 AND length(queued_at)=24
                      AND length(next_attempt_at)=24 AND length(updated_at)=24),
                CHECK(issued_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9]Z'),
                CHECK(expires_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9]Z'),
                CHECK(queued_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9]Z'),
                CHECK(next_attempt_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9]Z'),
                CHECK(updated_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9]Z'),
                CHECK(preparing_at IS NULL OR (length(preparing_at)=24 AND preparing_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9]Z')),
                CHECK(actuating_at IS NULL OR (length(actuating_at)=24 AND actuating_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9]Z')),
                CHECK(terminal_at IS NULL OR (length(terminal_at)=24 AND terminal_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9]Z')),
                CHECK(host_artifact_at IS NULL OR (length(host_artifact_at)=24 AND host_artifact_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9]Z')),
                CHECK((selected_session_id IS NULL) = (selected_backend IS NULL)),
                CHECK(selected_backend IS NULL OR selected_backend IN ('localProcess','containerTransport')),
                CHECK(reason_code IS NULL OR reason_code IN (
                    'invalid_envelope','mixed_payload','unsupported_version','invalid_enter_mode',
                    'invalid_id','invalid_nonce','invalid_timestamp','expired','invalid_target',
                    'invalid_text','payload_too_large','idempotency_conflict','capacity_exceeded',
                    'session_token_required','invalid_session_token','ambiguous_session_token',
                    'sender_session_not_live','sender_backend_not_local','sender_identity_invalid',
                    'sender_not_coordinator','root_identity_invalid','target_not_member',
                    'target_is_coordinator','target_out_of_scope','unsafe_path','api_scope_required',
                    'api_client_unbound','api_client_stale','api_binding_mismatch','authority_changed',
                    'busy','resize_unsettled','untracked_readiness','unsupported_session',
                    'nonpersistent_live_session','inconsistent_session','unsupported_profile',
                    'readiness_timeout','store_corrupt','restore_in_progress','purge_in_progress',
                    'session_race','lease_lost','spawn_failed_safe','store_transient',
                    'final_revalidation_failed','text_write_failed','required_enter_failed',
                    'daemon_restart_after_actuation','runtime_actuation_orphan','terminal_store_failed',
                    'redundant_enter_failed','boundary_metadata_failed','artifact_unclaimed'
                )),
                CHECK((reason_code IS NULL) = (reason_detail IS NULL)),
                CHECK(
                    (status IN ('queued','preparing','retry')
                     AND payload IS NOT NULL
                     AND authority_session_id IS NOT NULL
                     AND sender_identity_fingerprint IS NOT NULL
                     AND target_identity_fingerprint IS NOT NULL
                     AND actuating_at IS NULL AND terminal_at IS NULL
                     AND selected_session_id IS NULL AND selected_backend IS NULL)
                    OR
                    (status='rejected' AND payload IS NULL AND requested_agent_id IS NULL
                     AND authority_session_id IS NULL AND authority_client_id IS NULL
                     AND authority_client_generation IS NULL
                     AND sender_identity_fingerprint IS NULL
                     AND target_identity_fingerprint IS NULL
                     AND actuating_at IS NULL AND terminal_at IS NOT NULL
                     AND selected_session_id IS NULL AND selected_backend IS NULL)
                    OR
                    (status IN ('actuating','injected','indeterminate')
                     AND payload IS NULL AND requested_agent_id IS NULL
                     AND authority_session_id IS NULL AND authority_client_id IS NULL
                     AND authority_client_generation IS NULL
                     AND sender_identity_fingerprint IS NULL
                     AND target_identity_fingerprint IS NULL
                     AND actuating_at IS NOT NULL
                     AND selected_session_id IS NOT NULL AND selected_backend IS NOT NULL)
                ),
                CHECK((source_plane='host_cli' AND confirmation_tag IS NOT NULL
                       AND authority_client_id IS NULL AND authority_client_generation IS NULL)
                   OR (source_plane='container_api' AND confirmation_tag IS NULL
                       AND ((status IN ('queued','preparing','retry')
                             AND authority_client_id IS NOT NULL
                             AND authority_client_generation IS NOT NULL)
                         OR (status IN ('actuating','injected','rejected','indeterminate')
                             AND authority_client_id IS NULL
                             AND authority_client_generation IS NULL)))),
                CHECK((status IN ('injected','rejected','indeterminate')) = (terminal_at IS NOT NULL)),
                CHECK((status = 'preparing') = (lease_owner IS NOT NULL AND lease_until IS NOT NULL)),
                CHECK(status!='preparing' OR preparing_at IS NOT NULL),
                CHECK(queued_at>=issued_at AND queued_at<expires_at),
                CHECK(actuating_at IS NULL OR (actuating_at>=queued_at AND actuating_at<expires_at)),
                CHECK(terminal_at IS NULL OR terminal_at>=queued_at),
                CHECK(host_artifact_at IS NULL OR terminal_at IS NOT NULL),
                CHECK((status IN ('queued','preparing','retry') AND
                       (reason_code IS NULL OR reason_code IN (
                         'restore_in_progress','purge_in_progress','session_race',
                         'lease_lost','spawn_failed_safe','store_transient')))
                   OR (status='actuating' AND reason_code IS NULL)
                   OR (status='injected' AND
                       (reason_code IS NULL OR reason_code IN (
                         'redundant_enter_failed','boundary_metadata_failed')))
                   OR (status='rejected' AND reason_code IS NOT NULL AND reason_code NOT IN (
                         'final_revalidation_failed','text_write_failed','required_enter_failed',
                         'daemon_restart_after_actuation','runtime_actuation_orphan',
                         'terminal_store_failed','redundant_enter_failed',
                         'boundary_metadata_failed','artifact_unclaimed'))
                   OR (status='indeterminate' AND reason_code IN (
                         'final_revalidation_failed','text_write_failed','required_enter_failed',
                         'daemon_restart_after_actuation','runtime_actuation_orphan',
                         'terminal_store_failed'))),
                CHECK(source_plane != 'host_cli' OR injection_id=op_id)
            );
            CREATE INDEX idx_pty_input_due
                ON pty_input_operations(source_plane, status, next_attempt_at, lease_until);
            CREATE TABLE pty_input_audit(
                event_id TEXT PRIMARY KEY,
                injection_id TEXT NOT NULL,
                op_id TEXT NOT NULL,
                sender_fqn TEXT NOT NULL,
                target_fqn TEXT NOT NULL,
                version INTEGER NOT NULL,
                payload_bytes INTEGER NOT NULL,
                payload_sha256 TEXT NOT NULL,
                source_plane TEXT NOT NULL,
                selected_session_id TEXT NULL,
                selected_backend TEXT NULL,
                status TEXT NOT NULL,
                reason_code TEXT NULL,
                at TEXT NOT NULL,
                FOREIGN KEY(injection_id) REFERENCES pty_input_operations(injection_id) ON DELETE CASCADE,
                CHECK(version=1),
                CHECK(payload_bytes BETWEEN 1 AND 65536),
                CHECK(length(payload_sha256)=64 AND payload_sha256 NOT GLOB '*[^0-9a-f]*'),
                CHECK(source_plane IN ('host_cli','container_api')),
                CHECK(status IN ('queued','preparing','retry','actuating','injected','rejected','indeterminate')),
                CHECK((selected_session_id IS NULL) = (selected_backend IS NULL)),
                CHECK(selected_backend IS NULL OR selected_backend IN ('localProcess','containerTransport')),
                CHECK(length(at)=24 AND at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9]Z')
            );
            CREATE TABLE pty_input_tombstones(
                injection_id TEXT PRIMARY KEY,
                sender_fqn TEXT NOT NULL,
                target_fqn TEXT NOT NULL,
                op_id TEXT NOT NULL,
                nonce_sha256 TEXT NOT NULL,
                request_fingerprint TEXT NOT NULL,
                confirmation_tag TEXT NULL,
                sender_incarnation_fingerprint TEXT NOT NULL,
                version INTEGER NOT NULL,
                payload_sha256 TEXT NOT NULL,
                payload_bytes INTEGER NOT NULL,
                source_plane TEXT NOT NULL,
                selected_session_id TEXT NULL,
                selected_backend TEXT NULL,
                issued_at TEXT NOT NULL,
                expires_at TEXT NOT NULL,
                queued_at TEXT NOT NULL,
                actuating_at TEXT NULL,
                terminal_at TEXT NOT NULL,
                status TEXT NOT NULL CHECK(status IN ('injected','rejected','indeterminate')),
                reason_code TEXT NULL,
                reason_detail TEXT NULL,
                UNIQUE(sender_fqn, op_id),
                UNIQUE(sender_fqn, nonce_sha256),
                CHECK(version=1),
                CHECK(payload_bytes BETWEEN 1 AND 65536),
                CHECK(length(injection_id)=36
                      AND substr(injection_id,9,1)='-'
                      AND substr(injection_id,14,1)='-'
                      AND substr(injection_id,15,1)='4'
                      AND substr(injection_id,19,1)='-'
                      AND substr(injection_id,20,1) GLOB '[89ab]'
                      AND substr(injection_id,24,1)='-'
                      AND length(replace(injection_id,'-',''))=32
                      AND replace(injection_id,'-','') NOT GLOB '*[^0-9a-f]*'),
                CHECK(length(op_id)=36
                      AND substr(op_id,9,1)='-'
                      AND substr(op_id,14,1)='-'
                      AND substr(op_id,15,1)='4'
                      AND substr(op_id,19,1)='-'
                      AND substr(op_id,20,1) GLOB '[89ab]'
                      AND substr(op_id,24,1)='-'
                      AND length(replace(op_id,'-',''))=32
                      AND replace(op_id,'-','') NOT GLOB '*[^0-9a-f]*'),
                CHECK(selected_session_id IS NULL OR
                      (length(selected_session_id)=36
                       AND substr(selected_session_id,9,1)='-'
                       AND substr(selected_session_id,14,1)='-'
                       AND substr(selected_session_id,15,1)='4'
                       AND substr(selected_session_id,19,1)='-'
                       AND substr(selected_session_id,20,1) GLOB '[89ab]'
                       AND substr(selected_session_id,24,1)='-'
                       AND length(replace(selected_session_id,'-',''))=32
                       AND replace(selected_session_id,'-','') NOT GLOB '*[^0-9a-f]*')),
                CHECK(length(nonce_sha256)=64 AND nonce_sha256 NOT GLOB '*[^0-9a-f]*'),
                CHECK(length(request_fingerprint)=64 AND request_fingerprint NOT GLOB '*[^0-9a-f]*'),
                CHECK(length(sender_incarnation_fingerprint)=64 AND sender_incarnation_fingerprint NOT GLOB '*[^0-9a-f]*'),
                CHECK(length(payload_sha256)=64 AND payload_sha256 NOT GLOB '*[^0-9a-f]*'),
                CHECK(confirmation_tag IS NULL OR (length(confirmation_tag)=64 AND confirmation_tag NOT GLOB '*[^0-9a-f]*')),
                CHECK((source_plane='host_cli' AND confirmation_tag IS NOT NULL AND injection_id=op_id)
                   OR (source_plane='container_api' AND confirmation_tag IS NULL)),
                CHECK((selected_session_id IS NULL) = (selected_backend IS NULL)),
                CHECK(selected_backend IS NULL OR selected_backend IN ('localProcess','containerTransport')),
                CHECK(length(issued_at)=24 AND issued_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9]Z'),
                CHECK(length(expires_at)=24 AND expires_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9]Z'),
                CHECK(length(queued_at)=24 AND queued_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9]Z'),
                CHECK(length(terminal_at)=24 AND terminal_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9]Z'),
                CHECK(actuating_at IS NULL OR (length(actuating_at)=24 AND actuating_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9]Z')),
                CHECK(queued_at>=issued_at AND queued_at<expires_at),
                CHECK(actuating_at IS NULL OR (actuating_at>=queued_at AND actuating_at<expires_at)),
                CHECK(terminal_at>=queued_at),
                CHECK((reason_code IS NULL) = (reason_detail IS NULL)),
                CHECK(reason_code IS NULL OR reason_code IN (
                    'invalid_envelope','mixed_payload','unsupported_version','invalid_enter_mode',
                    'invalid_id','invalid_nonce','invalid_timestamp','expired','invalid_target',
                    'invalid_text','payload_too_large','idempotency_conflict','capacity_exceeded',
                    'session_token_required','invalid_session_token','ambiguous_session_token',
                    'sender_session_not_live','sender_backend_not_local','sender_identity_invalid',
                    'sender_not_coordinator','root_identity_invalid','target_not_member',
                    'target_is_coordinator','target_out_of_scope','unsafe_path','api_scope_required',
                    'api_client_unbound','api_client_stale','api_binding_mismatch','authority_changed',
                    'busy','resize_unsettled','untracked_readiness','unsupported_session',
                    'nonpersistent_live_session','inconsistent_session','unsupported_profile',
                    'readiness_timeout','store_corrupt','restore_in_progress','purge_in_progress',
                    'session_race','lease_lost','spawn_failed_safe','store_transient',
                    'final_revalidation_failed','text_write_failed','required_enter_failed',
                    'daemon_restart_after_actuation','runtime_actuation_orphan','terminal_store_failed',
                    'redundant_enter_failed','boundary_metadata_failed','artifact_unclaimed'
                )),
                CHECK((status='injected' AND actuating_at IS NOT NULL
                       AND selected_session_id IS NOT NULL
                       AND (reason_code IS NULL OR reason_code IN (
                         'redundant_enter_failed','boundary_metadata_failed')))
                   OR (status='rejected' AND actuating_at IS NULL
                       AND selected_session_id IS NULL
                       AND reason_code IS NOT NULL AND reason_code NOT IN (
                         'final_revalidation_failed','text_write_failed','required_enter_failed',
                         'daemon_restart_after_actuation','runtime_actuation_orphan',
                         'terminal_store_failed','redundant_enter_failed',
                         'boundary_metadata_failed','artifact_unclaimed'))
                   OR (status='indeterminate' AND actuating_at IS NOT NULL
                       AND selected_session_id IS NOT NULL
                       AND reason_code IN (
                         'final_revalidation_failed','text_write_failed','required_enter_failed',
                         'daemon_restart_after_actuation','runtime_actuation_orphan',
                         'terminal_store_failed')))
            );

            INSERT INTO api_message_schema(version, applied_at)

            VALUES(2, '2026-08-30T00:00:01.000Z');
            "#,
        )
        .unwrap();
        conn
    }

    fn snapshot_table(
        conn: &rusqlite::Connection,
        table: &str,
    ) -> Vec<Vec<rusqlite::types::Value>> {
        let sql = format!("SELECT * FROM {table} ORDER BY injection_id");
        let mut statement = conn.prepare(&sql).unwrap();
        let column_count = statement.column_count();
        let rows = statement
            .query_map([], |row| {
                (0..column_count)
                    .map(|index| row.get(index))
                    .collect::<rusqlite::Result<Vec<_>>>()
            })
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        rows
    }

    #[test]
    fn test_schema_v3_migration_rebuilds_check_constraints() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join(DB_FILENAME);
        let conn = setup_v2_database(&path);
        conn.execute_batch(
            r#"
            INSERT INTO pty_input_operations(
                injection_id,sender_fqn,target_fqn,op_id,nonce_sha256,
                request_fingerprint,confirmation_tag,version,enter_mode,
                requested_agent_id,payload,payload_sha256,payload_bytes,source_plane,
                sender_incarnation_fingerprint,sender_identity_fingerprint,
                target_identity_fingerprint,authority_session_id,authority_client_id,
                authority_client_generation,status,attempt,next_attempt_at,lease_owner,
                lease_until,selected_session_id,selected_backend,issued_at,expires_at,
                queued_at,preparing_at,actuating_at,terminal_at,host_artifact_at,
                updated_at,reason_code,reason_detail
            ) VALUES
            (
                '00000000-0000-4000-8000-000000000001','proj:wg-1/s','proj:wg-1/t',
                '00000000-0000-4000-8000-000000000001',printf('%064x',101),
                printf('%064x',201),NULL,1,'agent-submit','agent-queued',X'717565756564',
                printf('%064x',301),6,'container_api',printf('%064x',401),
                printf('%064x',501),printf('%064x',601),
                '00000000-0000-4000-8000-000000000011','client-queued',
                '00000000-0000-4000-8000-000000000021','queued',0,
                '2099-08-30T00:00:02.000Z',NULL,NULL,NULL,NULL,
                '2099-08-30T00:00:00.000Z','2099-08-30T01:00:00.000Z',
                '2099-08-30T00:00:01.000Z',NULL,NULL,NULL,NULL,
                '2099-08-30T00:00:01.000Z',NULL,NULL
            ),
            (
                '00000000-0000-4000-8000-000000000002','proj:wg-1/s','proj:wg-1/t',
                '00000000-0000-4000-8000-000000000002',printf('%064x',102),
                printf('%064x',202),NULL,1,'agent-submit','agent-preparing',
                X'707265706172696e67',printf('%064x',302),9,'container_api',
                printf('%064x',402),printf('%064x',502),printf('%064x',602),
                '00000000-0000-4000-8000-000000000012','client-preparing',
                '00000000-0000-4000-8000-000000000022','preparing',2,
                '2099-08-30T00:00:02.000Z','lease-preparing',
                '2099-08-30T00:00:04.000Z',NULL,NULL,
                '2099-08-30T00:00:00.000Z','2099-08-30T01:00:00.000Z',
                '2099-08-30T00:00:01.000Z','2099-08-30T00:00:03.000Z',
                NULL,NULL,NULL,'2099-08-30T00:00:03.000Z',NULL,NULL
            ),
            (
                '00000000-0000-4000-8000-000000000003','proj:wg-1/s','proj:wg-1/t',
                '00000000-0000-4000-8000-000000000003',printf('%064x',103),
                printf('%064x',203),NULL,1,'agent-submit','agent-retry',X'7265747279',
                printf('%064x',303),5,'container_api',printf('%064x',403),
                printf('%064x',503),printf('%064x',603),
                '00000000-0000-4000-8000-000000000013','client-retry',
                '00000000-0000-4000-8000-000000000023','retry',3,
                '2099-08-30T00:00:06.000Z',NULL,NULL,NULL,NULL,
                '2099-08-30T00:00:00.000Z','2099-08-30T01:00:00.000Z',
                '2099-08-30T00:00:01.000Z','2099-08-30T00:00:02.000Z',
                NULL,NULL,NULL,'2099-08-30T00:00:04.000Z','store_transient',
                'A transient operation-store failure prevented actuation.'
            );

            INSERT INTO pty_input_tombstones(
                injection_id,sender_fqn,target_fqn,op_id,nonce_sha256,
                request_fingerprint,confirmation_tag,sender_incarnation_fingerprint,
                version,payload_sha256,payload_bytes,source_plane,selected_session_id,
                selected_backend,issued_at,expires_at,queued_at,actuating_at,
                terminal_at,status,reason_code,reason_detail
            ) VALUES
            (
                '00000000-0000-4000-8000-000000000004','proj:wg-1/s','proj:wg-1/t',
                '00000000-0000-4000-8000-000000000004',printf('%064x',104),
                printf('%064x',204),printf('%064x',704),printf('%064x',404),1,
                printf('%064x',304),8,'host_cli',
                '00000000-0000-4000-8000-000000000034','localProcess',
                '2099-08-30T00:00:00.000Z','2099-08-30T01:00:00.000Z',
                '2099-08-30T00:00:01.000Z','2099-08-30T00:00:03.000Z',
                '2099-08-30T00:00:05.000Z','injected','boundary_metadata_failed',
                'The terminal result was committed but boundary metadata failed.'
            ),
            (
                '00000000-0000-4000-8000-000000000005','proj:wg-1/s','proj:wg-1/t',
                '00000000-0000-4000-8000-000000000005',printf('%064x',105),
                printf('%064x',205),NULL,printf('%064x',405),1,printf('%064x',305),
                7,'container_api',NULL,NULL,'2099-08-30T00:00:00.000Z',
                '2099-08-30T01:00:00.000Z','2099-08-30T00:00:01.000Z',NULL,
                '2099-08-30T00:00:05.000Z','rejected','invalid_text',
                'The payload text contains prohibited control characters.'
            );
            "#,
        )
        .unwrap();

        let version: i64 = conn
            .query_row("SELECT MAX(version) FROM api_message_schema", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(version, 2);
        let v2_schema: String = conn
            .query_row(
                r#"SELECT group_concat(sql, char(10)) FROM sqlite_schema
                   WHERE type='table'
                     AND name IN ('pty_input_operations','pty_input_tombstones')"#,
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(!v2_schema.contains("menu_guard_blocked"));
        assert!(conn
            .execute(
                r#"UPDATE pty_input_operations
                   SET reason_code='menu_guard_blocked', reason_detail='blocked'
                   WHERE injection_id='00000000-0000-4000-8000-000000000003'"#,
                [],
            )
            .is_err());

        let expected_operations = snapshot_table(&conn, "pty_input_operations");
        let expected_tombstones = snapshot_table(&conn, "pty_input_tombstones");
        assert_eq!(expected_operations.len(), 3);
        assert_eq!(expected_tombstones.len(), 2);
        drop(conn);

        // Startup recovery would legitimately rewrite a preparing operation. Hold its
        // operation stripe so this test isolates the v2 -> v3 table-copy migration.
        let preparing_guard = try_stripe_lock_at(
            path.parent().unwrap(),
            "00000000-0000-4000-8000-000000000002",
            true,
        )
        .unwrap()
        .unwrap();
        let store = MessageStore::open(path).unwrap();
        drop(preparing_guard);

        let conn = store.conn.lock().unwrap();
        let version: i64 = conn
            .query_row("SELECT MAX(version) FROM api_message_schema", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(version, 3);
        let due_index: (String, String, String) = conn
            .query_row(
                r#"SELECT name,tbl_name,sql FROM sqlite_schema
                   WHERE type='index' AND name='idx_pty_input_due'"#,
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(due_index.0, "idx_pty_input_due");
        assert_eq!(due_index.1, "pty_input_operations");
        assert_eq!(
            due_index.2.split_whitespace().collect::<Vec<_>>().join(" "),
            "CREATE INDEX idx_pty_input_due ON \
             pty_input_operations(source_plane, status, next_attempt_at, lease_until)"
        );
        assert_eq!(
            snapshot_table(&conn, "pty_input_operations"),
            expected_operations
        );
        assert_eq!(
            snapshot_table(&conn, "pty_input_tombstones"),
            expected_tombstones
        );
        drop(conn);

        let id = Uuid::new_v4().to_string();
        let enqueued = store.enqueue_pty_input(pty_request(&id, "hello")).unwrap();
        assert_eq!(
            enqueued.result.status,
            crate::phone::types::PtyInputPublicStatus::Queued
        );
        store
            .claim_pty_input(
                crate::phone::types::PtyInputSourcePlane::ContainerApi,
                Some(&id),
                "lease-v3",
                Utc::now(),
            )
            .unwrap()
            .unwrap();
        store
            .retry_pty_input(
                &id,
                "lease-v3",
                crate::phone::types::PtyInputReasonCode::MenuGuardBlocked,
                Utc::now(),
            )
            .unwrap();
        let retry = store.query_pty_input_by_injection(&id).unwrap().unwrap();
        assert_eq!(
            retry.reason.unwrap().code,
            crate::phone::types::PtyInputReasonCode::MenuGuardBlocked
        );

        let terminal = store
            .terminalize_pty_input(
                &id,
                crate::phone::types::PtyInputPublicStatus::Rejected,
                Some(crate::phone::types::PtyInputReasonCode::MenuGuardBlocked),
                Utc::now(),
            )
            .unwrap();
        assert!(terminal.terminal);
        assert_eq!(
            terminal.reason.unwrap().code,
            crate::phone::types::PtyInputReasonCode::MenuGuardBlocked
        );
        let tombstone: (String, String) = store
            .conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT status,reason_code FROM pty_input_tombstones WHERE injection_id=?1",
                [&id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(tombstone, ("rejected".into(), "menu_guard_blocked".into()));
    }
}
