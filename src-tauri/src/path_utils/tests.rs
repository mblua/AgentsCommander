use super::*;

#[test]
fn strip_windows_verbatim_prefix_str_converts_drive_path() {
    assert_eq!(
        strip_windows_verbatim_prefix_str(r"\\?\C:\tmp\repo").as_deref(),
        Some(r"C:\tmp\repo")
    );
}

#[test]
fn strip_windows_verbatim_prefix_str_converts_unc_path() {
    assert_eq!(
        strip_windows_verbatim_prefix_str(r"\\?\UNC\server\share\repo").as_deref(),
        Some(r"\\server\share\repo")
    );
}

#[test]
fn strip_windows_verbatim_prefix_str_converts_nt_drive_and_unc_paths() {
    assert_eq!(
        strip_windows_verbatim_prefix_str(r"\??\C:\tmp\repo").as_deref(),
        Some(r"C:\tmp\repo")
    );
    assert_eq!(
        strip_windows_verbatim_prefix_str(r"\??\UNC\server\share\repo").as_deref(),
        Some(r"\\server\share\repo")
    );
}

#[test]
fn strip_windows_verbatim_prefix_str_leaves_ordinary_paths() {
    assert_eq!(strip_windows_verbatim_prefix_str(r"C:\tmp\repo"), None);
    assert_eq!(
        strip_windows_verbatim_prefix_str(r"\\server\share\repo"),
        None
    );
    assert_eq!(strip_windows_verbatim_prefix_str("/tmp/repo"), None);
}

#[test]
#[cfg(windows)]
fn normalize_windows_verbatim_path_strips_prefix_on_windows() {
    assert_eq!(
        normalize_windows_verbatim_path(r"\\?\C:\tmp\repo"),
        r"C:\tmp\repo"
    );
}

#[test]
#[cfg(not(windows))]
fn normalize_windows_verbatim_path_preserves_prefix_off_windows() {
    assert_eq!(
        normalize_windows_verbatim_path(r"\\?\C:\tmp\repo"),
        r"\\?\C:\tmp\repo"
    );
}
