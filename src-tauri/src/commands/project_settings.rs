use std::path::Path;

use crate::config::project_settings::{
    load_workgroup_groups, save_workgroup_groups, WorkgroupGroupsConfig,
};

pub(crate) fn get_project_groups_inner(path: &str) -> Result<WorkgroupGroupsConfig, String> {
    load_workgroup_groups(Path::new(path))
}

#[tauri::command]
pub async fn get_project_groups(path: String) -> Result<WorkgroupGroupsConfig, String> {
    get_project_groups_inner(&path)
}

pub(crate) fn update_project_groups_inner(
    path: &str,
    config: WorkgroupGroupsConfig,
) -> Result<WorkgroupGroupsConfig, String> {
    save_workgroup_groups(Path::new(path), config)
}

#[tauri::command]
pub async fn update_project_groups(
    path: String,
    config: WorkgroupGroupsConfig,
) -> Result<WorkgroupGroupsConfig, String> {
    update_project_groups_inner(&path, config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::project_settings::WorkgroupGroup;

    fn project_with_workspace() -> tempfile::TempDir {
        let temp = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(temp.path().join(".ac")).expect("create .ac");
        temp
    }

    fn sample_config() -> WorkgroupGroupsConfig {
        WorkgroupGroupsConfig {
            groups: vec![WorkgroupGroup {
                id: "bots".to_string(),
                name: "BOTS".to_string(),
                regex: "^(wg-9)$".to_string(),
            }],
            show_all: true,
            show_ungrouped: true,
            non_stop: None,
        }
    }

    #[tokio::test]
    async fn get_project_groups_returns_default_for_missing_file() {
        let project = project_with_workspace();

        let loaded = get_project_groups(project.path().to_string_lossy().to_string())
            .await
            .expect("get groups");

        assert_eq!(loaded, WorkgroupGroupsConfig::default());
    }

    #[tokio::test]
    async fn update_project_groups_round_trips_saved_config() {
        let project = project_with_workspace();
        let path = project.path().to_string_lossy().to_string();
        let config = sample_config();

        let saved = update_project_groups(path.clone(), config.clone())
            .await
            .expect("update groups");
        let loaded = get_project_groups(path).await.expect("get groups");

        assert_eq!(saved, config);
        assert_eq!(loaded, config);
    }
}
