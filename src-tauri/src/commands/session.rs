use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager, Runtime, State};
use uuid::Uuid;

use crate::config::agent_command::AgentSpawnCommand;
use crate::config::agent_config;
use crate::config::coding_agents_catalog::{command_executable_basename, CodingAgentDefinition};
use crate::config::coordinator_clocks::{ClearedCloseMarkers, CoordinatorClocksState};
use crate::config::sessions_persistence::persist_current_state;
use crate::config::settings::{AppSettings, SettingsState};
use crate::pty::backend::{
    BackendSpawnSpec, LaunchWitness, PtyViewport, ResolvedAgentHostShell, SessionBackendKind,
};
use crate::pty::container_paths::{
    claude_config_dir_no_value_warning, container_config_dir, ContainerEnvWarning,
    ContainerPathMap, CLAUDE_CONFIG_DIR_KEY,
};
use crate::pty::container_runtime::DEFAULT_CONTAINER_WORKDIR;
use crate::pty::manager::PtyManager;
use crate::pty::output::PtyOutputTarget;
use crate::resource_monitor::{
    AgentLaunchPermit, ResourceLaunchMetadata, ResourceLaunchRegistration, ResourceLimits,
    ResourceLogicalAgentSlot, ResourceMonitorState,
};
use crate::session::manager::{
    acquire_carrier_mutation_permit, CommitDecision, LifecycleMutations, PendingCreateBinding,
    SessionManager,
};
use crate::session::profile::{
    idle_tuning_for, idle_tuning_for_launch, locate_pi_command, CodingAgentKind, IdleTuning,
    PiInsertionPoint,
};
use crate::session::selection::{
    CriticalAdmissionOutcome, SelectionCause, SelectionCoordinator, SelectionRequest,
    SelectionSource, SelectionTransaction, TrustedCreateIntent, TrustedRestartIntent,
};
use crate::session::session::{
    SessionCommunication, SessionCommunicationKind, SessionInfo, SessionRepo, SessionStatus,
};
use crate::session::warnings::{emit_session_warning, SessionWarning};
use crate::telegram::manager::TelegramBridgeState;
use crate::DetachedSessionsState;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExistingRootAction {
    ReuseLive,
    WakeDormant,
    DiscardMissingPty,
}

fn classify_existing_root(status: &SessionStatus, has_pty: bool) -> ExistingRootAction {
    if matches!(status, SessionStatus::Exited(_)) {
        ExistingRootAction::WakeDormant
    } else if has_pty {
        ExistingRootAction::ReuseLive
    } else {
        ExistingRootAction::DiscardMissingPty
    }
}

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
struct CoManagedInputs {
    is_orchestrator: bool,
    capture_supported: bool,
    api_key: String,
    globally_enabled: bool,
    agent_label: &'static str,
}

async fn co_managed_inputs_for_session<R: Runtime>(
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

#[cfg(test)]
pub(crate) mod room_raise_seam;

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
fn co_managed_provider_label(kind: Option<CodingAgentKind>) -> &'static str {
    match kind {
        Some(CodingAgentKind::Claude) => "claude",
        Some(CodingAgentKind::Codex) => "codex",
        Some(CodingAgentKind::Antigravity) => "antigravity",
        Some(CodingAgentKind::Pi) => "pi",
        Some(CodingAgentKind::Muse) => "muse",
        None => "none",
    }
}

fn co_managed_same_directory(left: &std::path::Path, right: &std::path::Path) -> bool {
    match (std::fs::canonicalize(left), std::fs::canonicalize(right)) {
        (Ok(left), Ok(right)) => crate::path_identity::paths_equivalent(&left, &right),
        _ => crate::path_identity::paths_equivalent(left, right),
    }
}

/// #1032 + #1171 - start sampling a freshly spawned session, with both engines.
///
/// **Sessions with no agent are never registered** with the scraper or the quota engine, and
/// that rule lives HERE, once: a plain shell costs neither anything, ever. #2879 - the watcher
/// engine is asked BEFORE that rule, with the optional agent, because its internal activity
/// consumer may observe a Codex session that has no configured agent; it registers nothing
/// when there is neither an agent nor a consumer. `try_state` and not `state`, because
/// `state` panics when unmanaged and a test app manages neither engine - an absent engine is
/// simply the feature being off.
///
/// There is no race with the screen parser: `PtyManager::spawn` has already returned `Ok` at
/// the call site and the parser is registered during the spawn. The scraper's first sample is
/// 5 s away and the first watcher tick is 200 ms away, and neither can arrive before the
/// parser exists.
pub(crate) fn register_session_samplers<R: tauri::Runtime>(
    app: &AppHandle<R>,
    id: Uuid,
    agent_id: Option<String>,
) {
    if let Some(watchers) = app.try_state::<Arc<crate::pty::watchers::WatcherEngine>>() {
        watchers.register_session(id, agent_id.clone());
    }
    let Some(agent_id) = agent_id else {
        return;
    };
    if let Some(scraper) = app.try_state::<Arc<crate::pty::context_scrape::ContextScraper>>() {
        scraper.register_session(id, agent_id.clone());
    }
    if let Some(quota) = app.try_state::<Arc<crate::pty::agent_quota::AgentQuotaEngine>>() {
        quota.register_session(id, agent_id);
    }
}

/// #1171 - purge every per-session side structure a DESTROYED session leaves behind.
///
/// One helper with two call sites, because the per-session side-state purge is already
/// duplicated in this file: the destroy loop and `publish_restart_destroyed` are parallel
/// copies of each other, and adding a third thing to purge to only one of them is how a leak
/// gets written.
///
/// **The order is load-bearing.** The engine is retired FIRST, so no tick still in flight can
/// republish this session's status and recreate the entry step 2 removes. `WatcherHistory`
/// creates entries only from the engine's publish, and the engine publishes only while holding
/// its registration lock, so once `retire_session` returns nothing can bring the entry back.
///
/// **Destroyed sessions only.** A session that exits on its own is NOT destroyed: the engine
/// stops sampling it and its buffer stays, which is the case that matters - an API error or a
/// CLI crash is exactly when the evidence is worth keeping. Root-agent sessions retained as
/// `Exited` keep theirs too, because their row is still in the list.
pub(crate) fn purge_session_side_state<R: tauri::Runtime>(app: &AppHandle<R>, session_id: Uuid) {
    if let Some(quota) = app.try_state::<Arc<crate::pty::agent_quota::AgentQuotaEngine>>() {
        quota.retire_session(session_id);
    }
    if let Some(watchers) = app.try_state::<Arc<crate::pty::watchers::WatcherEngine>>() {
        watchers.retire_session(session_id);
    }
    if let Some(history) = app.try_state::<crate::pty::watchers::history::WatcherHistoryState>() {
        history.purge(session_id);
    }
    reset_substantive_input(app, session_id);
}

/// #871 - clear a session's substantive-input marker. (#2336) Also clears the
/// session's typing-hold entry, so the destroy and restart teardown paths drop
/// the manual padlock, the natural clock and every counted held ID with the
/// tracker they already reset.
///
/// Extracted by #1171 so it has ONE definition rather than the two inline copies the destroy
/// and restart cleanups each carried. It is a map removal, so calling it twice for the same
/// session is a no-op and not a second effect - which is what lets the destroy path keep
/// resetting every id it publishes, retained-exited ones included, while the destroyed-only
/// purge below also resets the ones it owns.
fn reset_substantive_input<R: tauri::Runtime>(app: &AppHandle<R>, session_id: Uuid) {
    if let Some(activity) = app.try_state::<crate::pty::input_activity::SubstantiveInputState>() {
        activity
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .reset(session_id);
    }
    if let Some(hold) = app.try_state::<crate::pty::input_activity::TypingHoldState>() {
        hold.lock()
            .unwrap_or_else(|error| error.into_inner())
            .reset(session_id);
    }
}

async fn rollback_pre_created_session<R: tauri::Runtime>(
    _app: &AppHandle<R>,
    _session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
    _pty_mgr: &Arc<Mutex<PtyManager>>,
    id: Uuid,
    reason: &str,
) {
    log::warn!(
        "[session] Rolling back pre-created session {} after setup failure: {}",
        id,
        reason
    );
    // The reserved create ticket or inline transaction owns the one common
    // backend-kind-aware rollback after this setup body returns. Killing here
    // would race that finalizer and perform the same teardown twice.
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CreateSelectionIntent {
    User,
    Background,
    Suppress,
}

impl CreateSelectionIntent {
    fn trusted(self) -> Option<TrustedCreateIntent> {
        match self {
            Self::User => Some(TrustedCreateIntent::User),
            Self::Background => Some(TrustedCreateIntent::Background),
            Self::Suppress => None,
        }
    }
}

struct DeferredCreateOutput {
    info: SessionInfo,
    binding: PendingCreateBinding,
    warnings: Vec<SessionWarning>,
    // Keep admission through the enclosing retained transaction's finalization.
    _create_admission: Arc<Mutex<OwnedCreateAdmission>>,
    effective_continuation: bool,
}

#[derive(Clone)]
struct InitialLastPrompt {
    text: Option<String>,
    agent_id: Option<String>,
    requested_profile: Option<String>,
    effective_profile: Option<String>,
}

impl InitialLastPrompt {
    fn from_persisted(row: &crate::config::sessions_persistence::PersistedSession) -> Self {
        Self {
            text: row.last_prompt.clone(),
            agent_id: row.agent_id.clone(),
            requested_profile: row.requested_profile.clone(),
            effective_profile: row.effective_profile.clone(),
        }
    }

    fn compatible(&self, row: &crate::session::session::Session) -> bool {
        self.agent_id.is_some()
            && self.agent_id == row.agent_id
            && self.requested_profile == row.requested_profile
            && self.effective_profile == row.effective_profile
    }
}

enum CreateCompletion {
    Retained,
    Finalized(SessionInfo),
    Deferred(DeferredCreateOutput),
}

impl CreateCompletion {
    async fn into_finalized(self) -> Result<SessionInfo, String> {
        match self {
            Self::Retained => std::future::pending().await,
            Self::Finalized(info) => Ok(info),
            Self::Deferred(_) => Err("create unexpectedly deferred finalization".to_string()),
        }
    }
}

fn release_resource_launch_permit(
    monitor: &Arc<ResourceMonitorState>,
    permit: &mut Option<AgentLaunchPermit>,
) {
    if let Some(permit) = permit.take() {
        monitor.release_unregistered_permit(permit);
    }
}

fn token_has_unclosed_quote(token: &str, quote: char) -> bool {
    token.chars().filter(|c| *c == quote).count() % 2 == 1
}

fn advance_past_config_value(tokens: &[&str], start: usize) -> usize {
    if start >= tokens.len() {
        return start;
    }

    let mut idx = start;
    let mut in_single = false;
    let mut in_double = false;

    while idx < tokens.len() {
        let token = tokens[idx];
        if token_has_unclosed_quote(token, '\'') {
            in_single = !in_single;
        }
        if token_has_unclosed_quote(token, '"') {
            in_double = !in_double;
        }
        idx += 1;
        if !in_single && !in_double {
            break;
        }
    }

    idx
}

fn codex_option_takes_value(token: &str) -> bool {
    matches!(
        token.to_ascii_lowercase().as_str(),
        "-c" | "--config"
            | "--enable"
            | "--disable"
            | "--remote"
            | "--remote-auth-token-env"
            | "-i"
            | "--image"
            | "-m"
            | "--model"
            | "--local-provider"
            | "-p"
            | "--profile"
            | "-s"
            | "--sandbox"
            | "-a"
            | "--ask-for-approval"
            | "--cd"
            | "--add-dir"
    )
}

fn codex_has_explicit_subcommand(tokens: &[&str], start: usize) -> bool {
    const CODEX_SUBCOMMANDS: &[&str] = &[
        "exec",
        "e",
        "review",
        "login",
        "logout",
        "mcp",
        "marketplace",
        "mcp-server",
        "app-server",
        "completion",
        "sandbox",
        "debug",
        "apply",
        "a",
        "resume",
        "fork",
        "cloud",
        "exec-server",
        "features",
        "help",
    ];

    let mut idx = start;
    while idx < tokens.len() {
        let token = tokens[idx];
        if token.eq_ignore_ascii_case("-c") || token.eq_ignore_ascii_case("--config") {
            idx = advance_past_config_value(tokens, idx + 1);
            continue;
        }
        if codex_option_takes_value(token) {
            idx += 2;
            continue;
        }
        if token.starts_with('-') {
            idx += 1;
            continue;
        }
        return CODEX_SUBCOMMANDS
            .iter()
            .any(|subcommand| token.eq_ignore_ascii_case(subcommand));
    }

    false
}

fn codex_tokens_have_resume(tokens: &[&str], start: usize) -> bool {
    let mut idx = start;
    while idx < tokens.len() {
        let token = tokens[idx];
        if token.eq_ignore_ascii_case("-c") || token.eq_ignore_ascii_case("--config") {
            idx = advance_past_config_value(tokens, idx + 1);
            continue;
        }
        if token.eq_ignore_ascii_case("resume") || token.eq_ignore_ascii_case("--last") {
            return true;
        }
        idx += 1;
    }
    false
}

fn antigravity_tokens_have_resume(tokens: &[&str], start: usize) -> bool {
    tokens[start..].iter().any(|t| {
        let lower = t.to_lowercase();
        lower == "--continue"
            || lower == "-c"
            || lower == "--conversation"
            || lower.starts_with("--conversation=")
    })
}

fn inject_antigravity_resume(shell: &str, shell_args: &mut Vec<String>) -> bool {
    // #260/#1482 — resume token sourced from the CodingAgentProfile (single
    // source of truth). G6: slice-pattern destructure, never index — a future
    // <1-element slice degrades gracefully instead of panicking in release.
    let &[resume_token] = CodingAgentKind::Antigravity.profile().resume_tokens else {
        debug_assert!(
            false,
            "Antigravity resume_tokens must have exactly 1 element"
        );
        return false;
    };
    match executable_basename(shell).as_str() {
        "agy" | "antigravity" => {
            let tokens: Vec<&str> = shell_args.iter().map(String::as_str).collect();
            if antigravity_tokens_have_resume(&tokens, 0) {
                return false;
            }
            shell_args.insert(0, resume_token.to_string());
            true
        }
        "cmd" => {
            // Tokenized and embedded forms, structurally mirroring the deleted
            // inject_gemini_resume cmd arm, but searching for a token whose
            // executable basename is exactly "agy" | "antigravity".
            let is_antigravity_executable =
                |arg: &str| matches!(executable_basename(arg).as_str(), "agy" | "antigravity");
            if let Some(idx) = shell_args
                .iter()
                .position(|arg| is_antigravity_executable(arg))
            {
                let tokens: Vec<&str> = shell_args.iter().map(|arg| arg.as_str()).collect();
                if antigravity_tokens_have_resume(&tokens, idx + 1) {
                    return false;
                }
                shell_args.insert(idx + 1, resume_token.to_string());
                return true;
            }

            for arg in shell_args.iter_mut() {
                let mut tokens: Vec<String> = arg
                    .split_whitespace()
                    .map(|token| token.to_string())
                    .collect();
                if let Some(idx) = tokens
                    .iter()
                    .position(|token| is_antigravity_executable(token))
                {
                    let token_refs: Vec<&str> = tokens.iter().map(|token| token.as_str()).collect();
                    if antigravity_tokens_have_resume(&token_refs, idx + 1) {
                        return false;
                    }
                    tokens.insert(idx + 1, resume_token.to_string());
                    *arg = tokens.join(" ");
                    return true;
                }
            }

            false
        }
        _ => false,
    }
}

fn inject_codex_resume(shell: &str, shell_args: &mut Vec<String>) -> bool {
    // #260 — resume tokens sourced from the CodingAgentProfile (single source
    // of truth). G6: slice-pattern destructure, never index — a future
    // <2-element slice degrades gracefully instead of panicking in release.
    let &[resume_subcmd, resume_flag] = CodingAgentKind::Codex.profile().resume_tokens else {
        debug_assert!(false, "Codex resume_tokens must have exactly 2 elements");
        return false;
    };
    match executable_basename(shell).as_str() {
        "codex" => {
            let tokens: Vec<&str> = shell_args.iter().map(|arg| arg.as_str()).collect();
            if codex_tokens_have_resume(&tokens, 0) || codex_has_explicit_subcommand(&tokens, 0) {
                return false;
            }
            shell_args.insert(0, resume_subcmd.to_string());
            shell_args.insert(1, resume_flag.to_string());
            true
        }
        "cmd" => {
            if let Some(idx) = shell_args
                .iter()
                .position(|arg| executable_basename(arg) == "codex")
            {
                let tokens: Vec<&str> = shell_args.iter().map(|arg| arg.as_str()).collect();
                if codex_tokens_have_resume(&tokens, idx + 1)
                    || codex_has_explicit_subcommand(&tokens, idx + 1)
                {
                    return false;
                }
                shell_args.insert(idx + 1, resume_subcmd.to_string());
                shell_args.insert(idx + 2, resume_flag.to_string());
                return true;
            }

            for arg in shell_args.iter_mut() {
                let mut tokens: Vec<String> = arg
                    .split_whitespace()
                    .map(|token| token.to_string())
                    .collect();
                if let Some(idx) = tokens
                    .iter()
                    .position(|token| executable_basename(token) == "codex")
                {
                    let token_refs: Vec<&str> = tokens.iter().map(|token| token.as_str()).collect();
                    if codex_tokens_have_resume(&token_refs, idx + 1)
                        || codex_has_explicit_subcommand(&token_refs, idx + 1)
                    {
                        return false;
                    }
                    tokens.insert(idx + 1, resume_subcmd.to_string());
                    tokens.insert(idx + 2, resume_flag.to_string());
                    *arg = tokens.join(" ");
                    return true;
                }
            }

            false
        }
        _ => false,
    }
}

fn pi_has_explicit_session_control(option_tokens: &[String]) -> bool {
    const SHORT_CONTROLS: &[&str] = &["-c", "-r"];
    const LONG_CONTROLS: &[&str] = &[
        "--continue",
        "--resume",
        "--session",
        "--session-id",
        "--fork",
        "--no-session",
    ];

    option_tokens.iter().any(|token| {
        SHORT_CONTROLS.contains(&token.as_str())
            || LONG_CONTROLS.iter().any(|name| {
                token == *name
                    || token
                        .strip_prefix(*name)
                        .is_some_and(|suffix| suffix.starts_with('='))
            })
    })
}

fn pi_is_non_conversation_invocation(option_tokens: &[String]) -> bool {
    const MANAGEMENT_COMMANDS: &[&str] =
        &["install", "remove", "uninstall", "update", "list", "config"];
    const ONE_SHOT_FLAGS: &[&str] = &["--help", "-h", "--version", "-v"];
    const JOINABLE_ONE_SHOT_FLAGS: &[&str] = &["--export", "--list-models"];

    option_tokens
        .first()
        .is_some_and(|token| MANAGEMENT_COMMANDS.contains(&token.as_str()))
        || option_tokens.iter().any(|token| {
            ONE_SHOT_FLAGS.contains(&token.as_str())
                || JOINABLE_ONE_SHOT_FLAGS.iter().any(|name| {
                    token == *name
                        || token
                            .strip_prefix(*name)
                            .is_some_and(|suffix| suffix.starts_with('='))
                })
        })
}

fn inject_pi_resume(shell: &str, shell_args: &mut Vec<String>) -> bool {
    let &[resume_token] = CodingAgentKind::Pi.profile().resume_tokens else {
        debug_assert!(false, "Pi resume_tokens must have exactly 1 element");
        return false;
    };
    let Ok(Some(location)) = locate_pi_command(shell, shell_args) else {
        return false;
    };
    if pi_has_explicit_session_control(&location.option_tokens)
        || pi_is_non_conversation_invocation(&location.option_tokens)
    {
        return false;
    }

    match location.insertion {
        PiInsertionPoint::Arg { index } => {
            if index > shell_args.len() {
                return false;
            }
            shell_args.insert(index, resume_token.to_string());
        }
        PiInsertionPoint::CmdText {
            arg_index,
            executable_range,
            segment_range,
        } => {
            let Some(text) = shell_args.get_mut(arg_index) else {
                return false;
            };
            if segment_range.start != executable_range.start
                || executable_range.start > executable_range.end
                || executable_range.end > segment_range.end
                || segment_range.end > text.len()
                || !text.is_char_boundary(executable_range.start)
                || !text.is_char_boundary(executable_range.end)
                || !text.is_char_boundary(segment_range.start)
                || !text.is_char_boundary(segment_range.end)
            {
                return false;
            }
            let insertion = format!(" {resume_token}");
            text.insert_str(executable_range.end, &insertion);
        }
    }
    true
}

fn maybe_inject_pi_resume(
    agent_kind: Option<CodingAgentKind>,
    resolved_spawn: Option<&AgentSpawnCommand>,
    skip_auto_resume: bool,
    shell: &str,
    shell_args: &mut Vec<String>,
) -> bool {
    if agent_kind != Some(CodingAgentKind::Pi) || resolved_spawn.is_none() || skip_auto_resume {
        return false;
    }
    inject_pi_resume(shell, shell_args)
}

/// #1873 - pure eligibility predicate for Muse workspace-latest auto-resume.
/// True only when EVERY condition holds:
/// - a resolved `AgentSpawnCommand` is present (configured-spawn provenance);
/// - the actual `shell`/`shell_args` equal that resolved recipe exactly;
/// - the resolved backend is `LocalProcess`;
/// - the compiled host is macOS or Linux;
/// - `Path::file_name` of the direct shell is the exact, case-sensitive `muse`;
/// - the configured argument vector is EMPTY.
///
/// Empty argv is the precedence rule: any configured argument (resume, --last,
/// UUID, prompt, --no-session-log, --workspace/--root, help/version, or a future
/// flag) wins and suppresses AC injection; AC neither validates nor rewrites it.
/// Ad-hoc launches, prefix names, wrappers, recipe mismatches, Windows, and
/// containers are all `false`. Shared with `loops::delivery` (an existing module
/// arc) so the cold-loop fresh rule and the launch seam agree.
pub(crate) fn trusted_muse_auto_resume_spawn(
    resolved_spawn: Option<&AgentSpawnCommand>,
    shell: &str,
    shell_args: &[String],
) -> bool {
    let Some(spawn) = resolved_spawn else {
        return false;
    };
    if spawn.shell != shell || spawn.shell_args != shell_args {
        return false;
    }
    if SessionBackendKind::from(&spawn.backend) != SessionBackendKind::LocalProcess {
        return false;
    }
    if !cfg!(any(target_os = "macos", target_os = "linux")) {
        return false;
    }
    if std::path::Path::new(shell)
        .file_name()
        .and_then(|name| name.to_str())
        != Some("muse")
    {
        return false;
    }
    shell_args.is_empty()
}

/// #1873 - append the Muse profile's resume tokens to the LOCAL effective vector
/// only. Requires detected Muse, resume intent (`skip_auto_resume == false`) and
/// the trusted predicate above. Slice-destructures the profile tokens: a wrong
/// arity, or a second call (the vector is no longer empty), fails closed with no
/// mutation. `Session.shell_args` (configured) is never touched.
fn maybe_inject_muse_resume(
    agent_kind: Option<CodingAgentKind>,
    resolved_spawn: Option<&AgentSpawnCommand>,
    skip_auto_resume: bool,
    shell: &str,
    shell_args: &mut Vec<String>,
) -> bool {
    if agent_kind != Some(CodingAgentKind::Muse) || skip_auto_resume {
        return false;
    }
    if !trusted_muse_auto_resume_spawn(resolved_spawn, shell, shell_args) {
        return false;
    }
    let &[resume_subcmd, resume_flag] = CodingAgentKind::Muse.profile().resume_tokens else {
        debug_assert!(false, "Muse resume_tokens must have exactly 2 elements");
        return false;
    };
    shell_args.push(resume_subcmd.to_string());
    shell_args.push(resume_flag.to_string());
    true
}

/// Single-pass expansion of `%NAME%` (cmd) and `$env:NAME` (PowerShell)
/// environment-variable references against `std::env::var`. Unknown names
/// are preserved literally, so a downstream `is_dir()` check returns
/// false rather than silently mis-resolving. Names must be ASCII
/// alphanumeric or `_`; anything else terminates the name.
///
/// Limitations (acceptable for real-world wrappers):
///   - No nested expansion: `%A%` whose value contains `%B%` is not re-expanded.
///   - No escape syntax (cmd's `^%`, PowerShell's backtick); wrappers don't use these.
///
/// (#599 R2) Lifted to module scope (was nested in `resolve_claude_projects_dir`)
/// so `claude_projects_dir_for_config_dir` can reuse it for the env-layer probe.
fn expand_env_var_refs(input: &str) -> String {
    // Pass 1: %NAME% (cmd-style).
    let mut buf = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(start) = rest.find('%') {
        buf.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        match after.find('%') {
            Some(end) => {
                let name = &after[..end];
                let valid =
                    !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
                if valid {
                    if let Ok(v) = std::env::var(name) {
                        buf.push_str(&v);
                    } else {
                        buf.push('%');
                        buf.push_str(name);
                        buf.push('%');
                    }
                } else {
                    // Not a valid var name (e.g. "100%" literal); preserve.
                    buf.push('%');
                    buf.push_str(name);
                    buf.push('%');
                }
                rest = &after[end + 1..];
            }
            None => {
                buf.push('%');
                buf.push_str(after);
                rest = "";
                break;
            }
        }
    }
    buf.push_str(rest);

    // Pass 2: $env:NAME (PowerShell-style). Case-insensitive prefix; name
    // terminates at the first byte that is not [A-Za-z0-9_].
    let mut out = String::with_capacity(buf.len());
    let bytes = buf.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let remaining = &buf[i..];
        if remaining.len() >= 5 && remaining.as_bytes()[..5].eq_ignore_ascii_case(b"$env:") {
            let name_start = i + 5;
            let mut name_end = name_start;
            while name_end < bytes.len() {
                let c = bytes[name_end];
                if c.is_ascii_alphanumeric() || c == b'_' {
                    name_end += 1;
                } else {
                    break;
                }
            }
            if name_end > name_start {
                let name = &buf[name_start..name_end];
                if let Ok(v) = std::env::var(name) {
                    out.push_str(&v);
                } else {
                    out.push_str(&buf[i..name_end]);
                }
                i = name_end;
                continue;
            }
        }
        // Default: copy one full UTF-8 char.
        let ch = buf[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

/// Resolve the directory where Claude Code stores its project transcripts for
/// `cwd`, taking `CLAUDE_CONFIG_DIR` overrides set by `.cmd`/`.bat`/`.ps1`/`.sh`
/// wrapper scripts into account.
///
/// Background: a user can put a wrapper like `claude-mb.cmd` on `%PATH%`:
///
/// ```bat
/// @echo off
/// set CLAUDE_CONFIG_DIR=C:\Users\maria\.claude-mb
/// claude %*
/// ```
///
/// Real Claude then writes project transcripts under
/// `C:\Users\maria\.claude-mb\projects\<mangled-cwd>`, NOT
/// `~/.claude/projects/<mangled-cwd>`. This helper finds the right base.
///
/// Returns `Some(<base>/projects/<mangled-cwd>)` when a Claude-family token
/// exists in the launch command, else `None`. Falls back to `~/.claude/...`
/// whenever the wrapper cannot be resolved or parsed; this preserves the
/// pre-#186 default-install behavior exactly.
///
/// Visibility: `pub(crate)` so the Telegram JSONL watcher follow-up can reuse
/// the same resolver without a private→pub(crate) refactor (see plan §6.3).
pub(crate) fn resolve_claude_projects_dir(
    shell: &str,
    shell_args: &[String],
    cwd: &str,
) -> Option<std::path::PathBuf> {
    resolve_claude_projects_dir_in(
        shell,
        shell_args,
        cwd,
        &crate::config::agent_path::effective_search_path(),
    )
}

/// #2589 - `resolve_claude_projects_dir` resolving a bare wrapper token through
/// an explicit search path.
pub(crate) fn resolve_claude_projects_dir_in(
    shell: &str,
    shell_args: &[String],
    cwd: &str,
    search_path: &std::ffi::OsStr,
) -> Option<std::path::PathBuf> {
    use std::path::{Path, PathBuf};

    fn default_base() -> Option<PathBuf> {
        dirs::home_dir().map(|h| h.join(".claude"))
    }

    fn strip_ascii_prefix_ci<'a>(haystack: &'a str, needle: &str) -> Option<&'a str> {
        if haystack.len() < needle.len() {
            return None;
        }
        if haystack.as_bytes()[..needle.len()].eq_ignore_ascii_case(needle.as_bytes()) {
            Some(&haystack[needle.len()..])
        } else {
            None
        }
    }

    fn parse_config_dir_from_wrapper(path: &Path) -> Option<PathBuf> {
        // Cap read at 64 KiB; real wrappers are < 1 KiB. Refusing larger
        // files protects against accidentally treating an exe-renamed-as-cmd
        // as a wrapper.
        const MAX: u64 = 64 * 1024;
        let metadata = std::fs::metadata(path).ok()?;
        if metadata.len() > MAX {
            return None;
        }
        let bytes = std::fs::read(path).ok()?;
        // Strip UTF-8 BOM if present; tolerate non-UTF-8 by lossy decode.
        let text_bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(&bytes);
        let text = String::from_utf8_lossy(text_bytes);

        for raw_line in text.lines() {
            let line = raw_line.trim_start();

            // Strip optional shell-prefix introducer:
            //   `cmd`/`.bat`: `set CLAUDE_CONFIG_DIR=...`
            //   `cmd`/`.bat`: `set "CLAUDE_CONFIG_DIR=..."`     (cmd-quoted whole-assignment)
            //   `.ps1`:       `$env:CLAUDE_CONFIG_DIR = ...`
            //   `.sh`:        `export CLAUDE_CONFIG_DIR=...`
            //   Bare:         `CLAUDE_CONFIG_DIR=...`
            let after_prefix = if let Some(rest) = strip_ascii_prefix_ci(line, "set ") {
                rest.trim_start()
            } else if let Some(rest) = strip_ascii_prefix_ci(line, "$env:") {
                rest.trim_start()
            } else if let Some(rest) = strip_ascii_prefix_ci(line, "export ") {
                rest.trim_start()
            } else {
                line
            };

            // Detect cmd's whole-assignment quoting: `set "VAR=value"`. After the
            // `set ` strip we may be sitting on a leading `"`; if so, the matching
            // closing `"` terminates the value (rather than wrapping it).
            let (after_open_quote, cmd_quoted) = match after_prefix.strip_prefix('"') {
                Some(rest) => (rest, true),
                None => (after_prefix, false),
            };

            let Some(rest) = strip_ascii_prefix_ci(after_open_quote, "CLAUDE_CONFIG_DIR") else {
                continue;
            };
            let rest = rest.trim_start();
            let Some(rest) = rest.strip_prefix('=') else {
                continue;
            };
            let value = rest.trim();

            // Strip surrounding quotes. Two flavors:
            //   (a) cmd whole-assignment: `set "VAR=value"`     → consume trailing `"`
            //   (b) value-quoted:         `set VAR="value"` or `'value'` → strip matched pair
            let unquoted: &str = if cmd_quoted {
                value.strip_suffix('"').unwrap_or(value)
            } else if value.len() >= 2
                && ((value.starts_with('"') && value.ends_with('"'))
                    || (value.starts_with('\'') && value.ends_with('\'')))
            {
                &value[1..value.len() - 1]
            } else {
                value
            };

            if unquoted.is_empty() {
                return None;
            }
            let expanded = expand_env_var_refs(unquoted);
            return Some(PathBuf::from(expanded));
        }
        None
    }

    fn looks_like_wrapper_extension(path: &Path) -> bool {
        path.extension()
            .and_then(|e| e.to_str())
            .map(|e| {
                let lc = e.to_ascii_lowercase();
                matches!(lc.as_str(), "cmd" | "bat" | "ps1" | "sh")
            })
            .unwrap_or(false)
    }

    // Find the first token whose basename starts with "claude" across
    // shell + shell_args, splitting each arg on whitespace so cmd-wrapped
    // strings ("git pull && claude-mb -x") are also covered.
    let claude_token: Option<String> = {
        let mut direct = std::iter::once(shell.to_string()).chain(
            shell_args
                .iter()
                .flat_map(|a| a.split_whitespace().map(str::to_string)),
        );
        direct.find(|t| executable_basename(t).starts_with("claude"))
    };
    let claude_token = claude_token?;

    let mangled = crate::session::session::mangle_cwd_for_claude(cwd);

    // Stem == "claude" → no wrapper, default base. This covers `claude`,
    // `claude.exe`, `C:\Tools\claude.cmd` (where the .cmd is the official
    // installer's launcher and writes nothing of its own), etc.
    // executable_basename returns a lowercased stem; comparison is case-insensitive by
    // construction.
    if executable_basename(&claude_token) == "claude" {
        return default_base().map(|base| base.join("projects").join(&mangled));
    }

    // Non-default name (e.g. `claude-mb`). Try to resolve to an actual file
    // and parse it for a CLAUDE_CONFIG_DIR override.
    if let Some(file) = crate::config::agent_command::resolve_program_in(&claude_token, search_path)
    {
        if looks_like_wrapper_extension(&file) {
            if let Some(custom_base) = parse_config_dir_from_wrapper(&file) {
                return Some(custom_base.join("projects").join(&mangled));
            }
        }
    }

    // Fall back to default base. Preserves pre-fix behavior whenever the
    // wrapper is missing, unreadable, has no `CLAUDE_CONFIG_DIR` line, or
    // points at a non-text extension.
    default_base().map(|base| base.join("projects").join(&mangled))
}

/// (#599 R2) Claude `projects/<mangled-cwd>` dir for an explicit
/// `CLAUDE_CONFIG_DIR` taken from the profile/agent env layer. That layer is
/// what the spawned Claude actually sees: the env approach launches a plain
/// `claude`, so `resolve_claude_projects_dir` short-circuits to `~/.claude`
/// (its `file_stem == "claude"` fast path) and misses env-relocated
/// transcripts. The value is already AC-placeholder-expanded by the spawn
/// builder; `expand_env_var_refs` additionally resolves any residual
/// `%VAR%` / `$env:VAR` for parity with the wrapper path.
fn claude_projects_dir_for_config_dir(config_dir: &str, cwd: &str) -> std::path::PathBuf {
    let base = std::path::PathBuf::from(expand_env_var_refs(config_dir));
    let mangled = crate::session::session::mangle_cwd_for_claude(cwd);
    base.join("projects").join(mangled)
}

#[derive(Debug, Clone)]
struct ResumeProbeTarget {
    host_probe_path: Option<PathBuf>,
    filesystem: &'static str,
    warning: Option<ContainerEnvWarning>,
}

#[derive(Debug, Clone)]
struct ContainerPathContext {
    host_root: String,
    map: ContainerPathMap,
}

fn container_path_context_for_cwd(cwd: &str) -> Result<ContainerPathContext, String> {
    let canonical_host_root = std::fs::canonicalize(cwd)
        .map(|path| crate::path_utils::path_to_string_without_windows_verbatim_prefix(&path))
        .map_err(|err| {
            format!(
                "failed to canonicalize container mount source '{}': {}",
                cwd, err
            )
        })?;
    if let Some(reason) = crate::pty::container_paths::container_mount_source_rejection(
        std::path::Path::new(&canonical_host_root),
    ) {
        return Err(container_mount_rejection_message(
            cwd,
            &canonical_host_root,
            reason,
        ));
    };
    let map = ContainerPathMap::new(&canonical_host_root, DEFAULT_CONTAINER_WORKDIR)?;
    Ok(ContainerPathContext {
        host_root: canonical_host_root,
        map,
    })
}

fn container_mount_rejection_message(selected: &str, canonical: &str, reason: String) -> String {
    if selected == canonical {
        reason
    } else {
        format!(
            "{} (selected path '{}', canonical path '{}')",
            reason, selected, canonical
        )
    }
}

fn effective_codex_home_for_backend(
    backend_kind: SessionBackendKind,
    container_map: Option<&ContainerPathMap>,
    spawn: &AgentSpawnCommand,
) -> Option<String> {
    let raw_home = spawn.effective_codex_home.as_ref()?;
    let raw_home =
        crate::path_utils::path_to_string_without_windows_verbatim_prefix(raw_home.as_path());
    match backend_kind {
        SessionBackendKind::LocalProcess => Some(raw_home),
        SessionBackendKind::ContainerTransport => {
            let map = container_map?;
            let container_home = container_config_dir(map, &raw_home)?;
            map.to_host(&container_home).map(|path| {
                crate::path_utils::path_to_string_without_windows_verbatim_prefix(path.as_path())
            })
        }
    }
}

fn resume_probe_target(
    backend_kind: SessionBackendKind,
    container_map: Option<&ContainerPathMap>,
    resolved_spawn: Option<&AgentSpawnCommand>,
    injected_claude_config_dir: Option<&str>,
    shell: &str,
    shell_args: &[String],
    cwd: &str,
) -> ResumeProbeTarget {
    // #930 - an injected copy-in default (host path under the replica mount) wins
    // only when the user set no CLAUDE_CONFIG_DIR; the caller enforces that
    // precondition, so fall back to the user's effective value otherwise.
    let claude_config_dir_override = injected_claude_config_dir
        .or_else(|| resolved_spawn.and_then(|s| s.effective_env_value(CLAUDE_CONFIG_DIR_KEY)));
    resume_probe_target_for_config_dir(
        backend_kind,
        container_map,
        claude_config_dir_override,
        shell,
        shell_args,
        cwd,
    )
}

#[allow(clippy::too_many_arguments)]
fn claude_resume_probe_target_for_kind(
    agent_kind: Option<CodingAgentKind>,
    backend_kind: SessionBackendKind,
    container_map: Option<&ContainerPathMap>,
    resolved_spawn: Option<&AgentSpawnCommand>,
    injected_claude_config_dir: Option<&str>,
    shell: &str,
    shell_args: &[String],
    cwd: &str,
) -> Option<ResumeProbeTarget> {
    if agent_kind != Some(CodingAgentKind::Claude) {
        return None;
    }
    Some(resume_probe_target(
        backend_kind,
        container_map,
        resolved_spawn,
        injected_claude_config_dir,
        shell,
        shell_args,
        cwd,
    ))
}

/// #930 - the CLAUDE_CONFIG_DIR value to inject for a container coding agent whose
/// host credentials we will copy. Returns the copy directory (a host path under
/// the replica mount, which the container env translation later maps to
/// `/workspace/.claude`) ONLY when a copy plan exists AND the user configured no
/// CLAUDE_CONFIG_DIR (`user_has_claude_config_dir == false`). `None` => inject
/// nothing (host-login-reuse off, no host credentials, or an explicit user
/// value/removal we must not override).
fn injected_claude_config_dir_for_copy(
    plan: Option<&crate::pty::container_credentials::ContainerCredentialPlan>,
    user_has_claude_config_dir: bool,
) -> Option<String> {
    if user_has_claude_config_dir {
        return None;
    }
    plan?
        .dest
        .parent()
        .map(crate::path_utils::path_to_string_without_windows_verbatim_prefix)
}

fn resume_probe_target_for_config_dir(
    backend_kind: SessionBackendKind,
    container_map: Option<&ContainerPathMap>,
    claude_config_dir_override: Option<&str>,
    shell: &str,
    shell_args: &[String],
    cwd: &str,
) -> ResumeProbeTarget {
    match backend_kind {
        SessionBackendKind::LocalProcess => {
            let host_probe_path = match claude_config_dir_override {
                Some(dir) => Some(claude_projects_dir_for_config_dir(dir, cwd)),
                None => resolve_claude_projects_dir(shell, shell_args, cwd),
            };
            ResumeProbeTarget {
                host_probe_path,
                filesystem: "host",
                warning: None,
            }
        }
        SessionBackendKind::ContainerTransport => {
            let Some(map) = container_map else {
                return ResumeProbeTarget {
                    host_probe_path: None,
                    filesystem: "container-unreachable",
                    warning: Some(claude_config_dir_no_value_warning()),
                };
            };
            let Some(raw_config_dir) = claude_config_dir_override else {
                return ResumeProbeTarget {
                    host_probe_path: None,
                    filesystem: "container-unreachable",
                    warning: Some(claude_config_dir_no_value_warning()),
                };
            };
            let Some(container_config_dir) = container_config_dir(map, raw_config_dir) else {
                return ResumeProbeTarget {
                    host_probe_path: None,
                    filesystem: "container-unreachable",
                    warning: None,
                };
            };
            let container_projects_path = format!(
                "{}/projects/{}",
                container_config_dir.trim_end_matches('/'),
                crate::session::session::mangle_cwd_for_claude(DEFAULT_CONTAINER_WORKDIR)
            );
            ResumeProbeTarget {
                host_probe_path: map.to_host(&container_projects_path),
                filesystem: "container-via-mount",
                warning: None,
            }
        }
    }
}

/// Decide whether to auto-inject `--continue` for a Claude session.
/// Pure function: no filesystem access. Caller is responsible for resolving
/// `claude_project_exists` (typically `~/.claude/projects/<mangled-cwd>/.is_dir()`).
///
/// Callers should compute `claude_project_exists` via `resolve_claude_projects_dir`
/// to honor wrapper-set `CLAUDE_CONFIG_DIR`. (#599 R2) The caller additionally
/// honors the profile/agent env-layer `CLAUDE_CONFIG_DIR` first via
/// `claude_projects_dir_for_config_dir`; this function itself is unchanged.
/// Note: a wrapper named exactly
/// `claude.cmd` / `claude.exe` / `claude.ps1` that overrides `CLAUDE_CONFIG_DIR`
/// is intentionally NOT honored — the resolver short-circuits when the file_stem
/// equals `claude`. Users who need wrapper overrides should rename to
/// `claude-<suffix>` (e.g. `claude-mb`).
///
/// Returns `true` only when ALL of:
///   - the session is a Claude variant
///   - the caller has not requested skip
///   - the projects dir exists on disk
///   - the configured argv does not already contain `--continue`,
///     `--continue=<value>`, or `-c` (case-insensitive token match against
///     each whitespace-split token of `full_cmd`)
///
/// Note: `-c` is also Codex's short form for `--config` (e.g.,
/// `codex -c key=value`). In compound commands that mix `codex` and `claude`
/// (e.g., `cmd /K codex -c k=v && claude`), the `-c` from codex's tokens will
/// suppress Claude's `--continue` injection. Pre-existing behavior; documented
/// here so refactors do not silently lose it.
fn should_inject_continue(
    is_claude: bool,
    skip_auto_resume: bool,
    claude_project_exists: bool,
    full_cmd: &str,
) -> bool {
    if !is_claude || skip_auto_resume || !claude_project_exists {
        return false;
    }
    let already_has_continue = full_cmd.split_whitespace().any(|t| {
        let lower = t.to_lowercase();
        lower == "--continue" || lower.starts_with("--continue=") || lower == "-c"
    });
    !already_has_continue
}

/// Inspect the configured argv before AC adds its own launch controls. No
/// injection can also mean a first conversation, which must remain retainable.
fn configured_prompt_identity(
    kind: Option<CodingAgentKind>,
    shell: &str,
    shell_args: &[String],
) -> bool {
    let tokens: Vec<&str> = shell_args
        .iter()
        .flat_map(|arg| arg.split_whitespace())
        .collect();
    match kind {
        Some(CodingAgentKind::Claude) => {
            let full_cmd = format!("{} {}", shell, shell_args.join(" "));
            !should_inject_fresh_session_id(true, true, &full_cmd)
        }
        Some(CodingAgentKind::Codex) => {
            codex_tokens_have_resume(&tokens, 0)
                || (codex_has_explicit_subcommand(&tokens, 0)
                    && tokens
                        .iter()
                        .any(|token| token.eq_ignore_ascii_case("fork")))
        }
        Some(CodingAgentKind::Antigravity) => antigravity_tokens_have_resume(&tokens, 0),
        Some(CodingAgentKind::Pi) => locate_pi_command(shell, shell_args)
            .ok()
            .flatten()
            .is_some_and(|location| pi_has_explicit_session_control(&location.option_tokens)),
        Some(CodingAgentKind::Muse) => tokens.iter().any(|token| {
            CodingAgentKind::Muse
                .profile()
                .resume_tokens
                .iter()
                .any(|control| token.eq_ignore_ascii_case(control))
        }),
        _ => false,
    }
}

/// (#756) Decide whether to pass a launcher-minted `--session-id <uuid>` to a
/// Claude spawn. Pure function; mirrors `should_inject_continue`'s token scan.
///
/// Returns true only when BOTH:
///   - the session is a Claude variant, and
///   - AC is deliberately NOT resuming (`skip_auto_resume` true: fresh create
///     default, restart-fresh, or the #756 mirror guard),
///
/// and the configured argv does not already steer session identity:
/// `--session-id[=]`, `--resume[=]`/`-r`, `--continue[=]`/`-c`,
/// `--fork-session` (case-insensitive token match). User args win; stacking
/// identity flags is a HARD CLI error (Q2-verified: `--session-id` combined
/// with `--continue`/`--resume` and no `--fork-session` errors at argv
/// parse), so this veto scan is mandatory.
///
/// Purpose: belt-and-suspenders freshness (immune to future provider
/// auto-resume surprises) and a known-AT-SPAWN transcript identity (hands the
/// telegram claude_watcher its file identity instead of cwd+mtime heuristics).
/// The injected id is the SPAWN id, not a stable session id: a later in-session
/// `/clear` mints a new one.
fn should_inject_fresh_session_id(is_claude: bool, skip_auto_resume: bool, full_cmd: &str) -> bool {
    if !is_claude || !skip_auto_resume {
        return false;
    }
    let has_identity_flag = full_cmd.split_whitespace().any(|t| {
        let lower = t.to_lowercase();
        lower == "--session-id"
            || lower.starts_with("--session-id=")
            || lower == "--resume"
            || lower.starts_with("--resume=")
            || lower == "-r"
            || lower == "--continue"
            || lower.starts_with("--continue=")
            || lower == "-c"
            || lower == "--fork-session"
    });
    !has_identity_flag
}

/// #2454: the transcript id an argv names, so the Claude reader can pin its
/// first attach to `<id>.jsonl` instead of the newest file by mtime.
///
/// Rules, in order: the value of `--session-id` (or `--session-id=<v>`), then
/// the value of `--resume` (or `--resume=<v>`). Flags match case-insensitively
/// and every argument is split on whitespace first, so the `cmd` spawn shape
/// (`"<...>claude --session-id <uuid>"` in one element) is covered. A value
/// that is not a UUID yields `None`; `--continue`, `-c` and `-r` name no file
/// and yield `None`.
pub(crate) fn transcript_id_from_args(args: &[String]) -> Option<String> {
    let tokens: Vec<&str> = args.iter().flat_map(|a| a.split_whitespace()).collect();
    identity_flag_value(&tokens, "--session-id")
        .or_else(|| identity_flag_value(&tokens, "--resume"))
}

/// The UUID value of the first `flag` in `tokens`, in either the spaced or the
/// `=` form. A missing or non-UUID value yields `None`.
fn identity_flag_value(tokens: &[&str], flag: &str) -> Option<String> {
    for (i, token) in tokens.iter().enumerate() {
        let lower = token.to_ascii_lowercase();
        let value = if lower == flag {
            tokens.get(i + 1).copied()
        } else if lower.starts_with(flag) && lower.as_bytes().get(flag.len()) == Some(&b'=') {
            token.get(flag.len() + 1..)
        } else {
            continue;
        };
        return value
            .and_then(|v| Uuid::parse_str(v).ok())
            .map(|id| id.to_string());
    }
    None
}

/// Issue #107 round 5 — build the optional title prompt, or `Ok(None)` if the
/// auto-title preconditions do not hold.
///
/// Synchronous: filesystem reads only, no PTY, no await, no snapshot.
/// (#137 introduced `task-set-title` which creates its own atomic backup;
/// the backend no longer snapshots before injection.)
///
/// The caller is the post-spawn task in `create_session_inner`; it injects this
/// prompt by itself. Credentials are not part of this payload.
///
/// Gates layered (in order):
///   1. workgroup TASK.md path resolvable from `cwd` → else `Err`
///      (config issue, F7 preserved).
///   2. TASK.md exists and read succeeds → else `Err`.
///   3. TASK.md non-empty (after trim) → else `Ok(None)` (silent skip).
///   4. No `title:` field in existing frontmatter → else `Ok(None)` (silent
///      skip).
///   5. Build title prompt with the absolute, UNC-stripped path (F4
///      preserved). Return `Ok(Some(prompt))`.
fn build_title_prompt_appendage(_cwd: &str) -> Result<Option<String>, String> {
    Ok(None)
}

/// #1063: test-only finite barriers that pause a real session create at the two
/// points a deletion race needs: after the pending row is inserted and before the
/// config-seed project gate (`before_project_gate`), and after the seed transaction
/// and before PTY spawn/finalization (`after_seed_before_pty`). Barriers are keyed
/// by the create's normalized working directory, so concurrently running create
/// tests never take each other's barrier, and they exist only in `#[cfg(test)]`
/// builds, so production create is unchanged and stays non-emitting.
#[cfg(test)]
pub(crate) mod seed_race_barriers;

/// (#2413) Test-only coordinator override for `create_session_inner_impl`.
/// `is_coordinator` comes from `discover_teams()`, which reads the real
/// settings, so a lib test cannot make a temp cwd a coordinator otherwise.
/// Keyed by the normalized cwd, like `seed_race_barriers`.
#[cfg(test)]
pub(crate) mod coordinator_create_seam;

/// Test-only rendezvous inside `execute_restart_transaction`, after the old
/// id's reader demands are released and before the replacement is created
/// (#2232 phase 4 test 9).
///
/// Test 9 needs to prove the post-commit sweep: an old-id raise installed
/// *after* the early release but *before* the commit must be gone once the
/// commit has removed the old row. This barrier makes that window
/// deterministic instead of timing-dependent.
#[cfg(test)]
pub(crate) mod restart_reader_seam;

/// Core session creation logic shared by the Tauri command and the restore path.
/// Creates a session record, spawns a PTY, and emits the session_created event.
/// Auto-detects agent from shell command if not provided, and auto-injects provider-specific
/// resume flags (Claude/Pi/Antigravity `--continue`, Codex `resume --last`, and for a
/// trusted empty-argv local macOS/Linux Muse recipe `resume --last`, #1873)
/// when appropriate.
/// If `skip_tooling_save` is true, skips writing to the repo's config.json (for temp sessions).
///
/// `skip_auto_resume` controls provider auto-resume injection:
/// - `true` — suppress all provider auto-resume. Use this for any "fresh
///   create" call site (UI/CLI/root-agent create, mailbox wake-from-cold
///   meaning no SessionManager record at this CWD, `restart_session` with
///   default semantics from `effective_restart_skip_auto_resume`).
/// - `false` — allow provider auto-resume. Use this only for paths restoring
///   a session AC already knows about (the startup-restore loop in `lib.rs`,
///   the wake-from-known-state branch in `mailbox::deliver_wake` — any
///   `RespawnExited` match, today driven exclusively by deferred-non-coord
///   `Exited(0)` records — and `restart_session` when its caller passes
///   `Some(false)`).
///
/// Owned launch inputs retained before a response replacement starts effects.
/// The executor alone supplies app state and a real lexical target proof.
#[derive(Debug, Clone)]
pub(crate) struct OwnedReplacementCreateRequest {
    pub(crate) shell: String,
    pub(crate) shell_args: Vec<String>,
    pub(crate) cwd: String,
    pub(crate) name: Option<String>,
    pub(crate) agent_id: Option<String>,
    pub(crate) agent_label: Option<String>,
    pub(crate) skip_tooling_save: bool,
    pub(crate) git_repos: Vec<SessionRepo>,
    pub(crate) resolved_spawn: Option<AgentSpawnCommand>,
    pub(crate) host_shell: Option<ResolvedAgentHostShell>,
}

#[derive(Debug)]
#[expect(
    clippy::large_enum_variant,
    reason = "Preserve owned finalization result layout; boxing adds an unnecessary allocation"
)]
pub(crate) enum OwnedReplacementCreateStep {
    Finalized(SessionInfo),
    Retained,
}

/// Owns the original locks only across the admitted test create cut. These
/// values never enter the retained operation record.
#[cfg(test)]
pub(crate) struct FixturePtyCutLease {
    pub(crate) gate: Arc<crate::api::message_store::PtyInputTargetGate>,
    pub(crate) target: String,
    pub(crate) exact: crate::api::message_store::PtyInputTargetGuard,
    pub(crate) stripe: crate::api::message_store::PtyInputStripeGuard,
}

#[cfg(test)]
pub(crate) enum FixtureReplacementLease {
    OrdinaryMoved(FixturePtyCutLease),
    PtyCutShared(Arc<FixturePtyCutLease>),
}

#[cfg(test)]
pub(crate) struct FixtureReplacementInput {
    pub(crate) request: OwnedReplacementCreateRequest,
    pub(crate) continuation: crate::session::selection::OwnedReplacementContinuation,
    pub(crate) lease: FixtureReplacementLease,
}

#[cfg(test)]
pub(crate) struct FixtureReplacementResult {
    pub(crate) outcome: Result<OwnedReplacementCreateStep, String>,
    pub(crate) ordinary_lease: Option<FixturePtyCutLease>,
}

#[cfg(test)]
impl std::fmt::Debug for FixtureReplacementResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FixtureReplacementResult")
            .field("outcome", &self.outcome)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
impl std::fmt::Debug for FixtureReplacementInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FixtureReplacementInput")
            .field("continuation", &self.continuation)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
pub(crate) async fn execute_inline_replacement_fixture<R: tauri::Runtime>(
    transaction: &SelectionTransaction<R>,
    input: FixtureReplacementInput,
) -> FixtureReplacementResult {
    let FixtureReplacementInput {
        request,
        continuation,
        lease,
    } = input;
    let observed = continuation.clone();
    if !observed
        .fixture_reached(crate::session::selection::ReplacementFixtureCut::BeforeSourceValidation)
        .await
    {
        return FixtureReplacementResult {
            outcome: Ok(OwnedReplacementCreateStep::Retained),
            ordinary_lease: match lease {
                FixtureReplacementLease::OrdinaryMoved(lease) => Some(lease),
                FixtureReplacementLease::PtyCutShared(lease) => {
                    drop(lease);
                    None
                }
            },
        };
    }
    let borrowed = match &lease {
        FixtureReplacementLease::OrdinaryMoved(lease) => lease,
        FixtureReplacementLease::PtyCutShared(lease) => lease.as_ref(),
    };
    let mut outcome =
        match borrowed
            .gate
            .target_ownership(&borrowed.target, &borrowed.stripe, &borrowed.exact)
        {
            Ok(proof) => {
                create_owned_response_replacement_in_context(
                    transaction.app(),
                    request,
                    continuation,
                    &proof,
                    Some((
                        transaction,
                        SelectionCause::SessionCreated(
                            crate::session::selection::TrustedCreateIntent::Background,
                        ),
                    )),
                )
                .await
            }
            Err(_) => Ok(OwnedReplacementCreateStep::Retained),
        };
    if !observed
        .fixture_reached(
            crate::session::selection::ReplacementFixtureCut::AdapterOutcomeBeforeLeaseDrop,
        )
        .await
        && outcome.is_ok()
    {
        outcome = Ok(OwnedReplacementCreateStep::Retained);
    }
    let ordinary_lease = match lease {
        FixtureReplacementLease::OrdinaryMoved(lease) => Some(lease),
        FixtureReplacementLease::PtyCutShared(lease) => {
            drop(lease);
            None
        }
    };
    FixtureReplacementResult {
        outcome,
        ordinary_lease,
    }
}

pub(crate) async fn create_owned_response_replacement<R: tauri::Runtime>(
    app: &AppHandle<R>,
    request: OwnedReplacementCreateRequest,
    continuation: crate::session::selection::OwnedReplacementContinuation,
    target: &crate::api::message_store::PtyInputTargetOwnership<'_>,
) -> Result<OwnedReplacementCreateStep, String> {
    create_owned_response_replacement_in_context(app, request, continuation, target, None).await
}

// An inline caller must supply its actual current selection turn and cause.
// Absence selects the existing external finalization ticket, never an inferred
// restore or lifecycle capability.
async fn create_owned_response_replacement_in_context<R: tauri::Runtime>(
    app: &AppHandle<R>,
    request: OwnedReplacementCreateRequest,
    continuation: crate::session::selection::OwnedReplacementContinuation,
    target: &crate::api::message_store::PtyInputTargetOwnership<'_>,
    inline: Option<(&SelectionTransaction<R>, SelectionCause)>,
) -> Result<OwnedReplacementCreateStep, String> {
    let manager = app
        .state::<Arc<tokio::sync::RwLock<SessionManager>>>()
        .inner()
        .clone();
    let pty = app.state::<Arc<Mutex<PtyManager>>>().inner().clone();
    let completion = create_session_inner_impl(
        app,
        &manager,
        &pty,
        request.shell,
        request.shell_args,
        request.cwd,
        request.name,
        request.agent_id,
        request.agent_label,
        request.skip_tooling_save,
        request.git_repos,
        false,
        request.resolved_spawn,
        request.host_shell,
        None,
        CreateSelectionIntent::Background,
        None,
        None,
        inline.as_ref().map(|(transaction, _)| *transaction),
        inline.map(|(_, cause)| cause),
        false,
        Some(target),
        crate::config::sessions_persistence::default_creation_gate_enforcement(),
        None,
        None,
        Some(continuation.clone()),
        None,
    )
    .await;
    let completion = match completion {
        Ok(completion) => completion,
        Err(error) => {
            let manager = manager.read().await.clone();
            continuation
                .rollback_failed_create(&manager, &pty, &error)
                .await;
            return Err(error);
        }
    };
    match completion {
        CreateCompletion::Finalized(info) => Ok(OwnedReplacementCreateStep::Finalized(info)),
        CreateCompletion::Retained => Ok(OwnedReplacementCreateStep::Retained),
        CreateCompletion::Deferred(_) => Err("create unexpectedly deferred finalization".into()),
    }
}

// Shared by Tauri command + restore path; collapsing args would force a context struct.
#[allow(clippy::too_many_arguments)]
pub async fn create_session_inner<R: tauri::Runtime>(
    app: &AppHandle<R>,
    session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
    pty_mgr: &Arc<Mutex<PtyManager>>,
    shell: String,
    shell_args: Vec<String>,
    cwd: String,
    session_name: Option<String>,
    agent_id: Option<String>,
    agent_label: Option<String>,
    skip_tooling_save: bool,
    git_repos: Vec<SessionRepo>,
    skip_auto_resume: bool,
    resolved_spawn: Option<AgentSpawnCommand>,
    resolved_agent_host_shell: Option<ResolvedAgentHostShell>,
    // #973 - the size the view has already fitted to, when the caller has a view at all.
    // `None` keeps AC's historical 120x30. See `PtyViewport`.
    viewport: Option<PtyViewport>,
    selection_intent: CreateSelectionIntent,
) -> Result<SessionInfo, String> {
    create_session_inner_impl(
        app,
        session_mgr,
        pty_mgr,
        shell,
        shell_args,
        cwd,
        session_name,
        agent_id,
        agent_label,
        skip_tooling_save,
        git_repos,
        skip_auto_resume,
        resolved_spawn,
        resolved_agent_host_shell,
        viewport,
        selection_intent,
        None,
        None,
        None,
        None,
        false,
        None,
        crate::config::sessions_persistence::default_creation_gate_enforcement(),
        None,
        None,
        None,
        None,
    )
    .await?
    .into_finalized()
    .await
}

#[derive(Debug)]
#[expect(
    clippy::large_enum_variant,
    reason = "Preserve PTY create result layout and unchanged completion consumers"
)]
pub(crate) enum PtyTargetCreateStep {
    Finalized(SessionInfo),
    Retained,
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn create_session_inner_with_pty_target_ownership<R: tauri::Runtime>(
    app: &AppHandle<R>,
    session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
    pty_mgr: &Arc<Mutex<PtyManager>>,
    shell: String,
    shell_args: Vec<String>,
    cwd: String,
    session_name: Option<String>,
    agent_id: Option<String>,
    agent_label: Option<String>,
    skip_tooling_save: bool,
    git_repos: Vec<SessionRepo>,
    skip_auto_resume: bool,
    resolved_spawn: Option<AgentSpawnCommand>,
    resolved_agent_host_shell: Option<ResolvedAgentHostShell>,
    viewport: Option<PtyViewport>,
    selection_intent: CreateSelectionIntent,
    target_ownership: &crate::api::message_store::PtyInputTargetOwnership<'_>,
) -> Result<PtyTargetCreateStep, String> {
    let completion = create_session_inner_impl(
        app,
        session_mgr,
        pty_mgr,
        shell,
        shell_args,
        cwd,
        session_name,
        agent_id,
        agent_label,
        skip_tooling_save,
        git_repos,
        skip_auto_resume,
        resolved_spawn,
        resolved_agent_host_shell,
        viewport,
        selection_intent,
        None,
        None,
        None,
        None,
        false,
        Some(target_ownership),
        crate::config::sessions_persistence::default_creation_gate_enforcement(),
        None,
        None,
        None,
        None,
    )
    .await?;
    match completion {
        CreateCompletion::Finalized(info) => Ok(PtyTargetCreateStep::Finalized(info)),
        CreateCompletion::Retained => Ok(PtyTargetCreateStep::Retained),
        CreateCompletion::Deferred(_) => Err("create unexpectedly deferred finalization".into()),
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn create_session_inner_for_restore<R: tauri::Runtime>(
    transaction: &SelectionTransaction<R>,
    session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
    pty_mgr: &Arc<Mutex<PtyManager>>,
    shell: String,
    shell_args: Vec<String>,
    cwd: String,
    session_name: Option<String>,
    agent_id: Option<String>,
    agent_label: Option<String>,
    skip_tooling_save: bool,
    git_repos: Vec<SessionRepo>,
    skip_auto_resume: bool,
    resolved_spawn: Option<AgentSpawnCommand>,
    resolved_agent_host_shell: Option<ResolvedAgentHostShell>,
    viewport: Option<PtyViewport>,
    pending_start_fresh: Option<bool>,
    pending_communication: Option<SessionCommunication>,
    coding_agent_binding: Option<crate::session::manager::CodingAgentBinding>,
    initial_last_prompt: Option<&crate::config::sessions_persistence::PersistedSession>,
) -> Result<SessionInfo, String> {
    create_session_inner_impl(
        transaction.app(),
        session_mgr,
        pty_mgr,
        shell,
        shell_args,
        cwd,
        session_name,
        agent_id,
        agent_label,
        skip_tooling_save,
        git_repos,
        skip_auto_resume,
        resolved_spawn,
        resolved_agent_host_shell,
        viewport,
        CreateSelectionIntent::Suppress,
        pending_start_fresh,
        pending_communication,
        Some(transaction),
        Some(SelectionCause::Restore),
        false,
        None,
        crate::config::sessions_persistence::default_creation_gate_enforcement(),
        coding_agent_binding,
        None,
        None,
        initial_last_prompt.map(InitialLastPrompt::from_persisted),
    )
    .await?
    .into_finalized()
    .await
}

/// #552 Coordinator create/reopen: seed the badge clock and clear both close markers.
/// #552 seed the badge clock for a coordinator's first spawn so it shows 0m
/// immediately, and clear any "auto-closed" marker. An auto-closed coordinator
/// is DESTROYED, so its reopen flows through this create path (the "create
/// in-place" branch of handleReplicaClick), NOT restart_session_inner.
/// seed_if_absent never overwrites, so a respawn does NOT reset the badge.
/// (#2413) Returns the create's marker ticket (`None` = no clocks state); the
/// caller ends it with `succeeded()` or `failed()` so a failed create puts the
/// cleared markers back.
pub(crate) fn note_coordinator_create_on_app<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    cwd: &str,
) -> Option<CoordinatorCreateTicket<R>> {
    let clocks = app.try_state::<crate::config::coordinator_clocks::CoordinatorClocksState>()?;
    {
        // agent_fqn_from_path returns String (teams.rs:80), not Option.
        let fqn = crate::config::teams::agent_fqn_from_path(cwd);
        let now = chrono::Utc::now();
        // Held through the emits below: a restore can never emit its timestamp
        // after this create's clear has emitted null.
        let _order = CLOSE_MARKER_EVENT_ORDER
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let (seeded, cleared_auto, cleared_manual) = {
            let mut g = clocks.lock().unwrap_or_else(|e| e.into_inner());
            let seeded = g.seed_if_absent(&fqn, now);
            let auto_before = g.auto_closed_at(&fqn);
            let manual_before = g.manually_closed_at(&fqn);
            let cleared_auto = g.clear_auto_closed(&fqn);
            let cleared_manual = g.clear_manually_closed(&fqn);
            g.begin_coordinator_create(
                &fqn,
                ClearedCloseMarkers {
                    auto_closed_at: auto_before.filter(|_| cleared_auto),
                    manually_closed_at: manual_before.filter(|_| cleared_manual),
                },
            );
            (seeded, cleared_auto, cleared_manual)
        };
        if seeded {
            let _ = app.emit(
                "coordinator_clock_updated",
                serde_json::json!({ "replicaPath": cwd, "lastUserMessageAt": now.to_rfc3339() }),
            );
        }
        if cleared_auto {
            let _ = app.emit(
                "coordinator_auto_close_changed",
                serde_json::json!({ "replicaPath": cwd, "autoClosedAt": null }),
            );
        }
        if cleared_manual {
            let _ = app.emit(
                "coordinator_manual_close_changed",
                serde_json::json!({ "replicaPath": cwd, "manuallyClosedAt": null }),
            );
        }
        Some(CoordinatorCreateTicket {
            app: app.clone(),
            cwd: cwd.to_string(),
            fqn,
            done: false,
            session_may_exist: false,
        })
    }
}

/// (#2413) Orders close-marker changes made by coordinator creates with their
/// events. The create's clear and a failed create's restore each hold it from
/// the clocks change through their emits, so the UI receives their events in
/// the same order as the clock changes. Taken before the clocks lock, never
/// held across an `.await`.
static CLOSE_MARKER_EVENT_ORDER: Mutex<()> = Mutex::new(());

/// (#2413) Test-only pause in a failed create's restore, after the clocks lock
/// is dropped and before its events are emitted. Keyed by FQN.
#[cfg(test)]
pub(crate) mod restore_emit_seam;

/// (#2413) One in-flight coordinator create. Must end with `succeeded()` or
/// `failed()`; Drop without either counts as `failed()` (cancel/panic safety);
/// `failed()`/Drop with the spawn fence set counts as `succeeded()`.
pub(crate) struct CoordinatorCreateTicket<R: tauri::Runtime> {
    app: tauri::AppHandle<R>,
    cwd: String,
    fqn: String,
    done: bool,
    /// Spawn fence: true once a PTY may exist. Then every end counts as success.
    session_may_exist: bool,
}

impl<R: tauri::Runtime> CoordinatorCreateTicket<R> {
    pub(crate) fn succeeded(mut self) {
        self.done = true;
        self.finish_ok();
    }

    pub(crate) fn failed(mut self) {
        self.finish_failed();
    }

    fn finish_ok(&self) {
        if let Some(clocks) = self
            .app
            .try_state::<crate::config::coordinator_clocks::CoordinatorClocksState>()
        {
            let mut g = clocks.lock().unwrap_or_else(|e| e.into_inner());
            g.finish_coordinator_create(&self.fqn, true);
        }
    }

    fn finish_failed(&mut self) {
        if self.done {
            return;
        }
        self.done = true;
        if self.session_may_exist {
            self.finish_ok();
            log::info!(
                "[coordinator-create] create failed after spawn attempt; close markers stay \
                 cleared fqn={}",
                self.fqn
            );
            return;
        }
        let Some(clocks) = self
            .app
            .try_state::<crate::config::coordinator_clocks::CoordinatorClocksState>()
        else {
            return;
        };
        // Held through the emits below (see `CLOSE_MARKER_EVENT_ORDER`).
        let _order = CLOSE_MARKER_EVENT_ORDER
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let (escrow, manual, auto) = {
            let mut g = clocks.lock().unwrap_or_else(|e| e.into_inner());
            let escrow = g.finish_coordinator_create(&self.fqn, false);
            let mut manual = None;
            let mut auto = None;
            if let Some(c) = escrow {
                if let Some(t) = c.manually_closed_at {
                    if g.mark_manually_closed(&self.fqn, t) {
                        manual = Some(t);
                    }
                }
                if let Some(t) = c.auto_closed_at {
                    if g.mark_auto_closed(&self.fqn, t) {
                        auto = Some(t);
                    }
                }
            }
            (escrow, manual, auto)
        };
        #[cfg(test)]
        restore_emit_seam::hit(&self.fqn);
        if let Some(t) = manual {
            let _ = self.app.emit(
                "coordinator_manual_close_changed",
                serde_json::json!({ "replicaPath": self.cwd, "manuallyClosedAt": t.to_rfc3339() }),
            );
        }
        if let Some(t) = auto {
            let _ = self.app.emit(
                "coordinator_auto_close_changed",
                serde_json::json!({ "replicaPath": self.cwd, "autoClosedAt": t.to_rfc3339() }),
            );
        }
        if manual.is_some() || auto.is_some() {
            log::info!(
                "[coordinator-create] restored close markers after failed create fqn={} auto={:?} \
                 manual={:?}",
                self.fqn,
                auto,
                manual
            );
        } else if escrow.is_none() {
            log::info!(
                "[coordinator-create] failed create handed markers to an in-flight create fqn={}",
                self.fqn
            );
        }
    }
}

impl<R: tauri::Runtime> Drop for CoordinatorCreateTicket<R> {
    fn drop(&mut self) {
        self.finish_failed();
    }
}

/// The accepted create core retains this record independently of its waiter.
/// Reservations are renewed only under U, against current ALL-scope facts.
struct CreateResourceAdmission {
    monitor: Arc<ResourceMonitorState>,
    settings: SettingsState,
    permit: Option<AgentLaunchPermit>,
    needed: bool,
    in_region: bool,
    released: bool,
}

impl Drop for CreateResourceAdmission {
    fn drop(&mut self) {
        if let Some(permit) = self.permit.take() {
            self.monitor.release_unregistered_permit(permit);
        }
    }
}

struct CreateResourceRegion<'a> {
    local: &'a mut Option<AgentLaunchPermit>,
    resource: Arc<Mutex<CreateResourceAdmission>>,
}

impl Drop for CreateResourceRegion<'_> {
    fn drop(&mut self) {
        if let Ok(mut resource) = self.resource.lock() {
            *self.local = resource.permit.take();
            resource.in_region = false;
        }
    }
}

async fn with_create_resource_region<T>(
    owner: &Arc<Mutex<OwnedCreateAdmission>>,
    local: &mut Option<AgentLaunchPermit>,
    effect: impl std::future::Future<Output = Result<T, String>>,
) -> Result<T, String> {
    let resource = owner
        .lock()
        .map_err(|_| "targetCreateGateUnavailable".to_string())?
        .resource
        .clone();
    let Some(resource) = resource else {
        return effect.await;
    };
    {
        let mut state = resource
            .lock()
            .map_err(|_| "resource admission unavailable".to_string())?;
        if state.in_region || state.permit.is_some() {
            return Err("resource admission already owned".to_string());
        }
        state.permit = local.take();
        state.in_region = true;
        state.released = false;
    }
    let region = CreateResourceRegion { local, resource };
    let result = effect.await;
    drop(region);
    result
}

async fn revalidate_owned_create_effect(
    owner: &Arc<Mutex<OwnedCreateAdmission>>,
    resource: &mut Option<AgentLaunchPermit>,
    monitor: &Arc<ResourceMonitorState>,
    manager: &SessionManager,
    pty: &Arc<Mutex<PtyManager>>,
) -> Result<(), String> {
    let continuation = owner
        .lock()
        .ok()
        .and_then(|owner| owner.owned_replacement.clone());
    let Some(continuation) = continuation else {
        release_resource_launch_permit(monitor, resource);
        return Err("targetCreateGateUnavailable".into());
    };
    let result = with_create_resource_region(owner, resource, async {
        if continuation.validate_pre_effect_region(manager, pty).await {
            Ok(())
        } else {
            Err("targetCreateGateUnavailable".to_string())
        }
    })
    .await;
    if result.is_err() {
        release_resource_launch_permit(monitor, resource);
        if let Ok(mut owner) = owner.lock() {
            owner.blocked_bound = true;
        }
    }
    result
}

#[derive(Clone)]
struct CreateCapacityContext {
    ticket: Arc<Mutex<Option<crate::session::selection::CreateFinalizationTicket>>>,
    coordinator: SelectionCoordinator,
}

struct OwnedCreateTargetLease {
    key: String,
    gate: Arc<crate::api::message_store::PtyInputTargetGate>,
    replica: crate::path_identity::VerifiedPathIdentity,
    // Exact precedes stripe in actual drop order.
    guards: Option<(
        crate::api::message_store::PtyInputTargetGuard,
        crate::api::message_store::PtyInputStripeGuard,
    )>,
}

struct OwnedCreateAdmission {
    preheld_target: bool,
    owned_replacement: Option<crate::session::selection::OwnedReplacementContinuation>,
    replacement_pty: Option<Arc<Mutex<PtyManager>>>,
    tracker: crate::pty::manager::ResponseCloseState,
    target: String,
    cwd: PathBuf,
    nonce: Uuid,
    binding: PendingCreateBinding,
    reservation: Option<crate::pty::manager::CreateOwnerReservation>,
    failed_foreign: bool,
    owned_target: Option<OwnedCreateTargetLease>,
    capacity: Option<CreateCapacityContext>,
    resource: Option<Arc<Mutex<CreateResourceAdmission>>>,
    blocked_bound: bool,
    capacity_error: Option<String>,
}

impl OwnedCreateAdmission {
    fn classify_current(
        &self,
        scope: crate::config::sessions_persistence::CreateCarrierScope,
        current: &crate::session::manager::CarrierSnapshot,
    ) -> Option<(crate::pty::manager::CreateClassification, bool)> {
        use crate::config::sessions_persistence::CreateCarrierScope;
        use crate::pty::manager::CreateClassification;
        Some(match scope {
            CreateCarrierScope::Foreign { publication_epoch } => {
                (CreateClassification::Foreign { publication_epoch }, true)
            }
            CreateCarrierScope::Waiting => return None,
            CreateCarrierScope::Protected {
                target,
                session_id,
                token,
                version,
                epoch,
                preparing_owner,
            } => {
                let row = current.rows.iter().find(|row| {
                    row.id == session_id
                        && row.token == token
                        && !current.pending_ids.contains(&row.id)
                })?;
                if current.versions.get(&session_id) != Some(&version) {
                    return None;
                }
                let generation = self.tracker.generation(session_id)?;
                let identity = crate::pty::manager::ResponseRouteIdentity {
                    target,
                    session_id,
                    token,
                    generation,
                    backend: row.backend_kind,
                    carrier_version: version,
                };
                match preparing_owner {
                    Some(owner_nonce) => (
                        CreateClassification::Preparing {
                            owner_nonce,
                            identity,
                            publication_epoch: epoch,
                        },
                        owner_nonce == self.nonce,
                    ),
                    None => (
                        CreateClassification::Protected {
                            identity,
                            publication_epoch: epoch,
                        },
                        false,
                    ),
                }
            }
        })
    }

    fn reserve_current(
        &mut self,
        scope: crate::config::sessions_persistence::CreateCarrierScope,
        current: &crate::session::manager::CarrierSnapshot,
    ) -> bool {
        use crate::pty::manager::{CreateAdmission, CreateClassification, ResponseCloseError};
        self.failed_foreign = false;
        self.reservation = None;
        let Some((classification, may_mutate)) = self.classify_current(scope, current) else {
            return false;
        };
        // Generic G never reserves an exclusive Protected claim while waiting
        // for R. That would veto the owner whose progress G needs.
        self.reservation = None;
        if !may_mutate {
            return false;
        }
        let foreign = matches!(&classification, CreateClassification::Foreign { .. });
        let protected_generation = match &classification {
            CreateClassification::Protected { identity, .. }
            | CreateClassification::Preparing { identity, .. } => {
                Some((identity.session_id, identity.generation))
            }
            _ => None,
        };
        match self
            .tracker
            .reserve_create_owner(&self.target, self.nonce, classification)
        {
            Ok(CreateAdmission::Reserved(reservation)) => {
                self.reservation = Some(reservation);
                may_mutate
                    && protected_generation.is_none_or(|(id, generation)| {
                        self.tracker.generation(id) == Some(generation)
                    })
            }
            Err(ResponseCloseError::Unavailable)
                if foreign && !self.tracker.target_is_closing(&self.target) =>
            {
                // Unavailable is permanently latched by this exact tracker.
                // The upper owner stays alive; this is not a reset or a proof
                // inferred from missing facts, poison, or a different tracker.
                self.failed_foreign = true;
                self.failed_foreign
            }
            _ => false,
        }
    }
}

fn release_owned_create_target(owner: &Arc<Mutex<OwnedCreateAdmission>>) {
    let released = owner.lock().ok().map(|mut owner| {
        owner.reservation = None;
        let guards = owner
            .owned_target
            .as_mut()
            .and_then(|lease| lease.guards.take());
        (guards, owner.capacity.clone(), owner.resource.clone())
    });
    let Some((guards, capacity, resource)) = released else {
        return;
    };
    drop(guards);
    if let Some(resource) = resource {
        if let Ok(mut resource) = resource.lock() {
            if resource.in_region && resource.needed {
                if let Some(permit) = resource.permit.take() {
                    resource.monitor.release_unregistered_permit(permit);
                }
                resource.released = true;
            }
        }
    }
    if let Some(capacity) = capacity {
        let unbound = capacity
            .ticket
            .lock()
            .ok()
            .and_then(|mut ticket| {
                ticket
                    .as_mut()
                    .map(|ticket| ticket.release_capacity_for_wait())
            })
            .unwrap_or(false);
        if !unbound {
            if let Ok(mut owner) = owner.lock() {
                owner.blocked_bound = true;
            }
        }
    }
}

fn resume_owned_create_capacity(owner: &Arc<Mutex<OwnedCreateAdmission>>) -> Result<(), String> {
    let capacity = owner
        .lock()
        .map_err(|_| "targetCreateGateUnavailable".to_string())?
        .capacity
        .clone();
    let Some(capacity) = capacity else {
        return Ok(());
    };
    let mut ticket = capacity
        .ticket
        .lock()
        .map_err(|_| "create ticket unavailable".to_string())?;
    let Some(ticket) = ticket.as_mut() else {
        return Err("create ticket unavailable".to_string());
    };
    ticket
        .resume_unbound_capacity(&capacity.coordinator)
        .map_err(|error| error.to_string())
}

async fn resume_create_resource_admission(
    owner: &Arc<Mutex<OwnedCreateAdmission>>,
) -> Result<(), String> {
    let resource = owner
        .lock()
        .map_err(|_| "targetCreateGateUnavailable".to_string())?
        .resource
        .clone();
    let Some(resource) = resource else {
        return Ok(());
    };
    let (monitor, settings) = {
        let resource = resource
            .lock()
            .map_err(|_| "resource admission unavailable".to_string())?;
        if !resource.in_region || !resource.released || !resource.needed {
            return Ok(());
        }
        (resource.monitor.clone(), resource.settings.clone())
    };
    loop {
        // Do not occupy quota needed by the protected owner while G waits.
        // This is only a preflight probe; target/U/M admission still follows.
        let permit = acquire_carrier_mutation_permit().await;
        let eligible = owner.lock().ok().is_some_and(|owner| {
            use crate::config::sessions_persistence::CreateCarrierScope;
            match crate::config::sessions_persistence::classify_create_carrier_scope(
                &permit,
                &owner.target,
                &owner.cwd,
            ) {
                CreateCarrierScope::Foreign { .. } => true,
                CreateCarrierScope::Protected {
                    preparing_owner: Some(nonce),
                    ..
                } => nonce == owner.nonce,
                _ => false,
            }
        });
        drop(permit);
        if eligible {
            break;
        }
        tokio::task::yield_now().await;
    }
    // Fresh limits, outside U/S/target guards. No retry on a real quota error.
    let cfg = settings.read().await;
    let next = monitor.try_reserve_agent_slot(ResourceLimits::from(&*cfg))?;
    drop(cfg);
    let mut state = match resource.lock() {
        Ok(state) => state,
        Err(_) => {
            if let Some(permit) = next {
                monitor.release_unregistered_permit(permit);
            }
            return Err("resource admission unavailable".to_string());
        }
    };
    if !state.in_region || !state.released || state.permit.is_some() {
        if let Some(permit) = next {
            monitor.release_unregistered_permit(permit);
        }
        return Err("resource admission ownership changed".to_string());
    }
    state.permit = next;
    state.released = false;
    Ok(())
}

async fn ensure_owned_create_target(
    owner: &Arc<Mutex<OwnedCreateAdmission>>,
) -> Result<(), String> {
    // Archive invokes this preparation before acquiring its settings write
    // guard. Its later settings-first admission remains a pure U check.
    let replacement = owner
        .lock()
        .ok()
        .and_then(|owner| owner.owned_replacement.clone());
    if let Some(replacement) = replacement {
        if !replacement.validate_source().await {
            return Err("targetCreateGateUnavailable".into());
        }
    }
    let (blocked_bound, capacity_error) = {
        let owner = owner
            .lock()
            .map_err(|_| "targetCreateGateUnavailable".to_string())?;
        (owner.blocked_bound, owner.capacity_error.clone())
    };
    if let Some(error) = capacity_error {
        return Err(error);
    }
    if blocked_bound {
        // A new conflicting protection across admitted Pending is an invariant
        // violation. Keep the accepted core/binding unsafe, with no target or
        // finalizer capacity and no invented public outcome or repeated effect.
        return std::future::pending().await;
    }
    resume_create_resource_admission(owner).await?;
    let required = {
        let owner = owner
            .lock()
            .map_err(|_| "targetCreateGateUnavailable".to_string())?;
        owner
            .owned_target
            .as_ref()
            .filter(|lease| lease.guards.is_none())
            .map(|lease| {
                (
                    lease.gate.clone(),
                    lease.key.clone(),
                    lease.replica.clone(),
                    owner.cwd.clone(),
                )
            })
    };
    let Some((gate, key, replica, cwd)) = required else {
        return Ok(());
    };
    let stripe = gate
        .acquire_target_lock(&key)
        .await
        .map_err(|_| "targetCreateGateUnavailable".to_string())?;
    let exact = gate.acquire_exact(&key).await;
    let current = crate::path_identity::verify_directory(&replica.canonical_path)
        .map_err(|_| "targetCreateGateUnavailable".to_string())?;
    let current_key = crate::config::teams::pty_input_create_gate_key_from_cwd(&cwd)
        .map_err(|_| "targetCreateGateUnavailable".to_string())?;
    if current.object_id != replica.object_id
        || current.canonical_path != replica.canonical_path
        || current_key.as_deref() != Some(key.as_str())
    {
        return Err("targetCreateGateUnavailable".to_string());
    }
    let mut owner = owner
        .lock()
        .map_err(|_| "targetCreateGateUnavailable".to_string())?;
    let Some(lease) = owner.owned_target.as_mut() else {
        return Err("targetCreateGateUnavailable".to_string());
    };
    if lease.key != key || lease.guards.is_some() {
        return Err("targetCreateGateUnavailable".to_string());
    }
    lease.guards = Some((exact, stripe));
    Ok(())
}

async fn try_acquire_create_publication_region(
    owner: &Arc<Mutex<OwnedCreateAdmission>>,
    manager: &SessionManager,
) -> Option<crate::session::manager::CarrierMutationPermit> {
    try_acquire_create_publication_region_checked(owner, manager, || true).await
}

async fn try_acquire_create_publication_region_checked(
    owner: &Arc<Mutex<OwnedCreateAdmission>>,
    manager: &SessionManager,
    context_is_current: impl Fn() -> bool,
) -> Option<crate::session::manager::CarrierMutationPermit> {
    if owner
        .lock()
        .ok()?
        .owned_target
        .as_ref()
        .is_some_and(|lease| lease.guards.is_none())
    {
        return None;
    }
    let permit = acquire_carrier_mutation_permit().await;
    let scope = owner.lock().ok().map(|owner| {
        crate::config::sessions_persistence::classify_create_carrier_scope(
            &permit,
            &owner.target,
            &owner.cwd,
        )
    })?;
    let admitted = manager
        .with_current_carrier_snapshot(&permit, |current| {
            owner.lock().ok().is_some_and(|mut owner| {
                context_is_current()
                    && match (
                        owner.owned_replacement.as_ref(),
                        owner.replacement_pty.as_ref(),
                    ) {
                        (Some(continuation), Some(pty)) => continuation.validate_create_region(
                            &permit,
                            current,
                            &owner.tracker,
                            pty,
                        ),
                        (None, _) => owner.reserve_current(scope, current),
                        _ => false,
                    }
            })
        })
        .await;
    if admitted {
        if let Err(error) = resume_owned_create_capacity(owner) {
            if let Ok(mut owner) = owner.lock() {
                owner.capacity_error = Some(error);
            }
            drop(permit);
            release_owned_create_target(owner);
            return None;
        }
        Some(permit)
    } else {
        drop(permit);
        release_owned_create_target(owner);
        None
    }
}

async fn acquire_create_publication_region(
    owner: &Arc<Mutex<OwnedCreateAdmission>>,
    manager: &SessionManager,
) -> Result<crate::session::manager::CarrierMutationPermit, String> {
    loop {
        ensure_owned_create_target(owner).await?;
        if let Some(permit) = try_acquire_create_publication_region(owner, manager).await {
            return Ok(permit);
        }
        tokio::task::yield_now().await;
    }
}

/// A borrowed lifecycle lease cannot be released by common create. A changed
/// region returns through the explicit private outcome to its lexical owner;
/// generic creates release and reacquire their actual owned lease and capacity.
async fn acquire_lifecycle_create_publication_region<R: tauri::Runtime>(
    owner: &Arc<Mutex<OwnedCreateAdmission>>,
    manager: &SessionManager,
    domain: Option<&LifecycleEntryAuthority>,
    transaction: Option<&SelectionTransaction<R>>,
) -> Result<crate::session::manager::CarrierMutationPermit, String> {
    let replacement = owner
        .lock()
        .ok()
        .and_then(|owner| owner.owned_replacement.clone());
    if let Some(replacement) = replacement {
        if !replacement.validate_source().await {
            return Err("targetCreateGateUnavailable".into());
        }
        return match try_acquire_create_publication_region(owner, manager).await {
            Some(permit) => Ok(permit),
            None => {
                if let Ok(mut owner) = owner.lock() {
                    owner.blocked_bound = true;
                }
                Err("targetCreateGateUnavailable".into())
            }
        };
    }
    let Some(domain) = domain else {
        if owner.lock().ok().is_some_and(|owner| owner.preheld_target) {
            return match try_acquire_create_publication_region(owner, manager).await {
                Some(permit) => Ok(permit),
                None => {
                    if let Ok(mut owner) = owner.lock() {
                        owner.blocked_bound = true;
                    }
                    Err("targetCreateGateUnavailable".into())
                }
            };
        }
        if let Some(transaction) =
            transaction.filter(|transaction| transaction.has_restore_context())
        {
            if transaction.has_active_restore_capability() {
                if let Some(permit) =
                    try_acquire_create_publication_region_checked(owner, manager, || {
                        transaction.has_active_restore_capability()
                    })
                    .await
                {
                    return Ok(permit);
                }
            }
            transaction.invalidate_restore_ownership();
            release_owned_create_target(owner);
            return Err(
                crate::session::selection::SelectionCoordinatorError::Unavailable.to_string(),
            );
        }
        return acquire_create_publication_region(owner, manager).await;
    };
    let active_context = transaction.is_some_and(|transaction| {
        transaction.has_accepted_lifecycle_core() || transaction.has_active_restore_capability()
    });
    let cwd = owner.lock().ok().map(|owner| owner.cwd.clone());
    let valid_destination = cwd.is_some_and(|cwd| {
        crate::config::teams::pty_input_create_gate_key_from_cwd(&cwd)
            .is_ok_and(|key| domain.validates_destination(&cwd.to_string_lossy(), key.as_deref()))
    });
    if active_context
        && valid_destination
        && !domain
            .interrupted
            .load(std::sync::atomic::Ordering::Acquire)
    {
        if let Some(permit) = try_acquire_create_publication_region_checked(owner, manager, || {
            transaction.is_some_and(|transaction| {
                transaction.has_accepted_lifecycle_core()
                    || transaction.has_active_restore_capability()
            })
        })
        .await
        {
            return Ok(permit);
        }
    }
    if let Some(transaction) = transaction {
        transaction.invalidate_restore_ownership();
    }
    domain
        .interrupted
        .store(true, std::sync::atomic::Ordering::Release);
    Err(crate::session::selection::SelectionCoordinatorError::Unavailable.to_string())
}

/// The accepted archive operation owns its application and inputs. Cancelling
/// the create waiter cannot detach native mutation from settings publication.
async fn enforce_owned_unarchived_for_spawn<R: tauri::Runtime>(
    app: &AppHandle<R>,
    cwd: &str,
    session_label: &str,
    create_owner: &Arc<Mutex<OwnedCreateAdmission>>,
    manager: &SessionManager,
    lifecycle_domain: Option<&LifecycleEntryAuthority>,
    inline_transaction: Option<&SelectionTransaction<R>>,
) -> Result<(), String> {
    if lifecycle_domain.is_some()
        || inline_transaction.is_some_and(|transaction| transaction.has_restore_context())
    {
        // The actual outer lifecycle/restore executor already owns this effect.
        // Its borrow and outcome must return to that same executor, not a child.
        let tracker = app
            .state::<Arc<Mutex<PtyManager>>>()
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .response_close_state();
        return crate::config::archive_gate::enforce_unarchived_for_spawn(
            app,
            cwd,
            session_label,
            || async { ensure_owned_create_target(create_owner).await },
            || async {
                acquire_lifecycle_create_publication_region(
                    create_owner,
                    manager,
                    lifecycle_domain,
                    inline_transaction,
                )
                .await
                .map(Some)
            },
            |written| {
                tracker.apply_settings(
                    written.response_close_enabled,
                    written.response_close_idle_seconds,
                    std::time::Instant::now(),
                )
            },
        )
        .await;
    }
    let create_owner = create_owner.clone();
    let manager = manager.clone();
    let app = app.clone();
    let cwd = cwd.to_owned();
    let session_label = session_label.to_owned();
    let tracker = app
        .state::<Arc<Mutex<PtyManager>>>()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .response_close_state();
    crate::config::sessions_persistence::run_owned_persist_core(async move {
        crate::config::archive_gate::enforce_unarchived_for_spawn(
            &app,
            &cwd,
            &session_label,
            || {
                let owner = create_owner.clone();
                async move { ensure_owned_create_target(&owner).await }
            },
            || {
                let owner = create_owner.clone();
                let manager = manager.clone();
                async move {
                    let admitted = try_acquire_create_publication_region(&owner, &manager).await;
                    if admitted.is_none()
                        && owner.lock().ok().is_some_and(|owner| owner.preheld_target)
                    {
                        if let Ok(mut owner) = owner.lock() {
                            owner.blocked_bound = true;
                        }
                        return Err("targetCreateGateUnavailable".to_string());
                    }
                    Ok::<_, String>(admitted)
                }
            },
            |written| {
                tracker.apply_settings(
                    written.response_close_enabled,
                    written.response_close_idle_seconds,
                    std::time::Instant::now(),
                )
            },
        )
        .await
    })
    .await
}

#[allow(clippy::too_many_arguments)]
async fn create_session_inner_impl<R: tauri::Runtime>(
    app: &AppHandle<R>,
    session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
    pty_mgr: &Arc<Mutex<PtyManager>>,
    shell: String,
    shell_args: Vec<String>,
    cwd: String,
    session_name: Option<String>,
    agent_id: Option<String>,
    agent_label: Option<String>,
    skip_tooling_save: bool,
    git_repos: Vec<SessionRepo>,
    skip_auto_resume: bool,
    resolved_spawn: Option<AgentSpawnCommand>,
    resolved_agent_host_shell: Option<ResolvedAgentHostShell>,
    viewport: Option<PtyViewport>,
    selection_intent: CreateSelectionIntent,
    pending_start_fresh: Option<bool>,
    pending_communication: Option<SessionCommunication>,
    inline_transaction: Option<&SelectionTransaction<R>>,
    inline_cause: Option<SelectionCause>,
    defer_inline_finalization: bool,
    preheld_target: Option<&crate::api::message_store::PtyInputTargetOwnership<'_>>,
    enforcement: crate::config::sessions_persistence::CreationGateEnforcement,
    coding_agent_binding: Option<crate::session::manager::CodingAgentBinding>,
    lifecycle_domain: Option<&LifecycleEntryAuthority>,
    owned_replacement: Option<crate::session::selection::OwnedReplacementContinuation>,
    initial_last_prompt: Option<InitialLastPrompt>,
) -> Result<CreateCompletion, String> {
    if inline_transaction.is_some_and(|transaction| {
        transaction.has_restore_context() && !transaction.has_active_restore_capability()
    }) {
        if let Some(domain) = lifecycle_domain {
            domain
                .interrupted
                .store(true, std::sync::atomic::Ordering::Release);
        }
        return Err(crate::session::selection::SelectionCoordinatorError::Unavailable.to_string());
    }
    if preheld_target.is_some()
        || inline_transaction.is_some_and(|transaction| {
            transaction.has_accepted_lifecycle_core() || transaction.has_active_restore_capability()
        })
        || lifecycle_domain.is_some()
    {
        // Accepted lifecycle jobs already own the one outer create core.
        // The PTY producer owns its entire operation, target stripe/file and
        // exact target guard. Its borrowed proof stays in that task's scope.
        return create_session_inner_core(
            app,
            session_mgr,
            pty_mgr,
            shell,
            shell_args,
            cwd,
            session_name,
            agent_id,
            agent_label,
            skip_tooling_save,
            git_repos,
            skip_auto_resume,
            resolved_spawn,
            resolved_agent_host_shell,
            viewport,
            selection_intent,
            pending_start_fresh,
            pending_communication,
            inline_transaction,
            inline_cause,
            defer_inline_finalization,
            preheld_target,
            enforcement,
            coding_agent_binding,
            lifecycle_domain,
            owned_replacement,
            initial_last_prompt,
        )
        .await;
    }
    let app = app.clone();
    let session_mgr = session_mgr.clone();
    let pty_mgr = pty_mgr.clone();
    let inline_transaction = inline_transaction.cloned();
    crate::config::sessions_persistence::run_owned_persist_core(async move {
        create_session_inner_core(
            &app,
            &session_mgr,
            &pty_mgr,
            shell,
            shell_args,
            cwd,
            session_name,
            agent_id,
            agent_label,
            skip_tooling_save,
            git_repos,
            skip_auto_resume,
            resolved_spawn,
            resolved_agent_host_shell,
            viewport,
            selection_intent,
            pending_start_fresh,
            pending_communication,
            inline_transaction.as_ref(),
            inline_cause,
            defer_inline_finalization,
            None,
            enforcement,
            coding_agent_binding,
            None,
            owned_replacement,
            initial_last_prompt,
        )
        .await
    })
    .await
}

fn resolve_create_destination<R: tauri::Runtime>(
    cwd: &str,
    lifecycle_domain: Option<&LifecycleEntryAuthority>,
    preheld_target: Option<&crate::api::message_store::PtyInputTargetOwnership<'_>>,
    inline_transaction: Option<&SelectionTransaction<R>>,
) -> Result<
    (
        Option<String>,
        Option<crate::path_identity::VerifiedPathIdentity>,
    ),
    String,
> {
    let destination_proof = (|| {
        let cwd_path = std::path::Path::new(cwd);
        let create_target_key = crate::config::teams::pty_input_create_gate_key_from_cwd(cwd_path)
            .map_err(|_| "targetCreateGateUnavailable".to_string())?;
        let create_target_replica_identity = if create_target_key.is_some() {
            let anchor = crate::config::teams::strict_wg_replica_anchor_from_cwd(cwd_path)
                .map_err(|_| "targetCreateGateUnavailable".to_string())?
                .ok_or_else(|| "targetCreateGateUnavailable".to_string())?;
            Some(
                crate::path_identity::verify_directory(&anchor)
                    .map_err(|_| "targetCreateGateUnavailable".to_string())?,
            )
        } else {
            None
        };
        if let Some(domain) = lifecycle_domain {
            if !domain.validates_destination(cwd, create_target_key.as_deref()) {
                return Err("invalidTargetCreateOwnership".to_string());
            }
        }
        if let Some(preheld) = preheld_target {
            let target_key = create_target_key
                .as_deref()
                .ok_or_else(|| "invalidTargetCreateOwnership".to_string())?;
            if !preheld.proves(target_key) {
                return Err("invalidTargetCreateOwnership".to_string());
            }
        }
        Ok::<_, String>((create_target_key, create_target_replica_identity))
    })();
    match destination_proof {
        Ok(proof) => Ok(proof),
        Err(error) => {
            if let Some(domain) = lifecycle_domain {
                domain
                    .interrupted
                    .store(true, std::sync::atomic::Ordering::Release);
            }
            if let Some(transaction) = inline_transaction {
                transaction.invalidate_restore_ownership();
            }
            Err(error)
        }
    }
}

async fn acquire_create_target_lease<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    target_key: Option<&str>,
    replica: Option<&crate::path_identity::VerifiedPathIdentity>,
    has_preheld_target: bool,
    has_lifecycle_domain: bool,
) -> Result<Option<OwnedCreateTargetLease>, String> {
    Ok(if !has_preheld_target && !has_lifecycle_domain {
        if let Some(target_key) = target_key {
            let gate = app
                .try_state::<crate::api::message_store::PtyInputTargetGateState>()
                .and_then(|state| state.gate.as_ref().ok().map(Arc::clone))
                .or_else(|| {
                    app.try_state::<crate::api::message_store::MessageStoreState>()
                        .and_then(|state| state.target_gate.as_ref().ok().map(Arc::clone))
                })
                .ok_or_else(|| "targetCreateGateUnavailable".to_string())?;
            let stripe = gate
                .acquire_target_lock(target_key)
                .await
                .map_err(|_| "targetCreateGateUnavailable".to_string())?;
            let exact = gate.acquire_exact(target_key).await;
            Some(OwnedCreateTargetLease {
                key: target_key.to_owned(),
                gate,
                replica: replica
                    .cloned()
                    .ok_or_else(|| "targetCreateGateUnavailable".to_string())?,
                guards: Some((exact, stripe)),
            })
        } else {
            None
        }
    } else {
        None
    })
}

#[expect(
    clippy::too_many_arguments,
    reason = "P05 retains distinct result, marker, admission and original cleanup ownership \
              without a context bundle."
)]
async fn finish_create_core_result<R: tauri::Runtime>(
    marker: Option<CoordinatorCreateTicket<R>>,
    mut result: Result<CreateCompletion, String>,
    owned_replacement: Option<&crate::session::selection::OwnedReplacementContinuation>,
    create_owner: &Arc<Mutex<OwnedCreateAdmission>>,
    session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
    pty_mgr: &Arc<Mutex<PtyManager>>,
    has_preheld_target: bool,
    inline_pending_binding: &Mutex<Option<crate::session::manager::PendingCreateBinding>>,
    inline_transaction: Option<&SelectionTransaction<R>>,
) -> Result<CreateCompletion, String> {
    if let Some(t) = marker {
        if result.is_err() {
            t.failed();
        } else if matches!(
            &result,
            Ok(CreateCompletion::Finalized(_)) | Ok(CreateCompletion::Deferred(_))
        ) {
            t.succeeded();
        }
    }
    if let (Err(error), Some(continuation)) = (&result, owned_replacement) {
        let waiting = continuation.source_stop().is_none()
            && create_owner
                .lock()
                .ok()
                .is_some_and(|owner| owner.blocked_bound);
        if !waiting {
            let manager = session_mgr.read().await.clone();
            continuation
                .rollback_failed_create(&manager, pty_mgr, error)
                .await;
        }
    }
    if let Some(continuation) = owned_replacement {
        if continuation.source_stop().is_some() {
            // Validation failure is not a setup Err or whole-source stopped
            // proof. Retain the real pending owner and its work.
            result = Ok(CreateCompletion::Retained);
        } else if create_owner
            .lock()
            .ok()
            .is_some_and(|owner| owner.blocked_bound)
        {
            result = Ok(CreateCompletion::Retained);
        }
    }
    if has_preheld_target
        && create_owner
            .lock()
            .ok()
            .is_some_and(|owner| owner.blocked_bound)
    {
        result = Ok(CreateCompletion::Retained);
    }
    if result.is_err() && owned_replacement.is_none() {
        let binding = {
            inline_pending_binding
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .take()
        };
        if let (Some(transaction), Some(binding)) = (inline_transaction, binding) {
            transaction.rollback_inline_create(binding).await;
        }
    }
    result
}

async fn destroy_one_legacy_session<R: tauri::Runtime>(
    transaction: &SelectionTransaction<R>,
    request: &DestroyRequest,
    before: &crate::session::manager::ManagerAggregateSnapshot,
    session_id: uuid::Uuid,
    outcome: &mut DestroyOutcome,
    mutations: &mut LifecycleMutations,
    retained_marked: &mut std::collections::HashSet<uuid::Uuid>,
) {
    let Some(existing) = before
        .sessions
        .iter()
        .find(|session| session.id == session_id)
        .cloned()
    else {
        outcome
            .failed
            .push((session_id, "Session not found".to_string()));
        return;
    };
    {
        let tracker = transaction
            .app()
            .state::<Arc<Mutex<PtyManager>>>()
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .response_close_state();
        let permit = acquire_carrier_mutation_permit().await;
        tracker.invalidate_manager_row(session_id, std::time::Instant::now());
        if !crate::config::sessions_persistence::retire_carrier_for_discard(&permit, session_id) {
            outcome
                .failed
                .push((session_id, "response_close_deferred".to_owned()));
            return;
        }
        crate::session::selection::forget_response_close_registration(session_id);
    }
    let is_root_agent = existing.is_root_agent
        || crate::config::root_agent::is_root_agent_path(&existing.working_directory);

    if let Err(error) =
        teardown_legacy_destroy_runtime(transaction, session_id, &request.source).await
    {
        outcome.failed.push((session_id, error));
        return;
    }

    #[cfg(test)]
    restart_reader_seam::hit_after_old_release(&format!("destroy:{session_id}")).await;

    if is_root_agent && !request.force_destroy_root {
        mutations.set_detached_intent(session_id, false);
        if !matches!(existing.status, SessionStatus::Exited(_)) {
            mutations.mark_exited(session_id, 0);
            retained_marked.insert(session_id);
        }
        outcome.retained_exited_ids.push(session_id);
    } else {
        mutations.remove(session_id);
        outcome.destroyed_ids.push(session_id);
    }
}

async fn teardown_legacy_destroy_runtime<R: tauri::Runtime>(
    transaction: &SelectionTransaction<R>,
    session_id: uuid::Uuid,
    source: &DestructionSource,
) -> Result<(), String> {
    {
        let detached = transaction.app().state::<DetachedSessionsState>();
        detached
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&session_id);
    }

    let bridge_shutdown = {
        let telegram = transaction.app().state::<TelegramBridgeState>();
        let mut telegram = telegram.lock().await;
        if telegram.has_bridge(session_id) {
            telegram.detach(session_id).ok()
        } else {
            None
        }
    };
    if let Some(shutdown) = bridge_shutdown {
        transaction
            .manager()
            .await
            .set_telegram_bot_id(session_id, None)
            .await;
        if let Err(error) = transaction.app().emit(
            "telegram_bridge_detached",
            serde_json::json!({ "sessionId": session_id.to_string() }),
        ) {
            log::warn!(
                "[session] telegram detach publication failed session={} source={:?}: {}",
                session_id,
                source,
                error
            );
        }
        shutdown.spawn_wait_or_abort();
    }

    let resource_monitor = transaction
        .app()
        .state::<Arc<ResourceMonitorState>>()
        .inner()
        .clone();
    if resource_monitor.has_registered_group(session_id) {
        {
            let pty = transaction.app().state::<Arc<Mutex<PtyManager>>>();
            pty.lock()
                .unwrap_or_else(|error| error.into_inner())
                .publish_stop_witness(session_id, "session-destroy");
        }
        let monitor = Arc::clone(&resource_monitor);
        match tokio::task::spawn_blocking(move || {
            monitor.kill_group(
                session_id,
                crate::resource_monitor::ResourceKillReason::SessionDestroy,
            )
        })
        .await
        {
            Ok(Ok(result)) => {
                if result.quarantined {
                    log::warn!(
                        "[session] resource cleanup quarantined session={} source={:?}: {}",
                        session_id,
                        source,
                        result.message
                    );
                }
            }
            Ok(Err(error)) => log::warn!(
                "[session] resource cleanup failed session={} source={:?}: {}",
                session_id,
                source,
                error
            ),
            Err(error) => log::warn!(
                "[session] resource cleanup task failed session={} source={:?}: {}",
                session_id,
                source,
                error
            ),
        }
    }

    // The std-Mutex guard from this `let` initializer is dropped at the `;`
    // before `runtime_snapshot` re-locks the same pty Mutex below. Moving this
    // into an `if let`/`match` scrutinee or an inner blocking block would hold
    // the guard across that re-lock and re-introduce the re-entrant std-Mutex
    // deadlock fixed in resource_monitor (§1295).
    let kill_result = transaction
        .app()
        .state::<Arc<Mutex<PtyManager>>>()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .kill(session_id);
    let still_has_pty = transaction.runtime_snapshot(session_id).has_pty;
    if let Err(error) = kill_result {
        if still_has_pty {
            return Err(error.to_string());
        }
        log::warn!(
            "[session] teardown reported an error after PTY loss session={} source={:?}: {}",
            session_id,
            source,
            error
        );
    }
    Ok(())
}

async fn accept_restart_deferred_completion<R: tauri::Runtime>(
    transaction: &SelectionTransaction<R>,
    completion: Result<CreateCompletion, String>,
    context: &LifecycleExecutionContext<'_>,
    uuid: uuid::Uuid,
    old_lost: bool,
    old_selected: bool,
    intent: TrustedRestartIntent,
) -> Result<DeferredCreateOutput, String> {
    Ok(match completion {
        Ok(CreateCompletion::Deferred(deferred)) => deferred,
        Ok(CreateCompletion::Retained) => {
            if let Some(authority) = context.authority() {
                authority
                    .interrupted
                    .store(true, std::sync::atomic::Ordering::Release);
            }
            return Err(
                crate::session::selection::SelectionCoordinatorError::Unavailable.to_string(),
            );
        }
        Ok(CreateCompletion::Finalized(_)) => {
            let error = "restart replacement was published before atomic finalization".to_string();
            if old_lost {
                finalize_failed_restart(transaction, uuid, old_selected, intent).await?;
            }
            return Err(error);
        }
        Err(error) => {
            if context.authority().is_some_and(|authority| {
                authority
                    .interrupted
                    .load(std::sync::atomic::Ordering::Acquire)
            }) {
                return Err(error);
            }
            if old_lost {
                if let Err(finalize_error) =
                    finalize_failed_restart(transaction, uuid, old_selected, intent).await
                {
                    return Err(format!(
                        "{}; restart failure finalization failed: {}",
                        error, finalize_error
                    ));
                }
            }
            return Err(error);
        }
    })
}

#[allow(clippy::too_many_arguments)]
async fn finalize_restart_deferred<R: tauri::Runtime>(
    transaction: &SelectionTransaction<R>,
    deferred: &DeferredCreateOutput,
    uuid: uuid::Uuid,
    old_lost: bool,
    old_selected: bool,
    activate_after: bool,
    intent: TrustedRestartIntent,
    restart_start_fresh: bool,
) -> Result<
    (
        uuid::Uuid,
        crate::session::manager::CommitResult,
        SessionInfo,
    ),
    String,
> {
    let new_uuid = match Uuid::parse_str(&deferred.info.id) {
        Ok(id) => id,
        Err(error) => {
            transaction.rollback_inline_create(deferred.binding).await;
            if old_lost {
                finalize_failed_restart(transaction, uuid, old_selected, intent).await?;
            }
            return Err(format!(
                "restart replacement returned an invalid id: {error}"
            ));
        }
    };
    let live = match transaction.live_decision(new_uuid) {
        Some(CommitDecision::Live(live)) => live,
        _ => {
            transaction.rollback_inline_create(deferred.binding).await;
            if old_lost {
                finalize_failed_restart(transaction, uuid, old_selected, intent).await?;
            }
            return Err("restart replacement is not displayable".to_string());
        }
    };

    // 5. Finalize the replacement, remove the old row, and decide selection in
    // one manager commit. Neither lifecycle row was public before this point.
    let mut mutations = LifecycleMutations::default();
    mutations.finalize_live(deferred.binding, live);
    mutations.remove(uuid);
    if deferred.effective_continuation && !restart_start_fresh {
        let manager = transaction.manager().await;
        if let (Some(source), Some(target)) = (
            manager.get_session(uuid).await,
            manager.get_pending_session(deferred.binding).await,
        ) {
            if source.agent_id.is_some()
                && source.agent_id == target.agent_id
                && source.requested_profile == target.requested_profile
                && source.effective_profile == target.effective_profile
                && source.working_directory == target.working_directory
                && !source.start_fresh_on_restore
                && !target.start_fresh_on_restore
            {
                mutations.transfer_prompt(uuid, deferred.binding);
            }
        }
    }
    let decision = if old_selected || activate_after {
        CommitDecision::Live(live)
    } else {
        CommitDecision::Keep
    };
    let committed = match transaction
        .commit_entry_create(
            deferred.binding,
            decision,
            SelectionCause::Restart(intent),
            mutations,
        )
        .await
    {
        Ok(committed) => committed,
        Err(error) => {
            transaction.rollback_inline_create(deferred.binding).await;
            if old_lost {
                finalize_failed_restart(transaction, uuid, old_selected, intent).await?;
            }
            return Err(error);
        }
    };
    let session_info = committed
        .finalized_rows
        .iter()
        .find(|row| row.id == new_uuid.to_string())
        .cloned()
        .ok_or_else(|| "restart finalization did not produce the replacement row".to_string())?;
    Ok((new_uuid, committed, session_info))
}

async fn publish_restart_before_warnings<R: tauri::Runtime>(
    transaction: &SelectionTransaction<R>,
    uuid: uuid::Uuid,
    new_uuid: uuid::Uuid,
    binding: crate::session::manager::PendingCreateBinding,
    committed: &crate::session::manager::CommitResult,
    session_info: &SessionInfo,
    old_lost: bool,
) -> Result<Option<crate::session::selection::LifecycleEffectGuard>, String> {
    let app = transaction.app();
    let publication = transaction
        .start_lifecycle_effect(crate::session::selection::LifecycleEffectSlot::Publication);
    if transaction.has_lifecycle_entry() && publication.is_none() {
        return Err(crate::session::selection::SelectionCoordinatorError::Unavailable.to_string());
    }
    // Section 5.2: the commit removed the old row. Detach its bridge (dormant
    // old) and sweep every old-id reader demand raised since teardown before
    // the replacement is published or attached; then evaluate the new uuid's
    // own Room eligibility, because the create-time raise was suppressed for
    // `SelectionCause::Restart(_)`. Exactly one demand per consumer, one reader
    // and one slot may exist.
    if !old_lost {
        detach_restart_telegram(transaction, uuid).await;
        transaction
            .app()
            .state::<DetachedSessionsState>()
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&uuid);
    }
    crate::commands::telegram::release_all_reader_demands(app, uuid).await;
    raise_room_reader_demand(app, new_uuid).await;
    transaction.observe_entry_reader_demand(
        binding,
        crate::session::selection::EntryReaderDemandReceipt::Returned,
    );
    transaction
        .persist(SelectionSource::Restart, Some(new_uuid))
        .await;
    transaction.publish_created(session_info);
    publish_restart_destroyed(transaction, uuid);
    if let Some(selection) = committed.selection.as_ref() {
        transaction.publish_selection(selection);
    }
    Ok(publication)
}

async fn publish_restart_after_warnings<R: tauri::Runtime>(
    transaction: &SelectionTransaction<R>,
    new_uuid: uuid::Uuid,
    session_info: SessionInfo,
    restart_start_fresh: bool,
    telegram_bot_id: Option<&str>,
    cwd: &str,
    publication: Option<crate::session::selection::LifecycleEffectGuard>,
) -> Result<SessionInfo, String> {
    let app = transaction.app();
    // (#756) C3: mirror the honored fresh intent into the coordinator clocks so
    // it survives record destruction (idle auto-close, manual close destroy the
    // record that carries start_fresh_on_restore). Self-gates on coordinators
    // inside the helper; root agents are never coordinators, so this is a no-op
    // for them. Self-handoff-and-switch (#668) respawns through this function
    // and inherits the mirror automatically.
    if restart_start_fresh {
        crate::commands::pty::write_start_fresh_mirror_for_session(app, new_uuid, true).await;
    }

    // 6. Re-attach Telegram bridge from live persisted intent, or fall back to repo config.
    if telegram_bot_id.is_some() {
        attach_persisted_telegram_if_configured(app, new_uuid, telegram_bot_id).await;
    } else {
        attach_local_config_telegram_if_any(app, new_uuid, cwd).await;
    }

    if let Some(publication) = publication {
        publication.complete(Ok(Some(session_info.id.clone())));
    }
    Ok(session_info)
}

#[allow(clippy::too_many_arguments)]
async fn create_session_inner_core<R: tauri::Runtime>(
    app: &AppHandle<R>,
    session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
    pty_mgr: &Arc<Mutex<PtyManager>>,
    shell: String,
    shell_args: Vec<String>,
    cwd: String,
    session_name: Option<String>,
    agent_id: Option<String>,
    agent_label: Option<String>,
    skip_tooling_save: bool,
    git_repos: Vec<SessionRepo>,
    mut skip_auto_resume: bool,
    resolved_spawn: Option<AgentSpawnCommand>,
    resolved_agent_host_shell: Option<ResolvedAgentHostShell>,
    viewport: Option<PtyViewport>,
    selection_intent: CreateSelectionIntent,
    pending_start_fresh: Option<bool>,
    pending_communication: Option<SessionCommunication>,
    inline_transaction: Option<&SelectionTransaction<R>>,
    inline_cause: Option<SelectionCause>,
    defer_inline_finalization: bool,
    preheld_target: Option<&crate::api::message_store::PtyInputTargetOwnership<'_>>,
    enforcement: crate::config::sessions_persistence::CreationGateEnforcement,
    coding_agent_binding: Option<crate::session::manager::CodingAgentBinding>,
    lifecycle_domain: Option<&LifecycleEntryAuthority>,
    owned_replacement: Option<crate::session::selection::OwnedReplacementContinuation>,
    initial_last_prompt: Option<InitialLastPrompt>,
) -> Result<CreateCompletion, String> {
    {
        let manager = session_mgr.read().await;
        crate::config::sessions_persistence::initialize_prompt_retention(&manager).await;
    }
    // #1327 - startup coding-agent updates gate: no session may open (GUI,
    // restore, restart, root agent, web, phone) before the startup update run
    // finishes or times out. Absent in tests (mock app, nothing managed) and in
    // non-GUI contexts -> no-op.
    if let Some(gate) = app.try_state::<Arc<crate::agent_update::AgentUpdateGate>>() {
        gate.wait_until_done().await;
    }
    if inline_transaction.is_some_and(|transaction| {
        transaction.has_restore_context() && !transaction.has_active_restore_capability()
    }) {
        if let Some(domain) = lifecycle_domain {
            domain
                .interrupted
                .store(true, std::sync::atomic::Ordering::Release);
        }
        return Err(crate::session::selection::SelectionCoordinatorError::Unavailable.to_string());
    }
    if !inline_transaction.is_some_and(|transaction| transaction.has_active_restore_capability()) {
        if let Some(coordinator) = app.try_state::<SelectionCoordinator>() {
            coordinator.wait_for_restore_release().await?;
        }
    }
    let cwd = crate::path_utils::normalize_windows_verbatim_path(&cwd);
    let (create_target_key, create_target_replica_identity) =
        resolve_create_destination(&cwd, lifecycle_domain, preheld_target, inline_transaction)?;
    let _owned_target_gate = acquire_create_target_lease(
        app,
        create_target_key.as_deref(),
        create_target_replica_identity.as_ref(),
        preheld_target.is_some(),
        lifecycle_domain.is_some(),
    )
    .await?;
    let create_manager = session_mgr.read().await.clone();
    let (create_nonce, create_binding) = match owned_replacement.as_ref() {
        Some(continuation) => match continuation.binding() {
            Some(binding) => (continuation.operation(), binding),
            None => return Ok(CreateCompletion::Retained),
        },
        None => (
            Uuid::new_v4(),
            create_manager.reserve_carrier_create_binding(),
        ),
    };
    let create_owner = Arc::new(Mutex::new(OwnedCreateAdmission {
        preheld_target: preheld_target.is_some(),
        owned_replacement: owned_replacement.clone(),
        replacement_pty: owned_replacement.as_ref().map(|_| pty_mgr.clone()),
        tracker: pty_mgr
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .response_close_state(),
        target: create_target_key
            .clone()
            .unwrap_or_else(|| format!("create-cwd:{}", cwd)),
        cwd: PathBuf::from(&cwd),
        nonce: create_nonce,
        binding: create_binding,
        reservation: None,
        failed_foreign: false,
        owned_target: _owned_target_gate,
        capacity: None,
        resource: None,
        blocked_bound: false,
        capacity_error: None,
    }));
    let session_label = session_name.as_deref().unwrap_or(&shell).to_string();
    let coordinator = app
        .try_state::<SelectionCoordinator>()
        .ok_or_else(|| "selectionCoordinatorUnavailable".to_string())?;
    let create_ticket = Arc::new(Mutex::new(match inline_transaction {
        Some(_) => None,
        None => Some(match selection_intent.trusted() {
            Some(intent) => coordinator
                .reserve_create(intent)
                .await
                .map_err(|error| error.to_string())?,
            None => coordinator
                .reserve_suppressed_create()
                .await
                .map_err(|error| error.to_string())?,
        }),
    }));
    if let Some(continuation) = owned_replacement.as_ref() {
        if let Some(ticket) = create_ticket
            .lock()
            .map_err(|_| "create ticket unavailable".to_string())?
            .as_mut()
        {
            ticket.retain_owned_replacement(continuation.clone())?;
        }
    }
    if inline_transaction.is_none() {
        create_owner
            .lock()
            .map_err(|_| "targetCreateGateUnavailable".to_string())?
            .capacity = Some(CreateCapacityContext {
            ticket: create_ticket.clone(),
            coordinator: coordinator.inner().clone(),
        });
    }
    let inline_pending_binding = Arc::new(Mutex::new(None));
    let inline_pending_for_body = Arc::clone(&inline_pending_binding);
    // (#2413) Close-marker ticket of this create; the body fills it, the outer
    // scope ends it after the select.
    let marker_ticket = Mutex::new(None::<CoordinatorCreateTicket<R>>);
    let create_body = async {
        // §1295 5.1a creation gate: FIRST statement of create_body, BEFORE the
        // existing `enforce_unarchived_for_spawn` (which the archive-gate source
        // test :7438 counts separately). Reading the SAME normalized `cwd` string
        // the archive gate receives; no resource permit and no pending row exist
        // yet, so a rejection leaves zero RAM/disk residue and needs no
        // `rollback_pre_created_session`. (dev-rust E: the two call strings
        // at :1443/:1708 are untouched.)
        crate::config::sessions_persistence::enforce_creation_gate(app, &cwd, enforcement).await?;
        enforce_owned_unarchived_for_spawn(
            app,
            &cwd,
            &session_label,
            &create_owner,
            &create_manager,
            lifecycle_domain,
            inline_transaction,
        )
        .await?;
        // An unarchived path has no native activation/admission callback.
        // It still reserves healthy ownership before the first create effect.
        let initial_admission = acquire_lifecycle_create_publication_region(
            &create_owner,
            &create_manager,
            lifecycle_domain,
            inline_transaction,
        )
        .await?;
        drop(initial_admission);
        let (agent_id, agent_label) = {
            if let Some(spawn) = resolved_spawn.as_ref() {
                (
                    Some(spawn.trusted_agent_id.clone()),
                    Some(spawn.trusted_agent_label.clone()),
                )
            } else {
                let settings_state = app.state::<SettingsState>();
                let cfg = settings_state
                    .current_read()
                    .await
                    .map_err(|error| error.to_string())?;
                resolve_actual_agent(
                    &shell,
                    &shell_args,
                    agent_id.as_deref(),
                    agent_label.as_deref(),
                    &cfg,
                )
            }
        };

        // Recompute is_coordinator from the current team snapshot. One source of truth:
        // every caller of create_session_inner gets the same computation.
        let teams = tokio::task::spawn_blocking(crate::config::teams::discover_teams)
            .await
            .map_err(|e| e.to_string())?;
        let is_coordinator = crate::config::teams::is_coordinator_for_cwd(&cwd, &teams);
        #[cfg(test)]
        let is_coordinator = is_coordinator || coordinator_create_seam::forced(&cwd);
        let is_root_agent = crate::config::root_agent::is_root_agent_path(&cwd);
        #[cfg(test)]
        let is_root_agent = is_root_agent || coordinator_create_seam::forced_root(&cwd);

        if is_coordinator {
            *marker_ticket.lock().unwrap_or_else(|e| e.into_inner()) =
                note_coordinator_create_on_app(app, &cwd);
        }

        // (#756) Durable fresh-intent mirror, consumed at the create path: the
        // caller requested resume (#599 reopen passes skipAutoResume=false), but
        // this cwd's coordinator carries a pending fresh boundary (restart or
        // successful logical clear (/clear or Pi /new) whose record died with
        // an auto/manual close). Force a fresh spawn BEFORE any provider resume
        // injection below (Claude/Pi/Antigravity --continue, Codex resume --last);
        // provider-agnostic by construction. The mirror is deliberately NOT cleared
        // here: only post-boundary content (typed input or AC-injected content)
        // drops it, so repeated close/reopen cycles with no content stay fresh.
        let mut fresh_forced_by_mirror = false;
        if is_coordinator && !skip_auto_resume {
            if let Some(clocks) =
                app.try_state::<crate::config::coordinator_clocks::CoordinatorClocksState>()
            {
                let fqn = crate::config::teams::agent_fqn_from_path(&cwd);
                let pending = {
                    let guard = clocks.lock().unwrap_or_else(|e| e.into_inner());
                    guard.start_fresh_at(&fqn)
                };
                if mirror_forces_fresh(skip_auto_resume, is_coordinator, pending.is_some()) {
                    log::info!(
                        "[session] fresh-intent mirror pending for '{}' (boundary {}): forcing \
                     skip_auto_resume (#756)",
                        fqn,
                        pending.map(|d| d.to_rfc3339()).unwrap_or_default()
                    );
                    skip_auto_resume = true;
                    fresh_forced_by_mirror = true;
                }
            }
        }

        let agent_kind = CodingAgentKind::detect(&shell, &shell_args);
        let is_agent_owned_launch = agent_id.is_some() || agent_kind.is_some() || is_root_agent;
        let coding_agent_binding = match coding_agent_binding.clone() {
            Some(binding) => Some(binding),
            None => match agent_id.as_ref() {
                Some(original_agent_id) => {
                    let settings_state = app.state::<SettingsState>();
                    let cfg = settings_state
                        .current_read()
                        .await
                        .map_err(|error| error.to_string())?;
                    let id = Uuid::new_v4().to_string();
                    let seed = match crate::config::settings::bootstrap_binding_revision(
                        &cfg,
                        original_agent_id,
                    ) {
                        Ok(identity_revision) => Some(serde_json::json!({
                            "identityRevision": identity_revision,
                            "originalAgentId": original_agent_id,
                        })),
                        Err(error) => {
                            log::warn!(
                                "[session] binding seed unavailable for '{}': {}",
                                original_agent_id,
                                error
                            );
                            None
                        }
                    };
                    Some(crate::session::manager::CodingAgentBinding {
                        id,
                        seed,
                        origin: None,
                    })
                }
                None => None,
            },
        };
        let resource_monitor = app.state::<Arc<ResourceMonitorState>>().inner().clone();
        let mut resource_permit = if is_agent_owned_launch {
            let cfg = app
                .state::<SettingsState>()
                .current_snapshot()
                .await
                .map_err(|error| error.to_string())?;
            resource_monitor.try_reserve_agent_slot(ResourceLimits::from(&cfg))?
        } else {
            None
        };

        create_owner
            .lock()
            .map_err(|_| "targetCreateGateUnavailable".to_string())?
            .resource = Some(Arc::new(Mutex::new(CreateResourceAdmission {
            monitor: resource_monitor.clone(),
            settings: app.state::<SettingsState>().inner().clone(),
            permit: None,
            needed: is_agent_owned_launch,
            in_region: false,
            released: false,
        })));

        // Clone the manager handle so the outer RwLock guard drops before spawn awaits.
        let mgr = session_mgr.read().await.clone();
        let backend_kind = resolved_spawn
            .as_ref()
            .map(|spawn| SessionBackendKind::from(&spawn.backend))
            .unwrap_or_default();
        if is_root_agent && backend_kind == SessionBackendKind::ContainerTransport {
            release_resource_launch_permit(&resource_monitor, &mut resource_permit);
            return Err("root-agent cannot use container transport".to_string());
        }
        let container_path_context = if backend_kind == SessionBackendKind::ContainerTransport {
            match container_path_context_for_cwd(&cwd) {
                Ok(context) => Some(context),
                Err(err) => {
                    release_resource_launch_permit(&resource_monitor, &mut resource_permit);
                    return Err(err);
                }
            }
        } else {
            None
        };
        // #935 - resolve the container's read-write repo bind mounts from its own
        // per-agent config.json repos[] (plan Sec 2/4), on the CANONICAL host root.
        // This runs BEFORE create_session below, so an inadmissible entry (an escape
        // signature: a repos[] rewrite reaching a sibling replica or messaging/) hard-
        // fails the spawn here with no session record, token, or container created
        // (plan Sec 4.3/4.4). None for local-process sessions.
        let container_repos = match container_path_context.as_ref() {
            Some(context) => {
                match crate::pty::container_repos::resolve_repo_mounts(std::path::Path::new(
                    &context.host_root,
                )) {
                    Ok(resolution) => Some(resolution),
                    Err(err) => {
                        release_resource_launch_permit(&resource_monitor, &mut resource_permit);
                        return Err(err);
                    }
                }
            }
            None => None,
        };
        // Attach the B capability already retained before create effects to
        // this ticket/inline owner. No binder, default insert, or queue
        // admission may mint another binding in the publication region.
        let prepared_binding = create_owner.lock().ok().map(|owner| owner.binding);
        let Some(prepared_binding) = prepared_binding else {
            return Ok(CreateCompletion::Retained);
        };
        let carrier_u0 = match with_create_resource_region(
            &create_owner,
            &mut resource_permit,
            acquire_lifecycle_create_publication_region(
                &create_owner,
                &mgr,
                lifecycle_domain,
                inline_transaction,
            ),
        )
        .await
        {
            Ok(permit) => permit,
            Err(error) => {
                release_resource_launch_permit(&resource_monitor, &mut resource_permit);
                return Err(error);
            }
        };
        let mut inline_reserved = None;
        if let Some(ticket) = create_ticket
            .lock()
            .map_err(|_| "create ticket unavailable".to_string())?
            .as_mut()
        {
            #[cfg(test)]
            if owned_replacement
                .as_ref()
                .is_some_and(|owned| owned.fixture_bind_twice())
            {
                assert_eq!(ticket.bind_prepared(prepared_binding)?, prepared_binding);
            }
            ticket.bind_prepared(prepared_binding)?;
        } else if let Some(transaction) = inline_transaction {
            #[cfg(test)]
            if owned_replacement
                .as_ref()
                .is_some_and(|owned| owned.fixture_bind_twice())
            {
                assert_eq!(
                    transaction
                        .prepare_inline_create_binding(prepared_binding, &mut inline_reserved)?,
                    prepared_binding
                );
            }
            transaction.prepare_inline_create_binding(prepared_binding, &mut inline_reserved)?;
        } else {
            return Err("inline create transaction is unavailable".to_string());
        }
        if let Some(transaction) = inline_transaction {
            if !transaction.retain_entry_create_binding(prepared_binding) {
                drop(carrier_u0);
                release_resource_launch_permit(&resource_monitor, &mut resource_permit);
                return Ok(CreateCompletion::Retained);
            }
        }
        // #1101: the restart replacement create runs while `teardown_old_for_restart`
        // has released the old PTY but RETAINED its canonical manager row (removed only
        // at the atomic commit's `mutations.remove(uuid)`, which runs after this gate).
        // For a live-session restart that row is non-Exited, so `appeared_session` is a
        // false positive and would refuse every restart of a live WG replica session.
        // Restart passes `preheld_target = None` and acquires its own exact target lock
        // (`_owned_target_gate`), so it is already serialized and is the authoritative
        // replacement; this cold-spawn dedup heuristic is redundant and wrong for it.
        // Skip the gate only for the restart cause. The wake path (inline_cause = None)
        // is unchanged and keeps its dedup.
        if create_target_replica_identity.is_some()
            && matches!(
                selection_intent,
                CreateSelectionIntent::Background | CreateSelectionIntent::Suppress
            )
            && !matches!(inline_cause, Some(SelectionCause::Restart(_)))
        {
            let target = create_target_replica_identity
                .as_ref()
                .ok_or_else(|| "targetCreateGateUnavailable".to_string())?;
            let appeared_session = mgr.list_sessions().await.into_iter().any(|session| {
                !matches!(session.status, SessionStatus::Exited(_))
                    && crate::path_identity::verify_directory(std::path::Path::new(
                        &session.working_directory,
                    ))
                    .is_ok_and(|cwd_identity| {
                        crate::path_identity::is_verified_descendant(&cwd_identity, target)
                    })
            });
            let appeared_spawn = pty_mgr
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .has_pending_spawn_for_replica(target);
            if appeared_session || appeared_spawn {
                drop(carrier_u0);
                release_resource_launch_permit(&resource_monitor, &mut resource_permit);
                return Err("sessionRace".to_string());
            }
        }
        let spawn_mark = {
            let pty = pty_mgr.lock().unwrap_or_else(|error| error.into_inner());
            pty.mark_spawning(&cwd, &session_label)
        };
        let pending_result = mgr
            .create_pending_session_with_binding_and_carrier_permit(
                prepared_binding,
                &carrier_u0,
                shell.clone(),
                shell_args.clone(),
                cwd.clone(),
                agent_id.clone(),
                agent_label.clone(),
                git_repos,
                is_coordinator,
                backend_kind,
            )
            .await
            .map(|session| (session, prepared_binding))
            .map_err(|error| error.to_string());
        let (mut session, pending_binding) = match pending_result {
            Ok(created) => created,
            Err(e) => {
                drop(carrier_u0);
                release_resource_launch_permit(&resource_monitor, &mut resource_permit);
                return Err(e);
            }
        };
        if owned_replacement
            .as_ref()
            .is_some_and(|continuation| !continuation.mark_pending(pending_binding))
        {
            drop(carrier_u0);
            release_resource_launch_permit(&resource_monitor, &mut resource_permit);
            return Ok(CreateCompletion::Retained);
        }
        if inline_transaction
            .is_some_and(|transaction| !transaction.observe_entry_pending_binding(pending_binding))
        {
            drop(carrier_u0);
            release_resource_launch_permit(&resource_monitor, &mut resource_permit);
            return Ok(CreateCompletion::Retained);
        }
        let previous = match owned_replacement.as_ref() {
            Some(continuation) => match continuation.previous_for_launch(&carrier_u0) {
                Some(previous) => Some(previous),
                None => {
                    drop(carrier_u0);
                    release_resource_launch_permit(&resource_monitor, &mut resource_permit);
                    return Ok(CreateCompletion::Retained);
                }
            },
            None => None,
        };
        if let Some(previous) = previous {
            // Carry is local launch input only. FinalizedOwned performs the
            // exact persisted carry after B's real finalization/version receipt.
            // The producer requests resume; A's unconsumed durable fresh intent
            // still determines the actual runtime argv before launch.
            skip_auto_resume = previous.start_fresh_on_restore;
            session.telegram_bot_id = previous.telegram_bot_id;
            session.communication = previous.communication;
            session.start_fresh_on_restore = previous.start_fresh_on_restore;
        }
        if let Err(error) = mgr
            .install_pending_coding_agent_binding_with_carrier_permit(
                &carrier_u0,
                pending_binding,
                coding_agent_binding,
            )
            .await
        {
            drop(carrier_u0);
            if owned_replacement.is_none() {
                if let Some(transaction) = inline_transaction {
                    transaction.rollback_inline_create(pending_binding).await;
                }
            }
            release_resource_launch_permit(&resource_monitor, &mut resource_permit);
            return Err(error);
        }
        if inline_transaction.is_some() {
            *inline_pending_for_body
                .lock()
                .unwrap_or_else(|error| error.into_inner()) = Some(pending_binding);
        }
        if let Some(value) = pending_start_fresh {
            mgr.set_pending_start_fresh_on_restore_with_carrier_permit(
                &carrier_u0,
                pending_binding,
                value,
            )
            .await
            .map_err(|error| error.to_string())?;
            session.start_fresh_on_restore = value;
        }
        if let Some(communication) = pending_communication.clone() {
            mgr.set_pending_communication_with_carrier_permit(
                &carrier_u0,
                pending_binding,
                communication.clone(),
            )
            .await
            .map_err(|error| error.to_string())?;
            session.communication = Some(communication);
        }

        drop(carrier_u0);

        // #1063 test-only barrier: the pending row now exists and is visible to a
        // deletion's pending-inclusive snapshot; pause here before any config-seed
        // project gate so a delete can interleave deterministically. No-op in production.
        #[cfg(test)]
        {
            seed_race_barriers::hit_before_project_gate(&cwd).await;
        }

        if let Err(e) = with_create_resource_region(
            &create_owner,
            &mut resource_permit,
            enforce_owned_unarchived_for_spawn(
                app,
                &cwd,
                &session_label,
                &create_owner,
                &create_manager,
                lifecycle_domain,
                inline_transaction,
            ),
        )
        .await
        {
            let err = e.to_string();
            release_resource_launch_permit(&resource_monitor, &mut resource_permit);
            drop(mgr);
            rollback_pre_created_session(app, session_mgr, pty_mgr, session.id, &err).await;
            return Err(err);
        }

        let carrier_u1 = match with_create_resource_region(
            &create_owner,
            &mut resource_permit,
            acquire_lifecycle_create_publication_region(
                &create_owner,
                &mgr,
                lifecycle_domain,
                inline_transaction,
            ),
        )
        .await
        {
            Ok(permit) => permit,
            Err(error) => {
                release_resource_launch_permit(&resource_monitor, &mut resource_permit);
                return Err(error);
            }
        };
        // (#756) Propagate the mirror-forced intent onto the NEW record: the
        // startup-restore path reads ONLY the record, so without this an app close
        // after the forced-fresh reopen would resume the pre-boundary conversation.
        // No persist call here: the command, restart and both restore callers
        // persist after inner returns; the wake/loop/web creates do not, which is
        // fine because the mirror is already persisted and re-consumed by the
        // guard above on every later create, and the record stamp reaches disk
        // with the next persist-any.
        if fresh_forced_by_mirror {
            mgr.set_pending_start_fresh_on_restore_with_carrier_permit(
                &carrier_u1,
                pending_binding,
                true,
            )
            .await
            .map_err(|error| error.to_string())?;
        }

        if is_root_agent {
            mgr.set_pending_is_root_agent_with_carrier_permit(&carrier_u1, pending_binding, true)
                .await
                .map_err(|error| error.to_string())?;
            session.is_root_agent = true;
        }

        if let Some(name) = session_name {
            if let Err(e) = mgr
                .rename_pending_session_with_carrier_permit(
                    &carrier_u1,
                    pending_binding,
                    name.clone(),
                )
                .await
            {
                let err = e.to_string();
                drop(carrier_u1);
                release_resource_launch_permit(&resource_monitor, &mut resource_permit);
                drop(mgr);
                rollback_pre_created_session(app, session_mgr, pty_mgr, session.id, &err).await;
                return Err(err);
            }
            session.name = name;
        }

        let id = session.id;
        if let Some(spawn) = resolved_spawn.as_ref() {
            let effective_codex_home = effective_codex_home_for_backend(
                session.backend_kind,
                container_path_context.as_ref().map(|context| &context.map),
                spawn,
            );
            mgr.set_pending_profile_metadata_with_carrier_permit(
                &carrier_u1,
                pending_binding,
                Some(spawn.profile_resolution.requested_profile.clone()),
                Some(spawn.profile_resolution.effective_profile.clone()),
                spawn.profile_resolution.fallback_chain.clone(),
                spawn.profile_resolution.fallback_applied,
                spawn.match_tier.clone(),
                spawn.original_profile_letter.clone(),
                effective_codex_home.clone(),
                Some(spawn.profile_content_hash.clone()),
            )
            .await
            .map_err(|error| error.to_string())?;
            session.requested_profile = Some(spawn.profile_resolution.requested_profile.clone());
            session.effective_profile = Some(spawn.profile_resolution.effective_profile.clone());
            session.profile_fallback_chain = spawn.profile_resolution.fallback_chain.clone();
            session.profile_fallback_applied = spawn.profile_resolution.fallback_applied;
            session.match_tier = spawn.match_tier.clone();
            session.original_profile_letter = spawn.original_profile_letter.clone();
            session.effective_codex_home = effective_codex_home;
            session.profile_content_hash = Some(spawn.profile_content_hash.clone());
        }

        let mut shell_args = shell_args;
        let mut effective_continuation = false;
        let configured_identity = configured_prompt_identity(agent_kind, &shell, &shell_args);
        let full_cmd = format!("{} {}", shell, shell_args.join(" "));
        // #260: single detector (session/profile.rs). Replaces the old
        // starts_with triple; this is the SAME call `strip_auto_injected_args`
        // makes, so the persisted recipe and the runtime identity cannot disagree.
        let context_target = match agent_kind {
            Some(CodingAgentKind::Claude) => {
                Some(crate::config::session_context::ManagedContextTarget::Claude)
            }
            Some(CodingAgentKind::Codex) => {
                Some(crate::config::session_context::ManagedContextTarget::Codex)
            }
            Some(CodingAgentKind::Antigravity) => {
                Some(crate::config::session_context::ManagedContextTarget::Antigravity)
            }
            Some(CodingAgentKind::Pi) => {
                Some(crate::config::session_context::ManagedContextTarget::Pi)
            }
            // #1873 - Muse has no managed context target; a configured agent still
            // resolves its instructions filename (AGENTS.md) via `agent_command`.
            Some(CodingAgentKind::Muse) => None,
            None => None,
        };

        // Single source of truth: store the identity on the SessionManager record
        // AND the local clone (the latter feeds the imminent `session_created` emit).
        mgr.set_pending_agent_kind_with_carrier_permit(&carrier_u1, pending_binding, agent_kind)
            .await
            .map_err(|error| error.to_string())?;
        session.agent_kind = agent_kind;
        let trusted_configured_spawn = resolved_spawn.is_some();
        mgr.set_pending_trusted_configured_spawn_with_carrier_permit(
            &carrier_u1,
            pending_binding,
            trusted_configured_spawn,
        )
        .await
        .map_err(|error| error.to_string())?;
        session.trusted_configured_spawn = trusted_configured_spawn;

        drop(carrier_u1);
        if let Some(continuation) = owned_replacement.as_ref() {
            if !continuation.validate_source().await {
                release_resource_launch_permit(&resource_monitor, &mut resource_permit);
                return Ok(CreateCompletion::Retained);
            }
        }

        // #930 - resolve the host-credential copy-in plan for container coding agents
        // BEFORE the resume probe and the container env translation, so both consumers
        // observe the CLAUDE_CONFIG_DIR we may inject below. Gated by the global setting
        // (default on) and the per-agent profile descriptor; None when off,
        // non-container, an unrecognized agent, or the host file is absent. The plan is
        // pure (env read + file-exists check); the actual copy runs later in the
        // container backend spawn (spawn_runtime_backed), preserving copy-after-seed.
        let spawn_cwd = container_path_context
            .as_ref()
            .map(|context| context.host_root.clone())
            .unwrap_or_else(|| cwd.clone());
        let container_credential = if session.backend_kind == SessionBackendKind::ContainerTransport
        {
            let settings_state = app.state::<SettingsState>();
            let copy_enabled = {
                let cfg = match settings_state.current_read().await {
                    Ok(cfg) => cfg,
                    Err(error) => {
                        let error = error.to_string();
                        release_resource_launch_permit(&resource_monitor, &mut resource_permit);
                        drop(mgr);
                        rollback_pre_created_session(app, session_mgr, pty_mgr, id, &error).await;
                        return Err(error);
                    }
                };
                cfg.container_credentials_from_host
            };
            if copy_enabled {
                agent_kind
                    .and_then(|k| k.profile().container_credential)
                    .and_then(|src| {
                        crate::pty::container_credentials::resolve_plan(&src, &spawn_cwd)
                    })
            } else {
                None
            }
        } else {
            None
        };
        // #930 - when we WILL copy host creds and the user configured no
        // CLAUDE_CONFIG_DIR (respecting an explicit value OR an explicit removal),
        // default it to the copy directory so a default-on container authenticates with
        // zero env rows. Injected as a host path; the container env translation maps it
        // to /workspace/.claude and the resume probe then treats state as durable.
        let user_has_claude_config_dir = resolved_spawn
            .as_ref()
            .map(|spawn| {
                spawn.effective_env_value(CLAUDE_CONFIG_DIR_KEY).is_some()
                    || spawn.env_remove_keys.iter().any(|remove| {
                        crate::config::settings::normalize_env_key_for_platform(remove)
                            == crate::config::settings::normalize_env_key_for_platform(
                                CLAUDE_CONFIG_DIR_KEY,
                            )
                    })
            })
            .unwrap_or(false);
        let injected_claude_config_dir = injected_claude_config_dir_for_copy(
            container_credential.as_ref(),
            user_has_claude_config_dir,
        );

        // Resolve and inspect Claude transcript storage only for a canonically
        // detected Claude launch. Provider-looking Pi option values must not cause
        // any Claude path resolution, filesystem check, warning, or stored memo.
        let is_claude = agent_kind == Some(CodingAgentKind::Claude);
        let resume_probe = claude_resume_probe_target_for_kind(
            agent_kind,
            session.backend_kind,
            container_path_context.as_ref().map(|context| &context.map),
            resolved_spawn.as_ref(),
            injected_claude_config_dir.as_deref(),
            &shell,
            &shell_args,
            &cwd,
        );
        let resolved_claude_projects_dir = resume_probe
            .as_ref()
            .and_then(|probe| probe.host_probe_path.clone());
        let carrier_projects = match with_create_resource_region(
            &create_owner,
            &mut resource_permit,
            acquire_lifecycle_create_publication_region(
                &create_owner,
                &mgr,
                lifecycle_domain,
                inline_transaction,
            ),
        )
        .await
        {
            Ok(permit) => permit,
            Err(error) => {
                release_resource_launch_permit(&resource_monitor, &mut resource_permit);
                return Err(error);
            }
        };
        mgr.set_pending_resolved_claude_projects_dir_with_carrier_permit(
            &carrier_projects,
            pending_binding,
            resolved_claude_projects_dir.clone(),
        )
        .await
        .map_err(|error| error.to_string())?;
        drop(carrier_projects);
        if let Some(continuation) = owned_replacement.as_ref() {
            if !continuation.validate_source().await {
                release_resource_launch_permit(&resource_monitor, &mut resource_permit);
                return Ok(CreateCompletion::Retained);
            }
        }
        session.resolved_claude_projects_dir = resolved_claude_projects_dir.clone();
        let claude_project_exists = resume_probe
            .as_ref()
            .and_then(|probe| probe.host_probe_path.as_ref())
            .is_some_and(|path| path.is_dir());
        let mut session_env_warnings: Vec<ContainerEnvWarning> = Vec::new();
        if let Some(warning) = resume_probe
            .as_ref()
            .and_then(|probe| probe.warning.clone())
        {
            session_env_warnings.push(warning);
        }
        let will_inject_continue = should_inject_continue(
            is_claude,
            skip_auto_resume,
            claude_project_exists,
            &full_cmd,
        );
        if let Some(probe) = resume_probe.as_ref() {
            log::info!(
                "[session] claude-resume-decision {} cwd={:?} projects_dir={:?} filesystem={} \
                 exists={} skip_auto_resume={} -> inject_continue={}",
                &id.to_string()[..8],
                cwd,
                resolved_claude_projects_dir,
                probe.filesystem,
                claude_project_exists,
                skip_auto_resume,
                will_inject_continue
            );
        } else if let Some(kind) = agent_kind {
            log::info!(
                "[session] resume-decision {} agent={} cwd={:?} skip_auto_resume={}",
                &id.to_string()[..8],
                kind.as_str(),
                cwd,
                skip_auto_resume
            );
        }
        if will_inject_continue {
            // #260: Claude's resume flag from the CodingAgentProfile. resume_tokens
            // is a 1-element const for Claude, so [0] is provably in bounds.
            let continue_flag = CodingAgentKind::Claude.profile().resume_tokens[0];
            if let Some(ref aid) = agent_id {
                if executable_basename(&shell) == "cmd" {
                    if let Some(last) = shell_args.last_mut() {
                        if executable_basename(last) == "claude"
                            || last.to_lowercase().contains("claude")
                        {
                            *last = format!("{} {}", last, continue_flag);
                            effective_continuation = true;
                            log::info!(
                                "Auto-injected --continue for agent '{}' \
                                 (prior conversation exists, cmd path)",
                                aid
                            );
                        }
                    }
                } else {
                    shell_args.push(continue_flag.to_string());
                    effective_continuation = true;
                    log::info!(
                        "Auto-injected --continue for agent '{}' (prior conversation exists)",
                        aid
                    );
                }
            }
        }

        // (#756) Rider: on Claude spawns where AC suppresses resume, mint the fresh
        // session's identity at spawn. Uuid::new_v4() satisfies the "unused UUID"
        // requirement (Claude errors on a colliding --session-id; v4 collision is
        // negligible and a fresh one is minted per spawn). Mutually exclusive with
        // the --continue block above by construction (will_inject_continue requires
        // skip_auto_resume=false). `full_cmd` predates both blocks' mutations, so
        // the veto scan sees user-configured identity flags, never AC's own.
        if should_inject_fresh_session_id(is_claude, skip_auto_resume, &full_cmd) {
            if let Some(ref aid) = agent_id {
                let fresh_session_id = Uuid::new_v4();
                if executable_basename(&shell) == "cmd" {
                    if let Some(last) = shell_args.last_mut() {
                        if executable_basename(last) == "claude"
                            || last.to_lowercase().contains("claude")
                        {
                            *last = format!("{} --session-id {}", last, fresh_session_id);
                            log::info!(
                            "Auto-injected --session-id {} for agent '{}' (fresh spawn, cmd path, \
                             #756)",
                            fresh_session_id,
                            aid
                        );
                        }
                    }
                } else {
                    shell_args.push("--session-id".to_string());
                    shell_args.push(fresh_session_id.to_string());
                    log::info!(
                        "Auto-injected --session-id {} for agent '{}' (fresh spawn, #756)",
                        fresh_session_id,
                        aid
                    );
                }
            }
        }

        if maybe_inject_pi_resume(
            agent_kind,
            resolved_spawn.as_ref(),
            skip_auto_resume,
            &shell,
            &mut shell_args,
        ) {
            effective_continuation = true;
            if let Some(spawn) = resolved_spawn.as_ref() {
                log::info!(
                    "Auto-injected Pi `--continue` for trusted agent '{}'",
                    spawn.trusted_agent_id
                );
            }
        }

        if agent_kind == Some(CodingAgentKind::Codex) && !skip_auto_resume {
            if let Some(ref aid) = agent_id {
                if inject_codex_resume(&shell, &mut shell_args) {
                    effective_continuation = true;
                    log::info!("Auto-injected `codex resume --last` for agent '{}'", aid);
                }
            }
        }

        if agent_kind == Some(CodingAgentKind::Antigravity) && !skip_auto_resume {
            if let Some(ref aid) = agent_id {
                if inject_antigravity_resume(&shell, &mut shell_args) {
                    effective_continuation = true;
                    log::info!("Auto-injected `agy --continue` for agent '{}'", aid);
                }
            }
        }

        // #1873 - Muse workspace-latest resume: only a trusted, exact, empty-argv,
        // local macOS/Linux recipe with resume intent receives `resume --last`,
        // appended to the LOCAL effective vector only. Fresh intent, configured
        // arguments, wrappers, containers, Windows and ad-hoc launches stay plain.
        if maybe_inject_muse_resume(
            agent_kind,
            resolved_spawn.as_ref(),
            skip_auto_resume,
            &shell,
            &mut shell_args,
        ) {
            effective_continuation = true;
            if let Some(spawn) = resolved_spawn.as_ref() {
                log::info!(
                    "Auto-injected Muse `resume --last` for trusted agent '{}'",
                    spawn.trusted_agent_id
                );
            }
        }

        // #529 - resolve the instructions filename from the configured coding agent
        // (falling back to detection for ad-hoc launches), plus the union of every
        // configured agent's filename for cleanup. Computed under a single settings
        // read guard that is dropped before any filesystem I/O (no guard across the
        // materialize call).
        let (target_filename, managed_filenames, auto_self_clear): (
            Option<String>,
            Vec<String>,
            bool,
        ) = {
            let settings_state = app.state::<SettingsState>();
            let cfg = match settings_state.current_read().await {
                Ok(cfg) => cfg,
                Err(error) => {
                    let error = error.to_string();
                    release_resource_launch_permit(&resource_monitor, &mut resource_permit);
                    drop(mgr);
                    rollback_pre_created_session(app, session_mgr, pty_mgr, id, &error).await;
                    return Err(error);
                }
            };
            let managed = crate::config::agent_command::managed_instructions_filenames(&cfg);
            let target = crate::config::agent_command::resolve_target_filename(
                agent_id.as_deref(),
                &cfg,
                context_target,
            );
            // Resolve under the same guard; no second lock is held across I/O.
            // Eligibility is an explicit direct-shell capability, independent of
            // CodingAgentKind provider tuning.
            let auto_self_clear =
                resolve_launch_auto_self_clear(&cfg, &shell, &cwd, is_coordinator);
            (target, managed, auto_self_clear)
        };

        if let Some(ref target_filename) = target_filename {
            if owned_replacement.is_some() {
                revalidate_owned_create_effect(
                    &create_owner,
                    &mut resource_permit,
                    &resource_monitor,
                    &mgr,
                    pty_mgr,
                )
                .await?;
            }
            let context_effect = inline_transaction
                .and_then(|transaction| {
                    transaction.start_lifecycle_effect(
                        crate::session::selection::LifecycleEffectSlot::ContextPreparation,
                    )
                })
                .or_else(|| {
                    owned_replacement.as_ref().and_then(|continuation| {
                        continuation.start_effect(
                            crate::session::selection::LifecycleEffectSlot::ContextPreparation,
                        )
                    })
                });
            if (owned_replacement.is_some()
                || inline_transaction.is_some_and(|transaction| transaction.has_lifecycle_entry()))
                && context_effect.is_none()
            {
                release_resource_launch_permit(&resource_monitor, &mut resource_permit);
                return Ok(CreateCompletion::Retained);
            }
            #[cfg(test)]
            let context_fixture = owned_replacement
                .as_ref()
                .and_then(|owned| owned.fixture_native(false));
            #[cfg(test)]
            let native_await_panic = owned_replacement
                .as_ref()
                .and_then(|owned| owned.fixture_take_native_await_panic(false));
            let context_native = coordinator.run_blocking_seed_work({
                let cwd = cwd.clone();
                // #1172 D5: capture the FINAL fresh-versus-resume decision. `skip_auto_resume`
                // is a `mut` parameter of this function whose only mutation is the #756 mirror
                // at :1470, ~500 lines above, so the value read here is final. `true` means AC
                // is deliberately NOT resuming: a fresh conversation begins.
                let start_fresh = skip_auto_resume;
                let target_filename = target_filename.clone();
                let managed_filenames = managed_filenames.clone();
                let container_repos = container_repos.clone();
                move || {
                    #[cfg(test)]
                    if let Some(control) = context_fixture.as_ref() {
                        control.enter();
                    }
                    // #1065 Stage F: production records the session-spawn context
                    // read/sync and self-heal under the per-project gate (acquired
                    // inside the blocking worker so the 10-second poll never blocks a
                    // Tokio worker); a `#[cfg(test)]` lib build stays non-emitting.
                    #[cfg(not(test))]
                    let activation =
                        Some(crate::config::seed_manifest::ManifestActivationToken::production());
                    #[cfg(test)]
                    let activation: Option<
                        crate::config::seed_manifest::ManifestActivationToken,
                    > = None;
                    use crate::config::session_context as ctx_file;
                    let context_result =
                        ctx_file::materialize_agent_context_file_with_filename_activated(
                            &cwd,
                            &target_filename,
                            &managed_filenames,
                            is_coordinator,
                            auto_self_clear,
                            container_repos.as_ref(),
                            activation.as_ref(),
                        );
                    // #1172 - rotate the origin Agent Matrix's `memory/` for the fresh session
                    // about to start, so it begins with a clean write target and the previous
                    // session's memory is preserved under `memory-archive/memory_<ts>/`.
                    //
                    // Two gates, both load-bearing (D5):
                    //   - `start_fresh`: a RESUME never rotates. This chokepoint also
                    //     serves the app-startup restore path
                    //     (`create_session_inner_for_restore`, :1239), which continues an
                    //     existing conversation; emptying `memory/` under a resumed agent
                    //     is the one outcome the user ruled out.
                    //   - `is_ok()`: a launch that is about to roll back (:2016-2036)
                    //     leaves no rotation behind.
                    //
                    // Never fails a launch: `rotate_origin_memory_at_spawn` returns `()`
                    // and every error path inside it warns and returns.
                    if start_fresh && context_result.is_ok() {
                        crate::config::agent_memory::rotate_origin_memory_at_spawn(&cwd);
                    }
                    if let Some(effect) = context_effect {
                        effect
                            .complete(context_result.as_ref().map(|_| None).map_err(Clone::clone));
                    }
                    #[cfg(test)]
                    if let Some(control) = context_fixture.as_ref() {
                        control.returned(format!("{context_result:?}"));
                    }
                    context_result
                }
            });
            #[cfg(test)]
            let context_result =
                crate::session::selection::await_fixture_native(context_native, native_await_panic)
                    .await;
            #[cfg(not(test))]
            let context_result = context_native.await;
            let context_result = match context_result {
                Ok(result) => result,
                Err(error) => {
                    let error = format!("replica context blocking preparation failed: {error}");
                    release_resource_launch_permit(&resource_monitor, &mut resource_permit);
                    drop(mgr);
                    rollback_pre_created_session(app, session_mgr, pty_mgr, id, &error).await;
                    return Err(error);
                }
            };
            match context_result {
                Ok(_) => {}
                Err(e) => {
                    log::error!("Replica context validation failed: {}", e);
                    use tauri_plugin_dialog::DialogExt;
                    // #537 facet (b) - the old copy blamed "context files missing",
                    // but the real cause is usually a transient config.json lock
                    // during replica identity repair. State what actually failed;
                    // the interpolated error carries the precise, retry-suggesting
                    // detail from format_publish_error.
                    let dialog_msg = format!(
                        "Cannot launch session - failed to update replica config:\n\n{}",
                        e
                    );
                    let dialog_title = "Session Launch Error";
                    #[cfg(test)]
                    let notification_observed = owned_replacement.as_ref().is_some_and(|owned| {
                        owned.fixture_assert_context_failure_observer(
                            &e,
                            dialog_title,
                            &dialog_msg,
                        );
                        owned.fixture_observe_context_failure(&e, dialog_title, &dialog_msg)
                    });
                    #[cfg(not(test))]
                    let notification_observed = false;
                    if !notification_observed {
                        app.dialog()
                            .message(&dialog_msg)
                            .title(dialog_title)
                            .show(|_| {});
                    }
                    release_resource_launch_permit(&resource_monitor, &mut resource_permit);
                    drop(mgr);
                    rollback_pre_created_session(app, session_mgr, pty_mgr, id, &e).await;
                    return Err(e);
                }
            }
        }

        // Capture the effective arg vector BEFORE spawn so SessionInfo::from(&session)
        // (emitted at line ~439 as "session_created") carries provider resume flags.
        // Bind once, broadcast to two consumers: the store write is for later
        // `mgr.get_session` callers; the local-clone write is for the imminent emit.
        //
        // DO NOT REMOVE OR GATE THIS CAPTURE. Issue #65 regression guard: removing
        // or wrapping in a condition reintroduces the exact bug this plan fixes.
        // See _plans/bug-statusbar-dynamic-launch-args.md §10 and §15 for rationale.
        let effective = shell_args.clone();
        let carrier_args = match with_create_resource_region(
            &create_owner,
            &mut resource_permit,
            acquire_lifecycle_create_publication_region(
                &create_owner,
                &mgr,
                lifecycle_domain,
                inline_transaction,
            ),
        )
        .await
        {
            Ok(permit) => permit,
            Err(error) => {
                release_resource_launch_permit(&resource_monitor, &mut resource_permit);
                return Err(error);
            }
        };
        mgr.set_pending_effective_shell_args_with_carrier_permit(
            &carrier_args,
            pending_binding,
            effective.clone(),
        )
        .await
        .map_err(|error| error.to_string())?;
        drop(carrier_args);
        if let Some(continuation) = owned_replacement.as_ref() {
            if !continuation.validate_source().await {
                release_resource_launch_permit(&resource_monitor, &mut resource_permit);
                return Ok(CreateCompletion::Retained);
            }
        }
        session.effective_shell_args = Some(effective);

        let extra_env = if agent_id.is_some() {
            crate::pty::credentials::build_credentials_env(&session.token, &cwd)
        } else {
            Vec::new()
        };
        let mut configured_env: Vec<(String, String)> = resolved_spawn
            .as_ref()
            .map(|spawn| spawn.child_env.clone())
            .unwrap_or_default();
        // #930 - inject the copy directory as a host-path CLAUDE_CONFIG_DIR so the
        // container env translation below maps it to /workspace/.claude and the copied
        // token is actually read. Only present when we will copy and the user set none.
        if let Some(dir) = injected_claude_config_dir.as_ref() {
            configured_env.push((CLAUDE_CONFIG_DIR_KEY.to_string(), dir.clone()));
        }
        let env_remove_keys: Vec<String> = resolved_spawn
            .as_ref()
            .map(|spawn| spawn.env_remove_keys.clone())
            .unwrap_or_default();
        let mut env_unset: Vec<String> = Vec::new();
        if session.backend_kind == SessionBackendKind::ContainerTransport {
            if let Some(context) = container_path_context.as_ref() {
                let translated = crate::pty::container_backend::container_child_env(
                    configured_env,
                    env_remove_keys.clone(),
                    &context.map,
                );
                configured_env = translated.child_env;
                env_unset = translated.env_unset;
                session_env_warnings.extend(translated.warnings);
            }
        }
        let (resource_registration, logical_resource_slot): (
            Option<ResourceLaunchRegistration>,
            Option<ResourceLogicalAgentSlot>,
        ) = match session.backend_kind {
            SessionBackendKind::LocalProcess => resource_permit
                .take()
                .map(|permit| {
                    // #516 - human-readable WG + agent identity for the Resource Monitor row,
                    // derived from the deterministic spawn cwd (not the user-renamable
                    // session.name). Root agents carry no wg-/__agent_ segments, so label them
                    // explicitly with the bare replica dir name.
                    let (workgroup, agent) = {
                        let (wg, ag) = crate::config::teams::workgroup_and_agent_from_path(&cwd);
                        if wg.is_some() {
                            (wg, ag)
                        } else if is_root_agent {
                            let bare = cwd
                                .replace('\\', "/")
                                .rsplit('/')
                                .find(|s| !s.is_empty())
                                .map(|s| s.to_string());
                            (Some("Root agent".to_string()), bare)
                        } else {
                            (None, None)
                        }
                    };
                    // #566 - project folder name for the Resource Monitor row, derived from
                    // the same deterministic spawn cwd as the (workgroup, agent) pair above.
                    let project = crate::config::teams::project_from_path(&cwd);
                    ResourceLaunchRegistration::new(
                        resource_monitor.as_ref().clone(),
                        permit,
                        ResourceLaunchMetadata {
                            session_id: id,
                            name: session.name.clone(),
                            agent_id: agent_id.clone(),
                            agent_label: agent_label.clone(),
                            workgroup,
                            agent,
                            project,
                        },
                    )
                })
                .map(|registration| (Some(registration), None))
                .unwrap_or((None, None)),
            SessionBackendKind::ContainerTransport => (
                None,
                resource_permit
                    .take()
                    .and_then(|permit| resource_monitor.hold_logical_agent_slot(permit)),
            ),
        };

        // #598 - seed the config folder before the PTY starts so the agent sees the
        // templated config. Best-effort: never aborts the spawn. This is the single
        // execution chokepoint every real spawn funnels through (create / replica /
        // other variants, delivery / mailbox / web wakes); prevalidation never hits
        // it because it discards the spawn. Per Q2 this runs on EVERY spawn,
        // including loop/scheduler resume-wakes (overwrite every spawn).
        if let Some(seed) = resolved_spawn.as_ref().and_then(|s| s.seed.as_ref()) {
            // grinch HIGH-1: serialize the seed swap for ALL dests (not just
            // `.claude`). Two same-replica spawns can run concurrently (e.g. a
            // delivery tick + a mailbox wake) holding only `session_mgr.read()`,
            // with no global spawn lock. Without serialization, spawn B's
            // prefix-sweep of stale scratch (`clear_stale_seed_scratch`) could
            // delete spawn A's in-flight temp/trash mid-swap and lose the config
            // (breaking H1's "dest always fully-old or fully-new" + M3 isolation).
            // Under this lock any other-id scratch is truly stale, so the
            // leak-reclaim sweep stays safe. Clone the Arc out of State first so the
            // owned guard does not borrow a State temporary (E0716).
            let seed = seed.clone();
            if owned_replacement.is_some() {
                revalidate_owned_create_effect(
                    &create_owner,
                    &mut resource_permit,
                    &resource_monitor,
                    &mgr,
                    pty_mgr,
                )
                .await?;
            }
            let seed_effect = inline_transaction
                .and_then(|transaction| {
                    transaction.start_lifecycle_effect(
                        crate::session::selection::LifecycleEffectSlot::ConfigSeed,
                    )
                })
                .or_else(|| {
                    owned_replacement.as_ref().and_then(|continuation| {
                        continuation.start_effect(
                            crate::session::selection::LifecycleEffectSlot::ConfigSeed,
                        )
                    })
                });
            if (inline_transaction.is_some_and(|transaction| transaction.has_lifecycle_entry())
                || owned_replacement.is_some())
                && seed_effect.is_none()
            {
                release_resource_launch_permit(&resource_monitor, &mut resource_permit);
                return Ok(CreateCompletion::Retained);
            }
            let lock = app.state::<crate::ConfigSeedLockState>().inner().clone();
            #[cfg(test)]
            let seed_fixture = owned_replacement
                .as_ref()
                .and_then(|owned| owned.fixture_native(true));
            #[cfg(test)]
            let native_await_panic = owned_replacement
                .as_ref()
                .and_then(|owned| owned.fixture_take_native_await_panic(true));
            let seed_native = coordinator.run_blocking_seed_work(move || {
                #[cfg(test)]
                if let Some(control) = seed_fixture.as_ref() {
                    control.enter();
                }
                let _seed_guard = lock.blocking_lock_owned();
                // #1065 Stage F: production records the config-seed publication
                // under the per-project gate, acquired AFTER this global lock
                // (plan section 6.3 lock order) and dropped before returning; a
                // `#[cfg(test)]` lib build stays non-emitting.
                #[cfg(not(test))]
                let activation =
                    Some(crate::config::seed_manifest::ManifestActivationToken::production());
                #[cfg(test)]
                let activation: Option<
                    crate::config::seed_manifest::ManifestActivationToken,
                > = None;
                let report = crate::config::config_seed::perform_config_seed_recorded(
                    &seed,
                    &id.to_string(),
                    activation.as_ref(),
                );
                // The blocking worker records its actual report even when
                // the async receiver disappears. Native failure remains the
                // existing best-effort report, not a new spawn error.
                if let Some(effect) = seed_effect {
                    effect.complete(Ok(Some(format!("{report:?}"))));
                }
                drop(_seed_guard);
                #[cfg(test)]
                if let Some(control) = seed_fixture.as_ref() {
                    control.returned(format!("{report:?}"));
                }
                report
            });
            #[cfg(test)]
            let seed_result =
                crate::session::selection::await_fixture_native(seed_native, native_await_panic)
                    .await;
            #[cfg(not(test))]
            let seed_result = seed_native.await;
            match seed_result {
                Ok(report) => {
                    log::debug!("[config-seed] session {} outcome: {:?}", id, report);
                }
                Err(error) => {
                    let error = format!("config seed blocking preparation failed: {error}");
                    release_resource_launch_permit(&resource_monitor, &mut resource_permit);
                    drop(mgr);
                    rollback_pre_created_session(app, session_mgr, pty_mgr, id, &error).await;
                    return Err(error);
                }
            }
        }

        // #1063 test-only barrier: the seed transaction (if any) has acquired and
        // released the project gate, but the create is still pending and unfinalized,
        // so a deletion's pending-inclusive snapshot must still observe it. No-op in
        // production.
        #[cfg(test)]
        {
            seed_race_barriers::hit_after_seed_before_pty(&cwd).await;
        }

        let viewport = viewport.unwrap_or(PtyViewport::DEFAULT);
        // #2124 - the idle tuning this launch hands the detector: the agent kind
        // plus the EFFECTIVE catalog entry matched to the configured command.
        // One settings snapshot, released before the blocking filesystem read;
        // no lock is held across that I/O.
        let launch_idle_tuning = {
            let settings_state = app.state::<SettingsState>();
            let snapshot = match settings_state.current_snapshot().await {
                Ok(snapshot) => snapshot,
                Err(error) => {
                    let error = error.to_string();
                    release_resource_launch_permit(&resource_monitor, &mut resource_permit);
                    drop(mgr);
                    rollback_pre_created_session(app, session_mgr, pty_mgr, id, &error).await;
                    return Err(error);
                }
            };
            let requested_agent_id = agent_id.clone();
            match tokio::task::spawn_blocking(move || {
                let catalog =
                    crate::config::coding_agents_catalog::load_catalog_for_settings(&snapshot)
                        .unwrap_or_default();
                resolve_launch_idle_tuning(
                    &snapshot,
                    requested_agent_id.as_deref(),
                    agent_kind,
                    &catalog,
                )
            })
            .await
            {
                Ok(tuning) => tuning,
                Err(error) => {
                    log::warn!("[idle] launch idle tuning resolution failed: {error}");
                    idle_tuning_for(agent_kind)
                }
            }
        };
        let launch_witness = LaunchWitness::default();
        let spawn_spec = BackendSpawnSpec {
            id,
            agent_id: agent_id.clone(),
            // #942 - the CLI identity from the canonical detector (`agent_kind`, resolved
            // at the top of this function). The profile id above cannot stand in for it:
            // it is opaque, several profiles can drive the same CLI, and a coding-agent
            // launch can carry no profile id at all.
            coding_agent: agent_kind,
            cmd: shell.clone(),
            args: shell_args.clone(),
            // #1271 - the configured host shell paired with a resolved agent
            // command, carried as one immutable snapshot (never as loose
            // program/argument fields, so the pairing invariant cannot drift).
            resolved_agent_host_shell: resolved_agent_host_shell.clone(),
            cwd: spawn_cwd.clone(),
            selected_cwd: if spawn_cwd != cwd {
                Some(cwd.clone())
            } else {
                None
            },
            // #973 - open the PTY at the size the terminal is actually going to be, so the
            // frontend never has to resize a child that is still starting up. Callers with no
            // view (restore loop, delivery loop, mailbox, CLI, tests) get PtyViewport::DEFAULT,
            // which is the 120x30 AC always used.
            cols: viewport.cols,
            rows: viewport.rows,
            container_image: resolved_spawn
                .as_ref()
                .and_then(|spawn| spawn.backend.image.clone()),
            configured_env,
            env_remove_keys,
            env_unset,
            extra_env,
            idle_tuning: launch_idle_tuning,
            output_target: PtyOutputTarget::from_app_handle(app.clone()),
            resource_registration,
            logical_resource_slot,
            container_credential,
            container_repo_mounts: container_repos
                .as_ref()
                .map(|resolution| {
                    resolution
                        .mounts()
                        .map(
                            |(host, container)| crate::pty::container_repos::ContainerRepoMount {
                                host_path: host.to_path_buf(),
                                container_path: container.to_string(),
                            },
                        )
                        .collect()
                })
                .unwrap_or_default(),
            launch_witness: launch_witness.clone(),
        };
        // (#2413) Spawn fence: set BEFORE the first await of spawn, so a drop at
        // any point inside or after spawn keeps the close markers cleared.
        if let Some(t) = marker_ticket
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_mut()
        {
            t.session_may_exist = true;
        }
        if let Some(continuation) = owned_replacement.as_ref() {
            if !continuation.validate_source().await {
                release_resource_launch_permit(&resource_monitor, &mut resource_permit);
                return Ok(CreateCompletion::Retained);
            }
        }
        if owned_replacement.is_some() {
            revalidate_owned_create_effect(
                &create_owner,
                &mut resource_permit,
                &resource_monitor,
                &mgr,
                pty_mgr,
            )
            .await?;
        }
        let backend_effect = inline_transaction
            .and_then(|transaction| {
                transaction.start_lifecycle_effect(
                    crate::session::selection::LifecycleEffectSlot::BackendSpawn,
                )
            })
            .or_else(|| {
                owned_replacement.as_ref().and_then(|continuation| {
                    continuation
                        .start_effect(crate::session::selection::LifecycleEffectSlot::BackendSpawn)
                })
            });
        if (inline_transaction.is_some_and(|transaction| transaction.has_lifecycle_entry())
            || owned_replacement.is_some())
            && backend_effect.is_none()
        {
            release_resource_launch_permit(&resource_monitor, &mut resource_permit);
            return Ok(CreateCompletion::Retained);
        }
        #[cfg(test)]
        if let Some(owned) = owned_replacement.as_ref() {
            if !owned
                .fixture_reached(
                    crate::session::selection::ReplacementFixtureCut::BeforeBackendSpawn,
                )
                .await
            {
                return Ok(CreateCompletion::Retained);
            }
        }
        let spawn_result = PtyManager::spawn(pty_mgr, session.backend_kind, spawn_spec).await;
        if let Some(effect) = backend_effect {
            effect.complete(
                spawn_result
                    .as_ref()
                    .map(|_| Some(id.to_string()))
                    .map_err(|error| error.to_string()),
            );
        }
        drop(spawn_mark);
        if let Err(e) = spawn_result {
            if !launch_witness.launched() {
                if let Some(t) = marker_ticket
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .as_mut()
                {
                    t.session_may_exist = false;
                }
            } else {
                log::warn!(
                    "[coordinator-create] spawn failed after process launch; close markers stay \
                     cleared session={}",
                    id
                );
            }
            let err = e.to_string();
            drop(mgr);
            rollback_pre_created_session(app, session_mgr, pty_mgr, id, &err).await;
            return Err(err);
        }

        if let Some(continuation) = owned_replacement.as_ref() {
            let actual = mgr.get_pending_session(pending_binding).await;
            let generation = pty_mgr
                .lock()
                .ok()
                .and_then(|pty| pty.route_identities(id).map(|route| route.2));
            if !actual
                .zip(generation)
                .is_some_and(|(row, generation)| continuation.record_launch(&row, generation))
            {
                return Ok(CreateCompletion::Retained);
            }
        }
        register_session_samplers(app, id, agent_id.clone());

        // Auto-inject optional non-credential bootstrap text for agent sessions
        // after PTY spawn. Credentials are already present in child environment
        // variables; no credentials are written through PTY.
        //
        // Currently the only bootstrap payload is the Coordinator auto-title prompt.
        if agent_id.is_some() && !is_coordinator {
            log::debug!(
                "[session] No bootstrap injection for non-coordinator agent session {}",
                id
            );
        }
        if agent_id.is_some() && is_coordinator {
            let auto_title_enabled = false; /*
                                                let settings_state = app.state::<SettingsState>();
                                                let cfg = settings_state.read().await;
                                                cfg.auto_generate_task_title
                                            };*/

            if auto_title_enabled {
                let app_clone = app.clone();
                let session_id = id;
                let cwd_clone = cwd.clone();

                tokio::spawn(async move {
                    let prompt = match build_title_prompt_appendage(&cwd_clone) {
                        Ok(Some(prompt)) => {
                            log::info!(
                                "[session] Auto-title prompt built for session {}",
                                session_id
                            );
                            prompt
                        }
                        Ok(None) => {
                            log::info!(
                            "[session] Auto-title prompt skipped (gate not passed) for session {}",
                            session_id
                        );
                            return;
                        }
                        Err(e) => {
                            log::warn!(
                                "[session] Auto-title prompt skipped for session {}: {}",
                                session_id,
                                e
                            );
                            return;
                        }
                    };

                    let max_wait = std::time::Duration::from_secs(30);
                    let poll = std::time::Duration::from_millis(500);
                    let start = std::time::Instant::now();

                    loop {
                        if start.elapsed() >= max_wait {
                            log::warn!(
                            "[session] Timeout waiting for idle before auto-title prompt injection \
                             for session {}",
                            session_id
                        );
                            break;
                        }
                        tokio::time::sleep(poll).await;

                        let session_mgr =
                            app_clone.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
                        let mgr = session_mgr.read().await;
                        let sessions = mgr.list_sessions().await;
                        match sessions.iter().find(|s| s.id == session_id.to_string()) {
                            Some(s) if s.waiting_for_input => break,
                            Some(_) => {}
                            None => {
                                log::warn!(
                                    "[session] Session {} gone before auto-title prompt injection",
                                    session_id
                                );
                                return;
                            }
                        }
                    }

                    match crate::pty::inject::inject_text_into_session(
                        &app_clone, session_id, &prompt,
                    )
                    .await
                    {
                        Ok(()) => {
                            log::info!(
                                "[session] Auto-title prompt injected for session {}",
                                session_id
                            );
                        }
                        Err(e) => {
                            log::warn!(
                                "[session] Failed to inject auto-title prompt for {}: {}",
                                session_id,
                                e
                            );
                        }
                    }
                });
            }
        }

        let finalization_warnings = session_env_warnings
            .iter()
            .map(|warning| {
                SessionWarning::new(
                    id,
                    warning.key.clone(),
                    warning.kind,
                    warning.message.clone(),
                )
            })
            .collect::<Vec<_>>();

        // 0.8.0: removed the "Show the terminal window when a session is created" branch.
        // Under the unified-window model the main window is created up-front and stays
        // visible; session creation has no window-show responsibility.

        // Save lastCodingAgent + codingAgents (skip for temp sessions)
        if !skip_tooling_save {
            if let Some(ref aid) = agent_id {
                // Resolve label: use provided agent_label, or look up from settings by agent_id.
                // Without this fallback, callers that pass agent_id but no label (session-requests,
                // web remote) would write app: "Unknown" into the per-instance config.json.
                // #2433 - computed under the settings read lock, written after it drops.
                let write = {
                    let settings = app.state::<SettingsState>();
                    let write = match settings.current_read().await {
                        Ok(cfg) => Some(prepare_last_coding_agent_write(
                            aid,
                            agent_label.as_deref(),
                            &cfg,
                        )),
                        Err(error) => {
                            log::debug!("[session] skipping lastCodingAgent publication: {error}");
                            None
                        }
                    };
                    write
                };
                if let Some(write) = write {
                    if let Err(e) = save_last_coding_agent(&cwd, &id.to_string(), &write) {
                        log::warn!("Failed to save lastCodingAgent: {}", e);
                    }
                }
                // #592 - persist the loaded profile content-hash so drift survives an
                // AC restart. Best-effort; never aborts the spawn. No-op off-replica.
                if let Some(hash) = session.profile_content_hash.as_deref() {
                    log::debug!(
                        "[profile-hash] spawn-persist: session={} agent={} hash={} cwd={:?}",
                        id,
                        aid,
                        hash,
                        cwd,
                    );
                    if let Err(e) =
                        crate::config::coding_agent_profiles::set_replica_profile_content_hash(
                            std::path::Path::new(&cwd),
                            hash,
                        )
                    {
                        log::warn!("Failed to persist profileContentHash: {}", e);
                    }
                }
            }
        }

        // A configured selector is never evidence of AC-attributed continuation,
        // even when an injector also happened to add a control of its own.
        effective_continuation &= !configured_identity;
        let prompt_fresh = !configured_identity
            && (!effective_continuation
                || skip_auto_resume
                || pending_start_fresh == Some(true)
                || fresh_forced_by_mirror);
        let key = if session.backend_kind == crate::pty::backend::SessionBackendKind::LocalProcess {
            crate::config::sessions_persistence::prompt_key(
                &cwd,
                session.agent_id.as_deref(),
                session.requested_profile.as_deref(),
                session.effective_profile.as_deref(),
            )
        } else {
            None
        };
        let initial = initial_last_prompt
            .as_ref()
            .filter(|source| source.compatible(&session))
            .and_then(|source| source.text.clone());
        let revalidated = crate::config::sessions_persistence::prompt_key(
            &cwd,
            session.agent_id.as_deref(),
            session.requested_profile.as_deref(),
            session.effective_profile.as_deref(),
        );
        let key = key.filter(|candidate| Some(candidate) == revalidated.as_ref());
        let carrier_prompt = match with_create_resource_region(
            &create_owner,
            &mut resource_permit,
            acquire_lifecycle_create_publication_region(
                &create_owner,
                &mgr,
                lifecycle_domain,
                inline_transaction,
            ),
        )
        .await
        {
            Ok(permit) => permit,
            Err(error) => {
                release_resource_launch_permit(&resource_monitor, &mut resource_permit);
                return Err(error);
            }
        };
        mgr.initialize_pending_prompt_with_carrier_permit(
            &carrier_prompt,
            pending_binding,
            key,
            initial,
            effective_continuation,
            prompt_fresh,
            !defer_inline_finalization,
        )
        .await?;
        drop(carrier_prompt);

        if defer_inline_finalization {
            if create_ticket
                .lock()
                .map_err(|_| "create ticket unavailable".to_string())?
                .is_some()
                || inline_transaction.is_none()
            {
                return Err(
                    "deferred create finalization requires an inline transaction".to_string(),
                );
            }
            let info = mgr
                .get_pending_session(pending_binding)
                .await
                .map(|record| SessionInfo::from(&record))
                .ok_or_else(|| "deferred create pending record disappeared".to_string())?;
            inline_pending_for_body
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .take();
            return Ok(CreateCompletion::Deferred(DeferredCreateOutput {
                info,
                binding: pending_binding,
                warnings: finalization_warnings,
                _create_admission: create_owner.clone(),
                effective_continuation,
            }));
        }

        let finalization_ticket = create_ticket
            .lock()
            .map_err(|_| "create ticket unavailable".to_string())?
            .take();
        let finalized = if let Some(ticket) = finalization_ticket {
            ticket
                .finalize_with_owned_outcome(finalization_warnings)
                .await
        } else {
            let transaction = inline_transaction
                .ok_or_else(|| "inline create transaction is unavailable".to_string())?;
            let cause =
                inline_cause.ok_or_else(|| "inline create cause is unavailable".to_string())?;
            transaction
                .finalize_inline_create(
                    pending_binding,
                    cause,
                    finalization_warnings,
                    owned_replacement.clone(),
                )
                .await
        };
        let Some(finalized) = finalized else {
            return Ok(CreateCompletion::Retained);
        };
        if finalized.is_ok() && inline_transaction.is_some() {
            inline_pending_for_body
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .take();
        }
        let completion = finalized.map(CreateCompletion::Finalized);
        // #2232 phase 4 section 5.1: the Room demand may be raised only once
        // the row is **finalized and visible** to `get_session` — a
        // pending-create row is hidden, so a raise spawned earlier answers
        // "session not found" and is never retried. Detached, as before: the
        // raise resolves the reader kind and spawns the watcher under its own
        // locks, off the create path. A restart replacement is finalized later
        // by `execute_restart_transaction`, which raises its Room demand
        // synchronously after the commit (section 5.2); `SelectionCause::Restart(_)`
        // never reaches a finalize here, and the gate keeps that explicit.
        if completion.is_ok() && !matches!(inline_cause, Some(SelectionCause::Restart(_))) {
            let app_for_demand = app.clone();
            tauri::async_runtime::spawn(async move {
                raise_room_reader_demand(&app_for_demand, id).await;
            });
            if let Some(transaction) = inline_transaction {
                transaction.observe_entry_reader_demand(
                    pending_binding,
                    crate::session::selection::EntryReaderDemandReceipt::LegacyTaskAccepted,
                );
            }
        }
        completion
    };
    // This accepted core owns every launched effect and its real result.
    // Coordinator shutdown may reject finalization, but cannot drop a pending
    // backend/native effect and manufacture completion for the caller.
    let result = create_body.await;
    let marker = marker_ticket
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take();
    finish_create_core_result(
        marker,
        result,
        owned_replacement.as_ref(),
        &create_owner,
        session_mgr,
        pty_mgr,
        preheld_target.is_some(),
        inline_pending_binding.as_ref(),
        inline_transaction,
    )
    .await
}

fn resolve_launch_idle_tuning(
    settings: &AppSettings,
    agent_id: Option<&str>,
    agent_kind: Option<CodingAgentKind>,
    catalog: &[CodingAgentDefinition],
) -> IdleTuning {
    let Some(command) = agent_id
        .and_then(|id| settings.agents.iter().find(|agent| agent.id == id))
        .map(|agent| agent.command.as_str())
    else {
        return idle_tuning_for_launch(agent_kind, None);
    };
    let matched = catalog
        .iter()
        .find(|definition| definition.command == command)
        .or_else(|| {
            let basename = command_executable_basename(command);
            basename.as_deref().and_then(|basename| {
                catalog.iter().find(|definition| {
                    command_executable_basename(&definition.command).as_deref() == Some(basename)
                })
            })
        });
    idle_tuning_for_launch(
        agent_kind,
        matched
            .and_then(|definition| definition.idle_burst.as_ref())
            .map(|burst| (burst.max_bytes, burst.max_secs, burst.prior_silence_secs)),
    )
}

fn resolve_launch_auto_self_clear(
    settings: &AppSettings,
    shell: &str,
    cwd: &str,
    is_coordinator: bool,
) -> bool {
    if !crate::pty::inject::supports_auto_self_maintenance(shell) {
        return false;
    }
    let class_default_on = is_coordinator || crate::config::root_agent::is_root_agent_dir_name(cwd);
    let agent_name =
        crate::config::coding_agent_profiles::agent_name_from_dir(std::path::Path::new(cwd))
            .unwrap_or_default();
    crate::config::settings::resolve_auto_self_clear(settings, &agent_name, class_default_on)
}

pub(crate) fn resolve_configured_agent_spawn_for_cwd(
    settings: &AppSettings,
    agent_id: &str,
    cwd: &str,
    requested_profile: Option<&str>,
    requested_profile_authoritative: bool,
) -> Result<Option<AgentSpawnCommand>, String> {
    // #2716 (B3, option C): the agents layer could not be read, so the list is not
    // the user's; refuse rather than fall through to a plain terminal.
    if let Some(path) = settings.agents_layer.unreadable_path() {
        return Err(crate::config::settings::agents_unreadable_session_error(
            path,
        ));
    }
    if !settings.agents.iter().any(|agent| agent.id == agent_id) {
        return Ok(None);
    }
    let cwd = crate::path_utils::normalize_windows_verbatim_path(cwd);
    crate::config::agent_command::resolve_agent_spawn_command(
        settings,
        agent_id,
        Some(std::path::Path::new(&cwd)),
        requested_profile,
        requested_profile_authoritative,
    )
    .map(Some)
}

pub(crate) fn build_configured_agent_spawn_for_cwd(
    settings: &AppSettings,
    agent_id: &str,
    cwd: &str,
    requested_profile: Option<&str>,
) -> Result<Option<AgentSpawnCommand>, String> {
    // #2716 (B3, option C): the same refusal as the resolver above; each builder
    // carries its own, so no entry point can reach a spawn past either one.
    if let Some(path) = settings.agents_layer.unreadable_path() {
        return Err(crate::config::settings::agents_unreadable_session_error(
            path,
        ));
    }
    if !settings.agents.iter().any(|agent| agent.id == agent_id) {
        return Ok(None);
    }
    let cwd = crate::path_utils::normalize_windows_verbatim_path(cwd);
    crate::config::agent_command::build_agent_spawn_command(
        settings,
        agent_id,
        Some(std::path::Path::new(&cwd)),
        requested_profile,
    )
    .map(Some)
}

/// #592 - true when a session's loaded profile cell no longer matches what a
/// Reload would load right now. Mirrors the restart-time resolution so the
/// "configured" side equals `restart_session`'s effective cell. Hashes RAW
/// cells (cell-only); swallows all "cannot determine" cases to false (never
/// false-alarms). Plain-shell sessions (no agent) never drift.
fn compute_profile_outdated(settings: &AppSettings, info: &SessionInfo) -> bool {
    if info.agent_id.is_none() {
        return false;
    }
    let cwd = info.working_directory.as_str();
    // #2434 - the SAME selection a Reload would launch: the matched agent, its
    // letter and its authority. `NoAgent` and `Unresolved` raise no badge (a
    // badge whose click restarts a session that cannot start is a trap).
    let requested = effective_restart_requested_profile(None, info.requested_profile.clone());
    let MatchedSelection::Agent {
        agent_id,
        profile_letter,
        authoritative,
        ..
    } = matched_selection(
        settings,
        cwd,
        None,
        info.agent_id.as_deref(),
        requested.as_deref(),
    )
    else {
        return false;
    };
    let resolution = crate::config::coding_agent_profiles::resolve_profile(
        settings,
        crate::config::coding_agent_profiles::ProfileResolutionRequest {
            coding_agent_id: &agent_id,
            launch_path: Some(std::path::Path::new(cwd)),
            agent_matrix_name: None,
            requested_profile: profile_letter.as_deref(),
            requested_profile_authoritative: authoritative,
        },
    );
    // If the agent vanished from settings, do not false-flag.
    let Some(agent) = settings.agents.iter().find(|a| a.id == agent_id) else {
        return false;
    };
    // #2431 - the one definition of a cell digest, as the spawn stamps it.
    let configured = crate::config::agent_command::cell_identity(agent, &resolution.cell);
    // Loaded hash: in-memory stamp, else the persisted replica copy (survives an
    // AC restart that cleared the in-memory stamp).
    let loaded = info.profile_content_hash.clone().or_else(|| {
        crate::config::coding_agent_profiles::read_replica_profile_content_hash(
            std::path::Path::new(cwd),
        )
    });
    match loaded {
        Some(loaded) => loaded != configured,
        None => false,
    }
}

/// §1295 S5 — resolve the create-command working directory. The bare
/// `invoke("create_session", {})` default (cwd: None) falls back to the home
/// dir. In production the creation gate then REJECTS that default when home is
/// not a registered project root (an intentional, documented UX change, §5.8);
/// every shipped frontend call site passes an explicit registered cwd.
pub(crate) fn resolve_create_session_cwd(cwd: Option<String>) -> String {
    let cwd = cwd.unwrap_or_else(|| {
        dirs::home_dir()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| "C:\\".to_string())
    });
    crate::path_utils::normalize_windows_verbatim_path(&cwd)
}

/// Create a new session. Optionally override shell/args/cwd/name (for action buttons).
/// Falls back to settings defaults when not provided.
// Tauri command: State<> injections push us over clippy's 7-arg threshold.
#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub async fn create_session(
    app: AppHandle,
    session_mgr: State<'_, Arc<tokio::sync::RwLock<SessionManager>>>,
    pty_mgr: State<'_, Arc<Mutex<PtyManager>>>,
    _tg_mgr: State<'_, TelegramBridgeState>,
    settings: State<'_, SettingsState>,
    shell: Option<String>,
    shell_args: Option<Vec<String>>,
    cwd: Option<String>,
    session_name: Option<String>,
    agent_id: Option<String>,
    requested_profile: Option<String>,
    git_repos: Option<Vec<SessionRepo>>,
    skip_auto_resume: Option<bool>,
    // #973 - the terminal size the view has already fitted to. Optional: an older frontend,
    // or any caller that has no tile to measure, simply omits it and gets AC's 120x30.
    cols: Option<u16>,
    rows: Option<u16>,
) -> Result<SessionInfo, String> {
    let cfg = settings
        .current_read()
        .await
        .map_err(|error| error.to_string())?;

    let cwd = resolve_create_session_cwd(cwd);

    let resolved_spawn = if let Some(aid) = agent_id.as_deref() {
        build_configured_agent_spawn_for_cwd(&cfg, aid, &cwd, requested_profile.as_deref())?
    } else {
        None
    };

    let (shell, shell_args, agent_label, resolved_agent_host_shell) =
        if let Some(spawn) = resolved_spawn.as_ref() {
            // #1271 - keep the resolved agent executable and its logical argv as
            // the command-to-run, and carry a copy of the configured default host
            // shell (program + args from the SAME immutable config snapshot, before
            // `drop(cfg)`) separately to the backend. The backend launches a
            // non-direct Windows agent command through this host shell instead of
            // the unconditional `cmd.exe /C` fallback.
            (
                spawn.shell.clone(),
                spawn.shell_args.clone(),
                Some(spawn.trusted_agent_label.clone()),
                Some(ResolvedAgentHostShell {
                    program: cfg.default_shell.clone(),
                    args: cfg.default_shell_args.clone(),
                }),
            )
        } else {
            let s = shell.unwrap_or_else(|| cfg.default_shell.clone());
            let sa = shell_args.unwrap_or_else(|| cfg.default_shell_args.clone());
            let al = agent_id.as_ref().and_then(|aid| {
                cfg.agents
                    .iter()
                    .find(|a| a.id == *aid)
                    .map(|a| a.label.clone())
            });
            (s, sa, al, None)
        };

    log::info!(
        "[session] FINAL resolved: shell={:?}, args={:?}, label={:?}",
        shell,
        shell_args,
        agent_label
    );

    drop(cfg);

    let info = create_session_inner(
        &app,
        session_mgr.inner(),
        pty_mgr.inner(),
        shell,
        shell_args,
        cwd.clone(),
        session_name,
        agent_id,
        agent_label,
        false, // persist tooling
        git_repos.unwrap_or_default(),
        // #599 R1: default true (fresh create); reopen of a closed coordinator
        // passes Some(false) so the prior conversation resumes.
        effective_create_skip_auto_resume(skip_auto_resume),
        resolved_spawn,
        resolved_agent_host_shell,
        // #973 - the only caller that has a terminal to measure.
        match (cols, rows) {
            (Some(c), Some(r)) => Some(PtyViewport::from_fit(c, r)),
            _ => None,
        },
        CreateSelectionIntent::User,
    )
    .await?;

    // Auto-attach Telegram bot if repo has .agentscommander/config.json
    let id = Uuid::parse_str(&info.id).unwrap();
    attach_local_config_telegram_if_any(&app, id, &cwd).await;

    Ok(info)
}

pub(crate) async fn attach_persisted_telegram_if_configured<R: tauri::Runtime>(
    app: &AppHandle<R>,
    session_id: Uuid,
    bot_id: Option<&str>,
) {
    let Some(bot_id) = bot_id else {
        return;
    };

    let settings = app.state::<SettingsState>();
    let exists = {
        let cfg = match settings.current_read().await {
            Ok(cfg) => cfg,
            Err(error) => {
                log::debug!("[telegram] skipping association: {error}");
                return;
            }
        };
        cfg.telegram_bots.iter().any(|b| b.id == bot_id)
    };

    if !exists {
        log::warn!(
            "[telegram] Persisted bot '{}' for session {} no longer exists in settings; leaving \
             bridge OFF",
            bot_id,
            session_id
        );
        let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
        let mgr = session_mgr.read().await;
        mgr.set_telegram_bot_id(session_id, None).await;
        persist_current_state(&mgr).await;
        return;
    }

    let already_attached = {
        let tg_mgr = app.state::<TelegramBridgeState>();
        let tg = tg_mgr.lock().await;
        tg.get_bridge(session_id)
    };
    if let Some(info) = already_attached {
        let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
        let mgr = session_mgr.read().await;
        if info.bot_id == bot_id {
            mgr.set_telegram_bot_id(session_id, Some(bot_id.to_string()))
                .await;
        } else {
            log::warn!(
                "[telegram] Session {} already has Telegram bot '{}' attached; persisted bot '{}' \
                 will be cleared",
                session_id,
                info.bot_id,
                bot_id
            );
            mgr.set_telegram_bot_id(session_id, None).await;
        }
        persist_current_state(&mgr).await;
        return;
    }

    if let Err(e) =
        crate::commands::telegram::attach_telegram_bot_by_id(app, session_id, bot_id).await
    {
        log::warn!(
            "[telegram] Failed to restore persisted bot '{}' for session {}: {}",
            bot_id,
            session_id,
            e
        );
        // Availability failure during attach must not erase the persisted intent.
        if let Err(error) = settings.current_read().await {
            log::debug!("[telegram] preserving association intent: {error}");
            return;
        }
        let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
        let mgr = session_mgr.read().await;
        mgr.set_telegram_bot_id(session_id, None).await;
        persist_current_state(&mgr).await;
    }
}

pub(crate) async fn preserve_deferred_telegram_intent_if_valid(
    mgr: &SessionManager,
    settings: &SettingsState,
    session_id: Uuid,
    session_name: &str,
    bot_id: Option<&str>,
) {
    let Some(bot_id) = bot_id else {
        return;
    };

    let exists = {
        let cfg = match settings.current_read().await {
            Ok(cfg) => cfg,
            Err(error) => {
                log::debug!("[telegram] skipping association: {error}");
                return;
            }
        };
        cfg.telegram_bots.iter().any(|b| b.id == bot_id)
    };

    if exists {
        mgr.set_telegram_bot_id(session_id, Some(bot_id.to_string()))
            .await;
    } else {
        log::warn!(
            "[telegram] Persisted bot '{}' for deferred session '{}' no longer exists in settings; \
             leaving bridge OFF",
            bot_id,
            session_name
        );
        mgr.set_telegram_bot_id(session_id, None).await;
    }
}

pub(crate) async fn attach_local_config_telegram_if_any<R: tauri::Runtime>(
    app: &AppHandle<R>,
    session_id: Uuid,
    cwd: &str,
) {
    let config_dir = std::path::Path::new(cwd).join(crate::config::agent_local_dir_name());

    // #2786 (C1) - through the shared loader, off the async worker: the
    // loader does blocking file reads.
    let Some(bot_label) =
        tokio::task::spawn_blocking(move || agent_config::read_agent_local_config(&config_dir))
            .await
            .ok()
            .flatten()
            .and_then(|local_config| local_config.tooling.telegram_bot)
    else {
        return;
    };

    let settings = app.state::<SettingsState>();
    let bot_id = {
        let cfg = match settings.current_read().await {
            Ok(cfg) => cfg,
            Err(error) => {
                log::debug!("[telegram] skipping association: {error}");
                return;
            }
        };
        cfg.telegram_bots
            .iter()
            .find(|b| b.label == bot_label)
            .map(|b| b.id.clone())
    };

    match bot_id {
        Some(bot_id) => {
            if let Err(e) =
                crate::commands::telegram::attach_telegram_bot_by_id(app, session_id, &bot_id).await
            {
                log::warn!(
                    "[telegram] Failed to auto-attach configured bot '{}' for session {}: {}",
                    bot_label,
                    session_id,
                    e
                );
            }
        }
        None => {
            log::warn!(
                "[telegram] Configured bot label '{}' for session {} no longer exists in settings; \
                 leaving bridge OFF",
                bot_label,
                session_id
            );
        }
    }
}

/// Core session destruction logic shared by the Tauri command and the MailboxPoller.
/// Kills PTY, detaches Telegram bridge, removes from SessionManager, persists, and emits events.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DestructionSource {
    ManualClose,
    AutoClose,
    SpawnRollback,
    BackgroundCleanup,
}

impl DestructionSource {
    fn cause(self) -> SelectionCause {
        match self {
            Self::ManualClose => SelectionCause::ManualClose,
            Self::AutoClose => SelectionCause::AutoClose,
            Self::SpawnRollback => SelectionCause::SpawnRollback,
            Self::BackgroundCleanup => SelectionCause::BackgroundCleanup,
        }
    }

    fn selection_source(self) -> SelectionSource {
        match self {
            Self::ManualClose => SelectionSource::ManualClose,
            Self::AutoClose => SelectionSource::AutoClose,
            Self::SpawnRollback => SelectionSource::SpawnRollback,
            Self::BackgroundCleanup => SelectionSource::BackgroundCleanup,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct DestroyRequest {
    pub ids: Vec<Uuid>,
    pub source: DestructionSource,
    pub force_destroy_root: bool,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct DestroyOutcome {
    pub destroyed_ids: Vec<Uuid>,
    pub retained_exited_ids: Vec<Uuid>,
    pub failed: Vec<(Uuid, String)>,
}

impl DestroyOutcome {
    pub fn succeeded(&self, id: Uuid) -> bool {
        self.destroyed_ids.contains(&id) || self.retained_exited_ids.contains(&id)
    }

    fn into_single_result(self, id: Uuid) -> Result<(), String> {
        if self.succeeded(id) {
            return Ok(());
        }
        let message = self
            .failed
            .into_iter()
            .find(|(failed_id, _)| *failed_id == id)
            .map(|(_, message)| message)
            .unwrap_or_else(|| "Session not found".to_string());
        Err(message)
    }
}

pub async fn destroy_session_inner<R: tauri::Runtime>(
    app: &AppHandle<R>,
    uuid: Uuid,
) -> Result<(), String> {
    destroy_sessions_with_source(app, vec![uuid], DestructionSource::ManualClose, false)
        .await?
        .into_single_result(uuid)
}

pub(crate) async fn background_destroy_session_inner<R: tauri::Runtime>(
    app: &AppHandle<R>,
    uuid: Uuid,
) -> Result<(), String> {
    let coordinator = app
        .try_state::<SelectionCoordinator>()
        .ok_or_else(|| "selectionCoordinatorUnavailable".to_string())?;
    match coordinator.background_destroy(uuid).await? {
        CriticalAdmissionOutcome::Completed(outcome) => outcome.into_single_result(uuid),
        CriticalAdmissionOutcome::AlreadyPending => Ok(()),
    }
}

pub(crate) async fn destroy_sessions_with_source<R: tauri::Runtime>(
    app: &AppHandle<R>,
    ids: Vec<Uuid>,
    source: DestructionSource,
    force_destroy_root: bool,
) -> Result<DestroyOutcome, String> {
    let coordinator = app
        .try_state::<SelectionCoordinator>()
        .ok_or_else(|| "selectionCoordinatorUnavailable".to_string())?;
    coordinator
        .destroy(DestroyRequest {
            ids,
            source,
            force_destroy_root,
        })
        .await
}

#[derive(Clone)]
enum ResponseEffectState<T> {
    NotStarted,
    Running,
    Completed(Result<T, String>),
    Ambiguous,
}

struct ResponseEffectRecord {
    stages: Vec<(&'static str, ResponseEffectState<()>)>,
    group: Option<ResponseEffectState<crate::resource_monitor::ResourceKillResult>>,
    group_worker: Option<
        tokio::task::JoinHandle<Result<crate::resource_monitor::ResourceKillResult, String>>,
    >,
    sealed: bool,
}

struct ResponseEffectOwner {
    ticket: crate::pty::manager::ResponseCloseTicket,
    registration: crate::session::selection::ResponseCloseRegistration,
    backend: Arc<dyn crate::pty::backend::PtyBackend>,
    record: Mutex<ResponseEffectRecord>,
}

fn response_effect_owners(
) -> &'static Mutex<std::collections::HashMap<Uuid, Arc<ResponseEffectOwner>>> {
    static OWNERS: std::sync::OnceLock<
        Mutex<std::collections::HashMap<Uuid, Arc<ResponseEffectOwner>>>,
    > = std::sync::OnceLock::new();
    OWNERS.get_or_init(Default::default)
}

impl ResponseEffectOwner {
    fn start(&self, name: &'static str) -> Option<usize> {
        let mut record = self.record.lock().ok()?;
        if record.sealed || record.stages.len() == record.stages.capacity() {
            return None;
        }
        let index = record.stages.len();
        record.stages.push((name, ResponseEffectState::NotStarted));
        record.stages[index].1 = ResponseEffectState::Running;
        Some(index)
    }

    fn complete(&self, index: usize, outcome: Result<(), String>) -> Option<()> {
        let mut record = self.record.lock().ok()?;
        let (_, stage) = record.stages.get_mut(index)?;
        if !matches!(stage, ResponseEffectState::Running) {
            return None;
        }
        *stage = ResponseEffectState::Completed(outcome);
        Some(())
    }

    fn terminal(&self) -> bool {
        let Ok(record) = self.record.lock() else {
            return false;
        };
        record.sealed
            && record
                .stages
                .iter()
                .all(|(_, stage)| matches!(stage, ResponseEffectState::Completed(_)))
            && record
                .group
                .as_ref()
                .is_none_or(|stage| matches!(stage, ResponseEffectState::Completed(_)))
    }

    fn kill_failed(&self) -> bool {
        self.record.lock().ok().is_some_and(|record| {
            record.stages.iter().any(|(name, stage)| {
                *name == "kill" && matches!(stage, ResponseEffectState::Completed(Err(_)))
            })
        })
    }

    fn seal(&self) -> Option<()> {
        self.record.lock().ok()?.sealed = true;
        Some(())
    }
}

async fn response_group_effect(
    owner: &Arc<ResponseEffectOwner>,
    monitor: &Arc<ResourceMonitorState>,
) -> Option<()> {
    let id = owner.registration.member.session_id;
    if !monitor.has_registered_group(id) {
        return Some(());
    }
    {
        let mut record = owner.record.lock().ok()?;
        record.group = Some(ResponseEffectState::Running);
        let monitor = monitor.clone();
        owner.backend.publish_stop_witness(id, "session-destroy");
        record.group_worker = Some(tokio::task::spawn_blocking(move || {
            monitor.kill_group(
                id,
                crate::resource_monitor::ResourceKillReason::SessionDestroy,
            )
        }));
    }
    // The JoinHandle stays in the retained owner while it is polled. Losing
    // this waiter does not drop the only handle or fabricate a group outcome.
    std::future::poll_fn(|cx| {
        use std::future::Future;
        let Ok(mut record) = owner.record.lock() else {
            return std::task::Poll::Pending;
        };
        let Some(worker) = record.group_worker.as_mut() else {
            return std::task::Poll::Pending;
        };
        match std::pin::Pin::new(worker).poll(cx) {
            std::task::Poll::Pending => std::task::Poll::Pending,
            std::task::Poll::Ready(Ok(outcome)) => {
                if let Err(error) = &outcome {
                    log::warn!(
                        "[session] resource cleanup failed session={} source=ResponseClose: {}",
                        id,
                        error
                    );
                }
                record.group = Some(ResponseEffectState::Completed(outcome));
                let _ = record.group_worker.take();
                std::task::Poll::Ready(Some(()))
            }
            std::task::Poll::Ready(Err(error)) => {
                record.group = Some(ResponseEffectState::Ambiguous);
                let _ = record.group_worker.take();
                log::warn!(
                    "[session] resource cleanup task failed session={} source=ResponseClose: {}",
                    id,
                    error
                );
                std::task::Poll::Ready(None)
            }
        }
    })
    .await
}

pub(crate) async fn execute_response_close_transaction<R: tauri::Runtime>(
    transaction: &SelectionTransaction<R>,
    ticket: crate::pty::manager::ResponseCloseTicket,
) -> Result<ResponseCloseDisposition, String> {
    let id = ticket.committed_binding().member.session_id;
    execute_destroy_transaction_common(
        transaction,
        DestroyRequest {
            ids: vec![id],
            source: DestructionSource::BackgroundCleanup,
            force_destroy_root: false,
        },
        Some(ticket),
    )
    .await
    .map(|completion| match completion {
        DestroyCompletion::Response(disposition) => disposition,
        DestroyCompletion::Legacy(_) => {
            unreachable!("conditional close selected the response core")
        }
    })
}

async fn execute_owned_response_close<R: tauri::Runtime>(
    transaction: &SelectionTransaction<R>,
    ticket: crate::pty::manager::ResponseCloseTicket,
) -> Result<ResponseCloseDisposition, String> {
    let transaction = transaction.clone();
    crate::config::sessions_persistence::run_owned_persist_core(async move {
        execute_response_close_core(&transaction, ticket).await
    })
    .await
}

#[derive(Debug)]
pub(crate) enum ResponseCloseDisposition {
    Rejected,
    Dormant,
    Reopened,
    ReconcilePending,
}

enum DestroyCompletion {
    Legacy(DestroyOutcome),
    Response(ResponseCloseDisposition),
}

fn response_close_current_rows<'a>(
    current: &'a crate::session::manager::CarrierSnapshot,
    registration: &crate::session::selection::ResponseCloseRegistration,
) -> Option<(
    &'a crate::session::session::Session,
    &'a crate::session::session::Session,
)> {
    let find = |identity: &crate::pty::manager::ResponseRouteIdentity| {
        current.rows.iter().find(|row| {
            row.id == identity.session_id
                && row.token == identity.token
                && row.backend_kind == identity.backend
                && current.versions.get(&row.id) == Some(&identity.carrier_version)
                && !current.pending_ids.contains(&row.id)
                && !matches!(row.status, SessionStatus::Exited(_))
        })
    };
    let member = find(&registration.member)?;
    let coordinator = find(&registration.coordinator)?;
    if member.backend_kind != SessionBackendKind::LocalProcess
        || member.is_coordinator
        || member.is_root_agent
        || member.agent_kind.is_none()
        || !coordinator.is_coordinator
        || coordinator.is_root_agent
        || member
            .communication
            .as_ref()
            .is_some_and(|communication| communication.kind == SessionCommunicationKind::CoManaged)
    {
        return None;
    }
    Some((member, coordinator))
}

async fn commit_response_close<R: tauri::Runtime>(
    app: &AppHandle<R>,
    manager: &SessionManager,
    owner: &Arc<ResponseEffectOwner>,
) -> bool {
    let member = &owner.registration.member;
    let coordinator = &owner.registration.coordinator;
    let pty = app.state::<Arc<Mutex<PtyManager>>>().inner().clone();
    let Ok(member_route) = PtyManager::snapshot_route_proof(&pty, member.session_id) else {
        return false;
    };
    let Ok(coordinator_route) = PtyManager::snapshot_route_proof(&pty, coordinator.session_id)
    else {
        return false;
    };
    let Ok(member_authority) = PtyManager::authority_route_proof(&pty, member.session_id) else {
        return false;
    };
    let Ok(coordinator_authority) = PtyManager::authority_route_proof(&pty, coordinator.session_id)
    else {
        return false;
    };
    if member_route.backend_kind() != SessionBackendKind::LocalProcess
        || coordinator_route.backend_kind() != coordinator.backend
    {
        return false;
    }
    let settings_state = app.state::<SettingsState>();
    let roots = settings_state.read().await.project_paths.clone();
    if crate::config::teams::verified_wg_coordinator_target(&coordinator.target, &roots).is_none() {
        return false;
    }
    let settings = settings_state.read().await;
    if settings.project_paths != roots {
        return false;
    }
    let tracker = app
        .state::<crate::pty::manager::ResponseCloseState>()
        .inner()
        .clone();
    let permit = acquire_carrier_mutation_permit().await;
    use crate::config::sessions_persistence::{
        with_confirmed_carrier, CarrierCheckpointTransition,
    };
    use crate::config::settings::try_with_agents_layer_publication_state;
    use crate::path_identity::{same_object, verify_directory};
    use crate::session::session::is_working;
    manager
        .with_current_carrier_snapshot(&permit, |current| {
            let Some((member_row, coordinator_row)) =
                response_close_current_rows(current, &owner.registration)
            else {
                return false;
            };
            let member_cwd = Path::new(&member_row.working_directory);
            let coordinator_cwd = Path::new(&coordinator_row.working_directory);
            if !verify_directory(member_cwd)
                .is_ok_and(|actual| same_object(&actual, member_route.saved_cwd()))
                || !verify_directory(coordinator_cwd)
                    .is_ok_and(|actual| same_object(&actual, coordinator_route.saved_cwd()))
            {
                return false;
            }
            let now = std::time::Instant::now();
            tracker.apply_settings(
                settings.response_close_enabled,
                settings.response_close_idle_seconds,
                now,
            );
            tracker.publish_manager_facts(member.clone(), true, is_working(member_row), now);
            tracker.publish_manager_facts(
                coordinator.clone(),
                false,
                is_working(coordinator_row),
                now,
            );
            let carrier = &owner.registration.carrier;
            try_with_agents_layer_publication_state(&carrier.directory, |suppressed| {
                if suppressed {
                    return None;
                }
                let _coordinator_guard = coordinator_authority
                    .lock_verified(
                        coordinator.backend,
                        coordinator_route.saved_cwd(),
                        coordinator_route.saved_replica(),
                    )
                    .ok()?;
                if tracker.generation(coordinator.session_id) != Some(coordinator.generation) {
                    return None;
                }
                with_confirmed_carrier(
                    &permit,
                    carrier,
                    current,
                    CarrierCheckpointTransition::BeginClosing,
                    |_| {
                        member_authority
                            .try_commit_response_close(&tracker, &owner.ticket, now)
                            .ok()
                    },
                )
            })
            .ok()
            .flatten()
            .is_some()
        })
        .await
}

async fn execute_response_close_core<R: tauri::Runtime>(
    transaction: &SelectionTransaction<R>,
    ticket: crate::pty::manager::ResponseCloseTicket,
) -> Result<ResponseCloseDisposition, String> {
    let binding = ticket.committed_binding();
    if binding.member.backend != SessionBackendKind::LocalProcess {
        return Ok(ResponseCloseDisposition::Rejected);
    }
    let Some(registration) =
        crate::session::selection::response_close_registration(binding.member.session_id)
    else {
        return Ok(ResponseCloseDisposition::Rejected);
    };
    if registration.member != binding.member {
        return Ok(ResponseCloseDisposition::Rejected);
    }
    let backend = {
        let pty = transaction.app().state::<Arc<Mutex<PtyManager>>>();
        let pty = pty.lock().map_err(|_| "pty_manager_poisoned".to_owned())?;
        pty.backend_for_kind(binding.member.backend)
    };
    let owner = Arc::new(ResponseEffectOwner {
        ticket,
        registration,
        backend,
        record: Mutex::new(ResponseEffectRecord {
            stages: Vec::with_capacity(16),
            group: None,
            group_worker: None,
            sealed: false,
        }),
    });
    {
        let mut owners = response_effect_owners()
            .lock()
            .map_err(|_| "response_effect_owner_poisoned".to_owned())?;
        if owners.contains_key(&binding.member.session_id) {
            return Ok(ResponseCloseDisposition::ReconcilePending);
        }
        owners.insert(binding.member.session_id, owner.clone());
    }
    let manager = transaction.manager().await;
    if !commit_response_close(transaction.app(), &manager, &owner).await {
        response_effect_owners()
            .lock()
            .map_err(|_| "response_effect_owner_poisoned".to_owned())?
            .remove(&binding.member.session_id);
        return Ok(ResponseCloseDisposition::Rejected);
    }
    let monitor = transaction
        .app()
        .state::<Arc<ResourceMonitorState>>()
        .inner()
        .clone();
    if response_group_effect(&owner, &monitor).await.is_none() {
        return Ok(ResponseCloseDisposition::ReconcilePending);
    }
    let Some(kill) = owner.start("kill") else {
        return Ok(ResponseCloseDisposition::ReconcilePending);
    };
    let kill_result = owner
        .backend
        .kill(binding.member.session_id)
        .map_err(|error| error.to_string());
    let kill_error = kill_result.as_ref().err().cloned();
    if owner.complete(kill, kill_result).is_none() {
        return Ok(ResponseCloseDisposition::ReconcilePending);
    }
    if owner.backend.has_session(binding.member.session_id) {
        let _ = owner.seal();
        return Ok(settle_response_close(transaction.app(), &manager, &owner)
            .await
            .unwrap_or(ResponseCloseDisposition::ReconcilePending));
    }
    if let Some(error) = kill_error {
        log::warn!(
            "[session] teardown reported an error after PTY loss session={} \
             source=ResponseClose: {}",
            binding.member.session_id,
            error
        );
    }
    run_response_close_auxiliary(transaction, &manager, &owner).await;
    let _ = owner.seal();
    Ok(settle_response_close(transaction.app(), &manager, &owner)
        .await
        .unwrap_or(ResponseCloseDisposition::ReconcilePending))
}

async fn detach_response_close_telegram<R: tauri::Runtime>(
    app: &AppHandle<R>,
    id: Uuid,
) -> Result<(), String> {
    let bridge_shutdown = {
        let telegram = app.state::<TelegramBridgeState>();
        let mut telegram = telegram.lock().await;
        if telegram.has_bridge(id) {
            telegram
                .detach(id)
                .map(Some)
                .map_err(|error| error.to_string())
        } else {
            Ok(None)
        }
    };
    match bridge_shutdown {
        Ok(Some(shutdown)) => {
            // Retain the bot assignment on A as resume intent while the live
            // bridge shuts down; the next owned replacement carries it.
            let result = app
                .emit(
                    "telegram_bridge_detached",
                    serde_json::json!({ "sessionId": id.to_string() }),
                )
                .map_err(|error| error.to_string());
            shutdown.spawn_wait_or_abort();
            result
        }
        Ok(None) => Ok(()),
        Err(error) => Err(error),
    }
}

async fn run_response_close_auxiliary<R: tauri::Runtime>(
    transaction: &SelectionTransaction<R>,
    manager: &SessionManager,
    owner: &Arc<ResponseEffectOwner>,
) {
    let id = owner.registration.member.session_id;
    let Some(detach) = owner.start("detach") else {
        return;
    };
    let detached_result = transaction
        .app()
        .state::<DetachedSessionsState>()
        .lock()
        .map(|mut detached| {
            detached.remove(&id);
        })
        .map_err(|_| "detached_sessions_poisoned".to_owned());
    if owner.complete(detach, detached_result).is_none() {
        return;
    }

    let Some(bridge) = owner.start("telegram") else {
        return;
    };
    let bridge_result = detach_response_close_telegram(transaction.app(), id).await;
    if let Err(error) = &bridge_result {
        log::warn!(
            "[session] telegram detach publication failed session={} source=ResponseClose: {}",
            id,
            error
        );
    }
    if owner.complete(bridge, bridge_result).is_none() {
        return;
    }

    let Some(commit) = owner.start("manager") else {
        return;
    };
    let committed = transaction.commit_response_close_retained(id).await;
    let committed = match committed {
        Ok(committed) => {
            let _ = owner.complete(commit, Ok(()));
            committed
        }
        Err(error) => {
            let _ = owner.complete(commit, Err(error));
            return;
        }
    };
    let Some(persist) = owner.start("persist") else {
        return;
    };
    let persisted =
        crate::config::sessions_persistence::persist_current_state_result(manager).await;
    if let Err(error) = &persisted {
        log::warn!(
            "[selection] persistence failed source={} session={:?}: {}",
            SelectionSource::BackgroundCleanup,
            Some(id),
            error
        );
    }
    if owner.complete(persist, persisted).is_none() {
        return;
    }

    let Some(readers) = owner.start("readers") else {
        return;
    };
    crate::commands::telegram::release_all_reader_demands(transaction.app(), id).await;
    if owner.complete(readers, Ok(())).is_none() {
        return;
    }

    let Some(window) = owner.start("window") else {
        return;
    };
    let label = format!("terminal-{}", id.to_string().replace('-', ""));
    let closed = transaction
        .app()
        .get_webview_window(&label)
        .map_or(Ok(()), |window| {
            window.destroy().map_err(|error| error.to_string())
        });
    if let Err(error) = &closed {
        log::warn!(
            "[session] detached window destroy failed session={} source=ResponseClose: {}",
            id,
            error
        );
    }
    if owner.complete(window, closed).is_none() {
        return;
    }

    let Some(events) = owner.start("events") else {
        return;
    };
    transaction.publish_destroyed(id);
    reset_substantive_input(transaction.app(), id);
    for row in &committed.changed_rows {
        transaction.publish_created(row);
    }
    for id in &committed.cleared_raise_hand_ids {
        transaction.publish_communication_cleared(*id);
    }
    if let Some(selection) = &committed.selection {
        transaction.publish_selection(selection);
    }
    let _ = owner.complete(events, Ok(()));
}

async fn settle_response_close<R: tauri::Runtime>(
    app: &AppHandle<R>,
    manager: &SessionManager,
    owner: &Arc<ResponseEffectOwner>,
) -> Option<ResponseCloseDisposition> {
    if !owner.terminal() {
        return None;
    }
    let binding = owner.ticket.committed_binding();
    let pty = app.state::<Arc<Mutex<PtyManager>>>().inner().clone();
    let route = PtyManager::snapshot_route_proof(&pty, binding.member.session_id).ok()?;
    let authority = PtyManager::authority_route_proof(&pty, binding.member.session_id).ok()?;
    if route.backend_kind() != SessionBackendKind::LocalProcess {
        return None;
    }
    let tracker = app
        .state::<crate::pty::manager::ResponseCloseState>()
        .inner()
        .clone();
    let permit = acquire_carrier_mutation_permit().await;
    use crate::config::sessions_persistence::{
        with_confirmed_carrier, CarrierCheckpointTransition,
    };
    use crate::config::settings::try_with_agents_layer_publication_state;
    use crate::path_identity::{same_object, verify_directory};
    use crate::pty::context_scrape::ContextSessionLiveness;
    use crate::pty::manager::{
        ResponseCloseCurrentSettlement, ResponseCloseSettlementEvidence,
        ResponseCloseSettlementOutcome,
    };
    use crate::session::manager::CarrierPhysicalIdentity;
    use crate::session::session::is_working;
    let settled = manager
        .with_current_carrier_snapshot(&permit, |current| {
            let row = current.rows.iter().find(|row| {
                row.id == binding.member.session_id
                    && row.token == binding.member.token
                    && row.backend_kind == SessionBackendKind::LocalProcess
                    && !current.pending_ids.contains(&row.id)
            })?;
            if !verify_directory(Path::new(&row.working_directory))
                .is_ok_and(|actual| same_object(&actual, route.saved_cwd()))
            {
                return None;
            }
            let _lifecycle = authority
                .lock_verified(
                    SessionBackendKind::LocalProcess,
                    route.saved_cwd(),
                    route.saved_replica(),
                )
                .ok()?;
            if tracker.generation(row.id) != Some(binding.member.generation) {
                return None;
            }
            let absent = !owner.backend.has_session(row.id);
            let dormant = absent
                && matches!(row.status, SessionStatus::Exited(0))
                && !row.was_detached
                && current.selection.id() != Some(row.id);
            let live = !absent
                && owner.kill_failed()
                && !matches!(row.status, SessionStatus::Exited(_))
                && current.versions.get(&row.id).is_some_and(|version| {
                    *version >= binding.member.carrier_version && *version < u64::MAX
                })
                && owner.backend.context_session_liveness(row.id) == ContextSessionLiveness::Live;
            if !dormant && !live {
                return None;
            }
            let detached = app.state::<DetachedSessionsState>();
            if dormant && detached.lock().ok()?.contains(&row.id) {
                return None;
            }
            let retired_replica = if dormant {
                Some(CarrierPhysicalIdentity::from(
                    route.saved_replica()?.clone(),
                ))
            } else {
                None
            };
            let carrier = &owner.registration.carrier;
            try_with_agents_layer_publication_state(&carrier.directory, |suppressed| {
                if suppressed {
                    return None;
                }
                let transition = if dormant {
                    CarrierCheckpointTransition::FinishDormant {
                        generation: binding.member.generation,
                        committed_version: binding.member.carrier_version,
                        replica: retired_replica.as_ref()?,
                    }
                } else {
                    CarrierCheckpointTransition::FinishClosing
                };
                with_confirmed_carrier(&permit, carrier, current, transition, |stamp| {
                    let mut identity = binding.member.clone();
                    identity.carrier_version = stamp.version;
                    tracker.publish_manager_facts(
                        identity,
                        !dormant,
                        is_working(row),
                        std::time::Instant::now(),
                    );
                    let outcome = if dormant {
                        ResponseCloseSettlementOutcome::Dormant {
                            exact_backend_absent: true,
                            all_effects_completed: true,
                            retained_exited_confirmed: true,
                            selection_settled: true,
                        }
                    } else {
                        ResponseCloseSettlementOutcome::ReopenAfterKillFailure {
                            terminal_kill_failure: true,
                            all_effects_terminal: true,
                            original_backend_alive: true,
                            original_manager_identity_intact: true,
                        }
                    };
                    let current_settlement = ResponseCloseCurrentSettlement {
                        current_carrier_version: stamp.version,
                        confirmed_carrier_version: stamp.version,
                        checkpoint_nonce: stamp.nonce,
                        current_checkpoint_nonce: stamp.nonce,
                        publication_epoch: stamp.epoch,
                        current_publication_epoch: stamp.epoch,
                    };
                    tracker
                        .finish_response_close(
                            &owner.ticket,
                            ResponseCloseSettlementEvidence {
                                committed_binding: binding.clone(),
                                current_settlement,
                                outcome,
                            },
                        )
                        .ok()?;
                    Some(if dormant {
                        ResponseCloseDisposition::Dormant
                    } else {
                        ResponseCloseDisposition::Reopened
                    })
                })
            })
            .ok()
            .flatten()
        })
        .await;
    drop(permit);
    if let Some(disposition) = settled {
        response_effect_owners()
            .lock()
            .ok()?
            .remove(&binding.member.session_id);
        crate::session::selection::forget_response_close_registration(binding.member.session_id);

        if matches!(disposition, ResponseCloseDisposition::Dormant) {
            pty.lock()
                .ok()?
                .remove_route_if_kind(binding.member.session_id, binding.member.backend);
        }
        return Some(disposition);
    }
    None
}

pub(crate) async fn reconcile_response_closes<R: tauri::Runtime>(app: &AppHandle<R>) {
    let Some(tracker) = app.try_state::<crate::pty::manager::ResponseCloseState>() else {
        return;
    };
    let owners = response_effect_owners()
        .lock()
        .ok()
        .map(|owners| owners.values().cloned().collect::<Vec<_>>());
    let Some(owners) = owners.filter(|owners| !owners.is_empty()) else {
        return;
    };
    let manager = app
        .state::<Arc<tokio::sync::RwLock<SessionManager>>>()
        .read()
        .await
        .clone();
    for owner in owners {
        if !owner.registration.tracker.same_state(&tracker) || !owner.terminal() {
            continue;
        }
        // Reconcile only the latest carrier, without repeating an observed
        // kill, group operation, or cleanup after a save/readback failure.
        if crate::config::sessions_persistence::persist_current_state_result(&manager)
            .await
            .is_ok()
        {
            let _ = settle_response_close(app, &manager, &owner).await;
        }
    }
}

pub(crate) async fn execute_destroy_transaction<R: tauri::Runtime>(
    transaction: &SelectionTransaction<R>,
    request: DestroyRequest,
) -> Result<DestroyOutcome, String> {
    match execute_destroy_transaction_common(transaction, request, None).await? {
        DestroyCompletion::Legacy(outcome) => Ok(outcome),
        DestroyCompletion::Response(_) => unreachable!("legacy destroy selected its original core"),
    }
}

async fn publish_legacy_destroy_outcome<R: tauri::Runtime>(
    transaction: &SelectionTransaction<R>,
    source: &DestructionSource,
    outcome: &DestroyOutcome,
    committed: &crate::session::manager::CommitResult,
    retained_marked: &std::collections::HashSet<uuid::Uuid>,
) {
    for session_id in outcome
        .destroyed_ids
        .iter()
        .chain(outcome.retained_exited_ids.iter())
        .copied()
    {
        transaction.publish_destroyed(session_id);
        let label = format!("terminal-{}", session_id.to_string().replace('-', ""));
        if let Some(window) = transaction.app().get_webview_window(&label) {
            if let Err(error) = window.destroy() {
                log::warn!(
                    "[session] detached window destroy failed session={} source={:?}: {}",
                    session_id,
                    source,
                    error
                );
            }
        }
        reset_substantive_input(transaction.app(), session_id);
    }

    // #1171 - a SEPARATE loop over `destroyed_ids` only, rather than a membership test inside
    // the loop above. That loop iterates `destroyed_ids.chain(retained_exited_ids)`, so a
    // `contains` check would be O(n squared) and, worse, would express a business rule as a
    // condition inside a loop that does something else. A separate loop makes "destroyed only"
    // structural.
    //
    // Root-agent sessions retained as `Exited` are deliberately NOT purged: they stay in the
    // manager and their row is still in the list, so their post-mortem view still has a place
    // to be shown.
    for session_id in outcome.destroyed_ids.iter().copied() {
        purge_session_side_state(transaction.app(), session_id);
        // #2232 phase 4 section 5: destroy releases **both** demands, which
        // cancels the reader and closes the session's `CaptureRegistry` entry.
        crate::commands::telegram::release_all_reader_demands(transaction.app(), session_id).await;
    }

    for row in committed.changed_rows.iter().filter(|row| {
        Uuid::parse_str(&row.id)
            .ok()
            .is_some_and(|id| retained_marked.contains(&id))
    }) {
        transaction.publish_created(row);
    }
    for session_id in &committed.cleared_raise_hand_ids {
        transaction.publish_communication_cleared(*session_id);
    }
    if let Some(selection) = committed.selection.as_ref() {
        transaction.publish_selection(selection);
    }
}

async fn execute_destroy_transaction_common<R: tauri::Runtime>(
    transaction: &SelectionTransaction<R>,
    request: DestroyRequest,
    response_ticket: Option<crate::pty::manager::ResponseCloseTicket>,
) -> Result<DestroyCompletion, String> {
    if let Some(ticket) = response_ticket {
        return execute_owned_response_close(transaction, ticket)
            .await
            .map(DestroyCompletion::Response);
    }
    let planned = request
        .ids
        .iter()
        .copied()
        .collect::<std::collections::HashSet<_>>();
    let before = transaction.aggregate_snapshot().await;
    let selected_id = before.selection.id();
    let mut outcome = DestroyOutcome::default();
    let mut mutations = LifecycleMutations::default();
    let mut retained_marked = std::collections::HashSet::new();

    for session_id in request.ids.iter().copied() {
        destroy_one_legacy_session(
            transaction,
            &request,
            &before,
            session_id,
            &mut outcome,
            &mut mutations,
            &mut retained_marked,
        )
        .await;
    }

    let selected_was_torn_down = selected_id.is_some_and(|selected| outcome.succeeded(selected));
    let decision = if !selected_was_torn_down {
        CommitDecision::Keep
    } else {
        match request.source {
            DestructionSource::ManualClose => {
                let final_snapshot = transaction.aggregate_snapshot().await;
                let mut fallback = None;
                for candidate_id in &final_snapshot.order {
                    let Some(candidate) = final_snapshot
                        .sessions
                        .iter()
                        .find(|candidate| candidate.id == *candidate_id)
                    else {
                        continue;
                    };
                    if planned.contains(&candidate.id) {
                        log::info!(
                            "[selection] fallback candidate={} status={:?} hasPty=false \
                             detached=false excluded=true reason=plannedForDestruction",
                            candidate.id,
                            candidate.status
                        );
                        continue;
                    }
                    if matches!(candidate.status, SessionStatus::Exited(_)) {
                        log::info!(
                            "[selection] fallback candidate={} status={:?} hasPty=false \
                             detached=false excluded=false reason=exited",
                            candidate.id,
                            candidate.status
                        );
                        continue;
                    }
                    let runtime = transaction.runtime_snapshot(candidate.id);
                    if runtime.detached {
                        log::info!(
                            "[selection] fallback candidate={} status={:?} hasPty={} detached=true \
                             excluded=false reason=detached",
                            candidate.id,
                            candidate.status,
                            runtime.has_pty
                        );
                        continue;
                    }
                    if !runtime.has_pty {
                        log::info!(
                            "[selection] fallback candidate={} status={:?} hasPty=false \
                             detached=false excluded=false reason=missingPty",
                            candidate.id,
                            candidate.status
                        );
                        continue;
                    }
                    fallback = transaction.live_decision(candidate.id);
                    if fallback.is_some() {
                        break;
                    }
                }
                fallback.unwrap_or(CommitDecision::Clear)
            }
            DestructionSource::AutoClose
            | DestructionSource::SpawnRollback
            | DestructionSource::BackgroundCleanup => CommitDecision::Clear,
        }
    };

    let committed = transaction
        .commit(decision, request.source.cause(), mutations)
        .await?;
    if !outcome.destroyed_ids.is_empty() || !outcome.retained_exited_ids.is_empty() {
        transaction
            .persist(request.source.selection_source(), selected_id)
            .await;
    }

    publish_legacy_destroy_outcome(
        transaction,
        &request.source,
        &outcome,
        &committed,
        &retained_marked,
    )
    .await;

    Ok(DestroyCompletion::Legacy(outcome))
}

#[tauri::command]
pub async fn destroy_session(
    app: AppHandle,
    _session_mgr: State<'_, Arc<tokio::sync::RwLock<SessionManager>>>,
    _pty_mgr: State<'_, Arc<Mutex<PtyManager>>>,
    _tg_mgr: State<'_, TelegramBridgeState>,
    _detached: State<'_, DetachedSessionsState>,
    id: String,
) -> Result<(), String> {
    let uuid = Uuid::parse_str(&id).map_err(|e| e.to_string())?;
    destroy_session_inner(&app, uuid).await
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CoordinatorCloseOutcome {
    /// true if the close was performed; false if confirmation is required first.
    pub closed: bool,
    /// number of live, working (not waiting_for_input) team members; only
    /// meaningful when `closed == false`.
    pub working_count: usize,
}

/// #588 Count LIVE team members that are working (not waiting_for_input). Pure so
/// the working-gate rule is unit-testable without a live app, mirroring how
/// `auto_close.rs` extracts `team_is_closeable`. A member counts only when it is
/// both live (PTY-backed) and not waiting for input.
fn count_working_members(members: &[(bool /*live*/, bool /*waiting_for_input*/)]) -> usize {
    members
        .iter()
        .filter(|(live, waiting)| *live && !*waiting)
        .count()
}

async fn execute_manual_coordinator_destroy<R: tauri::Runtime>(
    app: &AppHandle<R>,
    coordinator_id: Uuid,
    member_ids: Vec<Uuid>,
    cascade: bool,
) -> Result<DestroyOutcome, String> {
    let mut planned_ids = if cascade { member_ids } else { Vec::new() };
    planned_ids.push(coordinator_id);
    let outcome =
        destroy_sessions_with_source(app, planned_ids, DestructionSource::ManualClose, false)
            .await?;
    for (failed_id, error) in &outcome.failed {
        if *failed_id != coordinator_id {
            log::warn!(
                "[manual-close] member destroy {} failed: {}",
                &failed_id.to_string()[..8],
                error
            );
        }
    }
    outcome.clone().into_single_result(coordinator_id)?;
    Ok(outcome)
}

/// #588 Manually close a coordinator (and, when the cascade setting is on, its
/// team). Sets the MANUALLY-CLOSED marker on the coordinator regardless of the
/// cascade setting. When cascade is on and confirmation has not been given and at
/// least one team member is working, closes NOTHING and reports `closed:false` so
/// the FE can show the confirmation modal.
#[tauri::command]
pub async fn close_coordinator(
    app: AppHandle,
    id: String,
    confirmed: bool,
) -> Result<CoordinatorCloseOutcome, String> {
    let uuid = Uuid::parse_str(&id).map_err(|e| e.to_string())?;
    let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();

    // Snapshot coordinator facts BEFORE destroying (destroy removes the record).
    let (is_coordinator, cwd) = {
        let mgr = session_mgr.read().await;
        let s = mgr
            .get_session(uuid)
            .await
            .ok_or_else(|| "Session not found".to_string())?;
        (s.is_coordinator, s.working_directory.clone())
    };

    // Defensive: a non-coordinator target is a plain destroy (no marker/cascade).
    if !is_coordinator {
        destroy_session_inner(&app, uuid).await?;
        return Ok(CoordinatorCloseOutcome {
            closed: true,
            working_count: 0,
        });
    }

    let fqn = crate::config::teams::agent_fqn_from_path(&cwd);
    let team_key = match fqn.rsplit_once('/') {
        Some((team, _agent)) => team.to_string(),
        None => String::new(),
    };

    // E0716: bind the State to a local FIRST (matches auto_close.rs:115). The
    // guard-bound-then-used-next-line form does NOT compile (the temporary State is
    // dropped at the `;`, so the read guard dangles). `bool` is Copy.
    let settings = app.state::<SettingsState>();
    let cascade = settings
        .current_read()
        .await
        .map_err(|error| error.to_string())?
        .coordinator_cascade_close_enabled;
    let pty_mgr = app.state::<Arc<Mutex<PtyManager>>>();

    // LIVE-PTY members of THIS team, excluding the coordinator (decision 1; mirrors
    // auto_close.rs:136-142). agent_team_members() includes EXITED/dormant records
    // (exited sessions persist until destroy), so we filter on has_session to a live
    // set ONCE and share it between the working-count gate and the destroy loop;
    // dormant rows are never reaped. The std Mutex guard is taken AFTER the tokio
    // read await and dropped at the block end, so no lock is held across an await.
    let live_ids: Vec<Uuid> = {
        let mgr = session_mgr.read().await;
        let members = mgr.agent_team_members().await;
        let pm = pty_mgr.lock().unwrap_or_else(|e| e.into_inner());
        members
            .into_iter()
            .filter(|(mid, tk)| {
                *mid != uuid && !team_key.is_empty() && *tk == team_key && pm.has_session(*mid)
            })
            .map(|(mid, _)| mid)
            .collect()
    };

    // Confirmation gate: cascade ON, not yet confirmed, >=1 LIVE member working.
    // Only the tokio read guard is held across the get_session awaits.
    if cascade && !confirmed {
        let mgr = session_mgr.read().await;
        let mut member_states: Vec<(bool, bool)> = Vec::with_capacity(live_ids.len());
        for mid in &live_ids {
            // live_ids is pre-filtered to live PTYs, so `live` is true; the waiting
            // flag is the authoritative session record (absent -> treat as idle so a
            // vanished member never forces a modal).
            let waiting = mgr
                .get_session(*mid)
                .await
                .map(|s| s.waiting_for_input)
                .unwrap_or(true);
            member_states.push((true, waiting));
        }
        let working_count = count_working_members(&member_states);
        if working_count > 0 {
            return Ok(CoordinatorCloseOutcome {
                closed: false,
                working_count,
            });
        }
    }

    // Submit the complete cascade as one destruction transaction. The full
    // planned set is excluded from fallback ranking, member failures are known
    // before the one final manual selection, and no doomed sibling can become an
    // intermediate canonical target.
    execute_manual_coordinator_destroy(&app, uuid, live_ids, cascade).await?;

    // Set the manual marker on the coordinator FQN and persist immediately (this
    // command is not on the auto-close tick, so flush_clocks won't run).
    if let Some(clocks) = app.try_state::<CoordinatorClocksState>() {
        let now = chrono::Utc::now();
        let newly = clocks
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .mark_manually_closed(&fqn, now);
        if newly {
            let _ = app.emit(
                "coordinator_manual_close_changed",
                serde_json::json!({ "replicaPath": cwd, "manuallyClosedAt": now.to_rfc3339() }),
            );
            // also clear any stale auto-closed pill on the FE (mark_* cleared it
            // server-side via the mutual-exclusion transition).
            let _ = app.emit(
                "coordinator_auto_close_changed",
                serde_json::json!({ "replicaPath": cwd, "autoClosedAt": null }),
            );
        }
        let snapshot = { clocks.lock().unwrap_or_else(|e| e.into_inner()).snapshot() };
        if let Err(e) = crate::config::coordinator_clocks::save_map(&snapshot) {
            log::warn!("[manual-close] clocks save failed: {}", e);
        }
    }

    Ok(CoordinatorCloseOutcome {
        closed: true,
        working_count: 0,
    })
}

/// Resolves the effective `skip_auto_resume` flag for `restart_session`.
/// Defaults to `true` (fresh conversation) to preserve existing restart-button semantics.
/// `Some(false)` is used by the deferred-wake path (ProjectPanel.handleReplicaClick)
/// to allow provider auto-resume and continue the prior conversation.
fn effective_restart_skip_auto_resume(requested: Option<bool>) -> bool {
    requested.unwrap_or(true)
}

/// (#630/#631) Compose the restart path's effective fresh intent: fresh on an
/// explicit "Restart Session" (`requested` defaults to `true`), OR if the session
/// still carries an unconsumed durable fresh intent (`stored_start_fresh`). A
/// deferred member reopened via Branch A passes `Some(false)`, but its persisted
/// intent wins, so it stays fresh; a normal member (`false || false`) resumes,
/// unchanged. Named so the composition is unit-tested; the call site is guarded
/// against a silent revert by the `dead_code` lint under `-D warnings` (CI runs
/// `cargo clippy --all-targets -- -D warnings`), which fails the build if
/// reverting to the inline expression leaves this function unused. Mirrors
/// `lib::skip_auto_resume_for_restore`.
fn restart_skip_auto_resume_with_intent(stored_start_fresh: bool, requested: Option<bool>) -> bool {
    stored_start_fresh || effective_restart_skip_auto_resume(requested)
}

/// (#747) Decide whether a raised hand carries across the restart
/// destroy+create boundary. Carry ONLY a visible `RaiseHand` and ONLY when the
/// conversation resumes (`restart_start_fresh == false`, i.e. the Branch-A
/// reopen of a dormant session). A fresh restart starts a new conversation;
/// the raise belonged to the old one and is dropped, consistent with
/// "destroyed sessions leave no stale raised-hand state" (#747 AC 4).
///
/// Seam map: the explicit "Restart Session" command and the bulk
/// profile-assignment restart (commands/config.rs:627-637) pass fresh and drop
/// (user-initiated resets); the sidebar dormant reopen passes `Some(false)`
/// and carries; the startup wake arm reuses this helper with the persisted
/// intent (#747 plan change 3); the agent-initiated self-clear-and-handoff
/// restart carries via its own capture/re-apply (#747 plan change 10) because
/// it is not user input. `pub(crate)` so lib.rs's wake arm shares the exact
/// same decision.
pub(crate) fn carry_communication_for_restart(
    stored: Option<SessionCommunication>,
    restart_start_fresh: bool,
) -> Option<SessionCommunication> {
    if restart_start_fresh {
        return None;
    }
    stored.filter(|c| c.kind == SessionCommunicationKind::RaiseHand && c.visible)
}

/// (#599) Resolves the effective `skip_auto_resume` for the `create_session`
/// command. Defaults to `true` (fresh conversation) so every existing
/// create-in-place / new-agent / open-agent / CLI / web call site keeps its
/// intentional "no --continue on a fresh create" semantics. The
/// reopen-of-a-closed-coordinator path (ProjectPanel.handleReplicaClick) passes
/// `Some(false)` to resume the prior conversation. Mirror of
/// `effective_restart_skip_auto_resume` so both create and restart routes share
/// the same default.
fn effective_create_skip_auto_resume(requested: Option<bool>) -> bool {
    requested.unwrap_or(true)
}

/// (#756) Create-path mirror consume: force a fresh spawn when the caller
/// requested resume but the cwd's coordinator carries a pending fresh boundary.
/// Named + unit-tested per the #630 pattern (the call site is guarded against a
/// silent inline revert by the `dead_code` lint under CI's `-D warnings`).
fn mirror_forces_fresh(
    requested_skip_auto_resume: bool,
    is_coordinator: bool,
    mirror_pending: bool,
) -> bool {
    !requested_skip_auto_resume && is_coordinator && mirror_pending
}

fn effective_restart_requested_profile(
    requested: Option<String>,
    stored: Option<String>,
) -> Option<String> {
    requested.or(stored)
}

/// #2434 - one matched selection, read by both the restart and the drift badge
/// so a Reload launches exactly what the badge compared against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MatchedSelection {
    /// No coding-agent reference at all: restart the saved shell. Today's path.
    NoAgent,
    /// A reference that resolved.
    Agent {
        agent_id: String,
        /// The letter to launch, straight from `AgentMatch.profile_letter`;
        /// `None` whenever no letter was chosen. An absent request stays absent.
        profile_letter: Option<String>,
        /// True only when `tier.is_some()` and `profile_letter` is `Some`: a
        /// matched letter outranks the replica pin for this spawn.
        authoritative: bool,
        tier: Option<crate::config::agent_command::MatchTier>,
        original_letter: Option<String>,
    },
    /// A reference that exists and resolves to nothing.
    Unresolved { reference: String },
}

/// #2434 - pure, read-only: pick the reference a restart of `cwd` would launch
/// (explicit request, then `tooling.currentCodingAgent`, then the session's own
/// agent) and resolve it through `resolve_portable_reference`. Never falls back
/// to another agent and never writes the reference back.
pub(crate) fn matched_selection(
    settings: &AppSettings,
    cwd: &str,
    requested_agent_id: Option<&str>,
    stored_agent_id: Option<&str>,
    source_letter: Option<&str>,
) -> MatchedSelection {
    let dir = std::path::Path::new(cwd);
    // #2786 (C1) - ONE load through the shared loader serves both
    // `currentCodingAgent` and the descriptor, so they share one snapshot.
    let config = match crate::config::agent_config::read_agent_local_config_json(dir) {
        Ok(value) => value,
        Err(e) => {
            log::warn!(
                "[agent-match] malformed '{}', no descriptor: {}",
                dir.join("config.json").display(),
                e
            );
            None
        }
    };
    let current = config
        .as_ref()
        .and_then(|value| value.get("tooling")?.get("currentCodingAgent")?.as_str())
        // An empty or whitespace-only id names nothing: it is absent, always.
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_string);
    // Only a stale `currentCodingAgent` on a session with no agent of its own
    // may degrade to the saved shell; an explicit request never does.
    let plain_shell_selection =
        requested_agent_id.is_none() && stored_agent_id.is_none() && current.is_some();
    let Some(id) = requested_agent_id
        .map(str::to_string)
        .or(current)
        .or_else(|| stored_agent_id.map(str::to_string))
    else {
        return MatchedSelection::NoAgent;
    };
    let mut reference =
        crate::config::agent_command::StoredReference::from_config(config.as_ref(), &id);
    reference.source_letter = source_letter.map(str::to_string);
    match crate::config::agent_command::resolve_portable_reference(settings, &reference) {
        crate::config::agent_command::MatchOutcome::Matched(found) => MatchedSelection::Agent {
            authoritative: found.tier.is_some() && found.profile_letter.is_some(),
            agent_id: found.agent_id,
            profile_letter: found.profile_letter,
            tier: found.tier,
            original_letter: found.original_letter,
        },
        crate::config::agent_command::MatchOutcome::NoMatch { reference } => {
            if plain_shell_selection {
                // Nothing else to launch: the session already is a plain shell.
                log::warn!(
                    "[agent-match] ignoring unresolvable currentCodingAgent '{}' in '{}'; \
                     restarting the saved shell",
                    reference,
                    cwd
                );
                MatchedSelection::NoAgent
            } else {
                MatchedSelection::Unresolved { reference }
            }
        }
    }
}

/// #537 read-side: pick which coding agent a restart should launch.
///
/// Ranks an explicit request first, then the replica's Selection-UI assignment
/// (`tooling.currentCodingAgent` in `cwd/config.json`, validated against the
/// configured agents), then the agent the live session was launched with. This
/// mirrors the wake path (#534): a fresh user selection must override stale
/// launch history. The `currentCodingAgent` is validated so an unconfigured id
/// is never passed downstream (which would make `build_configured_agent_spawn_for_cwd`
/// return `Ok(None)` and silently keep the old recipe). Root agents and plain
/// terminals have no `currentCodingAgent`, so they fall straight through to
/// `stored_agent_id` and their behavior is unchanged.
pub(crate) fn resolve_restart_selected_agent_id(
    settings: &AppSettings,
    cwd: &str,
    requested_agent_id: Option<&str>,
    stored_agent_id: Option<&str>,
) -> Option<String> {
    if let Some(requested) = requested_agent_id {
        return Some(requested.to_string());
    }
    let current_selection =
        crate::config::coding_agent_profiles::read_replica_current_coding_agent(
            std::path::Path::new(cwd),
        )
        .filter(|id| settings.agents.iter().any(|agent| agent.id == *id));
    current_selection.or_else(|| stored_agent_id.map(str::to_string))
}

#[derive(Debug, Clone)]
pub(crate) struct RestartJobRequest {
    pub session_id: Uuid,
    pub agent_id: Option<String>,
    pub requested_profile: Option<String>,
    pub skip_auto_resume: Option<bool>,
    pub activate_after: bool,
    pub intent: TrustedRestartIntent,
    pub communication_override: Option<SessionCommunication>,
    /// §1295 5.1b — creation-gate enforcement for the restart replacement.
    /// Root-agent and archived-root cwds are exempt by the gate itself, so
    /// callers pass the build default; the mailbox wake-restart passes
    /// `Enforce` explicitly (§1295 6.6 / mailbox.rs:9796).
    pub enforcement: crate::config::sessions_persistence::CreationGateEnforcement,
}

/// Restart a session: destroy the existing one and recreate it with the same
/// configuration but a fresh PTY. By default suppresses provider auto-resume
/// (true user-intent restart — fresh conversation).
///
/// Callers that are instead *waking* a previously-deferred session pass
/// `skip_auto_resume = Some(false)`. A session is "deferred" (PTY `Exited(0)`
/// at startup) for any of the following reasons:
///
///   1. `restoreCoordinatorWakeState = false` (the new-policy default per
///      issue #248) defers every persisted session regardless of agent type.
///   2. `restoreCoordinatorWakeState = true` defers all non-coordinator team
///      members (these are never auto-woken on startup under the new policy).
///   3. `restoreCoordinatorWakeState = true` defers coordinators whose PTY
///      status was `Exited(_)` at the prior shutdown (the state-sensitive
///      branch added by issue #248).
///   4. The user explicitly closed the session during the prior run.
///
/// In all four cases, `skip_auto_resume = Some(false)` allows the next PTY
/// spawn to inject `claude --continue`, `codex resume --last`, or
/// `antigravity --continue` so the prior conversation continues.
///
/// The restarted session is automatically activated, Telegram bridges are
/// re-attached, and state is persisted.
#[allow(clippy::too_many_arguments)]
pub async fn restart_session_inner<R: tauri::Runtime>(
    app: &AppHandle<R>,
    session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
    pty_mgr: &Arc<Mutex<PtyManager>>,
    settings: &SettingsState,
    uuid: Uuid,
    agent_id: Option<String>,
    requested_profile: Option<String>,
    skip_auto_resume: Option<bool>,
) -> Result<SessionInfo, String> {
    restart_session_inner_with_activation(
        app,
        session_mgr,
        pty_mgr,
        settings,
        uuid,
        agent_id,
        requested_profile,
        skip_auto_resume,
        true,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub async fn restart_session_inner_with_activation<R: tauri::Runtime>(
    app: &AppHandle<R>,
    session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
    pty_mgr: &Arc<Mutex<PtyManager>>,
    settings: &SettingsState,
    uuid: Uuid,
    agent_id: Option<String>,
    requested_profile: Option<String>,
    skip_auto_resume: Option<bool>,
    activate_after: bool,
) -> Result<SessionInfo, String> {
    restart_session_inner_with_intent(
        app,
        session_mgr,
        pty_mgr,
        settings,
        uuid,
        agent_id,
        requested_profile,
        skip_auto_resume,
        activate_after,
        TrustedRestartIntent::User,
        None,
        crate::config::sessions_persistence::default_creation_gate_enforcement(),
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn restart_session_inner_with_intent<R: tauri::Runtime>(
    app: &AppHandle<R>,
    _session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
    _pty_mgr: &Arc<Mutex<PtyManager>>,
    _settings: &SettingsState,
    uuid: Uuid,
    agent_id: Option<String>,
    requested_profile: Option<String>,
    skip_auto_resume: Option<bool>,
    activate_after: bool,
    intent: TrustedRestartIntent,
    communication_override: Option<SessionCommunication>,
    enforcement: crate::config::sessions_persistence::CreationGateEnforcement,
) -> Result<SessionInfo, String> {
    let restarted = app
        .state::<SelectionCoordinator>()
        .restart_lifecycle(
            app.clone(),
            RestartJobRequest {
                session_id: uuid,
                agent_id,
                requested_profile,
                skip_auto_resume,
                activate_after,
                intent,
                communication_override,
                enforcement,
            },
        )
        .await?;

    // #2232 phase 4 section 5.2: the restart replaces the session UUID, so
    // `execute_restart_transaction` owns the reader lifecycle on both the
    // success and failure paths (release the old id, then raise the
    // replacement's own demands). Nothing reader-related is done here.

    Ok(restarted)
}

struct RestartTeardownError {
    message: String,
    old_lost: bool,
}

pub(crate) async fn complete_owned_replacement_retirement<R: tauri::Runtime>(
    transaction: &SelectionTransaction<R>,
    session_id: Uuid,
) {
    crate::session::selection::forget_response_close_registration(session_id);
    detach_restart_telegram(transaction, session_id).await;
    crate::commands::telegram::release_all_reader_demands(transaction.app(), session_id).await;
    transaction
        .app()
        .state::<DetachedSessionsState>()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .remove(&session_id);
}

async fn detach_restart_telegram<R: tauri::Runtime>(
    transaction: &SelectionTransaction<R>,
    session_id: Uuid,
) {
    let bridge_shutdown = {
        let telegram = transaction.app().state::<TelegramBridgeState>();
        let mut telegram = telegram.lock().await;
        if telegram.has_bridge(session_id) {
            telegram.detach(session_id).ok()
        } else {
            None
        }
    };
    if let Some(shutdown) = bridge_shutdown {
        if let Err(error) = transaction.app().emit(
            "telegram_bridge_detached",
            serde_json::json!({ "sessionId": session_id.to_string() }),
        ) {
            log::warn!(
                "[restart] telegram detach publication failed session={}: {}",
                session_id,
                error
            );
        }
        shutdown.spawn_wait_or_abort();
    }
}

async fn teardown_old_for_restart<R: tauri::Runtime>(
    transaction: &SelectionTransaction<R>,
    session_id: Uuid,
    old_was_dormant: bool,
) -> Result<bool, RestartTeardownError> {
    if !transaction.runtime_snapshot(session_id).has_pty {
        if old_was_dormant {
            return Ok(false);
        }
        transaction
            .app()
            .state::<DetachedSessionsState>()
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&session_id);
        detach_restart_telegram(transaction, session_id).await;
        // Section 5.2: the old runtime is confirmed gone, so its reader
        // demands are released before the replacement is created. A failure to
        // create the replacement therefore leaves no old reader behind.
        crate::commands::telegram::release_all_reader_demands(transaction.app(), session_id).await;
        return Ok(true);
    }

    let resource_monitor = transaction
        .app()
        .state::<Arc<ResourceMonitorState>>()
        .inner()
        .clone();
    if resource_monitor.has_registered_group(session_id) {
        transaction
            .app()
            .state::<Arc<Mutex<PtyManager>>>()
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .publish_stop_witness(session_id, "session-restart");
        let monitor = Arc::clone(&resource_monitor);
        match tokio::task::spawn_blocking(move || {
            monitor.kill_group(
                session_id,
                crate::resource_monitor::ResourceKillReason::SessionDestroy,
            )
        })
        .await
        {
            Ok(Ok(result)) if result.quarantined => log::warn!(
                "[restart] resource cleanup quarantined session={}: {}",
                session_id,
                result.message
            ),
            Ok(Ok(_)) => {}
            Ok(Err(error)) => log::warn!(
                "[restart] resource cleanup failed session={}: {}",
                session_id,
                error
            ),
            Err(error) => log::warn!(
                "[restart] resource cleanup task failed session={}: {}",
                session_id,
                error
            ),
        }
    }

    // The std-Mutex guard from this `let` initializer is dropped at the `;` before
    // `runtime_snapshot` re-locks the same pty Mutex below. Moving this into an
    // `if let`/`match` scrutinee or an inner blocking block would hold the guard
    // across that re-lock and re-introduce the re-entrant std-Mutex deadlock fixed
    // in resource_monitor (§1295).
    let kill_result = transaction
        .app()
        .state::<Arc<Mutex<PtyManager>>>()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .kill(session_id);
    let old_lost = !transaction.runtime_snapshot(session_id).has_pty;
    if !old_lost {
        let message = kill_result
            .err()
            .map(|error| error.to_string())
            .unwrap_or_else(|| "old PTY remained live after restart teardown".to_string());
        return Err(RestartTeardownError {
            message,
            old_lost: false,
        });
    }
    if let Err(error) = kill_result {
        log::warn!(
            "[restart] teardown reported an error after PTY loss session={}: {}",
            session_id,
            error
        );
    }
    transaction
        .app()
        .state::<DetachedSessionsState>()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .remove(&session_id);
    detach_restart_telegram(transaction, session_id).await;
    // Section 5.2: old runtime loss is confirmed, so the old id's reader
    // demands are released before the replacement is created.
    crate::commands::telegram::release_all_reader_demands(transaction.app(), session_id).await;
    Ok(true)
}

/// #1171 - this covers BOTH restart paths: `execute_restart_transaction`'s success path and
/// `finalize_failed_restart`. A restart is a normal flow - it is in the invoke handler, the
/// mailbox calls it, and a bulk settings save can restart sessions - so without the purge here
/// every restart would orphan up to 500 ring entries under an id that is already gone from the
/// session list, unreachable from the UI.
fn publish_restart_destroyed<R: tauri::Runtime>(
    transaction: &SelectionTransaction<R>,
    session_id: Uuid,
) {
    transaction.publish_destroyed(session_id);
    let label = format!("terminal-{}", session_id.to_string().replace('-', ""));
    if let Some(window) = transaction.app().get_webview_window(&label) {
        if let Err(error) = window.destroy() {
            log::warn!(
                "[restart] detached window destroy failed session={}: {}",
                session_id,
                error
            );
        }
    }
    purge_session_side_state(transaction.app(), session_id);
}

async fn finalize_failed_restart<R: tauri::Runtime>(
    transaction: &SelectionTransaction<R>,
    session_id: Uuid,
    selected: bool,
    intent: TrustedRestartIntent,
) -> Result<(), String> {
    let publication = transaction
        .start_lifecycle_effect(crate::session::selection::LifecycleEffectSlot::Publication);
    if transaction.has_lifecycle_entry() && publication.is_none() {
        return Err(crate::session::selection::SelectionCoordinatorError::Unavailable.to_string());
    }
    let result = async {
        detach_restart_telegram(transaction, session_id).await;
        let mut mutations = LifecycleMutations::default();
        mutations.remove(session_id);
        let committed = transaction
            .commit(
                if selected {
                    CommitDecision::Clear
                } else {
                    CommitDecision::Keep
                },
                SelectionCause::Restart(intent),
                mutations,
            )
            .await?;
        // Section 5.2: the old row is gone, so any old-id reader demand that
        // survived teardown (or was raised while it was in flight) is released and
        // its capture slot closed before the destroyed publication.
        crate::commands::telegram::release_all_reader_demands(transaction.app(), session_id).await;
        transaction
            .persist(SelectionSource::Restart, Some(session_id))
            .await;
        publish_restart_destroyed(transaction, session_id);
        if let Some(selection) = committed.selection.as_ref() {
            transaction.publish_selection(selection);
        }
        Ok(())
    }
    .await;
    if let Some(publication) = publication {
        publication.complete(result.as_ref().map(|_| None).map_err(Clone::clone));
    }
    result
}

enum LifecycleExecutionContext<'a> {
    ExternalEntry(&'a LifecycleEntryAuthority),
    RestoreEntry {
        authority: &'a LifecycleEntryAuthority,
        root_directory: Option<&'a str>,
    },
}
impl LifecycleExecutionContext<'_> {
    fn authority(&self) -> Option<&LifecycleEntryAuthority> {
        match self {
            Self::ExternalEntry(authority) => Some(authority),
            Self::RestoreEntry { authority, .. } => Some(authority),
        }
    }
}

#[expect(
    dead_code,
    reason = "P05 callers use guarded prepared paths; preserve this compatibility facade"
)]
pub(crate) async fn execute_restart_transaction<R: tauri::Runtime>(
    transaction: &SelectionTransaction<R>,
    request: RestartJobRequest,
) -> Result<SessionInfo, String> {
    if !transaction.has_active_restore_capability() {
        return Err(crate::session::selection::SelectionCoordinatorError::Unavailable.to_string());
    }
    let (authority, root_directory) =
        prepare_restore_lifecycle_authority(transaction, Some(request.session_id), false).await?;
    execute_restart_transaction_in_context(
        transaction,
        request,
        &LifecycleExecutionContext::RestoreEntry {
            authority: &authority,
            root_directory: root_directory.as_deref(),
        },
    )
    .await
}

#[expect(
    clippy::large_enum_variant,
    reason = "Preserve lifecycle result and Interrupted dispatch ownership without new allocation"
)]
pub(crate) enum LifecycleEntryOutcome {
    Returned(Result<SessionInfo, String>),
    Interrupted,
}

pub(crate) async fn execute_external_restart_transaction<R: tauri::Runtime>(
    transaction: &SelectionTransaction<R>,
    request: RestartJobRequest,
    authority: &LifecycleEntryAuthority,
) -> LifecycleEntryOutcome {
    let result = execute_restart_transaction_in_context(
        transaction,
        request,
        &LifecycleExecutionContext::ExternalEntry(authority),
    )
    .await;
    if authority
        .interrupted
        .load(std::sync::atomic::Ordering::Acquire)
    {
        LifecycleEntryOutcome::Interrupted
    } else {
        LifecycleEntryOutcome::Returned(result)
    }
}

async fn accept_restart_teardown<R: tauri::Runtime>(
    transaction: &SelectionTransaction<R>,
    teardown: Result<bool, RestartTeardownError>,
    uuid: uuid::Uuid,
    old_selected: bool,
    intent: TrustedRestartIntent,
) -> Result<bool, String> {
    match teardown {
        Ok(old_lost) => Ok(old_lost),
        Err(error) => {
            if error.old_lost {
                // Section 5.2: the old PTY is already gone, so its demands must
                // not outlive the failed restart.
                crate::commands::telegram::release_all_reader_demands(transaction.app(), uuid)
                    .await;
                if let Err(finalize_error) =
                    finalize_failed_restart(transaction, uuid, old_selected, intent).await
                {
                    return Err(format!(
                        "{}; restart failure finalization failed: {}",
                        error.message, finalize_error
                    ));
                }
            }
            Err(error.message)
        }
    }
}

async fn execute_restart_transaction_in_context<R: tauri::Runtime>(
    transaction: &SelectionTransaction<R>,
    request: RestartJobRequest,
    context: &LifecycleExecutionContext<'_>,
) -> Result<SessionInfo, String> {
    if matches!(context, LifecycleExecutionContext::RestoreEntry { .. })
        && !transaction.has_active_restore_capability()
    {
        return Err(crate::session::selection::SelectionCoordinatorError::Unavailable.to_string());
    }
    let app = transaction.app();
    let session_mgr = app
        .state::<Arc<tokio::sync::RwLock<SessionManager>>>()
        .inner()
        .clone();
    let pty_mgr = app.state::<Arc<Mutex<PtyManager>>>().inner().clone();
    let settings = app.state::<SettingsState>().inner().clone();
    let RestartJobRequest {
        session_id: uuid,
        agent_id,
        requested_profile,
        skip_auto_resume,
        activate_after,
        intent,
        communication_override,
        enforcement,
    } = request;
    // 1. Read config from existing session BEFORE destroying it
    let (
        shell,
        shell_args,
        cwd,
        name,
        stored_agent_id,
        stored_agent_label,
        git_repos,
        is_root_agent,
        telegram_bot_id,
        stored_requested_profile,
        stored_start_fresh,
        stored_communication,
        old_status,
        old_selected,
    ) = {
        let mgr = session_mgr.read().await;
        let session = match mgr.get_session(uuid).await {
            Some(session) => session,
            None => {
                transaction.observe_lifecycle_missing_source();
                return Err("Session not found".to_string());
            }
        };
        let old_selected = mgr.selection_payload().await.id() == Some(uuid);
        (
            session.shell.clone(),
            session.shell_args.clone(),
            session.working_directory.clone(),
            session.name.clone(),
            session.agent_id.clone(),
            session.agent_label.clone(),
            session.git_repos.clone(),
            session.is_root_agent
                || crate::config::root_agent::is_root_agent_path(&session.working_directory),
            session.telegram_bot_id.clone(),
            session.requested_profile.clone(),
            session.start_fresh_on_restore, // (#630/#631) honor an unconsumed durable fresh intent
            session.communication.clone(),  // (#747) candidate raised-hand carry across the reopen
            session.status.clone(),
            old_selected,
        )
    };

    let preparation = transaction
        .start_lifecycle_effect(crate::session::selection::LifecycleEffectSlot::ContextPreparation);
    if transaction.has_lifecycle_entry() && preparation.is_none() {
        if let Some(authority) = context.authority() {
            authority
                .interrupted
                .store(true, std::sync::atomic::Ordering::Release);
        }
        return Err(crate::session::selection::SelectionCoordinatorError::Unavailable.to_string());
    }
    let prepared = async {
        let cwd = if is_root_agent {
            ensure_lifecycle_root_directory(transaction, context).await?
        } else {
            cwd
        };
        let cwd = crate::path_utils::normalize_windows_verbatim_path(&cwd);
        crate::config::archive_gate::probe_spawn_refusal(app, &cwd).await?;
        // §1295 5.1b / B2: the creation gate runs AFTER `probe_spawn_refusal` but
        // WELL BEFORE `teardown_old_for_restart` (below). A rejected restart of a
        // live outside-roots session therefore DECLINES: the old session stays
        // running (live PTY untouched), `Err(sessionCreateBlocked: ...)` propagates,
        // and no `finalize_failed_restart` runs. Root-agent restarts already
        // normalized `cwd` to `ensure_root_agent_dir()` above, which the gate
        // exempts by rule 1.
        crate::config::sessions_persistence::enforce_creation_gate(app, &cwd, enforcement).await?;

        // 2. Strip auto-injected args before restart so the new session starts from the saved
        // recipe.
        let clean_args =
            crate::config::sessions_persistence::strip_auto_injected_args(&shell, &shell_args);

        let requested_agent_id = agent_id;
        let selected_requested_profile =
            effective_restart_requested_profile(requested_profile, stored_requested_profile);
        // #537 read-side / #2434: resolve the launch agent through the one matched
        // selection drift also reads, and resolve its spawn under a single settings
        // read guard. No await is held across the guard; it is dropped at the end of
        // this block. The #1271 host-shell snapshot is copied from the SAME guard so
        // program and args can never pair across a configuration change.
        let (selected_agent_id, resolved_spawn, resolved_agent_host_shell) = {
            let cfg = settings
                .current_read()
                .await
                .map_err(|error| error.to_string())?;
            let selection = matched_selection(
                &cfg,
                &cwd,
                requested_agent_id.as_deref(),
                stored_agent_id.as_deref(),
                selected_requested_profile.as_deref(),
            );
            let (selected_agent_id, resolved_spawn) = match selection {
                MatchedSelection::NoAgent => (None, None),
                MatchedSelection::Unresolved { reference } => {
                    return Err(format!(
                    "unresolved_coding_agent_reference: '{}' matches no configured coding agent",
                    reference
                ));
                }
                MatchedSelection::Agent {
                    agent_id,
                    profile_letter,
                    authoritative,
                    tier,
                    original_letter,
                } => {
                    let spawn = resolve_configured_agent_spawn_for_cwd(
                        &cfg,
                        &agent_id,
                        &cwd,
                        profile_letter.as_deref(),
                        authoritative,
                    )?
                    .map(|mut spawn| {
                        // A matched restart records the ORIGINAL request; the matched
                        // letter lives in `effective_profile`, its provenance in the
                        // #2451 carriers.
                        if authoritative {
                            if let Some(original) = selected_requested_profile.as_ref() {
                                spawn.profile_resolution.requested_profile = original.clone();
                            }
                        }
                        spawn.match_tier = tier.map(|tier| tier.as_wire_str().to_string());
                        spawn.original_profile_letter = original_letter;
                        spawn
                    });
                    (Some(agent_id), spawn)
                }
            };
            let resolved_agent_host_shell = if resolved_spawn.is_some() {
                Some(ResolvedAgentHostShell {
                    program: cfg.default_shell.clone(),
                    args: cfg.default_shell_args.clone(),
                })
            } else {
                None
            };
            (selected_agent_id, resolved_spawn, resolved_agent_host_shell)
        };
        // #2434 - prepare (isolated CODEX_HOME, OpenCode config dir) outside the
        // settings guard and BEFORE any teardown: a failure aborts the restart and
        // leaves the old session running.
        if let Some(spawn) = resolved_spawn.as_ref() {
            crate::config::agent_command::prepare_agent_spawn_command(spawn)?;
        }
        let (shell, shell_args, agent_label) = if let Some(spawn) = resolved_spawn.as_ref() {
            (
                spawn.shell.clone(),
                spawn.shell_args.clone(),
                Some(spawn.trusted_agent_label.clone()),
            )
        } else {
            (shell, clean_args, stored_agent_label)
        };

        // (#630/#631) Fresh on an explicit "Restart Session" (None -> true), OR if the
        // session still carries an unconsumed durable fresh intent. Root agents are
        // scoped out (separate restore path ignores the marker, §8): they are never
        // stamped below, so `stored_start_fresh` is false for root and this reduces to
        // today's `effective_restart_skip_auto_resume`.
        let restart_start_fresh =
            restart_skip_auto_resume_with_intent(stored_start_fresh, skip_auto_resume);
        let carried_communication = communication_override
            .or_else(|| carry_communication_for_restart(stored_communication, restart_start_fresh));

        Ok::<_, String>((
            shell,
            shell_args,
            cwd,
            selected_agent_id,
            agent_label,
            resolved_spawn,
            resolved_agent_host_shell,
            restart_start_fresh,
            carried_communication,
        ))
    }
    .await;
    if let Some(preparation) = preparation {
        preparation.complete(prepared.as_ref().map(|_| None).map_err(Clone::clone));
    }
    let (
        shell,
        shell_args,
        cwd,
        selected_agent_id,
        agent_label,
        resolved_spawn,
        resolved_agent_host_shell,
        restart_start_fresh,
        carried_communication,
    ) = match prepared {
        Ok(prepared) => prepared,
        Err(error) => {
            transaction.observe_lifecycle_preparation_failure();
            return Err(error);
        }
    };

    let retirement = transaction
        .start_lifecycle_effect(crate::session::selection::LifecycleEffectSlot::Retirement);
    if transaction.has_lifecycle_entry() && retirement.is_none() {
        if let Some(authority) = context.authority() {
            authority
                .interrupted
                .store(true, std::sync::atomic::Ordering::Release);
        }
        return Err(crate::session::selection::SelectionCoordinatorError::Unavailable.to_string());
    }
    if let Some(authority) = context.authority() {
        if !retire_lifecycle_entry_protection(app, authority).await {
            transaction.invalidate_restore_ownership();
            authority
                .interrupted
                .store(true, std::sync::atomic::Ordering::Release);
            // External adapters suppress this internal exit via Interrupted.
            // Restore unwinds its actual executor/barrier with Unavailable.
            return Err(
                crate::session::selection::SelectionCoordinatorError::Unavailable.to_string(),
            );
        }
    }

    // 3. Release the old runtime slot while retaining its canonical manager row.
    // Dormant rows skip teardown and remain retryable until replacement success.
    let old_was_dormant = matches!(old_status, SessionStatus::Exited(_));
    let teardown = teardown_old_for_restart(transaction, uuid, old_was_dormant).await;
    if let Some(retirement) = retirement {
        retirement.complete(match &teardown {
            Ok(old_lost) => Ok(Some(format!("old_lost={old_lost}"))),
            Err(error) => Err(format!("old_lost={}: {}", error.old_lost, error.message)),
        });
    }
    let old_lost =
        accept_restart_teardown(transaction, teardown, uuid, old_selected, intent).await?;

    // Test-only rendezvous after the old id's early release and before the
    // replacement is created (phase 4 test 9).
    #[cfg(test)]
    restart_reader_seam::hit_after_old_release(&uuid.to_string()).await;

    // 4. Spawn the replacement as a pending, unannounced row.
    let binding_manager = session_mgr.read().await.clone();
    let coding_agent_binding = binding_manager.coding_agent_binding(uuid).await;
    let target_ownership = context
        .authority()
        .map(|authority| authority.target_ownership())
        .transpose()?
        .flatten();
    let completion = create_session_inner_impl(
        app,
        &session_mgr,
        &pty_mgr,
        shell,
        shell_args,
        cwd.clone(),
        Some(name),
        selected_agent_id,
        agent_label,
        false,
        git_repos,
        restart_start_fresh,
        resolved_spawn,
        resolved_agent_host_shell,
        None,
        CreateSelectionIntent::Suppress,
        (!is_root_agent).then_some(restart_start_fresh),
        carried_communication,
        Some(transaction),
        Some(SelectionCause::Restart(intent)),
        true,
        target_ownership.as_ref(),
        enforcement,
        coding_agent_binding,
        context.authority(),
        None,
        None,
    )
    .await;
    let deferred = accept_restart_deferred_completion(
        transaction,
        completion,
        context,
        uuid,
        old_lost,
        old_selected,
        intent,
    )
    .await?;
    let (new_uuid, committed, session_info) = finalize_restart_deferred(
        transaction,
        &deferred,
        uuid,
        old_lost,
        old_selected,
        activate_after,
        intent,
        restart_start_fresh,
    )
    .await?;
    let publication = publish_restart_before_warnings(
        transaction,
        uuid,
        new_uuid,
        deferred.binding,
        &committed,
        &session_info,
        old_lost,
    )
    .await?;
    for warning in deferred.warnings {
        emit_session_warning(app, warning);
    }

    publish_restart_after_warnings(
        transaction,
        new_uuid,
        session_info,
        restart_start_fresh,
        telegram_bot_id.as_deref(),
        &cwd,
        publication,
    )
    .await
}

// Tauri command: State<> injections push us over clippy's 7-arg threshold.
#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub async fn restart_session(
    app: AppHandle,
    session_mgr: State<'_, Arc<tokio::sync::RwLock<SessionManager>>>,
    pty_mgr: State<'_, Arc<Mutex<PtyManager>>>,
    _tg_mgr: State<'_, TelegramBridgeState>,
    settings: State<'_, SettingsState>,
    id: String,
    agent_id: Option<String>,
    requested_profile: Option<String>,
    skip_auto_resume: Option<bool>,
) -> Result<SessionInfo, String> {
    let uuid = Uuid::parse_str(&id).map_err(|e| e.to_string())?;
    restart_session_inner(
        &app,
        session_mgr.inner(),
        pty_mgr.inner(),
        settings.inner(),
        uuid,
        agent_id,
        requested_profile,
        skip_auto_resume,
    )
    .await
}

#[tauri::command]
pub async fn switch_session(
    app: AppHandle,
    _session_mgr: State<'_, Arc<tokio::sync::RwLock<SessionManager>>>,
    detached: State<'_, DetachedSessionsState>,
    id: String,
) -> Result<(), String> {
    let uuid = Uuid::parse_str(&id).map_err(|e| e.to_string())?;

    // If this session is detached, focus its window instead of switching the main terminal
    let is_detached = {
        let detached_set = detached.lock().unwrap();
        detached_set.contains(&uuid)
    };
    if is_detached {
        let label = format!("terminal-{}", id.replace('-', ""));
        if let Some(win) = app.get_webview_window(&label) {
            let _ = win.set_focus();
        }
        return Ok(());
    }

    let coordinator = app
        .try_state::<SelectionCoordinator>()
        .ok_or_else(|| "selectionCoordinatorUnavailable".to_string())?;
    coordinator
        .transition(SelectionRequest::user_switch(uuid))
        .await?;

    Ok(())
}

#[tauri::command]
pub async fn rename_session(
    app: AppHandle,
    session_mgr: State<'_, Arc<tokio::sync::RwLock<SessionManager>>>,
    id: String,
    name: String,
) -> Result<(), String> {
    let uuid = Uuid::parse_str(&id).map_err(|e| e.to_string())?;

    let mgr = session_mgr.read().await;
    mgr.rename_session(uuid, name.clone())
        .await
        .map_err(|e| e.to_string())?;

    // Persist after rename
    persist_current_state(&mgr).await;

    let _ = app.emit(
        "session_renamed",
        serde_json::json!({ "id": id, "name": name }),
    );

    Ok(())
}

#[tauri::command]
pub async fn list_sessions(
    session_mgr: State<'_, Arc<tokio::sync::RwLock<SessionManager>>>,
    settings: State<'_, SettingsState>,
) -> Result<Vec<SessionInfo>, String> {
    let mut infos = { session_mgr.read().await.list_sessions().await };
    // #592 - recompute drift per session against current settings (settings-aware,
    // unlike the `From<&Session>` path which always emits `profile_outdated=false`).
    let cfg = settings
        .current_read()
        .await
        .map_err(|error| error.to_string())?;
    for info in infos.iter_mut() {
        info.profile_outdated = compute_profile_outdated(&cfg, info);
    }
    Ok(infos)
}

#[tauri::command]
pub async fn set_last_prompt(
    app: AppHandle,
    session_mgr: State<'_, Arc<tokio::sync::RwLock<SessionManager>>>,
    id: String,
    text: String,
) -> Result<(), String> {
    let uuid = Uuid::parse_str(&id).map_err(|e| e.to_string())?;
    let mgr = session_mgr.read().await;
    mgr.set_last_prompt(uuid, text.clone()).await;
    crate::config::sessions_persistence::persist_current_state(&mgr).await;
    let _ = app.emit(
        "last_prompt",
        serde_json::json!({ "sessionId": id, "text": text }),
    );
    Ok(())
}

/// A reused Root is already public: serialize the absent-only seed and event
/// with every external prompt setter, then persist after releasing the permit.
pub(crate) async fn seed_root_last_prompt<R: tauri::Runtime>(
    app: &AppHandle<R>,
    session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
    id: Uuid,
    persisted: &crate::config::sessions_persistence::PersistedSession,
) {
    if persisted.start_fresh_on_restore {
        return;
    }
    let Some(prompt) = persisted.last_prompt.clone() else {
        return;
    };
    let manager = session_mgr.read().await;
    crate::config::sessions_persistence::initialize_prompt_retention(&manager).await;
    let Some(row) = manager.get_session(id).await else {
        return;
    };
    if !InitialLastPrompt::from_persisted(persisted).compatible(&row) {
        return;
    }
    let key = crate::config::sessions_persistence::prompt_key(
        &row.working_directory,
        row.agent_id.as_deref(),
        row.requested_profile.as_deref(),
        row.effective_profile.as_deref(),
    );
    let verified = crate::config::sessions_persistence::prompt_key(
        &row.working_directory,
        row.agent_id.as_deref(),
        row.requested_profile.as_deref(),
        row.effective_profile.as_deref(),
    );
    let Some(key) = key.filter(|key| Some(key) == verified.as_ref()) else {
        return;
    };
    let permit = crate::session::manager::acquire_carrier_mutation_permit().await;
    let changed = manager
        .seed_if_absent_with_carrier_permit(&permit, id, &key, prompt.clone())
        .await;
    if changed {
        let _ = app.emit(
            "last_prompt",
            serde_json::json!({ "sessionId": id.to_string(), "text": prompt }),
        );
    }
    drop(permit);
    if changed {
        crate::config::sessions_persistence::persist_current_state(&manager).await;
    }
}

/// Extract the basename (without extension) from a path or command token.
pub(crate) fn executable_basename(s: &str) -> String {
    std::path::Path::new(s)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(s)
        .to_lowercase()
}

type ResolvedRootAgentCommand = (String, Vec<String>, Option<String>, Option<String>);

#[cfg(test)]
fn resolve_agent_command(
    agent_id: &str,
    settings: &AppSettings,
) -> Result<(String, Vec<String>, Option<String>), String> {
    if let Some(agent) = settings.agents.iter().find(|a| a.id == agent_id) {
        log::info!(
            "[session] Agent resolved: id={:?}, label={:?}, command={:?}",
            agent.id,
            agent.label,
            agent.command
        );
        let source = format!("selected agent '{}'", agent_id);
        let (shell, shell_args) = normalize_agent_command_for_source(agent, &source)?;
        Ok((shell, shell_args, Some(agent.label.clone())))
    } else {
        log::warn!(
            "[session] Agent NOT found for aid={:?}. Falling back to default shell.",
            agent_id
        );
        Ok((
            settings.default_shell.clone(),
            settings.default_shell_args.clone(),
            None,
        ))
    }
}

fn normalize_agent_command_for_source(
    agent: &crate::config::settings::AgentConfig,
    source: &str,
) -> Result<(String, Vec<String>), String> {
    crate::config::agent_command::normalize_legacy_agent_command(&agent.command)
        .map(|cmd| (cmd.shell, cmd.shell_args))
        .map_err(|e| {
            format!(
                "Invalid agent command from {} (agent id '{}', label '{}'): {}. command={:?}",
                source, agent.id, agent.label, e, agent.command
            )
        })
}

pub(crate) fn resolve_root_agent_command(
    settings: &AppSettings,
    requested_agent_id: Option<&str>,
    last_coding_agent: Option<&str>,
) -> Result<ResolvedRootAgentCommand, String> {
    let resolve_configured =
        |agent_id: &str| settings.agents.iter().find(|agent| agent.id == agent_id);

    if let Some(agent_id) = requested_agent_id {
        if let Some(agent) = resolve_configured(agent_id) {
            let source = format!("requested root agent '{}'", agent_id);
            let (shell, shell_args) = normalize_agent_command_for_source(agent, &source)?;
            return Ok((
                shell,
                shell_args,
                Some(agent.id.clone()),
                Some(agent.label.clone()),
            ));
        } else {
            log::warn!(
                "[root-agent] Requested coding agent '{}' no longer exists; falling back",
                agent_id
            );
        }
    }

    if let Some(agent_id) = last_coding_agent {
        if let Some(agent) = resolve_configured(agent_id) {
            let source = format!("root lastCodingAgent '{}'", agent_id);
            let (shell, shell_args) = normalize_agent_command_for_source(agent, &source)?;
            return Ok((
                shell,
                shell_args,
                Some(agent.id.clone()),
                Some(agent.label.clone()),
            ));
        } else {
            log::warn!(
                "[root-agent] lastCodingAgent '{}' no longer exists; falling back",
                agent_id
            );
        }
    }

    if let Some(agent) = settings.agents.first() {
        let source = format!("first configured root agent '{}'", agent.id);
        let (shell, shell_args) = normalize_agent_command_for_source(agent, &source)?;
        return Ok((
            shell,
            shell_args,
            Some(agent.id.clone()),
            Some(agent.label.clone()),
        ));
    }

    // #2716 (B3, option C): an unreadable agents layer is not "none configured".
    if let Some(path) = settings.agents_layer.unreadable_path() {
        return Err(crate::config::settings::agents_unreadable_session_error(
            path,
        ));
    }
    Err(
        "No resolvable coding agent is configured for the Root Agent. \
         Configure a coding agent before launching the Root Agent."
            .to_string(),
    )
}

fn resolve_agent_label(agent_id: &str, settings: &AppSettings) -> Option<String> {
    settings
        .agents
        .iter()
        .find(|a| a.id == agent_id)
        .map(|a| a.label.clone())
}

/// #2433 / #2450 seam - the enabled profile cells of `agent_id`. The single
/// accessor for reading an agent's cells here; #2450 re-keys the map and changes
/// only this body.
fn enabled_profile_cells<'a>(
    settings: &'a AppSettings,
    agent_id: &str,
) -> impl Iterator<Item = (&'a String, &'a crate::config::settings::ProfileCellConfig)> + 'a {
    settings
        .coding_agent_profiles
        .profiles_by_agent
        .get(agent_id)
        .into_iter()
        .flat_map(|cells| cells.iter())
        .filter(|(_, cell)| cell.enabled)
}

/// #2433 - `(canonical command, enabled letter -> cell identity)` for `agent_id`,
/// written beside its `codingAgents` entry. `None` when the agent is absent from
/// settings: nothing is known, so no descriptor is written (an empty identity
/// would read as a real agent with zero enabled cells).
fn coding_agent_descriptor(
    agent_id: &str,
    settings: &AppSettings,
) -> Option<(String, std::collections::BTreeMap<String, String>)> {
    let agent = settings.agents.iter().find(|a| a.id == agent_id)?;
    let identity = enabled_profile_cells(settings, agent_id)
        .map(|(letter, cell)| {
            (
                letter.clone(),
                crate::config::agent_command::cell_identity(agent, cell),
            )
        })
        .collect();
    Some((
        crate::config::agent_command::canonical_command_text(&agent.command),
        identity,
    ))
}

/// #2433 - everything the spawn-time `codingAgents` write needs, resolved from
/// settings for the launched agent only.
struct LastCodingAgentWrite {
    agent_id: String,
    label: String,
    descriptor: Option<(String, std::collections::BTreeMap<String, String>)>,
}

/// Resolve the label (the provided one, else settings, else "Unknown") and the
/// descriptor for `agent_id`. Without the label fallback, callers that pass an
/// agent id but no label (session-requests, web remote) would write
/// app: "Unknown" into the per-instance config.json.
fn prepare_last_coding_agent_write(
    agent_id: &str,
    agent_label: Option<&str>,
    settings: &AppSettings,
) -> LastCodingAgentWrite {
    let label = match agent_label {
        Some(l) => l.to_string(),
        None => resolve_agent_label(agent_id, settings).unwrap_or_else(|| {
            log::warn!(
                "Could not resolve label for agent_id='{}' — defaulting to 'Unknown'",
                agent_id
            );
            "Unknown".to_string()
        }),
    };
    LastCodingAgentWrite {
        agent_id: agent_id.to_string(),
        label,
        descriptor: coding_agent_descriptor(agent_id, settings),
    }
}

/// The spawn-time write of `lastCodingAgent` + `codingAgents[<id>]`.
fn save_last_coding_agent(
    cwd: &str,
    session_id: &str,
    write: &LastCodingAgentWrite,
) -> Result<(), String> {
    agent_config::set_last_coding_agent(
        cwd,
        &write.agent_id,
        &write.label,
        Some(session_id),
        write
            .descriptor
            .as_ref()
            .map(|(command, identity)| (command.as_str(), identity)),
    )
}

fn resolve_actual_agent(
    shell: &str,
    shell_args: &[String],
    requested_agent_id: Option<&str>,
    requested_agent_label: Option<&str>,
    settings: &AppSettings,
) -> (Option<String>, Option<String>) {
    if let Some(agent_id) = requested_agent_id {
        if let Some(agent) = settings.agents.iter().find(|a| a.id == agent_id) {
            match crate::config::agent_command::normalize_legacy_agent_command(&agent.command) {
                Ok(normalized)
                    if normalized.shell == shell && normalized.shell_args == shell_args =>
                {
                    return (
                        Some(agent.id.clone()),
                        requested_agent_label
                            .map(ToString::to_string)
                            .or_else(|| Some(agent.label.clone())),
                    );
                }
                Ok(_) => {}
                Err(e) => {
                    log::debug!(
                        "[session] Requested agent_id='{}' command did not normalize during \
                         metadata resolution: {}",
                        agent_id,
                        e
                    );
                }
            }
        }
    }

    let detected = resolve_agent_from_shell(shell, shell_args, settings);

    if let Some(agent_id) = requested_agent_id {
        match detected.0.as_deref() {
            Some(detected_id) if detected_id == agent_id => {
                return (
                    detected.0,
                    requested_agent_label
                        .map(ToString::to_string)
                        .or(detected.1),
                )
            }
            Some(detected_id) => {
                log::warn!(
                    "[session] Requested agent_id='{}' did not match final shell-resolved agent \
                     '{}'; storing resolved agent instead",
                    agent_id,
                    detected_id
                );
                return detected;
            }
            None => {
                log::warn!(
                    "[session] Requested agent_id='{}' did not validate against final launched \
                     shell; clearing actual agent metadata",
                    agent_id
                );
                return (None, None);
            }
        }
    }

    detected
}

/// Try to match the shell command against configured agents in settings.
/// Returns (Some(agent_id), Some(label)) if a match is found, (None, None) otherwise.
fn resolve_agent_from_shell(
    shell: &str,
    shell_args: &[String],
    settings: &AppSettings,
) -> (Option<String>, Option<String>) {
    // Compare against already-normalized launch tokens without re-splitting shell.
    let mut cmd_basenames: Vec<String> = Vec::with_capacity(shell_args.len() + 1);
    cmd_basenames.push(executable_basename(shell));
    cmd_basenames.extend(shell_args.iter().map(|arg| executable_basename(arg)));

    for agent in &settings.agents {
        let normalized =
            match crate::config::agent_command::normalize_legacy_agent_command(&agent.command) {
                Ok(cmd) => cmd,
                Err(e) => {
                    log::debug!(
                    "[session] Skipping agent '{}' during shell auto-detection because its command \
                     is invalid: {}",
                    agent.id,
                    e
                );
                    continue;
                }
            };
        let agent_basename = executable_basename(&normalized.shell);
        if !agent_basename.is_empty() && cmd_basenames.contains(&agent_basename) {
            log::info!(
                "Auto-detected agent '{}' ({}) from shell command",
                agent.id,
                agent.label
            );
            return (Some(agent.id.clone()), Some(agent.label.clone()));
        }
    }
    (None, None)
}

#[tauri::command]
pub async fn get_active_session(
    coordinator: State<'_, SelectionCoordinator>,
) -> Result<crate::session::selection::SessionSelection, String> {
    coordinator.snapshot().await
}

#[derive(Debug, Clone)]
pub(crate) struct RootJobRequest {
    pub requested_agent_id: Option<String>,
    pub requested_profile: Option<String>,
    pub skip_auto_resume_for_new_session: bool,
    pub intent: TrustedCreateIntent,
    pub select_after: bool,
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn create_root_agent_inner(
    app: &AppHandle,
    session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
    pty_mgr: &Arc<Mutex<PtyManager>>,
    tg_mgr: &TelegramBridgeState,
    settings: &SettingsState,
    requested_agent_id: Option<String>,
    requested_profile: Option<String>,
    skip_auto_resume_for_new_session: bool,
) -> Result<SessionInfo, String> {
    create_root_agent_inner_with_intent(
        app,
        session_mgr,
        pty_mgr,
        tg_mgr,
        settings,
        requested_agent_id,
        requested_profile,
        skip_auto_resume_for_new_session,
        TrustedCreateIntent::User,
        true,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn create_root_agent_inner_with_intent(
    app: &AppHandle,
    _session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
    _pty_mgr: &Arc<Mutex<PtyManager>>,
    _tg_mgr: &TelegramBridgeState,
    _settings: &SettingsState,
    requested_agent_id: Option<String>,
    requested_profile: Option<String>,
    skip_auto_resume_for_new_session: bool,
    intent: TrustedCreateIntent,
    select_after: bool,
) -> Result<SessionInfo, String> {
    app.state::<SelectionCoordinator>()
        .root_lifecycle(
            app.clone(),
            RootJobRequest {
                requested_agent_id,
                requested_profile,
                skip_auto_resume_for_new_session,
                intent,
                select_after,
            },
        )
        .await
}

struct LifecycleSourceIdentity {
    id: Uuid,
    token: Uuid,
    version: u64,
    saved_cwd: String,
    directory: crate::path_identity::VerifiedPathIdentity,
    replica: Option<crate::path_identity::VerifiedPathIdentity>,
}

enum LifecycleTargetLease {
    Keyed {
        key: String,
        destination: crate::path_identity::VerifiedPathIdentity,
        gate: Arc<crate::api::message_store::PtyInputTargetGate>,
        exact: crate::api::message_store::PtyInputTargetGuard,
        stripe: crate::api::message_store::PtyInputStripeGuard,
    },
    VerifiedNoReplica {
        destination: crate::path_identity::VerifiedPathIdentity,
    },
    MissingSource {
        requested: Uuid,
    },
}

pub(crate) struct LifecycleEntryAuthority {
    lease: LifecycleTargetLease,
    source: Option<LifecycleSourceIdentity>,
    root_request: bool,
    operation: Uuid,
    reservation: Mutex<Option<crate::pty::manager::CreateOwnerReservation>>,
    interrupted: std::sync::atomic::AtomicBool,
    discard: Option<crate::config::sessions_persistence::PublishedCarrier>,
    tracker: Option<crate::pty::manager::ResponseCloseState>,
}

impl std::fmt::Debug for LifecycleEntryAuthority {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LifecycleEntryAuthority")
            .field("operation", &self.operation)
            .finish_non_exhaustive()
    }
}

#[expect(
    clippy::large_enum_variant,
    reason = "Preserve the same lifecycle lease and reservation authority and its drop boundaries"
)]
pub(crate) enum LifecycleAuthorityAdmission {
    Ready(LifecycleEntryAuthority),
    Waiting,
    NeedsRootDirectory,
}

impl LifecycleEntryAuthority {
    pub(crate) fn release(self) -> bool {
        let claim = match self.reservation.lock() {
            Ok(mut claim) => claim.take(),
            Err(_) => return false,
        };
        let claimed = claim.is_some();
        let tracker = self.tracker.clone();
        drop(claim);
        drop(self);
        // Drop's with_inner error permanently latches tracker failure. A
        // positive health query after actual Drop proves that release;
        // false/unknown keeps this entry unsafe without inspecting newer claims.
        !claimed || tracker.is_some_and(|tracker| tracker.is_healthy())
    }
    pub(crate) fn has_owned_claim(&self) -> bool {
        self.reservation
            .lock()
            .ok()
            .is_some_and(|claim| claim.is_some())
    }

    fn destination(&self) -> Option<&crate::path_identity::VerifiedPathIdentity> {
        match &self.lease {
            LifecycleTargetLease::Keyed { destination, .. }
            | LifecycleTargetLease::VerifiedNoReplica { destination } => Some(destination),
            LifecycleTargetLease::MissingSource { .. } => None,
        }
    }

    fn physical_identity_is_current(&self) -> bool {
        let matches = |expected: &crate::path_identity::VerifiedPathIdentity| {
            crate::path_identity::verify_directory(&expected.canonical_path).is_ok_and(|actual| {
                actual.object_id == expected.object_id
                    && actual.canonical_path == expected.canonical_path
            })
        };
        self.destination().is_some_and(matches)
            && self.source.as_ref().is_none_or(|source| {
                matches(&source.directory) && source.replica.as_ref().is_none_or(matches)
            })
    }

    fn target_ownership(
        &self,
    ) -> Result<Option<crate::api::message_store::PtyInputTargetOwnership<'_>>, String> {
        match &self.lease {
            LifecycleTargetLease::Keyed {
                key,
                gate,
                stripe,
                exact,
                ..
            } => gate
                .target_ownership(key, stripe, exact)
                .map(Some)
                .map_err(|_| "invalidTargetCreateOwnership".to_string()),
            _ => Ok(None),
        }
    }

    fn validates_destination(&self, cwd: &str, key: Option<&str>) -> bool {
        let Some(expected) = self.destination() else {
            return false;
        };
        let Ok(current) = crate::path_identity::verify_directory(Path::new(cwd)) else {
            return false;
        };
        if current.object_id != expected.object_id
            || current.canonical_path != expected.canonical_path
        {
            return false;
        }
        match &self.lease {
            LifecycleTargetLease::Keyed { key: actual, .. } => key == Some(actual.as_str()),
            LifecycleTargetLease::VerifiedNoReplica { .. } => key.is_none(),
            _ => false,
        }
    }
}

struct LifecycleClaim {
    reservation: Option<crate::pty::manager::CreateOwnerReservation>,
    discard: Option<crate::config::sessions_persistence::PublishedCarrier>,
}

struct LifecycleClaimScope<'a> {
    source: Option<&'a LifecycleSourceIdentity>,
    source_key: Option<&'a str>,
    destination: &'a crate::path_identity::VerifiedPathIdentity,
    destination_key: Option<&'a str>,
    fallback_target: &'a str,
}

fn capture_lifecycle_claim(
    permit: &crate::session::manager::CarrierMutationPermit,
    current: &crate::session::manager::CarrierSnapshot,
    scope: LifecycleClaimScope<'_>,
    operation: Uuid,
    tracker: &crate::pty::manager::ResponseCloseState,
    pty: &Arc<Mutex<PtyManager>>,
    coordinator: Option<&crate::session::selection::SelectionCoordinator>,
) -> Option<LifecycleClaim> {
    let LifecycleClaimScope {
        source,
        source_key,
        destination,
        destination_key,
        fallback_target,
    } = scope;
    use crate::config::sessions_persistence::{
        classify_lifecycle_carrier_scope, CreateCarrierScope,
    };
    use crate::pty::manager::{CreateAdmission, CreateClassification, ResponseRouteIdentity};
    let destination_scope =
        classify_lifecycle_carrier_scope(permit, destination_key, &destination.canonical_path);
    let source_scope = source.map(|source| {
        classify_lifecycle_carrier_scope(permit, source_key, &source.directory.canonical_path)
    });
    let (target, classification, discard) = match source_scope {
        Some(CreateCarrierScope::Protected {
            target,
            session_id,
            token,
            version,
            epoch,
            preparing_owner: None,
        }) => {
            let source = source?;
            if source.id != session_id || source.token != token || source.version != version {
                return None;
            }
            match destination_scope {
                CreateCarrierScope::Foreign { .. } => {}
                CreateCarrierScope::Protected {
                    target: other_target,
                    session_id: other_id,
                    token: other_token,
                    version: other_version,
                    epoch: other_epoch,
                    preparing_owner: None,
                } if other_target == target
                    && other_id == session_id
                    && other_token == token
                    && other_version == version
                    && other_epoch == epoch => {}
                _ => return None,
            }
            let carrier = crate::config::sessions_persistence::manual_discard_carrier(
                permit,
                &source.directory.canonical_path,
                &target,
                session_id,
                current,
            )?;
            let row = current.rows.iter().find(|row| row.id == session_id)?;
            let generation = if let Some(historical) = carrier.retired_local_generation() {
                let physical_replica = crate::session::manager::CarrierPhysicalIdentity::from(
                    source.replica.as_ref()?.clone(),
                );
                if !carrier.retired_local_replica_matches(&physical_replica) {
                    return None;
                }
                if tracker.generation(session_id).is_some()
                    || !matches!(
                        crate::session::manager::RetiredCarrierRouteObservation::from(
                            PtyManager::snapshot_route_proof(pty, session_id)
                        ),
                        crate::session::manager::RetiredCarrierRouteObservation::Absent
                    )
                    || pty.lock().ok()?.backend_kind(session_id).is_some()
                {
                    return None;
                }
                historical
            } else {
                if matches!(row.status, SessionStatus::Exited(_)) {
                    return None;
                }
                let generation = tracker.generation(session_id)?;
                let route = PtyManager::snapshot_route_proof(pty, session_id).ok()?;
                let actual_generation = pty.lock().ok()?.route_identities(session_id)?.2;
                if actual_generation != generation
                    || !route.matches_requester_route(
                        row.backend_kind,
                        &source.directory,
                        source.replica.as_ref(),
                    )
                {
                    return None;
                }
                generation
            };
            let identity = ResponseRouteIdentity {
                target: target.clone(),
                session_id,
                token,
                generation,
                backend: row.backend_kind,
                carrier_version: version,
            };
            (
                target,
                CreateClassification::Protected {
                    identity,
                    publication_epoch: epoch,
                },
                Some(carrier),
            )
        }
        None | Some(CreateCarrierScope::Foreign { .. }) => {
            let CreateCarrierScope::Foreign { publication_epoch } = destination_scope else {
                return None;
            };
            (
                fallback_target.to_owned(),
                CreateClassification::Foreign { publication_epoch },
                None,
            )
        }
        _ => return None,
    };
    let foreign = matches!(&classification, CreateClassification::Foreign { .. });
    match tracker.reserve_create_owner(&target, operation, classification) {
        Ok(CreateAdmission::Reserved(reservation)) => Some(LifecycleClaim {
            reservation: Some(reservation),
            discard,
        }),
        Err(crate::pty::manager::ResponseCloseError::Unavailable)
            if foreign
                && !tracker.target_is_closing(&target)
                && coordinator.is_some_and(|coordinator| {
                    coordinator.retains_lifecycle_operation(operation)
                }) =>
        {
            // SAME permanently failed tracker, current ALL-scope Foreign and a
            // real upper retained owner. No healthy/Protected/unknown fallback.
            Some(LifecycleClaim {
                reservation: None,
                discard: None,
            })
        }
        _ => None,
    }
}

pub(crate) async fn prepare_lifecycle_entry_authority<R: tauri::Runtime>(
    app: &AppHandle<R>,
    operation: Uuid,
    requested_source: Option<Uuid>,
    root_request: bool,
    root_directory: Option<&str>,
) -> Result<LifecycleAuthorityAdmission, String> {
    let manager = app
        .state::<Arc<tokio::sync::RwLock<SessionManager>>>()
        .read()
        .await
        .clone();
    let source_id = if root_request {
        manager
            .list_sessions()
            .await
            .into_iter()
            .find(|row| {
                row.is_root_agent
                    || crate::config::root_agent::is_root_agent_path(&row.working_directory)
            })
            .map(|row| Uuid::parse_str(&row.id))
            .transpose()
            .map_err(|error| error.to_string())?
    } else {
        requested_source
    };
    let snapshot = manager.with_response_close_facts(Clone::clone).await;
    let source_row = source_id.and_then(|id| {
        snapshot
            .rows
            .iter()
            .find(|row| row.id == id && !snapshot.pending_ids.contains(&row.id))
    });
    if !root_request && source_row.is_none() {
        // A pending exact UUID is present, not a missing-source validation case.
        if requested_source.is_some_and(|id| snapshot.rows.iter().any(|row| row.id == id)) {
            return Ok(LifecycleAuthorityAdmission::Waiting);
        }
        let Some(requested) = requested_source else {
            return Ok(LifecycleAuthorityAdmission::Waiting);
        };
        return Ok(LifecycleAuthorityAdmission::Ready(
            LifecycleEntryAuthority {
                lease: LifecycleTargetLease::MissingSource { requested },
                source: None,
                root_request,
                operation,
                reservation: Mutex::new(None),
                interrupted: std::sync::atomic::AtomicBool::new(false),
                discard: None,
                tracker: None,
            },
        ));
    }
    let needs_root = root_request
        || source_row.is_some_and(|row| {
            row.is_root_agent
                || crate::config::root_agent::is_root_agent_path(&row.working_directory)
        });
    if needs_root && root_directory.is_none() {
        return Ok(LifecycleAuthorityAdmission::NeedsRootDirectory);
    }
    let cwd = if needs_root {
        root_directory.unwrap_or_default()
    } else {
        source_row
            .map(|row| row.working_directory.as_str())
            .unwrap_or_default()
    };
    let Ok(destination) = crate::path_identity::verify_directory(Path::new(cwd)) else {
        return Ok(LifecycleAuthorityAdmission::Waiting);
    };
    let key = crate::config::teams::pty_input_create_gate_key_from_cwd(Path::new(cwd))
        .map_err(|_| "targetCreateGateUnavailable".to_string())?;
    let mut source = if let Some(row) = source_row {
        let Some(version) = snapshot.versions.get(&row.id).copied() else {
            return Ok(LifecycleAuthorityAdmission::Waiting);
        };
        let Ok(directory) =
            crate::path_identity::verify_directory(Path::new(&row.working_directory))
        else {
            return Ok(LifecycleAuthorityAdmission::Waiting);
        };
        Some(LifecycleSourceIdentity {
            id: row.id,
            token: row.token,
            version,
            saved_cwd: row.working_directory.clone(),
            directory,
            replica: None,
        })
    } else {
        None
    };
    let source_key = source_row
        .map(|row| {
            crate::config::teams::pty_input_create_gate_key_from_cwd(Path::new(
                &row.working_directory,
            ))
        })
        .transpose()
        .map_err(|_| "targetCreateGateUnavailable".to_string())?
        .flatten();
    if let Some(source) = source.as_mut() {
        source.replica = match crate::config::teams::strict_wg_replica_anchor_from_cwd(Path::new(
            &source.saved_cwd,
        )) {
            Ok(Some(root)) => match crate::path_identity::verify_directory(&root) {
                Ok(identity) => Some(identity),
                Err(_) => return Ok(LifecycleAuthorityAdmission::Waiting),
            },
            Ok(None) => None,
            Err(_) => return Ok(LifecycleAuthorityAdmission::Waiting),
        };
    }
    let lease = if let Some(key) = key {
        let gate = app
            .try_state::<crate::api::message_store::PtyInputTargetGateState>()
            .and_then(|state| state.gate.as_ref().ok().map(Arc::clone))
            .or_else(|| {
                app.try_state::<crate::api::message_store::MessageStoreState>()
                    .and_then(|state| state.target_gate.as_ref().ok().map(Arc::clone))
            })
            .ok_or_else(|| "targetCreateGateUnavailable".to_string())?;
        let stripe = gate
            .acquire_target_lock(&key)
            .await
            .map_err(|_| "targetCreateGateUnavailable".to_string())?;
        let exact = gate.acquire_exact(&key).await;
        LifecycleTargetLease::Keyed {
            key,
            destination,
            gate,
            stripe,
            exact,
        }
    } else {
        LifecycleTargetLease::VerifiedNoReplica { destination }
    };
    // Gate acquisition may have suspended. Never admit an identity verified
    // only before those waits; cancellation drops exact then stripe normally.
    let physical_matches = |expected: &crate::path_identity::VerifiedPathIdentity| {
        crate::path_identity::verify_directory(&expected.canonical_path).is_ok_and(|actual| {
            actual.object_id == expected.object_id
                && actual.canonical_path == expected.canonical_path
        })
    };
    let destination_matches = match &lease {
        LifecycleTargetLease::Keyed { destination, .. }
        | LifecycleTargetLease::VerifiedNoReplica { destination } => physical_matches(destination),
        LifecycleTargetLease::MissingSource { .. } => false,
    };
    if !destination_matches
        || !source
            .as_ref()
            .is_none_or(|source| physical_matches(&source.directory))
    {
        drop(lease);
        return Ok(LifecycleAuthorityAdmission::Waiting);
    }
    let destination = match &lease {
        LifecycleTargetLease::Keyed { destination, .. }
        | LifecycleTargetLease::VerifiedNoReplica { destination } => destination,
        _ => unreachable!("prepared destination"),
    };
    let destination_key = match &lease {
        LifecycleTargetLease::Keyed { key, .. } => Some(key.as_str()),
        _ => None,
    };
    let tracker = app
        .state::<Arc<Mutex<PtyManager>>>()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .response_close_state();
    let target = destination_key
        .map(str::to_owned)
        .or(source_key.clone())
        .unwrap_or_else(|| format!("create-cwd:{}", destination.canonical_path.display()));
    let permit = acquire_carrier_mutation_permit().await;
    let reservation = manager
        .with_current_carrier_snapshot(&permit, |current| {
            if !lifecycle_source_matches(current, source.as_ref(), root_request) {
                return None;
            }
            capture_lifecycle_claim(
                &permit,
                current,
                LifecycleClaimScope {
                    source: source.as_ref(),
                    source_key: source_key.as_deref(),
                    destination,
                    destination_key,
                    fallback_target: &target,
                },
                operation,
                &tracker,
                app.state::<Arc<Mutex<PtyManager>>>().inner(),
                app.try_state::<crate::session::selection::SelectionCoordinator>()
                    .as_deref(),
            )
        })
        .await;
    drop(permit);
    let Some(reservation) = reservation else {
        drop(lease);
        return Ok(LifecycleAuthorityAdmission::Waiting);
    };
    Ok(LifecycleAuthorityAdmission::Ready(
        LifecycleEntryAuthority {
            lease,
            source,
            root_request,
            operation,
            reservation: Mutex::new(reservation.reservation),
            interrupted: std::sync::atomic::AtomicBool::new(false),
            discard: reservation.discard,
            tracker: Some(tracker),
        },
    ))
}

pub(crate) async fn validate_lifecycle_entry_authority<R: tauri::Runtime>(
    app: &AppHandle<R>,
    authority: &mut LifecycleEntryAuthority,
) -> bool {
    let manager = app
        .state::<Arc<tokio::sync::RwLock<SessionManager>>>()
        .read()
        .await
        .clone();
    if let LifecycleTargetLease::MissingSource { requested } = &authority.lease {
        let permit = acquire_carrier_mutation_permit().await;
        return manager
            .with_current_carrier_snapshot(&permit, |current| {
                !current.rows.iter().any(|row| row.id == *requested)
            })
            .await;
    }
    let Some(destination) = authority.destination().cloned() else {
        return false;
    };
    if !authority.physical_identity_is_current() {
        return false;
    }
    let Ok(key) =
        crate::config::teams::pty_input_create_gate_key_from_cwd(&destination.canonical_path)
    else {
        return false;
    };
    if !authority.validates_destination(
        &destination.canonical_path.to_string_lossy(),
        key.as_deref(),
    ) {
        return false;
    }
    let source_key = match authority
        .source
        .as_ref()
        .map(|source| {
            crate::config::teams::pty_input_create_gate_key_from_cwd(
                &source.directory.canonical_path,
            )
        })
        .transpose()
    {
        Ok(key) => key.flatten(),
        Err(_) => return false,
    };
    let target = key
        .clone()
        .or(source_key.clone())
        .unwrap_or_else(|| format!("create-cwd:{}", destination.canonical_path.display()));
    let tracker = app
        .state::<Arc<Mutex<PtyManager>>>()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .response_close_state();
    let permit = acquire_carrier_mutation_permit().await;
    {
        let Ok(mut owned_claim) = authority.reservation.lock() else {
            return false;
        };
        drop(owned_claim.take());
    }
    let reservation = manager
        .with_current_carrier_snapshot(&permit, |current| {
            if !lifecycle_source_matches(current, authority.source.as_ref(), authority.root_request)
            {
                return None;
            }
            capture_lifecycle_claim(
                &permit,
                current,
                LifecycleClaimScope {
                    source: authority.source.as_ref(),
                    source_key: source_key.as_deref(),
                    destination: &destination,
                    destination_key: key.as_deref(),
                    fallback_target: &target,
                },
                authority.operation,
                &tracker,
                app.state::<Arc<Mutex<PtyManager>>>().inner(),
                app.try_state::<crate::session::selection::SelectionCoordinator>()
                    .as_deref(),
            )
        })
        .await;
    drop(permit);
    let same_discard = match (
        &authority.discard,
        reservation
            .as_ref()
            .and_then(|claim| claim.discard.as_ref()),
    ) {
        (None, None) => true,
        (Some(expected), Some(actual)) => {
            expected.session_id == actual.session_id
                && expected.token == actual.token
                && expected.version == actual.version
                && expected.nonce == actual.nonce
                && expected.target == actual.target
                && expected.directory == actual.directory
        }
        _ => false,
    };
    if !same_discard {
        return false;
    }
    let Ok(mut owned_claim) = authority.reservation.lock() else {
        return false;
    };
    let admitted = reservation.is_some();
    *owned_claim = reservation.and_then(|claim| claim.reservation);
    admitted
}

/// Last prerequisite-to-teardown boundary. Re-admit the SAME claim under U/M,
/// then retire only its exact protection. No waiting or session deletion here.
async fn retire_lifecycle_entry_protection<R: tauri::Runtime>(
    app: &AppHandle<R>,
    authority: &LifecycleEntryAuthority,
) -> bool {
    if !authority.physical_identity_is_current() {
        return false;
    }
    let Some(destination) = authority.destination() else {
        return false;
    };
    let Ok(destination_key) =
        crate::config::teams::pty_input_create_gate_key_from_cwd(&destination.canonical_path)
    else {
        return false;
    };
    if !authority.validates_destination(
        &destination.canonical_path.to_string_lossy(),
        destination_key.as_deref(),
    ) {
        return false;
    }
    let source_key = match authority
        .source
        .as_ref()
        .map(|source| {
            crate::config::teams::pty_input_create_gate_key_from_cwd(
                &source.directory.canonical_path,
            )
        })
        .transpose()
    {
        Ok(key) => key.flatten(),
        Err(_) => return false,
    };
    let target = destination_key
        .clone()
        .or(source_key.clone())
        .unwrap_or_else(|| format!("create-cwd:{}", destination.canonical_path.display()));
    let manager = app
        .state::<Arc<tokio::sync::RwLock<SessionManager>>>()
        .read()
        .await
        .clone();
    let pty = app.state::<Arc<Mutex<PtyManager>>>();
    let tracker = match pty.lock() {
        Ok(pty) => pty.response_close_state(),
        Err(_) => return false,
    };
    let permit = acquire_carrier_mutation_permit().await;
    manager
        .with_current_carrier_snapshot(&permit, |current| {
            if !lifecycle_source_matches(current, authority.source.as_ref(), authority.root_request)
            {
                return false;
            }
            let Ok(mut owned) = authority.reservation.lock() else {
                return false;
            };
            drop(owned.take());
            let Some(claim) = capture_lifecycle_claim(
                &permit,
                current,
                LifecycleClaimScope {
                    source: authority.source.as_ref(),
                    source_key: source_key.as_deref(),
                    destination,
                    destination_key: destination_key.as_deref(),
                    fallback_target: &target,
                },
                authority.operation,
                &tracker,
                pty.inner(),
                app.try_state::<crate::session::selection::SelectionCoordinator>()
                    .as_deref(),
            ) else {
                return false;
            };
            let exact = match (&authority.discard, &claim.discard) {
                (None, None) => true,
                (Some(expected), Some(actual)) => {
                    expected.session_id == actual.session_id
                        && expected.token == actual.token
                        && expected.version == actual.version
                        && expected.nonce == actual.nonce
                        && expected.target == actual.target
                }
                _ => false,
            };
            if !exact {
                return false;
            }
            if let Some(carrier) = &claim.discard {
                let Some(source) = authority.source.as_ref() else {
                    return false;
                };
                if !crate::config::sessions_persistence::retire_manual_discard_carrier(
                    &permit,
                    carrier,
                    &source.directory.canonical_path,
                    current,
                ) {
                    return false;
                }
                tracker.invalidate_manager_row(carrier.session_id, std::time::Instant::now());
                crate::session::selection::forget_response_close_registration(carrier.session_id);
            }
            *owned = claim.reservation;
            true
        })
        .await
}

fn lifecycle_source_matches(
    current: &crate::session::manager::CarrierSnapshot,
    source: Option<&LifecycleSourceIdentity>,
    root_request: bool,
) -> bool {
    let exact = source.is_none_or(|source| {
        current.rows.iter().any(|row| {
            row.id == source.id
                && row.token == source.token
                && current.versions.get(&row.id) == Some(&source.version)
                && !current.pending_ids.contains(&row.id)
                && row.working_directory == source.saved_cwd
        })
    });
    if !exact {
        return false;
    }
    if root_request {
        let mut roots = current.rows.iter().filter(|row| {
            !current.pending_ids.contains(&row.id)
                && (row.is_root_agent
                    || crate::config::root_agent::is_root_agent_path(&row.working_directory))
        });
        let selected = roots.next().map(|row| row.id);
        selected == source.map(|source| source.id) && roots.next().is_none()
    } else {
        true
    }
}

async fn prepare_restore_lifecycle_authority<R: tauri::Runtime>(
    transaction: &SelectionTransaction<R>,
    source: Option<Uuid>,
    root_request: bool,
) -> Result<(LifecycleEntryAuthority, Option<String>), String> {
    let Some(operation) = transaction.current_restore_nonce() else {
        return Err(crate::session::selection::SelectionCoordinatorError::Unavailable.to_string());
    };
    let mut root_directory = if root_request {
        Some(crate::config::root_agent::ensure_root_agent_dir()?)
    } else {
        None
    };
    loop {
        if !transaction.has_active_restore_capability() {
            return Err(
                crate::session::selection::SelectionCoordinatorError::Unavailable.to_string(),
            );
        }
        match prepare_lifecycle_entry_authority(
            transaction.app(),
            operation,
            source,
            root_request,
            root_directory.as_deref(),
        )
        .await?
        {
            LifecycleAuthorityAdmission::Ready(mut authority) => {
                if authority.discard.is_some() {
                    transaction.invalidate_restore_ownership();
                    return Err(
                        crate::session::selection::SelectionCoordinatorError::Unavailable
                            .to_string(),
                    );
                }
                if !validate_lifecycle_entry_authority(transaction.app(), &mut authority).await {
                    transaction.invalidate_restore_ownership();
                    return Err(
                        crate::session::selection::SelectionCoordinatorError::Unavailable
                            .to_string(),
                    );
                }
                return Ok((authority, root_directory));
            }
            LifecycleAuthorityAdmission::NeedsRootDirectory => {
                root_directory = Some(crate::config::root_agent::ensure_root_agent_dir()?);
            }
            LifecycleAuthorityAdmission::Waiting => {
                transaction.invalidate_restore_ownership();
                // Startup cannot wait for a protected finalizer queued behind
                // its own real barrier. No ownership is inferred from Running.
                return Err(
                    crate::session::selection::SelectionCoordinatorError::Unavailable.to_string(),
                );
            }
        }
    }
}

pub(crate) fn ensure_owned_root_directory(
    effect: crate::session::selection::LifecycleEffectGuard,
) -> Result<String, String> {
    let result = crate::config::root_agent::ensure_root_agent_dir();
    effect.complete(result.clone().map(Some));
    result
}

async fn ensure_lifecycle_root_directory<R: tauri::Runtime>(
    transaction: &SelectionTransaction<R>,
    context: &LifecycleExecutionContext<'_>,
) -> Result<String, String> {
    if let LifecycleExecutionContext::RestoreEntry { root_directory, .. } = context {
        if !transaction.has_active_restore_capability() {
            return Err(
                crate::session::selection::SelectionCoordinatorError::Unavailable.to_string(),
            );
        }
        return root_directory.map(str::to_owned).ok_or_else(|| {
            crate::session::selection::SelectionCoordinatorError::Unavailable.to_string()
        });
    }
    if !transaction.has_accepted_lifecycle_core() {
        return std::future::pending().await;
    }
    if let Some(outcome) = transaction.lifecycle_root_directory_outcome() {
        return outcome;
    }
    let Some(effect) = transaction
        .start_lifecycle_effect(crate::session::selection::LifecycleEffectSlot::RootDirectory)
    else {
        return std::future::pending().await;
    };
    ensure_owned_root_directory(effect)
}

pub(crate) async fn execute_root_transaction<R: tauri::Runtime>(
    transaction: &SelectionTransaction<R>,
    request: RootJobRequest,
) -> Result<SessionInfo, String> {
    if !transaction.has_active_restore_capability() {
        return Err(crate::session::selection::SelectionCoordinatorError::Unavailable.to_string());
    }
    let (authority, root_directory) =
        prepare_restore_lifecycle_authority(transaction, None, true).await?;
    execute_root_transaction_in_context(
        transaction,
        request,
        &LifecycleExecutionContext::RestoreEntry {
            authority: &authority,
            root_directory: root_directory.as_deref(),
        },
    )
    .await
}

pub(crate) async fn execute_external_root_transaction<R: tauri::Runtime>(
    transaction: &SelectionTransaction<R>,
    request: RootJobRequest,
    authority: &LifecycleEntryAuthority,
) -> LifecycleEntryOutcome {
    let result = execute_root_transaction_in_context(
        transaction,
        request,
        &LifecycleExecutionContext::ExternalEntry(authority),
    )
    .await;
    if authority
        .interrupted
        .load(std::sync::atomic::Ordering::Acquire)
    {
        LifecycleEntryOutcome::Interrupted
    } else {
        LifecycleEntryOutcome::Returned(result)
    }
}

async fn execute_root_transaction_in_context<R: tauri::Runtime>(
    transaction: &SelectionTransaction<R>,
    request: RootJobRequest,
    context: &LifecycleExecutionContext<'_>,
) -> Result<SessionInfo, String> {
    if matches!(context, LifecycleExecutionContext::RestoreEntry { .. })
        && !transaction.has_active_restore_capability()
    {
        return Err(crate::session::selection::SelectionCoordinatorError::Unavailable.to_string());
    }
    let app = transaction.app();
    let session_mgr = app
        .state::<Arc<tokio::sync::RwLock<SessionManager>>>()
        .inner()
        .clone();
    let pty_mgr = app.state::<Arc<Mutex<PtyManager>>>().inner().clone();
    let settings = app.state::<SettingsState>().inner().clone();
    let root_agent_path = ensure_lifecycle_root_directory(transaction, context).await?;
    let existing = session_mgr
        .read()
        .await
        .list_sessions()
        .await
        .into_iter()
        .find(|session| {
            session.is_root_agent
                || crate::config::root_agent::is_root_agent_path(&session.working_directory)
        });

    if let Some(existing) = existing {
        let session_id = Uuid::parse_str(&existing.id).map_err(|error| error.to_string())?;
        session_mgr
            .read()
            .await
            .set_is_root_agent(session_id, true)
            .await;
        let has_pty = pty_mgr
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .has_session(session_id);
        match classify_existing_root(&existing.status, has_pty) {
            ExistingRootAction::ReuseLive => {
                // Reuse has no new binding/backend or replacement effects. Track
                // its actual selection/persistence/publication and final lookup.
                let publication = transaction.start_lifecycle_effect(
                    crate::session::selection::LifecycleEffectSlot::Publication,
                );
                if transaction.has_lifecycle_entry() && publication.is_none() {
                    return std::future::pending().await;
                }
                let result = async {
                    if request.select_after {
                        let decision = transaction
                            .live_decision(session_id)
                            .ok_or_else(|| "Root session has no displayable PTY".to_string())?;
                        let committed = transaction
                            .commit(
                                decision,
                                SelectionCause::UserSwitch,
                                LifecycleMutations::default(),
                            )
                            .await?;
                        if let Some(selection) = committed.selection.as_ref() {
                            transaction
                                .persist(SelectionSource::UserSwitch, Some(session_id))
                                .await;
                            transaction.publish_selection(selection);
                        }
                    }
                    session_mgr
                        .read()
                        .await
                        .get_session(session_id)
                        .await
                        .map(|session| SessionInfo::from(&session))
                        .ok_or_else(|| "Root session disappeared during reuse".to_string())
                }
                .await;
                if let Some(publication) = publication {
                    publication.complete(
                        result
                            .as_ref()
                            .map(|info| Some(info.id.clone()))
                            .map_err(Clone::clone),
                    );
                    transaction.observe_reuse_live_result(&result);
                }
                return result;
            }
            ExistingRootAction::WakeDormant | ExistingRootAction::DiscardMissingPty => {
                let wake_dormant = matches!(
                    classify_existing_root(&existing.status, has_pty),
                    ExistingRootAction::WakeDormant
                );
                let restart_intent = match request.intent {
                    TrustedCreateIntent::User => TrustedRestartIntent::User,
                    TrustedCreateIntent::Background => TrustedRestartIntent::Background,
                };
                return execute_restart_transaction_in_context(
                    transaction,
                    RestartJobRequest {
                        session_id,
                        agent_id: request.requested_agent_id,
                        requested_profile: request.requested_profile,
                        skip_auto_resume: Some(if wake_dormant {
                            false
                        } else {
                            request.skip_auto_resume_for_new_session
                        }),
                        activate_after: request.select_after,
                        intent: restart_intent,
                        communication_override: None,
                        enforcement:
                            crate::config::sessions_persistence::default_creation_gate_enforcement(
                            ),
                    },
                    context,
                )
                .await;
            }
        }
    }

    let last_coding_agent = crate::config::root_agent::read_last_coding_agent(&root_agent_path);
    let (shell, shell_args, agent_id, agent_label) = {
        let settings = settings
            .current_read()
            .await
            .map_err(|error| error.to_string())?;
        resolve_root_agent_command(
            &settings,
            request.requested_agent_id.as_deref(),
            last_coding_agent.as_deref(),
        )?
    };
    let (resolved_spawn, resolved_agent_host_shell) = if let Some(agent_id) = agent_id.as_deref() {
        // #1271 - build the spawn and copy the configured default host shell
        // from the SAME single guard, before any await, so the pair can never
        // mix across a configuration change (Phase 1 items 1-2; mirrors the
        // restart pattern).
        let settings = settings
            .current_read()
            .await
            .map_err(|error| error.to_string())?;
        let spawn = build_configured_agent_spawn_for_cwd(
            &settings,
            agent_id,
            &root_agent_path,
            request.requested_profile.as_deref(),
        )?;
        let host_shell = Some(ResolvedAgentHostShell {
            program: settings.default_shell.clone(),
            args: settings.default_shell_args.clone(),
        });
        (spawn, host_shell)
    } else {
        (None, None)
    };
    let (shell, shell_args, agent_label) = if let Some(spawn) = resolved_spawn.as_ref() {
        (
            spawn.shell.clone(),
            spawn.shell_args.clone(),
            Some(spawn.trusted_agent_label.clone()),
        )
    } else {
        (shell, shell_args, agent_label)
    };
    let target_ownership = context
        .authority()
        .map(|authority| authority.target_ownership())
        .transpose()?
        .flatten();
    let info = create_session_inner_impl(
        app,
        &session_mgr,
        &pty_mgr,
        shell,
        shell_args,
        root_agent_path.clone(),
        Some(crate::config::root_agent::ROOT_AGENT_SESSION_NAME.to_string()),
        agent_id,
        agent_label,
        false,
        Vec::new(),
        request.skip_auto_resume_for_new_session,
        resolved_spawn,
        resolved_agent_host_shell,
        None,
        CreateSelectionIntent::Suppress,
        None,
        None,
        Some(transaction),
        Some(SelectionCause::SessionCreated(request.intent)),
        false,
        target_ownership.as_ref(),
        crate::config::sessions_persistence::default_creation_gate_enforcement(),
        None,
        context.authority(),
        None,
        None,
    )
    .await?
    .into_finalized()
    .await?;
    let session_id = Uuid::parse_str(&info.id).map_err(|error| error.to_string())?;
    if request.select_after {
        let decision = transaction
            .live_decision(session_id)
            .ok_or_else(|| "new Root session has no displayable PTY".to_string())?;
        let committed = transaction
            .commit(
                decision,
                SelectionCause::SessionCreated(request.intent),
                LifecycleMutations::default(),
            )
            .await?;
        if let Some(selection) = committed.selection.as_ref() {
            transaction
                .persist(SelectionSource::SessionCreated, Some(session_id))
                .await;
            transaction.publish_selection(selection);
        }
    }
    attach_local_config_telegram_if_any(app, session_id, &root_agent_path).await;
    Ok(info)
}

/// Create, wake, or reuse a root agent session.
#[tauri::command]
pub async fn create_root_agent_session(
    app: AppHandle,
    session_mgr: State<'_, Arc<tokio::sync::RwLock<SessionManager>>>,
    pty_mgr: State<'_, Arc<Mutex<PtyManager>>>,
    tg_mgr: State<'_, TelegramBridgeState>,
    settings: State<'_, SettingsState>,
    agent_id: Option<String>,
    requested_profile: Option<String>,
) -> Result<SessionInfo, String> {
    create_root_agent_inner(
        &app,
        session_mgr.inner(),
        pty_mgr.inner(),
        tg_mgr.inner(),
        settings.inner(),
        agent_id,
        requested_profile,
        true, // skip_auto_resume = true → fresh create, no `--continue` injection
    )
    .await
}

/// #1646 / #1647 - Resolves a blocking menu episode for a session.
/// Clears the blocked menu in SessionManager and publishes the updated communication state.
/// #1669 - the clear is persisted through the same atomic helper `scan_tick` uses,
/// so a disk-reading CLI stops reporting `blockedMenu` as soon as the user
/// resolves the menu. The scan loop cannot do this: `resolve_current_episode`
/// suppresses the episode, and the suppressed arm of `evaluate_logical_rows`
/// returns BOTH `should_notify: false` and `should_clear_notification: false`.
#[tauri::command]
pub async fn resolve_blocking_menu<R: Runtime>(
    app: AppHandle<R>,
    session_mgr: State<'_, Arc<tokio::sync::RwLock<SessionManager>>>,
    menu_guard: State<'_, Arc<crate::pty::menu_guard::MenuGuard>>,
    id: String,
) -> Result<(), String> {
    let session_id = Uuid::parse_str(&id).map_err(|e| format!("Invalid session ID: {e}"))?;
    menu_guard.resolve_current_episode(session_id);
    let mgr = { session_mgr.read().await.clone() };
    crate::config::sessions_persistence::clear_blocked_menu_and_persist(&mgr, session_id).await;
    crate::session::selection::publish_session_communication(&app, session_id, None);
    Ok(())
}

/// #1682 - the stamp for the session's replica, for a terminal that just mounted
/// and missed the `session_agent_message` event. `None` covers every unavailable
/// case: unknown session, plain shell with no `agent_id`, missing or unparseable
/// config, missing key.
#[tauri::command]
pub async fn get_last_agent_message(
    session_mgr: tauri::State<'_, Arc<tokio::sync::RwLock<SessionManager>>>,
    session_id: String,
) -> Result<Option<String>, String> {
    let uuid = Uuid::parse_str(&session_id).map_err(|e| e.to_string())?;
    let Some(session) = session_mgr.read().await.get_session(uuid).await else {
        return Ok(None);
    };
    if session.agent_id.is_none() {
        return Ok(None);
    }
    Ok(agent_config::read_last_agent_message_at(
        &session.working_directory,
    ))
}

/// #1682 - called from the idle callback after `mark_idle`. Persists the stamp
/// for an armed session, then announces it on both transports. A no-op when a
/// control write less than `USER_WRITE_STAMP_WINDOW` old re-opened the busy
/// period this edge closed, when the user has unsubmitted input less than the
/// same window old in this session, when the session was never armed
/// or has no coding agent, when the stored value is already at or after this
/// instant, or when the write failed.
pub(crate) async fn record_agent_turn_completed<R: tauri::Runtime>(
    app: &AppHandle<R>,
    mgr: &SessionManager,
    id: Uuid,
    control_write_age: Option<std::time::Duration>,
) {
    // #1682 gate 1 - do not stamp when a PTY write we made, and that was not a
    // delivered message, is what re-opened the busy period this edge just
    // closed. `lib.rs` read the age off the idle detector at the edge instant;
    // see this phase's D1a for why it is a parameter and not a `try_state` here.
    // `None` means no mark stands, which lets the stamp proceed. The mark is a
    // near-exact answer to that question and not an exact one: phase 03's D1
    // owns the property, and its R4 and R6 are the two ways a mark can stand
    // for a write that re-opened nothing.
    if control_write_age
        .is_some_and(|age| age <= crate::pty::input_activity::USER_WRITE_STAMP_WINDOW)
    {
        return;
    }

    // #1682 gate 2 - do not stamp while the user has a RECENT half-typed,
    // unsubmitted line in this terminal. The agent CLI's echo of those
    // keystrokes is real PTY output, and `commands/pty.rs:70` marks activity
    // for the keystroke itself before `substantive` is known at `:75`, so both
    // mechanisms would otherwise advance a stamp that is meant to track this
    // agent's own output rather than the person's typing. `pending_nonspace`
    // (#871) is the repository's existing per-session answer to "is there
    // unsubmitted content"; the age bound is #1682's, and it is required
    // rather than defensive, because outside `pty::input_activity` the only
    // clear is `reset`, reached from a session destroy or restart and from a
    // fresh conversation boundary, and neither reaches an agent driven by
    // inter-agent messages: an injection's Enter never reaches the classifier
    // and the plain message payload stamps no boundary.
    //
    // Fails open when the tracker is not managed. This is the same
    // `try_state` fail-open PATTERN as `classify_substantive` in
    // `commands/pty.rs`, with the value that lets THIS function's
    // primary action proceed. The two values are opposite on purpose:
    // `classify_substantive` fails open to `true` to preserve #871's
    // clear contract, this gate to `false` so the stamp still lands.
    //
    // Bound to a `bool` in its own scope so the `std::sync::Mutex` guard is
    // dropped before any await: `clippy::await_holding_lock` does not recognise
    // a later explicit `drop`, and there must be no await while it is held.
    let typing_pending = match app.try_state::<crate::pty::input_activity::SubstantiveInputState>()
    {
        Some(state) => {
            let tracker = state.lock().unwrap_or_else(|e| e.into_inner());
            tracker.pending_within(id, crate::pty::input_activity::USER_WRITE_STAMP_WINDOW)
        }
        None => false,
    };
    if typing_pending {
        return;
    }

    // #1682 gate 3 - armed, known, and carrying a coding agent.
    let Some((cwd, _agent_id)) = mgr.agent_stamp_target(id).await else {
        return;
    };

    // #1682 gate 4 - the monotonic write reports whether it actually landed.
    let at = chrono::Utc::now().to_rfc3339();
    match agent_config::set_last_agent_message_at(&cwd, &at) {
        Ok(true) => {}
        Ok(false) => return,
        Err(e) => {
            log::warn!("Failed to persist lastAgentMessageAt for {}: {}", id, e);
            return;
        }
    }

    crate::web::commands::broadcast_all_r(
        app,
        "session_agent_message",
        &serde_json::json!({ "sessionId": id.to_string(), "at": at }),
    );
}

/// (#1793) Settle before the first readiness poll. NOT a readiness guard: the
/// idle detector SEEDS `activity` at PTY spawn (#260), so `waiting_for_input`
/// reaches `true` for a session that has printed nothing, and waiting longer
/// makes that more likely, not less. What makes readiness mean "the agent
/// painted its prompt and then went quiet" is the second conjunct,
/// `commands::pty::session_printed_since`. This settle covers only the agent
/// that prints, pauses longer than `idle_threshold` mid-load, then prints
/// again, and it keeps the pass off a machine still spawning a fleet of PTYs.
/// Same magnitude as `auto_close::WAKE_GRACE` but NOT its rationale, which is
/// kill-eligibility, ordering against `REPAINT_GRACE`, and a spawn race.
/// The value is engineering judgement.
const RESTART_RESUME_SETTLE: std::time::Duration = std::time::Duration::from_secs(30);

/// (#1793) Poll cadence, matching the idle detector's own tick.
const RESTART_RESUME_POLL: std::time::Duration = std::time::Duration::from_millis(500);

/// (#1793) Budget after the settle. A target that never reaches its prompt in
/// this window gets NOTHING: fails closed on purpose, diverging from
/// `loops::delivery::wait_for_session_idle`, which waits 90 s and fails OPEN.
/// The direction is forced by the feature; the 300 s is engineering judgement
/// (a restored fleet replays conversations concurrently), bounded above by the
/// risk of nudging an agent the user has already picked up by hand.
const RESTART_RESUME_DEADLINE: std::time::Duration = std::time::Duration::from_secs(300);

/// (#1793) One agent the pass will nudge. Built inside the restore loop, where
/// the persisted row and its new session id are both in hand, so no correlation
/// key is needed. `working_directory` is for log lines only.
#[derive(Debug, Clone)]
pub(crate) struct RestartResumeTarget {
    pub session_id: Uuid,
    pub name: String,
    pub working_directory: String,
    pub prompt: String,
    /// The instant the restore loop paired this persisted row with its new
    /// session id. The PTY was already spawned by then, so
    /// `IdleDetector::register_session` had already seeded `activity[id]` and
    /// that seed stamp is at or before this instant. Readiness therefore asks
    /// for printable output STRICTLY after this instant, which the spawn seed
    /// alone can never satisfy.
    pub collected_at: std::time::Instant,
}

/// (#1793) Pure: the exact text a restored row earns at restart, or `None`.
pub(crate) fn restart_resume_prompt_for(
    settings: &AppSettings,
    is_coordinator: bool,
    start_fresh_on_restore: bool,
    persisted_working: bool,
) -> Option<&str> {
    if start_fresh_on_restore || !persisted_working {
        return None;
    }
    let prompt = if is_coordinator {
        settings.restart_resume_orchestrator_prompt.as_str()
    } else {
        settings.restart_resume_agent_prompt.as_str()
    };
    if prompt.is_empty() {
        None
    } else {
        Some(prompt)
    }
}

/// (#1793) Pure: the FIRST readiness conjunct. The session must still exist,
/// must not have exited, and must be parked at its prompt. On its own this is
/// NOT enough: `waiting_for_input` is also true for a session that has printed
/// nothing since spawn (#260 seeds the activity clock). The second conjunct is
/// applied by `partition_restart_resume_ready`.
pub(crate) fn restart_resume_session_is_ready(session: Option<&SessionInfo>) -> bool {
    match session {
        Some(session) => {
            session.waiting_for_input && !matches!(session.status, SessionStatus::Exited(_))
        }
        None => false,
    }
}

/// (#1793) Pure: split the working set into the targets that stay and the
/// targets that may be written to NOW. Both readiness conjuncts are applied
/// here. Every input target is returned in EXACTLY ONE of the two vectors, and
/// the caller assigns `remaining` back over its working set, so selection and
/// removal cannot drift apart and a target can never be offered twice.
pub(crate) fn partition_restart_resume_ready(
    targets: Vec<RestartResumeTarget>,
    live: &[SessionInfo],
    printed_since_collection: impl Fn(&RestartResumeTarget) -> bool,
) -> (Vec<RestartResumeTarget>, Vec<RestartResumeTarget>) {
    let mut remaining: Vec<RestartResumeTarget> = Vec::with_capacity(targets.len());
    let mut ready: Vec<RestartResumeTarget> = Vec::new();
    for target in targets {
        let id = target.session_id.to_string();
        let session = live.iter().find(|s| s.id == id);
        if restart_resume_session_is_ready(session) && printed_since_collection(&target) {
            ready.push(target);
        } else {
            remaining.push(target);
        }
    }
    (remaining, ready)
}

/// (#1793) Spawned once per app start from the restore tail in `lib.rs`, after
/// the idle observer is running, with the targets the restore loop collected.
/// Sequential by construction: at most one PTY write is in flight, so a fleet
/// restart cannot produce a write storm.
pub(crate) async fn run_restart_resume(app: &AppHandle, mut targets: Vec<RestartResumeTarget>) {
    if targets.is_empty() {
        return;
    }
    log::info!(
        "[restart-resume] {} target(s); settling {}s before the first readiness poll",
        targets.len(),
        RESTART_RESUME_SETTLE.as_secs()
    );
    tokio::time::sleep(RESTART_RESUME_SETTLE).await;
    let start = std::time::Instant::now();
    let mut injected: usize = 0;
    while !targets.is_empty() && start.elapsed() < RESTART_RESUME_DEADLINE {
        tokio::time::sleep(RESTART_RESUME_POLL).await;
        let live = {
            let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
            let mgr = session_mgr.read().await;
            mgr.list_sessions().await
        };
        let (remaining, ready) =
            partition_restart_resume_ready(std::mem::take(&mut targets), &live, |target| {
                crate::commands::pty::session_printed_since(
                    app,
                    target.session_id,
                    target.collected_at,
                )
            });
        targets = remaining;
        for target in &ready {
            inject_restart_resume_prompt(app, target).await;
            injected += 1;
        }
    }
    for target in &targets {
        log::warn!(
            "[restart-resume] '{}' ({}) never reached its prompt within {}s; nothing injected",
            target.name,
            target.working_directory,
            RESTART_RESUME_DEADLINE.as_secs()
        );
    }
    log::info!(
        "[restart-resume] complete: {} injected, {} timed out",
        injected,
        targets.len()
    );
}

/// (#1793) One injection. Root, exited, agentless and plain-shell rejection, the
/// write permit, the menu guard, the two staggered Enters and the agent-turn
/// arming all come from the canonical trusted-notice injector. This adds only
/// the exact-text validation that injector leaves to its callers.
async fn inject_restart_resume_prompt(app: &AppHandle, target: &RestartResumeTarget) {
    if let Err(error) = crate::pty::inject::validate_pty_input_text(&target.prompt) {
        log::warn!(
            "[restart-resume] '{}' prompt rejected by the PTY text validator ({}); nothing \
             injected",
            target.name,
            error
        );
        return;
    }
    let result = crate::pty::inject::inject_text_into_supported_agent_session_with_pre_write_check(
        app,
        target.session_id,
        &target.prompt,
        |_session| Ok(()),
    )
    .await;
    match result {
        Ok(()) => log::info!(
            "[restart-resume] injected into '{}' ({})",
            target.name,
            target.working_directory
        ),
        Err(error) => log::warn!(
            "[restart-resume] injection into '{}' failed: {}",
            target.name,
            error
        ),
    }
}

#[cfg(test)]
mod tests;

/// #2232 phase 4: the reader supervisor, its demands and the production wiring.
///
/// These tests drive the same SCC-member entry points production uses —
/// `raise_room_reader_demand_in`, `release_room_reader_demand` and the
/// supervisor in `commands::telegram` — over the phase-2 room fixture.
#[cfg(test)]
pub(crate) mod reader_demand_tests;

/// #2265 phase 2, test 15: the gathering function answers from an SCC member.
///
/// A later refactor that moves the body back into the Co-managed command module breaks
/// these tests rather than only the module-cycle gate.
#[cfg(test)]
pub(crate) mod co_managed_tests;
