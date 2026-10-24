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
    let clocks = CoordinatorClocks {
        map,
        dirty: false,
        creates: HashMap::new(),
    };
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
    // The key carries the spawn case.
    clocks.note_user_message("MyProj:WG-1-Team/coord", ts(0));
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
    let candidates = enumerate_registered_project_candidates(&[proj.to_string_lossy().to_string()]);
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

/// (#2413) T9: the create ledger restores only when every overlapping create
/// failed, exactly once, on the last finish; any success wipes the escrow.
#[test]
fn create_ledger_handoff() {
    let fqn = "proj:wg-1-team/coord";
    let cleared = ClearedCloseMarkers {
        auto_closed_at: Some(ts(0)),
        manually_closed_at: None,
    };
    // Single create.
    let mut c = CoordinatorClocks::default();
    c.begin_coordinator_create(fqn, cleared);
    assert_eq!(c.finish_coordinator_create(fqn, false), Some(cleared));
    assert!(!c.has_create_ledger(fqn), "entry removed at active 0");
    c.begin_coordinator_create(fqn, cleared);
    assert_eq!(c.finish_coordinator_create(fqn, true), None);
    assert!(!c.has_create_ledger(fqn));
    // Unknown FQN -> None.
    assert_eq!(c.finish_coordinator_create(fqn, false), None);

    // Two overlapping creates: A cleared the marker, B cleared nothing.
    for (first_ok, second_ok) in [(true, true), (true, false), (false, true), (false, false)] {
        let mut c = CoordinatorClocks::default();
        c.begin_coordinator_create(fqn, cleared);
        c.begin_coordinator_create(fqn, ClearedCloseMarkers::default());
        assert_eq!(
            c.finish_coordinator_create(fqn, first_ok),
            None,
            "first finish always hands off"
        );
        assert!(c.has_create_ledger(fqn));
        let expected = (!first_ok && !second_ok).then_some(cleared);
        assert_eq!(
            c.finish_coordinator_create(fqn, second_ok),
            expected,
            "first_ok={first_ok} second_ok={second_ok}"
        );
        assert!(!c.has_create_ledger(fqn), "entry removed at active 0");
        assert!(!c.take_dirty(), "begin/finish never dirty the store");
    }

    // A newer cleared value replaces the escrowed one per field.
    let mut c = CoordinatorClocks::default();
    c.begin_coordinator_create(fqn, cleared);
    let manual = ClearedCloseMarkers {
        auto_closed_at: None,
        manually_closed_at: Some(ts(5)),
    };
    c.begin_coordinator_create(fqn, manual);
    assert_eq!(c.finish_coordinator_create(fqn, false), None);
    assert_eq!(
        c.finish_coordinator_create(fqn, false),
        Some(ClearedCloseMarkers {
            auto_closed_at: Some(ts(0)),
            manually_closed_at: Some(ts(5)),
        })
    );

    // The ledger is never serialized.
    let mut c = CoordinatorClocks::default();
    c.begin_coordinator_create(fqn, cleared);
    let json = serde_json::to_string(&c).expect("serialize");
    assert!(!json.contains("creates"), "{json}");
    assert!(!json.contains("escrow"), "{json}");
}
