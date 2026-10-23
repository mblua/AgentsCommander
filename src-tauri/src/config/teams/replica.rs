use super::enumerate_project_dirs;
use super::is_valid_wg_local_shape;
use super::split_project_prefix;
use crate::config::ac_root::existing_ac_root;
use crate::config::replica_identity::agent_bare_name_from_ref;
use crate::config::replica_identity::read_and_repair_wg_replica_config;
use crate::config::replica_identity::WG_REPLICA_REQUIRED_CONTEXT;
use std::path::Path;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WgCoordinatorReplica {
    pub project: String,
    pub team: String,
    pub wg_name: String,
    pub agent_name: String,
    pub replica_dir: PathBuf,
}

/// Normalize a path string to a stable comparison form.
///
/// - Strip the Windows extended-length (`\\?\`) prefix.
/// - Replace `\` with `/`.
/// - Lowercase (case-insensitive comparison; matches the convention used by
///   `cli/list_peers::norm_path`).
/// - Trim trailing `/`.
#[cfg(test)]
fn normalize_path_for_compare(s: &str) -> String {
    let stripped = s.strip_prefix(r"\\?\").unwrap_or(s);
    stripped
        .replace('\\', "/")
        .to_lowercase()
        .trim_end_matches('/')
        .to_string()
}

/// Logical resolution of `.` and `..` components in a path string (no disk
/// access). Used as a fallback when `std::fs::canonicalize()` fails because
/// the target doesn't exist — e.g., legacy identity refs that point to a
/// previous workspace folder name (see #299).
///
/// Returns a forward-slash string. `..` components above the root of an
/// absolute path are dropped; `..` components at the front of a purely
/// relative path are preserved.
#[cfg(test)]
pub(super) fn logical_path_resolve(path: &Path) -> String {
    use std::path::Component;
    let mut components: Vec<Component> = Vec::new();
    let mut rooted = false;
    for component in path.components() {
        match component {
            Component::Prefix(_) | Component::RootDir => {
                rooted = true;
                components.push(component);
            }
            Component::Normal(_) => {
                components.push(component);
            }
            Component::ParentDir => {
                if matches!(components.last(), Some(Component::Normal(_))) {
                    components.pop();
                } else if !rooted {
                    components.push(component);
                }
                // else: rooted with no Normal segments to pop → drop the `..`.
            }
            Component::CurDir => {}
        }
    }
    let mut out = String::new();
    for c in &components {
        match c {
            Component::Prefix(p) => {
                out.push_str(&p.as_os_str().to_string_lossy());
            }
            Component::RootDir => {
                out.push('/');
            }
            Component::Normal(s) => {
                if !out.is_empty() && !out.ends_with('/') {
                    out.push('/');
                }
                out.push_str(&s.to_string_lossy());
            }
            Component::ParentDir => {
                if !out.is_empty() && !out.ends_with('/') {
                    out.push('/');
                }
                out.push_str("..");
            }
            Component::CurDir => {}
        }
    }
    out
}

/// Stable comparison key for an identity ref path.
///
/// 1. Prefers `std::fs::canonicalize()` — same behavior as the legacy
///    `canonical_path_string` when the target exists on disk.
/// 2. Falls back to pure logical resolution when canonicalize fails (the
///    typical post-workspace-rename state, where both the team coordinator
///    ref and the replica identity ref point to a folder that no longer
///    exists — see #299).
///
/// Both branches feed `normalize_path_for_compare`, so two refs pointing at
/// the same conceptual location produce equal keys whether or not the
/// target exists on disk.
///
/// Identity-based authorization is preserved: the comparison uses only the
/// declared `identity` field from `config.json`, never the replica directory
/// name (`__agent_*`). A spoofed replica that declares a different matrix
/// path still produces a different key from the team coordinator ref.
#[cfg(test)]
pub(super) fn identity_compare_key(path: &Path) -> String {
    let raw = match std::fs::canonicalize(path) {
        Ok(canon) => canon.to_string_lossy().into_owned(),
        Err(_) => logical_path_resolve(path),
    };
    normalize_path_for_compare(&raw)
}

pub fn resolve_wg_coordinator_replica(
    ac_root: &Path,
    wg_dir: &Path,
) -> Option<WgCoordinatorReplica> {
    let project = ac_root.parent()?.file_name()?.to_str()?.to_string();
    let wg_name = wg_dir.file_name()?.to_str()?.to_string();
    let team = crate::config::entity_prefix::strip_entity_prefix(&wg_name)
        .and_then(|s| s.split_once('-').map(|(_, rest)| rest.to_string()))?;

    let team_dir = ac_root.join(format!("_team_{}", team));
    let team_config: serde_json::Value = std::fs::read_to_string(team_dir.join("config.json"))
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())?;
    let coordinator_ref = team_config.get("coordinator").and_then(|c| c.as_str())?;
    let coordinator_name = agent_bare_name_from_ref(coordinator_ref).ok()?;
    if !ac_root
        .join(format!("_agent_{}", coordinator_name))
        .is_dir()
    {
        return None;
    }

    for replica_entry in std::fs::read_dir(wg_dir).ok()?.flatten() {
        let replica_dir = replica_entry.path();
        if !replica_dir.is_dir() {
            continue;
        }
        let _dir_name = match replica_dir.file_name().and_then(|n| n.to_str()) {
            Some(n) if n.starts_with("__agent_") => n,
            _ => continue,
        };
        let Ok((_config, identity)) =
            read_and_repair_wg_replica_config(&replica_dir, WG_REPLICA_REQUIRED_CONTEXT)
        else {
            continue;
        };
        if identity.agent_name == coordinator_name {
            return Some(WgCoordinatorReplica {
                project,
                team,
                wg_name,
                agent_name: identity.agent_name,
                replica_dir,
            });
        }
    }

    None
}

pub fn verified_wg_coordinator_target(
    target: &str,
    project_paths: &[String],
) -> Option<WgCoordinatorReplica> {
    let (Some(project), local) = split_project_prefix(target) else {
        return None;
    };
    if !is_valid_wg_local_shape(local) {
        return None;
    }
    let (wg_name, agent_name) = local.split_once('/')?;

    for (project_name, project_dir) in enumerate_project_dirs(project_paths) {
        if project_name != project {
            continue;
        }
        let Some(ac_root) = existing_ac_root(&project_dir) else {
            continue;
        };
        let wg_dir = ac_root.join(wg_name);
        if !wg_dir.is_dir() {
            continue;
        }
        if let Some(resolved) = resolve_wg_coordinator_replica(&ac_root, &wg_dir) {
            if resolved.agent_name == agent_name {
                return Some(resolved);
            }
        }
    }

    None
}

// Privileged PTY-input routing is intentionally separate from broad message
// discovery and `can_communicate`.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PtyInputAuthorityKind {
    Coordinator,
    Root,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedPtyInputIdentity {
    pub canonical_fqn: String,
    pub project: String,
    pub workgroup: String,
    pub agent: String,
    pub replica_root: PathBuf,
    pub matrix_root: PathBuf,
    pub is_coordinator: bool,
    pub project_identity: crate::path_identity::VerifiedPathIdentity,
    pub ac_root_identity: crate::path_identity::VerifiedPathIdentity,
    pub workgroup_identity: crate::path_identity::VerifiedPathIdentity,
    pub replica_identity: crate::path_identity::VerifiedPathIdentity,
    pub matrix_identity: crate::path_identity::VerifiedPathIdentity,
    /// Permanent physical sender incarnation. This is derived only from the
    /// verified replica/root directory object and deliberately excludes mutable
    /// config bytes. It keys GET and permanent idempotency history.
    pub incarnation_fingerprint: String,
    /// Mutable authority/config snapshot used only while an operation can still
    /// actuate. Benign or authority-relevant config edits change this value.
    pub authority_fingerprint: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedPtyInputRoute {
    pub sender: VerifiedPtyInputIdentity,
    pub target: VerifiedPtyInputIdentity,
    pub kind: PtyInputAuthorityKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalSnapshotAuthorityKind {
    Coordinator,
    Root,
}

#[derive(Clone, PartialEq, Eq)]
pub struct VerifiedTerminalSnapshotRoute {
    pub sender: VerifiedPtyInputIdentity,
    pub target: VerifiedPtyInputIdentity,
    pub kind: TerminalSnapshotAuthorityKind,
}

impl std::fmt::Debug for VerifiedTerminalSnapshotRoute {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("VerifiedTerminalSnapshotRoute")
            .field("kind", &self.kind)
            .field("sender_is_coordinator", &self.sender.is_coordinator)
            .field("target_is_coordinator", &self.target.is_coordinator)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct TerminalSnapshotTargetIdentity {
    pub canonical_fqn: String,
    pub replica_root: PathBuf,
    pub project: String,
    pub workgroup: String,
    pub team: String,
    pub is_coordinator: bool,
}

impl std::fmt::Debug for TerminalSnapshotTargetIdentity {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TerminalSnapshotTargetIdentity")
            .field("is_coordinator", &self.is_coordinator)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct StrictPtyFqn {
    pub(super) project: String,
    pub(super) workgroup: String,
    pub(super) team: String,
    pub(super) agent: String,
}

fn forbidden_identity_scalar(ch: char) -> bool {
    ch.is_control()
        || matches!(
            ch,
            '\u{061c}'
                | '\u{200e}'
                | '\u{200f}'
                | '\u{202a}'..='\u{202e}'
                | '\u{2066}'..='\u{2069}'
        )
}

pub(super) fn parse_strict_pty_fqn(value: &str) -> Result<StrictPtyFqn, String> {
    if value.is_empty() || value.len() > 1_024 || value.matches(':').count() != 1 {
        return Err("invalid_target".to_string());
    }
    let (project, local) = value
        .split_once(':')
        .ok_or_else(|| "invalid_target".to_string())?;
    if project.is_empty()
        || matches!(project, "." | "..")
        || project.chars().any(forbidden_identity_scalar)
        || project
            .chars()
            .any(|ch| matches!(ch, '/' | '\\' | '*' | '?' | '"' | '<' | '>' | '|'))
        || local.matches('/').count() != 1
    {
        return Err("invalid_target".to_string());
    }
    let (workgroup, agent) = local
        .split_once('/')
        .ok_or_else(|| "invalid_target".to_string())?;
    let rest = crate::config::entity_prefix::strip_entity_prefix(workgroup)
        .ok_or_else(|| "invalid_target".to_string())?;
    let (digits, team) = rest
        .split_once('-')
        .ok_or_else(|| "invalid_target".to_string())?;
    if digits.is_empty()
        || !digits.bytes().all(|byte| byte.is_ascii_digit())
        || team.is_empty()
        || !team
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        || agent.is_empty()
        || !agent
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        return Err("invalid_target".to_string());
    }
    Ok(StrictPtyFqn {
        project: project.to_string(),
        workgroup: workgroup.to_string(),
        team: team.to_string(),
        agent: agent.to_string(),
    })
}

pub(crate) fn validate_pty_input_target_syntax(value: &str) -> Result<(), String> {
    let parsed = parse_strict_pty_fqn(value)?;
    let reconstructed = format!("{}:{}/{}", parsed.project, parsed.workgroup, parsed.agent);
    if reconstructed != value {
        return Err("invalid_target".to_string());
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TerminalSnapshotTargetSyntax {
    Workgroup,
    Origin,
    Root,
}

pub(super) fn terminal_snapshot_target_syntax(
    value: &str,
) -> Result<TerminalSnapshotTargetSyntax, String> {
    if value == crate::config::root_agent::ROOT_AGENT_SENDER {
        return Ok(TerminalSnapshotTargetSyntax::Root);
    }
    if validate_pty_input_target_syntax(value).is_ok() {
        return Ok(TerminalSnapshotTargetSyntax::Workgroup);
    }
    if value.is_empty()
        || value.len() > 1_024
        || value.contains(':')
        || value.matches('/').count() != 1
    {
        return Err("invalid_target".to_string());
    }
    let (project, agent) = value
        .split_once('/')
        .ok_or_else(|| "invalid_target".to_string())?;
    if [project, agent].iter().any(|component| {
        component.is_empty()
            || matches!(*component, "." | "..")
            || component.chars().any(forbidden_identity_scalar)
            || !component
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    }) {
        return Err("invalid_target".to_string());
    }
    Ok(TerminalSnapshotTargetSyntax::Origin)
}

pub(crate) fn validate_terminal_snapshot_target_syntax(value: &str) -> Result<(), String> {
    terminal_snapshot_target_syntax(value).map(|_| ())
}

pub(super) fn identity_fingerprint(
    identities: &[&crate::path_identity::VerifiedPathIdentity],
) -> String {
    use sha2::{Digest, Sha256};
    let mut digest = Sha256::new();
    digest.update(b"ac-pty-input-identity-v1");
    for identity in identities {
        digest.update(identity.object_id.volume.to_be_bytes());
        digest.update(identity.object_id.file.to_be_bytes());
        if let Some(content) = identity.content_sha256 {
            digest.update(content);
        }
    }
    format!("{:x}", digest.finalize())
}

pub(super) fn incarnation_fingerprint(
    anchor: &crate::path_identity::VerifiedPathIdentity,
) -> String {
    use sha2::{Digest, Sha256};
    let mut digest = Sha256::new();
    digest.update(b"ac-pty-input-sender-incarnation-v1");
    digest.update(anchor.object_id.volume.to_be_bytes());
    digest.update(anchor.object_id.file.to_be_bytes());
    format!("{:x}", digest.finalize())
}

fn read_identity_json(
    path: &Path,
) -> Result<
    (
        serde_json::Value,
        crate::path_identity::VerifiedPathIdentity,
    ),
    String,
> {
    let (bytes, identity) = crate::path_identity::read_bounded_regular(path, 1024 * 1024)?;
    let value = crate::path_identity::parse_json_no_duplicates(&bytes)?;
    if !value.is_object() {
        return Err("sender_identity_invalid".to_string());
    }
    Ok((value, identity))
}

fn identity_name_eq(left: &str, right: &str) -> bool {
    crate::path_identity::paths_equivalent(Path::new(left), Path::new(right))
}

pub(super) fn team_members(
    ac_root: &Path,
    team: &str,
) -> Result<
    (
        String,
        Vec<String>,
        crate::path_identity::VerifiedPathIdentity,
    ),
    String,
> {
    let team_dir = ac_root.join(format!("_team_{team}"));
    crate::path_identity::verify_directory(&team_dir)?;
    let (value, config_identity) = read_identity_json(&team_dir.join("config.json"))?;
    let coordinator = value
        .get("coordinator")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "sender_identity_invalid".to_string())
        .and_then(|value| {
            agent_bare_name_from_ref(value).map_err(|_| "sender_identity_invalid".to_string())
        })?;
    let values = value
        .get("agents")
        .and_then(serde_json::Value::as_array)
        .filter(|values| values.len() <= 1_024)
        .ok_or_else(|| "sender_identity_invalid".to_string())?;
    let mut members: Vec<String> = Vec::with_capacity(values.len());
    let mut seen: Vec<String> = Vec::with_capacity(values.len());
    for value in values {
        let raw = value
            .as_str()
            .ok_or_else(|| "sender_identity_invalid".to_string())?;
        let member =
            agent_bare_name_from_ref(raw).map_err(|_| "sender_identity_invalid".to_string())?;
        if seen
            .iter()
            .any(|existing| identity_name_eq(existing, &member))
        {
            return Err("sender_identity_invalid".to_string());
        }
        crate::path_identity::verify_directory(&ac_root.join(format!("_agent_{member}")))?;
        seen.push(member.clone());
        // #1245: the application's team-config writer requires the coordinator
        // to appear in `agents` (commands/entity_creation.rs::
        // normalize_team_config_for_project, and rule 4 of
        // docs/agent-matrix-conventions.md). The coordinator is returned
        // separately and must not also be counted as an ordinary member, so its
        // entry is consumed here instead of rejecting the whole config. `seen`
        // still covers it, so a repeated coordinator entry stays rejected.
        if !identity_name_eq(&member, &coordinator) {
            members.push(member);
        }
    }
    crate::path_identity::verify_directory(&ac_root.join(format!("_agent_{coordinator}")))?;
    Ok((coordinator, members, config_identity))
}

pub(super) fn verify_replica(
    project_dir: &Path,
    ac_root: &Path,
    parsed: &StrictPtyFqn,
) -> Result<VerifiedPtyInputIdentity, String> {
    let actual_project = project_dir
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| "invalid_target".to_string())?;
    if actual_project != parsed.project {
        return Err("invalid_target".to_string());
    }
    let replica_root = ac_root
        .join(&parsed.workgroup)
        .join(format!("__agent_{}", parsed.agent));
    let project_identity = crate::path_identity::verify_directory(project_dir)?;
    let ac_root_identity = crate::path_identity::verify_directory(ac_root)?;
    let workgroup_root = ac_root.join(&parsed.workgroup);
    let workgroup_identity = crate::path_identity::verify_directory(&workgroup_root)?;
    let replica_identity = crate::path_identity::verify_directory(&replica_root)?;
    if workgroup_identity
        .canonical_path
        .file_name()
        .and_then(|value| value.to_str())
        != Some(parsed.workgroup.as_str())
        || replica_identity
            .canonical_path
            .file_name()
            .and_then(|value| value.to_str())
            != Some(format!("__agent_{}", parsed.agent).as_str())
    {
        return Err("invalid_target".to_string());
    }
    let (replica_config, replica_config_identity) =
        read_identity_json(&replica_root.join("config.json"))?;
    // Use the repository's read-only identity reader, now bounded and
    // duplicate-aware, and require it to describe the retained security snapshot.
    let (read_only_config, resolved) =
        crate::config::replica_identity::read_wg_replica_config_read_only(&replica_root)
            .map_err(|_| "sender_identity_invalid".to_string())?;
    let persisted_identity = replica_config
        .get("identity")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "sender_identity_invalid".to_string())?;
    if read_only_config != replica_config
        || resolved.agent_name != parsed.agent
        || persisted_identity != resolved.identity
    {
        return Err("sender_identity_invalid".to_string());
    }
    let matrix_root = ac_root.join(format!("_agent_{}", parsed.agent));
    let matrix_identity = crate::path_identity::verify_directory(&matrix_root)?;
    if matrix_identity
        .canonical_path
        .file_name()
        .and_then(|value| value.to_str())
        != Some(format!("_agent_{}", parsed.agent).as_str())
    {
        return Err("invalid_target".to_string());
    }
    let (coordinator, members, team_config_identity) = team_members(ac_root, &parsed.team)?;
    let is_coordinator = identity_name_eq(&coordinator, &parsed.agent);
    if !is_coordinator
        && !members
            .iter()
            .any(|member| identity_name_eq(member, &parsed.agent))
    {
        return Err("target_not_member".to_string());
    }
    let canonical_fqn = format!("{}:{}/{}", actual_project, parsed.workgroup, parsed.agent);
    let authority_fingerprint = identity_fingerprint(&[
        &project_identity,
        &ac_root_identity,
        &workgroup_identity,
        &replica_identity,
        &replica_config_identity,
        &matrix_identity,
        &team_config_identity,
    ]);
    let incarnation_fingerprint = incarnation_fingerprint(&replica_identity);
    Ok(VerifiedPtyInputIdentity {
        canonical_fqn,
        project: actual_project.to_string(),
        workgroup: parsed.workgroup.clone(),
        agent: parsed.agent.clone(),
        replica_root,
        matrix_root,
        is_coordinator,
        project_identity,
        ac_root_identity,
        workgroup_identity,
        replica_identity,
        incarnation_fingerprint,
        matrix_identity,
        authority_fingerprint,
    })
}
