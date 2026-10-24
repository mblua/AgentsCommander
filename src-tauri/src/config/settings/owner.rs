use super::{
    empty_project_transaction, io_error_kind_as_str, json_error_category_as_str, AppSettings,
    ProjectPathsWriteTransaction, SettingsSaveCause, SettingsSaveError, SourceReadContext,
};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::RwLock;

mod capture;
mod lease;

#[cfg(test)]
pub(crate) use capture::OWNER_CAPTURE_PANIC;
pub(crate) use capture::{
    capture_source_file, capture_sources_locked, compose_captured_sources, recover_composition,
    CapturedSource, SETTINGS_CAPTURE_LIMIT,
};
pub(crate) use lease::SettingsOwnerLease;
#[cfg(all(test, windows))]
pub(crate) use lease::{SettingsOwnerLeaseVerifyFaultGuard, SETTINGS_OWNER_LEASE_VERIFY_FAULT};

// ---------------------------------------------------------------------------
// #2994 (E2992 P41) - the settings owner
// ---------------------------------------------------------------------------
//
// One authoritative `AppSettings` lives in the owner's `RwLock`. Until P50 the
// product runs the owner in `StagedCompatibility`: bootstrap, CLI and persistence
// keep their current paths through the forwarding methods below, and managed
// operations stay disabled. `Active` owners hold the lifecycle lease and run the
// complete operation contract; only tests construct them before P46.

/// Lifecycle availability of a settings owner. Owner metadata, changed only under
/// its control mutex. `Invalid` is sticky: it denies current reads, mutation and
/// authorization, and leaves only diagnostics and a validated close or restart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsAvailability {
    Starting,
    Ready,
    Invalid,
    Draining,
    Closed,
}

/// Errors of the owner contract. Messages carry stages and reason codes only,
/// never payloads, tokens or paths.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingsOwnerError {
    /// The owner is not `Ready`, or admission is sealed.
    Unavailable(SettingsAvailability),
    /// A try-acquisition found the state lock held.
    Busy,
    /// Managed operations are not enabled on this owner (StagedCompatibility).
    OperationsNotEnabled,
    /// The lifecycle lock is held by another owner past the wait deadline.
    LifecycleTimedOut,
    /// The lifecycle lock or its directory identity could not be established.
    /// Never evidence that the authority is free.
    Lifecycle(String),
    /// The transactional lock could not be taken for an operation.
    TransactionLock(String),
    /// A source could not be captured; IO, identity or malformation, never absence.
    Capture(String),
    /// The captured sources do not compose under the requested policy.
    Compose(String),
    /// The blocking worker could not be joined.
    Worker(String),
}

impl std::fmt::Display for SettingsOwnerError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unavailable(availability) => {
                write!(formatter, "settings owner unavailable ({availability:?})")
            }
            Self::Busy => formatter.write_str("settings owner state lock is busy"),
            Self::OperationsNotEnabled => {
                formatter.write_str("settings owner managed operations are not enabled")
            }
            Self::LifecycleTimedOut => {
                formatter.write_str("settings owner lifecycle lock is held by another owner")
            }
            Self::Lifecycle(reason) => write!(formatter, "settings owner lifecycle: {reason}"),
            Self::TransactionLock(reason) => {
                write!(formatter, "settings owner transaction lock: {reason}")
            }
            Self::Capture(reason) => write!(formatter, "settings owner capture: {reason}"),
            Self::Compose(reason) => write!(formatter, "settings owner composition: {reason}"),
            Self::Worker(reason) => write!(formatter, "settings owner worker: {reason}"),
        }
    }
}

impl std::error::Error for SettingsOwnerError {}

/// Stage and reason code of a settings save error, without its paths.
pub(crate) fn owner_save_error_text(error: &SettingsSaveError) -> String {
    let reason = match &error.cause {
        SettingsSaveCause::Io(source) => io_error_kind_as_str(source.kind()),
        SettingsSaveCause::Json(source) => json_error_category_as_str(source.classify()),
        SettingsSaveCause::Semantic(reason) => reason.as_str(),
    };
    format!("{error} ({:?}: {reason})", error.stage)
}

pub(crate) fn owner_transaction_error(error: SettingsSaveError) -> SettingsOwnerError {
    SettingsOwnerError::TransactionLock(owner_save_error_text(&error))
}

/// Identity the owner reports upward: no transport, no `AppHandle`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingsOwnerIdentity {
    pub instance_id: uuid::Uuid,
    pub epoch: u64,
    pub availability: SettingsAvailability,
    pub managed_operations_enabled: bool,
}

/// Project retention captured from RAM. A `source_error` means the lists are
/// unknown, never empty: no consumer may purge on it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingsRetention {
    pub active: Vec<String>,
    pub archived: Vec<String>,
    pub source_error: Option<String>,
}

/// Whether an operation may already have touched a persistent source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperationEffect {
    None,
    Possible,
}

/// How an operation ended. `PartialOrUnknown` is an honest error: never a
/// success and never a rollback.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperationOutcome {
    Committed,
    Rejected,
    PartialOrUnknown,
}

/// The publishers a locked operation may run. Each one flips the effect to
/// `Possible` before it starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsPublisher {
    PreflightBackup,
    ProjectPathsMigration,
    ProjectPathsBackup,
    ProjectPathsCleanup,
    BlockingMenusExport,
    AgentsExport,
    AgentsSave,
    SettingsSave,
    AuxCache,
    Stamp,
}

/// Composition policy. Startup keeps overlay and agents rejections as metadata;
/// recovery requires every affected source to be valid. Both keep legitimate
/// absence and neither turns IO or a base decode failure into defaults.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapturePolicy {
    Startup,
    Recovery,
}

/// The result an operation returns once the owner has adopted (or refused) the
/// effective reality.
#[derive(Debug)]
pub struct SettingsOperationReport<T> {
    pub outcome: OperationOutcome,
    pub value: Option<T>,
    pub error: Option<String>,
    pub epoch: u64,
    pub availability: SettingsAvailability,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SettingsOwnerStage {
    StagedCompatibility,
    Active,
}

#[derive(Debug)]
pub(crate) struct SettingsOwnerControl {
    pub(crate) availability: SettingsAvailability,
    pub(crate) epoch: u64,
    pub(crate) permits: usize,
    pub(crate) operations: usize,
    pub(crate) sealed: bool,
    pub(crate) managed_operations_enabled: bool,
}

/// Counters for tests and DEBUG diagnostics; no payloads.
#[derive(Debug, Default)]
pub(crate) struct SettingsOwnerCounters {
    pub(crate) transaction_lock_attempts: AtomicU64,
    pub(crate) captures: AtomicU64,
    pub(crate) source_reads: AtomicU64,
    pub(crate) operations: AtomicU64,
}

/// The single settings owner. `SettingsState` is `Arc<SettingsOwner>`.
pub struct SettingsOwner {
    pub(crate) settings: Arc<RwLock<AppSettings>>,
    pub(crate) stage: SettingsOwnerStage,
    pub(crate) instance_id: uuid::Uuid,
    pub(crate) control: std::sync::Mutex<SettingsOwnerControl>,
    pub(crate) released: std::sync::Condvar,
    pub(crate) lease: std::sync::Mutex<Option<SettingsOwnerLease>>,
    pub(crate) counters: SettingsOwnerCounters,
}

impl std::fmt::Debug for SettingsOwner {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SettingsOwner")
            .field("stage", &self.stage)
            .field("instance_id", &self.instance_id)
            .field("control", &*self.control())
            .finish_non_exhaustive()
    }
}

/// Admission of one read permit; releasing it wakes a draining owner.
pub(crate) struct SettingsPermitAdmission<'a> {
    pub(crate) owner: &'a SettingsOwner,
}

impl Drop for SettingsPermitAdmission<'_> {
    fn drop(&mut self) {
        self.owner.release_permit();
    }
}

/// Admission of one managed operation. It is released on every path, unwinding
/// included, which is what lets a drain terminate.
pub(crate) struct SettingsOperationAdmission<'a> {
    pub(crate) owner: &'a SettingsOwner,
}

impl Drop for SettingsOperationAdmission<'_> {
    fn drop(&mut self) {
        self.owner.finish_operation();
    }
}

/// Outcome, builder value and error of one classified operation.
pub(crate) type OperationParts<T> = (OperationOutcome, Option<T>, Option<String>);

/// A borrowed, validated read of the current settings.
pub struct SettingsReadGuard<'a> {
    pub(crate) guard: tokio::sync::RwLockReadGuard<'a, AppSettings>,
    pub(crate) _admission: SettingsPermitAdmission<'a>,
}

impl std::ops::Deref for SettingsReadGuard<'_> {
    type Target = AppSettings;

    fn deref(&self) -> &AppSettings {
        &self.guard
    }
}

/// An owned, validated read permit: the state guard plus the owner it belongs to.
/// A PTY route write keeps it until the write ends.
pub struct SettingsReadPermit {
    pub(crate) guard: Option<tokio::sync::OwnedRwLockReadGuard<AppSettings>>,
    pub(crate) owner: Arc<SettingsOwner>,
}

impl SettingsReadPermit {
    pub fn owner(&self) -> &Arc<SettingsOwner> {
        &self.owner
    }
}

impl std::ops::Deref for SettingsReadPermit {
    type Target = AppSettings;

    fn deref(&self) -> &AppSettings {
        self.guard
            .as_deref()
            .expect("a settings read permit holds its guard until drop")
    }
}

impl Drop for SettingsReadPermit {
    fn drop(&mut self) {
        drop(self.guard.take());
        self.owner.release_permit();
    }
}

/// Proof that an operation holds the owner's lifecycle authority and the current
/// transactional lock. Constructed only inside the owner; the locked halves take
/// it instead of reacquiring the transactional lock.
pub struct SettingsOperationGuard<'a> {
    pub(crate) lease: &'a SettingsOwnerLease,
    pub(crate) transaction: ProjectPathsWriteTransaction,
    pub(crate) counters: &'a SettingsOwnerCounters,
    pub(crate) effect: OperationEffect,
}

impl SettingsOperationGuard<'_> {
    /// The physical settings path under the retained directory.
    pub fn settings_path(&self) -> &Path {
        &self.transaction.path
    }

    pub fn effect(&self) -> OperationEffect {
        self.effect
    }

    pub(crate) fn verify_agent_file_context(&self, revision: &str) -> Result<(), String> {
        let captured = capture_sources_locked(self).map_err(|_| "sourceContextUnavailable")?;
        let fresh = compose_captured_sources(&captured, CapturePolicy::Recovery)
            .map_err(|_| "sourceContextUnavailable")?;
        let context = SourceReadContext::from_settings(&fresh, self.settings_path())
            .map_err(|_| "sourceContextUnavailable")?;
        if context.revision != revision {
            return Err("staleSourceContext".into());
        }
        Ok(())
    }

    /// Runs one publisher. The effect becomes `Possible` BEFORE the publisher
    /// starts, so a failure from here on classifies as `PartialOrUnknown`.
    pub fn publish<R>(
        &mut self,
        publisher: SettingsPublisher,
        run: impl FnOnce(&Path) -> Result<R, String>,
    ) -> Result<R, String> {
        self.effect = OperationEffect::Possible;
        log::debug!("[settings-owner] publisher start: {publisher:?}");
        let result = run(&self.transaction.path);
        log::debug!(
            "[settings-owner] publisher end: {publisher:?} ok={}",
            result.is_ok()
        );
        result
    }
}

impl SettingsOwner {
    /// StagedCompatibility owner around already-loaded settings: the P40
    /// constructor path. Ready, with managed operations disabled.
    pub(crate) fn staged(settings: AppSettings) -> Self {
        Self::with_stage(
            settings,
            SettingsOwnerStage::StagedCompatibility,
            SettingsAvailability::Ready,
            None,
        )
    }

    pub(crate) fn with_stage(
        settings: AppSettings,
        stage: SettingsOwnerStage,
        availability: SettingsAvailability,
        lease: Option<SettingsOwnerLease>,
    ) -> Self {
        Self {
            settings: Arc::new(RwLock::new(settings)),
            stage,
            instance_id: uuid::Uuid::new_v4(),
            control: std::sync::Mutex::new(SettingsOwnerControl {
                availability,
                epoch: 0,
                permits: 0,
                operations: 0,
                sealed: false,
                managed_operations_enabled: stage == SettingsOwnerStage::Active,
            }),
            released: std::sync::Condvar::new(),
            lease: std::sync::Mutex::new(lease),
            counters: SettingsOwnerCounters::default(),
        }
    }

    /// Opens an Active owner: lifecycle authority first, then the transactional
    /// lock, one capture and a Startup composition before `Ready`. Another owner
    /// holding the authority makes this wait up to `wait`, then fail.
    pub fn open_active(
        settings_path: &Path,
        wait: std::time::Duration,
    ) -> Result<Arc<Self>, SettingsOwnerError> {
        let started = std::time::Instant::now();
        let lease = SettingsOwnerLease::acquire(settings_path, wait)?;
        let mut owner = Self::with_stage(
            AppSettings::default(),
            SettingsOwnerStage::Active,
            SettingsAvailability::Starting,
            Some(lease),
        );
        let initial = {
            let lease = owner.lease_guard();
            let lease = lease.as_ref().ok_or(SettingsOwnerError::Unavailable(
                SettingsAvailability::Closed,
            ))?;
            let guard = owner.operation_guard(lease)?;
            let captured = capture_sources_locked(&guard)?;
            compose_captured_sources(&captured, CapturePolicy::Startup)?
        };
        // Not shared yet: the initial value replaces the placeholder without a lock.
        owner.settings = Arc::new(RwLock::new(initial));
        {
            let mut control = owner.control();
            control.availability = SettingsAvailability::Ready;
            control.epoch = 1;
        }
        log::debug!(
            "[settings-owner] {} ready in {:?}",
            owner.instance_id,
            started.elapsed()
        );
        Ok(Arc::new(owner))
    }

    pub(crate) fn control(&self) -> std::sync::MutexGuard<'_, SettingsOwnerControl> {
        self.control
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub(crate) fn lease_guard(&self) -> std::sync::MutexGuard<'_, Option<SettingsOwnerLease>> {
        self.lease
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn identity(&self) -> SettingsOwnerIdentity {
        let control = self.control();
        SettingsOwnerIdentity {
            instance_id: self.instance_id,
            epoch: control.epoch,
            availability: control.availability,
            managed_operations_enabled: control.managed_operations_enabled,
        }
    }

    pub fn availability(&self) -> SettingsAvailability {
        self.control().availability
    }

    pub(crate) fn admit_permit(&self) -> Result<SettingsPermitAdmission<'_>, SettingsOwnerError> {
        let mut control = self.control();
        if control.sealed || control.availability != SettingsAvailability::Ready {
            return Err(SettingsOwnerError::Unavailable(control.availability));
        }
        control.permits += 1;
        Ok(SettingsPermitAdmission { owner: self })
    }

    pub(crate) fn release_permit(&self) {
        let mut control = self.control();
        control.permits = control.permits.saturating_sub(1);
        if control.permits == 0 {
            self.released.notify_all();
        }
    }

    pub(crate) fn revalidate(&self) -> Result<(), SettingsOwnerError> {
        let control = self.control();
        if control.availability == SettingsAvailability::Ready {
            Ok(())
        } else {
            Err(SettingsOwnerError::Unavailable(control.availability))
        }
    }

    pub(crate) fn mark_invalid(&self, stage: &str) {
        let mut control = self.control();
        if !matches!(
            control.availability,
            SettingsAvailability::Draining | SettingsAvailability::Closed
        ) {
            control.availability = SettingsAvailability::Invalid;
        }
        let epoch = control.epoch;
        log::debug!(
            "[settings-owner] {} invalid at {stage} epoch={epoch}",
            self.instance_id
        );
    }

    /// A borrowed read validated against availability on admission and again
    /// once the state lock is held.
    pub async fn current_read(&self) -> Result<SettingsReadGuard<'_>, SettingsOwnerError> {
        let admission = self.admit_permit()?;
        let guard = self.settings.read().await;
        self.revalidate()?;
        Ok(SettingsReadGuard {
            guard,
            _admission: admission,
        })
    }

    pub fn try_current_read(&self) -> Result<SettingsReadGuard<'_>, SettingsOwnerError> {
        let admission = self.admit_permit()?;
        let guard = self
            .settings
            .try_read()
            .map_err(|_| SettingsOwnerError::Busy)?;
        self.revalidate()?;
        Ok(SettingsReadGuard {
            guard,
            _admission: admission,
        })
    }

    pub fn blocking_current_read(&self) -> Result<SettingsReadGuard<'_>, SettingsOwnerError> {
        let admission = self.admit_permit()?;
        let guard = self.settings.blocking_read();
        self.revalidate()?;
        Ok(SettingsReadGuard {
            guard,
            _admission: admission,
        })
    }

    /// An owned permit holding the state guard and this owner.
    pub fn try_current_read_owned(
        self: &Arc<Self>,
    ) -> Result<SettingsReadPermit, SettingsOwnerError> {
        let admission = self.admit_permit()?;
        let guard = Arc::clone(&self.settings)
            .try_read_owned()
            .map_err(|_| SettingsOwnerError::Busy)?;
        // The permit now owns the admission and releases it on drop.
        std::mem::forget(admission);
        let permit = SettingsReadPermit {
            guard: Some(guard),
            owner: Arc::clone(self),
        };
        self.revalidate()?;
        Ok(permit)
    }

    /// Clones RAM through a validated guard: no filesystem, no transactional lock.
    pub async fn current_snapshot(&self) -> Result<AppSettings, SettingsOwnerError> {
        self.current_read().await.map(|guard| guard.clone())
    }

    pub fn blocking_current_snapshot(&self) -> Result<AppSettings, SettingsOwnerError> {
        self.blocking_current_read().map(|guard| guard.clone())
    }

    pub(crate) fn retention_of(settings: &AppSettings) -> SettingsRetention {
        SettingsRetention {
            active: settings.project_paths.clone(),
            archived: settings.archived_project_paths.clone(),
            source_error: settings.project_paths_persistence_error.clone(),
        }
    }

    /// Retention from RAM, captured before any sessions lock is taken.
    pub async fn current_retention(&self) -> Result<SettingsRetention, SettingsOwnerError> {
        self.current_read()
            .await
            .map(|guard| Self::retention_of(&guard))
    }

    pub fn blocking_current_retention(&self) -> Result<SettingsRetention, SettingsOwnerError> {
        self.blocking_current_read()
            .map(|guard| Self::retention_of(&guard))
    }

    pub(crate) fn operation_guard<'a>(
        &'a self,
        lease: &'a SettingsOwnerLease,
    ) -> Result<SettingsOperationGuard<'a>, SettingsOwnerError> {
        lease.verify()?;
        let attempts = &self.counters.transaction_lock_attempts;
        attempts.fetch_add(1, Ordering::Relaxed);
        let operation = "settings_owner_operation";
        let transaction = empty_project_transaction(&lease.settings_path, operation);
        let transaction = transaction.map_err(owner_transaction_error)?;
        Ok(SettingsOperationGuard {
            lease,
            transaction,
            counters: &self.counters,
            effect: OperationEffect::None,
        })
    }

    pub(crate) fn admit_operation(
        &self,
    ) -> Result<SettingsOperationAdmission<'_>, SettingsOwnerError> {
        let mut control = self.control();
        if !control.managed_operations_enabled {
            return Err(SettingsOwnerError::OperationsNotEnabled);
        }
        if control.sealed || control.availability != SettingsAvailability::Ready {
            return Err(SettingsOwnerError::Unavailable(control.availability));
        }
        control.operations += 1;
        Ok(SettingsOperationAdmission { owner: self })
    }

    pub(crate) fn finish_operation(&self) {
        let mut control = self.control();
        control.operations = control.operations.saturating_sub(1);
        if control.operations == 0 {
            self.released.notify_all();
        }
    }

    /// Runs one complete synchronous operation: admission, state guard, current
    /// transactional lock, builder over the captured current settings, publishers,
    /// adoption of the effective reality, then release. Host effects belong to
    /// the caller, after this returns. Never nests a runtime.
    ///
    /// The admission is released on every path, panics included, so a drain
    /// always terminates. A panic before the guards are held published nothing
    /// and is a worker error; a panic while they are held is classified under
    /// them (see `run_operation`).
    pub fn with_operation_blocking<T>(
        &self,
        name: &'static str,
        builder: impl FnOnce(&AppSettings, &mut SettingsOperationGuard<'_>) -> Result<T, String>,
    ) -> Result<SettingsOperationReport<T>, SettingsOwnerError> {
        let _admission = self.admit_operation()?;
        let run = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.run_operation(name, builder)
        }));
        match run {
            Ok(result) => result,
            Err(_) => {
                let reason = "operation panicked before its guards were held";
                Err(SettingsOwnerError::Worker(reason.to_string()))
            }
        }
    }

    /// The async form: the same body on a blocking worker that retains its
    /// owner handle. The future must not be awaited while holding any guard.
    pub async fn with_operation<T, F>(
        self: &Arc<Self>,
        name: &'static str,
        builder: F,
    ) -> Result<SettingsOperationReport<T>, SettingsOwnerError>
    where
        T: Send + 'static,
        F: FnOnce(&AppSettings, &mut SettingsOperationGuard<'_>) -> Result<T, String>
            + Send
            + 'static,
    {
        let owner = Arc::clone(self);
        tokio::task::spawn_blocking(move || owner.with_operation_blocking(name, builder))
            .await
            .map_err(|error| SettingsOwnerError::Worker(error.to_string()))?
    }

    pub(crate) fn run_operation<T>(
        &self,
        name: &'static str,
        builder: impl FnOnce(&AppSettings, &mut SettingsOperationGuard<'_>) -> Result<T, String>,
    ) -> Result<SettingsOperationReport<T>, SettingsOwnerError> {
        let operation = self.counters.operations.fetch_add(1, Ordering::Relaxed) + 1;
        let started = std::time::Instant::now();
        let mut state = self.settings.blocking_write();
        let lease = self.lease_guard();
        let lease = lease.as_ref().ok_or(SettingsOwnerError::Unavailable(
            SettingsAvailability::Closed,
        ))?;
        let mut guard = self.operation_guard(lease)?;
        // Everything that reads sources, runs the builder or adopts reality runs
        // under this catch, so a panic anywhere is classified while the state
        // guard, the lease and the transactional lock are all still held.
        let body = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.operation_body(&mut state, &mut guard, builder)
        }));
        let effect = guard.effect;
        let (outcome, value, error) = match body {
            Ok(Ok(parts)) => parts,
            Ok(Err(error)) => return Err(error),
            Err(_) if effect == OperationEffect::None => {
                // No publisher ran and RAM was not replaced: nothing to recover.
                let reason = "operation panicked before any publisher";
                (OperationOutcome::Rejected, None, Some(reason.to_string()))
            }
            Err(_) => {
                // A source may have changed and reality could not be adopted.
                self.mark_invalid("panic");
                let reason = "operation panicked after a publisher";
                (
                    OperationOutcome::PartialOrUnknown,
                    None,
                    Some(reason.to_string()),
                )
            }
        };
        drop(guard);
        drop(state);
        let control = self.control();
        log::debug!(
            "[settings-owner] {} op={operation} {name} {outcome:?}/{effect:?} epoch={} in {:?}",
            self.instance_id,
            control.epoch,
            started.elapsed()
        );
        Ok(SettingsOperationReport {
            outcome,
            value,
            error,
            epoch: control.epoch,
            availability: control.availability,
        })
    }

    /// Capture, builder, classification and adoption, all under the caller's
    /// guards. Admission errors return before any publisher can run.
    pub(crate) fn operation_body<T>(
        &self,
        state: &mut AppSettings,
        guard: &mut SettingsOperationGuard<'_>,
        builder: impl FnOnce(&AppSettings, &mut SettingsOperationGuard<'_>) -> Result<T, String>,
    ) -> Result<OperationParts<T>, SettingsOwnerError> {
        // Sources that do not compose at admission reject the operation before
        // any publisher; RAM keeps the last adopted value.
        let captured = capture_sources_locked(guard)?;
        let current = compose_captured_sources(&captured, CapturePolicy::Recovery)?;
        let built = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            builder(&current, &mut *guard)
        }));
        let effect = guard.effect;
        let (value, error) = match built {
            Ok(Ok(value)) => (Some(value), None),
            Ok(Err(error)) => (None, Some(error)),
            Err(_) => (None, Some("operation panicked".to_string())),
        };
        let outcome = if error.is_none() {
            OperationOutcome::Committed
        } else if effect == OperationEffect::None {
            OperationOutcome::Rejected
        } else {
            OperationOutcome::PartialOrUnknown
        };
        // Rejected changed nothing: keep RAM. A success that published nothing
        // adopts the composition it already captured, without a second capture.
        // Otherwise recapture under the same guard and adopt reality, or become
        // Invalid without saving defaults.
        let outcome = if outcome == OperationOutcome::Rejected {
            outcome
        } else if effect == OperationEffect::None {
            *state = current;
            self.control().epoch += 1;
            outcome
        } else {
            let recaptured = capture_sources_locked(guard);
            match recaptured.and_then(|captured| recover_composition(&captured)) {
                Ok(effective) => {
                    *state = effective;
                    self.control().epoch += 1;
                    outcome
                }
                Err(_) => {
                    self.mark_invalid("recapture");
                    OperationOutcome::PartialOrUnknown
                }
            }
        };
        Ok((outcome, value, error))
    }

    /// Seals admission, waits for every permit and operation, then releases the
    /// lifecycle authority LAST. The soft deadline only diagnoses; it never
    /// detaches or unlocks early.
    pub fn drain_blocking(&self, soft_deadline: std::time::Duration) {
        let started = std::time::Instant::now();
        let mut control = self.control();
        control.sealed = true;
        if control.availability != SettingsAvailability::Closed {
            control.availability = SettingsAvailability::Draining;
        }
        let mut reported = false;
        while control.permits > 0 || control.operations > 0 {
            let (next, timeout) = self
                .released
                .wait_timeout(control, std::time::Duration::from_millis(50))
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            control = next;
            if timeout.timed_out() && !reported && started.elapsed() >= soft_deadline {
                reported = true;
                log::debug!(
                    "[settings-owner] {} drain past soft deadline permits={} operations={}",
                    self.instance_id,
                    control.permits,
                    control.operations
                );
            }
        }
        control.availability = SettingsAvailability::Closed;
        drop(control);
        let lease = self.lease_guard().take();
        drop(lease);
        log::debug!(
            "[settings-owner] {} closed in {:?}",
            self.instance_id,
            started.elapsed()
        );
    }

    pub(crate) fn staged_forwarding(&self) {
        debug_assert_eq!(
            self.stage,
            SettingsOwnerStage::StagedCompatibility,
            "lock forwarding is StagedCompatibility only"
        );
    }

    // StagedCompatibility forwarding: the pre-owner RwLock surface, kept only so
    // consumers not yet migrated compile and behave exactly as before. P50
    // retires it from the product.

    pub async fn read(&self) -> tokio::sync::RwLockReadGuard<'_, AppSettings> {
        self.staged_forwarding();
        self.settings.read().await
    }

    pub async fn write(&self) -> tokio::sync::RwLockWriteGuard<'_, AppSettings> {
        self.staged_forwarding();
        self.settings.write().await
    }

    pub fn try_read(
        &self,
    ) -> Result<tokio::sync::RwLockReadGuard<'_, AppSettings>, tokio::sync::TryLockError> {
        self.staged_forwarding();
        self.settings.try_read()
    }

    pub fn try_write(
        &self,
    ) -> Result<tokio::sync::RwLockWriteGuard<'_, AppSettings>, tokio::sync::TryLockError> {
        self.staged_forwarding();
        self.settings.try_write()
    }

    pub fn blocking_read(&self) -> tokio::sync::RwLockReadGuard<'_, AppSettings> {
        self.staged_forwarding();
        self.settings.blocking_read()
    }

    pub fn blocking_write(&self) -> tokio::sync::RwLockWriteGuard<'_, AppSettings> {
        self.staged_forwarding();
        self.settings.blocking_write()
    }

    pub async fn read_owned(self: Arc<Self>) -> tokio::sync::OwnedRwLockReadGuard<AppSettings> {
        self.staged_forwarding();
        Arc::clone(&self.settings).read_owned().await
    }

    pub async fn write_owned(self: Arc<Self>) -> tokio::sync::OwnedRwLockWriteGuard<AppSettings> {
        self.staged_forwarding();
        Arc::clone(&self.settings).write_owned().await
    }

    pub fn try_read_owned(
        self: Arc<Self>,
    ) -> Result<tokio::sync::OwnedRwLockReadGuard<AppSettings>, tokio::sync::TryLockError> {
        self.staged_forwarding();
        Arc::clone(&self.settings).try_read_owned()
    }
}

pub type SettingsState = Arc<SettingsOwner>;

pub fn new_settings_state(settings: AppSettings) -> SettingsState {
    Arc::new(SettingsOwner::staged(settings))
}
