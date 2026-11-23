//! The name rule, the context key normalizer, context lookup and the single locator check.

use super::{
    CodingAgentConfigFiles, ConfigContext, ConfigDirs, ConfigFileError, ConfigFileLocator,
    ConfigScope,
};
use crate::path_utils;
use std::path::{Path, PathBuf, MAIN_SEPARATOR};

pub(crate) const NAMED_PREFIX: &str = "ac.coding-agents-config.";
pub(crate) const NAMED_SUFFIX: &str = ".json";
const NAME_MAX: usize = 48;

fn locator_invalid() -> ConfigFileError {
    ConfigFileError::new("configLocatorInvalid", "This file reference is not valid.")
}

/// Trims and lowercases `raw`; accepts 1 to 48 of `[a-z0-9_-]`, first `[a-z0-9]`.
pub(crate) fn normalize_config_name(raw: &str) -> Result<String, ConfigFileError> {
    let name = raw.trim().to_lowercase();
    let allowed = |c: char| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_';
    let first_ok = name
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit());
    if first_ok && name.chars().count() <= NAME_MAX && name.chars().all(allowed) {
        Ok(name)
    } else {
        Err(ConfigFileError::new(
            "configNameInvalid",
            "Use 1 to 48 letters, numbers, \"-\" or \"_\".",
        ))
    }
}

/// The one normalizer for stored context keys and lookups.
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

fn is_inside(key: &str, root: &str) -> bool {
    match key.strip_prefix(root) {
        Some(rest) => {
            rest.is_empty() || root.ends_with(MAIN_SEPARATOR) || rest.starts_with(MAIN_SEPARATOR)
        }
        None => false,
    }
}

/// The project whose key is the longest prefix of `cwd` at a component boundary.
pub(crate) fn context_for_cwd(project_paths: &[String], cwd: &str) -> ConfigContext {
    let cwd_key = context_key(cwd);
    project_paths
        .iter()
        .map(|path| context_key(path))
        .filter(|key| is_inside(&cwd_key, key))
        .max_by_key(String::len)
        .map_or(ConfigContext::Instance, ConfigContext::Project)
}

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

fn file_name_of(path: &Path) -> Option<&str> {
    path.file_name().and_then(|name| name.to_str())
}

/// Decision 5: the names of the "other" group, given the dirs' layer and 30 file names.
pub(crate) fn is_other_name(file_name: &str, dirs: &ConfigDirs) -> bool {
    if file_name == "agents.local.FUERA.json" {
        return false;
    }
    if file_name_of(&dirs.registered_path) == Some(file_name) {
        return false;
    }
    if dirs
        .layers
        .iter()
        .any(|(_, _, path)| file_name_of(path) == Some(file_name))
    {
        return false;
    }
    let named_invalid = file_name
        .strip_prefix(NAMED_PREFIX)
        .and_then(|rest| rest.strip_suffix(NAMED_SUFFIX))
        .is_some_and(|name| normalize_config_name(name).ok().as_deref() != Some(name));
    let instance_like = file_name
        .strip_prefix("agents.")
        .and_then(|rest| rest.strip_suffix(".json"))
        .is_some_and(|middle| middle.contains(".instance"));
    named_invalid || instance_like
}

fn is_bare_name(file_name: &str) -> bool {
    !file_name.is_empty()
        && file_name != "."
        && file_name != ".."
        && !file_name.contains(['/', '\\', ':', '\0'])
}

fn scope_dir<'a>(
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

fn join_checked(dir: &Path, file_name: &str) -> Result<PathBuf, ConfigFileError> {
    let path = dir.join(file_name);
    if path.parent() == Some(dir) {
        Ok(path)
    } else {
        Err(locator_invalid())
    }
}

/// The single locator check; every reader and writer calls it first.
pub(crate) fn resolve_locator(
    sel: &CodingAgentConfigFiles,
    dirs: &ConfigDirs,
    ctx: &ConfigContext,
    loc: &ConfigFileLocator,
) -> Result<PathBuf, ConfigFileError> {
    match loc {
        ConfigFileLocator::RegisteredInstance {} => Ok(dirs.registered_path.clone()),
        ConfigFileLocator::Layer { scope, layer } => {
            if *scope == ConfigScope::Project {
                scope_dir(dirs, ctx, *scope)?;
            }
            dirs.layers
                .iter()
                .find(|(s, l, _)| s == scope && l == layer)
                .map(|(_, _, path)| path.clone())
                .ok_or_else(locator_invalid)
        }
        ConfigFileLocator::Named { scope, name } => {
            if normalize_config_name(name).ok().as_deref() != Some(name.as_str()) {
                return Err(locator_invalid());
            }
            let dir = scope_dir(dirs, ctx, *scope)?;
            join_checked(dir, &format!("{NAMED_PREFIX}{name}{NAMED_SUFFIX}"))
        }
        ConfigFileLocator::Other { scope, file_name } => {
            if !is_bare_name(file_name) || !is_other_name(file_name, dirs) {
                return Err(locator_invalid());
            }
            let dir = scope_dir(dirs, ctx, *scope)?;
            join_checked(dir, file_name)
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
