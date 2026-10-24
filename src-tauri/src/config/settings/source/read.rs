use super::super::agents_instance_groups;
use super::super::agents_instance_path;
use super::super::apply_project_decode_to_value;
use super::super::decode_physical_catalog;
use super::super::duplicate_profile_document;
use super::super::is_valid_profile_letter;
use super::super::production_instance_base;
use super::super::project_physical_profiles;
use super::super::refresh_physical_source_rows;
use super::super::registered_profile_raw_gate;
use super::super::AgentConfig;
use super::super::AppSettings;
use super::super::CodingAgentProfilesConfig;
use super::super::CONFIGURATION_COMMAND_PAIRS;
use super::super::OVERLAY_DERIVED_ID_CLOSURES;
use super::super::OVERLAY_INELIGIBLE_DISK_KEYS;
use super::super::OVERLAY_INELIGIBLE_LEGACY_KEYS;
use super::identity::configuration_identity;
use super::identity::profile_identity;
use super::identity::refresh_agent_id_hashes;
use super::identity::refresh_pair_id_hashes;
use super::identity::validate_identity_snapshot;
use super::identity::IdentitySnapshotEntry;
use super::source_bytes_revision;
use super::ConfigurationRef;
use super::OriginPhysicalInput;
use super::SourceAvailability;
use super::SourceCapabilities;
use super::SourceDiagnostic;
use super::SourceFamily;
use super::SourceKind;
use super::SourceOriginalSlot;
use super::SourcePreparationError;
use super::SourceRef;
use super::SourceRevision;
use super::SourceRole;
use super::SourceScope;
use super::SourceSnapshot;
use crate::config::instance_artifacts::SETTINGS_FILE_NAME;
use crate::config::instance_artifacts::SETTINGS_LOCAL_OVERRIDE_FILE_NAME;
use crate::config::local_overlay::LocalSettingsOverlay;
use crate::config::settings::project_path as projects;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::collections::HashSet;
use std::path::Path;
use std::path::PathBuf;

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
