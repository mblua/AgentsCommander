use super::default_context::SELF_CLEAR_MANUAL_SECTION;
#[cfg(test)]
use super::ROOT_CONTEXT_TEST_CONFIG;
use super::{
    append_budget_summary, detect_git_branch, discover_skill_index, display_path,
    ensure_session_context_with_config, is_replica_agent_dir, managed_template_local_override,
    push_with_budget, resolved_paths_include_path, sanitize_skill_metadata_for_context,
    simple_hash, skill_trigger_text, strip_yaml_frontmatter, write_combined_context_file,
    SkillIndex, CONTEXT_TOKEN_GLOBAL, CONTEXT_TOKEN_REPOS, ROLE_MD_FILENAME,
    ROOT_AGENT_CONTEXT_TEMPLATE_FILENAME, SKILL_INDEX_TOTAL_MAX_BYTES,
};
use crate::config::instance_artifacts::CONTEXT_CACHE_DIR_NAME;
use crate::filesystem::projects::entities::roles;
use crate::filesystem::{directories, documents};
use std::collections::HashMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

/// Build a combined context file for a replica session.
/// Reads config.json from `cwd`, looks for `context[]` array.
/// Entries are resolved in order:
/// - `$AGENTSCOMMANDER_CONTEXT` → resolves to the global AgentsCommanderContext.md
/// - `$REPOS_WORKSPACE_INFO` → deprecated; skipped because repos render inside the global context
/// - Any other string → resolved as a path relative to `cwd`
///
/// After resolving context[], if `identity` is set in config.json and `<identity>/Role.md`
/// exists on disk, it is auto-appended (unless already resolved from context[]).
/// The global context is NOT auto-prepended — it is only included if the token is in the array.
///
/// Returns Ok(Some(path)) with the combined temp file, Ok(None) if no context[] field,
/// or Err with details about missing files.
pub fn build_replica_context(
    cwd: &str,
    repo_mounts: Option<&crate::pty::container_repos::RepoMountResolution>,
) -> Result<Option<String>, String> {
    build_replica_context_with_activation(cwd, repo_mounts, None)
}

pub(crate) fn build_replica_context_with_activation(
    cwd: &str,
    repo_mounts: Option<&crate::pty::container_repos::RepoMountResolution>,
    activation: Option<&crate::config::seed_manifest::ManifestActivationToken>,
) -> Result<Option<String>, String> {
    // #979: Root has a dedicated builder. This must stay the FIRST executable
    // line, ahead of the missing-config early return below: a Root without a
    // config.json would otherwise still get `Ok(None)` and fall back into the
    // token-aware generic path.
    if root_context_candidate(cwd) {
        return Err(format!(
            "Root identity requires directory {} to be built with build_root_agent_context, not \
                the replica builder",
            cwd
        ));
    }
    let cwd_path = std::path::Path::new(cwd);
    let config_path = cwd_path.join("config.json");

    // No config.json → no replica context, fall back to default behavior
    if !crate::filesystem::session_context::context_source_exists(&config_path) {
        return Ok(None);
    }

    let (config, identity) = if is_replica_agent_dir(cwd) {
        crate::config::replica_identity::read_and_repair_wg_replica_config(
            cwd_path,
            crate::config::replica_identity::WG_REPLICA_REQUIRED_CONTEXT,
        )?
    } else {
        let config_content = crate::filesystem::session_context::read_context_source(&config_path)
            .map_err(|e| format!("Failed to read {}: {}", config_path.display(), e))?;
        let config: serde_json::Value = serde_json::from_str(&config_content)
            .map_err(|e| format!("Failed to parse {}: {}", config_path.display(), e))?;
        let identity = crate::config::replica_identity::expected_wg_replica_identity(cwd_path).ok();
        match identity {
            Some(identity) => (config, identity),
            None => {
                return build_replica_context_from_config(
                    cwd,
                    cwd_path,
                    config,
                    None,
                    repo_mounts,
                    activation,
                );
            }
        }
    };

    build_replica_context_from_config(
        cwd,
        cwd_path,
        config,
        Some(identity),
        repo_mounts,
        activation,
    )
}

pub(crate) fn build_replica_context_from_config(
    cwd: &str,
    cwd_path: &Path,
    config: serde_json::Value,
    repaired_identity: Option<crate::config::replica_identity::WgReplicaIdentity>,
    repo_mounts: Option<&crate::pty::container_repos::RepoMountResolution>,
    activation: Option<&crate::config::seed_manifest::ManifestActivationToken>,
) -> Result<Option<String>, String> {
    // No "context" field → no replica context
    let context_array = match config.get("context").and_then(|v| v.as_array()) {
        Some(arr) if !arr.is_empty() => arr,
        _ => return Ok(None),
    };

    // Resolve and validate all paths (supporting special tokens)
    let mut resolved_paths: Vec<(String, std::path::PathBuf)> = Vec::new(); // (label, abs_path)
    let mut missing: Vec<String> = Vec::new();

    for entry in context_array {
        let raw = match entry.as_str() {
            Some(s) => s,
            None => continue,
        };

        if raw == CONTEXT_TOKEN_GLOBAL {
            let global_path =
                ensure_session_context_with_config(cwd, Some(&config), repo_mounts, activation)?;
            resolved_paths.push((
                "AgentsCommanderContext.md".to_string(),
                std::path::PathBuf::from(&global_path),
            ));
        } else if raw == CONTEXT_TOKEN_REPOS {
            log::debug!(
                "Skipping deprecated {} context token for {}",
                CONTEXT_TOKEN_REPOS,
                cwd
            );
        } else {
            let abs = cwd_path.join(raw);
            if documents::configured_context_exists(&abs) {
                let label = abs
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or(raw)
                    .to_string();
                resolved_paths.push((label, abs));
            } else {
                missing.push(raw.to_string());
            }
        }
    }

    // Auto-inject Role.md from identity matrix if present and not already resolved.
    let auto_role_abs = repaired_identity
        .map(|identity| roles::role_path(&identity.matrix_dir))
        .or_else(|| {
            config
                .get("identity")
                .and_then(|v| v.as_str())
                .map(|identity| cwd_path.join(format!("{}/{}", identity, ROLE_MD_FILENAME)))
        });
    if let Some(role_abs) = auto_role_abs {
        let already_included = resolved_paths_include_path(&resolved_paths, &role_abs);
        if !already_included && crate::filesystem::session_context::context_source_exists(&role_abs)
        {
            resolved_paths.push((ROLE_MD_FILENAME.to_string(), role_abs));
        }
    }

    if !missing.is_empty() {
        let replica_name = cwd_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown");
        return Err(format!(
            "Replica '{}' has missing context files:\n{}",
            replica_name,
            missing
                .iter()
                .map(|m| format!("  - {}", m))
                .collect::<Vec<_>>()
                .join("\n")
        ));
    }

    // Build combined content in context[] order, followed by any auto-injected role.
    let file_path = write_combined_context_file(cwd, &resolved_paths, "replica-context")?;

    log::info!(
        "Built replica context for {} ({} context files) → {}",
        cwd,
        resolved_paths.len(),
        file_path
    );

    Ok(Some(file_path))
}

/// #979: the Root Agent's dedicated context builder.
///
/// Root never enters `build_replica_context`, so the global sentinel token can
/// never reach that builder's `$AGENTSCOMMANDER_CONTEXT` branch and no
/// `Ok(None)` fallback can drop Root's mandatory governance. The code-owned
/// runtime prologue is always the first resolved path, even when `context[]` is
/// absent, null, non-array, or empty; the configured raw Root files follow in
/// their stored order.
pub(crate) fn root_context_candidate(root: &str) -> bool {
    super::super::root_agent::is_root_agent_dir_name(root)
        || super::super::root_agent::is_root_agent_path(root)
        || crate::filesystem::session_context::canonical_context_source(root)
            .ok()
            .is_some_and(|path| {
                super::super::root_agent::is_root_agent_dir_name(&display_path(&path))
            })
}

pub(crate) fn root_context_config_dir() -> Result<PathBuf, String> {
    #[cfg(test)]
    if let Some(dir) = ROOT_CONTEXT_TEST_CONFIG.with(|slot| slot.borrow().clone()) {
        return Ok(dir);
    }
    super::super::config_dir().ok_or_else(|| "Could not resolve app config directory".to_string())
}

pub(crate) fn verify_root_context_identity(root: &str) -> Result<(), String> {
    #[cfg(test)]
    if let Some(dir) = ROOT_CONTEXT_TEST_CONFIG.with(|slot| slot.borrow().clone()) {
        return if super::super::root_agent::is_root_agent_path_at(root, &dir) {
            Ok(())
        } else {
            Err(format!("Root identity validation failed for {}", root))
        };
    }
    if super::super::root_agent::is_root_agent_path(root) {
        Ok(())
    } else {
        Err(format!("Root identity validation failed for {}", root))
    }
}

pub(crate) fn root_conditions(
    mounts: Option<&crate::pty::container_repos::RepoMountResolution>,
    auto: bool,
) -> [bool; 4] {
    [
        mounts.is_none() && cfg!(target_os = "windows"),
        mounts.is_none() && cfg!(target_os = "linux"),
        mounts.is_none() && cfg!(target_os = "macos"),
        auto,
    ]
}

// Conditions are standalone, non-nesting lines. Values are interpreted only in
// selected original text; inserted paths and metadata never become template code.
pub(crate) fn render_root_template_text(
    source: &str,
    path: &Path,
    values: &HashMap<&str, String>,
    predicates: [bool; 4],
) -> Result<String, String> {
    const CONDITIONS: [&str; 4] = [
        "HOST_WINDOWS",
        "HOST_LINUX",
        "HOST_MACOS",
        "AUTO_SELF_CLEAR",
    ];
    const VALUES: [&str; 7] = [
        "AGENT_ROOT",
        "ROOT_MESSAGING_DIR",
        "APP_CONFIG_DIR",
        "AGENCY_CACHE_DIR",
        "ROOT_SKILLS_DIR",
        "SKILLS_LIST",
        "AGENT_REPOS_LIST",
    ];
    let mut selected = String::new();
    let mut active: Option<(usize, usize)> = None;
    for (line_index, line) in source.split_inclusive('\n').enumerate() {
        let marker = line.trim_end_matches(['\n', '\r']);
        let mut handled = false;
        for (index, name) in CONDITIONS.iter().enumerate() {
            if marker == format!("{{{{#{name}}}}}") {
                if active.is_some() {
                    return Err(format!(
                        "{}:{}: nested Root condition {}",
                        path.display(),
                        line_index + 1,
                        name
                    ));
                }
                active = Some((index, line_index + 1));
                handled = true;
                break;
            }
            if marker == format!("{{{{/{name}}}}}") {
                if active.map(|(open, _)| open) != Some(index) {
                    return Err(format!(
                        "{}:{}: unmatched Root condition {}",
                        path.display(),
                        line_index + 1,
                        name
                    ));
                }
                active = None;
                handled = true;
                break;
            }
        }
        if !handled && active.is_none_or(|(index, _)| predicates[index]) {
            selected.push_str(line);
        }
    }
    if let Some((index, line)) = active {
        return Err(format!(
            "{}:{}: unclosed Root condition {}",
            path.display(),
            line,
            CONDITIONS[index]
        ));
    }
    let mut out = String::with_capacity(selected.len());
    let mut rest = selected.as_str();
    let tokens: Vec<(&str, String)> = VALUES
        .iter()
        .map(|name| (*name, format!("{{{{{name}}}}}")))
        .collect();
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        rest = &rest[start..];
        if let Some((name, token)) = tokens.iter().find(|(_, token)| rest.starts_with(token)) {
            if let Some(value) = values.get(name) {
                out.push_str(value);
            }
            rest = &rest[token.len()..];
        } else {
            out.push_str("{{");
            rest = &rest[2..];
        }
    }
    out.push_str(rest);
    Ok(out)
}

pub(crate) fn render_root_skills_list(index: &SkillIndex) -> String {
    let mut output = String::new();
    let mut omitted_skills = 0;
    let mut omitted_warnings = 0;
    if index.skills.is_empty() {
        push_with_budget(
            &mut output,
            "No valid skills with parseable SKILL.md frontmatter were discovered.\n",
        );
    }
    for skill in &index.skills {
        let name = sanitize_skill_metadata_for_context(&skill.name);
        let entrypoint = sanitize_skill_metadata_for_context(&skill.entrypoint_path);
        let trigger =
            sanitize_skill_metadata_for_context(&match (&skill.description, &skill.when_to_use) {
                (None, None) => "No description metadata.".to_string(),
                _ => skill_trigger_text(skill),
            });
        let entry = format!(
            "- `{}` - {}\n  Scope: Root Agent durable skills\n  Entrypoint: `{}`\n",
            name, trigger, entrypoint
        );
        if !push_with_budget(&mut output, &entry) {
            let minimal = format!(
                "- `{}` - Metadata omitted because the skill index exceeded the \
                {} byte startup-context budget.\n  Scope: Root Agent durable skills\n  Entrypoint: \
                `{}`\n",
                name, SKILL_INDEX_TOTAL_MAX_BYTES, entrypoint
            );
            if !push_with_budget(&mut output, &minimal) {
                omitted_skills += 1;
            }
        }
    }
    for warning in &index.warnings {
        if !push_with_budget(
            &mut output,
            &format!(
                "- Discovery warning: {}\n",
                sanitize_skill_metadata_for_context(warning)
            ),
        ) {
            omitted_warnings += 1;
        }
        log::warn!("[skills] {}", warning);
    }
    for skill in &index.skills {
        for warning in &skill.metadata_warnings {
            let entry = format!(
                "- Discovery warning: `{}` (`{}`): {}\n",
                sanitize_skill_metadata_for_context(&skill.name),
                sanitize_skill_metadata_for_context(&skill.folder_name),
                sanitize_skill_metadata_for_context(warning)
            );
            if !push_with_budget(&mut output, &entry) {
                omitted_warnings += 1;
            }
            log::warn!("[skills] {}: {}", skill.folder_name, warning);
        }
    }
    append_budget_summary(&mut output, omitted_skills, omitted_warnings, false);
    output
}

pub(crate) fn render_root_repos_list(
    cwd: &Path,
    config: Option<&serde_json::Value>,
    mounts: Option<&crate::pty::container_repos::RepoMountResolution>,
) -> String {
    use crate::pty::container_repos::RepoOutcome;
    let mut out = String::new();
    if let Some(mounts) = mounts {
        for entry in &mounts.entries {
            match &entry.outcome {
                RepoOutcome::Mounted {
                    name,
                    host_path,
                    container_path,
                } => {
                    let branch = detect_git_branch(&host_path.to_string_lossy())
                        .unwrap_or_else(|| "unknown".to_string());
                    out.push_str(&format!(
                        "- **{}** — Path: `{}` — Branch: `{}`\n",
                        sanitize_skill_metadata_for_context(name),
                        sanitize_skill_metadata_for_context(container_path),
                        sanitize_skill_metadata_for_context(&branch)
                    ));
                }
                RepoOutcome::NotFound => {
                    let name = Path::new(&entry.config_entry)
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or(&entry.config_entry);
                    out.push_str(&format!(
                        "- **{}** — Path: `{}/{}` — **(NOT FOUND)**\n",
                        sanitize_skill_metadata_for_context(name),
                        crate::pty::container_repos::CONTAINER_REPOS_ROOT,
                        sanitize_skill_metadata_for_context(name)
                    ));
                }
            }
        }
    } else {
        for rel in config
            .and_then(|c| c.get("repos"))
            .and_then(|v| v.as_array())
            .into_iter()
            .flatten()
            .filter_map(|v| v.as_str())
        {
            let resolved = cwd.join(rel);
            let path = directories::canonical_context_repo(&resolved)
                .map(|p| display_path(&p))
                .unwrap_or_else(|_| display_path(&resolved));
            let name = resolved.file_name().and_then(|n| n.to_str()).unwrap_or(rel);
            let status = if directories::context_repo_exists(&resolved) {
                format!(
                    "Branch: `{}`",
                    sanitize_skill_metadata_for_context(
                        &detect_git_branch(&path).unwrap_or_else(|| "unknown".to_string())
                    )
                )
            } else {
                "**(NOT FOUND)**".to_string()
            };
            out.push_str(&format!(
                "- **{}** — Path: `{}` — {}\n",
                sanitize_skill_metadata_for_context(name),
                sanitize_skill_metadata_for_context(&path),
                status
            ));
        }
    }
    if out.is_empty() {
        out.push_str("No repos configured for the Root Agent.\n");
    }
    out
}

pub(crate) fn root_runtime_values(
    root: &str,
    config_dir: &Path,
    config: Option<&serde_json::Value>,
    mounts: Option<&crate::pty::container_repos::RepoMountResolution>,
) -> HashMap<&'static str, String> {
    let root_path = Path::new(root);
    let canonical = crate::filesystem::session_context::canonical_context_source(root_path)
        .unwrap_or_else(|_| root_path.to_path_buf());
    let index = discover_skill_index(Some(root));
    HashMap::from([
        ("AGENT_ROOT", display_path(&canonical)),
        (
            "ROOT_MESSAGING_DIR",
            display_path(&canonical.join(crate::phone::messaging::MESSAGING_DIR_NAME)),
        ),
        ("APP_CONFIG_DIR", display_path(config_dir)),
        (
            "AGENCY_CACHE_DIR",
            display_path(&config_dir.join(crate::commands::role_templates::AGENCY_TEMPLATES_DIR)),
        ),
        ("ROOT_SKILLS_DIR", display_path(&canonical.join("skills"))),
        ("SKILLS_LIST", render_root_skills_list(&index)),
        (
            "AGENT_REPOS_LIST",
            render_root_repos_list(root_path, config, mounts),
        ),
    ])
}

pub(crate) fn read_rendered_root_template(
    root: &str,
    path: &Path,
    config_dir: &Path,
    config: Option<&serde_json::Value>,
    mounts: Option<&crate::pty::container_repos::RepoMountResolution>,
    auto_self_clear: bool,
) -> Result<String, String> {
    let base = crate::filesystem::session_context::read_context_source(path)
        .map_err(|e| format!("Failed to read context file {}: {}", path.display(), e))?;
    let selected = managed_template_local_override(path).unwrap_or(base);
    render_root_template_text(
        &selected,
        path,
        &root_runtime_values(root, config_dir, config, mounts),
        root_conditions(mounts, auto_self_clear),
    )
}

pub(crate) fn write_root_context_cache(
    root: &str,
    config_dir: &Path,
    content: &str,
    prefix: &str,
) -> Result<String, String> {
    let dir = config_dir.join(CONTEXT_CACHE_DIR_NAME);
    crate::filesystem::session_context::ensure_context_directory(&dir)
        .map_err(|e| format!("Failed to create context-cache dir: {}", e))?;
    let path = dir.join(format!("{}-{}.md", prefix, simple_hash(root)));
    crate::filesystem::session_context::write_context_cache(&path, content)
        .map_err(|e| format!("Failed to write combined context file: {}", e))?;
    Ok(path.to_string_lossy().to_string())
}

pub(crate) fn build_root_agent_context_with_auto(
    cwd: &str,
    repo_mounts: Option<&crate::pty::container_repos::RepoMountResolution>,
    auto_self_clear: bool,
) -> Result<String, String> {
    verify_root_context_identity(cwd)?;
    let config_dir = root_context_config_dir()?;
    build_root_agent_context_at(cwd, &config_dir, repo_mounts, auto_self_clear)
}

// Caller has verified canonical Root identity. Explicit roots permit isolated
// fixtures without mutating production configuration.
pub(crate) fn build_root_agent_context_at(
    cwd: &str,
    config_dir: &Path,
    repo_mounts: Option<&crate::pty::container_repos::RepoMountResolution>,
    auto_self_clear: bool,
) -> Result<String, String> {
    let cwd_path = Path::new(cwd);
    let config_path = cwd_path.join("config.json");
    let config: Option<serde_json::Value> =
        match crate::filesystem::session_context::read_context_source(&config_path) {
            Ok(text) => Some(
                serde_json::from_str(&text)
                    .map_err(|e| format!("Failed to parse {}: {}", config_path.display(), e))?,
            ),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(format!("Failed to read {}: {}", config_path.display(), e)),
        };
    let defaults = serde_json::json!(["../Context.root-agent.md", "Role.md"]);
    super::super::root_agent::ensure_default_root_agent_skills_at(cwd_path)?;
    let entries = config
        .as_ref()
        .and_then(|c| c.get("context"))
        .unwrap_or(&defaults)
        .as_array();
    let canonical_template = config_dir.join(ROOT_AGENT_CONTEXT_TEMPLATE_FILENAME);
    let missing: Vec<&str> = entries
        .into_iter()
        .flatten()
        .filter_map(|e| e.as_str())
        .filter(|raw| *raw != CONTEXT_TOKEN_GLOBAL && *raw != CONTEXT_TOKEN_REPOS)
        .filter(|raw| !documents::configured_context_exists(&cwd_path.join(raw)))
        .collect();
    if !missing.is_empty() {
        return Err(format!(
            "Root Agent has missing context files:\n{}",
            missing
                .iter()
                .map(|raw| format!("  - {}", raw))
                .collect::<Vec<_>>()
                .join("\n")
        ));
    }
    let mut combined = String::new();
    let mut first = true;
    for raw in entries.into_iter().flatten().filter_map(|e| e.as_str()) {
        if raw == CONTEXT_TOKEN_GLOBAL || raw == CONTEXT_TOKEN_REPOS {
            log::debug!("Ignoring deprecated Root context token {}", raw);
            continue;
        }
        let path = cwd_path.join(raw);
        let label = path.file_name().and_then(|n| n.to_str()).unwrap_or(raw);
        let is_template = match (
            crate::filesystem::session_context::canonical_context_source(&path),
            crate::filesystem::session_context::canonical_context_source(&canonical_template),
        ) {
            (Ok(left), Ok(right)) => left == right,
            _ => path == canonical_template,
        };
        // Always read the base before accepting an override.
        let content = if is_template {
            read_rendered_root_template(
                cwd,
                &path,
                config_dir,
                config.as_ref(),
                repo_mounts,
                auto_self_clear,
            )?
        } else {
            let base = crate::filesystem::session_context::read_context_source(&path)
                .map_err(|e| format!("Failed to read context file {}: {}", path.display(), e))?;
            let selected = managed_template_local_override(&path).unwrap_or(base);
            if path.file_name() == Some(OsStr::new(ROLE_MD_FILENAME)) {
                strip_yaml_frontmatter(&selected).to_string()
            } else {
                selected
            }
        };
        if !first {
            combined.push_str(&format!("\n\n---\n\n# Context: {}\n\n", label));
        }
        first = false;
        combined.push_str(&content);
    }
    combined.push_str(SELF_CLEAR_MANUAL_SECTION);
    write_root_context_cache(cwd, config_dir, &combined, "replica-context")
}

pub(crate) fn build_direct_matrix_context(
    cwd: &str,
    repo_mounts: Option<&crate::pty::container_repos::RepoMountResolution>,
    activation: Option<&crate::config::seed_manifest::ManifestActivationToken>,
) -> Result<String, String> {
    let cwd_path = Path::new(cwd);
    let role_path = roles::role_path(cwd_path);
    let config_path = cwd_path.join("config.json");
    let config = if crate::filesystem::session_context::context_source_exists(&config_path) {
        let config_content = crate::filesystem::session_context::read_context_source(&config_path)
            .map_err(|e| format!("Failed to read {}: {}", config_path.display(), e))?;
        Some(
            serde_json::from_str::<serde_json::Value>(&config_content)
                .map_err(|e| format!("Failed to parse {}: {}", config_path.display(), e))?,
        )
    } else {
        None
    };
    let global_context =
        ensure_session_context_with_config(cwd, config.as_ref(), repo_mounts, activation)?;
    let mut resolved_paths = vec![(
        "AgentsCommanderContext.md".to_string(),
        std::path::PathBuf::from(&global_context),
    )];
    let mut missing: Vec<String> = Vec::new();

    if let Some(config) = &config {
        if let Some(context_array) = config.get("context").and_then(|v| v.as_array()) {
            for entry in context_array {
                let raw = match entry.as_str() {
                    Some(s) => s,
                    None => continue,
                };

                if raw == CONTEXT_TOKEN_GLOBAL {
                    // GLOBAL is resolved separately.
                } else if raw == CONTEXT_TOKEN_REPOS {
                    log::debug!(
                        "Skipping deprecated {} context token for {}",
                        CONTEXT_TOKEN_REPOS,
                        cwd
                    );
                } else {
                    let abs = cwd_path.join(raw);
                    if documents::configured_context_exists(&abs) {
                        if !resolved_paths_include_path(&resolved_paths, &abs) {
                            let label = abs
                                .file_name()
                                .and_then(|n| n.to_str())
                                .unwrap_or(raw)
                                .to_string();
                            resolved_paths.push((label, abs));
                        }
                    } else {
                        missing.push(raw.to_string());
                    }
                }
            }
        }
    }

    if !missing.is_empty() {
        let matrix_name = cwd_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown");
        return Err(format!(
            "Agent Matrix '{}' has missing context files:\n{}",
            matrix_name,
            missing
                .iter()
                .map(|m| format!("  - {}", m))
                .collect::<Vec<_>>()
                .join("\n")
        ));
    }

    if roles::role_present(cwd_path) && !resolved_paths_include_path(&resolved_paths, &role_path) {
        resolved_paths.push((ROLE_MD_FILENAME.to_string(), role_path));
    }

    if resolved_paths.len() == 1 {
        return Ok(global_context);
    }

    let file_path = write_combined_context_file(cwd, &resolved_paths, "matrix-context")?;

    log::info!(
        "Built direct matrix context for {} ({} context files) → {}",
        cwd,
        resolved_paths.len(),
        file_path
    );

    Ok(file_path)
}
