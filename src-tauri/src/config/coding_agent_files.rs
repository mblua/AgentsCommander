//! #4845 coding-agent config files: the acyclic core. Types for file locators, contexts,
//! the persisted per-context selection and the file inventory, plus the pure selection
//! helpers. Names no settings type; the settings glue lives elsewhere (P17).

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;

pub(crate) mod inventory;
pub(crate) mod last_used;
pub(crate) mod launch_chain;
pub(crate) mod launched;
pub(crate) mod locator;
#[cfg(test)]
mod tests;

/// Where a config file lives: the instance config dir or the project's `.ac` dir.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum ConfigScope {
    Instance,
    Project,
}

/// A catalog layer file of the agents catalog.
#[allow(
    clippy::enum_variant_names,
    reason = "I4845 wire names catalogBase/Project/Personal"
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum CatalogLayer {
    CatalogBase,
    CatalogProject,
    CatalogPersonal,
}

/// A reference to one config file. Checked only by `locator::resolve_locator`.
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

pub(crate) const INSTANCE_CONTEXT_KEY: &str = "instance";

/// The context a selection belongs to: the instance, or one project by its key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ConfigContext {
    Instance,
    Project(String),
}

impl ConfigContext {
    pub(crate) fn key(&self) -> &str {
        match self {
            Self::Instance => INSTANCE_CONTEXT_KEY,
            Self::Project(key) => key,
        }
    }
}

/// The `settings.json` value: the active file per context and the loaded files.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CodingAgentConfigFiles {
    pub version: u32,
    pub active_by_context: BTreeMap<String, ConfigFileLocator>,
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

/// The folders and files a locator can point at. Built by the settings glue.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ConfigDirs {
    pub registered_path: PathBuf,
    pub instance_dir: PathBuf,
    pub project: Option<(String, PathBuf)>,
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

/// The active file of `ctx`; an absent selection or key is the registered instance file.
pub(crate) fn active_locator(
    sel: Option<&CodingAgentConfigFiles>,
    ctx: &ConfigContext,
) -> ConfigFileLocator {
    sel.and_then(|s| s.active_by_context.get(ctx.key()).cloned())
        .unwrap_or(ConfigFileLocator::RegisteredInstance {})
}

pub(crate) fn with_active(
    sel: Option<&CodingAgentConfigFiles>,
    ctx: &ConfigContext,
    loc: ConfigFileLocator,
) -> CodingAgentConfigFiles {
    let mut next = sel.cloned().unwrap_or_default();
    next.active_by_context.insert(ctx.key().to_string(), loc);
    next
}

/// Adds `path` to the loaded list once, keeping the existing order.
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
    active_by_context: BTreeMap<String, Value>,
    loaded: Vec<String>,
}

/// Serde reader for the settings field: any bad value reads as `None`, and inside a good
/// value every entry whose locator does not decode is dropped.
pub(crate) fn lenient_selection<'de, D>(
    deserializer: D,
) -> Result<Option<CodingAgentConfigFiles>, D::Error>
where
    D: Deserializer<'de>,
{
    let raw = Option::<Value>::deserialize(deserializer)?;
    let Some(raw) = raw.and_then(|value| serde_json::from_value::<RawSelection>(value).ok()) else {
        return Ok(None);
    };
    let active_by_context = raw
        .active_by_context
        .into_iter()
        .filter_map(|(key, value)| Some((key, serde_json::from_value(value).ok()?)))
        .collect();
    Ok(Some(CodingAgentConfigFiles {
        version: raw.version,
        active_by_context,
        loaded: raw.loaded,
    }))
}
