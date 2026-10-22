//! Activated project seed-manifest recorder.
//!
//! [`ManifestActivationToken::production`] is the sole production activation
//! constructor for supported publications and lifecycle cleanup. The historical
//! names in [`V1_COVERAGE_BOUNDARIES`] remain a stable compatibility vocabulary;
//! they are not a list of current publishers and do not imply recorder wiring.
//!
//! #1318 - coverage grew to v2 (`coding_agent_catalog` kind): an existing v1
//! manifest is upgraded IN PLACE on the first writer/reader acquire (see
//! [`ProjectSeedManifestGuard::try_upgrade_v1_to_v2`]). Supported rows and their
//! timestamps are preserved on disk, while legacy replica-config rows remain in
//! the held state but are omitted from canonical output. A v2 manifest written by
//! this build is NOT readable by an old build (writer strictness is a one-way
//! door; an old build preserves its bytes exactly).

#![allow(dead_code)]

mod guard;
mod handles;
mod lifecycle;
mod path_identity;
mod publish;
mod state;
mod windows;
mod wire;

#[cfg(test)]
use chrono::{DateTime, Utc};
pub(crate) use guard::ProjectSeedManifestGuard;
#[cfg(test)]
use guard::TestFilesystemHooks;
use handles::{
    classify_lock_open_error, classify_open_error, is_exact_not_found, CanonicalSnapshot,
    OpenedRegularFile,
};
#[cfg(unix)]
use handles::{handle_facts, open_regular_no_follow};
#[cfg(windows)]
use handles::{handle_facts, open_regular_no_follow};
#[cfg(not(any(unix, windows)))]
use handles::{handle_facts, open_regular_no_follow};
pub(crate) use handles::{PinnedDirectory, PinnedOwnerChain};
use lifecycle::ProjectPublicationPermitState;
pub(crate) use lifecycle::{
    acquire_project_gate_soft, run_blocking_owned, ManifestActivationToken,
    ManifestLifecycleFilter, ProjectPublicationPermit, SoftProjectGate,
};
#[cfg(test)]
pub(crate) use lifecycle::{
    BlockingTaskError, PublishedScopeBatch, V1CoverageBoundary, V1_COVERAGE_BOUNDARIES,
};
pub(crate) use path_identity::ManifestPathIdentity;
#[cfg(test)]
use path_identity::{encode_lower_hex_bytes, encode_lower_hex_u16};
use path_identity::{parse_utf8_components, DecodedPathComponents, ManifestPathKey};
#[cfg(not(windows))]
use publish::cleanup_current_owned_temp;
#[cfg(windows)]
use publish::native_name_has_temp_shape;
#[cfg(not(any(unix, windows)))]
use publish::native_name_has_temp_shape;
use publish::{
    acquire_kernel_lock, is_exact_temp_name, open_or_create_lock_file, stream_matches,
    verify_temp_path,
};
#[cfg(unix)]
use publish::{
    atomic_publish_manifest, native_name_has_temp_shape, remove_prior_unix_temp_if_safe,
};
#[cfg(test)]
use state::{checked_add, parse_config_scope};
use state::{
    classify_toml_parse_error, parse_manifest_bytes, read_bounded_file,
    sanitize_canonical_validation_error, serialize_state, ManifestState,
};
pub(crate) use state::{
    config_batch_base_serialized_len, exact_config_row_serialized_len, has_catalog_publication,
    PublishedManifestRow,
};
#[cfg(test)]
use state::{exact_serialized_len, exact_serialized_len_with_limit};
#[cfg(test)]
use std::fs::{File, OpenOptions};
#[cfg(test)]
use std::io::Write;
#[cfg(test)]
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;
#[cfg(test)]
use std::time::Instant;
use std::{fmt, io};
use thiserror::Error;
#[cfg(test)]
use uuid::Uuid;
#[cfg(all(test, windows))]
use windows::absolute_verbatim_utf16;
#[cfg(not(any(unix, windows)))]
use windows::atomic_publish_manifest;
#[cfg(windows)]
use windows::cleanup_current_owned_temp;
pub(crate) use wire::{ManifestFileKind, ManifestPathEncoding, ManifestSource};
use wire::{SeedManifestRowWire, SeedManifestWire};

pub(crate) const SEED_MANIFEST_FILENAME: &str = "seed-manifest.toml";
pub(crate) const SEED_MANIFEST_LOCK_FILENAME: &str = ".seed-manifest.lock";
const SEED_MANIFEST_TEMP_PREFIX: &str = ".seed-manifest.";
const SEED_MANIFEST_TEMP_SUFFIX: &str = ".tmp";
const MANAGED_HEADER: &str =
    "# Managed by AgentsCommander. Diagnostic only; never grants file ownership.\n";
const SCHEMA_VERSION: u32 = 1;
const COVERAGE_VERSION: u32 = 2;
const COVERAGE: [&str; 3] = [
    "project_context_templates",
    "replica_config_folders",
    "coding_agent_catalog",
];
pub(crate) const MAX_MANIFEST_BYTES: u64 = 128 * 1024 * 1024;
pub(crate) const MAX_MANIFEST_ROWS: usize = 250_000;
pub(crate) const MAX_FIELD_BYTES: usize = 256 * 1024;
const RAW_COMPARE_BUFFER_BYTES: usize = 64 * 1024;
const DEFAULT_LOCK_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_LOCK_POLL: Duration = Duration::from_millis(50);
const MAX_TEMP_INVENTORY_ENTRIES: usize = 4_096;
const MAX_TEMP_DIAGNOSTIC_SAMPLES: usize = 32;

#[cfg(windows)]
const WINDOWS_NAMESPACE_POWER_LOSS_DURABILITY_CLAIMED: bool = false;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FileIdentity {
    volume: u64,
    file: u128,
}

#[cfg(windows)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WindowsNamespaceOperation {
    MoveFileEx,
    ReplaceFile,
}

#[cfg(windows)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WindowsReplacePartial {
    CanonicalRemovedReplacementAtTemp,
    ReplacementEnrichedOldDestinationRenamed,
}

#[cfg(windows)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum WindowsPathState {
    NotFound,
    Same {
        identity: FileIdentity,
        links: u64,
        length: u64,
        bytes_match: bool,
    },
    Different {
        identity: FileIdentity,
        links: u64,
        length: u64,
        bytes_match: bool,
    },
    Unsafe {
        reason: String,
    },
    InspectionError {
        raw_error: Option<u32>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ResourceBoundKind {
    InputBytes,
    Rows,
    PathBytes,
    ScopeBytes,
    OutputBytes,
    ArithmeticOverflow,
}

impl fmt::Display for ResourceBoundKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let label = match self {
            Self::InputBytes => "input bytes",
            Self::Rows => "rows",
            Self::PathBytes => "path bytes",
            Self::ScopeBytes => "scope bytes",
            Self::OutputBytes => "output bytes",
            Self::ArithmeticOverflow => "checked arithmetic",
        };
        formatter.write_str(label)
    }
}

#[derive(Debug, Error)]
pub(crate) enum SeedManifestError {
    #[error("unsafe seed-manifest path {path}: {reason}")]
    UnsafePath { path: PathBuf, reason: String },
    #[error("seed-manifest lock remained busy for {waited:?}: {path}")]
    BusyTimeout { path: PathBuf, waited: Duration },
    #[error(
        "seed-manifest lock I/O failure at {path} after contention={saw_contention}: {source}"
    )]
    LockIo {
        path: PathBuf,
        saw_contention: bool,
        #[source]
        source: io::Error,
    },
    #[error(
        "seed-manifest {kind} bound exceeded: limit={limit}, observed_at_least={observed_at_least}"
    )]
    ResourceBound {
        kind: ResourceBoundKind,
        limit: u64,
        observed_at_least: u64,
    },
    #[error("seed-manifest bounded read failed at {path}: {source}")]
    BoundedRead {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("seed-manifest TOML parse failed: {0}")]
    Parse(String),
    #[error("seed-manifest validation failed: {0}")]
    Validation(String),
    #[error("seed-manifest schema version {found} is newer than supported version {supported}")]
    FutureSchema { found: u32, supported: u32 },
    #[error("seed-manifest changed outside the held project lock: {path}")]
    ExternalEditConflict { path: PathBuf },
    #[error("seed-manifest canonical is read-only and was preserved: {path}")]
    ReadOnlyCanonical { path: PathBuf },
    #[error("seed-manifest temporary file operation '{operation}' failed at {path}: {source}")]
    TempFile {
        operation: &'static str,
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("seed-manifest atomic {operation} failed from {temp} to {canonical}: {source}")]
    AtomicReplace {
        operation: &'static str,
        temp: PathBuf,
        canonical: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("seed-manifest serialization failed: {0}")]
    Serialize(String),
    #[error("seed-manifest serialized-size mismatch: counted={counted}, actual={actual}")]
    SerializedSizeMismatch { counted: usize, actual: usize },
    #[error("seed-manifest I/O operation '{operation}' failed at {path}: {source}")]
    Io {
        operation: &'static str,
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[cfg(windows)]
    #[error(
        "Windows seed-manifest namespace call {operation:?} failed before publication \
            raw_error={raw_error} (0x{raw_error:08x}) canonical={canonical} temp={temp} \
            canonical_state={canonical_state:?} temp_state={temp_state:?}"
    )]
    WindowsNamespaceFailure {
        operation: WindowsNamespaceOperation,
        raw_error: u32,
        canonical: PathBuf,
        temp: PathBuf,
        canonical_state: Box<WindowsPathState>,
        temp_state: Box<WindowsPathState>,
    },
    #[cfg(windows)]
    #[error(
        "Windows seed-manifest replacement partially changed the namespace raw_error={raw_error} \
            (0x{raw_error:08x}) partial={partial:?} canonical={canonical} temp={temp} \
            canonical_state={canonical_state:?} temp_state={temp_state:?}"
    )]
    WindowsReplacePartial {
        raw_error: u32,
        partial: WindowsReplacePartial,
        canonical: PathBuf,
        temp: PathBuf,
        canonical_state: Box<WindowsPathState>,
        temp_state: Box<WindowsPathState>,
    },
    #[cfg(windows)]
    #[error(
        "Windows seed-manifest replacement requires explicit recovery raw_error={raw_error} \
            (0x{raw_error:08x}) canonical={canonical} temp={temp} \
            canonical_state={canonical_state:?} temp_state={temp_state:?}"
    )]
    WindowsReplaceRecoveryRequired {
        raw_error: u32,
        canonical: PathBuf,
        temp: PathBuf,
        canonical_state: Box<WindowsPathState>,
        temp_state: Box<WindowsPathState>,
    },
    #[cfg(windows)]
    #[error(
        "Windows seed-manifest create-only move requires explicit recovery raw_error={raw_error} \
            (0x{raw_error:08x}) canonical={canonical} temp={temp} \
            canonical_state={canonical_state:?} temp_state={temp_state:?}"
    )]
    WindowsMoveRecoveryRequired {
        raw_error: u32,
        canonical: PathBuf,
        temp: PathBuf,
        canonical_state: Box<WindowsPathState>,
        temp_state: Box<WindowsPathState>,
    },
    #[cfg(windows)]
    #[error(
        "Windows seed-manifest namespace call succeeded but final identity validation failed \
            canonical={canonical} temp={temp} expected_source={expected_source:?} \
            final_state={final_state:?} old_destination_identity={old_destination_identity:?}"
    )]
    WindowsPostPublishIdentityConflict {
        canonical: PathBuf,
        temp: PathBuf,
        expected_source: FileIdentity,
        final_state: Box<WindowsPathState>,
        old_destination_identity: Box<Option<FileIdentity>>,
    },
}

impl SeedManifestError {
    fn resource_bound(kind: ResourceBoundKind, limit: usize, observed_at_least: usize) -> Self {
        Self::ResourceBound {
            kind,
            limit: u64::try_from(limit).unwrap_or(u64::MAX),
            observed_at_least: u64::try_from(observed_at_least).unwrap_or(u64::MAX),
        }
    }

    fn degraded_reason(&self) -> ManifestDegradedReason {
        match self {
            Self::BusyTimeout { .. } => ManifestDegradedReason::Busy,
            Self::LockIo { .. } => ManifestDegradedReason::LockUnavailable,
            Self::ResourceBound { kind, .. } => ManifestDegradedReason::ResourceBound(*kind),
            Self::FutureSchema { .. } => ManifestDegradedReason::FutureSchema,
            Self::Parse(_) | Self::Validation(_) | Self::BoundedRead { .. } => {
                ManifestDegradedReason::InvalidCanonical
            }
            Self::ExternalEditConflict { .. } => ManifestDegradedReason::ExternalEdit,
            Self::ReadOnlyCanonical { .. } => ManifestDegradedReason::ReadOnlyCanonical,
            Self::UnsafePath { .. } => ManifestDegradedReason::UnsafePath,
            Self::TempFile { .. }
            | Self::AtomicReplace { .. }
            | Self::Serialize(_)
            | Self::SerializedSizeMismatch { .. }
            | Self::Io { .. } => ManifestDegradedReason::PersistenceFailure,
            #[cfg(windows)]
            Self::WindowsNamespaceFailure { .. } => ManifestDegradedReason::PersistenceFailure,
            #[cfg(windows)]
            Self::WindowsReplacePartial { .. }
            | Self::WindowsReplaceRecoveryRequired { .. }
            | Self::WindowsMoveRecoveryRequired { .. } => ManifestDegradedReason::RecoveryRequired,
            #[cfg(windows)]
            Self::WindowsPostPublishIdentityConflict { .. } => {
                ManifestDegradedReason::IdentityConflict
            }
        }
    }

    fn diagnostic_kind(&self) -> &'static str {
        match self {
            Self::UnsafePath { .. } => "unsafe_path",
            Self::BusyTimeout { .. } => "busy_timeout",
            Self::LockIo { .. } => "lock_io",
            Self::ResourceBound { .. } => "resource_bound",
            Self::BoundedRead { .. } => "bounded_read",
            Self::Parse(_) => "parse",
            Self::Validation(_) => "validation",
            Self::FutureSchema { .. } => "future_schema",
            Self::ExternalEditConflict { .. } => "external_edit_conflict",
            Self::ReadOnlyCanonical { .. } => "read_only_canonical",
            Self::TempFile { .. } => "temp_file",
            Self::AtomicReplace { .. } => "atomic_replace",
            Self::Serialize(_) => "serialize",
            Self::SerializedSizeMismatch { .. } => "serialized_size_mismatch",
            Self::Io { .. } => "io",
            #[cfg(windows)]
            Self::WindowsNamespaceFailure { .. } => "windows_namespace_failure",
            #[cfg(windows)]
            Self::WindowsReplacePartial { .. } => "windows_replace_partial",
            #[cfg(windows)]
            Self::WindowsReplaceRecoveryRequired { .. } => "windows_replace_recovery_required",
            #[cfg(windows)]
            Self::WindowsMoveRecoveryRequired { .. } => "windows_move_recovery_required",
            #[cfg(windows)]
            Self::WindowsPostPublishIdentityConflict { .. } => {
                "windows_post_publish_identity_conflict"
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ManifestDegradedReason {
    Busy,
    LockUnavailable,
    InvalidCanonical,
    FutureSchema,
    ResourceBound(ResourceBoundKind),
    ExternalEdit,
    ReadOnlyCanonical,
    UnsafePath,
    PersistenceFailure,
    RecoveryRequired,
    IdentityConflict,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ManifestRecordOutcome {
    Recorded,
    Unchanged,
    PublishedUnrecorded(ManifestDegradedReason),
}

#[cfg(test)]
// The shared test module deliberately precedes the cfg-gated Windows implementation it exercises.
mod tests;

// ---------------------------------------------------------------------------
/// Stage E (#1064) adversarial-memory harness.
///
/// The retained adversarial parser-memory case is `#[ignore]` (registered in
/// `test-debt.allowlist.json`) and runs in an isolated release-mode child. Its
/// 512 MiB working-set gate is asserted only in a release build on the reference
/// machine; a debug run records the measurement without gating.
#[cfg(test)]
mod stage_e_conformance;
