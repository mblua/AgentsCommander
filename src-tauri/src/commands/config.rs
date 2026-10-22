use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::hash::Hash;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, State};
use uuid::Uuid;

use sha2::{Digest, Sha256};

use crate::api::auth;
use crate::config::coding_agent_mutations::{
    move_registered_agent, reorder_registered_agent, AgentMoveDirection,
};
use crate::config::instance_artifacts::{DEBUG_LOGS_FILE_NAME, SETTINGS_FILE_NAME};
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
    "Store this token now; it is shown only once. The registry keeps only a hash. A manually \
        requested pty-input scope does not grant actuation without an automatically bound live \
        container session.";

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
    // #2133 (P6) - the downloaded layer, served under the same non-empty rule as `local`.
    let remote = crate::config::settings::load_remote_agent_help_file(&path);
    let remote_has_content =
        remote.general.is_some() || !remote.by_command.is_empty() || !remote.by_agent.is_empty();
    AgentHelpOverlayPayload {
        local: has_content.then_some(file),
        remote: remote_has_content.then_some(remote),
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
    pub room_idle_confirmation_changed: bool,
    pub profiles_changed: bool,
    pub env_agent_ids: Vec<String>,
}

// #3083: closed diagnostic vocabulary; no client-selected target or text.
const RAIL_DIAGNOSTIC_ERROR: &str = "invalid rail diagnostic";
const RAIL_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

#[derive(Debug, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
enum RailSurface {
    Embedded,
    Sidebar,
    Store,
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
enum RailKind {
    Custom,
    System,
    Nonstop,
    Ungrouped,
    Project,
    Favorites,
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
enum RailPhase {
    Arming,
    Dragging,
    Saving,
    Idle,
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
enum RailOutcome {
    Applied,
    Unchanged,
    Stale,
    Error,
    Disconnected,
    Unloaded,
    Queued,
    Discarded,
    Suppressed,
    Overflow,
    NoPress,
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
enum RailTag {
    Button,
    Span,
    Svg,
    Path,
    Aside,
    Body,
    Input,
    Textarea,
    Other,
    None,
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
enum RailEvent {
    Pointerdown,
    Pointerup,
    Pointercancel,
    Mousedown,
    Mouseup,
    Click,
    Gotpointercapture,
    Lostpointercapture,
    Focusin,
    Focusout,
    WindowFocus,
    WindowBlur,
    HandlerEnter,
    HandlerEnd,
    SelectionChange,
    RailMount,
    RailDispose,
    SectionMount,
    SectionDispose,
    NodeMount,
    NodeDispose,
    ProjectList,
    ProjectButtons,
    FavoritesEntries,
    ReloadStart,
    ReloadEnd,
    DiscoverStart,
    DiscoverEnd,
    SnapshotStart,
    SnapshotEnd,
    SnapshotsStart,
    SnapshotsEnd,
    TaskStart,
    TaskEnd,
    LoopStart,
    LoopEnd,
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RailDiagnosticRecord {
    version: u8,
    window_id: Uuid,
    seq: u64,
    at_ms: f64,
    surface: RailSurface,
    event: RailEvent,
    #[serde(skip_serializing_if = "Option::is_none")]
    rail_id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    operation_id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    project_id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    group_id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    entity_id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    node_id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    original_node_id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    object_id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    selection_project_id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    selection_group_id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    count: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    changed_count: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    kind: Option<RailKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    selection_kind: Option<RailKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    phase: Option<RailPhase>,
    #[serde(skip_serializing_if = "Option::is_none")]
    outcome: Option<RailOutcome>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pointer_id: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    button: Option<i8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    target_tag: Option<RailTag>,
    #[serde(skip_serializing_if = "Option::is_none")]
    active_tag: Option<RailTag>,
    #[serde(skip_serializing_if = "Option::is_none")]
    duration_ms: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    primary: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    trusted: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    connected: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    original_connected: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    captured: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    document_focused: Option<bool>,
}

impl RailDiagnosticRecord {
    fn validate(&self) -> bool {
        self.version == 1
            && (1..=RAIL_SAFE_INTEGER).contains(&self.seq)
            && self.at_ms.is_finite()
            && self.at_ms >= 0.0
            && self.duration_ms.is_none_or(|n| n.is_finite() && n >= 0.0)
            && self.button.is_none_or(|n| (-1..=5).contains(&n))
            && self
                .rail_id
                .is_none_or(|n| (1..=RAIL_SAFE_INTEGER).contains(&n))
            && self
                .operation_id
                .is_none_or(|n| (1..=RAIL_SAFE_INTEGER).contains(&n))
            && self
                .project_id
                .is_none_or(|n| (1..=RAIL_SAFE_INTEGER).contains(&n))
            && self
                .group_id
                .is_none_or(|n| (1..=RAIL_SAFE_INTEGER).contains(&n))
            && self
                .entity_id
                .is_none_or(|n| (1..=RAIL_SAFE_INTEGER).contains(&n))
            && self
                .node_id
                .is_none_or(|n| (1..=RAIL_SAFE_INTEGER).contains(&n))
            && self
                .original_node_id
                .is_none_or(|n| (1..=RAIL_SAFE_INTEGER).contains(&n))
            && self
                .object_id
                .is_none_or(|n| (1..=RAIL_SAFE_INTEGER).contains(&n))
            && self
                .selection_project_id
                .is_none_or(|n| (1..=RAIL_SAFE_INTEGER).contains(&n))
            && self
                .selection_group_id
                .is_none_or(|n| (1..=RAIL_SAFE_INTEGER).contains(&n))
            && self.count.is_none_or(|n| n <= RAIL_SAFE_INTEGER)
            && self.changed_count.is_none_or(|n| n <= RAIL_SAFE_INTEGER)
    }
}

pub(crate) fn rail_diagnostic_line(record: Option<serde_json::Value>) -> Result<String, String> {
    let invalid = || RAIL_DIAGNOSTIC_ERROR.to_string();
    let value = record.ok_or_else(invalid)?;
    // Serde unit enums also accept externally tagged maps; IPC permits scalar fields only.
    let object = value.as_object().ok_or_else(invalid)?;
    if object.values().any(|v| v.is_object() || v.is_array()) {
        return Err(invalid());
    }
    let record: RailDiagnosticRecord = serde_json::from_value(value).map_err(|_| invalid())?;
    if !record.validate() {
        return Err(invalid());
    }
    let json = serde_json::to_string(&record).map_err(|_| invalid())?;
    if json.len() > 2048 {
        return Err(invalid());
    }
    Ok(format!("[rail-diag]{json}"))
}

pub(crate) fn append_rail_diagnostic_to(
    record: Option<serde_json::Value>,
    logger: &dyn log::Log,
) -> Result<(), String> {
    let line = rail_diagnostic_line(record)?;
    let metadata = log::Metadata::builder()
        .level(log::Level::Debug)
        .target("agentscommander_lib::commands::config")
        .build();
    if logger.enabled(&metadata) {
        logger.log(
            &log::Record::builder()
                .metadata(metadata)
                .args(format_args!("{line}"))
                .build(),
        );
    }
    Ok(())
}

/// ACK covers validation and dispatch, not file write/fsync. RUST_LOG must allow Debug.
#[tauri::command]
pub fn append_rail_diagnostic(record: Option<serde_json::Value>) -> Result<(), String> {
    append_rail_diagnostic_to(record, log::logger())
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

/// #3036 — sorted snapshot of the error-popup types muted for this process.
/// Memory only: a new process starts empty; a webview reload keeps it.
#[tauri::command]
pub fn get_error_popup_mutes() -> Vec<String> {
    crate::logging::error_sink().muted_snapshot()
}

/// #3036 — mute or unmute future popups for one captured `typeId`. Unknown IDs
/// are rejected without mutation; returns the sorted muted snapshot.
#[tauri::command]
pub fn set_error_popup_muted(type_id: String, muted: bool) -> Result<Vec<String>, String> {
    crate::logging::error_sink().set_muted(&type_id, muted)
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
    /// #2716 (B3, option C) - read-only disclosure that the agents file could
    /// not be read, so no agent session may start. Same stance as
    /// `overlay_owns_agents`: never a field a client draft can carry.
    pub agents_layer_unreadable: bool,
    /// #2716 (B3, option C) - the unreadable agents file's path, for the notice.
    pub agents_file_path: Option<String>,
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

/// #2736 - per catalog entry: installed (PATH only, no process spawn), the
/// code-only tested level and this host's install command. Read-only; an
/// unavailable persisted catalog is `Err` exactly as `get_coding_agent_catalog`.
#[tauri::command]
pub async fn get_coding_agent_welcome_status(
    settings: State<'_, SettingsState>,
) -> Result<Vec<crate::config::coding_agents_catalog::CodingAgentWelcomeStatus>, String> {
    coding_agent_welcome_status_inner(settings.inner()).await
}

/// #2736 - shared by the Tauri command and the WebSocket router. Takes one
/// settings snapshot and releases the async settings lock BEFORE any filesystem
/// work (mirrors `coding_agent_catalog_inner`). The embedded default is NEVER
/// substituted; never seeds, refreshes or writes.
pub async fn coding_agent_welcome_status_inner(
    settings: &SettingsState,
) -> Result<Vec<crate::config::coding_agents_catalog::CodingAgentWelcomeStatus>, String> {
    coding_agent_welcome_status_inner_with(settings, |snapshot| {
        crate::config::coding_agents_catalog::load_catalog_for_settings(snapshot)
            .map_err(|unavailable| unavailable.to_string())
    })
    .await
}

/// Testable twin: `load` carries ALL filesystem work, so a test can observe
/// the settings lock state at that step.
async fn coding_agent_welcome_status_inner_with(
    settings: &SettingsState,
    load: impl FnOnce(
        &AppSettings,
    ) -> Result<
        Vec<crate::config::coding_agents_catalog::CodingAgentDefinition>,
        String,
    >,
) -> Result<Vec<crate::config::coding_agents_catalog::CodingAgentWelcomeStatus>, String> {
    let snapshot = settings.read().await.clone();
    let catalog = load(&snapshot)?;
    Ok(crate::config::coding_agents_catalog::welcome_status_for(
        &catalog,
    ))
}

/// #2736 - run the catalog entry's install command for THIS host with no
/// visible window. Returns as soon as the install task is spawned; the outcome
/// arrives as the `coding_agent_install_finished` event.
#[tauri::command]
pub async fn install_coding_agent(
    app: AppHandle,
    settings: State<'_, SettingsState>,
    key: String,
) -> Result<(), String> {
    install_coding_agent_inner(&app, settings.inner(), key).await
}

/// #2736 - shared by the Tauri command and the WebSocket router. Never awaits
/// the install itself.
pub async fn install_coding_agent_inner(
    app: &AppHandle,
    settings: &SettingsState,
    key: String,
) -> Result<(), String> {
    install_coding_agent_inner_with(app, settings, key, |snapshot| {
        crate::config::coding_agents_catalog::load_catalog_for_settings(snapshot)
            .map_err(|unavailable| unavailable.to_string())
    })
    .await
}

/// Testable twin: `load` carries the catalog read, so a test can supply a
/// fixture catalog without writing one to disk.
async fn install_coding_agent_inner_with(
    app: &AppHandle,
    settings: &SettingsState,
    key: String,
    load: impl FnOnce(
        &AppSettings,
    ) -> Result<
        Vec<crate::config::coding_agents_catalog::CodingAgentDefinition>,
        String,
    >,
) -> Result<(), String> {
    let snapshot = settings.read().await.clone();
    let catalog = load(&snapshot)?;
    let entry = catalog
        .iter()
        .find(|entry| entry.key == key)
        .ok_or_else(|| format!("unknown coding agent '{key}'"))?;
    let command = entry
        .install_commands
        .as_ref()
        .map(crate::config::coding_agents_catalog::resolve_install_command)
        .ok_or_else(|| format!("no install command is available for '{key}' on this platform"))?
        .to_string();
    let cwd = install_cwd_for(&snapshot);
    crate::agent_update::start_agent_install(app, key, command, cwd)
}

/// #2736 - the install cwd, same precedence the updater uses. Pure over the
/// snapshot so test 15 can assert the precedence without spawning anything.
fn install_cwd_for(snapshot: &AppSettings) -> PathBuf {
    crate::config::coding_agents_catalog::primary_project_root(snapshot)
        .or_else(crate::config::config_dir)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
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
    update_settings_with_hotkey_registration(
        app,
        settings.inner(),
        new_settings,
        crate::screenshot::register_configured_hotkey,
    )
    .await
}

async fn update_settings_with_hotkey_registration(
    app: AppHandle,
    settings: &SettingsState,
    new_settings: AppSettings,
    register_hotkey: fn(&AppHandle, &str) -> Result<(), String>,
) -> Result<(), String> {
    let tracker = app
        .try_state::<crate::pty::manager::ResponseCloseState>()
        .map(|state| state.inner().clone());
    let (saved, changed) =
        persist_protected_settings_update_with_publication(settings, new_settings, |written| {
            if let Some(tracker) = &tracker {
                tracker.apply_settings(
                    written.response_close_enabled,
                    written.response_close_idle_seconds,
                    std::time::Instant::now(),
                );
            }
        })
        .await?;
    emit_room_idle_confirmation_updated(&app, changed);
    purge_sessions_after_settings_update(&saved).await;
    // #714 re-register the (possibly changed) hotkey. Syntax was already validated
    // before persistence; an OS-level registration conflict must NOT turn this
    // successfully-persisted save into an Err. Surface it as a visible event.
    if let Err(e) = register_hotkey(&app, &saved.screenshot_capture_hotkey) {
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
    save_settings_draft_with_hotkey_registration(
        app,
        settings.inner(),
        draft,
        crate::screenshot::register_configured_hotkey,
    )
    .await
}

async fn save_settings_draft_with_hotkey_registration(
    app: AppHandle,
    settings: &SettingsState,
    draft: AppSettings,
    register_hotkey: fn(&AppHandle, &str) -> Result<(), String>,
) -> Result<(), String> {
    let tracker = app
        .try_state::<crate::pty::manager::ResponseCloseState>()
        .map(|state| state.inner().clone());
    let (saved, events) =
        persist_settings_draft_update_with_publication(settings, draft, |written| {
            if let Some(tracker) = &tracker {
                tracker.apply_settings(
                    written.response_close_enabled,
                    written.response_close_idle_seconds,
                    std::time::Instant::now(),
                );
            }
        })
        .await?;
    emit_room_idle_confirmation_updated(&app, events.room_idle_confirmation_changed);
    purge_sessions_after_settings_update(&saved).await;
    // #714 same non-failing hotkey re-registration as update_settings: persisted
    // save stays successful even if the OS refuses the hotkey.
    if let Err(e) = register_hotkey(&app, &saved.screenshot_capture_hotkey) {
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
    app: AppHandle,
    settings: State<'_, SettingsState>,
    expected: bool,
    enabled: bool,
) -> Result<(), String> {
    let tracker = app
        .try_state::<crate::pty::manager::ResponseCloseState>()
        .map(|state| state.inner().clone());
    set_terminal_snapshots_enabled_with_publication(
        settings.inner(),
        expected,
        enabled,
        |written| {
            if let Some(tracker) = &tracker {
                tracker.apply_settings(
                    written.response_close_enabled,
                    written.response_close_idle_seconds,
                    std::time::Instant::now(),
                );
            }
        },
    )
    .await
}

/// Shared #1173 owner used by native Tauri and browser WebSocket transports.
/// Whole-settings writers preserve this field and cannot call this owner
/// without an explicit expected value.
// Compatibility entry point for tests and dormant callers without an app tracker.
#[allow(dead_code)]
pub(crate) async fn set_terminal_snapshots_enabled_inner(
    settings: &SettingsState,
    expected: bool,
    enabled: bool,
) -> Result<(), String> {
    set_terminal_snapshots_enabled_with_publication(settings, expected, enabled, |_| {}).await
}

pub(crate) async fn set_terminal_snapshots_enabled_with_publication(
    settings: &SettingsState,
    expected: bool,
    enabled: bool,
    mut publish: impl FnMut(&AppSettings),
) -> Result<(), String> {
    let mut guard = settings.write().await;
    let written = crate::config::settings::compare_and_set_terminal_snapshots_enabled(
        &guard, expected, enabled,
    )?;
    publish(&written);
    *guard = written;
    Ok(())
}

// Compatibility entry point for tests and dormant callers without an app tracker.
#[allow(dead_code)]
pub(crate) async fn persist_protected_settings_update(
    settings: &SettingsState,
    new_settings: AppSettings,
) -> Result<(AppSettings, bool), String> {
    persist_protected_settings_update_with_saver(settings, new_settings, save_settings).await
}

pub(crate) async fn persist_protected_settings_update_with_publication(
    settings: &SettingsState,
    new_settings: AppSettings,
    publish: impl FnMut(&AppSettings),
) -> Result<(AppSettings, bool), String> {
    persist_protected_settings_update_with_saver_and_publication(
        settings,
        new_settings,
        save_settings,
        publish,
    )
    .await
}

async fn persist_protected_settings_update_with_saver(
    settings: &SettingsState,
    new_settings: AppSettings,
    save: impl FnOnce(&AppSettings) -> Result<AppSettings, String>,
) -> Result<(AppSettings, bool), String> {
    persist_protected_settings_update_with_saver_and_publication(
        settings,
        new_settings,
        save,
        |_| {},
    )
    .await
}

/// Publish the effective saved value before adoption under settings ownership.
/// The synchronous callback must not acquire the settings lock.
async fn persist_protected_settings_update_with_saver_and_publication(
    settings: &SettingsState,
    new_settings: AppSettings,
    save: impl FnOnce(&AppSettings) -> Result<AppSettings, String>,
    mut publish: impl FnMut(&AppSettings),
) -> Result<(AppSettings, bool), String> {
    let mut s = settings.write().await;
    let current = s.clone();
    let candidate = build_protected_settings_candidate(&current, new_settings)?;
    let written = save(&candidate)?;
    let changed = current.room_idle_confirmation_seconds != written.room_idle_confirmation_seconds;
    publish(&written);
    *s = written.clone();
    Ok((written, changed))
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

// Compatibility entry point for tests and dormant callers without an app tracker.
#[allow(dead_code)]
pub(crate) async fn persist_settings_draft_update(
    settings: &SettingsState,
    draft: AppSettings,
) -> Result<(AppSettings, SettingsDraftUpdateEvents), String> {
    persist_settings_draft_update_with_saver(settings, draft, save_settings).await
}

pub(crate) async fn persist_settings_draft_update_with_publication(
    settings: &SettingsState,
    draft: AppSettings,
    publish: impl FnMut(&AppSettings),
) -> Result<(AppSettings, SettingsDraftUpdateEvents), String> {
    persist_settings_draft_update_with_saver_and_publication(
        settings,
        draft,
        save_settings,
        publish,
    )
    .await
}

async fn persist_settings_draft_update_with_saver(
    settings: &SettingsState,
    draft: AppSettings,
    save: impl FnOnce(&AppSettings) -> Result<AppSettings, String>,
) -> Result<(AppSettings, SettingsDraftUpdateEvents), String> {
    persist_settings_draft_update_with_saver_and_publication(settings, draft, save, |_| {}).await
}

/// Publish the effective saved value before adoption under settings ownership.
/// The synchronous callback must not acquire the settings lock.
async fn persist_settings_draft_update_with_saver_and_publication(
    settings: &SettingsState,
    mut draft: AppSettings,
    save: impl FnOnce(&AppSettings) -> Result<AppSettings, String>,
    mut publish: impl FnMut(&AppSettings),
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
    draft.coding_agent_identity_physical = current.coding_agent_identity_physical.clone();
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
    let mut events = settings_draft_update_events(&current, &draft);
    let written = save(&draft)?;
    events.room_idle_confirmation_changed =
        current.room_idle_confirmation_seconds != written.room_idle_confirmation_seconds;
    publish(&written);
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
            "[settings] Failed to purge sessions outside current projectPaths after settings \
                update: {}",
            e
        );
    }
}

async fn purge_sessions_after_settings_update_in_dir(
    saved: &AppSettings,
    dir: &Path,
) -> Result<(), String> {
    let retention_paths =
        crate::config::sessions_persistence::session_retention_project_paths(saved)?;
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

    let mut before_profiles = before.coding_agent_profiles.clone();
    let mut after_profiles = after.coding_agent_profiles.clone();
    for profiles in [&mut before_profiles, &mut after_profiles] {
        for cells in profiles.profiles_by_agent.values_mut() {
            for cell in cells.values_mut() {
                cell.id_hash = None;
                cell.id_hash_debug = None;
            }
        }
    }

    SettingsDraftUpdateEvents {
        room_idle_confirmation_changed: false,
        // #592/#597 - the drift content-hash fingerprints the EFFECTIVE command
        // (agent base command + cell command, via compose_effective_command), so a
        // bare agent-base-command edit (e.g. `claude` -> `claude-amp -c ...`) is real
        // drift even when the profile cells are untouched. It is neither a
        // coding_agent_profiles change nor an env change, so without folding it in
        // here the SettingsModal save path emits NO refresh event and the outdated
        // badge never updates. coding_agent_profiles_updated drives
        // refreshProfileOutdated in the sidebar, so route base-command edits through it.
        profiles_changed: before_profiles != after_profiles
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

fn emit_room_idle_confirmation_updated(app: &AppHandle, changed: bool) {
    if !changed {
        return;
    }
    let payload = serde_json::json!({});
    if let Err(error) = app.emit("room_idle_confirmation_updated", payload.clone()) {
        log::warn!("Failed to emit room idle confirmation update: {}", error);
    }
    if let Some(broadcaster) = app.try_state::<WsBroadcaster>() {
        broadcaster.broadcast_event("room_idle_confirmation_updated", &payload);
    }
}

fn emit_settings_draft_update_events(app: &AppHandle, events: &SettingsDraftUpdateEvents) {
    if events.profiles_changed {
        // #592/#597: the SettingsModal "Save" persists profile cells AND agent base
        // commands through save_settings_draft (NOT update_coding_agent_profiles), so
        // THIS is the emit point the user's normal edit flow actually reaches. Kept at
        // debug to confirm the refresh fired without spamming app.log.
        log::debug!(
            "[profile-hash] profile-save (save_settings_draft): coding-agent config changed; \
                emitting coding_agent_profiles_updated"
        );
        let _ = app.emit("coding_agent_profiles_updated", serde_json::json!({}));
    }

    for agent_id in &events.env_agent_ids {
        log::debug!(
            "[profile-hash] profile-save (save_settings_draft): agent env changed for {}; emitting \
                coding_agent_env_settings_updated",
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
    persist_coding_agent_env_settings_update_with_saver(
        settings,
        agent_id,
        envs,
        isolated_home,
        save_settings,
    )
    .await
}

async fn persist_coding_agent_env_settings_update_with_saver(
    settings: &SettingsState,
    agent_id: &str,
    envs: Vec<CodingAgentEnv>,
    isolated_home: bool,
    save: impl FnOnce(&AppSettings) -> Result<AppSettings, String>,
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
    let written = save(&candidate)?;
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
    pub scope_faults: Vec<ScopeFault>,
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
    crate::session::selection::run_owned_selection_read_timed(
        "preview_coding_agent_profile_selection",
        move || async move {
            preview_coding_agent_profile_selection_inner(&session_mgr, &settings, request).await
        },
    )
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
        let expected = candidate
            .expected
            .clone()
            .ok_or("invalid selection expectation")?;
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
                use crate::config::coding_agent_profiles::SelectionLockTransition;
                if outcome.lock_transition == Some(SelectionLockTransition::UnlockedToLocked) {
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
    expected: Option<crate::config::coding_agent_profiles::ReplicaSelectionExpectation>,
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
    fn new(code: ScopeFaultCode, path: &Path) -> Self {
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
fn scope_fault_code(fault: crate::config::replica_identity::ReplicaConfigFault) -> ScopeFaultCode {
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
struct ProfileTargetEnumeration {
    anchor_dir: PathBuf,
    anchor_matrix_dir: PathBuf,
    candidates: Vec<ProfileCandidate>,
    warnings: Vec<String>,
    scope_faults: Vec<ScopeFault>,
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
    faults: Vec<ScopeFault>,
    complete: bool,
}

impl CandidateDirs {
    fn new() -> Self {
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
            out.mark_incomplete(
                format!("Failed to read room '{}': {}", wg_dir.display(), e),
                ScopeFault::new(ScopeFaultCode::FolderUnreadable, wg_dir),
            );
            return;
        }
    };
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(e) => {
                out.mark_incomplete(
                    format!(
                        "Failed to read an entry in room '{}': {}",
                        wg_dir.display(),
                        e
                    ),
                    ScopeFault::new(ScopeFaultCode::FolderUnreadable, wg_dir),
                );
                continue;
            }
        };
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(e) => {
                out.mark_incomplete(
                    format!(
                        "Failed to inspect an entry in room '{}': {}",
                        wg_dir.display(),
                        e
                    ),
                    ScopeFault::new(ScopeFaultCode::FolderUnreadable, wg_dir),
                );
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
            out.mark_incomplete(
                format!("Failed to read workspace '{}': {}", ac_root.display(), e),
                ScopeFault::new(ScopeFaultCode::FolderUnreadable, ac_root),
            );
            return;
        }
    };
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(e) => {
                out.mark_incomplete(
                    format!(
                        "Failed to read an entry in workspace '{}': {}",
                        ac_root.display(),
                        e
                    ),
                    ScopeFault::new(ScopeFaultCode::FolderUnreadable, ac_root),
                );
                continue;
            }
        };
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(e) => {
                out.mark_incomplete(
                    format!(
                        "Failed to inspect an entry in workspace '{}': {}",
                        ac_root.display(),
                        e
                    ),
                    ScopeFault::new(ScopeFaultCode::FolderUnreadable, ac_root),
                );
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
    #[serde(skip_serializing_if = "Option::is_none")]
    configuration_ref: Option<crate::config::settings::ConfigurationRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    profile_override_source: Option<String>,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    configuration_ref: Option<crate::config::settings::ConfigurationRef>,
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
                configuration_ref: candidate
                    .expected
                    .as_ref()
                    .and_then(|expected| expected.configuration_ref.clone()),
                profile_override_source: candidate
                    .expected
                    .as_ref()
                    .and_then(|expected| expected.profile_override_source.clone()),
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
            configuration_ref: default.configuration_ref.clone(),
        }),
    };
    sha256_hex(&tuple)
}

/// Prepared source approvals bind the normalized ticket and marker as well as
/// the legacy target fingerprint. This companion has no IPC caller in P03.
#[allow(dead_code)]
fn source_selection_fingerprint(
    target_fingerprint: &str,
    ticket: &crate::config::settings::SourceSelectionTicket,
    marker: Option<&str>,
) -> Result<String, String> {
    sha256_hex(&("source-selection-v1", target_fingerprint, ticket, marker))
}

/// Prepare a manual setter from a freshly authorized physical source. Publication
/// remains a separate guarded step so no source IO enters the config layer.
#[allow(dead_code)]
async fn prepare_source_profile_override(
    settings: &SettingsState,
    replica_path: &Path,
    profile: &str,
) -> Result<
    (
        crate::config::coding_agent_profiles::ReplicaSelectionExpectation,
        crate::config::coding_agent_profiles::PreparedSourceSelection,
    ),
    String,
> {
    use crate::config::coding_agent_profiles::{
        read_replica_selection_state, resolve_profile, PreparedSourceSelection,
        ProfileResolutionRequest, ReplicaSelectionPair,
    };
    use crate::config::settings::{agent_file_projection, configuration_identity, pair_identity};
    let profile = crate::config::settings::normalize_profile_letter(profile)
        .ok_or("Profile must be a single letter A through Z")?;
    let host = settings
        .current_snapshot()
        .await
        .map_err(|e| e.to_string())?;
    let validated = crate::config::coding_agent_profiles::validate_profile_selection_agent_path(
        &host,
        replica_path,
    )?;
    let expected = read_replica_selection_state(&validated.launch_path).expectation()?;
    let mut reference = expected
        .configuration_ref
        .clone()
        .ok_or("source reference required")?;
    let snapshot = read_agent_file_source_inner(settings, reference.source.clone()).await?;
    let hint = reference
        .legacy_entry_hint
        .as_deref()
        .ok_or("source hint required")?;
    let matches: Vec<_> = snapshot
        .rows
        .iter()
        .filter(|row| row.entry_hint == hint)
        .collect();
    if matches.len() != 1
        || !matches[0].launchable
        || matches[0].base_hash.as_deref() != Some(reference.calculated_id.as_str())
    {
        return Err("stored source selection is unavailable or changed".into());
    }
    let scoped = agent_file_projection(&snapshot)?;
    let agents: Vec<_> = scoped
        .agents
        .iter()
        .filter(|agent| agent.id == hint)
        .collect();
    if agents.len() != 1
        || configuration_identity(&agents[0].command, &agents[0].envs) != reference.calculated_id
    {
        return Err("stored source identity is missing or ambiguous".into());
    }
    let resolution = resolve_profile(
        &scoped,
        ProfileResolutionRequest {
            coding_agent_id: hint,
            launch_path: None,
            agent_matrix_name: None,
            requested_profile: Some(&profile),
            requested_profile_authoritative: true,
        },
    );
    reference.profile_calculated_id = Some(pair_identity(
        &agents[0].command,
        &agents[0].envs,
        &resolution.cell,
    ));
    if reference.requested_profile.is_some() {
        reference.requested_profile = Some(profile.clone());
    }
    let prepared = PreparedSourceSelection {
        pair: ReplicaSelectionPair {
            coding_agent_id: hint.to_owned(),
            requested_profile: profile,
        },
        configuration_ref: reference,
        profile_override_source: Some("manual".into()),
    };
    Ok((expected, prepared))
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
        "conflictDecisionRequired: protected replicas are in scope; choose unlockedOnly or \
            forceReviewed"
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
    } else if message.contains("'tooling' must be a JSON object in the tracked config file") {
        // #2816 (C3) - the pair's cleanup rejects a malformed TRACKED `tooling`
        // before the closure runs, so the pre-C3 `invalid replica selection ...`
        // text no longer reaches here. Same classification, new spelling. The
        // state-side text is deliberately NOT matched: see consequence 4.
        "invalidSelectionState"
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
    crate::session::selection::run_owned_selection_read_timed(
        "preview_selection_lock_removal",
        move || async move {
            preview_selection_lock_removal_inner(&session_mgr, &settings, request).await
        },
    )
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
        scope_faults: enumeration.scope_faults,
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
                let expected = candidate
                    .expected
                    .clone()
                    .ok_or("invalid selection expectation")?;
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
    crate::session::selection::run_owned_selection_read_timed(
        "get_replica_selection_default",
        move || async move { get_replica_selection_default_inner(&settings, request).await },
    )
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
        configuration_ref: None,
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

/// #2542 - narrow owner of the one-step reorder: moves `id` to `target_index`
/// when `expected_ids` still matches the effective order. Mutates only the
/// `agents` vector under the settings write guard and persists through the
/// preserving `save_settings`.
#[tauri::command]
pub async fn reorder_coding_agent<R: tauri::Runtime>(
    app: AppHandle<R>,
    settings: State<'_, SettingsState>,
    id: String,
    expected_ids: Vec<String>,
    target_index: usize,
) -> Result<Vec<String>, String> {
    reorder_coding_agent_command_with_saver(
        &app,
        settings.inner(),
        id,
        expected_ids,
        target_index,
        save_settings,
    )
    .await
}

/// Shared Tauri-command body for the reorder; emits the move's event payload
/// after the inner has released the settings write guard.
pub(crate) async fn reorder_coding_agent_command_with_saver<R: tauri::Runtime>(
    app: &AppHandle<R>,
    settings: &SettingsState,
    id: String,
    expected_ids: Vec<String>,
    target_index: usize,
    save: impl FnOnce(&AppSettings) -> Result<AppSettings, String>,
) -> Result<Vec<String>, String> {
    let ids =
        reorder_coding_agent_inner_with_saver(settings, &id, &expected_ids, target_index, save)
            .await?;
    let _ = app.emit(
        "coding_agent_settings_updated",
        move_coding_agent_event_payload(&id),
    );
    Ok(ids)
}

/// The shared inner owner of the reorder, used by the Tauri command and the
/// browser WebSocket route.
pub(crate) async fn reorder_coding_agent_inner_with_saver(
    settings: &SettingsState,
    id: &str,
    expected_ids: &[String],
    target_index: usize,
    save: impl FnOnce(&AppSettings) -> Result<AppSettings, String>,
) -> Result<Vec<String>, String> {
    let written = persist_narrow_settings_update_fallible_with_saver(
        settings,
        |candidate| reorder_registered_agent(candidate, id, expected_ids, target_index).map(|_| ()),
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
            "[log-level] RUST_LOG override active; backend frozen, applying '{level}' to frontend \
                only"
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

// E2432 P02: profile orchestration stays above settings' physical primitives.
fn prepare_agent_file_tickets(
    snapshot: &mut crate::config::settings::SelectedAgentFileSnapshot,
) -> Result<(), String> {
    use crate::config::coding_agent_profiles::{resolve_profile, ProfileResolutionRequest};
    use crate::config::settings::{
        agent_file_projection, configuration_identity, pair_identity, ConfigurationRef,
        SourceAvailability, SourceSelectionTicket,
    };
    if snapshot.availability != SourceAvailability::Available {
        return Ok(());
    }
    let settings = agent_file_projection(snapshot)?;
    for agent in &settings.agents {
        let mut tickets = BTreeMap::new();
        let mut requests = vec![("default".to_owned(), None)];
        if let Some(cells) = settings
            .coding_agent_profiles
            .profiles_by_agent
            .get(&agent.id)
        {
            requests.extend(
                cells
                    .keys()
                    .map(|letter| (letter.clone(), Some(letter.clone()))),
            );
        }
        if !requests.iter().any(|(key, _)| key == "A") {
            requests.push(("A".into(), Some("A".into())));
        }
        for (key, requested_profile) in requests {
            let resolution = resolve_profile(
                &settings,
                ProfileResolutionRequest {
                    coding_agent_id: &agent.id,
                    launch_path: None,
                    agent_matrix_name: None,
                    requested_profile: requested_profile.as_deref(),
                    requested_profile_authoritative: true,
                },
            );
            tickets.insert(
                key,
                SourceSelectionTicket {
                    configuration_ref: ConfigurationRef {
                        version: 1,
                        source: snapshot.source.clone(),
                        calculated_id: configuration_identity(&agent.command, &agent.envs),
                        requested_profile,
                        profile_calculated_id: Some(pair_identity(
                            &agent.command,
                            &agent.envs,
                            &resolution.cell,
                        )),
                        legacy_entry_hint: Some(agent.id.clone()),
                    },
                    expected_source_revision: snapshot.revision.clone(),
                    catalog_context_revision: snapshot.context_revision.clone(),
                },
            );
        }
        snapshot.tickets.insert(agent.id.clone(), tickets);
    }
    Ok(())
}

pub(crate) async fn list_agent_file_sources_inner(
    settings: &SettingsState,
) -> Result<crate::config::settings::AgentFileSources, String> {
    use crate::config::settings::{AgentFileReadReply, AgentFileReadRequest};
    match settings
        .read_agent_files(AgentFileReadRequest::List)
        .await?
    {
        AgentFileReadReply::List(sources) => Ok(sources),
        AgentFileReadReply::Read(_) => Err("sourceReplyMismatch".into()),
    }
}

pub(crate) async fn read_agent_file_source_inner(
    settings: &SettingsState,
    source: crate::config::settings::SourceRef,
) -> Result<crate::config::settings::SelectedAgentFileSnapshot, String> {
    use crate::config::settings::{AgentFileReadReply, AgentFileReadRequest};
    let mut snapshot = match settings
        .read_agent_files(AgentFileReadRequest::Read(source))
        .await?
    {
        AgentFileReadReply::Read(snapshot) => snapshot,
        AgentFileReadReply::List(_) => return Err("sourceReplyMismatch".into()),
    };
    prepare_agent_file_tickets(&mut snapshot)?;
    Ok(snapshot)
}

pub(crate) async fn save_agent_file_source_inner(
    settings: &SettingsState,
    source: crate::config::settings::SourceRef,
    expected_revision: crate::config::settings::SourceRevision,
    context_revision: String,
    edits: crate::config::settings::AgentFileEdits,
) -> Result<crate::config::settings::AgentFileSaveReply, String> {
    let mut result = crate::config::settings::persist_agent_file_edits(
        settings,
        source,
        expected_revision,
        context_revision,
        edits,
    )
    .await?;
    if let Some(snapshot) = result.snapshot.as_mut() {
        prepare_agent_file_tickets(snapshot)?;
    }
    // Match the existing settings snapshot boundary on both transports.
    if let Some(effective) = result.effective_settings.as_mut() {
        effective.root_token = None;
    }
    Ok(result)
}

pub(crate) async fn remember_agent_file_source_inner(
    settings: &SettingsState,
    menu: crate::config::settings::AgentFileSourceMenu,
    source: crate::config::settings::SourceRef,
) -> Result<crate::config::settings::AgentFileSourcePreferences, String> {
    use crate::config::settings::SettingsOwnerError;
    let report = settings
        .with_operation("remember_agent_file_source", move |current, guard| {
            crate::config::settings::remember_agent_file_locked(current, guard, menu, &source)
        })
        .await
        .map_err(|error| match error {
            SettingsOwnerError::Capture(_) => "sourceParticipantReadFailed: no files changed; \
                correct the participating file externally"
                .to_owned(),
            SettingsOwnerError::Compose(_) => "sourceParticipantInvalid: no files changed; \
                correct the participating file externally"
                .to_owned(),
            _ => "settingsOwnerUnavailable: no files changed".to_owned(),
        })?;
    finish_remember_agent_file_report(settings, report).await
}

async fn finish_remember_agent_file_report(
    settings: &SettingsState,
    report: crate::config::settings::SettingsOperationReport<
        crate::config::settings::AgentFileSourcePreferences,
    >,
) -> Result<crate::config::settings::AgentFileSourcePreferences, String> {
    use crate::config::settings::{OperationOutcome, SettingsAvailability};
    if report.outcome == OperationOutcome::PartialOrUnknown
        || (report.outcome == OperationOutcome::Committed
            && report.availability != SettingsAvailability::Ready)
    {
        return Err(
            "partialOrUnknown: preference publication may have occurred; reload actual settings"
                .into(),
        );
    }
    if report.outcome != OperationOutcome::Committed || report.value.is_none() {
        let reason = report.error.unwrap_or_default();
        let code = [
            "duplicateJsonMember",
            "invalidLegacyProfileOrphans",
            "staleSourceContext",
            "staleSettingsRevision",
            "staleProfileSource",
            "sourceAbsentOrForged",
            "invalidSourcePreferences",
        ]
        .into_iter()
        .find(|code| reason.contains(code))
        .unwrap_or("sourcePreferenceRejected");
        return Err(format!(
            "{code}: no files changed; correct the participating file externally or refresh the \
                selection"
        ));
    }
    let adopted = settings.current_snapshot().await.map_err(|_| {
        "partialOrUnknown: preferences were published but adopted settings are unavailable"
    })?;
    Ok(adopted.agent_file_source_by_menu.unwrap_or_default())
}

#[tauri::command]
pub async fn list_agent_file_sources(
    settings: State<'_, SettingsState>,
) -> Result<crate::config::settings::AgentFileSources, String> {
    list_agent_file_sources_inner(settings.inner()).await
}

#[tauri::command]
pub async fn read_agent_file_source(
    settings: State<'_, SettingsState>,
    source: crate::config::settings::SourceRef,
) -> Result<crate::config::settings::SelectedAgentFileSnapshot, String> {
    read_agent_file_source_inner(settings.inner(), source).await
}

#[tauri::command]
pub async fn save_agent_file_source(
    app: AppHandle,
    settings: State<'_, SettingsState>,
    source: crate::config::settings::SourceRef,
    expected_revision: crate::config::settings::SourceRevision,
    context_revision: String,
    edits: crate::config::settings::AgentFileEdits,
) -> Result<crate::config::settings::AgentFileSaveReply, String> {
    let reply = save_agent_file_source_inner(
        settings.inner(),
        source,
        expected_revision,
        context_revision,
        edits,
    )
    .await?;
    emit_agent_file_save_events(&app, &reply);
    Ok(reply)
}

pub(crate) fn emit_agent_file_save_events(
    app: &AppHandle,
    reply: &crate::config::settings::AgentFileSaveReply,
) {
    if reply.outcome != crate::config::settings::AgentFileSaveOutcome::Committed
        || reply.effective_settings.is_none()
    {
        return;
    }
    let catalog_only = reply
        .diagnostics
        .iter()
        .any(|diagnostic| diagnostic.code == "effectiveSettingsUnchanged");
    // Catalogue publication adopts no owner settings; its actual reply is the receipt.
    if catalog_only {
        return;
    }
    let payload = serde_json::json!({});
    for event in [
        "coding_agent_settings_updated",
        "coding_agent_profiles_updated",
    ] {
        let _ = app.emit(event, &payload);
        if let Some(broadcaster) = app.try_state::<WsBroadcaster>() {
            broadcaster.broadcast_event(event, &payload);
        }
    }
}

#[tauri::command]
pub async fn remember_agent_file_source(
    settings: State<'_, SettingsState>,
    menu: crate::config::settings::AgentFileSourceMenu,
    source: crate::config::settings::SourceRef,
) -> Result<crate::config::settings::AgentFileSourcePreferences, String> {
    remember_agent_file_source_inner(settings.inner(), menu, source).await
}

#[cfg(test)]
mod tests;
