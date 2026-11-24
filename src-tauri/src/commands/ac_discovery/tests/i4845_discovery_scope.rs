//! #4845 P15: discovery shows the coding agent of the replica's own file (P03 order).
//! Tempdir only: project-scope named files under a tempdir project root; the 30 file is
//! the live `settings.agents` (`registeredInstance`).

use super::*;
use crate::config::agent_config::set_last_coding_agent_with_source;
use crate::config::coding_agent_files::locator::context_key;
use crate::config::coding_agent_files::{
    with_active, ConfigContext, ConfigFileLocator, ConfigScope,
};
use crate::config::settings::AgentConfig;
use std::path::{Path, PathBuf};

fn agent(id: &str) -> AgentConfig {
    serde_json::from_value(json!({"id": id, "label": id, "command": id, "color": "#123"})).unwrap()
}

fn team() -> ConfigFileLocator {
    ConfigFileLocator::Named {
        scope: ConfigScope::Project,
        name: "team".into(),
    }
}

fn s(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

struct Proj {
    _temp: tempfile::TempDir,
    project: PathBuf,
    replica: PathBuf,
    settings: AppSettings,
}

/// A project with `team` = [codex], one replica `__agent_x` (tooling `tooling`) and its
/// matrix; the 30 file = `agents_30`.
fn proj(agents_30: &[&str], tooling: Value) -> Proj {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project");
    let ac_root = project.join(".ac");
    let dir = ac_root.join("coding-agents");
    std::fs::create_dir_all(&dir).unwrap();
    let file = json!({ "agents": [{"id": "codex", "label": "codex", "command": "codex",
        "color": "#123"}] });
    std::fs::write(
        dir.join("ac.coding-agents-config.team.json"),
        file.to_string(),
    )
    .unwrap();
    let matrix = ac_root.join("_agent_x");
    let replica = ac_root.join("room-1-team").join("__agent_x");
    std::fs::create_dir_all(&matrix).unwrap();
    std::fs::create_dir_all(&replica).unwrap();
    std::fs::write(matrix.join("Role.md"), "# Role\n").unwrap();
    let config = json!({"identity": "../../_agent_x", "tooling": tooling});
    std::fs::write(replica.join("config.json"), config.to_string()).unwrap();
    let settings = AppSettings {
        agents: agents_30.iter().map(|id| agent(id)).collect(),
        project_paths: vec![s(&project)],
        ..AppSettings::default()
    };
    Proj {
        _temp: temp,
        project,
        replica,
        settings,
    }
}

impl Proj {
    fn activate_team(&mut self) {
        let ctx = ConfigContext::Project(context_key(&s(&self.project)));
        self.settings.coding_agent_config_files = Some(with_active(None, &ctx, team()));
    }

    fn preferred(&self) -> Option<String> {
        let mut warnings = Vec::new();
        read_preferred_agent_id_logged(&self.replica, &self.settings, &mut warnings)
    }

    async fn current_coding_agent(&self) -> Option<String> {
        let settings = crate::config::settings::new_settings_state(self.settings.clone());
        let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let clocks: crate::config::coordinator_clocks::CoordinatorClocksState = Arc::new(
            std::sync::Mutex::new(crate::config::coordinator_clocks::CoordinatorClocks::default()),
        );
        let app = crate::test_support::test_builder()
            .manage(settings.clone())
            .manage(Arc::clone(&session_mgr))
            .manage(clocks.clone())
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("build discovery test app");
        let watcher = DiscoveryBranchWatcher::new(app.handle().clone(), Arc::clone(&session_mgr));
        let result = discover_project_inner(
            app.handle(),
            &session_mgr,
            &s(&self.project),
            &settings,
            &watcher,
            &clocks,
        )
        .await
        .expect("discover_project_inner");
        let room = result.workgroups.iter().find(|wg| wg.name == "room-1-team");
        let x = room.and_then(|room| room.agents.iter().find(|a| a.name == "x"));
        x.expect("replica x discovered")
            .current_coding_agent_id
            .clone()
    }
}

#[test]
fn replica_label_from_last_used_named_file() {
    let p = proj(&["claude"], json!({}));
    let source = serde_json::to_value(team()).unwrap();
    let dir = s(&p.replica);
    set_last_coding_agent_with_source(&dir, "codex", "codex", None, None, Some(&source)).unwrap();
    assert_eq!(p.preferred().as_deref(), Some("codex"));
}

#[tokio::test]
async fn replica_never_started_uses_project_active_file() {
    let mut p = proj(&["claude"], json!({"currentCodingAgent": "codex"}));
    p.activate_team();
    assert_eq!(p.current_coding_agent().await.as_deref(), Some("codex"));
}

#[tokio::test]
async fn no_selection_discovery_is_identical_to_base() {
    let p = proj(&["codex"], json!({"currentCodingAgent": "codex"}));
    let dir = s(&p.replica);
    set_last_coding_agent_with_source(&dir, "codex", "codex", None, None, None).unwrap();
    let reference = crate::config::agent_command::StoredReference::from_config(
        crate::config::agent_config::read_agent_local_config_json(&p.replica)
            .unwrap()
            .as_ref(),
        "codex",
    );
    let base =
        match crate::config::agent_command::resolve_portable_reference(&p.settings, &reference) {
            crate::config::agent_command::MatchOutcome::Matched(found) => Some(found.agent_id),
            crate::config::agent_command::MatchOutcome::NoMatch { .. } => None,
        };
    assert_eq!(p.preferred(), base);
    assert_eq!(p.current_coding_agent().await.as_deref(), Some("codex"));
}
