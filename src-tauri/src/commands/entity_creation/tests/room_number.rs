use super::*;

/// Empty workspace returns slot 1, the lowest positive integer.
#[test]
fn determine_next_wg_number_returns_one_when_no_wg_dirs_exist() {
    let tmp = tempfile::tempdir().expect("tempdir");
    assert_eq!(determine_next_wg_number(tmp.path()), 1);
}

/// Contiguous allocation: `room-1`, `room-2`, `room-3` already exist for the team
/// → next slot is 4 (no internal gap to reuse).
#[test]
fn determine_next_wg_number_returns_next_after_contiguous_block() {
    let tmp = tempfile::tempdir().expect("tempdir");
    touch_dir(tmp.path(), "room-1-dev-team");
    touch_dir(tmp.path(), "room-2-dev-team");
    touch_dir(tmp.path(), "room-3-dev-team");
    assert_eq!(determine_next_wg_number(tmp.path()), 4);
}

/// Gap reuse — the load-bearing case from issue #177.
/// `room-1` and `room-3` exist (someone destroyed `room-2`) → next slot is 2.
#[test]
fn determine_next_wg_number_reuses_lowest_internal_gap() {
    let tmp = tempfile::tempdir().expect("tempdir");
    touch_dir(tmp.path(), "room-1-dev-team");
    touch_dir(tmp.path(), "room-3-dev-team");
    assert_eq!(determine_next_wg_number(tmp.path()), 2);
}

/// Leading gap — `room-1` is free even though higher slots are taken.
#[test]
fn determine_next_wg_number_reuses_slot_one_when_only_higher_slots_exist() {
    let tmp = tempfile::tempdir().expect("tempdir");
    touch_dir(tmp.path(), "room-2-dev-team");
    touch_dir(tmp.path(), "room-3-dev-team");
    assert_eq!(determine_next_wg_number(tmp.path()), 1);
}

/// Project scoping: dirs for any team block slot reuse.
/// `room-1-dev-team` and `room-3-qa-team` make slot 2 the next free slot.
#[test]
fn determine_next_wg_number_is_global_across_team_suffixes() {
    let tmp = tempfile::tempdir().expect("tempdir");
    touch_dir(tmp.path(), "room-1-dev-team");
    touch_dir(tmp.path(), "room-1-qa-team");
    touch_dir(tmp.path(), "room-3-qa-team");
    assert_eq!(determine_next_wg_number(tmp.path()), 2);
}

/// Invalid `room-*` directory names must not occupy any slot.
/// - `room-abc-dev-team`: non-numeric middle → parse fails → ignored.
/// - `room--dev-team`:    empty middle (`[3..3]` slice) → parse fails → ignored.
///
/// Only `room-2-dev-team` is real, so slot 1 is still free.
/// (The `room-dev-team` no-number case is covered by its own test below
/// because it specifically exercises the checked-slicing guard.)
#[test]
fn determine_next_wg_number_ignores_invalid_directory_names() {
    let tmp = tempfile::tempdir().expect("tempdir");
    touch_dir(tmp.path(), "room-abc-dev-team");
    touch_dir(tmp.path(), "room--dev-team");
    touch_dir(tmp.path(), "room-2-dev-team");
    assert_eq!(determine_next_wg_number(tmp.path()), 1);
}

/// `room-0-<team>` does not block slot 1. The allocator's lowest-free
/// search starts at 1, so any `0` that ends up in `taken` is never
/// tested by `find` — slot 1 stays reachable. The allocator only ever
/// produces values ≥ 1.
#[test]
fn determine_next_wg_number_ignores_zero_numbered_dir() {
    let tmp = tempfile::tempdir().expect("tempdir");
    touch_dir(tmp.path(), "room-0-dev-team");
    touch_dir(tmp.path(), "room-2-dev-team");
    assert_eq!(determine_next_wg_number(tmp.path()), 1);
}

/// Files (not directories) named like a room must not occupy a slot —
/// the allocator only considers real room directories.
#[test]
fn determine_next_wg_number_ignores_files_named_like_workgroups() {
    let tmp = tempfile::tempdir().expect("tempdir");
    std::fs::write(tmp.path().join("room-1-dev-team"), b"not a dir").expect("write file");
    assert_eq!(determine_next_wg_number(tmp.path()), 1);
}

/// Regression for the suffix-overlaps-prefix slice case: a directory
/// named `room-{team}` (no number, e.g. `room-dev-team`) passes both the
/// prefix and the `ends_with("-{team}")` check, but an index-based digits
/// slice would be `&name_str[3..2]` — invalid, and #1614 makes that worse
/// because `"room-".len()` is 5, not 3. `strip_entity_prefix` plus
/// `strip_suffix` removes the index arithmetic entirely and cannot panic.
/// This test locks in the no-panic behavior for both prefixes.
#[test]
fn determine_next_wg_number_does_not_panic_on_no_number_dir() {
    let tmp = tempfile::tempdir().expect("tempdir");
    touch_dir(tmp.path(), "room-dev-team");
    // Must return slot 1 (the bogus dir is ignored, not counted as taken).
    assert_eq!(determine_next_wg_number(tmp.path()), 1);
}

/// In-flight `.deleting-room-N-team-<uuid>` directories must NOT be
/// counted as occupying slot N. Locks the contract that #177 relies
/// on: the leading `.` of the temp name (set in `try_atomic_delete_wg`
/// at line 1535 — `.deleting-{wg_name}-{uuid}`) dodges the widened
/// `entity_prefix::has_entity_prefix` filter, so a freed slot is reusable on the
/// very next allocation tick. A future temp-name refactor that drops
/// the leading `.` would silently re-introduce the gap-leak this issue
/// closes; this test catches that regression.
#[test]
fn determine_next_wg_number_ignores_deleting_temp_dirs() {
    let tmp = tempfile::tempdir().expect("tempdir");
    touch_dir(tmp.path(), "room-1-dev-team");
    touch_dir(
        tmp.path(),
        ".deleting-room-2-dev-team-00000000-0000-0000-0000-000000000000",
    );
    // wg-2 is mid-delete: the `.deleting-…` entry must not block slot 2.
    assert_eq!(determine_next_wg_number(tmp.path()), 2);
}

/// Team suffix overlap is irrelevant to global allocation.
#[test]
fn determine_next_wg_number_handles_subset_team_suffixes_globally() {
    let tmp = tempfile::tempdir().expect("tempdir");
    touch_dir(tmp.path(), "room-1-dev-team");
    touch_dir(tmp.path(), "room-1-team");
    assert_eq!(determine_next_wg_number(tmp.path()), 2);
}

// ── #1614 — the Room slot namespace is independent of the legacy one ──

/// AC3 and requirement (B), the load-bearing case: in a `.ac` root that
/// already holds `wg-1-<team>`, the next Room is `room-1-<team>`, NOT
/// `room-3-<team>`. D5 is the single `strip_prefix` site in the change that
/// deliberately does NOT become dual; a sweep that made it dual would break
/// requirement (B) silently while every dual-prefix test stayed green.
#[test]
fn room_allocator_ignores_legacy_workgroup_directories() {
    let tmp = tempfile::tempdir().expect("tempdir");
    touch_dir(tmp.path(), "wg-1-dev-team");
    touch_dir(tmp.path(), "wg-2-dev-team");
    assert_eq!(
        determine_next_wg_number(tmp.path()),
        1,
        "legacy Room directories must not occupy a Room slot"
    );
}

/// The existing lowest-free-positive-slot reuse semantics are preserved for
/// Rooms; only the prefix the scan reads changed.
#[test]
fn room_allocator_reuses_lowest_free_room_slot() {
    let tmp = tempfile::tempdir().expect("tempdir");
    touch_dir(tmp.path(), "room-1-t");
    touch_dir(tmp.path(), "room-3-t");
    assert_eq!(determine_next_wg_number(tmp.path()), 2);
}

/// The `read_dir`-failure path is unchanged: it still degrades to slot 1,
/// and the post-allocate `wg_dir.exists()` guard is what surfaces the
/// degraded case as an "already exists" error (section 7 item 2).
#[test]
fn room_allocator_still_degrades_to_one_on_read_error() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let missing = tmp.path().join("no-such-ac-root");
    assert_eq!(determine_next_wg_number(&missing), 1);
}

/// The central name parser accepts both prefixes, and still rejects every
/// malformed shape it rejected before.
#[test]
fn parse_team_from_entity_name_accepts_room() {
    assert_eq!(
        parse_team_from_workgroup_name("room-1-ac-devs").expect("room parses"),
        "ac-devs"
    );
    assert_eq!(
        parse_team_from_workgroup_name("wg-1-ac-devs").expect("legacy still parses"),
        "ac-devs"
    );
    for bad in [
        "room--devs",
        "room-0-devs",
        "room-1-",
        "roomx-1-devs",
        "room-1",
    ] {
        assert!(
            parse_team_from_workgroup_name(bad).is_err(),
            "must reject {bad:?}"
        );
    }
}

/// Team deletion collects BOTH kinds (section 7 item 17), and a team name
/// longer than the directory remainder returns neither and does not panic
/// (section 7 item 18). The old `&name_str[3..name_str.len() - suffix]`
/// form could produce a reversed range here; `strip_suffix` cannot.
#[test]
fn collect_team_dirs_finds_room_and_legacy_and_survives_long_team_names() {
    let tmp = tempfile::tempdir().expect("tempdir");
    touch_dir(tmp.path(), "room-1-dev-team");
    touch_dir(tmp.path(), "wg-2-dev-team");
    touch_dir(tmp.path(), "room-3-qa-team");
    touch_dir(tmp.path(), "_team_dev-team");

    let mut found: Vec<String> = collect_team_workgroup_dirs(tmp.path(), "dev-team")
        .into_iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    found.sort();
    assert_eq!(found, vec!["room-1-dev-team", "wg-2-dev-team"]);

    // A team name longer than any directory remainder: no match, no panic.
    let long = "a-team-name-far-longer-than-any-directory-remainder";
    assert!(collect_team_workgroup_dirs(tmp.path(), long).is_empty());
}

// ── #271 — build_role_content (Role.md template merge) ──
