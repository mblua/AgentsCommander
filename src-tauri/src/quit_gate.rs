//! Backend quit gate (#2296).

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tauri::Manager;

// ---------------------------------------------------------------------------
// #2296 — backend quit gate.
//
// Closing the main window must not kill the app while another window still
// holds unsaved work. Every window that owns such work registers as a *gate*;
// a quit round asks each one and exits only when all of them consent.
//
// The whole state machine is synchronous and lives behind one `std::sync::Mutex`
// that is never held across an await. Every mutating entry point returns the
// events it wants emitted (`QuitGateEffects`) instead of emitting under the
// lock, and `quit_gate_run` awaits outside it.
// ---------------------------------------------------------------------------

/// Unpaused budget a non-force round may spend waiting for its gates.
///
/// Strictly beyond main's 10-second wall-clock Force offer, so the user is
/// always offered Force before the backend gives up on its own.
pub const QUIT_GATE_TIMEOUT: Duration = Duration::from_secs(30);

/// The only window allowed to start or force a quit.
pub const QUIT_GATE_MAIN_LABEL: &str = "main";

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum QuitOutcomeKind {
    Exiting,
    Aborted,
    InFlight,
    Stale,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum QuitAbortReason {
    Refused,
    Timeout,
    Unregistered,
    Destroyed,
    Cancelled,
}

/// Typed return of `quit_application` and payload of `app_quit_outcome`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuitOutcome {
    pub outcome: QuitOutcomeKind,
    pub epoch: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<QuitAbortReason>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refusing_labels: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unanswered_labels: Option<Vec<String>>,
}

impl QuitOutcome {
    fn bare(outcome: QuitOutcomeKind, epoch: u64) -> Self {
        Self {
            outcome,
            epoch,
            reason: None,
            refusing_labels: None,
            unanswered_labels: None,
        }
    }

    pub fn exiting(epoch: u64) -> Self {
        Self::bare(QuitOutcomeKind::Exiting, epoch)
    }

    pub fn in_flight(epoch: u64) -> Self {
        Self::bare(QuitOutcomeKind::InFlight, epoch)
    }

    pub fn stale(epoch: u64) -> Self {
        Self::bare(QuitOutcomeKind::Stale, epoch)
    }

    pub(crate) fn aborted(
        epoch: u64,
        reason: QuitAbortReason,
        refusing: Vec<String>,
        unanswered: Vec<String>,
    ) -> Self {
        Self {
            outcome: QuitOutcomeKind::Aborted,
            epoch,
            reason: Some(reason),
            refusing_labels: (!refusing.is_empty()).then_some(refusing),
            unanswered_labels: (!unanswered.is_empty()).then_some(unanswered),
        }
    }
}

/// Typed return of `quit_gate_register`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "status")]
pub enum QuitGateRegistration {
    Registered,
    InFlight { epoch: u64 },
}

/// Events a state transition asks the caller to emit, outside the lock.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct QuitGateEffects {
    /// Terminal abort outcome for `main` (`app_quit_outcome`).
    pub outcome_to_main: Option<QuitOutcome>,
    /// `app_quit_cancelled` targets: every gate enrolled in the cancelled epoch.
    pub cancelled_gates: Vec<String>,
    /// Epoch carried by `cancelled_gates`.
    pub cancelled_epoch: u64,
}

impl QuitGateEffects {
    fn is_empty(&self) -> bool {
        self.outcome_to_main.is_none() && self.cancelled_gates.is_empty()
    }
}

/// Time seam. Ticks are a monotonic offset from gate creation, so the whole
/// deadline arithmetic is plain `Duration` math and a test clock can step it
/// exactly. Injected in tests.
pub trait QuitGateClock: Send + Sync + 'static {
    fn now(&self) -> Duration;
    fn sleep_until(&self, at: Duration) -> std::pin::Pin<Box<dyn Future<Output = ()> + Send>>;
}

/// Production clock: real monotonic time on the Tokio timer.
pub struct TokioQuitClock {
    origin: tokio::time::Instant,
}

impl Default for TokioQuitClock {
    fn default() -> Self {
        Self {
            origin: tokio::time::Instant::now(),
        }
    }
}

impl QuitGateClock for TokioQuitClock {
    fn now(&self) -> Duration {
        self.origin.elapsed()
    }

    fn sleep_until(&self, at: Duration) -> std::pin::Pin<Box<dyn Future<Output = ()> + Send>> {
        Box::pin(tokio::time::sleep_until(self.origin + at))
    }
}

/// Side-effect seam: event emission and process exit. Injected in tests.
pub trait QuitGateHost: Send + Sync + 'static {
    fn emit_to(&self, label: &str, event: &str, payload: serde_json::Value) -> Result<(), String>;
    /// Destroys every detached terminal window. Returns the labels destroyed.
    /// Called immediately before `exit`, never on a non-Exiting outcome.
    fn close_detached_terminals(&self) -> Vec<String>;
    fn exit(&self, code: i32);
}

/// #2563 - closes detached terminal windows at the moment quit is decided,
/// before the blocking `RunEvent::Exit` work, then exits.
pub(crate) fn exit_closing_detached(host: &dyn QuitGateHost) {
    let closed = host.close_detached_terminals();
    if !closed.is_empty() {
        log::info!(
            "[shutdown] destroyed {} detached terminal window(s)",
            closed.len()
        );
    }
    host.exit(0);
}

/// Production host: targeted Tauri emits and `AppHandle::exit`.
pub struct TauriQuitHost {
    app: tauri::AppHandle,
}

impl TauriQuitHost {
    pub fn new(app: tauri::AppHandle) -> Self {
        Self { app }
    }
}

impl QuitGateHost for TauriQuitHost {
    fn emit_to(&self, label: &str, event: &str, payload: serde_json::Value) -> Result<(), String> {
        tauri::Emitter::emit_to(&self.app, label, event, payload).map_err(|e| e.to_string())
    }

    fn close_detached_terminals(&self) -> Vec<String> {
        // Never touches `was_detached`: restore reads the session flag, not
        // the live window set, so detached sessions restore detached.
        let mut closed = Vec::new();
        for (label, window) in self.app.webview_windows() {
            if !label.starts_with("terminal-") {
                continue;
            }
            if let Err(e) = window.destroy() {
                log::warn!("[shutdown] detached window destroy failed: {label}: {e}");
            }
            closed.push(label);
        }
        closed
    }

    fn exit(&self, code: i32) {
        // SINGLE production exit site for the quit gate. Everything else routes
        // its Exiting decision through `quit_gate_run`, which calls this once
        // per terminal Exiting (see `try_claim_exit`).
        self.app.exit(code);
    }
}

/// One live quit round.
pub(crate) struct QuitRound {
    epoch: u64,
    /// Every label enrolled by the atomic snapshot, answered or not.
    snapshot: std::collections::BTreeSet<String>,
    /// Gates that have not answered yet. Holding the sender *is* the pending
    /// record; dropping it closes the waiter's receiver.
    pending: HashMap<String, tokio::sync::oneshot::Sender<bool>>,
    refusing: std::collections::BTreeSet<String>,
    busy: HashSet<String>,
    /// Budget left when paused; authoritative across pause/resume.
    remaining: Duration,
    /// `Some` while the clock runs, `None` while a busy gate pauses it.
    /// Expressed in clock ticks (see `QuitGateClock`).
    deadline: Option<Duration>,
}

#[derive(Default)]
pub(crate) struct QuitGateInner {
    next_epoch: u64,
    registered: std::collections::BTreeSet<String>,
    round: Option<QuitRound>,
    /// Terminal result of the most recent round. Written before any cleanup so
    /// a waiter woken by receiver closure reads a decision, never infers one.
    terminal: Option<(u64, QuitOutcome)>,
    /// Guards the one-exit rule for the current terminal `Exiting`.
    exit_claimed: bool,
}

impl QuitGateInner {
    fn alloc_epoch(&mut self) -> u64 {
        self.next_epoch += 1;
        self.next_epoch
    }

    fn live_round(&mut self, epoch: u64) -> Option<&mut QuitRound> {
        match self.round.as_mut() {
            Some(round) if round.epoch == epoch => Some(round),
            _ => None,
        }
    }

    fn terminal_for(&self, epoch: u64) -> Option<QuitOutcome> {
        match &self.terminal {
            Some((e, outcome)) if *e == epoch => Some(outcome.clone()),
            _ => None,
        }
    }

    /// Writes `outcome` as this epoch's terminal result and tears the round
    /// down. Never replaces an existing terminal for the same epoch.
    fn finish(&mut self, epoch: u64, outcome: QuitOutcome) -> Option<QuitOutcome> {
        if self.terminal_for(epoch).is_some() {
            return None;
        }
        self.live_round(epoch)?;
        // Terminal result first, cleanup second — the ordering the waiter relies on.
        self.terminal = Some((epoch, outcome.clone()));
        self.exit_claimed = false;
        self.round = None;
        Some(outcome)
    }

    /// Recomputes the deadline from `remaining` when no busy gate holds it.
    fn resume_if_idle(round: &mut QuitRound, now: Duration) {
        if round.busy.is_empty() && round.deadline.is_none() {
            round.deadline = Some(now + round.remaining);
        }
    }

    fn pause(round: &mut QuitRound, now: Duration) {
        if let Some(deadline) = round.deadline.take() {
            round.remaining = deadline.saturating_sub(now);
        }
    }
}

/// Managed quit-gate state.
pub struct QuitGate {
    inner: Mutex<QuitGateInner>,
    /// Woken on every state change so the waiter re-reads deadline and terminal.
    notify: tokio::sync::Notify,
    clock: Arc<dyn QuitGateClock>,
}

impl Default for QuitGate {
    fn default() -> Self {
        Self::new()
    }
}

impl QuitGate {
    pub fn new() -> Self {
        Self::with_clock(Arc::new(TokioQuitClock::default()))
    }

    pub fn with_clock(clock: Arc<dyn QuitGateClock>) -> Self {
        Self {
            inner: Mutex::new(QuitGateInner::default()),
            notify: tokio::sync::Notify::new(),
            clock,
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, QuitGateInner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Builds the abort effects for a terminal outcome of `round`.
    fn abort_effects(round: &QuitRound, outcome: QuitOutcome) -> QuitGateEffects {
        QuitGateEffects {
            cancelled_gates: round.snapshot.iter().cloned().collect(),
            cancelled_epoch: round.epoch,
            outcome_to_main: Some(outcome),
        }
    }

    /// Aborts the live round `epoch`. No-op for a stale or already terminal one.
    fn abort(&self, epoch: u64, reason: QuitAbortReason) -> QuitGateEffects {
        let mut inner = self.lock();
        let (refusing, mut unanswered) = {
            let Some(round) = inner.live_round(epoch) else {
                return QuitGateEffects::default();
            };
            let refusing: Vec<String> = round.refusing.iter().cloned().collect();
            let unanswered: Vec<String> = match reason {
                QuitAbortReason::Timeout => round.pending.keys().cloned().collect(),
                _ => Vec::new(),
            };
            (refusing, unanswered)
        };
        unanswered.sort();
        let outcome = QuitOutcome::aborted(epoch, reason, refusing, unanswered);
        let effects = {
            let round = inner
                .live_round(epoch)
                .expect("round checked live above under the same lock");
            Self::abort_effects(round, outcome.clone())
        };
        match inner.finish(epoch, outcome) {
            Some(_) => {
                drop(inner);
                self.notify.notify_waiters();
                effects
            }
            None => QuitGateEffects::default(),
        }
    }

    /// Writes terminal `Exiting` once the live round has no unanswered gate.
    fn finalize_consent_locked(inner: &mut QuitGateInner, epoch: u64) -> bool {
        let settled = match inner.live_round(epoch) {
            Some(round) => round.pending.is_empty() && round.refusing.is_empty(),
            None => false,
        };
        if !settled {
            return false;
        }
        inner.finish(epoch, QuitOutcome::exiting(epoch)).is_some()
    }

    fn finalize_consent(&self, epoch: u64) {
        let mut inner = self.lock();
        if Self::finalize_consent_locked(&mut inner, epoch) {
            drop(inner);
            self.notify.notify_waiters();
        }
    }

    /// True exactly once per terminal `Exiting`, for the single exit site.
    fn try_claim_exit(&self, epoch: u64) -> bool {
        let mut inner = self.lock();
        let is_exiting = matches!(
            inner.terminal_for(epoch),
            Some(QuitOutcome {
                outcome: QuitOutcomeKind::Exiting,
                ..
            })
        );
        if is_exiting && !inner.exit_claimed {
            inner.exit_claimed = true;
            true
        } else {
            false
        }
    }

    // --- command-facing state transitions ------------------------------------

    /// Enrols `label`. Rejected during an active round: the snapshot is atomic,
    /// so a late registrant would otherwise be silently ungated.
    pub fn register(&self, label: &str) -> QuitGateRegistration {
        let mut inner = self.lock();
        if let Some(round) = inner.round.as_ref() {
            return QuitGateRegistration::InFlight { epoch: round.epoch };
        }
        inner.registered.insert(label.to_string());
        QuitGateRegistration::Registered
    }

    /// Drops `label`'s registration. Aborts the live round only if that gate
    /// had not already answered — same-epoch consent is final.
    pub fn unregister(&self, label: &str) -> QuitGateEffects {
        self.teardown_gate(label, QuitAbortReason::Unregistered)
    }

    /// Window destroyed: same as unregister, with reason `destroyed`.
    /// Main's destruction is not an abort — main is never a gate.
    pub fn window_gone(&self, label: &str) -> QuitGateEffects {
        self.teardown_gate(label, QuitAbortReason::Destroyed)
    }

    /// Drops `label`'s registration and, if it was still unanswered, aborts the
    /// live round under ONE lock acquisition.
    ///
    /// The pending sender is removed into a local and dropped only after the
    /// terminal result has been written, so no waiter can ever observe a closed
    /// receiver before the decision that explains it.
    fn teardown_gate(&self, label: &str, reason: QuitAbortReason) -> QuitGateEffects {
        let mut inner = self.lock();
        inner.registered.remove(label);

        let now = self.clock.now();
        let (epoch, pending_sender) = match inner.round.as_mut() {
            Some(round) => {
                // Only an UNANSWERED gate aborts the round; same-epoch consent
                // is final and cannot be revoked by teardown.
                let sender = round.pending.remove(label);
                round.busy.remove(label);
                QuitGateInner::resume_if_idle(round, now);
                match sender {
                    Some(sender) => (Some(round.epoch), Some(sender)),
                    None => (None, None),
                }
            }
            None => (None, None),
        };
        let Some(epoch) = epoch else {
            return QuitGateEffects::default();
        };

        let refusing: Vec<String> = inner
            .live_round(epoch)
            .map(|round| round.refusing.iter().cloned().collect())
            .unwrap_or_default();
        let outcome = QuitOutcome::aborted(epoch, reason, refusing, vec![label.to_string()]);
        let effects = {
            let round = inner
                .live_round(epoch)
                .expect("round checked live above under the same lock");
            Self::abort_effects(round, outcome.clone())
        };
        let finished = inner.finish(epoch, outcome).is_some();
        // Terminal written; only now may the sender close.
        drop(pending_sender);
        drop(inner);
        if finished {
            self.notify.notify_waiters();
            effects
        } else {
            QuitGateEffects::default()
        }
    }

    /// Gate answer. Stale epoch, duplicate answer and unknown label do nothing.
    pub fn resolve(&self, label: &str, epoch: u64, consent: bool) -> QuitGateEffects {
        let mut inner = self.lock();
        {
            let Some(round) = inner.live_round(epoch) else {
                return QuitGateEffects::default();
            };
            let Some(sender) = round.pending.remove(label) else {
                return QuitGateEffects::default();
            };
            round.busy.remove(label);
            let now = self.clock.now();
            QuitGateInner::resume_if_idle(round, now);
            // The receiver wakes the waiter; the decision itself always comes
            // from the terminal slot, never from this value or from closure.
            let _ = sender.send(consent);
            if !consent {
                round.refusing.insert(label.to_string());
            }
        }
        if !consent {
            // A refusal is terminal immediately, even while a peer is busy.
            let refusing: Vec<String> = inner
                .live_round(epoch)
                .map(|round| round.refusing.iter().cloned().collect())
                .unwrap_or_default();
            let outcome =
                QuitOutcome::aborted(epoch, QuitAbortReason::Refused, refusing, Vec::new());
            let effects = {
                let round = inner
                    .live_round(epoch)
                    .expect("round checked live above under the same lock");
                Self::abort_effects(round, outcome.clone())
            };
            return match inner.finish(epoch, outcome) {
                Some(_) => {
                    drop(inner);
                    self.notify.notify_waiters();
                    effects
                }
                None => QuitGateEffects::default(),
            };
        }
        QuitGate::finalize_consent_locked(&mut inner, epoch);
        drop(inner);
        self.notify.notify_waiters();
        QuitGateEffects::default()
    }

    /// Busy progress pauses the timeout and preserves the remaining budget.
    /// Only the current epoch's busy set can move the clock.
    pub fn progress(&self, label: &str, epoch: u64, busy: bool) {
        let mut inner = self.lock();
        let Some(round) = inner.live_round(epoch) else {
            return;
        };
        if !round.pending.contains_key(label) {
            return;
        }
        let now = self.clock.now();
        if busy {
            round.busy.insert(label.to_string());
            QuitGateInner::pause(round, now);
        } else {
            round.busy.remove(label);
            QuitGateInner::resume_if_idle(round, now);
        }
        drop(inner);
        self.notify.notify_waiters();
    }

    /// Expires the round if its deadline passed and no gate is busy.
    fn on_timeout(&self, epoch: u64) -> QuitGateEffects {
        {
            let mut inner = self.lock();
            let now = self.clock.now();
            let Some(round) = inner.live_round(epoch) else {
                return QuitGateEffects::default();
            };
            // Recheck busy under the lock: a pause may have landed between the
            // sleep firing and this acquisition.
            if !round.busy.is_empty() {
                return QuitGateEffects::default();
            }
            match round.deadline {
                Some(deadline) if now >= deadline => {}
                _ => return QuitGateEffects::default(),
            }
        }
        self.abort(epoch, QuitAbortReason::Timeout)
    }

    #[cfg(test)]
    pub(crate) fn terminal_snapshot_for_test(&self, epoch: u64) -> Option<QuitOutcome> {
        self.lock().terminal_for(epoch)
    }

    pub(crate) fn deadline_for(&self, epoch: u64) -> Option<Duration> {
        let mut inner = self.lock();
        inner.live_round(epoch).and_then(|round| round.deadline)
    }
}

/// Emits the events a transition asked for. Never called with the lock held.
pub fn apply_quit_gate_effects(host: &dyn QuitGateHost, effects: &QuitGateEffects) {
    if effects.is_empty() {
        return;
    }
    for label in &effects.cancelled_gates {
        if let Err(e) = host.emit_to(
            label,
            "app_quit_cancelled",
            serde_json::json!({ "epoch": effects.cancelled_epoch, "label": label }),
        ) {
            log::warn!("[quit-gate] app_quit_cancelled emit to {label} failed: {e}");
        }
    }
    if let Some(outcome) = &effects.outcome_to_main {
        if let Err(e) = host.emit_to(
            QUIT_GATE_MAIN_LABEL,
            "app_quit_outcome",
            serde_json::to_value(outcome).unwrap_or_else(|_| serde_json::json!({})),
        ) {
            log::warn!("[quit-gate] app_quit_outcome emit failed: {e}");
        }
    }
}

/// Crate-root hook for the builder's `Destroyed` arm.
pub fn quit_gate_window_gone(gate: &QuitGate, host: &dyn QuitGateHost, label: &str) {
    let effects = gate.window_gone(label);
    apply_quit_gate_effects(host, &effects);
}

/// Releases the round if the quit task is cancelled or panics mid-await.
/// Only ever touches its own epoch.
pub(crate) struct QuitRoundGuard {
    gate: Arc<QuitGate>,
    host: Arc<dyn QuitGateHost>,
    epoch: u64,
    pub(crate) armed: bool,
}

impl Drop for QuitRoundGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let effects = self.gate.abort(self.epoch, QuitAbortReason::Cancelled);
        apply_quit_gate_effects(self.host.as_ref(), &effects);
    }
}

/// Runs a quit round to its terminal outcome. `force` takes the epoch to
/// supersede; a non-force call allocates a fresh one.
pub async fn quit_gate_run(
    gate: Arc<QuitGate>,
    host: Arc<dyn QuitGateHost>,
    caller_label: &str,
    force: bool,
    epoch: Option<u64>,
    attempt_id: Option<String>,
) -> Result<QuitOutcome, String> {
    if caller_label != QUIT_GATE_MAIN_LABEL {
        return Err(format!(
            "quit_application is restricted to the '{QUIT_GATE_MAIN_LABEL}' window (caller: \
                {caller_label})"
        ));
    }

    if force {
        // Force must prove it targets the live, nonterminal round before any
        // exit effect; an omitted or stale epoch cannot authorize exit.
        let supplied = epoch.unwrap_or(0);
        let claimed = {
            let mut inner = gate.lock();
            if inner.live_round(supplied).is_some() {
                // Invalidate any stale work still referencing this epoch.
                inner.next_epoch += 1;
                // Terminal `Exiting` for the ORIGINAL epoch, written before the
                // pending senders are dropped, so the superseded waiter returns
                // `Exiting` with its own epoch and does not exit a second time.
                inner
                    .finish(supplied, QuitOutcome::exiting(supplied))
                    .is_some()
            } else {
                false
            }
        };
        if !claimed {
            return Ok(QuitOutcome::stale(supplied));
        }
        gate.notify.notify_waiters();
        if gate.try_claim_exit(supplied) {
            exit_closing_detached(host.as_ref());
        }
        return Ok(QuitOutcome::exiting(supplied));
    }

    let attempt_id = attempt_id.unwrap_or_default();
    if attempt_id.is_empty() {
        return Err("quit_application requires a nonempty attemptId".to_string());
    }

    // Atomic install: allocate the epoch, snapshot the registered gates and
    // install every pending sender under one lock.
    let (new_epoch, receivers) = {
        let mut inner = gate.lock();
        if let Some(round) = inner.round.as_ref() {
            return Ok(QuitOutcome::in_flight(round.epoch));
        }
        let new_epoch = inner.alloc_epoch();
        let snapshot = inner.registered.clone();
        let mut pending = HashMap::new();
        let mut receivers = Vec::new();
        for label in &snapshot {
            let (tx, rx) = tokio::sync::oneshot::channel::<bool>();
            pending.insert(label.clone(), tx);
            receivers.push(rx);
        }
        inner.round = Some(QuitRound {
            epoch: new_epoch,
            snapshot,
            pending,
            refusing: std::collections::BTreeSet::new(),
            busy: HashSet::new(),
            remaining: QUIT_GATE_TIMEOUT,
            deadline: Some(gate.clock.now() + QUIT_GATE_TIMEOUT),
        });
        (new_epoch, receivers)
    };

    let mut guard = QuitRoundGuard {
        gate: Arc::clone(&gate),
        host: Arc::clone(&host),
        epoch: new_epoch,
        armed: true,
    };

    // Start event before awaiting any gate, so main can show its progress UI
    // and its 10-second Force offer for a round that is already installed.
    if let Err(e) = host.emit_to(
        QUIT_GATE_MAIN_LABEL,
        "app_quit_started",
        serde_json::json!({ "epoch": new_epoch, "attemptId": attempt_id }),
    ) {
        log::warn!("[quit-gate] app_quit_started emit failed: {e}");
        // An unobservable live round is worse than no round: tear it down and
        // return its own terminal result.
        guard.armed = false;
        let effects = gate.abort(new_epoch, QuitAbortReason::Cancelled);
        apply_quit_gate_effects(host.as_ref(), &effects);
        let outcome = gate.lock().terminal_for(new_epoch).unwrap_or_else(|| {
            QuitOutcome::aborted(
                new_epoch,
                QuitAbortReason::Cancelled,
                Vec::new(),
                Vec::new(),
            )
        });
        return Ok(outcome);
    }

    let targets: Vec<String> = {
        let mut inner = gate.lock();
        inner
            .live_round(new_epoch)
            .map(|round| round.snapshot.iter().cloned().collect())
            .unwrap_or_default()
    };
    for label in &targets {
        if let Err(e) = host.emit_to(
            label,
            "app_quit_requested",
            serde_json::json!({ "epoch": new_epoch, "label": label }),
        ) {
            log::warn!("[quit-gate] app_quit_requested emit to {label} failed: {e}");
        }
    }

    // A round with no gates consents immediately.
    gate.finalize_consent(new_epoch);

    let mut receivers: futures_util::stream::FuturesUnordered<_> = receivers.into_iter().collect();

    let outcome = loop {
        // ENROL FIRST. `Notified` snapshots the notify-waiters counter when it
        // is CREATED and registers on first poll, so reading the round state
        // before taking that snapshot opens a window in which a
        // `notify_waiters()` is dropped on the floor. A lost resume wakeup
        // would leave this task parked on `pending()` with a stale
        // `deadline: None` and defeat QUIT_GATE_TIMEOUT entirely.
        // Pinned by `waiter_enrols_before_reading_round_state`.
        let notified = gate.notify.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();

        if let Some(outcome) = gate.lock().terminal_for(new_epoch) {
            break outcome;
        }
        let deadline = gate.deadline_for(new_epoch);
        let has_receivers = !receivers.is_empty();
        tokio::select! {
            _ = &mut notified => {}
            Some(_) = futures_util::StreamExt::next(&mut receivers), if has_receivers => {}
            _ = async {
                match deadline {
                    Some(deadline) => gate.clock.sleep_until(deadline).await,
                    None => std::future::pending::<()>().await,
                }
            } => {
                let effects = gate.on_timeout(new_epoch);
                apply_quit_gate_effects(host.as_ref(), &effects);
            }
        }
    };

    guard.armed = false;
    if outcome.outcome == QuitOutcomeKind::Exiting && gate.try_claim_exit(new_epoch) {
        exit_closing_detached(host.as_ref());
    }
    Ok(outcome)
}

#[tauri::command]
pub(crate) fn quit_gate_register(
    window: tauri::WebviewWindow,
    gate: tauri::State<'_, Arc<QuitGate>>,
) -> QuitGateRegistration {
    gate.register(window.label())
}

#[tauri::command]
pub(crate) fn quit_gate_unregister(
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
    gate: tauri::State<'_, Arc<QuitGate>>,
) {
    let effects = gate.unregister(window.label());
    apply_quit_gate_effects(&TauriQuitHost::new(app), &effects);
}

#[tauri::command]
pub(crate) fn quit_gate_resolve(
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
    gate: tauri::State<'_, Arc<QuitGate>>,
    epoch: u64,
    consent: bool,
) {
    let effects = gate.resolve(window.label(), epoch, consent);
    apply_quit_gate_effects(&TauriQuitHost::new(app), &effects);
}

#[tauri::command]
pub(crate) fn quit_gate_progress(
    window: tauri::WebviewWindow,
    gate: tauri::State<'_, Arc<QuitGate>>,
    epoch: u64,
    busy: bool,
) {
    gate.progress(window.label(), epoch, busy);
}

#[tauri::command]
pub(crate) async fn quit_application(
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
    gate: tauri::State<'_, Arc<QuitGate>>,
    force: bool,
    epoch: Option<u64>,
    attempt_id: Option<String>,
) -> Result<QuitOutcome, String> {
    let gate = Arc::clone(gate.inner());
    let host: Arc<dyn QuitGateHost> = Arc::new(TauriQuitHost::new(app));
    let label = window.label().to_string();
    quit_gate_run(gate, host, &label, force, epoch, attempt_id).await
}
