use super::pty_observation::PtyAuthorityState;

/// Borrowed response admission observations; state ownership stays with the tracker.
pub(crate) trait ResponseAdmissionState: PtyAuthorityState {
    fn response_tracker(&self) -> Option<&crate::pty::manager::ResponseCloseState>;
}

/// Borrowed response completion observations; selection retains state ownership.
pub(crate) trait ResponseCompletionState: PtyAuthorityState {
    fn response_selection(&self) -> Option<&crate::session::selection::SelectionCoordinator>;
}
