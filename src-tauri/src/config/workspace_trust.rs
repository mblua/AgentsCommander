use std::path::{Path, PathBuf};
use crate::session::profile::CodingAgentKind;

use serde::{Deserialize, Serialize};

#[derive(Debug, PartialEq, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TrustStatus {
    Trusted,
    Untrusted,
    Unknown,
}

/// Checks if the agent has recorded trust for the given path or its ancestors.
pub async fn is_workspace_trusted(cwd: &Path, shell: &str, shell_args: &[String], agent: CodingAgentKind) -> TrustStatus {
    match agent {
        CodingAgentKind::Claude => check_claude_trust(cwd, shell, shell_args).await,
        CodingAgentKind::Codex => check_codex_trust(cwd).await,
        CodingAgentKind::Gemini => TrustStatus::Unknown, // Not applicable or not requested yet
    }
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::Prefix(prefix) => {
                #[cfg(windows)]
                {
                    match prefix.kind() {
                        std::path::Prefix::VerbatimDisk(disk) | std::path::Prefix::Disk(disk) => {
                            normalized.push(format!("{}:", (disk as char).to_ascii_uppercase()));
                        }
                        std::path::Prefix::VerbatimUNC(server, share) | std::path::Prefix::UNC(server, share) => {
                            normalized.push(format!(r"\\{}\{}", server.to_string_lossy(), share.to_string_lossy()));
                        }
                        _ => normalized.push(component),
                    }
                }
                #[cfg(not(windows))]
                {
                    normalized.push(component);
                }
            }
            _ => normalized.push(component),
        }
    }
    normalized
}

fn path_starts_with_case_insensitive_windows(path: &Path, base: &Path) -> bool {
    let path_norm = normalize_path(path);
    let base_norm = normalize_path(base);
    let mut path_components = path_norm.components();
    let mut base_components = base_norm.components();

    loop {
        match (path_components.next(), base_components.next()) {
            (Some(p), Some(b)) => {
                #[cfg(windows)]
                {
                    if !p.as_os_str().to_string_lossy().eq_ignore_ascii_case(&b.as_os_str().to_string_lossy()) {
                        return false;
                    }
                }
                #[cfg(not(windows))]
                {
                    if p != b {
                        return false;
                    }
                }
            }
            (None, Some(_)) => return false, // path is shorter than base
            (_, None) => return true,        // base is exhausted, path matches or is longer
        }
    }
}

async fn check_claude_trust(cwd: &Path, shell: &str, shell_args: &[String]) -> TrustStatus {
    let projects_dir = crate::commands::session::resolve_claude_projects_dir(shell, shell_args, &cwd.to_string_lossy());
    let base_dir = match projects_dir {
        Some(p) => {
            // p is <base>/projects/<mangled>, so we go up two levels to get <base>
            if let Some(parent) = p.parent().and_then(|parent| parent.parent()) {
                parent.to_path_buf()
            } else {
                return TrustStatus::Unknown;
            }
        }
        None => match dirs::home_dir() {
            Some(h) => h.join(".claude"),
            None => return TrustStatus::Unknown,
        }
    };
    let global_config = base_dir.join(".claude.json");

    if !tokio::fs::try_exists(&global_config).await.unwrap_or(false) {
        return TrustStatus::Unknown;
    }

    let content = match tokio::fs::read_to_string(&global_config).await {
        Ok(c) => c,
        Err(e) => {
            log::warn!("Failed to read Claude config {}: {}", global_config.display(), e);
            return TrustStatus::Unknown;
        }
    };

    let parsed: serde_json::Value = match serde_json::from_str(&content) {
        Ok(p) => p,
        Err(e) => {
            log::warn!("Failed to parse Claude config: {}", e);
            return TrustStatus::Unknown;
        }
    };

    let projects = match parsed.get("projects").and_then(|p| p.as_object()) {
        Some(p) => p,
        None => return TrustStatus::Unknown,
    };

    evaluate_claude_trust_from_projects(cwd, projects)
}

fn evaluate_claude_trust_from_projects(cwd: &Path, projects: &serde_json::Map<String, serde_json::Value>) -> TrustStatus {
    let mut best_match: Option<(&str, &serde_json::Value)> = None;
    let mut longest_len = 0;

    for (project_path_str, project_data) in projects {
        let project_path = Path::new(project_path_str);
        if path_starts_with_case_insensitive_windows(cwd, project_path)
            && project_data.get("hasTrustDialogAccepted").is_some()
        {
            let len = normalize_path(project_path).components().count();
            if len > longest_len {
                longest_len = len;
                best_match = Some((project_path_str.as_str(), project_data));
            }
        }
    }

    if let Some((_, project_data)) = best_match {
        if let Some(trusted) = project_data.get("hasTrustDialogAccepted") {
            if trusted.as_bool() == Some(true) {
                return TrustStatus::Trusted;
            } else {
                return TrustStatus::Untrusted;
            }
        }
    }

    TrustStatus::Unknown
}

async fn check_codex_trust(cwd: &Path) -> TrustStatus {
    let home = match dirs::home_dir() {
        Some(h) => h,
        None => return TrustStatus::Unknown,
    };

    let global_config = home.join(".codex").join("config.toml");
    if !tokio::fs::try_exists(&global_config).await.unwrap_or(false) {
        return TrustStatus::Unknown;
    }

    let content = match tokio::fs::read_to_string(&global_config).await {
        Ok(c) => c,
        Err(e) => {
            log::warn!("Failed to read Codex config {}: {}", global_config.display(), e);
            return TrustStatus::Unknown;
        }
    };

    let parsed: toml::Value = match toml::from_str(&content) {
        Ok(p) => p,
        Err(e) => {
            log::warn!("Failed to parse Codex config: {}", e);
            return TrustStatus::Unknown;
        }
    };

    let projects = match parsed.get("projects").and_then(|p| p.as_table()) {
        Some(p) => p,
        None => return TrustStatus::Unknown,
    };

    evaluate_codex_trust_from_projects(cwd, projects)
}

fn evaluate_codex_trust_from_projects(cwd: &Path, projects: &toml::map::Map<String, toml::Value>) -> TrustStatus {
    // Find the longest matching project_path for correct path specificity
    let mut best_match: Option<(&str, &toml::Value)> = None;
    let mut longest_len = 0;

    for (project_path_str, project_config) in projects {
        let project_path = Path::new(project_path_str);
        if path_starts_with_case_insensitive_windows(cwd, project_path)
            && project_config.get("trust_level").is_some()
        {
            let len = normalize_path(project_path).components().count();
            if len > longest_len {
                longest_len = len;
                best_match = Some((project_path_str.as_str(), project_config));
            }
        }
    }

    if let Some((_, project_config)) = best_match {
        if let Some(trust_level) = project_config.get("trust_level").and_then(|t| t.as_str()) {
            if trust_level == "trusted" {
                return TrustStatus::Trusted;
            } else if trust_level == "untrusted" {
                return TrustStatus::Untrusted;
            }
        }
    }

    TrustStatus::Unknown
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_unknown_trust() {
        let status = check_claude_trust(Path::new("C:\\nonexistent\\path"), "claude", &[]).await;
        // In test environments without ~/.claude.json this correctly returns Unknown.
        assert_eq!(status, TrustStatus::Unknown);
    }

    #[test]
    fn test_path_starts_with_case_insensitive_windows() {
        let base = Path::new("C:\\Users\\Test\\repo");

        // Exact match
        assert!(path_starts_with_case_insensitive_windows(Path::new("C:\\Users\\Test\\repo"), base));

        // Subdirectory
        assert!(path_starts_with_case_insensitive_windows(Path::new("C:\\Users\\Test\\repo\\sub"), base));

        // Case-insensitive match on Windows
        #[cfg(windows)]
        assert!(path_starts_with_case_insensitive_windows(Path::new("c:\\users\\test\\REPO\\sub"), base));

        // False prefix matching (repo vs repo-other) should fail
        assert!(!path_starts_with_case_insensitive_windows(Path::new("C:\\Users\\Test\\repo-other"), base));

        // False prefix on Windows
        #[cfg(windows)]
        assert!(!path_starts_with_case_insensitive_windows(Path::new("c:\\users\\test\\REPO-other"), base));

        // Verbatim prefix handling
        #[cfg(windows)]
        assert!(path_starts_with_case_insensitive_windows(Path::new("\\\\?\\C:\\Users\\Test\\repo"), base));

        // Trailing slash matching
        let base_with_slash = Path::new("C:\\Users\\Test\\repo\\");
        assert!(path_starts_with_case_insensitive_windows(Path::new("C:\\Users\\Test\\repo"), base_with_slash));
    }

    #[test]
    fn test_claude_trust_specificity() {
        use serde_json::json;

        let projects_json = json!({
            "C:\\Users\\Test\\repo": { "hasTrustDialogAccepted": false },
            "C:\\Users\\Test\\repo\\child": { "hasTrustDialogAccepted": true }
        });
        let projects = projects_json.as_object().unwrap();

        // parent untrusted + child trusted => Trusted for child
        assert_eq!(
            evaluate_claude_trust_from_projects(Path::new("C:\\Users\\Test\\repo\\child"), projects),
            TrustStatus::Trusted
        );

        // And for the parent in the first case, it should be Untrusted
        assert_eq!(
            evaluate_claude_trust_from_projects(Path::new("C:\\Users\\Test\\repo"), projects),
            TrustStatus::Untrusted
        );

        // child untrusted + parent trusted => Untrusted for child
        let projects_json_inv = json!({
            "C:\\Users\\Test\\repo": { "hasTrustDialogAccepted": true },
            "C:\\Users\\Test\\repo\\child": { "hasTrustDialogAccepted": false }
        });
        let projects_inv = projects_json_inv.as_object().unwrap();

        assert_eq!(
            evaluate_claude_trust_from_projects(Path::new("C:\\Users\\Test\\repo\\child"), projects_inv),
            TrustStatus::Untrusted
        );

        // parent trusted + child matching without hasTrustDialogAccepted => Trusted by parent inheritance
        let projects_json_child_no_key1 = json!({
            "C:\\Users\\Test\\repo": { "hasTrustDialogAccepted": true },
            "C:\\Users\\Test\\repo\\child": { "theme": "dark" }
        });
        assert_eq!(
            evaluate_claude_trust_from_projects(Path::new("C:\\Users\\Test\\repo\\child"), projects_json_child_no_key1.as_object().unwrap()),
            TrustStatus::Trusted
        );

        // parent untrusted + child matching without key => Untrusted by parent
        let projects_json_child_no_key2 = json!({
            "C:\\Users\\Test\\repo": { "hasTrustDialogAccepted": false },
            "C:\\Users\\Test\\repo\\child": { "theme": "dark" }
        });
        assert_eq!(
            evaluate_claude_trust_from_projects(Path::new("C:\\Users\\Test\\repo\\child"), projects_json_child_no_key2.as_object().unwrap()),
            TrustStatus::Untrusted
        );

        // only child matching without key and no parent with key => Unknown
        let projects_json_child_no_key3 = json!({
            "C:\\Users\\Test\\repo\\child": { "theme": "dark" }
        });
        assert_eq!(
            evaluate_claude_trust_from_projects(Path::new("C:\\Users\\Test\\repo\\child"), projects_json_child_no_key3.as_object().unwrap()),
            TrustStatus::Unknown
        );
    }

    #[tokio::test]
    async fn test_codex_trust_specificity() {
        use toml::toml;

        let config_toml = toml! {
            [projects]
            "C:\\Users\\Test\\repo" = { trust_level = "untrusted" }
            "C:\\Users\\Test\\repo\\child" = { trust_level = "trusted" }
        };
        let projects = config_toml.get("projects").unwrap().as_table().unwrap();

        // parent untrusted + child trusted => Trusted for child
        assert_eq!(
            evaluate_codex_trust_from_projects(Path::new("C:\\Users\\Test\\repo\\child"), projects),
            TrustStatus::Trusted
        );

        // And for the parent in the first case, it should be Untrusted
        assert_eq!(
            evaluate_codex_trust_from_projects(Path::new("C:\\Users\\Test\\repo"), projects),
            TrustStatus::Untrusted
        );

        // child untrusted + parent trusted => Untrusted for child
        let config_toml_inv = toml! {
            [projects]
            "C:\\Users\\Test\\repo" = { trust_level = "trusted" }
            "C:\\Users\\Test\\repo\\child" = { trust_level = "untrusted" }
        };
        let projects_inv = config_toml_inv.get("projects").unwrap().as_table().unwrap();

        assert_eq!(
            evaluate_codex_trust_from_projects(Path::new("C:\\Users\\Test\\repo\\child"), projects_inv),
            TrustStatus::Untrusted
        );

        // parent trusted + child matching without trust_level => Trusted by parent inheritance
        let config_toml_child_no_key1 = toml! {
            [projects]
            "C:\\Users\\Test\\repo" = { trust_level = "trusted" }
            "C:\\Users\\Test\\repo\\child" = { theme = "dark" }
        };
        assert_eq!(
            evaluate_codex_trust_from_projects(Path::new("C:\\Users\\Test\\repo\\child"), config_toml_child_no_key1.get("projects").unwrap().as_table().unwrap()),
            TrustStatus::Trusted
        );

        // parent untrusted + child matching without key => Untrusted by parent
        let config_toml_child_no_key2 = toml! {
            [projects]
            "C:\\Users\\Test\\repo" = { trust_level = "untrusted" }
            "C:\\Users\\Test\\repo\\child" = { theme = "dark" }
        };
        assert_eq!(
            evaluate_codex_trust_from_projects(Path::new("C:\\Users\\Test\\repo\\child"), config_toml_child_no_key2.get("projects").unwrap().as_table().unwrap()),
            TrustStatus::Untrusted
        );

        // only child matching without key and no parent with key => Unknown
        let config_toml_child_no_key3 = toml! {
            [projects]
            "C:\\Users\\Test\\repo\\child" = { theme = "dark" }
        };
        assert_eq!(
            evaluate_codex_trust_from_projects(Path::new("C:\\Users\\Test\\repo\\child"), config_toml_child_no_key3.get("projects").unwrap().as_table().unwrap()),
            TrustStatus::Unknown
        );
    }
}
