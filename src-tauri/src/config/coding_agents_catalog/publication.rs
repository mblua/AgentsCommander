use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::time::Instant;

use crate::filesystem::{catalog, config_seed};

#[cfg(test)]
use super::take_catalog_path_fault;
use super::{
    catalog_dir, catalog_lock_timeout, inject_catalog_failure, is_reparse_point,
    publication_temp_name_for_destination, CATALOG_LOCK_FILENAME, CATALOG_LOCK_POLL_INTERVAL,
    CATALOG_MANIFEST_FILENAME, LOCAL_CATALOG_FILENAME, MIGRATION_BACKUP_FILENAME,
    MIGRATION_JOURNAL_FILENAME, SEED_TEMP_COUNTER,
};

// ---------------------------------------------------------------------------
// #1968 write machinery: directory/lock discipline, exclusive publication,
// migration, recovery, refresh and initialization.
// ---------------------------------------------------------------------------

/// The catalog publication targets and the lock, resolved once.
pub(crate) struct CatalogPaths {
    pub(crate) base: PathBuf,
    pub(crate) local: PathBuf,
    pub(crate) backup: PathBuf,
    pub(crate) journal: PathBuf,
    pub(crate) lock: PathBuf,
}

impl CatalogPaths {
    pub(crate) fn new(dir: &Path) -> Self {
        Self::with_names(dir, CATALOG_MANIFEST_FILENAME, LOCAL_CATALOG_FILENAME)
    }

    /// #2715 - the same composition over explicit data-file names, so an
    /// interrupted #1968 migration is recovered over the PRE-migration names.
    pub(crate) fn with_names(dir: &Path, base: &str, local: &str) -> Self {
        Self {
            base: dir.join(base),
            local: dir.join(local),
            backup: dir.join(MIGRATION_BACKUP_FILENAME),
            journal: dir.join(MIGRATION_JOURNAL_FILENAME),
            lock: dir.join(CATALOG_LOCK_FILENAME),
        }
    }
}

/// Create or validate the catalog directory and return its CANONICAL form, so
/// every cooperating writer locks one place even through raw, dot-segment or
/// case aliases. Creation may only happen under an existing real project `.ac`
/// directory; an existing catalog directory that is a symlink/reparse point or
/// a non-directory fails visibly.
pub(crate) fn ensure_catalog_dir(ac_dir: &Path) -> Result<PathBuf, String> {
    if !ac_dir.is_absolute() {
        return Err(format!(
            "the catalog root {} is not absolute; refusing to create it relative to the \
                process CWD",
            ac_dir.display()
        ));
    }
    let dir = catalog_dir(ac_dir);
    match config_seed::inspect_seed_destination(&dir) {
        Ok(meta) => {
            if meta.file_type().is_symlink() || is_reparse_point(&meta) {
                return Err(format!(
                    "the catalog directory {} is a symbolic link or reparse point",
                    dir.display()
                ));
            }
            if !meta.is_dir() {
                return Err(format!(
                    "the catalog path {} exists and is not a directory",
                    dir.display()
                ));
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            // `.ac` itself may be absent (a direct caller owns the path); an
            // EXISTING `.ac` must be a real directory, never a link, and the
            // catalog directory is created inside it.
            if let Ok(parent) = config_seed::inspect_seed_destination(ac_dir) {
                if parent.file_type().is_symlink() || is_reparse_point(&parent) || !parent.is_dir()
                {
                    return Err(format!(
                        "the project .ac directory {} is not a real directory; refusing to create \
                            the catalog",
                        ac_dir.display()
                    ));
                }
            }
            config_seed::ensure_seed_directory(&dir).map_err(|e| {
                format!(
                    "failed to create the catalog directory {} ({e})",
                    dir.display()
                )
            })?;
        }
        Err(error) => {
            return Err(format!(
                "failed to inspect the catalog directory {} ({error})",
                dir.display()
            ))
        }
    }
    catalog::canonical_catalog_path(&dir).map_err(|e| {
        format!(
            "failed to canonicalize the catalog directory {} ({e})",
            dir.display()
        )
    })
}

pub(crate) fn sync_parent_directory(path: &Path) {
    catalog::sync_parent_directory(path);
}

pub(crate) fn write_synced_temp(temp: &Path, bytes: &[u8]) -> Result<(), String> {
    catalog::write_synced_temp(temp, bytes)
}

/// The hard-link publication boundary. Production compiles to exactly
/// `std::fs::hard_link(source, destination)`; the `#[cfg(test)]` arm lets a
/// destination-aware fixture drive the production cleanup/error-return code
/// with a filesystem-style `ErrorKind::Unsupported` result. That is arm-level
/// behavioral evidence, never a claim of measured unsupported-filesystem
/// support.
pub(crate) fn publish_hard_link(source: &Path, destination: &Path) -> std::io::Result<()> {
    #[cfg(test)]
    if let Some(kind) = take_catalog_path_fault("hard_link", destination) {
        return Err(std::io::Error::new(kind, "injected hard-link failure"));
    }
    catalog::link_catalog_temp(source, destination)
}

pub(crate) fn publication_temp_path(parent: &Path, destination_file_name: &str) -> PathBuf {
    let counter = SEED_TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    parent.join(publication_temp_name_for_destination(
        destination_file_name,
        std::process::id(),
        counter,
    ))
}

/// Publish `bytes` at a destination that must NOT exist, without ever exposing
/// partial bytes: a same-directory create-new temp is written, flushed and
/// fsynced, then hard-linked into place, and only this run's temp is unlinked.
/// A filesystem without hard links fails visibly; there is deliberately no
/// copy-to-destination fallback that could expose a partially written file.
pub(crate) fn publish_exclusive(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("publication path {} has no parent", path.display()))?;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("publication path {} has no file name", path.display()))?;
    let temp = publication_temp_path(parent, name);
    write_synced_temp(&temp, bytes)?;
    inject_catalog_failure("publication_temp_synced")?;
    let result = publish_hard_link(&temp, path);
    let _ = catalog::remove_catalog_temp(&temp);
    match result {
        Ok(()) => {
            sync_parent_directory(path);
            Ok(())
        }
        Err(error) => Err(format!(
            "exclusive publication of {} failed via a hard link ({error}); the destination was \
                left untouched and no partial bytes were written",
            path.display()
        )),
    }
}

/// Replace an existing destination atomically through the vetted
/// `root_agent::atomic_replace_existing` primitive (plain rename when absent,
/// `ReplaceFileW` when present), after a same-directory create-new temp is
/// written, flushed and fsynced. The temp is removed on every failure path.
pub(crate) fn publish_replace(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("publication path {} has no parent", path.display()))?;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("publication path {} has no file name", path.display()))?;
    let temp = publication_temp_path(parent, name);
    write_synced_temp(&temp, bytes)?;
    inject_catalog_failure("publication_temp_synced")?;
    if let Err(error) = crate::config::root_agent::atomic_replace_existing(&temp, path) {
        let _ = catalog::remove_catalog_temp(&temp);
        return Err(error);
    }
    sync_parent_directory(path);
    Ok(())
}

/// A held catalog lock. The `File` IS the lock: dropping the guard closes the
/// handle and releases the OS lock, including on panic or process death. The
/// sidecar file itself is deliberately never deleted.
#[derive(Debug)]
pub(crate) struct CatalogLockGuard {
    pub(crate) _file: File,
}

/// Acquire the catalog sidecar lock: nontruncating create-once, 50 ms polling
/// against a 5 s deadline, with a distinct `catalogLockTimeout` failure. An OS
/// error that is not ordinary contention stops immediately with a visible
/// message: a filesystem without lock support must not silently degrade to
/// process-only exclusion.
pub(crate) fn acquire_catalog_lock(paths: &CatalogPaths) -> Result<CatalogLockGuard, String> {
    match config_seed::inspect_seed_destination(&paths.lock) {
        Ok(meta) if meta.file_type().is_symlink() || is_reparse_point(&meta) || !meta.is_file() => {
            return Err(format!(
                "the coding-agent catalog lock {} must be a regular non-symlink file",
                paths.lock.display()
            ))
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(format!(
                "failed to inspect the coding-agent catalog lock {} ({error})",
                paths.lock.display()
            ))
        }
    }
    let file = catalog::open_catalog_lock(&paths.lock).map_err(|e| {
        format!(
            "failed to open the coding-agent catalog lock {} ({e})",
            paths.lock.display()
        )
    })?;
    if !catalog::catalog_lock_is_regular(&file) {
        return Err(format!(
            "the coding-agent catalog lock {} must be a regular file",
            paths.lock.display()
        ));
    }
    let timeout = catalog_lock_timeout();
    let started = Instant::now();
    loop {
        match catalog::try_catalog_lock(&file) {
            Ok(()) => return Ok(CatalogLockGuard { _file: file }),
            Err(catalog::TryLockError::WouldBlock) if started.elapsed() >= timeout => {
                return Err(format!(
                    "catalogLockTimeout: timed out after {} ms waiting for the coding-agent \
                        catalog lock '{}'",
                    timeout.as_millis(),
                    paths.lock.display()
                ))
            }
            Err(catalog::TryLockError::WouldBlock) => {
                let remaining = timeout.saturating_sub(started.elapsed());
                std::thread::sleep(CATALOG_LOCK_POLL_INTERVAL.min(remaining));
            }
            Err(catalog::TryLockError::Error(error)) => {
                return Err(format!(
                    "the coding-agent catalog lock {} could not be acquired ({error}); this \
                        filesystem does not support the lock and no process-only fallback is used",
                    paths.lock.display()
                ))
            }
        }
    }
}
