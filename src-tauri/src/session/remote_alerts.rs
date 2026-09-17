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
//! dropped notice plus a log line, never a retry: the sweeper emits a fresh
//! transition on the next state change.
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
use crate::pty::remote_watcher::{RemoteTransition, TransitionKind};

/// At most this many notices per `(room_dir, repo_path)` in a rolling window.
/// Twelve is six complete CI cycles, and the cap is a safety net against a
/// re-run loop, not a mechanism: the first twelve are instant, and nothing is
/// delayed to make room for them.
const NOTICE_CAP: usize = 12;
const NOTICE_WINDOW: Duration = Duration::from_secs(60 * 60);

/// About ten sweeper rounds. Below this the blindness is normal operation and
/// saying so would be noise.
const BLIND_GAP_THRESHOLD_SECS: i64 = 300;

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

/// The production loop is only this `select!` plus one `handle` call per
/// transition, so the tests below drive `handle` directly with their own clock
/// and their own ports.
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
    mut transitions: mpsc::Receiver<RemoteTransition>,
    shutdown: CancellationToken,
) -> tauri::async_runtime::JoinHandle<()> {
    tauri::async_runtime::spawn(async move {
        let mut state = RemoteAlertState::new(ports);
        loop {
            tokio::select! {
                biased;
                _ = shutdown.cancelled() => break,
                received = transitions.recv() => match received {
                    Some(transition) => state.handle(transition, Instant::now()).await,
                    // The sweeper is gone, so nothing can arrive again.
                    None => break,
                },
            }
        }
    })
}

struct RemoteAlertState {
    ports: Arc<dyn RemoteAlertPorts>,
    /// The rolling window per `(room_dir, repo_path)`, and the rooms already
    /// warned about for the current window, so a suppressed run logs one line
    /// rather than one per round.
    windows: HashMap<(String, String), Vec<Instant>>,
    cap_warned: HashSet<(String, String)>,
}

impl RemoteAlertState {
    fn new(ports: Arc<dyn RemoteAlertPorts>) -> Self {
        Self {
            ports,
            windows: HashMap::new(),
            cap_warned: HashSet::new(),
        }
    }

    async fn handle(&mut self, transition: RemoteTransition, now: Instant) {
        let dials = self.ports.notify_dials().await;
        let enabled = match transition.kind {
            TransitionKind::CiStarted | TransitionKind::CiFinished => dials.ci,
            TransitionKind::BranchStale => dials.staleness,
        };
        if !enabled {
            // Dropped before any resolution, so a disabled axis costs nothing.
            return;
        }

        let key = (transition.room_dir.clone(), transition.repo_path.clone());
        if !self.admit(&key, now) {
            return;
        }

        let Some(target) = self
            .ports
            .resolve_orchestrator(transition.room_dir.clone())
            .await
        else {
            return;
        };
        let notice = match notice_for(&transition) {
            Ok(notice) => notice,
            Err(reason) => {
                log::warn!(
                    "[remote-alerts] dropped an unusable notice for {}: {}",
                    transition.room_dir,
                    reason
                );
                return;
            }
        };
        if let Err(reason) = self.ports.deliver(target, notice).await {
            log::warn!(
                "[remote-alerts] delivery failed for {}: {}",
                transition.room_dir,
                reason
            );
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
                    "[remote-alerts] hourly notice cap reached for room {} repo {}; further notices are suppressed until the window rolls",
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
        transition.base_branch.clone(),
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
/// does not convert 2820 into 47 minutes. The threshold guarantees at least five
/// minutes, so the singular never arises.
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
                "[remote-alerts] resolved orchestrator replica {} is not a direct child of room {}; notice dropped",
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
                "[remote-alerts] orchestrator replica {} is not named as a room replica; notice dropped",
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
            let settings = state.read().await;
            RemoteNotifyDials {
                ci: settings.ci_activity_enabled && settings.ci_activity_notify_orchestrator,
                staleness: settings.branch_staleness_enabled
                    && settings.branch_staleness_notify_orchestrator,
            }
        })
    }
}
#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use chrono::{DateTime, Local};

    use super::*;

    // ---- fixtures --------------------------------------------------------

    struct RoomFixture {
        _temp: tempfile::TempDir,
        room: PathBuf,
        coordinator_replica: PathBuf,
    }

    fn replica_config(agent: &str) -> String {
        format!(r#"{{"identity":"../../_agent_{agent}","context":[],"repos":[]}}"#)
    }

    const TEAM_CONFIG: &str = r#"{"agents":["_agent_member","_agent_coordinator","_agent_other"],"coordinator":"_agent_coordinator","repos":[],"contextAlertPercentages":[50,75]}"#;

    /// A room with three replicas, one of them the configured orchestrator.
    /// Deliberately not a helper shared with the context-alert tests: importing
    /// theirs would add an arc the phase's cycle condition forbids.
    fn room_fixture() -> RoomFixture {
        let temp = tempfile::tempdir().expect("temp dir");
        let ac_root = temp.path().join("project-a").join(".ac");
        let room = ac_root.join("wg-2-dev-team");
        let coordinator = room.join("__agent_coordinator");
        for path in [
            &coordinator,
            &room.join("__agent_member"),
            &room.join("__agent_other"),
            &ac_root.join("_agent_coordinator"),
            &ac_root.join("_agent_member"),
            &ac_root.join("_agent_other"),
            &ac_root.join("_team_dev-team"),
        ] {
            std::fs::create_dir_all(path).expect("fixture dir");
        }
        for (dir, agent) in [
            ("__agent_coordinator", "coordinator"),
            ("__agent_member", "member"),
            ("__agent_other", "other"),
        ] {
            std::fs::write(room.join(dir).join("config.json"), replica_config(agent))
                .expect("replica config");
        }
        std::fs::write(
            ac_root.join("_team_dev-team").join("config.json"),
            TEAM_CONFIG,
        )
        .expect("team config");
        RoomFixture {
            _temp: temp,
            room,
            coordinator_replica: coordinator,
        }
    }

    /// Creates the link the way the platform needs it, and FAILS rather than
    /// skipping, so the coverage cannot silently evaporate on a machine where
    /// the link cannot be created.
    fn link_directory(link: &Path, target: &Path) {
        #[cfg(windows)]
        {
            let output = std::process::Command::new("cmd")
                .args(["/c", "mklink", "/J"])
                .arg(link)
                .arg(target)
                .output()
                .expect("run mklink");
            assert!(
                output.status.success(),
                "mklink /J failed: {}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        #[cfg(not(windows))]
        {
            std::os::unix::fs::symlink(target, link).expect("symlink");
        }
    }

    fn dials(ci: bool, staleness: bool) -> RemoteNotifyDials {
        RemoteNotifyDials { ci, staleness }
    }

    /// A real `DateTime<Local>`, built from a fixed instant rather than the wall
    /// clock: the wall clock must not be read anywhere in this file.
    fn observed_at() -> DateTime<Local> {
        DateTime::parse_from_rfc3339("2026-09-15T23:18:43-03:00")
            .expect("fixture timestamp")
            .with_timezone(&Local)
    }

    /// Shaped exactly as Phase A's transitions are: an empty base branch and no
    /// commit count for the two CI kinds, both set for a stale branch.
    fn transition(kind: TransitionKind, room: &Path, repo: &str) -> RemoteTransition {
        let stale = matches!(kind, TransitionKind::BranchStale);
        RemoteTransition {
            repo_path: repo.to_string(),
            room_dir: room.to_string_lossy().to_string(),
            nwo: "mblua/AgentsCommander".to_string(),
            branch: "feature/2083-2064-remote-alerts".to_string(),
            head_sha: "abcdef1".repeat(5) + "abcde",
            kind,
            behind_by: stale.then_some(4),
            base_branch: if stale {
                "main".to_string()
            } else {
                String::new()
            },
            observed_at: observed_at(),
            last_confirmed_at: Some(observed_at()),
        }
    }

    fn production_target(room: &Path) -> InternalSystemTarget {
        resolve_orchestrator_blocking(room, &Mutex::new(HashSet::new())).expect("fixture target")
    }

    #[derive(Clone, Debug)]
    struct Delivery {
        target_fqn: String,
        kind: RemoteNoticeKind,
        at: String,
        blind_gap: Option<(String, String)>,
    }

    /// The variant's fields, never the rendered line: `line()` is private to
    /// `phone::mailbox`, so a test that cannot call it must not pretend to.
    fn delivery_of(target: &InternalSystemTarget, notice: &InternalSystemNotice) -> Delivery {
        match notice {
            InternalSystemNotice::RemoteActivity {
                kind,
                at,
                blind_gap,
                ..
            } => Delivery {
                target_fqn: target.fqn().to_string(),
                kind: *kind,
                at: at.clone(),
                blind_gap: blind_gap.clone(),
            },
            InternalSystemNotice::ContextAlert { .. } => panic!("a remote activity notice"),
        }
    }

    struct RecordingPorts {
        dials: Mutex<RemoteNotifyDials>,
        target: Mutex<Option<InternalSystemTarget>>,
        resolutions: AtomicUsize,
        deliveries: Mutex<Vec<Delivery>>,
    }

    impl RecordingPorts {
        fn new(dials: RemoteNotifyDials, target: Option<InternalSystemTarget>) -> Self {
            Self {
                dials: Mutex::new(dials),
                target: Mutex::new(target),
                resolutions: AtomicUsize::new(0),
                deliveries: Mutex::new(Vec::new()),
            }
        }

        fn set_dials(&self, dials: RemoteNotifyDials) {
            *self.dials.lock().unwrap_or_else(|error| error.into_inner()) = dials;
        }

        fn resolutions(&self) -> usize {
            self.resolutions.load(Ordering::SeqCst)
        }

        fn deliveries(&self) -> Vec<Delivery> {
            self.deliveries
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .clone()
        }
    }

    impl RemoteAlertPorts for RecordingPorts {
        fn resolve_orchestrator(
            &self,
            _room_dir: String,
        ) -> BoxFuture<'static, Option<InternalSystemTarget>> {
            self.resolutions.fetch_add(1, Ordering::SeqCst);
            let target = self
                .target
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .clone();
            Box::pin(async move { target })
        }

        fn deliver(
            &self,
            target: InternalSystemTarget,
            notice: InternalSystemNotice,
        ) -> BoxFuture<'static, Result<(), String>> {
            self.deliveries
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .push(delivery_of(&target, &notice));
            Box::pin(async move { Ok(()) })
        }

        fn notify_dials(&self) -> BoxFuture<'static, RemoteNotifyDials> {
            let dials = *self.dials.lock().unwrap_or_else(|error| error.into_inner());
            Box::pin(async move { dials })
        }
    }

    /// Resolves through the production path, so a containment drop is observed at
    /// `handle` and not only at the helper.
    #[derive(Default)]
    struct ResolvingPorts {
        deliveries: Mutex<Vec<Delivery>>,
    }

    impl ResolvingPorts {
        fn deliveries(&self) -> Vec<Delivery> {
            self.deliveries
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .clone()
        }
    }

    impl RemoteAlertPorts for ResolvingPorts {
        fn resolve_orchestrator(
            &self,
            room_dir: String,
        ) -> BoxFuture<'static, Option<InternalSystemTarget>> {
            Box::pin(async move {
                tauri::async_runtime::spawn_blocking(move || {
                    resolve_orchestrator_blocking(Path::new(&room_dir), &Mutex::new(HashSet::new()))
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
            self.deliveries
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .push(delivery_of(&target, &notice));
            Box::pin(async move { Ok(()) })
        }

        fn notify_dials(&self) -> BoxFuture<'static, RemoteNotifyDials> {
            Box::pin(async move { dials(true, true) })
        }
    }

    // ---- 13..20: policy ---------------------------------------------------

    #[tokio::test]
    async fn notify_dial_off_drops_before_resolution() {
        let ports = Arc::new(RecordingPorts::new(dials(false, true), None));
        let mut state = RemoteAlertState::new(ports.clone());
        let now = Instant::now();
        state
            .handle(
                transition(TransitionKind::CiStarted, Path::new("room"), "repo"),
                now,
            )
            .await;
        assert_eq!(
            ports.resolutions(),
            0,
            "a disabled CI axis resolves nothing"
        );

        ports.set_dials(dials(true, false));
        state
            .handle(
                transition(TransitionKind::BranchStale, Path::new("room"), "repo"),
                now,
            )
            .await;
        assert_eq!(
            ports.resolutions(),
            0,
            "a disabled staleness axis resolves nothing"
        );
        assert!(ports.deliveries().is_empty());
    }

    #[tokio::test]
    async fn ci_notice_from_a_sweeper_transition_is_delivered() {
        let fixture = room_fixture();
        let ports = Arc::new(RecordingPorts::new(
            dials(true, false),
            Some(production_target(&fixture.room)),
        ));
        let mut state = RemoteAlertState::new(ports.clone());
        let now = Instant::now();
        for kind in [TransitionKind::CiStarted, TransitionKind::CiFinished] {
            state
                .handle(transition(kind, &fixture.room, "repo-a"), now)
                .await;
        }

        let delivered = ports.deliveries();
        assert_eq!(delivered.len(), 2, "the CI axis must not ship dark");
        assert_eq!(delivered[0].kind, RemoteNoticeKind::CiStarted);
        assert_eq!(delivered[1].kind, RemoteNoticeKind::CiFinished);
        assert_eq!(
            delivered[0].target_fqn,
            "project-a:wg-2-dev-team/coordinator"
        );
    }

    #[tokio::test]
    async fn only_the_configured_orchestrator_is_ever_targeted() {
        let fixture = room_fixture();
        let ports = Arc::new(ResolvingPorts::default());
        let mut state = RemoteAlertState::new(ports.clone());
        state
            .handle(
                transition(TransitionKind::CiStarted, &fixture.room, "repo-a"),
                Instant::now(),
            )
            .await;

        let delivered = ports.deliveries();
        assert_eq!(delivered.len(), 1, "three replicas, exactly one target");
        assert_eq!(
            delivered[0].target_fqn,
            "project-a:wg-2-dev-team/coordinator"
        );
        assert!(
            delivered
                .iter()
                .all(|delivery| !delivery.target_fqn.contains("member")
                    && !delivery.target_fqn.contains("other")),
            "no code path enumerates the non-orchestrator replicas"
        );
    }

    #[test]
    fn resolved_replica_outside_the_room_is_dropped() {
        let temp = tempfile::tempdir().expect("temp dir");
        let ac_root = temp.path().join("project-a").join(".ac");
        let room = ac_root.join("wg-2-dev-team");
        let outside = temp.path().join("elsewhere").join("__agent_coordinator");
        for path in [
            &outside,
            &temp.path().join("_agent_coordinator"),
            &ac_root.join("_agent_coordinator"),
            &ac_root.join("_team_dev-team"),
            &room,
        ] {
            std::fs::create_dir_all(path).expect("fixture dir");
        }
        std::fs::write(outside.join("config.json"), replica_config("coordinator"))
            .expect("replica config");
        std::fs::write(
            ac_root.join("_team_dev-team").join("config.json"),
            r#"{"agents":["_agent_coordinator"],"coordinator":"_agent_coordinator","repos":[]}"#,
        )
        .expect("team config");

        // A real directory outside the room is never inside it.
        assert!(!replica_within_room(&room, &outside));

        // The resolver DOES find the orchestrator through the link; containment
        // is what drops it.
        link_directory(&room.join("__agent_coordinator"), &outside);
        assert!(
            crate::config::teams::resolve_wg_coordinator_replica(&ac_root, &room).is_some(),
            "the fixture must resolve, or this test proves nothing"
        );
        assert!(
            resolve_orchestrator_blocking(&room, &Mutex::new(HashSet::new())).is_none(),
            "a notice addressed through a replica outside the room is dropped"
        );
    }

    #[tokio::test]
    async fn cap_allows_twelve_then_suppresses_silently() {
        let fixture = room_fixture();
        let ports = Arc::new(RecordingPorts::new(
            dials(true, true),
            Some(production_target(&fixture.room)),
        ));
        let mut state = RemoteAlertState::new(ports.clone());
        let now = Instant::now();
        for _ in 0..12 {
            state
                .handle(
                    transition(TransitionKind::CiStarted, &fixture.room, "repo-a"),
                    now,
                )
                .await;
        }
        for _ in 0..3 {
            state
                .handle(
                    transition(TransitionKind::BranchStale, &fixture.room, "repo-a"),
                    now,
                )
                .await;
        }

        let delivered = ports.deliveries();
        assert_eq!(delivered.len(), 12);
        assert!(
            delivered
                .iter()
                .all(|delivery| delivery.kind == RemoteNoticeKind::CiStarted),
            "the first twelve transitions are the ones delivered"
        );
    }

    #[tokio::test]
    async fn cap_window_is_rolling_and_per_room_repo_pair() {
        let fixture = room_fixture();
        let ports = Arc::new(RecordingPorts::new(
            dials(true, true),
            Some(production_target(&fixture.room)),
        ));
        let mut state = RemoteAlertState::new(ports.clone());
        let now = Instant::now();
        let room = fixture.room.clone();

        for _ in 0..12 {
            state
                .handle(transition(TransitionKind::CiStarted, &room, "repo-a"), now)
                .await;
        }
        state
            .handle(transition(TransitionKind::CiStarted, &room, "repo-b"), now)
            .await;
        assert_eq!(
            ports.deliveries().len(),
            13,
            "the window is keyed by the (room, repo) pair, not by the room"
        );

        state
            .handle(
                transition(TransitionKind::CiStarted, &room, "repo-a"),
                now + Duration::from_secs(3599),
            )
            .await;
        assert_eq!(
            ports.deliveries().len(),
            13,
            "inside the window the cap still holds"
        );

        state
            .handle(
                transition(TransitionKind::CiStarted, &room, "repo-a"),
                now + Duration::from_secs(3601),
            )
            .await;
        assert_eq!(
            ports.deliveries().len(),
            14,
            "the window is rolling, not a fixed bucket"
        );
    }

    #[tokio::test]
    async fn blind_gap_threshold_is_three_hundred_seconds() {
        let fixture = room_fixture();
        let ports = Arc::new(RecordingPorts::new(
            dials(true, true),
            Some(production_target(&fixture.room)),
        ));
        let mut state = RemoteAlertState::new(ports.clone());
        let now = Instant::now();

        let mut short = transition(TransitionKind::CiStarted, &fixture.room, "repo-a");
        short.last_confirmed_at = Some(observed_at() - chrono::Duration::seconds(299));
        let mut long = transition(TransitionKind::CiStarted, &fixture.room, "repo-a");
        long.last_confirmed_at = Some(observed_at() - chrono::Duration::seconds(301));
        let mut first_ever = transition(TransitionKind::CiStarted, &fixture.room, "repo-a");
        first_ever.last_confirmed_at = None;
        for candidate in [short, long, first_ever] {
            state.handle(candidate, now).await;
        }

        let delivered = ports.deliveries();
        assert_eq!(delivered.len(), 3);
        assert_eq!(delivered[0].blind_gap, None, "299 s is not a blind gap");
        assert_eq!(
            delivered[1].blind_gap.as_ref().map(|(gap, _)| gap.as_str()),
            Some("5 minutes"),
            "301 s is"
        );
        assert_eq!(
            delivered[2].blind_gap, None,
            "no previous confirmed state appends nothing"
        );
    }

    #[tokio::test]
    async fn timestamp_format_is_local_with_a_numeric_offset() {
        let fixture = room_fixture();
        let ports = Arc::new(RecordingPorts::new(
            dials(true, false),
            Some(production_target(&fixture.room)),
        ));
        let mut state = RemoteAlertState::new(ports.clone());
        state
            .handle(
                transition(TransitionKind::CiStarted, &fixture.room, "repo-a"),
                Instant::now(),
            )
            .await;

        let at = ports.deliveries()[0].at.clone();
        assert!(
            is_offset_timestamp(&at),
            "the observation time must be local with a numeric offset: {at}"
        );
    }

    /// `^\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}[+-]\d{2}:\d{2}$`, spelled out rather
    /// than matched with a regex: a `to_rfc3339()` of UTC would still look like a
    /// timestamp to the eye, and this is the only assertion that catches it.
    fn is_offset_timestamp(value: &str) -> bool {
        let bytes = value.as_bytes();
        if bytes.len() != 25 {
            return false;
        }
        let digits = |range: std::ops::Range<usize>| bytes[range].iter().all(u8::is_ascii_digit);
        digits(0..4)
            && bytes[4] == b'-'
            && digits(5..7)
            && bytes[7] == b'-'
            && digits(8..10)
            && bytes[10] == b' '
            && digits(11..13)
            && bytes[13] == b':'
            && digits(14..16)
            && bytes[16] == b':'
            && digits(17..19)
            && matches!(bytes[19], b'+' | b'-')
            && digits(20..22)
            && bytes[22] == b':'
            && digits(23..25)
    }

    #[test]
    fn gap_is_rendered_in_human_units() {
        assert_eq!(human_gap(2820), "47 minutes");
        assert_eq!(human_gap(7500), "2 hours 5 minutes");
        assert_eq!(human_gap(7200), "2 hours");
        assert!(!human_gap(2820).contains("2820"));
    }

    // ---- 21..23: lifecycle and containment --------------------------------

    #[tokio::test]
    async fn channel_close_ends_the_monitor() {
        let ports = Arc::new(RecordingPorts::new(dials(true, false), None));
        let (sender, receiver) = mpsc::channel(4);
        let handle = start_with_ports(ports.clone(), receiver, CancellationToken::new());
        sender
            .send(transition(
                TransitionKind::CiStarted,
                Path::new("room"),
                "repo",
            ))
            .await
            .expect("queued before the close");
        drop(sender);

        tokio::time::timeout(Duration::from_secs(10), handle)
            .await
            .expect("the monitor ends when the sweeper's channel closes")
            .expect("the monitor task must not panic");
    }

    #[tokio::test]
    async fn shutdown_ends_the_monitor() {
        let ports = Arc::new(RecordingPorts::new(dials(true, true), None));
        let (_sender, receiver) = mpsc::channel(4);
        let cancellation = CancellationToken::new();
        let handle = start_with_ports(ports.clone(), receiver, cancellation.clone());
        cancellation.cancel();

        tokio::time::timeout(Duration::from_secs(10), handle)
            .await
            .expect("the monitor ends on shutdown")
            .expect("the monitor task must not panic");
        assert_eq!(ports.resolutions(), 0, "nothing is resolved after shutdown");
    }

    #[tokio::test]
    async fn containment_canonicalizes_both_comparands() {
        let fixture = room_fixture();
        // A `..` hop on the way to the room, and a `.`/`..` detour inside the
        // replica path: the same directories, so neither reads as an escape.
        let hop = fixture._temp.path().join("hop");
        std::fs::create_dir_all(&hop).expect("hop dir");
        let indirect_room = hop
            .join("..")
            .join("project-a")
            .join(".ac")
            .join("wg-2-dev-team");
        let indirect_replica = fixture
            .coordinator_replica
            .join(".")
            .join("..")
            .join("__agent_coordinator");
        assert!(replica_within_room(
            &indirect_room,
            &fixture.coordinator_replica
        ));
        assert!(replica_within_room(&fixture.room, &indirect_replica));

        // And the real resolver still resolves and delivers through that spelling.
        let ports = Arc::new(ResolvingPorts::default());
        let mut state = RemoteAlertState::new(ports.clone());
        state
            .handle(
                transition(TransitionKind::CiStarted, &indirect_room, "repo-a"),
                Instant::now(),
            )
            .await;
        let delivered = ports.deliveries();
        assert_eq!(
            delivered.len(),
            1,
            "a non-canonical spelling is the same room"
        );
        assert_eq!(
            delivered[0].target_fqn,
            "project-a:wg-2-dev-team/coordinator"
        );

        // A directory that is not a child of the room is still outside it.
        let outside = fixture._temp.path().join("elsewhere");
        std::fs::create_dir_all(&outside).expect("outside dir");
        assert!(!replica_within_room(&fixture.room, &outside));

        #[cfg(windows)]
        {
            // Windows resolves a differently-cased spelling to the same directory.
            // Only the room's parents are shouted: the room's own name carries the
            // `wg-` entity prefix, and the entity parser is case-sensitive, so
            // shouting that component would test the parser and not containment.
            let shouted = PathBuf::from(
                fixture
                    .room
                    .parent()
                    .expect("room parent")
                    .to_string_lossy()
                    .to_uppercase(),
            )
            .join(fixture.room.file_name().expect("room name"));
            let shouted_ac_root = shouted.parent().expect("ac root");
            let resolved =
                crate::config::teams::resolve_wg_coordinator_replica(shouted_ac_root, &shouted)
                    .expect("the resolver reads a case variant");
            assert!(replica_within_room(&shouted, &fixture.coordinator_replica));
            assert!(
                replica_within_room(&shouted, &resolved.replica_dir),
                "a case variant resolves to a replica this room contains"
            );
            // The delivery half IS asserted: the notice is addressed by the
            // canonical spelling, so a settings file or a discovery scan that
            // spells the room with different case does not lose every notice.
            let ports = Arc::new(ResolvingPorts::default());
            let mut state = RemoteAlertState::new(ports.clone());
            state
                .handle(
                    transition(TransitionKind::CiStarted, &shouted, "repo-a"),
                    Instant::now(),
                )
                .await;
            let delivered = ports.deliveries();
            assert_eq!(
                delivered.len(),
                1,
                "a differently-cased path is the same room and must deliver"
            );
            assert_eq!(
                delivered[0].target_fqn, "project-a:wg-2-dev-team/coordinator",
                "the FQN is spelled from the canonical directory names"
            );
        }
    }

    #[tokio::test]
    async fn containment_rejects_a_link_or_reparse_replica() {
        let temp = tempfile::tempdir().expect("temp dir");
        let ac_root = temp.path().join("project-a").join(".ac");
        let room = ac_root.join("wg-2-dev-team");
        let real_inside = room.join("__agent_real");
        for path in [
            &real_inside,
            &ac_root.join("_agent_coordinator"),
            &ac_root.join("_team_dev-team"),
        ] {
            std::fs::create_dir_all(path).expect("fixture dir");
        }
        std::fs::write(
            real_inside.join("config.json"),
            replica_config("coordinator"),
        )
        .expect("replica config");
        std::fs::write(
            ac_root.join("_team_dev-team").join("config.json"),
            r#"{"agents":["_agent_coordinator"],"coordinator":"_agent_coordinator","repos":[]}"#,
        )
        .expect("team config");

        // First as a real directory: the resolver finds it, so the drop below is
        // caused by the link and not by an unresolvable fixture.
        std::fs::create_dir_all(room.join("__agent_coordinator")).expect("real replica");
        std::fs::write(
            room.join("__agent_coordinator").join("config.json"),
            replica_config("coordinator"),
        )
        .expect("replica config");
        assert!(
            resolve_orchestrator_blocking(&room, &Mutex::new(HashSet::new())).is_some(),
            "the fixture must resolve before the link replaces it"
        );
        std::fs::remove_dir_all(room.join("__agent_coordinator")).expect("drop the real replica");

        // Now the configured coordinator replica is a LINK to a real directory
        // INSIDE the room, so only the link check can reject it: the parent check
        // would pass.
        link_directory(&room.join("__agent_coordinator"), &real_inside);
        assert!(!replica_within_room(
            &room,
            &room.join("__agent_coordinator")
        ));

        let ports = Arc::new(ResolvingPorts::default());
        let mut state = RemoteAlertState::new(ports.clone());
        state
            .handle(
                transition(TransitionKind::CiStarted, &room, "repo-a"),
                Instant::now(),
            )
            .await;
        assert!(
            ports.deliveries().is_empty(),
            "a linked replica is never targeted"
        );
    }
}
