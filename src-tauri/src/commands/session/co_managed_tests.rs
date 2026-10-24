use super::{
    co_managed_effective_state_for_session, AppSettings, CodingAgentKind, SessionBackendKind,
    SessionManager,
};
use crate::config::co_managed::{self, CoManagedState};
use std::path::PathBuf;
use std::sync::Arc;
use uuid::Uuid;

const TEAM_CONFIG: &str = concat!(
    r#"{"agents":["_agent_coordinator","_agent_member"],"#,
    r#""coordinator":"_agent_coordinator","repos":[]}"#
);

pub(crate) struct RoomFixture {
    _temp: tempfile::TempDir,
    room: PathBuf,
    coordinator: PathBuf,
    member: PathBuf,
}

// Accessors, so the phase-4 supervisor tests can share this fixture without
// reaching into its fields (#2232 phase 4).
impl RoomFixture {
    pub(crate) fn temp_path(&self) -> &std::path::Path {
        self._temp.path()
    }

    pub(crate) fn room_path(&self) -> &std::path::Path {
        &self.room
    }

    pub(crate) fn coordinator_path(&self) -> &std::path::Path {
        &self.coordinator
    }

    pub(crate) fn member_path(&self) -> &std::path::Path {
        &self.member
    }
}

fn replica_config(agent: &str) -> String {
    format!(r#"{{"identity":"../../_agent_{agent}","context":[],"repos":[]}}"#)
}

pub(crate) fn room_fixture() -> RoomFixture {
    let temp = tempfile::tempdir().expect("temp dir");
    let ac_root = temp.path().join("project-a").join(".ac");
    let room = ac_root.join("room-1-dev-team");
    let coordinator = room.join("__agent_coordinator");
    let member = room.join("__agent_member");
    for path in [
        &coordinator,
        &member,
        &ac_root.join("_agent_coordinator"),
        &ac_root.join("_agent_member"),
        &ac_root.join("_team_dev-team"),
    ] {
        std::fs::create_dir_all(path).expect("fixture dir");
    }
    std::fs::write(
        ac_root.join("_team_dev-team").join("config.json"),
        TEAM_CONFIG,
    )
    .expect("team config");
    std::fs::write(
        coordinator.join("config.json"),
        replica_config("coordinator"),
    )
    .expect("coordinator config");
    std::fs::write(member.join("config.json"), replica_config("member")).expect("member config");
    RoomFixture {
        _temp: temp,
        room,
        coordinator,
        member,
    }
}

pub(crate) fn configure_room(room: &std::path::Path, enabled: bool) {
    std::fs::create_dir_all(co_managed::co_managed_dir(room)).expect("co-managed dir");
    std::fs::write(room.join("catalog.json"), "{}").expect("catalog");
    std::fs::write(
        co_managed::config_path(room),
        format!(r#"{{"enabled":{enabled},"catalogPath":"catalog.json"}}"#),
    )
    .expect("config.json");
}

fn test_app(
    settings: AppSettings,
    manager: Arc<tokio::sync::RwLock<SessionManager>>,
) -> tauri::App<tauri::test::MockRuntime> {
    tauri::test::mock_builder()
        .manage(crate::config::settings::new_settings_state(settings))
        .manage(manager)
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build co-managed session test app")
}

pub(crate) fn settings_with_key() -> AppSettings {
    AppSettings {
        jev_api_key: "test-key".to_string(),
        co_managed_enabled: true,
        ..AppSettings::default()
    }
}

pub(crate) async fn add_session_for_tests(
    manager: &Arc<tokio::sync::RwLock<SessionManager>>,
    cwd: &std::path::Path,
    backend_kind: SessionBackendKind,
    agent_kind: CodingAgentKind,
) -> Uuid {
    add_session(manager, cwd, backend_kind, agent_kind).await
}

async fn add_session(
    manager: &Arc<tokio::sync::RwLock<SessionManager>>,
    cwd: &std::path::Path,
    backend_kind: SessionBackendKind,
    agent_kind: CodingAgentKind,
) -> Uuid {
    let session = {
        let guard = manager.read().await;
        guard
            .create_session(
                "bash".to_string(),
                Vec::new(),
                cwd.to_string_lossy().to_string(),
                None,
                None,
                Vec::new(),
                false,
                backend_kind,
            )
            .await
            .expect("create session")
    };
    {
        let guard = manager.read().await;
        guard.set_agent_kind(session.id, Some(agent_kind)).await;
    }
    session.id
}

async fn effective(
    app: &tauri::App<tauri::test::MockRuntime>,
    room: &std::path::Path,
    session_id: Uuid,
) -> CoManagedState {
    co_managed_effective_state_for_session(app.handle(), room, &session_id.to_string())
        .await
        .expect("gathering function answers")
}

#[cfg(test)]
mod tests;
