use super::regular::same_object;
use super::retain_immutable_directory;
use super::verify_directory;
use super::RetainedDirectory;
use super::VerifiedPathIdentity;
use std::fs::File;
use std::io::Write;
use std::path::Component;
use std::path::Path;
use std::path::PathBuf;

/// A caller-owned output file whose leaf handle and parent directory remain
/// retained until the write and identity checks complete.
pub struct VerifiedNewFile {
    path: PathBuf,
    file: File,
    parent: RetainedDirectory,
}

impl VerifiedNewFile {
    /// Failure intentionally leaves the newly created caller-owned file in
    /// place. The CLI must never unlink a path after ownership becomes unclear.
    pub fn write_all_and_sync(self, bytes: &[u8]) -> Result<(), String> {
        self.write_all_and_sync_inner(bytes, || {}, || {})
    }

    pub(super) fn write_all_and_sync_inner(
        mut self,
        bytes: &[u8],
        after_write: impl FnOnce(),
        after_sync: impl FnOnce(),
    ) -> Result<(), String> {
        self.parent
            .verify_current()
            .map_err(|_| "output_failed".to_string())?;
        self.file
            .write_all(bytes)
            .and_then(|_| self.file.flush())
            .map_err(|_| "output_failed".to_string())?;
        after_write();
        self.parent
            .verify_current()
            .map_err(|_| "output_failed".to_string())?;
        self.file
            .sync_all()
            .map_err(|_| "output_failed".to_string())?;
        after_sync();
        self.parent
            .verify_current()
            .map_err(|_| "output_failed".to_string())?;
        let opened = self
            .parent
            .verify_opened_regular_file(&self.path, &self.file, false)
            .map_err(|_| "output_failed".to_string())?;
        self.parent
            .verify_current()
            .map_err(|_| "output_failed".to_string())?;
        if opened.metadata.links != 1 || opened.metadata.len != bytes.len() as u64 {
            return Err("output_failed".to_string());
        }
        Ok(())
    }
}

pub fn create_terminal_snapshot_output(path: &Path) -> Result<VerifiedNewFile, String> {
    create_terminal_snapshot_output_inner(path, || {})
}

pub(super) fn create_terminal_snapshot_output_inner(
    path: &Path,
    before_retention: impl FnOnce(),
) -> Result<VerifiedNewFile, String> {
    create_terminal_snapshot_output_with_hooks(path, before_retention, || {})
}

pub(super) fn create_terminal_snapshot_output_with_hooks(
    path: &Path,
    before_retention: impl FnOnce(),
    before_open: impl FnOnce(),
) -> Result<VerifiedNewFile, String> {
    let validated_parent = validate_terminal_snapshot_output_path_inner(path, || {})?;
    let parent_path = path.parent().ok_or_else(|| "unsafe_path".to_string())?;
    before_retention();
    let parent =
        retain_immutable_directory(parent_path).map_err(|_| "output_failed".to_string())?;
    if !same_object(&validated_parent, parent.identity()) {
        return Err("output_failed".to_string());
    }
    before_open();
    let file = parent
        .create_new_output_file(path)
        .map_err(|_| "output_failed".to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))
            .map_err(|_| "output_failed".to_string())?;
        if file
            .metadata()
            .map_err(|_| "output_failed".to_string())?
            .permissions()
            .mode()
            & 0o777
            != 0o600
        {
            return Err("output_failed".to_string());
        }
    }
    parent
        .verify_opened_regular_file(path, &file, true)
        .map_err(|_| "output_failed".to_string())?;
    parent
        .verify_current()
        .map_err(|_| "output_failed".to_string())?;
    Ok(VerifiedNewFile {
        path: path.to_path_buf(),
        file,
        parent,
    })
}

pub fn validate_terminal_snapshot_output_path(path: &Path) -> Result<(), String> {
    validate_terminal_snapshot_output_path_inner(path, || {}).map(|_| ())
}

pub(super) fn validate_terminal_snapshot_output_path_inner(
    path: &Path,
    before_filesystem: impl FnOnce(),
) -> Result<VerifiedPathIdentity, String> {
    if !path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
    {
        return Err("unsafe_path".to_string());
    }
    let extension = path.extension().ok_or_else(|| "unsafe_path".to_string())?;
    #[cfg(unix)]
    let extension_is_png = {
        use std::os::unix::ffi::OsStrExt;
        extension
            .as_bytes()
            .iter()
            .copied()
            .map(|byte| byte.to_ascii_lowercase())
            .eq(b"png".iter().copied())
    };
    #[cfg(not(unix))]
    let extension_is_png = extension
        .to_str()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("png"));
    if !extension_is_png {
        return Err("unsafe_path".to_string());
    }
    #[cfg(windows)]
    validate_windows_snapshot_output_path(path)?;
    before_filesystem();
    let parent = path.parent().ok_or_else(|| "unsafe_path".to_string())?;
    let parent_identity = verify_directory(parent)?;
    match std::fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(parent_identity),
        _ => Err("unsafe_path".to_string()),
    }
}

#[cfg(windows)]
pub(super) fn validate_windows_snapshot_output_path(path: &Path) -> Result<(), String> {
    use std::path::Prefix;

    if !path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
    {
        return Err("unsafe_path".to_string());
    }
    let raw = path
        .as_os_str()
        .to_str()
        .ok_or_else(|| "unsafe_path".to_string())?;
    if raw
        .split(['\\', '/'])
        .any(|component| matches!(component, "." | ".."))
    {
        return Err("unsafe_path".to_string());
    }
    let mut components = path.components();
    let allowed_drive_colon = match components.next() {
        Some(Component::Prefix(prefix)) => match prefix.kind() {
            Prefix::Disk(_) => Some(1),
            Prefix::VerbatimDisk(_) => {
                raw.char_indices().find_map(
                    |(index, value)| {
                        if value == ':' {
                            Some(index)
                        } else {
                            None
                        }
                    },
                )
            }
            Prefix::UNC(_, _) | Prefix::VerbatimUNC(_, _) => None,
            _ => return Err("unsafe_path".to_string()),
        },
        _ => return Err("unsafe_path".to_string()),
    };
    if raw
        .char_indices()
        .any(|(index, value)| value == ':' && Some(index) != allowed_drive_colon)
    {
        return Err("unsafe_path".to_string());
    }
    for component in path.components() {
        let Component::Normal(value) = component else {
            continue;
        };
        let Some(value) = value.to_str() else {
            return Err("unsafe_path".to_string());
        };
        if value.ends_with([' ', '.']) {
            return Err("unsafe_path".to_string());
        }
        let stem = value
            .split('.')
            .next()
            .unwrap_or(value)
            .trim_end_matches([' ', '.']);
        let reserved = matches!(
            stem.to_ascii_uppercase().as_str(),
            "CON"
                | "PRN"
                | "AUX"
                | "NUL"
                | "CONIN$"
                | "CONOUT$"
                | "COM1"
                | "COM2"
                | "COM3"
                | "COM4"
                | "COM5"
                | "COM6"
                | "COM7"
                | "COM8"
                | "COM9"
                | "COM¹"
                | "COM²"
                | "COM³"
                | "LPT1"
                | "LPT2"
                | "LPT3"
                | "LPT4"
                | "LPT5"
                | "LPT6"
                | "LPT7"
                | "LPT8"
                | "LPT9"
                | "LPT¹"
                | "LPT²"
                | "LPT³"
        );
        if reserved
            || value.chars().any(|character| {
                character <= '\u{1f}' || matches!(character, '<' | '>' | '"' | '|' | '?' | '*')
            })
        {
            return Err("unsafe_path".to_string());
        }
    }
    Ok(())
}
