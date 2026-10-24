use super::super::super::{
    capture_source_file, decode_project_state, io_error_kind_as_str, list_active_sources,
    production_instance_base, project_paths_path, raw_project_source_from_bytes,
    select_project_source_from_raw, source_bytes_revision, source_diagnostic, AppSettings,
    CapturedSource, OperationEffect, SettingsOperationGuard, SettingsOwner, SettingsOwnerError,
    SettingsPublisher, SourceAvailability, SourceCapabilities, SourceKind, SourceReadContext,
    SourceRef, SourceRevision, SourceScope, SETTINGS_CAPTURE_LIMIT,
};
use super::super::{
    agents_instance_path, classify_registered_profile_inputs, RegisteredProfileCapture,
};
use super::{
    agent_file_sources_reply, prepare_agent_file_edits, snapshot_from_captured_source,
    AgentFileEdits, AgentFileSaveOutcome, AgentFileSaveReply, AgentFileSourceOption,
    AgentFileSourcePreferences, AgentFileSources, SelectedAgentFileSnapshot,
};
use crate::config::instance_artifacts::SETTINGS_LOCAL_OVERRIDE_FILE_NAME;
use crate::config::settings::project_path as projects;
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};
use std::sync::Arc;

// Source diagnostics retain bounded observations; Failure never means absence.
#[derive(Debug, Clone)]
pub(crate) enum BoundedSourceObservation {
    Absent,
    Bytes {
        bytes: Vec<u8>,
        witness: crate::path_identity::VerifiedPathIdentity,
    },
    Failure(String),
}

impl BoundedSourceObservation {
    pub(crate) fn bytes(&self) -> Option<&[u8]> {
        match self {
            Self::Bytes { bytes, witness } => {
                let _captured_identity = witness;
                Some(bytes)
            }
            _ => None,
        }
    }

    pub(crate) fn profile_input(&self) -> Result<Option<Vec<u8>>, String> {
        match self {
            Self::Absent => Ok(None),
            Self::Bytes { bytes, .. } => Ok(Some(bytes.clone())),
            Self::Failure(kind) => Err(kind.clone()),
        }
    }
}

pub(crate) fn observe_agent_file(path: &Path) -> BoundedSourceObservation {
    agent_file_checkpoint("observe", path);
    match std::fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return BoundedSourceObservation::Absent
        }
        Err(error) => {
            return BoundedSourceObservation::Failure(io_error_kind_as_str(error.kind()).into())
        }
        Ok(_) => {}
    }
    match crate::path_identity::read_bounded_regular(path, SETTINGS_CAPTURE_LIMIT) {
        Ok((bytes, witness)) => BoundedSourceObservation::Bytes { bytes, witness },
        Err(_) => BoundedSourceObservation::Failure("sourceReadFailed".into()),
    }
}

pub(crate) fn same_captured_source(left: &CapturedSource, right: &CapturedSource) -> bool {
    match (left, right) {
        (CapturedSource::Absent, CapturedSource::Absent) => true,
        (
            CapturedSource::Present {
                bytes: a,
                witness: aw,
            },
            CapturedSource::Present {
                bytes: b,
                witness: bw,
            },
        ) => a == b && crate::path_identity::same_object(aw, bw),
        _ => false,
    }
}

// Authorization only: never a composed or adoptable AppSettings projection.
pub(crate) struct AgentFileAuthority {
    pub(crate) base: CapturedSource,
    pub(crate) project_paths: CapturedSource,
    pub(crate) context: SourceReadContext,
    pub(crate) root: crate::path_identity::RetainedDirectory,
    pub(crate) raw_main: Map<String, Value>,
}

impl AgentFileAuthority {
    pub(crate) fn capture(guard: &SettingsOperationGuard<'_>) -> Result<Self, String> {
        guard.lease.verify().map_err(|_| "ownerIntegrityLost")?;
        let path = guard.settings_path();
        let destination = project_paths_path(path);
        let base =
            capture_source_file(path, guard.counters).map_err(|_| "sourceContextUnavailable")?;
        let project_paths = capture_source_file(&destination, guard.counters)
            .map_err(|_| "sourceContextUnavailable")?;
        let legacy = base
            .bytes()
            .map(|bytes| raw_project_source_from_bytes(bytes.to_vec(), path, path, true))
            .transpose()
            .map_err(|_| "sourceContextUnavailable")?;
        let project = project_paths
            .bytes()
            .map(|bytes| {
                raw_project_source_from_bytes(bytes.to_vec(), &destination, &destination, false)
            })
            .transpose()
            .map_err(|_| "sourceContextUnavailable")?;
        let selected =
            select_project_source_from_raw(legacy, project, path, &destination, &destination)
                .map_err(|_| "sourceContextUnavailable")?;
        let raw_main = selected
            .legacy
            .as_ref()
            .map(|source| source.object.clone())
            .unwrap_or_default();
        let decoded = decode_project_state(
            &selected.projection,
            production_instance_base().as_deref(),
            &projects::FsCandidateResolver,
        );
        if decoded.has_structural() {
            return Err("sourceContextUnavailable".into());
        }
        let primary = decoded
            .active_selected()
            .into_iter()
            .next()
            .or(decoded.selected_head)
            .map(PathBuf::from);
        let context = SourceReadContext::from_primary(path, primary)
            .map_err(|_| "sourceContextUnavailable")?;
        let root = crate::path_identity::retain_directory(&context.catalog_root)
            .map_err(|_| "sourceContextUnavailable")?;
        Ok(Self {
            base,
            project_paths,
            context,
            root,
            raw_main,
        })
    }

    pub(crate) fn recheck(&self, guard: &SettingsOperationGuard<'_>) -> Result<(), String> {
        guard.lease.verify().map_err(|_| "ownerIntegrityLost")?;
        self.root
            .verify_current()
            .map_err(|_| "staleSourceContext")?;
        let current = Self::capture(guard)?;
        if self.context != current.context
            || !same_captured_source(&self.base, &current.base)
            || !same_captured_source(&self.project_paths, &current.project_paths)
        {
            return Err("staleSourceContext".into());
        }
        Ok(())
    }

    pub(crate) fn preferences(&self) -> Result<AgentFileSourcePreferences, String> {
        self.raw_main
            .get("agentFileSourceByMenu")
            .cloned()
            .map(serde_json::from_value)
            .transpose()
            .map_err(|_| "invalidSourcePreferences".into())
            .map(|value| value.unwrap_or_default())
    }

    pub(crate) fn registered_validation(
        &self,
    ) -> (BoundedSourceObservation, RegisteredProfileCapture) {
        let path = &self.context.settings_path;
        let registered = observe_agent_file(&agents_instance_path(path));
        let overlay_path = path.with_file_name(SETTINGS_LOCAL_OVERRIDE_FILE_NAME);
        let overlay = observe_agent_file(&overlay_path);
        let inputs = vec![
            (path.clone(), Ok(self.base.bytes().map(<[u8]>::to_vec))),
            (agents_instance_path(path), registered.profile_input()),
            (overlay_path, overlay.profile_input()),
        ];
        (registered, classify_registered_profile_inputs(path, inputs))
    }
}

pub(crate) enum AgentFileReadRequest {
    List,
    Read(SourceRef),
}
pub(crate) enum AgentFileReadReply {
    List(AgentFileSources),
    Read(SelectedAgentFileSnapshot),
}

impl SettingsOwner {
    pub(crate) async fn read_agent_files(
        self: &Arc<Self>,
        request: AgentFileReadRequest,
    ) -> Result<AgentFileReadReply, String> {
        let owner = Arc::clone(self);
        tokio::task::spawn_blocking(move || owner.read_agent_files_blocking(request))
            .await
            .map_err(|_| "sourceWorkerFailed")?
    }

    pub(crate) fn read_agent_files_blocking(
        &self,
        request: AgentFileReadRequest,
    ) -> Result<AgentFileReadReply, String> {
        let _admission = self
            .admit_operation()
            .map_err(|_| "settingsOwnerUnavailable")?;
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _state = self.settings.blocking_write();
            let lease = self.lease_guard();
            let lease = lease.as_ref().ok_or("settingsOwnerUnavailable")?;
            let guard = self
                .operation_guard(lease)
                .map_err(|_| "ownerIntegrityLost")?;
            self.revalidate().map_err(|_| "settingsOwnerUnavailable")?;
            let authority = AgentFileAuthority::capture(&guard)?;
            let preferences = authority.preferences()?;
            let (registered, validation) = authority.registered_validation();
            agent_file_checkpoint("diagnostic_captured", guard.settings_path());
            let snapshot = |source: &SourceRef| -> Result<SelectedAgentFileSnapshot, String> {
                let registered_source = source.kind == SourceKind::RegisteredInstance;
                let path = authority
                    .context
                    .locator(source)
                    .map_err(|_| "sourceContextUnavailable")?;
                let observed = if registered_source {
                    registered.clone()
                } else {
                    observe_agent_file(&path)
                };
                let mut result = snapshot_from_captured_source(
                    &authority.context,
                    source,
                    &observed,
                    registered_source.then_some(&validation),
                )?;
                if validation
                    .problems
                    .iter()
                    .any(|problem| !problem.ordinary_participant)
                {
                    result
                        .diagnostics
                        .push(source_diagnostic("overlayUnavailable"));
                }
                Ok(result)
            };
            let reply = match request {
                AgentFileReadRequest::Read(source) => AgentFileReadReply::Read(snapshot(&source)?),
                AgentFileReadRequest::List => {
                    let sources = list_active_sources(&authority.context)
                        .iter()
                        .map(|source| {
                            let s = snapshot(source)?;
                            Ok(AgentFileSourceOption {
                                source: s.source,
                                revision: s.revision,
                                context_revision: s.context_revision,
                                format: s.format,
                                availability: s.availability,
                                capabilities: s.capabilities,
                                diagnostics: s.diagnostics,
                            })
                        })
                        .collect::<Result<Vec<_>, String>>()?;
                    AgentFileReadReply::List(agent_file_sources_reply(sources, preferences)?)
                }
            };
            authority.recheck(&guard)?;
            self.revalidate().map_err(|_| "settingsOwnerUnavailable")?;
            Ok(reply)
        }))
        .map_err(|_| "sourceWorkerFailed".to_owned())?
    }
}

// Constructed only while the closed catalog operation holds owner authority.
pub(crate) struct CatalogSourceOperationGuard<'op, 'lease> {
    pub(crate) owner: &'op SettingsOwner,
    pub(crate) guard: &'op mut SettingsOperationGuard<'lease>,
    pub(crate) authority: AgentFileAuthority,
    pub(crate) source: SourceRef,
    pub(crate) path: PathBuf,
    pub(crate) expected: SourceRevision,
    pub(crate) adopted: AppSettings,
}

impl CatalogSourceOperationGuard<'_, '_> {
    pub(crate) fn target(&self) -> &Path {
        &self.path
    }

    pub(crate) fn verify_target_parent_chain(&self) -> Result<(), String> {
        crate::path_identity::verify_component_chain(self.path.parent().expect("parent"))
            .map_err(|_| "sourceContextUnavailable".to_owned())
    }

    pub(crate) fn effect(&self) -> OperationEffect {
        self.guard.effect()
    }

    pub(crate) fn prepare(&self, edits: &AgentFileEdits) -> Result<Vec<u8>, String> {
        let observation = observe_agent_file(&self.path);
        let snapshot = snapshot_from_captured_source(
            &self.authority.context,
            &self.source,
            &observation,
            None,
        )?;
        if snapshot.revision != self.expected {
            return Err("staleSourceRevision".into());
        }
        if !snapshot.capabilities.editable_save {
            return Err("sourceReadOnlyOrUnavailable".into());
        }
        let before = crate::config::coding_agents_catalog::parse_strict_json(
            observation.bytes().ok_or("sourceReadFailed")?,
        )?;
        let candidate = prepare_agent_file_edits(&before, false, edits)?;
        let encoded = serde_json::to_vec_pretty(&candidate).map_err(|_| "invalidSource")?;
        if encoded.len() > SETTINGS_CAPTURE_LIMIT {
            return Err("sourceTooLarge".into());
        }
        Ok(encoded)
    }

    pub(crate) fn recheck(&self) -> Result<(), String> {
        agent_file_checkpoint("catalog_pre_recheck", &self.path);
        self.authority.recheck(self.guard)?;
        self.owner
            .revalidate()
            .map_err(|_| "settingsOwnerUnavailable")?;
        let path = self
            .authority
            .context
            .locator(&self.source)
            .map_err(|_| "sourceContextUnavailable")?;
        if path != self.path {
            return Err("staleSourceContext".into());
        }
        let observation = observe_agent_file(&path);
        let bytes = observation.bytes().ok_or("sourceReadFailed")?;
        if source_bytes_revision(bytes) != self.expected {
            return Err("staleSourceRevision".into());
        }
        Ok(())
    }

    pub(crate) fn publish(&mut self, bytes: &[u8]) -> Result<(), String> {
        let path = &self.path;
        self.guard.publish(SettingsPublisher::AgentsSave, |_| {
            agent_file_checkpoint("catalog_publish", path);
            crate::config::coding_agents_catalog::replace_selected_personal_bytes(path, bytes)
        })
    }

    pub(crate) fn finish(
        &self,
        mut outcome: AgentFileSaveOutcome,
        reason: Option<&str>,
    ) -> AgentFileSaveReply {
        agent_file_checkpoint("catalog_before_reread", &self.path);
        let observed = observe_agent_file(&self.path);
        let mut snapshot =
            snapshot_from_captured_source(&self.authority.context, &self.source, &observed, None)
                .ok();
        let mut diagnostics = vec![source_diagnostic("effectiveSettingsUnchanged")];
        if let Some(reason) = reason {
            let code = [
                "staleSourceRevision",
                "staleSourceContext",
                "sourceProfileConflict",
                "invalidLegacyProfileOrphans",
                "duplicateJsonMember",
                "sourceReadOnlyOrUnavailable",
                "catalogLockTimeout",
                "uneditablePatchKey",
                "unknownBaseHash",
            ]
            .into_iter()
            .find(|code| reason.contains(code))
            .unwrap_or("catalogSaveFailed");
            let mut diagnostic = source_diagnostic(code);
            if code == "sourceProfileConflict" {
                diagnostic.entry_hint = Some(reason.to_owned());
            }
            diagnostics.push(diagnostic);
        }
        let authority = self.authority.recheck(self.guard);
        let integrity_lost = matches!(&authority, Err(reason) if reason == "ownerIntegrityLost");
        if integrity_lost {
            self.owner.mark_invalid("catalog-owner-integrity");
        }
        let ready = self.owner.revalidate().is_ok();
        if authority.is_err() || !ready {
            diagnostics.push(source_diagnostic(if integrity_lost {
                "ownerIntegrityLost"
            } else {
                "sourceContextUnavailable"
            }));
            if let Some(snapshot) = snapshot.as_mut() {
                snapshot.availability = SourceAvailability::ContextUnavailable;
                snapshot.capabilities = SourceCapabilities::default();
                snapshot.rows.clear();
                snapshot.profiles_by_base_hash.clear();
                snapshot.profile_labels_by_base_hash.clear();
                snapshot.profile_slots.clear();
                snapshot.tickets.clear();
            }
            outcome = if self.effect() == OperationEffect::Possible {
                AgentFileSaveOutcome::PartialOrUnknown
            } else {
                AgentFileSaveOutcome::Rejected
            };
        }
        if outcome == AgentFileSaveOutcome::Committed
            && snapshot
                .as_ref()
                .is_none_or(|s| s.availability != SourceAvailability::Available)
        {
            outcome = AgentFileSaveOutcome::PartialOrUnknown;
            diagnostics.push(source_diagnostic("catalogRereadUnavailable"));
        }
        AgentFileSaveReply {
            outcome,
            snapshot,
            effective_settings: (ready && !integrity_lost).then(|| self.adopted.clone()),
            diagnostics,
        }
    }

    pub(crate) fn interrupted(&self) -> AgentFileSaveReply {
        AgentFileSaveReply {
            outcome: if self.effect() == OperationEffect::Possible {
                AgentFileSaveOutcome::PartialOrUnknown
            } else {
                AgentFileSaveOutcome::Rejected
            },
            snapshot: None,
            effective_settings: None,
            diagnostics: vec![source_diagnostic("catalogRereadUnavailable")],
        }
    }
}

impl SettingsOwner {
    pub(crate) fn save_catalog_source_blocking(
        &self,
        source: SourceRef,
        expected: SourceRevision,
        context_revision: String,
        edits: AgentFileEdits,
    ) -> Result<AgentFileSaveReply, String> {
        let _admission = self
            .admit_operation()
            .map_err(|_| "settingsOwnerUnavailable")?;
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let state = self.settings.blocking_write();
            let lease = self.lease_guard();
            let lease = lease.as_ref().ok_or("settingsOwnerUnavailable")?;
            let mut guard = self.operation_guard(lease).map_err(|error| {
                if matches!(error, SettingsOwnerError::Lifecycle(_)) {
                    self.mark_invalid("catalog-owner-integrity");
                    "ownerIntegrityLost"
                } else {
                    "settingsOwnerUnavailable"
                }
            })?;
            self.revalidate().map_err(|_| "settingsOwnerUnavailable")?;
            let authority = match AgentFileAuthority::capture(&guard) {
                Ok(authority) => authority,
                Err(reason) => {
                    if reason == "ownerIntegrityLost" {
                        self.mark_invalid("catalog-owner-integrity");
                    }
                    return Ok(AgentFileSaveReply {
                        outcome: AgentFileSaveOutcome::Rejected,
                        snapshot: None,
                        effective_settings: (self.revalidate().is_ok()).then(|| (*state).clone()),
                        diagnostics: vec![source_diagnostic(&reason)],
                    });
                }
            };
            if source.scope != SourceScope::Project
                || source.kind != SourceKind::CatalogPersonal
                || context_revision != authority.context.revision
            {
                return Ok(AgentFileSaveReply {
                    outcome: AgentFileSaveOutcome::Rejected,
                    snapshot: None,
                    effective_settings: Some((*state).clone()),
                    diagnostics: vec![source_diagnostic("sourceContextUnavailable")],
                });
            }
            let path = match authority.context.locator(&source) {
                Ok(path) => path,
                Err(_) => {
                    return Ok(AgentFileSaveReply {
                        outcome: AgentFileSaveOutcome::Rejected,
                        snapshot: None,
                        effective_settings: Some((*state).clone()),
                        diagnostics: vec![source_diagnostic("sourceContextUnavailable")],
                    })
                }
            };
            let mut proof = CatalogSourceOperationGuard {
                owner: self,
                guard: &mut guard,
                authority,
                source,
                path,
                expected,
                adopted: (*state).clone(),
            };
            agent_file_checkpoint("catalog_authorized", proof.target());
            Ok(crate::config::coding_agents_catalog::publish_selected_personal(&mut proof, &edits))
        }))
        .unwrap_or_else(|_| {
            Ok(AgentFileSaveReply {
                outcome: AgentFileSaveOutcome::Rejected,
                snapshot: None,
                effective_settings: None,
                diagnostics: vec![source_diagnostic("catalogSetupPanicked")],
            })
        })
    }
}

#[cfg(test)]
pub(crate) type AgentFileTestHook = Box<dyn FnMut(&str, &Path)>;

#[cfg(test)]
thread_local! {
    pub(crate) static AGENT_FILE_CHECKPOINT: std::cell::RefCell<Option<AgentFileTestHook>> =
        const { std::cell::RefCell::new(None) };
}

pub(crate) fn agent_file_checkpoint(stage: &str, path: &Path) {
    #[cfg(test)]
    AGENT_FILE_CHECKPOINT.with(|hook| {
        if let Some(hook) = hook.borrow_mut().as_mut() {
            hook(stage, path);
        }
    });
    #[cfg(not(test))]
    let _ = (stage, path);
}
