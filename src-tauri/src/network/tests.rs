use super::*;

#[tokio::test]
async fn permit_is_released_on_drop() {
    let network = OutboundNetwork::new_for_tests(1);
    let permit = network.acquire("test").await.unwrap();
    assert_eq!(network.available_permits_for_tests(), 0);
    drop(permit);
    assert_eq!(network.available_permits_for_tests(), 1);
}

#[test]
fn backoff_grows_and_resets_within_cap() {
    let mut backoff = NetworkBackoff::new(Duration::from_millis(100), Duration::from_secs(2), 42);
    let first = backoff.next_delay();
    let second = backoff.next_delay();
    let third = backoff.next_delay();
    assert!(first >= Duration::from_millis(100));
    assert!(second >= first);
    assert!(third >= second);
    assert!(third <= Duration::from_secs(2));

    backoff.reset();
    let reset = backoff.next_delay();
    assert_eq!(reset, first);
}

#[test]
fn backoff_salt_desynchronizes_same_attempts() {
    let mut first = NetworkBackoff::new(
        Duration::from_secs(3),
        Duration::from_secs(60),
        NetworkBackoff::salt_from_str("session-a"),
    );
    let mut second = NetworkBackoff::new(
        Duration::from_secs(3),
        Duration::from_secs(60),
        NetworkBackoff::salt_from_str("session-b"),
    );
    let first_delay = first.next_delay();
    let second_delay = second.next_delay();

    assert_ne!(first_delay, second_delay);
    assert!(first_delay >= Duration::from_secs(3));
    assert!(second_delay >= Duration::from_secs(3));
    assert!(first_delay <= Duration::from_secs(60));
    assert!(second_delay <= Duration::from_secs(60));
}
