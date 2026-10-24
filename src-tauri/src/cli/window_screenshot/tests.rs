use super::*;

#[test]
fn execute_inner_rejects_noncanonical_window_ids_before_capture() {
    for raw in [
        "",
        "abc",
        "-1",
        "+1",
        " 1",
        "1 ",
        "01",
        "007",
        "1.5",
        "%FF",
        "123456789012345678901",
        "18446744073709551616",
    ] {
        let args = WindowScreenshotArgs {
            window_id: raw.to_string(),
            output: PathBuf::from("out.png"),
        };
        assert_eq!(
            execute_inner(args),
            Err(WindowScreenshotCliError::InvalidWindowId)
        );
    }
}
