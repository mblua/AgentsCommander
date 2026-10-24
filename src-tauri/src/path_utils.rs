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
mod tests;
