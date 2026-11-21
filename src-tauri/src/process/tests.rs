use super::*;

#[test]
fn current_process_is_running() {
    assert!(is_running(process::id()));
}

#[cfg(any(windows, unix))]
struct OwnedChild(process::Child);

#[cfg(any(windows, unix))]
impl Drop for OwnedChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[cfg(any(windows, unix))]
fn fixture_command(mode: &str) -> Command {
    let mut command = Command::new(std::env::current_exe().expect("test executable"));
    command
        .args([
            "--exact",
            "process::tests::owned_child_fixture",
            "--nocapture",
        ])
        .env("AC_PROCESS_TEST_CHILD", mode)
        .stdin(process::Stdio::null())
        .stdout(process::Stdio::null())
        .stderr(process::Stdio::null());
    command
}

#[cfg(any(windows, unix))]
#[test]
fn owned_child_is_running_until_reaped() {
    let mut child = OwnedChild(fixture_command("sleep").spawn().expect("spawn owned child"));
    let pid = child.0.id();
    assert!(is_running(pid));
    child.0.kill().expect("kill owned child");
    child.0.wait().expect("reap owned child");
    assert!(!is_running(pid));
}

#[cfg(any(windows, unix))]
#[test]
fn query_failure_preserves_conservative_result() {
    let mut exited = OwnedChild(
        fixture_command("exit")
            .spawn()
            .expect("spawn short-lived child"),
    );
    let pid = exited.0.id();
    assert!(exited.0.wait().expect("reap short-lived child").success());
    assert!(!is_running(pid));
    let mut command = fixture_command("query-failure");
    command.env("AC_PROCESS_TEST_PROBE_PID", pid.to_string());
    #[cfg(unix)]
    command.env("PATH", "");
    command.stderr(process::Stdio::inherit());
    let mut child = OwnedChild(command.spawn().expect("spawn query fixture"));
    let status = child.0.wait().expect("reap query fixture");
    assert!(status.success(), "query fixture failed: {status}");
}

#[cfg(any(windows, unix))]
#[test]
fn owned_child_fixture() {
    match std::env::var("AC_PROCESS_TEST_CHILD").as_deref() {
        Ok("sleep") => std::thread::sleep(std::time::Duration::from_secs(60)),
        Ok("query-failure") => {
            let pid = std::env::var("AC_PROCESS_TEST_PROBE_PID")
                .expect("probe pid")
                .parse()
                .expect("numeric parent pid");
            assert_ne!(pid, process::id());
            #[cfg(windows)]
            let _job = prevent_child_creation();
            #[cfg(windows)]
            assert!(Command::new("tasklist").output().is_err());
            assert!(is_running(pid));
        }
        _ => {}
    }
}

#[cfg(windows)]
fn prevent_child_creation() -> std::os::windows::io::OwnedHandle {
    use std::os::windows::io::{FromRawHandle, OwnedHandle};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_ACTIVE_PROCESS,
    };
    use windows_sys::Win32::System::Threading::GetCurrentProcess;

    // SAFETY: the handle is checked and owned immediately; the initialized
    // buffer has the documented size. Only this isolated fixture joins the job.
    unsafe {
        let handle = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        assert!(!handle.is_null(), "create fixture job");
        let job = OwnedHandle::from_raw_handle(handle);
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_ACTIVE_PROCESS;
        info.BasicLimitInformation.ActiveProcessLimit = 1;
        assert_ne!(
            SetInformationJobObject(
                handle,
                JobObjectExtendedLimitInformation,
                (&info as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast::<core::ffi::c_void>(),
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            ),
            0,
            "configure fixture job"
        );
        assert_ne!(
            AssignProcessToJobObject(handle, GetCurrentProcess()),
            0,
            "assign fixture job"
        );
        job
    }
}

#[test]
fn audio_sink_exit_codes_map_to_status() {
    assert_eq!(status_from_exit(Some(0)), AudioSinkStatus::Ok);
    assert_eq!(status_from_exit(Some(1)), AudioSinkStatus::Missing);
    for code in [Some(2), Some(-1), Some(127), None] {
        assert_eq!(status_from_exit(code), AudioSinkStatus::Unknown, "{code:?}");
    }
}

#[test]
fn audio_sink_status_serializes_lowercase() {
    let wire = |s| serde_json::to_string(&s).expect("serialize");
    assert_eq!(wire(AudioSinkStatus::Ok), "\"ok\"");
    assert_eq!(wire(AudioSinkStatus::Missing), "\"missing\"");
    assert_eq!(wire(AudioSinkStatus::Unknown), "\"unknown\"");
}

#[tokio::test]
async fn audio_sink_status_skips_probe_in_appimage_or_off_linux() {
    let expected = if cfg!(target_os = "linux") {
        AudioSinkStatus::Unknown
    } else {
        AudioSinkStatus::Ok
    };
    assert_eq!(audio_sink_status(true).await, expected);
}

#[test]
fn audio_sink_program_is_gst_inspect() {
    assert_eq!(GST_INSPECT, "gst-inspect-1.0");
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn audio_sink_probe_sends_the_exact_argv() {
    let stub =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/gst-inspect-stub.sh");
    assert_eq!(
        probe_audio_sink(stub.as_os_str()).await,
        AudioSinkStatus::Ok
    );
    assert_eq!(
        probe_audio_sink(std::ffi::OsStr::new("false")).await,
        AudioSinkStatus::Missing
    );
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn audio_sink_probe_spawn_error_is_unknown() {
    let missing = std::ffi::OsStr::new("ac-4843-no-such-program");
    assert_eq!(probe_audio_sink(missing).await, AudioSinkStatus::Unknown);
}
