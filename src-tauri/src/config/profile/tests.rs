use super::*;

#[test]
fn issue_1577_binary_suffix_parsing_preserves_first_underscore_behavior() {
    assert_eq!(
        binary_suffix_from_path(Path::new("agentscommander.exe")),
        None
    );
    assert_eq!(
        binary_suffix_from_path(Path::new("agentscommander_stage.exe")),
        Some("stage".to_string())
    );
    assert_eq!(
        binary_suffix_from_path(Path::new("agentscommander_stage_blue.exe")),
        Some("stage_blue".to_string())
    );
    assert_eq!(
        binary_suffix_from_path(Path::new("agentscommander_.exe")),
        Some(String::new())
    );
    assert_eq!(
        binary_suffix_from_path(Path::new("agentscommander_dev.exe")),
        Some("dev".to_string())
    );
}

/// The HOME name is one literal, independent of `BUILD_PROFILE`,
/// so the same assertions pass in debug and release focused runs. Only an
/// executable without an underscore suffix routes to it; a suffixed one
/// routes to its adjacent directory and never to HOME.
#[test]
fn issue_1850_config_dir_name_table_is_profile_independent() {
    assert_eq!(config_dir_name(), ".agentscommander");
    for unsuffixed in [
        "agentscommander.exe",
        "agentscommander",
        "ac.exe",
        "agentscommander-stage.exe",
        "agentscommander-beta",
        "bin/agentscommander",
        r"relative\dir\agentscommander.exe",
    ] {
        assert_eq!(
            binary_suffix_from_path(Path::new(unsuffixed)),
            None,
            "{unsuffixed} must route to the HOME name"
        );
    }
    for suffixed in [
        "agentscommander_dev.exe",
        "agentscommander_stage.exe",
        "agentscommander_unknown",
        "agentscommander_.exe",
        "agentscommander_stage_blue.exe",
        "agentscommander_DEV.exe",
    ] {
        assert!(
            binary_suffix_from_path(Path::new(suffixed)).is_some(),
            "{suffixed} must route to its adjacent directory, never to HOME"
        );
    }
}

#[test]
fn mutex_name_is_nul_terminated_and_scoped() {
    // T9: the probe encodes exactly this name, including the embedded
    // trailing NUL. If this shape ever changes, gui_instance_running() would
    // silently open a different name and defeat the whole safe-write design.
    let name = mutex_name();
    assert!(name.ends_with('\0'), "mutex name must be NUL-terminated");
    assert!(
        name.starts_with("Local\\AgentsCommander_SingleInstance"),
        "unexpected mutex name shape: {name:?}"
    );
}

/// T9: in-process round-trip. Create the single-instance mutex under the
/// SAME `mutex_name()` the acquirer uses, assert the probe detects it, then
/// close it and assert it no longer detects it. Windows-only (the probe is a
/// no-op false off-Windows). Nothing else holds this name during `cargo test`
/// (the test exe's suffix scopes the name), but the negative assertion is
/// guarded on the pre-existing state to stay robust under parallel tests.
#[cfg(target_os = "windows")]
#[test]
fn gui_instance_running_probe_roundtrip() {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::CreateMutexW;

    let was_running_before = gui_instance_running();

    let name: Vec<u16> = mutex_name().encode_utf16().collect();
    // SAFETY: `name` is a NUL-terminated UTF-16 buffer valid for the call;
    // the returned handle is closed below.
    let handle = unsafe { CreateMutexW(std::ptr::null(), 0, name.as_ptr()) };
    assert!(!handle.is_null(), "CreateMutexW failed");

    // The named mutex now exists, so the OpenMutexW probe must see it.
    assert!(
        gui_instance_running(),
        "probe should detect the held single-instance mutex"
    );

    // SAFETY: `handle` is a valid mutex handle from the CreateMutexW above.
    unsafe {
        let _ = CloseHandle(handle);
    }

    // Once our handle is closed and nothing else held the name, the probe
    // must no longer detect it.
    if !was_running_before {
        assert!(
            !gui_instance_running(),
            "probe should not detect the mutex after it is released"
        );
    }
}

#[cfg(target_os = "linux")]
#[test]
fn issue_4788_lock_file_name_keeps_mutex_identity_and_case() {
    let upper = single_instance_lock_file_name("Local\\AgentsCommander_SingleInstance_FOO\0");
    let lower = single_instance_lock_file_name("Local\\AgentsCommander_SingleInstance_Foo\0");
    assert_eq!(upper, "AgentsCommander_SingleInstance_FOO.lock");
    assert_eq!(lower, "AgentsCommander_SingleInstance_Foo.lock");
    assert_eq!(
        single_instance_lock_file_name("Local\\AgentsCommander_SingleInstance_New_Dev\0"),
        "AgentsCommander_SingleInstance_New_Dev.lock"
    );
}

#[cfg(target_os = "linux")]
const WRAPPER_OUT: &str = "AC_4788_WRAPPER_OUT";

/// Child half: a no-op unless the parent set `AC_4788_WRAPPER_OUT`.
/// Writes the wrapper result, then keeps any lock it got until killed (≤60 s).
#[cfg(target_os = "linux")]
#[test]
fn issue_4788_wrapper_child() {
    let Some(out) = std::env::var_os(WRAPPER_OUT) else {
        return;
    };
    let got = try_acquire_single_instance_lock();
    std::fs::write(out, if got { "1" } else { "0" }).expect("write result");
    std::thread::sleep(std::time::Duration::from_secs(60));
}

/// The `main.rs` wrapper, across processes, with `XDG_RUNTIME_DIR`
/// set only on the children (private tempdir; parent env untouched).
#[cfg(target_os = "linux")]
#[test]
fn issue_4788_wrapper_refuses_second_process_until_first_dies() {
    use std::os::unix::fs::PermissionsExt;
    use std::process::{Child, Command, Stdio};
    use std::time::{Duration, Instant};

    struct Reaped(Child);
    impl Drop for Reaped {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700))
        .expect("chmod 0700");
    let run = |out: &str| {
        let child = Command::new(std::env::current_exe().expect("test exe"))
            .args([
                "--exact",
                "config::profile::tests::issue_4788_wrapper_child",
            ])
            .env("XDG_RUNTIME_DIR", dir.path())
            .env(WRAPPER_OUT, dir.path().join(out))
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn wrapper child");
        let child = Reaped(child);
        let path = dir.path().join(out);
        let start = Instant::now();
        let result = loop {
            let text = std::fs::read_to_string(&path).unwrap_or_default();
            if !text.is_empty() || start.elapsed() > Duration::from_secs(20) {
                break text;
            }
            std::thread::sleep(Duration::from_millis(25));
        };
        (child, result)
    };
    let (first, held) = run("first");
    assert_eq!(held, "1", "first instance acquires");
    let lock_file = dir
        .path()
        .join(single_instance_lock_file_name(mutex_name()));
    assert!(lock_file.exists(), "lock lives in XDG_RUNTIME_DIR");
    let (second, refused) = run("second");
    assert_eq!(refused, "0", "same identity refused while first runs");
    drop(second);
    drop(first);
    let (_third, again) = run("third");
    assert_eq!(again, "1", "lock released after the first dies");
}
