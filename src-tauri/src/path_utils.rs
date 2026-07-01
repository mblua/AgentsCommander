use std::path::{Path, PathBuf};

fn strip_windows_verbatim_prefix_str(path: &str) -> Option<String> {
    if let Some(rest) = path.strip_prefix(r"\\?\UNC\") {
        Some(format!(r"\\{}", rest))
    } else if let Some(rest) = path.strip_prefix(r"\\?\") {
        Some(rest.to_string())
    } else if let Some(rest) = path.strip_prefix(r"\??\UNC\") {
        Some(format!(r"\\{}", rest))
    } else {
        path.strip_prefix(r"\??\").map(str::to_string)
    }
}

pub(crate) fn normalize_windows_verbatim_path(path: &str) -> String {
    if cfg!(windows) {
        strip_windows_verbatim_prefix_str(path).unwrap_or_else(|| path.to_string())
    } else {
        path.to_string()
    }
}

pub(crate) fn normalize_windows_verbatim_path_buf(path: &Path) -> PathBuf {
    if cfg!(windows) {
        let raw = path.as_os_str().to_string_lossy();
        PathBuf::from(normalize_windows_verbatim_path(raw.as_ref()))
    } else {
        path.to_path_buf()
    }
}

pub(crate) fn path_to_string_without_windows_verbatim_prefix(path: &Path) -> String {
    let raw = path.to_string_lossy();
    normalize_windows_verbatim_path(raw.as_ref())
}

#[cfg(test)]
mod tests {
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
}
