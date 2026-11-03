//! Opt-in web and API server autostart for the `.setup` hook.

use std::sync::{Arc, Mutex};

use tauri::Manager;

use crate::{
    api, commands, config, ApiServerHandle, ApiServerTask, PtyManager, SessionManager,
    SettingsState, ShutdownSignal, WebAccessToken, WebServerHandle, WsBroadcaster,
    WEB_SERVER_START_CANCELLED,
};

/// The owned values the web server start consumes.
pub(super) struct WebServerStart {
    pub(super) settings: SettingsState,
    pub(super) web_token: Arc<WebAccessToken>,
    pub(super) session_mgr: Arc<tokio::sync::RwLock<SessionManager>>,
    pub(super) broadcaster: WsBroadcaster,
}

// Start web server if enabled in settings
pub(super) fn start_web_server(
    app: &tauri::App,
    start: WebServerStart,
    pty_mgr: &Arc<Mutex<PtyManager>>,
    shutdown: &ShutdownSignal,
    web_access_token: &WebAccessToken,
) -> Result<(), Box<dyn std::error::Error>> {
    register_web_command_dispatcher(
        app,
        &start.session_mgr,
        pty_mgr,
        &start.settings,
        &start.broadcaster,
    )?;
    let web_settings = config::settings::load_settings();
    if web_settings.web_server_enabled {
        let ws_handle = app.state::<WebServerHandle>().inner().clone();
        let waiter = commands::config::begin_web_server_start(
            ws_handle.clone(),
            start.settings,
            start.web_token,
            start.session_mgr,
            pty_mgr.clone(),
            start.broadcaster,
            app.handle().clone(),
            shutdown.clone(),
        );
        match tauri::async_runtime::block_on(waiter.wait()) {
            Ok(true) => {
                if let Some((bind, port)) = ws_handle.snapshot().endpoint {
                    println!(
                        "[web-token] Remote URL: \
                            http://{}:{}/?window=main&remoteToken={}",
                        bind,
                        port,
                        web_access_token.value()
                    );
                }
            }
            Ok(false) => {}
            Err(error) if error == WEB_SERVER_START_CANCELLED => {
                log::info!("[web-server] autostart cancelled by lifecycle stop");
            }
            Err(error) => {
                log::warn!("[web-server] startup failed: {}", error);
            }
        }
    }
    Ok(())
}

// #791 - start the control-plane API server if enabled in settings.
// Opt-in (default false), mirroring the web server block above. The
// managed handle is stored only after bind readiness is confirmed.
pub(super) fn start_api_server(
    app: &tauri::App,
    session_mgr_for_api: &Arc<tokio::sync::RwLock<SessionManager>>,
    pty_mgr: &Arc<Mutex<PtyManager>>,
    shutdown: &ShutdownSignal,
) {
    let api_settings = config::settings::load_settings();
    if api_settings.api_server_enabled {
        let bind = api_settings.api_server_bind.clone();
        let port = api_settings.api_server_port;
        let api_shutdown = shutdown.token().child_token();
        let server_start = api::start_server(
            bind,
            port,
            app.handle().clone(),
            session_mgr_for_api.clone(),
            pty_mgr.clone(),
            api_shutdown.clone(),
        );
        match tauri::async_runtime::block_on(api::wait_for_startup_ready(server_start.readiness)) {
            Ok(bound_addr) => {
                let api_handle = app.state::<ApiServerHandle>();
                if let Err(e) = api_handle.store_if_idle(ApiServerTask::new(
                    server_start.join_handle,
                    api_shutdown,
                    bound_addr,
                )) {
                    log::error!("[api-server] failed to store server handle: {}", e);
                }
            }
            Err(err) => {
                api_shutdown.cancel();
                log::warn!("[api-server] startup failed: {}", err);
            }
        }
    }
}

fn register_web_command_dispatcher(
    app: &tauri::App,
    session_mgr_for_web: &Arc<tokio::sync::RwLock<SessionManager>>,
    pty_mgr: &Arc<Mutex<PtyManager>>,
    settings_for_web: &SettingsState,
    broadcaster_for_web: &WsBroadcaster,
) -> Result<(), Box<dyn std::error::Error>> {
    let dispatcher =
        crate::web::commands::WebCommandsDispatcher::new(crate::web::commands::WsState::new(
            Arc::clone(session_mgr_for_web),
            Arc::clone(pty_mgr),
            Arc::clone(settings_for_web),
            broadcaster_for_web.clone(),
            app.handle().clone(),
        ));
    if !app.manage(crate::web::command_port::ManagedWebCommandDispatcher(
        Arc::new(dispatcher),
    )) {
        return Err("Web command dispatcher is already registered".into());
    }
    Ok(())
}
