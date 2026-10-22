use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::de::Visitor;
use serde::{de, Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt;

use crate::config::settings::{CodingAgentEnv, ConfigSeedConfig};

use super::{
    active_builtin_agent_support, catalog_dir, embedded_default_catalog, is_supported_builtin,
    unknown_field_names, CodingAgentDefinition, ConfigSeedPatch, IdleBurstConfig, InstallCommands,
    InstallCommandsPatch, CATALOG_SCHEMA_VERSION,
};

// ---------------------------------------------------------------------------
// #1968 P5 - managed base, local overrides and recoverable migration.
//
// `agents.json` becomes an AC-managed snapshot (root `managed` marker) of the
// supported shipped defaults; `<catalog>/agents.local.json` is the user-owned
// overrides layer. Ordinary reads compose the two and NEVER write. A one-shot
// migration extracts a legacy user-owned catalog into the local layer before
// publishing the managed base, through an immutable byte-exact backup plus a
// journal, so an interrupted migration is always resumable without inventing,
// overwriting or deleting user data.
// ---------------------------------------------------------------------------

/// Diagnostic codes this phase adds.
pub(crate) const REPORT_CODE_PROJECT_INVALID: &str = "projectInvalid";
pub(crate) const REPORT_CODE_LOCAL_INVALID: &str = "localInvalid";
pub(crate) const REPORT_CODE_REFRESH_FAILED: &str = "refreshFailed";
pub(crate) const REPORT_CODE_MIGRATION_CONFLICT: &str = "migrationConflict";
pub(crate) const REPORT_CODE_MANAGED_BASE_EDITED: &str = "managedBaseEdited";
pub(crate) const REPORT_CODE_PUBLICATION_UNTRACKED: &str = "publicationUntracked";

/// The only managed owner/version this build recognizes. Anything else never
/// grants ownership: it is readable, but neither refreshed nor migrated.
pub(crate) const MANAGED_OWNER: &str = "agentscommander";
pub(crate) const MANAGED_VERSION: u32 = 1;
pub(crate) const MANAGED_MARKER_FIELDS: &[&str] =
    &["owner", "version", "revision", "contentSha256"];

/// The local override file name and the two migration sidecars, aliased from
/// the leaf registry that also renders their git-ignore rows.
pub(crate) const LOCAL_CATALOG_FILENAME: &str =
    crate::config::instance_artifacts::CODING_AGENTS_LOCAL_FILENAME;
pub(crate) const MIGRATION_BACKUP_FILENAME: &str =
    crate::config::instance_artifacts::CODING_AGENTS_MIGRATION_BACKUP_FILENAME;
pub(crate) const MIGRATION_JOURNAL_FILENAME: &str =
    crate::config::instance_artifacts::CODING_AGENTS_MIGRATION_JOURNAL_FILENAME;
pub(crate) const CATALOG_LOCK_FILENAME: &str =
    crate::config::instance_artifacts::CODING_AGENTS_LOCK_FILENAME;

/// The create-once local stub: valid, empty, and user-owned after creation.
pub(crate) const LOCAL_STUB_BYTES: &[u8] = b"{\"schemaVersion\":1,\"agents\":[]}\n";

pub(crate) const MIGRATION_JOURNAL_VERSION: u32 = 1;

/// The catalog write lock's polling interval and whole acquire deadline,
/// mirroring `local_config_io`'s existing sidecar-lock behavior.
pub(crate) const CATALOG_LOCK_POLL_INTERVAL: Duration = Duration::from_millis(50);
pub(crate) const CATALOG_LOCK_TIMEOUT: Duration = Duration::from_secs(5);

pub(crate) fn local_catalog_path(ac_dir: &Path) -> PathBuf {
    catalog_dir(ac_dir).join(LOCAL_CATALOG_FILENAME)
}

pub(crate) fn project_catalog_path(ac_dir: &Path) -> PathBuf {
    catalog_dir(ac_dir).join(crate::config::instance_artifacts::CODING_AGENTS_PROJECT_TARGET_NAME)
}

#[derive(Clone, Copy)]
pub(crate) enum LayerDescription {
    Project,
    Local,
}

impl LayerDescription {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Project => "project",
            Self::Local => "local",
        }
    }
    pub(crate) fn invalid_code(self) -> &'static str {
        match self {
            Self::Project => REPORT_CODE_PROJECT_INVALID,
            Self::Local => REPORT_CODE_LOCAL_INVALID,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum CatalogReadScope {
    Project,
    Instance,
}

pub(crate) fn migration_journal_path(ac_dir: &Path) -> PathBuf {
    catalog_dir(ac_dir).join(MIGRATION_JOURNAL_FILENAME)
}

/// The four publication temporaries share one formula,
/// `.{destination}.{pid}.{counter}.tmp`, published into the destination's own
/// directory and named from its own file name. The formula itself lives in the
/// leaf registry (which also renders the ignore rows); this alias keeps the
/// writer and the policy on the same single source.
pub(crate) fn publication_temp_name_for_destination(
    destination_file_name: &str,
    pid: u32,
    counter: u64,
) -> String {
    crate::config::instance_artifacts::publication_temp_name_for_destination(
        destination_file_name,
        pid,
        counter,
    )
}

pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

/// The revision/content identity of a supported shipped definition array: the
/// SHA-256 of its DETERMINISTIC COMPACT UTF-8 serialization. The struct's own
/// field order is the serialization order, every authored field is emitted, and
/// `updateCommands` is always explicit (including `[]`), so formatting-free
/// round trips reproduce the hash and a whitespace-only edit does not count as
/// a data edit. `serde_json::Value` map iteration and raw bytes are never
/// consulted.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LegacyDefinitionHash<'a> {
    pub(crate) key: &'a String,
    pub(crate) label: &'a String,
    pub(crate) description: &'a String,
    pub(crate) color: &'a String,
    pub(crate) command: &'a String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) instructions_filename: &'a Option<String>,
    #[serde(default)]
    pub(crate) envs: &'a Vec<CodingAgentEnv>,
    #[serde(default)]
    pub(crate) isolated_home: &'a bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) config_seed: &'a Option<ConfigSeedConfig>,
    #[serde(default = "default_true")]
    pub(crate) removable: &'a bool,
    #[serde(default)]
    pub(crate) update_commands: &'a Vec<String>,
    #[serde(default)]
    pub(crate) auto_update: &'a bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) idle_burst: &'a Option<IdleBurstConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) install_commands: &'a Option<InstallCommands>,
}

impl<'a> From<&'a CodingAgentDefinition> for LegacyDefinitionHash<'a> {
    fn from(value: &'a CodingAgentDefinition) -> Self {
        Self {
            key: &value.key,
            label: &value.label,
            description: &value.description,
            color: &value.color,
            command: &value.command,
            instructions_filename: &value.instructions_filename,
            envs: &value.envs,
            isolated_home: &value.isolated_home,
            config_seed: &value.config_seed,
            removable: &value.removable,
            update_commands: &value.update_commands,
            auto_update: &value.auto_update,
            idle_burst: &value.idle_burst,
            install_commands: &value.install_commands,
        }
    }
}

pub(crate) fn managed_content_sha256(definitions: &[CodingAgentDefinition]) -> String {
    let historical: Vec<_> = definitions.iter().map(LegacyDefinitionHash::from).collect();
    match serde_json::to_vec(&historical) {
        Ok(bytes) => sha256_hex(&bytes),
        Err(error) => {
            log::error!(
                "[coding-agents] failed to serialize catalog definitions for the managed revision \
                    hash ({error})"
            );
            String::new()
        }
    }
}

/// The shipped definitions the support table in force allows, in shipped order.
pub(crate) fn supported_shipped_definitions() -> Vec<CodingAgentDefinition> {
    let table = active_builtin_agent_support();
    embedded_default_catalog()
        .agents
        .into_iter()
        .filter(|definition| is_supported_builtin(&definition.key, table))
        .collect()
}

/// The complete managed base bytes for `definitions`: pretty JSON, one trailing
/// LF, schemaVersion 1, the definitions, and the ownership marker whose
/// `revision` and `contentSha256` are the same content identity. No timestamp
/// is part of the content identity.
pub(crate) fn build_managed_base_bytes(definitions: &[CodingAgentDefinition]) -> Vec<u8> {
    build_managed_base_bytes_with_original(definitions, None)
}

pub(crate) fn original_catalog_command<'a>(
    original: Option<&'a serde_json::Value>,
    key: &str,
) -> Option<&'a serde_json::Value> {
    original?
        .get("agents")?
        .as_array()?
        .iter()
        .find(|row| row.get("key").and_then(serde_json::Value::as_str) == Some(key))?
        .get("command")
}

pub(crate) fn build_managed_base_bytes_with_original(
    definitions: &[CodingAgentDefinition],
    original: Option<&serde_json::Value>,
) -> Vec<u8> {
    let revision = managed_content_sha256(definitions);
    #[derive(Serialize)]
    struct DefinitionWire<'a> {
        #[serde(flatten)]
        definition: &'a CodingAgentDefinition,
        #[serde(skip_serializing_if = "Option::is_none")]
        command: Option<&'a serde_json::Value>,
    }
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct ManagedBaseWire<'a> {
        schema_version: u32,
        agents: Vec<DefinitionWire<'a>>,
        managed: ManagedCatalogMarker,
    }
    let root = ManagedBaseWire {
        schema_version: CATALOG_SCHEMA_VERSION,
        agents: definitions
            .iter()
            .map(|definition| DefinitionWire {
                definition,
                command: original_catalog_command(original, &definition.key),
            })
            .collect(),
        managed: ManagedCatalogMarker {
            owner: MANAGED_OWNER.to_string(),
            version: MANAGED_VERSION,
            revision: revision.clone(),
            content_sha256: revision,
        },
    };
    let mut bytes = match serde_json::to_vec_pretty(&root) {
        Ok(bytes) => bytes,
        Err(error) => {
            log::error!("[coding-agents] failed to serialize the managed catalog base ({error})");
            Vec::new()
        }
    };
    bytes.push(b'\n');
    bytes
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ManagedCatalogMarker {
    pub(crate) owner: String,
    pub(crate) version: u32,
    pub(crate) revision: String,
    pub(crate) content_sha256: String,
}

// ---------------------------------------------------------------------------
// Strict JSON: duplicate object members are rejected at every depth.
// ---------------------------------------------------------------------------

pub(crate) struct StrictValue(serde_json::Value);

impl<'de> Deserialize<'de> for StrictValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_any(StrictValueVisitor)
    }
}

pub(crate) struct StrictValueVisitor;

impl<'de> Visitor<'de> for StrictValueVisitor {
    type Value = StrictValue;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a JSON value whose objects have no duplicate members")
    }

    fn visit_bool<E>(self, value: bool) -> Result<Self::Value, E> {
        Ok(StrictValue(serde_json::Value::Bool(value)))
    }

    fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E> {
        Ok(StrictValue(serde_json::Value::Number(value.into())))
    }

    fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E> {
        Ok(StrictValue(serde_json::Value::Number(value.into())))
    }

    fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E> {
        Ok(StrictValue(
            serde_json::Number::from_f64(value)
                .map(serde_json::Value::Number)
                .unwrap_or(serde_json::Value::Null),
        ))
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E> {
        Ok(StrictValue(serde_json::Value::String(value.to_string())))
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E> {
        Ok(StrictValue(serde_json::Value::String(value)))
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E> {
        Ok(StrictValue(serde_json::Value::Null))
    }

    fn visit_none<E>(self) -> Result<Self::Value, E> {
        Ok(StrictValue(serde_json::Value::Null))
    }

    fn visit_some<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        StrictValue::deserialize(deserializer)
    }

    fn visit_seq<A>(self, mut access: A) -> Result<Self::Value, A::Error>
    where
        A: serde::de::SeqAccess<'de>,
    {
        let mut items = Vec::new();
        while let Some(item) = access.next_element::<StrictValue>()? {
            items.push(item.0);
        }
        Ok(StrictValue(serde_json::Value::Array(items)))
    }

    fn visit_map<A>(self, mut access: A) -> Result<Self::Value, A::Error>
    where
        A: serde::de::MapAccess<'de>,
    {
        let mut members = serde_json::Map::new();
        while let Some(key) = access.next_key::<String>()? {
            if members.contains_key(&key) {
                return Err(de::Error::custom(format!(
                    "duplicate JSON object member '{key}'"
                )));
            }
            let value = access.next_value::<StrictValue>()?;
            members.insert(key, value.0);
        }
        Ok(StrictValue(serde_json::Value::Object(members)))
    }
}

/// Parse JSON while rejecting duplicate object members at every depth. A raw
/// `Value` parse cannot detect those, and a duplicate member makes the document
/// ambiguous, which is exactly the case a destructive migration must refuse.
pub(crate) fn parse_strict_json(bytes: &[u8]) -> Result<serde_json::Value, String> {
    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    let value = StrictValue::deserialize(&mut deserializer).map_err(|error| error.to_string())?;
    deserializer
        .end()
        .map_err(|error| format!("trailing data after the JSON document ({error})"))?;
    Ok(value.0)
}

pub(crate) fn expect_json_object<'a>(
    value: &'a serde_json::Value,
    context: &str,
) -> Result<&'a serde_json::Map<String, serde_json::Value>, String> {
    value
        .as_object()
        .ok_or_else(|| format!("{context} must be a JSON object"))
}

pub(crate) fn expect_json_string<'a>(
    object: &'a serde_json::Map<String, serde_json::Value>,
    field: &str,
    context: &str,
) -> Result<&'a str, String> {
    match object.get(field) {
        Some(serde_json::Value::String(value)) => Ok(value),
        Some(_) => Err(format!("{context}: '{field}' must be a string")),
        None => Err(format!("{context}: '{field}' is required")),
    }
}

pub(crate) fn reject_unknown_json_fields(
    object: &serde_json::Map<String, serde_json::Value>,
    allowed: &[&str],
    context: &str,
) -> Result<(), String> {
    if let Some(names) = unknown_field_names(object.keys(), allowed) {
        return Err(format!("{context}: unknown field(s) ({names})"));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// The strict local layer schema and its composition.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub(crate) struct LocalFieldPatch {
    pub(crate) label: Option<String>,
    pub(crate) description: Option<String>,
    pub(crate) color: Option<String>,
    pub(crate) command: Option<String>,
    /// `None` = absent; `Some(None)` = explicit `null`.
    pub(crate) instructions_filename: Option<Option<String>>,
    pub(crate) envs: Option<Vec<CodingAgentEnv>>,
    pub(crate) isolated_home: Option<bool>,
    /// `None` = absent; `Some(None)` = explicit `null`; `Some(Some(_))` = object.
    pub(crate) config_seed: Option<Option<ConfigSeedPatch>>,
    pub(crate) removable: Option<bool>,
    pub(crate) update_commands: Option<Vec<String>>,
    pub(crate) auto_update: Option<bool>,
    /// `None` = absent; `Some(None)` = explicit `null`; `Some(Some(_))` = object.
    pub(crate) idle_burst: Option<Option<IdleBurstConfig>>,
    /// `None` = absent; `Some(None)` = explicit `null`; `Some(Some(_))` = object.
    pub(crate) install_commands: Option<Option<InstallCommandsPatch>>,
}
