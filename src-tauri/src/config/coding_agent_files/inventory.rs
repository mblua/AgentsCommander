//! The file inventory of a context, in display order.

use super::locator::{is_other_name, normalize_config_name, NAMED_PREFIX, NAMED_SUFFIX};
use super::{
    CatalogLayer, CodingAgentConfigFiles, ConfigContext, ConfigDirs, ConfigFileLocator,
    ConfigScope, EntryGroup, InventoryEntry,
};
use crate::filesystem::settings_sources;
use std::path::{Path, PathBuf};

const LOW_LAYER_PREFIX: &str = "agents.10.";

fn name_of(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn layer_entry(
    scope: ConfigScope,
    layer: CatalogLayer,
    path: &Path,
    group: EntryGroup,
) -> InventoryEntry {
    InventoryEntry {
        locator: ConfigFileLocator::Layer { scope, layer },
        group,
        file_name: name_of(path),
        path: path.to_path_buf(),
    }
}

fn valid_name(file_name: &str) -> Option<&str> {
    let name = file_name
        .strip_prefix(NAMED_PREFIX)?
        .strip_suffix(NAMED_SUFFIX)?;
    (normalize_config_name(name).ok().as_deref() == Some(name)).then_some(name)
}

/// One group: layers not named `agents.10.*`, named files (alpha), then `agents.10.*` layers.
/// Appends the group's "other" files to `others`.
fn scope_group(
    dirs: &ConfigDirs,
    scope: ConfigScope,
    dir: &Path,
    group: EntryGroup,
    others: &mut Vec<(String, ConfigScope, PathBuf)>,
) -> Vec<InventoryEntry> {
    let layers: Vec<_> = dirs.layers.iter().filter(|(s, _, _)| *s == scope).collect();
    let is_low = |path: &Path| name_of(path).starts_with(LOW_LAYER_PREFIX);
    let mut entries: Vec<InventoryEntry> = layers
        .iter()
        .filter(|(_, _, path)| !is_low(path))
        .map(|(s, l, path)| layer_entry(*s, *l, path, group))
        .collect();
    let names = settings_sources::list_file_names(dir).unwrap_or_default();
    for file_name in &names {
        if let Some(name) = valid_name(file_name) {
            entries.push(InventoryEntry {
                locator: ConfigFileLocator::Named {
                    scope,
                    name: name.to_string(),
                },
                group,
                file_name: file_name.clone(),
                path: dir.join(file_name),
            });
        } else if is_other_name(file_name, dirs) {
            others.push((file_name.clone(), scope, dir.join(file_name)));
        }
    }
    entries.extend(
        layers
            .iter()
            .filter(|(_, _, path)| is_low(path))
            .map(|(s, l, path)| layer_entry(*s, *l, path, group)),
    );
    entries
}

/// Every file the context can select, in display order (decision 5).
pub(crate) fn list_entries(
    sel: &CodingAgentConfigFiles,
    dirs: &ConfigDirs,
    ctx: &ConfigContext,
) -> Vec<InventoryEntry> {
    let mut others = Vec::new();
    let mut entries = vec![InventoryEntry {
        locator: ConfigFileLocator::RegisteredInstance {},
        group: EntryGroup::Instance,
        file_name: name_of(&dirs.registered_path),
        path: dirs.registered_path.clone(),
    }];
    entries.extend(scope_group(
        dirs,
        ConfigScope::Instance,
        &dirs.instance_dir,
        EntryGroup::Instance,
        &mut others,
    ));
    if let (ConfigContext::Project(key), Some((dir_key, dir))) = (ctx, &dirs.project) {
        if key == dir_key {
            entries.extend(scope_group(
                dirs,
                ConfigScope::Project,
                dir,
                EntryGroup::Project,
                &mut others,
            ));
        }
    }
    entries.extend(sel.loaded.iter().map(|path| InventoryEntry {
        locator: ConfigFileLocator::Loaded { path: path.clone() },
        group: EntryGroup::Loaded,
        file_name: name_of(Path::new(path)),
        path: PathBuf::from(path),
    }));
    others.sort_by(|a, b| a.0.cmp(&b.0));
    entries.extend(
        others
            .into_iter()
            .map(|(file_name, scope, path)| InventoryEntry {
                locator: ConfigFileLocator::Other {
                    scope,
                    file_name: file_name.clone(),
                },
                group: EntryGroup::Other,
                file_name,
                path,
            }),
    );
    entries
}
