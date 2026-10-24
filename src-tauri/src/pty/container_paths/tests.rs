use super::*;

fn map() -> ContainerPathMap {
    ContainerPathMap::new(r"C:\Users\maria\repo\.ac\wg-1\__agent_x", "/workspace").unwrap()
}

fn norm(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

#[test]
fn to_container_maps_windows_host_path_to_workspace_path() {
    assert_eq!(
        map().to_container(r"C:\Users\maria\repo\.ac\wg-1\__agent_x\.claude"),
        Some("/workspace/.claude".to_string())
    );
}

#[test]
fn to_container_maps_root_itself() {
    assert_eq!(
        map().to_container(r"C:\Users\maria\repo\.ac\wg-1\__agent_x"),
        Some("/workspace".to_string())
    );
    assert_eq!(
        map().to_container(r"C:\Users\maria\repo\.ac\wg-1\__agent_x\"),
        Some("/workspace".to_string())
    );
}

#[test]
fn to_container_rejects_parent_and_prefix_siblings() {
    assert_eq!(
        map().to_container(r"C:\Users\maria\repo\.ac\wg-1\.claude"),
        None
    );
    assert_eq!(
        map().to_container(r"C:\Users\maria\repo\.ac\wg-1\__agent_x-evil\.claude"),
        None
    );
}

#[test]
fn to_container_preserves_remainder_case() {
    assert_eq!(
        map().to_container(r"C:\Users\maria\repo\.ac\wg-1\__agent_x\MyCodex"),
        Some("/workspace/MyCodex".to_string())
    );
}

#[test]
fn to_container_rejects_dot_segments_markers_and_non_absolute_paths() {
    let map = map();
    assert_eq!(
        map.to_container(r"C:\Users\maria\repo\.ac\wg-1\__agent_x\..\x"),
        None
    );
    assert_eq!(
        map.to_container(r"C:\Users\maria\repo\.ac\wg-1\__agent_x\.\x"),
        None
    );
    assert_eq!(map.to_container(r"%AC_REPLICA_ROOT%\.claude"), None);
    assert_eq!(map.to_container(r"$env:AC_REPLICA_ROOT\.claude"), None);
    assert_eq!(map.to_container(r"$AC_REPLICA_ROOT/.claude"), None);
    assert_eq!(map.to_container(r"C:.claude"), None);
    assert_eq!(map.to_container(r"\.claude"), None);
}

#[test]
fn to_container_accepts_forward_slashes_and_strips_verbatim_prefixes_on_values() {
    let map = map();
    assert_eq!(
        map.to_container("C:/Users/maria/repo/.ac/wg-1/__agent_x/.claude"),
        Some("/workspace/.claude".to_string())
    );
    assert_eq!(
        map.to_container(r"\\?\C:\Users\maria\repo\.ac\wg-1\__agent_x\.claude"),
        Some("/workspace/.claude".to_string())
    );
}

#[test]
fn new_rejects_unc_host_root() {
    assert!(ContainerPathMap::new(r"\\server\share\repo", "/workspace").is_err());
    assert!(ContainerPathMap::new(r"\\?\UNC\server\share\repo", "/workspace").is_err());
}

#[test]
fn to_host_round_trips_container_path() {
    let got = map()
        .to_host("/workspace/.claude/projects/-workspace")
        .unwrap();
    assert!(norm(&got).ends_with("/__agent_x/.claude/projects/-workspace"));
}

#[test]
fn is_container_path_identifies_workspace_paths() {
    let map = map();
    assert!(map.is_container_path("/workspace/.claude"));
    assert!(map.is_container_path("/workspace"));
    assert!(!map.is_container_path("/workspace-evil/.claude"));
}

#[test]
fn classify_container_env_matches_host_platform_key_rules() {
    assert_eq!(
        classify_container_env("CLAUDE_CONFIG_DIR"),
        ContainerEnvClass::HostPathTranslate
    );
    assert_eq!(
        classify_container_env("CODEX_HOME"),
        ContainerEnvClass::HostPathTranslate
    );
    assert_eq!(
        classify_container_env("OPENCODE_CONFIG_DIR"),
        ContainerEnvClass::HostPathTranslate
    );
    assert_eq!(classify_container_env("MY_VAR"), ContainerEnvClass::Opaque);

    if cfg!(windows) {
        assert_eq!(
            classify_container_env("claude_config_dir"),
            ContainerEnvClass::HostPathTranslate
        );
    } else {
        assert_eq!(
            classify_container_env("claude_config_dir"),
            ContainerEnvClass::Opaque
        );
    }
}

#[test]
fn host_path_keys_have_pinned_validation_regimes() {
    assert_eq!(
        host_path_env_validation_regime("CODEX_HOME"),
        Some(ContainerEnvValidationRegime::StrictAbsolutePath)
    );
    assert_eq!(
        host_path_env_validation_regime("CLAUDE_CONFIG_DIR"),
        Some(ContainerEnvValidationRegime::AllowsUnexpandedMarkers)
    );
    assert_eq!(
        host_path_env_validation_regime("OPENCODE_CONFIG_DIR"),
        Some(ContainerEnvValidationRegime::AllowsUnexpandedMarkers)
    );
}

#[test]
fn claude_config_dir_warnings_have_pinned_kinds_and_messages() {
    let map = map();

    let container_path = claude_config_dir_unmappable_warning(&map, "/workspace/.claude");
    assert_eq!(container_path.key, CLAUDE_CONFIG_DIR_KEY);
    assert_eq!(
        container_path.kind,
        WARNING_KIND_CONTAINER_PATH_IN_HOST_FIELD
    );
    assert_eq!(
        container_path.message,
        "container session: CLAUDE_CONFIG_DIR='/workspace/.claude' looks like a container path. \
         Path env rows in AgentsCommander are host-side values; AgentsCommander translates them \
         for container sessions. This value was unset in the container, so conversation state \
         will not persist and auto-resume is skipped. Set \
         CLAUDE_CONFIG_DIR=%AC_REPLICA_ROOT%\\.claude (it resolves to /workspace/.claude inside \
         the container)."
    );

    let outside = claude_config_dir_unmappable_warning(&map, r"C:\Users\maria\.claude");
    assert_eq!(outside.key, CLAUDE_CONFIG_DIR_KEY);
    assert_eq!(outside.kind, WARNING_KIND_OUTSIDE_MOUNT);
    assert_eq!(
        outside.message,
        "container session: CLAUDE_CONFIG_DIR='C:\\Users\\maria\\.claude' is outside the bind \
         mount (C:\\Users\\maria\\repo\\.ac\\wg-1\\__agent_x -> /workspace); it is not visible \
         inside the container. The variable was unset in the container environment and \
         auto-resume is skipped. Set CLAUDE_CONFIG_DIR=%AC_REPLICA_ROOT%\\.claude to make \
         conversation state durable and resumable."
    );

    let no_value = claude_config_dir_no_value_warning();
    assert_eq!(no_value.key, CLAUDE_CONFIG_DIR_KEY);
    assert_eq!(no_value.kind, WARNING_KIND_NO_VALUE);
    assert_eq!(
        no_value.message,
        "container session: no CLAUDE_CONFIG_DIR is set. Conversation transcripts are \
         container-ephemeral, Telegram bridge is inactive, and auto-resume is skipped. Set \
         CLAUDE_CONFIG_DIR=%AC_REPLICA_ROOT%\\.claude to make conversation state durable and \
         resumable."
    );
}

#[test]
fn host_path_env_warnings_cover_non_claude_keys() {
    let map = map();

    let container_path =
        host_path_env_unmappable_warning(&map, CODEX_HOME_KEY, "/workspace/.codex");
    assert_eq!(container_path.key, CODEX_HOME_KEY);
    assert_eq!(
        container_path.kind,
        WARNING_KIND_CONTAINER_PATH_IN_HOST_FIELD
    );
    assert_eq!(
        container_path.message,
        "container session: CODEX_HOME='/workspace/.codex' looks like a container path. Path \
         env rows in AgentsCommander are host-side values; AgentsCommander translates them for \
         container sessions. This value was unset in the container. Set CODEX_HOME to a host \
         path under C:\\Users\\maria\\repo\\.ac\\wg-1\\__agent_x so it resolves under \
         /workspace inside the container."
    );

    let outside = host_path_env_unmappable_warning(
        &map,
        OPENCODE_CONFIG_DIR_KEY,
        r"C:\Users\maria\.opencode",
    );
    assert_eq!(outside.key, OPENCODE_CONFIG_DIR_KEY);
    assert_eq!(outside.kind, WARNING_KIND_OUTSIDE_MOUNT);
    assert_eq!(
        outside.message,
        "container session: OPENCODE_CONFIG_DIR='C:\\Users\\maria\\.opencode' is outside the \
         bind mount (C:\\Users\\maria\\repo\\.ac\\wg-1\\__agent_x -> /workspace); it is not \
         visible inside the container. The variable was unset in the container environment. Set \
         OPENCODE_CONFIG_DIR to a host path under C:\\Users\\maria\\repo\\.ac\\wg-1\\__agent_x \
         so AgentsCommander can translate it for the container."
    );
}

#[test]
fn mount_source_rejection_refuses_pinned_sources() {
    assert!(
        container_mount_source_rejection(Path::new(if cfg!(windows) { r"C:\" } else { "/" }))
            .is_some()
    );
    assert!(container_mount_source_rejection(Path::new(r"C:\")).is_some());
    assert!(container_mount_source_rejection(Path::new(r"\\server\share")).is_some());

    if let Some(home) = dirs::home_dir() {
        assert!(container_mount_source_rejection(&home).is_some());
    }

    let tmp = tempfile::tempdir().unwrap();
    let project = tmp.path().join("project");
    let ac_root = project.join(".ac");
    let wg = ac_root.join("wg-1-team");
    std::fs::create_dir_all(&wg).unwrap();

    assert!(container_mount_source_rejection(&ac_root).is_some());
    assert!(container_mount_source_rejection(&project).is_some());
    assert!(container_mount_source_rejection(&wg).is_some());
}

#[test]
fn mount_source_rejection_permits_pinned_sources() {
    let tmp = tempfile::tempdir().unwrap();
    let project = tmp.path().join("project");
    let ac_root = project.join(".ac");
    let replica = ac_root.join("wg-1-team").join("__agent_x");
    let repo = ac_root.join("wg-1-team").join("repo-foo");
    let matrix = ac_root.join("_agent_x");
    let plain_repo = tmp.path().join("plain-repo");
    for path in [&replica, &repo, &matrix, &plain_repo] {
        std::fs::create_dir_all(path).unwrap();
        assert_eq!(container_mount_source_rejection(path), None);
    }
}
