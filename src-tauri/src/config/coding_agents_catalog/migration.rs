use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::config::settings::{CodingAgentEnv, ConfigSeedConfig};

use super::{
    analyze_base_bytes, build_managed_base_bytes, build_managed_base_bytes_with_original,
    catalog_diagnostic, compose_local_layer, definition_problem_reason, expect_json_object,
    inject_catalog_failure, managed_base_is_stale, managed_content_sha256,
    original_catalog_command, parse_local_layer, parse_strict_json, publish_exclusive,
    publish_replace, push_unknown_field_warnings, raw_install_commands_problem,
    raw_update_commands_problem, read_instance_legacy_source, read_optional_regular_file,
    reject_unknown_json_fields, sha256_hex, supported_shipped_definitions, validate_definition,
    BaseAnalysis, CatalogDiagnostic, CatalogPaths, CodingAgentDefinition, IdleBurstConfig,
    InstallCommands, LayerDescription, CATALOG_MANIFEST_FILENAME, CATALOG_SCHEMA_VERSION,
    KNOWN_DEFINITION_FIELDS, LOCAL_STUB_BYTES, MIGRATION_JOURNAL_VERSION,
    REPORT_CODE_REFRESH_FAILED,
};

// ---------------------------------------------------------------------------
// Legacy extraction, migration, recovery and refresh.
// ---------------------------------------------------------------------------

pub(crate) const MIGRATION_SOURCE_PROJECT: &str = "project";
pub(crate) const MIGRATION_SOURCE_INSTANCE: &str = "instance";

/// The immutable migration sidecar. It records the source identity, the local
/// layer identity and the COMPLETE intended managed base (with its own
/// revision/content digest), so an interrupted transaction can be recomputed
/// deterministically without consulting current embedded defaults. It is not a
/// secret store: diagnostics never print its body.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MigrationJournalWire {
    pub(crate) version: u32,
    pub(crate) source_kind: String,
    pub(crate) source_path: String,
    pub(crate) source_sha256: String,
    pub(crate) local_sha256: String,
    pub(crate) local_byte_length: u64,
    pub(crate) managed_revision: String,
    pub(crate) managed_content_sha256: String,
    pub(crate) managed_base: serde_json::Value,
}

pub(crate) fn parse_migration_journal(bytes: &[u8]) -> Result<MigrationJournalWire, String> {
    let value = parse_strict_json(bytes)
        .map_err(|reason| format!("the migration journal is not valid JSON ({reason})"))?;
    let journal: MigrationJournalWire = serde_json::from_value(value)
        .map_err(|error| format!("the migration journal does not match the v1 shape ({error})"))?;
    if journal.version != MIGRATION_JOURNAL_VERSION {
        return Err(format!(
            "unsupported migration journal version {}; only version 1 is recognized",
            journal.version
        ));
    }
    if journal.source_kind != MIGRATION_SOURCE_PROJECT
        && journal.source_kind != MIGRATION_SOURCE_INSTANCE
    {
        return Err("the migration journal carries an unrecognized sourceKind".to_string());
    }
    Ok(journal)
}

/// The serialized local layer. Only authored fields are emitted, so an
/// extracted pin carries exactly the legacy presence it represents.
#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LocalRowWire {
    pub(crate) key: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) remove: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) color: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "baseCommand")]
    pub(crate) command: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) instructions_filename: Option<Option<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) envs: Option<Vec<CodingAgentEnv>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) isolated_home: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) config_seed: Option<Option<ConfigSeedWire>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) removable: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) update_commands: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) auto_update: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) idle_burst: Option<Option<IdleBurstConfig>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) install_commands: Option<Option<InstallCommandsWire>>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct HistoricalLocalRowWire<'a> {
    pub(crate) key: &'a String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) remove: &'a Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) label: &'a Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) description: &'a Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) color: &'a Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) command: &'a Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) instructions_filename: &'a Option<Option<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) envs: &'a Option<Vec<CodingAgentEnv>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) isolated_home: &'a Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) config_seed: &'a Option<Option<ConfigSeedWire>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) removable: &'a Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) update_commands: &'a Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) auto_update: &'a Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) idle_burst: &'a Option<Option<IdleBurstConfig>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) install_commands: &'a Option<Option<InstallCommandsWire>>,
}
impl<'a> From<&'a LocalRowWire> for HistoricalLocalRowWire<'a> {
    fn from(row: &'a LocalRowWire) -> Self {
        Self {
            key: &row.key,
            remove: &row.remove,
            label: &row.label,
            description: &row.description,
            color: &row.color,
            command: &row.command,
            instructions_filename: &row.instructions_filename,
            envs: &row.envs,
            isolated_home: &row.isolated_home,
            config_seed: &row.config_seed,
            removable: &row.removable,
            update_commands: &row.update_commands,
            auto_update: &row.auto_update,
            idle_burst: &row.idle_burst,
            install_commands: &row.install_commands,
        }
    }
}

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub(crate) struct InstallCommandsWire {
    pub(crate) default: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) windows: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) macos: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) linux: Option<String>,
}

impl From<&InstallCommands> for InstallCommandsWire {
    fn from(value: &InstallCommands) -> Self {
        Self {
            default: value.default.clone(),
            windows: value.windows.clone(),
            macos: value.macos.clone(),
            linux: value.linux.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ConfigSeedWire {
    pub(crate) enabled: bool,
    pub(crate) dest: String,
}

impl From<&ConfigSeedConfig> for ConfigSeedWire {
    fn from(config: &ConfigSeedConfig) -> Self {
        Self {
            enabled: config.enabled,
            dest: config.dest.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LocalLayerWire<'a> {
    pub(crate) schema_version: u32,
    pub(crate) agents: Vec<LocalRowWithLegacy<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) order: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct LocalRowWithLegacy<'a> {
    #[serde(flatten)]
    pub(crate) row: &'a LocalRowWire,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) command: Option<&'a serde_json::Value>,
}

pub(crate) fn local_layer_bytes(rows: Vec<LocalRowWire>, order: Option<Vec<String>>) -> Vec<u8> {
    local_layer_bytes_with_original(rows, order, None)
}

pub(crate) fn local_layer_bytes_with_original(
    rows: Vec<LocalRowWire>,
    order: Option<Vec<String>>,
    original: Option<&serde_json::Value>,
) -> Vec<u8> {
    let layer = LocalLayerWire {
        schema_version: CATALOG_SCHEMA_VERSION,
        agents: rows
            .iter()
            .map(|row| LocalRowWithLegacy {
                row,
                command: original_catalog_command(original, &row.key),
            })
            .collect(),
        order,
    };
    let mut bytes = match serde_json::to_vec_pretty(&layer) {
        Ok(bytes) => bytes,
        Err(error) => {
            log::error!("[coding-agents] failed to serialize the local catalog layer ({error})");
            Vec::new()
        }
    };
    bytes.push(b'\n');
    bytes
}

/// Pin exactly the fields the legacy row states EXPLICITLY: presence, never
/// value equality with a current default, is the intent signal. An absent
/// `updateCommands` therefore inherits the newly persisted shipped default,
/// while an explicit `[]` stays empty.
pub(crate) fn pin_explicit_legacy_fields(
    definition: &CodingAgentDefinition,
    raw: &serde_json::Map<String, serde_json::Value>,
) -> LocalRowWire {
    LocalRowWire {
        key: definition.key.clone(),
        remove: None,
        label: raw.contains_key("label").then(|| definition.label.clone()),
        description: raw
            .contains_key("description")
            .then(|| definition.description.clone()),
        color: raw.contains_key("color").then(|| definition.color.clone()),
        command: (raw.contains_key("baseCommand") || raw.contains_key("command"))
            .then(|| definition.command.clone()),
        instructions_filename: raw
            .contains_key("instructionsFilename")
            .then(|| definition.instructions_filename.clone()),
        envs: raw.contains_key("envs").then(|| definition.envs.clone()),
        isolated_home: raw
            .contains_key("isolatedHome")
            .then_some(definition.isolated_home),
        config_seed: raw
            .contains_key("configSeed")
            .then(|| definition.config_seed.as_ref().map(ConfigSeedWire::from)),
        removable: raw
            .contains_key("removable")
            .then_some(definition.removable),
        update_commands: raw
            .contains_key("updateCommands")
            .then(|| definition.update_commands.clone()),
        auto_update: raw
            .contains_key("autoUpdate")
            .then_some(definition.auto_update),
        idle_burst: raw
            .contains_key("idleBurst")
            .then(|| definition.idle_burst.clone()),
        install_commands: raw.contains_key("installCommands").then(|| {
            definition
                .install_commands
                .as_ref()
                .map(InstallCommandsWire::from)
        }),
    }
}

/// Materialize a COMPLETE local definition for a custom key or a changed
/// command, using the values the legacy read produced (explicit or default) and
/// forcing an absent `updateCommands` to `[]` so the new shipped sequence never
/// leaks into a command the user owns.
pub(crate) fn materialize_complete_legacy_fields(
    definition: &CodingAgentDefinition,
    raw: &serde_json::Map<String, serde_json::Value>,
) -> LocalRowWire {
    LocalRowWire {
        key: definition.key.clone(),
        remove: None,
        label: Some(definition.label.clone()),
        description: Some(definition.description.clone()),
        color: Some(definition.color.clone()),
        command: Some(definition.command.clone()),
        instructions_filename: raw
            .contains_key("instructionsFilename")
            .then(|| definition.instructions_filename.clone()),
        envs: Some(definition.envs.clone()),
        isolated_home: Some(definition.isolated_home),
        config_seed: raw
            .contains_key("configSeed")
            .then(|| definition.config_seed.as_ref().map(ConfigSeedWire::from)),
        removable: Some(definition.removable),
        update_commands: Some(if raw.contains_key("updateCommands") {
            definition.update_commands.clone()
        } else {
            Vec::new()
        }),
        auto_update: Some(definition.auto_update),
        idle_burst: raw
            .contains_key("idleBurst")
            .then(|| definition.idle_burst.clone()),
        install_commands: raw.contains_key("installCommands").then(|| {
            definition
                .install_commands
                .as_ref()
                .map(InstallCommandsWire::from)
        }),
    }
}

/// Compute the extracted local layer for a readable legacy catalog against the
/// intended (shipped) managed base. Every ambiguity refuses the transfer: a
/// duplicate member (already rejected by the strict parser), an unknown
/// field, an invalid definition, a duplicate key or a non-removable missing
/// shipped key. Nothing is written here.
pub(crate) fn extract_legacy_local(
    source_bytes: &[u8],
    shipped: &[CodingAgentDefinition],
) -> Result<Vec<u8>, String> {
    extract_legacy_local_mode(source_bytes, shipped, false)
}

pub(crate) fn extract_legacy_local_mode(
    source_bytes: &[u8],
    shipped: &[CodingAgentDefinition],
    historical: bool,
) -> Result<Vec<u8>, String> {
    let value = parse_strict_json(source_bytes)
        .map_err(|reason| format!("the legacy catalog is not valid JSON ({reason})"))?;
    let root = expect_json_object(&value, "the legacy catalog root")?;
    reject_unknown_json_fields(
        root,
        &["schemaVersion", "agents"],
        "the legacy catalog root",
    )?;
    if let Some(version) = root.get("schemaVersion") {
        if version.as_u64() != Some(CATALOG_SCHEMA_VERSION as u64) {
            return Err(format!(
                "unsupported legacy schemaVersion {version}; migration is refused"
            ));
        }
    }
    let empty_rows = Vec::new();
    let rows = match root.get("agents") {
        None => &empty_rows,
        Some(serde_json::Value::Array(rows)) => rows,
        Some(_) => return Err("the legacy 'agents' value must be a JSON array".to_string()),
    };

    let shipped_by_key: HashMap<&str, &CodingAgentDefinition> = shipped
        .iter()
        .map(|definition| (definition.key.as_str(), definition))
        .collect();

    let mut seen_keys = HashSet::new();
    let mut legacy: Vec<(
        &CodingAgentDefinition,
        &serde_json::Map<String, serde_json::Value>,
    )> = Vec::with_capacity(rows.len());
    let mut parsed: Vec<CodingAgentDefinition> = Vec::with_capacity(rows.len());
    for (index, raw) in rows.iter().enumerate() {
        let context = format!("legacy entry {index}");
        let object = expect_json_object(raw, &context)?;
        reject_unknown_json_fields(object, KNOWN_DEFINITION_FIELDS, &context)?;
        if let Some(problem) =
            raw_update_commands_problem(raw).or_else(|| raw_install_commands_problem(raw))
        {
            return Err(format!("{context} is ambiguous: {problem}"));
        }
        let definition: CodingAgentDefinition = serde_json::from_value(raw.clone())
            .map_err(|_| format!("{context} does not match the coding-agent definition schema"))?;
        if validate_definition(&definition).is_err() {
            return Err(format!(
                "{context} fails validation: {}",
                definition_problem_reason(&definition)
            ));
        }
        let mut nested = Vec::new();
        if push_unknown_field_warnings(object, &definition.key, Path::new("legacy"), &mut nested) {
            return Err(format!(
                "{context} carries unknown nested field(s) that cannot be migrated safely"
            ));
        }
        if !seen_keys.insert(definition.key.clone()) {
            return Err(format!(
                "duplicate legacy coding-agent key '{}'",
                definition.key
            ));
        }
        parsed.push(definition);
    }
    for (index, raw) in rows.iter().enumerate() {
        let object = raw
            .as_object()
            .expect("validated as an object in the first pass");
        legacy.push((&parsed[index], object));
    }

    let mut local_rows: Vec<LocalRowWire> = Vec::with_capacity(parsed.len() + shipped.len());
    let mut order: Vec<String> = Vec::with_capacity(parsed.len());
    for (definition, object) in &legacy {
        order.push(definition.key.clone());
        match shipped_by_key.get(definition.key.as_str()) {
            Some(shipped_definition) if shipped_definition.command == definition.command => {
                local_rows.push(pin_explicit_legacy_fields(definition, object));
            }
            _ => local_rows.push(materialize_complete_legacy_fields(definition, object)),
        }
    }
    for shipped_definition in shipped {
        if !seen_keys.contains(&shipped_definition.key) {
            if !shipped_definition.removable {
                return Err(format!(
                    "reconciliation is blocked: shipped key '{}' is absent from the legacy catalog \
                        and is not removable, so no tombstone can be recorded",
                    shipped_definition.key
                ));
            }
            local_rows.push(LocalRowWire {
                key: shipped_definition.key.clone(),
                remove: Some(true),
                ..LocalRowWire::default()
            });
        }
    }
    if historical {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct HistoricalLayer<'a> {
            schema_version: u32,
            agents: Vec<HistoricalLocalRowWire<'a>>,
            order: Option<Vec<String>>,
        }
        let layer = HistoricalLayer {
            schema_version: CATALOG_SCHEMA_VERSION,
            agents: local_rows
                .iter()
                .map(HistoricalLocalRowWire::from)
                .collect(),
            order: Some(order),
        };
        let mut bytes = serde_json::to_vec_pretty(&layer).map_err(|error| error.to_string())?;
        bytes.push(b'\n');
        return Ok(bytes);
    }
    if local_rows
        .iter()
        .all(|row| original_catalog_command(Some(&value), &row.key).is_none())
    {
        Ok(local_layer_bytes(local_rows, Some(order)))
    } else {
        Ok(local_layer_bytes_with_original(
            local_rows,
            Some(order),
            Some(&value),
        ))
    }
}

/// Execute one migration transaction under the held catalog lock. Order:
/// durable exclusive backup, durable journal, exclusive local layer (verified),
/// source/target re-check, then the base. Every failure leaves the earlier
/// artifacts in place for recovery and never publishes partial bytes.
pub(crate) fn migrate_legacy(
    paths: &CatalogPaths,
    source_kind: &'static str,
    source_path: &Path,
    source_bytes: &[u8],
) -> Result<Option<DateTime<Utc>>, String> {
    if read_optional_regular_file(&paths.local, "local coding-agent catalog")?.is_some() {
        return Err(
            "an existing local overrides file blocks the legacy ownership transfer; it was left \
                untouched"
                .to_string(),
        );
    }
    if read_optional_regular_file(&paths.backup, "coding-agent migration backup")?.is_some()
        || read_optional_regular_file(&paths.journal, "coding-agent migration journal")?.is_some()
    {
        return Err(
            "an existing migration sidecar blocks a new migration; recover or reconcile it first"
                .to_string(),
        );
    }

    let shipped = supported_shipped_definitions();
    let managed_bytes = build_managed_base_bytes(&shipped);
    let local_bytes = extract_legacy_local(source_bytes, &shipped)?;
    // All extraction is computed and strictly validated before ANY write.
    let layer = parse_local_layer(&local_bytes, LayerDescription::Local)?;
    compose_local_layer(&shipped, &layer, LayerDescription::Local)?;
    let managed_base_value: serde_json::Value = serde_json::from_slice(&managed_bytes)
        .map_err(|e| format!("the intended managed base did not serialize to JSON ({e})"))?;
    let revision = managed_content_sha256(&shipped);
    let journal = MigrationJournalWire {
        version: MIGRATION_JOURNAL_VERSION,
        source_kind: source_kind.to_string(),
        source_path: source_path.display().to_string(),
        source_sha256: sha256_hex(source_bytes),
        local_sha256: sha256_hex(&local_bytes),
        local_byte_length: local_bytes.len() as u64,
        managed_revision: revision.clone(),
        managed_content_sha256: revision,
        managed_base: managed_base_value,
    };
    let mut journal_bytes = serde_json::to_vec_pretty(&journal)
        .map_err(|e| format!("the migration journal did not serialize ({e})"))?;
    journal_bytes.push(b'\n');

    publish_exclusive(&paths.backup, source_bytes)?;
    inject_catalog_failure("after_backup")?;
    publish_exclusive(&paths.journal, &journal_bytes)?;
    inject_catalog_failure("after_journal")?;
    publish_exclusive(&paths.local, &local_bytes)?;
    let written = std::fs::read(&paths.local)
        .map_err(|e| format!("the published local layer could not be re-read ({e})"))?;
    if written.len() as u64 != journal.local_byte_length
        || sha256_hex(&written) != journal.local_sha256
    {
        return Err(
            "the published local layer did not verify byte-for-byte; the base was not published"
                .to_string(),
        );
    }
    inject_catalog_failure("after_local")?;

    if source_kind == MIGRATION_SOURCE_INSTANCE {
        if read_optional_regular_file(&paths.base, "persisted catalog")?.is_some() {
            return Err(
                "a managed base appeared during the migration; the base was not published"
                    .to_string(),
            );
        }
        let current = std::fs::read(source_path)
            .map_err(|e| format!("the instance source could not be re-read ({e})"))?;
        if current != source_bytes {
            return Err(
                "the instance source changed during the migration; the base was not published"
                    .to_string(),
            );
        }
    } else {
        let current = std::fs::read(&paths.base)
            .map_err(|e| format!("the project base could not be re-read ({e})"))?;
        if current != source_bytes {
            return Err(
                "the project base changed during the migration; it was left untouched".to_string(),
            );
        }
    }
    inject_catalog_failure("before_base")?;
    let published_at = Utc::now();
    if source_kind == MIGRATION_SOURCE_PROJECT {
        publish_replace(&paths.base, &managed_bytes)?;
    } else {
        publish_exclusive(&paths.base, &managed_bytes)?;
    }
    inject_catalog_failure("after_base")?;
    log::info!(
        "[coding-agents] migrated the {source_kind} catalog {} into the managed base with an \
            extracted local layer",
        source_path.display()
    );
    Ok(Some(published_at))
}

/// Resume an interrupted transaction recorded by a journal: complete when the
/// published base already matches the journal identity, otherwise recompute and
/// verify the local layer before finishing. Any mismatch preserves every byte
/// and blocks with a reconciliation reason.
pub(crate) fn recover_interrupted_migration(
    paths: &CatalogPaths,
) -> Result<Option<DateTime<Utc>>, String> {
    let journal_bytes = std::fs::read(&paths.journal)
        .map_err(|e| format!("the migration journal could not be read ({e})"))?;
    let journal = parse_migration_journal(&journal_bytes)?;
    let backup = std::fs::read(&paths.backup).map_err(|e| {
        format!(
            "the migration backup is missing or unreadable ({e}); recovery is blocked and every \
                byte is preserved"
        )
    })?;
    if sha256_hex(&backup) != journal.source_sha256 {
        return Err(
            "the migration backup does not match the journal source hash; recovery is blocked and \
                every byte is preserved"
                .to_string(),
        );
    }
    let managed_base_value = journal.managed_base.clone();
    let saved_agents = managed_base_value
        .get("agents")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    let managed_definitions: Vec<CodingAgentDefinition> = serde_json::from_value(saved_agents)
        .map_err(|_| {
            "the journal saved managed base is not a coding-agent definition array".to_string()
        })?;
    let saved_revision = managed_base_value
        .get("managed")
        .and_then(|marker| marker.get("revision"))
        .and_then(|value| value.as_str());
    let saved_content = managed_base_value
        .get("managed")
        .and_then(|marker| marker.get("contentSha256"))
        .and_then(|value| value.as_str());
    if saved_revision != Some(journal.managed_revision.as_str())
        || saved_content != Some(journal.managed_content_sha256.as_str())
    {
        return Err(
            "the journal saved managed base does not match its recorded revision; recovery is \
                blocked"
                .to_string(),
        );
    }
    let mut managed_bytes = serde_json::to_vec_pretty(&managed_base_value)
        .map_err(|e| format!("the journal saved managed base did not serialize ({e})"))?;
    managed_bytes.push(b'\n');

    if let Some(base_bytes) = read_optional_regular_file(&paths.base, "persisted catalog")? {
        if let Ok(analysis) = analyze_base_bytes(&paths.base, &base_bytes) {
            if let Some(marker) = analysis.marker.as_ref() {
                if marker.revision == journal.managed_revision
                    && marker.content_sha256 == journal.managed_content_sha256
                {
                    // Completed: never re-extract or overwrite the local layer.
                    return Ok(None);
                }
            }
        }
        if journal.source_kind == MIGRATION_SOURCE_INSTANCE || base_bytes != backup {
            return Err(
                "the existing catalog base does not match the interrupted migration; recovery is \
                    blocked and every byte is preserved"
                    .to_string(),
            );
        }
        // Project migration: the base still holds the exact source bytes, so
        // the base was never replaced; continue below and finish the sequence.
    }

    let published_at = match read_optional_regular_file(&paths.local, "local coding-agent catalog")?
    {
        Some(local_bytes) => {
            if sha256_hex(&local_bytes) != journal.local_sha256
                || local_bytes.len() as u64 != journal.local_byte_length
            {
                return Err(
                    "the local overrides file does not match the interrupted migration; \
                            recovery is blocked and every byte is preserved"
                        .to_string(),
                );
            }
            if journal.source_kind == MIGRATION_SOURCE_INSTANCE {
                let source_path = PathBuf::from(&journal.source_path);
                if let Some(source_bytes) =
                    read_optional_regular_file(&source_path, "recorded migration source")?
                {
                    if sha256_hex(&source_bytes) != journal.source_sha256 {
                        return Err(
                            "the instance source changed during the interrupted migration; \
                                    recovery is blocked"
                                .to_string(),
                        );
                    }
                }
            }
            publish_base(&paths.base, &managed_bytes, journal.source_kind.as_str())?;
            Utc::now()
        }
        None => {
            let mut recomputed = extract_legacy_local(&backup, &managed_definitions)?;
            if sha256_hex(&recomputed) != journal.local_sha256
                || recomputed.len() as u64 != journal.local_byte_length
            {
                recomputed = extract_legacy_local_mode(&backup, &managed_definitions, true)?;
            }
            if sha256_hex(&recomputed) != journal.local_sha256
                || recomputed.len() as u64 != journal.local_byte_length
            {
                return Err(
                    "the recomputed extraction does not match the journal local hash; recovery \
                            is blocked and every byte is preserved"
                        .to_string(),
                );
            }
            publish_exclusive(&paths.local, &recomputed)?;
            inject_catalog_failure("after_local")?;
            publish_base(&paths.base, &managed_bytes, journal.source_kind.as_str())?;
            Utc::now()
        }
    };
    inject_catalog_failure("after_base")?;
    log::info!("[coding-agents] resumed and completed an interrupted managed-catalog migration");
    Ok(Some(published_at))
}

/// Finish a journal recovery's base publication: a project migration replaces
/// the source file in place, while an instance import publishes into a
/// destination that must still be absent.
pub(crate) fn publish_base(path: &Path, bytes: &[u8], source_kind: &str) -> Result<(), String> {
    if source_kind == MIGRATION_SOURCE_PROJECT {
        publish_replace(path, bytes)
    } else {
        publish_exclusive(path, bytes)
    }
}

/// Backup-only recovery: no journal, so the only admissible continuation is the
/// case where the backup is byte-equal to the CURRENT original source and no
/// local file exists. The extraction is recomputed against the shipped defaults
/// because no saved base exists to recompute against.
pub(crate) fn resume_backup_only_migration(
    paths: &CatalogPaths,
    legacy_catalog_dir: Option<&Path>,
) -> Result<Option<DateTime<Utc>>, String> {
    let backup = std::fs::read(&paths.backup)
        .map_err(|e| format!("the migration backup could not be read ({e})"))?;
    if read_optional_regular_file(&paths.local, "local coding-agent catalog")?.is_some() {
        return Err(
            "a local overrides file exists without a journal; the interrupted migration cannot be \
                resumed safely"
                .to_string(),
        );
    }
    let shipped = supported_shipped_definitions();
    let base_bytes = read_optional_regular_file(&paths.base, "persisted catalog")?;
    let instance_path = legacy_catalog_dir.map(|dir| dir.join(CATALOG_MANIFEST_FILENAME));
    let instance_bytes = match instance_path.as_ref() {
        Some(path) => read_instance_legacy_source(path)?,
        None => None,
    };
    let source_kind =
        match (&base_bytes, &instance_bytes) {
            (Some(base), _) if *base == backup => MIGRATION_SOURCE_PROJECT,
            (None, Some(instance)) if *instance == backup => MIGRATION_SOURCE_INSTANCE,
            _ => return Err(
                "the backup does not match the current project base or instance catalog; recovery \
                    is blocked and every byte is preserved"
                    .to_string(),
            ),
        };
    let managed_bytes = build_managed_base_bytes(&shipped);
    let local_bytes = extract_legacy_local(&backup, &shipped)?;
    let layer = parse_local_layer(&local_bytes, LayerDescription::Local)?;
    compose_local_layer(&shipped, &layer, LayerDescription::Local)?;
    publish_exclusive(&paths.local, &local_bytes)?;
    let published_at = Utc::now();
    if source_kind == MIGRATION_SOURCE_PROJECT {
        publish_replace(&paths.base, &managed_bytes)?;
    } else {
        publish_exclusive(&paths.base, &managed_bytes)?;
    }
    log::info!("[coding-agents] resumed a backup-only interrupted migration");
    Ok(Some(published_at))
}

/// Refresh a verified, unedited managed base when the current shipped revision
/// differs from the base's recorded revision (a support-gate change is part of
/// that revision). A base whose content hash does not match its marker is NEVER
/// refreshed, and neither is one carrying unknown fields: refresh replaces only
/// a verified managed base.
pub(crate) fn refresh_managed_base(
    paths: &CatalogPaths,
    analysis: &BaseAnalysis,
) -> Result<Option<DateTime<Utc>>, String> {
    let shipped = supported_shipped_definitions();
    let shipped_revision = managed_content_sha256(&shipped);
    if !managed_base_is_stale(analysis, &shipped_revision) {
        return Ok(None);
    }
    let before = read_optional_regular_file(&paths.base, "persisted catalog")?
        .ok_or("catalog disappeared before refresh")?;
    let before = parse_strict_json(&before)?;
    let managed_bytes = build_managed_base_bytes_with_original(&shipped, Some(&before));
    publish_replace(&paths.base, &managed_bytes)?;
    log::info!(
        "[coding-agents] refreshed the managed catalog base revision at {}",
        paths.base.display()
    );
    Ok(Some(Utc::now()))
}

/// Fresh initialization: exclusive managed defaults plus the create-once local
/// stub. The base is written first; a stub failure leaves the valid base
/// published and usable, and NO automatic stub retry is scheduled - an absent
/// local file is valid, so an intentional deletion stays deleted. The failure
/// is returned alongside the successful publication so the caller's existing
/// initialization logging carries it; nothing durable records it.
pub(crate) fn fresh_initialize_catalog(
    paths: &CatalogPaths,
) -> Result<(Option<DateTime<Utc>>, Option<CatalogDiagnostic>), String> {
    let shipped = supported_shipped_definitions();
    let managed_bytes = build_managed_base_bytes(&shipped);
    let published_at = if read_optional_regular_file(&paths.base, "persisted catalog")?.is_none() {
        publish_exclusive(&paths.base, &managed_bytes)?;
        Some(Utc::now())
    } else {
        None
    };
    let stub_warning = |detail: String| {
        Some(catalog_diagnostic(
            REPORT_CODE_REFRESH_FAILED,
            &paths.local,
            format!(
                "The managed catalog base is usable, but its local overrides stub could not be \
                    created: {detail}. An absent local file is valid; no automatic stub retry is \
                    scheduled."
            ),
        ))
    };
    let stub_warning = match std::fs::symlink_metadata(&paths.local) {
        Ok(_) => None,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            match publish_exclusive(&paths.local, LOCAL_STUB_BYTES) {
                Ok(()) => None,
                Err(stub_error) => stub_warning(stub_error),
            }
        }
        Err(error) => stub_warning(error.to_string()),
    };
    Ok((published_at, stub_warning))
}

#[derive(Default)]
pub(crate) struct CatalogInitOutcome {
    pub(crate) published_at: Option<DateTime<Utc>>,
    pub(crate) base_verified_managed: bool,
    pub(crate) warnings: Vec<CatalogDiagnostic>,
}
