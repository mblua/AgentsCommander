use super::*;
use crate::api::message_store::{EnqueueRequest, DB_FILENAME, DEFAULT_CONTENT_TYPE};
use std::sync::{Arc, Mutex};

fn store() -> MessageStore {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join(DB_FILENAME);
    let store = MessageStore::open(path).unwrap();
    std::mem::forget(dir);
    store
}

fn enqueue(store: &MessageStore, op_id: &str, body: &str) -> String {
    store
        .enqueue(EnqueueRequest {
            sender_fqn: "proj:wg-1/a".into(),
            target_fqn: "proj:wg-1/b".into(),
            op_id: op_id.into(),
            content_type: DEFAULT_CONTENT_TYPE.into(),
            body: body.into(),
            source_plane: "api_inline".into(),
            source_ref: None,
        })
        .unwrap()
        .message_id
}

#[test]
fn build_outbox_message_has_no_privileged_fields() {
    let row = LeasedMessage {
        message_id: "m1".into(),
        sender_fqn: "proj:wg-1/a".into(),
        target_fqn: "proj:wg-1/b".into(),
        op_id: "op1".into(),
        content_type: DEFAULT_CONTENT_TYPE.into(),
        body: "hi".into(),
        attempt: 0,
    };

    let msg = build_outbox_message(&row);

    assert_eq!(msg.mode, "wake");
    assert_eq!(msg.body, "hi");
    assert!(msg.command.is_none());
    assert!(msg.action.is_none());
    assert!(msg.token.is_none());
    assert!(msg.target.is_none());
}

#[tokio::test]
async fn dispatch_success_marks_delivered_and_calls_deliver() {
    let store = store();
    let message_id = enqueue(&store, "op1", "hello");
    let seen = Arc::new(Mutex::new(Vec::new()));
    let seen_for_deliver = seen.clone();

    let processed = dispatch_due_with(
        &store,
        chrono::Utc::now(),
        &DispatcherConfig::default(),
        move |msg| {
            let seen = seen_for_deliver.clone();
            async move {
                seen.lock().unwrap().push(msg.body);
                Ok(())
            }
        },
    )
    .await
    .unwrap();

    assert_eq!(processed, 1);
    assert_eq!(*seen.lock().unwrap(), vec!["hello".to_string()]);
    let leased = store
        .lease_due(
            chrono::Utc::now() + chrono::Duration::hours(1),
            10,
            Duration::from_secs(1),
            "test",
        )
        .unwrap();
    assert!(leased.is_empty(), "{message_id} should be terminal");
}

#[tokio::test]
async fn dispatch_failure_retries_then_poisons() {
    let store = store();
    enqueue(&store, "op1", "hello");
    let config = DispatcherConfig {
        max_attempts: 1,
        ..DispatcherConfig::default()
    };

    dispatch_due_with(&store, chrono::Utc::now(), &config, |_msg| async {
        Err("no route".to_string())
    })
    .await
    .unwrap();

    let leased = store
        .lease_due(
            chrono::Utc::now() + chrono::Duration::hours(1),
            10,
            Duration::from_secs(1),
            "test",
        )
        .unwrap();
    assert!(leased.is_empty(), "poisoned rows must not lease again");
}

#[tokio::test]
async fn blocked_pty_worker_does_not_block_ordinary_or_different_target_progress() {
    let store = store();
    enqueue(&store, "ordinary-op", "ordinary body");
    let shutdown = CancellationToken::new();
    let targets = Arc::new(PtyWorkerTargets::default());
    let mut workers = JoinSet::new();
    let (blocked_started_tx, blocked_started_rx) = tokio::sync::oneshot::channel();

    assert!(spawn_pty_worker(
        &mut workers,
        &targets,
        &shutdown,
        DuePtyInputCandidate {
            injection_id: "blocked-operation".to_string(),
            target_fqn: "proj:wg-1-team/blocked".to_string(),
        },
        async move {
            blocked_started_tx.send(()).unwrap();
            std::future::pending::<()>().await;
        },
    ));
    blocked_started_rx.await.unwrap();
    assert_eq!(targets.len(), 1);
    assert!(
        !spawn_pty_worker(
            &mut workers,
            &targets,
            &shutdown,
            DuePtyInputCandidate {
                injection_id: "same-target-operation".to_string(),
                target_fqn: "proj:wg-1-team/blocked".to_string(),
            },
            async {},
        ),
        "only one lifecycle worker may wait on a logical target"
    );

    let processed = tokio::time::timeout(
        Duration::from_secs(1),
        dispatch_due_with(
            &store,
            chrono::Utc::now(),
            &DispatcherConfig::default(),
            |message| async move {
                assert_eq!(message.body, "ordinary body");
                Ok(())
            },
        ),
    )
    .await
    .expect("ordinary dispatch must not wait for PTY lifecycle")
    .unwrap();
    assert_eq!(processed, 1);

    let (different_done_tx, different_done_rx) = tokio::sync::oneshot::channel();
    assert!(spawn_pty_worker(
        &mut workers,
        &targets,
        &shutdown,
        DuePtyInputCandidate {
            injection_id: "different-target-operation".to_string(),
            target_fqn: "proj:wg-1-team/different".to_string(),
        },
        async move {
            different_done_tx.send(()).unwrap();
        },
    ));
    tokio::time::timeout(Duration::from_secs(1), different_done_rx)
        .await
        .expect("a different target must progress while the first is blocked")
        .unwrap();

    shutdown.cancel();
    tokio::time::timeout(
        Duration::from_secs(1),
        stop_pty_workers(&mut workers, Duration::from_millis(100)),
    )
    .await
    .expect("dispatcher shutdown must be bounded");
    assert_eq!(targets.len(), 0);
}

#[tokio::test]
async fn test_dispatcher_menu_guard_deferred_releases_lease() {
    let store = store();
    let message_id = enqueue(&store, "op-deferred", "hello");

    let config = DispatcherConfig::default();
    let processed = dispatch_due_with(&store, chrono::Utc::now(), &config, |_msg| async {
        Err(format!(
            "{}: session 123 is blocked by interactive menu",
            crate::pty::menu_guard::ERR_MENU_GUARD_DEFERRED
        ))
    })
    .await
    .unwrap();

    assert_eq!(processed, 1);

    // Verify status is queued and attempt count is preserved (0)
    let messages = store
        .lease_due(
            chrono::Utc::now(),
            10,
            Duration::from_secs(60),
            "test-worker",
        )
        .unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].message_id, message_id);
    assert_eq!(messages[0].attempt, 0);
}

#[tokio::test]
async fn test_dispatcher_typing_hold_deferred_releases_lease() {
    let store = store();
    let message_id = enqueue(&store, "op-typing-hold", "hello");

    let config = DispatcherConfig::default();
    let processed = dispatch_due_with(&store, chrono::Utc::now(), &config, |_msg| async {
        Err(format!(
            "{}: session 123 is holding peer wake injection while the user is typing",
            crate::pty::menu_guard::ERR_TYPING_HOLD_DEFERRED
        ))
    })
    .await
    .unwrap();

    assert_eq!(processed, 1);

    // Same recoverable contract as the menu guard: queued, attempt preserved.
    let messages = store
        .lease_due(
            chrono::Utc::now(),
            10,
            Duration::from_secs(60),
            "test-worker",
        )
        .unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].message_id, message_id);
    assert_eq!(messages[0].attempt, 0);
}
