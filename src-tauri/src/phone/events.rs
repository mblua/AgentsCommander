//! Phone notifications; excludes transport, state and runtime ownership.

use crate::phone::mailbox::refresh::ProjectRefreshEventPayload;

pub(crate) trait PtyInputEvents {
    fn pty_input_status(&self, result: &crate::phone::types::PtyInputResult) -> bool;
}

pub(crate) trait MailboxDeliveryEvents {
    fn message_delivered(&self, payload: &serde_json::Value) -> bool;
}

/// Notifications produced after processing mailbox requests.
pub(crate) trait MailboxRequestEvents {
    fn project_refresh(&self, payload: &ProjectRefreshEventPayload) -> bool;
    fn coding_agent_settings_updated(&self, payload: &serde_json::Value) -> bool;
}
