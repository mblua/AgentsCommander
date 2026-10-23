use super::*;
use tokio::sync::mpsc;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

// ── #2232 phase 4 section 5: the demand registry ──────────────────────

/// A reader whose task parks forever, so cancellation is observable and
/// nothing races the assertions.
fn test_reader(cancel: CancellationToken) -> ReaderTask {
    let (dest, _dest_rx) = watch::channel(None);
    let task_cancel = cancel.clone();
    let task = tokio::spawn(async move { task_cancel.cancelled().await });
    ReaderTask {
        cancel,
        tasks: vec![task],
        dest: ReaderDestSender::Claude(dest),
        frontier: Arc::new(Mutex::new(None)),
    }
}

fn test_manager(captures: &Arc<CaptureRegistry>) -> TelegramBridgeManager {
    TelegramBridgeManager::with_captures(Arc::new(Mutex::new(HashMap::new())), Arc::clone(captures))
}

/// Install a reader for `session_id` held by `consumer`, with its capture
/// endpoints open, exactly as the supervisor does.
fn install(
    manager: &mut TelegramBridgeManager,
    session_id: Uuid,
    consumer: ReaderConsumer,
) -> CancellationToken {
    let _ = manager.captures().open(&session_id.to_string());
    let cancel = CancellationToken::new();
    let reader_id = manager.next_reader_id();
    manager.reader_install(
        session_id,
        ReaderEntry::new(reader_id, test_reader(cancel.clone())),
        consumer,
    );
    cancel
}

/// #2516 T5: every release moves its own consumer's epoch, including one
/// that removed nothing, and `reader_release_all` moves both without
/// clearing the map.
#[tokio::test]
async fn demand_epochs_move_on_every_release() {
    let captures = Arc::new(CaptureRegistry::new());
    let mut manager = test_manager(&captures);
    let session_id = Uuid::new_v4();
    let bot = ReaderConsumer::Bot;
    let room = ReaderConsumer::Room;
    assert_eq!(manager.demand_epoch(session_id, bot), DemandEpoch::ZERO);
    assert_eq!(manager.demand_epoch(session_id, room), DemandEpoch::ZERO);

    // No reader installed: nothing is removed, the Room epoch still moves.
    assert!(manager.reader_demand_release(session_id, room).is_none());
    let room_after_first = manager.demand_epoch(session_id, room);
    assert_ne!(room_after_first, DemandEpoch::ZERO);
    assert_eq!(manager.demand_epoch(session_id, bot), DemandEpoch::ZERO);

    // A reader holding only Room: releasing the absent Bot moves Bot only.
    let _cancel = install(&mut manager, session_id, room);
    assert!(manager.reader_demand_release(session_id, bot).is_none());
    let bot_after_release = manager.demand_epoch(session_id, bot);
    assert_ne!(bot_after_release, DemandEpoch::ZERO);
    assert_eq!(manager.demand_epoch(session_id, room), room_after_first);

    // Release-all moves both and keeps the entries.
    if let Some(shutdown) = manager.reader_release_all(session_id) {
        shutdown.abort_now();
    }
    assert_ne!(manager.demand_epoch(session_id, bot), bot_after_release);
    assert_ne!(manager.demand_epoch(session_id, bot), DemandEpoch::ZERO);
    assert_ne!(manager.demand_epoch(session_id, room), room_after_first);
    assert_ne!(manager.demand_epoch(session_id, room), DemandEpoch::ZERO);
}

/// Test 5: adding a Room demand to a session that already has a Bot demand
/// does **not** restart the reader — its identity continues unbroken and
/// the task is never cancelled, which is what keeps `reader_seq` running.
#[tokio::test]
async fn adding_a_room_demand_over_a_bot_demand_never_restarts_the_reader() {
    let captures = Arc::new(CaptureRegistry::new());
    let mut manager = test_manager(&captures);
    let session_id = Uuid::new_v4();
    let cancel = install(&mut manager, session_id, ReaderConsumer::Bot);
    let before = manager.reader_id(session_id).expect("reader installed");

    assert!(manager.reader_demand_add(session_id, ReaderConsumer::Room, None));

    assert_eq!(
        manager.reader_id(session_id),
        Some(before),
        "the reader identity must continue unbroken"
    );
    assert!(
        !cancel.is_cancelled(),
        "the reader task must not be stopped"
    );
    assert_eq!(
        manager.reader_demands(session_id),
        [ReaderConsumer::Bot, ReaderConsumer::Room]
            .into_iter()
            .collect::<BTreeSet<_>>()
    );
    assert!(captures.is_open(&session_id.to_string()));
}

/// Test 6: adding the **same** demand twice registers one demand, so
/// releasing it once stops the reader. Re-attaching a persisted bot and a
/// local auto-attach both land here (section 5).
#[tokio::test]
async fn a_repeated_bot_demand_is_one_demand_and_one_release_stops_the_reader() {
    let captures = Arc::new(CaptureRegistry::new());
    let mut manager = test_manager(&captures);
    let session_id = Uuid::new_v4();
    let cancel = install(&mut manager, session_id, ReaderConsumer::Bot);
    let reader_id = manager.reader_id(session_id);

    assert!(manager.reader_demand_add(session_id, ReaderConsumer::Bot, None));
    assert_eq!(
        manager.reader_demands(session_id),
        [ReaderConsumer::Bot].into_iter().collect::<BTreeSet<_>>(),
        "a repeated demand is still one demand"
    );
    assert_eq!(manager.reader_id(session_id), reader_id, "no restart");

    let shutdown = manager
        .reader_demand_release(session_id, ReaderConsumer::Bot)
        .expect("the only demand was released");
    assert!(cancel.is_cancelled());
    assert!(!manager.reader_is_running(session_id));
    shutdown.abort_now();
}

/// Test 7: releasing the Bot demand while a Room demand remains keeps the
/// reader running and stops Telegram sends — the destination goes to
/// `None`, which is what the watcher reads.
#[tokio::test]
async fn releasing_the_bot_demand_keeps_a_room_reader_and_stops_telegram() {
    let captures = Arc::new(CaptureRegistry::new());
    let mut manager = test_manager(&captures);
    let session_id = Uuid::new_v4();
    let cancel = install(&mut manager, session_id, ReaderConsumer::Room);
    let mut dest_rx = manager
        .readers
        .get(&session_id)
        .expect("reader installed")
        .dest
        .subscribe();
    assert!(manager.reader_demand_add(
        session_id,
        ReaderConsumer::Bot,
        Some(ReaderDest {
            token: "token".into(),
            chat_id: 42,
        })
    ));
    assert_eq!(
        dest_rx.borrow_and_update().as_ref().map(|d| d.chat_id),
        Some(42)
    );

    assert!(
        manager
            .reader_demand_release(session_id, ReaderConsumer::Bot)
            .is_none(),
        "a surviving room demand keeps the reader"
    );

    assert!(!cancel.is_cancelled());
    assert!(manager.reader_is_running(session_id));
    assert!(
        dest_rx.borrow_and_update().is_none(),
        "Telegram sends must stop when the bot demand goes"
    );
    assert!(captures.is_open(&session_id.to_string()));
}

/// Test 8: releasing the **last** demand cancels the reader, drops its
/// state and calls `CaptureRegistry::close`.
#[tokio::test]
async fn releasing_the_last_demand_cancels_the_reader_and_closes_the_capture() {
    let captures = Arc::new(CaptureRegistry::new());
    let mut manager = test_manager(&captures);
    let session_id = Uuid::new_v4();
    let cancel = install(&mut manager, session_id, ReaderConsumer::Room);
    assert!(captures.is_open(&session_id.to_string()));

    let shutdown = manager
        .reader_demand_release(session_id, ReaderConsumer::Room)
        .expect("the last demand returns a shutdown");

    assert!(cancel.is_cancelled());
    assert!(!manager.reader_is_running(session_id));
    assert!(
        !captures.is_open(&session_id.to_string()),
        "the registry entry must be dropped with the reader"
    );
    // The existing 2 s budget, awaited outside the state.
    shutdown.wait_or_abort().await;
}

/// Test 10: a rollback releases **only** the Bot demand; the Room demand
/// survives, so the reader keeps running.
#[tokio::test]
async fn a_rollback_releases_only_the_bot_demand() {
    let captures = Arc::new(CaptureRegistry::new());
    let mut manager = test_manager(&captures);
    let session_id = Uuid::new_v4();
    let cancel = install(&mut manager, session_id, ReaderConsumer::Room);
    assert!(manager.reader_demand_add(session_id, ReaderConsumer::Bot, None));

    assert!(manager
        .reader_demand_release(session_id, ReaderConsumer::Bot)
        .is_none());

    assert_eq!(
        manager.reader_demands(session_id),
        [ReaderConsumer::Room].into_iter().collect::<BTreeSet<_>>()
    );
    assert!(!cancel.is_cancelled());
    assert!(captures.is_open(&session_id.to_string()));
}

/// Test 12: the drain acknowledgement is awaited with the state released. A
/// deliberately slow acknowledgement does not block a second session's
/// operation, because the release hands the shutdown **back** rather than
/// awaiting it under the lock.
#[tokio::test]
async fn a_slow_drain_does_not_block_another_session() {
    let captures = Arc::new(CaptureRegistry::new());
    let manager = Arc::new(tokio::sync::Mutex::new(test_manager(&captures)));
    let slow = Uuid::new_v4();
    let other = Uuid::new_v4();
    {
        let mut guard = manager.lock().await;
        let _ = guard.captures().open(&slow.to_string());
        let reader_id = guard.next_reader_id();
        let cancel = CancellationToken::new();
        let (dest, _dest_rx) = watch::channel(None);
        // A task that ignores cancellation: the acknowledgement never comes.
        let task = tokio::spawn(async { std::future::pending::<()>().await });
        guard.reader_install(
            slow,
            ReaderEntry::new(
                reader_id,
                ReaderTask {
                    cancel,
                    tasks: vec![task],
                    dest: ReaderDestSender::Claude(dest),
                    frontier: Arc::new(Mutex::new(None)),
                },
            ),
            ReaderConsumer::Room,
        );
        install(&mut guard, other, ReaderConsumer::Room);
    }

    let shutdown = {
        let mut guard = manager.lock().await;
        guard
            .reader_demand_release(slow, ReaderConsumer::Room)
            .expect("last demand")
    };
    // The drain runs outside `TelegramBridgeState`.
    let drain = tokio::spawn(async move { shutdown.wait_or_abort().await });

    let started = Instant::now();
    let second = tokio::time::timeout(Duration::from_millis(250), async {
        let mut guard = manager.lock().await;
        guard.reader_demand_add(other, ReaderConsumer::Bot, None)
    })
    .await;
    assert_eq!(
        second,
        Ok(true),
        "the second session must not wait on the slow drain"
    );
    assert!(started.elapsed() < Duration::from_millis(250));
    drain.await.expect("drain task joins after its own budget");
}

#[tokio::test]
async fn cancel_all_drains_bridge_state_and_returns_shutdowns() {
    let output_senders: OutputSenderMap = Arc::new(Mutex::new(HashMap::new()));
    let mut manager = TelegramBridgeManager::new(Arc::clone(&output_senders));
    let session_id = Uuid::new_v4();
    let cancel = CancellationToken::new();
    let (tx, _rx) = mpsc::channel(1);
    let task = tokio::spawn(async {
        std::future::pending::<()>().await;
    });

    manager.bot_assignments.insert("bot-1".into(), session_id);
    output_senders
        .lock()
        .unwrap()
        .insert(session_id, tx.clone());
    manager.bridges.insert(
        session_id,
        BridgeHandle {
            info: BridgeInfo {
                bot_id: "bot-1".into(),
                bot_label: "Bot 1".into(),
                session_id: session_id.to_string(),
                status: BridgeStatus::Active,
                color: "#229ED9".into(),
            },
            cancel: cancel.clone(),
            output_sender: tx,
            tasks: vec![task],
        },
    );

    let shutdowns = manager.cancel_all();

    assert!(cancel.is_cancelled());
    assert!(manager.bridges.is_empty());
    assert!(manager.bot_assignments.is_empty());
    assert!(output_senders.lock().unwrap().is_empty());
    assert_eq!(shutdowns.len(), 1);
    for shutdown in shutdowns {
        shutdown.abort_now();
    }
}

/// Test 20: **idempotent Bot transition.** Re-adding the same Bot target
/// sends no watch update (so the watcher runs no preamble and discards no
/// pending buffer); changing the target sends exactly one; releasing a Bot
/// demand that is not held sends no `None`.
#[tokio::test]
async fn a_repeated_bot_transition_is_not_a_watch_update() {
    let captures = Arc::new(CaptureRegistry::new());
    let mut manager = test_manager(&captures);
    let session_id = Uuid::new_v4();
    let _cancel = install(&mut manager, session_id, ReaderConsumer::Room);
    let target = ReaderDest {
        token: "token".into(),
        chat_id: 7,
    };
    let other = ReaderDest {
        token: "token".into(),
        chat_id: 8,
    };
    let mut transitions = manager
        .readers
        .get(&session_id)
        .expect("reader installed")
        .dest
        .subscribe();

    // The first Bot demand is one transition.
    assert!(manager.reader_demand_add(session_id, ReaderConsumer::Bot, Some(target.clone())));
    assert!(transitions.has_changed().unwrap());
    assert_eq!(
        transitions.borrow_and_update().clone(),
        Some(target.clone())
    );

    // The same demand with the same destination is not a transition.
    assert!(manager.reader_demand_add(session_id, ReaderConsumer::Bot, Some(target)));
    assert!(
        !transitions.has_changed().unwrap(),
        "a repeated Bot demand must send no watch update"
    );

    // The same demand with a changed destination is exactly one transition.
    assert!(manager.reader_demand_add(session_id, ReaderConsumer::Bot, Some(other.clone())));
    assert!(transitions.has_changed().unwrap());
    assert_eq!(transitions.borrow_and_update().clone(), Some(other.clone()));
    assert!(
        !transitions.has_changed().unwrap(),
        "one update per destination change"
    );

    // Releasing the Bot demand sends `None` once.
    assert!(manager
        .reader_demand_release(session_id, ReaderConsumer::Bot)
        .is_none());
    assert!(transitions.has_changed().unwrap());
    assert_eq!(transitions.borrow_and_update().clone(), None);

    // Releasing it again is absent, so it sends no second `None`.
    assert!(manager
        .reader_demand_release(session_id, ReaderConsumer::Bot)
        .is_none());
    assert!(
        !transitions.has_changed().unwrap(),
        "an absent Bot release must send no `None`"
    );
    assert!(
        manager.reader_is_running(session_id),
        "the Room demand keeps it"
    );
}
