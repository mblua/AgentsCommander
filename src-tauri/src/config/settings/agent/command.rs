use super::super::{config_seed_dest_os_token_stem, validate_config_seed_dest, AppSettings};
use super::{
    agent_order_permutation, finalize_agent_order, is_valid_profile_letter, validate_env_map,
    validate_env_rows, AgentConfig,
};
use crate::config::local_overlay::PairedLeaf;
use crate::session::profile::CodingAgentKind;
use serde::Deserialize;
use serde_json::{Map, Value};
use std::collections::{BTreeMap, HashSet};

/// Configuration selection uses the frozen cross-runtime set, never Rust trim.
pub(crate) fn configuration_command_has_content(value: &str) -> bool {
    value.chars().any(|c| {
        !matches!(c,
            '\u{0009}'..='\u{000D}' | '\u{0020}' | '\u{00A0}' | '\u{1680}' |
            '\u{2000}'..='\u{200A}' | '\u{2028}' | '\u{2029}' | '\u{202F}' |
            '\u{205F}' | '\u{3000}' | '\u{FEFF}'
        )
    })
}

pub(crate) fn select_configuration_command<'a>(
    new: Option<&'a str>,
    old: Option<&'a str>,
) -> Option<&'a str> {
    new.filter(|value| configuration_command_has_content(value))
        .or_else(|| old.filter(|value| configuration_command_has_content(value)))
        .or(new)
        .or(old)
}

/// Absent is optional; a present null or any other non-string is an error.
pub(crate) fn deserialize_present_command<'de, D>(
    deserializer: D,
) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    String::deserialize(deserializer).map(Some)
}

pub(crate) fn raw_configuration_command<'a>(
    row: &'a Map<String, Value>,
    key: &str,
) -> Result<Option<&'a str>, String> {
    for name in [key, "command"] {
        if row.get(name).is_some_and(|value| !value.is_string()) {
            return Err(format!("{name} must be a string"));
        }
    }
    Ok(select_configuration_command(
        row.get(key).and_then(Value::as_str),
        row.get("command").and_then(Value::as_str),
    ))
}

pub(crate) fn overlay_configuration_command(
    row: &Map<String, Value>,
    key: &str,
) -> Result<Option<String>, String> {
    raw_configuration_command(row, key).map(|value| value.map(str::to_owned))
}

pub(crate) const CONFIGURATION_COMMAND_PAIRS: &[PairedLeaf] = &[
    PairedLeaf {
        path: &["agents", "*"],
        canonical: "baseCommand",
        legacy: "command",
        select: overlay_configuration_command,
    },
    PairedLeaf {
        path: &["codingAgentProfiles", "profilesByAgent", "*", "*"],
        canonical: "additionalArguments",
        legacy: "command",
        select: overlay_configuration_command,
    },
];

/// An edit owns the canonical leaf only. The locked physical source owns the
/// legacy leaf, including its absence; a caller never supplies that authority.
pub(crate) fn retain_command_leaf(
    row: &mut Map<String, Value>,
    before: Option<&Map<String, Value>>,
    key: &str,
) -> Result<(), String> {
    if before.is_some_and(|before| {
        row.get(key) == before.get(key) && row.get("command") == before.get("command")
    }) {
        return Ok(());
    }
    raw_configuration_command(row, key)?;
    let edit = row
        .get(key)
        .or_else(|| row.get("command"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    if let Some(before) = before {
        raw_configuration_command(before, key)?;
    }
    if let Some(edit) = edit {
        row.insert(key.to_owned(), Value::String(edit));
    }
    match before.and_then(|row| row.get("command")) {
        Some(value) => {
            row.insert("command".to_owned(), value.clone());
        }
        None => {
            row.remove("command");
        }
    }
    Ok(())
}

pub(crate) fn retain_configuration_commands(
    after: &mut Value,
    before: &Value,
    identity: &str,
    catalog: bool,
    agents: bool,
    profiles: bool,
) -> Result<(), String> {
    retain_configuration_commands_with_startup(
        after, before, identity, catalog, agents, profiles, None,
    )
}

pub(crate) fn retain_configuration_commands_with_startup(
    after: &mut Value,
    before: &Value,
    identity: &str,
    catalog: bool,
    agents: bool,
    profiles: bool,
    startup: Option<&StartupAgentRows>,
) -> Result<(), String> {
    if agents {
        if !catalog {
            retain_duplicate_agent_rows(after, before, startup)?;
        }
        if let Some(rows) = after.get_mut("agents").and_then(Value::as_array_mut) {
            let mut counts = BTreeMap::new();
            for row in before
                .get("agents")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                if let Some(id) = row.get(identity).and_then(Value::as_str) {
                    *counts.entry(id).or_insert(0usize) += 1;
                }
            }
            for row in rows {
                if !catalog
                    && row
                        .get(identity)
                        .and_then(Value::as_str)
                        .is_some_and(|id| counts.get(id).copied().unwrap_or(0) > 1)
                {
                    continue;
                }
                let previous = row.get(identity).and_then(Value::as_str).and_then(|id| {
                    before
                        .get("agents")
                        .and_then(Value::as_array)
                        .and_then(|rows| {
                            rows.iter()
                                .find(|row| row.get(identity).and_then(Value::as_str) == Some(id))
                        })
                });
                if let Some(row) = row.as_object_mut() {
                    retain_command_leaf(row, previous.and_then(Value::as_object), "baseCommand")?;
                }
            }
        }
    }
    if profiles {
        let pointer = if catalog {
            "/codingAgentProfiles"
        } else {
            "/codingAgentProfiles/profilesByAgent"
        };
        if let Some(groups) = after.pointer_mut(pointer).and_then(Value::as_object_mut) {
            for (id, cells) in groups {
                if let Some(cells) = cells.as_object_mut() {
                    for (letter, cell) in cells {
                        let previous = before
                            .pointer(pointer)
                            .and_then(|groups| groups.get(id))
                            .and_then(|cells| cells.get(letter))
                            .and_then(Value::as_object);
                        if let Some(cell) = cell.as_object_mut() {
                            retain_command_leaf(cell, previous, "additionalArguments")?;
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

/// Private to one startup operation; never accepted from a settings caller.
pub(crate) struct StartupAgentRows {
    pub(crate) raw: Value,
    pub(crate) sidecar_owner: bool,
    pub(crate) sidecar_absent: bool,
    pub(crate) permutation: Vec<usize>,
    pub(crate) expected: Vec<AgentConfig>,
}

impl StartupAgentRows {
    pub(crate) fn capture(raw: Value, sidecar_owner: bool, sidecar_absent: bool) -> Option<Self> {
        let mut expected: Vec<AgentConfig> = serde_json::from_value(raw.clone()).ok()?;
        let mut ids = HashSet::new();
        if !expected.iter().any(|agent| !ids.insert(agent.id.clone())) {
            return None;
        }
        let permutation = agent_order_permutation(&expected);
        finalize_agent_order(&mut expected);
        Some(Self {
            raw,
            sidecar_owner,
            sidecar_absent,
            permutation,
            expected,
        })
    }

    pub(crate) fn validate_source(&self, raw: &Value, sidecar_owner: bool) -> Result<(), String> {
        if self.raw == *raw
            && (self.sidecar_owner == sidecar_owner
                || (!self.sidecar_owner && self.sidecar_absent && sidecar_owner))
        {
            Ok(())
        } else {
            let mut ids = HashSet::new();
            let id = self
                .expected
                .iter()
                .find(|agent| !ids.insert(agent.id.as_str()))
                .map(|agent| agent.id.as_str())
                .unwrap_or("unknown");
            Err(format!(
                "duplicate agent ID '{id}': unprovable row correspondence (stale startup source)"
            ))
        }
    }
}

// Includes all AgentConfig wire fields and its sole alias, even omitted defaults.
pub(crate) fn known_agent_field(key: &str) -> bool {
    matches!(
        key,
        "id" | "id_hash"
            | "id_hash_debug"
            | "label"
            | "command"
            | "baseCommand"
            | "preflight"
            | "preflightTimeoutSeconds"
            | "color"
            | "envs"
            | "isolatedHome"
            | "isolateCodexHome"
            | "instructionsFilename"
            | "configSeed"
            | "contextRegex"
            | "blockingMenus"
            | "order"
            | "backend"
    )
}

pub(crate) fn comparable_agent(agent: &AgentConfig) -> Result<Value, String> {
    let mut value = serde_json::to_value(agent).map_err(|error| error.to_string())?;
    if let Some(object) = value.as_object_mut() {
        object.remove("order");
        object.remove("id_hash");
        object.remove("id_hash_debug");
    }
    Ok(value)
}

/// Prove the ordered duplicate subsequence before transferring any raw payload.
pub(crate) fn retain_duplicate_agent_rows(
    after: &mut Value,
    before: &Value,
    startup: Option<&StartupAgentRows>,
) -> Result<(), String> {
    let Some(raw) = before.get("agents").and_then(Value::as_array) else {
        return Ok(());
    };
    let Some(rows) = after.get_mut("agents").and_then(Value::as_array_mut) else {
        return Ok(());
    };
    let mut groups: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    for (index, row) in raw.iter().enumerate() {
        if let Some(id) = row.get("id").and_then(Value::as_str) {
            groups.entry(id).or_default().push(index);
        }
    }
    // Existing unique/new-row saves need no typed duplicate preparation.
    let mut candidate_counts = BTreeMap::new();
    for row in rows.iter() {
        if let Some(id) = row.get("id").and_then(Value::as_str) {
            *candidate_counts.entry(id.to_owned()).or_insert(0usize) += 1;
        }
    }
    for (id, indices) in &groups {
        if indices.len() == 1 && candidate_counts.get(*id).copied().unwrap_or(0) > 1 {
            return Err(format!(
                "duplicate agent ID '{id}': unprovable row correspondence"
            ));
        }
    }
    if !groups.values().any(|indices| indices.len() > 1) {
        return Ok(());
    }
    let decoded: Vec<AgentConfig> =
        serde_json::from_value(Value::Array(raw.clone())).map_err(|error| error.to_string())?;
    let permutation = startup
        .map(|context| context.permutation.clone())
        .unwrap_or_else(|| agent_order_permutation(&decoded));
    let expected = startup
        .map(|context| context.expected.clone())
        .unwrap_or_else(|| {
            let mut expected = decoded;
            finalize_agent_order(&mut expected);
            expected
        });
    for (id, indices) in groups.iter().filter(|(_, indices)| indices.len() > 1) {
        let rejected = || format!("duplicate agent ID '{id}': unprovable row correspondence");
        let candidates: Vec<usize> = rows
            .iter()
            .enumerate()
            .filter(|(_, row)| row.get("id").and_then(Value::as_str) == Some(*id))
            .map(|(index, _)| index)
            .collect();
        if candidates.is_empty() {
            continue;
        }
        let baseline: Vec<(usize, &AgentConfig)> = permutation
            .iter()
            .copied()
            .zip(expected.iter())
            .filter(|(_, agent)| agent.id == *id)
            .collect();
        if candidates.len() != indices.len() || baseline.len() != indices.len() {
            return Err(rejected());
        }
        for (candidate_index, (raw_index, expected)) in candidates.into_iter().zip(baseline) {
            let candidate: AgentConfig =
                serde_json::from_value(rows[candidate_index].clone()).map_err(|_| rejected())?;
            if comparable_agent(&candidate)? != comparable_agent(expected)? {
                return Err(rejected());
            }
            let original = raw[raw_index].as_object().ok_or_else(rejected)?;
            let provided = rows[candidate_index].as_object().ok_or_else(rejected)?;
            let mut retained = original.clone();
            for (key, value) in provided {
                if known_agent_field(key) {
                    continue;
                }
                if original.get(key) != Some(value) {
                    return Err(rejected());
                }
            }
            // Rebuild recognized fields through the existing serializer, preserving
            // unknown metadata from this exact whole raw row only.
            retained.retain(|key, _| !known_agent_field(key));
            retained.extend(
                serde_json::to_value(&candidate)
                    .map_err(|error| error.to_string())?
                    .as_object()
                    .ok_or_else(rejected)?
                    .clone(),
            );
            retained.insert("baseCommand".to_owned(), Value::String(candidate.command));
            match original.get("command") {
                Some(value) => {
                    retained.insert("command".to_owned(), value.clone());
                }
                None => {
                    retained.remove("command");
                }
            }
            rows[candidate_index] = Value::Object(retained);
        }
    }
    Ok(())
}

/// The tree's only executable-stem rule, and the byte-for-byte twin of
/// `executableTokenBasename` in `src/shared/profile-utils.ts`. It deliberately does
/// not use `std::path::Path`, which ignores `\` off Windows (#2430).
pub(crate) fn command_token_basename(token: &str) -> String {
    let normalized = token.replace('\\', "/");
    let leaf = match normalized.rsplit('/').next() {
        Some(segment) if !segment.is_empty() => segment,
        _ => normalized.as_str(),
    };
    let stem = match leaf.rsplit_once('.') {
        Some((stem, ext)) if !ext.is_empty() => stem,
        _ => leaf,
    };
    stem.to_lowercase()
}

pub(crate) fn token_has_unclosed_quote(token: &str, quote: char) -> bool {
    token.chars().filter(|c| *c == quote).count() % 2 == 1
}

pub(crate) fn advance_past_config_value(tokens: &[&str], start: usize) -> usize {
    if start >= tokens.len() {
        return start;
    }

    let mut idx = start;
    let mut in_single = false;
    let mut in_double = false;

    while idx < tokens.len() {
        let token = tokens[idx];
        if token_has_unclosed_quote(token, '\'') {
            in_single = !in_single;
        }
        if token_has_unclosed_quote(token, '"') {
            in_double = !in_double;
        }
        idx += 1;
        if !in_single && !in_double {
            break;
        }
    }

    idx
}

pub(crate) fn find_provider_token(tokens: &[&str], provider: &str) -> Option<usize> {
    tokens
        .iter()
        .position(|token| command_token_basename(token) == provider)
}

pub(crate) fn antigravity_has_manual_resume(tokens: &[&str], antigravity_idx: usize) -> bool {
    tokens[antigravity_idx + 1..].iter().any(|t| {
        let lower = t.to_lowercase();
        lower == "--continue" || lower == "-c"
    })
}

pub(crate) fn codex_has_manual_resume(tokens: &[&str], codex_idx: usize) -> bool {
    let mut idx = codex_idx + 1;
    while idx < tokens.len() {
        let token = tokens[idx];
        if token.eq_ignore_ascii_case("-c") || token.eq_ignore_ascii_case("--config") {
            idx = advance_past_config_value(tokens, idx + 1);
            continue;
        }
        if token.eq_ignore_ascii_case("resume") || token.eq_ignore_ascii_case("--last") {
            return true;
        }
        idx += 1;
    }
    false
}

pub fn validate_agent_commands(settings: &AppSettings) -> Result<(), String> {
    for agent in &settings.agents {
        validate_agent_command_text(&format!("Agent \"{}\"", agent.label), &agent.command)?;

        validate_env_rows(
            &agent.envs,
            &format!("Agent \"{}\" env settings", agent.label),
        )?;

        // #529 (G8): reject a bad instructions filename here so the user gets a
        // clear IPC error at save, not a silent fallback at launch. Empty/absent
        // is allowed (means "use the command-derived default").
        if let Some(f) = agent.instructions_filename.as_deref() {
            let f = f.trim();
            if !f.is_empty() && !crate::config::agent_command::is_safe_instructions_filename(f) {
                return Err(format!(
                    "Agent \"{}\" instructions filename is invalid: must be a bare .md filename \
                        with no path separators",
                    agent.label
                ));
            }
        }

        // #598: reject a bad config-seed dest at save so the user gets a clear
        // IPC error here, not a silent fail-soft skip at launch. Inactive
        // (disabled or empty dest) is allowed (means "no seeding").
        if let Some(seed) = agent.config_seed.as_ref() {
            if seed.is_active() {
                validate_config_seed_dest(&seed.dest).map_err(|e| {
                    format!("Agent \"{}\" config seed is invalid: {}", agent.label, e)
                })?;
                // #2162: a dest ending in an OS token names the same folder as
                // the OS variant of the shorter dest. Warn, never reject.
                if let Some(stem) = config_seed_dest_os_token_stem(&seed.dest) {
                    if stem.is_empty() {
                        log::warn!(
                            "[config-seed] agent \"{}\" dest '{}' is only an OS token suffix",
                            agent.label,
                            seed.dest.trim()
                        );
                    } else {
                        log::warn!(
                            "[config-seed] agent \"{}\" dest '{}' ends with an OS token; it names \
                                the same template folder as the OS variant of dest '{}'",
                            agent.label,
                            seed.dest.trim(),
                            stem
                        );
                    }
                }
            }
        }
    }

    for (agent_id, cells) in &settings.coding_agent_profiles.profiles_by_agent {
        for (letter, cell) in cells {
            if !is_valid_profile_letter(letter) {
                return Err(format!(
                    "Coding agent profile cell '{}:{}' uses an invalid profile letter",
                    agent_id, letter
                ));
            }
            validate_env_map(
                &cell.env,
                &format!(
                    "Coding agent profile '{}:{}' env settings",
                    agent_id, letter
                ),
            )?;
            if cell.enabled && !cell.command.trim().is_empty() {
                // #597 - the cell holds params appended to the agent base command,
                // so validate the COMPOSED effective command. Otherwise a banned
                // provider flag (Claude --continue/-c or Codex manual resume /
                // Antigravity --continue/-c) placed in the cell params escapes the
                // check. Pi session selectors are intentionally allowed and remain
                // user-authoritative. The provider token lives in the base, not the
                // cell. Falls back to the cell text when the cell references an
                // agent id that has no configured agent.
                let base = settings
                    .agents
                    .iter()
                    .find(|a| a.id == *agent_id)
                    .map(|a| a.command.as_str())
                    .unwrap_or("");
                let effective =
                    crate::config::agent_command::compose_effective_command(base, &cell.command);
                validate_agent_command_text(
                    &format!("Coding agent profile '{}:{}'", agent_id, letter),
                    &effective,
                )?;
            }
        }
    }

    for (agent_name, letter) in &settings.coding_agent_profiles.default_profile_by_agent {
        if !is_valid_profile_letter(letter) {
            return Err(format!(
                "Coding agent default profile for '{}' must be A through Z",
                agent_name
            ));
        }
    }

    Ok(())
}

pub(crate) fn validate_agent_command_text(context: &str, command: &str) -> Result<(), String> {
    let normalized = crate::config::agent_command::normalize_legacy_agent_command(command)
        .map_err(|e| format!("{context}: invalid command: {e}. command={command:?}"))?;

    // Pi selectors are user-authored configuration and intentionally outrank AC
    // automation. Canonical Pi identity must win before the independent legacy
    // provider-token scans below inspect model or provider option values.
    // #1873 - the same holds for a directly configured Muse: any configured
    // argument (resume, --last, UUID, prompt, --workspace ...) is user-authored,
    // suppresses AC injection, and is neither validated nor rewritten here. A
    // wrapper (`env muse ...`) is not Muse and keeps the legacy scans below.
    if matches!(
        CodingAgentKind::detect(&normalized.shell, &normalized.shell_args),
        Some(CodingAgentKind::Pi | CodingAgentKind::Muse)
    ) {
        return Ok(());
    }

    let mut token_strings = Vec::with_capacity(normalized.shell_args.len() + 1);
    token_strings.push(normalized.shell);
    token_strings.extend(normalized.shell_args);
    let tokens: Vec<&str> = token_strings.iter().map(String::as_str).collect();

    if let Some(claude_idx) = find_provider_token(&tokens, "claude") {
        if tokens[claude_idx + 1..].iter().any(|token| {
            token.eq_ignore_ascii_case("--continue") || token.eq_ignore_ascii_case("-c")
        }) {
            return Err(format!(
                "{context}: Claude commands must not include --continue or -c"
            ));
        }
    }

    if let Some(codex_idx) = find_provider_token(&tokens, "codex") {
        if codex_has_manual_resume(&tokens, codex_idx) {
            return Err(format!(
                "{context}: Codex commands must not include resume or --last; AgentsCommander \
                    injects codex resume --last automatically"
            ));
        }
    }

    if let Some(antigravity_idx) =
        find_provider_token(&tokens, "agy").or_else(|| find_provider_token(&tokens, "antigravity"))
    {
        if antigravity_has_manual_resume(&tokens, antigravity_idx) {
            return Err(format!(
                "{context}: Antigravity commands must not include --continue or -c; \
                    AgentsCommander injects agy --continue automatically"
            ));
        }
    }

    Ok(())
}
