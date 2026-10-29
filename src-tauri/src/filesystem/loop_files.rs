//! Physical access to Loop files, directories, and replacement stages.

use std::io::{self, Write};
use std::path::{Path, PathBuf};

pub(crate) fn loop_discovery_entries(root: &Path) -> io::Result<impl Iterator<Item = PathBuf>> {
    std::fs::read_dir(root).map(|entries| entries.flatten().map(|entry| entry.path()))
}

pub(crate) fn loop_path_is_directory(path: &Path) -> bool {
    path.is_dir()
}

pub(crate) fn loop_path_is_file(path: &Path) -> bool {
    path.is_file()
}

pub(crate) fn loop_path_exists(path: &Path) -> bool {
    path.exists()
}

pub(crate) fn canonical_loop_root(path: &Path) -> io::Result<PathBuf> {
    std::fs::canonicalize(path)
}

pub(crate) fn read_loop_document(path: &Path) -> io::Result<String> {
    std::fs::read_to_string(path)
}

pub(crate) fn read_loop_bytes(path: &Path) -> io::Result<Vec<u8>> {
    std::fs::read(path)
}

pub(crate) fn create_loop_directory(path: &Path) -> io::Result<()> {
    std::fs::create_dir_all(path)
}

pub(crate) fn remove_loop_directory(path: &Path) -> io::Result<()> {
    std::fs::remove_dir_all(path)
}

pub(crate) fn remove_loop_stage(path: &Path) -> io::Result<()> {
    std::fs::remove_file(path)
}

pub(crate) fn write_loop_state_stage(path: &Path, content: &str) -> io::Result<()> {
    std::fs::write(path, content)
}

pub(crate) fn write_loop_bytes(path: &Path, content: impl AsRef<[u8]>) -> io::Result<()> {
    std::fs::write(path, content)
}

pub(crate) fn append_loop_ignore_rows(path: &Path, content: &str) -> io::Result<()> {
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .and_then(|mut file| file.write_all(content.as_bytes()))
}

pub(crate) fn append_loop_audit_line(path: &Path, line: &str) -> Result<(), String> {
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| format!("Failed to open {}: {}", path.display(), e))?;
    writeln!(file, "{}", line).map_err(|e| format!("Failed to append {}: {}", path.display(), e))
}

/// Writes `content` to `tmp_path` and replaces `config_path` with it. On any
/// failure, including a partial tmp write, the tmp file is removed.
pub(crate) fn write_config_via_tmp(
    tmp_path: &Path,
    config_path: &Path,
    content: &str,
) -> Result<(), String> {
    std::fs::write(tmp_path, content)
        .and_then(|()| replace_file_with_retry(tmp_path, config_path))
        .map_err(|e| {
            let _ = std::fs::remove_file(tmp_path);
            format!("Failed to write Loop config: {}", e)
        })
}

/// Sleeps between replace attempts: 5 attempts, at most 150 ms added.
const REPLACE_RETRY_DELAYS_MS: [u64; 4] = [10, 20, 40, 80];

/// `replace_file`, retried while the destination is transiently held open
/// (any open handle, even a fully shared reader, fails `MoveFileExW`).
/// Blocking `std::thread::sleep` on purpose: the callers are synchronous and
/// the CLI uses them too; 150 ms on an already-failing path is cheaper than
/// making the write path async.
pub(crate) fn replace_file_with_retry(src: &Path, dst: &Path) -> std::io::Result<()> {
    let mut delays = REPLACE_RETRY_DELAYS_MS.iter();
    loop {
        match replace_file(src, dst) {
            Err(e) if is_transient_replace_error(&e) => match delays.next() {
                Some(ms) => std::thread::sleep(std::time::Duration::from_millis(*ms)),
                None => return Err(e),
            },
            result => return result,
        }
    }
}

#[cfg(windows)]
pub(crate) fn is_transient_replace_error(e: &std::io::Error) -> bool {
    // ERROR_ACCESS_DENIED (what a held destination produces) and
    // ERROR_SHARING_VIOLATION.
    matches!(e.raw_os_error(), Some(5) | Some(32))
}

#[cfg(not(windows))]
pub(crate) fn is_transient_replace_error(e: &std::io::Error) -> bool {
    e.kind() == std::io::ErrorKind::PermissionDenied
}

#[cfg(windows)]
pub(crate) fn replace_file(src: &Path, dst: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };

    let src_wide: Vec<u16> = src.as_os_str().encode_wide().chain(Some(0)).collect();
    let dst_wide: Vec<u16> = dst.as_os_str().encode_wide().chain(Some(0)).collect();
    let ok = unsafe {
        MoveFileExW(
            src_wide.as_ptr(),
            dst_wide.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if ok == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(not(windows))]
pub(crate) fn replace_file(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::rename(src, dst)
}
