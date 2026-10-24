pub(crate) fn test_window_placement() -> Option<String> {
    std::env::var("AC_TEST_WINDOW_PLACEMENT").ok()
}
