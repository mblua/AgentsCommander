use super::*;

/// (#1399 T1) Same-target exclusion and RAII release.
#[test]
fn same_target_is_exclusive_until_the_reservation_drops() {
    let lanes = Arc::new(WakeLanes::default());
    let first = lanes.try_reserve("proj:wg-1/dev-rust");
    assert!(first.is_some());
    assert!(lanes.try_reserve("proj:wg-1/dev-rust").is_none());
    drop(first);
    assert_eq!(lanes.len(), 0);
    assert!(lanes.try_reserve("proj:wg-1/dev-rust").is_some());
}

/// (#1399 T1) The global cap refuses a ninth distinct target until one
/// reservation is released.
#[test]
fn ninth_distinct_target_is_refused_until_a_release() {
    let lanes = Arc::new(WakeLanes::default());
    let mut held: Vec<WakeLaneReservation> = (0..WAKE_WORKER_LIMIT)
        .map(|slot| {
            lanes
                .try_reserve(&format!("target-{slot}"))
                .expect("distinct target under the cap must reserve")
        })
        .collect();
    assert!(lanes.try_reserve("target-8").is_none());
    held.pop();
    let ninth = lanes.try_reserve("target-8");
    assert!(ninth.is_some());
    drop(ninth);
    drop(held);
    assert_eq!(lanes.len(), 0);
}

#[test]
fn issue_3371_observations_keep_targets_and_release_active_capacity() {
    let lanes = Arc::new(WakeLanes::default());
    let held: Vec<_> = (0..WAKE_WORKER_LIMIT)
        .map(|slot| Arc::new(lanes.try_reserve(&format!("old-{slot}")).unwrap()))
        .collect();
    assert!(lanes.try_reserve("ninth").is_none());
    for reservation in &held {
        reservation.begin_observation();
        reservation.begin_observation();
    }
    assert_eq!(lanes.observing_count(), 8);
    let active: Vec<_> = (0..WAKE_WORKER_LIMIT)
        .map(|slot| lanes.try_reserve(&format!("new-{slot}")).unwrap())
        .collect();
    assert_eq!(lanes.len(), 16);
    assert!(lanes.try_reserve("old-0").is_none());
    assert!(lanes.try_reserve("another").is_none());
    let duplicate = held[0].clone();
    drop(held);
    assert_eq!(lanes.observing_count(), 1);
    assert!(lanes.try_reserve("old-0").is_none());
    drop(active);
    assert_eq!(lanes.len(), 1);
    drop(duplicate);
    assert_eq!(lanes.len(), 0);
    assert_eq!(lanes.observing_count(), 0);
}

#[test]
fn issue_3371_observation_panic_and_drop_orders_balance() {
    for demote in [false, true] {
        let lanes = Arc::new(WakeLanes::default());
        let owner = lanes.clone();
        assert!(std::panic::catch_unwind(move || {
            let reservation = owner.try_reserve("panic").unwrap();
            if demote {
                reservation.begin_observation();
            }
            panic!("fixture unwind");
        })
        .is_err());
        assert_eq!(lanes.len(), 0);
        assert_eq!(lanes.observing_count(), 0);
        assert!(lanes.try_reserve("panic").is_some());
    }
}
