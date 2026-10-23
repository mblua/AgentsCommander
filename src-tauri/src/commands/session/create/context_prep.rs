use crate::config::session_context as ctx_file;
use crate::pty::container_repos::RepoMountResolution;
use crate::session::selection::OwnedReplacementContinuation;

/// Inputs of the blocking replica-context preparation of one create.
pub(crate) struct ReplicaContextPreparation {
    pub(crate) cwd: String,
    pub(crate) target_filename: String,
    pub(crate) managed_filenames: Vec<String>,
    pub(crate) is_coordinator: bool,
    pub(crate) auto_self_clear: bool,
    pub(crate) container_repos: Option<RepoMountResolution>,
}

/// Runs on the blocking seed worker: materializes the replica context file.
pub(crate) fn materialize_replica_context(
    prep: &ReplicaContextPreparation,
) -> Result<Option<String>, String> {
    // #1065 Stage F: production records the session-spawn context
    // read/sync and self-heal under the per-project gate (acquired
    // inside the blocking worker so the 10-second poll never blocks a
    // Tokio worker); a `#[cfg(test)]` lib build stays non-emitting.
    #[cfg(not(test))]
    let activation = Some(crate::config::seed_manifest::ManifestActivationToken::production());
    #[cfg(test)]
    let activation: Option<crate::config::seed_manifest::ManifestActivationToken> = None;
    ctx_file::materialize_agent_context_file_with_filename_activated(
        &prep.cwd,
        &prep.target_filename,
        &prep.managed_filenames,
        prep.is_coordinator,
        prep.auto_self_clear,
        prep.container_repos.as_ref(),
        activation.as_ref(),
    )
}

/// Logs a failed replica-context preparation and shows the launch-error
/// dialog, unless a test fixture observes the notification instead.
pub(crate) fn notify_replica_context_failure<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    owned_replacement: Option<&OwnedReplacementContinuation>,
    e: &str,
) {
    log::error!("Replica context validation failed: {}", e);
    use tauri_plugin_dialog::DialogExt;
    // #537 facet (b) - the old copy blamed "context files missing",
    // but the real cause is usually a transient config.json lock
    // during replica identity repair. State what actually failed;
    // the interpolated error carries the precise, retry-suggesting
    // detail from format_publish_error.
    let dialog_msg = format!(
        "Cannot launch session - failed to update replica config:\n\n{}",
        e
    );
    let dialog_title = "Session Launch Error";
    #[cfg(test)]
    let notification_observed = owned_replacement.is_some_and(|owned| {
        owned.fixture_assert_context_failure_observer(e, dialog_title, &dialog_msg);
        owned.fixture_observe_context_failure(e, dialog_title, &dialog_msg)
    });
    #[cfg(not(test))]
    let notification_observed = {
        let _ = owned_replacement;
        false
    };
    if !notification_observed {
        app.dialog()
            .message(&dialog_msg)
            .title(dialog_title)
            .show(|_| {});
    }
}
