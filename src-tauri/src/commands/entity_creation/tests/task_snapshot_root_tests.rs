#[test]
fn facade_root_projection_matches_original_keys_without_canonicalizing() {
    for (raw, windows) in [
        (
            r"C:\PROGRA~1\project\.ac\room-1",
            r"C:\PROGRA~1\project\.ac\room-1",
        ),
        (r"\\server\share\.ac\room-1", r"\\server\share\.ac\room-1"),
        (
            r"\\?\C:\PROGRA~1\project\.ac\room-1",
            r"C:\PROGRA~1\project\.ac\room-1",
        ),
        (
            r"\\?\UNC\server\share\.ac\room-1",
            r"\\server\share\.ac\room-1",
        ),
        (
            r"\??\UNC\server\share\.ac\room-1",
            r"\\server\share\.ac\room-1",
        ),
    ] {
        let key = crate::path_utils::path_to_string_without_windows_verbatim_prefix(
            std::path::Path::new(raw),
        );
        assert_eq!(key, if cfg!(windows) { windows } else { raw });
    }
}
