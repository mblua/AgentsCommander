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

async fn tick<R: tauri::Runtime>(
    app: &AppHandle<R>,
    last_emitted: &mut HashMap<String, i64>,
) {
    let settings = app.state::<SettingsState>();
    let (enabled, timeout_min, skip_telegram_assigned) = {
        let s = settings.read().await;
        (
            s.coordinator_auto_close_enabled,
            s.coordinator_auto_close_minutes,
            s.coordinator_auto_close_skip_telegram_assigned,
        )
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
    let telegram_protected: HashSet<Uuid> = if skip_telegram_assigned {
        let mgr = { session_mgr.read().await.clone() };
        let mut out = HashSet::new();
        for (id, _) in &members {
            if mgr.session_has_telegram_bot(*id).await {
                out.insert(*id);
            }
        }
        out
    } else {
        HashSet::new()
    };

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
    let team_sil = team_min_silence(&members, &|id| {
        member_post_repaint_silence(idle.alive_age(id), idle.silence_age(id), REPAINT_GRACE)
    });
    {
        let mut g = clocks.lock().unwrap_or_else(|e| e.into_inner());
        for (team, (fqn, _)) in &coord_refs {
            if let Some(sil) = team_sil.get(team) {
                if let Some(c) =
                    DateTime::<Utc>::from_timestamp(now_secs - sil.as_secs() as i64, 0)
                {
                    g.note_activity(fqn, c); // monotonic; dirties only on a real move
                }
            }
        }
    } // guard dropped before any emit

    // (2) BADGE: snapshot, RELEASE, then emit the unified anchor per team (deduped).
    let snap = {
        let g = clocks.lock().unwrap_or_else(|e| e.into_inner());
        g.snapshot()
    };
    for (team, (fqn, cwd)) in &coord_refs {
        let e = snap.get(fqn);
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
            if anchor == i64::MIN { 0 } else { now_secs - anchor }
        ); // Decision D: debug-only, no new IPC field
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
    if !enabled || timeout_min == 0 {
        return;
    }
    let timeout_secs = i64::from(timeout_min) * 60;
    // closeable iff: an ESTABLISHED member is present AND idle past timeout.
    // `snap` already reflects step 1's advance (taken after it) -> reuse, no re-lock.
    let to_close: Vec<Uuid> = members
        .iter()
        .filter(|(id, team)| {
            let is_telegram_protected = session_is_telegram_protected(
                skip_telegram_assigned,
                telegram_protected.contains(id),
            );
            let anchor =
                resolve_member_anchor(coord_refs.get(team), &snap, idle.silence_age(*id), now_secs);
            should_close_member(
                is_telegram_protected,
                established_teams.contains(team),
                anchor,
                now_secs,
                timeout_secs,
            )
        })
        .map(|(id, _)| *id)
        .collect();

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
            fresh_anchor != i64::MIN && (now_secs - fresh_anchor) <= timeout_secs;
        let emitted_recently = idle.silence_age(id).is_some_and(|a| a < REPAINT_GRACE);
        if anchor_fresh || emitted_recently {
            log::info!(
                "[auto-close] {} re-activated during destroy window; skipped",
                &id.to_string()[..8]
            );
            continue;
        }
        if skip_telegram_assigned {
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
    let mut coord_closed_teams: HashSet<String> = HashSet::new();
    for id in outcome
        .destroyed_ids
        .iter()
        .chain(outcome.retained_exited_ids.iter())
        .copied()
    {
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
mod tests {
    use super::*;

    #[derive(Default)]
    struct AutoCloseBackend {
        live: Mutex<HashSet<Uuid>>,
    }

    impl AutoCloseBackend {
        fn set_live(&self, id: Uuid) {
            self.live.lock().unwrap().insert(id);
        }
    }

    impl crate::pty::backend::PtyBackend for AutoCloseBackend {
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }

        fn spawn(
            &self,
            spec: crate::pty::backend::BackendSpawnSpec,
        ) -> futures::future::BoxFuture<'_, Result<(), crate::errors::AppError>> {
            Box::pin(async move {
                self.set_live(spec.id);
                Ok(())
            })
        }

        fn write(&self, id: Uuid, _data: &[u8]) -> Result<(), crate::errors::AppError> {
            self.live
                .lock()
                .unwrap()
                .contains(&id)
                .then_some(())
                .ok_or_else(|| crate::errors::AppError::SessionNotFound(id.to_string()))
        }

        fn resize(
            &self,
            id: Uuid,
            _cols: u16,
            _rows: u16,
        ) -> Result<(), crate::errors::AppError> {
            self.write(id, &[])
        }

        fn kill(&self, id: Uuid) -> Result<(), crate::errors::AppError> {
            self.live.lock().unwrap().remove(&id);
            Ok(())
        }

        fn has_session(&self, id: Uuid) -> bool {
            self.live.lock().unwrap().contains(&id)
        }

        fn get_screen_snapshot(
            &self,
            _id: Uuid,
        ) -> Option<crate::pty::output::PtyScreenSnapshot> {
            None
        }

        fn get_pty_size(&self, id: Uuid) -> Option<(u16, u16)> {
            self.has_session(id).then_some((30, 120))
        }

        fn register_response_watcher(
            &self,
            _session_id: Uuid,
            _request_id: String,
            _response_dir: std::path::PathBuf,
        ) {
        }

        fn terminate_job_for_session(&self, _id: Uuid) -> bool {
            false
        }

        fn kill_all_jobs(&self) -> (usize, usize) {
            (0, 0)
        }
    }

    fn key(id: Uuid, team: &str) -> (Uuid, String) {
        (id, team.to_string())
    }

    fn ts(secs: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(1_700_000_000 + secs, 0).expect("valid timestamp")
    }

    // ---- member_post_repaint_silence (the E1 repaint-exclusion fix) ----

    #[test]
    fn post_repaint_none_when_untracked() {
        let g = Duration::from_secs(10);
        assert_eq!(
            member_post_repaint_silence(None, Some(Duration::from_secs(5)), g),
            None,
            "missing alive_age (spawn race) -> excluded"
        );
        assert_eq!(
            member_post_repaint_silence(Some(Duration::from_secs(60)), None, g),
            None,
            "missing silence_age (spawn race) -> excluded"
        );
    }

    #[test]
    fn post_repaint_none_when_inside_wake_window() {
        let g = Duration::from_secs(10);
        // alive < grace -> checked_sub None -> excluded.
        assert_eq!(
            member_post_repaint_silence(
                Some(Duration::from_secs(5)),
                Some(Duration::from_secs(0)),
                g
            ),
            None
        );
        // alive == grace -> window 0 -> silence(0) < 0 is false -> excluded.
        assert_eq!(
            member_post_repaint_silence(
                Some(Duration::from_secs(10)),
                Some(Duration::from_secs(0)),
                g
            ),
            None
        );
    }

    #[test]
    fn post_repaint_none_when_last_output_was_repaint_replay() {
        // Case A (abandoned): alive 60, repaint burst ended at t=2 -> silence 58.
        // window = 50; 58 >= 50 -> excluded, the persisted anchor stays frozen.
        let g = Duration::from_secs(10);
        assert_eq!(
            member_post_repaint_silence(
                Some(Duration::from_secs(60)),
                Some(Duration::from_secs(58)),
                g
            ),
            None,
            "scrollback-only output must NOT advance the anchor"
        );
    }

    #[test]
    fn post_repaint_some_when_genuine_post_repaint_output() {
        // Case B (genuine resume): alive 60, real work at ~t=25 -> silence 35.
        // window = 50; 35 < 50 -> contributes Some(35).
        let g = Duration::from_secs(10);
        assert_eq!(
            member_post_repaint_silence(
                Some(Duration::from_secs(60)),
                Some(Duration::from_secs(35)),
                g
            ),
            Some(Duration::from_secs(35)),
            "genuine post-repaint work must advance the anchor"
        );
    }

    #[test]
    fn post_repaint_boundary_is_excluded() {
        // silence == alive - grace -> NOT strictly less -> excluded (strict <).
        let g = Duration::from_secs(10);
        assert_eq!(
            member_post_repaint_silence(
                Some(Duration::from_secs(60)),
                Some(Duration::from_secs(50)),
                g
            ),
            None,
            "boundary silence == alive - grace is excluded"
        );
    }

    // ---- team_min_silence (folds the per-member post-repaint filter) ----

    #[test]
    fn team_min_silence_takes_min_over_contributing_members() {
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let members = vec![key(a, "proj:wg-1"), key(b, "proj:wg-1")];
        let out = team_min_silence(&members, &|id| {
            if id == a {
                Some(Duration::from_secs(30))
            } else {
                Some(Duration::from_secs(10))
            }
        });
        assert_eq!(out.get("proj:wg-1"), Some(&Duration::from_secs(10)));
    }

    #[test]
    fn team_min_silence_absent_when_no_member_contributes() {
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let members = vec![key(a, "proj:wg-1"), key(b, "proj:wg-1")];
        // All members repaint-only / sub-grace / untracked -> None for each.
        let out = team_min_silence(&members, &|_| None);
        assert!(
            !out.contains_key("proj:wg-1"),
            "a team with no contributing member must be ABSENT (anchor frozen)"
        );
        assert!(out.is_empty());
    }

    #[test]
    fn team_min_silence_isolates_teams() {
        let a = Uuid::new_v4(); // team 1, contributes 40
        let b = Uuid::new_v4(); // team 2, contributes 5
        let c = Uuid::new_v4(); // team 2, untracked -> excluded
        let members = vec![
            key(a, "proj:wg-1"),
            key(b, "proj:wg-2"),
            key(c, "proj:wg-2"),
        ];
        let out = team_min_silence(&members, &|id| {
            if id == a {
                Some(Duration::from_secs(40))
            } else if id == b {
                Some(Duration::from_secs(5))
            } else {
                None
            }
        });
        assert_eq!(out.get("proj:wg-1"), Some(&Duration::from_secs(40)));
        assert_eq!(out.get("proj:wg-2"), Some(&Duration::from_secs(5)));
    }

    // ---- team_idle_since_secs (unified max(user, activity) anchor) ----

    #[test]
    fn idle_since_secs_picks_max_present_component() {
        assert_eq!(team_idle_since_secs(Some(ts(100)), None), ts(100).timestamp());
        assert_eq!(team_idle_since_secs(None, Some(ts(200))), ts(200).timestamp());
        assert_eq!(
            team_idle_since_secs(Some(ts(100)), Some(ts(200))),
            ts(200).timestamp(),
            "max of the two present components"
        );
        assert_eq!(
            team_idle_since_secs(None, None),
            i64::MIN,
            "both absent -> MIN (no badge)"
        );
    }

    // ---- team_is_closeable (the KILL predicate; replaces the teams_to_close tests) ----

    #[test]
    fn closeable_when_established_and_idle_past_timeout() {
        let now = ts(10_000).timestamp();
        let timeout = 3600; // 60 min
        // 3700s idle > 3600s timeout.
        let anchor = team_idle_since_secs(Some(ts(10_000 - 3700)), None);
        assert!(team_is_closeable(true, anchor, now, timeout));
    }

    #[test]
    fn not_closeable_without_established_member() {
        // wake grace: no member alive past WAKE_GRACE -> not established -> never closes.
        let now = ts(10_000).timestamp();
        let anchor = team_idle_since_secs(Some(ts(10_000 - 3700)), None);
        assert!(
            !team_is_closeable(false, anchor, now, 3600),
            "no established member must protect the team (wake grace)"
        );
    }

    #[test]
    fn not_closeable_when_user_message_keeps_anchor_fresh() {
        let now = ts(10_000).timestamp();
        // Recent user message (60s ago) keeps the anchor fresh even though activity is old.
        let anchor = team_idle_since_secs(Some(ts(10_000 - 60)), Some(ts(10_000 - 9000)));
        assert!(
            !team_is_closeable(true, anchor, now, 3600),
            "a recent last_user_message_at must keep the team open"
        );
    }

    #[test]
    fn not_closeable_when_activity_recent() {
        let now = ts(10_000).timestamp();
        // Recent activity (120s ago) keeps the anchor fresh even though the user message is old.
        let anchor = team_idle_since_secs(Some(ts(10_000 - 9000)), Some(ts(10_000 - 120)));
        assert!(
            !team_is_closeable(true, anchor, now, 3600),
            "a recent last_activity_at must keep the team open"
        );
    }

    #[test]
    fn not_closeable_when_anchor_absent() {
        let now = ts(10_000).timestamp();
        assert!(
            !team_is_closeable(true, i64::MIN, now, 3600),
            "a team with no anchor (i64::MIN) is never closeable"
        );
    }

    #[test]
    fn telegram_protection_is_opt_in() {
        assert!(!session_is_telegram_protected(false, true));
        assert!(!session_is_telegram_protected(false, false));
    }

    #[test]
    fn telegram_protection_skips_only_assigned_sessions_when_enabled() {
        assert!(session_is_telegram_protected(true, true));
        assert!(!session_is_telegram_protected(true, false));
    }

    #[test]
    fn protected_member_not_selected_when_established_and_idle_past_timeout() {
        let now = ts(10_000).timestamp();
        let timeout = 3600;
        let anchor = team_idle_since_secs(Some(ts(10_000 - 3700)), None);

        assert!(!should_close_member(true, true, anchor, now, timeout));
    }

    #[test]
    fn unprotected_member_selected_when_established_and_idle_past_timeout() {
        let now = ts(10_000).timestamp();
        let timeout = 3600;
        let anchor = team_idle_since_secs(Some(ts(10_000 - 3700)), None);

        assert!(should_close_member(false, true, anchor, now, timeout));
    }

    // ---- orphan_anchor_secs (#582 orphan fallback anchor) ----

    #[test]
    fn orphan_anchor_uses_member_own_silence_and_becomes_closeable() {
        let now = ts(10_000).timestamp();
        let anchor = orphan_anchor_secs(Some(Duration::from_secs(3700)), now);
        assert_eq!(anchor, now - 3700);
        assert!(
            team_is_closeable(true, anchor, now, 3600),
            "an orphan silent past the timeout must become closeable"
        );
    }

    #[test]
    fn orphan_anchor_fresh_silence_keeps_member_open() {
        let now = ts(10_000).timestamp();
        let anchor = orphan_anchor_secs(Some(Duration::from_secs(30)), now);
        assert!(
            !team_is_closeable(true, anchor, now, 3600),
            "a recently-active orphan must keep its full-timeout lease"
        );
    }

    #[test]
    fn orphan_anchor_untracked_is_min_and_not_closeable() {
        let now = ts(10_000).timestamp();
        assert_eq!(orphan_anchor_secs(None, now), i64::MIN);
        assert!(!team_is_closeable(true, i64::MIN, now, 3600));
    }

    // ---- resolve_member_anchor (#582 two-case anchor; HIGH-1 regression guard) ----

    #[test]
    fn resolve_anchor_orphan_uses_member_silence() {
        let now = ts(10_000).timestamp();
        let snap: std::collections::HashMap<String, ClockEntry> = std::collections::HashMap::new();
        // No coordinator record -> genuine orphan -> own silence.
        let anchor = resolve_member_anchor(None, &snap, Some(Duration::from_secs(3700)), now);
        assert_eq!(anchor, now - 3700);
        assert!(team_is_closeable(true, anchor, now, 3600));
    }

    #[test]
    fn resolve_anchor_live_coordinator_empty_snapshot_stays_min() {
        // HIGH-1 GUARD: a LIVE coordinator (record present) whose FQN is NOT in the
        // snapshot (e.g. a team restored after restart, FQN not yet in the clock
        // file) must stay i64::MIN / not-closeable -- it must NOT fall back to the
        // member's raw silence. This is the normal path, unchanged from before #582.
        let now = ts(10_000).timestamp();
        let snap: std::collections::HashMap<String, ClockEntry> = std::collections::HashMap::new();
        let coord = ("proj:wg-1/tech-lead".to_string(), "C:/x".to_string());
        let anchor = resolve_member_anchor(
            Some(&coord),
            &snap,
            Some(Duration::from_secs(99_999)), // very idle member -- MUST be ignored
            now,
        );
        assert_eq!(
            anchor,
            i64::MIN,
            "live coordinator + empty snapshot must NOT use member silence"
        );
        assert!(
            !team_is_closeable(true, anchor, now, 3600),
            "an unclocked LIVE coordinator team must stay open (normal path)"
        );
    }

    #[test]
    fn resolve_anchor_live_coordinator_uses_team_clock() {
        let now = ts(10_000).timestamp();
        let fqn = "proj:wg-1/tech-lead".to_string();
        let coord = (fqn.clone(), "C:/x".to_string());
        let mut snap: std::collections::HashMap<String, ClockEntry> = std::collections::HashMap::new();
        snap.insert(
            fqn,
            ClockEntry {
                last_user_message_at: Some(ts(10_000 - 3700)),
                last_activity_at: None,
                auto_closed_at: None,
                // #588 added this required field to ClockEntry; the orphan-anchor
                // path (#582) ignores it, so None keeps this test's intent intact.
                manually_closed_at: None,
                // (#756) same reasoning: the anchor path ignores the fresh intent.
                start_fresh_at: None,
            },
        );
        // Normal path: anchor from the persisted team clock; member silence ignored.
        let anchor = resolve_member_anchor(Some(&coord), &snap, Some(Duration::from_secs(1)), now);
        assert_eq!(anchor, ts(10_000 - 3700).timestamp());
        assert!(team_is_closeable(true, anchor, now, 3600));
    }

    // ---- destroyed_is_team_coordinator (#589 gated AUTO-CLOSED mark) ----

    #[test]
    fn mark_gate_true_only_when_destroyed_is_the_coordinator() {
        let coord = Uuid::new_v4();
        let member = Uuid::new_v4();

        // The destroyed session IS the team's coordinator -> the row may be marked.
        assert!(
            destroyed_is_team_coordinator(coord, Some(coord)),
            "destroying the coordinator's own session marks the row"
        );

        // A sibling member was reaped while the coordinator (coord) survives ->
        // the live coordinator row must NOT be stamped. This is the #589 bug.
        assert!(
            !destroyed_is_team_coordinator(member, Some(coord)),
            "a surviving coordinator must NOT be stamped when only a member is reaped"
        );

        // No coordinator record for the team -> never a coordinator close.
        assert!(
            !destroyed_is_team_coordinator(member, None),
            "absent coordinator id must not mark the row"
        );
    }

    #[tokio::test]
    async fn selected_idle_team_closes_as_one_batch_and_clears_selection_once() {
        use crate::config::coordinator_clocks::CoordinatorClocks;
        use crate::config::settings::{AppSettings, SettingsState};
        use crate::pty::backend::{PtyBackend, SessionBackendKind};
        use crate::resource_monitor::ResourceMonitorState;
        use crate::session::selection::{SelectionCoordinator, SelectionMode, SelectionSource};
        use crate::telegram::manager::{TelegramBridgeManager, TelegramBridgeState};
        use tauri::Listener;

        const COORDINATOR_CWD: &str =
            "C:\\repos\\myproj\\.ac\\wg-1-team\\__agent_lead";
        const MEMBER_CWD: &str = "C:\\repos\\myproj\\.ac\\wg-1-team\\__agent_rust";

        let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let coordinator_session = manager
            .read()
            .await
            .create_session(
                "shell".to_string(),
                Vec::new(),
                COORDINATOR_CWD.to_string(),
                None,
                None,
                Vec::new(),
                true,
                SessionBackendKind::LocalProcess,
            )
            .await
            .unwrap();
        let member_session = manager
            .read()
            .await
            .create_session(
                "shell".to_string(),
                Vec::new(),
                MEMBER_CWD.to_string(),
                Some("codex".to_string()),
                None,
                Vec::new(),
                false,
                SessionBackendKind::LocalProcess,
            )
            .await
            .unwrap();

        let backend = Arc::new(AutoCloseBackend::default());
        let pty = Arc::new(Mutex::new(PtyManager::new_for_test(backend.clone())));
        for id in [coordinator_session.id, member_session.id] {
            backend.set_live(id);
            pty.lock()
                .unwrap()
                .record_route(id, SessionBackendKind::LocalProcess);
        }
        let idle = IdleDetector::new(|_| {}, |_| {});
        for id in [coordinator_session.id, member_session.id] {
            idle.set_auto_close_ages_for_test(
                id,
                Duration::from_secs(120),
                Duration::from_secs(120),
            );
        }

        let settings = AppSettings {
            coordinator_auto_close_enabled: true,
            coordinator_auto_close_minutes: 1,
            coordinator_auto_close_skip_telegram_assigned: false,
            ..AppSettings::default()
        };
        let settings: SettingsState = Arc::new(tokio::sync::RwLock::new(settings));
        let clocks = Arc::new(Mutex::new(CoordinatorClocks::default()));
        clocks.lock().unwrap().note_user_message(
            "myproj:wg-1-team/lead",
            Utc::now() - chrono::Duration::seconds(120),
        );
        let shutdown = crate::shutdown::ShutdownSignal::new();
        let coordinator = SelectionCoordinator::new(Arc::clone(&manager), shutdown.token().clone());
        let output_senders = Arc::new(Mutex::new(HashMap::new()));
        let telegram: TelegramBridgeState = Arc::new(tokio::sync::Mutex::new(
            TelegramBridgeManager::new(output_senders),
        ));
        let app = tauri::test::mock_builder()
            .manage(settings)
            .manage(Arc::clone(&manager))
            .manage(Arc::clone(&pty))
            .manage(Arc::clone(&idle))
            .manage(clocks)
            .manage(crate::DetachedSessionsState::default())
            .manage(telegram)
            .manage(Arc::new(ResourceMonitorState::new()))
            .manage(coordinator.clone())
            .manage(shutdown)
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("build auto-close test app");
        coordinator
            .start(app.handle().clone())
            .expect("start selection coordinator");
        coordinator.submit_restore_first().await.unwrap().finish();

        let (events_tx, events_rx) = std::sync::mpsc::channel();
        for event_name in ["session_destroyed", "session_switched"] {
            let events_tx = events_tx.clone();
            app.listen_any(event_name, move |event| {
                let _ = events_tx.send((event_name, event.payload().to_string()));
            });
        }

        tick(app.handle(), &mut HashMap::new()).await;

        assert!(manager.read().await.list_sessions().await.is_empty());
        assert!(!backend.has_session(coordinator_session.id));
        assert!(!backend.has_session(member_session.id));
        let selection = manager.read().await.selection_payload().await;
        assert_eq!(selection.mode(), SelectionMode::None);
        assert_eq!(selection.source(), SelectionSource::AutoClose);
        let observed = (0..3)
            .map(|_| events_rx.recv_timeout(Duration::from_secs(1)).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            observed.iter().map(|(name, _)| *name).collect::<Vec<_>>(),
            vec!["session_destroyed", "session_destroyed", "session_switched"]
        );
        let selection_payload: serde_json::Value =
            serde_json::from_str(&observed[2].1).unwrap();
        assert!(selection_payload["id"].is_null());
        assert_eq!(selection_payload["source"], "autoClose");
        assert!(events_rx.try_recv().is_err());
        coordinator.close_and_join().await;
    }
}
