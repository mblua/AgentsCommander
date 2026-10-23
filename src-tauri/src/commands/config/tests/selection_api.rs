use super::*;

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

mod barrier;
mod bench;
mod decision;
mod default;
mod error_code;
mod preview;
mod removal;
