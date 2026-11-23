//! #4845 locator rules: the name rule, the context key, the context of a path and
//! the single locator check. Includes file-name patterns. Does not touch the disk.

use super::{
    CatalogLayer, CodingAgentConfigFiles, ConfigContext, ConfigDirs, ConfigFileError,
    ConfigFileLocator, ConfigScope,
};
use crate::path_utils;
use std::path::{Path, PathBuf, MAIN_SEPARATOR};

const NAMED_PREFIX: &str = "ac.coding-agents-config.";
const JSON_SUFFIX: &str = ".json";
const MAX_NAME_CHARS: usize = 48;

/// Name rule (D4): trim, lowercase; 1..=48 of `[a-z0-9_-]`, first `[a-z0-9]`.
pub(crate) fn normalize_config_name(raw: &str) -> Result<String, ConfigFileError> {
    let name = raw.trim().to_lowercase();
    let allowed = |c: char| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_';
    let first_ok = name
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit());
    if first_ok && name.chars().count() <= MAX_NAME_CHARS && name.chars().all(allowed) {
        Ok(name)
    } else {
        Err(ConfigFileError::new(
            "configNameInvalid",
            r#"Use 1 to 48 letters, numbers, "-" or "_"."#,
        ))
    }
}

/// The one key normalizer for stored keys and lookups.
pub(crate) fn context_key(root: &str) -> String {
    let plain = path_utils::normalize_windows_verbatim_path(root);
    let unified: String = plain
        .chars()
        .map(|c| {
            if c == '/' || c == '\\' {
                MAIN_SEPARATOR
            } else {
                c
            }
        })
        .collect();
    let trimmed = unified.trim_end_matches(MAIN_SEPARATOR);
    let key = if trimmed.is_empty() && !unified.is_empty() {
        MAIN_SEPARATOR.to_string()
    } else {
        trimmed.to_string()
    };
    if cfg!(windows) {
        key.to_ascii_lowercase()
    } else {
        key
    }
}

fn contains_key(outer: &str, inner: &str) -> bool {
    match inner.strip_prefix(outer) {
        Some(rest) => {
            rest.is_empty() || rest.starts_with(MAIN_SEPARATOR) || outer.ends_with(MAIN_SEPARATOR)
        }
        None => false,
    }
}

/// The project whose key is the longest component-boundary prefix of `cwd`.
pub(crate) fn context_for_cwd(project_paths: &[String], cwd: &str) -> ConfigContext {
    let cwd_key = context_key(cwd);
    project_paths
        .iter()
        .map(|root| context_key(root))
        .filter(|key| contains_key(key, &cwd_key))
        .max_by_key(String::len)
        .map_or(ConfigContext::Instance, ConfigContext::Project)
}

/// `None` = Instance; otherwise `root` must be one of `project_paths` (by key).
pub(crate) fn context_for_root(
    project_paths: &[String],
    root: Option<&str>,
) -> Result<ConfigContext, ConfigFileError> {
    let Some(root) = root else {
        return Ok(ConfigContext::Instance);
    };
    let key = context_key(root);
    if project_paths.iter().any(|path| context_key(path) == key) {
        Ok(ConfigContext::Project(key))
    } else {
        Err(ConfigFileError::new(
            "configContextUnknown",
            "This folder is not one of the known projects.",
        ))
    }
}

/// `<n>` of `ac.coding-agents-config.<n>.json`, valid or not.
pub(super) fn named_part(file_name: &str) -> Option<&str> {
    file_name
        .strip_prefix(NAMED_PREFIX)?
        .strip_suffix(JSON_SUFFIX)
}

pub(super) fn is_valid_name(name: &str) -> bool {
    normalize_config_name(name).is_ok_and(|normal| normal == name)
}

pub(super) fn named_file_name(name: &str) -> String {
    format!("{NAMED_PREFIX}{name}{JSON_SUFFIX}")
}

/// Decision 5 "other" patterns: a config name with an invalid `<n>`, or
/// `agents.*.instance*.json`.
pub(super) fn is_other_pattern(file_name: &str) -> bool {
    if let Some(name) = named_part(file_name) {
        return !is_valid_name(name);
    }
    file_name
        .strip_prefix("agents.")
        .and_then(|rest| rest.strip_suffix(JSON_SUFFIX))
        .is_some_and(|middle| middle.contains(".instance"))
}

fn is_bare_name(file_name: &str) -> bool {
    !file_name.is_empty()
        && file_name != "."
        && file_name != ".."
        && !file_name.contains(['/', '\\', ':', '\0'])
}

fn invalid() -> ConfigFileError {
    ConfigFileError::new("configLocatorInvalid", "This file reference is not valid.")
}

/// Directory of `scope` in `ctx`; project scope only for its own project.
pub(super) fn scope_dir<'a>(
    dirs: &'a ConfigDirs,
    ctx: &ConfigContext,
    scope: ConfigScope,
) -> Result<&'a Path, ConfigFileError> {
    match scope {
        ConfigScope::Instance => Ok(&dirs.instance_dir),
        ConfigScope::Project => match (ctx, &dirs.project) {
            (ConfigContext::Project(key), Some((dir_key, dir))) if key == dir_key => Ok(dir),
            _ => Err(ConfigFileError::new(
                "configFileWrongProject",
                "This file belongs to another project.",
            )),
        },
    }
}

fn in_dir(dir: &Path, file_name: &str) -> Result<PathBuf, ConfigFileError> {
    let path = dir.join(file_name);
    if path.parent() == Some(dir) {
        Ok(path)
    } else {
        Err(invalid())
    }
}

fn layer_path(
    dirs: &ConfigDirs,
    scope: ConfigScope,
    layer: CatalogLayer,
) -> Result<PathBuf, ConfigFileError> {
    dirs.layers
        .iter()
        .find(|(s, l, _)| *s == scope && *l == layer)
        .map(|(_, _, path)| path.clone())
        .ok_or_else(invalid)
}

/// The single locator check (D6); every reader and writer calls it first.
pub(crate) fn resolve_locator(
    sel: &CodingAgentConfigFiles,
    dirs: &ConfigDirs,
    ctx: &ConfigContext,
    loc: &ConfigFileLocator,
) -> Result<PathBuf, ConfigFileError> {
    match loc {
        ConfigFileLocator::RegisteredInstance {} => Ok(dirs.registered_path.clone()),
        ConfigFileLocator::Layer { scope, layer } => {
            scope_dir(dirs, ctx, *scope)?;
            layer_path(dirs, *scope, *layer)
        }
        ConfigFileLocator::Named { scope, name } => {
            let dir = scope_dir(dirs, ctx, *scope)?;
            if !is_valid_name(name) {
                return Err(invalid());
            }
            in_dir(dir, &named_file_name(name))
        }
        ConfigFileLocator::Other { scope, file_name } => {
            let dir = scope_dir(dirs, ctx, *scope)?;
            if !is_bare_name(file_name) || !is_other_pattern(file_name) {
                return Err(invalid());
            }
            in_dir(dir, file_name)
        }
        ConfigFileLocator::Loaded { path } => {
            if sel.loaded.iter().any(|known| known == path) {
                Ok(PathBuf::from(path))
            } else {
                Err(ConfigFileError::new(
                    "configFileUnknown",
                    "This file is not in the list of loaded files.",
                ))
            }
        }
    }
}
