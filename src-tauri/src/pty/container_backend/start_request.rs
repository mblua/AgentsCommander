use crate::errors::AppError;
use crate::pty::container_paths::{
    canonical_host_path_env_key, container_config_dir, host_path_env_unmappable_warning,
    ContainerEnvClass, ContainerEnvWarning, ContainerPathMap,
};
use crate::pty::container_runtime::{
    api_url_for_container, resolve_container_image, ContainerStartRequest,
    DEFAULT_CONTAINER_WORKDIR,
};
use crate::pty::container_tokens::ContainerApiToken;
use uuid::Uuid;

#[allow(clippy::too_many_arguments)]
pub(super) fn build_start_request(
    session_id: Uuid,
    cmd: &str,
    args: Vec<String>,
    cwd: &str,
    rows: u16,
    cols: u16,
    configured_env: Vec<(String, String)>,
    env_remove_keys: Vec<String>,
    env_unset: Vec<String>,
    container_image: Option<String>,
    selected_cwd: Option<&str>,
    registration_ticket: String,
    token: &ContainerApiToken,
    repo_mounts: Vec<crate::pty::container_repos::ContainerRepoMount>,
    #[cfg(test)] settings_override: Option<&crate::config::settings::AppSettings>,
) -> Result<ContainerStartRequest, AppError> {
    #[cfg(not(test))]
    let settings = crate::config::settings::load_settings();
    #[cfg(test)]
    let settings = settings_override
        .cloned()
        .unwrap_or_else(crate::config::settings::load_settings);
    build_start_request_with_settings(
        session_id,
        cmd,
        args,
        cwd,
        rows,
        cols,
        configured_env,
        env_remove_keys,
        env_unset,
        container_image,
        selected_cwd,
        registration_ticket,
        token,
        &settings,
        repo_mounts,
    )
}

#[allow(clippy::too_many_arguments)]
pub(super) fn build_start_request_with_settings(
    session_id: Uuid,
    cmd: &str,
    args: Vec<String>,
    cwd: &str,
    rows: u16,
    cols: u16,
    configured_env: Vec<(String, String)>,
    env_remove_keys: Vec<String>,
    env_unset: Vec<String>,
    container_image: Option<String>,
    selected_cwd: Option<&str>,
    registration_ticket: String,
    token: &ContainerApiToken,
    settings: &crate::config::settings::AppSettings,
    repo_mounts: Vec<crate::pty::container_repos::ContainerRepoMount>,
) -> Result<ContainerStartRequest, AppError> {
    if !settings.api_server_enabled {
        return Err(AppError::Other(
            "container transport requires the control-plane API server to be enabled".to_string(),
        ));
    }
    if let Some(reason) =
        crate::pty::container_paths::container_mount_source_rejection(std::path::Path::new(cwd))
    {
        let message = match selected_cwd {
            Some(selected) if selected != cwd => format!(
                "{} (selected path '{}', canonical path '{}')",
                reason, selected, cwd
            ),
            _ => reason,
        };
        return Err(AppError::Other(message));
    }
    let api_url = api_url_for_container(&settings.api_server_bind, settings.api_server_port)?;
    let child_env = sanitized_child_env(configured_env, env_remove_keys);
    match selected_cwd {
        Some(selected) if selected != cwd => log::info!(
            "[container-transport] mount source selected '{}' canonical '{}' -> '{}'",
            selected,
            cwd,
            DEFAULT_CONTAINER_WORKDIR
        ),
        _ => log::info!(
            "[container-transport] mount source '{}' -> '{}'",
            cwd,
            DEFAULT_CONTAINER_WORKDIR
        ),
    }
    for mount in &repo_mounts {
        log::info!(
            "[container-transport] repo mount '{}' -> '{}'",
            mount.host_path.display(),
            mount.container_path
        );
    }
    Ok(ContainerStartRequest {
        session_id,
        image: resolve_container_image(container_image.as_deref())?,
        host_root: cwd.to_string(),
        container_workdir: DEFAULT_CONTAINER_WORKDIR.to_string(),
        api_url,
        api_token: token.secret.clone(),
        registration_ticket,
        local_dir: crate::config::agent_local_dir_name(),
        command: cmd.to_string(),
        args,
        child_env,
        env_unset,
        cols,
        rows,
        repo_mounts,
    })
}

pub(crate) struct ContainerChildEnv {
    pub child_env: Vec<(String, String)>,
    pub env_unset: Vec<String>,
    pub warnings: Vec<ContainerEnvWarning>,
}

pub(crate) fn container_child_env(
    configured_env: Vec<(String, String)>,
    env_remove_keys: Vec<String>,
    map: &ContainerPathMap,
) -> ContainerChildEnv {
    let mut child_env = Vec::new();
    let mut env_unset = Vec::new();
    let mut warnings = Vec::new();
    for (key, value) in configured_env {
        if env_remove_keys
            .iter()
            .any(|remove| env_key_matches_platform(remove, &key))
            || is_reserved_container_env(&key)
        {
            continue;
        }

        let Some(canonical_key) = canonical_host_path_env_key(&key) else {
            child_env.push((key, value));
            continue;
        };

        match crate::pty::container_paths::classify_container_env(&key) {
            ContainerEnvClass::Opaque => child_env.push((key, value)),
            ContainerEnvClass::HostPathTranslate => {
                if let Some(container_value) = container_config_dir(map, &value) {
                    child_env.push((canonical_key.to_string(), container_value));
                } else {
                    let already_unset = env_unset.iter().any(|existing| existing == canonical_key);
                    if !already_unset {
                        env_unset.push(canonical_key.to_string());
                        warnings.push(host_path_env_unmappable_warning(map, canonical_key, &value));
                    }
                }
            }
        }
    }
    ContainerChildEnv {
        child_env,
        env_unset,
        warnings,
    }
}

pub(super) fn env_key_matches_platform(left: &str, right: &str) -> bool {
    crate::config::settings::normalize_env_key_for_platform(left)
        == crate::config::settings::normalize_env_key_for_platform(right)
}

pub(super) fn sanitized_child_env(
    configured_env: Vec<(String, String)>,
    env_remove_keys: Vec<String>,
) -> Vec<(String, String)> {
    configured_env
        .into_iter()
        .filter(|(key, _)| {
            !env_remove_keys
                .iter()
                .any(|remove| remove.eq_ignore_ascii_case(key))
        })
        .filter(|(key, _)| !is_reserved_container_env(key))
        .collect()
}

pub(super) fn is_reserved_container_env(key: &str) -> bool {
    key.eq_ignore_ascii_case("AGENTSCOMMANDER_TOKEN")
        || key.eq_ignore_ascii_case("AGENTSCOMMANDER_BINARY_PATH")
        || key.eq_ignore_ascii_case("AGENTSCOMMANDER_SESSION_REGISTRATION_TOKEN")
        || key.eq_ignore_ascii_case("AGENTSCOMMANDER_API_TOKEN")
        || key.eq_ignore_ascii_case("AGENTSCOMMANDER_ROOT")
}
