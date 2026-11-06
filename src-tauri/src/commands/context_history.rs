//! Context-history IPC (#3532 P05): the desktop Tauri edge of the report.
//!
//! Four read-only commands share the report core with the CLI: validate in
//! memory, check the project against this app's Settings registration and the
//! live session snapshot, then verify the project and read one retained
//! snapshot off the async workers. Errors are `{code, message}` without paths.
//! [`dispatch`] routes the four names so `invoke_handler` does not grow.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use tauri::State;
use uuid::Uuid;

use crate::config::settings::SettingsState;
use crate::context_history::config::load_history_config;
use crate::context_history::observation::ScopeResolution;
use crate::context_history::report::{
    build_agent_detail, build_changes_page, build_report, sidebar_means, validate_sidebar_ids,
    AgentDetailResponse, ChangesPageResponse, ReportContext, ReportError, ReportQuery,
    ReportResponse, SidebarMeansResponse, SidebarScope, SidebarScopeKind, SidebarTarget,
};
use crate::context_history::store::{read_history, ReadAttempt, StoreError};
use crate::context_history::{HistoryRecord, TeamAttribution};
use crate::history_runtime::project::{align_project_paths, plain_path, verified_project};
use crate::history_runtime::scopes::{ResolvedScope, SidebarScopeError};
use crate::history_runtime::HistoryScopeState;
use crate::path_identity::paths_equivalent;

/// Stable error codes of the context-history family.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub(crate) enum HistoryErrorCode {
    InvalidProject,
    InvalidQuery,
    InvalidCursor,
    UnsupportedVersion,
    StorageLimitMismatch,
    Busy,
    Timeout,
    Corrupt,
    ReadFailed,
    RecoveryRequired,
}

/// A refused request; the message never carries paths, argv or secrets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct HistoryIpcError {
    code: HistoryErrorCode,
    message: String,
}

type IpcResult<T> = Result<T, HistoryIpcError>;

fn ipc_error(code: HistoryErrorCode, message: &str) -> HistoryIpcError {
    HistoryIpcError {
        code,
        message: message.to_string(),
    }
}

fn report_error(error: ReportError) -> HistoryIpcError {
    let code = match error {
        ReportError::InvalidCursor(_) => HistoryErrorCode::InvalidCursor,
        ReportError::InvalidQuery(_) | ReportError::NotInSelection(_) => {
            HistoryErrorCode::InvalidQuery
        }
    };
    ipc_error(code, &error.to_string())
}

fn store_error(error: StoreError) -> HistoryIpcError {
    use HistoryErrorCode as Code;
    let (code, message) = match error {
        StoreError::UnsupportedVersion(_) => {
            (Code::UnsupportedVersion, "unsupported history version")
        }
        StoreError::StorageLimitMismatch(_) => {
            (Code::StorageLimitMismatch, "storage limits differ")
        }
        StoreError::RetryableBusy => (Code::Busy, "history is busy; retry"),
        StoreError::DeadlineExceeded => (Code::Timeout, "history read timed out"),
        StoreError::Corrupt(_) => (Code::Corrupt, "history database is corrupt"),
        StoreError::RecoveryRequired { .. } => (
            Code::RecoveryRequired,
            "close other AgentsCommander instances of this project and retry",
        ),
        _ => (Code::ReadFailed, "history could not be read"),
    };
    ipc_error(code, message)
}

fn invalid_project() -> HistoryIpcError {
    ipc_error(
        HistoryErrorCode::InvalidProject,
        "project is not a registered AC project",
    )
}

fn invalid_query(message: &str) -> HistoryIpcError {
    ipc_error(HistoryErrorCode::InvalidQuery, message)
}

fn read_failed() -> HistoryIpcError {
    ipc_error(HistoryErrorCode::ReadFailed, "history could not be read")
}

/// Parse a request in memory; a malformed body is `InvalidQuery`.
fn parse<T: DeserializeOwned>(request: serde_json::Value) -> IpcResult<T> {
    serde_json::from_value(request).map_err(|_| invalid_query("malformed request"))
}

fn parse_uuid(text: &str) -> IpcResult<Uuid> {
    Uuid::parse_str(text).map_err(|_| invalid_query("malformed session id"))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ChangesRequest {
    query: ReportQuery,
    logical_session_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SidebarMeansRequest {
    project_path: String,
    runtime_session_ids: Vec<String>,
}

/// The registered spellings equivalent to the requested project, compared in
/// memory with the P13 identity; none means the project is not registered.
async fn registered_paths(settings: &SettingsState, project: &str) -> IpcResult<Vec<PathBuf>> {
    let requested = Path::new(project);
    if !requested.is_absolute() {
        return Err(invalid_project());
    }
    let snapshot = settings.current_snapshot().await.map_err(|_error| {
        log::warn!("[context-history] settings snapshot unavailable for a history query");
        read_failed()
    })?;
    let requested = PathBuf::from(plain_path(project));
    let mut paths: Vec<PathBuf> = snapshot
        .project_paths
        .iter()
        .map(|registered| PathBuf::from(plain_path(registered)))
        .filter(|registered| paths_equivalent(registered, &requested))
        .collect();
    if paths.is_empty() {
        return Err(invalid_project());
    }
    paths.push(requested);
    Ok(paths)
}

/// The verified project's canonical text, retention and aligned records.
fn read_project(project: &str) -> IpcResult<(String, u32, Vec<HistoryRecord>)> {
    let verified = verified_project(Path::new(project)).map_err(|_| invalid_project())?;
    let canonical = verified
        .canonical
        .to_str()
        .ok_or_else(invalid_project)?
        .to_string();
    let retention_days = load_history_config(&verified.ac_root).config.retention_days;
    let mut records = match read_history(&verified.ac_root, ReadAttempt::ReadOnly) {
        Err(StoreError::RecoveryRequired { .. }) => {
            read_history(&verified.ac_root, ReadAttempt::AllowRecovery)
        }
        other => other,
    }
    .map_err(store_error)?;
    align_project_paths(&mut records, &verified.spellings, &canonical);
    Ok((canonical, retention_days, records))
}

/// Run `build` over one snapshot off the async workers.
async fn blocking<T, B>(project: String, build: B) -> IpcResult<T>
where
    T: Send + 'static,
    B: FnOnce(String, u32, Vec<HistoryRecord>) -> IpcResult<T> + Send + 'static,
{
    tokio::task::spawn_blocking(move || {
        let (canonical, retention_days, records) = read_project(&project)?;
        build(canonical, retention_days, records)
    })
    .await
    .map_err(|_error| {
        log::warn!("[context-history] history read task failed");
        read_failed()
    })?
}

fn now_utc_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// Validate a report-family query and its registration; returns the project.
async fn checked_query(settings: &SettingsState, query: &ReportQuery) -> IpcResult<String> {
    query.validate().map_err(report_error)?;
    registered_paths(settings, &query.project_path).await?;
    Ok(query.project_path.clone())
}

/// The query bound to the verified canonical project, as the CLI binds it.
fn bound(mut query: ReportQuery, canonical: String) -> ReportQuery {
    query.project_path = canonical;
    query
}

fn context(retention_days: u32, now: i64) -> ReportContext {
    ReportContext {
        retention_days,
        generated_at_utc_ms: now,
    }
}

#[tauri::command]
pub(crate) async fn get_context_history_report(
    request: serde_json::Value,
    settings: State<'_, SettingsState>,
) -> IpcResult<ReportResponse> {
    let query: ReportQuery = parse(request)?;
    let project = checked_query(&settings, &query).await?;
    let now = now_utc_ms();
    blocking(project, move |canonical, retention, records| {
        build_report(&records, &bound(query, canonical), context(retention, now))
            .map_err(report_error)
    })
    .await
}

#[tauri::command]
pub(crate) async fn get_context_history_agent_detail(
    request: serde_json::Value,
    settings: State<'_, SettingsState>,
) -> IpcResult<AgentDetailResponse> {
    let query: ReportQuery = parse(request)?;
    let agent_key = query
        .agent_key
        .clone()
        .ok_or_else(|| invalid_query("agentKey required"))?;
    let project = checked_query(&settings, &query).await?;
    let now = now_utc_ms();
    blocking(project, move |canonical, retention, records| {
        let query = bound(query, canonical);
        build_agent_detail(&records, &query, &agent_key, context(retention, now))
            .map_err(report_error)
    })
    .await
}

#[tauri::command]
pub(crate) async fn get_context_history_changes(
    request: serde_json::Value,
    settings: State<'_, SettingsState>,
) -> IpcResult<ChangesPageResponse> {
    let request: ChangesRequest = parse(request)?;
    let query = request.query;
    let agent_key = query
        .agent_key
        .clone()
        .ok_or_else(|| invalid_query("agentKey required"))?;
    let session_id = parse_uuid(&request.logical_session_id)?;
    let project = checked_query(&settings, &query).await?;
    let now = now_utc_ms();
    blocking(project, move |canonical, retention, records| {
        let query = bound(query, canonical);
        build_changes_page(
            &records,
            &query,
            &agent_key,
            session_id,
            context(retention, now),
        )
        .map_err(report_error)
    })
    .await
}

/// The sidebar scope of one session from the shared resolution; anything
/// not provably scoped is unknown, never an empty known scope.
fn sidebar_scope(resolved: Option<&ResolvedScope>) -> SidebarScope {
    let unknown = SidebarScope {
        kind: SidebarScopeKind::Unknown,
        agent_key: None,
        team_ids: Vec::new(),
    };
    let Some(ScopeResolution::Scoped(scope)) = resolved.map(|resolved| &resolved.scope) else {
        return unknown;
    };
    let kind = match scope.team_attribution {
        TeamAttribution::Known => SidebarScopeKind::Known,
        TeamAttribution::Ambiguous => SidebarScopeKind::Ambiguous,
        TeamAttribution::Unknown => SidebarScopeKind::Unknown,
    };
    SidebarScope {
        kind,
        agent_key: Some(scope.layout.agent_key.clone()),
        team_ids: scope.team_ids.clone(),
    }
}

#[tauri::command]
pub(crate) async fn get_context_history_sidebar_means(
    request: serde_json::Value,
    settings: State<'_, SettingsState>,
    scopes: State<'_, Arc<HistoryScopeState>>,
) -> IpcResult<SidebarMeansResponse> {
    let request: SidebarMeansRequest = parse(request)?;
    let ids = request
        .runtime_session_ids
        .iter()
        .map(|id| parse_uuid(id))
        .collect::<IpcResult<Vec<Uuid>>>()?;
    validate_sidebar_ids(&ids).map_err(report_error)?;
    let paths = registered_paths(&settings, &request.project_path).await?;
    let resolved = match scopes.snapshot_sidebar_scopes(&paths, &ids).await {
        Ok(resolved) => resolved,
        Err(SidebarScopeError::ForeignSession(_)) => return Err(invalid_project()),
        Err(SidebarScopeError::InvalidRequest) => return Err(invalid_query("invalid session ids")),
    };
    let targets: Vec<SidebarTarget> = resolved
        .iter()
        .map(|(runtime_session_id, scope)| SidebarTarget {
            runtime_session_id: *runtime_session_id,
            scope: sidebar_scope(scope.as_deref()),
        })
        .collect();
    let now = now_utc_ms();
    blocking(
        request.project_path,
        move |canonical, _retention, records| {
            sidebar_means(&records, &canonical, &targets, now).map_err(report_error)
        },
    )
    .await
}

/// Route the four history commands to their typed handler; every other
/// command goes to `fallback` exactly once and keeps its result. The caller
/// already observed the invoke.
pub(crate) fn dispatch<R, F>(invoke: tauri::ipc::Invoke<R>, fallback: &F) -> bool
where
    R: tauri::Runtime,
    F: Fn(tauri::ipc::Invoke<R>) -> bool,
{
    let history: fn(tauri::ipc::Invoke<R>) -> bool = tauri::generate_handler![
        get_context_history_report,
        get_context_history_agent_detail,
        get_context_history_changes,
        get_context_history_sidebar_means,
    ];
    match invoke.message.command() {
        "get_context_history_report"
        | "get_context_history_agent_detail"
        | "get_context_history_changes"
        | "get_context_history_sidebar_means" => history(invoke),
        _ => fallback(invoke),
    }
}

#[cfg(test)]
mod tests;
