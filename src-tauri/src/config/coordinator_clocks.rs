use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::config::instance_artifacts::COORDINATOR_CLOCKS_FILE_NAME;
use crate::filesystem::coordinator_clocks as clock_io;

/// Minimum gap between two recorded user-message timestamps for the same
/// coordinator. Keystroke bursts inside this window are coalesced to one
/// update so we do not emit/persist per keystroke.
const COALESCE_SECS: i64 = 10;

/// (#621 MED-2) Monotonic suffix so concurrent `save_map_to` calls in one process
/// (auto-close tick + off-tick workgroup-remove saves) never share a temp path.
static SAVE_TMP_SEQ: AtomicU64 = AtomicU64::new(0);

/// (#2413) Orders close-marker changes made by coordinator creates with their
/// events. The create's clear and a failed create's restore each hold it from
/// the clocks change through their emits, so the UI receives their events in
/// the same order as the clock changes. Taken before the clocks lock, never
/// held across an `.await`.
pub struct CoordinatorCreateOrder {
    order: Mutex<()>,
}

impl CoordinatorCreateOrder {
    pub(crate) fn new() -> Self {
        Self {
            order: Mutex::new(()),
        }
    }

    pub(crate) fn with_order<T>(&self, operation: impl FnOnce() -> T) -> T {
        let _order = self.order.lock().unwrap_or_else(|error| error.into_inner());
        operation()
    }
}

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
    /// injection (Claude/Pi/Antigravity --continue / Codex resume --last).
    /// Survives record destruction (idle auto-close, manual close).
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
    /// (#2413) In-flight coordinator creates per FQN. In-memory only; never
    /// persisted, never marks dirty.
    #[serde(skip)]
    creates: HashMap<String, CreateLedger>,
}

/// (#2413) Close markers a coordinator create cleared, kept so a failed create
/// can put them back with their original timestamps.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ClearedCloseMarkers {
    pub auto_closed_at: Option<DateTime<Utc>>,
    pub manually_closed_at: Option<DateTime<Utc>>,
}

/// (#2413) Creates still running for one FQN and the markers they cleared.
#[derive(Debug, Default)]
struct CreateLedger {
    active: u32,
    escrow: ClearedCloseMarkers,
}

impl CoordinatorClocks {
    /// (#2413) Record the start of a coordinator create that cleared `cleared`.
    /// Per field, a newly cleared value replaces the escrowed one (it is the
    /// latest state).
    pub fn begin_coordinator_create(&mut self, fqn: &str, cleared: ClearedCloseMarkers) {
        let l = self.creates.entry(fqn.to_string()).or_default();
        l.active += 1;
        if cleared.auto_closed_at.is_some() {
            l.escrow.auto_closed_at = cleared.auto_closed_at;
        }
        if cleared.manually_closed_at.is_some() {
            l.escrow.manually_closed_at = cleared.manually_closed_at;
        }
    }

    /// (#2413) End one coordinator create. Returns the markers to restore: only
    /// for a failure that ends the last in-flight create. A success wipes the
    /// escrow: a session opened, so the markers must stay cleared.
    pub fn finish_coordinator_create(
        &mut self,
        fqn: &str,
        ok: bool,
    ) -> Option<ClearedCloseMarkers> {
        let l = self.creates.get_mut(fqn)?;
        l.active = l.active.saturating_sub(1);
        if ok {
            l.escrow = ClearedCloseMarkers::default();
        }
        if l.active > 0 {
            // Hand off to the creates still running.
            return None;
        }
        let e = self.creates.remove(fqn).map(|l| l.escrow)?;
        if ok || e == ClearedCloseMarkers::default() {
            None
        } else {
            Some(e)
        }
    }

    #[cfg(test)]
    pub(crate) fn has_create_ledger(&self, fqn: &str) -> bool {
        self.creates.contains_key(fqn)
    }

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
    crate::config::config_dir().map(|d| d.join(COORDINATOR_CLOCKS_FILE_NAME))
}

pub fn load() -> CoordinatorClocks {
    let Some(path) = clocks_path() else {
        log::warn!("[coordinator-clocks] no config dir; badge clocks start empty");
        return CoordinatorClocks::default();
    };
    let map = match clock_io::read_clock_document(&path) {
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
    CoordinatorClocks {
        map,
        dirty: false,
        creates: HashMap::new(),
    }
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
        clock_io::ensure_clock_parent(dir).map_err(|e| e.to_string())?;
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
    let file = clock_io::create_clock_stage(&tmp).map_err(|e| e.to_string())?;
    let write_result = (|| -> std::io::Result<()> {
        let mut file = file;
        clock_io::write_clock_stage(&mut file, json.as_bytes())?;
        #[cfg(all(test, windows))]
        let file = tests::issue_3278_sync_handle(&tmp, file)?;
        clock_io::sync_clock_stage(&file)
    })();
    if let Err(error) = write_result {
        // The closure dropped the handle before cleanup, including on Windows.
        let _ = clock_io::remove_clock_stage(&tmp);
        return Err(error.to_string());
    }
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
    clock_io::clock_workgroup_is_directory(&ac_root.join(wg))
}

/// Path-explicit load for unit tests, symmetric with `save_map_to`.
#[cfg(test)]
pub fn load_from(path: &Path) -> CoordinatorClocks {
    let map = match std::fs::read_to_string(path) {
        Ok(raw) => serde_json::from_str::<HashMap<String, ClockEntry>>(&raw).unwrap_or_default(),
        Err(_) => HashMap::new(),
    };
    CoordinatorClocks {
        map,
        dirty: false,
        creates: HashMap::new(),
    }
}

#[cfg(test)]
mod tests;
