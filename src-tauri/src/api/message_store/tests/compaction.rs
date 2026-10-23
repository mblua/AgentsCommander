use super::*;

#[test]
fn compaction_rejects_any_tombstone_mismatch() {
    let store = store();
    let (id, actuating_at) = enqueue_claim_actuating(&store, 0);
    let terminal_at = actuating_at + chrono::Duration::seconds(1);
    store
        .terminalize_pty_input(
            &id,
            crate::phone::types::PtyInputPublicStatus::Injected,
            None,
            terminal_at,
        )
        .unwrap();
    store
        .conn
        .lock()
        .unwrap()
        .execute(
            "UPDATE pty_input_tombstones SET target_fqn='tampered' WHERE injection_id=?1",
            [&id],
        )
        .unwrap();
    let error = store
        .compact_pty_terminal_before(terminal_at + chrono::Duration::days(8), 64)
        .unwrap_err();
    assert!(matches!(error, MessageStoreError::StoreCorrupt));
    let live_count: i64 = store
        .conn
        .lock()
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM pty_input_operations WHERE injection_id=?1",
            [&id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(live_count, 1);
}

#[test]
fn compaction_advances_past_sixty_four_locked_terminal_rows() {
    let store = store();
    let mut ids = Vec::new();
    let mut cutoff = Utc::now();
    for index in 0..65 {
        let (id, actuating_at) = enqueue_claim_actuating(&store, index / 16);
        let terminal_at = actuating_at + chrono::Duration::seconds(1);
        store
            .terminalize_pty_input(
                &id,
                crate::phone::types::PtyInputPublicStatus::Injected,
                None,
                terminal_at,
            )
            .unwrap();
        cutoff = cutoff.max(terminal_at + chrono::Duration::days(8));
        ids.push(id);
    }
    let stripe = |id: &str| {
        let digest = Sha256::digest(id.as_bytes());
        ((usize::from(digest[0]) << 4) | (usize::from(digest[1]) >> 4))
            % PTY_INPUT_OPERATION_LOCK_STRIPES
    };
    let tail = ids
        .iter()
        .find(|candidate| {
            ids.iter()
                .filter(|other| stripe(other) == stripe(candidate))
                .count()
                == 1
        })
        .expect("fixture includes a unique operation stripe")
        .clone();
    let latest_at = cutoff - chrono::Duration::days(8) + chrono::Duration::hours(1);
    cutoff = latest_at + chrono::Duration::days(8);
    let latest = crate::phone::types::canonical_pty_timestamp(latest_at);
    {
        let conn = store.conn.lock().unwrap();
        conn.execute(
            "UPDATE pty_input_operations SET terminal_at=?1,updated_at=?1 WHERE injection_id=?2",
            params![latest, tail],
        )
        .unwrap();
        conn.execute(
            "UPDATE pty_input_tombstones SET terminal_at=?1 WHERE injection_id=?2",
            params![latest, tail],
        )
        .unwrap();
    }
    let ordered = {
        let conn = store.conn.lock().unwrap();
        let mut statement = conn
            .prepare(
                "SELECT injection_id FROM pty_input_operations ORDER BY terminal_at,injection_id",
            )
            .unwrap();
        statement
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    };
    assert_eq!(ordered[64], tail);
    let locks = ordered[..64]
        .iter()
        .filter_map(|id| store.try_operation_lock(id).unwrap())
        .collect::<Vec<_>>();

    assert_eq!(store.compact_pty_terminal_before(cutoff, 64).unwrap(), 0);
    assert_eq!(
        store.compact_pty_terminal_before(cutoff, 64).unwrap(),
        1,
        "a fair compaction cursor must reach the independent 65th row"
    );
    drop(locks);
    assert_eq!(
        store.compact_pty_terminal_before(cutoff, 64).unwrap(),
        64,
        "released prefix rows are revisited immediately after cursor wrap"
    );
}

#[test]
fn maintenance_compaction_advances_past_sixty_four_locked_terminal_rows() {
    let store = store();
    let mut ids = Vec::new();
    for index in 0..65 {
        let (id, actuating_at) = enqueue_claim_actuating(&store, index / 16);
        store
            .terminalize_pty_input(
                &id,
                crate::phone::types::PtyInputPublicStatus::Injected,
                None,
                actuating_at + chrono::Duration::seconds(1),
            )
            .unwrap();
        ids.push(id);
    }
    let stripe = |id: &str| {
        let digest = Sha256::digest(id.as_bytes());
        ((usize::from(digest[0]) << 4) | (usize::from(digest[1]) >> 4))
            % PTY_INPUT_OPERATION_LOCK_STRIPES
    };
    let tail = ids
        .iter()
        .find(|candidate| {
            ids.iter()
                .filter(|other| stripe(other) == stripe(candidate))
                .count()
                == 1
        })
        .unwrap()
        .clone();
    let base_at = Utc::now() + chrono::Duration::seconds(1);
    let old = crate::phone::types::canonical_pty_timestamp(base_at);
    let tail_terminal_at = base_at + chrono::Duration::milliseconds(1);
    let tail_at = crate::phone::types::canonical_pty_timestamp(tail_terminal_at);
    let maintenance_at = tail_terminal_at + chrono::Duration::days(8);
    {
        let conn = store.conn.lock().unwrap();
        conn.execute(
            "UPDATE pty_input_operations SET terminal_at=?1,updated_at=?1",
            [&old],
        )
        .unwrap();
        conn.execute("UPDATE pty_input_tombstones SET terminal_at=?1", [&old])
            .unwrap();
        conn.execute(
            "UPDATE pty_input_operations SET terminal_at=?1,updated_at=?1 WHERE injection_id=?2",
            params![tail_at, tail],
        )
        .unwrap();
        conn.execute(
            "UPDATE pty_input_tombstones SET terminal_at=?1 WHERE injection_id=?2",
            params![tail_at, tail],
        )
        .unwrap();
    }
    let ordered = {
        let conn = store.conn.lock().unwrap();
        let mut statement = conn
            .prepare(
                "SELECT injection_id FROM pty_input_operations ORDER BY terminal_at,injection_id",
            )
            .unwrap();
        statement
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    };
    assert_eq!(ordered[64], tail);
    let locks = ordered[..64]
        .iter()
        .filter_map(|id| store.try_operation_lock(id).unwrap())
        .collect::<Vec<_>>();

    assert_eq!(
        store
            .compact_pty_terminal_maintenance(maintenance_at, 64)
            .unwrap(),
        0
    );
    assert_eq!(
        store
            .compact_pty_terminal_maintenance(maintenance_at, 64)
            .unwrap(),
        1,
        "runtime maintenance must reach the independent 65th row"
    );
    drop(locks);
    assert_eq!(
        store
            .compact_pty_terminal_maintenance(maintenance_at, 64)
            .unwrap(),
        64,
        "released maintenance prefix rows are revisited immediately after cursor wrap"
    );
}

#[test]
fn compaction_is_bounded_to_sixty_four_rows_per_call() {
    let store = store();
    let mut latest_terminal = Utc::now();
    for index in 0..65 {
        let (id, actuating_at) = enqueue_claim_actuating(&store, index / 16);
        let terminal_at = actuating_at + chrono::Duration::seconds(1);
        store
            .terminalize_pty_input(
                &id,
                crate::phone::types::PtyInputPublicStatus::Injected,
                None,
                terminal_at,
            )
            .unwrap();
        latest_terminal = latest_terminal.max(terminal_at);
    }
    let cutoff = latest_terminal + chrono::Duration::days(8);
    assert_eq!(
        store
            .compact_pty_terminal_before(cutoff, usize::MAX)
            .unwrap(),
        64
    );
    assert_eq!(
        store
            .compact_pty_terminal_before(cutoff, usize::MAX)
            .unwrap(),
        1
    );
}

#[tokio::test]
async fn heartbeat_finish_joins_and_prevents_late_renewal() {
    let store = Arc::new(store());
    let id = Uuid::new_v4().to_string();
    store
        .enqueue_pty_input(pty_request(&id, "exact text"))
        .unwrap();
    store
        .claim_pty_input(
            crate::phone::types::PtyInputSourcePlane::ContainerApi,
            Some(&id),
            "lease",
            Utc::now() + chrono::Duration::seconds(1),
        )
        .unwrap()
        .unwrap();
    let mut heartbeat = PreparationHeartbeatGuard::start_with_interval(
        Arc::clone(&store),
        id.clone(),
        "lease".to_string(),
        Utc::now() + chrono::Duration::minutes(5),
        Duration::from_millis(5),
    );
    tokio::time::sleep(Duration::from_millis(15)).await;
    assert!(heartbeat.finish().await);
    assert!(
        heartbeat.finish().await,
        "finishing an already joined heartbeat is safe"
    );
    let updated_before: String = store
        .conn
        .lock()
        .unwrap()
        .query_row(
            "SELECT updated_at FROM pty_input_operations WHERE injection_id=?1",
            [&id],
            |row| row.get(0),
        )
        .unwrap();
    tokio::time::sleep(Duration::from_millis(20)).await;
    let updated_after: String = store
        .conn
        .lock()
        .unwrap()
        .query_row(
            "SELECT updated_at FROM pty_input_operations WHERE injection_id=?1",
            [&id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(updated_before, updated_after);
    assert!(store
        .begin_pty_actuating(
            &id,
            "lease",
            &Uuid::new_v4().to_string(),
            "localProcess",
            Utc::now(),
        )
        .is_ok());
}

#[test]
fn backoff_is_bounded_exponential() {
    assert_eq!(retry_backoff_seconds(1), 5);
    assert_eq!(retry_backoff_seconds(2), 10);
    assert_eq!(retry_backoff_seconds(6), 160);
    assert_eq!(retry_backoff_seconds(100), 160);
}

#[test]
fn test_release_delivery_lease_resets_status_queued_and_preserves_attempts() {
    let store = store();
    let res = store.enqueue(request("op-rel-1", "hello")).unwrap();
    let id = res.message_id;

    // Lease the message
    let leased = store
        .lease_due(Utc::now(), 10, Duration::from_secs(60), "worker-1")
        .unwrap();
    assert_eq!(leased.len(), 1);
    assert_eq!(leased[0].message_id, id);

    // Fail once so attempt count is > 0
    let status = store
        .mark_delivery_failed(&id, "transient error", Utc::now(), 5)
        .unwrap();
    assert_eq!(status, STATUS_RETRY);

    // Lease again
    let leased = store
        .lease_due(
            Utc::now() + chrono::Duration::seconds(10),
            10,
            Duration::from_secs(60),
            "worker-1",
        )
        .unwrap();
    assert_eq!(leased.len(), 1);
    assert_eq!(leased[0].message_id, id);
    assert_eq!(leased[0].attempt, 1);

    // Release delivery lease (menu guard deferred)
    let now = Utc::now();
    store
        .release_delivery_lease(&id, "session blocked by interactive menu", now)
        .unwrap();

    // Verify status and attempt in DB
    let (status, attempt, lease_owner, lease_until, last_error): (
        String,
        i64,
        Option<String>,
        Option<String>,
        Option<String>,
    ) = store
        .conn
        .lock()
        .unwrap()
        .query_row(
            "SELECT status, attempt, lease_owner, lease_until, last_error FROM messages WHERE \
                message_id = ?1",
            [&id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )
        .unwrap();

    assert_eq!(status, STATUS_QUEUED);
    assert_eq!(attempt, 1); // Preserved!
    assert!(lease_owner.is_none());
    assert!(lease_until.is_none());
    assert_eq!(
        last_error,
        Some("session blocked by interactive menu".to_string())
    );

    // Check audit record
    let audit_status: String = store
        .conn
        .lock()
        .unwrap()
        .query_row(
            "SELECT status FROM message_audit WHERE message_id = ?1 ORDER BY at DESC LIMIT 1",
            [&id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(audit_status, "lease-released-deferred");
}

#[test]
fn test_retry_pty_input_menu_guard_blocked_persisted() {
    let store = store();
    let id = Uuid::new_v4().to_string();
    store.enqueue_pty_input(pty_request(&id, "hello")).unwrap();

    // Lease operation
    store
        .claim_pty_input(
            crate::phone::types::PtyInputSourcePlane::ContainerApi,
            Some(&id),
            "lease-1",
            Utc::now(),
        )
        .unwrap();

    // Retry with MenuGuardBlocked
    store
        .retry_pty_input(
            &id,
            "lease-1",
            crate::phone::types::PtyInputReasonCode::MenuGuardBlocked,
            Utc::now(),
        )
        .unwrap();

    let (status, reason_code, reason_detail): (String, Option<String>, Option<String>) = store
        .conn
        .lock()
        .unwrap()
        .query_row(
            "SELECT status, reason_code, reason_detail FROM pty_input_operations WHERE \
                injection_id = ?1",
            [&id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();

    assert_eq!(status, "retry");
    assert_eq!(reason_code, Some("menu_guard_blocked".to_string()));
    assert_eq!(
        reason_detail,
        Some("The target session is blocked by an interactive menu.".to_string())
    );
}
