use super::{
    cleanup_temp_role, ensure_default_skill_file, merge_root_agent_config, normalize_role_text,
    paths_equivalent, publish_missing_role_file, replace_role_file, root_agent_dir, sync_role_dir,
    unique_role_temp_path, write_role_file, DefaultRootSkill, DEFAULT_ROOT_SKILLS,
    OLD_DEFERRED_MESSAGING_PARAGRAPH, OLD_ROOT_ROLE_MD, ROOT_AGENT_SKILLS_DIR,
    ROOT_CONTEXT_BEFORE_AGENCY_SKILL_MD, ROOT_CONTEXT_BEFORE_BOUNDARY_AUDIT_MD,
    ROOT_CONTEXT_BEFORE_ORCHESTRATOR_RENAME_MD, ROOT_CONTEXT_BEFORE_ROOM_RENAME_MD,
    ROOT_CONTEXT_BEFORE_TOKEN_MINIMIZATION_MD, ROOT_CONTEXT_BEFORE_WORKSPACE_PROSE_MD,
    ROOT_COORDINATION_MESSAGING_PARAGRAPH, ROOT_ROLE_MD,
};
use crate::config::ROOT_AGENT_DIR_NAME;
use std::path::Path;

pub(crate) const MINIMAL_ROOT_ROLE_MD: &str = r#"# Role

You are the personal Root Agent for AgentsCommander.
"#;

pub fn is_root_agent_dir_name(cwd: &str) -> bool {
    Path::new(cwd)
        .file_name()
        .and_then(|name| name.to_str())
        .map(|name| name.eq_ignore_ascii_case(ROOT_AGENT_DIR_NAME))
        .unwrap_or(false)
}

pub fn is_root_agent_path(cwd: &str) -> bool {
    let Ok(root_dir) = root_agent_dir() else {
        return false;
    };
    paths_equivalent(Path::new(cwd), Path::new(&root_dir))
}

/// Isolated context fixtures use the same canonical path comparison as live
/// Root routing, with an explicit configuration directory and existing roots.
#[cfg(test)]
pub(crate) fn is_root_agent_path_at(cwd: &str, config_dir: &Path) -> bool {
    let configured = config_dir.join(ROOT_AGENT_DIR_NAME);
    Path::new(cwd).is_dir() && configured.is_dir() && paths_equivalent(Path::new(cwd), &configured)
}

/// Read-only proof that `path` is the existing canonical Root Agent directory.
/// This wrapper never provisions or repairs Root state.
pub(crate) fn verify_live_root_agent_path(
    path: &Path,
) -> Result<crate::path_identity::VerifiedPathIdentity, String> {
    let configured = root_agent_dir()?;
    let configured_identity = crate::path_identity::verify_directory(Path::new(&configured))?;
    let supplied_identity = crate::path_identity::verify_directory(path)?;
    if configured_identity.object_id != supplied_identity.object_id {
        return Err("root_identity_invalid".to_string());
    }
    Ok(supplied_identity)
}

pub(crate) fn validate_root_agent_root_path(root_dir: &Path) -> Result<(), String> {
    match std::fs::symlink_metadata(root_dir) {
        Ok(metadata) => {
            if is_link_or_reparse(&metadata) || !metadata.is_dir() {
                return Err(format!(
                    "Root agent directory {} exists but is not a regular directory",
                    root_dir.display()
                ));
            }
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!(
            "Failed to inspect root agent directory {}: {}",
            root_dir.display(),
            e
        )),
    }
}

pub(crate) fn is_link_or_reparse(metadata: &std::fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }

    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;

        metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }

    #[cfg(not(windows))]
    {
        false
    }
}

pub fn ensure_root_agent_dir() -> Result<String, String> {
    let root_dir = root_agent_dir()?;
    ensure_root_agent_dir_at(Path::new(&root_dir))?;
    Ok(root_dir)
}

pub(crate) fn ensure_root_agent_dir_at(root_dir: &Path) -> Result<(), String> {
    validate_root_agent_root_path(root_dir)?;
    crate::commands::entity_creation::create_agent_matrix_layout(root_dir).map_err(
        |(sub, e)| {
            format!(
                "Failed to create root agent layout entry '{}' at {}: {}",
                sub,
                root_dir.display(),
                e
            )
        },
    )?;
    validate_root_agent_root_path(root_dir)?;
    ensure_default_root_agent_skills_at(root_dir)?;

    let messaging_dir = root_dir.join(crate::phone::messaging::MESSAGING_DIR_NAME);
    std::fs::create_dir_all(&messaging_dir).map_err(|e| {
        format!(
            "Failed to create root agent messaging directory at {}: {}",
            messaging_dir.display(),
            e
        )
    })?;

    let role_path = root_dir.join("Role.md");
    migrate_root_role(&role_path)?;

    merge_root_agent_config(&root_dir.join("config.json"))?;

    // #979: BEST-EFFORT, never `?`. `ensure_root_agent_dir` is fatal at
    // commands/session.rs:2741 and :3276, so a retirement error here would make the
    // Root Agent permanently uncreatable and unrestartable while the app itself
    // starts fine (lib.rs:798-800 and :1283-1289 only log and continue). A
    // surviving stale global is inert after this change: nothing reads it. The
    // governance steps above stay fail-closed.
    match root_dir.parent() {
        Some(config_dir) => {
            if let Err(e) =
                crate::config::seeded_context_templates::retire_standalone_global_context(
                    config_dir,
                )
            {
                log::warn!(
                    "[979] failed to retire standalone global context in {}: {}",
                    config_dir.display(),
                    e
                );
            }
        }
        None => log::warn!(
            "[979] cannot derive the config directory from root agent dir {}; skipping retirement",
            root_dir.display()
        ),
    }

    Ok(())
}

pub(crate) fn ensure_default_root_agent_skills_at(root_dir: &Path) -> Result<(), String> {
    validate_root_agent_root_path(root_dir)?;
    let skills_root = root_dir.join(ROOT_AGENT_SKILLS_DIR);
    std::fs::create_dir_all(&skills_root).map_err(|e| {
        format!(
            "Failed to create root agent skills directory at {}: {}",
            skills_root.display(),
            e
        )
    })?;
    validate_root_agent_root_path(root_dir)?;

    validate_root_agent_skills_root(&skills_root)?;

    for skill in DEFAULT_ROOT_SKILLS {
        ensure_default_root_skill_file(root_dir, &skills_root, skill)?;
    }

    Ok(())
}

pub(crate) fn ensure_default_root_skill_file(
    root_dir: &Path,
    skills_root: &Path,
    skill: &DefaultRootSkill,
) -> Result<(), String> {
    validate_root_agent_root_path(root_dir)?;
    validate_root_agent_skills_root(skills_root)?;
    let skill_dir = skills_root.join(skill.dir_name);
    match std::fs::symlink_metadata(&skill_dir) {
        Ok(metadata) => {
            if is_link_or_reparse(&metadata) || !metadata.is_dir() {
                return Err(format!(
                    "Root agent default skill path {} exists but is not a regular directory",
                    skill_dir.display()
                ));
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            match std::fs::create_dir(&skill_dir) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(e) => {
                    return Err(format!(
                        "Failed to create root agent default skill directory {}: {}",
                        skill_dir.display(),
                        e
                    ));
                }
            }
        }
        Err(e) => {
            return Err(format!(
                "Failed to inspect root agent default skill directory {}: {}",
                skill_dir.display(),
                e
            ));
        }
    }

    validate_root_agent_root_path(root_dir)?;
    validate_root_agent_skills_root(skills_root)?;
    let metadata = std::fs::symlink_metadata(&skill_dir).map_err(|e| {
        format!(
            "Failed to inspect root agent default skill directory {} after create: {}",
            skill_dir.display(),
            e
        )
    })?;
    if is_link_or_reparse(&metadata) || !metadata.is_dir() {
        return Err(format!(
            "Root agent default skill path {} exists but is not a regular directory",
            skill_dir.display()
        ));
    }

    let skill_path = skill_dir.join(skill.file_name);
    ensure_default_skill_file(
        root_dir,
        skills_root,
        &skill_path,
        skill.content,
        skill.legacy_snapshots,
    )
}

pub(crate) fn validate_root_agent_skills_root(skills_root: &Path) -> Result<(), String> {
    let metadata = std::fs::symlink_metadata(skills_root).map_err(|e| {
        format!(
            "Failed to inspect root agent skills directory {}: {}",
            skills_root.display(),
            e
        )
    })?;
    if is_link_or_reparse(&metadata) || !metadata.is_dir() {
        return Err(format!(
            "Root agent skills path {} exists but is not a regular directory",
            skills_root.display()
        ));
    }
    Ok(())
}

pub(crate) fn validate_default_skill_directory(skill_dir: &Path) -> Result<(), String> {
    let metadata = std::fs::symlink_metadata(skill_dir).map_err(|e| {
        format!(
            "Failed to inspect root agent default skill directory {}: {}",
            skill_dir.display(),
            e
        )
    })?;
    if is_link_or_reparse(&metadata) || !metadata.is_dir() {
        return Err(format!(
            "Root agent default skill path {} exists but is not a regular directory",
            skill_dir.display()
        ));
    }
    Ok(())
}

pub(crate) fn migrate_root_role(role_path: &Path) -> Result<(), String> {
    let root_dir = role_path.parent().ok_or_else(|| {
        format!(
            "Could not resolve root agent directory from {}",
            role_path.display()
        )
    })?;
    let config_dir = root_dir.parent().ok_or_else(|| {
        format!(
            "Could not resolve config directory from {}",
            role_path.display()
        )
    })?;
    // #979: Root provisioning no longer seeds the project templates
    // (`Context.AgentsCommander.md` / `Context.coordinator.md`). Only the Root
    // supplement below is seeded. `create_default_context_templates` itself stays:
    // project discovery (`commands/ac_discovery.rs`) and project registration
    // (`config/projects.rs`) still call it, and `config_dir` is still needed here
    // for the Root supplement path.
    let context_template_path =
        config_dir.join(crate::config::instance_artifacts::ROOT_AGENT_CONTEXT_TEMPLATE_FILENAME);

    migrate_root_context_template(&context_template_path)?;
    match create_missing_role(role_path, MINIMAL_ROOT_ROLE_MD)? {
        CreateMissingRole::Created => return Ok(()),
        CreateMissingRole::AlreadyExists => {}
    }

    let existing = std::fs::read_to_string(role_path)
        .map_err(|e| format!("Failed to read {}: {}", role_path.display(), e))?;
    let existing_normalized = normalize_role_text(&existing);
    let migrated = if existing_normalized == normalize_role_text(OLD_ROOT_ROLE_MD)
        || existing_normalized == normalize_role_text(ROOT_CONTEXT_BEFORE_BOUNDARY_AUDIT_MD)
        || existing_normalized == normalize_role_text(ROOT_CONTEXT_BEFORE_AGENCY_SKILL_MD)
        || existing_normalized == normalize_role_text(ROOT_CONTEXT_BEFORE_TOKEN_MINIMIZATION_MD)
        || existing_normalized == normalize_role_text(ROOT_CONTEXT_BEFORE_WORKSPACE_PROSE_MD)
        || existing_normalized == normalize_role_text(ROOT_CONTEXT_BEFORE_ORCHESTRATOR_RENAME_MD)
        || existing_normalized == normalize_role_text(ROOT_CONTEXT_BEFORE_ROOM_RENAME_MD)
        || existing_normalized == normalize_role_text(ROOT_ROLE_MD)
    {
        if existing_normalized != normalize_role_text(MINIMAL_ROOT_ROLE_MD) {
            Some(MINIMAL_ROOT_ROLE_MD.to_string())
        } else {
            None
        }
    } else if existing.contains(OLD_DEFERRED_MESSAGING_PARAGRAPH) {
        Some(existing.replace(
            OLD_DEFERRED_MESSAGING_PARAGRAPH,
            ROOT_COORDINATION_MESSAGING_PARAGRAPH,
        ))
    } else {
        None
    };

    if let Some(content) = migrated {
        atomic_write_role(role_path, &content)?;
    }

    Ok(())
}

pub(crate) fn migrate_root_context_template(context_template_path: &Path) -> Result<(), String> {
    let config_dir = context_template_path.parent().ok_or_else(|| {
        format!(
            "Could not resolve config directory from {}",
            context_template_path.display()
        )
    })?;
    crate::config::seeded_context_templates::ensure_root_context_template(config_dir)
}

pub(crate) fn atomic_write_role(role_path: &Path, content: &str) -> Result<(), String> {
    let parent = role_path.parent().ok_or_else(|| {
        format!(
            "Could not resolve parent directory for {}",
            role_path.display()
        )
    })?;
    let temp_path = unique_role_temp_path(role_path);
    let mut file = match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp_path)
    {
        Ok(file) => file,
        Err(e) => {
            return Err(format!(
                "Failed to create temporary role file {}: {}",
                temp_path.display(),
                e
            ))
        }
    };

    if let Err(e) = write_role_file(&mut file, role_path, content) {
        drop(file);
        cleanup_temp_role(&temp_path);
        return Err(e);
    }
    drop(file);

    if let Err(e) = replace_role_file(&temp_path, role_path) {
        cleanup_temp_role(&temp_path);
        return Err(e);
    }

    if let Ok(dir) = std::fs::File::open(parent) {
        if let Err(e) = dir.sync_all() {
            log::warn!(
                "Failed to sync root agent role directory {}: {}",
                parent.display(),
                e
            );
        }
    }

    Ok(())
}

pub(crate) enum CreateMissingRole {
    Created,
    AlreadyExists,
}

pub(crate) fn create_missing_role(
    role_path: &Path,
    content: &str,
) -> Result<CreateMissingRole, String> {
    let parent = role_path.parent().ok_or_else(|| {
        format!(
            "Could not resolve parent directory for {}",
            role_path.display()
        )
    })?;
    let temp_path = unique_role_temp_path(role_path);
    let mut file = match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp_path)
    {
        Ok(file) => file,
        Err(e) => {
            return Err(format!(
                "Failed to create temporary role file {}: {}",
                temp_path.display(),
                e
            ))
        }
    };

    if let Err(e) = write_role_file(&mut file, role_path, content) {
        drop(file);
        cleanup_temp_role(&temp_path);
        return Err(e);
    }
    drop(file);

    let published = match publish_missing_role_file(&temp_path, role_path) {
        Ok(published) => published,
        Err(e) => {
            cleanup_temp_role(&temp_path);
            return Err(e);
        }
    };

    cleanup_temp_role(&temp_path);

    if published {
        sync_role_dir(parent);
        Ok(CreateMissingRole::Created)
    } else {
        Ok(CreateMissingRole::AlreadyExists)
    }
}
