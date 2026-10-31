#[cfg(test)]
use crate::commands::session::room_raise_seam;
use crate::config::co_managed::CoManagedState;
use crate::config::settings::SettingsState;
use crate::session::manager::SessionManager;
use crate::session::profile::CodingAgentKind;
use std::future::Future;
use std::sync::Arc;
use tauri::AppHandle;
use tauri::Manager;
use tauri::Runtime;
use uuid::Uuid;

/// Gather every SCC-owned value the Co-managed effective state needs and call
/// the leaf `config::co_managed::effective_state` (#2265, epic #2232).
///
/// **The home of this function is a layering requirement, not a preference.**
/// `commands::session` is already inside the 88-member SCC, so the arcs it
/// needs (`config::settings` for the key, `config::teams` for the orchestrator,
/// `commands::telegram::derive_reader` for the reader kind) are internal to the
/// SCC, and the call down into the leaf cannot grow it either. Putting the
/// reading in the Co-managed command module and letting this module call it would give
/// that command module an incoming arc from an SCC member and absorb it into
/// the cycle (phase 2 section 4.1).
///
/// `derive_reader` is called read-only for its return value and is not modified;
/// the session memo's container homes are passed through unchanged, because
/// `None` would falsely reject a supported container.
pub(crate) async fn co_managed_effective_state_for_session<R: Runtime>(
    app: &AppHandle<R>,
    room_root: &std::path::Path,
    session_id: &str,
) -> Result<crate::config::co_managed::CoManagedState, String> {
    let inputs = co_managed_inputs_for_session(app, room_root, session_id).await?;
    Ok(crate::config::co_managed::effective_state(
        room_root,
        inputs.globally_enabled,
        &inputs.api_key,
        inputs.is_orchestrator,
        inputs.capture_supported,
        inputs.agent_label,
    ))
}

/// #2756 U2a sibling of [`co_managed_effective_state_for_session`]: whether the
/// session may hold a Room reader and keep its candidate. Same gathering, so
/// the two predicates cannot read different inputs; never reads the key or
/// the catalog.
pub(crate) async fn co_managed_capturable_for_session<R: Runtime>(
    app: &AppHandle<R>,
    room_root: &std::path::Path,
    session_id: &str,
) -> Result<bool, String> {
    let inputs = co_managed_inputs_for_session(app, room_root, session_id).await?;
    Ok(crate::config::co_managed::capturable(
        room_root,
        inputs.globally_enabled,
        inputs.is_orchestrator,
        inputs.capture_supported,
    ))
}

/// Every SCC-owned input of the Co-managed predicates, gathered once.
pub(crate) struct CoManagedInputs {
    is_orchestrator: bool,
    capture_supported: bool,
    api_key: String,
    globally_enabled: bool,
    agent_label: &'static str,
}

pub(crate) async fn co_managed_inputs_for_session<R: Runtime>(
    app: &AppHandle<R>,
    room_root: &std::path::Path,
    session_id: &str,
) -> Result<CoManagedInputs, String> {
    let session = {
        let manager = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
        let id =
            Uuid::parse_str(session_id).map_err(|e| format!("coManagedSessionIdInvalid: {e}"))?;
        let guard = manager.read().await;
        guard
            .get_session(id)
            .await
            .ok_or_else(|| "coManagedSessionNotFound".to_string())?
    };

    let is_orchestrator = room_root
        .parent()
        .and_then(|ac_root| {
            crate::config::teams::resolve_wg_coordinator_replica(ac_root, room_root)
        })
        .map(|resolved| {
            co_managed_same_directory(
                &resolved.replica_dir,
                std::path::Path::new(&session.working_directory),
            )
        })
        .unwrap_or(false);

    let reader = crate::commands::telegram::derive_reader(
        &session.shell,
        &session.shell_args,
        session
            .effective_shell_args
            .as_deref()
            .unwrap_or(&session.shell_args),
        &session.working_directory,
        session.backend_kind,
        session.agent_kind,
        session.resolved_claude_projects_dir.clone(),
        session.effective_codex_home.as_deref(),
    );
    let capture_supported = matches!(reader, Ok(Some(_)));

    let (api_key, globally_enabled) = {
        let settings = app.state::<SettingsState>();
        let guard = settings
            .current_read()
            .await
            .map_err(|error| error.to_string())?;
        (guard.jev_api_key.clone(), guard.co_managed_enabled)
    };

    Ok(CoManagedInputs {
        is_orchestrator,
        capture_supported,
        api_key,
        globally_enabled,
        agent_label: co_managed_provider_label(session.agent_kind),
    })
}

/// Raise the **Room** demand for `session_id`, in `room_root`, if and only if
/// the session is capturable (#2232 phase 4 section 5.1, #2756 U2a).
///
/// One call to the shared gathering function decides: it encodes global
/// switch, orchestrator-only, provider-supported and flag-on. The key and the
/// catalog are `Ready` inputs, not capture inputs, so a room without them
/// still keeps its reader and its candidate. A room without the flag raises
/// nothing and behaves exactly as today. Returns `true` when a reader is running afterwards.
///
/// Every caller reaches the supervisor through this SCC-member function, never
/// through the Co-managed command module, which keeps **outgoing arcs only**.
pub(crate) async fn raise_room_reader_demand_in<R: Runtime>(
    app: &AppHandle<R>,
    room_root: &std::path::Path,
    session_id: Uuid,
) -> bool {
    // #2516: sampled before readiness, so a release anywhere in the window up
    // to the install aborts the raise under the bridge lock.
    let expected = crate::commands::telegram::room_demand_epoch(app, session_id).await;
    #[cfg(test)]
    let key = session_id.to_string();
    #[cfg(test)]
    room_raise_seam::hit_before_readiness(&key).await;
    match co_managed_capturable_for_session(app, room_root, &session_id.to_string()).await {
        Ok(true) => {
            #[cfg(test)]
            room_raise_seam::hit_before_add(&key).await;
            crate::commands::telegram::raise_reader_demand_guarded(
                app,
                session_id,
                crate::telegram::manager::ReaderConsumer::Room,
                None,
                Some(expected),
            )
            .await
        }
        _ => false,
    }
}

/// Resolve the session's own room and raise the Room demand there, if any.
pub(crate) async fn raise_room_reader_demand<R: Runtime>(
    app: &AppHandle<R>,
    session_id: Uuid,
) -> bool {
    let cwd = {
        let manager = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
        let guard = manager.read().await;
        match guard.get_session(session_id).await {
            Some(session) => session.working_directory,
            None => return false,
        }
    };
    let Some(room_root) = crate::config::co_managed::room_root_for_path(std::path::Path::new(&cwd))
    else {
        return false;
    };
    raise_room_reader_demand_in(app, &room_root, session_id).await
}

/// Release the Room demand. The reader stops only when it was the last one.
pub(crate) async fn release_room_reader_demand<R: Runtime>(app: &AppHandle<R>, session_id: Uuid) {
    crate::commands::telegram::release_reader_demand(
        app,
        session_id,
        crate::telegram::manager::ReaderConsumer::Room,
    )
    .await;
}

/// The provider name that reaches `OffReason::UnsupportedProvider`; it matches
/// `CodingAgentKind`'s snake_case wire form.
pub(crate) fn co_managed_provider_label(kind: Option<CodingAgentKind>) -> &'static str {
    match kind {
        Some(CodingAgentKind::Claude) => "claude",
        Some(CodingAgentKind::Codex) => "codex",
        Some(CodingAgentKind::Antigravity) => "antigravity",
        Some(CodingAgentKind::Pi) => "pi",
        Some(CodingAgentKind::Muse) => "muse",
        None => "none",
    }
}

pub(crate) fn co_managed_same_directory(left: &std::path::Path, right: &std::path::Path) -> bool {
    match (
        crate::filesystem::projects::canonical_project_path(left),
        crate::filesystem::projects::canonical_project_path(right),
    ) {
        (Ok(left), Ok(right)) => crate::path_identity::paths_equivalent(&left, &right),
        _ => crate::path_identity::paths_equivalent(left, right),
    }
}

impl<R: Runtime> crate::reader_demand::ReaderDemandHost for AppHandle<R> {
    fn reader_settings(&self) -> Option<&SettingsState> {
        self.try_state::<SettingsState>().map(|state| state.inner())
    }

    fn reader_sessions(&self) -> Option<&Arc<tokio::sync::RwLock<SessionManager>>> {
        self.try_state::<Arc<tokio::sync::RwLock<SessionManager>>>()
            .map(|state| state.inner())
    }

    fn reader_sessions_required(&self) -> &Arc<tokio::sync::RwLock<SessionManager>> {
        self.state::<Arc<tokio::sync::RwLock<SessionManager>>>()
            .inner()
    }

    fn reader_registry(&self) -> Option<&Arc<crate::capture::registry::CaptureRegistry>> {
        self.try_state::<Arc<crate::capture::registry::CaptureRegistry>>()
            .map(|state| state.inner())
    }

    fn holds_room_reader_demand<'a>(
        &'a self,
        session_id: Uuid,
    ) -> impl std::future::Future<Output = bool> + Send + 'a {
        crate::commands::telegram::holds_room_reader_demand(self, session_id)
    }

    fn raise_room_reader_demand<'a>(
        &'a self,
        session_id: Uuid,
    ) -> impl std::future::Future<Output = bool> + Send + 'a {
        crate::commands::session::raise_room_reader_demand(self, session_id)
    }

    fn release_room_reader_demand<'a>(
        &'a self,
        session_id: Uuid,
    ) -> impl std::future::Future<Output = ()> + Send + 'a {
        crate::commands::session::co_managed::release_room_reader_demand(self, session_id)
    }

    fn co_managed_capturable_for_session<'a>(
        &'a self,
        room_root: &'a std::path::Path,
        session_id: &'a str,
    ) -> impl std::future::Future<Output = Result<bool, String>> + Send + 'a {
        crate::commands::session::co_managed_capturable_for_session(self, room_root, session_id)
    }

    fn co_managed_effective_state_for_session<'a>(
        &'a self,
        room_root: &'a std::path::Path,
        session_id: &'a str,
    ) -> impl Future<Output = Result<CoManagedState, String>> + Send + 'a {
        crate::commands::session::co_managed::co_managed_effective_state_for_session(
            self, room_root, session_id,
        )
    }
}
