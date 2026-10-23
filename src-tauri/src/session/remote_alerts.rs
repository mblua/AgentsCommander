//! #2064 Phase B - the remote-activity notifier.
//!
//! Phase A's sweeper publishes a [`RemoteTransition`] stream and `lib.rs` hands
//! its receiver here. This module owns POLICY and DELIVERY, never the transition
//! machine: the sweeper already holds the per-key last-confirmed state that
//! decides the ten-second versus thirty-second cadence, and a second copy of that
//! state would be two sources of one fact. The layering conclusion is unchanged,
//! which is the point of the split: `pty` never gains a `phone` arc.
//!
//! Per transition: read the notify dials, apply the rolling hourly cap, resolve
//! the room's orchestrator, build the notice and deliver it. Every failure is a
//! dropped notice plus a log line, except menu/typing deferrals: those retain
//! the admitted notice and retry before receiving another transition.
//!
//! The only FQN ever constructed is the one the coordinator field of the room's
//! team config resolves to. No code path here enumerates replicas, so a
//! non-orchestrator member is never a candidate, because it is never named.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures::future::BoxFuture;
use tauri::Manager;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::config::settings::SettingsState;
use crate::phone::mailbox::{
    InternalNoticeGuard, InternalSystemNotice, InternalSystemTarget, MailboxPoller,
    RemoteNoticeKind,
};
use crate::pty::remote_watcher::{base_branch_display, RemoteTransition, TransitionKind};

/// At most this many notices per `(room_dir, repo_path)` in a rolling window.
/// Twelve is six complete CI cycles, and the cap is a safety net against a
/// re-run loop, not a mechanism: the first twelve are instant, and nothing is
/// delayed to make room for them.
const NOTICE_CAP: usize = 12;
const NOTICE_WINDOW: Duration = Duration::from_secs(60 * 60);
const PENDING_RETRY_INTERVAL: Duration = Duration::from_secs(2);
type NowSource = Arc<dyn Fn() -> Instant + Send + Sync>;
type RetryWait = Arc<dyn Fn(Duration) -> BoxFuture<'static, ()> + Send + Sync>;

fn production_retry_wait() -> RetryWait {
    Arc::new(|delay| Box::pin(tokio::time::sleep(delay)))
}

/// Below this the blindness is normal operation and saying so would be noise.
/// The value is strictly above twice, and at most three times, the default
/// branch-staleness interval, so one lost round never renders the clause and
/// two always do. `blind_gap_threshold_needs_two_lost_rounds_at_the_default_interval`
/// pins that relation.
const BLIND_GAP_THRESHOLD_SECS: i64 = 600;

/// Local time with an explicit numeric offset. UTC with `Z` makes the human at
/// the terminal compare against their own clock and misread by hours; local
/// without an offset is ambiguous for the agent and worse when pasted into a
/// report; an epoch is unreadable. This serves both readers.
const TIMESTAMP_FORMAT: &str = "%Y-%m-%d %H:%M:%S%:z";

/// The two notify axes, already ANDed with their feature dials by the port.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RemoteNotifyDials {
    pub ci: bool,
    pub staleness: bool,
}

/// The notifier's three seams, all inside this module so no arc moves. They
/// follow the context-alert actor, which already routes resolution and delivery
/// through a trait returning a boxed future.
pub(crate) trait RemoteAlertPorts: Send + Sync {
    fn resolve_orchestrator(
        &self,
        room_dir: String,
    ) -> BoxFuture<'static, Option<InternalSystemTarget>>;
    fn deliver(
        &self,
        target: InternalSystemTarget,
        notice: InternalSystemNotice,
    ) -> BoxFuture<'static, Result<(), String>>;
    fn notify_dials(&self) -> BoxFuture<'static, RemoteNotifyDials>;
}

/// One owner retains the active notice through terminal mailbox completion.
pub(crate) fn start(
    app: tauri::AppHandle,
    transitions: mpsc::Receiver<RemoteTransition>,
    shutdown: CancellationToken,
) -> tauri::async_runtime::JoinHandle<()> {
    let ports = Arc::new(ProductionRemoteAlertPorts::new(app, shutdown.clone()));
    start_with_ports(ports, transitions, shutdown)
}

fn start_with_ports(
    ports: Arc<dyn RemoteAlertPorts>,
    transitions: mpsc::Receiver<RemoteTransition>,
    shutdown: CancellationToken,
) -> tauri::async_runtime::JoinHandle<()> {
    tauri::async_runtime::spawn(run_with_ports(
        ports,
        transitions,
        shutdown,
        Arc::new(Instant::now),
        production_retry_wait(),
    ))
}

async fn run_with_ports(
    ports: Arc<dyn RemoteAlertPorts>,
    mut transitions: mpsc::Receiver<RemoteTransition>,
    shutdown: CancellationToken,
    now_source: NowSource,
    retry_wait: RetryWait,
) {
    let mut state = RemoteAlertState::with_controls(ports, shutdown.clone(), retry_wait);
    loop {
        tokio::select! {
            biased;
            _ = shutdown.cancelled() => break,
            received = transitions.recv() => match received {
                Some(transition) => {
                    let now = now_source();
                    if !state.handle(transition, now).await { break; }
                }
                None => break,
            },
        }
    }
}

struct RemoteAlertState {
    ports: Arc<dyn RemoteAlertPorts>,
    /// The rolling window per `(room_dir, repo_path)`, and the rooms already
    /// warned about for the current window, so a suppressed run logs one line
    /// rather than one per round.
    windows: HashMap<(String, String), Vec<Instant>>,
    cap_warned: HashSet<(String, String)>,
    shutdown: CancellationToken,
    retry_wait: RetryWait,
    #[cfg(test)]
    admission_snapshots: Option<Arc<Mutex<Vec<usize>>>>,
}

impl RemoteAlertState {
    fn new(ports: Arc<dyn RemoteAlertPorts>) -> Self {
        Self {
            ports,
            windows: HashMap::new(),
            cap_warned: HashSet::new(),
            shutdown: CancellationToken::new(),
            retry_wait: production_retry_wait(),
            #[cfg(test)]
            admission_snapshots: None,
        }
    }

    fn with_controls(
        ports: Arc<dyn RemoteAlertPorts>,
        shutdown: CancellationToken,
        retry_wait: RetryWait,
    ) -> Self {
        let mut state = Self::new(ports);
        state.shutdown = shutdown;
        state.retry_wait = retry_wait;
        state
    }

    async fn handle(&mut self, transition: RemoteTransition, now: Instant) -> bool {
        let dials = self.ports.notify_dials().await;
        let enabled = match transition.kind {
            TransitionKind::CiStarted | TransitionKind::CiFinished => dials.ci,
            TransitionKind::BranchStale => dials.staleness,
        };
        if !enabled {
            // Dropped before any resolution, so a disabled axis costs nothing.
            return true;
        }

        let key = (transition.room_dir.clone(), transition.repo_path.clone());
        if !self.admit(&key, now) {
            return true;
        }

        let Some(target) = self
            .ports
            .resolve_orchestrator(transition.room_dir.clone())
            .await
        else {
            return true;
        };
        let notice = match notice_for(&transition) {
            Ok(notice) => notice,
            Err(reason) => {
                log::warn!(
                    "[remote-alerts] dropped an unusable notice for {}: {}",
                    transition.room_dir,
                    reason
                );
                return true;
            }
        };
        self.deliver_notice_until_terminal(target, notice, &transition.room_dir)
            .await
    }

    async fn deliver_notice_until_terminal(
        &self,
        target: InternalSystemTarget,
        notice: InternalSystemNotice,
        room_dir: &str,
    ) -> bool {
        loop {
            // Never drop an in-progress payload/Enter future from outside mailbox.
            match self.ports.deliver(target.clone(), notice.clone()).await {
                Ok(()) => return true,
                Err(reason)
                    if crate::phone::mailbox::is_deferred_internal_delivery_error(&reason) =>
                {
                    #[cfg(test)]
                    if let Some(snapshots) = &self.admission_snapshots {
                        snapshots
                            .lock()
                            .unwrap()
                            .push(self.windows.values().map(Vec::len).sum());
                    }
                }
                Err(reason) => {
                    log::warn!(
                        "[remote-alerts] delivery failed for {}: {}",
                        room_dir,
                        reason
                    );
                    return true;
                }
            }
            tokio::select! {
                biased;
                _ = self.shutdown.cancelled() => return false,
                _ = (self.retry_wait)(PENDING_RETRY_INTERVAL) => {}
            }
            if self.shutdown.is_cancelled() {
                return false;
            }
        }
    }

    /// Records the notice and answers whether it may be delivered. The window's
    /// timestamps come only from `handle`'s `now` argument, never from a clock,
    /// so no test sleeps.
    fn admit(&mut self, key: &(String, String), now: Instant) -> bool {
        let window = self.windows.entry(key.clone()).or_default();
        window.retain(|at| now.saturating_duration_since(*at) < NOTICE_WINDOW);
        if window.len() >= NOTICE_CAP {
            if self.cap_warned.insert(key.clone()) {
                log::warn!(
                    "[remote-alerts] hourly notice cap reached for room {} repo {}; \
                     further notices are suppressed until the window rolls",
                    key.0,
                    key.1
                );
            }
            return false;
        }
        if window.is_empty() {
            self.cap_warned.remove(key);
        }
        window.push(now);
        true
    }
}

/// The remote-activity kind, mapped exhaustively so a new transition kind can
/// never fall through to a default.
fn notice_for(transition: &RemoteTransition) -> Result<InternalSystemNotice, String> {
    let kind = match transition.kind {
        TransitionKind::CiStarted => RemoteNoticeKind::CiStarted,
        TransitionKind::CiFinished => RemoteNoticeKind::CiFinished,
        TransitionKind::BranchStale => RemoteNoticeKind::BranchStale,
    };
    // Seven characters, because that is what a human and an agent paste into a
    // command. The 40-character SHA goes to the QUERY, never to the text, where
    // an abbreviated one would make the CI answer a confident, silent zero.
    let sha7: String = transition.head_sha.chars().take(7).collect();
    InternalSystemNotice::for_remote_activity(
        kind,
        transition.nwo.clone(),
        transition.branch.clone(),
        sha7,
        // #2129 - branch-stale text only. CI kinds carry an empty base branch,
        // which `for_remote_activity` requires to stay empty (mailbox.rs:319).
        if matches!(kind, RemoteNoticeKind::BranchStale) {
            base_branch_display(&transition.branch, &transition.base_branch)
        } else {
            transition.base_branch.clone()
        },
        transition.behind_by,
        transition.observed_at.format(TIMESTAMP_FORMAT).to_string(),
        blind_gap(transition),
    )
}

/// The blind-gap clause, rendered only when the gap since the last CONFIRMED
/// observation for this key exceeds the threshold. `last_confirmed_at == None`
/// appends nothing: there is no previous confirmed state to name.
fn blind_gap(transition: &RemoteTransition) -> Option<(String, String)> {
    let last = transition.last_confirmed_at?;
    let seconds = transition
        .observed_at
        .signed_duration_since(last)
        .num_seconds();
    if seconds <= BLIND_GAP_THRESHOLD_SECS {
        return None;
    }
    Some((
        human_gap(seconds),
        last.format(TIMESTAMP_FORMAT).to_string(),
    ))
}

/// Minutes below the hour, hours and minutes above it, never seconds: a reader
/// does not convert 2820 into 47 minutes. The threshold is above 120 seconds, so
/// `minutes >= 2` and the singular never arises.
fn human_gap(seconds: i64) -> String {
    let minutes = seconds / 60;
    if minutes < 60 {
        return format!("{} minutes", minutes);
    }
    let hours = minutes / 60;
    let rest = minutes % 60;
    if rest == 0 {
        format!("{} hours", hours)
    } else {
        format!("{} hours {} minutes", hours, rest)
    }
}

// ---------------------------------------------------------------------------
// Containment
// ---------------------------------------------------------------------------

/// Rejects a link, and on Windows a reparse point, then canonicalizes. Both
/// behaviours of the context-alert resolver's private helper are kept: importing
/// that helper would add a `session::remote_alerts` arc the phase's cycle
/// condition forbids, and a faithful copy would have to name two more modules,
/// each of which is another forbidden arc.
fn real_canonical_dir(path: &Path) -> Option<PathBuf> {
    let metadata = std::fs::symlink_metadata(path).ok()?;
    if !metadata.is_dir() {
        return None;
    }
    if metadata.file_type().is_symlink() {
        return None;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        /// `FILE_ATTRIBUTE_REPARSE_POINT`.
        const REPARSE_POINT: u32 = 0x400;
        if metadata.file_attributes() & REPARSE_POINT != 0 {
            return None;
        }
    }
    std::fs::canonicalize(path).ok()
}

/// BOTH comparands go through [`real_canonical_dir`], so a `..`, a `.` or a case
/// spelling cannot read as an escape, and both sides carry the same verbatim
/// prefix, so no prefix stripping is needed. Comparing one canonical side
/// against one raw side would drop every transition.
fn replica_within_room(room_dir: &Path, replica_dir: &Path) -> bool {
    let (Some(room), Some(replica)) = (
        real_canonical_dir(room_dir),
        real_canonical_dir(replica_dir),
    ) else {
        return false;
    };
    replica.parent() == Some(room.as_path())
}

/// The FQN, spelled from the CANONICAL directory names.
///
/// The target constructor accepts an FQN only if it equals the one it rebuilds
/// from the canonical replica, and that rebuild uses the on-disk case. The
/// resolver, by contrast, spells the project and the room with whatever case the
/// caller passed, so a settings file or a discovery scan that spells a room
/// `Room-26-...` would produce an FQN the constructor refuses and every notice
/// for that room would be dropped. Both sides describe the same replica: the
/// canonical replica directory this FQN is built from is the one the resolver
/// selected by identity, and containment has already proved it is a child of
/// this room.
fn canonical_fqn(canonical_room: &Path, canonical_replica: &Path) -> Option<String> {
    let agent = canonical_replica
        .file_name()?
        .to_str()?
        .strip_prefix("__agent_")?;
    let room = canonical_room.file_name()?.to_str()?;
    let project = canonical_room.parent()?.parent()?.file_name()?.to_str()?;
    Some(format!("{project}:{room}/{agent}"))
}

/// One warn per room for the conditions that repeat every round: the sweeper
/// ticks every ten seconds, so an unconfigured room would otherwise fill the log.
fn warn_room_once(warned: &Mutex<HashSet<String>>, room: &Path, message: String) {
    let first = match warned.lock() {
        Ok(mut warned) => warned.insert(room.to_string_lossy().to_string()),
        // A poisoned set must never cost the operator the message.
        Err(_) => true,
    };
    if first {
        log::warn!("{}", message);
    }
}

/// The blocking half of resolution, on its own so the tests can exercise the
/// real path instead of a stub.
fn resolve_orchestrator_blocking(
    room_dir: &Path,
    warned: &Mutex<HashSet<String>>,
) -> Option<InternalSystemTarget> {
    let ac_root = room_dir.parent()?;
    let Some(resolved) = crate::config::teams::resolve_wg_coordinator_replica(ac_root, room_dir)
    else {
        warn_room_once(
            warned,
            room_dir,
            format!(
                "[remote-alerts] no orchestrator is configured for room {}; notice dropped",
                room_dir.display()
            ),
        );
        return None;
    };
    if !replica_within_room(room_dir, &resolved.replica_dir) {
        warn_room_once(
            warned,
            room_dir,
            format!(
                "[remote-alerts] resolved orchestrator replica {} \
                 is not a direct child of room {}; notice dropped",
                resolved.replica_dir.display(),
                room_dir.display()
            ),
        );
        return None;
    }
    let Some(canonical_room) = real_canonical_dir(room_dir) else {
        warn_room_once(
            warned,
            room_dir,
            format!(
                "[remote-alerts] room {} cannot be canonicalized; notice dropped",
                room_dir.display()
            ),
        );
        return None;
    };
    let Some(canonical) = real_canonical_dir(&resolved.replica_dir) else {
        warn_room_once(
            warned,
            room_dir,
            format!(
                "[remote-alerts] orchestrator replica {} cannot be canonicalized; notice dropped",
                resolved.replica_dir.display()
            ),
        );
        return None;
    };
    let Some(fqn) = canonical_fqn(&canonical_room, &canonical) else {
        warn_room_once(
            warned,
            room_dir,
            format!(
                "[remote-alerts] orchestrator replica {} is not named as a room replica; \
                 notice dropped",
                canonical.display()
            ),
        );
        return None;
    };
    match InternalSystemTarget::for_context_alert(fqn, canonical) {
        Ok(target) => Some(target),
        Err(reason) => {
            warn_room_once(
                warned,
                room_dir,
                format!(
                    "[remote-alerts] resolved orchestrator target is unusable for room {}: {}",
                    room_dir.display(),
                    reason
                ),
            );
            None
        }
    }
}

// ---------------------------------------------------------------------------
// Production ports
// ---------------------------------------------------------------------------

struct ProductionRemoteAlertPorts {
    app: tauri::AppHandle,
    shutdown: CancellationToken,
    warned: Arc<Mutex<HashSet<String>>>,
}

impl ProductionRemoteAlertPorts {
    fn new(app: tauri::AppHandle, shutdown: CancellationToken) -> Self {
        Self {
            app,
            shutdown,
            warned: Arc::new(Mutex::new(HashSet::new())),
        }
    }
}

impl RemoteAlertPorts for ProductionRemoteAlertPorts {
    fn resolve_orchestrator(
        &self,
        room_dir: String,
    ) -> BoxFuture<'static, Option<InternalSystemTarget>> {
        let warned = Arc::clone(&self.warned);
        Box::pin(async move {
            // Resolution does synchronous `read_dir` and `read_to_string`, so it
            // belongs on the blocking pool.
            tauri::async_runtime::spawn_blocking(move || {
                resolve_orchestrator_blocking(Path::new(&room_dir), &warned)
            })
            .await
            .ok()
            .flatten()
        })
    }

    fn deliver(
        &self,
        target: InternalSystemTarget,
        notice: InternalSystemNotice,
    ) -> BoxFuture<'static, Result<(), String>> {
        let app = self.app.clone();
        let cancellation = self.shutdown.child_token();
        Box::pin(async move {
            // The PERMISSIVE guard, built here rather than copied from the
            // context-alert actor: a remote notice has no alert state to
            // re-check, and a purge in progress is already deferred inside the
            // mailbox, so a second check would be a second source of one fact.
            let guard: InternalNoticeGuard = Arc::new(|| Ok(()));
            MailboxPoller::new()
                .deliver_internal_system_notice(&app, target, notice, cancellation, guard)
                .await
        })
    }

    fn notify_dials(&self) -> BoxFuture<'static, RemoteNotifyDials> {
        let app = self.app.clone();
        Box::pin(async move {
            // An axis that cannot be read is not an axis that is on: delivering
            // an injection the operator switched off is the one irreversible
            // mistake here.
            let Some(state) = app.try_state::<SettingsState>() else {
                return RemoteNotifyDials {
                    ci: false,
                    staleness: false,
                };
            };
            let settings = match state.current_read().await {
                Ok(settings) => settings,
                Err(_) => {
                    log::debug!("[RemoteAlerts] settings unavailable; denying notifications");
                    return RemoteNotifyDials {
                        ci: false,
                        staleness: false,
                    };
                }
            };
            RemoteNotifyDials {
                ci: settings.ci_activity_enabled && settings.ci_activity_notify_orchestrator,
                staleness: settings.branch_staleness_enabled
                    && settings.branch_staleness_notify_orchestrator,
            }
        })
    }
}
#[cfg(test)]
mod tests;
