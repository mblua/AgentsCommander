use super::*;

#[test]
fn p03_readers_select_sidecar_without_migration_and_preserve_unrelated_on_conflict() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    let a = real_ac_project_path(temp.path(), "A");
    let b = real_ac_project_path(temp.path(), "B");
    let legacy = serde_json::json!({"defaultShell":"sentinel","defaultShellArgs":[],"agents":[],"pr\
        ojectPaths":[a],"projectPath":a});
    std::fs::write(&path, legacy.to_string()).unwrap();
    let original = std::fs::read(&path).unwrap();
    assert_eq!(
        super::super::load_settings_for_cli_strict_from_path(Some(&path))
            .unwrap()
            .project_paths,
        vec![a.clone()]
    );
    assert_eq!(std::fs::read(&path).unwrap(), original);
    assert!(!super::super::project_paths_path(&path).exists());
    std::fs::write(
        super::super::project_paths_path(&path),
        serde_json::json!({"projectPaths":[b]}).to_string(),
    )
    .unwrap();
    assert!(super::super::load_settings_for_cli_strict_from_path(Some(&path)).is_err());
    let failed = super::super::load_settings_for_cli_from_path(&path);
    assert_eq!(failed.default_shell, "sentinel");
    assert!(failed.project_paths.is_empty());
    assert!(failed.project_paths_persistence_error.is_some());
    let app = super::super::load_settings_from_path(&path);
    assert!(app.project_paths_persistence_error.is_some());
    assert!(app.root_token.is_none());
    assert_eq!(std::fs::read(&path).unwrap(), original);
    std::fs::write(
        &path,
        r#"{"defaultShell":"sentinel","defaultShellArgs":[],"agents":[]}"#,
    )
    .unwrap();
    assert_eq!(
        super::super::load_settings_for_cli_strict_from_path(Some(&path))
            .unwrap()
            .project_paths,
        vec![b]
    );
    std::fs::write(super::super::project_paths_path(&path), b"{}").unwrap();
    let mut stale = failed;
    stale.project_paths = vec![a];
    super::super::refresh_and_decode_project_paths_from_path(&mut stale, &path).unwrap();
    assert!(stale.project_paths.is_empty());
    assert!(stale.project_paths_persistence_error.is_none());
    std::fs::remove_file(&path).unwrap();
    assert!(
        super::super::load_settings_for_cli_strict_from_path(Some(&path))
            .unwrap()
            .project_paths
            .is_empty()
    );
    assert!(!path.exists());
}

#[test]
fn p03_preserve_and_cas_strip_six_keys_and_do_not_write_project_drafts() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    let a = real_ac_project_path(temp.path(), "A");
    let raw = serde_json::json!({"defaultShell":"sentinel","defaultShellArgs":[],"agents":[],"proje\
        ctPaths":[a],"projectPathsRelativeToInstance":[null],"projectPath":a,"projectPathRelativeTo\
        Instance":null,"archivedProjectPaths":[],"archivedProjectPathsRelativeToInstance":[],"defau\
        ltShell":"sentinel"});
    std::fs::write(&path, raw.to_string()).unwrap();
    let original = std::fs::read(&path).unwrap();
    let mut settings = super::super::load_settings_for_cli_strict_from_path(Some(&path)).unwrap();
    settings.project_paths = vec!["renderer draft".into()];
    let written =
        super::super::save_settings_to_path_preserving_project_paths(&settings, &path).unwrap();
    assert_eq!(written.project_paths, vec![a]);
    let destination = super::super::project_paths_path(&path);
    let sidecar = std::fs::read(&destination).unwrap();
    assert_eq!(
        std::fs::read(path.with_file_name(
            super::super::super::instance_artifacts::PROJECT_PATHS_BACKUP_FILE_NAME
        ))
        .unwrap(),
        original
    );
    let managed: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert!(super::super::raw_project_projection(managed.as_object().unwrap()).is_empty());
    let cas = super::super::compare_and_set_terminal_snapshots_enabled_at_path(
        &written, &path, true, false,
    )
    .unwrap();
    assert!(!cas.terminal_snapshots_enabled);
    assert_eq!(std::fs::read(&destination).unwrap(), sidecar);
    let security =
        super::super::read_terminal_snapshot_security_settings_strict_from_path(&path).unwrap();
    assert!(!security.terminal_snapshots_enabled);
    assert_eq!(security.project_paths, cas.project_paths);
    let managed: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert!(super::super::raw_project_projection(managed.as_object().unwrap()).is_empty());
    std::fs::write(&destination, r#"{"projectPaths":"wrong","unknown":"keep"}"#).unwrap();
    let sidecar = std::fs::read(&destination).unwrap();
    super::super::save_settings_to_path_preserving_project_paths(&cas, &path).unwrap();
    assert_eq!(std::fs::read(destination).unwrap(), sidecar);
}

#[test]
fn p03_privileged_source_choice_and_disclosure_fail_closed() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    let a = "C:/authorized-A";
    let b = "C:/legacy-B";
    std::fs::write(&path, r#"{"terminalSnapshotsEnabled":false}"#).unwrap();
    let destination = super::super::project_paths_path(&path);
    std::fs::write(
        &destination,
        serde_json::json!({"projectPaths":[a]}).to_string(),
    )
    .unwrap();
    assert_eq!(
        super::super::read_pty_input_project_paths_strict_from_path(&path).unwrap(),
        Some(vec![a.into()])
    );
    let security =
        super::super::read_terminal_snapshot_security_settings_strict_from_path(&path).unwrap();
    assert!(!security.terminal_snapshots_enabled);
    assert_eq!(security.project_paths, vec![a]);
    std::fs::write(
        &path,
        serde_json::json!({"terminalSnapshotsEnabled":true,"projectPaths":[b]}).to_string(),
    )
    .unwrap();
    assert!(super::super::read_pty_input_project_paths_strict_from_path(&path).is_err());
    assert!(
        super::super::read_terminal_snapshot_security_settings_strict_from_path(&path).is_err()
    );
    std::fs::write(&path, serde_json::json!({"projectPaths":[a]}).to_string()).unwrap();
    assert_eq!(
        super::super::read_terminal_snapshot_security_settings_strict_from_path(&path)
            .unwrap()
            .project_paths,
        vec![a]
    );
    std::fs::write(&destination, b"{\"projectPaths\":[],\"projectPaths\":[]}").unwrap();
    assert!(super::super::read_pty_input_project_paths_strict_from_path(&path).is_err());
    std::fs::write(&path, b"{}").unwrap();
    std::fs::write(&destination, b"{\"projectPaths\":null}").unwrap();
    assert_eq!(
        super::super::read_pty_input_project_paths_strict_from_path(&path).unwrap(),
        Some(vec![])
    );
    assert!(
        super::super::read_terminal_snapshot_security_settings_strict_from_path(&path).is_err()
    );
}

#[test]
fn p03_loader_reader_first_holds_lock_until_capture_and_decode() {
    use std::sync::mpsc;
    use std::time::Duration;
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    let a = real_ac_project_path(temp.path(), "A");
    std::fs::write(
        &path,
        serde_json::json!({"defaultShell":"sentinel","defaultShellArgs":[],"agents":[],"project\
                Paths":[a]})
        .to_string(),
    )
    .unwrap();
    let (captured_tx, captured_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let reader_path = path.clone();
    let reader = std::thread::spawn(move || {
        super::super::PROJECT_SOURCE_CAPTURE_HOOK.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move |_| {
                captured_tx.send(()).unwrap();
                release_rx.recv_timeout(Duration::from_secs(10)).unwrap();
            }))
        });
        let result = super::super::load_settings_for_cli_strict_from_path(Some(&reader_path));
        super::super::PROJECT_SOURCE_CAPTURE_HOOK.with(|hook| *hook.borrow_mut() = None);
        result.unwrap()
    });
    captured_rx.recv_timeout(Duration::from_secs(10)).unwrap();
    let (attempt_tx, attempt_rx) = mpsc::channel();
    let (owned_tx, owned_rx) = mpsc::channel();
    let writer_path = path.clone();
    let writer = std::thread::spawn(move || {
        attempt_tx.send(()).unwrap();
        let token = super::super::empty_project_transaction(&writer_path, "test-lock").unwrap();
        owned_tx.send(()).unwrap();
        super::super::ensure_project_paths_migrated_locked(&token).unwrap();
    });
    attempt_rx.recv_timeout(Duration::from_secs(10)).unwrap();
    assert!(owned_rx.recv_timeout(Duration::from_millis(100)).is_err());
    assert!(!super::super::project_paths_path(&path).exists());
    release_tx.send(()).unwrap();
    let settings = reader.join().unwrap();
    assert_eq!(settings.project_paths, vec![a.clone()]);
    owned_rx.recv_timeout(Duration::from_secs(10)).unwrap();
    writer.join().unwrap();
    assert_eq!(
        super::super::load_settings_for_cli_strict_from_path(Some(&path))
            .unwrap()
            .project_paths,
        vec![a]
    );
}

#[test]
fn p03_loader_migrator_first_timeout_never_authorizes_and_retry_recovers() {
    use std::sync::mpsc;
    use std::time::Duration;
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    let a = real_ac_project_path(temp.path(), "A");
    std::fs::write(
        &path,
        serde_json::json!({"defaultShell":"sentinel","defaultShellArgs":[],"agents":[],"project\
                Paths":[a]})
        .to_string(),
    )
    .unwrap();
    let (published_tx, published_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let writer_path = path.clone();
    let writer = std::thread::spawn(move || {
        let token = super::super::empty_project_transaction(&writer_path, "test-lock").unwrap();
        super::super::ensure_project_paths_migrated_with(&token, &mut |stage, _| {
            if stage == "destination_published" {
                published_tx.send(()).unwrap();
                release_rx.recv_timeout(Duration::from_secs(10)).unwrap();
            }
            Ok(())
        })
        .unwrap();
    });
    published_rx.recv_timeout(Duration::from_secs(10)).unwrap();
    assert!(super::super::load_settings_for_cli_strict_from_path(Some(&path)).is_err());
    let failed = super::super::load_settings_for_cli_from_path(&path);
    assert!(failed.project_paths.is_empty());
    assert!(failed.project_paths_persistence_error.is_some());
    release_tx.send(()).unwrap();
    writer.join().unwrap();
    let recovered = super::super::load_settings_for_cli_strict_from_path(Some(&path)).unwrap();
    assert_eq!(recovered.project_paths, vec![a]);
    assert!(recovered.project_paths_persistence_error.is_none());
}

#[test]
fn p03_setup_rollback_restores_exact_raw_projection_under_same_token() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    let a = real_ac_project_path(temp.path(), "A");
    let b = real_ac_project_path(temp.path(), "B");
    let raw = serde_json::json!({"defaultShell":"sentinel","defaultShellArgs":[],"agents":[],"proje\
        ctPath":a,"projectPaths":[a,"C:/missing"],"projectPathsRelativeToInstance":[null,null]});
    std::fs::write(&path, raw.to_string()).unwrap();
    let mut token = super::super::begin_project_paths_transaction_at_path(
        &super::super::AppSettings::default(),
        &path,
    )
    .unwrap();
    let before = token.raw_project_projection().unwrap();
    let mut candidate = token.fresh_settings().clone();
    token.refresh_raw_projects(&mut candidate).unwrap();
    candidate.project_paths.push(b);
    super::super::resync_project_state_from_runtime(&mut candidate);
    token.commit_project_groups(&candidate, true, true).unwrap();
    token.restore_raw_project_projection(&before).unwrap();
    assert_eq!(token.raw_project_projection().unwrap(), before);
    let managed: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert!(super::super::raw_project_projection(managed.as_object().unwrap()).is_empty());
}

#[test]
fn project_migration_fault_reopen_and_backup_recovery() {
    let raw = serde_json::json!({"projectPaths":["C:/missing"],"projectPath":null,"unknown":{"kee\
        p":true}});
    for point in [
        "backup_create",
        "backup_prefix",
        "backup_write",
        "backup_sync",
        "backup_before_rename",
        "backup_published",
        "destination_before_write",
        "destination_published",
        "destination_readback",
        "source_cleanup",
    ] {
        let (temp, path, bytes) = migration_fixture(&raw);
        let transaction = migration_transaction(&path);
        let mut orphan = None;
        assert!(
            super::super::ensure_project_paths_migrated_with(&transaction, &mut |stage, target| {
                if stage == point {
                    if target
                        .extension()
                        .is_some_and(|extension| extension == "tmp")
                        && target.exists()
                    {
                        orphan = Some((target.to_path_buf(), std::fs::read(target).unwrap()));
                    }
                    return Err(super::super::project_migration_error(
                        target,
                        super::super::SettingsSaveStage::TempWrite,
                        "injected backup/publication failure",
                    ));
                }
                Ok(())
            })
            .is_err(),
            "{point}"
        );
        assert_eq!(std::fs::read(&path).unwrap(), bytes, "{point}");
        let backup = temp
            .path()
            .join(super::super::super::instance_artifacts::PROJECT_PATHS_BACKUP_FILE_NAME);
        if point.starts_with("backup_") && point != "backup_published" {
            assert!(!backup.exists());
        }
        drop(transaction);
        super::super::ensure_project_paths_migrated_locked(&migration_transaction(&path)).unwrap();
        assert_eq!(std::fs::read(backup).unwrap(), bytes);
        if let Some((orphan, original)) = orphan {
            assert_eq!(std::fs::read(orphan).unwrap(), original);
        }
        assert_eq!(
            super::super::read_raw_project_source(&super::super::project_paths_path(&path))
                .unwrap()
                .unwrap()
                .object,
            super::super::raw_project_projection(raw.as_object().unwrap())
        );
    }
    let (temp, path, bytes) = migration_fixture(&raw);
    let backup = temp
        .path()
        .join(super::super::super::instance_artifacts::PROJECT_PATHS_BACKUP_FILE_NAME);
    std::fs::write(&backup, b"historical partial backup").unwrap();
    let transaction = migration_transaction(&path);
    let error = super::super::ensure_project_paths_migrated_locked(&transaction).unwrap_err();
    assert!(matches!(
        error.cause,
        super::super::SettingsSaveCause::Semantic(
            super::super::SettingsSaveReason::ProjectPathsBackupMismatch
        )
    ));
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    assert!(!super::super::project_paths_path(&path).exists());
    // Simulate the specified stopped-operator recovery, inside this fixture only.
    let recovered = temp
        .path()
        .join("project-paths.pre-migration.recovered-20261008-unique.no-git.json");
    assert!(!recovered.exists());
    std::fs::rename(&backup, &recovered).unwrap();
    super::super::ensure_project_paths_migrated_locked(&transaction).unwrap();
    assert_eq!(
        std::fs::read(recovered).unwrap(),
        b"historical partial backup"
    );
    assert_eq!(std::fs::read(backup).unwrap(), bytes);
}

#[test]
fn project_migration_atomic_failure_matrix() {
    for target_source in [false, true] {
        for stage in [
            super::super::SettingsSaveStage::TempCreate,
            super::super::SettingsSaveStage::TempWrite,
            super::super::SettingsSaveStage::TempSync,
            super::super::SettingsSaveStage::AtomicReplace,
            super::super::SettingsSaveStage::PostWriteRead,
        ] {
            let (_temp, path, bytes) =
                migration_fixture(&serde_json::json!({"projectPaths":["C:/gone"],"unknown":9}));
            let target = if target_source {
                path.clone()
            } else {
                super::super::project_paths_path(&path)
            };
            let transaction = migration_transaction(&path);
            super::super::PROJECT_MIGRATION_ATOMIC_FAULT
                .with(|slot| *slot.borrow_mut() = Some((target, stage)));
            let result = super::super::ensure_project_paths_migrated_locked(&transaction);
            super::super::PROJECT_MIGRATION_ATOMIC_FAULT.with(|slot| *slot.borrow_mut() = None);
            assert!(result.is_err());
            if !target_source || stage != super::super::SettingsSaveStage::PostWriteRead {
                assert_eq!(std::fs::read(&path).unwrap(), bytes);
            }
            drop(transaction);
            super::super::ensure_project_paths_migrated_locked(&migration_transaction(&path))
                .unwrap();
            assert_eq!(
                super::super::read_raw_project_source(&super::super::project_paths_path(&path))
                    .unwrap()
                    .unwrap()
                    .object["projectPaths"],
                serde_json::json!(["C:/gone"])
            );
            assert!(super::super::raw_project_projection(
                &super::super::read_raw_project_source(&path)
                    .unwrap()
                    .unwrap()
                    .object
            )
            .is_empty());
        }
    }
}

#[test]
fn project_migration_equal_retry_skips_backup_and_keeps_unrelated_changes() {
    let (temp, path, _) =
        migration_fixture(&serde_json::json!({"projectPaths":["C:/gone"],"unknown":1}));
    let transaction = migration_transaction(&path);
    assert!(super::super::ensure_project_paths_migrated_with(
        &transaction,
        &mut |stage, target| {
            if stage == "source_cleanup" {
                return Err(super::super::project_migration_error(
                    target,
                    super::super::SettingsSaveStage::AtomicReplace,
                    "cleanup failed",
                ));
            }
            Ok(())
        }
    )
    .is_err());
    drop(transaction);
    std::fs::write(&path, br#"{"projectPaths":["C:/gone"],"unknown":2}"#).unwrap();
    let backup = temp
        .path()
        .join(super::super::super::instance_artifacts::PROJECT_PATHS_BACKUP_FILE_NAME);
    let backup_bytes = std::fs::read(&backup).unwrap();
    super::super::ensure_project_paths_migrated_locked(&migration_transaction(&path)).unwrap();
    assert_eq!(
        super::super::read_raw_project_source(&path)
            .unwrap()
            .unwrap()
            .object["unknown"],
        2
    );
    assert_eq!(std::fs::read(&backup).unwrap(), backup_bytes);
    std::fs::write(&path, br#"{"projectPaths":["different"],"unknown":3}"#).unwrap();
    assert!(
        super::super::ensure_project_paths_migrated_locked(&migration_transaction(&path)).is_err()
    );
    std::fs::write(&path, br#"{"projectPaths":["C:/gone"],"unknown":3}"#).unwrap();
    std::fs::remove_file(&backup).unwrap();
    std::fs::create_dir(&backup).unwrap();
    super::super::ensure_project_paths_migrated_locked(&migration_transaction(&path)).unwrap();
    assert_eq!(
        super::super::read_raw_project_source(&path)
            .unwrap()
            .unwrap()
            .object["unknown"],
        3
    );
    assert!(
        backup.is_dir(),
        "equal remnant must never read historical backup"
    );
}

#[cfg(unix)]
#[test]
fn p03_final_retained_root_check_rejects_persistent_replacement() {
    let temp = tempfile::tempdir().unwrap();
    let parent = temp.path().join("selected");
    std::fs::create_dir(&parent).unwrap();
    let path = parent.join("settings.json");
    let retired = temp.path().join("retired");
    super::super::PROJECT_SOURCE_CAPTURE_HOOK.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move |path| {
            let parent = path.parent().unwrap();
            std::fs::rename(parent, &retired).unwrap();
            std::fs::create_dir(parent).unwrap();
            let mut replacement =
                serde_json::to_value(super::super::AppSettings::default()).unwrap();
            replacement["roomNumberMask"] = serde_json::json!("###");
            std::fs::write(path, serde_json::to_vec(&replacement).unwrap()).unwrap();
        }));
    });
    let captured = super::super::capture_project_settings(&path, false);
    super::super::PROJECT_SOURCE_CAPTURE_HOOK.with(|hook| *hook.borrow_mut() = None);
    let (settings, _, original) = captured.unwrap();
    assert_eq!(settings.room_number_mask, "###");
    assert_eq!(
        settings.project_paths_persistence_error.as_deref(),
        Some(super::super::SETTINGS_LOCK_UNAVAILABLE_CODE)
    );
    assert!(settings.project_paths.is_empty());
    assert!(original.is_none());
    assert!(!super::super::project_paths_path(&path).exists());
}

#[test]
fn p03_capture_creates_fresh_parent_without_readonly_data_files() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("fresh/nested/settings.json");
    let (settings, _, _) = super::super::capture_project_settings(&path, false).unwrap();
    assert!(settings.project_paths_persistence_error.is_none());
    assert!(settings.project_paths.is_empty());
    assert!(path.parent().unwrap().is_dir());
    assert!(!path.exists());
    assert!(!super::super::project_paths_path(&path).exists());
}

#[test]
fn p03_physical_agents_io_preserves_logical_registry_and_messages() {
    let temp = tempfile::tempdir().unwrap();
    let io = temp.path().join("physical/settings.json");
    let label = temp.path().join("logical/settings.json");
    std::fs::create_dir_all(io.parent().unwrap()).unwrap();
    std::fs::write(super::super::agents_instance_path(&io), b"{broken").unwrap();
    let projection = serde_json::Map::new();
    let contents = serde_json::to_string(&super::super::AppSettings::default()).unwrap();
    let (mut settings, _) = super::super::parse_settings_json_with_projection_labeled(
        &contents,
        &label.to_string_lossy(),
        Some(&io),
        Some(&label),
        Some(&projection),
    )
    .unwrap();
    assert_eq!(
        settings.agents_layer.unreadable_path(),
        Some(super::super::agents_instance_path(&label).as_path())
    );
    assert!(super::super::agents_layer_unreadable_for_dir(
        label.parent().unwrap()
    ));
    assert!(!super::super::agents_layer_unreadable_for_dir(
        io.parent().unwrap()
    ));
    if let super::super::AgentsLayerState::Unreadable { reason, .. } = &settings.agents_layer {
        assert!(reason.contains(
            &super::super::agents_instance_path(&label)
                .display()
                .to_string()
        ));
        assert!(!reason.contains(
            &super::super::agents_instance_path(&io)
                .display()
                .to_string()
        ));
    } else {
        panic!("physical unreadable agents layer was lost");
    }
    super::super::set_agents_layer_state(
        &mut settings,
        &label,
        super::super::AgentsLayerState::Readable,
    );
}

#[test]
fn p03_labeled_migration_keeps_raw_bytes_and_logical_conflict_paths() {
    let temp = tempfile::tempdir().unwrap();
    let io = temp.path().join("physical/settings.json");
    let label = temp.path().join("logical/settings.json");
    std::fs::create_dir_all(io.parent().unwrap()).unwrap();
    let original = include_bytes!("fixtures/archived_project_paths_settings.json");
    std::fs::write(&io, original).unwrap();
    let _transaction = super::super::empty_project_transaction(&io, "test-lock").unwrap();
    super::super::ensure_project_paths_migrated_at_labeled_path(&io, &label, &mut |_, _| Ok(()))
        .unwrap();
    let backup =
        io.with_file_name(crate::config::instance_artifacts::PROJECT_PATHS_BACKUP_FILE_NAME);
    assert_eq!(std::fs::read(backup).unwrap(), original);
    let cleaned: serde_json::Value = serde_json::from_slice(&std::fs::read(&io).unwrap()).unwrap();
    assert!(super::super::raw_project_projection(cleaned.as_object().unwrap()).is_empty());
    assert_eq!(cleaned["roomNumberMask"], "###");
    let selected = super::super::select_project_source_at_labeled_path_with_limit(
        &io,
        &label,
        16 * 1024 * 1024,
    )
    .unwrap();
    let raw: serde_json::Value = serde_json::from_slice(original).unwrap();
    assert_eq!(
        selected.projection,
        super::super::raw_project_projection(raw.as_object().unwrap())
    );
    let before = std::fs::read(super::super::project_paths_path(&io)).unwrap();
    super::super::ensure_project_paths_migrated_at_labeled_path(&io, &label, &mut |_, _| Ok(()))
        .unwrap();
    assert_eq!(
        std::fs::read(super::super::project_paths_path(&io)).unwrap(),
        before
    );
    std::fs::write(&io, br#"{"projectPaths":["other"]}"#).unwrap();
    let error = super::super::select_project_source_at_labeled_path_with_limit(
        &io,
        &label,
        16 * 1024 * 1024,
    )
    .unwrap_err();
    let message = error.legacy_outward.into_general_string();
    assert!(message.contains(&label.display().to_string()));
    assert!(message.contains(
        &super::super::project_paths_path(&label)
            .display()
            .to_string()
    ));
    assert!(!message.contains(&io.display().to_string()));
}

#[test]
fn e2432_project_capture_syntax_admission_all_participants() {
    for case in [
        "syntax",
        "overlay-syntax",
        "overlay-syntax-only",
        "overlay-io",
        "main-duplicate",
        "overlay-duplicate",
        "overlay-binding",
        "overlay-quarantine",
        "overlay-conflict",
        "sidecar-io",
    ] {
        project_capture_syntax_case(case);
    }
}

fn project_capture_syntax_case(case: &str) {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    let project = temp.path().join("project");
    std::fs::create_dir_all(project.join(".ac")).unwrap();
    let mut main = serde_json::json!({
        "rootToken":"stable", "projectPaths":[project], "projectPath":project
    })
    .to_string();
    if case == "main-duplicate" {
        main.pop();
        main.push_str(",\"unknown\":1,\"unknown\":1}");
    }
    std::fs::write(&path, &main).unwrap();
    let sidecar = super::super::agents_instance_path(&path);
    if case == "sidecar-io" {
        std::fs::create_dir(&sidecar).unwrap();
    } else if case == "overlay-syntax-only" {
        std::fs::write(&sidecar, b"{}").unwrap();
    } else {
        std::fs::write(&sidecar, b"{broken agents").unwrap();
    }
    let overlay = path.with_file_name(super::super::SETTINGS_LOCAL_OVERRIDE_FILE_NAME);
    if case == "overlay-io" {
        std::fs::create_dir(&overlay).unwrap();
    }
    let overlay_bytes = match case {
        "overlay-syntax" | "overlay-syntax-only" => Some("{broken overlay".to_owned()),
        "overlay-duplicate" => Some(r#"{"agents":[],"unknown":1,"unknown":1}"#.to_owned()),
        "overlay-binding" => {
            Some(r#"{"codingAgentProfiles":{"profileBindingVersion":9}}"#.to_owned())
        }
        "overlay-quarantine" => {
            Some(r#"{"codingAgentProfiles":{"legacyProfileOrphans":null}}"#.to_owned())
        }
        "overlay-conflict" => Some(
            serde_json::json!({
                "agents":[b3_agent("alpha", "A"), b3_agent("beta", "B")],
                "codingAgentProfiles":{"profilesByAgent":{
                    "alpha":{"A":{"notes":"first"}}, "beta":{"A":{"notes":"second"}}
                }}
            })
            .to_string(),
        ),
        _ => None,
    };
    if let Some(bytes) = &overlay_bytes {
        std::fs::write(&overlay, bytes).unwrap();
    }
    let capture = super::super::collect_registered_profile_gate(&path);
    let migrates = matches!(
        case,
        "syntax" | "overlay-syntax" | "overlay-syntax-only" | "overlay-io"
    );
    assert_eq!(capture.admits_project_migration(&path), migrates, "{case}");
    assert_eq!(
        capture.ordinary_error().is_some(),
        case != "overlay-syntax-only",
        "{case}"
    );
    if matches!(
        case,
        "overlay-syntax" | "overlay-syntax-only" | "overlay-io"
    ) {
        assert!(!capture
            .participant_documents()
            .iter()
            .any(|(document, _)| document == &overlay));
    }
    super::super::read_settings_for_startup(&path);
    assert_eq!(
        super::super::project_paths_path(&path).exists(),
        migrates,
        "{case}"
    );
    if !migrates {
        assert_eq!(std::fs::read_to_string(&path).unwrap(), main, "{case}");
        assert!(!path
            .with_file_name(super::super::super::instance_artifacts::PROJECT_PATHS_BACKUP_FILE_NAME)
            .exists());
    }
    if case == "overlay-syntax-only" {
        assert_eq!(std::fs::read(&sidecar).unwrap(), b"{}");
    } else if case != "sidecar-io" {
        assert_eq!(std::fs::read(&sidecar).unwrap(), b"{broken agents");
    }
    if let Some(bytes) = overlay_bytes {
        assert_eq!(std::fs::read_to_string(&overlay).unwrap(), bytes);
    }
    if case == "overlay-io" {
        assert!(overlay.is_dir());
    }
}

#[test]
fn e2432_project_capture_rechecks_malformed_bytes_and_absence() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    std::fs::write(&path, b"{}").unwrap();
    let sidecar = super::super::agents_instance_path(&path);
    std::fs::write(&sidecar, b"{broken").unwrap();
    let captured = super::super::collect_registered_profile_gate(&path);
    assert!(captured.admits_project_migration(&path));
    assert!(super::super::recheck_registered_profile_gate(&captured.documents).is_ok());
    std::fs::write(&sidecar, b"{changed").unwrap();
    assert!(super::super::recheck_registered_profile_gate(&captured.documents).is_err());
    std::fs::write(&sidecar, b"{broken").unwrap();
    std::fs::write(
        path.with_file_name(super::super::SETTINGS_LOCAL_OVERRIDE_FILE_NAME),
        b"{}",
    )
    .unwrap();
    assert!(super::super::recheck_registered_profile_gate(&captured.documents).is_err());
}

#[test]
fn e2432_editable_legacy_action_preserves_opaque_canonical_fixed_point() {
    let owner = serde_json::json!({
        "agents":[b3_agent("alpha", "A")],
        "codingAgentProfiles":{
            "schemaVersion":2,"letters":{"old":true},
            "profileSlots":{"A":{"name":"opaque", "extension":7}},
            "profilesByAgent":{"alpha":{"A":{"command":"--x", "argv":["opaque"], "extension":9}}}
        }
    });
    assert!(super::super::actionable_profile_legacy_work(&owner, true).unwrap());
    let projected = super::super::project_physical_profiles(&owner, true).unwrap();
    let mut canonical = super::super::canonical_profile_source(&owner, &projected, true).unwrap();
    canonical["codingAgentProfiles"]
        .as_object_mut()
        .unwrap()
        .remove("letters");
    assert!(super::super::legacy_profiles_shape_present(
        canonical.as_object().unwrap()
    ));
    assert!(!super::super::actionable_profile_legacy_work(&canonical, true).unwrap());
    assert_eq!(
        canonical["codingAgentProfiles"]["profileSlots"]["A"]["name"],
        "opaque"
    );
    let hash = super::super::configuration_identity("claude", &[]);
    assert_eq!(
        canonical["codingAgentProfiles"]["profilesByBaseHash"][&hash]["A"]["argv"],
        serde_json::json!(["opaque"])
    );
    assert_eq!(
        canonical["codingAgentProfiles"]["profilesByBaseHash"][&hash]["A"]["extension"],
        9
    );
    canonical["codingAgentProfiles"]["matrix"] = serde_json::json!({"old":true});
    assert!(super::super::actionable_profile_legacy_work(&canonical, true).unwrap());
}

#[test]
fn selected_project_decode_fallback_retains_overlay_and_agents_state() {
    for startup in [false, true] {
        for unreadable in [false, true] {
            assert_selected_project_decode_fallback_case(startup, unreadable);
        }
    }
}

pub(super) fn n1_test_overlay(kind: &str) -> serde_json::Value {
    if kind == "agents" {
        serde_json::json!({"agents":[b3_agent("overlay-only", "Overlay")]})
    } else {
        serde_json::json!({"codingAgentProfiles":{
            "profileSlots":{"A":{"label":"overlay-slot"}},
            "profilesByAgent":{"overlay-only":{"A":{"additionalArguments":"--overlay"}}},
            "defaultProfileByAgent":{"overlay-matrix":"A"}
        }})
    }
}
