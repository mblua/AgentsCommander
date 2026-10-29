//! #777 "Non-stop" watchdog: backend timing + actuation half of the hybrid design.
//!
//! The frontend owns the ONLY thing it can compute correctly, the visible
//! `working/total` counter parity, and pushes a full snapshot (one entry per
//! project with an ACTIVE Non-stop group) via the `non_stop_report` IPC command.
//! This module owns everything latency- and reliability-sensitive: the tolerance
//! timer and the two measures (a frontend alarm event and a Telegram message). The
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
//! - Alarm lifecycle (G3): serialization, cancellation and audibility now live
//!   in `src/shared/sound.ts`; the backend only decides WHEN to alarm and emits
//!   an alarm event.
//! - TOCTOU (G5): `fire` re-reads the episode under the lock and skips if the
//!   disparity cleared between the tick's fire-decision and actuation.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use crate::config::settings::SettingsState;
use crate::network::OutboundNetwork;
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

/// Frontend alarm event. The backend decides WHEN to alarm; the frontend owns
/// serialization, cancellation and audibility (`src/shared/sound.ts`).
pub const NON_STOP_ALARM_EVENT: &str = "non_stop_alarm";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NonStopAlarmPayload {
    pub project_path: String,
    pub group_name: String,
    pub seconds: u32,
    pub action: String, // "start" | "stop"
}

pub trait NonStopAlarmEvents {
    fn alarm(&self, payload: NonStopAlarmPayload);
}

pub trait WatchdogHost: NonStopAlarmEvents + Clone + Send + Sync + 'static {
    fn settings_owner(&self) -> &SettingsState;
    fn outbound_network(&self) -> &OutboundNetwork;
    fn spawn_background(&self, future: Pin<Box<dyn Future<Output = ()> + Send + 'static>>);
}

pub fn emit_non_stop_alarm(app: &impl NonStopAlarmEvents, payload: NonStopAlarmPayload) {
    app.alarm(payload);
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
    /// is returned as recovered, so the caller can emit the `stop` alarm for it.
    pub async fn ingest(&self, reports: Vec<NonStopReport>) -> Vec<String> {
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
            // returned as recovered.
            for gone in map.keys() {
                recovered.push(gone.clone());
            }
            *map = next;
        }
        recovered
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

pub fn start<H: WatchdogHost>(app: H, state: NonStopWatchdogState, shutdown: ShutdownSignal) {
    app.clone().spawn_background(Box::pin(async move {
        let mut interval = tokio::time::interval(TICK_INTERVAL);
        loop {
            tokio::select! {
                _ = shutdown.token().cancelled() => break,
                _ = interval.tick() => tick(&app, &state).await,
            }
        }
    }));
}

async fn tick(app: &impl WatchdogHost, state: &NonStopWatchdogState) {
    let now = Instant::now();
    let to_fire: Vec<NonStopReport> = {
        let mut map = state.inner.lock().await;
        collect_fireable(&mut map, now)
    };
    for r in to_fire {
        fire(app, state, &r).await;
    }
}

async fn fire(app: &impl WatchdogHost, state: &NonStopWatchdogState, r: &NonStopReport) {
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
        emit_non_stop_alarm(
            app,
            NonStopAlarmPayload {
                project_path: fresh.project_path.clone(),
                group_name: fresh.group_name.clone(),
                seconds: fresh.sound_seconds.clamp(1, 60),
                action: "start".to_string(),
            },
        );
    }
    if fresh.telegram_enabled {
        send_telegram(app, &fresh).await;
    }
}

async fn send_telegram(app: &impl WatchdogHost, r: &NonStopReport) {
    // Resolve bot from global settings: requested id, else first bot; skip if none.
    // Read the settings owner only at this Telegram resolution point.
    let resolved = {
        let settings = app.settings_owner();
        let guard = match settings.current_read().await {
            Ok(guard) => guard,
            Err(error) => {
                log::debug!("[non-stop] telegram skipped: {error}");
                return;
            }
        };
        match resolve_bot(&guard.telegram_bots, r.telegram_bot_id.as_deref()) {
            Some(b) => (b.token.clone(), b.chat_id, b.label.clone()),
            None => {
                log::warn!("[non-stop] telegram enabled but no bots configured; skipping");
                return;
            }
        }
    };
    let (token, chat_id, label) = resolved;
    let network = app.outbound_network().clone();
    let text = format!(
        "\u{26A0} Alert me! [{}]: {} {}/{} rooms working. Not working: {}. Persisted >{}s.",
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
    app.spawn_background(Box::pin(async move {
        if let Err(e) = crate::telegram::api::send_message(&network, &token, chat_id, &text).await {
            log::warn!("[non-stop] telegram send via '{}' failed: {}", label, e);
        }
    }));
}

fn project_folder_name(path: &str) -> String {
    std::path::Path::new(path)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(path)
        .to_string()
}

#[cfg(test)]
mod tests;
