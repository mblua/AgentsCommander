use super::{Path, PathBuf};
use crate::config::instance_artifacts::{SETTINGS_BACKUP_PREFIX, SETTINGS_BACKUP_SUFFIX};
use crate::filesystem::settings_file;

pub(crate) fn write_pre_384_v1_backup(settings_path: &Path, contents: &str) -> Result<(), String> {
    let backup_path = settings_path
        .with_file_name(crate::config::instance_artifacts::SETTINGS_MIGRATION_BACKUP_384_V1_NAME);
    if settings_file::settings_path_exists(&backup_path) {
        return Ok(());
    }
    settings_file::write_settings_migration_backup(&backup_path, contents)
        .map_err(|e| format!("Failed to write {}: {}", backup_path.display(), e))?;
    log::info!(
        "[settings-migration] wrote pre-384 v1 settings backup to {:?}",
        backup_path
    );
    Ok(())
}

/// #2058 - number of previous `settings.json` generations kept beside the live
/// file. Slot 1 is the version the most recent save replaced, slot
/// `SETTINGS_BACKUP_KEEP` the oldest kept; the next rotation drops it.
pub(crate) const SETTINGS_BACKUP_KEEP: u32 = 5;

pub(crate) fn settings_backup_path(settings_path: &Path, index: u32) -> PathBuf {
    settings_path.with_file_name(format!(
        "{SETTINGS_BACKUP_PREFIX}{index}{SETTINGS_BACKUP_SUFFIX}"
    ))
}

/// #2058 - archive `previous` (the bytes a just-completed save replaced) into
/// slot 1, shifting the older slots down and dropping slot
/// `SETTINGS_BACKUP_KEEP`.
///
/// Best effort by design: the caller's save has already succeeded on disk, so a
/// rotation failure is logged and never propagated. Runs with
/// `settings.json.lock` held by the caller, and never touches
/// `settings.pre-*.json`, `settings.local.json`, the lock file or a write
/// temporary. Deliberate local mirror of
/// `sessions_persistence::rotate_orphan_archive`.
pub(crate) fn rotate_settings_backups(settings_path: &Path, previous: &[u8]) {
    rotate_settings_backups_labeled(settings_path, settings_path, previous)
}

pub(crate) fn rotate_settings_backups_labeled(settings_path: &Path, label: &Path, previous: &[u8]) {
    // D8: a non-regular slot is a user-placed symlink or directory; never follow
    // or clobber it.
    for index in 1..=SETTINGS_BACKUP_KEEP {
        let slot = settings_backup_path(settings_path, index);
        match settings_file::settings_path_metadata(&slot) {
            Ok(metadata) => {
                if !metadata.is_file() {
                    log::warn!(
                        "[settings] #2058 aborting backup rotation: slot {:?} is not a regular \
                            file",
                        settings_backup_path(label, index)
                    );
                    return;
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                log::warn!(
                    "[settings] #2058 aborting backup rotation: stat of slot {:?} failed: {}",
                    settings_backup_path(label, index),
                    error
                );
                return;
            }
        }
    }

    // Shift the older slots down; the last rename drops the oldest generation.
    for index in (1..SETTINGS_BACKUP_KEEP).rev() {
        let from = settings_backup_path(settings_path, index);
        let to = settings_backup_path(settings_path, index + 1);
        match settings_file::settings_path_metadata(&from) {
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                log::warn!(
                    "[settings] #2058 aborting backup rotation: stat of slot {:?} failed: {}",
                    settings_backup_path(label, index),
                    error
                );
                return;
            }
        }
        if let Err(error) = settings_file::rename_settings_file(&from, &to) {
            log::warn!(
                "[settings] #2058 aborting backup rotation: rename {:?} -> {:?} failed: {}",
                settings_backup_path(label, index),
                settings_backup_path(label, index + 1),
                error
            );
            return;
        }
    }

    // Write the just-replaced bytes into slot 1.
    let slot_one = settings_backup_path(settings_path, 1);
    match settings_file::open_settings_backup_slot(&slot_one) {
        Ok(mut file) => {
            if let Err(error) = settings_file::write_settings_bytes(&mut file, previous) {
                log::warn!(
                    "[settings] #2058 aborting backup rotation: write to slot {:?} failed: {}",
                    settings_backup_path(label, 1),
                    error
                );
                return;
            }
        }
        Err(error) => {
            log::warn!(
                "[settings] #2058 aborting backup rotation: open of slot {:?} failed: {}",
                settings_backup_path(label, 1),
                error
            );
            return;
        }
    }

    log::info!(
        "[settings] #2058 archived the replaced settings to {:?}",
        settings_backup_path(label, 1)
    );
}
