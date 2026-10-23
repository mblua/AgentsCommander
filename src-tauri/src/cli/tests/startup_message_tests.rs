use super::*;

#[test]
fn startup_message_destination_covers_all_handle_states() {
    assert_eq!(
        startup_message_destination(true, true),
        StartupMessageDestination::NativeDialog
    );
    assert_eq!(
        startup_message_destination(false, true),
        StartupMessageDestination::Stdout
    );
    assert_eq!(
        startup_message_destination(true, false),
        StartupMessageDestination::Stderr
    );
    assert_eq!(
        startup_message_destination(false, false),
        StartupMessageDestination::Stderr
    );
}

#[cfg(target_os = "windows")]
#[test]
fn null_and_unknown_standard_handles_are_invalid() {
    use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;

    assert!(standard_handle_is_invalid(std::ptr::null_mut()));
    assert!(standard_handle_is_invalid(INVALID_HANDLE_VALUE));
}

#[cfg(target_os = "windows")]
#[test]
fn pipe_and_file_standard_handles_are_valid() {
    use std::os::windows::io::AsRawHandle;
    use std::process::{Command, Stdio};

    let mut child = Command::new("cmd.exe")
        .args(["/D", "/C", "exit 0"])
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn child with piped stdout");
    let pipe = child.stdout.as_ref().expect("piped child stdout");
    assert!(!standard_handle_is_invalid(pipe.as_raw_handle()));
    child.wait().expect("wait for piped child");

    let file = tempfile::tempfile().expect("create temporary file");
    assert!(!standard_handle_is_invalid(file.as_raw_handle()));
}
