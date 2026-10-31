#![cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]

use std::path::{Path, PathBuf};

use crate::path_identity::is_link_or_reparse;

/// Upper bound on the `-<n>.png` collision suffix, mirroring
/// `phone::messaging::MAX_COLLISION_SUFFIX`.
const MAX_COLLISION_SUFFIX: u32 = 9999;

/// Lazily create only the direct child of the already validated canonical root.
pub(crate) fn ensure_screenshots_directory(root: &Path, child: &Path) -> Result<PathBuf, String> {
    match std::fs::create_dir(child) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(format!("Cannot create screenshots directory: {e}")),
    }
    let meta = std::fs::symlink_metadata(child)
        .map_err(|e| format!("Cannot read screenshots directory: {e}"))?;
    if !meta.is_dir() || is_link_or_reparse(&meta) {
        return Err("Screenshots path is not a real directory".to_string());
    }
    let canonical = std::fs::canonicalize(child)
        .map_err(|e| format!("Cannot canonicalize screenshots directory: {e}"))?;
    if canonical != child || canonical.parent() != Some(root) {
        return Err("Screenshots directory is not directly inside the replica root".to_string());
    }
    Ok(canonical)
}

/// Collision loop mirroring `phone::messaging::create_message_file`: never
/// overwrite an existing file; retry with `-<n>.png` up to the bound.
pub(crate) fn create_png_file(
    dir: &Path,
    base_filename: &str,
) -> Result<(PathBuf, impl std::io::Write + std::io::Seek), String> {
    let stem = base_filename
        .strip_suffix(".png")
        .ok_or_else(|| format!("filename '{base_filename}' must end in .png"))?;
    for n in 0..=MAX_COLLISION_SUFFIX {
        let filename = if n == 0 {
            base_filename.to_string()
        } else {
            format!("{stem}-{n}.png")
        };
        let path = dir.join(&filename);
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(file) => return Ok((path, file)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(format!("failed to create {}: {e}", path.display())),
        }
    }
    Err(format!(
        "exhausted {MAX_COLLISION_SUFFIX} filename collisions for '{base_filename}'"
    ))
}

/// Recheck the real child and saved path after writing.
pub(crate) fn validate_saved_screenshot(
    root: &Path,
    child: &Path,
    path: &Path,
) -> Result<PathBuf, String> {
    let meta = std::fs::symlink_metadata(child)
        .map_err(|e| format!("Cannot read screenshots directory: {e}"))?;
    if !meta.is_dir() || is_link_or_reparse(&meta) {
        return Err("Screenshots path is not a real directory".to_string());
    }
    let canonical_child = std::fs::canonicalize(child)
        .map_err(|e| format!("Cannot canonicalize screenshots directory: {e}"))?;
    if canonical_child != child || canonical_child.parent() != Some(root) {
        return Err("Screenshots directory changed during save".to_string());
    }
    let canonical =
        std::fs::canonicalize(path).map_err(|e| format!("canonicalize saved file failed: {e}"))?;
    let parent_ok = canonical
        .parent()
        .map(|p| p == canonical_child)
        .unwrap_or(false);
    if !parent_ok {
        return Err("saved file escaped the screenshots directory".to_string());
    }
    Ok(canonical)
}

/// Remove only the supplied partial screenshot.
pub(crate) fn remove_partial_screenshot(path: &Path) -> std::io::Result<()> {
    std::fs::remove_file(path)
}
