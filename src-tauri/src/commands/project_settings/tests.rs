use super::*;
use crate::config::project_settings::WorkgroupGroup;

fn project_with_ac_root() -> tempfile::TempDir {
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
    let project = project_with_ac_root();

    let loaded = get_project_groups(project.path().to_string_lossy().to_string())
        .await
        .expect("get groups");

    assert_eq!(loaded, WorkgroupGroupsConfig::default());
}

#[tokio::test]
async fn update_project_groups_round_trips_saved_config() {
    let project = project_with_ac_root();
    let path = project.path().to_string_lossy().to_string();
    let config = sample_config();

    let saved = update_project_groups_inner(&path, config.clone()).expect("update groups");
    let loaded = get_project_groups(path).await.expect("get groups");

    assert_eq!(saved, config);
    assert_eq!(loaded, config);
}
