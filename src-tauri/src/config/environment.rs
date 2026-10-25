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

#[cfg(test)]
mod tests;
