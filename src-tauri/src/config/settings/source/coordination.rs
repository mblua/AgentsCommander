use super::super::IdentityReferenceError;
use super::super::IdentityReferencePhysicalState;
use super::super::IdentityReferenceProtectedRevision;
use super::identity::source_transition_digest;
use super::read::OriginBinding;
use super::read::SourceReadContext;
use super::source_bytes_revision;
use super::ConfigurationRef;
use super::SourceCoordinationError;
use super::SourceKind;
use super::SourceOriginalInventoryEvidence;
use super::SourceRef;
use super::SourceRevision;
use crate::filesystem::settings_sources;
use serde::Deserialize;
use serde::Serialize;
use std::collections::BTreeMap;
use std::collections::HashSet;
use std::path::Path;
use std::path::PathBuf;

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
    let parent = settings_sources::canonical(
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
    match settings_sources::probe(path) {
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
    match settings_sources::probe(path) {
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
