//! Project resolution for the `workgroup` CLI verbs. Maps a `--project` reference to its
//! path using the CLI settings and finds the project's existing `.ac` root. It does not
//! create the `.ac` root; resolution rules live in `config::projects` and `config::ac_root`.

use std::path::{Path, PathBuf};

use crate::config::ac_root::existing_ac_root;
use crate::config::projects::resolve_project_reference;

pub(crate) fn resolve_cli_project(project: &str) -> Result<PathBuf, String> {
    let settings = crate::config::settings::load_settings_for_cli();
    let resolved =
        resolve_project_reference(&settings.project_paths, project).map_err(|e| e.to_string())?;
    Ok(resolved.path)
}

pub(crate) fn resolve_cli_ac_root(project_path: &Path) -> Result<PathBuf, String> {
    existing_ac_root(project_path).ok_or_else(|| {
        format!(
            "Project AC Root not found in {} (.ac)",
            project_path.display()
        )
    })
}
