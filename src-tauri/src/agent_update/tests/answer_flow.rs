use super::*;

// ---------------------------------------------------------------------
// #1551 - the answer flow
// ---------------------------------------------------------------------

/// #1551 - waker of the prompt's `oneshot::Receiver` in
/// `answer_prompt_enqueues_the_closure_before_releasing_the_loop`. tokio's
/// `oneshot::Sender::send` runs it INLINE (`Inner::complete` -> `wake_by_ref`), i.e. inside
/// `ClaimedPrompt::deliver`, before `answer_prompt` executes anything else, so it sees the
/// WebSocket queue at the instant the loop is released. `seen`: 0 = never woken, 1 = woken
/// while no closure frame was enqueued (the superseded send-before-emit order), 2 = woken
/// with `agent_update_prompt_closed {claude}` already enqueued (the plan's order).
struct ReleaseProbe {
    probe_rx: std::sync::Mutex<tokio::sync::mpsc::Receiver<WsOutMsg>>,
    seen: AtomicU8,
}

impl Wake for ReleaseProbe {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        let frame = self.probe_rx.lock().unwrap().try_recv().ok();
        let closed = frame.is_some_and(|frame| match frame {
            WsOutMsg::Text(text) => {
                serde_json::from_str::<Value>(&text)
                    .ok()
                    .is_some_and(|frame| {
                        frame["event"] == "agent_update_prompt_closed"
                            && frame["payload"]["command"] == "claude"
                    })
            }
            WsOutMsg::Binary(_) => false,
        });
        self.seen
            .store(if closed { 2 } else { 1 }, Ordering::SeqCst);
    }
}

#[tokio::test]
async fn answer_prompt_first_answer_resolves_persists_and_closes_every_surface() {
    let (app, mut frames_rx) = app_with_broadcaster();
    let handle = app.handle().clone();
    let gate = AgentUpdateGate::new();
    let rx = gate.register_prompt("claude", "Claude");
    let calls = Arc::new(AtomicUsize::new(0));
    let persist = {
        let calls = Arc::clone(&calls);
        move || {
            calls.fetch_add(1, Ordering::SeqCst);
            async { Ok::<(), String>(()) }
        }
    };
    assert!(answer_prompt(&handle, &gate, "claude", true, persist)
        .await
        .expect("answer"));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(rx.await.expect("the loop is released"));
    let frames = drain_frames(&mut frames_rx);
    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0]["event"], "agent_update_prompt_closed");
    assert_eq!(
        frames[0]["payload"],
        json!({ "command": "claude", "label": "Claude" })
    );
    assert_eq!(gate.prompt_state("claude"), PromptState::Answered(true));
    assert!(gate.snapshot().prompt.is_none());
}

#[tokio::test]
async fn answer_prompt_enqueues_the_closure_before_releasing_the_loop() {
    let broadcaster = WsBroadcaster::new();
    let probe_rx = broadcaster.subscribe();
    let mut order_rx = broadcaster.subscribe();
    let app = build_mock_app(crate::test_support::test_builder().manage(broadcaster));
    let handle = app.handle().clone();
    let gate = AgentUpdateGate::new();
    let mut rx = gate.register_prompt("claude", "Claude");

    let probe = Arc::new(ReleaseProbe {
        probe_rx: std::sync::Mutex::new(probe_rx),
        seen: AtomicU8::new(0),
    });
    let waker = Waker::from(Arc::clone(&probe));
    let mut cx = Context::from_waker(&waker);
    assert!(
        Pin::new(&mut rx).poll(&mut cx).is_pending(),
        "the probe must become the receiver's waker"
    );

    assert!(answer_prompt(&handle, &gate, "claude", true, || async {
        Ok::<(), String>(())
    })
    .await
    .expect("answer"));
    assert_eq!(
        probe.seen.load(Ordering::SeqCst),
        2,
        "the closure must already be enqueued when the loop is released"
    );
    assert!(rx.await.expect("the loop is released"));

    // Mirror the loop's advance to the next prompt.
    let _codex_rx = gate.register_prompt("codex", "Codex");
    emit_all(
        &handle,
        "agent_update_prompt",
        json!(AgentUpdatePrompt {
            command: "codex".to_string(),
            label: "Codex".to_string(),
        }),
    );
    let frames = drain_frames(&mut order_rx);
    assert_eq!(frames.len(), 2, "unexpected frames: {frames:?}");
    assert_eq!(frames[0]["event"], "agent_update_prompt_closed");
    assert_eq!(
        frames[0]["payload"],
        json!({ "command": "claude", "label": "Claude" })
    );
    assert_eq!(frames[1]["event"], "agent_update_prompt");
    assert_eq!(
        frames[1]["payload"],
        json!({ "command": "codex", "label": "Codex" })
    );
}

#[tokio::test]
async fn answer_prompt_superseded_answer_returns_false_without_persisting() {
    let (app, mut frames_rx) = app_with_broadcaster();
    let handle = app.handle().clone();
    let gate = AgentUpdateGate::new();
    let rx = gate.register_prompt("claude", "Claude");
    assert!(answer_prompt(&handle, &gate, "claude", true, || async {
        Ok::<(), String>(())
    })
    .await
    .expect("first answer"));
    assert!(rx.await.expect("the loop is released"));
    assert_eq!(drain_frames(&mut frames_rx).len(), 1);

    let calls = Arc::new(AtomicUsize::new(0));
    let persist = {
        let calls = Arc::clone(&calls);
        move || {
            calls.fetch_add(1, Ordering::SeqCst);
            async { Ok::<(), String>(()) }
        }
    };
    assert!(!answer_prompt(&handle, &gate, "claude", false, persist)
        .await
        .expect("second answer"));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(gate.snapshot().answered.get("claude"), Some(&true));
    assert_no_frame(&mut frames_rx).await;
}

#[tokio::test]
async fn answer_prompt_two_simultaneous_answers_are_serialized_first_wins() {
    let (app, mut frames_rx) = app_with_broadcaster();
    let handle = app.handle().clone();
    let gate = Arc::new(AgentUpdateGate::new());
    let rx = gate.register_prompt("claude", "Claude");
    let started = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());

    let task_a = tokio::spawn({
        let gate = Arc::clone(&gate);
        let handle = handle.clone();
        let started = Arc::clone(&started);
        let release = Arc::clone(&release);
        async move {
            answer_prompt(&handle, &gate, "claude", true, move || async move {
                started.notify_one();
                release.notified().await;
                Ok::<(), String>(())
            })
            .await
        }
    });
    started.notified().await;

    let calls_b = Arc::new(AtomicUsize::new(0));
    let mut task_b = tokio::spawn({
        let gate = Arc::clone(&gate);
        let handle = handle.clone();
        let calls = Arc::clone(&calls_b);
        async move {
            answer_prompt(&handle, &gate, "claude", false, move || {
                calls.fetch_add(1, Ordering::SeqCst);
                async { Ok::<(), String>(()) }
            })
            .await
        }
    });
    assert!(
        tokio::time::timeout(Duration::from_millis(100), &mut task_b)
            .await
            .is_err(),
        "B must be queued behind the serial lock"
    );

    release.notify_one();
    assert!(task_a.await.expect("join a").expect("answer a"));
    assert!(!task_b.await.expect("join b").expect("answer b"));
    assert_eq!(calls_b.load(Ordering::SeqCst), 0);
    assert!(rx.await.expect("the loop is released"));
    let frames = drain_frames(&mut frames_rx);
    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0]["event"], "agent_update_prompt_closed");
    assert_eq!(
        gate.snapshot().answered,
        BTreeMap::from([("claude".to_string(), true)])
    );
}

#[tokio::test]
async fn answer_prompt_failed_persist_keeps_the_prompt_pending_and_the_next_answer_wins() {
    let (app, mut frames_rx) = app_with_broadcaster();
    let handle = app.handle().clone();
    let gate = AgentUpdateGate::new();
    let rx = gate.register_prompt("claude", "Claude");

    let failed = answer_prompt(&handle, &gate, "claude", true, || async {
        Err::<(), String>("disk full".to_string())
    })
    .await;
    assert_eq!(failed, Err("disk full".to_string()));
    assert_eq!(gate.prompt_state("claude"), PromptState::Pending);
    assert!(gate.snapshot().answered.is_empty());
    assert!(drain_frames(&mut frames_rx).is_empty());

    assert!(answer_prompt(&handle, &gate, "claude", false, || async {
        Ok::<(), String>(())
    })
    .await
    .expect("second answer"));
    assert!(!rx.await.expect("the loop is released"));
    assert_eq!(drain_frames(&mut frames_rx).len(), 1);
    assert_eq!(
        gate.snapshot().answered,
        BTreeMap::from([("claude".to_string(), false)])
    );
}

#[tokio::test]
async fn answer_prompt_failed_persist_while_queued_lets_the_waiter_win() {
    let (app, _frames_rx) = app_with_broadcaster();
    let handle = app.handle().clone();
    let gate = Arc::new(AgentUpdateGate::new());
    let _rx = gate.register_prompt("claude", "Claude");
    let started = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());

    let task_a = tokio::spawn({
        let gate = Arc::clone(&gate);
        let handle = handle.clone();
        let started = Arc::clone(&started);
        let release = Arc::clone(&release);
        async move {
            answer_prompt(&handle, &gate, "claude", true, move || async move {
                started.notify_one();
                release.notified().await;
                Err::<(), String>("disk full".to_string())
            })
            .await
        }
    });
    started.notified().await;
    let task_b = tokio::spawn({
        let gate = Arc::clone(&gate);
        let handle = handle.clone();
        async move {
            answer_prompt(&handle, &gate, "claude", false, || async {
                Ok::<(), String>(())
            })
            .await
        }
    });
    release.notify_one();
    assert_eq!(
        task_a.await.expect("join a"),
        Err("disk full".to_string()),
        "a failed persist changes nothing"
    );
    assert!(task_b.await.expect("join b").expect("answer b"));
    assert_eq!(gate.snapshot().answered.get("claude"), Some(&false));
}

#[tokio::test]
async fn answer_prompt_late_answer_after_timeout_persists_and_returns_false() {
    let (app, mut frames_rx) = app_with_broadcaster();
    let handle = app.handle().clone();
    let gate = AgentUpdateGate::new();
    let rx = gate.register_prompt("claude", "Claude");
    drop(rx);
    assert!(run_expiry_arm(&handle, &gate, "claude").await.is_some());
    assert_eq!(drain_frames(&mut frames_rx).len(), 1);

    let calls = Arc::new(AtomicUsize::new(0));
    let persist = {
        let calls = Arc::clone(&calls);
        move || {
            calls.fetch_add(1, Ordering::SeqCst);
            async { Ok::<(), String>(()) }
        }
    };
    assert!(!answer_prompt(&handle, &gate, "claude", true, persist)
        .await
        .expect("late answer"));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        gate.snapshot().answered,
        BTreeMap::from([("claude".to_string(), true)])
    );
    assert_no_frame(&mut frames_rx).await;
}

#[tokio::test]
async fn answer_prompt_and_expiry_race_emit_exactly_one_closure_with_either_winner() {
    // (i) the answer claims first: the expiry then finds nothing.
    {
        let (app, mut frames_rx) = app_with_broadcaster();
        let handle = app.handle().clone();
        let gate = AgentUpdateGate::new();
        let rx = gate.register_prompt("claude", "Claude");
        assert!(answer_prompt(&handle, &gate, "claude", true, || async {
            Ok::<(), String>(())
        })
        .await
        .expect("answer"));
        assert!(rx.await.expect("the loop is released"));
        assert!(run_expiry_arm(&handle, &gate, "claude").await.is_none());
        assert_eq!(drain_frames(&mut frames_rx).len(), 1);
    }

    // (ii) the expiry wins: the late answer persists and emits nothing.
    {
        let (app, mut frames_rx) = app_with_broadcaster();
        let handle = app.handle().clone();
        let gate = AgentUpdateGate::new();
        let rx = gate.register_prompt("claude", "Claude");
        drop(rx);
        assert!(run_expiry_arm(&handle, &gate, "claude").await.is_some());
        let calls = Arc::new(AtomicUsize::new(0));
        let persist = {
            let calls = Arc::clone(&calls);
            move || {
                calls.fetch_add(1, Ordering::SeqCst);
                async { Ok::<(), String>(()) }
            }
        };
        assert!(!answer_prompt(&handle, &gate, "claude", true, persist)
            .await
            .expect("late answer"));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(drain_frames(&mut frames_rx).len(), 1);
    }

    // (iii) the expiry is queued behind a claiming answer.
    {
        let (app, mut frames_rx) = app_with_broadcaster();
        let handle = app.handle().clone();
        let gate = Arc::new(AgentUpdateGate::new());
        let rx = gate.register_prompt("claude", "Claude");
        drop(rx); // the timeout elapsed
        let started = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let calls = Arc::new(AtomicUsize::new(0));

        let answer = tokio::spawn({
            let gate = Arc::clone(&gate);
            let handle = handle.clone();
            let started = Arc::clone(&started);
            let release = Arc::clone(&release);
            let calls = Arc::clone(&calls);
            async move {
                answer_prompt(&handle, &gate, "claude", true, move || async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    started.notify_one();
                    release.notified().await;
                    Ok::<(), String>(())
                })
                .await
            }
        });
        started.notified().await;
        let mut expiry = tokio::spawn({
            let gate = Arc::clone(&gate);
            let handle = handle.clone();
            async move { run_expiry_arm(&handle, &gate, "claude").await }
        });
        assert!(
            tokio::time::timeout(Duration::from_millis(100), &mut expiry)
                .await
                .is_err(),
            "the expiry arm must block on the serial lock"
        );
        release.notify_one();
        assert!(
            !answer.await.expect("join answer").expect("answer"),
            "the receiver is gone, so the answer is late"
        );
        assert!(
            expiry.await.expect("join expiry").is_none(),
            "the expiry found no entry and emitted nothing"
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(drain_frames(&mut frames_rx).len(), 1);
        assert_eq!(
            gate.snapshot().answered,
            BTreeMap::from([("claude".to_string(), true)])
        );
    }
}

#[tokio::test]
async fn answer_prompt_rejects_unprompted_command_without_persisting() {
    let (app, mut frames_rx) = app_with_broadcaster();
    let handle = app.handle().clone();
    let gate = AgentUpdateGate::new();
    let calls = Arc::new(AtomicUsize::new(0));
    let persist = {
        let calls = Arc::clone(&calls);
        move || {
            calls.fetch_add(1, Ordering::SeqCst);
            async { Ok::<(), String>(()) }
        }
    };
    let error = answer_prompt(&handle, &gate, "claude", true, persist)
        .await
        .expect_err("an unprompted command must be rejected");
    assert!(error.contains("was not prompted"), "unexpected: {error}");
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_no_frame(&mut frames_rx).await;
}
