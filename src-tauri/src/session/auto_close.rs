use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tauri::{AppHandle, Emitter, Manager};
use uuid::Uuid;

use crate::config::settings::SettingsState;
use crate::pty::idle_detector::IdleDetector;
use crate::pty::manager::PtyManager;
use crate::session::manager::SessionManager;

const TICK: Duration = Duration::from_secs(60);

/// Spawn the auto-close watcher. Mirrors MailboxPoller/LoopScheduler::start
/// (lib.rs): a tokio task with a `select!` on the shutdown token.
pub fn start(app: AppHandle, shutdown: crate::shutdown::ShutdownSignal) {
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::select! {
                biased;
                _ = shutdown.token().cancelled() => break,
                _ = tokio::time::sleep(TICK) => {
                    tick(&app).await;
                    flush_clocks(&app); // debounced disk flush for the badge store
                }
            }
        }
    });
}

/// Pure decision: team keys whose EVERY member is silent past the timeout.
/// `members` = (session_id, team_key) for agent-owned LIVE-PTY sessions only.
/// `age_of(id)` returns the silence age (None = untracked live session = a
/// spawn race -> treat as NOT silent so we never kill a brand-new session).
/// Unit-testable without Tauri.
fn teams_to_close(
    members: &[(Uuid, String)],
    timeout: Duration,
    age_of: &dyn Fn(Uuid) -> Option<Duration>,
) -> Vec<Uuid> {
    let mut by_team: HashMap<&str, Vec<Uuid>> = HashMap::new();
    for (id, key) in members {
        by_team.entry(key.as_str()).or_default().push(*id);
    }
    let mut to_close = Vec::new();
    for (_team, ids) in by_team {
        if ids.is_empty() {
            continue;
        }
        let all_silent = ids
            .iter()
            .all(|id| age_of(*id).map(|a| a > timeout).unwrap_or(false));
        if all_silent {
            to_close.extend(ids);
        }
    }
    to_close
}

async fn tick(app: &AppHandle) {
    let settings = app.state::<SettingsState>();
    let (enabled, timeout_min) = {
        let s = settings.read().await;
        (
            s.coordinator_auto_close_enabled,
            s.coordinator_auto_close_minutes,
        )
    };
    if !enabled || timeout_min == 0 {
        return;
    }
    let timeout = Duration::from_secs(u64::from(timeout_min) * 60);

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

    let to_close = teams_to_close(&members, timeout, &|id| idle.silence_age(id));

    // #552 auto-closed badge: map each member id -> its team key, and snapshot
    // the live coordinator (fqn, cwd) per team BEFORE destroying (destroy removes
    // the coordinator session record). Used to set the marker on the right row.
    let id_to_team: HashMap<Uuid, String> =
        members.iter().map(|(id, k)| (*id, k.clone())).collect();
    let coord_refs = session_mgr.read().await.coordinator_refs_by_team().await;

    let mut closed_teams: HashSet<String> = HashSet::new();
    for id in to_close {
        // M2 TOCTOU: re-check immediately before each destroy; skip if the
        // session became active in the destroy window (cheap in-memory read).
        if idle.silence_age(id).map(|a| a > timeout).unwrap_or(false) {
            if let Err(e) = crate::commands::session::destroy_session_inner(app, id).await {
                log::warn!("[auto-close] destroy {} failed: {}", &id.to_string()[..8], e);
            } else {
                log::info!(
                    "[auto-close] terminated idle session {}",
                    &id.to_string()[..8]
                );
                if let Some(team) = id_to_team.get(&id) {
                    closed_teams.insert(team.clone());
                }
            }
        } else {
            log::info!(
                "[auto-close] {} became active during destroy window; skipped",
                &id.to_string()[..8]
            );
        }
    }

    // #552 mark each genuinely-closed team's coordinator row "auto-closed" and
    // emit so the sidebar shows the pill live (discovery reload self-heals from
    // the persisted marker). mark_auto_closed is idempotent (emits once). The
    // dirty flag set here is persisted by flush_clocks at the end of this tick.
    if !closed_teams.is_empty() {
        if let Some(clocks) =
            app.try_state::<crate::config::coordinator_clocks::CoordinatorClocksState>()
        {
            let now = chrono::Utc::now();
            for team in &closed_teams {
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

fn flush_clocks(app: &AppHandle) {
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

    fn key(id: Uuid, team: &str) -> (Uuid, String) {
        (id, team.to_string())
    }

    #[test]
    fn all_silent_team_returns_all_members() {
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let members = vec![key(a, "proj:wg-1"), key(b, "proj:wg-1")];
        let timeout = Duration::from_secs(60);
        let out = teams_to_close(&members, timeout, &|_| Some(Duration::from_secs(120)));
        assert_eq!(out.len(), 2);
        assert!(out.contains(&a) && out.contains(&b));
    }

    #[test]
    fn one_fresh_member_protects_the_whole_team() {
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let members = vec![key(a, "proj:wg-1"), key(b, "proj:wg-1")];
        let timeout = Duration::from_secs(60);
        // `a` is stale, `b` is fresh -> the team is NOT closed.
        let out = teams_to_close(&members, timeout, &|id| {
            if id == a {
                Some(Duration::from_secs(120))
            } else {
                Some(Duration::from_secs(1))
            }
        });
        assert!(out.is_empty(), "a single active member must protect the team");
    }

    #[test]
    fn untracked_none_member_protects_the_team() {
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let members = vec![key(a, "proj:wg-1"), key(b, "proj:wg-1")];
        let timeout = Duration::from_secs(60);
        // `b` has no silence age (spawn race) -> treated as NOT silent.
        let out = teams_to_close(&members, timeout, &|id| {
            if id == a {
                Some(Duration::from_secs(120))
            } else {
                None
            }
        });
        assert!(out.is_empty(), "an untracked live member must protect the team");
    }

    #[test]
    fn multiple_teams_are_isolated() {
        let a = Uuid::new_v4(); // team 1, stale
        let b = Uuid::new_v4(); // team 2, stale
        let c = Uuid::new_v4(); // team 2, fresh
        let members = vec![
            key(a, "proj:wg-1"),
            key(b, "proj:wg-2"),
            key(c, "proj:wg-2"),
        ];
        let timeout = Duration::from_secs(60);
        let out = teams_to_close(&members, timeout, &|id| {
            if id == c {
                Some(Duration::from_secs(1))
            } else {
                Some(Duration::from_secs(120))
            }
        });
        // team 1 (only `a`, stale) closes; team 2 protected by fresh `c`.
        assert_eq!(out, vec![a]);
    }

    #[test]
    fn empty_input_returns_empty() {
        let out = teams_to_close(&[], Duration::from_secs(60), &|_| Some(Duration::from_secs(120)));
        assert!(out.is_empty());
    }
}
