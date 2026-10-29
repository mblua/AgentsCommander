//! Physical operations for pinned seed-manifest files.
use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
#[derive(Debug)]
pub(crate) struct PhysicalFileIdentity {
    pub(crate) volume: u64,
    pub(crate) file: u128,
}
#[derive(Debug)]
pub(crate) struct PhysicalHandleFacts {
    pub(crate) identity: PhysicalFileIdentity,
    pub(crate) is_directory: bool,
    pub(crate) is_regular_file: bool,
    pub(crate) is_reparse: bool,
    pub(crate) links: u64,
    pub(crate) length: u64,
    pub(crate) attributes: u32,
    pub(crate) creation_time: u64,
}

pub(crate) struct ManifestFile(File);
impl std::fmt::Debug for ManifestFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Debug::fmt(&self.0, f)
    }
}
#[cfg(test)]
impl From<File> for ManifestFile {
    fn from(file: File) -> Self {
        Self(file)
    }
}
#[cfg(all(test, windows))]
impl std::os::windows::io::AsRawHandle for ManifestFile {
    fn as_raw_handle(&self) -> std::os::windows::io::RawHandle {
        std::os::windows::io::AsRawHandle::as_raw_handle(&self.0)
    }
}
impl ManifestFile {
    pub(crate) fn sync_all(&self) -> io::Result<()> {
        self.0.sync_all()
    }
    pub(crate) fn unlock(&self) -> io::Result<()> {
        File::unlock(&self.0)
    }
    pub(crate) fn try_lock(&self) -> Result<(), ManifestLockError> {
        self.0.try_lock().map_err(|error| match error {
            std::fs::TryLockError::WouldBlock => ManifestLockError::WouldBlock,
            std::fs::TryLockError::Error(source) => ManifestLockError::Error(source),
        })
    }
}
pub(crate) enum ManifestLockError {
    WouldBlock,
    Error(io::Error),
}
impl io::Read for ManifestFile {
    fn read(&mut self, b: &mut [u8]) -> io::Result<usize> {
        self.0.read(b)
    }
}
impl io::Seek for ManifestFile {
    fn seek(&mut self, p: io::SeekFrom) -> io::Result<u64> {
        self.0.seek(p)
    }
}
impl io::Write for ManifestFile {
    fn write(&mut self, b: &[u8]) -> io::Result<usize> {
        self.0.write(b)
    }
    fn write_all(&mut self, b: &[u8]) -> io::Result<()> {
        self.0.write_all(b)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}
pub(crate) fn inspect_manifest_path(path: &Path) -> io::Result<std::fs::Metadata> {
    std::fs::symlink_metadata(path)
}
pub(crate) fn canonical_manifest_project(path: &Path) -> io::Result<PathBuf> {
    std::fs::canonicalize(path)
}
pub(crate) fn manifest_temp_entries(path: &Path) -> io::Result<ManifestEntries> {
    std::fs::read_dir(path).map(ManifestEntries)
}
pub(crate) struct ManifestEntries(std::fs::ReadDir);
pub(crate) struct ManifestEntry(std::fs::DirEntry);
impl Iterator for ManifestEntries {
    type Item = io::Result<ManifestEntry>;
    fn next(&mut self) -> Option<Self::Item> {
        self.0.next().map(|r| r.map(ManifestEntry))
    }
}
impl ManifestEntry {
    pub(crate) fn file_name(&self) -> std::ffi::OsString {
        self.0.file_name()
    }
}
pub(crate) fn remove_manifest_temp(path: &Path) -> io::Result<()> {
    std::fs::remove_file(path)
}
#[cfg(not(windows))]
pub(crate) fn rename_manifest_temp(temp: &Path, canonical: &Path) -> io::Result<()> {
    std::fs::rename(temp, canonical)
}
pub(crate) fn is_windows_reparse(metadata: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;

        metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }

    #[cfg(not(windows))]
    {
        let _ = metadata;
        false
    }
}

#[cfg(unix)]
pub(crate) fn open_directory_no_follow(path: &Path) -> io::Result<ManifestFile> {
    use std::os::unix::fs::OpenOptionsExt;

    OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(path)
        .map(ManifestFile)
}

#[cfg(windows)]
pub(crate) fn open_directory_no_follow(path: &Path) -> io::Result<ManifestFile> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE,
        FILE_SHARE_READ, FILE_SHARE_WRITE,
    };

    OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
        .map(ManifestFile)
}

#[cfg(not(any(unix, windows)))]
pub(crate) fn open_directory_no_follow(path: &Path) -> io::Result<ManifestFile> {
    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "path is not a real directory",
        ));
    }
    File::open(path).map(ManifestFile)
}

#[cfg(unix)]
pub(crate) fn open_regular_no_follow(
    path: &Path,
    writable: bool,
    create_new: bool,
) -> io::Result<ManifestFile> {
    use std::os::unix::fs::OpenOptionsExt;

    let mut options = OpenOptions::new();
    options
        .read(true)
        .write(writable)
        .custom_flags(libc::O_NOFOLLOW);
    if create_new {
        options.create_new(true);
    }
    options.open(path).map(ManifestFile)
}

#[cfg(windows)]
pub(crate) fn open_regular_no_follow(
    path: &Path,
    writable: bool,
    create_new: bool,
) -> io::Result<ManifestFile> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    };

    let mut options = OpenOptions::new();
    options
        .read(true)
        .write(writable)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    if create_new {
        options.create_new(true);
    }
    options.open(path).map(ManifestFile)
}

#[cfg(not(any(unix, windows)))]
pub(crate) fn open_regular_no_follow(
    path: &Path,
    writable: bool,
    create_new: bool,
) -> io::Result<ManifestFile> {
    if !create_new {
        let metadata = std::fs::symlink_metadata(path)?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "path is not a real regular file",
            ));
        }
    }
    let mut options = OpenOptions::new();
    options
        .read(true)
        .write(writable)
        .create_new(create_new)
        .open(path)
        .map(ManifestFile)
}

#[cfg(unix)]
pub(crate) fn handle_facts(file: &ManifestFile) -> io::Result<PhysicalHandleFacts> {
    use std::os::unix::fs::MetadataExt;

    let metadata = file.0.metadata()?;
    Ok(PhysicalHandleFacts {
        identity: PhysicalFileIdentity {
            volume: metadata.dev(),
            file: u128::from(metadata.ino()),
        },
        is_directory: metadata.is_dir(),
        is_regular_file: metadata.is_file(),
        is_reparse: false,
        links: metadata.nlink(),
        length: metadata.len(),
        attributes: 0,
        creation_time: 0,
    })
}

#[cfg(windows)]
pub(crate) fn handle_facts(file: &ManifestFile) -> io::Result<PhysicalHandleFacts> {
    use std::mem::MaybeUninit;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_DIRECTORY,
        FILE_ATTRIBUTE_REPARSE_POINT,
    };

    let mut info = MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::zeroed();
    let success =
        unsafe { GetFileInformationByHandle(file.0.as_raw_handle() as HANDLE, info.as_mut_ptr()) };
    if success == 0 {
        return Err(io::Error::last_os_error());
    }
    let info = unsafe { info.assume_init() };
    let index = (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow);
    let length = (u64::from(info.nFileSizeHigh) << 32) | u64::from(info.nFileSizeLow);
    let creation_time = (u64::from(info.ftCreationTime.dwHighDateTime) << 32)
        | u64::from(info.ftCreationTime.dwLowDateTime);
    let is_directory = info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0;
    let is_reparse = info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0;
    Ok(PhysicalHandleFacts {
        identity: PhysicalFileIdentity {
            volume: u64::from(info.dwVolumeSerialNumber),
            file: u128::from(index),
        },
        is_directory,
        is_regular_file: !is_directory,
        is_reparse,
        links: u64::from(info.nNumberOfLinks),
        length,
        attributes: info.dwFileAttributes,
        creation_time,
    })
}

#[cfg(not(any(unix, windows)))]
pub(crate) fn handle_facts(file: &ManifestFile) -> io::Result<PhysicalHandleFacts> {
    let metadata = file.0.metadata()?;
    Ok(PhysicalHandleFacts {
        identity: PhysicalFileIdentity {
            volume: 0,
            file: u128::from(metadata.len()),
        },
        is_directory: metadata.is_dir(),
        is_regular_file: metadata.is_file(),
        is_reparse: false,
        links: 1,
        length: metadata.len(),
        attributes: 0,
        creation_time: 0,
    })
}

#[cfg(windows)]
pub(crate) fn reopen_windows_zero_access(writer: &ManifestFile) -> io::Result<ManifestFile> {
    use std::os::windows::io::{AsRawHandle, FromRawHandle};
    use windows_sys::Win32::Foundation::{HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{
        ReOpenFile, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE, FILE_SHARE_READ,
        FILE_SHARE_WRITE,
    };

    let handle = unsafe {
        ReOpenFile(
            writer.0.as_raw_handle() as HANDLE,
            0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            FILE_FLAG_OPEN_REPARSE_POINT,
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    let file = unsafe { File::from_raw_handle(handle as _) };
    Ok(ManifestFile(file))
}

#[cfg(windows)]
pub(crate) fn open_windows_delete_handle(path: &Path) -> io::Result<ManifestFile> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Foundation::GENERIC_READ;
    use windows_sys::Win32::Storage::FileSystem::{
        DELETE, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    };

    OpenOptions::new()
        .access_mode(GENERIC_READ | DELETE)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
        .map(ManifestFile)
}

#[cfg(windows)]
pub(crate) fn publish_windows_manifest_namespace(
    replace: bool,
    temp_wide: &[u16],
    canonical_wide: &[u16],
) -> Result<(), u32> {
    use windows_sys::Win32::Foundation::GetLastError;
    use windows_sys::Win32::Storage::FileSystem::{
        MoveFileExW, ReplaceFileW, MOVEFILE_WRITE_THROUGH,
    };

    let success = match replace {
        false => unsafe {
            MoveFileExW(
                temp_wide.as_ptr(),
                canonical_wide.as_ptr(),
                MOVEFILE_WRITE_THROUGH,
            )
        },
        true => unsafe {
            ReplaceFileW(
                canonical_wide.as_ptr(),
                temp_wide.as_ptr(),
                std::ptr::null(),
                0,
                std::ptr::null(),
                std::ptr::null(),
            )
        },
    };
    if success == 0 {
        let raw_error = unsafe { GetLastError() };
        Err(raw_error)
    } else {
        Ok(())
    }
}
#[cfg(windows)]
pub(crate) fn mark_windows_manifest_temp_deleted(file: &ManifestFile) -> io::Result<()> {
    use std::mem::size_of;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::{GetLastError, HANDLE};
    use windows_sys::Win32::Storage::FileSystem::{
        FileDispositionInfo, SetFileInformationByHandle, FILE_DISPOSITION_INFO,
    };
    let disposition = FILE_DISPOSITION_INFO { DeleteFile: 1 };
    let success = unsafe {
        SetFileInformationByHandle(
            file.0.as_raw_handle() as HANDLE,
            FileDispositionInfo,
            (&disposition as *const FILE_DISPOSITION_INFO).cast(),
            u32::try_from(size_of::<FILE_DISPOSITION_INFO>()).unwrap_or(u32::MAX),
        )
    };
    if success == 0 {
        let raw_error = unsafe { GetLastError() };
        return Err(io::Error::from_raw_os_error(
            i32::try_from(raw_error).unwrap_or(i32::MAX),
        ));
    }
    Ok(())
}
#[cfg(windows)]
pub(crate) enum ManifestStreamError {
    Io {
        operation: &'static str,
        source: io::Error,
    },
}
#[cfg(windows)]
pub(crate) fn manifest_temp_has_only_unnamed_stream(
    wide: &[u16],
) -> Result<bool, ManifestStreamError> {
    use std::mem::MaybeUninit;
    use windows_sys::Win32::Foundation::{GetLastError, ERROR_HANDLE_EOF, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{
        FindClose, FindFirstStreamW, FindNextStreamW, FindStreamInfoStandard,
        WIN32_FIND_STREAM_DATA,
    };

    struct FindStreamHandle(windows_sys::Win32::Foundation::HANDLE);
    impl Drop for FindStreamHandle {
        fn drop(&mut self) {
            if unsafe { FindClose(self.0) } == 0 {
                log::warn!(
                    "[seed_manifest] FindClose failed after bounded temp stream inventory: {}",
                    io::Error::last_os_error()
                );
            }
        }
    }

    let mut data = MaybeUninit::<WIN32_FIND_STREAM_DATA>::zeroed();
    let raw_handle = unsafe {
        FindFirstStreamW(
            wide.as_ptr(),
            FindStreamInfoStandard,
            data.as_mut_ptr().cast(),
            0,
        )
    };
    if raw_handle == INVALID_HANDLE_VALUE {
        let raw_error = unsafe { GetLastError() };
        return Err(ManifestStreamError::Io {
            operation: "FindFirstStreamW",
            source: io::Error::from_raw_os_error(i32::try_from(raw_error).unwrap_or(i32::MAX)),
        });
    }
    let handle = FindStreamHandle(raw_handle);
    let mut data = unsafe { data.assume_init() };
    loop {
        let end = data
            .cStreamName
            .iter()
            .position(|unit| *unit == 0)
            .unwrap_or(data.cStreamName.len());
        let expected = "::$DATA".encode_utf16().collect::<Vec<_>>();
        if data.cStreamName[..end] != expected {
            return Ok(false);
        }

        let next =
            unsafe { FindNextStreamW(handle.0, (&mut data as *mut WIN32_FIND_STREAM_DATA).cast()) };
        if next != 0 {
            continue;
        }
        let raw_error = unsafe { GetLastError() };
        if raw_error == ERROR_HANDLE_EOF {
            break;
        }
        return Err(ManifestStreamError::Io {
            operation: "FindNextStreamW",
            source: io::Error::from_raw_os_error(i32::try_from(raw_error).unwrap_or(i32::MAX)),
        });
    }
    drop(handle);
    Ok(true)
}
