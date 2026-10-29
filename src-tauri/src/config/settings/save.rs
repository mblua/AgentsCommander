#[cfg(test)]
use super::{project_migration_error, reconcile_project_state_to_path};
use super::{AppSettings, ProjectWriteMode};
use crate::config::instance_artifacts::SETTINGS_FILE_NAME;
use crate::filesystem::settings_file;
use serde_json::Value;
use std::path::Path;
#[cfg(test)]
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

mod disk;
mod error;
mod lock;
mod locked;
mod value;

pub use disk::refresh_project_paths_from_disk;
pub(crate) use disk::{
    compare_and_set_terminal_snapshots_enabled, project_lists_from_projection,
    project_state_has_structural, read_pty_input_project_paths_strict,
    read_pty_input_project_paths_strict_offloaded, read_terminal_snapshot_project_paths_strict,
    read_terminal_snapshot_security_settings_strict,
    read_terminal_snapshot_security_settings_strict_offloaded, set_main_window_placement,
};
#[cfg(test)]
pub(crate) use disk::{
    compare_and_set_terminal_snapshots_enabled_at_path, read_project_paths_from_disk,
    read_pty_input_project_paths_strict_from_path,
    read_terminal_snapshot_security_settings_strict_from_path,
    refresh_and_decode_project_paths_from_path, refresh_project_paths_from_path,
    set_main_window_placement_at_path, TerminalSnapshotSecuritySettings,
};
pub(crate) use error::{
    io_error_kind_as_str, json_error_category_as_str,
    read_disk_object_and_contents_for_write_typed, read_disk_object_for_write_typed,
    report_settings_save_error, SettingsSaveCause, SettingsSaveError, SettingsSaveLegacyOutward,
    SettingsSaveReason, SettingsSaveReportSurface, SettingsSaveStage,
};
#[cfg(test)]
pub(crate) use error::{
    render_diagnostic_path, render_settings_save_diagnostic, SettingsSaveDiagnosticCapture,
    SETTINGS_LOCK_UNAVAILABLE_CODE, SETTINGS_SAVE_DIAGNOSTIC_CAPTURE,
};
pub(crate) use lock::SettingsFileLock;
#[cfg(test)]
pub(crate) use lock::{
    settings_lock_debug_lost_count, settings_lock_debug_path, settings_lock_debug_sender,
    settings_lock_debug_set_attempt, settings_lock_debug_transport,
    settings_lock_debug_transport_initialized, SettingsLockDebugAttempt, SettingsLockDebugContext,
    SettingsLockDebugTestHooks, SETTINGS_LOCK_DEBUG_CAPACITY, SETTINGS_LOCK_DEBUG_PATH_BYTES,
    SETTINGS_LOCK_DEBUG_TEST_HOOKS, SETTINGS_LOCK_DEBUG_TEST_UNLOCK_ERROR,
};
pub(crate) use locked::save_settings_value_locked_with_startup;
#[cfg(test)]
pub(crate) use value::preserve_main_window_placement_keys;
pub(crate) use value::{
    apply_registered_main_disk_fields, preflight_duplicate_agents,
    prepare_registered_main_agent_groups, save_settings_value, save_settings_value_locked,
    save_settings_value_with_startup, serialize_settings_component,
};

/// #774: counter feeding the per-call unique temp filename for settings saves.
/// Combined with the PID it makes `settings.json.<pid>.<op_id>.tmp` distinct from
/// any concurrent cross-process save (GUI startup, CLI verbs, closing flush) and
/// from any leftover temp written by a prior crashed run. Mirrors
/// `sessions_persistence::SAVE_OP_ID` (sessions_persistence.rs:47); a separate
/// counter so the two can be reasoned about independently in diagnostics.
static SAVE_OP_ID: AtomicU64 = AtomicU64::new(0);

pub(crate) fn next_settings_save_op_id() -> u64 {
    SAVE_OP_ID.fetch_add(1, Ordering::Relaxed)
}

/// Save unrelated settings under the shared settings lock. Migration completes
/// first; afterward the sidecar bytes are preserved exactly and all six project
/// keys are excluded from managed settings. The returned runtime view is decoded
/// from the authoritative sidecar, never from a renderer's project draft.
pub fn save_settings(settings: &AppSettings) -> Result<AppSettings, String> {
    let dir = super::super::config_dir().ok_or("Could not determine home directory")?;
    let path = dir.join(SETTINGS_FILE_NAME);
    save_settings_to_path_preserving_project_paths(settings, &path)
}

#[cfg(test)]
thread_local! {
    pub(crate) static PROJECT_MIGRATION_ATOMIC_FAULT: std::cell::RefCell<
        Option<(PathBuf, SettingsSaveStage)>,
    > = const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
pub(crate) fn project_atomic_checkpoint(
    path: &Path,
    stage: SettingsSaveStage,
) -> Result<(), SettingsSaveError> {
    PROJECT_MIGRATION_ATOMIC_FAULT.with(|slot| {
        if slot
            .borrow()
            .as_ref()
            .is_some_and(|(target, point)| target == path && *point == stage)
        {
            Err(project_migration_error(
                path,
                stage,
                "injected migration atomic failure",
            ))
        } else {
            Ok(())
        }
    })
}

/// #1077: atomic tmp+rename writer over an already-built JSON `Value`. Shared by
/// the raw and the project-aware writers. Preserves the #774 unique-temp +
/// `rename_with_retry` behavior. The temp IS fsynced
/// (`temporary.sync_all()`), and the parent directory is fsynced on unix after
/// the rename. The caller owns #2058 backup rotation of the returned bytes.
pub(crate) fn write_value_atomic(value: &Value, path: &Path) -> Result<Vec<u8>, SettingsSaveError> {
    write_value_atomic_labeled(value, path, path)
}

pub(crate) fn write_value_atomic_labeled(
    value: &Value,
    path: &Path,
    label: &Path,
) -> Result<Vec<u8>, SettingsSaveError> {
    let dir = path.parent().ok_or_else(|| {
        SettingsSaveError::semantic(
            SettingsSaveStage::PrepareTarget,
            path,
            None,
            SettingsSaveReason::MissingParentDirectory,
            SettingsSaveLegacyOutward::SettingsSaveFailed,
        )
    })?;
    settings_file::ensure_settings_directory(dir).map_err(|source| {
        SettingsSaveError::io(
            SettingsSaveStage::PrepareTarget,
            path,
            None,
            source,
            SettingsSaveLegacyOutward::SettingsSaveFailed,
        )
    })?;
    crate::path_identity::verify_component_chain(dir).map_err(|_| {
        SettingsSaveError::semantic(
            SettingsSaveStage::PrepareTarget,
            path,
            None,
            SettingsSaveReason::TargetIdentityRejected,
            SettingsSaveLegacyOutward::SettingsSaveFailed,
        )
    })?;
    match settings_file::settings_path_metadata(path) {
        Ok(metadata) => {
            if !metadata.is_file() {
                return Err(SettingsSaveError::semantic(
                    SettingsSaveStage::PrepareTarget,
                    path,
                    None,
                    SettingsSaveReason::TargetNotRegularFile,
                    SettingsSaveLegacyOutward::SettingsSaveFailed,
                ));
            }
            crate::path_identity::verify_regular_file(path).map_err(|_| {
                SettingsSaveError::semantic(
                    SettingsSaveStage::PrepareTarget,
                    path,
                    None,
                    SettingsSaveReason::TargetIdentityRejected,
                    SettingsSaveLegacyOutward::SettingsSaveFailed,
                )
            })?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(source) => {
            return Err(SettingsSaveError::io(
                SettingsSaveStage::PrepareTarget,
                path,
                None,
                source,
                SettingsSaveLegacyOutward::SettingsSaveFailed,
            ));
        }
    }

    let json = serde_json::to_vec_pretty(value).map_err(|source| {
        SettingsSaveError::json(
            SettingsSaveStage::Serialize,
            path,
            None,
            source,
            SettingsSaveLegacyOutward::SettingsSaveFailed,
        )
    })?;
    if json.len() > 16 * 1024 * 1024 {
        return Err(SettingsSaveError::semantic(
            SettingsSaveStage::Serialize,
            path,
            None,
            SettingsSaveReason::SerializedPayloadTooLarge,
            SettingsSaveLegacyOutward::SettingsSaveFailed,
        ));
    }

    let op_id = next_settings_save_op_id();
    let pid = std::process::id();
    // Settings keep their existing instance-qualified temp prefix, including
    // callers supplying a fixture path. The fixed sidecar uses its own name.
    let temp_base = match path.file_name() {
        Some(name)
            if name
                == std::ffi::OsStr::new(
                    super::super::instance_artifacts::PROJECT_PATHS_FILE_NAME,
                ) =>
        {
            name
        }
        _ => std::ffi::OsStr::new(SETTINGS_FILE_NAME),
    };
    let mut temp_name = temp_base.to_os_string();
    temp_name.push(format!(".{pid}.{op_id}.tmp"));
    let tmp_path = dir.join(temp_name);
    let result: Result<(), SettingsSaveError> = (|| {
        #[cfg(test)]
        project_atomic_checkpoint(path, SettingsSaveStage::TempCreate)?;
        let mut temporary =
            settings_file::open_settings_temporary(&tmp_path).map_err(|source| {
                SettingsSaveError::io(
                    SettingsSaveStage::TempCreate,
                    path,
                    Some(&tmp_path),
                    source,
                    SettingsSaveLegacyOutward::SettingsSaveFailed,
                )
            })?;
        #[cfg(test)]
        project_atomic_checkpoint(path, SettingsSaveStage::TempWrite)?;
        settings_file::write_settings_bytes(&mut temporary, &json).map_err(|source| {
            SettingsSaveError::io(
                SettingsSaveStage::TempWrite,
                path,
                Some(&tmp_path),
                source,
                SettingsSaveLegacyOutward::SettingsSaveFailed,
            )
        })?;
        settings_file::flush_settings_file(&mut temporary).map_err(|source| {
            SettingsSaveError::io(
                SettingsSaveStage::TempFlush,
                path,
                Some(&tmp_path),
                source,
                SettingsSaveLegacyOutward::SettingsSaveFailed,
            )
        })?;
        #[cfg(test)]
        project_atomic_checkpoint(path, SettingsSaveStage::TempSync)?;
        settings_file::sync_settings_file(&temporary).map_err(|source| {
            SettingsSaveError::io(
                SettingsSaveStage::TempSync,
                path,
                Some(&tmp_path),
                source,
                SettingsSaveLegacyOutward::SettingsSaveFailed,
            )
        })?;
        crate::path_identity::verify_opened_regular_file(&tmp_path, &temporary, false).map_err(
            |_| {
                SettingsSaveError::semantic(
                    SettingsSaveStage::TempVerify,
                    path,
                    Some(&tmp_path),
                    SettingsSaveReason::TempIdentityRejected,
                    SettingsSaveLegacyOutward::SettingsSaveFailed,
                )
            },
        )?;
        drop(temporary);

        #[cfg(test)]
        project_atomic_checkpoint(path, SettingsSaveStage::AtomicReplace)?;
        replace_settings_file_atomic(&tmp_path, path).map_err(|source| {
            SettingsSaveError::io(
                SettingsSaveStage::AtomicReplace,
                path,
                Some(&tmp_path),
                source,
                SettingsSaveLegacyOutward::SettingsSaveFailed,
            )
        })?;
        #[cfg(unix)]
        {
            settings_file::set_private_settings_permissions(path).map_err(|source| {
                SettingsSaveError::io(
                    SettingsSaveStage::TargetPermissions,
                    path,
                    Some(&tmp_path),
                    source,
                    SettingsSaveLegacyOutward::SettingsSaveFailed,
                )
            })?;
            settings_file::sync_settings_directory(dir).map_err(|source| {
                SettingsSaveError::io(
                    SettingsSaveStage::ParentDirectorySync,
                    path,
                    Some(&tmp_path),
                    source,
                    SettingsSaveLegacyOutward::SettingsSaveFailed,
                )
            })?;
        }
        #[cfg(test)]
        project_atomic_checkpoint(path, SettingsSaveStage::PostWriteRead)?;
        let (written, _) = crate::path_identity::read_bounded_regular(path, 16 * 1024 * 1024)
            .map_err(|_| {
                SettingsSaveError::semantic(
                    SettingsSaveStage::PostWriteRead,
                    path,
                    Some(&tmp_path),
                    SettingsSaveReason::PostWriteReadRejected,
                    SettingsSaveLegacyOutward::SettingsSaveFailed,
                )
            })?;
        if written != json {
            return Err(SettingsSaveError::semantic(
                SettingsSaveStage::PostWriteVerify,
                path,
                Some(&tmp_path),
                SettingsSaveReason::WrittenBytesMismatch,
                SettingsSaveLegacyOutward::SettingsSaveFailed,
            ));
        }
        Ok(())
    })();
    if result.is_err() && settings_file::settings_path_exists(&tmp_path) {
        let _ = settings_file::remove_settings_temporary(&tmp_path);
    }
    result?;

    log::debug!("Saved settings to {:?}", label);
    Ok(json)
}

#[cfg(windows)]
pub(crate) fn replace_settings_file_atomic_with_retry<Replace, Sleep>(
    replace: Replace,
    sleep: Sleep,
) -> std::io::Result<()>
where
    Replace: FnMut() -> std::io::Result<()>,
    Sleep: FnMut(std::time::Duration),
{
    crate::config::retry_transient_io_with(
        replace,
        sleep,
        |failed_attempt, total_attempts, error, delay| {
            if let Some(delay) = delay {
                let raw_os_error = error.raw_os_error().unwrap_or_default();
                log::warn!(
                    "Windows settings atomic replace attempt {} of {} failed with raw OS error {}; \
                        retrying in {} ms",
                    failed_attempt,
                    total_attempts,
                    raw_os_error,
                    delay.as_millis()
                );
            }
        },
    )
    .map_err(|failure| failure.error)
}

#[cfg(windows)]
pub(crate) fn replace_settings_file_atomic(
    source: &Path,
    destination: &Path,
) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;

    let source: Vec<u16> = source.as_os_str().encode_wide().chain(Some(0)).collect();
    let destination: Vec<u16> = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    replace_settings_file_atomic_with_retry(
        || {
            // SAFETY: both Vecs use encode_wide().chain(Some(0)), remain live,
            // and are never mutated during replacement attempts.
            unsafe { settings_file::replace_settings_file_windows_once(&source, &destination) }
        },
        std::thread::sleep,
    )
}

#[cfg(not(windows))]
pub(crate) fn replace_settings_file_atomic(
    source: &Path,
    destination: &Path,
) -> std::io::Result<()> {
    settings_file::rename_settings_file(source, destination)
}

/// Snapshot fixture/migration writer only. Production operations own a token
/// spanning fresh reads, mutation, sidecar commit, revalidation and rollback.
#[cfg(test)]
pub fn save_settings_with_project_paths(settings: &AppSettings) -> Result<(), String> {
    let dir = super::super::config_dir().ok_or("Could not determine home directory")?;
    let path = dir.join(SETTINGS_FILE_NAME);
    save_settings_with_project_paths_to_path(settings, &path)
}

#[cfg(test)]
pub(crate) fn save_settings_with_project_paths_to_path(
    settings: &AppSettings,
    path: &Path,
) -> Result<(), String> {
    reconcile_project_state_to_path(settings, path, true, true).map(|_| ())
}

/// #778/#881 + #1077: the preserve-disk wrapper behind the default
/// `save_settings`. Preserves all six on-disk project fields (materializing a
/// group only when disk has no truth), then hands off to the #774-hardened
/// atomic writer and returns the fresh-decoded settings.
pub(crate) fn save_settings_to_path_preserving_project_paths(
    settings: &AppSettings,
    path: &Path,
) -> Result<AppSettings, String> {
    save_settings_to_path_preserving_project_paths_typed(settings, path).map_err(|error| {
        report_settings_save_error(error, SettingsSaveReportSurface::GeneralSettings)
    })
}

pub(crate) fn save_settings_to_path_preserving_project_paths_typed(
    settings: &AppSettings,
    path: &Path,
) -> Result<AppSettings, SettingsSaveError> {
    save_settings_value(settings, path, ProjectWriteMode::Preserve)
}

/// Raw writer: serialize `AppSettings` verbatim (three primary project fields;
/// the companion fields are `#[serde(skip)]`, yielding a legacy-shaped file).
/// Used only for test seeding; production writes go through the preserve/
/// reconcile modes so companions are materialized.
#[cfg(test)]
pub(crate) fn save_settings_to_path(
    settings: &AppSettings,
    path: &Path,
) -> Result<(), SettingsSaveError> {
    let value = serde_json::to_value(settings).map_err(|source| {
        let outward = format!("Failed to serialize settings: {}", source);
        SettingsSaveError::json(
            SettingsSaveStage::Serialize,
            path,
            None,
            source,
            SettingsSaveLegacyOutward::Serialize(outward),
        )
    })?;
    write_value_atomic(&value, path).map(|_| ())
}
