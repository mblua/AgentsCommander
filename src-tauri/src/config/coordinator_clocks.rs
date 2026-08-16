use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Minimum gap between two recorded user-message timestamps for the same
/// coordinator. Keystroke bursts inside this window are coalesced to one
/// update so we do not emit/persist per keystroke.
const COALESCE_SECS: i64 = 10;

/// (#621 MED-2) Monotonic suffix so concurrent `save_map_to` calls in one process
/// (auto-close tick + off-tick workgroup-remove saves) never share a temp path.
static SAVE_TMP_SEQ: AtomicU64 = AtomicU64::new(0);

/// Persisted per-coordinator state, keyed by the coordinator FQN
/// (`<project>:<wg>/<agent>`). One entry holds the persisted facts behind the
/// unified idle counter (#580): the user-message clock (`last_user_message_at`),
/// the last-real-activity clock (`last_activity_at`), and the auto-closed marker
/// (`auto_closed_at`). All are wall-clock, persisted, FQN-keyed, and survive
/// restart, so they share one map / one file (one source of truth). The badge and
/// auto-close both read the unified anchor
/// `team_idle_since = max(last_user_message_at, last_activity_at)`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClockEntry {
    /// Badge clock: time of the user's last message to this coordinator.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_user_message_at: Option<DateTime<Utc>>,
    /// (#580) Wall-clock of the last REAL team activity (any member or the
    /// coordinator), advanced by the auto-close evaluator from the in-memory
    /// silence clock. With `last_user_message_at` it forms the unified idle
    /// anchor `team_idle_since = max(last_user_message_at, last_activity_at)`,
    /// which the badge displays and auto-close triggers on. Persisted so the
    /// counter survives restart (closed time counts).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_activity_at: Option<DateTime<Utc>>,
    /// Auto-closed marker: set by the auto-close task when this coordinator's
    /// team was terminated for inactivity; cleared on reopen. `Some` => show
    /// the "auto-closed" pill. Only the task sets it, which is what
    /// distinguishes inactivity auto-close from never-started / manual close /
    /// error exit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_closed_at: Option<DateTime<Utc>>,
    /// #588 Manual-close marker: set when this coordinator was closed manually
    /// by the user; cleared on reopen. `Some` => show the "manually-closed" pill.
    /// Mutually exclusive with `auto_closed_at` (hardened in `mark_*`), but the
    /// renderer treats manual as the winner if both were ever set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manually_closed_at: Option<DateTime<Utc>>,
    /// (#756) Durable fresh-conversation intent, mirrored from the session
    /// record's `start_fresh_on_restore` for coordinators. `Some` means: the
    /// newest provider transcript for this cwd predates the user's last
    /// fresh-boundary action (restart or successful logical clear: /clear or Pi
    /// /new) and no post-boundary content exists yet, so the create path must
    /// suppress provider resume
    /// injection (Claude/Pi --continue / Codex resume --last / Gemini --resume
    /// latest). Survives record destruction (idle auto-close, manual close).
    /// Cleared by typed user input and by AC-injected message content; NOT
    /// cleared when honored at create (repeat reopens must stay fresh).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_fresh_at: Option<DateTime<Utc>>,
}

impl ClockEntry {
    /// (#580) The unified team-idle anchor for this coordinator: the newer of the
    /// user-message and last-activity clocks (`max`), or None if neither is set.
    /// The badge displays and auto-close triggers on this value. Single source for
    /// the max rule; mirrors `auto_close::team_idle_since_secs`, which folds the
    /// same two components as i64 seconds on the evaluator's hot path.
    pub fn idle_anchor(&self) -> Option<DateTime<Utc>> {
        [self.last_user_message_at, self.last_activity_at]
            .into_iter()
            .flatten()
            .max()
    }
}

/// Persisted per-coordinator clocks store. ON DISK this is the flat map
/// `{ "proj:wg/agent": { "lastUserMessageAt": ..., "autoClosedAt": ... } }`
/// (see save/load). `save_map`/`load` stay symmetric (the B1/H1 fix); only the
/// value type grew from a bare timestamp to `ClockEntry`.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct CoordinatorClocks {
    #[serde(default)]
    map: HashMap<String, ClockEntry>,
    #[serde(skip)]
    dirty: bool,
}

impl CoordinatorClocks {
    /// Record a user message for `fqn` at `now`. Coalesces: returns `true`
    /// (caller should emit/flag dirty) only if no value existed or the prior
    /// value is older than COALESCE_SECS. Returns `false` when skipped.
    pub fn note_user_message(&mut self, fqn: &str, now: DateTime<Utc>) -> bool {
        let entry = self.map.entry(fqn.to_string()).or_default();
        if let Some(prev) = entry.last_user_message_at {
            if (now - prev).num_seconds() < COALESCE_SECS {
                return false;
            }
        }
        entry.last_user_message_at = Some(now); // preserves auto_closed_at
        self.dirty = true;
        true
    }

    /// Seed `fqn` at `now` only if absent (coordinator first spawn). Returns
    /// `true` if a new entry was created. A respawn of an existing coordinator
    /// must NOT reset the clock, so this never overwrites.
    pub fn seed_if_absent(&mut self, fqn: &str, now: DateTime<Utc>) -> bool {
        if self.map.contains_key(fqn) {
            return false;
        }
        self.map.insert(
            fqn.to_string(),
            ClockEntry {
                last_user_message_at: Some(now),
                last_activity_at: None,
                auto_closed_at: None,
                manually_closed_at: None,
                start_fresh_at: None,
            },
        );
        self.dirty = true;
        true
    }

    /// Badge clock accessor.
    pub fn last_user_message_at(&self, fqn: &str) -> Option<DateTime<Utc>> {
        self.map.get(fqn).and_then(|e| e.last_user_message_at)
    }

    /// (#580) Last-real-activity clock accessor.
    pub fn last_activity_at(&self, fqn: &str) -> Option<DateTime<Utc>> {
        self.map.get(fqn).and_then(|e| e.last_activity_at)
    }

    /// (#580) Advance `last_activity_at` for `fqn` to `candidate` iff it is newer
    /// than the stored value (monotonic forward). Returns true (and dirties the
    /// store) only when it actually moved, so an idle team (stable candidate) does
    /// not churn the file each tick. Preserves last_user_message_at / auto_closed_at.
    pub fn note_activity(&mut self, fqn: &str, candidate: DateTime<Utc>) -> bool {
        let entry = self.map.entry(fqn.to_string()).or_default();
        if entry.last_activity_at.is_none_or(|prev| candidate > prev) {
            entry.last_activity_at = Some(candidate);
            self.dirty = true;
            return true;
        }
        false
    }

    /// Auto-closed marker accessor.
    pub fn auto_closed_at(&self, fqn: &str) -> Option<DateTime<Utc>> {
        self.map.get(fqn).and_then(|e| e.auto_closed_at)
    }

    /// #552 Mark this coordinator's team auto-closed at `now`. Idempotent:
    /// returns `true` only on the transition None -> Some, so the caller emits
    /// the event once. Preserves `last_user_message_at` (the badge keeps
    /// counting on the dormant row).
    pub fn mark_auto_closed(&mut self, fqn: &str, now: DateTime<Utc>) -> bool {
        let entry = self.map.entry(fqn.to_string()).or_default();
        // #588 manual close wins: never auto-mark a coordinator the user closed by
        // hand (stops a concurrent tick re-setting auto_closed_at + emitting a stray
        // coordinator_auto_close_changed).
        if entry.manually_closed_at.is_some() {
            return false;
        }
        if entry.auto_closed_at.is_some() {
            return false;
        }
        entry.auto_closed_at = Some(now);
        self.dirty = true;
        true
    }

    /// #552 Clear the auto-closed marker on reopen. Returns `true` only on the
    /// transition Some -> None, so the caller emits the clear event once.
    /// Preserves `last_user_message_at`.
    pub fn clear_auto_closed(&mut self, fqn: &str) -> bool {
        if let Some(entry) = self.map.get_mut(fqn) {
            if entry.auto_closed_at.is_some() {
                entry.auto_closed_at = None;
                self.dirty = true;
                return true;
            }
        }
        false
    }

    /// #588 Manual-close marker accessor.
    pub fn manually_closed_at(&self, fqn: &str) -> Option<DateTime<Utc>> {
        self.map.get(fqn).and_then(|e| e.manually_closed_at)
    }

    /// #588 Mark this coordinator manually-closed at `now`. Idempotent: returns
    /// `true` only on the None -> Some transition so the caller emits once. Clears
    /// `auto_closed_at` ON THE TRANSITION (mutual-exclusion that also keeps the
    /// dirty flag honest; manual is the deliberate action). Preserves
    /// last_user_message_at / last_activity_at.
    pub fn mark_manually_closed(&mut self, fqn: &str, now: DateTime<Utc>) -> bool {
        let entry = self.map.entry(fqn.to_string()).or_default();
        if entry.manually_closed_at.is_some() {
            return false;
        }
        entry.auto_closed_at = None; // mutual-exclusion, on the real transition
        entry.manually_closed_at = Some(now);
        self.dirty = true;
        true
    }

    /// #588 Clear the manual-close marker on reopen. Returns `true` only on the
    /// Some -> None transition so the caller emits the clear once. Preserves
    /// last_user_message_at.
    pub fn clear_manually_closed(&mut self, fqn: &str) -> bool {
        if let Some(entry) = self.map.get_mut(fqn) {
            if entry.manually_closed_at.is_some() {
                entry.manually_closed_at = None;
                self.dirty = true;
                return true;
            }
        }
        false
    }

    /// (#756) Fresh-intent accessor.
    pub fn start_fresh_at(&self, fqn: &str) -> Option<DateTime<Utc>> {
        self.map.get(fqn).and_then(|e| e.start_fresh_at)
    }

    /// (#756) Mark a fresh-conversation boundary for `fqn` at `now`. Idempotent:
    /// returns `true` only on the None -> Some transition (the FIRST boundary
    /// time is preserved), so the caller logs/persists once. Preserves every
    /// other field.
    pub fn mark_start_fresh(&mut self, fqn: &str, now: DateTime<Utc>) -> bool {
        let entry = self.map.entry(fqn.to_string()).or_default();
        if entry.start_fresh_at.is_some() {
            return false;
        }
        entry.start_fresh_at = Some(now);
        self.dirty = true;
        true
    }

    /// (#756) Drop the fresh intent once a post-boundary transcript exists
    /// (typed input or AC-injected content). Returns `true` only on the
    /// Some -> None transition. Preserves every other field.
    pub fn clear_start_fresh(&mut self, fqn: &str) -> bool {
        if let Some(entry) = self.map.get_mut(fqn) {
            if entry.start_fresh_at.is_some() {
                entry.start_fresh_at = None;
                self.dirty = true;
                return true;
            }
        }
        false
    }

    pub fn snapshot(&self) -> HashMap<String, ClockEntry> {
        self.map.clone()
    }

    pub fn take_dirty(&mut self) -> bool {
        std::mem::take(&mut self.dirty)
    }

    /// (#621) Remove every key for one workgroup: all FQNs prefixed
    /// `"<project>:<wg>/"`. Returns the number of keys removed; dirties the store
    /// iff any were removed. The prefix is reconstructed exactly from the on-disk
    /// project folder name + workgroup dir name (no hashing), so it is precise and
    /// can never touch another workgroup. The trailing '/' stops a short wg name
    /// from matching a longer sibling (`wg-1/` must not match `wg-12/coord`).
    /// (#621 LOW-1) The compare is case-INSENSITIVE: the key carries the spawn
    /// `working_directory` case while the caller passes `project_path.file_name()`,
    /// and the prune + Windows FS are case-insensitive; an exact match would
    /// `Ok(0)`-silently miss a case-drifted project. Case-insensitive project+wg
    /// identity IS the FS identity, so this still cannot touch another workgroup.
    pub fn remove_workgroup(&mut self, project: &str, wg: &str) -> usize {
        let prefix = format!("{}:{}/", project, wg);
        self.retain_keys(|k| !key_has_prefix_ignore_ascii_case(k, &prefix))
    }

    /// (#621) Retain only keys for which `keep` returns true. Returns the number
    /// removed; dirties the store iff any were removed. Backbone for
    /// `remove_workgroup` and the startup prune.
    pub fn retain_keys<F: FnMut(&str) -> bool>(&mut self, mut keep: F) -> usize {
        let before = self.map.len();
        self.map.retain(|k, _| keep(k));
        let removed = before - self.map.len();
        if removed > 0 {
            self.dirty = true;
        }
        removed
    }
}

/// (#621 LOW-1) ASCII-case-insensitive prefix test that never panics on a non-char
/// boundary (`get(..n)` returns None) and never allocates. `eq_ignore_ascii_case`
/// matches Windows FS / project-resolution case-insensitivity; project + wg names
/// are ASCII in practice, and a non-ASCII boundary simply yields "no match" (keep),
/// which the case-insensitive startup prune still backstops.
fn key_has_prefix_ignore_ascii_case(key: &str, prefix: &str) -> bool {
    key.get(..prefix.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(prefix))
}

pub type CoordinatorClocksState = std::sync::Arc<Mutex<CoordinatorClocks>>;

/// `None` when no config dir resolves (no home dir). Callers degrade: load ->
/// empty default, save -> skip with a warn.
fn clocks_path() -> Option<PathBuf> {
    crate::config::config_dir().map(|d| d.join("coordinator_clocks.json"))
}

pub fn load() -> CoordinatorClocks {
    let Some(path) = clocks_path() else {
        log::warn!("[coordinator-clocks] no config dir; badge clocks start empty");
        return CoordinatorClocks::default();
    };
    let map = match std::fs::read_to_string(&path) {
        Ok(raw) => match serde_json::from_str::<HashMap<String, ClockEntry>>(&raw) {
            Ok(m) => m,
            Err(e) => {
                // Do NOT silently wipe (grinch L5): log, then start empty.
                log::warn!(
                    "[coordinator-clocks] corrupt {}: {}; starting empty",
                    path.display(),
                    e
                );
                HashMap::new()
            }
        },
        Err(_) => HashMap::new(), // first run / missing file
    };
    CoordinatorClocks { map, dirty: false }
}

/// Atomic save of the flat map. Symmetric with `load`. Reuses the hardened
/// rename path (create_dir_all + rename_with_retry) shared with sessions.
/// No-op (Ok) when no config dir resolves.
pub fn save_map(map: &HashMap<String, ClockEntry>) -> Result<(), String> {
    let Some(path) = clocks_path() else {
        log::warn!("[coordinator-clocks] no config dir; skipping save");
        return Ok(());
    };
    save_map_to(&path, map)
}

/// Path-explicit variant for unit tests (exercises the REAL serialize+rename).
pub fn save_map_to(path: &Path, map: &HashMap<String, ClockEntry>) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let json = serde_json::to_string_pretty(map).map_err(|e| e.to_string())?;
    // (#621 MED-2) Unique temp per write: two concurrent saves in one process
    // (auto-close tick + off-tick workgroup-remove) must not share a temp path,
    // or interleaved writes produce a corrupt file that the next load() wipes.
    let tmp = path.with_extension(format!(
        "json.{}.{}.tmp",
        std::process::id(),
        SAVE_TMP_SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::write(&tmp, json).map_err(|e| e.to_string())?;
    // Reuse sessions_persistence::rename_with_retry for the Windows AV/indexer
    // hold retry (#280/#291). Discard the rich diagnostics on error.
    crate::config::sessions_persistence::rename_with_retry(&tmp, path)
        .map_err(|(msg, _diag)| msg)?;
    Ok(())
}

/// (#621) GUI-path helper: remove a workgroup's keys from the in-memory store
/// (authoritative for the running app) and persist iff something changed. Returns
/// keys removed. Used by `delete_workgroup` and `delete_team`.
pub fn remove_workgroup_in_state(state: &CoordinatorClocksState, project: &str, wg: &str) -> usize {
    let removed = {
        let mut g = state.lock().unwrap_or_else(|e| e.into_inner());
        g.remove_workgroup(project, wg)
    };
    if removed > 0 {
        let snap = { state.lock().unwrap_or_else(|e| e.into_inner()).snapshot() };
        if let Err(e) = save_map(&snap) {
            log::warn!("[coordinator-clocks] workgroup-remove save failed: {}", e);
        }
    }
    removed
}

/// (#621) CLI-path helper: the CLI runs in its own process with no in-memory
/// store, so load -> remove -> save the on-disk map directly. Best-effort; returns
/// Ok(0) when no config dir resolves. NOTE: if the GUI is running concurrently it
/// owns the in-memory map and may re-persist the key on its next flush; the
/// startup prune is the backstop for that race (see `prune_orphaned_workgroups_and_persist`).
pub fn remove_workgroup_on_disk(project: &str, wg: &str) -> Result<usize, String> {
    let mut clocks = load();
    let removed = clocks.remove_workgroup(project, wg);
    if removed > 0 {
        save_map(&clocks.snapshot())?;
    }
    Ok(removed)
}

/// (#621) Conservative startup backstop: drop clock keys whose workgroup dir is
/// CONFIRMED gone on disk, EXCEPT any key that belongs to a persisted session.
/// Locks `state`, prunes, persists iff anything changed. Never fails startup
/// (logs on save error). Cleans historical orphans + anything a concurrent/
/// standalone CLI remove left behind.
///
/// (#621 MED-1) The `live_fqns` keep-set is built from `sessions.json` BEFORE the
/// lock and makes the prune immune to FQN homonyms + temporarily-unregistered
/// projects: a clock key is NEVER pruned while a session with that FQN is
/// persisted. This literally enforces the issue's "never delete state of a live
/// workgroup" constraint. The prune runs in `lib.rs` before the restore, but
/// `load_sessions_raw()` is a read-only snapshot, safe to read here.
///
/// (#621 I4) The std `Mutex` is held across the per-key `is_dir` fs-IO inside
/// `prune_with`/`should_keep_clock_key`. Benign: this runs single-threaded in
/// `run()` BEFORE `.manage(coordinator_clocks)` and `.setup()`, so nothing else
/// touches the store yet. Do NOT relocate this onto a hot path / shared-state tick
/// without moving the fs work out from under the lock.
pub fn prune_orphaned_workgroups_and_persist(
    state: &CoordinatorClocksState,
    project_paths: &[String],
) {
    let candidates =
        crate::config::projects::enumerate_registered_project_candidates(project_paths);
    let live_fqns = persisted_session_fqns();
    let removed = {
        let mut guard = state.lock().unwrap_or_else(|e| e.into_inner());
        prune_with(&mut guard, &candidates, &live_fqns)
    };
    if removed > 0 {
        let snap = { state.lock().unwrap_or_else(|e| e.into_inner()).snapshot() };
        if let Err(e) = save_map(&snap) {
            log::warn!("[coordinator-clocks] startup prune save failed: {}", e);
        }
        log::info!(
            "[coordinator-clocks] startup prune removed {} orphaned workgroup key(s)",
            removed
        );
    }
}

/// (#621) FQN keep-set from the persisted session snapshot (read-only). Same
/// function (`agent_fqn_from_path`) + same input (`working_directory`) that seeds
/// the clock keys, so a live coordinator's session maps to exactly its clock key.
/// Member sessions add harmless extra entries (their FQNs never equal a clock key).
fn persisted_session_fqns() -> HashSet<String> {
    crate::config::sessions_persistence::load_sessions_raw()
        .iter()
        .map(|s| crate::config::teams::agent_fqn_from_path(&s.working_directory))
        .collect()
}

/// (#621) Testable prune core: returns keys removed. A key is KEPT iff it belongs
/// to a persisted session (MED-1) OR `should_keep_clock_key` keeps it. Split out so
/// tests can inject `candidates` + `live_fqns` without touching real sessions.json.
pub fn prune_with(
    clocks: &mut CoordinatorClocks,
    candidates: &[crate::config::projects::ProjectResolution],
    live_fqns: &HashSet<String>,
) -> usize {
    clocks.retain_keys(|key| live_fqns.contains(key) || should_keep_clock_key(key, candidates))
}

/// (#621) Keep-decision for one clock key during the startup prune. CONSERVATIVE:
/// returns true (KEEP) on ANY uncertainty. Returns false (PRUNE) only when the key
/// is a WG-replica FQN whose project resolves to EXACTLY ONE registered project,
/// that project's workspace dir exists, and the specific `<wg>` dir under it is
/// confirmed absent.
fn should_keep_clock_key(
    key: &str,
    candidates: &[crate::config::projects::ProjectResolution],
) -> bool {
    // Origin-agent keys ("<project>/<agent>", no ':') are never workgroup-scoped.
    let Some((project, rest)) = key.split_once(':') else {
        return true;
    };
    // Malformed (no "<wg>/<agent>") -> keep.
    let Some((wg, _agent)) = rest.split_once('/') else {
        return true;
    };
    // Resolve the project folder name to EXACTLY ONE registered candidate
    // (case-insensitive, matching resolve_project_reference). 0 or >1 -> keep.
    let mut matches = candidates
        .iter()
        .filter(|c| c.folder_name.eq_ignore_ascii_case(project));
    let (Some(resolved), None) = (matches.next(), matches.next()) else {
        return true;
    };
    // Workspace dir missing/unreadable -> cannot confirm absence -> keep.
    let Some(ac_root) = crate::config::ac_root::existing_ac_root(&resolved.path) else {
        return true;
    };
    // Keep iff the workgroup dir still exists; prune only on CONFIRMED absence.
    ac_root.join(wg).is_dir()
}

/// Path-explicit load for unit tests, symmetric with `save_map_to`.
pub fn load_from(path: &Path) -> CoordinatorClocks {
    let map = match std::fs::read_to_string(path) {
        Ok(raw) => serde_json::from_str::<HashMap<String, ClockEntry>>(&raw).unwrap_or_default(),
        Err(_) => HashMap::new(),
    };
    CoordinatorClocks { map, dirty: false }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::projects::enumerate_registered_project_candidates;
    use chrono::Duration;

    fn ts(secs: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(1_700_000_000 + secs, 0).expect("valid timestamp")
    }

    #[test]
    fn note_user_message_coalesces_within_window() {
        let mut clocks = CoordinatorClocks::default();
        let fqn = "proj:wg-1-team/coord";

        // First message always records.
        assert!(clocks.note_user_message(fqn, ts(0)));
        // Inside the 10s window -> skipped.
        assert!(!clocks.note_user_message(fqn, ts(5)));
        // The recorded timestamp is unchanged by the skipped write.
        assert_eq!(clocks.last_user_message_at(fqn), Some(ts(0)));
        // At/after the window -> recorded.
        assert!(clocks.note_user_message(fqn, ts(10)));
        assert_eq!(clocks.last_user_message_at(fqn), Some(ts(10)));
    }

    #[test]
    fn seed_if_absent_never_overwrites() {
        let mut clocks = CoordinatorClocks::default();
        let fqn = "proj:wg-1-team/coord";

        assert!(clocks.seed_if_absent(fqn, ts(0)));
        assert_eq!(clocks.last_user_message_at(fqn), Some(ts(0)));
        // A respawn must NOT reset the clock.
        assert!(!clocks.seed_if_absent(fqn, ts(100)));
        assert_eq!(clocks.last_user_message_at(fqn), Some(ts(0)));
    }

    #[test]
    fn take_dirty_toggles() {
        let mut clocks = CoordinatorClocks::default();
        assert!(!clocks.take_dirty(), "fresh store is clean");
        clocks.note_user_message("proj:wg-1-team/coord", ts(0));
        assert!(clocks.take_dirty(), "a recorded message dirties the store");
        assert!(!clocks.take_dirty(), "take_dirty resets the flag");
    }

    #[test]
    fn mark_auto_closed_is_idempotent() {
        let mut clocks = CoordinatorClocks::default();
        let fqn = "proj:wg-1-team/coord";

        assert!(
            clocks.mark_auto_closed(fqn, ts(0)),
            "first mark transitions None -> Some"
        );
        assert_eq!(clocks.auto_closed_at(fqn), Some(ts(0)));
        assert!(
            !clocks.mark_auto_closed(fqn, ts(50)),
            "second mark is a no-op (already Some)"
        );
        // The original marker time is preserved on the idempotent call.
        assert_eq!(clocks.auto_closed_at(fqn), Some(ts(0)));
    }

    #[test]
    fn clear_auto_closed_only_on_some_to_none_and_preserves_badge() {
        let mut clocks = CoordinatorClocks::default();
        let fqn = "proj:wg-1-team/coord";

        // No entry -> nothing to clear.
        assert!(!clocks.clear_auto_closed(fqn));

        clocks.note_user_message(fqn, ts(0));
        clocks.mark_auto_closed(fqn, ts(1));
        assert!(clocks.clear_auto_closed(fqn), "Some -> None returns true");
        assert_eq!(clocks.auto_closed_at(fqn), None);
        // The badge clock survives the clear.
        assert_eq!(clocks.last_user_message_at(fqn), Some(ts(0)));
        // Clearing again is a no-op.
        assert!(!clocks.clear_auto_closed(fqn));
    }

    #[test]
    fn mark_manually_closed_is_idempotent() {
        let mut clocks = CoordinatorClocks::default();
        let fqn = "proj:wg-1-team/coord";

        assert!(
            clocks.mark_manually_closed(fqn, ts(0)),
            "first mark transitions None -> Some"
        );
        assert_eq!(clocks.manually_closed_at(fqn), Some(ts(0)));
        assert!(
            !clocks.mark_manually_closed(fqn, ts(50)),
            "second mark is a no-op (already Some)"
        );
        // The original marker time is preserved on the idempotent call.
        assert_eq!(clocks.manually_closed_at(fqn), Some(ts(0)));
    }

    #[test]
    fn clear_manually_closed_only_on_some_to_none_and_preserves_badge() {
        let mut clocks = CoordinatorClocks::default();
        let fqn = "proj:wg-1-team/coord";

        // No entry -> nothing to clear.
        assert!(!clocks.clear_manually_closed(fqn));

        clocks.note_user_message(fqn, ts(0));
        clocks.mark_manually_closed(fqn, ts(1));
        assert!(
            clocks.clear_manually_closed(fqn),
            "Some -> None returns true"
        );
        assert_eq!(clocks.manually_closed_at(fqn), None);
        // The badge clock survives the clear.
        assert_eq!(clocks.last_user_message_at(fqn), Some(ts(0)));
        // Clearing again is a no-op.
        assert!(!clocks.clear_manually_closed(fqn));
    }

    #[test]
    fn mark_start_fresh_is_idempotent_and_preserves_first_time() {
        let mut clocks = CoordinatorClocks::default();
        let fqn = "proj:wg-1-team/coord";

        assert!(
            clocks.mark_start_fresh(fqn, ts(0)),
            "first mark transitions None -> Some"
        );
        assert_eq!(clocks.start_fresh_at(fqn), Some(ts(0)));
        assert!(
            !clocks.mark_start_fresh(fqn, ts(50)),
            "second mark is a no-op (already Some)"
        );
        // The FIRST boundary time is preserved on the idempotent call.
        assert_eq!(clocks.start_fresh_at(fqn), Some(ts(0)));
    }

    #[test]
    fn clear_start_fresh_only_on_some_to_none_and_preserves_other_fields() {
        let mut clocks = CoordinatorClocks::default();
        let fqn = "proj:wg-1-team/coord";

        // No entry -> nothing to clear.
        assert!(!clocks.clear_start_fresh(fqn));

        clocks.note_user_message(fqn, ts(0));
        clocks.mark_auto_closed(fqn, ts(1));
        clocks.mark_start_fresh(fqn, ts(2));
        assert!(clocks.clear_start_fresh(fqn), "Some -> None returns true");
        assert_eq!(clocks.start_fresh_at(fqn), None);
        // The badge clock and the auto-closed marker survive the clear.
        assert_eq!(clocks.last_user_message_at(fqn), Some(ts(0)));
        assert_eq!(clocks.auto_closed_at(fqn), Some(ts(1)));
        // Clearing again is a no-op.
        assert!(!clocks.clear_start_fresh(fqn));
    }

    #[test]
    fn start_fresh_at_defaults_none_for_legacy_json() {
        // Legacy coordinator_clocks.json entries carry no `startFreshAt` key;
        // the additive field must deserialize to None.
        let raw = r#"{ "proj:wg-1-team/coord": { "lastUserMessageAt": "2023-11-14T22:13:20Z" } }"#;
        let map: HashMap<String, ClockEntry> =
            serde_json::from_str(raw).expect("legacy JSON must deserialize");
        let clocks = CoordinatorClocks { map, dirty: false };
        assert_eq!(clocks.start_fresh_at("proj:wg-1-team/coord"), None);
        assert_eq!(
            clocks.last_user_message_at("proj:wg-1-team/coord"),
            Some(ts(0))
        );
    }

    #[test]
    fn mark_manually_closed_clears_preset_auto_closed_on_transition() {
        let mut clocks = CoordinatorClocks::default();
        let fqn = "proj:wg-1-team/coord";

        clocks.note_user_message(fqn, ts(0));
        clocks.mark_auto_closed(fqn, ts(1));
        assert_eq!(clocks.auto_closed_at(fqn), Some(ts(1)));

        // Marking manual clears the auto marker on the real transition.
        assert!(clocks.mark_manually_closed(fqn, ts(2)));
        assert_eq!(
            clocks.auto_closed_at(fqn),
            None,
            "mark_manually_closed must clear a pre-set auto_closed_at"
        );
        assert_eq!(clocks.manually_closed_at(fqn), Some(ts(2)));
        // The badge clock is untouched.
        assert_eq!(clocks.last_user_message_at(fqn), Some(ts(0)));
    }

    #[test]
    fn mark_auto_closed_skips_when_manually_closed() {
        let mut clocks = CoordinatorClocks::default();
        let fqn = "proj:wg-1-team/coord";

        clocks.mark_manually_closed(fqn, ts(0));
        // A concurrent auto-close tick must NOT re-mark a manually-closed coordinator.
        assert!(
            !clocks.mark_auto_closed(fqn, ts(5)),
            "mark_auto_closed must skip when manually_closed_at is set"
        );
        assert_eq!(
            clocks.auto_closed_at(fqn),
            None,
            "auto_closed_at must stay None when manual close already won"
        );
        assert_eq!(clocks.manually_closed_at(fqn), Some(ts(0)));
    }

    #[test]
    fn note_user_message_preserves_auto_closed_marker() {
        let mut clocks = CoordinatorClocks::default();
        let fqn = "proj:wg-1-team/coord";

        clocks.mark_auto_closed(fqn, ts(0));
        // A later user message updates only the badge, not the marker.
        assert!(clocks.note_user_message(fqn, ts(100)));
        assert_eq!(clocks.last_user_message_at(fqn), Some(ts(100)));
        assert_eq!(
            clocks.auto_closed_at(fqn),
            Some(ts(0)),
            "note_user_message must not touch auto_closed_at"
        );
    }

    #[test]
    fn note_activity_advances_monotonic_forward_only() {
        let mut clocks = CoordinatorClocks::default();
        let fqn = "proj:wg-1-team/coord";

        // First activity: None -> Some advances and dirties.
        assert!(clocks.note_activity(fqn, ts(100)));
        assert_eq!(clocks.last_activity_at(fqn), Some(ts(100)));
        assert!(clocks.take_dirty(), "a real advance dirties the store");

        // A newer candidate advances; clear dirty to isolate the no-op checks.
        assert!(clocks.note_activity(fqn, ts(200)));
        assert_eq!(clocks.last_activity_at(fqn), Some(ts(200)));
        assert!(clocks.take_dirty(), "a forward advance dirties the store");

        // An older candidate is a no-op (monotonic forward) and does not dirty.
        assert!(!clocks.note_activity(fqn, ts(150)));
        assert_eq!(clocks.last_activity_at(fqn), Some(ts(200)));
        assert!(
            !clocks.take_dirty(),
            "an older candidate must not dirty the store"
        );

        // An equal candidate is also a no-op (advance is strictly-newer).
        assert!(!clocks.note_activity(fqn, ts(200)));
        assert_eq!(clocks.last_activity_at(fqn), Some(ts(200)));
        assert!(
            !clocks.take_dirty(),
            "an equal candidate must not dirty the store"
        );
    }

    #[test]
    fn note_activity_preserves_user_message_and_auto_closed() {
        let mut clocks = CoordinatorClocks::default();
        let fqn = "proj:wg-1-team/coord";

        clocks.note_user_message(fqn, ts(0));
        clocks.mark_auto_closed(fqn, ts(1));
        // Advancing activity must touch ONLY last_activity_at.
        assert!(clocks.note_activity(fqn, ts(500)));
        assert_eq!(clocks.last_activity_at(fqn), Some(ts(500)));
        assert_eq!(
            clocks.last_user_message_at(fqn),
            Some(ts(0)),
            "note_activity must not touch last_user_message_at"
        );
        assert_eq!(
            clocks.auto_closed_at(fqn),
            Some(ts(1)),
            "note_activity must not touch auto_closed_at"
        );
    }

    #[test]
    fn idle_anchor_is_max_of_present_components() {
        assert_eq!(ClockEntry::default().idle_anchor(), None);

        let user_only = ClockEntry {
            last_user_message_at: Some(ts(100)),
            ..Default::default()
        };
        assert_eq!(user_only.idle_anchor(), Some(ts(100)));

        let activity_only = ClockEntry {
            last_activity_at: Some(ts(200)),
            ..Default::default()
        };
        assert_eq!(activity_only.idle_anchor(), Some(ts(200)));

        let both = ClockEntry {
            last_user_message_at: Some(ts(100)),
            last_activity_at: Some(ts(200)),
            ..Default::default()
        };
        assert_eq!(
            both.idle_anchor(),
            Some(ts(200)),
            "anchor is the max of the two present components"
        );
    }

    /// B1/H1 trap: exercise the REAL save_map_to + load_from pair (NOT a
    /// hand-rolled map round-trip) so an asymmetry between serialize and
    /// deserialize is caught. Asserts BOTH ClockEntry fields survive.
    #[test]
    fn save_load_real_round_trip_is_symmetric() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("coordinator_clocks.json");

        let mut original: HashMap<String, ClockEntry> = HashMap::new();
        original.insert(
            "proj:wg-1-team/coord".to_string(),
            ClockEntry {
                last_user_message_at: Some(ts(0) + Duration::minutes(45)),
                last_activity_at: Some(ts(0) + Duration::minutes(50)),
                auto_closed_at: Some(ts(0) + Duration::minutes(60)),
                manually_closed_at: Some(ts(0) + Duration::minutes(70)),
                start_fresh_at: Some(ts(0) + Duration::minutes(80)),
            },
        );
        // A second entry exercising the skip_serializing_if absence of two fields.
        original.insert(
            "proj:wg-2-team/coord".to_string(),
            ClockEntry {
                last_user_message_at: Some(ts(0)),
                last_activity_at: None,
                auto_closed_at: None,
                manually_closed_at: None,
                start_fresh_at: None,
            },
        );

        save_map_to(&path, &original).expect("save");
        let loaded = load_from(&path);

        assert_eq!(
            loaded.snapshot(),
            original,
            "the loaded map must equal the saved map (save/load symmetry)"
        );
        // Spot-check all three fields explicitly via the accessors.
        assert_eq!(
            loaded.last_user_message_at("proj:wg-1-team/coord"),
            Some(ts(0) + Duration::minutes(45))
        );
        assert_eq!(
            loaded.last_activity_at("proj:wg-1-team/coord"),
            Some(ts(0) + Duration::minutes(50)),
            "last_activity_at must survive save/load (#580)"
        );
        assert_eq!(
            loaded.auto_closed_at("proj:wg-1-team/coord"),
            Some(ts(0) + Duration::minutes(60))
        );
        assert_eq!(
            loaded.manually_closed_at("proj:wg-1-team/coord"),
            Some(ts(0) + Duration::minutes(70)),
            "manually_closed_at must survive save/load (#588)"
        );
        assert_eq!(
            loaded.start_fresh_at("proj:wg-1-team/coord"),
            Some(ts(0) + Duration::minutes(80)),
            "start_fresh_at must survive save/load (#756)"
        );
        assert_eq!(loaded.last_activity_at("proj:wg-2-team/coord"), None);
        assert_eq!(loaded.auto_closed_at("proj:wg-2-team/coord"), None);
        assert_eq!(loaded.manually_closed_at("proj:wg-2-team/coord"), None);
        assert_eq!(loaded.start_fresh_at("proj:wg-2-team/coord"), None);
    }

    // ── #621 targeted-removal + startup-prune tests ──────────────────────────

    #[test]
    fn remove_workgroup_drops_only_target_prefix() {
        let mut clocks = CoordinatorClocks::default();
        for k in [
            "proj:wg-1-team/coord",
            "proj:wg-1-team/dev",    // same wg, second agent
            "proj:wg-2-team/coord",  // sibling wg
            "other:wg-1-team/coord", // different project, same wg name
        ] {
            clocks.note_user_message(k, ts(0));
        }
        clocks.seed_if_absent("proj/architect", ts(0)); // origin agent, no ':'
        let _ = clocks.take_dirty();

        let removed = clocks.remove_workgroup("proj", "wg-1-team");
        assert_eq!(removed, 2, "only the two proj:wg-1-team/ keys are dropped");
        assert!(clocks.take_dirty(), "a real removal dirties the store");
        assert_eq!(clocks.last_user_message_at("proj:wg-1-team/coord"), None);
        assert_eq!(clocks.last_user_message_at("proj:wg-1-team/dev"), None);
        assert!(clocks
            .last_user_message_at("proj:wg-2-team/coord")
            .is_some());
        assert!(clocks
            .last_user_message_at("other:wg-1-team/coord")
            .is_some());
        assert!(clocks.last_user_message_at("proj/architect").is_some());
    }

    #[test]
    fn remove_workgroup_trailing_slash_guards_sibling_prefix() {
        let mut clocks = CoordinatorClocks::default();
        clocks.note_user_message("proj:wg-1/coord", ts(0));
        clocks.note_user_message("proj:wg-12/coord", ts(0)); // wg-1 is a string prefix of wg-12
        assert_eq!(clocks.remove_workgroup("proj", "wg-1"), 1);
        assert_eq!(clocks.last_user_message_at("proj:wg-1/coord"), None);
        assert!(
            clocks.last_user_message_at("proj:wg-12/coord").is_some(),
            "trailing '/' must stop wg-1 from matching wg-12"
        );
    }

    #[test]
    fn remove_workgroup_absent_is_noop_and_clean() {
        let mut clocks = CoordinatorClocks::default();
        clocks.note_user_message("proj:wg-2-team/coord", ts(0));
        let _ = clocks.take_dirty();
        assert_eq!(clocks.remove_workgroup("proj", "wg-1-team"), 0);
        assert!(
            !clocks.take_dirty(),
            "removing nothing must not dirty the store"
        );
    }

    #[test]
    fn remove_workgroup_is_case_insensitive() {
        let mut clocks = CoordinatorClocks::default();
        clocks.note_user_message("MyProj:WG-1-Team/coord", ts(0)); // key carries spawn case
                                                                   // Caller passes a different case (e.g. project resolved as a read_dir child).
        assert_eq!(clocks.remove_workgroup("myproj", "wg-1-team"), 1);
        assert_eq!(clocks.last_user_message_at("MyProj:WG-1-Team/coord"), None);
    }

    #[test]
    fn should_keep_clock_key_policy() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let proj = tmp.path().join("MyProj");
        std::fs::create_dir_all(proj.join(".ac").join("wg-1-live")).unwrap();
        // wg-2-gone deliberately not created.
        let project_paths = vec![proj.to_string_lossy().to_string()];
        let candidates = enumerate_registered_project_candidates(&project_paths);

        // Live wg dir present -> KEEP.
        assert!(should_keep_clock_key("MyProj:wg-1-live/coord", &candidates));
        // Wg dir confirmed absent -> PRUNE.
        assert!(!should_keep_clock_key(
            "MyProj:wg-2-gone/coord",
            &candidates
        ));
        // Origin agent (no ':') -> KEEP.
        assert!(should_keep_clock_key("MyProj/architect", &candidates));
        // Unknown/unregistered project -> KEEP (cannot confirm).
        assert!(should_keep_clock_key(
            "Unregistered:wg-1/coord",
            &candidates
        ));
        // Project name resolves case-insensitively (matches resolve_project_reference).
        assert!(!should_keep_clock_key(
            "myproj:wg-2-gone/coord",
            &candidates
        ));
    }

    #[test]
    fn should_keep_clock_key_keeps_when_project_unenumerable() {
        // (#621 E1) A project whose `.ac` does not exist yields ZERO candidates
        // (enumerate requires `.ac`), so the key is kept by the 0-match branch, NOT
        // by the `existing_ac_root` guard (which is only a TOCTOU belt).
        let tmp = tempfile::tempdir().expect("temp dir");
        let proj = tmp.path().join("NoAc");
        std::fs::create_dir_all(&proj).unwrap(); // project exists, but no .ac/
        let candidates =
            enumerate_registered_project_candidates(&[proj.to_string_lossy().to_string()]);
        assert!(candidates.is_empty(), "no .ac -> not a candidate");
        assert!(should_keep_clock_key("NoAc:wg-1/coord", &candidates));
    }

    #[test]
    fn should_keep_clock_key_keeps_ambiguous_homonym_project() {
        // (#621 E3 / LOW-3a) Two registered projects share a folder_name; one lacks
        // the wg. >1 match -> cannot confirm -> KEEP (mirrors resolve_project_reference
        // Ambiguous). This is the conservative guard that backs MED-1.
        let tmp = tempfile::tempdir().expect("temp dir");
        let a = tmp.path().join("a").join("app");
        let b = tmp.path().join("b").join("app"); // same folder_name "app", different parent
        std::fs::create_dir_all(a.join(".ac").join("wg-1-team")).unwrap(); // a HAS the wg
        std::fs::create_dir_all(b.join(".ac")).unwrap(); // b lacks wg-1-team
        let candidates = enumerate_registered_project_candidates(&[
            a.to_string_lossy().to_string(),
            b.to_string_lossy().to_string(),
        ]);
        // Even though `b` lacks wg-1-team, the homonym makes "app" ambiguous -> KEEP.
        assert!(should_keep_clock_key("app:wg-1-team/coord", &candidates));
    }

    #[test]
    fn prune_with_never_drops_a_persisted_session_key() {
        // (#621 MED-1) The cardinal-sin guard: a live coordinator of a temporarily
        // UNREGISTERED homonym project must keep its clock even though the prune would
        // otherwise resolve the name to the registered homonym and find the wg absent.
        let tmp = tempfile::tempdir().expect("temp dir");
        let registered = tmp.path().join("work").join("app"); // registered, lacks wg-5
        std::fs::create_dir_all(registered.join(".ac")).unwrap();
        let candidates =
            enumerate_registered_project_candidates(&[registered.to_string_lossy().to_string()]);

        // Control: with NO keep-set the key WOULD be pruned (one match, wg-5 absent).
        let empty: HashSet<String> = HashSet::new();
        let mut probe = CoordinatorClocks::default();
        probe.note_user_message("app:wg-5-team/coord", ts(0));
        assert_eq!(
            prune_with(&mut probe, &candidates, &empty),
            1,
            "unprotected -> pruned"
        );

        // With the persisted-session keep-set the live key survives.
        let mut clocks = CoordinatorClocks::default();
        clocks.note_user_message("app:wg-5-team/coord", ts(0)); // live coord of the OTHER "app"
        let live: HashSet<String> = HashSet::from(["app:wg-5-team/coord".to_string()]);
        assert_eq!(
            prune_with(&mut clocks, &candidates, &live),
            0,
            "live key protected"
        );
        assert!(clocks.last_user_message_at("app:wg-5-team/coord").is_some());
    }
}
