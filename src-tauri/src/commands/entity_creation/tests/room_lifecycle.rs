use super::*;

#[test]
fn workgroup_removal_clears_clock_key_and_cache_sibling_intact() {
    use crate::config::coordinator_clocks::CoordinatorClocks;
    use crate::config::session_context::sweep_context_cache_dir;
    use std::time::{Duration, SystemTime};

    // (#621 E4) inline timestamp; no helper exists in this module.
    let ts = chrono::DateTime::from_timestamp(1_700_000_000, 0).unwrap();

    let tmp = tempfile::tempdir().expect("temp dir");
    let proj = tmp.path().join("Proj");
    let ac = proj.join(".ac");
    let wg1 = ac.join("wg-1-team");
    let wg2 = ac.join("wg-2-team");
    std::fs::create_dir_all(&wg1).unwrap();
    std::fs::create_dir_all(&wg2).unwrap();

    // Seed clocks: target wg, sibling wg, origin agent.
    let mut clocks = CoordinatorClocks::default();
    clocks.note_user_message("Proj:wg-1-team/coord", ts);
    clocks.note_user_message("Proj:wg-2-team/coord", ts);
    clocks.seed_if_absent("Proj/architect", ts);

    // Seed cache: a stale file for the removed wg's replica + a fresh sibling.
    let cache = tmp.path().join("context-cache");
    std::fs::create_dir_all(&cache).unwrap();
    let old = SystemTime::now() - Duration::from_secs(40 * 24 * 60 * 60);
    let stale = cache.join("replica-context-aaa.md");
    std::fs::write(&stale, "x").unwrap();
    std::fs::File::options()
        .write(true)
        .open(&stale)
        .unwrap()
        .set_modified(old)
        .unwrap();
    let fresh = cache.join("replica-context-bbb.md");
    std::fs::write(&fresh, "x").unwrap();

    // Remove wg-1: dir + clock key (cache via the age sweep).
    assert!(matches!(
        try_atomic_delete_wg(&wg1),
        WgDeleteOutcome::Deleted
    ));
    assert_eq!(clocks.remove_workgroup("Proj", "wg-1-team"), 1);
    let swept = sweep_context_cache_dir(
        &cache,
        SystemTime::now(),
        Duration::from_secs(30 * 24 * 60 * 60),
    );

    assert!(!wg1.exists() && wg2.exists(), "only wg-1 dir removed");
    assert_eq!(clocks.last_user_message_at("Proj:wg-1-team/coord"), None);
    assert!(
        clocks
            .last_user_message_at("Proj:wg-2-team/coord")
            .is_some(),
        "sibling clock intact"
    );
    assert!(
        clocks.last_user_message_at("Proj/architect").is_some(),
        "origin clock intact"
    );
    assert_eq!(swept, 1);
    assert!(
        !stale.exists() && fresh.exists(),
        "stale cache gone, fresh kept"
    );
}

#[test]
fn team_cascade_clears_each_wg_clock_keeps_others() {
    // (#621 LOW-3b) delete_team loops its wg-N-<team> dirs; each removal drops that
    // wg's keys. A non-team wg and an origin agent survive. Mirrors the §3.3 loop's
    // per-wg `remove_workgroup` over multiple wgs (accumulator semantics).
    use crate::config::coordinator_clocks::CoordinatorClocks;
    let ts = chrono::DateTime::from_timestamp(1_700_000_000, 0).unwrap();

    let mut clocks = CoordinatorClocks::default();
    for k in [
        "Proj:wg-1-squad/coord",
        "Proj:wg-2-squad/coord",
        "Proj:wg-2-squad/dev",   // second agent in a team wg
        "Proj:wg-9-other/coord", // a DIFFERENT team's wg, must survive
    ] {
        clocks.note_user_message(k, ts);
    }
    clocks.seed_if_absent("Proj/architect", ts); // origin, survives

    // Simulate the cascade over the "squad" team's wgs.
    let mut total = 0usize;
    for wg in ["wg-1-squad", "wg-2-squad"] {
        total += clocks.remove_workgroup("Proj", wg);
    }
    assert_eq!(total, 3, "wg-1 (1) + wg-2 (2 agents) keys removed");
    assert_eq!(clocks.last_user_message_at("Proj:wg-1-squad/coord"), None);
    assert_eq!(clocks.last_user_message_at("Proj:wg-2-squad/coord"), None);
    assert_eq!(clocks.last_user_message_at("Proj:wg-2-squad/dev"), None);
    assert!(
        clocks
            .last_user_message_at("Proj:wg-9-other/coord")
            .is_some(),
        "other team wg intact"
    );
    assert!(
        clocks.last_user_message_at("Proj/architect").is_some(),
        "origin intact"
    );
}

#[test]
fn failed_wg_delete_keeps_clock_key() {
    // (#621 LOW-3c) the clock cleanup is gated on a SUCCESSFUL dir delete (delete_workgroup
    // via `?` early-return; delete_team via the `else` success branch). Force a failed
    // remove and assert, applying that exact gate, that the key survives.
    use crate::config::coordinator_clocks::CoordinatorClocks;
    let ts = chrono::DateTime::from_timestamp(1_700_000_000, 0).unwrap();

    let tmp = tempfile::tempdir().expect("temp dir");
    let wg = tmp.path().join("wg-1-team");
    std::fs::create_dir_all(&wg).unwrap();
    let mut clocks = CoordinatorClocks::default();
    clocks.note_user_message("Proj:wg-1-team/coord", ts);

    // Force the remove step to fail (rename succeeds, remove_dir_all errors -> Partial).
    let outcome = try_atomic_delete_wg_with_remove(&wg, |_p| {
        Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "blocked",
        ))
    });
    // The production gate: clean the clock ONLY on Deleted.
    if matches!(outcome, WgDeleteOutcome::Deleted) {
        clocks.remove_workgroup("Proj", "wg-1-team");
    }
    assert!(
        !matches!(outcome, WgDeleteOutcome::Deleted),
        "forced failure is not a Deleted outcome"
    );
    assert!(
        clocks
            .last_user_message_at("Proj:wg-1-team/coord")
            .is_some(),
        "a failed delete must keep the clock key"
    );
}

/// #1795 `T15` (`AC-17`). The new shared directories must be invisible to room
/// discovery. `room-shared` is the one name that needs the argument rather than
/// a prefix mismatch: `has_entity_prefix("room-shared")` IS true, so it is never
/// created under an `.ac` root, only under a room root. Both scanners below are
/// local to this file, which is why the test lives here: written in a `config::*`
/// test module it would add a `config::* -> commands::entity_creation` arc.
#[test]
fn shared_directories_are_invisible_to_room_discovery() {
    let temp = tempfile::tempdir().expect("tempdir");
    let ac_root = temp.path().join(".ac");
    std::fs::create_dir_all(&ac_root).expect("ac root");

    let number_before = determine_next_wg_number(&ac_root);
    let rooms_before = list_workgroup_dirs(&ac_root);

    crate::config::shared_locations::create_project_shared_dirs(&ac_root)
        .expect("create the four project shared dirs");

    assert_eq!(
        determine_next_wg_number(&ac_root),
        number_before,
        "the four project shared dirs must not consume a room slot"
    );
    assert_eq!(
        list_workgroup_dirs(&ac_root),
        rooms_before,
        "the four project shared dirs must not appear as rooms"
    );

    let wg_dir = ac_root.join("room-1-dev-team");
    std::fs::create_dir_all(&wg_dir).expect("room dir");
    let number_with_room = determine_next_wg_number(&ac_root);
    let rooms_with_room = list_workgroup_dirs(&ac_root);
    assert_eq!(
        rooms_with_room,
        vec![wg_dir.clone()],
        "the fixture must contain exactly one real room"
    );

    crate::config::shared_locations::create_room_shared_dir(&wg_dir)
        .expect("create the room shared dir");

    assert_eq!(
        determine_next_wg_number(&ac_root),
        number_with_room,
        "`room-shared` under a room root must not consume a room slot"
    );
    assert_eq!(
        list_workgroup_dirs(&ac_root),
        vec![wg_dir],
        "`room-shared` under a room root must not appear as a phantom room"
    );
}

// ── #2507 room number mask ──

#[test]
fn default_mask_reproduces_todays_names() {
    let width = pad_width_for_mask("#");
    assert_eq!(width, 1);
    for n in [1u32, 9, 10, 99, 100] {
        assert_eq!(room_dir_name(n, "team", width), format!("room-{n}-team"));
    }
}

#[test]
fn room_dir_name_pads_and_never_truncates() {
    for width in 1usize..=9 {
        for n in [1u32, 9, 10, 99, 100, 4294967295] {
            let digits = n.to_string();
            let expected = if digits.len() < width {
                format!("room-{}{digits}-team", "0".repeat(width - digits.len()))
            } else {
                format!("room-{digits}-team")
            };
            assert_eq!(room_dir_name(n, "team", width), expected, "n={n} w={width}");
        }
    }
    assert_eq!(room_dir_name(100, "team", 2), "room-100-team");
    assert_eq!(room_dir_name(1, "team", 1), "room-1-team");
    assert_eq!(room_dir_name(1, "team", 2), "room-01-team");
    assert_eq!(room_dir_name(1, "team", 4), "room-0001-team");
}

#[test]
fn pad_width_for_mask_returns_the_mask_width() {
    assert_eq!(pad_width_for_mask("#"), 1);
    assert_eq!(pad_width_for_mask("##"), 2);
    assert_eq!(pad_width_for_mask("#########"), 9);
}

#[test]
fn allocated_room_number_is_never_zero() {
    let tmp = tempfile::tempdir().expect("tempdir");
    touch_dir(tmp.path(), "room-0-team");
    let n = determine_next_wg_number(tmp.path());
    assert_eq!(n, 1);
    assert_eq!(room_dir_name(n, "team", 2), "room-01-team");
}

#[test]
fn unpadded_existing_room_does_not_collide_with_a_padded_new_one() {
    let tmp = tempfile::tempdir().expect("tempdir");
    touch_dir(tmp.path(), "room-1-team");
    let n = determine_next_wg_number(tmp.path());
    assert_eq!(n, 2);
    let name = room_dir_name(n, "team", 2);
    assert_eq!(name, "room-02-team");
    for entry in std::fs::read_dir(tmp.path()).expect("read_dir") {
        assert_ne!(entry.expect("entry").file_name().to_string_lossy(), name);
    }
}

#[test]
fn invalid_or_missing_mask_falls_back_to_the_default_width() {
    let default_width = crate::config::settings::default_room_number_mask().len();
    assert_eq!(default_width, 1);
    for mask in ["", " ##", "##1", "2", "#-#", "##########"] {
        let width = pad_width_for_mask(mask);
        assert_eq!(width, default_width, "mask {mask:?}");
        assert_eq!(room_dir_name(1, "team", width), "room-1-team");
    }
}

/// C4 (#2470) E3, sites 5, 6 and 7, each on its own seeded directory: none
/// writes a state key to the tracked file and none touches the state file.
#[test]
fn c4_sites_5_6_7_leave_no_state_key() {
    let journal = tempfile::tempdir().expect("journal dir");
    let _journal = crate::config::agent_config::journal_redirect::set(journal.path());
    let seeded_replica = |fixture: &ReplicaCreationFixture| -> PathBuf {
        let replica_dir = fixture.wg_dir.join("__agent_tech-lead");
        std::fs::create_dir_all(&replica_dir).expect("create replica dir");
        let tracked = replica_dir.join("config.json");
        let seed = r#"{"identity": "../../_agent_tech-lead", "tooling": {}}"#;
        std::fs::write(&tracked, seed).expect("seed tracked");
        std::fs::write(c4_state_path(&tracked), C4_SEEDED_STATE).expect("seed state");
        replica_dir
    };

    let tmp = tempfile::tempdir().expect("tempdir");
    let tracked = tmp.path().join("config.json");
    std::fs::write(&tracked, r#"{"tooling": {}}"#).expect("seed tracked");
    std::fs::write(c4_state_path(&tracked), C4_SEEDED_STATE).expect("seed state");
    assert_eq!(
        write_local_config_value(&tracked, default_agent_matrix_config()),
        Ok(())
    );
    c4_tracked_is_clean(&tracked, "site 5");
    c4_state_is(&c4_state_path(&tracked), C4_SEEDED_STATE, "site 5");
    let written = replica_config(tmp.path());
    assert_eq!(
        written["context"],
        serde_json::json!(["$AGENTSCOMMANDER_CONTEXT", "Role.md"])
    );
    assert_eq!(written["tooling"], serde_json::json!({}));

    let fixture = replica_creation_fixture();
    let replica_dir = seeded_replica(&fixture);
    let tracked = replica_dir.join("config.json");
    let result = initialize_replica_config_on_disk(&replica_dir, &["repo-a".to_string()]);
    assert_eq!(result, Ok(()));
    c4_tracked_is_clean(&tracked, "site 6");
    c4_state_is(&c4_state_path(&tracked), C4_SEEDED_STATE, "site 6");
    let written = replica_config(&replica_dir);
    assert_eq!(written["repos"], serde_json::json!(["repo-a"]));
    assert_eq!(
        written["context"][0],
        serde_json::json!("$AGENTSCOMMANDER_CONTEXT")
    );

    let fixture = replica_creation_fixture();
    let replica_dir = seeded_replica(&fixture);
    let tracked = replica_dir.join("config.json");
    let identity = apply_wg_replica_repos(&replica_dir, &["repo-b".to_string()]).expect("site 7");
    assert_eq!(identity.identity, "../../_agent_tech-lead");
    c4_tracked_is_clean(&tracked, "site 7");
    c4_state_is(&c4_state_path(&tracked), C4_SEEDED_STATE, "site 7");
    assert_eq!(
        replica_config(&replica_dir)["repos"],
        serde_json::json!(["repo-b"])
    );
}
