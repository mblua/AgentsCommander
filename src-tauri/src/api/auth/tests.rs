use super::*;
use std::net::Ipv4Addr;

fn client(hash: &str, revoked: bool, expires: Option<&str>) -> ApiClient {
    ApiClient {
        client_id: "c1".into(),
        label: "l".into(),
        token_hash: hash.into(),
        bound_fqn: "proj:wg-1/dev".into(),
        bound_root: "C:/root".into(),
        scopes: vec![
            SCOPE_SEND.into(),
            SCOPE_LIST_PEERS.into(),
            SCOPE_SESSION_TRANSPORT.into(),
        ],
        issued_at: "2026-01-01T00:00:00Z".into(),
        expires_at: expires.map(|s| s.to_string()),
        revoked,
        bound_session_id: None,
        credential_generation: None,
    }
}

#[test]
fn hash_is_deterministic_and_prefixed() {
    let h = hash_token("secret-abc");
    assert!(h.starts_with("sha256:"));
    assert_eq!(h, hash_token("secret-abc"));
    assert_ne!(h, hash_token("secret-abd"));
    // sha256 hex is 64 chars after the prefix.
    assert_eq!(h.len(), "sha256:".len() + 64);
}

#[test]
fn hash_never_contains_plaintext() {
    let secret = "super-secret-value";
    let h = hash_token(secret);
    assert!(!h.contains(secret));
}

#[test]
fn constant_time_eq_matches_semantics() {
    assert!(constant_time_eq("abc", "abc"));
    assert!(!constant_time_eq("abc", "abd"));
    assert!(!constant_time_eq("abc", "abcd"));
}

#[test]
fn expiry_fail_closed_on_garbage() {
    let now = chrono::Utc::now();
    assert!(!is_expired(&client("h", false, None), now));
    assert!(is_expired(&client("h", false, Some("not-a-date")), now));
    assert!(is_expired(
        &client("h", false, Some("2000-01-01T00:00:00Z")),
        now
    ));
    assert!(!is_expired(
        &client("h", false, Some("2099-01-01T00:00:00Z")),
        now
    ));
}

#[test]
fn validate_scopes_rejects_unknown_and_empty() {
    assert!(validate_scopes(&[]).is_err());
    assert!(validate_scopes(&["send".into()]).is_ok());
    assert!(validate_scopes(&["send".into(), "list-peers-lean".into()]).is_ok());
    assert!(validate_scopes(&["session-transport".into()]).is_ok());
    assert!(validate_scopes(&["close-session".into()]).is_err());
}

#[test]
fn mint_stores_hash_not_plaintext_and_authenticate_reads_through() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join(REGISTRY_FILENAME);
    let out = mint(
        &path,
        MintRequest {
            client_id: "client-1".into(),
            secret: "the-secret".into(),
            label: "docker:test".into(),
            bound_root: "C:/root/replica".into(),
            bound_fqn: "proj:wg-1/dev".into(),
            scopes: vec![SCOPE_SEND.into()],
            issued_at: "2026-01-01T00:00:00Z".into(),
            expires_at: None,
            bound_session_id: None,
            credential_generation: None,
        },
    )
    .unwrap();
    assert_eq!(out.secret, "the-secret");

    let raw = std::fs::read_to_string(&path).unwrap();
    assert!(
        !raw.contains("the-secret"),
        "plaintext must not be persisted"
    );
    assert!(raw.contains("sha256:"));

    // A fresh store (empty cache) reads through to disk on first auth.
    let store = ApiClientStore::new(path.clone());
    assert!(store.authenticate("the-secret").unwrap().is_some());
    assert!(store.authenticate("wrong-secret").unwrap().is_none());
}

#[test]
fn revoke_takes_effect_on_next_read_through() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join(REGISTRY_FILENAME);
    mint(
        &path,
        MintRequest {
            client_id: "client-1".into(),
            secret: "s".into(),
            label: "l".into(),
            bound_root: "C:/root".into(),
            bound_fqn: "proj:wg-1/dev".into(),
            scopes: vec![SCOPE_SEND.into()],
            issued_at: "2026-01-01T00:00:00Z".into(),
            expires_at: None,
            bound_session_id: None,
            credential_generation: None,
        },
    )
    .unwrap();
    let store = ApiClientStore::new(path.clone());
    assert!(store.authenticate("s").unwrap().is_some());

    // Revoke via the (separate-process-equivalent) file write.
    assert!(revoke(&path, "client-1").unwrap());
    // Read-through picks up the change (mtime advanced).
    assert!(
        store.authenticate("s").unwrap().is_none(),
        "revocation must take effect on the next read-through"
    );
}

#[test]
fn store_authenticate_skips_revoked_and_expired() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join(REGISTRY_FILENAME);
    let reg = ApiClientRegistry {
        version: 1,
        clients: vec![
            client(&hash_token("revoked-tok"), true, None),
            client(
                &hash_token("expired-tok"),
                false,
                Some("2000-01-01T00:00:00Z"),
            ),
        ],
    };
    std::fs::write(&path, serde_json::to_string_pretty(&reg).unwrap()).unwrap();
    let store = ApiClientStore::new(path);
    assert!(store.authenticate("revoked-tok").unwrap().is_none());
    assert!(store.authenticate("expired-tok").unwrap().is_none());
}

#[test]
fn privileged_registry_rejects_a_dangling_link_when_supported() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join(REGISTRY_FILENAME);
    let missing = dir.path().join("missing-registry.json");
    #[cfg(unix)]
    let linked = std::os::unix::fs::symlink(&missing, &path).is_ok();
    #[cfg(windows)]
    let linked = std::os::windows::fs::symlink_file(&missing, &path).is_ok();
    #[cfg(not(any(unix, windows)))]
    let linked = false;
    if linked {
        assert!(read_registry_strict(&path).is_err());
        let store = ApiClientStore::new(path);
        assert!(store.authenticate_pty_input_fresh("anything").is_err());
    }
}

#[test]
fn api_client_debug_redacts_credential_and_bound_path() {
    let mut value = client(&hash_token("credential-sentinel"), false, None);
    value.bound_root = "C:/bound-path-sentinel".to_string();
    let rendered = format!("{value:?}");
    assert!(!rendered.contains("credential-sentinel"));
    assert!(!rendered.contains(&value.token_hash));
    assert!(!rendered.contains("bound-path-sentinel"));
}

#[test]
fn malformed_registry_snapshot_is_fail_closed_and_visible() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join(REGISTRY_FILENAME);
    std::fs::write(&path, "{ invalid").unwrap();

    let snapshot = list_with_status(&path);

    assert!(snapshot.registry.clients.is_empty());
    let problem = snapshot.problem.expect("malformed registry is reported");
    assert_eq!(problem.status, "malformed");
    assert!(problem.message.contains(REGISTRY_FILENAME), "{:?}", problem);
}

#[test]
fn poisoned_registry_cache_returns_internal_error() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join(REGISTRY_FILENAME);
    let store = ApiClientStore::new(path);

    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _guard = store.cache.lock().unwrap();
        panic!("poison registry cache");
    }));

    let err = store.authenticate("anything").unwrap_err();
    assert!(matches!(err, ApiError::Internal(_)));
}

#[test]
fn automatic_generation_compaction_preserves_manual_and_one_history_witness() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join(REGISTRY_FILENAME);
    mint(
        &path,
        MintRequest {
            client_id: "container-manual-name".into(),
            secret: "manual-secret".into(),
            label: "container:manual".into(),
            bound_root: "C:/manual".into(),
            bound_fqn: "project:wg-1-team/manual".into(),
            scopes: vec![SCOPE_SEND.into()],
            issued_at: chrono::Utc::now().to_rfc3339(),
            expires_at: None,
            bound_session_id: None,
            credential_generation: None,
        },
    )
    .unwrap();
    let session_id = uuid::Uuid::new_v4().to_string();
    for index in 0..3 {
        let generation = uuid::Uuid::new_v4().to_string();
        mint(
            &path,
            MintRequest {
                client_id: format!("container-{session_id}-{generation}"),
                secret: format!("automatic-secret-{index}"),
                label: format!("container:{session_id}"),
                bound_root: "C:/automatic".into(),
                bound_fqn: "project:wg-1-team/coordinator".into(),
                scopes: vec![SCOPE_SEND.into(), SCOPE_PTY_INPUT.into()],
                issued_at: chrono::Utc::now().to_rfc3339(),
                expires_at: None,
                bound_session_id: Some(session_id.clone()),
                credential_generation: Some(generation),
            },
        )
        .unwrap();
    }
    let registry = read_registry_strict(&path).unwrap();
    assert!(registry
        .clients
        .iter()
        .any(|client| client.client_id == "container-manual-name" && !client.revoked));
    let automatic: Vec<_> = registry
        .clients
        .iter()
        .filter(|client| automatic_client(client))
        .collect();
    assert_eq!(automatic.len(), 2, "one live generation plus one witness");
    assert_eq!(automatic.iter().filter(|client| !client.revoked).count(), 1);
}

#[tokio::test]
async fn privileged_registry_lock_wait_is_offloaded_from_async_ingress() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join(REGISTRY_FILENAME);
    let lock = open_registry_lock(dir.path()).unwrap();
    lock.lock().unwrap();
    let store = std::sync::Arc::new(ApiClientStore::new(path));
    let waiting = {
        let store = std::sync::Arc::clone(&store);
        tokio::spawn(async move {
            store
                .authenticate_pty_input_fresh_offloaded("not-a-secret".to_string())
                .await
        })
    };

    tokio::time::timeout(Duration::from_millis(100), async {
        tokio::time::sleep(Duration::from_millis(10)).await;
    })
    .await
    .expect("the async executor must progress while the file lock is held");
    assert!(matches!(
        waiting.await.unwrap(),
        Err(FreshRegistryError::Contended)
    ));
}

#[tokio::test]
async fn dispatch_and_final_binding_reads_preserve_registry_contention() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join(REGISTRY_FILENAME);
    let lock = open_registry_lock(dir.path()).unwrap();
    lock.lock().unwrap();
    let store = std::sync::Arc::new(ApiClientStore::new(path));
    let waiting = {
        let store = std::sync::Arc::clone(&store);
        tokio::spawn(async move {
            store
                .load_active_binding_fresh_offloaded(
                    "container-client".to_string(),
                    uuid::Uuid::new_v4().to_string(),
                    SCOPE_PTY_INPUT,
                )
                .await
        })
    };

    tokio::time::timeout(Duration::from_millis(100), async {
        tokio::time::sleep(Duration::from_millis(10)).await;
    })
    .await
    .expect("binding lookup must not block the async executor");
    assert!(matches!(
        waiting.await.unwrap(),
        Err(FreshRegistryError::Contended)
    ));
}

#[test]
fn privileged_registry_contention_is_retry_class_not_stale() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join(REGISTRY_FILENAME);
    let lock = open_registry_lock(dir.path()).unwrap();
    lock.lock().unwrap();
    let store = ApiClientStore::new(path);

    let error = match store.authenticate_pty_input_fresh("not-a-secret") {
        Err(error) => error,
        Ok(_) => panic!("a held registry lock must report contention"),
    };

    assert_eq!(
        error,
        FreshRegistryError::Contended,
        "lock contention is retryable and must never be classified as stale authority"
    );
}

#[test]
fn fresh_privileged_auth_observes_revocation_without_cache() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join(REGISTRY_FILENAME);
    let session_id = uuid::Uuid::new_v4().to_string();
    let generation = uuid::Uuid::new_v4().to_string();
    mint(
        &path,
        MintRequest {
            client_id: format!("container-{session_id}-{generation}"),
            secret: "fresh-secret".into(),
            label: format!("container:{session_id}"),
            bound_root: "C:/automatic".into(),
            bound_fqn: "project:wg-1-team/coordinator".into(),
            scopes: vec![SCOPE_PTY_INPUT.into()],
            issued_at: chrono::Utc::now().to_rfc3339(),
            expires_at: None,
            bound_session_id: Some(session_id),
            credential_generation: Some(generation),
        },
    )
    .unwrap();
    let store = ApiClientStore::new(path.clone());
    assert!(store
        .authenticate_pty_input_fresh("fresh-secret")
        .unwrap()
        .is_some());
    let client_id = read_registry_strict(&path).unwrap().clients[0]
        .client_id
        .clone();
    assert!(revoke(&path, &client_id).unwrap());
    assert!(store
        .authenticate_pty_input_fresh("fresh-secret")
        .unwrap()
        .is_none());
}

#[test]
fn lockout_triggers_after_threshold_and_check_rejects() {
    let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let lock = FailedAuthLockout::new(3, Duration::from_secs(10), Duration::from_secs(60));
    let t0 = Instant::now();
    assert!(lock.check_at(ip, t0).is_ok());
    lock.record_failure_at(ip, t0).unwrap();
    lock.record_failure_at(ip, t0).unwrap();
    assert!(lock.check_at(ip, t0).is_ok(), "below threshold: allowed");
    lock.record_failure_at(ip, t0).unwrap();
    assert!(
        lock.check_at(ip, t0).is_err(),
        "at threshold: source is locked"
    );
    // After lockout expires, allowed again.
    assert!(lock.check_at(ip, t0 + Duration::from_secs(61)).is_ok());
}

#[test]
fn lockout_success_clears_history() {
    let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let lock = FailedAuthLockout::new(3, Duration::from_secs(10), Duration::from_secs(60));
    let t0 = Instant::now();
    lock.record_failure_at(ip, t0).unwrap();
    lock.record_failure_at(ip, t0).unwrap();
    lock.record_success(ip).unwrap();
    lock.record_failure_at(ip, t0).unwrap();
    lock.record_failure_at(ip, t0).unwrap();
    assert!(
        lock.check_at(ip, t0).is_ok(),
        "success reset the counter, so 2 more failures stay below threshold"
    );
}

#[test]
fn poisoned_lockout_returns_internal_error() {
    let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let lock = FailedAuthLockout::default();

    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _guard = lock.inner.lock().unwrap();
        panic!("poison lockout");
    }));

    let err = lock.check(ip).unwrap_err();
    assert!(matches!(err, ApiError::Internal(_)));
    let err = lock.record_failure(ip).unwrap_err();
    assert!(matches!(err, ApiError::Internal(_)));
    let err = lock.record_success(ip).unwrap_err();
    assert!(matches!(err, ApiError::Internal(_)));
}
