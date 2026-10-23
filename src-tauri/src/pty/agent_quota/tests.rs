use super::test_support::QuotaHarness;
use super::*;

const AGENT: &str = "claude";
const PATTERN: &str = r"Weekly (\d{1,3})% used";

fn spec(pattern: &str) -> SourceSpec {
    SourceSpec::ScreenRegex {
        pattern: pattern.to_string(),
    }
}

fn rows(percent: u8) -> ScreenRowsRead {
    ScreenRowsRead::Rows(vec![format!("Weekly {percent}% used")])
}

fn payload(agent_id: &str, percent: Option<u8>) -> AgentQuotaPayload {
    AgentQuotaPayload {
        agent_id: agent_id.to_string(),
        weekly_used_percent: percent,
    }
}

#[tokio::test]
async fn e2992_p51_unavailable_mid_tick_retirement() {
    let (h, id) = configured();
    h.sources.unavailable.store(true, Ordering::Relaxed);
    h.rows.set_liveness(id, ContextSessionLiveness::SessionOver);
    let (entered, handshake) = tokio::sync::oneshot::channel();
    let entered = std::sync::Mutex::new(Some(entered));
    let (release, resume) = std::sync::mpsc::sync_channel(1);
    let resume = std::sync::Mutex::new(resume);
    *h.rows.on_liveness.lock().unwrap() = Some(Arc::new(move |_| {
        entered.lock().unwrap().take().unwrap().send(()).unwrap();
        resume
            .lock()
            .unwrap()
            .recv_timeout(Duration::from_secs(2))
            .unwrap();
    }));
    let engine = Arc::clone(&h.engine);
    let tick = tokio::task::spawn_blocking(move || futures::executor::block_on(engine.tick()));
    tokio::time::timeout(Duration::from_secs(2), handshake)
        .await
        .unwrap()
        .unwrap();
    h.engine.retire_session(id);
    let replacement = Uuid::new_v4();
    h.engine.register_session(replacement, AGENT.to_string());
    release.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(2), tick)
        .await
        .unwrap()
        .unwrap();
    *h.rows.on_liveness.lock().unwrap() = None;
    assert!(!h.engine.is_session_registered(id));
    assert!(h.engine.is_session_registered(replacement));
    assert!(h.rows.rows_calls().is_empty());
    assert!(h.sink.emitted().is_empty());

    let shutdown = crate::shutdown::ShutdownSignal::new();
    let mut completed_ticks = 0;
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            tokio::select! {
                biased;
                _ = shutdown.token().cancelled() => break,
                _ = h.engine.tick() => {
                    completed_ticks += 1;
                    if completed_ticks == 3 { shutdown.trigger(); }
                }
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(completed_ticks, 3);
}

#[tokio::test]
async fn e2992_p51_quota_unavailable_preserves() {
    let (h, id) = configured();
    h.sources.configure_on(AGENT, "shared", spec(PATTERN));
    h.sources.configure_on("peer", "shared", spec(PATTERN));
    h.rows.push(id, rows(42));
    h.engine.tick().await;
    let published = h.engine.published();
    let cursor = h.engine.sample_cursor.load(Ordering::Relaxed);
    h.sources.unavailable.store(true, Ordering::Relaxed);
    for live in [
        ContextSessionLiveness::Live,
        ContextSessionLiveness::Unavailable,
    ] {
        h.rows.set_liveness(id, live);
        tokio::time::timeout(Duration::from_secs(1), h.engine.tick())
            .await
            .unwrap();
    }
    assert_eq!(h.engine.published(), published);
    assert_eq!(
        h.engine.account_readings.lock().unwrap().get("shared"),
        Some(&42)
    );
    assert_eq!(h.engine.resolved.lock().unwrap().len(), 1);
    assert_eq!(h.engine.resolve_count(), 1);
    assert_eq!(h.engine.sample_cursor.load(Ordering::Relaxed), cursor);
    assert_eq!(h.rows.rows_calls(), vec![id]);
    assert_eq!(h.sink.emitted().len(), 2);
    h.rows.set_liveness(id, ContextSessionLiveness::SessionOver);
    h.engine.tick().await;
    assert!(!h.engine.is_session_registered(id));
    assert_eq!(
        h.engine.account_readings.lock().unwrap().get("shared"),
        Some(&42)
    );
    assert_eq!(h.engine.published(), published);
    h.sources.disable(AGENT);
    h.sources.disable("peer");
    h.sources.unavailable.store(false, Ordering::Relaxed);
    h.engine.tick().await;
    h.engine.tick().await;
    assert!(h.engine.published().is_empty());
    assert!(h.engine.account_readings.lock().unwrap().is_empty());
    assert!(h.engine.resolved.lock().unwrap().is_empty());
    assert_eq!(h.sink.emitted().len(), 4);
    assert_eq!(
        h.sink.emitted()[2..],
        [payload(AGENT, None), payload("peer", None)]
    );
    h.sources.configure(AGENT, spec(PATTERN));
    h.engine.register_session(id, AGENT.to_string());
    h.rows.push(id, rows(70));
    h.engine.tick().await;
    assert_eq!(h.engine.published().get(AGENT), Some(&Some(70)));
}

#[tokio::test]
async fn e2992_p51_unavailable_progress_and_retirement() {
    let (h, id) = configured();
    h.sources.unavailable.store(true, Ordering::Relaxed);
    for _ in 0..3 {
        tokio::time::timeout(Duration::from_secs(1), h.engine.tick())
            .await
            .unwrap();
    }
    h.rows.set_liveness(id, ContextSessionLiveness::SessionOver);
    h.engine.tick().await;
    h.engine.retire_session(id);
    h.engine.tick().await;
    assert!(!h.engine.is_session_registered(id));
    assert!(h.rows.rows_calls().is_empty());
    assert!(h.sink.emitted().is_empty());
    h.engine.register_session(id, AGENT.to_string());
    h.rows.set_liveness(id, ContextSessionLiveness::Live);
    let shutdown = crate::shutdown::ShutdownSignal::new();
    let worker = h
        .engine
        .start_with_interval(shutdown.clone(), Duration::from_millis(1));
    shutdown.trigger();
    tokio::time::timeout(
        Duration::from_secs(5),
        tokio::task::spawn_blocking(move || worker.join().unwrap()),
    )
    .await
    .unwrap()
    .unwrap();
}

/// A harness with AGENT configured and one registered session.
fn configured() -> (QuotaHarness, Uuid) {
    let h = QuotaHarness::new();
    h.sources.configure(AGENT, spec(PATTERN));
    let id = Uuid::new_v4();
    h.engine.register_session(id, AGENT.to_string());
    (h, id)
}

#[tokio::test]
async fn an_empty_registration_set_with_nothing_published_never_calls_sources() {
    let h = QuotaHarness::new();
    h.sources.configure(AGENT, spec(PATTERN));

    h.engine.tick().await;

    assert_eq!(h.sources.calls(), 0);
    assert!(h.rows.rows_calls().is_empty());
}

#[tokio::test]
async fn an_empty_registration_set_still_reconciles_what_is_published() {
    let (h, id) = configured();
    h.rows.push(id, rows(42));
    h.engine.tick().await;
    h.engine.retire_session(id);
    h.sources.disable(AGENT);
    let calls = h.sources.calls();

    h.engine.tick().await;

    assert_eq!(
        h.sources.calls(),
        calls + 1,
        "published state needs sources()"
    );
    assert_eq!(
        h.sink.emitted(),
        vec![payload(AGENT, Some(42)), payload(AGENT, None)]
    );
    assert!(h.engine.published().is_empty());
}

#[tokio::test]
async fn an_unconfigured_agent_never_reads_rows_and_never_emits() {
    let h = QuotaHarness::new();
    let id = Uuid::new_v4();
    h.engine.register_session(id, AGENT.to_string());
    h.rows.set_liveness(id, ContextSessionLiveness::Live);

    h.engine.tick().await;
    h.engine.tick().await;

    assert!(
        h.rows.rows_calls().is_empty(),
        "no source must mean no rows read"
    );
    assert_eq!(h.rows.liveness_calls(), vec![id, id]);
    assert!(h.sink.emitted().is_empty());
    assert!(h.engine.published().is_empty());
}

#[tokio::test]
async fn a_disabled_entry_never_reads_rows_and_never_emits() {
    let (h, id) = configured();
    h.sources.disable(AGENT);
    h.rows.push(id, rows(40));

    h.engine.tick().await;

    assert!(h.rows.rows_calls().is_empty());
    assert_eq!(h.rows.liveness_calls(), vec![id]);
    assert!(h.sink.emitted().is_empty());
}

#[tokio::test]
async fn an_uncompilable_pattern_resolves_once_and_not_once_per_tick() {
    let h = QuotaHarness::new();
    h.sources.configure(AGENT, spec(r"(unclosed"));
    let id = Uuid::new_v4();
    h.engine.register_session(id, AGENT.to_string());

    for _ in 0..3 {
        h.engine.tick().await;
    }

    assert_eq!(h.engine.resolve_count(), 1);
    assert!(
        h.rows.rows_calls().is_empty(),
        "a failed source takes the liveness path"
    );
    assert_eq!(h.rows.liveness_calls().len(), 3);
    assert!(h.sink.emitted().is_empty());
}

#[tokio::test]
async fn a_changed_pattern_resolves_again() {
    let (h, _id) = configured();
    h.engine.tick().await;
    h.engine.tick().await;
    assert_eq!(h.engine.resolve_count(), 1, "unchanged: no second resolve");

    h.sources.configure(AGENT, spec(r"Week (\d{1,3})%"));
    h.engine.tick().await;

    assert_eq!(h.engine.resolve_count(), 2);
}

#[tokio::test]
async fn a_reading_emits_once_and_an_unchanged_reading_emits_nothing() {
    let (h, id) = configured();
    h.rows.push(id, rows(42));
    h.rows.push(id, rows(42));

    h.engine.tick().await;
    h.engine.tick().await;

    assert_eq!(h.sink.emitted(), vec![payload(AGENT, Some(42))]);
    assert_eq!(h.engine.published().get(AGENT), Some(&Some(42)));
}

#[tokio::test]
async fn a_changed_reading_emits_the_new_value() {
    let (h, id) = configured();
    h.rows.push(id, rows(42));
    h.rows.push(id, rows(43));

    h.engine.tick().await;
    h.engine.tick().await;

    assert_eq!(
        h.sink.emitted(),
        vec![payload(AGENT, Some(42)), payload(AGENT, Some(43))]
    );
}

#[tokio::test]
async fn a_reading_that_becomes_unavailable_keeps_the_last_account_value() {
    let (h, id) = configured();
    h.rows.push(id, rows(42));
    h.rows.push(id, ScreenRowsRead::Unavailable);
    h.rows
        .push(id, ScreenRowsRead::Rows(vec!["no match".to_string()]));
    h.rows.push(id, ScreenRowsRead::Unavailable);

    for _ in 0..4 {
        h.engine.tick().await;
    }

    assert_eq!(h.sink.emitted(), vec![payload(AGENT, Some(42))]);
    assert_eq!(h.engine.published().get(AGENT), Some(&Some(42)));
    assert!(h.engine.is_session_registered(id));
}

#[tokio::test]
async fn zero_percent_and_one_hundred_percent_are_real_readings_and_are_emitted() {
    let (h, id) = configured();
    h.rows.push(id, rows(0));
    h.rows.push(id, rows(100));
    h.rows.push(id, ScreenRowsRead::Unavailable);

    for _ in 0..3 {
        h.engine.tick().await;
    }

    assert_eq!(
        h.sink.emitted(),
        vec![payload(AGENT, Some(0)), payload(AGENT, Some(100))]
    );
}

#[tokio::test]
async fn session_over_retires_the_session_and_keeps_the_account_value() {
    // Live reading, then over: retired, and the account value stays published.
    let (h, id) = configured();
    h.rows.push(id, rows(42));
    h.rows.push(id, ScreenRowsRead::SessionOver);
    h.engine.tick().await;
    h.engine.tick().await;
    h.engine.tick().await;
    assert_eq!(h.sink.emitted(), vec![payload(AGENT, Some(42))]);
    assert!(!h.engine.is_session_registered(id));
    assert_eq!(h.engine.published().get(AGENT), Some(&Some(42)));

    // No reading ever, then over (liveness path): retired, nothing emitted.
    let h = QuotaHarness::new();
    let id = Uuid::new_v4();
    h.engine.register_session(id, AGENT.to_string());
    h.rows.set_liveness(id, ContextSessionLiveness::SessionOver);
    h.engine.tick().await;
    assert!(h.sink.emitted().is_empty());
    assert!(!h.engine.is_session_registered(id));
}

#[tokio::test]
async fn rows_unavailable_keeps_the_registration_for_the_next_tick() {
    let (h, id) = configured();
    h.rows.push(id, ScreenRowsRead::Unavailable);
    h.rows.push(id, rows(55));

    h.engine.tick().await;
    assert!(h.engine.is_session_registered(id));
    h.engine.tick().await;

    assert_eq!(h.rows.rows_calls(), vec![id, id]);
    assert_eq!(h.sink.emitted(), vec![payload(AGENT, Some(55))]);
}

#[tokio::test]
async fn a_session_retired_mid_tick_still_contributes_its_sample() {
    let (h, id) = configured();
    h.rows.push(id, rows(42));
    let engine = Arc::downgrade(&h.engine);
    h.rows.on_rows_read(move |id| {
        engine.upgrade().expect("engine alive").retire_session(id);
    });

    h.engine.tick().await;

    assert_eq!(h.sink.emitted(), vec![payload(AGENT, Some(42))]);
    assert!(!h.engine.is_session_registered(id));
}

#[tokio::test]
async fn the_sample_order_rotates_between_ticks() {
    let h = QuotaHarness::new();
    h.sources.configure(AGENT, spec(PATTERN));
    let mut ids: Vec<Uuid> = (0..3).map(|_| Uuid::new_v4()).collect();
    for id in &ids {
        h.engine.register_session(*id, AGENT.to_string());
    }
    ids.sort();

    h.engine.tick().await;
    h.engine.tick().await;

    let calls = h.rows.rows_calls();
    assert_eq!(&calls[..3], &[ids[0], ids[1], ids[2]]);
    assert_eq!(&calls[3..], &[ids[1], ids[2], ids[0]]);
}

#[test]
fn the_payload_serializes_unknown_as_an_explicit_null_key() {
    let value = serde_json::to_value(payload("a1", None)).unwrap();
    let object = value.as_object().expect("an object");
    assert!(
        object.contains_key("weeklyUsedPercent"),
        "the key is present"
    );
    assert_eq!(value["weeklyUsedPercent"], serde_json::Value::Null);

    let value = serde_json::to_value(payload("a1", Some(0))).unwrap();
    assert_eq!(value["weeklyUsedPercent"], serde_json::json!(0));
}

#[tokio::test]
async fn the_test_support_harness_drives_a_reading_end_to_end() {
    let h = QuotaHarness::new();
    h.sources.configure(AGENT, spec(PATTERN));
    let id = Uuid::new_v4();
    h.engine.register_session(id, AGENT.to_string());
    h.rows.push(id, rows(73));

    h.engine.tick().await;

    assert_eq!(h.sink.emitted(), vec![payload(AGENT, Some(73))]);
    assert_eq!(h.engine.published().get(AGENT), Some(&Some(73)));
}

#[test]
fn a_pending_provider_does_not_outlive_shutdown() {
    let h = QuotaHarness::new();
    h.sources.configure(AGENT, spec(PATTERN));
    h.sources.hang();
    h.engine.register_session(Uuid::new_v4(), AGENT.to_string());
    let shutdown = crate::shutdown::ShutdownSignal::new();

    let handle = h
        .engine
        .start_with_interval(shutdown.clone(), Duration::from_millis(5));

    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while h.sources.calls() < 1 {
        assert!(
            std::time::Instant::now() < deadline,
            "no tick reached sources()"
        );
        std::thread::sleep(Duration::from_millis(1));
    }

    shutdown.trigger();
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while !handle.is_finished() && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(1));
    }

    assert!(
        handle.is_finished(),
        "the worker outlived shutdown with a tick in flight"
    );
    assert!(h.sink.emitted().is_empty());
}

#[tokio::test]
async fn the_resolution_cache_evicts_agents_that_are_no_longer_configured() {
    let h = QuotaHarness::new();
    h.sources.configure("a", spec(PATTERN));
    h.sources.configure("b", spec(r"Week (\d{1,3})%"));
    let a = Uuid::new_v4();
    let b = Uuid::new_v4();
    h.engine.register_session(a, "a".to_string());
    h.engine.register_session(b, "b".to_string());

    h.engine.tick().await;
    assert_eq!(h.engine.resolve_count(), 2);

    h.engine.retire_session(b);
    h.engine.tick().await;
    assert_eq!(
        h.engine.resolve_count(),
        2,
        "B is still configured: closing its terminal keeps it cached"
    );
    // Only a NEW resolve of B can prove the cache kept it across a tick with no B session.
    let b2 = Uuid::new_v4();
    h.engine.register_session(b2, "b".to_string());
    h.engine.tick().await;
    assert_eq!(
        h.engine.resolve_count(),
        2,
        "a new B session reuses the cached pattern: no terminal close evicts it"
    );
    h.engine.retire_session(b2);

    h.sources.disable("b");
    h.engine.tick().await;
    h.sources.configure("b", spec(r"Week (\d{1,3})%"));
    h.engine.register_session(b, "b".to_string());
    h.engine.tick().await;
    assert_eq!(h.engine.resolve_count(), 3, "B was evicted, not reused");

    h.engine.retire_session(a);
    h.engine.retire_session(b);
    let calls = h.sources.calls();
    h.engine.tick().await;
    assert_eq!(h.sources.calls(), calls, "the idle path skips sources()");

    h.engine.register_session(a, "a".to_string());
    h.engine.tick().await;
    assert_eq!(h.engine.resolve_count(), 4, "the idle tick cleared the map");
}

#[tokio::test]
async fn two_agents_on_one_command_are_both_filled_from_one_session() {
    let h = QuotaHarness::new();
    h.sources.configure_on("a1", "claude", spec(PATTERN));
    h.sources.configure_on("a2", "claude", spec(PATTERN));
    let id = Uuid::new_v4();
    h.engine.register_session(id, "a1".to_string());
    h.rows.push(id, rows(42));

    h.engine.tick().await;

    assert_eq!(
        h.sink.emitted(),
        vec![payload("a1", Some(42)), payload("a2", Some(42))]
    );
}

#[tokio::test]
async fn two_agents_on_different_commands_keep_separate_readings() {
    let h = QuotaHarness::new();
    h.sources.configure_on("a1", "claude", spec(PATTERN));
    h.sources.configure_on("a2", "/a/claude", spec(PATTERN));
    let s1 = Uuid::new_v4();
    let s2 = Uuid::new_v4();
    h.engine.register_session(s1, "a1".to_string());
    h.engine.register_session(s2, "a2".to_string());
    h.rows.push(s1, rows(30));
    h.rows.push(s2, rows(70));

    h.engine.tick().await;
    h.engine.tick().await;

    assert_eq!(
        h.sink.emitted(),
        vec![payload("a1", Some(30)), payload("a2", Some(70))]
    );
    let published = h.engine.published();
    assert_eq!(published.get("a1"), Some(&Some(30)));
    assert_eq!(published.get("a2"), Some(&Some(70)));
}

#[tokio::test]
async fn an_agent_with_no_session_is_filled_and_stays_filled_after_the_last_session_retires() {
    let (h, id) = configured();
    h.rows.push(id, rows(42));
    h.engine.tick().await;

    h.engine.retire_session(id);
    h.engine.tick().await;

    assert_eq!(h.sink.emitted(), vec![payload(AGENT, Some(42))]);
    assert_eq!(h.engine.published().get(AGENT), Some(&Some(42)));
}

#[tokio::test]
async fn an_agent_that_leaves_the_source_map_is_published_as_null_once() {
    let (h, id) = configured();
    h.rows.push(id, rows(42));
    h.engine.tick().await;

    h.sources.disable(AGENT);
    h.engine.tick().await;
    h.engine.tick().await;

    assert_eq!(
        h.sink.emitted(),
        vec![payload(AGENT, Some(42)), payload(AGENT, None)]
    );
    assert!(h.engine.published().is_empty());
}

#[tokio::test]
async fn nothing_is_emitted_twice_for_an_unchanged_account_value() {
    let (h, id) = configured();
    for _ in 0..3 {
        h.rows.push(id, rows(42));
    }

    for _ in 0..3 {
        h.engine.tick().await;
    }

    assert_eq!(h.sink.emitted(), vec![payload(AGENT, Some(42))]);
}

#[tokio::test]
async fn emission_order_is_sorted_by_agent_id() {
    let h = QuotaHarness::new();
    h.sources.configure_on("b", "claude", spec(PATTERN));
    h.sources.configure_on("a", "claude", spec(PATTERN));
    let id = Uuid::new_v4();
    h.engine.register_session(id, "b".to_string());
    h.rows.push(id, rows(42));

    h.engine.tick().await;

    assert_eq!(
        h.sink.emitted(),
        vec![payload("a", Some(42)), payload("b", Some(42))]
    );
}

#[tokio::test]
async fn a_configured_agent_with_no_reading_emits_nothing_on_the_first_tick() {
    let (h, id) = configured();
    h.rows
        .push(id, ScreenRowsRead::Rows(vec!["no match".to_string()]));

    h.engine.tick().await;

    assert!(h.sink.emitted().is_empty());
    assert!(h.engine.published().is_empty());
}

#[tokio::test]
async fn a_disabled_agent_is_still_filled_from_its_enabled_sibling_on_the_same_command() {
    // What `lib.rs` hands over when `a1` is disabled and `a2` is enabled on one command.
    let h = QuotaHarness::new();
    h.sources.configure_on("a1", "claude", spec(PATTERN));
    h.sources.configure_on("a2", "claude", spec(PATTERN));
    let id = Uuid::new_v4();
    h.engine.register_session(id, "a1".to_string());
    h.rows.push(id, rows(42));

    h.engine.tick().await;

    assert_eq!(
        h.sink.emitted(),
        vec![payload("a1", Some(42)), payload("a2", Some(42))]
    );
}
