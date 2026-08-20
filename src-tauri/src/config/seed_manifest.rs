//! Activated project seed-manifest recorder.
//!
//! Stage F introduces the sole production [`ManifestActivationToken`] constructor
//! ([`ManifestActivationToken::production`]) and threads it through every
//! publisher and lifecycle hook named in [`V1_COVERAGE_BOUNDARIES`], so a
//! production build now emits `.ac/seed-manifest.toml`. Stages A through E kept
//! the module reachable only from `#[cfg(test)]` builds; that dormancy ends here.
//! The coverage declaration is the exhaustive compile-time checklist; it is not
//! by itself wiring evidence. Real-boundary coverage lives in
//! `tests/seed_manifest_activation.rs` plus the CLI integration suites:
//! boundaries reachable from a plain entry point are driven end-to-end through
//! the library compiled in non-test mode and assert the resulting manifest
//! mutation, while the `#[cfg(not(test))]`-gated and GUI-only boundaries are
//! guarded by source-scrape wiring assertions that red if a boundary's
//! `ManifestActivationToken::production()` threading (or its recording adapter)
//! is removed. Either way, removing an actual adapter call turns a test red
//! while this list stays green.
//!
//! #1318 - coverage grew to v2 (`coding_agent_catalog` kind): an existing v1
//! manifest is upgraded IN PLACE on the first writer/reader acquire (see
//! [`ProjectSeedManifestGuard::try_upgrade_v1_to_v2`]) with every row and
//! timestamp preserved verbatim; a v2 manifest written by this build is NOT
//! readable by an old build (writer strictness is a one-way door; an old build
//! preserves its bytes exactly). [`V1CoverageBoundary::CatalogSeed`] is the
//! first coverage-v2-era publisher; the declaration remains the exhaustive
//! production-publisher checklist across coverage versions.

#![allow(dead_code)]

use chrono::{DateTime, SecondsFormat, Utc};
use serde::de::{self, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use thiserror::Error;
use uuid::Uuid;

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
        "Windows seed-manifest namespace call {operation:?} failed before publication raw_error={raw_error} (0x{raw_error:08x}) canonical={canonical} temp={temp} canonical_state={canonical_state:?} temp_state={temp_state:?}"
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
        "Windows seed-manifest replacement partially changed the namespace raw_error={raw_error} (0x{raw_error:08x}) partial={partial:?} canonical={canonical} temp={temp} canonical_state={canonical_state:?} temp_state={temp_state:?}"
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
        "Windows seed-manifest replacement requires explicit recovery raw_error={raw_error} (0x{raw_error:08x}) canonical={canonical} temp={temp} canonical_state={canonical_state:?} temp_state={temp_state:?}"
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
        "Windows seed-manifest create-only move requires explicit recovery raw_error={raw_error} (0x{raw_error:08x}) canonical={canonical} temp={temp} canonical_state={canonical_state:?} temp_state={temp_state:?}"
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
        "Windows seed-manifest namespace call succeeded but final identity validation failed canonical={canonical} temp={temp} expected_source={expected_source:?} final_state={final_state:?} old_destination_identity={old_destination_identity:?}"
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ManifestPathEncoding {
    UnixBytesHex,
    Utf8,
    WindowsUtf16Hex,
}

impl ManifestPathEncoding {
    fn as_str(self) -> &'static str {
        match self {
            Self::UnixBytesHex => "unix_bytes_hex",
            Self::Utf8 => "utf8",
            Self::WindowsUtf16Hex => "windows_utf16_hex",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ManifestFileKind {
    ProjectContextTemplate,
    ReplicaConfigFile,
    /// #1318 - the coding-agent catalog manifest published per project at
    /// `.ac/coding-agents/agents.json` (scope `catalog:coding-agents`).
    CodingAgentCatalog,
}

impl ManifestFileKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::ProjectContextTemplate => "project_context_template",
            Self::ReplicaConfigFile => "replica_config_file",
            Self::CodingAgentCatalog => "coding_agent_catalog",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ManifestSource {
    Builtin,
    WorkspaceProfile,
    WorkspaceBase,
    MatrixProfile,
    MatrixBase,
    CatalogDefault,
}

impl ManifestSource {
    fn as_str(self) -> &'static str {
        match self {
            Self::Builtin => "builtin",
            Self::WorkspaceProfile => "workspace_profile",
            Self::WorkspaceBase => "workspace_base",
            Self::MatrixProfile => "matrix_profile",
            Self::MatrixBase => "matrix_base",
            Self::CatalogDefault => "catalog_default",
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SeedManifestWire {
    schema_version: u32,
    coverage_version: u32,
    coverage: Vec<String>,
    #[serde(deserialize_with = "deserialize_rows")]
    files: Vec<SeedManifestRowWire>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SeedManifestRowWire {
    #[serde(deserialize_with = "deserialize_path_string")]
    path: String,
    path_encoding: ManifestPathEncoding,
    kind: ManifestFileKind,
    #[serde(deserialize_with = "deserialize_scope_string")]
    scope: String,
    source: ManifestSource,
    last_seeded_at: String,
}

fn deserialize_path_string<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    deserializer.deserialize_string(LimitedStringVisitor {
        label: "path",
        bound: ResourceBoundKind::PathBytes,
    })
}

fn deserialize_scope_string<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    deserializer.deserialize_string(LimitedStringVisitor {
        label: "scope",
        bound: ResourceBoundKind::ScopeBytes,
    })
}

struct LimitedStringVisitor {
    label: &'static str,
    bound: ResourceBoundKind,
}

impl Visitor<'_> for LimitedStringVisitor {
    type Value = String;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "a {} string no longer than {} UTF-8 bytes",
            self.label, MAX_FIELD_BYTES
        )
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        if value.len() > MAX_FIELD_BYTES {
            return Err(E::custom(format!(
                "{} {} bound exceeded: limit={}, observed_at_least={}",
                self.label,
                self.bound,
                MAX_FIELD_BYTES,
                value.len()
            )));
        }
        Ok(value.to_string())
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        if value.len() > MAX_FIELD_BYTES {
            return Err(E::custom(format!(
                "{} {} bound exceeded: limit={}, observed_at_least={}",
                self.label,
                self.bound,
                MAX_FIELD_BYTES,
                value.len()
            )));
        }
        Ok(value)
    }
}

fn deserialize_rows<'de, D>(deserializer: D) -> Result<Vec<SeedManifestRowWire>, D::Error>
where
    D: Deserializer<'de>,
{
    struct RowsVisitor;

    impl<'de> Visitor<'de> for RowsVisitor {
        type Value = Vec<SeedManifestRowWire>;

        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(
                formatter,
                "at most {} seed-manifest rows",
                MAX_MANIFEST_ROWS
            )
        }

        fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
        where
            A: SeqAccess<'de>,
        {
            let capacity = sequence.size_hint().unwrap_or(0).min(MAX_MANIFEST_ROWS);
            let mut rows = Vec::with_capacity(capacity);
            while let Some(row) = sequence.next_element()? {
                if rows.len() == MAX_MANIFEST_ROWS {
                    return Err(de::Error::custom(format!(
                        "rows {} bound exceeded: limit={}, observed_at_least={}",
                        ResourceBoundKind::Rows,
                        MAX_MANIFEST_ROWS,
                        MAX_MANIFEST_ROWS + 1
                    )));
                }
                rows.push(row);
            }
            Ok(rows)
        }
    }

    deserializer.deserialize_seq(RowsVisitor)
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum DecodedPathComponents {
    Utf8(Vec<String>),
    UnixBytes(Vec<Vec<u8>>),
    WindowsUtf16(Vec<Vec<u16>>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ManifestPathIdentity {
    encoding: ManifestPathEncoding,
    serialized: String,
    components: DecodedPathComponents,
}

impl ManifestPathIdentity {
    pub(crate) fn parse(
        encoding: ManifestPathEncoding,
        serialized: String,
    ) -> Result<Self, SeedManifestError> {
        if serialized.len() > MAX_FIELD_BYTES {
            return Err(SeedManifestError::resource_bound(
                ResourceBoundKind::PathBytes,
                MAX_FIELD_BYTES,
                serialized.len(),
            ));
        }

        let components = match encoding {
            ManifestPathEncoding::Utf8 => {
                DecodedPathComponents::Utf8(parse_utf8_components(&serialized)?)
            }
            ManifestPathEncoding::UnixBytesHex => {
                DecodedPathComponents::UnixBytes(parse_unix_hex_components(&serialized)?)
            }
            ManifestPathEncoding::WindowsUtf16Hex => {
                DecodedPathComponents::WindowsUtf16(parse_windows_hex_components(&serialized)?)
            }
        };
        let identity = Self {
            encoding,
            serialized,
            components,
        };
        identity.require_ac_root()?;
        Ok(identity)
    }

    pub(crate) fn from_relative_path(path: &Path) -> Result<Self, SeedManifestError> {
        if path.is_absolute() {
            return Err(SeedManifestError::Validation(format!(
                "manifest path must be relative: {}",
                path.display()
            )));
        }

        let mut components = Vec::new();
        for component in path.components() {
            match component {
                Component::Normal(value) => components.push(value),
                _ => {
                    return Err(SeedManifestError::Validation(format!(
                        "manifest path contains a non-normal component: {}",
                        path.display()
                    )))
                }
            }
        }
        if components.is_empty() {
            return Err(SeedManifestError::Validation(
                "manifest path must contain at least one component".to_string(),
            ));
        }

        if components
            .iter()
            .all(|component| component.to_str().is_some())
        {
            let serialized = components
                .iter()
                .map(|component| component.to_str().unwrap_or_default())
                .collect::<Vec<_>>()
                .join("/");
            return Self::parse(ManifestPathEncoding::Utf8, serialized);
        }

        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;

            let mut bytes = Vec::new();
            for (index, component) in components.iter().enumerate() {
                if index > 0 {
                    bytes.push(b'/');
                }
                bytes.extend_from_slice(component.as_bytes());
            }
            let serialized = encode_lower_hex_bytes(&bytes);
            return Self::parse(ManifestPathEncoding::UnixBytesHex, serialized);
        }

        #[cfg(windows)]
        {
            use std::os::windows::ffi::OsStrExt;

            let mut units = Vec::new();
            for (index, component) in components.iter().enumerate() {
                if index > 0 {
                    units.push(u16::from(b'/'));
                }
                units.extend(component.encode_wide());
            }
            let serialized = encode_lower_hex_u16(&units);
            return Self::parse(ManifestPathEncoding::WindowsUtf16Hex, serialized);
        }

        #[allow(unreachable_code)]
        Err(SeedManifestError::Validation(
            "native path encoding is unsupported on this platform".to_string(),
        ))
    }

    pub(crate) fn encoding(&self) -> ManifestPathEncoding {
        self.encoding
    }

    pub(crate) fn serialized(&self) -> &str {
        &self.serialized
    }

    fn key(&self) -> ManifestPathKey {
        ManifestPathKey {
            encoding: self.encoding,
            serialized: self.serialized.clone(),
        }
    }

    fn require_ac_root(&self) -> Result<(), SeedManifestError> {
        let is_ac = match &self.components {
            DecodedPathComponents::Utf8(components) => {
                components.first().map(String::as_str) == Some(".ac")
            }
            DecodedPathComponents::UnixBytes(components) => {
                components.first().map(Vec::as_slice) == Some(b".ac".as_slice())
            }
            DecodedPathComponents::WindowsUtf16(components) => {
                components.first().map(|component| component.as_slice())
                    == Some([46_u16, 97_u16, 99_u16].as_slice())
            }
        };
        if !is_ac {
            return Err(SeedManifestError::Validation(format!(
                "manifest path must begin with the .ac component: {}",
                self.serialized
            )));
        }
        Ok(())
    }

    fn starts_with_utf8_components(&self, prefix: &[String]) -> bool {
        match &self.components {
            DecodedPathComponents::Utf8(components) => components.starts_with(prefix),
            DecodedPathComponents::UnixBytes(components) => {
                components.len() >= prefix.len()
                    && components
                        .iter()
                        .zip(prefix)
                        .all(|(actual, expected)| actual.as_slice() == expected.as_bytes())
            }
            DecodedPathComponents::WindowsUtf16(components) => {
                components.len() >= prefix.len()
                    && components.iter().zip(prefix).all(|(actual, expected)| {
                        actual.iter().copied().eq(expected.encode_utf16())
                    })
            }
        }
    }

    fn is_strict_descendant_of_utf8(&self, prefix: &[String]) -> bool {
        let component_count = match &self.components {
            DecodedPathComponents::Utf8(components) => components.len(),
            DecodedPathComponents::UnixBytes(components) => components.len(),
            DecodedPathComponents::WindowsUtf16(components) => components.len(),
        };
        component_count > prefix.len() && self.starts_with_utf8_components(prefix)
    }

    fn utf8_component_equals(&self, index: usize, expected: &str) -> bool {
        match &self.components {
            DecodedPathComponents::Utf8(components) => {
                components.get(index).map(String::as_str) == Some(expected)
            }
            DecodedPathComponents::UnixBytes(components) => {
                components.get(index).map(Vec::as_slice) == Some(expected.as_bytes())
            }
            DecodedPathComponents::WindowsUtf16(components) => components
                .get(index)
                .map(|actual| actual.iter().copied().eq(expected.encode_utf16()))
                .unwrap_or(false),
        }
    }
}

fn validate_plain_component(component: &str, label: &str) -> Result<(), SeedManifestError> {
    if component.is_empty() || component == "." || component == ".." {
        return Err(SeedManifestError::Validation(format!(
            "{} contains an empty or traversal component",
            label
        )));
    }
    if component.contains('/') || component.contains('\0') {
        return Err(SeedManifestError::Validation(format!(
            "{} contains a separator or NUL inside a component",
            label
        )));
    }
    Ok(())
}

fn parse_utf8_components(serialized: &str) -> Result<Vec<String>, SeedManifestError> {
    if serialized.is_empty() || serialized.starts_with('/') || serialized.ends_with('/') {
        return Err(SeedManifestError::Validation(format!(
            "invalid relative UTF-8 manifest path: {serialized:?}"
        )));
    }
    let components = serialized
        .split('/')
        .map(str::to_string)
        .collect::<Vec<_>>();
    for component in &components {
        validate_plain_component(component, "UTF-8 manifest path")?;
    }
    Ok(components)
}

fn parse_lower_hex_bytes(serialized: &str) -> Result<Vec<u8>, SeedManifestError> {
    if serialized.is_empty() || !serialized.len().is_multiple_of(2) {
        return Err(SeedManifestError::Validation(
            "unix_bytes_hex must contain a nonempty even number of digits".to_string(),
        ));
    }
    if !serialized
        .bytes()
        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(SeedManifestError::Validation(
            "unix_bytes_hex must use lowercase fixed-width hexadecimal".to_string(),
        ));
    }
    serialized
        .as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|digits| {
            let text = std::str::from_utf8(digits).map_err(|error| {
                SeedManifestError::Validation(format!("invalid unix_bytes_hex: {error}"))
            })?;
            u8::from_str_radix(text, 16).map_err(|error| {
                SeedManifestError::Validation(format!("invalid unix_bytes_hex: {error}"))
            })
        })
        .collect()
}

fn parse_unix_hex_components(serialized: &str) -> Result<Vec<Vec<u8>>, SeedManifestError> {
    let bytes = parse_lower_hex_bytes(serialized)?;
    if std::str::from_utf8(&bytes).is_ok() {
        return Err(SeedManifestError::Validation(
            "unix_bytes_hex is noncanonical because the decoded path is valid UTF-8".to_string(),
        ));
    }
    split_byte_components(&bytes)
}

fn split_byte_components(bytes: &[u8]) -> Result<Vec<Vec<u8>>, SeedManifestError> {
    let mut components = Vec::new();
    for component in bytes.split(|byte| *byte == b'/') {
        if component.is_empty() || component == b"." || component == b".." || component.contains(&0)
        {
            return Err(SeedManifestError::Validation(
                "unix_bytes_hex contains an empty, traversal, or NUL component".to_string(),
            ));
        }
        components.push(component.to_vec());
    }
    Ok(components)
}

fn parse_windows_hex_components(serialized: &str) -> Result<Vec<Vec<u16>>, SeedManifestError> {
    if serialized.is_empty() || !serialized.len().is_multiple_of(4) {
        return Err(SeedManifestError::Validation(
            "windows_utf16_hex must contain a nonempty multiple of four digits".to_string(),
        ));
    }
    if !serialized
        .bytes()
        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(SeedManifestError::Validation(
            "windows_utf16_hex must use lowercase fixed-width hexadecimal".to_string(),
        ));
    }
    let units = serialized
        .as_bytes()
        .as_chunks::<4>()
        .0
        .iter()
        .map(|digits| {
            let text = std::str::from_utf8(digits).map_err(|error| {
                SeedManifestError::Validation(format!("invalid windows_utf16_hex: {error}"))
            })?;
            u16::from_str_radix(text, 16).map_err(|error| {
                SeedManifestError::Validation(format!("invalid windows_utf16_hex: {error}"))
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    if !char::decode_utf16(units.iter().copied()).any(|unit| unit.is_err()) {
        return Err(SeedManifestError::Validation(
            "windows_utf16_hex is noncanonical because the decoded path is valid Unicode"
                .to_string(),
        ));
    }

    let mut components = Vec::new();
    for component in units.split(|unit| *unit == u16::from(b'/')) {
        if component.is_empty()
            || component == [u16::from(b'.')]
            || component == [u16::from(b'.'), u16::from(b'.')]
            || component.contains(&0)
            || component.contains(&u16::from(b'\\'))
        {
            return Err(SeedManifestError::Validation(
                "windows_utf16_hex contains an empty, traversal, separator, or NUL component"
                    .to_string(),
            ));
        }
        components.push(component.to_vec());
    }
    Ok(components)
}

fn encode_lower_hex_bytes(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len().saturating_mul(2));
    for byte in bytes {
        encoded.push(char::from(HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    encoded
}

fn encode_lower_hex_u16(units: &[u16]) -> String {
    let mut encoded = String::with_capacity(units.len().saturating_mul(4));
    for unit in units {
        use std::fmt::Write as _;
        let _ = write!(encoded, "{unit:04x}");
    }
    encoded
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ManifestPathKey {
    encoding: ManifestPathEncoding,
    serialized: String,
}

impl Ord for ManifestPathKey {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.encoding
            .as_str()
            .as_bytes()
            .cmp(other.encoding.as_str().as_bytes())
            .then_with(|| self.serialized.as_bytes().cmp(other.serialized.as_bytes()))
    }
}

impl PartialOrd for ManifestPathKey {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PublishedManifestRow {
    path: ManifestPathIdentity,
    kind: ManifestFileKind,
    scope: String,
    source: ManifestSource,
    published_at: DateTime<Utc>,
}

impl PublishedManifestRow {
    pub(crate) fn project_context(
        path: ManifestPathIdentity,
        scope: &str,
        published_at: DateTime<Utc>,
    ) -> Result<Self, SeedManifestError> {
        let row = Self {
            path,
            kind: ManifestFileKind::ProjectContextTemplate,
            scope: scope.to_string(),
            source: ManifestSource::Builtin,
            published_at,
        };
        validate_row(&row)?;
        Ok(row)
    }

    pub(crate) fn replica_config(
        path: ManifestPathIdentity,
        scope: String,
        source: ManifestSource,
        published_at: DateTime<Utc>,
    ) -> Result<Self, SeedManifestError> {
        let row = Self {
            path,
            kind: ManifestFileKind::ReplicaConfigFile,
            scope,
            source,
            published_at,
        };
        validate_row(&row)?;
        Ok(row)
    }

    /// #1318 - one coding-agent catalog publication row: `.ac/coding-agents/
    /// agents.json`, `builtin` source, scope `catalog:coding-agents`. Records
    /// the WRITE, not content validity: a verbatim-migrated legacy catalog that
    /// does not parse still records its copy (the read path self-heals).
    pub(crate) fn coding_agent_catalog(
        path: ManifestPathIdentity,
        published_at: DateTime<Utc>,
    ) -> Result<Self, SeedManifestError> {
        let row = Self {
            path,
            kind: ManifestFileKind::CodingAgentCatalog,
            scope: "catalog:coding-agents".to_string(),
            source: ManifestSource::Builtin,
            published_at,
        };
        validate_row(&row)?;
        Ok(row)
    }

    fn to_wire(&self) -> SeedManifestRowWire {
        SeedManifestRowWire {
            path: self.path.serialized.clone(),
            path_encoding: self.path.encoding,
            kind: self.kind,
            scope: self.scope.clone(),
            source: self.source,
            last_seeded_at: canonical_timestamp(self.published_at),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct ManifestState {
    rows: BTreeMap<ManifestPathKey, PublishedManifestRow>,
}

impl ManifestState {
    fn from_wire(wire: SeedManifestWire) -> Result<Self, SeedManifestError> {
        if wire.schema_version > SCHEMA_VERSION {
            return Err(SeedManifestError::FutureSchema {
                found: wire.schema_version,
                supported: SCHEMA_VERSION,
            });
        }
        if wire.schema_version != SCHEMA_VERSION {
            return Err(SeedManifestError::Validation(format!(
                "schema_version must be {SCHEMA_VERSION}, found {}",
                wire.schema_version
            )));
        }
        if wire.coverage_version != COVERAGE_VERSION {
            return Err(SeedManifestError::Validation(format!(
                "coverage_version must be {COVERAGE_VERSION}, found {}",
                wire.coverage_version
            )));
        }
        let expected_coverage = COVERAGE.map(str::to_string).to_vec();
        if wire.coverage != expected_coverage {
            return Err(SeedManifestError::Validation(format!(
                "coverage must equal {:?} in that order",
                COVERAGE
            )));
        }
        if wire.files.len() > MAX_MANIFEST_ROWS {
            return Err(SeedManifestError::resource_bound(
                ResourceBoundKind::Rows,
                MAX_MANIFEST_ROWS,
                wire.files.len(),
            ));
        }

        let mut rows = BTreeMap::new();
        let mut config_batches: BTreeMap<String, (ManifestSource, String)> = BTreeMap::new();
        for row in wire.files {
            if row.path.len() > MAX_FIELD_BYTES {
                return Err(SeedManifestError::resource_bound(
                    ResourceBoundKind::PathBytes,
                    MAX_FIELD_BYTES,
                    row.path.len(),
                ));
            }
            if row.scope.len() > MAX_FIELD_BYTES {
                return Err(SeedManifestError::resource_bound(
                    ResourceBoundKind::ScopeBytes,
                    MAX_FIELD_BYTES,
                    row.scope.len(),
                ));
            }
            let path = ManifestPathIdentity::parse(row.path_encoding, row.path)?;
            let published_at = parse_canonical_timestamp(&row.last_seeded_at)?;
            let internal = PublishedManifestRow {
                path,
                kind: row.kind,
                scope: row.scope,
                source: row.source,
                published_at,
            };
            validate_row(&internal)?;
            if internal.kind == ManifestFileKind::ReplicaConfigFile {
                let canonical_time = canonical_timestamp(internal.published_at);
                match config_batches.get(&internal.scope) {
                    Some((source, time))
                        if *source != internal.source || *time != canonical_time =>
                    {
                        return Err(SeedManifestError::Validation(format!(
                            "config scope {} contains mixed source or publication time",
                            internal.scope
                        )));
                    }
                    Some(_) => {}
                    None => {
                        config_batches
                            .insert(internal.scope.clone(), (internal.source, canonical_time));
                    }
                }
            }
            let key = internal.path.key();
            if rows.insert(key, internal).is_some() {
                return Err(SeedManifestError::Validation(
                    "duplicate manifest path identity".to_string(),
                ));
            }
        }
        Ok(Self { rows })
    }

    fn to_wire(&self) -> SeedManifestWire {
        SeedManifestWire {
            schema_version: SCHEMA_VERSION,
            coverage_version: COVERAGE_VERSION,
            coverage: COVERAGE.map(str::to_string).to_vec(),
            files: self
                .rows
                .values()
                .map(PublishedManifestRow::to_wire)
                .collect(),
        }
    }

    fn upsert(&mut self, row: PublishedManifestRow) -> RowMutationJournal {
        let key = row.path.key();
        let previous = self.rows.insert(key.clone(), row);
        RowMutationJournal { key, previous }
    }

    fn replace_scope(
        &mut self,
        scope: &str,
        replacement: Vec<PublishedManifestRow>,
    ) -> ScopeMutationJournal {
        let removed_keys = self
            .rows
            .iter()
            .filter_map(|(key, row)| (row.scope == scope).then_some(key.clone()))
            .collect::<Vec<_>>();
        let mut removed = Vec::with_capacity(removed_keys.len());
        for key in removed_keys {
            if let Some(row) = self.rows.remove(&key) {
                removed.push((key, row));
            }
        }
        let mut inserted = Vec::with_capacity(replacement.len());
        for row in replacement {
            let key = row.path.key();
            self.rows.insert(key.clone(), row);
            inserted.push(key);
        }
        ScopeMutationJournal { removed, inserted }
    }

    fn remove_scope(&mut self, scope: &str) -> RemovalMutationJournal {
        self.remove_matching(|row| row.scope == scope)
    }

    fn remove_matching(
        &mut self,
        predicate: impl Fn(&PublishedManifestRow) -> bool,
    ) -> RemovalMutationJournal {
        let keys = self
            .rows
            .iter()
            .filter_map(|(key, row)| predicate(row).then_some(key.clone()))
            .collect::<Vec<_>>();
        let mut removed = Vec::with_capacity(keys.len());
        for key in keys {
            if let Some(row) = self.rows.remove(&key) {
                removed.push((key, row));
            }
        }
        RemovalMutationJournal { removed }
    }
}

#[derive(Debug)]
struct RowMutationJournal {
    key: ManifestPathKey,
    previous: Option<PublishedManifestRow>,
}

impl RowMutationJournal {
    fn changed(&self, state: &ManifestState) -> bool {
        match (&self.previous, state.rows.get(&self.key)) {
            (Some(previous), Some(current)) => previous != current,
            (None, Some(_)) => true,
            _ => false,
        }
    }

    fn rollback(self, state: &mut ManifestState) {
        match self.previous {
            Some(row) => {
                state.rows.insert(self.key, row);
            }
            None => {
                state.rows.remove(&self.key);
            }
        }
    }
}

#[derive(Debug)]
struct ScopeMutationJournal {
    removed: Vec<(ManifestPathKey, PublishedManifestRow)>,
    inserted: Vec<ManifestPathKey>,
}

impl ScopeMutationJournal {
    fn changed(&self, state: &ManifestState) -> bool {
        if self.removed.len() != self.inserted.len() {
            return true;
        }
        self.removed
            .iter()
            .any(|(key, previous)| state.rows.get(key) != Some(previous))
    }

    fn rollback(self, state: &mut ManifestState) {
        for key in self.inserted {
            state.rows.remove(&key);
        }
        for (key, row) in self.removed {
            state.rows.insert(key, row);
        }
    }
}

#[derive(Debug)]
struct RemovalMutationJournal {
    removed: Vec<(ManifestPathKey, PublishedManifestRow)>,
}

impl RemovalMutationJournal {
    fn changed(&self) -> bool {
        !self.removed.is_empty()
    }

    fn rollback(self, state: &mut ManifestState) {
        for (key, row) in self.removed {
            state.rows.insert(key, row);
        }
    }
}

fn validate_row(row: &PublishedManifestRow) -> Result<(), SeedManifestError> {
    if row.path.serialized.len() > MAX_FIELD_BYTES {
        return Err(SeedManifestError::resource_bound(
            ResourceBoundKind::PathBytes,
            MAX_FIELD_BYTES,
            row.path.serialized.len(),
        ));
    }
    if row.scope.len() > MAX_FIELD_BYTES {
        return Err(SeedManifestError::resource_bound(
            ResourceBoundKind::ScopeBytes,
            MAX_FIELD_BYTES,
            row.scope.len(),
        ));
    }

    match row.kind {
        ManifestFileKind::ProjectContextTemplate => {
            if row.path.encoding != ManifestPathEncoding::Utf8
                || row.source != ManifestSource::Builtin
            {
                return Err(SeedManifestError::Validation(
                    "context rows require utf8 encoding and builtin source".to_string(),
                ));
            }
            let expected_scope = match row.path.serialized.as_str() {
                ".ac/Context.AgentsCommander.md" => "context:agentscommander",
                ".ac/Context.coordinator.md" => "context:coordinator",
                _ => {
                    return Err(SeedManifestError::Validation(format!(
                        "unsupported project context path {}",
                        row.path.serialized
                    )))
                }
            };
            if row.scope != expected_scope {
                return Err(SeedManifestError::Validation(format!(
                    "context path {} requires scope {}",
                    row.path.serialized, expected_scope
                )));
            }
        }
        ManifestFileKind::ReplicaConfigFile => {
            if row.source == ManifestSource::Builtin {
                return Err(SeedManifestError::Validation(
                    "replica config rows cannot use builtin source".to_string(),
                ));
            }
            let scope_components = parse_config_scope(&row.scope)?;
            if !row.path.is_strict_descendant_of_utf8(&scope_components) {
                return Err(SeedManifestError::Validation(format!(
                    "config row {} is outside scope {}",
                    row.path.serialized, row.scope
                )));
            }
        }
        ManifestFileKind::CodingAgentCatalog => {
            if row.path.encoding != ManifestPathEncoding::Utf8
                || row.source != ManifestSource::Builtin
            {
                return Err(SeedManifestError::Validation(
                    "coding-agent catalog rows require utf8 encoding and builtin source"
                        .to_string(),
                ));
            }
            if row.path.serialized != ".ac/coding-agents/agents.json" {
                return Err(SeedManifestError::Validation(format!(
                    "unsupported coding-agent catalog path {}",
                    row.path.serialized
                )));
            }
            if row.scope != "catalog:coding-agents" {
                return Err(SeedManifestError::Validation(format!(
                    "coding-agent catalog path {} requires scope catalog:coding-agents",
                    row.path.serialized
                )));
            }
        }
    }
    Ok(())
}

fn parse_config_scope(scope: &str) -> Result<Vec<String>, SeedManifestError> {
    if scope.len() > MAX_FIELD_BYTES {
        return Err(SeedManifestError::resource_bound(
            ResourceBoundKind::ScopeBytes,
            MAX_FIELD_BYTES,
            scope.len(),
        ));
    }
    let path = scope
        .strip_prefix("config:")
        .ok_or_else(|| SeedManifestError::Validation(format!("invalid config scope {scope:?}")))?;
    let components = parse_utf8_components(path)?;
    if components.len() != 4 {
        return Err(SeedManifestError::Validation(format!(
            "config scope must be config:.ac/<workgroup>/<replica>/<dest>: {scope}"
        )));
    }
    crate::commands::entity_creation::parse_team_from_workgroup_name(&components[1])
        .map_err(SeedManifestError::Validation)?;
    let agent = components[2].strip_prefix("__agent_").ok_or_else(|| {
        SeedManifestError::Validation(format!(
            "config scope replica must be named __agent_<name>: {}",
            components[2]
        ))
    })?;
    crate::commands::entity_creation::validate_existing_name(agent, "Agent")
        .map_err(SeedManifestError::Validation)?;
    crate::config::settings::validate_config_seed_dest(&components[3])
        .map_err(SeedManifestError::Validation)?;
    Ok(components)
}

fn parse_canonical_timestamp(value: &str) -> Result<DateTime<Utc>, SeedManifestError> {
    let parsed = DateTime::parse_from_rfc3339(value).map_err(|error| {
        SeedManifestError::Validation(format!("invalid last_seeded_at {value:?}: {error}"))
    })?;
    let utc = parsed.with_timezone(&Utc);
    if canonical_timestamp(utc) != value {
        return Err(SeedManifestError::Validation(format!(
            "last_seeded_at is not canonical UTC milliseconds: {value:?}"
        )));
    }
    Ok(utc)
}

fn canonical_timestamp(value: DateTime<Utc>) -> String {
    value.to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn checked_add(total: &mut usize, value: usize) -> Result<(), SeedManifestError> {
    *total = total.checked_add(value).ok_or_else(|| {
        SeedManifestError::resource_bound(
            ResourceBoundKind::ArithmeticOverflow,
            usize::MAX,
            usize::MAX,
        )
    })?;
    Ok(())
}

fn toml_basic_string_escaped_len(value: &str) -> Result<usize, SeedManifestError> {
    let mut length = 0_usize;
    for character in value.chars() {
        let encoded = match character {
            '\u{0008}' | '\t' | '\n' | '\u{000c}' | '\r' | '"' | '\\' => 2,
            '\u{0000}'..='\u{001f}' | '\u{007f}' => 6,
            _ => character.len_utf8(),
        };
        checked_add(&mut length, encoded)?;
    }
    Ok(length)
}

/// Exact serialized contribution of one config row in the fixed v1 wire
/// layout. Config staging uses this before retaining another path identity so a
/// successful physical copy never requires an unbounded metadata allocation.
pub(crate) fn exact_config_row_serialized_len(
    project_relative_path: &Path,
    scope: &str,
    source: ManifestSource,
) -> Result<usize, SeedManifestError> {
    let path = ManifestPathIdentity::from_relative_path(project_relative_path)?;
    parse_config_scope(scope)?;
    let mut length = "\n[[files]]\n".len();
    checked_add(&mut length, "path = \"\"\n".len())?;
    checked_add(
        &mut length,
        toml_basic_string_escaped_len(&path.serialized)?,
    )?;
    checked_add(&mut length, "path_encoding = \"\"\n".len())?;
    checked_add(
        &mut length,
        toml_basic_string_escaped_len(path.encoding.as_str())?,
    )?;
    checked_add(&mut length, "kind = \"\"\n".len())?;
    checked_add(
        &mut length,
        toml_basic_string_escaped_len(ManifestFileKind::ReplicaConfigFile.as_str())?,
    )?;
    checked_add(&mut length, "scope = \"\"\n".len())?;
    checked_add(&mut length, toml_basic_string_escaped_len(scope)?)?;
    checked_add(&mut length, "source = \"\"\n".len())?;
    checked_add(&mut length, toml_basic_string_escaped_len(source.as_str())?)?;
    checked_add(&mut length, "last_seeded_at = \"\"\n".len())?;
    checked_add(
        &mut length,
        toml_basic_string_escaped_len("1970-01-01T00:00:00.000Z")?,
    )?;
    Ok(length)
}

pub(crate) fn config_batch_base_serialized_len() -> usize {
    MANAGED_HEADER.len()
        + "schema_version = 1\n".len()
        + "coverage_version = 2\n".len()
        + "coverage = [\"project_context_templates\", \"replica_config_folders\", \"coding_agent_catalog\"]\n".len()
}

fn exact_serialized_len(state: &ManifestState) -> Result<usize, SeedManifestError> {
    exact_serialized_len_with_limit(state, MAX_MANIFEST_BYTES)
}

fn exact_serialized_len_with_limit(
    state: &ManifestState,
    output_limit: u64,
) -> Result<usize, SeedManifestError> {
    if state.rows.len() > MAX_MANIFEST_ROWS {
        return Err(SeedManifestError::resource_bound(
            ResourceBoundKind::Rows,
            MAX_MANIFEST_ROWS,
            state.rows.len(),
        ));
    }

    let mut length = MANAGED_HEADER.len();
    checked_add(&mut length, "schema_version = 1\n".len())?;
    checked_add(&mut length, "coverage_version = 2\n".len())?;
    checked_add(
        &mut length,
        "coverage = [\"project_context_templates\", \"replica_config_folders\", \"coding_agent_catalog\"]\n".len(),
    )?;
    if state.rows.is_empty() {
        checked_add(&mut length, "files = []\n".len())?;
    } else {
        for row in state.rows.values() {
            checked_add(&mut length, "\n[[files]]\n".len())?;
            checked_add(&mut length, "path = \"\"\n".len())?;
            checked_add(
                &mut length,
                toml_basic_string_escaped_len(&row.path.serialized)?,
            )?;
            checked_add(&mut length, "path_encoding = \"\"\n".len())?;
            checked_add(
                &mut length,
                toml_basic_string_escaped_len(row.path.encoding.as_str())?,
            )?;
            checked_add(&mut length, "kind = \"\"\n".len())?;
            checked_add(
                &mut length,
                toml_basic_string_escaped_len(row.kind.as_str())?,
            )?;
            checked_add(&mut length, "scope = \"\"\n".len())?;
            checked_add(&mut length, toml_basic_string_escaped_len(&row.scope)?)?;
            checked_add(&mut length, "source = \"\"\n".len())?;
            checked_add(
                &mut length,
                toml_basic_string_escaped_len(row.source.as_str())?,
            )?;
            checked_add(&mut length, "last_seeded_at = \"\"\n".len())?;
            checked_add(
                &mut length,
                toml_basic_string_escaped_len(&canonical_timestamp(row.published_at))?,
            )?;
        }
    }
    if u64::try_from(length).unwrap_or(u64::MAX) > output_limit {
        return Err(SeedManifestError::ResourceBound {
            kind: ResourceBoundKind::OutputBytes,
            limit: output_limit,
            observed_at_least: u64::try_from(length).unwrap_or(u64::MAX),
        });
    }
    Ok(length)
}

fn serialize_state(state: &ManifestState) -> Result<Vec<u8>, SeedManifestError> {
    let exact_length = exact_serialized_len(state)?;
    let body = toml::to_string(&state.to_wire())
        .map_err(|error| SeedManifestError::Serialize(error.to_string()))?;
    let body = body.trim_end_matches('\n');
    let mut bytes = Vec::with_capacity(exact_length);
    bytes.extend_from_slice(MANAGED_HEADER.as_bytes());
    bytes.extend_from_slice(body.as_bytes());
    bytes.push(b'\n');
    if bytes.len() != exact_length {
        return Err(SeedManifestError::SerializedSizeMismatch {
            counted: exact_length,
            actual: bytes.len(),
        });
    }
    Ok(bytes)
}

fn parse_manifest_bytes(raw: &[u8]) -> Result<ManifestState, SeedManifestError> {
    let text = std::str::from_utf8(raw).map_err(|error| {
        SeedManifestError::Parse(format!(
            "manifest is not UTF-8 at byte offset {} with invalid_sequence_length={}",
            error.valid_up_to(),
            error.error_len().map_or(0, usize::from)
        ))
    })?;
    let wire: SeedManifestWire = toml::from_str(text).map_err(classify_toml_parse_error)?;
    ManifestState::from_wire(wire).map_err(sanitize_canonical_validation_error)
}

fn classify_toml_parse_error(error: toml::de::Error) -> SeedManifestError {
    let message = error.message();
    let bound = if message.starts_with("path path bytes bound exceeded:") {
        Some(ResourceBoundKind::PathBytes)
    } else if message.starts_with("scope scope bytes bound exceeded:") {
        Some(ResourceBoundKind::ScopeBytes)
    } else if message.starts_with("rows rows bound exceeded:") {
        Some(ResourceBoundKind::Rows)
    } else {
        None
    };
    match bound {
        Some(ResourceBoundKind::PathBytes) => SeedManifestError::resource_bound(
            ResourceBoundKind::PathBytes,
            MAX_FIELD_BYTES,
            MAX_FIELD_BYTES.saturating_add(1),
        ),
        Some(ResourceBoundKind::ScopeBytes) => SeedManifestError::resource_bound(
            ResourceBoundKind::ScopeBytes,
            MAX_FIELD_BYTES,
            MAX_FIELD_BYTES.saturating_add(1),
        ),
        Some(ResourceBoundKind::Rows) => SeedManifestError::resource_bound(
            ResourceBoundKind::Rows,
            MAX_MANIFEST_ROWS,
            MAX_MANIFEST_ROWS.saturating_add(1),
        ),
        _ => match error.span() {
            Some(span) => SeedManifestError::Parse(format!(
                "invalid TOML syntax or schema at byte range {}..{}",
                span.start, span.end
            )),
            None => SeedManifestError::Parse("invalid TOML syntax or schema".to_string()),
        },
    }
}

fn sanitize_canonical_validation_error(error: SeedManifestError) -> SeedManifestError {
    match error {
        SeedManifestError::Validation(_) => SeedManifestError::Validation(
            "canonical fields failed strict v1 validation".to_string(),
        ),
        other => other,
    }
}

fn read_bounded_file(
    file: &mut File,
    known_length: u64,
    limit: u64,
    path: &Path,
) -> Result<Vec<u8>, SeedManifestError> {
    let read_limit = limit.checked_add(1).ok_or_else(|| {
        SeedManifestError::resource_bound(
            ResourceBoundKind::ArithmeticOverflow,
            usize::MAX,
            usize::MAX,
        )
    })?;
    let capacity_u64 = known_length.min(read_limit);
    let capacity = usize::try_from(capacity_u64).map_err(|_| SeedManifestError::ResourceBound {
        kind: ResourceBoundKind::InputBytes,
        limit,
        observed_at_least: capacity_u64,
    })?;
    let mut raw = Vec::with_capacity(capacity);
    file.seek(SeekFrom::Start(0))
        .and_then(|_| file.take(read_limit).read_to_end(&mut raw))
        .map_err(|source| SeedManifestError::BoundedRead {
            path: path.to_path_buf(),
            source,
        })?;
    if u64::try_from(raw.len()).unwrap_or(u64::MAX) > limit {
        return Err(SeedManifestError::ResourceBound {
            kind: ResourceBoundKind::InputBytes,
            limit,
            observed_at_least: u64::try_from(raw.len()).unwrap_or(u64::MAX),
        });
    }
    Ok(raw)
}

#[derive(Debug)]
struct HandleFacts {
    identity: FileIdentity,
    is_directory: bool,
    is_regular_file: bool,
    is_reparse: bool,
    links: u64,
    length: u64,
    attributes: u32,
    creation_time: u64,
}

#[derive(Debug)]
pub(crate) struct PinnedDirectory {
    path: PathBuf,
    file: File,
    identity: FileIdentity,
}

impl PinnedDirectory {
    pub(crate) fn open(path: &Path) -> Result<Self, SeedManifestError> {
        let file = open_directory_no_follow(path).map_err(|source| {
            classify_open_error(path, source, "open directory without following links", true)
        })?;
        let facts = handle_facts(&file).map_err(|source| SeedManifestError::Io {
            operation: "inspect opened directory",
            path: path.to_path_buf(),
            source,
        })?;
        if !facts.is_directory || facts.is_reparse {
            return Err(SeedManifestError::UnsafePath {
                path: path.to_path_buf(),
                reason: "opened handle is not a real non-reparse directory".to_string(),
            });
        }
        Ok(Self {
            path: path.to_path_buf(),
            file,
            identity: facts.identity,
        })
    }

    pub(crate) fn revalidate(&self) -> Result<(), SeedManifestError> {
        self.revalidate_at(&self.path)
    }

    pub(crate) fn revalidate_at(&self, path: &Path) -> Result<(), SeedManifestError> {
        let reopened = Self::open(path)?;
        if reopened.identity != self.identity {
            return Err(SeedManifestError::UnsafePath {
                path: path.to_path_buf(),
                reason: "directory identity changed while the project gate was held".to_string(),
            });
        }
        Ok(())
    }
}

#[derive(Debug)]
struct OpenedRegularFile {
    path: PathBuf,
    file: File,
    identity: FileIdentity,
    links: u64,
    length: u64,
    attributes: u32,
    creation_time: u64,
}

impl OpenedRegularFile {
    fn from_file(path: &Path, file: File) -> Result<Self, SeedManifestError> {
        let facts = handle_facts(&file).map_err(|source| SeedManifestError::Io {
            operation: "inspect opened regular file",
            path: path.to_path_buf(),
            source,
        })?;
        if !facts.is_regular_file || facts.is_reparse {
            return Err(SeedManifestError::UnsafePath {
                path: path.to_path_buf(),
                reason: "opened handle is not a real non-reparse regular file".to_string(),
            });
        }
        Ok(Self {
            path: path.to_path_buf(),
            file,
            identity: facts.identity,
            links: facts.links,
            length: facts.length,
            attributes: facts.attributes,
            creation_time: facts.creation_time,
        })
    }

    fn open_existing(path: &Path, writable: bool) -> Result<Self, SeedManifestError> {
        let file = open_regular_no_follow(path, writable, false).map_err(|source| {
            classify_open_error(
                path,
                source,
                "open regular file without following links",
                false,
            )
        })?;
        Self::from_file(path, file)
    }

    fn from_lock_file(path: &Path, file: File) -> Result<Self, SeedManifestError> {
        let facts = handle_facts(&file).map_err(|source| SeedManifestError::LockIo {
            path: path.to_path_buf(),
            saw_contention: false,
            source,
        })?;
        if !facts.is_regular_file || facts.is_reparse {
            return Err(SeedManifestError::UnsafePath {
                path: path.to_path_buf(),
                reason: "opened lock handle is not a real non-reparse regular file".to_string(),
            });
        }
        Ok(Self {
            path: path.to_path_buf(),
            file,
            identity: facts.identity,
            links: facts.links,
            length: facts.length,
            attributes: facts.attributes,
            creation_time: facts.creation_time,
        })
    }

    fn open_existing_lock(path: &Path) -> Result<Self, SeedManifestError> {
        let file = open_regular_no_follow(path, true, false)
            .map_err(|source| classify_lock_open_error(path, source))?;
        Self::from_lock_file(path, file)
    }

    fn create_new(path: &Path) -> Result<Self, SeedManifestError> {
        let file = open_regular_no_follow(path, true, true).map_err(|source| {
            SeedManifestError::TempFile {
                operation: "create-new",
                path: path.to_path_buf(),
                source,
            }
        })?;
        Self::from_file(path, file)
    }

    fn refresh_facts(&mut self) -> Result<(), SeedManifestError> {
        let facts = handle_facts(&self.file).map_err(|source| SeedManifestError::Io {
            operation: "refresh opened regular-file identity",
            path: self.path.clone(),
            source,
        })?;
        if !facts.is_regular_file || facts.is_reparse || facts.identity != self.identity {
            return Err(SeedManifestError::UnsafePath {
                path: self.path.clone(),
                reason: "opened regular-file handle changed type or identity".to_string(),
            });
        }
        self.links = facts.links;
        self.length = facts.length;
        self.attributes = facts.attributes;
        self.creation_time = facts.creation_time;
        Ok(())
    }
}

fn classify_open_error(
    path: &Path,
    source: io::Error,
    operation: &'static str,
    expect_directory: bool,
) -> SeedManifestError {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || is_windows_reparse(&metadata) => {
            SeedManifestError::UnsafePath {
                path: path.to_path_buf(),
                reason: "path is a symlink, junction, or reparse point".to_string(),
            }
        }
        Ok(metadata)
            if (expect_directory && !metadata.is_dir())
                || (!expect_directory && !metadata.is_file()) =>
        {
            SeedManifestError::UnsafePath {
                path: path.to_path_buf(),
                reason: if expect_directory {
                    "path is not a real directory".to_string()
                } else {
                    "path is not a real regular file".to_string()
                },
            }
        }
        _ => SeedManifestError::Io {
            operation,
            path: path.to_path_buf(),
            source,
        },
    }
}

fn classify_lock_open_error(path: &Path, source: io::Error) -> SeedManifestError {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || is_windows_reparse(&metadata) => {
            SeedManifestError::UnsafePath {
                path: path.to_path_buf(),
                reason: "lock path is a symlink, junction, or reparse point".to_string(),
            }
        }
        Ok(metadata) if !metadata.is_file() => SeedManifestError::UnsafePath {
            path: path.to_path_buf(),
            reason: "lock path is not a real regular file".to_string(),
        },
        Ok(_) | Err(_) => SeedManifestError::LockIo {
            path: path.to_path_buf(),
            saw_contention: false,
            source,
        },
    }
}

fn is_exact_not_found(error: &io::Error) -> bool {
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_PATH_NOT_FOUND};

        error.raw_os_error().is_some_and(|raw| {
            u32::try_from(raw)
                .map(|raw| raw == ERROR_FILE_NOT_FOUND || raw == ERROR_PATH_NOT_FOUND)
                .unwrap_or(false)
        })
    }

    #[cfg(not(windows))]
    {
        error.kind() == io::ErrorKind::NotFound
    }
}

fn is_windows_reparse(metadata: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;

        metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }

    #[cfg(not(windows))]
    {
        let _ = metadata;
        false
    }
}

#[cfg(unix)]
fn open_directory_no_follow(path: &Path) -> io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;

    OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(path)
}

#[cfg(windows)]
fn open_directory_no_follow(path: &Path) -> io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE,
        FILE_SHARE_READ, FILE_SHARE_WRITE,
    };

    OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
}

#[cfg(not(any(unix, windows)))]
fn open_directory_no_follow(path: &Path) -> io::Result<File> {
    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "path is not a real directory",
        ));
    }
    File::open(path)
}

#[cfg(unix)]
fn open_regular_no_follow(path: &Path, writable: bool, create_new: bool) -> io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;

    let mut options = OpenOptions::new();
    options
        .read(true)
        .write(writable)
        .custom_flags(libc::O_NOFOLLOW);
    if create_new {
        options.create_new(true);
    }
    options.open(path)
}

#[cfg(windows)]
fn open_regular_no_follow(path: &Path, writable: bool, create_new: bool) -> io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    };

    let mut options = OpenOptions::new();
    options
        .read(true)
        .write(writable)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    if create_new {
        options.create_new(true);
    }
    options.open(path)
}

#[cfg(not(any(unix, windows)))]
fn open_regular_no_follow(path: &Path, writable: bool, create_new: bool) -> io::Result<File> {
    if !create_new {
        let metadata = std::fs::symlink_metadata(path)?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "path is not a real regular file",
            ));
        }
    }
    let mut options = OpenOptions::new();
    options
        .read(true)
        .write(writable)
        .create_new(create_new)
        .open(path)
}

#[cfg(unix)]
fn handle_facts(file: &File) -> io::Result<HandleFacts> {
    use std::os::unix::fs::MetadataExt;

    let metadata = file.metadata()?;
    Ok(HandleFacts {
        identity: FileIdentity {
            volume: metadata.dev(),
            file: u128::from(metadata.ino()),
        },
        is_directory: metadata.is_dir(),
        is_regular_file: metadata.is_file(),
        is_reparse: false,
        links: metadata.nlink(),
        length: metadata.len(),
        attributes: 0,
        creation_time: 0,
    })
}

#[cfg(windows)]
fn handle_facts(file: &File) -> io::Result<HandleFacts> {
    use std::mem::MaybeUninit;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_DIRECTORY,
        FILE_ATTRIBUTE_REPARSE_POINT,
    };

    let mut info = MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::zeroed();
    let success =
        unsafe { GetFileInformationByHandle(file.as_raw_handle() as HANDLE, info.as_mut_ptr()) };
    if success == 0 {
        return Err(io::Error::last_os_error());
    }
    let info = unsafe { info.assume_init() };
    let index = (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow);
    let length = (u64::from(info.nFileSizeHigh) << 32) | u64::from(info.nFileSizeLow);
    let creation_time = (u64::from(info.ftCreationTime.dwHighDateTime) << 32)
        | u64::from(info.ftCreationTime.dwLowDateTime);
    let is_directory = info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0;
    let is_reparse = info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0;
    Ok(HandleFacts {
        identity: FileIdentity {
            volume: u64::from(info.dwVolumeSerialNumber),
            file: u128::from(index),
        },
        is_directory,
        is_regular_file: !is_directory,
        is_reparse,
        links: u64::from(info.nNumberOfLinks),
        length,
        attributes: info.dwFileAttributes,
        creation_time,
    })
}

#[cfg(not(any(unix, windows)))]
fn handle_facts(file: &File) -> io::Result<HandleFacts> {
    let metadata = file.metadata()?;
    Ok(HandleFacts {
        identity: FileIdentity {
            volume: 0,
            file: u128::from(metadata.len()),
        },
        is_directory: metadata.is_dir(),
        is_regular_file: metadata.is_file(),
        is_reparse: false,
        links: 1,
        length: metadata.len(),
        attributes: 0,
        creation_time: 0,
    })
}

#[derive(Debug)]
pub(crate) struct PinnedOwnerChain {
    directories: Vec<PinnedDirectory>,
}

impl PinnedOwnerChain {
    pub(crate) fn revalidate(&self) -> Result<(), SeedManifestError> {
        for directory in &self.directories {
            directory.revalidate()?;
        }
        Ok(())
    }
}

enum CanonicalSnapshot {
    Writable {
        raw: Vec<u8>,
        state: ManifestState,
        canonical: Option<OpenedRegularFile>,
    },
    ReadOnly {
        reason: ManifestDegradedReason,
        canonical: Option<OpenedRegularFile>,
    },
}

impl fmt::Debug for CanonicalSnapshot {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Writable {
                raw,
                state,
                canonical,
            } => formatter
                .debug_struct("WritableCanonicalSnapshot")
                .field("raw_len", &raw.len())
                .field("row_count", &state.rows.len())
                .field("canonical_present", &canonical.is_some())
                .finish(),
            Self::ReadOnly { reason, canonical } => formatter
                .debug_struct("ReadOnlyCanonicalSnapshot")
                .field("reason", reason)
                .field("canonical_present", &canonical.is_some())
                .finish(),
        }
    }
}

impl CanonicalSnapshot {
    fn reason(&self) -> Option<ManifestDegradedReason> {
        match self {
            Self::Writable { .. } => None,
            Self::ReadOnly { reason, .. } => Some(*reason),
        }
    }

    fn has_valid_existing_canonical(&self) -> bool {
        matches!(
            self,
            Self::Writable {
                canonical: Some(_),
                ..
            }
        )
    }
}

#[derive(Debug, Default)]
struct TempInventory {
    exact_names: Vec<String>,
    exact_count: u64,
    malformed_count: u64,
    malformed_native_count: u64,
    exact_samples: Vec<String>,
    malformed_samples: Vec<String>,
    entry_errors: u64,
    scan_truncated: bool,
    scan_error: Option<String>,
}

#[cfg(test)]
type TestPathHook = std::sync::Arc<dyn Fn(&Path) + Send + Sync>;

#[cfg(test)]
type TestTempInventoryHook = std::sync::Arc<dyn Fn(&TempInventory) + Send + Sync>;

#[cfg(all(test, windows))]
type TestWindowsNamespaceHook = std::sync::Arc<
    dyn Fn(WindowsNamespaceOperation, &Path, &Path) -> Result<(), u32> + Send + Sync,
>;

#[cfg(test)]
#[derive(Clone, Default)]
struct TestFilesystemHooks {
    before_temp_validation: Option<TestPathHook>,
    before_raw_conflict_check: Option<TestPathHook>,
    on_prior_temp_diagnostic: Option<TestTempInventoryHook>,
    #[cfg(windows)]
    windows_namespace_call: Option<TestWindowsNamespaceHook>,
}

pub(crate) struct ProjectSeedManifestGuard {
    project: PinnedDirectory,
    ac_root: PinnedDirectory,
    lock_path: PathBuf,
    lock_file: File,
    lock_identity: FileIdentity,
    snapshot: CanonicalSnapshot,
    locked: bool,
    #[cfg(test)]
    hooks: TestFilesystemHooks,
}

impl fmt::Debug for ProjectSeedManifestGuard {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProjectSeedManifestGuard")
            .field("project", &self.project.path)
            .field("ac_root", &self.ac_root.path)
            .field("lock_path", &self.lock_path)
            .field("locked", &self.locked)
            .field("snapshot", &self.snapshot)
            .finish()
    }
}

impl ProjectSeedManifestGuard {
    pub(crate) fn acquire(project_root: &Path) -> Result<Self, SeedManifestError> {
        Self::acquire_with_timeout(project_root, DEFAULT_LOCK_TIMEOUT)
    }

    pub(crate) fn acquire_with_timeout(
        project_root: &Path,
        timeout: Duration,
    ) -> Result<Self, SeedManifestError> {
        #[cfg(test)]
        let hooks = TestFilesystemHooks::default();
        Self::acquire_inner(
            project_root,
            timeout,
            #[cfg(test)]
            hooks,
        )
    }

    #[cfg(test)]
    fn acquire_with_hooks(
        project_root: &Path,
        timeout: Duration,
        hooks: TestFilesystemHooks,
    ) -> Result<Self, SeedManifestError> {
        Self::acquire_inner(project_root, timeout, hooks)
    }

    fn acquire_inner(
        project_root: &Path,
        timeout: Duration,
        #[cfg(test)] hooks: TestFilesystemHooks,
    ) -> Result<Self, SeedManifestError> {
        let supplied_project = PinnedDirectory::open(project_root)?;
        let canonical_project_path =
            std::fs::canonicalize(project_root).map_err(|source| SeedManifestError::Io {
                operation: "canonicalize pinned project root",
                path: project_root.to_path_buf(),
                source,
            })?;
        let project = PinnedDirectory::open(&canonical_project_path)?;
        if project.identity != supplied_project.identity {
            return Err(SeedManifestError::UnsafePath {
                path: project_root.to_path_buf(),
                reason: "canonical project identity differs from the opened project handle"
                    .to_string(),
            });
        }

        let ac_path =
            canonical_project_path.join(crate::config::ac_root::CANONICAL_AC_ROOT_DIR);
        let ac_root = PinnedDirectory::open(&ac_path)?;
        let lock_path = ac_path.join(SEED_MANIFEST_LOCK_FILENAME);
        let started = Instant::now();
        let deadline = started.checked_add(timeout).unwrap_or(started);
        let lock_file = open_or_create_lock_file(&lock_path, deadline)?;
        let lock_identity = handle_facts(&lock_file)
            .map_err(|source| SeedManifestError::LockIo {
                path: lock_path.clone(),
                saw_contention: false,
                source,
            })?
            .identity;
        acquire_kernel_lock(&lock_file, &lock_path, started, deadline)?;

        let mut guard = Self {
            project,
            ac_root,
            lock_path,
            lock_file,
            lock_identity,
            snapshot: CanonicalSnapshot::Writable {
                raw: Vec::new(),
                state: ManifestState::default(),
                canonical: None,
            },
            locked: true,
            #[cfg(test)]
            hooks,
        };
        guard.revalidate_owner()?;
        let inventory = guard.inventory_prior_temps();
        match guard.load_canonical_snapshot() {
            Ok(snapshot) => {
                let valid_canonical = snapshot.has_valid_existing_canonical();
                guard.snapshot = snapshot;
                guard.process_prior_temp_inventory(inventory, valid_canonical);
            }
            Err(error) => {
                guard.process_prior_temp_inventory(inventory, false);
                return Err(error);
            }
        }
        // #1318 - one-shot v1 -> v2 coverage upgrade. Runs after the prior-temp
        // policy evaluated the PRE-migration state, and its own temp (created
        // and renamed inside `write_canonical`, cleaned on failure) never
        // interacts with that inventory. After a successful upgrade the strict
        // parse succeeds, so the next acquire never re-enters the migration.
        guard.try_upgrade_v1_to_v2();
        Ok(guard)
    }

    pub(crate) fn ac_root(&self) -> &Path {
        &self.ac_root.path
    }

    pub(crate) fn revalidate_owner(&self) -> Result<(), SeedManifestError> {
        self.project.revalidate()?;
        self.ac_root.revalidate()?;
        let reopened = OpenedRegularFile::open_existing_lock(&self.lock_path)?;
        if reopened.identity != self.lock_identity {
            return Err(SeedManifestError::UnsafePath {
                path: self.lock_path.clone(),
                reason: "lock-file identity changed while its original handle remained locked"
                    .to_string(),
            });
        }
        Ok(())
    }

    pub(crate) fn pin_owner_chain_from_ac(
        &self,
        relative: &Path,
    ) -> Result<PinnedOwnerChain, SeedManifestError> {
        if relative.is_absolute() {
            return Err(SeedManifestError::Validation(format!(
                "owner chain must be relative to .ac: {}",
                relative.display()
            )));
        }
        self.revalidate_owner()?;
        let mut path = self.ac_root.path.clone();
        let mut directories = Vec::new();
        for component in relative.components() {
            let Component::Normal(component) = component else {
                return Err(SeedManifestError::Validation(format!(
                    "owner chain contains a non-normal component: {}",
                    relative.display()
                )));
            };
            path.push(component);
            directories.push(PinnedDirectory::open(&path)?);
        }
        let chain = PinnedOwnerChain { directories };
        self.revalidate_owner()?;
        chain.revalidate()?;
        Ok(chain)
    }

    pub(crate) fn publication_permit(&mut self) -> ProjectPublicationPermit<'_> {
        if let Some(reason) = self.snapshot.reason() {
            ProjectPublicationPermit {
                state: ProjectPublicationPermitState::DegradedUntracked {
                    held_guard: Some(self),
                    reason,
                },
            }
        } else {
            ProjectPublicationPermit {
                state: ProjectPublicationPermitState::Tracked(self),
            }
        }
    }

    pub(crate) fn release(mut self) {
        self.unlock_explicitly();
    }

    fn unlock_explicitly(&mut self) {
        if !self.locked {
            return;
        }
        if let Err(error) = File::unlock(&self.lock_file) {
            log::warn!(
                "[seed_manifest] explicit unlock failed path={} error={}",
                self.lock_path.display(),
                error
            );
        }
        self.locked = false;
    }

    fn load_canonical_snapshot(&self) -> Result<CanonicalSnapshot, SeedManifestError> {
        let canonical_path = self.ac_root.path.join(SEED_MANIFEST_FILENAME);
        let mut canonical = match open_regular_no_follow(&canonical_path, false, false) {
            Ok(file) => OpenedRegularFile::from_file(&canonical_path, file)?,
            Err(error) if is_exact_not_found(&error) => {
                return Ok(CanonicalSnapshot::Writable {
                    raw: Vec::new(),
                    state: ManifestState::default(),
                    canonical: None,
                });
            }
            Err(error) => {
                return Err(classify_open_error(
                    &canonical_path,
                    error,
                    "open canonical manifest",
                    false,
                ));
            }
        };

        #[cfg(windows)]
        {
            use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_READONLY;

            if canonical.attributes & FILE_ATTRIBUTE_READONLY != 0 {
                log::warn!(
                    "[seed_manifest] canonical manifest is read-only and was preserved path={}",
                    canonical_path.display()
                );
                return Ok(CanonicalSnapshot::ReadOnly {
                    reason: ManifestDegradedReason::ReadOnlyCanonical,
                    canonical: Some(canonical),
                });
            }
        }

        let raw = match read_bounded_file(
            &mut canonical.file,
            canonical.length,
            MAX_MANIFEST_BYTES,
            &canonical_path,
        ) {
            Ok(raw) => raw,
            Err(SeedManifestError::ResourceBound { .. }) => {
                return Ok(CanonicalSnapshot::ReadOnly {
                    reason: ManifestDegradedReason::ResourceBound(ResourceBoundKind::InputBytes),
                    canonical: Some(canonical),
                });
            }
            Err(error) => return Err(error),
        };
        let reopened = OpenedRegularFile::open_existing(&canonical_path, false)?;
        if reopened.identity != canonical.identity {
            return Err(SeedManifestError::UnsafePath {
                path: canonical_path,
                reason: "canonical identity changed during its bounded read".to_string(),
            });
        }
        match parse_manifest_bytes(&raw) {
            Ok(state) => Ok(CanonicalSnapshot::Writable {
                raw,
                state,
                canonical: Some(canonical),
            }),
            Err(error) => {
                let reason = error.degraded_reason();
                log::warn!(
                    "[seed_manifest] canonical manifest is preserved read-only path={} reason={:?} diagnostic_kind={}",
                    canonical_path.display(),
                    reason,
                    error.diagnostic_kind()
                );
                Ok(CanonicalSnapshot::ReadOnly {
                    reason,
                    canonical: Some(canonical),
                })
            }
        }
    }

    /// #1318 - one-shot v1 -> v2 coverage upgrade, run under the held lock on
    /// the first acquire of an exact-shape v1 manifest. Deterministic and
    /// lossless: the v2 state is built by SUBSTITUTION from the parsed v1 wire
    /// (`coverage_version` and the coverage list only), so every existing strict
    /// row check and bound runs verbatim and no timestamp is invented. The
    /// atomic write goes through [`Self::write_canonical`] DIRECTLY (not
    /// `persist_current_state`, whose `Unchanged` fast path and `Recorded`
    /// classification are publication semantics, not migration semantics):
    /// `verify_canonical_unchanged` stream-compares the CURRENT disk file
    /// against the retained v1 bytes under the lock, so a concurrent external
    /// edit between the re-read and the write fails the raw-conflict check.
    ///
    /// Only a `ReadOnly { reason: InvalidCanonical, canonical: Some(..) }`
    /// snapshot is migratable; every other degraded reason (ResourceBound,
    /// FutureSchema, ExternalEdit, ReadOnlyCanonical, UnsafePath,
    /// PersistenceFailure) is preserved as-is. Detection keys ONLY on the wire
    /// pre-parse, never on the sanitized message string.
    fn try_upgrade_v1_to_v2(&mut self) {
        let snapshot = std::mem::replace(
            &mut self.snapshot,
            CanonicalSnapshot::ReadOnly {
                reason: ManifestDegradedReason::InvalidCanonical,
                canonical: None,
            },
        );
        let mut canonical = match snapshot {
            CanonicalSnapshot::ReadOnly {
                reason: ManifestDegradedReason::InvalidCanonical,
                canonical: Some(canonical),
            } => canonical,
            other => {
                self.snapshot = other;
                return;
            }
        };
        let canonical_path = canonical.path.clone();
        let restore = |guard: &mut Self, canonical: OpenedRegularFile| {
            guard.snapshot = CanonicalSnapshot::ReadOnly {
                reason: ManifestDegradedReason::InvalidCanonical,
                canonical: Some(canonical),
            };
        };

        // b. Re-read the raw bytes from the retained handle (the ReadOnly
        // snapshot holds no raw copy; the handle stays valid under the lock).
        let raw = match read_bounded_file(
            &mut canonical.file,
            canonical.length,
            MAX_MANIFEST_BYTES,
            &canonical_path,
        ) {
            Ok(raw) => raw,
            Err(error) => {
                log::warn!(
                    "[seed_manifest] v1->v2 upgrade re-read failed path={} error={}; manifest stays preserved read-only",
                    canonical_path.display(),
                    error
                );
                restore(self, canonical);
                return;
            }
        };

        // c. Lenient pre-parse. The wire deserializers enforce the MAX_* bounds,
        // so a bound-violating v1 file fails here and stays preserved (the v2
        // strict parse would reject the same rows).
        let text = match std::str::from_utf8(&raw) {
            Ok(text) => text,
            Err(error) => {
                log::warn!(
                    "[seed_manifest] v1->v2 upgrade skipped: manifest is not UTF-8 at byte offset {} path={}; manifest stays preserved read-only",
                    error.valid_up_to(),
                    canonical_path.display()
                );
                restore(self, canonical);
                return;
            }
        };
        let wire = match toml::from_str::<SeedManifestWire>(text) {
            Ok(wire) => wire,
            Err(error) => {
                log::warn!(
                    "[seed_manifest] v1->v2 upgrade skipped path={} error={}; manifest stays preserved read-only",
                    canonical_path.display(),
                    classify_toml_parse_error(error)
                );
                restore(self, canonical);
                return;
            }
        };

        // Exact v1 shape: schema 1, coverage 1, the exact two-item list in that
        // order, nothing else. Any other shape (including `coverage_version = 2`
        // with the old list, or a v1 file with a different list) keeps the
        // strict-invalid handling unchanged.
        const V1_COVERAGE: [&str; 2] = ["project_context_templates", "replica_config_folders"];
        if wire.schema_version != 1
            || wire.coverage_version != 1
            || wire.coverage != V1_COVERAGE.map(str::to_string).to_vec()
        {
            restore(self, canonical);
            return;
        }

        // d. Build the v2 state by SUBSTITUTION and run the existing strict
        // checks verbatim (per-row validate_row, duplicate-identity rejection,
        // mixed-source/timestamp batch checks, MAX_* bounds).
        let mut v2 = wire;
        v2.coverage_version = COVERAGE_VERSION;
        v2.coverage = COVERAGE.map(str::to_string).to_vec();
        let state = match ManifestState::from_wire(v2) {
            Ok(state) => state,
            Err(error) => {
                log::warn!(
                    "[seed_manifest] v1 manifest rows failed v2 validation path={} error={}; manifest stays preserved read-only",
                    canonical_path.display(),
                    sanitize_canonical_validation_error(error)
                );
                restore(self, canonical);
                return;
            }
        };

        // e. Serialize and write under the held lock. `write_canonical` updates
        // the snapshot itself on success; on failure its temp is cleaned and the
        // pre-migration degraded snapshot is restored (bytes preserved exactly,
        // writer disabled, published-unrecorded).
        let bytes = match serialize_state(&state) {
            Ok(bytes) => bytes,
            Err(error) => {
                log::warn!(
                    "[seed_manifest] v1->v2 upgrade serialize failed path={} error={}; manifest stays preserved read-only",
                    canonical_path.display(),
                    error
                );
                restore(self, canonical);
                return;
            }
        };
        self.snapshot = CanonicalSnapshot::Writable {
            raw,
            state,
            canonical: Some(canonical),
        };
        if let Err(error) = self.write_canonical(bytes) {
            log::warn!(
                "[seed_manifest] v1->v2 upgrade write failed path={} error={}; manifest stays preserved read-only",
                canonical_path.display(),
                error
            );
            let canonical = match std::mem::replace(
                &mut self.snapshot,
                CanonicalSnapshot::ReadOnly {
                    reason: ManifestDegradedReason::InvalidCanonical,
                    canonical: None,
                },
            ) {
                CanonicalSnapshot::Writable { canonical, .. } => canonical,
                other => {
                    // Unreachable: write_canonical leaves the snapshot untouched
                    // on error, so it is still the Writable we just installed.
                    self.snapshot = other;
                    None
                }
            };
            self.snapshot = CanonicalSnapshot::ReadOnly {
                reason: ManifestDegradedReason::InvalidCanonical,
                canonical,
            };
        }
    }

    fn inventory_prior_temps(&self) -> TempInventory {
        let mut inventory = TempInventory::default();
        let mut entries = match std::fs::read_dir(&self.ac_root.path) {
            Ok(entries) => entries,
            Err(error) => {
                inventory.scan_truncated = true;
                inventory.scan_error = Some(error.to_string());
                return inventory;
            }
        };

        for _ in 0..MAX_TEMP_INVENTORY_ENTRIES {
            let Some(entry) = entries.next() else {
                return inventory;
            };
            let entry = match entry {
                Ok(entry) => entry,
                Err(_) => {
                    inventory.entry_errors = inventory.entry_errors.saturating_add(1);
                    continue;
                }
            };
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                if native_name_has_temp_shape(&name) {
                    inventory.malformed_count = inventory.malformed_count.saturating_add(1);
                    inventory.malformed_native_count =
                        inventory.malformed_native_count.saturating_add(1);
                    if inventory.malformed_samples.len() < MAX_TEMP_DIAGNOSTIC_SAMPLES {
                        inventory
                            .malformed_samples
                            .push("<native-non-unicode-temp-name>".to_string());
                    }
                }
                continue;
            };
            if is_exact_temp_name(name) {
                inventory.exact_count = inventory.exact_count.saturating_add(1);
                if inventory.exact_samples.len() < MAX_TEMP_DIAGNOSTIC_SAMPLES {
                    inventory.exact_samples.push(name.to_string());
                }
                inventory.exact_names.push(name.to_string());
            } else if name.starts_with(SEED_MANIFEST_TEMP_PREFIX)
                && name.ends_with(SEED_MANIFEST_TEMP_SUFFIX)
            {
                inventory.malformed_count = inventory.malformed_count.saturating_add(1);
                if inventory.malformed_samples.len() < MAX_TEMP_DIAGNOSTIC_SAMPLES {
                    inventory.malformed_samples.push(name.to_string());
                }
            }
        }
        inventory.scan_truncated = true;
        inventory
    }

    fn process_prior_temp_inventory(&self, inventory: TempInventory, valid_canonical: bool) {
        #[cfg(not(unix))]
        let _ = valid_canonical;
        #[cfg(unix)]
        let mut removed = 0_u64;
        #[cfg(unix)]
        let mut preserved = inventory.exact_count;
        #[cfg(not(unix))]
        let removed = 0_u64;
        #[cfg(not(unix))]
        let preserved = inventory.exact_count;

        #[cfg(unix)]
        if valid_canonical
            && !inventory.scan_truncated
            && inventory.scan_error.is_none()
            && inventory.entry_errors == 0
        {
            for name in &inventory.exact_names {
                let path = self.ac_root.path.join(name);
                if remove_prior_unix_temp_if_safe(&path).is_ok() {
                    removed = removed.saturating_add(1);
                    preserved = preserved.saturating_sub(1);
                }
            }
        }

        if inventory.exact_count == 0
            && inventory.malformed_count == 0
            && inventory.entry_errors == 0
            && !inventory.scan_truncated
            && inventory.scan_error.is_none()
        {
            return;
        }

        #[cfg(windows)]
        let policy = "WindowsPriorTempRecoveryCandidate";
        #[cfg(unix)]
        let policy = if valid_canonical
            && !inventory.scan_truncated
            && inventory.scan_error.is_none()
            && inventory.entry_errors == 0
        {
            "UnixValidCanonicalIdentityProof"
        } else {
            "UnixPreserveAmbiguousPriorTemp"
        };
        #[cfg(not(any(unix, windows)))]
        let policy = "UnsupportedPlatformPreservePriorTemp";

        #[cfg(test)]
        if let Some(hook) = &self.hooks.on_prior_temp_diagnostic {
            hook(&inventory);
        }

        log::warn!(
            "[seed_manifest] bounded prior-temp inventory root={} policy={} exact={} malformed={} malformed_native={} removed={} preserved={} entry_errors={} scan_truncated={} scan_error={:?} exact_samples={:?} malformed_samples={:?}",
            self.ac_root.path.display(),
            policy,
            inventory.exact_count,
            inventory.malformed_count,
            inventory.malformed_native_count,
            removed,
            preserved,
            inventory.entry_errors,
            inventory.scan_truncated,
            inventory.scan_error,
            inventory.exact_samples,
            inventory.malformed_samples
        );
    }

    fn persist_current_state(&mut self) -> Result<ManifestRecordOutcome, SeedManifestError> {
        let (bytes, current_raw, empty_absent) = match &self.snapshot {
            CanonicalSnapshot::Writable {
                raw,
                state,
                canonical,
            } => (
                serialize_state(state)?,
                raw.as_slice(),
                state.rows.is_empty() && canonical.is_none(),
            ),
            CanonicalSnapshot::ReadOnly { reason, .. } => {
                return Ok(ManifestRecordOutcome::PublishedUnrecorded(*reason));
            }
        };
        if empty_absent || bytes == current_raw {
            return Ok(ManifestRecordOutcome::Unchanged);
        }
        self.write_canonical(bytes)?;
        Ok(ManifestRecordOutcome::Recorded)
    }

    fn write_canonical(&mut self, bytes: Vec<u8>) -> Result<(), SeedManifestError> {
        self.revalidate_owner()?;
        let canonical_path = self.ac_root.path.join(SEED_MANIFEST_FILENAME);
        let temp_path = self.unique_temp_path();
        let mut temp = OpenedRegularFile::create_new(&temp_path)?;
        let write_result = temp
            .file
            .write_all(&bytes)
            .and_then(|_| temp.file.flush())
            .and_then(|_| temp.file.sync_all());
        if let Err(source) = write_result {
            self.cleanup_owned_temp(&temp, None);
            return Err(SeedManifestError::TempFile {
                operation: "write-flush-sync",
                path: temp_path,
                source,
            });
        }
        if let Err(error) = temp.refresh_facts() {
            self.cleanup_owned_temp(&temp, None);
            return Err(error);
        }

        #[cfg(test)]
        if let Some(hook) = &self.hooks.before_temp_validation {
            hook(&temp_path);
        }

        if let Err(error) = verify_temp_path(&temp_path, &temp, &bytes) {
            self.cleanup_owned_temp(&temp, Some(&bytes));
            return Err(error);
        }
        if let Err(error) = self.revalidate_owner() {
            self.cleanup_owned_temp(&temp, Some(&bytes));
            return Err(error);
        }

        #[cfg(test)]
        if let Some(hook) = &self.hooks.before_raw_conflict_check {
            hook(&canonical_path);
        }

        if let Err(error) = self.verify_canonical_unchanged(&canonical_path) {
            self.cleanup_owned_temp(&temp, Some(&bytes));
            return Err(error);
        }

        #[cfg(windows)]
        let published = self.publish_windows_temp(temp, &canonical_path, &bytes)?;

        #[cfg(not(windows))]
        let published = {
            let had_canonical = matches!(
                self.snapshot,
                CanonicalSnapshot::Writable {
                    canonical: Some(_),
                    ..
                }
            );
            if let Err(error) = atomic_publish_manifest(&temp_path, &canonical_path, had_canonical)
            {
                self.cleanup_owned_temp(&temp, Some(&bytes));
                return Err(error);
            }

            #[cfg(unix)]
            if let Err(error) = self.ac_root.file.sync_all() {
                log::warn!(
                    "[seed_manifest] parent-directory sync failed after canonical replace root={} error={}",
                    self.ac_root.path.display(),
                    error
                );
            }

            temp.path = canonical_path.clone();
            temp.length = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
            temp
        };
        match &mut self.snapshot {
            CanonicalSnapshot::Writable { raw, canonical, .. } => {
                *raw = bytes;
                *canonical = Some(published);
            }
            CanonicalSnapshot::ReadOnly { .. } => {
                return Err(SeedManifestError::Validation(
                    "read-only manifest unexpectedly reached the atomic writer".to_string(),
                ));
            }
        }
        Ok(())
    }

    fn verify_canonical_unchanged(&self, canonical_path: &Path) -> Result<(), SeedManifestError> {
        let (expected_raw, expected_identity) = match &self.snapshot {
            CanonicalSnapshot::Writable { raw, canonical, .. } => {
                if let Some(canonical) = canonical {
                    let facts =
                        handle_facts(&canonical.file).map_err(|source| SeedManifestError::Io {
                            operation: "reinspect retained canonical handle",
                            path: canonical_path.to_path_buf(),
                            source,
                        })?;
                    if facts.identity != canonical.identity
                        || !facts.is_regular_file
                        || facts.is_reparse
                    {
                        return Err(SeedManifestError::ExternalEditConflict {
                            path: canonical_path.to_path_buf(),
                        });
                    }
                }
                (raw.as_slice(), canonical.as_ref().map(|file| file.identity))
            }
            CanonicalSnapshot::ReadOnly { .. } => {
                return Err(SeedManifestError::Validation(
                    "read-only manifest cannot perform a raw conflict check".to_string(),
                ));
            }
        };

        match open_regular_no_follow(canonical_path, false, false) {
            Ok(file) => {
                let mut reopened = OpenedRegularFile::from_file(canonical_path, file)?;
                if expected_identity != Some(reopened.identity)
                    || !stream_matches(&mut reopened.file, expected_raw).map_err(|source| {
                        SeedManifestError::Io {
                            operation: "stream canonical conflict comparison",
                            path: canonical_path.to_path_buf(),
                            source,
                        }
                    })?
                {
                    return Err(SeedManifestError::ExternalEditConflict {
                        path: canonical_path.to_path_buf(),
                    });
                }
                Ok(())
            }
            Err(error) if is_exact_not_found(&error) && expected_identity.is_none() => Ok(()),
            Err(error) if is_exact_not_found(&error) => {
                Err(SeedManifestError::ExternalEditConflict {
                    path: canonical_path.to_path_buf(),
                })
            }
            Err(error) => Err(classify_open_error(
                canonical_path,
                error,
                "reopen canonical manifest for raw conflict comparison",
                false,
            )),
        }
    }

    fn unique_temp_path(&self) -> PathBuf {
        self.ac_root.path.join(format!(
            "{SEED_MANIFEST_TEMP_PREFIX}{}{SEED_MANIFEST_TEMP_SUFFIX}",
            Uuid::new_v4().hyphenated()
        ))
    }

    fn cleanup_owned_temp(&self, temp: &OpenedRegularFile, expected_bytes: Option<&[u8]>) {
        if let Err(error) = cleanup_current_owned_temp(temp, expected_bytes) {
            log::warn!(
                "[seed_manifest] owned temp cleanup left path={} error={}",
                temp.path.display(),
                error
            );
        }
    }
}

impl Drop for ProjectSeedManifestGuard {
    fn drop(&mut self) {
        self.unlock_explicitly();
    }
}

fn open_or_create_lock_file(path: &Path, deadline: Instant) -> Result<File, SeedManifestError> {
    loop {
        match open_regular_no_follow(path, true, false) {
            Ok(file) => {
                let opened = OpenedRegularFile::from_lock_file(path, file)?;
                return Ok(opened.file);
            }
            Err(error) if is_exact_not_found(&error) => {
                match open_regular_no_follow(path, true, true) {
                    Ok(file) => {
                        let opened = OpenedRegularFile::from_lock_file(path, file)?;
                        return Ok(opened.file);
                    }
                    Err(create_error) if create_error.kind() == io::ErrorKind::AlreadyExists => {}
                    Err(create_error) => {
                        return Err(classify_lock_open_error(path, create_error));
                    }
                }
            }
            Err(error) => {
                return Err(classify_lock_open_error(path, error));
            }
        }
        if Instant::now() >= deadline {
            return Err(SeedManifestError::LockIo {
                path: path.to_path_buf(),
                saw_contention: false,
                source: io::Error::new(
                    io::ErrorKind::TimedOut,
                    "lock-file create/open race did not stabilize before the deadline",
                ),
            });
        }
    }
}

fn acquire_kernel_lock(
    file: &File,
    path: &Path,
    started: Instant,
    deadline: Instant,
) -> Result<(), SeedManifestError> {
    let mut saw_contention = false;
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(()),
            Err(std::fs::TryLockError::WouldBlock) => {
                saw_contention = true;
            }
            Err(std::fs::TryLockError::Error(source)) => {
                return Err(SeedManifestError::LockIo {
                    path: path.to_path_buf(),
                    saw_contention,
                    source,
                });
            }
        }
        let now = Instant::now();
        if now >= deadline {
            return Err(SeedManifestError::BusyTimeout {
                path: path.to_path_buf(),
                waited: started.elapsed(),
            });
        }
        let remaining = deadline.saturating_duration_since(now);
        std::thread::sleep(remaining.min(MAX_LOCK_POLL));
    }
}

fn is_exact_temp_name(name: &str) -> bool {
    let Some(uuid_text) = name
        .strip_prefix(SEED_MANIFEST_TEMP_PREFIX)
        .and_then(|name| name.strip_suffix(SEED_MANIFEST_TEMP_SUFFIX))
    else {
        return false;
    };
    Uuid::parse_str(uuid_text)
        .map(|uuid| uuid.hyphenated().to_string() == uuid_text)
        .unwrap_or(false)
}

#[cfg(unix)]
fn native_name_has_temp_shape(name: &std::ffi::OsStr) -> bool {
    use std::os::unix::ffi::OsStrExt;

    let bytes = name.as_bytes();
    bytes.starts_with(SEED_MANIFEST_TEMP_PREFIX.as_bytes())
        && bytes.ends_with(SEED_MANIFEST_TEMP_SUFFIX.as_bytes())
}

#[cfg(windows)]
fn native_name_has_temp_shape(name: &std::ffi::OsStr) -> bool {
    use std::os::windows::ffi::OsStrExt;

    let units = name.encode_wide().collect::<Vec<_>>();
    let prefix = SEED_MANIFEST_TEMP_PREFIX.encode_utf16().collect::<Vec<_>>();
    let suffix = SEED_MANIFEST_TEMP_SUFFIX.encode_utf16().collect::<Vec<_>>();
    units.starts_with(&prefix) && units.ends_with(&suffix)
}

#[cfg(not(any(unix, windows)))]
fn native_name_has_temp_shape(name: &std::ffi::OsStr) -> bool {
    name.to_str().is_some_and(|name| {
        name.starts_with(SEED_MANIFEST_TEMP_PREFIX) && name.ends_with(SEED_MANIFEST_TEMP_SUFFIX)
    })
}

fn remove_if_same_identity(path: &Path, expected: FileIdentity) -> Result<(), SeedManifestError> {
    let reopened = OpenedRegularFile::open_existing(path, false)?;
    if reopened.identity != expected || reopened.links != 1 {
        return Err(SeedManifestError::UnsafePath {
            path: path.to_path_buf(),
            reason: "temp identity changed or acquired another hard link before removal"
                .to_string(),
        });
    }
    std::fs::remove_file(path).map_err(|source| SeedManifestError::TempFile {
        operation: "remove unchanged temp",
        path: path.to_path_buf(),
        source,
    })
}

#[cfg(not(windows))]
fn cleanup_current_owned_temp(
    temp: &OpenedRegularFile,
    _expected_bytes: Option<&[u8]>,
) -> Result<(), SeedManifestError> {
    remove_if_same_identity(&temp.path, temp.identity)
}

#[cfg(unix)]
fn remove_prior_unix_temp_if_safe(path: &Path) -> Result<(), SeedManifestError> {
    let first = OpenedRegularFile::open_existing(path, false)?;
    if first.links != 1 {
        return Err(SeedManifestError::UnsafePath {
            path: path.to_path_buf(),
            reason: "prior temp has more than one hard link".to_string(),
        });
    }
    let second = OpenedRegularFile::open_existing(path, false)?;
    if second.identity != first.identity || second.links != 1 {
        return Err(SeedManifestError::UnsafePath {
            path: path.to_path_buf(),
            reason: "prior temp path identity changed during cleanup proof".to_string(),
        });
    }
    std::fs::remove_file(path).map_err(|source| SeedManifestError::TempFile {
        operation: "remove proved stale Unix temp",
        path: path.to_path_buf(),
        source,
    })
}

fn verify_temp_path(
    path: &Path,
    expected: &OpenedRegularFile,
    bytes: &[u8],
) -> Result<(), SeedManifestError> {
    let mut reopened = OpenedRegularFile::open_existing(path, false)?;
    if reopened.identity != expected.identity || reopened.links != 1 {
        return Err(SeedManifestError::UnsafePath {
            path: path.to_path_buf(),
            reason: "temp identity changed or has more than one hard link".to_string(),
        });
    }
    if reopened.length != u64::try_from(bytes.len()).unwrap_or(u64::MAX)
        || !stream_matches(&mut reopened.file, bytes).map_err(|source| {
            SeedManifestError::TempFile {
                operation: "verify temp bytes",
                path: path.to_path_buf(),
                source,
            }
        })?
    {
        return Err(SeedManifestError::UnsafePath {
            path: path.to_path_buf(),
            reason: "temp length or bytes changed before atomic publication".to_string(),
        });
    }
    Ok(())
}

fn stream_matches(file: &mut File, expected: &[u8]) -> io::Result<bool> {
    file.seek(SeekFrom::Start(0))?;
    let mut offset = 0_usize;
    let mut buffer = [0_u8; RAW_COMPARE_BUFFER_BYTES];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            return Ok(offset == expected.len());
        }
        let Some(end) = offset.checked_add(read) else {
            return Ok(false);
        };
        if end > expected.len() || buffer[..read] != expected[offset..end] {
            return Ok(false);
        }
        offset = end;
    }
}

#[cfg(unix)]
fn atomic_publish_manifest(
    temp: &Path,
    canonical: &Path,
    _had_canonical: bool,
) -> Result<(), SeedManifestError> {
    std::fs::rename(temp, canonical).map_err(|source| SeedManifestError::AtomicReplace {
        operation: "rename",
        temp: temp.to_path_buf(),
        canonical: canonical.to_path_buf(),
        source,
    })
}

/// #1065 Stage F: soft project-gate acquisition for the automatic, fail-soft
/// context and config publishers (discovery scan, session read/self-heal,
/// explicit overwrite).
///
/// It distinguishes the three frozen degraded cases in plan sections 5.2/6.1:
/// * `Held`: the gate is owned; publish and record under it.
/// * `DegradedUntracked`: a pre-contention lock capability/I-O error left no
///   guard; the target may still publish, but unrecorded.
/// * `Unavailable`: a contention timeout, post-contention lock I/O error, or an
///   unsafe/reparse/identity path; skip the operation rather than race a
///   cooperating writer or follow a substituted path.
///
/// The classification mirrors `commands::entity_creation::acquire_lifecycle_project_gate`
/// (which keeps its `Result<Option<_>, String>` shape for the error-returning
/// deletion contract); this enum form suits the fail-soft automatic publishers.
// The `Held` variant carries the full guard, so the enum is guard-sized. This is a
// transient return value that every caller matches and destructures immediately, so
// the disparity is a single stack move (identical in cost to returning the guard
// itself); boxing it would only add an allocation on the per-spawn gate-acquire path.
#[allow(clippy::large_enum_variant)]
pub(crate) enum SoftProjectGate {
    Held(ProjectSeedManifestGuard),
    DegradedUntracked,
    Unavailable(SeedManifestError),
}

pub(crate) fn acquire_project_gate_soft(project_root: &Path) -> SoftProjectGate {
    match ProjectSeedManifestGuard::acquire(project_root) {
        Ok(guard) => SoftProjectGate::Held(guard),
        Err(
            error @ (SeedManifestError::UnsafePath { .. }
            | SeedManifestError::BusyTimeout { .. }
            | SeedManifestError::LockIo {
                saw_contention: true,
                ..
            }),
        ) => SoftProjectGate::Unavailable(error),
        // Pre-contention capability or other lock-acquisition error: preserve the
        // existing ungated behavior as untracked degradation.
        Err(_) => SoftProjectGate::DegradedUntracked,
    }
}

#[derive(Debug)]
pub(crate) struct ManifestActivationToken {
    _private: (),
}

impl ManifestActivationToken {
    #[cfg(test)]
    pub(crate) fn for_test() -> Self {
        Self { _private: () }
    }

    /// Stage F: the sole production activation constructor.
    ///
    /// Constructing this token is what activates v1 seed-manifest emission. Every
    /// production publisher and lifecycle hook enumerated in
    /// [`V1_COVERAGE_BOUNDARIES`] threads the resulting token into the recorder;
    /// there is deliberately no other non-test way to obtain one, so a build that
    /// never calls this constructor cannot mutate a manifest. The struct's private
    /// unit field keeps this the only construction path.
    pub(crate) fn production() -> Self {
        Self { _private: () }
    }

    fn authorize(&self) {}
}

/// Exhaustive Stage F activation coverage (plan section 9 item 6, acceptance
/// item 22).
///
/// Every production publisher and lifecycle hook that threads a real
/// [`ManifestActivationToken`] is named here exactly once, next to the module
/// and adapter that wires it. This declaration is the compile-time checklist; it
/// is NOT by itself wiring evidence. Real-boundary coverage in
/// `tests/seed_manifest_activation.rs` and the CLI integration suites reds if
/// any variant's production `ManifestActivationToken::production()` threading (or
/// its recording adapter) is removed, so a declaration cannot silently stay green
/// after its adapter call was removed (which would be insufficient).
///
/// #1318 - [`V1CoverageBoundary::CatalogSeed`] is the first coverage-v2-era
/// publisher; the enum keeps its historical name (minimal diff) while the
/// declaration remains the exhaustive production-publisher checklist across
/// coverage versions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum V1CoverageBoundary {
    /// `commands::ac_discovery::create_ac_project` fresh-root context creation.
    DirectCreateAcProjectFreshRoot,
    /// `config::seeded_context_templates` create-missing project context (also the
    /// `config::projects` new-project registration re-ensure path).
    ContextCreate,
    /// `config::seeded_context_templates` recognized-update project context.
    ContextUpdate,
    /// `config::session_context::heal_stale_global_context_template` self-heal.
    ContextSelfHeal,
    /// `config::seeded_context_templates::overwrite_context_template_with_default`.
    ContextOverwrite,
    /// Coordinator pristine v2 -> v4 recognized update.
    CoordinatorStatelessV2ToV4,
    /// Coordinator pristine v3 -> v4 recognized update.
    CoordinatorStatelessV3ToV4,
    /// Coordinator seeded v3 -> v4 recognized update with state-version bump.
    CoordinatorSeededV3ToV4,
    /// `config::config_seed` exact replica config publish (whole-scope replace).
    ConfigExactPublish,
    /// `config::config_seed` over-bound replica config publish (scope removal only).
    ConfigOverBoundPublish,
    /// `config::config_seed` `FailedAfterLogicalRemoval` prior-scope removal.
    ConfigFailedRestore,
    /// `commands::entity_creation`/`cli::team` replica removal prune.
    LifecycleReplicaRemoval,
    /// `commands::entity_creation`/`cli::workgroup` workgroup removal prune.
    LifecycleWorkgroupRemoval,
    /// `commands::entity_creation::delete_team` team-deletion prune.
    LifecycleTeamDeletion,
    /// `commands::entity_creation::delete_agent_matrix` Agent Matrix prune.
    LifecycleAgentMatrixDeletion,
    /// Agent Matrix pending-inclusive delete protection staged-rollback prune.
    PendingInclusiveDeleteProtection,
    /// `config::coding_agents_catalog` per-project catalog seed + seed-manifest
    /// publication (`ensure_seeded_for_project` / `record_catalog_publication`,
    /// #1318). The first coverage-v2-era publisher: the catalog row kind only
    /// exists at `coverage_version = 2`.
    CatalogSeed,
}

/// The exhaustive, ordered coverage set. Its length equals the number of
/// [`V1CoverageBoundary`] variants; the `v1_coverage_declaration_is_exhaustive`
/// test enforces that a new boundary is added here and matched, so a coverage
/// entry cannot be dropped from the checklist silently.
pub(crate) const V1_COVERAGE_BOUNDARIES: [V1CoverageBoundary; 17] = [
    V1CoverageBoundary::DirectCreateAcProjectFreshRoot,
    V1CoverageBoundary::ContextCreate,
    V1CoverageBoundary::ContextUpdate,
    V1CoverageBoundary::ContextSelfHeal,
    V1CoverageBoundary::ContextOverwrite,
    V1CoverageBoundary::CoordinatorStatelessV2ToV4,
    V1CoverageBoundary::CoordinatorStatelessV3ToV4,
    V1CoverageBoundary::CoordinatorSeededV3ToV4,
    V1CoverageBoundary::ConfigExactPublish,
    V1CoverageBoundary::ConfigOverBoundPublish,
    V1CoverageBoundary::ConfigFailedRestore,
    V1CoverageBoundary::LifecycleReplicaRemoval,
    V1CoverageBoundary::LifecycleWorkgroupRemoval,
    V1CoverageBoundary::LifecycleTeamDeletion,
    V1CoverageBoundary::LifecycleAgentMatrixDeletion,
    V1CoverageBoundary::PendingInclusiveDeleteProtection,
    V1CoverageBoundary::CatalogSeed,
];

const BLOCKING_WORK_QUEUED: u8 = 0;
const BLOCKING_WORK_STARTED: u8 = 1;
const BLOCKING_WORK_CANCELED: u8 = 2;

#[derive(Debug, Error)]
pub(crate) enum BlockingTaskError {
    #[error("blocking task canceled before start")]
    CanceledBeforeStart,
    #[error("blocking task join failed: {0}")]
    Join(#[source] tokio::task::JoinError),
}

enum BlockingWorkResult<T> {
    Completed(T),
    CanceledBeforeStart,
}

struct BlockingWaiterGuard {
    state: Arc<AtomicU8>,
    abort: tokio::task::AbortHandle,
    armed: bool,
}

impl BlockingWaiterGuard {
    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for BlockingWaiterGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        if self
            .state
            .compare_exchange(
                BLOCKING_WORK_QUEUED,
                BLOCKING_WORK_CANCELED,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
        {
            self.abort.abort();
        }
    }
}

/// Runs blocking work with explicit ownership of the queued-to-start boundary.
/// Dropping an unpolled future schedules nothing. Dropping a polled waiter
/// cancels work that is still queued, while already-started work remains owned
/// by the blocking worker and runs to completion.
pub(crate) async fn run_blocking_owned<F, T>(work: F) -> Result<T, BlockingTaskError>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    let state = Arc::new(AtomicU8::new(BLOCKING_WORK_QUEUED));
    let worker_state = Arc::clone(&state);
    let handle = tokio::task::spawn_blocking(move || {
        if worker_state
            .compare_exchange(
                BLOCKING_WORK_QUEUED,
                BLOCKING_WORK_STARTED,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_err()
        {
            return BlockingWorkResult::CanceledBeforeStart;
        }
        BlockingWorkResult::Completed(work())
    });
    let mut waiter = BlockingWaiterGuard {
        state: Arc::clone(&state),
        abort: handle.abort_handle(),
        armed: true,
    };
    let joined = handle.await;
    waiter.disarm();
    match joined {
        Ok(BlockingWorkResult::Completed(result)) => Ok(result),
        Ok(BlockingWorkResult::CanceledBeforeStart) => Err(BlockingTaskError::CanceledBeforeStart),
        Err(error)
            if error.is_cancelled() && state.load(Ordering::Acquire) == BLOCKING_WORK_CANCELED =>
        {
            Err(BlockingTaskError::CanceledBeforeStart)
        }
        Err(error) => Err(BlockingTaskError::Join(error)),
    }
}

#[derive(Debug, Clone)]
pub(crate) struct PublishedScopeBatch {
    scope: String,
    rows: Vec<PublishedManifestRow>,
}

impl PublishedScopeBatch {
    pub(crate) fn new(
        scope: String,
        source: ManifestSource,
        files: Vec<ManifestPathIdentity>,
        published_at: DateTime<Utc>,
    ) -> Result<Self, SeedManifestError> {
        parse_config_scope(&scope)?;
        if source == ManifestSource::Builtin {
            return Err(SeedManifestError::Validation(
                "config scope publication cannot use builtin source".to_string(),
            ));
        }
        if files.len() > MAX_MANIFEST_ROWS {
            return Err(SeedManifestError::resource_bound(
                ResourceBoundKind::Rows,
                MAX_MANIFEST_ROWS,
                files.len(),
            ));
        }
        let mut keys = BTreeSet::new();
        let mut rows = Vec::with_capacity(files.len());
        for path in files {
            let row =
                PublishedManifestRow::replica_config(path, scope.clone(), source, published_at)?;
            if !keys.insert(row.path.key()) {
                return Err(SeedManifestError::Validation(
                    "scope publication contains a duplicate path identity".to_string(),
                ));
            }
            rows.push(row);
        }
        Ok(Self { scope, rows })
    }
}

#[derive(Debug, Clone)]
pub(crate) enum ManifestLifecycleFilter {
    ConfigPathPrefix(Vec<String>),
    ReplicaComponent(String),
    WorkgroupComponents(BTreeSet<String>),
}

impl ManifestLifecycleFilter {
    pub(crate) fn config_path_prefix(
        prefix: ManifestPathIdentity,
    ) -> Result<Self, SeedManifestError> {
        let DecodedPathComponents::Utf8(components) = prefix.components else {
            return Err(SeedManifestError::Validation(
                "lifecycle config prefix must use canonical UTF-8 components".to_string(),
            ));
        };
        if components.len() < 2 {
            return Err(SeedManifestError::Validation(
                "lifecycle config prefix must include a workgroup component".to_string(),
            ));
        }
        crate::commands::entity_creation::parse_team_from_workgroup_name(&components[1])
            .map_err(SeedManifestError::Validation)?;
        if let Some(replica) = components.get(2) {
            let agent = replica.strip_prefix("__agent_").ok_or_else(|| {
                SeedManifestError::Validation(format!(
                    "lifecycle replica prefix must be named __agent_<name>: {replica}"
                ))
            })?;
            crate::commands::entity_creation::validate_existing_name(agent, "Agent")
                .map_err(SeedManifestError::Validation)?;
        }
        Ok(Self::ConfigPathPrefix(components))
    }

    pub(crate) fn replica_component(component: String) -> Result<Self, SeedManifestError> {
        let agent = component.strip_prefix("__agent_").ok_or_else(|| {
            SeedManifestError::Validation(format!(
                "replica component must be named __agent_<name>: {component}"
            ))
        })?;
        crate::commands::entity_creation::validate_existing_name(agent, "Agent")
            .map_err(SeedManifestError::Validation)?;
        Ok(Self::ReplicaComponent(component))
    }

    pub(crate) fn workgroup_components(
        workgroups: impl IntoIterator<Item = String>,
    ) -> Result<Self, SeedManifestError> {
        let mut validated = BTreeSet::new();
        for workgroup in workgroups {
            crate::commands::entity_creation::parse_team_from_workgroup_name(&workgroup)
                .map_err(SeedManifestError::Validation)?;
            validated.insert(workgroup);
        }
        Ok(Self::WorkgroupComponents(validated))
    }

    fn matches(&self, row: &PublishedManifestRow) -> bool {
        if row.kind != ManifestFileKind::ReplicaConfigFile {
            return false;
        }
        match self {
            Self::ConfigPathPrefix(prefix) => row.path.starts_with_utf8_components(prefix),
            Self::ReplicaComponent(replica) => row.path.utf8_component_equals(2, replica),
            Self::WorkgroupComponents(workgroups) => workgroups
                .iter()
                .any(|workgroup| row.path.utf8_component_equals(1, workgroup)),
        }
    }
}

enum ProjectPublicationPermitState<'a> {
    Tracked(&'a mut ProjectSeedManifestGuard),
    DegradedUntracked {
        held_guard: Option<&'a mut ProjectSeedManifestGuard>,
        reason: ManifestDegradedReason,
    },
}

pub(crate) struct ProjectPublicationPermit<'a> {
    state: ProjectPublicationPermitState<'a>,
}

impl fmt::Debug for ProjectPublicationPermit<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (mode, reason, holds_guard) = match &self.state {
            ProjectPublicationPermitState::Tracked(_) => ("tracked", None, true),
            ProjectPublicationPermitState::DegradedUntracked { held_guard, reason } => {
                ("degraded_untracked", Some(*reason), held_guard.is_some())
            }
        };
        formatter
            .debug_struct("ProjectPublicationPermit")
            .field("mode", &mode)
            .field("reason", &reason)
            .field("holds_guard", &holds_guard)
            .finish()
    }
}

impl<'a> ProjectPublicationPermit<'a> {
    fn degraded_without_guard(reason: ManifestDegradedReason) -> Self {
        Self {
            state: ProjectPublicationPermitState::DegradedUntracked {
                held_guard: None,
                reason,
            },
        }
    }

    pub(crate) fn is_tracked(&self) -> bool {
        matches!(self.state, ProjectPublicationPermitState::Tracked(_))
    }

    pub(crate) fn record_file(
        self,
        activation: &ManifestActivationToken,
        row: PublishedManifestRow,
    ) -> ManifestRecordOutcome {
        activation.authorize();
        match self.state {
            ProjectPublicationPermitState::Tracked(guard) => guard.record_file(row),
            ProjectPublicationPermitState::DegradedUntracked { reason, .. } => {
                ManifestRecordOutcome::PublishedUnrecorded(reason)
            }
        }
    }

    pub(crate) fn replace_scope(
        self,
        activation: &ManifestActivationToken,
        batch: PublishedScopeBatch,
    ) -> ManifestRecordOutcome {
        activation.authorize();
        match self.state {
            ProjectPublicationPermitState::Tracked(guard) => guard.replace_scope(batch),
            ProjectPublicationPermitState::DegradedUntracked { reason, .. } => {
                ManifestRecordOutcome::PublishedUnrecorded(reason)
            }
        }
    }

    pub(crate) fn remove_unrecordable_scope(
        self,
        activation: &ManifestActivationToken,
        scope: String,
        bound: ResourceBoundKind,
    ) -> ManifestRecordOutcome {
        activation.authorize();
        if let Err(error) = parse_config_scope(&scope) {
            log::warn!(
                "[seed_manifest] rejected invalid unrecordable scope={} error={}",
                scope,
                error
            );
            return ManifestRecordOutcome::PublishedUnrecorded(
                ManifestDegradedReason::InvalidCanonical,
            );
        }
        match self.state {
            ProjectPublicationPermitState::Tracked(guard) => {
                guard.remove_scope_as_unrecorded(&scope, bound)
            }
            ProjectPublicationPermitState::DegradedUntracked { reason, .. } => {
                ManifestRecordOutcome::PublishedUnrecorded(reason)
            }
        }
    }

    /// #1065 Stage F failed-restore path (plan sections 5.3/5.4): remove an entire
    /// config scope's rows without adding a row or timestamp. The old destination
    /// was renamed away and not restored, so its prior rows no longer describe
    /// reality; this is a pure removal, never a publication and never a resource
    /// bound. Unlike `remove_unrecordable_scope` it carries no `ResourceBoundKind`.
    pub(crate) fn remove_config_scope(
        self,
        activation: &ManifestActivationToken,
        scope: String,
    ) -> ManifestRecordOutcome {
        activation.authorize();
        if let Err(error) = parse_config_scope(&scope) {
            log::warn!(
                "[seed_manifest] rejected invalid removal scope={} error={}",
                scope,
                error
            );
            return ManifestRecordOutcome::PublishedUnrecorded(
                ManifestDegradedReason::InvalidCanonical,
            );
        }
        match self.state {
            ProjectPublicationPermitState::Tracked(guard) => guard.remove_config_scope(&scope),
            ProjectPublicationPermitState::DegradedUntracked { reason, .. } => {
                ManifestRecordOutcome::PublishedUnrecorded(reason)
            }
        }
    }

    pub(crate) fn apply_lifecycle_filter(
        self,
        activation: &ManifestActivationToken,
        filter: ManifestLifecycleFilter,
    ) -> ManifestRecordOutcome {
        activation.authorize();
        match self.state {
            ProjectPublicationPermitState::Tracked(guard) => guard.apply_lifecycle_filter(filter),
            ProjectPublicationPermitState::DegradedUntracked { reason, .. } => {
                ManifestRecordOutcome::PublishedUnrecorded(reason)
            }
        }
    }
}

impl ProjectSeedManifestGuard {
    fn writable_state_mut(&mut self) -> Option<&mut ManifestState> {
        match &mut self.snapshot {
            CanonicalSnapshot::Writable { state, .. } => Some(state),
            CanonicalSnapshot::ReadOnly { .. } => None,
        }
    }

    fn record_file(&mut self, row: PublishedManifestRow) -> ManifestRecordOutcome {
        let scope = row.scope.clone();
        let Some(state) = self.writable_state_mut() else {
            return ManifestRecordOutcome::PublishedUnrecorded(
                self.snapshot
                    .reason()
                    .unwrap_or(ManifestDegradedReason::InvalidCanonical),
            );
        };
        let journal = state.upsert(row);
        if !journal.changed(state) {
            journal.rollback(state);
            return ManifestRecordOutcome::Unchanged;
        }
        match self.persist_current_state() {
            Ok(outcome) => outcome,
            Err(SeedManifestError::ResourceBound { kind, .. }) => {
                if let Some(state) = self.writable_state_mut() {
                    journal.rollback(state);
                }
                self.remove_scope_as_unrecorded(&scope, kind)
            }
            Err(error) => {
                if let Some(state) = self.writable_state_mut() {
                    journal.rollback(state);
                }
                log_record_failure(&scope, &error);
                ManifestRecordOutcome::PublishedUnrecorded(error.degraded_reason())
            }
        }
    }

    fn replace_scope(&mut self, batch: PublishedScopeBatch) -> ManifestRecordOutcome {
        let Some(state) = self.writable_state_mut() else {
            return ManifestRecordOutcome::PublishedUnrecorded(
                self.snapshot
                    .reason()
                    .unwrap_or(ManifestDegradedReason::InvalidCanonical),
            );
        };
        let journal = state.replace_scope(&batch.scope, batch.rows);
        if !journal.changed(state) {
            journal.rollback(state);
            return ManifestRecordOutcome::Unchanged;
        }
        match self.persist_current_state() {
            Ok(outcome) => outcome,
            Err(SeedManifestError::ResourceBound { kind, .. }) => {
                if let Some(state) = self.writable_state_mut() {
                    journal.rollback(state);
                }
                self.remove_scope_as_unrecorded(&batch.scope, kind)
            }
            Err(error) => {
                if let Some(state) = self.writable_state_mut() {
                    journal.rollback(state);
                }
                log_record_failure(&batch.scope, &error);
                ManifestRecordOutcome::PublishedUnrecorded(error.degraded_reason())
            }
        }
    }

    fn remove_scope_as_unrecorded(
        &mut self,
        scope: &str,
        bound: ResourceBoundKind,
    ) -> ManifestRecordOutcome {
        let Some(state) = self.writable_state_mut() else {
            return ManifestRecordOutcome::PublishedUnrecorded(
                self.snapshot
                    .reason()
                    .unwrap_or(ManifestDegradedReason::ResourceBound(bound)),
            );
        };
        let journal = state.remove_scope(scope);
        if journal.changed() {
            if let Err(error) = self.persist_current_state() {
                if let Some(state) = self.writable_state_mut() {
                    journal.rollback(state);
                }
                log_record_failure(scope, &error);
                return ManifestRecordOutcome::PublishedUnrecorded(error.degraded_reason());
            }
        }
        ManifestRecordOutcome::PublishedUnrecorded(ManifestDegradedReason::ResourceBound(bound))
    }

    fn remove_config_scope(&mut self, scope: &str) -> ManifestRecordOutcome {
        let Some(state) = self.writable_state_mut() else {
            return ManifestRecordOutcome::PublishedUnrecorded(
                self.snapshot
                    .reason()
                    .unwrap_or(ManifestDegradedReason::InvalidCanonical),
            );
        };
        let journal = state.remove_scope(scope);
        if !journal.changed() {
            return ManifestRecordOutcome::Unchanged;
        }
        match self.persist_current_state() {
            Ok(outcome) => outcome,
            Err(error) => {
                if let Some(state) = self.writable_state_mut() {
                    journal.rollback(state);
                }
                log_record_failure(scope, &error);
                ManifestRecordOutcome::PublishedUnrecorded(error.degraded_reason())
            }
        }
    }

    fn apply_lifecycle_filter(&mut self, filter: ManifestLifecycleFilter) -> ManifestRecordOutcome {
        let Some(state) = self.writable_state_mut() else {
            return ManifestRecordOutcome::PublishedUnrecorded(
                self.snapshot
                    .reason()
                    .unwrap_or(ManifestDegradedReason::InvalidCanonical),
            );
        };
        let journal = state.remove_matching(|row| filter.matches(row));
        if !journal.changed() {
            return ManifestRecordOutcome::Unchanged;
        }
        match self.persist_current_state() {
            Ok(outcome) => outcome,
            Err(error) => {
                if let Some(state) = self.writable_state_mut() {
                    journal.rollback(state);
                }
                log_record_failure("lifecycle-filter", &error);
                ManifestRecordOutcome::PublishedUnrecorded(error.degraded_reason())
            }
        }
    }
}

fn log_record_failure(scope: &str, error: &SeedManifestError) {
    log::warn!(
        "[seed_manifest] target or lifecycle action succeeded but manifest persistence did not scope={} error={}",
        scope,
        error
    );
}

#[cfg(test)]
// The shared test module deliberately precedes the cfg-gated Windows implementation it exercises.
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
    use std::sync::Arc;

    #[tokio::test]
    async fn blocking_owned_drop_before_first_poll_schedules_nothing() {
        struct DropProbe(Arc<AtomicBool>);

        impl Drop for DropProbe {
            fn drop(&mut self) {
                self.0.store(true, AtomicOrdering::Release);
            }
        }

        let ran = Arc::new(AtomicBool::new(false));
        let dropped = Arc::new(AtomicBool::new(false));
        let drop_probe = DropProbe(Arc::clone(&dropped));
        let future = run_blocking_owned({
            let ran = Arc::clone(&ran);
            move || {
                let _drop_probe = drop_probe;
                ran.store(true, AtomicOrdering::Release);
            }
        });
        drop(future);
        tokio::task::yield_now().await;
        assert!(!ran.load(AtomicOrdering::Acquire));
        assert!(dropped.load(AtomicOrdering::Acquire));
    }

    #[test]
    fn blocking_owned_drop_while_queued_suppresses_user_work() {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .max_blocking_threads(1)
            .enable_all()
            .build()
            .expect("runtime");
        runtime.block_on(async {
            let (blocker_started_tx, blocker_started_rx) = std::sync::mpsc::channel();
            let (release_tx, release_rx) = std::sync::mpsc::channel();
            let blocker = tokio::task::spawn_blocking(move || {
                blocker_started_tx.send(()).expect("signal blocker");
                release_rx.recv().expect("release blocker");
            });
            blocker_started_rx
                .recv_timeout(Duration::from_secs(1))
                .expect("blocking pool occupied");

            let ran = Arc::new(AtomicBool::new(false));
            let waiter = tokio::spawn(run_blocking_owned({
                let ran = Arc::clone(&ran);
                move || ran.store(true, AtomicOrdering::Release)
            }));
            for _ in 0..8 {
                tokio::task::yield_now().await;
            }
            waiter.abort();
            let _ = waiter.await;
            release_tx.send(()).expect("release blocking pool");
            blocker.await.expect("join blocker");
            tokio::task::yield_now().await;
            assert!(!ran.load(AtomicOrdering::Acquire));
        });
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn blocking_owned_started_work_finishes_after_waiter_drop() {
        let started = Arc::new(AtomicBool::new(false));
        let completed = Arc::new(AtomicBool::new(false));
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let waiter = tokio::spawn(run_blocking_owned({
            let started = Arc::clone(&started);
            let completed = Arc::clone(&completed);
            move || {
                started.store(true, AtomicOrdering::Release);
                release_rx.recv().expect("release started work");
                completed.store(true, AtomicOrdering::Release);
            }
        }));
        while !started.load(AtomicOrdering::Acquire) {
            tokio::task::yield_now().await;
        }
        waiter.abort();
        let _ = waiter.await;
        release_tx.send(()).expect("release worker");
        tokio::time::timeout(Duration::from_secs(1), async {
            while !completed.load(AtomicOrdering::Acquire) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("started work must remain self-owned");
    }

    #[tokio::test]
    async fn blocking_owned_surfaces_worker_panic_as_join_error() {
        let error = run_blocking_owned(|| -> () { panic!("injected blocking panic") })
            .await
            .expect_err("panic must surface as a join error");
        let BlockingTaskError::Join(join_error) = &error else {
            panic!("expected typed join error, got {error:?}");
        };
        assert!(join_error.is_panic());
        let source = std::error::Error::source(&error).expect("join error remains the source");
        assert!(source.downcast_ref::<tokio::task::JoinError>().is_some());
    }

    fn timestamp(value: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(value)
            .expect("valid test timestamp")
            .with_timezone(&Utc)
    }

    fn context_row(
        filename: &str,
        scope: &str,
        at: &str,
    ) -> Result<PublishedManifestRow, SeedManifestError> {
        PublishedManifestRow::project_context(
            ManifestPathIdentity::parse(ManifestPathEncoding::Utf8, format!(".ac/{filename}"))?,
            scope,
            timestamp(at),
        )
    }

    fn config_path(workgroup: &str, agent: &str, dest: &str, suffix: &str) -> ManifestPathIdentity {
        ManifestPathIdentity::parse(
            ManifestPathEncoding::Utf8,
            format!(".ac/{workgroup}/__agent_{agent}/{dest}/{suffix}"),
        )
        .expect("valid config path")
    }

    fn config_scope(workgroup: &str, agent: &str, dest: &str) -> String {
        format!("config:.ac/{workgroup}/__agent_{agent}/{dest}")
    }

    fn setup_project() -> (tempfile::TempDir, PathBuf) {
        let _ = env_logger::builder()
            .is_test(true)
            .filter_level(log::LevelFilter::Warn)
            .try_init();
        let temp = tempfile::tempdir().expect("tempdir");
        let project = temp.path().join("project");
        std::fs::create_dir_all(project.join(".ac")).expect("create project .ac");
        (temp, project)
    }

    fn canonical_path(project: &Path) -> PathBuf {
        project.join(".ac").join(SEED_MANIFEST_FILENAME)
    }

    fn read_disk_state(project: &Path) -> ManifestState {
        let bytes = std::fs::read(canonical_path(project)).expect("read canonical manifest");
        parse_manifest_bytes(&bytes).expect("parse canonical manifest")
    }

    fn exact_temp_paths(project: &Path) -> Vec<PathBuf> {
        std::fs::read_dir(project.join(".ac"))
            .expect("read .ac")
            .filter_map(Result::ok)
            .filter_map(|entry| {
                entry
                    .file_name()
                    .to_str()
                    .filter(|name| is_exact_temp_name(name))
                    .map(|_| entry.path())
            })
            .collect()
    }

    #[cfg(windows)]
    fn windows_ads_path(path: &Path, name: &str) -> PathBuf {
        let mut stream = path.as_os_str().to_os_string();
        stream.push(":");
        stream.push(name);
        PathBuf::from(stream)
    }

    #[cfg(windows)]
    fn windows_get_dacl(path: &Path) -> Vec<u8> {
        use windows_sys::Win32::Foundation::{GetLastError, ERROR_INSUFFICIENT_BUFFER};
        use windows_sys::Win32::Security::{GetFileSecurityW, DACL_SECURITY_INFORMATION};

        let wide = absolute_verbatim_utf16(path).expect("verbatim path");
        let mut needed = 0_u32;
        let first = unsafe {
            GetFileSecurityW(
                wide.as_ptr(),
                DACL_SECURITY_INFORMATION,
                std::ptr::null_mut(),
                0,
                &mut needed,
            )
        };
        assert_eq!(first, 0);
        let raw_error = unsafe { GetLastError() };
        assert_eq!(raw_error, ERROR_INSUFFICIENT_BUFFER);
        let mut descriptor = vec![0_u8; usize::try_from(needed).expect("descriptor size")];
        let success = unsafe {
            GetFileSecurityW(
                wide.as_ptr(),
                DACL_SECURITY_INFORMATION,
                descriptor.as_mut_ptr().cast(),
                needed,
                &mut needed,
            )
        };
        assert_ne!(
            success,
            0,
            "GetFileSecurityW: {}",
            io::Error::last_os_error()
        );
        descriptor.truncate(usize::try_from(needed).expect("descriptor size"));
        descriptor
    }

    #[cfg(windows)]
    fn windows_protect_existing_dacl(path: &Path) -> Vec<u8> {
        use windows_sys::Win32::Security::{
            AddAccessAllowedAceEx, GetSecurityDescriptorControl, GetSecurityDescriptorDacl,
            InitializeAcl, InitializeSecurityDescriptor, SetFileSecurityW,
            SetSecurityDescriptorDacl, ACCESS_ALLOWED_ACE, ACL, ACL_REVISION,
            DACL_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION, SECURITY_DESCRIPTOR,
            SE_DACL_PROTECTED,
        };
        use windows_sys::Win32::Storage::FileSystem::FILE_ALL_ACCESS;

        let world_sid = [1_u8, 1, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0];
        let acl_length = std::mem::size_of::<ACL>() + std::mem::size_of::<ACCESS_ALLOWED_ACE>()
            - std::mem::size_of::<u32>()
            + world_sid.len();
        let mut acl = vec![0_u8; acl_length];
        let acl_ptr = acl.as_mut_ptr().cast::<ACL>();
        let initialized = unsafe {
            InitializeAcl(
                acl_ptr,
                u32::try_from(acl.len()).expect("ACL size"),
                ACL_REVISION,
            )
        };
        assert_ne!(
            initialized,
            0,
            "InitializeAcl: {}",
            io::Error::last_os_error()
        );
        let added = unsafe {
            AddAccessAllowedAceEx(
                acl_ptr,
                ACL_REVISION,
                0,
                FILE_ALL_ACCESS,
                world_sid.as_ptr().cast_mut().cast(),
            )
        };
        assert_ne!(
            added,
            0,
            "AddAccessAllowedAceEx: {}",
            io::Error::last_os_error()
        );

        let mut descriptor = std::mem::MaybeUninit::<SECURITY_DESCRIPTOR>::zeroed();
        let initialized =
            unsafe { InitializeSecurityDescriptor(descriptor.as_mut_ptr().cast(), 1) };
        assert_ne!(
            initialized,
            0,
            "InitializeSecurityDescriptor: {}",
            io::Error::last_os_error()
        );
        let mut descriptor = unsafe { descriptor.assume_init() };
        let dacl_set = unsafe {
            SetSecurityDescriptorDacl(
                (&mut descriptor as *mut SECURITY_DESCRIPTOR).cast(),
                1,
                acl_ptr,
                0,
            )
        };
        assert_ne!(
            dacl_set,
            0,
            "SetSecurityDescriptorDacl: {}",
            io::Error::last_os_error()
        );
        descriptor.Control |= SE_DACL_PROTECTED;
        let wide = absolute_verbatim_utf16(path).expect("verbatim path");
        let success = unsafe {
            SetFileSecurityW(
                wide.as_ptr(),
                DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                (&mut descriptor as *mut SECURITY_DESCRIPTOR).cast(),
            )
        };
        assert_ne!(
            success,
            0,
            "SetFileSecurityW: {}",
            io::Error::last_os_error()
        );
        let mut persisted = windows_get_dacl(path);
        let mut control = 0_u16;
        let mut revision = 0_u32;
        let control_ok = unsafe {
            GetSecurityDescriptorControl(persisted.as_mut_ptr().cast(), &mut control, &mut revision)
        };
        assert_ne!(control_ok, 0);
        assert_ne!(control & SE_DACL_PROTECTED, 0);
        let mut present = 0;
        let mut defaulted = 0;
        let mut persisted_acl = std::ptr::null_mut();
        let dacl_ok = unsafe {
            GetSecurityDescriptorDacl(
                persisted.as_mut_ptr().cast(),
                &mut present,
                &mut persisted_acl,
                &mut defaulted,
            )
        };
        assert_ne!(dacl_ok, 0);
        assert_ne!(present, 0);
        assert!(!persisted_acl.is_null());
        let persisted_size = unsafe { usize::from((*persisted_acl).AclSize) };
        unsafe { std::slice::from_raw_parts(persisted_acl.cast::<u8>(), persisted_size) }.to_vec()
    }

    #[cfg(windows)]
    fn windows_dacl_bytes(path: &Path) -> Vec<u8> {
        use windows_sys::Win32::Security::GetSecurityDescriptorDacl;

        let mut descriptor = windows_get_dacl(path);
        let mut present = 0;
        let mut defaulted = 0;
        let mut acl = std::ptr::null_mut();
        let success = unsafe {
            GetSecurityDescriptorDacl(
                descriptor.as_mut_ptr().cast(),
                &mut present,
                &mut acl,
                &mut defaulted,
            )
        };
        assert_ne!(success, 0);
        assert_ne!(present, 0);
        assert!(!acl.is_null());
        let size = unsafe { usize::from((*acl).AclSize) };
        unsafe { std::slice::from_raw_parts(acl.cast::<u8>(), size) }.to_vec()
    }

    #[cfg(windows)]
    fn windows_set_compression(path: &Path, format: u16) {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Foundation::HANDLE;
        use windows_sys::Win32::System::Ioctl::FSCTL_SET_COMPRESSION;
        use windows_sys::Win32::System::IO::DeviceIoControl;

        let file = open_regular_no_follow(path, true, false).expect("open compression fixture");
        let mut returned = 0_u32;
        let success = unsafe {
            DeviceIoControl(
                file.as_raw_handle() as HANDLE,
                FSCTL_SET_COMPRESSION,
                (&format as *const u16).cast(),
                u32::try_from(std::mem::size_of::<u16>()).expect("u16 size"),
                std::ptr::null_mut(),
                0,
                &mut returned,
                std::ptr::null_mut(),
            )
        };
        assert_ne!(
            success,
            0,
            "DeviceIoControl: {}",
            io::Error::last_os_error()
        );
    }

    #[cfg(windows)]
    fn windows_set_attributes(path: &Path, attributes: u32) {
        use windows_sys::Win32::Storage::FileSystem::SetFileAttributesW;

        let wide = absolute_verbatim_utf16(path).expect("verbatim path");
        let success = unsafe { SetFileAttributesW(wide.as_ptr(), attributes) };
        assert_ne!(
            success,
            0,
            "SetFileAttributesW: {}",
            io::Error::last_os_error()
        );
    }

    fn populated_state() -> ManifestState {
        let mut state = ManifestState::default();
        let context = context_row(
            "Context.AgentsCommander.md",
            "context:agentscommander",
            "2026-07-16T19:40:07.123Z",
        )
        .expect("context row");
        let config = PublishedManifestRow::replica_config(
            config_path("wg-14-dev-team", "architect", ".claude", "settings.json"),
            config_scope("wg-14-dev-team", "architect", ".claude"),
            ManifestSource::WorkspaceBase,
            timestamp("2026-07-16T19:41:12.456Z"),
        )
        .expect("config row");
        let _ = state.upsert(config);
        let _ = state.upsert(context);
        state
    }

    #[test]
    fn golden_empty_manifest_is_exact() {
        let bytes = serialize_state(&ManifestState::default()).expect("serialize empty");
        assert_eq!(
            bytes,
            concat!(
                "# Managed by AgentsCommander. Diagnostic only; never grants file ownership.\n",
                "schema_version = 1\n",
                "coverage_version = 2\n",
                "coverage = [\"project_context_templates\", \"replica_config_folders\", \"coding_agent_catalog\"]\n",
                "files = []\n"
            )
            .as_bytes()
        );
        assert!(!bytes.windows(2).any(|window| window == b"\r\n"));
        assert!(bytes.ends_with(b"\n"));
        assert!(!bytes.ends_with(b"\n\n"));
    }

    #[test]
    fn golden_populated_manifest_is_exact_and_sorted() {
        let bytes = serialize_state(&populated_state()).expect("serialize populated");
        let expected = concat!(
            "# Managed by AgentsCommander. Diagnostic only; never grants file ownership.\n",
            "schema_version = 1\n",
            "coverage_version = 2\n",
            "coverage = [\"project_context_templates\", \"replica_config_folders\", \"coding_agent_catalog\"]\n",
            "\n",
            "[[files]]\n",
            "path = \".ac/Context.AgentsCommander.md\"\n",
            "path_encoding = \"utf8\"\n",
            "kind = \"project_context_template\"\n",
            "scope = \"context:agentscommander\"\n",
            "source = \"builtin\"\n",
            "last_seeded_at = \"2026-07-16T19:40:07.123Z\"\n",
            "\n",
            "[[files]]\n",
            "path = \".ac/wg-14-dev-team/__agent_architect/.claude/settings.json\"\n",
            "path_encoding = \"utf8\"\n",
            "kind = \"replica_config_file\"\n",
            "scope = \"config:.ac/wg-14-dev-team/__agent_architect/.claude\"\n",
            "source = \"workspace_base\"\n",
            "last_seeded_at = \"2026-07-16T19:41:12.456Z\"\n"
        );
        assert_eq!(bytes, expected.as_bytes());
        assert_eq!(
            exact_serialized_len(&populated_state()).unwrap(),
            bytes.len()
        );
        assert_eq!(parse_manifest_bytes(&bytes).unwrap(), populated_state());
    }

    #[test]
    fn insertion_order_does_not_change_bytes() {
        let first = populated_state();
        let mut second = ManifestState::default();
        for row in first.rows.values().rev() {
            let _ = second.upsert(row.clone());
        }
        assert_eq!(
            serialize_state(&first).unwrap(),
            serialize_state(&second).unwrap()
        );
    }

    #[test]
    fn strict_wire_rejects_missing_unknown_duplicate_and_noncanonical_values() {
        let empty = String::from_utf8(serialize_state(&ManifestState::default()).unwrap()).unwrap();
        let without_files = empty.replace("files = []\n", "");
        assert!(matches!(
            parse_manifest_bytes(without_files.as_bytes()),
            Err(SeedManifestError::Parse(_))
        ));

        let unknown = empty.replace("schema_version = 1\n", "schema_version = 1\nextra = 1\n");
        assert!(matches!(
            parse_manifest_bytes(unknown.as_bytes()),
            Err(SeedManifestError::Parse(_))
        ));

        let duplicate = empty.replace(
            "schema_version = 1\n",
            "schema_version = 1\nschema_version = 1\n",
        );
        assert!(matches!(
            parse_manifest_bytes(duplicate.as_bytes()),
            Err(SeedManifestError::Parse(_))
        ));

        let wrong_coverage = empty.replace("coverage_version = 2", "coverage_version = 3");
        assert!(matches!(
            parse_manifest_bytes(wrong_coverage.as_bytes()),
            Err(SeedManifestError::Validation(_))
        ));

        let unknown_enum = String::from_utf8(serialize_state(&populated_state()).unwrap())
            .unwrap()
            .replace("source = \"builtin\"", "source = \"mystery\"");
        assert!(matches!(
            parse_manifest_bytes(unknown_enum.as_bytes()),
            Err(SeedManifestError::Parse(_))
        ));

        let noncanonical_time = String::from_utf8(serialize_state(&populated_state()).unwrap())
            .unwrap()
            .replace("2026-07-16T19:40:07.123Z", "2026-07-16T19:40:07.123+00:00");
        assert!(matches!(
            parse_manifest_bytes(noncanonical_time.as_bytes()),
            Err(SeedManifestError::Validation(_))
        ));
    }

    #[test]
    fn strict_state_rejects_duplicate_identity_and_mixed_config_batches() {
        let row = PublishedManifestRow::replica_config(
            config_path("wg-1-team", "alpha", ".claude", "settings.json"),
            config_scope("wg-1-team", "alpha", ".claude"),
            ManifestSource::WorkspaceBase,
            timestamp("2026-07-16T19:41:12.456Z"),
        )
        .unwrap()
        .to_wire();
        let duplicate_wire = SeedManifestWire {
            schema_version: 1,
            coverage_version: 2,
            coverage: COVERAGE.map(str::to_string).to_vec(),
            files: vec![row.clone(), row.clone()],
        };
        assert!(matches!(
            ManifestState::from_wire(duplicate_wire),
            Err(SeedManifestError::Validation(_))
        ));

        let mut mixed = row;
        mixed.path = ".ac/wg-1-team/__agent_alpha/.claude/other.json".to_string();
        mixed.source = ManifestSource::MatrixBase;
        let mixed_wire = SeedManifestWire {
            schema_version: 1,
            coverage_version: 2,
            coverage: COVERAGE.map(str::to_string).to_vec(),
            files: vec![
                PublishedManifestRow::replica_config(
                    config_path("wg-1-team", "alpha", ".claude", "settings.json"),
                    config_scope("wg-1-team", "alpha", ".claude"),
                    ManifestSource::WorkspaceBase,
                    timestamp("2026-07-16T19:41:12.456Z"),
                )
                .unwrap()
                .to_wire(),
                mixed,
            ],
        };
        assert!(matches!(
            ManifestState::from_wire(mixed_wire),
            Err(SeedManifestError::Validation(_))
        ));
    }

    #[test]
    fn path_codecs_are_lossless_canonical_and_component_wise() {
        let readable = ManifestPathIdentity::parse(
            ManifestPathEncoding::Utf8,
            ".ac/wg-1-team/__agent_alpha/.claude/settings.json".to_string(),
        )
        .unwrap();
        assert_eq!(readable.encoding(), ManifestPathEncoding::Utf8);

        let mut unix_bytes = b".ac/wg-1-team/__agent_alpha/.claude/file".to_vec();
        unix_bytes.push(0xff);
        let unix = ManifestPathIdentity::parse(
            ManifestPathEncoding::UnixBytesHex,
            encode_lower_hex_bytes(&unix_bytes),
        )
        .unwrap();
        assert!(unix.starts_with_utf8_components(&[
            ".ac".to_string(),
            "wg-1-team".to_string(),
            "__agent_alpha".to_string(),
        ]));

        let mut windows_units = ".ac/wg-1-team/__agent_alpha/.claude/file"
            .encode_utf16()
            .collect::<Vec<_>>();
        windows_units.push(0xd800);
        let windows = ManifestPathIdentity::parse(
            ManifestPathEncoding::WindowsUtf16Hex,
            encode_lower_hex_u16(&windows_units),
        )
        .unwrap();
        assert!(windows.starts_with_utf8_components(&[
            ".ac".to_string(),
            "wg-1-team".to_string(),
            "__agent_alpha".to_string(),
        ]));
        assert_ne!(unix.key(), windows.key());

        let canonical_hex = encode_lower_hex_bytes(b".ac/readable");
        assert!(
            ManifestPathIdentity::parse(ManifestPathEncoding::UnixBytesHex, canonical_hex).is_err()
        );
        assert!(ManifestPathIdentity::parse(
            ManifestPathEncoding::UnixBytesHex,
            "2E6163ff".to_string()
        )
        .is_err());
        assert!(ManifestPathIdentity::parse(
            ManifestPathEncoding::Utf8,
            ".ac/../escape".to_string()
        )
        .is_err());
        assert!(ManifestPathIdentity::parse(
            ManifestPathEncoding::Utf8,
            "/.ac/absolute".to_string()
        )
        .is_err());
    }

    #[cfg(unix)]
    #[test]
    fn native_unix_invalid_byte_path_round_trips() {
        use std::os::unix::ffi::OsStringExt;

        let path = PathBuf::from(".ac").join(std::ffi::OsString::from_vec(vec![b'f', 0xff]));
        let identity = ManifestPathIdentity::from_relative_path(&path).unwrap();
        assert_eq!(identity.encoding(), ManifestPathEncoding::UnixBytesHex);
        let reparsed =
            ManifestPathIdentity::parse(identity.encoding(), identity.serialized().to_string())
                .unwrap();
        assert_eq!(identity, reparsed);
    }

    #[cfg(windows)]
    #[test]
    fn native_windows_unpaired_utf16_path_round_trips() {
        use std::os::windows::ffi::OsStringExt;

        let path =
            PathBuf::from(".ac").join(std::ffi::OsString::from_wide(&[u16::from(b'f'), 0xd800]));
        let identity = ManifestPathIdentity::from_relative_path(&path).unwrap();
        assert_eq!(identity.encoding(), ManifestPathEncoding::WindowsUtf16Hex);
        let reparsed =
            ManifestPathIdentity::parse(identity.encoding(), identity.serialized().to_string())
                .unwrap();
        assert_eq!(identity, reparsed);
    }

    #[test]
    fn bounds_and_exact_outbound_counter_are_enforced() {
        let oversized = format!(".ac/{}", "x".repeat(MAX_FIELD_BYTES));
        assert!(matches!(
            ManifestPathIdentity::parse(ManifestPathEncoding::Utf8, oversized),
            Err(SeedManifestError::ResourceBound {
                kind: ResourceBoundKind::PathBytes,
                ..
            })
        ));

        let escaped = PublishedManifestRow::replica_config(
            ManifestPathIdentity::parse(
                ManifestPathEncoding::Utf8,
                ".ac/wg-1-team/__agent_alpha/.claude/quote\"-slash\\-tab\t-del\u{7f}".to_string(),
            )
            .unwrap(),
            config_scope("wg-1-team", "alpha", ".claude"),
            ManifestSource::CatalogDefault,
            timestamp("2026-07-16T19:41:12.456Z"),
        )
        .unwrap();
        let mut state = ManifestState::default();
        let _ = state.upsert(escaped);
        let serialized = serialize_state(&state).unwrap();
        assert_eq!(exact_serialized_len(&state).unwrap(), serialized.len());
        let output_limit = u64::try_from(serialized.len() - 1).unwrap();
        assert!(matches!(
            exact_serialized_len_with_limit(&state, output_limit),
            Err(SeedManifestError::ResourceBound {
                kind: ResourceBoundKind::OutputBytes,
                limit,
                ..
            }) if limit == output_limit
        ));

        let oversized_scope = format!("config:{}", "x".repeat(MAX_FIELD_BYTES));
        assert!(matches!(
            PublishedScopeBatch::new(
                oversized_scope,
                ManifestSource::WorkspaceBase,
                Vec::new(),
                timestamp("2026-07-16T19:41:12.456Z"),
            ),
            Err(SeedManifestError::ResourceBound {
                kind: ResourceBoundKind::ScopeBytes,
                ..
            })
        ));

        let mut total = usize::MAX;
        assert!(matches!(
            checked_add(&mut total, 1),
            Err(SeedManifestError::ResourceBound {
                kind: ResourceBoundKind::ArithmeticOverflow,
                ..
            })
        ));
    }

    #[test]
    fn bounded_reader_rejects_the_first_extra_byte() {
        let temp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(temp.path(), b"12345").unwrap();
        let mut file = File::open(temp.path()).unwrap();
        let error = read_bounded_file(&mut file, 5, 4, temp.path()).unwrap_err();
        assert!(matches!(
            error,
            SeedManifestError::ResourceBound {
                kind: ResourceBoundKind::InputBytes,
                limit: 4,
                observed_at_least: 5
            }
        ));
    }

    #[test]
    fn fixed_buffer_stream_comparison_detects_content_and_length_changes() {
        let temp = tempfile::NamedTempFile::new().unwrap();
        let bytes = vec![b'a'; RAW_COMPARE_BUFFER_BYTES * 2 + 7];
        std::fs::write(temp.path(), &bytes).unwrap();
        let mut file = File::open(temp.path()).unwrap();
        assert!(stream_matches(&mut file, &bytes).unwrap());
        let mut changed = bytes.clone();
        changed[RAW_COMPARE_BUFFER_BYTES + 1] = b'b';
        assert!(!stream_matches(&mut file, &changed).unwrap());
        assert!(!stream_matches(&mut file, &bytes[..bytes.len() - 1]).unwrap());
    }

    #[test]
    fn independent_handles_contend_and_persistent_lock_releases() {
        let (_temp, project) = setup_project();
        let first = ProjectSeedManifestGuard::acquire(&project).unwrap();
        let started = Instant::now();
        let error =
            ProjectSeedManifestGuard::acquire_with_timeout(&project, Duration::from_millis(120))
                .unwrap_err();
        assert!(matches!(error, SeedManifestError::BusyTimeout { .. }));
        assert!(started.elapsed() >= Duration::from_millis(100));
        assert!(started.elapsed() < Duration::from_secs(1));
        let lock_path = project.join(".ac").join(SEED_MANIFEST_LOCK_FILENAME);
        assert!(lock_path.is_file());
        first.release();

        let second =
            ProjectSeedManifestGuard::acquire_with_timeout(&project, Duration::from_millis(250))
                .unwrap();
        second.release();
        assert!(lock_path.is_file(), "unlock must not delete the lock file");
    }

    #[cfg(windows)]
    #[test]
    fn lock_share_denial_is_precontention_lock_io_and_preserves_the_lock_file() {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Foundation::ERROR_SHARING_VIOLATION;
        use windows_sys::Win32::Storage::FileSystem::FILE_SHARE_READ;

        let (_temp, project) = setup_project();
        let lock_path = project.join(".ac").join(SEED_MANIFEST_LOCK_FILENAME);
        let original = b"persistent lock sentinel";
        std::fs::write(&lock_path, original).unwrap();
        let canonical_lock_path = std::fs::canonicalize(&lock_path).unwrap();
        let blocker = OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ)
            .open(&lock_path)
            .unwrap();

        let error =
            ProjectSeedManifestGuard::acquire_with_timeout(&project, Duration::from_millis(100))
                .unwrap_err();
        match &error {
            SeedManifestError::LockIo {
                path,
                saw_contention,
                source,
            } => {
                assert_eq!(path, &canonical_lock_path);
                assert!(!saw_contention);
                assert_eq!(
                    source
                        .raw_os_error()
                        .and_then(|raw| u32::try_from(raw).ok()),
                    Some(ERROR_SHARING_VIOLATION)
                );
            }
            other => panic!("expected pre-contention LockIo, got {other:?}"),
        }
        assert_eq!(
            error.degraded_reason(),
            ManifestDegradedReason::LockUnavailable
        );
        drop(blocker);
        assert_eq!(std::fs::read(&lock_path).unwrap(), original);
    }

    #[test]
    fn canonical_diagnostics_and_guard_debug_never_expose_manifest_contents() {
        let secret = "PRIVATE-SEED-CONTENT-7f9c1a";
        let syntax_bytes = format!("{secret} = [\n");
        let syntax_error = parse_manifest_bytes(syntax_bytes.as_bytes()).unwrap_err();
        assert!(matches!(syntax_error, SeedManifestError::Parse(_)));
        assert!(!format!("{syntax_error}").contains(secret));
        assert!(!format!("{syntax_error:?}").contains(secret));
        assert!(!format!("{syntax_error}").contains("1 |"));

        let invalid_wire = format!(
            concat!(
                "schema_version = 1\n",
                "coverage_version = 2\n",
                "coverage = [\"project_context_templates\", \"replica_config_folders\", \"coding_agent_catalog\"]\n",
                "\n",
                "[[files]]\n",
                "path = \".ac/{secret}\"\n",
                "path_encoding = \"utf8\"\n",
                "kind = \"project_context_template\"\n",
                "scope = \"context:agentscommander\"\n",
                "source = \"builtin\"\n",
                "last_seeded_at = \"2026-07-16T19:40:07.123Z\"\n"
            ),
            secret = secret
        );
        let validation_error = parse_manifest_bytes(invalid_wire.as_bytes()).unwrap_err();
        assert!(matches!(validation_error, SeedManifestError::Validation(_)));
        assert!(!format!("{validation_error}").contains(secret));
        assert!(!format!("{validation_error:?}").contains(secret));

        let (_invalid_temp, invalid_project) = setup_project();
        std::fs::write(canonical_path(&invalid_project), syntax_bytes.as_bytes()).unwrap();
        let mut invalid_guard = ProjectSeedManifestGuard::acquire(&invalid_project).unwrap();
        let invalid_debug = format!("{invalid_guard:?}");
        assert!(!invalid_debug.contains(secret));
        assert_eq!(
            invalid_guard.publication_permit().record_file(
                &ManifestActivationToken::for_test(),
                context_row(
                    "Context.AgentsCommander.md",
                    "context:agentscommander",
                    "2026-07-16T19:40:07.123Z",
                )
                .unwrap(),
            ),
            ManifestRecordOutcome::PublishedUnrecorded(ManifestDegradedReason::InvalidCanonical)
        );
        invalid_guard.release();
        assert_eq!(
            std::fs::read(canonical_path(&invalid_project)).unwrap(),
            syntax_bytes.as_bytes()
        );

        let (_temp, project) = setup_project();
        std::fs::write(
            canonical_path(&project),
            serialize_state(&populated_state()).unwrap(),
        )
        .unwrap();
        let guard = ProjectSeedManifestGuard::acquire(&project).unwrap();
        let debug = format!("{guard:?}");
        assert!(debug.contains("raw_len"));
        assert!(debug.contains("row_count"));
        for content in [
            "Context.AgentsCommander.md",
            "settings.json",
            "context:agentscommander",
            "workspace_base",
        ] {
            assert!(!debug.contains(content), "debug leaked {content}");
        }
        guard.release();
    }

    #[test]
    fn corrupt_and_future_canonical_bytes_are_preserved_read_only() {
        for (name, bytes, expected_reason) in [
            (
                "corrupt",
                b"<<<<<<< conflict\n".as_slice(),
                ManifestDegradedReason::InvalidCanonical,
            ),
            (
                "future",
                concat!(
                    "schema_version = 2\n",
                    "coverage_version = 1\n",
                    "coverage = [\"project_context_templates\", \"replica_config_folders\"]\n",
                    "files = []\n"
                )
                .as_bytes(),
                ManifestDegradedReason::FutureSchema,
            ),
        ] {
            let (_temp, project) = setup_project();
            let path = canonical_path(&project);
            std::fs::write(&path, bytes).unwrap();
            let mut guard = ProjectSeedManifestGuard::acquire(&project).unwrap();
            let outcome = guard.publication_permit().record_file(
                &ManifestActivationToken::for_test(),
                context_row(
                    "Context.AgentsCommander.md",
                    "context:agentscommander",
                    "2026-07-16T19:40:07.123Z",
                )
                .unwrap(),
            );
            assert_eq!(
                outcome,
                ManifestRecordOutcome::PublishedUnrecorded(expected_reason),
                "case {name}"
            );
            guard.release();
            assert_eq!(std::fs::read(path).unwrap(), bytes, "case {name}");
        }
    }

    #[test]
    fn nonregular_canonical_fails_closed() {
        let (_temp, project) = setup_project();
        std::fs::create_dir(canonical_path(&project)).unwrap();
        let error = ProjectSeedManifestGuard::acquire(&project).unwrap_err();
        assert!(matches!(error, SeedManifestError::UnsafePath { .. }));
    }

    #[cfg(unix)]
    #[test]
    fn symlink_canonical_and_lock_fail_closed_without_following() {
        use std::os::unix::fs::symlink;

        for filename in [SEED_MANIFEST_FILENAME, SEED_MANIFEST_LOCK_FILENAME] {
            let (_temp, project) = setup_project();
            let outside = project.join("outside");
            std::fs::write(&outside, b"outside").unwrap();
            symlink(&outside, project.join(".ac").join(filename)).unwrap();
            let error = ProjectSeedManifestGuard::acquire(&project).unwrap_err();
            assert!(matches!(error, SeedManifestError::UnsafePath { .. }));
            assert_eq!(std::fs::read(&outside).unwrap(), b"outside");
        }
    }

    #[test]
    fn prior_temp_policy_is_platform_specific_and_preserves_lookalikes() {
        let (_temp, project) = setup_project();
        let ac = project.join(".ac");
        std::fs::write(
            canonical_path(&project),
            serialize_state(&ManifestState::default()).unwrap(),
        )
        .unwrap();
        let valid = ac.join(".seed-manifest.00000000-0000-0000-0000-000000000000.tmp");
        let uppercase = ac.join(".seed-manifest.00000000-0000-0000-0000-00000000000A.tmp");
        let malformed = ac.join(".seed-manifest.not-a-uuid.tmp");
        let directory = ac.join(".seed-manifest.11111111-1111-1111-1111-111111111111.tmp");
        std::fs::write(&valid, b"stale").unwrap();
        std::fs::write(&uppercase, b"user").unwrap();
        std::fs::write(&malformed, b"user").unwrap();
        std::fs::create_dir(&directory).unwrap();

        let guard = ProjectSeedManifestGuard::acquire(&project).unwrap();
        guard.release();
        #[cfg(unix)]
        assert!(!valid.exists());
        #[cfg(windows)]
        assert!(valid.is_file());
        assert!(uppercase.is_file());
        assert!(malformed.is_file());
        assert!(directory.is_dir());
    }

    #[test]
    fn second_hard_link_on_temp_blocks_publication() {
        let (_temp, project) = setup_project();
        let alias = project.join(".ac").join("temp-alias");
        let hooks = TestFilesystemHooks {
            before_temp_validation: Some(Arc::new({
                let alias = alias.clone();
                move |temp| {
                    std::fs::hard_link(temp, &alias).expect("create adversarial temp hard link");
                }
            })),
            ..TestFilesystemHooks::default()
        };
        let mut guard =
            ProjectSeedManifestGuard::acquire_with_hooks(&project, Duration::from_secs(1), hooks)
                .unwrap();
        let outcome = guard.publication_permit().record_file(
            &ManifestActivationToken::for_test(),
            context_row(
                "Context.AgentsCommander.md",
                "context:agentscommander",
                "2026-07-16T19:40:07.123Z",
            )
            .unwrap(),
        );
        assert_eq!(
            outcome,
            ManifestRecordOutcome::PublishedUnrecorded(ManifestDegradedReason::UnsafePath)
        );
        assert!(!canonical_path(&project).exists());
        assert!(alias.is_file());
        guard.release();
    }

    #[test]
    fn raw_external_edit_conflict_preserves_external_bytes() {
        let (_temp, project) = setup_project();
        let activation = ManifestActivationToken::for_test();
        let mut initial = ProjectSeedManifestGuard::acquire(&project).unwrap();
        assert_eq!(
            initial.publication_permit().record_file(
                &activation,
                context_row(
                    "Context.AgentsCommander.md",
                    "context:agentscommander",
                    "2026-07-16T19:40:07.123Z",
                )
                .unwrap(),
            ),
            ManifestRecordOutcome::Recorded
        );
        initial.release();

        let external = b"external lock-unaware edit\n".to_vec();
        let hooks = TestFilesystemHooks {
            before_raw_conflict_check: Some(Arc::new({
                let external = external.clone();
                move |canonical| std::fs::write(canonical, &external).unwrap()
            })),
            ..TestFilesystemHooks::default()
        };
        let mut guard =
            ProjectSeedManifestGuard::acquire_with_hooks(&project, Duration::from_secs(1), hooks)
                .unwrap();
        let outcome = guard.publication_permit().record_file(
            &activation,
            context_row(
                "Context.coordinator.md",
                "context:coordinator",
                "2026-07-16T19:42:00.000Z",
            )
            .unwrap(),
        );
        assert_eq!(
            outcome,
            ManifestRecordOutcome::PublishedUnrecorded(ManifestDegradedReason::ExternalEdit)
        );
        guard.release();
        assert_eq!(std::fs::read(canonical_path(&project)).unwrap(), external);
    }

    // ---- #1318 v1 -> v2 coverage migration + catalog row -------------------

    /// A byte-exact v1 manifest (context row + replica-config row with distinct
    /// timestamps), exactly what an old build wrote.
    fn v1_manifest_fixture() -> String {
        concat!(
            "# Managed by AgentsCommander. Diagnostic only; never grants file ownership.\n",
            "schema_version = 1\n",
            "coverage_version = 1\n",
            "coverage = [\"project_context_templates\", \"replica_config_folders\"]\n",
            "\n",
            "[[files]]\n",
            "path = \".ac/Context.AgentsCommander.md\"\n",
            "path_encoding = \"utf8\"\n",
            "kind = \"project_context_template\"\n",
            "scope = \"context:agentscommander\"\n",
            "source = \"builtin\"\n",
            "last_seeded_at = \"2026-07-16T19:40:07.123Z\"\n",
            "\n",
            "[[files]]\n",
            "path = \".ac/wg-1-dev-team/__agent_alpha/.claude/settings.json\"\n",
            "path_encoding = \"utf8\"\n",
            "kind = \"replica_config_file\"\n",
            "scope = \"config:.ac/wg-1-dev-team/__agent_alpha/.claude\"\n",
            "source = \"workspace_base\"\n",
            "last_seeded_at = \"2026-07-16T19:41:12.456Z\"\n"
        )
        .to_string()
    }

    #[test]
    fn v1_manifest_upgrades_to_v2_preserving_rows_and_timestamps() {
        let (_temp, project) = setup_project();
        let path = canonical_path(&project);
        let v1 = v1_manifest_fixture();
        std::fs::write(&path, &v1).unwrap();

        // First acquire runs the one-shot upgrade under the held lock.
        let mut guard = ProjectSeedManifestGuard::acquire(&project).unwrap();
        let disk = std::fs::read_to_string(&path).unwrap();
        assert!(
            disk.contains("coverage_version = 2"),
            "upgraded manifest must declare coverage v2: {disk}"
        );
        assert!(
            disk.contains(
                "coverage = [\"project_context_templates\", \"replica_config_folders\", \"coding_agent_catalog\"]"
            ),
            "upgraded manifest must carry the 3-item v2 coverage: {disk}"
        );
        // Rows and timestamps preserved verbatim.
        let state = read_disk_state(&project);
        assert_eq!(state.rows.len(), 2);
        assert!(disk.contains("2026-07-16T19:40:07.123Z"));
        assert!(disk.contains("2026-07-16T19:41:12.456Z"));
        // The guard is a fully writable tracked snapshot: publications record.
        assert_eq!(
            guard.publication_permit().record_file(
                &ManifestActivationToken::for_test(),
                context_row(
                    "Context.coordinator.md",
                    "context:coordinator",
                    "2026-07-16T19:42:00.000Z",
                )
                .unwrap(),
            ),
            ManifestRecordOutcome::Recorded
        );
        guard.release();

        // Runs exactly once: a second acquire parses v2 strictly and never
        // re-enters the migration (bytes only change by the recorded row).
        let after_first = std::fs::read(&path).unwrap();
        let guard2 = ProjectSeedManifestGuard::acquire(&project).unwrap();
        guard2.release();
        assert_eq!(std::fs::read(&path).unwrap(), after_first);
        let state = read_disk_state(&project);
        assert_eq!(state.rows.len(), 3);
    }

    #[test]
    fn v1_with_different_coverage_list_stays_strictly_invalid() {
        let (_temp, project) = setup_project();
        let path = canonical_path(&project);
        // v1 header with a DIFFERENT (3-item, wrong order) coverage list.
        let v1 = v1_manifest_fixture().replace(
            "coverage = [\"project_context_templates\", \"replica_config_folders\"]",
            "coverage = [\"replica_config_folders\", \"project_context_templates\"]",
        );
        std::fs::write(&path, &v1).unwrap();

        let mut guard = ProjectSeedManifestGuard::acquire(&project).unwrap();
        assert_eq!(
            guard.publication_permit().record_file(
                &ManifestActivationToken::for_test(),
                context_row(
                    "Context.AgentsCommander.md",
                    "context:agentscommander",
                    "2026-07-16T19:40:07.123Z",
                )
                .unwrap(),
            ),
            ManifestRecordOutcome::PublishedUnrecorded(ManifestDegradedReason::InvalidCanonical)
        );
        guard.release();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            v1,
            "non-exact v1 shape stays byte-preserved"
        );
    }

    #[test]
    fn coverage_version_three_stays_invalid_and_non_migratable() {
        let (_temp, project) = setup_project();
        let path = canonical_path(&project);
        // v1 -> v2 -> the WRONG value for this build (3): the strict parse
        // rejects it and the migration must not touch it (not the exact v1
        // shape: coverage_version != 1).
        let wrong = v1_manifest_fixture().replace("coverage_version = 1", "coverage_version = 3");
        std::fs::write(&path, &wrong).unwrap();

        let mut guard = ProjectSeedManifestGuard::acquire(&project).unwrap();
        assert_eq!(
            guard.publication_permit().record_file(
                &ManifestActivationToken::for_test(),
                context_row(
                    "Context.AgentsCommander.md",
                    "context:agentscommander",
                    "2026-07-16T19:40:07.123Z",
                )
                .unwrap(),
            ),
            ManifestRecordOutcome::PublishedUnrecorded(ManifestDegradedReason::InvalidCanonical)
        );
        guard.release();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            wrong,
            "wrong coverage version stays byte-preserved"
        );
    }

    #[test]
    fn bound_violating_v1_file_stays_preserved_read_only() {
        let (_temp, project) = setup_project();
        let path = canonical_path(&project);
        // An over-long path field: the wire deserializers enforce MAX_FIELD_BYTES
        // on the strict parse (ResourceBound, never migratable) AND on the
        // migration's lenient pre-parse, so the file stays preserved either way.
        let overlong = format!("x{}\n", "a".repeat(MAX_FIELD_BYTES + 1));
        let v1 = format!(
            concat!(
                "# Managed by AgentsCommander. Diagnostic only; never grants file ownership.\n",
                "schema_version = 1\n",
                "coverage_version = 1\n",
                "coverage = [\"project_context_templates\", \"replica_config_folders\"]\n",
                "\n",
                "[[files]]\n",
                "path = \"{}\"\n",
                "path_encoding = \"utf8\"\n",
                "kind = \"project_context_template\"\n",
                "scope = \"context:agentscommander\"\n",
                "source = \"builtin\"\n",
                "last_seeded_at = \"2026-07-16T19:40:07.123Z\"\n"
            ),
            overlong
        );
        std::fs::write(&path, &v1).unwrap();

        let mut guard = ProjectSeedManifestGuard::acquire(&project).unwrap();
        // The strict parse classifies the over-long path as a parse failure on
        // this TOML surface (the lenient pre-parse fails the same way), and the
        // migration is never entered; either way the contract is preservation
        // with the writer disabled.
        let outcome = guard.publication_permit().record_file(
            &ManifestActivationToken::for_test(),
            context_row(
                "Context.AgentsCommander.md",
                "context:agentscommander",
                "2026-07-16T19:40:07.123Z",
            )
            .unwrap(),
        );
        assert!(matches!(
            outcome,
            ManifestRecordOutcome::PublishedUnrecorded(
                ManifestDegradedReason::InvalidCanonical | ManifestDegradedReason::ResourceBound(_)
            )
        ));
        guard.release();
        assert_eq!(
            std::fs::read(&path).unwrap(),
            v1.as_bytes(),
            "bound-violating v1 file stays byte-preserved"
        );
    }

    #[test]
    fn future_schema_snapshot_never_migrated() {
        let (_temp, project) = setup_project();
        let path = canonical_path(&project);
        let future = concat!(
            "# Managed by AgentsCommander. Diagnostic only; never grants file ownership.\n",
            "schema_version = 2\n",
            "coverage_version = 1\n",
            "coverage = [\"project_context_templates\", \"replica_config_folders\"]\n",
            "files = []\n"
        );
        std::fs::write(&path, future).unwrap();

        let mut guard = ProjectSeedManifestGuard::acquire(&project).unwrap();
        assert_eq!(
            guard.publication_permit().record_file(
                &ManifestActivationToken::for_test(),
                context_row(
                    "Context.AgentsCommander.md",
                    "context:agentscommander",
                    "2026-07-16T19:40:07.123Z",
                )
                .unwrap(),
            ),
            ManifestRecordOutcome::PublishedUnrecorded(ManifestDegradedReason::FutureSchema)
        );
        guard.release();
        assert_eq!(
            std::fs::read(&path).unwrap(),
            future.as_bytes(),
            "future-schema manifest stays byte-preserved"
        );
    }

    #[test]
    fn external_edit_conflict_during_upgrade_preserves_v1_bytes() {
        let (_temp, project) = setup_project();
        let path = canonical_path(&project);
        std::fs::write(&path, v1_manifest_fixture()).unwrap();

        // A lock-unaware external writer replaces the file between the
        // migration's re-read and its atomic write: `write_canonical`'s
        // raw-conflict check must fail and preserve the external bytes.
        let external = b"external lock-unaware edit\n".to_vec();
        let hooks = TestFilesystemHooks {
            before_raw_conflict_check: Some(Arc::new({
                let external = external.clone();
                move |canonical| std::fs::write(canonical, &external).unwrap()
            })),
            ..TestFilesystemHooks::default()
        };
        let mut guard =
            ProjectSeedManifestGuard::acquire_with_hooks(&project, Duration::from_secs(1), hooks)
                .unwrap();
        // The migration's own write fails the raw-conflict check and restores the
        // pre-migration degraded snapshot (InvalidCanonical), so the next
        // publication reports the restored reason; the load-bearing assertion is
        // that the external bytes win and no partial v2 upgrade ever lands.
        assert_eq!(
            guard.publication_permit().record_file(
                &ManifestActivationToken::for_test(),
                context_row(
                    "Context.coordinator.md",
                    "context:coordinator",
                    "2026-07-16T19:42:00.000Z",
                )
                .unwrap(),
            ),
            ManifestRecordOutcome::PublishedUnrecorded(ManifestDegradedReason::InvalidCanonical)
        );
        guard.release();
        assert_eq!(
            std::fs::read(&path).unwrap(),
            external,
            "the external edit wins; v1 bytes never partially upgraded"
        );
    }

    #[test]
    fn coding_agent_catalog_row_requires_exact_path_scope_source_encoding() {
        let at = timestamp("2026-07-16T19:43:00.000Z");
        let ok = PublishedManifestRow::coding_agent_catalog(
            ManifestPathIdentity::parse(
                ManifestPathEncoding::Utf8,
                ".ac/coding-agents/agents.json".to_string(),
            )
            .unwrap(),
            at,
        )
        .expect("exact catalog row is accepted");
        assert_eq!(ok.kind, ManifestFileKind::CodingAgentCatalog);
        assert_eq!(ok.scope, "catalog:coding-agents");
        assert_eq!(ok.source, ManifestSource::Builtin);
        assert_eq!(ok.path.serialized, ".ac/coding-agents/agents.json");
        assert_eq!(ok.path.encoding, ManifestPathEncoding::Utf8);

        // Wrong path -> rejected.
        assert!(PublishedManifestRow::coding_agent_catalog(
            ManifestPathIdentity::parse(
                ManifestPathEncoding::Utf8,
                ".ac/coding-agents/other.json".to_string(),
            )
            .unwrap(),
            at,
        )
        .is_err());
        // Non-utf8 encoding -> rejected. A WindowsUtf16Hex wire shape only
        // parses for paths that are NOT valid Unicode (e.g. an unpaired
        // surrogate), exactly like the existing round-trip fixtures.
        let mut windows_units: Vec<u16> = ".ac/coding-agents/agents.json".encode_utf16().collect();
        windows_units.push(0xd800);
        let windows_hex = windows_units
            .iter()
            .map(|unit| format!("{unit:04x}"))
            .collect::<String>();
        assert!(PublishedManifestRow::coding_agent_catalog(
            ManifestPathIdentity::parse(ManifestPathEncoding::WindowsUtf16Hex, windows_hex)
                .unwrap(),
            at,
        )
        .is_err());

        // A catalog row records under a held gate, alongside context rows.
        let (_temp, project) = setup_project();
        let mut guard = ProjectSeedManifestGuard::acquire(&project).unwrap();
        assert_eq!(
            guard.publication_permit().record_file(
                &ManifestActivationToken::for_test(),
                context_row(
                    "Context.AgentsCommander.md",
                    "context:agentscommander",
                    "2026-07-16T19:40:07.123Z",
                )
                .unwrap(),
            ),
            ManifestRecordOutcome::Recorded
        );
        assert_eq!(
            guard.publication_permit().record_file(
                &ManifestActivationToken::for_test(),
                PublishedManifestRow::coding_agent_catalog(
                    ManifestPathIdentity::parse(
                        ManifestPathEncoding::Utf8,
                        ".ac/coding-agents/agents.json".to_string(),
                    )
                    .unwrap(),
                    at,
                )
                .unwrap(),
            ),
            ManifestRecordOutcome::Recorded
        );
        guard.release();
        let disk = std::fs::read_to_string(canonical_path(&project)).unwrap();
        assert!(disk.contains("kind = \"coding_agent_catalog\""));
        assert!(disk.contains("scope = \"catalog:coding-agents\""));
        assert!(disk.contains("path = \".ac/coding-agents/agents.json\""));
        assert!(disk.contains("source = \"builtin\""));
    }

    #[test]
    fn lock_identity_replacement_is_detected() {
        let (_temp, project) = setup_project();
        let guard = ProjectSeedManifestGuard::acquire(&project).unwrap();
        let lock_path = project.join(".ac").join(SEED_MANIFEST_LOCK_FILENAME);
        std::fs::remove_file(&lock_path).unwrap();
        std::fs::write(&lock_path, b"replacement").unwrap();
        assert!(matches!(
            guard.revalidate_owner(),
            Err(SeedManifestError::UnsafePath { .. })
        ));
        guard.release();
    }

    #[test]
    fn pinned_owner_chain_detects_replacement() {
        let (_temp, project) = setup_project();
        let ac = project.join(".ac");
        let owner = ac.join("wg-1-team").join("__agent_alpha");
        std::fs::create_dir_all(&owner).unwrap();
        let guard = ProjectSeedManifestGuard::acquire(&project).unwrap();
        let chain = guard
            .pin_owner_chain_from_ac(Path::new("wg-1-team/__agent_alpha"))
            .unwrap();
        #[cfg(not(windows))]
        {
            std::fs::rename(ac.join("wg-1-team"), ac.join("wg-old")).unwrap();
            std::fs::create_dir_all(&owner).unwrap();
        }
        #[cfg(windows)]
        let chain = {
            let mut chain = chain;
            chain.directories[0].identity.file ^= 1;
            chain
        };
        assert!(matches!(
            chain.revalidate(),
            Err(SeedManifestError::UnsafePath { .. })
        ));
        guard.release();
    }

    #[test]
    fn upsert_and_same_millisecond_noop_use_atomic_writer_once() {
        let (_temp, project) = setup_project();
        let activation = ManifestActivationToken::for_test();
        let row = context_row(
            "Context.AgentsCommander.md",
            "context:agentscommander",
            "2026-07-16T19:40:07.123Z",
        )
        .unwrap();
        let mut guard = ProjectSeedManifestGuard::acquire(&project).unwrap();
        assert_eq!(
            guard
                .publication_permit()
                .record_file(&activation, row.clone()),
            ManifestRecordOutcome::Recorded
        );
        let first = std::fs::read(canonical_path(&project)).unwrap();
        assert_eq!(
            guard.publication_permit().record_file(&activation, row),
            ManifestRecordOutcome::Unchanged
        );
        assert_eq!(std::fs::read(canonical_path(&project)).unwrap(), first);

        let later = context_row(
            "Context.AgentsCommander.md",
            "context:agentscommander",
            "2026-07-16T19:40:07.124Z",
        )
        .unwrap();
        assert_eq!(
            guard.publication_permit().record_file(&activation, later),
            ManifestRecordOutcome::Recorded
        );
        guard.release();
        assert!(
            String::from_utf8(std::fs::read(canonical_path(&project)).unwrap())
                .unwrap()
                .contains("2026-07-16T19:40:07.124Z")
        );
    }

    #[test]
    fn exact_scope_replacement_removes_omitted_rows_and_empty_scope() {
        let (_temp, project) = setup_project();
        let activation = ManifestActivationToken::for_test();
        let scope = config_scope("wg-1-team", "alpha", ".claude");
        let first = PublishedScopeBatch::new(
            scope.clone(),
            ManifestSource::WorkspaceBase,
            vec![
                config_path("wg-1-team", "alpha", ".claude", "a"),
                config_path("wg-1-team", "alpha", ".claude", "b"),
            ],
            timestamp("2026-07-16T19:41:12.456Z"),
        )
        .unwrap();
        let second = PublishedScopeBatch::new(
            scope.clone(),
            ManifestSource::MatrixBase,
            vec![
                config_path("wg-1-team", "alpha", ".claude", "b"),
                config_path("wg-1-team", "alpha", ".claude", "c"),
            ],
            timestamp("2026-07-16T19:42:12.456Z"),
        )
        .unwrap();
        let mut guard = ProjectSeedManifestGuard::acquire(&project).unwrap();
        assert_eq!(
            guard.publication_permit().replace_scope(&activation, first),
            ManifestRecordOutcome::Recorded
        );
        assert_eq!(
            guard
                .publication_permit()
                .replace_scope(&activation, second),
            ManifestRecordOutcome::Recorded
        );
        let paths = match &guard.snapshot {
            CanonicalSnapshot::Writable { state, .. } => state
                .rows
                .values()
                .map(|row| row.path.serialized().to_string())
                .collect::<Vec<_>>(),
            CanonicalSnapshot::ReadOnly { .. } => panic!("writable snapshot expected"),
        };
        assert_eq!(
            paths,
            vec![
                ".ac/wg-1-team/__agent_alpha/.claude/b".to_string(),
                ".ac/wg-1-team/__agent_alpha/.claude/c".to_string(),
            ]
        );
        let empty = PublishedScopeBatch::new(
            scope,
            ManifestSource::MatrixBase,
            Vec::new(),
            timestamp("2026-07-16T19:43:12.456Z"),
        )
        .unwrap();
        assert_eq!(
            guard.publication_permit().replace_scope(&activation, empty),
            ManifestRecordOutcome::Recorded
        );
        guard.release();
        assert!(read_disk_state(&project).rows.is_empty());
    }

    #[test]
    fn empty_scope_against_absence_is_noop_without_header_file() {
        let (_temp, project) = setup_project();
        let mut guard = ProjectSeedManifestGuard::acquire(&project).unwrap();
        let batch = PublishedScopeBatch::new(
            config_scope("wg-1-team", "alpha", ".claude"),
            ManifestSource::WorkspaceBase,
            Vec::new(),
            timestamp("2026-07-16T19:41:12.456Z"),
        )
        .unwrap();
        assert_eq!(
            guard
                .publication_permit()
                .replace_scope(&ManifestActivationToken::for_test(), batch),
            ManifestRecordOutcome::Unchanged
        );
        guard.release();
        assert!(!canonical_path(&project).exists());
    }

    #[test]
    fn overbound_publication_removes_prior_scope_without_partial_rows() {
        let (_temp, project) = setup_project();
        let activation = ManifestActivationToken::for_test();
        let scope = config_scope("wg-1-team", "alpha", ".claude");
        let batch = PublishedScopeBatch::new(
            scope.clone(),
            ManifestSource::WorkspaceBase,
            vec![config_path(
                "wg-1-team",
                "alpha",
                ".claude",
                "settings.json",
            )],
            timestamp("2026-07-16T19:41:12.456Z"),
        )
        .unwrap();
        let mut guard = ProjectSeedManifestGuard::acquire(&project).unwrap();
        assert_eq!(
            guard.publication_permit().replace_scope(&activation, batch),
            ManifestRecordOutcome::Recorded
        );
        assert_eq!(
            guard.publication_permit().remove_unrecordable_scope(
                &activation,
                scope,
                ResourceBoundKind::Rows,
            ),
            ManifestRecordOutcome::PublishedUnrecorded(ManifestDegradedReason::ResourceBound(
                ResourceBoundKind::Rows
            ))
        );
        guard.release();
        assert!(read_disk_state(&project).rows.is_empty());
    }

    #[test]
    fn failed_overbound_scope_removal_reports_the_persistence_failure() {
        let (_temp, project) = setup_project();
        let activation = ManifestActivationToken::for_test();
        let scope = config_scope("wg-1-team", "alpha", ".claude");
        let batch = PublishedScopeBatch::new(
            scope.clone(),
            ManifestSource::WorkspaceBase,
            vec![config_path(
                "wg-1-team",
                "alpha",
                ".claude",
                "settings.json",
            )],
            timestamp("2026-07-16T19:41:12.456Z"),
        )
        .unwrap();
        let mut initial = ProjectSeedManifestGuard::acquire(&project).unwrap();
        assert_eq!(
            initial
                .publication_permit()
                .replace_scope(&activation, batch),
            ManifestRecordOutcome::Recorded
        );
        initial.release();

        let external = b"lock-unaware external edit".to_vec();
        let hook_bytes = external.clone();
        let hooks = TestFilesystemHooks {
            before_raw_conflict_check: Some(Arc::new(move |canonical| {
                std::fs::write(canonical, &hook_bytes).expect("write external edit");
            })),
            ..TestFilesystemHooks::default()
        };
        let mut guard =
            ProjectSeedManifestGuard::acquire_with_hooks(&project, DEFAULT_LOCK_TIMEOUT, hooks)
                .unwrap();
        assert_eq!(
            guard.publication_permit().remove_unrecordable_scope(
                &activation,
                scope,
                ResourceBoundKind::Rows,
            ),
            ManifestRecordOutcome::PublishedUnrecorded(ManifestDegradedReason::ExternalEdit)
        );
        guard.release();
        assert_eq!(std::fs::read(canonical_path(&project)).unwrap(), external);
    }

    #[test]
    fn lifecycle_filters_are_component_wise_for_foreign_encoded_rows() {
        let mut bytes = b".ac/wg-1-team/__agent_alpha/.claude/foreign".to_vec();
        bytes.push(0xff);
        let foreign = ManifestPathIdentity::parse(
            ManifestPathEncoding::UnixBytesHex,
            encode_lower_hex_bytes(&bytes),
        )
        .unwrap();
        let mut state = ManifestState::default();
        let _ = state.upsert(
            PublishedManifestRow::replica_config(
                foreign,
                config_scope("wg-1-team", "alpha", ".claude"),
                ManifestSource::WorkspaceBase,
                timestamp("2026-07-16T19:41:12.456Z"),
            )
            .unwrap(),
        );
        let _ = state.upsert(
            PublishedManifestRow::replica_config(
                config_path("wg-2-team", "beta", ".claude", "keep"),
                config_scope("wg-2-team", "beta", ".claude"),
                ManifestSource::WorkspaceBase,
                timestamp("2026-07-16T19:41:12.456Z"),
            )
            .unwrap(),
        );
        let filter = ManifestLifecycleFilter::config_path_prefix(
            ManifestPathIdentity::parse(
                ManifestPathEncoding::Utf8,
                ".ac/wg-1-team/__agent_alpha".to_string(),
            )
            .unwrap(),
        )
        .unwrap();
        let journal = state.remove_matching(|row| filter.matches(row));
        assert!(journal.changed());
        assert_eq!(state.rows.len(), 1);
        assert!(state
            .rows
            .values()
            .next()
            .unwrap()
            .path
            .serialized()
            .contains("wg-2-team"));
    }

    #[test]
    fn degraded_permit_never_records_even_with_an_activation_token() {
        let permit = ProjectPublicationPermit::degraded_without_guard(
            ManifestDegradedReason::LockUnavailable,
        );
        assert!(!permit.is_tracked());
        let outcome = permit.record_file(
            &ManifestActivationToken::production(),
            context_row(
                "Context.AgentsCommander.md",
                "context:agentscommander",
                "2026-07-16T19:40:07.123Z",
            )
            .unwrap(),
        );
        assert_eq!(
            outcome,
            ManifestRecordOutcome::PublishedUnrecorded(ManifestDegradedReason::LockUnavailable)
        );
    }

    #[test]
    fn production_activation_token_records_through_a_tracked_permit() {
        // Stage F: the production constructor is a real, non-test activation path.
        // A tracked permit driven by it records exactly like the test token, so
        // production emission is genuinely wired and not a dormant no-op.
        let (_temp, project) = setup_project();
        let mut guard = ProjectSeedManifestGuard::acquire(&project).unwrap();
        let outcome = guard.publication_permit().record_file(
            &ManifestActivationToken::production(),
            context_row(
                "Context.AgentsCommander.md",
                "context:agentscommander",
                "2026-07-16T19:40:07.123Z",
            )
            .unwrap(),
        );
        assert_eq!(outcome, ManifestRecordOutcome::Recorded);
        guard.release();
        let disk = read_disk_state(&project);
        assert_eq!(disk.rows.len(), 1);
    }

    #[test]
    fn v1_coverage_declaration_is_exhaustive() {
        // The declaration must name every coverage boundary exactly
        // once. The exhaustive match makes adding a `V1CoverageBoundary` variant
        // without listing it here (or vice versa) a compile/count failure, so the
        // Stage F coverage checklist cannot silently drop a boundary.
        assert_eq!(V1_COVERAGE_BOUNDARIES.len(), 17);
        for boundary in V1_COVERAGE_BOUNDARIES {
            // Exhaustive match: a new variant forces an update here.
            match boundary {
                V1CoverageBoundary::DirectCreateAcProjectFreshRoot
                | V1CoverageBoundary::ContextCreate
                | V1CoverageBoundary::ContextUpdate
                | V1CoverageBoundary::ContextSelfHeal
                | V1CoverageBoundary::ContextOverwrite
                | V1CoverageBoundary::CoordinatorStatelessV2ToV4
                | V1CoverageBoundary::CoordinatorStatelessV3ToV4
                | V1CoverageBoundary::CoordinatorSeededV3ToV4
                | V1CoverageBoundary::ConfigExactPublish
                | V1CoverageBoundary::ConfigOverBoundPublish
                | V1CoverageBoundary::ConfigFailedRestore
                | V1CoverageBoundary::LifecycleReplicaRemoval
                | V1CoverageBoundary::LifecycleWorkgroupRemoval
                | V1CoverageBoundary::LifecycleTeamDeletion
                | V1CoverageBoundary::LifecycleAgentMatrixDeletion
                | V1CoverageBoundary::PendingInclusiveDeleteProtection
                | V1CoverageBoundary::CatalogSeed => {}
            }
        }
        // No duplicates: every declared boundary is distinct.
        for (index, boundary) in V1_COVERAGE_BOUNDARIES.iter().enumerate() {
            assert!(
                !V1_COVERAGE_BOUNDARIES[index + 1..].contains(boundary),
                "duplicate coverage boundary {boundary:?}"
            );
        }
    }

    #[test]
    fn exact_temp_name_requires_lowercase_hyphenated_uuid() {
        assert!(is_exact_temp_name(
            ".seed-manifest.00000000-0000-0000-0000-000000000000.tmp"
        ));
        assert!(!is_exact_temp_name(
            ".seed-manifest.00000000000000000000000000000000.tmp"
        ));
        assert!(!is_exact_temp_name(
            ".seed-manifest.00000000-0000-0000-0000-00000000000A.tmp"
        ));
        assert!(!is_exact_temp_name(".seed-manifest.not-a-uuid.tmp"));
    }

    #[test]
    fn prior_temp_inventory_is_hard_bounded_and_preserves_uninspected_entries() {
        let (_temp, project) = setup_project();
        let ac = project.join(".ac");
        let candidate_count = MAX_TEMP_INVENTORY_ENTRIES + 5;
        for index in 0..candidate_count {
            let name = format!(
                "{SEED_MANIFEST_TEMP_PREFIX}{}{SEED_MANIFEST_TEMP_SUFFIX}",
                Uuid::from_u128(u128::try_from(index + 1).unwrap()).hyphenated()
            );
            std::fs::write(ac.join(name), b"candidate").unwrap();
        }
        for index in 0..(MAX_TEMP_DIAGNOSTIC_SAMPLES + 5) {
            std::fs::write(
                ac.join(format!(".seed-manifest.malformed-{index}.tmp")),
                b"lookalike",
            )
            .unwrap();
        }

        let guard = ProjectSeedManifestGuard::acquire(&project).unwrap();
        let inventory = guard.inventory_prior_temps();
        assert!(inventory.scan_truncated);
        assert!(inventory.exact_count > 0);
        assert!(
            inventory
                .exact_count
                .saturating_add(inventory.malformed_count)
                <= u64::try_from(MAX_TEMP_INVENTORY_ENTRIES).unwrap()
        );
        assert!(inventory.exact_names.len() <= MAX_TEMP_INVENTORY_ENTRIES);
        assert!(inventory.exact_samples.len() <= MAX_TEMP_DIAGNOSTIC_SAMPLES);
        assert!(inventory.malformed_samples.len() <= MAX_TEMP_DIAGNOSTIC_SAMPLES);
        guard.release();
        assert_eq!(exact_temp_paths(&project).len(), candidate_count);
    }

    #[test]
    fn prior_temp_inventory_counts_and_samples_malformed_names_exactly() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let (_temp, project) = setup_project();
        let ac = project.join(".ac");
        for index in 0..3_u128 {
            let name = format!(
                "{SEED_MANIFEST_TEMP_PREFIX}{}{SEED_MANIFEST_TEMP_SUFFIX}",
                Uuid::from_u128(index + 1).hyphenated()
            );
            std::fs::write(ac.join(name), b"candidate").unwrap();
        }
        let malformed_count = MAX_TEMP_DIAGNOSTIC_SAMPLES + 5;
        for index in 0..malformed_count {
            std::fs::write(
                ac.join(format!(".seed-manifest.malformed-{index}.tmp")),
                b"lookalike",
            )
            .unwrap();
        }

        let diagnostic_count = Arc::new(AtomicUsize::new(0));
        let hooks = TestFilesystemHooks {
            on_prior_temp_diagnostic: Some(Arc::new({
                let diagnostic_count = Arc::clone(&diagnostic_count);
                move |inventory| {
                    diagnostic_count.fetch_add(1, Ordering::SeqCst);
                    assert_eq!(inventory.exact_count, 3);
                    assert_eq!(
                        inventory.malformed_count,
                        u64::try_from(malformed_count).unwrap()
                    );
                    assert_eq!(inventory.exact_samples.len(), 3);
                    assert_eq!(
                        inventory.malformed_samples.len(),
                        MAX_TEMP_DIAGNOSTIC_SAMPLES
                    );
                }
            })),
            ..TestFilesystemHooks::default()
        };
        let guard =
            ProjectSeedManifestGuard::acquire_with_hooks(&project, Duration::from_secs(1), hooks)
                .unwrap();
        assert_eq!(diagnostic_count.load(Ordering::SeqCst), 1);

        let inventory = guard.inventory_prior_temps();
        assert!(!inventory.scan_truncated);
        assert!(inventory.scan_error.is_none());
        assert_eq!(inventory.entry_errors, 0);
        assert_eq!(inventory.exact_count, 3);
        assert_eq!(
            inventory.malformed_count,
            u64::try_from(malformed_count).unwrap()
        );
        assert_eq!(inventory.malformed_native_count, 0);
        assert_eq!(inventory.exact_names.len(), 3);
        assert_eq!(inventory.exact_samples.len(), 3);
        assert_eq!(
            inventory.malformed_samples.len(),
            MAX_TEMP_DIAGNOSTIC_SAMPLES
        );
        assert!(inventory
            .malformed_samples
            .iter()
            .all(|sample| sample.starts_with(SEED_MANIFEST_TEMP_PREFIX)
                && sample.ends_with(SEED_MANIFEST_TEMP_SUFFIX)));
        guard.release();
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn native_non_unicode_temp_shape_is_counted_without_lossy_materialization() {
        let (_temp, project) = setup_project();
        let guard = ProjectSeedManifestGuard::acquire(&project).unwrap();

        #[cfg(unix)]
        let native_name = {
            use std::os::unix::ffi::OsStringExt;

            let mut bytes = SEED_MANIFEST_TEMP_PREFIX.as_bytes().to_vec();
            bytes.push(0xff);
            bytes.extend_from_slice(SEED_MANIFEST_TEMP_SUFFIX.as_bytes());
            std::ffi::OsString::from_vec(bytes)
        };
        #[cfg(windows)]
        let native_name = {
            use std::os::windows::ffi::OsStringExt;

            let mut units = SEED_MANIFEST_TEMP_PREFIX.encode_utf16().collect::<Vec<_>>();
            units.push(0xd800);
            units.extend(SEED_MANIFEST_TEMP_SUFFIX.encode_utf16());
            std::ffi::OsString::from_wide(&units)
        };

        assert!(native_name.to_str().is_none());
        std::fs::write(project.join(".ac").join(&native_name), b"lookalike").unwrap();
        let inventory = guard.inventory_prior_temps();
        assert!(!inventory.scan_truncated);
        assert_eq!(inventory.malformed_count, 1);
        assert_eq!(inventory.malformed_native_count, 1);
        assert_eq!(
            inventory.malformed_samples,
            vec!["<native-non-unicode-temp-name>".to_string()]
        );
        assert!(!inventory
            .malformed_samples
            .iter()
            .any(|sample| sample.contains('\u{fffd}')));
        guard.release();
    }

    #[cfg(windows)]
    #[test]
    fn open_read_write_temp_handle_conflicts_with_replace_even_with_delete_access() {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Foundation::{
            GetLastError, ERROR_SHARING_VIOLATION, GENERIC_READ, GENERIC_WRITE,
        };
        use windows_sys::Win32::Storage::FileSystem::{
            ReplaceFileW, DELETE, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE, FILE_SHARE_READ,
            FILE_SHARE_WRITE,
        };

        let (_temp, project) = setup_project();
        let canonical = canonical_path(&project);
        std::fs::write(&canonical, b"old canonical").unwrap();
        for include_delete in [false, true] {
            let temp_path = project.join(".ac").join(format!(
                ".seed-manifest.{}-writer-conflict.tmp",
                if include_delete { "delete" } else { "ordinary" }
            ));
            let file = if include_delete {
                OpenOptions::new()
                    .read(true)
                    .write(true)
                    .access_mode(GENERIC_READ | GENERIC_WRITE | DELETE)
                    .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
                    .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
                    .create_new(true)
                    .open(&temp_path)
                    .unwrap()
            } else {
                open_regular_no_follow(&temp_path, true, true).unwrap()
            };
            let mut opened = OpenedRegularFile::from_file(&temp_path, file).unwrap();
            let replacement = b"replacement";
            opened.file.write_all(replacement).unwrap();
            opened.file.sync_all().unwrap();
            opened.refresh_facts().unwrap();
            let temp_wide = absolute_verbatim_utf16(&temp_path).unwrap();
            let canonical_wide = absolute_verbatim_utf16(&canonical).unwrap();
            let success = unsafe {
                ReplaceFileW(
                    canonical_wide.as_ptr(),
                    temp_wide.as_ptr(),
                    std::ptr::null(),
                    0,
                    std::ptr::null(),
                    std::ptr::null(),
                )
            };
            assert_eq!(success, 0);
            let raw_error = unsafe { GetLastError() };
            assert_eq!(raw_error, ERROR_SHARING_VIOLATION);
            assert_eq!(std::fs::read(&canonical).unwrap(), b"old canonical");
            assert_eq!(std::fs::read(&temp_path).unwrap(), replacement);
            cleanup_current_owned_temp(&opened, Some(replacement)).unwrap();
            drop(opened);
            assert!(!temp_path.exists());
        }
    }

    #[cfg(windows)]
    #[test]
    fn zero_access_witness_replace_preserves_old_handle_identity_with_zero_links() {
        let (_temp, project) = setup_project();
        let activation = ManifestActivationToken::for_test();
        let mut first = ProjectSeedManifestGuard::acquire(&project).unwrap();
        assert_eq!(
            first.publication_permit().record_file(
                &activation,
                context_row(
                    "Context.AgentsCommander.md",
                    "context:agentscommander",
                    "2026-07-16T19:40:07.123Z",
                )
                .unwrap(),
            ),
            ManifestRecordOutcome::Recorded
        );
        first.release();

        let canonical = canonical_path(&project);
        let old = OpenedRegularFile::open_existing(&canonical, false).unwrap();
        let old_identity = old.identity;
        let mut second = ProjectSeedManifestGuard::acquire(&project).unwrap();
        assert_eq!(
            second.publication_permit().record_file(
                &activation,
                context_row(
                    "Context.coordinator.md",
                    "context:coordinator",
                    "2026-07-16T19:42:00.000Z",
                )
                .unwrap(),
            ),
            ManifestRecordOutcome::Recorded
        );
        second.release();

        let final_file = OpenedRegularFile::open_existing(&canonical, false).unwrap();
        assert_ne!(final_file.identity, old_identity);
        let old_facts = handle_facts(&old.file).unwrap();
        assert_eq!(old_facts.identity, old_identity);
        assert_eq!(old_facts.links, 0);
    }

    #[cfg(windows)]
    #[test]
    fn destination_share_denial_returns_typed_error_and_preserves_both_states() {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Foundation::ERROR_SHARING_VIOLATION;
        use windows_sys::Win32::Storage::FileSystem::{FILE_SHARE_READ, FILE_SHARE_WRITE};

        let (_temp, project) = setup_project();
        let activation = ManifestActivationToken::for_test();
        let mut initial = ProjectSeedManifestGuard::acquire(&project).unwrap();
        assert_eq!(
            initial.publication_permit().record_file(
                &activation,
                context_row(
                    "Context.AgentsCommander.md",
                    "context:agentscommander",
                    "2026-07-16T19:40:07.123Z",
                )
                .unwrap(),
            ),
            ManifestRecordOutcome::Recorded
        );
        initial.release();

        let canonical = canonical_path(&project);
        let original = std::fs::read(&canonical).unwrap();
        let mut desired = read_disk_state(&project);
        let _ = desired.upsert(
            context_row(
                "Context.coordinator.md",
                "context:coordinator",
                "2026-07-16T19:42:00.000Z",
            )
            .unwrap(),
        );
        let desired_bytes = serialize_state(&desired).unwrap();
        let mut guard = ProjectSeedManifestGuard::acquire(&project).unwrap();
        let blocker = OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
            .open(&canonical)
            .unwrap();

        let error = guard.write_canonical(desired_bytes).unwrap_err();
        match &error {
            SeedManifestError::WindowsNamespaceFailure {
                operation,
                raw_error,
                canonical_state,
                temp_state,
                ..
            } => {
                assert_eq!(*operation, WindowsNamespaceOperation::ReplaceFile);
                assert_eq!(*raw_error, ERROR_SHARING_VIOLATION);
                assert!(matches!(
                    canonical_state.as_ref(),
                    WindowsPathState::Same {
                        bytes_match: true,
                        ..
                    }
                ));
                assert!(matches!(
                    temp_state.as_ref(),
                    WindowsPathState::Same {
                        links: 1,
                        bytes_match: true,
                        ..
                    }
                ));
            }
            other => panic!("expected typed stable-state namespace failure, got {other:?}"),
        }
        assert_eq!(
            error.degraded_reason(),
            ManifestDegradedReason::PersistenceFailure
        );
        guard.release();
        drop(blocker);
        assert_eq!(std::fs::read(&canonical).unwrap(), original);
        assert!(exact_temp_paths(&project).is_empty());
    }

    #[cfg(windows)]
    #[test]
    fn writer_supports_non_ascii_verbatim_paths_beyond_max_path() {
        use std::os::windows::ffi::OsStrExt;

        let temp = tempfile::tempdir().unwrap();
        let mut project = temp.path().join("project");
        while project.as_os_str().encode_wide().count() < 430 {
            project.push("長い-path-component-0123456789abcdef");
        }
        std::fs::create_dir_all(project.join(".ac")).unwrap();
        let mut guard = ProjectSeedManifestGuard::acquire(&project).unwrap();
        assert_eq!(
            guard.publication_permit().record_file(
                &ManifestActivationToken::for_test(),
                context_row(
                    "Context.AgentsCommander.md",
                    "context:agentscommander",
                    "2026-07-16T19:40:07.123Z",
                )
                .unwrap(),
            ),
            ManifestRecordOutcome::Recorded
        );
        guard.release();
        assert!(canonical_path(&project).is_file());
    }

    #[cfg(windows)]
    #[test]
    fn verbatim_conversion_covers_long_unc_paths_and_rejects_interior_nul() {
        use std::os::windows::ffi::OsStringExt;

        let unc = PathBuf::from(format!(
            "\\\\server\\share\\{}",
            "長い-unc-component-0123456789abcdef\\".repeat(12)
        ));
        let converted = absolute_verbatim_utf16(&unc).unwrap();
        let expected_prefix = "\\\\?\\UNC\\server\\share\\"
            .encode_utf16()
            .collect::<Vec<_>>();
        assert!(converted.starts_with(&expected_prefix));
        assert_eq!(converted.last(), Some(&0));
        assert!(converted.len() > 260);

        let with_nul = PathBuf::from(std::ffi::OsString::from_wide(&[
            u16::from(b'C'),
            u16::from(b':'),
            u16::from(b'\\'),
            u16::from(b'a'),
            0,
            u16::from(b'b'),
        ]));
        let error = absolute_verbatim_utf16(&with_nul).unwrap_err();
        assert!(matches!(
            error,
            SeedManifestError::Validation(message)
                if message == "Windows namespace path contains an interior NUL"
        ));
    }

    #[cfg(windows)]
    #[test]
    fn windows_namespace_power_loss_durability_is_explicitly_unclaimed() {
        assert!(!std::hint::black_box(
            WINDOWS_NAMESPACE_POWER_LOSS_DURABILITY_CLAIMED
        ));
    }

    #[cfg(windows)]
    #[test]
    fn named_stream_injected_into_temp_aborts_and_preserves_candidate() {
        let (_temp, project) = setup_project();
        let hooks = TestFilesystemHooks {
            before_temp_validation: Some(Arc::new(|temp| {
                std::fs::write(windows_ads_path(temp, "injected"), b"foreign stream").unwrap();
            })),
            ..TestFilesystemHooks::default()
        };
        let mut guard =
            ProjectSeedManifestGuard::acquire_with_hooks(&project, Duration::from_secs(1), hooks)
                .unwrap();
        let outcome = guard.publication_permit().record_file(
            &ManifestActivationToken::for_test(),
            context_row(
                "Context.AgentsCommander.md",
                "context:agentscommander",
                "2026-07-16T19:40:07.123Z",
            )
            .unwrap(),
        );
        assert_eq!(
            outcome,
            ManifestRecordOutcome::PublishedUnrecorded(ManifestDegradedReason::UnsafePath)
        );
        guard.release();
        assert!(!canonical_path(&project).exists());
        assert_eq!(exact_temp_paths(&project).len(), 1);
    }

    #[cfg(windows)]
    #[test]
    fn read_only_canonical_is_preserved_without_creating_a_temp() {
        use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_READONLY;

        let (_temp, project) = setup_project();
        let activation = ManifestActivationToken::for_test();
        let mut first = ProjectSeedManifestGuard::acquire(&project).unwrap();
        assert_eq!(
            first.publication_permit().record_file(
                &activation,
                context_row(
                    "Context.AgentsCommander.md",
                    "context:agentscommander",
                    "2026-07-16T19:40:07.123Z",
                )
                .unwrap(),
            ),
            ManifestRecordOutcome::Recorded
        );
        first.release();
        let canonical = canonical_path(&project);
        let original = std::fs::read(&canonical).unwrap();
        let attributes = OpenedRegularFile::open_existing(&canonical, false)
            .unwrap()
            .attributes;
        windows_set_attributes(&canonical, attributes | FILE_ATTRIBUTE_READONLY);

        let mut guard = ProjectSeedManifestGuard::acquire(&project).unwrap();
        let outcome = guard.publication_permit().record_file(
            &activation,
            context_row(
                "Context.coordinator.md",
                "context:coordinator",
                "2026-07-16T19:42:00.000Z",
            )
            .unwrap(),
        );
        assert_eq!(
            outcome,
            ManifestRecordOutcome::PublishedUnrecorded(ManifestDegradedReason::ReadOnlyCanonical)
        );
        guard.release();
        assert_eq!(std::fs::read(&canonical).unwrap(), original);
        assert!(exact_temp_paths(&project).is_empty());
        windows_set_attributes(&canonical, attributes & !FILE_ATTRIBUTE_READONLY);
    }

    #[cfg(windows)]
    #[test]
    fn injected_replace_errors_follow_1175_1176_and_1177_recovery_policy() {
        use windows_sys::Win32::Foundation::{
            ERROR_UNABLE_TO_MOVE_REPLACEMENT, ERROR_UNABLE_TO_MOVE_REPLACEMENT_2,
            ERROR_UNABLE_TO_REMOVE_REPLACED,
        };

        for raw_error in [
            ERROR_UNABLE_TO_REMOVE_REPLACED,
            ERROR_UNABLE_TO_MOVE_REPLACEMENT,
            ERROR_UNABLE_TO_MOVE_REPLACEMENT_2,
        ] {
            let (_temp, project) = setup_project();
            let activation = ManifestActivationToken::for_test();
            let mut initial = ProjectSeedManifestGuard::acquire(&project).unwrap();
            assert_eq!(
                initial.publication_permit().record_file(
                    &activation,
                    context_row(
                        "Context.AgentsCommander.md",
                        "context:agentscommander",
                        "2026-07-16T19:40:07.123Z",
                    )
                    .unwrap(),
                ),
                ManifestRecordOutcome::Recorded
            );
            initial.release();
            let canonical = canonical_path(&project);
            let original = std::fs::read(&canonical).unwrap();
            let renamed_old = project.join(".ac").join("old-destination-recovery");
            let hooks = TestFilesystemHooks {
                windows_namespace_call: Some(Arc::new({
                    let renamed_old = renamed_old.clone();
                    move |operation, _temp, canonical| {
                        assert_eq!(operation, WindowsNamespaceOperation::ReplaceFile);
                        if raw_error == ERROR_UNABLE_TO_MOVE_REPLACEMENT {
                            std::fs::remove_file(canonical).unwrap();
                        } else if raw_error == ERROR_UNABLE_TO_MOVE_REPLACEMENT_2 {
                            std::fs::rename(canonical, &renamed_old).unwrap();
                        }
                        Err(raw_error)
                    }
                })),
                ..TestFilesystemHooks::default()
            };
            let mut guard = ProjectSeedManifestGuard::acquire_with_hooks(
                &project,
                Duration::from_secs(1),
                hooks,
            )
            .unwrap();
            let outcome = guard.publication_permit().record_file(
                &activation,
                context_row(
                    "Context.coordinator.md",
                    "context:coordinator",
                    "2026-07-16T19:42:00.000Z",
                )
                .unwrap(),
            );
            guard.release();

            if raw_error == ERROR_UNABLE_TO_REMOVE_REPLACED {
                assert_eq!(
                    outcome,
                    ManifestRecordOutcome::PublishedUnrecorded(
                        ManifestDegradedReason::PersistenceFailure
                    )
                );
                assert_eq!(std::fs::read(&canonical).unwrap(), original);
                assert!(exact_temp_paths(&project).is_empty());
            } else {
                assert_eq!(
                    outcome,
                    ManifestRecordOutcome::PublishedUnrecorded(
                        ManifestDegradedReason::RecoveryRequired
                    )
                );
                let candidates = exact_temp_paths(&project);
                assert_eq!(candidates.len(), 1);
                if raw_error == ERROR_UNABLE_TO_MOVE_REPLACEMENT {
                    assert!(!canonical.exists());
                    std::fs::write(
                        &canonical,
                        serialize_state(&ManifestState::default()).unwrap(),
                    )
                    .unwrap();
                    let next = ProjectSeedManifestGuard::acquire(&project).unwrap();
                    next.release();
                    assert!(candidates[0].is_file());
                } else {
                    assert!(!canonical.exists());
                    assert_eq!(std::fs::read(&renamed_old).unwrap(), original);
                }
            }
        }
    }

    #[cfg(windows)]
    #[test]
    fn injected_replace_error_matrix_retains_raw_code_and_never_retries() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use windows_sys::Win32::Foundation::{
            ERROR_ACCESS_DENIED, ERROR_SHARING_VIOLATION, ERROR_UNABLE_TO_MOVE_REPLACEMENT,
            ERROR_UNABLE_TO_MOVE_REPLACEMENT_2, ERROR_UNABLE_TO_REMOVE_REPLACED,
        };

        #[derive(Clone, Copy, Debug)]
        enum Mutation {
            StableNames,
            RemoveCanonical,
            RenameCanonical,
        }

        #[derive(Clone, Copy, Debug)]
        enum ExpectedError {
            StableFailure,
            RecoveryRequired,
            Partial(WindowsReplacePartial),
        }

        #[derive(Clone, Copy, Debug)]
        struct Case {
            label: &'static str,
            raw_error: u32,
            mutation: Mutation,
            expected: ExpectedError,
            canonical_present: bool,
            temp_present: bool,
            renamed_old_present: bool,
        }

        let cases = [
            Case {
                label: "1175 documented stable names",
                raw_error: ERROR_UNABLE_TO_REMOVE_REPLACED,
                mutation: Mutation::StableNames,
                expected: ExpectedError::StableFailure,
                canonical_present: true,
                temp_present: false,
                renamed_old_present: false,
            },
            Case {
                label: "1175 contradictory missing canonical",
                raw_error: ERROR_UNABLE_TO_REMOVE_REPLACED,
                mutation: Mutation::RemoveCanonical,
                expected: ExpectedError::RecoveryRequired,
                canonical_present: false,
                temp_present: true,
                renamed_old_present: false,
            },
            Case {
                label: "1176 documented removed canonical",
                raw_error: ERROR_UNABLE_TO_MOVE_REPLACEMENT,
                mutation: Mutation::RemoveCanonical,
                expected: ExpectedError::Partial(
                    WindowsReplacePartial::CanonicalRemovedReplacementAtTemp,
                ),
                canonical_present: false,
                temp_present: true,
                renamed_old_present: false,
            },
            Case {
                label: "1176 contradictory stable names",
                raw_error: ERROR_UNABLE_TO_MOVE_REPLACEMENT,
                mutation: Mutation::StableNames,
                expected: ExpectedError::Partial(
                    WindowsReplacePartial::CanonicalRemovedReplacementAtTemp,
                ),
                canonical_present: true,
                temp_present: true,
                renamed_old_present: false,
            },
            Case {
                label: "1177 documented renamed old destination",
                raw_error: ERROR_UNABLE_TO_MOVE_REPLACEMENT_2,
                mutation: Mutation::RenameCanonical,
                expected: ExpectedError::Partial(
                    WindowsReplacePartial::ReplacementEnrichedOldDestinationRenamed,
                ),
                canonical_present: false,
                temp_present: true,
                renamed_old_present: true,
            },
            Case {
                label: "1177 contradictory stable names",
                raw_error: ERROR_UNABLE_TO_MOVE_REPLACEMENT_2,
                mutation: Mutation::StableNames,
                expected: ExpectedError::Partial(
                    WindowsReplacePartial::ReplacementEnrichedOldDestinationRenamed,
                ),
                canonical_present: true,
                temp_present: true,
                renamed_old_present: false,
            },
            Case {
                label: "other access denied stable names",
                raw_error: ERROR_ACCESS_DENIED,
                mutation: Mutation::StableNames,
                expected: ExpectedError::StableFailure,
                canonical_present: true,
                temp_present: false,
                renamed_old_present: false,
            },
            Case {
                label: "other sharing error contradictory state",
                raw_error: ERROR_SHARING_VIOLATION,
                mutation: Mutation::RemoveCanonical,
                expected: ExpectedError::RecoveryRequired,
                canonical_present: false,
                temp_present: true,
                renamed_old_present: false,
            },
        ];

        for case in cases {
            let (_temp, project) = setup_project();
            let activation = ManifestActivationToken::for_test();
            let mut initial = ProjectSeedManifestGuard::acquire(&project).unwrap();
            assert_eq!(
                initial.publication_permit().record_file(
                    &activation,
                    context_row(
                        "Context.AgentsCommander.md",
                        "context:agentscommander",
                        "2026-07-16T19:40:07.123Z",
                    )
                    .unwrap(),
                ),
                ManifestRecordOutcome::Recorded,
                "{}",
                case.label
            );
            initial.release();

            let canonical = canonical_path(&project);
            let original = std::fs::read(&canonical).unwrap();
            let renamed_old = project.join(".ac").join("renamed-old-destination");
            let call_count = Arc::new(AtomicUsize::new(0));
            let hooks = TestFilesystemHooks {
                windows_namespace_call: Some(Arc::new({
                    let call_count = Arc::clone(&call_count);
                    let renamed_old = renamed_old.clone();
                    move |operation, _temp, canonical| {
                        assert_eq!(operation, WindowsNamespaceOperation::ReplaceFile);
                        call_count.fetch_add(1, Ordering::SeqCst);
                        match case.mutation {
                            Mutation::StableNames => {}
                            Mutation::RemoveCanonical => {
                                std::fs::remove_file(canonical).unwrap();
                            }
                            Mutation::RenameCanonical => {
                                std::fs::rename(canonical, &renamed_old).unwrap();
                            }
                        }
                        Err(case.raw_error)
                    }
                })),
                ..TestFilesystemHooks::default()
            };
            let mut guard = ProjectSeedManifestGuard::acquire_with_hooks(
                &project,
                Duration::from_secs(1),
                hooks,
            )
            .unwrap();
            let mut desired = read_disk_state(&project);
            let _ = desired.upsert(
                context_row(
                    "Context.coordinator.md",
                    "context:coordinator",
                    "2026-07-16T19:42:00.000Z",
                )
                .unwrap(),
            );
            let error = guard
                .write_canonical(serialize_state(&desired).unwrap())
                .unwrap_err();
            assert_eq!(call_count.load(Ordering::SeqCst), 1, "{}", case.label);

            let observed_raw = match &error {
                SeedManifestError::WindowsNamespaceFailure {
                    raw_error,
                    canonical_state,
                    temp_state,
                    ..
                } => {
                    assert!(matches!(case.expected, ExpectedError::StableFailure));
                    assert!(matches!(
                        canonical_state.as_ref(),
                        WindowsPathState::Same {
                            bytes_match: true,
                            ..
                        }
                    ));
                    assert!(matches!(
                        temp_state.as_ref(),
                        WindowsPathState::Same {
                            bytes_match: true,
                            ..
                        }
                    ));
                    *raw_error
                }
                SeedManifestError::WindowsReplaceRecoveryRequired { raw_error, .. } => {
                    assert!(matches!(case.expected, ExpectedError::RecoveryRequired));
                    *raw_error
                }
                SeedManifestError::WindowsReplacePartial {
                    raw_error, partial, ..
                } => {
                    let ExpectedError::Partial(expected_partial) = case.expected else {
                        panic!("{} returned an unexpected partial error", case.label);
                    };
                    assert_eq!(*partial, expected_partial);
                    *raw_error
                }
                other => panic!("{} returned unexpected error {other:?}", case.label),
            };
            assert_eq!(observed_raw, case.raw_error, "{}", case.label);
            let display = format!("{error}");
            assert!(
                display.contains(&case.raw_error.to_string()),
                "{}",
                case.label
            );
            assert!(
                display.contains(&format!("0x{:08x}", case.raw_error)),
                "{}",
                case.label
            );
            guard.release();

            assert_eq!(canonical.exists(), case.canonical_present, "{}", case.label);
            if case.canonical_present {
                assert_eq!(
                    std::fs::read(&canonical).unwrap(),
                    original,
                    "{}",
                    case.label
                );
            }
            assert_eq!(
                exact_temp_paths(&project).len(),
                usize::from(case.temp_present),
                "{}",
                case.label
            );
            assert_eq!(
                renamed_old.exists(),
                case.renamed_old_present,
                "{}",
                case.label
            );
            if case.renamed_old_present {
                assert_eq!(
                    std::fs::read(&renamed_old).unwrap(),
                    original,
                    "{}",
                    case.label
                );
            }
        }
    }

    #[cfg(windows)]
    #[test]
    fn injected_move_failures_clean_only_the_exact_prepublication_state() {
        use windows_sys::Win32::Foundation::ERROR_ACCESS_DENIED;

        for move_before_error in [false, true] {
            let (_temp, project) = setup_project();
            let hooks = TestFilesystemHooks {
                windows_namespace_call: Some(Arc::new(move |operation, temp, canonical| {
                    assert_eq!(operation, WindowsNamespaceOperation::MoveFileEx);
                    if move_before_error {
                        std::fs::rename(temp, canonical).unwrap();
                    }
                    Err(ERROR_ACCESS_DENIED)
                })),
                ..TestFilesystemHooks::default()
            };
            let mut guard = ProjectSeedManifestGuard::acquire_with_hooks(
                &project,
                Duration::from_secs(1),
                hooks,
            )
            .unwrap();
            let outcome = guard.publication_permit().record_file(
                &ManifestActivationToken::for_test(),
                context_row(
                    "Context.AgentsCommander.md",
                    "context:agentscommander",
                    "2026-07-16T19:40:07.123Z",
                )
                .unwrap(),
            );
            guard.release();
            if move_before_error {
                assert_eq!(
                    outcome,
                    ManifestRecordOutcome::PublishedUnrecorded(
                        ManifestDegradedReason::RecoveryRequired
                    )
                );
                assert!(canonical_path(&project).is_file());
            } else {
                assert_eq!(
                    outcome,
                    ManifestRecordOutcome::PublishedUnrecorded(
                        ManifestDegradedReason::PersistenceFailure
                    )
                );
                assert!(!canonical_path(&project).exists());
                assert!(exact_temp_paths(&project).is_empty());
            }
        }
    }

    #[cfg(windows)]
    #[test]
    fn successful_call_with_substituted_final_identity_is_not_recorded_or_cleaned() {
        let (_temp, project) = setup_project();
        let foreign = b"foreign final object".to_vec();
        let hooks = TestFilesystemHooks {
            windows_namespace_call: Some(Arc::new({
                let foreign = foreign.clone();
                move |operation, temp, canonical| {
                    assert_eq!(operation, WindowsNamespaceOperation::MoveFileEx);
                    std::fs::rename(temp, canonical).unwrap();
                    std::fs::remove_file(canonical).unwrap();
                    std::fs::write(canonical, &foreign).unwrap();
                    Ok(())
                }
            })),
            ..TestFilesystemHooks::default()
        };
        let mut guard =
            ProjectSeedManifestGuard::acquire_with_hooks(&project, Duration::from_secs(1), hooks)
                .unwrap();
        let outcome = guard.publication_permit().record_file(
            &ManifestActivationToken::for_test(),
            context_row(
                "Context.AgentsCommander.md",
                "context:agentscommander",
                "2026-07-16T19:40:07.123Z",
            )
            .unwrap(),
        );
        guard.release();
        assert_eq!(
            outcome,
            ManifestRecordOutcome::PublishedUnrecorded(ManifestDegradedReason::IdentityConflict)
        );
        assert_eq!(std::fs::read(canonical_path(&project)).unwrap(), foreign);
    }

    #[cfg(windows)]
    #[test]
    fn handle_bound_cleanup_never_deletes_a_substituted_path() {
        let (_temp, project) = setup_project();
        let path = project
            .join(".ac")
            .join(".seed-manifest.00000000-0000-0000-0000-000000000001.tmp");
        let mut opened = OpenedRegularFile::create_new(&path).unwrap();
        opened.file.write_all(b"owned").unwrap();
        opened.file.sync_all().unwrap();
        opened.refresh_facts().unwrap();
        std::fs::remove_file(&path).unwrap();
        std::fs::write(&path, b"substitution").unwrap();
        assert!(matches!(
            cleanup_current_owned_temp(&opened, Some(b"owned")),
            Err(SeedManifestError::UnsafePath { .. })
        ));
        drop(opened);
        assert_eq!(std::fs::read(path).unwrap(), b"substitution");
    }

    #[cfg(windows)]
    #[test]
    fn replace_preserves_certified_ntfs_metadata_surface_and_both_compression_directions() {
        use windows_sys::Win32::Storage::FileSystem::{
            COMPRESSION_FORMAT_DEFAULT, COMPRESSION_FORMAT_NONE, FILE_ATTRIBUTE_COMPRESSED,
        };

        let (_temp, project) = setup_project();
        let activation = ManifestActivationToken::for_test();
        let mut initial = ProjectSeedManifestGuard::acquire(&project).unwrap();
        assert_eq!(
            initial.publication_permit().record_file(
                &activation,
                context_row(
                    "Context.AgentsCommander.md",
                    "context:agentscommander",
                    "2026-07-16T19:40:07.123Z",
                )
                .unwrap(),
            ),
            ManifestRecordOutcome::Recorded
        );
        initial.release();

        let canonical = canonical_path(&project);
        let protected_dacl = windows_protect_existing_dacl(&canonical);
        let ads = windows_ads_path(&canonical, "agentscommander-stage-a");
        std::fs::write(&ads, b"destination stream").unwrap();
        windows_set_compression(&canonical, COMPRESSION_FORMAT_DEFAULT);
        let before = OpenedRegularFile::open_existing(&canonical, false).unwrap();
        assert_ne!(before.attributes & FILE_ATTRIBUTE_COMPRESSED, 0);
        let creation_time = before.creation_time;
        drop(before);

        let mut compressed = ProjectSeedManifestGuard::acquire(&project).unwrap();
        assert_eq!(
            compressed.publication_permit().record_file(
                &activation,
                context_row(
                    "Context.coordinator.md",
                    "context:coordinator",
                    "2026-07-16T19:42:00.000Z",
                )
                .unwrap(),
            ),
            ManifestRecordOutcome::Recorded
        );
        compressed.release();
        let after_compressed = OpenedRegularFile::open_existing(&canonical, false).unwrap();
        assert_eq!(after_compressed.creation_time, creation_time);
        assert_ne!(after_compressed.attributes & FILE_ATTRIBUTE_COMPRESSED, 0);
        assert_eq!(windows_dacl_bytes(&canonical), protected_dacl);
        assert_eq!(std::fs::read(&ads).unwrap(), b"destination stream");
        drop(after_compressed);

        windows_set_compression(&canonical, COMPRESSION_FORMAT_NONE);
        assert_eq!(
            OpenedRegularFile::open_existing(&canonical, false)
                .unwrap()
                .attributes
                & FILE_ATTRIBUTE_COMPRESSED,
            0
        );
        let mut uncompressed = ProjectSeedManifestGuard::acquire(&project).unwrap();
        assert_eq!(
            uncompressed.publication_permit().record_file(
                &activation,
                context_row(
                    "Context.AgentsCommander.md",
                    "context:agentscommander",
                    "2026-07-16T19:43:00.000Z",
                )
                .unwrap(),
            ),
            ManifestRecordOutcome::Recorded
        );
        uncompressed.release();
        assert_eq!(
            OpenedRegularFile::open_existing(&canonical, false)
                .unwrap()
                .attributes
                & FILE_ATTRIBUTE_COMPRESSED,
            0
        );
        assert_eq!(std::fs::read(&ads).unwrap(), b"destination stream");
    }

    #[cfg(windows)]
    #[test]
    fn ordinary_attribute_inputs_are_observed_without_a_preservation_assertion() {
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_ATTRIBUTE_ARCHIVE, FILE_ATTRIBUTE_HIDDEN, FILE_ATTRIBUTE_NORMAL,
            FILE_ATTRIBUTE_NOT_CONTENT_INDEXED, FILE_ATTRIBUTE_SYSTEM,
        };

        let (_temp, project) = setup_project();
        let activation = ManifestActivationToken::for_test();
        let mut initial = ProjectSeedManifestGuard::acquire(&project).unwrap();
        assert_eq!(
            initial.publication_permit().record_file(
                &activation,
                context_row(
                    "Context.AgentsCommander.md",
                    "context:agentscommander",
                    "2026-07-16T19:40:07.123Z",
                )
                .unwrap(),
            ),
            ManifestRecordOutcome::Recorded
        );
        initial.release();
        let canonical = canonical_path(&project);
        let selected = FILE_ATTRIBUTE_ARCHIVE
            | FILE_ATTRIBUTE_HIDDEN
            | FILE_ATTRIBUTE_SYSTEM
            | FILE_ATTRIBUTE_NOT_CONTENT_INDEXED;
        windows_set_attributes(&canonical, selected);
        let mut set_case = ProjectSeedManifestGuard::acquire(&project).unwrap();
        assert_eq!(
            set_case.publication_permit().record_file(
                &activation,
                context_row(
                    "Context.coordinator.md",
                    "context:coordinator",
                    "2026-07-16T19:42:00.000Z",
                )
                .unwrap(),
            ),
            ManifestRecordOutcome::Recorded
        );
        set_case.release();
        let final_set = OpenedRegularFile::open_existing(&canonical, false)
            .unwrap()
            .attributes
            & selected;

        windows_set_attributes(&canonical, FILE_ATTRIBUTE_NORMAL);
        let mut clear_case = ProjectSeedManifestGuard::acquire(&project).unwrap();
        assert_eq!(
            clear_case.publication_permit().record_file(
                &activation,
                context_row(
                    "Context.AgentsCommander.md",
                    "context:agentscommander",
                    "2026-07-16T19:43:00.000Z",
                )
                .unwrap(),
            ),
            ManifestRecordOutcome::Recorded
        );
        clear_case.release();
        let final_clear = OpenedRegularFile::open_existing(&canonical, false)
            .unwrap()
            .attributes
            & selected;
        eprintln!(
            "ordinary attribute non-guarantee observation set=0x{final_set:08x} clear=0x{final_clear:08x}"
        );
    }
}

#[cfg(windows)]
impl WindowsPathState {
    fn is_exact_same(&self, identity: FileIdentity, require_one_link: bool) -> bool {
        matches!(
            self,
            Self::Same {
                identity: observed,
                links,
                bytes_match: true,
                ..
            } if *observed == identity && (!require_one_link || *links == 1)
        )
    }
}

#[cfg(windows)]
impl ProjectSeedManifestGuard {
    fn publish_windows_temp(
        &self,
        temp: OpenedRegularFile,
        canonical_path: &Path,
        bytes: &[u8],
    ) -> Result<OpenedRegularFile, SeedManifestError> {
        use windows_sys::Win32::Foundation::{
            ERROR_UNABLE_TO_MOVE_REPLACEMENT, ERROR_UNABLE_TO_MOVE_REPLACEMENT_2,
            ERROR_UNABLE_TO_REMOVE_REPLACED,
        };

        if temp.identity.volume != self.ac_root.identity.volume
            || temp.path.parent() != Some(self.ac_root.path.as_path())
            || canonical_path.parent() != Some(self.ac_root.path.as_path())
        {
            self.cleanup_owned_temp(&temp, Some(bytes));
            return Err(SeedManifestError::UnsafePath {
                path: temp.path,
                reason: "manifest temp and canonical must share the pinned .ac parent and volume"
                    .to_string(),
            });
        }
        verify_windows_temp_has_only_unnamed_stream(&temp.path)?;

        let source_identity = temp.identity;
        let source_length = temp.length;
        let temp_path = temp.path.clone();
        let source_witness = match reopen_windows_zero_access(&temp.file, &temp.path) {
            Ok(witness) => witness,
            Err(error) => {
                self.cleanup_owned_temp(&temp, Some(bytes));
                return Err(error);
            }
        };
        if source_witness.identity != source_identity
            || source_witness.length != source_length
            || source_witness.links != 1
        {
            self.cleanup_owned_temp(&temp, Some(bytes));
            return Err(SeedManifestError::UnsafePath {
                path: temp.path,
                reason: "zero-access ReOpenFile witness does not match the flushed temp"
                    .to_string(),
            });
        }

        drop(temp);

        let final_temp_state = inspect_windows_path(&temp_path, source_identity, bytes);
        if !final_temp_state.is_exact_same(source_identity, true) {
            return Err(SeedManifestError::UnsafePath {
                path: temp_path,
                reason: format!(
                    "temp path failed the final writer-closed identity probe: {final_temp_state:?}"
                ),
            });
        }
        self.revalidate_owner()?;
        self.verify_canonical_unchanged(canonical_path)?;
        let temp_wide = absolute_verbatim_utf16(&temp_path)?;
        let canonical_wide = absolute_verbatim_utf16(canonical_path)?;

        let (operation, destination_identity, destination_raw) = match &self.snapshot {
            CanonicalSnapshot::Writable {
                raw,
                canonical: Some(canonical),
                ..
            } => (
                WindowsNamespaceOperation::ReplaceFile,
                Some(canonical.identity),
                Some(raw.as_slice()),
            ),
            CanonicalSnapshot::Writable {
                canonical: None, ..
            } => (WindowsNamespaceOperation::MoveFileEx, None, None),
            CanonicalSnapshot::ReadOnly { .. } => {
                return Err(SeedManifestError::Validation(
                    "read-only snapshot reached the Windows namespace writer".to_string(),
                ));
            }
        };

        let namespace_result = self.call_windows_namespace(
            operation,
            &temp_path,
            canonical_path,
            &temp_wide,
            &canonical_wide,
        );
        match namespace_result {
            Ok(()) => {
                let final_file =
                    match open_validated_windows_final(canonical_path, source_identity, bytes) {
                        Ok(file) => file,
                        Err(final_state) => {
                            return Err(SeedManifestError::WindowsPostPublishIdentityConflict {
                                canonical: canonical_path.to_path_buf(),
                                temp: temp_path,
                                expected_source: source_identity,
                                final_state: Box::new(final_state),
                                old_destination_identity: Box::new(destination_identity),
                            });
                        }
                    };

                if let Some(expected_destination) = destination_identity {
                    let destination_facts_result = match &self.snapshot {
                        CanonicalSnapshot::Writable {
                            canonical: Some(canonical),
                            ..
                        } => handle_facts(&canonical.file),
                        _ => {
                            return Err(SeedManifestError::WindowsPostPublishIdentityConflict {
                                canonical: canonical_path.to_path_buf(),
                                temp: temp_path,
                                expected_source: source_identity,
                                final_state: Box::new(WindowsPathState::Same {
                                    identity: final_file.identity,
                                    links: final_file.links,
                                    length: final_file.length,
                                    bytes_match: true,
                                }),
                                old_destination_identity: Box::new(Some(expected_destination)),
                            })
                        }
                    };
                    let destination_facts = destination_facts_result.map_err(|_| {
                        SeedManifestError::WindowsPostPublishIdentityConflict {
                            canonical: canonical_path.to_path_buf(),
                            temp: temp_path.clone(),
                            expected_source: source_identity,
                            final_state: Box::new(WindowsPathState::Same {
                                identity: final_file.identity,
                                links: final_file.links,
                                length: final_file.length,
                                bytes_match: true,
                            }),
                            old_destination_identity: Box::new(Some(expected_destination)),
                        }
                    })?;
                    if destination_facts.identity != expected_destination
                        || !destination_facts.is_regular_file
                        || destination_facts.is_reparse
                    {
                        return Err(SeedManifestError::WindowsPostPublishIdentityConflict {
                            canonical: canonical_path.to_path_buf(),
                            temp: temp_path,
                            expected_source: source_identity,
                            final_state: Box::new(WindowsPathState::Same {
                                identity: final_file.identity,
                                links: final_file.links,
                                length: final_file.length,
                                bytes_match: true,
                            }),
                            old_destination_identity: Box::new(Some(expected_destination)),
                        });
                    }
                }
                Ok(final_file)
            }
            Err(raw_error) => {
                let canonical_state = match (destination_identity, destination_raw) {
                    (Some(identity), Some(raw)) => {
                        inspect_windows_path(canonical_path, identity, raw)
                    }
                    _ => inspect_windows_path(canonical_path, source_identity, bytes),
                };
                let temp_state = inspect_windows_path(&temp_path, source_identity, bytes);
                log::warn!(
                    "[seed_manifest] Windows namespace failure operation={:?} raw_error={} raw_error_hex=0x{:08x} canonical={} temp={} backup=NULL ac_identity={:?} lock_identity={:?} source_identity={:?} destination_identity={:?} destination_handle_live={} canonical_state={:?} temp_state={:?}",
                    operation,
                    raw_error,
                    raw_error,
                    canonical_path.display(),
                    temp_path.display(),
                    self.ac_root.identity,
                    self.lock_identity,
                    source_identity,
                    destination_identity,
                    destination_identity.is_some(),
                    canonical_state,
                    temp_state
                );

                match operation {
                    WindowsNamespaceOperation::MoveFileEx => {
                        if matches!(canonical_state, WindowsPathState::NotFound)
                            && temp_state.is_exact_same(source_identity, true)
                        {
                            if let Err(cleanup_error) = cleanup_windows_temp_with_witness(
                                &source_witness,
                                &temp_path,
                                source_identity,
                                Some(bytes),
                            ) {
                                log::warn!(
                                    "[seed_manifest] Windows current-owned temp cleanup failed after MoveFileExW raw_error={} path={} cleanup_error={}",
                                    raw_error,
                                    temp_path.display(),
                                    cleanup_error
                                );
                            }
                            Err(SeedManifestError::WindowsNamespaceFailure {
                                operation,
                                raw_error,
                                canonical: canonical_path.to_path_buf(),
                                temp: temp_path,
                                canonical_state: Box::new(canonical_state),
                                temp_state: Box::new(temp_state),
                            })
                        } else {
                            Err(SeedManifestError::WindowsMoveRecoveryRequired {
                                raw_error,
                                canonical: canonical_path.to_path_buf(),
                                temp: temp_path,
                                canonical_state: Box::new(canonical_state),
                                temp_state: Box::new(temp_state),
                            })
                        }
                    }
                    WindowsNamespaceOperation::ReplaceFile => {
                        if raw_error == ERROR_UNABLE_TO_MOVE_REPLACEMENT {
                            return Err(SeedManifestError::WindowsReplacePartial {
                                raw_error,
                                partial: WindowsReplacePartial::CanonicalRemovedReplacementAtTemp,
                                canonical: canonical_path.to_path_buf(),
                                temp: temp_path,
                                canonical_state: Box::new(canonical_state),
                                temp_state: Box::new(temp_state),
                            });
                        }
                        if raw_error == ERROR_UNABLE_TO_MOVE_REPLACEMENT_2 {
                            return Err(SeedManifestError::WindowsReplacePartial {
                                raw_error,
                                partial:
                                    WindowsReplacePartial::ReplacementEnrichedOldDestinationRenamed,
                                canonical: canonical_path.to_path_buf(),
                                temp: temp_path,
                                canonical_state: Box::new(canonical_state),
                                temp_state: Box::new(temp_state),
                            });
                        }

                        let stable_destination = destination_identity
                            .is_some_and(|identity| canonical_state.is_exact_same(identity, false));
                        let stable_source = temp_state.is_exact_same(source_identity, true);
                        if stable_destination && stable_source {
                            if let Err(cleanup_error) = cleanup_windows_temp_with_witness(
                                &source_witness,
                                &temp_path,
                                source_identity,
                                Some(bytes),
                            ) {
                                log::warn!(
                                    "[seed_manifest] Windows current-owned temp cleanup failed after ReplaceFileW raw_error={} path={} cleanup_error={}",
                                    raw_error,
                                    temp_path.display(),
                                    cleanup_error
                                );
                            }
                            Err(SeedManifestError::WindowsNamespaceFailure {
                                operation,
                                raw_error,
                                canonical: canonical_path.to_path_buf(),
                                temp: temp_path,
                                canonical_state: Box::new(canonical_state),
                                temp_state: Box::new(temp_state),
                            })
                        } else {
                            if raw_error == ERROR_UNABLE_TO_REMOVE_REPLACED {
                                log::warn!(
                                    "[seed_manifest] ReplaceFileW error 1175 contradicted the documented stable-name state; preserving all observed names"
                                );
                            }
                            Err(SeedManifestError::WindowsReplaceRecoveryRequired {
                                raw_error,
                                canonical: canonical_path.to_path_buf(),
                                temp: temp_path,
                                canonical_state: Box::new(canonical_state),
                                temp_state: Box::new(temp_state),
                            })
                        }
                    }
                }
            }
        }
    }

    fn call_windows_namespace(
        &self,
        operation: WindowsNamespaceOperation,
        _temp: &Path,
        _canonical: &Path,
        temp_wide: &[u16],
        canonical_wide: &[u16],
    ) -> Result<(), u32> {
        #[cfg(test)]
        if let Some(hook) = &self.hooks.windows_namespace_call {
            return hook(operation, _temp, _canonical);
        }

        use windows_sys::Win32::Foundation::GetLastError;
        use windows_sys::Win32::Storage::FileSystem::{
            MoveFileExW, ReplaceFileW, MOVEFILE_WRITE_THROUGH,
        };

        let success = match operation {
            WindowsNamespaceOperation::MoveFileEx => unsafe {
                MoveFileExW(
                    temp_wide.as_ptr(),
                    canonical_wide.as_ptr(),
                    MOVEFILE_WRITE_THROUGH,
                )
            },
            WindowsNamespaceOperation::ReplaceFile => unsafe {
                ReplaceFileW(
                    canonical_wide.as_ptr(),
                    temp_wide.as_ptr(),
                    std::ptr::null(),
                    0,
                    std::ptr::null(),
                    std::ptr::null(),
                )
            },
        };
        if success == 0 {
            let raw_error = unsafe { GetLastError() };
            Err(raw_error)
        } else {
            Ok(())
        }
    }
}

#[cfg(windows)]
fn absolute_verbatim_utf16(path: &Path) -> Result<Vec<u16>, SeedManifestError> {
    use std::os::windows::ffi::OsStrExt;

    if !path.is_absolute() {
        return Err(SeedManifestError::Validation(format!(
            "Windows namespace path must be absolute: {}",
            path.display()
        )));
    }
    let units = path.as_os_str().encode_wide().collect::<Vec<_>>();
    if units.contains(&0) {
        return Err(SeedManifestError::Validation(
            "Windows namespace path contains an interior NUL".to_string(),
        ));
    }
    let slash = u16::from(b'\\');
    let question = u16::from(b'?');
    let mut verbatim = if units.starts_with(&[slash, slash, question, slash]) {
        units
    } else if units.starts_with(&[slash, slash]) {
        let mut prefixed = "\\\\?\\UNC\\".encode_utf16().collect::<Vec<_>>();
        prefixed.extend_from_slice(&units[2..]);
        prefixed
    } else {
        let mut prefixed = "\\\\?\\".encode_utf16().collect::<Vec<_>>();
        prefixed.extend_from_slice(&units);
        prefixed
    };
    verbatim.push(0);
    Ok(verbatim)
}

#[cfg(windows)]
fn reopen_windows_zero_access(
    writer: &File,
    path: &Path,
) -> Result<OpenedRegularFile, SeedManifestError> {
    use std::os::windows::io::{AsRawHandle, FromRawHandle};
    use windows_sys::Win32::Foundation::{HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{
        ReOpenFile, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE, FILE_SHARE_READ,
        FILE_SHARE_WRITE,
    };

    let handle = unsafe {
        ReOpenFile(
            writer.as_raw_handle() as HANDLE,
            0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            FILE_FLAG_OPEN_REPARSE_POINT,
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(SeedManifestError::TempFile {
            operation: "ReOpenFile zero-access witness",
            path: path.to_path_buf(),
            source: io::Error::last_os_error(),
        });
    }
    let file = unsafe { File::from_raw_handle(handle as _) };
    OpenedRegularFile::from_file(path, file)
}

#[cfg(windows)]
fn inspect_windows_path(
    path: &Path,
    expected_identity: FileIdentity,
    expected_bytes: &[u8],
) -> WindowsPathState {
    let file = match open_regular_no_follow(path, false, false) {
        Ok(file) => file,
        Err(error) if is_exact_not_found(&error) => return WindowsPathState::NotFound,
        Err(error) => {
            return WindowsPathState::InspectionError {
                raw_error: error.raw_os_error().and_then(|raw| u32::try_from(raw).ok()),
            }
        }
    };
    let mut opened = match OpenedRegularFile::from_file(path, file) {
        Ok(opened) => opened,
        Err(SeedManifestError::UnsafePath { reason, .. }) => {
            return WindowsPathState::Unsafe { reason }
        }
        Err(error) => {
            return WindowsPathState::InspectionError {
                raw_error: match error {
                    SeedManifestError::Io { source, .. }
                    | SeedManifestError::TempFile { source, .. } => source
                        .raw_os_error()
                        .and_then(|raw| u32::try_from(raw).ok()),
                    _ => None,
                },
            }
        }
    };
    let bytes_match = match stream_matches(&mut opened.file, expected_bytes) {
        Ok(matches) => matches,
        Err(error) => {
            return WindowsPathState::InspectionError {
                raw_error: error.raw_os_error().and_then(|raw| u32::try_from(raw).ok()),
            }
        }
    };
    if opened.identity == expected_identity {
        WindowsPathState::Same {
            identity: opened.identity,
            links: opened.links,
            length: opened.length,
            bytes_match,
        }
    } else {
        WindowsPathState::Different {
            identity: opened.identity,
            links: opened.links,
            length: opened.length,
            bytes_match,
        }
    }
}

#[cfg(windows)]
fn open_validated_windows_final(
    path: &Path,
    expected_identity: FileIdentity,
    expected_bytes: &[u8],
) -> Result<OpenedRegularFile, WindowsPathState> {
    let file = match open_regular_no_follow(path, false, false) {
        Ok(file) => file,
        Err(error) if is_exact_not_found(&error) => return Err(WindowsPathState::NotFound),
        Err(error) => {
            return Err(WindowsPathState::InspectionError {
                raw_error: error.raw_os_error().and_then(|raw| u32::try_from(raw).ok()),
            })
        }
    };
    let mut opened = match OpenedRegularFile::from_file(path, file) {
        Ok(opened) => opened,
        Err(SeedManifestError::UnsafePath { reason, .. }) => {
            return Err(WindowsPathState::Unsafe { reason })
        }
        Err(_) => return Err(WindowsPathState::InspectionError { raw_error: None }),
    };
    let bytes_match = match stream_matches(&mut opened.file, expected_bytes) {
        Ok(matches) => matches,
        Err(error) => {
            return Err(WindowsPathState::InspectionError {
                raw_error: error.raw_os_error().and_then(|raw| u32::try_from(raw).ok()),
            })
        }
    };
    if opened.identity != expected_identity || !bytes_match {
        return Err(WindowsPathState::Different {
            identity: opened.identity,
            links: opened.links,
            length: opened.length,
            bytes_match,
        });
    }
    Ok(opened)
}

#[cfg(windows)]
fn open_windows_delete_handle(path: &Path) -> io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Foundation::GENERIC_READ;
    use windows_sys::Win32::Storage::FileSystem::{
        DELETE, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    };

    OpenOptions::new()
        .access_mode(GENERIC_READ | DELETE)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
}

#[cfg(windows)]
fn cleanup_current_owned_temp(
    temp: &OpenedRegularFile,
    expected_bytes: Option<&[u8]>,
) -> Result<(), SeedManifestError> {
    cleanup_windows_temp_with_witness(temp, &temp.path, temp.identity, expected_bytes)
}

#[cfg(windows)]
fn cleanup_windows_temp_with_witness(
    witness: &OpenedRegularFile,
    path: &Path,
    expected_identity: FileIdentity,
    expected_bytes: Option<&[u8]>,
) -> Result<(), SeedManifestError> {
    use std::mem::size_of;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::{GetLastError, HANDLE};
    use windows_sys::Win32::Storage::FileSystem::{
        FileDispositionInfo, SetFileInformationByHandle, FILE_DISPOSITION_INFO,
    };

    let witness_facts =
        handle_facts(&witness.file).map_err(|source| SeedManifestError::TempFile {
            operation: "inspect current-owned temp witness",
            path: path.to_path_buf(),
            source,
        })?;
    if witness_facts.identity != expected_identity
        || !witness_facts.is_regular_file
        || witness_facts.is_reparse
    {
        return Err(SeedManifestError::UnsafePath {
            path: path.to_path_buf(),
            reason: "current-owned temp witness changed identity or type before cleanup"
                .to_string(),
        });
    }

    let delete_file =
        open_windows_delete_handle(path).map_err(|source| SeedManifestError::TempFile {
            operation: "open no-follow GENERIC_READ|DELETE cleanup handle",
            path: path.to_path_buf(),
            source,
        })?;
    let mut delete_opened = OpenedRegularFile::from_file(path, delete_file)?;
    if delete_opened.identity != expected_identity || delete_opened.links != 1 {
        return Err(SeedManifestError::UnsafePath {
            path: path.to_path_buf(),
            reason: "current-owned temp cleanup handle did not match the one-link witness"
                .to_string(),
        });
    }
    if let Some(bytes) = expected_bytes {
        if delete_opened.length != u64::try_from(bytes.len()).unwrap_or(u64::MAX)
            || !stream_matches(&mut delete_opened.file, bytes).map_err(|source| {
                SeedManifestError::TempFile {
                    operation: "verify current-owned temp bytes before handle deletion",
                    path: path.to_path_buf(),
                    source,
                }
            })?
        {
            return Err(SeedManifestError::UnsafePath {
                path: path.to_path_buf(),
                reason: "current-owned temp bytes changed before handle deletion".to_string(),
            });
        }
        let immediate = inspect_windows_path(path, expected_identity, bytes);
        if !immediate.is_exact_same(expected_identity, true) {
            return Err(SeedManifestError::UnsafePath {
                path: path.to_path_buf(),
                reason: format!(
                    "current-owned temp path changed before FileDispositionInfo: {immediate:?}"
                ),
            });
        }
    }

    let disposition = FILE_DISPOSITION_INFO { DeleteFile: 1 };
    let success = unsafe {
        SetFileInformationByHandle(
            delete_opened.file.as_raw_handle() as HANDLE,
            FileDispositionInfo,
            (&disposition as *const FILE_DISPOSITION_INFO).cast(),
            u32::try_from(size_of::<FILE_DISPOSITION_INFO>()).unwrap_or(u32::MAX),
        )
    };
    if success == 0 {
        let raw_error = unsafe { GetLastError() };
        return Err(SeedManifestError::TempFile {
            operation: "SetFileInformationByHandle(FileDispositionInfo)",
            path: path.to_path_buf(),
            source: io::Error::from_raw_os_error(i32::try_from(raw_error).unwrap_or(i32::MAX)),
        });
    }
    drop(delete_opened);
    Ok(())
}

#[cfg(windows)]
fn verify_windows_temp_has_only_unnamed_stream(path: &Path) -> Result<(), SeedManifestError> {
    use std::mem::MaybeUninit;
    use windows_sys::Win32::Foundation::{GetLastError, ERROR_HANDLE_EOF, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{
        FindClose, FindFirstStreamW, FindNextStreamW, FindStreamInfoStandard,
        WIN32_FIND_STREAM_DATA,
    };

    struct FindStreamHandle(windows_sys::Win32::Foundation::HANDLE);
    impl Drop for FindStreamHandle {
        fn drop(&mut self) {
            if unsafe { FindClose(self.0) } == 0 {
                log::warn!(
                    "[seed_manifest] FindClose failed after bounded temp stream inventory: {}",
                    io::Error::last_os_error()
                );
            }
        }
    }

    let wide = absolute_verbatim_utf16(path)?;
    let mut data = MaybeUninit::<WIN32_FIND_STREAM_DATA>::zeroed();
    let raw_handle = unsafe {
        FindFirstStreamW(
            wide.as_ptr(),
            FindStreamInfoStandard,
            data.as_mut_ptr().cast(),
            0,
        )
    };
    if raw_handle == INVALID_HANDLE_VALUE {
        let raw_error = unsafe { GetLastError() };
        return Err(SeedManifestError::TempFile {
            operation: "FindFirstStreamW",
            path: path.to_path_buf(),
            source: io::Error::from_raw_os_error(i32::try_from(raw_error).unwrap_or(i32::MAX)),
        });
    }
    let handle = FindStreamHandle(raw_handle);
    let mut data = unsafe { data.assume_init() };
    loop {
        let end = data
            .cStreamName
            .iter()
            .position(|unit| *unit == 0)
            .unwrap_or(data.cStreamName.len());
        let expected = "::$DATA".encode_utf16().collect::<Vec<_>>();
        if data.cStreamName[..end] != expected {
            return Err(SeedManifestError::UnsafePath {
                path: path.to_path_buf(),
                reason: "manifest temp contains a named data stream".to_string(),
            });
        }

        let next =
            unsafe { FindNextStreamW(handle.0, (&mut data as *mut WIN32_FIND_STREAM_DATA).cast()) };
        if next != 0 {
            continue;
        }
        let raw_error = unsafe { GetLastError() };
        if raw_error == ERROR_HANDLE_EOF {
            break;
        }
        return Err(SeedManifestError::TempFile {
            operation: "FindNextStreamW",
            path: path.to_path_buf(),
            source: io::Error::from_raw_os_error(i32::try_from(raw_error).unwrap_or(i32::MAX)),
        });
    }
    drop(handle);
    Ok(())
}

#[cfg(not(any(unix, windows)))]
fn atomic_publish_manifest(
    temp: &Path,
    canonical: &Path,
    _had_canonical: bool,
) -> Result<(), SeedManifestError> {
    std::fs::rename(temp, canonical).map_err(|source| SeedManifestError::AtomicReplace {
        operation: "rename",
        temp: temp.to_path_buf(),
        canonical: canonical.to_path_buf(),
        source,
    })
}

// ---------------------------------------------------------------------------
// Stage E (#1064) conformance, scale, and adversarial-memory harness.
//
// These are dormant, test-only additions: they exercise the same DORMANT APIs
// used by the Stage A/C unit tests (`ManifestActivationToken::for_test`,
// `ProjectSeedManifestGuard`, `PublishedScopeBatch`) and never touch a
// production manifest. The scale benchmarks and the adversarial parser-memory
// case are `#[ignore]` (registered in `test-debt.allowlist.json`): they are run
// in an isolated release-mode child as Stage F acceptance evidence (plan
// sections 7.4, 10.1 item 4, 10.5 items 6-8). The hard 10 s / 512 MiB gates
// (plan section 7.4) are asserted only in a release build on the reference
// machine; a debug run records the measurements without gating.
#[cfg(test)]
mod stage_e_conformance {
    use super::*;
    use chrono::{DateTime, Utc};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    // Plan section 7.4 hard per-operation gates for the 100k cases.
    const HARD_ELAPSED_LIMIT: Duration = Duration::from_secs(10);
    const HARD_WORKING_SET_LIMIT: u64 = 512 * 1024 * 1024;
    const BENCH_SIZES: [usize; 3] = [1_000, 10_000, 100_000];

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

    fn config_file(agent: &str, dest: &str, suffix: &str) -> ManifestPathIdentity {
        ManifestPathIdentity::parse(
            ManifestPathEncoding::Utf8,
            format!(".ac/wg-1-dev-team/__agent_{agent}/{dest}/{suffix}"),
        )
        .expect("valid config path")
    }

    fn scope_files(agent: &str, dest: &str, n: usize) -> Vec<ManifestPathIdentity> {
        (0..n)
            .map(|i| config_file(agent, dest, &format!("file-{i:07}")))
            .collect()
    }

    fn parsed_row_count(project: &Path) -> usize {
        let bytes = std::fs::read(canonical_path(project)).expect("read canonical");
        match parse_manifest_bytes(&bytes) {
            Ok(state) => state.rows.len(),
            Err(error) => panic!("canonical must parse: {error}"),
        }
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

    /// Write an N-row manifest directly (fixture generation is outside the
    /// measured interval per plan section 7.4). Rows spread across
    /// `rows_per_scope`-sized config scopes so the small-scope benchmark can
    /// mutate one scope inside a whole N-row file.
    fn seed_whole_manifest(project: &Path, total_rows: usize, rows_per_scope: usize) {
        let mut state = ManifestState::default();
        let scopes = total_rows.div_ceil(rows_per_scope);
        let mut remaining = total_rows;
        for s in 0..scopes {
            let agent = format!("a{s:06}");
            let scope = config_scope(&agent, ".claude");
            let count = remaining.min(rows_per_scope);
            remaining -= count;
            for i in 0..count {
                let row = PublishedManifestRow::replica_config(
                    config_file(&agent, ".claude", &format!("file-{i:05}")),
                    scope.clone(),
                    ManifestSource::WorkspaceBase,
                    timestamp("2026-07-16T19:41:12.456Z"),
                )
                .expect("config row");
                let _ = state.upsert(row);
            }
        }
        let bytes = serialize_state(&state).expect("serialize whole manifest");
        std::fs::write(canonical_path(project), bytes).expect("write whole manifest");
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

    fn enforce_hard_gate(label: &str, rows: usize, elapsed: Duration, working_set_delta: u64) {
        // The 10 s / 512 MiB gate is a release-mode reference-machine contract
        // (plan section 7.4). A debug run records only; it never gates.
        if rows == 100_000 && !cfg!(debug_assertions) {
            assert!(
                elapsed <= HARD_ELAPSED_LIMIT,
                "{label}: 100k op took {elapsed:?}, over the 10 s gate"
            );
            assert!(
                working_set_delta <= HARD_WORKING_SET_LIMIT,
                "{label}: 100k op used {working_set_delta} bytes additional working set, over 512 MiB"
            );
        }
    }

    // -- plan section 10.5 item 6: whole-scope replacement at 1k/10k/100k --
    #[test]
    #[ignore = "release-mode scale benchmark (plan 7.4/10.5); run manually or as Stage F acceptance"]
    fn bench_whole_scope_replacement_1k_10k_100k() {
        for &n in &BENCH_SIZES {
            let (_temp, project) = setup_project();
            let activation = ManifestActivationToken::for_test();
            let scope = config_scope("alpha", ".claude");
            {
                let mut guard = ProjectSeedManifestGuard::acquire(&project).expect("acquire");
                let batch = PublishedScopeBatch::new(
                    scope.clone(),
                    ManifestSource::WorkspaceBase,
                    scope_files("alpha", ".claude", n),
                    timestamp("2026-07-16T19:41:12.456Z"),
                )
                .expect("seed batch");
                assert_eq!(
                    guard.publication_permit().replace_scope(&activation, batch),
                    ManifestRecordOutcome::Recorded
                );
                guard.release();
            }

            let baseline = working_set_bytes();
            let sampler = WorkingSetSampler::start();
            let started = Instant::now();
            {
                let mut guard = ProjectSeedManifestGuard::acquire(&project).expect("acquire");
                let batch = PublishedScopeBatch::new(
                    scope.clone(),
                    ManifestSource::MatrixBase,
                    scope_files("alpha", ".claude", n),
                    timestamp("2026-07-16T19:42:12.456Z"),
                )
                .expect("replacement batch");
                assert_eq!(
                    guard.publication_permit().replace_scope(&activation, batch),
                    ManifestRecordOutcome::Recorded
                );
                guard.release();
            }
            let elapsed = started.elapsed();
            let peak = sampler.finish();
            let delta = peak.saturating_sub(baseline);
            let canonical_bytes = std::fs::metadata(canonical_path(&project))
                .map(|m| m.len())
                .unwrap_or(0);

            // One transaction produced the whole N-row result: no leftover temp
            // and exactly N rows on disk (plan section 10.5 item 5, observational).
            assert_eq!(
                temp_leftovers(&project),
                0,
                "one transaction, no leftover temp"
            );
            assert_eq!(
                parsed_row_count(&project),
                n,
                "whole scope replaced to N rows"
            );
            eprintln!(
                "[stage-e bench whole-scope] rows={n} elapsed={elapsed:?} \
                 working_set_delta_bytes={delta} canonical_bytes={canonical_bytes}"
            );
            enforce_hard_gate("whole-scope replacement", n, elapsed, delta);
        }
    }

    // -- plan section 10.5 item 6: small-scope mutation inside a whole N-row manifest --
    #[test]
    #[ignore = "release-mode scale benchmark (plan 7.4/10.5); run manually or as Stage F acceptance"]
    fn bench_small_scope_mutation_in_whole_manifest_1k_10k_100k() {
        for &n in &BENCH_SIZES {
            let (_temp, project) = setup_project();
            let activation = ManifestActivationToken::for_test();
            // Whole manifest of N rows across many scopes; mutate one small scope.
            seed_whole_manifest(&project, n, 100);
            let target_scope = config_scope("a000000", ".claude");

            let baseline = working_set_bytes();
            let sampler = WorkingSetSampler::start();
            let started = Instant::now();
            {
                let mut guard = ProjectSeedManifestGuard::acquire(&project).expect("acquire");
                let batch = PublishedScopeBatch::new(
                    target_scope,
                    ManifestSource::MatrixBase,
                    scope_files("a000000", ".claude", 2),
                    timestamp("2026-07-16T20:00:00.000Z"),
                )
                .expect("small batch");
                assert_eq!(
                    guard.publication_permit().replace_scope(&activation, batch),
                    ManifestRecordOutcome::Recorded
                );
                guard.release();
            }
            let elapsed = started.elapsed();
            let peak = sampler.finish();
            let delta = peak.saturating_sub(baseline);
            let canonical_bytes = std::fs::metadata(canonical_path(&project))
                .map(|m| m.len())
                .unwrap_or(0);

            assert_eq!(
                temp_leftovers(&project),
                0,
                "one transaction, no leftover temp"
            );
            eprintln!(
                "[stage-e bench small-scope] rows={n} elapsed={elapsed:?} \
                 working_set_delta_bytes={delta} canonical_bytes={canonical_bytes}"
            );
            enforce_hard_gate("small-scope mutation", n, elapsed, delta);
        }
    }

    // -- plan section 10.5 item 8: git diff --numstat / byte / time evidence --
    #[test]
    #[ignore = "release-mode scale benchmark; records git diff evidence (plan 7.4/10.5 item 8)"]
    fn bench_git_diff_numstat_1k_10k_100k() {
        if std::process::Command::new("git")
            .arg("--version")
            .output()
            .map(|o| !o.status.success())
            .unwrap_or(true)
        {
            eprintln!("[stage-e bench git-diff] git unavailable; recording skip");
            return;
        }
        for &n in &BENCH_SIZES {
            let (_temp, project) = setup_project();
            let git = |args: &[&str]| {
                let status = std::process::Command::new("git")
                    .args(args)
                    .current_dir(&project)
                    .output()
                    .expect("run git");
                assert!(
                    status.status.success(),
                    "git {args:?} failed: {}",
                    String::from_utf8_lossy(&status.stderr)
                );
                status
            };
            git(&["init", "--quiet"]);
            git(&["config", "user.email", "stage-e@example.test"]);
            git(&["config", "user.name", "stage-e"]);

            // Baseline: empty valid manifest committed.
            let empty = serialize_state(&ManifestState::default()).expect("serialize empty");
            std::fs::write(canonical_path(&project), empty).expect("write empty manifest");
            git(&["add", "-A"]);
            git(&["commit", "--quiet", "-m", "baseline"]);

            // Publish an N-row scope, then measure the git diff.
            seed_whole_manifest(&project, n, 100);
            let canonical_bytes = std::fs::metadata(canonical_path(&project))
                .map(|m| m.len())
                .unwrap_or(0);
            let started = Instant::now();
            let numstat = std::process::Command::new("git")
                .args(["diff", "--numstat", "--", ".ac/seed-manifest.toml"])
                .current_dir(&project)
                .output()
                .expect("git diff");
            let diff_time = started.elapsed();
            assert!(numstat.status.success(), "git diff --numstat must succeed");
            let numstat_text = String::from_utf8_lossy(&numstat.stdout);
            eprintln!(
                "[stage-e bench git-diff] rows={n} canonical_bytes={canonical_bytes} \
                 diff_time={diff_time:?} numstat={}",
                numstat_text.trim()
            );
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
            "coverage = [\"project_context_templates\", \"replica_config_folders\", \"coding_agent_catalog\"]\n",
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
