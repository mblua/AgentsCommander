use crate::config::project_settings::WorkgroupGroupsConfig;
use crate::web::broadcast::WsBroadcaster;
use crate::web::commands::broadcast_all;
use crate::web::commands::{
    get_project_groups_inner, project_groups_updated_payload, update_project_groups_inner,
    PROJECT_GROUPS_UPDATED_EVENT,
};
use tauri::{AppHandle, State};

#[tauri::command]
pub async fn get_project_groups(path: String) -> Result<WorkgroupGroupsConfig, String> {
    get_project_groups_inner(&path)
}

#[tauri::command]
pub async fn update_project_groups(
    app: AppHandle,
    broadcaster: State<'_, WsBroadcaster>,
    path: String,
    config: WorkgroupGroupsConfig,
) -> Result<WorkgroupGroupsConfig, String> {
    let result = update_project_groups_inner(&path, config)?;
    let payload = project_groups_updated_payload(&path, &result);
    broadcast_all(
        &app,
        broadcaster.inner(),
        PROJECT_GROUPS_UPDATED_EVENT,
        &payload,
    );
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::get_project_groups;
    use crate::config::project_settings::{WorkgroupGroup, WorkgroupGroupsConfig};
    use crate::web::commands::update_project_groups_inner;

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
                favorite: false,
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

        let saved = update_project_groups_inner(&path, config.clone()).expect("update groups");
        let loaded = get_project_groups(path).await.expect("get groups");

        assert_eq!(saved, config);
        assert_eq!(loaded, config);
    }
}
