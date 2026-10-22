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
mod stage_e_conformance {
    use super::*;
    use chrono::{DateTime, Utc};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    const HARD_WORKING_SET_LIMIT: u64 = 512 * 1024 * 1024;

    // ---- small local fixtures (kept independent of `mod tests`) ----

    fn timestamp(value: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(value)
            .expect("valid test timestamp")
            .with_timezone(&Utc)
    }

    fn setup_project() -> (tempfile::TempDir, PathBuf) {
        let temp = tempfile::tempdir().expect("tempdir");
        let project = temp.path().join("project");
        std::fs::create_dir_all(project.join(".ac")).expect("create project .ac");
        (temp, project)
    }

    fn canonical_path(project: &Path) -> PathBuf {
        project.join(".ac").join(SEED_MANIFEST_FILENAME)
    }

    fn config_scope(agent: &str, dest: &str) -> String {
        format!("config:.ac/wg-1-dev-team/__agent_{agent}/{dest}")
    }

    fn temp_leftovers(project: &Path) -> usize {
        let dir = project.join(".ac");
        std::fs::read_dir(&dir)
            .expect("read .ac")
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .file_name()
                    .to_str()
                    .map(is_exact_temp_name)
                    .unwrap_or(false)
            })
            .count()
    }

    // ---- working-set sampling (plan section 7.4 additional-working-set) ----

    #[cfg(windows)]
    fn working_set_bytes() -> u64 {
        use windows_sys::Win32::System::ProcessStatus::{
            K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS,
        };
        use windows_sys::Win32::System::Threading::GetCurrentProcess;
        let mut counters = unsafe { std::mem::zeroed::<PROCESS_MEMORY_COUNTERS>() };
        counters.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
        let ok =
            unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), &mut counters, counters.cb) };
        if ok == 0 {
            0
        } else {
            counters.WorkingSetSize as u64
        }
    }

    #[cfg(all(unix, target_os = "linux"))]
    fn working_set_bytes() -> u64 {
        // Resident pages from /proc/self/statm (field 1) times the page size.
        std::fs::read_to_string("/proc/self/statm")
            .ok()
            .and_then(|s| s.split_whitespace().nth(1).map(str::to_string))
            .and_then(|pages| pages.parse::<u64>().ok())
            .map(|pages| pages.saturating_mul(4096))
            .unwrap_or(0)
    }

    #[cfg(all(unix, not(target_os = "linux")))]
    fn working_set_bytes() -> u64 {
        0
    }

    /// Background sampler that tracks the peak working set during an operation.
    struct WorkingSetSampler {
        stop: Arc<AtomicBool>,
        peak: Arc<AtomicU64>,
        handle: Option<std::thread::JoinHandle<()>>,
    }

    impl WorkingSetSampler {
        fn start() -> Self {
            let stop = Arc::new(AtomicBool::new(false));
            let peak = Arc::new(AtomicU64::new(working_set_bytes()));
            let (stop_t, peak_t) = (Arc::clone(&stop), Arc::clone(&peak));
            let handle = std::thread::spawn(move || {
                while !stop_t.load(Ordering::Relaxed) {
                    peak_t.fetch_max(working_set_bytes(), Ordering::Relaxed);
                    std::thread::sleep(Duration::from_millis(2));
                }
                peak_t.fetch_max(working_set_bytes(), Ordering::Relaxed);
            });
            Self {
                stop,
                peak,
                handle: Some(handle),
            }
        }

        fn finish(mut self) -> u64 {
            self.stop.store(true, Ordering::Relaxed);
            if let Some(handle) = self.handle.take() {
                let _ = handle.join();
            }
            self.peak.load(Ordering::Relaxed)
        }
    }

    // -- plan section 10.1 item 4: adversarial parser-memory bound --
    #[test]
    #[ignore = "release-mode adversarial parser-memory measurement (plan 7.4/10.1 item 4)"]
    fn bench_adversarial_parser_memory_stays_bounded() {
        // Near the 128 MiB cap in release; a smaller shape in debug so a manual
        // debug run is tolerable. Each shape is generated, parsed, and dropped in
        // isolation with its own working-set sample.
        let target: usize = if cfg!(debug_assertions) {
            16 * 1024 * 1024
        } else {
            120 * 1024 * 1024
        };

        // (label, bytes, expect_ok)
        let mut cases: Vec<(&str, String, bool)> = Vec::new();

        // Valid near-cap manifest at the row cap with padded paths.
        cases.push(("valid-near-cap", valid_near_cap_manifest(target), true));
        // Adversarial invalid shapes: all must return a typed error, never abort.
        cases.push(("many-tiny-tables", many_tiny_tables(target), false));
        cases.push(("huge-escaped-string", huge_escaped_string(target), false));
        cases.push(("deep-array", deep_array(target), false));
        cases.push(("duplicate-headers", duplicate_headers(target), false));
        cases.push(("early-syntax-error", early_syntax_error(target), false));
        cases.push(("late-syntax-error", late_syntax_error(target), false));

        for (label, bytes, expect_ok) in cases {
            let input_len = bytes.len();
            let baseline = working_set_bytes();
            let sampler = WorkingSetSampler::start();
            let result = parse_manifest_bytes(bytes.as_bytes());
            drop(bytes);
            let peak = sampler.finish();
            let delta = peak.saturating_sub(baseline);
            if expect_ok {
                assert!(result.is_ok(), "{label}: valid near-cap input must parse");
            } else {
                assert!(
                    result.is_err(),
                    "{label}: adversarial input must return a typed error, not parse"
                );
            }
            eprintln!(
                "[stage-e adversarial] shape={label} input_bytes={input_len} \
                 ok={expect_ok} working_set_delta_bytes={delta}"
            );
            if !cfg!(debug_assertions) {
                assert!(
                    delta <= HARD_WORKING_SET_LIMIT,
                    "{label}: parser used {delta} bytes additional working set, over 512 MiB"
                );
            }
        }
    }

    fn manifest_prefix() -> String {
        let mut s = String::new();
        s.push_str(MANAGED_HEADER);
        s.push('\n');
        s.push_str("schema_version = 1\n");
        s.push_str("coverage_version = 2\n");
        s.push_str(
            "coverage = [\"project_context_templates\", \"replica_config_folders\", \
                \"coding_agent_catalog\"]\n",
        );
        s
    }

    fn valid_near_cap_manifest(target: usize) -> String {
        // Rows in one config scope; pad the path to grow bytes without exceeding
        // the 250k row cap. Stay comfortably under both caps.
        let mut out = manifest_prefix();
        let rows = 200_000usize;
        let pad = (target / rows).clamp(8, 4096);
        let filler = "d".repeat(pad);
        for i in 0..rows {
            out.push_str("\n[[files]]\n");
            out.push_str(&format!(
                "path = \".ac/wg-1-dev-team/__agent_alpha/.claude/{filler}-{i:07}\"\n"
            ));
            out.push_str("path_encoding = \"utf8\"\n");
            out.push_str("kind = \"replica_config_file\"\n");
            out.push_str("scope = \"config:.ac/wg-1-dev-team/__agent_alpha/.claude\"\n");
            out.push_str("source = \"workspace_base\"\n");
            out.push_str("last_seeded_at = \"2026-07-16T19:41:12.456Z\"\n");
            if out.len() >= target {
                break;
            }
        }
        out
    }

    fn many_tiny_tables(target: usize) -> String {
        // Valid-shaped tables but with a duplicate identity that is rejected.
        let mut out = manifest_prefix();
        while out.len() < target {
            out.push_str("\n[[files]]\n");
            out.push_str("path = \".ac/wg-1-dev-team/__agent_alpha/.claude/same\"\n");
            out.push_str("path_encoding = \"utf8\"\n");
            out.push_str("kind = \"replica_config_file\"\n");
            out.push_str("scope = \"config:.ac/wg-1-dev-team/__agent_alpha/.claude\"\n");
            out.push_str("source = \"workspace_base\"\n");
            out.push_str("last_seeded_at = \"2026-07-16T19:41:12.456Z\"\n");
        }
        out
    }

    fn huge_escaped_string(target: usize) -> String {
        let mut out = manifest_prefix();
        out.push_str("\n[[files]]\npath = \"");
        // A giant escaped-control-character string; the path decoder rejects it.
        while out.len() < target {
            out.push_str("\\u0000\\t\\\\");
        }
        out.push_str("\"\n");
        out
    }

    fn deep_array(target: usize) -> String {
        let mut out = manifest_prefix();
        out.push_str("\ncoverage = [");
        let depth = target / 2;
        for _ in 0..depth {
            out.push('[');
        }
        out
    }

    fn duplicate_headers(target: usize) -> String {
        let mut out = manifest_prefix();
        // Duplicate scalar top-level keys are a hard parse error.
        while out.len() < target {
            out.push_str("schema_version = 1\n");
        }
        out
    }

    fn early_syntax_error(target: usize) -> String {
        let mut out = String::from("!!! not toml at all\n");
        out.push_str(&manifest_prefix());
        while out.len() < target {
            out.push_str("padding = padding = padding\n");
        }
        out
    }

    fn late_syntax_error(target: usize) -> String {
        let mut out = valid_near_cap_manifest(target);
        out.push_str("\n[[files]\nbroken = \n");
        out
    }
}
