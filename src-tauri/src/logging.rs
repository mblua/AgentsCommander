//! Process-wide logger initialization shared by the GUI and CLI entry points.
//!
//! Both `lib::run()` (GUI) and `main()` (CLI branch, before `cli::handle_cli`)
//! call [`init_logger`] so every `log::*` invocation reaches a single sink:
//! stderr **and** `<config_dir>/app.log`. Pre-#137-followup the CLI path
//! skipped this and silently dropped every `log::*` call (including the
//! `[brief]` audit lines), undermining plan #137 §3a's HIGH-1 mitigation.
//!
//! Idempotent via a process-wide [`OnceLock`]: calling more than once is a
//! silent no-op. Defensive only — current call sites are mutually exclusive
//! (a single process runs either the GUI path OR the CLI path, never both).
//! Without the guard, a second `env_logger::Builder::init()` would panic via
//! `log::set_logger`'s "called twice" contract.

use std::collections::VecDeque;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

static INIT: OnceLock<()> = OnceLock::new();

/// #280 §2 — size cap per `app.log` file. Above this, the file is rotated.
/// 50 MB chosen so a single file is still grep-able in a text editor while
/// keeping the combined retention cap below ~300 MB total per binary instance.
const APP_LOG_MAX_BYTES: u64 = 50 * 1024 * 1024;

/// #280 §2 — number of rotated files to keep (`app.log.1` … `app.log.<KEEP>`).
/// Combined with `APP_LOG_MAX_BYTES` this caps total log disk usage at
/// roughly `APP_LOG_MAX_BYTES * (APP_LOG_KEEP + 1)` bytes per binary instance
/// (active file + KEEP rotated). Rotation walks `(KEEP - 1) ..= 1` so the
/// highest rename is `.(KEEP - 1) → .KEEP`, atomically evicting the prior
/// `.KEEP` via `std::fs::rename`'s replace semantics — see `rotate()`.
const APP_LOG_KEEP: u32 = 5;

/// #280 §2 — log file plus a bookkeeping counter for size-based rotation.
/// `bytes` is read/written with `Relaxed` ordering: it's a best-effort cap
/// counter, not a synchronization primitive. A small over-shoot at the
/// rotation boundary is acceptable.
struct AppLogFile {
    file: Mutex<std::fs::File>,
    bytes: AtomicU64,
    path: PathBuf,
}

/// #280 §2 — rotate `path` → `path.1` and shift existing `path.<i>` to
/// `path.<i+1>`. The walk runs `(KEEP - 1) ..= 1` so the highest rename is
/// `.(KEEP - 1) → .KEEP`; the existing `.KEEP` is implicitly evicted by
/// `std::fs::rename`'s atomic-replace semantics (G-HIGH-1 fix). Caller must
/// have observed `bytes >= APP_LOG_MAX_BYTES`; we re-check under the lock
/// (G-MED-1 fix) to keep concurrent invocations idempotent — only one
/// thread actually rotates.
///
/// On any IO error inside rotation, we log to stderr (NOT `log::*` — that
/// would re-enter the format closure) and continue. Worst case: the file
/// grows somewhat past the cap before a successful rotation.
fn rotate(state: &AppLogFile) {
    let mut file_guard = match state.file.lock() {
        Ok(g) => g,
        Err(e) => e.into_inner(),
    };

    // #280 G-MED-1 — re-check the byte counter under the lock. If another
    // thread already rotated (their `bytes.store(0)` ran), nothing to do.
    if state.bytes.load(Ordering::Relaxed) < APP_LOG_MAX_BYTES {
        return;
    }

    let base = state.path.as_path();
    let parent = match base.parent() {
        Some(p) => p,
        None => {
            eprintln!(
                "[log] rotate: cannot resolve parent for {} — skipping",
                base.display()
            );
            return;
        }
    };
    let stem = match base.file_name().and_then(|n| n.to_str()) {
        Some(s) => s,
        None => {
            eprintln!(
                "[log] rotate: cannot extract file name for {} — skipping",
                base.display()
            );
            return;
        }
    };

    let numbered = |i: u32| parent.join(format!("{stem}.{i}"));

    // Walk descending from KEEP - 1 down to 1. Each rename atomically
    // replaces the destination, so the old `.<KEEP>` is dropped when the
    // first iteration renames `.<KEEP - 1>` over it (G-HIGH-1 fix).
    if APP_LOG_KEEP >= 2 {
        for i in (1..=APP_LOG_KEEP - 1).rev() {
            let from = numbered(i);
            if !from.exists() {
                continue;
            }
            let to = numbered(i + 1);
            if let Err(e) = std::fs::rename(&from, &to) {
                eprintln!(
                    "[log] rotate: failed to rename {} → {}: {} (continuing)",
                    from.display(),
                    to.display(),
                    e
                );
            }
        }
    } else {
        // KEEP == 1 (or 0): no shift needed; `.1` will be overwritten below.
        // KEEP == 0 is degenerate but harmless — `app.log` gets truncated
        // to a fresh file with no retained history.
    }

    // Shift the active file into the `.1` slot.
    if APP_LOG_KEEP >= 1 {
        let one = numbered(1);
        if let Err(e) = std::fs::rename(base, &one) {
            eprintln!(
                "[log] rotate: failed to rename {} → {}: {} — leaving active file in place",
                base.display(),
                one.display(),
                e
            );
            // Active file stays — next writes append to it; cap will be
            // breached temporarily.
            return;
        }
    } else if let Err(e) = std::fs::remove_file(base) {
        eprintln!(
            "[log] rotate: KEEP=0 and failed to remove {}: {} — leaving active file in place",
            base.display(),
            e
        );
        return;
    }

    // Open a fresh active file. `create_new(true)` so we don't inherit a
    // stale fd or content if something else raced.
    let fresh = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(base)
        .or_else(|e| {
            // Fallback: another process already created it. Open append-mode
            // so we don't truncate their content. Log to stderr for visibility.
            eprintln!(
                "[log] rotate: create_new failed for {} ({}); falling back to append",
                base.display(),
                e
            );
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(base)
        });

    match fresh {
        Ok(f) => {
            *file_guard = f;
            state.bytes.store(0, Ordering::Relaxed);
        }
        Err(e) => {
            eprintln!(
                "[log] rotate: failed to open fresh active file at {}: {}",
                base.display(),
                e
            );
            // Counter NOT reset — next writes will see "over cap" and try
            // rotating again. Active fd in `file_guard` is now stale (the
            // file on disk was renamed). Subsequent writes silently fail at
            // the OS level on Unix; on Windows the file handle remains
            // valid via the moved inode. Either way, no log is lost on
            // stderr.
        }
    }
}

/// Install the global `log` backend. Safe to call from any entry point and
/// safe to call multiple times.
///
/// Filter precedence (matches `lib::run()` pre-fix):
/// 1. `RUST_LOG` environment variable
/// 2. `settings.json::logLevel`
/// 3. Hardcoded default `"agentscommander=info"`
///
/// Sink: stderr + `<config_dir>/app.log` (append-mode; per-line writes are
/// serialized through a `Mutex` so concurrent log calls within one process
/// do not interleave bytes mid-line).
pub fn init_logger() {
    INIT.get_or_init(init_logger_inner);
}

fn init_logger_inner() {
    let log_state: Option<Arc<AppLogFile>> = crate::config::config_dir().and_then(|dir| {
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("app.log");
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .ok()
            .map(|f| {
                eprintln!("[log] file logging to {}", path.display());
                let initial_bytes = f.metadata().map(|m| m.len()).unwrap_or(0);
                Arc::new(AppLogFile {
                    file: Mutex::new(f),
                    bytes: AtomicU64::new(initial_bytes),
                    path,
                })
            })
    });

    // #280 §2.6 — first-time migration. Existing users may have multi-GB
    // app.log from before the rotation rollout; rotate immediately so this
    // run starts on a fresh file instead of appending to it.
    if let Some(ref state) = log_state {
        if state.bytes.load(Ordering::Relaxed) >= APP_LOG_MAX_BYTES {
            rotate(state);
        }
    }
    let log_state = Arc::new(log_state);

    // #93 precedence: RUST_LOG env > settings.logLevel > "agentscommander=info" default.
    // - read_log_level_only is read-only and side-effect-free: does NOT trigger
    //   migrations, auto-token-gen, or save_settings, so all log calls inside the
    //   full load_settings() flow re-fire on the post-init SettingsState construction
    //   call and are captured.
    // - from_env(Env::default()) preserves RUST_LOG_STYLE handling (color output).
    // - No floor is applied: if `resolved_filter` is malformed (e.g. user typo in
    //   settings.json::logLevel), parse_filters produces no matching directives for
    //   agentscommander* targets, and all logs from those targets are suppressed.
    //   The user-facing recovery is to fix the typo.
    let resolved_filter = std::env::var("RUST_LOG")
        .ok()
        .or_else(crate::config::settings::read_log_level_only)
        .unwrap_or_else(|| "agentscommander=info".to_string());

    env_logger::Builder::from_env(env_logger::Env::default())
        .parse_filters(&resolved_filter)
        .format({
            let log_state = Arc::clone(&log_state);
            move |buf, record| {
                let ts = chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f");
                let line = format!(
                    "{} [{}] {} — {}\n",
                    ts,
                    record.level(),
                    record.target(),
                    record.args()
                );
                // #280 — universal secret scrub before the line reaches
                // stderr or app.log. Defends against new call sites and
                // third-party crate errors that may have bypassed api.rs's
                // and voice.rs's redaction. Near-zero cost when the line
                // contains no "/bot" or "key=" substring (early-return
                // inside redact()).
                let line = crate::telegram::redact::redact(&line);
                // #264 — tee ERROR-level entries from AgentsCommander's own
                // targets into the process-wide sink for the UI error modal.
                // Placed BEFORE the `?` writes below so a failing stderr/app.log
                // write cannot skip capture (M1). MUST NOT call any log::* macro
                // — this runs inside the logger and would recurse.
                if should_capture(record) {
                    error_sink().capture(ErrorLogEntry::from_record(ts.to_string(), record));
                }
                buf.write_all(line.as_bytes())?;
                if let Some(ref state) = *log_state {
                    if let Ok(mut f) = state.file.lock() {
                        let _ = f.write_all(line.as_bytes());
                    }
                    let new_total = state
                        .bytes
                        .fetch_add(line.len() as u64, Ordering::Relaxed)
                        + line.len() as u64;
                    if new_total >= APP_LOG_MAX_BYTES {
                        rotate(state);
                    }
                }
                Ok(())
            }
        })
        .init();
}

// ===========================================================================
// #264 — ERROR-level log capture for the UI error modal.
// ===========================================================================

/// One captured ERROR-level log entry, surfaced to the UI error modal (#264).
/// Field names serialize to camelCase to match `src/shared/types.ts`.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ErrorLogEntry {
    /// Local wall-clock timestamp, e.g. "2026-05-21 15:56:11.123".
    pub timestamp: String,
    /// Level string — always "ERROR" today; kept for forward-compat + copy text.
    pub level: String,
    /// Log target (module path), e.g. "agentscommander_lib::commands::entity_creation".
    pub target: String,
    /// Full message; may contain embedded newlines (multi-line git errors etc.).
    pub message: String,
}

impl ErrorLogEntry {
    /// Build an entry from a log `Record` plus a preformatted timestamp.
    /// Factored out of the format closure so it is unit-testable with a
    /// synthetic `Record` — the closure itself cannot be invoked from a test.
    fn from_record(timestamp: String, record: &log::Record) -> Self {
        // #280 — the error modal is exposed to the UI; scrub the message
        // so a leaked bot token / Gemini key cannot reach the frontend
        // payload. Source-side redaction in api.rs / voice.rs is the
        // first line of defense; this is defense-in-depth.
        let message = crate::telegram::redact::redact(&record.args().to_string());
        Self {
            timestamp,
            level: record.level().to_string(),
            target: record.target().to_string(),
            message,
        }
    }
}

/// Whether a log record should be teed into the error-modal sink: ERROR level
/// AND a target owned by one of AgentsCommander's own crates
/// (`agentscommander_lib::…` or `agentscommander_new`). The target-prefix guard
/// keeps third-party crates' ERROR logs (`hyper`, `reqwest`, …) out of the modal
/// even if `RUST_LOG` is widened. Factored out of the format closure so the
/// single most safety-critical predicate of #264 is directly unit-testable (M3).
fn should_capture(record: &log::Record) -> bool {
    record.level() == log::Level::Error && record.target().starts_with("agentscommander")
}

/// Buffer cap. Bounds memory if ERROR entries are produced before any frontend
/// drains them (startup storm, or CLI mode where the sink is never drained).
/// Oldest entries are dropped on overflow.
const ERROR_BUFFER_CAP: usize = 200;

/// Process-wide sink that tees ERROR-level log entries to the Tauri UI layer.
///
/// `pending` is the source of truth — every captured entry lands there.
/// `notify` wakes the emit task (`spawn_error_emit_task`). The `env_logger`
/// format closure calls only `capture()`, which pushes to `pending` and then
/// `notify_one()` — both sync, log-free and panic-free. The actual
/// `error_log_event` emit runs OUTSIDE the logger, in the emit task (see §3.7).
/// The emitted event is a content-free *ping*: the frontend responds by calling
/// `drain_error_logs`, which read-and-clears `pending`. Because `capture()`
/// pushes BEFORE signalling, a drain triggered by the ping always observes the
/// entry — race-free without sequence numbers.
pub struct ErrorEventSink {
    pending: Mutex<VecDeque<ErrorLogEntry>>,
    notify: tokio::sync::Notify,
}

impl ErrorEventSink {
    fn new() -> Self {
        Self {
            pending: Mutex::new(VecDeque::new()),
            notify: tokio::sync::Notify::new(),
        }
    }

    /// Buffer an ERROR entry and wake the emit task. Runs inside the
    /// `env_logger` format closure, so it MUST NOT call any `log::*` macro and
    /// MUST NOT panic. `notify_one()` is sync, non-blocking, log-free,
    /// panic-free, and needs no tokio runtime (only the awaiting side does).
    pub fn capture(&self, entry: ErrorLogEntry) {
        {
            // into_inner() recovers a poisoned lock so one panic while holding
            // the mutex cannot permanently disable error capture.
            let mut buf = self.pending.lock().unwrap_or_else(|e| e.into_inner());
            if buf.len() >= ERROR_BUFFER_CAP {
                buf.pop_front();
            }
            buf.push_back(entry);
        }
        // Wake the emit task. Coalescing is intentional: Notify holds at most
        // one permit, so a burst of captures between wake-ups yields a single
        // ping — the frontend's read-and-clear drain collects them all anyway.
        self.notify.notify_one();
    }

    /// Read-and-clear all buffered entries. Each entry is returned exactly once.
    pub fn drain(&self) -> Vec<ErrorLogEntry> {
        let mut buf = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        Vec::from(std::mem::take(&mut *buf))
    }

    /// Await the next `capture()` signal. Used only by `spawn_error_emit_task`.
    async fn wait_for_capture(&self) {
        self.notify.notified().await;
    }
}

static ERROR_SINK: OnceLock<ErrorEventSink> = OnceLock::new();

/// Accessor for the process-wide error sink. Lazily initialized on first use.
pub fn error_sink() -> &'static ErrorEventSink {
    ERROR_SINK.get_or_init(ErrorEventSink::new)
}

/// Spawn the background task that emits the `error_log_event` ping to the UI.
/// Called once from `lib::run()`'s `setup()` hook (§5.3.a).
///
/// The task waits on the sink's `Notify` and emits a content-free ping each
/// time `capture()` signals. Running the emit HERE — outside the `env_logger`
/// format closure — keeps the logging hot path minimal and isolates any panic
/// inside `emit()` from the arbitrary `log::error!` call site. See §3.7.
pub fn spawn_error_emit_task(app: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        loop {
            error_sink().wait_for_capture().await;
            // emit() is sync + thread-safe (idle-detector precedent, lib.rs:207);
            // a failed emit is swallowed. Running outside the logger means emit()'s
            // transitive log calls (if any) cannot re-enter the format closure.
            let _ = tauri::Emitter::emit(&app, "error_log_event", ());
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build an `ErrorLogEntry` carrying `message`; the other fields are fixed.
    fn entry(message: &str) -> ErrorLogEntry {
        ErrorLogEntry {
            timestamp: "2026-05-21 12:00:00.000".to_string(),
            level: "ERROR".to_string(),
            target: "agentscommander_lib::test".to_string(),
            message: message.to_string(),
        }
    }

    /// Run `should_capture` against a synthetic record. The `Record` (and the
    /// `format_args!` temporary it borrows) is built and consumed inside this
    /// single expression — see §7.1's lifetime note.
    fn captures(level: log::Level, target: &str) -> bool {
        should_capture(
            &log::Record::builder()
                .level(level)
                .target(target)
                .args(format_args!("synthetic message"))
                .build(),
        )
    }

    #[test]
    fn should_capture_applies_level_and_target_guard() {
        // ERROR from AgentsCommander's own targets (lib + bin) → captured.
        assert!(captures(
            log::Level::Error,
            "agentscommander_lib::commands::entity_creation"
        ));
        assert!(captures(log::Level::Error, "agentscommander_new"));
        // ERROR from a third-party crate → not captured (target guard).
        assert!(!captures(log::Level::Error, "hyper::client"));
        // Below ERROR, even from our own targets → not captured (level guard).
        assert!(!captures(log::Level::Warn, "agentscommander_lib::foo"));
        assert!(!captures(log::Level::Info, "agentscommander_lib::foo"));
    }

    #[test]
    fn from_record_copies_all_fields_and_keeps_newlines() {
        let built = ErrorLogEntry::from_record(
            "2026-05-21 12:00:00.000".to_string(),
            &log::Record::builder()
                .level(log::Level::Error)
                .target("agentscommander_lib::commands::entity_creation")
                .args(format_args!("line one\nline two"))
                .build(),
        );
        assert_eq!(built.timestamp, "2026-05-21 12:00:00.000");
        assert_eq!(built.level, "ERROR");
        assert_eq!(
            built.target,
            "agentscommander_lib::commands::entity_creation"
        );
        // The embedded newline survives verbatim (multi-line git errors etc.).
        assert_eq!(built.message, "line one\nline two");
    }

    /// #280 — the error modal payload reaches the UI; secrets must be
    /// scrubbed at `from_record` time so even a future caller that bypassed
    /// the source-side redaction in api.rs cannot leak a token into the
    /// frontend.
    #[test]
    fn from_record_redacts_telegram_token_in_message() {
        let fake_url = "error sending request for url \
            (https://api.telegram.org/bot987654321:FAKE_TOKEN_FOR_TESTING_xxxxxxxxxxxxxxx/getUpdates?offset=0)";
        let built = ErrorLogEntry::from_record(
            "2026-05-21 12:00:00.000".to_string(),
            &log::Record::builder()
                .level(log::Level::Error)
                .target("agentscommander_lib::telegram::bridge")
                .args(format_args!("Telegram poll error: {}", fake_url))
                .build(),
        );
        assert!(
            built.message.contains("/bot***/getUpdates"),
            "expected redacted shape; got: {}",
            built.message
        );
        assert!(
            !built.message.contains("FAKE_TOKEN_FOR_TESTING"),
            "token leaked into message: {}",
            built.message
        );
    }

    /// `ErrorLogEntry` crosses the Tauri IPC boundary, so the
    /// `#[serde(rename_all = "camelCase")]` rename is part of the contract with
    /// `src/shared/types.ts`. The four field names are single-word today, so the
    /// rename is a no-op — this test guards the contract against a future
    /// field rename (mirrors `rtk_sweep_result_serializes_camel_case`).
    #[test]
    fn error_log_entry_serializes_camel_case() {
        let json = serde_json::to_string(&entry("boom")).expect("serialize");
        assert!(
            json.contains("\"timestamp\""),
            "missing timestamp: {}",
            json
        );
        assert!(json.contains("\"level\""), "missing level: {}", json);
        assert!(json.contains("\"target\""), "missing target: {}", json);
        assert!(json.contains("\"message\""), "missing message: {}", json);
    }

    #[test]
    fn sink_capture_then_drain_is_read_and_clear() {
        let sink = ErrorEventSink::new();
        sink.capture(entry("only"));
        let first = sink.drain();
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].message, "only");
        // A second drain sees nothing — drain is read-and-clear.
        assert!(sink.drain().is_empty());
    }

    #[test]
    fn sink_drain_preserves_fifo_order() {
        let sink = ErrorEventSink::new();
        sink.capture(entry("first"));
        sink.capture(entry("second"));
        sink.capture(entry("third"));
        let messages: Vec<String> = sink.drain().into_iter().map(|e| e.message).collect();
        assert_eq!(messages, ["first", "second", "third"]);
    }

    #[test]
    fn sink_drops_oldest_entries_past_the_cap() {
        let sink = ErrorEventSink::new();
        let overflow = 5usize;
        for i in 0..ERROR_BUFFER_CAP + overflow {
            sink.capture(entry(&i.to_string()));
        }
        let drained = sink.drain();
        // The buffer never grows past the cap.
        assert_eq!(drained.len(), ERROR_BUFFER_CAP);
        // The oldest `overflow` entries (0..5) were evicted; the surviving
        // window starts at `overflow` and stays FIFO-ordered.
        assert_eq!(drained[0].message, overflow.to_string());
        assert_eq!(
            drained[ERROR_BUFFER_CAP - 1].message,
            (ERROR_BUFFER_CAP + overflow - 1).to_string()
        );
    }

    // ── #280 §2 — app.log rotation tests ──────────────────────────────

    /// Build a synthetic `AppLogFile` rooted at `dir`. `bytes` is preloaded
    /// so the tests can drive `rotate()` directly without needing to write
    /// `APP_LOG_MAX_BYTES` worth of data first.
    fn build_app_log(dir: &std::path::Path, bytes: u64) -> Arc<AppLogFile> {
        let path = dir.join("app.log");
        let f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .expect("open active log");
        Arc::new(AppLogFile {
            file: Mutex::new(f),
            bytes: AtomicU64::new(bytes),
            path,
        })
    }

    /// Seed `app.log.<i>` with the given marker content so a successful
    /// rotation can be observed by reading back which marker now lives
    /// where.
    fn seed_numbered(dir: &std::path::Path, i: u32, marker: &str) {
        let p = dir.join(format!("app.log.{i}"));
        std::fs::write(p, marker).expect("seed numbered");
    }

    fn read_marker(dir: &std::path::Path, name: &str) -> Option<String> {
        std::fs::read_to_string(dir.join(name)).ok()
    }

    #[test]
    fn rotate_renames_active_to_dot_1_and_creates_fresh() {
        let tmp = tempfile::tempdir().expect("tmp");
        let state = build_app_log(tmp.path(), APP_LOG_MAX_BYTES);
        // Put a marker in the active file so we can verify it moved to .1.
        std::fs::write(&state.path, b"ACTIVE").expect("seed active");

        rotate(&state);

        assert_eq!(read_marker(tmp.path(), "app.log.1").as_deref(), Some("ACTIVE"));
        // Fresh active file exists and is empty.
        let active = std::fs::read(tmp.path().join("app.log")).expect("read active");
        assert!(active.is_empty(), "active file should be empty after rotate");
        // Counter reset.
        assert_eq!(state.bytes.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn rotate_shifts_existing_numbered_files() {
        let tmp = tempfile::tempdir().expect("tmp");
        let state = build_app_log(tmp.path(), APP_LOG_MAX_BYTES);
        std::fs::write(&state.path, b"ACTIVE").expect("seed active");
        seed_numbered(tmp.path(), 1, "ONE");
        seed_numbered(tmp.path(), 2, "TWO");

        rotate(&state);

        assert_eq!(read_marker(tmp.path(), "app.log.1").as_deref(), Some("ACTIVE"));
        assert_eq!(read_marker(tmp.path(), "app.log.2").as_deref(), Some("ONE"));
        assert_eq!(read_marker(tmp.path(), "app.log.3").as_deref(), Some("TWO"));
    }

    /// G-HIGH-1 — the rotation walks `(KEEP - 1) ..= 1`, so the highest
    /// rename is `.<KEEP - 1> → .<KEEP>`. The old `.<KEEP>` is evicted
    /// atomically. No `.<KEEP + 1>` is ever created.
    #[test]
    fn rotate_caps_total_file_count_at_keep_plus_one() {
        let tmp = tempfile::tempdir().expect("tmp");
        let state = build_app_log(tmp.path(), APP_LOG_MAX_BYTES);
        std::fs::write(&state.path, b"ACTIVE").expect("seed active");
        // Pre-seed every retention slot.
        for i in 1..=APP_LOG_KEEP {
            seed_numbered(tmp.path(), i, &format!("DOT_{i}"));
        }

        rotate(&state);

        // After rotation: active + KEEP numbered files = KEEP + 1.
        let count = std::fs::read_dir(tmp.path())
            .expect("read dir")
            .filter_map(|e| e.ok())
            .filter(|e| {
                let name = e.file_name().to_string_lossy().to_string();
                name == "app.log" || name.starts_with("app.log.")
            })
            .count();
        assert_eq!(count, (APP_LOG_KEEP + 1) as usize);

        // `.<KEEP + 1>` must NOT exist.
        let overflow = tmp.path().join(format!("app.log.{}", APP_LOG_KEEP + 1));
        assert!(!overflow.exists(), "overflow file should not exist: {:?}", overflow);

        // Oldest retained slot (.<KEEP>) is the former .<KEEP - 1> content
        // (because we walked descending and renamed .<KEEP - 1> over the
        // prior .<KEEP>, evicting it).
        let oldest = read_marker(tmp.path(), &format!("app.log.{}", APP_LOG_KEEP));
        assert_eq!(oldest.as_deref(), Some(format!("DOT_{}", APP_LOG_KEEP - 1).as_str()));
    }

    /// G-MED-1 — under concurrent invocation, `rotate()` re-checks the
    /// byte counter inside the lock. After the first rotation resets the
    /// counter to 0, the second call must return early without shifting
    /// the just-rotated files.
    #[test]
    fn rotate_is_idempotent_under_concurrent_invocations() {
        let tmp = tempfile::tempdir().expect("tmp");
        let state = build_app_log(tmp.path(), APP_LOG_MAX_BYTES);
        std::fs::write(&state.path, b"ACTIVE").expect("seed active");

        // First rotation: shifts ACTIVE → .1.
        rotate(&state);
        assert_eq!(read_marker(tmp.path(), "app.log.1").as_deref(), Some("ACTIVE"));
        assert_eq!(state.bytes.load(Ordering::Relaxed), 0);

        // Second rotation with bytes already reset: must be a no-op. If it
        // wasn't idempotent, the (now-empty) active file would shift to
        // .1, evicting the real backup.
        rotate(&state);
        assert_eq!(read_marker(tmp.path(), "app.log.1").as_deref(), Some("ACTIVE"));
        assert!(
            read_marker(tmp.path(), "app.log.2").is_none(),
            "no second-slot file should exist after idempotent second rotation"
        );
    }
}
