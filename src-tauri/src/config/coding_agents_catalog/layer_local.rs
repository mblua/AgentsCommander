use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::config::settings::{CodingAgentEnv, CodingAgentEnvSource, ConfigSeedConfig};

use super::{
    active_builtin_agent_support, catalog_diagnostic, definition_problem_reason,
    expect_json_object, expect_json_string, is_supported_builtin, parse_catalog_schema2,
    parse_strict_json, reject_unknown_json_fields, validate_catalog_key, validate_definition,
    validate_install_commands, CatalogDiagnostic, CodingAgentDefinition, IdleBurstConfig,
    InstallCommands, LayerDescription, LocalFieldPatch, CATALOG_SCHEMA_VERSION,
    REPORT_CODE_INVALID_DEFINITION,
};

/// #2736 - a local `installCommands` patch. For the three per-OS keys,
/// `None` = absent (inherit), `Some(None)` = explicit `null` (clear).
#[derive(Debug, Clone, Default)]
pub(crate) struct InstallCommandsPatch {
    pub(crate) default: Option<String>,
    #[expect(
        clippy::option_option,
        reason = "Absent inherits/omits; null clears; value replaces; keep patch/wire historic JSON"
    )]
    pub(crate) windows: Option<Option<String>>,
    #[expect(
        clippy::option_option,
        reason = "Absent inherits/omits; null clears; value replaces; keep patch/wire historic JSON"
    )]
    pub(crate) macos: Option<Option<String>>,
    #[expect(
        clippy::option_option,
        reason = "Absent inherits/omits; null clears; value replaces; keep patch/wire historic JSON"
    )]
    pub(crate) linux: Option<Option<String>>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct ConfigSeedPatch {
    pub(crate) enabled: Option<bool>,
    pub(crate) dest: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct LocalRow {
    pub(crate) key: String,
    pub(crate) remove: bool,
    pub(crate) fields: LocalFieldPatch,
}

#[derive(Debug, Clone)]
pub(crate) struct LocalLayer {
    pub(crate) rows: Vec<LocalRow>,
    pub(crate) order: Option<Vec<String>>,
    pub(crate) complete_definitions: bool,
}

pub(crate) const LOCAL_ROOT_FIELDS: &[&str] = &[
    "schemaVersion",
    "agents",
    "order",
    "profileBindingVersion",
    "profilesByBaseHash",
    "profileLabelsByBaseHash",
    "profileSlots",
    "legacyProfileOrphans",
    "codingAgentProfiles",
    "profileLabels",
];
pub(crate) const LOCAL_ROW_FIELDS: &[&str] = &[
    "key",
    "label",
    "description",
    "color",
    "command",
    "baseCommand",
    "instructionsFilename",
    "envs",
    "isolatedHome",
    "configSeed",
    "removable",
    "updateCommands",
    "autoUpdate",
    "idleBurst",
    "installCommands",
];
pub(crate) const LOCAL_ENV_FIELDS: &[&str] = &["key", "value", "source", "enabled"];
pub(crate) const LOCAL_CONFIG_SEED_FIELDS: &[&str] = &["enabled", "dest"];
pub(crate) const LOCAL_IDLE_BURST_FIELDS: &[&str] = &["maxBytes", "maxSecs", "priorSilenceSecs"];
pub(crate) const LOCAL_INSTALL_COMMANDS_FIELDS: &[&str] = &["default", "windows", "macos", "linux"];

pub(crate) fn validate_update_command_string(
    command: &str,
    context: &str,
    index: usize,
) -> Result<(), String> {
    if command.trim().is_empty() {
        return Err(format!("{context}: updateCommands item {index} is blank"));
    }
    if command
        .chars()
        .any(|c| c.is_control() || c == '\u{2028}' || c == '\u{2029}')
    {
        return Err(format!(
            "{context}: updateCommands item {index} contains a Unicode control character, U+2028 \
                or U+2029"
        ));
    }
    Ok(())
}

pub(crate) fn parse_update_commands_value(
    value: &serde_json::Value,
    context: &str,
) -> Result<Vec<String>, String> {
    let serde_json::Value::Array(items) = value else {
        return Err(format!(
            "{context}: updateCommands must be an array of complete command strings"
        ));
    };
    let mut commands = Vec::with_capacity(items.len());
    for (index, item) in items.iter().enumerate() {
        let serde_json::Value::String(command) = item else {
            return Err(format!(
                "{context}: updateCommands item {index} must be a string"
            ));
        };
        validate_update_command_string(command, context, index)?;
        commands.push(command.clone());
    }
    Ok(commands)
}

pub(crate) fn parse_local_envs(
    value: &serde_json::Value,
    context: &str,
) -> Result<Vec<CodingAgentEnv>, String> {
    let serde_json::Value::Array(items) = value else {
        return Err(format!("{context}: envs must be an array"));
    };
    let mut envs = Vec::with_capacity(items.len());
    for (index, item) in items.iter().enumerate() {
        let row_context = format!("{context} envs[{index}]");
        let object = expect_json_object(item, &row_context)?;
        reject_unknown_json_fields(object, LOCAL_ENV_FIELDS, &row_context)?;
        let key = expect_json_string(object, "key", &row_context)?.to_string();
        let value = expect_json_string(object, "value", &row_context)?.to_string();
        let source = match object.get("source") {
            None => CodingAgentEnvSource::User,
            Some(serde_json::Value::String(source)) => match source.as_str() {
                "user" => CodingAgentEnvSource::User,
                "system" | "agentsCommander" => CodingAgentEnvSource::System,
                _ => {
                    return Err(format!(
                        "{row_context}: 'source' is not a recognized env source value"
                    ))
                }
            },
            Some(_) => return Err(format!("{row_context}: 'source' must be a string")),
        };
        let enabled = match object.get("enabled") {
            None => true,
            Some(serde_json::Value::Bool(enabled)) => *enabled,
            Some(_) => return Err(format!("{row_context}: 'enabled' must be a boolean")),
        };
        envs.push(CodingAgentEnv {
            key,
            value,
            source,
            enabled,
        });
    }
    Ok(envs)
}

pub(crate) fn parse_config_seed_patch(
    value: &serde_json::Value,
    context: &str,
) -> Result<ConfigSeedPatch, String> {
    let seed_context = format!("{context} configSeed");
    let object = expect_json_object(value, &seed_context)?;
    reject_unknown_json_fields(object, LOCAL_CONFIG_SEED_FIELDS, &seed_context)?;
    let mut patch = ConfigSeedPatch::default();
    if let Some(enabled) = object.get("enabled") {
        let serde_json::Value::Bool(enabled) = enabled else {
            return Err(format!("{seed_context}: 'enabled' must be a boolean"));
        };
        patch.enabled = Some(*enabled);
    }
    if let Some(dest) = object.get("dest") {
        let serde_json::Value::String(dest) = dest else {
            return Err(format!("{seed_context}: 'dest' must be a string"));
        };
        patch.dest = Some(dest.clone());
    }
    Ok(patch)
}

pub(crate) fn parse_idle_burst_patch(
    value: &serde_json::Value,
    context: &str,
) -> Result<IdleBurstConfig, String> {
    let burst_context = format!("{context} idleBurst");
    let object = expect_json_object(value, &burst_context)?;
    reject_unknown_json_fields(object, LOCAL_IDLE_BURST_FIELDS, &burst_context)?;
    let mut config = IdleBurstConfig::default();
    if let Some(max_bytes) = object.get("maxBytes") {
        let Some(max_bytes) = max_bytes.as_u64() else {
            return Err(format!(
                "{burst_context}: 'maxBytes' must be a non-negative integer"
            ));
        };
        config.max_bytes = Some(max_bytes);
    }
    if let Some(max_secs) = object.get("maxSecs") {
        config.max_secs = Some(parse_idle_burst_secs(max_secs, "maxSecs", &burst_context)?);
    }
    if let Some(prior_silence_secs) = object.get("priorSilenceSecs") {
        config.prior_silence_secs = Some(parse_idle_burst_secs(
            prior_silence_secs,
            "priorSilenceSecs",
            &burst_context,
        )?);
    }
    Ok(config)
}

/// #2736 - the strict local `installCommands` patch: an object whose only
/// members are the four platform keys; `default` must be a string, each
/// per-OS key a string or `null`. Values are checked after composition.
pub(crate) fn parse_install_commands_patch(
    value: &serde_json::Value,
    context: &str,
) -> Result<InstallCommandsPatch, String> {
    let install_context = format!("{context} installCommands");
    let object = expect_json_object(value, &install_context)?;
    reject_unknown_json_fields(object, LOCAL_INSTALL_COMMANDS_FIELDS, &install_context)?;
    let mut patch = InstallCommandsPatch::default();
    if let Some(default) = object.get("default") {
        let serde_json::Value::String(default) = default else {
            return Err(format!("{install_context}: 'default' must be a string"));
        };
        patch.default = Some(default.clone());
    }
    for (name, slot) in [
        ("windows", &mut patch.windows),
        ("macos", &mut patch.macos),
        ("linux", &mut patch.linux),
    ] {
        if let Some(value) = object.get(name) {
            *slot = Some(match value {
                serde_json::Value::Null => None,
                serde_json::Value::String(value) => Some(value.clone()),
                _ => {
                    return Err(format!(
                        "{install_context}: '{name}' must be a string or null"
                    ))
                }
            });
        }
    }
    Ok(patch)
}

/// One `idleBurst` seconds subfield, strictly: a JSON number, finite and >= 0.
pub(crate) fn parse_idle_burst_secs(
    value: &serde_json::Value,
    field: &str,
    context: &str,
) -> Result<f64, String> {
    let parsed = value
        .as_f64()
        .filter(|value| value.is_finite() && *value >= 0.0);
    parsed.ok_or_else(|| format!("{context}: '{field}' must be a finite number >= 0"))
}

pub(crate) fn parse_local_fields(
    object: &serde_json::Map<String, serde_json::Value>,
    context: &str,
) -> Result<LocalFieldPatch, String> {
    reject_unknown_json_fields(object, LOCAL_ROW_FIELDS, context)?;
    let mut fields = LocalFieldPatch::default();
    if let Some(value) = object.get("label") {
        let serde_json::Value::String(value) = value else {
            return Err(format!("{context}: 'label' must be a string"));
        };
        fields.label = Some(value.clone());
    }
    if let Some(value) = object.get("description") {
        let serde_json::Value::String(value) = value else {
            return Err(format!("{context}: 'description' must be a string"));
        };
        fields.description = Some(value.clone());
    }
    if let Some(value) = object.get("color") {
        let serde_json::Value::String(value) = value else {
            return Err(format!("{context}: 'color' must be a string"));
        };
        fields.color = Some(value.clone());
    }
    for name in ["baseCommand", "command"] {
        if object.get(name).is_some_and(|value| !value.is_string()) {
            return Err(format!("{context}: '{name}' must be a string"));
        }
    }
    fields.command = crate::config::settings::select_configuration_command(
        object
            .get("baseCommand")
            .and_then(serde_json::Value::as_str),
        object.get("command").and_then(serde_json::Value::as_str),
    )
    .map(str::to_owned);
    if let Some(value) = object.get("instructionsFilename") {
        fields.instructions_filename = Some(match value {
            serde_json::Value::Null => None,
            serde_json::Value::String(value) => Some(value.clone()),
            _ => {
                return Err(format!(
                    "{context}: 'instructionsFilename' must be a string or null"
                ))
            }
        });
    }
    if let Some(value) = object.get("envs") {
        fields.envs = Some(parse_local_envs(value, context)?);
    }
    if let Some(value) = object.get("isolatedHome") {
        let serde_json::Value::Bool(value) = value else {
            return Err(format!("{context}: 'isolatedHome' must be a boolean"));
        };
        fields.isolated_home = Some(*value);
    }
    if let Some(value) = object.get("configSeed") {
        fields.config_seed = Some(match value {
            serde_json::Value::Null => None,
            serde_json::Value::Object(_) => Some(parse_config_seed_patch(value, context)?),
            _ => return Err(format!("{context}: 'configSeed' must be an object or null")),
        });
    }
    if let Some(value) = object.get("removable") {
        let serde_json::Value::Bool(value) = value else {
            return Err(format!("{context}: 'removable' must be a boolean"));
        };
        fields.removable = Some(*value);
    }
    if let Some(value) = object.get("updateCommands") {
        fields.update_commands = Some(parse_update_commands_value(value, context)?);
    }
    if let Some(value) = object.get("autoUpdate") {
        let serde_json::Value::Bool(value) = value else {
            return Err(format!("{context}: 'autoUpdate' must be a boolean"));
        };
        fields.auto_update = Some(*value);
    }
    if let Some(value) = object.get("idleBurst") {
        fields.idle_burst = Some(match value {
            serde_json::Value::Null => None,
            serde_json::Value::Object(_) => Some(parse_idle_burst_patch(value, context)?),
            _ => return Err(format!("{context}: 'idleBurst' must be an object or null")),
        });
    }
    if let Some(value) = object.get("installCommands") {
        fields.install_commands = Some(match value {
            serde_json::Value::Null => None,
            serde_json::Value::Object(_) => Some(parse_install_commands_patch(value, context)?),
            _ => {
                return Err(format!(
                    "{context}: 'installCommands' must be an object or null"
                ))
            }
        });
    }
    Ok(fields)
}

/// Parse the local layer STRICTLY: unknown fields, duplicate JSON members,
/// duplicate local identities, duplicate order keys and unsupported schemas are
/// all whole-layer errors, so a partially applied layer is impossible.
pub(crate) fn parse_local_layer(
    bytes: &[u8],
    description: LayerDescription,
) -> Result<LocalLayer, String> {
    let layer = description.name();
    let value = parse_strict_json(bytes)
        .map_err(|reason| format!("the {layer} catalog is not valid JSON ({reason})"))?;
    let root = expect_json_object(&value, &format!("the {layer} catalog root"))?;
    if root
        .get("schemaVersion")
        .and_then(serde_json::Value::as_u64)
        == Some(2)
    {
        return parse_schema2_local_layer(bytes);
    }
    crate::config::settings::physical_profile_sets(&value, false)?;
    reject_unknown_json_fields(
        root,
        LOCAL_ROOT_FIELDS,
        &format!("the {layer} catalog root"),
    )?;
    match root.get("schemaVersion") {
        Some(serde_json::Value::Number(version))
            if version.as_u64() == Some(CATALOG_SCHEMA_VERSION as u64) => {}
        Some(version) => {
            return Err(format!(
                "unsupported schemaVersion {version}; only schemaVersion 1 is recognized"
            ))
        }
        None => {
            return Err(format!(
                "the {layer} catalog root must declare schemaVersion 1"
            ))
        }
    }
    let agents = match root.get("agents") {
        Some(serde_json::Value::Array(agents)) => agents,
        Some(_) => {
            return Err(format!(
                "the {layer} catalog 'agents' value must be a JSON array"
            ))
        }
        None => {
            return Err(format!(
                "the {layer} catalog root must declare an 'agents' array"
            ))
        }
    };
    let order = match root.get("order") {
        None => None,
        Some(serde_json::Value::Array(items)) => {
            let mut seen = HashSet::new();
            let mut keys = Vec::with_capacity(items.len());
            for (index, item) in items.iter().enumerate() {
                let serde_json::Value::String(key) = item else {
                    return Err(format!("{layer} order item {index} must be a string"));
                };
                if validate_catalog_key(key).is_err() {
                    return Err(format!(
                        "{layer} order item {index} is not a valid catalog key"
                    ));
                }
                if !seen.insert(key.clone()) {
                    return Err(format!("duplicate {layer} order key '{key}'"));
                }
                keys.push(key.clone());
            }
            Some(keys)
        }
        Some(_) => {
            return Err(format!(
                "the {layer} catalog 'order' value must be a JSON array"
            ))
        }
    };

    let mut seen_keys = HashSet::new();
    let mut rows = Vec::with_capacity(agents.len());
    for (index, raw) in agents.iter().enumerate() {
        let context = format!("{layer} agents[{index}]");
        let object = expect_json_object(raw, &context)?;
        let key = expect_json_string(object, "key", &context)?.to_string();
        validate_catalog_key(&key).map_err(|reason| format!("{context}: {reason}"))?;
        if !seen_keys.insert(key.clone()) {
            return Err(format!("duplicate {layer} coding-agent key '{key}'"));
        }
        let remove = match object.get("remove") {
            None => false,
            Some(serde_json::Value::Bool(true)) => true,
            Some(serde_json::Value::Bool(false)) => {
                return Err(format!(
                    "{context}: remove:false is rejected; omit remove for ordinary patches"
                ))
            }
            Some(_) => return Err(format!("{context}: 'remove' must be true")),
        };
        if remove {
            reject_unknown_json_fields(object, &["key", "remove"], &context)?;
            rows.push(LocalRow {
                key,
                remove: true,
                fields: LocalFieldPatch::default(),
            });
        } else {
            let fields = parse_local_fields(object, &context)?;
            rows.push(LocalRow {
                key,
                remove: false,
                fields,
            });
        }
    }
    Ok(LocalLayer {
        rows,
        order,
        complete_definitions: false,
    })
}

fn parse_schema2_local_layer(bytes: &[u8]) -> Result<LocalLayer, String> {
    let catalog = parse_catalog_schema2(bytes).map_err(|e| e.to_string())?;
    let rows = catalog
        .agents
        .iter()
        .map(|agent| {
            let raw = serde_json::to_value(agent).map_err(|e| e.to_string())?;
            let fields =
                parse_local_fields(raw.as_object().expect("definition"), "schema2 definition")?;
            Ok(LocalRow {
                key: agent.key.clone(),
                remove: false,
                fields,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(LocalLayer {
        rows,
        order: None,
        complete_definitions: true,
    })
}

pub(crate) fn merge_config_seed(
    current: Option<ConfigSeedConfig>,
    patch: &ConfigSeedPatch,
) -> ConfigSeedConfig {
    let mut merged = current.unwrap_or(ConfigSeedConfig {
        enabled: true,
        dest: String::new(),
    });
    if let Some(enabled) = patch.enabled {
        merged.enabled = enabled;
    }
    if let Some(dest) = &patch.dest {
        merged.dest = dest.clone();
    }
    merged
}

/// #2124 - merge the local `idleBurst` patch by presence: an absent subfield
/// inherits the base value; an object after a missing or `null` base starts from
/// all-absent, i.e. the launch-time defaults.
pub(crate) fn merge_idle_burst(
    current: Option<IdleBurstConfig>,
    patch: &IdleBurstConfig,
) -> IdleBurstConfig {
    let mut merged = current.unwrap_or_default();
    if patch.max_bytes.is_some() {
        merged.max_bytes = patch.max_bytes;
    }
    if patch.max_secs.is_some() {
        merged.max_secs = patch.max_secs;
    }
    if patch.prior_silence_secs.is_some() {
        merged.prior_silence_secs = patch.prior_silence_secs;
    }
    merged
}

/// #2736 - merge the local `installCommands` patch by presence onto the base
/// object, or onto an empty `default` when there is none. A blank resulting
/// `default` fails the composed re-validation.
pub(crate) fn merge_install_commands(
    base: Option<InstallCommands>,
    patch: &InstallCommandsPatch,
) -> InstallCommands {
    let mut merged = base.unwrap_or(InstallCommands {
        default: String::new(),
        windows: None,
        macos: None,
        linux: None,
    });
    if let Some(default) = &patch.default {
        merged.default = default.clone();
    }
    if let Some(value) = &patch.windows {
        merged.windows = value.clone();
    }
    if let Some(value) = &patch.macos {
        merged.macos = value.clone();
    }
    if let Some(value) = &patch.linux {
        merged.linux = value.clone();
    }
    merged
}

pub(crate) fn apply_local_fields(
    mut definition: CodingAgentDefinition,
    fields: &LocalFieldPatch,
) -> CodingAgentDefinition {
    if let Some(value) = &fields.label {
        definition.label = value.clone();
    }
    if let Some(value) = &fields.description {
        definition.description = value.clone();
    }
    if let Some(value) = &fields.color {
        definition.color = value.clone();
    }
    if let Some(value) = &fields.command {
        definition.command = value.clone();
    }
    if let Some(value) = &fields.instructions_filename {
        definition.instructions_filename = value.clone();
    }
    if let Some(value) = &fields.envs {
        definition.envs = value.clone();
    }
    if let Some(value) = fields.isolated_home {
        definition.isolated_home = value;
    }
    if let Some(patch) = &fields.config_seed {
        definition.config_seed = match patch {
            None => None,
            Some(patch) => Some(merge_config_seed(definition.config_seed.clone(), patch)),
        };
    }
    if let Some(value) = fields.removable {
        definition.removable = value;
    }
    if let Some(value) = &fields.update_commands {
        definition.update_commands = value.clone();
    }
    if let Some(value) = fields.auto_update {
        definition.auto_update = value;
    }
    if let Some(patch) = &fields.idle_burst {
        definition.idle_burst = match patch {
            None => None,
            Some(patch) => Some(merge_idle_burst(definition.idle_burst.clone(), patch)),
        };
    }
    if let Some(patch) = &fields.install_commands {
        definition.install_commands = match patch {
            None => None,
            Some(patch) => Some(merge_install_commands(
                definition.install_commands.clone(),
                patch,
            )),
        };
    }
    definition
}

/// A NEW key requires every authored field explicitly; `instructionsFilename`
/// `configSeed`, `idleBurst` and `installCommands` stay optional (absent or
/// `null`).
pub(crate) fn build_new_definition(
    key: &str,
    fields: &LocalFieldPatch,
    description: LayerDescription,
) -> Result<CodingAgentDefinition, String> {
    let mut missing = Vec::new();
    if fields.label.is_none() {
        missing.push("label");
    }
    if fields.description.is_none() {
        missing.push("description");
    }
    if fields.color.is_none() {
        missing.push("color");
    }
    if fields.command.is_none() {
        missing.push("command");
    }
    if fields.envs.is_none() {
        missing.push("envs");
    }
    if fields.isolated_home.is_none() {
        missing.push("isolatedHome");
    }
    if fields.removable.is_none() {
        missing.push("removable");
    }
    if fields.update_commands.is_none() {
        missing.push("updateCommands");
    }
    if fields.auto_update.is_none() {
        missing.push("autoUpdate");
    }
    if !missing.is_empty() {
        return Err(format!(
            "new {} coding-agent '{key}' is missing required field(s): {}",
            description.name(),
            missing.join(", ")
        ));
    }
    let config_seed = match &fields.config_seed {
        None | Some(None) => None,
        Some(Some(patch)) => Some(merge_config_seed(None, patch)),
    };
    Ok(CodingAgentDefinition {
        key: key.to_string(),
        label: fields.label.clone().unwrap_or_default(),
        description: fields.description.clone().unwrap_or_default(),
        color: fields.color.clone().unwrap_or_default(),
        command: fields.command.clone().unwrap_or_default(),
        instructions_filename: fields.instructions_filename.clone().unwrap_or(None),
        envs: fields.envs.clone().unwrap_or_default(),
        isolated_home: fields.isolated_home.unwrap_or(false),
        config_seed,
        removable: fields.removable.unwrap_or(true),
        update_commands: fields.update_commands.clone().unwrap_or_default(),
        auto_update: fields.auto_update.unwrap_or(false),
        idle_burst: match &fields.idle_burst {
            None | Some(None) => None,
            Some(Some(config)) => Some(config.clone()),
        },
        install_commands: match &fields.install_commands {
            None | Some(None) => None,
            Some(Some(patch)) => Some(merge_install_commands(None, patch)),
        },
    })
}

/// Compose the local layer onto the base definitions. Rows merge by stable key
/// (omission inherits, present values replace); local additions append in local
/// order; the optional root `order` moves listed survivors first while every
/// unlisted survivor keeps base-then-local-add order. The whole candidate is
/// validated before anything is returned, and one bad definition discards the
/// ENTIRE local layer. The built-in support gate is applied by the caller so
/// the unedited-content check can still see the full base.
pub(crate) fn compose_local_layer(
    base: &[CodingAgentDefinition],
    local: &LocalLayer,
    description: LayerDescription,
) -> Result<Vec<CodingAgentDefinition>, String> {
    let layer = description.name();
    let composition = format!("{layer} composition");
    let mut working: Vec<CodingAgentDefinition> = base.to_vec();
    let mut index: HashMap<String, usize> = HashMap::with_capacity(working.len());
    for (position, definition) in working.iter().enumerate() {
        index.insert(definition.key.clone(), position);
    }
    let mut removed = vec![false; working.len()];
    let mut local_added: Vec<CodingAgentDefinition> = Vec::new();

    for row in &local.rows {
        match index.get(&row.key).copied() {
            Some(position) => {
                if row.remove {
                    if !working[position].removable {
                        return Err(format!(
                            "{layer} remove row targets nonremovable coding agent '{}'",
                            row.key
                        ));
                    }
                    removed[position] = true;
                } else {
                    working[position] = if local.complete_definitions {
                        build_new_definition(&row.key, &row.fields, description)?
                    } else {
                        apply_local_fields(working[position].clone(), &row.fields)
                    };
                }
            }
            None => {
                // An unknown tombstone is valid and retained for a future
                // shipped key; only a new ordinary row needs a complete def.
                if row.remove {
                    continue;
                }
                local_added.push(build_new_definition(&row.key, &row.fields, description)?);
            }
        }
    }

    let mut effective: Vec<CodingAgentDefinition> =
        Vec::with_capacity(working.len() + local_added.len());
    for (position, definition) in working.into_iter().enumerate() {
        if !removed[position] {
            effective.push(definition);
        }
    }
    effective.extend(local_added);

    for definition in &effective {
        if validate_definition(definition).is_err() {
            return Err(format!(
                "coding agent '{}' failed validation after composition: {}",
                definition.key,
                definition_problem_reason(definition)
            ));
        }
        for (index, command) in definition.update_commands.iter().enumerate() {
            validate_update_command_string(command, &composition, index).map_err(|reason| {
                format!(
                    "coding agent '{}' failed validation after composition: {reason}",
                    definition.key
                )
            })?;
        }
        if let Some(install) = definition.install_commands.as_ref() {
            validate_install_commands(install, &composition).map_err(|reason| {
                format!(
                    "coding agent '{}' failed validation after composition: {reason}",
                    definition.key
                )
            })?;
        }
    }

    if let Some(order) = &local.order {
        let position: HashMap<&str, usize> = effective
            .iter()
            .enumerate()
            .map(|(index, definition)| (definition.key.as_str(), index))
            .collect();
        let mut taken = vec![false; effective.len()];
        let mut ordered = Vec::with_capacity(effective.len());
        for key in order {
            if let Some(&position) = position.get(key.as_str()) {
                if !taken[position] {
                    taken[position] = true;
                    ordered.push(effective[position].clone());
                }
            }
        }
        for (index, definition) in effective.into_iter().enumerate() {
            if !taken[index] {
                ordered.push(definition);
            }
        }
        effective = ordered;
    }

    Ok(effective)
}

/// Apply the shipped support gate to a resolved catalog, warning once per
/// suppressed built-in key.
pub(crate) fn apply_support_gate(
    definitions: Vec<CodingAgentDefinition>,
    path: &Path,
    warnings: &mut Vec<CatalogDiagnostic>,
) -> Vec<CodingAgentDefinition> {
    let table = active_builtin_agent_support();
    let mut suppressed: Vec<String> = Vec::new();
    let mut kept = Vec::with_capacity(definitions.len());
    for definition in definitions {
        if is_supported_builtin(&definition.key, table) {
            kept.push(definition);
        } else if !suppressed.contains(&definition.key) {
            suppressed.push(definition.key.clone());
            warnings.push(catalog_diagnostic(
                REPORT_CODE_INVALID_DEFINITION,
                path,
                format!(
                    "coding-agent '{}' is not supported by this build and was omitted",
                    definition.key
                ),
            ));
        }
    }
    kept
}
