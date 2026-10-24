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

/// #1868, #1930: the HOME name is one literal, independent of BUILD_PROFILE,
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
    // #786 T9: the probe encodes exactly this name, including the embedded
    // trailing NUL. If this shape ever changes, gui_instance_running() would
    // silently open a different name and defeat the whole safe-write design.
    let name = mutex_name();
    assert!(name.ends_with('\0'), "mutex name must be NUL-terminated");
    assert!(
        name.starts_with("Local\\AgentsCommander_SingleInstance"),
        "unexpected mutex name shape: {name:?}"
    );
}

/// #786 T9: in-process round-trip. Create the single-instance mutex under the
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
