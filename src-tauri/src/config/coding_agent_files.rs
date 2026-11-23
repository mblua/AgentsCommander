#![allow(dead_code, unused_imports, reason = "I4845 staged; removed in P14")]
//! #4845 coding-agent config files: the acyclic core with no settings types.
//! Includes file locators and their one check, contexts and their key, the name rule,
//! the file inventory of a context and the persisted per-context selection.
//! Does not include the settings glue, IPC or any file write.

mod inventory;
mod locator;
#[cfg(test)]
mod tests;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;

pub(crate) use inventory::list_entries;
pub(crate) use locator::{
    context_for_cwd, context_for_root, context_key, normalize_config_name, resolve_locator,
};

/// Stored key of the `Instance` context.
pub(crate) const INSTANCE_CONTEXT_KEY: &str = "instance";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum ConfigScope {
    Instance,
    Project,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(
    clippy::enum_variant_names,
    reason = "I4845 wire names catalogBase/Project/Personal"
)]
pub(crate) enum CatalogLayer {
    CatalogBase,
    CatalogProject,
    CatalogPersonal,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub(crate) enum ConfigFileLocator {
    RegisteredInstance {},
    Layer {
        scope: ConfigScope,
        layer: CatalogLayer,
    },
    Named {
        scope: ConfigScope,
        name: String,
    },
    Other {
        scope: ConfigScope,
        #[serde(rename = "fileName")]
        file_name: String,
    },
    Loaded {
        path: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ConfigContext {
    Instance,
    /// The project's `context_key`.
    Project(String),
}

impl ConfigContext {
    /// Key of this context in `active_by_context`.
    pub(crate) fn key(&self) -> &str {
        match self {
            ConfigContext::Instance => INSTANCE_CONTEXT_KEY,
            ConfigContext::Project(key) => key,
        }
    }
}

/// The `settings.json` value `codingAgentConfigFiles`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CodingAgentConfigFiles {
    pub version: u32,
    pub active_by_context: BTreeMap<String, ConfigFileLocator>,
    /// Canonical paths of the loaded external files.
    pub loaded: Vec<String>,
}

impl Default for CodingAgentConfigFiles {
    fn default() -> Self {
        Self {
            version: 1,
            active_by_context: BTreeMap::new(),
            loaded: Vec::new(),
        }
    }
}

/// Directories of a context; built by the settings glue (P17), plain data here.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ConfigDirs {
    /// The 30 file.
    pub registered_path: PathBuf,
    /// `<config dir>/coding-agents`.
    pub instance_dir: PathBuf,
    /// `(context_key, <root>/.ac/coding-agents)`.
    pub project: Option<(String, PathBuf)>,
    /// Existing catalog paths.
    pub layers: Vec<(ConfigScope, CatalogLayer, PathBuf)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ConfigFileError {
    pub code: &'static str,
    pub message: String,
}

impl ConfigFileError {
    pub(crate) fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl fmt::Display for ConfigFileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {} Nothing was changed.", self.code, self.message)
    }
}

impl std::error::Error for ConfigFileError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EntryGroup {
    Instance,
    Project,
    Loaded,
    Other,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct InventoryEntry {
    pub locator: ConfigFileLocator,
    pub group: EntryGroup,
    pub file_name: String,
    pub path: PathBuf,
}

/// Active locator of `ctx`; an absent selection or key is the 30 file (R4).
pub(crate) fn active_locator(
    sel: Option<&CodingAgentConfigFiles>,
    ctx: &ConfigContext,
) -> ConfigFileLocator {
    sel.and_then(|sel| sel.active_by_context.get(ctx.key()))
        .cloned()
        .unwrap_or(ConfigFileLocator::RegisteredInstance {})
}

/// Copy of `sel` with `loc` active for `ctx`.
pub(crate) fn with_active(
    sel: Option<&CodingAgentConfigFiles>,
    ctx: &ConfigContext,
    loc: ConfigFileLocator,
) -> CodingAgentConfigFiles {
    let mut next = sel.cloned().unwrap_or_default();
    next.active_by_context.insert(ctx.key().to_string(), loc);
    next
}

/// Copy of `sel` with `path` remembered as loaded; no duplicates, order kept.
pub(crate) fn with_loaded(
    sel: Option<&CodingAgentConfigFiles>,
    path: &str,
) -> CodingAgentConfigFiles {
    let mut next = sel.cloned().unwrap_or_default();
    if !next.loaded.iter().any(|known| known == path) {
        next.loaded.push(path.to_string());
    }
    next
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawSelection {
    version: u32,
    #[serde(default)]
    active_by_context: BTreeMap<String, Value>,
    #[serde(default)]
    loaded: Vec<String>,
}

/// Lenient reader of `codingAgentConfigFiles`: a bad value reads as `None`; inside a
/// good value, entries whose locator does not decode are dropped.
pub(crate) fn lenient_selection<'de, D>(
    deserializer: D,
) -> Result<Option<CodingAgentConfigFiles>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw = Option::<Value>::deserialize(deserializer)?;
    let Some(raw) = raw.and_then(|value| serde_json::from_value::<RawSelection>(value).ok()) else {
        return Ok(None);
    };
    let active_by_context = raw
        .active_by_context
        .into_iter()
        .filter_map(|(key, value)| serde_json::from_value(value).ok().map(|loc| (key, loc)))
        .collect();
    Ok(Some(CodingAgentConfigFiles {
        version: raw.version,
        active_by_context,
        loaded: raw.loaded,
    }))
}
