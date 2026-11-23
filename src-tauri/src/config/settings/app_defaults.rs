//! The `Default` implementation of `AppSettings`.
//! Builds defaults through serde from the per-field `#[serde(default...)]` attributes, adding
//! only required fields, the platform default shell and the context-history color window.
//! Field default functions live in `field`; loading and saving live in `load` and `save`.

#![cfg_attr(not(test), warn(clippy::unwrap_used, clippy::expect_used))]

use super::context_history::DEFAULT_SESSION_WINDOWS;
use super::AppSettings;

fn default_shell_and_args() -> (String, Vec<String>) {
    if cfg!(target_os = "windows") {
        ("powershell.exe".to_string(), vec!["-NoLogo".to_string()])
    } else {
        ("/bin/bash".to_string(), vec![])
    }
}

impl Default for AppSettings {
    /// Every field default lives on its `#[serde(default...)]` attribute.
    /// Only required fields and fields whose `Default` differs from the
    /// serde default are listed here.
    #[allow(
        clippy::unwrap_used,
        clippy::expect_used,
        reason = "the default settings literal is a fixed valid JSON value for AppSettings"
    )]
    fn default() -> Self {
        let (shell, args) = default_shell_and_args();
        let mut settings: Self = serde_json::from_value(serde_json::json!({
            "defaultShell": shell,
            "defaultShellArgs": args,
            "agents": [],
            "restoreCoordinatorWakeState": true,
            "restartResumeWakeWorkingAgents": true,
        }))
        .expect("#4766: serde field defaults must build AppSettings");
        settings.context_history_color_sessions = DEFAULT_SESSION_WINDOWS.last().copied();
        settings
    }
}
