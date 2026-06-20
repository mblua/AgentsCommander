use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Minimum gap between two recorded user-message timestamps for the same
/// coordinator. Keystroke bursts inside this window are coalesced to one
/// update so we do not emit/persist per keystroke.
const COALESCE_SECS: i64 = 10;

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

    pub fn snapshot(&self) -> HashMap<String, ClockEntry> {
        self.map.clone()
    }

    pub fn take_dirty(&mut self) -> bool {
        std::mem::take(&mut self.dirty)
    }
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
    let tmp = path.with_extension(format!("json.{}.tmp", std::process::id()));
    std::fs::write(&tmp, json).map_err(|e| e.to_string())?;
    // Reuse sessions_persistence::rename_with_retry for the Windows AV/indexer
    // hold retry (#280/#291). Discard the rich diagnostics on error.
    crate::config::sessions_persistence::rename_with_retry(&tmp, path)
        .map_err(|(msg, _diag)| msg)?;
    Ok(())
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

        assert!(clocks.mark_auto_closed(fqn, ts(0)), "first mark transitions None -> Some");
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
        assert!(!clocks.take_dirty(), "an older candidate must not dirty the store");

        // An equal candidate is also a no-op (advance is strictly-newer).
        assert!(!clocks.note_activity(fqn, ts(200)));
        assert_eq!(clocks.last_activity_at(fqn), Some(ts(200)));
        assert!(!clocks.take_dirty(), "an equal candidate must not dirty the store");
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
            },
        );
        // A second entry exercising the skip_serializing_if absence of two fields.
        original.insert(
            "proj:wg-2-team/coord".to_string(),
            ClockEntry {
                last_user_message_at: Some(ts(0)),
                last_activity_at: None,
                auto_closed_at: None,
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
        assert_eq!(loaded.last_activity_at("proj:wg-2-team/coord"), None);
        assert_eq!(loaded.auto_closed_at("proj:wg-2-team/coord"), None);
    }
}
