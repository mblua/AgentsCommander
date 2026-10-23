use super::super::is_valid_profile_letter;
use super::super::physical_profile_rows;
use super::super::project_physical_profiles;
use super::super::raw_configuration_command;
use super::super::registered_profiles_for_read;
use super::super::AgentConfig;
use super::super::CodingAgentEnv;
use super::super::CodingAgentProfilesConfig;
use super::super::ProfileCellConfig;
use super::source_bytes_revision;
use super::SourceCoordinationError;
use super::SourceRevision;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Map;
use serde_json::Value;
use std::collections::BTreeMap;
use std::collections::HashSet;

pub(crate) fn identity_preimage(parts: impl IntoIterator<Item = impl AsRef<str>>) -> String {
    let mut scalars = Vec::new();
    for part in parts {
        scalars.extend(
            part.as_ref()
                .chars()
                .flat_map(char::to_lowercase)
                .filter(|scalar| !scalar.is_whitespace()),
        );
    }
    scalars.sort_unstable();
    scalars.into_iter().collect()
}

pub(crate) fn configuration_identity_preimage(command: &str, envs: &[CodingAgentEnv]) -> String {
    identity_preimage(
        std::iter::once(command).chain(
            envs.iter()
                .flat_map(|row| [row.key.as_str(), row.value.as_str()]),
        ),
    )
}

pub(crate) fn profile_identity_preimage(profile: &ProfileCellConfig) -> String {
    identity_preimage(
        std::iter::once(profile.command.as_str()).chain(
            profile
                .env
                .iter()
                .flat_map(|(key, value)| [key.as_str(), value.as_str()]),
        ),
    )
}

pub(crate) fn identity_digest(prefix: &str, preimage: &str) -> String {
    use sha2::{Digest, Sha256};
    format!("{prefix}{:x}", Sha256::digest(preimage.as_bytes()))
}

/// Includes every configured env row, even disabled rows. Labels, backend and
/// other metadata do not participate. Anagrams and repeated scalars are retained.
pub fn configuration_identity(command: &str, envs: &[CodingAgentEnv]) -> String {
    identity_digest("cid1:", &configuration_identity_preimage(command, envs))
}

pub(crate) fn configuration_identity_debug(command: &str, envs: &[CodingAgentEnv]) -> Vec<String> {
    std::iter::once(command.to_owned())
        .chain(
            envs.iter()
                .flat_map(|row| [row.key.clone(), row.value.clone()]),
        )
        .collect()
}

pub(crate) fn pair_identity_debug(
    command: &str,
    envs: &[CodingAgentEnv],
    profile: &ProfileCellConfig,
) -> Vec<String> {
    let mut leaves = configuration_identity_debug(command, envs);
    leaves.push(profile.command.clone());
    leaves.extend(
        profile
            .env
            .iter()
            .flat_map(|(key, value)| [key.clone(), value.clone()]),
    );
    leaves
}

/// Refresh informational metadata only; stable IDs and row order stay untouched.
pub fn refresh_agent_id_hashes(agents: &mut [AgentConfig]) {
    for agent in agents {
        agent.id_hash = Some(configuration_identity(&agent.command, &agent.envs));
        agent.id_hash_debug = Some(configuration_identity_debug(&agent.command, &agent.envs));
    }
}

/// Informational identity of one base/profile pair, using the existing scalar rules.
pub(crate) fn pair_identity(
    command: &str,
    envs: &[CodingAgentEnv],
    profile: &ProfileCellConfig,
) -> String {
    identity_digest(
        "cid1:",
        &identity_preimage([
            configuration_identity_preimage(command, envs),
            profile_identity_preimage(profile),
        ]),
    )
}

pub(crate) fn refresh_pair_id_hashes(
    agents: &[AgentConfig],
    profiles: &mut CodingAgentProfilesConfig,
) {
    for (id, cells) in &mut profiles.profiles_by_agent {
        // Historical duplicate IDs use the same first-match rule as spawn.
        let agent = agents.iter().find(|agent| agent.id == *id);
        for cell in cells.values_mut() {
            cell.id_hash = agent.map(|agent| pair_identity(&agent.command, &agent.envs, cell));
            cell.id_hash_debug =
                agent.map(|agent| pair_identity_debug(&agent.command, &agent.envs, cell));
        }
    }
}

/// Project only identity inputs from the first physical row. Protected groups
/// are best effort here; their existing full decoders still own save errors.
pub(crate) fn physical_pair_base(
    agents: Option<&Value>,
    id: &str,
) -> Option<(String, Vec<CodingAgentEnv>)> {
    let row = agents?
        .as_array()?
        .iter()
        .find(|row| row.get("id").and_then(Value::as_str) == Some(id))?
        .as_object()?;
    let command = raw_configuration_command(row, "baseCommand")
        .ok()?
        .unwrap_or_default();
    let envs = match row.get("envs") {
        Some(value) => serde_json::from_value(value.clone()).ok()?,
        None => Vec::new(),
    };
    Some((command.to_owned(), envs))
}

pub(crate) fn refresh_physical_pair_id_hashes(agents: Option<&Value>, profiles: &mut Value) {
    let Some(rows) = profiles
        .get_mut("profilesByAgent")
        .and_then(Value::as_object_mut)
    else {
        return;
    };
    for (id, cells) in rows {
        let base = physical_pair_base(agents, id);
        for raw in cells
            .as_object_mut()
            .into_iter()
            .flat_map(|cells| cells.values_mut())
        {
            let metadata = base.as_ref().and_then(|(command, envs)| {
                // Decode the cell's existing aliases/env contract without rewriting it.
                let cell: ProfileCellConfig = serde_json::from_value(raw.clone()).ok()?;
                Some((
                    pair_identity(command, envs, &cell),
                    pair_identity_debug(command, envs, &cell),
                ))
            });
            if let Some(object) = raw.as_object_mut() {
                match metadata {
                    Some((hash, debug)) => {
                        object.insert("id_hash".into(), Value::String(hash));
                        object.insert("id_hash_debug".into(), serde_json::json!(debug));
                    }
                    None => {
                        object.remove("id_hash");
                        object.remove("id_hash_debug");
                    }
                }
            }
        }
    }
}

/// A configured empty cell has an identity; an absent cell has no identity.
/// The base configuration ID and decorative profile letter are not inputs.
pub fn profile_identity(profile: &ProfileCellConfig) -> String {
    identity_digest("pid1:", &profile_identity_preimage(profile))
}

/// Borrowed projection lets each source retain its own complete agent model.
pub struct IdentitySnapshotEntry<'a> {
    pub key: &'a str,
    pub name: &'a str,
    pub command: &'a str,
    pub envs: &'a [CodingAgentEnv],
    pub profiles: Option<&'a BTreeMap<String, ProfileCellConfig>>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum IdentityValidationCode {
    DuplicateConfigurationIdentity,
    DuplicateProfileIdentity,
    IdentityHashCollision,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, thiserror::Error)]
#[error("{code:?}: existing Coding Agent '{existing_name}'")]
#[serde(rename_all = "camelCase")]
pub struct IdentityValidationError {
    pub code: IdentityValidationCode,
    pub existing_name: String,
    pub entry_hint: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requested_profile: Option<String>,
}

/// Validate one physical source at a time; this never compares other sources.
/// Historical duplicate policy and update exclusion are caller-owned: pass the
/// complete proposed snapshot, replacing the updated entry rather than appending.
pub fn validate_identity_snapshot(
    entries: &[IdentitySnapshotEntry<'_>],
) -> Result<(), IdentityValidationError> {
    validate_identity_snapshot_with_digest(entries, identity_digest)
}

pub(crate) fn validate_identity_snapshot_with_digest(
    entries: &[IdentitySnapshotEntry<'_>],
    digest: impl Fn(&str, &str) -> String,
) -> Result<(), IdentityValidationError> {
    let mut configurations = BTreeMap::<String, (String, &str)>::new();
    // Profile identities are local to an agent. Collision evidence, however,
    // must not be discarded simply because a second preimage is in another one.
    let mut profile_preimages = BTreeMap::<String, (String, &str)>::new();
    for entry in entries {
        let preimage = configuration_identity_preimage(entry.command, entry.envs);
        let id = digest("cid1:", &preimage);
        if let Some((existing_preimage, existing_name)) = configurations.get(&id) {
            return Err(IdentityValidationError {
                code: if existing_preimage == &preimage {
                    IdentityValidationCode::DuplicateConfigurationIdentity
                } else {
                    IdentityValidationCode::IdentityHashCollision
                },
                existing_name: (*existing_name).to_owned(),
                entry_hint: entry.key.to_owned(),
                requested_profile: None,
            });
        }
        configurations.insert(id, (preimage, entry.name));
        let mut local_profiles = BTreeMap::<String, &str>::new();
        for (letter, profile) in entry.profiles.into_iter().flat_map(|cells| cells.iter()) {
            let preimage = profile_identity_preimage(profile);
            let id = digest("pid1:", &preimage);
            if let Some((existing_preimage, existing_name)) = profile_preimages.get(&id) {
                if existing_preimage != &preimage {
                    return Err(IdentityValidationError {
                        code: IdentityValidationCode::IdentityHashCollision,
                        existing_name: (*existing_name).to_owned(),
                        entry_hint: entry.key.to_owned(),
                        requested_profile: Some(letter.clone()),
                    });
                }
            }
            profile_preimages
                .entry(id.clone())
                .or_insert((preimage, entry.name));
            if local_profiles.insert(id, letter).is_some() {
                return Err(IdentityValidationError {
                    code: IdentityValidationCode::DuplicateProfileIdentity,
                    existing_name: entry.name.to_owned(),
                    entry_hint: entry.key.to_owned(),
                    requested_profile: Some(letter.clone()),
                });
            }
        }
    }
    Ok(())
}

// Canonical pair signatures deliberately omit labels, order and supplied hashes.
// Counts retain repeated legacy rows, including repeated stable IDs.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum PhysicalIdentityViolation {
    Configuration([(String, String); 2]),
    Profile([(String, String, String); 2]),
}

pub(crate) fn physical_identity_violations(
    groups: &Map<String, Value>,
    configurations_editable: bool,
    profiles_editable: bool,
) -> Result<BTreeMap<PhysicalIdentityViolation, (usize, IdentityValidationError)>, String> {
    let agents: Vec<AgentConfig> = groups
        .get("agents")
        .map(|value| serde_json::from_value(value.clone()))
        .transpose()
        .map_err(|error| error.to_string())?
        .unwrap_or_default();
    let profiles: CodingAgentProfilesConfig = groups
        .get("codingAgentProfiles")
        .map(|value| serde_json::from_value(value.clone()))
        .transpose()
        .map_err(|error| error.to_string())?
        .unwrap_or_default();
    let mut violations = BTreeMap::new();
    let mut record = |key, error| {
        let entry = violations.entry(key).or_insert((0usize, error));
        entry.0 += 1;
    };
    if configurations_editable {
        collect_configuration_identity_violations(&agents, &mut record);
    }
    if profiles_editable {
        collect_profile_identity_violations(&agents, &profiles, &mut record);
    }
    Ok(violations)
}

pub(crate) fn collect_configuration_identity_violations(
    agents: &[AgentConfig],
    record: &mut impl FnMut(PhysicalIdentityViolation, IdentityValidationError),
) {
    let preimages: Vec<_> = agents
        .iter()
        .map(|agent| configuration_identity_preimage(&agent.command, &agent.envs))
        .collect();
    let hashes: Vec<_> = preimages
        .iter()
        .map(|p| identity_digest("cid1:", p))
        .collect();
    for (index, agent) in agents.iter().enumerate() {
        for other in 0..index {
            if hashes[index] != hashes[other]
                || (preimages[index] == preimages[other] && agent.id != agents[other].id)
            {
                continue;
            }
            let mut pair = [
                (agents[other].id.clone(), preimages[other].clone()),
                (agent.id.clone(), preimages[index].clone()),
            ];
            pair.sort();
            record(
                PhysicalIdentityViolation::Configuration(pair),
                IdentityValidationError {
                    code: if preimages[index] == preimages[other] {
                        IdentityValidationCode::DuplicateConfigurationIdentity
                    } else {
                        IdentityValidationCode::IdentityHashCollision
                    },
                    existing_name: agents[other].label.clone(),
                    entry_hint: agent.id.clone(),
                    requested_profile: None,
                },
            );
        }
    }
}

pub(crate) fn collect_profile_identity_violations(
    agents: &[AgentConfig],
    profiles: &CodingAgentProfilesConfig,
    record: &mut impl FnMut(PhysicalIdentityViolation, IdentityValidationError),
) {
    let group_for = |id: &str| {
        agents
            .iter()
            .find(|agent| agent.id == id)
            .map(|agent| configuration_identity(&agent.command, &agent.envs))
            .unwrap_or_else(|| format!("orphan:{id}"))
    };
    let mut groups = HashSet::new();
    let cells: Vec<_> = profiles
        .profiles_by_agent
        .iter()
        .filter(|(id, _)| groups.insert(group_for(id)))
        .flat_map(|(id, cells)| {
            cells
                .iter()
                .filter(|(letter, _)| is_valid_profile_letter(letter))
                .map(move |(letter, cell)| {
                    let preimage = profile_identity_preimage(cell);
                    let hash = identity_digest("pid1:", &preimage);
                    (id, letter, preimage, hash)
                })
        })
        .collect();
    for (index, (id, letter, preimage, hash)) in cells.iter().enumerate() {
        for (old_id, old_letter, old_preimage, old_hash) in &cells[..index] {
            if hash != old_hash || (id != old_id && preimage == old_preimage) {
                continue;
            }
            let mut pair = [
                (
                    group_for(old_id),
                    (*old_letter).clone(),
                    old_preimage.clone(),
                ),
                (group_for(id), (*letter).clone(), preimage.clone()),
            ];
            pair.sort();
            record(
                PhysicalIdentityViolation::Profile(pair),
                IdentityValidationError {
                    code: if preimage == old_preimage {
                        IdentityValidationCode::DuplicateProfileIdentity
                    } else {
                        IdentityValidationCode::IdentityHashCollision
                    },
                    existing_name: agents
                        .iter()
                        .find(|agent| &agent.id == *old_id)
                        .map(|agent| agent.label.clone())
                        .unwrap_or_else(|| (*old_id).clone()),
                    entry_hint: (*id).clone(),
                    requested_profile: Some((*letter).clone()),
                },
            );
        }
    }
}

pub(crate) fn validate_physical_identity_delta(
    before: &Map<String, Value>,
    after: &Map<String, Value>,
    configurations_editable: bool,
    profiles_editable: bool,
) -> Result<(), String> {
    let project = |groups: &Map<String, Value>| {
        let source = Value::Object(groups.clone());
        if physical_profile_rows(&source, true).is_err() {
            Ok(registered_profiles_for_read(&source))
        } else {
            project_physical_profiles(&source, true)
        }
    };
    let before_view = project(before)?;
    let after_view = project(after)?;
    let old = physical_identity_violations(
        before_view.as_object().unwrap(),
        configurations_editable,
        profiles_editable,
    )?;
    let new = physical_identity_violations(
        after_view.as_object().unwrap(),
        configurations_editable,
        profiles_editable,
    )?;
    for (signature, (count, mut error)) in new {
        if count > old.get(&signature).map_or(0, |(count, _)| *count) {
            // A prepended new row must still name an incumbent from BEFORE.
            if let PhysicalIdentityViolation::Configuration(pair) = &signature {
                let incumbent = before
                    .get("agents")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|raw| serde_json::from_value::<AgentConfig>(raw.clone()).ok())
                    .find(|agent| {
                        pair.contains(&(
                            agent.id.clone(),
                            configuration_identity_preimage(&agent.command, &agent.envs),
                        ))
                    });
                if let Some(incumbent) = incumbent {
                    error.existing_name = incumbent.label;
                }
            }
            return Err(error.to_string());
        }
    }
    Ok(())
}

/// Caller excludes the edited entry and supplies incumbent persisted names.
/// This computes a proposed name only; notices belong after the later commit.
pub fn identity_persisted_name(name: &str, occupied: &[String]) -> String {
    let names: HashSet<String> = occupied
        .iter()
        .map(|name| name.trim().to_lowercase())
        .collect();
    let available = |candidate: &str| !names.contains(&candidate.trim().to_lowercase());
    if available(name) {
        return name.to_owned();
    }
    let mut suffix = 1usize;
    loop {
        let candidate = format!("{} ({suffix})", name.trim());
        if available(&candidate) {
            return candidate;
        }
        suffix += 1;
    }
}

#[allow(dead_code)]
pub(crate) fn source_transition_digest<T: Serialize>(
    value: &T,
) -> Result<String, SourceCoordinationError> {
    let bytes = serde_json::to_vec(value).map_err(|_| SourceCoordinationError::Invalid)?;
    let SourceRevision::Bytes { sha256 } = source_bytes_revision(&bytes) else {
        unreachable!()
    };
    Ok(sha256)
}
