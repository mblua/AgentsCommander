//! #4845 file inventory of a context: every coding-agent file the bar lists, in
//! display order. Includes the name filters. Does not read file contents.

use super::locator::{is_other_pattern, is_valid_name, named_part, scope_dir};
use super::{
    CatalogLayer, CodingAgentConfigFiles, ConfigContext, ConfigDirs, ConfigFileLocator,
    ConfigScope, EntryGroup, InventoryEntry,
};
use crate::filesystem::settings_sources;
use std::path::{Path, PathBuf};

fn file_name_of(path: &Path) -> String {
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
        file_name: file_name_of(path),
        path: path.to_path_buf(),
    }
}

/// One scope's group: non-base layers (50/40), named (alpha), then the 10 layer.
fn scope_group(
    dirs: &ConfigDirs,
    scope: ConfigScope,
    dir: &Path,
    names: &[String],
    group: EntryGroup,
) -> Vec<InventoryEntry> {
    let layers = dirs.layers.iter().filter(|(s, _, _)| *s == scope);
    let mut entries: Vec<InventoryEntry> = layers
        .clone()
        .filter(|(_, layer, _)| *layer != CatalogLayer::CatalogBase)
        .map(|(_, layer, path)| layer_entry(scope, *layer, path, group))
        .collect();
    for file_name in names {
        let Some(name) = named_part(file_name).filter(|name| is_valid_name(name)) else {
            continue;
        };
        entries.push(InventoryEntry {
            locator: ConfigFileLocator::Named {
                scope,
                name: name.to_string(),
            },
            group,
            file_name: file_name.clone(),
            path: dir.join(file_name),
        });
    }
    entries.extend(
        layers
            .filter(|(_, layer, _)| *layer == CatalogLayer::CatalogBase)
            .map(|(_, layer, path)| layer_entry(scope, *layer, path, group)),
    );
    entries
}

fn other_entries(
    dirs: &ConfigDirs,
    scope: ConfigScope,
    dir: &Path,
    names: &[String],
) -> Vec<InventoryEntry> {
    let registered = file_name_of(&dirs.registered_path);
    let is_layer = |name: &str| {
        dirs.layers
            .iter()
            .any(|(_, _, path)| file_name_of(path) == name)
    };
    names
        .iter()
        .filter(|name| is_other_pattern(name) && **name != registered && !is_layer(name))
        .map(|file_name| InventoryEntry {
            locator: ConfigFileLocator::Other {
                scope,
                file_name: file_name.clone(),
            },
            group: EntryGroup::Other,
            file_name: file_name.clone(),
            path: dir.join(file_name),
        })
        .collect()
}

fn names_in(dir: &Path) -> Vec<String> {
    settings_sources::list_file_names(dir).unwrap_or_default()
}

/// Every file of `ctx`, in display order: instance group, project group (Project ctx
/// only), loaded files, other files (alpha).
pub(crate) fn list_entries(
    sel: &CodingAgentConfigFiles,
    dirs: &ConfigDirs,
    ctx: &ConfigContext,
) -> Vec<InventoryEntry> {
    let mut entries = vec![InventoryEntry {
        locator: ConfigFileLocator::RegisteredInstance {},
        group: EntryGroup::Instance,
        file_name: file_name_of(&dirs.registered_path),
        path: dirs.registered_path.clone(),
    }];
    let instance_names = names_in(&dirs.instance_dir);
    let mut others = other_entries(
        dirs,
        ConfigScope::Instance,
        &dirs.instance_dir,
        &instance_names,
    );
    entries.extend(scope_group(
        dirs,
        ConfigScope::Instance,
        &dirs.instance_dir,
        &instance_names,
        EntryGroup::Instance,
    ));
    if let Ok(project_dir) = scope_dir(dirs, ctx, ConfigScope::Project) {
        let names = names_in(project_dir);
        others.extend(other_entries(
            dirs,
            ConfigScope::Project,
            project_dir,
            &names,
        ));
        entries.extend(scope_group(
            dirs,
            ConfigScope::Project,
            project_dir,
            &names,
            EntryGroup::Project,
        ));
    }
    entries.extend(sel.loaded.iter().map(|path| InventoryEntry {
        locator: ConfigFileLocator::Loaded { path: path.clone() },
        group: EntryGroup::Loaded,
        file_name: file_name_of(Path::new(path)),
        path: PathBuf::from(path),
    }));
    others.sort_by(|a, b| a.file_name.cmp(&b.file_name));
    entries.extend(others);
    entries
}
