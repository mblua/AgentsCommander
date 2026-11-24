//! Test-only construction helpers shared by this crate's unit tests and by the
//! integration tests under `src-tauri/tests/`.
//!
//! This module is `pub` rather than `#[cfg(test)]` on purpose: integration
//! tests are separate crates and cannot see a library's `#[cfg(test)]` items,
//! so a gated module could not serve them. It is `#[doc(hidden)]` and must not
//! be referenced from any production code path.

/// Origin Tauri treats as the local app for IPC ACL checks
/// (`http://tauri.localhost/` on Windows, `tauri://localhost/` elsewhere).
#[doc(hidden)]
pub const LOCAL_IPC_ORIGIN: &str = if cfg!(windows) {
    "http://tauri.localhost/"
} else {
    "tauri://localhost/"
};
type TestBuilder = tauri::Builder<tauri::Wry>;

#[doc(hidden)]
pub fn test_builder() -> TestBuilder {
    test_builder_with_owners(crate::app_state::AppOwners::new())
}

/// A `tauri::Builder` that tests can `build()` off the main thread wherever the
/// platform allows it.
///
/// `Builder::any_thread` is declared `#[cfg(any(windows, target_os = "linux"))]`
/// in Tauri (`tauri-2.10.3/src/app.rs:1503`): macOS requires the event loop on
/// the main thread, so the method is not exposed there. Tests that only need a
/// built `App` still compile on macOS; only the off-main-thread relaxation is
/// unavailable.
#[cfg(any(windows, target_os = "linux"))]
#[doc(hidden)]
pub fn test_builder_with_owners(owners: crate::app_state::AppOwners) -> TestBuilder {
    crate::commands::session::selection_executor::install(
        tauri::Builder::default().any_thread().manage(owners),
    )
}

/// macOS counterpart. See the note above.
#[cfg(not(any(windows, target_os = "linux")))]
#[doc(hidden)]
pub fn test_builder_with_owners(owners: crate::app_state::AppOwners) -> TestBuilder {
    crate::commands::session::selection_executor::install(tauri::Builder::default().manage(owners))
}

/// Invoke type of the Tauri mock runtime (unit tests only).
#[cfg(test)]
pub(crate) type MockInvoke = tauri::ipc::Invoke<tauri::test::MockRuntime>;

/// Build a mock app from `builder`, open the `main` webview and send one IPC
/// request `cmd` with JSON `body` from the local origin (unit tests only).
#[cfg(test)]
pub(crate) fn mock_ipc_response(
    builder: tauri::Builder<tauri::test::MockRuntime>,
    cmd: &str,
    body: serde_json::Value,
) -> Result<tauri::ipc::InvokeResponseBody, serde_json::Value> {
    let app = builder
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("mock app");
    let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .expect("webview");
    tauri::test::get_ipc_response(
        &webview,
        tauri::webview::InvokeRequest {
            cmd: cmd.into(),
            callback: tauri::ipc::CallbackFn(0),
            error: tauri::ipc::CallbackFn(1),
            url: LOCAL_IPC_ORIGIN.parse().unwrap(),
            body: tauri::ipc::InvokeBody::Json(body),
            headers: Default::default(),
            invoke_key: tauri::test::INVOKE_KEY.to_string(),
        },
    )
}

/// Send `cmd` through `route`, whose fallback counts its calls and leaves the
/// invoke unhandled. Returns the IPC response and the fallback count.
#[cfg(test)]
pub(crate) fn invoke_counting_fallback<D>(
    builder: tauri::Builder<tauri::test::MockRuntime>,
    route: D,
    cmd: &str,
    body: serde_json::Value,
) -> (
    Result<tauri::ipc::InvokeResponseBody, serde_json::Value>,
    usize,
)
where
    D: Fn(MockInvoke, &dyn Fn(MockInvoke) -> bool) -> bool + Send + Sync + 'static,
{
    use std::sync::atomic::{AtomicUsize, Ordering};
    let calls = std::sync::Arc::new(AtomicUsize::new(0));
    let counted = std::sync::Arc::clone(&calls);
    let fallback = move |_invoke: MockInvoke| {
        counted.fetch_add(1, Ordering::SeqCst);
        false
    };
    let builder = builder.invoke_handler(move |invoke| route(invoke, &fallback));
    let response = mock_ipc_response(builder, cmd, body);
    (response, calls.load(Ordering::SeqCst))
}

/// Private marker `FQN|scenario` that turns a test process into a #5174 child.
#[cfg(test)]
const CASE_MARKER: &str = "AC_5174_CASE";

/// Absolute path of a fixture the parent prepared for its child (#5174).
#[cfg(test)]
pub(crate) const CASE_FIXTURE: &str = "AC_5174_FIXTURE";

/// Test name as libtest prints it: `path` from `module_path!()::name`
/// without the crate prefix.
#[cfg(test)]
pub(crate) fn case_name(path: &str) -> &str {
    path.strip_prefix("agentscommander_lib::").unwrap_or(path)
}

/// Child side of a #5174 process case. Returns the scenario when this
/// process is the child that runs `path`, and `None` in the ordinary parent.
/// A marker that names another test, an unknown scenario or a libtest
/// command line without `--exact <name>` fails the test.
#[cfg(test)]
pub(crate) fn case_child(path: &str, scenarios: &[&'static str]) -> Option<&'static str> {
    let name = case_name(path);
    let marker = std::env::var_os(CASE_MARKER)?;
    let marker = marker.to_str().expect("AC_5174_CASE is not UTF-8");
    let parts: Vec<&str> = marker.split('|').collect();
    let [fqn, scenario] = parts[..] else {
        panic!("malformed AC_5174_CASE {marker:?}");
    };
    assert_eq!(fqn, name, "AC_5174_CASE names another test");
    let args: Vec<String> = std::env::args().skip(1).collect();
    assert!(
        args.iter().any(|a| a == "--exact") && args.iter().filter(|a| *a == name).count() == 1,
        "AC_5174_CASE child without `--exact {name}`: {args:?}"
    );
    let found = scenarios.iter().find(|s| **s == scenario).copied();
    Some(found.unwrap_or_else(|| panic!("unknown AC_5174_CASE scenario {scenario:?}")))
}

/// Child receipt, printed once after every assertion of the scenario ran.
#[cfg(test)]
pub(crate) fn case_done(path: &str, scenario: &str) {
    println!("AC5174_DONE {} {scenario}", case_name(path));
}

/// Child process owned by a #5174 parent: killed and reaped on every exit.
#[cfg(test)]
struct CaseChild {
    process: std::process::Child,
    directory: Option<tempfile::TempDir>,
    reaped: bool,
}

#[cfg(test)]
impl CaseChild {
    /// Kill, then poll up to 10 s for the reap.
    fn reap(&mut self) -> bool {
        if self.reaped {
            return true;
        }
        let _ = self.process.kill();
        let started = std::time::Instant::now();
        while started.elapsed() < std::time::Duration::from_secs(10) {
            match self.process.try_wait() {
                Ok(Some(_)) => {
                    self.reaped = true;
                    return true;
                }
                Ok(None) => std::thread::sleep(std::time::Duration::from_millis(20)),
                Err(_) => return false,
            }
        }
        false
    }

    /// Fail the parent, keeping the child logs (and, without a reap, its fixtures).
    fn fail(&mut self, reason: &str) -> ! {
        let reaped = self.reap();
        let directory = self.directory.take().expect("case directory").keep();
        panic!(
            "{reason}; reaped={reaped}; child logs retained at {}",
            directory.display()
        );
    }
}

#[cfg(test)]
impl Drop for CaseChild {
    fn drop(&mut self) {
        if !self.reap() {
            if let Some(directory) = self.directory.take() {
                let _ = directory.keep();
            }
        }
    }
}

/// Parent side of a #5174 process case: run `path` again in a child process
/// with private cwd and environment, set only on its `Command` through
/// `configure`, and require the child receipt for `scenario`.
#[cfg(test)]
pub(crate) fn run_case(
    path: &str,
    scenario: &str,
    configure: impl FnOnce(&mut std::process::Command),
) {
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};
    let name = case_name(path);
    let directory = tempfile::tempdir().expect("case directory");
    let config = directory.path().join("config");
    std::fs::create_dir(&config).expect("case config");
    let mut command = Command::new(std::env::current_exe().expect("current exe"));
    command
        .args([name, "--exact", "--test-threads=1", "--show-output"])
        .args(["--format=pretty", "--color=never"])
        .env_remove("RUST_TEST_NOCAPTURE")
        .env("AGENTSCOMMANDER_CONFIG_DIR", &config)
        .env("AGENTSCOMMANDER_TEST_CONFIG_DIR", &config)
        .env(CASE_MARKER, format!("{name}|{scenario}"))
        .stdin(Stdio::null())
        .stdout(std::fs::File::create(directory.path().join("stdout.txt")).expect("stdout"))
        .stderr(std::fs::File::create(directory.path().join("stderr.txt")).expect("stderr"));
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    configure(&mut command);
    let mut owner = CaseChild {
        process: command.spawn().expect("spawn case child"),
        directory: Some(directory),
        reaped: false,
    };
    let started = Instant::now();
    let status = loop {
        match owner.process.try_wait() {
            Ok(Some(status)) => {
                owner.reaped = true;
                break status;
            }
            Ok(None) if started.elapsed() < Duration::from_secs(45) => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Ok(None) => owner.fail("case child deadline"),
            Err(_) => owner.fail("case child poll failed"),
        }
    };
    eprintln!(
        "#5174 case {name} {scenario}: parent pid {} child pid {} {status} in {:?}",
        std::process::id(),
        owner.process.id(),
        started.elapsed()
    );
    let stdout = owner
        .directory
        .as_ref()
        .map(|d| d.path().join("stdout.txt"));
    let stdout = std::fs::read_to_string(stdout.expect("case directory")).unwrap_or_default();
    let count = |line: &str| stdout.lines().filter(|l| *l == line).count();
    let summary = stdout
        .lines()
        .filter(|l| l.starts_with("test result: ok. 1 passed; 0 failed; 0 ignored;"))
        .count();
    if !status.success()
        || count(&format!("AC5174_DONE {name} {scenario}")) != 1
        || count(&format!("test {name} ... ok")) != 1
        || summary != 1
    {
        owner.fail("case child exit, receipt, result line or summary failed");
    }
    println!("AC5174_OK {name} {scenario}");
}
