use super::pty_observation::PtyAuthorityState;

/// Borrowed response admission observations; state ownership stays with the tracker.
pub(crate) trait ResponseAdmissionState: PtyAuthorityState {
    fn response_tracker(&self) -> Option<&crate::pty::manager::ResponseCloseState>;
}
