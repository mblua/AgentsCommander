use super::{
    IdentityReferenceCompleteProof, IdentityReferenceError, IdentityReferencePhysicalState,
    IdentityReferenceProtectedRevision, ProfileCellConfig,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

mod coordination;
mod identity;
mod inventory;
mod prepare;
mod proof;
mod publication;
mod read;

pub(crate) use coordination::read_own_source_journal;
pub(crate) use coordination::read_source_transition;
#[cfg(test)]
pub(crate) use coordination::source_transition_effect_counts;
#[cfg(test)]
pub(crate) use coordination::source_transition_publication_counts;
pub(crate) use coordination::IdentityWriteLease;
pub(crate) use coordination::SourceDiscoveryWitness;
pub(crate) use coordination::SourceIdentityMapping;
pub(crate) use coordination::SourceInventoryTarget;
#[cfg(test)]
pub(crate) use coordination::SourceLocalInventory;
pub(crate) use coordination::SourceLocalProgress;
pub(crate) use coordination::SourcePacketRecord;
pub(crate) use coordination::SourceReceipt;
pub(crate) use coordination::SourceTransitionPhase;
pub(crate) use coordination::SourceTransitionStatus;
pub(crate) use coordination::SourceWriteContext;
pub use identity::configuration_identity;
pub(crate) use identity::configuration_identity_debug;
#[cfg(test)]
pub(crate) use identity::identity_digest;
pub use identity::identity_persisted_name;
pub(crate) use identity::pair_identity;
pub(crate) use identity::pair_identity_debug;
pub(crate) use identity::physical_identity_violations;
pub use identity::profile_identity;
pub use identity::refresh_agent_id_hashes;
pub(crate) use identity::refresh_pair_id_hashes;
pub(crate) use identity::refresh_physical_pair_id_hashes;
pub(crate) use identity::source_transition_digest;
pub use identity::validate_identity_snapshot;
#[cfg(test)]
pub(crate) use identity::validate_identity_snapshot_with_digest;
pub(crate) use identity::validate_physical_identity_delta;
pub use identity::IdentitySnapshotEntry;
pub use identity::IdentityValidationCode;
pub use identity::IdentityValidationError;
pub(crate) use inventory::recover_source_target_packet;
#[cfg(test)]
pub(crate) use inventory::source_inventory_digest;
#[cfg(test)]
pub(crate) use inventory::source_inventory_projection;
pub(crate) use inventory::source_inventory_target;
pub(crate) use inventory::source_original_mappings;
pub(crate) use inventory::verify_completed_source_target;
pub(crate) use inventory::SourceDiscoveredTargetEvidence;
pub(crate) use inventory::SourceInventoryCapture;
pub(crate) use inventory::SourceInventoryInputs;
pub(crate) use inventory::SourceInventoryProof;
pub(crate) use inventory::SourceMembershipTopology;
#[cfg(test)]
pub(crate) use prepare::prepare_settings_without_source_write;
#[cfg(test)]
pub(crate) use prepare::prepare_source_candidate;
pub(crate) use prepare::prepare_source_candidate_with_original_links;
pub(crate) use prepare::raw_entries;
#[cfg(test)]
pub(crate) use prepare::require_source_original_units;
pub(crate) use prepare::source_general_field_names;
#[cfg(test)]
pub(crate) use prepare::PreparedSourceCandidate;
#[cfg(test)]
pub(crate) use prepare::SourceOriginalProofAggregate;
#[cfg(test)]
pub(crate) use prepare::SOURCE_GENERAL_PROTECTED;
pub(crate) use publication::publish_source_intent_with_validation;
#[cfg(test)]
pub(crate) use publication::read_own_source_intent;
pub(crate) use publication::source_complete_proof;
pub(crate) use publication::update_own_source_journal_with_validation;
pub(crate) use read::decode_registered_raw;
pub(crate) use read::list_active_sources;
pub(crate) use read::read_source_snapshot;
pub(crate) use read::source_diagnostic;
pub(crate) use read::source_snapshot_from_original;
pub(crate) use read::LegacyCompatibilityOrigin;
#[cfg(test)]
pub(crate) use read::LegacyOriginUnit;
pub(crate) use read::OriginInputs;
pub(crate) use read::RawSourceSnapshot;
pub(crate) use read::SourceReadContext;

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
