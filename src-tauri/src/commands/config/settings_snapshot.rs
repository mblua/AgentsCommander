use std::path::PathBuf;
use tauri::State;

use sha2::{Digest, Sha256};

use crate::config::instance_artifacts::SETTINGS_FILE_NAME;
use crate::config::projects::{
    display_canonical, IssueKind, ProjectPathPersistenceState, ProjectSource, RawJsonField,
    RawStringField, ResolvedPair, SideStatus, StructuralIssue,
};
use crate::config::settings::{AppSettings, SettingsState, OVERLAY_KEY_AGENTS};

// ── #1077 SettingsSnapshot: the get_settings client response ────────────────

/// Tagged raw string field state (absent vs JSON null vs string). `present ==
/// false` always pairs with `value == None`.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RawStringFieldState {
    pub present: bool,
    pub value: Option<String>,
}

/// Tagged raw JSON field state (needed for wrong-typed structural fields and
/// absent/null parity).
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RawJsonFieldState {
    pub present: bool,
    pub value: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Copy, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ReconciliationStage {
    Read,
    Write,
}

/// The transport/I-O reconciliation error; structural/candidate issues belong in
/// `issues`, not here. `retryable` is always true.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectPathReconciliationError {
    pub stage: ReconciliationStage,
    pub message: String,
    pub retryable: bool,
}

#[derive(Debug, Clone, Copy, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum IssueSource {
    ProjectPath,
    ProjectPaths,
    ArchivedProjectPaths,
}

/// The discriminated project-path issue union. `rename_all_fields` is required
/// so struct-variant fields are camelCase (plain `rename_all` does not rename
/// them). Optional `index` is omitted when absent.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ProjectPathIssue {
    Conflict {
        id: String,
        source: IssueSource,
        #[serde(skip_serializing_if = "Option::is_none")]
        index: Option<usize>,
        absolute_candidate: String,
        instance_relative_candidate: String,
        absolute_resolved_path: String,
        instance_relative_resolved_path: String,
        message: String,
    },
    Missing {
        id: String,
        source: IssueSource,
        #[serde(skip_serializing_if = "Option::is_none")]
        index: Option<usize>,
        absolute_candidate: RawStringFieldState,
        instance_relative_candidate: RawStringFieldState,
        absolute_resolved_path: Option<String>,
        instance_relative_resolved_path: Option<String>,
        message: String,
    },
    Invalid {
        id: String,
        source: IssueSource,
        #[serde(skip_serializing_if = "Option::is_none")]
        index: Option<usize>,
        absolute_candidate: RawJsonFieldState,
        instance_relative_candidate: RawJsonFieldState,
        absolute_resolved_path: Option<String>,
        instance_relative_resolved_path: Option<String>,
        reason: String,
    },
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectPathResolution {
    pub active_registration_count: usize,
    pub archived_registration_count: usize,
    pub issues: Vec<ProjectPathIssue>,
    pub reconciliation_error: Option<ProjectPathReconciliationError>,
}

/// The flattened `get_settings` response: the runtime-selected `AppSettings`
/// plus the structured resolution report. Serialize-only.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsSnapshot {
    #[serde(flatten)]
    pub settings: AppSettings,
    pub project_path_resolution: ProjectPathResolution,
    /// #1347 - absolute path of the instance `settings.json` this snapshot was
    /// built for, so the UI can name the file that holds the plaintext secrets.
    /// `None` only when `config_dir()` itself is unresolvable (no `current_exe()`
    /// and no home dir); there is no `skip_serializing_if`, so the key is always
    /// present on the wire and is `null` in that degraded mode.
    pub settings_file_path: Option<String>,
    /// #2306 P2 - read-only disclosure that the local `settings.local.json`
    /// overlay owns the whole top-level `agents` key. While true, an ordered
    /// move cannot be durable: the writer restores the overlay's base array to
    /// `settings.json` and reapplies the overlay-effective array only to
    /// memory, so the picker must not offer (or must explain the rejection of)
    /// a move. Deliberately NOT a field of `AppSettings`: a client draft can
    /// never carry or forge overlay ownership.
    pub overlay_owns_agents: bool,
    /// #2716 (B3, option C) - read-only disclosure that the agents file could
    /// not be read, so no agent session may start. Same stance as
    /// `overlay_owns_agents`: never a field a client draft can carry.
    pub agents_layer_unreadable: bool,
    /// #2716 (B3, option C) - the unreadable agents file's path, for the notice.
    pub agents_file_path: Option<String>,
}

pub(crate) fn issue_source(source: ProjectSource) -> IssueSource {
    match source {
        ProjectSource::ProjectPath => IssueSource::ProjectPath,
        ProjectSource::ProjectPaths => IssueSource::ProjectPaths,
        ProjectSource::ArchivedProjectPaths => IssueSource::ArchivedProjectPaths,
    }
}

/// The active|archived logical list of a source (both active fields collapse to
/// "active"), used in the stable issue id.
pub(crate) fn source_list(source: ProjectSource) -> &'static str {
    match source {
        ProjectSource::ProjectPath | ProjectSource::ProjectPaths => "active",
        ProjectSource::ArchivedProjectPaths => "archived",
    }
}

pub(crate) fn raw_string_to_json(field: &RawStringField) -> RawJsonField {
    RawJsonField {
        present: field.present,
        value: field.value.clone().map(serde_json::Value::String),
    }
}

pub(crate) fn raw_string_state(field: &RawStringField) -> RawStringFieldState {
    RawStringFieldState {
        present: field.present,
        value: field.value.clone(),
    }
}

pub(crate) fn raw_json_state(field: &RawJsonField) -> RawJsonFieldState {
    RawJsonFieldState {
        present: field.present,
        value: field.value.clone(),
    }
}

/// Stable issue id: full lowercase SHA-256 hex of `(kind, active|archived list,
/// tagged raw absolute, tagged raw relative)`, absent distinct from JSON null,
/// excluding source index and resolved paths.
pub(crate) fn compute_issue_id(
    kind: &str,
    list: &str,
    abs: &RawJsonField,
    rel: &RawJsonField,
) -> String {
    let tuple = serde_json::json!([
        kind,
        list,
        { "present": abs.present, "value": abs.value },
        { "present": rel.present, "value": rel.value },
    ]);
    let serialized = serde_json::to_string(&tuple).unwrap_or_default();
    let digest = Sha256::digest(serialized.as_bytes());
    digest.iter().map(|b| format!("{:02x}", b)).collect()
}

pub(crate) fn side_status_label(status: SideStatus) -> &'static str {
    match status {
        SideStatus::Absent => "not set",
        SideStatus::BaseUnavailable => "no portable instance base",
        SideStatus::Malformed => "malformed",
        SideStatus::Missing => "not found",
        SideStatus::Inaccessible => "permission denied",
        SideStatus::ProbeIoError => "filesystem error",
        SideStatus::NotADirectory => "not a directory",
        SideStatus::AcRootOrCollectionMissing => "no .ac project or collection",
        SideStatus::NonUtf8 => "path is not valid UTF-8",
        SideStatus::ValidDirectProject => "valid project",
        SideStatus::ValidCollectionRoot => "valid collection root",
    }
}

pub(crate) fn pair_to_issue(pair: &ResolvedPair) -> ProjectPathIssue {
    let source = issue_source(pair.source);
    let list = source_list(pair.source);
    let abs_json = raw_string_to_json(&pair.raw_absolute);
    let rel_json = raw_string_to_json(&pair.raw_relative);
    let abs_resolved = pair
        .absolute_side
        .canonical_path
        .as_deref()
        .map(display_canonical);
    let rel_resolved = pair
        .relative_side
        .canonical_path
        .as_deref()
        .map(display_canonical);
    match pair.issue {
        Some(IssueKind::Conflict) => {
            let id = compute_issue_id("conflict", list, &abs_json, &rel_json);
            let abs_path = abs_resolved.clone().unwrap_or_default();
            let rel_path = rel_resolved.clone().unwrap_or_default();
            ProjectPathIssue::Conflict {
                id,
                source,
                index: pair.index,
                absolute_candidate: pair.raw_absolute.value.clone().unwrap_or_default(),
                instance_relative_candidate: pair.raw_relative.value.clone().unwrap_or_default(),
                message: format!(
                    "This project's two stored locations resolve to different directories, so \
                        neither was loaded. Absolute path: {abs_path}. Instance-relative path: \
                        {rel_path}."
                ),
                absolute_resolved_path: abs_path,
                instance_relative_resolved_path: rel_path,
            }
        }
        Some(IssueKind::Missing) => {
            let id = compute_issue_id("missing", list, &abs_json, &rel_json);
            ProjectPathIssue::Missing {
                id,
                source,
                index: pair.index,
                absolute_candidate: raw_string_state(&pair.raw_absolute),
                instance_relative_candidate: raw_string_state(&pair.raw_relative),
                message:
                    "This project directory could not be found. Reconnect the drive or remove \
                    it from the list."
                        .to_string(),
                absolute_resolved_path: pair.absolute_side.syntactic_path.clone(),
                instance_relative_resolved_path: pair.relative_side.syntactic_path.clone(),
            }
        }
        _ => {
            let id = compute_issue_id("invalid", list, &abs_json, &rel_json);
            ProjectPathIssue::Invalid {
                id,
                source,
                index: pair.index,
                absolute_candidate: raw_json_state(&abs_json),
                instance_relative_candidate: raw_json_state(&rel_json),
                reason: format!(
                    "This project could not be loaded (absolute candidate: {}; instance-relative \
                        candidate: {}).",
                    side_status_label(pair.absolute_side.status),
                    side_status_label(pair.relative_side.status)
                ),
                absolute_resolved_path: pair.absolute_side.syntactic_path.clone(),
                instance_relative_resolved_path: pair.relative_side.syntactic_path.clone(),
            }
        }
    }
}

pub(crate) fn structural_to_issue(structural: &StructuralIssue) -> ProjectPathIssue {
    let id = compute_issue_id(
        "invalid",
        source_list(structural.source),
        &structural.raw_absolute,
        &structural.raw_relative,
    );
    ProjectPathIssue::Invalid {
        id,
        source: issue_source(structural.source),
        index: None,
        absolute_candidate: raw_json_state(&structural.raw_absolute),
        instance_relative_candidate: raw_json_state(&structural.raw_relative),
        absolute_resolved_path: None,
        instance_relative_resolved_path: None,
        reason: format!("Malformed project settings: {}.", structural.reason),
    }
}

/// Build the structured resolution report from the hidden persistence state.
pub(crate) fn build_project_path_resolution(
    state: &ProjectPathPersistenceState,
    reconciliation_error: Option<ProjectPathReconciliationError>,
) -> ProjectPathResolution {
    let mut issues: Vec<ProjectPathIssue> = state.issues().map(pair_to_issue).collect();
    issues.extend(state.structural_issues.iter().map(structural_to_issue));

    // Counts merged logical records; force a minimum of one for any group that
    // has a structural issue so a corruption-only startup is never called pristine.
    let mut active = state.active_registration_count;
    let mut archived = state.archived_registration_count;
    for structural in &state.structural_issues {
        match structural.source {
            ProjectSource::ProjectPath | ProjectSource::ProjectPaths => active = active.max(1),
            ProjectSource::ArchivedProjectPaths => archived = archived.max(1),
        }
    }

    ProjectPathResolution {
        active_registration_count: active,
        archived_registration_count: archived,
        issues,
        reconciliation_error,
    }
}

/// Snapshot builder: clone `settings`, clear the root token, attach the
/// resolution report, and record the instance settings-file path (#1347).
/// Shared by the Tauri and WebSocket transports so both clients receive the
/// identical report and `rootToken` is absent from each. The only non-argument
/// input is the process-wide, cached instance location behind `config_dir()`.
pub(crate) fn settings_snapshot_from(
    settings: &AppSettings,
    reconciliation_error: Option<ProjectPathReconciliationError>,
) -> SettingsSnapshot {
    let resolution =
        build_project_path_resolution(&settings.project_path_state, reconciliation_error);
    let mut cleaned = settings.clone();
    // Clear so the existing skip_serializing_if omits rootToken (absent, not null).
    cleaned.root_token = None;
    SettingsSnapshot {
        settings: cleaned,
        project_path_resolution: resolution,
        // #1347: derived from the process instance location, deliberately NOT
        // from `settings_snapshot_helper`'s injectable `settings_path`, which is
        // a test-only reconciliation write target and not a client-facing
        // location. Same expression already used at the reconciliation site.
        settings_file_path: crate::config::config_dir()
            .map(|d| d.join(SETTINGS_FILE_NAME).to_string_lossy().into_owned()),
        overlay_owns_agents: settings
            .local_overlay_state
            .owns_top_level(OVERLAY_KEY_AGENTS),
        agents_layer_unreadable: settings.agents_layer.unreadable_path().is_some(),
        agents_file_path: settings
            .agents_layer
            .unreadable_path()
            .map(|path| path.to_string_lossy().into_owned()),
    }
}

/// The shared, path-injectable settings-snapshot helper. At the first snapshot
/// boundary it reconciles any eligible pending repair to disk (§4.3), then
/// returns the report. Tests inject `settings_path`; production resolves it.
pub(crate) async fn settings_snapshot_helper(
    settings: &SettingsState,
    settings_path: Option<PathBuf>,
) -> SettingsSnapshot {
    let path =
        settings_path.or_else(|| crate::config::config_dir().map(|d| d.join(SETTINGS_FILE_NAME)));
    // State precedes the file lock. Disk work runs off the runtime worker;
    // no file lock or transaction survives an await.
    let mut guard = settings.clone().write_owned().await;
    let task = tokio::task::spawn_blocking(move || {
        let result = match path {
            Some(path) => {
                crate::config::settings::repair_fresh_project_paths_at_path(&mut guard, &path)
            }
            None => {
                let message = "Could not determine project settings path".to_string();
                guard.project_paths_persistence_error = Some(message.clone());
                Err((
                    crate::config::settings::ProjectPathsRepairStage::Read,
                    message,
                ))
            }
        };
        let reconciliation_error =
            result
                .err()
                .map(|(stage, message)| ProjectPathReconciliationError {
                    stage: match stage {
                        crate::config::settings::ProjectPathsRepairStage::Read => {
                            ReconciliationStage::Read
                        }
                        crate::config::settings::ProjectPathsRepairStage::Write => {
                            ReconciliationStage::Write
                        }
                    },
                    message,
                    retryable: true,
                });
        settings_snapshot_from(&guard, reconciliation_error)
    });
    match task.await {
        Ok(snapshot) => snapshot,
        Err(error) => {
            let mut guard = settings.write().await;
            let message = format!("Project-path snapshot task failed: {error}");
            guard.project_paths_persistence_error = Some(message.clone());
            settings_snapshot_from(
                &guard,
                Some(ProjectPathReconciliationError {
                    stage: ReconciliationStage::Read,
                    message,
                    retryable: true,
                }),
            )
        }
    }
}

#[tauri::command]
pub async fn get_settings(settings: State<'_, SettingsState>) -> Result<SettingsSnapshot, String> {
    Ok(settings_snapshot_helper(settings.inner(), None).await)
}
