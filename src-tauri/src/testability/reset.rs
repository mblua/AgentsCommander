#![deny(clippy::undocumented_unsafe_blocks)]

use clap::Args;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::window_placement::TESTABLE_EXE_NAME;

const TESTABLE_CONFIG_DIR: &str = ".agentscommander_testeable";
const TESTABLE_PROJECT_DIR: &str = "agentscommander_testeable";
// Only `platform_reset_mutex_wait_adapter` reads this, and that function is
// Windows-only, so off Windows the constant is dead code and `-D warnings` fails.
#[cfg(target_os = "windows")]
const RESET_PROCESS_MUTEX_NAME: &str = "Local\\AgentsCommander.TestReset.ProcessLock.v1";
const RESET_PROCESS_MUTEX_TIMEOUT_MS: u32 = 5_000;
const RESET_MUTEX_HARNESS_CONFIG_ENV: &str = "AGENTSCOMMANDER_TEST_RESET_MUTEX_HARNESS_CONFIG_V1";
const RESET_MUTEX_HARNESS_EVENT_SENTINEL: &str =
    "AGENTSCOMMANDER_TEST_RESET_MUTEX_HARNESS_EVENT_V1";
const RESET_MUTEX_HARNESS_MAX_RECORD_BYTES: usize = 256;

const WAIT_OBJECT_0: u32 = 0;
const WAIT_ABANDONED: u32 = 0x0000_0080;
const WAIT_TIMEOUT: u32 = 0x0000_0102;

#[derive(Debug, thiserror::Error)]
#[error("{wire}")]
struct ResetCommandError {
    code: String,
    message: String,
    raw: serde_json::Value,
    wire: String,
}

impl ResetCommandError {
    fn from_wire(wire: String) -> Self {
        let parsed = serde_json::from_str::<serde_json::Value>(&wire).unwrap_or_default();
        let code = parsed
            .get("error")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("test_reset_failed")
            .to_string();
        let message = parsed
            .get("message")
            .and_then(serde_json::Value::as_str)
            .unwrap_or(&code)
            .to_string();
        let raw = parsed
            .get("raw")
            .cloned()
            .unwrap_or_else(|| serde_json::json!({}));
        Self {
            code,
            message,
            raw,
            wire,
        }
    }

    fn new(code: &str, message: &str, raw: serde_json::Value) -> Self {
        Self {
            code: code.to_string(),
            message: message.to_string(),
            wire: error_json(code, serde_json::json!({"message": message, "raw": &raw})),
            raw,
        }
    }

    fn with_cleanup(mut self, cleanup: Self) -> Self {
        let mut value =
            serde_json::from_str::<serde_json::Value>(&self.wire).unwrap_or_else(|_| {
                serde_json::json!({
                    "ok": false,
                    "error": self.code,
                    "message": self.message,
                    "raw": self.raw,
                })
            });
        if let Some(object) = value.as_object_mut() {
            object.insert("cleanupError".to_string(), cleanup.harness_value());
        }
        self.wire = serde_json::to_string(&value).unwrap_or(self.wire);
        self
    }

    fn harness_value(&self) -> serde_json::Value {
        serde_json::json!({
            "code": self.code,
            "message": self.message,
            "raw": self.raw,
        })
    }
}

#[derive(Clone)]
struct ResetMutexWaitAdapter {
    create_mutex: Arc<dyn Fn() -> Result<usize, ResetCommandError> + Send + Sync>,
    wait_for_single_object: Arc<dyn Fn(usize, u32) -> u32 + Send + Sync>,
}

#[derive(Clone)]
struct ResetMutexCleanupAdapter {
    release_mutex: Arc<dyn Fn(usize) -> Result<(), ResetCommandError> + Send + Sync>,
    close_handle: Arc<dyn Fn(usize) -> Result<(), ResetCommandError> + Send + Sync>,
}

struct ResetProcessGuard {
    handle: usize,
    owned: bool,
    handle_open: bool,
    cleanup: ResetMutexCleanupAdapter,
}

impl ResetProcessGuard {
    fn complete(mut self) -> Result<(), ResetCommandError> {
        let mut primary = None;

        if self.owned && self.handle_open {
            match (self.cleanup.release_mutex)(self.handle) {
                Ok(()) => self.owned = false,
                Err(error) => primary = Some(error),
            }
        }

        if self.handle_open {
            match (self.cleanup.close_handle)(self.handle) {
                Ok(()) => self.handle_open = false,
                Err(error) => {
                    primary = Some(match primary {
                        Some(primary) => primary.with_cleanup(error),
                        None => error,
                    });
                }
            }
        }

        match primary {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}

impl Drop for ResetProcessGuard {
    fn drop(&mut self) {
        if self.owned && self.handle_open {
            match (self.cleanup.release_mutex)(self.handle) {
                Ok(()) => self.owned = false,
                Err(error) => log::warn!("test-reset mutex fallback release failed: {error}"),
            }
        }
        if self.handle_open {
            match (self.cleanup.close_handle)(self.handle) {
                Ok(()) => self.handle_open = false,
                Err(error) => log::warn!("test-reset mutex fallback close failed: {error}"),
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ResetWaitOutcome {
    Object0,
    Abandoned,
}

impl ResetWaitOutcome {
    fn as_str(self) -> &'static str {
        match self {
            Self::Object0 => "WAIT_OBJECT_0",
            Self::Abandoned => "WAIT_ABANDONED",
        }
    }
}

struct ResetProcessAcquisition {
    guard: ResetProcessGuard,
    outcome: ResetWaitOutcome,
}

fn classify_reset_wait(
    wait_result: u32,
    timeout_ms: u32,
) -> Result<ResetWaitOutcome, ResetCommandError> {
    match wait_result {
        WAIT_OBJECT_0 => Ok(ResetWaitOutcome::Object0),
        WAIT_ABANDONED => Ok(ResetWaitOutcome::Abandoned),
        WAIT_TIMEOUT => Err(ResetCommandError::new(
            "reset_process_lock_timeout",
            "timed out waiting for test-reset mutex",
            serde_json::json!({"timeoutMs": timeout_ms}),
        )),
        other => Err(ResetCommandError::new(
            "reset_process_lock_wait_failed",
            "failed waiting for test-reset mutex",
            serde_json::json!({"timeoutMs": timeout_ms, "waitResult": other}),
        )),
    }
}

fn acquire_reset_process_guard_with<F>(
    timeout_ms: u32,
    wait_adapter: &ResetMutexWaitAdapter,
    cleanup_adapter: ResetMutexCleanupAdapter,
    on_wait_started: F,
) -> Result<ResetProcessAcquisition, ResetCommandError>
where
    F: FnOnce() -> Result<(), ResetCommandError>,
{
    let handle = (wait_adapter.create_mutex)()?;
    let mut guard = ResetProcessGuard {
        handle,
        owned: false,
        handle_open: true,
        cleanup: cleanup_adapter,
    };

    if let Err(primary) = on_wait_started() {
        return Err(match guard.complete() {
            Ok(()) => primary,
            Err(cleanup) => primary.with_cleanup(cleanup),
        });
    }

    let wait_result = (wait_adapter.wait_for_single_object)(handle, timeout_ms);
    match classify_reset_wait(wait_result, timeout_ms) {
        Ok(outcome) => {
            guard.owned = true;
            Ok(ResetProcessAcquisition { guard, outcome })
        }
        Err(primary) => Err(match guard.complete() {
            Ok(()) => primary,
            Err(cleanup) => primary.with_cleanup(cleanup),
        }),
    }
}

fn no_op_reset_wait_started() -> Result<(), ResetCommandError> {
    Ok(())
}

fn acquire_reset_process_guard() -> Result<ResetProcessAcquisition, ResetCommandError> {
    let wait_adapter = platform_reset_mutex_wait_adapter();
    acquire_reset_process_guard_with(
        RESET_PROCESS_MUTEX_TIMEOUT_MS,
        &wait_adapter,
        platform_reset_mutex_cleanup_adapter(),
        no_op_reset_wait_started,
    )
}

fn finish_reset_operation<T>(
    result: Result<T, ResetCommandError>,
    guard: ResetProcessGuard,
) -> Result<T, ResetCommandError> {
    match (result, guard.complete()) {
        (Ok(value), Ok(())) => Ok(value),
        (Ok(_), Err(cleanup)) => Err(cleanup),
        (Err(primary), Ok(())) => Err(primary),
        (Err(primary), Err(cleanup)) => Err(primary.with_cleanup(cleanup)),
    }
}

#[cfg(target_os = "windows")]
mod reset_mutex_win32 {
    use std::ffi::c_void;

    // SAFETY: each signature below is the kernel32 declaration of the symbol named
    // in its `link_name`, with `HANDLE` spelled `*mut c_void` and `LPCWSTR` spelled
    // `*const u16`. The `system` ABI is the convention kernel32 exports, so calls
    // through these items are ABI-correct; every call site proves its own argument
    // validity.
    #[link(name = "kernel32")]
    unsafe extern "system" {
        #[link_name = "CreateMutexW"]
        pub fn create_mutex_w(
            mutex_attributes: *const c_void,
            initial_owner: i32,
            name: *const u16,
        ) -> *mut c_void;
        #[link_name = "WaitForSingleObject"]
        pub fn wait_for_single_object(handle: *mut c_void, milliseconds: u32) -> u32;
        #[link_name = "ReleaseMutex"]
        pub fn release_mutex(handle: *mut c_void) -> i32;
        #[link_name = "CloseHandle"]
        pub fn close_handle(handle: *mut c_void) -> i32;
        #[link_name = "GetLastError"]
        pub fn get_last_error() -> u32;
    }
}

#[cfg(target_os = "windows")]
fn platform_reset_mutex_wait_adapter() -> ResetMutexWaitAdapter {
    ResetMutexWaitAdapter {
        create_mutex: Arc::new(|| {
            let mut name: Vec<u16> = RESET_PROCESS_MUTEX_NAME.encode_utf16().collect();
            name.push(0);
            // SAFETY: `name` is a NUL-terminated UTF-16 buffer (the `push(0)` above)
            // that lives until after the call returns, and a null attributes pointer
            // selects the default security descriptor. `initial_owner = 0` means the
            // call never takes ownership, so ownership can only come from the wait
            // below. A non-null return is a handle owned by this process, and
            // `ResetProcessGuard` is the only thing that closes it.
            let handle =
                unsafe { reset_mutex_win32::create_mutex_w(std::ptr::null(), 0, name.as_ptr()) };
            if handle.is_null() {
                // SAFETY: `GetLastError` takes no arguments and only reads the calling
                // thread's last-error value, set by the failed call above. Nothing runs
                // between the two calls that could overwrite it.
                let last_error = unsafe { reset_mutex_win32::get_last_error() };
                Err(ResetCommandError::new(
                    "reset_process_lock_create_failed",
                    "failed to create or open test-reset mutex",
                    serde_json::json!({"lastError": last_error}),
                ))
            } else {
                Ok(handle as usize)
            }
        }),
        // SAFETY: `acquire_reset_process_guard_with` is the only caller, and it passes
        // back the handle this adapter's `create_mutex` just returned, still open for
        // the whole wait because the guard that owns it has not been dropped yet.
        // `WaitForSingleObject` only reads the handle, and `timeout_ms` is finite, so
        // the call cannot block forever.
        wait_for_single_object: Arc::new(|handle, timeout_ms| unsafe {
            reset_mutex_win32::wait_for_single_object(handle as *mut std::ffi::c_void, timeout_ms)
        }),
    }
}

#[cfg(target_os = "windows")]
fn platform_reset_mutex_cleanup_adapter() -> ResetMutexCleanupAdapter {
    ResetMutexCleanupAdapter {
        release_mutex: Arc::new(|handle| {
            // SAFETY: `handle` is the still-open handle held by `ResetProcessGuard`,
            // which calls this only while `owned && handle_open` and clears `owned`
            // on success, so `ReleaseMutex` runs at most once per acquisition. The
            // guard is built, completed and dropped inside a single synchronous
            // function body on both entry paths, and this module contains no `async`,
            // no `tokio` and no `spawn`, so it cannot reach another thread: the
            // release therefore runs on the same thread that took ownership in the
            // wait, which is what `ReleaseMutex` requires.
            if unsafe { reset_mutex_win32::release_mutex(handle as *mut std::ffi::c_void) } == 0 {
                // SAFETY: `GetLastError` takes no arguments and only reads the calling
                // thread's last-error value, set by the failed release above.
                let last_error = unsafe { reset_mutex_win32::get_last_error() };
                Err(ResetCommandError::new(
                    "reset_process_lock_release_failed",
                    "failed to release test-reset mutex",
                    serde_json::json!({"lastError": last_error}),
                ))
            } else {
                Ok(())
            }
        }),
        close_handle: Arc::new(|handle| {
            // SAFETY: `handle` is the handle returned by `create_mutex_w`, and
            // `ResetProcessGuard` calls this only while `handle_open` and clears the
            // flag on success, so the handle is closed at most once and never used
            // again afterwards.
            if unsafe { reset_mutex_win32::close_handle(handle as *mut std::ffi::c_void) } == 0 {
                // SAFETY: `GetLastError` takes no arguments and only reads the calling
                // thread's last-error value, set by the failed close above.
                let last_error = unsafe { reset_mutex_win32::get_last_error() };
                Err(ResetCommandError::new(
                    "reset_process_lock_close_failed",
                    "failed to close test-reset mutex handle",
                    serde_json::json!({"lastError": last_error}),
                ))
            } else {
                Ok(())
            }
        }),
    }
}

#[cfg(not(target_os = "windows"))]
fn platform_reset_mutex_wait_adapter() -> ResetMutexWaitAdapter {
    ResetMutexWaitAdapter {
        create_mutex: Arc::new(|| Ok(1)),
        wait_for_single_object: Arc::new(|_, _| WAIT_OBJECT_0),
    }
}

#[cfg(not(target_os = "windows"))]
fn platform_reset_mutex_cleanup_adapter() -> ResetMutexCleanupAdapter {
    ResetMutexCleanupAdapter {
        release_mutex: Arc::new(|_| Ok(())),
        close_handle: Arc::new(|_| Ok(())),
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
enum ResetMutexHarnessRole {
    Holder,
    Contender,
}

impl ResetMutexHarnessRole {
    fn as_str(self) -> &'static str {
        match self {
            Self::Holder => "holder",
            Self::Contender => "contender",
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ResetMutexHarnessConfig {
    version: u32,
    role: ResetMutexHarnessRole,
    nonce: String,
    timeout_ms: u32,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ResetMutexHarnessCommand {
    version: u32,
    role: ResetMutexHarnessRole,
    nonce: String,
    phase: String,
}

fn parse_reset_mutex_harness_config(
    raw: &str,
) -> Result<ResetMutexHarnessConfig, ResetCommandError> {
    if raw.is_empty()
        || raw.len() > RESET_MUTEX_HARNESS_MAX_RECORD_BYTES
        || raw.as_bytes().contains(&0)
        || raw.trim() != raw
    {
        return Err(ResetCommandError::new(
            "reset_mutex_harness_config_invalid",
            "invalid test-reset mutex harness configuration",
            serde_json::json!({"reason": "framing"}),
        ));
    }
    let config: ResetMutexHarnessConfig = serde_json::from_str(raw).map_err(|error| {
        ResetCommandError::new(
            "reset_mutex_harness_config_invalid",
            "invalid test-reset mutex harness configuration",
            serde_json::json!({"reason": error.to_string()}),
        )
    })?;
    let nonce_valid = (1..=64).contains(&config.nonce.len())
        && config
            .nonce
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-');
    if config.version != 1 || !nonce_valid || !matches!(config.timeout_ms, 100 | 5_000) {
        return Err(ResetCommandError::new(
            "reset_mutex_harness_config_invalid",
            "invalid test-reset mutex harness configuration",
            serde_json::json!({"reason": "value"}),
        ));
    }
    Ok(config)
}

fn emit_reset_mutex_harness_event(
    config: &ResetMutexHarnessConfig,
    phase: &str,
    value: serde_json::Value,
) -> Result<(), ResetCommandError> {
    use std::io::Write;

    let event = serde_json::json!({
        "version": 1,
        "nonce": config.nonce,
        "role": config.role.as_str(),
        "phase": phase,
        "value": value,
    });
    let line = serde_json::to_string(&event).map_err(|error| {
        ResetCommandError::new(
            "reset_mutex_harness_event_failed",
            "failed to serialize test-reset mutex harness event",
            serde_json::json!({"message": error.to_string()}),
        )
    })?;
    let stdout = std::io::stdout();
    let mut lock = stdout.lock();
    writeln!(lock, "{RESET_MUTEX_HARNESS_EVENT_SENTINEL} {line}")
        .and_then(|()| lock.flush())
        .map_err(|error| {
            ResetCommandError::new(
                "reset_mutex_harness_event_failed",
                "failed to write test-reset mutex harness event",
                serde_json::json!({"message": error.to_string()}),
            )
        })
}

fn read_reset_mutex_harness_command<R: std::io::Read>(
    reader: &mut R,
    config: &ResetMutexHarnessConfig,
    expected_phase: &str,
) -> Result<(), ResetCommandError> {
    let mut record = Vec::with_capacity(RESET_MUTEX_HARNESS_MAX_RECORD_BYTES + 1);
    let mut byte = [0_u8; 1];
    let mut complete = false;
    for _ in 0..=RESET_MUTEX_HARNESS_MAX_RECORD_BYTES {
        match reader.read(&mut byte) {
            Ok(0) => break,
            Ok(_) => {
                record.push(byte[0]);
                if byte[0] == b'\n' {
                    complete = true;
                    break;
                }
            }
            Err(error) => {
                return Err(ResetCommandError::new(
                    "reset_mutex_harness_command_invalid",
                    "failed reading test-reset mutex harness command",
                    serde_json::json!({"message": error.to_string()}),
                ));
            }
        }
    }
    if !complete
        || record.len() > RESET_MUTEX_HARNESS_MAX_RECORD_BYTES
        || record.contains(&0)
        || record.contains(&b'\r')
    {
        return Err(ResetCommandError::new(
            "reset_mutex_harness_command_invalid",
            "invalid test-reset mutex harness command framing",
            serde_json::json!({"expectedPhase": expected_phase}),
        ));
    }
    record.pop();
    let text = std::str::from_utf8(&record).map_err(|_| {
        ResetCommandError::new(
            "reset_mutex_harness_command_invalid",
            "test-reset mutex harness command is not UTF-8",
            serde_json::json!({"expectedPhase": expected_phase}),
        )
    })?;
    if text.trim() != text {
        return Err(ResetCommandError::new(
            "reset_mutex_harness_command_invalid",
            "invalid test-reset mutex harness command framing",
            serde_json::json!({"expectedPhase": expected_phase}),
        ));
    }
    let command: ResetMutexHarnessCommand = serde_json::from_str(text).map_err(|error| {
        ResetCommandError::new(
            "reset_mutex_harness_command_invalid",
            "invalid test-reset mutex harness command",
            serde_json::json!({"message": error.to_string(), "expectedPhase": expected_phase}),
        )
    })?;
    if command.version != 1
        || command.role != ResetMutexHarnessRole::Holder
        || command.nonce != config.nonce
        || command.phase != expected_phase
    {
        return Err(ResetCommandError::new(
            "reset_mutex_harness_command_invalid",
            "unexpected test-reset mutex harness command",
            serde_json::json!({"expectedPhase": expected_phase}),
        ));
    }
    Ok(())
}

fn require_reset_mutex_harness_eof<R: std::io::Read>(
    reader: &mut R,
) -> Result<(), ResetCommandError> {
    let mut trailing = [0_u8; 1];
    match reader.read(&mut trailing) {
        Ok(0) => Ok(()),
        Ok(_) => Err(ResetCommandError::new(
            "reset_mutex_harness_command_invalid",
            "trailing test-reset mutex harness command data",
            serde_json::json!({}),
        )),
        Err(error) => Err(ResetCommandError::new(
            "reset_mutex_harness_command_invalid",
            "failed reading test-reset mutex harness command",
            serde_json::json!({"message": error.to_string()}),
        )),
    }
}

#[derive(Debug, Args)]
pub struct TestResetArgs {
    #[arg(long)]
    pub confirm_testeable: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct TestResetOutput {
    ok: bool,
    exe_path: PathBuf,
    deleted: Vec<PathBuf>,
    missing: Vec<PathBuf>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct TestResetPlan {
    ok: bool,
    exe_path: PathBuf,
    planned_delete: Vec<PathBuf>,
}

fn execute_reset_mutex_harness(args: TestResetArgs) -> Result<i32, ResetCommandError> {
    let raw = std::env::var(RESET_MUTEX_HARNESS_CONFIG_ENV).map_err(|error| {
        ResetCommandError::new(
            "reset_mutex_harness_config_invalid",
            "invalid test-reset mutex harness configuration",
            serde_json::json!({"reason": error.to_string()}),
        )
    })?;
    let config = parse_reset_mutex_harness_config(&raw)?;
    let stdin = std::io::stdin();
    let mut stdin = stdin.lock();
    if config.role == ResetMutexHarnessRole::Contender {
        require_reset_mutex_harness_eof(&mut stdin)?;
    }

    let wait_adapter = platform_reset_mutex_wait_adapter();
    let acquisition = acquire_reset_process_guard_with(
        config.timeout_ms,
        &wait_adapter,
        platform_reset_mutex_cleanup_adapter(),
        || {
            emit_reset_mutex_harness_event(
                &config,
                "wait_started",
                serde_json::json!({"timeoutMs": config.timeout_ms}),
            )
        },
    );
    let acquisition = match acquisition {
        Ok(acquisition) => acquisition,
        Err(error) if error.code == "reset_process_lock_timeout" => {
            emit_reset_mutex_harness_event(&config, "timeout", error.harness_value())?;
            return Ok(2);
        }
        Err(error) => return Err(error),
    };

    emit_reset_mutex_harness_event(
        &config,
        "acquired",
        serde_json::json!({"waitResult": acquisition.outcome.as_str()}),
    )?;

    match config.role {
        ResetMutexHarnessRole::Holder => {
            read_reset_mutex_harness_command(&mut stdin, &config, "release")?;
            acquisition.guard.complete()?;
            emit_reset_mutex_harness_event(&config, "released", serde_json::json!({}))?;
            read_reset_mutex_harness_command(&mut stdin, &config, "exit")?;
            require_reset_mutex_harness_eof(&mut stdin)?;
        }
        ResetMutexHarnessRole::Contender => {
            let result =
                execute_reset_body(args, false, false).map_err(ResetCommandError::from_wire);
            finish_reset_operation(result, acquisition.guard)?;
            emit_reset_mutex_harness_event(&config, "released", serde_json::json!({}))?;
        }
    }

    emit_reset_mutex_harness_event(&config, "exited", serde_json::json!({"code": 0}))?;
    Ok(0)
}

pub fn execute(args: TestResetArgs) -> i32 {
    if std::env::var_os(RESET_MUTEX_HARNESS_CONFIG_ENV).is_some() {
        return match execute_reset_mutex_harness(args) {
            Ok(code) => code,
            Err(error) => {
                eprintln!("{error}");
                1
            }
        };
    }

    match execute_inner(args) {
        Ok(output) => {
            print_stdout_json(&output);
            0
        }
        Err(err) => {
            crate::cli_println!("{err}");
            eprintln!("{err}");
            1
        }
    }
}

fn execute_inner(args: TestResetArgs) -> Result<TestResetOutput, ResetCommandError> {
    let acquisition = acquire_reset_process_guard()?;
    let result = execute_reset_body(args, true, true).map_err(ResetCommandError::from_wire);
    finish_reset_operation(result, acquisition.guard)
}

fn execute_reset_body(
    args: TestResetArgs,
    enforce_testable_identity: bool,
    emit_plan: bool,
) -> Result<TestResetOutput, String> {
    if !args.confirm_testeable {
        return Err(error_json(
            "missing_confirm_testeable",
            serde_json::json!({"required": "--confirm-testeable"}),
        ));
    }

    let exe_path = std::env::current_exe().map_err(|e| {
        error_json(
            "current_exe_failed",
            serde_json::json!({"message": e.to_string()}),
        )
    })?;
    let exe_name = exe_path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    if enforce_testable_identity && exe_name != TESTABLE_EXE_NAME {
        return Err(error_json(
            "refusing_non_testeable_binary",
            serde_json::json!({"exePath": exe_path, "expected": TESTABLE_EXE_NAME}),
        ));
    }

    let exe_parent = exe_path.parent().ok_or_else(|| {
        error_json(
            "current_exe_has_no_parent",
            serde_json::json!({"exePath": exe_path}),
        )
    })?;

    let candidates = candidate_paths(exe_parent);
    for candidate in &candidates {
        validate_candidate(exe_parent, candidate).map_err(|e| {
            error_json(
                e.as_str(),
                serde_json::json!({
                    "path": candidate,
                    "exeParent": exe_parent,
                    "plannedDelete": &candidates,
                }),
            )
        })?;
    }

    let (active, _mutex_guard) =
        crate::testability::acquire_profile_mutex_probe().map_err(|e| {
            error_json(
                "profile_mutex_probe_failed",
                serde_json::json!({"message": e, "plannedDelete": &candidates}),
            )
        })?;
    if active {
        return Err(error_json(
            "testable_gui_active",
            serde_json::json!({"exePath": &exe_path, "plannedDelete": &candidates}),
        ));
    }

    if emit_plan {
        print_stdout_json(&TestResetPlan {
            ok: true,
            exe_path: exe_path.clone(),
            planned_delete: candidates.clone(),
        });
    }

    let mut deleted = Vec::new();
    let mut missing = Vec::new();
    for candidate in &candidates {
        validate_candidate(exe_parent, candidate).map_err(|e| {
            error_json(
                e.as_str(),
                serde_json::json!({
                    "path": candidate,
                    "exeParent": exe_parent,
                    "plannedDelete": &candidates,
                }),
            )
        })?;

        if candidate.exists() {
            std::fs::remove_dir_all(candidate).map_err(|e| {
                error_json(
                    "remove_dir_all_failed",
                    serde_json::json!({
                        "path": candidate,
                        "message": e.to_string(),
                        "exeParent": exe_parent,
                        "plannedDelete": &candidates,
                    }),
                )
            })?;
            deleted.push(candidate.clone());
        } else {
            missing.push(candidate.clone());
        }
    }

    Ok(TestResetOutput {
        ok: true,
        exe_path,
        deleted,
        missing,
    })
}

fn candidate_paths(exe_parent: &Path) -> Vec<PathBuf> {
    vec![
        exe_parent.join(TESTABLE_CONFIG_DIR),
        exe_parent.join(TESTABLE_PROJECT_DIR),
    ]
}

fn validate_candidate(exe_parent: &Path, candidate: &Path) -> Result<(), String> {
    let parent_ok = candidate.parent() == Some(exe_parent);
    if !parent_ok {
        return Err("reset_candidate_parent_mismatch".to_string());
    }

    let file_name = candidate.file_name().and_then(|n| n.to_str()).unwrap_or("");
    if file_name != TESTABLE_CONFIG_DIR && file_name != TESTABLE_PROJECT_DIR {
        return Err("reset_candidate_name_not_allowed".to_string());
    }

    let metadata = match std::fs::symlink_metadata(candidate) {
        Ok(metadata) => metadata,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(format!("reset_candidate_metadata_failed: {e}")),
    };

    if has_windows_reparse_point(&metadata) {
        return Err("reset_candidate_is_reparse_point".to_string());
    }
    if metadata.file_type().is_symlink() {
        return Err("reset_candidate_is_symlink".to_string());
    }
    if !metadata.is_dir() {
        return Err("reset_candidate_not_directory".to_string());
    }

    let canonical_parent = exe_parent
        .canonicalize()
        .map_err(|e| format!("reset_parent_canonicalize_failed: {e}"))?;
    let expected = canonical_parent.join(file_name);
    let actual = candidate
        .canonicalize()
        .map_err(|e| format!("reset_candidate_canonicalize_failed: {e}"))?;
    if actual != expected {
        return Err("reset_candidate_canonical_path_mismatch".to_string());
    }

    Ok(())
}

#[cfg(target_os = "windows")]
fn has_windows_reparse_point(metadata: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(target_os = "windows"))]
fn has_windows_reparse_point(_metadata: &std::fs::Metadata) -> bool {
    false
}

fn print_stdout_json<T: Serialize>(value: &T) {
    match serde_json::to_string(value) {
        Ok(json) => crate::cli_println!("{json}"),
        Err(e) => crate::cli_println!(
            "{{\"ok\":false,\"error\":\"json_serialize_failed\",\"message\":{}}}",
            serde_json::to_string(&e.to_string()).unwrap_or_else(|_| "\"unknown\"".to_string())
        ),
    }
}

fn error_json(error: &str, extra: serde_json::Value) -> String {
    let mut value = serde_json::json!({
        "ok": false,
        "error": error,
    });
    if let (Some(dst), Some(src)) = (value.as_object_mut(), extra.as_object()) {
        for (key, value) in src {
            dst.insert(key.clone(), value.clone());
        }
    }
    serde_json::to_string(&value)
        .unwrap_or_else(|_| "{\"ok\":false,\"error\":\"json_serialize_failed\"}".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_candidates_are_valid() {
        let tmp = tempfile::tempdir().unwrap();
        let candidate = tmp.path().join(TESTABLE_CONFIG_DIR);
        validate_candidate(tmp.path(), &candidate).unwrap();
    }

    #[test]
    fn file_candidate_is_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let candidate = tmp.path().join(TESTABLE_CONFIG_DIR);
        std::fs::write(&candidate, "not a dir").unwrap();
        assert_eq!(
            validate_candidate(tmp.path(), &candidate).unwrap_err(),
            "reset_candidate_not_directory"
        );
    }

    #[test]
    fn reset_mutex_wait_classification_is_closed() {
        assert_eq!(
            classify_reset_wait(WAIT_OBJECT_0, 100).unwrap(),
            ResetWaitOutcome::Object0
        );
        assert_eq!(
            classify_reset_wait(WAIT_ABANDONED, 5_000).unwrap(),
            ResetWaitOutcome::Abandoned
        );

        let timeout = classify_reset_wait(WAIT_TIMEOUT, 100).unwrap_err();
        assert_eq!(timeout.code, "reset_process_lock_timeout");
        assert_eq!(
            timeout.harness_value(),
            serde_json::json!({
                "code": "reset_process_lock_timeout",
                "message": "timed out waiting for test-reset mutex",
                "raw": {"timeoutMs": 100},
            })
        );

        let unexpected = classify_reset_wait(7, 5_000).unwrap_err();
        assert_eq!(unexpected.code, "reset_process_lock_wait_failed");
    }

    #[test]
    fn reset_mutex_harness_config_accepts_exact_role_timeout_matrix() {
        for (role, timeout_ms) in [
            ("holder", 100),
            ("holder", 5_000),
            ("contender", 100),
            ("contender", 5_000),
        ] {
            let raw = serde_json::json!({
                "version": 1,
                "role": role,
                "nonce": "abc-1",
                "timeoutMs": timeout_ms,
            })
            .to_string();
            let parsed = parse_reset_mutex_harness_config(&raw).unwrap();
            assert_eq!(parsed.role.as_str(), role);
            assert_eq!(parsed.timeout_ms, timeout_ms);
        }
    }

    #[test]
    fn reset_mutex_harness_config_rejects_non_contract_inputs() {
        let overlong_nonce = "a".repeat(65);
        let invalid = [
            r#"{"version":1,"role":"holder","nonce":"abc-1","timeoutMs":5000,"extra":1}"#
                .to_string(),
            r#"{"version":1,"role":"holder","nonce":"abc-1"}"#.to_string(),
            r#"{"version":1,"version":1,"role":"holder","nonce":"abc-1","timeoutMs":5000}"#
                .to_string(),
            "\0".to_string(),
            "{".to_string(),
            r#"{"version":1,"role":"holder","nonce":"abc-1","timeoutMs":5000} trailing"#
                .to_string(),
            format!(
                r#"{{"version":1,"role":"holder","nonce":"{overlong_nonce}","timeoutMs":5000}}"#
            ),
            r#"{"version":2,"role":"holder","nonce":"abc-1","timeoutMs":5000}"#.to_string(),
            r#"{"version":1,"role":"other","nonce":"abc-1","timeoutMs":5000}"#.to_string(),
            r#"{"version":1,"role":"holder","nonce":"ABC","timeoutMs":5000}"#.to_string(),
            r#"{"version":1,"role":"holder","nonce":"abc-1","timeoutMs":101}"#.to_string(),
        ];
        for raw in invalid {
            assert!(
                parse_reset_mutex_harness_config(&raw).is_err(),
                "unexpectedly accepted {raw:?}"
            );
        }
    }

    #[test]
    fn reset_mutex_acquire_returns_guard_and_outcome() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let waited = Arc::new(AtomicUsize::new(0));
        let released = Arc::new(AtomicUsize::new(0));
        let closed = Arc::new(AtomicUsize::new(0));
        let callback = Arc::new(AtomicUsize::new(0));
        let wait_adapter = ResetMutexWaitAdapter {
            create_mutex: Arc::new(|| Ok(41)),
            wait_for_single_object: {
                let waited = Arc::clone(&waited);
                Arc::new(move |handle, timeout_ms| {
                    assert_eq!(handle, 41);
                    assert_eq!(timeout_ms, 5_000);
                    waited.fetch_add(1, Ordering::SeqCst);
                    WAIT_ABANDONED
                })
            },
        };
        let cleanup_adapter = ResetMutexCleanupAdapter {
            release_mutex: {
                let released = Arc::clone(&released);
                Arc::new(move |_| {
                    released.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                })
            },
            close_handle: {
                let closed = Arc::clone(&closed);
                Arc::new(move |_| {
                    closed.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                })
            },
        };
        let acquisition =
            acquire_reset_process_guard_with(5_000, &wait_adapter, cleanup_adapter, {
                let callback = Arc::clone(&callback);
                move || {
                    callback.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                }
            })
            .unwrap();

        assert_eq!(acquisition.outcome, ResetWaitOutcome::Abandoned);
        acquisition.guard.complete().unwrap();
        assert_eq!(waited.load(Ordering::SeqCst), 1);
        assert_eq!(callback.load(Ordering::SeqCst), 1);
        assert_eq!(released.load(Ordering::SeqCst), 1);
        assert_eq!(closed.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn reset_mutex_timeout_closes_unowned_handle() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let released = Arc::new(AtomicUsize::new(0));
        let closed = Arc::new(AtomicUsize::new(0));
        let wait_adapter = ResetMutexWaitAdapter {
            create_mutex: Arc::new(|| Ok(42)),
            wait_for_single_object: Arc::new(|_, _| WAIT_TIMEOUT),
        };
        let cleanup_adapter = ResetMutexCleanupAdapter {
            release_mutex: {
                let released = Arc::clone(&released);
                Arc::new(move |_| {
                    released.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                })
            },
            close_handle: {
                let closed = Arc::clone(&closed);
                Arc::new(move |_| {
                    closed.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                })
            },
        };

        let error = acquire_reset_process_guard_with(
            100,
            &wait_adapter,
            cleanup_adapter,
            no_op_reset_wait_started,
        )
        .err()
        .expect("timeout error");
        assert_eq!(error.code, "reset_process_lock_timeout");
        assert_eq!(released.load(Ordering::SeqCst), 0);
        assert_eq!(closed.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn reset_mutex_complete_preserves_primary_cleanup_error() {
        let guard = ResetProcessGuard {
            handle: 43,
            owned: true,
            handle_open: true,
            cleanup: ResetMutexCleanupAdapter {
                release_mutex: Arc::new(|_| {
                    Err(ResetCommandError::new(
                        "release-primary",
                        "release failed",
                        serde_json::json!({}),
                    ))
                }),
                close_handle: Arc::new(|_| {
                    Err(ResetCommandError::new(
                        "close-secondary",
                        "close failed",
                        serde_json::json!({}),
                    ))
                }),
            },
        };

        let error = guard.complete().unwrap_err();
        assert_eq!(error.code, "release-primary");
        let wire: serde_json::Value = serde_json::from_str(&error.wire).unwrap();
        assert_eq!(
            wire["cleanupError"]["code"],
            serde_json::Value::String("close-secondary".to_string())
        );
    }

    #[test]
    fn reset_mutex_drop_is_cleanup_fallback() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let released = Arc::new(AtomicUsize::new(0));
        let closed = Arc::new(AtomicUsize::new(0));
        {
            let _guard = ResetProcessGuard {
                handle: 44,
                owned: true,
                handle_open: true,
                cleanup: ResetMutexCleanupAdapter {
                    release_mutex: {
                        let released = Arc::clone(&released);
                        Arc::new(move |_| {
                            released.fetch_add(1, Ordering::SeqCst);
                            Ok(())
                        })
                    },
                    close_handle: {
                        let closed = Arc::clone(&closed);
                        Arc::new(move |_| {
                            closed.fetch_add(1, Ordering::SeqCst);
                            Ok(())
                        })
                    },
                },
            };
        }
        assert_eq!(released.load(Ordering::SeqCst), 1);
        assert_eq!(closed.load(Ordering::SeqCst), 1);
    }
}
