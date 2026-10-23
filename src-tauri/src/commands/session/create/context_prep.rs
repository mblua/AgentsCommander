use crate::session::selection::OwnedReplacementContinuation;

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
