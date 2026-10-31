use super::resolve_restarted_session;
use super::{ForgottenSummary, SelfClearBoundary, SelfHandoffFlavor};
use crate::phone::mailbox::settle::inject_handoff_prompt_with_archive;
use std::path::{Path, PathBuf};
use uuid::Uuid;

pub(crate) async fn persist_and_restart<Persist, PersistFut, Restart, RestartFut>(
    targets: (&Path, &str, &str),
    flavor: &SelfHandoffFlavor,
    sessions: (Uuid, Uuid),
    current: &mut Option<Uuid>,
    persist: &mut Persist,
    restart: &mut Restart,
) -> Result<Uuid, String>
where
    Persist: FnMut(PathBuf, String, String) -> PersistFut,
    PersistFut: std::future::Future<Output = Result<(), String>>,
    Restart: FnMut(Uuid, String, String) -> RestartFut,
    RestartFut: std::future::Future<Output = Result<String, String>>,
{
    let (cwd, target_agent, target_profile) = targets;
    let (original_session_id, session_id) = sessions;
    if let Err(e) = persist(
        cwd.to_path_buf(),
        target_agent.to_string(),
        target_profile.to_string(),
    )
    .await
    {
        log::warn!(
            "[mailbox] {}: persist failed for session {}: {}",
            flavor.action,
            original_session_id,
            e
        );
        return Err(e);
    }
    let restarted = restart(
        session_id,
        target_agent.to_string(),
        target_profile.to_string(),
    )
    .await;
    resolve_restarted_session(restarted, flavor, original_session_id, current)
}

pub(crate) async fn inject_handoff<Inject, InjectFut, NoteBoundary, NoteFut>(
    session: (&Path, Uuid, std::time::Duration),
    flavor: &SelfHandoffFlavor,
    forgotten_summary: Option<&ForgottenSummary>,
    inject: &mut Inject,
    note_boundary: &mut NoteBoundary,
) -> bool
where
    Inject: FnMut(Uuid, String) -> InjectFut + Send + 'static,
    InjectFut: std::future::Future<Output = Result<(), String>> + Send,
    NoteBoundary: FnMut(Uuid, SelfClearBoundary) -> NoteFut,
    NoteFut: std::future::Future<Output = ()>,
{
    let (cwd, session_id, settle) = session;
    log::info!(
        "[mailbox] {}: session {} idle >={}s {}; injecting handoff prompt (phase \
                        2)",
        flavor.action,
        session_id,
        settle.as_secs(),
        flavor.phase2_verb
    );
    // #749 - archive-first contract; see inject_handoff_prompt_with_archive.
    // (#756) NOTE: the switch phase 1 fires NO Cleared event; it is a
    // restart through restart_session_inner_with_activation (skip
    // Some(true)), which stamps record + mirror itself via C3. The
    // handoff lands on the CURRENT session_id (the post-switch id).
    let injected = inject_handoff_prompt_with_archive(
        cwd,
        session_id,
        flavor.action,
        flavor.build_prompt,
        forgotten_summary,
        inject,
    )
    .await;
    if injected {
        // (#756) switch handoff prompt is post-boundary content.
        note_boundary(session_id, SelfClearBoundary::ContentInjected).await;
    }

    injected
}

pub(crate) async fn begin_switch(
    session_id: Uuid,
    settle: std::time::Duration,
    flavor: &SelfHandoffFlavor,
    target_agent: &str,
    target_profile: &str,
) -> Result<impl Send, String> {
    log::info!(
        "[mailbox] {}: session {} idle >={}s; {} with coding agent '{}' profile \
                        '{}' (phase 1)",
        flavor.action,
        session_id,
        settle.as_secs(),
        flavor.phase1_verb,
        target_agent,
        super::super::handoff::display_profile_for_log(target_profile)
    );

    crate::session::selection::acquire_selection_operation_turn()
        .await
        .map_err(|error| {
            log::warn!(
                "[mailbox] {}: selection operation turn unavailable for session \
                                {}: {}",
                flavor.action,
                session_id,
                error
            );
            error.to_string()
        })
}
