use super::{
    agents_instance_path, compose_settings_parts, empty_project_transaction, io_error_kind_as_str,
    json_error_category_as_str, parse_agents_instance_contents, project_paths_path,
    raw_project_source_from_bytes, repair_loaded_response_close_settings,
    repair_loaded_room_idle_confirmation_settings, select_project_source_from_raw, AppSettings,
    ProjectPathsWriteTransaction, SettingsFileLock, SettingsSaveCause, SettingsSaveError,
    SettingsSaveReason, SourceReadContext, CONFIGURATION_COMMAND_PAIRS,
    OVERLAY_DERIVED_ID_CLOSURES, OVERLAY_INELIGIBLE_DISK_KEYS, OVERLAY_INELIGIBLE_LEGACY_KEYS,
};
use crate::config::instance_artifacts::{
    SETTINGS_LOCAL_OVERRIDE_FILE_NAME, SETTINGS_OWNER_LOCK_FILE_NAME,
};
use crate::config::local_overlay::LocalSettingsOverlay;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::RwLock;

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

/// The lifecycle lease: an exclusive lock on the stable owner sidecar inside a
/// retained physical directory. Dropping it unlocks; the sidecar is never
/// unlinked.
pub(crate) struct SettingsOwnerLease {
    pub(crate) _lock: SettingsFileLock,
    pub(crate) retained: crate::path_identity::RetainedDirectory,
    pub(crate) settings_path: PathBuf,
}

#[cfg(all(test, windows))]
pub(crate) struct SettingsOwnerLeaseVerifyFault {
    pub(crate) path: PathBuf,
    pub(crate) armed: bool,
    pub(crate) hits: std::rc::Rc<std::cell::Cell<usize>>,
}

#[cfg(all(test, windows))]
thread_local! {
    pub(crate) static SETTINGS_OWNER_LEASE_VERIFY_FAULT: std::cell::RefCell<
        Option<SettingsOwnerLeaseVerifyFault>,
    > = const { std::cell::RefCell::new(None) };
}

#[cfg(all(test, windows))]
pub(crate) struct SettingsOwnerLeaseVerifyFaultGuard {
    pub(crate) path: PathBuf,
    pub(crate) hits: std::rc::Rc<std::cell::Cell<usize>>,
}

#[cfg(all(test, windows))]
impl SettingsOwnerLeaseVerifyFaultGuard {
    pub(crate) fn install(path: PathBuf) -> Self {
        let hits = std::rc::Rc::new(std::cell::Cell::new(0));
        SETTINGS_OWNER_LEASE_VERIFY_FAULT.with(|slot| {
            let mut slot = slot.borrow_mut();
            assert!(slot.is_none(), "lease verification faults cannot nest");
            *slot = Some(SettingsOwnerLeaseVerifyFault {
                path: path.clone(),
                armed: true,
                hits: std::rc::Rc::clone(&hits),
            });
        });
        Self { path, hits }
    }
}

#[cfg(all(test, windows))]
impl Drop for SettingsOwnerLeaseVerifyFaultGuard {
    fn drop(&mut self) {
        SETTINGS_OWNER_LEASE_VERIFY_FAULT.with(|slot| {
            let mut slot = slot.borrow_mut();
            if slot.as_ref().is_some_and(|fault| {
                fault.path == self.path && std::rc::Rc::ptr_eq(&fault.hits, &self.hits)
            }) {
                *slot = None;
            }
        });
    }
}

impl SettingsOwnerLease {
    pub(crate) fn acquire(
        settings_path: &Path,
        wait: std::time::Duration,
    ) -> Result<Self, SettingsOwnerError> {
        let identity = |reason: &str| SettingsOwnerError::Lifecycle(reason.to_string());
        let parent = settings_path
            .parent()
            .ok_or_else(|| identity("missing_parent_directory"))?;
        let leaf = settings_path
            .file_name()
            .ok_or_else(|| identity("missing_file_name"))?;
        // The chain check stops at the first link, so a `..` after a tolerated
        // link (macOS `/var`) would otherwise be resolved by canonicalize.
        if parent
            .components()
            .any(|component| component == std::path::Component::ParentDir)
        {
            return Err(identity("lock_identity_rejected"));
        }
        match crate::path_identity::verify_component_chain_reason(parent) {
            Ok(()) | Err(crate::path_identity::PathRule::ComponentIsLink) => {}
            Err(_) => return Err(identity("lock_identity_rejected")),
        }
        let physical_parent =
            std::fs::canonicalize(parent).map_err(|_| identity("lock_identity_rejected"))?;
        let retained = crate::path_identity::retain_directory(&physical_parent)
            .map_err(|_| identity("lock_identity_rejected"))?;
        let physical = retained.identity().canonical_path.join(leaf);
        let lock = SettingsFileLock::acquire_named(
            &physical,
            SETTINGS_OWNER_LOCK_FILE_NAME,
            wait,
            "settings_owner_lifecycle",
        )
        .map_err(|error| match error.cause {
            SettingsSaveCause::Semantic(SettingsSaveReason::LockTimedOut) => {
                SettingsOwnerError::LifecycleTimedOut
            }
            _ => SettingsOwnerError::Lifecycle(owner_save_error_text(&error)),
        })?;
        retained
            .verify_current()
            .map_err(|_| identity("lock_identity_rejected"))?;
        Ok(Self {
            _lock: lock,
            retained,
            settings_path: physical,
        })
    }

    pub(crate) fn verify(&self) -> Result<(), SettingsOwnerError> {
        #[cfg(all(test, windows))]
        if SETTINGS_OWNER_LEASE_VERIFY_FAULT.with(|slot| {
            let mut slot = slot.borrow_mut();
            let Some(fault) = slot.as_mut() else {
                return false;
            };
            if fault.armed && fault.path == self.settings_path {
                fault.armed = false;
                fault.hits.set(fault.hits.get() + 1);
                true
            } else {
                false
            }
        }) {
            return Err(SettingsOwnerError::Lifecycle(
                "lock_identity_rejected".to_string(),
            ));
        }
        self.retained
            .verify_current()
            .map_err(|_| SettingsOwnerError::Lifecycle("lock_identity_rejected".to_string()))
    }
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

/// One captured source: verified absence, or its bytes with the identity witness
/// of the object they were read from.
#[derive(Debug, Clone)]
pub(crate) enum CapturedSource {
    Absent,
    Present {
        bytes: Vec<u8>,
        witness: crate::path_identity::VerifiedPathIdentity,
    },
}

impl CapturedSource {
    pub(crate) fn bytes(&self) -> Option<&[u8]> {
        match self {
            Self::Absent => None,
            Self::Present { bytes, .. } => Some(bytes),
        }
    }

    /// A short, non-reversible witness label for DEBUG diagnostics.
    pub(crate) fn witness_label(&self) -> String {
        match self {
            Self::Absent => "absent".to_string(),
            Self::Present { witness, .. } => match witness.content_sha256 {
                Some(digest) => digest[..4]
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect(),
                None => format!("len{}", witness.metadata.len),
            },
        }
    }
}

/// The four settings sources, captured once under the transactional lock.
#[derive(Debug, Clone)]
pub(crate) struct CapturedSettingsSources {
    pub(crate) settings_path: PathBuf,
    pub(crate) base: CapturedSource,
    pub(crate) project_paths: CapturedSource,
    pub(crate) agents: CapturedSource,
    pub(crate) overlay: CapturedSource,
}

pub(crate) const SETTINGS_CAPTURE_LIMIT: usize = 16 * 1024 * 1024;

pub(crate) fn capture_source_file(
    path: &Path,
    counters: &SettingsOwnerCounters,
) -> Result<CapturedSource, SettingsOwnerError> {
    counters.source_reads.fetch_add(1, Ordering::Relaxed);
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    match std::fs::symlink_metadata(path) {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(CapturedSource::Absent);
        }
        Err(error) => {
            let kind = io_error_kind_as_str(error.kind());
            return Err(SettingsOwnerError::Capture(format!("{name}: {kind}")));
        }
    }
    // A source that vanishes after the metadata probe is a race, not absence.
    crate::path_identity::read_bounded_regular(path, SETTINGS_CAPTURE_LIMIT)
        .map(|(bytes, witness)| CapturedSource::Present { bytes, witness })
        .map_err(|reason| SettingsOwnerError::Capture(format!("{name}: {reason}")))
}

// Test seam: the next owner capture on this thread panics, so the panic is
// raised outside the builder while the operation's guards are held.
#[cfg(test)]
thread_local! {
    pub(crate) static OWNER_CAPTURE_PANIC: std::cell::Cell<bool> =
        const { std::cell::Cell::new(false) };
}

/// Captures base, project-paths, agents and overlay exactly once while the guard
/// holds the transactional lock. No directory creation, migration or write.
pub(crate) fn capture_sources_locked(
    guard: &SettingsOperationGuard<'_>,
) -> Result<CapturedSettingsSources, SettingsOwnerError> {
    guard.counters.captures.fetch_add(1, Ordering::Relaxed);
    #[cfg(test)]
    OWNER_CAPTURE_PANIC.with(|armed| {
        if armed.replace(false) {
            panic!("injected settings owner capture panic");
        }
    });
    let path = guard.settings_path();
    let captured = CapturedSettingsSources {
        settings_path: path.to_path_buf(),
        base: capture_source_file(path, guard.counters)?,
        project_paths: capture_source_file(&project_paths_path(path), guard.counters)?,
        agents: capture_source_file(&agents_instance_path(path), guard.counters)?,
        overlay: capture_source_file(
            &path.with_file_name(SETTINGS_LOCAL_OVERRIDE_FILE_NAME),
            guard.counters,
        )?,
    };
    guard.lease.verify()?;
    log::debug!(
        "[settings-owner] captured base={} project={} agents={} overlay={}",
        captured.base.witness_label(),
        captured.project_paths.witness_label(),
        captured.agents.witness_label(),
        captured.overlay.witness_label()
    );
    Ok(captured)
}

pub(crate) fn recover_composition(
    captured: &CapturedSettingsSources,
) -> Result<AppSettings, SettingsOwnerError> {
    compose_captured_sources(captured, CapturePolicy::Recovery)
}

/// Composes captured bytes through the same selector and parser core the loaders
/// use. No second read, no defaults for a present source that fails.
pub(crate) fn compose_captured_sources(
    captured: &CapturedSettingsSources,
    policy: CapturePolicy,
) -> Result<AppSettings, SettingsOwnerError> {
    let path = captured.settings_path.as_path();
    let destination_path = project_paths_path(path);
    let selection_error = |error: SettingsSaveError| {
        SettingsOwnerError::Compose(format!("selection: {}", owner_save_error_text(&error)))
    };
    let raw = |source: &CapturedSource, label: &Path, legacy: bool| {
        source
            .bytes()
            .map(|bytes| raw_project_source_from_bytes(bytes.to_vec(), label, label, legacy))
            .transpose()
    };
    let legacy = raw(&captured.base, path, true).map_err(selection_error)?;
    let destination = raw(&captured.project_paths, &destination_path, false);
    let destination = destination.map_err(selection_error)?;
    let selected = select_project_source_from_raw(
        legacy,
        destination,
        path,
        &destination_path,
        &destination_path,
    )
    .map_err(selection_error)?;
    let agents_label = agents_instance_path(path);
    let agents = match captured.agents.bytes() {
        None => Ok(None),
        Some(bytes) => std::str::from_utf8(bytes)
            .map_err(|_| format!("{} is not valid UTF-8", agents_label.display()))
            .and_then(|contents| parse_agents_instance_contents(contents, &agents_label))
            .map(Some),
    };
    if policy == CapturePolicy::Recovery && agents.is_err() {
        let reason = "agents source is invalid during recovery";
        return Err(SettingsOwnerError::Compose(reason.to_string()));
    }
    let source = path.to_string_lossy();
    // Only a verified-absent base composes over serialized defaults.
    let value = match &selected.legacy {
        Some(legacy) => Value::Object(legacy.object.clone()),
        None => serde_json::to_value(AppSettings::default())
            .map_err(|error| SettingsOwnerError::Compose(format!("defaults: {error}")))?,
    };
    let overlay_bytes = captured.overlay.bytes();
    let (mut settings, _legacy_profiles, agents_layer) = compose_settings_parts(
        value,
        &source,
        Some(path),
        Some(&selected.projection),
        Some(agents),
        |value| match overlay_bytes {
            Some(bytes) => LocalSettingsOverlay::from_overlay_bytes_paired(
                value,
                bytes,
                OVERLAY_INELIGIBLE_DISK_KEYS,
                OVERLAY_INELIGIBLE_LEGACY_KEYS,
                OVERLAY_DERIVED_ID_CLOSURES,
                CONFIGURATION_COMMAND_PAIRS,
            ),
            None => LocalSettingsOverlay::default(),
        },
        None,
    )
    .map_err(|_| SettingsOwnerError::Compose("base settings do not decode".to_string()))?;
    let overlay_rejected = settings.local_overlay_state.rejection().is_some();
    if policy == CapturePolicy::Recovery && overlay_rejected {
        let reason = "overlay rejected during recovery";
        return Err(SettingsOwnerError::Compose(reason.to_string()));
    }
    if let Some(state) = agents_layer {
        settings.agents_layer = state;
    }
    repair_loaded_response_close_settings(&mut settings, &source);
    repair_loaded_room_idle_confirmation_settings(&mut settings, &source);
    Ok(settings)
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
