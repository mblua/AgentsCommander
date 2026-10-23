use super::*;

#[cfg(windows)]
#[test]
fn windows_atomic_replace_failure_retains_os_metadata() {
    use std::os::windows::fs::OpenOptionsExt as _;
    use windows_sys::Win32::Storage::FileSystem::{FILE_SHARE_READ, FILE_SHARE_WRITE};

    let temp = tempfile::TempDir::new().unwrap();
    let path = temp.path().join("settings.json");
    let baseline = super::super::AppSettings::default();
    super::super::save_settings_to_path(&baseline, &path).unwrap();
    let original = std::fs::read(&path).unwrap();

    let lock_guard =
        super::super::SettingsFileLock::acquire(&path, std::time::Duration::from_secs(1)).unwrap();
    let retained_handle = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .open(&path)
        .unwrap();

    let mut candidate_value = serde_json::to_value(&baseline).unwrap();
    candidate_value.as_object_mut().unwrap().insert(
        "rootToken".to_string(),
        serde_json::Value::String("issue-1330-candidate-root".to_string()),
    );
    let candidate: super::super::AppSettings = serde_json::from_value(candidate_value).unwrap();
    let error = super::super::save_settings_value_locked(
        &candidate,
        &path,
        super::super::ProjectWriteMode::Preserve,
        super::super::TerminalSnapshotGateWriteMode::Preserve,
        super::super::MainWindowPlacementWriteMode::Preserve,
    )
    .unwrap_err();

    assert_eq!(error.stage, super::super::SettingsSaveStage::AtomicReplace);
    assert!(matches!(
        &error.legacy_outward,
        super::super::SettingsSaveLegacyOutward::SettingsSaveFailed
    ));
    let recorded_target = error.target_path.clone();
    let recorded_temp = error.temp_path.clone().unwrap();
    let recorded_pid = error.pid;
    let (source_raw, source_kind) = match &error.cause {
        super::super::SettingsSaveCause::Io(source) => (source.raw_os_error(), source.kind()),
        _ => panic!("expected retained atomic replacement I/O source"),
    };
    assert!(source_raw.is_some());
    assert_eq!(recorded_target.as_path(), path.as_path());
    assert_eq!(recorded_pid, std::process::id());
    assert_eq!(recorded_temp.parent(), Some(temp.path()));
    let temp_name = recorded_temp.file_name().unwrap().to_string_lossy();
    assert!(temp_name.starts_with(&format!(
        "settings.30.instance.no-git.json.{}.",
        recorded_pid
    )));
    assert!(temp_name.ends_with(".tmp"));

    let (records, _capture_guard) = capture_settings_save_diagnostics();
    let outward = super::super::report_settings_save_error(
        error,
        super::super::SettingsSaveReportSurface::GeneralSettings,
    );
    assert_eq!(outward, "settings_save_failed");
    let records = CapturedRecordsSnapshot::snapshot(&records);
    assert_eq!(records.len(), 1);
    let fields = parse_settings_save_diagnostic(&records[0]);
    assert_eq!(fields["code"], "settings_save_failed");
    assert_eq!(fields["stage"], "atomic_replace");
    assert_eq!(fields["reason"], "none");
    assert_eq!(fields["source_kind"], "io");
    assert_eq!(fields["raw_os_error"], source_raw.unwrap().to_string(),);
    assert_eq!(
        fields["io_kind"],
        expected_io_kind_from_literal_table(source_kind),
    );
    assert_eq!(fields["pid"], recorded_pid.to_string());
    assert_eq!(
        fields["target_path"],
        expected_rendered_diagnostic_path(&recorded_target),
    );
    assert_eq!(
        fields["temp_path"],
        expected_rendered_diagnostic_path(&recorded_temp),
    );
    assert_eq!(std::fs::read(&path).unwrap(), original);
    assert!(!recorded_temp.exists());
    drop(retained_handle);
    drop(lock_guard);
}

#[cfg(windows)]
#[test]
fn windows_settings_replace_retries_access_denied() {
    let mut calls = 0;
    let mut sleeps = Vec::new();

    super::super::replace_settings_file_atomic_with_retry(
        || {
            calls += 1;
            if calls == 1 {
                Err(std::io::Error::from_raw_os_error(5))
            } else {
                Ok(())
            }
        },
        |duration| sleeps.push(duration.as_millis() as u64),
    )
    .unwrap();

    assert_eq!(calls, 2);
    assert_eq!(sleeps, vec![15]);
}

#[cfg(windows)]
#[test]
fn windows_settings_replace_retries_sharing_violation() {
    let mut calls = 0;
    let mut sleeps = Vec::new();

    super::super::replace_settings_file_atomic_with_retry(
        || {
            calls += 1;
            if calls == 1 {
                Err(std::io::Error::from_raw_os_error(32))
            } else {
                Ok(())
            }
        },
        |duration| sleeps.push(duration.as_millis() as u64),
    )
    .unwrap();

    assert_eq!(calls, 2);
    assert_eq!(sleeps, vec![15]);
}

#[cfg(windows)]
#[test]
fn windows_settings_replace_non_retryable_fails_immediately() {
    let mut calls = 0;
    let mut sleeps = Vec::new();

    let error = super::super::replace_settings_file_atomic_with_retry(
        || {
            calls += 1;
            Err(std::io::Error::from_raw_os_error(87))
        },
        |duration| sleeps.push(duration.as_millis() as u64),
    )
    .unwrap_err();

    assert_eq!(error.raw_os_error(), Some(87));
    assert_eq!(calls, 1);
    assert!(sleeps.is_empty());
}

#[cfg(windows)]
#[test]
fn windows_settings_replace_exhaustion_returns_final_error() {
    let mut calls = 0;
    let mut sleeps = Vec::new();

    let error = super::super::replace_settings_file_atomic_with_retry(
        || {
            calls += 1;
            let raw_os_error = if calls < 6 { 5 } else { 32 };
            Err(std::io::Error::from_raw_os_error(raw_os_error))
        },
        |duration| sleeps.push(duration.as_millis() as u64),
    )
    .unwrap_err();

    assert_eq!(calls, 6);
    assert_eq!(sleeps, vec![15, 30, 60, 120, 240]);
    assert_eq!(error.raw_os_error(), Some(32));
}

#[cfg(any(unix, windows))]
#[test]
fn startup_source_lock_failure_blocks_all_save_triggers() {
    for case in [
        "missing_root_token",
        "issue_248_migration",
        "coding_agent_profile_v1_to_v2",
        "profile_repair",
    ] {
        let temp = tempfile::TempDir::new().unwrap();
        let path = temp.path().join("settings.json");
        let mut value = serde_json::to_value(super::super::AppSettings::default()).unwrap();
        let object = value.as_object_mut().unwrap();
        if case != "missing_root_token" {
            object.insert(
                "rootToken".to_string(),
                serde_json::Value::String(format!("issue-1330-root-{case}")),
            );
        }
        match case {
            "missing_root_token" => {
                object.remove("rootToken");
            }
            "issue_248_migration" => {
                object.remove("restoreCoordinatorWakeState");
                object.insert(
                    "startOnlyCoordinators".to_string(),
                    serde_json::Value::Bool(true),
                );
            }
            "coding_agent_profile_v1_to_v2" => {
                object["codingAgentProfiles"]
                    .as_object_mut()
                    .unwrap()
                    .insert(
                        "schemaVersion".to_string(),
                        serde_json::Value::Number(1.into()),
                    );
            }
            "profile_repair" => {
                object["codingAgentProfiles"]
                    .as_object_mut()
                    .unwrap()
                    .insert(
                        "profileSlots".to_string(),
                        serde_json::Value::Object(serde_json::Map::new()),
                    );
            }
            _ => unreachable!(),
        }
        let original = serde_json::to_vec_pretty(&value).unwrap();
        std::fs::write(&path, &original).unwrap();
        let held_lock =
            super::super::SettingsFileLock::acquire(&path, std::time::Duration::from_secs(1))
                .unwrap();
        let (records, _capture_guard) = capture_settings_save_diagnostics();

        let loaded = super::super::load_settings_from_path(&path);

        assert_eq!(std::fs::read(&path).unwrap(), original, "case {case}");
        let records = records.borrow();
        assert_eq!(records.len(), 1, "case {case}");
        let fields = parse_settings_save_diagnostic(&records[0]);
        assert_eq!(fields["code"], "settings_lock_unavailable", "case {case}");
        assert_eq!(fields["stage"], "lock_acquire", "case {case}");
        assert_eq!(fields["reason"], "lock_timed_out", "case {case}");
        assert_eq!(fields["temp_path"], "none", "case {case}");
        assert!(!records[0].contains(&format!("issue-1330-root-{case}")));
        // The source cannot be captured while another process owns the lock.
        // Startup must expose no projects or generated privilege from defaults.
        assert!(loaded.project_paths.is_empty());
        assert!(loaded.project_paths_persistence_error.is_some());
        assert!(loaded.root_token.is_none());
        drop(held_lock);
    }
}

#[test]
fn preserve_disk_gate_failures_are_typed_and_do_not_overwrite() {
    let rows = [
        ("{not json", "malformed"),
        ("[]", "non_object"),
        (
            r#"{"terminalSnapshotsEnabled":"invalid"}"#,
            "invalid_terminal_snapshot_setting",
        ),
    ];

    for (contents, case) in rows {
        let temp = tempfile::TempDir::new().unwrap();
        let path = temp.path().join("settings.json");
        std::fs::write(&path, contents).unwrap();
        let original = std::fs::read(&path).unwrap();
        let expected = match case {
            "malformed" => {
                let source = serde_json::from_str::<serde_json::Value>(contents).unwrap_err();
                format!(
                    "Refusing to overwrite {}: the existing settings file is not valid JSON \
                            ({})",
                    path.display(),
                    source
                )
            }
            "non_object" => format!(
                "Refusing to overwrite {}: the existing settings root is not a JSON object",
                path.display()
            ),
            "invalid_terminal_snapshot_setting" => "Refusing to overwrite present settings \
                    whose non-project fields are invalid: invalid type: string \"invalid\", \
                    expected a boolean"
                .to_string(),
            _ => unreachable!(),
        };

        let error = super::super::save_settings_value(
            &super::super::AppSettings::default(),
            &path,
            super::super::ProjectWriteMode::Preserve,
        )
        .unwrap_err();

        assert_eq!(
            error.stage,
            super::super::SettingsSaveStage::PreserveDiskGate
        );
        match case {
            "malformed" => {
                assert!(matches!(
                    &error.cause,
                    super::super::SettingsSaveCause::Json(_)
                ));
                assert!(matches!(
                    &error.legacy_outward,
                    super::super::SettingsSaveLegacyOutward::DiskJson(_)
                ));
            }
            "non_object" => {
                assert!(matches!(
                    &error.cause,
                    super::super::SettingsSaveCause::Semantic(
                        super::super::SettingsSaveReason::DiskJsonNotObject
                    )
                ));
                assert!(matches!(
                    &error.legacy_outward,
                    super::super::SettingsSaveLegacyOutward::DiskJsonNotObject(_)
                ));
            }
            "invalid_terminal_snapshot_setting" => {
                assert!(matches!(
                    &error.cause,
                    super::super::SettingsSaveCause::Semantic(
                        super::super::SettingsSaveReason::DiskSettingsValidationRejected
                    )
                ));
                assert!(matches!(
                    &error.legacy_outward,
                    super::super::SettingsSaveLegacyOutward::DiskValidation(_)
                ));
                assert_ne!(expected, "terminal_snapshot_setting_invalid",);
            }
            _ => unreachable!(),
        }
        let (records, _capture_guard) = capture_settings_save_diagnostics();
        let outward = super::super::report_settings_save_error(
            error,
            super::super::SettingsSaveReportSurface::GeneralSettings,
        );
        assert_eq!(outward, expected);
        let captured_records = records.borrow();
        assert_eq!(captured_records.len(), 1);
        let fields = parse_settings_save_diagnostic(&captured_records[0]);
        assert_eq!(fields["code"], "settings_save_failed");
        assert_eq!(fields["stage"], "preserve_disk_gate");
        let expected_reason = match case {
            "malformed" => "none",
            "non_object" => "disk_json_not_object",
            "invalid_terminal_snapshot_setting" => "disk_settings_validation_rejected",
            _ => unreachable!(),
        };
        assert_eq!(fields["reason"], expected_reason);
        assert_eq!(std::fs::read(&path).unwrap(), original);
        assert_no_issue_1330_temp_files(temp.path());
    }
}

#[test]
fn reconcile_disk_gate_failure_uses_reconcile_stage_and_legacy_outward() {
    let temp = tempfile::TempDir::new().unwrap();
    let path = temp.path().join("settings.json");
    std::fs::write(&path, r#"{"terminalSnapshotsEnabled":"invalid"}"#).unwrap();
    let original = std::fs::read(&path).unwrap();
    let error = super::super::save_settings_value(
        &super::super::AppSettings::default(),
        &path,
        super::super::ProjectWriteMode::Reconcile {
            active: false,
            archived: false,
        },
    )
    .unwrap_err();

    assert_eq!(
        error.stage,
        super::super::SettingsSaveStage::ReconcileDiskGate
    );
    assert!(matches!(
        &error.cause,
        super::super::SettingsSaveCause::Semantic(
            super::super::SettingsSaveReason::DiskSettingsValidationRejected
        )
    ));
    assert!(matches!(
        &error.legacy_outward,
        super::super::SettingsSaveLegacyOutward::DiskValidation(_)
    ));
    let (records, _capture_guard) = capture_settings_save_diagnostics();
    let outward = super::super::report_settings_save_error(
        error,
        super::super::SettingsSaveReportSurface::GeneralSettings,
    );
    assert_eq!(
        outward,
        "Refusing to overwrite present settings whose non-project fields are invalid: invalid \
                type: string \"invalid\", expected a boolean"
    );
    assert_ne!(outward, "terminal_snapshot_setting_invalid");
    let records = CapturedRecordsSnapshot::snapshot(&records);
    assert_eq!(records.len(), 1);
    let fields = parse_settings_save_diagnostic(&records[0]);
    assert_eq!(fields["code"], "settings_save_failed");
    assert_eq!(fields["stage"], "reconcile_disk_gate");
    assert_eq!(fields["reason"], "disk_settings_validation_rejected");
    assert_eq!(std::fs::read(&path).unwrap(), original);
    assert_no_issue_1330_temp_files(temp.path());
}

#[test]
fn project_refresh_compatibility_adapter_maps_typed_failures_once() {
    let rows = [
        ("{not json", "malformed"),
        ("[]", "non_object"),
        (
            r#"{"terminalSnapshotsEnabled":"invalid"}"#,
            "invalid_terminal_snapshot_setting",
        ),
    ];

    for (contents, case) in rows {
        let temp = tempfile::TempDir::new().unwrap();
        let path = temp.path().join("settings.json");
        std::fs::write(&path, contents).unwrap();
        let original = std::fs::read(&path).unwrap();
        let expected = match case {
            "malformed" => {
                let source = serde_json::from_str::<serde_json::Value>(contents).unwrap_err();
                format!(
                    "Refusing to overwrite {}: the existing settings file is not valid JSON \
                            ({})",
                    path.display(),
                    source
                )
            }
            "non_object" => format!(
                "Refusing to overwrite {}: the existing settings root is not a JSON object",
                path.display()
            ),
            "invalid_terminal_snapshot_setting" => "Refusing to overwrite present settings \
                    whose non-project fields are invalid: invalid type: string \"invalid\", \
                    expected a boolean"
                .to_string(),
            _ => unreachable!(),
        };
        let mut settings = super::super::AppSettings::default();
        let before = serde_json::to_value(&settings).unwrap();

        let (records, _capture_guard) = capture_settings_save_diagnostics();
        let error = super::super::refresh_and_decode_project_paths_from_path(&mut settings, &path)
            .unwrap_err();

        assert_eq!(error, expected);
        let records = CapturedRecordsSnapshot::snapshot(&records);
        assert_eq!(records.len(), 1);
        let fields = parse_settings_save_diagnostic(&records[0]);
        assert_eq!(fields["code"], "settings_save_failed");
        assert_eq!(fields["stage"], "project_paths_refresh_disk_gate");
        let records = CapturedRecordsSnapshot::snapshot(&records);
        assert_eq!(records.len(), 1);
        let fields = parse_settings_save_diagnostic(&records[0]);
        assert_eq!(fields["code"], "settings_save_failed");
        assert_eq!(fields["stage"], "project_paths_refresh_disk_gate");
        assert_eq!(serde_json::to_value(&settings).unwrap(), before);
        assert_eq!(std::fs::read(&path).unwrap(), original);
        assert_no_issue_1330_temp_files(temp.path());
    }

    let temp = tempfile::TempDir::new().unwrap();
    let path = temp.path().join("settings.json");
    std::fs::create_dir(&path).unwrap();
    let source = std::fs::read_to_string(&path).unwrap_err();
    let expected = format!(
        "Failed to read {} for a project-preserving save (aborting to avoid dropping a \
                project): {}",
        path.display(),
        source
    );
    let mut settings = super::super::AppSettings::default();
    let before = serde_json::to_value(&settings).unwrap();

    let error =
        super::super::refresh_and_decode_project_paths_from_path(&mut settings, &path).unwrap_err();

    assert_eq!(error, expected);
    assert_eq!(serde_json::to_value(&settings).unwrap(), before);
}

#[test]
fn lock_and_writer_failures_keep_static_legacy_outward_strings() {
    let temp = tempfile::TempDir::new().unwrap();
    let settings_path = temp.path().join("settings.json");
    let first_lock =
        super::super::SettingsFileLock::acquire(&settings_path, std::time::Duration::from_secs(1))
            .unwrap();
    let second = super::super::SettingsFileLock::acquire(&settings_path, std::time::Duration::ZERO);
    let lock_error = match second {
        Ok(_) => panic!("second lock unexpectedly succeeded"),
        Err(error) => error,
    };
    assert_eq!(
        lock_error.stage,
        super::super::SettingsSaveStage::LockAcquire
    );
    assert!(matches!(
        &lock_error.cause,
        super::super::SettingsSaveCause::Semantic(super::super::SettingsSaveReason::LockTimedOut)
    ));
    assert!(matches!(
        &lock_error.legacy_outward,
        super::super::SettingsSaveLegacyOutward::SettingsLockUnavailable
    ));
    let (lock_records, _lock_capture_guard) = capture_settings_save_diagnostics();
    let lock_outward = super::super::report_settings_save_error(
        lock_error,
        super::super::SettingsSaveReportSurface::GeneralSettings,
    );
    assert_eq!(lock_outward, "settings_lock_unavailable");
    let lock_records = lock_records.borrow();
    assert_eq!(lock_records.len(), 1);
    let lock_fields = parse_settings_save_diagnostic(&lock_records[0]);
    assert_eq!(lock_fields["code"], "settings_lock_unavailable");
    assert_eq!(lock_fields["stage"], "lock_acquire");
    assert_eq!(lock_fields["reason"], "lock_timed_out");
    drop(first_lock);

    let writer_temp = tempfile::TempDir::new().unwrap();
    let writer_path = writer_temp.path().join("settings.json");
    std::fs::create_dir(&writer_path).unwrap();
    let writer_error =
        super::super::save_settings_to_path(&super::super::AppSettings::default(), &writer_path)
            .unwrap_err();
    assert_eq!(
        writer_error.stage,
        super::super::SettingsSaveStage::PrepareTarget
    );
    assert!(matches!(
        &writer_error.cause,
        super::super::SettingsSaveCause::Semantic(
            super::super::SettingsSaveReason::TargetNotRegularFile
        )
    ));
    assert!(matches!(
        &writer_error.legacy_outward,
        super::super::SettingsSaveLegacyOutward::SettingsSaveFailed
    ));
    let (writer_records, _writer_capture_guard) = capture_settings_save_diagnostics();
    let writer_outward = super::super::report_settings_save_error(
        writer_error,
        super::super::SettingsSaveReportSurface::GeneralSettings,
    );
    assert_eq!(writer_outward, "settings_save_failed");
    let writer_records = writer_records.borrow();
    assert_eq!(writer_records.len(), 1);
    let writer_fields = parse_settings_save_diagnostic(&writer_records[0]);
    assert_eq!(writer_fields["code"], "settings_save_failed");
    assert_eq!(writer_fields["stage"], "prepare_target");
    assert_eq!(writer_fields["reason"], "target_not_regular_file");
    assert_no_issue_1330_temp_files(writer_temp.path());
}

#[cfg(any(unix, windows))]
#[test]
fn terminal_compare_and_set_reports_typed_reader_failure_after_dropping_lock() {
    let temp = tempfile::TempDir::new().unwrap();
    let path = temp.path().join("settings.json");
    std::fs::write(&path, "{not json").unwrap();
    let original = std::fs::read(&path).unwrap();
    let records = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let reacquired = std::rc::Rc::new(std::cell::RefCell::new(None));
    let captured_records = std::rc::Rc::clone(&records);
    let captured_reacquired = std::rc::Rc::clone(&reacquired);
    let probe_path = path.clone();
    let _capture_guard = SettingsSaveDiagnosticCaptureGuard::install(move |payload| {
        captured_records.borrow_mut().push(payload.to_string());
        let probe = super::super::SettingsFileLock::acquire(&probe_path, std::time::Duration::ZERO);
        let succeeded = probe.is_ok();
        if let Ok(probe_guard) = probe {
            drop(probe_guard);
        }
        *captured_reacquired.borrow_mut() = Some(succeeded);
    });

    let outward = super::super::compare_and_set_terminal_snapshots_enabled_at_path(
        &super::super::AppSettings::default(),
        &path,
        false,
        true,
    )
    .unwrap_err();

    assert_eq!(outward, "terminal_snapshot_setting_save_failed");
    assert_eq!(*reacquired.borrow(), Some(true));
    let records = CapturedRecordsSnapshot::snapshot(&records);
    assert_eq!(records.len(), 1);
    let fields = parse_settings_save_diagnostic(&records[0]);
    assert_eq!(fields["code"], "terminal_snapshot_setting_save_failed");
    assert_eq!(fields["stage"], "preserve_disk_gate");
    assert_eq!(fields["source_kind"], "json");
    assert_eq!(std::fs::read(&path).unwrap(), original);
}

#[cfg(any(unix, windows))]
#[test]
fn string_facades_and_compatibility_adapters_map_typed_failures_once() {
    fn assert_held_lock_mapping(
        call: impl FnOnce(&super::super::AppSettings, &std::path::Path) -> Result<(), String>,
        expected_code: &str,
    ) {
        let temp = tempfile::TempDir::new().unwrap();
        let path = temp.path().join("settings.json");
        let settings = super::super::AppSettings::default();
        super::super::save_settings_to_path(&settings, &path).unwrap();
        let original = std::fs::read(&path).unwrap();
        let held_lock =
            super::super::SettingsFileLock::acquire(&path, std::time::Duration::from_secs(1))
                .unwrap();
        let (records, _capture_guard) = capture_settings_save_diagnostics();

        let outward = call(&settings, &path).unwrap_err();

        assert_eq!(outward, expected_code);
        assert_eq!(std::fs::read(&path).unwrap(), original);
        let records = records.borrow();
        assert_eq!(records.len(), 1);
        let fields = parse_settings_save_diagnostic(&records[0]);
        assert_eq!(fields["code"], expected_code);
        assert_eq!(fields["stage"], "lock_acquire");
        assert_eq!(fields["reason"], "lock_timed_out");
        drop(held_lock);
    }

    assert_held_lock_mapping(
        |settings, path| {
            super::super::reconcile_project_state_to_path(settings, path, false, false).map(|_| ())
        },
        "settings_lock_unavailable",
    );
    assert_held_lock_mapping(
        super::super::save_settings_with_project_paths_to_path,
        "settings_lock_unavailable",
    );
    assert_held_lock_mapping(
        |settings, path| {
            super::super::save_settings_to_path_preserving_project_paths(settings, path).map(|_| ())
        },
        "settings_lock_unavailable",
    );
    assert_held_lock_mapping(
        |settings, path| {
            super::super::compare_and_set_terminal_snapshots_enabled_at_path(
                settings, path, false, true,
            )
            .map(|_| ())
        },
        "terminal_snapshot_setting_save_failed",
    );
}

#[test]
fn terminal_snapshot_gate_defaults_true_and_strict_reader_fails_closed() {
    assert!(super::super::AppSettings::default().terminal_snapshots_enabled);
    let mut legacy = serde_json::to_value(super::super::AppSettings::default()).unwrap();
    legacy
        .as_object_mut()
        .unwrap()
        .remove("terminalSnapshotsEnabled");
    let decoded: super::super::AppSettings = serde_json::from_value(legacy).unwrap();
    assert!(decoded.terminal_snapshots_enabled);

    let temp = tempfile::TempDir::new().unwrap();
    let path = temp.path().join("settings.json");
    std::fs::write(&path, r#"{"projectPaths":[]}"#).unwrap();
    let absent = super::super::read_terminal_snapshot_security_settings_strict_from_path(&path)
        .expect("an absent legacy gate key is a valid default-on object");
    assert!(absent.terminal_snapshots_enabled);
    for malformed in [
        r#"{"terminalSnapshotsEnabled":null,"projectPaths":[]}"#,
        r#"{"terminalSnapshotsEnabled":"invalid","projectPaths":[]}"#,
    ] {
        std::fs::write(&path, malformed).unwrap();
        assert!(
            super::super::read_terminal_snapshot_security_settings_strict_from_path(&path).is_err(),
            "{malformed} must fail closed"
        );
    }
    std::fs::write(
        &path,
        r#"{"terminalSnapshotsEnabled":true,"terminalSnapshotsEnabled":false,"projectPaths":[]}"#,
    )
    .unwrap();
    assert!(
        super::super::read_terminal_snapshot_security_settings_strict_from_path(&path).is_err()
    );
    std::fs::write(
        &path,
        r#"{"terminalSnapshotsEnabled":true,"projectPaths":["one"]}"#,
    )
    .unwrap();
    let strict =
        super::super::read_terminal_snapshot_security_settings_strict_from_path(&path).unwrap();
    assert!(strict.terminal_snapshots_enabled);
    assert_eq!(strict.project_paths, vec!["one"]);

    // Empty active authority is valid only with all four active keys absent.
    std::fs::write(&path, r#"{"terminalSnapshotsEnabled":true}"#).unwrap();
    let destination = super::super::project_paths_path(&path);
    for projection in [
        r#"{}"#,
        r#"{"archivedProjectPaths":["archived"]}"#,
        r#"{"projectPaths":[]}"#,
    ] {
        std::fs::write(&destination, projection).unwrap();
        let strict =
            super::super::read_terminal_snapshot_security_settings_strict_from_path(&path).unwrap();
        assert!(strict.terminal_snapshots_enabled);
        assert!(strict.project_paths.is_empty());
    }
    for projection in [
        r#"{"projectPath":"singular"}"#,
        r#"{"projectPath":null}"#,
        r#"{"projectPathRelativeToInstance":null}"#,
        r#"{"projectPathsRelativeToInstance":[]}"#,
        r#"{"projectPaths":null}"#,
        r#"{"projectPaths":"scalar"}"#,
        r#"{"projectPaths":[null]}"#,
    ] {
        std::fs::write(&destination, projection).unwrap();
        assert!(
            super::super::read_terminal_snapshot_security_settings_strict_from_path(&path).is_err(),
            "{projection}"
        );
    }
    std::fs::write(&destination, b"{}").unwrap();
    std::fs::remove_file(&path).unwrap();
    assert!(
        super::super::read_terminal_snapshot_security_settings_strict_from_path(&path).is_err()
    );

    const PATH_CANARY: &str = r"C:\PATH_1173_SETTINGS_N3X6\project";
    let structural = super::super::TerminalSnapshotSecuritySettings {
        terminal_snapshots_enabled: true,
        project_paths: vec![PATH_CANARY.to_string()],
    };
    let diagnostic = format!("{structural:?}");
    assert!(!diagnostic.contains(PATH_CANARY));
    assert!(diagnostic.contains("terminal_snapshots_enabled: true"));
    assert!(diagnostic.contains("project_paths: 1"));
    assert!(diagnostic.contains(&format!("project_path_bytes: {}", PATH_CANARY.len())));
}

#[test]
fn terminal_snapshot_gate_cas_initializes_an_absent_file_before_success() {
    let temp = tempfile::TempDir::new().unwrap();
    let path = temp.path().join("settings.json");
    assert!(!path.exists());
    let current = super::super::AppSettings {
        project_paths: vec![temp.path().to_string_lossy().to_string()],
        ..super::super::AppSettings::default()
    };

    // The absent file compares as the default-on value: an explicit false
    // request is stale against a missing file and must not write anything.
    assert_eq!(
        super::super::compare_and_set_terminal_snapshots_enabled_at_path(
            &current, &path, false, false,
        )
        .unwrap_err(),
        "terminal_snapshot_setting_conflict"
    );
    assert!(!path.exists());

    // A matching default-on request may not report idempotent success while
    // no valid settings file exists: it must materialize the file first.
    let written = super::super::compare_and_set_terminal_snapshots_enabled_at_path(
        &current, &path, true, true,
    )
    .unwrap();
    assert!(written.terminal_snapshots_enabled);
    assert!(
        path.exists(),
        "an absent file must be written before the strict reader can authorize"
    );
    let strict =
        super::super::read_terminal_snapshot_security_settings_strict_from_path(&path).unwrap();
    assert!(strict.terminal_snapshots_enabled);
}

#[test]
fn terminal_snapshot_gate_cas_is_idempotent_and_rejects_stale_expected_value() {
    let temp = tempfile::TempDir::new().unwrap();
    let path = temp.path().join("settings.json");
    let current = super::super::AppSettings::default();
    super::super::save_settings_to_path(&current, &path).unwrap();

    let enabled = super::super::compare_and_set_terminal_snapshots_enabled_at_path(
        &current, &path, false, true,
    )
    .unwrap();
    assert!(enabled.terminal_snapshots_enabled);
    let idempotent = super::super::compare_and_set_terminal_snapshots_enabled_at_path(
        &enabled, &path, false, true,
    )
    .unwrap();
    assert!(idempotent.terminal_snapshots_enabled);
    assert_eq!(
        super::super::compare_and_set_terminal_snapshots_enabled_at_path(
            &enabled, &path, false, false,
        )
        .unwrap_err(),
        "terminal_snapshot_setting_conflict"
    );
    let disabled = super::super::compare_and_set_terminal_snapshots_enabled_at_path(
        &enabled, &path, true, false,
    )
    .unwrap();
    assert!(!disabled.terminal_snapshots_enabled);
}

#[test]
fn whole_settings_writer_preserves_disk_terminal_snapshot_gate() {
    let temp = tempfile::TempDir::new().unwrap();
    let path = temp.path().join("settings.json");
    let enabled = super::super::AppSettings {
        terminal_snapshots_enabled: true,
        ..super::super::AppSettings::default()
    };
    super::super::save_settings_to_path(&enabled, &path).unwrap();

    let stale = super::super::AppSettings::default();
    let written =
        super::super::save_settings_to_path_preserving_project_paths(&stale, &path).unwrap();
    assert!(written.terminal_snapshots_enabled);

    let false_path = temp.path().join("settings-false.json");
    let mut false_object = serde_json::to_value(super::super::AppSettings::default()).unwrap();
    false_object.as_object_mut().unwrap().insert(
        "terminalSnapshotsEnabled".to_string(),
        serde_json::json!(false),
    );
    std::fs::write(&false_path, serde_json::to_vec(&false_object).unwrap()).unwrap();
    let written = super::super::save_settings_to_path_preserving_project_paths(
        &super::super::AppSettings::default(),
        &false_path,
    )
    .unwrap();
    assert!(!written.terminal_snapshots_enabled);
    let object: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&false_path).unwrap()).unwrap();
    assert_eq!(object["terminalSnapshotsEnabled"], serde_json::json!(false));
    let strict =
        super::super::read_terminal_snapshot_security_settings_strict_from_path(&false_path)
            .unwrap();
    assert!(!strict.terminal_snapshots_enabled);
}

#[test]
fn whole_settings_writer_materializes_true_for_an_absent_legacy_gate_key() {
    let temp = tempfile::TempDir::new().unwrap();
    let path = temp.path().join("settings.json");
    let mut legacy_object = serde_json::to_value(super::super::AppSettings::default()).unwrap();
    legacy_object
        .as_object_mut()
        .unwrap()
        .remove("terminalSnapshotsEnabled");
    std::fs::write(&path, serde_json::to_vec(&legacy_object).unwrap()).unwrap();

    let written = super::super::save_settings_to_path_preserving_project_paths(
        &super::super::AppSettings::default(),
        &path,
    )
    .unwrap();
    assert!(written.terminal_snapshots_enabled);
    let object: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(object["terminalSnapshotsEnabled"], serde_json::json!(true));
}

#[test]
fn privileged_project_paths_reader_rejects_duplicates_and_non_strings() {
    let temp = tempfile::TempDir::new().unwrap();
    let path = temp.path().join("settings.json");
    std::fs::write(&path, r#"{"projectPaths":["one"],"projectPaths":["two"]}"#).unwrap();
    assert!(super::super::read_pty_input_project_paths_strict_from_path(&path).is_err());

    std::fs::write(&path, r#"{"projectPaths":["one",7]}"#).unwrap();
    assert!(super::super::read_pty_input_project_paths_strict_from_path(&path).is_err());
}

#[test]
fn privileged_project_paths_reader_rejects_a_dangling_link_when_supported() {
    let temp = tempfile::TempDir::new().unwrap();
    let path = temp.path().join("settings.json");
    let missing = temp.path().join("missing.json");
    #[cfg(unix)]
    let linked = std::os::unix::fs::symlink(&missing, &path).is_ok();
    #[cfg(windows)]
    let linked = std::os::windows::fs::symlink_file(&missing, &path).is_ok();
    #[cfg(not(any(unix, windows)))]
    let linked = false;
    if linked {
        assert!(super::super::read_pty_input_project_paths_strict_from_path(&path).is_err());
    }
}

#[test]
fn privileged_project_paths_reader_is_bounded_and_exact() {
    let temp = tempfile::TempDir::new().unwrap();
    let path = temp.path().join("settings.json");
    std::fs::write(&path, r#"{"projectPaths":["one","two"]}"#).unwrap();
    assert_eq!(
        super::super::read_pty_input_project_paths_strict_from_path(&path).unwrap(),
        Some(vec!["one".to_string(), "two".to_string()])
    );

    std::fs::write(&path, vec![b' '; 1024 * 1024 + 1]).unwrap();
    assert!(super::super::read_pty_input_project_paths_strict_from_path(&path).is_err());
}

#[test]
fn validate_agent_commands_allows_plain_antigravity() {
    let settings = settings_with_agents(&[("Antigravity", "agy")]);
    assert!(super::super::validate_agent_commands(&settings).is_ok());
}

#[test]
fn validate_agent_commands_rejects_antigravity_continue() {
    for command in ["agy --continue", "agy -c"] {
        let settings = settings_with_agents(&[("Antigravity", command)]);
        let err = super::super::validate_agent_commands(&settings).unwrap_err();
        assert!(
            err.contains("must not include --continue or -c"),
            "command={command:?} err={err}"
        );
    }
}

#[test]
fn validate_agent_commands_allows_antigravity_conversation() {
    // `--conversation <ID>` is a user-authored resume-by-ID form and stays
    // allowed (analog of Claude `--resume <id>`; the injector skip honors it).
    let settings = settings_with_agents(&[("Antigravity", "agy --conversation abc123")]);
    assert!(super::super::validate_agent_commands(&settings).is_ok());
}

#[test]
fn codex_home_template_accepts_each_token_alone_and_as_leading_segment() {
    for token in [
        "%AC_REPLICA_ROOT%",
        "%AC_WORKSPACE_ROOT%",
        "%AC_MATRIX_ROOT%",
    ] {
        super::super::validate_codex_home_template_value(token, "ctx")
            .unwrap_or_else(|e| panic!("{token} alone should be accepted: {e}"));
        super::super::validate_codex_home_template_value(&format!("{token}\\.codex"), "ctx")
            .unwrap_or_else(|e| panic!("{token}\\.codex should be accepted: {e}"));
        super::super::validate_codex_home_template_value(&format!("{token}/.codex"), "ctx")
            .unwrap_or_else(|e| panic!("{token}/.codex should be accepted: {e}"));
    }
}

#[test]
fn codex_home_template_rejects_token_not_at_leading_segment() {
    let err = super::super::validate_codex_home_template_value(r"prefix%AC_MATRIX_ROOT%\x", "ctx")
        .unwrap_err();
    assert!(
        err.contains("complete path segment") && err.contains("%AC_MATRIX_ROOT%"),
        "{err}"
    );
}

#[test]
fn codex_home_template_rejects_legacy_ac_root_as_unknown_marker() {
    // Save-time break: the old token is now an unknown placeholder, named in the error.
    let err = super::super::validate_codex_home_template_value(r"%AC_ROOT%\x", "ctx").unwrap_err();
    assert!(err.contains("%AC_ROOT%"), "{err}");
}

#[test]
fn codex_home_template_keys_on_leading_token_not_list_order() {
    // F3: the leading segment is a valid token; another token appearing later must
    // NOT trigger a rejection that names the wrong token.
    super::super::validate_codex_home_template_value(r"%AC_MATRIX_ROOT%\sub", "ctx")
        .expect("matrix leading segment is valid");
    super::super::validate_codex_home_template_value(
        r"%AC_MATRIX_ROOT%\%AC_REPLICA_ROOT%\x",
        "ctx",
    )
    .expect("leading token valid; remainder tokens are known at shape level");
}
