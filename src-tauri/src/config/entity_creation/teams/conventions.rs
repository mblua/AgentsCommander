use crate::filesystem::projects::entities::publication;
use std::path::Path;

pub(crate) fn ensure_empty_if_absent(team_dir: &Path) -> Result<(), String> {
    let path = team_dir.join("conventions.md");
    if !publication::team_conventions_present(&path) {
        publication::write_empty_team_conventions(&path)
            .map_err(|e| format!("Failed to write conventions.md: {}", e))?;
    }
    Ok(())
}

pub(crate) fn write_empty_for_new_team(team_dir: &Path) -> Result<(), String> {
    publication::write_empty_team_conventions(&team_dir.join("conventions.md"))
        .map_err(|e| format!("Failed to write conventions.md: {}", e))
}

pub(crate) fn write_owned_empty(
    owned: &mut crate::filesystem::projects::entities::copies::OwnedCopyArtifacts,
) -> Result<(), String> {
    crate::filesystem::projects::entities::copies::write_owned_team_file(
        owned,
        Path::new("conventions.md"),
        &[],
    )
}
