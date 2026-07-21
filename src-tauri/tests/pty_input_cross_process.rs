use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};

use agentscommander_lib::api::auth::{self, MintRequest, SCOPE_SEND};
use agentscommander_lib::api::message_store::{
    MessageStore, PtyInputEnqueueRequest, PtyInputTargetGate, DB_FILENAME,
};
use agentscommander_lib::phone::types::{
    canonical_pty_timestamp, pty_input_request_fingerprint, sha256_hex, PtyInputPublicStatus,
    PtyInputReasonCode, PtyInputSourcePlane,
};
use chrono::Utc;
use uuid::Uuid;

fn wait_for(path: &Path, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    while !path.exists() {
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {}",
            path.display()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn spawn_holder(test_name: &str, db: &Path, value: &str, ready: &Path, release: &Path) -> Child {
    Command::new(std::env::current_exe().expect("current test executable"))
        .arg("--exact")
        .arg(test_name)
        .arg("--nocapture")
        .env("AC_PTY_LOCK_DB", db)
        .env("AC_PTY_LOCK_VALUE", value)
        .env("AC_PTY_LOCK_READY", ready)
        .env("AC_PTY_LOCK_RELEASE", release)
        .spawn()
        .expect("spawn lock-holder child")
}

#[test]
fn child_mutates_auth_registry() {
    let Some(registry) = std::env::var_os("AC_AUTH_REGISTRY") else {
        return;
    };
    let client_id = std::env::var("AC_AUTH_CLIENT_ID").expect("auth client id");
    let action = std::env::var("AC_AUTH_ACTION").expect("auth action");
    let ready = PathBuf::from(std::env::var_os("AC_AUTH_READY").expect("auth ready"));
    let release = PathBuf::from(std::env::var_os("AC_AUTH_RELEASE").expect("auth release"));
    std::fs::write(&ready, b"ready").expect("write auth ready");
    wait_for(&release, Duration::from_secs(30));
    let registry = PathBuf::from(registry);
    match action.as_str() {
        "mint" => {
            auth::mint(
                &registry,
                MintRequest {
                    client_id,
                    secret: Uuid::new_v4().to_string(),
                    label: "manual concurrent test".to_string(),
                    bound_root: "C:/manual".to_string(),
                    bound_fqn: "project:wg-1-team/member".to_string(),
                    scopes: vec![SCOPE_SEND.to_string()],
                    issued_at: canonical_pty_timestamp(Utc::now()),
                    expires_at: None,
                    bound_session_id: None,
                    credential_generation: None,
                },
            )
            .expect("concurrent mint");
        }
        "revoke" => assert!(auth::revoke(&registry, &client_id).expect("concurrent revoke")),
        _ => panic!("unknown auth child action"),
    }
}

fn child_fixture() -> Option<(MessageStore, String, PathBuf, PathBuf)> {
    let db = std::env::var_os("AC_PTY_LOCK_DB")?;
    let value = std::env::var("AC_PTY_LOCK_VALUE").expect("lock value");
    let ready = PathBuf::from(std::env::var_os("AC_PTY_LOCK_READY").expect("ready path"));
    let release = PathBuf::from(std::env::var_os("AC_PTY_LOCK_RELEASE").expect("release path"));
    Some((
        MessageStore::open(PathBuf::from(db)).expect("child store"),
        value,
        ready,
        release,
    ))
}

#[test]
fn child_holds_operation_lock() {
    let Some((store, value, ready, release)) = child_fixture() else {
        return;
    };
    let _guard = store
        .try_operation_lock(&value)
        .expect("operation lock")
        .expect("operation lock acquired");
    std::fs::write(&ready, b"ready").expect("write ready");
    wait_for(&release, Duration::from_secs(30));
}

#[test]
fn child_holds_target_lock() {
    let Some((store, value, ready, release)) = child_fixture() else {
        return;
    };
    let _guard = store
        .try_target_lock(&value)
        .expect("target lock")
        .expect("target lock acquired");
    std::fs::write(&ready, b"ready").expect("write ready");
    wait_for(&release, Duration::from_secs(30));
}

#[test]
fn child_holds_create_gate_target_lock() {
    // Proves the create gate serializes across processes without any SQLite
    // store: this child holds the target stripe through a store-less
    // PtyInputTargetGate.
    let Some(root) = std::env::var_os("AC_GATE_ROOT") else {
        return;
    };
    let value = std::env::var("AC_GATE_VALUE").expect("gate value");
    let ready = PathBuf::from(std::env::var_os("AC_GATE_READY").expect("gate ready"));
    let release = PathBuf::from(std::env::var_os("AC_GATE_RELEASE").expect("gate release"));
    let gate = PtyInputTargetGate::new(PathBuf::from(root)).expect("child create gate");
    let _guard = gate
        .try_target_lock(&value)
        .expect("gate target lock")
        .expect("gate target lock acquired");
    std::fs::write(&ready, b"ready").expect("write gate ready");
    wait_for(&release, Duration::from_secs(30));
}

fn pty_request(injection_id: &str, now: chrono::DateTime<Utc>) -> PtyInputEnqueueRequest {
    let payload = b"cross-process exact input".to_vec();
    PtyInputEnqueueRequest {
        injection_id: injection_id.to_string(),
        sender_fqn: "project:wg-1-team/coordinator".to_string(),
        target_fqn: "project:wg-1-team/member".to_string(),
        op_id: injection_id.to_string(),
        nonce_sha256: sha256_hex(Uuid::new_v4().to_string().as_bytes()),
        request_fingerprint: pty_input_request_fingerprint(&[
            b"container_api",
            b"project:wg-1-team/coordinator",
            b"project:wg-1-team/member",
            b"1",
            b"agent-submit",
            sha256_hex(&payload).as_bytes(),
            &(payload.len() as u64).to_be_bytes(),
            b"",
        ]),
        confirmation_tag: None,
        requested_agent_id: None,
        payload,
        source_plane: PtyInputSourcePlane::ContainerApi,
        sender_incarnation_fingerprint: "a".repeat(64),
        sender_identity_fingerprint: "b".repeat(64),
        target_identity_fingerprint: "c".repeat(64),
        authority_session_id: Uuid::new_v4().to_string(),
        authority_client_id: Some("container-client".to_string()),
        authority_client_generation: Some(Uuid::new_v4().to_string()),
        issued_at: canonical_pty_timestamp(now),
        expires_at: canonical_pty_timestamp(now + chrono::Duration::minutes(10)),
    }
}

fn release_holder(mut child: Child, release: &Path) {
    std::fs::write(release, b"release").expect("release child");
    assert!(child.wait().expect("wait child").success());
}

fn spawn_operation_holder(directory: &Path, injection_id: &str) -> (Child, PathBuf, PathBuf) {
    let holder_db = directory.join("stripe-holder.sqlite3");
    let ready = directory.join(format!("ready-{injection_id}"));
    let release = directory.join(format!("release-{injection_id}"));
    let child = spawn_holder(
        "child_holds_operation_lock",
        &holder_db,
        injection_id,
        &ready,
        &release,
    );
    wait_for(&ready, Duration::from_secs(10));
    (child, ready, release)
}

fn assert_cross_process_exclusive(test_name: &str, target: bool) {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let db = dir.path().join(DB_FILENAME);
    let store = MessageStore::open(db.clone()).expect("parent store");
    let value = if target {
        "project:wg-1-team/member".to_string()
    } else {
        Uuid::new_v4().to_string()
    };
    let ready = dir.path().join("ready");
    let release = dir.path().join("release");
    let mut child = spawn_holder(test_name, &db, &value, &ready, &release);
    wait_for(&ready, Duration::from_secs(10));

    let blocked = if target {
        store.try_target_lock(&value).expect("parent target try")
    } else {
        store
            .try_operation_lock(&value)
            .expect("parent operation try")
    };
    assert!(
        blocked.is_none(),
        "second process must not acquire held stripe"
    );

    std::fs::write(&release, b"release").expect("release child");
    assert!(child.wait().expect("wait child").success());
    let acquired = if target {
        store.try_target_lock(&value).expect("target after release")
    } else {
        store
            .try_operation_lock(&value)
            .expect("operation after release")
    };
    assert!(
        acquired.is_some(),
        "stripe must become available after owner exits"
    );
}

#[test]
fn operation_stripe_blocks_a_second_process() {
    assert_cross_process_exclusive("child_holds_operation_lock", false);
}

#[test]
fn target_stripe_blocks_a_second_process() {
    assert_cross_process_exclusive("child_holds_target_lock", true);
}

#[test]
fn create_gate_target_stripe_blocks_a_second_process_without_a_store() {
    // The DB-independent create gate must serialize same-target creates across
    // processes even when the specialized SQLite store never opened. Neither the
    // holder child nor this parent constructs a MessageStore.
    let dir = tempfile::TempDir::new().expect("tempdir");
    let gate = PtyInputTargetGate::new(dir.path().to_path_buf()).expect("parent create gate");
    let value = "project:wg-1-team/member".to_string();
    let ready = dir.path().join("gate-ready");
    let release = dir.path().join("gate-release");

    let mut child = Command::new(std::env::current_exe().expect("current test executable"))
        .arg("--exact")
        .arg("child_holds_create_gate_target_lock")
        .arg("--nocapture")
        .env("AC_GATE_ROOT", dir.path())
        .env("AC_GATE_VALUE", &value)
        .env("AC_GATE_READY", &ready)
        .env("AC_GATE_RELEASE", &release)
        .spawn()
        .expect("spawn gate holder child");
    wait_for(&ready, Duration::from_secs(10));

    assert!(
        gate.try_target_lock(&value)
            .expect("parent gate target try")
            .is_none(),
        "a second process must not acquire the create gate stripe held by another owner"
    );

    std::fs::write(&release, b"release").expect("release gate child");
    assert!(child.wait().expect("wait gate child").success());
    assert!(
        gate.try_target_lock(&value)
            .expect("parent gate target after release")
            .is_some(),
        "the create gate stripe must become available after the owner exits"
    );
}

fn spawn_auth_mutator(
    registry: &Path,
    client_id: &str,
    action: &str,
    ready: &Path,
    release: &Path,
) -> Child {
    Command::new(std::env::current_exe().expect("current test executable"))
        .arg("--exact")
        .arg("child_mutates_auth_registry")
        .arg("--nocapture")
        .env("AC_AUTH_REGISTRY", registry)
        .env("AC_AUTH_CLIENT_ID", client_id)
        .env("AC_AUTH_ACTION", action)
        .env("AC_AUTH_READY", ready)
        .env("AC_AUTH_RELEASE", release)
        .spawn()
        .expect("spawn auth mutator")
}

fn manual_mint(registry: &Path, client_id: &str) {
    auth::mint(
        registry,
        MintRequest {
            client_id: client_id.to_string(),
            secret: Uuid::new_v4().to_string(),
            label: "manual concurrent test".to_string(),
            bound_root: "C:/manual".to_string(),
            bound_fqn: "project:wg-1-team/member".to_string(),
            scopes: vec![SCOPE_SEND.to_string()],
            issued_at: canonical_pty_timestamp(Utc::now()),
            expires_at: None,
            bound_session_id: None,
            credential_generation: None,
        },
    )
    .expect("manual mint");
}

#[test]
fn concurrent_registry_mint_and_revoke_have_no_lost_updates() {
    const CLIENTS: usize = 6;
    let dir = tempfile::TempDir::new().expect("tempdir");
    let registry = dir.path().join("api-clients.json");
    let release_mint = dir.path().join("release-mint");
    let mut mint_children = Vec::new();
    for index in 0..CLIENTS {
        let client_id = format!("concurrent-{index}");
        let ready = dir.path().join(format!("mint-ready-{index}"));
        let child = spawn_auth_mutator(&registry, &client_id, "mint", &ready, &release_mint);
        mint_children.push((child, ready));
    }
    for (_, ready) in &mint_children {
        wait_for(ready, Duration::from_secs(10));
    }
    std::fs::write(&release_mint, b"release").expect("release concurrent mint");
    for (mut child, _) in mint_children {
        assert!(child.wait().expect("wait mint child").success());
    }
    manual_mint(&registry, "strict-read-witness");
    let value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&registry).expect("read registry"))
            .expect("registry is complete JSON");
    let clients = value["clients"].as_array().expect("clients array");
    for index in 0..CLIENTS {
        assert!(clients
            .iter()
            .any(|client| client["clientId"] == format!("concurrent-{index}")
                && client["revoked"] == false));
    }

    let release_revoke = dir.path().join("release-revoke");
    let mut revoke_children = Vec::new();
    for index in 0..CLIENTS {
        let client_id = format!("concurrent-{index}");
        let ready = dir.path().join(format!("revoke-ready-{index}"));
        let child = spawn_auth_mutator(&registry, &client_id, "revoke", &ready, &release_revoke);
        revoke_children.push((child, ready));
    }
    for (_, ready) in &revoke_children {
        wait_for(ready, Duration::from_secs(10));
    }
    std::fs::write(&release_revoke, b"release").expect("release concurrent revoke");
    for (mut child, _) in revoke_children {
        assert!(child.wait().expect("wait revoke child").success());
    }
    manual_mint(&registry, "strict-read-after-revoke");
    let value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&registry).expect("read registry"))
            .expect("registry remains complete JSON");
    let clients = value["clients"].as_array().expect("clients array");
    for index in 0..CLIENTS {
        assert!(clients
            .iter()
            .any(|client| client["clientId"] == format!("concurrent-{index}")
                && client["revoked"] == true));
    }
}

#[test]
fn suspended_preparing_owner_blocks_recovery_until_release() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let db = dir.path().join(DB_FILENAME);
    let store = MessageStore::open(db).expect("store");
    let injection_id = Uuid::new_v4().to_string();
    let now = Utc::now();
    store
        .enqueue_pty_input(pty_request(&injection_id, now))
        .expect("enqueue");
    let claim_at = now + chrono::Duration::seconds(1);
    store
        .claim_pty_input(
            PtyInputSourcePlane::ContainerApi,
            Some(&injection_id),
            "preparing-owner",
            claim_at,
        )
        .expect("claim")
        .expect("claimed");

    let (child, _ready, release) = spawn_operation_holder(dir.path(), &injection_id);
    let recovered = store
        .recover_pty_input_runtime(
            &Default::default(),
            claim_at + chrono::Duration::seconds(121),
        )
        .expect("recovery while held");
    assert_eq!(recovered, 0);
    let held = store
        .query_pty_input_by_injection(&injection_id)
        .expect("query")
        .expect("row");
    assert_eq!(held.status, PtyInputPublicStatus::Queued);
    assert!(held.reason.is_none());

    release_holder(child, &release);
    let recovered = store
        .recover_pty_input_runtime(
            &Default::default(),
            claim_at + chrono::Duration::seconds(121),
        )
        .expect("recovery after release");
    assert_eq!(recovered, 1);
    let released = store
        .query_pty_input_by_injection(&injection_id)
        .expect("query")
        .expect("row");
    assert_eq!(released.status, PtyInputPublicStatus::Queued);
    assert_eq!(
        released.reason.expect("retry reason").code,
        PtyInputReasonCode::LeaseLost
    );
}

#[test]
fn suspended_actuating_owner_blocks_runtime_recovery_until_release() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let db = dir.path().join(DB_FILENAME);
    let store = MessageStore::open(db).expect("store");
    let injection_id = Uuid::new_v4().to_string();
    let now = Utc::now();
    store
        .enqueue_pty_input(pty_request(&injection_id, now))
        .expect("enqueue");
    let claim_at = now + chrono::Duration::seconds(1);
    store
        .claim_pty_input(
            PtyInputSourcePlane::ContainerApi,
            Some(&injection_id),
            "actuating-owner",
            claim_at,
        )
        .expect("claim")
        .expect("claimed");
    store
        .begin_pty_actuating(
            &injection_id,
            "actuating-owner",
            &Uuid::new_v4().to_string(),
            "localProcess",
            claim_at,
        )
        .expect("actuating");

    let (child, _ready, release) = spawn_operation_holder(dir.path(), &injection_id);
    let recovered = store
        .recover_pty_input_runtime(
            &Default::default(),
            claim_at + chrono::Duration::seconds(16),
        )
        .expect("recovery while held");
    assert_eq!(recovered, 0);
    assert_eq!(
        store
            .query_pty_input_by_injection(&injection_id)
            .expect("query")
            .expect("row")
            .status,
        PtyInputPublicStatus::Actuating
    );

    release_holder(child, &release);
    let recovered = store
        .recover_pty_input_runtime(
            &Default::default(),
            claim_at + chrono::Duration::seconds(16),
        )
        .expect("recovery after release");
    assert_eq!(recovered, 1);
    let result = store
        .query_pty_input_by_injection(&injection_id)
        .expect("query")
        .expect("row");
    assert_eq!(result.status, PtyInputPublicStatus::Indeterminate);
    assert_eq!(
        result.reason.expect("orphan reason").code,
        PtyInputReasonCode::RuntimeActuationOrphan
    );
}

#[test]
fn startup_recovery_skips_held_actuation_and_recovers_after_release() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let db = dir.path().join(DB_FILENAME);
    let store = MessageStore::open(db.clone()).expect("store");
    let injection_id = Uuid::new_v4().to_string();
    let now = Utc::now();
    store
        .enqueue_pty_input(pty_request(&injection_id, now))
        .expect("enqueue");
    let claim_at = Utc::now() + chrono::Duration::milliseconds(1);
    store
        .claim_pty_input(
            PtyInputSourcePlane::ContainerApi,
            Some(&injection_id),
            "startup-owner",
            claim_at,
        )
        .expect("claim")
        .expect("claimed");
    store
        .begin_pty_actuating(
            &injection_id,
            "startup-owner",
            &Uuid::new_v4().to_string(),
            "containerTransport",
            claim_at,
        )
        .expect("actuating");

    let (child, _ready, release) = spawn_operation_holder(dir.path(), &injection_id);
    let observer = MessageStore::open(db.clone()).expect("observer startup");
    assert_eq!(
        observer
            .query_pty_input_by_injection(&injection_id)
            .expect("query")
            .expect("row")
            .status,
        PtyInputPublicStatus::Actuating
    );

    release_holder(child, &release);
    drop(observer);
    let recovered = MessageStore::open(db).expect("recovery startup");
    let result = recovered
        .query_pty_input_by_injection(&injection_id)
        .expect("query")
        .expect("row");
    assert_eq!(result.status, PtyInputPublicStatus::Indeterminate);
    assert_eq!(
        result.reason.expect("restart reason").code,
        PtyInputReasonCode::DaemonRestartAfterActuation
    );
}
