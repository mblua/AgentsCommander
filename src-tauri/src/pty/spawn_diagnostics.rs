//! #942 - spawn diagnostics for local PTY sessions.
//!
//! Diagnostics only. Nothing in this module changes how a session is spawned,
//! killed or read; it only records evidence so an intermittent coding-agent
//! blank-terminal hang explains itself after the fact:
//!
//! 1. `[pty] spawn-record` - the argv AC actually executed, the resolved cwd,
//!    the coding-agent id, the effective `CODEX_HOME`, and how many other
//!    sessions were spawned in the preceding window (shared `~/.codex` contention).
//! 2. `[pty] first-output` (DEBUG) - time from spawn to the first byte read off
//!    the PTY, and `[pty] first-paint` (INFO) - time until the child produced
//!    real output. Both are needed on Windows: ConPTY writes its own 16-byte
//!    handshake (`ESC[?9001h ESC[?1004h`) into the PTY within ~20ms of every
//!    spawn, child or no child, so the first byte is never evidence that the
//!    child came up. Crossing the paint floor is.
//! 3. `[pty] startup-stall` - WARN when the child produced nothing (or nothing
//!    past the ConPTY handshake) within the threshold, with child liveness at
//!    that moment.
//! 4. `[pty] child-exit` - every child exit, tagged with an unambiguous cause:
//!    `ac-requested` (we asked: session kill, job terminate, resource-monitor
//!    kill_group) vs `child-initiated` (it died on its own).
//! 5. The first N bytes the child ever wrote, replayed into the stall and the
//!    unexpected-exit events, so an error message that never reached the screen
//!    is still on record.
//!
//! Tunables (env vars, read once):
//! - `AC_SPAWN_STALL_TIMEOUT_MS` (default 5000, `0` disables the stall check)
//! - `AC_SPAWN_PAINT_FLOOR_BYTES` (default 64)
//! - `AC_SPAWN_HEAD_BYTES` (default 512)
//! - `AC_SPAWN_CONCURRENCY_WINDOW_MS` (default 10000)

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use portable_pty::ExitStatus;
use uuid::Uuid;

/// No output within this window after spawn is a startup stall.
const DEFAULT_STALL_TIMEOUT_MS: u64 = 5_000;
/// Output below this many bytes is not a painted screen; it is the ConPTY handshake
/// (16 bytes) and nothing else. A coding agent paints a full TUI immediately, so a
/// coding-agent session still under the floor at the stall deadline is the
/// blank-terminal symptom. Plain shells are exempt from the stall check: a bare
/// prompt is legitimately tiny.
const DEFAULT_PAINT_FLOOR_BYTES: u64 = 64;
/// How much of the child's first output we retain for stall / early-exit reports.
const DEFAULT_HEAD_BYTES: u64 = 512;
/// Window used to correlate a hang with concurrent startups on shared agent state.
const DEFAULT_CONCURRENCY_WINDOW_MS: u64 = 10_000;
/// Safety net on the recent-spawn ring; only the time window is load-bearing.
const RECENT_SPAWNS_CAP: usize = 128;

/// Child poll cadence while we are still waiting for a painted screen.
const STARTUP_POLL: Duration = Duration::from_millis(250);
/// Child poll cadence once the session is up (exit attribution only).
const STEADY_POLL: Duration = Duration::from_secs(2);

fn env_u64(key: &str, default: u64) -> u64 {
    match std::env::var(key) {
        Ok(raw) => match raw.trim().parse::<u64>() {
            Ok(value) => value,
            Err(_) => {
                log::warn!("[pty] ignoring non-numeric {key}='{raw}', using {default}");
                default
            }
        },
        Err(_) => default,
    }
}

pub fn stall_timeout() -> Duration {
    static VALUE: OnceLock<Duration> = OnceLock::new();
    *VALUE.get_or_init(|| {
        Duration::from_millis(env_u64(
            "AC_SPAWN_STALL_TIMEOUT_MS",
            DEFAULT_STALL_TIMEOUT_MS,
        ))
    })
}

fn paint_floor() -> u64 {
    static VALUE: OnceLock<u64> = OnceLock::new();
    *VALUE.get_or_init(|| env_u64("AC_SPAWN_PAINT_FLOOR_BYTES", DEFAULT_PAINT_FLOOR_BYTES))
}

fn head_bytes() -> usize {
    static VALUE: OnceLock<usize> = OnceLock::new();
    *VALUE.get_or_init(|| env_u64("AC_SPAWN_HEAD_BYTES", DEFAULT_HEAD_BYTES) as usize)
}

fn concurrency_window() -> Duration {
    static VALUE: OnceLock<Duration> = OnceLock::new();
    *VALUE.get_or_init(|| {
        Duration::from_millis(env_u64(
            "AC_SPAWN_CONCURRENCY_WINDOW_MS",
            DEFAULT_CONCURRENCY_WINDOW_MS,
        ))
    })
}

/// What AC knows about a child process at the instant we ask.
#[derive(Debug, Clone)]
pub enum ChildLiveness {
    /// The child is running.
    Alive,
    /// The child has exited; we hold its status.
    Exited {
        code: u32,
        success: bool,
        status: String,
    },
    /// The poll itself failed.
    Unknown(String),
    /// No PTY instance for this session anymore (killed, or never inserted).
    Gone,
}

impl ChildLiveness {
    pub fn as_log(&self) -> String {
        match self {
            ChildLiveness::Alive => "alive".to_string(),
            ChildLiveness::Exited { code, .. } => format!("exited({code})"),
            ChildLiveness::Unknown(err) => format!("unknown({err})"),
            ChildLiveness::Gone => "gone".to_string(),
        }
    }

    fn exited_ok(&self) -> bool {
        matches!(self, ChildLiveness::Exited { success: true, .. })
    }
}

impl From<&ExitStatus> for ChildLiveness {
    fn from(status: &ExitStatus) -> Self {
        ChildLiveness::Exited {
            code: status.exit_code(),
            success: status.success(),
            status: format!("{status:?}"),
        }
    }
}

/// Who ended the child. The whole point of #942's requirement 3: an AC-initiated
/// stop (session destroy, job terminate, resource-monitor kill_group) can never be
/// confused with a child that died on its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitCause {
    AcRequested,
    ChildInitiated,
}

impl ExitCause {
    pub fn as_log(self) -> &'static str {
        match self {
            ExitCause::AcRequested => "ac-requested",
            ExitCause::ChildInitiated => "child-initiated",
        }
    }
}

/// How busy the spawner was just before this spawn. Correlates a hang with
/// concurrent startups against the same shared agent state (e.g. `~/.codex`).
#[derive(Debug, Clone, Copy)]
pub struct SpawnWindow {
    pub window_ms: u64,
    pub total: usize,
    pub same_agent: usize,
}

pub struct SpawnRecordInit {
    pub session_id: Uuid,
    pub pid: Option<u32>,
    /// The argv as executed, including any `cmd.exe /C` wrapper AC added.
    pub argv: Vec<String>,
    pub cwd: String,
    pub agent_id: Option<String>,
    /// Effective `CODEX_HOME` for the child (configured, inherited, or removed).
    pub codex_home: Option<String>,
    pub configured_env_count: usize,
    pub removed_env_count: usize,
    pub window: SpawnWindow,
    /// Taken immediately before the child was spawned; time zero for first-output.
    pub started: Instant,
}

pub struct SpawnRecord {
    session_id: Uuid,
    pid: Option<u32>,
    argv: Vec<String>,
    cwd: String,
    agent_id: Option<String>,
    codex_home: Option<String>,
    configured_env_count: usize,
    removed_env_count: usize,
    window: SpawnWindow,
    started: Instant,
    /// Microseconds from spawn to the first byte; 0 means nothing read yet.
    first_output_us: AtomicU64,
    /// Microseconds from spawn to the first real output (past the paint floor);
    /// 0 means the child has still painted nothing.
    first_paint_us: AtomicU64,
    bytes_read: AtomicU64,
    head: Mutex<Vec<u8>>,
    head_full: AtomicBool,
    ac_stop: AtomicBool,
    ac_stop_source: Mutex<Option<String>>,
    stall_reported: AtomicBool,
    exit_reported: AtomicBool,
}

impl SpawnRecord {
    fn new(init: SpawnRecordInit) -> Self {
        Self {
            session_id: init.session_id,
            pid: init.pid,
            argv: init.argv,
            cwd: init.cwd,
            agent_id: init.agent_id,
            codex_home: init.codex_home,
            configured_env_count: init.configured_env_count,
            removed_env_count: init.removed_env_count,
            window: init.window,
            started: init.started,
            first_output_us: AtomicU64::new(0),
            first_paint_us: AtomicU64::new(0),
            bytes_read: AtomicU64::new(0),
            head: Mutex::new(Vec::new()),
            head_full: AtomicBool::new(false),
            ac_stop: AtomicBool::new(false),
            ac_stop_source: Mutex::new(None),
            stall_reported: AtomicBool::new(false),
            exit_reported: AtomicBool::new(false),
        }
    }

    pub fn session_id(&self) -> Uuid {
        self.session_id
    }

    fn agent_log(&self) -> &str {
        self.agent_id.as_deref().unwrap_or("none")
    }

    fn codex_home_log(&self) -> &str {
        self.codex_home.as_deref().unwrap_or("unset")
    }

    fn pid_log(&self) -> String {
        match self.pid {
            Some(pid) => pid.to_string(),
            None => "unknown".to_string(),
        }
    }

    pub fn saw_output(&self) -> bool {
        self.first_output_us.load(Ordering::Relaxed) != 0
    }

    pub fn painted(&self) -> bool {
        self.first_paint_us.load(Ordering::Relaxed) != 0
    }

    fn ms_log(stamp_us: &AtomicU64) -> String {
        match stamp_us.load(Ordering::Relaxed) {
            0 => "none".to_string(),
            us => (us / 1_000).to_string(),
        }
    }

    pub fn stop_requested(&self) -> bool {
        self.ac_stop.load(Ordering::Relaxed)
    }

    fn stop_source(&self) -> String {
        self.ac_stop_source
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
            .unwrap_or_else(|| "none".to_string())
    }

    /// Poll fast while the child still owes us a painted screen, slowly afterwards.
    pub fn in_startup_phase(&self) -> bool {
        !self.painted() && !self.stall_reported.load(Ordering::Relaxed)
    }

    fn mark_ac_stop(&self, source: &str) {
        {
            let mut current = self.ac_stop_source.lock().unwrap_or_else(|e| e.into_inner());
            if current.is_none() {
                *current = Some(source.to_string());
            }
        }
        self.ac_stop.store(true, Ordering::SeqCst);
    }

    /// The child is dead or on its way out. ConPTY repaints the master while a
    /// session is torn down, so anything read past this point belongs to the teardown,
    /// not to the child, and must not land in the record.
    fn torn_down(&self) -> bool {
        self.exit_reported.load(Ordering::Relaxed) || self.stop_requested()
    }

    /// Hot path: called for every chunk read off the PTY. After the first paint and
    /// a full head buffer it costs one relaxed add plus a handful of relaxed loads.
    pub fn note_output(&self, chunk: &[u8]) {
        if chunk.is_empty() || self.torn_down() {
            return;
        }
        let total = self
            .bytes_read
            .fetch_add(chunk.len() as u64, Ordering::Relaxed)
            + chunk.len() as u64;

        // Both stamps are one-shot, so a settled session never reads the clock here.
        let needs_first = self.first_output_us.load(Ordering::Relaxed) == 0;
        let needs_paint =
            self.first_paint_us.load(Ordering::Relaxed) == 0 && total >= paint_floor();

        if needs_first || needs_paint {
            let elapsed = self.started.elapsed();
            // 0 is the "nothing yet" sentinel for both stamps, so never store it.
            let stamp = (elapsed.as_micros().min(u64::MAX as u128) as u64).max(1);

            // The first byte off the PTY. On Windows this is ConPTY greeting us, not
            // the child, which is why it is DEBUG and never taken as proof of life.
            if needs_first
                && self
                    .first_output_us
                    .compare_exchange(0, stamp, Ordering::SeqCst, Ordering::Relaxed)
                    .is_ok()
            {
                log::debug!(
                    "[pty] first-output session={} pid={} agent={} after_ms={} bytes={}",
                    self.session_id,
                    self.pid_log(),
                    self.agent_log(),
                    elapsed.as_millis(),
                    total
                );
            }

            // The first real output: the child actually painted something. This is the
            // healthy-startup signal, and its absence is the hang.
            if needs_paint
                && self
                    .first_paint_us
                    .compare_exchange(0, stamp, Ordering::SeqCst, Ordering::Relaxed)
                    .is_ok()
            {
                // Only a stall we actually reported can be recovered from.
                if self.stall_reported.load(Ordering::Relaxed) {
                    log::warn!(
                        "[pty] first-paint session={} pid={} agent={} after_ms={} bytes={} late=true (startup stall recovered)",
                        self.session_id,
                        self.pid_log(),
                        self.agent_log(),
                        elapsed.as_millis(),
                        total
                    );
                } else {
                    log::info!(
                        "[pty] first-paint session={} pid={} agent={} after_ms={} bytes={}",
                        self.session_id,
                        self.pid_log(),
                        self.agent_log(),
                        elapsed.as_millis(),
                        total
                    );
                }
            }
        }

        if !self.head_full.load(Ordering::Relaxed) {
            self.capture_head(chunk);
        }
    }

    fn capture_head(&self, chunk: &[u8]) {
        let cap = head_bytes();
        let mut head = self.head.lock().unwrap_or_else(|e| e.into_inner());
        if head.len() >= cap {
            self.head_full.store(true, Ordering::Relaxed);
            return;
        }
        let take = (cap - head.len()).min(chunk.len());
        head.extend_from_slice(&chunk[..take]);
        if head.len() >= cap {
            self.head_full.store(true, Ordering::Relaxed);
        }
    }

    fn head_log(&self) -> String {
        let head = self.head.lock().unwrap_or_else(|e| e.into_inner());
        if head.is_empty() {
            return "<empty>".to_string();
        }
        format!("\"{}\"", String::from_utf8_lossy(&head).escape_debug())
    }

    /// True once the child has had its whole startup window and still produced no
    /// output at all (any session), or nothing past the ConPTY handshake (coding-agent
    /// sessions, which always paint a TUI). Never fires when the check is disabled.
    pub fn startup_stalled(&self) -> bool {
        let timeout = stall_timeout();
        if timeout.is_zero() || self.stall_reported.load(Ordering::Relaxed) {
            return false;
        }
        if self.started.elapsed() < timeout {
            return false;
        }
        let bytes = self.bytes_read.load(Ordering::Relaxed);
        bytes == 0 || (self.agent_id.is_some() && !self.painted())
    }

    /// Requirement 2 + 4: the startup stall event. Emitted at most once.
    pub fn log_startup_stall(&self, liveness: &ChildLiveness) {
        if self.stall_reported.swap(true, Ordering::SeqCst) {
            return;
        }
        log::warn!(
            "[pty] startup-stall session={} pid={} agent={} backend=local child={} elapsed_ms={} threshold_ms={} bytes_read={} paint_floor={} first_output_ms={} argv={:?} cwd={} codex_home={} recent_spawns={} recent_same_agent={} window_ms={} head={}",
            self.session_id,
            self.pid_log(),
            self.agent_log(),
            liveness.as_log(),
            self.started.elapsed().as_millis(),
            stall_timeout().as_millis(),
            self.bytes_read.load(Ordering::Relaxed),
            paint_floor(),
            Self::ms_log(&self.first_output_us),
            self.argv,
            self.cwd,
            self.codex_home_log(),
            self.window.total,
            self.window.same_agent,
            self.window.window_ms,
            self.head_log()
        );
    }

    /// Requirement 3 + 4: the one exit event for this session. Returns false when
    /// another path already reported the exit (the caller may log a debug crumb).
    pub fn log_child_exit(&self, cause: ExitCause, liveness: &ChildLiveness, detail: &str) -> bool {
        if self.exit_reported.swap(true, Ordering::SeqCst) {
            return false;
        }
        let uptime = self.started.elapsed();
        let first_output_ms = Self::ms_log(&self.first_output_us);
        let first_paint_ms = Self::ms_log(&self.first_paint_us);
        let bytes = self.bytes_read.load(Ordering::Relaxed);
        // A child that ends itself without ever painting a screen, or with a failing
        // status, is the #942 smoking gun. A clean exit from a session that did come up
        // is just someone quitting.
        let unexpected =
            cause == ExitCause::ChildInitiated && (!self.painted() || !liveness.exited_ok());

        if unexpected {
            log::warn!(
                "[pty] child-exit session={} pid={} agent={} cause={} detail={} child={} uptime_ms={} first_output_ms={} first_paint_ms={} bytes_read={} argv={:?} cwd={} codex_home={} recent_spawns={} recent_same_agent={} window_ms={} head={}",
                self.session_id,
                self.pid_log(),
                self.agent_log(),
                cause.as_log(),
                detail,
                liveness.as_log(),
                uptime.as_millis(),
                first_output_ms,
                first_paint_ms,
                bytes,
                self.argv,
                self.cwd,
                self.codex_home_log(),
                self.window.total,
                self.window.same_agent,
                self.window.window_ms,
                self.head_log()
            );
        } else {
            log::info!(
                "[pty] child-exit session={} pid={} agent={} cause={} detail={} child={} stop_source={} uptime_ms={} first_output_ms={} first_paint_ms={} bytes_read={}",
                self.session_id,
                self.pid_log(),
                self.agent_log(),
                cause.as_log(),
                detail,
                liveness.as_log(),
                self.stop_source(),
                uptime.as_millis(),
                first_output_ms,
                first_paint_ms,
                bytes
            );
        }
        true
    }

    /// Requirement 1: the full spawn record, one line, on every local spawn.
    fn log_spawn(&self) {
        log::info!(
            "[pty] spawn-record session={} pid={} agent={} backend=local argv={:?} cwd={} codex_home={} env_configured={} env_removed={} recent_spawns={} recent_same_agent={} window_ms={}",
            self.session_id,
            self.pid_log(),
            self.agent_log(),
            self.argv,
            self.cwd,
            self.codex_home_log(),
            self.configured_env_count,
            self.removed_env_count,
            self.window.total,
            self.window.same_agent,
            self.window.window_ms
        );
    }
}

#[derive(Default)]
struct Registry {
    records: HashMap<Uuid, Arc<SpawnRecord>>,
    recent: VecDeque<(Instant, Option<String>)>,
}

fn registry() -> &'static Mutex<Registry> {
    static REGISTRY: OnceLock<Mutex<Registry>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(Registry::default()))
}

/// Requirement 5: count the spawns that happened in the window BEFORE this one,
/// then record this attempt. Call once per spawn, at the top of the spawn path.
pub fn note_spawn_attempt(agent_id: Option<&str>) -> SpawnWindow {
    let window = concurrency_window();
    let now = Instant::now();
    let mut registry = registry().lock().unwrap_or_else(|e| e.into_inner());

    while let Some((at, _)) = registry.recent.front() {
        if now.duration_since(*at) > window {
            registry.recent.pop_front();
        } else {
            break;
        }
    }

    let total = registry.recent.len();
    let same_agent = match agent_id {
        Some(id) => registry
            .recent
            .iter()
            .filter(|(_, recent_agent)| recent_agent.as_deref() == Some(id))
            .count(),
        None => 0,
    };

    registry
        .recent
        .push_back((now, agent_id.map(|id| id.to_string())));
    if registry.recent.len() > RECENT_SPAWNS_CAP {
        registry.recent.pop_front();
    }

    SpawnWindow {
        window_ms: window.as_millis().min(u64::MAX as u128) as u64,
        total,
        same_agent,
    }
}

/// Register a spawned child and emit its spawn record.
pub fn register(init: SpawnRecordInit) -> Arc<SpawnRecord> {
    let record = Arc::new(SpawnRecord::new(init));
    record.log_spawn();
    registry()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .records
        .insert(record.session_id, Arc::clone(&record));
    record
}

/// Tag a session's child as stopped by AC (session kill, job terminate,
/// resource-monitor kill_group) BEFORE any process is touched, so its exit can
/// never be misread as a spontaneous death. No-op for unknown sessions.
pub fn mark_ac_stop(session_id: Uuid, source: &str) -> Option<Arc<SpawnRecord>> {
    let record = registry()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .records
        .get(&session_id)
        .cloned()?;
    record.mark_ac_stop(source);
    Some(record)
}

pub fn forget(session_id: Uuid) {
    registry()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .records
        .remove(&session_id);
}

/// One monitor thread per local session. It owns two signals: the startup stall
/// WARN, and exit attribution for a child that dies without AC asking (the PTY
/// reader cannot be trusted for that on ConPTY, which keeps the pipe open).
/// It ends when the child exits or the session's PTY instance is gone.
pub fn watch_child<P>(record: Arc<SpawnRecord>, probe: P)
where
    P: Fn() -> ChildLiveness + Send + 'static,
{
    std::thread::spawn(move || loop {
        let tick = if record.in_startup_phase() {
            STARTUP_POLL
        } else {
            STEADY_POLL
        };
        std::thread::sleep(tick);

        let liveness = probe();
        match liveness {
            ChildLiveness::Gone => return,
            ChildLiveness::Exited { .. } => {
                let cause = if record.stop_requested() {
                    ExitCause::AcRequested
                } else {
                    ExitCause::ChildInitiated
                };
                record.log_child_exit(cause, &liveness, "observed-by-monitor");
                return;
            }
            ChildLiveness::Alive | ChildLiveness::Unknown(_) => {
                if record.startup_stalled() {
                    record.log_startup_stall(&liveness);
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_record(agent_id: Option<&str>) -> SpawnRecord {
        SpawnRecord::new(SpawnRecordInit {
            session_id: Uuid::new_v4(),
            pid: Some(4242),
            argv: vec!["codex".to_string(), "resume".to_string()],
            cwd: "C:/repo".to_string(),
            agent_id: agent_id.map(|id| id.to_string()),
            codex_home: None,
            configured_env_count: 0,
            removed_env_count: 0,
            window: SpawnWindow {
                window_ms: 10_000,
                total: 0,
                same_agent: 0,
            },
            started: Instant::now(),
        })
    }

    #[test]
    fn head_capture_is_capped_and_stops_after_full() {
        let record = test_record(Some("codex"));
        let cap = head_bytes();
        record.note_output(&vec![b'a'; cap + 64]);
        record.note_output(b"ignored");

        let head = record.head.lock().unwrap();
        assert_eq!(head.len(), cap);
        assert!(head.iter().all(|b| *b == b'a'));
        assert!(record.head_full.load(Ordering::Relaxed));
        assert_eq!(
            record.bytes_read.load(Ordering::Relaxed),
            (cap + 64 + 7) as u64
        );
    }

    #[test]
    fn first_output_stamp_is_recorded_once() {
        let record = test_record(None);
        assert!(!record.saw_output());

        record.note_output(b"hello");
        let first = record.first_output_us.load(Ordering::Relaxed);
        record.note_output(b" world");

        assert!(record.saw_output());
        assert_eq!(record.first_output_us.load(Ordering::Relaxed), first);
        assert_eq!(record.bytes_read.load(Ordering::Relaxed), 11);
    }

    #[test]
    fn paint_is_stamped_once_the_output_passes_the_floor() {
        let record = test_record(Some("codex"));

        // The ConPTY handshake alone is not a paint.
        record.note_output(b"\x1b[?9001h\x1b[?1004h");
        assert!(record.saw_output());
        assert!(!record.painted());

        record.note_output(&vec![b'x'; paint_floor() as usize]);
        assert!(record.painted());
        let painted_at = record.first_paint_us.load(Ordering::Relaxed);

        record.note_output(b"more");
        assert_eq!(
            record.first_paint_us.load(Ordering::Relaxed),
            painted_at,
            "the paint stamp is taken once"
        );
    }

    #[test]
    fn silent_child_stalls_only_after_the_threshold() {
        let record = test_record(Some("codex"));
        assert!(!record.startup_stalled(), "not yet past the threshold");

        let record = SpawnRecord {
            started: Instant::now() - stall_timeout() - Duration::from_millis(1),
            ..test_record(Some("codex"))
        };
        assert!(record.startup_stalled());

        record.log_startup_stall(&ChildLiveness::Alive);
        assert!(
            !record.startup_stalled(),
            "the stall event is emitted at most once"
        );
    }

    #[test]
    fn plain_shell_prompt_is_not_a_stall_but_a_silent_agent_is() {
        let shell = SpawnRecord {
            started: Instant::now() - stall_timeout() - Duration::from_millis(1),
            ..test_record(None)
        };
        shell.note_output(b"C:\\repo>");
        assert!(
            !shell.startup_stalled(),
            "a tiny shell prompt is a healthy start"
        );

        // Exactly what a hung Codex leaves behind on Windows: the ConPTY handshake
        // and not one byte more.
        let agent = SpawnRecord {
            started: Instant::now() - stall_timeout() - Duration::from_millis(1),
            ..test_record(Some("codex"))
        };
        agent.note_output(b"\x1b[?9001h\x1b[?1004h");
        assert!(
            agent.saw_output(),
            "ConPTY greets us even when the child never does"
        );
        assert!(
            agent.startup_stalled(),
            "a coding agent that never paints its TUI is stalled"
        );

        let painted = SpawnRecord {
            started: Instant::now() - stall_timeout() - Duration::from_millis(1),
            ..test_record(Some("codex"))
        };
        painted.note_output(&vec![b'x'; paint_floor() as usize]);
        assert!(
            !painted.startup_stalled(),
            "a coding agent that painted its TUI is healthy"
        );
    }

    #[test]
    fn exit_is_reported_once_and_carries_its_cause() {
        let record = test_record(Some("codex"));
        let liveness = ChildLiveness::Exited {
            code: 1,
            success: false,
            status: "ExitStatus { code: 1 }".to_string(),
        };

        assert!(record.log_child_exit(ExitCause::ChildInitiated, &liveness, "observed-by-monitor"));
        assert!(
            !record.log_child_exit(ExitCause::AcRequested, &liveness, "reaped-after-stop"),
            "a second exit report must be suppressed"
        );
    }

    #[test]
    fn ac_stop_keeps_the_first_source_and_flags_the_record() {
        let record = test_record(Some("codex"));
        assert!(!record.stop_requested());

        record.mark_ac_stop("watchdog");
        record.mark_ac_stop("session-kill");

        assert!(record.stop_requested());
        assert_eq!(record.stop_source(), "watchdog");
    }

    #[test]
    fn spawn_window_counts_only_the_preceding_spawns() {
        let agent = format!("codex-{}", Uuid::new_v4());
        let first = note_spawn_attempt(Some(&agent));
        let second = note_spawn_attempt(Some(&agent));
        let third = note_spawn_attempt(Some("other-agent"));

        assert_eq!(first.same_agent, 0);
        assert_eq!(second.same_agent, 1);
        assert_eq!(third.same_agent, 0);
        assert!(second.total >= 1);
        assert!(third.total >= 2);
        assert_eq!(first.window_ms, concurrency_window().as_millis() as u64);
    }

    #[test]
    fn registry_forgets_a_session() {
        let record = register(SpawnRecordInit {
            session_id: Uuid::new_v4(),
            pid: None,
            argv: vec!["cmd.exe".to_string()],
            cwd: "C:/repo".to_string(),
            agent_id: None,
            codex_home: None,
            configured_env_count: 0,
            removed_env_count: 0,
            window: SpawnWindow {
                window_ms: 10_000,
                total: 0,
                same_agent: 0,
            },
            started: Instant::now(),
        });
        let id = record.session_id();

        assert!(mark_ac_stop(id, "session-kill").is_some());
        forget(id);
        assert!(mark_ac_stop(id, "session-kill").is_none());
    }
}
