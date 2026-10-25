use super::*;

const CHILD_CASE: &str = "AC_ENVIRONMENT_CHILD_CASE";
const INPUT_KEYS: &[&str] = &[
    "AC_MACHINE_OUTPUT",
    "AGENTSCOMMANDER_ROOT",
    "AGENTSCOMMANDER_ROLE_EXPERIMENT_TEST_FAKE_EXECUTOR",
    "AC_TERMINAL_SNAPSHOT_STAGE_FILE",
    "AC_UI_AUTOMATION",
    "CLAUDE_CONFIG_DIR",
    "CODEX_HOME",
    "AGENTSCOMMANDER_CONTAINER_IMAGE",
    "RUST_LOG",
    #[cfg(target_os = "linux")]
    "XDG_SESSION_TYPE",
    #[cfg(target_os = "linux")]
    "WAYLAND_DISPLAY",
    #[cfg(target_os = "linux")]
    "WAYLAND_SOCKET",
    #[cfg(target_os = "linux")]
    "XDG_RUNTIME_DIR",
    #[cfg(windows)]
    "PATH",
    "AC_SPAWN_STALL_TIMEOUT_MS",
    "AC_SPAWN_HEAD_BYTES",
    "AC_SPAWN_PAINT_FLOOR_BYTES",
    "AC_SPAWN_CONCURRENCY_WINDOW_MS",
    "AC_SPAWN_STOP_ATTRIBUTION_MS",
];
const UNICODE_VALUE: &str = "machine-ñ-東京";

fn spawn_environment_case(case: &str) {
    let current_exe = std::env::current_exe().expect("current test executable");
    let mut child = std::process::Command::new(current_exe);
    child
        .args([
            "--exact",
            "config::environment::tests::environment_child_case",
        ])
        .env(CHILD_CASE, case);
    for key in INPUT_KEYS {
        child.env_remove(key);
        match case {
            "missing" => {}
            "unicode" => {
                child.env(key, format!("{key}-{UNICODE_VALUE}"));
            }
            "non-unicode" => {
                child.env(key, non_unicode_value());
            }
            _ => panic!("unknown environment case: {case}"),
        }
    }
    let output = child.output().expect("run environment child case");
    assert!(
        output.status.success(),
        "{case}: status={}\nstdout={}\nstderr={}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[cfg(unix)]
fn non_unicode_value() -> std::ffi::OsString {
    use std::os::unix::ffi::OsStringExt;
    std::ffi::OsString::from_vec(vec![0xff])
}

#[cfg(windows)]
fn non_unicode_value() -> std::ffi::OsString {
    use std::os::windows::ffi::OsStringExt;
    std::ffi::OsString::from_wide(&[0xd800])
}

#[test]
fn machine_output_missing() {
    spawn_environment_case("missing");
}

#[test]
fn machine_output_unicode() {
    spawn_environment_case("unicode");
}

#[test]
fn machine_output_non_unicode() {
    spawn_environment_case("non-unicode");
}

#[test]
fn environment_child_case() {
    let Ok(case) = std::env::var(CHILD_CASE) else {
        return;
    };
    let inputs = [
        (INPUT_KEYS[1], harness_root()),
        (INPUT_KEYS[2], role_experiment_fake_executor()),
        (INPUT_KEYS[3], terminal_snapshot_stage_file()),
        (INPUT_KEYS[4], ui_automation_enable(INPUT_KEYS[4])),
        (INPUT_KEYS[5], container_credential_override(INPUT_KEYS[5])),
        (INPUT_KEYS[6], container_credential_override(INPUT_KEYS[6])),
        (INPUT_KEYS[7], container_image()),
        ("RUST_LOG", rust_log()),
        ("CODEX_HOME", codex_home()),
        #[cfg(target_os = "linux")]
        ("XDG_SESSION_TYPE", xdg_session_type()),
        #[cfg(target_os = "linux")]
        ("WAYLAND_DISPLAY", wayland_display()),
        #[cfg(target_os = "linux")]
        ("WAYLAND_SOCKET", wayland_socket()),
    ];
    for (key, actual) in inputs.into_iter().chain(
        INPUT_KEYS
            .iter()
            .filter(|key| key.starts_with("AC_SPAWN_"))
            .map(|key| (*key, spawn_diagnostic_value(key))),
    ) {
        let expected = match case.as_str() {
            "missing" => Err(std::env::VarError::NotPresent),
            "unicode" => Ok(format!("{key}-{UNICODE_VALUE}")),
            "non-unicode" => Err(std::env::VarError::NotUnicode(non_unicode_value())),
            _ => panic!("unknown environment child case: {case}"),
        };
        assert_eq!(actual, expected, "{key}");
    }
    let os_inputs = [
        ("AC_MACHINE_OUTPUT", machine_output_os()),
        #[cfg(target_os = "linux")]
        ("XDG_RUNTIME_DIR", xdg_runtime_dir()),
        #[cfg(windows)]
        ("PATH", inherited_path()),
    ];
    for (key, actual) in os_inputs {
        let expected = match case.as_str() {
            "missing" => None,
            "unicode" => Some(format!("{key}-{UNICODE_VALUE}").into()),
            "non-unicode" => Some(non_unicode_value()),
            _ => panic!("unknown environment child case: {case}"),
        };
        assert_eq!(actual, expected, "{key}");
    }
    match case.as_str() {
        "missing" => {
            assert_eq!(machine_output_string(), Err(std::env::VarError::NotPresent));
            assert_eq!(machine_output_os(), None);
        }
        "unicode" => {
            assert_eq!(
                machine_output_string(),
                Ok(format!("AC_MACHINE_OUTPUT-{UNICODE_VALUE}"))
            );
            assert_eq!(
                machine_output_os(),
                Some(format!("AC_MACHINE_OUTPUT-{UNICODE_VALUE}").into())
            );
        }
        "non-unicode" => {
            let expected = non_unicode_value();
            assert_eq!(
                machine_output_string(),
                Err(std::env::VarError::NotUnicode(expected.clone()))
            );
            assert_eq!(machine_output_os(), Some(expected));
        }
        _ => panic!("unknown environment child case: {case}"),
    }
}
