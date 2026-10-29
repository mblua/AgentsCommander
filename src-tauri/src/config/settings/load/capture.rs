#[cfg(test)]
use super::PROJECT_SOURCE_CAPTURE_HOOK;
use super::{
    agents_layer_state_at_labeled, apply_project_decode_to_value, collect_registered_profile_gate,
    default_settings_with_overlay_captured, empty_project_transaction,
    ensure_project_paths_migrated_at_labeled_path, parse_settings_json_with_projection_labeled,
    parse_settings_with_startup, production_instance_base, project_migration_error_labeled,
    projects, read_raw_project_source_labeled_with_limit, recheck_registered_profile_gate,
    report_settings_save_error, select_project_source_at_labeled_path_with_limit,
    set_agents_layer_state, unavailable_project_settings, AppSettings, Map, Path,
    RegisteredProfileProblemKind, SettingsSaveError, SettingsSaveLegacyOutward, SettingsSaveReason,
    SettingsSaveReportSurface, SettingsSaveStage, StartupAgentRows, Value, FIELD_ARCHIVED,
    FIELD_ARCHIVED_REL, FIELD_PROJECT_PATH, FIELD_PROJECT_PATHS, FIELD_PROJECT_PATHS_REL,
    FIELD_PROJECT_PATH_REL,
};
use crate::filesystem::settings_file;

pub(crate) fn capture_project_settings(
    path: &Path,
    migrate: bool,
) -> Result<(AppSettings, bool, Option<String>), String> {
    capture_project_settings_with_startup(path, migrate, None)
}

pub(crate) fn capture_project_settings_with_startup(
    path: &Path,
    migrate: bool,
    mut startup: Option<&mut Option<StartupAgentRows>>,
) -> Result<(AppSettings, bool, Option<String>), String> {
    let lock_error = |reason| {
        SettingsSaveError::semantic(
            SettingsSaveStage::LockAcquire,
            path,
            None,
            reason,
            SettingsSaveLegacyOutward::SettingsLockUnavailable,
        )
    };
    let report = |error: SettingsSaveError| {
        report_settings_save_error(
            error.with_capture_labels(path),
            SettingsSaveReportSurface::GeneralSettings,
        )
    };
    let parent = path
        .parent()
        .ok_or_else(|| report(lock_error(SettingsSaveReason::MissingParentDirectory)))?;
    settings_file::ensure_settings_directory(parent).map_err(|source| {
        report(SettingsSaveError::io(
            SettingsSaveStage::LockAcquire,
            path,
            None,
            source,
            SettingsSaveLegacyOutward::SettingsLockUnavailable,
        ))
    })?;
    match crate::path_identity::verify_component_chain_reason(parent) {
        Ok(()) | Err(crate::path_identity::PathRule::ComponentIsLink) => {}
        Err(_) => return Err(report(lock_error(SettingsSaveReason::LockIdentityRejected))),
    }
    let physical_parent = settings_file::canonical_settings_parent(parent)
        .map_err(|_| report(lock_error(SettingsSaveReason::LockIdentityRejected)))?;
    // Windows retains a no-share-delete handle. Unix detects persistent
    // replacement, not transient A -> B -> A absolute-path substitution.
    let retained = crate::path_identity::retain_directory(&physical_parent)
        .map_err(|_| report(lock_error(SettingsSaveReason::LockIdentityRejected)))?;
    let leaf = path
        .file_name()
        .ok_or_else(|| report(lock_error(SettingsSaveReason::LockIdentityRejected)))?;
    let physical = retained.identity().canonical_path.join(leaf);
    let transaction = empty_project_transaction(&physical, "capture_project_settings")
        .map_err(|error| report(error.with_capture_labels(path)))?;
    retained
        .verify_current()
        .map_err(|_| report(lock_error(SettingsSaveReason::LockIdentityRejected)))?;
    #[cfg(test)]
    PROJECT_SOURCE_CAPTURE_HOOK.with(|hook| {
        if let Some(callback) = hook.borrow().as_ref() {
            callback(path);
        }
    });
    let raw_profile_capture = collect_registered_profile_gate(&transaction.path);
    if let Some(reason) = raw_profile_capture.ordinary_error() {
        log::warn!("[profiles] {reason}");
    }
    let migration_error = if migrate
        && raw_profile_capture.admits_project_migration(&transaction.path)
    {
        let mut checked = false;
        ensure_project_paths_migrated_at_labeled_path(&transaction.path, path, &mut |stage, _| {
            if !checked
                && matches!(
                    stage,
                    "backup_create" | "destination_before_write" | "source_cleanup"
                )
            {
                recheck_registered_profile_gate(&raw_profile_capture.participant_documents())
                    .map_err(|reason| {
                        project_migration_error_labeled(
                            &transaction.path,
                            path,
                            SettingsSaveStage::ProjectPathsRefreshDiskGate,
                            &reason,
                        )
                    })?;
                checked = true;
            }
            Ok(())
        })
        .err()
    } else {
        None
    };
    let selected = match migration_error {
        Some(error) => Err(error),
        None => select_project_source_at_labeled_path_with_limit(
            &transaction.path,
            path,
            16 * 1024 * 1024,
        ),
    };
    let captured = if raw_profile_capture
        .problems
        .iter()
        .any(|problem| problem.kind == RegisteredProfileProblemKind::DuplicateMember)
        && selected
            .as_ref()
            .err()
            .is_none_or(|error| error.target_path.as_path() == transaction.path.as_path())
    {
        // The ordinary reader remains lenient. The strict project selector must
        // not turn an unrelated duplicate into project persistence failure.
        let contents = settings_file::read_settings_text(&transaction.path).unwrap_or_else(|_| {
            serde_json::to_string(&AppSettings::default()).expect("default settings")
        });
        let legacy: Value =
            serde_json::from_str(&contents).unwrap_or_else(|_| serde_json::json!({}));
        let mut probe = legacy.clone();
        apply_project_decode_to_value(
            &mut probe,
            production_instance_base().as_deref(),
            &projects::FsCandidateResolver,
        );
        let projection = selected
            .as_ref()
            .ok()
            .map(|selected| selected.projection.clone())
            .unwrap_or_else(|| {
                probe
                    .as_object()
                    .map(|map| {
                        let mut projection = Map::new();
                        for key in [
                            FIELD_PROJECT_PATH,
                            FIELD_PROJECT_PATHS,
                            FIELD_ARCHIVED,
                            FIELD_PROJECT_PATH_REL,
                            FIELD_PROJECT_PATHS_REL,
                            FIELD_ARCHIVED_REL,
                        ] {
                            if let Some(value) = map.get(key) {
                                projection.insert(key.to_owned(), value.clone());
                            }
                        }
                        projection
                    })
                    .unwrap_or_default()
            });
        let parsed = parse_settings_with_startup(
            &contents,
            &path.to_string_lossy(),
            Some(&transaction.path),
            Some(path),
            Some(&projection),
            startup.as_deref_mut(),
        );
        parsed.map(|(settings, legacy)| (settings, legacy, Some(contents)))
    } else {
        (|| {
            match selected {
                Ok(selected) => match selected.legacy {
                    Some(legacy) => {
                        let contents =
                            String::from_utf8(legacy.bytes).map_err(|error| error.to_string())?;
                        match parse_settings_with_startup(
                            &contents,
                            &path.to_string_lossy(),
                            Some(&transaction.path),
                            Some(path),
                            Some(&selected.projection),
                            startup.as_deref_mut(),
                        ) {
                            Ok((settings, legacy_profiles)) => {
                                Ok((settings, legacy_profiles, Some(contents)))
                            }
                            Err(error) => {
                                if let Some(startup) = startup {
                                    *startup = None;
                                }
                                log::error!("{error}");
                                // Selection succeeded; unrelated decode failure must not
                                // mark the authoritative project source unavailable.
                                let mut settings = default_settings_with_overlay_captured(
                                    &transaction.path,
                                    &path.to_string_lossy(),
                                    &selected.projection,
                                );
                                set_agents_layer_state(
                                    &mut settings,
                                    path,
                                    agents_layer_state_at_labeled(&transaction.path, path),
                                );
                                Ok((settings, false, None))
                            }
                        }
                    }
                    None => Ok((
                        default_settings_with_overlay_captured(
                            &transaction.path,
                            &path.to_string_lossy(),
                            &selected.projection,
                        ),
                        false,
                        None,
                    )),
                },
                Err(error) => {
                    let message = report_settings_save_error(
                        error.with_capture_labels(path),
                        SettingsSaveReportSurface::GeneralSettings,
                    );
                    // Preserve readable unrelated values while exposing no project authorization.
                    let mut settings = read_raw_project_source_labeled_with_limit(
                        &transaction.path,
                        path,
                        16 * 1024 * 1024,
                        false,
                    )
                    .ok()
                    .flatten()
                    .and_then(|legacy| {
                        let contents = String::from_utf8(legacy.bytes).ok()?;
                        parse_settings_json_with_projection_labeled(
                            &contents,
                            &path.to_string_lossy(),
                            Some(&transaction.path),
                            Some(path),
                            Some(&Map::new()),
                        )
                        .ok()
                        .map(|pair| pair.0)
                    })
                    .unwrap_or_else(|| {
                        default_settings_with_overlay_captured(
                            &transaction.path,
                            &path.to_string_lossy(),
                            &Map::new(),
                        )
                    });
                    set_agents_layer_state(
                        &mut settings,
                        path,
                        agents_layer_state_at_labeled(&transaction.path, path),
                    );
                    Ok((unavailable_project_settings(settings, message), false, None))
                }
            }
        })()
    };
    // Every fallback passes this check while the same lock is still held.
    if retained.verify_current().is_err() {
        let message = report(lock_error(SettingsSaveReason::LockIdentityRejected));
        return match captured {
            Ok((settings, _, _)) => {
                Ok((unavailable_project_settings(settings, message), false, None))
            }
            Err(_) => Err(message),
        };
    }
    captured
}
