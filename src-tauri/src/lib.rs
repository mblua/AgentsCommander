pub mod agent_update;
pub mod agent_version;
pub mod api;
pub mod capture;
pub mod cli;
pub mod commands;
pub mod config;
pub(crate) mod errors;
pub(crate) mod events;
pub(crate) mod filesystem;
pub mod logging;
pub(crate) mod loops;
pub mod network;
pub(crate) mod path_identity;
pub(crate) mod path_utils;
pub mod phone;
pub(crate) mod process;
pub mod pty;
pub(crate) mod repos;
pub mod resource_monitor;
pub mod screenshot;
pub mod session;
pub mod shutdown;
pub mod telegram;
pub mod test_support;
pub mod testability;
pub mod update_check;
pub mod voice;
pub mod web;

mod app_run;
mod app_state;
mod co_managed;
mod pty_wiring;
mod quit_gate;
mod reader_demand;
mod restore;
mod startup;
mod web_server;
mod window;

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use commands::ac_discovery::DiscoveryBranchWatcher;
use config::sessions_persistence;
use config::settings::SettingsState;
use pty::agent_quota::AgentQuotaEngine;
use pty::context_scrape::ContextScraper;
use pty::git_watcher::GitWatcher;
use pty::idle_detector::IdleDetector;
use pty::manager::PtyManager;
use pty::watchers::WatcherEngine;
use session::manager::SessionManager;
use shutdown::ShutdownSignal;
use telegram::manager::{OutputSenderMap, TelegramBridgeManager, TelegramBridgeState};
use voice::tracker::{VoiceTracker, VoiceTrackingState};
use web::auth::WebAccessToken;
use web::broadcast::WsBroadcaster;

pub use app_state::{AppOwners, ConfigSeedLockState, DetachedSessionsState, RestoreInProgress};
pub(crate) use co_managed::route::spawn_co_managed_supervisor;
#[cfg(test)]
pub(crate) use co_managed::CoManagedTestHooks;
#[cfg(test)]
pub(crate) use co_managed::{
    abstain_reason_label, co_managed_state_reason, co_managed_user_message, route_co_managed_wake,
    CoManagedRoute,
};
pub(crate) use co_managed::{emit_session_idle_edge, CoManagedSupervisorHandle, CoManagedTrigger};
pub(crate) use commands::quit_gate::{
    quit_application, quit_gate_progress, quit_gate_register, quit_gate_resolve,
    quit_gate_unregister,
};
pub(crate) use pty_wiring::{
    codex_activity_factory, QuotaRows, QuotaSink, QuotaSources, ScraperPatterns, ScraperPersist,
    ScraperRows, ScraperSamples, ScraperSink, WatcherBackends, WatcherDelivery, WatcherPatterns,
    WatcherSink,
};
pub use startup::AppOutbox;

pub use quit_gate::{
    apply_quit_gate_effects, quit_gate_run, quit_gate_window_gone, QuitAbortReason, QuitGate,
    QuitGateClock, QuitGateEffects, QuitGateHost, QuitGateRegistration, QuitOutcome,
    QuitOutcomeKind, TauriQuitHost, TokioQuitClock, QUIT_GATE_MAIN_LABEL, QUIT_GATE_TIMEOUT,
};
#[cfg(test)]
pub(crate) use reader_demand::{reader_demand_pass, recovery_pass, watch_capture_slots};
#[cfg(test)]
pub(crate) use reader_demand::{tick_raise_calls, tick_recheck_calls};
pub(crate) use restore::native::start_native_restore_startup;
pub(crate) use restore::{
    load_startup_sessions, normalize_persisted_active_flags, PersistedActiveFlagNormalization,
    RestoreObserverStartBarrier,
};
#[cfg(test)]
pub(crate) use restore::{
    resolve_is_coord_for_restore, restore_session_should_become_active,
    restore_session_should_wake, should_auto_create_root_agent_on_first_restore,
    should_wake_root_agent_on_restore, skip_auto_resume_for_restore, spawn_restore_startup,
};
pub(crate) use startup::prepare_app_outbox;
pub use startup::{log_naming_migration_summary, preflight_config_startup, StartupError};
pub(crate) use web_server::WEB_SERVER_START_CANCELLED;
pub use web_server::{
    ApiServerHandle, ApiServerTask, WebServerHandle, WebServerLifecycle,
    WebServerLifecycleSnapshot, WebServerStartWaiter, WebServerStopWaiter,
};
pub(crate) use window::{
    apply_main_display_state, centered_default_main_geometry, effective_main_display_state,
    is_visible_on_monitors, physical_to_logical, response_close_wiring_matches,
};

// Test-only names the `tests` tree reaches through `super::*`.
#[cfg(test)]
pub(crate) use co_managed::route::{
    co_managed_cycle, commit_co_managed_effect, handle_co_managed_trigger,
    resolve_co_managed_route, CoManagedOutcome,
};
#[cfg(test)]
pub(crate) use co_managed::{
    co_managed_candidate_label, utf8_safe_excerpt, CO_MANAGED_EXCERPT_BYTES,
};
#[cfg(test)]
use pty::agent_quota::source::SourceSpec;
#[cfg(test)]
use pty::agent_quota::{AgentQuotaSource, QuotaSourceProvider};
#[cfg(test)]
use pty::context_scrape::{ContextPatternSource, ContextSample, ContextSampleSink};
#[cfg(test)]
use pty::watchers::{WatcherEventSink, WatcherPatternSource};
#[cfg(test)]
pub(crate) use reader_demand::{
    release_room_reader_demands_when_globally_off, reraise_room_reader_demands,
};
#[cfg(test)]
pub(crate) use restore::{should_wake_on_restore, should_wake_working_agent_on_restore};
#[cfg(test)]
pub(crate) use startup::{
    preflight_config_startup_for, preflight_config_startup_in, take_naming_migration_summary,
    StartupErrorKind, INSTANCE_FAMILIES_RENAMES,
};

/// Snapshot scanner terminality is deliberately not an input here.
///
/// A retained scanner task publishes a response file; its only `SessionManager`
/// access is a read-lock clone, so it cannot leave session state inconsistent
/// and cannot make a session snapshot wrong. Gating persistence on it was not a
/// rare edge either: `SNAPSHOT_SERVER_TIMEOUT` is twice
/// `SHUTDOWN_CLEANUP_BUDGET_SECS`, and the drain correctly refuses to abort
/// owned or finalizer tasks, so any snapshot admitted inside the shutdown window
/// and running near its own legitimate deadline suppressed persistence and cost
/// the user their session list. Retained scanner work is still reported in the
/// shutdown diagnostics.
pub(crate) fn shutdown_persistence_allowed(
    selection_persistence_safe: bool,
    container_cleanup_terminal: bool,
) -> bool {
    selection_persistence_safe && container_cleanup_terminal
}

#[cfg(test)]
pub(crate) fn combined_shutdown_retained_diagnostics(
    selection_retained: Vec<String>,
    container_retained: Vec<String>,
) -> Vec<String> {
    combined_shutdown_retained_diagnostics_with_scanner(
        Vec::new(),
        selection_retained,
        container_retained,
    )
}

pub(crate) fn combined_shutdown_retained_diagnostics_with_scanner(
    scanner_retained: Vec<String>,
    selection_retained: Vec<String>,
    container_retained: Vec<String>,
) -> Vec<String> {
    let scanner = scanner_retained.into_iter().map(|context| {
        crate::pty::container_runtime::normalize_retained_owner_diagnostic(
            "terminalSnapshotScanner",
            context,
        )
    });
    let selection = selection_retained.into_iter().map(|context| {
        crate::pty::container_runtime::normalize_retained_owner_diagnostic("selection", context)
    });
    let container = container_retained.into_iter().map(|context| {
        crate::pty::container_runtime::normalize_retained_owner_diagnostic(
            "containerShutdown",
            context,
        )
    });
    crate::pty::container_runtime::cap_retained_owner_diagnostics(
        scanner.chain(selection).chain(container),
    )
}

fn remove_container_route_until(
    weak_pty_mgr: &std::sync::Weak<Mutex<PtyManager>>,
    session_id: uuid::Uuid,
    deadline: std::time::Instant,
) -> Result<(), crate::pty::container_backend::RouteRemovalError> {
    let Some(pty_mgr) = weak_pty_mgr.upgrade() else {
        return Ok(());
    };
    loop {
        if std::time::Instant::now() >= deadline {
            return Err(crate::pty::container_backend::RouteRemovalError::Deadline(
                "ptyManager",
            ));
        }
        match pty_mgr.try_lock() {
            Ok(pty_manager) => match pty_manager.try_remove_route_if_kind(
                session_id,
                crate::pty::backend::SessionBackendKind::ContainerTransport,
            ) {
                Ok(()) => return Ok(()),
                Err(crate::pty::manager::PtyRouteRemovalError::LockPoisoned) => {
                    return Err(
                        crate::pty::container_backend::RouteRemovalError::LockPoisoned(
                            "ptyRouteRegistry",
                        ),
                    );
                }
                Err(crate::pty::manager::PtyRouteRemovalError::Busy) => {}
            },
            Err(std::sync::TryLockError::Poisoned(_)) => {
                return Err(
                    crate::pty::container_backend::RouteRemovalError::LockPoisoned("ptyManager"),
                );
            }
            Err(std::sync::TryLockError::WouldBlock) => {}
        }
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            return Err(crate::pty::container_backend::RouteRemovalError::Deadline(
                "ptyManager",
            ));
        }
        std::thread::sleep(std::time::Duration::from_millis(2).min(remaining));
    }
}

pub(crate) fn install_container_route_remover(pty_mgr: &Arc<Mutex<PtyManager>>) {
    let weak_pty_mgr = Arc::downgrade(pty_mgr);
    let container_backend = pty_mgr.lock().unwrap().container_backend();
    container_backend.set_route_remover(Arc::new(move |session_id, deadline| {
        remove_container_route_until(&weak_pty_mgr, session_id, deadline)
    }));
}

// Issue #609 - cached "npm update available" result. Set ONCE by the startup
// check task; read by `get_update_status` so a late-mounting sidebar still
// sees a pending update.
pub type UpdateCheckState = Arc<std::sync::OnceLock<update_check::UpdateInfo>>;

/// Floating spec/Mermaid board document state.
pub type SpecBoardState = Arc<tokio::sync::RwLock<commands::spec_board::SpecBoardManager>>;

/// #632 - hard ceiling on the shutdown reaper cleanup. For jobbed sessions the Job
/// Object kill already prevented orphans, so exceeding this just stops the
/// best-effort accounting reaper. For a job-less session (assign failed) this bound
/// CAN abandon a still-dying tree, so the Exit handler warns when that is possible
/// (MED-2).
const SHUTDOWN_CLEANUP_BUDGET_SECS: u64 = 5;

/// Master token generated at app startup. Allows bypassing team validation (can_reach).
/// Persisted to `master-token.txt` in config_dir for CLI use. Regenerated on each app startup. See
/// #34.
/// Field is private — use `matches()` for constant-time comparison.
pub struct MasterToken(String);

impl MasterToken {
    pub fn new(token: String) -> Self {
        Self(token)
    }

    /// Constant-time comparison to prevent timing oracle attacks.
    pub fn matches(&self, candidate: &str) -> bool {
        let a = self.0.as_bytes();
        let b = candidate.as_bytes();
        if a.len() != b.len() {
            return false;
        }
        a.iter()
            .zip(b.iter())
            .fold(0u8, |acc, (x, y)| acc | (x ^ y))
            == 0
    }

    /// Display value (for printing to stdout at startup only).
    pub fn value(&self) -> &str {
        &self.0
    }
}

/// (#617/#668) Sessions with a self context operation awaiting their sustained-idle
/// window. Insert on queue; remove when the deferred task completes, the session
/// dies, or the safety cap expires. A session_id already present means a repeat
/// self operation is a no-op ("already_queued") - requests never stack.
/// In-memory only: a daemon restart drops pending requests (accepted, best-effort).
///
/// Newtype is mandatory: `DetachedSessionsState` (lib.rs:38) is already a managed
/// bare `Arc<Mutex<HashSet<Uuid>>>`, and Tauri keys managed state by Rust type, so
/// a second bare alias would collide. Mirrors `RestoreInProgress`.
#[derive(Default)]
pub struct PendingSelfClear(pub Mutex<HashSet<uuid::Uuid>>);

/// (#1652) Pins the type of the closure `generate_handler!` produces. The
/// macro's closure parameter is UNANNOTATED and rustc type-checks the body at
/// the `let` that binds it. A bare binding is therefore
/// `error[E0282]: type annotations needed`, and annotating the OUTER closure's
/// parameter does not reach it. This identity function supplies the expected
/// type on the binding itself. Preferred over
/// `let generated: Box<dyn Fn(..) -> bool + Send + Sync> = ...` because
/// `pty_write` is the highest-frequency command and boxing adds an indirection
/// to every invoke.
fn pin_handler<F: Fn(tauri::ipc::Invoke<tauri::Wry>) -> bool + Send + Sync + 'static>(f: F) -> F {
    f
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run(
    test_window_placement: Option<crate::testability::window_placement::TestWindowPlacement>,
    ui_automation_enabled: bool,
    owners: AppOwners,
) -> Result<(), StartupError> {
    app_run::run(test_window_placement, ui_automation_enabled, owners)
}

/// #2374 - the remote sweeper's production seams. The snapshot destination is
/// the instance config directory, resolved here and never inferred from
/// settings or another artifact.
pub(crate) fn remote_sweeper_seams() -> crate::pty::remote_watcher::RemoteSweeperSeams {
    crate::pty::remote_watcher::RemoteSweeper::production_seams(crate::config::config_dir())
}

// (#1652) Every invoke that reaches the app handler is stamped here,
// before dispatch: the only place in the process that sees the
// frontend -> backend direction as a stream, which is what lets
// `[ipc-observer] SILENCE` speak about the CHANNEL and not one command.
// Entry only - `generate_handler!` returns as soon as the command is
// dispatched, so there is no exit to hook. The exit side is the
// renderer's `settled` accounting (phase 2); the pair separates "never
// arrived" from "never came back".
//
// UNSTABLE API: `tauri::ipc::Invoke`'s own doc comment says it "is used
// internally by macros and is explicitly **NOT** stable"
// (tauri-2.10.3/src/ipc/mod.rs:209). `Cargo.lock` pins 2.10.3 and this
// wrapper is the ONLY place in the app depending on an explicitly
// unstable tauri type. The next tauri bump must re-check three things:
// `Invoke::message` public, `InvokeMessage::command()` public, and
// `generate_handler!` still expanding to a bindable closure expression.
//
// Blind spot, stated: Tauri routes plugin commands (`plugin:window|*`,
// `plugin:event|emit`, `plugin:dialog|open`) through `manager.extend_api`
// in a branch that never reaches `run_invoke_handler`
// (tauri-2.10.3/src/webview/mod.rs:1840-1890), so they never reach this
// closure - and the renderer's registry does not see them either, because
// `@tauri-apps/api`'s window/event/dialog helpers call
// `__TAURI_INTERNALS__.invoke` directly, not `TauriTransport.invoke`. The
// two sides therefore stay symmetric and no false "sent but never
// entered" signal is produced. The `[ipc-blackbox] coverage:` line repeats
// this in every record block.
fn invoke_handler(
    observer: Arc<crate::loops::ipc_observer::IpcObserver>,
) -> impl Fn(tauri::ipc::Invoke<tauri::Wry>) -> bool + Send + Sync + 'static {
    let generated = pin_handler(tauri::generate_handler![
        commands::session::create_session,
        commands::session::destroy_session,
        commands::session::close_coordinator,
        commands::session::restart_session,
        commands::session::resolve_blocking_menu,
        commands::session::switch_session,
        commands::session::rename_session,
        commands::session::set_last_prompt,
        commands::session::list_sessions,
        commands::session::get_active_session,
        commands::session::get_last_agent_message,
        commands::session_warnings::drain_session_warnings,
        quit_gate_register,
        quit_gate_unregister,
        quit_gate_resolve,
        quit_gate_progress,
        quit_application,
        commands::session::create_root_agent_session,
        commands::task::task_get_title,
        commands::task::task_set_title,
        commands::task::task_clean,
        commands::task::task_clean_at,
        commands::task::task_set_title_at,
        commands::pty::pty_write,
        commands::pty::get_typing_hold,
        commands::pty::toggle_typing_hold,
        commands::pty::pty_resize,
        commands::pty::get_screen_snapshot,
        commands::pty::activate_terminal_output,
        commands::pty::detach_terminal_output,
        commands::pty::get_session_context,
        commands::pty::get_agent_quota_readings,
        commands::pty::get_watcher_activity,
        commands::pty::preview_watcher_pattern,
        commands::pty::preview_watcher_reach,
        commands::config::get_settings,
        commands::config::list_agent_file_sources,
        commands::config::read_agent_file_source,
        commands::config::save_agent_file_source,
        commands::config::remember_agent_file_source,
        commands::config::get_agent_help,
        commands::config::get_coding_agent_catalog,
        commands::config::get_coding_agent_welcome_status,
        commands::config::install_coding_agent,
        commands::config::get_coding_agent_catalog_report,
        commands::config::list_reseedable_agent_commands,
        commands::config::reseed_coding_agent_default,
        commands::config::update_settings,
        commands::resource_monitor::get_resource_snapshot,
        commands::resource_monitor::kill_resource_group,
        commands::config::save_settings_draft,
        commands::config::set_terminal_snapshots_enabled,
        commands::config::update_coding_agent_profiles,
        commands::config::update_coding_agent_env_settings,
        commands::config::set_agent_default_profile,
        commands::config::set_instance_profile_override,
        commands::config::resolve_coding_agent_profile,
        commands::config::preview_coding_agent_profile_selection,
        commands::config::apply_coding_agent_profile_selection,
        commands::config::preview_selection_lock_removal,
        commands::config::apply_selection_lock_removal,
        commands::config::get_replica_selection_default,
        commands::config::set_replica_selection_default,
        commands::config::set_sounds_enabled,
        commands::config::set_theme_light,
        commands::config::set_main_resource_monitor_attached,
        commands::config::set_rail_collapse,
        commands::config::move_coding_agent,
        commands::config::reorder_coding_agent,
        commands::config::set_log_level,
        commands::config::get_update_status,
        commands::config::get_agent_update_status,
        commands::config::agent_update_answer,
        commands::config::agent_update_cancel,
        commands::config::agent_updates_cancel_all,
        commands::config::get_agent_update_overview,
        commands::co_managed::co_managed_get,
        commands::co_managed::co_managed_set_enabled,
        commands::co_managed::co_managed_effective_state,
        commands::repos::search_repos,
        commands::repos::git_remote_url,
        commands::telegram::telegram_attach,
        commands::telegram::telegram_detach,
        commands::telegram::telegram_list_bridges,
        commands::telegram::telegram_get_bridge,
        commands::telegram::telegram_send_test,
        commands::telegram::telegram_send_image,
        commands::testability::ui_automation_enabled,
        commands::testability::ui_automation_frontend_ready,
        commands::testability::ui_automation_complete,
        commands::window::detach_terminal,
        commands::window::attach_terminal,
        commands::window::list_detached_sessions,
        commands::window::set_detached_geometry,
        commands::window::set_watchers_geometry,
        commands::window::set_main_window_placement,
        commands::window::open_in_explorer,
        commands::window::open_spec_board_window,
        commands::window::open_resource_monitor_window,
        commands::window::dock_resource_monitor_window,
        commands::window::open_watchers_window,
        commands::window::get_watchers_scope,
        commands::window::open_external_url,
        commands::window::focus_main_window,
        commands::spec_board::spec_board_new,
        commands::spec_board::spec_board_pick_open,
        commands::spec_board::spec_board_open,
        commands::spec_board::spec_board_save,
        commands::spec_board::spec_board_pick_save,
        commands::spec_board::spec_board_update_content,
        commands::spec_board::spec_board_list_snapshots,
        commands::spec_board::spec_board_checkout_snapshot,
        commands::spec_board::spec_board_apply_external,
        commands::spec_board::spec_board_keep_mine,
        commands::spec_board::spec_board_close,
        commands::voice::voice_transcribe,
        commands::voice::voice_mark_recording,
        commands::voice::voice_had_typing,
        commands::config::save_debug_logs,
        commands::config::append_rail_diagnostic,
        commands::config::drain_error_logs,
        commands::config::get_error_popup_mutes,
        commands::config::set_error_popup_muted,
        commands::config::open_web_remote,
        commands::config::start_api_server,
        commands::config::stop_api_server,
        commands::config::api_server_status,
        commands::config::mint_api_client,
        commands::config::start_web_server,
        commands::config::stop_web_server,
        commands::config::get_web_server_status,
        commands::config::get_web_server_owned_status,
        commands::config::list_web_server_interfaces,
        commands::config::get_instance_label,
        commands::config::fetch_home_markdown,
        commands::agent_creator::pick_folder,
        commands::agent_creator::create_agent_folder,
        commands::ac_discovery::task_get_snapshot,
        commands::ac_discovery::task_get_snapshot_at,
        commands::ac_discovery::discover_ac_agents,
        commands::ac_discovery::check_project_path,
        commands::ac_discovery::create_ac_project,
        commands::ac_discovery::open_project,
        commands::ac_discovery::new_project,
        commands::ac_discovery::remove_project,
        commands::ac_discovery::archive_project,
        commands::ac_discovery::unarchive_project,
        commands::ac_discovery::list_archived_projects,
        commands::ac_discovery::discover_project,
        commands::ac_discovery::get_project_file_difference,
        commands::project_settings::get_project_groups,
        commands::project_settings::update_project_groups,
        commands::non_stop::non_stop_report,
        commands::ipc_blackbox::ipc_blackbox_report,
        commands::ac_discovery::keep_custom_context_template,
        commands::ac_discovery::overwrite_context_template_with_default,
        commands::ac_discovery::get_replica_context_files,
        commands::ac_discovery::set_replica_context_files,
        commands::loops::create_loop,
        commands::loops::update_loop,
        commands::loops::delete_loop,
        commands::loops::toggle_loop,
        commands::loops::run_loop_now,
        commands::loops::get_loop_config,
        commands::loops::list_unresolved_loop_targets,
        commands::loops::preview_loop_cron,
        commands::entity_creation::create_agent_matrix,
        commands::entity_creation::delete_agent_matrix,
        commands::entity_creation::list_all_agents,
        commands::entity_creation::create_team,
        commands::entity_creation::delete_team,
        commands::entity_creation::update_team,
        commands::entity_creation::get_team_config,
        commands::entity_creation::create_workgroup,
        commands::entity_creation::delete_workgroup,
        commands::role_templates::list_role_templates,
        commands::role_templates::get_agency_templates_status,
        commands::role_templates::update_agency_templates,
        commands::screenshot::screenshot_get_overlay_state,
        commands::screenshot::screenshot_confirm_selection,
        commands::screenshot::screenshot_cancel_capture,
        commands::screenshot::screenshot_get_hotkey_status,
        commands::screenshot::screenshot_reload_hotkey,
    ]);
    move |invoke: tauri::ipc::Invoke<tauri::Wry>| {
        observer.note_invoke(invoke.message.command());
        generated(invoke)
    }
}

#[cfg(test)]
mod tests;

impl testability::ui_automation::UiAutomationHost for tauri::AppHandle {
    fn available_windows(&self) -> Vec<String> {
        testability::ui_automation::liveness::available_window_labels(self)
    }
    fn spawn_background(&self, future: futures::future::BoxFuture<'static, ()>) {
        tauri::async_runtime::spawn(future);
    }
    fn backend_request<'a>(
        &'a self,
        request: &'a testability::ui_automation::UiAutomationRequest,
    ) -> futures::future::BoxFuture<'a, testability::ui_automation::UiAutomationResponse> {
        Box::pin(
            testability::ui_automation::backend_watchdog::handle_backend_request(self, request),
        )
    }
}

impl<R: tauri::Runtime> testability::ui_automation::backend_watchdog::BackendWatchdogHost
    for tauri::AppHandle<R>
{
    fn resource_monitor(&self) -> Option<Arc<resource_monitor::ResourceMonitorState>> {
        use tauri::Manager;
        self.try_state::<Arc<resource_monitor::ResourceMonitorState>>()
            .map(|state| Arc::clone(state.inner()))
    }
    fn selection_coordinator(&self) -> Option<session::selection::SelectionCoordinator> {
        use tauri::Manager;
        self.try_state::<session::selection::SelectionCoordinator>()
            .map(|state| state.inner().clone())
    }
}

impl<R: tauri::Runtime> update_check::UpdateCheckHost for tauri::AppHandle<R> {
    fn update_settings(&self) -> config::settings::SettingsState {
        Clone::clone(tauri::Manager::state::<SettingsState>(self).inner())
    }
    fn outbound_network(&self) -> network::OutboundNetwork {
        Clone::clone(tauri::Manager::state::<network::OutboundNetwork>(self).inner())
    }
}

impl restore::RestoreStartupTask for tauri::async_runtime::JoinHandle<()> {
    type Error = tauri::Error;
    fn spawn_owned(future: futures::future::BoxFuture<'static, ()>) -> Self {
        tauri::async_runtime::spawn(future)
    }
}

impl resource_monitor::watchdog::ResourceWatchdogTaskScope for ShutdownSignal {
    fn token(&self) -> &tokio_util::sync::CancellationToken {
        ShutdownSignal::token(self)
    }
    fn spawn_owned(&self, future: futures::future::BoxFuture<'static, ()>) {
        tauri::async_runtime::spawn(future);
    }
}
