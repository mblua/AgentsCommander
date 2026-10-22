use std::collections::HashSet;
use std::path::Path;

/// Scan the selected Project AC Root for existing `room-<N>-{team_name}/` dirs and return the
/// **lowest free positive integer** starting at 1.
///
/// #1614: legacy `wg-*` directories are deliberately NOT counted, because the
/// Room and legacy Workgroup slot namespaces are independent; in a root that
/// already holds `wg-1-{team}` the next Room is `room-1-{team}`.
///
/// Issue #177: previously this returned `max(existing) + 1`, which left
/// permanent gaps after a room was destroyed. The new policy reuses
/// any freed numbers so the user-facing sequence stays compact.
///
/// Filtering rules:
/// - Only directories are considered (regular files are ignored).
/// - The directory name must match `room-<positive digits>-<team>`.
/// - Team suffix is ignored for allocation, so room numbers are unique
///   across the whole project.
///
/// Slot 1 is always reachable because the lowest-free search starts at
/// 1 (see the `find` call below); a stray `room-0-{team}` directory ends
/// up in `taken` but is never tested by `find` and so cannot displace
/// slot 1.
///
/// Read-error degradation: if `std::fs::read_dir(ac_root)` fails
/// (permission denied, transient I/O, broken junction, path-too-long
/// on Windows), the function returns `1` as a graceful fallback. The
/// post-allocate `wg_dir.exists()` guard in `create_workgroup` will
/// surface the real condition as an "already exists" error if a
/// `room-1-{team}` is in fact present; otherwise the slot-1 creation
/// succeeds with stale state. Surfacing the read error is tracked
/// separately and is out of scope for #177.
/// #2507 Pure seam: mask -> pad width, applying the fallback.
pub(crate) fn pad_width_for_mask(mask: &str) -> usize {
    crate::config::settings::validate_room_number_mask(mask)
        .unwrap_or_else(|_| crate::config::settings::default_room_number_mask().len())
}

/// #2507 The ONLY place a room directory name is built from a number.
pub(crate) fn room_dir_name(wg_number: u32, suffix: &str, width: usize) -> String {
    use crate::config::entity_prefix::ROOM_DIR_PREFIX;
    format!("{ROOM_DIR_PREFIX}{wg_number:0width$}-{suffix}")
}

/// #2507 Pad width for a NEW room number, from persisted settings.
pub(crate) fn room_number_pad_width() -> usize {
    pad_width_for_mask(&crate::config::settings::load_settings_for_cli().room_number_mask)
}

pub(crate) fn determine_next_wg_number(ac_root: &Path) -> u32 {
    let mut taken: HashSet<u32> = HashSet::new();

    if let Ok(entries) = std::fs::read_dir(ac_root) {
        for entry in entries.flatten() {
            if !entry.path().is_dir() {
                continue;
            }
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if let Some(rest) = name_str.strip_prefix(crate::config::entity_prefix::ROOM_DIR_PREFIX)
            {
                if let Some((middle, team)) = rest.split_once('-') {
                    if team.is_empty() {
                        continue;
                    }
                    if let Ok(n) = middle.parse::<u32>() {
                        taken.insert(n);
                    }
                }
            }
        }
    }

    // Lowest free positive integer ≥ 1. The bounded `..=u32::MAX` form avoids
    // any iterator-overflow footgun in debug builds; `find` short-circuits at
    // the first miss so the actual cost is O(taken.len() + 1) in practice.
    // A `0` may end up in `taken` (from a stray `wg-0-{team}`) but is never
    // tested here — the search starts at 1, so slot 1 is always reachable.
    (1u32..=u32::MAX).find(|n| !taken.contains(n)).unwrap_or(1)
}
