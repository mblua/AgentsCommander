use super::canonicalize_for_compare;
use super::path_is_under_windows;
use super::strip_long_prefix_str;
use super::BlockerProcess;
use std::path::Path;

#[cfg(windows)]
#[derive(Debug)]
struct ProcessSnapshotEntry {
    pid: u32,
    name: String,
}

#[cfg(windows)]
struct HandleGuard(windows_sys::Win32::Foundation::HANDLE);

#[cfg(windows)]
impl HandleGuard {
    fn raw(&self) -> windows_sys::Win32::Foundation::HANDLE {
        self.0
    }
}

#[cfg(windows)]
impl Drop for HandleGuard {
    fn drop(&mut self) {
        use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};

        if !self.0.is_null() && self.0 != INVALID_HANDLE_VALUE {
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }
}

#[cfg(windows)]
#[repr(C)]
#[derive(Clone, Copy)]
struct ProcessBasicInformationRaw {
    exit_status: windows_sys::Win32::Foundation::NTSTATUS,
    peb_base_address: *mut core::ffi::c_void,
    affinity_mask: usize,
    base_priority: i32,
    unique_process_id: usize,
    inherited_from_unique_process_id: usize,
}

#[cfg(all(windows, target_pointer_width = "64"))]
#[repr(C)]
#[derive(Clone, Copy)]
struct RemotePebPrefix64 {
    reserved: [u8; 0x20],
    process_parameters: *mut core::ffi::c_void,
}

#[cfg(all(windows, target_pointer_width = "64"))]
#[repr(C)]
#[derive(Clone, Copy)]
struct RemoteUnicodeString {
    length: u16,
    maximum_length: u16,
    buffer: *mut u16,
}

#[cfg(all(windows, target_pointer_width = "64"))]
#[repr(C)]
#[derive(Clone, Copy)]
struct RemoteCurDir {
    dos_path: RemoteUnicodeString,
    handle: windows_sys::Win32::Foundation::HANDLE,
}

#[cfg(all(windows, target_pointer_width = "64"))]
#[repr(C)]
#[derive(Clone, Copy)]
struct RemoteProcessParametersPrefix {
    maximum_length: u32,
    length: u32,
    flags: u32,
    debug_flags: u32,
    console_handle: windows_sys::Win32::Foundation::HANDLE,
    console_flags: u32,
    standard_input: windows_sys::Win32::Foundation::HANDLE,
    standard_output: windows_sys::Win32::Foundation::HANDLE,
    standard_error: windows_sys::Win32::Foundation::HANDLE,
    current_directory: RemoteCurDir,
}

#[cfg(all(windows, target_pointer_width = "64"))]
#[repr(C)]
#[derive(Clone, Copy)]
struct RemotePebPrefix32 {
    reserved: [u8; 0x10],
    process_parameters: u32,
}

#[cfg(all(windows, target_pointer_width = "64"))]
#[repr(C)]
#[derive(Clone, Copy)]
struct RemoteUnicodeString32 {
    length: u16,
    maximum_length: u16,
    buffer: u32,
}

#[cfg(all(windows, target_pointer_width = "64"))]
#[repr(C)]
#[derive(Clone, Copy)]
struct RemoteCurDir32 {
    dos_path: RemoteUnicodeString32,
    handle: u32,
}

#[cfg(all(windows, target_pointer_width = "64"))]
#[repr(C)]
#[derive(Clone, Copy)]
struct RemoteProcessParametersPrefix32 {
    maximum_length: u32,
    length: u32,
    flags: u32,
    debug_flags: u32,
    console_handle: u32,
    console_flags: u32,
    standard_input: u32,
    standard_output: u32,
    standard_error: u32,
    current_directory: RemoteCurDir32,
}

#[cfg(windows)]
const MAX_CWD_BYTES: usize = 32 * 1024;

#[cfg(windows)]
pub(super) fn scan_cwd_processes_windows(wg_dir: &Path) -> Result<Vec<BlockerProcess>, String> {
    use windows_sys::Win32::System::Threading::GetCurrentProcessId;

    let canonical_wg = canonicalize_for_compare(wg_dir);
    let current_pid = unsafe { GetCurrentProcessId() };
    let mut out = Vec::new();

    if let Some(blocker) = current_process_cwd_blocker(&canonical_wg, current_pid) {
        out.push(blocker);
    }

    for process in enumerate_processes_windows()? {
        if process.pid == 0 || process.pid == current_pid {
            continue;
        }
        let Some(cwd) = read_process_cwd_windows(process.pid) else {
            continue;
        };
        let cwd_compare = canonicalize_for_compare(Path::new(&cwd));
        if path_is_under_windows(&cwd_compare, &canonical_wg) {
            out.push(BlockerProcess {
                pid: process.pid,
                name: process.name,
                cwd: Some(cwd),
                files: Vec::new(),
            });
        }
    }

    Ok(out)
}

#[cfg(windows)]
fn current_process_cwd_blocker(canonical_wg: &Path, current_pid: u32) -> Option<BlockerProcess> {
    let current_dir = std::env::current_dir().ok()?;
    let current_exe = std::env::current_exe().ok();
    current_process_cwd_blocker_from_parts(
        canonical_wg,
        current_pid,
        &current_dir,
        current_exe.as_deref(),
    )
}

#[cfg(windows)]
pub(super) fn current_process_cwd_blocker_from_parts(
    canonical_wg: &Path,
    current_pid: u32,
    current_dir: &Path,
    current_exe: Option<&Path>,
) -> Option<BlockerProcess> {
    let cwd_compare = canonicalize_for_compare(current_dir);
    if !path_is_under_windows(&cwd_compare, canonical_wg) {
        return None;
    }

    let name = current_exe
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .filter(|n| !n.is_empty())
        .unwrap_or("current process")
        .to_string();

    Some(BlockerProcess {
        pid: current_pid,
        name,
        cwd: Some(strip_long_prefix_str(&current_dir.to_string_lossy())),
        files: Vec::new(),
    })
}

#[cfg(windows)]
fn enumerate_processes_windows() -> Result<Vec<ProcessSnapshotEntry>, String> {
    use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };

    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return Err("CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS) failed".into());
    }
    let snapshot = HandleGuard(snapshot);

    let mut entry = unsafe { std::mem::zeroed::<PROCESSENTRY32W>() };
    entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;

    let mut out = Vec::new();
    let mut ok = unsafe { Process32FirstW(snapshot.raw(), &mut entry) };
    while ok != 0 {
        out.push(ProcessSnapshotEntry {
            pid: entry.th32ProcessID,
            name: nul_terminated_utf16(&entry.szExeFile),
        });
        ok = unsafe { Process32NextW(snapshot.raw(), &mut entry) };
    }

    Ok(out)
}

#[cfg(windows)]
fn nul_terminated_utf16(buf: &[u16]) -> String {
    let nul = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..nul])
}

#[cfg(windows)]
fn read_process_cwd_windows(pid: u32) -> Option<String> {
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_VM_READ,
    };

    let handle =
        unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_VM_READ, 0, pid) };
    if handle.is_null() {
        log::debug!(
            "[wg_delete_diagnostic] CWD scan: OpenProcess failed for pid {}",
            pid
        );
        return None;
    }
    let handle = HandleGuard(handle);

    read_process_cwd_from_handle(handle.raw()).or_else(|| {
        log::debug!(
            "[wg_delete_diagnostic] CWD scan: unable to read cwd for pid {}",
            pid
        );
        None
    })
}

#[cfg(all(windows, target_pointer_width = "64"))]
fn read_process_cwd_from_handle(handle: windows_sys::Win32::Foundation::HANDLE) -> Option<String> {
    if let Some(wow64_peb) = query_wow64_peb_address(handle) {
        return read_process_cwd_wow64_32(handle, wow64_peb);
    }

    let pbi = query_process_basic_info(handle)?;
    if pbi.peb_base_address.is_null() {
        return None;
    }
    let peb: RemotePebPrefix64 = read_remote_struct(handle, pbi.peb_base_address as usize)?;
    if peb.process_parameters.is_null() {
        return None;
    }
    let params: RemoteProcessParametersPrefix =
        read_remote_struct(handle, peb.process_parameters as usize)?;
    read_remote_unicode_string(handle, params.current_directory.dos_path)
}

#[cfg(all(windows, target_pointer_width = "32"))]
fn read_process_cwd_from_handle(_handle: windows_sys::Win32::Foundation::HANDLE) -> Option<String> {
    // 32-bit AC builds reading remote ProcessParameters are out of scope for
    // this release. The shipped Windows app is 64-bit, which covers the
    // reported terminal-blocker repro.
    None
}

#[cfg(windows)]
fn query_process_basic_info(
    handle: windows_sys::Win32::Foundation::HANDLE,
) -> Option<ProcessBasicInformationRaw> {
    use windows_sys::Wdk::System::Threading::{NtQueryInformationProcess, ProcessBasicInformation};

    let mut pbi = std::mem::MaybeUninit::<ProcessBasicInformationRaw>::uninit();
    let mut return_len: u32 = 0;
    let status = unsafe {
        NtQueryInformationProcess(
            handle,
            ProcessBasicInformation,
            pbi.as_mut_ptr() as *mut core::ffi::c_void,
            std::mem::size_of::<ProcessBasicInformationRaw>() as u32,
            &mut return_len,
        )
    };
    if status != 0 {
        return None;
    }
    Some(unsafe { pbi.assume_init() })
}

#[cfg(all(windows, target_pointer_width = "64"))]
fn query_wow64_peb_address(handle: windows_sys::Win32::Foundation::HANDLE) -> Option<usize> {
    use windows_sys::Wdk::System::Threading::{NtQueryInformationProcess, ProcessWow64Information};

    let mut peb_address: usize = 0;
    let mut return_len: u32 = 0;
    let status = unsafe {
        NtQueryInformationProcess(
            handle,
            ProcessWow64Information,
            &mut peb_address as *mut usize as *mut core::ffi::c_void,
            std::mem::size_of::<usize>() as u32,
            &mut return_len,
        )
    };
    if status == 0 && peb_address != 0 {
        Some(peb_address)
    } else {
        None
    }
}

#[cfg(windows)]
fn read_remote_struct<T: Copy>(
    handle: windows_sys::Win32::Foundation::HANDLE,
    address: usize,
) -> Option<T> {
    use windows_sys::Win32::System::Diagnostics::Debug::ReadProcessMemory;

    if address == 0 {
        return None;
    }
    let mut out = std::mem::MaybeUninit::<T>::uninit();
    let requested = std::mem::size_of::<T>();
    let mut bytes_read: usize = 0;
    let ok = unsafe {
        ReadProcessMemory(
            handle,
            address as *const core::ffi::c_void,
            out.as_mut_ptr() as *mut core::ffi::c_void,
            requested,
            &mut bytes_read,
        )
    };
    if ok == 0 || bytes_read != requested {
        return None;
    }
    Some(unsafe { out.assume_init() })
}

#[cfg(windows)]
fn read_remote_bytes(
    handle: windows_sys::Win32::Foundation::HANDLE,
    address: usize,
    len: usize,
) -> Option<Vec<u8>> {
    use windows_sys::Win32::System::Diagnostics::Debug::ReadProcessMemory;

    if address == 0 || len == 0 || len > MAX_CWD_BYTES {
        return None;
    }
    let mut buf = vec![0u8; len];
    let mut bytes_read: usize = 0;
    let ok = unsafe {
        ReadProcessMemory(
            handle,
            address as *const core::ffi::c_void,
            buf.as_mut_ptr() as *mut core::ffi::c_void,
            len,
            &mut bytes_read,
        )
    };
    if ok == 0 || bytes_read != len {
        return None;
    }
    Some(buf)
}

#[cfg(all(windows, target_pointer_width = "64"))]
fn read_process_cwd_wow64_32(
    handle: windows_sys::Win32::Foundation::HANDLE,
    peb_address: usize,
) -> Option<String> {
    let peb: RemotePebPrefix32 = read_remote_struct(handle, peb_address)?;
    if peb.process_parameters == 0 {
        return None;
    }
    let params: RemoteProcessParametersPrefix32 =
        read_remote_struct(handle, peb.process_parameters as usize)?;
    read_remote_unicode_string32(handle, params.current_directory.dos_path)
}

#[cfg(all(windows, target_pointer_width = "64"))]
fn read_remote_unicode_string(
    handle: windows_sys::Win32::Foundation::HANDLE,
    remote: RemoteUnicodeString,
) -> Option<String> {
    read_remote_utf16_path(
        handle,
        remote.buffer as usize,
        remote.length,
        remote.maximum_length,
    )
}

#[cfg(all(windows, target_pointer_width = "64"))]
fn read_remote_unicode_string32(
    handle: windows_sys::Win32::Foundation::HANDLE,
    remote: RemoteUnicodeString32,
) -> Option<String> {
    read_remote_utf16_path(
        handle,
        remote.buffer as usize,
        remote.length,
        remote.maximum_length,
    )
}

#[cfg(windows)]
fn read_remote_utf16_path(
    handle: windows_sys::Win32::Foundation::HANDLE,
    buffer: usize,
    length: u16,
    maximum_length: u16,
) -> Option<String> {
    let length = usize::from(length);
    let maximum_length = usize::from(maximum_length);
    if length == 0
        || length % 2 != 0
        || maximum_length % 2 != 0
        || length > maximum_length
        || length > MAX_CWD_BYTES
        || maximum_length > MAX_CWD_BYTES + 2
        || buffer == 0
    {
        return None;
    }

    let bytes = read_remote_bytes(handle, buffer, length)?;
    let wide: Vec<u16> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|b| u16::from_le_bytes([b[0], b[1]]))
        .collect();
    let cwd = strip_long_prefix_str(String::from_utf16_lossy(&wide).trim_end_matches('\0'));
    if cwd.is_empty() {
        None
    } else {
        Some(cwd)
    }
}
