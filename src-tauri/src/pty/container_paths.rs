use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContainerEnvClass {
    HostPathTranslate,
    Opaque,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContainerEnvValidationRegime {
    StrictAbsolutePath,
    AllowsUnexpandedMarkers,
}

pub const CLAUDE_CONFIG_DIR_KEY: &str = "CLAUDE_CONFIG_DIR";
pub const CODEX_HOME_KEY: &str = "CODEX_HOME";
pub const OPENCODE_CONFIG_DIR_KEY: &str = "OPENCODE_CONFIG_DIR";

pub const HOST_PATH_TRANSLATE_ENV_KEYS: [&str; 3] = [
    CLAUDE_CONFIG_DIR_KEY,
    CODEX_HOME_KEY,
    OPENCODE_CONFIG_DIR_KEY,
];

pub const WARNING_KIND_CONTAINER_PATH_IN_HOST_FIELD: &str = "container-path-in-host-field";
pub const WARNING_KIND_OUTSIDE_MOUNT: &str = "outside-mount";
pub const WARNING_KIND_NO_VALUE: &str = "no-value";
pub const WARNING_KIND_PROTOCOL_MISMATCH: &str = "protocol-mismatch";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainerEnvWarning {
    pub key: String,
    pub kind: &'static str,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainerPathMap {
    host_root: String,
    host_root_normalized: String,
    host_root_key: String,
    container_root: String,
}

impl ContainerPathMap {
    pub fn new(canonical_host_root: &str, container_root: &str) -> Result<Self, String> {
        let host_root_normalized = normalize_path_text(canonical_host_root);
        if is_unc_path(&host_root_normalized) {
            return Err(format!(
                "container transport cannot bind-mount UNC host root '{}'",
                canonical_host_root
            ));
        }
        if !looks_absolute_host_path(canonical_host_root) {
            return Err(format!(
                "container transport host root '{}' is not absolute",
                canonical_host_root
            ));
        }

        let host_root_normalized = trim_trailing_separators(&host_root_normalized);
        let container_root = normalize_container_root(container_root);
        Ok(Self {
            host_root: canonical_host_root.to_string(),
            host_root_key: root_key(&host_root_normalized),
            host_root_normalized,
            container_root,
        })
    }

    pub fn to_container(&self, host_path: &str) -> Option<String> {
        if contains_unexpanded_marker(host_path) || !looks_absolute_host_path(host_path) {
            return None;
        }

        let normalized = trim_trailing_separators(&normalize_path_text(host_path));
        if contains_dot_segment(&normalized) {
            return None;
        }

        let compare_key = root_key(&normalized);
        if compare_key == self.host_root_key {
            return Some(self.container_root.clone());
        }

        let prefix = format!("{}/", self.host_root_key);
        if !compare_key.starts_with(&prefix) {
            return None;
        }

        let remainder = normalized.get(self.host_root_normalized.len()..)?;
        let remainder = remainder.strip_prefix('/')?;
        if remainder.is_empty() {
            Some(self.container_root.clone())
        } else {
            Some(format!("{}/{}", self.container_root, remainder))
        }
    }

    pub fn to_host(&self, container_path: &str) -> Option<PathBuf> {
        let normalized = normalize_container_path(container_path)?;
        if normalized == self.container_root {
            return Some(PathBuf::from(&self.host_root));
        }
        let prefix = format!("{}/", self.container_root);
        let remainder = normalized.strip_prefix(&prefix)?;
        let sep = host_separator(&self.host_root);
        Some(PathBuf::from(format!(
            "{}{}{}",
            self.host_root,
            sep,
            remainder.replace('/', sep)
        )))
    }

    pub fn is_container_path(&self, value: &str) -> bool {
        normalize_container_path(value)
            .map(|normalized| {
                normalized == self.container_root
                    || normalized.starts_with(&format!("{}/", self.container_root))
            })
            .unwrap_or(false)
    }

    pub fn host_root(&self) -> &str {
        &self.host_root
    }

    pub fn container_root(&self) -> &str {
        &self.container_root
    }
}

pub fn root_key(root: &str) -> String {
    let normalized = normalize_path_text(root);
    let trimmed = trim_trailing_separators(&normalized);
    if cfg!(windows) {
        trimmed.to_ascii_lowercase()
    } else {
        trimmed
    }
}

pub fn classify_container_env(key: &str) -> ContainerEnvClass {
    if canonical_host_path_env_key(key).is_some() {
        ContainerEnvClass::HostPathTranslate
    } else {
        ContainerEnvClass::Opaque
    }
}

pub fn canonical_host_path_env_key(key: &str) -> Option<&'static str> {
    let normalized = crate::config::settings::normalize_env_key_for_platform(key);
    HOST_PATH_TRANSLATE_ENV_KEYS
        .iter()
        .copied()
        .find(|candidate| {
            crate::config::settings::normalize_env_key_for_platform(candidate) == normalized
        })
}

pub fn host_path_env_validation_regime(key: &str) -> Option<ContainerEnvValidationRegime> {
    canonical_host_path_env_key(key).map(|canonical| {
        if canonical == CODEX_HOME_KEY {
            ContainerEnvValidationRegime::StrictAbsolutePath
        } else {
            ContainerEnvValidationRegime::AllowsUnexpandedMarkers
        }
    })
}

pub fn container_config_dir(map: &ContainerPathMap, raw_value: &str) -> Option<String> {
    map.to_container(raw_value)
}

pub fn claude_config_dir_unmappable_warning(
    map: &ContainerPathMap,
    raw_value: &str,
) -> ContainerEnvWarning {
    let (kind, message) = if map.is_container_path(raw_value) {
        (
            WARNING_KIND_CONTAINER_PATH_IN_HOST_FIELD,
            format!(
                "container session: CLAUDE_CONFIG_DIR='{raw_value}' looks like a container path. Path env rows in AgentsCommander are host-side values; AgentsCommander translates them for container sessions. This value was unset in the container, so conversation state will not persist and auto-resume is skipped. Set CLAUDE_CONFIG_DIR=%AC_REPLICA_ROOT%\\.claude (it resolves to /workspace/.claude inside the container)."
            ),
        )
    } else {
        (
            WARNING_KIND_OUTSIDE_MOUNT,
            format!(
                "container session: CLAUDE_CONFIG_DIR='{raw_value}' is outside the bind mount ({} -> {}); it is not visible inside the container. The variable was unset in the container environment and auto-resume is skipped. Set CLAUDE_CONFIG_DIR=%AC_REPLICA_ROOT%\\.claude to make conversation state durable and resumable.",
                map.host_root(),
                map.container_root()
            ),
        )
    };
    ContainerEnvWarning {
        key: CLAUDE_CONFIG_DIR_KEY.to_string(),
        kind,
        message,
    }
}

pub fn host_path_env_unmappable_warning(
    map: &ContainerPathMap,
    key: &str,
    raw_value: &str,
) -> ContainerEnvWarning {
    if key == CLAUDE_CONFIG_DIR_KEY {
        return claude_config_dir_unmappable_warning(map, raw_value);
    }

    let (kind, message) = if map.is_container_path(raw_value) {
        (
            WARNING_KIND_CONTAINER_PATH_IN_HOST_FIELD,
            format!(
                "container session: {key}='{raw_value}' looks like a container path. Path env rows in AgentsCommander are host-side values; AgentsCommander translates them for container sessions. This value was unset in the container. Set {key} to a host path under {} so it resolves under {} inside the container.",
                map.host_root(),
                map.container_root()
            ),
        )
    } else {
        (
            WARNING_KIND_OUTSIDE_MOUNT,
            format!(
                "container session: {key}='{raw_value}' is outside the bind mount ({} -> {}); it is not visible inside the container. The variable was unset in the container environment. Set {key} to a host path under {} so AgentsCommander can translate it for the container.",
                map.host_root(),
                map.container_root(),
                map.host_root()
            ),
        )
    };
    ContainerEnvWarning {
        key: key.to_string(),
        kind,
        message,
    }
}

pub fn claude_config_dir_no_value_warning() -> ContainerEnvWarning {
    ContainerEnvWarning {
        key: CLAUDE_CONFIG_DIR_KEY.to_string(),
        kind: WARNING_KIND_NO_VALUE,
        message: "container session: no CLAUDE_CONFIG_DIR is set. Conversation transcripts are container-ephemeral, Telegram bridge is inactive, and auto-resume is skipped. Set CLAUDE_CONFIG_DIR=%AC_REPLICA_ROOT%\\.claude to make conversation state durable and resumable.".to_string(),
    }
}

pub fn container_mount_source_rejection(cwd: &Path) -> Option<String> {
    let cwd_text = cwd.to_string_lossy();
    if is_filesystem_or_drive_or_unc_share_root(&cwd_text) {
        return Some(format!(
            "container transport refuses to bind-mount filesystem root '{}'",
            cwd.display()
        ));
    }

    if let Some(home) = dirs::home_dir() {
        if same_path_or_string(cwd, &home) {
            return Some(format!(
                "container transport refuses to bind-mount the user's home directory '{}'",
                cwd.display()
            ));
        }
    }

    if cwd
        .file_name()
        .and_then(|name| name.to_str())
        .map(crate::config::workspace::is_workspace_dir_name)
        .unwrap_or(false)
    {
        return Some(format!(
            "container transport refuses to bind-mount AC workspace root '{}'",
            cwd.display()
        ));
    }

    if crate::config::workspace::has_workspace_dir(cwd) {
        return Some(format!(
            "container transport refuses to bind-mount project root '{}' because it directly contains an AC workspace",
            cwd.display()
        ));
    }

    let is_wg_dir = cwd
        .file_name()
        .and_then(|name| name.to_str())
        .map(|name| name.starts_with("wg-"))
        .unwrap_or(false);
    if is_wg_dir && crate::config::workspace::find_workspace_ancestor(cwd).is_some() {
        return Some(format!(
            "container transport refuses to bind-mount workgroup root '{}'",
            cwd.display()
        ));
    }

    None
}

fn same_path_or_string(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => root_key(&a.to_string_lossy()) == root_key(&b.to_string_lossy()),
    }
}

fn normalize_path_text(path: &str) -> String {
    let stripped = strip_windows_verbatim_prefix_any(path);
    stripped.replace('\\', "/")
}

fn strip_windows_verbatim_prefix_any(path: &str) -> String {
    if let Some(rest) = path.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{}", rest)
    } else if let Some(rest) = path.strip_prefix(r"\\?\") {
        rest.to_string()
    } else if let Some(rest) = path.strip_prefix(r"\??\UNC\") {
        format!(r"\\{}", rest)
    } else if let Some(rest) = path.strip_prefix(r"\??\") {
        rest.to_string()
    } else {
        path.to_string()
    }
}

fn trim_trailing_separators(path: &str) -> String {
    if is_drive_root_text(path) || path == "/" || is_unc_share_root_text(path) {
        return path.to_string();
    }
    path.trim_end_matches('/').to_string()
}

fn normalize_container_root(root: &str) -> String {
    let normalized = root.replace('\\', "/");
    if normalized == "/" {
        normalized
    } else {
        normalized.trim_end_matches('/').to_string()
    }
}

fn normalize_container_path(path: &str) -> Option<String> {
    if path.contains('\\') {
        return None;
    }
    if !path.starts_with('/') || path.contains("//") || contains_dot_segment(path) {
        return None;
    }
    Some(if path == "/" {
        path.to_string()
    } else {
        path.trim_end_matches('/').to_string()
    })
}

fn host_separator(host_root: &str) -> &str {
    if host_root.contains('\\') && !host_root.contains('/') {
        r"\"
    } else {
        "/"
    }
}

fn contains_unexpanded_marker(value: &str) -> bool {
    value.contains('%') || value.contains('$')
}

fn contains_dot_segment(path: &str) -> bool {
    path.split('/').any(|part| part == "." || part == "..")
}

fn looks_absolute_host_path(path: &str) -> bool {
    let normalized = normalize_path_text(path);
    if is_unc_path(&normalized) {
        return true;
    }
    if normalized.len() >= 3 {
        let bytes = normalized.as_bytes();
        if bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && bytes[2] == b'/' {
            return true;
        }
    }
    normalized.starts_with('/') && !path.starts_with('\\')
}

fn is_unc_path(normalized: &str) -> bool {
    normalized.starts_with("//")
}

fn is_drive_root_text(normalized: &str) -> bool {
    normalized.len() == 3
        && normalized.as_bytes()[0].is_ascii_alphabetic()
        && normalized.as_bytes()[1] == b':'
        && normalized.as_bytes()[2] == b'/'
}

fn is_unc_share_root_text(normalized: &str) -> bool {
    if !is_unc_path(normalized) {
        return false;
    }
    let parts: Vec<&str> = normalized
        .split('/')
        .filter(|part| !part.is_empty())
        .collect();
    parts.len() == 2
}

fn is_filesystem_or_drive_or_unc_share_root(path: &str) -> bool {
    let normalized = normalize_path_text(path);
    normalized == "/" || is_drive_root_text(&normalized) || is_unc_share_root_text(&normalized)
}

#[cfg(test)]
mod tests {
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
            "container session: CLAUDE_CONFIG_DIR='/workspace/.claude' looks like a container path. Path env rows in AgentsCommander are host-side values; AgentsCommander translates them for container sessions. This value was unset in the container, so conversation state will not persist and auto-resume is skipped. Set CLAUDE_CONFIG_DIR=%AC_REPLICA_ROOT%\\.claude (it resolves to /workspace/.claude inside the container)."
        );

        let outside = claude_config_dir_unmappable_warning(&map, r"C:\Users\maria\.claude");
        assert_eq!(outside.key, CLAUDE_CONFIG_DIR_KEY);
        assert_eq!(outside.kind, WARNING_KIND_OUTSIDE_MOUNT);
        assert_eq!(
            outside.message,
            "container session: CLAUDE_CONFIG_DIR='C:\\Users\\maria\\.claude' is outside the bind mount (C:\\Users\\maria\\repo\\.ac\\wg-1\\__agent_x -> /workspace); it is not visible inside the container. The variable was unset in the container environment and auto-resume is skipped. Set CLAUDE_CONFIG_DIR=%AC_REPLICA_ROOT%\\.claude to make conversation state durable and resumable."
        );

        let no_value = claude_config_dir_no_value_warning();
        assert_eq!(no_value.key, CLAUDE_CONFIG_DIR_KEY);
        assert_eq!(no_value.kind, WARNING_KIND_NO_VALUE);
        assert_eq!(
            no_value.message,
            "container session: no CLAUDE_CONFIG_DIR is set. Conversation transcripts are container-ephemeral, Telegram bridge is inactive, and auto-resume is skipped. Set CLAUDE_CONFIG_DIR=%AC_REPLICA_ROOT%\\.claude to make conversation state durable and resumable."
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
            "container session: CODEX_HOME='/workspace/.codex' looks like a container path. Path env rows in AgentsCommander are host-side values; AgentsCommander translates them for container sessions. This value was unset in the container. Set CODEX_HOME to a host path under C:\\Users\\maria\\repo\\.ac\\wg-1\\__agent_x so it resolves under /workspace inside the container."
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
            "container session: OPENCODE_CONFIG_DIR='C:\\Users\\maria\\.opencode' is outside the bind mount (C:\\Users\\maria\\repo\\.ac\\wg-1\\__agent_x -> /workspace); it is not visible inside the container. The variable was unset in the container environment. Set OPENCODE_CONFIG_DIR to a host path under C:\\Users\\maria\\repo\\.ac\\wg-1\\__agent_x so AgentsCommander can translate it for the container."
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
        let workspace = project.join(".ac");
        let wg = workspace.join("wg-1-team");
        std::fs::create_dir_all(&wg).unwrap();

        assert!(container_mount_source_rejection(&workspace).is_some());
        assert!(container_mount_source_rejection(&project).is_some());
        assert!(container_mount_source_rejection(&wg).is_some());
    }

    #[test]
    fn mount_source_rejection_permits_pinned_sources() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("project");
        let workspace = project.join(".ac");
        let replica = workspace.join("wg-1-team").join("__agent_x");
        let repo = workspace.join("wg-1-team").join("repo-foo");
        let matrix = workspace.join("_agent_x");
        let plain_repo = tmp.path().join("plain-repo");
        for path in [&replica, &repo, &matrix, &plain_repo] {
            std::fs::create_dir_all(path).unwrap();
            assert_eq!(container_mount_source_rejection(path), None);
        }
    }
}
