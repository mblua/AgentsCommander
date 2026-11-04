use super::*;
use crate::config::settings::{
    instance_settings_display_path, instance_settings_target, persisted_instance_token_matches,
    settings_fixture_path, SettingsFileTarget, SettingsProjectCheckpoint,
    SettingsProjectTransaction,
};
use serde_json::json;

#[test]
fn locator_and_fixture_preserve_legacy_paths_without_io() {
    let expected = crate::config::config_dir().map(|dir| dir.join(SETTINGS_FILE_NAME));
    assert_eq!(settings_path(), expected);
    assert_eq!(
        instance_settings_target().map(|target| target.path),
        expected
    );
    assert_eq!(
        instance_settings_display_path(),
        expected.map(|path| path.display().to_string())
    );
    let temp = tempfile::tempdir().unwrap();
    let directory = temp.path().join("not-created");
    assert_eq!(
        settings_fixture_path(&directory),
        directory.join(SETTINGS_FILE_NAME)
    );
    assert!(!directory.exists());
    let legacy = temp.path().join("arbitrary-main-name.json");
    let bytes = b"{malformed fixture bytes";
    std::fs::write(&legacy, bytes).unwrap();
    let target = SettingsFileTarget::for_test_path(&legacy);
    assert_eq!(target.clone().path, legacy);
    assert_eq!(std::fs::read(&legacy).unwrap(), bytes);
}

#[test]
fn token_predicate_requires_only_the_persisted_root_string() {
    let temp = tempfile::tempdir().unwrap();
    let path = settings_fixture_path(temp.path());
    assert!(!persisted_instance_token_matches(temp.path(), "fallback"));
    std::fs::write(temp.path().join("master-token.txt"), "fallback").unwrap();
    std::fs::write(
        temp.path()
            .join(crate::config::instance_artifacts::SETTINGS_LOCAL_OVERRIDE_FILE_NAME),
        br#"{"rootToken":"overlay"}"#,
    )
    .unwrap();
    for bytes in [
        br#"{"rootToken":"persisted","unrelated":true}"#.as_slice(),
        br#"{"rootToken":"persisted","agents":"malformed"}"#.as_slice(),
    ] {
        std::fs::write(&path, bytes).unwrap();
        assert!(persisted_instance_token_matches(temp.path(), "persisted"));
        assert!(!persisted_instance_token_matches(temp.path(), "overlay"));
        assert!(!persisted_instance_token_matches(temp.path(), "fallback"));
        assert!(!persisted_instance_token_matches(
            temp.path(),
            " persisted "
        ));
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
    }
    for bytes in [
        br#"{"rootToken":"persisted","rootToken":"persisted"}"#.as_slice(),
        br#"{"rootToken":"persisted","nested":{"a":1,"a":2}}"#.as_slice(),
        br#"{"rootToken":42}"#.as_slice(),
        br#"{"other":"persisted"}"#.as_slice(),
        b"{malformed".as_slice(),
        b"\xff".as_slice(),
    ] {
        std::fs::write(&path, bytes).unwrap();
        assert!(!persisted_instance_token_matches(temp.path(), "persisted"));
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
    }
}

#[test]
fn token_predicate_enforces_regular_file_and_one_mib_limit() {
    let temp = tempfile::tempdir().unwrap();
    let path = settings_fixture_path(temp.path());
    let mut bytes = br#"{"rootToken":"limit"}"#.to_vec();
    bytes.resize(1024 * 1024, b' ');
    std::fs::write(&path, &bytes).unwrap();
    assert!(persisted_instance_token_matches(temp.path(), "limit"));
    bytes.push(b' ');
    std::fs::write(&path, &bytes).unwrap();
    assert!(!persisted_instance_token_matches(temp.path(), "limit"));
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();
    assert!(!persisted_instance_token_matches(temp.path(), "limit"));
}

#[test]
fn project_transaction_matches_legacy_and_restores_checkpoint() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("legacy-settings.json");
    let a = temp.path().join("a");
    let b = temp.path().join("b");
    std::fs::create_dir_all(a.join(".ac")).unwrap();
    std::fs::create_dir_all(b.join(".ac")).unwrap();
    let a = a.to_string_lossy().replace(char::from(92), "/");
    let b = b.to_string_lossy().replace(char::from(92), "/");
    let main = json!({"defaultShell":"sentinel","agents":[],"projectPath":a,
        "projectPaths":[a,"C:/missing"],"projectPathsRelativeToInstance":[null,null]});
    std::fs::write(&path, main.to_string()).unwrap();
    let current = AppSettings::default();
    let mut old = super::super::begin_project_paths_transaction_at_path(&current, &path).unwrap();
    let old_fresh = old.fresh_settings().project_paths.clone();
    let before = old.raw_project_projection().unwrap();
    let mut old_refreshed = old.fresh_settings().clone();
    old.refresh_raw_projects(&mut old_refreshed).unwrap();
    let old_written = old
        .commit_project_groups(&old_refreshed, false, false)
        .unwrap();
    drop(old);
    let target = SettingsFileTarget::for_test_path(&path);
    let mut transaction: SettingsProjectTransaction = target.begin_project_paths(&current).unwrap();
    assert_eq!(transaction.fresh_settings().project_paths, old_fresh);
    let checkpoint: SettingsProjectCheckpoint = transaction.checkpoint().unwrap();
    let mut candidate = transaction.fresh_settings().clone();
    transaction.refresh_raw_projects(&mut candidate).unwrap();
    assert_eq!(candidate.project_paths, old_refreshed.project_paths);
    let unchanged = transaction
        .commit_project_groups(&candidate, false, false)
        .unwrap();
    assert_eq!(unchanged.project_paths, old_written.project_paths);
    candidate.project_paths.push(b);
    super::super::resync_project_state_from_runtime(&mut candidate);
    transaction
        .commit_project_groups(&candidate, true, true)
        .unwrap();
    let restored = transaction.restore_checkpoint(&checkpoint).unwrap();
    assert_eq!(restored.project_paths, old_fresh);
    drop(transaction);
    let reopened = super::super::begin_project_paths_transaction_at_path(&current, &path).unwrap();
    assert_eq!(reopened.raw_project_projection().unwrap(), before);
    let main_after: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(main_after["defaultShell"], "sentinel");
}

#[test]
fn checkpoint_restores_malformed_values_and_keeps_unknown_sidecar_fields() {
    let temp = tempfile::tempdir().unwrap();
    let path = settings_fixture_path(temp.path());
    std::fs::write(&path, b"{}").unwrap();
    let sidecar = super::super::project_paths_path(&path);
    let original = json!({"projectPaths":"malformed","projectPath":17,
        "archivedProjectPaths":{"bad":true},"unknown":{"keep":[1,null,"x"]}});
    std::fs::write(&sidecar, original.to_string()).unwrap();
    let target = SettingsFileTarget::for_test_path(&path);
    let mut transaction = target.begin_project_paths(&AppSettings::default()).unwrap();
    let checkpoint = transaction.checkpoint().unwrap();
    let mut candidate = transaction.fresh_settings().clone();
    assert!(transaction.refresh_raw_projects(&mut candidate).is_err());
    let mut replacement = original.clone();
    replacement["projectPaths"] = json!([]);
    replacement["projectPath"] = Value::Null;
    replacement["archivedProjectPaths"] = json!([]);
    std::fs::write(&sidecar, replacement.to_string()).unwrap();
    transaction.restore_checkpoint(&checkpoint).unwrap();
    let restored: Value = serde_json::from_slice(&std::fs::read(&sidecar).unwrap()).unwrap();
    assert_eq!(restored, original);
}

#[test]
fn repair_preserves_legacy_results_and_failure_stage() {
    let temp = tempfile::tempdir().unwrap();
    let path = settings_fixture_path(temp.path());
    std::fs::write(&path, b"{}").unwrap();
    let mut expected = AppSettings::default();
    let mut actual = expected.clone();
    super::super::repair_fresh_project_paths_at_path(&mut expected, &path).unwrap();
    SettingsFileTarget::for_test_path(&path)
        .repair_project_paths(&mut actual)
        .unwrap();
    assert_eq!(actual.project_paths, expected.project_paths);
    assert_eq!(
        actual.project_paths_persistence_error,
        expected.project_paths_persistence_error
    );
    let failed = temp.path().join("directory-target");
    std::fs::create_dir(&failed).unwrap();
    let legacy = super::super::repair_fresh_project_paths_at_path(&mut expected, &failed);
    let typed = SettingsFileTarget::for_test_path(&failed).repair_project_paths(&mut actual);
    let (legacy_stage, legacy_error) = legacy.unwrap_err();
    let (typed_stage, typed_error) = typed.unwrap_err();
    assert!(matches!(legacy_stage, ProjectPathsRepairStage::Read));
    assert!(matches!(typed_stage, ProjectPathsRepairStage::Read));
    assert_eq!(typed_error, legacy_error);
    assert!(SettingsFileTarget::for_test_path(&failed)
        .begin_project_paths(&actual)
        .is_err());
}

#[test]
fn sibling_stamps_and_caches_match_legacy_bytes_and_layers() {
    let typed_dir = tempfile::tempdir().unwrap();
    let old_dir = tempfile::tempdir().unwrap();
    let typed_path = typed_dir.path().join("arbitrary.json");
    let old_path = old_dir.path().join("arbitrary.json");
    let target = SettingsFileTarget::for_test_path(&typed_path);
    let at = DateTime::parse_from_rfc3339("2026-11-04T12:34:56Z")
        .unwrap()
        .with_timezone(&Utc);
    assert_eq!(target.read_remote_agent_help_check_stamp(), None);
    assert_eq!(target.read_remote_blocking_menus_check_stamp(), None);
    target.write_remote_agent_help_check_stamp(at).unwrap();
    target.write_remote_blocking_menus_check_stamp(at).unwrap();
    super::super::write_remote_agent_help_check_stamp(&old_path, at).unwrap();
    super::super::write_remote_blocking_menus_check_stamp(&old_path, at).unwrap();
    assert_eq!(target.read_remote_agent_help_check_stamp(), Some(at));
    assert_eq!(target.read_remote_blocking_menus_check_stamp(), Some(at));
    let help = super::super::shipped_agent_help().clone();
    let menus = super::super::shipped_blocking_menus().clone();
    target
        .write_remote_agent_help_cache(help.clone(), "https://example.com/help", at)
        .unwrap();
    super::super::write_remote_agent_help_cache(
        &old_path,
        help.clone(),
        "https://example.com/help",
        at,
    )
    .unwrap();
    target
        .write_remote_blocking_menus_cache(menus.clone(), "https://example.com/menus", at)
        .unwrap();
    super::super::write_remote_blocking_menus_cache(
        &old_path,
        menus,
        "https://example.com/menus",
        at,
    )
    .unwrap();
    for entry in std::fs::read_dir(typed_dir.path()).unwrap() {
        let entry = entry.unwrap();
        assert_eq!(
            std::fs::read(entry.path()).unwrap(),
            std::fs::read(old_dir.path().join(entry.file_name())).unwrap()
        );
    }
    let local_path = super::super::agent_help_local_path(&typed_path);
    std::fs::write(&local_path, serde_json::to_vec(&help).unwrap()).unwrap();
    let (local, error, remote) = target.read_agent_help_layers();
    assert_eq!(local, help);
    assert_eq!(error, None);
    assert_eq!(remote, super::super::load_remote_agent_help_file(&old_path));
    std::fs::write(&local_path, b"malformed").unwrap();
    let (local, error, _) = target.read_agent_help_layers();
    let (old_local, old_error) = super::super::load_local_agent_help_file(&typed_path);
    assert_eq!(local, old_local);
    assert_eq!(error, old_error);
    assert!(!typed_path.exists());
}
