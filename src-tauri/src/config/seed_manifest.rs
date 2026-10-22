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

    fn is_canonical_output(self) -> bool {
        match self {
            Self::ProjectContextTemplate => true,
            Self::ReplicaConfigFile => false,
            Self::CodingAgentCatalog => true,
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

    #[cfg(test)]
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

    fn canonical_rows(&self) -> impl Iterator<Item = &PublishedManifestRow> {
        self.rows
            .values()
            .filter(|row| row.kind.is_canonical_output())
    }

    fn to_wire(&self) -> SeedManifestWire {
        SeedManifestWire {
            schema_version: SCHEMA_VERSION,
            coverage_version: COVERAGE_VERSION,
            coverage: COVERAGE.map(str::to_string).to_vec(),
            files: self
                .canonical_rows()
                .map(PublishedManifestRow::to_wire)
                .collect(),
        }
    }

    fn upsert(&mut self, row: PublishedManifestRow) -> RowMutationJournal {
        let key = row.path.key();
        let previous = self.rows.insert(key.clone(), row);
        RowMutationJournal { key, previous }
    }

    #[cfg(test)]
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

#[cfg(test)]
#[derive(Debug)]
struct ScopeMutationJournal {
    removed: Vec<(ManifestPathKey, PublishedManifestRow)>,
    inserted: Vec<ManifestPathKey>,
}

#[cfg(test)]
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
                // #1605: the per-platform `{{HOST_PLATFORM_RULES}}` files join the
                // project-context scope set (mirrors
                // `manifest_scope_for_project_context_filename` in session_context.rs;
                // literals kept here to avoid a new module arc).
                ".ac/Context.platform.windows.md" => "context:platform",
                ".ac/Context.platform.linux.md" => "context:platform",
                ".ac/Context.platform.macos.md" => "context:platform",
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
            "config scope must be config:.ac/<room>/<replica>/<dest>: {scope}"
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
        + "coverage = [\"project_context_templates\", \"replica_config_folders\", \
            \"coding_agent_catalog\"]\n"
            .len()
}

fn exact_serialized_len(state: &ManifestState) -> Result<usize, SeedManifestError> {
    exact_serialized_len_with_limit(state, MAX_MANIFEST_BYTES)
}

fn exact_serialized_len_with_limit(
    state: &ManifestState,
    output_limit: u64,
) -> Result<usize, SeedManifestError> {
    let canonical_row_count = state.canonical_rows().count();
    if canonical_row_count > MAX_MANIFEST_ROWS {
        return Err(SeedManifestError::resource_bound(
            ResourceBoundKind::Rows,
            MAX_MANIFEST_ROWS,
            canonical_row_count,
        ));
    }

    let mut length = MANAGED_HEADER.len();
    checked_add(&mut length, "schema_version = 1\n".len())?;
    checked_add(&mut length, "coverage_version = 2\n".len())?;
    checked_add(
        &mut length,
        "coverage = [\"project_context_templates\", \"replica_config_folders\", \
            \"coding_agent_catalog\"]\n"
            .len(),
    )?;
    if canonical_row_count == 0 {
        checked_add(&mut length, "files = []\n".len())?;
    } else {
        for row in state.canonical_rows() {
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

/// #1968 - narrow READ-ONLY query: does this project's seed manifest carry a
/// canonical coding-agent catalog publication row for
/// `.ac/coding-agents/agents.json`?
///
/// Deliberately not a guard: it takes no lock, creates no directory, upgrades no
/// schema and writes nothing, so a caller may use it to decide whether
/// bookkeeping is missing. It reuses the canonical reader stack
/// ([`open_regular_no_follow`] + [`OpenedRegularFile`] + [`read_bounded_file`] +
/// [`parse_manifest_bytes`]), revalidates the opened identity across the read,
/// and compares the PARSED [`ManifestPathIdentity`] (never raw path text) plus
/// the canonical row kind, the catalog scope and the `Builtin` source.
///
/// * `Ok(true)` - at least one parsed canonical row matches.
/// * `Ok(false)` - the manifest is absent (exact NotFound) or carries no such
///   row. Absence of proof is not proof, so the caller reports an untracked
///   publication rather than assuming the row was ever written.
/// * `Err` - malformed, future, oversized, nonregular, link or unreadable
///   manifests.
pub(crate) fn has_catalog_publication(project_root: &Path) -> Result<bool, SeedManifestError> {
    let canonical_path = project_root
        .join(crate::config::ac_root::CANONICAL_AC_ROOT_DIR)
        .join(SEED_MANIFEST_FILENAME);
    let mut canonical = match open_regular_no_follow(&canonical_path, false, false) {
        Ok(file) => OpenedRegularFile::from_file(&canonical_path, file)?,
        Err(error) if is_exact_not_found(&error) => return Ok(false),
        Err(error) => {
            return Err(classify_open_error(
                &canonical_path,
                error,
                "open canonical manifest",
                false,
            ))
        }
    };
    let raw = read_bounded_file(
        &mut canonical.file,
        canonical.length,
        MAX_MANIFEST_BYTES,
        &canonical_path,
    )?;
    let reopened = OpenedRegularFile::open_existing(&canonical_path, false)?;
    if reopened.identity != canonical.identity {
        return Err(SeedManifestError::UnsafePath {
            path: canonical_path,
            reason: "canonical identity changed during its bounded read".to_string(),
        });
    }
    let state = parse_manifest_bytes(&raw)?;
    let identity =
        ManifestPathIdentity::from_relative_path(Path::new(".ac/coding-agents/agents.json"))?;
    Ok(state.rows.values().any(|row| {
        row.path == identity
            && row.kind == ManifestFileKind::CodingAgentCatalog
            && row.scope == "catalog:coding-agents"
            && row.source == ManifestSource::Builtin
    }))
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

        let ac_path = canonical_project_path.join(crate::config::ac_root::CANONICAL_AC_ROOT_DIR);
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
                    "[seed_manifest] canonical manifest is preserved read-only path={} reason={:?} \
                        diagnostic_kind={}",
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
    /// strict: the v2 state is built by SUBSTITUTION from the parsed v1 wire
    /// (`coverage_version` and the coverage list only), so every existing row
    /// check and bound runs verbatim and no timestamp is invented. Canonical
    /// output preserves supported rows and retains legacy replica-config rows
    /// only in the held state. The atomic write goes through
    /// [`Self::write_canonical`] DIRECTLY (not
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
                    "[seed_manifest] v1->v2 upgrade re-read failed path={} error={}; manifest \
                        stays preserved read-only",
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
                    "[seed_manifest] v1->v2 upgrade skipped: manifest is not UTF-8 at byte offset \
                        {} path={}; manifest stays preserved read-only",
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
                    "[seed_manifest] v1->v2 upgrade skipped path={} error={}; manifest stays \
                        preserved read-only",
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
                    "[seed_manifest] v1 manifest rows failed v2 validation path={} error={}; \
                        manifest stays preserved read-only",
                    canonical_path.display(),
                    sanitize_canonical_validation_error(error)
                );
                restore(self, canonical);
                return;
            }
        };

        // e. Serialize canonical output and write under the held lock. The held
        // state still retains every validated v1 row. `write_canonical` updates
        // the snapshot itself on success; on failure its temp is cleaned and the
        // pre-migration degraded snapshot is restored (bytes preserved exactly,
        // writer disabled, published-unrecorded).
        let bytes = match serialize_state(&state) {
            Ok(bytes) => bytes,
            Err(error) => {
                log::warn!(
                    "[seed_manifest] v1->v2 upgrade serialize failed path={} error={}; manifest \
                        stays preserved read-only",
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
                "[seed_manifest] v1->v2 upgrade write failed path={} error={}; manifest stays \
                    preserved read-only",
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
            "[seed_manifest] bounded prior-temp inventory root={} policy={} exact={} malformed={} \
                malformed_native={} removed={} preserved={} entry_errors={} scan_truncated={} \
                scan_error={:?} exact_samples={:?} malformed_samples={:?}",
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
                    "[seed_manifest] parent-directory sync failed after canonical replace root={} \
                        error={}",
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

    /// The sole production activation constructor.
    ///
    /// Supported publishers and lifecycle hooks thread this token into the
    /// recorder. Historical entries in [`V1_COVERAGE_BOUNDARIES`] need not remain
    /// active publishers. There is deliberately no other non-test construction
    /// path, so code without this token cannot mutate a manifest.
    pub(crate) fn production() -> Self {
        Self { _private: () }
    }

    fn authorize(&self) {}
}

/// Historical Stage F coverage-boundary declaration.
///
/// These stable names preserve the compatibility vocabulary established by the
/// original activation rollout. They include historical publication boundaries
/// that no longer emit canonical rows, so this declaration is not a checklist of
/// current production publishers or wiring evidence.
///
/// #1318 - [`V1CoverageBoundary::CatalogSeed`] is the first coverage-v2-era
/// publisher; the enum keeps its historical name (minimal diff) while the
/// declaration remains the exhaustive historical coverage-boundary checklist
/// across coverage versions.
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
    /// Historical stable coverage-boundary name for exact config publication.
    ConfigExactPublish,
    /// Historical stable coverage-boundary name for over-bound config publication.
    ConfigOverBoundPublish,
    /// Historical stable coverage-boundary name for failed config restoration.
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

#[cfg(test)]
#[derive(Debug, Clone)]
pub(crate) struct PublishedScopeBatch {
    scope: String,
    rows: Vec<PublishedManifestRow>,
}

#[cfg(test)]
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
                "lifecycle config prefix must include a room component".to_string(),
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

    #[cfg(test)]
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

    #[cfg(test)]
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

    #[cfg(test)]
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
        "[seed_manifest] target or lifecycle action succeeded but manifest persistence did not \
            scope={} error={}",
        scope,
        error
    );
}

#[cfg(test)]
// The shared test module deliberately precedes the cfg-gated Windows implementation it exercises.
mod tests;

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
                    "[seed_manifest] Windows namespace failure operation={:?} raw_error={} \
                        raw_error_hex=0x{:08x} canonical={} temp={} backup=NULL ac_identity={:?} \
                        lock_identity={:?} source_identity={:?} destination_identity={:?} \
                        destination_handle_live={} canonical_state={:?} temp_state={:?}",
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
                                    "[seed_manifest] Windows current-owned temp cleanup failed \
                                        after MoveFileExW raw_error={} path={} cleanup_error={}",
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
                                    "[seed_manifest] Windows current-owned temp cleanup failed \
                                        after ReplaceFileW raw_error={} path={} cleanup_error={}",
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
                                    "[seed_manifest] ReplaceFileW error 1175 contradicted the \
                                        documented stable-name state; preserving all observed names"
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
