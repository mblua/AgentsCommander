use super::{
    SettingsSaveCause, SettingsSaveError, SettingsSaveLegacyOutward, SettingsSaveReason,
    SettingsSaveStage,
};
use crate::config::instance_artifacts::SETTINGS_LOCK_FILE_NAME;
use crate::filesystem::settings_file;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

// Diagnostic transport is prepared before kernel ownership. The worker is the
// only place that formats or writes logs; an owner never waits for its sink.
pub(crate) const SETTINGS_LOCK_DEBUG_CAPACITY: usize = 256;
pub(crate) const SETTINGS_LOCK_DEBUG_PATH_BYTES: usize = 4096;
static SETTINGS_LOCK_DEBUG_TRANSPORT: OnceLock<
    Option<std::sync::mpsc::SyncSender<SettingsLockDebugEvent>>,
> = OnceLock::new();
static SETTINGS_LOCK_DEBUG_INSTANCE: OnceLock<uuid::Uuid> = OnceLock::new();
static SETTINGS_LOCK_DEBUG_ATTEMPT: AtomicU64 = AtomicU64::new(0);
static SETTINGS_LOCK_DEBUG_LOST: AtomicU64 = AtomicU64::new(0);

#[cfg(test)]
pub(crate) fn settings_lock_debug_transport_initialized() -> bool {
    SETTINGS_LOCK_DEBUG_TRANSPORT.get().is_some()
}

#[cfg(test)]
pub(crate) fn settings_lock_debug_sender(
) -> Option<&'static std::sync::mpsc::SyncSender<SettingsLockDebugEvent>> {
    SETTINGS_LOCK_DEBUG_TRANSPORT.get().and_then(Option::as_ref)
}

#[cfg(test)]
pub(crate) fn settings_lock_debug_lost_count() -> u64 {
    SETTINGS_LOCK_DEBUG_LOST.load(Ordering::Relaxed)
}

#[cfg(test)]
pub(crate) fn settings_lock_debug_set_attempt(value: u64) {
    SETTINGS_LOCK_DEBUG_ATTEMPT.store(value, Ordering::Relaxed);
}

pub(crate) struct SettingsLockDebugContext {
    pub(crate) instance: uuid::Uuid,
    pub(crate) pid: u32,
    pub(crate) attempt: u64,
    pub(crate) operation: &'static str,
    pub(crate) caller: &'static std::panic::Location<'static>,
    pub(crate) thread: std::thread::ThreadId,
    pub(crate) target: String,
    pub(crate) sidecar: String,
    pub(crate) path_truncated: bool,
}

pub(crate) struct SettingsLockDebugEvent {
    pub(crate) context: Arc<SettingsLockDebugContext>,
    pub(crate) phase: &'static str,
    pub(crate) utc: chrono::DateTime<chrono::Utc>,
    pub(crate) thread: std::thread::ThreadId,
    pub(crate) preparation_ms: Option<f64>,
    pub(crate) wait_ms: Option<f64>,
    pub(crate) hold_ms: Option<f64>,
    pub(crate) reason: Option<&'static str>,
    pub(crate) raw_os_error: Option<i32>,
}

pub(crate) struct SettingsLockDebugAttempt {
    pub(crate) context: Arc<SettingsLockDebugContext>,
    pub(crate) sender: std::sync::mpsc::SyncSender<SettingsLockDebugEvent>,
    pub(crate) started: std::time::Instant,
    pub(crate) retry_started: Option<std::time::Instant>,
    pub(crate) acquired_at: Option<std::time::Instant>,
}

// Bound the escaped representation, including quotes, without splitting UTF-8.
// A truncated prefix is intentionally not a complete identity for a lock.
pub(crate) fn settings_lock_debug_path(path: &Path) -> (String, bool) {
    let mut escaped = String::with_capacity(SETTINGS_LOCK_DEBUG_PATH_BYTES);
    escaped.push('"');
    for character in path.to_string_lossy().chars() {
        let piece = match character {
            '"' => "\\\"".to_string(),
            '\\' => "\\\\".to_string(),
            '\n' => "\\n".to_string(),
            '\r' => "\\r".to_string(),
            '\t' => "\\t".to_string(),
            c if c.is_control() => format!("\\u{:04x}", c as u32),
            c => c.to_string(),
        };
        if escaped.len() + piece.len() + 1 > SETTINGS_LOCK_DEBUG_PATH_BYTES {
            escaped.push('"');
            return (escaped, true);
        }
        escaped.push_str(&piece);
    }
    escaped.push('"');
    (escaped, false)
}

pub(crate) fn settings_lock_debug_transport(
    slot: &OnceLock<Option<std::sync::mpsc::SyncSender<SettingsLockDebugEvent>>>,
    enabled: bool,
    spawn: impl FnOnce(std::sync::mpsc::Receiver<SettingsLockDebugEvent>) -> std::io::Result<()>,
) -> Option<std::sync::mpsc::SyncSender<SettingsLockDebugEvent>> {
    // In particular, INFO must not freeze None before a runtime change to DEBUG.
    if !enabled {
        return None;
    }
    slot.get_or_init(|| {
        let (sender, receiver) = std::sync::mpsc::sync_channel(SETTINGS_LOCK_DEBUG_CAPACITY);
        spawn(receiver).ok().map(|()| sender)
    })
    .clone()
}

pub(crate) fn settings_lock_debug_worker(
    receiver: std::sync::mpsc::Receiver<SettingsLockDebugEvent>,
) {
    for event in receiver {
        #[cfg(test)]
        settings_lock_debug_test_before_emit(&event);
        settings_lock_debug_emit(&event);
        #[cfg(test)]
        settings_lock_debug_test_after_emit(&event);
    }
}

#[cfg(test)]
pub(crate) struct SettingsLockDebugTestHooks {
    pub(crate) gate_phase: &'static str,
    pub(crate) gate_once: std::sync::atomic::AtomicBool,
    pub(crate) gated: std::sync::mpsc::Sender<()>,
    pub(crate) resume: std::sync::Mutex<std::sync::mpsc::Receiver<()>>,
    pub(crate) processed: std::sync::mpsc::Sender<serde_json::Value>,
}

#[cfg(test)]
pub(crate) static SETTINGS_LOCK_DEBUG_TEST_HOOKS: OnceLock<SettingsLockDebugTestHooks> =
    OnceLock::new();

#[cfg(test)]
thread_local! {
    pub(crate) static SETTINGS_LOCK_DEBUG_TEST_UNLOCK_ERROR: std::cell::Cell<bool> =
        const { std::cell::Cell::new(false) };
}

#[cfg(test)]
pub(crate) fn settings_lock_debug_test_before_emit(event: &SettingsLockDebugEvent) {
    let Some(hooks) = SETTINGS_LOCK_DEBUG_TEST_HOOKS.get() else {
        return;
    };
    if event.phase == hooks.gate_phase && hooks.gate_once.swap(false, Ordering::Relaxed) {
        hooks.gated.send(()).expect("gate observer disconnected");
        hooks
            .resume
            .lock()
            .unwrap()
            .recv_timeout(std::time::Duration::from_secs(20))
            .expect("gate not resumed by harness");
    }
}

#[cfg(test)]
pub(crate) fn settings_lock_debug_test_after_emit(event: &SettingsLockDebugEvent) {
    if let Some(hooks) = SETTINGS_LOCK_DEBUG_TEST_HOOKS.get() {
        let _ = hooks.processed.send(serde_json::json!({
            "phase": event.phase,
            "instance": event.context.instance.to_string(),
            "pid": event.context.pid,
            "attempt": event.context.attempt,
            "operation": event.context.operation,
            "sidecar": event.context.sidecar,
            "target": event.context.target,
            "reason": event.reason,
            "raw_os_error": event.raw_os_error,
            "wait_ms": event.wait_ms,
            "hold_ms": event.hold_ms,
            "preparation_ms": event.preparation_ms,
            "acquisition_thread": format!("{:?}", event.context.thread),
            "event_thread": format!("{:?}", event.thread),
            "event_utc": event.utc.to_rfc3339(),
            "lost_events": SETTINGS_LOCK_DEBUG_LOST.load(Ordering::Relaxed),
        }));
    }
}

pub(crate) fn settings_lock_debug_emit(event: &SettingsLockDebugEvent) {
    // Reevaluate effective app/CLI filtering after a potentially stalled sink.
    if !log::log_enabled!(target: module_path!(), log::Level::Debug) {
        return;
    }
    let context = &event.context;
    log::debug!(target: module_path!(),
        "settings-lock phase={} pid={} instance={} attempt={} operation={} origin={}:{}:{} \
            acquisition_thread={:?} event_thread={:?} target={} sidecar={} path_truncated={} \
            event_utc={} emitted_utc={} preparation_ms={:?} wait_ms={:?} hold_ms={:?} stage={} \
            reason={:?} raw_os_error={:?} handle_closed={} lost_events={}",
        event.phase, context.pid, context.instance, context.attempt,
        context.operation, context.caller.file(), context.caller.line(),
        context.caller.column(), context.thread, event.thread, context.target,
        context.sidecar, context.path_truncated, event.utc.to_rfc3339(),
        chrono::Utc::now().to_rfc3339(), event.preparation_ms, event.wait_ms,
        event.hold_ms, if event.phase == "release" { "unlock" } else { "lock_acquir\
            e" }, event.reason, event.raw_os_error,
        matches!(event.phase, "release" | "timeout" | "acquire-error"),
        SETTINGS_LOCK_DEBUG_LOST.load(Ordering::Relaxed));
}

impl SettingsLockDebugAttempt {
    pub(crate) fn prepare(
        settings_path: &Path,
        lock_name: &str,
        operation: &'static str,
        caller: &'static std::panic::Location<'static>,
    ) -> Option<Self> {
        let sender = settings_lock_debug_transport(
            &SETTINGS_LOCK_DEBUG_TRANSPORT,
            log::log_enabled!(target: module_path!(), log::Level::Debug),
            |receiver| {
                std::thread::Builder::new()
                    .name("settings-lock-debug".into())
                    .spawn(move || settings_lock_debug_worker(receiver))
                    .map(|_| ())
            },
        )?;
        let attempt = SETTINGS_LOCK_DEBUG_ATTEMPT
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                value.checked_add(1)
            })
            .ok()?
            + 1;
        let (target, target_truncated) = settings_lock_debug_path(settings_path);
        let lock_path = settings_path
            .parent()
            .unwrap_or_else(|| Path::new(""))
            .join(lock_name);
        let (sidecar, sidecar_truncated) = settings_lock_debug_path(&lock_path);
        let debug = Self {
            context: Arc::new(SettingsLockDebugContext {
                instance: *SETTINGS_LOCK_DEBUG_INSTANCE.get_or_init(uuid::Uuid::new_v4),
                pid: std::process::id(),
                attempt,
                operation,
                caller,
                thread: std::thread::current().id(),
                target,
                sidecar,
                path_truncated: target_truncated || sidecar_truncated,
            }),
            sender,
            started: std::time::Instant::now(),
            retry_started: None,
            acquired_at: None,
        };
        debug.event("acquire-start", None, None);
        Some(debug)
    }

    pub(crate) fn event(
        &self,
        phase: &'static str,
        reason: Option<&'static str>,
        raw_os_error: Option<i32>,
    ) {
        self.event_at(phase, reason, raw_os_error, std::time::Instant::now());
    }

    pub(crate) fn event_at(
        &self,
        phase: &'static str,
        reason: Option<&'static str>,
        raw_os_error: Option<i32>,
        at: std::time::Instant,
    ) {
        let event = SettingsLockDebugEvent {
            context: Arc::clone(&self.context),
            phase,
            utc: chrono::Utc::now(),
            thread: std::thread::current().id(),
            preparation_ms: (phase != "acquire-start").then(|| {
                self.retry_started
                    .unwrap_or(at)
                    .duration_since(self.started)
                    .as_secs_f64()
                    * 1000.0
            }),
            wait_ms: self.retry_started.map(|retry| {
                self.acquired_at
                    .unwrap_or(at)
                    .duration_since(retry)
                    .as_secs_f64()
                    * 1000.0
            }),
            hold_ms: self
                .acquired_at
                .filter(|_| phase == "release")
                .map(|acquired| at.duration_since(acquired).as_secs_f64() * 1000.0),
            reason,
            raw_os_error,
        };
        if self.sender.try_send(event).is_err() {
            // Saturation avoids diagnostic counter wraparound too.
            let _ = SETTINGS_LOCK_DEBUG_LOST.fetch_update(
                Ordering::Relaxed,
                Ordering::Relaxed,
                |value| Some(value.saturating_add(1)),
            );
        }
    }

    pub(crate) fn failure(&self, error: &SettingsSaveError) {
        let (reason, raw) = match &error.cause {
            SettingsSaveCause::Io(source) => ("io_error", source.raw_os_error()),
            SettingsSaveCause::Semantic(reason) => (reason.as_str(), None),
            SettingsSaveCause::Json(_) => ("json_error", None),
        };
        self.event(
            if reason == "lock_timed_out" {
                "timeout"
            } else {
                "acquire-error"
            },
            Some(reason),
            raw,
        );
    }
}

pub(crate) struct SettingsFileLock {
    pub(crate) file: Option<std::fs::File>,
    pub(crate) debug: Option<SettingsLockDebugAttempt>,
}

impl SettingsFileLock {
    #[cfg(test)]
    #[track_caller]
    pub(crate) fn acquire(
        settings_path: &Path,
        timeout: std::time::Duration,
    ) -> Result<Self, SettingsSaveError> {
        Self::acquire_contextual(settings_path, timeout, "test-lock")
    }

    #[track_caller]
    pub(crate) fn acquire_contextual(
        settings_path: &Path,
        timeout: std::time::Duration,
        operation: &'static str,
    ) -> Result<Self, SettingsSaveError> {
        Self::acquire_named(settings_path, SETTINGS_LOCK_FILE_NAME, timeout, operation)
    }

    /// The same acquisition over another stable sidecar beside the settings file.
    #[track_caller]
    pub(crate) fn acquire_named(
        settings_path: &Path,
        lock_name: &str,
        timeout: std::time::Duration,
        operation: &'static str,
    ) -> Result<Self, SettingsSaveError> {
        let mut debug = SettingsLockDebugAttempt::prepare(
            settings_path,
            lock_name,
            operation,
            std::panic::Location::caller(),
        );
        // Inner failure drops every opened file before a diagnostic is enqueued.
        let file = match Self::acquire_file(settings_path, lock_name, timeout, &mut debug) {
            Ok(file) => file,
            Err(error) => {
                if let Some(debug) = &debug {
                    debug.failure(&error);
                }
                return Err(error);
            }
        };
        Ok(Self {
            file: Some(file),
            debug,
        })
    }

    pub(crate) fn acquire_file(
        settings_path: &Path,
        lock_name: &str,
        timeout: std::time::Duration,
        debug: &mut Option<SettingsLockDebugAttempt>,
    ) -> Result<std::fs::File, SettingsSaveError> {
        let parent = settings_path.parent().ok_or_else(|| {
            SettingsSaveError::semantic(
                SettingsSaveStage::LockAcquire,
                settings_path,
                None,
                SettingsSaveReason::MissingParentDirectory,
                SettingsSaveLegacyOutward::SettingsLockUnavailable,
            )
        })?;
        settings_file::ensure_settings_directory(parent).map_err(|source| {
            SettingsSaveError::io(
                SettingsSaveStage::LockAcquire,
                settings_path,
                None,
                source,
                SettingsSaveLegacyOutward::SettingsLockUnavailable,
            )
        })?;
        crate::path_identity::verify_component_chain(parent).map_err(|_| {
            SettingsSaveError::semantic(
                SettingsSaveStage::LockAcquire,
                settings_path,
                None,
                SettingsSaveReason::LockIdentityRejected,
                SettingsSaveLegacyOutward::SettingsLockUnavailable,
            )
        })?;
        let lock_path = parent.join(lock_name);
        let file = settings_file::open_settings_lock(&lock_path).map_err(|source| {
            SettingsSaveError::io(
                SettingsSaveStage::LockAcquire,
                settings_path,
                None,
                source,
                SettingsSaveLegacyOutward::SettingsLockUnavailable,
            )
        })?;
        let metadata = settings_file::settings_handle_metadata(&file).map_err(|source| {
            SettingsSaveError::io(
                SettingsSaveStage::LockAcquire,
                settings_path,
                None,
                source,
                SettingsSaveLegacyOutward::SettingsLockUnavailable,
            )
        })?;
        if !metadata.is_file() {
            return Err(SettingsSaveError::semantic(
                SettingsSaveStage::LockAcquire,
                settings_path,
                None,
                SettingsSaveReason::LockFileNotRegular,
                SettingsSaveLegacyOutward::SettingsLockUnavailable,
            ));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if metadata.nlink() != 1 {
                return Err(SettingsSaveError::semantic(
                    SettingsSaveStage::LockAcquire,
                    settings_path,
                    None,
                    SettingsSaveReason::LockLinkCountRejected,
                    SettingsSaveLegacyOutward::SettingsLockUnavailable,
                ));
            }
            settings_file::set_private_settings_permissions(&lock_path).map_err(|source| {
                SettingsSaveError::io(
                    SettingsSaveStage::LockAcquire,
                    settings_path,
                    None,
                    source,
                    SettingsSaveLegacyOutward::SettingsLockUnavailable,
                )
            })?;
        }
        #[cfg(windows)]
        crate::path_identity::verify_regular_file(&lock_path).map_err(|_| {
            SettingsSaveError::semantic(
                SettingsSaveStage::LockAcquire,
                settings_path,
                None,
                SettingsSaveReason::LockIdentityRejected,
                SettingsSaveLegacyOutward::SettingsLockUnavailable,
            )
        })?;

        let retry_started = std::time::Instant::now();
        if let Some(debug) = debug.as_mut() {
            debug.retry_started = Some(retry_started);
        }
        let deadline = retry_started.checked_add(timeout).ok_or_else(|| {
            SettingsSaveError::semantic(
                SettingsSaveStage::LockAcquire,
                settings_path,
                None,
                SettingsSaveReason::LockDeadlineOverflow,
                SettingsSaveLegacyOutward::SettingsLockUnavailable,
            )
        })?;
        loop {
            #[cfg(any(unix, windows))]
            let locked = try_lock_settings_file(&file).map_err(|source| {
                SettingsSaveError::io(
                    SettingsSaveStage::LockAcquire,
                    settings_path,
                    None,
                    source,
                    SettingsSaveLegacyOutward::SettingsLockUnavailable,
                )
            })?;
            #[cfg(not(any(unix, windows)))]
            let locked = try_lock_settings_file(&file).map_err(|reason| {
                SettingsSaveError::semantic(
                    SettingsSaveStage::LockAcquire,
                    settings_path,
                    None,
                    reason,
                    SettingsSaveLegacyOutward::SettingsLockUnavailable,
                )
            })?;
            if locked {
                let acquired_at = std::time::Instant::now();
                if let Some(debug) = debug.as_mut() {
                    debug.acquired_at = Some(acquired_at);
                    debug.event("acquired", None, None);
                }
                return Ok(file);
            }
            if std::time::Instant::now() >= deadline {
                return Err(SettingsSaveError::semantic(
                    SettingsSaveStage::LockAcquire,
                    settings_path,
                    None,
                    SettingsSaveReason::LockTimedOut,
                    SettingsSaveLegacyOutward::SettingsLockUnavailable,
                ));
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
}

impl Drop for SettingsFileLock {
    fn drop(&mut self) {
        if let Some(file) = self.file.take() {
            let unlock_at = std::time::Instant::now();
            let result = unlock_settings_file(&file);
            #[cfg(test)]
            let result = if SETTINGS_LOCK_DEBUG_TEST_UNLOCK_ERROR.with(|flag| flag.replace(false)) {
                Err(std::io::Error::from_raw_os_error(5))
            } else {
                result
            };
            drop(file);
            if let Some(debug) = &self.debug {
                let (reason, raw) = match result {
                    Ok(()) => ("unlock_ok", None),
                    Err(error) => ("unlock_error", error.raw_os_error()),
                };
                debug.event_at("release", Some(reason), raw, unlock_at);
            }
        }
    }
}

#[cfg(any(unix, windows))]
pub(crate) fn try_lock_settings_file(file: &std::fs::File) -> std::io::Result<bool> {
    settings_file::try_lock_settings_handle(file)
}

#[cfg(any(unix, windows))]
pub(crate) fn unlock_settings_file(file: &std::fs::File) -> std::io::Result<()> {
    settings_file::unlock_settings_handle(file)
}

#[cfg(not(any(unix, windows)))]
pub(crate) fn try_lock_settings_file(_file: &std::fs::File) -> Result<bool, SettingsSaveReason> {
    Err(SettingsSaveReason::LockPlatformUnsupported)
}

#[cfg(not(any(unix, windows)))]
pub(crate) fn unlock_settings_file(_file: &std::fs::File) -> std::io::Result<()> {
    Ok(())
}
