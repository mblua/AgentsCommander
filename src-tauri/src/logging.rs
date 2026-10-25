//! Process-wide logger initialization shared by the GUI and CLI entry points.
//!
//! Both `lib::run()` (GUI) and `main()` (CLI branch, before `cli::handle_cli`)
//! call [`init_logger`] so every `log::*` invocation reaches a single sink:
//! stderr **and** `<config_dir>/app.log`. Pre-#137-followup the CLI path
//! skipped this and silently dropped every `log::*` call (including the
//! `[task]` audit lines), undermining plan #137 §3a's HIGH-1 mitigation.
//!
//! Idempotent via a process-wide [`OnceLock`]: calling more than once is a
//! silent no-op. Defensive only — current call sites are mutually exclusive
//! (a single process runs either the GUI path OR the CLI path, never both).
//! Without the guard, a second `env_logger::Builder::init()` would panic via
//! `log::set_logger`'s "called twice" contract.

use std::collections::{HashSet, VecDeque};
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

static INIT: OnceLock<()> = OnceLock::new();
static PAYLOAD_PANIC_HOOK: OnceLock<()> = OnceLock::new();

thread_local! {
    static PAYLOAD_WORKER_DEPTH: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

struct PayloadWorkerGuard;

impl PayloadWorkerGuard {
    fn enter() -> Self {
        PAYLOAD_PANIC_HOOK.get_or_init(|| {
            let previous = std::panic::take_hook();
            std::panic::set_hook(Box::new(move |information| {
                let suppressed = PAYLOAD_WORKER_DEPTH.with(|depth| depth.get() > 0);
                if !suppressed {
                    previous(information);
                }
            }));
        });
        PAYLOAD_WORKER_DEPTH.with(|depth| depth.set(depth.get().saturating_add(1)));
        Self
    }
}

impl Drop for PayloadWorkerGuard {
    fn drop(&mut self) {
        PAYLOAD_WORKER_DEPTH.with(|depth| depth.set(depth.get().saturating_sub(1)));
    }
}

/// Structural marker returned after a terminal-payload panic. The original
/// panic object is dropped while hook suppression is still active and never
/// crosses a snapshot task boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PayloadPanic;

/// Catch a panic in a terminal-payload worker without allowing the default
/// panic hook to print attacker-controlled parser or renderer data. Unrelated
/// panics continue through the process hook unchanged.
pub(crate) fn catch_payload_unwind<F, T>(operation: F) -> Result<T, PayloadPanic>
where
    F: FnOnce() -> T,
{
    let _guard = PayloadWorkerGuard::enter();
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(operation)) {
        Ok(value) => Ok(value),
        Err(payload) => {
            drop(payload);
            Err(PayloadPanic)
        }
    }
}

/// Poll a payload-bearing future with panic-hook suppression active only for
/// each synchronous poll. This remains correct when Tokio moves the future
/// between worker threads and converts a panic to one structural marker.
pub(crate) async fn catch_payload_future<F>(future: F) -> Result<F::Output, PayloadPanic>
where
    F: std::future::Future,
{
    let mut future = Some(Box::pin(future));
    std::future::poll_fn(move |context| {
        let Some(pending) = future.as_mut() else {
            return std::task::Poll::Ready(Err(PayloadPanic));
        };
        match catch_payload_unwind(|| pending.as_mut().poll(context)) {
            Ok(std::task::Poll::Pending) => std::task::Poll::Pending,
            Ok(std::task::Poll::Ready(output)) => {
                let completed = future.take();
                match catch_payload_unwind(|| drop(completed)) {
                    Ok(()) => std::task::Poll::Ready(Ok(output)),
                    Err(panic) => std::task::Poll::Ready(Err(panic)),
                }
            }
            Err(panic) => {
                let failed = future.take();
                let drop_panic = catch_payload_unwind(|| drop(failed)).err();
                std::task::Poll::Ready(Err(drop_panic.unwrap_or(panic)))
            }
        }
    })
    .await
}

/// Collapse both a guarded worker panic and a Tokio task failure without ever
/// invoking `JoinError` formatting, which includes string panic payloads.
pub(crate) fn collapse_payload_task<T>(
    joined: Result<Result<T, PayloadPanic>, tokio::task::JoinError>,
) -> Result<T, PayloadPanic> {
    match joined {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(_)) | Err(_) => Err(PayloadPanic),
    }
}

/// #612 runtime-adjustable verbosity for our own targets (`agentscommander*`),
/// stored as `log::Level as u8` (Error=1 .. Trace=5). Installed ONLY on the
/// no-RUST_LOG path (see `init_logger_inner`); the RUST_LOG dev-override path
/// keeps env_logger frozen and leaves this unset.
static RUNTIME_LEVEL: OnceLock<Arc<AtomicU8>> = OnceLock::new();

/// #612 fixed inner filter for the gated path: third-party crates pinned at
/// `warn`; our own targets opened to `trace` so the runtime atomic (not
/// env_logger) is the sole gate for `agentscommander*`.
const GATE_INNER_FILTER: &str = "warn,agentscommander=trace";

/// #612 lenient string -> Level for the runtime gate. None / legacy full-filter
/// string / typo all fall back to Info (the historical default).
fn level_from_str(s: &str) -> log::Level {
    match s.trim().to_ascii_lowercase().as_str() {
        "error" => log::Level::Error,
        "warn" => log::Level::Warn,
        "info" => log::Level::Info,
        "debug" => log::Level::Debug,
        "trace" => log::Level::Trace,
        _ => log::Level::Info,
    }
}

/// #612 strict normalize for the command boundary: returns the canonical
/// lowercase string for a valid level, else None (so `set_log_level` rejects
/// junk instead of silently persisting it).
pub fn normalize_log_level(s: &str) -> Option<&'static str> {
    match s.trim().to_ascii_lowercase().as_str() {
        "error" => Some("error"),
        "warn" => Some("warn"),
        "info" => Some("info"),
        "debug" => Some("debug"),
        "trace" => Some("trace"),
        _ => None,
    }
}

/// #612 global `max_level` for the gated path. Clamped to at least Warn so
/// third-party `warn!` stays visible even when the user picks Error for our own
/// targets; raising it to the selected level lets the `log!` macro short-circuit
/// suppressed debug!/trace! on hot paths BEFORE they reach the gate (the PTY
/// read-loop trace! is the motivating case).
fn max_filter_for(level: log::Level) -> log::LevelFilter {
    std::cmp::max(level.to_level_filter(), log::LevelFilter::Warn)
}

/// #612 thin `log::Log` wrapper: consults the runtime atomic for
/// `agentscommander*` targets and delegates everything else to a fixed inner
/// `env_logger::Logger` (third-party crates pinned by `GATE_INNER_FILTER`).
/// Lets the level change with no restart while reusing the inner logger's
/// format closure (timestamp + secret scrub + #264 sink + rotation) unchanged.
struct LevelGateLogger {
    inner: env_logger::Logger,
    level: Arc<AtomicU8>,
}

impl log::Log for LevelGateLogger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        if metadata.target().starts_with("agentscommander") {
            (metadata.level() as u8) <= self.level.load(Ordering::Relaxed)
        } else {
            // Third-party: defer to the inner env_logger's fixed warn filter.
            self.inner.enabled(metadata)
        }
    }

    fn log(&self, record: &log::Record) {
        // Re-check per the log::Log contract: the global max_level may admit
        // records this gate rejects. Delegating to inner reuses the existing
        // format closure unchanged (scrub + #264 sink + rotation).
        if self.enabled(record.metadata()) {
            self.inner.log(record);
        }
    }

    fn flush(&self) {
        self.inner.flush();
    }
}

struct PrivacyFilterLogger<L> {
    inner: L,
}

fn is_payload_logging_target(target: &str) -> bool {
    target == "vt100" || target.starts_with("vt100::")
}

impl<L: log::Log> log::Log for PrivacyFilterLogger<L> {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        !is_payload_logging_target(metadata.target()) && self.inner.enabled(metadata)
    }

    fn log(&self, record: &log::Record) {
        #[cfg(test)]
        test_tee_push(record);
        if self.enabled(record.metadata()) {
            self.inner.log(record);
        }
    }

    fn flush(&self) {
        self.inner.flush();
    }
}

/// #2455 test-only tee: every record reaching `PrivacyFilterLogger`, whichever
/// `init_logger` leg installed it. Snapshot-only on purpose: the lib test
/// binary runs tests concurrently, so a drain by one test would destroy
/// another's evidence.
#[cfg(test)]
static TEST_TEE: Mutex<Vec<String>> = Mutex::new(Vec::new());

#[cfg(test)]
fn test_tee_push(record: &log::Record) {
    let line = format!("{} {} {}", record.level(), record.target(), record.args());
    TEST_TEE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push(line);
}

/// #2455 clone of every line the tee has seen so far; never drains.
#[cfg(test)]
pub(crate) fn test_tee_snapshot() -> Vec<String> {
    TEST_TEE.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// #2455 make sure SOME logger is installed (a second `set_boxed_logger` is a
/// harmless `Err`), then open the static max level to `Info`: `log::info!`
/// checks it before it ever reaches the logger.
#[cfg(test)]
pub(crate) fn test_install_logger() {
    init_logger();
    log::set_max_level(log::LevelFilter::Info);
}

/// #612 update runtime verbosity for `agentscommander*`. Returns false (no-op)
/// when the gate is not installed (RUST_LOG override active, or called before
/// `init_logger`), so callers can tell whether the backend stayed frozen.
pub fn set_runtime_log_level(level: &str) -> bool {
    let lvl = level_from_str(level);
    match RUNTIME_LEVEL.get() {
        Some(atomic) => {
            atomic.store(lvl as u8, Ordering::Relaxed);
            log::set_max_level(max_filter_for(lvl));
            true
        }
        None => false,
    }
}

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

fn machine_output_enabled() -> bool {
    std::env::var_os("AC_MACHINE_OUTPUT").is_some()
}

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
            if !machine_output_enabled() {
                eprintln!(
                    "[log] rotate: cannot resolve parent for {}, skipping",
                    base.display()
                );
            }
            return;
        }
    };
    let stem = match base.file_name().and_then(|n| n.to_str()) {
        Some(s) => s,
        None => {
            if !machine_output_enabled() {
                eprintln!(
                    "[log] rotate: cannot extract file name for {}, skipping",
                    base.display()
                );
            }
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
                if !machine_output_enabled() {
                    eprintln!(
                        "[log] rotate: failed to rename {} to {}: {} (continuing)",
                        from.display(),
                        to.display(),
                        e
                    );
                }
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
            if !machine_output_enabled() {
                eprintln!(
                    "[log] rotate: failed to rename {} to {}: {}, leaving active file in place",
                    base.display(),
                    one.display(),
                    e
                );
            }
            // Active file stays — next writes append to it; cap will be
            // breached temporarily.
            return;
        }
    } else if let Err(e) = std::fs::remove_file(base) {
        if !machine_output_enabled() {
            eprintln!(
                "[log] rotate: KEEP=0 and failed to remove {}: {}, leaving active file in place",
                base.display(),
                e
            );
        }
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
            if !machine_output_enabled() {
                eprintln!(
                    "[log] rotate: create_new failed for {} ({}); falling back to append",
                    base.display(),
                    e
                );
            }
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
            if !machine_output_enabled() {
                eprintln!(
                    "[log] rotate: failed to open fresh active file at {}: {}",
                    base.display(),
                    e
                );
            }
            // Counter NOT reset — next writes will see "over cap" and try
            // rotating again. Active fd in `file_guard` is now stale: on
            // both Unix and Windows the open handle survives `rename()` and
            // continues to point at the moved file, so subsequent writes
            // land in the renamed `.1` rather than a fresh `app.log`. The
            // next rotation may then shift that file again (LOW-5 hazard,
            // documented in the round-2 review). Stderr writes are
            // unaffected, so no log is silently dropped. Recovery is a
            // logger re-init (process restart).
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

/// #612 apply the shared format closure (timestamp + universal secret scrub +
/// #264 error-sink tee + 50MB rotation) to `builder`. The body is the EXACT
/// closure that was inline in `init_logger_inner`; only the captured `log_state`
/// moves to this parameter, so both the RUST_LOG path and the runtime-gated
/// path build on one definition. Do NOT reorder: the #264 capture stays BEFORE
/// the buf/file writes (M1) so a failing write cannot skip error capture.
fn install_format(builder: &mut env_logger::Builder, log_state: Arc<Option<Arc<AppLogFile>>>) {
    builder.format(move |buf, record| {
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
        if !machine_output_enabled() {
            buf.write_all(line.as_bytes())?;
        }
        if let Some(ref state) = *log_state {
            if let Ok(mut f) = state.file.lock() {
                let _ = f.write_all(line.as_bytes());
            }
            let new_total =
                state.bytes.fetch_add(line.len() as u64, Ordering::Relaxed) + line.len() as u64;
            if new_total >= APP_LOG_MAX_BYTES {
                rotate(state);
            }
        }
        Ok(())
    });
}

fn init_logger_inner() {
    if let Err(error) = crate::config::instance_gitignore::ensure_instance_gitignore() {
        if !machine_output_enabled() {
            eprintln!("[instance-gitignore] warning: {error}");
        }
    }

    let log_state: Option<Arc<AppLogFile>> = crate::config::config_dir().and_then(|dir| {
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("app.log");
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .ok()
            .map(|f| {
                if !machine_output_enabled() {
                    eprintln!("[log] file logging to {}", path.display());
                }
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

    // #612 precedence (was #93): RUST_LOG env > settings.logLevel > Info default.
    //
    // RUST_LOG is the dev override and wins. When set, keep the original frozen
    // env_logger path with NO runtime gate: a dev who exports RUST_LOG wants
    // exactly that filter (including third-party verbosity) for the whole
    // session, so the UI selector cannot change the backend level until a
    // restart without RUST_LOG. read_log_level_only is read-only and
    // side-effect-free (no migration / token-gen / save), so log calls inside
    // the later full load_settings() flow still reach this sink.
    if let Ok(rust_log) = std::env::var("RUST_LOG") {
        let mut builder = env_logger::Builder::from_env(env_logger::Env::default());
        builder.parse_filters(&rust_log);
        install_format(&mut builder, Arc::clone(&log_state));
        let inner = builder.build();
        let max_level = inner.filter();
        let logger = PrivacyFilterLogger { inner };
        if log::set_boxed_logger(Box::new(logger)).is_ok() {
            log::set_max_level(max_level);
        }
        return;
    }

    // No RUST_LOG: install the runtime-gated logger. The initial level comes
    // from settings.logLevel (canonical 5-value string; None / legacy filter /
    // typo all fall back to Info via level_from_str). from_env(Env::default())
    // contributes nothing to the filter here (RUST_LOG is unset) but preserves
    // RUST_LOG_STYLE color handling, matching the prior behavior; the fixed
    // GATE_INNER_FILTER then pins third-party crates at warn and opens our own
    // targets to trace so the runtime atomic is the sole gate for them.
    let initial = level_from_str(
        crate::config::settings::read_log_level_only()
            .as_deref()
            .unwrap_or("info"),
    );
    let atomic = Arc::new(AtomicU8::new(initial as u8));
    let _ = RUNTIME_LEVEL.set(Arc::clone(&atomic));

    let mut builder = env_logger::Builder::from_env(env_logger::Env::default());
    builder.parse_filters(GATE_INNER_FILTER);
    install_format(&mut builder, Arc::clone(&log_state));
    let inner = builder.build(); // build(), NOT init(): we wrap it below

    let gate = LevelGateLogger {
        inner,
        level: atomic,
    };
    let logger = PrivacyFilterLogger { inner: gate };
    if log::set_boxed_logger(Box::new(logger)).is_ok() {
        log::set_max_level(max_filter_for(initial));
    }
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
    /// #3036 emission-site identity (`typeId`); see `error_type_id`.
    pub type_id: String,
}

/// #3036 — collision-free emission-site identity built from
/// `(target, file, line)`: byte-length prefixes and explicit presence markers,
/// never the message. Format: `v1|<len>:<target>|<n|s<len>:file>|<n|u<line>>`.
fn error_type_id(target: &str, file: Option<&str>, line: Option<u32>) -> String {
    let file_part = match file {
        Some(f) => format!("s{}:{}", f.len(), f),
        None => "n".to_string(),
    };
    let line_part = match line {
        Some(l) => format!("u{l}"),
        None => "n".to_string(),
    };
    format!("v1|{}:{}|{}|{}", target.len(), target, file_part, line_part)
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
            type_id: error_type_id(record.target(), record.file(), record.line()),
        }
    }
}

/// Whether a log record should be teed into the error-modal sink: ERROR level
/// AND a target owned by one of AgentsCommander's own crates
/// (`agentscommander_lib::…` or `agentscommander`). The target-prefix guard
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

/// Process-wide sink that tees ERROR-level log entries to the UI layer.
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
    state: Mutex<ErrorSinkState>,
    notify: tokio::sync::Notify,
}

/// #3036 — one lock guards the queue and the process-only mute state, so a
/// mute linearizes against capture/drain: already-queued entries stay; only
/// later inserts of a muted type are skipped. `known` holds observed type IDs.
#[derive(Default)]
struct ErrorSinkState {
    pending: VecDeque<ErrorLogEntry>,
    muted: HashSet<String>,
    known: HashSet<String>,
}

impl ErrorEventSink {
    fn new() -> Self {
        Self {
            state: Mutex::new(ErrorSinkState::default()),
            notify: tokio::sync::Notify::new(),
        }
    }

    fn lock_state(&self) -> std::sync::MutexGuard<'_, ErrorSinkState> {
        // into_inner() recovers a poisoned lock so one panic while holding
        // the mutex cannot permanently disable error capture.
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Buffer an ERROR entry and wake the emit task. Runs inside the
    /// `env_logger` format closure, so it MUST NOT call any `log::*` macro and
    /// MUST NOT panic. `notify_one()` is sync, non-blocking, log-free,
    /// panic-free, and needs no tokio runtime (only the awaiting side does).
    pub fn capture(&self, entry: ErrorLogEntry) {
        {
            let mut state = self.lock_state();
            if !state.known.contains(&entry.type_id) {
                state.known.insert(entry.type_id.clone());
            }
            // #3036 — muted type: skip only the popup queue; the caller's
            // stderr/app.log writes still run.
            if state.muted.contains(&entry.type_id) {
                return;
            }
            let buf = &mut state.pending;
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
        let mut state = self.lock_state();
        Vec::from(std::mem::take(&mut state.pending))
    }

    /// #3036 — sorted snapshot of the muted type IDs.
    pub fn muted_snapshot(&self) -> Vec<String> {
        sorted_ids(&self.lock_state().muted)
    }

    /// #3036 — idempotently mute/unmute future captures of a known type ID.
    /// Unknown IDs are rejected with no mutation. Never replays skipped entries.
    pub fn set_muted(&self, type_id: &str, muted: bool) -> Result<Vec<String>, String> {
        let mut state = self.lock_state();
        if !state.known.contains(type_id) {
            return Err("Unknown error popup type".to_string());
        }
        if muted {
            state.muted.insert(type_id.to_string());
        } else {
            state.muted.remove(type_id);
        }
        Ok(sorted_ids(&state.muted))
    }

    /// Await the next `capture()` signal. Used only by `spawn_error_emit_task`.
    async fn wait_for_capture(&self) {
        self.notify.notified().await;
    }
}

fn sorted_ids(ids: &HashSet<String>) -> Vec<String> {
    let mut out: Vec<String> = ids.iter().cloned().collect();
    out.sort();
    out
}

static ERROR_SINK: OnceLock<ErrorEventSink> = OnceLock::new();

/// Accessor for the process-wide error sink. Lazily initialized on first use.
pub fn error_sink() -> &'static ErrorEventSink {
    ERROR_SINK.get_or_init(ErrorEventSink::new)
}

/// #1883 — test-only serialization for error-sink drain windows. Two library
/// tests read-and-clear the process-wide `ERROR_SINK` (`phone::mailbox::tests`
/// async, `logging::tests` sync); without mutual exclusion one test's drain can
/// steal the other's entries. This lock is compiled only for tests and never
/// enters any production sink or logging path.
#[cfg(test)]
pub(crate) static ERROR_SINK_TEST_DRAIN_LOCK: tokio::sync::Mutex<()> =
    tokio::sync::Mutex::const_new(());

/// Delivery port for the content-free error-log ping.
pub(crate) trait ErrorLogEvents {
    fn error_log_event(&self);
}

/// Wait for captures and deliver pings outside the logger's format closure.
/// The setup hook spawns this future on the existing application runtime.
pub(crate) async fn spawn_error_emit_task<E: ErrorLogEvents>(events: E) {
    error_emit_loop(error_sink(), &events).await;
}

async fn error_emit_loop<E: ErrorLogEvents + ?Sized>(sink: &ErrorEventSink, events: &E) {
    loop {
        sink.wait_for_capture().await;
        events.error_log_event();
    }
}

#[cfg(test)]
mod tests;
