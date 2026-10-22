use super::*;

// #552: a WG replica cwd that agent_fqn_from_path resolves to
// `<project>:<wg>/<agent>`. team key = `<project>:<wg>`.
const COORD_CWD: &str = "C:\\repos\\myproj\\.ac\\wg-1-team\\__agent_lead";
const RUST_CWD: &str = "C:\\repos\\myproj\\.ac\\wg-1-team\\__agent_rust";
const TEAM_KEY: &str = "myproj:wg-1-team";

#[tokio::test]
async fn coordinator_cwd_returns_cwd_only_for_coordinators() {
    let mgr = SessionManager::new();
    let coord = mgr
        .create_session(
            "claude".into(),
            vec![],
            COORD_CWD.into(),
            None,
            None,
            vec![],
            true, // is_coordinator
            crate::pty::backend::SessionBackendKind::LocalProcess,
        )
        .await
        .unwrap();
    let plain = mgr
        .create_session(
            "powershell.exe".into(),
            vec![],
            "C:\\tmp".into(),
            None,
            None,
            vec![],
            false,
            crate::pty::backend::SessionBackendKind::LocalProcess,
        )
        .await
        .unwrap();

    assert_eq!(
        mgr.coordinator_cwd(coord.id).await.as_deref(),
        Some(COORD_CWD)
    );
    assert!(mgr.coordinator_cwd(plain.id).await.is_none());
    assert!(mgr.coordinator_cwd(Uuid::new_v4()).await.is_none());
}

#[tokio::test]
async fn agent_team_members_includes_agent_owned_excludes_adhoc() {
    let mgr = SessionManager::new();
    let coord = mgr
        .create_session(
            "claude".into(),
            vec![],
            COORD_CWD.into(),
            None,
            None,
            vec![],
            true,
            crate::pty::backend::SessionBackendKind::LocalProcess,
        )
        .await
        .unwrap();
    // agent_id set, not a coordinator -> still agent-owned.
    let rust = mgr
        .create_session(
            "codex".into(),
            vec![],
            RUST_CWD.into(),
            Some("codex".into()),
            None,
            vec![],
            false,
            crate::pty::backend::SessionBackendKind::LocalProcess,
        )
        .await
        .unwrap();
    // ad-hoc user shell: no agent_id, not a coordinator -> excluded.
    let _shell = mgr
        .create_session(
            "powershell.exe".into(),
            vec![],
            "C:\\repos\\myproj\\.ac\\wg-1-team\\scratch".into(),
            None,
            None,
            vec![],
            false,
            crate::pty::backend::SessionBackendKind::LocalProcess,
        )
        .await
        .unwrap();

    let members: std::collections::HashMap<Uuid, String> =
        mgr.agent_team_members().await.into_iter().collect();

    assert_eq!(members.len(), 2, "only the two agent-owned sessions");
    assert_eq!(members.get(&coord.id).map(String::as_str), Some(TEAM_KEY));
    assert_eq!(members.get(&rust.id).map(String::as_str), Some(TEAM_KEY));
}

#[tokio::test]
async fn coordinator_refs_by_team_maps_team_to_coordinator() {
    let mgr = SessionManager::new();
    let coord = mgr
        .create_session(
            "claude".into(),
            vec![],
            COORD_CWD.into(),
            None,
            None,
            vec![],
            true,
            crate::pty::backend::SessionBackendKind::LocalProcess,
        )
        .await
        .unwrap();
    // A non-coordinator agent must NOT appear in the refs map.
    let _rust = mgr
        .create_session(
            "codex".into(),
            vec![],
            RUST_CWD.into(),
            Some("codex".into()),
            None,
            vec![],
            false,
            crate::pty::backend::SessionBackendKind::LocalProcess,
        )
        .await
        .unwrap();

    let refs = mgr.coordinator_refs_by_team().await;
    assert_eq!(refs.len(), 1, "exactly one coordinator team");
    let (fqn, cwd) = refs.get(TEAM_KEY).expect("team key present");
    assert_eq!(fqn, "myproj:wg-1-team/lead");
    assert_eq!(cwd, COORD_CWD);
    let _ = coord;
}

#[tokio::test]
async fn coordinator_ids_by_team_maps_team_to_coordinator_id() {
    let mgr = SessionManager::new();
    let coord = mgr
        .create_session(
            "claude".into(),
            vec![],
            COORD_CWD.into(),
            None,
            None,
            vec![],
            true,
            crate::pty::backend::SessionBackendKind::LocalProcess,
        )
        .await
        .unwrap();
    // A non-coordinator agent on the same team must NOT appear (only the coordinator).
    let _rust = mgr
        .create_session(
            "codex".into(),
            vec![],
            RUST_CWD.into(),
            Some("codex".into()),
            None,
            vec![],
            false,
            crate::pty::backend::SessionBackendKind::LocalProcess,
        )
        .await
        .unwrap();

    let ids = mgr.coordinator_ids_by_team().await;
    assert_eq!(ids.len(), 1, "exactly one coordinator team");
    assert_eq!(
        ids.get(TEAM_KEY),
        Some(&coord.id),
        "team key maps to the coordinator session id, not a member"
    );
}
