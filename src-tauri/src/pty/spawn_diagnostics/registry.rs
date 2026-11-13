use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use portable_pty::ExitStatus;
use uuid::Uuid;

use crate::session::profile::CodingAgentKind;

use super::thresholds::Thresholds;

/// Safety net on the recent-spawn ring; only the time window is load-bearing.
const RECENT_SPAWNS_CAP: usize = 128;
/// Env keys whose VALUE is a secret and must never reach the log. Keyed on the name, not
/// swept over every value: a coding-agent profile legitimately carries rows like
/// `MODEL=gpt-5.6-sol`, and blanking those would shred the argv this module exists to
/// record, and the Codex error text inside `head=`.
const SECRET_KEY_MARKERS: &[&str] = &[
    "token",
    "key",
    "secret",
    "password",
    "passwd",
    "credential",
    // `AUTHORIZATION=Bearer sk-...` is a secret and neither the other markers nor the
    // URL-shape redactor catches it.
    "auth",
];

/// True when an env row's value must be redacted out of anything we echo into the log.
/// AC's own credential env is covered by this: the only secret it carries is
/// `AGENTSCOMMANDER_TOKEN` (the other rows are paths and a binary name, which are not
/// secrets and which we deliberately print in `cwd=` / `argv=`).
pub fn is_secret_env_key(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    if SECRET_KEY_MARKERS.iter().any(|marker| key.contains(marker)) {
        return true;
    }
    // A personal access token (`GH_PAT`). Matched as a whole segment, never as a
    // substring: `PATH`, `PATHEXT` and `COMPATIBILITY` all contain "pat", and blanking a
    // PATH across the log would shred every path in the evidence.
    key.split(|c: char| !c.is_ascii_alphanumeric())
        .any(|segment| segment == "pat")
}

/// A value from a secret-named row is redacted whatever its length: an explicitly named
/// `*_TOKEN` row is a secret even when it is short. The one exclusion is a purely numeric
/// value, because `MAX_TOKENS=8192` and `TOKEN_LIMIT=4096` are real config rows, and
/// blanking `8192` everywhere it appears would corrupt the evidence while protecting
/// nothing.
fn is_redactable_value(value: &str) -> bool {
    !value.trim().is_empty() && !value.chars().all(|c| c.is_ascii_digit())
}

/// What AC knows about a child process at the instant we ask.
#[derive(Debug, Clone)]
pub enum ChildLiveness {
    Alive,
    Exited {
        code: u32,
        success: bool,
    },
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
    /// #1271 - last liveness observed by the child monitor before it stopped.
    final_liveness: Mutex<Option<ChildLiveness>>,
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
                .filter(|value| is_redactable_value(value))
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
            final_liveness: Mutex::new(None),
        }
    }

    pub fn session_id(&self) -> Uuid {
        self.session_id
    }

    /// #1271 - the tracked child's OS pid, when the spawn captured one.
    pub fn pid(&self) -> Option<u32> {
        self.pid
    }

    /// #1271 - the exact argv the child was launched with (the adapter result),
    /// unredacted. Integration regressions assert the configured host shell was
    /// selected here.
    pub fn argv(&self) -> &[String] {
        &self.argv
    }

    /// #1271 - the last liveness the child monitor observed before it stopped
    /// polling. `Some(Exited { code, .. })` is the awaited child's exit code;
    /// `Some(Gone)` means the PTY instance was removed first (AC teardown).
    /// #1873 test-only: whether `log_child_exit` has already reported this child's
    /// exit (the duplicate-suppression latch). Production reads nothing here.
    #[cfg(test)]
    pub fn exit_reported(&self) -> bool {
        self.exit_reported.load(Ordering::SeqCst)
    }

    /// #1873 test-only: the cause stamped by the single accepted `log_child_exit`.
    #[cfg(test)]
    pub fn exit_cause(&self) -> Option<ExitCause> {
        *self
            .exit_cause
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn final_liveness(&self) -> Option<ChildLiveness> {
        self.final_liveness
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
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

    /// Mask secrets out of anything we echo into the log. `app.log` is what users paste
    /// into issues.
    ///
    /// ALWAYS runs on the RAW text, never on escaped text. Escaping first and matching
    /// second is a silent no-op for any secret containing a backslash, a quote, a control
    /// character or non-ASCII (the haystack has been rewritten, the needle has not), and
    /// a redaction that silently fails is worse than none, because it invites the log to
    /// be pasted in public.
    fn scrub(&self, text: &str) -> String {
        // One pass over the ORIGINAL text: collect the spans the URL/credential
        // redactor masks plus every occurrence of the exact secret-keyed env values
        // this child was given, merge spans that overlap or touch, then render once.
        // Two sequential passes let one pass break the other's match and leave a
        // secret tail in clear (#4319). A merged span that holds a needle prints
        // `<redacted>`; one made only of redactor spans prints `***`.
        let mut spans: Vec<(usize, usize, bool)> = crate::telegram::redact::secret_spans(text)
            .into_iter()
            .map(|(start, end)| (start, end, false))
            .collect();
        for secret in self.redact.iter().filter(|s| !s.is_empty()) {
            spans.extend(
                text.match_indices(secret.as_str())
                    .map(|(at, s)| (at, at + s.len(), true)),
            );
        }
        if spans.is_empty() {
            return text.to_string();
        }
        spans.sort_unstable();
        let mut out = String::with_capacity(text.len());
        let mut last = 0;
        let mut iter = spans.into_iter().peekable();
        while let Some((start, mut end, mut needle)) = iter.next() {
            while let Some(&(next_start, next_end, next_needle)) = iter.peek() {
                if next_start > end {
                    break;
                }
                end = end.max(next_end);
                needle |= next_needle;
                iter.next();
            }
            out.push_str(&text[last..start]);
            out.push_str(if needle { "<redacted>" } else { "***" });
            last = end;
        }
        out.push_str(&text[last..]);
        out
    }

    fn argv_log(&self) -> String {
        // Scrub each raw argument, THEN let Debug do the escaping.
        let argv: Vec<String> = self.argv.iter().map(|arg| self.scrub(arg)).collect();
        format!("{argv:?}")
    }

    fn head_log(&self) -> String {
        let head = self
            .head
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if head.is_empty() {
            return "<empty>".to_string();
        }
        // Scrub the raw child output, THEN escape it for the log line.
        let scrubbed = self.scrub(&String::from_utf8_lossy(&head));
        format!("\"{}\"", scrubbed.escape_debug())
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
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
            .unwrap_or_else(|| "none".to_string())
    }

    fn pre_stop_liveness(&self) -> Option<ChildLiveness> {
        self.pre_stop_liveness
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
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

    /// A stop AC asked for that still owns the child: the flag is set and it is inside
    /// the attribution window. A stop older than that evidently failed (a Quarantined kill
    /// leaves the agent running), so the next stop opens a NEW episode.
    fn stop_episode_open(&self) -> bool {
        self.stop_requested() && self.stop_cause() == ExitCause::AcRequested
    }

    /// Timestamp and source are LAST-wins: the attribution window must be measured from
    /// the stop that actually landed, not from a stale one that failed. The WITNESS is the
    /// opposite: it is the EARLIEST probe of the current stop episode, never the latest.
    ///
    /// Once AC has begun stopping a child, a probe is not a witness, it is a photograph of
    /// our own kill. On the destroy path the resource monitor `TerminateProcess`es the PTY
    /// child (its process tree includes the root) BEFORE `PtyBackend::kill` ever probes it,
    /// so a "latest wins" witness would see `Exited(1)`, conclude the child died on its
    /// own, and stamp a WARN with the full evidence dump on **every closed coding-agent
    /// tab**. That is requirement 3 inverted on the most common path in the app.
    ///
    /// So: a stop already in flight keeps the witness it published (or the absence of one:
    /// the watchdog publishes none, and a later probe of the corpse it made is no better).
    /// A new episode takes a fresh probe, because the child may have survived the last stop
    /// and its state now is genuinely unknown. And a witness that saw the child dead is
    /// sticky forever, because a dead child does not come back and no later `Gone` or
    /// `Unqueryable` from a racing second kill can improve on it.
    fn mark_ac_stop(&self, source: &str, pre_stop: Option<ChildLiveness>) {
        // Read the episode state BEFORE the timestamp below refreshes it.
        let episode_open = self.stop_episode_open();
        self.record_witness(pre_stop, episode_open);

        *self
            .ac_stop_source
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(source.to_string());
        let stamp = (self.started.elapsed().as_micros().min(u128::from(u64::MAX)) as u64).max(1);
        self.ac_stop_at_us.store(stamp, Ordering::SeqCst);
        // Published last: any thread that sees this flag also sees the source, the
        // timestamp and the pre-stop liveness written above.
        self.ac_stop.store(true, Ordering::SeqCst);
    }

    /// #1271 - the child monitor's last observation, published before the
    /// monitor stops polling so integration regressions can read the awaited
    /// child's exit code.
    fn record_final_liveness(&self, liveness: &ChildLiveness) {
        *self
            .final_liveness
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(liveness.clone());
    }

    fn record_witness(&self, pre_stop: Option<ChildLiveness>, episode_open: bool) {
        let Some(pre_stop) = pre_stop else {
            // A witness-less stop (the resource-monitor watchdog) never blanks a witness a
            // real probe already established.
            return;
        };
        let mut current = self
            .pre_stop_liveness
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match current.as_ref() {
            // Sticky: a dead child does not come back.
            Some(ChildLiveness::Exited { .. }) => {}
            // An open episode keeps what it has, including nothing at all.
            _ if episode_open => {}
            // A new episode: the child may have outlived the last stop, so ask again.
            _ => *current = Some(pre_stop),
        }
    }

    /// A stop AC asked for, recent enough that the teardown it triggers is still in
    /// flight. `ConPTY` repaints the master when the session is torn down (milliseconds
    /// after the kill), and that repaint is not the child painting. Deliberately a short
    /// window of its own, NOT the 60s attribution window: a stop that fails to kill the
    /// child must not blind the startup timings of a session that keeps running.
    fn stop_in_flight(&self, elapsed: Duration) -> bool {
        match self.ac_stop_at_us.load(Ordering::SeqCst) {
            0 => false,
            at => {
                elapsed.saturating_sub(Duration::from_micros(at))
                    <= self.thresholds.teardown_suppression
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
        let needs_paint = self.first_paint_us.load(Ordering::Relaxed) == 0
            && total >= self.thresholds.paint_floor;

        if needs_first || needs_paint {
            let elapsed = self.started.elapsed();
            // ConPTY repaints the master while a session is being torn down. That is
            // the teardown talking, not the child starting up.
            if !self.stop_in_flight(elapsed) {
                // 0 is the "nothing yet" sentinel for both stamps, so never store it.
                let stamp = (elapsed.as_micros().min(u128::from(u64::MAX)) as u64).max(1);

                // The first byte off the PTY. On Windows this is ConPTY greeting us,
                // not the child, which is why it is DEBUG and never proof of life.
                if needs_first
                    && self
                        .first_output_us
                        .compare_exchange(0, stamp, Ordering::SeqCst, Ordering::Relaxed)
                        .is_ok()
                {
                    log::debug!(
                        target: "agentscommander_lib::pty::spawn_diagnostics",
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
                            target: "agentscommander_lib::pty::spawn_diagnostics",
                            "[pty] first-paint session={} pid={} cli={} after_ms={} bytes={} \
                            late=true (startup stall recovered)",
                            self.session_id,
                            self.pid_log(),
                            self.cli_log(),
                            elapsed.as_millis(),
                            total
                        );
                    } else {
                        log::info!(
                            target: "agentscommander_lib::pty::spawn_diagnostics",
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
        let mut head = self
            .head
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
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

    /// The child never painted a screen: nothing at all, or nothing past the `ConPTY`
    /// handshake for a coding agent (which always paints a TUI immediately). A plain
    /// shell whose prompt is legitimately tiny is not stalled, it is just quiet.
    ///
    /// Keyed on the BYTES, not on the paint stamp: the stamp can be suppressed while a
    /// stop is in flight, and a suppressed stamp must never be able to fake a stall on a
    /// session that is painting normally.
    fn is_stalled(&self) -> bool {
        let bytes = self.bytes_read.load(Ordering::Relaxed);
        bytes == 0 || (self.cli.is_some() && bytes < self.thresholds.paint_floor)
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
            "[pty] {} session={} pid={} cli={} agent_profile={} backend=local child={} stalled={} \
            elapsed_ms={} threshold_ms={} bytes_read={} paint_floor={} first_output_ms={} \
            first_paint_ms={} argv={} cwd={} codex_home={} recent_spawns={} recent_same_cli={} \
            window_ms={} head={}",
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
            log::warn!(
                target: "agentscommander_lib::pty::spawn_diagnostics",
                "{line}");
        } else {
            log::info!(
                target: "agentscommander_lib::pty::spawn_diagnostics",
                "{line}");
        }
        true
    }

    /// The one exit event for this session. Returns false when another path already
    /// reported it (the caller may then leave a debug crumb instead).
    pub fn log_child_exit(&self, cause: ExitCause, liveness: &ChildLiveness, detail: &str) -> bool {
        if self.exit_reported.swap(true, Ordering::SeqCst) {
            return false;
        }
        *self
            .exit_cause
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(cause);

        let uptime = self.started.elapsed();
        let bytes = self.bytes_read.load(Ordering::Relaxed);
        // A child that ends itself without ever painting a screen, or with a failing
        // status, is the #942 smoking gun. A clean exit from a session that did come up
        // is just someone quitting.
        let never_came_up = self.is_stalled();
        let unexpected =
            cause == ExitCause::ChildInitiated && (never_came_up || !liveness.exited_ok());
        // The head bytes and the argv go out whenever the session never came up, whoever
        // ended it. The user who kills a blank terminal at t=3s (before the deadline
        // report can fire) is reporting THIS bug, and we must not answer with a one-line
        // shrug just because the kill was ours.
        let with_evidence = unexpected || never_came_up;

        if with_evidence {
            let line = format!(
                "[pty] child-exit session={} pid={} cli={} agent_profile={} cause={} detail={} \
                child={} came_up={} uptime_ms={} first_output_ms={} first_paint_ms={} \
                bytes_read={} stop_source={} stop_age_ms={} argv={} cwd={} codex_home={} \
                recent_spawns={} recent_same_cli={} window_ms={} head={}",
                self.session_id,
                self.pid_log(),
                self.cli_log(),
                self.profile_log(),
                cause.as_log(),
                detail,
                liveness.as_log(),
                !never_came_up,
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
            if unexpected {
                log::warn!(
                    target: "agentscommander_lib::pty::spawn_diagnostics",
                    "{line}");
            } else {
                log::info!(
                    target: "agentscommander_lib::pty::spawn_diagnostics",
                    "{line}");
            }
        } else {
            log::info!(
                target: "agentscommander_lib::pty::spawn_diagnostics",
                "[pty] child-exit session={} pid={} cli={} agent_profile={} cause={} detail={} \
                child={} uptime_ms={} first_output_ms={} first_paint_ms={} bytes_read={} \
                stop_source={} stop_age_ms={}",
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
            target: "agentscommander_lib::pty::spawn_diagnostics",
            "[pty] spawn-record session={} pid={} cli={} agent_profile={} backend=local argv={} \
            cwd={} codex_home={} env_configured={} env_removed={} recent_spawns={} \
            recent_same_cli={} window_ms={}",
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

pub struct SpawnRegistry {
    registry: OnceLock<Mutex<Registry>>,
}

#[allow(clippy::new_without_default)]
impl SpawnRegistry {
    pub fn new() -> Self {
        Self {
            registry: OnceLock::new(),
        }
    }
}

fn registry(owner: &SpawnRegistry) -> &Mutex<Registry> {
    owner
        .registry
        .get_or_init(|| Mutex::new(Registry::default()))
}

/// Count the spawns that happened in the window BEFORE this one, then record this
/// attempt. Call once per spawn, at the top of the spawn path.
pub fn note_spawn_attempt(
    cli: Option<CodingAgentKind>,
    thresholds: Thresholds,
    owner: &SpawnRegistry,
) -> SpawnWindow {
    let window = thresholds.concurrency_window;
    let now = Instant::now();
    let mut registry = registry(owner)
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);

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
        window_ms: window.as_millis().min(u128::from(u64::MAX)) as u64,
        total,
        same_cli,
    }
}

/// Register a spawned child and emit its spawn record.
pub fn register(init: SpawnRecordInit, owner: &SpawnRegistry) -> Arc<SpawnRecord> {
    let record = Arc::new(SpawnRecord::new(init));
    record.log_spawn();
    registry(owner)
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
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
    owner: &SpawnRegistry,
) -> Option<Arc<SpawnRecord>> {
    let record = registry(owner)
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .records
        .get(&session_id)
        .cloned()?;
    record.mark_ac_stop(source, pre_stop);
    Some(record)
}

pub fn forget(session_id: Uuid, owner: &SpawnRegistry) {
    registry(owner)
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .records
        .remove(&session_id);
}

/// #1271 - read the spawn record for one session, when one exists. Test
/// postcondition seam (Finding E): lets a test assert that a rejected spawn
/// never recorded launch provenance, and lets the Windows integration
/// regression read the exact adapted argv and awaited child exit.
pub fn record_for(session_id: Uuid, owner: &SpawnRegistry) -> Option<Arc<SpawnRecord>> {
    registry(owner)
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .records
        .get(&session_id)
        .cloned()
}

/// One monitor thread per local session. It owns two signals the PTY reader cannot
/// carry: the startup verdict at the deadline, and the exit of a child that dies
/// without AC asking (`ConPTY` keeps the pipe open past the death of the child, so reader
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
            ChildLiveness::Gone => {
                record.record_final_liveness(&liveness);
                return;
            }
            ChildLiveness::Exited { .. } => {
                record.record_final_liveness(&liveness);
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
mod tests;
