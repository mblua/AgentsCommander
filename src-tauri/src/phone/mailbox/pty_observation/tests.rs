use super::*;
use crate::api::message_store::{MessageStore, PtyInputEnqueueRequest};
use crate::config::teams::VerifiedPtyInputIdentity;
use crate::phone::mailbox::poller::MailboxPoller;
use crate::phone::types::{
    canonical_pty_timestamp, PtyInputEnterMode, PtyInputHostEnvelope, PtyInputReasonCode,
    PtyInputSourcePlane, PtyInputWirePayload, PTY_INPUT_VERSION,
};
use crate::pty::backend::SessionBackendKind;
use std::sync::atomic::{AtomicUsize, Ordering};
use uuid::Uuid;

#[derive(Default)]
struct Observations {
    accesses: AtomicUsize,
    readiness_flow: bool,
    sessions: Option<Arc<tokio::sync::RwLock<SessionManager>>>,
    restore: Option<Arc<RestoreInProgress>>,
}

impl Observations {
    fn record(&self, position: usize, getter: &str) {
        let previous = self.accesses.fetch_add(1, Ordering::SeqCst);
        assert_eq!(previous, position, "unexpected getter: {getter}");
    }

    fn unexpected<T>(&self, getter: &str) -> T {
        self.accesses.fetch_add(1, Ordering::SeqCst);
        panic!("unexpected getter: {getter}");
    }
}

impl PtyAuthorityState for Observations {
    fn pty_sessions(&self) -> &Arc<tokio::sync::RwLock<SessionManager>> {
        self.record(if self.readiness_flow { 3 } else { 2 }, "sessions");
        self.sessions.as_ref().expect("unexpected sessions getter")
    }

    fn pty_routes(&self) -> &Arc<Mutex<PtyManager>> {
        self.unexpected("routes")
    }

    fn pty_settings(&self) -> &SettingsState {
        self.unexpected("settings")
    }
}

impl PtyTargetState for Observations {
    fn pty_readiness(&self) -> &Arc<IdleDetector> {
        self.unexpected("readiness")
    }

    fn pty_restore(&self) -> Option<&Arc<RestoreInProgress>> {
        self.record(0, "restore");
        self.restore.as_ref()
    }

    fn pty_purge(&self) -> Option<&Arc<PurgeGuard>> {
        self.record(1, "purge");
        None
    }
}

fn claim(store: &MessageStore) -> crate::api::message_store::ClaimedPtyInputOperation {
    let id = Uuid::new_v4().to_string();
    let now = chrono::Utc::now();
    store
        .enqueue_pty_input(PtyInputEnqueueRequest {
            injection_id: id.clone(),
            sender_fqn: "project:room/sender".into(),
            target_fqn: "project:room/target".into(),
            op_id: id.clone(),
            nonce_sha256: "a".repeat(64),
            request_fingerprint: "b".repeat(64),
            confirmation_tag: Some("c".repeat(64)),
            requested_agent_id: None,
            payload: b"input".to_vec(),
            source_plane: PtyInputSourcePlane::HostCli,
            sender_incarnation_fingerprint: "d".repeat(64),
            sender_identity_fingerprint: "e".repeat(64),
            target_identity_fingerprint: "f".repeat(64),
            authority_session_id: Uuid::new_v4().to_string(),
            authority_client_id: None,
            authority_client_generation: None,
            issued_at: canonical_pty_timestamp(now),
            expires_at: canonical_pty_timestamp(now + chrono::Duration::minutes(10)),
        })
        .unwrap();
    store
        .claim_pty_input(
            PtyInputSourcePlane::HostCli,
            Some(&id),
            &Uuid::new_v4().to_string(),
            chrono::Utc::now(),
        )
        .unwrap()
        .unwrap()
}

fn target(path: &std::path::Path) -> VerifiedPtyInputIdentity {
    let identity = crate::path_identity::verify_directory(path).unwrap();
    VerifiedPtyInputIdentity {
        canonical_fqn: "project:room/target".into(),
        project: "project".into(),
        workgroup: "room".into(),
        agent: "target".into(),
        replica_root: path.into(),
        matrix_root: path.into(),
        is_coordinator: false,
        project_identity: identity.clone(),
        ac_root_identity: identity.clone(),
        workgroup_identity: identity.clone(),
        replica_identity: identity.clone(),
        matrix_identity: identity,
        incarnation_fingerprint: "d".repeat(64),
        authority_fingerprint: "f".repeat(64),
    }
}

#[tokio::test]
async fn host_enqueue_rejects_app_outbox_without_getters() {
    let temp = tempfile::tempdir().unwrap();
    let store = MessageStore::open(temp.path().join("host.sqlite3")).unwrap();
    let identity = crate::path_identity::verify_directory(temp.path()).unwrap();
    let id = Uuid::new_v4().to_string();
    let now = chrono::Utc::now();
    let issued_at = canonical_pty_timestamp(now);
    let envelope = PtyInputHostEnvelope {
        id: id.clone(),
        token: Uuid::new_v4().to_string(),
        from: "project:room/sender".into(),
        to: "project:room/target".into(),
        body: String::new(),
        mode: "wake".into(),
        get_output: false,
        preferred_agent: String::new(),
        priority: "normal".into(),
        timestamp: issued_at.clone(),
        action: "pty-input".into(),
        pty_input: PtyInputWirePayload {
            version: PTY_INPUT_VERSION,
            text: "input".into(),
            enter: PtyInputEnterMode::AgentSubmit,
            injection_id: id.clone(),
            op_id: id.clone(),
            issued_at,
            expires_at: canonical_pty_timestamp(now + chrono::Duration::minutes(10)),
            nonce: Uuid::new_v4().to_string(),
            agent_id: None,
        },
    };
    let observations = Observations::default();
    let result = MailboxPoller::delivery_only_for_tests()
        .validate_and_enqueue_host_pty_input(
            &observations,
            &temp.path().join(format!("{id}.json")),
            true,
            &identity,
            &store,
            &envelope,
        )
        .await;
    assert_eq!(result, Err(PtyInputReasonCode::UnsafePath));
    assert_eq!(observations.accesses.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn bad_claimed_authority_uuid_observes_no_getters() {
    let temp = tempfile::tempdir().unwrap();
    let store = MessageStore::open(temp.path().join("authority.sqlite3")).unwrap();
    let mut claimed = claim(&store);
    claimed.authority_session_id = "invalid".into();
    let observations = Observations::default();
    assert!(MailboxPoller::delivery_only_for_tests()
        .validate_claimed_host_authority(&observations, &claimed)
        .await
        .is_none());
    assert_eq!(observations.accesses.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn expired_target_observes_no_getters() {
    let temp = tempfile::tempdir().unwrap();
    let store = MessageStore::open(temp.path().join("expired.sqlite3")).unwrap();
    let mut claimed = claim(&store);
    claimed.expires_at = canonical_pty_timestamp(chrono::Utc::now() - chrono::Duration::seconds(1));
    let observations = Observations::default();
    assert!(MailboxPoller::delivery_only_for_tests()
        .validate_selected_pty_target(
            &observations,
            &claimed,
            &target(temp.path()),
            Uuid::new_v4(),
            SessionBackendKind::LocalProcess,
        )
        .await
        .is_none());
    assert_eq!(observations.accesses.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn target_observes_restore_then_purge_then_sessions() {
    let temp = tempfile::tempdir().unwrap();
    let store = MessageStore::open(temp.path().join("target.sqlite3")).unwrap();
    let claimed = claim(&store);
    let observations = Observations {
        sessions: Some(Arc::new(tokio::sync::RwLock::new(SessionManager::new()))),
        ..Default::default()
    };
    assert!(MailboxPoller::delivery_only_for_tests()
        .validate_selected_pty_target(
            &observations,
            &claimed,
            &target(temp.path()),
            Uuid::new_v4(),
            SessionBackendKind::LocalProcess,
        )
        .await
        .is_none());
    assert_eq!(observations.accesses.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn blocked_restore_skips_later_getters() {
    let temp = tempfile::tempdir().unwrap();
    let store = MessageStore::open(temp.path().join("restore.sqlite3")).unwrap();
    let claimed = claim(&store);
    let observations = Observations {
        restore: Some(Arc::new(RestoreInProgress(
            std::sync::atomic::AtomicBool::new(true),
        ))),
        ..Default::default()
    };
    assert!(MailboxPoller::delivery_only_for_tests()
        .validate_selected_pty_target(
            &observations,
            &claimed,
            &target(temp.path()),
            Uuid::new_v4(),
            SessionBackendKind::LocalProcess,
        )
        .await
        .is_none());
    assert_eq!(observations.accesses.load(Ordering::SeqCst), 1);
}

impl PtyReadinessState for Observations {
    fn pty_menu(&self) -> Option<&Arc<crate::pty::menu_guard::MenuGuard>> {
        self.record(2, "menu");
        None
    }
}

#[tokio::test]
async fn api_missing_store_or_credentials_observes_no_getters() {
    let temp = tempfile::tempdir().unwrap();
    let store = MessageStore::open(temp.path().join("api.sqlite3")).unwrap();
    let mut claimed = claim(&store);
    let clients = Arc::new(crate::api::auth::ApiClientStore::new(
        temp.path().join("clients"),
    ));
    let observations = Observations::default();
    let poller = MailboxPoller::delivery_only_for_tests();
    assert!(poller
        .validate_claimed_api_authority(&observations, &claimed, None)
        .await
        .unwrap()
        .is_none());
    assert!(poller
        .validate_claimed_api_authority(&observations, &claimed, Some(&clients))
        .await
        .unwrap()
        .is_none());
    claimed.authority_client_id = Some(Uuid::new_v4().to_string());
    assert!(poller
        .validate_claimed_api_authority(&observations, &claimed, Some(&clients))
        .await
        .unwrap()
        .is_none());
    assert_eq!(observations.accesses.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn readiness_expiry_and_failed_heartbeat_observe_no_getters() {
    let temp = tempfile::tempdir().unwrap();
    let store = Arc::new(MessageStore::open(temp.path().join("expiry.sqlite3")).unwrap());
    let mut claimed = claim(&store);
    let observations = Observations::default();
    let past = chrono::Utc::now() - chrono::Duration::seconds(1);
    let mut heartbeat = crate::api::message_store::PreparationHeartbeatGuard::start(
        Arc::clone(&store),
        claimed.injection_id.clone(),
        claimed.lease_owner.clone(),
        past,
    );
    claimed.expires_at = canonical_pty_timestamp(past);
    let poller = MailboxPoller::delivery_only_for_tests();
    assert_eq!(
        poller
            .wait_for_pty_input_ready(
                &observations,
                &claimed,
                &target(temp.path()),
                Uuid::new_v4(),
                false,
                PtyInputSourcePlane::HostCli,
                None,
                &heartbeat,
            )
            .await,
        Err(PtyInputReasonCode::Expired)
    );
    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        while !heartbeat.failed() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    claimed.expires_at = canonical_pty_timestamp(chrono::Utc::now() + chrono::Duration::minutes(1));
    assert_eq!(
        poller
            .wait_for_pty_input_ready(
                &observations,
                &claimed,
                &target(temp.path()),
                Uuid::new_v4(),
                false,
                PtyInputSourcePlane::HostCli,
                None,
                &heartbeat,
            )
            .await,
        Err(PtyInputReasonCode::LeaseLost)
    );
    assert_eq!(observations.accesses.load(Ordering::SeqCst), 0);
    assert!(!heartbeat.finish().await);
}

#[tokio::test]
async fn readiness_restore_blocks_later_getters_and_absence_reaches_authority() {
    let temp = tempfile::tempdir().unwrap();
    let store = Arc::new(MessageStore::open(temp.path().join("readiness.sqlite3")).unwrap());
    let claimed = claim(&store);
    let mut heartbeat = crate::api::message_store::PreparationHeartbeatGuard::start(
        Arc::clone(&store),
        claimed.injection_id.clone(),
        claimed.lease_owner.clone(),
        crate::phone::types::parse_canonical_pty_timestamp(&claimed.expires_at).unwrap(),
    );
    let poller = MailboxPoller::delivery_only_for_tests();
    for blocked in [true, false] {
        let observations = Observations {
            readiness_flow: true,
            sessions: Some(Arc::new(tokio::sync::RwLock::new(SessionManager::new()))),
            restore: blocked
                .then(|| Arc::new(RestoreInProgress(std::sync::atomic::AtomicBool::new(true)))),
            ..Default::default()
        };
        assert_eq!(
            poller
                .wait_for_pty_input_ready(
                    &observations,
                    &claimed,
                    &target(temp.path()),
                    Uuid::new_v4(),
                    false,
                    PtyInputSourcePlane::HostCli,
                    None,
                    &heartbeat,
                )
                .await,
            Err(if blocked {
                PtyInputReasonCode::RestoreInProgress
            } else {
                PtyInputReasonCode::AuthorityChanged
            })
        );
        assert_eq!(
            observations.accesses.load(Ordering::SeqCst),
            if blocked { 1 } else { 4 }
        );
    }
    assert!(heartbeat.finish().await);
}
