use super::super::{
    next_settings_save_op_id, rotate_settings_backups_labeled, write_value_atomic_labeled,
    AppSettings, SettingsFileLock, SettingsSaveError, SettingsSaveLegacyOutward,
    SettingsSaveReason, SettingsSaveStage,
};
use super::{
    FIELD_ARCHIVED, FIELD_ARCHIVED_REL, FIELD_PROJECT_PATH, FIELD_PROJECT_PATHS,
    FIELD_PROJECT_PATHS_REL, FIELD_PROJECT_PATH_REL,
};
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};

// Sole project registration authority; callers hold the shared settings lock.
pub(crate) fn project_paths_path(settings_path: &Path) -> PathBuf {
    settings_path.with_file_name(crate::config::instance_artifacts::PROJECT_PATHS_FILE_NAME)
}

pub(crate) fn raw_project_projection(object: &Map<String, Value>) -> Map<String, Value> {
    [
        FIELD_PROJECT_PATH,
        FIELD_PROJECT_PATH_REL,
        FIELD_PROJECT_PATHS,
        FIELD_PROJECT_PATHS_REL,
        FIELD_ARCHIVED,
        FIELD_ARCHIVED_REL,
    ]
    .into_iter()
    .filter_map(|key| {
        object
            .get(key)
            .map(|value| (key.to_string(), value.clone()))
    })
    .collect()
}

pub(crate) fn project_migration_error(
    path: &Path,
    stage: SettingsSaveStage,
    reason: &str,
) -> SettingsSaveError {
    project_migration_error_labeled(path, path, stage, reason)
}

pub(crate) fn project_migration_error_labeled(
    path: &Path,
    label: &Path,
    stage: SettingsSaveStage,
    reason: &str,
) -> SettingsSaveError {
    let kind = if reason.starts_with("project_paths_conflict") {
        SettingsSaveReason::ProjectPathsConflict
    } else if reason.starts_with("project_paths_backup_mismatch") {
        SettingsSaveReason::ProjectPathsBackupMismatch
    } else {
        SettingsSaveReason::ProjectPathsSourceRejected
    };
    SettingsSaveError::semantic(
        stage,
        path,
        None,
        kind,
        SettingsSaveLegacyOutward::DiskRead(format!("{}: {reason}", label.display())),
    )
}

pub(crate) fn read_project_source_bytes_labeled_with_limit(
    path: &Path,
    label: &Path,
    limit: usize,
) -> Result<Option<Vec<u8>>, SettingsSaveError> {
    match std::fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(SettingsSaveError::io(
                SettingsSaveStage::ProjectPathsRefreshDiskGate,
                path,
                None,
                error,
                SettingsSaveLegacyOutward::SettingsSaveFailed,
            ))
        }
        Ok(_) => {}
    }
    crate::path_identity::read_bounded_regular(path, limit)
        .map(|(bytes, _)| Some(bytes))
        .map_err(|reason| {
            project_migration_error_labeled(
                path,
                label,
                SettingsSaveStage::ProjectPathsRefreshDiskGate,
                &reason,
            )
        })
}

#[derive(Debug)]
pub(crate) struct RawProjectSource {
    pub(crate) object: Map<String, Value>,
    pub(crate) bytes: Vec<u8>,
}

pub(crate) fn read_raw_project_source(
    path: &Path,
) -> Result<Option<RawProjectSource>, SettingsSaveError> {
    read_raw_project_source_with_limit(path, 16 * 1024 * 1024, false)
}

pub(crate) fn read_raw_project_source_with_limit(
    path: &Path,
    limit: usize,
    legacy_settings_diagnostic: bool,
) -> Result<Option<RawProjectSource>, SettingsSaveError> {
    read_raw_project_source_labeled_with_limit(path, path, limit, legacy_settings_diagnostic)
}

pub(crate) fn read_raw_project_source_labeled_with_limit(
    path: &Path,
    label: &Path,
    limit: usize,
    legacy_settings_diagnostic: bool,
) -> Result<Option<RawProjectSource>, SettingsSaveError> {
    let Some(bytes) = read_project_source_bytes_labeled_with_limit(path, label, limit)? else {
        return Ok(None);
    };
    raw_project_source_from_bytes(bytes, path, label, legacy_settings_diagnostic).map(Some)
}

/// The decode half of the raw project-source reader, shared with the owner's
/// single capture so selection judges captured bytes exactly like read ones.
pub(crate) fn raw_project_source_from_bytes(
    bytes: Vec<u8>,
    path: &Path,
    label: &Path,
    legacy_settings_diagnostic: bool,
) -> Result<RawProjectSource, SettingsSaveError> {
    let value = crate::path_identity::parse_json_no_duplicates(&bytes).map_err(|reason| {
        let mut error = project_migration_error_labeled(
            path,
            label,
            SettingsSaveStage::ProjectPathsRefreshDiskGate,
            &reason,
        );
        if legacy_settings_diagnostic {
            error.legacy_outward = SettingsSaveLegacyOutward::DiskRead(format!(
                "settings.json exists but could not be parsed ({}: {reason}); refusing to modify \
                    it - fix or remove the file first",
                label.display()
            ));
        }
        error
    })?;
    let Value::Object(object) = value else {
        return Err(project_migration_error_labeled(
            path,
            label,
            SettingsSaveStage::ProjectPathsRefreshDiskGate,
            "project_paths_source_not_object",
        ));
    };
    Ok(RawProjectSource { object, bytes })
}

#[derive(Debug)]
pub(crate) struct SelectedProjectSource {
    pub(crate) legacy: Option<RawProjectSource>,
    pub(crate) destination: Option<RawProjectSource>,
    pub(crate) projection: Map<String, Value>,
}

// Capture both immutable objects only while the original settings token is held.
pub(crate) fn select_project_source_locked(
    transaction: &ProjectPathsWriteTransaction,
) -> Result<SelectedProjectSource, SettingsSaveError> {
    select_project_source_at_locked_path(&transaction.path)
}

pub(crate) fn select_project_source_at_locked_path(
    path: &Path,
) -> Result<SelectedProjectSource, SettingsSaveError> {
    select_project_source_at_locked_path_with_limit(path, 16 * 1024 * 1024)
}

pub(crate) fn select_project_source_at_locked_path_with_limit(
    path: &Path,
    limit: usize,
) -> Result<SelectedProjectSource, SettingsSaveError> {
    select_project_source_at_labeled_path_with_limit(path, path, limit)
}

pub(crate) fn select_project_source_at_labeled_path_with_limit(
    path: &Path,
    label: &Path,
    limit: usize,
) -> Result<SelectedProjectSource, SettingsSaveError> {
    let legacy = read_raw_project_source_labeled_with_limit(path, label, limit, true)?;
    let destination_path = project_paths_path(path);
    let destination_label = project_paths_path(label);
    let destination = read_raw_project_source_labeled_with_limit(
        &destination_path,
        &destination_label,
        limit,
        false,
    )?;
    select_project_source_from_raw(
        legacy,
        destination,
        label,
        &destination_path,
        &destination_label,
    )
}

/// The pure selector over both raw sources: the destination wins, and a non-empty
/// legacy projection that differs from it is a conflict, never a silent choice.
pub(crate) fn select_project_source_from_raw(
    legacy: Option<RawProjectSource>,
    destination: Option<RawProjectSource>,
    label: &Path,
    destination_path: &Path,
    destination_label: &Path,
) -> Result<SelectedProjectSource, SettingsSaveError> {
    let legacy_projection = legacy
        .as_ref()
        .map(|source| raw_project_projection(&source.object))
        .unwrap_or_default();
    let projection = match &destination {
        None => legacy_projection,
        Some(source) => {
            let projection = raw_project_projection(&source.object);
            if !legacy_projection.is_empty() && legacy_projection != projection {
                return Err(project_migration_error_labeled(
                    destination_path,
                    destination_label,
                    SettingsSaveStage::ProjectPathsRefreshDiskGate,
                    &format!(
                        "project_paths_conflict; source={}; destination={}; preserve both \
                        files and reconcile raw projections to equality",
                        label.display(),
                        destination_label.display()
                    ),
                ));
            }
            projection
        }
    };
    Ok(SelectedProjectSource {
        legacy,
        destination,
        projection,
    })
}

// Read-only selection acquires before even checking either source's existence.
#[cfg(test)]
pub(crate) fn select_project_source_at_path(
    path: &Path,
) -> Result<Map<String, Value>, SettingsSaveError> {
    let lock = SettingsFileLock::acquire_contextual(
        path,
        std::time::Duration::from_secs(2),
        "select_project_source_at_path",
    )?;
    let transaction = ProjectPathsWriteTransaction {
        _lock: lock,
        path: path.to_path_buf(),
        fresh: AppSettings::default(),
        revalidated_after_failure: false,
    };
    Ok(select_project_source_locked(&transaction)?.projection)
}

pub(crate) fn publish_project_migration_backup_labeled(
    settings_path: &Path,
    label: &Path,
    bytes: &[u8],
    checkpoint: &mut impl FnMut(&str, &Path) -> Result<(), SettingsSaveError>,
) -> Result<(), SettingsSaveError> {
    use std::io::Write as _;
    let backup = settings_path
        .with_file_name(crate::config::instance_artifacts::PROJECT_PATHS_BACKUP_FILE_NAME);
    let backup_label =
        label.with_file_name(crate::config::instance_artifacts::PROJECT_PATHS_BACKUP_FILE_NAME);
    let mismatch = || {
        project_migration_error_labeled(
            &backup,
            &backup_label,
            SettingsSaveStage::PostWriteVerify,
            &format!(
                "project_paths_backup_mismatch; source={}; destination={}; backup={}; stop all \
            AC/old processes, preserve copies and orphan temps, rename backup without overwrite to \
            project-paths.pre-migration.recovered-<UTC>-<unique>.no-git.json, verify intended \
            legacy projection and \
            retry",
                label.display(),
                project_paths_path(label).display(),
                backup_label.display()
            ),
        )
    };
    if let Some(existing) =
        read_project_source_bytes_labeled_with_limit(&backup, &backup_label, 16 * 1024 * 1024)?
    {
        return if existing == bytes {
            Ok(())
        } else {
            Err(mismatch())
        };
    }
    let parent = backup.parent().ok_or_else(|| {
        project_migration_error_labeled(
            &backup,
            &backup_label,
            SettingsSaveStage::PrepareTarget,
            "missing parent",
        )
    })?;
    crate::path_identity::verify_component_chain(parent).map_err(|reason| {
        project_migration_error_labeled(
            &backup,
            &backup_label,
            SettingsSaveStage::PrepareTarget,
            &reason,
        )
    })?;
    let mut name = backup.file_name().unwrap().to_os_string();
    name.push(format!(
        ".{}.{}.tmp",
        std::process::id(),
        next_settings_save_op_id()
    ));
    let temp = parent.join(name);
    checkpoint("backup_create", &temp)?;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let io_error = |stage, source| {
        SettingsSaveError::io(
            stage,
            &backup,
            Some(&temp),
            source,
            SettingsSaveLegacyOutward::SettingsSaveFailed,
        )
    };
    let mut file = options
        .open(&temp)
        .map_err(|error| io_error(SettingsSaveStage::TempCreate, error))?;
    crate::path_identity::verify_opened_regular_file(&temp, &file, false).map_err(|reason| {
        project_migration_error_labeled(
            &temp,
            &label.with_file_name(temp.file_name().unwrap()),
            SettingsSaveStage::TempVerify,
            &reason,
        )
    })?;
    let prefix = bytes.len() / 2;
    file.write_all(&bytes[..prefix])
        .map_err(|error| io_error(SettingsSaveStage::TempWrite, error))?;
    checkpoint("backup_prefix", &temp)?;
    file.write_all(&bytes[prefix..])
        .map_err(|error| io_error(SettingsSaveStage::TempWrite, error))?;
    checkpoint("backup_write", &temp)?;
    file.sync_all()
        .map_err(|error| io_error(SettingsSaveStage::TempSync, error))?;
    checkpoint("backup_sync", &temp)?;
    drop(file);
    if read_project_source_bytes_labeled_with_limit(
        &temp,
        &label.with_file_name(temp.file_name().unwrap()),
        16 * 1024 * 1024,
    )?
    .as_deref()
        != Some(bytes)
    {
        return Err(mismatch());
    }
    checkpoint("backup_before_rename", &temp)?;
    // Never use the replacing atomic writer for the immutable fixed name.
    if let Some(existing) =
        read_project_source_bytes_labeled_with_limit(&backup, &backup_label, 16 * 1024 * 1024)?
    {
        return if existing == bytes {
            Ok(())
        } else {
            Err(mismatch())
        };
    }
    if let Err(error) = std::fs::rename(&temp, &backup) {
        match read_project_source_bytes_labeled_with_limit(
            &backup,
            &backup_label,
            16 * 1024 * 1024,
        )? {
            Some(existing) if existing == bytes => {}
            Some(_) => return Err(mismatch()),
            None => return Err(io_error(SettingsSaveStage::AtomicReplace, error)),
        }
    }
    #[cfg(unix)]
    std::fs::File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| io_error(SettingsSaveStage::ParentDirectorySync, error))?;
    if read_project_source_bytes_labeled_with_limit(&backup, &backup_label, 16 * 1024 * 1024)?
        .as_deref()
        != Some(bytes)
    {
        return Err(mismatch());
    }
    checkpoint("backup_published", &backup)
}

pub(crate) fn ensure_project_paths_migrated_locked(
    transaction: &ProjectPathsWriteTransaction,
) -> Result<(), SettingsSaveError> {
    ensure_project_paths_migrated_at_locked_path(&transaction.path, &mut |_, _| Ok(()))
}

#[cfg(test)]
pub(crate) fn ensure_project_paths_migrated_with(
    transaction: &ProjectPathsWriteTransaction,
    checkpoint: &mut impl FnMut(&str, &Path) -> Result<(), SettingsSaveError>,
) -> Result<(), SettingsSaveError> {
    ensure_project_paths_migrated_at_locked_path(&transaction.path, checkpoint)
}

pub(crate) fn ensure_project_paths_migrated_at_locked_path(
    path: &Path,
    checkpoint: &mut impl FnMut(&str, &Path) -> Result<(), SettingsSaveError>,
) -> Result<(), SettingsSaveError> {
    ensure_project_paths_migrated_at_labeled_path(path, path, checkpoint)
}

pub(crate) fn ensure_project_paths_migrated_at_labeled_path(
    path: &Path,
    label: &Path,
    checkpoint: &mut impl FnMut(&str, &Path) -> Result<(), SettingsSaveError>,
) -> Result<(), SettingsSaveError> {
    let selected = select_project_source_at_labeled_path_with_limit(path, label, 16 * 1024 * 1024)?;
    let destination = project_paths_path(path);
    let destination_label = project_paths_path(label);
    let legacy_fields = selected
        .legacy
        .as_ref()
        .is_some_and(|source| !raw_project_projection(&source.object).is_empty());
    if selected.destination.is_none() {
        if legacy_fields {
            publish_project_migration_backup_labeled(
                path,
                label,
                &selected.legacy.as_ref().unwrap().bytes,
                checkpoint,
            )?;
        }
        checkpoint("destination_before_write", &destination)?;
        write_value_atomic_labeled(
            &Value::Object(selected.projection.clone()),
            &destination,
            &destination_label,
        )?;
        checkpoint("destination_published", &destination)?;
    }
    if !legacy_fields {
        return Ok(());
    }
    checkpoint("destination_readback", &destination)?;
    let readback = read_raw_project_source_labeled_with_limit(
        &destination,
        &destination_label,
        16 * 1024 * 1024,
        false,
    )?
    .ok_or_else(|| {
        project_migration_error_labeled(
            &destination,
            &destination_label,
            SettingsSaveStage::PostWriteRead,
            "project_paths_destination_disappeared",
        )
    })?;
    if raw_project_projection(&readback.object) != selected.projection {
        return Err(project_migration_error_labeled(
            &destination,
            &destination_label,
            SettingsSaveStage::PostWriteVerify,
            "project_paths_readback_mismatch",
        ));
    }
    checkpoint("source_cleanup", path)?;
    let original = selected.legacy.unwrap();
    let mut cleaned = original.object;
    for key in selected.projection.keys() {
        cleaned.remove(key);
    }
    let written = write_value_atomic_labeled(&Value::Object(cleaned), path, label)?;
    if original.bytes != written {
        rotate_settings_backups_labeled(path, label, &original.bytes);
    }
    checkpoint("cleanup_complete", path)
}

/// Owns the shared file lock throughout a synchronous project operation.
/// Never carry this token across an await.
pub(crate) struct ProjectPathsWriteTransaction {
    pub(crate) _lock: SettingsFileLock,
    pub(crate) path: PathBuf,
    pub(crate) fresh: AppSettings,
    pub(crate) revalidated_after_failure: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProjectPathsRepairStage {
    Read,
    Write,
}
