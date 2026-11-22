//! Re-export seam for the web browser mailbox action names and helpers.
//! Production code uses the navigate, reload and status action constants; the rest is test-only.
//! The action handling itself lives in `crate::phone::mailbox::browser`.

#[cfg(test)]
pub(crate) use crate::phone::mailbox::browser::is_web_browser_action;
#[cfg(test)]
pub(crate) use crate::phone::mailbox::browser::{action_requires_orchestrator, response_json};
pub(crate) use crate::phone::mailbox::browser::{
    WEB_BROWSER_NAVIGATE_ACTION, WEB_BROWSER_RELOAD_ACTION, WEB_BROWSER_STATUS_ACTION,
};
