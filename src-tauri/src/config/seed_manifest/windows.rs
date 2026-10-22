use super::{
    handle_facts, is_exact_not_found, open_regular_no_follow, stream_matches, CanonicalSnapshot,
    FileIdentity, OpenedRegularFile, ProjectSeedManifestGuard, SeedManifestError,
};
#[cfg(windows)]
use super::{WindowsNamespaceOperation, WindowsPathState, WindowsReplacePartial};
use std::fs::{File, OpenOptions};
use std::io;
use std::path::Path;

#[cfg(windows)]
impl WindowsPathState {
    pub(crate) fn is_exact_same(&self, identity: FileIdentity, require_one_link: bool) -> bool {
        matches!(
            self,
            Self::Same {
                identity: observed,
                links,
                bytes_match: true,
                ..
            } if *observed == identity && (!require_one_link || *links == 1)
        )
    }
}

#[cfg(windows)]
impl ProjectSeedManifestGuard {
    pub(crate) fn publish_windows_temp(
        &self,
        temp: OpenedRegularFile,
        canonical_path: &Path,
        bytes: &[u8],
    ) -> Result<OpenedRegularFile, SeedManifestError> {
        use windows_sys::Win32::Foundation::{
            ERROR_UNABLE_TO_MOVE_REPLACEMENT, ERROR_UNABLE_TO_MOVE_REPLACEMENT_2,
            ERROR_UNABLE_TO_REMOVE_REPLACED,
        };

        if temp.identity.volume != self.ac_root.identity.volume
            || temp.path.parent() != Some(self.ac_root.path.as_path())
            || canonical_path.parent() != Some(self.ac_root.path.as_path())
        {
            self.cleanup_owned_temp(&temp, Some(bytes));
            return Err(SeedManifestError::UnsafePath {
                path: temp.path,
                reason: "manifest temp and canonical must share the pinned .ac parent and volume"
                    .to_string(),
            });
        }
        verify_windows_temp_has_only_unnamed_stream(&temp.path)?;

        let source_identity = temp.identity;
        let source_length = temp.length;
        let temp_path = temp.path.clone();
        let source_witness = match reopen_windows_zero_access(&temp.file, &temp.path) {
            Ok(witness) => witness,
            Err(error) => {
                self.cleanup_owned_temp(&temp, Some(bytes));
                return Err(error);
            }
        };
        if source_witness.identity != source_identity
            || source_witness.length != source_length
            || source_witness.links != 1
        {
            self.cleanup_owned_temp(&temp, Some(bytes));
            return Err(SeedManifestError::UnsafePath {
                path: temp.path,
                reason: "zero-access ReOpenFile witness does not match the flushed temp"
                    .to_string(),
            });
        }

        drop(temp);

        let final_temp_state = inspect_windows_path(&temp_path, source_identity, bytes);
        if !final_temp_state.is_exact_same(source_identity, true) {
            return Err(SeedManifestError::UnsafePath {
                path: temp_path,
                reason: format!(
                    "temp path failed the final writer-closed identity probe: {final_temp_state:?}"
                ),
            });
        }
        self.revalidate_owner()?;
        self.verify_canonical_unchanged(canonical_path)?;
        let temp_wide = absolute_verbatim_utf16(&temp_path)?;
        let canonical_wide = absolute_verbatim_utf16(canonical_path)?;

        let (operation, destination_identity, destination_raw) = match &self.snapshot {
            CanonicalSnapshot::Writable {
                raw,
                canonical: Some(canonical),
                ..
            } => (
                WindowsNamespaceOperation::ReplaceFile,
                Some(canonical.identity),
                Some(raw.as_slice()),
            ),
            CanonicalSnapshot::Writable {
                canonical: None, ..
            } => (WindowsNamespaceOperation::MoveFileEx, None, None),
            CanonicalSnapshot::ReadOnly { .. } => {
                return Err(SeedManifestError::Validation(
                    "read-only snapshot reached the Windows namespace writer".to_string(),
                ));
            }
        };

        let namespace_result = self.call_windows_namespace(
            operation,
            &temp_path,
            canonical_path,
            &temp_wide,
            &canonical_wide,
        );
        match namespace_result {
            Ok(()) => {
                let final_file =
                    match open_validated_windows_final(canonical_path, source_identity, bytes) {
                        Ok(file) => file,
                        Err(final_state) => {
                            return Err(SeedManifestError::WindowsPostPublishIdentityConflict {
                                canonical: canonical_path.to_path_buf(),
                                temp: temp_path,
                                expected_source: source_identity,
                                final_state: Box::new(final_state),
                                old_destination_identity: Box::new(destination_identity),
                            });
                        }
                    };

                if let Some(expected_destination) = destination_identity {
                    let destination_facts_result = match &self.snapshot {
                        CanonicalSnapshot::Writable {
                            canonical: Some(canonical),
                            ..
                        } => handle_facts(&canonical.file),
                        _ => {
                            return Err(SeedManifestError::WindowsPostPublishIdentityConflict {
                                canonical: canonical_path.to_path_buf(),
                                temp: temp_path,
                                expected_source: source_identity,
                                final_state: Box::new(WindowsPathState::Same {
                                    identity: final_file.identity,
                                    links: final_file.links,
                                    length: final_file.length,
                                    bytes_match: true,
                                }),
                                old_destination_identity: Box::new(Some(expected_destination)),
                            })
                        }
                    };
                    let destination_facts = destination_facts_result.map_err(|_| {
                        SeedManifestError::WindowsPostPublishIdentityConflict {
                            canonical: canonical_path.to_path_buf(),
                            temp: temp_path.clone(),
                            expected_source: source_identity,
                            final_state: Box::new(WindowsPathState::Same {
                                identity: final_file.identity,
                                links: final_file.links,
                                length: final_file.length,
                                bytes_match: true,
                            }),
                            old_destination_identity: Box::new(Some(expected_destination)),
                        }
                    })?;
                    if destination_facts.identity != expected_destination
                        || !destination_facts.is_regular_file
                        || destination_facts.is_reparse
                    {
                        return Err(SeedManifestError::WindowsPostPublishIdentityConflict {
                            canonical: canonical_path.to_path_buf(),
                            temp: temp_path,
                            expected_source: source_identity,
                            final_state: Box::new(WindowsPathState::Same {
                                identity: final_file.identity,
                                links: final_file.links,
                                length: final_file.length,
                                bytes_match: true,
                            }),
                            old_destination_identity: Box::new(Some(expected_destination)),
                        });
                    }
                }
                Ok(final_file)
            }
            Err(raw_error) => {
                let canonical_state = match (destination_identity, destination_raw) {
                    (Some(identity), Some(raw)) => {
                        inspect_windows_path(canonical_path, identity, raw)
                    }
                    _ => inspect_windows_path(canonical_path, source_identity, bytes),
                };
                let temp_state = inspect_windows_path(&temp_path, source_identity, bytes);
                log::warn!(
                    "[seed_manifest] Windows namespace failure operation={:?} raw_error={} \
                        raw_error_hex=0x{:08x} canonical={} temp={} backup=NULL ac_identity={:?} \
                        lock_identity={:?} source_identity={:?} destination_identity={:?} \
                        destination_handle_live={} canonical_state={:?} temp_state={:?}",
                    operation,
                    raw_error,
                    raw_error,
                    canonical_path.display(),
                    temp_path.display(),
                    self.ac_root.identity,
                    self.lock_identity,
                    source_identity,
                    destination_identity,
                    destination_identity.is_some(),
                    canonical_state,
                    temp_state
                );

                match operation {
                    WindowsNamespaceOperation::MoveFileEx => {
                        if matches!(canonical_state, WindowsPathState::NotFound)
                            && temp_state.is_exact_same(source_identity, true)
                        {
                            if let Err(cleanup_error) = cleanup_windows_temp_with_witness(
                                &source_witness,
                                &temp_path,
                                source_identity,
                                Some(bytes),
                            ) {
                                log::warn!(
                                    "[seed_manifest] Windows current-owned temp cleanup failed \
                                        after MoveFileExW raw_error={} path={} cleanup_error={}",
                                    raw_error,
                                    temp_path.display(),
                                    cleanup_error
                                );
                            }
                            Err(SeedManifestError::WindowsNamespaceFailure {
                                operation,
                                raw_error,
                                canonical: canonical_path.to_path_buf(),
                                temp: temp_path,
                                canonical_state: Box::new(canonical_state),
                                temp_state: Box::new(temp_state),
                            })
                        } else {
                            Err(SeedManifestError::WindowsMoveRecoveryRequired {
                                raw_error,
                                canonical: canonical_path.to_path_buf(),
                                temp: temp_path,
                                canonical_state: Box::new(canonical_state),
                                temp_state: Box::new(temp_state),
                            })
                        }
                    }
                    WindowsNamespaceOperation::ReplaceFile => {
                        if raw_error == ERROR_UNABLE_TO_MOVE_REPLACEMENT {
                            return Err(SeedManifestError::WindowsReplacePartial {
                                raw_error,
                                partial: WindowsReplacePartial::CanonicalRemovedReplacementAtTemp,
                                canonical: canonical_path.to_path_buf(),
                                temp: temp_path,
                                canonical_state: Box::new(canonical_state),
                                temp_state: Box::new(temp_state),
                            });
                        }
                        if raw_error == ERROR_UNABLE_TO_MOVE_REPLACEMENT_2 {
                            return Err(SeedManifestError::WindowsReplacePartial {
                                raw_error,
                                partial:
                                    WindowsReplacePartial::ReplacementEnrichedOldDestinationRenamed,
                                canonical: canonical_path.to_path_buf(),
                                temp: temp_path,
                                canonical_state: Box::new(canonical_state),
                                temp_state: Box::new(temp_state),
                            });
                        }

                        let stable_destination = destination_identity
                            .is_some_and(|identity| canonical_state.is_exact_same(identity, false));
                        let stable_source = temp_state.is_exact_same(source_identity, true);
                        if stable_destination && stable_source {
                            if let Err(cleanup_error) = cleanup_windows_temp_with_witness(
                                &source_witness,
                                &temp_path,
                                source_identity,
                                Some(bytes),
                            ) {
                                log::warn!(
                                    "[seed_manifest] Windows current-owned temp cleanup failed \
                                        after ReplaceFileW raw_error={} path={} cleanup_error={}",
                                    raw_error,
                                    temp_path.display(),
                                    cleanup_error
                                );
                            }
                            Err(SeedManifestError::WindowsNamespaceFailure {
                                operation,
                                raw_error,
                                canonical: canonical_path.to_path_buf(),
                                temp: temp_path,
                                canonical_state: Box::new(canonical_state),
                                temp_state: Box::new(temp_state),
                            })
                        } else {
                            if raw_error == ERROR_UNABLE_TO_REMOVE_REPLACED {
                                log::warn!(
                                    "[seed_manifest] ReplaceFileW error 1175 contradicted the \
                                        documented stable-name state; preserving all observed names"
                                );
                            }
                            Err(SeedManifestError::WindowsReplaceRecoveryRequired {
                                raw_error,
                                canonical: canonical_path.to_path_buf(),
                                temp: temp_path,
                                canonical_state: Box::new(canonical_state),
                                temp_state: Box::new(temp_state),
                            })
                        }
                    }
                }
            }
        }
    }

    pub(crate) fn call_windows_namespace(
        &self,
        operation: WindowsNamespaceOperation,
        _temp: &Path,
        _canonical: &Path,
        temp_wide: &[u16],
        canonical_wide: &[u16],
    ) -> Result<(), u32> {
        #[cfg(test)]
        if let Some(hook) = &self.hooks.windows_namespace_call {
            return hook(operation, _temp, _canonical);
        }

        use windows_sys::Win32::Foundation::GetLastError;
        use windows_sys::Win32::Storage::FileSystem::{
            MoveFileExW, ReplaceFileW, MOVEFILE_WRITE_THROUGH,
        };

        let success = match operation {
            WindowsNamespaceOperation::MoveFileEx => unsafe {
                MoveFileExW(
                    temp_wide.as_ptr(),
                    canonical_wide.as_ptr(),
                    MOVEFILE_WRITE_THROUGH,
                )
            },
            WindowsNamespaceOperation::ReplaceFile => unsafe {
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
}

#[cfg(windows)]
pub(crate) fn absolute_verbatim_utf16(path: &Path) -> Result<Vec<u16>, SeedManifestError> {
    use std::os::windows::ffi::OsStrExt;

    if !path.is_absolute() {
        return Err(SeedManifestError::Validation(format!(
            "Windows namespace path must be absolute: {}",
            path.display()
        )));
    }
    let units = path.as_os_str().encode_wide().collect::<Vec<_>>();
    if units.contains(&0) {
        return Err(SeedManifestError::Validation(
            "Windows namespace path contains an interior NUL".to_string(),
        ));
    }
    let slash = u16::from(b'\\');
    let question = u16::from(b'?');
    let mut verbatim = if units.starts_with(&[slash, slash, question, slash]) {
        units
    } else if units.starts_with(&[slash, slash]) {
        let mut prefixed = "\\\\?\\UNC\\".encode_utf16().collect::<Vec<_>>();
        prefixed.extend_from_slice(&units[2..]);
        prefixed
    } else {
        let mut prefixed = "\\\\?\\".encode_utf16().collect::<Vec<_>>();
        prefixed.extend_from_slice(&units);
        prefixed
    };
    verbatim.push(0);
    Ok(verbatim)
}

#[cfg(windows)]
pub(crate) fn reopen_windows_zero_access(
    writer: &File,
    path: &Path,
) -> Result<OpenedRegularFile, SeedManifestError> {
    use std::os::windows::io::{AsRawHandle, FromRawHandle};
    use windows_sys::Win32::Foundation::{HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{
        ReOpenFile, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE, FILE_SHARE_READ,
        FILE_SHARE_WRITE,
    };

    let handle = unsafe {
        ReOpenFile(
            writer.as_raw_handle() as HANDLE,
            0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            FILE_FLAG_OPEN_REPARSE_POINT,
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(SeedManifestError::TempFile {
            operation: "ReOpenFile zero-access witness",
            path: path.to_path_buf(),
            source: io::Error::last_os_error(),
        });
    }
    let file = unsafe { File::from_raw_handle(handle as _) };
    OpenedRegularFile::from_file(path, file)
}

#[cfg(windows)]
pub(crate) fn inspect_windows_path(
    path: &Path,
    expected_identity: FileIdentity,
    expected_bytes: &[u8],
) -> WindowsPathState {
    let file = match open_regular_no_follow(path, false, false) {
        Ok(file) => file,
        Err(error) if is_exact_not_found(&error) => return WindowsPathState::NotFound,
        Err(error) => {
            return WindowsPathState::InspectionError {
                raw_error: error.raw_os_error().and_then(|raw| u32::try_from(raw).ok()),
            }
        }
    };
    let mut opened = match OpenedRegularFile::from_file(path, file) {
        Ok(opened) => opened,
        Err(SeedManifestError::UnsafePath { reason, .. }) => {
            return WindowsPathState::Unsafe { reason }
        }
        Err(error) => {
            return WindowsPathState::InspectionError {
                raw_error: match error {
                    SeedManifestError::Io { source, .. }
                    | SeedManifestError::TempFile { source, .. } => source
                        .raw_os_error()
                        .and_then(|raw| u32::try_from(raw).ok()),
                    _ => None,
                },
            }
        }
    };
    let bytes_match = match stream_matches(&mut opened.file, expected_bytes) {
        Ok(matches) => matches,
        Err(error) => {
            return WindowsPathState::InspectionError {
                raw_error: error.raw_os_error().and_then(|raw| u32::try_from(raw).ok()),
            }
        }
    };
    if opened.identity == expected_identity {
        WindowsPathState::Same {
            identity: opened.identity,
            links: opened.links,
            length: opened.length,
            bytes_match,
        }
    } else {
        WindowsPathState::Different {
            identity: opened.identity,
            links: opened.links,
            length: opened.length,
            bytes_match,
        }
    }
}

#[cfg(windows)]
pub(crate) fn open_validated_windows_final(
    path: &Path,
    expected_identity: FileIdentity,
    expected_bytes: &[u8],
) -> Result<OpenedRegularFile, WindowsPathState> {
    let file = match open_regular_no_follow(path, false, false) {
        Ok(file) => file,
        Err(error) if is_exact_not_found(&error) => return Err(WindowsPathState::NotFound),
        Err(error) => {
            return Err(WindowsPathState::InspectionError {
                raw_error: error.raw_os_error().and_then(|raw| u32::try_from(raw).ok()),
            })
        }
    };
    let mut opened = match OpenedRegularFile::from_file(path, file) {
        Ok(opened) => opened,
        Err(SeedManifestError::UnsafePath { reason, .. }) => {
            return Err(WindowsPathState::Unsafe { reason })
        }
        Err(_) => return Err(WindowsPathState::InspectionError { raw_error: None }),
    };
    let bytes_match = match stream_matches(&mut opened.file, expected_bytes) {
        Ok(matches) => matches,
        Err(error) => {
            return Err(WindowsPathState::InspectionError {
                raw_error: error.raw_os_error().and_then(|raw| u32::try_from(raw).ok()),
            })
        }
    };
    if opened.identity != expected_identity || !bytes_match {
        return Err(WindowsPathState::Different {
            identity: opened.identity,
            links: opened.links,
            length: opened.length,
            bytes_match,
        });
    }
    Ok(opened)
}

#[cfg(windows)]
pub(crate) fn open_windows_delete_handle(path: &Path) -> io::Result<File> {
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
}

#[cfg(windows)]
pub(crate) fn cleanup_current_owned_temp(
    temp: &OpenedRegularFile,
    expected_bytes: Option<&[u8]>,
) -> Result<(), SeedManifestError> {
    cleanup_windows_temp_with_witness(temp, &temp.path, temp.identity, expected_bytes)
}

#[cfg(windows)]
pub(crate) fn cleanup_windows_temp_with_witness(
    witness: &OpenedRegularFile,
    path: &Path,
    expected_identity: FileIdentity,
    expected_bytes: Option<&[u8]>,
) -> Result<(), SeedManifestError> {
    use std::mem::size_of;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::{GetLastError, HANDLE};
    use windows_sys::Win32::Storage::FileSystem::{
        FileDispositionInfo, SetFileInformationByHandle, FILE_DISPOSITION_INFO,
    };

    let witness_facts =
        handle_facts(&witness.file).map_err(|source| SeedManifestError::TempFile {
            operation: "inspect current-owned temp witness",
            path: path.to_path_buf(),
            source,
        })?;
    if witness_facts.identity != expected_identity
        || !witness_facts.is_regular_file
        || witness_facts.is_reparse
    {
        return Err(SeedManifestError::UnsafePath {
            path: path.to_path_buf(),
            reason: "current-owned temp witness changed identity or type before cleanup"
                .to_string(),
        });
    }

    let delete_file =
        open_windows_delete_handle(path).map_err(|source| SeedManifestError::TempFile {
            operation: "open no-follow GENERIC_READ|DELETE cleanup handle",
            path: path.to_path_buf(),
            source,
        })?;
    let mut delete_opened = OpenedRegularFile::from_file(path, delete_file)?;
    if delete_opened.identity != expected_identity || delete_opened.links != 1 {
        return Err(SeedManifestError::UnsafePath {
            path: path.to_path_buf(),
            reason: "current-owned temp cleanup handle did not match the one-link witness"
                .to_string(),
        });
    }
    if let Some(bytes) = expected_bytes {
        if delete_opened.length != u64::try_from(bytes.len()).unwrap_or(u64::MAX)
            || !stream_matches(&mut delete_opened.file, bytes).map_err(|source| {
                SeedManifestError::TempFile {
                    operation: "verify current-owned temp bytes before handle deletion",
                    path: path.to_path_buf(),
                    source,
                }
            })?
        {
            return Err(SeedManifestError::UnsafePath {
                path: path.to_path_buf(),
                reason: "current-owned temp bytes changed before handle deletion".to_string(),
            });
        }
        let immediate = inspect_windows_path(path, expected_identity, bytes);
        if !immediate.is_exact_same(expected_identity, true) {
            return Err(SeedManifestError::UnsafePath {
                path: path.to_path_buf(),
                reason: format!(
                    "current-owned temp path changed before FileDispositionInfo: {immediate:?}"
                ),
            });
        }
    }

    let disposition = FILE_DISPOSITION_INFO { DeleteFile: 1 };
    let success = unsafe {
        SetFileInformationByHandle(
            delete_opened.file.as_raw_handle() as HANDLE,
            FileDispositionInfo,
            (&disposition as *const FILE_DISPOSITION_INFO).cast(),
            u32::try_from(size_of::<FILE_DISPOSITION_INFO>()).unwrap_or(u32::MAX),
        )
    };
    if success == 0 {
        let raw_error = unsafe { GetLastError() };
        return Err(SeedManifestError::TempFile {
            operation: "SetFileInformationByHandle(FileDispositionInfo)",
            path: path.to_path_buf(),
            source: io::Error::from_raw_os_error(i32::try_from(raw_error).unwrap_or(i32::MAX)),
        });
    }
    drop(delete_opened);
    Ok(())
}

#[cfg(windows)]
pub(crate) fn verify_windows_temp_has_only_unnamed_stream(
    path: &Path,
) -> Result<(), SeedManifestError> {
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

    let wide = absolute_verbatim_utf16(path)?;
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
        return Err(SeedManifestError::TempFile {
            operation: "FindFirstStreamW",
            path: path.to_path_buf(),
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
            return Err(SeedManifestError::UnsafePath {
                path: path.to_path_buf(),
                reason: "manifest temp contains a named data stream".to_string(),
            });
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
        return Err(SeedManifestError::TempFile {
            operation: "FindNextStreamW",
            path: path.to_path_buf(),
            source: io::Error::from_raw_os_error(i32::try_from(raw_error).unwrap_or(i32::MAX)),
        });
    }
    drop(handle);
    Ok(())
}

#[cfg(not(any(unix, windows)))]
pub(crate) fn atomic_publish_manifest(
    temp: &Path,
    canonical: &Path,
    _had_canonical: bool,
) -> Result<(), SeedManifestError> {
    std::fs::rename(temp, canonical).map_err(|source| SeedManifestError::AtomicReplace {
        operation: "rename",
        temp: temp.to_path_buf(),
        canonical: canonical.to_path_buf(),
        source,
    })
}
