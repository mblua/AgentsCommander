use super::*;

#[test]
fn pair_activity_two_edges_and_older_edge_keep_digest_and_maximum() {
    let f = Fixture::new(false);
    let original = f.reserve(&f.request()).unwrap();
    let d = read_config_pair_physical(&f.d()).unwrap();
    assert_eq!(f.activity(T2), Ok(true));
    assert_eq!(f.activity(T3), Ok(true));
    assert_eq!(f.activity(T1), Ok(false));
    let packet = f.packet();
    assert_eq!(packet.timestamp_floor.as_deref(), Some(T3));
    assert_eq!(packet.plan_digest, original.plan_digest);
    assert_eq!(
        packet.protected_revision_digests().unwrap(),
        original.protected_revision_digests().unwrap()
    );
    assert_eq!(read_config_pair_physical(&f.d()).unwrap(), d);
    execute_reserved_config_pair(&f.d(), &f.s(), &f.r(), "operation-1").unwrap();
    assert_eq!(timestamp_value(&f.tuple()[1]).unwrap().as_deref(), Some(T3));
}

#[test]
fn pair_activity_every_t0_t4_cut_recovers_with_monotonic_stamp() {
    for absent in [false, true] {
        for cut in 0..=4 {
            let f = Fixture::new(absent);
            f.reserve(&f.request()).unwrap();
            if cut != 0 {
                let result = execute_reserved_config_pair_with_stage(
                    &f.d(),
                    &f.s(),
                    &f.r(),
                    "operation-1",
                    &|stage| {
                        if stage == cut {
                            Err(PreparedPairError::Io("lost progress".into()))
                        } else {
                            Ok(())
                        }
                    },
                );
                assert_eq!(result, Err(PreparedPairError::Io("lost progress".into())));
            }
            assert_eq!(f.activity(T2), Ok(true));
            assert_eq!(f.activity(T3), Ok(true));
            assert_eq!(f.activity(T1), Ok(false));
            // Re-read durable packet: no in-memory plan or progress authority.
            let packet = f.packet();
            assert_eq!(packet.timestamp_floor.as_deref(), Some(T3));
            execute_reserved_config_pair(&f.d(), &f.s(), &f.r(), "operation-1").unwrap();
            assert_eq!(f.packet().recognize(&f.tuple()).unwrap(), 4);
            assert_eq!(timestamp_value(&f.tuple()[1]).unwrap().as_deref(), Some(T3));
            assert_eq!(f.tuple()[0].map().unwrap()["decision"], json!(1));
            assert_eq!(f.tuple()[0].map().unwrap()["opaque"], Value::Null);
            if !absent {
                assert_eq!(f.tuple()[1].map().unwrap()["opaque"], json!([1, 2]));
            }
        }
    }
}

#[test]
fn pair_activity_intent_state_floor_crashes_never_report_false_success() {
    for stop in [
        "activity_intent_durable",
        "activity_state_durable",
        "activity_floor_durable",
    ] {
        let f = Fixture::new(false);
        f.reserve(&f.request()).unwrap();
        assert_eq!(
            f.interrupted_activity(T3, stop),
            Err(PreparedPairError::Io("lost activity response".into()))
        );
        let packet = f.packet();
        if stop == "activity_intent_durable" {
            assert_eq!(timestamp_value(&f.tuple()[1]).unwrap().as_deref(), Some(T1));
            assert!(packet.activity_intent.is_some());
            // A generic no-op cannot consolidate a pending activity.
            let bytes = std::fs::read(f.r()).unwrap();
            let tuple = f.tuple();
            update_config_pair_guarded(
                &f.d(),
                &f.s(),
                &f.r(),
                &[],
                None,
                None,
                &|_| panic!("no publish"),
                |_, _| Ok(()),
                &|_| panic!("no stamp"),
                None,
            )
            .unwrap();
            assert_eq!(std::fs::read(f.r()).unwrap(), bytes);
            assert_eq!(f.tuple(), tuple);
        } else {
            assert_eq!(timestamp_value(&f.tuple()[1]).unwrap().as_deref(), Some(T3));
        }
        assert_eq!(f.activity(T2), Ok(false));
        assert_eq!(timestamp_value(&f.tuple()[1]).unwrap().as_deref(), Some(T3));
        assert_eq!(f.packet().timestamp_floor.as_deref(), Some(T3));
        assert!(f.packet().activity_intent.is_none());
    }
}

#[test]
fn pair_activity_absent_overlay_structural_presence_and_legacy_floor() {
    let f = Fixture::new(true);
    f.reserve(&f.request()).unwrap();
    assert_eq!(f.activity(T2), Ok(true));
    let state = f.tuple()[1].map().unwrap();
    assert_eq!(
        state,
        json!({"tooling":{"lastAgentMessageAt":T2}})
            .as_object()
            .unwrap()
            .clone()
    );
    assert!(protected_matches(&PhysicalState::Absent, &f.tuple()[1], true).unwrap());
    let empty = PhysicalState::from_bytes(b"{}".to_vec());
    assert_ne!(
        protected_revision(&f.tuple()[1], true).unwrap(),
        protected_revision(&empty, true).unwrap()
    );
    execute_reserved_config_pair(&f.d(), &f.s(), &f.r(), "operation-1").unwrap();
    assert_eq!(timestamp_value(&f.tuple()[1]).unwrap().as_deref(), Some(T2));
    assert_eq!(f.tuple()[0].map().unwrap()["opaque"], Value::Null);
}

#[test]
fn pair_activity_external_unknown_deleted_malformed_or_lower_timestamp_conflicts() {
    for kind in [
        "unknown",
        "deleted",
        "malformed",
        "lower",
        "tooling",
        "stateGone",
    ] {
        let f = Fixture::new(false);
        f.reserve(&f.request()).unwrap();
        f.activity(T3).unwrap();
        let mut map = f.tuple()[1].map().unwrap();
        match kind {
            "unknown" => {
                map.insert("outside".into(), json!(7));
            }
            "deleted" => {
                map.get_mut("tooling")
                    .unwrap()
                    .as_object_mut()
                    .unwrap()
                    .remove("lastAgentMessageAt");
            }
            "malformed" => {
                map.get_mut("tooling").unwrap()["lastAgentMessageAt"] = json!("bad");
            }
            "lower" => {
                map.get_mut("tooling").unwrap()["lastAgentMessageAt"] = json!(T1);
            }
            "tooling" => {
                map.insert("tooling".into(), Value::Null);
            }
            _ => {}
        }
        if kind == "stateGone" {
            std::fs::remove_file(f.s()).unwrap();
        } else {
            f.write(&f.s(), Value::Object(map));
        }
        let tuple = f.tuple();
        let packet = std::fs::read(f.r()).unwrap();
        assert!(f.activity(T2).is_err());
        assert_eq!(
            execute_reserved_config_pair(&f.d(), &f.s(), &f.r(), "operation-1"),
            Err(PreparedPairError::Conflict)
        );
        assert_eq!(f.tuple(), tuple);
        assert_eq!(std::fs::read(f.r()).unwrap(), packet);
    }
}

#[test]
fn pair_activity_corrupt_packet_wrong_target_and_before_cas_preserve_evidence() {
    let f = Fixture::new(false);
    let mut wrong = f.request();
    wrong.before_protected[0] = ProtectedRevision::Absent;
    assert_eq!(f.reserve(&wrong), Err(PreparedPairError::Conflict));
    assert!(!f.r().exists());
    f.reserve(&f.request()).unwrap();
    let other = Fixture::new(false);
    std::fs::write(other.r(), std::fs::read(f.r()).unwrap()).unwrap();
    assert!(other.activity(T2).is_err());
    let tuple = f.tuple();
    f.write(&f.r(), json!({"version":1,"operationId":"operation-1"}));
    let bytes = std::fs::read(f.r()).unwrap();
    assert!(f.activity(T2).is_err());
    assert_eq!(f.tuple(), tuple);
    assert_eq!(std::fs::read(f.r()).unwrap(), bytes);
}

#[test]
fn pair_activity_initial_null_parse_error_incompatible_tooling_zero_write() {
    for bytes in [
        b"null".as_slice(),
        b"[]",
        b"not json",
        br#"{"tooling":null}"#,
        br#"{"tooling":{"lastAgentMessageAt":null}}"#,
        br#"{"tooling":{"lastAgentMessageAt":"bad"}}"#,
    ] {
        let f = Fixture::new(false);
        let request = f.request();
        std::fs::write(f.s(), bytes).unwrap();
        let d = std::fs::read(f.d()).unwrap();
        assert!(f.reserve(&request).is_err());
        assert!(!f.r().exists());
        assert_eq!(std::fs::read(f.s()).unwrap(), bytes);
        assert_eq!(std::fs::read(f.d()).unwrap(), d);
    }
}

#[test]
fn pair_activity_collapsed_absent_stages_choose_greatest_and_keep_overlay() {
    let f = Fixture {
        root: tempfile::tempdir().unwrap(),
    };
    let request = f.request();
    let packet =
        prepare_and_reserve_config_pair(&f.d(), &f.s(), &f.r(), &request, None, |_, _| Ok(()))
            .unwrap();
    assert_eq!(packet.recognize(&f.tuple()).unwrap(), 4);
    execute_reserved_config_pair(&f.d(), &f.s(), &f.r(), "operation-1").unwrap();
    assert!(!f.d().exists());
    assert!(!f.s().exists());
    assert_eq!(f.activity(T2), Ok(true));
    execute_reserved_config_pair(&f.d(), &f.s(), &f.r(), "operation-1").unwrap();
    assert!(!f.d().exists());
    assert_eq!(timestamp_value(&f.tuple()[1]).unwrap().as_deref(), Some(T2));
    assert_eq!(f.packet().recognize(&f.tuple()).unwrap(), 4);
}

#[test]
fn pair_activity_intent_unknown_timestamp_and_exact_generic_delta() {
    let f = Fixture::new(false);
    f.reserve(&f.request()).unwrap();
    let tuple = f.tuple();
    update_config_pair_guarded(
        &f.d(),
        &f.s(),
        &f.r(),
        &[],
        None,
        None,
        &|_| panic!("no cleanup stages"),
        |_, s| {
            s.get_mut("tooling").unwrap()["lastAgentMessageAt"] = json!(T2);
            Ok(())
        },
        &|_| panic!("no split stamp"),
        None,
    )
    .unwrap();
    assert_eq!(f.tuple()[0], tuple[0]);
    assert_eq!(f.packet().timestamp_floor.as_deref(), Some(T2));
    let before = f.tuple();
    let packet = std::fs::read(f.r()).unwrap();
    let error = update_config_pair_guarded(
        &f.d(),
        &f.s(),
        &f.r(),
        &[],
        None,
        None,
        &|_| {},
        |_, s| {
            s.get_mut("tooling").unwrap()["lastAgentMessageAt"] = json!(T1);
            Ok(())
        },
        &|_| panic!("no stamp"),
        None,
    )
    .unwrap_err();
    assert!(error.contains("targetTransitionPending"));
    assert_eq!(f.tuple(), before);
    assert_eq!(std::fs::read(f.r()).unwrap(), packet);
    assert!(f
        .interrupted_activity(T3, "activity_intent_durable")
        .is_err());
    let mut state = f.tuple()[1].map().unwrap();
    state.get_mut("tooling").unwrap()["lastAgentMessageAt"] = json!("2026-10-08T02:30:00Z");
    f.write(&f.s(), Value::Object(state));
    let tuple = f.tuple();
    let packet = std::fs::read(f.r()).unwrap();
    assert!(f.activity(T3).is_err());
    assert_eq!(f.tuple(), tuple);
    assert_eq!(std::fs::read(f.r()).unwrap(), packet);
}

#[test]
fn pair_activity_preflight_precedes_state_sidecars_and_holds_decisions_lease() {
    let f = Fixture::new(false);
    let parent = f.root.path().canonicalize().unwrap();
    let state_lock = config_lock_path(&parent, &f.s()).unwrap();
    let reservation_lock = config_lock_path(&parent, &f.r()).unwrap();
    let called = Cell::new(false);
    let preflight = || {
        assert!(!state_lock.exists());
        assert!(!reservation_lock.exists());
        assert!(acquire_config_file_write_lock(&f.d(), Duration::from_millis(20)).is_err());
        called.set(true);
        Err("ignore preflight refused".to_string())
    };
    let before = f.tuple();
    let error = update_config_pair_guarded(
        &f.d(),
        &f.s(),
        &f.r(),
        &[],
        Some(&preflight),
        None,
        &|_| panic!("no publish"),
        |_, _| panic!("no mutation"),
        &|_| Ok(()),
        None,
    )
    .unwrap_err();
    assert_eq!(error, "ignore preflight refused");
    assert!(called.get());
    assert_eq!(f.tuple(), before);
    assert!(!f.r().exists());
    assert!(!state_lock.exists());
    assert!(!reservation_lock.exists());
    std::fs::write(f.r(), b"{invalid").unwrap();
    called.set(false);
    assert!(update_config_pair_guarded(
        &f.d(),
        &f.s(),
        &f.r(),
        &[],
        Some(&preflight),
        None,
        &|_| panic!("no publish"),
        |_, _| panic!("no mutation"),
        &|_| Ok(()),
        None
    )
    .is_err());
    assert!(!called.get());
    assert_eq!(std::fs::read(f.r()).unwrap(), b"{invalid");
    assert!(!state_lock.exists());
    assert!(!reservation_lock.exists());
}

#[test]
fn single_editor_pair_activity_release_requires_local_receipt_source_and_final_target() {
    let f = Fixture::new(false);
    f.reserve(&f.request()).unwrap();
    let target = f.packet().physical_target_identity;
    let ledger = f.complete_projection();
    let revision = &ledger["afterSourceRevision"];
    let proof =
        PairCompleteProof::from_single_editor_complete(&ledger, "operation-1", &target, revision)
            .unwrap();
    assert_eq!(
        release_reserved_config_pair(&f.d(), &f.s(), &f.r(), &proof),
        Err(PreparedPairError::Conflict)
    );
    assert!(PairCompleteProof::from_single_editor_complete(
        &ledger,
        "operation-1",
        &target,
        &json!({"kind":"absent"})
    )
    .is_err());
    for key in ["phase", "sourcePublished", "receipt"] {
        let mut invalid = ledger.clone();
        invalid[key] = Value::Null;
        assert!(PairCompleteProof::from_single_editor_complete(
            &invalid,
            "operation-1",
            &target,
            revision
        )
        .is_err());
    }
    execute_reserved_config_pair(&f.d(), &f.s(), &f.r(), "operation-1").unwrap();
    f.activity(T3).unwrap();
    let before = f.tuple();
    assert_eq!(
        release_reserved_config_pair(&f.d(), &f.s(), &f.r(), &proof).unwrap(),
        before
    );
    assert!(!f.r().exists());
    assert!(config_lock_path(f.root.path(), &f.r()).unwrap().exists());
    assert_eq!(
        release_reserved_config_pair(&f.d(), &f.s(), &f.r(), &proof).unwrap(),
        before
    );
    assert_eq!(timestamp_value(&before[1]).unwrap().as_deref(), Some(T3));
}
