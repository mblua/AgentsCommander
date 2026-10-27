use std::io::Write;

pub(crate) struct SnapshotOutputFile {
    path: std::path::PathBuf,
    file: std::fs::File,
    parent: SnapshotDirectoryProof,
    file_identity: SnapshotFileIdentity,
}

struct SnapshotDirectoryProof {
    path: std::path::PathBuf,
    file: std::fs::File,
    identity: SnapshotFileObjectId,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct SnapshotFileObjectId {
    volume: u64,
    file: u64,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct SnapshotFileIdentity {
    object_id: SnapshotFileObjectId,
    links: u64,
}

impl SnapshotOutputFile {
    pub(crate) fn write_and_verify(
        self,
        bytes: &[u8],
    ) -> Result<(), terminal_snapshot_renderer::TerminalSnapshotReasonCode> {
        self.write_and_verify_inner(bytes, || {}, || {})
    }

    fn write_and_verify_inner(
        mut self,
        bytes: &[u8],
        after_write: impl FnOnce(),
        after_sync: impl FnOnce(),
    ) -> Result<(), terminal_snapshot_renderer::TerminalSnapshotReasonCode> {
        use terminal_snapshot_renderer::TerminalSnapshotReasonCode as C;

        let parent_path = self.path.parent().ok_or(C::OutputFailed)?;
        let current_parent = snapshot_directory_identity(parent_path, C::OutputFailed)?;
        if current_parent.identity != self.parent.identity {
            return Err(C::OutputFailed);
        }
        self.file
            .write_all(bytes)
            .and_then(|_| self.file.flush())
            .map_err(|_| C::OutputFailed)?;
        after_write();
        let current_parent = snapshot_directory_identity(parent_path, C::OutputFailed)?;
        if current_parent.identity != self.parent.identity {
            return Err(C::OutputFailed);
        }
        self.file.sync_all().map_err(|_| C::OutputFailed)?;
        after_sync();
        let current_parent = snapshot_directory_identity(parent_path, C::OutputFailed)?;
        let retained_parent = snapshot_handle_identity(&self.parent.file, true)
            .map_err(|_| C::OutputFailed)?
            .object_id;
        let current_handle = snapshot_file_identity(&self.file).map_err(|_| C::OutputFailed)?;
        let current_path =
            open_snapshot_leaf_in_parent(&self.parent, &self.path).map_err(|_| C::OutputFailed)?;
        let current_path_identity =
            snapshot_file_identity(&current_path).map_err(|_| C::OutputFailed)?;
        let length = self.file.metadata().map_err(|_| C::OutputFailed)?.len();
        if retained_parent != self.parent.identity
            || current_parent.identity != self.parent.identity
            || current_handle != self.file_identity
            || current_path_identity != self.file_identity
            || current_handle.links != 1
            || length != bytes.len() as u64
        {
            return Err(C::OutputFailed);
        }
        Ok(())
    }
}

pub(crate) fn validate_snapshot_output_path(
    path: &std::path::Path,
) -> Result<(), terminal_snapshot_renderer::TerminalSnapshotReasonCode> {
    validate_snapshot_output_path_inner(path, || {}).map(|_| ())
}

fn validate_snapshot_output_path_inner(
    path: &std::path::Path,
    before_filesystem: impl FnOnce(),
) -> Result<SnapshotFileObjectId, terminal_snapshot_renderer::TerminalSnapshotReasonCode> {
    use terminal_snapshot_renderer::TerminalSnapshotReasonCode as C;

    if !path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                std::path::Component::ParentDir | std::path::Component::CurDir
            )
        })
    {
        return Err(C::UnsafePath);
    }
    let extension = path.extension().ok_or(C::UnsafePath)?;
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
    #[cfg(windows)]
    let extension_is_png = extension
        .to_str()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("png"));
    if !extension_is_png {
        return Err(C::UnsafePath);
    }
    #[cfg(windows)]
    validate_windows_snapshot_path(path)?;
    before_filesystem();
    let parent = path.parent().ok_or(C::UnsafePath)?;
    verify_snapshot_directory_chain(parent)?;
    let parent_proof = snapshot_directory_identity(parent, C::UnsafePath)?;
    let parent_identity = parent_proof.identity;
    drop(parent_proof);
    match std::fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(parent_identity),
        _ => Err(C::UnsafePath),
    }
}

pub(crate) fn create_snapshot_output(
    path: &std::path::Path,
) -> Result<SnapshotOutputFile, terminal_snapshot_renderer::TerminalSnapshotReasonCode> {
    create_snapshot_output_inner(path, || {})
}

fn create_snapshot_output_inner(
    path: &std::path::Path,
    before_retention: impl FnOnce(),
) -> Result<SnapshotOutputFile, terminal_snapshot_renderer::TerminalSnapshotReasonCode> {
    create_snapshot_output_with_hooks(path, before_retention, || {})
}

fn create_snapshot_output_with_hooks(
    path: &std::path::Path,
    before_retention: impl FnOnce(),
    before_open: impl FnOnce(),
) -> Result<SnapshotOutputFile, terminal_snapshot_renderer::TerminalSnapshotReasonCode> {
    use terminal_snapshot_renderer::TerminalSnapshotReasonCode as C;

    let validated_parent_identity = validate_snapshot_output_path_inner(path, || {})?;
    let parent = path.parent().ok_or(C::UnsafePath)?;
    before_retention();
    let parent_proof = snapshot_directory_identity(parent, C::OutputFailed)?;
    if parent_proof.identity != validated_parent_identity {
        return Err(C::OutputFailed);
    }
    before_open();
    let current_parent = snapshot_directory_identity(parent, C::OutputFailed)?;
    if current_parent.identity != parent_proof.identity {
        return Err(C::OutputFailed);
    }
    let file = create_snapshot_leaf_in_parent(&parent_proof, path).map_err(|_| C::OutputFailed)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))
            .map_err(|_| C::OutputFailed)?;
        if file
            .metadata()
            .map_err(|_| C::OutputFailed)?
            .permissions()
            .mode()
            & 0o777
            != 0o600
        {
            return Err(C::OutputFailed);
        }
    }
    let file_identity = snapshot_file_identity(&file).map_err(|_| C::OutputFailed)?;
    let path_file =
        open_snapshot_leaf_in_parent(&parent_proof, path).map_err(|_| C::OutputFailed)?;
    let path_identity = snapshot_file_identity(&path_file).map_err(|_| C::OutputFailed)?;
    let current_parent = snapshot_directory_identity(parent, C::OutputFailed)?;
    if file_identity != path_identity
        || file_identity.links != 1
        || current_parent.identity != parent_proof.identity
    {
        return Err(C::OutputFailed);
    }
    Ok(SnapshotOutputFile {
        path: path.to_path_buf(),
        file,
        parent: parent_proof,
        file_identity,
    })
}

fn verify_snapshot_directory_chain(
    directory: &std::path::Path,
) -> Result<(), terminal_snapshot_renderer::TerminalSnapshotReasonCode> {
    use terminal_snapshot_renderer::TerminalSnapshotReasonCode as C;

    let mut current = std::path::PathBuf::new();
    for component in directory.components() {
        current.push(component.as_os_str());
        if matches!(
            component,
            std::path::Component::Prefix(_) | std::path::Component::RootDir
        ) {
            continue;
        }
        let metadata = std::fs::symlink_metadata(&current).map_err(|_| C::UnsafePath)?;
        if !metadata.is_dir()
            || metadata.file_type().is_symlink()
            || snapshot_metadata_reparse(&metadata)
        {
            return Err(C::UnsafePath);
        }
    }
    snapshot_directory_identity(directory, C::UnsafePath).map(|_| ())
}

fn snapshot_directory_identity(
    path: &std::path::Path,
    reason: terminal_snapshot_renderer::TerminalSnapshotReasonCode,
) -> Result<SnapshotDirectoryProof, terminal_snapshot_renderer::TerminalSnapshotReasonCode> {
    let metadata = std::fs::symlink_metadata(path).map_err(|_| reason)?;
    if !metadata.is_dir()
        || metadata.file_type().is_symlink()
        || snapshot_metadata_reparse(&metadata)
    {
        return Err(reason);
    }
    let file = open_snapshot_directory(path).map_err(|_| reason)?;
    let identity = snapshot_handle_identity(&file, true)
        .map_err(|_| reason)?
        .object_id;
    Ok(SnapshotDirectoryProof {
        path: path.to_path_buf(),
        file,
        identity,
    })
}

fn open_snapshot_directory(path: &std::path::Path) -> std::io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ,
        };
        options
            .share_mode(FILE_SHARE_READ)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT);
    }
    options.open(path)
}

#[cfg(not(unix))]
fn open_snapshot_leaf(path: &std::path::Path) -> std::io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT;
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    options.open(path)
}

struct SnapshotChildName<'a>(&'a std::ffi::OsStr);

impl SnapshotChildName<'_> {
    fn as_os_str(&self) -> &std::ffi::OsStr {
        self.0
    }
}

fn snapshot_child_name<'a>(
    parent: &SnapshotDirectoryProof,
    path: &'a std::path::Path,
) -> std::io::Result<SnapshotChildName<'a>> {
    use std::path::Component;

    if path.parent() != Some(parent.path.as_path()) {
        return Err(std::io::Error::from(std::io::ErrorKind::InvalidInput));
    }
    let relative = path
        .strip_prefix(&parent.path)
        .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
    let mut components = relative.components();
    let name = match (components.next(), components.next()) {
        (Some(Component::Normal(name)), None) if !name.is_empty() => name,
        _ => return Err(std::io::Error::from(std::io::ErrorKind::InvalidInput)),
    };
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        if name.as_bytes().contains(&0) {
            return Err(std::io::Error::from(std::io::ErrorKind::InvalidInput));
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        if name.encode_wide().any(|unit| unit == 0)
            || validate_windows_snapshot_path(&parent.path.join(name)).is_err()
        {
            return Err(std::io::Error::from(std::io::ErrorKind::InvalidInput));
        }
    }
    Ok(SnapshotChildName(name))
}

#[cfg(all(unix, not(any(target_os = "linux", target_os = "macos"))))]
fn unsupported_snapshot_output_error() -> std::io::Error {
    std::io::Error::from(std::io::ErrorKind::Unsupported)
}

fn create_snapshot_leaf_in_parent(
    parent: &SnapshotDirectoryProof,
    path: &std::path::Path,
) -> std::io::Result<std::fs::File> {
    let name = snapshot_child_name(parent, path)?;
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        open_snapshot_leaf_at(
            parent,
            &name,
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            0o600,
        )
    }
    #[cfg(all(unix, not(any(target_os = "linux", target_os = "macos"))))]
    {
        let _ = (parent, name);
        Err(unsupported_snapshot_output_error())
    }
    #[cfg(not(unix))]
    {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            use windows_sys::Win32::Storage::FileSystem::{
                FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ,
            };
            options
                .share_mode(FILE_SHARE_READ)
                .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
        }
        options.open(parent.path.join(name.as_os_str()))
    }
}

fn open_snapshot_leaf_in_parent(
    parent: &SnapshotDirectoryProof,
    path: &std::path::Path,
) -> std::io::Result<std::fs::File> {
    let name = snapshot_child_name(parent, path)?;
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        open_snapshot_leaf_at(
            parent,
            &name,
            libc::O_RDONLY | libc::O_NONBLOCK | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            0,
        )
    }
    #[cfg(all(unix, not(any(target_os = "linux", target_os = "macos"))))]
    {
        let _ = (parent, name);
        Err(unsupported_snapshot_output_error())
    }
    #[cfg(not(unix))]
    {
        open_snapshot_leaf(&parent.path.join(name.as_os_str()))
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn open_snapshot_leaf_at(
    parent: &SnapshotDirectoryProof,
    name: &SnapshotChildName<'_>,
    flags: i32,
    mode: u32,
) -> std::io::Result<std::fs::File> {
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::ffi::OsStrExt;

    let name = std::ffi::CString::new(name.as_os_str().as_bytes())
        .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
    let descriptor = unsafe {
        // `openat` is variadic, so the mode must arrive default-promoted to at least
        // `c_int`/`c_uint`. `libc::mode_t` is `u32` on Linux but `u16` on macOS, where passing
        // it unpromoted is a hard compile error (E0617). Casting straight to `c_uint` is
        // identity on Linux and avoids narrowing to `u16` and widening back on macOS.
        libc::openat(
            parent.file.as_raw_fd(),
            name.as_ptr(),
            flags,
            mode as libc::c_uint,
        )
    };
    if descriptor < 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(unsafe { std::fs::File::from_raw_fd(descriptor) })
    }
}

fn snapshot_file_identity(file: &std::fs::File) -> std::io::Result<SnapshotFileIdentity> {
    snapshot_handle_identity(file, false)
}

#[cfg(unix)]
fn snapshot_handle_identity(
    file: &std::fs::File,
    require_directory: bool,
) -> std::io::Result<SnapshotFileIdentity> {
    use std::os::unix::fs::MetadataExt;
    let metadata = file.metadata()?;
    if metadata.is_dir() != require_directory || metadata.is_file() == require_directory {
        return Err(std::io::Error::other("unexpected filesystem object"));
    }
    Ok(SnapshotFileIdentity {
        object_id: SnapshotFileObjectId {
            volume: metadata.dev(),
            file: metadata.ino(),
        },
        links: metadata.nlink(),
    })
}

#[cfg(windows)]
fn snapshot_handle_identity(
    file: &std::fs::File,
    require_directory: bool,
) -> std::io::Result<SnapshotFileIdentity> {
    use std::mem::MaybeUninit;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION,
    };

    let metadata = file.metadata()?;
    if metadata.is_dir() != require_directory || metadata.is_file() == require_directory {
        return Err(std::io::Error::other("unexpected filesystem object"));
    }
    let mut information = MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::zeroed();
    let ok =
        unsafe { GetFileInformationByHandle(file.as_raw_handle() as _, information.as_mut_ptr()) };
    if ok == 0 {
        return Err(std::io::Error::last_os_error());
    }
    let information = unsafe { information.assume_init() };
    Ok(SnapshotFileIdentity {
        object_id: SnapshotFileObjectId {
            volume: u64::from(information.dwVolumeSerialNumber),
            file: (u64::from(information.nFileIndexHigh) << 32)
                | u64::from(information.nFileIndexLow),
        },
        links: u64::from(information.nNumberOfLinks),
    })
}

#[cfg(unix)]
fn snapshot_metadata_reparse(_metadata: &std::fs::Metadata) -> bool {
    false
}

#[cfg(windows)]
fn snapshot_metadata_reparse(metadata: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;
    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(windows)]
fn validate_windows_snapshot_path(
    path: &std::path::Path,
) -> Result<(), terminal_snapshot_renderer::TerminalSnapshotReasonCode> {
    use std::path::{Component, Prefix};
    use terminal_snapshot_renderer::TerminalSnapshotReasonCode as C;

    if !path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
    {
        return Err(C::UnsafePath);
    }
    let raw = path.as_os_str().to_str().ok_or(C::UnsafePath)?;
    if raw
        .split(['\\', '/'])
        .any(|component| matches!(component, "." | ".."))
    {
        return Err(C::UnsafePath);
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
            _ => return Err(C::UnsafePath),
        },
        _ => return Err(C::UnsafePath),
    };
    if raw
        .char_indices()
        .any(|(index, value)| value == ':' && Some(index) != allowed_drive_colon)
    {
        return Err(C::UnsafePath);
    }
    for component in path.components() {
        let Component::Normal(value) = component else {
            continue;
        };
        let value = value.to_str().ok_or(C::UnsafePath)?;
        if value.ends_with([' ', '.']) {
            return Err(C::UnsafePath);
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
            return Err(C::UnsafePath);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
