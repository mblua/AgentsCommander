use serde::Deserialize;
use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BrowserProjectCommand {
    CheckProjectPath,
    DiscoverProject,
    GetProjectFileDifference,
    GetProjectGroups,
    UpdateProjectGroups,
    OpenProject,
    RemoveProject,
    ArchiveProject,
    UnarchiveProject,
    ListArchivedProjects,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WebCommandRoute {
    BrowserProject(BrowserProjectCommand),
    Other,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ThemeChangedPayload {
    light: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ResourceMonitorAttachPayload {}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OpenSettingsPayload {
    #[serde(default)]
    section: Option<String>,
}

pub(crate) fn validate_client_broadcast(event: &str, payload: &Value) -> Result<(), String> {
    if !payload.is_object() {
        return Err(format!("Invalid {event} payload: expected object"));
    }
    match event {
        "theme_changed" => {
            let decoded: ThemeChangedPayload = serde_json::from_value(payload.clone())
                .map_err(|error| format!("Invalid theme_changed payload: {error}"))?;
            let _ = decoded.light;
        }
        "resource_monitor_attach" => {
            serde_json::from_value::<ResourceMonitorAttachPayload>(payload.clone())
                .map_err(|error| format!("Invalid resource_monitor_attach payload: {error}"))?;
        }
        "open_settings" => {
            let decoded: OpenSettingsPayload = serde_json::from_value(payload.clone())
                .map_err(|error| format!("Invalid open_settings payload: {error}"))?;
            let _ = decoded.section;
        }
        _ => return Err(format!("Client broadcast event is not allowed: {event}")),
    }
    Ok(())
}

pub(crate) fn route_web_command(cmd: &str) -> WebCommandRoute {
    match cmd {
        "check_project_path" => {
            WebCommandRoute::BrowserProject(BrowserProjectCommand::CheckProjectPath)
        }
        "discover_project" => {
            WebCommandRoute::BrowserProject(BrowserProjectCommand::DiscoverProject)
        }
        "get_project_file_difference" => {
            WebCommandRoute::BrowserProject(BrowserProjectCommand::GetProjectFileDifference)
        }
        "get_project_groups" => {
            WebCommandRoute::BrowserProject(BrowserProjectCommand::GetProjectGroups)
        }
        "update_project_groups" => {
            WebCommandRoute::BrowserProject(BrowserProjectCommand::UpdateProjectGroups)
        }
        "open_project" => WebCommandRoute::BrowserProject(BrowserProjectCommand::OpenProject),
        "remove_project" => WebCommandRoute::BrowserProject(BrowserProjectCommand::RemoveProject),
        "archive_project" => WebCommandRoute::BrowserProject(BrowserProjectCommand::ArchiveProject),
        "unarchive_project" => {
            WebCommandRoute::BrowserProject(BrowserProjectCommand::UnarchiveProject)
        }
        "list_archived_projects" => {
            WebCommandRoute::BrowserProject(BrowserProjectCommand::ListArchivedProjects)
        }
        _ => WebCommandRoute::Other,
    }
}
