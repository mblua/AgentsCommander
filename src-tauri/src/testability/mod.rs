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

#[cfg(target_os = "windows")]
pub fn acquire_profile_mutex_probe() -> Result<(bool, Option<ProfileMutexGuard>), String> {
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
    if already_exists {
        unsafe {
            let _ = windows_sys::Win32::Foundation::CloseHandle(handle);
        }
        Ok((true, None))
    } else {
        Ok((false, Some(ProfileMutexGuard(handle))))
    }
}

#[cfg(not(target_os = "windows"))]
pub fn acquire_profile_mutex_probe() -> Result<(bool, Option<()>), String> {
    Ok((false, None))
}
