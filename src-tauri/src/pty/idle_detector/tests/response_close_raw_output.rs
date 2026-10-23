use super::*;

#[test]
fn response_close_raw_output_observer_precedes_filter_and_receives_escape_only_output() {
    let observed = Arc::new(Mutex::new(Vec::new()));
    let captured = observed.clone();
    let detector = IdleDetector::new(|_| {}, |_| {});
    assert!(detector
        .bind_raw_output_observer(1, move |id, now| captured.lock().unwrap().push((id, now))));
    assert!(!detector.bind_raw_output_observer(2, |_, _| {}));
    let id = Uuid::new_v4();
    let now = Instant::now();
    detector.record_output_at(id, 3, false, now);
    assert_eq!(observed.lock().unwrap().as_slice(), &[(id, now)]);
    let silence = detector.silence.lock().unwrap().clone();
    let registered = detector.registered_at.lock().unwrap().clone();
    let idle = detector.idle_set.lock().unwrap().clone();
    let later = now + Duration::from_secs(1);
    detector.notify_raw_output_at(id, later);
    assert_eq!(
        observed.lock().unwrap().as_slice(),
        &[(id, now), (id, later)]
    );
    assert_eq!(*detector.silence.lock().unwrap(), silence);
    assert_eq!(*detector.registered_at.lock().unwrap(), registered);
    assert_eq!(*detector.idle_set.lock().unwrap(), idle);
    assert_eq!(detector.raw_output_observer_identity(), Some(1));
}
