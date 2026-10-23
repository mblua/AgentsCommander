use serde::{Deserialize, Serialize};

use crate::config::settings::{
    finalize_agent_order, normalize_agent_order, normalize_container_image_input,
    validate_and_repair_settings, AgentConfig, AppSettings, CodingAgentEnv, ConfigSeedConfig,
};
use crate::pty::backend::SessionBackendKind;

/// Default custom-agent color when none is supplied (matches OnboardingModal).
pub const DEFAULT_CUSTOM_COLOR: &str = "#6366f1";

/// A settings.agents mutation. Serialized (camelCase, internally tagged on
/// `kind`) into the request file: `{ "kind": "add", "agent": { ... } }`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum CodingAgentOp {
    /// Add a new agent. `id` is already minted and syntactically valid CLI-side.
    Add { agent: AgentConfig },
    /// Patch selected fields of the agent with this exact `id`.
    Update { id: String, patch: AgentPatch },
    /// Remove the agent with this exact `id`.
    Remove { id: String },
}

/// Partial update for `coding-agent update`. Every field is optional; only the
/// provided ones change. `envs: Some(vec)` replaces the WHOLE env list
/// (`Some(empty)` clears it, matching `update_coding_agent_env_settings`).
#[derive(Debug, Clone, Default, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AgentPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(rename = "baseCommand")]
    pub command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub envs: Option<Vec<CodingAgentEnv>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub isolated_home: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions_filename: Option<String>,
    #[serde(default)]
    pub clear_instructions_filename: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_seed_dest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_seed_enabled: Option<bool>,
    #[serde(default)]
    pub clear_config_seed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend_kind: Option<SessionBackendKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub container_image: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub clear_container_image: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgentPatchWire {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(
        default,
        deserialize_with = "crate::config::settings::deserialize_present_command"
    )]
    pub command: Option<String>,
    #[serde(
        default,
        rename = "baseCommand",
        deserialize_with = "crate::config::settings::deserialize_present_command"
    )]
    pub canonical_command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub envs: Option<Vec<CodingAgentEnv>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub isolated_home: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions_filename: Option<String>,
    #[serde(default)]
    pub clear_instructions_filename: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_seed_dest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_seed_enabled: Option<bool>,
    #[serde(default)]
    pub clear_config_seed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend_kind: Option<SessionBackendKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub container_image: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub clear_container_image: bool,
}

impl<'de> Deserialize<'de> for AgentPatch {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let wire = AgentPatchWire::deserialize(deserializer)?;
        let command = crate::config::settings::select_configuration_command(
            wire.canonical_command.as_deref(),
            wire.command.as_deref(),
        )
        .map(str::to_owned);
        Ok(Self {
            label: wire.label,
            command,
            color: wire.color,
            envs: wire.envs,
            isolated_home: wire.isolated_home,
            instructions_filename: wire.instructions_filename,
            clear_instructions_filename: wire.clear_instructions_filename,
            config_seed_dest: wire.config_seed_dest,
            config_seed_enabled: wire.config_seed_enabled,
            clear_config_seed: wire.clear_config_seed,
            backend_kind: wire.backend_kind,
            container_image: wire.container_image,
            clear_container_image: wire.clear_container_image,
        })
    }
}

fn is_false(value: &bool) -> bool {
    !*value
}

/// Result of a successful `apply_coding_agent_op`. `op` is `"add"|"update"|"remove"`.
#[derive(Debug, Clone)]
pub struct CodingAgentOpOutcome {
    pub op: &'static str,
    pub agent: Option<AgentConfig>,
    pub removed_id: Option<String>,
}

/// Validate a user-supplied custom agent id: `^[a-z0-9][a-z0-9_-]{0,63}$`.
/// Deliberately distinct from (not a superset of) the catalog key rule
/// (`^[a-z0-9-]+$`): first-char alnum + a 64-char cap, with `_` allowed so
/// FE-minted `agent_<ms>_<n>` ids conform. Lowercase-only because
/// `find_launch_agent` resolves ids case-insensitively.
pub fn validate_custom_agent_id(id: &str) -> Result<(), String> {
    let bytes = id.as_bytes();
    if bytes.is_empty() {
        return Err("agent id must not be empty".to_string());
    }
    if bytes.len() > 64 {
        return Err(format!("agent id '{id}' must be at most 64 characters"));
    }
    let first = bytes[0];
    if !(first.is_ascii_lowercase() || first.is_ascii_digit()) {
        return Err(format!(
            "agent id '{id}' must start with a lowercase letter or digit"
        ));
    }
    if !bytes
        .iter()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-' || *b == b'_')
    {
        return Err(format!(
            "agent id '{id}' must match ^[a-z0-9][a-z0-9_-]{{0,63}}$ \
             (lowercase letters, digits, hyphen, underscore)"
        ));
    }
    Ok(())
}

/// Validate an explicit `--color` flag value: strict `^#[0-9a-fA-F]{6}$`.
pub fn validate_agent_color(color: &str) -> Result<(), String> {
    let bytes = color.as_bytes();
    let ok = bytes.len() == 7 && bytes[0] == b'#' && bytes[1..].iter().all(u8::is_ascii_hexdigit);
    if ok {
        Ok(())
    } else {
        Err(format!(
            "color '{color}' must be a 6-digit hex value like #RRGGBB"
        ))
    }
}

/// Mint an agent id in the FE-compatible shape `agent_<unix_ms>_<6 hex>`.
/// Collision-safe for scripted loops (the uuid suffix disambiguates same-ms
/// mints); conforms to `validate_custom_agent_id`.
pub fn mint_agent_id() -> String {
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let uuid = uuid::Uuid::new_v4().simple().to_string();
    format!("agent_{}_{}", ms, &uuid[..6])
}

/// Error message for an unknown selector id that lists the available ids.
pub fn unknown_agent_id_error(id: &str, agents: &[AgentConfig]) -> String {
    let available: Vec<&str> = agents.iter().map(|a| a.id.as_str()).collect();
    let list = if available.is_empty() {
        "(none)".to_string()
    } else {
        available.join(", ")
    };
    format!("agent id '{id}' not found. Available ids: {list}")
}

/// #2306 P1 - direction of a registered-agent move request. Serializes as the
/// `"up"`/`"down"` strings the picker sends.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentMoveDirection {
    Up,
    Down,
}

/// #786 R8: PURE op application. Mutates `settings` in memory and runs the shared
/// validator/repair suite; NEVER saves to disk. Both callers own persistence.
pub fn apply_coding_agent_op(
    settings: &mut AppSettings,
    op: &CodingAgentOp,
) -> Result<CodingAgentOpOutcome, String> {
    let outcome = match op {
        CodingAgentOp::Add { agent } => {
            validate_custom_agent_id(&agent.id)?;
            if settings
                .agents
                .iter()
                .any(|a| a.id.eq_ignore_ascii_case(&agent.id))
            {
                return Err(format!("agent id '{}' already exists", agent.id));
            }
            if agent.label.trim().is_empty() {
                return Err("agent label must not be empty".to_string());
            }
            ensure_seed_consistent(&agent.config_seed)?;
            warn_on_duplicate_label(settings, agent.label.trim(), None);
            let mut new_agent = agent.clone();
            new_agent.label = new_agent.label.trim().to_string();
            ensure_backend_consistent(&mut new_agent)?;
            // #2306 P1 - Add appends at position n; a caller-supplied `order` is
            // ignored on purpose because only a move may choose a position.
            new_agent.order = Some(settings.agents.len() as u32);
            settings.agents.push(new_agent.clone());
            CodingAgentOpOutcome {
                op: "add",
                agent: Some(new_agent),
                removed_id: None,
            }
        }
        CodingAgentOp::Update { id, patch } => {
            let idx = settings
                .agents
                .iter()
                .position(|a| a.id == *id)
                .ok_or_else(|| unknown_agent_id_error(id, &settings.agents))?;
            let mut updated = settings.agents[idx].clone();
            apply_patch(&mut updated, patch)?;
            // #2306 P1 - Update/relabel keeps the current position; `AgentPatch` has
            // no order field, so a patch can never move a tool.
            updated.order = Some(idx as u32);
            warn_on_duplicate_label(settings, updated.label.trim(), Some(idx));
            settings.agents[idx] = updated.clone();
            CodingAgentOpOutcome {
                op: "update",
                agent: Some(updated),
                removed_id: None,
            }
        }
        CodingAgentOp::Remove { id } => {
            let idx = settings
                .agents
                .iter()
                .position(|a| a.id == *id)
                .ok_or_else(|| unknown_agent_id_error(id, &settings.agents))?;
            settings.agents.remove(idx);
            // Orphan `profilesByAgent[id]` / label entries are intentionally left
            // in place: `repair_coding_agent_profiles_config` never prunes by
            // agent id, and the GUI leaves them too. A same-id re-add resurrects
            // the old cells (documented, accepted).
            CodingAgentOpOutcome {
                op: "remove",
                agent: None,
                removed_id: Some(id.clone()),
            }
        }
    };

    // #2306 P1 - keep vector and ordinals aligned after every op: Add appended at
    // n, Remove compacted and Update kept its slot. Renumbering in current vector
    // order (never by stored ordinals) is the effective-view transition.
    normalize_agent_order(&mut settings.agents);

    // Authoritative shared suite: command bans, env rows, instructions filename,
    // config-seed dest, plus profile repair (A-cell bookkeeping for a new agent).
    // This is exactly what every existing writer enforces.
    validate_and_repair_settings(settings)?;
    outcome_from_written(outcome, settings)
}

/// Select the final row by its stable ID after validation or persistence.
/// Update intentionally retains the historical first-exact-ID selection.
pub fn outcome_from_written(
    mut outcome: CodingAgentOpOutcome,
    written: &AppSettings,
) -> Result<CodingAgentOpOutcome, String> {
    if matches!(outcome.op, "add" | "update") {
        let id = outcome
            .agent
            .as_ref()
            .ok_or_else(|| "coding-agent outcome is missing its required row".to_string())?
            .id
            .clone();
        outcome.agent = Some(
            written
                .agents
                .iter()
                .find(|agent| agent.id == id)
                .ok_or_else(|| format!("written settings are missing agent '{id}'"))?
                .clone(),
        );
    }
    Ok(outcome)
}

/// #2306 P1 - pure "swap with the adjacent tool" transition over the effective
/// order.
///
/// Effective order is derived on a temporary candidate first, so a rejected
/// request (or a caller that supplied an unnormalized vector) can never reorder
/// or renumber the live settings. `id` and `neighbor_id` must each occur exactly
/// once by exact, case-sensitive id, and `neighbor_id` must be the immediate
/// `direction` neighbor of `id` in that effective order: a boundary move, a
/// stale/nonadjacent neighbor, an unknown id and a duplicated id all fail with
/// `Err` and leave `settings` untouched. On success the live vector is replaced
/// by the swapped, renumbered candidate and the authoritative ordered id list is
/// returned.
pub fn move_registered_agent(
    settings: &mut AppSettings,
    id: &str,
    neighbor_id: &str,
    direction: AgentMoveDirection,
) -> Result<Vec<String>, String> {
    let mut candidate = settings.agents.clone();
    finalize_agent_order(&mut candidate);

    let from = unique_agent_index(&candidate, id)?;
    let neighbor = unique_agent_index(&candidate, neighbor_id)?;
    let target = match direction {
        AgentMoveDirection::Up => from.checked_sub(1),
        AgentMoveDirection::Down => from.checked_add(1).filter(|index| *index < candidate.len()),
    };
    match target {
        Some(index) if index == neighbor => {}
        _ => {
            let boundary = match direction {
                AgentMoveDirection::Up => from == 0,
                AgentMoveDirection::Down => from + 1 >= candidate.len(),
            };
            return Err(if boundary {
                let edge = match direction {
                    AgentMoveDirection::Up => "top",
                    AgentMoveDirection::Down => "bottom",
                };
                format!("agent '{id}' is already at the {edge} of the list")
            } else {
                format!(
                    "agent '{neighbor_id}' is not adjacent to '{id}' in the requested direction"
                )
            });
        }
    }

    candidate.swap(from, neighbor);
    normalize_agent_order(&mut candidate);
    settings.agents = candidate;
    Ok(settings
        .agents
        .iter()
        .map(|agent| agent.id.clone())
        .collect())
}

/// #2542 - pure "move one tool to `target_index`" transition over the
/// effective order, in one step.
///
/// Like `move_registered_agent`, all validation runs on a finalized candidate,
/// so a rejected request can never reorder or renumber the live settings. The
/// candidate's ordered ids must equal `expected_ids` element-wise (a stale view
/// is rejected), `id` must occur exactly once, `target_index` must be in range
/// and must differ from the current index (a no-op is rejected so nothing is
/// written or emitted). The move is remove-then-insert-at-index; on success the
/// live vector is replaced by the renumbered candidate and the authoritative
/// ordered id list is returned.
pub fn reorder_registered_agent(
    settings: &mut AppSettings,
    id: &str,
    expected_ids: &[String],
    target_index: usize,
) -> Result<Vec<String>, String> {
    let mut candidate = settings.agents.clone();
    finalize_agent_order(&mut candidate);

    let current: Vec<&str> = candidate.iter().map(|agent| agent.id.as_str()).collect();
    if current.len() != expected_ids.len()
        || current
            .iter()
            .zip(expected_ids)
            .any(|(actual, expected)| *actual != expected.as_str())
    {
        return Err(
            "the agent order changed since it was read; refresh and retry the reorder".to_string(),
        );
    }

    let from = unique_agent_index(&candidate, id)?;
    if target_index >= candidate.len() {
        return Err(format!(
            "target index {target_index} is out of range for {} agents",
            candidate.len()
        ));
    }
    if target_index == from {
        return Err(format!(
            "agent '{id}' is already at index {target_index}; nothing to reorder"
        ));
    }

    let agent = candidate.remove(from);
    candidate.insert(target_index, agent);
    normalize_agent_order(&mut candidate);
    settings.agents = candidate;
    Ok(settings
        .agents
        .iter()
        .map(|agent| agent.id.clone())
        .collect())
}

/// #2306 P1 - the single index of `id` in `agents`, rejecting an unknown or
/// duplicated id. Comparison is exact: ids differing only by case are distinct
/// records here, matching the move contract.
fn unique_agent_index(agents: &[AgentConfig], id: &str) -> Result<usize, String> {
    let mut found: Option<usize> = None;
    for (index, agent) in agents.iter().enumerate() {
        if agent.id == id {
            if found.is_some() {
                return Err(format!(
                    "agent id '{id}' appears more than once; reordering requires unique ids"
                ));
            }
            found = Some(index);
        }
    }
    found.ok_or_else(|| unknown_agent_id_error(id, agents))
}

fn apply_patch(agent: &mut AgentConfig, patch: &AgentPatch) -> Result<(), String> {
    if let Some(label) = &patch.label {
        if label.trim().is_empty() {
            return Err("agent label must not be empty".to_string());
        }
        agent.label = label.trim().to_string();
    }
    if let Some(command) = &patch.command {
        agent.command = command.clone();
    }
    if let Some(color) = &patch.color {
        agent.color = color.clone();
    }
    if let Some(envs) = &patch.envs {
        agent.envs = envs.clone();
    }
    if let Some(isolated) = patch.isolated_home {
        agent.isolated_home = isolated;
    }
    if patch.clear_instructions_filename {
        agent.instructions_filename = None;
    } else if let Some(name) = &patch.instructions_filename {
        agent.instructions_filename = Some(name.clone());
    }
    if patch.clear_config_seed {
        agent.config_seed = None;
    } else if patch.config_seed_dest.is_some() || patch.config_seed_enabled.is_some() {
        let mut seed = agent.config_seed.clone().unwrap_or(ConfigSeedConfig {
            enabled: true,
            dest: String::new(),
        });
        if let Some(dest) = &patch.config_seed_dest {
            seed.dest = dest.clone();
        }
        if let Some(enabled) = patch.config_seed_enabled {
            seed.enabled = enabled;
        }
        ensure_seed_consistent(&Some(seed.clone()))?;
        agent.config_seed = Some(seed);
    }
    if let Some(kind) = patch.backend_kind {
        agent.backend.kind = kind;
    }
    if patch.clear_container_image {
        agent.backend.image = None;
    } else if let Some(image) = &patch.container_image {
        agent.backend.image = Some(normalize_container_image_input(image, "container image")?);
    }
    ensure_backend_consistent(agent)?;
    Ok(())
}

fn ensure_backend_consistent(agent: &mut AgentConfig) -> Result<(), String> {
    if let Some(image) = agent.backend.image.take() {
        let trimmed = image.trim();
        if trimmed.is_empty() {
            agent.backend.image = None;
        } else {
            agent.backend.image = Some(normalize_container_image_input(
                trimmed,
                &format!("Agent \"{}\" container image", agent.label),
            )?);
        }
    }
    if agent.backend.kind == SessionBackendKind::LocalProcess {
        agent.backend.image = None;
    }
    Ok(())
}

/// Reject an enabled seed with no destination (the GUI picker never produces
/// this; enabling requires a dest). An inactive seed (disabled, any dest) is fine.
fn ensure_seed_consistent(seed: &Option<ConfigSeedConfig>) -> Result<(), String> {
    if let Some(s) = seed {
        if s.enabled && s.dest.trim().is_empty() {
            return Err(
                "config seed is enabled but has no destination; pass --config-seed-dest"
                    .to_string(),
            );
        }
    }
    Ok(())
}

fn warn_on_duplicate_label(settings: &AppSettings, label: &str, skip_idx: Option<usize>) {
    let dup = settings
        .agents
        .iter()
        .enumerate()
        .any(|(i, a)| Some(i) != skip_idx && a.label.trim().eq_ignore_ascii_case(label));
    if dup {
        log::warn!(
            "[coding-agent] duplicate label '{label}'; launch-by-label resolution may be ambiguous"
        );
    }
}
