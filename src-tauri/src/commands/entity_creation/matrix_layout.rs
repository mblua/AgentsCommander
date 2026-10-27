pub(crate) use crate::config::entity_creation::matrices::{
    create_agent_matrix_layout, create_agent_replica_layout, default_agent_matrix_config,
};
use crate::config::entity_creation::matrices::{
    create_new_agent_matrix_layout, matrix_directory_present,
};
pub(crate) use crate::config::entity_creation::names::{
    sanitize_name, selected_ac_root, validate_existing_name,
};
use crate::config::entity_creation::roles;
use crate::config::settings::AppSettings;
use std::path::{Path, PathBuf};

/// Extract a repo directory name from a git URL.
/// `https://github.com/org/my-repo.git` → `my-repo`
pub(super) fn repo_dir_name_from_url(url: &str) -> String {
    let without_trailing = url.trim_end_matches('/');
    let last_segment = without_trailing.rsplit('/').next().unwrap_or("repo");
    last_segment
        .strip_suffix(".git")
        .unwrap_or(last_segment)
        .to_string()
}

/// Check if a team-config agent entry (absolute path or dir name) matches a given agent name.
/// `agent_name` is the bare name (e.g., "dev-rust"), not prefixed.
pub(super) fn agent_matches(team_agent_entry: &str, agent_name: &str) -> bool {
    let entry_dir = Path::new(team_agent_entry)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(team_agent_entry);
    entry_dir == format!("_agent_{}", agent_name) || entry_dir == agent_name
}

/// Parse YAML frontmatter from a Role.md file.
/// Returns (name, description) if found. Strips a leading UTF-8 BOM first so a
/// `Role.md` saved as UTF-8-with-BOM still yields its display name/description
/// — mirrors `role_templates::parse_template_frontmatter`.
pub(super) fn parse_role_frontmatter(content: &str) -> (Option<String>, Option<String>) {
    let content = content.strip_prefix('\u{FEFF}').unwrap_or(content);
    if !content.starts_with("---") {
        return (None, None);
    }

    let rest = &content[3..];
    let end = match rest.find("---") {
        Some(i) => i,
        None => return (None, None),
    };

    let frontmatter = &rest[..end];
    let mut name = None;
    let mut description = None;

    for line in frontmatter.lines() {
        let trimmed = line.trim();
        if let Some(val) = trimmed.strip_prefix("name:") {
            name = Some(val.trim().trim_matches('"').trim_matches('\'').to_string());
        } else if let Some(val) = trimmed.strip_prefix("description:") {
            description = Some(val.trim().trim_matches('"').trim_matches('\'').to_string());
        }
    }

    (name, description)
}

pub(crate) fn build_role_content(
    safe_name: &str,
    description: &str,
    template: Option<&crate::commands::role_templates::ResolvedRoleTemplate>,
) -> String {
    let profile = template.map(|t| roles::RoleProfileRef {
        id: &t.id,
        body: &t.body,
    });
    roles::build_role_content(safe_name, description, profile.as_ref())
}

pub(crate) struct CreateAgentMatrixDiskArgs<'a> {
    pub project_path: &'a str,
    pub name: &'a str,
    pub description: &'a str,
    pub role_template_id: Option<&'a str>,
    pub settings: &'a AppSettings,
    pub config_dir: Option<&'a Path>,
}

#[derive(Debug, Clone)]
pub(crate) struct CreatedAgentMatrixOnDisk {
    pub agent_dir: PathBuf,
    pub display_name: String,
    pub safe_name: String,
    pub role_path: PathBuf,
}

pub(super) fn agent_matrix_display_name(project_path: &Path, safe_name: &str) -> String {
    let project_folder = project_path
        .file_name()
        .and_then(|n| n.to_str())
        .map(str::to_string)
        .unwrap_or_else(|| project_path.to_string_lossy().to_string());
    format!("{}/{}", project_folder, safe_name)
}

pub(crate) fn create_agent_matrix_on_disk(
    args: CreateAgentMatrixDiskArgs<'_>,
) -> Result<CreatedAgentMatrixOnDisk, String> {
    let safe_name = sanitize_name(args.name)?;
    let project = Path::new(args.project_path);
    let base = selected_ac_root(project)?;

    let agent_dir = base.join(format!("_agent_{}", safe_name));
    if matrix_directory_present(&agent_dir) {
        return Err(format!("Agent '{}' already exists", safe_name));
    }

    // Resolve the picked template before any target disk mutation so unknown or
    // unreadable template ids cannot leave a half-built matrix behind.
    let resolved_template: Option<crate::commands::role_templates::ResolvedRoleTemplate> =
        match args
            .role_template_id
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            Some(id) => {
                let config_dir = args
                    .config_dir
                    .ok_or_else(|| "Could not determine config directory".to_string())?;
                Some(crate::commands::role_templates::resolve_role_template(
                    id,
                    args.settings,
                    config_dir,
                )?)
            }
            None => None,
        };

    create_new_agent_matrix_layout(&agent_dir).map_err(|(sub, e)| match (sub, e.kind()) {
        ("agent_dir", std::io::ErrorKind::AlreadyExists) => {
            format!("Agent '{}' already exists", safe_name)
        }
        ("agent_dir", _) => format!("Failed to create agent directory: {}", e),
        _ => format!("Failed to create {} directory: {}", sub, e),
    })?;

    let role_content = build_role_content(&safe_name, args.description, resolved_template.as_ref());
    let role_path = roles::write_created_role(&agent_dir, role_content.as_bytes())?;

    if let Some(ref t) = resolved_template {
        if let Some(ref src) = t.skills_src {
            let dst = agent_dir.join("skills");
            let failures = crate::commands::role_templates::copy_dir_recursive(src, &dst);
            if !failures.is_empty() {
                log::error!(
                    "[entity_creation] some files from template '{}' skills/ could not be \
                     copied into {} ({} failure(s)): {}",
                    t.id,
                    dst.display(),
                    failures.len(),
                    failures.join("; ")
                );
            }
        }
    }

    write_local_config_value(
        &agent_dir.join("config.json"),
        default_agent_matrix_config(),
    )?;

    let display_name = agent_matrix_display_name(project, &safe_name);
    Ok(CreatedAgentMatrixOnDisk {
        agent_dir,
        display_name,
        safe_name,
        role_path,
    })
}

pub(crate) struct CreateAgentMatrixFromRoleArgs<'a> {
    pub ac_root: &'a Path,
    pub safe_name: &'a str,
    pub role_bytes: &'a [u8],
}

pub(crate) fn create_agent_matrix_from_role(
    args: CreateAgentMatrixFromRoleArgs<'_>,
) -> Result<CreatedAgentMatrixOnDisk, String> {
    if sanitize_name(args.safe_name)? != args.safe_name {
        return Err(format!(
            "Agent '{}' must already be a lowercase slug",
            args.safe_name
        ));
    }
    validate_existing_name(args.safe_name, "Agent")?;
    let agent_dir = args.ac_root.join(format!("_agent_{}", args.safe_name));
    if matrix_directory_present(&agent_dir) {
        return Err(format!("Agent '{}' already exists", args.safe_name));
    }
    create_new_agent_matrix_layout(&agent_dir).map_err(|(sub, e)| match (sub, e.kind()) {
        ("agent_dir", std::io::ErrorKind::AlreadyExists) => {
            format!("Agent '{}' already exists", args.safe_name)
        }
        ("agent_dir", _) => format!("Failed to create agent directory: {}", e),
        _ => format!("Failed to create {} directory: {}", sub, e),
    })?;

    let role_path = roles::write_created_role(&agent_dir, args.role_bytes)?;

    write_local_config_value(
        &agent_dir.join("config.json"),
        default_agent_matrix_config(),
    )?;

    let project_path = args
        .ac_root
        .parent()
        .ok_or_else(|| "Project AC Root has no project parent".to_string())?;
    Ok(CreatedAgentMatrixOnDisk {
        agent_dir,
        display_name: agent_matrix_display_name(project_path, args.safe_name),
        safe_name: args.safe_name.to_string(),
        role_path,
    })
}

pub(super) fn write_local_config_value(
    config_path: &Path,
    value: serde_json::Value,
) -> Result<(), String> {
    crate::config::agent_config::update_agent_config(config_path, |obj, _state| {
        let map = value
            .as_object()
            .ok_or_else(|| "Local config value must be a JSON object".to_string())?;
        *obj = map.clone();
        Ok(())
    })?;
    Ok(())
}
