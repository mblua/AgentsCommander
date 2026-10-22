use std::collections::BTreeMap;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::config::settings::SettingsState;
use crate::web::broadcast::WsBroadcaster;

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
