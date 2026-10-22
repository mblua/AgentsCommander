use uuid::Uuid;

use crate::config::instance_artifacts::DEBUG_LOGS_FILE_NAME;

// #3083: closed diagnostic vocabulary; no client-selected target or text.
pub(crate) const RAIL_DIAGNOSTIC_ERROR: &str = "invalid rail diagnostic";
pub(crate) const RAIL_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

#[derive(Debug, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum RailSurface {
    Embedded,
    Sidebar,
    Store,
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum RailKind {
    Custom,
    System,
    Nonstop,
    Ungrouped,
    Project,
    Favorites,
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum RailPhase {
    Arming,
    Dragging,
    Saving,
    Idle,
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum RailOutcome {
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
pub(crate) enum RailTag {
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
pub(crate) enum RailEvent {
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
pub(crate) struct RailDiagnosticRecord {
    version: u8,
    window_id: Uuid,
    seq: u64,
    pub(crate) at_ms: f64,
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
    pub(crate) duration_ms: Option<f64>,
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
    pub(crate) fn validate(&self) -> bool {
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
