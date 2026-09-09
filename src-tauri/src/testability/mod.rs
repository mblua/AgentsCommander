pub mod mutex_holders;
pub mod reset;
pub mod ui_automation;
pub mod window_info;
pub mod window_placement;

#[cfg(target_os = "windows")]
pub struct ProfileMutexGuard(windows_sys::Win32::Foundation::HANDLE);

#[cfg(target_os = "windows")]
impl Drop for ProfileMutexGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = windows_sys::Win32::Foundation::CloseHandle(self.0);
        }
    }
}

/// Probe the single-instance mutex for this binary identity.
///
/// Returns `(active, guard)`. `guard` is this process's own handle to the mutex on
/// both branches: when `active` is false it keeps the name reserved for the rest of
/// the reset; when `active` is true it is the reference object that
/// `mutex_holders::find_holders` compares other processes' handles against. It is
/// closed on drop either way. #1773 removed the early `CloseHandle` on the active
/// branch; `active` itself still comes only from `ERROR_ALREADY_EXISTS`.
#[cfg(target_os = "windows")]
pub fn acquire_profile_mutex_probe() -> Result<(bool, ProfileMutexGuard), String> {
    use windows_sys::Win32::Foundation::GetLastError;
    use windows_sys::Win32::System::Threading::CreateMutexW;

    const ERROR_ALREADY_EXISTS: u32 = 183;

    let mutex_name = crate::config::profile::mutex_name();
    let name: Vec<u16> = mutex_name.encode_utf16().collect();
    let handle = unsafe { CreateMutexW(std::ptr::null(), 0, name.as_ptr()) };
    if handle.is_null() {
        return Err("profile_mutex_create_failed".to_string());
    }

    let already_exists = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
    Ok((already_exists, ProfileMutexGuard(handle)))
}

#[cfg(not(target_os = "windows"))]
pub fn acquire_profile_mutex_probe() -> Result<(bool, ()), String> {
    Ok((false, ()))
}
