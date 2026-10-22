use super::super::super::AppSettings;
use super::{agents_instance_path, read_agents_instance_file_labeled};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// #2716 (B3, option C) - whether the agents file could be read on this load.
/// `Default` is "readable". Carried on `AppSettings` as a never-serialized field,
/// like `local_overlay_state`, so no save can write it and no client can forge it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) enum AgentsLayerState {
    #[default]
    Readable,
    Unreadable {
        path: PathBuf,
        reason: String,
    },
}

impl AgentsLayerState {
    /// The agents file's path while the layer is unreadable.
    pub(crate) fn unreadable_path(&self) -> Option<&Path> {
        match self {
            Self::Readable => None,
            Self::Unreadable { path, .. } => Some(path),
        }
    }
}

/// #2716 (B3, option C) - the CLI's answer: the list is unknown, not empty.
pub(crate) fn agents_unreadable_cli_error(path: &Path) -> String {
    format!(
        "Coding agents are unavailable: AgentsCommander could not read {}. Fix or delete that file \
            and restart.",
        path.display()
    )
}

/// #2716 (B3, option C) - notice 1: the startup dialog, raised by the error log.
pub(crate) fn agents_unreadable_startup_notice(path: &Path) -> String {
    format!(
        "Coding agents are unavailable: AgentsCommander could not read {}. Agent sessions will not \
            start until this file is fixed or deleted. Every other setting is unchanged. While \
            this file is unreadable, changes to your sessions are not saved.",
        path.display()
    )
}

/// #2716 (B3, option C) - notice 3: the error every spawn entry point returns.
pub(crate) fn agents_unreadable_session_error(path: &Path) -> String {
    format!(
        "Cannot start this coding agent: AgentsCommander could not read {}. Fix or delete that \
            file and restart.",
        path.display()
    )
}

/// #2716 (B3, option C) - the directories whose agents file could not be read,
/// keyed by the settings file's parent. `save_sessions_to_dir` reads it with the
/// directory it already receives, so a test in its own tempdir is isolated.
pub(crate) fn agents_unreadable_dirs() -> &'static std::sync::Mutex<HashSet<PathBuf>> {
    static DIRS: OnceLock<std::sync::Mutex<HashSet<PathBuf>>> = OnceLock::new();
    DIRS.get_or_init(Default::default)
}

// Dormant until the P05 carrier publication consumer is wired.
#[cfg_attr(not(test), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AgentsPublicationLockError {
    WouldBlock,
    Poisoned,
}

/// Runs a synchronous publication step while the suppression registry is locked.
/// The callback receives the unreadable flag and must not await or reacquire
/// this registry. Contention and poison never authorize publication.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn try_with_agents_layer_publication_state<R>(
    dir: &Path,
    f: impl FnOnce(bool) -> R,
) -> Result<R, AgentsPublicationLockError> {
    try_with_agents_layer_publication_state_in(agents_unreadable_dirs(), dir, f)
}

// A local registry lets poison tests exercise this same path without poisoning
// the process-wide registry used by other settings tests.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn try_with_agents_layer_publication_state_in<R>(
    registry: &std::sync::Mutex<HashSet<PathBuf>>,
    dir: &Path,
    f: impl FnOnce(bool) -> R,
) -> Result<R, AgentsPublicationLockError> {
    let dirs = registry.try_lock().map_err(|error| match error {
        std::sync::TryLockError::WouldBlock => AgentsPublicationLockError::WouldBlock,
        std::sync::TryLockError::Poisoned(_) => AgentsPublicationLockError::Poisoned,
    })?;
    Ok(f(dirs.contains(dir)))
}

/// #2716 (B3, option C) - true while the agents file beside `dir`'s settings file
/// could not be read by the last full load of it.
pub(crate) fn agents_layer_unreadable_for_dir(dir: &Path) -> bool {
    agents_unreadable_dirs()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .contains(dir)
}

/// #2716 (B3, option C) - the one setter: the field and the registry change in
/// the same statement, so they cannot diverge.
pub(crate) fn set_agents_layer_state(
    settings: &mut AppSettings,
    settings_path: &Path,
    state: AgentsLayerState,
) {
    if let Some(dir) = settings_path.parent() {
        let mut dirs = agents_unreadable_dirs()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match state {
            AgentsLayerState::Readable => dirs.remove(dir),
            AgentsLayerState::Unreadable { .. } => dirs.insert(dir.to_path_buf()),
        };
    }
    settings.agents_layer = state;
}

/// #2716 (B3, option C) - the agents layer's state from the file alone, for the
/// defaults fallback, where no merge result survives.
pub(crate) fn agents_layer_state_at(settings_path: &Path) -> AgentsLayerState {
    agents_layer_state_at_labeled(settings_path, settings_path)
}

pub(crate) fn agents_layer_state_at_labeled(
    io_path: &Path,
    settings_path: &Path,
) -> AgentsLayerState {
    let path = agents_instance_path(settings_path);
    match read_agents_instance_file_labeled(&agents_instance_path(io_path), &path) {
        Ok(_) => AgentsLayerState::Readable,
        Err(reason) => AgentsLayerState::Unreadable { path, reason },
    }
}
