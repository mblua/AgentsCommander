use std::path::Path;
use crate::session::profile::CodingAgentKind;

#[derive(Debug, PartialEq)]
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

    for ancestor in cwd.ancestors() {
        let ancestor_str = ancestor.to_string_lossy().to_string();

        // Case-insensitive comparison for Windows path casing mismatches, slash normalization
        for (project_path_str, project_data) in projects {
            let normalized_project = project_path_str.replace("\\", "/");
            let normalized_ancestor = ancestor_str.replace("\\", "/");
            if normalized_project.eq_ignore_ascii_case(&normalized_ancestor) {
                if let Some(trusted) = project_data.get("hasTrustDialogAccepted") {
                    if trusted.as_bool() == Some(true) {
                        return TrustStatus::Trusted;
                    } else {
                        return TrustStatus::Untrusted;
                    }
                }
            }
        }
    }

    TrustStatus::Unknown
}

fn path_starts_with_case_insensitive_windows(path: &Path, base: &Path) -> bool {
    let mut path_components = path.components();
    let mut base_components = base.components();

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

    // Find the longest matching project_path for correct path specificity
    let mut best_match: Option<(&str, &toml::Value)> = None;
    let mut longest_len = 0;

    for (project_path_str, project_config) in projects {
        let project_path = Path::new(project_path_str);
        if path_starts_with_case_insensitive_windows(cwd, project_path) {
            let len = project_path.components().count();
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
    }
}
