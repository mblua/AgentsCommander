use super::*;
use crate::config::settings::{
    AgentFileEdits, AgentFileSaveOutcome, AppSettings, SettingsOwner, SourceKind,
};
use serde_json::json;
use std::sync::Arc;

fn fixture() -> (
    tempfile::TempDir,
    Arc<SettingsOwner>,
    crate::config::settings::AgentFileSourceOption,
    CatalogPaths,
) {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project");
    std::fs::create_dir(&project).unwrap();
    let path = temp
        .path()
        .join(crate::config::instance_artifacts::SETTINGS_FILE_NAME);
    let settings = AppSettings {
        project_paths: vec![project.to_string_lossy().into_owned()],
        ..AppSettings::default()
    };
    std::fs::write(&path, serde_json::to_vec(&settings).unwrap()).unwrap();
    let dir = catalog_dir(&project.join(".ac"));
    std::fs::create_dir_all(&dir).unwrap();
    let paths = CatalogPaths::new(&dir);
    std::fs::write(
        &paths.local,
        br#"{"schemaVersion":1,"agents":[],"extension":"keep"}"#,
    )
    .unwrap();
    let owner = SettingsOwner::open_active(&path, Duration::from_secs(1)).unwrap();
    let source = crate::config::settings::list_agent_files(
        &owner.blocking_current_snapshot().unwrap(),
        &path,
    )
    .unwrap()
    .sources
    .into_iter()
    .find(|s| s.source.kind == SourceKind::CatalogPersonal)
    .unwrap();
    (temp, owner, source, paths)
}

#[test]
fn e2432_p02_catalog_lock_and_atomic_failure_classify_effects() {
    let (_temp, owner, source, paths) = fixture();
    let before = std::fs::read(&paths.local).unwrap();
    let held = acquire_catalog_lock(&paths).unwrap();
    CATALOG_LOCK_TIMEOUT_OVERRIDE.with(|cell| cell.set(Some(Duration::from_millis(10))));
    let rejected = owner
        .save_catalog_source_blocking(
            source.source.clone(),
            source.revision.clone(),
            source.context_revision.clone(),
            AgentFileEdits::default(),
        )
        .unwrap();
    CATALOG_LOCK_TIMEOUT_OVERRIDE.with(|cell| cell.set(None));
    drop(held);
    assert_eq!(rejected.outcome, AgentFileSaveOutcome::Rejected);
    assert!(rejected
        .diagnostics
        .iter()
        .any(|d| d.code == "catalogLockTimeout"));
    assert_eq!(std::fs::read(&paths.local).unwrap(), before);
    let foreign = paths.local.parent().unwrap().join("foreign.tmp");
    std::fs::write(&foreign, b"foreign").unwrap();
    CATALOG_FAILURE_POINT.with(|cell| cell.set(Some("publication_temp_synced")));
    let failed = owner
        .save_catalog_source_blocking(
            source.source.clone(),
            source.revision.clone(),
            source.context_revision.clone(),
            AgentFileEdits::default(),
        )
        .unwrap();
    CATALOG_FAILURE_POINT.with(|cell| cell.set(None));
    assert_eq!(failed.outcome, AgentFileSaveOutcome::PartialOrUnknown);
    assert_eq!(failed.snapshot.unwrap().revision, source.revision);
    assert_eq!(std::fs::read(&paths.local).unwrap(), before);
    assert_eq!(std::fs::read(&foreign).unwrap(), b"foreign");
    assert!(owner.blocking_current_snapshot().is_ok());
    let committed = owner
        .save_catalog_source_blocking(
            source.source,
            source.revision,
            source.context_revision,
            AgentFileEdits::default(),
        )
        .unwrap();
    assert_eq!(committed.outcome, AgentFileSaveOutcome::Committed);
    let actual = crate::config::settings::agent_file_read(&paths.local).unwrap();
    assert_ne!(sha256_hex(&actual), sha256_hex(&before));
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&actual).unwrap()["extension"],
        json!("keep")
    );
}

#[test]
fn e2432_p02_linked_physical_file_is_not_read_as_regular() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("original.json");
    let alias = temp.path().join("alias.json");
    std::fs::write(&source, b"{}").unwrap();
    std::fs::hard_link(&source, &alias).unwrap();
    assert!(crate::config::settings::agent_file_read(&source).is_err());
    assert!(crate::config::settings::agent_file_read(&alias).is_err());
    assert_eq!(std::fs::read(&source).unwrap(), b"{}");
}
