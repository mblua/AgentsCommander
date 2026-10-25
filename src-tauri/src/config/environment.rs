pub(crate) fn test_window_placement() -> Option<String> {
    std::env::var("AC_TEST_WINDOW_PLACEMENT").ok()
}

pub(crate) fn machine_output_string() -> Result<String, std::env::VarError> {
    std::env::var("AC_MACHINE_OUTPUT")
}

pub(crate) fn machine_output_os() -> Option<std::ffi::OsString> {
    std::env::var_os("AC_MACHINE_OUTPUT")
}

#[cfg(test)]
mod tests;
