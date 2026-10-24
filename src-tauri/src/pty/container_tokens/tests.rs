use super::*;

#[test]
fn mint_stores_hash_only_with_container_scopes_and_revoke_marks_client() {
    let dir = tempfile::TempDir::new().unwrap();
    let manager = ContainerApiTokenManager::new_for_path(dir.path().join("clients.json"));
    let session_id = Uuid::new_v4();

    let token = manager
        .mint_for_session(session_id, "C:/project/.ac/wg-1-team/__agent_dev")
        .expect("mint");

    let raw = std::fs::read_to_string(manager.path()).expect("registry");
    assert!(!raw.contains(&token.secret));
    let registry = auth::list(manager.path());
    let client = registry
        .clients
        .iter()
        .find(|client| client.client_id == token.client_id)
        .expect("client");
    assert_eq!(client.label, format!("container:{}", session_id));
    assert!(client.scopes.contains(&SCOPE_SEND.to_string()));
    assert!(client.scopes.contains(&SCOPE_LIST_PEERS.to_string()));
    assert!(client.scopes.contains(&SCOPE_SESSION_TRANSPORT.to_string()));
    assert!(client.expires_at.is_some());

    manager.revoke(&token.client_id);
    let registry = auth::list(manager.path());
    assert!(registry.clients[0].revoked);
}

#[test]
fn revoke_all_container_clients_only_revokes_container_labels() {
    let dir = tempfile::TempDir::new().unwrap();
    let manager = ContainerApiTokenManager::new_for_path(dir.path().join("clients.json"));
    let one = manager
        .mint_for_session(Uuid::new_v4(), "C:/project/.ac/wg-1-team/__agent_dev")
        .unwrap();
    auth::mint(
        manager.path(),
        MintRequest {
            client_id: "manual".to_string(),
            secret: "manual-secret".to_string(),
            label: "manual".to_string(),
            bound_root: "C:/project/.ac/wg-1-team/__agent_dev".to_string(),
            bound_fqn: "project:wg-1-team/dev".to_string(),
            scopes: vec![SCOPE_SEND.to_string()],
            issued_at: Utc::now().to_rfc3339(),
            expires_at: None,
            bound_session_id: None,
            credential_generation: None,
        },
    )
    .unwrap();

    assert_eq!(manager.revoke_all_container_clients().unwrap(), 1);
    let registry = auth::list(manager.path());
    assert!(
        registry
            .clients
            .iter()
            .find(|client| client.client_id == one.client_id)
            .unwrap()
            .revoked
    );
    assert!(
        !registry
            .clients
            .iter()
            .find(|client| client.client_id == "manual")
            .unwrap()
            .revoked
    );
}

fn manual_client(label: &str, expires_at: Option<String>) -> MintRequest {
    MintRequest {
        client_id: format!("client-{}", Uuid::new_v4()),
        secret: "manual-secret".to_string(),
        label: label.to_string(),
        bound_root: "C:/project/.ac/wg-1-team/__agent_dev".to_string(),
        bound_fqn: "project:wg-1-team/dev".to_string(),
        scopes: vec![SCOPE_SEND.to_string()],
        issued_at: Utc::now().to_rfc3339(),
        expires_at,
        bound_session_id: None,
        credential_generation: None,
    }
}

#[test]
fn has_container_clients_is_false_when_registry_is_absent() {
    // The fresh-install path. If this ever returns true, every install pays a
    // load-bearing sweep forever.
    let dir = tempfile::TempDir::new().unwrap();
    let manager = ContainerApiTokenManager::new_for_path(dir.path().join("clients.json"));
    assert!(!manager.has_container_clients().unwrap());
}

#[test]
fn has_container_clients_is_true_after_minting_for_a_session() {
    let dir = tempfile::TempDir::new().unwrap();
    let manager = ContainerApiTokenManager::new_for_path(dir.path().join("clients.json"));
    manager
        .mint_for_session(Uuid::new_v4(), "C:/project/.ac/wg-1-team/__agent_dev")
        .unwrap();
    assert!(manager.has_container_clients().unwrap());
}

#[test]
fn has_container_clients_stays_true_after_revoke_all_container_clients() {
    // Regression test for the stranding trap: the startup revoke runs even when the
    // sweep failed, so a marker that counted only NON-revoked clients would be
    // cleared by a start with Docker down. Do not delete this test.
    let dir = tempfile::TempDir::new().unwrap();
    let manager = ContainerApiTokenManager::new_for_path(dir.path().join("clients.json"));
    manager
        .mint_for_session(Uuid::new_v4(), "C:/project/.ac/wg-1-team/__agent_dev")
        .unwrap();
    manager.revoke_all_container_clients().unwrap();
    assert!(manager.has_container_clients().unwrap());
}

/// Assert the registry really holds what a test claims it does. The first cut of
/// these tests minted through `mint_for_session`, which stamps +24h, so they never
/// expired anything and would have stayed green against a predicate that was broken
/// on exactly the state they were named for.
fn assert_client_is_expired(manager: &ContainerApiTokenManager, client_id: &str, revoked: bool) {
    let registry = auth::list(manager.path());
    let client = registry
        .clients
        .iter()
        .find(|client| client.client_id == client_id)
        .expect("the client the test minted");
    let expiry =
        chrono::DateTime::parse_from_rfc3339(client.expires_at.as_deref().expect("expires_at"))
            .expect("rfc3339 expiry")
            .with_timezone(&Utc);
    assert!(
        expiry < Utc::now(),
        "this test must actually expire the client"
    );
    assert_eq!(
        client.revoked, revoked,
        "this test must leave the client revoked={revoked}"
    );
}

fn mint_expired_container_client(manager: &ContainerApiTokenManager) -> String {
    let client_id = format!("container-{}", Uuid::new_v4());
    let mut request = manual_client(
        &format!("{}{}", CONTAINER_LABEL_PREFIX, Uuid::new_v4()),
        Some((Utc::now() - ChronoDuration::hours(48)).to_rfc3339()),
    );
    request.client_id = client_id.clone();
    request.issued_at = (Utc::now() - ChronoDuration::hours(72)).to_rfc3339();
    auth::mint(manager.path(), request).unwrap();
    client_id
}

#[test]
fn has_container_clients_stays_true_for_an_expired_client() {
    // Container tokens live 24h (CONTAINER_TOKEN_TTL_HOURS), so an expired entry is
    // the normal state of an ex-container user. `auth::list` does not filter expired
    // entries; `authenticate` does. If someone ever harmonizes the two, this fails
    // before the marker silently flips false for every ex-container user.
    // `mint_for_session` cannot be used here: it stamps +24h.
    let dir = tempfile::TempDir::new().unwrap();
    let manager = ContainerApiTokenManager::new_for_path(dir.path().join("clients.json"));
    let client_id = mint_expired_container_client(&manager);

    assert_client_is_expired(&manager, &client_id, false);
    assert!(manager.has_container_clients().unwrap());
}

#[test]
fn has_container_clients_stays_true_for_an_expired_and_revoked_client() {
    // The ex-container user's REAL steady state: the token expired after 24h AND a
    // later startup revoked it. A predicate such as
    // `prefix && (!client.revoked || !is_expired(client))` passes the fresh+revoked
    // test and the expired-only test above, yet answers false here, silently
    // downgrading every ex-container user's sweep. That must not be possible.
    let dir = tempfile::TempDir::new().unwrap();
    let manager = ContainerApiTokenManager::new_for_path(dir.path().join("clients.json"));
    let client_id = mint_expired_container_client(&manager);
    manager.revoke(&client_id);

    assert_client_is_expired(&manager, &client_id, true);
    assert!(manager.has_container_clients().unwrap());
}

#[test]
fn has_container_clients_errors_on_a_malformed_registry() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("clients.json");
    std::fs::write(&path, "{").unwrap();
    let manager = ContainerApiTokenManager::new_for_path(path);
    assert!(manager.has_container_clients().is_err());
}

#[test]
fn has_container_clients_ignores_non_container_clients() {
    let dir = tempfile::TempDir::new().unwrap();
    let manager = ContainerApiTokenManager::new_for_path(dir.path().join("clients.json"));
    auth::mint(manager.path(), manual_client("cli:some-agent", None)).unwrap();
    assert!(!manager.has_container_clients().unwrap());
}

#[test]
fn revoke_all_container_clients_is_a_noop_on_an_empty_registry() {
    // Pins the claim that a sweep which skips revocation skips nothing.
    let dir = tempfile::TempDir::new().unwrap();
    let manager = ContainerApiTokenManager::new_for_path(dir.path().join("clients.json"));
    assert_eq!(manager.revoke_all_container_clients().unwrap(), 0);
}
