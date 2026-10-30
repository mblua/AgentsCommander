use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{DateTime, Utc};
use tauri::{AppHandle, Emitter, Manager};
use uuid::Uuid;

use crate::config::coordinator_clocks::{ClockEntry, CoordinatorClocksState};
use crate::config::settings::SettingsState;
use crate::pty::idle_detector::IdleDetector;
use crate::pty::manager::PtyManager;
use crate::session::manager::SessionManager;
use crate::session::selection::{SelectionCoordinator, SelectionCoordinatorError};

const TICK: Duration = Duration::from_secs(60);
/// (#580) A just-woken session replays scrollback for ~1-2s; exclude its first
/// REPAINT_GRACE of life from the idle-anchor advance so reopening an abandoned
/// team cannot reset its persisted idle. Feeds `member_post_repaint_silence`.
const REPAINT_GRACE: Duration = Duration::from_secs(10);
/// (#580) A session is not kill-eligible until alive past WAKE_GRACE, so a
/// just-woken (or wake-on-restore) team is visible >=30s before any auto-close.
/// MUST be > REPAINT_GRACE (a member protects its team via the anchor before it
/// can be killed) and is also the spawn-race guard (untracked -> not established).
const WAKE_GRACE: Duration = Duration::from_secs(30);

/// Spawn the auto-close watcher. Mirrors MailboxPoller/LoopScheduler::start
/// (lib.rs): a tokio task with a `select!` on the shutdown token.
pub fn start(app: AppHandle, shutdown: crate::shutdown::ShutdownSignal) {
    tauri::async_runtime::spawn(async move {
        // (#580) per-team last-emitted anchor (seconds) for the badge dedup;
        // task-local, self-bounded via `retain` in `tick`.
        let mut last_emitted: HashMap<String, i64> = HashMap::new();
        loop {
            tokio::select! {
                biased;
                _ = shutdown.token().cancelled() => break,
                _ = tokio::time::sleep(TICK) => {
                    tick(&app, &mut last_emitted).await;
                    flush_clocks(&app); // debounced disk flush for the badge store
                }
            }
        }
    });
}

/// (#580) Effective "post-repaint" silence for ONE live member, or None if the
/// member must NOT advance the idle anchor this tick. None when:
///   - untracked (alive_age or silence_age missing) -> spawn race, ignore;
///   - alive <= REPAINT_GRACE (still inside the wake-repaint window);
///   - the member's LAST output landed inside its repaint window
///     (silence_age >= alive_age - REPAINT_GRACE) -> that output was scrollback
///     replay (or nothing genuine has happened since), so it must not reset the
///     persisted idle of an abandoned-then-reopened team.
///
/// Some(silence_age) ONLY when the last output is genuine post-repaint activity.
/// Pure; this is the E1 fix and gets its own boundary-tested unit (§8.1).
fn member_post_repaint_silence(
    alive_age: Option<Duration>,
    silence_age: Option<Duration>,
    repaint_grace: Duration,
) -> Option<Duration> {
    let alive = alive_age?;
    let silence = silence_age?;
    let window = alive.checked_sub(repaint_grace)?; // None if alive <= grace
    (silence < window).then_some(silence)
}

/// (#580) Per-team MIN post-repaint silence across the members, folding the
/// per-member filter above. A member that does not contribute (untracked, inside
/// its repaint window, or last output was scrollback replay) is SKIPPED; a team
/// with NO contributing member is ABSENT from the map, so the caller leaves its
/// persisted anchor FROZEN (no advance). Pure. Spawn-race safety lives entirely
/// in the `established` KILL gate, not here.
fn team_min_silence(
    members: &[(Uuid, String)],
    post_repaint_of: &dyn Fn(Uuid) -> Option<Duration>,
) -> HashMap<String, Duration> {
    let mut out: HashMap<String, Duration> = HashMap::new();
    for (id, key) in members {
        if let Some(age) = post_repaint_of(*id) {
            out.entry(key.clone())
                .and_modify(|m| {
                    if age < *m {
                        *m = age;
                    }
                })
                .or_insert(age);
        }
    }
    out
}

/// (#580) Unified idle anchor as whole UNIX seconds: max(user-message, activity).
/// i64::MIN for an absent component so the present one wins; both absent -> MIN
/// (caller treats as "no badge"). Pure.
fn team_idle_since_secs(
    last_user_message_at: Option<DateTime<Utc>>,
    last_activity_at: Option<DateTime<Utc>>,
) -> i64 {
    let u = last_user_message_at.map_or(i64::MIN, |t| t.timestamp());
    let a = last_activity_at.map_or(i64::MIN, |t| t.timestamp());
    u.max(a)
}

/// (#582) Idle anchor for a GENUINELY ORPHANED member: a live agent member whose
/// team has NO live coordinator record (its coordinator was destroyed, so there is
/// no persisted clock to key by the coordinator FQN). Falls back to the member's
/// own in-memory silence (`now - silence_age`) so the orphan stays
/// auto-close-eligible and is reaped on a later idle tick instead of reading
/// `i64::MIN` and lingering until a manual close. `None` silence_age (a
/// just-spawned, untracked member) yields `i64::MIN` (not closeable this tick),
/// matching the spawn-race conservatism elsewhere. Pure; unit-tested.
fn orphan_anchor_secs(silence_age: Option<Duration>, now_secs: i64) -> i64 {
    match silence_age {
        Some(d) => now_secs.saturating_sub(d.as_secs() as i64),
        None => i64::MIN,
    }
}

/// (#582) Resolve the idle anchor for ONE live member, keeping the orphan and
/// normal cases DISTINCT (the #582 HIGH fix). If the team has a LIVE coordinator
/// record, use the normal persisted team anchor from the snapshot; an absent
/// snapshot entry yields `i64::MIN` (not closeable), exactly as before #582 -- the
/// normal path is unchanged, so an unclocked-but-live coordinator (e.g. a team
/// restored after restart whose FQN is not yet in the clock file) is NEVER closed
/// from raw member silence. ONLY a member whose team has NO coordinator record (a
/// genuine orphan) falls back to its own silence via `orphan_anchor_secs`. Mirrors
/// the kill-loop re-check, which already matches on `coord_refs.get(team)` first
/// (`auto_close.rs:265`). Pure; unit-tested for all branches.
fn resolve_member_anchor(
    coord_ref: Option<&(String, String)>,
    snap: &std::collections::HashMap<String, ClockEntry>,
    member_silence: Option<Duration>,
    now_secs: i64,
) -> i64 {
    match coord_ref {
        Some((fqn, _)) => {
            let e = snap.get(fqn);
            team_idle_since_secs(
                e.and_then(|e| e.last_user_message_at),
                e.and_then(|e| e.last_activity_at),
            )
        }
        None => orphan_anchor_secs(member_silence, now_secs),
    }
}

/// (#580) KILL predicate for ONE team: closeable iff it has an ESTABLISHED
/// member (alive past WAKE_GRACE, the visible-window + spawn-race gate) AND its
/// unified anchor is idle past the timeout. `anchor_secs` is the output of
/// `team_idle_since_secs`; i64::MIN (no anchor) is never closeable. Pure, so the
/// kill rule is unit-tested directly (§8.1) instead of re-implemented in a test.
fn team_is_closeable(
    established: bool,
    anchor_secs: i64,
    now_secs: i64,
    timeout_secs: i64,
) -> bool {
    established && anchor_secs != i64::MIN && (now_secs - anchor_secs) > timeout_secs
}

fn session_is_telegram_protected(skip_telegram_assigned: bool, has_telegram: bool) -> bool {
    skip_telegram_assigned && has_telegram
}

fn should_close_member(
    telegram_protected: bool,
    established: bool,
    anchor_secs: i64,
    now_secs: i64,
    timeout_secs: i64,
) -> bool {
    !telegram_protected && team_is_closeable(established, anchor_secs, now_secs, timeout_secs)
}

/// (#589) Should a successful destroy stamp its team's coordinator row
/// AUTO-CLOSED? True IFF the destroyed session IS that team's coordinator
/// (`coord_id_of_team == Some(destroyed_id)`). A reaped sibling member while the
/// coordinator survives (spared by the TOCTOU/repaint guards) returns false, so
/// the LIVE coordinator keeps its idle counter instead of a stale pill. `None`
/// (no coordinator record for the team) is never a coordinator close. Pure; this
/// is the #589 gate and gets its own unit test.
fn destroyed_is_team_coordinator(destroyed_id: Uuid, coord_id_of_team: Option<Uuid>) -> bool {
    coord_id_of_team == Some(destroyed_id)
}

async fn tick<R: tauri::Runtime>(app: &AppHandle<R>, last_emitted: &mut HashMap<String, i64>) {
    let settings = app.state::<SettingsState>();
    let close_settings = match settings.current_read().await {
        Ok(s) => Some((
            s.coordinator_auto_close_enabled,
            s.coordinator_auto_close_minutes,
            s.coordinator_auto_close_skip_telegram_assigned,
        )),
        Err(_) => {
            log::debug!("[AutoClose] settings unavailable; skipping auto-close");
            None
        }
    };
    // NOTE: the enabled/timeout gate moved DOWN: steps 1-2 (advance + badge
    // emit) run even when auto-close is OFF (the badge is informational). Only
    // step 3 (kill) is gated.

    let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
    let pty_mgr = app.state::<Arc<Mutex<PtyManager>>>();
    let idle = app.state::<Arc<IdleDetector>>();

    // Agent-owned sessions + their team key (read lock released here).
    let candidates = session_mgr.read().await.agent_team_members().await;

    // Keep only sessions with a LIVE PTY (M3: deferred/restored members must not
    // veto, and a session with no PTY has nothing to terminate).
    let members: Vec<(Uuid, String)> = {
        let pm = pty_mgr.lock().unwrap_or_else(|e| e.into_inner());
        candidates
            .into_iter()
            .filter(|(id, _)| pm.has_session(*id))
            .collect()
    };
    let telegram_protected = telegram_protected_members(
        &session_mgr,
        &members,
        matches!(close_settings, Some((_, _, true))),
    )
    .await;

    // KILL filter: teams with any member alive past WAKE_GRACE (kill-eligible).
    let established_teams: HashSet<String> = members
        .iter()
        .filter(|(id, _)| idle.alive_age(*id).is_some_and(|a| a >= WAKE_GRACE))
        .map(|(_, k)| k.clone())
        .collect();

    // #552 auto-closed badge: map each member id -> its team key, and snapshot
    // the live coordinator (fqn, cwd) per team BEFORE destroying (destroy removes
    // the coordinator session record). Used to set the marker on the right row.
    let id_to_team: HashMap<Uuid, String> =
        members.iter().map(|(id, k)| (*id, k.clone())).collect();
    let coord_refs = session_mgr.read().await.coordinator_refs_by_team().await;
    // (#589) team -> coordinator session id, over the same coordinator records as
    // coord_refs. Gates the AUTO-CLOSED mark on the coordinator's OWN destruction
    // (not "any member destroyed"): a surviving coordinator whose sibling was
    // reaped keeps its idle counter instead of getting a stale pill. A separate
    // read lock from coord_refs, but a transient disagreement is benign (a team in
    // one map but not the other simply yields no spurious mark).
    let coord_ids = session_mgr.read().await.coordinator_ids_by_team().await;

    let now_secs = Utc::now().timestamp();
    let Some(clocks) = app.try_state::<CoordinatorClocksState>() else {
        return;
    };

    // (1) ADVANCE last_activity_at from POST-REPAINT members (REPAINT_GRACE filter).
    advance_team_activity(&clocks, &members, &idle, &coord_refs, now_secs);

    // (2) BADGE: snapshot, RELEASE, then emit the unified anchor per team (deduped).
    let snap = {
        let g = clocks.lock().unwrap_or_else(|e| e.into_inner());
        g.snapshot()
    };
    for (team, (fqn, cwd)) in &coord_refs {
        let e = snap.get(fqn);
        let anchor = logged_team_idle_anchor(team, e, now_secs);
        if anchor != i64::MIN && last_emitted.get(team).copied() != Some(anchor) {
            last_emitted.insert(team.clone(), anchor);
            if let Some(dt) = DateTime::<Utc>::from_timestamp(anchor, 0) {
                let _ = app.emit(
                    "coordinator_clock_updated",
                    serde_json::json!({ "replicaPath": cwd, "lastUserMessageAt": dt.to_rfc3339() }),
                );
            }
        }
    }
    last_emitted.retain(|team, _| coord_refs.contains_key(team)); // bound the map

    // (3) KILL: gated on the setting; SAME anchor; established + timeout.
    let Some((enabled, timeout_min, skip_telegram_assigned)) = close_settings else {
        return;
    };
    if !enabled || timeout_min == 0 {
        return;
    }
    let timeout_secs = i64::from(timeout_min) * 60;
    // closeable iff: an ESTABLISHED member is present AND idle past timeout.
    // `snap` already reflects step 1's advance (taken after it) -> reuse, no re-lock.
    let policy = AutoClosePolicy {
        now_secs,
        timeout_secs,
        skip_telegram_assigned,
    };
    let to_close = closeable_members(
        &members,
        &coord_refs,
        &snap,
        &idle,
        &established_teams,
        &telegram_protected,
        policy,
    );

    if to_close.is_empty() {
        return;
    }
    let coordinator = app.state::<SelectionCoordinator>();
    let ticket = match coordinator.reserve_auto_close() {
        Ok(ticket) => ticket,
        Err(SelectionCoordinatorError::Busy) => {
            log::info!("[auto-close] coordinator busy; deferred idle batch to next tick");
            return;
        }
        Err(error) => {
            log::warn!("[auto-close] coordinator unavailable: {error}");
            return;
        }
    };

    // (#589) Teams whose COORDINATOR'S OWN session was auto-closed this tick. A
    // team where only a non-coordinator member was reaped is deliberately absent,
    // so the surviving coordinator row is never stamped AUTO-CLOSED.
    let confirmed = confirm_closeable_members(
        to_close,
        &clocks,
        &id_to_team,
        &coord_refs,
        &idle,
        &session_mgr,
        policy,
    )
    .await;

    let outcome = match ticket.finalize(confirmed).await {
        Ok(outcome) => outcome,
        Err(error) => {
            log::warn!("[auto-close] batch finalization failed: {error}");
            return;
        }
    };
    for (id, error) in &outcome.failed {
        log::warn!(
            "[auto-close] destroy {} failed: {}",
            &id.to_string()[..8],
            error
        );
    }
    let coord_closed_teams = closed_coordinator_teams(
        outcome
            .destroyed_ids
            .iter()
            .chain(outcome.retained_exited_ids.iter())
            .copied(),
        &id_to_team,
        &coord_ids,
    );

    // #552 mark each genuinely-closed team's coordinator row "auto-closed" and
    // emit so the sidebar shows the pill live (discovery reload self-heals from
    // the persisted marker). mark_auto_closed is idempotent (emits once). The
    // dirty flag set here is persisted by flush_clocks at the end of this tick.
    if !coord_closed_teams.is_empty() {
        if let Some(clocks) =
            app.try_state::<crate::config::coordinator_clocks::CoordinatorClocksState>()
        {
            let now = chrono::Utc::now();
            for team in &coord_closed_teams {
                // No live coordinator record for this team at close time (already
                // torn down) -> nothing to badge; sessions were still terminated.
                // Rare; documented in plan §7.
                let Some((fqn, cwd)) = coord_refs.get(team) else {
                    continue;
                };
                let newly = {
                    clocks
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .mark_auto_closed(fqn, now)
                };
                if newly {
                    let _ = app.emit(
                        "coordinator_auto_close_changed",
                        serde_json::json!({ "replicaPath": cwd, "autoClosedAt": now.to_rfc3339() }),
                    );
                }
            }
        }
    }
}

fn logged_team_idle_anchor(team: &str, e: Option<&ClockEntry>, now_secs: i64) -> i64 {
    let anchor = team_idle_since_secs(
        e.and_then(|e| e.last_user_message_at),
        e.and_then(|e| e.last_activity_at),
    );
    log::debug!(
        "[auto-close] team={} user={:?} activity={:?} anchor={} idle_s={}",
        team,
        e.and_then(|e| e.last_user_message_at),
        e.and_then(|e| e.last_activity_at),
        anchor,
        if anchor == i64::MIN {
            0
        } else {
            now_secs - anchor
        }
    ); // Decision D: debug-only, no new IPC field
    anchor
}

async fn telegram_protected_members(
    session_mgr: &tokio::sync::RwLock<SessionManager>,
    members: &[(Uuid, String)],
    skip_telegram_assigned: bool,
) -> HashSet<Uuid> {
    if skip_telegram_assigned {
        let mgr = { session_mgr.read().await.clone() };
        let mut out = HashSet::new();
        for (id, _) in members {
            if mgr.session_has_telegram_bot(*id).await {
                out.insert(*id);
            }
        }
        out
    } else {
        HashSet::new()
    }
}

fn advance_team_activity(
    clocks: &CoordinatorClocksState,
    members: &[(Uuid, String)],
    idle: &IdleDetector,
    coord_refs: &HashMap<String, (String, String)>,
    now_secs: i64,
) {
    let team_sil = team_min_silence(members, &|id| {
        member_post_repaint_silence(idle.alive_age(id), idle.silence_age(id), REPAINT_GRACE)
    });
    {
        let mut g = clocks.lock().unwrap_or_else(|e| e.into_inner());
        for (team, (fqn, _)) in coord_refs {
            if let Some(sil) = team_sil.get(team) {
                if let Some(c) = DateTime::<Utc>::from_timestamp(now_secs - sil.as_secs() as i64, 0)
                {
                    g.note_activity(fqn, c); // monotonic; dirties only on a real move
                }
            }
        }
    } // guard dropped before any emit
}

fn closed_coordinator_teams(
    ids: impl Iterator<Item = Uuid>,
    id_to_team: &HashMap<Uuid, String>,
    coord_ids: &HashMap<String, Uuid>,
) -> HashSet<String> {
    let mut coord_closed_teams = HashSet::new();
    for id in ids {
        log::info!(
            "[auto-close] terminated idle session {}",
            &id.to_string()[..8]
        );
        // (#589) Record the team for the AUTO-CLOSED mark ONLY when the
        // destroyed session IS this team's coordinator. A sibling member
        // reaped while the coordinator survives must NOT stamp the live
        // coordinator row; it keeps its idle counter.
        if let Some(team) = id_to_team.get(&id) {
            if destroyed_is_team_coordinator(id, coord_ids.get(team).copied()) {
                coord_closed_teams.insert(team.clone());
            }
        }
    }
    coord_closed_teams
}

#[derive(Clone, Copy)]
struct AutoClosePolicy {
    now_secs: i64,
    timeout_secs: i64,
    skip_telegram_assigned: bool,
}

fn closeable_members(
    members: &[(Uuid, String)],
    coord_refs: &HashMap<String, (String, String)>,
    snap: &HashMap<String, ClockEntry>,
    idle: &IdleDetector,
    established_teams: &HashSet<String>,
    telegram_protected: &HashSet<Uuid>,
    policy: AutoClosePolicy,
) -> Vec<Uuid> {
    members
        .iter()
        .filter(|(id, team)| {
            let is_telegram_protected = session_is_telegram_protected(
                policy.skip_telegram_assigned,
                telegram_protected.contains(id),
            );
            let anchor = resolve_member_anchor(
                coord_refs.get(team),
                snap,
                idle.silence_age(*id),
                policy.now_secs,
            );
            should_close_member(
                is_telegram_protected,
                established_teams.contains(team),
                anchor,
                policy.now_secs,
                policy.timeout_secs,
            )
        })
        .map(|(id, _)| *id)
        .collect()
}

async fn confirm_closeable_members(
    to_close: Vec<Uuid>,
    clocks: &CoordinatorClocksState,
    id_to_team: &HashMap<Uuid, String>,
    coord_refs: &HashMap<String, (String, String)>,
    idle: &IdleDetector,
    session_mgr: &tokio::sync::RwLock<SessionManager>,
    policy: AutoClosePolicy,
) -> Vec<Uuid> {
    let mut confirmed = Vec::new();
    for id in to_close {
        // TOCTOU re-check (G2): skip if a user message advanced the anchor since
        // the snapshot, OR this member emitted within REPAINT_GRACE. The second
        // clause is REQUIRED: a user message to a NON-coordinator member or a
        // member's first genuine byte only touches the in-memory silence clock,
        // never last_user_message_at, so the anchor re-read alone is blind to it.
        let fresh_anchor = {
            let g = clocks.lock().unwrap_or_else(|e| e.into_inner());
            match id_to_team.get(&id).and_then(|t| coord_refs.get(t)) {
                Some((fqn, _)) => {
                    team_idle_since_secs(g.last_user_message_at(fqn), g.last_activity_at(fqn))
                }
                None => i64::MIN,
            }
        }; // guard dropped before the destroy await
        let anchor_fresh =
            fresh_anchor != i64::MIN && (policy.now_secs - fresh_anchor) <= policy.timeout_secs;
        let emitted_recently = idle.silence_age(id).is_some_and(|a| a < REPAINT_GRACE);
        if anchor_fresh || emitted_recently {
            log::info!(
                "[auto-close] {} re-activated during destroy window; skipped",
                &id.to_string()[..8]
            );
            continue;
        }
        if policy.skip_telegram_assigned {
            let mgr = { session_mgr.read().await.clone() };
            if session_is_telegram_protected(true, mgr.session_has_telegram_bot(id).await) {
                log::info!(
                    "[auto-close] {} has Telegram assigned; skipped",
                    &id.to_string()[..8]
                );
                continue;
            }
        }
        confirmed.push(id);
    }
    confirmed
}

fn flush_clocks<R: tauri::Runtime>(app: &AppHandle<R>) {
    if let Some(clocks) =
        app.try_state::<crate::config::coordinator_clocks::CoordinatorClocksState>()
    {
        // Snapshot under the lock, RELEASE, then do disk I/O (the same mutex is
        // on the keystroke path; never hold it across the ~260ms rename).
        let snapshot = {
            let mut guard = clocks.lock().unwrap_or_else(|e| e.into_inner());
            if !guard.take_dirty() {
                return;
            }
            guard.snapshot()
        };
        if let Err(e) = crate::config::coordinator_clocks::save_map(&snapshot) {
            log::warn!("[coordinator-clocks] save failed: {}", e);
        }
    }
}

#[cfg(test)]
mod tests;
