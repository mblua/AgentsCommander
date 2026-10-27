pub(crate) fn test_window_placement() -> Option<String> {
    std::env::var("AC_TEST_WINDOW_PLACEMENT").ok()
}

pub(crate) fn machine_output_string() -> Result<String, std::env::VarError> {
    std::env::var("AC_MACHINE_OUTPUT")
}

pub(crate) fn machine_output_os() -> Option<std::ffi::OsString> {
    std::env::var_os("AC_MACHINE_OUTPUT")
}

pub(crate) fn harness_root() -> Result<String, std::env::VarError> {
    std::env::var("AGENTSCOMMANDER_ROOT")
}

pub(crate) fn role_experiment_fake_executor() -> Result<String, std::env::VarError> {
    std::env::var("AGENTSCOMMANDER_ROLE_EXPERIMENT_TEST_FAKE_EXECUTOR")
}

pub(crate) fn terminal_snapshot_stage_file() -> Result<String, std::env::VarError> {
    std::env::var("AC_TERMINAL_SNAPSHOT_STAGE_FILE")
}

pub(crate) fn ui_automation_enable(key: &str) -> Result<String, std::env::VarError> {
    std::env::var(key)
}

pub(crate) fn container_credential_override(key: &str) -> Result<String, std::env::VarError> {
    std::env::var(key)
}

pub(crate) fn container_image() -> Result<String, std::env::VarError> {
    std::env::var("AGENTSCOMMANDER_CONTAINER_IMAGE")
}

pub(crate) fn rust_log() -> Result<String, std::env::VarError> {
    std::env::var("RUST_LOG")
}

#[cfg(target_os = "linux")]
pub(crate) fn xdg_runtime_dir() -> Option<std::ffi::OsString> {
    std::env::var_os("XDG_RUNTIME_DIR")
}

#[cfg(target_os = "linux")]
pub(crate) fn xdg_session_type() -> Result<String, std::env::VarError> {
    std::env::var("XDG_SESSION_TYPE")
}

#[cfg(target_os = "linux")]
pub(crate) fn wayland_display() -> Result<String, std::env::VarError> {
    std::env::var("WAYLAND_DISPLAY")
}

#[cfg(target_os = "linux")]
pub(crate) fn wayland_socket() -> Result<String, std::env::VarError> {
    std::env::var("WAYLAND_SOCKET")
}

#[cfg(windows)]
pub(crate) fn inherited_path() -> Option<std::ffi::OsString> {
    std::env::var_os("PATH")
}

pub(crate) fn codex_home() -> Result<String, std::env::VarError> {
    std::env::var("CODEX_HOME")
}

pub(crate) fn spawn_diagnostic_value(key: &str) -> Result<String, std::env::VarError> {
    std::env::var(key)
}

/// Reads a named environment value for resume-wrapper reference expansion.
/// Performs a fresh lookup and preserves absent and non-Unicode errors.
pub(crate) fn resume_expansion_value(name: &str) -> Result<String, std::env::VarError> {
    std::env::var(name)
}

#[cfg(test)]
mod tests;
