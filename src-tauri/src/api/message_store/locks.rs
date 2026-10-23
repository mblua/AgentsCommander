use super::{
    MessageStore, MessageStoreError, PTY_INPUT_OPERATION_LOCK_STRIPES,
    PTY_INPUT_TARGET_LOCK_STRIPES,
};
use crate::config::instance_artifacts::PTY_INPUT_LOCKS_DIR_NAME;
use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

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

pub(super) struct TargetLockEntry {
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

pub(super) struct PtyInputTargetReservation {
    owner: Arc<PtyInputTargetLocks>,
    target: String,
    entry: Arc<TargetLockEntry>,
    transferred: bool,
}

pub(super) fn release_target_reservation(
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
    pub(super) fn entry_count(&self) -> usize {
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

    pub(super) fn start_with_interval(
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

pub(super) fn sqlite_sidecar_paths(path: &Path) -> [PathBuf; 3] {
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

pub(super) fn verify_existing_sqlite_files(path: &Path) -> Result<(), MessageStoreError> {
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

pub(super) fn try_stripe_lock_at(
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
