use super::super::projects;
use super::{
    agents_instance_groups, agents_instance_path, apply_project_decode_to_value,
    canonical_profile_source, decode_physical_catalog, duplicate_profile_document,
    is_valid_profile_letter, physical_profile_rows, production_instance_base,
    project_physical_profiles, raw_configuration_command, refresh_physical_source_rows,
    registered_profile_raw_gate, registered_profiles_for_read, retain_configuration_commands,
    AgentConfig, AppSettings, CodingAgentEnv, CodingAgentProfilesConfig,
    IdentityReferenceCompleteProof, IdentityReferenceError, IdentityReferencePacket,
    IdentityReferencePhysicalState, IdentityReferenceProtectedRevision, ProfileCellConfig,
    CONFIGURATION_COMMAND_PAIRS, OVERLAY_DERIVED_ID_CLOSURES, OVERLAY_INELIGIBLE_DISK_KEYS,
    OVERLAY_INELIGIBLE_LEGACY_KEYS,
};
use crate::config::instance_artifacts::{SETTINGS_FILE_NAME, SETTINGS_LOCAL_OVERRIDE_FILE_NAME};
use crate::config::local_overlay::LocalSettingsOverlay;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

// #2892 P01: additive source-aware contracts. These are data and pure helpers;
// none of them changes the legacy selection, settings or persistence paths.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SourceFamily {
    Agents,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SourceRole {
    Catalog,
    Registered,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SourceScope {
    Project,
    Instance,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SourceKind {
    CatalogBase,
    CatalogProject,
    CatalogPersonal,
    RegisteredInstance,
}

/// Logical source only. A backend resolver must derive and authorize its locator
/// and capabilities; a deserialized reference grants no filesystem authority.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SourceRef {
    pub family: SourceFamily,
    pub role: SourceRole,
    pub scope: SourceScope,
    pub kind: SourceKind,
    pub context_root: String,
}

/// Physical absence is distinct from every byte revision, including empty bytes.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum SourceRevision {
    Bytes {
        #[serde(deserialize_with = "deserialize_source_sha256")]
        sha256: String,
    },
    Absent {},
}

pub(crate) fn deserialize_source_sha256<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let sha256 = String::deserialize(deserializer)?;
    if sha256.len() != 64
        || !sha256
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(serde::de::Error::custom(
            "sha256 must be a full lowercase SHA256 digest",
        ));
    }
    Ok(sha256)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConfigurationRef {
    pub version: u32,
    pub source: SourceRef,
    pub calculated_id: String,
    #[serde(default)]
    pub requested_profile: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_calculated_id: Option<String>,
    /// Existing local key/id, never an alternate calculated identity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub legacy_entry_hint: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SourceSelectionTicket {
    #[serde(rename = "ref")]
    pub configuration_ref: ConfigurationRef,
    pub expected_source_revision: SourceRevision,
    pub catalog_context_revision: String,
}

/// Generic companions keep the legacy pair and complete physical CAS contract
/// owned by their existing modules, without importing profiles into settings.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SourceSelectionState<LegacyPair, PhysicalRevision> {
    pub legacy_pair: LegacyPair,
    #[serde(rename = "ref")]
    pub configuration_ref: Option<ConfigurationRef>,
    pub physical_revision: PhysicalRevision,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SourceSelectionExpectation<LegacyPair, PhysicalRevision> {
    pub legacy_pair: LegacyPair,
    #[serde(rename = "ref")]
    pub configuration_ref: Option<ConfigurationRef>,
    pub physical_revision: PhysicalRevision,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SourceSelectionDefault<LegacyPair, PhysicalRevision> {
    pub legacy_pair: LegacyPair,
    #[serde(rename = "ref")]
    pub configuration_ref: ConfigurationRef,
    pub physical_revision: PhysicalRevision,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SourceAvailability {
    Available,
    Absent,
    Invalid,
    IndependentFormatRequired,
    ContextUnavailable,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SourceCapabilities {
    pub available_read: bool,
    pub editable_create: bool,
    pub editable_save: bool,
    pub repair_by_owner: bool,
    pub publish_managed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SourceDiagnostic {
    pub code: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entry_hint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub existing_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requested_profile: Option<String>,
}

/// The agent model is source-owned: registered entries and catalog definitions
/// need not be converted into one another to carry an independent snapshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SourceSnapshot<Agent> {
    pub source: SourceRef,
    pub revision: SourceRevision,
    pub availability: SourceAvailability,
    pub capabilities: SourceCapabilities,
    pub agents: Vec<Agent>,
    pub profiles: BTreeMap<String, BTreeMap<String, ProfileCellConfig>>,
    pub diagnostics: Vec<SourceDiagnostic>,
    pub context_revision: String,
}

// #2897 P03: compiled gate; no caller-controlled activation bypass.
#[allow(dead_code)]
pub(crate) const fn source_protocol_active() -> bool {
    false
}

/// PAIR layout belongs beside the physical identity-reference facade.
#[allow(dead_code)]
pub(crate) fn identity_reference_pair_paths(root: &Path) -> (PathBuf, PathBuf, PathBuf) {
    (
        root.join("config.json"),
        root.join(crate::config::instance_artifacts::CONFIG_STATE_TARGET_NAME),
        root.join(crate::config::instance_artifacts::CONFIG_IDENTITY_RESERVATION_NAME),
    )
}

#[allow(dead_code)]
pub(crate) fn read_identity_reference_physical(
    path: &Path,
) -> Result<IdentityReferencePhysicalState, IdentityReferenceError> {
    crate::config::local_config_io::read_config_pair_physical(path)
}

#[allow(dead_code)]
pub(crate) fn identity_reference_protected_revision(
    tuple: &[IdentityReferencePhysicalState; 2],
) -> Result<[IdentityReferenceProtectedRevision; 2], IdentityReferenceError> {
    crate::config::local_config_io::protected_config_pair_revision(tuple)
}

#[allow(dead_code)]
pub(crate) fn execute_identity_reference_pair_guarded(
    decisions: &Path,
    state: &Path,
    reservation: &Path,
    operation_id: &str,
    validate: &dyn Fn() -> Result<(), IdentityReferenceError>,
) -> Result<(), IdentityReferenceError> {
    crate::config::local_config_io::execute_reserved_config_pair_guarded(
        decisions,
        state,
        reservation,
        operation_id,
        validate,
    )
}

#[allow(dead_code)]
pub(crate) fn verify_identity_reference_pair_guarded(
    decisions: &Path,
    state: &Path,
    reservation: &Path,
    operation_id: &str,
    plan_digest: &str,
    validate: &dyn Fn() -> Result<(), IdentityReferenceError>,
) -> Result<[IdentityReferencePhysicalState; 2], IdentityReferenceError> {
    crate::config::local_config_io::verify_reserved_config_pair_guarded(
        decisions,
        state,
        reservation,
        operation_id,
        plan_digest,
        validate,
    )
}

#[allow(dead_code)]
pub(crate) fn release_identity_reference_pair_guarded(
    decisions: &Path,
    state: &Path,
    reservation: &Path,
    proof: &IdentityReferenceCompleteProof,
    validate: &dyn Fn() -> Result<(), IdentityReferenceError>,
) -> Result<[IdentityReferencePhysicalState; 2], IdentityReferenceError> {
    crate::config::local_config_io::release_reserved_config_pair_guarded(
        decisions,
        state,
        reservation,
        proof,
        validate,
    )
}

// Dormant single-editor source primitives. C serializes internal writes;
// settings owns authorization, original inventory and the local journal.
// No legacy loader, Save endpoint or transport invokes these primitives yet.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[allow(dead_code)]
pub(crate) enum SourceCoordinationError {
    #[error("sourceIdentityUncertain")]
    IdentityUncertain,
    #[error("sourceBusy")]
    Busy,
    #[error("sourceTransitionPending")]
    Pending,
    #[error("staleRevision")]
    Stale,
    #[error("journalInvalid")]
    Invalid,
    #[error("notFound")]
    NotFound,
    #[error("unauthorized")]
    Unauthorized,
    #[error("sourceCoordinationIo: {0}")]
    Io(String),
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
pub(crate) enum SourceTransitionPhase {
    IntentStored,
    Prepared,
    SourcePublished,
    Complete,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[allow(dead_code)]
pub(crate) struct SourcePacketRecord {
    pub(crate) target: PathBuf,
    pub(crate) target_id: String,
    pub(crate) before_protected_revision: SourceRevision,
    pub(crate) after_protected_revision: SourceRevision,
    pub(crate) plan_digest: String,
}

// This is metadata only; original candidate bytes stay in the private Intent.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[allow(dead_code)]
pub(crate) struct SourceIdentityMapping {
    pub(crate) entry_hint: String,
    pub(crate) old_calculated_id: Option<String>,
    pub(crate) new_calculated_id: Option<String>,
    pub(crate) profiles: BTreeMap<String, (Option<String>, Option<String>)>,
    pub(crate) unavailable: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[allow(dead_code)]
pub(crate) struct SourceReceipt {
    pub(crate) operation_id: String,
    pub(crate) source_revision: SourceRevision,
}

pub(crate) fn coordination_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
pub(crate) fn coordination_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}
pub(crate) fn valid_coordination_revision(revision: &SourceRevision) -> bool {
    match revision {
        SourceRevision::Absent {} => true,
        SourceRevision::Bytes { sha256 } => coordination_digest(sha256),
    }
}

#[allow(dead_code)]
pub(crate) struct IdentityWriteLease {
    pub(crate) _lock: crate::config::local_config_io::SidecarWriteLock,
    pub(crate) parent: crate::path_identity::RetainedDirectory,
    pub(crate) instance_id: String,
}
#[allow(dead_code)]
impl IdentityWriteLease {
    pub(crate) fn instance_id(&self) -> &str {
        &self.instance_id
    }

    pub(crate) fn acquire(
        context: &SourceReadContext,
        timeout: std::time::Duration,
    ) -> Result<Self, SourceCoordinationError> {
        #[cfg(test)]
        SOURCE_COORDINATION_ACQUISITIONS.with(|count| {
            let mut value = count.get();
            value[0] += 1;
            count.set(value);
        });
        let parent = coordination_parent(&context.settings_path)?;
        let canonical = &parent.identity().canonical_path;
        let spelling = canonical
            .to_str()
            .ok_or(SourceCoordinationError::IdentityUncertain)?;
        let spelling = if cfg!(windows) {
            spelling.to_lowercase()
        } else {
            spelling.into()
        };
        let SourceRevision::Bytes {
            sha256: instance_id,
        } = source_bytes_revision(spelling.as_bytes())
        else {
            unreachable!()
        };
        let lock = crate::config::local_config_io::acquire_sidecar_write_lock(
            &canonical.join(crate::config::instance_artifacts::IDENTITY_TRANSITION_LOCK_NAME),
            timeout,
            "identityWriteTimeout",
            "identity write lock",
        )
        .map_err(SourceCoordinationError::Io)?;
        parent
            .verify_current()
            .map_err(|_| SourceCoordinationError::IdentityUncertain)?;
        Ok(Self {
            _lock: lock,
            parent,
            instance_id,
        })
    }
}

#[allow(dead_code)]
pub(crate) struct SourceWriteContext<'a> {
    pub(crate) identity: &'a IdentityWriteLease,
    pub(crate) parent: crate::path_identity::RetainedDirectory,
    pub(crate) source_path: PathBuf,
    pub(crate) source: SourceRef,
    pub(crate) context_revision: String,
    pub(crate) physical_source_key: String,
}

#[allow(dead_code)]
impl<'a> SourceWriteContext<'a> {
    pub(crate) fn source(&self) -> &SourceRef {
        &self.source
    }

    pub(crate) fn physical_source_key(&self) -> &str {
        &self.physical_source_key
    }

    pub(crate) fn context_revision(&self) -> &str {
        &self.context_revision
    }

    /// Internal only: caller acquires C first and supplies backend context.
    /// Parent object + normalized basename stays stable over atomic replace.
    pub(crate) fn acquire(
        context: &SourceReadContext,
        source: &SourceRef,
        identity: &'a IdentityWriteLease,
    ) -> Result<Self, SourceCoordinationError> {
        let own = coordination_parent(&context.settings_path)?;
        if !crate::path_identity::same_object(own.identity(), identity.parent.identity()) {
            return Err(SourceCoordinationError::Unauthorized);
        }
        identity
            .parent
            .verify_current()
            .map_err(|_| SourceCoordinationError::IdentityUncertain)?;
        let locator = context
            .locator(source)
            .map_err(|_| SourceCoordinationError::Unauthorized)?;
        let parent = coordination_parent(&locator)?;
        let canonical = parent.identity().canonical_path.clone();
        let name = locator
            .file_name()
            .ok_or(SourceCoordinationError::IdentityUncertain)?;
        let key = coordination_key(&parent.identity().object_id, name)?;
        // Legacy artifacts are diagnosed and preserved, never used as authority.
        reject_legacy_source_ledger(&canonical.join(source_ledger_name(source.kind)))?;
        let lease = Self {
            identity,
            parent,
            source_path: canonical.join(name),
            source: source.clone(),
            context_revision: context.revision.clone(),
            physical_source_key: key,
        };
        lease.revalidate(context)?;
        Ok(lease)
    }

    pub(crate) fn revalidate(
        &self,
        context: &SourceReadContext,
    ) -> Result<(), SourceCoordinationError> {
        if context.revision != self.context_revision {
            return Err(SourceCoordinationError::Stale);
        }
        let locator = context
            .locator(&self.source)
            .map_err(|_| SourceCoordinationError::Unauthorized)?;
        let current = coordination_parent(&locator)?;
        if !crate::path_identity::same_object(current.identity(), self.parent.identity()) {
            return Err(SourceCoordinationError::IdentityUncertain);
        }
        self.parent
            .verify_current()
            .map_err(|_| SourceCoordinationError::IdentityUncertain)?;
        reject_legacy_source_ledger(
            &self
                .parent
                .identity()
                .canonical_path
                .join(source_ledger_name(self.source.kind)),
        )?;
        coordination_read(&self.source_path)?;
        Ok(())
    }

    pub(crate) fn source_revision(&self) -> Result<SourceRevision, SourceCoordinationError> {
        self.identity
            .parent
            .verify_current()
            .map_err(|_| SourceCoordinationError::IdentityUncertain)?;
        self.parent
            .verify_current()
            .map_err(|_| SourceCoordinationError::IdentityUncertain)?;
        Ok(coordination_read(&self.source_path)?.0)
    }
}

pub(crate) fn coordination_publication_error(
    error: IdentityReferenceError,
) -> SourceCoordinationError {
    match error {
        IdentityReferenceError::Conflict => SourceCoordinationError::Stale,
        IdentityReferenceError::InvalidPlan(_) => SourceCoordinationError::Invalid,
        other => SourceCoordinationError::Io(other.to_string()),
    }
}

pub(crate) fn coordination_parent(
    path: &Path,
) -> Result<crate::path_identity::RetainedDirectory, SourceCoordinationError> {
    let parent = std::fs::canonicalize(
        path.parent()
            .ok_or(SourceCoordinationError::IdentityUncertain)?,
    )
    .map_err(|_| SourceCoordinationError::IdentityUncertain)?;
    crate::path_identity::retain_directory(&parent)
        .map_err(|_| SourceCoordinationError::IdentityUncertain)
}
pub(crate) fn coordination_key(
    object: &crate::path_identity::FileObjectId,
    name: &std::ffi::OsStr,
) -> Result<String, SourceCoordinationError> {
    let name = name
        .to_str()
        .ok_or(SourceCoordinationError::IdentityUncertain)?;
    let name = if cfg!(windows) {
        name.to_lowercase()
    } else {
        name.into()
    };
    let bytes = serde_json::to_vec(&(object.volume, object.file, name))
        .map_err(|_| SourceCoordinationError::Invalid)?;
    let SourceRevision::Bytes { sha256 } = source_bytes_revision(&bytes) else {
        unreachable!()
    };
    Ok(sha256)
}
pub(crate) fn source_ledger_name(kind: SourceKind) -> &'static str {
    use crate::config::instance_artifacts::{
        SOURCE_CATALOG_BASE_LEDGER_NAME, SOURCE_CATALOG_PERSONAL_LEDGER_NAME,
        SOURCE_CATALOG_PROJECT_LEDGER_NAME, SOURCE_REGISTERED_INSTANCE_LEDGER_NAME,
    };
    match kind {
        SourceKind::CatalogBase => SOURCE_CATALOG_BASE_LEDGER_NAME,
        SourceKind::CatalogProject => SOURCE_CATALOG_PROJECT_LEDGER_NAME,
        SourceKind::CatalogPersonal => SOURCE_CATALOG_PERSONAL_LEDGER_NAME,
        SourceKind::RegisteredInstance => SOURCE_REGISTERED_INSTANCE_LEDGER_NAME,
    }
}
pub(crate) fn coordination_read(
    path: &Path,
) -> Result<(SourceRevision, Option<Vec<u8>>), SourceCoordinationError> {
    match std::fs::symlink_metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok((SourceRevision::Absent {}, None)),
        Err(e) => Err(SourceCoordinationError::Io(e.to_string())),
        Ok(_) => {
            let (bytes, _) = crate::path_identity::read_bounded_regular(path, 16 * 1024 * 1024)
                .map_err(|_| SourceCoordinationError::IdentityUncertain)?;
            Ok((source_bytes_revision(&bytes), Some(bytes)))
        }
    }
}

pub(crate) fn reject_legacy_source_ledger(path: &Path) -> Result<(), SourceCoordinationError> {
    match std::fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(SourceCoordinationError::Io(error.to_string())),
        Ok(_) => Err(SourceCoordinationError::Invalid),
    }
}

#[derive(Clone, Debug)]
#[allow(dead_code)]
pub(crate) struct IdentityTransitionSummary {
    pub(crate) operation_id: String,
    pub(crate) phase: SourceTransitionPhase,
    pub(crate) source: SourceRef,
    pub(crate) source_revision_observed: SourceRevision,
    pub(crate) expected_source_revision: SourceRevision,
    pub(crate) after_source_revision: SourceRevision,
    pub(crate) journal_revision: SourceRevision,
    pub(crate) mappings: Vec<SourceIdentityMapping>,
    pub(crate) receipt: Option<SourceReceipt>,
}

#[derive(Clone, Debug)]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) struct SourceTransitionStatus {
    pub(crate) journal_revision: SourceRevision,
    pub(crate) source_revision_observed: SourceRevision,
    pub(crate) summary: Option<IdentityTransitionSummary>,
}

/// Stable journal -> source -> journal observation; no write proofs or effects.
#[allow(dead_code)]
pub(crate) fn read_source_transition(
    context: &SourceReadContext,
    source: &SourceRef,
    operation_id: &str,
) -> Result<SourceTransitionStatus, SourceCoordinationError> {
    let locator = context
        .locator(source)
        .map_err(|_| SourceCoordinationError::Unauthorized)?;
    let parent = coordination_parent(&locator)?;
    reject_legacy_source_ledger(
        &parent
            .identity()
            .canonical_path
            .join(source_ledger_name(source.kind)),
    )?;
    for _ in 0..=2 {
        let (revision, journal) = read_own_source_journal(context)?;
        let observed = coordination_read(&locator)?.0;
        if read_own_source_journal(context)?.0 != revision {
            continue;
        }
        parent
            .verify_current()
            .map_err(|_| SourceCoordinationError::IdentityUncertain)?;
        let summary = match journal.intents.get(operation_id) {
            None => None,
            Some(intent) => {
                if &intent.source != source {
                    return Err(SourceCoordinationError::Unauthorized);
                }
                let progress = journal
                    .progress
                    .get(operation_id)
                    .ok_or(SourceCoordinationError::Invalid)?;
                Some(IdentityTransitionSummary {
                    operation_id: operation_id.into(),
                    phase: progress.phase(),
                    source: source.clone(),
                    source_revision_observed: observed.clone(),
                    expected_source_revision: intent.expected_revision.clone(),
                    after_source_revision: source_bytes_revision(&intent.serialized_after),
                    journal_revision: revision.clone(),
                    mappings: intent.mappings.clone(),
                    receipt: progress.receipt.clone(),
                })
            }
        };
        return Ok(SourceTransitionStatus {
            journal_revision: revision,
            source_revision_observed: observed,
            summary,
        });
    }
    Err(SourceCoordinationError::Busy)
}

/// Original private Intent. Persisted with inventory before any packet/source effect.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[allow(dead_code)]
pub(crate) struct SourceStoredIntent {
    pub(crate) schema_version: u32,
    pub(crate) operation_id: String,
    pub(crate) request_id: String,
    pub(crate) payload_digest: String,
    pub(crate) source: SourceRef,
    pub(crate) expected_revision: SourceRevision,
    pub(crate) before_bytes: Option<Vec<u8>>,
    pub(crate) serialized_after: Vec<u8>,
    pub(crate) mappings: Vec<SourceIdentityMapping>,
    pub(crate) editor_id: String,
    pub(crate) context_revision: String,
    pub(crate) authorization_revision: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[allow(dead_code)]
pub(crate) struct SourcePrivateJournal {
    pub(crate) schema_version: u32,
    pub(crate) intents: BTreeMap<String, SourceStoredIntent>,
    #[serde(default)]
    pub(crate) inventories: BTreeMap<String, SourceLocalInventory>,
    #[serde(default)]
    pub(crate) progress: BTreeMap<String, SourceLocalProgress>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SourceDiscoveryWitness {
    pub(crate) registry: Vec<String>,
    pub(crate) parents: Vec<PathBuf>,
    pub(crate) slots: Vec<(PathBuf, bool)>,
    pub(crate) membership: Vec<(PathBuf, Vec<PathBuf>)>,
}

#[cfg(test)]
thread_local! {
    pub(crate) static SOURCE_TRANSITION_PUBLICATIONS: std::cell::Cell<[usize; 3]> =
        const { std::cell::Cell::new([0; 3]) };
    pub(crate) static SOURCE_COORDINATION_ACQUISITIONS: std::cell::Cell<[usize; 2]> =
        const { std::cell::Cell::new([0, 0]) };
}

/// Calling-thread oracle only: driver fixtures and local reload execute synchronously
/// on this thread. A worker must return its own counts; these are not global totals.
#[cfg(test)]
pub(crate) fn source_transition_effect_counts() -> ([usize; 2], usize) {
    (
        SOURCE_COORDINATION_ACQUISITIONS.with(std::cell::Cell::get),
        crate::config::local_config_io::source_guard_publication_calls(),
    )
}

#[cfg(test)]
pub(crate) fn source_transition_publication_counts() -> [usize; 3] {
    SOURCE_TRANSITION_PUBLICATIONS.with(std::cell::Cell::get)
}

#[cfg(test)]
pub(crate) fn count_source_transition_publication(kind: usize) {
    SOURCE_TRANSITION_PUBLICATIONS.with(|count| {
        let mut next = count.get();
        next[kind] += 1;
        count.set(next);
    });
}

/// Private locators are frozen by the backend's complete authorized inventory.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SourceInventoryTarget {
    pub(crate) root: PathBuf,
    pub(crate) root_object: (u64, u64),
    pub(crate) decisions: PathBuf,
    pub(crate) state: PathBuf,
    pub(crate) reservation: PathBuf,
    pub(crate) before_protected: [IdentityReferenceProtectedRevision; 2],
    pub(crate) original_selections: Vec<ConfigurationRef>,
    #[serde(default)]
    pub(crate) original_physical: Option<[IdentityReferencePhysicalState; 2]>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SourceLocalInventory {
    pub(crate) editor_id: String,
    pub(crate) source: SourceRef,
    pub(crate) physical_source_key: String,
    pub(crate) context_revision: String,
    pub(crate) authorization_revision: String,
    pub(crate) authorized_roots: Vec<(PathBuf, (u64, u64))>,
    pub(crate) targets: Vec<SourceInventoryTarget>,
    pub(crate) mappings: Vec<SourceIdentityMapping>,
    pub(crate) origin_binding: Option<OriginBinding>,
    #[serde(default)]
    pub(crate) original_evidence: Option<SourceOriginalInventoryEvidence>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SourceLocalProgress {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) source_publication:
        Option<crate::config::local_config_io::PreparedSourcePublication>,
    pub(crate) packets: Vec<SourcePacketRecord>,
    pub(crate) packets_prepared: bool,
    pub(crate) source_published: bool,
    pub(crate) refs_verified: bool,
    pub(crate) reloaded: bool,
    pub(crate) receipt: Option<SourceReceipt>,
}

impl SourceLocalProgress {
    pub(crate) fn phase(&self) -> SourceTransitionPhase {
        if self.receipt.is_some() {
            SourceTransitionPhase::Complete
        } else if self.source_published {
            SourceTransitionPhase::SourcePublished
        } else if self.packets_prepared {
            SourceTransitionPhase::Prepared
        } else {
            SourceTransitionPhase::IntentStored
        }
    }
}

impl SourcePrivateJournal {
    pub(crate) fn empty() -> Self {
        Self {
            schema_version: 2,
            intents: BTreeMap::new(),
            inventories: BTreeMap::new(),
            progress: BTreeMap::new(),
        }
    }

    pub(crate) fn validate(&self) -> Result<(), SourceCoordinationError> {
        if self.schema_version != 2 {
            return Err(SourceCoordinationError::Invalid);
        }
        if self.intents.keys().ne(self.inventories.keys())
            || self.intents.keys().ne(self.progress.keys())
        {
            return Err(SourceCoordinationError::Invalid);
        }
        let mut pending_sources = HashSet::new();
        for (id, inventory) in &self.inventories {
            if self.progress.get(id).is_none_or(|p| p.receipt.is_none())
                && !pending_sources.insert(&inventory.physical_source_key)
            {
                return Err(SourceCoordinationError::Invalid);
            }
        }
        let mut requests = std::collections::BTreeSet::new();
        for (id, intent) in &self.intents {
            if id != &intent.operation_id
                || !coordination_id(id)
                || intent.schema_version != 2
                || !coordination_id(&intent.request_id)
                || !requests.insert(&intent.request_id)
                || !coordination_digest(&intent.payload_digest)
                || !coordination_digest(&intent.editor_id)
                || !coordination_digest(&intent.context_revision)
                || !coordination_digest(&intent.authorization_revision)
                || intent.source.context_root.is_empty()
                || !valid_coordination_revision(&intent.expected_revision)
                || intent.serialized_after.is_empty()
                || intent
                    .before_bytes
                    .as_ref()
                    .map(|bytes| source_bytes_revision(bytes))
                    .unwrap_or(SourceRevision::Absent {})
                    != intent.expected_revision
            {
                return Err(SourceCoordinationError::Invalid);
            }
        }
        for (id, inventory) in &self.inventories {
            inventory.validate_original_evidence()?;
            if !coordination_id(id)
                || !coordination_digest(&inventory.editor_id)
                || !coordination_digest(&inventory.physical_source_key)
                || !coordination_digest(&inventory.context_revision)
                || !coordination_digest(&inventory.authorization_revision)
                || inventory.source.context_root.is_empty()
                || inventory
                    .authorized_roots
                    .windows(2)
                    .any(|pair| pair[0].0 >= pair[1].0)
                || inventory
                    .targets
                    .windows(2)
                    .any(|pair| pair[0].decisions >= pair[1].decisions)
            {
                return Err(SourceCoordinationError::Invalid);
            }
            for target in &inventory.targets {
                if !target.root.is_absolute()
                    || !target.decisions.is_absolute()
                    || !target.state.is_absolute()
                    || !target.reservation.is_absolute()
                    || target.decisions.parent() != Some(target.root.as_path())
                    || target.state.parent() != Some(target.root.as_path())
                    || target.reservation.parent() != Some(target.root.as_path())
                    || target.decisions == target.state
                    || target.decisions == target.reservation
                    || target.state == target.reservation
                    || !inventory
                        .authorized_roots
                        .iter()
                        .any(|(root, _)| target.root.starts_with(root))
                {
                    return Err(SourceCoordinationError::Invalid);
                }
            }
            if let Some(intent) = self.intents.get(id) {
                if intent.source != inventory.source
                    || intent.mappings != inventory.mappings
                    || intent.editor_id != inventory.editor_id
                    || intent.context_revision != inventory.context_revision
                    || intent.authorization_revision != inventory.authorization_revision
                {
                    return Err(SourceCoordinationError::Invalid);
                }
            }
        }
        for (id, progress) in &self.progress {
            if let Some(proof) = &progress.source_publication {
                let intent = self
                    .intents
                    .get(id)
                    .ok_or(SourceCoordinationError::Invalid)?;
                let digest = source_transition_digest(intent)?;
                if intent.before_bytes.is_some() || !proof.validate_record(&digest) {
                    return Err(SourceCoordinationError::Invalid);
                }
            }
            let inventory = self
                .inventories
                .get(id)
                .ok_or(SourceCoordinationError::Invalid)?;
            let mut targets = std::collections::BTreeSet::new();
            for packet in &progress.packets {
                if !targets.insert(&packet.target)
                    || !inventory
                        .targets
                        .iter()
                        .any(|target| target.decisions == packet.target)
                    || !coordination_digest(&packet.target_id)
                    || !coordination_digest(&packet.plan_digest)
                    || !valid_coordination_revision(&packet.before_protected_revision)
                    || !valid_coordination_revision(&packet.after_protected_revision)
                    || inventory
                        .targets
                        .iter()
                        .find(|target| target.decisions == packet.target)
                        .is_none_or(|target| {
                            !source_transition_digest(&target.before_protected).is_ok_and(
                                |digest| {
                                    packet.before_protected_revision
                                        == (SourceRevision::Bytes { sha256: digest })
                                },
                            )
                        })
                {
                    return Err(SourceCoordinationError::Invalid);
                }
            }
            let intent = self
                .intents
                .get(id)
                .ok_or(SourceCoordinationError::Invalid)?;
            if progress.packets_prepared && progress.packets.len() != inventory.targets.len()
                || progress.source_published
                    && (!progress.packets_prepared
                        || intent.before_bytes.is_none() && progress.source_publication.is_none())
            {
                return Err(SourceCoordinationError::Invalid);
            }
            if progress.refs_verified && !progress.source_published
                || progress.reloaded && !progress.refs_verified
                || progress.receipt.as_ref().is_some_and(|receipt| {
                    receipt.operation_id != *id
                        || !progress.reloaded
                        || receipt.source_revision
                            != source_bytes_revision(&intent.serialized_after)
                })
            {
                return Err(SourceCoordinationError::Invalid);
            }
        }
        Ok(())
    }
}

pub(crate) fn own_source_journal_path(
    context: &SourceReadContext,
) -> Result<PathBuf, SourceCoordinationError> {
    Ok(context
        .settings_path
        .parent()
        .ok_or(SourceCoordinationError::Unauthorized)?
        .join(crate::config::instance_artifacts::IDENTITY_TRANSITION_STATE_NAME))
}

pub(crate) fn read_own_source_journal(
    context: &SourceReadContext,
) -> Result<(SourceRevision, SourcePrivateJournal), SourceCoordinationError> {
    let (revision, bytes) = coordination_read(&own_source_journal_path(context)?)?;
    let journal = match bytes {
        None => SourcePrivateJournal::empty(),
        Some(bytes) => {
            serde_json::from_slice(&bytes).map_err(|_| SourceCoordinationError::Invalid)?
        }
    };
    journal.validate()?;
    let parent = coordination_parent(&context.settings_path)?;
    let canonical = parent
        .identity()
        .canonical_path
        .to_str()
        .ok_or(SourceCoordinationError::IdentityUncertain)?;
    let spelling = if cfg!(windows) {
        canonical.to_lowercase()
    } else {
        canonical.to_owned()
    };
    let own_revision = source_bytes_revision(spelling.as_bytes());
    let SourceRevision::Bytes { sha256: own_id } = own_revision else {
        unreachable!()
    };
    if journal
        .intents
        .values()
        .any(|intent| intent.editor_id != own_id)
        || journal
            .inventories
            .values()
            .any(|inventory| inventory.editor_id != own_id)
    {
        return Err(SourceCoordinationError::Unauthorized);
    }
    parent
        .verify_current()
        .map_err(|_| SourceCoordinationError::IdentityUncertain)?;
    Ok((revision, journal))
}

#[derive(Debug)]
#[allow(dead_code)]
pub(crate) struct SourcePublicationObservation {
    pub(crate) revision: SourceRevision,
}

/// The caller retains its original authority; exact after is recognized outside
/// the publisher and is never interpreted as a rejected, effect-free attempt.
#[allow(dead_code)]
pub(crate) fn publish_source_intent_with_validation(
    lease: &SourceWriteContext,
    context: &SourceReadContext,
    intent: &SourceStoredIntent,
    validate: &dyn Fn() -> Result<(), IdentityReferenceError>,
) -> Result<SourcePublicationObservation, SourceCoordinationError> {
    if intent.source != lease.source
        || intent.editor_id != lease.identity.instance_id
        || intent.context_revision != lease.context_revision
    {
        return Err(SourceCoordinationError::Unauthorized);
    }
    let after = source_bytes_revision(&intent.serialized_after);
    let guard = || {
        lease
            .identity
            .parent
            .verify_current()
            .map_err(|_| IdentityReferenceError::Conflict)?;
        lease
            .revalidate(context)
            .map_err(|_| IdentityReferenceError::Conflict)?;
        let revision = lease
            .source_revision()
            .map_err(|_| IdentityReferenceError::Conflict)?;
        if revision != intent.expected_revision && revision != after {
            return Err(IdentityReferenceError::Conflict);
        }
        validate()
    };
    guard().map_err(coordination_publication_error)?;
    let observed = lease.source_revision()?;
    let before = match &intent.before_bytes {
        None => IdentityReferencePhysicalState::Absent,
        Some(bytes) => IdentityReferencePhysicalState::from_bytes(bytes.clone()),
    };
    let (mut private_revision, mut journal) = read_own_source_journal(context)?;
    if journal.intents.get(&intent.operation_id) != Some(intent) {
        return Err(SourceCoordinationError::Invalid);
    }
    let inventory = journal
        .inventories
        .get(&intent.operation_id)
        .ok_or(SourceCoordinationError::Invalid)?;
    if inventory.source != intent.source
        || inventory.editor_id != intent.editor_id
        || inventory.context_revision != intent.context_revision
        || inventory.authorization_revision != intent.authorization_revision
        || inventory.physical_source_key != lease.physical_source_key
        || inventory.mappings != intent.mappings
    {
        return Err(SourceCoordinationError::Invalid);
    }
    let progress = journal
        .progress
        .get(&intent.operation_id)
        .ok_or(SourceCoordinationError::Invalid)?;
    if !progress.packets_prepared {
        return Err(SourceCoordinationError::Pending);
    }
    let already_published = progress.source_published;
    let initial_proof = progress.source_publication.clone();
    let intent_digest = source_transition_digest(intent)?;
    let ownership_required = intent.before_bytes.is_none();
    if ownership_required && observed == after {
        let proof = initial_proof
            .as_ref()
            .ok_or(SourceCoordinationError::Stale)?;
        if !already_published {
            crate::config::local_config_io::verify_prepared_source_target(
                &lease.parent,
                &lease.source_path,
                &intent.serialized_after,
                proof,
                &guard,
            )
            .map_err(coordination_publication_error)?;
        }
    }
    if observed == after {
        if coordination_read(&lease.source_path)?.1.as_deref()
            != Some(intent.serialized_after.as_slice())
        {
            return Err(SourceCoordinationError::Stale);
        }
        guard().map_err(coordination_publication_error)?;
        return Ok(SourcePublicationObservation { revision: after });
    }
    if already_published {
        return Err(SourceCoordinationError::Stale);
    }
    if ownership_required {
        let mut capability = match initial_proof.as_ref() {
            Some(proof) => crate::config::local_config_io::reopen_source_publication(
                &lease.parent,
                &lease.source_path,
                &intent.serialized_after,
                proof,
                &guard,
            ),
            None => crate::config::local_config_io::prepare_source_publication(
                &lease.parent,
                &lease.source_path,
                &intent.serialized_after,
                &intent_digest,
                &guard,
            ),
        }
        .map_err(coordination_publication_error)?;
        if initial_proof.is_none() {
            let proof = capability.proof().clone();
            let mut planned = journal.clone();
            planned
                .progress
                .get_mut(&intent.operation_id)
                .ok_or(SourceCoordinationError::Invalid)?
                .source_publication = Some(proof.clone());
            let checkpoint_guard = || capability.check();
            let checkpoint = update_own_source_journal_for_publication(
                lease.identity,
                lease,
                context,
                &private_revision,
                |journal| {
                    journal
                        .progress
                        .get_mut(&intent.operation_id)
                        .ok_or(SourceCoordinationError::Invalid)?
                        .source_publication = Some(proof);
                    Ok(())
                },
                &checkpoint_guard,
                Some((&intent.operation_id, &capability)),
            );
            match checkpoint {
                Ok(revision) => {
                    capability.check().map_err(coordination_publication_error)?;
                    let (confirmed_revision, confirmed) = read_own_source_journal(context)?;
                    if confirmed_revision != revision || confirmed != planned {
                        return Err(SourceCoordinationError::Stale);
                    }
                    private_revision = confirmed_revision;
                    journal = confirmed;
                }
                Err(error) => {
                    // Pure Err-before/Err-landed/other classification. Every
                    // outcome retains the stage and returns the original error;
                    // no native, unlink or second CAS follows an Err.
                    if capability.check().is_ok() {
                        if let Ok((revision, current)) = read_own_source_journal(context) {
                            let _before_exact = revision == private_revision && current == journal;
                            let _after_exact = current == planned;
                        }
                    }
                    return Err(error);
                }
            }
        }
        let checkpoint_guard = || {
            guard()?;
            let (revision, current) =
                read_own_source_journal(context).map_err(|_| IdentityReferenceError::Conflict)?;
            if revision != private_revision || current != journal {
                return Err(IdentityReferenceError::Conflict);
            }
            Ok(())
        };
        #[cfg(test)]
        count_source_transition_publication(2);
        capability
            .commit(&checkpoint_guard)
            .map_err(coordination_publication_error)?;
        return Ok(SourcePublicationObservation { revision: after });
    }
    #[cfg(test)]
    count_source_transition_publication(2);
    let publication =
        crate::config::local_config_io::publish_source_coordination_bytes_with_validation(
            &lease.source_path,
            &before,
            &intent.serialized_after,
            &lease.parent,
            &guard,
        );
    publication.result.map_err(coordination_publication_error)?;
    // Validate authority again, then classify durable bytes even if the final
    // publication check failed. No callback advances local progress or events.
    guard().map_err(coordination_publication_error)?;
    match lease.source_revision()? {
        revision if revision == after => Ok(SourcePublicationObservation { revision }),
        revision if revision == intent.expected_revision => Err(SourceCoordinationError::Stale),
        _ => Err(SourceCoordinationError::Stale),
    }
}

/// Pure record changes are redecoded before guarded CAS publication; callbacks
/// borrow original proofs and never advance coordinator progress themselves.
#[allow(dead_code)]
pub(crate) fn update_own_source_journal_with_validation<F>(
    identity: &IdentityWriteLease,
    lease: &SourceWriteContext,
    context: &SourceReadContext,
    expected: &SourceRevision,
    change: F,
    validate: &dyn Fn() -> Result<(), IdentityReferenceError>,
) -> Result<SourceRevision, SourceCoordinationError>
where
    F: FnOnce(&mut SourcePrivateJournal) -> Result<(), SourceCoordinationError>,
{
    update_own_source_journal_for_publication(
        identity, lease, context, expected, change, validate, None,
    )
}

pub(crate) fn update_own_source_journal_for_publication<F>(
    identity: &IdentityWriteLease,
    lease: &SourceWriteContext,
    context: &SourceReadContext,
    expected: &SourceRevision,
    change: F,
    validate: &dyn Fn() -> Result<(), IdentityReferenceError>,
    prepared: Option<(
        &str,
        &crate::config::local_config_io::PreparedSourceCapability<'_>,
    )>,
) -> Result<SourceRevision, SourceCoordinationError>
where
    F: FnOnce(&mut SourcePrivateJournal) -> Result<(), SourceCoordinationError>,
{
    if !std::ptr::eq(identity, lease.identity) {
        return Err(SourceCoordinationError::Unauthorized);
    }
    let parent = coordination_parent(&context.settings_path)?;
    if !crate::path_identity::same_object(parent.identity(), identity.parent.identity()) {
        return Err(SourceCoordinationError::Unauthorized);
    }
    let guard = || {
        identity
            .parent
            .verify_current()
            .map_err(|_| IdentityReferenceError::Conflict)?;
        lease
            .revalidate(context)
            .map_err(|_| IdentityReferenceError::Conflict)?;
        validate()
    };
    guard().map_err(coordination_publication_error)?;
    let path = own_source_journal_path(context)?;
    let before = read_identity_reference_physical(&path).map_err(coordination_publication_error)?;
    let (revision, mut journal) = read_own_source_journal(context)?;
    if &revision != expected
        || before
            != read_identity_reference_physical(&path).map_err(coordination_publication_error)?
    {
        return Err(SourceCoordinationError::Stale);
    }
    let original = journal.clone();
    change(&mut journal)?;
    journal.validate()?;
    if original
        .intents
        .iter()
        .any(|(id, intent)| journal.intents.get(id) != Some(intent))
        || original
            .inventories
            .iter()
            .any(|(id, inventory)| journal.inventories.get(id) != Some(inventory))
        || original
            .progress
            .keys()
            .any(|id| !journal.progress.contains_key(id))
    {
        return Err(SourceCoordinationError::Invalid);
    }
    for (id, previous) in &original.progress {
        let next = journal
            .progress
            .get(id)
            .ok_or(SourceCoordinationError::Invalid)?;
        if !next.packets.starts_with(&previous.packets)
            || previous
                .source_publication
                .as_ref()
                .is_some_and(|proof| next.source_publication.as_ref() != Some(proof))
            || previous.packets_prepared && !next.packets_prepared
            || previous.source_published && !next.source_published
            || previous.refs_verified && !next.refs_verified
            || previous.reloaded && !next.reloaded
            || previous
                .receipt
                .as_ref()
                .is_some_and(|receipt| next.receipt.as_ref() != Some(receipt))
        {
            return Err(SourceCoordinationError::Invalid);
        }
    }
    for (id, next) in &journal.progress {
        if original
            .progress
            .get(id)
            .and_then(|p| p.source_publication.as_ref())
            .is_none()
            && next.source_publication.is_some()
        {
            let (prepared_id, capability) = prepared.ok_or(SourceCoordinationError::Invalid)?;
            let intent = journal
                .intents
                .get(id)
                .ok_or(SourceCoordinationError::Invalid)?;
            if prepared_id != id
                || intent.source != lease.source
                || next.source_publication.as_ref() != Some(capability.proof())
                || !next.packets_prepared
                || intent.editor_id != identity.instance_id
                || intent.expected_revision != (SourceRevision::Absent {})
                || lease.source_revision()? != (SourceRevision::Absent {})
            {
                return Err(SourceCoordinationError::Invalid);
            }
            capability.check().map_err(coordination_publication_error)?;
        }
    }
    for (id, next) in &journal.progress {
        let previous = original.progress.get(id);
        let publishing = next.source_published && previous.is_none_or(|p| !p.source_published);
        let completing = next.receipt.is_some() && previous.is_none_or(|p| p.receipt.is_none());
        if publishing || completing {
            let intent = journal
                .intents
                .get(id)
                .ok_or(SourceCoordinationError::Invalid)?;
            let inventory = journal
                .inventories
                .get(id)
                .ok_or(SourceCoordinationError::Invalid)?;
            if intent.source != lease.source
                || intent.editor_id != identity.instance_id
                || inventory.physical_source_key != lease.physical_source_key
                || coordination_read(&lease.source_path)?.1.as_deref()
                    != Some(intent.serialized_after.as_slice())
            {
                return Err(SourceCoordinationError::Stale);
            }
            if publishing && intent.before_bytes.is_none() {
                let proof = next
                    .source_publication
                    .as_ref()
                    .ok_or(SourceCoordinationError::Invalid)?;
                crate::config::local_config_io::verify_prepared_source_target(
                    &lease.parent,
                    &lease.source_path,
                    &intent.serialized_after,
                    proof,
                    &guard,
                )
                .map_err(coordination_publication_error)?;
            }
        }
    }
    for (id, intent) in &journal.intents {
        if original.intents.contains_key(id) {
            continue;
        }
        if intent.editor_id != identity.instance_id
            || intent.source != lease.source
            || intent.context_revision != lease.context_revision
            || lease.source_revision()? != intent.expected_revision
        {
            return Err(SourceCoordinationError::Stale);
        }
    }
    for (id, inventory) in &journal.inventories {
        if original.inventories.get(id) == Some(inventory) {
            continue;
        }
        if inventory.editor_id != identity.instance_id
            || inventory.source != lease.source
            || inventory.physical_source_key != lease.physical_source_key
            || inventory.context_revision != lease.context_revision
        {
            return Err(SourceCoordinationError::Unauthorized);
        }
        let intent = journal
            .intents
            .get(id)
            .ok_or(SourceCoordinationError::Invalid)?;
        let evidence = inventory
            .original_evidence
            .as_ref()
            .ok_or(SourceCoordinationError::Invalid)?;
        if intent.editor_id != identity.instance_id
            || intent.source != inventory.source
            || intent.mappings != inventory.mappings
            || intent.authorization_revision != inventory.authorization_revision
            || evidence.expected_source != intent.expected_revision
            || evidence.after_source != source_bytes_revision(&intent.serialized_after)
            || evidence.inventory_digest
                != source_inventory_digest(
                    &inventory.editor_id,
                    &inventory.source,
                    &inventory.physical_source_key,
                    &inventory.context_revision,
                    &evidence.topology,
                )?
            || lease.source_revision()? != evidence.expected_source
        {
            return Err(SourceCoordinationError::Unauthorized);
        }
    }
    if journal == original {
        guard().map_err(coordination_publication_error)?;
        return Ok(revision);
    }
    let mut bytes =
        serde_json::to_vec_pretty(&journal).map_err(|_| SourceCoordinationError::Invalid)?;
    bytes.push(b'\n');
    let decoded: SourcePrivateJournal =
        serde_json::from_slice(&bytes).map_err(|_| SourceCoordinationError::Invalid)?;
    decoded.validate()?;
    if decoded != journal {
        return Err(SourceCoordinationError::Invalid);
    }
    #[cfg(test)]
    count_source_transition_publication(1);
    crate::config::local_config_io::publish_source_coordination_bytes_with_validation(
        &path,
        &before,
        &bytes,
        &identity.parent,
        &guard,
    )
    .result
    .map_err(coordination_publication_error)?;
    Ok(source_bytes_revision(&bytes))
}

/// Lookup is restricted to the own C path derived from backend context. Status
/// can expose the original ID and CAS before a shared fence exists.
#[allow(dead_code)]
pub(crate) fn read_own_source_intent(
    context: &SourceReadContext,
    operation_id: &str,
) -> Result<(SourceRevision, Option<SourceStoredIntent>), SourceCoordinationError> {
    let (revision, journal) = read_own_source_journal(context)?;
    Ok((revision, journal.intents.get(operation_id).cloned()))
}

#[allow(dead_code)]
pub(crate) fn source_complete_proof(
    lease: &SourceWriteContext,
    context: &SourceReadContext,
    operation_id: &str,
    target_id: &str,
    validate: &dyn Fn() -> Result<(), IdentityReferenceError>,
) -> Result<IdentityReferenceCompleteProof, SourceCoordinationError> {
    validate().map_err(coordination_publication_error)?;
    lease.revalidate(context)?;
    let (_, journal) = read_own_source_journal(context)?;
    let intent = journal
        .intents
        .get(operation_id)
        .ok_or(SourceCoordinationError::Invalid)?;
    let inventory = journal
        .inventories
        .get(operation_id)
        .ok_or(SourceCoordinationError::Invalid)?;
    let progress = journal
        .progress
        .get(operation_id)
        .ok_or(SourceCoordinationError::Invalid)?;
    if intent.editor_id != lease.identity.instance_id
        || intent.source != lease.source
        || inventory.physical_source_key != lease.physical_source_key
        || coordination_read(&lease.source_path)?.1.as_deref()
            != Some(intent.serialized_after.as_slice())
    {
        return Err(SourceCoordinationError::Stale);
    }
    let projection = serde_json::json!({
        "schemaVersion":2, "operationId":operation_id, "phase":progress.phase(),
        "physicalSourceKey":inventory.physical_source_key,
        "afterSourceRevision":source_bytes_revision(&intent.serialized_after),
        "packetsPrepared":progress.packets_prepared, "sourcePublished":progress.source_published,
        "refsVerified":progress.refs_verified, "reloaded":progress.reloaded,
        "packets":progress.packets, "receipt":progress.receipt
    });
    let source = serde_json::to_value(lease.source_revision()?)
        .map_err(|_| SourceCoordinationError::Invalid)?;
    IdentityReferenceCompleteProof::from_single_editor_complete(
        &projection,
        operation_id,
        target_id,
        &source,
    )
    .map_err(coordination_publication_error)
}

// #2893 P02: internal readers and pure preparations. No caller publishes these
// plans yet; the existing whole-settings save remains the production boundary.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[allow(dead_code)] // Dormant until the later source protocol cuts.
pub(crate) enum SourcePreparationError {
    #[error("contextUnavailable")]
    ContextUnavailable,
    #[error("contextChanged")]
    ContextChanged,
    #[error("originUnavailable")]
    OriginUnavailable,
    #[error("legacyOriginUnresolved")]
    LegacyOriginUnresolved,
    #[error("staleRevision")]
    StaleRevision,
    #[error("sourceReadOnly")]
    ReadOnly,
    #[error("independentFormatRequired")]
    IndependentFormatRequired,
    #[error("invalidSourceCandidate")]
    InvalidCandidate,
    #[error("protectedSettingsField")]
    ProtectedSettingsField,
    #[error("{0}")]
    Identity(IdentityValidationError),
}

#[allow(dead_code)] // Dormant until the later source protocol cuts.
pub(crate) fn source_transition_digest_bytes(bytes: &[u8]) -> SourceRevision {
    source_bytes_revision(bytes)
}

pub(crate) fn source_bytes_revision(bytes: &[u8]) -> SourceRevision {
    use sha2::{Digest, Sha256};
    SourceRevision::Bytes {
        sha256: format!("{:x}", Sha256::digest(bytes)),
    }
}

/// Unknown evidence is not physical absence and is never a CAS precondition.
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)] // Dormant until the later source protocol cuts.
pub(crate) enum OriginPhysicalInput {
    Absent,
    Bytes(Vec<u8>),
    Unknown,
}

#[allow(dead_code)] // Dormant until the later source protocol cuts.
impl OriginPhysicalInput {
    pub(crate) fn read(path: &Path) -> Self {
        match std::fs::read(path) {
            Ok(bytes) => Self::Bytes(bytes),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Self::Absent,
            Err(_) => Self::Unknown,
        }
    }

    pub(crate) fn revision(&self) -> Option<SourceRevision> {
        match self {
            Self::Absent => Some(SourceRevision::Absent {}),
            Self::Bytes(bytes) => Some(source_bytes_revision(bytes)),
            Self::Unknown => None,
        }
    }

    pub(crate) fn value(&self) -> Option<Value> {
        match self {
            Self::Bytes(bytes) => serde_json::from_slice(bytes).ok(),
            Self::Absent | Self::Unknown => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[allow(dead_code)] // Dormant until the later source protocol cuts.
pub(crate) struct SourceReadContext {
    pub(crate) settings_path: PathBuf,
    pub(crate) catalog_root: PathBuf,
    pub(crate) project: bool,
    pub(crate) revision: String,
}

#[allow(dead_code)] // Dormant until the later source protocol cuts.
impl SourceReadContext {
    /// Backend-only context: P comes from the current registry, never the
    /// assignment target Q or a deserialized SourceRef.
    pub(crate) fn from_settings(
        settings: &AppSettings,
        settings_path: &Path,
    ) -> Result<Self, SourcePreparationError> {
        let primary = crate::config::coding_agents_catalog::primary_project_root(settings);
        Self::from_primary(settings_path, primary)
    }

    pub(crate) fn from_primary(
        settings_path: &Path,
        primary: Option<PathBuf>,
    ) -> Result<Self, SourcePreparationError> {
        let config = settings_path
            .parent()
            .ok_or(SourcePreparationError::ContextUnavailable)?;
        let config = std::fs::canonicalize(config)
            .map_err(|_| SourcePreparationError::ContextUnavailable)?;
        let catalog_root = match &primary {
            Some(root) => std::fs::canonicalize(root)
                .map_err(|_| SourcePreparationError::ContextUnavailable)?,
            None => config.clone(),
        };
        let bytes = serde_json::to_vec(&(
            config.to_string_lossy(),
            catalog_root.to_string_lossy(),
            primary.is_some(),
        ))
        .map_err(|_| SourcePreparationError::ContextUnavailable)?;
        let SourceRevision::Bytes { sha256 } = source_bytes_revision(&bytes) else {
            unreachable!()
        };
        Ok(Self {
            settings_path: config.join(SETTINGS_FILE_NAME),
            catalog_root,
            project: primary.is_some(),
            revision: sha256,
        })
    }

    pub(crate) fn source(&self, kind: SourceKind) -> SourceRef {
        let registered = kind == SourceKind::RegisteredInstance;
        SourceRef {
            family: SourceFamily::Agents,
            role: if registered {
                SourceRole::Registered
            } else {
                SourceRole::Catalog
            },
            scope: if self.project && !registered {
                SourceScope::Project
            } else {
                SourceScope::Instance
            },
            kind,
            context_root: if registered {
                self.settings_path
                    .parent()
                    .expect("constructed with parent")
                    .to_string_lossy()
                    .into_owned()
            } else {
                self.catalog_root.to_string_lossy().into_owned()
            },
        }
    }

    pub(crate) fn locator(&self, source: &SourceRef) -> Result<PathBuf, SourcePreparationError> {
        if source.scope == SourceScope::Instance
            && source.kind != SourceKind::RegisteredInstance
            && self.project
        {
            let mut instance = self.clone();
            instance.project = false;
            instance.catalog_root = self
                .settings_path
                .parent()
                .ok_or(SourcePreparationError::ContextUnavailable)?
                .to_path_buf();
            return instance.locator(source);
        }
        if source != &self.source(source.kind)
            || (!self.project && source.kind == SourceKind::CatalogProject)
        {
            return Err(SourcePreparationError::ContextUnavailable);
        }
        if source.kind == SourceKind::RegisteredInstance {
            return Ok(agents_instance_path(&self.settings_path));
        }
        let ac = if self.project {
            self.catalog_root.join(".ac")
        } else {
            self.catalog_root.clone()
        };
        crate::config::coding_agents_catalog::independent_catalog_path(&ac, source.kind)
            .ok_or(SourcePreparationError::ContextUnavailable)
    }
}

#[allow(dead_code)] // Dormant until the later source protocol cuts.
pub(crate) fn list_active_sources(context: &SourceReadContext) -> Vec<SourceRef> {
    let mut contexts = vec![context.clone()];
    if context.project {
        let mut instance = context.clone();
        instance.project = false;
        instance.catalog_root = context
            .settings_path
            .parent()
            .expect("settings parent")
            .to_path_buf();
        contexts.push(instance);
    }
    let mut paths = HashSet::new();
    let mut sources = Vec::new();
    for owner in contexts {
        for kind in [
            SourceKind::RegisteredInstance,
            SourceKind::CatalogBase,
            SourceKind::CatalogProject,
            SourceKind::CatalogPersonal,
        ] {
            if !owner.project && kind == SourceKind::CatalogProject {
                continue;
            }
            let source = owner.source(kind);
            let Ok(path) = owner.locator(&source) else {
                continue;
            };
            // Missing files are omitted; nonregular/unreadable existing sources
            // remain visible for a diagnostic snapshot. Never create on read.
            if std::fs::symlink_metadata(&path)
                .is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound)
            {
                continue;
            }
            if paths.insert(path) {
                sources.push(source);
            }
        }
    }
    sources
}

#[derive(Debug, Clone)]
#[allow(dead_code)] // Dormant until the later source protocol cuts.
pub(crate) struct RawSourceSnapshot {
    pub(crate) snapshot: SourceSnapshot<Value>,
    pub(crate) candidate_template: Option<Value>,
    pub(crate) raw: OriginPhysicalInput,
    pub(crate) context: SourceReadContext,
}

#[allow(dead_code)] // Dormant until the later source protocol cuts.
pub(crate) fn source_diagnostic(code: &str) -> SourceDiagnostic {
    SourceDiagnostic {
        code: code.into(),
        entry_hint: None,
        existing_name: None,
        requested_profile: None,
    }
}

#[allow(dead_code)] // Dormant until the later source protocol cuts.
pub(crate) fn decode_registered_raw(
    value: &Value,
) -> Result<(Vec<AgentConfig>, CodingAgentProfilesConfig), SourcePreparationError> {
    let map = value
        .as_object()
        .ok_or(SourcePreparationError::InvalidCandidate)?;
    let agents: Vec<AgentConfig> = map
        .get("agents")
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .map_err(|_| SourcePreparationError::InvalidCandidate)?
        .unwrap_or_default();
    let projected = project_physical_profiles(value, true)
        .map_err(|_| SourcePreparationError::InvalidCandidate)?;
    let profiles: CodingAgentProfilesConfig = projected
        .get("codingAgentProfiles")
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .map_err(|_| SourcePreparationError::InvalidCandidate)?
        .unwrap_or_default();
    let mut keys = HashSet::new();
    if agents.iter().any(|agent| {
        agent.id.is_empty() || agent.command.trim().is_empty() || !keys.insert(&agent.id)
    }) {
        return Err(SourcePreparationError::InvalidCandidate);
    }
    if profiles
        .profiles_by_agent
        .values()
        .any(|cells| cells.keys().any(|letter| !is_valid_profile_letter(letter)))
    {
        return Err(SourcePreparationError::InvalidCandidate);
    }
    let mut agents = agents;
    let mut profiles = profiles;
    refresh_agent_id_hashes(&mut agents);
    refresh_pair_id_hashes(&agents, &mut profiles);
    Ok((agents, profiles))
}

#[allow(dead_code)] // Dormant until the later source protocol cuts.
pub(crate) fn read_source_snapshot(
    context: &SourceReadContext,
    source: &SourceRef,
) -> Result<RawSourceSnapshot, SourcePreparationError> {
    let path = context.locator(source)?;
    let raw = OriginPhysicalInput::read(&path);
    source_snapshot_from_original(context, source, raw)
}

pub(crate) fn source_snapshot_from_original(
    context: &SourceReadContext,
    source: &SourceRef,
    raw: OriginPhysicalInput,
) -> Result<RawSourceSnapshot, SourcePreparationError> {
    context.locator(source)?;
    let revision = raw.revision().unwrap_or(SourceRevision::Absent {});
    let mut snapshot = SourceSnapshot {
        source: source.clone(),
        revision,
        availability: SourceAvailability::Absent,
        capabilities: SourceCapabilities::default(),
        agents: Vec::new(),
        profiles: BTreeMap::new(),
        diagnostics: Vec::new(),
        context_revision: context.revision.clone(),
    };
    let candidate_template = None;
    match &raw {
        OriginPhysicalInput::Absent => {}
        OriginPhysicalInput::Bytes(bytes) => {
            let path = context.locator(source)?;
            let ambiguity = duplicate_profile_document(bytes, &path);
            let registered_gate = if source.kind == SourceKind::RegisteredInstance {
                registered_profile_raw_gate(&context.settings_path).err()
            } else {
                None
            };
            let catalog_gate = if source.kind != SourceKind::RegisteredInstance {
                crate::config::coding_agents_catalog::parse_strict_json(bytes)
                    .and_then(|value| decode_physical_catalog(&value))
                    .err()
                    .map(|reason| format!("{}: {reason}", path.display()))
            } else {
                None
            };
            let invalid = ambiguity.or(registered_gate).or(catalog_gate);
            if let Some(reason) = invalid {
                snapshot.availability = SourceAvailability::Invalid;
                let code = [
                    "duplicateJsonMember",
                    "invalidLegacyProfileOrphans",
                    "sourceProfileConflict",
                ]
                .into_iter()
                .find(|code| reason.contains(code))
                .unwrap_or("invalidSource");
                let mut diagnostic = source_diagnostic(code);
                diagnostic.entry_hint = Some(reason);
                snapshot.diagnostics.push(diagnostic);
                return Ok(RawSourceSnapshot {
                    snapshot,
                    candidate_template: None,
                    raw,
                    context: context.clone(),
                });
            }
            let decoded = if source.kind == SourceKind::RegisteredInstance {
                raw.value()
                    .ok_or(SourcePreparationError::InvalidCandidate)
                    .and_then(|value| {
                        let (agents, profiles) = decode_registered_raw(&value)?;
                        snapshot.profiles = profiles.profiles_by_agent;
                        let mut view = value.clone();
                        refresh_physical_source_rows(&mut view, true)
                            .map_err(|_| SourcePreparationError::InvalidCandidate)?;
                        snapshot.agents = view
                            .get("agents")
                            .and_then(Value::as_array)
                            .cloned()
                            .unwrap_or_default();
                        let entries: Vec<_> = agents
                            .iter()
                            .map(|agent| IdentitySnapshotEntry {
                                key: &agent.id,
                                name: &agent.label,
                                command: &agent.command,
                                envs: &agent.envs,
                                profiles: snapshot.profiles.get(&agent.id),
                            })
                            .collect();
                        if let Err(error) = validate_identity_snapshot(&entries) {
                            snapshot.diagnostics.push(SourceDiagnostic {
                                code: format!("{:?}", error.code),
                                entry_hint: Some(error.entry_hint),
                                existing_name: Some(error.existing_name),
                                requested_profile: error.requested_profile,
                            });
                        }
                        Ok(())
                    })
            } else {
                crate::config::coding_agents_catalog::parse_strict_json(bytes)
                    .and_then(|value| {
                        let (agents, profiles) = decode_physical_catalog(&value)?;
                        snapshot.profiles = profiles.profiles_by_agent;
                        let mut view = value.clone();
                        refresh_physical_source_rows(&mut view, false)?;
                        snapshot.agents = view
                            .get("agents")
                            .and_then(Value::as_array)
                            .into_iter()
                            .flatten()
                            .filter(|row| row.get("remove") != Some(&Value::Bool(true)))
                            .cloned()
                            .collect();
                        let active: HashSet<_> =
                            agents.iter().map(|agent| agent.id.as_str()).collect();
                        for row in value
                            .get("agents")
                            .and_then(Value::as_array)
                            .into_iter()
                            .flatten()
                        {
                            if let Some(key) = row
                                .get("key")
                                .and_then(Value::as_str)
                                .filter(|key| !active.contains(key))
                            {
                                let mut diagnostic = source_diagnostic("incompletePhysicalRow");
                                diagnostic.entry_hint = Some(key.to_owned());
                                snapshot.diagnostics.push(diagnostic);
                            }
                        }
                        Ok(())
                    })
                    .map_err(|_| SourcePreparationError::InvalidCandidate)
            };
            match decoded {
                Ok(()) => {
                    snapshot.availability = SourceAvailability::Available;
                    snapshot.capabilities.available_read = true;
                    snapshot.capabilities.editable_save = matches!(
                        source.kind,
                        SourceKind::RegisteredInstance | SourceKind::CatalogPersonal
                    );
                    snapshot.capabilities.editable_save &= source.scope == SourceScope::Project
                        || source.kind == SourceKind::RegisteredInstance;
                    snapshot.capabilities.editable_save &= source.kind
                        == SourceKind::RegisteredInstance
                        || raw
                            .value()
                            .is_some_and(|value| value.get("managed").is_none());
                    snapshot.capabilities.repair_by_owner = false;
                }
                Err(SourcePreparationError::IndependentFormatRequired) => {
                    snapshot.availability = SourceAvailability::IndependentFormatRequired;
                    snapshot
                        .diagnostics
                        .push(source_diagnostic("independentFormatRequired"));
                }
                Err(_) => {
                    snapshot.availability = SourceAvailability::Invalid;
                    snapshot.agents.clear();
                    snapshot.profiles.clear();
                    snapshot
                        .diagnostics
                        .push(source_diagnostic("invalidSource"));
                }
            }
        }
        OriginPhysicalInput::Unknown => {
            snapshot.availability = SourceAvailability::Invalid;
            snapshot
                .diagnostics
                .push(source_diagnostic("unreadableSource"));
        }
    }
    Ok(RawSourceSnapshot {
        snapshot,
        candidate_template,
        raw,
        context: context.clone(),
    })
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
#[allow(dead_code)] // Dormant until the later source protocol cuts.
pub(crate) enum LegacyOriginUnit {
    Entry(String),
    Profile(String, String),
    Default(String),
    Presentation(String, String),
    Slot(String),
}

#[allow(dead_code)] // Dormant until the later source protocol cuts.
impl LegacyOriginUnit {
    pub(crate) fn paths(&self) -> Vec<Vec<String>> {
        let p = |items: &[&str]| items.iter().map(|item| (*item).to_owned()).collect();
        match self {
            Self::Entry(_) => vec![p(&["agents"])],
            Self::Profile(key, letter) => vec![
                p(&["agents"]),
                p(&["codingAgentProfiles", "profilesByAgent", key, letter]),
                p(&["codingAgentProfiles", "identityByAgent", key]),
            ],
            Self::Default(matrix) => {
                vec![p(&["codingAgentProfiles", "defaultProfileByAgent", matrix])]
            }
            Self::Presentation(key, letter) => vec![p(&[
                "codingAgentProfiles",
                "profileLabelsByAgent",
                key,
                letter,
            ])],
            Self::Slot(letter) => vec![p(&["codingAgentProfiles", "profileSlots", letter])],
        }
    }
}

#[allow(dead_code)] // Dormant until the later source protocol cuts.
pub(crate) fn origin_paths_overlap(a: &[String], b: &[String]) -> bool {
    a.starts_with(b) || b.starts_with(a)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[allow(dead_code)] // Dormant until the later source protocol cuts.
pub(crate) struct OriginBinding {
    pub(crate) registered: Option<SourceRevision>,
    pub(crate) settings: Option<SourceRevision>,
    pub(crate) overlay: Option<SourceRevision>,
    pub(crate) context: SourceReadContext,
}

#[derive(Debug, Clone)]
#[allow(dead_code)] // Dormant until the later source protocol cuts.
pub(crate) struct OriginInputs {
    pub(crate) registered: OriginPhysicalInput,
    pub(crate) settings: OriginPhysicalInput,
    pub(crate) overlay: OriginPhysicalInput,
    pub(crate) binding: OriginBinding,
}

#[allow(dead_code)] // Dormant until the later source protocol cuts.
impl OriginInputs {
    pub(crate) fn read(context: &SourceReadContext) -> Self {
        let registered = OriginPhysicalInput::read(&agents_instance_path(&context.settings_path));
        let settings = OriginPhysicalInput::read(&context.settings_path);
        let overlay = OriginPhysicalInput::read(
            &context
                .settings_path
                .with_file_name(SETTINGS_LOCAL_OVERRIDE_FILE_NAME),
        );
        Self {
            binding: OriginBinding {
                registered: registered.revision(),
                settings: settings.revision(),
                overlay: overlay.revision(),
                context: context.clone(),
            },
            registered,
            settings,
            overlay,
        }
    }

    /// At most two retries of read-only preparation; callers must supply a fresh
    /// backend context on revalidation, not rely on the registered30 revision.
    pub(crate) fn capture(context: &SourceReadContext) -> Result<Self, SourcePreparationError> {
        for _ in 0..=2 {
            let first = Self::read(context);
            let second = Self::read(context);
            if first.binding == second.binding {
                return Ok(first);
            }
        }
        Err(SourcePreparationError::StaleRevision)
    }

    pub(crate) fn revalidate(
        &self,
        context: &SourceReadContext,
    ) -> Result<(), SourcePreparationError> {
        if context != &self.binding.context {
            return Err(SourcePreparationError::ContextChanged);
        }
        let fresh = Self::read(context);
        if fresh.binding != self.binding {
            return Err(SourcePreparationError::StaleRevision);
        }
        if [
            self.binding.registered.as_ref(),
            self.binding.settings.as_ref(),
            self.binding.overlay.as_ref(),
        ]
        .iter()
        .any(|revision| revision.is_none())
        {
            return Err(SourcePreparationError::OriginUnavailable);
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
#[allow(dead_code)] // Dormant until the later source protocol cuts.
pub(crate) struct LegacyCompatibilityOrigin {
    pub(crate) applied_paths: Vec<Vec<String>>,
    pub(crate) unknown_paths: Vec<Vec<String>>,
    pub(crate) represented: std::collections::BTreeSet<LegacyOriginUnit>,
    pub(crate) binding: OriginBinding,
    pub(crate) base: Value,
    pub(crate) effective: Value,
    pub(crate) overlay_diagnostics: Vec<crate::config::local_overlay::OverlayDiagnostic>,
}

/// Evidence issued by the backend that owns the original selection. No serde
/// implementation: source save payloads cannot claim an authorization or link.
#[derive(Debug, Clone)]
#[allow(dead_code)] // Dormant until the later source protocol cuts.
pub(crate) struct LegacyOriginLinkProof {
    pub(crate) binding: OriginBinding,
    pub(crate) units: std::collections::BTreeSet<LegacyOriginUnit>,
}

#[allow(dead_code)] // Dormant until the later source protocol cuts.
impl LegacyCompatibilityOrigin {
    pub(crate) fn classify(inputs: &OriginInputs) -> Self {
        let mut unknown_paths = Vec::new();
        let mut base = match inputs.settings.value() {
            Some(value @ Value::Object(_)) => value,
            _ if inputs.settings == OriginPhysicalInput::Absent => {
                serde_json::to_value(AppSettings::default()).unwrap_or(Value::Null)
            }
            _ => {
                unknown_paths.push(Vec::new());
                Value::Null
            }
        };
        let settings_base = base.clone();
        let raw30 = inputs.registered.value();
        if let Some(raw) = &raw30 {
            if decode_registered_raw(raw).is_err() {
                unknown_paths.push(vec!["agents".into()]);
                unknown_paths.push(vec!["codingAgentProfiles".into()]);
            } else if let (Some(dst), Some(src)) = (base.as_object_mut(), raw.as_object()) {
                dst.extend(agents_instance_groups(src));
            }
        } else if inputs.registered != OriginPhysicalInput::Absent {
            unknown_paths.push(vec!["agents".into()]);
            unknown_paths.push(vec!["codingAgentProfiles".into()]);
        }
        let before = base.clone();
        let mut effective = base.clone();
        let mut overlay = match &inputs.overlay {
            OriginPhysicalInput::Absent => LocalSettingsOverlay::default(),
            OriginPhysicalInput::Bytes(bytes) => LocalSettingsOverlay::from_overlay_bytes_paired(
                &mut effective,
                bytes,
                OVERLAY_INELIGIBLE_DISK_KEYS,
                OVERLAY_INELIGIBLE_LEGACY_KEYS,
                OVERLAY_DERIVED_ID_CLOSURES,
                CONFIGURATION_COMMAND_PAIRS,
            ),
            OriginPhysicalInput::Unknown => {
                unknown_paths.push(Vec::new());
                LocalSettingsOverlay::default()
            }
        };
        // Same project decoder and AppSettings decoder as the ordinary loader,
        // before its order finalization/repair/adoption and without publication.
        let decode = |raw: &Value| {
            let mut probe = raw.clone();
            if let Some(root) = probe.as_object_mut() {
                root.entry("agents")
                    .or_insert_with(|| Value::Array(Vec::new()));
            }
            let instance_base = production_instance_base();
            apply_project_decode_to_value(
                &mut probe,
                instance_base.as_deref(),
                &projects::FsCandidateResolver,
            );
            serde_json::from_value::<AppSettings>(probe)
        };
        if let Err(error) = decode(&effective) {
            if !overlay.is_empty() && decode(&before).is_ok() {
                overlay = overlay.into_undecodable(error.to_string());
                effective = before;
            } else {
                unknown_paths.push(Vec::new());
            }
        }
        let mut represented = std::collections::BTreeSet::new();
        for value in inputs
            .settings
            .value()
            .iter()
            .chain(raw30.iter())
            .chain(std::iter::once(&effective))
        {
            collect_origin_units(value, &mut represented, &mut unknown_paths);
        }
        Self {
            applied_paths: overlay.owned_paths().to_vec(),
            unknown_paths,
            represented,
            binding: inputs.binding.clone(),
            base: settings_base,
            effective,
            overlay_diagnostics: overlay.diagnostics("settings overlay"),
        }
    }

    pub(crate) fn require_disjoint(
        &self,
        units: impl IntoIterator<Item = LegacyOriginUnit>,
    ) -> Result<(), SourcePreparationError> {
        for unit in units {
            for path in unit.paths() {
                if self
                    .unknown_paths
                    .iter()
                    .any(|unknown| origin_paths_overlap(unknown, &path))
                {
                    return Err(SourcePreparationError::OriginUnavailable);
                }
                if self
                    .applied_paths
                    .iter()
                    .any(|owned| origin_paths_overlap(owned, &path))
                {
                    return Err(SourcePreparationError::LegacyOriginUnresolved);
                }
            }
        }
        Ok(())
    }

    /// Only original source-aware selection evidence binds a default N to K/L.
    /// The future inventory owner supplies authorization and full target CAS;
    /// this pure seam verifies that evidence against the captured source and
    /// complete origin binding, without scanning or mutating any matrix.
    #[allow(clippy::too_many_arguments)] // Complete original source and target CAS evidence.
    pub(crate) fn bind_original_selection(
        &self,
        snapshot: &RawSourceSnapshot,
        selection: &ConfigurationRef,
        original_source_revision: &SourceRevision,
        original_context_revision: &str,
        original_target: &[u8],
        current_target: &[u8],
        matrix: Option<&str>,
    ) -> Result<LegacyOriginLinkProof, SourcePreparationError> {
        let slot = if matrix.is_some() {
            SourceOriginalSlot::MatrixDefault
        } else {
            SourceOriginalSlot::Decision
        };
        self.bind_original_selection_at(
            snapshot,
            selection,
            original_source_revision,
            original_context_revision,
            original_target,
            current_target,
            matrix,
            &slot,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn bind_original_selection_at(
        &self,
        snapshot: &RawSourceSnapshot,
        selection: &ConfigurationRef,
        original_source_revision: &SourceRevision,
        original_context_revision: &str,
        original_target: &[u8],
        current_target: &[u8],
        matrix: Option<&str>,
        slot: &SourceOriginalSlot,
    ) -> Result<LegacyOriginLinkProof, SourcePreparationError> {
        if matrix.is_some() != matches!(slot, SourceOriginalSlot::MatrixDefault) {
            return Err(SourcePreparationError::LegacyOriginUnresolved);
        }
        if original_target != current_target
            || selection.version != 1
            || selection.source != snapshot.snapshot.source
            || original_source_revision != &snapshot.snapshot.revision
            || original_context_revision != snapshot.snapshot.context_revision
            || self.binding.context != snapshot.context
            || self.binding.registered.as_ref() != Some(original_source_revision)
        {
            return Err(SourcePreparationError::LegacyOriginUnresolved);
        }
        let target: Value = serde_json::from_slice(current_target)
            .map_err(|_| SourcePreparationError::OriginUnavailable)?;
        let stored = slot.stored(&target);
        let stored: ConfigurationRef = serde_json::from_value(
            stored
                .cloned()
                .ok_or(SourcePreparationError::LegacyOriginUnresolved)?,
        )
        .map_err(|_| SourcePreparationError::LegacyOriginUnresolved)?;
        if &stored != selection {
            return Err(SourcePreparationError::LegacyOriginUnresolved);
        }
        let key = selection
            .legacy_entry_hint
            .as_deref()
            .ok_or(SourcePreparationError::LegacyOriginUnresolved)?;
        let raw = snapshot
            .raw
            .value()
            .ok_or(SourcePreparationError::OriginUnavailable)?;
        let (agents, profiles) = decode_registered_raw(&raw)?;
        let agent = agents
            .iter()
            .find(|agent| agent.id == key)
            .ok_or(SourcePreparationError::LegacyOriginUnresolved)?;
        if configuration_identity(&agent.command, &agent.envs) != selection.calculated_id {
            return Err(SourcePreparationError::LegacyOriginUnresolved);
        }
        let mut units = std::collections::BTreeSet::from([LegacyOriginUnit::Entry(key.into())]);
        if let Some(letter) = &selection.requested_profile {
            let cell = profiles
                .profiles_by_agent
                .get(key)
                .and_then(|cells| cells.get(letter))
                .ok_or(SourcePreparationError::LegacyOriginUnresolved)?;
            if selection.profile_calculated_id.as_deref() != Some(profile_identity(cell).as_str()) {
                return Err(SourcePreparationError::LegacyOriginUnresolved);
            }
            units.insert(LegacyOriginUnit::Profile(key.into(), letter.clone()));
        }
        if let Some(matrix) = matrix {
            units.insert(LegacyOriginUnit::Default(matrix.into()));
        }
        Ok(LegacyOriginLinkProof {
            binding: self.binding.clone(),
            units,
        })
    }

    pub(crate) fn require_legacy_units(
        &self,
        units: impl IntoIterator<Item = LegacyOriginUnit>,
        proof: Option<&LegacyOriginLinkProof>,
    ) -> Result<(), SourcePreparationError> {
        for unit in units {
            match self.require_disjoint([unit.clone()]) {
                Ok(()) => {}
                Err(SourcePreparationError::LegacyOriginUnresolved)
                    if proof.is_some_and(|proof| {
                        proof.binding == self.binding && proof.units.contains(&unit)
                    }) => {}
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }
}

#[allow(dead_code)] // Dormant until the later source protocol cuts.
pub(crate) fn collect_origin_units(
    value: &Value,
    out: &mut std::collections::BTreeSet<LegacyOriginUnit>,
    unknown: &mut Vec<Vec<String>>,
) {
    if let Some(agents) = value.get("agents") {
        let mut keys = HashSet::new();
        match agents.as_array() {
            Some(agents) => {
                for agent in agents {
                    match agent.get("id").and_then(Value::as_str) {
                        Some(key) if !key.is_empty() && keys.insert(key) => {
                            out.insert(LegacyOriginUnit::Entry(key.into()));
                        }
                        _ => unknown.push(vec!["agents".into()]),
                    }
                }
            }
            None => unknown.push(vec!["agents".into()]),
        }
    }
    let Some(profiles) = value.get("codingAgentProfiles") else {
        return;
    };
    if !profiles.is_object() {
        unknown.push(vec!["codingAgentProfiles".into()]);
        return;
    }
    for name in [
        "profilesByAgent",
        "profileLabelsByAgent",
        "identityByAgent",
        "defaultProfileByAgent",
        "profileSlots",
    ] {
        let Some(map) = profiles.get(name) else {
            continue;
        };
        let Some(map) = map.as_object() else {
            unknown.push(vec!["codingAgentProfiles".into(), name.into()]);
            continue;
        };
        for (key, value) in map {
            match name {
                "defaultProfileByAgent" => {
                    out.insert(LegacyOriginUnit::Default(key.clone()));
                }
                "profileSlots" => {
                    out.insert(LegacyOriginUnit::Slot(key.clone()));
                }
                "identityByAgent" => {
                    out.insert(LegacyOriginUnit::Entry(key.clone()));
                }
                _ => {
                    if let Some(cells) = value.as_object() {
                        for letter in cells.keys() {
                            out.insert(if name == "profilesByAgent" {
                                LegacyOriginUnit::Profile(key.clone(), letter.clone())
                            } else {
                                LegacyOriginUnit::Presentation(key.clone(), letter.clone())
                            });
                        }
                    } else {
                        unknown.push(vec!["codingAgentProfiles".into(), name.into(), key.clone()]);
                    }
                }
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)] // Dormant until the later source protocol cuts.
pub(crate) struct PreparedSourceCandidate {
    pub(crate) source: SourceRef,
    pub(crate) expected_revision: SourceRevision,
    pub(crate) context_revision: String,
    pub(crate) before_bytes: Option<Vec<u8>>,
    pub(crate) serialized_after: Vec<u8>,
    pub(crate) persisted_names: Vec<(String, String)>,
    pub(crate) origin_binding: Option<OriginBinding>,
}

#[allow(dead_code)] // The later writer calls this while holding its proofs.
impl PreparedSourceCandidate {
    pub(crate) fn revalidate_before_write(
        &self,
        context: &SourceReadContext,
    ) -> Result<(), SourcePreparationError> {
        if self.context_revision != context.revision {
            return Err(SourcePreparationError::ContextChanged);
        }
        let observed = OriginPhysicalInput::read(&context.locator(&self.source)?)
            .revision()
            .ok_or(SourcePreparationError::OriginUnavailable)?;
        if observed != self.expected_revision {
            return Err(SourcePreparationError::StaleRevision);
        }
        if let Some(binding) = &self.origin_binding {
            if binding.registered.is_none()
                || binding.settings.is_none()
                || binding.overlay.is_none()
            {
                return Err(SourcePreparationError::OriginUnavailable);
            }
            let observed = OriginInputs::read(context);
            if &observed.binding != binding {
                return Err(SourcePreparationError::StaleRevision);
            }
        }
        Ok(())
    }
}

#[allow(dead_code)] // Dormant until the later source protocol cuts.
pub(crate) fn raw_entries(value: &Value, key_name: &str) -> BTreeMap<String, Value> {
    value
        .get("agents")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            entry
                .get(key_name)
                .and_then(Value::as_str)
                .map(|key| (key.into(), entry.clone()))
        })
        .collect()
}

#[allow(dead_code)] // Dormant until the later source protocol cuts.
pub(crate) fn changed_origin_units(
    before: &Value,
    after: &Value,
    origin: &LegacyCompatibilityOrigin,
) -> std::collections::BTreeSet<LegacyOriginUnit> {
    let mut affected = std::collections::BTreeSet::new();
    let old = raw_entries(before, "id");
    let new = raw_entries(after, "id");
    for key in old.keys().chain(new.keys()) {
        if old.get(key) != new.get(key) {
            affected.insert(LegacyOriginUnit::Entry(key.clone()));
            affected.extend(
                origin
                    .represented
                    .iter()
                    .filter(|unit| matches!(unit, LegacyOriginUnit::Profile(k, _) if k == key))
                    .cloned(),
            );
        }
    }
    // Include complete raw units, including removals, unknown fields within a
    // cell and absent/null changes. Labels stay a separate presentation domain.
    for map_name in [
        "profilesByAgent",
        "profileLabelsByAgent",
        "identityByAgent",
        "defaultProfileByAgent",
        "profileSlots",
    ] {
        let root = |v: &Value| {
            v.get("codingAgentProfiles")
                .and_then(|v| v.get(map_name))
                .cloned()
        };
        let old = root(before);
        let new = root(after);
        if old == new {
            continue;
        }
        let keys: std::collections::BTreeSet<_> = old
            .as_ref()
            .and_then(Value::as_object)
            .into_iter()
            .flat_map(|m| m.keys())
            .chain(
                new.as_ref()
                    .and_then(Value::as_object)
                    .into_iter()
                    .flat_map(|m| m.keys()),
            )
            .cloned()
            .collect();
        for key in keys {
            let a = old.as_ref().and_then(|v| v.get(&key));
            let b = new.as_ref().and_then(|v| v.get(&key));
            if a == b {
                continue;
            }
            match map_name {
                "defaultProfileByAgent" => {
                    affected.insert(LegacyOriginUnit::Default(key));
                }
                "profileSlots" => {
                    affected.insert(LegacyOriginUnit::Slot(key));
                }
                "identityByAgent" => {
                    affected.extend(
                        origin
                            .represented
                            .iter()
                            .filter(|u| matches!(u, LegacyOriginUnit::Profile(k, _) if k == &key))
                            .cloned(),
                    );
                    // A missing/orphan profile container still needs its own
                    // identity ownership checked, not equality of the stamp.
                    if origin.applied_paths.iter().any(|p| {
                        origin_paths_overlap(
                            p,
                            &[
                                "codingAgentProfiles".into(),
                                "identityByAgent".into(),
                                key.clone(),
                            ],
                        )
                    }) {
                        affected.insert(LegacyOriginUnit::Profile(key, String::new()));
                    }
                }
                _ => {
                    let letters: std::collections::BTreeSet<_> = a
                        .and_then(Value::as_object)
                        .into_iter()
                        .flat_map(|m| m.keys())
                        .chain(
                            b.and_then(Value::as_object)
                                .into_iter()
                                .flat_map(|m| m.keys()),
                        )
                        .cloned()
                        .collect();
                    for letter in letters {
                        if a.and_then(|v| v.get(&letter)) != b.and_then(|v| v.get(&letter)) {
                            affected.insert(if map_name == "profilesByAgent" {
                                LegacyOriginUnit::Profile(key.clone(), letter)
                            } else {
                                LegacyOriginUnit::Presentation(key.clone(), letter)
                            });
                        }
                    }
                }
            }
        }
    }
    affected
}

#[allow(dead_code)] // Dependency inventory is owned by the later coordinator.
pub(crate) fn unresolved_default_dependency(
    origin: &LegacyCompatibilityOrigin,
    units: &std::collections::BTreeSet<LegacyOriginUnit>,
) -> bool {
    units.iter().any(|unit| {
        matches!(
            unit,
            LegacyOriginUnit::Entry(_) | LegacyOriginUnit::Profile(_, _)
        )
    }) && origin.represented.iter().any(|unit| {
        matches!(unit, LegacyOriginUnit::Default(_))
            && origin.require_disjoint([unit.clone()]).is_err()
    })
}

/// Unknown persisted fields are not discarded by the typed validation view.
/// Schema keys derive from the concrete model; raw absence/null remains raw.
#[allow(dead_code)] // Dormant until the later source protocol cuts.
pub(crate) fn preserve_unknown_fields(before: &Value, after: &Value, model: &Value) -> bool {
    match (before, after, model) {
        (Value::Object(old), Value::Object(new), Value::Object(known)) => {
            old.iter().all(|(key, value)| match known.get(key) {
                Some(shape) => new
                    .get(key)
                    .is_none_or(|next| preserve_unknown_fields(value, next, shape)),
                None => new.get(key) == Some(value),
            })
        }
        (Value::Array(old), Value::Array(new), Value::Array(known)) => {
            let key = if old.first().is_some_and(|v| v.get("id").is_some()) {
                "id"
            } else {
                "key"
            };
            old.iter().all(|entry| {
                let id = entry.get(key);
                match id {
                    Some(id) => match new.iter().find(|entry| entry.get(key) == Some(id)) {
                        Some(next) => known
                            .iter()
                            .find(|entry| entry.get(key) == Some(id))
                            .is_some_and(|shape| preserve_unknown_fields(entry, next, shape)),
                        None => true, // Explicit removal is checked by OperationAffected.
                    },
                    None => true,
                }
            })
        }
        _ => true,
    }
}

#[allow(dead_code)] // Dormant until the later source protocol cuts.
pub(crate) fn validate_source_identity_groups(
    before: &Value,
    after: &Value,
    registered: bool,
) -> Result<(), SourcePreparationError> {
    let groups = |value: &Value| -> Result<
        BTreeMap<String, std::collections::BTreeSet<String>>,
        SourcePreparationError,
    > {
        let mut groups = BTreeMap::<String, std::collections::BTreeSet<String>>::new();
        let (agents, profiles) = if registered {
            let (agents, profiles) = decode_registered_raw(value)?;
            (
                agents
                    .into_iter()
                    .map(|a| (a.id, a.command, a.envs))
                    .collect::<Vec<_>>(),
                profiles.profiles_by_agent,
            )
        } else {
            let (agents, profiles) = decode_physical_catalog(value)
                .map_err(|_| SourcePreparationError::InvalidCandidate)?;
            (
                agents
                    .into_iter()
                    .map(|a| (a.id, a.command, a.envs))
                    .collect::<Vec<_>>(),
                profiles.profiles_by_agent,
            )
        };
        for (key, command, envs) in agents {
            groups
                .entry(configuration_identity(&command, &envs))
                .or_default()
                .insert(key.clone());
            for (letter, profile) in profiles
                .get(&key)
                .into_iter()
                .flat_map(|cells| cells.iter())
            {
                groups
                    .entry(format!(
                        "{}/{}",
                        configuration_identity(&command, &envs),
                        profile_identity(profile)
                    ))
                    .or_default()
                    .insert(letter.clone());
            }
        }
        Ok(groups)
    };
    let old = groups(before)?;
    let new = groups(after)?;
    if let Some((id, members)) = new.iter().find(|(id, members)| {
        id.contains("/pid1:")
            && members.len() > 1
            && old.get(*id).is_none_or(|prior| !members.is_subset(prior))
    }) {
        let entries = raw_entries(after, if registered { "id" } else { "key" });
        let profile_key = id.rsplit_once("/pid1:").map(|(key, _)| key);
        let incumbent = profile_key.unwrap_or_else(|| {
            old.get(id)
                .and_then(|prior| prior.iter().find(|key| members.contains(*key)))
                .or_else(|| members.first())
                .expect("duplicate group")
                .as_str()
        });
        return Err(SourcePreparationError::Identity(IdentityValidationError {
            code: if profile_key.is_some() {
                IdentityValidationCode::DuplicateProfileIdentity
            } else {
                IdentityValidationCode::DuplicateConfigurationIdentity
            },
            existing_name: entries
                .get(incumbent)
                .and_then(|entry| entry.get("label"))
                .and_then(Value::as_str)
                .unwrap_or(incumbent)
                .into(),
            entry_hint: incumbent.into(),
            requested_profile: profile_key.and_then(|_| members.last().cloned()),
        }));
    }
    Ok(())
}

#[allow(dead_code)] // Dormant until the later source protocol cuts.
/// Grouping preserves each original selection's Entry/Profile/Default relation.
/// Only the complete backend inventory factory may construct this aggregate.
#[derive(Debug)]
pub(crate) struct SourceOriginalProofAggregate {
    pub(crate) originals: Vec<LegacyOriginLinkProof>,
}

pub(crate) fn require_source_original_units(
    origin: &LegacyCompatibilityOrigin,
    units: &std::collections::BTreeSet<LegacyOriginUnit>,
    aggregate: Option<&SourceOriginalProofAggregate>,
) -> Result<(), SourcePreparationError> {
    let Some(aggregate) = aggregate else {
        if unresolved_default_dependency(origin, units) {
            return Err(SourcePreparationError::LegacyOriginUnresolved);
        }
        return origin.require_disjoint(units.iter().cloned());
    };
    if aggregate
        .originals
        .iter()
        .any(|proof| proof.binding != origin.binding)
    {
        return Err(SourcePreparationError::StaleRevision);
    }
    for unit in units {
        if origin.require_disjoint([unit.clone()]).is_ok() {
            continue;
        }
        let mut covered = false;
        for proof in aggregate
            .originals
            .iter()
            .filter(|proof| proof.units.contains(unit))
        {
            // This model records binding and unit membership only; the per-unit
            // loop rechecks those facts. Same-Default relation coherence is
            // enforced below; distinct default/profile groups remain separate.
            origin.require_legacy_units([unit.clone()], Some(proof))?;
            covered = true;
        }
        if !covered {
            return Err(SourcePreparationError::LegacyOriginUnresolved);
        }
    }
    let entry_or_profile = units.iter().any(|unit| {
        matches!(
            unit,
            LegacyOriginUnit::Entry(_) | LegacyOriginUnit::Profile(_, _)
        )
    });
    if !entry_or_profile {
        return Ok(());
    }
    for default in origin
        .represented
        .iter()
        .filter(|unit| matches!(unit, LegacyOriginUnit::Default(_)))
    {
        if origin.require_disjoint([default.clone()]).is_ok() {
            continue;
        }
        let mut groups = aggregate
            .originals
            .iter()
            .filter(|proof| proof.units.contains(default));
        let group = groups
            .next()
            .ok_or(SourcePreparationError::LegacyOriginUnresolved)?;
        if groups.any(|other| other.units != group.units) {
            return Err(SourcePreparationError::LegacyOriginUnresolved);
        }
        let keys: Vec<_> = group
            .units
            .iter()
            .filter_map(|unit| match unit {
                LegacyOriginUnit::Entry(key) => Some(key),
                _ => None,
            })
            .collect();
        if keys.len() != 1 {
            return Err(SourcePreparationError::LegacyOriginUnresolved);
        }
        let key = keys[0];
        let profiles: Vec<_> = group
            .units
            .iter()
            .filter_map(|unit| match unit {
                LegacyOriginUnit::Profile(profile_key, letter) if profile_key == key => {
                    Some(letter)
                }
                _ => None,
            })
            .collect();
        if profiles.len() > 1 {
            return Err(SourcePreparationError::LegacyOriginUnresolved);
        }
        let changes_profile = units.iter().any(|unit| {
            matches!(unit,
            LegacyOriginUnit::Profile(profile_key, _) if profile_key == key)
        });
        // Missing original selected profile cannot prove disjunction for K/L.
        if changes_profile && profiles.is_empty() {
            return Err(SourcePreparationError::LegacyOriginUnresolved);
        }
        let linked_change = units.contains(&LegacyOriginUnit::Entry(key.clone()))
            || profiles.iter().any(|letter| {
                units.contains(&LegacyOriginUnit::Profile(key.clone(), (*letter).clone()))
            })
            || units.contains(default);
        if linked_change {
            origin.require_legacy_units([default.clone()], Some(group))?;
        }
    }
    Ok(())
}

#[allow(dead_code)]
pub(crate) fn prepare_source_candidate(
    snapshot: &RawSourceSnapshot,
    expected: &SourceRevision,
    candidate: &Value,
    origin: Option<&LegacyCompatibilityOrigin>,
) -> Result<PreparedSourceCandidate, SourcePreparationError> {
    prepare_source_candidate_with_original_links(snapshot, expected, candidate, origin, None)
}

pub(crate) fn prepare_source_candidate_with_original_links(
    snapshot: &RawSourceSnapshot,
    expected: &SourceRevision,
    candidate: &Value,
    origin: Option<&LegacyCompatibilityOrigin>,
    proofs: Option<&SourceOriginalProofAggregate>,
) -> Result<PreparedSourceCandidate, SourcePreparationError> {
    if expected != &snapshot.snapshot.revision {
        return Err(SourcePreparationError::StaleRevision);
    }
    if !snapshot.snapshot.capabilities.editable_save
        && !snapshot.snapshot.capabilities.editable_create
    {
        return Err(SourcePreparationError::ReadOnly);
    }
    let registered = snapshot.snapshot.source.kind == SourceKind::RegisteredInstance;
    if registered {
        decode_registered_raw(candidate)?;
    }
    let before = snapshot
        .raw
        .value()
        .or_else(|| snapshot.candidate_template.clone())
        .ok_or(SourcePreparationError::InvalidCandidate)?;
    let mut retained_candidate = candidate.clone();
    retain_configuration_commands(
        &mut retained_candidate,
        &before,
        if registered { "id" } else { "key" },
        !registered,
        true,
        true,
    )
    .map_err(|_| SourcePreparationError::InvalidCandidate)?;
    let candidate = &retained_candidate;
    let model = if registered {
        let (agents, profiles) = decode_registered_raw(candidate)?;
        serde_json::json!({"agents":agents, "codingAgentProfiles":profiles})
    } else {
        decode_physical_catalog(candidate).map_err(|_| SourcePreparationError::InvalidCandidate)?;
        candidate.clone()
    };
    // Binding fields are validated and preserved by canonical_profile_source,
    // including unresolved raw legacy entries omitted from the editor's ID view.
    let mut unknown_before = before.clone();
    let binding = if registered {
        unknown_before.get_mut("codingAgentProfiles")
    } else {
        Some(&mut unknown_before)
    };
    if let Some(binding) = binding.and_then(Value::as_object_mut) {
        for key in [
            "profileBindingVersion",
            "profilesByBaseHash",
            "profileLabelsByBaseHash",
            if registered {
                "profilesByAgent"
            } else {
                "codingAgentProfiles"
            },
            if registered {
                "profileLabelsByAgent"
            } else {
                "profileLabels"
            },
        ] {
            binding.remove(key);
        }
        if registered {
            binding.remove("identityByAgent");
        }
    }
    // Catalog slots default to an empty object. Compare an omitted default
    // without discarding any actual before/candidate field or opaque data.
    if !registered
        && before
            .get("profileSlots")
            .and_then(Value::as_object)
            .is_some_and(Map::is_empty)
        && candidate.get("profileSlots").is_none()
    {
        unknown_before
            .as_object_mut()
            .expect("catalog object")
            .remove("profileSlots");
    }
    if !preserve_unknown_fields(&unknown_before, candidate, &model) {
        return Err(SourcePreparationError::InvalidCandidate);
    }
    validate_source_identity_groups(&before, candidate, registered)?;
    if registered {
        let origin = origin.ok_or(SourcePreparationError::OriginUnavailable)?;
        if origin.unknown_paths.iter().any(Vec::is_empty) {
            return Err(SourcePreparationError::OriginUnavailable);
        }
        if origin.binding.registered.is_none()
            || origin.binding.settings.is_none()
            || origin.binding.overlay.is_none()
        {
            return Err(SourcePreparationError::OriginUnavailable);
        }
        if origin.binding.context != snapshot.context
            || origin.binding.registered.as_ref() != Some(expected)
        {
            return Err(SourcePreparationError::StaleRevision);
        }
        let units = changed_origin_units(&before, candidate, origin);
        require_source_original_units(origin, &units, proofs)?;
    }
    let mut candidate = candidate.clone();
    let key_name = if registered { "id" } else { "key" };
    let label_name = "label";
    let incumbents = raw_entries(&before, key_name);
    let mut used = std::collections::BTreeSet::new();
    let mut names = Vec::new();
    if let Some(agents) = candidate.get_mut("agents").and_then(Value::as_array_mut) {
        // Retained unchanged names are the incumbents even when a newly added
        // row appears earlier in the candidate's order.
        for agent in agents.iter() {
            let key = agent
                .get(key_name)
                .and_then(Value::as_str)
                .unwrap_or_default();
            if incumbents.get(key).and_then(|a| a.get(label_name)) == agent.get(label_name) {
                if let Some(label) = agent.get(label_name).and_then(Value::as_str) {
                    used.insert(label.trim().to_lowercase());
                }
            }
        }
        for agent in agents {
            let key = agent
                .get(key_name)
                .and_then(Value::as_str)
                .unwrap_or_default();
            let label = agent
                .get(label_name)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            if incumbents.get(key).and_then(|a| a.get(label_name)) == agent.get(label_name) {
                continue;
            }
            let mut persisted = label.clone();
            let mut suffix = 1;
            while !used.insert(persisted.trim().to_lowercase()) {
                persisted = format!("{label} ({suffix})");
                suffix += 1;
            }
            if persisted != label {
                names.push((label, persisted.clone()));
                agent
                    .as_object_mut()
                    .ok_or(SourcePreparationError::InvalidCandidate)?
                    .insert(label_name.into(), Value::String(persisted));
            }
        }
    }
    // Naming is also part of the full diff. Recheck it before serialization.
    if registered {
        let origin = origin.ok_or(SourcePreparationError::OriginUnavailable)?;
        let units = changed_origin_units(&before, &candidate, origin);
        require_source_original_units(origin, &units, proofs)?;
    }
    let candidate = canonical_profile_source(&before, &candidate, registered)
        .map_err(|_| SourcePreparationError::InvalidCandidate)?;
    let mut serialized_after = serde_json::to_vec_pretty(&candidate)
        .map_err(|_| SourcePreparationError::InvalidCandidate)?;
    serialized_after.push(b'\n');
    Ok(PreparedSourceCandidate {
        source: snapshot.snapshot.source.clone(),
        expected_revision: expected.clone(),
        context_revision: snapshot.context.revision.clone(),
        before_bytes: match &snapshot.raw {
            OriginPhysicalInput::Bytes(bytes) => Some(bytes.clone()),
            _ => None,
        },
        serialized_after,
        persisted_names: names,
        origin_binding: origin.map(|origin| origin.binding.clone()),
    })
}

#[allow(dead_code)] // Dormant until the later source protocol cuts.
pub(crate) const SOURCE_GENERAL_PROTECTED: &[&str] = &[
    "agents",
    "codingAgentProfiles",
    "profileLabels",
    "rootToken",
    "projectPaths",
    "projectPath",
    "archivedProjectPaths",
    "railCollapsedProjects",
    "railFavoritesCollapsed",
    "terminalSnapshotsEnabled",
    "mainGeometry",
    "mainWindowDisplayState",
    "projectPathState",
    "localOverlayState",
];

#[allow(dead_code)] // Dormant until the later source protocol cuts.
pub(crate) struct PreparedSettingsWithoutSourceWrite {
    pub(crate) serialized_after: Vec<u8>,
    pub(crate) redecoded_settings: AppSettings,
}

#[allow(dead_code)] // Serde's field table also includes omitted optional fields.
pub(crate) fn source_general_field_names() -> Vec<&'static str> {
    struct Fields<'a>(&'a mut Vec<&'static str>);
    impl<'de> serde::Deserializer<'de> for Fields<'_> {
        type Error = serde::de::value::Error;
        fn deserialize_any<V: serde::de::Visitor<'de>>(
            self,
            _: V,
        ) -> Result<V::Value, Self::Error> {
            Err(serde::de::Error::custom("field table only"))
        }
        fn deserialize_struct<V: serde::de::Visitor<'de>>(
            self,
            _: &'static str,
            fields: &'static [&'static str],
            _: V,
        ) -> Result<V::Value, Self::Error> {
            self.0.extend_from_slice(fields);
            Err(serde::de::Error::custom("field table captured"))
        }
        serde::forward_to_deserialize_any! {
            bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string
            bytes byte_buf option unit unit_struct newtype_struct seq tuple tuple_struct
            map enum identifier ignored_any
        }
    }
    let mut names = Vec::new();
    let _ = AppSettings::deserialize(Fields(&mut names));
    names
}

/// General-only pure preparation over the captured settings base. Never calls
/// a saver, migration, export, order/adoption/repair helper, or source writer.
#[allow(dead_code)] // Dormant until the later source protocol cuts.
pub(crate) fn prepare_settings_without_source_write(
    inputs: &OriginInputs,
    candidate_general: &Value,
) -> Result<PreparedSettingsWithoutSourceWrite, SourcePreparationError> {
    let origin = LegacyCompatibilityOrigin::classify(inputs);
    let mut out = match &inputs.settings {
        OriginPhysicalInput::Absent => Value::Object(Map::new()),
        OriginPhysicalInput::Bytes(_) => inputs
            .settings
            .value()
            .ok_or(SourcePreparationError::OriginUnavailable)?,
        OriginPhysicalInput::Unknown => return Err(SourcePreparationError::OriginUnavailable),
    };
    // Capture disk restoration before applying the draft. In particular, an
    // overlay-owned general key must retain its raw absent/null/value state,
    // rather than capturing the effective value echoed by the caller.
    let mut restoration_base = out.clone();
    let restoration = match &inputs.overlay {
        OriginPhysicalInput::Bytes(bytes) => LocalSettingsOverlay::from_overlay_bytes_paired(
            &mut restoration_base,
            bytes,
            OVERLAY_INELIGIBLE_DISK_KEYS,
            OVERLAY_INELIGIBLE_LEGACY_KEYS,
            OVERLAY_DERIVED_ID_CLOSURES,
            CONFIGURATION_COMMAND_PAIRS,
        ),
        _ => LocalSettingsOverlay::default(),
    };
    let map = out
        .as_object_mut()
        .ok_or(SourcePreparationError::InvalidCandidate)?;
    let general = candidate_general
        .as_object()
        .ok_or(SourcePreparationError::InvalidCandidate)?;
    let public = serde_json::to_value(AppSettings::default())
        .map_err(|_| SourcePreparationError::InvalidCandidate)?;
    let fields = source_general_field_names();
    if general.keys().any(|key| {
        SOURCE_GENERAL_PROTECTED.contains(&key.as_str()) || !fields.contains(&key.as_str())
    }) {
        return Err(SourcePreparationError::ProtectedSettingsField);
    }
    if public
        .as_object()
        .ok_or(SourcePreparationError::InvalidCandidate)?
        .keys()
        .any(|key| !SOURCE_GENERAL_PROTECTED.contains(&key.as_str()) && !general.contains_key(key))
    {
        return Err(SourcePreparationError::InvalidCandidate);
    }
    // Only caller-owned general keys are updated. Unknown/source/placement keys
    // retain exact raw absence, null or value, even when the typed view differs.
    for key in fields {
        if !SOURCE_GENERAL_PROTECTED.contains(&key) && !general.contains_key(key) {
            map.remove(key);
        }
    }
    for (key, value) in general {
        map.insert(key.clone(), value.clone());
    }
    let mut effective = out.clone();
    if let (Some(dst), Some(raw)) = (
        effective.as_object_mut(),
        inputs
            .registered
            .value()
            .filter(|raw| decode_registered_raw(raw).is_ok())
            .and_then(|v| v.as_object().cloned()),
    ) {
        dst.extend(agents_instance_groups(&raw));
    }
    let unmerged = effective.clone();
    if let OriginPhysicalInput::Bytes(bytes) = &inputs.overlay {
        let overlay = LocalSettingsOverlay::from_overlay_bytes_paired(
            &mut effective,
            bytes,
            OVERLAY_INELIGIBLE_DISK_KEYS,
            OVERLAY_INELIGIBLE_LEGACY_KEYS,
            OVERLAY_DERIVED_ID_CLOSURES,
            CONFIGURATION_COMMAND_PAIRS,
        );
        let mut probe = effective.clone();
        if let Some(root) = probe.as_object_mut() {
            root.entry("agents")
                .or_insert_with(|| Value::Array(Vec::new()));
        }
        if !overlay.is_empty() && serde_json::from_value::<AppSettings>(probe).is_err() {
            effective = unmerged;
        } else {
            restoration.restore_base(out.as_object_mut().expect("object checked"));
        }
    }
    if origin.unknown_paths.iter().any(Vec::is_empty) {
        return Err(SourcePreparationError::OriginUnavailable);
    }
    if let Some(root) = effective.as_object_mut() {
        root.entry("agents")
            .or_insert_with(|| Value::Array(Vec::new()));
    }
    let base = production_instance_base();
    apply_project_decode_to_value(
        &mut effective,
        base.as_deref(),
        &projects::FsCandidateResolver,
    );
    let redecoded_settings =
        serde_json::from_value(effective).map_err(|_| SourcePreparationError::InvalidCandidate)?;
    let mut serialized_after =
        serde_json::to_vec_pretty(&out).map_err(|_| SourcePreparationError::InvalidCandidate)?;
    serialized_after.push(b'\n');
    Ok(PreparedSettingsWithoutSourceWrite {
        serialized_after,
        redecoded_settings,
    })
}

pub(crate) fn identity_preimage(parts: impl IntoIterator<Item = impl AsRef<str>>) -> String {
    let mut scalars = Vec::new();
    for part in parts {
        scalars.extend(
            part.as_ref()
                .chars()
                .flat_map(char::to_lowercase)
                .filter(|scalar| !scalar.is_whitespace()),
        );
    }
    scalars.sort_unstable();
    scalars.into_iter().collect()
}

pub(crate) fn configuration_identity_preimage(command: &str, envs: &[CodingAgentEnv]) -> String {
    identity_preimage(
        std::iter::once(command).chain(
            envs.iter()
                .flat_map(|row| [row.key.as_str(), row.value.as_str()]),
        ),
    )
}

pub(crate) fn profile_identity_preimage(profile: &ProfileCellConfig) -> String {
    identity_preimage(
        std::iter::once(profile.command.as_str()).chain(
            profile
                .env
                .iter()
                .flat_map(|(key, value)| [key.as_str(), value.as_str()]),
        ),
    )
}

pub(crate) fn identity_digest(prefix: &str, preimage: &str) -> String {
    use sha2::{Digest, Sha256};
    format!("{prefix}{:x}", Sha256::digest(preimage.as_bytes()))
}

/// Includes every configured env row, even disabled rows. Labels, backend and
/// other metadata do not participate. Anagrams and repeated scalars are retained.
pub fn configuration_identity(command: &str, envs: &[CodingAgentEnv]) -> String {
    identity_digest("cid1:", &configuration_identity_preimage(command, envs))
}

pub(crate) fn configuration_identity_debug(command: &str, envs: &[CodingAgentEnv]) -> Vec<String> {
    std::iter::once(command.to_owned())
        .chain(
            envs.iter()
                .flat_map(|row| [row.key.clone(), row.value.clone()]),
        )
        .collect()
}

pub(crate) fn pair_identity_debug(
    command: &str,
    envs: &[CodingAgentEnv],
    profile: &ProfileCellConfig,
) -> Vec<String> {
    let mut leaves = configuration_identity_debug(command, envs);
    leaves.push(profile.command.clone());
    leaves.extend(
        profile
            .env
            .iter()
            .flat_map(|(key, value)| [key.clone(), value.clone()]),
    );
    leaves
}

/// Refresh informational metadata only; stable IDs and row order stay untouched.
pub fn refresh_agent_id_hashes(agents: &mut [AgentConfig]) {
    for agent in agents {
        agent.id_hash = Some(configuration_identity(&agent.command, &agent.envs));
        agent.id_hash_debug = Some(configuration_identity_debug(&agent.command, &agent.envs));
    }
}

/// Informational identity of one base/profile pair, using the existing scalar rules.
pub(crate) fn pair_identity(
    command: &str,
    envs: &[CodingAgentEnv],
    profile: &ProfileCellConfig,
) -> String {
    identity_digest(
        "cid1:",
        &identity_preimage([
            configuration_identity_preimage(command, envs),
            profile_identity_preimage(profile),
        ]),
    )
}

pub(crate) fn refresh_pair_id_hashes(
    agents: &[AgentConfig],
    profiles: &mut CodingAgentProfilesConfig,
) {
    for (id, cells) in &mut profiles.profiles_by_agent {
        // Historical duplicate IDs use the same first-match rule as spawn.
        let agent = agents.iter().find(|agent| agent.id == *id);
        for cell in cells.values_mut() {
            cell.id_hash = agent.map(|agent| pair_identity(&agent.command, &agent.envs, cell));
            cell.id_hash_debug =
                agent.map(|agent| pair_identity_debug(&agent.command, &agent.envs, cell));
        }
    }
}

/// Project only identity inputs from the first physical row. Protected groups
/// are best effort here; their existing full decoders still own save errors.
pub(crate) fn physical_pair_base(
    agents: Option<&Value>,
    id: &str,
) -> Option<(String, Vec<CodingAgentEnv>)> {
    let row = agents?
        .as_array()?
        .iter()
        .find(|row| row.get("id").and_then(Value::as_str) == Some(id))?
        .as_object()?;
    let command = raw_configuration_command(row, "baseCommand")
        .ok()?
        .unwrap_or_default();
    let envs = match row.get("envs") {
        Some(value) => serde_json::from_value(value.clone()).ok()?,
        None => Vec::new(),
    };
    Some((command.to_owned(), envs))
}

pub(crate) fn refresh_physical_pair_id_hashes(agents: Option<&Value>, profiles: &mut Value) {
    let Some(rows) = profiles
        .get_mut("profilesByAgent")
        .and_then(Value::as_object_mut)
    else {
        return;
    };
    for (id, cells) in rows {
        let base = physical_pair_base(agents, id);
        for raw in cells
            .as_object_mut()
            .into_iter()
            .flat_map(|cells| cells.values_mut())
        {
            let metadata = base.as_ref().and_then(|(command, envs)| {
                // Decode the cell's existing aliases/env contract without rewriting it.
                let cell: ProfileCellConfig = serde_json::from_value(raw.clone()).ok()?;
                Some((
                    pair_identity(command, envs, &cell),
                    pair_identity_debug(command, envs, &cell),
                ))
            });
            if let Some(object) = raw.as_object_mut() {
                match metadata {
                    Some((hash, debug)) => {
                        object.insert("id_hash".into(), Value::String(hash));
                        object.insert("id_hash_debug".into(), serde_json::json!(debug));
                    }
                    None => {
                        object.remove("id_hash");
                        object.remove("id_hash_debug");
                    }
                }
            }
        }
    }
}

/// A configured empty cell has an identity; an absent cell has no identity.
/// The base configuration ID and decorative profile letter are not inputs.
pub fn profile_identity(profile: &ProfileCellConfig) -> String {
    identity_digest("pid1:", &profile_identity_preimage(profile))
}

/// Borrowed projection lets each source retain its own complete agent model.
pub struct IdentitySnapshotEntry<'a> {
    pub key: &'a str,
    pub name: &'a str,
    pub command: &'a str,
    pub envs: &'a [CodingAgentEnv],
    pub profiles: Option<&'a BTreeMap<String, ProfileCellConfig>>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum IdentityValidationCode {
    DuplicateConfigurationIdentity,
    DuplicateProfileIdentity,
    IdentityHashCollision,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, thiserror::Error)]
#[error("{code:?}: existing Coding Agent '{existing_name}'")]
#[serde(rename_all = "camelCase")]
pub struct IdentityValidationError {
    pub code: IdentityValidationCode,
    pub existing_name: String,
    pub entry_hint: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requested_profile: Option<String>,
}

/// Validate one physical source at a time; this never compares other sources.
/// Historical duplicate policy and update exclusion are caller-owned: pass the
/// complete proposed snapshot, replacing the updated entry rather than appending.
pub fn validate_identity_snapshot(
    entries: &[IdentitySnapshotEntry<'_>],
) -> Result<(), IdentityValidationError> {
    validate_identity_snapshot_with_digest(entries, identity_digest)
}

pub(crate) fn validate_identity_snapshot_with_digest(
    entries: &[IdentitySnapshotEntry<'_>],
    digest: impl Fn(&str, &str) -> String,
) -> Result<(), IdentityValidationError> {
    let mut configurations = BTreeMap::<String, (String, &str)>::new();
    // Profile identities are local to an agent. Collision evidence, however,
    // must not be discarded simply because a second preimage is in another one.
    let mut profile_preimages = BTreeMap::<String, (String, &str)>::new();
    for entry in entries {
        let preimage = configuration_identity_preimage(entry.command, entry.envs);
        let id = digest("cid1:", &preimage);
        if let Some((existing_preimage, existing_name)) = configurations.get(&id) {
            return Err(IdentityValidationError {
                code: if existing_preimage == &preimage {
                    IdentityValidationCode::DuplicateConfigurationIdentity
                } else {
                    IdentityValidationCode::IdentityHashCollision
                },
                existing_name: (*existing_name).to_owned(),
                entry_hint: entry.key.to_owned(),
                requested_profile: None,
            });
        }
        configurations.insert(id, (preimage, entry.name));
        let mut local_profiles = BTreeMap::<String, &str>::new();
        for (letter, profile) in entry.profiles.into_iter().flat_map(|cells| cells.iter()) {
            let preimage = profile_identity_preimage(profile);
            let id = digest("pid1:", &preimage);
            if let Some((existing_preimage, existing_name)) = profile_preimages.get(&id) {
                if existing_preimage != &preimage {
                    return Err(IdentityValidationError {
                        code: IdentityValidationCode::IdentityHashCollision,
                        existing_name: (*existing_name).to_owned(),
                        entry_hint: entry.key.to_owned(),
                        requested_profile: Some(letter.clone()),
                    });
                }
            }
            profile_preimages
                .entry(id.clone())
                .or_insert((preimage, entry.name));
            if local_profiles.insert(id, letter).is_some() {
                return Err(IdentityValidationError {
                    code: IdentityValidationCode::DuplicateProfileIdentity,
                    existing_name: entry.name.to_owned(),
                    entry_hint: entry.key.to_owned(),
                    requested_profile: Some(letter.clone()),
                });
            }
        }
    }
    Ok(())
}

// Canonical pair signatures deliberately omit labels, order and supplied hashes.
// Counts retain repeated legacy rows, including repeated stable IDs.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum PhysicalIdentityViolation {
    Configuration([(String, String); 2]),
    Profile([(String, String, String); 2]),
}

pub(crate) fn physical_identity_violations(
    groups: &Map<String, Value>,
    configurations_editable: bool,
    profiles_editable: bool,
) -> Result<BTreeMap<PhysicalIdentityViolation, (usize, IdentityValidationError)>, String> {
    let agents: Vec<AgentConfig> = groups
        .get("agents")
        .map(|value| serde_json::from_value(value.clone()))
        .transpose()
        .map_err(|error| error.to_string())?
        .unwrap_or_default();
    let profiles: CodingAgentProfilesConfig = groups
        .get("codingAgentProfiles")
        .map(|value| serde_json::from_value(value.clone()))
        .transpose()
        .map_err(|error| error.to_string())?
        .unwrap_or_default();
    let mut violations = BTreeMap::new();
    let mut record = |key, error| {
        let entry = violations.entry(key).or_insert((0usize, error));
        entry.0 += 1;
    };
    if configurations_editable {
        collect_configuration_identity_violations(&agents, &mut record);
    }
    if profiles_editable {
        collect_profile_identity_violations(&agents, &profiles, &mut record);
    }
    Ok(violations)
}

pub(crate) fn collect_configuration_identity_violations(
    agents: &[AgentConfig],
    record: &mut impl FnMut(PhysicalIdentityViolation, IdentityValidationError),
) {
    let preimages: Vec<_> = agents
        .iter()
        .map(|agent| configuration_identity_preimage(&agent.command, &agent.envs))
        .collect();
    let hashes: Vec<_> = preimages
        .iter()
        .map(|p| identity_digest("cid1:", p))
        .collect();
    for (index, agent) in agents.iter().enumerate() {
        for other in 0..index {
            if hashes[index] != hashes[other]
                || (preimages[index] == preimages[other] && agent.id != agents[other].id)
            {
                continue;
            }
            let mut pair = [
                (agents[other].id.clone(), preimages[other].clone()),
                (agent.id.clone(), preimages[index].clone()),
            ];
            pair.sort();
            record(
                PhysicalIdentityViolation::Configuration(pair),
                IdentityValidationError {
                    code: if preimages[index] == preimages[other] {
                        IdentityValidationCode::DuplicateConfigurationIdentity
                    } else {
                        IdentityValidationCode::IdentityHashCollision
                    },
                    existing_name: agents[other].label.clone(),
                    entry_hint: agent.id.clone(),
                    requested_profile: None,
                },
            );
        }
    }
}

pub(crate) fn collect_profile_identity_violations(
    agents: &[AgentConfig],
    profiles: &CodingAgentProfilesConfig,
    record: &mut impl FnMut(PhysicalIdentityViolation, IdentityValidationError),
) {
    let group_for = |id: &str| {
        agents
            .iter()
            .find(|agent| agent.id == id)
            .map(|agent| configuration_identity(&agent.command, &agent.envs))
            .unwrap_or_else(|| format!("orphan:{id}"))
    };
    let mut groups = HashSet::new();
    let cells: Vec<_> = profiles
        .profiles_by_agent
        .iter()
        .filter(|(id, _)| groups.insert(group_for(id)))
        .flat_map(|(id, cells)| {
            cells
                .iter()
                .filter(|(letter, _)| is_valid_profile_letter(letter))
                .map(move |(letter, cell)| {
                    let preimage = profile_identity_preimage(cell);
                    let hash = identity_digest("pid1:", &preimage);
                    (id, letter, preimage, hash)
                })
        })
        .collect();
    for (index, (id, letter, preimage, hash)) in cells.iter().enumerate() {
        for (old_id, old_letter, old_preimage, old_hash) in &cells[..index] {
            if hash != old_hash || (id != old_id && preimage == old_preimage) {
                continue;
            }
            let mut pair = [
                (
                    group_for(old_id),
                    (*old_letter).clone(),
                    old_preimage.clone(),
                ),
                (group_for(id), (*letter).clone(), preimage.clone()),
            ];
            pair.sort();
            record(
                PhysicalIdentityViolation::Profile(pair),
                IdentityValidationError {
                    code: if preimage == old_preimage {
                        IdentityValidationCode::DuplicateProfileIdentity
                    } else {
                        IdentityValidationCode::IdentityHashCollision
                    },
                    existing_name: agents
                        .iter()
                        .find(|agent| &agent.id == *old_id)
                        .map(|agent| agent.label.clone())
                        .unwrap_or_else(|| (*old_id).clone()),
                    entry_hint: (*id).clone(),
                    requested_profile: Some((*letter).clone()),
                },
            );
        }
    }
}

pub(crate) fn validate_physical_identity_delta(
    before: &Map<String, Value>,
    after: &Map<String, Value>,
    configurations_editable: bool,
    profiles_editable: bool,
) -> Result<(), String> {
    let project = |groups: &Map<String, Value>| {
        let source = Value::Object(groups.clone());
        if physical_profile_rows(&source, true).is_err() {
            Ok(registered_profiles_for_read(&source))
        } else {
            project_physical_profiles(&source, true)
        }
    };
    let before_view = project(before)?;
    let after_view = project(after)?;
    let old = physical_identity_violations(
        before_view.as_object().unwrap(),
        configurations_editable,
        profiles_editable,
    )?;
    let new = physical_identity_violations(
        after_view.as_object().unwrap(),
        configurations_editable,
        profiles_editable,
    )?;
    for (signature, (count, mut error)) in new {
        if count > old.get(&signature).map_or(0, |(count, _)| *count) {
            // A prepended new row must still name an incumbent from BEFORE.
            if let PhysicalIdentityViolation::Configuration(pair) = &signature {
                let incumbent = before
                    .get("agents")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|raw| serde_json::from_value::<AgentConfig>(raw.clone()).ok())
                    .find(|agent| {
                        pair.contains(&(
                            agent.id.clone(),
                            configuration_identity_preimage(&agent.command, &agent.envs),
                        ))
                    });
                if let Some(incumbent) = incumbent {
                    error.existing_name = incumbent.label;
                }
            }
            return Err(error.to_string());
        }
    }
    Ok(())
}

/// Caller excludes the edited entry and supplies incumbent persisted names.
/// This computes a proposed name only; notices belong after the later commit.
pub fn identity_persisted_name(name: &str, occupied: &[String]) -> String {
    let names: HashSet<String> = occupied
        .iter()
        .map(|name| name.trim().to_lowercase())
        .collect();
    let available = |candidate: &str| !names.contains(&candidate.trim().to_lowercase());
    if available(name) {
        return name.to_owned();
    }
    let mut suffix = 1usize;
    loop {
        let candidate = format!("{} ({suffix})", name.trim());
        if available(&candidate) {
            return candidate;
        }
        suffix += 1;
    }
}

#[allow(dead_code)]
pub(crate) fn source_transition_digest<T: Serialize>(
    value: &T,
) -> Result<String, SourceCoordinationError> {
    let bytes = serde_json::to_vec(value).map_err(|_| SourceCoordinationError::Invalid)?;
    let SourceRevision::Bytes { sha256 } = source_bytes_revision(&bytes) else {
        unreachable!()
    };
    Ok(sha256)
}

/// Validate the retained original packet with existing guarded PAIR verification.
/// Pending is admissible here only after the primitive validates its reachable
/// stage; it never licenses preparation of a replacement packet.
#[allow(dead_code)]
pub(crate) fn recover_source_target_packet(
    target: &SourceInventoryTarget,
    operation_id: &str,
    source_key: &str,
    mappings_digest: &str,
    validate: &dyn Fn() -> Result<(), IdentityReferenceError>,
) -> Result<Option<IdentityReferencePacket>, IdentityReferenceError> {
    for _ in 0..=2 {
        validate()?;
        let physical = read_identity_reference_physical(&target.reservation)?;
        let bytes = match &physical {
            IdentityReferencePhysicalState::Absent => return Ok(None),
            IdentityReferencePhysicalState::Bytes { bytes, .. } => bytes,
        };
        let packet: IdentityReferencePacket =
            serde_json::from_slice(bytes).map_err(|_| IdentityReferenceError::Conflict)?;
        let value: Value =
            serde_json::from_slice(bytes).map_err(|_| IdentityReferenceError::Conflict)?;
        if packet.operation_id != operation_id
            || packet.source_physical_key != source_key
            || value.get("mappingsDigest").and_then(Value::as_str) != Some(mappings_digest)
        {
            return Err(IdentityReferenceError::Conflict);
        }
        let before_bytes = serde_json::to_vec(&target.before_protected)
            .map_err(|error| IdentityReferenceError::Io(error.to_string()))?;
        let before = source_bytes_revision(&before_bytes);
        let hashes = packet.protected_revision_digests()?;
        if before
            != (SourceRevision::Bytes {
                sha256: hashes[0].clone(),
            })
        {
            return Err(IdentityReferenceError::Conflict);
        }
        match verify_identity_reference_pair_guarded(
            &target.decisions,
            &target.state,
            &target.reservation,
            operation_id,
            &packet.plan_digest,
            validate,
        ) {
            Ok(_) | Err(IdentityReferenceError::Pending) => {}
            Err(error) => return Err(error),
        }
        validate()?;
        if read_identity_reference_physical(&target.reservation)? == physical {
            return Ok(Some(packet));
        }
    }
    Err(IdentityReferenceError::Conflict)
}

#[allow(dead_code)]
pub(crate) fn verify_completed_source_target(
    target: &SourceInventoryTarget,
    commitment: &SourcePacketRecord,
    validate: &dyn Fn() -> Result<(), IdentityReferenceError>,
) -> Result<(), IdentityReferenceError> {
    validate()?;
    let original_before = serde_json::to_vec(&target.before_protected)
        .map_err(|error| IdentityReferenceError::Io(error.to_string()))?;
    if source_bytes_revision(&original_before) != commitment.before_protected_revision {
        return Err(IdentityReferenceError::Conflict);
    }
    let before = [
        read_identity_reference_physical(&target.decisions)?,
        read_identity_reference_physical(&target.state)?,
    ];
    let protected = identity_reference_protected_revision(&before)?;
    let digest =
        source_transition_digest(&protected).map_err(|_| IdentityReferenceError::Conflict)?;
    if commitment.after_protected_revision != (SourceRevision::Bytes { sha256: digest }) {
        return Err(IdentityReferenceError::Conflict);
    }
    validate()?;
    if before
        != [
            read_identity_reference_physical(&target.decisions)?,
            read_identity_reference_physical(&target.state)?,
        ]
    {
        return Err(IdentityReferenceError::Conflict);
    }
    Ok(())
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SourceOriginalInventoryEvidence {
    pub(crate) discovery: SourceDiscoveryWitness,
    pub(crate) objects: Vec<(PathBuf, (u64, u64))>,
    pub(crate) source_before: IdentityReferencePhysicalState,
    pub(crate) expected_source: SourceRevision,
    pub(crate) after_source: SourceRevision,
    pub(crate) discovered: Vec<SourceDiscoveredTargetEvidence>,
    pub(crate) topology: SourceMembershipTopology,
    pub(crate) inventory_digest: String,
}

#[allow(dead_code)]
pub(crate) fn source_inventory_target(
    root: PathBuf,
    decisions: PathBuf,
    state: PathBuf,
    reservation: PathBuf,
    physical: [IdentityReferencePhysicalState; 2],
    originals: Vec<ConfigurationRef>,
) -> Result<SourceInventoryTarget, IdentityReferenceError> {
    let identity = crate::path_identity::verify_directory(&root)
        .map_err(IdentityReferenceError::Preparation)?;
    if identity.canonical_path != root
        || decisions.parent() != Some(root.as_path())
        || state.parent() != Some(root.as_path())
        || reservation.parent() != Some(root.as_path())
    {
        return Err(IdentityReferenceError::Conflict);
    }
    let before_protected = identity_reference_protected_revision(&physical)?;
    Ok(SourceInventoryTarget {
        root,
        root_object: (identity.object_id.volume, identity.object_id.file),
        decisions,
        state,
        reservation,
        before_protected,
        original_selections: originals,
        original_physical: Some(physical),
    })
}

pub(crate) fn source_original_mappings(
    snapshot: &RawSourceSnapshot,
    prepared: &PreparedSourceCandidate,
) -> Result<Vec<SourceIdentityMapping>, SourcePreparationError> {
    let registered = snapshot.snapshot.source.kind == SourceKind::RegisteredInstance;
    let after: Value = serde_json::from_slice(&prepared.serialized_after)
        .map_err(|_| SourcePreparationError::InvalidCandidate)?;
    let (after_agents, after_profiles) = if registered {
        let (agents, profiles) = decode_registered_raw(&after)?;
        (
            agents
                .iter()
                .map(serde_json::to_value)
                .collect::<Result<Vec<_>, _>>()
                .map_err(|_| SourcePreparationError::InvalidCandidate)?,
            profiles.profiles_by_agent,
        )
    } else {
        let catalog =
            crate::config::coding_agents_catalog::parse_catalog_schema2(&prepared.serialized_after)
                .map_err(|_| SourcePreparationError::InvalidCandidate)?;
        (
            catalog
                .agents
                .iter()
                .map(serde_json::to_value)
                .collect::<Result<Vec<_>, _>>()
                .map_err(|_| SourcePreparationError::InvalidCandidate)?,
            catalog.coding_agent_profiles,
        )
    };
    let key_field = if registered { "id" } else { "key" };
    let old: BTreeMap<_, _> = snapshot
        .snapshot
        .agents
        .iter()
        .map(|agent| {
            agent
                .get(key_field)
                .and_then(Value::as_str)
                .map(|key| (key.to_owned(), agent))
                .ok_or(SourcePreparationError::InvalidCandidate)
        })
        .collect::<Result<_, _>>()?;
    let new: BTreeMap<_, _> = after_agents
        .iter()
        .map(|agent| {
            agent
                .get(key_field)
                .and_then(Value::as_str)
                .map(|key| (key.to_owned(), agent))
                .ok_or(SourcePreparationError::InvalidCandidate)
        })
        .collect::<Result<_, _>>()?;
    let calculated = |value: &&Value| -> Result<String, SourcePreparationError> {
        let command = raw_configuration_command(
            value
                .as_object()
                .ok_or(SourcePreparationError::InvalidCandidate)?,
            "baseCommand",
        )
        .map_err(|_| SourcePreparationError::InvalidCandidate)?
        .ok_or(SourcePreparationError::InvalidCandidate)?;
        let envs: Vec<CodingAgentEnv> = value
            .get("envs")
            .cloned()
            .map(serde_json::from_value)
            .transpose()
            .map_err(|_| SourcePreparationError::InvalidCandidate)?
            .unwrap_or_default();
        Ok(configuration_identity(command, &envs))
    };
    let keys: std::collections::BTreeSet<_> = old.keys().chain(new.keys()).cloned().collect();
    let mut mappings = Vec::new();
    for key in keys {
        let old_calculated_id = old.get(&key).map(&calculated).transpose()?;
        let new_calculated_id = new.get(&key).map(&calculated).transpose()?;
        let old_cells = snapshot.snapshot.profiles.get(&key);
        let new_cells = after_profiles.get(&key);
        let letters: std::collections::BTreeSet<_> = old_cells
            .into_iter()
            .flat_map(|cells| cells.keys())
            .chain(new_cells.into_iter().flat_map(|cells| cells.keys()))
            .cloned()
            .collect();
        let profiles: BTreeMap<_, _> = letters
            .into_iter()
            .filter_map(|letter| {
                let before = old_cells
                    .and_then(|cells| cells.get(&letter))
                    .map(profile_identity);
                let after = new_cells
                    .and_then(|cells| cells.get(&letter))
                    .map(profile_identity);
                (before != after).then_some((letter, (before, after)))
            })
            .collect();
        if old_calculated_id != new_calculated_id || !profiles.is_empty() {
            let unavailable = new_calculated_id.is_none()
                || !registered
                    && crate::config::coding_agents_catalog::BUILTIN_AGENT_SUPPORT
                        .iter()
                        .any(|(builtin, supported)| *builtin == key && !supported);
            mappings.push(SourceIdentityMapping {
                entry_hint: key,
                old_calculated_id,
                new_calculated_id,
                profiles,
                unavailable,
            });
        }
    }
    Ok(mappings)
}

pub(crate) type SourceMembershipTarget = (PathBuf, (u64, u64), PathBuf, PathBuf, PathBuf);

/// Private, backend-validated topology; never a deserialized authority token.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SourceMembershipTopology {
    pub(crate) registry: Vec<String>,
    pub(crate) directories: Vec<(PathBuf, (u64, u64))>,
    pub(crate) slots: Vec<(PathBuf, bool)>,
    pub(crate) membership: Vec<(PathBuf, Vec<PathBuf>)>,
    pub(crate) targets: Vec<SourceMembershipTarget>,
    // Slot coordinate is structural: mutable selection values are excluded.
    pub(crate) links: Vec<(PathBuf, String, SourceRef, String)>,
}

pub(crate) fn source_ref_tuple(
    source: &SourceRef,
) -> (SourceFamily, SourceRole, SourceScope, SourceKind, &str) {
    (
        source.family,
        source.role,
        source.scope,
        source.kind,
        &source.context_root,
    )
}

pub(crate) fn source_inventory_projection(
    editor: &str,
    source: &SourceRef,
    physical_source_key: &str,
    context_revision: &str,
    original: &SourceMembershipTopology,
) -> Result<Vec<u8>, SourceCoordinationError> {
    if editor.is_empty() || physical_source_key.is_empty() || context_revision.is_empty() {
        return Err(SourceCoordinationError::Invalid);
    }
    // Registry order is policy-significant. Everything below is set-valued.
    let mut topology = original.clone();
    topology.directories.sort();
    topology.directories.dedup();
    topology.slots.sort();
    topology.slots.dedup();
    for (_, members) in &mut topology.membership {
        members.sort();
        members.dedup();
    }
    topology.membership.sort();
    topology.membership.dedup();
    topology.targets.sort();
    topology.targets.dedup();
    for pair in topology.directories.windows(2) {
        if pair[0].0 == pair[1].0 {
            return Err(SourceCoordinationError::Invalid);
        }
    }
    for pair in topology.slots.windows(2) {
        if pair[0].0 == pair[1].0 {
            return Err(SourceCoordinationError::Invalid);
        }
    }
    for pair in topology.membership.windows(2) {
        if pair[0].0 == pair[1].0 {
            return Err(SourceCoordinationError::Invalid);
        }
    }
    for pair in topology.targets.windows(2) {
        if pair[0].0 == pair[1].0 {
            return Err(SourceCoordinationError::Invalid);
        }
    }
    let mut links = topology
        .links
        .iter()
        .map(|(root, slot, linked, key)| {
            if slot.is_empty() || linked != source || key != physical_source_key {
                return Err(SourceCoordinationError::Invalid);
            }
            // Encode enum spellings via serde, not Debug or platform formatting.
            serde_json::to_vec(&(root, slot, source_ref_tuple(linked), key))
                .map(|bytes| ((root.clone(), slot.clone()), bytes))
                .map_err(|_| SourceCoordinationError::Invalid)
        })
        .collect::<Result<Vec<_>, _>>()?;
    links.sort();
    links.dedup();
    for pair in links.windows(2) {
        if pair[0].0 == pair[1].0 {
            return Err(SourceCoordinationError::Invalid);
        }
    }
    let links = links
        .into_iter()
        .map(|(_, bytes)| {
            serde_json::from_slice::<Value>(&bytes).map_err(|_| SourceCoordinationError::Invalid)
        })
        .collect::<Result<Vec<_>, _>>()?;
    serde_json::to_vec(&(
        "p03-member-inventory-v1",
        editor,
        1u32,
        source_ref_tuple(source),
        physical_source_key,
        context_revision,
        topology.registry,
        topology.directories,
        topology.slots,
        topology.membership,
        topology.targets,
        links,
    ))
    .map_err(|_| SourceCoordinationError::Invalid)
}

pub(crate) fn source_inventory_digest(
    participant: &str,
    source: &SourceRef,
    physical_source_key: &str,
    context_revision: &str,
    topology: &SourceMembershipTopology,
) -> Result<String, SourceCoordinationError> {
    let bytes = source_inventory_projection(
        participant,
        source,
        physical_source_key,
        context_revision,
        topology,
    )?;
    match source_bytes_revision(&bytes) {
        SourceRevision::Bytes { sha256 } => Ok(sha256),
        SourceRevision::Absent {} => Err(SourceCoordinationError::Invalid),
    }
}

/// Backend semantic coordinate; history cannot manufacture matrix-default authority.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) enum SourceOriginalSlot {
    Decision,
    MatrixDefault,
    ReplicaDefault,
    HistoryKey(String),
    HistoryIndex(usize),
}

impl SourceOriginalSlot {
    pub(crate) fn stored<'a>(&self, target: &'a Value) -> Option<&'a Value> {
        match self {
            Self::Decision => target.pointer("/tooling/configurationRef"),
            Self::MatrixDefault | Self::ReplicaDefault => {
                target.pointer("/tooling/replicaSelectionDefault/configurationRef")
            }
            Self::HistoryKey(key) => target
                .get("tooling")?
                .get("codingAgents")?
                .as_object()?
                .get(key)?
                .get("configurationRef"),
            Self::HistoryIndex(index) => target
                .get("tooling")?
                .get("sourceReferences")?
                .as_array()?
                .get(*index)?
                .get("configurationRef"),
        }
    }

    pub(crate) fn coordinate(&self) -> String {
        match self {
            Self::Decision => "decision/configurationRef".into(),
            Self::MatrixDefault | Self::ReplicaDefault => {
                "decision/replicaSelectionDefault/configurationRef".into()
            }
            Self::HistoryKey(key) => format!(
                "state/codingAgents/{}/configurationRef",
                key.replace('~', "~0").replace('/', "~1")
            ),
            Self::HistoryIndex(index) => format!("state/sourceReferences/{index}/configurationRef"),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SourceDiscoveredTargetEvidence {
    pub(crate) target: SourceInventoryTarget,
    pub(crate) slots: Vec<(SourceOriginalSlot, ConfigurationRef)>,
    pub(crate) affected: bool,
}

pub(crate) struct SourceInventoryCapture {
    pub(crate) discovery: SourceDiscoveryWitness,
    pub(crate) objects: Vec<(PathBuf, (u64, u64))>,
    pub(crate) discovered: Vec<SourceDiscoveredTargetEvidence>,
    pub(crate) topology: SourceMembershipTopology,
    pub(crate) original_links: SourceOriginalProofAggregate,
    pub(crate) classified: bool,
}

impl SourceInventoryCapture {
    pub(crate) fn capture(
        discovery: SourceDiscoveryWitness,
        mut discovered: Vec<SourceDiscoveredTargetEvidence>,
        snapshot: &RawSourceSnapshot,
        origin: &LegacyCompatibilityOrigin,
        physical_source_key: &str,
    ) -> Result<Self, SourcePreparationError> {
        let invalid = || SourcePreparationError::LegacyOriginUnresolved;
        let mut paths = discovery.parents.clone();
        for (root, members) in &discovery.membership {
            paths.push(root.clone());
            paths.extend(members.iter().cloned());
        }
        paths.extend(
            discovered
                .iter()
                .map(|evidence| evidence.target.root.clone()),
        );
        paths.sort();
        paths.dedup();
        let mut objects = Vec::new();
        for path in paths {
            let verified = crate::path_identity::verify_directory(&path).map_err(|_| invalid())?;
            objects.push((
                verified.canonical_path,
                (verified.object_id.volume, verified.object_id.file),
            ));
        }
        objects.sort();
        objects.dedup();
        if objects.windows(2).any(|pair| pair[0].0 == pair[1].0) {
            return Err(invalid());
        }
        for (slot, present) in &discovery.slots {
            if *present {
                let verified =
                    crate::path_identity::verify_directory(slot).map_err(|_| invalid())?;
                if !objects.iter().any(|(path, object)| {
                    path == &verified.canonical_path
                        && *object == (verified.object_id.volume, verified.object_id.file)
                }) {
                    return Err(invalid());
                }
            } else {
                let parent = slot.parent().ok_or_else(invalid)?;
                crate::path_identity::verify_directory(parent).map_err(|_| invalid())?;
                match std::fs::symlink_metadata(slot) {
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    _ => return Err(invalid()),
                }
            }
        }
        discovered.sort_by(|a, b| a.target.decisions.cmp(&b.target.decisions));
        if discovered
            .windows(2)
            .any(|pair| pair[0].target.root == pair[1].target.root)
        {
            return Err(invalid());
        }
        let mut links = Vec::new();
        let mut originals = Vec::new();
        let mut defaults: BTreeMap<
            String,
            (PathBuf, std::collections::BTreeSet<LegacyOriginUnit>),
        > = BTreeMap::new();
        for evidence in &discovered {
            let target = &evidence.target;
            let physical = target.original_physical.as_ref().ok_or_else(invalid)?;
            if identity_reference_protected_revision(physical).map_err(|_| invalid())?
                != target.before_protected
                || !objects
                    .iter()
                    .any(|(root, object)| root == &target.root && object == &target.root_object)
            {
                return Err(invalid());
            }
            for (slot, selection) in &evidence.slots {
                if selection.version != 1 {
                    return Err(invalid());
                }
                if matches!(
                    slot,
                    SourceOriginalSlot::MatrixDefault | SourceOriginalSlot::ReplicaDefault
                ) {
                    let name = target
                        .root
                        .file_name()
                        .and_then(|name| name.to_str())
                        .ok_or_else(invalid)?;
                    let matrix = name.starts_with("_agent_");
                    let replica = name.starts_with("__agent_");
                    if matrix != matches!(slot, SourceOriginalSlot::MatrixDefault)
                        || replica != matches!(slot, SourceOriginalSlot::ReplicaDefault)
                    {
                        return Err(invalid());
                    }
                }
                let side = match slot {
                    SourceOriginalSlot::Decision
                    | SourceOriginalSlot::MatrixDefault
                    | SourceOriginalSlot::ReplicaDefault => 0,
                    _ => 1,
                };
                let bytes = match &physical[side] {
                    IdentityReferencePhysicalState::Bytes { bytes, .. } => bytes,
                    _ => return Err(invalid()),
                };
                let raw: Value = serde_json::from_slice(bytes).map_err(|_| invalid())?;
                let stored: ConfigurationRef =
                    serde_json::from_value(slot.stored(&raw).cloned().ok_or_else(invalid)?)
                        .map_err(|_| invalid())?;
                if stored != *selection {
                    return Err(invalid());
                }
                if selection.source != snapshot.snapshot.source {
                    continue;
                }
                links.push((
                    target.root.clone(),
                    slot.coordinate(),
                    selection.source.clone(),
                    physical_source_key.into(),
                ));
                if snapshot.snapshot.source.kind != SourceKind::RegisteredInstance {
                    continue;
                }
                let matrix = if matches!(slot, SourceOriginalSlot::MatrixDefault) {
                    // Only a real origin Matrix can own N. Replica/history never mint Default.
                    let name = target
                        .root
                        .file_name()
                        .and_then(|name| name.to_str())
                        .ok_or_else(invalid)?;
                    Some(
                        name.strip_prefix("_agent_")
                            .filter(|name| !name.is_empty())
                            .ok_or_else(invalid)?,
                    )
                } else {
                    None
                };
                let proof = origin.bind_original_selection_at(
                    snapshot,
                    selection,
                    &snapshot.snapshot.revision,
                    &snapshot.snapshot.context_revision,
                    bytes,
                    bytes,
                    matrix,
                    slot,
                )?;
                if let Some(matrix) = matrix {
                    let provenance = (target.root.clone(), proof.units.clone());
                    if defaults
                        .get(matrix)
                        .is_some_and(|original| original != &provenance)
                    {
                        return Err(invalid());
                    }
                    defaults.insert(matrix.into(), provenance);
                }
                originals.push(proof);
            }
        }
        let topology = SourceMembershipTopology {
            registry: discovery.registry.clone(),
            directories: objects.clone(),
            slots: discovery.slots.clone(),
            membership: discovery.membership.clone(),
            targets: discovered
                .iter()
                .map(|e| {
                    let t = &e.target;
                    (
                        t.root.clone(),
                        t.root_object,
                        t.decisions.clone(),
                        t.state.clone(),
                        t.reservation.clone(),
                    )
                })
                .collect(),
            links,
        };
        Ok(Self {
            discovery,
            objects,
            discovered,
            topology,
            original_links: SourceOriginalProofAggregate { originals },
            classified: false,
        })
    }

    pub(crate) fn original_links(&self) -> &SourceOriginalProofAggregate {
        &self.original_links
    }
}

pub(crate) type SourceOriginalSlotParser<'a> = dyn Fn(
        &SourceInventoryTarget,
    ) -> Result<Vec<(SourceOriginalSlot, ConfigurationRef)>, IdentityReferenceError>
    + 'a;

pub(crate) struct SourceInventoryInputs<'borrow, 'identity> {
    pub(crate) settings: &'borrow AppSettings,
    pub(crate) context: &'borrow SourceReadContext,
    pub(crate) identity: &'borrow IdentityWriteLease,
    pub(crate) lease: &'borrow SourceWriteContext<'identity>,
}

/// Backend-only original operation authority. No retained discovery handles.
pub(crate) struct SourceInventoryProof {
    pub(crate) operation_id: String,
    pub(crate) inventory: SourceLocalInventory,
}

impl SourceInventoryProof {
    pub(crate) fn for_candidate(
        inputs: SourceInventoryInputs<'_, '_>,
        snapshot: &RawSourceSnapshot,
        prepared: &PreparedSourceCandidate,
        origin: &OriginInputs,
        capture: SourceInventoryCapture,
        operation_id: &str,
    ) -> Result<Self, SourcePreparationError> {
        let SourceInventoryInputs {
            settings,
            context,
            identity,
            lease,
        } = inputs;
        if !capture.classified
            || SourceReadContext::from_settings(settings, &context.settings_path)? != *context
            || capture.discovery.registry != settings.project_paths
            || prepared.source != snapshot.snapshot.source
            || prepared.source != *lease.source()
        {
            return Err(SourcePreparationError::ContextChanged);
        }
        prepared.revalidate_before_write(context)?;
        origin.revalidate(context)?;
        let inventory_digest = source_inventory_digest(
            identity.instance_id(),
            lease.source(),
            lease.physical_source_key(),
            lease.context_revision(),
            &capture.topology,
        )
        .map_err(|_| SourcePreparationError::ContextChanged)?;
        let mappings = source_original_mappings(snapshot, prepared)?;
        let source_before = match &prepared.before_bytes {
            Some(bytes) => IdentityReferencePhysicalState::from_bytes(bytes.clone()),
            None => IdentityReferencePhysicalState::Absent,
        };
        let evidence = SourceOriginalInventoryEvidence {
            discovery: capture.discovery,
            objects: capture.objects.clone(),
            source_before,
            expected_source: prepared.expected_revision.clone(),
            after_source: source_bytes_revision(&prepared.serialized_after),
            discovered: capture.discovered.clone(),
            topology: capture.topology,
            inventory_digest,
        };
        let mut targets = Vec::new();
        for classified in &capture.discovered {
            if classified.affected {
                let mut target = classified.target.clone();
                // Original images have one durable location, in full evidence.
                target.original_physical = None;
                targets.push(target);
            }
        }
        let mut inventory = SourceLocalInventory {
            editor_id: identity.instance_id().into(),
            source: prepared.source.clone(),
            physical_source_key: lease.physical_source_key().into(),
            context_revision: context.revision.clone(),
            authorization_revision: String::new(),
            authorized_roots: capture.objects,
            targets,
            mappings,
            origin_binding: Some(origin.binding.clone()),
            original_evidence: Some(evidence),
        };
        inventory.authorization_revision = source_transition_digest(&inventory)
            .map_err(|_| SourcePreparationError::ContextChanged)?;
        let proof = Self {
            operation_id: operation_id.into(),
            inventory,
        };
        proof
            .validate(identity, lease, context)
            .map_err(|_| SourcePreparationError::ContextChanged)?;
        Ok(proof)
    }

    pub(crate) fn inventory(&self) -> &SourceLocalInventory {
        &self.inventory
    }

    pub(crate) fn inventory_digest(&self) -> Result<&str, SourceCoordinationError> {
        let original = self
            .inventory
            .original_evidence
            .as_ref()
            .ok_or(SourceCoordinationError::Invalid)?;
        let actual = source_inventory_digest(
            &self.inventory.editor_id,
            &self.inventory.source,
            &self.inventory.physical_source_key,
            &self.inventory.context_revision,
            &original.topology,
        )?;
        if actual != original.inventory_digest {
            return Err(SourceCoordinationError::Invalid);
        }
        Ok(&original.inventory_digest)
    }

    pub(crate) fn stored_intent(
        &self,
        prepared: &PreparedSourceCandidate,
        operation_id: &str,
        request_id: &str,
        payload_digest: &str,
    ) -> Result<SourceStoredIntent, SourceCoordinationError> {
        let original = self
            .inventory
            .original_evidence
            .as_ref()
            .ok_or(SourceCoordinationError::Invalid)?;
        if operation_id != self.operation_id
            || prepared.source != self.inventory.source
            || prepared.expected_revision != original.expected_source
            || source_bytes_revision(&prepared.serialized_after) != original.after_source
        {
            return Err(SourceCoordinationError::Invalid);
        }
        Ok(SourceStoredIntent {
            schema_version: 2,
            operation_id: operation_id.into(),
            request_id: request_id.into(),
            payload_digest: payload_digest.into(),
            source: self.inventory.source.clone(),
            expected_revision: prepared.expected_revision.clone(),
            before_bytes: prepared.before_bytes.clone(),
            serialized_after: prepared.serialized_after.clone(),
            mappings: self.inventory.mappings.clone(),
            editor_id: self.inventory.editor_id.clone(),
            context_revision: self.inventory.context_revision.clone(),
            authorization_revision: self.inventory.authorization_revision.clone(),
        })
    }

    /// Pure segment guard: directory/object and original origin checks only.
    /// Full enumeration is orchestrated outside PAIR by mutations.
    pub(crate) fn validate(
        &self,
        identity: &IdentityWriteLease,
        lease: &SourceWriteContext,
        context: &SourceReadContext,
    ) -> Result<(), IdentityReferenceError> {
        #[cfg(test)]
        source_factory_guard_cut()?;
        let conflict = || IdentityReferenceError::Conflict;
        let inventory = &self.inventory;
        let original = inventory.original_evidence.as_ref().ok_or_else(conflict)?;
        if identity.instance_id() != inventory.editor_id
            || lease.source() != &inventory.source
            || lease.physical_source_key() != inventory.physical_source_key
            || context.revision != inventory.context_revision
        {
            return Err(conflict());
        }
        lease.revalidate(context).map_err(|_| conflict())?;
        for (path, object) in &original.objects {
            let current = crate::path_identity::verify_directory(path).map_err(|_| conflict())?;
            if current.canonical_path != *path
                || (current.object_id.volume, current.object_id.file) != *object
            {
                return Err(conflict());
            }
        }
        for (slot, present) in &original.discovery.slots {
            let exists = match std::fs::symlink_metadata(slot) {
                Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => true,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
                _ => return Err(conflict()),
            };
            if exists != *present {
                return Err(conflict());
            }
        }
        let observed = OriginInputs::read(context);
        let binding = inventory.origin_binding.as_ref().ok_or_else(conflict)?;
        if observed.binding.context != binding.context
            || observed.binding.settings != binding.settings
            || observed.binding.overlay != binding.overlay
        {
            return Err(conflict());
        }
        let current_source = lease.source_revision().map_err(|_| conflict())?;
        if current_source != original.expected_source && current_source != original.after_source {
            return Err(conflict());
        }
        if current_source != original.expected_source {
            let (_, journal) = read_own_source_journal(context).map_err(|_| conflict())?;
            let intent = journal
                .intents
                .get(&self.operation_id)
                .ok_or_else(conflict)?;
            let progress = journal
                .progress
                .get(&self.operation_id)
                .ok_or_else(conflict)?;
            if !progress.packets_prepared
                || intent.source != inventory.source
                || intent.expected_revision != original.expected_source
                || source_bytes_revision(&intent.serialized_after) != original.after_source
                || intent.mappings != inventory.mappings
                || intent.editor_id != identity.instance_id()
            {
                return Err(conflict());
            }
        }
        if observed.binding.registered != binding.registered {
            if inventory.source.kind != SourceKind::RegisteredInstance
                || current_source != original.after_source
                || observed.binding.registered.as_ref() != Some(&original.after_source)
            {
                return Err(conflict());
            }
            let (_, journal) = read_own_source_journal(context).map_err(|_| conflict())?;
            let intent = journal
                .intents
                .get(&self.operation_id)
                .ok_or_else(conflict)?;
            if intent.source != inventory.source
                || intent.expected_revision != original.expected_source
                || source_bytes_revision(&intent.serialized_after) != original.after_source
                || intent.mappings != inventory.mappings
                || intent.editor_id != identity.instance_id()
            {
                return Err(conflict());
            }
        }
        Ok(())
    }
}

impl SourceInventoryCapture {
    pub(crate) fn same_original_capture(&self, other: &Self) -> bool {
        self.discovery == other.discovery
            && self.objects == other.objects
            && self.discovered == other.discovered
            && self.topology == other.topology
    }

    pub(crate) fn original_targets(&self) -> &[SourceDiscoveredTargetEvidence] {
        &self.discovered
    }

    pub(crate) fn classify(
        &mut self,
        classifications: Vec<(PathBuf, bool)>,
    ) -> Result<(), SourcePreparationError> {
        if classifications.len() != self.discovered.len() {
            return Err(SourcePreparationError::LegacyOriginUnresolved);
        }
        for (evidence, (target, affected)) in self.discovered.iter_mut().zip(classifications) {
            if evidence.target.decisions != target {
                return Err(SourcePreparationError::LegacyOriginUnresolved);
            }
            evidence.affected = affected;
        }
        self.classified = true;
        Ok(())
    }
}

impl SourceLocalInventory {
    pub(crate) fn validate_original_evidence(&self) -> Result<(), SourceCoordinationError> {
        let Some(original) = &self.original_evidence else {
            return Ok(());
        };
        let invalid = || SourceCoordinationError::Invalid;
        let mut unsigned = self.clone();
        unsigned.authorization_revision.clear();
        if source_transition_digest(&unsigned)? != self.authorization_revision
            || original.objects != self.authorized_roots
            || original.topology.directories != original.objects
            || original.topology.registry != original.discovery.registry
            || original.topology.slots != original.discovery.slots
            || original.topology.membership != original.discovery.membership
            || self.origin_binding.is_none()
        {
            return Err(invalid());
        }
        let before = match &original.source_before {
            IdentityReferencePhysicalState::Absent => SourceRevision::Absent {},
            IdentityReferencePhysicalState::Bytes { bytes, .. } => {
                if IdentityReferencePhysicalState::from_bytes(bytes.clone())
                    != original.source_before
                {
                    return Err(invalid());
                }
                source_bytes_revision(bytes)
            }
        };
        if before != original.expected_source
            || source_inventory_digest(
                &self.editor_id,
                &self.source,
                &self.physical_source_key,
                &self.context_revision,
                &original.topology,
            )? != original.inventory_digest
            || original
                .discovered
                .windows(2)
                .any(|pair| pair[0].target.decisions >= pair[1].target.decisions)
        {
            return Err(invalid());
        }
        let mut selected = Vec::new();
        let mut targets = Vec::new();
        let mut links = Vec::new();
        for evidence in &original.discovered {
            let target = &evidence.target;
            let physical = target.original_physical.as_ref().ok_or_else(invalid)?;
            for image in physical {
                if let IdentityReferencePhysicalState::Bytes { bytes, .. } = image {
                    if &IdentityReferencePhysicalState::from_bytes(bytes.clone()) != image {
                        return Err(invalid());
                    }
                }
            }
            if identity_reference_protected_revision(physical).map_err(|_| invalid())?
                != target.before_protected
                || !original
                    .objects
                    .iter()
                    .any(|(path, object)| path == &target.root && object == &target.root_object)
                || target.original_selections
                    != evidence
                        .slots
                        .iter()
                        .map(|(_, selection)| selection.clone())
                        .collect::<Vec<_>>()
            {
                return Err(invalid());
            }
            targets.push((
                target.root.clone(),
                target.root_object,
                target.decisions.clone(),
                target.state.clone(),
                target.reservation.clone(),
            ));
            for (slot, selection) in &evidence.slots {
                if selection.source == self.source {
                    links.push((
                        target.root.clone(),
                        slot.coordinate(),
                        selection.source.clone(),
                        self.physical_source_key.clone(),
                    ));
                }
            }
            if evidence.affected {
                let mut target = target.clone();
                target.original_physical = None;
                selected.push(target);
            }
        }
        if targets != original.topology.targets
            || links != original.topology.links
            || selected != self.targets
        {
            return Err(invalid());
        }
        Ok(())
    }

    pub(crate) fn verify_original_slots(
        &self,
        parse: &SourceOriginalSlotParser<'_>,
    ) -> Result<(), IdentityReferenceError> {
        let original = self
            .original_evidence
            .as_ref()
            .ok_or(IdentityReferenceError::Conflict)?;
        for evidence in &original.discovered {
            let slots = parse(&evidence.target)?;
            if slots != evidence.slots
                || slots
                    .iter()
                    .map(|(_, reference)| reference.clone())
                    .collect::<Vec<_>>()
                    != evidence.target.original_selections
            {
                return Err(IdentityReferenceError::Conflict);
            }
        }
        Ok(())
    }

    /// Shared three-outcome reader: Ok(true) matches, Ok(false) is definite
    /// protected mismatch, Err is uncertainty/invalid authority, never stale.
    pub(crate) fn affected_original_protected_matches(
        &self,
    ) -> Result<bool, SourceCoordinationError> {
        self.validate_original_evidence()?;
        let original = self
            .original_evidence
            .as_ref()
            .ok_or(SourceCoordinationError::Invalid)?;
        let mut matches = true;
        for evidence in original
            .discovered
            .iter()
            .filter(|evidence| evidence.affected)
        {
            let target = &evidence.target;
            let identity = crate::path_identity::verify_directory(&target.root)
                .map_err(|_| SourceCoordinationError::IdentityUncertain)?;
            if (identity.object_id.volume, identity.object_id.file) != target.root_object {
                return Err(SourceCoordinationError::IdentityUncertain);
            }
            let physical = [
                read_identity_reference_physical(&target.decisions)
                    .map_err(|_| SourceCoordinationError::IdentityUncertain)?,
                read_identity_reference_physical(&target.state)
                    .map_err(|_| SourceCoordinationError::IdentityUncertain)?,
            ];
            let protected = identity_reference_protected_revision(&physical)
                .map_err(|_| SourceCoordinationError::IdentityUncertain)?;
            matches &= protected == target.before_protected;
        }
        Ok(matches)
    }
}

impl SourceInventoryProof {
    pub(crate) fn for_resume(
        inputs: SourceInventoryInputs<'_, '_>,
        operation_id: &str,
        journal: &SourcePrivateJournal,
        parse_slots: &SourceOriginalSlotParser<'_>,
    ) -> Result<Self, SourceCoordinationError> {
        let SourceInventoryInputs {
            settings,
            context,
            identity,
            lease,
        } = inputs;
        let inventory = journal
            .inventories
            .get(operation_id)
            .ok_or(SourceCoordinationError::Invalid)?
            .clone();
        // Recompute from original physical images BEFORE restoring any proof.
        inventory
            .verify_original_slots(parse_slots)
            .map_err(|_| SourceCoordinationError::Invalid)?;
        let original = inventory
            .original_evidence
            .as_ref()
            .ok_or(SourceCoordinationError::Invalid)?;
        if original.discovery.registry != settings.project_paths
            || SourceReadContext::from_settings(settings, &context.settings_path)
                .map_err(|_| SourceCoordinationError::Unauthorized)?
                != *context
        {
            return Err(SourceCoordinationError::Unauthorized);
        }
        let mut unsigned = inventory.clone();
        unsigned.authorization_revision.clear();
        if source_transition_digest(&unsigned)? != inventory.authorization_revision {
            return Err(SourceCoordinationError::Invalid);
        }
        let intent = journal
            .intents
            .get(operation_id)
            .ok_or(SourceCoordinationError::Invalid)?;
        if intent.editor_id != identity.instance_id()
            || intent.authorization_revision != inventory.authorization_revision
            || intent.expected_revision != original.expected_source
            || source_bytes_revision(&intent.serialized_after) != original.after_source
            || intent.source != inventory.source
            || intent.mappings != inventory.mappings
        {
            return Err(SourceCoordinationError::Invalid);
        }
        let proof = Self {
            operation_id: operation_id.into(),
            inventory,
        };
        proof.inventory_digest()?;
        proof
            .validate(identity, lease, context)
            .map_err(|_| SourceCoordinationError::Stale)?;
        proof.restore_original_links(context)?;
        Ok(proof)
    }

    /// Replay the original pure patch over every discovered before image.
    /// The callback is supplied by the orchestration owner, outside PAIR guards.
    pub(crate) fn verify_original_classification(
        &self,
        classify: &dyn Fn(&SourceInventoryTarget) -> Result<bool, IdentityReferenceError>,
    ) -> Result<(), IdentityReferenceError> {
        let original = self
            .inventory
            .original_evidence
            .as_ref()
            .ok_or(IdentityReferenceError::Conflict)?;
        let mut selected = Vec::new();
        for evidence in &original.discovered {
            if classify(&evidence.target)? != evidence.affected {
                return Err(IdentityReferenceError::Conflict);
            }
            if evidence.affected {
                let mut target = evidence.target.clone();
                target.original_physical = None;
                selected.push(target);
            }
        }
        if selected != self.inventory.targets {
            return Err(IdentityReferenceError::Conflict);
        }
        Ok(())
    }

    /// Fence-only invariant; later PAIR progress legitimately changes revisions.
    pub(crate) fn verify_affected_before_fence(&self) -> Result<(), SourceCoordinationError> {
        if self.inventory.affected_original_protected_matches()? {
            Ok(())
        } else {
            Err(SourceCoordinationError::Stale)
        }
    }

    pub(crate) fn verify_complete_discovery(
        &self,
        witness: &SourceDiscoveryWitness,
        current: &[SourceDiscoveredTargetEvidence],
    ) -> Result<(), SourceCoordinationError> {
        let original = self
            .inventory
            .original_evidence
            .as_ref()
            .ok_or(SourceCoordinationError::Invalid)?;
        if witness != &original.discovery || current.len() != original.discovered.len() {
            return Err(SourceCoordinationError::Stale);
        }
        let mut links = Vec::new();
        for (now_evidence, before_evidence) in current.iter().zip(&original.discovered) {
            if !before_evidence.affected
                && now_evidence.target.before_protected != before_evidence.target.before_protected
            {
                return Err(SourceCoordinationError::Stale);
            }
        }
        for (now, before) in current.iter().zip(&original.discovered) {
            let now = &now.target;
            let before = &before.target;
            if now.root != before.root
                || now.root_object != before.root_object
                || now.decisions != before.decisions
                || now.state != before.state
                || now.reservation != before.reservation
            {
                return Err(SourceCoordinationError::Stale);
            }
        }
        for evidence in current {
            for (slot, selection) in &evidence.slots {
                if selection.source == self.inventory.source {
                    links.push((
                        evidence.target.root.clone(),
                        slot.coordinate(),
                        selection.source.clone(),
                        self.inventory.physical_source_key.clone(),
                    ));
                }
            }
        }
        let mut topology = original.topology.clone();
        topology.links = links;
        if source_inventory_digest(
            &self.inventory.editor_id,
            &self.inventory.source,
            &self.inventory.physical_source_key,
            &self.inventory.context_revision,
            &topology,
        )? != original.inventory_digest
        {
            return Err(SourceCoordinationError::Stale);
        }
        Ok(())
    }
}

#[cfg(test)]
thread_local! {
    pub(crate) static SOURCE_FACTORY_GUARD_CUT: std::cell::Cell<(usize, Option<usize>)> =
        const { std::cell::Cell::new((0, None)) };
}

#[cfg(test)]
pub(crate) fn configure_source_factory_guard_cut(fail_at: Option<usize>) {
    SOURCE_FACTORY_GUARD_CUT.with(|state| state.set((0, fail_at)));
}

#[cfg(test)]
pub(crate) fn source_factory_guard_calls() -> usize {
    SOURCE_FACTORY_GUARD_CUT.with(|state| state.get().0)
}

#[cfg(test)]
pub(crate) fn source_factory_guard_cut() -> Result<(), IdentityReferenceError> {
    SOURCE_FACTORY_GUARD_CUT.with(|state| {
        let (count, fail_at) = state.get();
        let count = count + 1;
        if fail_at == Some(count) {
            state.set((count, None));
            return Err(IdentityReferenceError::Conflict);
        }
        state.set((count, fail_at));
        Ok(())
    })
}

impl SourceInventoryProof {
    /// Rebuild original proof groups from the SAME saved before source and target
    /// images, after current source/packet authority has been independently checked.
    pub(crate) fn restore_original_links(
        &self,
        context: &SourceReadContext,
    ) -> Result<(), SourceCoordinationError> {
        let original = self
            .inventory
            .original_evidence
            .as_ref()
            .ok_or(SourceCoordinationError::Invalid)?;
        let raw = match &original.source_before {
            IdentityReferencePhysicalState::Absent => OriginPhysicalInput::Absent,
            IdentityReferencePhysicalState::Bytes { bytes, .. } => {
                if IdentityReferencePhysicalState::from_bytes(bytes.clone())
                    != original.source_before
                {
                    return Err(SourceCoordinationError::Invalid);
                }
                OriginPhysicalInput::Bytes(bytes.clone())
            }
        };
        if raw.revision().as_ref() != Some(&original.expected_source) {
            return Err(SourceCoordinationError::Invalid);
        }
        let binding = self
            .inventory
            .origin_binding
            .as_ref()
            .ok_or(SourceCoordinationError::Invalid)?;
        let mut inputs = OriginInputs::read(context);
        if inputs.binding.settings != binding.settings
            || inputs.binding.overlay != binding.overlay
            || inputs.binding.context != binding.context
        {
            return Err(SourceCoordinationError::Stale);
        }
        if self.inventory.source.kind == SourceKind::RegisteredInstance {
            if binding.registered.as_ref() != Some(&original.expected_source) {
                return Err(SourceCoordinationError::Invalid);
            }
            inputs.registered = raw.clone();
        } else if inputs.binding.registered != binding.registered {
            return Err(SourceCoordinationError::Stale);
        }
        inputs.binding = binding.clone();
        let origin = LegacyCompatibilityOrigin::classify(&inputs);
        let snapshot = source_snapshot_from_original(context, &self.inventory.source, raw)
            .map_err(|_| SourceCoordinationError::Invalid)?;
        let captured = SourceInventoryCapture::capture(
            original.discovery.clone(),
            original.discovered.clone(),
            &snapshot,
            &origin,
            &self.inventory.physical_source_key,
        )
        .map_err(|_| SourceCoordinationError::Invalid)?;
        if captured.objects != original.objects || captured.topology != original.topology {
            return Err(SourceCoordinationError::Stale);
        }
        let selected: Vec<_> = original
            .discovered
            .iter()
            .filter(|target| target.affected)
            .map(|record| {
                let mut target = record.target.clone();
                target.original_physical = None;
                target
            })
            .collect();
        if selected != self.inventory.targets {
            return Err(SourceCoordinationError::Invalid);
        }
        Ok(())
    }
}

#[cfg(test)]
pub(crate) fn source_factory_guard_cut_armed() -> bool {
    SOURCE_FACTORY_GUARD_CUT.with(|state| state.get().1.is_some())
}

#[cfg(test)]
pub(crate) fn source_transition_test_root() -> tempfile::TempDir {
    let scratch = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/i2897-p03");
    std::fs::create_dir_all(&scratch).unwrap();
    tempfile::Builder::new()
        .prefix("source-transition-")
        .tempdir_in(scratch)
        .unwrap()
}

#[cfg(test)]
pub(crate) fn source_transition_test_locator(
    context: &SourceReadContext,
    source: &SourceRef,
) -> Result<PathBuf, SourcePreparationError> {
    context.locator(source)
}
