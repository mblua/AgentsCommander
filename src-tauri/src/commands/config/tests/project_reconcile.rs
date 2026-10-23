use super::*;

/// Build a pending (companion-population-eligible) SettingsState by decoding a
/// legacy `[project]` file (no companion) from `settings_path`.
fn pending_state_from_legacy(settings_path: &Path, project: &str) -> SettingsState {
    write_settings_file(
        settings_path.parent().unwrap(),
        &AppSettings {
            project_paths: vec![project.to_string()],
            project_path: Some(project.to_string()),
            ..AppSettings::default()
        },
    );
    let value: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(settings_path).unwrap()).unwrap();
    let decoded = crate::config::settings::decode_project_state(
        value.as_object().unwrap(),
        None,
        &crate::config::projects::FsCandidateResolver,
    );
    let startup = AppSettings {
        project_paths: decoded.active_selected(),
        project_path: decoded.selected_head.clone(),
        project_path_state: std::sync::Arc::new(decoded),
        ..AppSettings::default()
    };
    assert!(
        startup.project_path_state.active_reconcile_eligible,
        "legacy no-companion state should be pending"
    );
    state_for(startup)
}

#[tokio::test]
async fn auto_reconcile_redecodes_and_preserves_intervening_cli_write() {
    // Grinch Defect 2: the get_settings boundary must re-decode disk before
    // reconciling, so a CLI registration that landed after startup survives.
    let temp = tempfile::tempdir().unwrap();
    let settings_path = temp.path().join("settings.json");
    let a = real_ac_project(temp.path(), "A");
    let b = real_ac_project(temp.path(), "B");
    let state = pending_state_from_legacy(&settings_path, &a);

    // Intervening CLI registration: disk now carries [A, B].
    write_settings_file(
        temp.path(),
        &AppSettings {
            project_paths: vec![a.clone(), b.clone()],
            project_path: Some(a.clone()),
            ..AppSettings::default()
        },
    );

    let _snap = super::settings_snapshot_helper(&state, Some(settings_path.clone())).await;

    let disk: AppSettings =
        crate::config::settings::load_settings_from_path(std::path::Path::new(&settings_path));
    assert_eq!(
        disk.project_paths,
        vec![a.clone(), b.clone()],
        "CLI-registered B must not be clobbered by the auto-reconcile"
    );
    let live = state.read().await;
    assert_eq!(live.project_paths, vec![a, b]);
}

#[tokio::test]
async fn auto_reconcile_reports_stage_read_on_corrupt_disk() {
    // Grinch Defect 2: a disk re-decode failure at the boundary reports
    // stage `read`, performs no write, and retains the prior state.
    let temp = tempfile::tempdir().unwrap();
    let settings_path = temp.path().join("settings.json");
    let a = real_ac_project(temp.path(), "A");
    let state = pending_state_from_legacy(&settings_path, &a);

    // Corrupt the settings file after startup.
    std::fs::write(&settings_path, b"{ not valid json").unwrap();
    let snap = super::settings_snapshot_helper(&state, Some(settings_path.clone())).await;

    let err = snap
        .project_path_resolution
        .reconciliation_error
        .expect("a corrupt disk re-decode must surface a reconciliation error");
    assert!(matches!(err.stage, super::ReconciliationStage::Read));
    assert!(err.retryable);
    // No write: the corrupt bytes are untouched.
    assert_eq!(
        std::fs::read_to_string(&settings_path).unwrap(),
        "{ not valid json"
    );
}

#[tokio::test]
async fn unavailable_snapshot_blocks_purge_even_without_cached_repair() {
    let temp = tempfile::tempdir().unwrap();
    let settings_path = temp.path().join("settings.json");
    let state = state_for(AppSettings::default());
    std::fs::write(&settings_path, b"invalid").unwrap();
    write_sessions_file(
        temp.path(),
        &[PersistedSession {
            name: "retained".into(),
            shell: "codex".into(),
            working_directory: temp.path().to_string_lossy().into_owned(),
            ..Default::default()
        }],
    );
    let session_path = temp.path().join("sessions.json");
    let original = std::fs::read(&session_path).unwrap();
    let snapshot = super::settings_snapshot_helper(&state, Some(settings_path.clone())).await;
    let error = snapshot
        .project_path_resolution
        .reconciliation_error
        .unwrap();
    assert!(matches!(error.stage, super::ReconciliationStage::Read));
    let saved = state.read().await.clone();
    assert!(saved.project_paths_persistence_error.is_some());
    assert!(
        purge_sessions_after_settings_update_in_dir(&saved, temp.path())
            .await
            .is_err()
    );
    assert_eq!(std::fs::read(&session_path).unwrap(), original);
    assert_eq!(std::fs::read(&settings_path).unwrap(), b"invalid");
}

#[tokio::test]
async fn settings_update_returns_written_project_lists_for_session_purge() {
    let temp = tempfile::tempdir().unwrap();
    let settings_path = temp.path().join("settings.json");
    let project = temp.path().join("project-a");
    let kept_workdir = project.join(".ac").join("wg-1").join("__agent_dev");
    let outside = temp.path().join("project-b");
    let outside_workdir = outside.join(".ac").join("wg-1").join("__agent_dev");
    std::fs::create_dir_all(&kept_workdir).expect("create kept workdir");
    std::fs::create_dir_all(&outside_workdir).expect("create outside workdir");

    let project_path = canonical_display(&project);
    write_settings_file(
        temp.path(),
        &AppSettings {
            project_paths: vec![project_path.clone()],
            project_path: Some(project_path.clone()),
            ..AppSettings::default()
        },
    );
    write_sessions_file(
        temp.path(),
        &[
            PersistedSession {
                name: "kept".to_string(),
                shell: "codex".to_string(),
                working_directory: kept_workdir.to_string_lossy().to_string(),
                ..Default::default()
            },
            PersistedSession {
                name: "outside".to_string(),
                shell: "codex".to_string(),
                working_directory: outside_workdir.to_string_lossy().to_string(),
                ..Default::default()
            },
        ],
    );

    let mut current = settings_with_single_agent();
    current.project_paths.clear();
    current.project_path = None;
    let state = state_for(current.clone());
    let stale = current;

    let (saved, _) = persist_protected_settings_update_with_saver(&state, stale, |candidate| {
        crate::config::settings::save_settings_to_path_preserving_project_paths(
            candidate,
            &settings_path,
        )
    })
    .await
    .unwrap();

    purge_sessions_after_settings_update_in_dir(&saved, temp.path())
        .await
        .unwrap();

    let remaining = read_sessions_file(temp.path());
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].name, "kept");
    assert_eq!(
        remaining[0].working_directory,
        kept_workdir.to_string_lossy()
    );
    assert_single_project(&saved, &project_path);
}

#[tokio::test]
async fn settings_update_missing_settings_file_preserves_project_sessions() {
    let temp = tempfile::tempdir().unwrap();
    let settings_path = temp.path().join("settings.json");
    assert!(!settings_path.exists());

    let project = temp.path().join("project-a");
    let kept_workdir = project.join(".ac").join("wg-1").join("__agent_dev");
    std::fs::create_dir_all(&kept_workdir).expect("create kept workdir");
    let project_path = canonical_display(&project);

    write_sessions_file(
        temp.path(),
        &[PersistedSession {
            name: "kept".to_string(),
            shell: "codex".to_string(),
            working_directory: kept_workdir.to_string_lossy().to_string(),
            ..Default::default()
        }],
    );

    let mut current = settings_with_single_agent();
    current.project_paths = vec![project_path.clone()];
    current.project_path = Some(project_path.clone());
    let state = state_for(current.clone());

    // Seed legacy persisted authority before Preserve, independent of renderer fields.
    std::fs::write(
        &settings_path,
        serde_json::json!({
            "projectPaths": [project_path.clone()],
            "projectPath": project_path.clone(),
            "defaultShell": "test-shell",
            "defaultShellArgs": [],
            "agents": [],
        })
        .to_string(),
    )
    .unwrap();
    let mut incoming = current.clone();
    incoming.sidebar_style = "deep-space".to_string();
    let (saved, _) = persist_protected_settings_update_with_saver(&state, incoming, |candidate| {
        crate::config::settings::save_settings_to_path_preserving_project_paths(
            candidate,
            &settings_path,
        )
    })
    .await
    .unwrap();

    assert_single_project(&saved, &project_path);
    {
        let live = state.read().await;
        assert_single_project(&live, &project_path);
    }
    assert!(settings_path.exists(), "writer must create settings.json");
    let sidecar = settings_path.with_file_name("project-paths.json");
    let sidecar_present = sidecar.try_exists().expect("inspect project authority");
    let authority = if sidecar_present {
        &sidecar
    } else {
        &settings_path
    };
    let disk: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(authority).unwrap()).unwrap();
    if sidecar_present {
        let legacy: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&settings_path).unwrap()).unwrap();
        for key in [
            "projectPath",
            "projectPaths",
            "archivedProjectPaths",
            "projectPathRelativeToInstance",
            "projectPathsRelativeToInstance",
            "archivedProjectPathsRelativeToInstance",
        ] {
            assert!(
                legacy.get(key).is_none(),
                "legacy project key remains: {key}"
            );
        }
    }
    assert_eq!(
        disk["projectPaths"],
        serde_json::json!([project_path.clone()])
    );
    assert_eq!(disk["projectPath"], serde_json::json!(project_path));

    purge_sessions_after_settings_update_in_dir(&saved, temp.path())
        .await
        .unwrap();

    let remaining = read_sessions_file(temp.path());
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].name, "kept");
    assert_eq!(
        remaining[0].working_directory,
        kept_workdir.to_string_lossy()
    );
}
