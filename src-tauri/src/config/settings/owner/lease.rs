use super::{owner_save_error_text, SettingsOwnerError};
use crate::config::instance_artifacts::SETTINGS_OWNER_LOCK_FILE_NAME;
use crate::config::settings::{SettingsFileLock, SettingsSaveCause, SettingsSaveReason};
use std::path::{Path, PathBuf};

/// The lifecycle lease: an exclusive lock on the stable owner sidecar inside a
/// retained physical directory. Dropping it unlocks; the sidecar is never
/// unlinked.
pub(crate) struct SettingsOwnerLease {
    pub(crate) _lock: SettingsFileLock,
    pub(crate) retained: crate::path_identity::RetainedDirectory,
    pub(crate) settings_path: PathBuf,
}

#[cfg(all(test, windows))]
pub(crate) struct SettingsOwnerLeaseVerifyFault {
    pub(crate) path: PathBuf,
    pub(crate) armed: bool,
    pub(crate) hits: std::rc::Rc<std::cell::Cell<usize>>,
}

#[cfg(all(test, windows))]
thread_local! {
    pub(crate) static SETTINGS_OWNER_LEASE_VERIFY_FAULT: std::cell::RefCell<
        Option<SettingsOwnerLeaseVerifyFault>,
    > = const { std::cell::RefCell::new(None) };
}

#[cfg(all(test, windows))]
pub(crate) struct SettingsOwnerLeaseVerifyFaultGuard {
    pub(crate) path: PathBuf,
    pub(crate) hits: std::rc::Rc<std::cell::Cell<usize>>,
}

#[cfg(all(test, windows))]
impl SettingsOwnerLeaseVerifyFaultGuard {
    pub(crate) fn install(path: PathBuf) -> Self {
        let hits = std::rc::Rc::new(std::cell::Cell::new(0));
        SETTINGS_OWNER_LEASE_VERIFY_FAULT.with(|slot| {
            let mut slot = slot.borrow_mut();
            assert!(slot.is_none(), "lease verification faults cannot nest");
            *slot = Some(SettingsOwnerLeaseVerifyFault {
                path: path.clone(),
                armed: true,
                hits: std::rc::Rc::clone(&hits),
            });
        });
        Self { path, hits }
    }
}

#[cfg(all(test, windows))]
impl Drop for SettingsOwnerLeaseVerifyFaultGuard {
    fn drop(&mut self) {
        SETTINGS_OWNER_LEASE_VERIFY_FAULT.with(|slot| {
            let mut slot = slot.borrow_mut();
            if slot.as_ref().is_some_and(|fault| {
                fault.path == self.path && std::rc::Rc::ptr_eq(&fault.hits, &self.hits)
            }) {
                *slot = None;
            }
        });
    }
}

impl SettingsOwnerLease {
    pub(crate) fn acquire(
        settings_path: &Path,
        wait: std::time::Duration,
    ) -> Result<Self, SettingsOwnerError> {
        let identity = |reason: &str| SettingsOwnerError::Lifecycle(reason.to_string());
        let parent = settings_path
            .parent()
            .ok_or_else(|| identity("missing_parent_directory"))?;
        let leaf = settings_path
            .file_name()
            .ok_or_else(|| identity("missing_file_name"))?;
        // The chain check stops at the first link, so a `..` after a tolerated
        // link (macOS `/var`) would otherwise be resolved by canonicalize.
        if parent
            .components()
            .any(|component| component == std::path::Component::ParentDir)
        {
            return Err(identity("lock_identity_rejected"));
        }
        match crate::path_identity::verify_component_chain_reason(parent) {
            Ok(()) | Err(crate::path_identity::PathRule::ComponentIsLink) => {}
            Err(_) => return Err(identity("lock_identity_rejected")),
        }
        let physical_parent =
            std::fs::canonicalize(parent).map_err(|_| identity("lock_identity_rejected"))?;
        let retained = crate::path_identity::retain_directory(&physical_parent)
            .map_err(|_| identity("lock_identity_rejected"))?;
        let physical = retained.identity().canonical_path.join(leaf);
        let lock = SettingsFileLock::acquire_named(
            &physical,
            SETTINGS_OWNER_LOCK_FILE_NAME,
            wait,
            "settings_owner_lifecycle",
        )
        .map_err(|error| match error.cause {
            SettingsSaveCause::Semantic(SettingsSaveReason::LockTimedOut) => {
                SettingsOwnerError::LifecycleTimedOut
            }
            _ => SettingsOwnerError::Lifecycle(owner_save_error_text(&error)),
        })?;
        retained
            .verify_current()
            .map_err(|_| identity("lock_identity_rejected"))?;
        Ok(Self {
            _lock: lock,
            retained,
            settings_path: physical,
        })
    }

    pub(crate) fn verify(&self) -> Result<(), SettingsOwnerError> {
        #[cfg(all(test, windows))]
        if SETTINGS_OWNER_LEASE_VERIFY_FAULT.with(|slot| {
            let mut slot = slot.borrow_mut();
            let Some(fault) = slot.as_mut() else {
                return false;
            };
            if fault.armed && fault.path == self.settings_path {
                fault.armed = false;
                fault.hits.set(fault.hits.get() + 1);
                true
            } else {
                false
            }
        }) {
            return Err(SettingsOwnerError::Lifecycle(
                "lock_identity_rejected".to_string(),
            ));
        }
        self.retained
            .verify_current()
            .map_err(|_| SettingsOwnerError::Lifecycle("lock_identity_rejected".to_string()))
    }
}
