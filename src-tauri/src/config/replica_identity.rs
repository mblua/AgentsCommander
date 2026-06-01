use serde_json::Value;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::config::workspace::{ensure_authoritative_workspace_dir, find_workspace_ancestor};

pub const ROLE_MD_FILENAME: &str = "Role.md";
pub const WG_REPLICA_REQUIRED_CONTEXT: &[&str] =
    &["$AGENTSCOMMANDER_CONTEXT", "$REPOS_WORKSPACE_INFO"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WgReplicaIdentity {
    pub agent_name: String,
    pub workspace_dir: PathBuf,
    pub matrix_dir: PathBuf,
    pub identity: String,
}

fn strip_unc(path: PathBuf) -> PathBuf {
    let s = path.to_string_lossy();
    s.strip_prefix(r"\\?\").map(PathBuf::from).unwrap_or(path)
}

fn display_path(path: &Path) -> String {
    path.to_string_lossy()
        .trim_start_matches(r"\\?\")
        .to_string()
}

fn canonical_or_original(path: &Path) -> PathBuf {
    strip_unc(std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf()))
}

fn validate_agent_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("Agent name cannot be empty".to_string());
    }
    if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return Err(
            "Invalid Agent name: only alphanumeric characters and hyphens are allowed".to_string(),
        );
    }
    Ok(())
}

pub fn agent_bare_name_from_ref(raw_agent: &str) -> Result<String, String> {
    let trimmed = raw_agent.trim();
    if trimmed.is_empty() {
        return Err("Agent reference cannot be empty".to_string());
    }
    if trimmed.contains('\0') {
        return Err("Agent reference must not contain NUL".to_string());
    }
    let normalized = trimmed.replace('\\', "/");
    let dir_name = normalized
        .split('/')
        .rfind(|part| !part.is_empty())
        .ok_or_else(|| format!("Invalid agent reference '{}'", raw_agent))?;
    let bare = dir_name
        .strip_prefix("__agent_")
        .or_else(|| dir_name.strip_prefix("_agent_"))
        .unwrap_or(dir_name);
    validate_agent_name(bare)?;
    Ok(bare.to_string())
}

fn persisted_identity_agent_name(identity: &str) -> Result<String, String> {
    let trimmed = identity.trim();
    if trimmed.is_empty() {
        return Err("WG replica identity cannot be empty".to_string());
    }
    if trimmed.contains('\0') {
        return Err("WG replica identity must not contain NUL".to_string());
    }

    let normalized = trimmed.replace('\\', "/");
    let dir_name = normalized
        .split('/')
        .rfind(|part| !part.is_empty())
        .ok_or_else(|| format!("Invalid WG replica identity '{}'", identity))?;

    #[cfg(windows)]
    let bare = {
        if !dir_name
            .get(.."_agent_".len())
            .map(|prefix| prefix.eq_ignore_ascii_case("_agent_"))
            .unwrap_or(false)
        {
            return Err(format!(
                "WG replica identity '{}' must end in '_agent_<name>'",
                identity
            ));
        }
        &dir_name["_agent_".len()..]
    };

    #[cfg(not(windows))]
    let bare = dir_name.strip_prefix("_agent_").ok_or_else(|| {
        format!(
            "WG replica identity '{}' must end in '_agent_<name>'",
            identity
        )
    })?;

    validate_agent_name(bare)?;
    Ok(bare.to_string())
}

fn persisted_identity_names_match(persisted: &str, expected: &str) -> bool {
    if cfg!(windows) {
        persisted.eq_ignore_ascii_case(expected)
    } else {
        persisted == expected
    }
}

fn relative_path(from: &Path, to: &Path) -> Result<String, String> {
    let from_abs = canonical_or_original(from);
    let to_abs = canonical_or_original(to);

    let from_components: Vec<_> = from_abs.components().collect();
    let to_components: Vec<_> = to_abs.components().collect();
    let common = from_components
        .iter()
        .zip(to_components.iter())
        .take_while(|(a, b)| a == b)
        .count();

    if common == 0 {
        return Err(format!(
            "cannot compute same-workspace identity between '{}' and '{}'",
            display_path(from),
            display_path(to)
        ));
    }

    let mut result = PathBuf::new();
    for _ in common..from_components.len() {
        result.push("..");
    }
    for component in &to_components[common..] {
        result.push(component.as_os_str());
    }

    Ok(result.to_string_lossy().replace('\\', "/"))
}

fn validate_local_matrix(
    workspace_dir: &Path,
    matrix_dir: &Path,
    agent_name: &str,
) -> Result<(), String> {
    let expected_dir_name = format!("_agent_{}", agent_name);
    let actual_dir_name = matrix_dir
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            format!(
                "Agent Matrix '{}' has no valid directory name",
                matrix_dir.display()
            )
        })?;
    if actual_dir_name != expected_dir_name {
        return Err(format!(
            "WG replica identity target '{}' does not match expected '{}'",
            actual_dir_name, expected_dir_name
        ));
    }

    let metadata = std::fs::symlink_metadata(matrix_dir).map_err(|e| {
        format!(
            "Expected local Agent Matrix '{}' for WG replica identity, but it is not readable: {}",
            display_path(matrix_dir),
            e
        )
    })?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(format!(
            "Expected local Agent Matrix '{}' for WG replica identity, but it is not a real directory",
            display_path(matrix_dir)
        ));
    }

    let workspace_abs = std::fs::canonicalize(workspace_dir)
        .map(strip_unc)
        .map_err(|e| {
            format!(
                "Failed to canonicalize AC workspace '{}': {}",
                display_path(workspace_dir),
                e
            )
        })?;
    let matrix_abs = std::fs::canonicalize(matrix_dir)
        .map(strip_unc)
        .map_err(|e| {
            format!(
                "Failed to canonicalize Agent Matrix '{}': {}",
                display_path(matrix_dir),
                e
            )
        })?;
    if !matrix_abs.starts_with(&workspace_abs) {
        return Err(format!(
            "WG replica identity target '{}' escapes authoritative workspace '{}'",
            display_path(&matrix_abs),
            display_path(&workspace_abs)
        ));
    }

    Ok(())
}

/// Derive the only valid identity for a WG replica:
/// `<project>/<workspace>/wg-N-team/__agent_NAME` -> `<project>/<workspace>/_agent_NAME`.
pub fn expected_wg_replica_identity(replica_dir: &Path) -> Result<WgReplicaIdentity, String> {
    let replica_dir_name = replica_dir
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            format!(
                "Replica path '{}' has no valid directory name",
                replica_dir.display()
            )
        })?;
    let agent_name = replica_dir_name.strip_prefix("__agent_").ok_or_else(|| {
        format!(
            "WG replica directory '{}' must be named '__agent_<name>'",
            replica_dir_name
        )
    })?;
    validate_agent_name(agent_name)?;

    let wg_dir = replica_dir.parent().ok_or_else(|| {
        format!(
            "WG replica directory '{}' has no parent workgroup",
            display_path(replica_dir)
        )
    })?;
    let wg_name = wg_dir
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    if !wg_name.starts_with("wg-") {
        return Err(format!(
            "WG replica directory '{}' is not inside a wg-* workgroup",
            display_path(replica_dir)
        ));
    }

    let workspace_dir = find_workspace_ancestor(replica_dir).ok_or_else(|| {
        format!(
            "WG replica directory '{}' is not inside an AC workspace",
            display_path(replica_dir)
        )
    })?;
    ensure_authoritative_workspace_dir(&workspace_dir)?;

    let matrix_dir = workspace_dir.join(format!("_agent_{}", agent_name));
    validate_local_matrix(&workspace_dir, &matrix_dir, agent_name)?;
    let identity = relative_path(replica_dir, &matrix_dir)?;

    Ok(WgReplicaIdentity {
        agent_name: agent_name.to_string(),
        workspace_dir,
        matrix_dir,
        identity,
    })
}

pub fn validate_or_repair_wg_replica_identity(
    replica_dir: &Path,
    persisted_identity: Option<&str>,
) -> Result<WgReplicaIdentity, String> {
    let expected = expected_wg_replica_identity(replica_dir)?;
    let persisted = persisted_identity.ok_or_else(|| {
        format!(
            "WG replica '{}' has no config.json identity; expected '{}'",
            display_path(replica_dir),
            expected.identity
        )
    })?;
    let persisted_agent = persisted_identity_agent_name(persisted)?;
    if !persisted_identity_names_match(&persisted_agent, &expected.agent_name) {
        return Err(format!(
            "WG replica '{}' identity '{}' names '_agent_{}', but the replica directory requires '_agent_{}'",
            display_path(replica_dir),
            persisted,
            persisted_agent,
            expected.agent_name
        ));
    }
    Ok(expected)
}

fn is_identity_role_context_entry(entry: &str) -> bool {
    let normalized = entry.replace('\\', "/");
    let lower = normalized.to_ascii_lowercase();
    lower.ends_with("/role.md")
        && normalized
            .split('/')
            .any(|part| part.to_ascii_lowercase().starts_with("_agent_"))
}

pub fn normalize_wg_replica_context_entries(
    existing_context: &[String],
    required_prefix: &[&str],
    identity_rel: &str,
    role_exists: bool,
) -> Vec<String> {
    let mut result = Vec::new();
    let mut seen = HashSet::new();

    for required in required_prefix {
        if seen.insert((*required).to_string()) {
            result.push((*required).to_string());
        }
    }

    for entry in existing_context {
        if required_prefix.contains(&entry.as_str()) || is_identity_role_context_entry(entry) {
            continue;
        }
        if seen.insert(entry.clone()) {
            result.push(entry.clone());
        }
    }

    if role_exists {
        let role_entry = format!("{}/{}", identity_rel, ROLE_MD_FILENAME);
        if seen.insert(role_entry.clone()) {
            result.push(role_entry);
        }
    }

    result
}

pub fn repair_wg_replica_config_value(
    replica_dir: &Path,
    config: &mut Value,
    required_context_prefix: &[&str],
) -> Result<WgReplicaIdentity, String> {
    let identity = validate_or_repair_wg_replica_identity(
        replica_dir,
        config.get("identity").and_then(|value| value.as_str()),
    )?;
    config["identity"] = Value::String(identity.identity.clone());

    if let Some(context_array) = config.get("context").and_then(|value| value.as_array()) {
        let existing_context: Vec<String> = context_array
            .iter()
            .filter_map(|value| value.as_str().map(String::from))
            .collect();
        let role_exists = identity.matrix_dir.join(ROLE_MD_FILENAME).exists();
        config["context"] = serde_json::json!(normalize_wg_replica_context_entries(
            &existing_context,
            required_context_prefix,
            &identity.identity,
            role_exists,
        ));
    }

    Ok(identity)
}

pub fn read_and_repair_wg_replica_config(
    replica_dir: &Path,
    required_context_prefix: &[&str],
) -> Result<(Value, WgReplicaIdentity), String> {
    let config_path = replica_dir.join("config.json");
    let original = std::fs::read_to_string(&config_path)
        .map_err(|e| format!("Failed to read {}: {}", config_path.display(), e))?;
    let mut config: Value = serde_json::from_str(&original)
        .map_err(|e| format!("Failed to parse {}: {}", config_path.display(), e))?;
    let original_config = config.clone();
    let identity =
        repair_wg_replica_config_value(replica_dir, &mut config, required_context_prefix)?;
    if config != original_config {
        let repaired = serde_json::to_string_pretty(&config)
            .map_err(|e| format!("Failed to serialize {}: {}", config_path.display(), e))?;
        std::fs::write(&config_path, repaired)
            .map_err(|e| format!("Failed to repair {}: {}", config_path.display(), e))?;
    }
    Ok((config, identity))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup_replica(project_workspace_name: &str) -> tempfile::TempDir {
        let temp = tempfile::tempdir().expect("tempdir");
        let workspace = temp
            .path()
            .join("AgentsCommander_ac")
            .join(project_workspace_name);
        let matrix = workspace.join("_agent_tech-lead");
        let replica = workspace.join("wg-2-dev-team").join("__agent_tech-lead");
        std::fs::create_dir_all(&matrix).expect("create matrix");
        std::fs::create_dir_all(&replica).expect("create replica");
        std::fs::write(matrix.join(ROLE_MD_FILENAME), "# Tech Lead\n").expect("write Role.md");
        temp
    }

    #[test]
    fn expected_identity_is_same_workspace_for_canonical_ac() {
        let temp = setup_replica(".ac");
        let replica = temp
            .path()
            .join("AgentsCommander_ac")
            .join(".ac")
            .join("wg-2-dev-team")
            .join("__agent_tech-lead");
        let stale_sibling = temp
            .path()
            .join("agentscommander-old")
            .join(".ac")
            .join("_agent_tech-lead");
        std::fs::create_dir_all(stale_sibling).expect("create stale sibling");

        let identity = expected_wg_replica_identity(&replica).expect("identity");

        assert_eq!(identity.identity, "../../_agent_tech-lead");
        assert!(identity.matrix_dir.ends_with(
            Path::new("AgentsCommander_ac")
                .join(".ac")
                .join("_agent_tech-lead")
        ));
    }

    #[test]
    fn read_and_repair_rewrites_stale_identity_and_role_context() {
        let temp = setup_replica(".ac");
        let replica = temp
            .path()
            .join("AgentsCommander_ac")
            .join(".ac")
            .join("wg-2-dev-team")
            .join("__agent_tech-lead");
        let stale = temp
            .path()
            .join("agentscommander-old")
            .join(".ac")
            .join("_agent_tech-lead")
            .to_string_lossy()
            .replace('\\', "/");
        std::fs::write(
            replica.join("config.json"),
            serde_json::json!({
                "identity": stale,
                "context": [
                    "$AGENTSCOMMANDER_CONTEXT",
                    "../../../../agentscommander-old/.ac/_agent_tech-lead/Role.md",
                    "notes.md",
                    "../../_agent_tech-lead/Role.md"
                ],
                "repos": []
            })
            .to_string(),
        )
        .expect("write config");

        let (config, identity) =
            read_and_repair_wg_replica_config(&replica, WG_REPLICA_REQUIRED_CONTEXT)
                .expect("repair config");

        assert_eq!(identity.identity, "../../_agent_tech-lead");
        assert_eq!(config["identity"], "../../_agent_tech-lead");
        assert_eq!(
            config["context"],
            serde_json::json!([
                "$AGENTSCOMMANDER_CONTEXT",
                "$REPOS_WORKSPACE_INFO",
                "notes.md",
                "../../_agent_tech-lead/Role.md"
            ])
        );

        let persisted: Value = serde_json::from_str(
            &std::fs::read_to_string(replica.join("config.json")).expect("read config"),
        )
        .expect("parse config");
        assert_eq!(persisted["identity"], "../../_agent_tech-lead");
        assert!(!persisted.to_string().contains("agentscommander-old"));
    }

    #[test]
    fn repair_rejects_identity_for_different_agent_basename() {
        let temp = setup_replica(".ac");
        let replica = temp
            .path()
            .join("AgentsCommander_ac")
            .join(".ac")
            .join("wg-2-dev-team")
            .join("__agent_tech-lead");
        std::fs::write(
            replica.join("config.json"),
            r#"{"identity":"../../../../agentscommander-old/.ac/_agent_architect"}"#,
        )
        .expect("write config");

        let err = read_and_repair_wg_replica_config(&replica, WG_REPLICA_REQUIRED_CONTEXT)
            .expect_err("mismatched identity must reject");

        assert!(err.contains("_agent_architect"), "{err}");
        assert!(err.contains("_agent_tech-lead"), "{err}");
    }

    #[test]
    fn repair_rejects_bare_persisted_identity_component() {
        let temp = setup_replica(".ac");
        let replica = temp
            .path()
            .join("AgentsCommander_ac")
            .join(".ac")
            .join("wg-2-dev-team")
            .join("__agent_tech-lead");
        std::fs::write(replica.join("config.json"), r#"{"identity":"tech-lead"}"#)
            .expect("write config");

        let err = read_and_repair_wg_replica_config(&replica, WG_REPLICA_REQUIRED_CONTEXT)
            .expect_err("bare identity basename must reject");

        assert!(err.contains("must end in '_agent_<name>'"), "{err}");
        let persisted: Value = serde_json::from_str(
            &std::fs::read_to_string(replica.join("config.json")).expect("read config"),
        )
        .expect("parse config");
        assert_eq!(persisted["identity"], "tech-lead");
    }

    #[test]
    fn repair_rejects_replica_persisted_identity_component() {
        let temp = setup_replica(".ac");
        let replica = temp
            .path()
            .join("AgentsCommander_ac")
            .join(".ac")
            .join("wg-2-dev-team")
            .join("__agent_tech-lead");
        std::fs::write(
            replica.join("config.json"),
            r#"{"identity":"../../__agent_tech-lead"}"#,
        )
        .expect("write config");

        let err = read_and_repair_wg_replica_config(&replica, WG_REPLICA_REQUIRED_CONTEXT)
            .expect_err("replica identity basename must reject");

        assert!(err.contains("must end in '_agent_<name>'"), "{err}");
    }

    #[cfg(windows)]
    #[test]
    fn repair_accepts_windows_case_variant_persisted_identity_component() {
        let temp = setup_replica(".ac");
        let replica = temp
            .path()
            .join("AgentsCommander_ac")
            .join(".ac")
            .join("wg-2-dev-team")
            .join("__agent_tech-lead");
        std::fs::write(
            replica.join("config.json"),
            r#"{"identity":"../../../../agentscommander-old/.ac/_Agent_Tech-Lead"}"#,
        )
        .expect("write config");

        let (config, identity) =
            read_and_repair_wg_replica_config(&replica, WG_REPLICA_REQUIRED_CONTEXT)
                .expect("Windows identity basename matching is case-insensitive");

        assert_eq!(identity.identity, "../../_agent_tech-lead");
        assert_eq!(config["identity"], "../../_agent_tech-lead");
    }
}
