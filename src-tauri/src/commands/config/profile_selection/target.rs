use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::config::settings::AppSettings;
use crate::session::manager::SessionManager;
use crate::session::session::SessionInfo;

use super::wire::{
    AssignmentMode, ConflictDecision, ProfileAssignmentScope, ProfileAssignmentTarget, SavedPair,
    SelectionState,
};

/// #1941 - one identified candidate plus its canonical Matrix dir, kept so a
/// fingerprint never has to re-resolve identity from a mutable path.
pub(crate) struct ProfileCandidate {
    pub(crate) target: ProfileAssignmentTarget,
    pub(crate) matrix_dir: PathBuf,
    pub(crate) expected: Option<crate::config::coding_agent_profiles::ReplicaSelectionExpectation>,
}

/// #2557 - one replica or folder the scope walk could not count, and why.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScopeFault {
    pub code: ScopeFaultCode,
    pub replica_name: String,
    pub replica_path: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ScopeFaultCode {
    ConfigUnreadable,
    ConfigNotJson,
    ConfigNotObject,
    IdentityMissing,
    IdentityMismatch,
    LocationInvalid,
    PathUnreadable,
    FolderUnreadable,
}

impl ScopeFault {
    /// Unlike `build_profile_assignment_target`, a missing `__agent_` prefix
    /// keeps the whole folder name: a fault is never nameless.
    pub(crate) fn new(code: ScopeFaultCode, path: &Path) -> Self {
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        Self {
            code,
            replica_name: name
                .strip_prefix("__agent_")
                .map(str::to_string)
                .unwrap_or(name),
            replica_path: crate::path_utils::path_to_string_without_windows_verbatim_prefix(path),
        }
    }
}

// Exhaustive on purpose: a new replica_identity fault must break the build here.
pub(crate) fn scope_fault_code(
    fault: crate::config::replica_identity::ReplicaConfigFault,
) -> ScopeFaultCode {
    use crate::config::replica_identity::ReplicaConfigFault;
    match fault {
        ReplicaConfigFault::ConfigUnreadable => ScopeFaultCode::ConfigUnreadable,
        ReplicaConfigFault::ConfigNotJson => ScopeFaultCode::ConfigNotJson,
        ReplicaConfigFault::ConfigNotObject => ScopeFaultCode::ConfigNotObject,
        ReplicaConfigFault::IdentityMissing => ScopeFaultCode::IdentityMissing,
        ReplicaConfigFault::IdentityMismatch => ScopeFaultCode::IdentityMismatch,
        ReplicaConfigFault::LocationInvalid => ScopeFaultCode::LocationInvalid,
    }
}

/// #1941 - the complete identified candidate set for one scope, together with
/// whether the directory walk could establish that set.
pub(crate) struct ProfileTargetEnumeration {
    pub(crate) anchor_dir: PathBuf,
    pub(crate) anchor_matrix_dir: PathBuf,
    pub(crate) candidates: Vec<ProfileCandidate>,
    pub(crate) warnings: Vec<String>,
    pub(crate) scope_faults: Vec<ScopeFault>,
    pub(crate) counts_complete: bool,
}

impl ProfileTargetEnumeration {
    pub(crate) fn targets(&self) -> Vec<ProfileAssignmentTarget> {
        self.candidates
            .iter()
            .map(|candidate| candidate.target.clone())
            .collect()
    }
}

/// #1941 - directory-walk accumulator. A read failure is a visible warning that
/// makes the scope counts incomplete, never a silent skip: an unreadable member
/// could be protected, so the scope can no longer certify a total.
#[derive(Default)]
pub(crate) struct CandidateDirs {
    pub(crate) dirs: Vec<PathBuf>,
    warnings: Vec<String>,
    pub(crate) faults: Vec<ScopeFault>,
    pub(crate) complete: bool,
}

impl CandidateDirs {
    pub(crate) fn new() -> Self {
        Self {
            dirs: Vec::new(),
            warnings: Vec::new(),
            faults: Vec::new(),
            complete: true,
        }
    }

    fn mark_incomplete(&mut self, warning: String, fault: ScopeFault) {
        self.warnings.push(warning);
        self.faults.push(fault);
        self.complete = false;
    }
}

pub(crate) fn validate_profile_assignment_request(
    settings: &AppSettings,
    coding_agent_id: &str,
    profile: &str,
) -> Result<(), String> {
    if !settings
        .agents
        .iter()
        .any(|agent| agent.id == coding_agent_id)
    {
        return Err(format!("Agent '{}' is not configured", coding_agent_id));
    }
    normalize_profile_letter_for_assignment(profile)?;
    Ok(())
}

pub(crate) fn normalize_profile_letter_for_assignment(profile: &str) -> Result<String, String> {
    crate::config::settings::normalize_profile_letter(profile)
        .ok_or_else(|| "Profile must be a single letter A through Z".to_string())
}

pub(crate) fn profile_assignment_requires_explicit_confirmation(
    scope: &ProfileAssignmentScope,
    mode: AssignmentMode,
) -> bool {
    *scope != ProfileAssignmentScope::Replica || mode == AssignmentMode::AssignAndLock
}

/// #1941 - a conflict decision belongs only to a bulk assign-and-lock; anything
/// else is rejected before enumeration so no client boolean can widen force.
pub(crate) fn validate_assignment_decision_scope(
    scope: &ProfileAssignmentScope,
    mode: AssignmentMode,
    decision: Option<ConflictDecision>,
) -> Result<(), String> {
    if decision.is_none() {
        return Ok(());
    }
    if *scope == ProfileAssignmentScope::Replica {
        return Err("conflictDecision is not valid for replica scope".to_string());
    }
    if mode == AssignmentMode::Ordinary {
        return Err("conflictDecision is only valid with assignmentMode assignAndLock".to_string());
    }
    Ok(())
}

pub(crate) fn validate_profile_assignment_confirmation(
    scope: &ProfileAssignmentScope,
    mode: AssignmentMode,
    provided: Option<&str>,
    fingerprint: &str,
) -> Result<(), String> {
    match provided {
        Some(value) if value == fingerprint => Ok(()),
        // #1941 - the legacy deliberate replica assignment keeps working without
        // a fingerprint; every bulk, assignAndLock and removal path needs one.
        None if *scope == ProfileAssignmentScope::Replica && mode == AssignmentMode::Ordinary => {
            Ok(())
        }
        // #2051 - no fingerprint at all is a client contract failure, not a
        // target change: never report it as a stale preview.
        None => Err(
            "confirmationMissing: This operation requires the preview confirmation, and none was \
                sent. Nothing was written."
                .to_string(),
        ),
        Some(_) => Err(
            "stalePreview: Target selection changed. Rerun preview before applying profile \
                selection."
                .to_string(),
        ),
    }
}

pub(crate) fn prevalidate_profile_assignment_restarts(
    settings: &AppSettings,
    coding_agent_id: &str,
    profile: &str,
    targets: &[ProfileAssignmentTarget],
) -> Result<(), String> {
    for target in targets {
        if target.live_session_ids.is_empty() {
            continue;
        }
        crate::config::agent_command::build_agent_spawn_command(
            settings,
            coding_agent_id,
            Some(Path::new(&target.replica_path)),
            Some(profile),
        )?;
    }
    Ok(())
}

pub(crate) async fn classify_restart_failure(
    session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
    uuid: uuid::Uuid,
) -> (&'static str, bool) {
    let session_survived = {
        let mgr = session_mgr.read().await;
        mgr.get_session(uuid).await.is_some()
    };
    if session_survived {
        ("restartFailed", false)
    } else {
        ("destroyedButNotRecreated", true)
    }
}

/// #1941 - the scope anchor must be a strictly validated `__agent_*` replica
/// inside a `room-*`/`wg-*` directory whose canonical origin Matrix lives under
/// a configured project root. An origin Matrix or the Root Agent is never a
/// lockable replica target, and no client-supplied target list is accepted.
pub(crate) fn validate_selection_anchor(
    settings: &AppSettings,
    target_replica_path: &Path,
) -> Result<(PathBuf, PathBuf), String> {
    let target_replica = canonical_real_dir(target_replica_path, "target replica")?;
    validate_wg_replica_path(&target_replica)?;
    let validated = crate::config::coding_agent_profiles::validate_profile_selection_agent_path(
        settings,
        &target_replica,
    )?;
    let is_replica = validated
        .launch_path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with("__agent_"));
    if !is_replica {
        return Err(format!(
            "Selection lock target '{}' is not a Room replica",
            validated.launch_path.display()
        ));
    }
    Ok((validated.launch_path, validated.origin_matrix_dir))
}

pub(crate) fn enumerate_profile_assignment_targets(
    settings: &AppSettings,
    target_replica_path: &Path,
    scope: &ProfileAssignmentScope,
    sessions: &[SessionInfo],
) -> Result<ProfileTargetEnumeration, String> {
    let (anchor_dir, anchor_matrix_dir) = validate_selection_anchor(settings, target_replica_path)?;
    let mut candidate_dirs = CandidateDirs::new();
    match scope {
        ProfileAssignmentScope::Replica => candidate_dirs.dirs.push(anchor_dir.clone()),
        ProfileAssignmentScope::Workgroup => {
            let room_dir = anchor_dir
                .parent()
                .ok_or_else(|| "Target replica has no room parent".to_string())?;
            collect_replica_dirs_in_workgroup(room_dir, &mut candidate_dirs);
        }
        ProfileAssignmentScope::Kind => {
            for ac_root in crate::config::coding_agent_profiles::configured_ac_roots(settings) {
                collect_kind_replica_dirs(&ac_root, &mut candidate_dirs);
            }
        }
    }
    let mut warnings = candidate_dirs.warnings;
    let mut scope_faults = candidate_dirs.faults;
    let mut counts_complete = candidate_dirs.complete;

    let anchor_matrix_key = canonical_compare_key(&anchor_matrix_dir);
    let live_by_cwd = live_sessions_by_cwd(sessions);
    let mut seen = BTreeSet::new();
    let mut candidates = Vec::new();

    // The anchor always belongs to its own scope; the walk dedupes against it.
    let (anchor_config, anchor_identity) =
        crate::config::replica_identity::read_wg_replica_config_read_only(&anchor_dir)
            .map_err(|e| format!("Target replica '{}': {}", anchor_dir.display(), e))?;
    let anchor_state = crate::config::coding_agent_profiles::selection_state_from_value(
        &anchor_config,
        &anchor_identity.identity,
    );
    seen.insert(canonical_compare_key(&anchor_dir));
    candidates.push(ProfileCandidate {
        expected: anchor_state.expectation().ok(),
        target: build_profile_assignment_target(
            &anchor_dir,
            &anchor_identity,
            &anchor_state,
            &live_by_cwd,
        ),
        matrix_dir: anchor_identity.matrix_dir,
    });

    for candidate in candidate_dirs.dirs {
        let Ok(replica_dir) = canonical_real_dir(&candidate, "candidate replica") else {
            warnings.push(format!(
                "Skipping unreadable replica '{}'",
                candidate.display()
            ));
            scope_faults.push(ScopeFault::new(ScopeFaultCode::PathUnreadable, &candidate));
            counts_complete = false;
            continue;
        };
        let key = canonical_compare_key(&replica_dir);
        if !seen.insert(key) {
            continue;
        }
        let (config, identity) =
            match crate::config::replica_identity::read_wg_replica_config_read_only_classified(
                &replica_dir,
            ) {
                Ok(read) => read,
                Err((fault, _message)) => {
                    // #1941 - an unprovable identity inside the scope means membership
                    // cannot be established: keep the diagnostic visible and refuse to
                    // certify a scope total for it.
                    warnings.push(format!(
                        "Skipping invalid replica '{}'",
                        replica_dir.display()
                    ));
                    scope_faults.push(ScopeFault::new(scope_fault_code(fault), &replica_dir));
                    counts_complete = false;
                    continue;
                }
            };
        if *scope == ProfileAssignmentScope::Kind
            && canonical_compare_key(&identity.matrix_dir) != anchor_matrix_key
        {
            // Homonymous matrices in other projects stay excluded: Kind means
            // the canonical origin Matrix, not the directory name.
            continue;
        }
        let state = crate::config::coding_agent_profiles::selection_state_from_value(
            &config,
            &identity.identity,
        );
        let target = build_profile_assignment_target(&replica_dir, &identity, &state, &live_by_cwd);
        candidates.push(ProfileCandidate {
            expected: state.expectation().ok(),
            target,
            matrix_dir: identity.matrix_dir,
        });
    }
    candidates.sort_by(|a, b| {
        a.target
            .workgroup_name
            .cmp(&b.target.workgroup_name)
            .then_with(|| a.target.replica_name.cmp(&b.target.replica_name))
            .then_with(|| a.target.replica_path.cmp(&b.target.replica_path))
    });
    // read_dir order is platform-defined; the payload order must not be.
    scope_faults.sort_by(|a, b| (&a.replica_path, a.code).cmp(&(&b.replica_path, b.code)));
    Ok(ProfileTargetEnumeration {
        anchor_dir,
        anchor_matrix_dir,
        candidates,
        warnings,
        scope_faults,
        counts_complete,
    })
}

pub(crate) fn canonical_compare_key(path: &Path) -> String {
    let path = crate::filesystem::projects::canonical_project_path(path)
        .unwrap_or_else(|_| path.to_path_buf());
    let mut text =
        crate::path_utils::path_to_string_without_windows_verbatim_prefix(&path).replace('\\', "/");
    if cfg!(windows) {
        text = text.to_ascii_lowercase();
    }
    text
}

pub(crate) fn canonical_real_dir(path: &Path, label: &str) -> Result<PathBuf, String> {
    let metadata = crate::filesystem::projects::project_path_symlink_metadata(path)
        .map_err(|e| format!("{} '{}' is not readable: {}", label, path.display(), e))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(format!(
            "{} '{}' is not a real directory",
            label,
            path.display()
        ));
    }
    crate::filesystem::projects::canonical_project_path(path)
        .map(|path| crate::path_utils::normalize_windows_verbatim_path_buf(&path))
        .map_err(|e| {
            format!(
                "Failed to canonicalize {} '{}': {}",
                label,
                path.display(),
                e
            )
        })
}

pub(crate) fn validate_wg_replica_path(path: &Path) -> Result<(), String> {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    if !name.starts_with("__agent_") {
        return Err(format!("Target '{}' is not a Room replica", path.display()));
    }
    let wg_name = path
        .parent()
        .and_then(|parent| parent.file_name())
        .and_then(|name| name.to_str())
        .unwrap_or("");
    if !crate::config::entity_prefix::has_entity_prefix(wg_name) {
        return Err(format!(
            "Target '{}' is not inside a `room-*` or legacy `wg-*` Room directory",
            path.display()
        ));
    }
    Ok(())
}

pub(crate) fn collect_replica_dirs_in_workgroup(wg_dir: &Path, out: &mut CandidateDirs) {
    crate::filesystem::profile_targets::visit_room_replicas(wg_dir, |event| {
        record_profile_directory_event(out, event);
    });
}

pub(crate) fn collect_kind_replica_dirs(ac_root: &Path, out: &mut CandidateDirs) {
    crate::filesystem::profile_targets::visit_kind_replicas(
        ac_root,
        crate::config::entity_prefix::has_entity_prefix,
        |event| record_profile_directory_event(out, event),
    );
}

fn record_profile_directory_event(
    out: &mut CandidateDirs,
    event: crate::filesystem::profile_targets::ProfileDirectoryEvent,
) {
    use crate::filesystem::profile_targets::ProfileDirectoryEvent;

    match event {
        ProfileDirectoryEvent::Candidate(path) => out.dirs.push(path),
        ProfileDirectoryEvent::Fault { warning, scope } => out.mark_incomplete(
            warning,
            ScopeFault::new(ScopeFaultCode::FolderUnreadable, &scope),
        ),
    }
}

pub(crate) fn live_sessions_by_cwd(sessions: &[SessionInfo]) -> BTreeMap<String, Vec<String>> {
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for session in sessions {
        if matches!(
            session.status,
            crate::session::session::SessionStatus::Exited(_)
        ) {
            continue;
        }
        out.entry(canonical_compare_key(Path::new(&session.working_directory)))
            .or_default()
            .push(session.id.clone());
    }
    out
}

pub(crate) fn build_profile_assignment_target(
    replica_dir: &Path,
    identity: &crate::config::replica_identity::WgReplicaIdentity,
    state: &crate::config::coding_agent_profiles::ReplicaSelectionState,
    live_by_cwd: &BTreeMap<String, Vec<String>>,
) -> ProfileAssignmentTarget {
    let wg_dir = replica_dir.parent().unwrap_or_else(|| Path::new(""));
    let workgroup_name = wg_dir
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("")
        .to_string();
    let replica_name = replica_dir
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| name.strip_prefix("__agent_"))
        .unwrap_or("")
        .to_string();
    let origin_project = identity
        .ac_root
        .parent()
        .and_then(|project| project.file_name())
        .and_then(|name| name.to_str())
        .map(str::to_string);
    let (saved_pair, selection_state, selection_error) = target_selection_wire(state);
    ProfileAssignmentTarget {
        workgroup_name,
        workgroup_path: crate::path_utils::path_to_string_without_windows_verbatim_prefix(wg_dir),
        replica_name,
        replica_path: crate::path_utils::path_to_string_without_windows_verbatim_prefix(
            replica_dir,
        ),
        identity_path: identity.identity.clone(),
        origin_project,
        live_session_ids: live_by_cwd
            .get(&canonical_compare_key(replica_dir))
            .cloned()
            .unwrap_or_default(),
        saved_pair,
        selection_state,
        selection_error,
    }
}

/// #1941 - strict state to wire fields. An invalid state carries its diagnostic
/// and never a synthesized `selectionLocked: false`.
pub(crate) fn target_selection_wire(
    state: &crate::config::coding_agent_profiles::ReplicaSelectionState,
) -> (Option<SavedPair>, SelectionState, Option<String>) {
    use crate::config::coding_agent_profiles::ReplicaSelectionState;
    match state {
        ReplicaSelectionState::Unlocked { pair, .. } => (
            pair.as_ref().map(saved_pair_wire),
            SelectionState::Unlocked,
            None,
        ),
        ReplicaSelectionState::Locked { pair, .. } => {
            (Some(saved_pair_wire(pair)), SelectionState::Locked, None)
        }
        ReplicaSelectionState::Invalid { diagnostic } => {
            (None, SelectionState::Invalid, Some(diagnostic.clone()))
        }
    }
}

pub(crate) fn saved_pair_wire(
    pair: &crate::config::coding_agent_profiles::ReplicaSelectionPair,
) -> SavedPair {
    SavedPair {
        coding_agent_id: Some(pair.coding_agent_id.clone()),
        requested_profile: Some(pair.requested_profile.clone()),
    }
}

pub(crate) fn target_pair(
    target: &ProfileAssignmentTarget,
) -> Option<crate::config::coding_agent_profiles::ReplicaSelectionPair> {
    let pair = target.saved_pair.as_ref()?;
    match (&pair.coding_agent_id, &pair.requested_profile) {
        (Some(coding_agent_id), Some(requested_profile)) => {
            Some(crate::config::coding_agent_profiles::ReplicaSelectionPair {
                coding_agent_id: coding_agent_id.clone(),
                requested_profile: requested_profile.clone(),
            })
        }
        _ => None,
    }
}
