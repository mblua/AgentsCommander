//! Tauri commands to list, read, save and remember coding-agent file sources.
//!
//! On each read or save it attaches selection tickets (default, every profile letter and `A`)
//! per agent, strips the root token from returned effective settings, and emits
//! `coding_agent_settings_updated` / `coding_agent_profiles_updated` after a real commit.
//! File IO, revisions and persistence live in `config::settings`; profile resolution lives in
//! `config::coding_agent_profiles`. It holds no state.

use std::collections::BTreeMap;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::config::coding_agent_files::locator::context_for_root;
use crate::config::coding_agent_files::{
    ConfigContext, ConfigFileError, ConfigFileLocator, ConfigScope, EntryGroup,
};
use crate::config::settings::agent::file::{
    agents_of, create_config_file, list_config_files, publish_config_selection_locked,
    read_config_file, register_loaded, save_config_file, set_active_config_file,
    ConfigFileSnapshot,
};
use crate::config::settings::SettingsState;
use crate::config::settings::{
    AgentConfig, AppSettings, CodingAgentProfilesConfig, SourceAvailability, SourceRevision,
};
use crate::web::broadcast::WsBroadcaster;
use serde::Serialize;
use std::path::{Path, PathBuf};

// E2432 P02: profile orchestration stays above settings' physical primitives.
pub(crate) fn prepare_agent_file_tickets(
    snapshot: &mut crate::config::settings::SelectedAgentFileSnapshot,
) -> Result<(), String> {
    use crate::config::coding_agent_profiles::{resolve_profile, ProfileResolutionRequest};
    use crate::config::settings::{
        agent_file_projection, configuration_identity, pair_identity, ConfigurationRef,
        SourceAvailability, SourceSelectionTicket,
    };
    if snapshot.availability != SourceAvailability::Available {
        return Ok(());
    }
    let settings = agent_file_projection(snapshot)?;
    for agent in &settings.agents {
        let mut tickets = BTreeMap::new();
        let mut requests = vec![("default".to_owned(), None)];
        if let Some(cells) = settings
            .coding_agent_profiles
            .profiles_by_agent
            .get(&agent.id)
        {
            requests.extend(
                cells
                    .keys()
                    .map(|letter| (letter.clone(), Some(letter.clone()))),
            );
        }
        if !requests.iter().any(|(key, _)| key == "A") {
            requests.push(("A".into(), Some("A".into())));
        }
        for (key, requested_profile) in requests {
            let resolution = resolve_profile(
                &settings,
                ProfileResolutionRequest {
                    coding_agent_id: &agent.id,
                    launch_path: None,
                    agent_matrix_name: None,
                    requested_profile: requested_profile.as_deref(),
                    requested_profile_authoritative: true,
                },
            );
            tickets.insert(
                key,
                SourceSelectionTicket {
                    configuration_ref: ConfigurationRef {
                        version: 1,
                        source: snapshot.source.clone(),
                        calculated_id: configuration_identity(&agent.command, &agent.envs),
                        requested_profile,
                        profile_calculated_id: Some(pair_identity(
                            &agent.command,
                            &agent.envs,
                            &resolution.cell,
                        )),
                        legacy_entry_hint: Some(agent.id.clone()),
                    },
                    expected_source_revision: snapshot.revision.clone(),
                    catalog_context_revision: snapshot.context_revision.clone(),
                },
            );
        }
        snapshot.tickets.insert(agent.id.clone(), tickets);
    }
    Ok(())
}

pub(crate) async fn list_agent_file_sources_inner(
    settings: &SettingsState,
) -> Result<crate::config::settings::AgentFileSources, String> {
    use crate::config::settings::{AgentFileReadReply, AgentFileReadRequest};
    match settings
        .read_agent_files(AgentFileReadRequest::List)
        .await?
    {
        AgentFileReadReply::List(sources) => Ok(sources),
        AgentFileReadReply::Read(_) => Err("sourceReplyMismatch".into()),
    }
}

pub(crate) async fn read_agent_file_source_inner(
    settings: &SettingsState,
    source: crate::config::settings::SourceRef,
) -> Result<crate::config::settings::SelectedAgentFileSnapshot, String> {
    use crate::config::settings::{AgentFileReadReply, AgentFileReadRequest};
    let mut snapshot = match settings
        .read_agent_files(AgentFileReadRequest::Read(source))
        .await?
    {
        AgentFileReadReply::Read(snapshot) => snapshot,
        AgentFileReadReply::List(_) => return Err("sourceReplyMismatch".into()),
    };
    prepare_agent_file_tickets(&mut snapshot)?;
    Ok(snapshot)
}

pub(crate) async fn save_agent_file_source_inner(
    settings: &SettingsState,
    source: crate::config::settings::SourceRef,
    expected_revision: crate::config::settings::SourceRevision,
    context_revision: String,
    edits: crate::config::settings::AgentFileEdits,
) -> Result<crate::config::settings::AgentFileSaveReply, String> {
    let mut result = crate::config::settings::persist_agent_file_edits(
        settings,
        source,
        expected_revision,
        context_revision,
        edits,
    )
    .await?;
    if let Some(snapshot) = result.snapshot.as_mut() {
        prepare_agent_file_tickets(snapshot)?;
    }
    // Match the existing settings snapshot boundary on both transports.
    if let Some(effective) = result.effective_settings.as_mut() {
        effective.root_token = None;
    }
    Ok(result)
}

pub(crate) async fn remember_agent_file_source_inner(
    settings: &SettingsState,
    menu: crate::config::settings::AgentFileSourceMenu,
    source: crate::config::settings::SourceRef,
) -> Result<crate::config::settings::AgentFileSourcePreferences, String> {
    use crate::config::settings::SettingsOwnerError;
    let report = settings
        .with_operation("remember_agent_file_source", move |current, guard| {
            crate::config::settings::remember_agent_file_locked(current, guard, menu, &source)
        })
        .await
        .map_err(|error| match error {
            SettingsOwnerError::Capture(_) => "sourceParticipantReadFailed: no files changed; \
                correct the participating file externally"
                .to_owned(),
            SettingsOwnerError::Compose(_) => "sourceParticipantInvalid: no files changed; \
                correct the participating file externally"
                .to_owned(),
            _ => "settingsOwnerUnavailable: no files changed".to_owned(),
        })?;
    finish_remember_agent_file_report(settings, report).await
}

pub(crate) async fn finish_remember_agent_file_report(
    settings: &SettingsState,
    report: crate::config::settings::SettingsOperationReport<
        crate::config::settings::AgentFileSourcePreferences,
    >,
) -> Result<crate::config::settings::AgentFileSourcePreferences, String> {
    use crate::config::settings::{OperationOutcome, SettingsAvailability};
    if report.outcome == OperationOutcome::PartialOrUnknown
        || (report.outcome == OperationOutcome::Committed
            && report.availability != SettingsAvailability::Ready)
    {
        return Err(
            "partialOrUnknown: preference publication may have occurred; reload actual settings"
                .into(),
        );
    }
    if report.outcome != OperationOutcome::Committed || report.value.is_none() {
        let reason = report.error.unwrap_or_default();
        let code = [
            "duplicateJsonMember",
            "invalidLegacyProfileOrphans",
            "staleSourceContext",
            "staleSettingsRevision",
            "staleProfileSource",
            "sourceAbsentOrForged",
            "invalidSourcePreferences",
        ]
        .into_iter()
        .find(|code| reason.contains(code))
        .unwrap_or("sourcePreferenceRejected");
        return Err(format!(
            "{code}: no files changed; correct the participating file externally or refresh the \
                selection"
        ));
    }
    let adopted = settings.current_snapshot().await.map_err(|_| {
        "partialOrUnknown: preferences were published but adopted settings are unavailable"
    })?;
    Ok(adopted.agent_file_source_by_menu.unwrap_or_default())
}

#[tauri::command]
pub async fn list_agent_file_sources(
    settings: State<'_, SettingsState>,
) -> Result<crate::config::settings::AgentFileSources, String> {
    list_agent_file_sources_inner(settings.inner()).await
}

#[tauri::command]
pub async fn read_agent_file_source(
    settings: State<'_, SettingsState>,
    source: crate::config::settings::SourceRef,
) -> Result<crate::config::settings::SelectedAgentFileSnapshot, String> {
    read_agent_file_source_inner(settings.inner(), source).await
}

#[tauri::command]
pub async fn save_agent_file_source(
    app: AppHandle,
    settings: State<'_, SettingsState>,
    source: crate::config::settings::SourceRef,
    expected_revision: crate::config::settings::SourceRevision,
    context_revision: String,
    edits: crate::config::settings::AgentFileEdits,
) -> Result<crate::config::settings::AgentFileSaveReply, String> {
    let reply = save_agent_file_source_inner(
        settings.inner(),
        source,
        expected_revision,
        context_revision,
        edits,
    )
    .await?;
    emit_agent_file_save_events(&app, &reply);
    Ok(reply)
}

pub(crate) fn emit_agent_file_save_events(
    app: &AppHandle,
    reply: &crate::config::settings::AgentFileSaveReply,
) {
    if reply.outcome != crate::config::settings::AgentFileSaveOutcome::Committed
        || reply.effective_settings.is_none()
    {
        return;
    }
    let catalog_only = reply
        .diagnostics
        .iter()
        .any(|diagnostic| diagnostic.code == "effectiveSettingsUnchanged");
    // Catalogue publication adopts no owner settings; its actual reply is the receipt.
    if catalog_only {
        return;
    }
    let payload = serde_json::json!({});
    for event in [
        "coding_agent_settings_updated",
        "coding_agent_profiles_updated",
    ] {
        let _ = app.emit(event, &payload);
        if let Some(broadcaster) = app.try_state::<WsBroadcaster>() {
            broadcaster.broadcast_event(event, &payload);
        }
    }
}

#[tauri::command]
pub async fn remember_agent_file_source(
    settings: State<'_, SettingsState>,
    menu: crate::config::settings::AgentFileSourceMenu,
    source: crate::config::settings::SourceRef,
) -> Result<crate::config::settings::AgentFileSourcePreferences, String> {
    remember_agent_file_source_inner(settings.inner(), menu, source).await
}

// #4845 P02: coding-agent config-file commands. Writes run in one settings operation.

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ConfigContextOption {
    pub root: Option<String>,
    pub name: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ConfigFileEntry {
    pub locator: ConfigFileLocator,
    pub group: &'static str,
    pub file_name: String,
    pub path: String,
    pub availability: SourceAvailability,
    pub read_only: bool,
    pub activatable: bool,
    pub agent_count: Option<usize>,
    pub diagnostic: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ConfigFileList {
    pub context: Option<String>,
    pub contexts: Vec<ConfigContextOption>,
    pub active: ConfigFileLocator,
    pub active_notice: Option<String>,
    pub entries: Vec<ConfigFileEntry>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ConfigFileSnapshotReply {
    pub locator: ConfigFileLocator,
    pub path: String,
    pub revision: SourceRevision,
    pub availability: SourceAvailability,
    pub agents: Vec<AgentConfig>,
    pub coding_agent_profiles: CodingAgentProfilesConfig,
    pub read_only: bool,
    pub diagnostic: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ScopedCodingAgents {
    pub agents: Vec<AgentConfig>,
    pub coding_agent_profiles: CodingAgentProfilesConfig,
    pub locator: ConfigFileLocator,
    pub file_name: String,
    pub notice: Option<String>,
}

fn snapshot_reply(snapshot: ConfigFileSnapshot) -> ConfigFileSnapshotReply {
    ConfigFileSnapshotReply {
        locator: snapshot.locator,
        path: snapshot.path.to_string_lossy().into_owned(),
        revision: snapshot.revision,
        availability: snapshot.availability,
        agents: snapshot.agents,
        coding_agent_profiles: snapshot.coding_agent_profiles,
        read_only: snapshot.read_only,
        diagnostic: snapshot.diagnostic,
    }
}

fn file_name_of(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn root_name(root: &str) -> String {
    let trimmed = root.trim_end_matches(['/', '\\']);
    trimmed
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(trimmed)
        .to_owned()
}

fn context_of(settings: &AppSettings, context: Option<&str>) -> Result<ConfigContext, String> {
    context_for_root(&settings.project_paths, context).map_err(|e| e.to_string())
}

/// Decision 7: the settings path of the read wrappers (same value as the owner's path).
fn settings_file_path() -> Result<PathBuf, String> {
    crate::config::config_dir()
        .map(|dir| dir.join(crate::config::instance_artifacts::SETTINGS_FILE_NAME))
        .ok_or_else(|| {
            ConfigFileError::new(
                "configFileUnavailable",
                "The settings folder is not available.",
            )
            .to_string()
        })
}

fn config_list(
    settings: &AppSettings,
    settings_path: &Path,
    ctx: &ConfigContext,
) -> ConfigFileList {
    let listing = list_config_files(settings, settings_path, ctx);
    let mut contexts = vec![ConfigContextOption {
        root: None,
        name: "Outside projects".into(),
    }];
    contexts.extend(
        settings
            .project_paths
            .iter()
            .map(|root| ConfigContextOption {
                root: Some(root.clone()),
                name: root_name(root),
            }),
    );
    let entries = listing
        .entries
        .into_iter()
        .map(
            |(entry, availability, read_only, agent_count)| ConfigFileEntry {
                group: match entry.group {
                    EntryGroup::Instance => "instance",
                    EntryGroup::Project => "project",
                    EntryGroup::Loaded => "loaded",
                    EntryGroup::Other => "other",
                },
                locator: entry.locator,
                file_name: entry.file_name,
                path: entry.path.to_string_lossy().into_owned(),
                activatable: availability == SourceAvailability::Available,
                availability,
                read_only,
                agent_count,
                diagnostic: None,
            },
        )
        .collect();
    ConfigFileList {
        context: match &listing.context {
            ConfigContext::Instance => None,
            ConfigContext::Project(key) => Some(key.clone()),
        },
        contexts,
        active: listing.active,
        active_notice: listing.active_notice,
        entries,
    }
}

fn publish_error(error: &str, sentence: &str) -> String {
    let code = error.split(':').next().unwrap_or(error);
    format!("{code}: {sentence}")
}

const OWNER_UNAVAILABLE: &str =
    "settingsOwnerUnavailable: The settings are not available. Nothing was changed.";

const PUBLISH_FAILED: &str = "The settings file could not be updated. Nothing was changed.";

/// Decision 1: the `finish_remember_agent_file_report` mapping for config-file writes.
fn finish_config_report<T>(
    report: crate::config::settings::SettingsOperationReport<T>,
) -> Result<T, String> {
    use crate::config::settings::{OperationOutcome, SettingsAvailability};
    if report.outcome == OperationOutcome::PartialOrUnknown
        || (report.outcome == OperationOutcome::Committed
            && report.availability != SettingsAvailability::Ready)
    {
        return Err(
            "partialOrUnknown: The change may have been saved. Reload the settings.".into(),
        );
    }
    match (report.outcome, report.value) {
        (OperationOutcome::Committed, Some(value)) => Ok(value),
        _ => Err(report.error.unwrap_or_default()),
    }
}

async fn run_config_write<T, F>(
    settings: &SettingsState,
    name: &'static str,
    builder: F,
) -> Result<(T, AppSettings), String>
where
    T: Send + 'static,
    F: FnOnce(
            &AppSettings,
            &mut crate::config::settings::SettingsOperationGuard<'_>,
        ) -> Result<T, String>
        + Send
        + 'static,
{
    let report = settings.with_operation(name, builder).await.map_err(|_| {
        "settingsOwnerUnavailable: The settings are not available. Nothing was changed.".to_owned()
    })?;
    let value = finish_config_report(report)?;
    let adopted = settings.current_snapshot().await.map_err(|_| {
        "partialOrUnknown: The change was saved but the settings are unavailable.".to_owned()
    })?;
    Ok((value, adopted))
}

pub(crate) async fn list_coding_agent_config_files_inner(
    settings: &SettingsState,
    settings_path: &Path,
    context: Option<String>,
) -> Result<ConfigFileList, String> {
    let current = settings
        .current_snapshot()
        .await
        .map_err(|_| OWNER_UNAVAILABLE.to_owned())?;
    let ctx = context_of(&current, context.as_deref())?;
    Ok(config_list(&current, settings_path, &ctx))
}

pub(crate) async fn read_coding_agent_config_file_inner(
    settings: &SettingsState,
    settings_path: &Path,
    context: Option<String>,
    locator: ConfigFileLocator,
) -> Result<ConfigFileSnapshotReply, String> {
    let current = settings
        .current_snapshot()
        .await
        .map_err(|_| OWNER_UNAVAILABLE.to_owned())?;
    let ctx = context_of(&current, context.as_deref())?;
    read_config_file(&current, settings_path, &ctx, &locator)
        .map(snapshot_reply)
        .map_err(|e| e.to_string())
}

pub(crate) async fn get_scoped_coding_agents_inner(
    settings: &SettingsState,
    settings_path: &Path,
    context: Option<String>,
    locator: Option<ConfigFileLocator>,
) -> Result<ScopedCodingAgents, String> {
    let current = settings
        .current_snapshot()
        .await
        .map_err(|_| OWNER_UNAVAILABLE.to_owned())?;
    let ctx = context_of(&current, context.as_deref())?;
    let requested = locator.is_some();
    let locator = locator.unwrap_or_else(|| {
        crate::config::coding_agent_files::active_locator(
            current.coding_agent_config_files.as_ref(),
            &ctx,
        )
    });
    let read = agents_of(&current, settings_path, &ctx, &locator);
    if let Ok((agents, coding_agent_profiles)) = read {
        let file_name = read_config_file(&current, settings_path, &ctx, &locator)
            .map(|snapshot| file_name_of(&snapshot.path))
            .unwrap_or_default();
        return Ok(ScopedCodingAgents {
            agents,
            coding_agent_profiles,
            locator,
            file_name,
            notice: None,
        });
    }
    if let Err(error) = read.as_ref() {
        // A bad requested locator is an error; a stored one is only unavailable (D6).
        if requested && error.code != "configFileUnavailable" {
            return Err(error.to_string());
        }
    }
    let registered = ConfigFileLocator::RegisteredInstance {};
    let path = read_config_file(&current, settings_path, &ctx, &registered)
        .map(|snapshot| snapshot.path)
        .unwrap_or_default();
    Ok(ScopedCodingAgents {
        agents: current.agents.clone(),
        coding_agent_profiles: current.coding_agent_profiles.clone(),
        locator: registered,
        file_name: file_name_of(&path),
        notice: Some("activeConfigFileUnavailable".into()),
    })
}

pub(crate) async fn save_coding_agent_config_file_inner(
    settings: &SettingsState,
    settings_path: &Path,
    context: Option<String>,
    locator: ConfigFileLocator,
    expected_revision: SourceRevision,
    agents: Vec<AgentConfig>,
    coding_agent_profiles: CodingAgentProfilesConfig,
) -> Result<ConfigFileSnapshotReply, String> {
    let loc = locator.clone();
    let (ctx, adopted) = run_config_write(
        settings,
        "save_coding_agent_config_file",
        move |current, guard| {
            let ctx = context_of(current, context.as_deref())?;
            let path = guard.settings_path().to_path_buf();
            let profiles = coding_agent_profiles;
            save_config_file(
                current,
                &path,
                &ctx,
                &loc,
                &expected_revision,
                agents,
                profiles,
            )
            .map_err(|e| e.to_string())?;
            Ok(ctx)
        },
    )
    .await?;
    read_config_file(&adopted, settings_path, &ctx, &locator)
        .map(snapshot_reply)
        .map_err(|e| e.to_string())
}

pub(crate) async fn save_as_coding_agent_config_file_inner(
    settings: &SettingsState,
    settings_path: &Path,
    context: Option<String>,
    scope: ConfigScope,
    name: String,
    agents: Vec<AgentConfig>,
    coding_agent_profiles: CodingAgentProfilesConfig,
) -> Result<ConfigFileSnapshotReply, String> {
    let ((ctx, locator), adopted) = run_config_write(
        settings,
        "save_as_coding_agent_config_file",
        move |current, guard| {
            let ctx = context_of(current, context.as_deref())?;
            let path = guard.settings_path().to_path_buf();
            let profiles = coding_agent_profiles;
            let created = create_config_file(current, &path, &ctx, scope, &name, agents, profiles)
                .map_err(|e| e.to_string())?;
            let sel = set_active_config_file(current, &path, &ctx, &created.locator)
                .map_err(|e| e.to_string())?;
            publish_config_selection_locked(guard, &sel).map_err(|e| {
                publish_error(&e, "The file was created but could not be made active.")
            })?;
            Ok((ctx, created.locator))
        },
    )
    .await?;
    read_config_file(&adopted, settings_path, &ctx, &locator)
        .map(snapshot_reply)
        .map_err(|e| e.to_string())
}

pub(crate) async fn set_active_coding_agent_config_file_inner(
    settings: &SettingsState,
    settings_path: &Path,
    context: Option<String>,
    locator: ConfigFileLocator,
) -> Result<ConfigFileList, String> {
    let (ctx, adopted) = run_config_write(
        settings,
        "set_active_coding_agent_config_file",
        move |current, guard| {
            let ctx = context_of(current, context.as_deref())?;
            let path = guard.settings_path().to_path_buf();
            let sel = set_active_config_file(current, &path, &ctx, &locator)
                .map_err(|e| e.to_string())?;
            publish_config_selection_locked(guard, &sel)
                .map_err(|e| publish_error(&e, PUBLISH_FAILED))?;
            Ok(ctx)
        },
    )
    .await?;
    Ok(config_list(&adopted, settings_path, &ctx))
}

pub(crate) async fn load_coding_agent_config_file_inner(
    settings: &SettingsState,
    settings_path: &Path,
    context: Option<String>,
    path: String,
) -> Result<ConfigFileList, String> {
    let (ctx, adopted) = run_config_write(
        settings,
        "load_coding_agent_config_file",
        move |current, guard| {
            let ctx = context_of(current, context.as_deref())?;
            let settings_path = guard.settings_path().to_path_buf();
            // Decision 2, steps 1-5: `set_active` runs on `next`, never on `current`.
            let sel = register_loaded(current, &path).map_err(|e| e.to_string())?;
            let canonical = crate::filesystem::settings_sources::canonical(Path::new(&path))
                .map_err(|_| {
                    ConfigFileError::new("configFileInvalid", "This is not a valid agents file.")
                        .to_string()
                })?
                .to_string_lossy()
                .into_owned();
            let next = AppSettings {
                coding_agent_config_files: Some(sel),
                ..current.clone()
            };
            let loaded = ConfigFileLocator::Loaded { path: canonical };
            let sel = set_active_config_file(&next, &settings_path, &ctx, &loaded)
                .map_err(|e| e.to_string())?;
            publish_config_selection_locked(guard, &sel)
                .map_err(|e| publish_error(&e, PUBLISH_FAILED))?;
            Ok(ctx)
        },
    )
    .await?;
    Ok(config_list(&adopted, settings_path, &ctx))
}

/// Decision 5: the change events after a committed config-file write.
pub(crate) fn emit_coding_agent_settings_events(app: &AppHandle) {
    let payload = serde_json::json!({});
    for event in [
        "coding_agent_settings_updated",
        "coding_agent_profiles_updated",
    ] {
        let _ = app.emit(event, &payload);
        if let Some(broadcaster) = app.try_state::<WsBroadcaster>() {
            broadcaster.broadcast_event(event, &payload);
        }
    }
}

#[tauri::command]
pub(crate) async fn list_coding_agent_config_files(
    settings: State<'_, SettingsState>,
    context: Option<String>,
) -> Result<ConfigFileList, String> {
    let path = settings_file_path()?;
    list_coding_agent_config_files_inner(settings.inner(), &path, context).await
}

#[tauri::command]
pub(crate) async fn read_coding_agent_config_file(
    settings: State<'_, SettingsState>,
    context: Option<String>,
    locator: ConfigFileLocator,
) -> Result<ConfigFileSnapshotReply, String> {
    let path = settings_file_path()?;
    read_coding_agent_config_file_inner(settings.inner(), &path, context, locator).await
}

#[tauri::command]
pub(crate) async fn get_scoped_coding_agents(
    settings: State<'_, SettingsState>,
    context: Option<String>,
    locator: Option<ConfigFileLocator>,
) -> Result<ScopedCodingAgents, String> {
    let path = settings_file_path()?;
    get_scoped_coding_agents_inner(settings.inner(), &path, context, locator).await
}

#[tauri::command]
pub(crate) async fn save_coding_agent_config_file(
    app: AppHandle,
    settings: State<'_, SettingsState>,
    context: Option<String>,
    locator: ConfigFileLocator,
    expected_revision: SourceRevision,
    agents: Vec<AgentConfig>,
    coding_agent_profiles: CodingAgentProfilesConfig,
) -> Result<ConfigFileSnapshotReply, String> {
    let path = settings_file_path()?;
    let reply = save_coding_agent_config_file_inner(
        settings.inner(),
        &path,
        context,
        locator,
        expected_revision,
        agents,
        coding_agent_profiles,
    )
    .await?;
    emit_coding_agent_settings_events(&app);
    Ok(reply)
}

#[tauri::command]
pub(crate) async fn save_as_coding_agent_config_file(
    app: AppHandle,
    settings: State<'_, SettingsState>,
    context: Option<String>,
    scope: ConfigScope,
    name: String,
    agents: Vec<AgentConfig>,
    coding_agent_profiles: CodingAgentProfilesConfig,
) -> Result<ConfigFileSnapshotReply, String> {
    let path = settings_file_path()?;
    let reply = save_as_coding_agent_config_file_inner(
        settings.inner(),
        &path,
        context,
        scope,
        name,
        agents,
        coding_agent_profiles,
    )
    .await?;
    emit_coding_agent_settings_events(&app);
    Ok(reply)
}

#[tauri::command]
pub(crate) async fn set_active_coding_agent_config_file(
    app: AppHandle,
    settings: State<'_, SettingsState>,
    context: Option<String>,
    locator: ConfigFileLocator,
) -> Result<ConfigFileList, String> {
    let path = settings_file_path()?;
    let reply =
        set_active_coding_agent_config_file_inner(settings.inner(), &path, context, locator)
            .await?;
    emit_coding_agent_settings_events(&app);
    Ok(reply)
}

#[tauri::command]
pub(crate) async fn load_coding_agent_config_file(
    app: AppHandle,
    settings: State<'_, SettingsState>,
    context: Option<String>,
    path: String,
) -> Result<ConfigFileList, String> {
    let settings_path = settings_file_path()?;
    let reply =
        load_coding_agent_config_file_inner(settings.inner(), &settings_path, context, path)
            .await?;
    emit_coding_agent_settings_events(&app);
    Ok(reply)
}
