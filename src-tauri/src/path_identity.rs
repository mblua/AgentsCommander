//! Narrow filesystem identity primitives for privileged PTY input.
//!
//! These helpers do not repair configuration. They reject link/reparse
//! components, bind checks to stable filesystem object IDs, and cap every
//! security-bearing read.

use std::collections::HashSet;
use std::fs::{File, Metadata, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};

use serde::de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor};
use sha2::{Digest, Sha256};

#[cfg(any(target_os = "linux", target_os = "android"))]
const UNIX_O_NOFOLLOW: i32 = 0x0002_0000;
#[cfg(all(unix, not(any(target_os = "linux", target_os = "android"))))]
const UNIX_O_NOFOLLOW: i32 = 0x0000_0100;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FileObjectId {
    pub volume: u64,
    pub file: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedMetadata {
    pub is_dir: bool,
    pub is_file: bool,
    pub len: u64,
    pub links: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedPathIdentity {
    pub canonical_path: PathBuf,
    pub object_id: FileObjectId,
    pub metadata: VerifiedMetadata,
    pub content_sha256: Option<[u8; 32]>,
}

fn is_link_or_reparse(metadata: &Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
        metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }
    #[cfg(not(windows))]
    {
        false
    }
}

#[cfg(unix)]
fn handle_identity(file: &File) -> Result<(FileObjectId, u64), String> {
    use std::os::unix::fs::MetadataExt;
    let metadata = file.metadata().map_err(|_| "unsafe_path".to_string())?;
    Ok((
        FileObjectId {
            volume: metadata.dev(),
            file: metadata.ino(),
        },
        metadata.nlink(),
    ))
}

#[cfg(windows)]
fn handle_identity(file: &File) -> Result<(FileObjectId, u64), String> {
    use std::mem::MaybeUninit;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION,
    };

    let mut information = MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::zeroed();
    let succeeded =
        unsafe { GetFileInformationByHandle(file.as_raw_handle(), information.as_mut_ptr()) };
    if succeeded == 0 {
        return Err("unsafe_path".to_string());
    }
    let information = unsafe { information.assume_init() };
    Ok((
        FileObjectId {
            volume: u64::from(information.dwVolumeSerialNumber),
            file: (u64::from(information.nFileIndexHigh) << 32)
                | u64::from(information.nFileIndexLow),
        },
        u64::from(information.nNumberOfLinks),
    ))
}

#[cfg(not(any(unix, windows)))]
fn handle_identity(file: &File) -> Result<(FileObjectId, u64), String> {
    let metadata = file.metadata().map_err(|_| "unsafe_path".to_string())?;
    Ok((
        FileObjectId {
            volume: 0,
            file: metadata.len(),
        },
        1,
    ))
}

fn snapshot(metadata: &Metadata, links: u64) -> VerifiedMetadata {
    VerifiedMetadata {
        is_dir: metadata.is_dir(),
        is_file: metadata.is_file(),
        len: metadata.len(),
        links,
    }
}

/// Inspect every existing path component without following a link/reparse
/// component. The path itself must exist.
pub fn verify_component_chain(path: &Path) -> Result<(), String> {
    let mut current = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(_) | Component::RootDir => {
                current.push(component.as_os_str());
                continue;
            }
            Component::CurDir => continue,
            Component::ParentDir => {
                return Err("unsafe_path".to_string());
            }
            Component::Normal(part) => current.push(part),
        }
        let metadata =
            std::fs::symlink_metadata(&current).map_err(|_| "unsafe_path".to_string())?;
        if is_link_or_reparse(&metadata) {
            return Err("unsafe_path".to_string());
        }
    }
    Ok(())
}

pub fn verify_directory(path: &Path) -> Result<VerifiedPathIdentity, String> {
    verify_component_chain(path)?;
    let entry_before = std::fs::symlink_metadata(path).map_err(|_| "unsafe_path".to_string())?;
    if !entry_before.is_dir() || is_link_or_reparse(&entry_before) {
        return Err("unsafe_path".to_string());
    }
    let opened = open_directory_no_follow(path)?;
    let opened_metadata = opened.metadata().map_err(|_| "unsafe_path".to_string())?;
    if !opened_metadata.is_dir() || is_link_or_reparse(&opened_metadata) {
        return Err("unsafe_path".to_string());
    }
    let (opened_id, links) = handle_identity(&opened)?;
    let canonical_path = std::fs::canonicalize(path).map_err(|_| "unsafe_path".to_string())?;
    let entry_after = std::fs::symlink_metadata(path).map_err(|_| "unsafe_path".to_string())?;
    if !entry_after.is_dir() || is_link_or_reparse(&entry_after) {
        return Err("unsafe_path".to_string());
    }
    let reopened = open_directory_no_follow(path)?;
    let (reopened_id, _) = handle_identity(&reopened)?;
    if reopened_id != opened_id {
        return Err("unsafe_path".to_string());
    }
    Ok(VerifiedPathIdentity {
        canonical_path,
        object_id: opened_id,
        metadata: snapshot(&opened_metadata, links),
        content_sha256: None,
    })
}

fn open_directory_no_follow(path: &Path) -> Result<File, String> {
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
            FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE,
            FILE_SHARE_READ, FILE_SHARE_WRITE,
        };
        options
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT);
    }
    options.open(path).map_err(|_| "unsafe_path".to_string())
}

pub fn verify_regular_file(path: &Path) -> Result<VerifiedPathIdentity, String> {
    let file = open_read_no_follow(path)?;
    verify_opened_regular_file(path, &file, false)
}

pub fn verify_opened_regular_file(
    path: &Path,
    file: &File,
    require_empty: bool,
) -> Result<VerifiedPathIdentity, String> {
    let parent = path.parent().ok_or_else(|| "unsafe_path".to_string())?;
    verify_component_chain(parent)?;
    let entry = std::fs::symlink_metadata(path).map_err(|_| "unsafe_path".to_string())?;
    if !entry.is_file() || is_link_or_reparse(&entry) {
        return Err("unsafe_path".to_string());
    }
    let metadata = file.metadata().map_err(|_| "unsafe_path".to_string())?;
    let (opened_id, links) = handle_identity(file)?;
    if !metadata.is_file()
        || is_link_or_reparse(&metadata)
        || links != 1
        || (require_empty && metadata.len() != 0)
    {
        return Err("unsafe_path".to_string());
    }
    let reopened = open_read_no_follow(path)?;
    let (reopened_id, reopened_links) = handle_identity(&reopened)?;
    if reopened_id != opened_id || reopened_links != 1 {
        return Err("unsafe_path".to_string());
    }
    Ok(VerifiedPathIdentity {
        canonical_path: std::fs::canonicalize(path).map_err(|_| "unsafe_path".to_string())?,
        object_id: opened_id,
        metadata: snapshot(&metadata, links),
        content_sha256: None,
    })
}

fn open_read_no_follow(path: &Path) -> Result<File, String> {
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

fn read_bounded_regular_inner<F>(
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

#[cfg(windows)]
pub fn publish_new_file_atomic(source: &Path, destination: &Path) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn MoveFileExW(existing: *const u16, new: *const u16, flags: u32) -> i32;
    }
    const MOVEFILE_WRITE_THROUGH: u32 = 0x0000_0008;
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

#[cfg(any(target_os = "linux", target_os = "android"))]
pub fn publish_new_file_atomic(source: &Path, destination: &Path) -> Result<(), String> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    unsafe extern "C" {
        fn renameat2(
            old_dir_fd: i32,
            old_path: *const std::ffi::c_char,
            new_dir_fd: i32,
            new_path: *const std::ffi::c_char,
            flags: u32,
        ) -> i32;
    }
    const AT_FDCWD: i32 = -100;
    const RENAME_NOREPLACE: u32 = 1;
    let source = CString::new(source.as_os_str().as_bytes())
        .map_err(|_| "atomic_publish_failed".to_string())?;
    let destination = CString::new(destination.as_os_str().as_bytes())
        .map_err(|_| "atomic_publish_failed".to_string())?;
    let result = unsafe {
        renameat2(
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

#[cfg(any(target_os = "macos", target_os = "ios"))]
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
    const RENAME_EXCL: u32 = 0x0000_0004;
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

#[cfg(not(any(
    windows,
    target_os = "linux",
    target_os = "android",
    target_os = "macos",
    target_os = "ios"
)))]
pub fn publish_new_file_atomic(_source: &Path, _destination: &Path) -> Result<(), String> {
    Err("atomic_publish_unsupported".to_string())
}

#[cfg(windows)]
fn atomic_replace_existing(source: &Path, destination: &Path) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;

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
fn atomic_replace_existing(source: &Path, destination: &Path) -> Result<(), String> {
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

pub struct ScannedJsonDocument {
    pub value: serde_json::Value,
    pub duplicate_keys: bool,
    pub privileged_candidate: bool,
}

#[derive(Default)]
struct JsonScanState {
    duplicate_keys: bool,
    privileged_candidate: bool,
}

/// Parse one JSON document while retaining whether any object key was
/// duplicated and whether any top-level occurrence identifies privileged PTY
/// input. Escaped JSON key and value spellings are decoded by Serde before the
/// classification is recorded.
pub fn scan_json_document(bytes: &[u8]) -> Result<ScannedJsonDocument, String> {
    let state = std::rc::Rc::new(std::cell::RefCell::new(JsonScanState::default()));
    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    let value = ScanningValue {
        state: std::rc::Rc::clone(&state),
        depth: 0,
    }
    .deserialize(&mut deserializer)
    .map_err(|_| "invalid_envelope".to_string())?;
    deserializer
        .end()
        .map_err(|_| "invalid_envelope".to_string())?;
    let state = state.borrow();
    Ok(ScannedJsonDocument {
        value,
        duplicate_keys: state.duplicate_keys,
        privileged_candidate: state.privileged_candidate,
    })
}

/// Parse JSON while rejecting duplicate object keys at every depth.
pub fn parse_json_no_duplicates(bytes: &[u8]) -> Result<serde_json::Value, String> {
    let document = scan_json_document(bytes)?;
    if document.duplicate_keys {
        return Err("invalid_envelope".to_string());
    }
    Ok(document.value)
}

struct ScanningValue {
    state: std::rc::Rc<std::cell::RefCell<JsonScanState>>,
    depth: usize,
}

impl<'de> DeserializeSeed<'de> for ScanningValue {
    type Value = serde_json::Value;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_any(ScanningVisitor {
            state: self.state,
            depth: self.depth,
        })
    }
}

struct ScanningVisitor {
    state: std::rc::Rc<std::cell::RefCell<JsonScanState>>,
    depth: usize,
}

impl<'de> Visitor<'de> for ScanningVisitor {
    type Value = serde_json::Value;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("a JSON value")
    }

    fn visit_bool<E>(self, value: bool) -> Result<Self::Value, E> {
        Ok(serde_json::Value::Bool(value))
    }

    fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E> {
        Ok(serde_json::Value::Number(value.into()))
    }

    fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E> {
        Ok(serde_json::Value::Number(value.into()))
    }

    fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        serde_json::Number::from_f64(value)
            .map(serde_json::Value::Number)
            .ok_or_else(|| E::custom("invalid number"))
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(serde_json::Value::String(value.to_string()))
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E> {
        Ok(serde_json::Value::String(value))
    }

    fn visit_none<E>(self) -> Result<Self::Value, E> {
        Ok(serde_json::Value::Null)
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E> {
        Ok(serde_json::Value::Null)
    }

    fn visit_some<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        ScanningValue {
            state: self.state,
            depth: self.depth,
        }
        .deserialize(deserializer)
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut values = Vec::new();
        while let Some(value) = sequence.next_element_seed(ScanningValue {
            state: std::rc::Rc::clone(&self.state),
            depth: self.depth + 1,
        })? {
            values.push(value);
        }
        Ok(serde_json::Value::Array(values))
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut seen = HashSet::new();
        let mut values = serde_json::Map::new();
        while let Some(key) = map.next_key::<String>()? {
            if !seen.insert(key.clone()) {
                self.state.borrow_mut().duplicate_keys = true;
            }
            let value = map.next_value_seed(ScanningValue {
                state: std::rc::Rc::clone(&self.state),
                depth: self.depth + 1,
            })?;
            if self.depth == 0 {
                let privileged = key == "ptyInput"
                    || (key == "action" && value.as_str() == Some("pty-input"))
                    || (key == "kind" && value.as_str() == Some("pty-input-marker"));
                if privileged {
                    self.state.borrow_mut().privileged_candidate = true;
                }
            }
            values.insert(key, value);
        }
        Ok(serde_json::Value::Object(values))
    }
}

/// A caller-owned output file whose leaf handle and parent directory remain
/// retained until the write and identity checks complete.
pub struct VerifiedNewFile {
    path: PathBuf,
    file: File,
    parent: VerifiedPathIdentity,
}

impl VerifiedNewFile {
    /// Failure intentionally leaves the newly created caller-owned file in
    /// place. The CLI must never unlink a path after ownership becomes unclear.
    pub fn write_all_and_sync(mut self, bytes: &[u8]) -> Result<(), String> {
        self.file
            .write_all(bytes)
            .and_then(|_| self.file.flush())
            .and_then(|_| self.file.sync_all())
            .map_err(|_| "output_failed".to_string())?;
        let opened = verify_opened_regular_file(&self.path, &self.file, false)
            .map_err(|_| "output_failed".to_string())?;
        let current_parent = verify_directory(
            self.path
                .parent()
                .ok_or_else(|| "output_failed".to_string())?,
        )
        .map_err(|_| "output_failed".to_string())?;
        if !same_object(&self.parent, &current_parent)
            || opened.metadata.links != 1
            || opened.metadata.len != bytes.len() as u64
        {
            return Err("output_failed".to_string());
        }
        Ok(())
    }
}

pub fn create_terminal_snapshot_output(path: &Path) -> Result<VerifiedNewFile, String> {
    create_terminal_snapshot_output_inner(path, || {})
}

fn create_terminal_snapshot_output_inner(
    path: &Path,
    before_create: impl FnOnce(),
) -> Result<VerifiedNewFile, String> {
    validate_terminal_snapshot_output_path(path)?;
    let parent_path = path.parent().ok_or_else(|| "unsafe_path".to_string())?;
    let parent = verify_directory(parent_path)?;
    match std::fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        _ => return Err("unsafe_path".to_string()),
    }
    before_create();
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
        use windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT;
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let file = options
        .open(path)
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
    verify_opened_regular_file(path, &file, true).map_err(|_| "output_failed".to_string())?;
    let current_parent = verify_directory(parent_path).map_err(|_| "output_failed".to_string())?;
    if !same_object(&parent, &current_parent) {
        return Err("output_failed".to_string());
    }
    Ok(VerifiedNewFile {
        path: path.to_path_buf(),
        file,
        parent,
    })
}

pub fn validate_terminal_snapshot_output_path(path: &Path) -> Result<(), String> {
    if !path.is_absolute() {
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
    let parent = path.parent().ok_or_else(|| "unsafe_path".to_string())?;
    verify_directory(parent)?;
    Ok(())
}

#[cfg(windows)]
fn validate_windows_snapshot_output_path(path: &Path) -> Result<(), String> {
    use std::path::Prefix;

    let raw = path.as_os_str().to_string_lossy();
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
        let stem = value.split('.').next().unwrap_or(value);
        let reserved = matches!(
            stem.to_ascii_uppercase().as_str(),
            "CON"
                | "PRN"
                | "AUX"
                | "NUL"
                | "COM1"
                | "COM2"
                | "COM3"
                | "COM4"
                | "COM5"
                | "COM6"
                | "COM7"
                | "COM8"
                | "COM9"
                | "LPT1"
                | "LPT2"
                | "LPT3"
                | "LPT4"
                | "LPT5"
                | "LPT6"
                | "LPT7"
                | "LPT8"
                | "LPT9"
        );
        if reserved
            || value
                .chars()
                .any(|character| matches!(character, '<' | '>' | '"' | '|' | '?' | '*'))
        {
            return Err("unsafe_path".to_string());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duplicate_json_keys_reject_recursively() {
        assert!(parse_json_no_duplicates(br#"{"a":1,"a":2}"#).is_err());
        assert!(parse_json_no_duplicates(br#"{"a":{"b":1,"b":2}}"#).is_err());
        assert!(parse_json_no_duplicates(br#"{"a":[{"b":1}]}"#).is_ok());
    }

    #[test]
    fn scan_detects_escaped_and_overwritten_privileged_occurrences() {
        let document =
            scan_json_document(br#"{"action":"pty\u002dinput","action":"ordinary","body":"x"}"#)
                .unwrap();
        assert!(document.duplicate_keys);
        assert!(document.privileged_candidate);
        assert_eq!(document.value["action"], "ordinary");

        let standard = scan_json_document(br#"{"body":"pty-input","body":"ordinary"}"#).unwrap();
        assert!(standard.duplicate_keys);
        assert!(!standard.privileged_candidate);
    }

    #[test]
    fn bounded_read_returns_content_fingerprint() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("value.json");
        std::fs::write(&path, b"{}" as &[u8]).unwrap();
        let (bytes, identity) = read_bounded_regular(&path, 16).unwrap();
        assert_eq!(bytes, b"{}");
        assert!(identity.content_sha256.is_some());
        assert!(read_bounded_regular(&path, 1).is_err());
    }

    #[test]
    fn bounded_read_rejects_same_length_in_place_mutation_between_snapshots() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("identity.json");
        std::fs::write(&path, b"first").unwrap();
        let replacement = path.clone();
        let result = read_bounded_regular_inner(&path, 16, move || {
            std::fs::write(replacement, b"other").unwrap();
        });
        assert!(result.is_err());
    }

    #[test]
    fn atomic_new_file_publication_never_clobbers_an_existing_destination() {
        let dir = tempfile::TempDir::new().unwrap();
        let source = dir.path().join("source.tmp");
        let destination = dir.path().join("destination.json");
        std::fs::write(&source, b"new").unwrap();
        std::fs::write(&destination, b"existing").unwrap();

        assert!(publish_new_file_atomic(&source, &destination).is_err());
        assert_eq!(std::fs::read(&source).unwrap(), b"new");
        assert_eq!(std::fs::read(&destination).unwrap(), b"existing");
    }

    #[test]
    fn atomic_regular_replacement_is_bounded_and_leaves_a_complete_file() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("marker.json");
        std::fs::write(&path, b"old").unwrap();
        let before = verify_regular_file(&path).unwrap();
        let (_, expected) = read_bounded_regular(&path, 32).unwrap();
        let after = replace_regular_file_atomic(&path, &expected, b"new marker", 32).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"new marker");
        assert!(!same_object(&before, &after));
        assert!(replace_regular_file_atomic(&path, &after, &[b'x'; 33], 32).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"new marker");

        let (_, expected) = read_bounded_regular(&path, 32).unwrap();
        std::fs::write(&path, b"other value").unwrap();
        assert!(replace_regular_file_atomic(&path, &expected, b"marker", 32).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"other value");
    }

    #[test]
    fn regular_file_verification_rejects_hard_links() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("identity.json");
        let alias = dir.path().join("identity-alias.json");
        std::fs::write(&path, b"{}").unwrap();
        std::fs::hard_link(&path, &alias).unwrap();
        assert!(verify_regular_file(&path).is_err());
        assert!(read_bounded_regular(&path, 16).is_err());
    }

    #[test]
    fn directory_identity_detects_same_spelling_replacement() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("replica");
        let retired = dir.path().join("retired");
        std::fs::create_dir(&path).unwrap();
        let before = verify_directory(&path).unwrap();
        std::fs::rename(&path, &retired).unwrap();
        std::fs::create_dir(&path).unwrap();
        let after = verify_directory(&path).unwrap();
        assert!(!same_object(&before, &after));
    }

    #[test]
    fn component_chain_rejects_a_linked_ancestor_when_supported() {
        let dir = tempfile::TempDir::new().unwrap();
        let real = dir.path().join("real");
        let link = dir.path().join("link");
        std::fs::create_dir(&real).unwrap();
        std::fs::write(real.join("identity.json"), b"{}").unwrap();
        #[cfg(unix)]
        let linked = std::os::unix::fs::symlink(&real, &link).is_ok();
        #[cfg(windows)]
        let linked = std::os::windows::fs::symlink_dir(&real, &link).is_ok();
        #[cfg(not(any(unix, windows)))]
        let linked = false;
        if linked {
            assert!(verify_directory(&link).is_err());
            assert!(read_bounded_regular(&link.join("identity.json"), 16).is_err());
        }
    }

    #[test]
    fn terminal_snapshot_post_validation_create_race_is_output_failed() {
        let directory = tempfile::TempDir::new().unwrap();
        let output = directory.path().join("raced.png");
        let raced = output.clone();
        let error = create_terminal_snapshot_output_inner(&output, move || {
            std::fs::write(raced, b"collision").unwrap();
        })
        .err()
        .unwrap();
        assert_eq!(error, "output_failed");
        assert_eq!(std::fs::read(output).unwrap(), b"collision");
    }

    #[cfg(windows)]
    #[test]
    fn terminal_snapshot_windows_namespaces_reject_lexically() {
        for path in [
            r"\\.\C:\snapshot.png",
            r"\\?\GLOBALROOT\Device\HarddiskVolume1\snapshot.png",
            r"\\?\Volume{00000000-0000-0000-0000-000000000000}\snapshot.png",
            r"C:\safe\snapshot.png:stream",
            r"C:\safe\CON.png",
            r"C:\safe\trailing.\snapshot.png",
        ] {
            assert!(validate_windows_snapshot_output_path(Path::new(path)).is_err());
        }
    }

    #[test]
    fn terminal_snapshot_output_is_absolute_png_create_new() {
        let directory = tempfile::TempDir::new().unwrap();
        let output = directory.path().join("snapshot.PNG");
        let file = create_terminal_snapshot_output(&output).unwrap();
        file.write_all_and_sync(b"owned bytes").unwrap();
        assert_eq!(std::fs::read(&output).unwrap(), b"owned bytes");
        assert!(create_terminal_snapshot_output(&output).is_err());
        assert!(validate_terminal_snapshot_output_path(Path::new("relative.png")).is_err());
        assert!(
            validate_terminal_snapshot_output_path(&directory.path().join("wrong.txt")).is_err()
        );
    }
}
