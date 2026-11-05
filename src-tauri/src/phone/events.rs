//! Phone notifications; excludes transport, state and runtime ownership.

pub(crate) trait PtyInputEvents {
    fn pty_input_status(&self, result: &crate::phone::types::PtyInputResult) -> bool;
}

pub(crate) trait MailboxDeliveryEvents {
    fn message_delivered(&self, payload: &serde_json::Value) -> bool;
}
