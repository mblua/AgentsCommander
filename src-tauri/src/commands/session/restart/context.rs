use super::super::lifecycle::LifecycleEntryAuthority;
use crate::session::selection::SelectionTransaction;

pub(crate) enum LifecycleExecutionContext<'a> {
    ExternalEntry(&'a LifecycleEntryAuthority),
    RestoreEntry {
        authority: &'a LifecycleEntryAuthority,
        root_directory: Option<&'a str>,
    },
}
impl LifecycleExecutionContext<'_> {
    pub(crate) fn authority(&self) -> Option<&LifecycleEntryAuthority> {
        match self {
            Self::ExternalEntry(authority) => Some(authority),
            Self::RestoreEntry { authority, .. } => Some(authority),
        }
    }
}

pub(crate) fn ensure_owned_root_directory(
    effect: crate::session::selection::LifecycleEffectGuard,
) -> Result<String, String> {
    let result = crate::config::root_agent::ensure_root_agent_dir();
    effect.complete(result.clone().map(Some));
    result
}

pub(crate) async fn ensure_lifecycle_root_directory<R: tauri::Runtime>(
    transaction: &SelectionTransaction<R>,
    context: &LifecycleExecutionContext<'_>,
) -> Result<String, String> {
    if let LifecycleExecutionContext::RestoreEntry { root_directory, .. } = context {
        if !transaction.has_active_restore_capability() {
            return Err(
                crate::session::selection::SelectionCoordinatorError::Unavailable.to_string(),
            );
        }
        return root_directory.map(str::to_owned).ok_or_else(|| {
            crate::session::selection::SelectionCoordinatorError::Unavailable.to_string()
        });
    }
    if !transaction.has_accepted_lifecycle_core() {
        return std::future::pending().await;
    }
    if let Some(outcome) = transaction.lifecycle_root_directory_outcome() {
        return outcome;
    }
    let Some(effect) = transaction
        .start_lifecycle_effect(crate::session::selection::LifecycleEffectSlot::RootDirectory)
    else {
        return std::future::pending().await;
    };
    ensure_owned_root_directory(effect)
}
