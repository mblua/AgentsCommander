//! #777 "Non-stop" watchdog: backend timing + actuation half of the hybrid design.
//!
//! The frontend owns the ONLY thing it can compute correctly, the visible
//! `working/total` counter parity, and pushes a full snapshot (one entry per
//! project with an ACTIVE Non-stop group) via the `non_stop_report` IPC command.
//! This module owns everything latency- and reliability-sensitive: the tolerance
//! timer and the two measures (a pulsed Win32 beep and a Telegram message). The
//! backend NEVER computes "working"; it only times and actuates on the reported
//! disparity.
//!
//! Failure-mode coverage (resolved in the plan's Step-7):
//! - Self-heal (D1 addendum): an episode whose frontend has gone silent past
//!   `REPORT_STALENESS_CEILING` (zombie webview / crash) is disarmed, so a dead
//!   frontend cannot keep firing. The ceiling sits ABOVE the ~60s worst-case
//!   throttled keepalive, so a legitimately-minimized episode is never
//!   false-disarmed.
//! - Single-fire (Maria, locked): the `fired` latch fires at most once per
//!   episode; it re-arms only when the disparity clears and reappears.
//! - Beep lifecycle (G3): one alarm at a time (`beep_serialize`), and recovery
//!   cancels an in-progress beep via a per-project `CancellationToken`.
//! - TOCTOU (G5): `fire` re-reads the episode under the lock and skips if the
//!   disparity cleared between the tick's fire-decision and actuation.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::Deserialize;
use tauri::{AppHandle, Manager};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use crate::shutdown::ShutdownSignal;
use crate::telegram::types::TelegramBotConfig;

/// Backend loop cadence. Firing latency stays within one tick of onset+tolerance.
const TICK_INTERVAL: Duration = Duration::from_secs(1);

/// Self-heal backstop for the "webview crashed, Rust process alive, no frontend"
/// case (D1 addendum). 180s > the ~60s throttled-keepalive worst case, so a
/// legitimately minimized (stably disparate) episode is never false-disarmed.
const REPORT_STALENESS_CEILING: Duration = Duration::from_secs(180);

/// Frontend -> backend watchdog signal. Mirrors `NonStopReport` in
/// `src/shared/types.ts` (camelCase over the IPC boundary).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NonStopReport {
    pub project_path: String,
    pub group_name: String,
    pub disparity: bool,
    pub working: u32,
    pub total: u32,
    pub not_working_workgroups: Vec<String>,
    pub tolerance_seconds: u32,
    pub telegram_enabled: bool,
    pub telegram_bot_id: Option<String>,
    pub sound_enabled: bool,
    pub sound_seconds: u32,
}

struct Episode {
    report: NonStopReport,
    disparity_since: Option<Instant>,
    fired: bool,
    /// Stamped on every ingest. The tick disarms an episode whose frontend has
    /// gone silent past `REPORT_STALENESS_CEILING`, so a crashed frontend cannot
    /// keep firing.
    last_seen: Instant,
}

#[derive(Clone, Default)]
pub struct NonStopWatchdogState {
    inner: Arc<Mutex<HashMap<String, Episode>>>,
    /// (G3) One in-flight alarm token per project; recovery cancels it so a long
    /// beep (up to 60s) is silenced the moment the disparity clears.
    alarms: Arc<Mutex<HashMap<String, CancellationToken>>>,
    /// (G3) Serializes beeps so two simultaneous project fires do not overlap
    /// into a garbled tone.
    #[cfg_attr(not(any(target_os = "windows", test)), allow(dead_code))]
    beep_serialize: Arc<Mutex<()>>,
}

impl NonStopWatchdogState {
    /// `Default` is derived (avoids `clippy::new_without_default`, which fails the
    /// CI gate); `new()` delegates to it.
    pub fn new() -> Self {
        Self::default()
    }

    /// Replace the episode map from the latest full snapshot. Carries
    /// `disparity_since` + `fired` for a continuing episode; stamps a new onset on
    /// false->true; clears on ->false; stamps `last_seen = now` on every entry.
    /// Any project that transitions to no-disparity (or drops out of the snapshot)
    /// has its in-flight alarm cancelled (G3), so recovery silences the beep.
    pub async fn ingest(&self, reports: Vec<NonStopReport>) {
        let now = Instant::now();
        let mut recovered: Vec<String> = Vec::new();
        {
            let mut map = self.inner.lock().await;
            let mut next: HashMap<String, Episode> = HashMap::new();
            for r in reports {
                let prev = map.remove(&r.project_path);
                let (since, fired) = if r.disparity {
                    match prev {
                        Some(p) if p.disparity_since.is_some() => (p.disparity_since, p.fired),
                        _ => (Some(now), false),
                    }
                } else {
                    recovered.push(r.project_path.clone());
                    (None, false)
                };
                next.insert(
                    r.project_path.clone(),
                    Episode {
                        report: r,
                        disparity_since: since,
                        fired,
                        last_seen: now,
                    },
                );
            }
            // Projects known before but absent from this snapshot are also
            // recovered (disarmed) for alarm-cancellation purposes.
            for gone in map.keys() {
                recovered.push(gone.clone());
            }
            *map = next;
        }
        if !recovered.is_empty() {
            let mut alarms = self.alarms.lock().await;
            for p in recovered {
                if let Some(tok) = alarms.remove(&p) {
                    tok.cancel();
                }
            }
        }
    }
}

/// Pure state-machine seam (testable without a Tauri app): disarm stale episodes,
/// then collect the ones whose tolerance has elapsed, marking each `fired` so the
/// single-fire latch holds. Mutates the map in place; returns the reports to act
/// on. Both passes run under the caller's lock.
fn collect_fireable(map: &mut HashMap<String, Episode>, now: Instant) -> Vec<NonStopReport> {
    // Self-heal: disarm episodes whose frontend has gone silent past the ceiling.
    // Never trips for a live minimized episode (keepalive <= ~60s << 180s).
    for ep in map.values_mut() {
        if ep.disparity_since.is_some()
            && now.duration_since(ep.last_seen) > REPORT_STALENESS_CEILING
        {
            log::warn!(
                "[non-stop] '{}' disarmed: no frontend report for >{}s (frontend gone)",
                ep.report.project_path,
                REPORT_STALENESS_CEILING.as_secs()
            );
            ep.disparity_since = None;
            ep.fired = false;
        }
    }

    let mut out = Vec::new();
    for ep in map.values_mut() {
        if ep.fired {
            continue;
        }
        let Some(since) = ep.disparity_since else {
            continue;
        };
        if now.duration_since(since) >= Duration::from_secs(ep.report.tolerance_seconds as u64) {
            ep.fired = true;
            out.push(ep.report.clone());
        }
    }
    out
}

/// (G5) TOCTOU re-read seam: the current report for `project_path` iff the
/// disparity still holds. Returns `None` if the episode recovered or dropped
/// between the tick's fire-decision and actuation.
fn fresh_if_still_disparate(
    map: &HashMap<String, Episode>,
    project_path: &str,
) -> Option<NonStopReport> {
    match map.get(project_path) {
        Some(ep) if ep.disparity_since.is_some() => Some(ep.report.clone()),
        _ => None,
    }
}

/// Bot resolution: requested id, else the first configured bot, else none.
fn resolve_bot<'a>(
    bots: &'a [TelegramBotConfig],
    requested: Option<&str>,
) -> Option<&'a TelegramBotConfig> {
    match requested {
        Some(id) => bots.iter().find(|b| b.id == id).or_else(|| bots.first()),
        None => bots.first(),
    }
}

pub fn start(
    app: AppHandle,
    state: NonStopWatchdogState,
    shutdown: ShutdownSignal,
) -> tauri::async_runtime::JoinHandle<()> {
    tauri::async_runtime::spawn(async move {
        if !shutdown.wait_for_startup_commit().await {
            return;
        }
        let mut interval = tokio::time::interval(TICK_INTERVAL);
        loop {
            tokio::select! {
                _ = shutdown.token().cancelled() => break,
                _ = interval.tick() => tick(&app, &state).await,
            }
        }
    })
}

async fn tick(app: &AppHandle, state: &NonStopWatchdogState) {
    let now = Instant::now();
    let to_fire: Vec<NonStopReport> = {
        let mut map = state.inner.lock().await;
        collect_fireable(&mut map, now)
    };
    for r in to_fire {
        fire(app, state, &r).await;
    }
}

async fn fire(app: &AppHandle, state: &NonStopWatchdogState, r: &NonStopReport) {
    // (G5, TOCTOU) Re-read under the lock: if the disparity cleared between the
    // tick's fire-decision and now, abort. No beep, no stale "not working"
    // Telegram message listing already-recovered workgroups.
    let fresh = {
        let map = state.inner.lock().await;
        match fresh_if_still_disparate(&map, &r.project_path) {
            Some(report) => report,
            None => return,
        }
    };
    log::warn!(
        "[non-stop] '{}' [{}] {}/{} working; not working: {:?}; persisted >{}s",
        fresh.group_name,
        fresh.project_path,
        fresh.working,
        fresh.total,
        fresh.not_working_workgroups,
        fresh.tolerance_seconds
    );
    if fresh.sound_enabled {
        let token = CancellationToken::new();
        state
            .alarms
            .lock()
            .await
            .insert(fresh.project_path.clone(), token.clone());
        play_alarm_beep(
            state.clone(),
            fresh.project_path.clone(),
            fresh.sound_seconds.clamp(1, 60),
            token,
        );
    }
    if fresh.telegram_enabled {
        send_telegram(app, &fresh).await;
    }
}

// Serialized, cancellable pulsed alarm (G3). Only one plays at a time; a recovery
// (ingest) cancels the token so an in-progress long beep stops within one pulse
// (<= ~550ms).
//
// (F1) This function does NOT touch the alarm map. `ingest`'s recovery/drop path
// owns removal + cancellation EXCLUSIVELY, and `fire`'s insert overwrites any
// stale (already-cancelled) entry cleanly. A trailing self-remove here would be
// wrong: under `beep_serialize` contention a queued-then-cancelled spawn can run
// LONG after a recovery + re-fire replaced its map entry with a newer token, so
// removing "its own" project key would actually delete the NEWER token, orphaning
// it (a later recovery would find nothing to cancel and the beep would play its
// full clamped duration unsilenceable). Leaving removal to `ingest` keeps the
// cancellation invariant intact; a fired-but-never-recovered project leaves at
// most one stale entry, bounded by project count and reclaimed on its next
// recovery/drop.
#[cfg(target_os = "windows")]
fn play_alarm_beep(
    state: NonStopWatchdogState,
    _project_path: String,
    seconds: u32,
    token: CancellationToken,
) {
    tauri::async_runtime::spawn(async move {
        // One alarm at a time. Held across the blocking beep so a second project
        // fire queues behind this one instead of overlapping.
        let _serialize = state.beep_serialize.lock().await;
        if !token.is_cancelled() {
            let beep_token = token.clone();
            let _ = tauri::async_runtime::spawn_blocking(move || {
                // `Beep` lives in Win32::System::Diagnostics::Debug (feature
                // `Win32_System_Diagnostics_Debug`, already enabled in Cargo.toml),
                // NOT in ::System::Console.
                use windows_sys::Win32::System::Diagnostics::Debug::Beep;
                let deadline =
                    std::time::Instant::now() + std::time::Duration::from_secs(seconds as u64);
                while std::time::Instant::now() < deadline && !beep_token.is_cancelled() {
                    // 400ms tone (blocking) + 150ms gap -> reads as a pulsed alarm.
                    let _ = unsafe { Beep(880, 400) };
                    std::thread::sleep(std::time::Duration::from_millis(150));
                }
            })
            .await;
        }
    });
}

#[cfg(not(target_os = "windows"))]
fn play_alarm_beep(
    _state: NonStopWatchdogState,
    _project_path: String,
    _seconds: u32,
    _token: CancellationToken,
) {
    // No backend audio off Windows; the modal disables the Sound measure there.
    // (F1) Does not touch the alarm map: `ingest` owns removal exclusively.
    log::info!("[non-stop] sound alert requested (no-op on non-Windows)");
}

async fn send_telegram(app: &AppHandle, r: &NonStopReport) {
    // Resolve bot from global settings: requested id, else first bot; skip if none.
    // Bind the State to a local before .read().await (E0716).
    let resolved = {
        let settings = app.state::<crate::config::settings::SettingsState>();
        let guard = settings.read().await;
        match resolve_bot(&guard.telegram_bots, r.telegram_bot_id.as_deref()) {
            Some(b) => (b.token.clone(), b.chat_id, b.label.clone()),
            None => {
                log::warn!("[non-stop] telegram enabled but no bots configured; skipping");
                return;
            }
        }
    };
    let (token, chat_id, label) = resolved;
    let network = app
        .state::<crate::network::OutboundNetwork>()
        .inner()
        .clone();
    let text = format!(
        "\u{26A0} Alert me! [{}]: {} {}/{} workgroups working. Not working: {}. Persisted >{}s.",
        project_folder_name(&r.project_path),
        r.group_name,
        r.working,
        r.total,
        if r.not_working_workgroups.is_empty() {
            "-".to_string()
        } else {
            r.not_working_workgroups.join(", ")
        },
        r.tolerance_seconds
    );
    tauri::async_runtime::spawn(async move {
        if let Err(e) = crate::telegram::api::send_message(&network, &token, chat_id, &text).await {
            log::warn!("[non-stop] telegram send via '{}' failed: {}", label, e);
        }
    });
}

fn project_folder_name(path: &str) -> String {
    std::path::Path::new(path)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(path)
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(project: &str, disparity: bool, tolerance_seconds: u32) -> NonStopReport {
        NonStopReport {
            project_path: project.to_string(),
            group_name: "Alert me!".to_string(),
            disparity,
            working: if disparity { 1 } else { 2 },
            total: 2,
            not_working_workgroups: if disparity {
                vec!["wg-2".to_string()]
            } else {
                vec![]
            },
            tolerance_seconds,
            telegram_enabled: false,
            telegram_bot_id: None,
            sound_enabled: false,
            sound_seconds: 3,
        }
    }

    fn bot(id: &str) -> TelegramBotConfig {
        TelegramBotConfig {
            id: id.to_string(),
            label: format!("label-{id}"),
            token: format!("token-{id}"),
            chat_id: 100,
            color: "#000".to_string(),
        }
    }

    async fn disparity_since(state: &NonStopWatchdogState, project: &str) -> Option<Instant> {
        state
            .inner
            .lock()
            .await
            .get(project)
            .unwrap()
            .disparity_since
    }

    async fn is_fired(state: &NonStopWatchdogState, project: &str) -> bool {
        state.inner.lock().await.get(project).unwrap().fired
    }

    /// Force an armed episode's `disparity_since` (and `last_seen`) into the past
    /// so tolerance/staleness logic can be exercised without sleeping.
    async fn backdate_disparity(state: &NonStopWatchdogState, project: &str, ago: Duration) {
        let mut map = state.inner.lock().await;
        let ep = map.get_mut(project).unwrap();
        ep.disparity_since = Some(Instant::now() - ago);
    }

    async fn backdate_last_seen(state: &NonStopWatchdogState, project: &str, ago: Duration) {
        let mut map = state.inner.lock().await;
        map.get_mut(project).unwrap().last_seen = Instant::now() - ago;
    }

    async fn fireable(state: &NonStopWatchdogState) -> Vec<NonStopReport> {
        let mut map = state.inner.lock().await;
        collect_fireable(&mut map, Instant::now())
    }

    #[tokio::test]
    async fn ingest_stamps_onset_once() {
        let state = NonStopWatchdogState::new();
        state.ingest(vec![report("p", true, 30)]).await;
        let first = disparity_since(&state, "p").await;
        assert!(first.is_some());

        state.ingest(vec![report("p", true, 30)]).await;
        let second = disparity_since(&state, "p").await;
        assert_eq!(first, second, "a continuing disparity keeps the same onset");
    }

    #[tokio::test]
    async fn ingest_clears_on_recovery() {
        let state = NonStopWatchdogState::new();
        state.ingest(vec![report("p", true, 30)]).await;
        state.ingest(vec![report("p", false, 30)]).await;
        assert_eq!(disparity_since(&state, "p").await, None);
        assert!(!is_fired(&state, "p").await);
    }

    #[tokio::test]
    async fn ingest_drops_absent_projects() {
        let state = NonStopWatchdogState::new();
        state.ingest(vec![report("p", true, 30)]).await;
        // A snapshot that omits "p" disarms it.
        state.ingest(vec![]).await;
        assert!(state.inner.lock().await.get("p").is_none());
    }

    #[tokio::test]
    async fn tick_fires_after_tolerance_and_not_before() {
        let state = NonStopWatchdogState::new();
        state.ingest(vec![report("p", true, 30)]).await;

        // Not yet past tolerance.
        assert!(fireable(&state).await.is_empty());

        backdate_disparity(&state, "p", Duration::from_secs(31)).await;
        let fired = fireable(&state).await;
        assert_eq!(fired.len(), 1);
        assert_eq!(fired[0].project_path, "p");

        // Single-fire latch: a second collection yields nothing.
        assert!(fireable(&state).await.is_empty());
    }

    #[tokio::test]
    async fn tick_refires_after_rearm() {
        let state = NonStopWatchdogState::new();
        state.ingest(vec![report("p", true, 30)]).await;
        backdate_disparity(&state, "p", Duration::from_secs(31)).await;
        assert_eq!(fireable(&state).await.len(), 1);

        // Recover, then a fresh disparity re-arms and fires again.
        state.ingest(vec![report("p", false, 30)]).await;
        state.ingest(vec![report("p", true, 30)]).await;
        backdate_disparity(&state, "p", Duration::from_secs(31)).await;
        assert_eq!(fireable(&state).await.len(), 1);
    }

    #[tokio::test]
    async fn tick_disarms_after_staleness_ceiling() {
        let state = NonStopWatchdogState::new();
        state.ingest(vec![report("p", true, 30)]).await;
        backdate_disparity(&state, "p", Duration::from_secs(31)).await;
        // Frontend went silent well past the ceiling.
        backdate_last_seen(
            &state,
            "p",
            REPORT_STALENESS_CEILING + Duration::from_secs(1),
        )
        .await;

        // One collection disarms instead of firing.
        assert!(fireable(&state).await.is_empty());
        assert_eq!(disparity_since(&state, "p").await, None);
        assert!(!is_fired(&state, "p").await);
    }

    #[tokio::test]
    async fn fresh_last_seen_does_not_disarm() {
        let state = NonStopWatchdogState::new();
        state.ingest(vec![report("p", true, 30)]).await;
        backdate_disparity(&state, "p", Duration::from_secs(31)).await;
        // last_seen is fresh (just ingested): a live minimized episode still fires.
        let fired = fireable(&state).await;
        assert_eq!(fired.len(), 1);
    }

    #[tokio::test]
    async fn ingest_cancels_alarm_on_recovery() {
        let state = NonStopWatchdogState::new();
        state.ingest(vec![report("p", true, 30)]).await;
        // Simulate an in-flight alarm.
        let token = CancellationToken::new();
        state
            .alarms
            .lock()
            .await
            .insert("p".to_string(), token.clone());

        state.ingest(vec![report("p", false, 30)]).await;

        assert!(token.is_cancelled(), "recovery cancels the in-flight beep");
        assert!(
            state.alarms.lock().await.get("p").is_none(),
            "the cancelled token is removed"
        );
    }

    #[tokio::test]
    async fn stale_queued_beep_completion_does_not_orphan_a_refired_token() {
        // F1 regression: under beep_serialize contention, a queued-then-cancelled
        // first spawn must NOT remove a newer token's alarm-map entry when it later
        // completes, or a subsequent recovery cannot cancel the second beep.
        let state = NonStopWatchdogState::new();

        // Fire #1 for "p" produced token_p1, which a recovery already cancelled.
        let token_p1 = CancellationToken::new();
        token_p1.cancel();

        // Re-arm + Fire #2 inserted a fresh token_p2 (as `fire` does on re-fire).
        let token_p2 = CancellationToken::new();
        state
            .alarms
            .lock()
            .await
            .insert("p".to_string(), token_p2.clone());

        // Simulate project A holding the serialize lock (a long beep in flight), so
        // the stale Spawn_p1 queues behind it instead of running immediately.
        let a_guard = state.beep_serialize.lock().await;
        play_alarm_beep(state.clone(), "p".to_string(), 3, token_p1);
        // Give the stale spawn time to reach its `beep_serialize.lock().await`.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert!(
            state.alarms.lock().await.contains_key("p"),
            "token_p2 present while the stale spawn is still queued"
        );

        // A finishes. tokio's Mutex is FIFO, so the stale spawn (queued before this
        // re-lock) runs to completion before the test re-acquires the lock.
        drop(a_guard);
        drop(state.beep_serialize.lock().await);

        // With the F1 fix, the stale spawn left the alarm map untouched.
        assert!(
            state.alarms.lock().await.contains_key("p"),
            "a stale, cancelled spawn must not orphan the re-fired token"
        );

        // The invariant grinch's trace breaks: a later recovery must cancel token_p2.
        state.ingest(vec![report("p", false, 30)]).await;
        assert!(
            token_p2.is_cancelled(),
            "recovery must find and cancel the re-fired beep token"
        );
        assert!(!state.alarms.lock().await.contains_key("p"));
    }

    #[tokio::test]
    async fn fire_skips_when_disparity_cleared() {
        let state = NonStopWatchdogState::new();
        state.ingest(vec![report("p", true, 30)]).await;
        backdate_disparity(&state, "p", Duration::from_secs(31)).await;
        // The tick decided to fire (episode marked fired)...
        assert_eq!(fireable(&state).await.len(), 1);
        // ...but a clear arrives before actuation.
        state.ingest(vec![report("p", false, 30)]).await;

        let map = state.inner.lock().await;
        assert!(
            fresh_if_still_disparate(&map, "p").is_none(),
            "TOCTOU guard: no send/beep once the disparity cleared"
        );
    }

    #[test]
    fn resolve_bot_prefers_requested_then_first_then_none() {
        let bots = vec![bot("a"), bot("b")];
        assert_eq!(resolve_bot(&bots, Some("b")).unwrap().id, "b");
        // Unknown requested id falls back to the first configured bot.
        assert_eq!(resolve_bot(&bots, Some("zzz")).unwrap().id, "a");
        // No request -> first bot.
        assert_eq!(resolve_bot(&bots, None).unwrap().id, "a");
        // No bots -> none.
        assert!(resolve_bot(&[], Some("a")).is_none());
        assert!(resolve_bot(&[], None).is_none());
    }

    #[test]
    fn project_folder_name_extracts_leaf() {
        assert_eq!(project_folder_name("C:/repos/my-project"), "my-project");
        assert_eq!(project_folder_name("plain"), "plain");
    }
}
