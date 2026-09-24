use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::hash::Hash;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tauri::{AppHandle, Emitter, State};
use uuid::Uuid;

use sha2::{Digest, Sha256};

use crate::api::auth;
use crate::config::coding_agent_mutations::{move_registered_agent, AgentMoveDirection};
use crate::config::instance_artifacts::DEBUG_LOGS_FILE_NAME;
use crate::config::projects::{
    display_canonical, IssueKind, ProjectPathPersistenceState, ProjectSource, RawJsonField,
    RawStringField, ResolvedPair, SideStatus, StructuralIssue,
};
use crate::config::settings::{
    load_settings, merge_protected_coding_agent_settings, normalize_agent_order,
    parse_api_server_socket_addr, save_settings, validate_and_repair_settings, AgentConfig,
    AppSettings, CodingAgentEnv, CodingAgentProfilesConfig, SettingsState, OVERLAY_KEY_AGENTS,
};
use crate::network::OutboundNetwork;
use crate::pty::manager::PtyManager;
use crate::session::manager::SessionManager;
use crate::session::session::SessionInfo;
use crate::web::auth::WebAccessToken;
use crate::web::broadcast::WsBroadcaster;
use crate::{
    ApiServerHandle, ApiServerTask, WebServerHandle, WebServerLifecycle,
    WebServerLifecycleSnapshot, WebServerStartWaiter, WEB_SERVER_START_CANCELLED,
};

const HOME_MARKDOWN_URL: &str =
    "https://raw.githubusercontent.com/mblua/AgentsCommander/main/docs/home-en.md";

const HOME_MARKDOWN_MAX_BYTES: usize = 256 * 1024; // 256 KB
const HOME_MARKDOWN_TIMEOUT_SECS: u64 = 5;
const WEB_STATUS_CONNECT_TIMEOUT_MS: u64 = 500;
const API_SERVER_STOP_TIMEOUT_MS: u64 = 5_000;
// Default mirrors container API token TTL; manual GUI mints get a longer cap.
const MINT_API_CLIENT_DEFAULT_TTL_HOURS: i64 = 24;
const MINT_API_CLIENT_MAX_TTL_DAYS: i64 = 30;
const MINT_API_CLIENT_NOTE: &str =
    "Store this token now; it is shown only once. The registry keeps only a hash. A manually requested pty-input scope does not grant actuation without an automatically bound live container session.";

/// #2133 - the agent-help overlays the frontend layers over its embedded copy. The embedded
/// layer never travels over IPC (epic D6). `remote` stays `None` until P6 fills it.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentHelpOverlayPayload {
    pub local: Option<crate::config::settings::AgentHelpFile>,
    pub remote: Option<crate::config::settings::AgentHelpFile>,
    pub local_error: Option<String>,
}

/// #2133 - the path-injectable overlay reader. Tests inject `settings_path`;
/// production resolves it. Mirrors `settings_snapshot_helper`'s seam.
pub(crate) fn agent_help_overlay_payload(
    settings_path: Option<PathBuf>,
) -> AgentHelpOverlayPayload {
    let Some(path) = settings_path.or_else(crate::config::settings::settings_path) else {
        return AgentHelpOverlayPayload {
            local: None,
            remote: None,
            local_error: None,
        };
    };
    let (file, local_error) = crate::config::settings::load_local_agent_help_file(&path);
    let has_content =
        file.general.is_some() || !file.by_command.is_empty() || !file.by_agent.is_empty();
    AgentHelpOverlayPayload {
        local: has_content.then_some(file),
        remote: None,
        local_error,
    }
}

#[tauri::command]
pub fn get_agent_help() -> AgentHelpOverlayPayload {
    agent_help_overlay_payload(None)
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodingAgentProfileResolutionResult {
    pub requested_profile: String,
    pub effective_profile: String,
    pub fallback_chain: Vec<String>,
    pub fallback_applied: bool,
    pub requested_profile_input: Option<String>,
    pub instance_profile_override: Option<String>,
    pub origin_default_profile: Option<String>,
    pub agent_default_profile: Option<String>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MintApiClientResponse {
    pub client_id: String,
    pub token: String,
    pub bound_fqn: String,
    pub bound_root: String,
    pub scopes: Vec<String>,
    pub expires_at: Option<String>,
    pub note: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SettingsDraftUpdateEvents {
    pub profiles_changed: bool,
    pub env_agent_ids: Vec<String>,
}

#[tauri::command]
pub async fn save_debug_logs(content: String) -> Result<(), String> {
    let path = crate::config::config_dir()
        .ok_or("No config dir")?
        .join(DEBUG_LOGS_FILE_NAME);
    tokio::fs::write(&path, &content)
        .await
        .map_err(|e| format!("Failed to write logs: {}", e))?;
    log::info!("Debug logs saved to {:?} ({} bytes)", path, content.len());
    Ok(())
}

/// #264 — read-and-clear the buffered ERROR-level log entries for the UI error
/// modal. The frontend calls this once on `ErrorModal` mount (to collect errors
/// logged before the webview was listening) and again on every `error_log_event`
/// ping. Sync (no I/O, no `.await`) — a sub-microsecond mutex take.
#[tauri::command]
pub fn drain_error_logs() -> Vec<crate::logging::ErrorLogEntry> {
    crate::logging::error_sink().drain()
}

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
}

fn issue_source(source: ProjectSource) -> IssueSource {
    match source {
        ProjectSource::ProjectPath => IssueSource::ProjectPath,
        ProjectSource::ProjectPaths => IssueSource::ProjectPaths,
        ProjectSource::ArchivedProjectPaths => IssueSource::ArchivedProjectPaths,
    }
}

/// The active|archived logical list of a source (both active fields collapse to
/// "active"), used in the stable issue id.
fn source_list(source: ProjectSource) -> &'static str {
    match source {
        ProjectSource::ProjectPath | ProjectSource::ProjectPaths => "active",
        ProjectSource::ArchivedProjectPaths => "archived",
    }
}

fn raw_string_to_json(field: &RawStringField) -> RawJsonField {
    RawJsonField {
        present: field.present,
        value: field.value.clone().map(serde_json::Value::String),
    }
}

fn raw_string_state(field: &RawStringField) -> RawStringFieldState {
    RawStringFieldState {
        present: field.present,
        value: field.value.clone(),
    }
}

fn raw_json_state(field: &RawJsonField) -> RawJsonFieldState {
    RawJsonFieldState {
        present: field.present,
        value: field.value.clone(),
    }
}

/// Stable issue id: full lowercase SHA-256 hex of `(kind, active|archived list,
/// tagged raw absolute, tagged raw relative)`, absent distinct from JSON null,
/// excluding source index and resolved paths.
fn compute_issue_id(kind: &str, list: &str, abs: &RawJsonField, rel: &RawJsonField) -> String {
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

fn side_status_label(status: SideStatus) -> &'static str {
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

fn pair_to_issue(pair: &ResolvedPair) -> ProjectPathIssue {
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
                    "This project's two stored locations resolve to different directories, so neither was loaded. Absolute path: {abs_path}. Instance-relative path: {rel_path}."
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
                message: "This project directory could not be found. Reconnect the drive or remove it from the list.".to_string(),
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
                    "This project could not be loaded (absolute candidate: {}; instance-relative candidate: {}).",
                    side_status_label(pair.absolute_side.status),
                    side_status_label(pair.relative_side.status)
                ),
                absolute_resolved_path: pair.absolute_side.syntactic_path.clone(),
                instance_relative_resolved_path: pair.relative_side.syntactic_path.clone(),
            }
        }
    }
}

fn structural_to_issue(structural: &StructuralIssue) -> ProjectPathIssue {
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
            .map(|d| d.join("settings.json").to_string_lossy().into_owned()),
        overlay_owns_agents: settings
            .local_overlay_state
            .owns_top_level(OVERLAY_KEY_AGENTS),
    }
}

/// The shared, path-injectable settings-snapshot helper. At the first snapshot
/// boundary it reconciles any eligible pending repair to disk (§4.3), then
/// returns the report. Tests inject `settings_path`; production resolves it.
pub(crate) async fn settings_snapshot_helper(
    settings: &SettingsState,
    settings_path: Option<PathBuf>,
) -> SettingsSnapshot {
    let mut reconciliation_error = None;
    {
        // Reconciliation transaction under the write guard; no lock across await.
        let mut guard = settings.write().await;
        let pending = {
            let state = &guard.project_path_state;
            !state.has_structural()
                && (state.active_reconcile_eligible || state.archived_reconcile_eligible)
        };
        if pending {
            if let Some(path) = settings_path
                .clone()
                .or_else(|| crate::config::config_dir().map(|d| d.join("settings.json")))
            {
                // §4.3 step 2: re-decode all six project fields from disk and
                // re-resolve BEFORE reconciling, so a CLI registration that
                // happened after startup is authoritative and not clobbered. On a
                // disk read/parse failure, retain the previously validated state,
                // perform no write, and report stage `read`.
                match crate::config::settings::refresh_and_decode_project_paths_from_path(
                    &mut guard, &path,
                ) {
                    Err(message) => {
                        reconciliation_error = Some(ProjectPathReconciliationError {
                            stage: ReconciliationStage::Read,
                            message,
                            retryable: true,
                        });
                    }
                    Ok(()) => {
                        let fresh = guard.project_path_state.clone();
                        let still_eligible = !fresh.has_structural()
                            && (fresh.active_reconcile_eligible
                                || fresh.archived_reconcile_eligible);
                        if still_eligible {
                            match crate::config::settings::reconcile_project_state_to_path(
                                &guard,
                                &path,
                                fresh.active_reconcile_eligible,
                                fresh.archived_reconcile_eligible,
                            ) {
                                Ok(written) => *guard = written,
                                Err(message) => {
                                    reconciliation_error = Some(ProjectPathReconciliationError {
                                        stage: ReconciliationStage::Write,
                                        message,
                                        retryable: true,
                                    });
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    let guard = settings.read().await;
    settings_snapshot_from(&guard, reconciliation_error)
}

#[tauri::command]
pub async fn get_settings(settings: State<'_, SettingsState>) -> Result<SettingsSnapshot, String> {
    Ok(settings_snapshot_helper(settings.inner(), None).await)
}

/// #769 Phase 1 + #1318 - return the externalized coding-agent catalog for the onboarding
/// and Settings pick lists. Resolves against the PRIMARY registered project's
/// `.ac/coding-agents` catalog; with no registered project it falls back to the
/// legacy `<config_dir>/coding-agents` catalog when one exists (pre-migration
/// installs keep today's read behavior).
/// #1967 P4 contract: this is a persisted-only read. `Ok(vec![])` is honored
/// verbatim when the user removed every built-in, and `Err` (code/path/reason)
/// is returned when no readable persisted catalog exists - a missing or
/// unparseable `agents.json` is NEVER replaced with the embedded default, which
/// is seed material only. Warnings are logged here and carried structurally by
/// `get_coding_agent_catalog_report`. #1912: a `false` row in
/// `BUILTIN_AGENT_SUPPORT` (`src-tauri/src/config/coding_agents_catalog.rs`) is
/// already dropped via the persisted read gate.
#[tauri::command]
pub async fn get_coding_agent_catalog(
    settings: State<'_, SettingsState>,
) -> Result<Vec<crate::config::coding_agents_catalog::CodingAgentDefinition>, String> {
    coding_agent_catalog_inner(settings.inner()).await
}

/// #1551 - shared by the Tauri command and the WebSocket router. #1967 P4 -
/// resolves through the persisted-only report: a readable catalog is `Ok(Vec)`
/// (a valid empty list included) and an unavailable persisted source is `Err`
/// carrying the diagnostic's code/path/reason. The embedded default is NEVER
/// substituted on this path. Takes one settings snapshot and releases the async
/// settings lock BEFORE any filesystem work (mirrors
/// `coding_agent_catalog_report_inner`).
pub async fn coding_agent_catalog_inner(
    settings: &SettingsState,
) -> Result<Vec<crate::config::coding_agents_catalog::CodingAgentDefinition>, String> {
    let snapshot = settings.read().await.clone();
    crate::config::coding_agents_catalog::load_catalog_for_settings(&snapshot)
        .map_err(|unavailable| unavailable.to_string())
}

/// #1963 (P1) - read-only persisted-catalog report for the pre-migration UI.
/// Additive IPC: since P4 the array endpoint above resolves through the SAME
/// resolver, so both agree about availability and the persisted selection.
/// Always `Ok`; ordinary filesystem failure is carried by the report's
/// `unavailable` field, and `Err` is reserved for a transport fault.
#[tauri::command]
pub async fn get_coding_agent_catalog_report(
    settings: State<'_, SettingsState>,
) -> Result<crate::config::coding_agents_catalog::CatalogReport, String> {
    Ok(coding_agent_catalog_report_inner(settings.inner()).await)
}

/// #1963 (P1) - shared by the Tauri command and the WebSocket router. Takes one
/// settings snapshot and releases the async settings lock BEFORE any filesystem
/// work, so the report resolver never holds the settings read guard across I/O.
/// Read-only: never seeds, refreshes or writes.
pub async fn coding_agent_catalog_report_inner(
    settings: &SettingsState,
) -> crate::config::coding_agents_catalog::CatalogReport {
    let snapshot = settings.read().await.clone();
    crate::config::coding_agents_catalog::load_catalog_report_for_settings(&snapshot)
}

/// #769 Phase 2 - the coding-agent command executable basenames that ship a
/// re-seedable default config-folder master (`claude`, `codex`, `opencode`). The
/// frontend enables the "Re-seed default configuration" button only for a catalog
/// def whose command reduces (exact basename) to one of these; the reseed command
/// re-checks server-side.
#[tauri::command]
pub fn list_reseedable_agent_commands() -> Vec<String> {
    crate::config::coding_agents_catalog::reseedable_command_basenames()
}

/// #769 Phase 2 + #1318 - restore a built-in's shipped default config-folder master (the
/// Settings "Re-seed default configuration" button). Resolves the master from the
/// PRIMARY registered project's `.ac/coding-agents/_seed/<dest>`; with NO primary
/// root it falls back to the legacy `<config_dir>/coding-agents/_seed/<dest>`
/// masters (pre-migration installs keep the button working). Gating is re-checked
/// server-side (exact executable basename, never `starts_with`, so `pi`/`agent`
/// cannot false-match): `command` must map to a built-in that ships a master, else
/// `Err`. On success the current master is `.bak`ed first, then atomically
/// replaced with the embedded default; it takes effect on NEW sessions via the
/// absent-only fill. Running replicas and their live config are untouched.
#[tauri::command]
pub async fn reseed_coding_agent_default(
    settings: State<'_, SettingsState>,
    command: String,
) -> Result<crate::config::coding_agents_catalog::ReseedResult, String> {
    let settings = settings.read().await;
    let ac_dir = crate::config::coding_agents_catalog::primary_project_root(&settings)
        .map(|root| root.join(".ac"));
    match ac_dir {
        Some(ac_dir) => {
            crate::config::coding_agents_catalog::reseed_master_for_command(&ac_dir, &command)
        }
        None => {
            let config_dir = crate::config::config_dir().ok_or("No config dir")?;
            crate::config::coding_agents_catalog::reseed_master_for_command(&config_dir, &command)
        }
    }
}

#[tauri::command]
pub async fn update_settings(
    app: AppHandle,
    settings: State<'_, SettingsState>,
    new_settings: AppSettings,
) -> Result<(), String> {
    let saved = persist_protected_settings_update(settings.inner(), new_settings).await?;
    purge_sessions_after_settings_update(&saved).await;
    // #714 re-register the (possibly changed) hotkey. Syntax was already validated
    // before persistence; an OS-level registration conflict must NOT turn this
    // successfully-persisted save into an Err. Surface it as a visible event.
    if let Err(e) =
        crate::screenshot::register_configured_hotkey(&app, &saved.screenshot_capture_hotkey)
    {
        let _ = app.emit(
            "screenshot_capture_failed",
            serde_json::json!({
                "message": format!("Screenshot hotkey was saved but could not be registered: {}", e)
            }),
        );
    }
    Ok(())
}

#[tauri::command]
pub async fn save_settings_draft(
    app: AppHandle,
    settings: State<'_, SettingsState>,
    draft: AppSettings,
) -> Result<(), String> {
    let (saved, events) = persist_settings_draft_update(settings.inner(), draft).await?;
    purge_sessions_after_settings_update(&saved).await;
    // #714 same non-failing hotkey re-registration as update_settings: persisted
    // save stays successful even if the OS refuses the hotkey.
    if let Err(e) =
        crate::screenshot::register_configured_hotkey(&app, &saved.screenshot_capture_hotkey)
    {
        let _ = app.emit(
            "screenshot_capture_failed",
            serde_json::json!({
                "message": format!("Screenshot hotkey was saved but could not be registered: {}", e)
            }),
        );
    }
    emit_settings_draft_update_events(&app, &events);
    // #612 apply the (possibly changed) log level live + broadcast so every
    // webview re-applies its console gate. Idempotent and cheap; runs only on an
    // explicit Settings Save. `saved` is the binding already destructured from
    // the persist call above, so this does NOT re-persist.
    let level = crate::logging::normalize_log_level(saved.log_level.as_deref().unwrap_or("info"))
        .unwrap_or("info");
    apply_and_broadcast_log_level(&app, level);
    Ok(())
}

#[tauri::command]
pub async fn set_terminal_snapshots_enabled(
    settings: State<'_, SettingsState>,
    expected: bool,
    enabled: bool,
) -> Result<(), String> {
    set_terminal_snapshots_enabled_inner(settings.inner(), expected, enabled).await
}

/// Shared #1173 owner used by native Tauri and browser WebSocket transports.
/// Whole-settings writers preserve this field and cannot call this owner
/// without an explicit expected value.
pub(crate) async fn set_terminal_snapshots_enabled_inner(
    settings: &SettingsState,
    expected: bool,
    enabled: bool,
) -> Result<(), String> {
    let mut guard = settings.write().await;
    let written = crate::config::settings::compare_and_set_terminal_snapshots_enabled(
        &guard, expected, enabled,
    )?;
    *guard = written;
    Ok(())
}

pub(crate) async fn persist_protected_settings_update(
    settings: &SettingsState,
    new_settings: AppSettings,
) -> Result<AppSettings, String> {
    persist_protected_settings_update_with_saver(settings, new_settings, save_settings).await
}

async fn persist_protected_settings_update_with_saver(
    settings: &SettingsState,
    new_settings: AppSettings,
    save: impl FnOnce(&AppSettings) -> Result<AppSettings, String>,
) -> Result<AppSettings, String> {
    let mut s = settings.write().await;
    let current = s.clone();
    let candidate = build_protected_settings_candidate(&current, new_settings)?;
    let written = save(&candidate)?;
    *s = written.clone();
    Ok(written)
}

/// #2306 P2 - make the live effective order authoritative over a whole-settings
/// payload, without letting the payload's explicit ordinals reorder anything.
///
/// Both whole-settings writers call this after their protected-field restores
/// and before validation, event derivation and save. The live in-memory vector
/// is the effective order; the incoming payload may be a snapshot that predates
/// a move, so its `order` values carry no authority:
///
///   1. every incoming record whose exact id exists live survives, ordered by
///      its live position (records sharing a live rank keep incoming order);
///   2. incoming-only ids are appended in incoming order;
///   3. records and membership come from the incoming payload, so a user edit,
///      add or remove still applies;
///   4. `order` is finally renumbered by post-merge vector position.
///
/// A stale draft/update therefore cannot undo an ordered move for a surviving
/// id, and a forged incoming ordinal cannot sort the vector. Known limitation
/// (accepted, per the phase plan): membership is still the incoming payload's,
/// so a stale snapshot that predates an independent add omits that id here;
/// this merge does not resurrect it.
fn merge_live_agent_order(current: &AppSettings, incoming: &mut AppSettings) {
    let mut survivors: Vec<(usize, AgentConfig)> = Vec::new();
    let mut appended: Vec<AgentConfig> = Vec::new();
    for record in incoming.agents.drain(..) {
        match current.agents.iter().position(|live| live.id == record.id) {
            Some(live_rank) => survivors.push((live_rank, record)),
            None => appended.push(record),
        }
    }
    // Stable sort: records sharing a live rank (duplicate ids) keep incoming order.
    survivors.sort_by_key(|(live_rank, _)| *live_rank);
    let mut merged: Vec<AgentConfig> = survivors.into_iter().map(|(_, record)| record).collect();
    merged.extend(appended);
    incoming.agents = merged;
    normalize_agent_order(&mut incoming.agents);
}

fn build_protected_settings_candidate(
    current: &AppSettings,
    new_settings: AppSettings,
) -> Result<AppSettings, String> {
    let mut candidate = merge_protected_coding_agent_settings(current, new_settings);
    // Preserve existing root token. Frontend settings payloads cannot overwrite it.
    candidate.root_token = current.root_token.clone();
    // #881 A7 / R2-G1: project lists are disk-authoritative and are mutated
    // only by dedicated project commands. A settings payload from the GUI, CLI,
    // or API must never carry authority for them, so restore all three from
    // live memory before repair.
    //
    // This is not the prohibited disk re-read. The preserving writer still
    // overwrites all three from disk whenever disk has an opinion, so disk
    // authority is untouched. These copies only matter on the no-disk-truth
    // arms where a stale client would otherwise publish an empty list.
    candidate.project_paths = current.project_paths.clone();
    candidate.project_path = current.project_path.clone();
    candidate.archived_project_paths = current.archived_project_paths.clone();
    // #1077: restore the hidden project-path pair state too, so a returned
    // SettingsSnapshot echoed back as an update cannot erase or re-pair the
    // companion metadata. The disk serializer builds project fields only from
    // this protected state (or disk truth), never from the incoming payload.
    candidate.project_path_state = current.project_path_state.clone();
    // #1737: `local_overlay_state` is `#[serde(skip)]`, so a payload that arrived from
    // the webview or the WebSocket transport decodes it as an empty overlay. Restore
    // it from live memory, exactly like `project_path_state` above: without this the
    // save that follows would find nothing to restore, would write the overlay value
    // into settings.json, and `*s = written.clone()` would install the empty overlay
    // so every later save is unprotected too. General rule (plan D14): any AppSettings
    // that did not come out of `parse_settings_json` and is later saved must inherit
    // the live overlay state first.
    candidate.local_overlay_state = current.local_overlay_state.clone();
    // #965: rail collapse is mutated only by `set_rail_collapse`. A settings payload
    // from the GUI, CLI, or API must never carry authority for it, so restore both
    // fields from live memory. Same rule and same mechanism as the project lists
    // above; this is not the prohibited disk re-read.
    candidate.rail_collapsed_projects = current.rail_collapsed_projects.clone();
    candidate.rail_favorites_collapsed = current.rail_favorites_collapsed;
    // #1173: terminal snapshot disclosure is owned only by its dedicated CAS.
    candidate.terminal_snapshots_enabled = current.terminal_snapshots_enabled;
    merge_live_agent_order(current, &mut candidate);
    validate_and_repair_settings(&mut candidate)?;
    Ok(candidate)
}

pub(crate) async fn persist_settings_draft_update(
    settings: &SettingsState,
    draft: AppSettings,
) -> Result<(AppSettings, SettingsDraftUpdateEvents), String> {
    persist_settings_draft_update_with_saver(settings, draft, save_settings).await
}

async fn persist_settings_draft_update_with_saver(
    settings: &SettingsState,
    mut draft: AppSettings,
    save: impl FnOnce(&AppSettings) -> Result<AppSettings, String>,
) -> Result<(AppSettings, SettingsDraftUpdateEvents), String> {
    let mut s = settings.write().await;
    let current = s.clone();
    draft.root_token = current.root_token.clone();
    // #881 A7 / R2-G1: same protected-list restore as the whole settings
    // publisher above. This is an in-memory copy from the held settings guard,
    // not a second disk read, and it only affects the no-disk-truth arms.
    draft.project_paths = current.project_paths.clone();
    draft.project_path = current.project_path.clone();
    draft.archived_project_paths = current.archived_project_paths.clone();
    // #1077: restore the hidden project-path pair state (see the protected
    // candidate builder above).
    draft.project_path_state = current.project_path_state.clone();
    // #1737: the same overlay-state carry-over as in
    // `build_protected_settings_candidate` above, and for the same reason (plan D14).
    draft.local_overlay_state = current.local_overlay_state.clone();
    // #965: same protect as `build_protected_settings_candidate`. The SettingsModal
    // Save path lands here, and so does every whole-object writer that reads
    // `settingsStore.current` first (window geometry, zoom, titlebar...). Rail
    // collapse is owned by `set_rail_collapse`; restore it from live memory.
    draft.rail_collapsed_projects = current.rail_collapsed_projects.clone();
    draft.rail_favorites_collapsed = current.rail_favorites_collapsed;
    // #1173: a stale whole-settings draft has no disclosure-gate authority.
    draft.terminal_snapshots_enabled = current.terminal_snapshots_enabled;
    merge_live_agent_order(&current, &mut draft);
    validate_and_repair_settings(&mut draft)?;
    let events = settings_draft_update_events(&current, &draft);
    let written = save(&draft)?;
    *s = written.clone();
    Ok((written, events))
}

pub(crate) async fn purge_sessions_after_settings_update(saved: &AppSettings) {
    let Some(dir) = crate::config::config_dir() else {
        log::warn!("Could not determine home directory for session purge after settings update");
        return;
    };

    if let Err(e) = purge_sessions_after_settings_update_in_dir(saved, &dir).await {
        log::warn!(
            "[settings] Failed to purge sessions outside current projectPaths after settings update: {}",
            e
        );
    }
}

async fn purge_sessions_after_settings_update_in_dir(
    saved: &AppSettings,
    dir: &Path,
) -> Result<(), String> {
    let retention_paths =
        crate::config::sessions_persistence::session_retention_project_paths(saved);
    crate::config::sessions_persistence::purge_sessions_outside_project_paths_in_dir(
        dir,
        &retention_paths,
    )
    .await
    .map(|_| ())
}

fn settings_draft_update_events(
    before: &AppSettings,
    after: &AppSettings,
) -> SettingsDraftUpdateEvents {
    let before_envs = agent_env_settings_by_id(before);
    let after_envs = agent_env_settings_by_id(after);
    let mut env_agent_ids = Vec::new();

    for agent_id in before_envs.keys().chain(after_envs.keys()) {
        if env_agent_ids.iter().any(|existing| existing == agent_id) {
            continue;
        }
        if before_envs.get(agent_id) != after_envs.get(agent_id) {
            env_agent_ids.push(agent_id.clone());
        }
    }

    SettingsDraftUpdateEvents {
        // #592/#597 - the drift content-hash fingerprints the EFFECTIVE command
        // (agent base command + cell command, via compose_effective_command), so a
        // bare agent-base-command edit (e.g. `claude` -> `claude-amp -c ...`) is real
        // drift even when the profile cells are untouched. It is neither a
        // coding_agent_profiles change nor an env change, so without folding it in
        // here the SettingsModal save path emits NO refresh event and the outdated
        // badge never updates. coding_agent_profiles_updated drives
        // refreshProfileOutdated in the sidebar, so route base-command edits through it.
        profiles_changed: before.coding_agent_profiles != after.coding_agent_profiles
            || agent_commands_by_id(before) != agent_commands_by_id(after),
        env_agent_ids,
    }
}

fn agent_env_settings_by_id(
    settings: &AppSettings,
) -> BTreeMap<String, (Vec<CodingAgentEnv>, bool)> {
    settings
        .agents
        .iter()
        .map(|agent| (agent.id.clone(), (agent.envs.clone(), agent.isolated_home)))
        .collect()
}

/// #592/#597 - per-agent base command, the other half of the drift hash input
/// (`compose_effective_command(agent.command, cell.command)`). A change here must
/// trigger a drift refresh just like a profile-cell edit.
fn agent_commands_by_id(settings: &AppSettings) -> BTreeMap<String, String> {
    settings
        .agents
        .iter()
        .map(|agent| (agent.id.clone(), agent.command.clone()))
        .collect()
}

fn emit_settings_draft_update_events(app: &AppHandle, events: &SettingsDraftUpdateEvents) {
    if events.profiles_changed {
        // #592/#597: the SettingsModal "Save" persists profile cells AND agent base
        // commands through save_settings_draft (NOT update_coding_agent_profiles), so
        // THIS is the emit point the user's normal edit flow actually reaches. Kept at
        // debug to confirm the refresh fired without spamming app.log.
        log::debug!(
            "[profile-hash] profile-save (save_settings_draft): coding-agent config changed; emitting coding_agent_profiles_updated"
        );
        let _ = app.emit("coding_agent_profiles_updated", serde_json::json!({}));
    }

    for agent_id in &events.env_agent_ids {
        log::debug!(
            "[profile-hash] profile-save (save_settings_draft): agent env changed for {}; emitting coding_agent_env_settings_updated",
            agent_id
        );
        let _ = app.emit(
            "coding_agent_env_settings_updated",
            serde_json::json!({ "agentId": agent_id }),
        );
    }
}

#[tauri::command]
pub async fn update_coding_agent_profiles(
    app: AppHandle,
    settings: State<'_, SettingsState>,
    profiles: CodingAgentProfilesConfig,
) -> Result<(), String> {
    persist_coding_agent_profiles_update(settings.inner(), profiles).await?;
    let _ = app.emit("coding_agent_profiles_updated", serde_json::json!({}));
    Ok(())
}

async fn persist_coding_agent_profiles_update(
    settings: &SettingsState,
    profiles: CodingAgentProfilesConfig,
) -> Result<(), String> {
    let mut s = settings.write().await;
    let mut candidate = s.clone();
    candidate.coding_agent_profiles = profiles;
    validate_and_repair_settings(&mut candidate)?;
    let written = save_settings(&candidate)?;
    *s = written;
    Ok(())
}

#[tauri::command]
pub async fn update_coding_agent_env_settings(
    app: AppHandle,
    settings: State<'_, SettingsState>,
    agent_id: String,
    envs: Vec<CodingAgentEnv>,
    isolated_home: bool,
) -> Result<(), String> {
    persist_coding_agent_env_settings_update(settings.inner(), &agent_id, envs, isolated_home)
        .await?;
    let _ = app.emit(
        "coding_agent_env_settings_updated",
        serde_json::json!({ "agentId": agent_id }),
    );
    Ok(())
}

async fn persist_coding_agent_env_settings_update(
    settings: &SettingsState,
    agent_id: &str,
    envs: Vec<CodingAgentEnv>,
    isolated_home: bool,
) -> Result<(), String> {
    let mut s = settings.write().await;
    let mut candidate = s.clone();
    let agent = candidate
        .agents
        .iter_mut()
        .find(|agent| agent.id == agent_id)
        .ok_or_else(|| format!("Agent '{}' is not configured", agent_id))?;
    agent.envs = envs;
    agent.isolated_home = isolated_home;
    validate_and_repair_settings(&mut candidate)?;
    let written = save_settings(&candidate)?;
    *s = written;
    Ok(())
}

#[tauri::command]
pub async fn set_agent_default_profile(
    app: AppHandle,
    settings: State<'_, SettingsState>,
    agent_path: String,
    profile: String,
) -> Result<(), String> {
    let payload = set_agent_default_profile_inner(settings.inner(), &agent_path, &profile).await?;
    let _ = app.emit("coding_agent_profile_selection_updated", payload);
    Ok(())
}

/// Persist an origin agent's default profile and return the
/// `coding_agent_profile_selection_updated` event payload for the caller to
/// emit (desktop) or broadcast to web clients (browser transport). Shared by
/// the desktop command and the web dispatcher.
pub(crate) async fn set_agent_default_profile_inner(
    settings: &SettingsState,
    agent_path: &str,
    profile: &str,
) -> Result<serde_json::Value, String> {
    let snapshot = settings.read().await.clone();
    crate::config::coding_agent_profiles::set_agent_default_profile(
        &snapshot,
        std::path::Path::new(agent_path),
        profile,
    )?;
    Ok(serde_json::json!({ "agentPath": agent_path, "profile": profile, "scope": "default" }))
}

#[tauri::command]
pub async fn set_instance_profile_override(
    app: AppHandle,
    settings: State<'_, SettingsState>,
    agent_path: String,
    profile: Option<String>,
) -> Result<(), String> {
    // #1940 - settings snapshot, guarded profile write and event publication are
    // one owned selection operation, so this override cannot interleave with a
    // broad apply or a self-switch persist/restart.
    let app_handle = app.clone();
    let settings = settings.inner().clone();
    crate::session::selection::run_owned_selection_operation(move || {
        let app = app_handle.clone();
        async move {
            let payload =
                set_instance_profile_override_inner(&settings, &agent_path, profile.as_deref())
                    .await?;
            let _ = app.emit("coding_agent_profile_selection_updated", payload);
            Ok(())
        }
    })
    .await
}

/// Persist a replica's instance-level profile override (or clear it when
/// `profile` is `None`) and return the `coding_agent_profile_selection_updated`
/// event payload for the caller to emit or broadcast. Shared by the desktop
/// command and the web dispatcher.
pub(crate) async fn set_instance_profile_override_inner(
    settings: &SettingsState,
    agent_path: &str,
    profile: Option<&str>,
) -> Result<serde_json::Value, String> {
    let snapshot = settings.read().await.clone();
    crate::config::coding_agent_profiles::set_instance_profile_override(
        &snapshot,
        std::path::Path::new(agent_path),
        profile,
    )?;
    Ok(serde_json::json!({ "agentPath": agent_path, "profile": profile, "scope": "instance" }))
}

#[tauri::command]
pub async fn resolve_coding_agent_profile(
    settings: State<'_, SettingsState>,
    agent_path: Option<String>,
    agent_id: String,
    requested_profile: Option<String>,
) -> Result<CodingAgentProfileResolutionResult, String> {
    resolve_coding_agent_profile_inner(
        settings.inner(),
        agent_path.as_deref(),
        &agent_id,
        requested_profile.as_deref(),
    )
    .await
}

/// Resolve the effective coding-agent profile for an agent/replica. Shared by
/// the desktop command and the web dispatcher; reads settings only.
pub(crate) async fn resolve_coding_agent_profile_inner(
    settings: &SettingsState,
    agent_path: Option<&str>,
    agent_id: &str,
    requested_profile: Option<&str>,
) -> Result<CodingAgentProfileResolutionResult, String> {
    let snapshot = settings.read().await.clone();
    let agent_path = agent_path
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .map(std::path::PathBuf::from);
    let details = crate::config::coding_agent_profiles::resolve_profile_selection(
        &snapshot,
        agent_path.as_deref(),
        agent_id,
        requested_profile,
    )?;

    Ok(CodingAgentProfileResolutionResult {
        requested_profile: details.resolution.requested_profile,
        effective_profile: details.resolution.effective_profile,
        fallback_chain: details.resolution.fallback_chain,
        fallback_applied: details.resolution.fallback_applied,
        requested_profile_input: details.requested_profile_input,
        instance_profile_override: details.instance_profile_override,
        origin_default_profile: details.origin_default_profile,
        agent_default_profile: details.agent_default_profile,
        warnings: details.resolution.warnings,
    })
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase")]
pub enum ProfileAssignmentScope {
    Replica,
    Kind,
    Workgroup,
}

/// #1941 - requested publication policy. `Ordinary` preserves the stored lock
/// flag exactly; `AssignAndLock` publishes the pair and sets the flag, and is
/// the only mode that offers a reviewed force for protected replicas.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum AssignmentMode {
    #[default]
    Ordinary,
    AssignAndLock,
}

/// #1941 - explicit review decision for a bulk assign-and-lock that found
/// protected replicas. There is deliberately no default: a conflict needs a
/// choice, so the backend can never force a lock silently.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ConflictDecision {
    UnlockedOnly,
    ForceReviewed,
}

/// #1941 - strict protection state of a replica, as read by #1939. `Invalid`
/// is a diagnostic, never a permissive default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SelectionState {
    Unlocked,
    Locked,
    Invalid,
}

/// #1941 - the protected pair as exposed on the wire. Both fields are `null`
/// when no complete pair is stored.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedPair {
    pub coding_agent_id: Option<String>,
    pub requested_profile: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileAssignmentTarget {
    pub workgroup_name: String,
    pub workgroup_path: String,
    pub replica_name: String,
    pub replica_path: String,
    pub identity_path: String,
    pub origin_project: Option<String>,
    pub live_session_ids: Vec<String>,
    /// #1941 - the complete stored pair, or `null` when absent/invalid.
    pub saved_pair: Option<SavedPair>,
    /// #1941 - strict protection state; never a synthesized `false`.
    pub selection_state: SelectionState,
    /// #1941 - the strict-read diagnostic when `selection_state` is invalid.
    pub selection_error: Option<String>,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewCodingAgentProfileSelectionRequest {
    pub target_replica_path: String,
    pub coding_agent_id: String,
    pub profile: String,
    pub scope: ProfileAssignmentScope,
    #[serde(default)]
    pub restart_sessions: bool,
    /// #1941 - absent keeps the legacy ordinary semantics.
    #[serde(default)]
    pub assignment_mode: AssignmentMode,
}

/// #1941 - one explicit conflict outcome as offered by a preview.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SelectionDecisionProjection {
    pub fingerprint: String,
    pub eligible_paths: Vec<String>,
    pub eligible_count: usize,
    pub skipped_locked_count: usize,
    pub live_session_count: usize,
}

/// #1941 - both reviewable outcomes; present only when a bulk
/// assign-and-lock preview actually found protected candidates. Each carries
/// its own fingerprint, so the client echoes exactly the reviewed decision.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SelectionDecisions {
    pub unlocked_only: SelectionDecisionProjection,
    pub force_reviewed: SelectionDecisionProjection,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewCodingAgentProfileSelectionResult {
    pub scope: ProfileAssignmentScope,
    pub target_count: usize,
    pub live_session_count: usize,
    pub target_fingerprint: String,
    pub requires_explicit_confirmation: bool,
    pub targets: Vec<ProfileAssignmentTarget>,
    pub warnings: Vec<String>,
    /// #1941 - false when scoped membership could not be established from the
    /// directory walk (unreadable entries / unprovable identities).
    pub counts_complete: bool,
    pub candidate_count: usize,
    pub protected_count: usize,
    pub invalid_count: usize,
    /// #1941 - valid protected candidates for a bulk assign-and-lock; zero for
    /// every other mode.
    pub conflict_count: usize,
    pub decisions: Option<SelectionDecisions>,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplyCodingAgentProfileSelectionRequest {
    pub target_replica_path: String,
    pub coding_agent_id: String,
    pub profile: String,
    pub scope: ProfileAssignmentScope,
    pub restart_sessions: bool,
    pub confirmed_target_fingerprint: Option<String>,
    pub typed_confirmation: Option<String>,
    /// #1941 - absent keeps the legacy ordinary semantics.
    #[serde(default)]
    pub assignment_mode: AssignmentMode,
    /// #1941 - required only for a bulk assign-and-lock that found protected
    /// candidates. Rejected in every other mode/scope combination.
    #[serde(default)]
    pub conflict_decision: Option<ConflictDecision>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileAssignmentError {
    pub code: String,
    pub message: String,
    pub session_ids: Vec<String>,
    pub replica_paths: Vec<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplyCodingAgentProfileSelectionResult {
    pub scope: ProfileAssignmentScope,
    pub updated_count: usize,
    pub restarted_count: usize,
    pub updated_replica_paths: Vec<String>,
    pub restarted_session_ids: Vec<String>,
    pub destroyed_but_not_recreated_session_ids: Vec<String>,
    pub target_fingerprint: String,
    pub warnings: Vec<String>,
    pub errors: Vec<ProfileAssignmentError>,
    /// #1941 - false-to-true lock flips only.
    pub newly_protected_paths: Vec<String>,
    /// #1941 - valid protected replicas deliberately skipped by the policy.
    pub skipped_locked_paths: Vec<String>,
    /// #1941 - candidates whose strict protection state is invalid.
    pub invalid_paths: Vec<String>,
    /// #1941 - protected replicas left after the operation; `null` when the
    /// refresh cannot establish the total.
    pub locked_after_apply_count: Option<usize>,
    /// #1941 - true only when a reviewed force actually overwrote a protected
    /// target, never merely because a decision was selected.
    pub force_applied: bool,
}

// ── #1941 selection-lock removal and Matrix default wire contract ──────────

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PreviewSelectionLockRemovalRequest {
    pub target_replica_path: String,
    pub scope: ProfileAssignmentScope,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApplySelectionLockRemovalRequest {
    pub target_replica_path: String,
    pub scope: ProfileAssignmentScope,
    pub confirmed_target_fingerprint: String,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GetReplicaSelectionDefaultRequest {
    pub target_replica_path: String,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SetReplicaSelectionDefaultRequest {
    pub target_replica_path: String,
    pub coding_agent_id: String,
    pub requested_profile: String,
    pub selection_locked: bool,
    pub confirmed_default_fingerprint: String,
}

/// #1941 - Matrix creation default (`tooling.replicaSelectionDefault`).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SelectionDefault {
    pub coding_agent_id: String,
    pub requested_profile: String,
    pub selection_locked: bool,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewSelectionLockRemovalResult {
    pub scope: ProfileAssignmentScope,
    pub target_fingerprint: String,
    pub candidate_count: usize,
    pub counts_complete: bool,
    pub protected_count: usize,
    pub already_unlocked_count: usize,
    pub invalid_count: usize,
    pub targets: Vec<ProfileAssignmentTarget>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplySelectionLockRemovalResult {
    pub scope: ProfileAssignmentScope,
    pub target_fingerprint: String,
    pub removed_count: usize,
    pub removed_replica_paths: Vec<String>,
    pub already_unlocked_paths: Vec<String>,
    pub failed_replica_paths: Vec<String>,
    pub remaining_protected_count: Option<usize>,
    pub candidate_count: usize,
    pub counts_complete: bool,
    pub invalid_count: usize,
    pub errors: Vec<ProfileAssignmentError>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReplicaSelectionDefaultResult {
    pub target_replica_path: String,
    pub matrix_path: String,
    /// `null` only when the Matrix has no stored default (old creation
    /// behavior); a malformed default is an error, never a silent `null`.
    pub default: Option<SelectionDefault>,
    pub default_fingerprint: String,
    pub warnings: Vec<String>,
}

#[tauri::command]
pub async fn preview_coding_agent_profile_selection(
    session_mgr: State<'_, Arc<tokio::sync::RwLock<SessionManager>>>,
    settings: State<'_, SettingsState>,
    request: PreviewCodingAgentProfileSelectionRequest,
) -> Result<PreviewCodingAgentProfileSelectionResult, String> {
    // #1941 - a preview takes the shared operation turn for a consistent
    // operation boundary; its owned task survives a dropped or timed-out caller
    // (the transport discards the late response) without cancelling the read.
    let session_mgr = Arc::clone(session_mgr.inner());
    let settings = settings.inner().clone();
    crate::session::selection::run_owned_selection_operation(move || async move {
        preview_coding_agent_profile_selection_inner(&session_mgr, &settings, request).await
    })
    .await
}

/// Enumerate the replicas a broad-scope profile assignment would touch and
/// return the fingerprints the frontend echoes back on apply. Shared by the
/// desktop command and the web dispatcher; the caller owns the #1940 turn and
/// this function only reads settings, sessions and strict replica state.
pub(crate) async fn preview_coding_agent_profile_selection_inner(
    session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
    settings: &SettingsState,
    request: PreviewCodingAgentProfileSelectionRequest,
) -> Result<PreviewCodingAgentProfileSelectionResult, String> {
    let settings_snapshot = settings.read().await.clone();
    validate_profile_assignment_request(
        &settings_snapshot,
        &request.coding_agent_id,
        &request.profile,
    )?;
    let normalized_profile = normalize_profile_letter_for_assignment(&request.profile)?;
    let sessions = { session_mgr.read().await.list_sessions().await };
    let enumeration = enumerate_profile_assignment_targets(
        &settings_snapshot,
        Path::new(&request.target_replica_path),
        &request.scope,
        &sessions,
    )?;
    let snapshot = assignment_snapshot(
        &enumeration,
        &sessions,
        &request.scope,
        request.assignment_mode,
        &request.coding_agent_id,
        &normalized_profile,
        request.restart_sessions,
    )?;
    Ok(PreviewCodingAgentProfileSelectionResult {
        scope: request.scope.clone(),
        target_count: snapshot.top_eligible_count,
        live_session_count: snapshot.top_live_session_count,
        target_fingerprint: snapshot.top_fingerprint,
        requires_explicit_confirmation: profile_assignment_requires_explicit_confirmation(
            &request.scope,
            request.assignment_mode,
        ),
        targets: enumeration.targets(),
        warnings: enumeration.warnings,
        counts_complete: enumeration.counts_complete,
        candidate_count: enumeration.candidates.len(),
        protected_count: snapshot.protected_count,
        invalid_count: snapshot.invalid_count,
        conflict_count: snapshot.conflict_count,
        decisions: snapshot.decisions,
    })
}

#[tauri::command]
pub async fn apply_coding_agent_profile_selection(
    app: AppHandle,
    session_mgr: State<'_, Arc<tokio::sync::RwLock<SessionManager>>>,
    pty_mgr: State<'_, Arc<std::sync::Mutex<PtyManager>>>,
    settings: State<'_, SettingsState>,
    request: ApplyCodingAgentProfileSelectionRequest,
) -> Result<ApplyCodingAgentProfileSelectionResult, String> {
    // #1940 - the entire apply (enumeration, per-replica publication, every
    // restart submission, its settlement and classification) is one owned
    // selection operation. Dropping this caller detaches the owned task instead
    // of releasing the turn while an accepted restart still settles.
    let app_handle = app.clone();
    let session_mgr = Arc::clone(session_mgr.inner());
    let pty_mgr = Arc::clone(pty_mgr.inner());
    let settings = settings.inner().clone();
    crate::session::selection::run_owned_selection_operation(move || {
        let app = app_handle.clone();
        async move {
            let (result, payload) = apply_coding_agent_profile_selection_inner(
                &app,
                &session_mgr,
                &pty_mgr,
                &settings,
                request,
            )
            .await?;
            let _ = app.emit("coding_agent_profile_selection_updated", payload);
            Ok(result)
        }
    })
    .await
}

/// Apply a coding-agent profile assignment across the enumerated replicas
/// (optionally restarting live sessions) and return both the result and the
/// `coding_agent_profile_selection_updated` event payload. The caller emits the
/// payload (desktop) or broadcasts it to web clients. Shared by the desktop
/// command and the web dispatcher.
pub(crate) async fn apply_coding_agent_profile_selection_inner(
    app: &AppHandle,
    session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
    pty_mgr: &Arc<std::sync::Mutex<PtyManager>>,
    settings: &SettingsState,
    request: ApplyCodingAgentProfileSelectionRequest,
) -> Result<(ApplyCodingAgentProfileSelectionResult, serde_json::Value), String> {
    // #1940 - the caller owns the selection operation turn; this inner function
    // neither reacquires it nor spawns another turn, and it does not release it
    // before the last restart reply, classification and publication.
    //
    // #1941 - a conflict decision is only meaningful for a bulk assign-and-lock;
    // every other scope/mode rejects it before any enumeration or write.
    validate_assignment_decision_scope(
        &request.scope,
        request.assignment_mode,
        request.conflict_decision,
    )?;
    let settings_snapshot = settings.read().await.clone();
    validate_profile_assignment_request(
        &settings_snapshot,
        &request.coding_agent_id,
        &request.profile,
    )?;
    let normalized_profile = normalize_profile_letter_for_assignment(&request.profile)?;
    let sessions = { session_mgr.read().await.list_sessions().await };
    let enumeration = enumerate_profile_assignment_targets(
        &settings_snapshot,
        Path::new(&request.target_replica_path),
        &request.scope,
        &sessions,
    )?;
    // #1941 - re-enumerate the complete candidate set under the owned turn and
    // build fresh fingerprints from the strict per-candidate state; the client's
    // approval is compared against THIS snapshot before any write.
    let snapshot = assignment_snapshot(
        &enumeration,
        &sessions,
        &request.scope,
        request.assignment_mode,
        &request.coding_agent_id,
        &normalized_profile,
        request.restart_sessions,
    )?;
    let resolved_decision =
        resolve_assignment_conflict_decision(&snapshot, request.conflict_decision)?;
    let target_fingerprint = assignment_expected_fingerprint(&snapshot, resolved_decision)?;
    validate_profile_assignment_confirmation(
        &request.scope,
        request.assignment_mode,
        request.confirmed_target_fingerprint.as_deref(),
        &target_fingerprint,
    )?;

    let write_eligible_paths = assignment_write_paths(&snapshot, &request.scope, resolved_decision);
    if request.restart_sessions {
        let restart_targets: Vec<ProfileAssignmentTarget> = enumeration
            .candidates
            .iter()
            .filter(|candidate| {
                write_eligible_paths.contains(&canonical_compare_key(Path::new(
                    &candidate.target.replica_path,
                )))
            })
            .map(|candidate| candidate.target.clone())
            .collect();
        prevalidate_profile_assignment_restarts(
            &settings_snapshot,
            &request.coding_agent_id,
            &normalized_profile,
            &restart_targets,
        )?;
    }

    let intent =
        assignment_write_intent(&request.scope, request.assignment_mode, resolved_decision);
    let mut updated_replica_paths = Vec::new();
    let mut newly_protected_paths = Vec::new();
    let mut skipped_locked_paths = Vec::new();
    let mut invalid_paths = Vec::new();
    let mut write_succeeded_keys = BTreeSet::new();
    let mut errors = Vec::new();
    let mut warnings = enumeration.warnings.clone();
    let mut force_applied = false;
    for candidate in &enumeration.candidates {
        let target = &candidate.target;
        let key = canonical_compare_key(Path::new(&target.replica_path));
        if target.selection_state == SelectionState::Invalid {
            // #1941 - an invalid protection state is an explicit error with no
            // write and no restart; it is never read as an unlocked replica.
            invalid_paths.push(target.replica_path.clone());
            errors.push(ProfileAssignmentError {
                code: "invalidSelectionState".to_string(),
                message: target.selection_error.clone().unwrap_or_else(|| {
                    format!(
                        "Replica '{}' has an invalid selection state",
                        target.replica_path
                    )
                }),
                session_ids: target.live_session_ids.clone(),
                replica_paths: vec![target.replica_path.clone()],
            });
            continue;
        }
        if !write_eligible_paths.contains(&key) {
            if target.selection_state == SelectionState::Locked {
                // #1939/#1941 - ordinary and unlocked-only policies never force
                // a valid lock: skip it (no write, no restart) and report it.
                skipped_locked_paths.push(target.replica_path.clone());
                warnings.push(format!("Skipping locked replica '{}'", target.replica_path));
            }
            continue;
        }
        let expected = crate::config::coding_agent_profiles::ReplicaSelectionExpectation {
            identity: target.identity_path.clone(),
            pair: target_pair(target),
            locked: target.selection_state == SelectionState::Locked,
        };
        let pair = crate::config::coding_agent_profiles::ReplicaSelectionPair {
            coding_agent_id: request.coding_agent_id.clone(),
            requested_profile: normalized_profile.clone(),
        };
        // #1940/#1939 - the guarded callback re-reads the strict state and
        // compares this expectation inside the OS write lock, so a target that
        // changed after enumeration fails its own CAS instead of being
        // overwritten; earlier writes stay committed and accurately reported.
        match crate::config::coding_agent_profiles::write_replica_selection(
            &settings_snapshot,
            Path::new(&target.replica_path),
            &pair,
            intent,
            &expected,
        ) {
            Ok(outcome) if outcome.changed => {
                if outcome.lock_transition
                    == Some(crate::config::coding_agent_profiles::SelectionLockTransition::UnlockedToLocked)
                {
                    newly_protected_paths.push(target.replica_path.clone());
                }
                if intent
                    == crate::config::coding_agent_profiles::SelectionWriteIntent::BulkForceReviewed
                    && expected.locked
                {
                    // #1941 - forceApplied means a protected target was actually
                    // overwritten, never merely that a decision was selected.
                    force_applied = true;
                }
                updated_replica_paths.push(target.replica_path.clone());
                write_succeeded_keys.insert(key);
            }
            Ok(_unchanged) => warnings.push(format!(
                "Replica '{}' was already in the requested state",
                target.replica_path
            )),
            Err(e) => errors.push(ProfileAssignmentError {
                code: selection_error_code(&e).to_string(),
                message: e,
                session_ids: target.live_session_ids.clone(),
                replica_paths: vec![target.replica_path.clone()],
            }),
        }
    }

    let mut restarted_session_ids = Vec::new();
    let mut destroyed_but_not_recreated_session_ids = Vec::new();
    if request.restart_sessions && !write_succeeded_keys.is_empty() {
        for target in &enumeration.targets() {
            if !write_succeeded_keys
                .contains(&canonical_compare_key(Path::new(&target.replica_path)))
            {
                continue;
            }
            for session_id in &target.live_session_ids {
                let Ok(uuid) = uuid::Uuid::parse_str(session_id) else {
                    continue;
                };
                match crate::commands::session::restart_session_inner_with_intent(
                    app,
                    session_mgr,
                    pty_mgr,
                    settings,
                    uuid,
                    Some(request.coding_agent_id.clone()),
                    Some(normalized_profile.clone()),
                    Some(true),
                    false,
                    crate::session::selection::TrustedRestartIntent::Background,
                    None,
                    crate::config::sessions_persistence::default_creation_gate_enforcement(),
                )
                .await
                {
                    Ok(_) => restarted_session_ids.push(session_id.clone()),
                    Err(e) => {
                        let (code, destroyed_but_not_recreated) =
                            classify_restart_failure(session_mgr, uuid).await;
                        errors.push(ProfileAssignmentError {
                            code: code.to_string(),
                            message: e,
                            session_ids: vec![session_id.clone()],
                            replica_paths: vec![target.replica_path.clone()],
                        });
                        if destroyed_but_not_recreated {
                            destroyed_but_not_recreated_session_ids.push(session_id.clone());
                        }
                    }
                }
            }
        }
    }

    // #1941 - refresh under the same turn so the reported remaining protection
    // never comes from the pre-write snapshot; an unknowable total is `null`.
    let locked_after_apply_count = refresh_locked_count(&enumeration);
    let result = ApplyCodingAgentProfileSelectionResult {
        scope: request.scope.clone(),
        updated_count: updated_replica_paths.len(),
        restarted_count: restarted_session_ids.len(),
        updated_replica_paths,
        restarted_session_ids,
        destroyed_but_not_recreated_session_ids,
        target_fingerprint,
        warnings,
        errors,
        newly_protected_paths,
        skipped_locked_paths,
        invalid_paths,
        locked_after_apply_count,
        force_applied,
    };
    let operation = match request.assignment_mode {
        AssignmentMode::Ordinary => "assign",
        AssignmentMode::AssignAndLock => "assignAndLock",
    };
    let payload = serde_json::json!({
        "scope": request.scope,
        "operation": operation,
        "codingAgentId": request.coding_agent_id,
        "profile": normalized_profile,
        "updatedCount": result.updated_count,
        "restartedCount": result.restarted_count,
        "targetFingerprint": &result.target_fingerprint,
        "affectedPaths": &result.updated_replica_paths,
        "newlyProtectedCount": result.newly_protected_paths.len(),
        "skippedLockedCount": result.skipped_locked_paths.len(),
        "errors": &result.errors,
    });
    Ok((result, payload))
}

/// #1941 - one identified candidate plus its canonical Matrix dir, kept so a
/// fingerprint never has to re-resolve identity from a mutable path.
struct ProfileCandidate {
    target: ProfileAssignmentTarget,
    matrix_dir: PathBuf,
}

/// #1941 - the complete identified candidate set for one scope, together with
/// whether the directory walk could establish that set.
struct ProfileTargetEnumeration {
    anchor_dir: PathBuf,
    anchor_matrix_dir: PathBuf,
    candidates: Vec<ProfileCandidate>,
    warnings: Vec<String>,
    counts_complete: bool,
}

impl ProfileTargetEnumeration {
    fn targets(&self) -> Vec<ProfileAssignmentTarget> {
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
struct CandidateDirs {
    dirs: Vec<PathBuf>,
    warnings: Vec<String>,
    complete: bool,
}

impl CandidateDirs {
    fn new() -> Self {
        Self {
            dirs: Vec::new(),
            warnings: Vec::new(),
            complete: true,
        }
    }

    fn mark_incomplete(&mut self, warning: String) {
        self.warnings.push(warning);
        self.complete = false;
    }
}

fn validate_profile_assignment_request(
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

fn normalize_profile_letter_for_assignment(profile: &str) -> Result<String, String> {
    crate::config::settings::normalize_profile_letter(profile)
        .ok_or_else(|| "Profile must be a single letter A through Z".to_string())
}

fn profile_assignment_requires_explicit_confirmation(
    scope: &ProfileAssignmentScope,
    mode: AssignmentMode,
) -> bool {
    *scope != ProfileAssignmentScope::Replica || mode == AssignmentMode::AssignAndLock
}

/// #1941 - a conflict decision belongs only to a bulk assign-and-lock; anything
/// else is rejected before enumeration so no client boolean can widen force.
fn validate_assignment_decision_scope(
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

fn validate_profile_assignment_confirmation(
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
            "confirmationMissing: This operation requires the preview confirmation, and none was sent. Nothing was written."
                .to_string(),
        ),
        Some(_) => Err(
            "stalePreview: Target selection changed. Rerun preview before applying profile selection."
                .to_string(),
        ),
    }
}

fn prevalidate_profile_assignment_restarts(
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

async fn classify_restart_failure(
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
fn validate_selection_anchor(
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

fn enumerate_profile_assignment_targets(
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
    let mut counts_complete = candidate_dirs.complete;

    let anchor_matrix_key = canonical_compare_key(&anchor_matrix_dir);
    let live_by_cwd = live_sessions_by_cwd(sessions);
    let mut seen = BTreeSet::new();
    let mut candidates = Vec::new();

    // The anchor always belongs to its own scope; the walk dedupes against it.
    let anchor_state =
        crate::config::coding_agent_profiles::read_replica_selection_state(&anchor_dir);
    let (anchor_config, anchor_identity) =
        crate::config::replica_identity::read_wg_replica_config_read_only(&anchor_dir)
            .map_err(|e| format!("Target replica '{}': {}", anchor_dir.display(), e))?;
    let _ = anchor_config;
    seen.insert(canonical_compare_key(&anchor_dir));
    candidates.push(ProfileCandidate {
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
            counts_complete = false;
            continue;
        };
        let key = canonical_compare_key(&replica_dir);
        if !seen.insert(key) {
            continue;
        }
        let Ok((_config, identity)) =
            crate::config::replica_identity::read_wg_replica_config_read_only(&replica_dir)
        else {
            // #1941 - an unprovable identity inside the scope means membership
            // cannot be established: keep the diagnostic visible and refuse to
            // certify a scope total for it.
            warnings.push(format!(
                "Skipping invalid replica '{}'",
                replica_dir.display()
            ));
            counts_complete = false;
            continue;
        };
        if *scope == ProfileAssignmentScope::Kind
            && canonical_compare_key(&identity.matrix_dir) != anchor_matrix_key
        {
            // Homonymous matrices in other projects stay excluded: Kind means
            // the canonical origin Matrix, not the directory name.
            continue;
        }
        let state =
            crate::config::coding_agent_profiles::read_replica_selection_state(&replica_dir);
        let target = build_profile_assignment_target(&replica_dir, &identity, &state, &live_by_cwd);
        candidates.push(ProfileCandidate {
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
    Ok(ProfileTargetEnumeration {
        anchor_dir,
        anchor_matrix_dir,
        candidates,
        warnings,
        counts_complete,
    })
}

pub(crate) fn canonical_compare_key(path: &Path) -> String {
    let path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let mut text =
        crate::path_utils::path_to_string_without_windows_verbatim_prefix(&path).replace('\\', "/");
    if cfg!(windows) {
        text = text.to_ascii_lowercase();
    }
    text
}

fn canonical_real_dir(path: &Path, label: &str) -> Result<PathBuf, String> {
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|e| format!("{} '{}' is not readable: {}", label, path.display(), e))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(format!(
            "{} '{}' is not a real directory",
            label,
            path.display()
        ));
    }
    std::fs::canonicalize(path)
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

fn validate_wg_replica_path(path: &Path) -> Result<(), String> {
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

fn collect_replica_dirs_in_workgroup(wg_dir: &Path, out: &mut CandidateDirs) {
    let entries = match std::fs::read_dir(wg_dir) {
        Ok(entries) => entries,
        Err(e) => {
            out.mark_incomplete(format!("Failed to read room '{}': {}", wg_dir.display(), e));
            return;
        }
    };
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(e) => {
                out.mark_incomplete(format!(
                    "Failed to read an entry in room '{}': {}",
                    wg_dir.display(),
                    e
                ));
                continue;
            }
        };
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(e) => {
                out.mark_incomplete(format!(
                    "Failed to inspect an entry in room '{}': {}",
                    wg_dir.display(),
                    e
                ));
                continue;
            }
        };
        if !file_type.is_dir() {
            continue;
        }
        let name = entry.file_name();
        if name.to_string_lossy().starts_with("__agent_") {
            out.dirs.push(entry.path());
        }
    }
}

fn collect_kind_replica_dirs(ac_root: &Path, out: &mut CandidateDirs) {
    let entries = match std::fs::read_dir(ac_root) {
        Ok(entries) => entries,
        Err(e) => {
            out.mark_incomplete(format!(
                "Failed to read workspace '{}': {}",
                ac_root.display(),
                e
            ));
            return;
        }
    };
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(e) => {
                out.mark_incomplete(format!(
                    "Failed to read an entry in workspace '{}': {}",
                    ac_root.display(),
                    e
                ));
                continue;
            }
        };
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(e) => {
                out.mark_incomplete(format!(
                    "Failed to inspect an entry in workspace '{}': {}",
                    ac_root.display(),
                    e
                ));
                continue;
            }
        };
        if !file_type.is_dir()
            || !crate::config::entity_prefix::has_entity_prefix(
                &entry.file_name().to_string_lossy(),
            )
        {
            continue;
        }
        collect_replica_dirs_in_workgroup(&entry.path(), out);
    }
}

fn live_sessions_by_cwd(sessions: &[SessionInfo]) -> BTreeMap<String, Vec<String>> {
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

fn build_profile_assignment_target(
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
fn target_selection_wire(
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

fn saved_pair_wire(pair: &crate::config::coding_agent_profiles::ReplicaSelectionPair) -> SavedPair {
    SavedPair {
        coding_agent_id: Some(pair.coding_agent_id.clone()),
        requested_profile: Some(pair.requested_profile.clone()),
    }
}

fn target_pair(
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

// ── #1941 selection fingerprints ───────────────────────────────────────────
//
// Fingerprints are lowercase SHA-256 of a typed serde tuple with a schema tag.
// The complete candidate list (pair, flag, identity state and the live-session
// provider/profile identity) plus the eligible membership are part of the
// tuple, so a flag, pair, membership, anchor-identity or relevant session change
// invalidates an old approval. A conflicting bulk assign-and-lock offers one
// hash per decision; a no-conflict operation binds decision `null`.

const SELECTION_LOCK_FINGERPRINT_SCHEMA: &str = "selection-lock-v1";
const SELECTION_DEFAULT_FINGERPRINT_SCHEMA: &str = "replica-selection-default-v1";

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct SelectionFingerprintPair {
    coding_agent_id: String,
    requested_profile: String,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct SelectionFingerprintSession {
    id: String,
    coding_agent_id: Option<String>,
    requested_profile: Option<String>,
    effective_profile: Option<String>,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct SelectionFingerprintCandidate {
    path: String,
    matrix: String,
    pair: Option<SelectionFingerprintPair>,
    locked: bool,
    invalid: bool,
    live_sessions: Vec<SelectionFingerprintSession>,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct SelectionAssignmentFingerprint {
    schema: &'static str,
    operation: &'static str,
    anchor: String,
    matrix: String,
    scope: &'static str,
    coding_agent_id: String,
    requested_profile: String,
    restart_sessions: bool,
    decision: Option<&'static str>,
    candidates: Vec<SelectionFingerprintCandidate>,
    eligible_paths: Vec<String>,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct SelectionRemovalFingerprint {
    schema: &'static str,
    operation: &'static str,
    anchor: String,
    matrix: String,
    scope: &'static str,
    candidates: Vec<SelectionFingerprintCandidate>,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct SelectionFingerprintDefault {
    coding_agent_id: String,
    requested_profile: String,
    selection_locked: bool,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct SelectionDefaultFingerprint {
    schema: &'static str,
    matrix: String,
    default: Option<SelectionFingerprintDefault>,
}

fn sha256_hex<T: serde::Serialize>(value: &T) -> Result<String, String> {
    let serialized = serde_json::to_vec(value)
        .map_err(|e| format!("Failed to serialize selection fingerprint: {e}"))?;
    Ok(Sha256::digest(&serialized)
        .iter()
        .map(|byte| format!("{:02x}", byte))
        .collect())
}

fn selection_scope_wire(scope: &ProfileAssignmentScope) -> &'static str {
    match scope {
        ProfileAssignmentScope::Replica => "replica",
        ProfileAssignmentScope::Kind => "kind",
        ProfileAssignmentScope::Workgroup => "workgroup",
    }
}

fn conflict_decision_wire(decision: ConflictDecision) -> &'static str {
    match decision {
        ConflictDecision::UnlockedOnly => "unlockedOnly",
        ConflictDecision::ForceReviewed => "forceReviewed",
    }
}

fn live_session_identities(
    sessions: &[SessionInfo],
) -> BTreeMap<String, SelectionFingerprintSession> {
    sessions
        .iter()
        .map(|session| {
            (
                session.id.clone(),
                SelectionFingerprintSession {
                    id: session.id.clone(),
                    coding_agent_id: session.agent_id.clone(),
                    requested_profile: session.requested_profile.clone(),
                    effective_profile: session.effective_profile.clone(),
                },
            )
        })
        .collect()
}

fn fingerprint_candidates(
    enumeration: &ProfileTargetEnumeration,
    sessions: &[SessionInfo],
) -> Vec<SelectionFingerprintCandidate> {
    let live = live_session_identities(sessions);
    let mut candidates: Vec<SelectionFingerprintCandidate> = enumeration
        .candidates
        .iter()
        .map(|candidate| {
            let target = &candidate.target;
            let mut live_sessions: Vec<SelectionFingerprintSession> = target
                .live_session_ids
                .iter()
                .filter_map(|id| live.get(id).cloned())
                .collect();
            live_sessions.sort_by(|a, b| a.id.cmp(&b.id));
            SelectionFingerprintCandidate {
                path: canonical_compare_key(Path::new(&target.replica_path)),
                matrix: canonical_compare_key(&candidate.matrix_dir),
                pair: target_pair(target).map(|pair| SelectionFingerprintPair {
                    coding_agent_id: pair.coding_agent_id,
                    requested_profile: pair.requested_profile,
                }),
                locked: target.selection_state == SelectionState::Locked,
                invalid: target.selection_state == SelectionState::Invalid,
                live_sessions,
            }
        })
        .collect();
    candidates.sort_by(|a, b| a.path.cmp(&b.path));
    candidates
}

struct EligibilitySets {
    unlocked_paths: Vec<String>,
    valid_paths: Vec<String>,
}

fn eligibility_sets(enumeration: &ProfileTargetEnumeration) -> EligibilitySets {
    let mut unlocked_paths = Vec::new();
    let mut valid_paths = Vec::new();
    for candidate in &enumeration.candidates {
        let path = canonical_compare_key(Path::new(&candidate.target.replica_path));
        match candidate.target.selection_state {
            SelectionState::Invalid => {}
            SelectionState::Unlocked => {
                unlocked_paths.push(path.clone());
                valid_paths.push(path);
            }
            SelectionState::Locked => valid_paths.push(path),
        }
    }
    unlocked_paths.sort();
    valid_paths.sort();
    EligibilitySets {
        unlocked_paths,
        valid_paths,
    }
}

fn live_session_count_for(
    enumeration: &ProfileTargetEnumeration,
    eligible_paths: &[String],
) -> usize {
    enumeration
        .candidates
        .iter()
        .filter(|candidate| {
            eligible_paths.contains(&canonical_compare_key(Path::new(
                &candidate.target.replica_path,
            )))
        })
        .map(|candidate| candidate.target.live_session_ids.len())
        .sum()
}

#[allow(clippy::too_many_arguments)]
fn assignment_fingerprint(
    enumeration: &ProfileTargetEnumeration,
    sessions: &[SessionInfo],
    scope: &ProfileAssignmentScope,
    operation: &'static str,
    coding_agent_id: &str,
    requested_profile: &str,
    restart_sessions: bool,
    decision: Option<ConflictDecision>,
    eligible_paths: &[String],
) -> Result<String, String> {
    let tuple = SelectionAssignmentFingerprint {
        schema: SELECTION_LOCK_FINGERPRINT_SCHEMA,
        operation,
        anchor: canonical_compare_key(&enumeration.anchor_dir),
        matrix: canonical_compare_key(&enumeration.anchor_matrix_dir),
        scope: selection_scope_wire(scope),
        coding_agent_id: coding_agent_id.to_string(),
        requested_profile: requested_profile.to_string(),
        restart_sessions,
        decision: decision.map(conflict_decision_wire),
        candidates: fingerprint_candidates(enumeration, sessions),
        eligible_paths: eligible_paths.to_vec(),
    };
    sha256_hex(&tuple)
}

/// #1941 - a removal fingerprint binds only the operation, scope, anchor and
/// each candidate's pair/flag/identity state; removal has no session operation.
fn removal_fingerprint(
    enumeration: &ProfileTargetEnumeration,
    scope: &ProfileAssignmentScope,
) -> Result<String, String> {
    let tuple = SelectionRemovalFingerprint {
        schema: SELECTION_LOCK_FINGERPRINT_SCHEMA,
        operation: "unlock",
        anchor: canonical_compare_key(&enumeration.anchor_dir),
        matrix: canonical_compare_key(&enumeration.anchor_matrix_dir),
        scope: selection_scope_wire(scope),
        candidates: fingerprint_candidates(enumeration, &[]),
    };
    sha256_hex(&tuple)
}

fn selection_default_fingerprint(
    matrix_dir: &Path,
    default: Option<&crate::config::coding_agent_profiles::ReplicaSelectionDefault>,
) -> Result<String, String> {
    let tuple = SelectionDefaultFingerprint {
        schema: SELECTION_DEFAULT_FINGERPRINT_SCHEMA,
        matrix: canonical_compare_key(matrix_dir),
        default: default.map(|default| SelectionFingerprintDefault {
            coding_agent_id: default.coding_agent_id.clone(),
            requested_profile: default.requested_profile.clone(),
            selection_locked: default.selection_locked,
        }),
    };
    sha256_hex(&tuple)
}

struct AssignmentSnapshot {
    direct_fingerprint: String,
    decisions: Option<SelectionDecisions>,
    conflict_count: usize,
    protected_count: usize,
    invalid_count: usize,
    top_fingerprint: String,
    top_eligible_count: usize,
    top_live_session_count: usize,
    eligibility: EligibilitySets,
}

fn assignment_snapshot(
    enumeration: &ProfileTargetEnumeration,
    sessions: &[SessionInfo],
    scope: &ProfileAssignmentScope,
    mode: AssignmentMode,
    coding_agent_id: &str,
    requested_profile: &str,
    restart_sessions: bool,
) -> Result<AssignmentSnapshot, String> {
    let eligibility = eligibility_sets(enumeration);
    let protected_count = enumeration
        .candidates
        .iter()
        .filter(|candidate| candidate.target.selection_state == SelectionState::Locked)
        .count();
    let invalid_count = enumeration
        .candidates
        .iter()
        .filter(|candidate| candidate.target.selection_state == SelectionState::Invalid)
        .count();
    let bulk = *scope != ProfileAssignmentScope::Replica;
    let conflict_count = if bulk && mode == AssignmentMode::AssignAndLock {
        protected_count
    } else {
        0
    };
    let operation = match mode {
        AssignmentMode::Ordinary => "assign",
        AssignmentMode::AssignAndLock => "assignAndLock",
    };
    // A deliberate replica target counts whenever it is valid; a bulk ordinary
    // assignment is actionable only for unlocked replicas.
    let direct_eligible = if *scope == ProfileAssignmentScope::Replica {
        eligibility.valid_paths.clone()
    } else {
        eligibility.unlocked_paths.clone()
    };
    let direct_fingerprint = assignment_fingerprint(
        enumeration,
        sessions,
        scope,
        operation,
        coding_agent_id,
        requested_profile,
        restart_sessions,
        None,
        &direct_eligible,
    )?;
    let decisions = if conflict_count > 0 {
        let unlocked_only = SelectionDecisionProjection {
            fingerprint: assignment_fingerprint(
                enumeration,
                sessions,
                scope,
                operation,
                coding_agent_id,
                requested_profile,
                restart_sessions,
                Some(ConflictDecision::UnlockedOnly),
                &eligibility.unlocked_paths,
            )?,
            eligible_paths: eligibility.unlocked_paths.clone(),
            eligible_count: eligibility.unlocked_paths.len(),
            skipped_locked_count: protected_count,
            live_session_count: live_session_count_for(enumeration, &eligibility.unlocked_paths),
        };
        let force_reviewed = SelectionDecisionProjection {
            fingerprint: assignment_fingerprint(
                enumeration,
                sessions,
                scope,
                operation,
                coding_agent_id,
                requested_profile,
                restart_sessions,
                Some(ConflictDecision::ForceReviewed),
                &eligibility.valid_paths,
            )?,
            eligible_paths: eligibility.valid_paths.clone(),
            eligible_count: eligibility.valid_paths.len(),
            skipped_locked_count: 0,
            live_session_count: live_session_count_for(enumeration, &eligibility.valid_paths),
        };
        Some(SelectionDecisions {
            unlocked_only,
            force_reviewed,
        })
    } else {
        None
    };
    let (top_fingerprint, top_eligible_paths) = match &decisions {
        Some(decisions) => (
            decisions.unlocked_only.fingerprint.clone(),
            decisions.unlocked_only.eligible_paths.clone(),
        ),
        None => (direct_fingerprint.clone(), direct_eligible),
    };
    let top_live_session_count = live_session_count_for(enumeration, &top_eligible_paths);
    Ok(AssignmentSnapshot {
        direct_fingerprint,
        decisions,
        conflict_count,
        protected_count,
        invalid_count,
        top_fingerprint,
        top_eligible_count: top_eligible_paths.len(),
        top_live_session_count,
        eligibility,
    })
}

fn resolve_assignment_conflict_decision(
    snapshot: &AssignmentSnapshot,
    requested: Option<ConflictDecision>,
) -> Result<Option<ConflictDecision>, String> {
    if snapshot.conflict_count == 0 {
        if requested.is_some() {
            return Err(
                "conflictDecision is not required because no protected replica is in scope"
                    .to_string(),
            );
        }
        return Ok(None);
    }
    Ok(Some(requested.ok_or_else(|| {
        "conflictDecisionRequired: protected replicas are in scope; choose unlockedOnly or forceReviewed"
            .to_string()
    })?))
}

fn assignment_expected_fingerprint(
    snapshot: &AssignmentSnapshot,
    decision: Option<ConflictDecision>,
) -> Result<String, String> {
    match decision {
        None => Ok(snapshot.direct_fingerprint.clone()),
        Some(ConflictDecision::UnlockedOnly) => snapshot
            .decisions
            .as_ref()
            .map(|decisions| decisions.unlocked_only.fingerprint.clone())
            .ok_or_else(|| "selection decision projection is missing".to_string()),
        Some(ConflictDecision::ForceReviewed) => snapshot
            .decisions
            .as_ref()
            .map(|decisions| decisions.force_reviewed.fingerprint.clone())
            .ok_or_else(|| "selection decision projection is missing".to_string()),
    }
}

fn assignment_write_paths(
    snapshot: &AssignmentSnapshot,
    scope: &ProfileAssignmentScope,
    decision: Option<ConflictDecision>,
) -> Vec<String> {
    if *scope == ProfileAssignmentScope::Replica
        || decision == Some(ConflictDecision::ForceReviewed)
    {
        snapshot.eligibility.valid_paths.clone()
    } else {
        snapshot.eligibility.unlocked_paths.clone()
    }
}

fn assignment_write_intent(
    scope: &ProfileAssignmentScope,
    mode: AssignmentMode,
    decision: Option<ConflictDecision>,
) -> crate::config::coding_agent_profiles::SelectionWriteIntent {
    use crate::config::coding_agent_profiles::SelectionWriteIntent;
    match (scope, mode) {
        (ProfileAssignmentScope::Replica, AssignmentMode::Ordinary) => {
            SelectionWriteIntent::Individual
        }
        (ProfileAssignmentScope::Replica, AssignmentMode::AssignAndLock) => {
            SelectionWriteIntent::IndividualAssignLock
        }
        (_, AssignmentMode::Ordinary) => SelectionWriteIntent::BulkOrdinary,
        (_, AssignmentMode::AssignAndLock) => match decision {
            Some(ConflictDecision::ForceReviewed) => SelectionWriteIntent::BulkForceReviewed,
            _ => SelectionWriteIntent::BulkAssignLockUnlockedOnly,
        },
    }
}

/// #1941 - classify a guarded-write error into the wire error vocabulary while
/// preserving the original message and paths.
fn selection_error_code(message: &str) -> &'static str {
    if message.starts_with("stale replica selection") {
        "stalePreview"
    } else if message.starts_with("invalid replica selection") {
        "invalidSelectionState"
    } else if message.contains("configLockTimeout") {
        "configLockTimeout"
    } else {
        "configWriteFailed"
    }
}

/// #1941 - count the protected replicas left after an operation. `None` means
/// the total cannot be established (incomplete membership or an invalid state),
/// never a false zero.
fn refresh_locked_count(enumeration: &ProfileTargetEnumeration) -> Option<usize> {
    if !enumeration.counts_complete {
        return None;
    }
    let mut count = 0;
    for candidate in &enumeration.candidates {
        match crate::config::coding_agent_profiles::read_replica_selection_state(Path::new(
            &candidate.target.replica_path,
        )) {
            crate::config::coding_agent_profiles::ReplicaSelectionState::Unlocked { .. } => {}
            crate::config::coding_agent_profiles::ReplicaSelectionState::Locked { .. } => {
                count += 1
            }
            crate::config::coding_agent_profiles::ReplicaSelectionState::Invalid { .. } => {
                return None
            }
        }
    }
    Some(count)
}

// ── #1941 selection-lock removal commands ────────────────────────────────

#[tauri::command]
pub async fn preview_selection_lock_removal(
    session_mgr: State<'_, Arc<tokio::sync::RwLock<SessionManager>>>,
    settings: State<'_, SettingsState>,
    request: PreviewSelectionLockRemovalRequest,
) -> Result<PreviewSelectionLockRemovalResult, String> {
    let session_mgr = Arc::clone(session_mgr.inner());
    let settings = settings.inner().clone();
    crate::session::selection::run_owned_selection_operation(move || async move {
        preview_selection_lock_removal_inner(&session_mgr, &settings, request).await
    })
    .await
}

/// Enumerate the removal candidates and compute the fingerprint the client
/// echoes back on apply. Reads only; the caller owns the #1940 turn.
pub(crate) async fn preview_selection_lock_removal_inner(
    session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
    settings: &SettingsState,
    request: PreviewSelectionLockRemovalRequest,
) -> Result<PreviewSelectionLockRemovalResult, String> {
    let settings_snapshot = settings.read().await.clone();
    let sessions = { session_mgr.read().await.list_sessions().await };
    let enumeration = enumerate_profile_assignment_targets(
        &settings_snapshot,
        Path::new(&request.target_replica_path),
        &request.scope,
        &sessions,
    )?;
    let target_fingerprint = removal_fingerprint(&enumeration, &request.scope)?;
    let protected_count = enumeration
        .candidates
        .iter()
        .filter(|candidate| candidate.target.selection_state == SelectionState::Locked)
        .count();
    let already_unlocked_count = enumeration
        .candidates
        .iter()
        .filter(|candidate| candidate.target.selection_state == SelectionState::Unlocked)
        .count();
    let invalid_count = enumeration
        .candidates
        .iter()
        .filter(|candidate| candidate.target.selection_state == SelectionState::Invalid)
        .count();
    Ok(PreviewSelectionLockRemovalResult {
        scope: request.scope,
        target_fingerprint,
        candidate_count: enumeration.candidates.len(),
        counts_complete: enumeration.counts_complete,
        protected_count,
        already_unlocked_count,
        invalid_count,
        targets: enumeration.targets(),
        warnings: enumeration.warnings,
    })
}

#[tauri::command]
pub async fn apply_selection_lock_removal<R: tauri::Runtime>(
    app: AppHandle<R>,
    settings: State<'_, SettingsState>,
    request: ApplySelectionLockRemovalRequest,
) -> Result<ApplySelectionLockRemovalResult, String> {
    // #1940/#1941 - the whole removal (prevalidation re-read, every guarded
    // unlock and the final refresh) is one owned operation; the event is emitted
    // before the turn is released, so no later bulk can restart behind it.
    let app_handle = app.clone();
    let settings = settings.inner().clone();
    crate::session::selection::run_owned_selection_operation(move || {
        let app = app_handle.clone();
        async move {
            let (result, payload) = apply_selection_lock_removal_inner(&settings, request).await?;
            let _ = app.emit("coding_agent_profile_selection_updated", payload);
            Ok(result)
        }
    })
    .await
}

/// Remove selection locks for the enumerated candidates. Already-unlocked
/// targets are classified from the validated read and never rewritten; only
/// protected candidates reach `clear_replica_selection_lock`, each behind its
/// own per-file CAS. Removal never assigns and never requests a restart.
pub(crate) async fn apply_selection_lock_removal_inner(
    settings: &SettingsState,
    request: ApplySelectionLockRemovalRequest,
) -> Result<(ApplySelectionLockRemovalResult, serde_json::Value), String> {
    let settings_snapshot = settings.read().await.clone();
    let enumeration = enumerate_profile_assignment_targets(
        &settings_snapshot,
        Path::new(&request.target_replica_path),
        &request.scope,
        // Removal has no session operation, so live-session identity is not part
        // of its fingerprint and no session state is read.
        &[],
    )?;
    let target_fingerprint = removal_fingerprint(&enumeration, &request.scope)?;
    if request.confirmed_target_fingerprint != target_fingerprint {
        return Err(
            "stalePreview: Target selection changed. Rerun preview before removing selection locks."
                .to_string(),
        );
    }
    let mut removed_replica_paths = Vec::new();
    let mut already_unlocked_paths = Vec::new();
    let mut failed_replica_paths = Vec::new();
    let mut errors = Vec::new();
    for candidate in &enumeration.candidates {
        let target = &candidate.target;
        match target.selection_state {
            SelectionState::Invalid => {
                errors.push(ProfileAssignmentError {
                    code: "invalidSelectionState".to_string(),
                    message: target.selection_error.clone().unwrap_or_else(|| {
                        format!(
                            "Replica '{}' has an invalid selection state",
                            target.replica_path
                        )
                    }),
                    session_ids: target.live_session_ids.clone(),
                    replica_paths: vec![target.replica_path.clone()],
                });
            }
            SelectionState::Unlocked => {
                // #1941 - classified from the validated read; the mutation
                // funnel is deliberately NOT called for this target, so no
                // config publish, sidecar or restart can happen for it.
                already_unlocked_paths.push(target.replica_path.clone());
            }
            SelectionState::Locked => {
                let expected = crate::config::coding_agent_profiles::ReplicaSelectionExpectation {
                    identity: target.identity_path.clone(),
                    pair: target_pair(target),
                    locked: true,
                };
                match crate::config::coding_agent_profiles::clear_replica_selection_lock(
                    &settings_snapshot,
                    Path::new(&target.replica_path),
                    &expected,
                ) {
                    Ok(outcome) if outcome.changed => {
                        removed_replica_paths.push(target.replica_path.clone())
                    }
                    Ok(_unchanged) => already_unlocked_paths.push(target.replica_path.clone()),
                    Err(e) => {
                        failed_replica_paths.push(target.replica_path.clone());
                        errors.push(ProfileAssignmentError {
                            code: selection_error_code(&e).to_string(),
                            message: e,
                            session_ids: target.live_session_ids.clone(),
                            replica_paths: vec![target.replica_path.clone()],
                        });
                    }
                }
            }
        }
    }
    let remaining_protected_count = refresh_locked_count(&enumeration);
    let invalid_count = enumeration
        .candidates
        .iter()
        .filter(|candidate| candidate.target.selection_state == SelectionState::Invalid)
        .count();
    let removed_count = removed_replica_paths.len();
    let result = ApplySelectionLockRemovalResult {
        scope: request.scope.clone(),
        target_fingerprint,
        removed_count,
        removed_replica_paths,
        already_unlocked_paths,
        failed_replica_paths,
        remaining_protected_count,
        candidate_count: enumeration.candidates.len(),
        counts_complete: enumeration.counts_complete,
        invalid_count,
        errors,
        warnings: enumeration.warnings.clone(),
    };
    let payload = serde_json::json!({
        "scope": request.scope,
        "operation": "unlock",
        "affectedPaths": &result.removed_replica_paths,
        "removedCount": result.removed_count,
    });
    Ok((result, payload))
}

// ── #1941 Matrix selection default commands ──────────────────────────────

#[tauri::command]
pub async fn get_replica_selection_default(
    settings: State<'_, SettingsState>,
    request: GetReplicaSelectionDefaultRequest,
) -> Result<ReplicaSelectionDefaultResult, String> {
    let settings = settings.inner().clone();
    crate::session::selection::run_owned_selection_operation(move || async move {
        get_replica_selection_default_inner(&settings, request).await
    })
    .await
}

pub(crate) async fn get_replica_selection_default_inner(
    settings: &SettingsState,
    request: GetReplicaSelectionDefaultRequest,
) -> Result<ReplicaSelectionDefaultResult, String> {
    let settings_snapshot = settings.read().await.clone();
    let (replica_dir, matrix_dir) =
        validate_selection_anchor(&settings_snapshot, Path::new(&request.target_replica_path))?;
    let default =
        crate::config::coding_agent_profiles::read_replica_selection_default(&matrix_dir)?;
    let default_wire = default.as_ref().map(selection_default_to_wire);
    let default_fingerprint = selection_default_fingerprint(&matrix_dir, default.as_ref())?;
    Ok(ReplicaSelectionDefaultResult {
        target_replica_path: crate::path_utils::path_to_string_without_windows_verbatim_prefix(
            &replica_dir,
        ),
        matrix_path: crate::path_utils::path_to_string_without_windows_verbatim_prefix(&matrix_dir),
        default: default_wire,
        default_fingerprint,
        warnings: Vec::new(),
    })
}

#[tauri::command]
pub async fn set_replica_selection_default<R: tauri::Runtime>(
    app: AppHandle<R>,
    settings: State<'_, SettingsState>,
    request: SetReplicaSelectionDefaultRequest,
) -> Result<ReplicaSelectionDefaultResult, String> {
    let app_handle = app.clone();
    let settings = settings.inner().clone();
    crate::session::selection::run_owned_selection_operation(move || {
        let app = app_handle.clone();
        async move {
            let (result, payload) = set_replica_selection_default_inner(&settings, request).await?;
            let _ = app.emit("coding_agent_profile_selection_updated", payload);
            Ok(result)
        }
    })
    .await
}

pub(crate) async fn set_replica_selection_default_inner(
    settings: &SettingsState,
    request: SetReplicaSelectionDefaultRequest,
) -> Result<(ReplicaSelectionDefaultResult, serde_json::Value), String> {
    let settings_snapshot = settings.read().await.clone();
    validate_profile_assignment_request(
        &settings_snapshot,
        &request.coding_agent_id,
        &request.requested_profile,
    )?;
    let normalized_profile = normalize_profile_letter_for_assignment(&request.requested_profile)?;
    let (replica_dir, matrix_dir) =
        validate_selection_anchor(&settings_snapshot, Path::new(&request.target_replica_path))?;
    // #1941 - the fingerprint binds the canonical Matrix plus the PRIOR default
    // (including absence), so a stale window cannot silently overwrite a default
    // published by another window; the write repeats the CAS inside its lock.
    let prior = crate::config::coding_agent_profiles::read_replica_selection_default(&matrix_dir)?;
    let prior_fingerprint = selection_default_fingerprint(&matrix_dir, prior.as_ref())?;
    if request.confirmed_default_fingerprint != prior_fingerprint {
        return Err(
            "stalePreview: The Matrix selection default changed. Rerun the read before saving."
                .to_string(),
        );
    }
    let default = crate::config::coding_agent_profiles::ReplicaSelectionDefault {
        coding_agent_id: request.coding_agent_id.clone(),
        requested_profile: normalized_profile,
        selection_locked: request.selection_locked,
    };
    crate::config::coding_agent_profiles::write_replica_selection_default(
        &matrix_dir,
        &default,
        prior.as_ref(),
    )?;
    let default_wire = selection_default_to_wire(&default);
    let default_fingerprint = selection_default_fingerprint(&matrix_dir, Some(&default))?;
    let result = ReplicaSelectionDefaultResult {
        target_replica_path: crate::path_utils::path_to_string_without_windows_verbatim_prefix(
            &replica_dir,
        ),
        matrix_path: crate::path_utils::path_to_string_without_windows_verbatim_prefix(&matrix_dir),
        default: Some(default_wire.clone()),
        default_fingerprint,
        warnings: Vec::new(),
    };
    let payload = serde_json::json!({
        "scope": "default",
        "operation": "default",
        "agentPath": &result.matrix_path,
        "affectedPaths": [],
        "default": default_wire,
    });
    Ok((result, payload))
}

fn selection_default_to_wire(
    default: &crate::config::coding_agent_profiles::ReplicaSelectionDefault,
) -> SelectionDefault {
    SelectionDefault {
        coding_agent_id: default.coding_agent_id.clone(),
        requested_profile: default.requested_profile.clone(),
        selection_locked: default.selection_locked,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum WebServerOwnershipState {
    Starting,
    OwnedRunning,
    Stopping,
    ExternalListening,
    Stopped,
}

/// §1453: motivo del ultimo arranque fallido, expuesto al frontend.
/// `detail` es el texto verbatim del error subyacente; la UI lo demota a
/// linea secundaria y construye el titular llano por su cuenta (D8).
/// CONTRATO (D2): `bind` es SIEMPRE el string de settings crudo, en las dos
/// variantes del error. Quien lo cruce contra la lista de interfaces debe
/// gatear antes con un shape IPv4 (ver 5.7.2).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WebServerBindFailure {
    pub bind: String,
    pub port: u16,
    pub detail: String,
}

/// §1453: una direccion IPv4 ofrecible como bind, con su adaptador.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WebServerInterfaceInfo {
    pub address: String,
    pub interface_name: String,
    pub is_virtual: bool,
}

/// §1453: unica conversion error -> payload. Reemplaza los accesores
/// bind()/port()/detail() de la ronda 1: la conversion vive donde vive el
/// payload. Emite el `bind` CRUDO en ambas variantes (contrato de D2).
impl From<&crate::web::StartServerError> for WebServerBindFailure {
    fn from(err: &crate::web::StartServerError) -> Self {
        let (bind, port, detail) = match err {
            crate::web::StartServerError::InvalidAddr { bind, port, detail } => {
                (bind.clone(), *port, detail.clone())
            }
            crate::web::StartServerError::BindFailed { bind, addr, detail } => {
                (bind.clone(), addr.port(), detail.clone())
            }
        };
        WebServerBindFailure { bind, port, detail }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WebServerOwnedStatus {
    pub listening: bool,
    pub owned: bool,
    pub external_listening: bool,
    pub open_allowed: bool,
    pub bind: String,
    pub port: u16,
    pub state: WebServerOwnershipState,
    pub bind_failure: Option<WebServerBindFailure>,
}

fn ensure_web_remote_open_allowed(
    settings: &AppSettings,
    ws_handle: &WebServerHandle,
) -> Result<WebServerLifecycleSnapshot, String> {
    if !settings.web_server_enabled {
        return Err("Web server is not enabled".into());
    }

    let snapshot = ws_handle.snapshot();
    if snapshot.lifecycle != WebServerLifecycle::Running
        || snapshot.endpoint.as_ref()
            != Some(&(settings.web_server_bind.clone(), settings.web_server_port))
    {
        return Err("Web server is not owned by this app process".into());
    }

    Ok(snapshot)
}

/// §1453 D2: el status solo carga el fallo si describe el target configurado
/// AHORA y si nada esta escuchando. Un fallo grabado para un bind/port
/// anterior (p.ej. el usuario edito el puerto estando parado) no debe pintar
/// la alerta del target nuevo, y un fallo viejo no debe pintar sobre algo que
/// SI esta sirviendo. El slot significa exactamente "esto explica por que
/// nada esta sirviendo".
/// El guard de `listening` es la SEGUNDA guardia, independiente de la del
/// frontend (`showBindAlert`, 5.7.2), que sigue siendo la primaria: ninguna
/// de las dos es load-bearing sola porque los dos early-returns Ok(false) de
/// start_web_server nunca graban fallo.
fn select_current_bind_failure(
    recorded: Option<crate::web::StartServerError>,
    bind: &str,
    port: u16,
    listening: bool,
) -> Option<WebServerBindFailure> {
    if listening {
        return None;
    }
    recorded
        .as_ref()
        .map(WebServerBindFailure::from)
        .filter(|failure| failure.bind == bind && failure.port == port)
}

fn build_web_server_owned_status(
    bind: String,
    port: u16,
    lifecycle: WebServerLifecycle,
    listening: bool,
    bind_failure: Option<WebServerBindFailure>,
) -> WebServerOwnedStatus {
    let (state, owned, external_listening, open_allowed) = match lifecycle {
        WebServerLifecycle::Starting => (WebServerOwnershipState::Starting, true, false, false),
        WebServerLifecycle::Running => (WebServerOwnershipState::OwnedRunning, true, false, true),
        WebServerLifecycle::Stopping => (WebServerOwnershipState::Stopping, true, false, false),
        WebServerLifecycle::Stopped if listening => (
            WebServerOwnershipState::ExternalListening,
            false,
            true,
            false,
        ),
        WebServerLifecycle::Stopped => (WebServerOwnershipState::Stopped, false, false, false),
    };

    WebServerOwnedStatus {
        listening,
        owned,
        external_listening,
        open_allowed,
        bind,
        port,
        state,
        bind_failure,
    }
}

fn web_remote_url(bind: &str, port: u16, token: &str) -> Result<String, String> {
    let destination_ip = match bind
        .parse::<IpAddr>()
        .map_err(|error| format!("Invalid web server bind address '{bind}': {error}"))?
    {
        IpAddr::V4(ip) if ip.is_unspecified() => IpAddr::V4(Ipv4Addr::LOCALHOST),
        IpAddr::V6(ip) if ip.is_unspecified() => IpAddr::V6(Ipv6Addr::LOCALHOST),
        ip => ip,
    };
    let socket_addr = SocketAddr::new(destination_ip, port);

    Ok(format!(
        "http://{socket_addr}/?window=browser&remoteToken={token}"
    ))
}

#[tauri::command]
pub async fn open_web_remote(ws_handle: State<'_, WebServerHandle>) -> Result<(), String> {
    let settings = load_settings();
    let authorized = ensure_web_remote_open_allowed(&settings, &ws_handle)?;

    let token_path = crate::config::config_dir()
        .ok_or("No config dir")?
        .join("web-token.txt");

    let token = std::fs::read_to_string(&token_path)
        .map_err(|e| format!("Cannot read web token: {}", e))?;

    let (bind, port) = authorized
        .endpoint
        .as_ref()
        .ok_or("Web server is not owned by this app process")?;
    let url = web_remote_url(bind, *port, token.trim())?;

    let current_settings = load_settings();
    let revalidated = ensure_web_remote_open_allowed(&current_settings, &ws_handle)?;
    if revalidated != authorized {
        return Err("Web server is not owned by this app process".into());
    }

    open::that(&url).map_err(|e| format!("Failed to open browser: {}", e))?;
    Ok(())
}

#[tauri::command]
pub async fn start_api_server(
    app_handle: tauri::AppHandle,
    api_handle: State<'_, ApiServerHandle>,
    settings: State<'_, SettingsState>,
    session_mgr: State<'_, Arc<tokio::sync::RwLock<SessionManager>>>,
    pty_mgr: State<'_, Arc<std::sync::Mutex<PtyManager>>>,
    shutdown: State<'_, crate::shutdown::ShutdownSignal>,
) -> Result<bool, String> {
    if api_handle.has_running()? {
        return Ok(false);
    }

    let s = settings.read().await;
    let bind = s.api_server_bind.clone();
    let port = s.api_server_port;
    drop(s);

    let probe_addr = api_server_probe_addr(&bind, port)?;
    if is_tcp_socket_listening(probe_addr).await {
        return Ok(false);
    }

    let api_shutdown = shutdown.token().child_token();
    let server_start = crate::api::start_server(
        bind,
        port,
        app_handle,
        Arc::clone(&session_mgr),
        Arc::clone(&pty_mgr),
        api_shutdown.clone(),
    );
    let bound_addr = match crate::api::wait_for_startup_ready(server_start.readiness).await {
        Ok(bound_addr) => bound_addr,
        Err(err) => {
            api_shutdown.cancel();
            return Err(err);
        }
    };

    if !api_handle.store_if_idle(ApiServerTask::new(
        server_start.join_handle,
        api_shutdown,
        bound_addr,
    ))? {
        return Ok(false);
    }

    log::info!("[api-server] Started via command");
    Ok(true)
}

#[tauri::command]
pub async fn stop_api_server(api_handle: State<'_, ApiServerHandle>) -> Result<bool, String> {
    let stopped = api_handle
        .shutdown_running(Duration::from_millis(API_SERVER_STOP_TIMEOUT_MS))
        .await?;
    if stopped {
        log::info!("[api-server] Stopped via command");
    }
    Ok(stopped)
}

#[tauri::command]
pub async fn api_server_status(
    api_handle: State<'_, ApiServerHandle>,
    settings: State<'_, SettingsState>,
) -> Result<bool, String> {
    if api_handle.has_running()? {
        return Ok(true);
    }

    let probe_addr = {
        let s = settings.read().await;
        api_server_probe_addr(&s.api_server_bind, s.api_server_port)?
    };
    Ok(is_tcp_socket_listening(probe_addr).await)
}

#[tauri::command]
pub async fn mint_api_client(
    root: String,
    scopes: Vec<String>,
    label: Option<String>,
    expires: Option<String>,
) -> Result<MintApiClientResponse, String> {
    let settings = load_settings();
    let path = crate::config::config_dir()
        .ok_or_else(|| "Cannot resolve the host config directory".to_string())?
        .join(auth::REGISTRY_FILENAME);
    mint_api_client_with_path(
        &path,
        &settings,
        root,
        scopes,
        label,
        expires,
        Uuid::new_v4().to_string(),
        Uuid::new_v4().to_string(),
        chrono::Utc::now().to_rfc3339(),
    )
}

#[allow(clippy::too_many_arguments)]
fn mint_api_client_with_path(
    path: &Path,
    settings: &AppSettings,
    root: String,
    scopes: Vec<String>,
    label: Option<String>,
    expires: Option<String>,
    client_id: String,
    secret: String,
    issued_at: String,
) -> Result<MintApiClientResponse, String> {
    if crate::config::root_agent::is_root_agent_path(&root) {
        return Err(
            "Cannot mint an API client bound to the Root Agent; root-agent is API-excluded"
                .to_string(),
        );
    }

    let (bound_root, bound_fqn) = validate_mint_api_client_root(&root, settings)?;
    if crate::config::root_agent::is_root_agent_target(&bound_fqn)
        || bound_fqn == crate::config::root_agent::ROOT_AGENT_SENDER
    {
        return Err(
            "Cannot mint an API client bound to the Root Agent; root-agent is API-excluded"
                .to_string(),
        );
    }

    let scopes: Vec<String> = scopes
        .into_iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    auth::validate_scopes(&scopes)?;
    let expires_at = bounded_mint_api_client_expiry(expires, &issued_at)?;

    let out = auth::mint(
        path,
        auth::MintRequest {
            client_id,
            secret,
            label: label.unwrap_or_default(),
            bound_root: bound_root.clone(),
            bound_fqn: bound_fqn.clone(),
            scopes: scopes.clone(),
            issued_at,
            expires_at: Some(expires_at.clone()),
            bound_session_id: None,
            credential_generation: None,
        },
    )?;

    crate::api::audit::record(&out.client_id, &bound_fqn, "mint", "ok");
    Ok(MintApiClientResponse {
        client_id: out.client_id,
        token: out.secret,
        bound_fqn,
        bound_root,
        scopes,
        expires_at: Some(expires_at),
        note: MINT_API_CLIENT_NOTE.to_string(),
    })
}

fn validate_mint_api_client_root(
    root: &str,
    settings: &AppSettings,
) -> Result<(String, String), String> {
    let canonical = crate::config::coding_agent_profiles::validate_profile_selection_agent_path(
        settings,
        Path::new(root),
    )
    .map_err(|_| format!("unknown or invalid root: {}", root))?
    .launch_path;

    let is_replica = canonical
        .file_name()
        .and_then(|name| name.to_str())
        .map(|name| name.starts_with("__agent_"))
        .unwrap_or(false);
    if !is_replica {
        return Err(format!("unknown or invalid root: {}", root));
    }

    let bound_root = crate::path_utils::path_to_string_without_windows_verbatim_prefix(&canonical);
    let bound_fqn = crate::config::teams::agent_fqn_from_path(&bound_root);
    Ok((bound_root, bound_fqn))
}

fn bounded_mint_api_client_expiry(
    expires: Option<String>,
    issued_at: &str,
) -> Result<String, String> {
    let issued_at = chrono::DateTime::parse_from_rfc3339(issued_at)
        .map_err(|e| format!("Invalid issuedAt timestamp: {}", e))?
        .with_timezone(&chrono::Utc);
    let max_expires_at = issued_at + chrono::Duration::days(MINT_API_CLIENT_MAX_TTL_DAYS);

    let expires_at = match expires {
        Some(raw) => {
            let trimmed = raw.trim();
            let parsed = chrono::DateTime::parse_from_rfc3339(trimmed)
                .map_err(|e| format!("Invalid expires value; expected RFC3339: {}", e))?
                .with_timezone(&chrono::Utc);
            if parsed <= issued_at {
                return Err("expires must be after the issuedAt timestamp".to_string());
            }
            if parsed > max_expires_at {
                return Err(format!(
                    "expires exceeds maximum API client lifetime of {} days",
                    MINT_API_CLIENT_MAX_TTL_DAYS
                ));
            }
            parsed
        }
        None => issued_at + chrono::Duration::hours(MINT_API_CLIENT_DEFAULT_TTL_HOURS),
    };

    Ok(expires_at.to_rfc3339())
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn begin_web_server_start(
    ws_handle: WebServerHandle,
    settings: SettingsState,
    web_token: Arc<WebAccessToken>,
    session_mgr: Arc<tokio::sync::RwLock<SessionManager>>,
    pty_mgr: Arc<std::sync::Mutex<PtyManager>>,
    broadcaster: WsBroadcaster,
    app_handle: tauri::AppHandle,
    shutdown: crate::shutdown::ShutdownSignal,
) -> WebServerStartWaiter {
    let lifecycle = ws_handle.clone();
    let factory_shutdown = shutdown.clone();
    ws_handle.begin_start(
        shutdown,
        move |generation, admission, generation_token| async move {
            let s = settings.read().await;
            let bind = s.web_server_bind.clone();
            let port = s.web_server_port;
            drop(s);

            if !lifecycle.publish_effective_endpoint(generation, bind.clone(), port) {
                return Err(WEB_SERVER_START_CANCELLED.to_string());
            }

            let probe_addr = match web_server_probe_addr(&bind, port) {
                Ok(addr) => addr,
                Err(detail) => {
                    let error = crate::web::StartServerError::InvalidAddr { bind, port, detail };
                    log::warn!("[web-server] start failed: {}", error);
                    lifecycle.record_bind_failure(error);
                    return Ok(None);
                }
            };

            if is_tcp_listening(probe_addr).await {
                return Ok(None);
            }
            if generation_token.is_cancelled() || factory_shutdown.is_cancelled() {
                return Err(WEB_SERVER_START_CANCELLED.to_string());
            }

            match crate::web::start_server(
                bind,
                port,
                web_token,
                session_mgr,
                pty_mgr,
                settings,
                broadcaster,
                app_handle,
                admission,
                generation_token,
                factory_shutdown,
            )
            .await
            {
                Ok(join_handle) => {
                    lifecycle.clear_bind_failure();
                    Ok(Some(join_handle))
                }
                Err(error) => {
                    log::warn!("[web-server] start failed: {}", error);
                    lifecycle.record_bind_failure(error);
                    Ok(None)
                }
            }
        },
    )
}

// Tauri command: State<> injections push us over clippy's 7-arg threshold.
#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub async fn start_web_server(
    app_handle: tauri::AppHandle,
    ws_handle: State<'_, WebServerHandle>,
    settings: State<'_, SettingsState>,
    web_token: State<'_, Arc<WebAccessToken>>,
    session_mgr: State<'_, Arc<tokio::sync::RwLock<SessionManager>>>,
    pty_mgr: State<'_, Arc<std::sync::Mutex<PtyManager>>>,
    broadcaster: State<'_, WsBroadcaster>,
    shutdown: State<'_, crate::shutdown::ShutdownSignal>,
) -> Result<bool, String> {
    let waiter = begin_web_server_start(
        ws_handle.inner().clone(),
        Arc::clone(&settings),
        Arc::clone(&web_token),
        Arc::clone(&session_mgr),
        Arc::clone(&pty_mgr),
        (*broadcaster).clone(),
        app_handle,
        shutdown.inner().clone(),
    );
    let started = waiter.wait().await?;
    if started {
        log::info!("[web-server] Started via command");
    }
    Ok(started)
}

#[tauri::command]
pub async fn stop_web_server(ws_handle: State<'_, WebServerHandle>) -> Result<bool, String> {
    stop_web_server_handle(&ws_handle).await
}

async fn stop_web_server_handle(ws_handle: &WebServerHandle) -> Result<bool, String> {
    let Some(waiter) = ws_handle.begin_stop() else {
        return Ok(false);
    };
    waiter.wait().await?;
    log::info!("[web-server] Stopped via command");
    Ok(true)
}

#[tauri::command]
pub async fn get_web_server_status(settings: State<'_, SettingsState>) -> Result<bool, String> {
    let s = settings.read().await;
    let addr = web_server_probe_addr(&s.web_server_bind, s.web_server_port).ok();
    drop(s);
    Ok(match addr {
        Some(addr) => is_tcp_listening(addr).await,
        None => false,
    })
}

#[tauri::command]
pub async fn get_web_server_owned_status(
    ws_handle: State<'_, WebServerHandle>,
    settings: State<'_, SettingsState>,
) -> Result<WebServerOwnedStatus, String> {
    let s = settings.read().await;
    let bind = s.web_server_bind.clone();
    let port = s.web_server_port;
    drop(s);

    Ok(resolve_web_server_owned_status(&ws_handle, (bind, port), is_tcp_listening).await)
}

/// §1453 D3: heuristica por nombre para separar adaptadores virtuales/tunel
/// de los fisicos. Cubre los friendly names de Windows y los prefijos unix.
/// Un falso positivo/negativo solo mueve la fila de grupo en el popover; la
/// fila sigue siendo seleccionable, asi que el costo de un miss es cosmetico.
fn classify_virtual_interface(name: &str) -> bool {
    let lowered = name.to_ascii_lowercase();
    // "loopback" NO va aca: la clasificacion corre en el .map() POSTERIOR al
    // filtro de loopback, asi que seria una rama inalcanzable (dev-rust).
    const CONTAINS: [&str; 10] = [
        "vethernet",
        "wsl",
        "tailscale",
        "virtualbox",
        "vmware",
        "hyper-v",
        "docker",
        "zerotier",
        "hamachi",
        "wireguard",
    ];
    const PREFIXES: [&str; 9] = [
        "veth", "virbr", "vmnet", "tun", "tap", "utun", "wg", "br-", "zt",
    ];
    CONTAINS.iter().any(|needle| lowered.contains(needle))
        || PREFIXES.iter().any(|prefix| lowered.starts_with(prefix))
}

/// §1453 D1/D3: transforma pares (nombre, IPv4) crudos en la lista ofrecida.
/// Filtra loopback (la representa el preset Localhost), link-local 169.254/16
/// (APIPA: exactamente la clase de bind fragil que este issue elimina) y
/// unspecified; ordena fisicas primero y dedup exacto.
///
/// NO BORRAR el filtro is_link_local() "por muerto": en Windows es un no-op
/// porque el crate ya descarta 169.254.* antes de entregarlas, pero en
/// Linux/macOS SI llegan y el filtro es el unico que las saca (D1).
fn map_web_server_interfaces(
    raw: Vec<(String, std::net::Ipv4Addr)>,
) -> Vec<WebServerInterfaceInfo> {
    let mut out: Vec<WebServerInterfaceInfo> = raw
        .into_iter()
        .filter(|(_, ip)| !ip.is_loopback() && !ip.is_link_local() && !ip.is_unspecified())
        .map(|(name, ip)| WebServerInterfaceInfo {
            address: ip.to_string(),
            is_virtual: classify_virtual_interface(&name),
            interface_name: name,
        })
        .collect();
    // INVARIANTE: la clave de sort debe cubrir TODOS los campos del struct,
    // porque Vec::dedup solo elimina duplicados CONSECUTIVOS. Si se agrega un
    // campo a WebServerInterfaceInfo sin agregarlo aca, el dedup deja de
    // funcionar en silencio (dev-rust).
    out.sort_by(|a, b| {
        (a.is_virtual, &a.interface_name, &a.address).cmp(&(
            b.is_virtual,
            &b.interface_name,
            &b.address,
        ))
    });
    out.dedup();
    out
}

/// §1453: IPv4 actuales de la maquina con su adaptador, para el chooser de
/// bind del popover. Solo registrado en el invoke_handler de escritorio; el
/// bridge WS del browser no rutea ningun comando de web server (D3).
/// async no por consistencia (este archivo tiene comandos sync), sino para no
/// ocupar el main thread: Tauri v2 corre los comandos sync ahi.
/// Lista adaptadores caidos con IPv4 estatica: deliberado (D3/D6).
/// get_if_addrs() puede entrar en panico si falla HeapAlloc (camino de OOM).
#[tauri::command]
pub async fn list_web_server_interfaces() -> Result<Vec<WebServerInterfaceInfo>, String> {
    let interfaces = if_addrs::get_if_addrs().map_err(|e| {
        log::warn!("[web-server] interface enumeration failed: {}", e);
        format!("Failed to enumerate network interfaces: {}", e)
    })?;
    Ok(map_web_server_interfaces(
        interfaces
            .into_iter()
            .filter_map(|iface| match iface.addr {
                if_addrs::IfAddr::V4(v4) => Some((iface.name, v4.ip)),
                _ => None,
            })
            .collect(),
    ))
}

fn api_server_probe_addr(bind: &str, port: u16) -> Result<SocketAddr, String> {
    let configured = parse_api_server_socket_addr(bind, port)?;
    let probe_ip = match configured.ip() {
        IpAddr::V4(ip) if ip.is_unspecified() => IpAddr::V4(Ipv4Addr::LOCALHOST),
        IpAddr::V6(ip) if ip.is_unspecified() => IpAddr::V6(Ipv6Addr::LOCALHOST),
        ip => ip,
    };
    Ok(SocketAddr::new(probe_ip, configured.port()))
}

fn web_server_probe_addr(bind: &str, port: u16) -> Result<SocketAddr, String> {
    let configured = format!("{}:{}", bind, port)
        .parse::<SocketAddr>()
        .map_err(|error| error.to_string())?;
    let probe_ip = match configured.ip() {
        IpAddr::V4(ip) if ip.is_unspecified() => IpAddr::V4(Ipv4Addr::LOCALHOST),
        IpAddr::V6(ip) if ip.is_unspecified() => IpAddr::V6(Ipv6Addr::LOCALHOST),
        ip => ip,
    };
    Ok(SocketAddr::new(probe_ip, configured.port()))
}

async fn resolve_web_server_owned_status<F, Fut>(
    ws_handle: &WebServerHandle,
    configured_fallback: (String, u16),
    mut probe: F,
) -> WebServerOwnedStatus
where
    F: FnMut(SocketAddr) -> Fut,
    Fut: Future<Output = bool>,
{
    loop {
        let snapshot = ws_handle.snapshot();
        let (bind, port) = match (&snapshot.lifecycle, snapshot.endpoint.as_ref()) {
            (WebServerLifecycle::Stopped, _) => configured_fallback.clone(),
            (_, Some(endpoint)) => endpoint.clone(),
            (WebServerLifecycle::Starting | WebServerLifecycle::Stopping, None) => {
                configured_fallback.clone()
            }
            (WebServerLifecycle::Running, None) => configured_fallback.clone(),
        };
        let listening = match web_server_probe_addr(&bind, port) {
            Ok(addr) => probe(addr).await,
            Err(_) => false,
        };
        if !ws_handle.snapshot_is_current(&snapshot) {
            continue;
        }

        let bind_failure =
            select_current_bind_failure(ws_handle.last_bind_failure(), &bind, port, listening);
        return build_web_server_owned_status(
            bind,
            port,
            snapshot.lifecycle,
            listening,
            bind_failure,
        );
    }
}

async fn is_tcp_socket_listening(addr: SocketAddr) -> bool {
    matches!(
        tokio::time::timeout(
            Duration::from_millis(WEB_STATUS_CONNECT_TIMEOUT_MS),
            tokio::net::TcpStream::connect(addr),
        )
        .await,
        Ok(Ok(_))
    )
}

async fn is_tcp_listening(addr: SocketAddr) -> bool {
    is_tcp_socket_listening(addr).await
}

/// Returns the runtime instance label for the titlebar badge.
/// E.g. "STAGE", "STANDALONE", or "" for prod (no badge).
#[tauri::command]
pub fn get_instance_label() -> String {
    crate::config::profile::instance_label().to_string()
}

/// `pub(crate)` since #1171: `set_watchers_geometry` lives in `commands/window.rs` beside the
/// window it belongs to, and must write its one field through this same candidate-save-publish
/// path rather than a second one of its own.
pub(crate) async fn persist_narrow_settings_update(
    settings: &SettingsState,
    mutate_candidate: impl FnOnce(&mut AppSettings),
) -> Result<(), String> {
    persist_narrow_settings_update_with_saver(settings, mutate_candidate, save_settings).await
}

async fn persist_narrow_settings_update_with_saver(
    settings: &SettingsState,
    mutate_candidate: impl FnOnce(&mut AppSettings),
    save: impl FnOnce(&AppSettings) -> Result<AppSettings, String>,
) -> Result<(), String> {
    let mut s = settings.write().await;
    let mut candidate = s.clone();
    mutate_candidate(&mut candidate);
    let written = save(&candidate)?;
    *s = written;
    Ok(())
}

/// #2306 P2 - the ordered-move rejection while the local overlay owns the whole
/// top-level `agents` key. Stable token shared by the Tauri command and the
/// browser route (transport parity).
pub(crate) const AGENT_ORDER_OVERLAY_PINNED: &str =
    "agent order is controlled by the local settings overlay (settings.local.json)";

/// #2306 P2 - fallible sibling of `persist_narrow_settings_update_with_saver`,
/// used by the ordered move.
///
/// Same candidate-save-publish pattern with two differences:
///
///   * the mutation is fallible, so a rejected request (boundary move, unknown
///     or non-adjacent neighbor) returns before the saver runs and leaves live
///     settings untouched;
///   * while the local overlay owns the top-level `agents` key the request is
///     rejected before the clone. `save_settings_value_locked` restores the
///     overlay's base array to `settings.json` and reapplies the overlay-
///     effective array only to memory, so no move could survive a restart and a
///     "successful" move would publish a transient order. The check runs under
///     the write guard, so an overlay appearing concurrently cannot slip
///     between the check and the save.
///
/// On success it installs the written snapshot and returns it, so the caller
/// derives the authoritative ids (and ordinals) from what was persisted.
async fn persist_narrow_settings_update_fallible_with_saver(
    settings: &SettingsState,
    mutate_candidate: impl FnOnce(&mut AppSettings) -> Result<(), String>,
    save: impl FnOnce(&AppSettings) -> Result<AppSettings, String>,
) -> Result<AppSettings, String> {
    let mut s = settings.write().await;
    if s.local_overlay_state.owns_top_level(OVERLAY_KEY_AGENTS) {
        return Err(AGENT_ORDER_OVERLAY_PINNED.to_string());
    }
    let mut candidate = s.clone();
    mutate_candidate(&mut candidate)?;
    let written = save(&candidate)?;
    *s = written.clone();
    Ok(written)
}

/// Narrow setter for `sounds_enabled`. Same candidate-save-publish
/// pattern as the other narrow setters (issue #158). Replaces the toolbar's
/// previous full-object `update_settings(next)` call, which could clobber
/// unrelated fields from a stale `settingsStore.current` snapshot.
#[tauri::command]
pub async fn set_sounds_enabled(
    settings: State<'_, SettingsState>,
    value: bool,
) -> Result<(), String> {
    persist_narrow_settings_update(settings.inner(), |candidate| {
        candidate.sounds_enabled = value;
    })
    .await
}

/// Narrow setter for `theme_light`. Same candidate-save-publish
/// pattern as `set_sounds_enabled` (issue #289). Lets the UI persist the
/// user's light/dark mode choice without going through `update_settings`,
/// which could clobber unrelated fields from a stale snapshot.
#[tauri::command]
pub async fn set_theme_light(
    settings: State<'_, SettingsState>,
    value: bool,
) -> Result<(), String> {
    persist_narrow_settings_update(settings.inner(), |candidate| {
        candidate.theme_light = value;
    })
    .await
}

/// Narrow setter for `main_resource_monitor_attached` (#587). Same
/// candidate-save-publish pattern as `set_theme_light`; lets the central-view
/// toggle persist without a get+update round-trip racing SettingsModal.
#[tauri::command]
pub async fn set_main_resource_monitor_attached(
    settings: State<'_, SettingsState>,
    value: bool,
) -> Result<(), String> {
    persist_narrow_settings_update(settings.inner(), |candidate| {
        candidate.main_resource_monitor_attached = value;
    })
    .await
}

/// #965 narrow setter for the rail collapse snapshot. Same candidate-save-publish
/// pattern as `set_theme_light`: holds the `SettingsState` write lock across
/// `save_settings`, so it cannot lose an interleaved `update_settings`.
/// ONE command for both fields: the rail always persists its whole snapshot, so a
/// per-field setter would just double the writes. Called once per explicit header
/// click (no auto-focus path writes here), so it is cold by construction.
///
/// The `_inner` split exists so the web/WS route (`web/commands.rs`) reaches the
/// same read-modify-save instead of duplicating it; the browser client mounts the
/// rail too. Precedent: `set_agent_default_profile_inner`.
pub(crate) async fn set_rail_collapse_inner(
    settings: &SettingsState,
    collapsed_projects: Vec<String>,
    favorites_collapsed: bool,
) -> Result<(), String> {
    set_rail_collapse_inner_with_saver(
        settings,
        collapsed_projects,
        favorites_collapsed,
        save_settings,
    )
    .await
}

/// Saver seam for `set_rail_collapse_inner`, mirroring the three existing
/// `*_with_saver` pairs in this module. It exists so a unit test can drive the REAL
/// mutation without reaching `save_settings` -> `config::config_dir()`, a `OnceLock`
/// that no test redirects and that would overwrite the developer's own settings.json.
async fn set_rail_collapse_inner_with_saver(
    settings: &SettingsState,
    collapsed_projects: Vec<String>,
    favorites_collapsed: bool,
    save: impl FnOnce(&AppSettings) -> Result<AppSettings, String>,
) -> Result<(), String> {
    persist_narrow_settings_update_with_saver(
        settings,
        |candidate| {
            candidate.rail_collapsed_projects = collapsed_projects;
            candidate.rail_favorites_collapsed = favorites_collapsed;
        },
        save,
    )
    .await
}

#[tauri::command]
pub async fn set_rail_collapse(
    settings: State<'_, SettingsState>,
    collapsed_projects: Vec<String>,
    favorites_collapsed: bool,
) -> Result<(), String> {
    set_rail_collapse_inner(settings.inner(), collapsed_projects, favorites_collapsed).await
}

/// #2306 P2 - narrow owner of the registered-agent move. Mutates only the
/// `agents` vector under the settings write guard, persists through the
/// preserving `save_settings`, and returns the authoritative ordered ids.
#[tauri::command]
pub async fn move_coding_agent<R: tauri::Runtime>(
    app: AppHandle<R>,
    settings: State<'_, SettingsState>,
    id: String,
    neighbor_id: String,
    direction: AgentMoveDirection,
) -> Result<Vec<String>, String> {
    move_coding_agent_command_with_saver(
        &app,
        settings.inner(),
        id,
        neighbor_id,
        direction,
        save_settings,
    )
    .await
}

/// Shared Tauri-command body. The settings write guard is released when the
/// inner returns, so `coding_agent_settings_updated` is emitted after unlock.
/// Split so a test can drive the real emit with a temp-path saver.
pub(crate) async fn move_coding_agent_command_with_saver<R: tauri::Runtime>(
    app: &AppHandle<R>,
    settings: &SettingsState,
    id: String,
    neighbor_id: String,
    direction: AgentMoveDirection,
    save: impl FnOnce(&AppSettings) -> Result<AppSettings, String>,
) -> Result<Vec<String>, String> {
    let ids =
        move_coding_agent_inner_with_saver(settings, &id, &neighbor_id, direction, save).await?;
    let _ = app.emit(
        "coding_agent_settings_updated",
        move_coding_agent_event_payload(&id),
    );
    Ok(ids)
}

/// The shared inner owner of the move, used by the Tauri command and the
/// browser WebSocket route.
pub(crate) async fn move_coding_agent_inner_with_saver(
    settings: &SettingsState,
    id: &str,
    neighbor_id: &str,
    direction: AgentMoveDirection,
    save: impl FnOnce(&AppSettings) -> Result<AppSettings, String>,
) -> Result<Vec<String>, String> {
    let written = persist_narrow_settings_update_fallible_with_saver(
        settings,
        |candidate| move_registered_agent(candidate, id, neighbor_id, direction).map(|_| ()),
        save,
    )
    .await?;
    Ok(written
        .agents
        .iter()
        .map(|agent| agent.id.clone())
        .collect())
}

/// One payload shape for the move's `coding_agent_settings_updated` event on
/// both transports (native `app.emit` and browser `broadcast_all`).
pub(crate) fn move_coding_agent_event_payload(id: &str) -> serde_json::Value {
    serde_json::json!({ "op": "move", "agentId": id })
}

/// #612 apply the runtime log level (no-op under RUST_LOG) and broadcast so
/// every webview re-applies its own console gate. Single point that both the
/// dedicated `set_log_level` command and the SettingsModal Save path reuse.
fn apply_and_broadcast_log_level(app: &AppHandle, level: &str) {
    // set_runtime_log_level returns false when the gate is not installed
    // (RUST_LOG dev-override active): the backend stays frozen and only the
    // frontend console gate moves. Emit one debug line so an operator reading
    // app.log isn't confused why the backend verbosity didn't change.
    if !crate::logging::set_runtime_log_level(level) {
        log::debug!(
            "[log-level] RUST_LOG override active; backend frozen, applying '{level}' to frontend only"
        );
    }
    let _ = app.emit("log_level_changed", serde_json::json!({ "level": level }));
}

/// #612 canonical command to change ONLY the log level: validates, persists
/// `logLevel`, applies live, broadcasts. Used by programmatic callers / future
/// quick-switch; the SettingsModal uses the form Save path (`save_settings_draft`).
#[tauri::command]
pub async fn set_log_level(
    app: AppHandle,
    settings: State<'_, SettingsState>,
    level: String,
) -> Result<(), String> {
    let normalized = crate::logging::normalize_log_level(&level)
        .ok_or_else(|| format!("Invalid log level: {level}"))?;
    persist_narrow_settings_update(settings.inner(), |candidate| {
        candidate.log_level = Some(normalized.to_string());
    })
    .await?;
    apply_and_broadcast_log_level(&app, normalized);
    Ok(())
}

/// Issue #609 - return the pending "npm update available" info, or null if the
/// running build is current / the check has not finished / it is disabled.
/// Pure read of the cached `UpdateCheckState` (set by the startup task in
/// `update_check::run_startup_check`). The sidebar queries this on mount to
/// cover the case where the startup emit fired before its listener registered.
#[tauri::command]
pub async fn get_update_status(
    cache: State<'_, crate::UpdateCheckState>,
) -> Result<Option<crate::update_check::UpdateInfo>, String> {
    Ok(cache.get().cloned())
}

/// #1327 - snapshot of the startup coding-agent update run (in progress flag +
/// the currently registered-but-unanswered prompt + per-command results). The
/// sidebar queries it on mount to cover events that fired before its listeners
/// registered (mirrors `get_update_status`).
#[tauri::command]
pub async fn get_agent_update_status(
    gate: State<'_, Arc<crate::agent_update::AgentUpdateGate>>,
) -> Result<crate::agent_update::AgentUpdateStatus, String> {
    Ok(gate.snapshot())
}

/// #1327/#1551 - the user answered the startup prompt for one command, from any
/// surface. The gate's serial section classifies, persists, claims the pending
/// prompt, announces `agent_update_prompt_closed` to every surface and only then
/// releases the prompt loop. Returns Ok(true) ONLY when THIS call resolved the
/// pending prompt (its choice was persisted and this boot acts on it); Ok(false)
/// when the prompt had already expired (persisted for future boots, this boot
/// unaffected) or an answer was already recorded this boot (nothing persisted,
/// the earlier answer stands). Unknown commands are rejected.
#[tauri::command]
pub async fn agent_update_answer(
    app: AppHandle,
    settings: State<'_, SettingsState>,
    command: String,
    enabled: bool,
) -> Result<bool, String> {
    agent_update_answer_inner(&app, settings.inner(), command, enabled).await
}

#[tauri::command]
pub async fn agent_update_cancel(
    app: AppHandle,
    command: String,
) -> Result<crate::agent_update::AgentUpdateCancelResponse, String> {
    agent_update_cancel_inner(&app, command).await
}

#[tauri::command]
pub async fn agent_updates_cancel_all(
    app: AppHandle,
) -> Result<crate::agent_update::AgentUpdateCancelAllResponse, String> {
    agent_updates_cancel_all_inner(&app).await
}

/// #1551 - the managed startup gate, or a plain error string when an unmanaged
/// (test) app asks for it, so a missing state never panics a command.
fn managed_agent_update_gate(
    app: &AppHandle,
) -> Result<State<'_, Arc<crate::agent_update::AgentUpdateGate>>, String> {
    tauri::Manager::try_state::<Arc<crate::agent_update::AgentUpdateGate>>(app)
        .ok_or_else(|| "agent update gate is not managed".to_string())
}

/// #1551 - shared by the Tauri command and the WebSocket router. `try_state` so an
/// unmanaged test app errors instead of panicking.
pub fn agent_update_status_inner(
    app: &AppHandle,
) -> Result<crate::agent_update::AgentUpdateStatus, String> {
    Ok(managed_agent_update_gate(app)?.snapshot())
}

/// #1551 - the only answer path: `agent_update::answer_prompt` with the existing
/// narrow persist injected. Classification, persist, and settlement all happen
/// inside the gate's serial section.
pub async fn agent_update_answer_inner(
    app: &AppHandle,
    settings: &SettingsState,
    command: String,
    enabled: bool,
) -> Result<bool, String> {
    let gate = managed_agent_update_gate(app)?;
    crate::agent_update::answer_prompt(app, &gate, &command, enabled, || {
        persist_narrow_settings_update(settings, |candidate| {
            candidate
                .agent_auto_update_by_command
                .insert(command.clone(), enabled);
        })
    })
    .await
}

pub async fn agent_update_cancel_inner(
    app: &AppHandle,
    command: String,
) -> Result<crate::agent_update::AgentUpdateCancelResponse, String> {
    let gate = managed_agent_update_gate(app)?;
    Ok(crate::agent_update::cancel_update(app, &gate, command).await)
}

pub async fn agent_updates_cancel_all_inner(
    app: &AppHandle,
) -> Result<crate::agent_update::AgentUpdateCancelAllResponse, String> {
    let gate = managed_agent_update_gate(app)?;
    Ok(crate::agent_update::cancel_all_updates(app, &gate).await)
}

/// #1551 - instant read of the Settings "Auto-update" table. Never awaits a probe;
/// probes are scheduled in the background only once the startup pass is finished.
/// #1967 P4 - `Err` when the persisted catalog is unavailable (nothing is probed).
pub async fn agent_update_overview_inner(
    app: &AppHandle,
    settings: &SettingsState,
) -> Result<Vec<crate::agent_update::AgentUpdateOverviewRow>, String> {
    let gate = managed_agent_update_gate(app)?;
    let cache = tauri::Manager::try_state::<Arc<crate::agent_version::AgentInstallCache>>(app)
        .ok_or_else(|| "agent install cache is not managed".to_string())?;
    crate::agent_update::update_overview(app, settings, &gate, &cache)
        .await
        .map_err(|unavailable| unavailable.to_string())
}

/// #1551 - one row per update-capable catalog entry with its configured policy,
/// installed version, and this boot's live state.
#[tauri::command]
pub async fn get_agent_update_overview(
    app: AppHandle,
    settings: State<'_, SettingsState>,
) -> Result<Vec<crate::agent_update::AgentUpdateOverviewRow>, String> {
    agent_update_overview_inner(&app, settings.inner()).await
}

/// Fetch the Home screen Markdown source from the public docs URL.
/// Returns the raw Markdown body as a String.
/// Errors are returned as user-facing strings; the frontend renders them in
/// the Home view's error state.
#[tauri::command]
pub async fn fetch_home_markdown(network: State<'_, OutboundNetwork>) -> Result<String, String> {
    let _permit = network.acquire("docs.fetch_home_markdown").await?;
    let resp = tokio::time::timeout(
        Duration::from_secs(HOME_MARKDOWN_TIMEOUT_SECS),
        network
            .general()
            .get(HOME_MARKDOWN_URL)
            .header(
                reqwest::header::USER_AGENT,
                concat!("agentscommander/", env!("CARGO_PKG_VERSION")),
            )
            .send(),
    )
    .await
    .map_err(|_| "Network error: request timed out".to_string())?
    .map_err(|e| format!("Network error: {}", e))?;

    if !resp.status().is_success() {
        return Err(format!("Server returned status {}", resp.status().as_u16()));
    }

    // Use bytes() so we can length-check before allocating a String.
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| format!("Failed to read response: {}", e))?;

    if bytes.is_empty() {
        return Err("Server returned empty response".to_string());
    }
    if bytes.len() > HOME_MARKDOWN_MAX_BYTES {
        return Err("Response too large".to_string());
    }

    // Strip a leading UTF-8 BOM if present so it does not render as an
    // invisible character at the top of the document (grinch §M, optional).
    let trimmed: &[u8] = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(&bytes);

    String::from_utf8(trimmed.to_vec()).map_err(|_| "Response is not valid UTF-8".to_string())
}

#[cfg(test)]
mod tests {
    use super::{
        api_server_probe_addr, api_server_status, build_web_server_owned_status,
        classify_virtual_interface, ensure_web_remote_open_allowed, is_tcp_socket_listening,
        map_web_server_interfaces, mint_api_client_with_path, move_coding_agent_command_with_saver,
        move_coding_agent_inner_with_saver, persist_coding_agent_env_settings_update,
        persist_coding_agent_profiles_update, persist_narrow_settings_update_with_saver,
        persist_protected_settings_update_with_saver, persist_settings_draft_update_with_saver,
        purge_sessions_after_settings_update_in_dir, resolve_web_server_owned_status,
        select_current_bind_failure, set_rail_collapse_inner_with_saver, settings_snapshot_from,
        start_api_server, stop_web_server_handle, web_remote_url, web_server_probe_addr,
        WebServerOwnershipState, AGENT_ORDER_OVERLAY_PINNED, MINT_API_CLIENT_DEFAULT_TTL_HOURS,
        MINT_API_CLIENT_MAX_TTL_DAYS, MINT_API_CLIENT_NOTE,
    };
    #[cfg(windows)]
    use super::{build_profile_assignment_target, canonical_compare_key};
    use crate::api::auth;
    use crate::config::coding_agent_mutations::AgentMoveDirection;
    use crate::config::sessions_persistence::{session_retention_project_paths, PersistedSession};
    use crate::config::settings::{
        AgentConfig, AppSettings, CodingAgentEnv, CodingAgentEnvSource, MainWindowDisplayState,
        ProfileCellConfig, SettingsState, WindowGeometry,
    };
    use crate::session::manager::SessionManager;
    use crate::{
        ApiServerHandle, ApiServerTask, WebServerHandle, WebServerLifecycle,
        WEB_SERVER_START_CANCELLED,
    };
    use serde_json::{json, Value};
    use std::collections::{BTreeMap, HashMap, VecDeque};
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;
    use tauri::Manager;
    use tokio::sync::{mpsc, oneshot, RwLock};
    use tokio_util::sync::CancellationToken;

    // ── #1077 SettingsSnapshot / resolution report ───────────────────────
    mod snapshot {
        use super::super::{settings_snapshot_from, ProjectPathIssue};
        use crate::config::projects::{
            DirectoryIdentity, IssueKind, ProjectPathPersistenceState, ProjectSource, RawJsonField,
            RawStringField, RepairKind, ResolvedPair, SideOutcome, SideStatus, StructuralIssue,
        };
        use crate::config::settings::AppSettings;

        fn side(status: SideStatus, canonical: Option<&str>) -> SideOutcome {
            SideOutcome {
                status,
                syntactic_path: canonical.map(str::to_string),
                canonical_path: canonical.map(str::to_string),
                identity: canonical.map(|_| DirectoryIdentity { volume: 1, file: 1 }),
            }
        }

        fn conflict_pair() -> ResolvedPair {
            ResolvedPair {
                source: ProjectSource::ProjectPaths,
                index: Some(0),
                raw_absolute: RawStringField::string("/abs/alpha"),
                raw_relative: RawStringField::string("../rel/beta"),
                absolute_side: side(SideStatus::ValidDirectProject, Some("/abs/alpha")),
                relative_side: side(SideStatus::ValidDirectProject, Some("/rel/beta")),
                selected: None,
                selected_canonical_raw: None,
                selected_identity: None,
                issue: Some(IssueKind::Conflict),
                repair: RepairKind::None,
            }
        }

        fn missing_pair() -> ResolvedPair {
            ResolvedPair {
                source: ProjectSource::ProjectPaths,
                index: Some(1),
                raw_absolute: RawStringField::string("/gone/x"),
                raw_relative: RawStringField::absent(),
                absolute_side: SideOutcome {
                    status: SideStatus::Missing,
                    syntactic_path: Some("/gone/x".to_string()),
                    canonical_path: None,
                    identity: None,
                },
                relative_side: SideOutcome {
                    status: SideStatus::Absent,
                    syntactic_path: None,
                    canonical_path: None,
                    identity: None,
                },
                selected: None,
                selected_canonical_raw: None,
                selected_identity: None,
                issue: Some(IssueKind::Missing),
                repair: RepairKind::None,
            }
        }

        fn state_with(
            pairs: Vec<ResolvedPair>,
            structural: Vec<StructuralIssue>,
        ) -> ProjectPathPersistenceState {
            let active_registration_count = pairs.len();
            ProjectPathPersistenceState {
                pairs,
                selected_head: None,
                active_registration_count,
                archived_registration_count: 0,
                active_companion_present: true,
                archived_companion_present: false,
                has_genuine_singular: false,
                active_reconcile_eligible: false,
                archived_reconcile_eligible: false,
                structural_issues: structural,
                runtime_authoritative: false,
            }
        }

        fn settings_with_state(state: ProjectPathPersistenceState) -> AppSettings {
            AppSettings {
                project_path_state: std::sync::Arc::new(state),
                ..AppSettings::default()
            }
        }

        #[test]
        fn snapshot_serializes_camelcase_variants_and_omits_root_token() {
            let settings = AppSettings {
                root_token: Some("SUPER-SECRET-TOKEN-VALUE".to_string()),
                project_path_state: std::sync::Arc::new(state_with(
                    vec![conflict_pair(), missing_pair()],
                    vec![],
                )),
                ..AppSettings::default()
            };

            let snap = settings_snapshot_from(&settings, None);
            let json = serde_json::to_value(&snap).unwrap();

            // rootToken absent from both the flattened settings and anywhere else.
            let text = serde_json::to_string(&json).unwrap();
            assert!(!text.contains("rootToken"), "rootToken leaked: {text}");
            assert!(
                !text.contains("SUPER-SECRET-TOKEN-VALUE"),
                "token value leaked"
            );

            let resolution = &json["projectPathResolution"];
            assert_eq!(resolution["activeRegistrationCount"], 2);
            assert_eq!(resolution["archivedRegistrationCount"], 0);
            let issues = resolution["issues"].as_array().unwrap();
            assert_eq!(issues.len(), 2);

            let conflict = &issues[0];
            assert_eq!(conflict["kind"], "conflict");
            assert_eq!(conflict["source"], "projectPaths");
            assert_eq!(conflict["index"], 0);
            assert!(conflict["absoluteResolvedPath"].is_string());
            assert!(conflict["instanceRelativeResolvedPath"].is_string());
            // id is a full lowercase 64-char hex.
            let id = conflict["id"].as_str().unwrap();
            assert_eq!(id.len(), 64);
            assert!(id
                .chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));

            let missing = &issues[1];
            assert_eq!(missing["kind"], "missing");
            // Tagged raw states: absent relative candidate is present:false, value:null.
            assert_eq!(missing["instanceRelativeCandidate"]["present"], false);
            assert!(missing["instanceRelativeCandidate"]["value"].is_null());
            assert_eq!(missing["absoluteCandidate"]["present"], true);
            assert_eq!(missing["absoluteCandidate"]["value"], "/gone/x");
        }

        #[test]
        fn structural_issue_forces_minimum_count_and_invalid_kind() {
            let structural = StructuralIssue {
                source: ProjectSource::ProjectPaths,
                reason: "plural primary is not an array of strings".to_string(),
                raw_absolute: RawJsonField {
                    present: true,
                    value: Some(serde_json::json!("not-an-array")),
                },
                raw_relative: RawJsonField {
                    present: false,
                    value: None,
                },
            };
            let settings = settings_with_state(state_with(vec![], vec![structural]));
            let snap = settings_snapshot_from(&settings, None);
            let json = serde_json::to_value(&snap).unwrap();
            let resolution = &json["projectPathResolution"];
            // Corruption-only startup is never pristine: min count of 1.
            assert_eq!(resolution["activeRegistrationCount"], 1);
            let issues = resolution["issues"].as_array().unwrap();
            assert_eq!(issues.len(), 1);
            assert_eq!(issues[0]["kind"], "invalid");
            // Invalid candidates are tagged RawJsonFieldState (wrong-typed value).
            assert_eq!(issues[0]["absoluteCandidate"]["present"], true);
            assert_eq!(issues[0]["absoluteCandidate"]["value"], "not-an-array");
        }

        #[test]
        fn issue_id_is_stable_across_index_and_resolved_paths() {
            // Same kind/list/raw fields but different index → same id (index excluded).
            let mut a = missing_pair();
            let mut b = missing_pair();
            b.index = Some(9);
            let settings_a = settings_with_state(state_with(vec![a.clone()], vec![]));
            let settings_b = settings_with_state(state_with(vec![b], vec![]));
            let id_a = issue_id(&settings_snapshot_from(&settings_a, None));
            let id_b = issue_id(&settings_snapshot_from(&settings_b, None));
            assert_eq!(id_a, id_b);
            // A different raw absolute → different id.
            a.raw_absolute = RawStringField::string("/gone/y");
            let settings_c = settings_with_state(state_with(vec![a], vec![]));
            assert_ne!(id_a, issue_id(&settings_snapshot_from(&settings_c, None)));
        }

        #[test]
        fn snapshot_carries_the_instance_settings_file_path() {
            // #1347: the UI names the file that holds the plaintext secrets. Assert
            // agreement with config_dir() rather than a literal path, so the test is
            // independent of where the test binary happens to live.
            let snap = settings_snapshot_from(&AppSettings::default(), None);
            match (crate::config::config_dir(), snap.settings_file_path) {
                (Some(dir), Some(path)) => {
                    assert_eq!(path, dir.join("settings.json").to_string_lossy())
                }
                (None, None) => {}
                (dir, path) => {
                    panic!("settings_file_path disagrees with config_dir: {dir:?} vs {path:?}")
                }
            }
        }

        fn issue_id(snap: &super::super::SettingsSnapshot) -> String {
            match &snap.project_path_resolution.issues[0] {
                ProjectPathIssue::Conflict { id, .. }
                | ProjectPathIssue::Missing { id, .. }
                | ProjectPathIssue::Invalid { id, .. } => id.clone(),
            }
        }
    }

    fn settings_with_single_agent() -> AppSettings {
        AppSettings {
            agents: vec![AgentConfig {
                id: "agent-0".to_string(),
                label: "Codex".to_string(),
                command: "codex".to_string(),
                color: "#000000".to_string(),
                order: None,
                envs: Vec::new(),
                isolated_home: false,
                instructions_filename: None,
                config_seed: None,
                context_regex: None,
                blocking_menus: None,
                backend: Default::default(),
            }],
            ..AppSettings::default()
        }
    }

    #[test]
    #[cfg(windows)]
    fn canonical_compare_key_converts_verbatim_unc() {
        assert_eq!(
            canonical_compare_key(Path::new(r"\\?\UNC\server\share\Repo")),
            "//server/share/repo"
        );
    }

    #[test]
    #[cfg(windows)]
    fn profile_assignment_target_paths_convert_verbatim_unc() {
        let live_by_cwd = BTreeMap::new();
        let identity = crate::config::replica_identity::WgReplicaIdentity {
            agent_name: "dev".to_string(),
            ac_root: PathBuf::from(r"\\?\UNC\server\share\repo\.ac"),
            matrix_dir: PathBuf::from(r"\\?\UNC\server\share\repo\.ac\_agent_dev"),
            identity: "../../_agent_dev".to_string(),
        };

        let target = build_profile_assignment_target(
            Path::new(r"\\?\UNC\server\share\repo\.ac\wg-1\__agent_dev"),
            &identity,
            &crate::config::coding_agent_profiles::ReplicaSelectionState::Unlocked {
                identity: "../../_agent_dev".to_string(),
                pair: None,
                warning: None,
            },
            &live_by_cwd,
        );

        assert_eq!(target.workgroup_path, r"\\server\share\repo\.ac\wg-1");
        assert_eq!(
            target.replica_path,
            r"\\server\share\repo\.ac\wg-1\__agent_dev"
        );
    }

    fn state_for(settings: AppSettings) -> SettingsState {
        Arc::new(RwLock::new(settings))
    }

    fn write_settings_file(dir: &Path, settings: &AppSettings) {
        let json = serde_json::to_vec_pretty(settings).expect("serialize settings");
        std::fs::write(dir.join("settings.json"), json).expect("write settings.json");
    }

    fn write_sessions_file(dir: &Path, sessions: &[PersistedSession]) {
        let json = serde_json::to_vec_pretty(sessions).expect("serialize sessions");
        std::fs::write(dir.join("sessions.json"), json).expect("write sessions.json");
    }

    fn read_sessions_file(dir: &Path) -> Vec<PersistedSession> {
        let json = std::fs::read_to_string(dir.join("sessions.json")).expect("read sessions.json");
        serde_json::from_str(&json).expect("parse sessions.json")
    }

    fn settings_payload_without_keys(settings: &AppSettings, keys: &[&str]) -> AppSettings {
        let mut value = serde_json::to_value(settings).expect("serialize settings payload");
        let object = value
            .as_object_mut()
            .expect("settings payload must serialize to an object");
        for key in keys {
            assert!(
                object.remove(*key).is_some(),
                "settings payload has no `{key}` key to strip; the A7 gate would be a no-op"
            );
        }
        serde_json::from_value(value).expect("deserialize stale settings payload")
    }

    fn assert_single_project(settings: &AppSettings, project: &str) {
        assert_eq!(settings.project_paths, vec![project.to_string()]);
        assert_eq!(settings.project_path.as_deref(), Some(project));
    }

    /// #1077: create a real AC project dir and return its display-canonical path,
    /// so the six-field decoder validates it and the SELECTED runtime path (in the
    /// preserving writer's fresh-decoded return) equals the stored string.
    fn real_ac_project(parent: &Path, name: &str) -> String {
        let dir = parent.join(name);
        std::fs::create_dir_all(dir.join(".ac")).unwrap();
        canonical_display(&dir)
    }

    /// #1077: the display-canonical form of an existing directory - what the
    /// six-field decoder selects. Storing this instead of the raw temp path keeps
    /// runtime-path assertions platform-robust (equal to raw on Windows, the
    /// symlink-resolved canonical on Linux/CI, which the preserving writer publishes).
    fn canonical_display(dir: &Path) -> String {
        crate::config::projects::display_canonical(
            &std::fs::canonicalize(dir).unwrap().to_string_lossy(),
        )
    }

    /// #1077: seed a whole, valid AppSettings object on disk with `keys` removed
    /// (to simulate an absent-key scenario without tripping the preserve writer's
    /// whole-object gate).
    fn write_settings_file_without_keys(dir: &Path, settings: &AppSettings, keys: &[&str]) {
        let mut value = serde_json::to_value(settings).expect("serialize settings");
        let object = value.as_object_mut().expect("settings object");
        for key in keys {
            object.remove(*key);
        }
        std::fs::write(
            dir.join("settings.json"),
            serde_json::to_vec_pretty(&value).expect("serialize settings json"),
        )
        .expect("write settings.json");
    }

    fn api_server_command_test_app(settings: AppSettings) -> tauri::App {
        let session_mgr = Arc::new(RwLock::new(SessionManager::new()));
        let store_dir = tempfile::tempdir().expect("create API store directory");
        let message_store = Arc::new(
            crate::api::message_store::MessageStore::open(
                store_dir
                    .path()
                    .join(crate::api::message_store::DB_FILENAME),
            )
            .expect("open API message store"),
        );
        let app = crate::test_support::test_builder()
            .manage(ApiServerHandle::default())
            .manage(crate::api::message_store::MessageStoreState::ready(
                message_store,
            ))
            .manage(store_dir)
            .manage(state_for(settings))
            .manage(Arc::clone(&session_mgr))
            .manage(crate::shutdown::ShutdownSignal::new())
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("build test app");
        let idle_detector = crate::pty::idle_detector::IdleDetector::new(|_| {}, |_| {});
        let git_watcher = crate::pty::git_watcher::GitWatcher::new(
            Arc::clone(&session_mgr),
            app.handle().clone(),
        );
        let pty_mgr = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new(
            Arc::new(Mutex::new(HashMap::new())),
            idle_detector,
            git_watcher,
            None,
            None,
        )));
        app.manage(pty_mgr);
        app
    }

    #[test]
    fn web_remote_url_maps_wildcards_and_preserves_concrete_ips() {
        let cases = [
            (
                "0.0.0.0",
                "http://127.0.0.1:8888/?window=browser&remoteToken=test-token",
            ),
            (
                "::",
                "http://[::1]:8888/?window=browser&remoteToken=test-token",
            ),
            (
                "0:0:0:0:0:0:0:0",
                "http://[::1]:8888/?window=browser&remoteToken=test-token",
            ),
            (
                "0::0",
                "http://[::1]:8888/?window=browser&remoteToken=test-token",
            ),
            (
                "127.0.0.1",
                "http://127.0.0.1:8888/?window=browser&remoteToken=test-token",
            ),
            (
                "192.168.1.50",
                "http://192.168.1.50:8888/?window=browser&remoteToken=test-token",
            ),
            (
                "::1",
                "http://[::1]:8888/?window=browser&remoteToken=test-token",
            ),
            (
                "2001:db8::25",
                "http://[2001:db8::25]:8888/?window=browser&remoteToken=test-token",
            ),
        ];

        for (bind, expected) in cases {
            assert_eq!(web_remote_url(bind, 8888, "test-token").unwrap(), expected);
        }
    }

    #[test]
    fn web_remote_url_rejects_invalid_bind() {
        let error = web_remote_url("not-an-ip", 8888, "test-token").unwrap_err();

        assert!(error.starts_with("Invalid web server bind address 'not-an-ip':"));
    }

    #[test]
    fn api_server_probe_addr_maps_wildcards_to_loopback() {
        assert_eq!(
            api_server_probe_addr("0.0.0.0", 9906).unwrap().to_string(),
            "127.0.0.1:9906"
        );
        assert_eq!(
            api_server_probe_addr("127.0.0.1", 9906)
                .unwrap()
                .to_string(),
            "127.0.0.1:9906"
        );
        assert_eq!(
            api_server_probe_addr("::", 9906).unwrap().to_string(),
            "[::1]:9906"
        );
        assert_eq!(
            api_server_probe_addr("::1", 9906).unwrap().to_string(),
            "[::1]:9906"
        );
    }

    #[tokio::test]
    async fn api_server_probe_addr_detects_wildcard_listener_via_loopback() {
        let listener = tokio::net::TcpListener::bind((Ipv4Addr::UNSPECIFIED, 0))
            .await
            .expect("bind wildcard test listener");
        let port = listener.local_addr().expect("listener local addr").port();
        let probe_addr = api_server_probe_addr("0.0.0.0", port).expect("probe addr");

        assert_eq!(probe_addr.ip(), IpAddr::V4(Ipv4Addr::LOCALHOST));
        assert!(is_tcp_socket_listening(probe_addr).await);
    }

    #[test]
    fn web_server_probe_addr_maps_wildcards_to_loopback() {
        assert_eq!(
            web_server_probe_addr("0.0.0.0", 8765).unwrap(),
            "127.0.0.1:8765".parse().unwrap()
        );
        assert_eq!(
            web_server_probe_addr("[::]", 8766).unwrap(),
            "[::1]:8766".parse().unwrap()
        );
        assert_eq!(
            web_server_probe_addr("192.168.1.50", 8767).unwrap(),
            "192.168.1.50:8767".parse().unwrap()
        );
        assert_eq!(
            web_server_probe_addr("[2001:db8::25]", 8768).unwrap(),
            "[2001:db8::25]:8768".parse().unwrap()
        );
    }

    #[tokio::test]
    async fn web_server_probe_addr_detects_wildcard_listener_via_loopback() {
        let listener = tokio::net::TcpListener::bind((Ipv4Addr::UNSPECIFIED, 0))
            .await
            .expect("bind wildcard web listener");
        let port = listener.local_addr().expect("wildcard address").port();
        let probe_addr = web_server_probe_addr("0.0.0.0", port).expect("web probe addr");

        assert_eq!(probe_addr.ip(), IpAddr::V4(Ipv4Addr::LOCALHOST));
        assert!(is_tcp_socket_listening(probe_addr).await);
    }

    #[tokio::test]
    async fn api_server_status_reports_running_for_managed_wildcard_server() {
        let listener = tokio::net::TcpListener::bind((Ipv4Addr::UNSPECIFIED, 0))
            .await
            .expect("bind wildcard test listener");
        let bound_addr = listener.local_addr().expect("listener local addr");
        let settings = AppSettings {
            api_server_bind: "0.0.0.0".to_string(),
            api_server_port: bound_addr.port(),
            ..AppSettings::default()
        };
        let app = tauri::test::mock_builder()
            .manage(ApiServerHandle::default())
            .manage(state_for(settings))
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("build test app");

        let shutdown = CancellationToken::new();
        let task_shutdown = shutdown.clone();
        let join = tauri::async_runtime::spawn(async move {
            let _listener = listener;
            task_shutdown.cancelled().await;
        });
        app.state::<ApiServerHandle>()
            .store_if_idle(ApiServerTask::new(join, shutdown, bound_addr))
            .expect("store api server task");

        let running =
            api_server_status(app.state::<ApiServerHandle>(), app.state::<SettingsState>())
                .await
                .expect("status succeeds");

        assert!(running);
        assert!(app
            .state::<ApiServerHandle>()
            .shutdown_running(Duration::from_secs(1))
            .await
            .expect("shutdown stored task"));
    }

    #[tokio::test]
    async fn start_api_server_bind_failure_returns_err_and_status_false() {
        let settings = AppSettings {
            api_server_bind: "192.0.2.1".to_string(),
            api_server_port: 9906,
            ..AppSettings::default()
        };
        let app = api_server_command_test_app(settings);

        let err = start_api_server(
            app.handle().clone(),
            app.state::<ApiServerHandle>(),
            app.state::<SettingsState>(),
            app.state::<Arc<RwLock<SessionManager>>>(),
            app.state::<Arc<Mutex<crate::pty::manager::PtyManager>>>(),
            app.state::<crate::shutdown::ShutdownSignal>(),
        )
        .await
        .expect_err("nonlocal bind should fail readiness");

        assert!(err.contains("API server bind failed"), "{err}");
        assert!(!app.state::<ApiServerHandle>().has_running().unwrap());
        assert!(
            !api_server_status(app.state::<ApiServerHandle>(), app.state::<SettingsState>())
                .await
                .expect("status succeeds")
        );
    }

    /// #1768 fix case. The port is held continuously from reservation to service, so nothing can
    /// take it in between. **This is the discriminating test**: section 7.3 runs it against the
    /// unfixed shape (mutant M1) and records it red, then against the fixed shape and records it
    /// green.
    #[tokio::test]
    async fn api_start_server_adopts_the_prebound_listener_and_never_releases_the_port() {
        let app = api_server_command_test_app(AppSettings::default());
        let listener =
            std::net::TcpListener::bind(("127.0.0.1", 0)).expect("reserve loopback port");
        let port = listener
            .local_addr()
            .expect("reserved loopback address")
            .port();
        // Mutation tripwire. This duplicate handle keeps the port bound even if the
        // implementation drops the listener it was handed, so a drop-then-rebind implementation
        // fails readiness instead of quietly passing.
        let keeper = listener.try_clone().expect("duplicate reserved listener");

        let session_mgr = app.state::<Arc<RwLock<SessionManager>>>();
        let pty_mgr = app.state::<Arc<Mutex<crate::pty::manager::PtyManager>>>();
        let shutdown = tokio_util::sync::CancellationToken::new();
        let start = crate::api::start_server_on_listener(
            listener,
            app.handle().clone(),
            session_mgr.inner().clone(),
            pty_mgr.inner().clone(),
            shutdown.clone(),
        );

        let addr = crate::api::wait_for_startup_ready(start.readiness)
            .await
            .expect("prebound listener adopted");
        assert_eq!(addr.port(), port, "server must serve the reserved port");

        // The port assertion and a bare `TcpStream::connect` are BOTH satisfied by an
        // implementation that keeps `keeper`'s port bound and serves Axum on a different socket:
        // `keeper` is a listening socket, so the OS completes the TCP handshake into its backlog
        // even though nothing ever accepts. Only a timed application-level response proves that
        // THIS server is the thing answering on the reserved address, so that is what is asserted.
        let client = reqwest::Client::builder()
            .no_proxy()
            .build()
            .expect("healthz client");
        let url = format!("http://{}/api/v1/healthz", addr);
        let body = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            client
                .get(&url)
                .send()
                .await
                .expect("healthz request")
                .text()
                .await
                .expect("healthz body")
        })
        .await
        .expect("healthz deadline on the reserved address");
        assert_eq!(
            body, "{\"ok\":true}",
            "the reserved address must be served by this server"
        );

        shutdown.cancel();
        tokio::time::timeout(std::time::Duration::from_secs(5), start.join_handle)
            .await
            .expect("API server shutdown deadline")
            .expect("API server task");
        drop(keeper);
    }

    /// #1768 readiness-error **contract** test for the unchanged `Address` path. It pins the
    /// wording and the shape of the bind-failure readiness error that CI logs show. It is NOT a
    /// control for this change: it exercises only the `Address` path, which this plan does not
    /// alter, so it passes identically on the unfixed and the fixed code. Section 7.3 M1 is the
    /// red-before / green-after evidence; this test is not.
    #[tokio::test]
    async fn api_start_server_address_path_bind_failure_readiness_contract() {
        let app = api_server_command_test_app(AppSettings::default());
        let adversary =
            std::net::TcpListener::bind(("127.0.0.1", 0)).expect("adversary loopback port");
        let port = adversary
            .local_addr()
            .expect("adversary loopback address")
            .port();

        let session_mgr = app.state::<Arc<RwLock<SessionManager>>>();
        let pty_mgr = app.state::<Arc<Mutex<crate::pty::manager::PtyManager>>>();
        let shutdown = tokio_util::sync::CancellationToken::new();
        let start = crate::api::start_server(
            "127.0.0.1".to_string(),
            port,
            app.handle().clone(),
            session_mgr.inner().clone(),
            pty_mgr.inner().clone(),
            shutdown.clone(),
        );

        let err = crate::api::wait_for_startup_ready(start.readiness)
            .await
            .expect_err("a taken port must fail readiness");
        assert!(err.contains("API server bind failed"), "{err}");
        assert!(err.contains(&port.to_string()), "{err}");

        shutdown.cancel();
        tokio::time::timeout(std::time::Duration::from_secs(5), start.join_handle)
            .await
            .expect("API server shutdown deadline")
            .expect("API server task");
        drop(adversary);
    }

    struct MintApiClientFixture {
        _temp: tempfile::TempDir,
        path: PathBuf,
        project: PathBuf,
        settings: AppSettings,
        root: String,
        expected_fqn: String,
    }

    fn mint_api_client_fixture() -> MintApiClientFixture {
        let temp = tempfile::tempdir().expect("tempdir");
        let project = temp.path().join("proj-a");
        let ac_root = project.join(".ac");
        let matrix = ac_root.join("_agent_alice");
        let replica = ac_root.join("wg-1-devs").join("__agent_alice");
        std::fs::create_dir_all(&matrix).expect("create matrix");
        std::fs::create_dir_all(&replica).expect("create replica");
        std::fs::write(
            replica.join("config.json"),
            r#"{"identity":"../../_agent_alice"}"#,
        )
        .expect("write replica config");

        let root_path = std::fs::canonicalize(&replica).expect("canonical replica");
        let root = crate::path_utils::path_to_string_without_windows_verbatim_prefix(&root_path);
        let expected_fqn = crate::config::teams::agent_fqn_from_path(&root);
        let settings = AppSettings {
            project_paths: vec![project.to_string_lossy().to_string()],
            ..AppSettings::default()
        };

        MintApiClientFixture {
            path: temp.path().join(auth::REGISTRY_FILENAME),
            _temp: temp,
            project,
            settings,
            root,
            expected_fqn,
        }
    }

    #[test]
    fn mint_api_client_command_helper_returns_token_and_stores_hash_only_with_default_expiry() {
        let fixture = mint_api_client_fixture();
        let issued_at = "2026-01-01T00:00:00Z";
        let expected_expires_at = (chrono::DateTime::parse_from_rfc3339(issued_at)
            .unwrap()
            .with_timezone(&chrono::Utc)
            + chrono::Duration::hours(MINT_API_CLIENT_DEFAULT_TTL_HOURS))
        .to_rfc3339();

        let response = mint_api_client_with_path(
            &fixture.path,
            &fixture.settings,
            fixture.root.clone(),
            vec![
                " send ".to_string(),
                "".to_string(),
                "list-peers-lean".to_string(),
            ],
            Some("gui mint".to_string()),
            None,
            "client-1".to_string(),
            "plain-secret".to_string(),
            issued_at.to_string(),
        )
        .expect("mint succeeds");

        assert_eq!(response.client_id, "client-1");
        assert_eq!(response.token, "plain-secret");
        assert_eq!(response.bound_root, fixture.root);
        assert_eq!(response.bound_fqn, fixture.expected_fqn);
        assert_eq!(response.scopes, vec!["send", "list-peers-lean"]);
        assert_eq!(
            response.expires_at.as_deref(),
            Some(expected_expires_at.as_str())
        );
        assert_eq!(response.note, MINT_API_CLIENT_NOTE);

        let raw = std::fs::read_to_string(&fixture.path).expect("read registry");
        assert!(raw.contains("client-1"));
        assert!(raw.contains("sha256:"));
        assert!(!raw.contains("plain-secret"));

        let registry = auth::list(&fixture.path);
        assert_eq!(registry.clients.len(), 1);
        let client = &registry.clients[0];
        assert_eq!(client.client_id, "client-1");
        assert_eq!(client.label, "gui mint");
        assert_eq!(client.bound_fqn, fixture.expected_fqn);
        assert_eq!(client.bound_root, response.bound_root);
        assert_eq!(client.scopes, response.scopes);
        assert_eq!(client.expires_at, response.expires_at);
        assert_ne!(client.token_hash, "plain-secret");
    }

    #[test]
    fn mint_api_client_rejects_unknown_root() {
        let fixture = mint_api_client_fixture();
        let unknown_root = fixture
            .project
            .join(".ac")
            .join("wg-1-devs")
            .join("__agent_missing");

        let err = mint_api_client_with_path(
            &fixture.path,
            &fixture.settings,
            unknown_root.to_string_lossy().to_string(),
            vec!["send".to_string()],
            None,
            None,
            "client-1".to_string(),
            "plain-secret".to_string(),
            "2026-01-01T00:00:00Z".to_string(),
        )
        .expect_err("unknown root should fail");

        assert!(err.contains("unknown or invalid root"), "{err}");
        assert!(!fixture.path.exists());
    }

    #[test]
    fn mint_api_client_rejects_expiry_beyond_max() {
        let fixture = mint_api_client_fixture();
        let issued_at = chrono::DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let over_max =
            (issued_at + chrono::Duration::days(MINT_API_CLIENT_MAX_TTL_DAYS + 1)).to_rfc3339();

        let err = mint_api_client_with_path(
            &fixture.path,
            &fixture.settings,
            fixture.root,
            vec!["send".to_string()],
            None,
            Some(over_max),
            "client-1".to_string(),
            "plain-secret".to_string(),
            issued_at.to_rfc3339(),
        )
        .expect_err("over-max expiry should fail");

        assert!(
            err.contains("expires exceeds maximum API client lifetime"),
            "{err}"
        );
        assert!(!fixture.path.exists());
    }

    #[test]
    fn web_server_owned_status_maps_owned_running() {
        let status = build_web_server_owned_status(
            "127.0.0.1".to_string(),
            8765,
            WebServerLifecycle::Running,
            false,
            None,
        );

        assert!(!status.listening, "listening comes from the probe");
        assert!(status.owned);
        assert!(!status.external_listening);
        assert!(status.open_allowed);
        assert_eq!(status.bind, "127.0.0.1");
        assert_eq!(status.port, 8765);
        assert_eq!(status.state, WebServerOwnershipState::OwnedRunning);
    }

    #[test]
    fn web_server_owned_status_maps_external_listener() {
        let status = build_web_server_owned_status(
            "127.0.0.1".to_string(),
            8765,
            WebServerLifecycle::Stopped,
            true,
            None,
        );

        assert!(status.listening);
        assert!(!status.owned);
        assert!(status.external_listening);
        assert!(!status.open_allowed);
        assert_eq!(status.state, WebServerOwnershipState::ExternalListening);
    }

    #[test]
    fn web_server_owned_status_maps_stopped() {
        let status = build_web_server_owned_status(
            "127.0.0.1".to_string(),
            8765,
            WebServerLifecycle::Stopped,
            false,
            None,
        );

        assert!(!status.listening);
        assert!(!status.owned);
        assert!(!status.external_listening);
        assert!(!status.open_allowed);
        assert_eq!(status.state, WebServerOwnershipState::Stopped);
    }

    #[test]
    fn web_server_owned_status_maps_starting_and_stopping() {
        for (lifecycle, state, listening) in [
            (
                WebServerLifecycle::Starting,
                WebServerOwnershipState::Starting,
                true,
            ),
            (
                WebServerLifecycle::Stopping,
                WebServerOwnershipState::Stopping,
                false,
            ),
        ] {
            let status = build_web_server_owned_status(
                "0.0.0.0".to_string(),
                8765,
                lifecycle,
                listening,
                None,
            );
            assert_eq!(status.state, state);
            assert_eq!(status.listening, listening);
            assert!(status.owned);
            assert!(!status.external_listening);
            assert!(!status.open_allowed);
        }
    }

    // §1453 D2: el fallo viaja en el status solo si describe el target actual.
    #[test]
    fn web_server_owned_status_carries_matching_bind_failure() {
        let recorded = Some(crate::web::StartServerError::BindFailed {
            bind: "192.168.1.12".to_string(),
            addr: "192.168.1.12:8888".parse().unwrap(),
            detail: "os error 10049".to_string(),
        });
        let failure = select_current_bind_failure(recorded, "192.168.1.12", 8888, false)
            .expect("matching failure must survive");
        assert_eq!(failure.bind, "192.168.1.12");
        assert_eq!(failure.port, 8888);
        assert_eq!(failure.detail, "os error 10049");

        let status = build_web_server_owned_status(
            "192.168.1.12".to_string(),
            8888,
            WebServerLifecycle::Stopped,
            false,
            Some(failure),
        );
        assert_eq!(status.state, WebServerOwnershipState::Stopped);
        assert!(status.bind_failure.is_some());
    }

    // §1453 D2: un fallo de un target anterior (o sobre algo que SI escucha)
    // no pinta el target nuevo. El caso IPv6 bracketed es el fix D-2: con el
    // bind crudo guardado, matchea; con la forma canonica se perdia siempre.
    #[test]
    fn web_server_owned_status_drops_stale_bind_failure() {
        let recorded = || {
            Some(crate::web::StartServerError::BindFailed {
                bind: "192.168.1.12".to_string(),
                addr: "192.168.1.12:8888".parse().unwrap(),
                detail: "os error 10049".to_string(),
            })
        };
        assert!(select_current_bind_failure(recorded(), "192.168.1.12", 9999, false).is_none());
        assert!(select_current_bind_failure(recorded(), "0.0.0.0", 8888, false).is_none());
        assert!(select_current_bind_failure(None, "192.168.1.12", 8888, false).is_none());
        // guardia de listening (D2, higiene)
        assert!(select_current_bind_failure(recorded(), "192.168.1.12", 8888, true).is_none());
        // IPv6 bracketed: el crudo matchea contra el settings crudo
        let v6 = Some(crate::web::StartServerError::BindFailed {
            bind: "[::1]".to_string(),
            addr: "[::1]:8888".parse().unwrap(),
            detail: "os error 10049".to_string(),
        });
        assert!(select_current_bind_failure(v6, "[::1]", 8888, false).is_some());
    }

    // §1453 D3: heuristica de clasificacion sobre nombres reales de ambos mundos.
    #[test]
    fn classify_virtual_interface_names() {
        for physical in [
            "Ethernet",
            "Wi-Fi",
            "Local Area Connection",
            "eth0",
            "enp3s0",
            "en0",
            "wlan0",
        ] {
            assert!(
                !classify_virtual_interface(physical),
                "{physical} must be physical"
            );
        }
        for virtual_name in [
            // el nombre REAL de esta maquina, con parentesis anidados (2.4)
            "vEthernet (WSL (Hyper-V firewall))",
            "vEthernet (WSL)",
            "vEthernet (Default Switch)",
            "Tailscale",
            "VirtualBox Host-Only Network",
            "Docker0",
            "docker0",
            "tailscale0",
            "tun0",
            "utun3",
            "wg0",
            "br-1a2b3c",
            "veth42",
            "virbr0",
            "zt0",
            "WireGuard Tunnel",
        ] {
            assert!(
                classify_virtual_interface(virtual_name),
                "{virtual_name} must be virtual"
            );
        }
    }

    // §1453 D1: filtro (loopback, APIPA, unspecified), orden y dedup.
    #[test]
    fn map_web_server_interfaces_filters_and_sorts() {
        let mapped = map_web_server_interfaces(vec![
            ("Tailscale".to_string(), "100.121.138.61".parse().unwrap()),
            ("Ethernet".to_string(), "192.168.1.9".parse().unwrap()),
            ("Ethernet".to_string(), "169.254.10.20".parse().unwrap()),
            (
                "Loopback Pseudo-Interface 1".to_string(),
                "127.0.0.1".parse().unwrap(),
            ),
            ("Wi-Fi".to_string(), "192.168.1.2".parse().unwrap()),
            ("Ethernet".to_string(), "192.168.1.9".parse().unwrap()),
        ]);
        let flat: Vec<(String, String, bool)> = mapped
            .into_iter()
            .map(|i| (i.interface_name, i.address, i.is_virtual))
            .collect();
        assert_eq!(
            flat,
            vec![
                ("Ethernet".to_string(), "192.168.1.9".to_string(), false),
                ("Wi-Fi".to_string(), "192.168.1.2".to_string(), false),
                ("Tailscale".to_string(), "100.121.138.61".to_string(), true),
            ]
        );
    }

    // §1453 D8: el backend PUEDE devolver [] legitimamente (maquina cuyas
    // unicas direcciones son loopback y APIPA). Es la precondicion del memo
    // hasDetection() del frontend: lista vacia = sin evidencia, no evidencia
    // de ausencia.
    #[test]
    fn map_web_server_interfaces_returns_empty_when_all_filtered() {
        let mapped = map_web_server_interfaces(vec![
            (
                "Loopback Pseudo-Interface 1".to_string(),
                "127.0.0.1".parse().unwrap(),
            ),
            ("Ethernet".to_string(), "169.254.10.20".parse().unwrap()),
        ]);
        assert!(mapped.is_empty());
    }

    async fn wait_for_web_lifecycle(
        handle: &WebServerHandle,
        expected: WebServerLifecycle,
    ) -> crate::WebServerLifecycleSnapshot {
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let snapshot = handle.snapshot();
                if snapshot.lifecycle == expected {
                    return snapshot;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("web lifecycle did not converge")
    }

    async fn start_running_web_handle(
        handle: &WebServerHandle,
        bind: &str,
        port: u16,
    ) -> Arc<crate::web::WebSocketAdmission> {
        let lifecycle = handle.clone();
        let bind = bind.to_string();
        let (admission_sender, admission_receiver) = oneshot::channel();
        let waiter = handle.begin_start(
            crate::shutdown::ShutdownSignal::new(),
            move |generation, admission, generation_token| async move {
                assert!(lifecycle.publish_effective_endpoint(generation, bind, port));
                assert!(admission_sender.send(Arc::clone(&admission)).is_ok());
                Ok::<_, String>(Some(tauri::async_runtime::spawn(async move {
                    generation_token.cancelled().await;
                })))
            },
        );
        let admission = admission_receiver.await.expect("admission exposed");
        assert_eq!(waiter.wait().await, Ok(true));
        wait_for_web_lifecycle(handle, WebServerLifecycle::Running).await;
        admission
    }

    #[tokio::test]
    async fn stop_web_server_reports_stopped_and_waits_for_active_terminal() {
        let stopped = WebServerHandle::default();
        assert_eq!(stop_web_server_handle(&stopped).await, Ok(false));

        let handle = WebServerHandle::default();
        let admission = start_running_web_handle(&handle, "127.0.0.1", 8751).await;
        let retained = admission
            .try_acquire()
            .expect("running generation admits retained work");
        let handle_for_stop = handle.clone();
        let mut stop_task =
            tokio::spawn(async move { stop_web_server_handle(&handle_for_stop).await });

        assert!(
            tokio::time::timeout(Duration::from_millis(25), &mut stop_task)
                .await
                .is_err(),
            "Stop cannot report true while admitted work is retained"
        );
        assert_eq!(handle.snapshot().lifecycle, WebServerLifecycle::Stopping);
        drop(retained);
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(2), stop_task)
                .await
                .expect("active Stop did not reach terminal")
                .expect("active Stop task panicked"),
            Ok(true)
        );
        assert_eq!(handle.snapshot().lifecycle, WebServerLifecycle::Stopped);
    }

    #[tokio::test]
    async fn stop_web_server_clears_bind_failure_published_by_late_bind() {
        let handle = WebServerHandle::default();
        let lifecycle = handle.clone();
        let failure_recorder = handle.clone();
        let (output_reached, release_output) = handle.gate_next_start_output();
        let (bind_pending_sender, bind_pending_receiver) = oneshot::channel();
        let (release_bind_sender, release_bind_receiver) = oneshot::channel();
        let start = handle.begin_start(
            crate::shutdown::ShutdownSignal::new(),
            move |generation, _admission, _generation_token| async move {
                assert!(lifecycle.publish_effective_endpoint(
                    generation,
                    "127.0.0.1".to_string(),
                    8753,
                ));
                bind_pending_sender
                    .send(())
                    .expect("report pending bind to the test");
                release_bind_receiver
                    .await
                    .expect("release late bind failure");
                failure_recorder.record_bind_failure(crate::web::StartServerError::BindFailed {
                    bind: "127.0.0.1".to_string(),
                    addr: SocketAddr::from(([127, 0, 0, 1], 8753)),
                    detail: "late bind failure".to_string(),
                });
                Ok::<_, String>(None)
            },
        );
        bind_pending_receiver.await.expect("bind became pending");

        let handle_for_stop = handle.clone();
        let stop = tokio::spawn(async move { stop_web_server_handle(&handle_for_stop).await });
        wait_for_web_lifecycle(&handle, WebServerLifecycle::Stopping).await;
        release_bind_sender
            .send(())
            .expect("release pending bind after Stop linealizes");
        assert!(
            !output_reached
                .await
                .expect("late bind output reaches the supervisor"),
            "Stop must already own the lifecycle transition"
        );
        assert!(
            handle.last_bind_failure().is_some(),
            "the late bind must really publish before terminal cleanup"
        );
        release_output
            .send(())
            .expect("release late bind result processing");

        assert_eq!(
            start.wait().await,
            Err(WEB_SERVER_START_CANCELLED.to_string())
        );
        assert_eq!(stop.await.expect("late-bind Stop task panicked"), Ok(true));
        let status = resolve_web_server_owned_status(
            &handle,
            ("127.0.0.1".to_string(), 8753),
            |_addr| async { false },
        )
        .await;
        assert_eq!(status.state, WebServerOwnershipState::Stopped);
        assert!(status.bind_failure.is_none());
        assert!(handle.last_bind_failure().is_none());
    }

    #[tokio::test]
    async fn stop_web_server_timeout_preserves_stopping_and_shared_deadline() {
        let handle = WebServerHandle::default();
        let admission = start_running_web_handle(&handle, "127.0.0.1", 8752).await;
        let retained = admission
            .try_acquire()
            .expect("running generation admits retained work");
        let first = handle
            .begin_stop_with_timeout(Duration::from_millis(40))
            .expect("running generation is owned");
        drop(first);

        assert_eq!(
            stop_web_server_handle(&handle).await,
            Err("Timed out waiting for web server generation to stop".to_string())
        );
        assert_eq!(handle.snapshot().lifecycle, WebServerLifecycle::Stopping);

        drop(retained);
        wait_for_web_lifecycle(&handle, WebServerLifecycle::Stopped).await;
    }

    #[tokio::test]
    async fn ensure_web_remote_open_allowed_rejects_enabled_settings_without_owned_handle() {
        let settings = AppSettings {
            web_server_enabled: true,
            web_server_bind: "127.0.0.1".to_string(),
            web_server_port: 8765,
            ..AppSettings::default()
        };
        let handle = WebServerHandle::default();
        assert_eq!(
            ensure_web_remote_open_allowed(&settings, &handle).unwrap_err(),
            "Web server is not owned by this app process"
        );

        let lifecycle = handle.clone();
        let (factory_entered_sender, factory_entered_receiver) = oneshot::channel();
        let (factory_release_sender, factory_release_receiver) = oneshot::channel();
        let start = handle.begin_start(
            crate::shutdown::ShutdownSignal::new(),
            move |generation, _admission, generation_token| async move {
                assert!(lifecycle.publish_effective_endpoint(
                    generation,
                    "127.0.0.1".to_string(),
                    8765,
                ));
                assert!(factory_entered_sender.send(()).is_ok());
                factory_release_receiver.await.expect("release factory");
                if generation_token.is_cancelled() {
                    Err(WEB_SERVER_START_CANCELLED.to_string())
                } else {
                    Ok(None)
                }
            },
        );
        factory_entered_receiver.await.expect("factory entered");
        assert_eq!(handle.snapshot().lifecycle, WebServerLifecycle::Starting);
        assert_eq!(
            ensure_web_remote_open_allowed(&settings, &handle).unwrap_err(),
            "Web server is not owned by this app process"
        );
        let stop = handle.begin_stop().expect("starting generation is owned");
        assert_eq!(handle.snapshot().lifecycle, WebServerLifecycle::Stopping);
        assert_eq!(
            ensure_web_remote_open_allowed(&settings, &handle).unwrap_err(),
            "Web server is not owned by this app process"
        );
        assert!(factory_release_sender.send(()).is_ok());
        assert_eq!(
            start.wait().await,
            Err(WEB_SERVER_START_CANCELLED.to_string())
        );
        stop.wait().await.expect("stopping generation drains");

        let mismatched = WebServerHandle::default();
        start_running_web_handle(&mismatched, "127.0.0.1", 8766).await;
        assert_eq!(
            ensure_web_remote_open_allowed(&settings, &mismatched).unwrap_err(),
            "Web server is not owned by this app process"
        );
        mismatched
            .begin_stop()
            .expect("mismatched running generation is owned")
            .wait()
            .await
            .expect("mismatched generation drains");
    }

    #[tokio::test]
    async fn ensure_web_remote_open_allowed_accepts_owned_running_handle() {
        let settings = AppSettings {
            web_server_enabled: true,
            web_server_bind: "127.0.0.1".to_string(),
            web_server_port: 8765,
            ..AppSettings::default()
        };
        let handle = WebServerHandle::default();
        start_running_web_handle(&handle, "127.0.0.1", 8765).await;

        let authorized = ensure_web_remote_open_allowed(&settings, &handle)
            .expect("matching Running generation is authorized");
        assert_eq!(authorized.lifecycle, WebServerLifecycle::Running);
        assert_eq!(authorized.endpoint, Some(("127.0.0.1".to_string(), 8765)));
        assert!(handle.snapshot_is_current(&authorized));

        let stop = handle.begin_stop().expect("running generation is owned");
        assert!(!handle.snapshot_is_current(&authorized));
        assert_eq!(
            ensure_web_remote_open_allowed(&settings, &handle).unwrap_err(),
            "Web server is not owned by this app process"
        );
        stop.wait().await.expect("authorized generation drains");
    }

    #[tokio::test]
    async fn web_server_owned_status_retries_when_starting_revision_changes() {
        let handle = WebServerHandle::default();
        let lifecycle = handle.clone();
        let endpoint_a = ("127.0.0.1".to_string(), 8811);
        let endpoint_b = ("127.0.0.1".to_string(), 8812);
        let (factory_entered_sender, factory_entered_receiver) = oneshot::channel();
        let (factory_release_sender, factory_release_receiver) = oneshot::channel();
        let endpoint_a_for_factory = endpoint_a.clone();
        let start = handle.begin_start(
            crate::shutdown::ShutdownSignal::new(),
            move |generation, _admission, generation_token| async move {
                assert!(lifecycle.publish_effective_endpoint(
                    generation,
                    endpoint_a_for_factory.0,
                    endpoint_a_for_factory.1,
                ));
                assert!(factory_entered_sender.send(()).is_ok());
                factory_release_receiver.await.expect("release factory");
                if generation_token.is_cancelled() {
                    Err(WEB_SERVER_START_CANCELLED.to_string())
                } else {
                    Ok(None)
                }
            },
        );
        factory_entered_receiver.await.expect("factory entered");
        let before = handle.snapshot();
        assert_eq!(before.lifecycle, WebServerLifecycle::Starting);
        assert_eq!(before.endpoint, Some(endpoint_a.clone()));
        let generation = before.generation.expect("starting generation id");

        let (probe_a_sender, probe_a_receiver) = oneshot::channel();
        let (probe_b_sender, probe_b_receiver) = oneshot::channel();
        let replies = Arc::new(Mutex::new(VecDeque::from([
            probe_a_receiver,
            probe_b_receiver,
        ])));
        let (calls_sender, mut calls_receiver) = mpsc::unbounded_channel();
        let status_handle = handle.clone();
        let replies_for_probe = Arc::clone(&replies);
        let status_task = tokio::spawn(async move {
            resolve_web_server_owned_status(
                &status_handle,
                ("127.0.0.1".to_string(), 8899),
                move |addr| {
                    calls_sender.send(addr).expect("record status probe");
                    let reply = replies_for_probe
                        .lock()
                        .unwrap()
                        .pop_front()
                        .expect("configured probe response");
                    async move { reply.await.expect("release status probe") }
                },
            )
            .await
        });
        assert_eq!(
            calls_receiver.recv().await.expect("first probe call"),
            web_server_probe_addr(&endpoint_a.0, endpoint_a.1).unwrap()
        );
        assert!(handle.publish_effective_endpoint(generation, endpoint_b.0.clone(), endpoint_b.1,));
        let after_publish = handle.snapshot();
        assert_eq!(after_publish.generation, before.generation);
        assert_eq!(after_publish.lifecycle, WebServerLifecycle::Starting);
        assert_eq!(after_publish.revision, before.revision + 1);
        assert_eq!(after_publish.endpoint, Some(endpoint_b.clone()));
        assert!(probe_a_sender.send(true).is_ok());
        assert_eq!(
            calls_receiver.recv().await.expect("second probe call"),
            web_server_probe_addr(&endpoint_b.0, endpoint_b.1).unwrap()
        );
        assert!(probe_b_sender.send(false).is_ok());
        let status = status_task.await.expect("status task panicked");
        assert_eq!(status.bind, endpoint_b.0);
        assert_eq!(status.port, endpoint_b.1);
        assert!(!status.listening);
        assert_eq!(status.state, WebServerOwnershipState::Starting);
        assert!(status.owned);
        assert!(!status.open_allowed);

        let stop = handle.begin_stop().expect("starting generation is owned");
        assert!(factory_release_sender.send(()).is_ok());
        assert_eq!(
            start.wait().await,
            Err(WEB_SERVER_START_CANCELLED.to_string())
        );
        stop.wait().await.expect("starting generation drains");
    }

    #[tokio::test]
    async fn web_server_owned_status_retries_when_phase_changes_during_probe() {
        let handle = WebServerHandle::default();
        let endpoint = ("127.0.0.1".to_string(), 8813);
        let admission = start_running_web_handle(&handle, &endpoint.0, endpoint.1).await;
        let retained = admission
            .try_acquire()
            .expect("running generation admits retained work");
        let (running_probe_sender, running_probe_receiver) = oneshot::channel();
        let (stopping_probe_sender, stopping_probe_receiver) = oneshot::channel();
        let replies = Arc::new(Mutex::new(VecDeque::from([
            running_probe_receiver,
            stopping_probe_receiver,
        ])));
        let (calls_sender, mut calls_receiver) = mpsc::unbounded_channel();
        let status_handle = handle.clone();
        let replies_for_probe = Arc::clone(&replies);
        let status_task = tokio::spawn(async move {
            resolve_web_server_owned_status(
                &status_handle,
                ("127.0.0.1".to_string(), 8899),
                move |addr| {
                    calls_sender.send(addr).expect("record status probe");
                    let reply = replies_for_probe
                        .lock()
                        .unwrap()
                        .pop_front()
                        .expect("configured probe response");
                    async move { reply.await.expect("release status probe") }
                },
            )
            .await
        });

        let expected_probe = web_server_probe_addr(&endpoint.0, endpoint.1).unwrap();
        assert_eq!(
            calls_receiver.recv().await.expect("Running probe call"),
            expected_probe
        );
        let stop = handle.begin_stop().expect("running generation is owned");
        assert_eq!(handle.snapshot().lifecycle, WebServerLifecycle::Stopping);
        assert!(running_probe_sender.send(true).is_ok());
        assert_eq!(
            calls_receiver.recv().await.expect("Stopping probe call"),
            expected_probe
        );
        assert!(stopping_probe_sender.send(false).is_ok());
        let status = status_task.await.expect("status task panicked");
        assert_eq!(status.state, WebServerOwnershipState::Stopping);
        assert!(status.owned);
        assert!(!status.listening);
        assert!(!status.external_listening);
        assert!(!status.open_allowed);

        drop(retained);
        stop.wait().await.expect("stopping generation drains");
    }

    #[tokio::test]
    async fn web_server_owned_status_stopping_before_endpoint_uses_configured_fallback() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind configured fallback listener");
        let fallback = listener.local_addr().expect("fallback listener address");
        let handle = WebServerHandle::default();
        let (factory_entered_sender, factory_entered_receiver) = oneshot::channel();
        let (factory_release_sender, factory_release_receiver) = oneshot::channel();
        let start = handle.begin_start(
            crate::shutdown::ShutdownSignal::new(),
            move |_generation, _admission, _generation_token| async move {
                assert!(factory_entered_sender.send(()).is_ok());
                factory_release_receiver.await.expect("release factory");
                Err(WEB_SERVER_START_CANCELLED.to_string())
            },
        );
        factory_entered_receiver.await.expect("factory entered");
        let stop = handle.begin_stop().expect("starting generation is owned");
        let stopping = handle.snapshot();
        assert_eq!(stopping.lifecycle, WebServerLifecycle::Stopping);
        assert_eq!(stopping.endpoint, None);
        let configured = (fallback.ip().to_string(), fallback.port());

        let listening =
            resolve_web_server_owned_status(&handle, configured.clone(), super::is_tcp_listening)
                .await;
        assert_eq!(listening.bind, configured.0);
        assert_eq!(listening.port, configured.1);
        assert!(listening.listening);
        assert_eq!(listening.state, WebServerOwnershipState::Stopping);
        assert!(listening.owned);
        assert!(!listening.external_listening);
        assert!(!listening.open_allowed);

        let failed_probe =
            resolve_web_server_owned_status(&handle, configured, |_addr| async { false }).await;
        assert!(!failed_probe.listening);
        assert_eq!(failed_probe.state, WebServerOwnershipState::Stopping);
        assert!(failed_probe.owned);
        assert!(!failed_probe.external_listening);
        assert!(!failed_probe.open_allowed);

        assert!(factory_release_sender.send(()).is_ok());
        assert_eq!(
            start.wait().await,
            Err(WEB_SERVER_START_CANCELLED.to_string())
        );
        stop.wait().await.expect("stopping generation drains");
    }

    #[tokio::test]
    async fn web_server_owned_status_stopping_retries_when_endpoint_appears_during_fallback_probe()
    {
        let handle = WebServerHandle::default();
        let lifecycle = handle.clone();
        let fallback_a = ("127.0.0.1".to_string(), 8821);
        let endpoint_b = ("127.0.0.1".to_string(), 8822);
        let endpoint_b_for_factory = endpoint_b.clone();
        let (factory_entered_sender, factory_entered_receiver) = oneshot::channel();
        let (publish_sender, publish_receiver) = oneshot::channel();
        let (published_sender, published_receiver) = oneshot::channel();
        let (finish_sender, finish_receiver) = oneshot::channel();
        let start = handle.begin_start(
            crate::shutdown::ShutdownSignal::new(),
            move |generation, _admission, _generation_token| async move {
                assert!(factory_entered_sender.send(()).is_ok());
                publish_receiver
                    .await
                    .expect("release endpoint publication");
                assert!(
                    !lifecycle.publish_effective_endpoint(
                        generation,
                        endpoint_b_for_factory.0,
                        endpoint_b_for_factory.1,
                    ),
                    "Stopping publication updates status but cannot continue start"
                );
                assert!(published_sender.send(()).is_ok());
                finish_receiver.await.expect("release factory terminal");
                Err(WEB_SERVER_START_CANCELLED.to_string())
            },
        );
        factory_entered_receiver.await.expect("factory entered");
        let stop = handle.begin_stop().expect("starting generation is owned");
        let before = handle.snapshot();
        assert_eq!(before.lifecycle, WebServerLifecycle::Stopping);
        assert_eq!(before.endpoint, None);

        let (probe_a_sender, probe_a_receiver) = oneshot::channel();
        let (probe_b_sender, probe_b_receiver) = oneshot::channel();
        let replies = Arc::new(Mutex::new(VecDeque::from([
            probe_a_receiver,
            probe_b_receiver,
        ])));
        let (calls_sender, mut calls_receiver) = mpsc::unbounded_channel();
        let status_handle = handle.clone();
        let replies_for_probe = Arc::clone(&replies);
        let fallback_for_status = fallback_a.clone();
        let status_task = tokio::spawn(async move {
            resolve_web_server_owned_status(&status_handle, fallback_for_status, move |addr| {
                calls_sender.send(addr).expect("record status probe");
                let reply = replies_for_probe
                    .lock()
                    .unwrap()
                    .pop_front()
                    .expect("configured probe response");
                async move { reply.await.expect("release status probe") }
            })
            .await
        });
        assert_eq!(
            calls_receiver.recv().await.expect("fallback probe call"),
            web_server_probe_addr(&fallback_a.0, fallback_a.1).unwrap()
        );
        assert!(publish_sender.send(()).is_ok());
        published_receiver.await.expect("endpoint published");
        let after_publish = handle.snapshot();
        assert_eq!(after_publish.generation, before.generation);
        assert_eq!(after_publish.lifecycle, WebServerLifecycle::Stopping);
        assert_eq!(after_publish.revision, before.revision + 1);
        assert_eq!(after_publish.endpoint, Some(endpoint_b.clone()));
        assert!(probe_a_sender.send(true).is_ok());
        assert_eq!(
            calls_receiver.recv().await.expect("effective probe call"),
            web_server_probe_addr(&endpoint_b.0, endpoint_b.1).unwrap()
        );
        assert!(probe_b_sender.send(false).is_ok());
        let status = status_task.await.expect("status task panicked");
        assert_eq!(status.bind, endpoint_b.0);
        assert_eq!(status.port, endpoint_b.1);
        assert!(!status.listening);
        assert_eq!(status.state, WebServerOwnershipState::Stopping);
        assert!(status.owned);
        assert!(!status.external_listening);
        assert!(!status.open_allowed);

        assert!(finish_sender.send(()).is_ok());
        assert_eq!(
            start.wait().await,
            Err(WEB_SERVER_START_CANCELLED.to_string())
        );
        stop.wait().await.expect("stopping generation drains");
    }

    async fn assert_wildcard_owned_phase_probe(
        handle: &WebServerHandle,
        expected_state: WebServerOwnershipState,
        probe_result: bool,
        port: u16,
    ) {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let calls_for_probe = Arc::clone(&calls);
        let status =
            resolve_web_server_owned_status(handle, ("0.0.0.0".to_string(), port), move |addr| {
                calls_for_probe.lock().unwrap().push(addr);
                async move { probe_result }
            })
            .await;
        assert_eq!(
            *calls.lock().unwrap(),
            vec![SocketAddr::from((Ipv4Addr::LOCALHOST, port))]
        );
        assert_eq!(status.state, expected_state);
        assert_eq!(status.listening, probe_result);
        assert!(status.owned);
        assert!(!status.external_listening);
        assert_eq!(
            status.open_allowed,
            expected_state == WebServerOwnershipState::OwnedRunning
        );
        assert_eq!(status.bind, "0.0.0.0");
        assert_eq!(status.port, port);
    }

    #[tokio::test]
    async fn web_server_owned_status_wildcard_uses_probe_in_all_owned_phases() {
        let port = 8831;
        let handle = WebServerHandle::default();
        let lifecycle = handle.clone();
        let (factory_entered_sender, factory_entered_receiver) = oneshot::channel();
        let (factory_release_sender, factory_release_receiver) = oneshot::channel();
        let (admission_sender, admission_receiver) = oneshot::channel();
        let start = handle.begin_start(
            crate::shutdown::ShutdownSignal::new(),
            move |generation, admission, generation_token| async move {
                assert!(lifecycle.publish_effective_endpoint(
                    generation,
                    "0.0.0.0".to_string(),
                    port,
                ));
                assert!(admission_sender.send(Arc::clone(&admission)).is_ok());
                assert!(factory_entered_sender.send(()).is_ok());
                factory_release_receiver.await.expect("release factory");
                Ok::<_, String>(Some(tauri::async_runtime::spawn(async move {
                    generation_token.cancelled().await;
                })))
            },
        );
        factory_entered_receiver.await.expect("factory entered");
        let admission = admission_receiver.await.expect("admission exposed");
        for result in [true, false] {
            assert_wildcard_owned_phase_probe(
                &handle,
                WebServerOwnershipState::Starting,
                result,
                port,
            )
            .await;
        }

        assert!(factory_release_sender.send(()).is_ok());
        assert_eq!(start.wait().await, Ok(true));
        for result in [true, false] {
            assert_wildcard_owned_phase_probe(
                &handle,
                WebServerOwnershipState::OwnedRunning,
                result,
                port,
            )
            .await;
        }

        let retained = admission
            .try_acquire()
            .expect("running generation admits retained work");
        let stop = handle.begin_stop().expect("running generation is owned");
        for result in [true, false] {
            assert_wildcard_owned_phase_probe(
                &handle,
                WebServerOwnershipState::Stopping,
                result,
                port,
            )
            .await;
        }
        drop(retained);
        stop.wait().await.expect("wildcard generation drains");
    }

    fn profile_assignment_request(
        scope: super::ProfileAssignmentScope,
        confirmed_target_fingerprint: Option<&str>,
        typed_confirmation: Option<&str>,
    ) -> super::ApplyCodingAgentProfileSelectionRequest {
        super::ApplyCodingAgentProfileSelectionRequest {
            target_replica_path: "C:/ac/wg-1/__agent_dev-rust".to_string(),
            coding_agent_id: "dev-rust".to_string(),
            profile: "B".to_string(),
            scope,
            restart_sessions: false,
            confirmed_target_fingerprint: confirmed_target_fingerprint
                .map(|value| value.to_string()),
            typed_confirmation: typed_confirmation.map(|value| value.to_string()),
            assignment_mode: super::AssignmentMode::Ordinary,
            conflict_decision: None,
        }
    }

    /// #1941 - a synthetic enumeration lets the fingerprint unit tests exercise
    /// the typed tuple without a filesystem fixture.
    fn fingerprint_enumeration(paths: &[&str]) -> super::ProfileTargetEnumeration {
        super::ProfileTargetEnumeration {
            anchor_dir: PathBuf::from("c:/ac/wg-1/__agent_a"),
            anchor_matrix_dir: PathBuf::from("c:/ac/_agent_a"),
            candidates: paths
                .iter()
                .enumerate()
                .map(|(index, path)| super::ProfileCandidate {
                    target: super::ProfileAssignmentTarget {
                        workgroup_name: "wg-1".to_string(),
                        workgroup_path: "c:/ac/wg-1".to_string(),
                        replica_name: format!("agent_{index}"),
                        replica_path: (*path).to_string(),
                        identity_path: "../../_agent_a".to_string(),
                        origin_project: Some("ac".to_string()),
                        live_session_ids: Vec::new(),
                        saved_pair: None,
                        selection_state: super::SelectionState::Unlocked,
                        selection_error: None,
                    },
                    matrix_dir: PathBuf::from("c:/ac/_agent_a"),
                })
                .collect(),
            warnings: Vec::new(),
            counts_complete: true,
        }
    }

    #[test]
    fn profile_assignment_fingerprint_uses_typed_fields() {
        let enumeration = fingerprint_enumeration(&["c:/ac/wg-1/__agent_a"]);
        let other = fingerprint_enumeration(&["c:/ac/wg-1/__agent_b"]);
        let eligible = vec!["c:/ac/wg-1/__agent_a".to_string()];

        let base = super::assignment_fingerprint(
            &enumeration,
            &[],
            &super::ProfileAssignmentScope::Kind,
            "assign",
            "ab",
            "C",
            false,
            None,
            &eligible,
        )
        .expect("fingerprint");
        let agent_bc = super::assignment_fingerprint(
            &enumeration,
            &[],
            &super::ProfileAssignmentScope::Kind,
            "assign",
            "a",
            "BC",
            false,
            None,
            &eligible,
        )
        .expect("fingerprint");
        let restart = super::assignment_fingerprint(
            &enumeration,
            &[],
            &super::ProfileAssignmentScope::Kind,
            "assign",
            "ab",
            "C",
            true,
            None,
            &eligible,
        )
        .expect("fingerprint");
        let decision = super::assignment_fingerprint(
            &enumeration,
            &[],
            &super::ProfileAssignmentScope::Kind,
            "assign",
            "ab",
            "C",
            false,
            Some(super::ConflictDecision::ForceReviewed),
            &eligible,
        )
        .expect("fingerprint");
        let other_targets = super::assignment_fingerprint(
            &other,
            &[],
            &super::ProfileAssignmentScope::Kind,
            "assign",
            "ab",
            "C",
            false,
            None,
            &["c:/ac/wg-1/__agent_b".to_string()],
        )
        .expect("fingerprint");

        assert_eq!(base.len(), 64, "sha256 hex");
        assert_eq!(base, base.to_lowercase(), "lowercase hex");
        assert_ne!(base, agent_bc);
        assert_ne!(base, restart);
        assert_ne!(base, decision);
        assert_ne!(base, other_targets);
    }

    #[test]
    fn profile_assignment_confirmation_hint_tracks_broad_scopes() {
        assert!(!super::profile_assignment_requires_explicit_confirmation(
            &super::ProfileAssignmentScope::Replica,
            super::AssignmentMode::Ordinary
        ));
        assert!(super::profile_assignment_requires_explicit_confirmation(
            &super::ProfileAssignmentScope::Replica,
            super::AssignmentMode::AssignAndLock
        ));
        assert!(super::profile_assignment_requires_explicit_confirmation(
            &super::ProfileAssignmentScope::Workgroup,
            super::AssignmentMode::Ordinary
        ));
        assert!(super::profile_assignment_requires_explicit_confirmation(
            &super::ProfileAssignmentScope::Kind,
            super::AssignmentMode::Ordinary
        ));
    }

    #[test]
    fn kind_assignment_accepts_fingerprint_without_typed_confirmation() {
        assert_eq!(
            super::validate_profile_assignment_confirmation(
                &super::ProfileAssignmentScope::Kind,
                super::AssignmentMode::Ordinary,
                Some("fp-kind"),
                "fp-kind",
            ),
            Ok(())
        );
    }

    #[test]
    fn kind_assignment_ignores_legacy_typed_confirmation() {
        let request = profile_assignment_request(
            super::ProfileAssignmentScope::Kind,
            Some("fp-kind"),
            Some("not the old exact phrase"),
        );

        assert_eq!(
            super::validate_profile_assignment_confirmation(
                &request.scope,
                request.assignment_mode,
                request.confirmed_target_fingerprint.as_deref(),
                "fp-kind",
            ),
            Ok(())
        );
    }

    #[test]
    fn broad_assignment_rejects_missing_or_stale_fingerprint() {
        for scope in [
            super::ProfileAssignmentScope::Kind,
            super::ProfileAssignmentScope::Workgroup,
        ] {
            let missing = profile_assignment_request(scope.clone(), None, None);
            let err = super::validate_profile_assignment_confirmation(
                &missing.scope,
                missing.assignment_mode,
                missing.confirmed_target_fingerprint.as_deref(),
                "current-fp",
            )
            .unwrap_err();
            // #2051 - a missing fingerprint is a contract failure, not a change.
            assert!(err.contains("confirmationMissing"), "{err}");
            assert!(!err.contains("Target selection changed"), "{err}");

            let stale = profile_assignment_request(scope, Some("old-fp"), None);
            let err = super::validate_profile_assignment_confirmation(
                &stale.scope,
                stale.assignment_mode,
                stale.confirmed_target_fingerprint.as_deref(),
                "current-fp",
            )
            .unwrap_err();
            assert!(err.contains("Target selection changed"), "{err}");
        }
    }

    #[test]
    fn replica_assign_and_lock_confirmation_shapes() {
        let validate = |mode: super::AssignmentMode, provided: Option<&str>| {
            super::validate_profile_assignment_confirmation(
                &super::ProfileAssignmentScope::Replica,
                mode,
                provided,
                "fp-replica",
            )
        };

        // (a) the preview's own fingerprint is accepted...
        assert_eq!(
            validate(super::AssignmentMode::AssignAndLock, Some("fp-replica")),
            Ok(())
        );
        // (b) ...while NO fingerprint at all is a contract failure, never stale.
        let err = validate(super::AssignmentMode::AssignAndLock, None).unwrap_err();
        assert!(err.contains("confirmationMissing"), "{err}");
        assert!(!err.contains("Target selection changed"), "{err}");
        // (c) a provided, non-matching value is stale.
        let err = validate(super::AssignmentMode::AssignAndLock, Some("old-fp")).unwrap_err();
        assert!(err.contains("stalePreview"), "{err}");
        // (d) the legacy replica ordinary path still accepts no fingerprint (D2).
        assert_eq!(validate(super::AssignmentMode::Ordinary, None), Ok(()));
        // (e) even ordinary is stale when a provided value does not match.
        let err = validate(super::AssignmentMode::Ordinary, Some("old-fp")).unwrap_err();
        assert!(err.contains("stalePreview"), "{err}");
    }

    #[test]
    fn settings_draft_events_flag_agent_base_command_edit() {
        // #592/#597 - a bare agent base-command edit changes the effective-command
        // drift hash, so save_settings_draft must emit coding_agent_profiles_updated
        // even though the profile cells and envs are untouched. This is the bug the
        // user hit editing `claude` -> `claude-amp -c ...` in Settings -> Coding Agents.
        let before = settings_with_single_agent();
        let mut after = settings_with_single_agent();
        after.agents[0].command = "codex --resume".to_string();

        let events = super::settings_draft_update_events(&before, &after);
        assert!(
            events.profiles_changed,
            "an agent base-command edit must flag a profiles refresh"
        );
        assert!(events.env_agent_ids.is_empty(), "no env changed");
    }

    #[test]
    fn settings_draft_events_quiet_for_non_drift_field_edit() {
        // A non-drift field (label) changing must NOT emit a profiles refresh, so the
        // fix does not over-fire on unrelated settings saves.
        let before = settings_with_single_agent();
        let mut after = settings_with_single_agent();
        after.agents[0].label = "Renamed".to_string();

        let events = super::settings_draft_update_events(&before, &after);
        assert!(!events.profiles_changed, "a label edit is not drift");
        assert!(events.env_agent_ids.is_empty());
    }

    #[test]
    fn settings_draft_events_flag_agent_env_edit() {
        // Regression guard: the pre-existing env path still flags the agent id (the
        // base env is also a drift-hash input, surfaced via the env event).
        let before = settings_with_single_agent();
        let mut after = settings_with_single_agent();
        after.agents[0].envs = vec![CodingAgentEnv {
            key: "FOO".to_string(),
            value: "bar".to_string(),
            source: CodingAgentEnvSource::User,
            enabled: true,
        }];

        let events = super::settings_draft_update_events(&before, &after);
        assert_eq!(events.env_agent_ids, vec!["agent-0".to_string()]);
        assert!(
            !events.profiles_changed,
            "env-only edit is not a profiles change"
        );
    }

    #[tokio::test]
    async fn restart_failure_classification_depends_on_session_survival() {
        let manager = SessionManager::new();
        let session = manager
            .create_session(
                "codex".to_string(),
                Vec::new(),
                "C:/work/repo".to_string(),
                Some("codex".to_string()),
                Some("Codex".to_string()),
                Vec::new(),
                false,
                crate::pty::backend::SessionBackendKind::LocalProcess,
            )
            .await
            .unwrap();
        let session_mgr = Arc::new(RwLock::new(manager));

        let (survived_code, survived_destroyed) =
            super::classify_restart_failure(&session_mgr, session.id).await;
        assert_eq!(survived_code, "restartFailed");
        assert!(!survived_destroyed);

        let (missing_code, missing_destroyed) =
            super::classify_restart_failure(&session_mgr, uuid::Uuid::new_v4()).await;
        assert_eq!(missing_code, "destroyedButNotRecreated");
        assert!(missing_destroyed);
    }

    #[tokio::test]
    async fn invalid_env_settings_update_leaves_live_settings_unchanged() {
        let mut original = settings_with_single_agent();
        original.agents[0].envs = vec![CodingAgentEnv {
            key: "OPENAI_API_BASE".to_string(),
            value: "current".to_string(),
            source: CodingAgentEnvSource::User,
            enabled: true,
        }];
        original.agents[0].isolated_home = true;
        let state = state_for(original.clone());

        let err = persist_coding_agent_env_settings_update(
            &state,
            "agent-0",
            vec![CodingAgentEnv {
                key: "AGENTSCOMMANDER_TOKEN".to_string(),
                value: "bad".to_string(),
                source: CodingAgentEnvSource::User,
                enabled: true,
            }],
            false,
        )
        .await
        .unwrap_err();

        assert!(err.contains("reserved by AgentsCommander"), "{err}");
        let live = state.read().await;
        assert_eq!(live.agents[0].envs, original.agents[0].envs);
        assert_eq!(
            live.agents[0].isolated_home,
            original.agents[0].isolated_home
        );
    }

    #[tokio::test]
    async fn invalid_profile_settings_update_leaves_live_settings_unchanged() {
        let original = settings_with_single_agent();
        let mut invalid_profiles = original.coding_agent_profiles.clone();
        invalid_profiles
            .profiles_by_agent
            .entry("agent-0".to_string())
            .or_default()
            .insert(
                "A".to_string(),
                ProfileCellConfig {
                    enabled: true,
                    command: String::new(),
                    env: BTreeMap::from([("AGENTSCOMMANDER_ROOT".to_string(), "bad".to_string())]),
                    notes: String::new(),
                },
            );
        let state = state_for(original.clone());

        let err = persist_coding_agent_profiles_update(&state, invalid_profiles)
            .await
            .unwrap_err();

        assert!(err.contains("reserved by AgentsCommander"), "{err}");
        let live = state.read().await;
        assert_eq!(live.coding_agent_profiles, original.coding_agent_profiles);
    }

    #[tokio::test]
    async fn invalid_settings_draft_transaction_leaves_generic_profiles_and_env_unchanged() {
        let mut original = settings_with_single_agent();
        original.sidebar_style = "noir-minimal".to_string();
        original.agents[0].envs = vec![CodingAgentEnv {
            key: "OPENAI_API_BASE".to_string(),
            value: "current".to_string(),
            source: CodingAgentEnvSource::User,
            enabled: true,
        }];
        original.agents[0].isolated_home = true;
        original
            .coding_agent_profiles
            .profiles_by_agent
            .entry("agent-0".to_string())
            .or_default()
            .insert(
                "B".to_string(),
                ProfileCellConfig {
                    enabled: true,
                    command: "codex --current-profile".to_string(),
                    env: BTreeMap::new(),
                    notes: String::new(),
                },
            );
        let state = state_for(original.clone());

        let mut draft = original.clone();
        draft.sidebar_style = "command-center".to_string();
        draft.agents[0].envs = vec![CodingAgentEnv {
            key: "OPENAI_API_BASE".to_string(),
            value: "draft".to_string(),
            source: CodingAgentEnvSource::User,
            enabled: true,
        }];
        draft.agents[0].isolated_home = false;
        draft
            .coding_agent_profiles
            .profiles_by_agent
            .entry("agent-0".to_string())
            .or_default()
            .insert(
                "C".to_string(),
                ProfileCellConfig {
                    enabled: true,
                    command: "codex --draft-profile".to_string(),
                    env: BTreeMap::from([("AGENTSCOMMANDER_TOKEN".to_string(), "bad".to_string())]),
                    notes: String::new(),
                },
            );

        let err = persist_settings_draft_update_with_saver(
            &state,
            draft,
            |_| -> Result<AppSettings, String> { panic!("save must not run") },
        )
        .await
        .unwrap_err();

        assert!(err.contains("reserved by AgentsCommander"), "{err}");
        let live = state.read().await;
        assert_eq!(live.sidebar_style, original.sidebar_style);
        assert_eq!(live.coding_agent_profiles, original.coding_agent_profiles);
        assert_eq!(live.agents[0].envs, original.agents[0].envs);
        assert_eq!(
            live.agents[0].isolated_home,
            original.agents[0].isolated_home
        );
    }

    #[tokio::test]
    async fn settings_draft_transaction_save_failure_leaves_live_settings_unchanged() {
        let mut original = settings_with_single_agent();
        original.agents[0].envs = vec![CodingAgentEnv {
            key: "OPENAI_API_BASE".to_string(),
            value: "current".to_string(),
            source: CodingAgentEnvSource::User,
            enabled: true,
        }];
        original.agents[0].isolated_home = true;
        let state = state_for(original.clone());

        let mut draft = original.clone();
        draft.sidebar_style = "command-center".to_string();
        draft.agents[0].envs = vec![CodingAgentEnv {
            key: "OPENAI_API_BASE".to_string(),
            value: "draft".to_string(),
            source: CodingAgentEnvSource::User,
            enabled: true,
        }];
        draft.agents[0].isolated_home = false;

        let err = persist_settings_draft_update_with_saver(&state, draft, |_| {
            Err("simulated save failure".to_string())
        })
        .await
        .unwrap_err();

        assert_eq!(err, "simulated save failure");
        let live = state.read().await;
        assert_eq!(live.sidebar_style, original.sidebar_style);
        assert_eq!(live.coding_agent_profiles, original.coding_agent_profiles);
        assert_eq!(live.agents[0].envs, original.agents[0].envs);
        assert_eq!(
            live.agents[0].isolated_home,
            original.agents[0].isolated_home
        );
    }

    async fn assert_narrow_setter_save_failure_rolls_back(
        field_name: &'static str,
        mutate_candidate: impl FnOnce(&mut AppSettings) + Send,
        assert_candidate: impl FnOnce(&AppSettings) + Send,
    ) {
        let mut original = settings_with_single_agent();
        original.sounds_enabled = true;
        original.theme_light = false;
        let state = state_for(original.clone());
        let expected_err = format!("simulated save failure for {field_name}");

        let err = persist_narrow_settings_update_with_saver(
            &state,
            |candidate| {
                mutate_candidate(candidate);
                assert_candidate(candidate);
            },
            |_| Err(expected_err.clone()),
        )
        .await
        .unwrap_err();

        assert_eq!(err, expected_err);
        let live = state.read().await;
        assert_eq!(live.sounds_enabled, original.sounds_enabled, "{field_name}");
        assert_eq!(live.theme_light, original.theme_light, "{field_name}");
    }

    #[tokio::test]
    async fn narrow_settings_setter_save_failure_leaves_live_settings_unchanged() {
        assert_narrow_setter_save_failure_rolls_back(
            "sounds_enabled",
            |candidate| candidate.sounds_enabled = false,
            |candidate| assert!(!candidate.sounds_enabled),
        )
        .await;
        assert_narrow_setter_save_failure_rolls_back(
            "theme_light",
            |candidate| candidate.theme_light = true,
            |candidate| assert!(candidate.theme_light),
        )
        .await;
    }

    // ── #2306 P2 - ordered move + whole-settings order protection ────────

    fn agent_with_order(id: &str, order: Option<u32>) -> AgentConfig {
        AgentConfig {
            id: id.to_string(),
            label: format!("{id}-label"),
            command: format!("{id}-cmd"),
            color: "#000000".to_string(),
            order,
            envs: Vec::new(),
            isolated_home: false,
            instructions_filename: None,
            config_seed: None,
            context_regex: None,
            blocking_menus: None,
            backend: Default::default(),
        }
    }

    fn settings_with_agents(ids: &[&str]) -> AppSettings {
        AppSettings {
            agents: ids
                .iter()
                .enumerate()
                .map(|(index, id)| agent_with_order(id, Some(index as u32)))
                .collect(),
            ..AppSettings::default()
        }
    }

    fn order_ids(agents: &[AgentConfig]) -> Vec<String> {
        agents.iter().map(|agent| agent.id.clone()).collect()
    }

    fn order_positions(agents: &[AgentConfig]) -> Vec<Option<u32>> {
        agents.iter().map(|agent| agent.order).collect()
    }

    fn disk_ids_and_orders(path: &Path) -> (Vec<String>, Vec<u64>) {
        let raw = std::fs::read_to_string(path).expect("read settings.json");
        let disk: Value = serde_json::from_str(&raw).expect("parse settings.json");
        let agents = disk["agents"].as_array().expect("agents array");
        (
            agents
                .iter()
                .map(|agent| agent["id"].as_str().unwrap().to_string())
                .collect(),
            agents
                .iter()
                .map(|agent| agent["order"].as_u64().unwrap())
                .collect(),
        )
    }

    /// Base `settings.json` + `settings.local.json` whose local overlay owns the
    /// top-level `agents` key; returns the base path. The caller loads through
    /// the production loader so ownership is genuine.
    fn seed_overlay_agents_fixture(dir: &Path, local_agents: &Value) -> PathBuf {
        let path = dir.join("settings.json");
        std::fs::write(
            &path,
            serde_json::to_string_pretty(&json!({
                "defaultShell": "test-shell",
                "defaultShellArgs": [],
                "rootToken": "base-token",
                "agents": [
                    { "id": "base-a", "label": "Base A", "command": "claude", "color": "#111111" },
                    { "id": "base-b", "label": "Base B", "command": "claude", "color": "#222222" }
                ]
            }))
            .unwrap(),
        )
        .unwrap();
        std::fs::write(
            dir.join("settings.local.json"),
            serde_json::to_string_pretty(&json!({ "agents": local_agents })).unwrap(),
        )
        .unwrap();
        path
    }

    #[tokio::test]
    async fn agent_order_update_keeps_live_survivors_and_appends_incoming_only() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("settings.json");
        let live = settings_with_agents(&["a", "b", "d"]);
        let state = state_for(live.clone());
        // The incoming snapshot carries a forged/stale ordinal order and the
        // incoming membership: `d` is gone, `c` is new, `b` was edited.
        let mut incoming = AppSettings {
            agents: vec![
                agent_with_order("c", Some(9)),
                agent_with_order("a", Some(50)),
                agent_with_order("b", Some(0)),
            ],
            ..live.clone()
        };
        incoming.agents[2].label = "b-edited".to_string();

        let written = persist_protected_settings_update_with_saver(&state, incoming, |candidate| {
            crate::config::settings::save_settings_to_path_preserving_project_paths(
                candidate, &path,
            )
        })
        .await
        .unwrap();

        assert_eq!(order_ids(&written.agents), ["a", "b", "c"]);
        assert_eq!(
            order_positions(&written.agents),
            [Some(0), Some(1), Some(2)]
        );
        assert_eq!(written.agents[1].label, "b-edited");
        let (disk_ids, disk_orders) = disk_ids_and_orders(&path);
        assert_eq!(disk_ids, ["a", "b", "c"]);
        assert_eq!(disk_orders, [0, 1, 2]);
        let reloaded = crate::config::settings::load_settings_from_path(&path);
        assert_eq!(order_ids(&reloaded.agents), ["a", "b", "c"]);
        let live_now = state.read().await;
        assert_eq!(order_ids(&live_now.agents), ["a", "b", "c"]);
    }

    #[tokio::test]
    async fn agent_order_stale_draft_cannot_undo_a_move_and_keeps_incoming_records() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("settings.json");
        // Live state after moving `b` above `a`.
        let state = state_for(settings_with_agents(&["b", "a", "c"]));
        // Stale draft taken before the move.
        let mut stale = settings_with_agents(&["a", "b", "c"]);
        stale.agents[1].label = "b-edited-in-draft".to_string();

        let (written, _events) =
            persist_settings_draft_update_with_saver(&state, stale, |candidate| {
                crate::config::settings::save_settings_to_path_preserving_project_paths(
                    candidate, &path,
                )
            })
            .await
            .unwrap();

        assert_eq!(order_ids(&written.agents), ["b", "a", "c"]);
        assert_eq!(written.agents[0].label, "b-edited-in-draft");
        assert_eq!(
            order_positions(&written.agents),
            [Some(0), Some(1), Some(2)]
        );
        let (disk_ids, disk_orders) = disk_ids_and_orders(&path);
        assert_eq!(disk_ids, ["b", "a", "c"]);
        assert_eq!(disk_orders, [0, 1, 2]);
    }

    #[tokio::test]
    async fn agent_order_draft_add_appends_and_remove_compacts() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("settings.json");
        let live = settings_with_agents(&["a", "b", "c"]);
        let state = state_for(live.clone());

        // Remove `b`: survivors keep the live order and ordinals compact.
        let mut removed = live.clone();
        removed.agents.retain(|agent| agent.id != "b");
        let (written, _) = persist_settings_draft_update_with_saver(&state, removed, |candidate| {
            crate::config::settings::save_settings_to_path_preserving_project_paths(
                candidate, &path,
            )
        })
        .await
        .unwrap();
        assert_eq!(order_ids(&written.agents), ["a", "c"]);
        assert_eq!(order_positions(&written.agents), [Some(0), Some(1)]);

        // Add `d` at the end of the effective list, as the modal does.
        let mut added = written.clone();
        added.agents.push(agent_with_order("d", None));
        let (written, _) = persist_settings_draft_update_with_saver(&state, added, |candidate| {
            crate::config::settings::save_settings_to_path_preserving_project_paths(
                candidate, &path,
            )
        })
        .await
        .unwrap();
        assert_eq!(order_ids(&written.agents), ["a", "c", "d"]);
        assert_eq!(
            order_positions(&written.agents),
            [Some(0), Some(1), Some(2)]
        );
        let (disk_ids, disk_orders) = disk_ids_and_orders(&path);
        assert_eq!(disk_ids, ["a", "c", "d"]);
        assert_eq!(disk_orders, [0, 1, 2]);
    }

    #[tokio::test]
    async fn agent_order_case_distinct_ids_stay_distinct() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("settings.json");
        let state = state_for(settings_with_agents(&["a"]));
        let incoming = settings_with_agents(&["a", "A"]);

        let written = persist_protected_settings_update_with_saver(&state, incoming, |candidate| {
            crate::config::settings::save_settings_to_path_preserving_project_paths(
                candidate, &path,
            )
        })
        .await
        .unwrap();

        assert_eq!(order_ids(&written.agents), ["a", "A"]);
    }

    #[tokio::test]
    async fn agent_order_draft_save_failure_leaves_live_settings_unchanged() {
        let state = state_for(settings_with_agents(&["a", "b", "c"]));
        let stale = settings_with_agents(&["c", "b", "a"]);

        let err = persist_settings_draft_update_with_saver(&state, stale, |_| {
            Err("simulated order save failure".to_string())
        })
        .await
        .unwrap_err();

        assert_eq!(err, "simulated order save failure");
        let live = state.read().await;
        assert_eq!(order_ids(&live.agents), ["a", "b", "c"]);
        assert_eq!(order_positions(&live.agents), [Some(0), Some(1), Some(2)]);
    }

    #[tokio::test]
    async fn move_coding_agent_with_saver_returns_written_order_and_persists_ordinals() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("settings.json");
        let state = state_for(settings_with_agents(&["a", "b", "c"]));

        let ids = move_coding_agent_inner_with_saver(
            &state,
            "b",
            "a",
            AgentMoveDirection::Up,
            |candidate| {
                crate::config::settings::save_settings_to_path_preserving_project_paths(
                    candidate, &path,
                )
            },
        )
        .await
        .unwrap();

        assert_eq!(ids, ["b", "a", "c"]);
        let (disk_ids, disk_orders) = disk_ids_and_orders(&path);
        assert_eq!(disk_ids, ["b", "a", "c"]);
        assert_eq!(disk_orders, [0, 1, 2]);
        let reloaded = crate::config::settings::load_settings_from_path(&path);
        assert_eq!(order_ids(&reloaded.agents), ["b", "a", "c"]);
        let live = state.read().await;
        assert_eq!(order_ids(&live.agents), ["b", "a", "c"]);
        assert_eq!(order_positions(&live.agents), [Some(0), Some(1), Some(2)]);
    }

    #[tokio::test]
    async fn move_coding_agent_with_saver_rejection_never_calls_saver() {
        let state = state_for(settings_with_agents(&["a", "b", "c"]));

        // `c` sits below `b`, so it is not `b`'s up-neighbor in the latest order.
        let err = move_coding_agent_inner_with_saver(
            &state,
            "b",
            "c",
            AgentMoveDirection::Up,
            |_| -> Result<AppSettings, String> { panic!("saver must not run on a rejected move") },
        )
        .await
        .unwrap_err();

        assert!(err.contains("not adjacent"), "{err}");
        let live = state.read().await;
        assert_eq!(order_ids(&live.agents), ["a", "b", "c"]);
    }

    #[tokio::test]
    async fn move_coding_agent_with_saver_write_failure_leaves_live_state_unchanged() {
        let state = state_for(settings_with_agents(&["a", "b", "c"]));

        let err =
            move_coding_agent_inner_with_saver(&state, "b", "a", AgentMoveDirection::Up, |_| {
                Err("simulated move save failure".to_string())
            })
            .await
            .unwrap_err();

        assert_eq!(err, "simulated move save failure");
        let live = state.read().await;
        assert_eq!(order_ids(&live.agents), ["a", "b", "c"]);
        assert_eq!(order_positions(&live.agents), [Some(0), Some(1), Some(2)]);
    }

    #[tokio::test]
    async fn move_coding_agent_checks_adjacency_against_the_latest_order() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("settings.json");
        let state = state_for(settings_with_agents(&["a", "b", "c"]));

        move_coding_agent_inner_with_saver(&state, "c", "b", AgentMoveDirection::Up, |candidate| {
            crate::config::settings::save_settings_to_path_preserving_project_paths(
                candidate, &path,
            )
        })
        .await
        .unwrap();

        // Latest order is a, c, b: the stale neighbor is rejected without a save.
        let err = move_coding_agent_inner_with_saver(
            &state,
            "c",
            "b",
            AgentMoveDirection::Up,
            |_| -> Result<AppSettings, String> {
                panic!("saver must not run on a stale-neighbor move")
            },
        )
        .await
        .unwrap_err();

        assert!(err.contains("not adjacent"), "{err}");
        let live = state.read().await;
        assert_eq!(order_ids(&live.agents), ["a", "c", "b"]);
    }

    #[tokio::test]
    async fn move_coding_agent_overlay_owned_agents_rejects_before_saver_without_event() {
        use tauri::Listener;

        let temp = tempfile::tempdir().unwrap();
        let path = seed_overlay_agents_fixture(
            temp.path(),
            &json!([
                { "id": "ov-a", "label": "A", "command": "claude", "color": "#111111", "order": 1 },
                { "id": "ov-b", "label": "B", "command": "claude", "color": "#222222" }
            ]),
        );
        let settings = crate::config::settings::load_settings_from_path(&path);
        assert!(settings
            .local_overlay_state
            .owns_top_level(crate::config::settings::OVERLAY_KEY_AGENTS));
        let overlay_order = order_ids(&settings.agents);
        let state = state_for(settings);

        let app = tauri::test::mock_builder()
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("build overlay rejection app");
        let (tx, rx) = std::sync::mpsc::channel();
        app.listen_any("coding_agent_settings_updated", move |event| {
            let _ = tx.send(event.payload().to_string());
        });

        let err = move_coding_agent_command_with_saver(
            app.handle(),
            &state,
            "ov-a".to_string(),
            "ov-b".to_string(),
            AgentMoveDirection::Up,
            |_| -> Result<AppSettings, String> {
                panic!("saver must not run while the overlay owns `agents`")
            },
        )
        .await
        .unwrap_err();

        assert_eq!(err, AGENT_ORDER_OVERLAY_PINNED);
        let live = state.read().await;
        assert_eq!(order_ids(&live.agents), overlay_order);
        assert!(
            rx.recv_timeout(Duration::from_millis(200)).is_err(),
            "a rejected move must not emit an event"
        );
    }

    #[test]
    fn settings_snapshot_exposes_overlay_owns_agents_false_and_not_in_base_settings() {
        let settings = settings_with_agents(&["a", "b"]);

        let snapshot = settings_snapshot_from(&settings, None);
        assert!(!snapshot.overlay_owns_agents);
        let wire = serde_json::to_value(&snapshot).unwrap();
        assert_eq!(wire["overlayOwnsAgents"], json!(false));
        // Read-only disclosure: the base settings object never carries the key.
        let base = serde_json::to_value(&settings).unwrap();
        assert!(base.get("overlayOwnsAgents").is_none());
    }

    #[test]
    fn settings_snapshot_reports_overlay_owned_agents_with_overlay_precedence() {
        let temp = tempfile::tempdir().unwrap();
        let path = seed_overlay_agents_fixture(
            temp.path(),
            &json!([
                { "id": "ov-a", "label": "A", "command": "claude", "color": "#111111", "order": 2 },
                { "id": "ov-b", "label": "B", "command": "claude", "color": "#222222" },
                { "id": "ov-c", "label": "C", "command": "claude", "color": "#333333", "order": -1 },
                { "id": "ov-d", "label": "D", "command": "claude", "color": "#444444", "order": "two" },
                { "id": "ov-e", "label": "E", "command": "claude", "color": "#555555", "order": 1 }
            ]),
        );
        let settings = crate::config::settings::load_settings_from_path(&path);
        // Capture AFTER the loader: the first load of this minimal fixture may
        // run its one-time repair write. Snapshot building must add no write.
        let base_before = std::fs::read(&path).unwrap();
        let local_path = temp.path().join("settings.local.json");
        let local_before = std::fs::read(&local_path).unwrap();
        assert!(settings
            .local_overlay_state
            .owns_top_level(crate::config::settings::OVERLAY_KEY_AGENTS));

        let snapshot = settings_snapshot_from(&settings, None);
        assert!(snapshot.overlay_owns_agents);
        let wire = serde_json::to_value(&snapshot).unwrap();
        assert_eq!(wire["overlayOwnsAgents"], json!(true));
        // P1 effective order: valid ordinals first (1 -> ov-e, 2 -> ov-a), then
        // absent/invalid records by their original overlay index.
        let wire_ids: Vec<&str> = wire["agents"]
            .as_array()
            .unwrap()
            .iter()
            .map(|agent| agent["id"].as_str().unwrap())
            .collect();
        assert_eq!(wire_ids, ["ov-b", "ov-e", "ov-a", "ov-c", "ov-d"]);
        // Building the snapshot must not touch either file.
        assert_eq!(std::fs::read(&path).unwrap(), base_before);
        assert_eq!(std::fs::read(&local_path).unwrap(), local_before);
    }

    #[tokio::test]
    async fn settings_update_does_not_clobber_in_memory_project_lists() {
        let temp = tempfile::tempdir().unwrap();
        let settings_path = temp.path().join("settings.json");
        let disk_project = real_ac_project(temp.path(), "disk-project-a");
        write_settings_file(
            temp.path(),
            &AppSettings {
                project_paths: vec![disk_project.clone()],
                project_path: Some(disk_project.clone()),
                ..AppSettings::default()
            },
        );

        let mut current = settings_with_single_agent();
        current.project_paths = vec!["C:/stale/current".to_string()];
        current.project_path = Some("C:/stale/current".to_string());
        current.root_token = Some("current-root-token".to_string());

        let protected_state = state_for(current.clone());
        let mut stale = current.clone();
        stale.project_paths.clear();
        stale.project_path = None;
        stale.root_token = Some("frontend-root-token".to_string());

        let saved =
            persist_protected_settings_update_with_saver(&protected_state, stale, |candidate| {
                crate::config::settings::save_settings_to_path_preserving_project_paths(
                    candidate,
                    &settings_path,
                )
            })
            .await
            .unwrap();

        assert_single_project(&saved, &disk_project);
        assert_eq!(saved.root_token.as_deref(), Some("current-root-token"));
        {
            let live = protected_state.read().await;
            assert_single_project(&live, &disk_project);
            assert_eq!(live.root_token.as_deref(), Some("current-root-token"));
        }

        let draft_state = state_for(current.clone());
        let mut draft = current;
        draft.project_paths.clear();
        draft.project_path = None;

        let (saved, _events) =
            persist_settings_draft_update_with_saver(&draft_state, draft, |candidate| {
                crate::config::settings::save_settings_to_path_preserving_project_paths(
                    candidate,
                    &settings_path,
                )
            })
            .await
            .unwrap();

        assert_single_project(&saved, &disk_project);
        {
            let live = draft_state.read().await;
            assert_single_project(&live, &disk_project);
        }
    }

    #[tokio::test]
    async fn update_settings_keeps_live_archived_list_when_disk_key_absent() {
        let temp = tempfile::tempdir().unwrap();
        let settings_path = temp.path().join("settings.json");
        let live_project = real_ac_project(temp.path(), "live-a");
        let archived_project = real_ac_project(temp.path(), "archived-b");
        write_settings_file_without_keys(
            temp.path(),
            &AppSettings {
                project_paths: vec![live_project.clone()],
                project_path: Some(live_project.clone()),
                ..AppSettings::default()
            },
            &["archivedProjectPaths"],
        );

        let mut current = settings_with_single_agent();
        current.project_paths = vec![live_project.to_string()];
        current.project_path = Some(live_project.to_string());
        current.archived_project_paths = vec![archived_project.to_string()];
        let state = state_for(current.clone());
        let payload = settings_payload_without_keys(&current, &["archivedProjectPaths"]);

        let saved = persist_protected_settings_update_with_saver(&state, payload, |candidate| {
            crate::config::settings::save_settings_to_path_preserving_project_paths(
                candidate,
                &settings_path,
            )
        })
        .await
        .unwrap();

        assert_eq!(
            saved.archived_project_paths,
            vec![archived_project.to_string()]
        );
        assert!(session_retention_project_paths(&saved).contains(&archived_project.to_string()));
        {
            let live = state.read().await;
            assert_eq!(
                live.archived_project_paths,
                vec![archived_project.to_string()]
            );
        }
        let disk: AppSettings =
            serde_json::from_str(&std::fs::read_to_string(settings_path).unwrap()).unwrap();
        assert_eq!(
            disk.archived_project_paths,
            vec![archived_project.to_string()]
        );
    }

    #[tokio::test]
    async fn save_settings_draft_keeps_live_archived_list_when_disk_key_absent() {
        let temp = tempfile::tempdir().unwrap();
        let settings_path = temp.path().join("settings.json");
        let live_project = real_ac_project(temp.path(), "live-a");
        let archived_project = real_ac_project(temp.path(), "archived-b");
        write_settings_file_without_keys(
            temp.path(),
            &AppSettings {
                project_paths: vec![live_project.clone()],
                project_path: Some(live_project.clone()),
                ..AppSettings::default()
            },
            &["archivedProjectPaths"],
        );

        let mut current = settings_with_single_agent();
        current.project_paths = vec![live_project.to_string()];
        current.project_path = Some(live_project.to_string());
        current.archived_project_paths = vec![archived_project.to_string()];
        let state = state_for(current.clone());
        let payload = settings_payload_without_keys(&current, &["archivedProjectPaths"]);

        let (saved, _events) =
            persist_settings_draft_update_with_saver(&state, payload, |candidate| {
                crate::config::settings::save_settings_to_path_preserving_project_paths(
                    candidate,
                    &settings_path,
                )
            })
            .await
            .unwrap();

        assert_eq!(
            saved.archived_project_paths,
            vec![archived_project.to_string()]
        );
        assert!(session_retention_project_paths(&saved).contains(&archived_project.to_string()));
        {
            let live = state.read().await;
            assert_eq!(
                live.archived_project_paths,
                vec![archived_project.to_string()]
            );
        }
        let disk: AppSettings =
            serde_json::from_str(&std::fs::read_to_string(settings_path).unwrap()).unwrap();
        assert_eq!(
            disk.archived_project_paths,
            vec![archived_project.to_string()]
        );
    }

    #[tokio::test]
    async fn update_settings_keeps_live_project_paths_when_settings_file_absent() {
        let temp = tempfile::tempdir().unwrap();
        let settings_path = temp.path().join("settings.json");
        let live_project = real_ac_project(temp.path(), "live-a");

        let mut current = settings_with_single_agent();
        current.project_paths = vec![live_project.clone()];
        current.project_path = Some(live_project.clone());
        let state = state_for(current.clone());
        let payload = settings_payload_without_keys(&current, &["projectPaths", "projectPath"]);

        let saved = persist_protected_settings_update_with_saver(&state, payload, |candidate| {
            crate::config::settings::save_settings_to_path_preserving_project_paths(
                candidate,
                &settings_path,
            )
        })
        .await
        .unwrap();

        assert_single_project(&saved, &live_project);
        assert!(session_retention_project_paths(&saved).contains(&live_project));
        {
            let live = state.read().await;
            assert_single_project(&live, &live_project);
        }
        let disk: AppSettings =
            serde_json::from_str(&std::fs::read_to_string(settings_path).unwrap()).unwrap();
        assert_single_project(&disk, &live_project);
    }

    #[tokio::test]
    async fn save_settings_draft_keeps_live_project_paths_when_settings_file_absent() {
        let temp = tempfile::tempdir().unwrap();
        let settings_path = temp.path().join("settings.json");
        let live_project = real_ac_project(temp.path(), "live-a");

        let mut current = settings_with_single_agent();
        current.project_paths = vec![live_project.clone()];
        current.project_path = Some(live_project.clone());
        let state = state_for(current.clone());
        let payload = settings_payload_without_keys(&current, &["projectPaths", "projectPath"]);

        let (saved, _events) =
            persist_settings_draft_update_with_saver(&state, payload, |candidate| {
                crate::config::settings::save_settings_to_path_preserving_project_paths(
                    candidate,
                    &settings_path,
                )
            })
            .await
            .unwrap();

        assert_single_project(&saved, &live_project);
        assert!(session_retention_project_paths(&saved).contains(&live_project));
        {
            let live = state.read().await;
            assert_single_project(&live, &live_project);
        }
        let disk: AppSettings =
            serde_json::from_str(&std::fs::read_to_string(settings_path).unwrap()).unwrap();
        assert_single_project(&disk, &live_project);
    }

    #[tokio::test]
    async fn update_settings_still_takes_disk_archived_list_when_key_present() {
        let temp = tempfile::tempdir().unwrap();
        let settings_path = temp.path().join("settings.json");
        let live_project = real_ac_project(temp.path(), "live-a");
        let disk_archived = real_ac_project(temp.path(), "archived-a");
        let live_archived = "C:/archived/b"; // distractor: overridden by disk
        let payload_archived = "C:/archived/c"; // distractor: overridden by disk
        write_settings_file(
            temp.path(),
            &AppSettings {
                project_paths: vec![live_project.clone()],
                project_path: Some(live_project.clone()),
                archived_project_paths: vec![disk_archived.clone()],
                ..AppSettings::default()
            },
        );

        let mut current = settings_with_single_agent();
        current.project_paths = vec![live_project.to_string()];
        current.project_path = Some(live_project.to_string());
        current.archived_project_paths = vec![live_archived.to_string()];
        let state = state_for(current.clone());
        let mut payload = settings_payload_without_keys(&current, &[]);
        payload.archived_project_paths = vec![payload_archived.to_string()];

        let saved = persist_protected_settings_update_with_saver(&state, payload, |candidate| {
            crate::config::settings::save_settings_to_path_preserving_project_paths(
                candidate,
                &settings_path,
            )
        })
        .await
        .unwrap();

        assert_eq!(
            saved.archived_project_paths,
            vec![disk_archived.to_string()]
        );
        {
            let live = state.read().await;
            assert_eq!(live.archived_project_paths, vec![disk_archived.to_string()]);
        }
        let disk: AppSettings =
            serde_json::from_str(&std::fs::read_to_string(settings_path).unwrap()).unwrap();
        assert_eq!(disk.archived_project_paths, vec![disk_archived.to_string()]);
    }

    /// Build a pending (companion-population-eligible) SettingsState by decoding a
    /// legacy `[project]` file (no companion) from `settings_path`.
    fn pending_state_from_legacy(settings_path: &Path, project: &str) -> SettingsState {
        write_settings_file(
            settings_path.parent().unwrap(),
            &AppSettings {
                project_paths: vec![project.to_string()],
                project_path: Some(project.to_string()),
                ..AppSettings::default()
            },
        );
        let value: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(settings_path).unwrap()).unwrap();
        let decoded = crate::config::settings::decode_project_state(
            value.as_object().unwrap(),
            None,
            &crate::config::projects::FsCandidateResolver,
        );
        let startup = AppSettings {
            project_paths: decoded.active_selected(),
            project_path: decoded.selected_head.clone(),
            project_path_state: std::sync::Arc::new(decoded),
            ..AppSettings::default()
        };
        assert!(
            startup.project_path_state.active_reconcile_eligible,
            "legacy no-companion state should be pending"
        );
        state_for(startup)
    }

    #[tokio::test]
    async fn auto_reconcile_redecodes_and_preserves_intervening_cli_write() {
        // Grinch Defect 2: the get_settings boundary must re-decode disk before
        // reconciling, so a CLI registration that landed after startup survives.
        let temp = tempfile::tempdir().unwrap();
        let settings_path = temp.path().join("settings.json");
        let a = real_ac_project(temp.path(), "A");
        let b = real_ac_project(temp.path(), "B");
        let state = pending_state_from_legacy(&settings_path, &a);

        // Intervening CLI registration: disk now carries [A, B].
        write_settings_file(
            temp.path(),
            &AppSettings {
                project_paths: vec![a.clone(), b.clone()],
                project_path: Some(a.clone()),
                ..AppSettings::default()
            },
        );

        let _snap = super::settings_snapshot_helper(&state, Some(settings_path.clone())).await;

        let disk: AppSettings =
            serde_json::from_str(&std::fs::read_to_string(&settings_path).unwrap()).unwrap();
        assert_eq!(
            disk.project_paths,
            vec![a.clone(), b.clone()],
            "CLI-registered B must not be clobbered by the auto-reconcile"
        );
        let live = state.read().await;
        assert_eq!(live.project_paths, vec![a, b]);
    }

    #[tokio::test]
    async fn auto_reconcile_reports_stage_read_on_corrupt_disk() {
        // Grinch Defect 2: a disk re-decode failure at the boundary reports
        // stage `read`, performs no write, and retains the prior state.
        let temp = tempfile::tempdir().unwrap();
        let settings_path = temp.path().join("settings.json");
        let a = real_ac_project(temp.path(), "A");
        let state = pending_state_from_legacy(&settings_path, &a);

        // Corrupt the settings file after startup.
        std::fs::write(&settings_path, b"{ not valid json").unwrap();
        let snap = super::settings_snapshot_helper(&state, Some(settings_path.clone())).await;

        let err = snap
            .project_path_resolution
            .reconciliation_error
            .expect("a corrupt disk re-decode must surface a reconciliation error");
        assert!(matches!(err.stage, super::ReconciliationStage::Read));
        assert!(err.retryable);
        // No write: the corrupt bytes are untouched.
        assert_eq!(
            std::fs::read_to_string(&settings_path).unwrap(),
            "{ not valid json"
        );
    }

    #[tokio::test]
    async fn settings_update_returns_written_project_lists_for_session_purge() {
        let temp = tempfile::tempdir().unwrap();
        let settings_path = temp.path().join("settings.json");
        let project = temp.path().join("project-a");
        let kept_workdir = project.join(".ac").join("wg-1").join("__agent_dev");
        let outside = temp.path().join("project-b");
        let outside_workdir = outside.join(".ac").join("wg-1").join("__agent_dev");
        std::fs::create_dir_all(&kept_workdir).expect("create kept workdir");
        std::fs::create_dir_all(&outside_workdir).expect("create outside workdir");

        let project_path = canonical_display(&project);
        write_settings_file(
            temp.path(),
            &AppSettings {
                project_paths: vec![project_path.clone()],
                project_path: Some(project_path.clone()),
                ..AppSettings::default()
            },
        );
        write_sessions_file(
            temp.path(),
            &[
                PersistedSession {
                    name: "kept".to_string(),
                    shell: "codex".to_string(),
                    working_directory: kept_workdir.to_string_lossy().to_string(),
                    ..Default::default()
                },
                PersistedSession {
                    name: "outside".to_string(),
                    shell: "codex".to_string(),
                    working_directory: outside_workdir.to_string_lossy().to_string(),
                    ..Default::default()
                },
            ],
        );

        let mut current = settings_with_single_agent();
        current.project_paths.clear();
        current.project_path = None;
        let state = state_for(current.clone());
        let stale = current;

        let saved = persist_protected_settings_update_with_saver(&state, stale, |candidate| {
            crate::config::settings::save_settings_to_path_preserving_project_paths(
                candidate,
                &settings_path,
            )
        })
        .await
        .unwrap();

        purge_sessions_after_settings_update_in_dir(&saved, temp.path())
            .await
            .unwrap();

        let remaining = read_sessions_file(temp.path());
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].name, "kept");
        assert_eq!(
            remaining[0].working_directory,
            kept_workdir.to_string_lossy()
        );
        assert_single_project(&saved, &project_path);
    }

    #[tokio::test]
    async fn settings_update_missing_settings_file_preserves_project_sessions() {
        let temp = tempfile::tempdir().unwrap();
        let settings_path = temp.path().join("settings.json");
        assert!(!settings_path.exists());

        let project = temp.path().join("project-a");
        let kept_workdir = project.join(".ac").join("wg-1").join("__agent_dev");
        std::fs::create_dir_all(&kept_workdir).expect("create kept workdir");
        let project_path = canonical_display(&project);

        write_sessions_file(
            temp.path(),
            &[PersistedSession {
                name: "kept".to_string(),
                shell: "codex".to_string(),
                working_directory: kept_workdir.to_string_lossy().to_string(),
                ..Default::default()
            }],
        );

        let mut current = settings_with_single_agent();
        current.project_paths = vec![project_path.clone()];
        current.project_path = Some(project_path.clone());
        let state = state_for(current.clone());

        let mut incoming = current.clone();
        incoming.sidebar_style = "deep-space".to_string();
        let saved = persist_protected_settings_update_with_saver(&state, incoming, |candidate| {
            crate::config::settings::save_settings_to_path_preserving_project_paths(
                candidate,
                &settings_path,
            )
        })
        .await
        .unwrap();

        assert_single_project(&saved, &project_path);
        {
            let live = state.read().await;
            assert_single_project(&live, &project_path);
        }
        assert!(settings_path.exists(), "writer must create settings.json");
        let disk: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&settings_path).unwrap()).unwrap();
        assert_eq!(
            disk["projectPaths"],
            serde_json::json!([project_path.clone()])
        );
        assert_eq!(disk["projectPath"], serde_json::json!(project_path));

        purge_sessions_after_settings_update_in_dir(&saved, temp.path())
            .await
            .unwrap();

        let remaining = read_sessions_file(temp.path());
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].name, "kept");
        assert_eq!(
            remaining[0].working_directory,
            kept_workdir.to_string_lossy()
        );
    }

    #[tokio::test]
    async fn set_rail_collapse_persists_both_fields_and_publishes_to_live_state() {
        let mut original = settings_with_single_agent();
        original.rail_collapsed_projects = vec!["c:/stale".to_string()];
        original.rail_favorites_collapsed = false;
        let state = state_for(original);

        // Drives the REAL `set_rail_collapse_inner` mutation through its saver seam, so
        // dropping or swapping a field assignment inside that fn fails this test. The fake
        // saver keeps it off disk: the real `save_settings` resolves `config::config_dir()`,
        // a `OnceLock` no test redirects, which would overwrite the developer's settings.json.
        let captured: Mutex<Option<AppSettings>> = Mutex::new(None);
        set_rail_collapse_inner_with_saver(
            &state,
            vec!["c:/foo".to_string(), "d:/bar".to_string()],
            true,
            |candidate| {
                *captured.lock().expect("capture lock") = Some(candidate.clone());
                Ok(candidate.clone())
            },
        )
        .await
        .expect("persist rail collapse");

        let written = captured
            .lock()
            .expect("capture lock")
            .clone()
            .expect("saver ran");
        assert_eq!(
            written.rail_collapsed_projects,
            vec!["c:/foo".to_string(), "d:/bar".to_string()]
        );
        assert!(written.rail_favorites_collapsed);

        let live = state.read().await;
        assert_eq!(
            live.rail_collapsed_projects,
            vec!["c:/foo".to_string(), "d:/bar".to_string()]
        );
        assert!(live.rail_favorites_collapsed);
    }

    // (#965 F1) The two whole-object writers must not let a stale client payload
    // revert the rail snapshot. Without these, `window-geometry.ts` alone (which
    // fires a whole-object write on every window move/resize) would silently wipe
    // a header click, and nothing re-persists it: the in-memory signal is never
    // invalidated, so it does NOT self-heal.
    #[tokio::test]
    async fn update_settings_keeps_live_rail_collapse_when_payload_is_stale() {
        let mut current = settings_with_single_agent();
        current.rail_collapsed_projects = vec!["c:/foo".to_string()];
        current.rail_favorites_collapsed = true;
        let state = state_for(current);

        let mut stale = settings_with_single_agent();
        stale.rail_collapsed_projects = Vec::new();
        stale.rail_favorites_collapsed = false;

        let saved = persist_protected_settings_update_with_saver(&state, stale, |c| Ok(c.clone()))
            .await
            .expect("persist");

        assert_eq!(saved.rail_collapsed_projects, vec!["c:/foo".to_string()]);
        assert!(saved.rail_favorites_collapsed);
        let live = state.read().await;
        assert_eq!(live.rail_collapsed_projects, vec!["c:/foo".to_string()]);
        assert!(live.rail_favorites_collapsed);
    }

    #[tokio::test]
    async fn save_settings_draft_keeps_live_rail_collapse_when_draft_is_stale() {
        let mut current = settings_with_single_agent();
        current.rail_collapsed_projects = vec!["c:/foo".to_string()];
        current.rail_favorites_collapsed = true;
        let state = state_for(current);

        let mut draft = settings_with_single_agent();
        draft.rail_collapsed_projects = Vec::new();
        draft.rail_favorites_collapsed = false;

        let (saved, _events) =
            persist_settings_draft_update_with_saver(&state, draft, |c| Ok(c.clone()))
                .await
                .expect("persist");

        assert_eq!(saved.rail_collapsed_projects, vec!["c:/foo".to_string()]);
        assert!(saved.rail_favorites_collapsed);
        let live = state.read().await;
        assert_eq!(live.rail_collapsed_projects, vec!["c:/foo".to_string()]);
        assert!(live.rail_favorites_collapsed);
    }

    #[tokio::test]
    async fn protected_update_settings_transaction_preserves_current_coding_fields() {
        let mut current = settings_with_single_agent();
        current.agents[0].envs = vec![CodingAgentEnv {
            key: "OPENAI_API_BASE".to_string(),
            value: "current".to_string(),
            source: CodingAgentEnvSource::User,
            enabled: true,
        }];
        current.agents[0].isolated_home = true;
        current
            .coding_agent_profiles
            .profiles_by_agent
            .entry("agent-0".to_string())
            .or_default()
            .insert(
                "B".to_string(),
                ProfileCellConfig {
                    enabled: true,
                    command: "codex --current-profile".to_string(),
                    env: BTreeMap::new(),
                    notes: String::new(),
                },
            );
        let state = state_for(current.clone());

        let mut stale = current.clone();
        stale.sidebar_style = "command-center".to_string();
        stale.agents[0].envs.clear();
        stale.agents[0].isolated_home = false;
        stale.coding_agent_profiles.profiles_by_agent.clear();

        let saved = persist_protected_settings_update_with_saver(&state, stale, |candidate| {
            Ok(candidate.clone())
        })
        .await
        .unwrap();

        assert_eq!(saved.sidebar_style, "command-center");
        assert_eq!(saved.agents[0].envs, current.agents[0].envs);
        assert_eq!(
            saved.agents[0].isolated_home,
            current.agents[0].isolated_home
        );
        assert!(saved
            .coding_agent_profiles
            .profiles_by_agent
            .get("agent-0")
            .is_some_and(|cells| cells.contains_key("B")));
        let live = state.read().await;
        assert_eq!(live.sidebar_style, "command-center");
        assert_eq!(live.agents[0].envs, current.agents[0].envs);
        assert_eq!(
            live.agents[0].isolated_home,
            current.agents[0].isolated_home
        );
        assert!(live
            .coding_agent_profiles
            .profiles_by_agent
            .get("agent-0")
            .is_some_and(|cells| cells.contains_key("B")));
    }

    // ─────────────────────────────────────────────────────────────────────────
    // #1737 D14 - the two payload-constructed `AppSettings` carry-overs, and D13's
    // narrowing on the three other production writers of `codingAgentProfiles`.
    // Plan section 11, P1 to P7.
    // ─────────────────────────────────────────────────────────────────────────
    mod local_overlay_1737 {
        use super::{
            persist_protected_settings_update_with_saver, persist_settings_draft_update_with_saver,
            state_for,
        };
        use crate::config::settings::{
            load_settings_from_path, save_settings_to_path_preserving_project_paths,
            validate_and_repair_settings, AppSettings,
        };
        use serde_json::{json, Value};
        use std::path::{Path, PathBuf};

        fn agent_json(id: &str) -> Value {
            json!({
                "id": id,
                "label": id,
                "command": id,
                "color": "#000000",
                "blockingMenus": [],
            })
        }

        /// A base `settings.json` with two agents and a watcher whose
        /// `dedupeWindowMs` the overlay overrides, plus the local file. Returns the
        /// settings path.
        fn seed(dir: &Path, local: &Value) -> PathBuf {
            let base = json!({
                "defaultShell": "test-shell",
                "defaultShellArgs": [],
                "rootToken": "base-token",
                "agents": [agent_json("codex"), agent_json("claude")],
                "watchers": {
                    "a": { "mode": "state", "pattern": "^ready", "dedupeWindowMs": 500 }
                },
                "codingAgentProfiles": {
                    "schemaVersion": 2,
                    "profileSlots": { "A": { "label": "" } },
                    "profilesByAgent": {
                        "codex": { "A": { "enabled": true, "command": "codex", "env": {}, "notes": "" } },
                        "claude": { "A": { "enabled": true, "command": "claude", "env": {}, "notes": "" } }
                    },
                    "defaultProfileByAgent": {},
                    "profileLabelsByAgent": {}
                }
            });
            let path = dir.join("settings.json");
            std::fs::write(&path, serde_json::to_string_pretty(&base).unwrap()).unwrap();
            std::fs::write(
                dir.join("settings.local.json"),
                serde_json::to_string_pretty(local).unwrap(),
            )
            .unwrap();
            path
        }

        fn disk(path: &Path) -> Value {
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
        }

        fn dedupe_window_on_disk(path: &Path) -> Value {
            disk(path)["watchers"]["a"]["dedupeWindowMs"].clone()
        }

        /// The exact renderer round trip: serialize and deserialize, which is what
        /// drops every `#[serde(skip)]` field.
        fn renderer_round_trip(settings: &AppSettings) -> AppSettings {
            serde_json::from_value(serde_json::to_value(settings).unwrap()).unwrap()
        }

        // P4, asserted first because P1 to P3 depend on the behaviour it pins.
        #[test]
        fn the_renderer_round_trip_drops_the_overlay_state() {
            let temp = tempfile::tempdir().unwrap();
            let path = seed(
                temp.path(),
                &json!({"watchers": {"a": {"dedupeWindowMs": 50}}}),
            );
            let loaded = load_settings_from_path(&path);
            assert!(
                !loaded.local_overlay_state.is_empty(),
                "the fixture must carry an overlay"
            );
            let payload = renderer_round_trip(&loaded);
            assert!(
                payload.local_overlay_state.is_empty(),
                "this is the serde behaviour the D14 carry-overs compensate for"
            );
        }

        // P1 and P3
        #[tokio::test]
        async fn the_protected_writer_inherits_the_live_overlay_state() {
            let temp = tempfile::tempdir().unwrap();
            let path = seed(
                temp.path(),
                &json!({"watchers": {"a": {"dedupeWindowMs": 50}}}),
            );
            let loaded = load_settings_from_path(&path);
            let state = state_for(loaded.clone());
            let payload = renderer_round_trip(&loaded);

            let saved = {
                let path = path.clone();
                persist_protected_settings_update_with_saver(&state, payload, move |candidate| {
                    save_settings_to_path_preserving_project_paths(candidate, &path)
                })
                .await
                .unwrap()
            };

            // P1: the overlay-owned leaf on disk still holds the base value.
            assert_eq!(dedupe_window_on_disk(&path), json!(500));
            assert!(!saved.local_overlay_state.is_empty());

            // P3: the value the state now holds is still protected, so a second
            // round trip through the same path is protected too.
            {
                let live = state.read().await;
                assert!(!live.local_overlay_state.is_empty());
            }
            let second_payload = {
                let live = state.read().await;
                renderer_round_trip(&live)
            };
            let path2 = path.clone();
            persist_protected_settings_update_with_saver(
                &state,
                second_payload,
                move |candidate| save_settings_to_path_preserving_project_paths(candidate, &path2),
            )
            .await
            .unwrap();
            assert_eq!(dedupe_window_on_disk(&path), json!(500));
        }

        // P2
        #[tokio::test]
        async fn the_draft_writer_inherits_the_live_overlay_state() {
            let temp = tempfile::tempdir().unwrap();
            let path = seed(
                temp.path(),
                &json!({"watchers": {"a": {"dedupeWindowMs": 50}}}),
            );
            let loaded = load_settings_from_path(&path);
            let state = state_for(loaded.clone());
            let payload = renderer_round_trip(&loaded);

            let (saved, _events) = {
                let path = path.clone();
                persist_settings_draft_update_with_saver(&state, payload, move |candidate| {
                    save_settings_to_path_preserving_project_paths(candidate, &path)
                })
                .await
                .unwrap()
            };

            assert_eq!(dedupe_window_on_disk(&path), json!(500));
            assert!(!saved.local_overlay_state.is_empty());
        }

        /// The `agents` overlay that introduces one scratch agent, which is what
        /// makes D13's closure own `codingAgentProfiles.profilesByAgent.scratch-agent`.
        fn scratch_agent_overlay() -> Value {
            json!({
                "agents": [
                    agent_json("codex"),
                    agent_json("claude"),
                    agent_json("scratch-agent"),
                ]
            })
        }

        // P5
        #[tokio::test]
        async fn a_profile_edit_persists_except_for_the_overlay_introduced_agent() {
            let temp = tempfile::tempdir().unwrap();
            let path = seed(temp.path(), &scratch_agent_overlay());
            let loaded = load_settings_from_path(&path);
            assert!(loaded
                .coding_agent_profiles
                .profiles_by_agent
                .contains_key("scratch-agent"));
            let state = state_for(loaded.clone());

            let mut draft = renderer_round_trip(&loaded);
            draft
                .coding_agent_profiles
                .profiles_by_agent
                .get_mut("codex")
                .unwrap()
                .get_mut("A")
                .unwrap()
                .command = "codex --edited".to_string();
            draft.coding_agent_profiles.profile_slots.insert(
                "B".to_string(),
                crate::config::settings::ProfileSlotConfig {
                    label: "Second".to_string(),
                },
            );
            draft
                .coding_agent_profiles
                .default_profile_by_agent
                .insert("codex".to_string(), "A".to_string());

            let path2 = path.clone();
            persist_settings_draft_update_with_saver(&state, draft, move |candidate| {
                save_settings_to_path_preserving_project_paths(candidate, &path2)
            })
            .await
            .unwrap();

            let profiles = disk(&path)["codingAgentProfiles"].clone();
            assert_eq!(
                profiles["profilesByAgent"]["codex"]["A"]["command"],
                json!("codex --edited")
            );
            assert_eq!(profiles["profileSlots"]["B"]["label"], json!("Second"));
            assert_eq!(profiles["defaultProfileByAgent"]["codex"], json!("A"));
            assert!(
                profiles["profilesByAgent"].get("scratch-agent").is_none(),
                "only the overlay-introduced agent's cells are frozen: {profiles}"
            );
        }

        // P6
        #[test]
        fn the_dedicated_profiles_command_shape_has_the_same_outcome() {
            let temp = tempfile::tempdir().unwrap();
            let path = seed(temp.path(), &scratch_agent_overlay());
            let loaded = load_settings_from_path(&path);

            // `persist_coding_agent_profiles_update` clones the live guard, assigns
            // the payload and saves; `save_settings` delegates to the path-taking
            // preserving writer for a resolved path. Reproduced here because that
            // helper takes no injectable saver and redirecting `save_settings`
            // would need a process-global env mutation.
            let mut candidate = loaded.clone();
            candidate
                .coding_agent_profiles
                .profiles_by_agent
                .get_mut("codex")
                .unwrap()
                .get_mut("A")
                .unwrap()
                .command = "codex --edited".to_string();
            candidate.coding_agent_profiles.profile_slots.insert(
                "B".to_string(),
                crate::config::settings::ProfileSlotConfig {
                    label: "Second".to_string(),
                },
            );
            candidate
                .coding_agent_profiles
                .default_profile_by_agent
                .insert("codex".to_string(), "A".to_string());
            validate_and_repair_settings(&mut candidate).unwrap();
            save_settings_to_path_preserving_project_paths(&candidate, &path).unwrap();

            let profiles = disk(&path)["codingAgentProfiles"].clone();
            assert_eq!(
                profiles["profilesByAgent"]["codex"]["A"]["command"],
                json!("codex --edited")
            );
            assert_eq!(profiles["profileSlots"]["B"]["label"], json!("Second"));
            assert_eq!(profiles["defaultProfileByAgent"]["codex"], json!("A"));
            assert!(profiles["profilesByAgent"].get("scratch-agent").is_none());
        }

        // P7
        #[test]
        fn an_agent_delete_removes_its_default_profile_entry_from_the_base_file() {
            let temp = tempfile::tempdir().unwrap();
            let path = seed(temp.path(), &scratch_agent_overlay());
            let mut seeded = load_settings_from_path(&path);
            seeded
                .coding_agent_profiles
                .default_profile_by_agent
                .insert("claude".to_string(), "A".to_string());
            save_settings_to_path_preserving_project_paths(&seeded, &path).unwrap();
            assert_eq!(
                disk(&path)["codingAgentProfiles"]["defaultProfileByAgent"]["claude"],
                json!("A")
            );

            // The agent-delete shape: remove the entry from a clone of the live
            // guard and save.
            let loaded = load_settings_from_path(&path);
            let mut candidate = loaded.clone();
            candidate
                .coding_agent_profiles
                .default_profile_by_agent
                .remove("claude");
            save_settings_to_path_preserving_project_paths(&candidate, &path).unwrap();

            assert!(
                disk(&path)["codingAgentProfiles"]["defaultProfileByAgent"]
                    .get("claude")
                    .is_none(),
                "the removal must persist"
            );
            assert!(disk(&path)["codingAgentProfiles"]["profilesByAgent"]
                .get("scratch-agent")
                .is_none());
        }
    }

    // ── #1941 scoped selection-lock API ─────────────────────────────

    struct SelectionApiFixture {
        _temp: tempfile::TempDir,
        project: PathBuf,
        ac_root: PathBuf,
    }

    fn selection_api_fixture() -> SelectionApiFixture {
        let temp = tempfile::tempdir().expect("tempdir");
        let project = temp.path().join("project");
        let ac_root = project.join(".ac");
        std::fs::create_dir_all(&ac_root).expect("create .ac");
        SelectionApiFixture {
            _temp: temp,
            project,
            ac_root,
        }
    }

    fn selection_api_settings(fixture: &SelectionApiFixture) -> AppSettings {
        let mut settings = settings_with_single_agent();
        settings.project_paths = vec![fixture.project.to_string_lossy().to_string()];
        settings
    }

    /// Create a valid `_agent_<name>` Matrix plus a `room-*` replica whose
    /// config.json stores the given tooling block and the correct identity.
    fn selection_api_replica(
        fixture: &SelectionApiFixture,
        room: &str,
        name: &str,
        tooling: serde_json::Value,
    ) -> PathBuf {
        let matrix = fixture.ac_root.join(format!("_agent_{name}"));
        let replica = fixture.ac_root.join(room).join(format!("__agent_{name}"));
        selection_api_replica_at(fixture, &matrix, &replica, name, tooling);
        replica
    }

    fn selection_api_replica_at(
        _fixture: &SelectionApiFixture,
        matrix: &Path,
        replica: &Path,
        name: &str,
        tooling: serde_json::Value,
    ) {
        std::fs::create_dir_all(matrix).expect("create matrix");
        std::fs::create_dir_all(replica).expect("create replica");
        std::fs::write(matrix.join("Role.md"), "# Role\n").expect("write Role.md");
        let config = json!({
            "identity": format!("../../_agent_{name}"),
            "tooling": tooling,
        });
        std::fs::write(
            replica.join("config.json"),
            serde_json::to_vec(&config).expect("serialize config"),
        )
        .expect("write config");
    }

    fn locked_tooling(profile: &str, agent: &str) -> serde_json::Value {
        json!({
            "profile": profile,
            "currentCodingAgent": agent,
            "selectionLocked": true,
        })
    }

    fn unlocked_tooling(profile: &str, agent: &str) -> serde_json::Value {
        json!({
            "profile": profile,
            "currentCodingAgent": agent,
        })
    }

    fn config_bytes(replica: &Path) -> Vec<u8> {
        std::fs::read(replica.join("config.json")).expect("read config.json")
    }

    fn sidecar_path(replica: &Path) -> PathBuf {
        replica.join(".config.json.lock")
    }

    fn empty_session_manager() -> Arc<RwLock<SessionManager>> {
        Arc::new(RwLock::new(SessionManager::new()))
    }

    fn api_preview_request(
        path: &Path,
        scope: super::ProfileAssignmentScope,
        mode: super::AssignmentMode,
    ) -> super::PreviewCodingAgentProfileSelectionRequest {
        super::PreviewCodingAgentProfileSelectionRequest {
            target_replica_path: path.to_string_lossy().to_string(),
            coding_agent_id: "agent-0".to_string(),
            profile: "B".to_string(),
            scope,
            restart_sessions: false,
            assignment_mode: mode,
        }
    }

    fn api_apply_request(
        path: &Path,
        scope: super::ProfileAssignmentScope,
        mode: super::AssignmentMode,
        decision: Option<super::ConflictDecision>,
        fingerprint: Option<&str>,
    ) -> super::ApplyCodingAgentProfileSelectionRequest {
        super::ApplyCodingAgentProfileSelectionRequest {
            target_replica_path: path.to_string_lossy().to_string(),
            coding_agent_id: "agent-0".to_string(),
            profile: "B".to_string(),
            scope,
            restart_sessions: false,
            confirmed_target_fingerprint: fingerprint.map(str::to_string),
            typed_confirmation: None,
            assignment_mode: mode,
            conflict_decision: decision,
        }
    }

    fn api_removal_preview_request(
        path: &Path,
        scope: super::ProfileAssignmentScope,
    ) -> super::PreviewSelectionLockRemovalRequest {
        super::PreviewSelectionLockRemovalRequest {
            target_replica_path: path.to_string_lossy().to_string(),
            scope,
        }
    }

    fn api_removal_apply_request(
        path: &Path,
        scope: super::ProfileAssignmentScope,
        fingerprint: &str,
    ) -> super::ApplySelectionLockRemovalRequest {
        super::ApplySelectionLockRemovalRequest {
            target_replica_path: path.to_string_lossy().to_string(),
            scope,
            confirmed_target_fingerprint: fingerprint.to_string(),
        }
    }

    fn api_default_request(path: &Path) -> super::GetReplicaSelectionDefaultRequest {
        super::GetReplicaSelectionDefaultRequest {
            target_replica_path: path.to_string_lossy().to_string(),
        }
    }

    fn api_set_default_request(
        path: &Path,
        agent: &str,
        profile: &str,
        locked: bool,
        fingerprint: &str,
    ) -> super::SetReplicaSelectionDefaultRequest {
        super::SetReplicaSelectionDefaultRequest {
            target_replica_path: path.to_string_lossy().to_string(),
            coding_agent_id: agent.to_string(),
            requested_profile: profile.to_string(),
            selection_locked: locked,
            confirmed_default_fingerprint: fingerprint.to_string(),
        }
    }

    async fn api_preview(
        settings: &SettingsState,
        path: &Path,
        scope: super::ProfileAssignmentScope,
        mode: super::AssignmentMode,
    ) -> super::PreviewCodingAgentProfileSelectionResult {
        api_preview_result(settings, path, scope, mode)
            .await
            .expect("assignment preview")
    }

    async fn api_preview_result(
        settings: &SettingsState,
        path: &Path,
        scope: super::ProfileAssignmentScope,
        mode: super::AssignmentMode,
    ) -> Result<super::PreviewCodingAgentProfileSelectionResult, String> {
        super::preview_coding_agent_profile_selection_inner(
            &empty_session_manager(),
            settings,
            api_preview_request(path, scope, mode),
        )
        .await
    }

    /// Minimal PTY backend for the non-restarting apply paths: the manager is
    /// required by the command signature but no spawn/kill is ever requested.
    struct SelectionApiPtyBackend;

    impl crate::pty::backend::PtyBackend for SelectionApiPtyBackend {
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }

        fn spawn(
            &self,
            _spec: crate::pty::backend::BackendSpawnSpec,
        ) -> futures::future::BoxFuture<'_, Result<(), crate::errors::AppError>> {
            Box::pin(async { Ok(()) })
        }

        fn write(
            &self,
            _authority: &crate::pty::manager::BackendWriteAuthority,
            _id: uuid::Uuid,
            _data: &[u8],
        ) -> Result<(), crate::errors::AppError> {
            Ok(())
        }

        fn resize(
            &self,
            _id: uuid::Uuid,
            _cols: u16,
            _rows: u16,
        ) -> Result<(), crate::errors::AppError> {
            Ok(())
        }

        fn kill(&self, _id: uuid::Uuid) -> Result<(), crate::errors::AppError> {
            Ok(())
        }

        fn has_session(&self, _id: uuid::Uuid) -> bool {
            false
        }

        fn get_screen_snapshot(
            &self,
            _id: uuid::Uuid,
        ) -> Option<crate::pty::output::PtyScreenSnapshot> {
            None
        }

        fn get_pty_size(&self, _id: uuid::Uuid) -> Option<(u16, u16)> {
            None
        }

        fn get_screen_rows(&self, _id: uuid::Uuid) -> crate::pty::context_scrape::ScreenRowsRead {
            crate::pty::context_scrape::ScreenRowsRead::SessionOver
        }

        fn register_response_watcher(
            &self,
            _session_id: uuid::Uuid,
            _request_id: String,
            _response_dir: PathBuf,
        ) {
        }

        fn terminate_job_for_session(&self, _id: uuid::Uuid) -> bool {
            false
        }

        fn kill_all_jobs(&self) -> (usize, usize) {
            (0, 0)
        }
    }

    async fn api_apply(
        settings: &SettingsState,
        path: &Path,
        scope: super::ProfileAssignmentScope,
        mode: super::AssignmentMode,
        decision: Option<super::ConflictDecision>,
        fingerprint: Option<&str>,
    ) -> Result<super::ApplyCodingAgentProfileSelectionResult, String> {
        let app = crate::test_support::test_builder()
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("build selection api test app");
        let pty_mgr = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new_for_test(
            Arc::new(SelectionApiPtyBackend),
        )));
        super::apply_coding_agent_profile_selection_inner(
            app.handle(),
            &empty_session_manager(),
            &pty_mgr,
            settings,
            api_apply_request(path, scope, mode, decision, fingerprint),
        )
        .await
        .map(|(result, _payload)| result)
    }

    async fn api_default(
        settings: &SettingsState,
        path: &Path,
    ) -> super::ReplicaSelectionDefaultResult {
        super::get_replica_selection_default_inner(settings, api_default_request(path))
            .await
            .expect("default read")
    }

    async fn api_removal_preview(
        settings: &SettingsState,
        path: &Path,
        scope: super::ProfileAssignmentScope,
    ) -> super::PreviewSelectionLockRemovalResult {
        super::preview_selection_lock_removal_inner(
            &empty_session_manager(),
            settings,
            api_removal_preview_request(path, scope),
        )
        .await
        .expect("removal preview")
    }

    async fn api_removal_apply(
        settings: &SettingsState,
        path: &Path,
        scope: super::ProfileAssignmentScope,
        fingerprint: &str,
    ) -> Result<super::ApplySelectionLockRemovalResult, String> {
        super::apply_selection_lock_removal_inner(
            settings,
            api_removal_apply_request(path, scope, fingerprint),
        )
        .await
        .map(|(result, _payload)| result)
    }

    #[tokio::test]
    async fn issue_1937_selection_api_preview_replica_reports_saved_pair_and_state() {
        let fixture = selection_api_fixture();
        let replica = selection_api_replica(
            &fixture,
            "room-1-team",
            "dev-rust",
            locked_tooling("B", "agent-0"),
        );
        let settings = state_for(selection_api_settings(&fixture));
        let preview = api_preview(
            &settings,
            &replica,
            super::ProfileAssignmentScope::Replica,
            super::AssignmentMode::Ordinary,
        )
        .await;

        assert_eq!(preview.scope, super::ProfileAssignmentScope::Replica);
        assert!(preview.counts_complete);
        assert_eq!(preview.candidate_count, 1);
        assert_eq!(preview.protected_count, 1);
        assert_eq!(preview.invalid_count, 0);
        assert_eq!(preview.conflict_count, 0);
        assert!(preview.decisions.is_none());
        assert_eq!(
            preview.target_count, 1,
            "a deliberate replica target counts"
        );
        assert_eq!(preview.targets.len(), 1);
        let target = &preview.targets[0];
        assert_eq!(target.selection_state, super::SelectionState::Locked);
        assert_eq!(
            target.saved_pair,
            Some(super::SavedPair {
                coding_agent_id: Some("agent-0".to_string()),
                requested_profile: Some("B".to_string()),
            })
        );
        assert!(target.selection_error.is_none());
        assert_eq!(preview.target_fingerprint.len(), 64);
        assert_eq!(
            preview.target_fingerprint,
            preview.target_fingerprint.to_lowercase()
        );
    }

    #[tokio::test]
    async fn issue_1937_selection_api_scopes_enumerate_canonical_matrices_only() {
        let fixture = selection_api_fixture();
        let anchor = selection_api_replica(
            &fixture,
            "room-1-team",
            "dev-rust",
            locked_tooling("B", "agent-0"),
        );
        selection_api_replica(
            &fixture,
            "room-2-team",
            "dev-rust",
            unlocked_tooling("A", "agent-0"),
        );
        // Homonymous Matrix in another configured project: same directory name,
        // different canonical origin Matrix, so Kind must exclude it.
        let other_project = fixture._temp.path().join("other-project");
        let other_fixture = SelectionApiFixture {
            _temp: tempfile::tempdir().expect("second tempdir"),
            project: other_project,
            ac_root: PathBuf::new(),
        };
        let other_ac = other_fixture.project.join(".ac");
        selection_api_replica_at(
            &other_fixture,
            &other_ac.join("_agent_dev-rust"),
            &other_ac.join("room-1-team").join("__agent_dev-rust"),
            "dev-rust",
            locked_tooling("C", "agent-0"),
        );
        let mut settings = selection_api_settings(&fixture);
        settings
            .project_paths
            .push(other_fixture.project.to_string_lossy().to_string());
        let settings = state_for(settings);

        let room = api_preview(
            &settings,
            &anchor,
            super::ProfileAssignmentScope::Workgroup,
            super::AssignmentMode::Ordinary,
        )
        .await;
        assert!(room.counts_complete);
        assert_eq!(room.candidate_count, 1, "one Workgroup means one room");

        let kind = api_preview(
            &settings,
            &anchor,
            super::ProfileAssignmentScope::Kind,
            super::AssignmentMode::Ordinary,
        )
        .await;
        assert!(kind.counts_complete);
        assert_eq!(
            kind.candidate_count, 2,
            "the canonical Matrix, not the name"
        );
        assert_eq!(kind.protected_count, 1);
        assert_eq!(kind.invalid_count, 0);
        let paths: Vec<&str> = kind
            .targets
            .iter()
            .map(|target| target.replica_path.as_str())
            .collect();
        assert!(paths.iter().any(|path| path.contains("room-1-team")));
        assert!(paths.iter().any(|path| path.contains("room-2-team")));
        assert!(
            paths.iter().all(|path| path.contains("project")),
            "the homonymous other-project replica must stay excluded: {paths:?}"
        );
    }

    #[tokio::test]
    async fn issue_1937_selection_api_preview_counts_offline_and_live_replicas() {
        let fixture = selection_api_fixture();
        let anchor = selection_api_replica(
            &fixture,
            "room-1-team",
            "a-rust",
            unlocked_tooling("A", "agent-0"),
        );
        let live = selection_api_replica(
            &fixture,
            "room-1-team",
            "b-rust",
            unlocked_tooling("A", "agent-0"),
        );
        let manager = SessionManager::new();
        let session = manager
            .create_session(
                "codex".to_string(),
                Vec::new(),
                live.to_string_lossy().to_string(),
                Some("agent-0".to_string()),
                Some("Codex".to_string()),
                Vec::new(),
                false,
                crate::pty::backend::SessionBackendKind::LocalProcess,
            )
            .await
            .expect("create live session");
        let settings = state_for(selection_api_settings(&fixture));
        let preview = super::preview_coding_agent_profile_selection_inner(
            &Arc::new(RwLock::new(manager)),
            &settings,
            api_preview_request(
                &anchor,
                super::ProfileAssignmentScope::Workgroup,
                super::AssignmentMode::Ordinary,
            ),
        )
        .await
        .expect("preview");

        assert_eq!(preview.candidate_count, 2, "offline replicas stay listed");
        assert_eq!(preview.live_session_count, 1);
        let live_target = preview
            .targets
            .iter()
            .find(|target| target.replica_path.contains("b-rust"))
            .expect("live target");
        assert_eq!(live_target.live_session_ids, vec![session.id.to_string()]);
        let offline_target = preview
            .targets
            .iter()
            .find(|target| target.replica_path.contains("a-rust"))
            .expect("offline target");
        assert!(offline_target.live_session_ids.is_empty());
    }

    #[tokio::test]
    async fn issue_1937_selection_api_all_protected_bulk_offers_both_decisions() {
        let fixture = selection_api_fixture();
        let anchor = selection_api_replica(
            &fixture,
            "room-1-team",
            "a-rust",
            locked_tooling("B", "agent-0"),
        );
        // Equal pair as well: a valid lock is still a conflict.
        selection_api_replica(
            &fixture,
            "room-1-team",
            "b-rust",
            locked_tooling("B", "agent-0"),
        );
        selection_api_replica(
            &fixture,
            "room-1-team",
            "c-rust",
            locked_tooling("A", "agent-0"),
        );
        let settings = state_for(selection_api_settings(&fixture));
        let preview = api_preview(
            &settings,
            &anchor,
            super::ProfileAssignmentScope::Workgroup,
            super::AssignmentMode::AssignAndLock,
        )
        .await;

        assert_eq!(preview.candidate_count, 3);
        assert_eq!(preview.protected_count, 3);
        assert_eq!(preview.conflict_count, 3, "valid protected candidates only");
        assert_eq!(preview.target_count, 0, "top-level uses unlockedOnly");
        let decisions = preview.decisions.as_ref().expect("both decisions");
        assert_eq!(decisions.unlocked_only.eligible_count, 0);
        assert_eq!(decisions.unlocked_only.skipped_locked_count, 3);
        assert_eq!(decisions.force_reviewed.eligible_count, 3);
        assert_eq!(decisions.force_reviewed.skipped_locked_count, 0);
        assert_ne!(
            decisions.unlocked_only.fingerprint,
            decisions.force_reviewed.fingerprint
        );
        assert_eq!(
            preview.target_fingerprint,
            decisions.unlocked_only.fingerprint
        );
    }

    #[tokio::test]
    async fn issue_1937_selection_api_absent_assignment_mode_is_ordinary_legacy() {
        let fixture = selection_api_fixture();
        let replica = selection_api_replica(
            &fixture,
            "room-1-team",
            "dev-rust",
            unlocked_tooling("A", "agent-0"),
        );
        let request: super::PreviewCodingAgentProfileSelectionRequest =
            serde_json::from_value(json!({
                "targetReplicaPath": replica.to_string_lossy(),
                "codingAgentId": "agent-0",
                "profile": "B",
                "scope": "replica",
                "restartSessions": false,
            }))
            .expect("deserialize legacy preview request");
        assert_eq!(request.assignment_mode, super::AssignmentMode::Ordinary);

        let settings = state_for(selection_api_settings(&fixture));
        let preview = super::preview_coding_agent_profile_selection_inner(
            &empty_session_manager(),
            &settings,
            request,
        )
        .await
        .expect("legacy preview");
        assert!(!preview.requires_explicit_confirmation);
        let result = api_apply(
            &settings,
            &replica,
            super::ProfileAssignmentScope::Replica,
            super::AssignmentMode::Ordinary,
            None,
            None,
        )
        .await
        .expect("legacy replica apply without fingerprint");
        assert_eq!(result.updated_count, 1);
        assert_eq!(result.newly_protected_paths.len(), 0);
    }

    #[tokio::test]
    async fn issue_1937_selection_api_apply_ordinary_bulk_skips_locked() {
        let fixture = selection_api_fixture();
        let anchor = selection_api_replica(
            &fixture,
            "room-1-team",
            "a-rust",
            locked_tooling("A", "agent-0"),
        );
        let unlocked = selection_api_replica(
            &fixture,
            "room-1-team",
            "b-rust",
            unlocked_tooling("A", "agent-0"),
        );
        let locked_bytes = config_bytes(&anchor);
        let settings = state_for(selection_api_settings(&fixture));
        let preview = api_preview(
            &settings,
            &anchor,
            super::ProfileAssignmentScope::Workgroup,
            super::AssignmentMode::Ordinary,
        )
        .await;
        assert_eq!(
            preview.target_count, 1,
            "actionable count skips the locked one"
        );
        assert!(preview.decisions.is_none());

        let result = api_apply(
            &settings,
            &anchor,
            super::ProfileAssignmentScope::Workgroup,
            super::AssignmentMode::Ordinary,
            None,
            Some(&preview.target_fingerprint),
        )
        .await
        .expect("ordinary bulk apply");

        assert_eq!(result.updated_count, 1);
        assert_eq!(result.newly_protected_paths.len(), 0);
        assert_eq!(result.skipped_locked_paths.len(), 1);
        assert!(result.skipped_locked_paths[0].contains("a-rust"));
        assert!(result.errors.is_empty());
        assert_eq!(result.locked_after_apply_count, Some(1));
        assert!(!result.force_applied);
        assert_eq!(
            config_bytes(&anchor),
            locked_bytes,
            "locked bytes untouched"
        );
        let saved: serde_json::Value =
            serde_json::from_slice(&config_bytes(&unlocked)).expect("parse unlocked config");
        assert_eq!(saved["tooling"]["profile"], json!("B"));
        assert_eq!(saved["tooling"]["currentCodingAgent"], json!("agent-0"));
        assert!(saved["tooling"].get("selectionLocked").is_none());
    }

    #[tokio::test]
    async fn issue_1937_selection_api_replica_assign_and_lock_is_atomic() {
        let fixture = selection_api_fixture();
        let replica = selection_api_replica(
            &fixture,
            "room-1-team",
            "dev-rust",
            unlocked_tooling("A", "agent-0"),
        );
        let settings = state_for(selection_api_settings(&fixture));
        let preview = api_preview(
            &settings,
            &replica,
            super::ProfileAssignmentScope::Replica,
            super::AssignmentMode::AssignAndLock,
        )
        .await;
        assert!(preview.requires_explicit_confirmation);
        assert!(
            preview.decisions.is_none(),
            "replica needs no conflict dialog"
        );

        let result = api_apply(
            &settings,
            &replica,
            super::ProfileAssignmentScope::Replica,
            super::AssignmentMode::AssignAndLock,
            None,
            Some(&preview.target_fingerprint),
        )
        .await
        .expect("replica assign and lock");

        assert_eq!(result.updated_count, 1);
        assert_eq!(
            result.newly_protected_paths,
            vec![result.updated_replica_paths[0].clone()]
        );
        assert!(!result.force_applied);
        let saved: serde_json::Value =
            serde_json::from_slice(&config_bytes(&replica)).expect("parse config");
        assert_eq!(saved["tooling"]["selectionLocked"], json!(true));
        assert_eq!(saved["tooling"]["currentCodingAgent"], json!("agent-0"));
        assert_eq!(saved["tooling"]["profile"], json!("B"));
    }

    #[tokio::test]
    async fn issue_2051_selection_api_replica_lock_changed_target_is_stale_without_writes() {
        let fixture = selection_api_fixture();
        let replica = selection_api_replica(
            &fixture,
            "room-1-team",
            "dev-rust",
            unlocked_tooling("A", "agent-0"),
        );
        let settings = state_for(selection_api_settings(&fixture));

        // 1. approve the replica + lock pair on this state.
        let preview = api_preview(
            &settings,
            &replica,
            super::ProfileAssignmentScope::Replica,
            super::AssignmentMode::AssignAndLock,
        )
        .await;

        // 2. the stored pair changes after the preview.
        let mut value: Value =
            serde_json::from_slice(&config_bytes(&replica)).expect("parse config");
        value["tooling"]["profile"] = json!("C");
        std::fs::write(
            replica.join("config.json"),
            serde_json::to_vec(&value).expect("serialize"),
        )
        .expect("rewrite config");
        let changed_bytes = config_bytes(&replica);

        // 3. the stale approval is rejected before any write.
        let error = api_apply(
            &settings,
            &replica,
            super::ProfileAssignmentScope::Replica,
            super::AssignmentMode::AssignAndLock,
            None,
            Some(&preview.target_fingerprint),
        )
        .await
        .expect_err("a changed target must stale");
        assert!(error.contains("stalePreview"), "{error}");
        assert_eq!(config_bytes(&replica), changed_bytes, "no write on stale");
        assert!(!sidecar_path(&replica).exists(), "no sidecar on stale");
        let saved: Value = serde_json::from_slice(&config_bytes(&replica)).expect("parse config");
        assert_ne!(
            saved["tooling"]["selectionLocked"],
            json!(true),
            "no lock on stale"
        );

        // 4. control: a fresh preview over the changed target still applies, so
        // step 3 rejected because of the change, not because lock is broken.
        let fresh = api_preview(
            &settings,
            &replica,
            super::ProfileAssignmentScope::Replica,
            super::AssignmentMode::AssignAndLock,
        )
        .await;
        let result = api_apply(
            &settings,
            &replica,
            super::ProfileAssignmentScope::Replica,
            super::AssignmentMode::AssignAndLock,
            None,
            Some(&fresh.target_fingerprint),
        )
        .await
        .expect("a fresh preview applies");
        assert_eq!(result.updated_count, 1);
        assert_eq!(
            result.newly_protected_paths,
            vec![result.updated_replica_paths[0].clone()]
        );
    }

    #[tokio::test]
    async fn issue_2051_selection_api_replica_lock_missing_fingerprint_is_not_stale() {
        let fixture = selection_api_fixture();
        let replica = selection_api_replica(
            &fixture,
            "room-1-team",
            "dev-rust",
            unlocked_tooling("A", "agent-0"),
        );
        let settings = state_for(selection_api_settings(&fixture));
        let before = config_bytes(&replica);

        let error = api_apply(
            &settings,
            &replica,
            super::ProfileAssignmentScope::Replica,
            super::AssignmentMode::AssignAndLock,
            None,
            None,
        )
        .await
        .expect_err("a lock apply without a fingerprint must be refused");

        // #2051 - nothing was sent is a client contract failure, never a
        // target change: the message must not blame a stale preview.
        assert!(error.contains("confirmationMissing"), "{error}");
        assert!(!error.contains("Target selection changed"), "{error}");
        assert_eq!(
            config_bytes(&replica),
            before,
            "no write on a missing fingerprint"
        );
        assert!(!sidecar_path(&replica).exists(), "no sidecar on rejection");
        let saved: serde_json::Value =
            serde_json::from_slice(&config_bytes(&replica)).expect("parse config");
        assert_ne!(
            saved["tooling"]["selectionLocked"],
            json!(true),
            "no lock written"
        );
    }

    #[tokio::test]
    async fn issue_2010_selection_api_replica_locked_same_pair_keeps_lock() {
        for mode in [
            super::AssignmentMode::Ordinary,
            super::AssignmentMode::AssignAndLock,
        ] {
            let fixture = selection_api_fixture();
            let replica = selection_api_replica(
                &fixture,
                "room-1-team",
                "dev-rust",
                locked_tooling("B", "agent-0"),
            );
            let settings = state_for(selection_api_settings(&fixture));
            let before = config_bytes(&replica);
            let preview = api_preview(
                &settings,
                &replica,
                super::ProfileAssignmentScope::Replica,
                mode,
            )
            .await;

            let result = api_apply(
                &settings,
                &replica,
                super::ProfileAssignmentScope::Replica,
                mode,
                None,
                Some(&preview.target_fingerprint),
            )
            .await
            .expect("locked same-pair replica apply");

            assert!(result.errors.is_empty(), "{mode:?}: {:?}", result.errors);
            assert_eq!(result.updated_count, 0, "{mode:?}");
            assert!(result.updated_replica_paths.is_empty(), "{mode:?}");
            assert!(result.newly_protected_paths.is_empty(), "{mode:?}");
            assert!(result.skipped_locked_paths.is_empty(), "{mode:?}");
            assert_eq!(result.restarted_count, 0, "{mode:?}");
            assert!(!result.force_applied, "{mode:?}");
            assert_eq!(config_bytes(&replica), before, "{mode:?}");
            let saved: serde_json::Value =
                serde_json::from_slice(&config_bytes(&replica)).expect("parse config");
            assert_eq!(saved["tooling"]["selectionLocked"], json!(true), "{mode:?}");
        }
    }

    #[tokio::test]
    async fn issue_2010_selection_api_replica_locked_different_pair_still_errors() {
        for mode in [
            super::AssignmentMode::Ordinary,
            super::AssignmentMode::AssignAndLock,
        ] {
            let fixture = selection_api_fixture();
            let replica = selection_api_replica(
                &fixture,
                "room-1-team",
                "dev-rust",
                locked_tooling("A", "agent-0"),
            );
            let settings = state_for(selection_api_settings(&fixture));
            let before = config_bytes(&replica);
            let preview = api_preview(
                &settings,
                &replica,
                super::ProfileAssignmentScope::Replica,
                mode,
            )
            .await;

            let result = api_apply(
                &settings,
                &replica,
                super::ProfileAssignmentScope::Replica,
                mode,
                None,
                Some(&preview.target_fingerprint),
            )
            .await
            .expect("apply reaches the writer");

            assert_eq!(result.errors.len(), 1, "{mode:?}: {:?}", result.errors);
            assert!(
                result.errors[0].message.contains("locked"),
                "{mode:?}: {:?}",
                result.errors
            );
            assert_eq!(result.updated_count, 0, "{mode:?}");
            assert_eq!(config_bytes(&replica), before, "{mode:?}");
        }
    }

    #[tokio::test]
    async fn issue_1937_selection_api_rejects_decision_outside_bulk_assign_and_lock() {
        let fixture = selection_api_fixture();
        let anchor = selection_api_replica(
            &fixture,
            "room-1-team",
            "a-rust",
            locked_tooling("A", "agent-0"),
        );
        selection_api_replica(
            &fixture,
            "room-1-team",
            "b-rust",
            locked_tooling("A", "agent-0"),
        );
        let settings = state_for(selection_api_settings(&fixture));
        let bytes_before = config_bytes(&anchor);

        let ordinary = api_apply(
            &settings,
            &anchor,
            super::ProfileAssignmentScope::Workgroup,
            super::AssignmentMode::Ordinary,
            Some(super::ConflictDecision::ForceReviewed),
            Some("whatever"),
        )
        .await
        .expect_err("ordinary plus decision must be rejected");
        assert!(
            ordinary.contains("only valid with assignmentMode assignAndLock"),
            "{ordinary}"
        );

        let single = api_apply(
            &settings,
            &anchor,
            super::ProfileAssignmentScope::Replica,
            super::AssignmentMode::AssignAndLock,
            Some(super::ConflictDecision::ForceReviewed),
            Some("whatever"),
        )
        .await
        .expect_err("replica plus decision must be rejected");
        assert!(single.contains("not valid for replica scope"), "{single}");

        assert_eq!(
            config_bytes(&anchor),
            bytes_before,
            "no writes on rejection"
        );
        assert!(!sidecar_path(&anchor).exists(), "no sidecar on rejection");
    }

    #[tokio::test]
    async fn issue_1937_selection_api_bulk_assign_and_lock_requires_decision() {
        let fixture = selection_api_fixture();
        let anchor = selection_api_replica(
            &fixture,
            "room-1-team",
            "a-rust",
            locked_tooling("A", "agent-0"),
        );
        let second = selection_api_replica(
            &fixture,
            "room-1-team",
            "b-rust",
            locked_tooling("A", "agent-0"),
        );
        let settings = state_for(selection_api_settings(&fixture));
        let bytes_before = config_bytes(&anchor);
        let second_bytes = config_bytes(&second);
        let preview = api_preview(
            &settings,
            &anchor,
            super::ProfileAssignmentScope::Workgroup,
            super::AssignmentMode::AssignAndLock,
        )
        .await;

        let error = api_apply(
            &settings,
            &anchor,
            super::ProfileAssignmentScope::Workgroup,
            super::AssignmentMode::AssignAndLock,
            None,
            Some(&preview.target_fingerprint),
        )
        .await
        .expect_err("conflicts need a decision");
        assert!(error.contains("conflictDecisionRequired"), "{error}");
        // A force decision without the current preview fingerprint is rejected.
        let error = api_apply(
            &settings,
            &anchor,
            super::ProfileAssignmentScope::Workgroup,
            super::AssignmentMode::AssignAndLock,
            Some(super::ConflictDecision::ForceReviewed),
            None,
        )
        .await
        .expect_err("a force decision still needs the reviewed fingerprint");
        // #2051 - a missing fingerprint is reported as missing, never stale.
        assert!(error.contains("confirmationMissing"), "{error}");
        assert!(!error.contains("Target selection changed"), "{error}");
        assert_eq!(config_bytes(&anchor), bytes_before);
        assert_eq!(config_bytes(&second), second_bytes);
        assert!(!sidecar_path(&anchor).exists());
        assert!(!sidecar_path(&second).exists());
    }

    #[tokio::test]
    async fn issue_1937_selection_api_decision_snapshots_are_per_decision() {
        let fixture = selection_api_fixture();
        let anchor = selection_api_replica(
            &fixture,
            "room-1-team",
            "a-rust",
            locked_tooling("A", "agent-0"),
        );
        selection_api_replica(
            &fixture,
            "room-1-team",
            "b-rust",
            locked_tooling("A", "agent-0"),
        );
        let settings = state_for(selection_api_settings(&fixture));
        let preview = api_preview(
            &settings,
            &anchor,
            super::ProfileAssignmentScope::Workgroup,
            super::AssignmentMode::AssignAndLock,
        )
        .await;
        let decisions = preview.decisions.as_ref().expect("decisions");

        // Force with the unlockedOnly hash is a stale approval: no force applied.
        let stale = api_apply(
            &settings,
            &anchor,
            super::ProfileAssignmentScope::Workgroup,
            super::AssignmentMode::AssignAndLock,
            Some(super::ConflictDecision::ForceReviewed),
            Some(&decisions.unlocked_only.fingerprint),
        )
        .await
        .expect_err("force with the wrong decision hash");
        assert!(stale.contains("stalePreview"), "{stale}");

        // UnlockedOnly with its own hash skips every protected replica.
        let unlocked_only = api_apply(
            &settings,
            &anchor,
            super::ProfileAssignmentScope::Workgroup,
            super::AssignmentMode::AssignAndLock,
            Some(super::ConflictDecision::UnlockedOnly),
            Some(&decisions.unlocked_only.fingerprint),
        )
        .await
        .expect("unlockedOnly apply");
        assert_eq!(unlocked_only.updated_count, 0);
        assert_eq!(unlocked_only.skipped_locked_paths.len(), 2);
        assert!(!unlocked_only.force_applied);
        assert_eq!(unlocked_only.locked_after_apply_count, Some(2));

        // ForceReviewed with its own hash overwrites the protected targets.
        let forced = api_apply(
            &settings,
            &anchor,
            super::ProfileAssignmentScope::Workgroup,
            super::AssignmentMode::AssignAndLock,
            Some(super::ConflictDecision::ForceReviewed),
            Some(&decisions.force_reviewed.fingerprint),
        )
        .await
        .expect("forceReviewed apply");
        assert_eq!(forced.updated_count, 2);
        assert!(forced.skipped_locked_paths.is_empty());
        assert!(
            forced.force_applied,
            "a protected target was actually overwritten"
        );
        assert_eq!(forced.locked_after_apply_count, Some(2));
        assert!(
            forced.newly_protected_paths.is_empty(),
            "already locked stays locked"
        );
    }

    #[tokio::test]
    async fn issue_1937_selection_api_stale_approvals_are_rejected_without_writes() {
        let fixture = selection_api_fixture();
        let anchor = selection_api_replica(
            &fixture,
            "room-1-team",
            "a-rust",
            unlocked_tooling("A", "agent-0"),
        );
        let settings = state_for(selection_api_settings(&fixture));

        // (a) the stored pair changes after the preview.
        let pair_preview = api_preview(
            &settings,
            &anchor,
            super::ProfileAssignmentScope::Workgroup,
            super::AssignmentMode::Ordinary,
        )
        .await;
        let mut value: Value =
            serde_json::from_slice(&config_bytes(&anchor)).expect("parse config");
        value["tooling"]["profile"] = json!("C");
        std::fs::write(
            anchor.join("config.json"),
            serde_json::to_vec(&value).expect("serialize"),
        )
        .expect("rewrite config");
        let changed_bytes = config_bytes(&anchor);
        let error = api_apply(
            &settings,
            &anchor,
            super::ProfileAssignmentScope::Workgroup,
            super::AssignmentMode::Ordinary,
            None,
            Some(&pair_preview.target_fingerprint),
        )
        .await
        .expect_err("pair change must stale");
        assert!(error.contains("stalePreview"), "{error}");
        assert_eq!(config_bytes(&anchor), changed_bytes);

        // (b) the flag changes after the preview.
        let flag_preview = api_preview(
            &settings,
            &anchor,
            super::ProfileAssignmentScope::Workgroup,
            super::AssignmentMode::Ordinary,
        )
        .await;
        let mut value: Value =
            serde_json::from_slice(&config_bytes(&anchor)).expect("parse config");
        value["tooling"]["selectionLocked"] = json!(true);
        value["tooling"]["currentCodingAgent"] = json!("agent-0");
        std::fs::write(
            anchor.join("config.json"),
            serde_json::to_vec(&value).expect("serialize"),
        )
        .expect("rewrite config");
        let changed_bytes = config_bytes(&anchor);
        let error = api_apply(
            &settings,
            &anchor,
            super::ProfileAssignmentScope::Workgroup,
            super::AssignmentMode::Ordinary,
            None,
            Some(&flag_preview.target_fingerprint),
        )
        .await
        .expect_err("flag change must stale");
        assert!(error.contains("stalePreview"), "{error}");
        assert_eq!(config_bytes(&anchor), changed_bytes);

        // (c) membership grows after the preview.
        let membership_preview = api_preview(
            &settings,
            &anchor,
            super::ProfileAssignmentScope::Workgroup,
            super::AssignmentMode::Ordinary,
        )
        .await;
        let added = selection_api_replica(
            &fixture,
            "room-1-team",
            "b-rust",
            unlocked_tooling("A", "agent-0"),
        );
        let added_bytes = config_bytes(&added);
        let error = api_apply(
            &settings,
            &anchor,
            super::ProfileAssignmentScope::Workgroup,
            super::AssignmentMode::Ordinary,
            None,
            Some(&membership_preview.target_fingerprint),
        )
        .await
        .expect_err("membership change must stale");
        assert!(error.contains("stalePreview"), "{error}");
        assert_eq!(config_bytes(&added), added_bytes);

        // (d) a relevant live session appears after the preview.
        let manager = SessionManager::new();
        let live = selection_api_replica(
            &fixture,
            "room-1-team",
            "c-rust",
            unlocked_tooling("A", "agent-0"),
        );
        let session_mgr = Arc::new(RwLock::new(manager));
        let session_preview = super::preview_coding_agent_profile_selection_inner(
            &session_mgr,
            &settings,
            api_preview_request(
                &anchor,
                super::ProfileAssignmentScope::Workgroup,
                super::AssignmentMode::Ordinary,
            ),
        )
        .await
        .expect("preview with sessions");
        {
            let manager = session_mgr.write().await;
            manager
                .create_session(
                    "codex".to_string(),
                    Vec::new(),
                    live.to_string_lossy().to_string(),
                    Some("agent-0".to_string()),
                    Some("Codex".to_string()),
                    Vec::new(),
                    false,
                    crate::pty::backend::SessionBackendKind::LocalProcess,
                )
                .await
                .expect("create session");
        }
        let live_bytes = config_bytes(&live);
        let app = crate::test_support::test_builder()
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("build test app");
        let pty_mgr = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new_for_test(
            Arc::new(SelectionApiPtyBackend),
        )));
        let error = super::apply_coding_agent_profile_selection_inner(
            app.handle(),
            &session_mgr,
            &pty_mgr,
            &settings,
            api_apply_request(
                &anchor,
                super::ProfileAssignmentScope::Workgroup,
                super::AssignmentMode::Ordinary,
                None,
                Some(&session_preview.target_fingerprint),
            ),
        )
        .await
        .expect_err("session change must stale");
        assert!(error.contains("stalePreview"), "{error}");
        assert_eq!(config_bytes(&live), live_bytes);
    }

    #[tokio::test]
    async fn issue_1937_selection_api_skipped_locked_never_restarts_live_session() {
        let fixture = selection_api_fixture();
        let anchor = selection_api_replica(
            &fixture,
            "room-1-team",
            "a-rust",
            locked_tooling("A", "agent-0"),
        );
        let manager = SessionManager::new();
        let session = manager
            .create_session(
                "codex".to_string(),
                Vec::new(),
                anchor.to_string_lossy().to_string(),
                Some("agent-0".to_string()),
                Some("Codex".to_string()),
                Vec::new(),
                false,
                crate::pty::backend::SessionBackendKind::LocalProcess,
            )
            .await
            .expect("create session");
        let session_mgr = Arc::new(RwLock::new(manager));
        let settings = state_for(selection_api_settings(&fixture));
        let bytes_before = config_bytes(&anchor);
        let mut preview_request = api_preview_request(
            &anchor,
            super::ProfileAssignmentScope::Workgroup,
            super::AssignmentMode::Ordinary,
        );
        preview_request.restart_sessions = true;
        let preview = super::preview_coding_agent_profile_selection_inner(
            &session_mgr,
            &settings,
            preview_request,
        )
        .await
        .expect("preview");
        assert_eq!(
            preview.live_session_count, 0,
            "the actionable count skips the protected replica's session"
        );
        assert_eq!(preview.target_count, 0, "the only replica is protected");
        assert_eq!(preview.targets.len(), 1);
        assert_eq!(
            preview.targets[0].live_session_ids,
            vec![session.id.to_string()],
            "the protected target still reports its live session"
        );

        let app = crate::test_support::test_builder()
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("build test app");
        let pty_mgr = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new_for_test(
            Arc::new(SelectionApiPtyBackend),
        )));
        let mut request = api_apply_request(
            &anchor,
            super::ProfileAssignmentScope::Workgroup,
            super::AssignmentMode::Ordinary,
            None,
            Some(&preview.target_fingerprint),
        );
        request.restart_sessions = true;
        let result = super::apply_coding_agent_profile_selection_inner(
            app.handle(),
            &session_mgr,
            &pty_mgr,
            &settings,
            request,
        )
        .await
        .expect("skipping a locked replica is not an error")
        .0;

        assert_eq!(result.updated_count, 0);
        assert_eq!(
            result.restarted_count, 0,
            "no lifecycle invocation for a skip"
        );
        assert_eq!(result.skipped_locked_paths.len(), 1);
        assert!(result.errors.is_empty());
        assert_eq!(config_bytes(&anchor), bytes_before);
        assert!(
            session_mgr
                .read()
                .await
                .get_session(session.id)
                .await
                .is_some(),
            "the skipped replica's session must survive untouched"
        );
    }

    #[tokio::test]
    async fn issue_1937_selection_api_later_target_race_yields_partial_results() {
        let fixture = selection_api_fixture();
        let anchor = selection_api_replica(
            &fixture,
            "room-1-team",
            "a-rust",
            unlocked_tooling("A", "agent-0"),
        );
        let later = selection_api_replica(
            &fixture,
            "room-1-team",
            "b-rust",
            unlocked_tooling("A", "agent-0"),
        );
        let settings = state_for(selection_api_settings(&fixture));
        let preview = api_preview(
            &settings,
            &anchor,
            super::ProfileAssignmentScope::Workgroup,
            super::AssignmentMode::Ordinary,
        )
        .await;
        let later_bytes = config_bytes(&later);

        // Hold the later target's sidecar lock to force a real per-file write
        // failure after the first target has already committed.
        let lock_file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(sidecar_path(&later))
            .expect("open sidecar");
        lock_file.lock().expect("hold later target lock");
        let result = api_apply(
            &settings,
            &anchor,
            super::ProfileAssignmentScope::Workgroup,
            super::AssignmentMode::Ordinary,
            None,
            Some(&preview.target_fingerprint),
        )
        .await
        .expect("earlier writes stay committed");
        lock_file.unlock().expect("release sidecar");
        drop(lock_file);

        assert_eq!(result.updated_count, 1, "the first target still committed");
        assert!(result.updated_replica_paths[0].contains("a-rust"));
        assert_eq!(
            config_bytes(&later),
            later_bytes,
            "the failed target is untouched"
        );
        assert_eq!(result.errors.len(), 1);
        assert_eq!(result.errors[0].code, "configLockTimeout");
        assert!(result.errors[0].replica_paths[0].contains("b-rust"));
        assert_eq!(result.locked_after_apply_count, Some(0));
    }

    #[tokio::test]
    async fn issue_1937_selection_api_invalid_state_is_visible_and_never_written() {
        let fixture = selection_api_fixture();
        let anchor = selection_api_replica(
            &fixture,
            "room-1-team",
            "a-rust",
            json!({"selectionLocked": true}),
        );
        let unlocked = selection_api_replica(
            &fixture,
            "room-1-team",
            "b-rust",
            unlocked_tooling("A", "agent-0"),
        );
        let settings = state_for(selection_api_settings(&fixture));
        let invalid_bytes = config_bytes(&anchor);
        let preview = api_preview(
            &settings,
            &anchor,
            super::ProfileAssignmentScope::Workgroup,
            super::AssignmentMode::Ordinary,
        )
        .await;
        assert_eq!(preview.invalid_count, 1);
        assert_eq!(preview.protected_count, 0);
        assert_eq!(
            preview.target_count, 1,
            "only the valid replica is actionable"
        );
        let invalid = preview
            .targets
            .iter()
            .find(|target| target.replica_path.contains("a-rust"))
            .expect("invalid target listed");
        assert_eq!(invalid.selection_state, super::SelectionState::Invalid);
        assert!(invalid.saved_pair.is_none());
        assert!(invalid.selection_error.is_some());

        let result = api_apply(
            &settings,
            &anchor,
            super::ProfileAssignmentScope::Workgroup,
            super::AssignmentMode::Ordinary,
            None,
            Some(&preview.target_fingerprint),
        )
        .await
        .expect("valid targets still apply");
        assert_eq!(result.updated_count, 1);
        assert_eq!(result.invalid_paths.len(), 1);
        assert_eq!(result.errors.len(), 1);
        assert_eq!(result.errors[0].code, "invalidSelectionState");
        assert_eq!(config_bytes(&anchor), invalid_bytes);
        assert_eq!(
            result.locked_after_apply_count, None,
            "invalid state is unknowable"
        );
        let saved: Value = serde_json::from_slice(&config_bytes(&unlocked)).expect("parse");
        assert_eq!(saved["tooling"]["profile"], json!("B"));
    }

    #[tokio::test]
    async fn issue_1937_selection_api_anchor_must_be_a_configured_replica() {
        let fixture = selection_api_fixture();
        let replica = selection_api_replica(
            &fixture,
            "room-1-team",
            "dev-rust",
            unlocked_tooling("A", "agent-0"),
        );
        let matrix = fixture.ac_root.join("_agent_dev-rust");
        // Origin Matrix is not a lockable replica target.
        let mut unconfigured = settings_with_single_agent();
        unconfigured.project_paths = Vec::new();
        let unconfigured = state_for(unconfigured);
        let error = api_preview_result(
            &unconfigured,
            &replica,
            super::ProfileAssignmentScope::Replica,
            super::AssignmentMode::Ordinary,
        )
        .await
        .expect_err("an unconfigured project root must be rejected");
        assert!(
            error.contains("outside configured AC project roots"),
            "{error}"
        );

        let settings = state_for(selection_api_settings(&fixture));
        let error = api_preview_result(
            &settings,
            &matrix,
            super::ProfileAssignmentScope::Replica,
            super::AssignmentMode::Ordinary,
        )
        .await
        .expect_err("the origin Matrix must be rejected");
        assert!(error.contains("not a Room replica"), "{error}");

        // A `__agent_*` directly under `.ac` has no room parent.
        let roomless = fixture.ac_root.join("__agent_dev-rust");
        std::fs::create_dir_all(&roomless).expect("create roomless replica dir");
        let error = api_preview_result(
            &settings,
            &roomless,
            super::ProfileAssignmentScope::Replica,
            super::AssignmentMode::Ordinary,
        )
        .await
        .expect_err("a roomless replica must be rejected");
        assert!(
            error.contains("room-*") || error.contains("Room"),
            "{error}"
        );
    }

    #[tokio::test]
    async fn issue_1937_selection_api_removal_zero_protected_is_a_pure_no_op() {
        let fixture = selection_api_fixture();
        let anchor = selection_api_replica(
            &fixture,
            "room-1-team",
            "a-rust",
            unlocked_tooling("A", "agent-0"),
        );
        let second = selection_api_replica(
            &fixture,
            "room-1-team",
            "b-rust",
            unlocked_tooling("C", "agent-0"),
        );
        let matrix = fixture.ac_root.join("_agent_a-rust");
        let settings = state_for(selection_api_settings(&fixture));
        let anchor_bytes = config_bytes(&anchor);
        let second_bytes = config_bytes(&second);
        let mut dir_before: Vec<std::ffi::OsString> = std::fs::read_dir(&anchor)
            .expect("read anchor dir")
            .map(|entry| entry.expect("entry").file_name())
            .collect();
        dir_before.sort();

        let preview =
            api_removal_preview(&settings, &anchor, super::ProfileAssignmentScope::Workgroup).await;
        assert!(preview.counts_complete);
        assert_eq!(preview.candidate_count, 2);
        assert_eq!(preview.protected_count, 0);
        assert_eq!(preview.already_unlocked_count, 2);
        assert_eq!(preview.invalid_count, 0);
        assert_eq!(preview.target_fingerprint.len(), 64);

        let result = api_removal_apply(
            &settings,
            &anchor,
            super::ProfileAssignmentScope::Workgroup,
            &preview.target_fingerprint,
        )
        .await
        .expect("zero-protected removal");
        assert_eq!(result.removed_count, 0);
        assert_eq!(result.already_unlocked_paths.len(), 2);
        assert!(result.failed_replica_paths.is_empty());
        assert_eq!(result.remaining_protected_count, Some(0));
        assert!(result.errors.is_empty());

        assert_eq!(config_bytes(&anchor), anchor_bytes);
        assert_eq!(config_bytes(&second), second_bytes);
        let mut dir_after: Vec<std::ffi::OsString> = std::fs::read_dir(&anchor)
            .expect("read anchor dir")
            .map(|entry| entry.expect("entry").file_name())
            .collect();
        dir_after.sort();
        assert_eq!(
            dir_after, dir_before,
            "zero-protected removal creates no sidecar or temp file"
        );
        assert!(!sidecar_path(&anchor).exists(), "no sidecar for a no-op");
        assert!(!sidecar_path(&second).exists(), "no sidecar for a no-op");
        assert!(
            !matrix.join("config.json").exists(),
            "removal never writes a Matrix default"
        );

        // The single-replica scope has the same zero-protected shape.
        let single =
            api_removal_preview(&settings, &anchor, super::ProfileAssignmentScope::Replica).await;
        assert_eq!(single.candidate_count, 1);
        assert_eq!(single.protected_count, 0);
        let single_result = api_removal_apply(
            &settings,
            &anchor,
            super::ProfileAssignmentScope::Replica,
            &single.target_fingerprint,
        )
        .await
        .expect("single zero-protected removal");
        assert_eq!(single_result.removed_count, 0);
        assert_eq!(config_bytes(&anchor), anchor_bytes);
    }

    #[tokio::test]
    async fn issue_1937_selection_api_removal_preserves_pair_and_unknown_tooling() {
        let fixture = selection_api_fixture();
        let replica = selection_api_replica(
            &fixture,
            "room-1-team",
            "dev-rust",
            json!({
                "profile": "A",
                "currentCodingAgent": "agent-0",
                "selectionLocked": true,
                "lastCodingAgent": "legacy-agent",
                "profileContentHash": "deadbeef",
            }),
        );
        let settings = state_for(selection_api_settings(&fixture));
        let preview =
            api_removal_preview(&settings, &replica, super::ProfileAssignmentScope::Replica).await;
        assert_eq!(preview.protected_count, 1);

        let result = api_removal_apply(
            &settings,
            &replica,
            super::ProfileAssignmentScope::Replica,
            &preview.target_fingerprint,
        )
        .await
        .expect("removal");
        assert_eq!(result.removed_count, 1);
        assert_eq!(result.remaining_protected_count, Some(0));
        assert!(result.errors.is_empty());
        let saved: Value = serde_json::from_slice(&config_bytes(&replica)).expect("parse");
        assert_eq!(saved["tooling"]["selectionLocked"], json!(false));
        assert_eq!(saved["tooling"]["currentCodingAgent"], json!("agent-0"));
        assert_eq!(saved["tooling"]["profile"], json!("A"));
        assert_eq!(saved["tooling"]["lastCodingAgent"], json!("legacy-agent"));
        assert_eq!(saved["tooling"]["profileContentHash"], json!("deadbeef"));
    }

    #[tokio::test]
    async fn issue_1937_selection_api_removal_mixed_skips_the_unlocked_target() {
        let fixture = selection_api_fixture();
        let first = selection_api_replica(
            &fixture,
            "room-1-team",
            "a-rust",
            locked_tooling("A", "agent-0"),
        );
        selection_api_replica(
            &fixture,
            "room-1-team",
            "b-rust",
            locked_tooling("B", "agent-0"),
        );
        selection_api_replica(
            &fixture,
            "room-1-team",
            "c-rust",
            locked_tooling("C", "agent-0"),
        );
        let unlocked_fourth = selection_api_replica(
            &fixture,
            "room-1-team",
            "d-rust",
            unlocked_tooling("A", "agent-0"),
        );
        let settings = state_for(selection_api_settings(&fixture));
        let fourth_bytes = config_bytes(&unlocked_fourth);
        let preview =
            api_removal_preview(&settings, &first, super::ProfileAssignmentScope::Workgroup).await;
        assert_eq!(preview.candidate_count, 4);
        assert_eq!(preview.protected_count, 3);
        assert_eq!(preview.already_unlocked_count, 1);

        let result = api_removal_apply(
            &settings,
            &first,
            super::ProfileAssignmentScope::Workgroup,
            &preview.target_fingerprint,
        )
        .await
        .expect("mixed removal");
        assert_eq!(result.removed_count, 3);
        assert_eq!(result.already_unlocked_paths.len(), 1);
        assert!(result.already_unlocked_paths[0].contains("d-rust"));
        assert_eq!(
            config_bytes(&unlocked_fourth),
            fourth_bytes,
            "no writer for the fourth"
        );
        assert!(
            !sidecar_path(&unlocked_fourth).exists(),
            "the unlocked fourth target must not create a write sidecar"
        );
        assert!(
            sidecar_path(&first).exists(),
            "the protected targets did write"
        );
        assert_eq!(result.remaining_protected_count, Some(0));
        assert!(result.errors.is_empty());
    }

    #[tokio::test]
    async fn issue_1937_selection_api_removal_stale_scope_rejects_without_effects() {
        let fixture = selection_api_fixture();
        let first = selection_api_replica(
            &fixture,
            "room-1-team",
            "a-rust",
            locked_tooling("A", "agent-0"),
        );
        let second = selection_api_replica(
            &fixture,
            "room-1-team",
            "b-rust",
            locked_tooling("B", "agent-0"),
        );
        let settings = state_for(selection_api_settings(&fixture));
        let preview =
            api_removal_preview(&settings, &first, super::ProfileAssignmentScope::Workgroup).await;
        // Another window unlocks the first replica after the preview.
        let mut value: Value = serde_json::from_slice(&config_bytes(&first)).expect("parse");
        value["tooling"]["selectionLocked"] = json!(false);
        std::fs::write(
            first.join("config.json"),
            serde_json::to_vec(&value).expect("serialize"),
        )
        .expect("rewrite");
        let first_bytes = config_bytes(&first);
        let second_bytes = config_bytes(&second);

        let error = api_removal_apply(
            &settings,
            &first,
            super::ProfileAssignmentScope::Workgroup,
            &preview.target_fingerprint,
        )
        .await
        .expect_err("scope-level stale");
        assert!(error.contains("stalePreview"), "{error}");
        assert_eq!(config_bytes(&first), first_bytes);
        assert_eq!(
            config_bytes(&second),
            second_bytes,
            "the second stays locked"
        );
    }

    #[tokio::test]
    async fn issue_1937_selection_api_per_file_cas_rejects_changed_target() {
        let fixture = selection_api_fixture();
        let replica = selection_api_replica(
            &fixture,
            "room-1-team",
            "dev-rust",
            unlocked_tooling("A", "agent-0"),
        );
        let settings = state_for(selection_api_settings(&fixture));
        let settings_snapshot = settings.read().await.clone();
        let enumeration = super::enumerate_profile_assignment_targets(
            &settings_snapshot,
            &replica,
            &super::ProfileAssignmentScope::Replica,
            &[],
        )
        .expect("enumerate");
        let target = &enumeration.candidates[0].target;
        let expected = crate::config::coding_agent_profiles::ReplicaSelectionExpectation {
            identity: target.identity_path.clone(),
            pair: super::target_pair(target),
            locked: false,
        };
        // The stored pair changes after the enumeration but before the write.
        let mut value: Value = serde_json::from_slice(&config_bytes(&replica)).expect("parse");
        value["tooling"]["profile"] = json!("C");
        std::fs::write(
            replica.join("config.json"),
            serde_json::to_vec(&value).expect("serialize"),
        )
        .expect("rewrite");
        let changed_bytes = config_bytes(&replica);

        let error = crate::config::coding_agent_profiles::write_replica_selection(
            &settings_snapshot,
            &replica,
            &crate::config::coding_agent_profiles::ReplicaSelectionPair {
                coding_agent_id: "agent-0".to_string(),
                requested_profile: "B".to_string(),
            },
            crate::config::coding_agent_profiles::SelectionWriteIntent::BulkOrdinary,
            &expected,
        )
        .expect_err("stale expectation must fail its CAS");
        assert!(error.starts_with("stale replica selection"), "{error}");
        assert_eq!(
            config_bytes(&replica),
            changed_bytes,
            "failed CAS writes nothing"
        );
    }

    #[tokio::test]
    async fn issue_1937_selection_api_default_round_trip_and_atomic_publication() {
        let fixture = selection_api_fixture();
        let replica = selection_api_replica(
            &fixture,
            "room-1-team",
            "dev-rust",
            unlocked_tooling("A", "agent-0"),
        );
        let matrix = fixture.ac_root.join("_agent_dev-rust");
        std::fs::write(
            matrix.join("config.json"),
            serde_json::to_vec(&json!({
                "tooling": { "defaultProfile": "A", "unknownKey": 123 },
            }))
            .expect("serialize matrix config"),
        )
        .expect("seed matrix config");
        let settings = state_for(selection_api_settings(&fixture));
        let replica_bytes = config_bytes(&replica);

        let initial = api_default(&settings, &replica).await;
        assert!(initial.default.is_none());
        assert_eq!(initial.default_fingerprint.len(), 64);
        // The returned replica path is the canonical identity, not the raw
        // spelling: on Windows the temp path may carry an 8.3 short component
        // (`RUNNER~1`) while the validated path is the long canonical form.
        assert_eq!(initial.target_replica_path, canonical_display(&replica));

        let error = super::set_replica_selection_default_inner(
            &settings,
            api_set_default_request(&replica, "agent-0", "B", true, "wrong"),
        )
        .await
        .expect_err("stale default fingerprint");
        assert!(error.contains("stalePreview"), "{error}");

        let (set, payload) = super::set_replica_selection_default_inner(
            &settings,
            api_set_default_request(&replica, "agent-0", "B", true, &initial.default_fingerprint),
        )
        .await
        .expect("set default");
        assert_eq!(
            set.default,
            Some(super::SelectionDefault {
                coding_agent_id: "agent-0".to_string(),
                requested_profile: "B".to_string(),
                selection_locked: true,
            })
        );
        assert_eq!(payload["scope"], json!("default"));
        assert_eq!(payload["operation"], json!("default"));
        assert_eq!(payload["agentPath"], json!(set.matrix_path));
        assert_eq!(payload["affectedPaths"], json!([]));
        assert_eq!(payload["default"]["selectionLocked"], json!(true));

        let disk: Value =
            serde_json::from_slice(&std::fs::read(matrix.join("config.json")).expect("read"))
                .expect("parse matrix config");
        assert_eq!(
            disk["tooling"]["replicaSelectionDefault"],
            json!({
                "codingAgentId": "agent-0",
                "requestedProfile": "B",
                "selectionLocked": true,
            })
        );
        assert_eq!(disk["tooling"]["defaultProfile"], json!("A"));
        assert_eq!(disk["tooling"]["unknownKey"], json!(123));
        assert_eq!(
            config_bytes(&replica),
            replica_bytes,
            "replicas are untouched"
        );

        let reread = api_default(&settings, &replica).await;
        assert_eq!(reread.default, set.default);
        assert_eq!(reread.default_fingerprint, set.default_fingerprint);

        // A second window that still holds the pre-set fingerprint is stale.
        let error = super::set_replica_selection_default_inner(
            &settings,
            api_set_default_request(
                &replica,
                "agent-0",
                "C",
                false,
                &initial.default_fingerprint,
            ),
        )
        .await
        .expect_err("the old window must not overwrite the new default");
        assert!(error.contains("stalePreview"), "{error}");
        let unchanged: Value =
            serde_json::from_slice(&std::fs::read(matrix.join("config.json")).expect("read"))
                .expect("parse");
        assert_eq!(
            unchanged["tooling"]["replicaSelectionDefault"]["requestedProfile"],
            json!("B")
        );
    }

    #[tokio::test]
    async fn issue_1937_selection_api_default_rejects_malformed_and_invalid_input() {
        let fixture = selection_api_fixture();
        let replica = selection_api_replica(
            &fixture,
            "room-1-team",
            "dev-rust",
            unlocked_tooling("A", "agent-0"),
        );
        let matrix = fixture.ac_root.join("_agent_dev-rust");
        let settings = state_for(selection_api_settings(&fixture));

        // A malformed stored default is an error, never a silent absence.
        std::fs::write(
            matrix.join("config.json"),
            serde_json::to_vec(&json!({
                "tooling": { "replicaSelectionDefault": { "codingAgentId": "agent-0" } },
            }))
            .expect("serialize malformed config"),
        )
        .expect("write malformed config");
        let error =
            super::get_replica_selection_default_inner(&settings, api_default_request(&replica))
                .await
                .expect_err("malformed default must fail the read");
        let error = error.to_string();
        assert!(error.contains("replicaSelectionDefault"), "{error}");

        let error = super::set_replica_selection_default_inner(
            &settings,
            api_set_default_request(&replica, "agent-0", "B", true, "whatever"),
        )
        .await
        .expect_err("malformed prior default must fail the CAS read");
        assert!(error.contains("replicaSelectionDefault"), "{error}");

        std::fs::remove_file(matrix.join("config.json")).expect("remove malformed config");
        let error = super::set_replica_selection_default_inner(
            &settings,
            api_set_default_request(&replica, "agent-0", "BB", true, "whatever"),
        )
        .await
        .expect_err("the profile must be a single letter");
        assert!(error.contains("single letter A through Z"), "{error}");

        let error = super::set_replica_selection_default_inner(
            &settings,
            api_set_default_request(&replica, "missing-agent", "B", true, "whatever"),
        )
        .await
        .expect_err("the coding agent must be configured");
        assert!(error.contains("not configured"), "{error}");
    }

    #[tokio::test]
    async fn issue_1937_selection_api_barrier_unlock_waits_for_paused_apply_publication() {
        let fixture = selection_api_fixture();
        let replica = selection_api_replica(
            &fixture,
            "room-1-team",
            "dev-rust",
            unlocked_tooling("A", "agent-0"),
        );
        let settings = state_for(selection_api_settings(&fixture));
        let settings_for_holder = settings.clone();
        let replica_for_holder = replica.clone();
        let (entered_tx, entered_rx) = oneshot::channel::<()>();
        let (release_tx, release_rx) = oneshot::channel::<()>();

        let holder = tokio::spawn(crate::session::selection::run_owned_selection_operation(
            move || {
                let settings = settings_for_holder;
                let replica = replica_for_holder;
                async move {
                    // Publication phase of an apply, then the held restart span.
                    let snapshot = settings.read().await.clone();
                    let enumeration = super::enumerate_profile_assignment_targets(
                        &snapshot,
                        &replica,
                        &super::ProfileAssignmentScope::Replica,
                        &[],
                    )
                    .expect("enumerate");
                    let target = &enumeration.candidates[0].target;
                    let expected =
                        crate::config::coding_agent_profiles::ReplicaSelectionExpectation {
                            identity: target.identity_path.clone(),
                            pair: super::target_pair(target),
                            locked: false,
                        };
                    crate::config::coding_agent_profiles::write_replica_selection(
                        &snapshot,
                        &replica,
                        &crate::config::coding_agent_profiles::ReplicaSelectionPair {
                            coding_agent_id: "agent-0".to_string(),
                            requested_profile: "B".to_string(),
                        },
                        crate::config::coding_agent_profiles::SelectionWriteIntent::IndividualAssignLock,
                        &expected,
                    )
                    .expect("publish the lock");
                    entered_tx.send(()).expect("signal entered");
                    release_rx.await.expect("release the publication span");
                    Ok::<(), String>(())
                }
            },
        ));
        entered_rx.await.expect("holder entered");

        let preview =
            api_removal_preview(&settings, &replica, super::ProfileAssignmentScope::Replica).await;
        assert_eq!(preview.protected_count, 1);
        let fingerprint = preview.target_fingerprint.clone();
        let settings_for_unlock = settings.clone();
        let replica_for_unlock = replica.clone();
        let unlock = tokio::spawn(crate::session::selection::run_owned_selection_operation(
            move || {
                let settings = settings_for_unlock;
                let replica = replica_for_unlock;
                let fingerprint = fingerprint.clone();
                async move {
                    super::apply_selection_lock_removal_inner(
                        &settings,
                        api_removal_apply_request(
                            &replica,
                            super::ProfileAssignmentScope::Replica,
                            &fingerprint,
                        ),
                    )
                    .await
                    .map(|(result, _payload)| result)
                }
            },
        ));
        for _ in 0..8 {
            tokio::task::yield_now().await;
        }
        assert!(
            !unlock.is_finished(),
            "the unlock must not acknowledge while the earlier apply still owns the turn"
        );

        release_tx.send(()).expect("release holder");
        holder
            .await
            .expect("holder join")
            .expect("holder operation");
        let removed = unlock
            .await
            .expect("unlock join")
            .expect("unlock operation");
        assert_eq!(removed.removed_count, 1);
        assert_eq!(removed.remaining_protected_count, Some(0));
        let saved: Value = serde_json::from_slice(&config_bytes(&replica)).expect("parse");
        assert_eq!(saved["tooling"]["selectionLocked"], json!(false));
        assert_eq!(saved["tooling"]["currentCodingAgent"], json!("agent-0"));
    }

    #[tokio::test]
    async fn issue_1937_selection_api_barrier_read_cannot_observe_half_settled_apply() {
        let fixture = selection_api_fixture();
        let replica = selection_api_replica(
            &fixture,
            "room-1-team",
            "dev-rust",
            unlocked_tooling("A", "agent-0"),
        );
        let settings = state_for(selection_api_settings(&fixture));
        let settings_for_holder = settings.clone();
        let replica_for_holder = replica.clone();
        let (entered_tx, entered_rx) = oneshot::channel::<()>();
        let (release_tx, release_rx) = oneshot::channel::<()>();

        let holder = tokio::spawn(crate::session::selection::run_owned_selection_operation(
            move || {
                let settings = settings_for_holder;
                let replica = replica_for_holder;
                async move {
                    let snapshot = settings.read().await.clone();
                    let enumeration = super::enumerate_profile_assignment_targets(
                        &snapshot,
                        &replica,
                        &super::ProfileAssignmentScope::Replica,
                        &[],
                    )
                    .expect("enumerate");
                    let target = &enumeration.candidates[0].target;
                    let expected =
                        crate::config::coding_agent_profiles::ReplicaSelectionExpectation {
                            identity: target.identity_path.clone(),
                            pair: super::target_pair(target),
                            locked: false,
                        };
                    crate::config::coding_agent_profiles::write_replica_selection(
                        &snapshot,
                        &replica,
                        &crate::config::coding_agent_profiles::ReplicaSelectionPair {
                            coding_agent_id: "agent-0".to_string(),
                            requested_profile: "B".to_string(),
                        },
                        crate::config::coding_agent_profiles::SelectionWriteIntent::IndividualAssignLock,
                        &expected,
                    )
                    .expect("publish the lock");
                    entered_tx.send(()).expect("signal entered");
                    release_rx.await.expect("release the publication span");
                    Ok::<(), String>(())
                }
            },
        ));
        entered_rx.await.expect("holder entered");
        let published_bytes = config_bytes(&replica);
        let mut dir_before: Vec<std::ffi::OsString> = std::fs::read_dir(&replica)
            .expect("read replica dir")
            .map(|entry| entry.expect("entry").file_name())
            .collect();
        dir_before.sort();

        let settings_for_read = settings.clone();
        let replica_for_read = replica.clone();
        let read = tokio::spawn(crate::session::selection::run_owned_selection_operation(
            move || {
                let settings = settings_for_read;
                let replica = replica_for_read;
                async move {
                    super::preview_selection_lock_removal_inner(
                        &empty_session_manager(),
                        &settings,
                        api_removal_preview_request(
                            &replica,
                            super::ProfileAssignmentScope::Replica,
                        ),
                    )
                    .await
                    .map(|preview| preview.protected_count)
                }
            },
        ));
        for _ in 0..8 {
            tokio::task::yield_now().await;
        }
        assert!(
            !read.is_finished(),
            "the read must not observe the state while the mutation still owns the turn"
        );

        // Dropping the read waiter leaves the earlier mutation's ownership intact.
        read.abort();
        let _ = read.await;
        let probe = tokio::time::timeout(
            Duration::from_millis(150),
            crate::session::selection::acquire_selection_operation_turn(),
        )
        .await;
        assert!(
            probe.is_err(),
            "the earlier mutation must still own the turn after the read waiter is dropped"
        );

        release_tx.send(()).expect("release holder");
        holder
            .await
            .expect("holder join")
            .expect("holder operation");
        assert_eq!(
            config_bytes(&replica),
            published_bytes,
            "the read wrote nothing"
        );
        let mut dir_after: Vec<std::ffi::OsString> = std::fs::read_dir(&replica)
            .expect("read replica dir")
            .map(|entry| entry.expect("entry").file_name())
            .collect();
        dir_after.sort();
        assert_eq!(dir_after, dir_before, "the read created no files");

        // A fresh read after release observes the settled publication.
        let preview =
            api_removal_preview(&settings, &replica, super::ProfileAssignmentScope::Replica).await;
        assert_eq!(preview.protected_count, 1);
        assert_eq!(
            preview.targets[0].saved_pair,
            Some(super::SavedPair {
                coding_agent_id: Some("agent-0".to_string()),
                requested_profile: Some("B".to_string()),
            })
        );
    }

    // ── #2348 - whole-settings writers inherit central placement preservation ──

    fn placement_geometry(x: f64, y: f64) -> WindowGeometry {
        WindowGeometry {
            x,
            y,
            width: 800.0,
            height: 600.0,
        }
    }

    /// Seed a `settings.json` with the placement pair present, absent, or both,
    /// so a Preserve save has (or lacks) disk truth to copy.
    fn write_placement_settings_file(
        dir: &Path,
        geometry: Option<(f64, f64)>,
        state: Option<MainWindowDisplayState>,
    ) {
        let mut value = serde_json::to_value(settings_with_single_agent()).unwrap();
        let object = value.as_object_mut().unwrap();
        match geometry {
            Some((x, y)) => {
                object.insert(
                    "mainGeometry".to_string(),
                    json!({ "x": x, "y": y, "width": 800.0, "height": 600.0 }),
                );
            }
            None => {
                object.remove("mainGeometry");
            }
        }
        match state {
            Some(state) => {
                object.insert(
                    "mainWindowDisplayState".to_string(),
                    serde_json::to_value(state).unwrap(),
                );
            }
            None => {
                object.remove("mainWindowDisplayState");
            }
        }
        std::fs::write(
            dir.join("settings.json"),
            serde_json::to_string_pretty(&value).unwrap(),
        )
        .unwrap();
    }

    fn assert_disk_placement(dir: &Path, expected_state: MainWindowDisplayState, expected_x: f64) {
        let raw = std::fs::read_to_string(dir.join("settings.json")).unwrap();
        let disk: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(
            disk["mainWindowDisplayState"],
            serde_json::to_value(expected_state).unwrap()
        );
        assert_eq!(disk["mainGeometry"]["x"], json!(expected_x));
    }

    #[tokio::test]
    async fn issue_2348_settings_update_cannot_regress_the_disk_placement_pair() {
        for (label, disk_state, stale_state) in [
            (
                "maximized disk, normal stale",
                MainWindowDisplayState::Maximized,
                MainWindowDisplayState::Normal,
            ),
            (
                "normal disk, maximized stale",
                MainWindowDisplayState::Normal,
                MainWindowDisplayState::Maximized,
            ),
        ] {
            let temp = tempfile::tempdir().unwrap();
            let settings_path = temp.path().join("settings.json");
            write_placement_settings_file(temp.path(), Some((1.0, 2.0)), Some(disk_state));

            let mut current = settings_with_single_agent();
            current.main_geometry = Some(placement_geometry(1.0, 2.0));
            current.main_window_display_state = disk_state;
            let state = state_for(current.clone());

            let mut stale = current.clone();
            stale.main_geometry = Some(placement_geometry(999.0, 999.0));
            stale.main_window_display_state = stale_state;

            let saved = persist_protected_settings_update_with_saver(&state, stale, |candidate| {
                crate::config::settings::save_settings_to_path_preserving_project_paths(
                    candidate,
                    &settings_path,
                )
            })
            .await
            .unwrap();

            assert_eq!(saved.main_window_display_state, disk_state, "{label}");
            assert_eq!(
                saved.main_geometry.as_ref().map(|g| (g.x, g.y)),
                Some((1.0, 2.0)),
                "{label}"
            );
            assert_disk_placement(temp.path(), disk_state, 1.0);
        }
    }

    #[tokio::test]
    async fn issue_2348_settings_draft_cannot_regress_the_disk_placement_pair() {
        let temp = tempfile::tempdir().unwrap();
        let settings_path = temp.path().join("settings.json");
        write_placement_settings_file(
            temp.path(),
            Some((1.0, 2.0)),
            Some(MainWindowDisplayState::Maximized),
        );

        let mut current = settings_with_single_agent();
        current.main_geometry = Some(placement_geometry(1.0, 2.0));
        current.main_window_display_state = MainWindowDisplayState::Maximized;
        let state = state_for(current.clone());

        let mut draft = current.clone();
        draft.main_geometry = Some(placement_geometry(999.0, 999.0));
        draft.main_window_display_state = MainWindowDisplayState::Normal;

        let (saved, _events) =
            persist_settings_draft_update_with_saver(&state, draft, |candidate| {
                crate::config::settings::save_settings_to_path_preserving_project_paths(
                    candidate,
                    &settings_path,
                )
            })
            .await
            .unwrap();

        assert_eq!(
            saved.main_window_display_state,
            MainWindowDisplayState::Maximized
        );
        assert_eq!(saved.main_geometry.as_ref().map(|g| g.x), Some(1.0));
        assert_disk_placement(temp.path(), MainWindowDisplayState::Maximized, 1.0);
    }

    #[tokio::test]
    async fn issue_2348_cli_style_whole_settings_payload_cannot_regress_the_disk_placement_pair() {
        let temp = tempfile::tempdir().unwrap();
        let settings_path = temp.path().join("settings.json");
        write_placement_settings_file(
            temp.path(),
            Some((1.0, 2.0)),
            Some(MainWindowDisplayState::Maximized),
        );

        let mut stale = settings_with_single_agent();
        stale.main_geometry = Some(placement_geometry(999.0, 999.0));
        stale.main_window_display_state = MainWindowDisplayState::Normal;

        crate::config::settings::save_settings_with_project_paths_to_path(&stale, &settings_path)
            .unwrap();

        assert_disk_placement(temp.path(), MainWindowDisplayState::Maximized, 1.0);
    }

    #[tokio::test]
    async fn issue_2348_absent_disk_placement_keys_keep_the_caller_values() {
        let temp = tempfile::tempdir().unwrap();
        let settings_path = temp.path().join("settings.json");
        write_placement_settings_file(temp.path(), None, None);

        let mut candidate = settings_with_single_agent();
        candidate.main_geometry = Some(placement_geometry(11.0, 22.0));
        candidate.main_window_display_state = MainWindowDisplayState::Normal;

        let written = crate::config::settings::save_settings_to_path_preserving_project_paths(
            &candidate,
            &settings_path,
        )
        .unwrap();

        assert_eq!(written.main_geometry.as_ref().map(|g| g.x), Some(11.0));
        assert_eq!(
            written.main_window_display_state,
            MainWindowDisplayState::Normal
        );
        let raw = std::fs::read_to_string(&settings_path).unwrap();
        let disk: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(disk["mainGeometry"]["x"], json!(11.0));
        assert_eq!(disk["mainWindowDisplayState"], json!("normal"));
    }

    /// #2133 (P4) - every test injects `<tempdir>/settings.json`; none resolves the config dir.
    mod agent_help_2133 {
        use super::super::{agent_help_overlay_payload, AgentHelpOverlayPayload};
        use crate::config::settings::shipped_agent_help;
        use serde_json::json;

        fn payload_with_local(contents: Option<&str>) -> AgentHelpOverlayPayload {
            let dir = tempfile::tempdir().unwrap();
            let settings_path = dir.path().join("settings.json");
            if let Some(contents) = contents {
                std::fs::write(dir.path().join("agent-help.local.json"), contents).unwrap();
            }
            agent_help_overlay_payload(Some(settings_path))
        }

        #[test]
        fn agent_help_with_no_local_file_returns_nulls() {
            let dir = tempfile::tempdir().unwrap();
            assert!(!dir.path().join("agent-help.local.json").exists());
            let payload = agent_help_overlay_payload(Some(dir.path().join("settings.json")));
            assert!(payload.local.is_none());
            assert!(payload.remote.is_none());
            assert!(payload.local_error.is_none());
        }

        #[test]
        fn agent_help_serves_the_local_layer() {
            let payload = payload_with_local(Some(
                r#"{"schemaVersion":1,
                    "byCommand":{"claude":{"paramsExample":"--mine","docsUrl":"https://mine.dev"}},
                    "byAgent":{"agent-1":{"label":"Mine"}}}"#,
            ));
            assert_eq!(payload.local_error, None);
            let local = payload.local.expect("the local layer is served");
            assert_eq!(
                local.by_command["claude"].params_example.as_deref(),
                Some("--mine")
            );
            assert_eq!(
                local.by_command["claude"].docs_url.as_deref(),
                Some("https://mine.dev")
            );
            assert_eq!(local.by_agent["agent-1"].label.as_deref(), Some("Mine"));
        }

        #[test]
        fn agent_help_reports_a_broken_local_file() {
            let payload = payload_with_local(Some("{ not json"));
            assert!(payload.local.is_none());
            let reason = payload.local_error.expect("a broken file reports why");
            assert!(!reason.trim().is_empty());
        }

        #[test]
        fn agent_help_never_serves_the_embedded_layer() {
            let shipped = shipped_agent_help();
            assert!(shipped.general.is_some() || !shipped.by_command.is_empty());
            let payload = payload_with_local(None);
            assert!(payload.local.is_none());
            assert!(payload.remote.is_none());
        }

        #[test]
        fn agent_help_leaves_remote_null_in_this_phase() {
            let payload = payload_with_local(Some(
                r#"{"schemaVersion":1,"byCommand":{"codex":{"label":"X"}}}"#,
            ));
            assert!(payload.local.is_some());
            assert!(payload.remote.is_none());
        }

        fn sorted_keys(value: &serde_json::Value) -> Vec<&str> {
            let mut keys: Vec<&str> = value
                .as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect();
            keys.sort_unstable();
            keys
        }

        #[test]
        fn the_payload_serializes_to_camel_case_keys() {
            // A valid local file, read through the injected-path helper.
            let served = payload_with_local(Some(
                r#"{"schemaVersion":1,"byCommand":{"claude":{"label":"L",
                    "paramsExample":"--p","docsUrl":"https://d.dev",
                    "tips":[{"title":"t","body":"b"}]}}}"#,
            ));
            let value = serde_json::to_value(&served).unwrap();
            assert_eq!(sorted_keys(&value), ["local", "localError", "remote"]);
            assert_eq!(value["localError"], json!(null));
            assert_eq!(value["remote"], json!(null));
            let entry = &value["local"]["byCommand"]["claude"];
            assert_eq!(entry["paramsExample"], json!("--p"));
            assert_eq!(entry["docsUrl"], json!("https://d.dev"));
            assert!(entry.get("params_example").is_none());
            assert!(entry.get("docs_url").is_none());

            // A broken local file, same path: the reason travels as `localError`.
            let broken = payload_with_local(Some(
                r#"{"schemaVersion":1,"byCommand":{"claude":{"docsUrl":"http://d.dev"}}}"#,
            ));
            let value = serde_json::to_value(&broken).unwrap();
            assert_eq!(sorted_keys(&value), ["local", "localError", "remote"]);
            assert_eq!(value["local"], json!(null));
            let reason = value["localError"]
                .as_str()
                .expect("localError is a string");
            assert!(reason.contains("docsUrl is not an https URL"), "{reason}");
            assert!(value.get("local_error").is_none());
        }
    }
}
