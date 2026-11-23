//! #4845 D8: whole-settings saves keep the coding-agent config-file selection.

use super::{
    persist_protected_settings_update_with_saver, persist_settings_draft_update_with_saver,
    state_for,
};
use crate::config::coding_agent_files::{
    with_active, with_loaded, CodingAgentConfigFiles, ConfigContext, ConfigFileLocator, ConfigScope,
};
use crate::config::settings::{
    load_settings_from_path, save_settings_to_path_preserving_project_paths, AppSettings,
};
use serde_json::Value;
use std::path::{Path, PathBuf};

fn seed(dir: &Path) -> PathBuf {
    let path = dir.join("settings.json");
    let json = serde_json::to_vec_pretty(&AppSettings::default()).unwrap();
    std::fs::write(&path, json).unwrap();
    path
}

fn disk_selection(path: &Path) -> Option<CodingAgentConfigFiles> {
    let raw: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    raw.get("codingAgentConfigFiles")
        .cloned()
        .map(|value| serde_json::from_value(value).unwrap())
}

fn selections() -> [CodingAgentConfigFiles; 2] {
    let named = ConfigFileLocator::Named {
        scope: ConfigScope::Instance,
        name: "team".into(),
    };
    let loaded_path = "/somewhere/agents.json";
    let loaded = with_loaded(None, loaded_path);
    [
        with_active(None, &ConfigContext::Instance, named),
        with_active(
            Some(&loaded),
            &ConfigContext::Instance,
            ConfigFileLocator::Loaded {
                path: loaded_path.into(),
            },
        ),
    ]
}

async fn assert_kept(draft: bool, selection: CodingAgentConfigFiles) {
    let temp = tempfile::tempdir().unwrap();
    let path = seed(temp.path());
    let old_snapshot = load_settings_from_path(&path);
    assert!(old_snapshot.coding_agent_config_files.is_none());
    let mut live = old_snapshot.clone();
    live.coding_agent_config_files = Some(selection.clone());
    let state = state_for(live);
    let save_path = path.clone();
    let save = move |candidate: &AppSettings| {
        save_settings_to_path_preserving_project_paths(candidate, &save_path)
    };
    if draft {
        persist_settings_draft_update_with_saver(&state, old_snapshot, save)
            .await
            .unwrap();
    } else {
        persist_protected_settings_update_with_saver(&state, old_snapshot, save)
            .await
            .unwrap();
    }
    assert_eq!(
        state.read().await.coding_agent_config_files.as_ref(),
        Some(&selection)
    );
    assert_eq!(disk_selection(&path), Some(selection));
}

#[tokio::test]
async fn whole_settings_save_keeps_selection() {
    for selection in selections() {
        assert_kept(false, selection.clone()).await;
        assert_kept(true, selection).await;
    }
}
