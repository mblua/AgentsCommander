use super::*;
use std::sync::{Barrier, Mutex};

fn request() -> Request {
    Request {
        identity: PlainIdentity {
            session_id: Uuid::new_v4(),
            request_id: Uuid::new_v4(),
            action: PLAIN_SELF_CLEAR_ACTION.into(),
            requested_by: "proj:room/agent".into(),
        },
        root: "owned-root".into(),
        local_dir: ".local".into(),
        clear_text: "/clear",
    }
}

#[derive(Default)]
struct Harness {
    receipt: Mutex<Option<Vec<u8>>>,
    stage: Mutex<Vec<u8>>,
    events: Mutex<Vec<&'static str>>,
    drivers: Mutex<Vec<Driver>>,
    prepare_barriers: Option<(Arc<Barrier>, Arc<Barrier>)>,
    publish_barriers: Option<(Arc<Barrier>, Arc<Barrier>)>,
    inject_barriers: Option<(Arc<tokio::sync::Barrier>, Arc<tokio::sync::Barrier>)>,
}

impl Harness {
    fn event(&self, value: &'static str) {
        self.events.lock().unwrap().push(value);
    }
    fn count(&self, value: &str) -> usize {
        self.events
            .lock()
            .unwrap()
            .iter()
            .filter(|e| **e == value)
            .count()
    }
}

async fn run(
    request: Request,
    pending: Arc<PendingSelfClear>,
    h: Arc<Harness>,
    failure: &'static str,
) -> Result<(), PlainError> {
    let (read, prepare, publish, launch, transport, inject, boundary) = (
        h.clone(),
        h.clone(),
        h.clone(),
        h.clone(),
        h.clone(),
        h.clone(),
        h.clone(),
    );
    let sid = request.identity.session_id;
    let identity = request.identity.clone();
    let publishing = pending.clone();
    accept(
        request,
        pending.clone(),
        Callbacks {
            read_receipt: move |_path: &Path| {
                read.event("read");
                if failure.starts_with("armed_") && read.count("read") == 1 {
                    return Ok(None);
                }
                if failure == "armed_error" {
                    return Err("armed read failed".into());
                }
                if failure == "read" {
                    return Err("read failed".into());
                }
                Ok(read.receipt.lock().unwrap().clone())
            },
            prepare_receipt: move |_path: &Path, bytes: &[u8], lease: &Lease| {
                prepare.event("prepare");
                if let Some((entered, release)) = &prepare.prepare_barriers {
                    entered.wait();
                    release.wait();
                }
                if failure == "write" {
                    return Err("write failed".into());
                }
                prepare.event("write");
                *prepare.stage.lock().unwrap() = bytes.to_vec();
                prepare.event("read_stage");
                if failure == "read_stage" {
                    return Err("stage read failed".into());
                }
                let mut verified = bytes.to_vec();
                if failure == "corrupt" {
                    verified = b"invalid".to_vec();
                }
                if failure == "mismatch" {
                    let mut receipt: Receipt = serde_json::from_slice(bytes).unwrap();
                    receipt.session_id = Uuid::new_v4();
                    verified = serde_json::to_vec_pretty(&receipt).unwrap();
                }
                if failure == "arm" {
                    // Remove only the captured current owner, then install an unrelated Legacy owner.
                    assert!(pending.release(lease));
                    pending.reserve_legacy(sid).unwrap();
                }
                Ok(Prepared {
                    stage: (),
                    bytes: verified,
                })
            },
            publish_prepared: move |_stage: &(), _path: &Path| {
                assert_eq!(
                    publishing.reserve_plain(identity),
                    PlainReservation::SameArmed
                );
                publish.event("validate");
                publish.event("arm");
                publish.event("rename");
                if let Some((entered, release)) = &publish.publish_barriers {
                    entered.wait();
                    release.wait();
                }
                if failure == "rename" {
                    return Err("rename failed".into());
                }
                if failure == "collision" {
                    *publish.receipt.lock().unwrap() = Some(b"foreign destination".to_vec());
                    return Err("destination exists".into());
                }
                *publish.receipt.lock().unwrap() = Some(publish.stage.lock().unwrap().clone());
                publish.event("rename_ok");
                Ok(())
            },
            launch: move |driver| {
                launch.event("launch");
                assert!(launch.receipt.lock().unwrap().is_some());
                launch.drivers.lock().unwrap().push(driver);
            },
            transport: move || async move {
                transport.event("transport");
                if ["delivered_before", "delivered_after", "remove"].contains(&failure) {
                    return Err(failure.into());
                }
                Ok(())
            },
            inject: move |got_sid, text| async move {
                assert_eq!(got_sid, sid);
                assert_eq!(text, "/clear");
                inject.event("inject");
                if let Some((entered, release)) = &inject.inject_barriers {
                    entered.wait().await;
                    release.wait().await;
                }
                if failure == "inject" {
                    Err("inject failed".into())
                } else {
                    Ok(())
                }
            },
            note_boundary: move |got_sid| async move {
                assert_eq!(got_sid, sid);
                boundary.event("boundary");
            },
        },
    )
    .await
}

#[tokio::test]
async fn successful_publication_launches_once_and_rescans_only_transport() {
    let r = request();
    let id = r.identity.clone();
    let pending = Arc::new(PendingSelfClear::default());
    let h = Arc::new(Harness::default());
    run(r, pending.clone(), h.clone(), "").await.unwrap();
    let bytes = h.receipt.lock().unwrap().clone().unwrap();
    let mut retry = request();
    retry.identity = id.clone();
    retry.validate_receipt(&bytes).unwrap();
    assert_eq!(
        pending.reserve_plain(id.clone()),
        PlainReservation::SameArmed
    );
    run(retry, pending.clone(), h.clone(), "").await.unwrap();
    assert_eq!(h.count("launch"), 1);
    let driver = h.drivers.lock().unwrap().pop().unwrap();
    driver.await;
    assert!(pending.is_empty());
    let mut retry = request();
    retry.identity = id;
    run(retry, pending, h.clone(), "").await.unwrap();
    assert_eq!(h.count("launch"), 1);
    assert_eq!(h.count("inject"), 1);
    assert_eq!(h.count("boundary"), 1);
    assert_eq!(h.count("transport"), 3);
    assert_eq!(
        &h.events.lock().unwrap()[..9],
        &[
            "read",
            "prepare",
            "write",
            "read_stage",
            "validate",
            "arm",
            "rename",
            "rename_ok",
            "launch"
        ]
    );
    let events = h.events.lock().unwrap();
    let rename = events.iter().position(|e| *e == "rename_ok").unwrap();
    assert_eq!(events[rename + 1], "launch");
    assert_eq!(events[rename + 2], "transport");
}

#[tokio::test]
async fn failures_before_publication_never_launch_and_release_only_their_lease() {
    for failure in [
        "read",
        "write",
        "read_stage",
        "corrupt",
        "mismatch",
        "arm",
        "rename",
        "collision",
    ] {
        let r = request();
        let sid = r.identity.session_id;
        let pending = Arc::new(PendingSelfClear::default());
        let h = Arc::new(Harness::default());
        assert!(
            run(r, pending.clone(), h.clone(), failure).await.is_err(),
            "{failure}"
        );
        assert_eq!(h.count("launch"), 0, "{failure}");
        assert_eq!(h.count("transport"), 0);
        if failure == "arm" {
            assert!(pending.legacy_lease(sid).is_some());
        } else {
            assert!(pending.is_empty());
        }
        if failure == "collision" {
            assert_eq!(
                *h.receipt.lock().unwrap(),
                Some(b"foreign destination".to_vec())
            );
        } else {
            assert!(h.receipt.lock().unwrap().is_none());
        }
        if !["rename", "collision"].contains(&failure) {
            assert_eq!(h.count("rename"), 0);
        }
    }
}

#[tokio::test]
async fn malformed_or_foreign_receipts_never_overwrite_or_reserve() {
    for bytes in [
        b"invalid".to_vec(),
        serde_json::to_vec(&request().receipt()).unwrap(),
    ] {
        let pending = Arc::new(PendingSelfClear::default());
        let h = Arc::new(Harness::default());
        *h.receipt.lock().unwrap() = Some(bytes.clone());
        assert!(run(request(), pending.clone(), h.clone(), "")
            .await
            .is_err());
        assert!(pending.is_empty());
        assert_eq!(h.count("prepare"), 0);
        assert_eq!(h.count("launch"), 0);
        assert_eq!(*h.receipt.lock().unwrap(), Some(bytes));
    }
}

#[tokio::test]
async fn delivered_and_remove_errors_leave_ack_and_never_replay() {
    for failure in ["delivered_before", "delivered_after", "remove"] {
        let r = request();
        let id = r.identity.clone();
        let pending = Arc::new(PendingSelfClear::default());
        let h = Arc::new(Harness::default());
        assert!(matches!(
            run(r, pending.clone(), h.clone(), failure).await,
            Err(PlainError::Transport(_))
        ));
        assert_eq!(h.count("launch"), 1);
        assert_eq!(
            pending.reserve_plain(id.clone()),
            PlainReservation::SameArmed
        );
        let driver = h.drivers.lock().unwrap().pop().unwrap();
        driver.await;
        let mut retry = request();
        retry.identity = id;
        run(retry, pending.clone(), h.clone(), "").await.unwrap();
        assert_eq!(h.count("launch"), 1);
        assert_eq!(h.count("inject"), 1);
        assert!(pending.is_empty());
    }
}

#[tokio::test]
async fn injector_error_has_no_boundary_or_replay_and_releases() {
    let pending = Arc::new(PendingSelfClear::default());
    let h = Arc::new(Harness::default());
    run(request(), pending.clone(), h.clone(), "inject")
        .await
        .unwrap();
    let driver = h.drivers.lock().unwrap().pop().unwrap();
    driver.await;
    assert_eq!(h.count("inject"), 1);
    assert_eq!(h.count("boundary"), 0);
    assert!(pending.is_empty());
}

#[tokio::test]
async fn publishing_and_armed_without_receipt_are_retryable_without_release() {
    let r = request();
    let id = r.identity.clone();
    let pending = Arc::new(PendingSelfClear::default());
    let PlainReservation::New(lease) = pending.reserve_plain(id.clone()) else {
        panic!()
    };
    let h = Arc::new(Harness::default());
    assert!(matches!(
        run(r, pending.clone(), h.clone(), "").await,
        Err(PlainError::Retryable(_))
    ));
    pending.arm_plain(&lease).unwrap();
    let mut retry = request();
    retry.identity = id.clone();
    assert!(matches!(
        run(retry, pending.clone(), h.clone(), "").await,
        Err(PlainError::Retryable(_))
    ));
    assert_eq!(pending.reserve_plain(id), PlainReservation::SameArmed);
    assert_eq!(h.count("launch"), 0);
    assert_eq!(h.count("prepare"), 0);
    assert!(pending.release(&lease));
}

#[test]
fn canonical_sender_is_checked_independently_of_transport_authentication() {
    let id = request().identity;
    assert!(validate_caller(&id, &id.requested_by, "", None).is_ok());
    assert!(validate_caller(&id, "other", "", None).is_err());
    assert!(validate_caller(&id, &id.requested_by, "resolvable", None).is_err());
    assert!(validate_caller(&id, &id.requested_by, "", Some("other")).is_err());
    let mut wrong = id.clone();
    wrong.action = "self-handoff-and-clear".into();
    assert!(validate_caller(&wrong, &wrong.requested_by, "", None).is_err());
}

#[tokio::test]
async fn pending_during_prepare_and_publish_never_acknowledges_or_launches_twice() {
    for during_prepare in [true, false] {
        let r = request();
        let id = r.identity.clone();
        let entered = Arc::new(Barrier::new(2));
        let release = Arc::new(Barrier::new(2));
        let pair = Some((entered.clone(), release.clone()));
        let h = Arc::new(if during_prepare {
            Harness {
                prepare_barriers: pair,
                ..Harness::default()
            }
        } else {
            Harness {
                publish_barriers: pair,
                ..Harness::default()
            }
        });
        let pending = Arc::new(PendingSelfClear::default());
        let (owner_h, owner_p) = (h.clone(), pending.clone());
        let owner = std::thread::spawn(move || {
            tokio::runtime::Builder::new_current_thread()
                .build()
                .unwrap()
                .block_on(run(r, owner_p, owner_h, ""))
        });
        entered.wait();
        let mut duplicate = request();
        duplicate.identity = id.clone();
        assert!(matches!(
            run(duplicate, pending.clone(), h.clone(), "").await,
            Err(PlainError::Retryable(_))
        ));
        assert!(h.receipt.lock().unwrap().is_none());
        assert_eq!(h.count("launch"), 0);
        release.wait();
        owner.join().unwrap().unwrap();
        assert_eq!(h.count("rename_ok"), 1);
        assert_eq!(h.count("launch"), 1);
        assert_eq!(pending.reserve_plain(id), PlainReservation::SameArmed);
        let driver = h.drivers.lock().unwrap().pop().unwrap();
        driver.await;
        assert!(pending.is_empty());
    }
}

#[tokio::test]
async fn two_threads_have_one_publication_and_launch_until_driver_releases() {
    let r = request();
    let id = r.identity;
    let pending = Arc::new(PendingSelfClear::default());
    let h = Arc::new(Harness::default());
    let barrier = Arc::new(Barrier::new(3));
    let mut threads = Vec::new();
    for _ in 0..2 {
        let (pending, h, barrier, id) = (pending.clone(), h.clone(), barrier.clone(), id.clone());
        threads.push(std::thread::spawn(move || {
            let mut r = request();
            r.identity = id;
            barrier.wait();
            tokio::runtime::Builder::new_current_thread()
                .build()
                .unwrap()
                .block_on(run(r, pending, h, ""))
        }));
    }
    barrier.wait();
    for thread in threads {
        match thread.join().unwrap() {
            Ok(()) | Err(PlainError::Retryable(_)) => {}
            result => panic!("unexpected: {result:?}"),
        }
    }
    assert_eq!(h.count("rename_ok"), 1);
    assert_eq!(h.count("launch"), 1);
    assert_eq!(pending.reserve_plain(id), PlainReservation::SameArmed);
    let driver = h.drivers.lock().unwrap().pop().unwrap();
    driver.await;
    assert!(pending.is_empty());
}

#[tokio::test]
async fn blocked_injector_keeps_ack_visible_and_files_intact_until_release() {
    let temp = tempfile::TempDir::new().unwrap();
    for name in ["SELF-HANDOFF.md", "SELF-FORGET.md"] {
        std::fs::write(temp.path().join(name), name).unwrap();
    }
    let entered = Arc::new(tokio::sync::Barrier::new(2));
    let release = Arc::new(tokio::sync::Barrier::new(2));
    let h = Arc::new(Harness {
        inject_barriers: Some((entered.clone(), release.clone())),
        ..Harness::default()
    });
    let pending = Arc::new(PendingSelfClear::default());
    let mut r = request();
    r.root = temp.path().into();
    let id = r.identity.clone();
    run(r, pending.clone(), h.clone(), "").await.unwrap();
    let driver = h.drivers.lock().unwrap().pop().unwrap();
    let task = tokio::spawn(driver);
    entered.wait().await;
    assert!(h.receipt.lock().unwrap().is_some());
    assert_eq!(h.count("launch"), 1);
    assert_eq!(h.count("inject"), 1);
    assert_eq!(h.count("boundary"), 0);
    assert_eq!(pending.reserve_plain(id), PlainReservation::SameArmed);
    release.wait().await;
    task.await.unwrap();
    assert!(pending.is_empty());
    assert_eq!(h.count("boundary"), 1);
    for name in ["SELF-HANDOFF.md", "SELF-FORGET.md"] {
        assert_eq!(
            std::fs::read(temp.path().join(name)).unwrap(),
            name.as_bytes()
        );
    }
    assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 2);
}

#[tokio::test]
async fn armed_second_read_is_transport_only_or_retryable_without_releasing_owner() {
    for failure in ["armed_receipt", "armed_error", "armed_corrupt"] {
        let r = request();
        let id = r.identity.clone();
        let pending = Arc::new(PendingSelfClear::default());
        let PlainReservation::New(lease) = pending.reserve_plain(id.clone()) else {
            panic!()
        };
        pending.arm_plain(&lease).unwrap();
        let h = Arc::new(Harness::default());
        *h.receipt.lock().unwrap() = Some(if failure == "armed_corrupt" {
            b"invalid".to_vec()
        } else {
            serde_json::to_vec_pretty(&r.receipt()).unwrap()
        });
        let result = run(r, pending.clone(), h.clone(), failure).await;
        if failure == "armed_receipt" {
            assert!(result.is_ok());
        } else {
            assert!(matches!(result, Err(PlainError::Retryable(_))));
        }
        assert_eq!(pending.reserve_plain(id), PlainReservation::SameArmed);
        assert_eq!(h.count("launch"), 0);
        assert_eq!(h.count("prepare"), 0);
        assert!(pending.release(&lease));
    }
}

#[test]
fn receipts_require_every_binding_and_forbid_legacy_or_secret_fields() {
    let r = request();
    let expected = serde_json::to_value(r.receipt()).unwrap();
    for (field, value) in [
        ("action", serde_json::json!("self-handoff-and-clear")),
        ("status", serde_json::json!("queued")),
        ("request_id", serde_json::json!(Uuid::new_v4())),
        ("session_id", serde_json::json!(Uuid::new_v4())),
        ("session_id", serde_json::json!("not-uuid")),
        ("requested_by", serde_json::json!("other")),
        ("token", serde_json::json!("forbidden")),
        ("settle_secs", serde_json::json!(30)),
    ] {
        let mut changed = expected.clone();
        changed[field] = value;
        assert!(
            r.validate_receipt(&serde_json::to_vec(&changed).unwrap())
                .is_err(),
            "{field}"
        );
    }
}
