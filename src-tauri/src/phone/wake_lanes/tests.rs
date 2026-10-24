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
