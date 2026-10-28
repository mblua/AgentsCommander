use super::model::AcReplicaSavedPair;
use crate::config::ac_root::find_ac_root_segment;
use std::path::{Path, PathBuf};

/// Resolve the preferred coding agent for a directory from its config.json
/// `tooling.lastCodingAgent` and that id's `codingAgents` descriptor.
///
/// #2434 - resolved through `resolve_portable_reference` (local id, cell digest,
/// case-insensitive label, canonical command), so a foreign id maps to OUR
/// agent. The sidebar lists rather than launches: no letter is requested and
/// only the resolved id is read. An unresolvable reference returns `None` with
/// a warning; another agent is never substituted.
pub(crate) fn read_preferred_agent_id(
    dir: &Path,
    settings: &crate::config::settings::AppSettings,
) -> Option<String> {
    let mut warnings = Vec::new();
    let preferred = read_preferred_agent_id_logged(dir, settings, &mut warnings);
    for warning in warnings {
        log::warn!("[discovery] {}", warning);
    }
    preferred
}

pub(crate) fn read_preferred_agent_id_logged(
    dir: &Path,
    settings: &crate::config::settings::AppSettings,
    warnings: &mut Vec<String>,
) -> Option<String> {
    let config_path = dir.join("config.json");
    // #2786 (C1) - through the shared loader: state file first, key-wise.
    let v = match crate::config::agent_config::read_agent_local_config_json(dir) {
        Ok(v) => v?,
        Err(e) => {
            warnings.push(format!(
                "malformed '{}', no coding-agent reference: {}",
                config_path.display(),
                e
            ));
            return None;
        }
    };
    let foreign_id = v.get("tooling")?.get("lastCodingAgent")?.as_str()?;
    let reference =
        crate::config::agent_command::StoredReference::from_config(Some(&v), foreign_id);
    match crate::config::agent_command::resolve_portable_reference(settings, &reference) {
        crate::config::agent_command::MatchOutcome::Matched(found) => Some(found.agent_id),
        crate::config::agent_command::MatchOutcome::NoMatch { reference } => {
            warnings.push(format!(
                "coding-agent reference '{}' in '{}' matches no configured agent",
                reference,
                config_path.display()
            ));
            None
        }
    }
}

/// Extract the origin project name from a resolved identity path.
/// Looks for the folder immediately before the rightmost Project AC Root marker.
pub(crate) fn extract_origin_project(identity_abs_path: &std::path::Path) -> Option<String> {
    let s = identity_abs_path.to_string_lossy().replace('\\', "/");
    let parts: Vec<&str> = s.split('/').collect();
    find_ac_root_segment(&parts).and_then(|i| (i > 0).then(|| parts[i - 1].to_string()))
}

pub(crate) struct ReplicaIdentityRead {
    pub(crate) config: Option<serde_json::Value>,
    pub(crate) identity: Option<crate::config::replica_identity::WgReplicaIdentity>,
    pub(crate) invalid_identity: bool,
}

/// #1941 - strict protection state for discovery: `invalid` carries its
/// diagnostic and is never reported as an unlocked/false default.
pub(crate) fn replica_selection_wire_fields(
    replica_dir: &Path,
) -> (Option<AcReplicaSavedPair>, Option<String>, Option<String>) {
    use crate::config::coding_agent_profiles::ReplicaSelectionState;
    match crate::config::coding_agent_profiles::read_replica_selection_state(replica_dir) {
        ReplicaSelectionState::Unlocked { pair, .. } => (
            pair.map(|pair| AcReplicaSavedPair {
                coding_agent_id: Some(pair.coding_agent_id),
                requested_profile: Some(pair.requested_profile),
            }),
            Some("unlocked".to_string()),
            None,
        ),
        ReplicaSelectionState::Locked { pair, .. } => (
            Some(AcReplicaSavedPair {
                coding_agent_id: Some(pair.coding_agent_id),
                requested_profile: Some(pair.requested_profile),
            }),
            Some("locked".to_string()),
            None,
        ),
        ReplicaSelectionState::Invalid { diagnostic } => {
            (None, Some("invalid".to_string()), Some(diagnostic))
        }
    }
}

pub(crate) fn read_replica_config_with_valid_identity(replica_dir: &Path) -> ReplicaIdentityRead {
    if !crate::config::agent_config::discovery_replica_config_exists(replica_dir) {
        return ReplicaIdentityRead {
            config: None,
            identity: None,
            invalid_identity: false,
        };
    }

    match crate::config::replica_identity::read_and_repair_wg_replica_config(
        replica_dir,
        crate::config::replica_identity::WG_REPLICA_REQUIRED_CONTEXT,
    ) {
        Ok((config, identity)) => ReplicaIdentityRead {
            config: Some(config),
            identity: Some(identity),
            invalid_identity: false,
        },
        Err(e) => {
            log::warn!(
                "[ac-discovery] Rejected invalid WG replica identity for '{}': {}",
                replica_dir.display(),
                e
            );
            let config = crate::config::agent_config::read_discovery_replica_json(replica_dir);
            ReplicaIdentityRead {
                config,
                identity: None,
                invalid_identity: true,
            }
        }
    }
}

pub(crate) fn origin_project_for_replica_identity(
    project_folder: &str,
    identity_read: &ReplicaIdentityRead,
) -> Option<String> {
    if identity_read.invalid_identity {
        return None;
    }
    identity_read
        .identity
        .as_ref()
        .and_then(|identity| extract_origin_project(&identity.matrix_dir))
        .or_else(|| Some(project_folder.to_string()))
}

/// Derive agent display name from its path.
/// Format: "{project_folder}/{agent_name}" where:
/// - project_folder = directory containing the Project AC Root
/// - agent_name = folder name with "_agent_" prefix stripped
pub(crate) fn agent_display_name(project_folder: &str, dir_name: &str) -> String {
    let agent_name = dir_name.strip_prefix("_agent_").unwrap_or(dir_name);
    format!("{}/{}", project_folder, agent_name)
}

/// Resolve an agent ref to a display name. Handles both relative refs
/// (e.g. "../_agent_tech-lead") and absolute paths.
/// For relative refs, uses project_folder as origin. For absolute paths,
/// extracts the origin project from the folder before the Project AC Root marker.
pub(crate) fn resolve_agent_ref(project_folder: &str, agent_ref: &str) -> String {
    let normalized = agent_ref.replace('\\', "/");
    let trimmed = normalized
        .trim_start_matches("../")
        .trim_start_matches("./");
    if trimmed.contains(':') || trimmed.starts_with('/') {
        // Absolute path: extract origin project from folder before the Project AC Root marker
        let parts: Vec<&str> = trimmed.split('/').collect();
        let origin = find_ac_root_segment(&parts)
            .and_then(|i| (i > 0).then_some(parts[i - 1]))
            .unwrap_or(project_folder);
        let dir_name = parts.last().unwrap_or(&trimmed);
        agent_display_name(origin, dir_name)
    } else {
        agent_display_name(project_folder, trimmed)
    }
}

/// Extract the first content line from a TASK.md-style markdown file,
/// skipping any YAML frontmatter block at the top (delimited by `---` on
/// its own line). Leading `# ` heading markers are stripped from the result.
///
/// Without this, a TASK.md that opens with `---` (frontmatter) sends the
/// literal `---` to the sidebar — the `wg.task` field then defeats the
/// frontend's frontmatter-stripping renderer (issue #161).
pub(crate) fn extract_task_first_line(content: &str) -> Option<String> {
    // Strip a UTF-8 BOM if present — `read_to_string` does not, and `str::trim`
    // does not treat U+FEFF as whitespace, so without this the frontmatter opener
    // check below sees `\u{FEFF}---` instead of `---` and the heading-strip below
    // leaves the BOM on the result.
    let content = content.strip_prefix('\u{FEFF}').unwrap_or(content);

    let mut lines = content.lines().peekable();

    // Drain leading blank lines first so a stray "\n\n" before the frontmatter
    // opener doesn't bypass detection and leak the literal `---` into the sidebar.
    let mut first_non_empty = lines.peek().map(|l| l.trim());
    while first_non_empty == Some("") {
        lines.next();
        first_non_empty = lines.peek().map(|l| l.trim());
    }

    // YAML frontmatter: opener `---` on the first non-empty line, drain through closer.
    // If the closer is missing, the for-loop drains the rest and `find` returns None.
    if first_non_empty == Some("---") {
        lines.next();
        for line in lines.by_ref() {
            if line.trim() == "---" {
                break;
            }
        }
    }

    lines
        .find(|l| !l.trim().is_empty())
        .map(|l| l.trim_start_matches("# ").to_string())
}

pub(crate) fn normalize_projected_path(p: &Path) -> PathBuf {
    crate::path_utils::normalize_windows_verbatim_path_buf(p)
}

pub(crate) fn projected_path_string(p: &Path) -> String {
    crate::path_utils::path_to_string_without_windows_verbatim_prefix(p)
}

pub(crate) type TaskFields = (Option<String>, Option<String>);

pub(crate) fn read_task_fields(wg_path: &Path) -> TaskFields {
    let task_path = wg_path.join("TASK.md");
    let Ok(content) = crate::filesystem::tasks::read_task_title_text(&task_path) else {
        return (None, None);
    };
    let task = extract_task_first_line(&content);
    let task_title = crate::commands::entity_creation::parse_task_title(&content);
    (task, task_title)
}
