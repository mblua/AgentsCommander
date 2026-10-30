use crate::config::settings::AgentConfig;
use crate::phone::types::OutboxMessage;
use crate::pty::backend::{BackendSpawnSpec, PtyBackend};
use crate::pty::manager::PtyManager;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use uuid::Uuid;

// (#885 E-3) Minimal mock PTY backend for purge-wg e2e tests. Sessions
// in `live` report `has_session: true`; `kill` removes them. Other
// methods are no-ops. This lets the purge gate exercise the F-3
// liveness predicate and the IdleDetector correlation end-to-end.
#[derive(Default)]
pub(crate) struct MailboxMockPtyBackend {
    pub(crate) live: std::sync::Mutex<HashSet<Uuid>>,
    pub(crate) writes: std::sync::Mutex<Vec<(Uuid, Vec<u8>)>>,
    pub(crate) fail_spawn: std::sync::atomic::AtomicBool,
    pub(crate) fail_kill_once: std::sync::atomic::AtomicBool,
    pub(crate) kill_attempts: std::sync::atomic::AtomicUsize,
    pub(crate) fail_write: std::sync::atomic::AtomicBool,
    pub(crate) write_attempts: AtomicU32,
}

impl MailboxMockPtyBackend {
    pub(crate) fn set_live(&self, id: Uuid) {
        self.live.lock().unwrap().insert(id);
    }

    pub(crate) fn writes_for(&self, id: Uuid) -> Vec<Vec<u8>> {
        self.writes
            .lock()
            .unwrap()
            .iter()
            .filter(|(session_id, _)| *session_id == id)
            .map(|(_, bytes)| bytes.clone())
            .collect()
    }
}

impl PtyBackend for MailboxMockPtyBackend {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn spawn(
        &self,
        spec: BackendSpawnSpec,
    ) -> futures::future::BoxFuture<'_, Result<(), crate::errors::AppError>> {
        // #1163 M2 - test-only failure switch so a session-request poll can
        // exercise the rejected-result path end to end.
        if self.fail_spawn.load(Ordering::SeqCst) {
            return Box::pin(async move {
                Err(crate::errors::AppError::PtyError(
                    "synthetic spawn failure".to_string(),
                ))
            });
        }
        Box::pin(async move {
            self.live.lock().unwrap().insert(spec.id);
            Ok(())
        })
    }

    fn write(
        &self,
        _authority: &crate::pty::manager::BackendWriteAuthority,
        id: Uuid,
        data: &[u8],
    ) -> Result<(), crate::errors::AppError> {
        // #1883 — test-only failure switch so both mailbox injection sites
        // can exercise the real-error path end to end.
        self.write_attempts.fetch_add(1, Ordering::SeqCst);
        if self.fail_write.load(Ordering::SeqCst) {
            return Err(crate::errors::AppError::PtyError(
                "synthetic write failure".to_string(),
            ));
        }
        self.writes.lock().unwrap().push((id, data.to_vec()));
        Ok(())
    }

    fn resize(&self, _id: Uuid, _cols: u16, _rows: u16) -> Result<(), crate::errors::AppError> {
        Ok(())
    }

    fn kill(&self, id: Uuid) -> Result<(), crate::errors::AppError> {
        self.kill_attempts.fetch_add(1, Ordering::SeqCst);
        if self.fail_kill_once.swap(false, Ordering::SeqCst) {
            return Err(crate::errors::AppError::PtyError(
                "simulated fixture kill failure once".into(),
            ));
        }
        self.live.lock().unwrap().remove(&id);
        Ok(())
    }

    fn has_session(&self, id: Uuid) -> bool {
        self.live.lock().unwrap().contains(&id)
    }

    fn get_screen_snapshot(&self, _id: Uuid) -> Option<crate::pty::output::PtyScreenSnapshot> {
        None
    }

    fn get_pty_size(&self, _id: Uuid) -> Option<(u16, u16)> {
        Some((30, 120))
    }

    fn get_screen_rows(&self, _id: Uuid) -> crate::pty::context_scrape::ScreenRowsRead {
        crate::pty::context_scrape::ScreenRowsRead::SessionOver
    }

    fn register_response_watcher(
        &self,
        _session_id: Uuid,
        _request_id: String,
        _response_dir: std::path::PathBuf,
    ) {
    }

    fn terminate_job_for_session(&self, id: Uuid) -> bool {
        self.live.lock().unwrap().remove(&id)
    }

    fn kill_all_jobs(&self) -> (usize, usize) {
        (0, 0)
    }
}

/// (#885 E-3) Build a `PtyManager` with a mock backend whose live set
/// can be pre-populated. Sessions registered as "live" will pass the
/// F-3 `has_pty` liveness check in `handle_purge_wg`.
pub(crate) fn make_test_pty_manager() -> (
    Arc<std::sync::Mutex<PtyManager>>,
    Arc<MailboxMockPtyBackend>,
) {
    let backend = Arc::new(MailboxMockPtyBackend::default());
    let mgr = Arc::new(std::sync::Mutex::new(PtyManager::new_for_test(
        backend.clone(),
    )));
    (mgr, backend)
}

pub(crate) fn make_session_info(
    id: &str,
    name: &str,
    cwd: &str,
    status: crate::session::session::SessionStatus,
    waiting_for_input: bool,
) -> crate::session::session::SessionInfo {
    crate::session::session::SessionInfo {
        id: id.into(),
        name: name.into(),
        shell: "claude".into(),
        shell_args: vec![],
        backend_kind: crate::pty::backend::SessionBackendKind::LocalProcess,
        effective_shell_args: None,
        created_at: "2026-05-16T00:00:00Z".into(),
        working_directory: cwd.into(),
        status,
        waiting_for_input,
        communication: None,
        pending_review: false,
        last_prompt: None,
        agent_id: None,
        agent_label: None,
        git_repos: vec![],
        workgroup_task: None,
        is_coordinator: false,
        is_root_agent: false,
        token: "t".into(),
        agent_kind: Some(crate::session::profile::CodingAgentKind::Claude),
        requested_profile: None,
        effective_profile: None,
        profile_fallback_chain: Vec::new(),
        profile_fallback_applied: false,
        match_tier: None,
        original_profile_letter: None,
        effective_codex_home: None,
        profile_content_hash: None,
        trusted_configured_spawn: false,
        profile_outdated: false,
        telegram_bot_id: None,
        was_detached: false,
        detached_geometry: None,
        start_fresh_on_restore: false,
        context_percent: None,
    }
}

pub(crate) fn path_string(path: &Path) -> String {
    path.to_string_lossy().to_string()
}

pub(crate) fn make_root_route_fixture(
    spoofed_coordinator_identity: bool,
) -> (tempfile::TempDir, Vec<String>) {
    let temp = tempfile::TempDir::new().unwrap();
    let project = temp.path().join("proj-a");
    let ac_root = project.join(".ac");
    let team_dir = ac_root.join("_team_dev-team");
    let origin_tech_lead = ac_root.join("_agent_tech-lead");
    let origin_dev_rust = ac_root.join("_agent_dev-rust");
    let wg_dir = ac_root.join("wg-1-dev-team");
    let tech_lead_replica = wg_dir.join("__agent_tech-lead");
    let dev_rust_replica = wg_dir.join("__agent_dev-rust");

    for dir in [
        &team_dir,
        &origin_tech_lead,
        &origin_dev_rust,
        &tech_lead_replica,
        &dev_rust_replica,
    ] {
        std::fs::create_dir_all(dir).unwrap();
    }

    std::fs::write(
        team_dir.join("config.json"),
        concat!(
            r#"{"agents":["../_agent_dev-rust","../_agent_tech-lead"],"#,
            r#""coordinator":"../_agent_tech-lead"}"#,
        ),
    )
    .unwrap();
    let tech_lead_identity = if spoofed_coordinator_identity {
        "../../_agent_dev-rust"
    } else {
        "../../_agent_tech-lead"
    };
    std::fs::write(
        tech_lead_replica.join("config.json"),
        format!(r#"{{"identity":"{}"}}"#, tech_lead_identity),
    )
    .unwrap();
    std::fs::write(
        dev_rust_replica.join("config.json"),
        r#"{"identity":"../../_agent_dev-rust"}"#,
    )
    .unwrap();

    let paths = vec![temp.path().to_string_lossy().to_string()];
    (temp, paths)
}

pub(crate) fn root_outbox_message(body: String, command: Option<String>) -> OutboxMessage {
    OutboxMessage {
        id: "msg-root".into(),
        token: None,
        from: crate::config::root_agent::ROOT_AGENT_SENDER.into(),
        to: "proj-a:wg-1-dev-team/tech-lead".into(),
        body,
        mode: "wake".into(),
        get_output: false,
        request_id: None,
        sender_agent: None,
        preferred_agent: "auto".into(),
        requested_profile: None,
        effective_agent_id: None,
        effective_profile: None,
        profile_fallback_applied: false,
        dispatch_not_applied: None,
        priority: "normal".into(),
        timestamp: "2026-05-24T00:00:00Z".into(),
        command,
        action: None,
        target: None,
        force: None,
        timeout_secs: None,
        switch_coding_agent: None,
        switch_profile: None,
        dry_run: None,
        quiet_period_ms: None,
        pty_input: None,
    }
}

pub(crate) fn wake_agent(id: &str, label: &str, command: &str) -> AgentConfig {
    AgentConfig {
        preflight: None,
        preflight_timeout_seconds: None,
        id: id.into(),
        id_hash: None,
        id_hash_debug: None,
        label: label.into(),
        command: command.into(),
        color: "#10b981".into(),
        order: None,
        envs: Vec::new(),
        isolated_home: false,
        instructions_filename: None,
        config_seed: None,
        context_regex: None,
        blocking_menus: None,
        backend: Default::default(),
    }
}

pub(crate) fn wake_agents() -> Vec<AgentConfig> {
    vec![
        wake_agent("codex", "Codex", "codex --yolo"),
        wake_agent("claude", "Claude", "claude"),
    ]
}

/// Fixture-owned barrier records the dormant adapter without activating PTY detection.
pub(crate) struct ClearObservationProbe {
    pub(crate) demote: bool,
    pub(crate) owned_children: AtomicU32,
    pub(crate) sources: Mutex<Vec<PathBuf>>,
    pub(crate) entered: tokio::sync::Notify,
    pub(crate) release: tokio::sync::Semaphore,
}

impl ClearObservationProbe {
    pub(crate) async fn wait_for(&self, count: usize) -> Vec<PathBuf> {
        loop {
            let entered = self.entered.notified();
            let sources = self.sources.lock().unwrap().clone();
            if sources.len() >= count {
                return sources;
            }
            entered.await;
        }
    }

    pub(crate) fn release(&self, count: usize) {
        self.release.add_permits(count);
    }
}
