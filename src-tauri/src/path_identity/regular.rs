use super::handle_identity;
use super::is_link_or_reparse;
#[cfg(unix)]
use super::open_directory_no_follow;
use super::retain_immutable_directory;
use super::snapshot;
use super::verify_component_chain;
use super::verify_directory;
use super::verify_regular_file;
use super::VerifiedPathIdentity;
#[cfg(unix)]
use super::UNIX_O_NOFOLLOW;
use sha2::Digest;
use sha2::Sha256;
use std::fs::File;
use std::fs::OpenOptions;
use std::io::Read;
use std::io::Seek;
use std::io::SeekFrom;
use std::io::Write;
use std::path::Path;
#[cfg(windows)]
use std::path::PathBuf;

pub(super) fn open_read_no_follow(path: &Path) -> Result<File, String> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(UNIX_O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
        };
        options
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    options.open(path).map_err(|_| "unsafe_path".to_string())
}

/// Open a regular one-link file, read at most `max_bytes + 1`, and prove that
/// the directory entry still names the opened object afterward.
pub fn read_bounded_regular(
    path: &Path,
    max_bytes: usize,
) -> Result<(Vec<u8>, VerifiedPathIdentity), String> {
    read_bounded_regular_inner(path, max_bytes, || {})
}

pub(super) fn read_bounded_regular_inner<F>(
    path: &Path,
    max_bytes: usize,
    after_first_snapshot: F,
) -> Result<(Vec<u8>, VerifiedPathIdentity), String>
where
    F: FnOnce(),
{
    let parent = path.parent().ok_or_else(|| "unsafe_path".to_string())?;
    verify_component_chain(parent)?;
    let entry_before = std::fs::symlink_metadata(path).map_err(|_| "unsafe_path".to_string())?;
    if !entry_before.is_file() || is_link_or_reparse(&entry_before) {
        return Err("unsafe_path".to_string());
    }

    let mut file = open_read_no_follow(path)?;
    let opened_metadata = file.metadata().map_err(|_| "unsafe_path".to_string())?;
    let (opened_id, opened_links) = handle_identity(&file)?;
    if !opened_metadata.is_file() || is_link_or_reparse(&opened_metadata) || opened_links != 1 {
        return Err("unsafe_path".to_string());
    }

    let mut bytes = Vec::with_capacity(max_bytes.min(64 * 1024).saturating_add(1));
    Read::by_ref(&mut file)
        .take(max_bytes.saturating_add(1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| "unsafe_path".to_string())?;
    if bytes.len() > max_bytes {
        return Err("capacity_exceeded".to_string());
    }

    after_first_snapshot();
    file.seek(SeekFrom::Start(0))
        .map_err(|_| "unsafe_path".to_string())?;
    #[expect(
        clippy::large_stack_arrays,
        reason = "Preserve bounded 64 KiB streaming buffer and existing I/O behavior"
    )]
    let mut verification = [0_u8; 64 * 1024];
    let mut verified_bytes = 0usize;
    loop {
        let read = file
            .read(&mut verification)
            .map_err(|_| "unsafe_path".to_string())?;
        if read == 0 {
            break;
        }
        let end = verified_bytes
            .checked_add(read)
            .filter(|end| *end <= bytes.len())
            .ok_or_else(|| "unsafe_path".to_string())?;
        if bytes.get(verified_bytes..end) != Some(&verification[..read]) {
            return Err("unsafe_path".to_string());
        }
        verified_bytes = end;
    }
    if verified_bytes != bytes.len() {
        return Err("unsafe_path".to_string());
    }
    let opened_after = file.metadata().map_err(|_| "unsafe_path".to_string())?;
    let (opened_id_after, opened_links_after) = handle_identity(&file)?;
    if opened_id_after != opened_id
        || opened_links_after != opened_links
        || opened_after.len() != opened_metadata.len()
        || opened_after.modified().ok() != opened_metadata.modified().ok()
    {
        return Err("unsafe_path".to_string());
    }

    let entry_after = std::fs::symlink_metadata(path).map_err(|_| "unsafe_path".to_string())?;
    if !entry_after.is_file() || is_link_or_reparse(&entry_after) {
        return Err("unsafe_path".to_string());
    }
    let reopened = open_read_no_follow(path)?;
    let (reopened_id, reopened_links) = handle_identity(&reopened)?;
    if reopened_id != opened_id || reopened_links != 1 {
        return Err("unsafe_path".to_string());
    }

    let digest: [u8; 32] = Sha256::digest(&bytes).into();
    Ok((
        bytes,
        VerifiedPathIdentity {
            canonical_path: std::fs::canonicalize(path).map_err(|_| "unsafe_path".to_string())?,
            object_id: opened_id,
            metadata: snapshot(&opened_metadata, opened_links),
            content_sha256: Some(digest),
        },
    ))
}

#[cfg(windows)]
pub fn opened_file_is_delete_pending(file: &File) -> bool {
    use std::mem::MaybeUninit;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        FileStandardInfo, GetFileInformationByHandleEx, FILE_STANDARD_INFO,
    };

    let mut information = MaybeUninit::<FILE_STANDARD_INFO>::zeroed();
    let succeeded = unsafe {
        GetFileInformationByHandleEx(
            file.as_raw_handle(),
            FileStandardInfo,
            information.as_mut_ptr().cast(),
            std::mem::size_of::<FILE_STANDARD_INFO>() as u32,
        )
    };
    succeeded != 0 && unsafe { information.assume_init() }.DeletePending != 0
}

// The sole caller is a `#[cfg(not(unix))]` match arm, so this fallback is only
// reachable on a target that is neither Windows nor Unix.
#[cfg(not(any(windows, unix)))]
pub fn opened_file_is_delete_pending(_file: &File) -> bool {
    false
}

pub fn sync_parent_best_effort(path: &Path) {
    #[cfg(unix)]
    if let Some(parent) = path.parent() {
        if let Ok(directory) = File::open(parent) {
            let _ = directory.sync_all();
        }
    }
    #[cfg(not(unix))]
    let _ = path;
}

/// Remove only the regular one-link object proven by `expected`. Windows
/// deletion is issued through the verified leaf handle, closing the final
/// pathname validation-to-delete race without deleting an attacker replacement.
pub fn remove_regular_file_if_same(path: &Path, expected: &VerifiedPathIdentity) -> bool {
    remove_regular_file_if_same_inner(path, expected, || {})
}

pub(super) fn remove_regular_file_if_same_inner(
    path: &Path,
    expected: &VerifiedPathIdentity,
    after_validation: impl FnOnce(),
) -> bool {
    let Some(parent_path) = path.parent() else {
        return false;
    };
    let Ok(parent_before) = verify_directory(parent_path) else {
        return false;
    };
    let Ok(current) = verify_regular_file(path) else {
        return std::fs::symlink_metadata(path)
            .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound);
    };
    if !same_object(expected, &current) {
        return false;
    }
    after_validation();
    let Ok(parent) = retain_immutable_directory(parent_path) else {
        return false;
    };
    if !same_object(&parent_before, parent.identity()) {
        return false;
    }

    #[cfg(windows)]
    {
        remove_windows_regular_file_by_handle(path, expected)
    }
    #[cfg(not(windows))]
    {
        let Ok(current) = verify_regular_file(path) else {
            return std::fs::symlink_metadata(path)
                .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound);
        };
        if !same_object(expected, &current) {
            return false;
        }
        match std::fs::remove_file(path) {
            Ok(()) => true,
            Err(error) => error.kind() == std::io::ErrorKind::NotFound,
        }
    }
}

#[cfg(windows)]
pub(super) fn remove_windows_regular_file_by_handle(
    path: &Path,
    expected: &VerifiedPathIdentity,
) -> bool {
    remove_windows_regular_file_by_handle_inner(path, expected, || {})
}

#[cfg(windows)]
pub(super) fn remove_windows_regular_file_by_handle_inner(
    path: &Path,
    expected: &VerifiedPathIdentity,
    before_delete: impl FnOnce(),
) -> bool {
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        FileDispositionInfo, SetFileInformationByHandle, DELETE, FILE_DISPOSITION_INFO,
        FILE_FLAG_OPEN_REPARSE_POINT, FILE_READ_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE,
    };

    let mut options = OpenOptions::new();
    options
        .read(true)
        .access_mode(DELETE | FILE_READ_ATTRIBUTES)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    let Ok(file) = options.open(path) else {
        return false;
    };
    let Ok(metadata) = file.metadata() else {
        return false;
    };
    let Ok((object_id, links)) = handle_identity(&file) else {
        return false;
    };
    if !metadata.is_file()
        || is_link_or_reparse(&metadata)
        || links != 1
        || object_id != expected.object_id
    {
        return false;
    }
    before_delete();
    let disposition = FILE_DISPOSITION_INFO { DeleteFile: 1 };
    unsafe {
        SetFileInformationByHandle(
            file.as_raw_handle(),
            FileDispositionInfo,
            std::ptr::addr_of!(disposition).cast(),
            std::mem::size_of::<FILE_DISPOSITION_INFO>() as u32,
        ) != 0
    }
}

#[cfg(windows)]
pub fn publish_new_file_atomic(source: &Path, destination: &Path) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;

    fn canonical_leaf(path: &Path) -> Result<PathBuf, String> {
        let leaf = path
            .file_name()
            .ok_or_else(|| "atomic_publish_failed".to_string())?;
        let parent = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        let parent =
            std::fs::canonicalize(parent).map_err(|_| "atomic_publish_failed".to_string())?;
        Ok(parent.join(leaf))
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn MoveFileExW(existing: *const u16, new: *const u16, flags: u32) -> i32;
    }
    const MOVEFILE_WRITE_THROUGH: u32 = 0x0000_0008;
    // Canonicalizing only each existing parent supplies the Windows verbatim
    // prefix needed beyond MAX_PATH while preserving the un-followed leaf name.
    let source = canonical_leaf(source)?;
    let destination = canonical_leaf(destination)?;
    let existing: Vec<u16> = source.as_os_str().encode_wide().chain(Some(0)).collect();
    let new: Vec<u16> = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    // Omitting MOVEFILE_REPLACE_EXISTING is the Windows no-clobber contract.
    let moved = unsafe { MoveFileExW(existing.as_ptr(), new.as_ptr(), MOVEFILE_WRITE_THROUGH) };
    if moved == 0 {
        Err("atomic_publish_failed".to_string())
    } else {
        Ok(())
    }
}

#[cfg(target_os = "linux")]
pub fn publish_new_file_atomic(source: &Path, destination: &Path) -> Result<(), String> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    const AT_FDCWD: libc::c_int = libc::AT_FDCWD;
    const RENAME_NOREPLACE: libc::c_uint = libc::RENAME_NOREPLACE;
    let source = CString::new(source.as_os_str().as_bytes())
        .map_err(|_| "atomic_publish_failed".to_string())?;
    let destination = CString::new(destination.as_os_str().as_bytes())
        .map_err(|_| "atomic_publish_failed".to_string())?;
    let result = unsafe {
        libc::syscall(
            libc::SYS_renameat2,
            AT_FDCWD,
            source.as_ptr(),
            AT_FDCWD,
            destination.as_ptr(),
            RENAME_NOREPLACE,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err("atomic_publish_failed".to_string())
    }
}

#[cfg(target_os = "macos")]
pub fn publish_new_file_atomic(source: &Path, destination: &Path) -> Result<(), String> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    unsafe extern "C" {
        fn renamex_np(
            old_path: *const std::ffi::c_char,
            new_path: *const std::ffi::c_char,
            flags: u32,
        ) -> i32;
    }
    const RENAME_EXCL: libc::c_uint = libc::RENAME_EXCL;
    let source = CString::new(source.as_os_str().as_bytes())
        .map_err(|_| "atomic_publish_failed".to_string())?;
    let destination = CString::new(destination.as_os_str().as_bytes())
        .map_err(|_| "atomic_publish_failed".to_string())?;
    let result = unsafe { renamex_np(source.as_ptr(), destination.as_ptr(), RENAME_EXCL) };
    if result == 0 {
        Ok(())
    } else {
        Err("atomic_publish_failed".to_string())
    }
}

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
pub fn publish_new_file_atomic(_source: &Path, _destination: &Path) -> Result<(), String> {
    Err("atomic_publish_unsupported".to_string())
}

#[cfg(windows)]
pub(super) fn atomic_replace_existing(source: &Path, destination: &Path) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;

    fn canonical_leaf(path: &Path) -> Result<PathBuf, String> {
        let leaf = path
            .file_name()
            .ok_or_else(|| "atomic_replace_failed".to_string())?;
        let parent = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        let parent =
            std::fs::canonicalize(parent).map_err(|_| "atomic_replace_failed".to_string())?;
        Ok(parent.join(leaf))
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn ReplaceFileW(
            replaced: *const u16,
            replacement: *const u16,
            backup: *const u16,
            flags: u32,
            exclude: *mut std::ffi::c_void,
            reserved: *mut std::ffi::c_void,
        ) -> i32;
    }
    // Normalize only existing parents to support long Windows paths without
    // following either leaf before the atomic replacement.
    let source = canonical_leaf(source)?;
    let destination = canonical_leaf(destination)?;
    let replaced: Vec<u16> = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    let replacement: Vec<u16> = source.as_os_str().encode_wide().chain(Some(0)).collect();
    const REPLACEFILE_WRITE_THROUGH: u32 = 0x0000_0001;
    let replaced_ok = unsafe {
        ReplaceFileW(
            replaced.as_ptr(),
            replacement.as_ptr(),
            std::ptr::null(),
            REPLACEFILE_WRITE_THROUGH,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    if replaced_ok == 0 {
        Err("atomic_replace_failed".to_string())
    } else {
        Ok(())
    }
}

#[cfg(not(windows))]
pub(super) fn atomic_replace_existing(source: &Path, destination: &Path) -> Result<(), String> {
    std::fs::rename(source, destination).map_err(|_| "atomic_replace_failed".to_string())
}

/// Replace an already existing, verified regular file without an unlink gap.
/// The temporary file is written in the same verified directory and synced
/// before the platform's atomic replacement operation.
pub fn replace_regular_file_atomic(
    path: &Path,
    expected: &VerifiedPathIdentity,
    bytes: &[u8],
    max_bytes: usize,
) -> Result<VerifiedPathIdentity, String> {
    if bytes.len() > max_bytes {
        return Err("capacity_exceeded".to_string());
    }
    let (_, original) = read_bounded_regular(path, max_bytes)?;
    if !same_object(expected, &original)
        || expected.content_sha256.is_none()
        || expected.content_sha256 != original.content_sha256
    {
        return Err("unsafe_path".to_string());
    }
    let parent = path.parent().ok_or_else(|| "unsafe_path".to_string())?;
    let parent_identity = verify_directory(parent)?;
    let temporary = parent.join(format!(
        ".ac-pty-input-{}.tmp",
        uuid::Uuid::new_v4().simple()
    ));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(UNIX_O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let result = (|| {
        let mut temporary_file = options
            .open(&temporary)
            .map_err(|_| "atomic_replace_failed".to_string())?;
        temporary_file
            .write_all(bytes)
            .map_err(|_| "atomic_replace_failed".to_string())?;
        temporary_file
            .sync_all()
            .map_err(|_| "atomic_replace_failed".to_string())?;
        drop(temporary_file);

        let current_parent = verify_directory(parent)?;
        if !same_object(&parent_identity, &current_parent) {
            return Err("unsafe_path".to_string());
        }
        let current = open_read_no_follow(path)?;
        let (current_id, current_links) = handle_identity(&current)?;
        if current_id != original.object_id || current_links != 1 {
            return Err("unsafe_path".to_string());
        }
        atomic_replace_existing(&temporary, path)?;
        #[cfg(unix)]
        open_directory_no_follow(parent)?
            .sync_all()
            .map_err(|_| "atomic_replace_failed".to_string())?;

        let (published, identity) = read_bounded_regular(path, max_bytes)?;
        if published != bytes {
            return Err("atomic_replace_failed".to_string());
        }
        Ok(identity)
    })();
    if result.is_err() {
        let _cleanup_result = std::fs::remove_file(&temporary);
    }
    result
}

#[cfg(windows)]
fn component_eq(left: &std::ffi::OsStr, right: &std::ffi::OsStr) -> bool {
    use std::os::windows::ffi::OsStrExt;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CompareStringOrdinal(
            left: *const u16,
            left_len: i32,
            right: *const u16,
            right_len: i32,
            ignore_case: i32,
        ) -> i32;
    }
    const CSTR_EQUAL: i32 = 2;
    let left: Vec<u16> = left.encode_wide().collect();
    let right: Vec<u16> = right.encode_wide().collect();
    let Ok(left_len) = i32::try_from(left.len()) else {
        return false;
    };
    let Ok(right_len) = i32::try_from(right.len()) else {
        return false;
    };
    unsafe {
        CompareStringOrdinal(left.as_ptr(), left_len, right.as_ptr(), right_len, 1) == CSTR_EQUAL
    }
}

#[cfg(not(windows))]
fn component_eq(left: &std::ffi::OsStr, right: &std::ffi::OsStr) -> bool {
    left == right
}

pub fn paths_equivalent(left: &Path, right: &Path) -> bool {
    let mut left = left.components();
    let mut right = right.components();
    loop {
        match (left.next(), right.next()) {
            (Some(left), Some(right)) if component_eq(left.as_os_str(), right.as_os_str()) => {}
            (None, None) => return true,
            _ => return false,
        }
    }
}

pub fn path_is_descendant(child: &Path, parent: &Path) -> bool {
    let child_components: Vec<_> = child.components().collect();
    let parent_components: Vec<_> = parent.components().collect();
    child_components.len() >= parent_components.len()
        && child_components
            .iter()
            .zip(parent_components.iter())
            .all(|(child, parent)| component_eq(child.as_os_str(), parent.as_os_str()))
}

pub fn is_verified_descendant(child: &VerifiedPathIdentity, parent: &VerifiedPathIdentity) -> bool {
    path_is_descendant(&child.canonical_path, &parent.canonical_path)
}

pub fn same_object(a: &VerifiedPathIdentity, b: &VerifiedPathIdentity) -> bool {
    a.object_id == b.object_id
}
