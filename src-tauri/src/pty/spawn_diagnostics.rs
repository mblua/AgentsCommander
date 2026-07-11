//! #942 - spawn diagnostics for local PTY sessions.
//!
//! Diagnostics only. Nothing in this module changes how a session is spawned,
//! killed or read; it only records evidence so an intermittent coding-agent
//! blank-terminal hang explains itself after the fact.
//!
//! ## Events
//!
//! | Event | Level | When |
//! |---|---|---|
//! | `[pty] spawn-record` | INFO | once, at spawn: argv as executed, cwd, CLI, profile, `CODEX_HOME`, concurrency |
//! | `[pty] first-output` | DEBUG | first byte off the PTY |
//! | `[pty] first-paint` | INFO (WARN if it recovers a stall) | output passes the paint floor |
//! | `[pty] startup-stall` | WARN | at the deadline, when the child never painted |
//! | `[pty] startup-checkpoint` | INFO | at the deadline otherwise |
//! | `[pty] child-exit` | INFO (WARN when unexpected) | once, when the child ends |
//!
//! The startup report (`startup-stall` / `startup-checkpoint`) fires for **every**
//! session at the deadline, whatever it painted and whoever launched it, and it always
//! carries the retained head bytes. A hang that gets as far as entering the alt screen
//! and clearing it (easily past the paint floor) would otherwise leave no trace at
//! all, and "alt screen entered, nothing drawn" is exactly what a blank terminal looks
//! like.
//!
//! ## Two identities, and why the CLI one is load-bearing
//!
//! `cli=` is [`CodingAgentKind`], detected from the argv by the canonical detector
//! (`session/profile.rs`). `agent_profile=` is the configured coding-agent profile id
//! (`settings.agents[].id`), an opaque string like `agent_1782513272568_0`. They are
//! not interchangeable: a Codex launch can carry no profile id at all
//! (`commands/session.rs`, `is_agent_owned_launch`), and several distinct profiles can
//! all run the `codex` CLI against the same global `~/.codex`. The stall predicate and
//! the concurrency counter therefore key on the **CLI**; the profile id is logged
//! beside it, never used as the identity.
//!
//! ## Windows caveats this module refuses to paper over
//!
//! - ConPTY writes its own 16-byte handshake (`ESC[?9001h ESC[?1004h`) into every PTY
//!   within ~20ms of the spawn, child or no child. The first byte is therefore never
//!   evidence that the child came up; crossing the paint floor is.
//! - `portable_pty::Child::try_wait` cannot tell "running" from "we cannot query this
//!   handle": `WinChild::is_complete` swallows a failed `GetExitCodeProcess` and
//!   returns `Ok(None)`, and its `STILL_ACTIVE` sentinel (259) is also a legal exit
//!   code. AV/EDR stripping query rights off our handles is a known scenario here
//!   (#647). The Windows probe in `local_backend` therefore calls
//!   `WaitForSingleObject` directly and reports [`ChildLiveness::Unqueryable`] when the
//!   OS refuses to answer. The kill path still uses `try_wait`, so a child that exits
//!   with code 259 stays invisible to it: an upstream limitation, documented here
//!   rather than asserted away.
//!
//! ## Lock order
//!
//! `ptys` -> diagnostics registry (`kill_all_jobs` holds the PTY map while tagging
//! records). Never the reverse: the monitor thread probes under `ptys` but only ever
//! touches its own `Arc<SpawnRecord>`, never the registry.
//!
//! ## Tunables (env vars, read once, clamped)
//!
//! | Var | Default | Ceiling |
//! |---|---|---|
//! | `AC_SPAWN_STALL_TIMEOUT_MS` | 5000 (`0` disables the startup report) | 600000 |
//! | `AC_SPAWN_PAINT_FLOOR_BYTES` | 64 | 65536 |
//! | `AC_SPAWN_HEAD_BYTES` | 512 | 65536 |
//! | `AC_SPAWN_CONCURRENCY_WINDOW_MS` | 10000 | 3600000 |
//! | `AC_SPAWN_STOP_ATTRIBUTION_MS` | 60000 | 3600000 |

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use portable_pty::ExitStatus;
use uuid::Uuid;

use crate::session::profile::CodingAgentKind;

const DEFAULT_STALL_TIMEOUT_MS: u64 = 5_000;
const MAX_STALL_TIMEOUT_MS: u64 = 10 * 60 * 1_000;
/// Output below this many bytes is not a painted screen; it is the ConPTY handshake
/// (16 bytes) and nothing else. A coding agent paints a full TUI immediately.
const DEFAULT_PAINT_FLOOR_BYTES: u64 = 64;
const MAX_PAINT_FLOOR_BYTES: u64 = 64 * 1024;
const DEFAULT_HEAD_BYTES: u64 = 512;
const MAX_HEAD_BYTES: u64 = 64 * 1024;
const DEFAULT_CONCURRENCY_WINDOW_MS: u64 = 10_000;
const MAX_CONCURRENCY_WINDOW_MS: u64 = 60 * 60 * 1_000;
/// How long an AC stop request owns the exit that follows it. A resource-monitor kill
/// can be blocked (a Quarantined result leaves the child running); a child that dies
/// half an hour later died on its own, whatever we asked for back then.
const DEFAULT_STOP_ATTRIBUTION_MS: u64 = 60_000;
const MAX_STOP_ATTRIBUTION_MS: u64 = 60 * 60 * 1_000;
/// Safety net on the recent-spawn ring; only the time window is load-bearing.
const RECENT_SPAWNS_CAP: usize = 128;
/// Values shorter than this are not secrets, they are flags like `1` or `true`.
const MIN_REDACTED_LEN: usize = 8;

/// Everything the diagnostics need to be told, resolved once from the environment and
/// then carried by each record, so no code path (and no test) reads a process-global.
#[derive(Debug, Clone, Copy)]
pub struct Thresholds {
    /// Deadline for the startup report. Zero disables it.
    pub stall_timeout: Duration,
    pub paint_floor: u64,
    pub head_bytes: usize,
    pub concurrency_window: Duration,
    pub stop_attribution: Duration,
    pub startup_poll: Duration,
    pub steady_poll: Duration,
}

impl Thresholds {
    pub const DEFAULT: Self = Self {
        stall_timeout: Duration::from_millis(DEFAULT_STALL_TIMEOUT_MS),
        paint_floor: DEFAULT_PAINT_FLOOR_BYTES,
        head_bytes: DEFAULT_HEAD_BYTES as usize,
        concurrency_window: Duration::from_millis(DEFAULT_CONCURRENCY_WINDOW_MS),
        stop_attribution: Duration::from_millis(DEFAULT_STOP_ATTRIBUTION_MS),
        startup_poll: Duration::from_millis(250),
        steady_poll: Duration::from_secs(2),
    };

    /// Read once per process. Every value is clamped: a fat-fingered
    /// `AC_SPAWN_HEAD_BYTES=1000000000` must not buy a 1 GB buffer per session.
    pub fn from_env() -> Self {
        static VALUE: OnceLock<Thresholds> = OnceLock::new();
        *VALUE.get_or_init(|| Thresholds {
            stall_timeout: Duration::from_millis(env_u64(
                "AC_SPAWN_STALL_TIMEOUT_MS",
                DEFAULT_STALL_TIMEOUT_MS,
                MAX_STALL_TIMEOUT_MS,
            )),
            paint_floor: env_u64(
                "AC_SPAWN_PAINT_FLOOR_BYTES",
                DEFAULT_PAINT_FLOOR_BYTES,
                MAX_PAINT_FLOOR_BYTES,
            ),
            head_bytes: env_u64("AC_SPAWN_HEAD_BYTES", DEFAULT_HEAD_BYTES, MAX_HEAD_BYTES) as usize,
            concurrency_window: Duration::from_millis(env_u64(
                "AC_SPAWN_CONCURRENCY_WINDOW_MS",
                DEFAULT_CONCURRENCY_WINDOW_MS,
                MAX_CONCURRENCY_WINDOW_MS,
            )),
            stop_attribution: Duration::from_millis(env_u64(
                "AC_SPAWN_STOP_ATTRIBUTION_MS",
                DEFAULT_STOP_ATTRIBUTION_MS,
                MAX_STOP_ATTRIBUTION_MS,
            )),
            ..Thresholds::DEFAULT
        })
    }
}

fn env_u64(key: &str, default: u64, max: u64) -> u64 {
    let Ok(raw) = std::env::var(key) else {
        return default;
    };
    match raw.trim().parse::<u64>() {
        Ok(value) if value <= max => value,
        Ok(value) => {
            log::warn!("[pty] {key}={value} exceeds the {max} ceiling; clamping to {max}");
            max
        }
        Err(_) => {
            log::warn!("[pty] ignoring non-numeric {key}='{raw}'; using {default}");
            default
        }
    }
}

/// What AC knows about a child process at the instant we ask.
#[derive(Debug, Clone)]
pub enum ChildLiveness {
    Alive,
    Exited { code: u32, success: bool },
    /// The OS would not tell us. Distinct from `Alive` on purpose: reporting an
    /// unanswerable handle as "running" is the same ambiguity class as the
    /// `ExitStatus { code: 1 }` trap this module exists to kill.
    Unqueryable(String),
    /// No PTY instance for this session anymore (killed, or never inserted).
    Gone,
}

impl ChildLiveness {
    pub fn as_log(&self) -> String {
        match self {
            ChildLiveness::Alive => "alive".to_string(),
            ChildLiveness::Exited { code, .. } => format!("exited({code})"),
            ChildLiveness::Unqueryable(detail) => format!("unqueryable({detail})"),
            ChildLiveness::Gone => "gone".to_string(),
        }
    }

    fn exited_ok(&self) -> bool {
        matches!(self, ChildLiveness::Exited { success: true, .. })
    }

    fn is_exited(&self) -> bool {
        matches!(self, ChildLiveness::Exited { .. })
    }
}

impl From<&ExitStatus> for ChildLiveness {
    fn from(status: &ExitStatus) -> Self {
        ChildLiveness::Exited {
            code: status.exit_code(),
            success: status.success(),
        }
    }
}

/// Who ended the child. An AC-initiated stop (session destroy, job terminate,
/// resource-monitor `kill_group`, shutdown) can never be confused with a child that
/// died on its own: that confusion is the bug #942 was filed against.
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

/// The stop state as it was **before** a liveness probe. Read it first, probe second:
/// a stop that lands after the read cannot have caused an exit the probe then sees.
#[derive(Debug, Clone, Copy)]
pub struct StopSnapshot {
    requested: bool,
}

/// How busy the spawner was just before this spawn. `same_cli` is keyed on the CLI
/// identity, because the suspect is contention between concurrent Codex startups on
/// the shared `~/.codex`, not between AC profiles.
#[derive(Debug, Clone, Copy)]
pub struct SpawnWindow {
    pub window_ms: u64,
    pub total: usize,
    pub same_cli: usize,
}

pub struct SpawnRecordInit {
    pub session_id: Uuid,
    pub pid: Option<u32>,
    /// The argv as executed, including any `cmd.exe /C` wrapper AC added.
    pub argv: Vec<String>,
    pub cwd: String,
    /// CLI identity from the canonical detector. Keys the stall predicate.
    pub cli: Option<CodingAgentKind>,
    /// Configured coding-agent profile id (opaque). Logged, never used as identity.
    pub agent_profile_id: Option<String>,
    /// Effective `CODEX_HOME` for the child (configured, inherited, or removed).
    pub codex_home: Option<String>,
    pub configured_env_count: usize,
    pub removed_env_count: usize,
    /// Values to mask out of anything we echo into the log (child output, argv): AC's
    /// own credentials plus every configured env value, which is where AC tells users
    /// to keep their API keys.
    pub redact: Vec<String>,
    pub window: SpawnWindow,
    /// Taken immediately before the child was spawned; time zero for first-output.
    pub started: Instant,
    pub thresholds: Thresholds,
}

pub struct SpawnRecord {
    session_id: Uuid,
    pid: Option<u32>,
    argv: Vec<String>,
    cwd: String,
    cli: Option<CodingAgentKind>,
    agent_profile_id: Option<String>,
    codex_home: Option<String>,
    configured_env_count: usize,
    removed_env_count: usize,
    redact: Vec<String>,
    window: SpawnWindow,
    started: Instant,
    thresholds: Thresholds,
    /// Microseconds from spawn to the first byte; 0 means nothing read yet.
    first_output_us: AtomicU64,
    /// Microseconds from spawn to the first real output (past the paint floor);
    /// 0 means the child has still painted nothing.
    first_paint_us: AtomicU64,
    bytes_read: AtomicU64,
    head: Mutex<Vec<u8>>,
    head_full: AtomicBool,
    ac_stop: AtomicBool,
    /// Microseconds from spawn to the stop request; 0 means none.
    ac_stop_at_us: AtomicU64,
    ac_stop_source: Mutex<Option<String>>,
    /// Liveness observed by the kill path **before** it touched job or child: the
    /// authoritative answer to "was it already dead when we asked?".
    pre_stop_liveness: Mutex<Option<ChildLiveness>>,
    startup_reported: AtomicBool,
    stall_reported: AtomicBool,
    exit_reported: AtomicBool,
    exit_cause: Mutex<Option<ExitCause>>,
}

impl SpawnRecord {
    fn new(init: SpawnRecordInit) -> Self {
        Self {
            session_id: init.session_id,
            pid: init.pid,
            argv: init.argv,
            cwd: init.cwd,
            cli: init.cli,
            agent_profile_id: init.agent_profile_id,
            codex_home: init.codex_home,
            configured_env_count: init.configured_env_count,
            removed_env_count: init.removed_env_count,
            redact: init
                .redact
                .into_iter()
                .filter(|value| value.len() >= MIN_REDACTED_LEN)
                .collect(),
            window: init.window,
            started: init.started,
            thresholds: init.thresholds,
            first_output_us: AtomicU64::new(0),
            first_paint_us: AtomicU64::new(0),
            bytes_read: AtomicU64::new(0),
            head: Mutex::new(Vec::new()),
            head_full: AtomicBool::new(false),
            ac_stop: AtomicBool::new(false),
            ac_stop_at_us: AtomicU64::new(0),
            ac_stop_source: Mutex::new(None),
            pre_stop_liveness: Mutex::new(None),
            startup_reported: AtomicBool::new(false),
            stall_reported: AtomicBool::new(false),
            exit_reported: AtomicBool::new(false),
            exit_cause: Mutex::new(None),
        }
    }

    pub fn session_id(&self) -> Uuid {
        self.session_id
    }

    pub fn thresholds(&self) -> Thresholds {
        self.thresholds
    }

    fn cli_log(&self) -> &'static str {
        match self.cli {
            Some(kind) => kind.as_str(),
            None => "none",
        }
    }

    fn profile_log(&self) -> &str {
        self.agent_profile_id.as_deref().unwrap_or("none")
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

    fn ms_log(stamp_us: &AtomicU64) -> String {
        match stamp_us.load(Ordering::Relaxed) {
            0 => "none".to_string(),
            us => (us / 1_000).to_string(),
        }
    }

    /// Mask AC credentials and configured env values out of anything we echo into the
    /// log. `app.log` is what users paste into issues.
    fn scrub(&self, text: String) -> String {
        let mut text = text;
        for secret in &self.redact {
            if text.contains(secret.as_str()) {
                text = text.replace(secret.as_str(), "<redacted>");
            }
        }
        text
    }

    fn argv_log(&self) -> String {
        self.scrub(format!("{:?}", self.argv))
    }

    fn head_log(&self) -> String {
        let head = self.head.lock().unwrap_or_else(|e| e.into_inner());
        if head.is_empty() {
            return "<empty>".to_string();
        }
        let text = String::from_utf8_lossy(&head).escape_debug().to_string();
        format!("\"{}\"", self.scrub(text))
    }

    pub fn saw_output(&self) -> bool {
        self.first_output_us.load(Ordering::Relaxed) != 0
    }

    pub fn painted(&self) -> bool {
        self.first_paint_us.load(Ordering::Relaxed) != 0
    }

    pub fn stop_requested(&self) -> bool {
        self.ac_stop.load(Ordering::SeqCst)
    }

    pub fn stop_snapshot(&self) -> StopSnapshot {
        StopSnapshot {
            requested: self.stop_requested(),
        }
    }

    fn stop_source(&self) -> String {
        self.ac_stop_source
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
            .unwrap_or_else(|| "none".to_string())
    }

    fn pre_stop_liveness(&self) -> Option<ChildLiveness> {
        self.pre_stop_liveness
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Age of the stop request, or `None` when AC never asked.
    fn stop_age(&self) -> Option<Duration> {
        match self.ac_stop_at_us.load(Ordering::SeqCst) {
            0 => None,
            at => Some(
                self.started
                    .elapsed()
                    .saturating_sub(Duration::from_micros(at)),
            ),
        }
    }

    fn stop_age_log(&self) -> String {
        match self.stop_age() {
            Some(age) => age.as_millis().to_string(),
            None => "none".to_string(),
        }
    }

    /// A stop request only owns the exits that follow it closely. AC has a documented
    /// path where the kill is blocked and the agent keeps running (a Quarantined
    /// resource-monitor kill leaves the instance intact); a child that dies an hour
    /// later died on its own, whatever we asked for back then.
    fn stop_cause(&self) -> ExitCause {
        match self.stop_age() {
            Some(age) if age <= self.thresholds.stop_attribution => ExitCause::AcRequested,
            _ => ExitCause::ChildInitiated,
        }
    }

    /// Cause for an observed exit. `before` is the stop state read BEFORE the liveness
    /// probe. The pre-stop liveness always wins when present: the kill path probes the
    /// child before it touches anything, so it is the one witness that cannot be racy.
    pub fn attribute_exit(&self, before: StopSnapshot) -> ExitCause {
        if let Some(pre_stop) = self.pre_stop_liveness() {
            if pre_stop.is_exited() {
                // It was already dead when AC asked. Our stop did not do this.
                return ExitCause::ChildInitiated;
            }
            return self.stop_cause();
        }
        if before.requested {
            self.stop_cause()
        } else {
            ExitCause::ChildInitiated
        }
    }

    /// Poll fast until the startup verdict is in, slowly afterwards.
    pub fn in_startup_phase(&self) -> bool {
        !self.startup_reported.load(Ordering::Relaxed)
    }

    fn mark_ac_stop(&self, source: &str, pre_stop: Option<ChildLiveness>) {
        {
            let mut current = self.ac_stop_source.lock().unwrap_or_else(|e| e.into_inner());
            if current.is_none() {
                *current = Some(source.to_string());
            }
        }
        if let Some(pre_stop) = pre_stop {
            let mut current = self
                .pre_stop_liveness
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if current.is_none() {
                *current = Some(pre_stop);
            }
        }
        let stamp = (self.started.elapsed().as_micros().min(u64::MAX as u128) as u64).max(1);
        let _ = self
            .ac_stop_at_us
            .compare_exchange(0, stamp, Ordering::SeqCst, Ordering::Relaxed);
        // Published last: any thread that sees this flag also sees the source, the
        // timestamp and the pre-stop liveness written above.
        self.ac_stop.store(true, Ordering::SeqCst);
    }

    /// A stop AC asked for, recent enough that the teardown it triggers is still in
    /// flight. Keeps ConPTY's teardown repaint out of the startup timings without
    /// going permanently blind on a session whose kill was blocked.
    fn stop_in_flight(&self, elapsed: Duration) -> bool {
        match self.ac_stop_at_us.load(Ordering::SeqCst) {
            0 => false,
            at => {
                elapsed.saturating_sub(Duration::from_micros(at)) <= self.thresholds.stop_attribution
            }
        }
    }

    /// Hot path: called for every chunk read off the PTY. Once the stamps are taken and
    /// the head buffer is full it costs one relaxed add and a few relaxed loads: no
    /// clock, no lock, no allocation.
    pub fn note_output(&self, chunk: &[u8]) {
        // Only a reported exit closes the record. A stop request must NOT: AC has a
        // path where the kill is blocked and the session keeps running, and going blind
        // on a live agent is exactly the evidence loss this module exists to prevent.
        if chunk.is_empty() || self.exit_reported.load(Ordering::Relaxed) {
            return;
        }
        let total = self
            .bytes_read
            .fetch_add(chunk.len() as u64, Ordering::Relaxed)
            + chunk.len() as u64;

        let needs_first = self.first_output_us.load(Ordering::Relaxed) == 0;
        let needs_paint =
            self.first_paint_us.load(Ordering::Relaxed) == 0 && total >= self.thresholds.paint_floor;

        if needs_first || needs_paint {
            let elapsed = self.started.elapsed();
            // ConPTY repaints the master while a session is being torn down. That is
            // the teardown talking, not the child starting up.
            if !self.stop_in_flight(elapsed) {
                // 0 is the "nothing yet" sentinel for both stamps, so never store it.
                let stamp = (elapsed.as_micros().min(u64::MAX as u128) as u64).max(1);

                // The first byte off the PTY. On Windows this is ConPTY greeting us,
                // not the child, which is why it is DEBUG and never proof of life.
                if needs_first
                    && self
                        .first_output_us
                        .compare_exchange(0, stamp, Ordering::SeqCst, Ordering::Relaxed)
                        .is_ok()
                {
                    log::debug!(
                        "[pty] first-output session={} pid={} cli={} after_ms={} bytes={}",
                        self.session_id,
                        self.pid_log(),
                        self.cli_log(),
                        elapsed.as_millis(),
                        total
                    );
                }

                // The first real output: the child actually painted something. This is
                // the healthy-startup signal, and its absence is the hang.
                if needs_paint
                    && self
                        .first_paint_us
                        .compare_exchange(0, stamp, Ordering::SeqCst, Ordering::Relaxed)
                        .is_ok()
                {
                    // Only a stall we actually reported can be recovered from.
                    if self.stall_reported.load(Ordering::Relaxed) {
                        log::warn!(
                            "[pty] first-paint session={} pid={} cli={} after_ms={} bytes={} late=true (startup stall recovered)",
                            self.session_id,
                            self.pid_log(),
                            self.cli_log(),
                            elapsed.as_millis(),
                            total
                        );
                    } else {
                        log::info!(
                            "[pty] first-paint session={} pid={} cli={} after_ms={} bytes={}",
                            self.session_id,
                            self.pid_log(),
                            self.cli_log(),
                            elapsed.as_millis(),
                            total
                        );
                    }
                }
            }
        }

        // The head is the child's opening bytes. Once AC has asked for a stop, what
        // arrives is teardown, so freeze it rather than pollute the evidence.
        if !self.head_full.load(Ordering::Relaxed) && !self.stop_requested() {
            self.capture_head(chunk);
        }
    }

    fn capture_head(&self, chunk: &[u8]) {
        let cap = self.thresholds.head_bytes;
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

    /// The child never painted a screen: nothing at all, or nothing past the ConPTY
    /// handshake for a coding agent (which always paints a TUI immediately). A plain
    /// shell whose prompt is legitimately tiny is not stalled, it is just quiet.
    fn is_stalled(&self) -> bool {
        let bytes = self.bytes_read.load(Ordering::Relaxed);
        bytes == 0 || (self.cli.is_some() && !self.painted())
    }

    /// True once the startup window has elapsed and the verdict is still unwritten.
    pub fn startup_deadline_reached(&self) -> bool {
        let timeout = self.thresholds.stall_timeout;
        !timeout.is_zero()
            && !self.startup_reported.load(Ordering::Relaxed)
            && self.started.elapsed() >= timeout
    }

    /// The startup verdict, emitted at most once, for EVERY session at the deadline:
    /// WARN when the child never painted, INFO otherwise. It always carries the head
    /// bytes, so a hang that painted past the floor still leaves its output on record.
    pub fn log_startup_report(&self, liveness: &ChildLiveness) -> bool {
        if self.startup_reported.swap(true, Ordering::SeqCst) {
            return false;
        }
        let stalled = self.is_stalled();
        self.stall_reported.store(stalled, Ordering::SeqCst);

        let event = if stalled {
            "startup-stall"
        } else {
            "startup-checkpoint"
        };
        let line = format!(
            "[pty] {} session={} pid={} cli={} agent_profile={} backend=local child={} stalled={} elapsed_ms={} threshold_ms={} bytes_read={} paint_floor={} first_output_ms={} first_paint_ms={} argv={} cwd={} codex_home={} recent_spawns={} recent_same_cli={} window_ms={} head={}",
            event,
            self.session_id,
            self.pid_log(),
            self.cli_log(),
            self.profile_log(),
            liveness.as_log(),
            stalled,
            self.started.elapsed().as_millis(),
            self.thresholds.stall_timeout.as_millis(),
            self.bytes_read.load(Ordering::Relaxed),
            self.thresholds.paint_floor,
            Self::ms_log(&self.first_output_us),
            Self::ms_log(&self.first_paint_us),
            self.argv_log(),
            self.cwd,
            self.codex_home_log(),
            self.window.total,
            self.window.same_cli,
            self.window.window_ms,
            self.head_log()
        );
        if stalled {
            log::warn!("{}", line);
        } else {
            log::info!("{}", line);
        }
        true
    }

    /// The one exit event for this session. Returns false when another path already
    /// reported it (the caller may then leave a debug crumb instead).
    pub fn log_child_exit(&self, cause: ExitCause, liveness: &ChildLiveness, detail: &str) -> bool {
        if self.exit_reported.swap(true, Ordering::SeqCst) {
            return false;
        }
        *self.exit_cause.lock().unwrap_or_else(|e| e.into_inner()) = Some(cause);

        let uptime = self.started.elapsed();
        let bytes = self.bytes_read.load(Ordering::Relaxed);
        // A child that ends itself without ever painting a screen, or with a failing
        // status, is the #942 smoking gun. A clean exit from a session that did come up
        // is just someone quitting.
        let unexpected =
            cause == ExitCause::ChildInitiated && (!self.painted() || !liveness.exited_ok());

        if unexpected {
            log::warn!(
                "[pty] child-exit session={} pid={} cli={} agent_profile={} cause={} detail={} child={} uptime_ms={} first_output_ms={} first_paint_ms={} bytes_read={} stop_source={} stop_age_ms={} argv={} cwd={} codex_home={} recent_spawns={} recent_same_cli={} window_ms={} head={}",
                self.session_id,
                self.pid_log(),
                self.cli_log(),
                self.profile_log(),
                cause.as_log(),
                detail,
                liveness.as_log(),
                uptime.as_millis(),
                Self::ms_log(&self.first_output_us),
                Self::ms_log(&self.first_paint_us),
                bytes,
                self.stop_source(),
                self.stop_age_log(),
                self.argv_log(),
                self.cwd,
                self.codex_home_log(),
                self.window.total,
                self.window.same_cli,
                self.window.window_ms,
                self.head_log()
            );
        } else {
            log::info!(
                "[pty] child-exit session={} pid={} cli={} agent_profile={} cause={} detail={} child={} uptime_ms={} first_output_ms={} first_paint_ms={} bytes_read={} stop_source={} stop_age_ms={}",
                self.session_id,
                self.pid_log(),
                self.cli_log(),
                self.profile_log(),
                cause.as_log(),
                detail,
                liveness.as_log(),
                uptime.as_millis(),
                Self::ms_log(&self.first_output_us),
                Self::ms_log(&self.first_paint_us),
                bytes,
                self.stop_source(),
                self.stop_age_log()
            );
        }
        true
    }

    /// The full spawn record, one line, on every local spawn.
    fn log_spawn(&self) {
        log::info!(
            "[pty] spawn-record session={} pid={} cli={} agent_profile={} backend=local argv={} cwd={} codex_home={} env_configured={} env_removed={} recent_spawns={} recent_same_cli={} window_ms={}",
            self.session_id,
            self.pid_log(),
            self.cli_log(),
            self.profile_log(),
            self.argv_log(),
            self.cwd,
            self.codex_home_log(),
            self.configured_env_count,
            self.removed_env_count,
            self.window.total,
            self.window.same_cli,
            self.window.window_ms
        );
    }
}

#[derive(Default)]
struct Registry {
    records: HashMap<Uuid, Arc<SpawnRecord>>,
    recent: VecDeque<(Instant, Option<CodingAgentKind>)>,
}

fn registry() -> &'static Mutex<Registry> {
    static REGISTRY: OnceLock<Mutex<Registry>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(Registry::default()))
}

/// Count the spawns that happened in the window BEFORE this one, then record this
/// attempt. Call once per spawn, at the top of the spawn path.
pub fn note_spawn_attempt(cli: Option<CodingAgentKind>, thresholds: Thresholds) -> SpawnWindow {
    let window = thresholds.concurrency_window;
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
    let same_cli = match cli {
        Some(kind) => registry
            .recent
            .iter()
            .filter(|(_, recent_cli)| *recent_cli == Some(kind))
            .count(),
        None => 0,
    };

    registry.recent.push_back((now, cli));
    if registry.recent.len() > RECENT_SPAWNS_CAP {
        registry.recent.pop_front();
    }

    SpawnWindow {
        window_ms: window.as_millis().min(u64::MAX as u128) as u64,
        total,
        same_cli,
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
/// resource-monitor `kill_group`, shutdown) BEFORE any process is touched, so its exit
/// can never be misread as a spontaneous death. `pre_stop` is the liveness the caller
/// observed before touching anything: pass it whenever you have it, it is the only
/// non-racy witness of "was it already dead when we asked?". No-op for unknown sessions
/// (container transport, or a session with no local spawn record).
pub fn mark_ac_stop(
    session_id: Uuid,
    source: &str,
    pre_stop: Option<ChildLiveness>,
) -> Option<Arc<SpawnRecord>> {
    let record = registry()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .records
        .get(&session_id)
        .cloned()?;
    record.mark_ac_stop(source, pre_stop);
    Some(record)
}

pub fn forget(session_id: Uuid) {
    registry()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .records
        .remove(&session_id);
}

/// One monitor thread per local session. It owns two signals the PTY reader cannot
/// carry: the startup verdict at the deadline, and the exit of a child that dies
/// without AC asking (ConPTY keeps the pipe open past the death of the child, so reader
/// EOF is not a reliable witness). It ends when the child exits, when the session is
/// gone, or when the probe itself panics.
pub fn watch_child<P>(record: Arc<SpawnRecord>, probe: P) -> JoinHandle<()>
where
    P: Fn() -> ChildLiveness + Send + 'static,
{
    std::thread::spawn(move || loop {
        let tick = if record.in_startup_phase() {
            record.thresholds.startup_poll
        } else {
            record.thresholds.steady_poll
        };
        std::thread::sleep(tick);

        // Read the stop state BEFORE looking at the child: a stop that lands after this
        // point cannot be what killed a child the probe is about to find already dead.
        let stop_before = record.stop_snapshot();

        // portable-pty unwraps its own internals while polling a child. A poisoned
        // mutex in there must not take this thread down silently.
        let liveness = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(&probe)) {
            Ok(liveness) => liveness,
            Err(_) => {
                log::warn!(
                    "[pty] child-probe panicked for session {}; stopping its monitor",
                    record.session_id()
                );
                return;
            }
        };

        match liveness {
            ChildLiveness::Gone => return,
            ChildLiveness::Exited { .. } => {
                let cause = record.attribute_exit(stop_before);
                record.log_child_exit(cause, &liveness, "observed-by-monitor");
                return;
            }
            ChildLiveness::Alive | ChildLiveness::Unqueryable(_) => {
                if record.startup_deadline_reached() {
                    record.log_startup_report(&liveness);
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    /// Fast, deterministic, and above all NOT read from the process environment: a test
    /// must not change its verdict because someone exported a tunable.
    fn test_thresholds() -> Thresholds {
        Thresholds {
            stall_timeout: Duration::from_millis(120),
            paint_floor: 64,
            head_bytes: 512,
            concurrency_window: Duration::from_millis(10_000),
            stop_attribution: Duration::from_millis(500),
            startup_poll: Duration::from_millis(5),
            steady_poll: Duration::from_millis(10),
        }
    }

    fn test_init(cli: Option<CodingAgentKind>) -> SpawnRecordInit {
        SpawnRecordInit {
            session_id: Uuid::new_v4(),
            pid: Some(4242),
            argv: vec!["codex".to_string(), "resume".to_string()],
            cwd: "C:/repo".to_string(),
            cli,
            agent_profile_id: cli.map(|_| "agent_1782513272568_0".to_string()),
            codex_home: None,
            configured_env_count: 0,
            removed_env_count: 0,
            redact: Vec::new(),
            window: SpawnWindow {
                window_ms: 10_000,
                total: 0,
                same_cli: 0,
            },
            started: Instant::now(),
            thresholds: test_thresholds(),
        }
    }

    fn test_record(cli: Option<CodingAgentKind>) -> SpawnRecord {
        SpawnRecord::new(test_init(cli))
    }

    /// A record whose startup deadline is already in the past.
    fn aged_record(cli: Option<CodingAgentKind>) -> SpawnRecord {
        let thresholds = test_thresholds();
        SpawnRecord {
            started: Instant::now() - thresholds.stall_timeout - Duration::from_millis(1),
            ..test_record(cli)
        }
    }

    const CONPTY_PROLOGUE: &[u8] = b"\x1b[?9001h\x1b[?1004h";

    fn wait_until(deadline: Duration, mut done: impl FnMut() -> bool) -> bool {
        let start = Instant::now();
        while start.elapsed() < deadline {
            if done() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        done()
    }

    #[test]
    fn head_capture_is_capped_and_stops_after_full() {
        let record = test_record(Some(CodingAgentKind::Codex));
        let cap = record.thresholds.head_bytes;
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
        let record = test_record(Some(CodingAgentKind::Codex));

        // The ConPTY handshake alone is not a paint.
        record.note_output(CONPTY_PROLOGUE);
        assert!(record.saw_output());
        assert!(!record.painted());

        record.note_output(&vec![b'x'; record.thresholds.paint_floor as usize]);
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
    fn a_coding_agent_that_never_paints_is_stalled_and_a_quiet_shell_is_not() {
        // Exactly what a hung Codex leaves behind on Windows: the ConPTY handshake and
        // not one byte more.
        let agent = aged_record(Some(CodingAgentKind::Codex));
        agent.note_output(CONPTY_PROLOGUE);
        assert!(
            agent.saw_output(),
            "ConPTY greets us even when the child never does"
        );
        assert!(agent.is_stalled());
        assert!(agent.startup_deadline_reached());

        let shell = aged_record(None);
        shell.note_output(b"C:\\repo>");
        assert!(!shell.is_stalled(), "a tiny shell prompt is a healthy start");

        let painted = aged_record(Some(CodingAgentKind::Codex));
        painted.note_output(&vec![b'x'; painted.thresholds.paint_floor as usize]);
        assert!(
            !painted.is_stalled(),
            "a coding agent that painted its TUI is healthy"
        );
    }

    #[test]
    fn the_startup_report_fires_once_for_every_session_whatever_it_painted() {
        // grinch F2: a hang that paints past the floor must still leave its bytes on
        // record, and so must a session with no coding-agent identity at all.
        let painted = aged_record(None);
        painted.note_output(&[b'x'; 200]);

        assert!(painted.startup_deadline_reached());
        assert!(painted.log_startup_report(&ChildLiveness::Alive));
        assert!(
            !painted.stall_reported.load(Ordering::Relaxed),
            "a painted session is a checkpoint, not a stall"
        );
        assert!(
            !painted.log_startup_report(&ChildLiveness::Alive),
            "the startup verdict is written once"
        );
        assert!(!painted.startup_deadline_reached());

        let stalled = aged_record(Some(CodingAgentKind::Codex));
        stalled.note_output(CONPTY_PROLOGUE);
        assert!(stalled.log_startup_report(&ChildLiveness::Alive));
        assert!(stalled.stall_reported.load(Ordering::Relaxed));
    }

    #[test]
    fn a_disabled_stall_timeout_disables_the_startup_report() {
        let record = SpawnRecord {
            thresholds: Thresholds {
                stall_timeout: Duration::ZERO,
                ..test_thresholds()
            },
            ..aged_record(Some(CodingAgentKind::Codex))
        };
        assert!(!record.startup_deadline_reached());
    }

    #[test]
    fn exit_is_reported_once_and_carries_its_cause() {
        let record = test_record(Some(CodingAgentKind::Codex));
        let liveness = ChildLiveness::Exited {
            code: 1,
            success: false,
        };

        assert!(record.log_child_exit(ExitCause::ChildInitiated, &liveness, "observed-by-monitor"));
        assert!(
            !record.log_child_exit(ExitCause::AcRequested, &liveness, "reaped-after-stop"),
            "a second exit report must be suppressed"
        );
        assert_eq!(
            *record.exit_cause.lock().unwrap(),
            Some(ExitCause::ChildInitiated)
        );
    }

    #[test]
    fn output_keeps_flowing_after_a_stop_that_did_not_kill_the_child() {
        // grinch F5: a Quarantined resource-monitor kill leaves the agent running. The
        // record must not go blind, and a much later death is the child's own.
        let record = test_record(Some(CodingAgentKind::Codex));
        record.mark_ac_stop("watchdog", Some(ChildLiveness::Alive));

        record.note_output(&[b'x'; 128]);
        assert_eq!(record.bytes_read.load(Ordering::Relaxed), 128);
        assert_eq!(
            record.attribute_exit(record.stop_snapshot()),
            ExitCause::AcRequested,
            "inside the attribution window the exit is ours"
        );

        let stale = test_record(Some(CodingAgentKind::Codex));
        stale.mark_ac_stop("watchdog", Some(ChildLiveness::Alive));
        // Rewind the clock: the stop request is now older than the attribution window.
        let long_ago = stale.thresholds.stop_attribution + Duration::from_secs(1);
        let stale = SpawnRecord {
            started: Instant::now() - long_ago,
            ..stale
        };
        assert_eq!(
            stale.attribute_exit(stale.stop_snapshot()),
            ExitCause::ChildInitiated,
            "a kill that evidently failed does not own a death an hour later"
        );
    }

    #[test]
    fn a_child_already_dead_at_the_stop_is_never_attributed_to_ac() {
        // grinch F6: the kill path probes before it touches anything, and that witness
        // outranks any flag the monitor reads afterwards.
        let record = test_record(Some(CodingAgentKind::Codex));
        record.mark_ac_stop(
            "session-kill",
            Some(ChildLiveness::Exited {
                code: 1,
                success: false,
            }),
        );

        assert_eq!(
            record.attribute_exit(record.stop_snapshot()),
            ExitCause::ChildInitiated,
            "it was already dead when AC asked"
        );
    }

    #[test]
    fn an_exit_seen_before_the_stop_flag_is_the_child_s_own() {
        let record = test_record(Some(CodingAgentKind::Codex));
        let before = record.stop_snapshot(); // no stop yet

        // The stop lands after the snapshot, with no pre-stop witness (the
        // resource-monitor path). The exit we already saw cannot be ours.
        record.mark_ac_stop("watchdog", None);

        assert_eq!(record.attribute_exit(before), ExitCause::ChildInitiated);
    }

    #[test]
    fn ac_stop_keeps_the_first_source_and_flags_the_record() {
        let record = test_record(Some(CodingAgentKind::Codex));
        assert!(!record.stop_requested());

        record.mark_ac_stop("watchdog", None);
        record.mark_ac_stop("session-kill", None);

        assert!(record.stop_requested());
        assert_eq!(record.stop_source(), "watchdog");
    }

    #[test]
    fn credentials_and_configured_env_values_are_scrubbed_from_echoed_output() {
        let token = "3f1c9d2e-7a4b-4c8d-9e1f-2b3c4d5e6f70";
        let record = SpawnRecord::new(SpawnRecordInit {
            argv: vec!["codex".to_string(), format!("--token={token}")],
            redact: vec![token.to_string(), "x".to_string()],
            ..test_init(Some(CodingAgentKind::Codex))
        });
        record.note_output(format!("AGENTSCOMMANDER_TOKEN={token}\r\n").as_bytes());

        assert!(record.head_log().contains("<redacted>"));
        assert!(!record.head_log().contains(token));
        assert!(record.argv_log().contains("<redacted>"));
        assert!(!record.argv_log().contains(token));
        assert!(
            record.head_log().contains("AGENTSCOMMANDER_TOKEN"),
            "a one-character redaction entry must not shred the whole line"
        );
    }

    #[test]
    fn thresholds_from_env_are_clamped() {
        assert_eq!(env_u64("AC_SPAWN_TEST_MISSING_KEY", 7, 100), 7);
        std::env::set_var("AC_SPAWN_TEST_CLAMP", "1000000000");
        assert_eq!(env_u64("AC_SPAWN_TEST_CLAMP", 512, 65_536), 65_536);
        std::env::set_var("AC_SPAWN_TEST_CLAMP", "not-a-number");
        assert_eq!(env_u64("AC_SPAWN_TEST_CLAMP", 512, 65_536), 512);
        std::env::remove_var("AC_SPAWN_TEST_CLAMP");
    }

    #[test]
    fn spawn_window_counts_the_preceding_spawns_by_cli() {
        let thresholds = test_thresholds();
        let first = note_spawn_attempt(Some(CodingAgentKind::Gemini), thresholds);
        let second = note_spawn_attempt(Some(CodingAgentKind::Gemini), thresholds);
        let third = note_spawn_attempt(Some(CodingAgentKind::Claude), thresholds);

        assert_eq!(first.same_cli, 0);
        assert_eq!(second.same_cli, 1, "two Geminis in the window");
        assert_eq!(third.same_cli, 0, "a Claude is not a Gemini");
        assert!(second.total >= 1);
        assert!(third.total >= 2);
        assert_eq!(
            first.window_ms,
            thresholds.concurrency_window.as_millis() as u64
        );
    }

    #[test]
    fn registry_forgets_a_session() {
        let record = register(test_init(None));
        let id = record.session_id();

        assert!(mark_ac_stop(id, "session-kill", None).is_some());
        forget(id);
        assert!(mark_ac_stop(id, "session-kill", None).is_none());
    }

    // ---- the monitor thread (grinch N1) ----

    struct FakeProbe {
        answers: Mutex<Vec<ChildLiveness>>,
        calls: AtomicUsize,
    }

    impl FakeProbe {
        /// Answers are consumed in order; the last one repeats forever.
        fn new(answers: Vec<ChildLiveness>) -> Arc<Self> {
            Arc::new(Self {
                answers: Mutex::new(answers),
                calls: AtomicUsize::new(0),
            })
        }

        fn probe(self: &Arc<Self>) -> impl Fn() -> ChildLiveness + Send + 'static {
            let probe = Arc::clone(self);
            move || {
                probe.calls.fetch_add(1, Ordering::SeqCst);
                let mut answers = probe.answers.lock().unwrap();
                if answers.len() > 1 {
                    answers.remove(0)
                } else {
                    answers.first().cloned().unwrap_or(ChildLiveness::Gone)
                }
            }
        }

        fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }
    }

    #[test]
    fn monitor_reports_a_child_that_dies_on_its_own_and_then_ends() {
        let record = Arc::new(test_record(Some(CodingAgentKind::Codex)));
        let probe = FakeProbe::new(vec![
            ChildLiveness::Alive,
            ChildLiveness::Alive,
            ChildLiveness::Exited {
                code: 1,
                success: false,
            },
        ]);

        let handle = watch_child(Arc::clone(&record), probe.probe());
        assert!(
            wait_until(Duration::from_secs(2), || handle.is_finished()),
            "the monitor must end with the child"
        );
        handle.join().expect("monitor thread");

        assert_eq!(
            *record.exit_cause.lock().unwrap(),
            Some(ExitCause::ChildInitiated)
        );
        let calls = probe.calls();
        std::thread::sleep(Duration::from_millis(40));
        assert_eq!(calls, probe.calls(), "no polling after the exit");
    }

    #[test]
    fn monitor_attributes_an_exit_after_an_ac_stop_to_ac() {
        let record = Arc::new(test_record(Some(CodingAgentKind::Codex)));
        record.mark_ac_stop("session-kill", Some(ChildLiveness::Alive));
        let probe = FakeProbe::new(vec![ChildLiveness::Exited {
            code: 1,
            success: false,
        }]);

        let handle = watch_child(Arc::clone(&record), probe.probe());
        handle.join().expect("monitor thread");

        assert_eq!(
            *record.exit_cause.lock().unwrap(),
            Some(ExitCause::AcRequested)
        );
    }

    #[test]
    fn monitor_keeps_child_initiated_when_the_child_was_dead_before_the_stop() {
        let record = Arc::new(test_record(Some(CodingAgentKind::Codex)));
        record.mark_ac_stop(
            "session-kill",
            Some(ChildLiveness::Exited {
                code: 0,
                success: true,
            }),
        );
        let probe = FakeProbe::new(vec![ChildLiveness::Exited {
            code: 0,
            success: true,
        }]);

        let handle = watch_child(Arc::clone(&record), probe.probe());
        handle.join().expect("monitor thread");

        assert_eq!(
            *record.exit_cause.lock().unwrap(),
            Some(ExitCause::ChildInitiated),
            "AC asked, but it was already dead: the stop did not cause this"
        );
    }

    #[test]
    fn monitor_ends_when_the_session_is_gone_without_reporting_an_exit() {
        let record = Arc::new(test_record(None));
        let probe = FakeProbe::new(vec![ChildLiveness::Gone]);

        let handle = watch_child(Arc::clone(&record), probe.probe());
        handle.join().expect("monitor thread");

        assert!(!record.exit_reported.load(Ordering::Relaxed));
        assert!(record.exit_cause.lock().unwrap().is_none());
    }

    #[test]
    fn monitor_writes_the_startup_verdict_for_a_silent_child_and_keeps_watching() {
        let record = Arc::new(test_record(Some(CodingAgentKind::Codex)));
        record.note_output(CONPTY_PROLOGUE);
        let probe = FakeProbe::new(vec![ChildLiveness::Alive]);

        let handle = watch_child(Arc::clone(&record), probe.probe());
        assert!(
            wait_until(Duration::from_secs(2), || record
                .startup_reported
                .load(Ordering::Relaxed)),
            "the startup verdict must land at the deadline"
        );
        assert!(record.stall_reported.load(Ordering::Relaxed));
        assert!(
            !handle.is_finished(),
            "a stalled but living child stays under watch"
        );

        // The monitor still ends when the session goes away.
        *probe.answers.lock().unwrap() = vec![ChildLiveness::Gone];
        assert!(wait_until(Duration::from_secs(2), || handle.is_finished()));
        handle.join().expect("monitor thread");
    }

    #[test]
    fn monitor_survives_a_panicking_probe() {
        let record = Arc::new(test_record(Some(CodingAgentKind::Codex)));
        let handle = watch_child(Arc::clone(&record), || panic!("poisoned child mutex"));

        assert!(
            wait_until(Duration::from_secs(2), || handle.is_finished()),
            "a panicking probe must end the monitor, not the app"
        );
        handle.join().expect("the panic is caught inside the thread");
        assert!(!record.exit_reported.load(Ordering::Relaxed));
    }

    #[test]
    fn monitor_reports_an_unqueryable_child_without_calling_it_alive() {
        let record = Arc::new(test_record(Some(CodingAgentKind::Codex)));
        record.note_output(CONPTY_PROLOGUE);
        let probe = FakeProbe::new(vec![ChildLiveness::Unqueryable("os error 5".to_string())]);

        let handle = watch_child(Arc::clone(&record), probe.probe());
        assert!(
            wait_until(Duration::from_secs(2), || record
                .startup_reported
                .load(Ordering::Relaxed)),
            "an unqueryable child still gets its startup verdict"
        );
        assert!(!record.exit_reported.load(Ordering::Relaxed));

        *probe.answers.lock().unwrap() = vec![ChildLiveness::Gone];
        assert!(wait_until(Duration::from_secs(2), || handle.is_finished()));
        handle.join().expect("monitor thread");
    }
}
